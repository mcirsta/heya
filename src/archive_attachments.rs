//! Uploaded envelopes join the ordinary durable outbox in one transaction.
use super::*;
use crate::{attachments::Destination, message_actions::Prepared};
impl Archive {
    pub async fn attachment_destination(&self, chat: i32) -> Result<Destination> {
        self.call(move|db|{
            let chat=local_chat(db,chat)?;
            chat_settings::unlocked(db,chat)?;
            let (title,readonly):(String,bool)=db.query_row("SELECT coalesce(nullif(c.name,''),nullif(n.saved_name,''),nullif(n.profile_name,''),c.jid),c.read_only FROM chats c LEFT JOIN contacts n ON n.jid=c.jid WHERE c.id=?1",[chat],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if readonly{return Err(Error::Invalid);}
            let reply:Option<i32>=db.query_row("SELECT message_id FROM reply_drafts WHERE chat_id=?1",[chat],|r|r.get(0)).optional()?;
            let reply=reply.map(|id|actions::source(db,chat,id)).transpose()?;
            let owner=db.query_row("SELECT value FROM metadata WHERE key='identity'",[],|r|r.get(0)).optional()?.unwrap_or_default();
            Ok(Destination{chat,title,reply,owner})
        }).await
    }
    #[cfg(test)]
    pub async fn queue_attachment(
        &self,
        chat: i32,
        id: String,
        prepared: Prepared,
        reply: Option<i32>,
    ) -> Result<()> {
        self.queue_media(chat, id, prepared, reply, None).await
    }
    pub async fn queue_media(
        &self,
        chat: i32,
        id: String,
        prepared: Prepared,
        reply: Option<i32>,
        audio: Option<std::sync::Arc<[u8]>>,
    ) -> Result<()> {
        if audio.as_ref().is_some_and(|a| {
            a.is_empty() || a.len() > crate::audio::MAX_BYTES || prepared.content.audio.is_none()
        }) {
            return Err(Error::Invalid);
        }
        self.call(move|db|{
            let tx=db.transaction()?;let chat=local_chat(&tx,chat)?;
            chat_settings::unlocked(&tx,chat)?;
            let (readonly,ephemeral):(bool,u32)=tx.query_row("SELECT read_only,ephemeral FROM chats WHERE id=?1",[chat],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if readonly || prepared.raw.is_empty() || id.len()>128 {return Err(Error::Invalid);}
            if let Some(reply)=reply {let _=actions::source(&tx,chat,reply)?;}
            let now=chrono::Utc::now().timestamp();
            // The staging item's ID survives upload/archive retries. A committed
            // row can never become a second message when its UI reply was lost.
            tx.execute("INSERT OR IGNORE INTO messages(chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,delivery,expires,source_chat,source_sender,content,thumbnail) VALUES(?1,'#self',?2,?3,1,'You',?4,?5,1,'queued',?6,(SELECT jid FROM chats WHERE id=?1),'#self',?7,?8)",params![chat,id,now,prepared.text,prepared.raw,(ephemeral>0).then_some(now+ephemeral as i64),encode_content(&prepared.content)?,prepared.thumbnail])?;
            if let Some(audio)=audio {tx.execute("INSERT OR IGNORE INTO message_audio(message_id,original) SELECT id,?3 FROM messages WHERE chat_id=?1 AND sender='#self' AND wire_id=?2 AND raw=?4",params![chat,id,audio.as_ref(),prepared.raw])?;}
            if let Some(reply)=reply {tx.execute("DELETE FROM reply_drafts WHERE chat_id=?1 AND message_id=?2",params![chat,reply])?;}
            tx.commit()?;Ok(())
        }).await?;
        self.changed();
        Ok(())
    }
}
