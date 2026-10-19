//! All search, identity resolution and history-window queries run on the archive worker.
use super::*;
use crate::message_navigation::{Hit, Search};

impl Archive {
    pub async fn search_messages(&self, chat: i32, query: String, request: i32) -> Result<Search> {
        self.call(move |db| {
            let chat = local_chat(db, chat)?;
            let needle = query.trim().to_lowercase();
            let mut result = Search {
                chat,
                request,
                query,
                ..Default::default()
            };
            if needle.is_empty() {
                return Ok(result);
            }
            // Rust's Unicode lowercase is shared across platforms. Iterate only
            // bodies, never raw envelopes/media, and retain at most 101 matches.
            let mut statement = db.prepare(
                "SELECT id,body,stamp FROM messages
                WHERE chat_id=?1 AND visible=1 AND deleted=0
                AND (expires IS NULL OR expires>unixepoch())
                ORDER BY stamp DESC,id DESC",
            )?;
            let mut rows = statement.query([chat])?;
            while let Some(row) = rows.next()? {
                let text: String = row.get(1)?;
                if !text.to_lowercase().contains(&needle) {
                    continue;
                }
                if result.hits.len() == 100 {
                    result.more = true;
                    break;
                }
                result.hits.push(Hit {
                    id: row.get(0)?,
                    text: domain::single_line_summary(&text),
                    time: time(row.get(2)?),
                });
            }
            Ok(result)
        })
        .await
    }

    pub async fn navigation_target(
        &self,
        chat: i32,
        message: i32,
        quote: bool,
    ) -> Result<(i32, (i64, i32))> {
        self.call(move |db| {
            let chat = local_chat(db, chat)?;
            let target = if quote {
                let raw: Vec<u8> = db.query_row("SELECT raw FROM messages WHERE id=?1 AND chat_id=?2 AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch())", params![message,chat], |r|r.get(0)).optional()?.ok_or(Error::Invalid)?;
                let (wire, sender, remote) = crate::protocol_actions::quoted_key(&raw).ok_or(Error::Invalid)?;
                let jid: String = db.query_row("SELECT jid FROM chats WHERE id=?1",[chat],|r|r.get(0))?;
                if !remote.is_empty() && canonical(db,&remote)? != jid {return Err(Error::Invalid);}
                let sender = canonical(db,&sender)?;
                let owner: Option<String> = db.query_row("SELECT value FROM metadata WHERE key='identity'",[],|r|r.get(0)).optional()?;
                let owner = owner.map(|s|canonical(db,&s)).transpose()?;
                let mut statement = db.prepare("SELECT id,sender,outgoing FROM messages WHERE chat_id=?1 AND wire_id=?2 AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch())")?;
                let mut matches = Vec::new();
                for row in statement.query_map(params![chat,wire],|r|Ok((r.get::<_,i32>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?)))? {
                    let (id,author,outgoing)=row?;
                    if sender.is_empty() || canonical(db,&author)? == sender || (outgoing && owner.as_deref()==Some(sender.as_str())) { matches.push(id); }
                }
                if matches.len()!=1 {return Err(Error::Invalid);}
                matches[0]
            } else {message};
            let stamp: i64 = db.query_row("SELECT stamp FROM messages WHERE id=?1 AND chat_id=?2 AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch())",params![target,chat],|r|r.get(0)).optional()?.ok_or(Error::Invalid)?;
            // Fifty following rows give context without loading the whole gap
            // between an old match and today's messages. The upper identity is
            // stable if new messages arrive while browsing this window.
            let end = db.query_row("SELECT stamp,id FROM messages WHERE chat_id=?1 AND visible=1 AND (stamp,id)>=(?2,?3) ORDER BY stamp,id LIMIT 1 OFFSET 50",params![chat,stamp,target],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let end = match end {Some(end)=>end,None=>db.query_row("SELECT stamp,id FROM messages WHERE chat_id=?1 AND visible=1 ORDER BY stamp DESC,id DESC LIMIT 1",[chat],|r|Ok((r.get(0)?,r.get(1)?)))?};
            Ok((target,end))
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{LID, PN, fixture, record};
    async fn ingest(db: &Archive, messages: Vec<Record>) {
        db.ingest(Batch {
            messages,
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn search_is_unicode_literal_bounded_and_excludes_unavailable_content() {
        let (_temp, db) = fixture();
        let records = (0..180)
            .map(|i| {
                let mut r = record(&format!("search-{i}"));
                r.text = "SĂNĂTOȘI 日本語 👩🏽‍💻 %_\nSecond line".into();
                r.timestamp += i;
                if i == 0 {
                    r.text.push_str(
                        &"Long paragraph 日本語
"
                        .repeat(10_000),
                    );
                    r.text.push_str("tail-marker");
                }
                r
            })
            .collect();
        ingest(&db, records).await;
        let hits = db.search_messages(1, "sănătoși".into(), 9).await.unwrap();
        assert_eq!(hits.request, 9);
        assert_eq!(hits.hits.len(), 100);
        assert!(hits.more);
        assert_eq!(hits.hits[0].id, 180);
        let tail = db
            .search_messages(1, "tail-marker".into(), 10)
            .await
            .unwrap();
        assert_eq!(tail.hits.len(), 1);
        assert_eq!(tail.hits[0].id, 1);
        assert!(tail.hits[0].text.chars().count() <= domain::SUMMARY_CHARS + 1);
        assert_eq!(
            db.search_messages(1, "%_".into(), 1)
                .await
                .unwrap()
                .hits
                .len(),
            100
        );
        assert!(
            db.search_messages(1, "  ".into(), 1)
                .await
                .unwrap()
                .hits
                .is_empty()
        );
        db.blocking(|c| {
            c.execute("UPDATE messages SET deleted=1 WHERE id<=60", [])?;
            c.execute("UPDATE messages SET visible=0 WHERE id>60 AND id<=120", [])?;
            c.execute("UPDATE messages SET expires=1 WHERE id>120 AND id<=170", [])?;
            c.execute(
                "UPDATE messages SET body='Edited replacement' WHERE id=171",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let hits = db.search_messages(1, "日本語 👩🏽‍💻".into(), 10).await.unwrap();
        assert_eq!(hits.hits.len(), 9);
        assert!(!hits.more);
        assert_eq!(
            db.search_messages(1, "replacement".into(), 11)
                .await
                .unwrap()
                .hits[0]
                .id,
            171
        );
        assert!(
            db.search_messages(999, "sănătoși".into(), 12)
                .await
                .unwrap()
                .hits
                .is_empty()
        );
    }
    #[tokio::test]
    async fn old_targets_load_small_stable_windows_and_quoted_authors_are_resolved() {
        let (_temp, db) = fixture();
        ingest(
            &db,
            (0..1000)
                .map(|i| {
                    let mut r = record(&format!("wire-{i}"));
                    r.timestamp += i;
                    r
                })
                .collect(),
        )
        .await;
        let (target, end) = db.navigation_target(1, 120, false).await.unwrap();
        let view = db
            .view_window(1, 100, (String::new(), false, false), Some(end))
            .await
            .unwrap();
        assert!(!view.phone_more);
        let c = view.conversation.unwrap();
        assert_eq!(c.messages.len(), 100);
        assert_eq!(c.older_count, 70);
        assert!(c.messages.iter().any(|m| m.id == target));
        ingest(
            &db,
            vec![{
                let mut r = record("new-arrival");
                r.timestamp += 2000;
                r
            }],
        )
        .await;
        let next = db
            .view_window(1, 100, (String::new(), false, false), Some(end))
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert_eq!(c.messages, next.messages);
        // Public adapter creates a real quoted envelope; lookup works for old
        // archives too and does not depend on display names or a migration.
        let source = db.call(|c| actions::source(c, 1, 120)).await.unwrap();
        let mut source = source;
        source.raw = Vec::new();
        let prepared = crate::protocol_actions::prepare(
            &source,
            crate::message_actions::Operation::Reply("reply".into()),
            PN,
            1_700_000_000,
        )
        .unwrap();
        let mut reply = record("reply");
        reply.raw = prepared.raw;
        reply.content = prepared.content;
        reply.timestamp += 2100;
        ingest(&db, vec![reply]).await;
        assert_eq!(db.navigation_target(1, 1002, true).await.unwrap().0, 120);
        // A wire ID alone is insufficient: another author can reuse it.
        let mut duplicate = record("wire-119");
        duplicate.sender = "15550000002@s.whatsapp.net".into();
        duplicate.timestamp += 2200;
        ingest(&db, vec![duplicate]).await;
        assert_eq!(db.navigation_target(1, 1002, true).await.unwrap().0, 120);
        source.sender.clear();
        let ambiguous = crate::protocol_actions::prepare(
            &source,
            crate::message_actions::Operation::Reply("ambiguous".into()),
            PN,
            1_700_000_000,
        )
        .unwrap();
        let mut reply = record("ambiguous-reply");
        reply.raw = ambiguous.raw;
        reply.timestamp += 2300;
        ingest(&db, vec![reply]).await;
        assert!(db.navigation_target(1, 1004, true).await.is_err());
        db.ingest(Batch {
            aliases: vec![(PN.into(), LID.into())],
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(db.navigation_target(1, 1002, true).await.unwrap().0, 120);
        db.blocking(|c| {
            c.execute("UPDATE messages SET deleted=1 WHERE id=120", [])?;
            Ok(())
        })
        .unwrap();
        assert!(db.navigation_target(1, 1002, true).await.is_err());
        assert!(db.navigation_target(1, 120, false).await.is_err());
    }
}
