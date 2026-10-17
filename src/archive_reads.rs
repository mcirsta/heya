//! Durable incoming read state. Outgoing delivery status is deliberately separate.
use super::*;

#[derive(Clone)]
pub struct ReadUpdate {
    pub jid: String,
    /// Inclusive message timestamp, never the receipt's arrival/read time.
    pub through: Option<i64>,
    pub message: Option<ReadMessage>,
    pub marked: bool,
    pub version: Option<i64>,
}
impl ReadUpdate {
    pub fn coalesce(&mut self, next: Self) {
        if next.through > self.through
            || (next.through == self.through && next.version >= self.version)
        {
            self.through = next.through;
            self.message = next.message;
        }
        if self.version.is_none() || next.version >= self.version {
            self.marked = next.marked;
            self.version = next.version;
        }
    }
}

impl Archive {
    pub async fn begin_reads(&self) -> Result<bool> {
        self.call(|db| {
            let tx = db.transaction()?;
            let clean: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='reads_clean' AND value='1')",
                [],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('reads_clean','0')",
                [],
            )?;
            tx.commit()?;
            Ok(!clean)
        })
        .await
    }
    pub async fn finish_reads(&self) -> Result<()> {
        self.call(|db| {
            db.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('reads_clean','1')",
                [],
            )?;
            Ok(())
        })
        .await
    }
    pub async fn read_updates(&self, updates: Vec<ReadUpdate>) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            for update in updates {
                let chat = ensure(&tx, &update.jid)?;
                // A newer explicit unread mark on another device supersedes
                // a local read still waiting for reconnect/retry.
                if update.marked {
                    tx.execute("UPDATE outgoing_reads SET pending=0,version=version+1 WHERE chat_id=?1 AND pending=1 AND ?2>(SELECT marked_stamp FROM chats WHERE id=?1)", params![chat,update.version])?;
                }
                if let Some(through) = update.through.filter(|stamp| *stamp > 0) {
                    // A private range may name a precise outgoing/incoming
                    // boundary. Do not round it up over a newer same-second row.
                    let row = if let Some(key) = &update.message {
                        let sender=canonical(&tx,&key.sender)?;
                        tx.query_row("SELECT id FROM messages WHERE chat_id=?1 AND wire_id=?2 AND outgoing=?3 AND stamp=?4 AND (outgoing=1 OR sender=?5 OR source_sender=?6 OR NOT (SELECT is_group FROM chats WHERE id=?1)) ORDER BY id DESC LIMIT 1",params![chat,key.wire,key.outgoing,through,sender,key.sender],|r|r.get::<_,i64>(0)).optional()?.unwrap_or(i64::MAX)
                    } else {i64::MAX};
                    advance(&tx, chat, through, row)?;
                }
                tx.execute("UPDATE chats SET marked_unread=?2,marked_stamp=?3 WHERE id=?1 AND (marked_stamp IS NULL OR ?3>=marked_stamp)", params![chat,update.marked,update.version])?;
                recount(&tx, chat)?;
            }
            tx.commit()?;
            Ok(())
        }).await?;
        self.changed();
        Ok(())
    }
    pub async fn read_receipts(&self, jid: String, ids: Vec<String>) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            let chat = ensure(&tx, &jid)?;
            for id in ids {
                if !id.is_empty() {
                    tx.execute(
                        "INSERT OR IGNORE INTO pending_reads(chat_id,wire_id) VALUES(?1,?2)",
                        params![chat, id],
                    )?;
                }
            }
            settle(&tx, chat)?;
            recount(&tx, chat)?;
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }

    pub fn history_unread(
        &self,
        jid: String,
        through: i64,
        count: u32,
        marked: Option<bool>,
    ) -> Result<()> {
        self.blocking(|db| {
            let tx = db.transaction()?;
            let chat = upsert_chat(&tx, &ChatMeta { jid, ..Default::default() })?;
            let (previous, old_count, read_stamp, read_row): (i64,i64,i64,i64) = tx.query_row("SELECT history_stamp,history_count,read_stamp,read_row FROM chats WHERE id=?1", [chat], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            // Equal-position history may clear stale counts, never resurrect a
            // larger count after a read snapshot or replay an imported chunk.
            if through <= 0 || through < previous || (through == previous && i64::from(count) >= old_count) {
                return Ok(());
            }
            tx.execute("UPDATE chats SET history_stamp=?2,history_count=?3 WHERE id=?1", params![chat,through,count])?;
            if count == 0 {
                advance(&tx, chat, through, i64::MAX)?;
            } else {
                // Only the snapshot's range is authoritative. Newer live
                // arrivals retain their own unread flags.
                tx.execute("UPDATE messages SET unread=0 WHERE chat_id=?1 AND stamp<=?2", params![chat,through])?;
                tx.execute("UPDATE messages SET unread=1 WHERE id IN (SELECT id FROM messages WHERE chat_id=?1 AND outgoing=0 AND visible=1 AND stamp<=?2 ORDER BY stamp DESC,id DESC LIMIT ?3) AND seen=0 AND (stamp,id)>(?4,?5)", params![chat,through,count,read_stamp,read_row])?;
                let available: i64 = tx.query_row("SELECT count(*) FROM messages WHERE chat_id=?1 AND outgoing=0 AND visible=1 AND stamp<=?2", params![chat,through], |r| r.get(0))?;
                // A partial history can omit unread messages. Retain its total
                // only until a read position reaches that unknown prefix.
                let extra = if read_stamp == 0 { (i64::from(count)-available).max(0) } else { 0 };
                tx.execute("UPDATE chats SET unread_extra=?2 WHERE id=?1", params![chat,extra])?;
            }
            if let Some(marked) = marked {
                tx.execute("UPDATE chats SET marked_unread=?2 WHERE id=?1 AND marked_stamp IS NULL", params![chat,marked])?;
            }
            settle(&tx, chat)?;
            recount(&tx, chat)?;
            tx.commit()?;
            Ok(())
        })?;
        self.changed();
        Ok(())
    }
}

pub(super) fn ensure(db: &Connection, jid: &str) -> Result<i32> {
    let jid = canonical(db, jid)?;
    db.execute(
        "INSERT OR IGNORE INTO chats(jid,is_group,listed) VALUES(?1,?2,0)",
        params![jid, jid.ends_with("@g.us")],
    )?;
    Ok(db.query_row("SELECT id FROM chats WHERE jid=?1", [jid], |r| r.get(0))?)
}

pub(super) fn advance(db: &Connection, chat: i32, stamp: i64, row: i64) -> Result<()> {
    db.execute(
        "UPDATE chats SET read_stamp=?2,read_row=?3 WHERE id=?1 AND (read_stamp,read_row)<(?2,?3)",
        params![chat, stamp, row],
    )?;
    db.execute("UPDATE messages SET unread=0,seen=1 WHERE chat_id=?1 AND outgoing=0 AND (unread=1 OR seen=0) AND (stamp,id)<=(SELECT read_stamp,read_row FROM chats WHERE id=?1)", [chat])?;
    db.execute(
        "UPDATE chats SET unread_extra=0 WHERE id=?1 AND history_stamp<=read_stamp AND unread_extra<>0",
        [chat],
    )?;
    Ok(())
}
pub(super) fn local_read(db: &Connection, chat: i32) -> Result<()> {
    let last: Option<(i64,i64)> = db.query_row("SELECT stamp,id FROM messages WHERE chat_id=?1 AND visible=1 ORDER BY stamp DESC,id DESC LIMIT 1", [chat], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((stamp, row)) = last {
        advance(db, chat, stamp, row)?;
    }
    db.execute("UPDATE chats SET unread=0,unread_extra=0,marked_unread=0,marked_stamp=?2 WHERE id=?1 AND (unread<>0 OR marked_unread<>0)", params![chat,chrono::Utc::now().timestamp_millis()])?;
    Ok(())
}
pub(super) fn inserted(db: &Connection, chat: i32, row: i64, live: bool) -> Result<()> {
    if live {
        let changed = db.execute("UPDATE messages SET unread=1 WHERE id=?2 AND visible=1 AND outgoing=0 AND (stamp,id)>(SELECT read_stamp,read_row FROM chats WHERE id=?1)", params![chat,row])?;
        if changed > 0 {
            db.execute("UPDATE chats SET unread=unread+1 WHERE id=?1", [chat])?;
        }
    }
    db.execute("UPDATE messages SET seen=1 WHERE id=?2 AND outgoing=0 AND (stamp,id)<=(SELECT read_stamp,read_row FROM chats WHERE id=?1)", params![chat,row])?;
    settle(db, chat)
}
pub(super) fn settle(db: &Connection, chat: i32) -> Result<()> {
    let pending: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pending_reads WHERE chat_id=?1)",
        [chat],
        |r| r.get(0),
    )?;
    if !pending {
        return Ok(());
    }
    // Receipts can precede offline message delivery. Keep unresolved identities
    // encrypted across restarts; never interpret their receipt time as a range.
    let position: Option<(i64,i64)> = db.query_row("SELECT m.stamp,m.id FROM messages m JOIN pending_reads p ON p.chat_id=m.chat_id AND p.wire_id=m.wire_id WHERE m.chat_id=?1 AND m.outgoing=0 AND m.visible=1 AND (SELECT count(*) FROM messages x WHERE x.chat_id=m.chat_id AND x.wire_id=m.wire_id AND x.outgoing=0)=1 ORDER BY m.stamp DESC,m.id DESC LIMIT 1", [chat], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((stamp, row)) = position {
        advance(db, chat, stamp, row)?;
        recount(db, chat)?;
    }
    db.execute("DELETE FROM pending_reads WHERE chat_id=?1 AND EXISTS(SELECT 1 FROM messages m WHERE m.chat_id=?1 AND m.wire_id=pending_reads.wire_id)", [chat])?;
    Ok(())
}
pub(super) fn recount(db: &Connection, chat: i32) -> Result<()> {
    db.execute("UPDATE chats SET unread=unread_extra+(SELECT count(*) FROM messages WHERE chat_id=?1 AND unread=1) WHERE id=?1", [chat])?;
    Ok(())
}

pub(super) fn merge(db: &Connection, old: i32, target: i32) -> Result<()> {
    let (stamp, row): (i64, i64) = db.query_row(
        "SELECT read_stamp,read_row FROM chats WHERE id=?1",
        [old],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // Local row order from another alias is not a comparable same-second
    // position. Per-message seen evidence survives deduplication; carry only
    // the strictly earlier timestamp, or a genuinely inclusive time range.
    advance(db, target, stamp, if row == i64::MAX { row } else { 0 })?;
    db.execute("UPDATE chats SET unread_extra=max(unread_extra,(SELECT unread_extra FROM chats WHERE id=?1)) WHERE id=?2 AND read_stamp=0", params![old,target])?;
    db.execute("UPDATE chats SET marked_unread=(SELECT marked_unread FROM chats WHERE id=?1),marked_stamp=(SELECT marked_stamp FROM chats WHERE id=?1) WHERE id=?2 AND (marked_stamp IS NULL OR marked_stamp<(SELECT marked_stamp FROM chats WHERE id=?1))", params![old,target])?;
    db.execute("UPDATE chats SET history_stamp=(SELECT history_stamp FROM chats WHERE id=?1),history_count=(SELECT history_count FROM chats WHERE id=?1) WHERE id=?2 AND history_stamp<(SELECT history_stamp FROM chats WHERE id=?1)", params![old,target])?;
    db.execute("INSERT OR IGNORE INTO pending_reads(chat_id,wire_id) SELECT ?2,wire_id FROM pending_reads WHERE chat_id=?1", params![old,target])?;
    db.execute("DELETE FROM pending_reads WHERE chat_id=?1", [old])?;
    settle(db, target)?;
    recount(db, target)
}

#[cfg(test)]
#[path = "archive_read_tests.rs"]
mod tests;
