//! Confirmed message stars and pins, scoped to the canonical chat and author.
use super::*;
use crate::message_actions::{Control, ControlKind, MarkUpdate, Metadata};
#[cfg(test)]
#[path = "message_mark_tests.rs"]
mod tests;

fn save(db: &Connection, chat: i32, sender: &str, wire: &str, update: &MarkUpdate) -> Result<()> {
    if wire.is_empty() || sender.is_empty() {
        return Err(Error::Invalid);
    }
    if let Some(starred) = update.starred {
        db.execute("INSERT INTO message_marks(chat_id,sender,wire_id,starred,star_stamp) VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(chat_id,sender,wire_id) DO UPDATE SET starred=excluded.starred,star_stamp=excluded.star_stamp
            WHERE message_marks.star_stamp IS NULL OR excluded.star_stamp>=message_marks.star_stamp",
            params![chat,sender,wire,starred,update.stamp])?;
    }
    if let Some(until) = update.pinned_until {
        db.execute("INSERT INTO message_marks(chat_id,sender,wire_id,pinned_until,pin_stamp) VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(chat_id,sender,wire_id) DO UPDATE SET pinned_until=excluded.pinned_until,pin_stamp=excluded.pin_stamp
            WHERE message_marks.pin_stamp IS NULL OR excluded.pin_stamp>=message_marks.pin_stamp",
            params![chat,sender,wire,until.max(0),update.stamp])?;
    }
    Ok(())
}

pub(super) fn ingest(db: &Connection, chat: i32, record: &Record) -> Result<()> {
    if !matches!(
        record.content.notice,
        Some(
            crate::message_content::Notice::MessagePinned
                | crate::message_content::Notice::MessageUnpinned
        )
    ) {
        return Ok(());
    }
    let Some(update) = crate::protocol_actions::pin_update(record) else {
        return Ok(());
    };
    let mut sender = canonical(db, &update.sender)?;
    let own: Option<String> = db
        .query_row("SELECT value FROM metadata WHERE key='identity'", [], |r| {
            r.get(0)
        })
        .optional()?;
    if own.as_deref() == Some(&sender) {
        sender = "#self".into();
    }
    save(db, chat, &sender, &update.wire, &update)
}

pub(super) fn accepted(db: &Connection, chat: i32, control: &Control) -> Result<()> {
    let sender = if control.outgoing {
        "#self".into()
    } else {
        canonical(db, &control.sender)?
    };
    let update = MarkUpdate {
        jid: String::new(),
        sender: sender.clone(),
        wire: control.wire.clone(),
        starred: match control.kind {
            ControlKind::Star => Some(true),
            ControlKind::Unstar => Some(false),
            _ => None,
        },
        pinned_until: match control.kind {
            ControlKind::Pin => Some(
                control.version / 1000 + control.text.parse::<i64>().map_err(|_| Error::Invalid)?,
            ),
            ControlKind::Unpin => Some(0),
            _ => None,
        },
        stamp: Some(control.version),
    };
    save(db, chat, &sender, &control.wire, &update)
}

pub(super) fn project(
    db: &Connection,
    chat: i32,
    sender: &str,
    wire: &str,
    metadata: &mut Metadata,
) -> Result<()> {
    if let Some((starred, pinned)) = db.query_row(
        "SELECT starred,pinned_until>unixepoch() FROM message_marks WHERE chat_id=?1 AND sender=?2 AND wire_id=?3",
        params![chat,sender,wire], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()? {
        metadata.starred = starred;
        metadata.pinned = pinned;
    }
    Ok(())
}

pub(super) fn merge(
    db: &Connection,
    column: &str,
    from: &dyn rusqlite::ToSql,
    to: &dyn rusqlite::ToSql,
) -> Result<()> {
    assert!(matches!(column, "chat_id" | "sender"));
    let chat = if column == "chat_id" { "?2" } else { "chat_id" };
    let sender = if column == "sender" { "?2" } else { "sender" };
    db.execute(&format!("INSERT INTO message_marks(chat_id,sender,wire_id,starred,star_stamp,pinned_until,pin_stamp)
        SELECT {chat},{sender},wire_id,starred,star_stamp,pinned_until,pin_stamp FROM message_marks WHERE {column}=?1
        ON CONFLICT(chat_id,sender,wire_id) DO UPDATE SET
        starred=CASE WHEN message_marks.star_stamp IS NULL OR excluded.star_stamp>=message_marks.star_stamp THEN excluded.starred ELSE message_marks.starred END,
        star_stamp=CASE WHEN message_marks.star_stamp IS NULL OR excluded.star_stamp>=message_marks.star_stamp THEN excluded.star_stamp ELSE message_marks.star_stamp END,
        pinned_until=CASE WHEN message_marks.pin_stamp IS NULL OR excluded.pin_stamp>=message_marks.pin_stamp THEN excluded.pinned_until ELSE message_marks.pinned_until END,
        pin_stamp=CASE WHEN message_marks.pin_stamp IS NULL OR excluded.pin_stamp>=message_marks.pin_stamp THEN excluded.pin_stamp ELSE message_marks.pin_stamp END"), [from,to])?;
    db.execute(
        &format!("DELETE FROM message_marks WHERE {column}=?1"),
        [from],
    )?;
    Ok(())
}

impl Archive {
    pub async fn save_message_marks(&self, updates: Vec<MarkUpdate>) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        self.call(move |db| {
            let tx = db.transaction()?;
            for update in updates {
                let jid = canonical(&tx, &update.jid)?;
                let chat = reads::ensure(&tx, &jid)?;
                let sender = canonical(&tx, &update.sender)?;
                save(&tx, chat, &sender, &update.wire, &update)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }
    pub async fn needs_message_marks_snapshot(&self) -> Result<bool> {
        self.call(|db| Ok(!db.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='message_marks_snapshot' AND value='1')",[],|r|r.get::<_,bool>(0))?)).await
    }
    pub async fn finish_message_marks_snapshot(&self) -> Result<()> {
        self.call(|db| {
            db.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('message_marks_snapshot','1')",
                [],
            )?;
            Ok(())
        })
        .await
    }
}
