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
        use whatsapp_rust::send::SendError;
        match self.0.send(request).await {
            Err(
                SendError::InvalidRequest(_)
                | SendError::MessageRef(_)
                | SendError::NotLoggedIn
                | SendError::NoRecipientDevice(_)
                | SendError::PrimaryDeviceRejected(_),
            ) => Attempt::Rejected,
            // Only the ACK/receipt event path establishes successful delivery.
            _ => Attempt::Unconfirmed,
        }
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
            let (jid, id) = (job.jid.clone(), job.id.clone());
            let result = tokio::select! {
                biased;
                _ = stop.changed() => None,
                result = tokio::time::timeout(send_timeout, transport.send(job)) => Some(result),
            };
            let status = if matches!(result, Some(Ok(Attempt::Confirmed))) {
                "accepted"
            } else if matches!(result, Some(Ok(Attempt::Rejected))) {
                "failed"
            } else {
                "uncertain"
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
    use whatsapp_rust::{
        EditOptions, EditRequest, MessageId, MessageRef, StanzaId, send::SendError,
    };
    if control.kind != ControlKind::Hide
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
    match result {
        Err(
            SendError::InvalidRequest(_)
            | SendError::MessageRef(_)
            | SendError::NotLoggedIn
            | SendError::NoRecipientDevice(_)
            | SendError::PrimaryDeviceRejected(_),
        ) => Attempt::Rejected,
        _ => Attempt::Unconfirmed,
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
            if control.kind == crate::message_actions::ControlKind::Hide {
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
            [ControlKind::Edit, ControlKind::Revoke, ControlKind::Hide]
        );
        assert!(db.claim_next().await.unwrap().is_none());
    }
    #[tokio::test]
    async fn real_worker_handles_offline_ack_rejection_timeout_and_cancellation() {
        for mode in 0..4 {
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
            if mode == 3 {
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
            if mode != 3 {
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
}
