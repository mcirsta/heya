//! One bounded sender-facing receipt queue; account privacy is checked at send time.
use crate::{
    archive::{self, Archive, ReceiptJob, ReceiptKind},
    preferences::Preferences,
};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::watch,
    time::{Instant, sleep_until, timeout},
};
use whatsapp_rust::{
    Client,
    privacy_settings::{PrivacyCategory, PrivacyValue},
    wacore_binary::JidExt,
};

#[async_trait::async_trait]
trait Transport: Send + Sync {
    async fn send(&self, job: &ReceiptJob) -> bool;
}
struct WhatsApp(Arc<Client>);

fn receipts_disabled(
    settings: &whatsapp_rust::privacy_settings::PrivacySettingsResponse,
) -> Option<bool> {
    match settings.get_value(&PrivacyCategory::ReadReceipts) {
        Some(PrivacyValue::All) => Some(false),
        Some(PrivacyValue::None) => Some(true),
        _ => None, // Never assume the account permits receipts after a failed/incomplete fetch.
    }
}

fn may_notify(chat: &whatsapp_rust::wacore_binary::Jid, disabled: bool) -> bool {
    chat.is_group() || (!disabled && (chat.is_pn() || chat.is_lid()))
}

#[async_trait::async_trait]
impl Transport for WhatsApp {
    async fn send(&self, job: &ReceiptJob) -> bool {
        let Ok(chat) = job.jid.parse::<whatsapp_rust::wacore_binary::Jid>() else {
            return true;
        };
        if !(chat.is_pn() || chat.is_lid() || chat.is_group()) {
            return true;
        }
        let sender = if chat.is_group() {
            let Ok(sender) = job.sender.parse::<whatsapp_rust::wacore_binary::Jid>() else {
                return true;
            };
            Some(sender)
        } else {
            None
        };
        // Validate every original key and the group participant before any network operation.
        for (_, wire) in &job.messages {
            let Ok(id) = whatsapp_rust::MessageId::new(wire.as_str()) else {
                return true;
            };
            if whatsapp_rust::MessageRef::new(&chat, id, sender.as_ref(), false).is_err() {
                return true;
            }
        }
        if self
            .0
            .wait_for_session_ready(Duration::from_secs(30))
            .await
            .is_err()
        {
            return false;
        }
        let Ok(settings) = self.0.fetch_privacy_settings().await else {
            return false;
        };
        let Some(disabled) = receipts_disabled(&settings) else {
            return false;
        };
        // Gate disabled DMs here too: SDK initialization refreshes its mutable
        // cache concurrently, so that cache must not be our only privacy guard.
        // Private unread state still synchronizes through the separate worker.
        if !may_notify(&chat, disabled) {
            return true;
        }
        // Refresh the SDK's existing gate through its public API as well.
        self.0
            .persistence_manager()
            .process_command(
                whatsapp_rust::wacore::store::DeviceCommand::SetReadReceiptsDisabled(disabled),
            )
            .await;
        let ids = job
            .messages
            .iter()
            .map(|(_, wire)| wire.as_str())
            .collect::<Vec<_>>();
        match job.kind {
            ReceiptKind::Read => self.0.mark_as_read(&chat, sender.as_ref(), &ids).await,
            ReceiptKind::Played => self.0.mark_as_played(&chat, sender.as_ref(), &ids).await,
        }
        .is_ok()
    }
}

pub async fn run(
    archive: Archive,
    client: Arc<Client>,
    online: watch::Receiver<bool>,
    stop: watch::Receiver<bool>,
    preferences: watch::Receiver<Preferences>,
) -> archive::Result<()> {
    worker(
        archive,
        WhatsApp(client),
        online,
        stop,
        preferences,
        Duration::from_secs(45),
        Duration::from_secs(30),
    )
    .await
}

async fn worker(
    archive: Archive,
    transport: impl Transport,
    mut online: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
    mut preferences: watch::Receiver<Preferences>,
    send_timeout: Duration,
    backoff: Duration,
) -> archive::Result<()> {
    let mut changes = archive.subscribe();
    let mut retry_at = Instant::now();
    let mut preferences_open = true;
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        let enabled = preferences.borrow_and_update().send_receipts;
        if !enabled {
            archive.discard_receipts().await?;
        }
        let connected = *online.borrow_and_update();
        let ready = Instant::now() >= retry_at;
        if connected && enabled && ready {
            changes.borrow_and_update();
            if let Some(job) = archive.next_receipt().await? {
                let success = tokio::select! {
                    biased;
                    _=stop.changed()=>return Ok(()),
                    _=preferences.changed(), if preferences_open=>{preferences_open=preferences.has_changed().is_ok();false},
                    _=online.changed()=>false,
                    result=timeout(send_timeout,transport.send(&job))=>result.unwrap_or(false),
                };
                if success {
                    archive.finish_receipt(job).await?;
                } else {
                    retry_at = Instant::now() + backoff;
                }
                continue;
            }
        }
        tokio::select! {
            biased;
            _=stop.changed()=>return Ok(()),
            event=preferences.changed(), if preferences_open=>{preferences_open=event.is_ok();},
            event=online.changed()=>{if event.is_err(){return Ok(());}},
            event=changes.changed()=>{if event.is_err(){return Ok(());}},
            _=sleep_until(retry_at), if connected && enabled && !ready=>{},
        }
    }
}

#[cfg(test)]
#[path = "protocol_receipt_tests.rs"]
mod tests;
