use super::*;
use crate::{
    archive::tests::{LID, PN, fixture},
    message_content::{Content, Target},
};
use whatsapp_rust::waproto::buffa::MessageField;

fn preview(bytes: Vec<u8>) -> wa::Message {
    let mut extended = wa::message::ExtendedTextMessage::default();
    extended.text = Some("https://example.invalid/community/news".into());
    extended.matched_text = Some("https://example.invalid/community/news".into());
    extended.title = Some("Community news 👩🏽‍💻".into());
    extended.description = Some("A synthetic preview\nwith two lines".into());
    extended.jpeg_thumbnail = Some(bytes);
    let mut message = wa::Message::default();
    message.extended_text_message = MessageField::some(extended);
    message
}

fn thumbnail() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(160, 90, image::Rgba([40, 120, 180, 255]))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn document(bytes: Vec<u8>) -> wa::Message {
    let mut doc = wa::message::DocumentMessage::default();
    doc.file_name = Some("Project_こんにちは_signed.pdf".into());
    doc.mimetype = Some("application/pdf".into());
    doc.file_length = Some(428000);
    doc.page_count = Some(2);
    doc.caption = Some("Complete caption 👩🏽‍💻\nSecond line".into());
    doc.jpeg_thumbnail = Some(bytes);
    let mut message = wa::Message::default();
    message.document_message = MessageField::some(doc);
    message
}

#[test]
fn documents_keep_supplied_details_and_full_caption_with_bounded_thumbnails() {
    let message = document(thumbnail());
    let mapped = super::tests::mapped(&message, false);
    let doc = mapped.content.document.as_ref().unwrap();
    assert_eq!(doc.details(), "2 pages • PDF • 428 kB");
    assert!(doc.has_thumbnail && mapped.content.has_attachment);
    assert_eq!(
        mapped.text,
        "[Document] Project_こんにちは_signed.pdf\nComplete caption 👩🏽‍💻\nSecond line"
    );
    assert_eq!(mapped.thumbnail, thumbnail());
    let mut wrapper = wa::message::FutureProofMessage::default();
    wrapper.message = MessageField::some(message.clone());
    let mut wrapped = wa::Message::default();
    wrapped.document_with_caption_message = MessageField::some(wrapper.clone());
    assert_eq!(
        super::tests::mapped(&wrapped, false).content,
        mapped.content
    );
    wrapped.document_with_caption_message = Default::default();
    wrapped.view_once_message = MessageField::some(wrapper);
    let once = super::tests::mapped(&wrapped, false);
    assert!(once.content.document.is_none() && once.thumbnail.is_empty() && once.raw.is_empty());
    let mut missing = document(vec![0; crate::message_thumbnails::MAX_ENCODED + 1]);
    let d = missing.document_message.get_or_insert_default();
    d.file_name = Some(" \n".into());
    d.title = Some("Worksheet.xlsx".into());
    d.mimetype = None;
    d.file_length = None;
    d.page_count = Some(0);
    let mapped = super::tests::mapped(&missing, false);
    let doc = mapped.content.document.unwrap();
    assert!(!doc.has_thumbnail && mapped.thumbnail.is_empty());
    assert_eq!(doc.file_name, "Worksheet.xlsx");
    assert_eq!(doc.details(), "XLSX");
    let mut doc = doc;
    doc.file_name = "No extension".into();
    doc.pages = Some(1);
    doc.size = Some(0);
    assert_eq!(doc.details(), "1 page • File • 0 B");
}

#[test]
fn document_thumbnail_decode_preserves_top_and_aspect_and_limits_memory() {
    use crate::message_thumbnails::{EDGE, MAX_ENCODED, decode};
    let mut image = crate::fixtures::document_thumbnail();
    for y in 0..8 {
        for x in 0..8 {
            image.put_pixel(x, y, image::Rgba([210, 0, 0, 255]));
        }
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let pixels = decode(bytes.get_ref()).unwrap();
    assert_eq!((pixels.width, pixels.height), (360, 480));
    assert_eq!(&pixels.rgba[..4], &[210, 0, 0, 255]);
    assert!(pixels.rgba.len() <= (EDGE * EDGE * 4) as usize);
    assert!(decode(&[]).is_none());
    assert!(decode(b"not an image").is_none());
    assert!(decode(&vec![0; MAX_ENCODED + 1]).is_none());
    let mut oversized = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(2049, 1)
        .write_to(&mut oversized, image::ImageFormat::Png)
        .unwrap();
    assert!(decode(oversized.get_ref()).is_none());
}

#[tokio::test]
async fn saved_documents_upgrade_once_and_keep_caption_without_resurrecting_deleted_content() {
    let (_temp, db) = fixture();
    let full = super::tests::mapped(&document(thumbnail()), false);
    let mut old = full.clone();
    old.id = "document".into();
    old.content = Content {
        has_attachment: true,
        ..Default::default()
    };
    old.text = "[Document] Project_こんにちは_signed.pdf".into();
    old.thumbnail.clear();
    let mut deleted = old.clone();
    deleted.id = "deleted-document".into();
    let mut expired = old.clone();
    expired.id = "expired-document".into();
    expired.expires = Some(1);
    db.ingest(Batch {
        messages: vec![old, deleted, expired],
        ..Default::default()
    })
    .await
    .unwrap();
    let mut change = crate::archive::tests::record("revoke-document");
    change.visible = false;
    change.change = Some(Change {
        id: "deleted-document".into(),
        sender: PN.into(),
        text: None,
        version: 1,
        raw: Vec::new(),
    });
    db.ingest(Batch {
        messages: vec![change],
        ..Default::default()
    })
    .await
    .unwrap();
    db.blocking(|c| {
        c.execute(
            "INSERT OR REPLACE INTO metadata VALUES('content_decoder_revision','5')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    repair_text(&db).unwrap();
    db.repair_message_content(|_| panic!("must not repeat completed upgrade"))
        .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    let m = view
        .messages
        .iter()
        .find(|m| m.content.document.is_some())
        .unwrap();
    crate::archive::tests::assert_content_payload(&m.content, &full.content);
    assert!(m.transcript_line().ends_with(&full.text));
    assert_eq!(
        view.messages
            .iter()
            .filter(|m| m.content.document.is_some())
            .count(),
        1
    );
    db.blocking(|c| {
        let bytes: Vec<u8> = c.query_row("SELECT thumbnail FROM messages WHERE wire_id='document'", [], |r|r.get(0))?;
        assert_eq!(bytes, thumbnail());
        let n: i64 = c.query_row("SELECT sum(length(content)+length(thumbnail)+length(raw)) FROM messages WHERE wire_id IN ('deleted-document','expired-document')", [], |r|r.get(0))?;
        assert_eq!(n, 0);
        Ok(())
    }).unwrap();
}

#[test]
fn protocol_cards_preserve_metadata_but_never_open_or_fetch_content() {
    let bytes = thumbnail();
    let message = preview(bytes.clone());
    let mapped = super::tests::mapped(
        &codec::message_decode(&codec::message_to_vec(&message)).unwrap(),
        false,
    );
    let card = mapped.content.preview.as_ref().unwrap();
    assert_eq!(card.title, "Community news 👩🏽‍💻");
    assert_eq!(card.description, "A synthetic preview with two lines");
    assert!(card.has_thumbnail);
    assert_eq!(mapped.thumbnail, bytes);
    assert_eq!(mapped.text, "https://example.invalid/community/news");
    let mut wrapper = wa::message::FutureProofMessage::default();
    wrapper.message = MessageField::some(message.clone());
    let mut wrapped = wa::Message::default();
    wrapped.ephemeral_message = MessageField::some(wrapper.clone());
    assert_eq!(
        super::tests::mapped(&wrapped, false).content,
        mapped.content
    );
    wrapped.ephemeral_message = Default::default();
    wrapped.view_once_message = MessageField::some(wrapper);
    let once = super::tests::mapped(&wrapped, false);
    assert_eq!(once.content, Content::default());
    assert!(once.thumbnail.is_empty() && once.raw.is_empty());

    let oversized = super::tests::mapped(
        &preview(vec![0; crate::message_thumbnails::MAX_ENCODED + 1]),
        false,
    );
    assert!(!oversized.content.preview.unwrap().has_thumbnail);
    assert!(oversized.thumbnail.is_empty());
    let mut unsafe_message = message.clone();
    let extended = unsafe_message.extended_text_message.get_or_insert_default();
    extended.matched_text = Some("javascript:alert(1)".into());
    extended.text = Some("No website".into());
    assert!(
        super::tests::mapped(&unsafe_message, false)
            .content
            .preview
            .is_none()
    );
    let business = crate::protocol_business::tests::template("A delivery\nKeep my full body");
    let mapped = super::tests::mapped(&business, false);
    assert_eq!(
        mapped.content.body.as_deref(),
        Some("A delivery\nKeep my full body")
    );
    assert_eq!(
        mapped.content.actions[0].target,
        Target::Url("https://example.invalid/tracking".into())
    );
    assert!(
        mapped
            .text
            .ends_with("Track package: https://example.invalid/tracking")
    );
}

#[tokio::test]
async fn archived_cards_upgrade_repair_alias_merge_and_mutations_keep_privacy() {
    use crate::{
        archive::tests::KEY,
        storage::{prepare_at, tests::MemoryVault},
    };
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let archive = Archive::open(files.clone(), KEY).unwrap();
    let mut record = super::tests::mapped(&preview(thumbnail()), false);
    record.id = "preview".into();
    let expected = record.content.clone();
    // Simulate a row saved before content support, without altering its body.
    record.content = Default::default();
    record.thumbnail.clear();
    let mut duplicate = record.clone();
    duplicate.chat = LID.into();
    duplicate.sender = LID.into();
    archive
        .ingest(Batch {
            messages: vec![record, duplicate],
            ..Default::default()
        })
        .await
        .unwrap();
    let chat = archive.create_chat(PN.into()).await.unwrap();
    archive.draft(chat, "Keep draft".into()).await.unwrap();
    // Reopen the actual pre-feature schema, with real encrypted envelopes.
    archive
        .blocking(|db| {
            db.execute_batch(
                "DROP TRIGGER discard_changed_document_preview; DROP TABLE document_previews; DROP TABLE outgoing_reads; DROP INDEX messages_action_wire; DROP TABLE reply_drafts; DROP TABLE reactions; DROP TRIGGER discard_changed_audio; DROP TABLE message_audio; DROP TRIGGER discard_changed_photo;
                          DROP TABLE message_photos;
                          ALTER TABLE messages DROP COLUMN content;
                          ALTER TABLE messages DROP COLUMN thumbnail;
                          DROP TABLE history_pages; DROP TABLE history_requests; ALTER TABLE messages DROP COLUMN edit_version; ALTER TABLE mutations DROP COLUMN raw; DROP TABLE outgoing_receipts; DROP TABLE chat_settings; PRAGMA user_version=4;",
            )?;
            Ok(())
        })
        .unwrap();
    drop(archive);
    let archive = Archive::open(files.clone(), KEY).unwrap();
    archive.blocking(|db|{db.execute_batch("CREATE TRIGGER fail_content BEFORE UPDATE OF content ON messages BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;")?;Ok(())}).unwrap();
    assert_eq!(repair_text(&archive), Err(archive::Error::Database));
    archive
        .blocking(|db| {
            db.execute_batch("DROP TRIGGER fail_content;")?;
            Ok(())
        })
        .unwrap();
    repair_text(&archive).unwrap();
    archive
        .repair_message_content(|_| panic!("completed upgrade must not decode again"))
        .unwrap();
    archive
        .ingest(Batch {
            aliases: vec![(LID.into(), PN.into())],
            ..Default::default()
        })
        .await
        .unwrap();
    drop(archive);
    let archive = Archive::open(files, KEY).unwrap();
    let view = archive
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.messages.len(), 1);
    crate::archive::tests::assert_content_payload(&view.messages[0].content, &expected);
    assert_eq!(view.draft, "Keep draft");
    for (version, text) in [(1, Some("Edited text".into())), (2, None)] {
        let mut change = crate::archive::tests::record(&format!("change-{version}"));
        change.visible = false;
        change.change = Some(Change {
            id: "preview".into(),
            sender: PN.into(),
            text,
            version,
            raw: Vec::new(),
        });
        archive
            .ingest(Batch {
                messages: vec![change],
                ..Default::default()
            })
            .await
            .unwrap();
        let view = archive
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        crate::archive::tests::assert_content_payload(
            &view.messages[0].content,
            &Content {
                unavailable: version == 2,
                ..Default::default()
            },
        );
        archive
            .blocking(|db| {
                let size: i64 = db.query_row(
                    "SELECT length(thumbnail) FROM messages WHERE wire_id='preview'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(size, 0);
                Ok(())
            })
            .unwrap();
        archive
            .blocking(|db| {
                db.execute(
                    "DELETE FROM metadata WHERE key='content_decoder_revision'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        repair_text(&archive).unwrap();
    }
    let mut expired = super::tests::mapped(&preview(thumbnail()), false);
    expired.id = "expired-card".into();
    expired.expires = Some(1);
    archive
        .ingest(Batch {
            messages: vec![expired],
            ..Default::default()
        })
        .await
        .unwrap();
    archive
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    archive.blocking(|db|{let size:i64=db.query_row("SELECT length(content)+length(thumbnail)+length(raw) FROM messages WHERE wire_id='expired-card'",[],|r|r.get(0))?;assert_eq!(size,0);Ok(())}).unwrap();
}

#[tokio::test]
async fn embedded_thumbnail_worker_reuses_pixels_evicts_and_handles_corrupt_images() {
    use std::time::Duration;
    use tokio::sync::watch;
    let (_temp, db) = fixture();
    let good = super::tests::mapped(&preview(thumbnail()), false);
    let mut broken = super::tests::mapped(&preview(vec![0xff, 0]), false);
    broken.id = "broken-image".into();
    db.ingest(Batch {
        messages: vec![good, broken],
        ..Default::default()
    })
    .await
    .unwrap();
    let ids = db
        .blocking(|db| {
            Ok(db
                .prepare("SELECT id FROM messages ORDER BY id")?
                .query_map([], |r| r.get::<_, i32>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .unwrap();
    let (archives, archive_rx) = watch::channel(Some(db.clone()));
    let (demand, demand_rx) = watch::channel(ids.clone());
    let (updates, mut snapshots) = watch::channel(crate::message_thumbnails::Snapshot::default());
    let worker = tokio::spawn(crate::message_thumbnails::present(
        archive_rx, demand_rx, updates,
    ));
    tokio::time::timeout(Duration::from_secs(15), snapshots.changed())
        .await
        .unwrap()
        .unwrap();
    let first = snapshots.borrow_and_update().clone();
    assert_eq!(first.len(), 1);
    assert_eq!(first[&ids[0]].rgba.len(), 160 * 90 * 4);
    db.changed();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), snapshots.changed())
            .await
            .is_err()
    );
    demand.send_replace(Vec::new());
    tokio::time::timeout(Duration::from_secs(15), snapshots.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(snapshots.borrow_and_update().is_empty());
    demand.send_replace(ids.clone());
    tokio::time::timeout(Duration::from_secs(15), snapshots.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(!Arc::ptr_eq(
        &first[&ids[0]],
        &snapshots.borrow_and_update()[&ids[0]]
    ));
    let messages = (0..40)
        .map(|i| {
            let mut record = super::tests::mapped(&preview(thumbnail()), false);
            record.id = format!("bounded-{i}");
            record
        })
        .collect();
    db.ingest(Batch {
        messages,
        ..Default::default()
    })
    .await
    .unwrap();
    let ids = db
        .blocking(|db| {
            Ok(db
                .prepare("SELECT id FROM messages WHERE wire_id LIKE 'bounded-%' ORDER BY id")?
                .query_map([], |r| r.get::<_, i32>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .unwrap();
    demand.send_replace(ids.clone());
    tokio::time::timeout(Duration::from_secs(15), snapshots.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshots.borrow_and_update().len(),
        crate::message_thumbnails::MAX_DEMAND
    );
    assert!(snapshots.borrow().keys().all(|id| ids[..32].contains(id)));
    archives.send_replace(None);
    tokio::time::timeout(Duration::from_secs(15), snapshots.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(snapshots.borrow().is_empty());
    drop(demand);
    tokio::time::timeout(Duration::from_secs(15), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn attachment_menu_classification_uses_protocol_metadata_and_repairs_saved_files() {
    let mut document = wa::Message::default();
    document.document_message = MessageField::some(wa::message::DocumentMessage::default());
    let record = super::tests::mapped(&document, false);
    assert!(record.content.has_attachment);
    assert!(record.content.photo.is_none() && record.content.audio.is_none());
    let mut text = wa::Message::default();
    text.conversation = Some("[Document] This is only text".into());
    assert!(!super::tests::mapped(&text, false).content.has_attachment);
    let mut once = wa::Message::default();
    let mut wrapper = wa::message::FutureProofMessage::default();
    wrapper.message = MessageField::some(document);
    once.view_once_message = MessageField::some(wrapper);
    assert!(!super::tests::mapped(&once, false).content.has_attachment);

    let (_temp, archive) = fixture();
    let mut legacy = record;
    legacy.content = Content::default();
    archive
        .ingest(Batch {
            messages: vec![legacy],
            ..Default::default()
        })
        .await
        .unwrap();
    archive
        .blocking(|db| {
            db.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('content_decoder_revision','4')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    repair_text(&archive).unwrap();
    let chat = archive.create_chat(PN.into()).await.unwrap();
    let view = archive
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert!(view.messages[0].content.has_attachment);
    archive
        .repair_message_content(|_| panic!("repair must run only once"))
        .unwrap();
}
