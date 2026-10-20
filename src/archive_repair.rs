//! Re-materialize previously unsupported text without changing message identity.
use super::*;

impl Archive {
    /// One-time metadata upgrade from encrypted envelopes, including messages
    /// whose text already worked. Never restores content removed by mutations.
    pub fn repair_message_content(
        &self,
        decode: impl Fn(&[u8]) -> Option<(crate::message_content::Content, Vec<u8>)>,
    ) -> Result<()> {
        if self.blocking(|db| Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='content_decoder_revision' AND value='9')",[],|r|r.get::<_,bool>(0))?))? {
            return Ok(());
        }
        let mut after = 0_i64;
        loop {
            let last = self.blocking(|db| {
                let tx = db.transaction()?;
                expire(&tx)?;
                let ids = tx.prepare("SELECT m.id FROM messages m WHERE m.id>?1 AND m.deleted=0 AND length(m.raw)>0 AND NOT EXISTS(SELECT 1 FROM mutations x WHERE x.chat_id=m.chat_id AND x.sender=m.sender AND x.wire_id=m.wire_id) ORDER BY m.id LIMIT 100")?
                    .query_map([after], |r|r.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
                for id in &ids {
                    let raw: Vec<u8> = tx.query_row("SELECT raw FROM messages WHERE id=?1",[id],|r|r.get(0))?;
                    if let Some((content, thumbnail)) = decode(&raw) {
                        if let Some(notice) = content.notice {
                            tx.execute("UPDATE messages SET body=?2 WHERE id=?1", params![id, notice.description()])?;
                        }
                        if let Some(reaction)=&content.reaction {
                            let (chat,actor,wire,stamp,outgoing):(i32,String,String,i64,bool)=tx.query_row("SELECT chat_id,sender,wire_id,stamp,outgoing FROM messages WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
                            actions::reaction(&tx,chat,&actor,&wire,stamp,if outgoing {"accepted"} else {""},reaction)?;
                        }
                        tx.execute("UPDATE messages SET content=?2,thumbnail=?3 WHERE id=?1",params![id, encode_content(&content)?, thumbnail])?;
                    }
                }
                if ids.is_empty() {
                    tx.execute("INSERT OR REPLACE INTO metadata(key,value) VALUES('content_decoder_revision','9')",[])?;
                }
                tx.commit()?;
                Ok(ids.last().copied())
            })?;
            match last {
                Some(id) => after = id,
                None => break,
            }
        }
        self.changed();
        Ok(())
    }
    /// Called on the startup blocking worker before publishing the archive.
    /// The adapter owns decoding; storage only handles opaque saved envelopes.
    /// Batches bound transaction size and decode one envelope at a time. A
    /// failed/interrupted pass can restart: repaired rows no longer qualify.
    pub fn repair_unsupported_text(
        &self,
        revision: u32,
        decode: impl Fn(&[u8]) -> Option<String>,
    ) -> Result<()> {
        let complete = self.blocking(|db| {
            Ok(db
                .query_row(
                    "SELECT value FROM metadata WHERE key='text_decoder_revision'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .and_then(|value| value.parse::<u32>().ok())
                .is_some_and(|value| value >= revision))
        })?;
        if complete {
            return Ok(());
        }
        self.blocking(|db| {
            let tx = db.transaction()?;
            expire(&tx)?;
            tx.commit()?;
            Ok(())
        })?;
        let mut after = 0_i64;
        loop {
            let (last, changed) = self.blocking(|db| {
                let tx = db.transaction()?;
                let ids = tx
                    .prepare(
                        "SELECT m.id FROM messages m
                     WHERE m.id>?1 AND m.body='[Unsupported message]'
                       AND m.visible=1 AND m.deleted=0 AND length(m.raw)>0
                       AND (m.expires IS NULL OR m.expires>unixepoch())
                       AND NOT EXISTS(SELECT 1 FROM mutations x WHERE
                         x.chat_id=m.chat_id AND x.sender=m.sender AND x.wire_id=m.wire_id)
                     ORDER BY m.id LIMIT 100",
                    )?
                    .query_map([after], |r| r.get::<_, i64>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut changed = false;
                for id in &ids {
                    let raw: Vec<u8> =
                        tx.query_row("SELECT raw FROM messages WHERE id=?1", [id], |r| r.get(0))?;
                    if let Some(text) = decode(&raw).filter(|text| !text.is_empty()) {
                        changed |= tx.execute(
                            "UPDATE messages SET body=?2 WHERE id=?1 AND body<>?2",
                            params![id, text],
                        )? > 0;
                    }
                }
                if ids.is_empty() {
                    tx.execute(
                        "INSERT INTO metadata(key,value) VALUES('text_decoder_revision',?1)
                         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                        [revision.to_string()],
                    )?;
                }
                tx.commit()?;
                Ok((ids.last().copied(), changed))
            })?;
            if changed {
                self.changed();
            }
            match last {
                Some(id) => after = id,
                None => return Ok(()),
            }
        }
    }
}

impl Archive {
    /// Reprocess delete envelopes rejected by the old sender-relative key check.
    /// Uses the normal mutation writer/projection on the startup archive worker.
    pub fn repair_deletions(
        &self,
        decode: impl Fn(&[u8], &str, &str, bool, i64) -> Option<Change>,
    ) -> Result<()> {
        if self.blocking(|db| Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='delete_decoder_revision' AND value='1')",[],|r|r.get::<_,bool>(0))?))? {
            return Ok(());
        }
        let mut after = 0_i64;
        loop {
            let last=self.blocking(|db| {
                let tx=db.transaction()?;
                let rows=tx.prepare("SELECT id,chat_id,source_chat,source_sender,sender,outgoing,stamp,raw FROM messages WHERE id>?1 AND visible=0 AND deleted=0 AND length(raw)>0 ORDER BY id LIMIT 100")?
                    .query_map([after],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i32>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,bool>(5)?,r.get::<_,i64>(6)?,r.get::<_,Vec<u8>>(7)?)))?
                    .collect::<std::result::Result<Vec<_>,_>>()?;
                let mut changed=std::collections::BTreeSet::new();
                for (id,chat,source,sender,author,outgoing,stamp,raw) in &rows {
                    if let Some(change)=decode(raw,source,sender,*outgoing,*stamp)
                        && change.text.is_none()
                        && edits::record(&tx,*chat,&canonical(&tx,author)?,&change)? {
                            tx.execute("UPDATE messages SET raw=X'' WHERE id=?1",[id])?;
                            changed.insert(*chat);
                    }
                }
                for chat in changed { edits::apply(&tx,Some(chat))?; }
                if rows.is_empty() {
                    // Previously accepted early revokes also need a tombstone.
                    edits::apply(&tx,None)?;
                    tx.execute("UPDATE messages SET body=?1 WHERE deleted=1 AND body='[Message deleted]'",[domain::DELETED_MESSAGE])?;
                    tx.execute("INSERT OR REPLACE INTO metadata(key,value) VALUES('delete_decoder_revision','1')",[])?;
                }
                tx.commit()?;
                Ok(rows.last().map(|row|row.0))
            })?;
            if let Some(id) = last {
                after = id;
            } else {
                break;
            }
        }
        self.changed();
        Ok(())
    }
}
