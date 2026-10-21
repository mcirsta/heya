//! Call-log records only. No call engine, capture device or signalling worker.
use crate::{
    archive::{self, Archive, Batch, Record},
    message_content::{Call, CallOutcome, Content, Notice},
};
use whatsapp_rust::{
    prelude::{Event, wa},
    wacore::types::events::CallLogSync,
    wacore_binary::{Jid, JidExt},
};

/// Finished signalling can arrive before the phone's durable log. Its unknown
/// media type stays unknown until the authoritative record fills it in.
pub fn signal(event: &Event) -> Option<Record> {
    let (from, id, stamp, outcome) = match event {
        Event::MissedCall(c) => (
            &c.from,
            &c.call_id,
            c.timestamp,
            wa::call_log_record::CallResult::MISSED,
        ),
        Event::CallEndedElsewhere(c) => {
            let outcome = match c.outcome {
                whatsapp_rust::wacore::types::call::ElsewhereOutcome::Accepted => {
                    wa::call_log_record::CallResult::ACCEPTEDELSEWHERE
                }
                whatsapp_rust::wacore::types::call::ElsewhereOutcome::Rejected => {
                    wa::call_log_record::CallResult::REJECTED
                }
                _ => return None,
            };
            (&c.from, &c.call_id, c.timestamp, outcome)
        }
        _ => return None,
    };
    let mut log = wa::CallLogRecord::default();
    log.start_time = Some(stamp.timestamp());
    log.call_result = Some(outcome);
    record(&log, from, id, false, &[], -1)
}

pub fn message(log: &wa::message::CallLogMessage) -> Call {
    use wa::message::call_log_message::CallOutcome as O;
    Call {
        video: log.is_video,
        seconds: log.duration_secs.and_then(|s| u64::try_from(s).ok()),
        revision: 0,
        outcome: match log.call_outcome {
            Some(O::CONNECTED) => CallOutcome::Connected,
            Some(O::MISSED | O::SILENCED_BY_DND | O::SILENCED_UNKNOWN_CALLER) => {
                CallOutcome::Missed
            }
            Some(O::REJECTED) => CallOutcome::Declined,
            Some(O::FAILED) => CallOutcome::Failed,
            Some(O::ACCEPTED_ELSEWHERE) => CallOutcome::AnsweredElsewhere,
            Some(O::ONGOING) => CallOutcome::Ongoing,
            _ => CallOutcome::Unknown,
        },
    }
}

pub fn record(
    log: &wa::CallLogRecord,
    creator: &Jid,
    id: &str,
    outgoing: bool,
    own: &[Jid],
    revision: i64,
) -> Option<Record> {
    use wa::call_log_record::CallResult as O;
    let stamp = log
        .start_time
        .filter(|s| *s > 0 && chrono::DateTime::from_timestamp(*s, 0).is_some())?;
    if id.is_empty() || !(creator.is_pn() || creator.is_lid()) {
        return None;
    }
    let chat = if let Some(group) = log.group_jid.as_deref() {
        let group = group.parse::<Jid>().ok()?;
        if !group.is_group() {
            return None;
        }
        group
    } else if outgoing {
        if log.is_call_link == Some(true) {
            return None;
        }
        let mut peers = log
            .participants
            .iter()
            .filter_map(|p| p.user_jid.as_deref()?.parse::<Jid>().ok())
            .map(|j| j.to_non_ad())
            .filter(|j| (j.is_pn() || j.is_lid()) && !own.contains(j));
        let peer = peers.next()?;
        // A multi-party call without a group ID has no unique chat destination.
        if peers.any(|j| j != peer) {
            return None;
        }
        peer
    } else {
        creator.to_non_ad()
    };
    let call = Call {
        video: log.is_video,
        seconds: log.duration.and_then(|s| u64::try_from(s).ok()),
        revision,
        outcome: match log.call_result {
            Some(O::CONNECTED) => CallOutcome::Connected,
            Some(O::MISSED) => CallOutcome::Missed,
            Some(O::REJECTED) => CallOutcome::Declined,
            Some(O::CANCELLED | O::ABANDONED) => CallOutcome::Cancelled,
            Some(O::ACCEPTEDELSEWHERE) => CallOutcome::AnsweredElsewhere,
            Some(O::FAILED | O::UNAVAILABLE) => CallOutcome::Failed,
            Some(O::ONGOING) => CallOutcome::Ongoing,
            Some(O::UPCOMING) => CallOutcome::Scheduled,
            _ => CallOutcome::Unknown,
        },
    };
    Some(Record {
        chat: super::protocol_messages::address(&chat),
        sender: super::protocol_messages::address(creator),
        id: id.into(),
        timestamp: stamp,
        outgoing,
        sender_name: String::new(),
        text: call.label().into(),
        raw: Vec::new(),
        thumbnail: Vec::new(),
        album: None,
        content: Content {
            notice: Some(Notice::Call(call)),
            ..Default::default()
        },
        visible: true,
        expires: None,
        change: None,
    })
}

pub fn history(log: &wa::CallLogRecord, own: &[Jid]) -> Option<Record> {
    let creator = log
        .call_creator_jid
        .as_deref()?
        .parse::<Jid>()
        .ok()?
        .to_non_ad();
    if own.is_empty() {
        return None;
    }
    record(
        log,
        &creator,
        log.call_id.as_deref()?,
        own.contains(&creator),
        own,
        0,
    )
}

pub fn account(archive: &Archive) -> archive::Result<Vec<Jid>> {
    Ok(archive
        .call_account()?
        .into_iter()
        .filter_map(|s| s.parse().ok())
        .collect())
}

pub async fn save(archive: &Archive, calls: Vec<CallLogSync>) -> archive::Result<()> {
    if calls.is_empty() {
        return Ok(());
    }
    let archive = archive.clone();
    tokio::task::spawn_blocking(move || {
        let own = account(&archive)?;
        let batch = Batch {
            messages: calls
                .into_iter()
                .filter_map(|c| {
                    record(
                        &c.record,
                        &c.call_creator_jid,
                        &c.call_id,
                        c.from_me,
                        &own,
                        c.timestamp.timestamp_millis(),
                    )
                })
                .collect(),
            ..Default::default()
        };
        archive.ingest_blocking(batch)
    })
    .await
    .map_err(|_| archive::Error::Database)?
}
