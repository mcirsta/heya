//! SDK contact envelopes and opaque vCard exports stay in the adapter.
use crate::{
    message_content::Content,
    shared_contacts::{self, Contact},
};
use whatsapp_rust::{
    prelude::{MessageExt, wa},
    waproto::codec,
};
pub fn extract(message: &wa::Message) -> Option<(String, Content)> {
    let contacts = if let Some(one) = message.contact_message.as_option() {
        vec![one]
    } else {
        message
            .contacts_array_message
            .as_option()?
            .contacts
            .iter()
            .take(shared_contacts::MAX_CARDS)
            .collect()
    };
    let mut budget = shared_contacts::MAX_BYTES;
    let contacts: Vec<Contact> = contacts
        .into_iter()
        .take_while(|c| {
            let n = c.vcard.as_ref().map_or(0, String::len);
            if n > budget {
                return false;
            }
            budget -= n;
            true
        })
        .map(|c| {
            shared_contacts::parse(
                c.display_name.as_deref().unwrap_or(""),
                c.vcard.as_deref().unwrap_or(""),
            )
        })
        .collect();
    let text = contacts
        .iter()
        .map(|c| format!("{}\n{}", c.name, c.details()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let unavailable = contacts.is_empty();
    Some((
        if text.is_empty() {
            "Contact unavailable".into()
        } else {
            text
        },
        Content {
            body: Some(if unavailable {
                "Contact unavailable".into()
            } else {
                String::new()
            }),
            contacts,
            has_attachment: !unavailable,
            ..Default::default()
        },
    ))
}
pub fn file(raw: &[u8]) -> Option<Vec<u8>> {
    let message = codec::message_decode(raw).ok()?;
    if message.is_view_once() {
        return None;
    }
    let base = message.get_base_message();
    let cards = if let Some(c) = base.contact_message.as_option() {
        vec![c]
    } else {
        base.contacts_array_message
            .as_option()?
            .contacts
            .iter()
            .take(shared_contacts::MAX_CARDS)
            .collect()
    };
    let mut out = String::new();
    for card in cards {
        let text = card.vcard.as_deref()?.trim();
        if !text.to_ascii_uppercase().starts_with("BEGIN:VCARD")
            || !text.to_ascii_uppercase().ends_with("END:VCARD")
        {
            return None;
        }
        if out.len() + text.len() > shared_contacts::MAX_BYTES {
            return None;
        }
        out.push_str(text);
        out.push_str("\r\n");
    }
    (!out.is_empty()).then(|| out.into_bytes())
}

/// Construct ordinary contact envelopes using the SDK public message types.
pub fn prepare(
    contacts: &[crate::contact_sending::Choice],
    reply: Option<&crate::message_actions::Source>,
    owner: &str,
) -> crate::archive::Result<crate::message_actions::Prepared> {
    use crate::archive::Error;
    use whatsapp_rust::waproto::buffa::MessageField;
    if contacts.is_empty() || contacts.len() > crate::contact_sending::MAX_SELECTED {
        return Err(Error::Invalid);
    }
    let mut numbers = std::collections::HashSet::new();
    let mut cards = Vec::with_capacity(contacts.len());
    for c in contacts {
        if !c.valid() || !numbers.insert(&c.number) {
            return Err(Error::Invalid);
        }
        let escaped = c
            .name
            .replace('\\', "\\\\")
            .replace(';', "\\;")
            .replace(',', "\\,");
        let mut card = wa::message::ContactMessage::default();
        card.display_name = Some(c.name.clone());
        card.vcard = Some(format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nN:;{escaped};;;\r\nFN:{escaped}\r\nTEL;TYPE=CELL;waid={}:+{}\r\nEND:VCARD\r\n",
            c.number, c.number
        ));
        cards.push(card);
    }
    let mut message = wa::Message::default();
    if cards.len() == 1 {
        message.contact_message = MessageField::some(cards.remove(0));
    } else {
        let bundle = message.contacts_array_message.get_or_insert_default();
        bundle.display_name = Some(format!("{} contacts", cards.len()));
        bundle.contacts = cards;
    }
    crate::protocol_actions::quote(&mut message, reply, owner)?;
    let (text, content, thumbnail) = crate::protocol_content::extract(&message);
    Ok(crate::message_actions::Prepared {
        raw: codec::message_to_vec(&message),
        text: text.ok_or(Error::Invalid)?,
        content,
        thumbnail,
        visible: true,
    })
}
