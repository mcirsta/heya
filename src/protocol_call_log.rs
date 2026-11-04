//! The call-history record for a voice or video call this device placed. It is
//! written to the call's chat and published to the account's `call_log`, so the
//! phone and other companions list it too. Result codes follow WA Web's VoIP
//! mapping: an unanswered or cancelled offer is missed for the callee, a decline
//! is rejected, a failure is unavailable.
use crate::archive::Archive;
use std::{
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::time::Instant;
use whatsapp_rust::{AppStateError, Client, prelude::wa, wacore_binary::Jid};

use wa::call_log_record::{CallResult, CallType, ParticipantInfo};

// Bounds the account write so a closing session releases the client promptly.
const PUBLISH: Duration = Duration::from_secs(10);

/// How an outgoing call's setup ended; the first one reported wins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Setup {
    Connected,
    Declined,
    Ended,
    Closed,
    Offline,
    MediaFailed,
    HungUp,
    Dropped,
}

impl Setup {
    pub(super) fn phrase(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Declined => "declined",
            Self::Ended => "ended",
            Self::Closed => "closed",
            Self::Offline => "offline",
            Self::MediaFailed => "media failed",
            Self::HungUp => "hung up",
            Self::Dropped => "dropped",
        }
    }
}

pub(super) struct Placed {
    client: Weak<Client>,
    archive: Archive,
    // Unix seconds when the offer was sent.
    started: i64,
    video: bool,
    connected: Option<Instant>,
    setup: Option<Setup>,
}

impl Placed {
    pub(super) fn new(client: &Arc<Client>, archive: Archive, video: bool) -> Self {
        Self {
            client: Arc::downgrade(client),
            archive,
            started: chrono::Utc::now().timestamp(),
            video,
            connected: None,
            setup: None,
        }
    }

    pub(super) fn setup(&mut self, setup: Setup) {
        if setup == Setup::Connected {
            self.connected.get_or_insert_with(Instant::now);
        }
        self.setup.get_or_insert(setup);
    }

    /// Writes the record off the caller's task. `declined` covers a reject
    /// that reached the signal queue after the SDK had already closed the call.
    pub(super) fn finish(self, call_id: &str, creator: &Jid, peer: &Jid, declined: bool) {
        let setup = if declined {
            Some(Setup::Declined)
        } else {
            self.setup
        };
        let creator = creator.to_non_ad();
        let log = record(
            call_id,
            &creator,
            peer,
            self.started,
            self.video,
            self.connected.map(|at| at.elapsed()),
            setup,
        );
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(publish(
            self.client,
            self.archive,
            creator,
            call_id.to_owned(),
            log,
        ));
    }
}

pub(super) fn record(
    call_id: &str,
    creator: &Jid,
    peer: &Jid,
    started: i64,
    video: bool,
    connected_for: Option<Duration>,
    setup: Option<Setup>,
) -> wa::CallLogRecord {
    let (result, participant) = match (connected_for, setup) {
        (Some(_), _) => (CallResult::CONNECTED, CallResult::CONNECTED),
        (None, Some(Setup::Declined)) => (CallResult::REJECTED, CallResult::REJECTED),
        (None, Some(Setup::Ended | Setup::Closed)) => (CallResult::MISSED, CallResult::MISSED),
        (None, Some(Setup::Offline | Setup::MediaFailed)) => {
            (CallResult::UNAVAILABLE, CallResult::MISSED)
        }
        (None, _) => (CallResult::CANCELLED, CallResult::MISSED),
    };
    let mut log = wa::CallLogRecord::default();
    log.call_id = Some(call_id.into());
    log.call_creator_jid = Some(creator.to_non_ad().to_string());
    log.call_result = Some(result);
    log.start_time = Some(started);
    log.duration = connected_for.map(|time| i64::try_from(time.as_secs()).unwrap_or(i64::MAX));
    // The literal direction, as the phone writes its own placed calls.
    log.is_incoming = Some(false);
    log.is_video = Some(video);
    log.call_type = Some(CallType::REGULAR);
    let mut callee = ParticipantInfo::default();
    callee.user_jid = Some(peer.to_non_ad().to_string());
    callee.call_result = Some(participant);
    log.participants.push(callee);
    log
}

async fn publish(
    client: Weak<Client>,
    archive: Archive,
    creator: Jid,
    id: String,
    log: wa::CallLogRecord,
) {
    // Local first: the chat shows the call even when the account log is not written.
    if crate::protocol_calls::save_placed(&archive, log.clone(), creator.clone(), id.clone())
        .await
        .is_err()
    {
        eprintln!("Heya call log: saving the local record failed");
    }
    let Some(client) = client.upgrade() else {
        return;
    };
    let sent = tokio::time::timeout(PUBLISH, client.send_call_log(&creator, &id, true, log)).await;
    drop(client);
    eprintln!(
        "Heya call log: {}",
        match sent {
            Ok(Ok(())) => "published to the account",
            Ok(Err(AppStateError::NotConnected)) => "not published: offline",
            Ok(Err(_)) => "not published: app-state request failed",
            Err(_) => "not published: timed out",
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        archive::{
            Batch,
            tests::{LID, PN, fixture},
        },
        message_content::{CallOutcome, Notice},
        protocol_calls as calls,
    };
    use whatsapp_rust::wacore::types::events::CallLogSync;

    const OWN: &str = "15550000099@s.whatsapp.net";
    const OWN_LID: &str = "10000000099@lid";

    fn parts() -> (Jid, Jid) {
        (
            OWN_LID.replace('@', ":3@").parse().unwrap(),
            LID.parse().unwrap(),
        )
    }

    #[test]
    fn each_setup_outcome_maps_to_whatsapp_call_results() {
        let (creator, peer) = parts();
        let cases = [
            (
                Some(Duration::from_secs(64)),
                Some(Setup::Connected),
                CallResult::CONNECTED,
                CallResult::CONNECTED,
            ),
            // A media failure after answering keeps the answered duration.
            (
                Some(Duration::from_secs(5)),
                Some(Setup::Connected),
                CallResult::CONNECTED,
                CallResult::CONNECTED,
            ),
            (
                None,
                Some(Setup::Declined),
                CallResult::REJECTED,
                CallResult::REJECTED,
            ),
            (
                None,
                Some(Setup::Ended),
                CallResult::MISSED,
                CallResult::MISSED,
            ),
            (
                None,
                Some(Setup::Closed),
                CallResult::MISSED,
                CallResult::MISSED,
            ),
            (
                None,
                Some(Setup::Offline),
                CallResult::UNAVAILABLE,
                CallResult::MISSED,
            ),
            (
                None,
                Some(Setup::MediaFailed),
                CallResult::UNAVAILABLE,
                CallResult::MISSED,
            ),
            (
                None,
                Some(Setup::HungUp),
                CallResult::CANCELLED,
                CallResult::MISSED,
            ),
            (None, None, CallResult::CANCELLED, CallResult::MISSED),
        ];
        for (connected, setup, result, participant) in cases {
            for video in [false, true] {
                let log = record(
                    "00call",
                    &creator,
                    &peer,
                    1_700_000_000,
                    video,
                    connected,
                    setup,
                );
                assert_eq!(log.call_result, Some(result), "{setup:?}");
                assert_eq!(log.participants.len(), 1);
                assert_eq!(log.participants[0].call_result, Some(participant));
                assert_eq!(log.participants[0].user_jid.as_deref(), Some(LID));
                assert_eq!(log.call_creator_jid.as_deref(), Some(OWN_LID));
                assert_eq!(
                    log.duration,
                    connected.map(|d| d.as_secs() as i64),
                    "{setup:?}"
                );
                assert_eq!(log.is_video, Some(video));
                assert_eq!(log.is_incoming, Some(false));
                assert_eq!(log.start_time, Some(1_700_000_000));
                assert_eq!(log.call_id.as_deref(), Some("00call"));
            }
        }
    }

    #[tokio::test]
    async fn a_placed_call_appears_once_in_its_chat_and_a_synced_record_replaces_it() {
        let (_temp, db) = fixture();
        db.bind_account(OWN.into(), String::new()).await.unwrap();
        db.ingest(Batch {
            aliases: vec![(OWN_LID.into(), OWN.into()), (LID.into(), PN.into())],
            ..Default::default()
        })
        .await
        .unwrap();
        let (creator, peer) = parts();
        let creator = creator.to_non_ad();
        let unanswered = record(
            "00placed",
            &creator,
            &peer,
            1_700_000_100,
            true,
            None,
            Some(Setup::Ended),
        );
        calls::save_placed(&db, unanswered.clone(), creator.clone(), "00placed".into())
            .await
            .unwrap();
        // Saving the same call again, as a resync would, keeps one row.
        calls::save_placed(&db, unanswered, creator.clone(), "00placed".into())
            .await
            .unwrap();
        let chats = db
            .view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats;
        assert_eq!(chats.len(), 1);
        let chat = chats[0].id;
        let view = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert_eq!(view.messages.len(), 1);
        assert!(view.messages[0].outgoing);
        assert_eq!(view.chat.preview, "Video call");
        assert_eq!(view.chat.call_kind, 2);
        assert_eq!(view.chat.unread, 0);
        let Some(Notice::Call(call)) = view.messages[0].content.notice else {
            panic!("missing call")
        };
        assert_eq!(call.outcome, CallOutcome::NoAnswer);
        assert_eq!(call.details(true), "Outgoing · No answer");

        // A later account record for the same call, e.g. the phone's, wins.
        let mut synced = record(
            "00placed",
            &creator,
            &peer,
            1_700_000_100,
            true,
            Some(Duration::from_secs(64)),
            Some(Setup::Connected),
        );
        synced.call_creator_jid = None;
        calls::save(
            &db,
            vec![
                CallLogSync::builder()
                    .call_creator_jid(creator)
                    .call_id("00placed".into())
                    .from_me(true)
                    .timestamp(chrono::Utc::now() + chrono::Duration::seconds(5))
                    .record(Box::new(synced))
                    .from_full_sync(false)
                    .build(),
            ],
        )
        .await
        .unwrap();
        let view = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert_eq!(view.messages.len(), 1);
        let Some(Notice::Call(call)) = view.messages[0].content.notice else {
            panic!("missing call")
        };
        assert_eq!(call.outcome, CallOutcome::Connected);
        assert_eq!(call.details(true), "Outgoing · 1:04");
    }
}
