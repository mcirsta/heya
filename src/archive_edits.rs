//! Apply each edit revision once, on the archive worker. No UI or media work.
use super::*;

// All incoming changes and acknowledged local controls use the same identity
// and revision rules. Do this after aliases have been merged in the transaction.
pub(super) fn record(db: &Connection, chat: i32, author: &str, change: &Change) -> Result<bool> {
    let target = canonical(db, &change.sender)?;
    if target != author || change.id.is_empty() {
        return Ok(false);
    }
    db.execute("INSERT INTO mutations(chat_id,sender,wire_id,body,version,deleted,raw) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(chat_id,sender,wire_id) DO UPDATE SET body=CASE WHEN mutations.deleted=1 THEN NULL ELSE excluded.body END,raw=CASE WHEN mutations.deleted=1 OR excluded.deleted=1 THEN X'' ELSE excluded.raw END,version=max(mutations.version,excluded.version),deleted=max(mutations.deleted,excluded.deleted) WHERE excluded.deleted=1 OR excluded.version>mutations.version",params![chat,target,change.id,change.text,change.version,change.text.is_none(),change.raw])?;
    Ok(true)
}

pub(super) fn apply(db: &Connection, chat: Option<i32>) -> Result<()> {
    // Keep normal ingestion scoped to one chat so the mutation primary key
    // remains usable; the unscoped path is only migration/alias reconciliation.
    let scope = if chat.is_some() {
        "x.chat_id=?1"
    } else {
        "?1 IS NULL"
    };
    db.execute(&format!("UPDATE mutations AS x SET raw=X'',body=NULL,deleted=1 WHERE {scope} AND (x.deleted=0 OR length(x.raw)>0 OR x.body IS NOT NULL) AND EXISTS(SELECT 1 FROM message_hides h WHERE h.chat_id=x.chat_id AND h.sender=x.sender AND h.wire_id=x.wire_id)"),[chat])?;
    // The LEFT JOIN keeps mutations outermost and also exposes missing originals.
    // The message identity index avoids scanning the entire conversation.
    loop {
        let mut query = db.prepare_cached(&format!(
            "SELECT m.id,coalesce(m.raw,X''),x.raw,x.body,x.version,x.deleted,x.rowid
             FROM mutations x LEFT JOIN messages m USING(chat_id,sender,wire_id)
             WHERE {scope} AND ((m.id IS NULL AND x.deleted=1) OR
               (m.deleted=0 AND m.visible=1
                AND (m.expires IS NULL OR m.expires>unixepoch())
                AND (x.deleted=1 OR x.version>m.edit_version)))
               AND NOT EXISTS(SELECT 1 FROM message_hides h
                 WHERE h.chat_id=x.chat_id AND h.sender=x.sender AND h.wire_id=x.wire_id)
             LIMIT 100"
        ))?;
        let rows = query
            .query_map([chat], |r| {
                Ok((
                    r.get::<_, Option<i32>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, bool>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            break;
        }
        for (id, raw, edited, body, version, deleted, mutation) in rows {
            // A revoke is a visible tombstone even if its original was never
            // delivered here. Keep the original identity, not a second bubble
            // for the delete envelope. It starts with no invented unread count.
            let Some(id) = id else {
                db.execute("INSERT INTO messages(chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,deleted,delivery,source_chat,source_sender,edit_version)
                    SELECT x.chat_id,x.sender,x.wire_id,CASE WHEN x.version>100000000000 THEN x.version/1000 ELSE x.version END,x.sender='#self','',?2,X'',1,1,CASE WHEN x.sender='#self' THEN 'accepted' ELSE '' END,c.jid,x.sender,x.version
                    FROM mutations x JOIN chats c ON c.id=x.chat_id WHERE x.rowid=?1",params![mutation,domain::DELETED_MESSAGE])?;
                continue;
            };
            if deleted {
                db.execute("UPDATE messages SET body=?3,content='',thumbnail=X'',raw=X'',deleted=1,edit_version=?2 WHERE id=?1",params![id,version,domain::DELETED_MESSAGE])?;
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
