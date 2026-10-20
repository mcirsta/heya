use super::*;
use crate::chat_management::{Setting, Target, Update};

pub(super) fn save(db: &Connection, update: Update) -> Result<()> {
    if update.jid.is_empty()
        || (update.setting != Setting::Mute && !matches!(update.value, 0 | 1))
        || update.value < -1
    {
        return Err(Error::Invalid);
    }
    let jid = canonical(db, &update.jid)?;
    db.execute(
        "INSERT INTO chat_settings(jid,kind,value,stamp) VALUES(?1,?2,?3,?4)
        ON CONFLICT(jid,kind) DO UPDATE SET value=excluded.value,stamp=excluded.stamp
        WHERE chat_settings.stamp IS NULL OR excluded.stamp>=chat_settings.stamp",
        params![jid, update.setting as i32, update.value, update.stamp],
    )?;
    Ok(())
}

pub(super) fn unlocked(db: &Connection, chat: i32) -> Result<()> {
    let locked: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM chats c JOIN chat_settings s ON s.jid=c.jid AND s.kind=2 WHERE c.id=?1 AND s.value=1)",[chat],|r|r.get(0))?;
    if locked { Err(Error::Invalid) } else { Ok(()) }
}

pub(super) fn merge(db: &Connection, lid: &str, pn: &str) -> Result<()> {
    db.execute(
        "INSERT INTO chat_settings(jid,kind,value,stamp)
        SELECT ?2,kind,value,stamp FROM chat_settings WHERE jid=?1
        ON CONFLICT(jid,kind) DO UPDATE SET value=excluded.value,stamp=excluded.stamp
        WHERE chat_settings.stamp IS NULL OR excluded.stamp>chat_settings.stamp",
        params![lid, pn],
    )?;
    db.execute("DELETE FROM chat_settings WHERE jid=?1", [lid])?;
    Ok(())
}

impl Archive {
    pub async fn save_chat_settings(&self, updates: Vec<Update>) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        self.call(move |db| {
            let tx = db.transaction()?;
            for update in updates {
                save(&tx, update)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }

    pub async fn chat_management_target(&self, chat: i32) -> Result<Target> {
        self.call(move |db| {
            let chat=local_chat(db,chat)?;
            unlocked(db,chat)?;
            let jid: String=db.query_row("SELECT jid FROM chats WHERE id=?1 AND listed=1",[chat],|r|r.get(0))?;
            let range=db.query_row("SELECT stamp,wire_id,sender,outgoing FROM messages WHERE chat_id=?1 AND visible=1 AND (outgoing=0 OR delivery IN ('accepted','delivered','read')) ORDER BY stamp DESC,id DESC LIMIT 1",[chat],|r|Ok(ReadSync {
                chat,jid:jid.clone(),through:r.get(0)?,version:0,message:Some(ReadMessage {wire:r.get(1)?,sender:r.get(2)?,outgoing:r.get(3)?})
            })).optional()?;
            Ok(Target{jid,range})
        }).await
    }

    pub async fn begin_chat_settings(&self) -> Result<bool> {
        self.call(|db| {
            let tx=db.transaction()?;
            let clean:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='chat_settings_clean' AND value='1')",[],|r|r.get(0))?;
            tx.execute("INSERT OR REPLACE INTO metadata(key,value) VALUES('chat_settings_clean','0')",[])?;
            tx.commit()?;
            Ok(!clean)
        }).await
    }
    pub async fn finish_chat_settings(&self) -> Result<()> {
        self.call(|db| {
            db.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('chat_settings_clean','1')",
                [],
            )?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
#[path = "archive_chat_settings_tests.rs"]
mod tests;
