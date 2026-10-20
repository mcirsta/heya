//! Synthetic pin/unpin envelopes only; no live-account payloads.
use super::*;
use crate::{
    archive::tests::{PN, fixture},
    message_content::{Content, Notice},
};
use whatsapp_rust::{prelude::MessageBuilderExt, waproto::buffa::MessageField};

fn pin(kind: wa::message::pin_in_chat_message::Type) -> wa::Message {
    let mut key = wa::MessageKey::default();
    key.id = Some("synthetic-target".into());
    let mut pin = wa::message::PinInChatMessage::default();
    pin.key = MessageField::some(key);
    pin.r#type = Some(kind);
    let mut message = wa::Message::default();
    message.pin_in_chat_message = MessageField::some(pin);
    message
}

#[test]
fn pin_notices_decode_known_types_wrappers_and_keep_unknowns_and_view_once_safe() {
    use wa::message::pin_in_chat_message::Type;
    for (kind, notice) in [
        (Type::PIN_FOR_ALL, Notice::MessagePinned),
        (Type::UNPIN_FOR_ALL, Notice::MessageUnpinned),
    ] {
        let message = pin(kind);
        let row = super::tests::mapped(&message, false);
        assert!(row.visible && row.change.is_none());
        assert_eq!(row.text, notice.description());
        assert_eq!(row.content.notice, Some(notice));
        assert_eq!(row.raw, codec::message_to_vec(&message));
        assert!(row.thumbnail.is_empty());
        let mut wrapper = wa::message::FutureProofMessage::default();
        wrapper.message = MessageField::some(message);
        let mut wrapped = wa::Message::default();
        wrapped.ephemeral_message = MessageField::some(wrapper.clone());
        assert_eq!(super::tests::mapped(&wrapped, true).content, row.content);
        wrapped.ephemeral_message = Default::default();
        wrapped.view_once_message = MessageField::some(wrapper);
        let once = super::tests::mapped(&wrapped, false);
        assert!(once.content.notice.is_none() && once.raw.is_empty());
    }
    let mut unknown = pin(Type::UNKNOWN_TYPE);
    assert!(
        super::tests::mapped(&unknown, false)
            .content
            .notice
            .is_none()
    );
    unknown.pin_in_chat_message.get_or_insert_default().r#type = None;
    assert!(
        super::tests::mapped(&unknown, false)
            .content
            .notice
            .is_none()
    );
    let literal = wa::Message::text("pinned a message");
    assert!(
        super::tests::mapped(&literal, false)
            .content
            .notice
            .is_none()
    );
    // Adding an absent optional field must not change existing media cache keys.
    assert!(
        !serde_json::to_string(&Content::default())
            .unwrap()
            .contains("notice")
    );
}

#[tokio::test]
async fn live_and_history_pin_notices_agree_and_cannot_be_sent_as_messages() {
    use wa::message::pin_in_chat_message::Type;
    use whatsapp_rust::wacore::types::message::{MessageInfo, MessageSource};
    let (_temp, db) = fixture();
    let group = "15550000000-123@g.us";
    let info = MessageInfo {
        source: MessageSource {
            chat: group.parse().unwrap(),
            sender: PN.parse().unwrap(),
            is_group: true,
            ..Default::default()
        },
        id: "live-pin".into(),
        push_name: "Synthetic Sender".into(),
        timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        ..Default::default()
    };
    let inbound = InboundMessage::builder()
        .message(Arc::new(pin(Type::PIN_FOR_ALL)))
        .info(Arc::new(info))
        .build();
    let batch = live_batch(&[inbound]);
    assert_eq!(
        batch.messages[0].content.notice,
        Some(Notice::MessagePinned)
    );
    db.ingest(batch).await.unwrap();

    let mut key = wa::MessageKey::default();
    key.remote_jid = Some(group.into());
    key.participant = Some(PN.into());
    key.id = Some("history-unpin".into());
    key.from_me = Some(false);
    let mut message = wa::WebMessageInfo::default();
    message.key = MessageField::some(key);
    message.message = MessageField::some(pin(Type::UNPIN_FOR_ALL));
    message.message_timestamp = Some(1_700_000_001);
    message.push_name = Some("Synthetic Sender".into());
    let mut item = wa::HistorySyncMsg::default();
    item.message = MessageField::some(message);
    let mut conversation = wa::Conversation::default();
    conversation.id = group.into();
    conversation.messages = vec![item];
    let mut history = wa::HistorySync::default();
    history.conversations = vec![conversation];
    db.capture_history("pin-history".into(), super::tests::compressed(&history))
        .await
        .unwrap();
    for _ in 0..2 {
        import_pending(&db, &AtomicBool::new(false)).unwrap();
    }
    let view = db.view(0, 100, String::new(), false, false).await.unwrap();
    let chat = view.chats[0].id;
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.messages.len(), 2);
    assert_eq!(view.messages[0].content.notice, Some(Notice::MessagePinned));
    assert_eq!(
        view.messages[1].content.notice,
        Some(Notice::MessageUnpinned)
    );
    assert!(view.chat.preview.ends_with(" unpinned a message"));
    assert!(!view.chat.preview.contains(':'));
    assert!(
        view.transcript
            .contains("Synthetic Sender pinned a message")
    );
    for m in view.messages.iter() {
        assert!(db.set_reply(chat, m.id).await.is_err());
        for operation in [
            crate::message_actions::Operation::React("❤".into()),
            crate::message_actions::Operation::Forward,
        ] {
            assert!(
                db.queue_operation(chat, m.id, chat, "blocked-notice-action".into(), operation)
                    .await
                    .is_err()
            );
        }
    }
}

#[tokio::test]
async fn saved_pin_notices_repair_in_batches_without_changing_identity_or_removed_content() {
    use wa::message::pin_in_chat_message::Type;
    let (_temp, db) = fixture();
    let old = |id: &str| {
        let mut r = super::tests::mapped(&pin(Type::PIN_FOR_ALL), false);
        r.id = id.into();
        r.text = "[Unsupported message]".into();
        r.content = Content::default();
        r
    };
    let mut rows = (0..205)
        .map(|i| old(&format!("old-pin-{i}")))
        .collect::<Vec<_>>();
    rows.push(old("deleted"));
    rows.push(old("edited"));
    let mut expired = old("expired");
    expired.expires = Some(1);
    rows.push(expired);
    let mut malformed = old("malformed");
    malformed.raw = vec![0xff];
    rows.push(malformed);
    let mut literal = old("literal");
    literal.raw = codec::message_to_vec(&wa::Message::text("[Unsupported message]"));
    rows.push(literal);
    for (target, text) in [("deleted", None), ("edited", Some("Edited text".into()))] {
        let mut mutation = old(&format!("mutation-{target}"));
        mutation.visible = false;
        mutation.raw.clear();
        mutation.change = Some(Change {
            id: target.into(),
            sender: PN.into(),
            text,
            version: 12,
            raw: Vec::new(),
        });
        rows.push(mutation);
    }
    db.ingest(Batch {
        messages: rows,
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    db.draft(chat, "Keep this draft".into()).await.unwrap();
    db.blocking(|c| {
        c.execute(
            "INSERT OR REPLACE INTO metadata(key,value) VALUES('content_decoder_revision','7')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let state = || {
        db.blocking(|c| {
            let mut query = c.prepare(
                "SELECT id,wire_id,sender,stamp,raw,unread,seen FROM messages ORDER BY id",
            )?;
            Ok(query
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, Vec<u8>>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .unwrap()
    };
    let before = state();
    repair_text(&db).unwrap();
    assert_eq!(before, state());
    let view = db
        .view(chat, 300, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.draft, "Keep this draft");
    assert_eq!(
        view.messages
            .iter()
            .filter(|m| m.content.notice.is_some())
            .count(),
        205
    );
    assert_eq!(
        view.messages
            .iter()
            .filter(|m| m.text == "[Unsupported message]")
            .count(),
        2
    );
    assert!(
        view.messages
            .iter()
            .any(|m| m.text == "Edited text" && m.content.notice.is_none())
    );
    assert!(
        view.messages
            .iter()
            .any(|m| m.text == "This message was deleted" && m.content.notice.is_none())
    );
    assert!(
        view.messages
            .iter()
            .any(|m| m.text == "[Disappearing message expired]" && m.content.notice.is_none())
    );
    db.repair_message_content(|_| panic!("completed upgrade must not rescan"))
        .unwrap();
}
