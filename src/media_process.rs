//! Finite voice capture/preview processes using the platform audio backend.
//! Capture resources stay outside the UI process. Media travels only over pipes.
use crate::voice_recorder::{Clip, Desired, Error, Input, MAX_BYTES, MAX_SECONDS, Progress};
use std::{
    io::{Read, Write},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

const MAGIC: &[u8] = b"HEYA-VOICE/1\n";
pub enum Mode {
    Capture(Input, crate::audio_devices::Settings),
    Devices,
    Preview {
        bytes: Arc<[u8]>,
        silent: bool,
        settings: crate::audio_devices::Settings,
    },
}
pub enum Outcome {
    Clip(Clip),
    End,
    Devices(Vec<crate::audio_devices::Device>),
}
pub struct Worker {
    control: watch::Sender<Desired>,
    task: tokio::task::JoinHandle<Result<Outcome, Error>>,
}
impl Worker {
    pub fn start(mode: Mode, progress: watch::Sender<Progress>) -> Result<Self, Error> {
        if !crate::audio::SUPPORTED {
            return Err(Error::Disabled);
        }
        let (control, commands) = watch::channel(Desired::Record);
        Ok(Self {
            control,
            task: tokio::spawn(pump(mode, progress, commands)),
        })
    }
    pub fn set(&self, desired: Desired) {
        self.control.send_replace(desired);
    }
    pub fn stop(&self) {
        self.set(Desired::Cancel);
    }
    pub async fn finished(&mut self) -> Result<Outcome, Error> {
        (&mut self.task).await.map_err(|_| Error::Device)?
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}
fn command(mode: &str) -> Result<tokio::process::Command, Error> {
    let mut cmd = tokio::process::Command::new(std::env::current_exe().map_err(|_| Error::Device)?);
    #[cfg(not(test))]
    cmd.args(["--heya-media-worker", mode]);
    #[cfg(test)]
    cmd.args([
        "--exact",
        "media_process::tests::child",
        "--ignored",
        "--nocapture",
    ])
    .env("HEYA_TEST_MEDIA_MODE", mode);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(cmd)
}
async fn pump(
    mode: Mode,
    progress: watch::Sender<Progress>,
    mut controls: watch::Receiver<Desired>,
) -> Result<Outcome, Error> {
    let name = match &mode {
        Mode::Capture(Input::Microphone, _) => "record",
        Mode::Capture(Input::Synthetic, _) => "record-synthetic",
        Mode::Capture(Input::Unavailable, _) => "record-unavailable",
        Mode::Preview { silent: true, .. } => "preview-silent",
        Mode::Preview { silent: false, .. } => "preview",
        Mode::Devices => "devices",
    };
    let mut child = command(name)?.spawn().map_err(|_| Error::Device)?;
    let mut input = child.stdin.take().ok_or(Error::Device)?;
    let output = child.stdout.take().ok_or(Error::Device)?;
    let settings = match &mode {
        Mode::Capture(_, s) | Mode::Preview { settings: s, .. } => s.clone(),
        Mode::Devices => Default::default(),
    };
    if !matches!(mode, Mode::Devices) {
        let json = serde_json::to_vec(&settings).map_err(|_| Error::Device)?;
        input
            .write_u32_le(json.len() as u32)
            .await
            .map_err(|_| Error::Device)?;
        input.write_all(&json).await.map_err(|_| Error::Device)?;
    }
    if let Mode::Preview { bytes, .. } = mode {
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            let _ = child.kill().await;
            return Err(Error::Limit);
        }
        if input.write_u32_le(bytes.len() as u32).await.is_err()
            || input.write_all(&bytes).await.is_err()
        {
            let _ = child.kill().await;
            return Err(Error::Playback);
        }
    }
    // Keep the reader future alive across control changes: read_exact is not
    // cancellation safe halfway through a frame.
    let read = read_output(output, progress);
    tokio::pin!(read);
    let mut deadline = if matches!(name, "devices") {
        Some(tokio::time::Instant::now() + Duration::from_secs(5))
    } else {
        None
    };
    let mut controls_open = true;
    let result = loop {
        tokio::select! {
            result = &mut read => break result,
            changed = controls.changed(), if controls_open => {
                controls_open = changed.is_ok();
                let desired = if changed.is_err() { Desired::Cancel } else { *controls.borrow_and_update() };
                let byte = match desired { Desired::Record=>0, Desired::Pause=>1, Desired::Finish=>2, Desired::Cancel=>3 };
                if input.write_all(&[byte]).await.is_err() { break Err(Error::Device); }
                if desired == Desired::Cancel { deadline=Some(tokio::time::Instant::now()+Duration::from_secs(2)); }
                else if desired == Desired::Finish { deadline=Some(tokio::time::Instant::now()+Duration::from_secs(7)); }
            }
            _ = async { match deadline { Some(d)=>tokio::time::sleep_until(d).await, None=>std::future::pending().await } } => break Err(Error::Cancelled),
        }
    };
    // A finished operation acknowledges process exit, including all device and
    // plugin threads. EOF also stops the helper if the parent disappears.
    drop(input);
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) if status.success() => result,
        _ => {
            let _ = child.kill().await;
            Err(Error::Device)
        }
    }
}
async fn read_output(
    mut input: impl AsyncRead + Unpin,
    progress: watch::Sender<Progress>,
) -> Result<Outcome, Error> {
    // libtest prints a short preamble in the synthetic helper test executable.
    let mut matched = 0;
    for _ in 0..256 {
        let byte = input.read_u8().await.map_err(|_| Error::Device)?;
        matched = if byte == MAGIC[matched] {
            matched + 1
        } else {
            usize::from(byte == MAGIC[0])
        };
        if matched == MAGIC.len() {
            break;
        }
    }
    if matched != MAGIC.len() {
        return Err(Error::Device);
    }
    loop {
        match input.read_u8().await.map_err(|_| Error::Device)? {
            0 => {
                let seconds = input.read_f64_le().await.map_err(|_| Error::Device)?;
                let mut levels = vec![0; 64];
                input
                    .read_exact(&mut levels)
                    .await
                    .map_err(|_| Error::Device)?;
                if !seconds.is_finite()
                    || !(0.0..=f64::from(MAX_SECONDS) + 1.).contains(&seconds)
                    || levels.iter().any(|v| *v > 100)
                {
                    return Err(Error::Device);
                }
                progress.send_replace(Progress { seconds, levels });
            }
            1 => {
                let seconds = input.read_u32_le().await.map_err(|_| Error::Device)?;
                let mut waveform = vec![0; 64];
                input
                    .read_exact(&mut waveform)
                    .await
                    .map_err(|_| Error::Device)?;
                let len = input.read_u32_le().await.map_err(|_| Error::Device)? as usize;
                if len == 0
                    || len > MAX_BYTES
                    || !(1..=MAX_SECONDS).contains(&seconds)
                    || waveform.iter().any(|v| *v > 100)
                {
                    return Err(Error::Limit);
                }
                let mut bytes = vec![0; len];
                input
                    .read_exact(&mut bytes)
                    .await
                    .map_err(|_| Error::Device)?;
                return Ok(Outcome::Clip(Clip {
                    bytes: bytes.into(),
                    seconds,
                    waveform,
                }));
            }
            2 => {
                return Err(decode_error(
                    input.read_u8().await.map_err(|_| Error::Device)?,
                ));
            }
            3 => return Ok(Outcome::End),
            4 => {
                let len = input.read_u32_le().await.map_err(|_| Error::Device)? as usize;
                if len > 65_536 {
                    return Err(Error::Limit);
                }
                let mut bytes = vec![0; len];
                input
                    .read_exact(&mut bytes)
                    .await
                    .map_err(|_| Error::Device)?;
                let devices: Vec<crate::audio_devices::Device> =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Device)?;
                if devices.len() > 128
                    || devices
                        .iter()
                        .any(|d| d.id.len() > 128 || d.name.len() > 1024)
                {
                    return Err(Error::Limit);
                }
                return Ok(Outcome::Devices(devices));
            }
            _ => return Err(Error::Device),
        }
    }
}
pub(crate) fn decode_error(code: u8) -> Error {
    match code {
        0 => Error::Disabled,
        1 => Error::Codec,
        2 => Error::Empty,
        3 => Error::Limit,
        4 => Error::Cancelled,
        5 => Error::Playback,
        7 => Error::NoInput,
        8 => Error::Permission,
        _ => Error::Device,
    }
}
pub(crate) fn error_code(e: Error) -> u8 {
    match e {
        Error::Disabled => 0,
        Error::Codec => 1,
        Error::Empty => 2,
        Error::Limit => 3,
        Error::Cancelled => 4,
        Error::Playback => 5,
        Error::Device => 6,
        Error::NoInput => 7,
        Error::Permission => 8,
    }
}
fn publish(out: &mut impl Write, p: Progress) {
    let result = (|| -> std::io::Result<()> {
        out.write_all(&[0])?;
        out.write_all(&p.seconds.to_le_bytes())?;
        let mut levels = [0; 64];
        for (to, from) in levels.iter_mut().zip(p.levels) {
            *to = from.min(100);
        }
        out.write_all(&levels)?;
        out.flush()
    })();
    if result.is_err() {
        std::process::exit(0);
    }
}
/// Internal subprocess entry. No account files, UI, logs or network are opened.
pub fn child_main(mode: &str) -> ! {
    let mut out = std::io::stdout().lock();
    if out.write_all(MAGIC).and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
    let result = child_run(mode, &mut out);
    let _ = (|| -> std::io::Result<()> {
        match result {
            Ok(Outcome::Clip(c)) => {
                out.write_all(&[1])?;
                out.write_all(&c.seconds.to_le_bytes())?;
                out.write_all(&c.waveform)?;
                out.write_all(&(c.bytes.len() as u32).to_le_bytes())?;
                out.write_all(&c.bytes)?;
            }
            Ok(Outcome::End) => out.write_all(&[3])?,
            Ok(Outcome::Devices(devices)) => {
                let bytes = serde_json::to_vec(&devices)?;
                out.write_all(&[4])?;
                out.write_all(&(bytes.len() as u32).to_le_bytes())?;
                out.write_all(&bytes)?;
            }
            Err(e) => out.write_all(&[2, error_code(e)])?,
        }
        out.flush()
    })();
    // The command reader and any GStreamer global pool threads belong solely
    // to this operation; process exit releases them together.
    std::process::exit(0)
}
fn child_run(mode: &str, out: &mut impl Write) -> Result<Outcome, Error> {
    if mode == "devices" {
        return crate::audio_devices::snapshot().map(Outcome::Devices);
    }
    if mode == "probe-input" {
        return crate::voice_recorder::probe_input().map(|()| Outcome::End);
    }
    let capture = match mode {
        "record" => Some(Input::Microphone),
        "record-synthetic" => Some(Input::Synthetic),
        "record-unavailable" => Some(Input::Unavailable),
        "preview" | "preview-silent" => None,
        _ => return Err(Error::Disabled),
    };
    let mut input = std::io::stdin();
    let mut len = [0; 4];
    input.read_exact(&mut len).map_err(|_| Error::Device)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 8192 {
        return Err(Error::Limit);
    }
    let mut json = vec![0; len];
    input.read_exact(&mut json).map_err(|_| Error::Device)?;
    let settings: crate::audio_devices::Settings =
        serde_json::from_slice(&json).map_err(|_| Error::Device)?;
    if !settings.valid() {
        return Err(Error::Device);
    }
    let bytes = if capture.is_none() {
        let mut len = [0; 4];
        input.read_exact(&mut len).map_err(|_| Error::Playback)?;
        let len = u32::from_le_bytes(len) as usize;
        if len == 0 || len > MAX_BYTES {
            return Err(Error::Limit);
        }
        let mut bytes = vec![0; len];
        input.read_exact(&mut bytes).map_err(|_| Error::Playback)?;
        Some(Arc::<[u8]>::from(bytes))
    } else {
        None
    };
    let (tx, commands) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("heya-control".into())
        .spawn(move || {
            let mut byte = [0];
            loop {
                // EOF means the parent is gone, even if a device/plugin is hung.
                if input.read_exact(&mut byte).is_err() {
                    std::process::exit(0);
                }
                let desired = match byte[0] {
                    0 => Desired::Record,
                    1 => Desired::Pause,
                    2 => Desired::Finish,
                    _ => Desired::Cancel,
                };
                if tx.send(desired).is_err() {
                    return;
                }
            }
        })
        .map_err(|_| Error::Device)?;
    if let Some(input) = capture {
        crate::voice_recorder::capture(input, settings, |p| publish(out, p), commands)
            .map(Outcome::Clip)
    } else {
        preview(
            bytes.ok_or(Error::Playback)?,
            mode == "preview-silent",
            settings,
            out,
            commands,
        )?;
        Ok(Outcome::End)
    }
}
fn preview(
    bytes: Arc<[u8]>,
    silent: bool,
    settings: crate::audio_devices::Settings,
    out: &mut impl Write,
    commands: std::sync::mpsc::Receiver<Desired>,
) -> Result<(), Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| Error::Playback)?;
    runtime.block_on(async {
        let (tx, mut status) = watch::channel(crate::audio::Status::default());
        let mut player = crate::audio_player::Worker::start(
            crate::audio_player::Desired {
                generation: 1,
                id: 1,
                bytes: Some(bytes),
                playing: true,
                speed: 1.,
                seek: None,
            },
            tx,
            if silent {
                crate::audio_player::Output::Silent
            } else {
                crate::audio_player::Output::Desktop(settings)
            },
        )
        .map_err(|_| Error::Playback)?;
        loop {
            tokio::select! {
                result=player.finished() => {
                    let s = result.map_err(|_| Error::Playback)?;
                    publish(
                        out,
                        Progress {
                            seconds: s.duration,
                            levels: vec![0; 64],
                        },
                    );
                    return if s.error.is_some() {
                        Err(Error::Playback)
                    } else {
                        Ok(())
                    };
                }
                changed=status.changed() => {
                    if changed.is_ok() {
                        let s = status.borrow_and_update();
                        publish(
                            out,
                            Progress {
                                seconds: s.position,
                                levels: vec![0; 64],
                            },
                        );
                    }
                }
                _=tokio::time::sleep(Duration::from_millis(50)) => {
                    if commands.try_recv().is_ok() {
                        player.stop();
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "subprocess fixture, invoked only by synthetic media tests"]
    fn child() {
        if let Ok(mode) = std::env::var("HEYA_TEST_MEDIA_MODE") {
            assert!(matches!(
                mode.as_str(),
                "record-synthetic" | "record-unavailable" | "preview-silent"
            ));
            child_main(&mode);
        }
    }
    #[cfg(any(
        target_os = "macos",
        all(feature = "media-gstreamer", target_os = "linux")
    ))]
    #[tokio::test]
    async fn missing_microphone_returns_error_after_helper_exit_without_recording() {
        for _ in 0..2 {
            let (tx, progress) = watch::channel(Progress::default());
            let mut worker =
                Worker::start(Mode::Capture(Input::Unavailable, Default::default()), tx).unwrap();
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(5), worker.finished())
                    .await
                    .unwrap(),
                Err(Error::NoInput)
            ));
            // Completion waits for child exit/reap. No audio or waveform was produced.
            assert_eq!(progress.borrow().seconds, 0.);
            assert!(progress.borrow().levels.is_empty());
        }
    }
    #[tokio::test]
    async fn rejects_oversized_and_truncated_frames_before_allocating_media() {
        let (tx, _) = watch::channel(Progress::default());
        let mut missing = MAGIC.to_vec();
        missing.extend([2, 7]);
        assert!(matches!(
            read_output(missing.as_slice(), tx.clone()).await,
            Err(Error::NoInput)
        ));
        let mut oversized = MAGIC.to_vec();
        oversized.push(1);
        oversized.extend(1u32.to_le_bytes());
        oversized.extend([0; 64]);
        oversized.extend((MAX_BYTES as u32 + 1).to_le_bytes());
        assert!(matches!(
            read_output(oversized.as_slice(), tx.clone()).await,
            Err(Error::Limit)
        ));
        assert!(matches!(
            read_output(&MAGIC[..MAGIC.len() - 1], tx).await,
            Err(Error::Device)
        ));
    }
    #[cfg(any(
        target_os = "macos",
        all(feature = "media-gstreamer", target_os = "linux")
    ))]
    #[tokio::test]
    async fn cancel_and_parent_eof_stop_and_reap_synthetic_helpers() {
        let (tx, mut status) = watch::channel(Progress::default());
        let mut worker =
            Worker::start(Mode::Capture(Input::Synthetic, Default::default()), tx).unwrap();
        tokio::time::timeout(Duration::from_secs(5), status.changed())
            .await
            .unwrap()
            .unwrap();
        worker.stop();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(4), worker.finished())
                .await
                .unwrap(),
            Err(Error::Cancelled)
        ));
        // Dropping the public owner also requests cancellation; await its real
        // pump task to establish that process teardown completed.
        let (tx, mut status) = watch::channel(Progress::default());
        let mut worker =
            Worker::start(Mode::Capture(Input::Synthetic, Default::default()), tx).unwrap();
        tokio::time::timeout(Duration::from_secs(5), status.changed())
            .await
            .unwrap()
            .unwrap();
        let task = std::mem::replace(
            &mut worker.task,
            tokio::spawn(async { Err(Error::Cancelled) }),
        );
        drop(worker);
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(4), task)
                .await
                .unwrap()
                .unwrap(),
            Err(Error::Cancelled)
        ));
        let mut child = command("record-synthetic").unwrap().spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let json = serde_json::to_vec(&crate::audio_devices::Settings::default()).unwrap();
        input.write_u32_le(json.len() as u32).await.unwrap();
        input.write_all(&json).await.unwrap();
        let output = child.stdout.take().unwrap();
        // Wait for actual encoded input before simulating a vanished parent.
        let (tx, mut status) = watch::channel(Progress::default());
        let reader = tokio::spawn(read_output(output, tx));
        tokio::time::timeout(Duration::from_secs(5), status.changed())
            .await
            .unwrap()
            .unwrap();
        drop(input);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        assert!(reader.await.unwrap().is_err());
    }
}
