//! Synthetic phone call logs. No calls are made and no account data is read.
use super::*;
use crate::{
    archive::tests::{KEY, LID, PN, fixture},
    message_content::{CallOutcome, Content, Notice},
    protocol_calls as calls,
};
use whatsapp_rust::{wacore::types::events::CallLogSync, waproto::buffa::MessageField};
const OWN: &str = "15550000099@s.whatsapp.net";
const OWN_LID: &str = "10000000099@lid";
fn log(id: &str, creator: &str) -> wa::CallLogRecord {
    let mut log = wa::CallLogRecord::default();
    log.call_id = Some(id.into());
    log.call_creator_jid = Some(creator.into());
    log.start_time = Some(1_700_000_100);
    log.duration = Some(125);
    log.is_video = Some(false);
    log.call_result = Some(wa::call_log_record::CallResult::CONNECTED);
    let mut peer = wa::call_log_record::ParticipantInfo::default();
    peer.user_jid = Some(PN.into());
    let mut own = wa::call_log_record::ParticipantInfo::default();
    own.user_jid = Some(OWN_LID.into());
    log.participants = vec![own, peer];
    log
}
fn sync(log: wa::CallLogRecord, creator: &str, revision: i64, outgoing: bool) -> Arc<Event> {
    Arc::new(Event::CallLogSync(
        CallLogSync::builder()
            .call_creator_jid(creator.parse().unwrap())
            .call_id(log.call_id.clone().unwrap())
            .from_me(outgoing)
            .timestamp(chrono::DateTime::from_timestamp_millis(revision).unwrap())
            .record(Box::new(log))
            .from_full_sync(false)
            .build(),
    ))
}
#[test]
fn call_mapping_uses_creator_identity_and_real_start_time_and_handles_unknowns() {
    let own = vec![OWN.parse().unwrap(), OWN_LID.parse().unwrap()];
    let mut log = log("placed", OWN);
    // The wire flag is inconsistent between clients and is deliberately ignored.
    for incoming in [true, false] {
        log.is_incoming = Some(incoming);
        let row = calls::history(&log, &own).unwrap();
        assert!(row.outgoing);
        assert_eq!(row.chat, PN);
        assert_eq!(row.timestamp, 1_700_000_100);
        assert_eq!(row.text, "Voice call");
        assert!(row.raw.is_empty());
    }
    log.call_creator_jid = Some(PN.into());
    log.is_video = Some(true);
    log.call_result = Some(wa::call_log_record::CallResult::MISSED);
    let row = calls::history(&log, &own).unwrap();
    assert!(!row.outgoing);
    assert_eq!(row.text, "Missed video call");
    log.call_result = Some(wa::call_log_record::CallResult::UPCOMING);
    let scheduled = calls::history(&log, &own).unwrap();
    let Some(Notice::Call(call)) = scheduled.content.notice else {
        panic!("missing scheduled call")
    };
    assert_eq!(call.details(false), "Incoming · Scheduled");
    log.group_jid = Some("15550000000-1@g.us".into());
    assert_eq!(
        calls::history(&log, &own).unwrap().chat,
        "15550000000-1@g.us"
    );
    log.group_jid = None;
    log.call_creator_jid = Some(OWN.into());
    log.is_call_link = Some(true);
    assert!(calls::history(&log, &own).is_none());
    log.is_call_link = Some(false);
    let mut other = wa::call_log_record::ParticipantInfo::default();
    other.user_jid = Some("15550000002@s.whatsapp.net".into());
    log.participants.push(other);
    assert!(calls::history(&log, &own).is_none());
    log.start_time = None;
    assert!(calls::history(&log, &own).is_none());
    log.start_time = Some(i64::MAX);
    assert!(calls::history(&log, &own).is_none());
    log.start_time = Some(1);
    log.call_creator_jid = None;
    assert!(calls::history(&log, &own).is_none());
    assert!(calls::history(&log, &[]).is_none());
}

#[tokio::test]
async fn synced_calls_update_one_row_preserve_messages_and_never_send_message_receipts() {
    use whatsapp_rust::prelude::EventHandler as _;
    let (_temp, db) = fixture();
    db.bind_account(OWN.into(), String::new()).await.unwrap();
    db.ingest(Batch {
        aliases: vec![(OWN_LID.into(), OWN.into()), (LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    let state = crate::protocol_sync::AppState::default();
    let missed = Event::MissedCall(whatsapp_rust::wacore::types::call::MissedCall::new(
        PN.parse().unwrap(),
        "same-call".into(),
        chrono::DateTime::from_timestamp(1_700_000_100, 0).unwrap(),
        whatsapp_rust::wacore::types::call::MissedReason::Remote,
    ));
    let provisional = calls::signal(&missed).unwrap();
    assert_eq!(provisional.text, "Missed call");
    db.ingest(Batch {
        messages: vec![provisional],
        ..Default::default()
    })
    .await
    .unwrap();
    let mut ringing = log("same-call", PN);
    ringing.call_result = Some(wa::call_log_record::CallResult::ONGOING);
    ringing.duration = None;
    state.handle_event(sync(ringing.clone(), PN, 100, false));
    state.flush(&db, false).await.unwrap();
    let chat = db
        .view(0, 100, String::new(), false, false)
        .await
        .unwrap()
        .chats[0]
        .id;
    let first = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
        .messages[0]
        .id;
    let mut final_log = log("same-call", PN);
    final_log.call_creator_jid = None; // The mutation index supplies this identity.
    state.handle_event(sync(final_log, PN, 200, false));
    state.flush(&db, false).await.unwrap();
    // Older history and an older app-state update cannot revert a completed call.
    db.ingest(Batch {
        messages: vec![calls::history(&ringing, &calls::account(&db).unwrap()).unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    state.handle_event(sync(ringing, PN, 150, false));
    state.flush(&db, false).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.messages.len(), 1);
    assert_eq!(view.messages[0].id, first);
    assert_eq!(view.chat.preview, "Voice call");
    assert_eq!(view.chat.call_kind, 1);
    assert_eq!(view.chat.unread, 0);
    let Some(Notice::Call(call)) = view.messages[0].content.notice else {
        panic!("missing call")
    };
    assert_eq!(call.outcome, CallOutcome::Connected);
    assert_eq!(call.details(false), "Incoming · 2:05");
    db.mark_visible_read_with_receipts(chat, first, true)
        .await
        .unwrap();
    assert!(db.next_receipt().await.unwrap().is_none());
    assert!(db.next_read_sync().await.unwrap().is_none());
    assert!(db.set_reply(chat, first).await.is_err());
    assert!(
        db.queue_operation(
            chat,
            first,
            chat,
            "invalid-call-forward".into(),
            crate::message_actions::Operation::Forward
        )
        .await
        .is_err()
    );
    assert!(
        db.phone_history_anchor(chat)
            .await
            .unwrap()
            .unwrap()
            .wire
            .is_empty()
    );
    let mut text = crate::archive::tests::record("after-call");
    text.timestamp = 1_700_000_200;
    db.ingest(Batch {
        messages: vec![text],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .chat
            .preview,
        "Synthetic after-call"
    );
    // Even a later call must not become a fake message read-range anchor.
    let mut late = log("later-call", PN);
    late.start_time = Some(1_700_000_300);
    state.handle_event(sync(late, PN, 300, false));
    state.flush(&db, false).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.chat.unread, 1);
    db.mark_visible_read_with_receipts(chat, view.messages.last().unwrap().id, true)
        .await
        .unwrap();
    assert_eq!(
        db.next_read_sync()
            .await
            .unwrap()
            .unwrap()
            .message
            .unwrap()
            .wire,
        "after-call"
    );
    let receipt = db.next_receipt().await.unwrap().unwrap();
    assert_eq!(receipt.messages.len(), 1);
    assert_eq!(receipt.messages[0].1, "after-call");
    db.draft(chat, "Synthetic draft".into()).await.unwrap();
    let list = db.view(0, 100, String::new(), false, false).await.unwrap();
    assert_eq!(list.chats[0].preview, "Draft: Synthetic draft");
    assert_eq!(list.chats[0].call_kind, 0);
    db.blocking(|c| {
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM messages WHERE delivery IN ('queued','sending','uncertain')",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn call_history_stream_replay_aliases_and_reopen_keep_one_event() {
    let temp = tempfile::tempdir().unwrap();
    let files =
        crate::storage::prepare_at(temp.path(), &crate::storage::tests::MemoryVault::default())
            .unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    db.bind_account(OWN.into(), String::new()).await.unwrap();
    let mut hs = wa::HistorySync::default();
    let mut mapping = wa::PhoneNumberToLIDMapping::default();
    mapping.pn_jid = Some(OWN.into());
    mapping.lid_jid = Some(OWN_LID.into());
    hs.phone_number_to_lid_mappings = vec![mapping];
    hs.call_log_records = vec![log("history-call", OWN), wa::CallLogRecord::default()];
    let compressed = super::tests::compressed(&hs);
    assert!(db.needs_call_snapshot().await.unwrap());
    db.finish_call_snapshot().await.unwrap();
    for id in ["calls-first", "calls-replay"] {
        db.capture_history(id.into(), compressed.clone())
            .await
            .unwrap();
        import_pending(&db, &AtomicBool::new(false)).unwrap();
    }
    let view = db.view(0, 100, String::new(), false, false).await.unwrap();
    assert_eq!(view.chats.len(), 1);
    let chat = view.chats[0].id;
    assert_eq!(view.chats[0].preview, "Voice call");
    assert!(
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .messages[0]
            .outgoing
    );
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert!(!db.needs_call_snapshot().await.unwrap());
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.messages.len(), 1);
    assert_eq!(view.chat.unread, 0);
    assert_eq!(view.messages[0].time.len(), 5);
}

#[tokio::test]
async fn call_message_decoding_and_repair_remove_old_unsupported_unread_without_losing_identity() {
    let mut message = wa::Message::default();
    let mut log = wa::message::CallLogMessage::default();
    log.is_video = Some(true);
    log.call_outcome = Some(wa::message::call_log_message::CallOutcome::MISSED);
    message.call_log_messsage = MessageField::some(log);
    let row = super::tests::mapped(&message, false);
    let wire = row.id.clone();
    assert_eq!(row.text, "Missed video call");
    let mut wrapper = wa::Message::default();
    wrapper.ephemeral_message.get_or_insert_default().message = MessageField::some(message.clone());
    assert_eq!(super::tests::mapped(&wrapper, false).content, row.content);
    let (_temp, db) = fixture();
    let mut old = row;
    old.text = "[Unsupported message]".into();
    old.content = Content::default();
    db.ingest(Batch {
        messages: vec![old],
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
    let id = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
        .messages[0]
        .id;
    repair_text(&db).unwrap();
    repair_text(&db).unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.messages[0].id, id);
    assert_eq!(view.chat.unread, 0);
    assert_eq!(view.chat.call_kind, 2);
    assert_eq!(view.chat.preview, "Missed video call");
    let mut sync_log = self::log(&wire, PN);
    sync_log.is_video = Some(true);
    sync_log.call_result = Some(wa::call_log_record::CallResult::MISSED);
    let update = calls::record(&sync_log, &PN.parse().unwrap(), &wire, false, &[], 500).unwrap();
    db.ingest(Batch {
        messages: vec![update],
        ..Default::default()
    })
    .await
    .unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.messages.len(), 1);
    assert_eq!(view.messages[0].id, id);
    db.mark_visible_read_with_receipts(chat, id, true)
        .await
        .unwrap();
    assert!(db.next_receipt().await.unwrap().is_none());
}
