//! Apply each edit revision once, on the archive worker. No UI or media work.
use super::*;

pub(super) fn apply(db: &Connection, chat: Option<i32>) -> Result<()> {
    // Keep normal ingestion scoped to one chat so the mutation primary key
    // remains usable; the unscoped path is only migration/alias reconciliation.
    let scope = if chat.is_some() {
        "x.chat_id=?1"
    } else {
        "?1 IS NULL"
    };
    db.execute(&format!("UPDATE mutations AS x SET raw=X'',body=NULL,deleted=1 WHERE {scope} AND (x.deleted=0 OR length(x.raw)>0 OR x.body IS NOT NULL) AND EXISTS(SELECT 1 FROM message_hides h WHERE h.chat_id=x.chat_id AND h.sender=x.sender AND h.wire_id=x.wire_id)"),[chat])?;
    // Keep mutations outermost. SQLite otherwise chooses the message index and
    // visits an entire conversation even when there are no pending edits.
    loop {
        let mut query = db.prepare_cached(&format!(
            "SELECT m.id,m.raw,x.raw,x.body,x.version,x.deleted
             FROM mutations x CROSS JOIN messages m USING(chat_id,sender,wire_id)
             WHERE {scope} AND m.deleted=0 AND m.visible=1
               AND (m.expires IS NULL OR m.expires>unixepoch())
               AND (x.deleted=1 OR x.version>m.edit_version)
               AND NOT EXISTS(SELECT 1 FROM message_hides h
                 WHERE h.chat_id=m.chat_id AND h.sender=m.sender AND h.wire_id=m.wire_id)
             LIMIT 100"
        ))?;
        let rows = query
            .query_map([chat], |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, bool>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            break;
        }
        for (id, raw, edited, body, version, deleted) in rows {
            if deleted {
                db.execute("UPDATE messages SET body='[Message deleted]',content='',thumbnail=X'',raw=X'',deleted=1,edit_version=?2 WHERE id=?1",params![id,version])?;
                continue;
            }
            if let Some(applied) =
                crate::protocol_edits::apply(&raw, &edited, body.as_deref().unwrap_or_default())
            {
                let message = applied.message;
                db.execute("UPDATE messages SET body=?2,content=?3,thumbnail=?4,raw=?5,edit_version=?6 WHERE id=?1",params![id,message.text,encode_content(&message.content)?,message.thumbnail,if applied.same_media {&raw}else{&message.raw},version])?;
                // Forward/quote the latest complete envelope. The immutable
                // media transfer reference may still be the original envelope.
                db.execute("UPDATE mutations SET raw=?2 WHERE (chat_id,sender,wire_id)=(SELECT chat_id,sender,wire_id FROM messages WHERE id=?1)",params![id,message.raw])?;
            } else {
                // Unknown or mismatched edits cannot erase a valid attachment.
                db.execute(
                    "UPDATE messages SET edit_version=?2 WHERE id=?1",
                    params![id, version],
                )?;
                db.execute("UPDATE mutations SET raw=?2 WHERE (chat_id,sender,wire_id)=(SELECT chat_id,sender,wire_id FROM messages WHERE id=?1)",params![id,raw])?;
            }
        }
    }
    // Revoke/expiry/hide always remove retained edit content too.
    db.execute(&format!("UPDATE mutations AS x SET raw=X'',body=NULL WHERE {scope} AND deleted=1 AND (length(raw)>0 OR body IS NOT NULL)"),[chat])?;
    Ok(())
}
