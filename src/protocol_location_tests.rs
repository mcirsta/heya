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
fn invalid_and_live_locations_do_not_become_fixed_map_requests() {
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
    assert!(tests::mapped(&message, false).content.location.is_none());
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
