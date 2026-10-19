use super::*;
use crate::archive::tests::{KEY, LID, PN, fixture, record};

async fn add(db: &Archive, rows: Vec<Record>) -> domain::Conversation {
    db.ingest(Batch {
        messages: rows,
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db
        .view(0, 1000, String::new(), false, false)
        .await
        .unwrap()
        .chats[0]
        .id;
    db.view(chat, 1000, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
}

#[tokio::test]
async fn visible_boundary_is_durable_deduplicated_and_does_not_read_newer_arrivals() {
    let (_temp, db) = fixture();
    let view = add(&db, vec![record("first"), record("newer")]).await;
    let first = view.messages[0].id;
    assert!(
        db.next_receipt().await.unwrap().is_none(),
        "ingestion is not a visible read"
    );
    db.mark_visible_read_with_receipts(view.chat.id, first, true)
        .await
        .unwrap();
    let files = db.0._files.clone();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    let job = db.next_receipt().await.unwrap().unwrap();
    assert!(job.kind == ReceiptKind::Read);
    assert_eq!(job.messages.len(), 1);
    assert_eq!(job.messages[0].1, "first");
    db.finish_receipt(job).await.unwrap();
    db.mark_visible_read_with_receipts(view.chat.id, first, true)
        .await
        .unwrap();
    assert!(db.next_receipt().await.unwrap().is_none());
    let current = db
        .view(view.chat.id, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(current.chat.unread, 1);
    db.mark_visible_read_with_receipts(view.chat.id, view.messages[1].id, true)
        .await
        .unwrap();
    assert_eq!(
        db.next_receipt().await.unwrap().unwrap().messages[0].1,
        "newer"
    );
}

#[tokio::test]
async fn local_privacy_preserves_private_sync_without_backfilling_receipts() {
    let (_temp, db) = fixture();
    let view = add(&db, vec![record("private")]).await;
    db.mark_visible_read_with_receipts(view.chat.id, view.messages[0].id, false)
        .await
        .unwrap();
    assert!(db.next_read_sync().await.unwrap().is_some());
    assert!(db.next_receipt().await.unwrap().is_none());
    db.mark_visible_read_with_receipts(view.chat.id, view.messages[0].id, true)
        .await
        .unwrap();
    assert!(
        db.next_receipt().await.unwrap().is_none(),
        "enabling receipts cannot backfill private reads"
    );
}

#[tokio::test]
async fn batches_are_bounded_and_hidden_deleted_and_expired_messages_are_not_sent() {
    let (_temp, db) = fixture();
    let rows = (0..300).map(|n| record(&format!("msg-{n}"))).collect();
    let view = add(&db, rows).await;
    db.mark_visible_read_with_receipts(view.chat.id, view.messages.last().unwrap().id, true)
        .await
        .unwrap();
    let first = db.next_receipt().await.unwrap().unwrap();
    assert_eq!(first.messages.len(), 256);
    db.finish_receipt(first).await.unwrap();
    let next = db.next_receipt().await.unwrap().unwrap();
    assert_eq!(next.messages.len(), 44);
    db.blocking(|c| {
        c.execute("UPDATE messages SET visible=0 WHERE wire_id='msg-299'", [])?;
        c.execute("UPDATE messages SET deleted=1 WHERE wire_id='msg-298'", [])?;
        c.execute("UPDATE messages SET expires=1 WHERE wire_id='msg-297'", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(db.next_receipt().await.unwrap().unwrap().messages.len(), 41);
    db.discard_receipts().await.unwrap();
    assert!(db.next_receipt().await.unwrap().is_none());
}

#[tokio::test]
async fn alias_merge_preserves_pending_intent_and_ignores_stale_completion() {
    let (_temp, db) = fixture();
    let mut lid = record("same");
    lid.chat = LID.into();
    lid.sender = LID.into();
    let lid_view = add(&db, vec![lid]).await;
    db.mark_visible_read_with_receipts(lid_view.chat.id, lid_view.messages[0].id, true)
        .await
        .unwrap();
    let stale = db.next_receipt().await.unwrap().unwrap();
    db.ingest(Batch {
        messages: vec![record("same")],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    db.finish_receipt(stale).await.unwrap();
    let job = db.next_receipt().await.unwrap().unwrap();
    assert_eq!(job.jid, PN);
    assert_eq!(job.messages.len(), 1);
    db.finish_receipt(job).await.unwrap();
    assert!(db.next_receipt().await.unwrap().is_none());
}

#[tokio::test]
async fn played_intent_requires_the_current_incoming_voice_envelope() {
    let (_temp, db) = fixture();
    let mut voice = record("voice");
    voice.content.audio = Some(crate::message_content::Audio {
        seconds: Some(3),
        voice: true,
        size: None,
        waveform: vec![],
    });
    let mut own = voice.clone();
    own.id = "own".into();
    own.outgoing = true;
    let mut music = voice.clone();
    music.id = "music".into();
    music.content.audio.as_mut().unwrap().voice = false;
    let view = add(&db, vec![voice, own, music]).await;
    for message in view.messages.iter() {
        let request = db.audio_request(message.id).await.unwrap().unwrap();
        db.queue_played_receipt(&request).await.unwrap();
    }
    let job = db.next_receipt().await.unwrap().unwrap();
    assert!(job.kind == ReceiptKind::Played);
    assert_eq!(job.messages.len(), 1);
    assert_eq!(job.messages[0].1, "voice");
    db.finish_receipt(job).await.unwrap();
    assert!(db.next_receipt().await.unwrap().is_none());
    let request = db
        .audio_request(view.messages[0].id)
        .await
        .unwrap()
        .unwrap();
    db.blocking(|c| {
        c.execute("UPDATE messages SET raw=X'09' WHERE wire_id='voice'", [])?;
        Ok(())
    })
    .unwrap();
    db.queue_played_receipt(&request).await.unwrap();
    assert!(db.next_receipt().await.unwrap().is_none());
}

#[tokio::test]
async fn group_batches_preserve_each_original_participant() {
    let (_temp, db) = fixture();
    let mut a = record("group-a");
    a.chat = "123456789-123@g.us".into();
    let mut b = a.clone();
    b.id = "group-b".into();
    b.sender = "15550000002@s.whatsapp.net".into();
    let view = add(&db, vec![a, b]).await;
    db.mark_visible_read_with_receipts(view.chat.id, view.messages.last().unwrap().id, true)
        .await
        .unwrap();
    let first = db.next_receipt().await.unwrap().unwrap();
    assert_eq!(first.jid, "123456789-123@g.us");
    assert_eq!(first.sender, PN);
    assert_eq!(first.messages.len(), 1);
    db.finish_receipt(first).await.unwrap();
    let second = db.next_receipt().await.unwrap().unwrap();
    assert_eq!(second.sender, "15550000002@s.whatsapp.net");
    assert_eq!(second.messages.len(), 1);
}

#[tokio::test]
async fn quick_reply_and_delayed_arrival_do_not_consume_visible_receipt_eligibility() {
    let (_temp, db) = fixture();
    let first = add(&db, vec![record("incoming")]).await;
    db.queue(first.chat.id, "reply".into(), "Quick reply".into())
        .await
        .unwrap();
    db.delivery(PN.into(), "reply".into(), "accepted")
        .await
        .unwrap();
    let view = db
        .view(first.chat.id, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.chat.unread, 0);
    assert!(
        db.next_receipt().await.unwrap().is_none(),
        "an outgoing ACK alone is not a visible read"
    );
    let mut history = record("old-history");
    history.timestamp -= 100;
    db.ingest(Batch {
        messages: vec![history],
        ..Default::default()
    })
    .await
    .unwrap();
    db.ingest(Batch {
        messages: vec![record("late")],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    db.mark_visible_read_with_receipts(view.chat.id, view.messages.last().unwrap().id, true)
        .await
        .unwrap();
    let job = db.next_receipt().await.unwrap().unwrap();
    assert_eq!(
        job.messages
            .iter()
            .map(|(_, wire)| wire.as_str())
            .collect::<Vec<_>>(),
        vec!["incoming", "late"]
    );
}
