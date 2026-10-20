//! Durable explicit operations, reply drafts, and reaction projections.
use super::*;
use crate::message_actions::{Operation, ReplyDraft, Source};
#[cfg(test)]
#[path = "message_action_tests.rs"]
mod tests;

pub(super) fn source(db: &Connection, chat: i32, id: i32) -> Result<Source> {
    let chat = local_chat(db, chat)?;
    chat_settings::unlocked(db, chat)?;
    // Use the effective edited envelope for quotes/forwards; media downloads
    // retain the immutable transfer reference in messages.raw.
    let value = db.query_row(
        "SELECT m.id,m.chat_id,c.jid,m.source_sender,m.wire_id,m.outgoing,m.body,
         CASE WHEN EXISTS(SELECT 1 FROM mutations x WHERE x.chat_id=m.chat_id
           AND x.sender=m.sender AND x.wire_id=m.wire_id) THEN coalesce((SELECT x.raw FROM mutations x WHERE x.chat_id=m.chat_id AND x.sender=m.sender AND x.wire_id=m.wire_id AND x.version=m.edit_version),X'') ELSE m.raw END
         FROM messages m JOIN chats c ON c.id=m.chat_id
         WHERE m.id=?1 AND m.chat_id=?2 AND m.visible=1 AND m.deleted=0
           AND CASE WHEN json_valid(m.content) THEN json_extract(m.content,'$.notice') IS NULL ELSE 1 END
           AND (m.expires IS NULL OR m.expires>unixepoch())
           AND (length(m.raw)>0 OR (m.outgoing=1 AND m.delivery IN ('accepted','delivered','read')))",
        params![id, chat],
        |r| Ok(Source { id:r.get(0)?, chat:r.get(1)?, jid:r.get(2)?, sender:r.get(3)?,
            wire:r.get(4)?, outgoing:r.get(5)?, body:r.get(6)?, raw:r.get(7)? }),
    ).optional()?;
    value.ok_or(Error::Invalid)
}
fn display_name(db: &Connection, sender: &str, outgoing: bool) -> Result<String> {
    if outgoing || sender == "#self" {
        return Ok("You".into());
    }
    let sender = canonical(db, sender)?;
    let own: Option<String> = db
        .query_row("SELECT value FROM metadata WHERE key='identity'", [], |r| {
            r.get(0)
        })
        .optional()?;
    if own.as_deref() == Some(&sender) {
        return Ok("You".into());
    }
    Ok(db.query_row("SELECT coalesce(nullif(saved_name,''),nullif(profile_name,'')) FROM contacts WHERE jid=?1",[&sender],|r|r.get::<_,Option<String>>(0)).optional()?.flatten().unwrap_or_else(||sender.split('@').next().unwrap_or(&sender).into()))
}
pub(super) fn draft(db: &Connection, chat: i32) -> Result<Option<ReplyDraft>> {
    let id: Option<i32> = db
        .query_row(
            "SELECT message_id FROM reply_drafts WHERE chat_id=?1",
            [chat],
            |r| r.get(0),
        )
        .optional()?;
    match id {
        Some(id) => match source(db, chat, id) {
            Ok(s) => Ok(Some(ReplyDraft {
                id: s.id,
                sender: display_name(db, &s.sender, s.outgoing)?,
                text: domain::single_line_summary(&s.body),
            })),
            Err(Error::Invalid) => {
                db.execute("DELETE FROM reply_drafts WHERE chat_id=?1", [chat])?;
                Ok(None)
            }
            Err(e) => Err(e),
        },
        None => Ok(None),
    }
}
#[derive(Default)]
struct ReactionView {
    counts: std::collections::BTreeMap<String, usize>,
    own: String,
    status: String,
}
pub(super) struct Projection {
    reactions: std::collections::HashMap<String, ReactionView>,
    names: std::collections::HashMap<String, String>,
    pending: std::collections::HashMap<String, String>,
}
impl Projection {
    pub(super) fn load(
        db: &Connection,
        chat: i32,
        limit: usize,
        boundary: (i64, i32),
    ) -> Result<Self> {
        let mut value = Self {
            reactions: Default::default(),
            names: Default::default(),
            pending: controls::pending(db, chat, limit)?,
        };
        let mut q=db.prepare("SELECT r.target,r.actor,r.emoji,coalesce(m.delivery,r.delivery) FROM reactions r LEFT JOIN messages m ON r.actor='#self' AND m.chat_id=r.chat_id AND m.sender='#self' AND m.wire_id=r.event_wire WHERE r.chat_id=?1 AND r.target IN (SELECT wire_id FROM messages WHERE chat_id=?1 AND visible=1 AND deleted=0 AND (stamp,id)<=(?3,?4) ORDER BY stamp DESC,id DESC LIMIT ?2) AND (SELECT count(*) FROM messages original WHERE original.chat_id=r.chat_id AND original.wire_id=r.target AND original.visible=1)=1")?;
        for row in q.query_map(params![chat, limit as i64, boundary.0, boundary.1], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (wire, actor, emoji, status) = row?;
            let entry = value.reactions.entry(wire).or_default();
            if !emoji.is_empty() && status != "failed" {
                *entry.counts.entry(emoji.clone()).or_default() += 1;
            }
            if actor == "#self" {
                entry.own = emoji;
                entry.status = match status.as_str() {
                    "queued" | "sending" => "Sending reaction…",
                    "failed" => "Reaction failed · choose it again to retry",
                    "uncertain" => "Reaction unconfirmed · check your phone",
                    _ => "",
                }
                .into();
            }
        }
        Ok(value)
    }
    pub(super) fn apply(
        &mut self,
        db: &Connection,
        wire: &str,
        content: &mut crate::message_content::Content,
    ) -> Result<()> {
        content.metadata.pending = self.pending.get(wire).cloned().unwrap_or_default();
        if let Some(quote) = &mut content.quote {
            if !self.names.contains_key(&quote.sender) {
                self.names.insert(
                    quote.sender.clone(),
                    display_name(db, &quote.sender, false)?,
                );
            }
            quote.sender = self.names[&quote.sender].clone();
        }
        if let Some(view) = self.reactions.get(wire) {
            content.reaction_summary = view
                .counts
                .iter()
                .take(6)
                .map(|(emoji, count)| {
                    if *count > 1 {
                        format!("{emoji} {count}")
                    } else {
                        emoji.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join("  ");
            content.own_reaction = view.own.clone();
            content.reaction_status = view.status.clone();
        }
        Ok(())
    }
}
pub(super) fn reaction(
    db: &Connection,
    chat: i32,
    actor: &str,
    event: &str,
    stamp: i64,
    delivery: &str,
    value: &crate::message_content::ReactionEvent,
) -> Result<()> {
    let actor = canonical(db, actor)?;
    let at = if value.at > 0 {
        value.at
    } else {
        stamp.saturating_mul(1000)
    };
    db.execute("INSERT INTO reactions(chat_id,target,actor,emoji,version,event_wire,delivery) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(chat_id,target,actor) DO UPDATE SET emoji=excluded.emoji,version=excluded.version,event_wire=excluded.event_wire,delivery=excluded.delivery WHERE excluded.version>reactions.version OR (excluded.version=reactions.version AND excluded.event_wire=reactions.event_wire AND excluded.delivery='accepted')",params![chat,value.target,actor,value.emoji,at,event,delivery])?;
    Ok(())
}
pub(super) fn merge(
    db: &Connection,
    column: &str,
    from: &dyn rusqlite::ToSql,
    to: &dyn rusqlite::ToSql,
) -> Result<()> {
    // Callers provide only these fixed column names, never user input.
    assert!(column == "chat_id" || column == "actor");
    let chat = if column == "chat_id" { "?2" } else { "chat_id" };
    let actor = if column == "actor" { "?2" } else { "actor" };
    db.execute(&format!("INSERT INTO reactions(chat_id,target,actor,emoji,version,event_wire,delivery) SELECT {chat},target,{actor},emoji,version,event_wire,delivery FROM reactions WHERE {column}=?1 ON CONFLICT(chat_id,target,actor) DO UPDATE SET emoji=excluded.emoji,version=excluded.version,event_wire=excluded.event_wire,delivery=excluded.delivery WHERE excluded.version>reactions.version"),[from,to])?;
    db.execute(&format!("DELETE FROM reactions WHERE {column}=?1"), [from])?;
    Ok(())
}
impl Archive {
    pub async fn destinations(&self) -> Result<Vec<crate::message_actions::Destination>> {
        self.call(|db|{
            let mut q=db.prepare("SELECT c.id,coalesce(nullif(n.saved_name,''),nullif(c.name,''),nullif(n.profile_name,''),c.jid) FROM chats c LEFT JOIN contacts n ON n.jid=c.jid WHERE c.listed=1 AND c.read_only=0 AND NOT EXISTS(SELECT 1 FROM chat_settings s WHERE s.jid=c.jid AND s.kind=2 AND s.value=1) ORDER BY 2")?;
            Ok(q.query_map([],|r|Ok(crate::message_actions::Destination{id:r.get(0)?,title:r.get(1)?}))?.collect::<std::result::Result<Vec<_>,_>>()?)
        }).await
    }
    pub async fn set_reply(&self, chat: i32, message: i32) -> Result<()> {
        self.call(move|db|{let chat=local_chat(db,chat)?;if message==0 {db.execute("DELETE FROM reply_drafts WHERE chat_id=?1",[chat])?;} else {let _=source(db,chat,message)?;db.execute("INSERT INTO reply_drafts(chat_id,message_id) VALUES(?1,?2) ON CONFLICT(chat_id) DO UPDATE SET message_id=excluded.message_id",params![chat,message])?;} Ok(())}).await?;
        self.changed();
        Ok(())
    }
    pub async fn queue_operation(
        &self,
        source_chat: i32,
        message: i32,
        destination: i32,
        id: String,
        operation: Operation,
    ) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            let s = source(&tx, source_chat, message)?;
            queue(&tx, &s, destination, id, operation, false)?;
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }
    pub(super) fn queue_reply(
        db: &Connection,
        chat: i32,
        id: String,
        text: String,
    ) -> Result<bool> {
        let reply: Option<i32> = db
            .query_row(
                "SELECT message_id FROM reply_drafts WHERE chat_id=?1",
                [chat],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(reply) = reply {
            let s = source(db, chat, reply)?;
            queue(db, &s, chat, id, Operation::Reply(text), true)?;
            return Ok(true);
        }
        Ok(false)
    }
}
pub(super) fn queue(
    db: &Connection,
    s: &Source,
    destination: i32,
    id: String,
    operation: Operation,
    clear_draft: bool,
) -> Result<()> {
    let destination = local_chat(db, destination)?;
    chat_settings::unlocked(db, destination)?;
    let (read_only, ephemeral): (bool, u32) = db.query_row(
        "SELECT read_only,ephemeral FROM chats WHERE id=?1",
        [destination],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if (read_only && !matches!(operation, Operation::React(_)))
        || (!matches!(operation, Operation::Forward) && destination != s.chat)
    {
        return Err(Error::Invalid);
    }
    let owner: String =
        db.query_row("SELECT value FROM metadata WHERE key='identity'", [], |r| {
            r.get(0)
        })?;
    let now = chrono::Utc::now();
    let mut timestamp = now.timestamp_millis();
    if matches!(operation, Operation::React(_)) {
        let previous: Option<i64> = db
            .query_row(
                "SELECT version FROM reactions WHERE chat_id=?1 AND target=?2 AND actor='#self'",
                params![s.chat, s.wire],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            timestamp = timestamp.max(previous.saturating_add(1));
        }
    }
    let prepared = crate::protocol_actions::prepare(s, operation, &owner, timestamp)?;
    db.execute("INSERT INTO messages(chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,delivery,expires,source_chat,source_sender,content,thumbnail) VALUES(?1,'#self',?2,?3,1,'You',?4,?5,?6,'queued',?7,(SELECT jid FROM chats WHERE id=?1),'#self',?8,?9)",params![destination,id,now.timestamp(),prepared.text,prepared.raw,prepared.visible,(ephemeral>0).then_some(now.timestamp()+ephemeral as i64),encode_content(&prepared.content)?,prepared.thumbnail])?;
    if let Some(r) = prepared.content.reaction {
        reaction(db, destination, "#self", &id, now.timestamp(), "queued", &r)?;
    }
    if clear_draft {
        db.execute("UPDATE chats SET draft='' WHERE id=?1", [destination])?;
        db.execute("DELETE FROM reply_drafts WHERE chat_id=?1", [destination])?;
    }
    Ok(())
}
