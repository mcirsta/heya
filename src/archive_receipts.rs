//! Durable sender-facing receipt intent, separate from private read positions.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReceiptKind {
    Read = 0,
    Played = 1,
}

#[derive(Clone)]
pub struct ReceiptJob {
    pub jid: String,
    pub sender: String,
    pub kind: ReceiptKind,
    pub messages: Vec<(i64, String)>,
}

pub(super) fn inserted(db: &Connection, row: i64) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO outgoing_receipts(message_id,kind,state)
        SELECT id,0,2 FROM messages WHERE id=?1 AND outgoing=0 AND visible=1 AND deleted=0",
        [row],
    )?;
    Ok(())
}

pub(super) fn visible(
    db: &Connection,
    chat: i32,
    stamp: i64,
    row: i32,
    enabled: bool,
) -> Result<bool> {
    // Existing unread history is eligible too. Already-read imported history
    // has no unobserved marker and must never be backfilled.
    let inserted = db.execute(
        "INSERT OR IGNORE INTO outgoing_receipts(message_id,kind,state)
         SELECT id,0,?4 FROM messages WHERE chat_id=?1 AND unread=1 AND outgoing=0
         AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch())
         AND (stamp,id)<=(?2,?3)",
        params![chat, stamp, row, enabled],
    )?;
    let observed = db.execute(
        "UPDATE outgoing_receipts SET state=?4 WHERE kind=0 AND
        (state=2 OR (state=1 AND ?4=0)) AND message_id IN (
        SELECT id FROM messages WHERE chat_id=?1 AND (stamp,id)<=(?2,?3)
        AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch()))",
        params![chat, stamp, row, enabled],
    )?;
    Ok(inserted > 0 || observed > 0)
}

impl Archive {
    pub async fn queue_played_receipt(&self, request: &crate::audio::Request) -> Result<()> {
        let request = request.clone();
        let changed = self
            .call(move |db| {
                Ok(db.execute(
                    "INSERT OR IGNORE INTO outgoing_receipts(message_id,kind)
             SELECT id,1 FROM messages WHERE id=?1 AND raw=?2 AND outgoing=0
             AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch())
             AND json_extract(nullif(content,''),'$.audio.voice')=1",
                    params![request.id, request.raw],
                )? > 0)
            })
            .await?;
        if changed {
            self.changed();
        }
        Ok(())
    }

    pub async fn discard_receipts(&self) -> Result<()> {
        self.call(|db| {
            db.execute("UPDATE outgoing_receipts SET state=0 WHERE state=1", [])?;
            Ok(())
        })
        .await
    }

    pub async fn next_receipt(&self) -> Result<Option<ReceiptJob>> {
        self.call(|db| {
            // Expired/revoked/hidden messages cannot keep a queue stuck.
            db.execute(
                "UPDATE outgoing_receipts SET state=0 WHERE state=1 AND EXISTS (
                SELECT 1 FROM messages m WHERE m.id=message_id AND (m.deleted=1 OR m.visible=0 OR m.expires<=unixepoch()))",
                [],
            )?;
            let first = db
                .query_row(
                    "SELECT m.chat_id,m.source_sender,r.kind FROM outgoing_receipts r
                 JOIN messages m ON m.id=r.message_id WHERE r.state=1
                 ORDER BY r.kind,r.message_id LIMIT 1",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, i32>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i32>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((chat, sender, kind)) = first else {
                return Ok(None);
            };
            let jid = db.query_row("SELECT jid FROM chats WHERE id=?1", [chat], |r| r.get(0))?;
            let messages = db.prepare(
                "SELECT m.id,m.wire_id FROM outgoing_receipts r JOIN messages m ON m.id=r.message_id
                 WHERE r.state=1 AND r.kind=?1 AND m.chat_id=?2 AND m.source_sender=?3
                 ORDER BY m.id LIMIT 256",
            )?.query_map(params![kind,chat,sender], |r|Ok((r.get(0)?,r.get(1)?)))?
                .collect::<std::result::Result<Vec<_>,_>>()?;
            Ok(Some(ReceiptJob {
                jid,
                sender,
                kind: if kind == 0 {
                    ReceiptKind::Read
                } else {
                    ReceiptKind::Played
                },
                messages,
            }))
        })
        .await
    }

    pub async fn finish_receipt(&self, job: ReceiptJob) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            for (id, wire) in job.messages {
                tx.execute(
                    "UPDATE outgoing_receipts SET state=0 WHERE message_id=?1 AND kind=?2
                    AND EXISTS(SELECT 1 FROM messages m JOIN chats c ON c.id=m.chat_id
                    WHERE m.id=?1 AND m.wire_id=?3 AND c.jid=?4 AND m.source_sender=?5)",
                    params![id, job.kind as i32, wire, job.jid, job.sender],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
#[path = "archive_receipt_tests.rs"]
mod tests;
