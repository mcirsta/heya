use super::*;
use crate::{
    archive::{
        self,
        tests::{LID, PN, fixture, record},
    },
    message_actions::Command,
};
use whatsapp_rust::{
    prelude::{MessageBuilderExt, MessageExt, wa},
    waproto::codec,
};

#[test]
fn single_and_bundle_envelopes_round_trip_vcards_and_quotes_without_importing_extra_fields() {
    let contacts = vec![
        Choice {
            name: "Ștefan, Sample; 👩🏽‍💻 \\ Example".into(),
            number: "15550000001".into(),
        },
        Choice {
            name: "Mîra Sample".into(),
            number: "15550000002".into(),
        },
    ];
    let reply = crate::message_actions::Source {
        id: 1,
        chat: 1,
        jid: PN.into(),
        sender: PN.into(),
        wire: "SYNTHETIC-QUOTE".into(),
        outgoing: false,
        body: "Quoted text".into(),
        raw: codec::message_to_vec(&wa::Message::text("Quoted text")),
    };
    for count in [1, 2] {
        let prepared =
            crate::protocol_contacts::prepare(&contacts[..count], Some(&reply), PN).unwrap();
        let message = codec::message_decode(&prepared.raw).unwrap();
        assert_eq!(message.contact_message.is_set(), count == 1);
        assert_eq!(message.contacts_array_message.is_set(), count == 2);
        assert_eq!(prepared.content.contacts.len(), count);
        assert_eq!(prepared.content.contacts[0].name, contacts[0].name);
        assert_eq!(prepared.content.contacts[0].account(), contacts[0].number);
        assert_eq!(
            message.context_info().unwrap().stanza_id.as_deref(),
            Some("SYNTHETIC-QUOTE")
        );
        let exported =
            String::from_utf8(crate::protocol_contacts::file(&prepared.raw).unwrap()).unwrap();
        assert!(!exported.contains("EMAIL:") && !exported.contains("NOTE:"));
        assert_eq!(exported.matches("BEGIN:VCARD").count(), count);
    }
    for invalid in [
        vec![],
        vec![contacts[0].clone(); 2],
        vec![Choice {
            name: "Injected\r\nEMAIL:bad".into(),
            number: contacts[0].number.clone(),
        }],
        vec![Choice {
            name: "Sample".into(),
            number: "bad".into(),
        }],
    ] {
        assert!(crate::protocol_contacts::prepare(&invalid, None, PN).is_err());
    }
    let mut private = wa::Message::default();
    private.view_once_message.get_or_insert_default().message =
        whatsapp_rust::waproto::buffa::MessageField::some(wa::Message::text("private"));
    let mut reply = reply;
    reply.raw = codec::message_to_vec(&private);
    assert!(crate::protocol_contacts::prepare(&contacts, Some(&reply), PN).is_err());
    let many = (0..MAX_SELECTED)
        .map(|i| Choice {
            name: format!("Sample {i}"),
            number: format!("1555{i:07}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        crate::protocol_contacts::prepare(&many, None, PN)
            .unwrap()
            .content
            .contacts
            .len(),
        MAX_SELECTED
    );
    let mut too_many = many;
    too_many.push(contacts[0].clone());
    assert!(crate::protocol_contacts::prepare(&too_many, None, PN).is_err());
}

#[tokio::test]
async fn phonebook_search_is_unicode_bounded_honors_aliases_and_excludes_locked_or_unknown_numbers()
{
    let (_dir, db) = fixture();
    let mut contacts = (0..103)
        .map(|i| archive::Contact {
            jid: format!("1556{i:07}@s.whatsapp.net"),
            name: format!("Sample {i:03}"),
            timestamp: Some(1),
        })
        .collect::<Vec<_>>();
    contacts.extend([
        archive::Contact {
            jid: LID.into(),
            name: "Ștefan Sample".into(),
            timestamp: Some(1),
        },
        archive::Contact {
            jid: "19990000001@lid".into(),
            name: "Unknown number".into(),
            timestamp: Some(1),
        },
        archive::Contact {
            jid: "15550000002@s.whatsapp.net".into(),
            name: "Locked contact".into(),
            timestamp: Some(1),
        },
    ]);
    db.save_contacts(contacts, vec![(LID.into(), PN.into())], true)
        .await
        .unwrap();
    db.blocking(|db|{db.execute("INSERT INTO chat_settings(jid,kind,value,stamp) VALUES('15550000002@s.whatsapp.net',2,1,1)",[])?;Ok(())}).unwrap();
    let page = db.contact_choices("".into()).await.unwrap();
    assert_eq!(page.choices.len(), PAGE_SIZE);
    assert!(page.more);
    let page = db.contact_choices("ȘTEFAN".into()).await.unwrap();
    assert_eq!(page.choices.len(), 1);
    assert_eq!(page.choices[0].number, "15550000001");
    assert_eq!(
        db.contact_choices("+1 (555) 000-0001".into())
            .await
            .unwrap()
            .choices
            .len(),
        1
    );
    assert!(
        db.contact_choices("Locked".into())
            .await
            .unwrap()
            .choices
            .is_empty()
    );
    assert!(
        db.contact_choices("Unknown".into())
            .await
            .unwrap()
            .choices
            .is_empty()
    );
    assert_eq!(
        db.blocking(|db| Ok(
            db.query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i32>(0))?
        ))
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn contacts_use_the_durable_outbox_and_keep_drafts_with_reply_and_destination_guards() {
    let (_dir, db) = fixture();
    let mut source = record("quoted");
    source.raw = codec::message_to_vec(&wa::Message::text("Quoted text"));
    db.ingest(archive::Batch {
        messages: vec![source],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    let quoted = view.messages[0].id;
    db.draft(chat, "Keep this composer draft 👩🏽‍💻".into())
        .await
        .unwrap();
    db.set_reply(chat, quoted).await.unwrap();
    let contacts = demo("").choices.to_vec();
    db.queue_contacts(chat, "CONTACTS-ONE".into(), contacts.clone(), quoted)
        .await
        .unwrap();
    db.queue_contacts(chat, "CONTACTS-ONE".into(), contacts.clone(), quoted)
        .await
        .unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.messages.len(), 2);
    assert_eq!(view.draft, "Keep this composer draft 👩🏽‍💻");
    let outgoing = view.messages.last().unwrap();
    assert!(outgoing.outgoing);
    assert_eq!(outgoing.content.contacts.len(), 4);
    assert!(outgoing.content.quote.is_some());
    assert!(
        db.queue_contacts(chat, "INVALID".into(), vec![], 0)
            .await
            .is_err()
    );
    db.hide_remote(PN.into(), PN.into(), "quoted".into(), false)
        .await
        .unwrap();
    assert!(
        db.queue_contacts(chat, "BAD-QUOTE".into(), contacts.clone(), quoted)
            .await
            .is_err()
    );
    db.blocking(move |db| {
        db.execute("UPDATE chats SET read_only=1 WHERE id=?1", [chat])?;
        Ok(())
    })
    .unwrap();
    assert!(
        db.queue_contacts(chat, "READONLY".into(), contacts, 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn accepted_commands_keep_acknowledgements_even_without_storage() {
    use tokio::sync::{mpsc, watch};
    let (_archives, archives) = watch::channel(None);
    let (tx, rx) = mpsc::channel(8);
    let (updates, mut states) = watch::channel(crate::messaging::Snapshot::empty());
    let task = tokio::spawn(crate::messaging::run(archives, rx, updates));
    for (request, command, kind) in [
        (
            1,
            Command::Contacts {
                query: "Sample".into(),
                request: 1,
            },
            10,
        ),
        (
            2,
            Command::SendContacts {
                chat: 1,
                contacts: demo("").choices.to_vec(),
                reply: 0,
                request: 2,
            },
            11,
        ),
    ] {
        tx.send(crate::messaging::Command::Action(command))
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            states.wait_for(|s| {
                s.operation
                    .as_ref()
                    .is_some_and(|r| r.request == request && r.kind == kind && !r.saved)
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!states.borrow().operation.as_ref().unwrap().error.is_empty());
    }
    drop(tx);
    task.await.unwrap();
}
