//! Transfer-local workers. No upload executor or blocking pool stays parked idle.
use crate::{attachments::Error, downloads::Control};
use std::{future::Future, time::Duration};
use tokio::sync::oneshot;

struct CancelOnDrop(Option<Control>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(control) = &self.0 {
            control.cancel();
        }
    }
}

/// Preparation is one finite CPU/file job, independent of Tokio's shared pool.
pub async fn blocking<T: Send + 'static>(
    control: Control,
    job: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    let (tx, rx) = oneshot::channel();
    let worker = std::thread::Builder::new()
        .name("heya-prepare".into())
        .spawn(move || {
            let result = job();
            let _ = tx.send(result);
        })
        .map_err(|_| Error::File)?;
    let mut cancel = CancelOnDrop(Some(control));
    let result = rx.await.map_err(|_| Error::File);
    let joined = worker.join();
    cancel.0 = None; // Finished normally, including errors; only abandonment cancels.
    joined.map_err(|_| Error::File)?;
    result?
}

/// The SDK's streaming HTTP API is synchronous. Its blocking tasks, including
/// resume checks, belong to a transfer-local executor instead of the app pool.
/// Drop the executor (and join its workers) BEFORE acknowledging completion.
/// Cancellation drops the SDK future immediately, releasing its Client/session;
/// pending OS calls unwind before this worker reports completion. App shutdown
/// still has the application's outer deadline if a network/OS call gets stuck.
pub async fn upload<T: Send + 'static>(
    control: Control,
    job: impl Future<Output = Result<T, Error>> + Send + 'static,
) -> Result<T, Error> {
    let cancellation = control.clone();
    let (tx, rx) = oneshot::channel();
    let worker = std::thread::Builder::new()
        .name("heya-upload".into())
        .spawn(move || {
            let result = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .max_blocking_threads(1)
                .thread_name("heya-upload-io")
                .thread_keep_alive(Duration::ZERO)
                .build()
            {
                Ok(runtime) => {
                    let result = runtime.block_on(async {
                        tokio::select! {
                            biased;
                            _ = control.cancelled() => Err(Error::Cancelled),
                            result = job => result,
                        }
                    });
                    drop(runtime);
                    result
                }
                Err(_) => Err(Error::Upload),
            };
            let _ = tx.send(result);
        })
        .map_err(|_| Error::Upload)?;
    let mut cancel = CancelOnDrop(Some(cancellation));
    let result = rx.await.map_err(|_| Error::Upload);
    let joined = worker.join();
    cancel.0 = None;
    joined.map_err(|_| Error::Upload)?;
    result?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn current_task() -> Option<std::path::PathBuf> {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_link("/proc/thread-self")
                .ok()
                .map(|p| std::path::Path::new("/proc").join(p))
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }
    #[tokio::test]
    async fn returning_a_worker_result_does_not_cancel_its_caller() {
        for fail in [false, true] {
            let control = Control::default();
            let result = upload(control.clone(), async move {
                tokio::task::spawn_blocking(|| ()).await.unwrap();
                if fail { Err(Error::Upload) } else { Ok(42) }
            })
            .await;
            assert_eq!(result, if fail { Err(Error::Upload) } else { Ok(42) });
            assert!(
                !control.is_cancelled(),
                "normal completion must not signal user cancellation"
            );
            let control = Control::default();
            let result = blocking(control.clone(), move || {
                if fail { Err(Error::File) } else { Ok(42) }
            })
            .await;
            assert_eq!(result, if fail { Err(Error::File) } else { Ok(42) });
            assert!(
                !control.is_cancelled(),
                "preparation completion must not cancel its caller"
            );
        }
    }
    #[tokio::test]
    async fn abandoning_an_active_upload_cancels_and_releases_the_worker() {
        let control = Control::default();
        let owner = Arc::new(());
        let weak = Arc::downgrade(&owner);
        let (started, ready) = oneshot::channel();
        let job = tokio::spawn(upload(control.clone(), async move {
            let _owner = owner;
            started.send(current_task()).unwrap();
            std::future::pending::<Result<(), Error>>().await
        }));
        let task = ready.await.unwrap();
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
        assert!(control.is_cancelled());
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak.upgrade().is_some() || task.as_ref().is_some_and(|p| p.exists()) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn completion_waits_for_sdk_blocking_work_and_close_cancels_client_ownership() {
        for cancel in [false, true] {
            let control = Control::default();
            let owner = Arc::new(());
            let weak = Arc::downgrade(&owner);
            let (started, ready) = oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let (body_done, mut body_gone) = oneshot::channel();
            let c = control.clone();
            let mut worker = tokio::spawn(upload(control, async move {
                let _owner = owner;
                let coordinator = current_task();
                let io = tokio::task::spawn_blocking(move || {
                    started.send((coordinator, current_task())).unwrap();
                    wait.recv().unwrap();
                    body_done.send(()).unwrap();
                });
                io.await.unwrap();
                Ok(())
            }));
            let tasks = ready.await.unwrap();
            if cancel {
                c.cancel();
                tokio::time::timeout(Duration::from_secs(2), async {
                    while weak.upgrade().is_some() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(20), &mut worker)
                    .await
                    .is_err()
            );
            assert!(body_gone.try_recv().is_err());
            release.send(()).unwrap();
            let result = tokio::time::timeout(Duration::from_secs(2), worker)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.err(), cancel.then_some(Error::Cancelled));
            body_gone.await.unwrap();
            assert!(weak.upgrade().is_none());
            // Observe retirement of these exact OS tasks. /proc can briefly
            // expose an exiting task after its join has returned under load.
            let paths: Vec<_> = [tasks.0, tasks.1].into_iter().flatten().collect();
            tokio::time::timeout(Duration::from_secs(2), async {
                while paths.iter().any(|p| p.exists()) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("this upload's thread survived completion");
            assert_eq!(
                c.is_cancelled(),
                cancel,
                "cleanup must preserve the cancellation reason"
            );
        }
    }
}
