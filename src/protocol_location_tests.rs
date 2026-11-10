//! Synthetic fixed locations; no real places, account data or network requests.
use super::*;
use whatsapp_rust::waproto::buffa::MessageField;

fn location() -> wa::Message {
    let mut message = wa::Message::default();
    let l = message.location_message.get_or_insert_default();
    l.degrees_latitude = Some(0.);
    l.degrees_longitude = Some(0.);
    l.name = Some("Sample meeting point".into());
    l.address = Some("Synthetic street 42".into());
    l.url = Some("https://example.org/place".into());
    l.context_info.get_or_insert_default().is_forwarded = Some(true);
    message
}
#[test]
fn fixed_locations_keep_coordinates_context_and_original_envelope() {
    let message = location();
    let row = tests::mapped(&message, false);
    assert!(row.visible && row.content.forwarded && !row.content.has_attachment);
    let l = row.content.location.as_ref().unwrap();
    assert_eq!(l.coordinates(), Some((0., 0.)));
    assert!(l.map_url().contains("q=0,0"));
    assert_eq!(l.summary(), "Sample meeting point");
    assert!(row.text.contains("Synthetic street 42"));
    assert_eq!(row.raw, codec::message_to_vec(&message));
    assert_eq!(row.content.body.as_deref(), Some(""));
    let mut wrapped = wa::Message::default();
    wrapped.ephemeral_message.get_or_insert_default().message = MessageField::some(message.clone());
    assert_eq!(tests::mapped(&wrapped, false).content, row.content);
    wrapped.ephemeral_message = Default::default();
    wrapped.view_once_message.get_or_insert_default().message = MessageField::some(message);
    assert!(tests::mapped(&wrapped, false).content.location.is_none());
}
#[test]
fn invalid_fixed_locations_do_not_become_map_requests() {
    for (lat, lon) in [
        (None, Some(0.)),
        (Some(f64::NAN), Some(0.)),
        (Some(0.), Some(181.)),
        (Some(91.), Some(0.)),
    ] {
        let mut message = location();
        let l = message.location_message.get_or_insert_default();
        l.degrees_latitude = lat;
        l.degrees_longitude = lon;
        let row = tests::mapped(&message, false);
        assert!(row.content.location.is_none());
        assert_eq!(row.text, "[Location unavailable]");
    }
    let mut message = location();
    message.location_message.get_or_insert_default().is_live = Some(true);
    assert!(
        tests::mapped(&message, false)
            .content
            .location
            .unwrap()
            .live
    );
    message.location_message.get_or_insert_default().is_live = None;
    message.location_message.get_or_insert_default().url = Some("javascript:alert(1)".into());
    assert!(
        tests::mapped(&message, false)
            .content
            .location
            .unwrap()
            .website
            .is_empty()
    );
}
#[tokio::test]
async fn saved_locations_repair_in_place_and_use_existing_preview_guards() {
    let (_temp, db) = crate::archive::tests::fixture();
    let mut row = tests::mapped(&location(), false);
    row.text = "[Unsupported message]".into();
    row.content = Default::default();
    db.ingest(Batch {
        messages: vec![row],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let identity = || {
        db.blocking(|c| {
            Ok(c.query_row(
                "SELECT id,stamp,unread,seen,raw FROM messages LIMIT 1",
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
    db.blocking(|c| {
        c.execute(
            "INSERT OR REPLACE INTO metadata(key,value) VALUES('text_decoder_revision','5')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    repair_text(&db).unwrap();
    assert_eq!(identity(), before);
    db.repair_unsupported_messages(6, |_| panic!("repair must not rescan"))
        .unwrap();
    let request = db
        .document_preview_request(vec![before.0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.kind, crate::document_previews::Kind::Location);
    db.save_document_preview(&request, None).await.unwrap();
    assert!(
        db.document_preview_request(vec![before.0])
            .await
            .unwrap()
            .is_none(),
        "failed maps use the existing cooldown"
    );
    let chat = db
        .create_chat(crate::archive::tests::PN.into())
        .await
        .unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    assert!(
        view.chats
            .iter()
            .any(|c| c.preview == "Sample meeting point")
    );
    db.blocking(|c| {
        c.execute("UPDATE messages SET deleted=1 WHERE id=?1", [before.0])?;
        Ok(())
    })
    .unwrap();
    assert!(!db.document_preview_current(&request).await.unwrap());
    assert!(
        !db.save_document_preview(&request, Some(vec![1, 2, 3]))
            .await
            .unwrap(),
        "late map cannot revive a deletion"
    );
}

fn live_location() -> wa::Message {
    let mut message = wa::Message::default();
    let live = message.live_location_message.get_or_insert_default();
    live.degrees_latitude = Some(0.);
    live.degrees_longitude = Some(0.);
    live.caption = Some("Meet here 👩🏽‍💻\nSecond line".into());
    live.context_info.get_or_insert_default().is_forwarded = Some(true);
    message
}
#[test]
fn live_location_is_a_static_card_with_caption_context_and_validated_coordinates() {
    let message = live_location();
    let row = tests::mapped(&message, false);
    let l = row.content.location.as_ref().unwrap();
    assert!(l.live && row.visible && !row.content.unavailable && row.content.forwarded);
    assert_eq!(l.coordinates(), Some((0., 0.)));
    assert_eq!(l.summary(), "Live location");
    assert_eq!(
        row.content.body.as_deref(),
        Some("Meet here 👩🏽‍💻\nSecond line")
    );
    assert_eq!(row.text, "Live location\nMeet here 👩🏽‍💻\nSecond line");
    assert_eq!(row.raw, codec::message_to_vec(&message));
    let mut wrapped = wa::Message::default();
    wrapped.ephemeral_message.get_or_insert_default().message = MessageField::some(message.clone());
    assert_eq!(tests::mapped(&wrapped, false).content, row.content);
    wrapped.ephemeral_message = Default::default();
    wrapped.view_once_message.get_or_insert_default().message = MessageField::some(message.clone());
    assert!(tests::mapped(&wrapped, false).content.location.is_none());
    // A legacy live flag gets the same card, never a fixed-location map link.
    let mut legacy = location();
    legacy.location_message.get_or_insert_default().is_live = Some(true);
    legacy.location_message.get_or_insert_default().comment = Some("Legacy caption".into());
    let row = tests::mapped(&legacy, false);
    assert!(row.content.location.unwrap().live);
    assert_eq!(row.content.body.as_deref(), Some("Legacy caption"));
    for (lat, lon) in [
        (None, Some(0.)),
        (Some(f64::NAN), Some(0.)),
        (Some(91.), Some(0.)),
        (Some(0.), Some(181.)),
    ] {
        let mut invalid = message.clone();
        let live = invalid.live_location_message.get_or_insert_default();
        live.degrees_latitude = lat;
        live.degrees_longitude = lon;
        let row = tests::mapped(&invalid, false);
        let l = row.content.location.as_ref().unwrap();
        assert!(l.live && l.coordinates().is_none());
        assert!(
            !crate::presentation::message_row(
                &crate::domain::Message {
                    content: std::sync::Arc::new(row.content),
                    ..crate::fixtures::demo().conversation().messages[0].clone()
                },
                None
            )
            .has_thumbnail
        );
    }
}
#[tokio::test]
async fn saved_phone_only_live_locations_upgrade_without_changing_identity_or_read_state() {
    let (_temp, db) = crate::archive::tests::fixture();
    let mut row = tests::mapped(&live_location(), false);
    row.text = "Live location. Open WhatsApp on your phone to view it.".into();
    row.content = crate::message_content::Content {
        body: Some(row.text.clone()),
        ..Default::default()
    };
    db.ingest(Batch {
        messages: vec![row],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let identity = || {
        db.blocking(|c| {
            Ok(
                c.query_row("SELECT id,stamp,unread,seen,raw FROM messages", [], |r| {
                    Ok((
                        r.get::<_, i32>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, Vec<u8>>(4)?,
                    ))
                })?,
            )
        })
        .unwrap()
    };
    let before = identity();
    db.blocking(|c| {
        c.execute(
            "INSERT OR REPLACE INTO metadata(key,value) VALUES('content_decoder_revision','13')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    repair_text(&db).unwrap();
    assert_eq!(identity(), before);
    let chat = db
        .create_chat(crate::archive::tests::PN.into())
        .await
        .unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    assert_eq!(
        view.chats.iter().find(|c| c.id == chat).unwrap().preview,
        "Live location"
    );
    let m = view.conversation.as_ref().unwrap().messages.last().unwrap();
    assert!(m.content.location.as_ref().unwrap().live);
    assert_eq!(m.text, "Live location\nMeet here 👩🏽‍💻\nSecond line");
    let request = db
        .document_preview_request(vec![before.0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.kind, crate::document_previews::Kind::Location);
    let mut revoked = wa::Message::default();
    let protocol = revoked.protocol_message.get_or_insert_default();
    protocol.r#type = Some(wa::message::protocol_message::Type::Revoke);
    let key = protocol.key.get_or_insert_default();
    key.remote_jid = Some(crate::archive::tests::PN.into());
    key.from_me = Some(false);
    key.id = Some("synthetic-envelope".into());
    let mut revoked = tests::mapped(&revoked, false);
    revoked.id = "synthetic-live-delete".into();
    revoked.timestamp += 1;
    db.ingest(Batch {
        messages: vec![revoked],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(!db.document_preview_current(&request).await.unwrap());
    assert!(
        !db.save_document_preview(&request, Some(vec![1, 2, 3]))
            .await
            .unwrap()
    );
    repair_text(&db).unwrap();
    assert!(
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .messages
            .last()
            .unwrap()
            .content
            .location
            .is_none()
    );
}
