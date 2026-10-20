use super::*;
use crate::{
    archive::tests::{KEY, PN},
    protocol_business::{self, tests::template},
    storage::{prepare_at, tests::MemoryVault},
};
use whatsapp_rust::{prelude::MessageBuilderExt, waproto::buffa::MessageField};

#[tokio::test]
async fn live_and_history_share_business_text_and_bounded_sidebar_summary() {
    let (_temp, archive) = crate::archive::tests::fixture();
    let body = format!(
        "Synthetic delivery\n{}\nFinal line",
        "👩🏽‍💻 文字\n".repeat(1000)
    );
    let message = template(&body);
    let live = super::tests::mapped(&message, false);
    let content = live.content.clone();
    let expected = protocol_business::text(&message).unwrap();
    assert_eq!(live.text, expected);
    assert_eq!(live.raw, codec::message_to_vec(&message));
    archive
        .ingest(Batch {
            messages: vec![live],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();

    let mut key = wa::MessageKey::default();
    key.remote_jid = Some(PN.into());
    key.id = Some("history-business".into());
    key.from_me = Some(false);
    let mut info = wa::WebMessageInfo::default();
    info.key = MessageField::some(key);
    info.message = MessageField::some(message);
    info.message_timestamp = Some(1_700_000_000);
    let mut item = wa::HistorySyncMsg::default();
    item.message = MessageField::some(info);
    let mut conversation = wa::Conversation::default();
    conversation.id = PN.into();
    conversation.messages = vec![item];
    let mut history = wa::HistorySync::default();
    history.conversations = vec![conversation];
    archive
        .capture_history("business".into(), super::tests::compressed(&history))
        .await
        .unwrap();
    import_pending(&archive, &AtomicBool::new(false)).unwrap();
    let chat = archive.create_chat(PN.into()).await.unwrap();
    let view = archive
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    let conversation = view.conversation.unwrap();
    assert_eq!(conversation.messages.len(), 2);
    for message in conversation.messages.iter() {
        assert_eq!(message.text, expected);
        crate::archive::tests::assert_content_payload(&message.content, &content);
        assert_eq!(message.content.body.as_deref(), Some(body.as_str()));
    }
    assert!(!conversation.chat.preview.contains('\n'));
    assert!(conversation.chat.preview.chars().count() <= crate::domain::SUMMARY_CHARS + 1);
}

#[tokio::test]
async fn archive_repair_is_restartable_preserves_state_and_never_resurrects_content() {
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let archive = Archive::open(files.clone(), KEY).unwrap();
    let message = template("Synthetic archived delivery\nSecond line");
    let expected = protocol_business::text(&message).unwrap();
    let old = |id: &str| {
        let mut record = super::tests::mapped(&message, false);
        record.id = id.into();
        record.text = "[Unsupported message]".into();
        record
    };
    let mut messages: Vec<_> = (0..205).map(|i| old(&format!("old-{i}"))).collect();
    messages.push(old("edited"));
    messages.push(old("revoked"));
    let mut expired = old("expired");
    expired.expires = Some(1);
    messages.push(expired);
    let mut hidden = old("hidden");
    hidden.visible = false;
    messages.push(hidden);
    let mut corrupt = old("corrupt");
    corrupt.raw = vec![0xff];
    messages.push(corrupt);
    let mut literal = old("literal");
    literal.raw = codec::message_to_vec(&wa::Message::text("[Unsupported message]"));
    messages.push(literal);
    let mut once = old("once");
    let mut wrapper = wa::message::FutureProofMessage::default();
    wrapper.message = MessageField::some(message.clone());
    let mut once_message = wa::Message::default();
    once_message.view_once_message = MessageField::some(wrapper);
    once.raw = codec::message_to_vec(&once_message);
    messages.push(once);
    for (id, body) in [
        ("edited", Some("[Unsupported message]".into())),
        ("revoked", None),
    ] {
        let mut mutation = old(&format!("change-{id}"));
        mutation.visible = false;
        mutation.raw.clear();
        mutation.change = Some(Change {
            id: id.into(),
            sender: PN.into(),
            text: body,
            version: 42,
            raw: Vec::new(),
        });
        messages.push(mutation);
    }
    archive
        .ingest(Batch {
            messages,
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let chat = archive.create_chat(PN.into()).await.unwrap();
    archive.draft(chat, "Keep my draft".into()).await.unwrap();
    let unchanged = |archive: &Archive| {
        archive.blocking(|db| {
        Ok(db.prepare("SELECT id,stamp,seen,unread,delivery,length(raw) FROM messages ORDER BY id")?
            .query_map([], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,bool>(2)?,r.get::<_,bool>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?)))?
            .collect::<std::result::Result<Vec<_>,_>>()?)
    }).unwrap()
    };
    let before = unchanged(&archive);
    archive.blocking(|db| {
        db.execute_batch("CREATE TRIGGER fail_repair BEFORE UPDATE OF body ON messages WHEN OLD.wire_id='old-150' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;")?;
        Ok(())
    }).unwrap();
    assert_eq!(repair_text(&archive), Err(archive::Error::Database));
    archive
        .blocking(|db| {
            let changed: i64 = db.query_row(
                "SELECT count(*) FROM messages WHERE body=?1",
                [&expected],
                |r| r.get(0),
            )?;
            assert_eq!(changed, 100, "only the first batch committed");
            db.execute_batch("DROP TRIGGER fail_repair")?;
            Ok(())
        })
        .unwrap();
    drop(archive);
    let archive = Archive::open(files.clone(), KEY).unwrap();
    repair_text(&archive).unwrap();
    assert_eq!(unchanged(&archive), before);
    archive
        .blocking(|db| {
            let changed: i64 = db.query_row(
                "SELECT count(*) FROM messages WHERE body=?1",
                [&expected],
                |r| r.get(0),
            )?;
            assert_eq!(changed, 205);
            for id in ["edited", "hidden", "corrupt", "literal", "once"] {
                let text: String =
                    db.query_row("SELECT body FROM messages WHERE wire_id=?1", [id], |r| {
                        r.get(0)
                    })?;
                assert_eq!(text, "[Unsupported message]", "{id}");
            }
            for (id, expected) in [
                ("revoked", "This message was deleted"),
                ("expired", "[Disappearing message expired]"),
            ] {
                let (text, raw): (String, Vec<u8>) = db.query_row(
                    "SELECT body,raw FROM messages WHERE wire_id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                assert_eq!(text, expected);
                assert!(raw.is_empty());
            }
            Ok(())
        })
        .unwrap();
    let view = archive
        .view(chat, 10, String::new(), false, false)
        .await
        .unwrap();
    assert_eq!(view.conversation.unwrap().draft, "Keep my draft");
    drop(archive);
    let archive = Archive::open(files, KEY).unwrap();
    archive
        .repair_unsupported_text(1, |_| panic!("completed repair must not rescan"))
        .unwrap();
}
