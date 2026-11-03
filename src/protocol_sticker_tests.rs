//! Synthetic sticker envelopes only; no account media or network requests.
use super::*;
use crate::archive::tests::{PN, fixture};
use whatsapp_rust::waproto::buffa::MessageField;

fn sticker() -> wa::Message {
    let mut message = wa::Message::default();
    let sticker = message.sticker_message.get_or_insert_default();
    sticker.width = Some(512);
    sticker.height = Some(512);
    sticker.file_length = Some(13862);
    sticker.mimetype = Some("image/webp".into());
    message
}

#[test]
fn static_stickers_keep_identity_wrappers_and_message_actions() {
    let mut message = sticker();
    let context = message
        .sticker_message
        .get_or_insert_default()
        .context_info
        .get_or_insert_default();
    context.is_forwarded = Some(true);
    let mapped = super::tests::mapped(&message, false);
    assert_eq!(mapped.text, "[Sticker]");
    assert!(mapped.visible && mapped.content.forwarded && mapped.content.has_attachment);
    assert_eq!(mapped.content.body.as_deref(), Some(""));
    assert!(mapped.content.photo.is_none() && mapped.album.is_none());
    assert_eq!(mapped.content.sticker.as_ref().unwrap().width, 512);
    assert_eq!(mapped.content.sticker.as_ref().unwrap().size, Some(13862));
    assert!(mapped.thumbnail.is_empty());
    assert_eq!(mapped.raw, codec::message_to_vec(&message));

    let mut wrapped = wa::Message::default();
    wrapped.ephemeral_message.get_or_insert_default().message = MessageField::some(message.clone());
    assert_eq!(
        super::tests::mapped(&wrapped, false).content,
        mapped.content
    );
    wrapped.ephemeral_message = Default::default();
    wrapped.view_once_message.get_or_insert_default().message = MessageField::some(message);
    let once = super::tests::mapped(&wrapped, false);
    assert!(once.raw.is_empty() && once.content.sticker.is_none());
    assert!(once.text.contains("View-once"));
}

#[test]
fn animated_and_malformed_sticker_metadata_do_not_claim_static_playback() {
    for lottie in [false, true] {
        let mut message = sticker();
        let sticker = message.sticker_message.get_or_insert_default();
        if lottie {
            sticker.is_lottie = Some(true);
        } else {
            sticker.is_animated = Some(true);
        }
        let mapped = super::tests::mapped(&message, false);
        assert_eq!(mapped.text, "[Animated sticker]");
        assert!(mapped.content.sticker.is_none() && mapped.content.has_attachment);
    }
    let mut message = sticker();
    let sticker = message.sticker_message.get_or_insert_default();
    sticker.width = Some(u32::MAX);
    sticker.height = Some(0);
    sticker.png_thumbnail = Some(vec![0; crate::message_thumbnails::MAX_ENCODED + 1]);
    let mapped = super::tests::mapped(&message, false);
    let dimensions = mapped.content.sticker.unwrap();
    assert_eq!((dimensions.width, dimensions.height), (512, 512));
    assert!(mapped.thumbnail.is_empty());
}

#[tokio::test]
async fn saved_unsupported_stickers_upgrade_in_place_without_changing_read_state() {
    let (_temp, db) = fixture();
    let old = |id: &str| {
        let mut row = super::tests::mapped(&sticker(), false);
        row.id = id.into();
        row.text = "[Unsupported message]".into();
        row.content = Default::default();
        row
    };
    let mut hidden = old("hidden");
    hidden.visible = false;
    let mut expired = old("expired");
    expired.expires = Some(1);
    let mut deleted = old("revoke");
    deleted.visible = false;
    deleted.change = Some(Change {
        id: "deleted".into(),
        sender: PN.into(),
        text: None,
        version: 2,
        raw: Vec::new(),
    });
    db.ingest(Batch {
        messages: vec![old("sticker"), old("deleted"), hidden, expired, deleted],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let identity = || {
        db.blocking(|db| {
            Ok(db.query_row(
                "SELECT id,stamp,seen,unread,raw FROM messages WHERE wire_id='sticker'",
                [],
                |r| {
                    Ok((
                        r.get::<_, i32>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )?)
        })
        .unwrap()
    };
    let before = identity();
    db.blocking(|db| {
        db.execute("INSERT OR REPLACE INTO metadata(key,value) VALUES('text_decoder_revision','4'),('content_decoder_revision','12')",[])?;
        Ok(())
    }).unwrap();
    repair_text(&db).unwrap();
    assert_eq!(identity(), before);
    let chat = db.create_chat(PN.into()).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    let sticker = view.messages.iter().find(|m| m.id == before.0).unwrap();
    assert!(sticker.content.sticker.is_some());
    assert_eq!(sticker.text, "[Sticker]");
    assert!(
        view.messages
            .iter()
            .any(|m| m.text == crate::domain::DELETED_MESSAGE && m.content.sticker.is_none())
    );
    db.blocking(|db| {
        let hidden: String = db.query_row(
            "SELECT body FROM messages WHERE wire_id='hidden'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(hidden, "[Unsupported message]");
        Ok(())
    })
    .unwrap();
    // Completed revision must not decode the whole archive at every startup.
    db.repair_unsupported_messages(5, |_| panic!("completed repair decoded again"))
        .unwrap();
    repair_text(&db).unwrap();
    assert_eq!(identity(), before);
}
