//! Private linked-device read synchronization. The queue contains positions,
//! never a request to read whatever happens to be newest when it is retried.
use super::*;

#[derive(Clone, PartialEq, Eq)]
pub struct ReadSync {
    pub chat: i32,
    pub jid: String,
    pub through: i64,
    pub version: i64,
    pub message: Option<ReadMessage>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReadMessage {
    pub wire: String,
    pub sender: String,
    pub outgoing: bool,
}

// Deduplicate the observed position, rather than the local badge. An outgoing
// message never increments that badge but still advances our devices' read range.
fn queue(
    db: &Connection,
    chat: i32,
    row: i64,
    stamp: i64,
    wire: &str,
    marked: bool,
) -> Result<bool> {
    let queued = db.execute("INSERT INTO outgoing_reads(chat_id,stamp,wire_id,version,pending) VALUES(?1,?2,?3,1,1) ON CONFLICT(chat_id) DO UPDATE SET stamp=excluded.stamp,wire_id=excluded.wire_id,version=outgoing_reads.version+1,pending=1 WHERE (excluded.stamp>outgoing_reads.stamp OR (excluded.stamp=outgoing_reads.stamp AND ?5>=coalesce((SELECT max(id) FROM messages WHERE chat_id=?1 AND wire_id=outgoing_reads.wire_id),0))) AND (excluded.wire_id<>outgoing_reads.wire_id OR excluded.stamp<>outgoing_reads.stamp OR ?4)", params![chat,stamp,wire,marked,row])?;
    if queued == 0 {
        return Ok(false);
    }
    reads::advance(db, chat, stamp, row)?;
    db.execute(
        "UPDATE chats SET marked_unread=0,marked_stamp=?2 WHERE id=?1",
        params![chat, chrono::Utc::now().timestamp_millis()],
    )?;
    reads::recount(db, chat)?;
    Ok(true)
}

/// A confirmed send advances the same private read queue as viewing a chat.
/// Queued and failed sends cannot advance it.
pub(super) fn sent(db: &Connection, chat: i32, wire: &str) -> Result<()> {
    let position: Option<(i64,i64)> = db.query_row(
        "SELECT id,stamp FROM messages WHERE chat_id=?1 AND sender='#self' AND wire_id=?2 AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch()) AND delivery IN ('accepted','delivered','read') AND NOT (SELECT marked_unread FROM chats WHERE id=?1)",
        params![chat,wire], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    if let Some((row, stamp)) = position.filter(|(_, stamp)| *stamp > 0) {
        queue(db, chat, row, stamp, wire, false)?;
    }
    Ok(())
}

impl Archive {
    pub async fn mark_visible_read(&self, chat: i32, message: i32) -> Result<()> {
        let changed = self.call(move |db| {
            let tx = db.transaction()?;
            let chat = local_chat(&tx, chat)?;
            let boundary: Option<i64> = tx.query_row(
                "SELECT stamp FROM messages WHERE chat_id=?1 AND id=?2 AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch())",
                params![chat,message], |r| r.get(0),
            ).optional()?;
            let Some(stamp) = boundary else { return Ok(false); };
            if stamp <= 0 { return Ok(false); }
            // A queued/failed local message has not reached our other devices.
            // Sync preceding observed messages now; sent() handles the later ACK.
            let position: Option<(i64,i64,String)> = tx.query_row(
                "SELECT id,stamp,wire_id FROM messages WHERE chat_id=?1 AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch()) AND stamp>0 AND (stamp,id)<=(?2,?3) AND (outgoing=0 OR delivery IN ('accepted','delivered','read')) ORDER BY stamp DESC,id DESC LIMIT 1",
                params![chat,stamp,message], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            ).optional()?;
            let marked = tx.query_row("SELECT marked_unread FROM chats WHERE id=?1", [chat], |r|r.get(0))?;
            let queued = match position { Some((row,stamp,wire))=>queue(&tx,chat,row,stamp,&wire,marked)?, None=>false };
            tx.commit()?;
            Ok(queued)
        }).await?;
        if changed {
            self.changed();
        }
        Ok(())
    }

    pub async fn next_read_sync(&self) -> Result<Option<ReadSync>> {
        self.call(|db| Ok(db.query_row(
            "SELECT r.chat_id,c.jid,r.stamp,r.version,m.wire_id,m.source_sender,m.outgoing FROM outgoing_reads r JOIN chats c ON c.id=r.chat_id LEFT JOIN messages m ON m.chat_id=r.chat_id AND m.wire_id=r.wire_id AND m.stamp=r.stamp AND (SELECT count(*) FROM messages x WHERE x.chat_id=r.chat_id AND x.wire_id=r.wire_id AND x.stamp=r.stamp)=1 WHERE r.pending=1 ORDER BY r.chat_id LIMIT 1",
            [], |r| {
                let message = match r.get::<_,Option<String>>(4)? {
                    Some(wire)=>Some(ReadMessage{wire,sender:r.get(5)?,outgoing:r.get(6)?}),
                    None=>None,
                };
                Ok(ReadSync {chat:r.get(0)?,jid:r.get(1)?,through:r.get(2)?,version:r.get(3)?,message})
            },
        ).optional()?)).await
    }

    pub async fn finish_read_sync(&self, job: ReadSync) -> Result<()> {
        self.call(move |db| {
            // A newer read, or an alias merge while the request was in flight,
            // must survive completion of the older request.
            db.execute("UPDATE outgoing_reads SET pending=0 WHERE chat_id=?1 AND version=?2 AND stamp=?3 AND EXISTS(SELECT 1 FROM chats WHERE id=?1 AND jid=?4)", params![job.chat,job.version,job.through,job.jid])?;
            Ok(())
        }).await
    }
}

pub(super) fn merge(db: &Connection, old: i32, target: i32) -> Result<()> {
    db.execute("INSERT INTO outgoing_reads(chat_id,stamp,wire_id,version,pending) SELECT ?2,stamp,wire_id,version+1,pending FROM outgoing_reads WHERE chat_id=?1 ON CONFLICT(chat_id) DO UPDATE SET stamp=max(outgoing_reads.stamp,excluded.stamp),wire_id=CASE WHEN excluded.stamp>=outgoing_reads.stamp THEN excluded.wire_id ELSE outgoing_reads.wire_id END,version=max(outgoing_reads.version,excluded.version)+1,pending=max(outgoing_reads.pending,excluded.pending)", params![old,target])?;
    db.execute("DELETE FROM outgoing_reads WHERE chat_id=?1", [old])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{KEY, LID, PN, fixture, record};

    async fn add(db: &Archive, id: &str, stamp: i64) -> (i32, i32) {
        let mut r = record(id);
        r.timestamp = stamp;
        db.ingest(Batch {
            messages: vec![r],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let chat = db
            .view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats[0]
            .id;
        let view = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        (
            chat,
            view.messages
                .iter()
                .find(|m| m.text == format!("Synthetic {id}"))
                .unwrap()
                .id,
        )
    }
    async fn count(db: &Archive, chat: i32) -> usize {
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .chat
            .unread as usize
    }
    #[tokio::test]
    async fn outgoing_messages_advance_an_already_read_chat_once() {
        let (_temp, db) = fixture();
        let (chat, a) = add(&db, "a", 100).await;
        db.mark_visible_read(chat, a).await.unwrap();
        db.finish_read_sync(db.next_read_sync().await.unwrap().unwrap())
            .await
            .unwrap();
        let mut outgoing = record("mine");
        outgoing.timestamp = 200;
        outgoing.outgoing = true;
        db.ingest(Batch {
            messages: vec![outgoing],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let last = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .messages
            .last()
            .unwrap()
            .id;
        db.mark_visible_read(chat, last).await.unwrap();
        let sent = db.next_read_sync().await.unwrap().unwrap();
        assert_eq!(sent.through, 200);
        assert!(sent.message.as_ref().unwrap().outgoing);
        assert_eq!(sent.message.as_ref().unwrap().wire, "mine");
        db.mark_visible_read(chat, last).await.unwrap();
        assert!(db.next_read_sync().await.unwrap().unwrap() == sent);
        db.finish_read_sync(sent).await.unwrap();
        db.mark_visible_read(chat, last).await.unwrap();
        assert!(db.next_read_sync().await.unwrap().is_none());
        let (_, b) = add(&db, "b", 300).await;
        db.mark_visible_read(chat, b).await.unwrap();
        assert_eq!(db.next_read_sync().await.unwrap().unwrap().through, 300);
    }

    async fn queued(db: &Archive, chat: i32, wire: &str, stamp: i64) -> i32 {
        db.queue(chat, wire.into(), "Synthetic outgoing text".into())
            .await
            .unwrap();
        let wire = wire.to_owned();
        db.call(move |c| {
            c.execute(
                "UPDATE messages SET stamp=?2 WHERE wire_id=?1 AND sender='#self'",
                params![wire, stamp],
            )?;
            Ok(c.query_row(
                "SELECT id FROM messages WHERE wire_id=?1 AND sender='#self'",
                [wire],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn confirmed_send_survives_restart_and_preserves_new_arrivals() {
        let (_temp, db) = fixture();
        let (chat, a) = add(&db, "a", 100).await;
        db.mark_visible_read(chat, a).await.unwrap();
        db.finish_read_sync(db.next_read_sync().await.unwrap().unwrap())
            .await
            .unwrap();
        let own = queued(&db, chat, "own", 150).await;
        db.mark_visible_read(chat, own).await.unwrap();
        assert!(
            db.next_read_sync().await.unwrap().is_none(),
            "a queued bubble is not a remote read position"
        );
        db.claim_next().await.unwrap().unwrap();
        db.mark_visible_read(chat, own).await.unwrap();
        assert!(db.next_read_sync().await.unwrap().is_none());
        db.delivery(PN.into(), "own".into(), "failed")
            .await
            .unwrap();
        db.mark_visible_read(chat, own).await.unwrap();
        assert!(
            db.next_read_sync().await.unwrap().is_none(),
            "a failed send must not advance the range"
        );
        let files = db.0._files.clone();
        drop(db);
        let db = Archive::open(files, KEY).unwrap();
        add(&db, "new-same-second", 220).await;
        db.delivery_at(PN.into(), "own".into(), "accepted", Some(220))
            .await
            .unwrap();
        let job = db.next_read_sync().await.unwrap().unwrap();
        assert_eq!(
            job.through, 220,
            "use actual send time, not offline queue time"
        );
        assert_eq!(job.message.as_ref().unwrap().wire, "own");
        assert!(job.message.as_ref().unwrap().outgoing);
        assert_eq!(
            count(&db, chat).await,
            1,
            "an unobserved incoming message in the ACK's second stays unread"
        );
        // The SDK re-fetches app state after writing it. Its echo must retain
        // the message key instead of rounding the read up to the whole second.
        db.read_updates(vec![ReadUpdate {
            jid: PN.into(),
            through: Some(job.through),
            message: job.message.clone(),
            marked: false,
            version: Some(chrono::Utc::now().timestamp_millis()),
        }])
        .await
        .unwrap();
        assert_eq!(count(&db, chat).await, 1);
        db.finish_read_sync(job).await.unwrap();
        for status in ["accepted", "delivered", "read"] {
            db.delivery(PN.into(), "own".into(), status).await.unwrap();
            assert!(
                db.next_read_sync().await.unwrap().is_none(),
                "duplicate ACK/stronger receipt must not resync the same position"
            );
        }
        let unseen = queued(&db, chat, "not-observed", 230).await;
        db.delivery(PN.into(), "not-observed".into(), "accepted")
            .await
            .unwrap();
        let sent = db.next_read_sync().await.unwrap().unwrap();
        assert_eq!(
            sent.through, 230,
            "a confirmed send advances the existing range without view tracking"
        );
        db.finish_read_sync(sent).await.unwrap();
        db.mark_visible_read(chat, unseen).await.unwrap();
        assert!(db.next_read_sync().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn late_send_success_respects_remote_manual_unread_and_deleted_bubbles() {
        let (_temp, db) = fixture();
        let chat = db.create_chat(PN.into()).await.unwrap();
        let own = queued(&db, chat, "own", 100).await;
        db.mark_visible_read(chat, own).await.unwrap();
        db.read_updates(vec![ReadUpdate {
            message: None,
            jid: PN.into(),
            through: None,
            marked: true,
            version: Some(i64::MAX),
        }])
        .await
        .unwrap();
        db.delivery(PN.into(), "own".into(), "accepted")
            .await
            .unwrap();
        assert!(db.next_read_sync().await.unwrap().is_none());
        assert!(
            db.view(chat, 100, String::new(), false, false)
                .await
                .unwrap()
                .conversation
                .unwrap()
                .chat
                .marked_unread
        );
        db.mark_visible_read(chat, own).await.unwrap();
        db.finish_read_sync(db.next_read_sync().await.unwrap().unwrap())
            .await
            .unwrap();
        let removed = queued(&db, chat, "removed", 200).await;
        db.mark_visible_read(chat, removed).await.unwrap();
        db.blocking(|c| {
            c.execute("UPDATE messages SET deleted=1 WHERE id=?1", [removed])?;
            Ok(())
        })
        .unwrap();
        db.delivery(PN.into(), "removed".into(), "accepted")
            .await
            .unwrap();
        assert!(
            db.next_read_sync().await.unwrap().is_none(),
            "late success cannot restore a deleted read position"
        );
    }

    #[tokio::test]
    async fn schema_eight_upgrade_does_not_retroactively_broadcast_old_local_reads() {
        let (_temp, db) = fixture();
        let (chat, a) = add(&db, "a", 100).await;
        db.view(chat, 100, String::new(), false, true)
            .await
            .unwrap();
        let files = db.0._files.clone();
        db.blocking(|c| {
            c.execute_batch("DROP TABLE outgoing_reads; DROP TRIGGER discard_changed_document_preview; DROP TABLE document_previews; DROP TABLE history_pages; DROP TABLE history_requests; ALTER TABLE messages DROP COLUMN edit_version; ALTER TABLE mutations DROP COLUMN raw; PRAGMA user_version=8;")?;
            Ok(())
        })
        .unwrap();
        drop(db);
        let db = Archive::open(files, KEY).unwrap();
        assert_eq!(count(&db, chat).await, 0);
        assert!(db.next_read_sync().await.unwrap().is_none());
        db.mark_visible_read(chat, a).await.unwrap();
        let job = db.next_read_sync().await.unwrap().unwrap();
        assert_eq!(job.through, 100);
        // A later explicit unread mark from the phone supersedes an offline
        // read, while an old snapshot must not cancel that read.
        db.read_updates(vec![ReadUpdate {
            message: None,
            jid: PN.into(),
            through: None,
            marked: true,
            version: Some(1),
        }])
        .await
        .unwrap();
        assert!(db.next_read_sync().await.unwrap().is_some());
        db.read_updates(vec![ReadUpdate {
            message: None,
            jid: PN.into(),
            through: None,
            marked: true,
            version: Some(i64::MAX),
        }])
        .await
        .unwrap();
        assert!(db.next_read_sync().await.unwrap().is_none());
        assert!(
            db.view(chat, 100, String::new(), false, false)
                .await
                .unwrap()
                .conversation
                .unwrap()
                .chat
                .marked_unread
        );
    }

    #[tokio::test]
    async fn visible_read_survives_restart_coalesces_and_preserves_new_arrivals() {
        let (_temp, db) = fixture();
        let (chat, a) = add(&db, "a", 100).await;
        let (_, b) = add(&db, "b", 100).await;
        // Arrival after the displayed snapshot, including the same second.
        db.mark_visible_read(chat, a).await.unwrap();
        assert_eq!(count(&db, chat).await, 1);
        let first = db.next_read_sync().await.unwrap().unwrap();
        assert_eq!(first.through, 100);
        db.mark_visible_read(chat, b).await.unwrap();
        let next = db.next_read_sync().await.unwrap().unwrap();
        assert!(next.version > first.version);
        db.mark_visible_read(chat, a).await.unwrap();
        assert!(db.next_read_sync().await.unwrap().unwrap() == next);
        db.finish_read_sync(first).await.unwrap();
        assert!(db.next_read_sync().await.unwrap().unwrap() == next);
        let files = db.0._files.clone();
        drop(db);
        let db = Archive::open(files, KEY).unwrap();
        assert!(db.next_read_sync().await.unwrap().unwrap() == next);
        add(&db, "later", 200).await;
        db.finish_read_sync(next).await.unwrap();
        assert_eq!(count(&db, chat).await, 1);
        assert!(db.next_read_sync().await.unwrap().is_none());
        // Revisiting the same displayed position creates no new network work.
        db.mark_visible_read(chat, b).await.unwrap();
        assert!(db.next_read_sync().await.unwrap().is_none());
    }
    #[tokio::test]
    async fn queue_is_atomic_with_local_badge_and_rejects_foreign_message() {
        let (_temp, db) = fixture();
        let (chat, a) = add(&db, "a", 100).await;
        let other = db
            .create_chat("15550000002@s.whatsapp.net".into())
            .await
            .unwrap();
        db.mark_visible_read(other, a).await.unwrap();
        assert!(db.next_read_sync().await.unwrap().is_none());
        assert_eq!(count(&db, chat).await, 1);
        db.blocking(|c|{c.execute_batch("CREATE TRIGGER reject_read BEFORE INSERT ON outgoing_reads BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;")?;Ok(())}).unwrap();
        assert!(db.mark_visible_read(chat, a).await.is_err());
        assert_eq!(count(&db, chat).await, 1);
        assert!(db.next_read_sync().await.unwrap().is_none());
    }
    #[tokio::test]
    async fn aliases_and_manual_unread_keep_pending_work_and_stale_ack_cannot_drop_it() {
        let (_temp, db) = fixture();
        let (pn, a) = add(&db, "a", 100).await;
        db.mark_visible_read(pn, a).await.unwrap();
        let old = db.next_read_sync().await.unwrap().unwrap();
        let mut r = record("b");
        r.timestamp = 200;
        r.chat = LID.into();
        r.sender = LID.into();
        db.ingest(Batch {
            messages: vec![r],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let lid = db
            .view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats
            .iter()
            .find(|c| c.id != pn)
            .unwrap()
            .id;
        let b = db
            .view(lid, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .messages[0]
            .id;
        db.mark_visible_read(lid, b).await.unwrap();
        db.save_contacts(vec![], vec![(LID.into(), PN.into())], false)
            .await
            .unwrap();
        db.finish_read_sync(old).await.unwrap();
        let merged = db.next_read_sync().await.unwrap().unwrap();
        assert_eq!(merged.jid, PN);
        assert_eq!(merged.through, 200);
        db.finish_read_sync(merged).await.unwrap();
        db.read_updates(vec![ReadUpdate {
            message: None,
            jid: PN.into(),
            through: None,
            marked: true,
            version: Some(i64::MAX),
        }])
        .await
        .unwrap();
        let view = db
            .view(pn, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert!(view.chat.marked_unread);
        db.mark_visible_read(pn, view.messages.last().unwrap().id)
            .await
            .unwrap();
        assert!(db.next_read_sync().await.unwrap().is_some());
    }
}
