use super::*;
use crate::archive::{
    Batch,
    tests::{fixture, record},
};
use tokio::sync::{mpsc, oneshot};

struct Fake(mpsc::UnboundedSender<(ReceiptJob, oneshot::Sender<bool>)>);
#[async_trait::async_trait]
impl Transport for Fake {
    async fn send(&self, job: &ReceiptJob) -> bool {
        let (tx, rx) = oneshot::channel();
        self.0.send((job.clone(), tx)).unwrap();
        rx.await.unwrap_or(false)
    }
}
async fn queued(db: &Archive) {
    db.ingest(Batch {
        messages: vec![record("receipt")],
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
    db.mark_visible_read_with_receipts(1, view.messages[0].id, true)
        .await
        .unwrap();
}
async fn call(
    rx: &mut mpsc::UnboundedReceiver<(ReceiptJob, oneshot::Sender<bool>)>,
) -> (ReceiptJob, oneshot::Sender<bool>) {
    timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

#[test]
fn unknown_privacy_never_defaults_to_public_receipts() {
    use whatsapp_rust::privacy_settings::{PrivacySetting, PrivacySettingsResponse};
    assert_eq!(receipts_disabled(&PrivacySettingsResponse::default()), None);
    for (value, expected) in [
        (PrivacyValue::All, Some(false)),
        (PrivacyValue::None, Some(true)),
        (PrivacyValue::Contacts, None),
    ] {
        assert_eq!(
            receipts_disabled(&PrivacySettingsResponse {
                settings: vec![PrivacySetting {
                    category: PrivacyCategory::ReadReceipts,
                    value
                }]
            }),
            expected
        );
    }
}

#[test]
fn account_privacy_blocks_both_dm_addressing_modes_and_preserves_group_rules() {
    for (jid, allowed_when_disabled) in [
        ("15550000001@s.whatsapp.net", false),
        ("10000000001@lid", false),
        ("123456789-123@g.us", true),
    ] {
        let jid = jid.parse().unwrap();
        assert!(may_notify(&jid, false));
        assert_eq!(may_notify(&jid, true), allowed_when_disabled);
    }
}

#[tokio::test]
async fn offline_failures_retry_durably_and_shutdown_cancels_an_inflight_receipt() {
    let (_temp, db) = fixture();
    queued(&db).await;
    let (online, rx) = watch::channel(false);
    let (stop, stopped) = watch::channel(false);
    let (_prefs, preferences) = watch::channel(Preferences::default());
    let (calls, mut results) = mpsc::unbounded_channel();
    let task = tokio::spawn(worker(
        db.clone(),
        Fake(calls),
        rx,
        stopped,
        preferences,
        Duration::from_secs(2),
        Duration::from_millis(50),
    ));
    assert!(
        timeout(Duration::from_millis(50), results.recv())
            .await
            .is_err()
    );
    online.send_replace(true);
    let (job, answer) = call(&mut results).await;
    assert!(job.kind == ReceiptKind::Read);
    answer.send(false).unwrap();
    let (_, answer) = call(&mut results).await;
    stop.send_replace(true);
    timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(answer.is_closed());
    assert!(db.next_receipt().await.unwrap().is_some());
}

#[tokio::test]
async fn disabling_receipts_cancels_and_discards_pending_jobs_even_offline() {
    let (_temp, db) = fixture();
    queued(&db).await;
    let (online, rx) = watch::channel(true);
    let (stop, stopped) = watch::channel(false);
    let (prefs, preferences) = watch::channel(Preferences::default());
    let (calls, mut results) = mpsc::unbounded_channel();
    let task = tokio::spawn(worker(
        db.clone(),
        Fake(calls),
        rx,
        stopped,
        preferences,
        Duration::from_secs(2),
        Duration::from_millis(20),
    ));
    let (_, answer) = call(&mut results).await;
    prefs.send_modify(|p| p.send_receipts = false);
    online.send_replace(false);
    timeout(Duration::from_secs(3), async {
        while db.next_receipt().await.unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(answer.is_closed());
    prefs.send_modify(|p| p.send_receipts = true);
    online.send_replace(true);
    assert!(
        timeout(Duration::from_millis(75), results.recv())
            .await
            .is_err()
    );
    stop.send_replace(true);
    task.await.unwrap().unwrap();
}
