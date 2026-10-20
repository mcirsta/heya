use super::*;
use crate::{
    archive::{
        Batch,
        tests::{fixture, record},
    },
    message_navigation::Command as Nav,
};
async fn until(rx: &mut watch::Receiver<Snapshot>, test: impl Fn(&Snapshot) -> bool) -> Snapshot {
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            let s = rx.borrow_and_update().clone();
            if test(&s) {
                return s;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn search_refreshes_and_navigation_retains_drafts_without_marking_hidden_latest_read() {
    let (_temp, db) = fixture();
    db.ingest(Batch {
        messages: (0..240)
            .map(|i| {
                let mut r = record(&format!("search-{i}"));
                r.timestamp += i;
                r
            })
            .collect(),
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let (archives, rx) = watch::channel(None);
    let (commands, receiver) = mpsc::channel(16);
    let (updates, mut views) = watch::channel(Snapshot::empty());
    let worker = tokio::spawn(run(rx, receiver, updates));
    archives.send_replace(Some(db.clone()));
    until(&mut views, |s| s.view.is_some()).await;
    commands
        .send(Command::Select {
            id: 1,
            previous: 0,
            draft: String::new(),
            draft_version: 0,
            revision: 1,
        })
        .await
        .unwrap();
    until(&mut views, |s| s.revision == 1).await;
    commands
        .send(Command::Draft(1, "Unsent draft 👩🏽‍💻".into(), 1))
        .await
        .unwrap();
    commands
        .send(Command::Navigation(Nav::Search {
            chat: 1,
            query: "search-19".into(),
            request: 1,
            revision: 1,
        }))
        .await
        .unwrap();
    let s = until(&mut views, |s| s.search.request == 1).await;
    assert_eq!(s.search.hits.len(), 11);
    commands
        .send(Command::Navigation(Nav::Navigate {
            chat: 1,
            message: 20,
            quote: false,
            request: 1,
            revision: 1,
        }))
        .await
        .unwrap();
    let s = until(&mut views, |s| s.jump.request == 1).await;
    let c = s.view.unwrap().conversation.as_ref().unwrap().clone();
    assert!(s.jump.browsing);
    assert_eq!(s.jump.message, 20);
    assert_eq!(c.messages.len(), 70);
    assert_eq!(c.draft, "Unsent draft 👩🏽‍💻");
    assert_eq!(c.chat.unread, 240);
    // Defensive service guard: even an incorrectly queued visibility command
    // cannot notify senders or clear private unread state in an old window.
    commands
        .send(Command::ReadVisible {
            chat: 1,
            message: 240,
            revision: 1,
        })
        .await
        .unwrap();
    db.call(|c| {
        c.execute("UPDATE messages SET body='Edited content' WHERE id=20", [])?;
        Ok(())
    })
    .await
    .unwrap();
    db.changed();
    let s = until(&mut views, |s| {
        s.search.request == 1 && s.search.hits.len() == 10
    })
    .await;
    assert_eq!(
        s.view.unwrap().conversation.as_ref().unwrap().chat.unread,
        240
    );
    commands
        .send(Command::Navigation(Nav::Navigate {
            chat: 1,
            message: 0,
            quote: false,
            request: 2,
            revision: 1,
        }))
        .await
        .unwrap();
    let s = until(&mut views, |s| s.jump.request == 2).await;
    assert!(s.jump.returning);
    assert!(!s.jump.browsing);
    assert_eq!(
        s.view
            .as_ref()
            .unwrap()
            .conversation
            .as_ref()
            .unwrap()
            .messages[0]
            .id,
        141
    );
    // Missing originals are acknowledged without replacing the current window.
    commands
        .send(Command::Navigation(Nav::Navigate {
            chat: 1,
            message: 9999,
            quote: false,
            request: 3,
            revision: 1,
        }))
        .await
        .unwrap();
    let s = until(&mut views, |s| s.jump.request == 3).await;
    assert_eq!(s.jump.message, 0);
    assert!(!s.notice.is_empty());
    commands
        .send(Command::Navigation(Nav::Search {
            chat: 1,
            query: "Edited content".into(),
            request: 2,
            revision: 0,
        }))
        .await
        .unwrap();
    commands
        .send(Command::Navigation(Nav::Search {
            chat: 1,
            query: "Edited content".into(),
            request: 3,
            revision: 1,
        }))
        .await
        .unwrap();
    let s = until(&mut views, |s| s.search.request == 3).await;
    assert_eq!(s.search.hits.len(), 1);
    assert_eq!(s.search.hits[0].id, 20);
    drop(commands);
    worker.await.unwrap();
}

#[tokio::test]
async fn folders_reset_navigation_and_a_synced_lock_clears_search_results() {
    use crate::archive::tests::PN;
    use crate::chat_management::{Setting, Update};
    let (_temp, db) = fixture();
    db.ingest(Batch {
        messages: (0..160)
            .map(|i| {
                let mut r = record(&format!("folder-{i}"));
                r.timestamp += i;
                r
            })
            .collect(),
        ..Default::default()
    })
    .await
    .unwrap();
    db.save_chat_settings(vec![Update {
        jid: PN.into(),
        setting: Setting::Archive,
        value: 1,
        stamp: Some(1),
    }])
    .await
    .unwrap();
    let (archives, rx) = watch::channel(None);
    let (commands, receiver) = mpsc::channel(16);
    let (updates, mut views) = watch::channel(Snapshot::empty());
    let worker = tokio::spawn(run(rx, receiver, updates));
    archives.send_replace(Some(db.clone()));
    until(&mut views, |s| s.view.is_some()).await;
    commands
        .send(Command::Folder {
            archived: true,
            previous: 0,
            draft: String::new(),
            draft_version: 0,
            revision: 1,
        })
        .await
        .unwrap();
    until(&mut views, |s| s.revision == 1).await;
    commands
        .send(Command::Select {
            id: 1,
            previous: 0,
            draft: String::new(),
            draft_version: 0,
            revision: 2,
        })
        .await
        .unwrap();
    until(&mut views, |s| s.revision == 2).await;
    commands
        .send(Command::Navigation(Nav::Navigate {
            chat: 1,
            message: 10,
            quote: false,
            request: 1,
            revision: 2,
        }))
        .await
        .unwrap();
    let s = until(&mut views, |s| s.jump.request == 1).await;
    let view = s.view.unwrap();
    assert!(view.archived_view);
    assert!(s.jump.browsing);
    assert_eq!(view.conversation.as_ref().unwrap().messages.len(), 60);
    commands
        .send(Command::Folder {
            archived: false,
            previous: 1,
            draft: "Keep the archived draft".into(),
            draft_version: 1,
            revision: 3,
        })
        .await
        .unwrap();
    let s = until(&mut views, |s| s.revision == 3).await;
    assert!(!s.jump.browsing);
    assert!(s.view.unwrap().conversation.is_none());
    commands
        .send(Command::Folder {
            archived: true,
            previous: 0,
            draft: String::new(),
            draft_version: 0,
            revision: 4,
        })
        .await
        .unwrap();
    until(&mut views, |s| s.revision == 4).await;
    commands
        .send(Command::Select {
            id: 1,
            previous: 0,
            draft: String::new(),
            draft_version: 0,
            revision: 5,
        })
        .await
        .unwrap();
    let s = until(&mut views, |s| s.revision == 5).await;
    let c = s.view.unwrap().conversation.clone().unwrap();
    assert_eq!(c.messages[0].id, 61, "reopen at the latest window");
    assert_eq!(c.draft, "Keep the archived draft");
    commands
        .send(Command::Navigation(Nav::Search {
            chat: 1,
            query: "folder-1".into(),
            request: 1,
            revision: 5,
        }))
        .await
        .unwrap();
    let s = until(&mut views, |s| s.search.request == 1).await;
    assert!(!s.search.hits.is_empty());
    db.save_chat_settings(vec![Update {
        jid: PN.into(),
        setting: Setting::Lock,
        value: 1,
        stamp: Some(2),
    }])
    .await
    .unwrap();
    let s = until(&mut views, |s| {
        s.view.as_ref().is_some_and(|v| v.conversation.is_none())
    })
    .await;
    assert!(s.search.hits.is_empty());
    assert!(!s.jump.browsing);
    assert!(db.search_messages(1, "folder".into(), 2).await.is_err());
    assert!(db.navigation_target(1, 10, false).await.is_err());
    drop(commands);
    worker.await.unwrap();
}
