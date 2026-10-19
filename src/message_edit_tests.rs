//! Synthetic typed wire edits through ingestion, encrypted storage and actions.
use super::*;
use crate::archive::tests::{KEY, LID, PN, fixture};
use whatsapp_rust::{prelude::MessageBuilderExt, waproto::buffa::MessageField};

fn media(document: bool, caption: Option<&str>, partial: bool) -> wa::Message {
    let mut message = wa::Message::default();
    if document {
        let mut value = wa::message::DocumentMessage::default();
        value.caption = caption.map(str::to_owned);
        if !partial {
            value.file_name = Some("Synthetic_こんにちは.pdf".into());
            value.mimetype = Some("application/pdf".into());
            value.page_count = Some(3);
            value.file_length = Some(12345);
            value.media_key = Some(vec![17; 32]);
            value.file_sha256 = Some(vec![18; 32]);
            value.jpeg_thumbnail = Some(vec![1, 2, 3]);
        }
        message.document_message = MessageField::some(value);
    } else {
        let mut value = wa::message::ImageMessage::default();
        value.caption = caption.map(str::to_owned);
        if !partial {
            value.width = Some(1600);
            value.scan_lengths = vec![1234, 5678];
            value.height = Some(1200);
            value.file_length = Some(12345);
            value.media_key = Some(vec![17; 32]);
            value.file_sha256 = Some(vec![18; 32]);
            value.jpeg_thumbnail = Some(vec![1, 2, 3]);
        }
        message.image_message = MessageField::some(value);
    }
    message
}
fn base(id: &str, message: &wa::Message) -> Record {
    record(
        &PN.parse().unwrap(),
        &PN.parse().unwrap(),
        id.into(),
        1_700_000_000,
        false,
        "Synthetic".into(),
        message,
        None,
    )
}
fn edit(target: &str, message: &wa::Message, version: i64) -> Record {
    let mut protocol = wa::message::ProtocolMessage::default();
    let mut key = wa::MessageKey::default();
    key.remote_jid = Some(PN.into());
    key.id = Some(target.into());
    protocol.key = MessageField::some(key);
    protocol.r#type = Some(wa::message::protocol_message::Type::MESSAGE_EDIT);
    protocol.timestamp_ms = Some(version);
    protocol.edited_message = MessageField::some(message.clone());
    let mut wrapper = wa::Message::default();
    wrapper.protocol_message = MessageField::some(protocol);
    base(&format!("edit-{target}-{version}"), &wrapper)
}
async fn ingest(db: &Archive, messages: Vec<Record>) {
    db.ingest(Batch {
        messages,
        ..Default::default()
    })
    .await
    .unwrap();
}
async fn load(db: &Archive) -> crate::domain::Conversation {
    let chat = db.create_chat(PN.into()).await.unwrap();
    db.view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
}

#[tokio::test]
async fn typed_caption_edits_keep_cards_cache_and_inflight_references_and_forward_current_caption()
{
    for document in [false, true] {
        let (_temp, db) = fixture();
        db.bind_account("15550000009@s.whatsapp.net".into(), "Owner".into())
            .await
            .unwrap();
        let original = base(
            "original",
            &media(document, Some("Original caption"), false),
        );
        let raw = original.raw.clone();
        ingest(&db, vec![original.clone()]).await;
        let before = load(&db).await.messages[0].clone();
        db.blocking(|c| {
            if document {
                c.execute("INSERT INTO document_previews(message_id,preview,sharp) VALUES(?1,X'030201',1)",[before.id])?;
            } else {
                c.execute("INSERT INTO message_photos(message_id,original,preview,state,revision) VALUES(?1,X'030201',X'030201',2,7)",[before.id])?;
            }
            Ok(())
        }).unwrap();
        let caption = "Edited 👩🏽‍💻\n\nA full, multiline caption こんにちは";
        let change = edit("original", &media(document, Some(caption), true), 20);
        assert!(change.raw.is_empty());
        assert!(!change.change.as_ref().unwrap().raw.is_empty());
        ingest(
            &db,
            vec![
                change.clone(),
                original,
                edit("original", &media(document, Some("Stale"), true), 10),
                change,
                edit("original", &media(document, Some(caption), false), 21),
            ],
        )
        .await;
        let after = load(&db).await.messages[0].clone();
        assert_eq!(after.id, before.id);
        assert_eq!(after.time, before.time);
        assert_eq!(after.content.body.as_deref(), Some(caption));
        assert_eq!(after.content.photo, before.content.photo);
        assert_eq!(after.content.document, before.content.document);
        db.blocking(|c| {
            let current: Vec<u8> =
                c.query_row("SELECT raw FROM messages WHERE id=?1", [before.id], |r| {
                    r.get(0)
                })?;
            assert_eq!(
                current, raw,
                "caption changes keep the transfer request valid"
            );
            let cached: i64 = c.query_row(
                if document {
                    "SELECT length(preview) FROM document_previews WHERE message_id=?1"
                } else {
                    "SELECT revision FROM message_photos WHERE message_id=?1"
                },
                [before.id],
                |r| r.get(0),
            )?;
            assert_eq!(cached, if document { 3 } else { 7 });
            Ok(())
        })
        .unwrap();
        let chat = load(&db).await.chat.id;
        db.queue_operation(
            chat,
            after.id,
            chat,
            "synthetic-forward".into(),
            crate::message_actions::Operation::Forward,
        )
        .await
        .unwrap();
        let forwarded =
            codec::message_decode(&db.claim_next().await.unwrap().unwrap().raw).unwrap();
        assert_eq!(
            crate::protocol_content::extract(&forwarded)
                .1
                .body
                .as_deref(),
            Some(caption)
        );
    }
}

#[tokio::test]
async fn caption_edits_before_base_survive_alias_merge_and_reopen_and_allow_removal() {
    use crate::storage::{prepare_at, tests::MemoryVault};
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    let mut change = edit("original", &media(false, Some("New caption"), true), 20);
    change.chat = LID.into();
    change.sender = LID.into();
    change.change.as_mut().unwrap().sender = LID.into();
    ingest(&db, vec![change]).await;
    ingest(
        &db,
        vec![base("original", &media(false, Some("Old"), false))],
    )
    .await;
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    let before = load(&db).await.messages[0].clone();
    assert_eq!(before.content.body.as_deref(), Some("New caption"));
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert_eq!(load(&db).await.messages[0], before);
    ingest(&db, vec![edit("original", &media(false, None, true), 30)]).await;
    let after = load(&db).await.messages[0].clone();
    assert_eq!(after.content.body.as_deref(), Some(""));
    assert_eq!(after.content.photo, before.content.photo);
}

#[tokio::test]
async fn media_replacement_invalidates_cache_but_revoke_expiry_and_hide_never_restore_edits() {
    let (_temp, db) = fixture();
    let original = base("original", &media(false, Some("Old"), false));
    ingest(&db, vec![original.clone()]).await;
    let id = load(&db).await.messages[0].id;
    db.blocking(|c|{c.execute("INSERT INTO message_photos(message_id,original,preview,state) VALUES(?1,X'01',X'01',2)",[id])?;Ok(())}).unwrap();
    let mut replacement = media(false, Some("Replacement"), false);
    replacement.image_message.get_or_insert_default().media_key = Some(vec![42; 32]);
    ingest(&db, vec![edit("original", &replacement, 10)]).await;
    db.blocking(|c| {
        assert_eq!(
            c.query_row("SELECT count(*) FROM message_photos", [], |r| r
                .get::<_, i32>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
    // Expiry cannot be undone by a later valid edit or base replay.
    db.blocking(|c| {
        c.execute("UPDATE messages SET expires=1 WHERE id=?1", [id])?;
        Ok(())
    })
    .unwrap();
    ingest(
        &db,
        vec![
            edit(
                "original",
                &media(false, Some("Must not restore"), true),
                30,
            ),
            original,
        ],
    )
    .await;
    let view = load(&db).await;
    assert!(view.messages[0].content.photo.is_none());
    db.blocking(|c| {
        assert_eq!(
            c.query_row("SELECT length(raw) FROM mutations", [], |r| r
                .get::<_, i32>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
    // A persisted local hide also discards a newly arriving edit envelope.
    let hidden = base("hidden", &media(true, Some("Private caption"), false));
    ingest(&db, vec![hidden]).await;
    db.blocking(|c|{c.execute("INSERT INTO message_hides(chat_id,sender,wire_id) SELECT chat_id,sender,wire_id FROM messages WHERE wire_id='hidden'",[])?;Ok(())}).unwrap();
    ingest(
        &db,
        vec![edit("hidden", &media(true, Some("Hidden edit"), true), 40)],
    )
    .await;
    db.blocking(|c| {
        let value: (i32, Option<String>) = c.query_row(
            "SELECT length(raw),body FROM mutations WHERE wire_id='hidden'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(value, (0, None));
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn migration_recovers_legacy_edited_media_without_claiming_the_lost_caption() {
    use crate::storage::{prepare_at, tests::MemoryVault};
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    ingest(
        &db,
        vec![base("legacy", &media(true, Some("Stale original"), false))],
    )
    .await;
    db.blocking(|c|{
        c.execute_batch("ALTER TABLE mutations DROP COLUMN raw; DROP TABLE history_pages; DROP TABLE history_requests; ALTER TABLE messages DROP COLUMN edit_version; DROP TABLE outgoing_receipts; PRAGMA user_version=12;")?;
        c.execute("INSERT INTO mutations(chat_id,sender,wire_id,body,version,deleted) SELECT chat_id,sender,wire_id,'[Unsupported edited message]',20,0 FROM messages WHERE wire_id='legacy'",[])?;
        c.execute("UPDATE messages SET body='[Unsupported edited message]',content='',thumbnail=X'' WHERE wire_id='legacy'",[])?;
        Ok(())
    }).unwrap();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    let view = load(&db).await;
    assert!(view.messages[0].content.document.is_some());
    assert_eq!(view.messages[0].content.body.as_deref(), Some(""));
    assert!(!view.transcript.contains("Stale original"));
}

#[test]
fn typed_edits_reject_other_authors_view_once_and_mismatched_attachments() {
    let original = media(false, Some("Old"), false);
    let mut envelope = wa::Message::default();
    let mut wrapper = wa::message::FutureProofMessage::default();
    wrapper.message = MessageField::some(original.clone());
    envelope.view_once_message = MessageField::some(wrapper);
    assert!(edit("original", &envelope, 30).change.is_none());
    assert!(
        crate::protocol_edits::apply(
            &codec::message_to_vec(&original),
            &codec::message_to_vec(&wa::Message::text("Not a photo")),
            ""
        )
        .is_none()
    );
    // Target a different group participant: the record's author cannot edit it.
    let mut protocol = wa::message::ProtocolMessage::default();
    let mut key = wa::MessageKey::default();
    key.id = Some("original".into());
    key.participant = Some("15550000002@s.whatsapp.net".into());
    protocol.key = MessageField::some(key);
    protocol.r#type = Some(wa::message::protocol_message::Type::MESSAGE_EDIT);
    protocol.edited_message = MessageField::some(original);
    let mut message = wa::Message::default();
    message.protocol_message = MessageField::some(protocol);
    assert!(base("wrong-author", &message).change.is_none());
}
