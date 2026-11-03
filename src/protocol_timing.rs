//! Content-free startup timing for one connection. The SDK opens the socket
//! while it checks the app version, logs in, and announces Connected only after
//! the server drains the offline backlog. Every delivered batch is saved before
//! acknowledgement, so "Connecting" covers all of these.
use std::sync::{Arc, Mutex, PoisonError, Weak};
use tokio::time::{Duration, Instant};
use whatsapp_rust::{
    Client, NodeFilter,
    async_channel::Receiver,
    transport::{Transport, TransportEvent, TransportFactory},
    types::events::OfflineSyncPreview,
};

// The SDK refetches the app version when its stamp is older than this.
const VERSION_CACHE_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Default)]
pub(crate) struct Startup(Mutex<State>);

#[derive(Default)]
struct State {
    started: Option<(&'static str, Instant)>,
    // Wall-clock milliseconds at the start, compared with the SDK's version stamp.
    since_ms: i64,
    dropped: u32,
    dials: u32,
    failed_dials: u32,
    // When the last socket opened, and how long its dial took.
    socket: Option<(Duration, Duration)>,
    logged_in: Option<Duration>,
    // Server-announced counts: messages, notifications, receipts, app-state, calls, statuses.
    backlog: Option<(Duration, i32, [i32; 6])>,
    delivered: Option<(Duration, i32)>,
    saving: Option<(Duration, u32)>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

impl Startup {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts timing an attempt; `label` is "connected" or "reconnected". A
    /// connection that drops before Connected keeps the original start.
    pub(crate) fn begin(&self, label: &'static str) {
        let mut state = self.state();
        if state.started.is_some() {
            state.dropped += 1;
            return;
        }
        *state = State {
            started: Some((label, Instant::now())),
            since_ms: now_ms(),
            ..State::default()
        };
    }

    fn dialed(&self, dial: Instant, open: bool) {
        let mut state = self.state();
        if let Some((_, started)) = state.started {
            state.dials += 1;
            if open {
                state.socket = Some((started.elapsed(), dial.elapsed()));
            } else {
                state.failed_dials += 1;
            }
        }
    }

    fn logged_in(&self) {
        let mut state = self.state();
        if let Some((_, started)) = state.started {
            state.logged_in = Some(started.elapsed());
        }
    }

    pub(crate) fn backlog(&self, preview: &OfflineSyncPreview) {
        let mut state = self.state();
        if let Some((_, started)) = state.started {
            state.backlog = Some((
                started.elapsed(),
                preview.total,
                [
                    preview.messages,
                    preview.notifications,
                    preview.receipts,
                    preview.app_data_changes,
                    preview.calls,
                    preview.statuses,
                ],
            ));
        }
    }

    pub(crate) fn delivered(&self, count: i32) {
        let mut state = self.state();
        if let Some((_, started)) = state.started {
            state.delivered = Some((started.elapsed(), count));
        }
    }

    /// Time spent in the durability hooks; live traffic after Connected is not counted.
    pub(crate) fn saved(&self, took: Duration) {
        let mut state = self.state();
        if state.started.is_some() {
            let (total, batches) = state.saving.get_or_insert_default();
            *total += took;
            *batches += 1;
        }
    }

    /// One line per attempt, taken when the session is announced.
    /// `version_fetched_ms` is the device's app-version stamp at that point.
    pub(crate) fn summary(&self, version_fetched_ms: i64) -> Option<String> {
        let state = std::mem::take(&mut *self.state());
        let (label, started) = state.started?;
        let mut line = format!(
            "Heya connection: {label} after {} ms",
            started.elapsed().as_millis()
        );
        if state.dropped > 0 {
            line += &format!("; connections dropped before Connected: {}", state.dropped);
        }
        let (dials, failed) = (state.dials, state.failed_dials);
        match state.socket {
            Some((at, took)) => {
                line += &format!(
                    "; socket open after {} ms on dial {dials} ({failed} failed, last took {} ms)",
                    at.as_millis(),
                    took.as_millis()
                );
            }
            None => line += &format!("; no socket opened ({dials} dials, {failed} failed)"),
        }
        // The version check runs alongside the dial; login waits for both.
        line += if version_fetched_ms >= state.since_ms {
            ", app version fetched"
        } else if now_ms() - version_fetched_ms > VERSION_CACHE_MS {
            ", app version fetch failed"
        } else {
            ", cached app version"
        };
        match state.logged_in {
            Some(at) => line += &format!(", logged in after {} ms", at.as_millis()),
            None => line += ", no login recorded",
        }
        match state.backlog {
            Some((at, total, [messages, notifications, receipts, app, calls, statuses])) => {
                line += &format!(
                    "; offline backlog of {total} ({messages} messages, {notifications} notifications, \
                     {receipts} receipts, {app} app-state changes, {calls} calls, {statuses} statuses) \
                     announced after {} ms",
                    at.as_millis()
                );
            }
            None => line += "; no offline backlog announced",
        }
        match state.delivered {
            Some((at, count)) => {
                line += &format!(", {count} delivered after {} ms", at.as_millis());
            }
            None => line += ", no delivery completion reported",
        }
        let (saving, batches) = state.saving.unwrap_or_default();
        line += &format!(
            "; saving {batches} batches before acknowledgement took {} ms",
            saving.as_millis()
        );
        Some(line)
    }
}

/// Times each socket dial, including one the SDK abandons at its connect timeout.
pub(crate) struct TimedDials<F>(pub(crate) F, pub(crate) Arc<Startup>);

struct Dial<'a>(&'a Startup, Instant, bool);

impl Drop for Dial<'_> {
    fn drop(&mut self) {
        self.0.dialed(self.1, self.2);
    }
}

#[async_trait::async_trait]
impl<F: TransportFactory> TransportFactory for TimedDials<F> {
    async fn create_transport(
        &self,
    ) -> Result<(Arc<dyn Transport>, Receiver<TransportEvent>), anyhow::Error> {
        let mut dial = Dial(&self.1, Instant::now(), false);
        let result = self.0.create_transport().await;
        dial.2 = result.is_ok();
        result
    }
}

/// Records each `<success>` login. The first waiter is registered before the
/// bot runs; the task ends when the client is dropped.
pub(crate) fn logins(
    client: &Arc<Client>,
    startup: Arc<Startup>,
) -> impl Future<Output = ()> + Send + 'static {
    let mut waiter = client.wait_for_node(NodeFilter::tag("success"));
    let client: Weak<Client> = Arc::downgrade(client);
    async move {
        loop {
            if waiter.await.is_err() {
                return;
            }
            startup.logged_in();
            let Some(client) = client.upgrade() else {
                return;
            };
            waiter = client.wait_for_node(NodeFilter::tag("success"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Socket;

    #[async_trait::async_trait]
    impl Transport for Socket {
        async fn send(&self, _: whatsapp_rust::bytes::Bytes) -> Result<(), anyhow::Error> {
            Ok(())
        }
        async fn disconnect(&self) {}
    }

    // Fails the first dial, hangs on the second and opens on the third.
    struct Dialer(std::sync::atomic::AtomicU32);

    #[async_trait::async_trait]
    impl TransportFactory for Dialer {
        async fn create_transport(
            &self,
        ) -> Result<(Arc<dyn Transport>, Receiver<TransportEvent>), anyhow::Error> {
            match self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => Err(anyhow::anyhow!("refused")),
                1 => std::future::pending().await,
                _ => {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    let (_, events) = whatsapp_rust::async_channel::bounded(1);
                    Ok((Arc::new(Socket) as Arc<dyn Transport>, events))
                }
            }
        }
    }

    #[tokio::test]
    async fn the_summary_separates_socket_login_backlog_delivery_and_saving_time() {
        let startup = Arc::new(Startup::default());
        let dials = TimedDials(Dialer(Default::default()), startup.clone());
        // Nothing is counted before an attempt starts or after it is reported.
        startup.saved(Duration::from_secs(5));
        assert!(dials.create_transport().await.is_err());
        assert!(startup.summary(0).is_none());
        startup.begin("reconnected");
        let wait = Duration::from_millis(50);
        assert!(
            tokio::time::timeout(wait, dials.create_transport())
                .await
                .is_err()
        );
        // A drop before Connected keeps the first start and its dials.
        startup.begin("reconnected");
        assert!(dials.create_transport().await.is_ok());
        startup.logged_in();
        startup.backlog(
            &OfflineSyncPreview::builder()
                .total(812)
                .messages(640)
                .notifications(40)
                .receipts(120)
                .app_data_changes(12)
                .calls(0)
                .statuses(0)
                .build(),
        );
        startup.saved(Duration::from_millis(1500));
        startup.saved(Duration::from_millis(250));
        startup.delivered(812);
        let line = startup.summary(now_ms() - 60_000).unwrap();
        assert!(
            line.starts_with("Heya connection: reconnected after "),
            "{line}"
        );
        assert!(
            line.contains(" ms; connections dropped before Connected: 1; socket open after "),
            "{line}"
        );
        let took = line
            .split(" ms on dial 2 (1 failed, last took ")
            .nth(1)
            .and_then(|rest| rest.split(" ms)").next())
            .and_then(|took| took.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("{line}"));
        assert!((30..1000).contains(&took), "{line}");
        assert!(
            line.contains(", cached app version, logged in after "),
            "{line}"
        );
        assert!(line.contains(
            "offline backlog of 812 (640 messages, 40 notifications, 120 receipts, \
             12 app-state changes, 0 calls, 0 statuses) announced after "
        ));
        assert!(line.contains(", 812 delivered after "), "{line}");
        assert!(line.ends_with("saving 2 batches before acknowledgement took 1750 ms"));
        assert!(startup.summary(0).is_none(), "one line per attempt");
        startup.saved(Duration::from_secs(1));
        startup.begin("connected");
        let line = startup.summary(now_ms() + 1).unwrap();
        assert!(
            line.contains(
                "; no socket opened (0 dials, 0 failed), app version fetched, no login recorded; \
                 no offline backlog announced, no delivery completion reported"
            ),
            "{line}"
        );
        assert!(line.ends_with("saving 0 batches before acknowledgement took 0 ms"));
        startup.begin("connected");
        let stale = now_ms() - VERSION_CACHE_MS - 60_000;
        assert!(
            startup
                .summary(stale)
                .unwrap()
                .contains(", app version fetch failed,")
        );
    }
}
