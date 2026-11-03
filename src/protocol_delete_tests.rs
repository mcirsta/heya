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
async fn viewing_a_later_group_revoke_renews_an_already_confirmed_private_read() {
    let (_temp, db) = fixture();
    db.ingest(live_batch(&[incoming(
        GROUP,
        PN,
        "original",
        100,
        wa::Message::text("Synthetic already-read group message"),
    )]))
    .await
    .unwrap();
    let before = load(&db, GROUP).await;
    let observed = before.messages[0].id;
    db.mark_visible_read(before.chat.id, observed)
        .await
        .unwrap();
    let original_read = db.next_read_sync().await.unwrap().unwrap();
    db.finish_read_sync(original_read.clone()).await.unwrap();
    assert!(db.next_read_sync().await.unwrap().is_none());

    db.ingest(live_batch(&[incoming(
        GROUP,
        PN,
        "later-delete-envelope",
        200,
        revoke(GROUP, Some(PN), false),
    )]))
    .await
    .unwrap();
    let after = load(&db, GROUP).await;
    assert_eq!(after.messages.len(), 1);
    assert_eq!(after.messages[0].id, observed);
    assert_eq!(after.messages[0].text, "This message was deleted");
    assert_eq!(after.chat.unread, 0);
    assert!(
        db.next_read_sync().await.unwrap().is_none(),
        "receiving a deletion alone does not establish that it was viewed"
    );

    db.mark_visible_read_with_receipts(after.chat.id, observed, true)
        .await
        .unwrap();
    let viewed_delete = db.next_read_sync().await.unwrap().expect(
        "viewing a newer deletion must publish a new private read even when its original was read",
    );
    assert!(viewed_delete.version > original_read.version);
    assert_eq!(viewed_delete.through, original_read.through);
    assert_eq!(viewed_delete.messages.len(), 1);
    assert_eq!(viewed_delete.messages[0].wire, "later-delete-envelope");
    assert_eq!(viewed_delete.messages[0].sender, PN);
    assert!(!viewed_delete.messages[0].outgoing);
    let range = crate::protocol_reads::read_range(&viewed_delete).unwrap();
    assert_eq!(range.last_message_timestamp, Some(1_700_000_100));
    assert_eq!(range.messages[0].timestamp, Some(1_700_000_100));
    assert_eq!(
        range.messages[0].key.as_option().unwrap().id.as_deref(),
        Some("later-delete-envelope")
    );
    assert!(
        db.next_receipt().await.unwrap().is_none(),
        "private deletion observation must not send a sender-facing read receipt"
    );
    db.finish_read_sync(viewed_delete).await.unwrap();
    db.mark_visible_read(after.chat.id, observed).await.unwrap();
    assert!(
        db.next_read_sync().await.unwrap().is_none(),
        "the same observed deletion remains deduplicated"
    );
}

#[tokio::test]
async fn same_second_deleted_group_messages_publish_all_revoke_keys_without_reading_newer_rows() {
    let (_temp, db) = fixture();
    let originals = [
        incoming(
            GROUP,
            PN,
            "original",
            100,
            wa::Message::text("Synthetic first"),
        ),
        incoming(
            GROUP,
            PN,
            "second",
            100,
            wa::Message::text("Synthetic second"),
        ),
    ];
    db.ingest(live_batch(&originals)).await.unwrap();
    let before = load(&db, GROUP).await;
    let observed = before.messages[1].id;
    let first_delete = revoke(GROUP, Some(PN), false);
    let mut second_delete = first_delete.clone();
    second_delete
        .protocol_message
        .as_option_mut()
        .unwrap()
        .key
        .as_option_mut()
        .unwrap()
        .id = Some("second".into());
    db.ingest(live_batch(&[
        incoming(GROUP, PN, "delete-first", 200, first_delete.clone()),
        incoming(GROUP, PN, "delete-second", 200, second_delete),
    ]))
    .await
    .unwrap();
    // Replaying the originals and an older revoke cannot replace a newer
    // accepted deletion key or revive either original's content.
    db.ingest(live_batch(&originals)).await.unwrap();
    db.ingest(live_batch(&[incoming(
        GROUP,
        PN,
        "old-delete-first",
        199,
        first_delete,
    )]))
    .await
    .unwrap();
    let deleted = load(&db, GROUP).await;
    assert_eq!(deleted.messages.len(), 2);
    assert!(
        deleted
            .messages
            .iter()
            .all(|m| m.text == "This message was deleted")
    );
    assert_eq!(deleted.messages[1].id, observed);
    db.ingest(live_batch(&[incoming(
        GROUP,
        PN,
        "not-observed",
        100,
        wa::Message::text("Synthetic later arrival"),
    )]))
    .await
    .unwrap();
    db.mark_visible_read_with_receipts(before.chat.id, observed, true)
        .await
        .unwrap();
    let sync = db.next_read_sync().await.unwrap().unwrap();
    assert_eq!(sync.through, 1_700_000_100);
    assert_eq!(
        sync.messages
            .iter()
            .map(|m| m.wire.as_str())
            .collect::<Vec<_>>(),
        ["delete-first", "delete-second"]
    );
    let range = crate::protocol_reads::read_range(&sync).unwrap();
    assert_eq!(range.last_message_timestamp, Some(1_700_000_100));
    assert_eq!(range.messages.len(), 2);
    for (entry, wire) in range.messages.iter().zip(["delete-first", "delete-second"]) {
        assert_eq!(entry.timestamp, Some(1_700_000_100));
        let key = entry.key.as_option().unwrap();
        assert_eq!(key.id.as_deref(), Some(wire));
        assert_eq!(key.participant.as_deref(), Some(PN));
        assert_eq!(key.from_me, Some(false));
        assert_eq!(key.remote_jid.as_deref(), Some(GROUP));
    }
    assert_eq!(load(&db, GROUP).await.chat.unread, 1);
    db.read_updates(vec![crate::archive::ReadUpdate {
        jid: GROUP.into(),
        through: Some(sync.through),
        message: sync.messages.last().cloned(),
        marked: false,
        version: Some(chrono::Utc::now().timestamp_millis()),
    }])
    .await
    .unwrap();
    assert_eq!(
        load(&db, GROUP).await.chat.unread,
        1,
        "the private read echo keeps the original exact cutoff"
    );
    assert!(
        db.next_receipt().await.unwrap().is_none(),
        "deleted originals and the unobserved newer row emit no sender-facing receipts"
    );
    db.finish_read_sync(sync).await.unwrap();
    db.mark_visible_read(before.chat.id, observed)
        .await
        .unwrap();
    assert!(db.next_read_sync().await.unwrap().is_none());
}

#[tokio::test]
async fn private_delete_receipt_before_revoke_settles_the_original_without_reading_a_newer_row() {
    let (_temp, db) = fixture();
    db.ingest(live_batch(&[
        incoming(
            GROUP,
            PN,
            "original",
            100,
            wa::Message::text("Synthetic deleted later"),
        ),
        incoming(
            GROUP,
            PN,
            "newer-same-second",
            100,
            wa::Message::text("Synthetic unobserved"),
        ),
    ]))
    .await
    .unwrap();
    assert_eq!(load(&db, GROUP).await.chat.unread, 2);
    db.read_receipts(GROUP.into(), vec!["early-delete-read".into()])
        .await
        .unwrap();
    assert_eq!(
        load(&db, GROUP).await.chat.unread,
        2,
        "an unknown receipt waits for its target"
    );
    db.ingest(live_batch(&[incoming(
        GROUP,
        PN,
        "early-delete-read",
        200,
        revoke(GROUP, Some(PN), false),
    )]))
    .await
    .unwrap();
    let view = load(&db, GROUP).await;
    assert_eq!(view.messages.len(), 2);
    assert_eq!(view.messages[0].text, "This message was deleted");
    assert_eq!(
        view.chat.unread, 1,
        "the revoke receipt clears the original row, not a later same-second arrival"
    );
    assert!(
        db.next_read_sync().await.unwrap().is_none(),
        "an incoming read receipt does not queue an outgoing private update"
    );
    assert!(db.next_receipt().await.unwrap().is_none());
    db.call(|c| {
        assert_eq!(
            c.query_row("SELECT count(*) FROM pending_reads", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn an_older_revoke_after_a_newer_edit_keeps_the_actual_delete_read_key() {
    let (_temp, db) = fixture();
    db.ingest(live_batch(&[incoming(
        GROUP,
        PN,
        "original",
        100,
        wa::Message::text("Synthetic original"),
    )]))
    .await
    .unwrap();
    let mut edit = revoke(GROUP, Some(PN), false);
    let protocol = edit.protocol_message.as_option_mut().unwrap();
    protocol.r#type = Some(wa::message::protocol_message::Type::MESSAGE_EDIT);
    protocol.edited_message = MessageField::some(wa::Message::text("Synthetic edited text"));
    db.ingest(live_batch(&[incoming(
        GROUP,
        PN,
        "newer-edit",
        300,
        edit.clone(),
    )]))
    .await
    .unwrap();
    assert_eq!(
        load(&db, GROUP).await.messages[0].text,
        "Synthetic edited text"
    );
    db.ingest(live_batch(&[incoming(
        GROUP,
        PN,
        "older-delete",
        200,
        revoke(GROUP, Some(PN), false),
    )]))
    .await
    .unwrap();
    db.ingest(live_batch(&[incoming(GROUP, PN, "late-edit", 400, edit)]))
        .await
        .unwrap();
    let view = load(&db, GROUP).await;
    assert_eq!(view.messages.len(), 1);
    assert_eq!(view.messages[0].text, "This message was deleted");
    db.mark_visible_read_with_receipts(view.chat.id, view.messages[0].id, true)
        .await
        .unwrap();
    let sync = db.next_read_sync().await.unwrap().unwrap();
    assert_eq!(sync.through, 1_700_000_100);
    assert_eq!(sync.messages.len(), 1);
    assert_eq!(sync.messages[0].wire, "older-delete");
    assert!(db.next_receipt().await.unwrap().is_none());
    db.call(|c| {
        let version = c.query_row(
            "SELECT version FROM mutations WHERE wire_id='original'",
            [],
            |r| r.get::<_, i64>(0),
        )?;
        assert_eq!(
            version, 1_700_000_200_000,
            "a winning deletion retains its own version after late edits"
        );
        Ok(())
    })
    .await
    .unwrap();
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
