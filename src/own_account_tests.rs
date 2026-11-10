use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Semaphore;

#[derive(PartialEq, Eq, Debug)]
enum Call {
    Presence(bool),
    Load,
    Save(String),
}
struct Fake {
    calls: mpsc::UnboundedSender<Call>,
    permits: Semaphore,
    fail: AtomicBool,
    active: AtomicUsize,
    about: Mutex<String>,
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl Source for Fake {
    async fn presence(&self, mode: Visibility) -> Result<(), &'static str> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(&self.active);
        self.calls
            .send(Call::Presence(mode == Visibility::Invisible))
            .unwrap();
        self.permits.acquire().await.unwrap().forget();
        if self.fail.load(Ordering::SeqCst) {
            Err("Synthetic presence failure")
        } else {
            Ok(())
        }
    }
    async fn about(&self) -> Result<String, &'static str> {
        self.calls.send(Call::Load).unwrap();
        Ok(self.about.lock().unwrap().clone())
    }
    async fn save_about(&self, text: &str) -> Result<(), &'static str> {
        self.calls.send(Call::Save(text.into())).unwrap();
        if self.fail.load(Ordering::SeqCst) {
            return Err("Synthetic About failure");
        }
        *self.about.lock().unwrap() = text.into();
        Ok(())
    }
}
fn fake() -> (Arc<Fake>, mpsc::UnboundedReceiver<Call>) {
    let (calls, events) = mpsc::unbounded_channel();
    (
        Arc::new(Fake {
            calls,
            permits: Semaphore::new(0),
            fail: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            about: Mutex::new("Synthetic About 👋".into()),
        }),
        events,
    )
}

#[tokio::test]
async fn a_stalled_presence_send_has_a_deadline_and_releases_its_source() {
    let (fake, mut calls) = fake();
    let (_sources, source) = watch::channel(Some(fake.clone() as Arc<dyn Source>));
    let (_prefs, preferences) = watch::channel(crate::preferences::Preferences::default());
    let (commands, requests) = mpsc::channel(4);
    let (updates, mut states) = watch::channel(Snapshot::default());
    let task = tokio::spawn(run(requests, source, preferences, updates));
    assert_eq!(calls.recv().await, Some(Call::Presence(false)));
    tokio::time::timeout(Duration::from_secs(14), async {
        while states.borrow().busy || states.borrow().warning.is_empty() {
            states.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert!(states.borrow().visibility.is_none());
    drop(commands);
    task.await.unwrap();
}
async fn until(snapshots: &mut watch::Receiver<Snapshot>, predicate: impl Fn(&Snapshot) -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate(&snapshots.borrow_and_update()) {
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn visibility_is_confirmed_after_send_and_reconnect_honors_invisible() {
    let (fake, mut calls) = fake();
    let (sources, source) = watch::channel(Some(fake.clone() as Arc<dyn Source>));
    let (prefs, preferences) = watch::channel(crate::preferences::Preferences {
        appear_offline: true,
        ..Default::default()
    });
    let (commands, requests) = mpsc::channel(4);
    let (updates, mut states) = watch::channel(Snapshot::default());
    let task = tokio::spawn(run(requests, source, preferences, updates));
    assert_eq!(calls.recv().await, Some(Call::Presence(true)));
    assert!(states.borrow().busy && states.borrow().visibility.is_none());
    fake.permits.add_permits(1);
    until(&mut states, |s| {
        s.visibility == Some(Visibility::Invisible) && !s.busy
    })
    .await;
    commands
        .send(Command {
            request: 1,
            action: Action::Presence(Visibility::Online),
        })
        .await
        .unwrap();
    assert_eq!(calls.recv().await, Some(Call::Presence(false)));
    fake.fail.store(true, Ordering::SeqCst);
    fake.permits.add_permits(1);
    until(&mut states, |s| {
        s.reply.as_ref().is_some_and(|r| r.request == 1)
    })
    .await;
    assert!(states.borrow().visibility == Some(Visibility::Invisible));
    assert_eq!(
        states.borrow().reply.as_ref().unwrap().error,
        "Synthetic presence failure"
    );
    sources.send_replace(None);
    until(&mut states, |s| !s.connected).await;
    sources.send_replace(Some(fake.clone()));
    assert_eq!(calls.recv().await, Some(Call::Presence(true)));
    sources.send_replace(None);
    until(&mut states, |s| !s.connected).await;
    assert_eq!(
        fake.active.load(Ordering::SeqCst),
        0,
        "disconnect must drop the in-flight operation"
    );
    // Automatic source updates retain the user's completion across coalescing.
    assert_eq!(states.borrow().reply.as_ref().unwrap().request, 1);
    drop(commands);
    drop(prefs);
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn about_failure_keeps_the_old_value_and_oversized_text_never_reaches_transport() {
    let (fake, mut calls) = fake();
    fake.permits.add_permits(1);
    let (_sources, source) = watch::channel(Some(fake.clone() as Arc<dyn Source>));
    let (_prefs, preferences) = watch::channel(crate::preferences::Preferences::default());
    let (commands, requests) = mpsc::channel(4);
    let (updates, mut states) = watch::channel(Snapshot::default());
    let task = tokio::spawn(run(requests, source, preferences, updates));
    assert_eq!(calls.recv().await, Some(Call::Presence(false)));
    until(&mut states, |s| !s.busy && s.visibility.is_some()).await;
    commands
        .send(Command {
            request: 1,
            action: Action::LoadAbout,
        })
        .await
        .unwrap();
    assert_eq!(calls.recv().await, Some(Call::Load));
    until(&mut states, |s| {
        s.reply.as_ref().is_some_and(|r| r.request == 1)
    })
    .await;
    fake.fail.store(true, Ordering::SeqCst);
    commands
        .send(Command {
            request: 2,
            action: Action::SaveAbout("Keep this draft".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        calls.recv().await,
        Some(Call::Save("Keep this draft".into()))
    );
    until(&mut states, |s| {
        s.reply.as_ref().is_some_and(|r| r.request == 2)
    })
    .await;
    assert_eq!(states.borrow().about.as_deref(), Some("Synthetic About 👋"));
    assert!(!states.borrow().reply.as_ref().unwrap().error.is_empty());
    commands
        .send(Command {
            request: 3,
            action: Action::SaveAbout("🙂".repeat(140)),
        })
        .await
        .unwrap();
    until(&mut states, |s| {
        s.reply.as_ref().is_some_and(|r| r.request == 3)
    })
    .await;
    assert!(calls.try_recv().is_err());
    fake.fail.store(false, Ordering::SeqCst);
    commands
        .send(Command {
            request: 4,
            action: Action::SaveAbout("🙂".repeat(139)),
        })
        .await
        .unwrap();
    assert_eq!(calls.recv().await, Some(Call::Save("🙂".repeat(139))));
    until(&mut states, |s| {
        s.reply.as_ref().is_some_and(|r| r.request == 4)
    })
    .await;
    assert!(states.borrow().reply.as_ref().unwrap().error.is_empty());
    drop(commands);
    task.await.unwrap();
}
