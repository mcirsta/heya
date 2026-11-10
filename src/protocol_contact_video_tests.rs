//! Synthetic received contacts/PTV through the real adapter and archive.
use super::*;
use crate::archive::tests::{PN, fixture};
use whatsapp_rust::waproto::buffa::MessageField;
const VCARD: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Sample;Alex;;;\r\nFN:Alex Sample\r\nTEL;TYPE=CELL;waid=15550000001:+1 555 000 0001\r\nEMAIL:alex@example.invalid\r\nADR:;;42 Example\\; Lane;Sample Town;;;\r\nNOTE:First line\\nSecond 👩🏽‍💻\r\nEND:VCARD";
fn contact() -> wa::Message {
    let mut m = wa::Message::default();
    let c = m.contact_message.get_or_insert_default();
    c.vcard = Some(VCARD.into());
    c.context_info.get_or_insert_default().is_forwarded = Some(true);
    m
}
fn ptv() -> wa::Message {
    let mut m = wa::Message::default();
    let v = m.ptv_message.get_or_insert_default();
    v.width = Some(720);
    v.height = Some(1280);
    v.seconds = Some(12);
    v.mimetype = Some("video/mp4".into());
    v.jpeg_thumbnail = Some(vec![1, 2, 3]);
    v.media_key = Some(vec![7; 32]);
    v.file_sha256 = Some(vec![8; 32]);
    v.file_enc_sha256 = Some(vec![9; 32]);
    v.file_length = Some(1024);
    v.direct_path = Some("/synthetic-ptv".into());
    m
}
#[test]
fn contacts_decode_names_fields_context_and_wrappers_without_changing_original_cards() {
    let m = contact();
    let row = tests::mapped(&m, false);
    let c = &row.content.contacts[0];
    assert_eq!(c.name, "Alex Sample");
    assert_eq!(c.account(), "15550000001");
    assert!(c.details().contains("42 Example; Lane, Sample Town"));
    assert!(c.details().contains("Second 👩🏽‍💻"));
    assert!(row.content.forwarded && row.content.has_attachment);
    assert_eq!(row.raw, codec::message_to_vec(&m));
    let bytes = crate::protocol_contacts::file(&row.raw).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        format!("{}\r\n", VCARD.trim())
    );
    let mut wrapped = wa::Message::default();
    wrapped.ephemeral_message.get_or_insert_default().message = MessageField::some(m.clone());
    assert_eq!(tests::mapped(&wrapped, false).content, row.content);
    let mut bundle = wa::Message::default();
    bundle
        .contacts_array_message
        .get_or_insert_default()
        .contacts = vec![m.contact_message.into_option().unwrap(); 3];
    assert_eq!(tests::mapped(&bundle, false).content.contacts.len(), 3);
    assert_eq!(
        crate::protocol_contacts::file(&codec::message_to_vec(&bundle))
            .unwrap()
            .windows(11)
            .filter(|w| *w == b"BEGIN:VCARD")
            .count(),
        3
    );
    wrapped.ephemeral_message = Default::default();
    wrapped.view_once_message.get_or_insert_default().message = MessageField::some(bundle);
    assert!(tests::mapped(&wrapped, false).content.contacts.is_empty());
    assert!(crate::protocol_contacts::file(&codec::message_to_vec(&wrapped)).is_none());
}
#[test]
fn vcard_parsing_handles_folded_unicode_quoted_printable_and_malformed_input() {
    let raw = "BEGIN:VCARD\nVERSION:2.1\nN:Family;Given;;;\nFN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:M=C3=AEra=\n Sample\nNOTE:Long\n  continuation\\, with comma\nPHOTO;VALUE=URI:https://example.invalid/private-image\nURL:javascript:bad\nEND:VCARD";
    let c = crate::shared_contacts::parse("", raw);
    assert_eq!(c.name, "Mîra Sample");
    assert!(c.details().contains("Long continuation, with comma"));
    assert!(c.phones.is_empty());
    assert!(!c.fields.iter().any(|f| f.label == "Photo"));
    assert_eq!(
        crate::shared_contacts::parse("Sender label", "broken").name,
        "Sender label"
    );
    assert_eq!(crate::shared_contacts::parse("", "broken").name, "Contact");
}
#[test]
fn round_videos_keep_the_video_transport_and_exclude_view_once_bytes() {
    let m = ptv();
    let row = tests::mapped(&m, false);
    let v = row.content.video.as_ref().unwrap();
    assert!(v.round && row.content.has_attachment);
    assert!(row.content.photo.is_none());
    assert_eq!(row.thumbnail, vec![1, 2, 3]);
    assert!(
        crate::protocol_downloads::info(&row.raw).unwrap().kind == crate::downloads::Kind::Video
    );
    let old: crate::message_content::Video =
        serde_json::from_str(r#"{"width":720,"height":1280,"seconds":12,"size":1024,"gif":false}"#)
            .unwrap();
    assert!(!old.round);
    let mut once = m;
    once.ptv_message.get_or_insert_default().view_once = Some(true);
    assert!(tests::mapped(&once, false).content.video.is_none());
    assert!(tests::mapped(&once, false).raw.is_empty());
    assert!(crate::protocol_downloads::info(&codec::message_to_vec(&once)).is_err());
}
#[tokio::test]
async fn old_cards_repair_in_place_offline_vcards_export_and_removed_media_stays_removed() {
    let (_dir, db) = fixture();
    let mut rows = vec![];
    for (id, m) in [
        ("contact", contact()),
        ("round", ptv()),
        ("deleted", contact()),
    ] {
        let mut r = tests::mapped(&m, false);
        r.id = id.into();
        r.text = "[Unsupported message]".into();
        r.content = Default::default();
        rows.push(r);
    }
    db.ingest(Batch {
        messages: rows,
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    db.blocking(|c| {
        c.execute(
            "UPDATE messages SET deleted=1,raw=X'' WHERE wire_id='deleted'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    repair_text(&db).unwrap();
    repair_text(&db).unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.messages[0].content.contacts[0].name, "Alex Sample");
    assert!(view.messages[1].content.video.as_ref().unwrap().round);
    let req = db
        .download_request(chat, view.messages[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        db.download_cached(&req, crate::downloads::Kind::Contact)
            .await
            .unwrap()
            .unwrap(),
        crate::protocol_contacts::file(&req.raw).unwrap()
    );
    let video = db
        .video_request(view.messages[1].id)
        .await
        .unwrap()
        .unwrap();
    db.queue_video_played_receipt(&video).await.unwrap();
    db.queue_video_played_receipt(&video).await.unwrap();
    let receipt = db.next_receipt().await.unwrap().unwrap();
    assert!(receipt.kind == crate::archive::ReceiptKind::Played);
    assert_eq!(receipt.messages.len(), 1);
    db.hide_remote(PN.into(), PN.into(), "round".into(), false)
        .await
        .unwrap();
    assert!(db.video_request(video.id).await.unwrap().is_none());
    db.hide_remote(PN.into(), PN.into(), "contact".into(), false)
        .await
        .unwrap();
    assert!(
        db.download_cached(&req, crate::downloads::Kind::Contact)
            .await
            .unwrap()
            .is_none()
    );
}
