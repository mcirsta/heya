//! Sender-relative deletes through the real adapter and encrypted archive.
use super::*;
use crate::archive::tests::{KEY, LID, PN, fixture};
use whatsapp_rust::{
    prelude::MessageBuilderExt, wacore::types::message::MessageInfo, waproto::buffa::MessageField,
};
const OWNER: &str = "15550000009@s.whatsapp.net";
const GROUP: &str = "15550000001-123456789@g.us";

fn revoke(remote: &str, participant: Option<&str>, from_me: bool) -> wa::Message {
    let mut key = wa::MessageKey::default();
    key.id = Some("original".into());
    key.remote_jid = Some(remote.into());
    key.from_me = Some(from_me);
    key.participant = participant.map(str::to_owned);
    let mut protocol = wa::message::ProtocolMessage::default();
    protocol.key = MessageField::some(key);
    protocol.r#type = Some(wa::message::protocol_message::Type::REVOKE);
    let mut message = wa::Message::default();
    message.protocol_message = MessageField::some(protocol);
    message
}
fn incoming(
    chat: &str,
    sender: &str,
    id: &str,
    stamp: i64,
    message: wa::Message,
) -> InboundMessage {
    let mut info = MessageInfo::default();
    info.source.chat = chat.parse().unwrap();
    info.source.sender = sender.parse().unwrap();
    info.source.is_group = chat == GROUP;
    info.id = id.into();
    info.push_name = "Synthetic sender".into();
    info.timestamp = chrono::DateTime::from_timestamp(1_700_000_000 + stamp, 0).unwrap();
    InboundMessage::builder()
        .message(Arc::new(message))
        .info(Arc::new(info))
        .build()
}
async fn load(db: &Archive, jid: &str) -> crate::domain::Conversation {
    let chat = db.create_chat(jid.into()).await.unwrap();
    db.view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
}

#[tokio::test]
async fn recipient_relative_delete_updates_the_existing_bubble_and_sidebar() {
    let (_temp, db) = fixture();
    db.ingest(live_batch(&[incoming(
        PN,
        PN,
        "original",
        100,
        wa::Message::text("Synthetic original"),
    )]))
    .await
    .unwrap();
    let before = load(&db, PN).await;
    let mut edited = revoke(OWNER, None, true);
    let protocol = edited.protocol_message.as_option_mut().unwrap();
    protocol.r#type = Some(wa::message::protocol_message::Type::MESSAGE_EDIT);
    protocol.edited_message = MessageField::some(wa::Message::text("Synthetic edited text"));
    db.ingest(live_batch(&[incoming(
        PN,
        PN,
        "edit-envelope",
        150,
        edited,
    )]))
    .await
    .unwrap();
    assert_eq!(
        load(&db, PN).await.messages[0].text,
        "Synthetic edited text"
    );
    let mut wrapped = wa::Message::default();
    let mut future = wa::message::FutureProofMessage::default();
    future.message = MessageField::some(revoke(OWNER, None, true));
    wrapped.ephemeral_message = MessageField::some(future);
    db.ingest(live_batch(&[incoming(
        PN,
        PN,
        "delete-envelope",
        200,
        wrapped,
    )]))
    .await
    .unwrap();
    let after = load(&db, PN).await;
    assert_eq!(after.messages.len(), 1);
    assert_eq!(after.messages[0].id, before.messages[0].id);
    assert_eq!(after.messages[0].text, "This message was deleted");
    assert_eq!(after.chat.preview, "This message was deleted");
    assert!(after.messages[0].content.unavailable);
    db.mark_visible_read(after.chat.id, after.messages[0].id)
        .await
        .unwrap();
    assert_eq!(load(&db, PN).await.chat.unread, 0);
}

#[tokio::test]
async fn early_delete_is_visible_once_and_late_history_only_restores_original_metadata() {
    use crate::storage::{prepare_at, tests::MemoryVault};
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    let deletion = incoming(PN, PN, "delete-envelope", 200, revoke(OWNER, None, true));
    db.ingest(live_batch(std::slice::from_ref(&deletion)))
        .await
        .unwrap();
    let before = load(&db, PN).await;
    assert_eq!(before.messages.len(), 1);
    assert_eq!(before.chat.preview, "This message was deleted");
    assert_eq!(before.chat.unread, 0);
    assert!(!before.chat.time.is_empty());
    db.ingest(live_batch(&[deletion])).await.unwrap();
    let mut original = wa::Message::default();
    let mut photo = wa::message::ImageMessage::default();
    photo.caption = Some("Synthetic private caption".into());
    photo.jpeg_thumbnail = Some(vec![1, 2, 3]);
    original.image_message = MessageField::some(photo);
    db.ingest(live_batch(&[incoming(PN, PN, "original", 100, original)]))
        .await
        .unwrap();
    let after = load(&db, PN).await;
    assert_eq!(after.messages.len(), 1);
    assert_eq!(after.messages[0].id, before.messages[0].id);
    assert_eq!(after.chat.preview, "This message was deleted");
    assert_eq!(after.chat.unread, 0);
    let id = after.messages[0].id;
    db.call(move |c| {
        let row: (i64, i64, i64, i64) = c.query_row(
            "SELECT stamp,length(raw),length(content),length(thumbnail) FROM messages WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        assert_eq!(row, (1_700_000_100, 0, 0, 0));
        Ok(())
    })
    .await
    .unwrap();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert_eq!(
        load(&db, PN).await.messages[0].text,
        "This message was deleted"
    );
}

#[tokio::test]
async fn group_mutation_ownership_uses_canonical_identities_and_never_a_different_author_or_chat() {
    let (_temp, db) = fixture();
    db.ingest(live_batch(&[incoming(
        GROUP,
        LID,
        "original",
        100,
        wa::Message::text("Synthetic group text"),
    )]))
    .await
    .unwrap();
    let mut batch = live_batch(&[incoming(
        GROUP,
        LID,
        "delete-envelope",
        200,
        revoke(GROUP, Some(PN), false),
    )]);
    batch.aliases.push((LID.into(), PN.into()));
    db.ingest(batch).await.unwrap();
    assert_eq!(
        load(&db, GROUP).await.messages[0].text,
        "This message was deleted"
    );
    for (remote, participant) in [
        (GROUP, Some(OWNER)),
        ("15550000002-123456789@g.us", Some(PN)),
    ] {
        let (_temp, db) = fixture();
        db.ingest(live_batch(&[incoming(
            GROUP,
            PN,
            "original",
            100,
            wa::Message::text("Keep synthetic text"),
        )]))
        .await
        .unwrap();
        db.ingest(live_batch(&[incoming(
            GROUP,
            PN,
            "invalid-delete",
            200,
            revoke(remote, participant, false),
        )]))
        .await
        .unwrap();
        let after = load(&db, GROUP).await;
        assert_eq!(after.messages.len(), 1);
        assert_eq!(after.messages[0].text, "Keep synthetic text");
    }
}

#[tokio::test]
async fn startup_repairs_saved_rejected_deletes_and_accepted_orphans_without_reviving_hidden_messages()
 {
    let (_temp, db) = fixture();
    db.ingest(live_batch(&[incoming(
        PN,
        PN,
        "original",
        100,
        wa::Message::text("Synthetic legacy original"),
    )]))
    .await
    .unwrap();
    let packet = incoming(
        PN,
        PN,
        "legacy-rejected-delete",
        200,
        revoke(OWNER, None, true),
    );
    let mut batch = live_batch(std::slice::from_ref(&packet));
    // Model the exact old rejection: the encrypted envelope was retained but
    // no mutation was produced, because remote_jid named the recipient.
    batch.messages[0].change = None;
    batch.messages[0].raw = codec::message_to_vec(&packet.message);
    db.ingest(batch).await.unwrap();
    let chat = load(&db, PN).await.chat.id;
    db.hide_remote(PN.into(), PN.into(), "hidden-original".into(), false)
        .await
        .unwrap();
    db.call(move|c| {
        for wire in ["orphan-original","hidden-original"] {
            c.execute("INSERT INTO mutations(chat_id,sender,wire_id,body,version,deleted) VALUES(?1,?2,?3,NULL,300,1)",rusqlite::params![chat,PN,wire])?;
        }
        Ok(())
    }).await.unwrap();
    assert_eq!(load(&db, PN).await.messages.len(), 1);
    repair_text(&db).unwrap();
    let after = load(&db, PN).await;
    assert_eq!(after.messages.len(), 2);
    assert!(
        after
            .messages
            .iter()
            .all(|m| m.text == "This message was deleted")
    );
    assert_eq!(after.chat.preview, "This message was deleted");
    db.call(move |c| {
        assert_eq!(
            c.query_row(
                "SELECT length(raw) FROM messages WHERE wire_id='legacy-rejected-delete'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        c.execute(
            "DELETE FROM metadata WHERE key='delete_decoder_revision'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    repair_text(&db).unwrap();
    assert_eq!(load(&db, PN).await.messages.len(), 2);
}
