use super::*;
use crate::archive::{
    Batch,
    tests::{fixture, record},
};
use tokio::sync::{Notify, mpsc};

struct Fake {
    entered: mpsc::UnboundedSender<()>,
    release: Notify,
    fail: bool,
}
#[async_trait::async_trait]
impl Source for Fake {
    async fn change(&self, target: Target, action: Action) -> Result<Update, Failure> {
        self.entered.send(()).unwrap();
        self.release.notified().await;
        if self.fail {
            Err(Failure::Unconfirmed)
        } else {
            Ok(action.update(target.jid, 10))
        }
    }
}
async fn seed() -> (tempfile::TempDir, Archive, i32) {
    let (temp, db) = fixture();
    db.ingest(Batch {
        messages: vec![record("one")],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db
        .view(0, 100, String::new(), false, false)
        .await
        .unwrap()
        .chats[0]
        .id;
    (temp, db, chat)
}

#[tokio::test]
async fn a_connection_published_before_the_click_is_not_a_disconnect() {
    let (_temp, db, chat) = seed().await;
    let (entered, _entries) = mpsc::unbounded_channel();
    let fake = Arc::new(Fake {
        entered,
        release: Notify::new(),
        fail: false,
    });
    let (sources, receiver) = watch::channel(None);
    // Real startup creates the watch while offline, then publishes the client.
    // The messaging receiver hasn't consumed that notification before the click.
    sources.send_replace(Some(fake.clone() as Arc<dyn Source>));
    fake.release.notify_one();
    assert_eq!(
        request(db.clone(), receiver, chat, Action::MuteAlways).await,
        Ok("Chat muted.")
    );
    let view = db.view(0, 100, String::new(), false, false).await.unwrap();
    assert!(view.chats[0].muted);
    assert_eq!(view.chats[0].unread, 1);
    assert_eq!(view.unread_chats, 0);
    assert_eq!(
        Arc::strong_count(&fake),
        2,
        "the finished action releases its client"
    );
}

#[tokio::test]
async fn an_unconfirmed_or_disconnected_action_keeps_the_saved_chat_state() {
    for fail in [false, true] {
        let (_temp, db, chat) = seed().await;
        let (entered, mut entries) = mpsc::unbounded_channel();
        let fake = Arc::new(Fake {
            entered,
            release: Notify::new(),
            fail,
        });
        let (sources, rx) = watch::channel(Some(fake.clone() as Arc<dyn Source>));
        let task = tokio::spawn(request(db.clone(), rx, chat, Action::MuteAlways));
        entries.recv().await.unwrap();
        assert_eq!(
            db.view(0, 100, String::new(), false, false)
                .await
                .unwrap()
                .unread_chats,
            1
        );
        if fail {
            fake.release.notify_one();
        } else {
            sources.send_replace(None);
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        sources.send_replace(None);
        assert_eq!(
            Arc::strong_count(&fake),
            1,
            "the completed future must release its source"
        );
        assert_eq!(
            db.view(0, 100, String::new(), false, false)
                .await
                .unwrap()
                .unread_chats,
            1
        );
    }
}

#[tokio::test]
async fn a_management_request_stays_responsive_until_completion_or_window_close() {
    for finish in [false, true] {
        let (_temp, db, chat) = seed().await;
        let (entered, mut entries) = mpsc::unbounded_channel();
        let fake = Arc::new(Fake {
            entered,
            release: Notify::new(),
            fail: false,
        });
        let (source_updates, sources) = watch::channel(None);
        let (archives, rx) = watch::channel(None);
        let (commands, command_rx) = mpsc::channel(16);
        let (updates, mut views) = watch::channel(crate::messaging::Snapshot::empty());
        let task = tokio::spawn(crate::messaging::run_with_chat_sources(
            rx,
            command_rx,
            updates,
            watch::channel(None).1,
            watch::channel(crate::preferences::Preferences::default()).1,
            sources,
        ));
        archives.send_replace(Some(db.clone()));
        source_updates.send_replace(Some(fake.clone() as Arc<dyn Source>));
        tokio::time::timeout(Duration::from_secs(2), views.changed())
            .await
            .unwrap()
            .unwrap();
        commands
            .send(crate::messaging::Command::Select {
                id: chat,
                previous: 0,
                draft: String::new(),
                draft_version: 0,
                revision: 1,
            })
            .await
            .unwrap();
        loop {
            views.changed().await.unwrap();
            if views
                .borrow()
                .view
                .as_ref()
                .is_some_and(|v| v.conversation.is_some())
            {
                break;
            }
        }
        commands
            .send(crate::messaging::Command::ManageChat {
                chat,
                action: Action::MuteAlways,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), entries.recv())
            .await
            .unwrap()
            .unwrap();
        commands
            .send(crate::messaging::Command::Draft(
                chat,
                "Typed during a slow action".into(),
                1,
            ))
            .await
            .unwrap();
        // Autosave deliberately avoids rebuilding the view. A following command
        // must still complete while the remote action waits.
        commands
            .send(crate::messaging::Command::Search(String::new()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                views.changed().await.unwrap();
                if views
                    .borrow()
                    .view
                    .as_ref()
                    .and_then(|v| v.conversation.as_ref())
                    .is_some_and(|c| c.draft == "Typed during a slow action")
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(views.borrow().chat_busy);
        assert!(
            !views.borrow().view.as_ref().unwrap().chats[0].muted,
            "a slow remote action must not optimistically change the sidebar"
        );
        if finish {
            for muted in [true, false] {
                if !muted {
                    commands
                        .send(crate::messaging::Command::ManageChat {
                            chat,
                            action: Action::Unmute,
                        })
                        .await
                        .unwrap();
                    tokio::time::timeout(Duration::from_secs(2), entries.recv())
                        .await
                        .unwrap()
                        .unwrap();
                }
                fake.release.notify_one();
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        let done = {
                            let view = views.borrow_and_update();
                            !view.chat_busy
                                && view
                                    .view
                                    .as_ref()
                                    .is_some_and(|v| v.chats[0].muted == muted)
                        };
                        if done {
                            break;
                        }
                        views.changed().await.unwrap();
                    }
                })
                .await
                .unwrap();
                let view = views.borrow();
                let view = view.view.as_ref().unwrap();
                assert_eq!(view.chats[0].unread, 1);
                assert_eq!(view.unread_chats, if muted { 0 } else { 1 });
            }
        }
        drop(commands);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            Arc::strong_count(&fake),
            2,
            "only the test and source watch retain it, not the action"
        );
    }
}

#[test]
fn history_mute_units_and_requested_durations_are_explicit() {
    assert_eq!(history_mute(0), 0);
    assert_eq!(history_mute(u64::MAX), -1);
    assert_eq!(history_mute(1_700_000_000), 1_700_000_000_000);
    assert_eq!(history_mute(1_700_000_000_000), 1_700_000_000_000);
    assert_eq!(
        Action::MuteHours(8).update("synthetic".into(), 100).value,
        28_800_100
    );
    assert!(Action::from_menu(7).is_none());
}
