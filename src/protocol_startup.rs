//! Startup waits for a late OS key store using the existing lifecycle task.
use crate::connection::{Failure, Phase, Request, Snapshot};
use std::{future::Future, time::Duration};
use tokio::sync::watch;

fn opening(request: Request) -> bool {
    matches!(
        request,
        Request::Startup | Request::OpenOffline | Request::Connect | Request::Relink
    )
}

// Each blocking attempt finishes before retry/cancellation releases its files.
// Between attempts only this cancellable async sleep remains: no parked worker.
pub(super) async fn prepare<T, F: Future<Output = Result<T, Failure>>>(
    requests: &mut watch::Receiver<Request>,
    updates: &watch::Sender<Snapshot>,
    mut open: impl FnMut() -> F,
) -> Result<Option<T>, Failure> {
    let mut delay = Duration::from_secs(2);
    loop {
        if requests.has_changed().is_err() || !opening(*requests.borrow()) {
            updates.send_replace(Snapshot::new(Phase::Stopped));
            return Ok(None);
        }
        let result = open().await;
        if requests.has_changed().is_err() || !opening(*requests.borrow()) {
            updates.send_replace(Snapshot::new(Phase::Stopped));
            return Ok(None);
        }
        if !matches!(&result, Err(Failure::KeyringUnavailable)) {
            return result.map(Some);
        }
        updates.send_replace(Snapshot::new(Phase::WaitingForKeyStore));
        tokio::select! {
            biased;
            _ = requests.wait_for(|request| !opening(*request)) => {
                updates.send_replace(Snapshot::new(Phase::Stopped));
                return Ok(None);
            }
            _ = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn late_key_store_retries_without_hammering_then_finishes() {
        let (_commands, mut requests) = watch::channel(Request::Startup);
        let (updates, mut snapshots) = watch::channel(Snapshot::new(Phase::Opening));
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let task = tokio::spawn(async move {
            prepare(&mut requests, &updates, || {
                let attempt = count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if attempt < 2 {
                        Err(Failure::KeyringUnavailable)
                    } else {
                        Ok(42)
                    }
                }
            })
            .await
        });
        snapshots
            .wait_for(|s| s.phase == Phase::WaitingForKeyStore)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        tokio::time::timeout(Duration::from_secs(4), snapshots.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "second delay backs off to four seconds"
        );
        assert_eq!(task.await.unwrap(), Ok(Some(42)));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            3,
            "success leaves no retry task"
        );
    }

    #[tokio::test]
    async fn disconnect_or_window_close_cancels_the_wait_immediately() {
        for close in [false, true] {
            let (commands, mut requests) = watch::channel(Request::Connect);
            let (updates, mut snapshots) = watch::channel(Snapshot::new(Phase::Opening));
            let attempts = Arc::new(AtomicUsize::new(0));
            let count = attempts.clone();
            let task = tokio::spawn(async move {
                prepare(&mut requests, &updates, || {
                    count.fetch_add(1, Ordering::SeqCst);
                    std::future::ready(Err::<(), _>(Failure::KeyringUnavailable))
                })
                .await
            });
            snapshots
                .wait_for(|s| s.phase == Phase::WaitingForKeyStore)
                .await
                .unwrap();
            if !close {
                commands.send_replace(Request::Disconnect);
            }
            drop(commands);
            assert_eq!(
                tokio::time::timeout(Duration::from_millis(500), task)
                    .await
                    .unwrap()
                    .unwrap(),
                Ok(None)
            );
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
            assert_eq!(snapshots.borrow().phase, Phase::Stopped);
        }
    }

    #[tokio::test]
    async fn refused_or_invalid_keys_fail_once_and_plaintext_does_not_wait() {
        for failure in [
            Failure::Keyring,
            Failure::MissingKey,
            Failure::InvalidKey,
            Failure::Archive,
            Failure::AlreadyRunning,
        ] {
            let (_commands, mut requests) = watch::channel(Request::Startup);
            let (updates, _) = watch::channel(Snapshot::new(Phase::Opening));
            let mut attempts = 0;
            let result = prepare(&mut requests, &updates, || {
                attempts += 1;
                std::future::ready(Err::<(), _>(failure))
            })
            .await;
            assert_eq!(result, Err(failure));
            assert_eq!(attempts, 1);
        }
        let (_commands, mut requests) = watch::channel(Request::OpenOffline);
        let (updates, snapshots) = watch::channel(Snapshot::new(Phase::Opening));
        assert_eq!(
            prepare(&mut requests, &updates, || std::future::ready(Ok(1))).await,
            Ok(Some(1))
        );
        assert_eq!(snapshots.borrow().phase, Phase::Opening);
    }
}
