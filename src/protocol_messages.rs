//! Protocol-specific ingestion and receipt mapping. SDK types stay in this adapter.
#[cfg(test)]
#[path = "protocol_business_archive_tests.rs"]
mod business_tests;
#[cfg(test)]
#[path = "message_content_tests.rs"]
mod content_tests;
#[cfg(test)]
#[path = "message_edit_tests.rs"]
mod edit_tests;
#[cfg(test)]
#[path = "protocol_notice_tests.rs"]
mod notice_tests;
#[cfg(test)]
#[path = "phone_history_tests.rs"]
mod phone_tests;
use crate::archive::{self, Archive, Batch, Change, ChatMeta, Record};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{mpsc, watch};
use whatsapp_rust::{
    Client, HistorySyncCaptureHook, InboundDurabilityHook,
    prelude::{Event, EventHandler, EventInterest, EventKind, InboundMessage, MessageExt, wa},
    wacore::{
        history_sync::{HistorySyncStream, MAX_DECOMPRESSED},
        types::presence::ReceiptType,
    },
    wacore_binary::{Jid, JidExt},
    waproto::{buffa::Message as _, codec},
};

pub fn address(jid: &Jid) -> String {
    jid.to_non_ad().to_string()
}
fn supported(jid: &Jid) -> bool {
    jid.is_group() || jid.is_pn() || jid.is_lid()
}

pub async fn verify_identity(client: &Client, archive: &Archive) -> archive::Result<()> {
    let snapshot = client.persistence_manager().get_device_snapshot();
    if let Some(pn) = &snapshot.pn {
        archive
            .bind_account(address(pn), snapshot.push_name.clone())
            .await?;
    } else {
        return Err(archive::Error::Identity);
    }
    Ok(())
}

#[derive(Clone)]
pub struct Ingest {
    pub archive: Archive,
    pub failure: watch::Sender<Option<archive::Error>>,
}
impl Ingest {
    fn report(&self, result: archive::Result<()>) -> anyhow::Result<()> {
        if let Err(error) = result {
            self.failure.send_replace(Some(error));
            return Err(error.into());
        }
        Ok(())
    }
}
#[async_trait::async_trait]
impl InboundDurabilityHook for Ingest {
    async fn on_messages(
        &self,
        client: Arc<Client>,
        messages: &[InboundMessage],
    ) -> anyhow::Result<()> {
        self.report(ingest_messages(&client, &self.archive, messages).await)
    }
}
#[async_trait::async_trait]
impl HistorySyncCaptureHook for Ingest {
    async fn on_history_sync(
        &self,
        client: Arc<Client>,
        _id: &str,
        _: Option<wa::message::HistorySyncType>,
        compressed: &[u8],
    ) -> anyhow::Result<()> {
        self.report(
            async {
                verify_identity(&client, &self.archive).await?;
                self.archive
                    .capture_history(
                        crate::protocol_history::chunk_id(compressed),
                        compressed.to_vec(),
                    )
                    .await
            }
            .await,
        )
    }
}

async fn known_alias(client: &Client, jid: &Jid, batch: &mut Batch) -> archive::Result<()> {
    if let Some(entry) = client
        .get_lid_pn_entry(jid)
        .await
        .map_err(|_| archive::Error::Database)?
    {
        batch.aliases.push((
            format!("{}@lid", entry.lid),
            format!("{}@s.whatsapp.net", entry.phone_number),
        ));
    }
    Ok(())
}
async fn ingest_messages(
    client: &Client,
    archive: &Archive,
    messages: &[InboundMessage],
) -> archive::Result<()> {
    verify_identity(client, archive).await?;
    let mut batch = live_batch(messages);
    let own = client.persistence_manager().get_device_snapshot();
    if let Some(pn) = &own.pn {
        batch.profiles.push((address(pn), own.push_name.clone()));
        if let Some(lid) = &own.lid {
            alias(&mut batch, pn, lid);
        }
    }
    for message in messages {
        known_alias(client, &message.info.source.chat, &mut batch).await?;
        known_alias(client, &message.info.source.sender, &mut batch).await?;
    }
    archive.ingest(batch).await
}
async fn remember_alias(client: &Client, archive: &Archive, jid: &Jid) -> archive::Result<()> {
    let mut batch = Batch::default();
    known_alias(client, jid, &mut batch).await?;
    if !batch.aliases.is_empty() {
        archive.ingest(batch).await?;
    }
    Ok(())
}

fn alias(batch: &mut Batch, first: &Jid, second: &Jid) {
    if first.is_lid() && second.is_pn() {
        batch.aliases.push((address(first), address(second)));
    } else if first.is_pn() && second.is_lid() {
        batch.aliases.push((address(second), address(first)));
    }
}

fn live_batch(messages: &[InboundMessage]) -> Batch {
    let mut batch = Batch {
        live: true,
        ..Default::default()
    };
    for inbound in messages {
        let info = &inbound.info;
        let source = &info.source;
        if !supported(&source.chat) {
            continue;
        }
        if let Some(alt) = &source.sender_alt {
            alias(&mut batch, &source.sender, alt);
        }
        if source.is_from_me {
            if let Some(alt) = &source.recipient_alt {
                alias(&mut batch, &source.chat, alt);
            }
        } else if !source.chat.is_group()
            && let Some(alt) = &source.sender_alt
        {
            alias(&mut batch, &source.chat, alt);
        }
        if !source.is_from_me {
            batch
                .profiles
                .push((address(&source.sender), info.push_name.to_string()));
        }
        batch.messages.push(record(
            &source.chat,
            &source.sender,
            info.id.to_string(),
            info.timestamp.timestamp(),
            source.is_from_me,
            info.push_name.to_string(),
            &inbound.message,
            inbound.ephemeral_expiration,
        ));
    }
    batch
}

#[allow(clippy::too_many_arguments)]
fn record(
    chat: &Jid,
    sender: &Jid,
    id: String,
    timestamp: i64,
    outgoing: bool,
    name: String,
    message: &wa::Message,
    ephemeral: Option<u32>,
) -> Record {
    let original = message;
    let (business_text, content, thumbnail) = crate::protocol_content::extract(original);
    let view_once = message.is_view_once();
    let message = message.get_base_message();
    let mut text = message.text_content().unwrap_or("").to_owned();
    let mut visible = true;
    let mut change = None;
    if view_once {
        text = "[View-once message · open on your phone]".into();
    } else if let Some(notice) = content.notice {
        text = notice.description().into();
    } else if let Some(protocol) = message.protocol_message.as_option() {
        visible = false;
        if let Some(key) = protocol.key.as_option() {
            let target_sender = if key.from_me == Some(true) {
                if outgoing {
                    "#self".into()
                } else {
                    address(sender)
                }
            } else {
                key.participant
                    .as_deref()
                    .and_then(|j| j.parse::<Jid>().ok())
                    .map(|j| address(&j))
                    .unwrap_or_else(|| address(sender))
            };
            let author = if outgoing {
                "#self".into()
            } else {
                address(sender)
            };
            let same_chat = key.remote_jid.as_deref().is_none_or(|j| {
                j.parse::<Jid>()
                    .ok()
                    .is_some_and(|j| address(&j) == address(chat))
            });
            if same_chat
                && target_sender == author
                && let Some(id) = &key.id
            {
                use wa::message::protocol_message::Type;
                if protocol.r#type == Some(Type::REVOKE) {
                    change = Some(Change {
                        id: id.clone(),
                        sender: target_sender,
                        text: None,
                        raw: Vec::new(),
                        version: timestamp,
                    });
                } else if protocol.r#type == Some(Type::MESSAGE_EDIT)
                    && let Some(edited) = protocol.edited_message.as_option()
                    && let Some(text) = crate::protocol_edits::text(edited)
                {
                    change = Some(Change {
                        id: id.clone(),
                        sender: target_sender,
                        text: Some(text),
                        raw: codec::message_to_vec(edited),
                        version: protocol.timestamp_ms.unwrap_or(timestamp * 1000),
                    });
                }
            }
        }
    } else if message.reaction_message.is_set() || message.sender_key_distribution_message.is_set()
    {
        visible = false;
    } else if message.image_message.is_set() {
        text = format!(
            "[Photo] {}",
            message.image_message.caption.as_deref().unwrap_or("")
        );
    } else if message.video_message.is_set() {
        text = format!(
            "[Video] {}",
            message.video_message.caption.as_deref().unwrap_or("")
        );
    } else if message.audio_message.is_set() {
        text = "[Audio message]".into();
    } else if message.document_message.is_set() {
        text = business_text.unwrap_or_else(|| "[Document]".into());
    } else if text.is_empty() {
        text = business_text.unwrap_or_else(|| "[Unsupported message]".into());
    }
    Record {
        chat: address(chat),
        sender: address(sender),
        id,
        timestamp,
        outgoing,
        sender_name: name,
        text,
        content,
        thumbnail,
        raw: if view_once || change.is_some() {
            Vec::new()
        } else {
            codec::message_to_vec(original)
        },
        visible,
        expires: ephemeral
            .filter(|n| *n > 0)
            .map(|n| timestamp.saturating_add(n as i64)),
        change,
    }
}

/// Decode saved envelopes with the same rules as new live/history messages.
pub fn repair_text(archive: &Archive) -> archive::Result<()> {
    archive.repair_unsupported_text(1, |raw| {
        let message = codec::message_decode(raw).ok()?;
        crate::protocol_business::text(&message)
    })?;
    archive.repair_message_content(|raw| {
        let message = codec::message_decode(raw).ok()?;
        let (_, content, thumbnail) = crate::protocol_content::extract(&message);
        (content != Default::default()).then_some((content, thumbnail))
    })
}

pub struct MessageEvents {
    pub sender: mpsc::Sender<Arc<Event>>,
    pub overflow: watch::Sender<bool>,
}
impl EventHandler for MessageEvents {
    fn interest(&self) -> EventInterest {
        EventInterest::of(&[
            EventKind::Messages,
            EventKind::HistorySync,
            EventKind::Receipt,
            EventKind::ServerAck,
            EventKind::SelfPushNameUpdated,
            EventKind::PictureUpdate,
            EventKind::DeleteMessageForMeUpdate,
        ])
    }
    fn handle_event(&self, event: Arc<Event>) {
        if matches!(event.as_ref(), Event::Messages(batch) if batch.hook_committed) {
            return;
        }
        if self.sender.try_send(event).is_err() {
            self.overflow.send_replace(true);
        }
    }
}

pub async fn event(archive: &Archive, client: &Client, event: Arc<Event>) -> archive::Result<()> {
    match event.as_ref() {
        Event::PictureUpdate(update) => {
            remember_alias(client, archive, &update.jid).await?;
            archive
                .invalidate_avatar(
                    address(&update.jid),
                    update.removed,
                    update.timestamp.timestamp(),
                )
                .await?;
        }
        Event::SelfPushNameUpdated(_) => {
            // The SDK updates its device snapshot before dispatch. Read the
            // latest value, so a queued older event cannot restore an old name.
            if client
                .persistence_manager()
                .get_device_snapshot()
                .pn
                .is_some()
            {
                verify_identity(client, archive).await?;
            }
        }
        Event::HistorySync(history) => {
            if let Some(request) = history.peer_data_request_session_id() {
                archive
                    .phone_history_metadata(
                        crate::protocol_history::chunk_id(history.compressed_bytes()),
                        request.into(),
                    )
                    .await?;
            }
        }
        Event::Messages(batch) => {
            ingest_messages(client, archive, &batch.messages).await?;
        }
        Event::ServerAck(ack) if ack.class.as_deref() == Some("message") => {
            if let Some(chat) = &ack.from {
                remember_alias(client, archive, chat).await?;
                archive
                    .delivery_at(
                        address(chat),
                        ack.id.clone(),
                        if ack.error.is_some() {
                            "failed"
                        } else {
                            "accepted"
                        },
                        ack.timestamp.map(|t| t.timestamp()),
                    )
                    .await?;
            }
        }
        Event::Receipt(receipt)
            if matches!(
                receipt.r#type,
                ReceiptType::ReadSelf | ReceiptType::PlayedSelf
            ) && supported(&receipt.source.chat) =>
        {
            remember_alias(client, archive, &receipt.source.chat).await?;
            archive
                .read_receipts(
                    address(&receipt.source.chat),
                    receipt
                        .message_ids
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                )
                .await?;
        }
        Event::DeleteMessageForMeUpdate(update) if supported(&update.chat_jid) => {
            remember_alias(client, archive, &update.chat_jid).await?;
            let sender = update.participant_jid.as_ref().unwrap_or(&update.chat_jid);
            remember_alias(client, archive, sender).await?;
            archive
                .hide_remote(
                    address(&update.chat_jid),
                    address(sender),
                    update.message_id.clone(),
                    update.from_me,
                )
                .await?;
        }
        Event::Receipt(receipt) if !receipt.source.is_from_me => {
            remember_alias(client, archive, &receipt.source.chat).await?;
            let status = match receipt.r#type {
                ReceiptType::Delivered => "delivered",
                ReceiptType::Read | ReceiptType::Played => "read",
                ReceiptType::Sent => "accepted",
                _ => return Ok(()),
            };
            if matches!(
                receipt.r#type,
                ReceiptType::Delivered | ReceiptType::Read | ReceiptType::Played
            ) {
                remember_alias(client, archive, &receipt.source.sender).await?;
                archive
                    .recipient_receipts(
                        address(&receipt.source.chat),
                        receipt
                            .message_ids
                            .iter()
                            .map(ToString::to_string)
                            .collect(),
                        address(&receipt.source.sender),
                        match receipt.r#type {
                            ReceiptType::Delivered => "delivered",
                            ReceiptType::Read => "read",
                            _ => "played",
                        },
                        receipt.timestamp.timestamp(),
                    )
                    .await?;
            }
            for id in &receipt.message_ids {
                archive
                    .delivery(address(&receipt.source.chat), id.to_string(), status)
                    .await?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub async fn history_worker(
    archive: Archive,
    stop: Arc<AtomicBool>,
    mut shutdown: watch::Receiver<bool>,
) -> archive::Result<()> {
    let mut changed = archive.subscribe();
    loop {
        let copy = archive.clone();
        let cancelled = stop.clone();
        tokio::task::spawn_blocking(move || import_pending(&copy, &cancelled))
            .await
            .map_err(|_| archive::Error::Database)??;
        tokio::select! { _ = shutdown.changed() => return Ok(()), _ = changed.changed() => {} }
    }
}

fn import_pending(archive: &Archive, stop: &AtomicBool) -> archive::Result<()> {
    while !stop.load(Ordering::Acquire) {
        let Some((id, data, progress)) = archive.next_history()? else {
            break;
        };
        if import_chunk(archive, &id, &data, progress, stop).is_err() {
            archive.history_progress(&id, progress, "failed")?;
        }
    }
    Ok(())
}
fn import_chunk(
    archive: &Archive,
    id: &str,
    data: &[u8],
    progress: usize,
    stop: &AtomicBool,
) -> archive::Result<()> {
    let mut stream = HistorySyncStream::new(data, MAX_DECOMPRESSED);
    let mut index = 0;
    let mut pages = Vec::new();
    while let Some(bytes) = stream
        .next_conversation_bytes()
        .map_err(|_| archive::Error::Invalid)?
    {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        index += 1;
        let conversation =
            wa::Conversation::decode_from_slice(bytes).map_err(|_| archive::Error::Invalid)?;
        let Ok(chat) = conversation.id.parse::<Jid>() else {
            return Err(archive::Error::Invalid);
        };
        if !supported(&chat) {
            archive.history_progress(id, index, "pending")?;
            continue;
        }
        use wa::conversation::EndOfHistoryTransferType as End;
        let more = match conversation.end_of_history_transfer_type {
            Some(
                End::COMPLETE_AND_NO_MORE_MESSAGE_REMAIN_ON_PRIMARY
                | End::COMPLETE_ON_DEMAND_SYNC_WITH_MORE_MSG_ON_PRIMARY_BUT_NO_ACCESS,
            ) => Some(false),
            Some(
                End::COMPLETE_BUT_MORE_MESSAGES_REMAIN_ON_PRIMARY
                | End::COMPLETE_ON_DEMAND_SYNC_BUT_MORE_MSG_REMAIN_ON_PRIMARY,
            ) => Some(true),
            _ => None,
        };
        pages.push((address(&chat), more));
        if index <= progress {
            continue;
        }
        let mut batch = Batch::default();
        for candidate in [
            conversation.pn_jid.as_deref(),
            conversation.lid_jid.as_deref(),
            conversation.old_jid.as_deref(),
            conversation.new_jid.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(other) = candidate.parse::<Jid>() {
                alias(&mut batch, &chat, &other);
            }
        }
        batch.chats.push(ChatMeta {
            jid: address(&chat),
            name: conversation
                .name
                .clone()
                .or(conversation.display_name.clone())
                .unwrap_or_default(),
            group: chat.is_group(),
            read_only: conversation.read_only,
            ephemeral: conversation.ephemeral_expiration,
        });
        for item in &conversation.messages {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            let Some(message) = item.message.as_option() else {
                continue;
            };
            let key = &message.key;
            let Some(wire_id) = &key.id else {
                continue;
            };
            let Some(body) = message.message.as_option() else {
                continue;
            };
            let outgoing = key.from_me.unwrap_or(false);
            let sender = key
                .participant
                .as_deref()
                .or(message.participant.as_deref())
                .and_then(|s| s.parse::<Jid>().ok())
                .unwrap_or_else(|| chat.clone());
            // A group message without an author cannot be deduplicated safely.
            if chat.is_group() && !outgoing && sender == chat {
                continue;
            }
            batch.messages.push(record(
                &chat,
                &sender,
                wire_id.clone(),
                message.message_timestamp.unwrap_or(0).min(i64::MAX as u64) as i64,
                outgoing,
                message.push_name.clone().unwrap_or_default(),
                body,
                message
                    .ephemeral_duration
                    .or(conversation.ephemeral_expiration),
            ));
            if batch.messages.len() == 100 {
                archive.ingest_blocking(std::mem::take(&mut batch))?;
            }
        }
        archive.ingest_blocking(batch)?;
        if let Some(count) = conversation.unread_count {
            let through = conversation
                .last_msg_timestamp
                .or(conversation.conversation_timestamp)
                .or_else(|| {
                    conversation
                        .messages
                        .iter()
                        .filter_map(|m| m.message.message_timestamp)
                        .max()
                })
                .map(|stamp| seconds(stamp.min(i64::MAX as u64) as i64))
                .unwrap_or(0);
            archive.history_unread(
                address(&chat),
                through,
                count,
                conversation.marked_as_unread,
            )?;
        }
        // A crash within a conversation replays that conversation idempotently.
        archive.history_progress(id, index, "pending")?;
    }
    let remainder = stream.remainder().map_err(|_| archive::Error::Invalid)?;
    let mut batch = Batch::default();
    for mapping in remainder.phone_number_to_lid_mappings {
        if let (Some(pn), Some(lid)) = (mapping.pn_jid, mapping.lid_jid)
            && let (Ok(pn), Ok(lid)) = (pn.parse::<Jid>(), lid.parse::<Jid>())
        {
            alias(&mut batch, &pn, &lid);
        }
    }
    for name in remainder.pushnames {
        if let (Some(id), Some(name)) = (name.id, name.pushname)
            && let Ok(jid) = id.parse::<Jid>()
            && supported(&jid)
        {
            batch.profiles.push((address(&jid), name));
        }
    }
    archive.ingest_blocking(batch)?;
    archive.finish_history(id, index, pages)
}

/// Some older sync producers encoded range timestamps in milliseconds.
pub(crate) fn seconds(stamp: i64) -> i64 {
    if stamp > 100_000_000_000 {
        stamp / 1000
    } else {
        stamp
    }
}

pub fn new_chat(value: &str) -> archive::Result<String> {
    crate::domain::phone_number(value)
        .map(|digits| format!("{digits}@s.whatsapp.net"))
        .ok_or(archive::Error::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{PN, fixture};
    use whatsapp_rust::prelude::MessageBuilderExt;
    use whatsapp_rust::waproto::buffa::MessageField;

    pub(super) fn mapped(message: &wa::Message, outgoing: bool) -> Record {
        let jid: Jid = PN.parse().unwrap();
        record(
            &jid,
            &jid,
            "synthetic-envelope".into(),
            1_700_000_000,
            outgoing,
            "Synthetic".into(),
            message,
            None,
        )
    }
    #[test]
    fn text_wrappers_and_sender_revokes_map_without_exposing_view_once() {
        let body = wa::Message::text("Synthetic 👩🏽‍💻\ntext");
        let mut wrapper = wa::message::FutureProofMessage::default();
        wrapper.message = MessageField::some(body);
        let mut wrapped = wa::Message::default();
        wrapped.ephemeral_message = MessageField::some(wrapper.clone());
        let value = mapped(&wrapped, false);
        assert_eq!(value.text, "Synthetic 👩🏽‍💻\ntext");
        assert_eq!(value.raw, codec::message_to_vec(&wrapped));
        let mut once = wa::Message::default();
        once.view_once_message = MessageField::some(wrapper);
        let value = mapped(&once, false);
        assert!(value.text.starts_with("[View-once"));
        assert!(value.raw.is_empty());
        let mut key = wa::MessageKey::default();
        key.id = Some("original".into());
        key.from_me = Some(true);
        key.remote_jid = Some(PN.into());
        let mut protocol = wa::message::ProtocolMessage::default();
        protocol.key = MessageField::some(key);
        protocol.r#type = Some(wa::message::protocol_message::Type::REVOKE);
        let mut revoke = wa::Message::default();
        revoke.protocol_message = MessageField::some(protocol);
        let incoming = mapped(&revoke, false);
        assert!(!incoming.visible);
        assert_eq!(incoming.change.unwrap().sender, PN);
        assert_eq!(mapped(&revoke, true).change.unwrap().sender, "#self");
        assert!(new_chat("+1 (555) 000-0001").is_ok());
        assert!(new_chat("@1555").is_err());
        assert!(new_chat("0123456789").is_err());
    }
    pub(super) fn compressed(history: &wa::HistorySync) -> Vec<u8> {
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        z.write_all(&history.encode_to_vec()).unwrap();
        z.finish().unwrap()
    }
    #[tokio::test]
    async fn history_chunks_replay_resume_and_retain_malformed_input_for_recovery() {
        let (_temp, db) = fixture();
        let messages = (0..205)
            .map(|i| {
                let mut key = wa::MessageKey::default();
                key.remote_jid = Some(PN.into());
                key.id = Some(format!("history-{i}"));
                key.from_me = Some(false);
                let mut info = wa::WebMessageInfo::default();
                info.key = MessageField::some(key);
                info.message =
                    MessageField::some(wa::Message::text(format!("Synthetic history {i}")));
                info.message_timestamp = Some(1_700_000_000);
                let mut item = wa::HistorySyncMsg::default();
                item.message = MessageField::some(info);
                item
            })
            .collect();
        let mut conversation = wa::Conversation::default();
        conversation.id = PN.into();
        conversation.name = Some("History contact".into());
        conversation.messages = messages;
        conversation.unread_count = Some(5);
        conversation.last_msg_timestamp = Some(1_700_000_000);
        let mut history = wa::HistorySync::default();
        history.conversations = vec![conversation];
        let data = compressed(&history);
        db.capture_history("chunk".into(), data.clone())
            .await
            .unwrap();
        db.capture_history("chunk".into(), data.clone())
            .await
            .unwrap();
        // Cancellation keeps the captured bytes pending and a replay can restart.
        import_pending(&db, &AtomicBool::new(true)).unwrap();
        assert!(db.next_history().unwrap().is_some());
        let stop = AtomicBool::new(false);
        import_pending(&db, &stop).unwrap();
        assert!(db.next_history().unwrap().is_none());
        let chat = db.create_chat(PN.into()).await.unwrap();
        let first = db
            .view(chat, 300, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert_eq!(first.messages.len(), 205);
        assert_eq!(first.chat.title, "History contact");
        assert_eq!(first.chat.unread, 5);
        // Simulate a crash after commit but before the conversation checkpoint.
        import_chunk(&db, "chunk", &data, 0, &stop).unwrap();
        // And a restart after the conversation checkpoint but before finalization.
        import_chunk(&db, "chunk", &data, 1, &stop).unwrap();
        assert_eq!(
            db.view(chat, 300, String::new(), false, false)
                .await
                .unwrap()
                .conversation
                .unwrap()
                .messages
                .len(),
            205
        );
        db.blocking(|c| {
            let bytes: Option<Vec<u8>> = c.query_row(
                "SELECT data FROM history_chunks WHERE id='chunk'",
                [],
                |r| r.get(0),
            )?;
            assert!(bytes.is_none());
            Ok(())
        })
        .unwrap();
        db.capture_history("malformed".into(), vec![0, 1, 2, 3])
            .await
            .unwrap();
        import_pending(&db, &stop).unwrap();
        assert!(
            db.view(0, 100, String::new(), false, false)
                .await
                .unwrap()
                .history_failed
        );
        db.blocking(|c| {
            let n: i64 = c.query_row(
                "SELECT length(data) FROM history_chunks WHERE id='malformed' AND state='failed'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(n, 4);
            Ok(())
        })
        .unwrap();
    }
}
