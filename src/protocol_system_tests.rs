//! Synthetic controls through the same adapter, archive and replay paths.
use super::*;
use crate::archive::tests::{KEY, LID, PN, fixture};
use whatsapp_rust::{prelude::MessageBuilderExt, waproto::buffa::MessageField};
const OWN: &str = "15550000009@s.whatsapp.net";
const GROUP: &str = "15550000000-123@g.us";
fn row(id: &str, message: &wa::Message) -> Record {
    record(
        &PN.parse().unwrap(),
        &PN.parse().unwrap(),
        id.into(),
        1_700_000_005,
        false,
        "Synthetic".into(),
        message,
        None,
    )
}
async fn ingest(db: &Archive, rows: Vec<Record>) {
    db.ingest(Batch {
        messages: rows,
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
}
fn edit(encrypted: bool) -> wa::Message {
    let mut key = wa::MessageKey::default();
    key.id = Some("original".into());
    key.remote_jid = Some(OWN.into());
    key.from_me = Some(true);
    let mut message = wa::Message::default();
    let p = message.protocol_message.get_or_insert_default();
    p.key = MessageField::some(key.clone());
    p.r#type = Some(wa::message::protocol_message::Type::MESSAGE_EDIT);
    p.timestamp_ms = Some(1_700_000_005_000);
    p.edited_message = MessageField::some(wa::Message::text("Edited 👩🏽‍💻\nこんにちは"));
    if !encrypted {
        return message;
    }
    let context = whatsapp_rust::wacore::message_edit::MessageEditContext {
        original_msg_id: "original",
        original_sender_jid: PN,
        editor_jid: PN,
    };
    let (payload, iv) =
        whatsapp_rust::wacore::message_edit::encrypt_message_edit(&message, &[7; 32], &context)
            .unwrap();
    let mut wrapped = wa::Message::default();
    let e = wrapped.secret_encrypted_message.get_or_insert_default();
    e.target_message_key = MessageField::some(key);
    e.secret_enc_type = Some(wa::message::secret_encrypted_message::SecretEncType::MESSAGE_EDIT);
    e.enc_iv = Some(iv.to_vec());
    e.enc_payload = Some(payload);
    wrapped
}
fn original() -> Record {
    let mut message = wa::Message::text("Original");
    message
        .message_context_info
        .get_or_insert_default()
        .message_secret = Some(vec![7; 32]);
    let mut r = row("original", &message);
    r.timestamp -= 5;
    r
}

#[test]
fn encrypted_edits_try_the_authors_lid_and_keep_the_original_secret() {
    let context = whatsapp_rust::wacore::message_edit::MessageEditContext {
        original_msg_id: "original",
        original_sender_jid: LID,
        editor_jid: LID,
    };
    let mut inner = edit(false);
    inner
        .protocol_message
        .get_or_insert_default()
        .edited_message
        .get_or_insert_default()
        .message_context_info
        .get_or_insert_default()
        .message_secret = Some(vec![9; 32]);
    let (payload, iv) =
        whatsapp_rust::wacore::message_edit::encrypt_message_edit(&inner, &[7; 32], &context)
            .unwrap();
    let mut envelope = edit(true);
    let secret = envelope.secret_encrypted_message.get_or_insert_default();
    secret.enc_payload = Some(payload);
    secret.enc_iv = Some(iv.to_vec());
    let encoded = codec::message_to_vec(&envelope);
    assert!(
        crate::protocol_edits::decrypt(&original().raw, &encoded, "original", PN, None).is_none()
    );
    let decrypted =
        crate::protocol_edits::decrypt(&original().raw, &encoded, "original", PN, Some(LID))
            .unwrap();
    let applied = crate::protocol_edits::apply(&original().raw, &decrypted, "").unwrap();
    assert_eq!(
        codec::message_decode(&applied.message.raw)
            .unwrap()
            .message_context_info
            .message_secret
            .as_deref(),
        Some([7; 32].as_slice())
    );
}
fn state(db: &Archive) -> (i64, i64, String) {
    db.blocking(|c|Ok(c.query_row("SELECT count(*),coalesce(sum(unread),0),coalesce(group_concat(body),'') FROM messages WHERE visible=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)).unwrap()
}

#[tokio::test]
async fn early_edits_survive_reopen_replay_and_aliases_without_a_second_bubble() {
    for encrypted in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let files =
            crate::storage::prepare_at(temp.path(), &crate::storage::tests::MemoryVault::default())
                .unwrap();
        let db = Archive::open(files.clone(), KEY).unwrap();
        db.bind_account(OWN.into(), "Owner".into()).await.unwrap();
        let mut event = row("edit", &edit(encrypted));
        // Incoming target.fromMe is sender-relative, not our own-account flag.
        assert!(!event.visible && event.change.is_some());
        event.sender = LID.into();
        event.change.as_mut().unwrap().sender = LID.into();
        db.ingest(Batch {
            aliases: vec![(LID.into(), PN.into())],
            messages: vec![event.clone()],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(state(&db).0, 0);
        assert_eq!(state(&db).1, 0);
        drop(db);
        let db = Archive::open(files, KEY).unwrap();
        ingest(&db, vec![original(), event.clone(), original(), event]).await;
        assert_eq!(state(&db), (1, 1, "Edited 👩🏽‍💻\nこんにちは".into()));
        let pending=db.blocking(|c|Ok(c.query_row("SELECT count(*) FROM outgoing_receipts r JOIN messages m ON r.message_id=m.id WHERE m.visible=0",[],|r|r.get::<_,i64>(0))?)).unwrap();
        assert_eq!(pending, 0);
        // Text edits retain the secret needed by a subsequent encrypted edit.
        let raw = db
            .blocking(|c| {
                Ok(c.query_row(
                    "SELECT raw FROM messages WHERE wire_id='original'",
                    [],
                    |r| r.get::<_, Vec<u8>>(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            codec::message_decode(&raw)
                .unwrap()
                .message_context_info
                .message_secret
                .as_deref(),
            Some([7; 32].as_slice())
        );
    }
}

#[tokio::test]
async fn encrypted_edits_authenticate_and_revocation_wins() {
    let (_temp, db) = fixture();
    ingest(&db, vec![original()]).await;
    let mut forged = edit(true);
    forged
        .secret_encrypted_message
        .get_or_insert_default()
        .enc_payload
        .as_mut()
        .unwrap()[0] ^= 1;
    ingest(&db, vec![row("bad-edit", &forged)]).await;
    assert_eq!(state(&db).2, "Original");
    let mut deletion = wa::Message::default();
    let p = deletion.protocol_message.get_or_insert_default();
    p.r#type = Some(wa::message::protocol_message::Type::REVOKE);
    p.key = edit(false).protocol_message.key.clone();
    ingest(
        &db,
        vec![row("delete", &deletion), row("late-edit", &edit(true))],
    )
    .await;
    assert_eq!(state(&db).2, crate::domain::DELETED_MESSAGE);
}

fn history(receivers: &[&str]) -> wa::Message {
    let mut m = wa::Message::default();
    let meta = m
        .message_history_notice
        .get_or_insert_default()
        .message_history_metadata
        .get_or_insert_default();
    meta.history_receivers = receivers.iter().map(|s| (*s).into()).collect();
    meta.oldest_message_timestamp_in_window = Some(1_600_000_000);
    m
}
#[tokio::test]
async fn history_notices_honor_sender_and_receivers_including_aliases() {
    let (_temp, db) = fixture();
    db.bind_account(OWN.into(), "Owner".into()).await.unwrap();
    db.ingest(Batch {
        aliases: vec![(LID.into(), OWN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    for (id, m, outgoing) in [
        ("other", history(&[PN]), false),
        ("me", history(&[OWN]), false),
        ("alias", history(&[LID]), false),
        ("sent", history(&[PN]), true),
        (
            "missing",
            {
                let mut m = wa::Message::default();
                m.message_history_notice = MessageField::some(Default::default());
                m
            },
            false,
        ),
    ] {
        let mut r = row(id, &m);
        r.chat = GROUP.into();
        r.outgoing = outgoing;
        ingest(&db, vec![r]).await;
    }
    let chat = db.create_chat(GROUP.into()).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.messages.len(), 1);
    assert_eq!(
        view.messages[0].content.notice,
        Some(crate::message_content::Notice::HistoryShared)
    );
    assert!(view.chat.preview.ends_with("shared message history"));
    assert!(db.set_reply(chat, view.messages[0].id).await.is_err());
}

#[tokio::test]
async fn startup_repairs_saved_controls_and_keeps_unavailable_phone_content_explanatory() {
    let (_temp, db) = fixture();
    db.bind_account(OWN.into(), "Owner".into()).await.unwrap();
    let mut code = wa::Message::default();
    code.placeholder_message.get_or_insert_default().r#type =
        Some(wa::message::placeholder_message::PlaceholderType::MASK_LINKED_DEVICES);
    let mut vote = wa::Message::default();
    vote.poll_update_message = MessageField::some(Default::default());
    let mut rows = Vec::new();
    for (id, m) in [
        ("legacy", edit(false)),
        ("encrypted", edit(true)),
        ("vote", vote),
        ("code", code.clone()),
        ("history", history(&[OWN])),
    ] {
        let mut r = row(id, &m);
        if id == "history" {
            r.chat = GROUP.into();
        }
        r.visible = true;
        r.change = None;
        r.raw = codec::message_to_vec(&m);
        r.text = "[Unsupported message]".into();
        r.content = Default::default();
        rows.push(r);
    }
    ingest(&db, rows).await;
    repair_text(&db).unwrap();
    repair_text(&db).unwrap();
    assert_eq!(state(&db), (1, 1, crate::protocol_system::CODE.into()));
    ingest(&db, vec![original()]).await;
    assert_eq!(state(&db).0, 2);
    assert!(state(&db).2.contains("Edited"));
    let r = row("new-code", &code);
    assert_eq!(r.text, crate::protocol_system::CODE);
    assert!(r.visible);
    let literal = row("literal", &wa::Message::text("[Unsupported message]"));
    ingest(&db, vec![literal]).await;
    repair_text(&db).unwrap();
    assert!(state(&db).2.contains("[Unsupported message]"));
}
