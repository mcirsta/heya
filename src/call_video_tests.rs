use super::*;
#[test]
#[ignore = "finite camera/codec helper, invoked by synthetic parent tests"]
fn child() {
    if std::env::var("HEYA_TEST_CALL_VIDEO_CHILD").as_deref() == Ok("1") {
        child_main();
    }
}
async fn roundtrip() {
    let (updates, mut frames) = watch::channel(Frames::default());
    let (mut worker, ports) = Worker::start(Input::Synthetic, updates).unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(8), worker.ready()).await;
    assert!(
        ready.is_ok(),
        "camera readiness timed out (local frame: {}, remote frame: {})",
        frames.borrow().local.is_some(),
        frames.borrow().remote.is_some()
    );
    ready.unwrap().unwrap();
    let packet = tokio::time::timeout(Duration::from_secs(2), ports.capture.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(!packet.data.is_empty());
    let first_timestamp = packet.timestamp;
    ports.playout.send(packet.clone()).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        frames.wait_for(|f| f.local.is_some() && f.remote.is_some()),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), worker.camera(false))
        .await
        .unwrap()
        .unwrap();
    assert!(!frames.borrow().camera_on);
    assert!(frames.borrow().local.is_none());
    let before = frames.borrow().remote_number;
    ports.playout.send(packet).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        frames.wait_for(|f| f.remote_number != before),
    )
    .await
    .unwrap()
    .unwrap();
    // Camera-off calls still receive video and answer the finite-helper watchdog.
    tokio::time::sleep(Duration::from_millis(5200)).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    while ports.capture.try_recv().is_ok() {}
    assert!(ports.capture.try_recv().is_err());
    // The remote decoder remains alive while the physical camera is stopped.
    tokio::time::timeout(Duration::from_secs(5), worker.camera(true))
        .await
        .unwrap()
        .unwrap();
    let packet = tokio::time::timeout(Duration::from_secs(2), ports.capture.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(!packet.data.is_empty());
    assert!(
        packet.timestamp.wrapping_sub(first_timestamp) > 90_000,
        "camera resume must preserve the capture gap instead of resetting RTP time"
    );
    worker.stop().await;
    assert!(
        !frames.borrow().camera_on
            && frames.borrow().local.is_none()
            && frames.borrow().remote.is_none()
    );
    worker.stop().await;
}
#[tokio::test]
async fn synthetic_camera_frames_pause_resume_and_stop_with_the_call() {
    roundtrip().await;
}
#[tokio::test]
async fn absent_or_stalled_camera_is_cancellable_before_signaling() {
    for input in [Input::Unavailable, Input::Stalled] {
        let (frames, _) = watch::channel(Frames::default());
        let (mut worker, _ports) = Worker::start(input, frames).unwrap();
        let wait = if matches!(input, Input::Unavailable) {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(150)
        };
        let result = tokio::time::timeout(wait, worker.ready()).await;
        if matches!(input, Input::Unavailable) {
            assert_eq!(result.unwrap(), Err(Error::NoCamera));
        } else {
            assert!(result.is_err());
        }
        tokio::time::timeout(Duration::from_secs(3), worker.stop())
            .await
            .unwrap();
    }
}
#[tokio::test]
#[ignore = "real codec loopback with a synthetic camera; no account or physical device"]
async fn native_camera_codec_loopback() {
    if std::env::var_os("HEYA_TEST_CALL_VIDEO_NATIVE").is_none() {
        return;
    }
    roundtrip().await;
}
