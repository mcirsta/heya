//! Synthetic phone replies through the real captured-chunk importer.
use super::*;
use crate::{
    archive::tests::{LID, PN, fixture, record as saved},
    phone_history::{self, Anchor, Failure, Source},
};
use tokio::sync::mpsc;
use whatsapp_rust::{prelude::MessageBuilderExt, waproto::buffa::MessageField};
struct Fake(mpsc::UnboundedSender<Anchor>);
#[async_trait::async_trait]
impl Source for Fake {
    async fn fetch(&self, a: &Anchor) -> Result<String, Failure> {
        self.0.send(a.clone()).unwrap();
        Ok("REQUEST-1".into())
    }
}
fn response(jid: &str, more: bool) -> Vec<u8> {
    response_at(jid, more, 1_600_000_000)
}
fn response_at(jid: &str, more: bool, stamp: u64) -> Vec<u8> {
    let mut key = wa::MessageKey::default();
    key.id = Some("older-message".into());
    key.from_me = Some(false);
    let mut info = wa::WebMessageInfo::default();
    info.key = MessageField::some(key);
    info.message = MessageField::some(wa::Message::text("Synthetic older message 👩🏽‍💻"));
    info.message_timestamp = Some(stamp);
    let mut item = wa::HistorySyncMsg::default();
    item.message = MessageField::some(info);
    let mut conversation = wa::Conversation::default();
    conversation.id = jid.into();
    conversation.messages = vec![item];
    use wa::conversation::EndOfHistoryTransferType as End;
    conversation.end_of_history_transfer_type = Some(if more {
        End::COMPLETE_ON_DEMAND_SYNC_BUT_MORE_MSG_REMAIN_ON_PRIMARY
    } else {
        End::COMPLETE_AND_NO_MORE_MESSAGE_REMAIN_ON_PRIMARY
    });
    let mut history = wa::HistorySync::default();
    history.sync_type = wa::history_sync::HistorySyncType::ON_DEMAND;
    history.conversations = vec![conversation];
    super::tests::compressed(&history)
}
#[tokio::test]
async fn phone_response_correlates_after_import_and_metadata_even_when_their_order_is_reversed() {
    for metadata_first in [true, false] {
        let (_temp, db) = fixture();
        db.ingest(Batch {
            messages: vec![saved("anchor")],
            ..Default::default()
        })
        .await
        .unwrap();
        let anchor = db.phone_history_anchor(1).await.unwrap().unwrap();
        let (sent, mut requests) = mpsc::unbounded_channel();
        let (_sources, rx) = watch::channel(Some(Arc::new(Fake(sent)) as Arc<dyn Source>));
        let job = tokio::spawn(phone_history::request(db.clone(), rx, anchor, 9, 2));
        requests.recv().await.unwrap();
        let data = response(PN, false);
        let chunk = crate::protocol_history::chunk_id(&data);
        db.capture_history(chunk.clone(), data.clone())
            .await
            .unwrap();
        if metadata_first {
            db.phone_history_metadata(chunk.clone(), "REQUEST-1".into())
                .await
                .unwrap();
        }
        assert!(
            !job.is_finished(),
            "capturing bytes is not completed history"
        );
        let copy = db.clone();
        let bytes = data.clone();
        let id = chunk.clone();
        tokio::task::spawn_blocking(move || {
            import_chunk(&copy, &id, &bytes, 0, &AtomicBool::new(false))
        })
        .await
        .unwrap()
        .unwrap();
        if !metadata_first {
            assert!(!job.is_finished());
            db.phone_history_metadata(chunk.clone(), "REQUEST-1".into())
                .await
                .unwrap();
        }
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), job)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.result, Ok(true));
        assert_eq!((outcome.request, outcome.revision), (9, 2));
        let view = db.view(1, 100, String::new(), false, false).await.unwrap();
        assert!(!view.phone_more);
        assert_eq!(view.conversation.unwrap().messages.len(), 2);
        // A completed/replayed response does not add a duplicate or change the anchor.
        import_chunk(&db, &chunk, &data, 1, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            db.phone_history_anchor(1).await.unwrap().unwrap().wire,
            "older-message"
        );
        db.ingest(Batch {
            aliases: vec![(LID.into(), PN.into())],
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(
            db.phone_history_answer("REQUEST-1".into(), 1)
                .await
                .unwrap()
        );
        db.phone_history_metadata(chunk.clone(), "REQUEST-2".into())
            .await
            .unwrap();
        assert!(
            db.phone_history_answer("REQUEST-2".into(), 1)
                .await
                .unwrap()
        );
        assert!(
            db.phone_history_answer("REQUEST-1".into(), 1)
                .await
                .unwrap()
        );
    }
}
#[tokio::test]
async fn phone_anchors_exclude_unconfirmed_outgoing_and_exhaustion_survives_aliases_and_reopen() {
    use crate::archive::tests::KEY;
    use crate::storage::{prepare_at, tests::MemoryVault};
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    let mut outgoing = saved("old-unconfirmed");
    outgoing.outgoing = true;
    outgoing.timestamp = 1;
    let mut lid = saved("lid-anchor");
    lid.chat = LID.into();
    lid.sender = LID.into();
    db.ingest(Batch {
        messages: vec![outgoing, saved("pn-anchor"), lid],
        ..Default::default()
    })
    .await
    .unwrap();
    db.blocking(|c| {
        c.execute(
            "UPDATE messages SET delivery='uncertain' WHERE wire_id='old-unconfirmed'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let anchor = db.phone_history_anchor(1).await.unwrap().unwrap();
    assert_eq!(anchor.wire, "pn-anchor");
    assert_eq!(anchor.stamp, 1_700_000_000);
    let data = response(LID, false);
    db.capture_history("synthetic".into(), data.clone())
        .await
        .unwrap();
    import_chunk(&db, "synthetic", &data, 0, &AtomicBool::new(false)).unwrap();
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(
        !db.view(1, 100, String::new(), false, false)
            .await
            .unwrap()
            .phone_more
    );
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert!(
        !db.view(1, 100, String::new(), false, false)
            .await
            .unwrap()
            .phone_more
    );
}

#[tokio::test]
async fn same_second_phone_history_advances_but_newer_live_arrivals_do_not() {
    let (_temp, db) = fixture();
    db.ingest(Batch {
        messages: vec![saved("anchor")],
        ..Default::default()
    })
    .await
    .unwrap();
    let anchor = db.phone_history_anchor(1).await.unwrap().unwrap();
    let mut live = saved("new-live-message");
    live.timestamp += 1;
    db.ingest(Batch {
        messages: vec![live],
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(!db.phone_history_advanced(anchor.clone()).await.unwrap());
    let data = response_at(PN, true, anchor.stamp as u64);
    db.capture_history("same-second".into(), data.clone())
        .await
        .unwrap();
    import_chunk(&db, "same-second", &data, 0, &AtomicBool::new(false)).unwrap();
    assert!(db.phone_history_advanced(anchor).await.unwrap());
    let anchor = db.phone_history_anchor(1).await.unwrap().unwrap();
    import_chunk(&db, "same-second", &data, 1, &AtomicBool::new(false)).unwrap();
    assert!(!db.phone_history_advanced(anchor).await.unwrap());
}

#[tokio::test]
async fn schema_thirteen_upgrade_preserves_chats_and_drafts_and_expires_only_due_messages() {
    use crate::{
        archive::tests::KEY,
        storage::{prepare_at, tests::MemoryVault},
    };
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let db = Archive::open(files.clone(), KEY).unwrap();
    let mut due = saved("due");
    due.expires = Some(chrono::Utc::now().timestamp() + 3600);
    db.ingest(Batch {
        messages: vec![saved("kept"), due],
        ..Default::default()
    })
    .await
    .unwrap();
    db.draft(1, "Preserve synthetic draft 👩🏽‍💻".into())
        .await
        .unwrap();
    db.blocking(|c|{c.execute_batch("DROP TABLE history_pages;DROP TABLE history_requests;DROP INDEX messages_expiring;DROP INDEX messages_visible_timeline;DROP TABLE outgoing_receipts; PRAGMA user_version=13;UPDATE messages SET expires=1 WHERE wire_id='due';")?;Ok(())}).unwrap();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    let view = db.view(1, 100, String::new(), false, false).await.unwrap();
    assert!(view.phone_more);
    let conversation = view.conversation.unwrap();
    assert_eq!(conversation.draft, "Preserve synthetic draft 👩🏽‍💻");
    assert_eq!(conversation.messages.len(), 2);
    assert!(
        conversation
            .messages
            .iter()
            .any(|m| m.text == "Synthetic kept")
    );
    assert!(
        conversation
            .messages
            .iter()
            .any(|m| m.text == "[Disappearing message expired]")
    );
}
