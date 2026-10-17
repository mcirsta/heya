//! On-demand phone history. A request owns its future only while it has work.
use crate::archive::Archive;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::watch;

pub const BATCH: i32 = 50;
pub const PATIENCE: Duration = Duration::from_secs(45);
pub const COOLDOWN: Duration = Duration::from_secs(30);
#[derive(Clone)]
pub struct Anchor {
    pub chat: i32,
    pub jid: String,
    pub wire: String,
    pub outgoing: bool,
    /// Unix seconds, despite the SDK parameter's historical `timestamp_ms` name.
    pub stamp: i64,
    /// Visible message count through the anchor second, for same-second replies.
    pub known: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Offline,
    Timeout,
    Failed,
}
impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::Offline => "Connect to fetch older messages from your phone.",
            Self::Timeout => {
                "Your phone did not send older messages. Check that it is online, then try again."
            }
            Self::Failed => {
                "Older messages could not be fetched from your phone. Please try again."
            }
        }
    }
}
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    async fn fetch(&self, anchor: &Anchor) -> Result<String, Failure>;
}
pub type Sources = Option<Arc<dyn Source>>;
#[derive(Clone, Default)]
pub struct Status {
    pub available: bool,
    pub loading: bool,
}
pub struct Outcome {
    pub chat: i32,
    pub request: i32,
    pub revision: i32,
    pub result: Result<bool, Failure>, // true only when the oldest position advanced
}
pub type Pending = Pin<Box<dyn Future<Output = Outcome> + Send>>;

pub fn request(
    db: Archive,
    sources: watch::Receiver<Sources>,
    anchor: Anchor,
    request: i32,
    revision: i32,
) -> Pending {
    request_with_patience(db, sources, anchor, request, revision, PATIENCE)
}
fn request_with_patience(
    db: Archive,
    sources: watch::Receiver<Sources>,
    anchor: Anchor,
    request: i32,
    revision: i32,
    patience: Duration,
) -> Pending {
    Box::pin(async move {
        let chat = anchor.chat;
        let mut changed = db.subscribe();
        let work = async {
            let source = sources.borrow().clone().ok_or(Failure::Offline)?;
            let session = source.fetch(&anchor).await?;
            drop(source); // Never retain the protocol client while awaiting import.
            loop {
                if db
                    .phone_history_answer(session.clone(), chat)
                    .await
                    .map_err(|_| Failure::Failed)?
                {
                    return db
                        .phone_history_advanced(anchor.clone())
                        .await
                        .map_err(|_| Failure::Failed);
                }
                changed.changed().await.map_err(|_| Failure::Failed)?;
            }
        };
        // Source replacement includes disconnect/reconnect. Dropping the losing
        // future releases Client ownership before the protocol's shutdown fence.
        let mut connection = sources.clone();
        let result = tokio::select! {
            biased;
            _ = connection.changed() => Err(Failure::Offline),
            result = tokio::time::timeout(patience, work) => result.unwrap_or(Err(Failure::Timeout)),
        };
        Outcome {
            chat,
            request,
            revision,
            result,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{fixture, record};
    use tokio::sync::mpsc;
    struct Fake(mpsc::UnboundedSender<Anchor>);
    #[async_trait::async_trait]
    impl Source for Fake {
        async fn fetch(&self, anchor: &Anchor) -> Result<String, Failure> {
            self.0.send(anchor.clone()).unwrap();
            Ok("synthetic-session".into())
        }
    }
    #[tokio::test]
    async fn phone_request_waits_for_matching_committed_chat_and_releases_sources_on_disconnect() {
        let (_temp, db) = fixture();
        db.ingest(crate::archive::Batch {
            messages: vec![record("anchor")],
            ..Default::default()
        })
        .await
        .unwrap();
        let anchor = db.phone_history_anchor(1).await.unwrap().unwrap();
        let (sent, mut requests) = mpsc::unbounded_channel();
        let (sources, rx) = watch::channel(Some(Arc::new(Fake(sent)) as Arc<dyn Source>));
        let job = tokio::spawn(request(db.clone(), rx, anchor, 7, 3));
        let sent = requests.recv().await.unwrap();
        assert_eq!(sent.stamp, 1_700_000_000, "wire timestamp uses seconds");
        db.capture_history("chunk".into(), vec![1]).await.unwrap();
        db.phone_history_metadata("chunk".into(), "unrelated-session".into())
            .await
            .unwrap();
        db.finish_history("chunk", 1, vec![(sent.jid.clone(), Some(true))])
            .unwrap();
        tokio::task::yield_now().await;
        assert!(!job.is_finished());
        sources.send_replace(None);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), job)
                .await
                .unwrap()
                .unwrap()
                .result,
            Err(Failure::Offline)
        );
    }
    #[tokio::test]
    async fn silent_phone_times_out_without_claiming_history_is_exhausted() {
        let (_temp, db) = fixture();
        db.ingest(crate::archive::Batch {
            messages: vec![record("anchor")],
            ..Default::default()
        })
        .await
        .unwrap();
        let anchor = db.phone_history_anchor(1).await.unwrap().unwrap();
        let (sent, _requests) = mpsc::unbounded_channel();
        let (_sources, rx) = watch::channel(Some(Arc::new(Fake(sent)) as Arc<dyn Source>));
        let result =
            request_with_patience(db.clone(), rx, anchor, 1, 1, Duration::from_millis(25)).await;
        assert_eq!(result.result, Err(Failure::Timeout));
        assert!(
            db.view(1, 100, String::new(), false, false)
                .await
                .unwrap()
                .phone_more
        );
    }
}
