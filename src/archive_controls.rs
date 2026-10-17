//! Durable message controls, local tombstones and receipt evidence.
use super::*;
use crate::message_actions::{self as action, Control, ControlKind, Info, InfoRow, Source};

fn kind(value: &str) -> Result<ControlKind> {
    match value {
        "edit" => Ok(ControlKind::Edit),
        "revoke" => Ok(ControlKind::Revoke),
        "hide" => Ok(ControlKind::Hide),
        _ => Err(Error::Invalid),
    }
}
pub(super) fn payload(db: &Connection, wire: &str) -> Result<Option<Control>> {
    let value = db.query_row("SELECT kind,target,sender,outgoing,stamp,text,version FROM message_controls WHERE event_wire=?1", [wire], |r| {
        Ok((r.get::<_,String>(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))
    }).optional()?;
    value
        .map(|(k, wire, sender, outgoing, stamp, text, version)| {
            Ok(Control {
                kind: kind(&k)?,
                wire,
                sender,
                outgoing,
                stamp,
                text,
                version,
            })
        })
        .transpose()
}
fn local_source(db: &Connection, chat: i32, message: i32) -> Result<Source> {
    db.query_row("SELECT m.id,m.chat_id,c.jid,m.source_sender,m.wire_id,m.outgoing,m.body,m.raw FROM messages m JOIN chats c ON c.id=m.chat_id WHERE m.id=?1 AND m.chat_id=?2 AND m.visible=1 AND json_extract(nullif(m.content,''),'$.notice') IS NULL", params![message,chat], |r| {
        Ok(Source{id:r.get(0)?,chat:r.get(1)?,jid:r.get(2)?,sender:r.get(3)?,wire:r.get(4)?,outgoing:r.get(5)?,body:r.get(6)?,raw:r.get(7)?})
    }).optional()?.ok_or(Error::Invalid)
}
fn queue(
    db: &Connection,
    source: &Source,
    event: &str,
    control: ControlKind,
    text: &str,
) -> Result<()> {
    let (stamp, status, expires): (i64, String, Option<i64>) = db.query_row(
        "SELECT stamp,delivery,expires FROM messages WHERE id=?1",
        [source.id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let now = chrono::Utc::now();
    if control != ControlKind::Hide {
        let window = if control == ControlKind::Edit {
            action::EDIT_WINDOW
        } else {
            action::REVOKE_WINDOW
        };
        if !source.outgoing
            || !matches!(status.as_str(), "accepted" | "delivered" | "read")
            || !action::within_window(stamp, now.timestamp(), window)
        {
            return Err(Error::Invalid);
        }
        let busy:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM message_controls c JOIN messages m ON m.wire_id=c.event_wire AND m.sender='#self' WHERE m.chat_id=?1 AND c.target=?2 AND c.kind IN ('edit','revoke') AND m.delivery IN ('queued','sending','uncertain'))",params![source.chat,source.wire],|r|r.get(0))?;
        if busy {
            return Err(Error::Invalid);
        }
    }
    let label = match control {
        ControlKind::Edit => "edit",
        ControlKind::Revoke => "revoke",
        ControlKind::Hide => "hide",
    };
    let previous:Option<i64>=db.query_row("SELECT max(version) FROM (SELECT version FROM mutations WHERE chat_id=?1 AND sender='#self' AND wire_id=?2 UNION ALL SELECT c.version FROM message_controls c JOIN messages m ON m.wire_id=c.event_wire AND m.sender='#self' WHERE m.chat_id=?1 AND c.target=?2)",params![source.chat,source.wire],|r|r.get(0))?;
    let version = now
        .timestamp_millis()
        .max(previous.unwrap_or(0).saturating_add(1));
    db.execute("INSERT INTO messages(chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,delivery,source_chat,source_sender,expires) VALUES(?1,'#self',?2,?3,1,'You','',X'',0,'queued',?4,'#self',?5)",params![source.chat,event,now.timestamp(),source.jid,if control==ControlKind::Hide {None}else{expires}])?;
    db.execute("INSERT INTO message_controls(event_wire,kind,target,sender,outgoing,stamp,text,version) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![event,label,source.wire,source.sender,source.outgoing,stamp,text,version])?;
    Ok(())
}
pub(super) fn hide(db: &Connection, chat: i32, sender: &str, wire: &str) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO message_hides(chat_id,sender,wire_id) VALUES(?1,?2,?3)",
        params![chat, sender, wire],
    )?;
    hide_if_needed(db, chat, sender, wire)
}
pub(super) fn hide_if_needed(db: &Connection, chat: i32, sender: &str, wire: &str) -> Result<()> {
    let hidden: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM message_hides WHERE chat_id=?1 AND sender=?2 AND wire_id=?3)",
        params![chat, sender, wire],
        |r| r.get(0),
    )?;
    if hidden {
        if sender == "#self" {
            db.execute("UPDATE messages SET delivery='failed' WHERE chat_id=?1 AND sender='#self' AND delivery='queued' AND wire_id IN (SELECT event_wire FROM message_controls WHERE target=?2 AND kind IN ('edit','revoke'))",params![chat,wire])?;
            db.execute("UPDATE message_controls SET text='' WHERE target=?2 AND event_wire IN (SELECT wire_id FROM messages WHERE chat_id=?1 AND sender='#self')",params![chat,wire])?;
        }
        db.execute("DELETE FROM reply_drafts WHERE message_id IN (SELECT id FROM messages WHERE chat_id=?1 AND sender=?2 AND wire_id=?3)",params![chat,sender,wire])?;
        db.execute("UPDATE messages SET visible=0,body='',raw=X'',content='',thumbnail=X'',unread=0,seen=1,delivery=CASE WHEN delivery='queued' THEN 'failed' ELSE delivery END WHERE chat_id=?1 AND sender=?2 AND wire_id=?3",params![chat,sender,wire])?;
        db.execute("UPDATE mutations SET body=NULL,raw=X'',deleted=1 WHERE chat_id=?1 AND sender=?2 AND wire_id=?3",params![chat,sender,wire])?;
        reads::recount(db, chat)?;
    }
    Ok(())
}
pub(super) fn settle_hides(db: &Connection, chat: i32) -> Result<()> {
    db.execute("DELETE FROM reply_drafts WHERE message_id IN (SELECT m.id FROM messages m JOIN message_hides h USING(chat_id,sender,wire_id) WHERE m.chat_id=?1)",[chat])?;
    db.execute("UPDATE messages AS m SET visible=0,body='',raw=X'',content='',thumbnail=X'',unread=0,seen=1 WHERE m.chat_id=?1 AND EXISTS(SELECT 1 FROM message_hides h WHERE h.chat_id=m.chat_id AND h.sender=m.sender AND h.wire_id=m.wire_id)",[chat])?;
    db.execute("UPDATE mutations AS x SET body=NULL,raw=X'',deleted=1 WHERE x.chat_id=?1 AND EXISTS(SELECT 1 FROM message_hides h WHERE h.chat_id=x.chat_id AND h.sender=x.sender AND h.wire_id=x.wire_id)",[chat])?;
    reads::recount(db, chat)?;
    Ok(())
}
pub(super) fn accepted(db: &Connection, chat: i32, event: &str, status: &str) -> Result<()> {
    let Some(control) = payload(db, event)? else {
        return Ok(());
    };
    if control.kind == ControlKind::Hide {
        // App-state deletion is idempotent, so a disconnected/ambiguous attempt
        // can safely retry. Edits/revokes never blindly retry an uncertain send.
        db.execute("UPDATE message_controls SET retry_at=unixepoch()+30 WHERE event_wire=?1 AND ?2='uncertain'",params![event,status])?;
    }
    if !matches!(status, "accepted" | "delivered" | "read") {
        return Ok(());
    }
    if db.execute(
        "UPDATE message_controls SET applied=1 WHERE event_wire=?1 AND applied=0",
        [event],
    )? == 0
    {
        return Ok(());
    }
    if control.kind == ControlKind::Hide {
        return Ok(());
    }
    let body = (control.kind == ControlKind::Edit).then_some(control.text);
    db.execute("INSERT INTO mutations(chat_id,sender,wire_id,body,version,deleted) VALUES(?1,'#self',?2,?3,?4,?5) ON CONFLICT(chat_id,sender,wire_id) DO UPDATE SET body=CASE WHEN mutations.deleted=1 THEN NULL ELSE excluded.body END,raw=X'',version=max(mutations.version,excluded.version),deleted=max(mutations.deleted,excluded.deleted) WHERE excluded.deleted=1 OR excluded.version>mutations.version",params![chat,control.wire,body,control.version,control.kind==ControlKind::Revoke])?;
    apply_mutations(db, chat)?;
    hide_if_needed(db, chat, "#self", &control.wire)?;
    db.execute(
        "UPDATE message_controls SET text='' WHERE event_wire=?1",
        [event],
    )?;
    Ok(())
}
pub(super) fn pending(
    db: &Connection,
    chat: i32,
    limit: usize,
) -> Result<std::collections::HashMap<String, String>> {
    let mut q=db.prepare("SELECT c.target,c.kind,e.delivery FROM message_controls c JOIN messages e ON e.wire_id=c.event_wire AND e.sender='#self' WHERE e.chat_id=?1 AND c.kind<>'hide' AND c.target IN (SELECT wire_id FROM messages WHERE chat_id=?1 AND visible=1 ORDER BY stamp DESC,id DESC LIMIT ?2) ORDER BY c.version,e.id")?;
    let mut result = std::collections::HashMap::new();
    for row in q.query_map(params![chat, limit as i64], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (target, k, status) = row?;
        result.insert(target, pending_label(kind(&k)?, &status));
    }
    Ok(result)
}
fn pending_label(kind: ControlKind, status: &str) -> String {
    let label = kind.label();
    match status {
        "queued" | "sending" => format!("{label} pending"),
        "uncertain" => format!("{label} unconfirmed"),
        "failed" => format!("{label} failed"),
        _ => String::new(),
    }
}

impl Archive {
    pub async fn queue_edit(
        &self,
        chat: i32,
        message: i32,
        event: String,
        expected: String,
        text: String,
    ) -> Result<()> {
        if text.trim().is_empty() || text.len() > 64 * 1024 {
            return Err(Error::Invalid);
        }
        self.call(move |db| {
            let tx = db.transaction()?;
            let source = actions::source(&tx, chat, message)?;
            if source.body != expected || !crate::protocol_actions::editable(&source) {
                return Err(Error::Invalid);
            }
            queue(&tx, &source, &event, ControlKind::Edit, &text)?;
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }
    pub async fn queue_revoke(&self, chat: i32, message: i32, event: String) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            let source = actions::source(&tx, chat, message)?;
            queue(&tx, &source, &event, ControlKind::Revoke, "")?;
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }
    pub async fn queue_hides(
        &self,
        chat: i32,
        messages: Vec<i32>,
        events: Vec<String>,
    ) -> Result<()> {
        if messages.is_empty()
            || messages.len() > action::MAX_SELECTION
            || messages.len() != events.len()
        {
            return Err(Error::Invalid);
        }
        self.call(move |db| {
            let tx = db.transaction()?;
            let chat = local_chat(&tx, chat)?;
            let mut seen = std::collections::HashSet::new();
            for (id, event) in messages.into_iter().zip(events) {
                if !seen.insert(id) {
                    return Err(Error::Invalid);
                }
                let source = local_source(&tx, chat, id)?;
                queue(&tx, &source, &event, ControlKind::Hide, "")?;
                let sender = if source.outgoing {
                    "#self".into()
                } else {
                    canonical(&tx, &source.sender)?
                };
                hide(&tx, chat, &sender, &source.wire)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }
    pub async fn hide_remote(
        &self,
        jid: String,
        sender: String,
        wire: String,
        outgoing: bool,
    ) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            let chat = reads::ensure(&tx, &jid)?;
            let sender = if outgoing {
                "#self".into()
            } else {
                canonical(&tx, &sender)?
            };
            hide(&tx, chat, &sender, &wire)?;
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }
    pub async fn queue_forwards(
        &self,
        chat: i32,
        messages: Vec<i32>,
        destination: i32,
        events: Vec<String>,
    ) -> Result<()> {
        if messages.is_empty()
            || messages.len() > action::MAX_SELECTION
            || messages.len() != events.len()
        {
            return Err(Error::Invalid);
        }
        self.call(move |db| {
            let tx = db.transaction()?;
            let mut seen = std::collections::HashSet::new();
            for (id, event) in messages.into_iter().zip(events) {
                if !seen.insert(id) {
                    return Err(Error::Invalid);
                }
                let source = actions::source(&tx, chat, id)?;
                actions::queue(
                    &tx,
                    &source,
                    destination,
                    event,
                    action::Operation::Forward,
                    false,
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }
    pub async fn recipient_receipts(
        &self,
        jid: String,
        wires: Vec<String>,
        recipient: String,
        status: &'static str,
        at: i64,
    ) -> Result<()> {
        if at <= 0 || !matches!(status, "delivered" | "read" | "played") {
            return Ok(());
        }
        self.call(move|db|{let tx=db.transaction()?;let jid=canonical(&tx,&jid)?;let recipient=canonical(&tx,&recipient)?;
            let chat=reads::ensure(&tx,&jid)?;
            let column=match status {"delivered"=>"delivered","read"=>"read",_=>"played"};
            for wire in wires {tx.execute(&format!("INSERT INTO message_receipts(chat_id,wire_id,recipient,{column}) VALUES(?1,?2,?3,?4) ON CONFLICT(chat_id,wire_id,recipient) DO UPDATE SET {column}=min(coalesce(message_receipts.{column},excluded.{column}),excluded.{column})"),params![chat,wire,recipient,at])?;}
            tx.commit()?;Ok(())}).await?;
        self.changed();
        Ok(())
    }
    pub async fn message_info(&self, chat: i32, message: i32) -> Result<Info> {
        self.call(move|db|{let chat=local_chat(db,chat)?;let source=local_source(db,chat,message)?;
            if !source.outgoing {return Err(Error::Invalid);}
            let (stamp,status):(i64,String)=db.query_row("SELECT stamp,delivery FROM messages WHERE id=?1",[message],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let mut q=db.prepare("SELECT coalesce(nullif(c.saved_name,''),nullif(c.profile_name,''),r.recipient),r.delivered,r.read,r.played FROM message_receipts r LEFT JOIN contacts c ON c.jid=r.recipient WHERE r.chat_id=?1 AND r.wire_id=?2 ORDER BY coalesce(r.read,r.delivered,r.played),r.recipient LIMIT 2048")?;
            let rows=q.query_map(params![chat,source.wire],|r|{
                let mut lines=Vec::new();
                for (column,label) in [(2,"Read"),(1,"Delivered"),(3,"Played")] {if let Some(at)=r.get::<_,Option<i64>>(column)? {lines.push(format!("{label} · {}",time(at)));}}
                Ok(InfoRow{name:r.get(0)?,detail:lines.join("\n")})
            })?.collect::<std::result::Result<Vec<_>,_>>()?;
            let pending:Option<(String,String)>=db.query_row("SELECT c.kind,e.delivery FROM message_controls c JOIN messages e ON e.wire_id=c.event_wire AND e.sender='#self' WHERE e.chat_id=?1 AND c.target=?2 AND c.kind<>'hide' ORDER BY c.version DESC,e.id DESC LIMIT 1",params![chat,source.wire],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let pending=match pending {Some((k,s))=>pending_label(kind(&k)?,&s),None=>String::new()};
            Ok(Info{message,wire:source.wire,sent:time(stamp),status:if pending.is_empty(){delivery_label(&status,false).into()}else{pending},rows})
        }).await
    }
}
fn time(stamp: i64) -> String {
    crate::message_time::local(stamp)
        .map(|v| v.format("%d %b %Y, %H:%M:%S").to_string())
        .unwrap_or_else(|| "Time unavailable".into())
}

#[cfg(test)]
#[path = "message_control_tests.rs"]
mod tests;

pub(super) fn merge(
    db: &Connection,
    column: &str,
    from: &dyn rusqlite::ToSql,
    to: &dyn rusqlite::ToSql,
) -> Result<()> {
    assert!(matches!(column, "chat_id" | "sender"));
    let chat = if column == "chat_id" { "?2" } else { "chat_id" };
    let sender = if column == "sender" { "?2" } else { "sender" };
    db.execute(&format!("INSERT OR IGNORE INTO message_hides(chat_id,sender,wire_id) SELECT {chat},{sender},wire_id FROM message_hides WHERE {column}=?1"),[from,to])?;
    db.execute(
        &format!("DELETE FROM message_hides WHERE {column}=?1"),
        [from],
    )?;
    let recipient = if column == "sender" {
        "?2"
    } else {
        "recipient"
    };
    let key = if column == "sender" {
        "recipient"
    } else {
        "chat_id"
    };
    db.execute(&format!("INSERT INTO message_receipts(chat_id,wire_id,recipient,delivered,read,played) SELECT {chat},wire_id,{recipient},delivered,read,played FROM message_receipts WHERE {key}=?1 ON CONFLICT(chat_id,wire_id,recipient) DO UPDATE SET delivered=min(coalesce(message_receipts.delivered,excluded.delivered),coalesce(excluded.delivered,message_receipts.delivered)),read=min(coalesce(message_receipts.read,excluded.read),coalesce(excluded.read,message_receipts.read)),played=min(coalesce(message_receipts.played,excluded.played),coalesce(excluded.played,message_receipts.played))"),[from,to])?;
    db.execute(
        &format!("DELETE FROM message_receipts WHERE {key}=?1"),
        [from],
    )?;
    Ok(())
}
