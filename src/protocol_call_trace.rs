//! Content-free trace of an outgoing call's setup: how the SDK addressed the
//! offer and how the server and the callee's devices replied. The SDK reports a
//! callee's offer receipt ("ringing") and call-key re-request only as plain
//! receipts, so this reads the stanzas while the call is being set up. Lines
//! carry counts, fixed protocol words and milliseconds; never JIDs, call IDs,
//! tokens or keys.
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::time::Instant;
use whatsapp_rust::{
    Client, NodeFilter, NodeWaiter, RawNodeLease,
    prelude::{Event, EventHandler, EventInterest, EventKind},
    wacore::types::events::Subscription,
    wacore_binary::{Node, NodeRef},
};

// Replies kept per call; setup normally produces a handful.
const KEPT: usize = 64;

#[derive(Debug, PartialEq)]
pub(super) enum Reply {
    Ack {
        stanza: String,
        error: Option<String>,
    },
    // A callee device received the offer; WhatsApp shows "Ringing" from here.
    Ringing {
        call: String,
        device: String,
    },
    // A callee device could not decrypt the call key and asked for it again.
    KeyRequest {
        call: String,
    },
    Control {
        call: String,
        kind: &'static str,
        reason: Option<String>,
    },
}

fn attr(node: &NodeRef<'_>, key: &str) -> Option<String> {
    node.get_attr(key).map(|value| value.as_str().into_owned())
}

/// Protocol words only: anything else is reported as "other".
fn word(value: Option<String>) -> Option<String> {
    value.map(|value| {
        if !value.is_empty()
            && value.len() <= 24
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            value
        } else {
            "other".into()
        }
    })
}

pub(super) fn reply(node: &NodeRef<'_>) -> Option<Reply> {
    if node.tag == "ack" {
        return (attr(node, "class").as_deref() == Some("call")).then(|| Reply::Ack {
            stanza: attr(node, "id").unwrap_or_default(),
            error: word(attr(node, "error")),
        });
    }
    if node.tag == "receipt" {
        if attr(node, "type").as_deref() == Some("enc_rekey_retry") {
            let call = attr(node.get_optional_child("enc_rekey")?, "call-id")?;
            return Some(Reply::KeyRequest { call });
        }
        let call = attr(node.get_optional_child("offer")?, "call-id")?;
        let device = attr(node, "participant")
            .or_else(|| attr(node, "from"))
            .unwrap_or_default();
        return Some(Reply::Ringing { call, device });
    }
    if node.tag == "call" {
        for kind in ["preaccept", "accept", "reject", "terminate"] {
            if let Some(child) = node.get_optional_child(kind) {
                return Some(Reply::Control {
                    call: attr(child, "call-id")?,
                    kind,
                    reason: word(attr(child, "reason")),
                });
            }
        }
    }
    None
}

#[derive(Debug, PartialEq)]
pub(super) struct Offer {
    stanza: Option<String>,
    devices: usize,
    new_sessions: usize,
    privacy: bool,
}

pub(super) fn offer(node: &NodeRef<'_>) -> Option<Offer> {
    let offer = node.get_optional_child("offer")?;
    let keys: Vec<&NodeRef<'_>> = match offer.get_optional_child("destination") {
        Some(destination) => destination
            .get_children_by_tag("to")
            .filter_map(|to| to.get_optional_child("enc"))
            .collect(),
        None => offer.get_children_by_tag("enc").collect(),
    };
    Some(Offer {
        stanza: attr(node, "id"),
        devices: keys.len(),
        new_sessions: keys
            .iter()
            .filter(|enc| attr(enc, "type").as_deref() == Some("pkmsg"))
            .count(),
        privacy: offer.get_optional_child("privacy").is_some(),
    })
}

#[derive(Default)]
struct Replies(Mutex<Vec<(Reply, Instant)>>);

impl Replies {
    fn record(&self, reply: Reply) {
        let mut replies = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if replies.len() < KEPT {
            replies.push((reply, Instant::now()));
        }
    }
}

impl EventHandler for Replies {
    fn interest(&self) -> EventInterest {
        EventInterest::of(&[EventKind::RawNode])
    }

    // Inline callback: a tag check per stanza, one short lock for call replies.
    fn handle_event(&self, event: Arc<Event>) {
        if let Event::RawNode(node) = event.as_ref()
            && let Some(reply) = reply(node.get())
        {
            self.record(reply);
        }
    }
}

/// Reads stanzas from just before the offer until the setup outcome.
pub(super) struct Outgoing {
    replies: Arc<Replies>,
    _subscription: Subscription,
    _lease: RawNodeLease,
    sent_offer: NodeWaiter<Node>,
    began: Instant,
    sent: Option<(Instant, String, Option<Offer>)>,
}

impl Outgoing {
    pub(super) fn begin(client: &Arc<Client>) -> Self {
        let replies = Arc::new(Replies::default());
        Self {
            _subscription: client.subscribe(replies.interest(), replies.clone()),
            _lease: client.acquire_raw_node_forwarding(),
            sent_offer: client.wait_for_sent_node(NodeFilter::tag("call")),
            replies,
            began: Instant::now(),
            sent: None,
        }
    }

    /// The SDK returns the handle right after writing the offer.
    pub(super) fn sent(&mut self, call: &str) {
        let offer = self
            .sent_offer
            .try_recv()
            .ok()
            .flatten()
            .and_then(|node| offer(&node.as_node_ref()));
        self.sent = Some((Instant::now(), call.into(), offer));
    }

    /// One line per outgoing call; `outcome` is a fixed phrase.
    pub(super) fn summary(self, outcome: &str) -> String {
        let replies = std::mem::take(
            &mut *self
                .replies
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        summarize(self.began, self.sent, replies, outcome)
    }
}

fn summarize(
    began: Instant,
    sent: Option<(Instant, String, Option<Offer>)>,
    replies: Vec<(Reply, Instant)>,
    outcome: &str,
) -> String {
    let Some((sent, call, offer)) = sent else {
        return format!(
            "Heya call trace: no offer sent; setup {outcome} after {} ms",
            began.elapsed().as_millis()
        );
    };
    let after = |at: Instant| at.saturating_duration_since(sent).as_millis();
    let mut line = format!(
        "Heya call trace: offer prepared in {} ms",
        sent.saturating_duration_since(began).as_millis()
    );
    let stanza = match &offer {
        Some(offer) => {
            line += &format!(
                " for {} devices ({} new sessions), privacy token {}",
                offer.devices,
                offer.new_sessions,
                if offer.privacy { "attached" } else { "missing" }
            );
            offer.stanza.as_deref()
        }
        None => {
            line += ", offer stanza not captured";
            None
        }
    };
    let ack = replies.iter().find_map(|(reply, at)| match reply {
        Reply::Ack { stanza: id, error } if Some(id.as_str()) == stanza => Some((error, *at)),
        _ => None,
    });
    line += &match ack {
        Some((None, at)) => format!("; server ack after {} ms", after(at)),
        Some((Some(error), at)) => format!("; server error {error} after {} ms", after(at)),
        None => "; no server ack matched".into(),
    };
    let mut devices = HashSet::new();
    let mut first_ring = None;
    let mut key_requests = (0, None);
    let mut controls = Vec::new();
    for (reply, at) in &replies {
        match reply {
            Reply::Ringing { call: id, device } if *id == call => {
                devices.insert(device.as_str());
                first_ring.get_or_insert(*at);
            }
            Reply::KeyRequest { call: id } if *id == call => {
                key_requests.0 += 1;
                key_requests.1.get_or_insert(*at);
            }
            Reply::Control {
                call: id,
                kind,
                reason,
            } if *id == call && controls.len() < 4 => {
                controls.push(match reason {
                    Some(reason) => format!("{kind} ({reason}) after {} ms", after(*at)),
                    None => format!("{kind} after {} ms", after(*at)),
                });
            }
            _ => {}
        }
    }
    line += &format!("; ringing receipts from {} devices", devices.len());
    if let Some(at) = first_ring {
        line += &format!(" (first after {} ms)", after(at));
    }
    line += &format!("; call-key requests {}", key_requests.0);
    if let Some(at) = key_requests.1 {
        line += &format!(" (first after {} ms)", after(at));
    }
    if controls.is_empty() {
        line += "; no callee control";
    } else {
        line += &format!("; callee {}", controls.join(", "));
    }
    line += &format!("; setup {outcome} after {} ms", sent.elapsed().as_millis());
    line
}

#[cfg(test)]
#[path = "protocol_call_trace_tests.rs"]
mod tests;
