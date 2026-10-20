//! Serial, retryable private read-state updates; no sender-facing receipts.
use crate::archive::{self, Archive, ReadSync};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::watch,
    time::{Instant, sleep_until, timeout},
};
use whatsapp_rust::{Client, wacore_binary::JidExt};

#[async_trait::async_trait]
trait Transport: Send + Sync {
    async fn send(&self, read: &ReadSync) -> bool;
}
struct WhatsApp(Arc<Client>);
#[async_trait::async_trait]
impl Transport for WhatsApp {
    async fn send(&self, read: &ReadSync) -> bool {
        let Ok(jid) = read.jid.parse::<whatsapp_rust::wacore_binary::Jid>() else {
            return false;
        };
        if !(jid.is_pn() || jid.is_lid() || jid.is_group()) {
            return false;
        }
        if self
            .0
            .wait_for_session_ready(Duration::from_secs(30))
            .await
            .is_err()
        {
            return false;
        }
        let Some(range) = read_range(read) else {
            return false;
        };
        self.0
            .chat_actions()
            .mark_chat_as_read(&jid, true, Some(range))
            .await
            .is_ok()
    }
}

pub(crate) fn read_range(read: &ReadSync) -> Option<whatsapp_rust::SyncActionMessageRange> {
    let jid = read.jid.parse::<whatsapp_rust::wacore_binary::Jid>().ok()?;
    let reference = if let Some(message) = &read.message {
        let sender = if !message.outgoing && jid.is_group() {
            Some(
                message
                    .sender
                    .parse::<whatsapp_rust::wacore_binary::Jid>()
                    .ok()?,
            )
        } else {
            None
        };
        let reference = whatsapp_rust::MessageRef::new(
            &jid,
            whatsapp_rust::MessageId::new(&message.wire).ok()?,
            sender.as_ref(),
            message.outgoing,
        )
        .ok()?;
        return Some(whatsapp_rust::message_range(
            read.through,
            None,
            [(reference, read.through)],
        ));
    } else {
        Vec::new()
    };
    // Old read positions can outlive deleted/expired bodies. Preserve their
    // bounded timestamp; never invent a message key or read the current tail.
    Some(whatsapp_rust::message_range(read.through, None, reference))
}

pub async fn run(
    archive: Archive,
    client: Arc<Client>,
    online: watch::Receiver<bool>,
    stop: watch::Receiver<bool>,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) -> archive::Result<()> {
    tokio::try_join!(
        worker(
            archive.clone(),
            WhatsApp(client.clone()),
            online.clone(),
            stop.clone(),
            Duration::from_secs(45),
            Duration::from_secs(30),
        ),
        crate::protocol_receipts::run(archive, client, online, stop, preferences)
    )?;
    Ok(())
}

async fn worker(
    archive: Archive,
    transport: impl Transport,
    mut online: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
    send_timeout: Duration,
    backoff: Duration,
) -> archive::Result<()> {
    let mut changes = archive.subscribe();
    let mut retry_at = Instant::now();
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        let retry_ready = Instant::now() >= retry_at;
        let connected = *online.borrow_and_update();
        if connected && retry_ready {
            changes.borrow_and_update();
            if let Some(job) = archive.next_read_sync().await? {
                // All app-state writes share a collection. Keep one in flight,
                // and back off after failure even if archive updates keep arriving.
                let success = tokio::select! {
                    biased;
                    _ = stop.changed() => { return Ok(()); }
                    _ = online.changed() => false,
                    result = timeout(send_timeout, transport.send(&job)) => result.unwrap_or(false),
                };
                if success {
                    archive.finish_read_sync(job).await?;
                } else {
                    retry_at = Instant::now() + backoff;
                }
                continue;
            }
        }
        tokio::select! {
            biased;
            _ = stop.changed() => { return Ok(()); }
            event = online.changed() => { if event.is_err() { return Ok(()); } }
            event = changes.changed() => { if event.is_err() { return Ok(()); } }
            _ = sleep_until(retry_at), if *online.borrow() && !retry_ready => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{
        Batch,
        tests::{fixture, record},
    };
    use tokio::sync::{mpsc, oneshot};

    struct Fake(mpsc::UnboundedSender<(ReadSync, oneshot::Sender<bool>)>);
    #[async_trait::async_trait]
    impl Transport for Fake {
        async fn send(&self, job: &ReadSync) -> bool {
            let (done, answer) = oneshot::channel();
            self.0.send((job.clone(), done)).unwrap();
            answer.await.unwrap_or(false)
        }
    }
    async fn mark(db: &Archive, id: &str, stamp: i64) {
        let mut r = record(id);
        r.timestamp = stamp;
        db.ingest(Batch {
            messages: vec![r],
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
        let view = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        db.mark_visible_read(chat, view.messages.last().unwrap().id)
            .await
            .unwrap();
    }
    async fn call(
        rx: &mut mpsc::UnboundedReceiver<(ReadSync, oneshot::Sender<bool>)>,
    ) -> (ReadSync, oneshot::Sender<bool>) {
        timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap()
    }
    #[tokio::test]
    async fn offline_reads_retry_serially_without_consuming_newer_work_or_spinning() {
        let (_temp, db) = fixture();
        mark(&db, "a", 100).await;
        let (online, rx) = watch::channel(false);
        let (stop, stopped) = watch::channel(false);
        let (calls, mut received) = mpsc::unbounded_channel();
        let task = tokio::spawn(worker(
            db.clone(),
            Fake(calls),
            rx,
            stopped,
            Duration::from_secs(3),
            Duration::from_millis(120),
        ));
        assert!(
            timeout(Duration::from_millis(30), received.recv())
                .await
                .is_err()
        );
        online.send_replace(true);
        let (a, failed) = call(&mut received).await;
        assert_eq!(a.through, 100);
        failed.send(false).unwrap();
        // Unrelated archive bursts must not bypass the collection backoff.
        for _ in 0..20 {
            db.changed();
            tokio::task::yield_now().await;
        }
        assert!(
            timeout(Duration::from_millis(40), received.recv())
                .await
                .is_err()
        );
        let (retry, done) = call(&mut received).await;
        assert!(retry == a);
        mark(&db, "b", 200).await;
        assert!(
            timeout(Duration::from_millis(30), received.recv())
                .await
                .is_err()
        );
        done.send(true).unwrap();
        let (newer, done) = call(&mut received).await;
        assert_eq!(newer.through, 200);
        done.send(true).unwrap();
        timeout(Duration::from_secs(3), async {
            while db.next_read_sync().await.unwrap().is_some() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        stop.send_replace(true);
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn timed_out_and_cancelled_reads_remain_durable_for_reconnect() {
        let (_temp, db) = fixture();
        mark(&db, "a", 100).await;
        let (_online, rx) = watch::channel(true);
        let (stop, stopped) = watch::channel(false);
        let (calls, mut received) = mpsc::unbounded_channel();
        let task = tokio::spawn(worker(
            db.clone(),
            Fake(calls),
            rx,
            stopped,
            Duration::from_millis(40),
            Duration::from_millis(50),
        ));
        let (a, mut hung) = call(&mut received).await;
        timeout(Duration::from_secs(3), hung.closed())
            .await
            .unwrap();
        assert!(db.next_read_sync().await.unwrap().unwrap() == a);
        let (_, mut second) = call(&mut received).await;
        stop.send_replace(true);
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        second.closed().await;
        assert!(db.next_read_sync().await.unwrap().unwrap() == a);
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use crate::archive::{
        Batch,
        tests::{PN, fixture, record},
    };
    #[tokio::test]
    async fn private_ranges_identify_the_read_message_and_its_author() {
        let (_temp, db) = fixture();
        db.ingest(Batch {
            messages: vec![record("synthetic-range")],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let view = db
            .view(1, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        db.mark_visible_read(view.chat.id, view.messages[0].id)
            .await
            .unwrap();
        let mut job = db.next_read_sync().await.unwrap().unwrap();
        for outgoing in [false, true] {
            for chat in [PN, "15550000001@lid", "123456789@g.us"] {
                job.jid = chat.into();
                let message = job.message.as_mut().unwrap();
                message.outgoing = outgoing;
                message.sender = PN.into();
                let range = read_range(&job).unwrap();
                assert_eq!(range.last_message_timestamp, Some(job.through));
                assert_eq!(range.messages.len(), 1);
                assert_eq!(range.messages[0].timestamp, Some(job.through));
                let key = range.messages[0].key.as_option().unwrap();
                assert_eq!(key.id.as_deref(), Some("synthetic-range"));
                assert_eq!(key.from_me, Some(outgoing));
                assert_eq!(key.remote_jid.as_deref(), Some(chat));
                assert_eq!(
                    key.participant.as_deref(),
                    if !outgoing && chat.ends_with("@g.us") {
                        Some(PN)
                    } else {
                        None
                    }
                );
            }
        }
        job.message = None;
        assert!(
            read_range(&job).unwrap().messages.is_empty(),
            "an expired body's range retains its original timestamp"
        );
    }
}
