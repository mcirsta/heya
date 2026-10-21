//! Bounded app-state materialization for contacts and incoming read positions.
//! The SDK persists sync cursors, so Heya must recover events missed by old builds
//! or interrupted archive writes from a fresh snapshot.
use crate::{
    archive::{self, Archive, Contact, ReadUpdate},
    protocol_messages::{address, verify_identity},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, watch};
use whatsapp_rust::{
    AppStateResyncMode, Client, WAPatchName,
    prelude::{Event, EventHandler, EventInterest, EventKind},
    wacore_binary::{Jid, JidExt},
};

#[derive(Default)]
struct Pending {
    chats: BTreeMap<(String, crate::chat_management::Setting), crate::chat_management::Update>,
    names: BTreeMap<String, Contact>,
    reads: BTreeMap<String, ReadUpdate>,
    calls: BTreeMap<(String, String), whatsapp_rust::wacore::types::events::CallLogSync>,
    aliases: BTreeMap<String, String>,
    bytes: usize,
    failed: bool,
}
#[derive(Default)]
struct Mailbox {
    pending: Mutex<Pending>,
    changed: Notify,
}
#[derive(Clone, Default)]
pub struct AppState(Arc<Mailbox>);
impl AppState {
    fn take(&self) -> archive::Result<Pending> {
        let mut pending = self
            .0
            .pending
            .lock()
            .map_err(|_| archive::Error::Database)?;
        if pending.failed {
            return Err(archive::Error::Limit);
        }
        Ok(std::mem::take(&mut pending))
    }
    pub(super) async fn flush(&self, archive: &Archive, replace: bool) -> archive::Result<()> {
        let pending = self.take()?;
        if replace || !pending.names.is_empty() || !pending.aliases.is_empty() {
            archive
                .save_contacts(
                    pending.names.into_values().collect(),
                    pending.aliases.into_iter().collect(),
                    replace,
                )
                .await?;
        }
        if !pending.reads.is_empty() {
            archive
                .read_updates(pending.reads.into_values().collect())
                .await?;
        }
        archive
            .save_chat_settings(pending.chats.into_values().collect())
            .await?;
        crate::protocol_calls::save(archive, pending.calls.into_values().collect()).await?;
        Ok(())
    }
}

/// Inline, bounded, coalesced by contact identity. Full address-book snapshots
/// must not overflow the small receipt mailbox or enqueue thousands of UI jobs.
/// This callback never does I/O, logs data or waits on a database operation.
impl EventHandler for AppState {
    fn interest(&self) -> EventInterest {
        EventInterest::of(&[
            EventKind::ContactUpdate,
            EventKind::ContactRemoved,
            EventKind::MarkChatAsReadUpdate,
            EventKind::MuteUpdate,
            EventKind::ArchiveUpdate,
            EventKind::LockChatUpdate,
            EventKind::CallLogSync,
        ])
    }
    fn handle_event(&self, event: Arc<Event>) {
        if let Event::CallLogSync(call) = event.as_ref() {
            use whatsapp_rust::waproto::buffa::Message as _;
            let mut pending = self.0.pending.lock().expect("app-state mailbox poisoned");
            let key = (address(&call.call_creator_jid), call.call_id.clone());
            let size = call.record.encoded_len() as usize + key.0.len() + key.1.len();
            if pending.failed {
                return;
            }
            let previous = pending.calls.get(&key);
            if previous.is_some_and(|old| call.timestamp < old.timestamp) {
                return;
            }
            let bytes = pending
                .bytes
                .saturating_sub(previous.map_or(0, |old| {
                    old.record.encoded_len() as usize + key.0.len() + key.1.len()
                }))
                .saturating_add(size);
            if (pending.calls.len() >= 100_000 && previous.is_none()) || bytes > 32 * 1024 * 1024 {
                pending.failed = true;
            } else {
                pending.calls.insert(key, call.clone());
                pending.bytes = bytes;
            }
            drop(pending);
            self.0.changed.notify_one();
            return;
        }
        if let Some(next) = chat_update(&event) {
            let mut pending = self.0.pending.lock().expect("app-state mailbox poisoned");
            let key = (next.jid.clone(), next.setting);
            if pending.failed {
                return;
            }
            if pending.chats.len() >= 100_000 && !pending.chats.contains_key(&key) {
                pending.failed = true;
            } else if pending
                .chats
                .get(&key)
                .is_none_or(|old| next.supersedes(old))
            {
                pending.chats.insert(key, next);
            }
            drop(pending);
            self.0.changed.notify_one();
            return;
        }
        if let Event::MarkChatAsReadUpdate(update) = event.as_ref() {
            if !(update.jid.is_pn() || update.jid.is_lid() || update.jid.is_group()) {
                return;
            }
            let Some(read) = update.action.read else {
                return;
            };
            let through = read
                .then(|| update.action.message_range.as_option())
                .flatten()
                .and_then(|range| {
                    range
                        .last_message_timestamp
                        .or_else(|| range.messages.iter().filter_map(|m| m.timestamp).max())
                })
                .map(crate::protocol_messages::seconds);
            let message = update.action.message_range.as_option().and_then(|range| {
                range
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m.timestamp.map(crate::protocol_messages::seconds) == through)
                    .and_then(|m| {
                        let key = m.key.as_option()?;
                        Some(crate::archive::ReadMessage {
                            wire: key.id.clone()?,
                            sender: key.participant.clone().unwrap_or_default(),
                            outgoing: key.from_me?,
                        })
                    })
            });
            let next = ReadUpdate {
                message,
                jid: address(&update.jid),
                through,
                marked: !read,
                version: update.action_timestamp.map(|t| t.timestamp_millis()),
            };
            let mut pending = self.0.pending.lock().expect("app-state mailbox poisoned");
            if pending.failed {
                return;
            }
            if pending.reads.len() >= 100_000 && !pending.reads.contains_key(&next.jid) {
                pending.failed = true;
            } else if let Some(old) = pending.reads.get_mut(&next.jid) {
                old.coalesce(next);
            } else {
                pending.reads.insert(next.jid.clone(), next);
            }
            drop(pending);
            self.0.changed.notify_one();
            return;
        }
        let (jid, name, timestamp, others) = match event.as_ref() {
            Event::ContactUpdate(update) => {
                let name = update
                    .action
                    .full_name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .or(update.action.first_name.as_deref())
                    .unwrap_or_default();
                (
                    &update.jid,
                    name,
                    update.action_timestamp,
                    [
                        update.action.pn_jid.as_deref(),
                        update.action.lid_jid.as_deref(),
                    ],
                )
            }
            Event::ContactRemoved(update) => {
                (&update.jid, "", update.action_timestamp, [None, None])
            }
            _ => return,
        };
        if !jid.is_pn() && !jid.is_lid() {
            return;
        }
        let mut pending = self.0.pending.lock().expect("contact mailbox poisoned");
        if pending.failed {
            return;
        }
        // Explicit bound for an unexpectedly large/untrusted sync response.
        if pending.names.len() >= 100_000
            || pending.bytes.saturating_add(name.len()) > 32 * 1024 * 1024
        {
            pending.failed = true;
            self.0.changed.notify_one();
            return;
        }
        let mut pn = jid.is_pn().then(|| address(jid));
        let mut lid = jid.is_lid().then(|| address(jid));
        for other in others
            .into_iter()
            .flatten()
            .filter_map(|s| s.parse::<Jid>().ok())
        {
            if other.is_pn() {
                pn = Some(address(&other));
            } else if other.is_lid() {
                lid = Some(address(&other));
            }
        }
        if let (Some(lid), Some(pn)) = (lid, pn)
            && let Some(old) = pending.aliases.insert(lid, pn.clone())
            && old != pn
        {
            pending.failed = true;
        }
        let next = Contact {
            jid: address(jid),
            name: name.to_owned(),
            timestamp: timestamp.map(|t| t.timestamp_millis()),
        };
        if pending
            .names
            .get(&next.jid)
            .is_none_or(|old| next.supersedes(old))
        {
            let old = pending.names.insert(next.jid.clone(), next);
            pending.bytes = pending
                .bytes
                .saturating_sub(old.map_or(0, |c| c.name.len()))
                + name.len();
        }
        drop(pending);
        self.0.changed.notify_one();
    }
}

fn chat_update(event: &Event) -> Option<crate::chat_management::Update> {
    use crate::chat_management::{Setting, Update};
    let (jid, setting, value, stamp) = match event {
        Event::MuteUpdate(u) => (
            &u.jid,
            Setting::Mute,
            if u.action.muted? {
                u.action.mute_end_timestamp.unwrap_or(-1)
            } else {
                0
            },
            u.action_timestamp,
        ),
        Event::ArchiveUpdate(u) => (
            &u.jid,
            Setting::Archive,
            i64::from(u.action.archived?),
            u.action_timestamp,
        ),
        Event::LockChatUpdate(u) => (
            &u.jid,
            Setting::Lock,
            i64::from(u.action.locked?),
            u.action_timestamp,
        ),
        _ => return None,
    };
    if !(jid.is_pn() || jid.is_lid() || jid.is_group()) || value < -1 {
        return None;
    }
    Some(Update {
        jid: address(jid),
        setting,
        value,
        stamp: stamp.map(|t| t.timestamp_millis()),
    })
}

#[cfg(test)]
#[path = "protocol_chat_settings_tests.rs"]
mod chat_settings_tests;

pub async fn run(
    archive: Archive,
    client: Arc<Client>,
    contacts: AppState,
    mut online: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
    mut rebuild: bool,
) -> archive::Result<bool> {
    let mut verified = false;
    let mut fatal = false;
    let mut next_attempt = tokio::time::Instant::now();
    let mut retry = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => {
                if verified {
                    contacts.flush(&archive, false).await?;
                }
                return Ok(verified && !rebuild);
            }
            result = online.changed() => { if result.is_err() { return Ok(false); } },
            _ = contacts.0.changed.notified() => {},
            _ = retry.tick() => {},
        }
        if !*online.borrow() {
            continue;
        }
        if !verified {
            verify_identity(&client, &archive).await?;
            verified = true;
            let own = client.persistence_manager().get_device_snapshot();
            if let Some(pn) = &own.pn {
                let mut batch = crate::archive::Batch::default();
                batch.profiles.push((address(pn), own.push_name.clone()));
                if let Some(lid) = &own.lid {
                    batch.aliases.push((address(lid), address(pn)));
                }
                archive.ingest(batch).await?;
            }
            rebuild |= archive.needs_call_snapshot().await?;
        }
        contacts.flush(&archive, false).await?;
        if rebuild && !fatal && tokio::time::Instant::now() >= next_attempt {
            // with_event_handler is synchronous: a successful report means all
            // replayed contact events have reached this mailbox. Keep their
            // names staged until success, then replace atomically, including
            // removal of contacts absent from the snapshot. Incremental events
            // interleaved with replay are folded by mutation timestamp.
            let result = tokio::select! {
                biased;
                _ = stop.changed() => return Ok(false), // dirty mark survives cancellation
                result = client.resync_app_state([WAPatchName::CriticalUnblockLow, WAPatchName::RegularLow, WAPatchName::RegularHigh, WAPatchName::Regular], AppStateResyncMode::Snapshot) => result,
            };
            let complete = result.as_ref().is_ok_and(|r| {
                r.synced.contains(&WAPatchName::CriticalUnblockLow)
                    && r.synced.contains(&WAPatchName::RegularLow)
                    && r.synced.contains(&WAPatchName::RegularHigh)
                    && r.synced.contains(&WAPatchName::Regular)
                    && r.all_synced()
            });
            fatal = result.as_ref().is_ok_and(|r| !r.fatal.is_empty());
            contacts.flush(&archive, complete).await?;
            rebuild = !complete;
            next_attempt = tokio::time::Instant::now() + Duration::from_secs(30);
            if complete {
                archive.finish_call_snapshot().await?;
                eprintln!("Heya: contacts, calls and read state synchronized");
            } else {
                eprintln!("Heya: app-state sync incomplete; saved state retained");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{
        Batch, ChatMeta,
        tests::{LID, PN, fixture, record},
    };
    use whatsapp_rust::{
        prelude::wa,
        wacore::types::events::{ContactRemoved, ContactUpdate},
    };

    fn update(jid: &str, name: &str, stamp: i64, pn: Option<&str>) -> Arc<Event> {
        let mut action = wa::sync_action_value::ContactAction::default();
        action.full_name = Some(name.into());
        action.first_name = Some("Synthetic first name".into());
        action.pn_jid = pn.map(str::to_owned);
        let stamp = chrono::DateTime::from_timestamp_millis(stamp).unwrap();
        Arc::new(Event::ContactUpdate(
            ContactUpdate::builder()
                .jid(jid.parse().unwrap())
                .timestamp(stamp)
                .action_timestamp(stamp)
                .action(Box::new(action))
                .from_full_sync(true)
                .build(),
        ))
    }
    async fn title(archive: &Archive, chat: i32) -> String {
        archive
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .chat
            .title
    }

    #[tokio::test]
    async fn read_echo_keeps_the_own_message_boundary_with_same_second_arrivals() {
        use whatsapp_rust::wacore::types::events::MarkChatAsReadUpdate;
        let (_temp, archive) = fixture();
        let mut old = record("old");
        old.timestamp = 100;
        let mut own = record("own");
        own.timestamp = 200;
        own.outgoing = true;
        let mut newer = record("newer");
        newer.timestamp = 200;
        archive
            .ingest(Batch {
                messages: vec![old, own, newer],
                live: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let jid = PN.parse().unwrap();
        let reference = whatsapp_rust::MessageRef::new(
            &jid,
            whatsapp_rust::MessageId::new("own").unwrap(),
            None,
            true,
        )
        .unwrap();
        let mut action = wa::sync_action_value::MarkChatAsReadAction::default();
        action.read = Some(true);
        action.message_range = whatsapp_rust::waproto::buffa::MessageField::some(
            whatsapp_rust::message_range(200, None, [(reference, 200)]),
        );
        let stamp = chrono::Utc::now();
        let state = AppState::default();
        state.handle_event(Arc::new(Event::MarkChatAsReadUpdate(
            MarkChatAsReadUpdate::builder()
                .jid(jid)
                .timestamp(stamp)
                .action_timestamp(stamp)
                .action(Box::new(action))
                .from_full_sync(false)
                .build(),
        )));
        state.flush(&archive, false).await.unwrap();
        let view = archive
            .view(0, 100, String::new(), false, false)
            .await
            .unwrap();
        assert_eq!(
            view.chats[0].unread, 1,
            "an echo must not round a keyed read up to the whole second"
        );
    }

    #[tokio::test]
    async fn read_snapshot_bursts_coalesce_and_never_create_phantom_sidebar_chats() {
        use whatsapp_rust::wacore::types::events::MarkChatAsReadUpdate;
        use whatsapp_rust::waproto::buffa::MessageField;
        let (_temp, archive) = fixture();
        let state = AppState::default();
        for i in 0..2048 {
            let stamp = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
            let mut action = wa::sync_action_value::MarkChatAsReadAction::default();
            action.read = Some(true);
            action.message_range = MessageField::some(whatsapp_rust::message_range(
                1_700_000_000,
                None,
                Vec::new(),
            ));
            let event = Arc::new(Event::MarkChatAsReadUpdate(
                MarkChatAsReadUpdate::builder()
                    .jid(format!("155501{i:05}@s.whatsapp.net").parse().unwrap())
                    .timestamp(stamp)
                    .action_timestamp(stamp)
                    .action(Box::new(action))
                    .from_full_sync(true)
                    .build(),
            ));
            state.handle_event(event.clone());
            state.handle_event(event);
        }
        assert_eq!(state.0.pending.lock().unwrap().reads.len(), 2048);
        state.flush(&archive, false).await.unwrap();
        assert!(
            archive
                .view(0, 100, String::new(), false, false)
                .await
                .unwrap()
                .chats
                .is_empty()
        );
        let mut message = record("late-offline");
        message.chat = "15550100000@s.whatsapp.net".into();
        archive
            .ingest(Batch {
                messages: vec![message.clone()],
                live: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            archive
                .view(0, 100, String::new(), false, false)
                .await
                .unwrap()
                .chats[0]
                .unread,
            0
        );
        message.id = "new".into();
        message.timestamp += 1;
        archive
            .ingest(Batch {
                messages: vec![message],
                live: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            archive
                .view(0, 100, String::new(), false, false)
                .await
                .unwrap()
                .chats[0]
                .unread,
            1
        );
    }

    #[tokio::test]
    async fn phonebook_names_cover_self_new_chats_aliases_and_group_senders() {
        let (_temp, archive) = fixture();
        let contacts = AppState::default();
        // Arrival before any message must not create a phantom conversation.
        contacts.handle_event(update(LID, "Saved 👩🏽‍💻 contact", 20, Some(PN)));
        contacts.flush(&archive, false).await.unwrap();
        assert!(
            archive
                .view(0, 100, String::new(), false, false)
                .await
                .unwrap()
                .chats
                .is_empty()
        );
        let chat = archive.create_chat(LID.into()).await.unwrap();
        assert_eq!(title(&archive, chat).await, "Saved 👩🏽‍💻 contact");
        let mut message = record("self-echo");
        message.outgoing = true;
        message.sender_name = String::new();
        archive
            .ingest(Batch {
                messages: vec![message],
                profiles: vec![(PN.into(), "Profile name".into())],
                chats: vec![ChatMeta {
                    jid: PN.into(),
                    name: "History name".into(),
                    ..Default::default()
                }],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(title(&archive, chat).await, "Saved 👩🏽‍💻 contact");
        let mut message = record("group-inbound");
        message.chat = "15550000001-42@g.us".into();
        message.sender = LID.into();
        message.sender_name = "Profile name".into();
        archive
            .ingest(Batch {
                messages: vec![message],
                ..Default::default()
            })
            .await
            .unwrap();
        let group = archive
            .create_chat("15550000001-42@g.us".into())
            .await
            .unwrap();
        let view = archive
            .view(group, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert_eq!(view.messages[0].sender, "Saved 👩🏽‍💻 contact");
        // A rename applies to already archived messages and sidebar/header alike.
        contacts.handle_event(update(PN, "Renamed contact", 30, None));
        contacts.handle_event(update(PN, "Stale replay", 10, None));
        contacts.flush(&archive, false).await.unwrap();
        assert_eq!(title(&archive, chat).await, "Renamed contact");
        let stamp = chrono::DateTime::from_timestamp_millis(40).unwrap();
        contacts.handle_event(Arc::new(Event::ContactRemoved(
            ContactRemoved::builder()
                .jid(PN.parse().unwrap())
                .timestamp(stamp)
                .action_timestamp(stamp)
                .from_full_sync(false)
                .build(),
        )));
        contacts.flush(&archive, false).await.unwrap();
        assert_eq!(
            title(&archive, chat).await,
            "Profile name",
            "removal must not resurrect the historical saved name"
        );
        contacts.handle_event(update(PN, "Stale replay after remove", 30, None));
        contacts.flush(&archive, false).await.unwrap();
        assert_eq!(title(&archive, chat).await, "Profile name");
    }

    #[tokio::test]
    async fn contact_snapshot_bursts_coalesce_and_replace_atomically() {
        let (_temp, archive) = fixture();
        let contacts = AppState::default();
        let chat = archive.create_chat(PN.into()).await.unwrap();
        contacts.handle_event(update(PN, "Saved contact", 1, None));
        contacts.flush(&archive, false).await.unwrap();
        for i in 0..2048 {
            let jid = format!("155501{i:05}@s.whatsapp.net");
            contacts.handle_event(update(&jid, "Old synthetic name", 1, None));
            contacts.handle_event(update(&jid, "New synthetic name", 2, None));
        }
        assert_eq!(contacts.0.pending.lock().unwrap().names.len(), 2048);
        contacts.flush(&archive, false).await.unwrap();
        archive
            .blocking(|db| {
                let count: i64 = db.query_row(
                    "SELECT count(*) FROM contacts WHERE saved_name='New synthetic name'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(count, 2048);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            archive
                .view(0, 10, String::new(), false, false)
                .await
                .unwrap()
                .chats
                .len(),
            1
        );
        contacts.handle_event(update(PN, "Must roll back", 2, None));
        archive.blocking(|db| {
            db.execute_batch("CREATE TRIGGER fail_contacts BEFORE INSERT ON contacts BEGIN SELECT RAISE(ABORT,'synthetic disk full'); END;")?;
            Ok(())
        }).unwrap();
        assert!(contacts.flush(&archive, true).await.is_err());
        assert_eq!(
            title(&archive, chat).await,
            "Saved contact",
            "failed replacement rolls back removal"
        );
        archive
            .blocking(|db| {
                db.execute_batch("DROP TRIGGER fail_contacts")?;
                Ok(())
            })
            .unwrap();
        // Full snapshot excludes this contact: only its saved name is removed.
        contacts.flush(&archive, true).await.unwrap();
        assert_eq!(title(&archive, chat).await, "15550000001");
        contacts.handle_event(update(PN, "   ", 50, None));
        contacts.flush(&archive, false).await.unwrap();
        assert_eq!(title(&archive, chat).await, "Synthetic first name");
    }
}
