use super::*;

#[test]
#[ignore = "finite child entry point, invoked only by the parent regression"]
fn child() {
    if std::env::var("HEYA_TEST_VIDEO_CHILD").as_deref() == Ok("1") {
        std::process::exit(if super::child().is_ok() { 0 } else { 1 });
    }
}

async fn state(rx: &mut watch::Receiver<Status>, accept: impl Fn(&Status) -> bool) -> Status {
    tokio::time::timeout(Duration::from_secs(6), rx.wait_for(accept))
        .await
        .unwrap()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn synthetic_player_pauses_seeks_and_acknowledges_process_exit() {
    let (updates, mut snapshots) = watch::channel(Status::default());
    let mut worker = Worker::start(1, Vec::new(), true, true, Default::default(), updates).unwrap();
    state(&mut snapshots, |s| {
        s.phase == 2 && s.position > 0.1 && s.frame.is_some()
    })
    .await;
    let mut control = Control {
        playing: false,
        ..Default::default()
    };
    worker.set(&control);
    let paused = state(&mut snapshots, |s| s.phase == 3).await;
    tokio::time::sleep(Duration::from_millis(180)).await;
    let still = snapshots.borrow().clone();
    assert!((still.position - paused.position).abs() < 0.001);
    assert_eq!(
        still.frame_number, paused.frame_number,
        "paused playback must not keep uploading identical frames"
    );
    control.seek = Some((1, 3.));
    control.muted = true;
    worker.set(&control);
    state(&mut snapshots, |s| {
        s.phase == 3 && s.position >= 3. && s.muted && s.frame_number > paused.frame_number
    })
    .await;
    control.playing = true;
    control.seek = Some((2, 4.8));
    worker.set(&control);
    tokio::time::timeout(Duration::from_secs(4), worker.finished())
        .await
        .unwrap()
        .unwrap();
    assert!(
        worker.task.is_finished(),
        "completion acknowledges the child process exiting"
    );

    let (updates, mut snapshots) = watch::channel(Status::default());
    let mut worker = Worker::start(2, Vec::new(), true, true, Default::default(), updates).unwrap();
    state(&mut snapshots, |s| s.frame.is_some()).await;
    let began = Instant::now();
    worker.stop().await;
    assert!(began.elapsed() < Duration::from_secs(3));
    assert!(worker.task.is_finished());
}

#[tokio::test]
async fn frame_protocol_rejects_oversized_frames_and_nonfinite_progress() {
    let (updates, _) = watch::channel(Status::default());
    let mut bytes = MAGIC.to_vec();
    bytes.push(1);
    bytes.extend((video::EDGE + 1).to_le_bytes());
    bytes.extend(1u32.to_le_bytes());
    assert!(matches!(
        read_output(bytes.as_slice(), 1, updates.clone()).await,
        Err(Error::Playback)
    ));
    let mut bytes = MAGIC.to_vec();
    bytes.push(0);
    let json = br#"{"phase":2,"position":-1,"duration":5,"muted":false,"error":null}"#;
    bytes.extend((json.len() as u32).to_le_bytes());
    bytes.extend(json);
    assert!(matches!(
        read_output(bytes.as_slice(), 1, updates).await,
        Err(Error::Playback)
    ));
}

#[tokio::test]
#[ignore = "run scripts/ui-offscreen.sh --video-check with synthetic H.264/AAC bytes"]
async fn real_playback() {
    let bytes =
        std::fs::read(std::env::var_os("HEYA_TEST_VIDEO_FILE").expect("synthetic test file"))
            .unwrap();
    let (updates, mut snapshots) = watch::channel(Status::default());
    let mut worker = Worker::start(1, bytes, false, true, Default::default(), updates).unwrap();
    let playing = state(&mut snapshots, |s| {
        s.phase == 2 && s.position > 0.1 && s.frame.is_some()
    })
    .await;
    let pixels = playing.frame.unwrap();
    assert!(pixels.width <= video::EDGE && pixels.height <= video::EDGE);
    let aspect = std::env::var("HEYA_TEST_VIDEO_ASPECT")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(16. / 9.);
    assert!(
        (pixels.width as f64 / pixels.height as f64 - aspect).abs() < 0.01,
        "frame scaling preserves the source aspect ratio: {}x{}",
        pixels.width,
        pixels.height
    );
    assert!(playing.duration >= 3.9);
    let mut control = Control {
        playing: false,
        ..Default::default()
    };
    worker.set(&control);
    state(&mut snapshots, |s| s.phase == 3).await;
    tokio::time::sleep(Duration::from_millis(180)).await;
    let paused = snapshots.borrow().clone();
    tokio::time::sleep(Duration::from_millis(180)).await;
    assert_eq!(
        snapshots.borrow().frame_number,
        paused.frame_number,
        "paused decoder must not keep emitting the same frame"
    );
    control.seek = Some((1, 2.));
    worker.set(&control);
    state(&mut snapshots, |s| s.phase == 3 && s.position > 1.8).await;
    control.playing = true;
    control.muted = true;
    worker.set(&control);
    state(&mut snapshots, |s| {
        s.phase == 2 && s.position > 2.1 && s.muted
    })
    .await;
    tokio::time::timeout(Duration::from_secs(5), worker.finished())
        .await
        .unwrap()
        .unwrap();
    println!(
        "Native video: decoded bounded RGBA frames, paused, sought, muted and reached EOS; decoder process exited."
    );
}

#[tokio::test]
async fn malformed_video_fails_and_a_new_player_can_start() {
    if !video::SUPPORTED {
        return;
    }
    let (updates, _states) = watch::channel(Status::default());
    let mut bad = Worker::start(
        1,
        b"invalid MP4".to_vec(),
        false,
        true,
        Default::default(),
        updates.clone(),
    )
    .unwrap();
    // Both native backends allow 15 seconds for a first frame, then the parent
    // waits up to two seconds to reap the child. AVFoundation may use that full
    // startup window before rejecting malformed input.
    let error = tokio::time::timeout(Duration::from_secs(20), bad.finished())
        .await
        .unwrap();
    assert!(error.is_err());
    assert!(bad.task.is_finished(), "failure acknowledges decoder exit");
    let mut valid = Worker::start(2, Vec::new(), true, true, Default::default(), updates).unwrap();
    valid.stop().await;
    assert!(valid.task.is_finished());
}
