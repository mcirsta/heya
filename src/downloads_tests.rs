//! Synthetic encrypted originals, explicit chooser replies and controllable I/O.
use super::*;
use crate::{
    archive::{
        Batch,
        tests::{fixture, record},
    },
    protocol_downloads::tests::attachment,
};
use std::{io::Write, sync::atomic::AtomicUsize, time::Duration};
use tokio::sync::Notify;

struct Choose {
    path: Option<PathBuf>,
    calls: AtomicUsize,
    hold: bool,
    opened: Notify,
}
#[async_trait::async_trait]
impl Picker for Choose {
    async fn save(&self, _: &str, _: &str, control: &Control) -> Result<Option<PathBuf>, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.opened.notify_one();
        if self.hold {
            control.cancelled().await;
            return Err(Error::Cancelled);
        }
        Ok(self.path.clone())
    }
}
struct Network {
    bytes: Vec<u8>,
    calls: AtomicUsize,
    hold: AtomicBool,
    failed: AtomicBool,
    started: Notify,
    release: Notify,
}
impl Network {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            calls: AtomicUsize::new(0),
            hold: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            started: Notify::new(),
            release: Notify::new(),
        }
    }
}
#[async_trait::async_trait]
impl Source for Network {
    async fn fetch(&self, _: &[u8], mut sink: Sink) -> Result<Sink, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        sink.write_all(&self.bytes[..1])
            .map_err(|_| Error::Storage)?;
        self.started.notify_one();
        if self.hold.load(Ordering::SeqCst) {
            self.release.notified().await;
        }
        sink.write_all(&self.bytes[1..])
            .map_err(|_| Error::Storage)?;
        if self.failed.load(Ordering::SeqCst) {
            Err(Error::Integrity)
        } else {
            Ok(sink)
        }
    }
}
async fn seed(kind: Kind, bytes: &[u8]) -> (tempfile::TempDir, Archive, Request) {
    let (dir, archive) = fixture();
    let (raw, content) = attachment(kind, bytes);
    let mut row = record("synthetic-download");
    row.raw = raw;
    row.content = content;
    archive
        .ingest(Batch {
            messages: vec![row],
            ..Default::default()
        })
        .await
        .unwrap();
    let chat = archive
        .view(0, 100, String::new(), false, false)
        .await
        .unwrap()
        .chats[0]
        .id;
    let id = archive
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
        .messages[0]
        .id;
    let request = archive.download_request(chat, id).await.unwrap().unwrap();
    (dir, archive, request)
}
async fn phase(rx: &mut watch::Receiver<Status>, wanted: i32) -> Status {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let status = rx.borrow().clone();
            if status.phase as i32 == wanted {
                return status;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}
fn clean(path: &std::path::Path) {
    assert!(!std::fs::read_dir(path).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".heya-download-")
    }));
}
fn start(request: &Request) -> Command {
    Command::Start(Target {
        chat: request.chat,
        message: request.id,
        parent: String::new(),
        demo: None,
    })
}

#[tokio::test]
async fn saved_status_expires_without_hiding_a_new_download_or_failure() {
    let bytes = b"synthetic attachment";
    let (_dir, archive, request) = seed(Kind::Document, bytes).await;
    let files = tempfile::tempdir().unwrap();
    let target = files.path().join("saved.txt");
    let picker = Arc::new(Choose {
        path: Some(target.clone()),
        calls: AtomicUsize::new(0),
        hold: false,
        opened: Notify::new(),
    });
    let source = Arc::new(Network::new(bytes));
    let (_archives, arx) = watch::channel(Some(archive));
    let (_sources, srx) = watch::channel(Some(source.clone() as Arc<dyn Source>));
    let (tx, rx) = mpsc::channel(8);
    let (updates, mut status) = watch::channel(Status::default());
    let task = tokio::spawn(run(arx, srx, rx, updates, picker));

    tx.send(start(&request)).await.unwrap();
    phase(&mut status, 4).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(status.borrow().phase, Phase::Saved);

    source.hold.store(true, Ordering::SeqCst);
    tx.send(start(&request)).await.unwrap();
    phase(&mut status, 2).await;
    tokio::time::sleep(SAVED_FOR + Duration::from_millis(100)).await;
    assert_eq!(status.borrow().phase, Phase::Downloading);
    tx.send(Command::Cancel).await.unwrap();
    phase(&mut status, 6).await;

    source.hold.store(false, Ordering::SeqCst);
    source.failed.store(true, Ordering::SeqCst);
    tx.send(Command::Retry).await.unwrap();
    phase(&mut status, 5).await;
    tokio::time::sleep(SAVED_FOR + Duration::from_millis(100)).await;
    assert_eq!(status.borrow().phase, Phase::Failed);
    assert_eq!(std::fs::read(&target).unwrap(), bytes);

    source.failed.store(false, Ordering::SeqCst);
    tx.send(Command::Retry).await.unwrap();
    phase(&mut status, 4).await;
    tokio::time::timeout(SAVED_FOR + Duration::from_secs(2), async {
        status.wait_for(|s| s.phase == Phase::Hidden).await.unwrap();
    })
    .await
    .unwrap();
    assert!(status.borrow().name.is_empty());
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    clean(files.path());
    drop(tx);
    task.await.unwrap();
}

#[tokio::test]
async fn explicit_save_coalesces_requests_retries_authentication_failure_and_never_redirects() {
    let bytes = b"synthetic authenticated original";
    let (_dir, archive, request) = seed(Kind::Document, bytes).await;
    let files = tempfile::tempdir().unwrap();
    let target = files.path().join("chosen.txt");
    std::fs::write(&target, b"old content").unwrap();
    let picker = Arc::new(Choose {
        path: Some(target.clone()),
        calls: AtomicUsize::new(0),
        hold: false,
        opened: Notify::new(),
    });
    let source = Arc::new(Network::new(bytes));
    source.failed.store(true, Ordering::SeqCst);
    let (_archives, arx) = watch::channel(Some(archive));
    let (_sources, srx) = watch::channel(Some(source.clone() as Arc<dyn Source>));
    let (tx, rx) = mpsc::channel(8);
    let (updates, mut status) = watch::channel(Status::default());
    let task = tokio::spawn(run(arx, srx, rx, updates, picker.clone()));
    assert_eq!(
        picker.calls.load(Ordering::SeqCst),
        0,
        "idle startup cannot open a chooser"
    );
    tx.send(start(&request)).await.unwrap();
    assert_eq!(phase(&mut status, 5).await.error, Some(Error::Integrity));
    assert_eq!(std::fs::read(&target).unwrap(), b"old content");
    clean(files.path());
    source.failed.store(false, Ordering::SeqCst);
    source.hold.store(true, Ordering::SeqCst);
    tx.send(Command::Retry).await.unwrap();
    while source.calls.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    tx.send(start(&request)).await.unwrap();
    tx.send(Command::Start(Target {
        chat: 999,
        message: 999,
        parent: String::new(),
        demo: None,
    }))
    .await
    .unwrap();
    // A command after the duplicate requests is a deterministic service barrier.
    tx.send(Command::Cancel).await.unwrap();
    phase(&mut status, 6).await;
    assert_eq!(picker.calls.load(Ordering::SeqCst), 2);
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read(&target).unwrap(), b"old content");
    clean(files.path());
    source.hold.store(false, Ordering::SeqCst);
    tx.send(Command::Retry).await.unwrap();
    assert_eq!(phase(&mut status, 4).await.name, "chosen.txt");
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    clean(files.path());
    tx.send(Command::Dismiss).await.unwrap();
    phase(&mut status, 0).await;
    drop(tx);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn cached_original_saves_offline_and_cancellation_does_not_create_any_output() {
    let bytes = b"synthetic cached original";
    let (_dir, archive, request) = seed(Kind::Photo, bytes).await;
    archive
        .save_photo(
            &crate::photos::Request {
                id: request.id,
                raw: request.raw.clone(),
            },
            crate::photos::Cached {
                original: bytes.to_vec(),
                preview: Vec::new(),
            },
        )
        .await
        .unwrap();
    let files = tempfile::tempdir().unwrap();
    let target = files.path().join("offline.png");
    let chooser = Arc::new(Choose {
        path: Some(target.clone()),
        calls: AtomicUsize::new(0),
        hold: false,
        opened: Notify::new(),
    });
    let (_archives, arx) = watch::channel(Some(archive.clone()));
    let (_sources, srx) = watch::channel(None);
    let (tx, rx) = mpsc::channel(8);
    let (updates, mut status) = watch::channel(Status::default());
    let task = tokio::spawn(run(arx, srx, rx, updates, chooser));
    tx.send(start(&request)).await.unwrap();
    phase(&mut status, 4).await;
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    drop(tx);
    task.await.unwrap();
    for (path, hold) in [(None, false), (Some(files.path().join("cancelled")), true)] {
        let chooser = Arc::new(Choose {
            path,
            calls: AtomicUsize::new(0),
            hold,
            opened: Notify::new(),
        });
        let (_archives, arx) = watch::channel(Some(archive.clone()));
        let (_sources, srx) = watch::channel(None);
        let (tx, rx) = mpsc::channel(8);
        let (updates, mut status) = watch::channel(Status::default());
        let task = tokio::spawn(run(arx, srx, rx, updates, chooser.clone()));
        tx.send(start(&request)).await.unwrap();
        chooser.opened.notified().await;
        if hold {
            tx.send(Command::Cancel).await.unwrap();
        }
        phase(&mut status, 6).await;
        drop(tx);
        task.await.unwrap();
        clean(files.path());
    }
    assert!(!files.path().join("cancelled").exists());
}
#[tokio::test]
async fn expiry_during_transfer_and_disconnect_prevent_publication_and_release_work() {
    for disconnect in [false, true] {
        let bytes = b"synthetic streaming attachment";
        let (_dir, archive, request) = seed(Kind::Document, bytes).await;
        let files = tempfile::tempdir().unwrap();
        let target = files.path().join("must-not-exist");
        let source = Arc::new(Network::new(bytes));
        source.hold.store(true, Ordering::SeqCst);
        let chooser = Arc::new(Choose {
            path: Some(target.clone()),
            calls: AtomicUsize::new(0),
            hold: false,
            opened: Notify::new(),
        });
        let (_archives, arx) = watch::channel(Some(archive.clone()));
        let (sources, srx) = watch::channel(Some(source.clone() as Arc<dyn Source>));
        let (tx, rx) = mpsc::channel(8);
        let (updates, mut status) = watch::channel(Status::default());
        let task = tokio::spawn(run(arx, srx, rx, updates, chooser));
        tx.send(start(&request)).await.unwrap();
        source.started.notified().await;
        if disconnect {
            sources.send_replace(None);
        } else {
            let id = request.id;
            archive
                .call(move |c| {
                    c.execute(
                        "UPDATE messages SET expires=unixepoch()-1 WHERE id=?1",
                        [id],
                    )?;
                    Ok(())
                })
                .await
                .unwrap();
            source.release.notify_one();
        }
        let result = phase(&mut status, 5).await;
        assert_eq!(
            result.error,
            Some(if disconnect {
                Error::Offline
            } else {
                Error::Unavailable
            })
        );
        assert!(!target.exists());
        clean(files.path());
        drop(tx);
        task.await.unwrap();
    }
}
#[tokio::test]
async fn offline_missing_original_fails_before_asking_for_a_destination_and_shutdown_cancels() {
    let bytes = b"synthetic attachment";
    let (_dir, archive, request) = seed(Kind::Document, bytes).await;
    let chooser = Arc::new(Choose {
        path: None,
        calls: AtomicUsize::new(0),
        hold: false,
        opened: Notify::new(),
    });
    let (_archives, arx) = watch::channel(Some(archive));
    let (_sources, srx) = watch::channel(None);
    let (tx, rx) = mpsc::channel(8);
    let (updates, mut status) = watch::channel(Status::default());
    let task = tokio::spawn(run(arx, srx, rx, updates, chooser.clone()));
    tx.send(start(&request)).await.unwrap();
    assert_eq!(phase(&mut status, 5).await.error, Some(Error::Offline));
    assert_eq!(chooser.calls.load(Ordering::SeqCst), 0);
    drop(tx);
    task.await.unwrap();
    let chooser = Arc::new(Choose {
        path: None,
        calls: AtomicUsize::new(0),
        hold: true,
        opened: Notify::new(),
    });
    let (_archives, arx) = watch::channel(None);
    let (_sources, srx) = watch::channel(None);
    let (tx, rx) = mpsc::channel(8);
    let (updates, _) = watch::channel(Status::default());
    let task = tokio::spawn(run(arx, srx, rx, updates, chooser.clone()));
    tx.send(Command::Start(Target {
        chat: 1,
        message: 1,
        parent: String::new(),
        demo: Some(Kind::Document),
    }))
    .await
    .unwrap();
    chooser.opened.notified().await;
    drop(tx);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn corrupt_cached_bytes_need_verification_and_can_recover_from_the_network() {
    let bytes = b"verified synthetic original";
    let (_dir, archive, request) = seed(Kind::Photo, bytes).await;
    assert!(
        archive
            .download_request(request.chat + 100, request.id)
            .await
            .unwrap()
            .is_none()
    );
    archive
        .save_photo(
            &crate::photos::Request {
                id: request.id,
                raw: request.raw.clone(),
            },
            crate::photos::Cached {
                original: b"corrupt cached bytes".to_vec(),
                preview: Vec::new(),
            },
        )
        .await
        .unwrap();
    let files = tempfile::tempdir().unwrap();
    let target = files.path().join("verified.png");
    let chooser = Arc::new(Choose {
        path: Some(target.clone()),
        calls: AtomicUsize::new(0),
        hold: false,
        opened: Notify::new(),
    });
    let (_archives, arx) = watch::channel(Some(archive));
    let (sources, srx) = watch::channel(None);
    let (tx, rx) = mpsc::channel(8);
    let (updates, mut status) = watch::channel(Status::default());
    let task = tokio::spawn(run(arx, srx, rx, updates, chooser.clone()));
    tx.send(start(&request)).await.unwrap();
    assert_eq!(phase(&mut status, 5).await.error, Some(Error::Integrity));
    assert_eq!(chooser.calls.load(Ordering::SeqCst), 0);
    assert!(!target.exists());
    let network = Arc::new(Network::new(bytes));
    sources.send_replace(Some(network.clone() as Arc<dyn Source>));
    tx.send(Command::Retry).await.unwrap();
    phase(&mut status, 4).await;
    assert_eq!(network.calls.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(target).unwrap(), bytes);
    clean(files.path());
    drop(tx);
    task.await.unwrap();
}
