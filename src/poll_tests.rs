//! Synthetic envelopes exercise real SDK crypto and the durable archive path.
use crate::{
    archive::{
        self,
        tests::{LID, PN, fixture, record},
    },
    message_actions::{Operation, Source},
    polls::{Draft, Poll},
    protocol_polls as wire,
};
use whatsapp_rust::{
    prelude::wa,
    waproto::{buffa::MessageField, codec},
};
const OWN: &str = "15550000009@s.whatsapp.net";
const GROUP: &str = "15550000000-123@g.us";
fn draft(multiple: bool) -> Draft {
    Draft {
        question: "Lunch? 🥪".into(),
        options: vec!["Mîra’s café".into(), "Picnic 👩🏽‍💻".into(), "At home".into()],
        multiple,
    }
}
fn parent(id: &str, group: bool) -> archive::Record {
    let p = wire::prepare(&draft(true), None, OWN).unwrap();
    let mut r = record(id);
    r.raw = p.raw;
    r.text = p.text;
    r.content = p.content;
    if group {
        r.chat = GROUP.into();
    }
    r
}
fn source(r: &archive::Record) -> Source {
    Source {
        id: 1,
        chat: 1,
        jid: r.chat.clone(),
        sender: r.sender.clone(),
        wire: r.id.clone(),
        outgoing: r.outgoing,
        body: r.text.clone(),
        raw: r.raw.clone(),
    }
}
fn vote(
    p: &archive::Record,
    id: &str,
    voter: &str,
    selected: &[usize],
    stamp: i64,
) -> archive::Record {
    let prepared = wire::vote(&source(p), selected, &p.sender, voter, stamp).unwrap();
    let mut r = record(id);
    r.chat = p.chat.clone();
    r.sender = voter.into();
    r.sender_name = "Sample voter".into();
    r.raw = prepared.raw;
    r.content = prepared.content;
    r.text = String::new();
    r.timestamp = stamp / 1000;
    r.visible = false;
    r
}
async fn ingest(db: &archive::Archive, rows: Vec<archive::Record>) {
    db.ingest(archive::Batch {
        messages: rows,
        ..Default::default()
    })
    .await
    .unwrap();
}
async fn view(db: &archive::Archive, chat: i32) -> crate::domain::Conversation {
    db.view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
}
async fn chat(db: &archive::Archive, jid: &str) -> i32 {
    db.create_chat(jid.into()).await.unwrap()
}
fn poll(v: &crate::domain::Conversation) -> &Poll {
    v.messages
        .iter()
        .find_map(|m| m.content.poll.as_ref())
        .unwrap()
}
#[test]
fn definitions_crypto_wrappers_and_validation() {
    for multiple in [false, true] {
        let p = wire::prepare(&draft(multiple), None, OWN).unwrap();
        let m = codec::message_decode(&p.raw).unwrap();
        assert_eq!(m.poll_creation_message_v3.is_set(), !multiple);
        assert_eq!(wire::secret(&m).unwrap().len(), 32);
        assert_eq!(
            p.content.poll.as_ref().unwrap().selectable,
            if multiple { 3 } else { 1 }
        );
        let base = wire::creation(&m).unwrap().clone();
        for version in [1, 2, 3, 5, 6] {
            let mut m = wa::Message::default();
            match version {
                1 => m.poll_creation_message = MessageField::some(base.clone()),
                2 => m.poll_creation_message_v2 = MessageField::some(base.clone()),
                3 => m.poll_creation_message_v3 = MessageField::some(base.clone()),
                5 => m.poll_creation_message_v5 = MessageField::some(base.clone()),
                _ => m.poll_creation_message_v6 = MessageField::some(base.clone()),
            };
            m.message_context_info
                .get_or_insert_default()
                .message_secret = Some(vec![7; 32]);
            let mut wrapped = wa::Message::default();
            wrapped.ephemeral_message.get_or_insert_default().message = MessageField::some(m);
            assert_eq!(
                wire::definition(&wrapped).unwrap().question,
                draft(multiple).question
            );
        }
    }
    let p = parent("P", true);
    let v = vote(&p, "V", OWN, &[0, 2], 5_000);
    assert_eq!(
        wire::decrypt(&p.raw, &v.raw, "P", &[LID.into(), PN.into()], &[OWN.into()]),
        Some(vec![0, 2])
    );
    assert!(wire::decrypt(&p.raw, &v.raw, "WRONG", &[PN.into()], &[OWN.into()]).is_none());
    assert!(wire::decrypt(&p.raw, &v.raw, "P", &[PN.into()], &[PN.into()]).is_none());
    assert!(wire::vote(&source(&p), &[0, 0], PN, OWN, 1).is_err());
    let mut bad = draft(false);
    bad.options[1] = bad.options[0].clone();
    assert!(!bad.valid());
    assert!(crate::protocol_actions::prepare(&source(&p), Operation::Forward, OWN, 1).is_err());
    assert!(crate::protocol_edits::apply(&p.raw, &[], "bad edit").is_none());
    let mut m = codec::message_decode(&p.raw).unwrap();
    m.poll_creation_message
        .get_or_insert_default()
        .hide_participant_name = Some(true);
    assert!(!wire::definition(&m).unwrap().restriction.is_empty());
}
#[tokio::test]
async fn votes_before_creation_update_remove_and_reject_invalid_without_new_bubbles() {
    let (_dir, db) = fixture();
    let p = parent("P", true);
    let a = vote(&p, "V1", OWN, &[0, 2], 10_000);
    ingest(&db, vec![a, p.clone()]).await;
    let chat = chat(&db, GROUP).await;
    let first = view(&db, chat).await;
    assert_eq!(first.messages.len(), 1);
    assert_eq!(poll(&first).view.counts, vec![1, 0, 1]);
    let unchanged = view(&db, chat).await;
    assert!(std::sync::Arc::ptr_eq(&first.messages, &unchanged.messages));
    ingest(
        &db,
        vec![
            vote(&p, "V3", OWN, &[1], 30_000),
            vote(&p, "V2", OWN, &[2], 20_000),
        ],
    )
    .await;
    let v = view(&db, chat).await;
    assert_eq!(poll(&v).view.counts, vec![0, 1, 0]);
    assert_eq!(poll(&v).view.voters, 1);
    let mut bad = vote(&p, "INVALID", OWN, &[0], 40_000);
    let mut m = codec::message_decode(&bad.raw).unwrap();
    m.poll_update_message
        .get_or_insert_default()
        .vote
        .get_or_insert_default()
        .enc_payload
        .as_mut()
        .unwrap()[0] ^= 1;
    bad.raw = codec::message_to_vec(&m);
    ingest(&db, vec![bad]).await;
    assert_eq!(poll(&view(&db, chat).await).view.counts, vec![0, 1, 0]);
    ingest(&db, vec![vote(&p, "CLEAR", OWN, &[], 50_000)]).await;
    let v = view(&db, chat).await;
    assert_eq!(poll(&v).view.counts, vec![0, 0, 0]);
    assert_eq!(poll(&v).view.voters, 0);
    assert_eq!(v.messages.len(), 1);
    assert_eq!(
        db.poll_results(chat, v.messages[0].id, 1, 100)
            .await
            .unwrap()
            .voters
            .len(),
        0
    );
}
#[tokio::test]
async fn aliases_retry_independent_creator_and_voter_namespaces_and_history_secrets() {
    let (_dir, db) = fixture();
    let mut p = parent("ALIASES", true);
    p.sender = LID.into();
    let mut crypto = p.clone();
    crypto.sender = PN.into();
    let mut v = vote(&crypto, "A", OWN, &[0], 1000);
    v.content.ballot.as_mut().unwrap().author = LID.into();
    ingest(&db, vec![p.clone(), v]).await;
    let chat = chat(&db, GROUP).await;
    assert_eq!(poll(&view(&db, chat).await).view.counts, vec![0, 0, 0]);
    db.save_contacts(vec![], vec![(LID.into(), PN.into())], true)
        .await
        .unwrap();
    assert_eq!(poll(&view(&db, chat).await).view.counts, vec![1, 0, 0]);
    let mut missing = parent("HISTORY", true);
    let mut m = codec::message_decode(&missing.raw).unwrap();
    let secret = wire::secret(&m).unwrap().to_vec();
    m.message_context_info = Default::default();
    missing.raw = codec::message_to_vec(&m);
    missing.content = crate::protocol_content::extract(&m).1;
    ingest(&db, vec![missing.clone()]).await;
    let mut info = wa::WebMessageInfo::default();
    info.message_secret = Some(secret);
    let mut u = wa::PollUpdate::default();
    let key = u.poll_update_message_key.get_or_insert_default();
    key.id = Some("HISTORY-VOTE".into());
    key.from_me = Some(false);
    key.participant = Some(OWN.into());
    u.vote.get_or_insert_default().selected_options =
        vec![whatsapp_rust::wacore::poll::compute_option_hash(&draft(true).options[2]).to_vec()];
    u.sender_timestamp_ms = Some(50_000);
    info.poll_updates.push(u);
    let votes = wire::history(&mut missing, &info);
    assert!(missing.content.poll.as_ref().unwrap().ready);
    assert_eq!(votes.len(), 1);
    ingest(&db, std::iter::once(missing).chain(votes).collect()).await;
    let v = view(&db, chat).await;
    let m = v
        .messages
        .iter()
        .find(|m| m.content.metadata.wire == "HISTORY")
        .unwrap();
    assert_eq!(m.content.poll.as_ref().unwrap().view.counts, vec![0, 0, 1]);
    db.repair_polls().unwrap();
    db.repair_polls().unwrap();
    assert_eq!(view(&db, chat).await.messages.len(), 2);
}
#[tokio::test]
async fn create_vote_outbox_confirmation_guards_and_results() {
    let (_dir, db) = fixture();
    db.blocking(|db| {
        db.execute(
            "INSERT OR REPLACE INTO metadata(key,value) VALUES('identity',?1)",
            [OWN],
        )?;
        Ok(())
    })
    .unwrap();
    let chat = chat(&db, PN).await;
    db.draft(chat, "Keep my draft 👩🏽‍💻".into()).await.unwrap();
    db.queue_poll(chat, "OWN-POLL".into(), draft(false), 0)
        .await
        .unwrap();
    let v = view(&db, chat).await;
    let id = v.messages[0].id;
    assert_eq!(v.draft, "Keep my draft 👩🏽‍💻");
    assert!(!poll(&v).view.can_vote);
    db.poll_creator(chat, "OWN-POLL".into(), OWN.into())
        .await
        .unwrap();
    assert!(poll(&view(&db, chat).await).view.can_vote);
    db.queue_vote(chat, id, "OWN-VOTE".into(), 0).await.unwrap();
    let v = view(&db, chat).await;
    assert_eq!(v.messages.len(), 1);
    assert!(poll(&v).view.busy);
    assert_eq!(poll(&v).view.selected, vec![0]);
    assert_eq!(poll(&v).view.counts, vec![0, 0, 0]);
    assert!(db.queue_vote(chat, id, "DOUBLE".into(), 1).await.is_err());
    db.blocking(|db| {
        db.execute(
            "UPDATE messages SET delivery='accepted' WHERE wire_id='OWN-VOTE'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let v = view(&db, chat).await;
    assert_eq!(poll(&v).view.counts, vec![1, 0, 0]);
    assert!(!poll(&v).view.busy);
    let result = db.poll_results(chat, id, 0, 100).await.unwrap();
    assert_eq!(result.voters[0].name, "You");
    db.queue_vote(chat, id, "REVOTE".into(), 1).await.unwrap();
    db.blocking(|db| {
        db.execute(
            "UPDATE messages SET delivery='failed' WHERE wire_id='REVOTE'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let v = view(&db, chat).await;
    assert!(!poll(&v).view.busy);
    assert_eq!(poll(&v).view.counts, vec![1, 0, 0]);
    assert!(poll(&v).view.status.starts_with("Vote failed"));
    db.queue_vote(chat, id, "HIDDEN-QUEUED".into(), 0)
        .await
        .unwrap();
    db.hide_remote(PN.into(), OWN.into(), "OWN-POLL".into(), true)
        .await
        .unwrap();
    assert!(
        db.claim_next().await.unwrap().is_none(),
        "Removing a poll must cancel queued votes, not send an empty envelope"
    );
    assert!(db.queue_vote(chat, id, "HIDDEN".into(), 0).await.is_err());
}

#[tokio::test]
async fn modern_history_addons_expiry_deletion_and_bounded_results() {
    let (_dir, db) = fixture();
    let mut p = parent("MODERN", true);
    let mut info = wa::WebMessageInfo::default();
    // Current add-ons take precedence over stale legacy updates.
    let mut legacy = wa::PollUpdate::default();
    let key = legacy.poll_update_message_key.get_or_insert_default();
    key.id = Some("STALE".into());
    key.from_me = Some(false);
    key.participant = Some(OWN.into());
    legacy.vote.get_or_insert_default().selected_options =
        vec![whatsapp_rust::wacore::poll::compute_option_hash(&draft(true).options[2]).to_vec()];
    info.poll_updates.push(legacy);
    let mut addon = wa::MessageAddOn::default();
    addon.message_add_on_type = Some(wa::message_add_on::MessageAddOnType::POLL_UPDATE);
    let key = addon.message_add_on_key.get_or_insert_default();
    key.id = Some("CURRENT".into());
    key.from_me = Some(false);
    key.participant = Some(OWN.into());
    addon
        .legacy_message
        .get_or_insert_default()
        .poll_vote
        .get_or_insert_default()
        .selected_options =
        vec![whatsapp_rust::wacore::poll::compute_option_hash(&draft(true).options[0]).to_vec()];
    addon.sender_timestamp_ms = Some(1000);
    info.message_add_ons.push(addon);
    let votes = wire::history(&mut p, &info);
    assert_eq!(votes.len(), 1);
    assert_eq!(votes[0].id, "CURRENT");
    ingest(&db, std::iter::once(p.clone()).chain(votes).collect()).await;
    let chat = chat(&db, GROUP).await;
    let v = view(&db, chat).await;
    let id = v.messages[0].id;
    assert_eq!(poll(&v).view.counts, vec![1, 0, 0]);
    ingest(&db, vec![vote(&p, "V2", PN, &[0], 2000)]).await;
    let r = db.poll_results(chat, id, 0, 1).await.unwrap();
    assert_eq!(r.voters.len(), 1);
    assert!(r.more);
    // Expired/deleted poll envelopes cannot be revived by late votes.
    db.blocking(move |db| {
        db.execute("UPDATE messages SET expires=1 WHERE id=?1", [id])?;
        Ok(())
    })
    .unwrap();
    let _ = view(&db, chat).await;
    ingest(&db, vec![vote(&p, "TOO-LATE", OWN, &[1], 3000)]).await;
    assert!(db.queue_vote(chat, id, "BAD".into(), 0).await.is_err());
    db.blocking(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM poll_events", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT sum(length(raw)+length(content)) FROM messages WHERE visible=0",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
#[ignore = "Optimized synthetic poll archive measurement; no account traffic"]
async fn poll_archive_measurement() {
    use std::time::Instant;
    let (_dir, db) = fixture();
    let p = parent("MEASURE", true);
    let mut rows = vec![p.clone()];
    for i in 0..3000 {
        rows.push(vote(
            &p,
            &format!("V{i:06}"),
            &format!("1555{:07}@s.whatsapp.net", i % 100),
            &[i % 3],
            1000 + i as i64,
        ));
    }
    ingest(&db, rows).await;
    let chat = chat(&db, GROUP).await;
    let start = Instant::now();
    let v = view(&db, chat).await;
    let rebuild = start.elapsed();
    assert_eq!(poll(&v).view.voters, 100);
    let start = Instant::now();
    for _ in 0..100 {
        let _ = view(&db, chat).await;
    }
    let cached = start.elapsed() / 100;
    let start = Instant::now();
    ingest(&db, vec![vote(&p, "LAST", OWN, &[0, 1], 10000)]).await;
    let _ = view(&db, chat).await;
    let update = start.elapsed();
    println!(
        "poll_archive 3000 updates / 100 voters: rebuild={rebuild:?}, cache={cached:?}, ingest_to_view={update:?}"
    );
}

#[tokio::test]
async fn sent_echo_recovers_an_interrupted_creator_save_without_replacing_a_known_namespace() {
    const OWN_LID: &str = "10000000009@lid";
    let (_dir, db) = fixture();
    db.blocking(|db| {
        db.execute(
            "INSERT OR REPLACE INTO metadata(key,value) VALUES('identity',?1)",
            [OWN],
        )?;
        Ok(())
    })
    .unwrap();
    db.save_contacts(vec![], vec![(OWN_LID.into(), OWN.into())], true)
        .await
        .unwrap();
    db.ingest(archive::Batch {
        chats: vec![archive::ChatMeta {
            jid: GROUP.into(),
            group: true,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = chat(&db, GROUP).await;
    for (name, known) in [("RECOVER", false), ("PRESERVE", true)] {
        db.queue_poll(chat, name.into(), draft(false), 0)
            .await
            .unwrap();
        if known {
            db.poll_creator(chat, name.into(), OWN.into())
                .await
                .unwrap();
        }
        let name = name.to_string();
        let raw = db
            .blocking(|db| {
                Ok(
                    db.query_row("SELECT raw FROM messages WHERE wire_id=?1", [&name], |r| {
                        r.get::<_, Vec<u8>>(0)
                    })?,
                )
            })
            .unwrap();
        let mut echo = record(&name);
        echo.chat = GROUP.into();
        echo.outgoing = true;
        echo.sender = OWN_LID.into();
        echo.raw = raw;
        let message = codec::message_decode(&echo.raw).unwrap();
        let (text, content, _) = crate::protocol_content::extract(&message);
        echo.text = text.unwrap();
        echo.content = content;
        ingest(&db, vec![echo]).await;
        let creator = db
            .blocking(|db| {
                Ok(db.query_row(
                    "SELECT source_sender FROM messages WHERE wire_id=?1",
                    [&name],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap();
        assert_eq!(creator, if known { OWN } else { OWN_LID });
        let v = view(&db, chat).await;
        let target = v
            .messages
            .iter()
            .find(|m| m.content.metadata.wire == name)
            .unwrap();
        assert!(target.content.poll.as_ref().unwrap().view.can_vote);
        db.queue_vote(chat, target.id, format!("VOTE-{name}"), 0)
            .await
            .unwrap();
    }
}
