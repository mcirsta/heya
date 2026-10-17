//! Finite local settings/filesystem work without a parked blocking pool.
pub async fn run<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Result<T, ()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let worker = std::thread::Builder::new()
        .name("heya-settings".into())
        .spawn(move || {
            let _ = tx.send(job());
        })
        .map_err(|_| ())?;
    let result = rx.await.map_err(|_| ());
    worker.join().map_err(|_| ())?;
    result
}
