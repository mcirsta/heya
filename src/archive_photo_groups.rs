//! Infer display albums from cached photo metadata, on the archive worker.
//! No wire IDs, envelopes, media caches or persistent rows are rewritten.
use super::*;
use crate::photo_albums::{self, Candidate};
use chrono::{Local, NaiveDate};
use std::collections::HashSet;

fn groups(db: &Connection, chat: i32, day: NaiveDate) -> Result<Vec<Vec<i32>>> {
    let range = || {
        let start = day
            .and_hms_opt(0, 0, 0)?
            .and_local_timezone(Local)
            .earliest()?;
        let end = day
            .succ_opt()?
            .and_hms_opt(0, 0, 0)?
            .and_local_timezone(Local)
            .earliest()?;
        Some((start.timestamp(), end.timestamp()))
    };
    let Some((start, end)) = range() else {
        return Ok(Vec::new());
    };
    // The date range uses the timeline index. Only photo metadata is returned;
    // the indexed predecessor lookup preserves barriers without loading text
    // messages, thumbnails or envelopes into this projection.
    let sql = format!("SELECT m.id,m.stamp,m.sender,m.outgoing,m.content,
        (SELECT p.id FROM messages p WHERE p.chat_id=m.chat_id AND p.visible=1
         AND (p.stamp,p.id)<(m.stamp,m.id) ORDER BY p.stamp DESC,p.id DESC LIMIT 1),
        EXISTS(SELECT 1 FROM reactions r LEFT JOIN messages event
          ON r.actor='#self' AND event.chat_id=r.chat_id AND event.sender='#self' AND event.wire_id=r.event_wire
          WHERE r.chat_id=m.chat_id AND r.target=m.wire_id
          AND ((r.emoji<>'' AND coalesce(event.delivery,r.delivery)<>'failed')
            OR (r.actor='#self' AND coalesce(event.delivery,r.delivery) IN ('queued','sending','failed','uncertain'))))
        FROM messages m WHERE m.chat_id=?1 AND m.stamp>=?2 AND m.stamp<?3
          AND m.album IS NULL AND {} ORDER BY m.stamp,m.id", photos::ELIGIBLE);
    let mut query = db.prepare_cached(&sql)?;
    let mut rows = query.query(params![chat, start, end])?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next()? {
        let Ok(content) =
            serde_json::from_str::<crate::message_content::Content>(&row.get::<_, String>(4)?)
        else {
            continue;
        };
        candidates.push(Candidate {
            id: row.get(0)?,
            previous: row.get(5)?,
            stamp: row.get(1)?,
            date: Some(day),
            sender: row.get(2)?,
            outgoing: row.get(3)?,
            forwarded: content.forwarded,
            separate: photo_albums::separate(&content) || row.get::<_, bool>(6)?,
        });
    }
    Ok(photo_albums::inferred(&candidates))
}

pub(super) fn members(db: &Connection, chat: i32, id: i32, stamp: i64) -> Result<Vec<i32>> {
    if let Some(when) = crate::message_time::local(stamp) {
        for group in groups(db, chat, when.date_naive())? {
            if group.contains(&id) {
                return Ok(group);
            }
        }
    }
    Ok(vec![id])
}

pub(super) fn annotate(db: &Connection, chat: i32, messages: &mut [domain::Message]) -> Result<()> {
    // An isolated loaded photo cannot form a grid. Query only dates that have
    // a potential loaded run; a viewer click resolves full membership separately.
    // Unchanged projections return before this path in the timeline cache.
    let candidates = photo_albums::candidates(messages);
    let dates: HashSet<_> = candidates
        .windows(2)
        .filter(|pair| photo_albums::joins(&pair[0], &pair[1]))
        .filter_map(|pair| pair[0].date)
        .collect();
    let mut runs = Vec::new();
    for date in dates {
        runs.extend(groups(db, chat, date)?);
    }
    // Explicit associations always win; inferred keys exist only in the view.
    photo_albums::annotate(messages, &runs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        archive::tests::{KEY, LID, PN, fixture, record},
        message_content::{Content, Photo},
        photo_albums::{Key, Layout},
    };

    fn photo(i: i32) -> Record {
        let mut row = record(&format!("unlinked-{i}"));
        row.timestamp += i64::from(i);
        row.text = "[Photo]".into();
        row.content = Content {
            body: Some(String::new()),
            forwarded: true,
            photo: Some(Photo {
                width: 900,
                height: 1200,
                size: Some(12345),
            }),
            ..Default::default()
        };
        row
    }
    async fn view(db: &Archive, limit: usize) -> domain::Conversation {
        db.view(1, limit, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
    }
    #[tokio::test]
    async fn inferred_albums_reuse_paging_gallery_cache_and_reopening_without_rewriting_records() {
        let (_temp, db) = fixture();
        db.ingest(Batch {
            messages: (0..37).map(photo).collect(),
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let first = view(&db, 37).await.messages[0].id;
        let last = view(&db, 2).await.messages[1].id;
        let request = db.photo_request(first).await.unwrap().unwrap();
        db.save_photo(
            &request,
            crate::photos::Cached {
                original: vec![1, 2, 3],
                preview: vec![4, 5, 6],
            },
        )
        .await
        .unwrap();
        let saved = db
            .blocking(|c| {
                Ok(c.query_row(
                    "SELECT sum(unread),sum(length(raw)),sum(album IS NOT NULL) FROM messages",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                        ))
                    },
                )?)
            })
            .unwrap();
        let page = view(&db, 2).await;
        assert!(
            page.messages
                .iter()
                .all(
                    |m| m.content.album.as_ref().unwrap().key == Key::Inferred(first)
                        && m.content.album_count == 37
                )
        );
        assert_eq!(Layout::new(&page.messages, true).grids[&0].total, 37);
        let gallery = db.photo_gallery(1, last).await.unwrap().unwrap();
        assert_eq!(gallery.members.len(), 37);
        assert_eq!(gallery.position, 36);
        assert_eq!(gallery.members[0], first);
        assert_eq!(
            db.clipboard_photo(1, first).await.unwrap(),
            Some(vec![1, 2, 3])
        );
        assert_eq!(
            saved,
            db.blocking(|c| Ok(c.query_row(
                "SELECT sum(unread),sum(length(raw)),sum(album IS NOT NULL) FROM messages",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            )?))
            .unwrap()
        );
        let files = db.0._files.clone();
        drop(db);
        let db = Archive::open(files, KEY).unwrap();
        assert_eq!(
            db.photo_gallery(1, last).await.unwrap().unwrap().members,
            gallery.members
        );
        assert_eq!(view(&db, 2).await.messages[0].content.album_count, 37);
        // Strict append must refresh older inferred counts even though those
        // messages and their transcript text come from the immutable cache.
        db.ingest(Batch {
            messages: vec![photo(37)],
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(
            view(&db, 2)
                .await
                .messages
                .iter()
                .all(|m| m.content.album_count == 38)
        );
        db.call(move |c| {
            c.execute(
                "UPDATE messages SET deleted=1,content='' WHERE id=?1",
                [first + 18],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let after = view(&db, 38).await;
        assert_eq!(Layout::new(&after.messages, true).grids.len(), 2);
        assert_eq!(
            db.photo_gallery(1, first)
                .await
                .unwrap()
                .unwrap()
                .members
                .len(),
            18
        );
        assert_eq!(
            db.photo_gallery(1, last)
                .await
                .unwrap()
                .unwrap()
                .members
                .len(),
            19
        );
        assert!(db.photo_gallery(1, first + 18).await.unwrap().is_none());
        assert_eq!(
            db.clipboard_photo(1, first).await.unwrap(),
            Some(vec![1, 2, 3])
        );
    }
    #[tokio::test]
    async fn text_appends_retain_album_metadata_and_break_the_next_photo_run() {
        let (_temp, db) = fixture();
        db.ingest(Batch {
            messages: (0..4).map(photo).collect(),
            ..Default::default()
        })
        .await
        .unwrap();
        let before = view(&db, 4).await;
        let mut text = photo(4);
        text.text = "A separate message".into();
        text.content = Content::default();
        db.ingest(Batch {
            messages: vec![text],
            ..Default::default()
        })
        .await
        .unwrap();
        let after = view(&db, 4).await;
        for (old, new) in before.messages[1..].iter().zip(after.messages.iter()) {
            assert_eq!(new.content.album_count, 4);
            assert!(Arc::ptr_eq(&old.content, &new.content));
        }
        assert_eq!(Layout::new(&after.messages, true).grids[&0].total, 4);
        db.ingest(Batch {
            messages: vec![photo(5)],
            ..Default::default()
        })
        .await
        .unwrap();
        let next = view(&db, 4).await;
        assert!(next.messages[3].content.album.is_none());
        assert_eq!(Layout::new(&next.messages, true).grids[&0].total, 4);
        assert_eq!(
            db.photo_gallery(1, next.messages[3].id)
                .await
                .unwrap()
                .unwrap()
                .members,
            [next.messages[3].id]
        );
    }
    #[tokio::test]
    async fn inference_respects_visible_barriers_captions_reactions_and_explicit_links() {
        let (_temp, db) = fixture();
        let mut records = (0..12).map(photo).collect::<Vec<_>>();
        records[4].content = Content::default();
        records[4].text = "Intervening text".into();
        records[7].album = Some(crate::message_content::Album {
            parent: "actual-parent".into(),
            index: Some(0),
        });
        records[10].content.body = Some("A visible caption 👩🏽‍💻".into());
        db.ingest(Batch {
            messages: records,
            ..Default::default()
        })
        .await
        .unwrap();
        let all = view(&db, 12).await;
        let ids = all.messages.iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(
            db.photo_gallery(1, ids[1]).await.unwrap().unwrap().members,
            ids[..4]
        );
        assert_eq!(
            db.photo_gallery(1, ids[5]).await.unwrap().unwrap().members,
            ids[5..7]
        );
        assert_eq!(
            db.photo_gallery(1, ids[7]).await.unwrap().unwrap().members,
            [ids[7]]
        );
        assert_eq!(
            db.photo_gallery(1, ids[8]).await.unwrap().unwrap().members,
            ids[8..10]
        );
        assert_eq!(
            db.photo_gallery(1, ids[11]).await.unwrap().unwrap().members,
            ids[10..]
        );
        assert!(matches!(
            all.messages[7].content.album.as_ref().unwrap().key,
            Key::Explicit(_)
        ));
        assert_eq!(
            all.messages[10].content.body.as_deref(),
            Some("A visible caption 👩🏽‍💻")
        );
        db.call(move |c| {
            let wire: String =
                c.query_row("SELECT wire_id FROM messages WHERE id=?1", [ids[2]], |r| {
                    r.get(0)
                })?;
            c.execute(
                "INSERT INTO reactions(chat_id,target,actor,emoji,version,event_wire)
                 VALUES(1,?1,'sample','👍',1,'reaction')",
                [wire],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let after = view(&db, 12).await;
        assert_eq!(after.messages[2].content.reaction_summary, "👍");
        assert_eq!(
            db.photo_gallery(1, after.messages[0].id)
                .await
                .unwrap()
                .unwrap()
                .members
                .len(),
            2
        );
        assert_eq!(
            db.photo_gallery(1, after.messages[2].id)
                .await
                .unwrap()
                .unwrap()
                .members
                .len(),
            2
        );
    }
    #[tokio::test]
    async fn inference_crosses_minutes_but_not_days_and_follows_aliases_and_locks() {
        use chrono::TimeZone;
        let (_temp, db) = fixture();
        let start = Local
            .with_ymd_and_hms(2026, 10, 27, 23, 59, 30)
            .earliest()
            .unwrap()
            .timestamp();
        let mut records = (0..4).map(photo).collect::<Vec<_>>();
        for (i, row) in records.iter_mut().enumerate() {
            row.timestamp = start + i as i64 * 20;
            if i == 0 {
                row.sender = LID.into();
            }
        }
        db.ingest(Batch {
            messages: records,
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(db.photo_gallery(1, 1).await.unwrap().unwrap().members, [1]);
        db.ingest(Batch {
            aliases: vec![(LID.into(), PN.into())],
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            db.photo_gallery(1, 1).await.unwrap().unwrap().members,
            [1, 2]
        );
        assert_eq!(
            db.photo_gallery(1, 3).await.unwrap().unwrap().members,
            [3, 4]
        );
        db.call(|c| {
            c.execute(
                "INSERT INTO chat_settings(jid,kind,value,stamp) VALUES(?1,2,1,1)",
                [PN],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(db.photo_gallery(1, 1).await.unwrap().is_none());
    }
}
