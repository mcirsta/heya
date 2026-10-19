//! The messaging worker must remain usable while a phone is silent.
use super::*;
use crate::{
    archive::tests::{PN, fixture, record},
    phone_history::{Anchor, Failure, Source},
};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct Silent {
    started: mpsc::UnboundedSender<()>,
    cancelled: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl Source for Silent {
    async fn fetch(&self, _: &Anchor) -> Result<String, Failure> {
        let _guard = Guard(self.cancelled.clone());
        self.started.send(()).unwrap();
        std::future::pending().await
    }
}
async fn until(rx: &mut watch::Receiver<Snapshot>, test: impl Fn(&Snapshot) -> bool) -> Snapshot {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
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
async fn phone_requests_coalesce_without_blocking_drafts_navigation_or_shutdown() {
    let (_temp, db) = fixture();
    db.ingest(crate::archive::Batch {
        messages: vec![record("anchor")],
        ..Default::default()
    })
    .await
    .unwrap();
    let second = db
        .create_chat("15550000002@s.whatsapp.net".into())
        .await
        .unwrap();
    let (archives, archive_rx) = watch::channel(None);
    let (commands, command_rx) = mpsc::channel(32);
    let (updates, mut views) = watch::channel(Snapshot::empty());
    let (started, mut starts) = mpsc::unbounded_channel();
    let cancelled = Arc::new(AtomicUsize::new(0));
    let (sources, source_rx) = watch::channel(Some(Arc::new(Silent {
        started,
        cancelled: cancelled.clone(),
    }) as Arc<dyn Source>));
    let worker = tokio::spawn(run_with_history(archive_rx, command_rx, updates, source_rx));
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
    until(&mut views, |s| s.revision == 1 && s.phone.available).await;
    commands
        .send(Command::Older {
            chat: 1,
            request: 1,
            revision: 1,
        })
        .await
        .unwrap();
    starts.recv().await.unwrap();
    until(&mut views, |s| s.phone.loading).await;
    for _ in 0..10 {
        commands
            .send(Command::FetchPhone {
                chat: 1,
                revision: 1,
            })
            .await
            .unwrap();
    }
    commands
        .send(Command::Draft(1, "Draft during phone history".into(), 1))
        .await
        .unwrap();
    commands
        .send(Command::Navigation(
            crate::message_navigation::Command::Navigate {
                chat: 1,
                message: 1,
                quote: false,
                request: 1,
                revision: 1,
            },
        ))
        .await
        .unwrap();
    let cancelled_page = until(&mut views, |s| {
        s.jump.request == 1 && s.history.is_some_and(|h| h.request == 1)
    })
    .await;
    assert!(!cancelled_page.phone.loading);
    assert!(
        !cancelled_page.history.unwrap().success,
        "navigation must release the timeline's pending history request"
    );
    commands
        .send(Command::Select {
            id: second,
            previous: 1,
            draft: "Draft during phone history".into(),
            draft_version: 1,
            revision: 2,
        })
        .await
        .unwrap();
    until(&mut views, |s| {
        s.revision == 2
            && s.view
                .as_ref()
                .and_then(|v| v.conversation.as_ref())
                .is_some_and(|c| c.chat.id == second)
    })
    .await;
    assert_eq!(cancelled.load(Ordering::SeqCst), 1);
    assert!(
        starts.try_recv().is_err(),
        "duplicate requests must not start new transport futures"
    );
    let saved = db.view(1, 100, String::new(), false, false).await.unwrap();
    assert_eq!(
        saved.conversation.unwrap().draft,
        "Draft during phone history"
    );
    // Reopening the same chat cannot duplicate an already transmitted request
    // whose waiting future was cancelled by navigation.
    commands
        .send(Command::Select {
            id: 1,
            previous: second,
            draft: String::new(),
            draft_version: 0,
            revision: 3,
        })
        .await
        .unwrap();
    until(&mut views, |s| s.revision == 3 && s.view.is_some()).await;
    commands
        .send(Command::FetchPhone {
            chat: 1,
            revision: 3,
        })
        .await
        .unwrap();
    until(&mut views, |s| !s.notice.is_empty()).await;
    assert!(starts.try_recv().is_err());
    drop(commands);
    tokio::time::timeout(std::time::Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap();
    drop((sources, archives));
    assert!(db.create_chat(PN.into()).await.is_ok());
}
