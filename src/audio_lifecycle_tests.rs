use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test(flavor = "multi_thread")]
async fn idle_download_cancel_and_load_failure_never_start_a_player() {
    let (_temp, db) = fixture();
    let id = inserted(&db).await;
    let (_archives, arx) = watch::channel(Some(db.clone()));
    let source = Arc::new(Delayed(tokio::sync::Notify::new()));
    let (sources, srx) = watch::channel(Some(source.clone() as Arc<dyn Source>));
    let (commands, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Status::default());
    let starts = Arc::new(AtomicUsize::new(0));
    let calls = starts.clone();
    let service = tokio::spawn(run_with_player(
        arx,
        srx,
        rx,
        updates,
        false,
        move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(Error::Playback) // Simulate failure to create an OS thread.
        },
    ));
    commands.send(Command::Speed).await.unwrap();
    until(&mut status, |s| s.speed == 1.5).await;
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    commands.send(Command::Toggle(id)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), source.0.notified())
        .await
        .unwrap();
    assert_eq!(
        starts.load(Ordering::SeqCst),
        0,
        "a download does not need a playback thread"
    );
    commands.send(Command::Stop).await.unwrap();
    until(&mut status, |s| s.id == 0).await;
    sources.send_replace(None);
    commands.send(Command::Toggle(id)).await.unwrap();
    until(&mut status, |s| s.error == Some(Error::Offline)).await;
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    db.save_audio(
        &db.audio_request(id).await.unwrap().unwrap(),
        crate::fixtures::audio_bytes(),
    )
    .await
    .unwrap();
    // A failed thread start is reported and can be retried, without a hidden
    // background retry loop or a coordinator that remains stuck in Loading.
    for attempt in 1..=2 {
        commands.send(Command::Toggle(id)).await.unwrap();
        until(&mut status, |s| s.error == Some(Error::Playback)).await;
        assert_eq!(starts.load(Ordering::SeqCst), attempt);
        status.borrow_and_update();
        commands.send(Command::Stop).await.unwrap();
        until(&mut status, |s| s.id == 0).await;
    }
    drop(commands);
    tokio::time::timeout(Duration::from_secs(2), service)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn real_playback_ends_releases_bytes_and_can_start_again() {
    let (_archives, arx) = watch::channel(None);
    let (_sources, srx) = watch::channel(None);
    let (commands, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Status::default());
    let (started, mut starts) = mpsc::unbounded_channel();
    let service = tokio::spawn(run_with_player(
        arx,
        srx,
        rx,
        updates,
        true,
        move |desired, updates| {
            let weak = Arc::downgrade(desired.bytes.as_ref().unwrap());
            let worker = Worker::start(desired, updates, Output::Silent)?;
            started.send(weak).unwrap();
            Ok(worker)
        },
    ));
    for complete in [true, false] {
        commands.send(Command::Toggle(1)).await.unwrap();
        let bytes = tokio::time::timeout(Duration::from_secs(2), starts.recv())
            .await
            .unwrap()
            .unwrap();
        until(&mut status, |s| s.phase == 2).await;
        if complete {
            commands.send(Command::Seek(1, 0.98)).await.unwrap();
            until(&mut status, |s| s.phase == 0 && s.id == 1).await;
        } else {
            commands.send(Command::Stop).await.unwrap();
            until(&mut status, |s| s.id == 0).await;
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while bytes.upgrade().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("idle playback must release compressed bytes and its control channel");
        commands.send(Command::Speed).await.unwrap();
        until(&mut status, |s| s.speed == if complete { 1.5 } else { 2. }).await;
        assert!(
            starts.try_recv().is_err(),
            "completion and idle commands must not respawn a player"
        );
    }
    drop(commands);
    tokio::time::timeout(Duration::from_secs(2), service)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn replacement_waits_for_teardown_and_ignores_old_failure() {
    let (_archives, arx) = watch::channel(None);
    let (_sources, srx) = watch::channel(None);
    let (commands, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Status::default());
    let (started, mut starts) = mpsc::unbounded_channel();
    let (stopping, mut stops) = mpsc::unbounded_channel();
    let active = Arc::new(AtomicUsize::new(0));
    let live = active.clone();
    let service = tokio::spawn(run_with_player(
        arx,
        srx,
        rx,
        updates,
        true,
        move |initial, updates| {
            let started = started.clone();
            let stopping = stopping.clone();
            let live = live.clone();
            Worker::spawn(initial, updates, move |mut desired, updates| {
                struct Active(Arc<AtomicUsize>);
                impl Drop for Active {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                assert_eq!(
                    live.fetch_add(1, Ordering::SeqCst),
                    0,
                    "players must never overlap"
                );
                let _active = Active(live);
                assert_eq!(std::thread::current().name(), Some("heya-audio"));
                let (generation, id) = {
                    let d = desired.borrow();
                    (d.generation, d.id)
                };
                let (release, gate) = std::sync::mpsc::channel();
                started.send((id, release)).unwrap();
                let runtime = tokio::runtime::Handle::current();
                loop {
                    let next = desired.borrow_and_update().clone();
                    if next.generation != generation || next.bytes.is_none() {
                        break;
                    }
                    updates.send_replace(Status {
                        generation,
                        id,
                        phase: if next.playing { 2 } else { 3 },
                        speed: next.speed,
                        ..Default::default()
                    });
                    if runtime.block_on(desired.changed()).is_err() {
                        break;
                    }
                }
                stopping.send(id).unwrap();
                gate.recv_timeout(Duration::from_secs(5))
                    .expect("test must release player teardown");
                let last = Status {
                    generation,
                    id,
                    phase: 4,
                    error: Some(Error::Playback),
                    ..Default::default()
                };
                updates.send_replace(last.clone());
                last
            })
        },
    ));
    commands.send(Command::Toggle(1)).await.unwrap();
    let (id, release) = tokio::time::timeout(Duration::from_secs(2), starts.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(id, 1);
    until(&mut status, |s| s.id == 1 && s.phase == 2).await;
    commands.send(Command::Toggle(1)).await.unwrap();
    until(&mut status, |s| s.phase == 3).await;
    assert!(starts.try_recv().is_err(), "pause reuses the active player");
    commands.send(Command::Toggle(2)).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), stops.recv())
            .await
            .unwrap(),
        Some(1)
    );
    commands.send(Command::Toggle(3)).await.unwrap();
    until(&mut status, |s| s.id == 3 && s.phase == 1).await;
    assert!(starts.try_recv().is_err());
    assert_eq!(active.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    let (id, release) = tokio::time::timeout(Duration::from_secs(2), starts.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(id, 3, "obsolete intermediate selection must never start");
    until(&mut status, |s| s.id == 3 && s.phase == 2).await;
    assert!(
        status.borrow().error.is_none(),
        "old player failure must not overwrite the replacement"
    );
    drop(commands);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), stops.recv())
            .await
            .unwrap(),
        Some(3)
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), service)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
}
