use super::*;
use crate::{
    archive::tests::{KEY, PN, fixture, record},
    message_content::{Quote, ReactionEvent},
};

async fn view(db: &Archive, limit: usize) -> domain::Conversation {
    db.view(1, limit, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
}
async fn ingest(db: &Archive, messages: Vec<Record>) {
    db.ingest(Batch {
        messages,
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
}
fn reused(a: &domain::Conversation, b: &domain::Conversation) {
    assert!(Arc::ptr_eq(&a.messages, &b.messages));
    assert!(Arc::ptr_eq(&a.transcript, &b.transcript));
}

#[tokio::test]
async fn unrelated_updates_reuse_full_text_but_refresh_sidebar_draft_and_reply() {
    let (_temp, db) = fixture();
    let mut first = record("full-text");
    first.text = "Unicode 👩🏽‍💻 日本語 مرحبا\nLong paragraph\n".repeat(100);
    ingest(&db, vec![first.clone()]).await;
    let before = view(&db, 100).await;
    assert!(before.transcript.contains(&first.text));
    let mut other = record("other-chat");
    other.chat = "15550000002@s.whatsapp.net".into();
    other.sender = other.chat.clone();
    let other_jid = other.chat.clone();
    ingest(&db, vec![other]).await;
    reused(&before, &view(&db, 100).await);
    let profiles = vec![(other_jid, "Other profile".into())];
    db.ingest(Batch {
        profiles: profiles.clone(),
        ..Default::default()
    })
    .await
    .unwrap();
    // A conservative shared-name invalidation with unchanged message output
    // must retain the same payloads in both the cache and subsequent snapshots.
    reused(&before, &view(&db, 100).await);
    // Live envelopes repeat push names. Once stored, an unchanged name must not
    // invalidate the selected timeline on every arrival in another chat.
    let before = view(&db, 100).await;
    db.ingest(Batch {
        profiles,
        ..Default::default()
    })
    .await
    .unwrap();
    db.draft(1, "Latest draft".into()).await.unwrap();
    db.set_reply(1, before.messages[0].id).await.unwrap();
    db.blocking(|c| {
        c.execute(
            "UPDATE chats SET name='Renamed chat',read_only=1,unread=3 WHERE id=1",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let update = db.view(1, 100, String::new(), false, false).await.unwrap();
    assert_eq!(update.chats.len(), 2);
    assert!(!update.can_send);
    let after = update.conversation.unwrap();
    reused(&before, &after);
    assert_eq!(after.chat.title, "Renamed chat");
    assert_eq!(after.chat.unread, 3);
    assert_eq!(after.draft, "Latest draft");
    assert_eq!(after.reply.unwrap().id, before.messages[0].id);
    assert_eq!(after.messages[0].text, first.text);
    // Filtering the sidebar must not hide or rebuild the selected chat.
    let filtered = db
        .view(1, 100, "No match".into(), true, false)
        .await
        .unwrap();
    assert!(filtered.chats.is_empty());
    reused(&before, &filtered.conversation.unwrap());
    let read = db
        .view(1, 100, String::new(), false, true)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(read.chat.unread, 0);
    reused(&before, &read);
}

#[tokio::test]
async fn message_writes_refresh_text_delivery_attachments_and_visibility() {
    let (_temp, db) = fixture();
    let mut photo = record("photo");
    photo.content.photo = Some(crate::message_content::Photo {
        width: 640,
        height: 480,
        size: Some(1234),
    });
    ingest(&db, vec![photo, record("second")]).await;
    let before = view(&db, 100).await;
    db.blocking(|c| {
        c.execute(
            "UPDATE messages SET body='New caption' WHERE wire_id='photo'",
            [],
        )?;
        c.execute(
            "UPDATE messages SET outgoing=1,delivery='read' WHERE wire_id='second'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let after = view(&db, 100).await;
    assert!(!Arc::ptr_eq(&before.messages, &after.messages));
    assert_eq!(after.messages[0].text, "New caption");
    assert!(after.messages[0].content.photo.is_some());
    assert!(after.messages[1].time.contains("Read"));
    assert!(after.transcript.contains("New caption"));
    assert!(!after.transcript.contains("Synthetic photo"));
    db.blocking(|c| {
        c.execute("UPDATE chats SET is_group=1 WHERE id=1", [])?;
        Ok(())
    })
    .unwrap();
    assert!(
        view(&db, 100).await.messages[1]
            .time
            .contains("Read (some)")
    );
    db.blocking(|c| {
        c.execute("UPDATE messages SET visible=0 WHERE wire_id='photo'", [])?;
        Ok(())
    })
    .unwrap();
    let hidden = view(&db, 100).await;
    assert_eq!(hidden.messages.len(), 1);
    assert!(!hidden.transcript.contains("New caption"));
    db.blocking(|c| {
        c.execute("DELETE FROM messages WHERE wire_id='second'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(view(&db, 100).await.messages.is_empty());
}

#[tokio::test]
async fn contact_names_quotes_and_reaction_echoes_invalidate_the_projection() {
    let (_temp, db) = fixture();
    let mut original = record("quoted");
    original.content.quote = Some(Quote {
        sender: PN.into(),
        text: "Quoted text".into(),
    });
    ingest(&db, vec![original]).await;
    let before = view(&db, 100).await;
    db.save_contacts(
        vec![Contact {
            jid: PN.into(),
            name: "Saved sender".into(),
            timestamp: Some(1),
        }],
        vec![],
        false,
    )
    .await
    .unwrap();
    let named = view(&db, 100).await;
    assert!(!Arc::ptr_eq(&before.messages, &named.messages));
    assert_eq!(named.messages[0].sender, "Saved sender");
    assert_eq!(
        named.messages[0].content.quote.as_ref().unwrap().sender,
        "Saved sender"
    );
    let mut reaction = record("reaction");
    reaction.visible = false;
    reaction.content.reaction = Some(ReactionEvent {
        target: "quoted".into(),
        emoji: "👍".into(),
        at: 1,
    });
    ingest(&db, vec![reaction]).await;
    let reacted = view(&db, 100).await;
    assert_eq!(reacted.messages.len(), 1);
    assert_eq!(reacted.messages[0].content.reaction_summary, "👍");
    db.blocking(|c| {
        c.execute("UPDATE reactions SET emoji='❤️' WHERE target='quoted'", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        view(&db, 100).await.messages[0].content.reaction_summary,
        "❤️"
    );
    db.blocking(|c| {
        c.execute("DELETE FROM reactions", [])?;
        Ok(())
    })
    .unwrap();
    assert!(
        view(&db, 100).await.messages[0]
            .content
            .reaction_summary
            .is_empty()
    );
}

#[tokio::test]
async fn paging_preserves_order_for_equal_timestamps_and_expiry_cannot_return_cached_text() {
    let (_temp, db) = fixture();
    ingest(
        &db,
        vec![record("first"), record("second"), record("third")],
    )
    .await;
    let page = view(&db, 2).await;
    assert_eq!(page.older_count, 1);
    assert_eq!(
        page.messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        ["Synthetic second", "Synthetic third"]
    );
    reused(&page, &view(&db, 2).await);
    let all = view(&db, 100).await;
    assert_eq!(all.older_count, 0);
    assert_eq!(all.messages[0].text, "Synthetic first");
    db.blocking(|c| {
        c.execute("UPDATE messages SET expires=1 WHERE wire_id='third'", [])?;
        Ok(())
    })
    .unwrap();
    let expired = view(&db, 100).await;
    assert_eq!(expired.messages[2].text, "[Disappearing message expired]");
    assert!(!expired.transcript.contains("Synthetic third"));
    assert!(expired.messages[2].content.unavailable);
}

#[tokio::test]
async fn rollback_keeps_cache_valid_but_external_commits_refresh_it() {
    let (_temp, db) = fixture();
    ingest(&db, vec![record("original")]).await;
    let before = view(&db, 100).await;
    db.blocking(|c| {
        let tx = c.transaction()?;
        tx.execute("UPDATE messages SET body='Rolled back'", [])?;
        // Deliberately drop without committing.
        Ok(())
    })
    .unwrap();
    reused(&before, &view(&db, 100).await);
    let external = Connection::open(db.0._files.path.with_file_name("archive.db")).unwrap();
    external.execute_batch(KEY).unwrap();
    external
        .execute("UPDATE messages SET body='External commit'", [])
        .unwrap();
    let after = view(&db, 100).await;
    assert_eq!(after.messages[0].text, "External commit");
    assert!(!Arc::ptr_eq(&before.messages, &after.messages));
    assert!(after.transcript.contains("External commit"));
    reused(&after, &view(&db, 100).await);
}

#[tokio::test]
async fn deselecting_releases_the_cached_timeline_and_query_uses_the_ordered_index() {
    let (_temp, db) = fixture();
    ingest(&db, vec![record("original")]).await;
    let before = view(&db, 100).await;
    let messages = Arc::downgrade(&before.messages);
    let transcript = Arc::downgrade(&before.transcript);
    drop(before);
    assert!(
        db.view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .is_none()
    );
    assert!(messages.upgrade().is_none());
    assert!(transcript.upgrade().is_none());
    db.blocking(|c| {
        for append in [false, true] {
            let mut query = c.prepare(&format!("EXPLAIN QUERY PLAN {}", query(append)))?;
            let params = [1, 100, 0, 0];
            let plan = query
                .query_map(
                    rusqlite::params_from_iter(params[..if append { 4 } else { 2 }].iter()),
                    |r| r.get::<_, String>(3),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
                .join("\n");
            assert!(plan.contains("messages_visible_timeline"), "{plan}");
            assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        }
        Ok(())
    })
    .unwrap();
}

async fn assert_fresh_equivalent(db: &Archive, cached: &domain::Conversation, limit: usize) {
    db.blocking(|c| db.0.timeline.lock().unwrap().clear(c))
        .unwrap();
    let fresh = view(db, limit).await;
    assert_eq!(cached.messages, fresh.messages);
    assert_eq!(cached.transcript, fresh.transcript);
    assert_eq!(cached.older_count, fresh.older_count);
}

#[tokio::test]
async fn append_and_rolling_windows_match_fresh_projections_without_redecoding_retained_messages() {
    for limit in [0, 1, 3, 100] {
        let (_temp, db) = fixture();
        let mut seed = (0..4)
            .map(|i| record(&format!("seed-{i}")))
            .collect::<Vec<_>>();
        for m in &mut seed {
            m.text = "👩🏽‍💻 日本語 مرحبا\n\nA full paragraph\nAnother line".into();
        }
        ingest(&db, seed).await;
        let mut before = view(&db, limit).await;
        for (batch, size) in [1, 2, 5].into_iter().enumerate() {
            let messages = (0..size)
                .rev()
                .map(|i| {
                    let mut m = record(&format!("append-{batch}-{i}"));
                    m.timestamp += batch as i64; // Includes equal timestamps and ties by row ID.
                    m.text = "More Unicode 🙂\n\nPreserve blank lines".into();
                    m
                })
                .collect();
            ingest(&db, messages).await;
            let after = view(&db, limit).await;
            for m in after.messages.iter() {
                if let Some(old) = before.messages.iter().find(|old| old.id == m.id) {
                    assert!(Arc::ptr_eq(&old.content, &m.content));
                }
            }
            assert_fresh_equivalent(&db, &after, limit).await;
            before = view(&db, limit).await;
        }
        let mut late = record("late-history");
        late.timestamp -= 100;
        ingest(&db, vec![late]).await;
        assert_fresh_equivalent(&db, &view(&db, limit).await, limit).await;
    }
}

#[tokio::test]
async fn appending_an_ambiguous_wire_key_refreshes_existing_reaction_projections() {
    let (_temp, db) = fixture();
    let original = record("ambiguous");
    let mut reaction = record("reaction");
    reaction.visible = false;
    reaction.content.reaction = Some(ReactionEvent {
        target: original.id.clone(),
        emoji: "👍".into(),
        at: 1,
    });
    ingest(&db, vec![original.clone(), reaction]).await;
    assert_eq!(
        view(&db, 100).await.messages[0].content.reaction_summary,
        "👍"
    );
    let mut duplicate = original;
    duplicate.sender = "15550000002@s.whatsapp.net".into();
    duplicate.timestamp += 1;
    ingest(&db, vec![duplicate]).await;
    let after = view(&db, 100).await;
    assert!(
        after
            .messages
            .iter()
            .all(|m| m.content.reaction_summary.is_empty())
    );
    assert_fresh_equivalent(&db, &after, 100).await;
}

#[tokio::test]
async fn an_appended_outbox_event_refreshes_pending_controls_on_earlier_messages() {
    let (_temp, db) = fixture();
    ingest(&db, vec![record("target")]).await;
    db.blocking(|c| {
        c.execute("INSERT INTO message_controls(event_wire,kind,target,sender,outgoing,stamp,text,version,applied) VALUES('CONTROL','edit','target','#self',1,1,'Synthetic edit',1,1)",[])?;
        Ok(())
    }).unwrap();
    assert!(
        view(&db, 100).await.messages[0]
            .content
            .metadata
            .pending
            .is_empty()
    );
    db.queue(1, "CONTROL".into(), "Synthetic outgoing event".into())
        .await
        .unwrap();
    let after = view(&db, 100).await;
    assert!(
        after.messages[0]
            .content
            .metadata
            .pending
            .contains("pending")
    );
    assert_fresh_equivalent(&db, &after, 100).await;
}
