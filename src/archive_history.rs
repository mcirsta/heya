//! Durable phone-history completion, correlated only after the chunk is imported.
use super::*;
use crate::phone_history::Anchor;
impl Archive {
    pub async fn phone_history_anchor(&self, chat: i32) -> Result<Option<Anchor>> {
        self.call(move|db| {
            let chat=local_chat(db,chat)?;
            let jid:Option<String>=db.query_row("SELECT jid FROM chats WHERE id=?1",[chat],|r|r.get(0)).optional()?;
            let Some(jid)=jid else {return Ok(None)};
            let first:Option<(String,bool,i64)>=db.query_row("SELECT wire_id,outgoing,stamp FROM messages WHERE chat_id=?1 AND visible=1 AND CASE WHEN json_valid(content) THEN json_type(content,'$.notice.call') IS NULL ELSE 1 END AND stamp>0 AND (outgoing=0 OR delivery IN ('accepted','delivered','read')) ORDER BY stamp,id LIMIT 1",[chat],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let (wire,outgoing,stamp)=first.unwrap_or_else(||(String::new(),false,chrono::Utc::now().timestamp()));
            let known=db.query_row("SELECT count(*) FROM messages WHERE chat_id=?1 AND visible=1 AND CASE WHEN json_valid(content) THEN json_type(content,'$.notice.call') IS NULL ELSE 1 END AND stamp<=?2 AND (outgoing=0 OR delivery IN ('accepted','delivered','read'))",params![chat,stamp],|r|r.get(0))?;
            Ok(Some(Anchor{chat,jid,wire,outgoing,stamp,known}))
        }).await
    }
    pub async fn phone_history_advanced(&self, anchor: Anchor) -> Result<bool> {
        self.call(move|db|{
            let chat=local_chat(db,anchor.chat)?;
            let count:i64=db.query_row("SELECT count(*) FROM messages WHERE chat_id=?1 AND visible=1 AND CASE WHEN json_valid(content) THEN json_type(content,'$.notice.call') IS NULL ELSE 1 END AND stamp<=?2 AND (outgoing=0 OR delivery IN ('accepted','delivered','read'))",params![chat,anchor.stamp],|r|r.get(0))?;
            Ok(count>anchor.known)
        }).await
    }
    pub async fn phone_history_metadata(&self, chunk: String, request: String) -> Result<()> {
        self.call(move|db|{db.execute("INSERT OR IGNORE INTO history_requests(request_id,chunk_id) SELECT ?2,id FROM history_chunks WHERE id=?1",params![chunk,request])?;Ok(())}).await?;
        self.changed();
        Ok(())
    }
    pub async fn phone_history_answer(&self, request: String, chat: i32) -> Result<bool> {
        self.call(move|db|{
            let chat=local_chat(db,chat)?;
            Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM history_requests r JOIN history_chunks h ON h.id=r.chunk_id JOIN history_pages p ON p.chunk_id=h.id WHERE r.request_id=?1 AND h.state='done' AND p.chat_id=?2)",params![request,chat],|r|r.get(0))?)
        }).await
    }
    pub fn finish_history(
        &self,
        chunk: &str,
        progress: usize,
        pages: Vec<(String, Option<bool>)>,
    ) -> Result<()> {
        self.blocking(|db|{
            let tx=db.transaction()?;
            for (jid,more) in pages {
                let jid=canonical(&tx,&jid)?;
                tx.execute("INSERT INTO history_pages(chunk_id,chat_id,more) SELECT ?1,id,?3 FROM chats WHERE jid=?2 ON CONFLICT(chunk_id,chat_id) DO UPDATE SET more=excluded.more",params![chunk,jid,more])?;
            }
            tx.execute("UPDATE history_chunks SET progress=?2,state='done',data=NULL WHERE id=?1",params![chunk,progress as i64])?;
            tx.commit()?;Ok(())
        })?;
        self.changed();
        Ok(())
    }
}
pub(super) fn more(db: &Connection, chat: i32) -> Result<bool> {
    Ok(db.query_row("SELECT coalesce((SELECT p.more FROM history_pages p JOIN history_chunks h ON h.id=p.chunk_id WHERE p.chat_id=?1 AND h.state='done' AND p.more IS NOT NULL ORDER BY h.rowid DESC LIMIT 1),1)",[chat],|r|r.get(0))?)
}
pub(super) fn merge(db: &Connection, old: i32, target: i32) -> Result<()> {
    db.execute("INSERT INTO history_pages(chunk_id,chat_id,more) SELECT chunk_id,?2,more FROM history_pages WHERE chat_id=?1 ON CONFLICT(chunk_id,chat_id) DO UPDATE SET more=coalesce(history_pages.more,excluded.more)",params![old,target])?;
    db.execute("DELETE FROM history_pages WHERE chat_id=?1", [old])?;
    Ok(())
}
