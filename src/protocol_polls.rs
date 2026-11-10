//! Poll message formats and SDK cryptography, independent of Slint/storage types.
use crate::{
    archive::{Error, Result},
    message_actions::{Prepared, Source},
    message_content::Content,
    polls::{Ballot, Draft, Poll},
};
use whatsapp_rust::{
    prelude::{MessageExt, wa},
    wacore::poll,
    waproto::{buffa::MessageField, codec},
};

pub fn creation(message: &wa::Message) -> Option<&wa::message::PollCreationMessage> {
    let m = message.get_base_message();
    m.poll_creation_message
        .as_option()
        .or(m.poll_creation_message_v2.as_option())
        .or(m.poll_creation_message_v3.as_option())
        .or(m.poll_creation_message_v5.as_option())
        .or(m.poll_creation_message_v6.as_option())
}
pub fn secret(message: &wa::Message) -> Option<&[u8]> {
    let mut current = message;
    for _ in 0..32 {
        if let Some(secret) = current
            .message_context_info
            .message_secret
            .as_deref()
            .filter(|s| s.len() == 32)
        {
            return Some(secret);
        }
        let next = current
            .device_sent_message
            .message
            .as_option()
            .or(current.ephemeral_message.message.as_option())
            .or(current.associated_child_message.message.as_option());
        let Some(next) = next else { break };
        current = next;
    }
    creation(message)?
        .enc_key
        .as_deref()
        .filter(|s| s.len() == 32)
}
pub fn definition(message: &wa::Message) -> Option<Poll> {
    if message.is_view_once() {
        return None;
    }
    let p = creation(message)?;
    let question = p.name.as_ref()?.clone();
    let options = p
        .options
        .iter()
        .map(|o| o.option_name.clone())
        .collect::<Option<Vec<_>>>()?;
    if question.is_empty()
        || question.chars().count() > 2048
        || !(2..=12).contains(&options.len())
        || options
            .iter()
            .any(|o| o.is_empty() || o.chars().count() > 1024)
        || options
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != options.len()
    {
        return None;
    }
    let selectable = match p.selectable_options_count.unwrap_or(0) as usize {
        0 => options.len(),
        n if n <= options.len() => n,
        _ => return None,
    };
    let restriction = if p
        .poll_content_type
        .is_some_and(|t| t != wa::message::PollContentType::TEXT)
        || p.poll_type == Some(wa::message::PollType::QUIZ)
        || p.allow_add_option == Some(true)
        || p.hide_participant_name == Some(true)
    {
        "This poll uses features that need WhatsApp on your phone.".into()
    } else {
        String::new()
    };
    Some(Poll {
        question,
        options,
        selectable,
        end: p
            .end_time
            .filter(|s| *s > 0)
            .map(|s| if s > 100_000_000_000 { s / 1000 } else { s }),
        restriction,
        ready: secret(message).is_some(),
        ..Default::default()
    })
}
pub fn text(p: &Poll) -> String {
    format!("Poll: {}\n{}", p.question, p.options.join("\n"))
}
pub fn from_raw(raw: &[u8]) -> Option<Poll> {
    definition(&codec::message_decode(raw).ok()?)
}
pub fn merge_secret(old: &[u8], new: &[u8]) -> Option<Vec<u8>> {
    let incoming = codec::message_decode(new).ok()?;
    let incoming_poll = definition(&incoming)?;
    let secret = secret(&incoming)?.to_vec();
    let mut original = codec::message_decode(old).ok()?;
    let previous = definition(&original)?;
    if self::secret(&original).is_some()
        || previous.question != incoming_poll.question
        || previous.options != incoming_poll.options
        || previous.selectable != incoming_poll.selectable
    {
        return None;
    }
    original
        .message_context_info
        .get_or_insert_default()
        .message_secret = Some(secret);
    Some(codec::message_to_vec(&original))
}
pub fn ballot(message: &wa::Message) -> Option<Ballot> {
    if message.is_view_once() {
        return None;
    }
    let u = message.get_base_message().poll_update_message.as_option()?;
    let key = u.poll_creation_message_key.as_option()?;
    let target = key
        .id
        .as_ref()
        .filter(|s| !s.is_empty() && s.len() <= 256)?
        .clone();
    let vote = u.vote.as_option()?;
    if vote.enc_iv.as_ref()?.len() != 12 || !(16..=4096).contains(&vote.enc_payload.as_ref()?.len())
    {
        return None;
    }
    Some(Ballot {
        target,
        author: key.participant.clone().unwrap_or_default(),
        remote: key.remote_jid.clone().unwrap_or_default(),
        stamp: u.sender_timestamp_ms.unwrap_or(0),
        hashes: None,
    })
}
pub fn extract(message: &wa::Message) -> Option<(String, Content)> {
    if let Some(poll) = definition(message) {
        return Some((
            text(&poll),
            Content {
                body: Some(String::new()),
                poll: Some(poll),
                ..Default::default()
            },
        ));
    }
    ballot(message).map(|ballot| {
        (
            String::new(),
            Content {
                ballot: Some(ballot),
                ..Default::default()
            },
        )
    })
}
pub fn indices(p: &Poll, hashes: &[Vec<u8>]) -> Option<Vec<usize>> {
    if hashes.len() > p.selectable {
        return None;
    }
    let all = p
        .options
        .iter()
        .map(|s| poll::compute_option_hash(s))
        .collect::<Vec<_>>();
    let mut selected = Vec::new();
    for h in hashes {
        let i = all.iter().position(|v| v.as_slice() == h.as_slice())?;
        if selected.contains(&i) {
            return None;
        }
        selected.push(i);
    }
    selected.sort_unstable();
    Some(selected)
}
pub fn decrypt(
    parent: &[u8],
    event: &[u8],
    wire: &str,
    creators: &[String],
    voters: &[String],
) -> Option<Vec<usize>> {
    let parent = codec::message_decode(parent).ok()?;
    let p = definition(&parent)?;
    let secret = secret(&parent)?;
    let event = codec::message_decode(event).ok()?;
    let v = event
        .get_base_message()
        .poll_update_message
        .vote
        .as_option()?;
    let cipher = poll::PollVoteCiphertext {
        enc_payload: v.enc_payload.as_deref()?,
        enc_iv: v.enc_iv.as_deref()?,
    };
    for creator in creators {
        for voter in voters {
            if let Ok(hashes) =
                poll::decrypt_poll_vote_with_secret(cipher, secret, wire, creator, voter)
            {
                return indices(&p, &hashes);
            }
        }
    }
    None
}
pub fn prepare(draft: &Draft, reply: Option<&Source>, owner: &str) -> Result<Prepared> {
    if !draft.valid() {
        return Err(Error::Invalid);
    }
    let mut p = wa::message::PollCreationMessage::default();
    p.name = Some(draft.question.clone());
    p.selectable_options_count = Some(if draft.multiple {
        draft.options.len() as u32
    } else {
        1
    });
    p.poll_content_type = Some(wa::message::PollContentType::TEXT);
    p.options = draft
        .options
        .iter()
        .map(|s| {
            let mut option = wa::message::poll_creation_message::Option::default();
            option.option_name = Some(s.clone());
            option
        })
        .collect();
    let mut m = wa::Message::default();
    if draft.multiple {
        m.poll_creation_message = MessageField::some(p);
    } else {
        m.poll_creation_message_v3 = MessageField::some(p);
    }
    let mut secret = vec![0; 32];
    getrandom::fill(&mut secret).map_err(|_| Error::Database)?;
    m.message_context_info
        .get_or_insert_default()
        .message_secret = Some(secret);
    crate::protocol_actions::quote(&mut m, reply, owner)?;
    let (text, content, thumbnail) = crate::protocol_content::extract(&m);
    Ok(Prepared {
        raw: codec::message_to_vec(&m),
        text: text.ok_or(Error::Invalid)?,
        content,
        thumbnail,
        visible: true,
    })
}
pub fn vote(
    source: &Source,
    selected: &[usize],
    creator: &str,
    voter: &str,
    stamp: i64,
) -> Result<Prepared> {
    let original = codec::message_decode(&source.raw).map_err(|_| Error::Invalid)?;
    let p = definition(&original).ok_or(Error::Invalid)?;
    if !p.restriction.is_empty()
        || selected.len() > p.selectable
        || selected.iter().any(|i| *i >= p.options.len())
    {
        return Err(Error::Invalid);
    }
    let secret = secret(&original).ok_or(Error::Invalid)?;
    let hashes = selected
        .iter()
        .map(|i| poll::compute_option_hash(&p.options[*i]).to_vec())
        .collect::<Vec<_>>();
    if indices(&p, &hashes).is_none() {
        return Err(Error::Invalid);
    }
    let (enc, iv) =
        poll::encrypt_poll_vote_with_secret(&hashes, secret, &source.wire, creator, voter)
            .map_err(|_| Error::Invalid)?;
    let mut key = wa::MessageKey::default();
    key.id = Some(source.wire.clone());
    key.remote_jid = Some(source.jid.clone());
    key.from_me = Some(source.outgoing);
    if source.jid.ends_with("@g.us") {
        key.participant = Some(creator.into());
    }
    let mut m = wa::Message::default();
    let u = m.poll_update_message.get_or_insert_default();
    u.poll_creation_message_key = MessageField::some(key);
    u.sender_timestamp_ms = Some(stamp);
    let mut value = wa::message::PollEncValue::default();
    value.enc_payload = Some(enc);
    value.enc_iv = Some(iv.to_vec());
    u.vote = MessageField::some(value);
    let (_, content, _) = crate::protocol_content::extract(&m);
    Ok(Prepared {
        raw: codec::message_to_vec(&m),
        text: String::new(),
        content,
        thumbnail: Vec::new(),
        visible: false,
    })
}

/// Recover the creation's secret and authoritative poll add-ons carried by history.
pub fn history(
    parent: &mut crate::archive::Record,
    info: &wa::WebMessageInfo,
) -> Vec<crate::archive::Record> {
    use crate::archive::Record;
    if parent.content.poll.is_none() {
        return Vec::new();
    }
    if let Ok(mut m) = codec::message_decode(&parent.raw)
        && secret(&m).is_none()
        && let Some(s) = info.message_secret.as_ref().filter(|s| s.len() == 32)
    {
        m.message_context_info
            .get_or_insert_default()
            .message_secret = Some(s.clone());
        let (text, content, thumbnail) = crate::protocol_content::extract(&m);
        parent.text = text.unwrap_or_else(|| parent.text.clone());
        parent.content = content;
        parent.thumbnail = thumbnail;
        parent.raw = codec::message_to_vec(&m);
    }
    if parent.outgoing
        && let Some(own) = info
            .original_self_author_user_jid_string
            .as_ref()
            .filter(|s| s.ends_with("@lid") || s.ends_with("@s.whatsapp.net"))
    {
        parent.sender = own.clone();
    }
    let mut updates = Vec::new();
    if info.message_add_ons.is_empty() {
        for u in &info.poll_updates {
            if let (Some(key), Some(vote)) =
                (u.poll_update_message_key.as_option(), u.vote.as_option())
            {
                updates.push((key, vote, u.sender_timestamp_ms.or(u.server_timestamp_ms)));
            }
        }
    } else {
        for u in &info.message_add_ons {
            if u.message_add_on_type != Some(wa::message_add_on::MessageAddOnType::POLL_UPDATE) {
                continue;
            }
            if let (Some(key), Some(vote)) = (
                u.message_add_on_key.as_option(),
                u.legacy_message.poll_vote.as_option(),
            ) {
                updates.push((key, vote, u.sender_timestamp_ms.or(u.server_timestamp_ms)));
            }
        }
    }
    updates
        .into_iter()
        .take(4096)
        .filter_map(|(key, vote, version)| {
            let wire = key
                .id
                .as_ref()
                .filter(|s| !s.is_empty() && s.len() <= 256 && *s != &parent.id)?
                .clone();
            if vote.selected_options.len() > 12
                || vote.selected_options.iter().any(|h| h.len() != 32)
            {
                return None;
            }
            let own = key.from_me?;
            let sender = if own {
                "#self".into()
            } else if parent.chat.ends_with("@g.us") {
                let jid = key
                    .participant
                    .as_ref()?
                    .parse::<whatsapp_rust::Jid>()
                    .ok()?;
                if !(jid.is_pn() || jid.is_lid()) {
                    return None;
                }
                crate::protocol_messages::address(&jid)
            } else {
                parent.chat.clone()
            };
            let stamp = version.unwrap_or(parent.timestamp.saturating_mul(1000));
            Some(Record {
                chat: parent.chat.clone(),
                sender,
                id: wire,
                timestamp: stamp / 1000,
                outgoing: own,
                sender_name: String::new(),
                text: String::new(),
                raw: Vec::new(),
                content: Content {
                    ballot: Some(Ballot {
                        target: parent.id.clone(),
                        author: if parent.outgoing {
                            "#self".into()
                        } else {
                            parent.sender.clone()
                        },
                        remote: parent.chat.clone(),
                        stamp,
                        hashes: Some(vote.selected_options.clone()),
                    }),
                    ..Default::default()
                },
                album: None,
                thumbnail: Vec::new(),
                visible: false,
                expires: parent.expires,
                change: None,
            })
        })
        .collect()
}
