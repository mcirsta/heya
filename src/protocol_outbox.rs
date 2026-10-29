//! Serial outbox delivery. The transport seam tests the real worker without a network.
use crate::archive::{self, Archive, Outgoing};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;
use whatsapp_rust::{
    Client,
    prelude::{MessageBuilderExt, SendOptions, SendRequest, wa},
};

#[derive(Clone, Copy)]
enum Attempt {
    Submitted,
    Confirmed,
    Rejected,
    Unconfirmed,
}

#[async_trait::async_trait]
trait Transport: Send + Sync {
    async fn ready(&self) -> bool;
    async fn send(&self, job: Outgoing) -> Attempt;
}
struct WhatsApp(Arc<Client>);
#[async_trait::async_trait]
impl Transport for WhatsApp {
    async fn ready(&self) -> bool {
        self.0
            .wait_for_session_ready(Duration::from_secs(30))
            .await
            .is_ok()
    }
    async fn send(&self, job: Outgoing) -> Attempt {
        let Ok(jid) = job.jid.parse::<whatsapp_rust::wacore_binary::Jid>() else {
            return Attempt::Rejected;
        };
        let Ok(id) = whatsapp_rust::MessageId::new(&job.id) else {
            return Attempt::Rejected;
        };
        if let Some(control) = job.control {
            return send_control(&self.0, &jid, &job.id, control).await;
        }
        let mut options = SendOptions::default().with_message_id(id);
        if job.ephemeral > 0 {
            options = options.with_ephemeral_expiration(job.ephemeral);
        }
        let message = if job.raw.is_empty() {
            wa::Message::text(job.text)
        } else {
            let Ok(message) = whatsapp_rust::waproto::codec::message_decode(&job.raw) else {
                return Attempt::Rejected;
            };
            message
        };
        let request = SendRequest::new(&jid, message).with_options(options);
        submitted(self.0.send(request).await)
    }
}

pub async fn run(
    archive: Archive,
    client: Arc<Client>,
    online: watch::Receiver<bool>,
    stop: watch::Receiver<bool>,
) -> archive::Result<()> {
    worker(
        archive,
        WhatsApp(client),
        online,
        stop,
        Duration::from_secs(45),
    )
    .await
}

async fn worker(
    archive: Archive,
    transport: impl Transport,
    mut online: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
    send_timeout: Duration,
) -> archive::Result<()> {
    let mut changed = archive.subscribe();
    let mut retry = tokio::time::interval(Duration::from_secs(30));
    changed.mark_changed();
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => return Ok(()),
            value = online.changed() => { if value.is_err() { return Ok(()); } },
            _ = changed.changed() => {},
            _ = retry.tick() => {},
        }
        while *online.borrow() && !*stop.borrow() {
            let ready = tokio::select! {
                biased;
                _ = stop.changed() => return Ok(()),
                ready = tokio::time::timeout(Duration::from_secs(30), transport.ready()) => ready,
            };
            if !matches!(ready, Ok(true)) || !*online.borrow() || *stop.borrow() {
                break;
            }
            let Some(job) = archive.claim_next().await? else {
                break;
            };
            let (chat, jid, id) = (job.chat, job.jid.clone(), job.id.clone());
            let send = async {
                let attempt = transport.send(job).await;
                if matches!(attempt, Attempt::Submitted) {
                    // A successful socket write isn't an ACK, nor is the short
                    // wait for its event a failure. Keep Sending until the
                    // existing event path records evidence, within the same
                    // overall send deadline. No polling or extra task.
                    loop {
                        changed.borrow_and_update();
                        if !archive.send_pending(chat, id.clone()).await? {
                            break;
                        }
                        changed
                            .changed()
                            .await
                            .map_err(|_| archive::Error::Database)?;
                    }
                }
                Ok::<_, archive::Error>(attempt)
            };
            let result = tokio::select! {
                biased;
                _ = stop.changed() => None,
                result = tokio::time::timeout(send_timeout, send) => Some(result),
            };
            let attempt = match result {
                Some(Ok(result)) => result?,
                _ => Attempt::Unconfirmed,
            };
            let status = match attempt {
                Attempt::Submitted => continue, // Evidence is already durable.
                Attempt::Confirmed => "accepted",
                Attempt::Rejected => "failed",
                Attempt::Unconfirmed => "uncertain",
            };
            // An ACK can race with send completion, cancellation or timeout.
            // Archive::delivery never downgrades that stronger evidence.
            archive.delivery(jid, id, status).await?;
            if *stop.borrow() {
                return Ok(());
            }
        }
    }
}

async fn send_control(
    client: &Client,
    jid: &whatsapp_rust::wacore_binary::Jid,
    event: &str,
    control: crate::message_actions::Control,
) -> Attempt {
    use crate::message_actions::{self as action, ControlKind};
    use whatsapp_rust::{EditOptions, EditRequest, MessageId, MessageRef, StanzaId};
    if matches!(control.kind, ControlKind::Edit | ControlKind::Revoke)
        && !action::within_window(
            control.stamp,
            chrono::Utc::now().timestamp(),
            if control.kind == ControlKind::Edit {
                action::EDIT_WINDOW
            } else {
                action::REVOKE_WINDOW
            },
        )
    {
        return Attempt::Rejected;
    }
    let Ok(target_id) = MessageId::new(&control.wire) else {
        return Attempt::Rejected;
    };
    let sender = (!control.outgoing && jid.to_string().ends_with("@g.us"))
        .then(|| control.sender.parse::<whatsapp_rust::wacore_binary::Jid>())
        .transpose();
    let Ok(sender) = sender else {
        return Attempt::Rejected;
    };
    let Ok(target) = MessageRef::new(jid, target_id, sender.as_ref(), control.outgoing) else {
        return Attempt::Rejected;
    };
    if matches!(control.kind, ControlKind::Star | ControlKind::Unstar) {
        let result = if control.kind == ControlKind::Star {
            client.chat_actions().star_message(&target).await
        } else {
            client.chat_actions().unstar_message(&target).await
        };
        return if result.is_ok() {
            Attempt::Confirmed
        } else {
            Attempt::Unconfirmed
        };
    }
    if matches!(control.kind, ControlKind::Pin | ControlKind::Unpin) {
        use whatsapp_rust::wacore_binary::JidExt as _;
        let mut key = target.to_raw_key();
        if jid.is_group() && control.outgoing && key.participant.is_none() {
            let Ok(routing) = client.groups().routing_info(jid).await else {
                return Attempt::Unconfirmed;
            };
            let author = match routing.addressing_mode {
                whatsapp_rust::wacore::types::message::AddressingMode::Pn => client.pn(),
                whatsapp_rust::wacore::types::message::AddressingMode::Lid => client.lid(),
            };
            let Some(author) = author else {
                return Attempt::Rejected;
            };
            key.participant = Some(author.to_non_ad_string());
        }
        let Ok(seconds) = control.text.parse::<u32>() else {
            return Attempt::Rejected;
        };
        let Some(message) = crate::protocol_actions::pin(key, seconds, control.version) else {
            return Attempt::Rejected;
        };
        let Ok(id) = MessageId::new(event) else {
            return Attempt::Rejected;
        };
        return submitted(
            client
                .send(
                    SendRequest::new(jid, message)
                        .with_options(SendOptions::default().with_message_id(id)),
                )
                .await,
        );
    }
    if control.kind == ControlKind::Hide {
        return if client
            .chat_actions()
            .delete_message_for_me(&target, true, Some(control.stamp))
            .await
            .is_ok()
        {
            Attempt::Confirmed
        } else {
            Attempt::Unconfirmed
        };
    }
    let result = if control.kind == ControlKind::Edit {
        let Ok(id) = StanzaId::new(event) else {
            return Attempt::Rejected;
        };
        client
            .edit_message(
                EditRequest::new(target, wa::Message::text(control.text))
                    .with_options(EditOptions::default().with_stanza_id(id)),
            )
            .await
    } else {
        let message = crate::protocol_actions::revoke(&jid.to_string(), control.wire);
        let Ok(id) = MessageId::new(event) else {
            return Attempt::Rejected;
        };
        client
            .send(
                SendRequest::new(jid, message)
                    .with_options(SendOptions::default().with_message_id(id)),
            )
            .await
    };
    submitted(result)
}

fn submitted(
    result: Result<whatsapp_rust::send::SendResult, whatsapp_rust::send::SendError>,
) -> Attempt {
    use whatsapp_rust::send::SendError;
    match result {
        Ok(_) => Attempt::Submitted,
        Err(
            SendError::InvalidRequest(_)
            | SendError::MessageRef(_)
            | SendError::NotLoggedIn
            | SendError::NoRecipientDevice(_)
            | SendError::PrimaryDeviceRejected(_),
        ) => Attempt::Rejected,
        Err(_) => Attempt::Unconfirmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{PN, fixture};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fake {
        archive: Archive,
        calls: Arc<AtomicUsize>,
        started: watch::Sender<bool>,
        mode: u8,
    }
    #[async_trait::async_trait]
    impl Transport for Fake {
        async fn ready(&self) -> bool {
            true
        }
        async fn send(&self, job: Outgoing) -> Attempt {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.send_replace(true);
            match self.mode {
                0 => {
                    self.archive
                        .delivery(job.jid, job.id, "read")
                        .await
                        .unwrap();
                    Attempt::Unconfirmed
                }
                1 => Attempt::Rejected,
                4 | 5 => Attempt::Submitted,
                _ => std::future::pending().await,
            }
        }
    }
    fn status(db: &Archive) -> String {
        db.blocking(|c| {
            Ok(c.query_row(
                "SELECT delivery FROM messages WHERE wire_id='synthetic-outbox'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap()
    }
    struct Controls {
        archive: Archive,
        calls: Arc<std::sync::Mutex<Vec<crate::message_actions::ControlKind>>>,
    }
    #[async_trait::async_trait]
    impl Transport for Controls {
        async fn ready(&self) -> bool {
            true
        }
        async fn send(&self, job: Outgoing) -> Attempt {
            let control = job.control.expect("queued control must retain its payload");
            assert_eq!(control.wire, "source");
            assert_ne!(job.id, "source");
            self.calls.lock().unwrap().push(control.kind);
            if matches!(
                control.kind,
                crate::message_actions::ControlKind::Hide
                    | crate::message_actions::ControlKind::Star
                    | crate::message_actions::ControlKind::Unstar
            ) {
                Attempt::Confirmed
            } else {
                self.archive
                    .delivery(job.jid, job.id, "accepted")
                    .await
                    .unwrap();
                Attempt::Unconfirmed
            }
        }
    }
    #[tokio::test]
    async fn controls_share_the_serial_outbox_and_stop_without_an_extra_task() {
        use crate::message_actions::ControlKind;
        let (_temp, db) = fixture();
        let chat = db.create_chat(PN.into()).await.unwrap();
        db.queue(chat, "source".into(), "Original".into())
            .await
            .unwrap();
        db.delivery(PN.into(), "source".into(), "accepted")
            .await
            .unwrap();
        let id = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .messages[0]
            .id;
        let (stop, stopped) = watch::channel(false);
        let (_online, online) = watch::channel(true);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let task = tokio::spawn(worker(
            db.clone(),
            Controls {
                archive: db.clone(),
                calls: calls.clone(),
            },
            online,
            stopped,
            Duration::from_secs(1),
        ));
        for (event, starred) in [("star", true), ("unstar", false)] {
            db.queue_star(chat, id, event.into(), starred)
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let done = db
                        .blocking(|c| {
                            Ok(c.query_row(
                                "SELECT delivery='accepted' FROM messages WHERE wire_id=?1",
                                [event],
                                |r| r.get::<_, bool>(0),
                            )?)
                        })
                        .unwrap();
                    if done {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                db.view(chat, 100, String::new(), false, false)
                    .await
                    .unwrap()
                    .conversation
                    .unwrap()
                    .messages[0]
                    .content
                    .metadata
                    .starred,
                starred
            );
        }
        for (event, seconds) in [("pin", 604800), ("unpin", 0)] {
            db.queue_pin(chat, id, event.into(), seconds).await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if db
                        .view(chat, 100, String::new(), false, false)
                        .await
                        .unwrap()
                        .conversation
                        .unwrap()
                        .messages[0]
                        .content
                        .metadata
                        .pinned
                        == (seconds > 0)
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        db.queue_edit(chat, id, "edit".into(), "Original".into(), "Edited".into())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if db
                    .view(chat, 100, String::new(), false, false)
                    .await
                    .unwrap()
                    .conversation
                    .unwrap()
                    .messages[0]
                    .text
                    == "Edited"
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        db.queue_revoke(chat, id, "revoke".into()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if db
                    .view(chat, 100, String::new(), false, false)
                    .await
                    .unwrap()
                    .conversation
                    .unwrap()
                    .messages[0]
                    .content
                    .unavailable
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        db.queue_hides(chat, vec![id], vec!["hide".into()])
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let accepted = db
                    .blocking(|c| {
                        Ok(c.query_row(
                            "SELECT delivery FROM messages WHERE wire_id='hide'",
                            [],
                            |r| r.get::<_, String>(0),
                        )?)
                    })
                    .unwrap();
                if accepted == "accepted" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            [
                ControlKind::Star,
                ControlKind::Unstar,
                ControlKind::Pin,
                ControlKind::Unpin,
                ControlKind::Edit,
                ControlKind::Revoke,
                ControlKind::Hide
            ]
        );
        assert!(db.claim_next().await.unwrap().is_none());
    }
    #[tokio::test]
    async fn real_worker_handles_offline_ack_rejection_timeout_and_cancellation() {
        for mode in 0..6 {
            let (_temp, db) = fixture();
            let chat = db.create_chat(PN.into()).await.unwrap();
            db.queue(
                chat,
                "synthetic-outbox".into(),
                "Synthetic text only".into(),
            )
            .await
            .unwrap();
            let (online, rx) = watch::channel(false);
            let (stop, stopped) = watch::channel(false);
            let (started, mut beginning) = watch::channel(false);
            let calls = Arc::new(AtomicUsize::new(0));
            let task = tokio::spawn(worker(
                db.clone(),
                Fake {
                    archive: db.clone(),
                    calls: calls.clone(),
                    started,
                    mode,
                },
                rx,
                stopped,
                Duration::from_millis(40),
            ));
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert_eq!(status(&db), "queued");
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            online.send_replace(true);
            tokio::time::timeout(Duration::from_secs(3), beginning.changed())
                .await
                .unwrap()
                .unwrap();
            if matches!(mode, 3 | 5) {
                stop.send_replace(true);
            }
            let expected = match mode {
                0 => "read",
                1 => "failed",
                _ => "uncertain",
            };
            tokio::time::timeout(Duration::from_secs(3), async {
                while status(&db) != expected {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            if !matches!(mode, 3 | 5) {
                stop.send_replace(true);
            }
            tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert!(
                db.claim_next().await.unwrap().is_none(),
                "attempted sends never auto-requeue"
            );
        }
    }

    #[tokio::test]
    async fn delayed_ack_keeps_sending_without_delaying_local_saves_or_views() {
        use crate::messaging::{self, Command, Snapshot};
        let (_temp, db) = fixture();
        let chat = db.create_chat(PN.into()).await.unwrap();
        let (archives, receiver) = watch::channel(None);
        let (commands, rx) = tokio::sync::mpsc::channel(16);
        let (updates, mut views) = watch::channel(Snapshot::empty());
        let actor = tokio::spawn(messaging::run(receiver, rx, updates));
        archives.send_replace(Some(db.clone()));
        views.changed().await.unwrap();
        commands
            .send(Command::Select {
                id: chat,
                previous: 0,
                draft: String::new(),
                draft_version: 0,
                revision: 1,
            })
            .await
            .unwrap();

        let (_online, online) = watch::channel(true);
        let (stop, stopped) = watch::channel(false);
        let (started, _beginning) = watch::channel(false);
        let calls = Arc::new(AtomicUsize::new(0));
        let network = tokio::spawn(worker(
            db.clone(),
            Fake {
                archive: db.clone(),
                calls: calls.clone(),
                started,
                mode: 4,
            },
            online,
            stopped,
            Duration::from_secs(3),
        ));

        for request in 1..=2 {
            commands
                .send(Command::Send {
                    chat,
                    text: format!("Synthetic message {request} 👩🏽‍💻\nSecond line"),
                    request,
                    draft_version: request,
                })
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    {
                        let snapshot = views.borrow_and_update();
                        if snapshot
                            .reply
                            .as_ref()
                            .is_some_and(|r| r.request == request && r.saved)
                            && snapshot
                                .view
                                .as_ref()
                                .and_then(|v| v.conversation.as_ref())
                                .is_some_and(|c| c.messages.len() == request as usize)
                        {
                            break;
                        }
                    }
                    views.changed().await.unwrap();
                }
            })
            .await
            .expect("local save and bubble must not wait for a network ACK");
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let rows = db
            .call(|c| {
                Ok(
                    c.prepare("SELECT wire_id,delivery FROM messages ORDER BY id")?
                        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                )
            })
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "network delivery remains serial"
        );
        assert_eq!(
            rows[0].1, "sending",
            "a pending ACK must not flash a warning"
        );
        assert_eq!(
            rows[1].1, "queued",
            "the second bubble is already durable and visible"
        );
        db.delivery(PN.into(), rows[0].0.clone(), "delivered")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while calls.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // A negative ACK must not be overwritten as a successful submission.
        db.delivery(PN.into(), rows[1].0.clone(), "failed")
            .await
            .unwrap();
        stop.send_replace(true);
        network.await.unwrap().unwrap();
        assert!(!db.send_pending(chat, rows[0].0.clone()).await.unwrap());
        assert!(!db.send_pending(chat, rows[1].0.clone()).await.unwrap());
        let statuses = db
            .call(|c| {
                Ok(c.prepare("SELECT delivery FROM messages ORDER BY id")?
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            })
            .await
            .unwrap();
        assert_eq!(statuses, ["delivered", "failed"]);
        drop(commands);
        actor.await.unwrap();
    }
}
