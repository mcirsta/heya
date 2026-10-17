use super::*;
use crate::archive::tests::{PN, fixture};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
struct Choose(Vec<PathBuf>);
#[async_trait::async_trait]
impl Picker for Choose {
    async fn open(&self, _: Kind, _: &str, _: &Control) -> Result<Vec<PathBuf>, Error> {
        Ok(self.0.clone())
    }
}
struct Upload {
    calls: Mutex<Vec<(String, String, i32)>>,
    fail: AtomicUsize,
    hold: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl Uploader for Upload {
    async fn upload(
        &self,
        file: Arc<Staged>,
        caption: String,
        destination: &Destination,
        control: Control,
        p: watch::Sender<u64>,
    ) -> Result<Prepared, Error> {
        let count = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((file.id.clone(), caption.clone(), destination.chat));
            calls.len()
        };
        p.send_replace(file.encrypted_len / 2);
        let hold = self.hold.load(Ordering::SeqCst);
        let fail = self.fail.load(Ordering::SeqCst) == count;
        let prepared = crate::protocol_uploads::tests::prepared(&file, caption, destination);
        crate::attachment_worker::upload(control, async move {
            if hold {
                std::future::pending::<()>().await;
            }
            if fail {
                return Err(Error::Upload);
            }
            tokio::task::spawn_blocking(|| ())
                .await
                .map_err(|_| Error::Upload)?;
            Ok(prepared)
        })
        .await
    }
}
async fn phase(status: &mut watch::Receiver<Snapshot>, phase: i32) -> Snapshot {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let s = status.borrow().clone();
            if s.phase == phase {
                return s;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}
fn pick(chat: i32) -> Command {
    Command::Pick {
        chat,
        title: "Synthetic recipient".into(),
        parent: String::new(),
        kind: Kind::Document,
    }
}
fn uploader() -> Arc<Upload> {
    Arc::new(Upload {
        calls: Mutex::new(Vec::new()),
        fail: AtomicUsize::new(0),
        hold: false.into(),
    })
}
#[tokio::test]
async fn queue_retries_are_idempotent_and_readonly_destinations_reject_media() {
    let (_dir, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Synthetic.txt");
    std::fs::write(&path, b"Synthetic original").unwrap();
    let files = crate::protocol_uploads::stage(vec![path], Kind::Document, Control::default())
        .await
        .unwrap();
    let file = &files[0];
    let destination = db.attachment_destination(chat).await.unwrap();
    db.queue_attachment(
        chat,
        file.id.clone(),
        crate::protocol_uploads::tests::prepared(file, "Caption".into(), &destination),
        None,
    )
    .await
    .unwrap();
    let sent = db.claim_next().await.unwrap().unwrap();
    db.queue_attachment(
        chat,
        file.id.clone(),
        crate::protocol_uploads::tests::prepared(file, "Caption".into(), &destination),
        None,
    )
    .await
    .unwrap();
    assert!(
        db.claim_next().await.unwrap().is_none(),
        "requeueing the same staging ID cannot send again"
    );
    let count = db
        .blocking(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))?))
        .unwrap();
    assert_eq!(count, 1);
    db.delivery(sent.jid, sent.id, "uncertain").await.unwrap();
    db.call(move |c| {
        c.execute("UPDATE chats SET read_only=1 WHERE id=?1", [chat])?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(db.attachment_destination(chat).await.is_err());
    assert!(
        db.queue_attachment(
            chat,
            "new-synthetic-id".into(),
            crate::protocol_uploads::tests::prepared(file, String::new(), &destination),
            None
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn batch_review_captions_retry_and_durable_queue_keep_recipient_and_drafts() {
    let (_dir, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    db.draft(chat, "Keep this composer draft".into())
        .await
        .unwrap();
    let other = db
        .create_chat("15550000002@s.whatsapp.net".into())
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let paths = (0..2)
        .map(|i| {
            let p = temp.path().join(format!("synthetic-{i}.txt"));
            std::fs::write(&p, format!("Original file {i}")).unwrap();
            p
        })
        .collect::<Vec<_>>();
    let source = uploader();
    source.fail.store(2, Ordering::SeqCst);
    let (_archives, arx) = watch::channel(Some(db.clone()));
    let (_sources, srx) = watch::channel(Some(source.clone() as Arc<dyn Uploader>));
    let (tx, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Snapshot::default());
    let task = tokio::spawn(run(
        arx,
        srx,
        rx,
        updates,
        Arc::new(Choose(paths.clone())),
        false,
    ));
    tx.send(pick(chat)).await.unwrap();
    phase(&mut status, 3).await;
    assert!(
        source.calls.lock().unwrap().is_empty(),
        "choosing files cannot send anything"
    );
    tx.send(Command::Select(1, "First caption".into()))
        .await
        .unwrap();
    while status.borrow().selected != 1 {
        status.changed().await.unwrap();
    }
    tx.send(Command::Select(0, "Second caption 👩🏽‍💻".into()))
        .await
        .unwrap();
    while status.borrow().selected != 0 {
        status.changed().await.unwrap();
    }
    for p in paths {
        std::fs::remove_file(p).unwrap();
    }
    tx.send(Command::Send("First caption".into()))
        .await
        .unwrap();
    let failed = phase(&mut status, 6).await;
    assert_eq!(failed.items.len(), 1);
    assert_eq!(failed.caption, "Second caption 👩🏽‍💻");
    tx.send(Command::Send(failed.caption)).await.unwrap();
    phase(&mut status, 0).await;
    {
        let calls = source.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_ne!(calls[0].0, calls[1].0);
        assert_eq!(calls[1].0, calls[2].0);
        assert_eq!(calls[0].1, "First caption");
        assert_eq!(calls[2].1, "Second caption 👩🏽‍💻");
        assert!(calls.iter().all(|c| c.2 == chat));
    }
    let count = db
        .blocking(move |c| {
            Ok(c.query_row(
                "SELECT count(*) FROM messages WHERE outgoing=1 AND chat_id=?1",
                [chat],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 2);
    let other_count = db
        .blocking(move |c| {
            Ok(c.query_row(
                "SELECT count(*) FROM messages WHERE chat_id=?1",
                [other],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(other_count, 0);
    assert_eq!(
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .draft,
        "Keep this composer draft"
    );
    let job = db.claim_next().await.unwrap().unwrap();
    assert!(!job.raw.is_empty());
    db.delivery(job.jid, job.id, "uncertain").await.unwrap();
    assert!(db.claim_next().await.unwrap().is_some());
    assert!(db.claim_next().await.unwrap().is_none());
    drop(tx);
    task.await.unwrap();
}
#[tokio::test]
async fn offline_cancel_shutdown_and_failed_archive_never_discard_unsent_selection() {
    let (_dir, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("synthetic.txt");
    std::fs::write(&path, b"Synthetic bytes").unwrap();
    let (_archives, arx) = watch::channel(Some(db.clone()));
    let (sources, srx) = watch::channel(None);
    let (tx, rx) = mpsc::channel(16);
    let (updates, mut status) = watch::channel(Snapshot::default());
    let task = tokio::spawn(run(
        arx,
        srx,
        rx,
        updates,
        Arc::new(Choose(vec![path])),
        false,
    ));
    tx.send(pick(chat)).await.unwrap();
    phase(&mut status, 3).await;
    tx.send(Command::Send("Preserved".into())).await.unwrap();
    assert!(phase(&mut status, 6).await.error.contains("Connect"));
    let source = uploader();
    source.hold.store(true, Ordering::SeqCst);
    sources.send_replace(Some(source.clone() as Arc<dyn Uploader>));
    tx.send(Command::Send("Preserved".into())).await.unwrap();
    phase(&mut status, 4).await;
    tx.send(Command::Cancel).await.unwrap();
    assert!(phase(&mut status, 6).await.error.contains("cancelled"));
    assert!(db.claim_next().await.unwrap().is_none());
    source.hold.store(false, Ordering::SeqCst);
    db.call(|c|{c.execute_batch("CREATE TRIGGER reject_attachment BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;")?;Ok(())}).await.unwrap();
    tx.send(Command::Send("Preserved".into())).await.unwrap();
    let failed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let s = status.borrow().clone();
            if s.phase == 6 && s.error.contains("archive") {
                break s;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(failed.items.len(), 1);
    assert_eq!(failed.caption, "Preserved");
    db.call(|c| {
        c.execute_batch("DROP TRIGGER reject_attachment")?;
        Ok(())
    })
    .await
    .unwrap();
    tx.send(Command::Send(failed.caption)).await.unwrap();
    phase(&mut status, 0).await;
    assert!(db.claim_next().await.unwrap().is_some());
    // Closing the application during another upload cancels it without a new row.
    source.hold.store(true, Ordering::SeqCst);
    tx.send(pick(chat)).await.unwrap();
    phase(&mut status, 3).await;
    tx.send(Command::Send("Never queued on close".into()))
        .await
        .unwrap();
    phase(&mut status, 4).await;
    drop(tx);
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}
