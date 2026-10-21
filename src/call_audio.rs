//! Duplex PCM in a finite platform-audio process. No device or thread exists at idle.
use crate::{
    audio_devices::Settings,
    voice_recorder::{Error, Input},
};
use std::{
    io::{Read, Write},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{oneshot, watch},
};

const MAGIC: &[u8] = b"HEYA-CALL-AUDIO/1\n";
pub const SAMPLES: usize = 960; // SDK's 60 ms, 16 kHz mono PCM contract.
const MAX_SAMPLES: usize = 4096;
#[derive(serde::Serialize, serde::Deserialize)]
struct Header {
    settings: Settings,
    synthetic: bool,
    unavailable: bool,
}
pub struct Ports {
    pub microphone: async_channel::Receiver<Vec<i16>>,
    pub speaker: async_channel::Sender<Vec<i16>>,
}
pub struct Worker {
    done: bool,
    stop: watch::Sender<bool>,
    ready: Option<oneshot::Receiver<Result<(), Error>>>,
    task: tokio::task::JoinHandle<Result<(), Error>>,
}
impl Worker {
    pub fn start(settings: Settings, input: Input) -> Result<(Self, Ports), Error> {
        if !settings.valid() {
            return Err(Error::Device);
        }
        if matches!(input, Input::Microphone) && !crate::audio::SUPPORTED {
            return Err(Error::Disabled);
        }
        let (stop, stopped) = watch::channel(false);
        let (ready, prepared) = oneshot::channel();
        // Drop excess audio rather than accumulate latency during a stalled device.
        let (microphone, capture) = async_channel::bounded(3);
        let trim = capture.clone();
        let (speaker, playout) = async_channel::bounded(3);
        let task = tokio::spawn(pump(
            Header {
                settings,
                synthetic: matches!(input, Input::Synthetic),
                unavailable: matches!(input, Input::Unavailable),
            },
            ready,
            microphone,
            trim,
            playout,
            stopped,
        ));
        Ok((
            Self {
                done: false,
                stop,
                ready: Some(prepared),
                task,
            },
            Ports {
                microphone: capture,
                speaker,
            },
        ))
    }
    pub async fn ready(&mut self) -> Result<(), Error> {
        self.ready
            .take()
            .ok_or(Error::Device)?
            .await
            .map_err(|_| Error::Device)?
    }
    pub async fn finished(&mut self) -> Result<(), Error> {
        let result = (&mut self.task).await;
        self.done = true;
        result.map_err(|_| Error::Device)?
    }
    pub async fn stop(&mut self) {
        if self.done {
            return;
        }
        self.stop.send_replace(true);
        if tokio::time::timeout(Duration::from_secs(3), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
            let _ = (&mut self.task).await;
        }
        self.done = true;
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

fn command() -> Result<tokio::process::Command, Error> {
    let mut c = tokio::process::Command::new(std::env::current_exe().map_err(|_| Error::Device)?);
    #[cfg(not(test))]
    c.arg("--heya-call-audio-worker");
    #[cfg(test)]
    c.args([
        "--exact",
        "call_audio::tests::child",
        "--ignored",
        "--nocapture",
    ])
    .env("HEYA_TEST_CALL_AUDIO_CHILD", "1");
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(c)
}
async fn pump(
    header: Header,
    ready: oneshot::Sender<Result<(), Error>>,
    microphone: async_channel::Sender<Vec<i16>>,
    trim: async_channel::Receiver<Vec<i16>>,
    speaker: async_channel::Receiver<Vec<i16>>,
    mut stopped: watch::Receiver<bool>,
) -> Result<(), Error> {
    let mut child = command()?.spawn().map_err(|_| Error::Device)?;
    let result = async {
        let mut input = child.stdin.take().ok_or(Error::Device)?;
        let mut output = child.stdout.take().ok_or(Error::Device)?;
        let header = serde_json::to_vec(&header).map_err(|_| Error::Device)?;
        input
            .write_u32_le(header.len() as u32)
            .await
            .map_err(|_| Error::Device)?;
        input.write_all(&header).await.map_err(|_| Error::Device)?;
        let read = async {
            // Cargo's test helper has a short libtest preamble before the magic.
            let mut matched = 0;
            for _ in 0..256 {
                let b = output.read_u8().await.map_err(|_| Error::Device)?;
                matched = if b == MAGIC[matched] {
                    matched + 1
                } else {
                    usize::from(b == MAGIC[0])
                };
                if matched == MAGIC.len() {
                    break;
                }
            }
            if matched != MAGIC.len() {
                return Err(Error::Device);
            }
            let status = match output.read_u8().await.map_err(|_| Error::Device)? {
                0 => Ok(()),
                2 => Err(crate::media_process::decode_error(
                    output.read_u8().await.map_err(|_| Error::Device)?,
                )),
                _ => Err(Error::Device),
            };
            let _ = ready.send(status);
            status?;
            loop {
                match output.read_u8().await.map_err(|_| Error::Device)? {
                    1 => {
                        let count = output.read_u16_le().await.map_err(|_| Error::Device)? as usize;
                        if count != SAMPLES {
                            return Err(Error::Device);
                        }
                        let mut raw = vec![0; count * 2];
                        output
                            .read_exact(&mut raw)
                            .await
                            .map_err(|_| Error::Device)?;
                        let pcm = raw
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|p| i16::from_le_bytes([p[0], p[1]]))
                            .collect();
                        if let Err(async_channel::TrySendError::Full(pcm)) =
                            microphone.try_send(pcm)
                        {
                            let _ = trim.try_recv();
                            let _ = microphone.try_send(pcm);
                        }
                    }
                    2 => {
                        return Err(crate::media_process::decode_error(
                            output.read_u8().await.map_err(|_| Error::Device)?,
                        ));
                    }
                    _ => return Err(Error::Device),
                }
            }
        };
        let write = async {
            while let Ok(pcm) = speaker.recv().await {
                if pcm.is_empty() || pcm.len() > MAX_SAMPLES {
                    return Err(Error::Device);
                }
                input.write_u8(1).await.map_err(|_| Error::Device)?;
                input
                    .write_u16_le(pcm.len() as u16)
                    .await
                    .map_err(|_| Error::Device)?;
                let raw: Vec<_> = pcm.into_iter().flat_map(i16::to_le_bytes).collect();
                input.write_all(&raw).await.map_err(|_| Error::Device)?;
            }
            Ok(())
        };
        tokio::pin!(read, write);
        tokio::select! {
            result = &mut read => result,
            result = &mut write => result,
            _ = async { if !*stopped.borrow() { let _ = stopped.changed().await; } } => Ok(()),
        }
    }
    .await;
    // Dropping the writer closes the parent pipe; the helper exits even if a
    // platform device blocks. Completion always acknowledges process exit.
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) if status.success() => result,
        _ => {
            let _ = child.kill().await;
            Err(Error::Device)
        }
    }
}

pub trait Device {
    fn capture(&mut self) -> Result<Option<Vec<i16>>, Error>;
    fn play(&mut self, pcm: &[i16]) -> Result<(), Error>;
}
pub fn child_main() -> ! {
    let mut out = std::io::stdout().lock();
    let result = (|| {
        out.write_all(MAGIC).map_err(|_| Error::Device)?;
        out.flush().map_err(|_| Error::Device)?;
        run_child(&mut out)
    })();
    if let Err(e) = result {
        let _ = out.write_all(&[2, crate::media_process::error_code(e)]);
        let _ = out.flush();
    }
    // The finite helper owns all platform and pipe-reader threads.
    std::process::exit(0)
}
fn run_child(out: &mut impl Write) -> Result<(), Error> {
    let mut input = std::io::stdin();
    let mut length = [0; 4];
    input.read_exact(&mut length).map_err(|_| Error::Device)?;
    let len = u32::from_le_bytes(length) as usize;
    if len == 0 || len > 8192 {
        return Err(Error::Device);
    }
    let mut header = vec![0; len];
    input.read_exact(&mut header).map_err(|_| Error::Device)?;
    let header: Header = serde_json::from_slice(&header).map_err(|_| Error::Device)?;
    if !header.settings.valid() {
        return Err(Error::Device);
    }
    if header.unavailable {
        return Err(Error::NoInput);
    }
    let (tx, playout) = std::sync::mpsc::sync_channel(3);
    std::thread::Builder::new()
        .name("heya-call-pipe".into())
        .spawn(move || {
            loop {
                let mut kind = [0];
                // Closing Heya also closes this pipe; no device can keep the helper alive.
                if input.read_exact(&mut kind).is_err() {
                    std::process::exit(0);
                }
                if kind[0] != 1 {
                    std::process::exit(1);
                }
                let mut size = [0; 2];
                if input.read_exact(&mut size).is_err() {
                    std::process::exit(0);
                }
                let count = u16::from_le_bytes(size) as usize;
                if count == 0 || count > MAX_SAMPLES {
                    std::process::exit(1);
                }
                let mut raw = vec![0; count * 2];
                if input.read_exact(&mut raw).is_err() {
                    std::process::exit(0);
                }
                let pcm: Vec<_> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| i16::from_le_bytes([p[0], p[1]]))
                    .collect();
                let _ = tx.try_send(pcm);
            }
        })
        .map_err(|_| Error::Device)?;
    let mut device: Box<dyn Device> = if header.synthetic {
        synthetic()?
    } else {
        platform(header.settings)?
    };
    let began = std::time::Instant::now();
    let first = loop {
        if let Some(pcm) = device.capture()? {
            break pcm;
        }
        if began.elapsed() > Duration::from_secs(5) {
            return Err(Error::Device);
        }
    };
    if first.len() != SAMPLES {
        return Err(Error::Device);
    }
    out.write_all(&[0])
        .and_then(|_| out.flush())
        .map_err(|_| Error::Device)?;
    loop {
        for pcm in playout.try_iter().take(3) {
            device.play(&pcm)?;
        }
        if let Some(pcm) = device.capture()? {
            if pcm.len() != SAMPLES {
                return Err(Error::Device);
            }
            out.write_all(&[1])
                .and_then(|_| out.write_all(&(pcm.len() as u16).to_le_bytes()))
                .map_err(|_| Error::Device)?;
            for sample in pcm {
                out.write_all(&sample.to_le_bytes())
                    .map_err(|_| Error::Device)?;
            }
            out.flush().map_err(|_| Error::Device)?;
        }
    }
}
fn synthetic() -> Result<Box<dyn Device>, Error> {
    #[cfg(all(test, target_os = "linux", feature = "media-gstreamer"))]
    if std::env::var("HEYA_TEST_CALL_GSTREAMER").as_deref() == Ok("1") {
        return Ok(Box::new(crate::gstreamer_call::Audio::synthetic()?));
    }
    Ok(Box::new(Synthetic(std::time::Instant::now())))
}
fn platform(settings: Settings) -> Result<Box<dyn Device>, Error> {
    #[cfg(all(target_os = "linux", feature = "media-gstreamer"))]
    {
        Ok(Box::new(crate::gstreamer_call::Audio::new(settings)?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(crate::macos_call_audio::Audio::new(settings)?))
    }
    #[cfg(not(any(
        target_os = "macos",
        all(target_os = "linux", feature = "media-gstreamer")
    )))]
    {
        let _ = settings;
        Err(Error::Disabled)
    }
}
struct Synthetic(std::time::Instant);
impl Device for Synthetic {
    fn capture(&mut self) -> Result<Option<Vec<i16>>, Error> {
        std::thread::sleep(Duration::from_millis(10));
        if self.0.elapsed() < Duration::from_millis(60) {
            return Ok(None);
        }
        self.0 = std::time::Instant::now();
        Ok(Some(vec![0; SAMPLES]))
    }
    fn play(&mut self, _: &[i16]) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "finite duplex-audio child, invoked by synthetic parent tests"]
    fn child() {
        if std::env::var("HEYA_TEST_CALL_AUDIO_CHILD").as_deref() == Ok("1") {
            child_main();
        }
    }
    #[tokio::test]
    async fn duplex_audio_is_bounded_and_stop_reaps_the_helper() {
        let (mut worker, ports) = Worker::start(Settings::default(), Input::Synthetic).unwrap();
        tokio::time::timeout(Duration::from_secs(4), worker.ready())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), ports.microphone.recv())
                .await
                .unwrap()
                .unwrap()
                .len(),
            SAMPLES
        );
        ports.speaker.send(vec![12; SAMPLES]).await.unwrap();
        worker.stop().await;
        assert!(worker.task.is_finished());
        assert!(ports.microphone.is_closed());
    }
    #[tokio::test]
    async fn missing_microphone_is_reported_and_the_helper_exits() {
        let (mut worker, _ports) = Worker::start(Settings::default(), Input::Unavailable).unwrap();
        assert_eq!(worker.ready().await, Err(Error::NoInput));
        assert_eq!(worker.finished().await, Err(Error::NoInput));
        assert!(worker.task.is_finished());
    }
    #[cfg(all(target_os = "linux", feature = "media-gstreamer"))]
    #[tokio::test]
    #[ignore = "run scripts/ui-offscreen.sh --call-audio-check; uses only audiotestsrc and fakesink"]
    async fn native_gstreamer_duplex_audio() {
        assert_eq!(
            std::env::var("HEYA_TEST_CALL_GSTREAMER").as_deref(),
            Ok("1")
        );
        let (mut worker, ports) = Worker::start(Settings::default(), Input::Synthetic).unwrap();
        tokio::time::timeout(Duration::from_secs(8), worker.ready())
            .await
            .unwrap()
            .unwrap();
        let mut nonzero = false;
        for _ in 0..20 {
            let pcm = tokio::time::timeout(Duration::from_secs(2), ports.microphone.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(pcm.len(), SAMPLES);
            nonzero |= pcm.iter().any(|s| *s != 0);
            ports.speaker.send(pcm).await.unwrap();
        }
        assert!(
            nonzero,
            "the real GStreamer capture branch conveys synthetic PCM"
        );
        worker.stop().await;
        assert!(worker.task.is_finished());
        println!(
            "Native duplex: bounded 16 kHz PCM, live synthetic GStreamer source, silent sink, DSP/probe when available, clean helper exit."
        );
    }
}
