//! Synthetic album headers, individual media and independent revokes.
use super::*;
use crate::archive::tests::{KEY, PN, fixture};
use whatsapp_rust::{prelude::MessageBuilderExt, waproto::buffa::MessageField};

const GROUP: &str = "15550000001-123456789@g.us";
const CAPTION: &str = "Synthetic album caption 👩🏽‍💻\nSecond line";

fn album() -> wa::Message {
    let mut album = wa::message::AlbumMessage::default();
    album.expected_image_count = Some(2);
    album.expected_video_count = Some(0);
    let mut message = wa::Message::default();
    message.album_message = MessageField::some(album);
    message
}
fn photo() -> wa::Message {
    let mut photo = wa::message::ImageMessage::default();
    photo.caption = Some(CAPTION.into());
    photo.width = Some(640);
    photo.height = Some(480);
    let mut message = wa::Message::default();
    message.image_message = MessageField::some(photo);
    let mut association = wa::MessageAssociation::default();
    association.association_type = Some(wa::message_association::AssociationType::MEDIA_ALBUM);
    association.parent_message_key.get_or_insert_default().id = Some("album-0".into());
    message
        .message_context_info
        .get_or_insert_default()
        .message_association = MessageField::some(association);
    message
}
fn deletion() -> wa::Message {
    let mut protocol = wa::message::ProtocolMessage::default();
    protocol.r#type = Some(wa::message::protocol_message::Type::REVOKE);
    let key = protocol.key.get_or_insert_default();
    key.id = Some("deleted-photo".into());
    key.remote_jid = Some(GROUP.into());
    key.participant = Some(PN.into());
    key.from_me = Some(false);
    let mut message = wa::Message::default();
    message.protocol_message = MessageField::some(protocol);
    message
}
fn row(id: &str, stamp: i64, message: &wa::Message) -> Record {
    record(
        &GROUP.parse().unwrap(),
        &PN.parse().unwrap(),
        id.into(),
        1_700_000_000 + stamp,
        false,
        "Synthetic sender".into(),
        message,
        None,
    )
}
async fn load(db: &Archive) -> crate::domain::Conversation {
    let chat = db.create_chat(GROUP.into()).await.unwrap();
    db.view(chat, 300, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
}

#[test]
fn album_headers_are_metadata_while_unknown_messages_and_view_once_stay_distinct() {
    let header = album();
    let mapped = row("header", 100, &header);
    assert!(!mapped.visible && mapped.change.is_none() && mapped.text.is_empty());
    assert_eq!(
        mapped.raw,
        codec::message_to_vec(&header),
        "keep album identity/counts for its children"
    );
    let mut wrapped = wa::Message::default();
    wrapped.ephemeral_message.get_or_insert_default().message = MessageField::some(header.clone());
    assert!(!row("wrapped", 100, &wrapped).visible);
    wrapped.ephemeral_message = Default::default();
    wrapped.view_once_message.get_or_insert_default().message = MessageField::some(header);
    let once = row("once", 100, &wrapped);
    assert!(once.visible && once.raw.is_empty());
    assert!(once.text.contains("View-once"));
    let unknown = row("unknown", 100, &wa::Message::default());
    assert!(unknown.visible);
    assert_eq!(unknown.text, "[Unsupported message]");
    assert!(row("literal", 100, &wa::Message::text("[Unsupported message]")).visible);
}

#[tokio::test]
async fn live_and_history_album_headers_do_not_add_bubbles_unreads_or_previews() {
    for history in [false, true] {
        let (_temp, db) = fixture();
        db.ingest(Batch {
            messages: vec![row("earlier", 50, &wa::Message::text("Earlier message"))],
            ..Default::default()
        })
        .await
        .unwrap();
        db.ingest(Batch {
            messages: vec![row("album-0", 100, &album())],
            live: !history,
            ..Default::default()
        })
        .await
        .unwrap();
        let before = load(&db).await;
        assert_eq!(before.messages.len(), 1);
        assert!(before.chat.preview.ends_with("Earlier message"));
        assert_eq!(
            before.chat.unread, 0,
            "a live header cannot add an unread message"
        );
        let records = [
            row("album-0", 100, &album()),
            row("revoke", 100, &deletion()),
            row("photo", 100, &photo()),
        ];
        if history {
            let mut conversation = wa::Conversation::default();
            conversation.id = GROUP.into();
            conversation.messages = records
                .iter()
                .map(|row| {
                    let mut info = wa::WebMessageInfo::default();
                    let key = info.key.get_or_insert_default();
                    key.remote_jid = Some(GROUP.into());
                    key.participant = Some(PN.into());
                    key.id = Some(row.id.clone());
                    key.from_me = Some(false);
                    // A parsed revoke intentionally has no retained Record.raw.
                    info.message = MessageField::some(if row.change.is_some() {
                        deletion()
                    } else {
                        codec::message_decode(&row.raw).unwrap()
                    });
                    info.message_timestamp = Some(row.timestamp as u64);
                    let mut item = wa::HistorySyncMsg::default();
                    item.message = MessageField::some(info);
                    item
                })
                .collect();
            let mut sync = wa::HistorySync::default();
            sync.conversations = vec![conversation];
            db.capture_history("album-history".into(), super::tests::compressed(&sync))
                .await
                .unwrap();
            import_pending(&db, &AtomicBool::new(false)).unwrap();
        } else {
            db.ingest(Batch {
                messages: records.into(),
                live: true,
                ..Default::default()
            })
            .await
            .unwrap();
        }
        let after = load(&db).await;
        assert_eq!(after.messages.len(), 3);
        assert_eq!(after.messages[1].text, "This message was deleted");
        assert_eq!(after.messages[2].content.body.as_deref(), Some(CAPTION));
        assert!(
            after
                .chat
                .preview
                .ends_with("Photo: Synthetic album caption 👩🏽‍💻")
        );
        assert_eq!(after.chat.unread, usize::from(!history));
        assert!(!after.transcript.contains("Unsupported"));
        let header = codec::message_to_vec(&album());
        db.blocking(|c| {
            let saved: (bool, bool, Vec<u8>) = c.query_row(
                "SELECT visible,unread,raw FROM messages WHERE wire_id='album-0'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            assert_eq!(saved, (false, false, header));
            Ok(())
        })
        .unwrap();
    }
}

#[tokio::test]
async fn saved_album_repair_resumes_batches_recounts_and_preserves_the_deleted_item_and_photo() {
    let temp = tempfile::tempdir().unwrap();
    let files =
        crate::storage::prepare_at(temp.path(), &crate::storage::tests::MemoryVault::default())
            .unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    let mut headers = (0..205)
        .map(|i| {
            let mut header = row(&format!("album-{i}"), 200 + i, &album());
            header.visible = true;
            header.text = "[Unsupported message]".into();
            header
        })
        .collect::<Vec<_>>();
    headers.extend([row("revoke", 100, &deletion()), row("photo", 100, &photo())]);
    db.ingest(Batch {
        messages: headers,
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let before = load(&db).await; // Populate the real timeline cache before repair.
    assert_eq!(before.messages.len(), 207);
    assert_eq!(before.chat.unread, 206);
    assert!(before.chat.preview.ends_with("[Unsupported message]"));
    db.draft(before.chat.id, "Keep this draft".into())
        .await
        .unwrap();
    db.blocking(|c| {
        c.execute_batch("INSERT OR REPLACE INTO metadata VALUES('text_decoder_revision','1'); CREATE TRIGGER fail_album_repair BEFORE UPDATE OF visible ON messages WHEN OLD.wire_id='album-150' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;")?; Ok(())
    }).unwrap();
    assert_eq!(repair_text(&db), Err(archive::Error::Database));
    db.blocking(|c| {
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM messages WHERE wire_id LIKE 'album-%' AND visible=0",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            100
        );
        assert_eq!(
            c.query_row(
                "SELECT unread FROM chats WHERE id=?1",
                [before.chat.id],
                |r| r.get::<_, i64>(0)
            )?,
            106,
            "counter changes roll back with the failed batch"
        );
        c.execute_batch("DROP TRIGGER fail_album_repair;")?;
        Ok(())
    })
    .unwrap();
    drop(db);
    let db = Archive::open(files.clone(), KEY).unwrap();
    repair_text(&db).unwrap();
    let after = load(&db).await;
    assert_eq!(after.messages.len(), 2);
    assert_eq!(after.messages[0].id, before.messages[0].id);
    assert_eq!(after.messages[0].text, "This message was deleted");
    assert_eq!(after.messages[1].id, before.messages[1].id);
    assert_eq!(after.messages[1].content.body.as_deref(), Some(CAPTION));
    assert_eq!(after.chat.unread, 1);
    assert_eq!(after.draft, "Keep this draft");
    assert!(
        after
            .chat
            .preview
            .ends_with("Photo: Synthetic album caption 👩🏽‍💻")
    );
    db.blocking(|c| {
        assert_eq!(c.query_row("SELECT count(*) FROM messages WHERE wire_id LIKE 'album-%' AND visible=0 AND unread=0 AND length(raw)>0",[],|r|r.get::<_,i64>(0))?,205);
        let raw:Vec<u8>=c.query_row("SELECT raw FROM messages WHERE wire_id='photo'",[],|r|r.get(0))?;
        let message=codec::message_decode(&raw).unwrap();
        assert_eq!(message.message_context_info.message_association.parent_message_key.id.as_deref(),Some("album-0")); Ok(())
    }).unwrap();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    db.repair_unsupported_messages(3, |_| panic!("completed repair must not scan again"))
        .unwrap();
    assert_eq!(load(&db).await.messages.len(), 2);
    db.ingest(Batch {
        messages: vec![row("album-0", 200, &album())],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        load(&db).await.messages.len(),
        2,
        "replayed headers cannot return"
    );
}

fn child(parent: &str, index: Option<i32>, wrapped: bool) -> wa::Message {
    let mut message = photo();
    message
        .message_context_info
        .get_or_insert_default()
        .message_association
        .get_or_insert_default()
        .parent_message_key
        .get_or_insert_default()
        .id = Some(parent.into());
    message
        .message_context_info
        .get_or_insert_default()
        .message_association
        .get_or_insert_default()
        .message_index = index;
    if wrapped {
        let mut outer = wa::Message::default();
        outer.message_context_info = message
            .message_context_info
            .take()
            .map(MessageField::some)
            .unwrap_or_default();
        outer
            .associated_child_message
            .get_or_insert_default()
            .message = MessageField::some(message);
        outer
    } else {
        message
    }
}

#[test]
fn associated_children_keep_media_caption_and_outer_album_context_without_exposing_view_once() {
    for wrapped in [false, true] {
        let message = child("explicit-parent", Some(2), wrapped);
        let mapped = row("child", 100, &message);
        assert!(mapped.visible && mapped.content.photo.is_some());
        assert_eq!(mapped.content.body.as_deref(), Some(CAPTION));
        assert_eq!(
            mapped.album,
            Some(crate::message_content::Album {
                parent: "explicit-parent".into(),
                index: Some(2)
            })
        );
        assert_eq!(
            message
                .clone()
                .into_base_message()
                .image_message
                .caption
                .as_deref(),
            Some(CAPTION)
        );
        let mut once = wa::Message::default();
        once.view_once_message_v2.get_or_insert_default().message = MessageField::some(photo());
        let mut wrapper = wa::Message::default();
        wrapper
            .associated_child_message
            .get_or_insert_default()
            .message = MessageField::some(once);
        assert!(wrapper.is_view_once());
        let hidden = row("once", 100, &wrapper);
        assert!(hidden.raw.is_empty() && hidden.content.photo.is_none() && hidden.album.is_none());
    }
    let mut unknown = child("parent", None, true);
    unknown
        .message_context_info
        .get_or_insert_default()
        .message_association
        .get_or_insert_default()
        .association_type = None;
    assert!(crate::protocol_content::album(&unknown).is_none());
}

#[tokio::test]
async fn viewer_resolves_only_explicit_album_photos_across_pages_and_rechecks_mutations_and_locks()
{
    let (_temp, db) = fixture();
    // Arrive out of order, with other messages interleaved; no parent header is
    // needed to use a child's explicit association. Exclude a mixed-in video.
    let mut messages = vec![
        row("third", 200, &child("parent", Some(2), true)),
        row("first", 200, &child("parent", Some(0), false)),
        row("second", 200, &child("parent", Some(1), true)),
        row("standalone", 200, &photo()),
    ];
    messages[3].album = None;
    let mut other = row("other-author", 200, &child("parent", Some(0), true));
    other.sender = "15550000999@s.whatsapp.net".into();
    messages.push(other);
    let mut other = row("other-chat", 200, &child("parent", Some(0), false));
    other.chat = "15550000002-123456789@g.us".into();
    messages.push(other);
    let mut video = wa::Message::default();
    video.video_message.get_or_insert_default().width = Some(640);
    video.message_context_info = child("parent", Some(3), false).message_context_info;
    messages.push(row("video", 200, &video));
    for i in 0..150 {
        messages.push(row(
            &format!("newer-{i}"),
            300 + i,
            &wa::Message::text("Newer synthetic text"),
        ));
    }
    db.ingest(Batch {
        messages,
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(GROUP.into()).await.unwrap();
    let id = |wire: &str| {
        db.blocking(|c| {
            Ok(
                c.query_row("SELECT id FROM messages WHERE wire_id=?1", [wire], |r| {
                    r.get::<_, i32>(0)
                })?,
            )
        })
        .unwrap()
    };
    let first = id("first");
    let second = id("second");
    let third = id("third");
    let page = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert!(page.messages.iter().all(|m| m.content.photo.is_none()));
    let gallery = db.photo_gallery(chat, second).await.unwrap().unwrap();
    assert_eq!(gallery.members, vec![first, second, third]);
    assert_eq!(gallery.position, 1);
    assert_eq!(gallery.caption, CAPTION);
    assert_eq!(
        db.photo_gallery(chat, id("standalone"))
            .await
            .unwrap()
            .unwrap()
            .members,
        vec![id("standalone")]
    );
    assert!(
        db.photo_gallery(chat, id("other-chat"))
            .await
            .unwrap()
            .is_none()
    );
    // A caption edit preserves association columns. Deleted/expired/hidden
    // media cannot remain navigable or downloadable while the viewer is open.
    db.blocking(|c|{c.execute("UPDATE messages SET body='Edited',content=json_set(content,'$.body','Edited') WHERE id=?1",[second])?;c.execute("UPDATE messages SET deleted=1 WHERE id=?1",[first])?;c.execute("UPDATE messages SET expires=unixepoch()-1 WHERE id=?1",[third])?;Ok(())}).unwrap();
    let gallery = db.photo_gallery(chat, second).await.unwrap().unwrap();
    assert_eq!(gallery.members, vec![second]);
    assert_eq!(gallery.caption, "Edited");
    db.blocking(|c| {
        c.execute(
            "INSERT INTO chat_settings(jid,kind,value,stamp) VALUES(?1,2,1,1)",
            [GROUP],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(db.photo_gallery(chat, second).await.unwrap().is_none());
    assert!(db.photo_request(second).await.unwrap().is_none());
    assert!(db.photo_candidates(vec![second]).await.unwrap().is_empty());
}

#[tokio::test]
async fn album_backfill_reopens_preserves_cache_and_alias_merges_keep_membership() {
    use crate::archive::tests::LID;
    let temp = tempfile::tempdir().unwrap();
    let files =
        crate::storage::prepare_at(temp.path(), &crate::storage::tests::MemoryVault::default())
            .unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    let mut first = row("a", 100, &child("parent", None, true));
    first.chat = LID.into();
    first.sender = LID.into();
    first.album = None;
    let mut second = row("b", 101, &child("parent", None, false));
    second.chat = PN.into();
    second.sender = PN.into();
    second.album = None;
    db.ingest(Batch {
        messages: vec![first, second],
        ..Default::default()
    })
    .await
    .unwrap();
    let first = db
        .blocking(|c| {
            Ok(
                c.query_row("SELECT id FROM messages WHERE wire_id='a'", [], |r| {
                    r.get::<_, i32>(0)
                })?,
            )
        })
        .unwrap();
    let request = db.photo_request(first).await.unwrap().unwrap();
    db.save_photo(
        &request,
        crate::photos::Cached {
            original: vec![1, 2, 3],
            preview: vec![4, 5, 6],
        },
    )
    .await
    .unwrap();
    db.blocking(|c|{c.execute_batch("DROP INDEX messages_album; ALTER TABLE messages DROP COLUMN album_index; ALTER TABLE messages DROP COLUMN album; PRAGMA user_version=17;")?;Ok(())}).unwrap();
    drop(db);
    let db = Archive::open(files.clone(), KEY).unwrap();
    repair_text(&db).unwrap();
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let gallery = db.photo_gallery(chat, first).await.unwrap().unwrap();
    assert_eq!(gallery.members.len(), 2);
    assert_eq!(gallery.position, 0);
    assert_eq!(
        db.clipboard_photo(chat, first).await.unwrap(),
        Some(vec![1, 2, 3])
    );
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    db.repair_albums(|_| panic!("completed backfill must not rescan"))
        .unwrap();
    assert_eq!(
        db.photo_gallery(chat, first)
            .await
            .unwrap()
            .unwrap()
            .members,
        gallery.members
    );
}
