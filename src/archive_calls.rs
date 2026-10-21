//! Call records share timeline ordering, but never message-send/read semantics.
use super::*;
use crate::message_content::Notice;

impl Archive {
    pub async fn call_target(&self, chat: i32) -> Result<crate::calling::Target> {
        self.call(move|db|{
            let chat=local_chat(db,chat)?;
            chat_settings::unlocked(db,chat)?;
            let (peer,title,readonly):(String,String,bool)=db.query_row("SELECT c.jid,coalesce(nullif(n.saved_name,''),nullif(n.profile_name,''),nullif(c.name,''),c.jid),c.read_only FROM chats c LEFT JOIN contacts n ON n.jid=c.jid WHERE c.id=?1",[chat],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let own:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='identity' AND value=?1)",[&peer],|r|r.get(0))?;
            if readonly||own||!peer.ends_with("@s.whatsapp.net")&&!peer.ends_with("@lid") {return Err(Error::Invalid);}
            Ok(crate::calling::Target{chat,peer,title})
        }).await
    }
    pub async fn incoming_call_target(&self, peer: String) -> Result<crate::calling::Target> {
        let chat = self.create_chat(peer).await?;
        self.call_target(chat).await
    }
    pub fn call_account(&self) -> Result<Vec<String>> {
        self.blocking(|db| {
            let mut q=db.prepare("SELECT value FROM metadata WHERE key='identity' UNION SELECT alias FROM aliases WHERE canonical=(SELECT value FROM metadata WHERE key='identity')")?;
            Ok(q.query_map([],|r|r.get(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })
    }
    pub async fn needs_call_snapshot(&self) -> Result<bool> {
        self.call(|db| {
            Ok(!db.query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='calls_snapshot' AND value='1')",
                [],
                |r| r.get::<_, bool>(0),
            )?)
        })
        .await
    }
    pub async fn finish_call_snapshot(&self) -> Result<()> {
        self.call(|db| {
            db.execute(
                "INSERT OR REPLACE INTO metadata VALUES('calls_snapshot','1')",
                [],
            )?;
            Ok(())
        })
        .await
    }
}

pub(super) fn update(db: &Connection, row: &Record, chat: i32, sender: &str) -> Result<()> {
    let Some(Notice::Call(mut call)) = row.content.notice else {
        return Ok(());
    };
    let old=db.query_row("SELECT content FROM messages WHERE chat_id=?1 AND sender=?2 AND wire_id=?3 AND deleted=0 AND visible=1",params![chat,sender,row.id],|r|r.get::<_,String>(0)).optional()?;
    let Some(old) =
        old.and_then(|s| serde_json::from_str::<crate::message_content::Content>(&s).ok())
    else {
        return Ok(());
    };
    let Some(Notice::Call(previous)) = old.notice else {
        return Ok(());
    };
    if call.revision < previous.revision {
        return Ok(());
    }
    if matches!(
        call.outcome,
        crate::message_content::CallOutcome::Unknown
            | crate::message_content::CallOutcome::Ongoing
            | crate::message_content::CallOutcome::Scheduled
    ) && !matches!(
        previous.outcome,
        crate::message_content::CallOutcome::Unknown
            | crate::message_content::CallOutcome::Ongoing
            | crate::message_content::CallOutcome::Scheduled
    ) {
        return Ok(());
    }
    call.video = call.video.or(previous.video);
    call.seconds = call.seconds.or(previous.seconds);
    let content = crate::message_content::Content {
        notice: Some(Notice::Call(call)),
        ..Default::default()
    };
    let encoded = encode_content(&content)?;
    db.execute("UPDATE messages SET content=?4,body=?5,stamp=?6 WHERE chat_id=?1 AND sender=?2 AND wire_id=?3 AND (content<>?4 OR body<>?5 OR stamp<>?6)",params![chat,sender,row.id,encoded,call.label(),row.timestamp])?;
    Ok(())
}
