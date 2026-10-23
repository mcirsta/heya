//! Reuse the platform player behind a killable process boundary. A blocked
//! device/plugin must not keep Heya's playback service or shutdown waiting.
use crate::{
    audio::{Error, Status},
    audio_player::{Desired, Output},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, Read, Write},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

const MAGIC: &[u8] = b"HEYA-AUDIO/1\n";
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Serialize, Deserialize)]
struct Control {
    playing: bool,
    speed: f64,
    seek: Option<(u64, f64)>,
}
impl Control {
    fn from_desired(value: &Desired) -> Self {
        Self {
            playing: value.playing,
            speed: value.speed,
            seek: value.seek,
        }
    }
    fn valid(&self) -> bool {
        self.speed.is_finite()
            && (0.5..=3.).contains(&self.speed)
            && self
                .seek
                .is_none_or(|(_, f)| f.is_finite() && (0.0..=1.).contains(&f))
    }
    fn apply(self, desired: &mut Desired) {
        desired.playing = self.playing;
        desired.speed = self.speed;
        desired.seek = self.seek;
    }
}
#[derive(Serialize, Deserialize)]
struct Header {
    status: Status,
    output: Output,
    control: Control,
    #[cfg(test)]
    #[serde(default)]
    stall: Option<i32>,
}

pub async fn run(
    mut desired: watch::Receiver<Desired>,
    updates: watch::Sender<Status>,
    output: Output,
) -> Status {
    let initial = desired.borrow_and_update().clone();
    let mut status = Status {
        id: initial.id,
        generation: initial.generation,
        phase: 1,
        speed: initial.speed,
        ..Default::default()
    };
    let header = Header {
        status: status.clone(),
        output,
        control: Control::from_desired(&initial),
        #[cfg(test)]
        stall: None,
    };
    match pump(&initial, header, desired, updates.clone()).await {
        Ok(result) => status = result,
        Err(error) => {
            let latest = updates.borrow().clone();
            if latest.id == initial.id && latest.generation == initial.generation {
                status = latest;
            }
            status.fail(error);
            eprintln!("Heya audio playback failed: {error:?}");
        }
    }
    updates.send_replace(status.clone());
    status
}

async fn pump(
    initial: &Desired,
    header: Header,
    mut desired: watch::Receiver<Desired>,
    updates: watch::Sender<Status>,
) -> Result<Status, Error> {
    let mut command =
        tokio::process::Command::new(std::env::current_exe().map_err(|_| Error::Playback)?);
    #[cfg(not(test))]
    command.arg("--heya-audio-worker");
    #[cfg(test)]
    command
        .args([
            "--exact",
            "audio_process::tests::child",
            "--ignored",
            "--nocapture",
        ])
        .env("HEYA_TEST_AUDIO_CHILD", "1");
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::Playback)?;
    let result = async {
        let mut input = child.stdin.take().ok_or(Error::Playback)?;
        let output = child.stdout.take().ok_or(Error::Playback)?;
        let read = read_output(output, header.status.clone(), updates, desired.clone());
        let mut stopping = desired.clone();
        let write = async {
            let json = serde_json::to_vec(&header).map_err(|_| Error::Playback)?;
            input
                .write_u32_le(json.len() as u32)
                .await
                .map_err(|_| Error::Playback)?;
            input.write_all(&json).await.map_err(|_| Error::Playback)?;
            let bytes = initial.bytes.as_ref().ok_or(Error::Unavailable)?;
            input
                .write_u32_le(bytes.len() as u32)
                .await
                .map_err(|_| Error::Playback)?;
            input.write_all(bytes).await.map_err(|_| Error::Playback)?;
            loop {
                desired.changed().await.map_err(|_| Error::Playback)?;
                let current = desired.borrow_and_update().clone();
                let mut json = serde_json::to_vec(&Control::from_desired(&current))
                    .map_err(|_| Error::Playback)?;
                json.push(b'\n');
                input.write_all(&json).await.map_err(|_| Error::Playback)?;
            }
        };
        tokio::pin!(read, write);
        tokio::select! {
            biased;
            _ = stopping.wait_for(|d| d.bytes.is_none() || d.generation != initial.generation) => {
                Ok(Status { phase: 0, ..header.status.clone() })
            },
            result = &mut read => result,
            result = &mut write => result,
        }
    }
    .await;
    // Dropping stdin tells the independent pipe reader to exit even when the
    // main helper thread is stuck opening, seeking or closing a device.
    if !matches!(
        tokio::time::timeout(Duration::from_secs(2), child.wait()).await,
        Ok(Ok(_))
    ) {
        let _ = child.kill().await;
    }
    result
}

async fn read_output(
    mut output: tokio::process::ChildStdout,
    mut status: Status,
    updates: watch::Sender<Status>,
    mut controls: watch::Receiver<Desired>,
) -> Result<Status, Error> {
    let mut deadline = Some(tokio::time::Instant::now() + RESPONSE_TIMEOUT);
    let read = async {
        let mut matched = 0;
        for _ in 0..256 {
            let byte = output.read_u8().await.map_err(|_| Error::Playback)?;
            matched = if byte == MAGIC[matched] {
                matched + 1
            } else {
                usize::from(byte == MAGIC[0])
            };
            if matched == MAGIC.len() {
                return Ok(());
            }
        }
        Err(Error::Playback)
    };
    tokio::time::timeout(RESPONSE_TIMEOUT, read)
        .await
        .map_err(|_| Error::Unresponsive)??;
    loop {
        let id = status.id;
        let generation = status.generation;
        let read = async {
            let len = output.read_u32_le().await.map_err(|_| Error::Playback)? as usize;
            if len == 0 || len > 4096 {
                return Err(Error::Playback);
            }
            let mut data = vec![0; len];
            output
                .read_exact(&mut data)
                .await
                .map_err(|_| Error::Playback)?;
            let next: Status = serde_json::from_slice(&data).map_err(|_| Error::Playback)?;
            if next.id != id
                || next.generation != generation
                || !next.position.is_finite()
                || !next.duration.is_finite()
                || !(0.0..=21600.).contains(&next.position)
                || !(0.0..=21600.).contains(&next.duration)
                || !(0..=4).contains(&next.phase)
            {
                return Err(Error::Playback);
            }
            Ok(next)
        };
        tokio::pin!(read);
        status = loop {
            tokio::select! {
                next = &mut read => break next?,
                changed = controls.changed() => {
                    if changed.is_err() { return Err(Error::Playback); }
                    controls.borrow_and_update();
                    // Resume/seek/pause also need an acknowledgement, even
                    // when the previous paused state had no running deadline.
                    deadline = Some(tokio::time::Instant::now() + RESPONSE_TIMEOUT);
                },
                _ = async { match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                } } => return Err(Error::Unresponsive),
            }
        };
        updates.send_replace(status.clone());
        if matches!(status.phase, 0 | 4) {
            return Ok(status);
        }
        // A paused player waits indefinitely. Playing/loading must make progress.
        deadline = (status.phase != 3).then(|| tokio::time::Instant::now() + RESPONSE_TIMEOUT);
    }
}

pub fn child_main() -> ! {
    if child_run().is_err() {
        std::process::exit(1);
    }
    std::process::exit(0)
}
fn child_run() -> Result<(), Error> {
    std::io::stdout()
        .write_all(MAGIC)
        .and_then(|_| std::io::stdout().flush())
        .map_err(|_| Error::Playback)?;
    let mut input = std::io::stdin();
    let mut len = [0; 4];
    input.read_exact(&mut len).map_err(|_| Error::Playback)?;
    let size = u32::from_le_bytes(len) as usize;
    if size == 0 || size > 4096 {
        return Err(Error::Playback);
    }
    let mut json = vec![0; size];
    input.read_exact(&mut json).map_err(|_| Error::Playback)?;
    let header: Header = serde_json::from_slice(&json).map_err(|_| Error::Playback)?;
    if !header.control.valid() {
        return Err(Error::Playback);
    }
    input.read_exact(&mut len).map_err(|_| Error::Playback)?;
    let size = u32::from_le_bytes(len) as usize;
    if size == 0 || size > crate::audio::MAX_BYTES {
        return Err(Error::TooLarge);
    }
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes).map_err(|_| Error::Playback)?;
    let mut initial = Desired {
        id: header.status.id,
        generation: header.status.generation,
        bytes: Some(bytes.into()),
        ..Default::default()
    };
    header.control.apply(&mut initial);
    let (control, desired) = watch::channel(initial);
    std::thread::Builder::new()
        .name("heya-audio-pipe".into())
        .spawn(move || {
            let mut reader = std::io::BufReader::new(input);
            loop {
                let mut line = String::new();
                if reader.by_ref().take(4097).read_line(&mut line).is_err() || line.is_empty() {
                    std::process::exit(0);
                }
                if line.len() > 4096 || !line.ends_with('\n') {
                    std::process::exit(1);
                }
                let Ok(next) = serde_json::from_str::<Control>(&line) else {
                    std::process::exit(1);
                };
                if !next.valid() {
                    std::process::exit(1);
                }
                control.send_modify(|d| next.apply(d));
            }
        })
        .map_err(|_| Error::Playback)?;
    #[cfg(test)]
    if let Some(phase) = header.stall {
        if phase != 1 {
            let mut status = header.status.clone();
            status.phase = phase;
            let data = serde_json::to_vec(&status).map_err(|_| Error::Playback)?;
            std::io::stdout()
                .write_all(&(data.len() as u32).to_le_bytes())
                .and_then(|_| std::io::stdout().write_all(&data))
                .and_then(|_| std::io::stdout().flush())
                .map_err(|_| Error::Playback)?;
        }
        loop {
            std::thread::park();
        }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|_| Error::Playback)?;
    let _entered = runtime.enter();
    let (updates, mut snapshots) = watch::channel(header.status);
    let handle = runtime.handle().clone();
    let writer = std::thread::Builder::new()
        .name("heya-audio-status".into())
        .spawn(move || {
            let mut out = std::io::stdout().lock();
            while handle.block_on(snapshots.changed()).is_ok() {
                let status = snapshots.borrow_and_update().clone();
                let Ok(data) = serde_json::to_vec(&status) else {
                    std::process::exit(1);
                };
                if out
                    .write_all(&(data.len() as u32).to_le_bytes())
                    .and_then(|_| out.write_all(&data))
                    .and_then(|_| out.flush())
                    .is_err()
                {
                    std::process::exit(0);
                }
            }
        })
        .map_err(|_| Error::Playback)?;
    let final_status = crate::audio_player::run(desired, updates.clone(), header.output);
    updates.send_replace(final_status);
    drop(updates);
    writer.join().map_err(|_| Error::Playback)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "finite playback helper, invoked by synthetic parent tests"]
    fn child() {
        if std::env::var("HEYA_TEST_AUDIO_CHILD").as_deref() == Ok("1") {
            child_main();
        }
    }
    #[tokio::test]
    async fn a_blocked_device_can_be_cancelled_and_retried_without_a_worker_left_behind() {
        let initial = Desired {
            id: 1,
            generation: 1,
            bytes: Some(crate::fixtures::audio_bytes().into()),
            playing: true,
            speed: 1.,
            ..Default::default()
        };
        let status = Status {
            id: 1,
            generation: 1,
            phase: 1,
            speed: 1.,
            ..Default::default()
        };
        let header = Header {
            status,
            output: Output::Silent,
            control: Control::from_desired(&initial),
            stall: Some(1),
        };
        let (tx, rx) = watch::channel(initial.clone());
        let (updates, _) = watch::channel(Status::default());
        let task = tokio::spawn(async move { pump(&initial, header, rx, updates).await });
        tokio::time::sleep(Duration::from_millis(150)).await;
        tx.send_modify(|d| d.clear());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .phase,
            0
        );
        #[cfg(any(
            target_os = "macos",
            all(target_os = "linux", feature = "media-gstreamer")
        ))]
        {
            let (updates, _) = watch::channel(Status::default());
            let mut next =
                crate::audio_player::Worker::start(tx.borrow().clone(), updates, Output::Silent);
            assert!(next.is_err(), "cancelled bytes cannot be reused");
            let initial = Desired {
                id: 2,
                generation: 2,
                bytes: Some(crate::fixtures::audio_bytes().into()),
                playing: true,
                speed: 1.,
                ..Default::default()
            };
            let (updates, _) = watch::channel(Status::default());
            next = crate::audio_player::Worker::start(initial, updates, Output::Silent);
            let mut next = next.unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(5), next.finished())
                    .await
                    .unwrap()
                    .unwrap()
                    .phase,
                0
            );
        }
    }
    #[tokio::test]
    async fn startup_and_playback_stalls_time_out_and_a_paused_player_is_cancellable() {
        for phase in [1, 2, 3] {
            let initial = Desired {
                id: 1,
                generation: 1,
                bytes: Some(crate::fixtures::audio_bytes().into()),
                playing: phase != 3,
                speed: 1.,
                ..Default::default()
            };
            let status = Status {
                id: 1,
                generation: 1,
                phase: 1,
                speed: 1.,
                ..Default::default()
            };
            let header = Header {
                status,
                output: Output::Silent,
                control: Control::from_desired(&initial),
                stall: Some(phase),
            };
            let (tx, rx) = watch::channel(initial.clone());
            let (updates, mut progress) = watch::channel(Status::default());
            let task = tokio::spawn(async move { pump(&initial, header, rx, updates).await });
            if phase == 3 {
                tokio::time::timeout(Duration::from_secs(3), progress.wait_for(|p| p.phase == 3))
                    .await
                    .unwrap()
                    .unwrap();
                // A paused helper is quiet by design; stopping still closes it.
                tx.send_modify(|d| d.clear());
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(3), task)
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap()
                        .phase,
                    0
                );
            } else {
                assert!(matches!(
                    tokio::time::timeout(Duration::from_secs(13), task)
                        .await
                        .unwrap()
                        .unwrap(),
                    Err(Error::Unresponsive)
                ));
            }
        }
    }
}
