//! One immutable projection of the loaded conversation, kept off the UI thread.
use super::*;
use crate::performance::{Metric, Probe};
use std::time::Instant;

const SELECT: &str =
    "SELECT m.id,coalesce(nullif(trim(n.saved_name),''),nullif(trim(n.profile_name),''),m.sender_name),
    m.sender,m.body,m.stamp,m.outgoing,m.delivery,m.content,m.wire_id,m.deleted,length(m.raw),
    nullif(trim(n.saved_name),'') IS NOT NULL,m.album,m.album_index
    FROM messages m LEFT JOIN contacts n ON n.jid=m.sender
    WHERE m.chat_id=?1 AND m.visible=1 AND (m.stamp,m.id)<=(?5,?6)";
fn query(append: bool) -> String {
    let after = if append {
        " AND (m.stamp,m.id)>(?3,?4)"
    } else {
        ""
    };
    format!("{SELECT}{after} ORDER BY m.stamp DESC,m.id DESC LIMIT ?2")
}

#[derive(Clone)]
pub(super) struct Rows {
    pub messages: Arc<[domain::Message]>,
    pub transcript: Arc<str>,
    pub total: usize,
}
#[derive(PartialEq, Eq)]
struct Key {
    chat: i32,
    limit: usize,
    group: bool,
    end: Option<(i64, i32)>,
    utc_offset: i32,
    data_version: i64,
}
#[derive(Default)]
pub(super) struct Cache(Option<(Key, Rows)>);

/// The single TEMP marker participates in the same transaction as archive writes:
/// a rollback keeps the cached projection valid. Unrelated message writes leave
/// it alone. Shared names/control state invalidate it conservatively. Nothing is
/// added to the persistent schema, and invalidation does not retain message data.
pub(super) fn install(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TEMP TABLE heya_timeline_cache (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1), chat_id INTEGER NOT NULL,
            stamp INTEGER NOT NULL, row_id INTEGER NOT NULL, appended INTEGER NOT NULL DEFAULT 0
        );",
    )?;
    // Ordinary, strictly later messages can extend the cached window. An
    // out-of-order insert or invisible event needs the complete projection.
    db.execute_batch(
        "CREATE TEMP TRIGGER heya_timeline_append AFTER INSERT ON main.messages
         BEGIN
           DELETE FROM heya_timeline_cache WHERE chat_id=NEW.chat_id
             AND NOT (NEW.visible=1 AND row_id>0 AND (NEW.stamp,NEW.id)>(stamp,row_id));
           UPDATE heya_timeline_cache SET appended=appended+1 WHERE chat_id=NEW.chat_id;
         END;",
    )?;
    // These identifiers are fixed application tables, never caller input.
    for table in [
        "messages",
        "reactions",
        "contacts",
        "aliases",
        "metadata",
        "message_controls",
    ] {
        for (event, condition) in [
            ("INSERT", "chat_id=NEW.chat_id"),
            ("DELETE", "chat_id=OLD.chat_id"),
            ("UPDATE", "chat_id=OLD.chat_id OR chat_id=NEW.chat_id"),
        ] {
            if table == "messages" && event == "INSERT" {
                continue;
            }
            // Read/unread flags are not timeline fields. In particular, marking
            // a newly arrived message unread must not defeat the append path.
            let trigger = format!("heya_timeline_{table}_{event}");
            let event = if table == "messages" && event == "UPDATE" {
                "UPDATE OF id,chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,deleted,expires,delivery,content,album,album_index"
            } else {
                event
            };
            let filter = if matches!(table, "messages" | "reactions") {
                format!(" WHERE {condition}")
            } else {
                String::new()
            };
            db.execute_batch(&format!(
                "CREATE TEMP TRIGGER {trigger} AFTER {event} ON main.{table}
                 BEGIN DELETE FROM heya_timeline_cache{filter}; END;"
            ))?;
        }
    }
    Ok(())
}

impl Cache {
    pub(super) fn clear(&mut self, db: &Connection) -> Result<()> {
        self.0 = None;
        db.execute("DELETE FROM heya_timeline_cache", [])?;
        Ok(())
    }

    // Caller holds the connection lock before locking this cache. Keep that
    // order everywhere: the cache is never accessed independently of SQLite.
    pub(super) fn load(
        &mut self,
        db: &Connection,
        chat: i32,
        limit: usize,
        group: bool,
        end: Option<(i64, i32)>,
        probe: Option<&Probe>,
    ) -> Result<Rows> {
        let started = probe.map(|_| Instant::now());
        let key = Key {
            chat,
            limit,
            group,
            end,
            utc_offset: chrono::Local::now().offset().local_minus_utc(),
            // TEMP triggers see our connection only. Also detect commits from
            // another connection, such as a repair tool or an archive test.
            data_version: db.query_row("PRAGMA main.data_version", [], |r| r.get(0))?,
        };
        let mut cached = if let Some((previous, rows)) = &self.0
            && previous == &key
        {
            db.query_row(
                "SELECT appended FROM heya_timeline_cache WHERE chat_id=?1",
                [chat],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .map(|count| (rows.clone(), count as usize))
        } else {
            None
        };
        if end.is_some() && cached.as_ref().is_some_and(|(_, count)| *count > 0) {
            cached = None;
        }
        let boundary = end.unwrap_or((i64::MAX, i32::MAX));
        if let Some((rows, 0)) = &cached {
            if let (Some(probe), Some(started)) = (probe, started) {
                probe.record(Metric::TimelineReuse, started.elapsed());
            }
            return Ok(rows.clone());
        }
        if let Some((rows, _)) = &cached {
            let last = rows.messages.last().ok_or(Error::Database)?;
            // A new row can change earlier derived state if its wire key is a
            // reaction target/event or a pending control event. Check only the
            // appended range, with indexed lookups into those shared tables.
            let linked: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages m WHERE m.chat_id=?1 AND m.visible=1
                 AND (m.stamp,m.id)>(?2,?3) AND (
                   EXISTS(SELECT 1 FROM reactions r WHERE r.chat_id=m.chat_id AND r.target=m.wire_id)
                   OR EXISTS(SELECT 1 FROM reactions r WHERE r.chat_id=m.chat_id AND r.event_wire=m.wire_id)
                   OR EXISTS(SELECT 1 FROM message_controls c WHERE c.event_wire=m.wire_id)))",
                params![chat,last.content.metadata.stamp,last.id], |r|r.get(0),
            )?;
            if linked {
                cached = None;
            }
        }
        let total = if let Some((rows, count)) = &cached {
            rows.total + count
        } else {
            db.query_row(
                "SELECT count(*) FROM messages WHERE chat_id=?1 AND visible=1 AND (stamp,id)<=(?2,?3)",
                params![chat,boundary.0,boundary.1],
                |r| r.get::<_, i64>(0),
            )? as usize
        };
        let projection_limit = cached
            .as_ref()
            .map_or(limit, |(_, count)| limit.min(*count));
        let mut projections = actions::Projection::load(db, chat, projection_limit, boundary)?;
        // Read the timeline index backwards and reverse the row vector.
        // Avoid materializing SELECT * (including envelopes/thumbnails) and a
        // second SQL sort just to restore chronological display order.
        let mut stmt = db.prepare(&query(cached.is_some()))?;
        let mut album_counts =
            std::collections::HashMap::<String, std::collections::HashMap<String, usize>>::new();
        let decode = |r: &rusqlite::Row<'_>| {
            let outgoing: bool = r.get(5)?;
            let name: String = r.get(1)?;
            let sender: String = r.get(2)?;
            let status: String = r.get(6)?;
            let when = crate::message_time::local(r.get(4)?);
            let mut content =
                serde_json::from_str::<crate::message_content::Content>(&r.get::<_, String>(7)?)
                    .unwrap_or_default();
            if let Some(parent) = r.get::<_, Option<String>>(12)?
                && content.photo.is_some()
            {
                let count = if let Some(count) = album_counts
                    .get(&sender)
                    .and_then(|albums| albums.get(&parent))
                {
                    *count
                } else {
                    let count = db.query_row(&format!(
                        "SELECT count(*) FROM messages m WHERE m.chat_id=?1 AND m.sender=?2 AND m.album=?3 AND {}",
                        photos::ELIGIBLE,
                    ), params![chat,sender,parent], |r| r.get::<_,i64>(0))?;
                    let count = count as usize;
                    album_counts
                        .entry(sender.clone())
                        .or_default()
                        .insert(parent.clone(), count);
                    count
                };
                content.album = Some(crate::photo_albums::Group {
                    key: crate::photo_albums::Key::Explicit(parent),
                    index: r.get(13)?,
                });
                content.album_count = count;
            }
            content.metadata = crate::message_actions::Metadata {
                wire: r.get(8)?,
                stamp: r.get(4)?,
                delivery: status.clone(),
                ..Default::default()
            };
            content.unavailable = r.get::<_, bool>(9)?
                || (r.get::<_, i64>(10)? == 0
                    && (!outgoing
                        || !matches!(status.as_str(), "accepted" | "delivered" | "read")));
            if !r.get::<_, bool>(9)? {
                projections
                    .apply(db, &r.get::<_, String>(8)?, &mut content)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
            }
            let mut stamp = when
                .map(|t| t.format("%H:%M").to_string())
                .unwrap_or_default();
            if outgoing && !status.is_empty() {
                stamp.push_str(" · ");
                stamp.push_str(delivery_label(&status, group));
            }
            if !content.metadata.pending.is_empty() {
                stamp.push_str(" · ");
                stamp.push_str(&content.metadata.pending);
            }
            Ok(domain::Message {
                id: r.get(0)?,
                sender: if outgoing {
                    "You".into()
                } else if group {
                    domain::participant_name(&name, r.get(11)?, &sender)
                } else if name.is_empty() {
                    sender.split('@').next().unwrap_or(&sender).into()
                } else {
                    name
                },
                sender_id: sender,
                text: r.get(3)?,
                time: stamp,
                date: when.map(|t| t.date_naive()),
                outgoing,
                local: false,
                content: Arc::new(content),
            })
        };
        let rows = if let Some((old, _)) = &cached {
            let last = old.messages.last().ok_or(Error::Database)?;
            stmt.query_map(
                params![
                    chat,
                    limit as i64,
                    last.content.metadata.stamp,
                    last.id,
                    boundary.0,
                    boundary.1
                ],
                decode,
            )?
        } else {
            stmt.query_map(
                params![
                    chat,
                    limit as i64,
                    rusqlite::types::Null,
                    rusqlite::types::Null,
                    boundary.0,
                    boundary.1
                ],
                decode,
            )?
        };
        let mut messages = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        messages.reverse();
        let mut dropped = 0;
        let mut appended_start = 0;
        if let Some((old, _)) = &cached {
            dropped = old
                .messages
                .len()
                .saturating_sub(limit.saturating_sub(messages.len()));
            let retained = &old.messages[dropped..];
            appended_start = retained.len();
            let mut combined = Vec::with_capacity(retained.len() + messages.len());
            combined.extend_from_slice(retained);
            combined.append(&mut messages);
            messages = combined;
        }
        // A strict text append cannot change existing photo membership. Keep
        // the cached metadata; edits/reactions/deletions already force rebuilds.
        if cached.is_none()
            || messages[appended_start..]
                .iter()
                .any(|m| m.content.photo.is_some())
        {
            photo_groups::annotate(db, chat, &mut messages)?;
        }
        if let (Some(probe), Some(started)) = (probe, started) {
            probe.record(
                if cached.is_some() {
                    Metric::TimelineAppend
                } else {
                    Metric::TimelineBuild
                },
                started.elapsed(),
            );
        }
        let started = probe.map(|_| Instant::now());
        let mut transcript = if let Some((old, _)) = &cached {
            // Calculate byte offsets from the removed messages, never by
            // splitting user text (which can itself contain blank lines).
            let skip = if dropped == old.messages.len() {
                old.transcript.len()
            } else {
                old.messages[..dropped]
                    .iter()
                    .map(|m| m.transcript_line().len() + 2)
                    .sum()
            };
            old.transcript[skip..].to_owned()
        } else {
            String::new()
        };
        for message in &messages[appended_start..] {
            if !transcript.is_empty() {
                transcript.push_str("\n\n");
            }
            transcript.push_str(&message.transcript_line());
        }
        if let (Some(probe), Some(started)) = (probe, started) {
            probe.record(Metric::TranscriptBuild, started.elapsed());
        }
        let (stamp, row_id) = messages
            .last()
            .map(|m| (m.content.metadata.stamp, m.id))
            .unwrap_or((0, 0));
        let mut rows = Rows {
            messages: messages.into(),
            transcript: transcript.into(),
            total,
        };
        // Keep one owner of payload reuse. A conservative invalidation can
        // rebuild identical data; retain the old Arcs here, so subsequent cache
        // hits also share the snapshot's identity without repeated comparisons.
        if let Some((previous, old)) = &self.0
            && previous.chat == chat
        {
            if old.messages == rows.messages {
                rows.messages = old.messages.clone();
            }
            if old.transcript == rows.transcript {
                rows.transcript = old.transcript.clone();
            }
        }
        db.execute(
            "INSERT OR REPLACE INTO heya_timeline_cache(singleton,chat_id,stamp,row_id) VALUES(1,?1,?2,?3)",
            params![chat,stamp,row_id],
        )?;
        self.0 = Some((key, rows.clone()));
        Ok(rows)
    }
}

#[cfg(test)]
#[path = "archive_timeline_tests.rs"]
mod tests;
