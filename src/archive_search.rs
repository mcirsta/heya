//! Indexed word search. All work runs through the existing archive worker.
use super::*;
use crate::message_navigation::{Hit, Search};

const QUERY: &str = "SELECT m.id,m.chat_id,substr(m.body,1,?2),m.stamp,
    coalesce(CASE WHEN n.saved_name IS NOT NULL THEN coalesce(nullif(n.saved_name,''),n.profile_name)
        ELSE coalesce(nullif(c.name,''),n.profile_name) END,'') AS name,c.jid
    FROM message_words JOIN messages m ON m.id=message_words.rowid
    JOIN chats c ON c.id=m.chat_id LEFT JOIN contacts n ON n.jid=c.jid
    WHERE message_words MATCH ?1 AND m.visible=1 AND m.deleted=0
    AND (m.expires IS NULL OR m.expires>unixepoch()) AND c.listed=1
    AND NOT EXISTS(SELECT 1 FROM chat_settings s WHERE s.jid=c.jid AND s.kind=2 AND s.value=1)
    ORDER BY message_words.rowid DESC LIMIT 100";

impl Archive {
    pub async fn chat_archived(&self, chat: i32) -> Result<bool> {
        self.call(move |db| {
            let chat = local_chat(db,chat)?;
            Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM chats c JOIN chat_settings s ON s.jid=c.jid AND s.kind=1 WHERE c.id=?1 AND s.value=1)",[chat],|r|r.get(0))?)
        }).await
    }
    /// One bounded, resumable batch; never rebuild history while opening storage.
    pub async fn index_search_batch(&self) -> Result<bool> {
        self.call(|db| {
            let tx = db.transaction()?;
            let (cursor, end): (i64, i64) = tx.query_row(
                "SELECT cursor,end_id FROM search_progress", [], |r| Ok((r.get(0)?,r.get(1)?)),
            )?;
            if cursor >= end { return Ok(true); }
            let next: Option<i64> = tx.query_row(
                "SELECT max(id) FROM (SELECT id FROM messages WHERE id>?1 AND id<=?2 ORDER BY id LIMIT 128)",
                params![cursor,end], |r|r.get(0),
            )?;
            if let Some(next) = next {
                tx.execute("INSERT INTO message_words(rowid,body) SELECT id,body FROM messages WHERE id>?1 AND id<=?2",params![cursor,next])?;
            }
            let next = next.unwrap_or(end);
            tx.execute("UPDATE search_progress SET cursor=?1",[next])?;
            tx.commit()?;
            Ok(next == end)
        }).await
    }

    pub async fn search_all_messages(&self, query: String, request: i32) -> Result<Search> {
        self.call(move |db| {
            let mut result = Search {
                query,
                request,
                ..Default::default()
            };
            result.indexing =
                db.query_row("SELECT cursor<end_id FROM search_progress", [], |r| {
                    r.get(0)
                })?;
            // Do not present an incomplete index as a complete search.
            if result.indexing {
                return Ok(result);
            }
            let words = crate::message_navigation::word_query(db, &result.query)?;
            if words.is_empty() {
                return Ok(result);
            }
            result.hits = db
                .prepare_cached(QUERY)?
                .query_map(params![words, (domain::SUMMARY_CHARS + 1) as i64], |r| {
                    let name: String = r.get(4)?;
                    let jid: String = r.get(5)?;
                    Ok(Hit {
                        id: r.get(0)?,
                        chat: r.get(1)?,
                        title: if name.is_empty() {
                            jid.split('@').next().unwrap_or(&jid).into()
                        } else {
                            name
                        },
                        text: domain::single_line_summary(&r.get::<_, String>(2)?),
                        time: time(r.get(3)?),
                    })
                })?
                .collect::<std::result::Result<_, _>>()?;
            // No extra scan to count matches: 100 means the user may refine.
            result.more = result.hits.len() == 100;
            Ok(result)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{KEY, PN, fixture, record};

    #[tokio::test]
    #[ignore = "Synthetic search timings; run in an optimized build without other builds running"]
    async fn indexed_search_benchmark() {
        let (_temp, db) = fixture();
        db.ingest(Batch {
            messages: vec![record("seed")],
            ..Default::default()
        })
        .await
        .unwrap();
        db.call(|c| {
            let tx=c.transaction()?;
            tx.execute("WITH RECURSIVE rows(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM rows WHERE n<100000)
                INSERT INTO messages(chat_id,sender,wire_id,source_chat,source_sender,stamp,outgoing,sender_name,body,raw,visible)
                SELECT 1,'synthetic',CAST(n AS TEXT),'synthetic','synthetic',n,0,'Synthetic',
                CASE WHEN n=1 THEN 'meeting rareword' WHEN n%2=0 THEN 'meeting project with the whole team' ELSE 'another note about a project' END,X'01',1 FROM rows",[])?;
            tx.commit()?;Ok(())
        }).await.unwrap();
        for (query, expected) in [
            ("meeting", 100),
            ("meeting project", 100),
            ("rareword", 1),
            ("missingword", 0),
            ("meeting missingword", 0),
        ] {
            let mut times = Vec::new();
            for _ in 0..9 {
                let start = std::time::Instant::now();
                assert_eq!(
                    db.search_all_messages(query.into(), 1)
                        .await
                        .unwrap()
                        .hits
                        .len(),
                    expected
                );
                times.push(start.elapsed().as_secs_f64() * 1000.);
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "search {query}: median={:.3} ms max={:.3} ms (100,000 synthetic messages)",
                times[4], times[8]
            );
        }
    }

    #[tokio::test]
    async fn whole_words_are_anded_and_ids_descend_across_all_history() {
        let (_temp, db) = fixture();
        let messages = (0..240)
            .map(|i| {
                let mut r = record(&format!("word-{i}"));
                r.chat = if i % 3 == 0 {
                    PN.into()
                } else {
                    "15550000002@s.whatsapp.net".into()
                };
                r.text = if i % 2 == 0 {
                    "Meeting at the școală for our CAFÉ project"
                } else {
                    "meetings about other projects"
                }
                .into();
                r.timestamp -= i; // ID order deliberately disagrees with date order.
                if i == 0 {
                    r.text
                        .push_str(&format!("{} rareword", " long paragraph".repeat(10_000)));
                }
                r
            })
            .collect();
        db.ingest(Batch {
            messages,
            ..Default::default()
        })
        .await
        .unwrap();
        let search = db
            .search_all_messages("project, MEETING scoala cafe".into(), 7)
            .await
            .unwrap();
        assert!(!search.indexing);
        assert_eq!(search.request, 7);
        assert_eq!(search.hits.len(), 100);
        assert!(search.more);
        assert_eq!(search.hits[0].id, 239);
        assert!(search.hits.windows(2).all(|w| w[0].id > w[1].id));
        assert!(search.hits.iter().any(|h| h.chat == 1));
        assert!(search.hits.iter().any(|h| h.chat == 2));
        assert!(search.hits.iter().all(|h| !h.title.is_empty()));
        assert_eq!(
            db.search_all_messages("meeting rareword".into(), 8)
                .await
                .unwrap()
                .hits[0]
                .id,
            1
        );
        for query in [
            "meet",
            "missingword",
            "meeting missingword",
            "meeting OR project",
            "😀 * \"",
            " ",
        ] {
            assert!(
                db.search_all_messages(query.into(), 9)
                    .await
                    .unwrap()
                    .hits
                    .is_empty(),
                "{query}"
            );
        }
        // SQL operators are literal words, and punctuation does not create a phrase.
        db.blocking(|c| {
            c.execute(
                "UPDATE messages SET body='OR before meeting; project after' WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            db.search_all_messages("\"meeting project\" OR".into(), 10)
                .await
                .unwrap()
                .hits[0]
                .id,
            1
        );
        let plan = db
            .blocking(|c| {
                let sql = format!("EXPLAIN QUERY PLAN {QUERY}");
                Ok(c.prepare(&sql)?
                    .query_map(params!["meeting", 100], |r| r.get::<_, String>(3))?
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            })
            .unwrap();
        assert!(
            plan.iter().any(|p| p.contains("VIRTUAL TABLE INDEX 192:")),
            "{plan:?}"
        );
        assert!(!plan.iter().any(|p| p.contains("TEMP B-TREE")), "{plan:?}");
    }

    #[tokio::test]
    async fn edits_removals_and_locked_chats_are_filtered_before_the_limit() {
        let (_temp, db) = fixture();
        db.ingest(Batch {
            messages: (0..240)
                .map(|i| {
                    let mut r = record(&format!("eligible-{i}"));
                    r.text = "meeting project".into();
                    r
                })
                .collect(),
            ..Default::default()
        })
        .await
        .unwrap();
        db.blocking(|c| {
            c.execute("UPDATE messages SET deleted=1 WHERE id>220", [])?;
            c.execute("UPDATE messages SET visible=0 WHERE id>200 AND id<=220", [])?;
            c.execute("UPDATE messages SET expires=1 WHERE id>180 AND id<=200", [])?;
            c.execute(
                "UPDATE messages SET body='changed replacement' WHERE id=180",
                [],
            )?;
            c.execute("DELETE FROM messages WHERE id=179", [])?;
            Ok(())
        })
        .unwrap();
        let s = db
            .search_all_messages("meeting project".into(), 1)
            .await
            .unwrap();
        assert_eq!(s.hits.len(), 100);
        assert_eq!(s.hits[0].id, 178);
        assert_eq!(
            db.search_all_messages("replacement".into(), 2)
                .await
                .unwrap()
                .hits[0]
                .id,
            180
        );
        db.save_chat_settings(vec![crate::chat_management::Update {
            jid: PN.into(),
            setting: crate::chat_management::Setting::Archive,
            value: 1,
            stamp: Some(1),
        }])
        .await
        .unwrap();
        assert_eq!(
            db.search_all_messages("meeting".into(), 3)
                .await
                .unwrap()
                .hits
                .len(),
            100
        );
        db.save_chat_settings(vec![crate::chat_management::Update {
            jid: PN.into(),
            setting: crate::chat_management::Setting::Lock,
            value: 1,
            stamp: Some(2),
        }])
        .await
        .unwrap();
        assert!(
            db.search_all_messages("meeting".into(), 4)
                .await
                .unwrap()
                .hits
                .is_empty()
        );
    }

    #[tokio::test]
    async fn migration_backfills_in_batches_and_resumes_with_concurrent_changes() {
        let (_temp, db) = fixture();
        db.ingest(Batch {
            messages: (0..300).map(|i| record(&format!("history-{i}"))).collect(),
            ..Default::default()
        })
        .await
        .unwrap();
        let files = db.0._files.clone();
        db.blocking(|c| {
            c.execute_batch("DROP TRIGGER message_words_insert; DROP TRIGGER message_words_update; DROP TRIGGER message_words_delete;
                DROP TABLE message_words; DROP TABLE search_progress; PRAGMA user_version=22;")?;
            Ok(())
        }).unwrap();
        drop(db);
        let db = Archive::open(files.clone(), KEY).unwrap();
        assert!(
            db.search_all_messages("history".into(), 1)
                .await
                .unwrap()
                .indexing
        );
        assert!(!db.index_search_batch().await.unwrap());
        db.blocking(|c| {
            for id in [1, 299] {
                c.execute("UPDATE messages SET body='replacement' WHERE id=?1", [id])?;
            }
            for id in [2, 298] {
                c.execute("DELETE FROM messages WHERE id=?1", [id])?;
            }
            Ok(())
        })
        .unwrap();
        db.ingest(Batch {
            messages: vec![record("newarrival")],
            ..Default::default()
        })
        .await
        .unwrap();
        drop(db);
        let db = Archive::open(files, KEY).unwrap();
        while !db.index_search_batch().await.unwrap() {}
        let hits = db
            .search_all_messages("replacement".into(), 2)
            .await
            .unwrap()
            .hits;
        assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), [299, 1]);
        assert_eq!(
            db.search_all_messages("newarrival".into(), 3)
                .await
                .unwrap()
                .hits[0]
                .id,
            301
        );
        db.blocking(|c| {
            c.execute(
                "INSERT INTO message_words(message_words,rank) VALUES('integrity-check',1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    }
}
