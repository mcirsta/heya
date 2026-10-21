//! Finite decoder process. Bounded binary frames over pipes; no media file paths.
use crate::{
    photos::Pixels,
    video::{self, Error, Status},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

const MAGIC: &[u8] = b"HEYA-VIDEO/1\n";
#[derive(Clone, Serialize, Deserialize)]
pub struct Control {
    pub playing: bool,
    pub muted: bool,
    pub seek: Option<(u64, f64)>,
    pub stop: bool,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            playing: true,
            muted: false,
            seek: None,
            stop: false,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub struct Header {
    pub demo: bool,
    pub silent: bool,
    pub settings: crate::audio_devices::Settings,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    pub phase: i32,
    pub position: f64,
    pub duration: f64,
    pub muted: bool,
    pub error: Option<Error>,
}
pub struct Worker {
    control: watch::Sender<Control>,
    task: tokio::task::JoinHandle<Result<(), Error>>,
}
impl Worker {
    pub fn start(
        id: i32,
        bytes: Vec<u8>,
        demo: bool,
        silent: bool,
        settings: crate::audio_devices::Settings,
        updates: watch::Sender<Status>,
    ) -> Result<Self, Error> {
        if (!demo && bytes.is_empty()) || bytes.len() > video::MAX_BYTES {
            return Err(Error::TooLarge);
        }
        let (control, commands) = watch::channel(Control::default());
        Ok(Self {
            control,
            task: tokio::spawn(pump(
                id,
                bytes,
                Header {
                    demo,
                    silent,
                    settings,
                },
                commands,
                updates,
            )),
        })
    }
    pub fn set(&self, control: &Control) {
        self.control.send_replace(control.clone());
    }
    pub async fn finished(&mut self) -> Result<(), Error> {
        (&mut self.task).await.map_err(|_| Error::Playback)?
    }
    pub async fn stop(&mut self) {
        self.control.send_modify(|c| c.stop = true);
        if tokio::time::timeout(Duration::from_secs(4), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
            let _ = (&mut self.task).await;
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.control.send_modify(|c| c.stop = true);
    }
}

async fn pump(
    id: i32,
    bytes: Vec<u8>,
    header: Header,
    mut controls: watch::Receiver<Control>,
    updates: watch::Sender<Status>,
) -> Result<(), Error> {
    let mut command =
        tokio::process::Command::new(std::env::current_exe().map_err(|_| Error::Playback)?);
    #[cfg(not(test))]
    command.arg("--heya-video-worker");
    #[cfg(test)]
    command
        .args([
            "--exact",
            "video_process::tests::child",
            "--ignored",
            "--nocapture",
        ])
        .env("HEYA_TEST_VIDEO_CHILD", "1");
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::Playback)?;
    let mut input = child.stdin.take().ok_or(Error::Playback)?;
    let output = child.stdout.take().ok_or(Error::Playback)?;
    let json = serde_json::to_vec(&header).map_err(|_| Error::Playback)?;
    input
        .write_u32_le(json.len() as u32)
        .await
        .map_err(|_| Error::Playback)?;
    input.write_all(&json).await.map_err(|_| Error::Playback)?;
    input
        .write_u32_le(bytes.len() as u32)
        .await
        .map_err(|_| Error::Playback)?;
    input.write_all(&bytes).await.map_err(|_| Error::Playback)?;
    drop(bytes);
    let read = read_output(output, id, updates);
    tokio::pin!(read);
    let mut deadline = None;
    let mut controls_open = true;
    let result = loop {
        tokio::select! {
            result=&mut read=>break result,
            changed=controls.changed(),if controls_open=>{
                controls_open=changed.is_ok();
                let control=if controls_open {controls.borrow_and_update().clone()} else {Control {stop:true,..Default::default()}};
                let mut json=serde_json::to_vec(&control).map_err(|_|Error::Playback)?;json.push(b'\n');
                if input.write_all(&json).await.is_err() {break Err(Error::Playback);}
                if control.stop {deadline=Some(tokio::time::Instant::now()+Duration::from_secs(2));}
            }
            _=async {match deadline {Some(d)=>tokio::time::sleep_until(d).await,None=>std::future::pending().await}}=>break Err(Error::Playback),
        }
    };
    drop(input);
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) if status.success() || result.is_err() => result,
        _ => {
            let _ = child.kill().await;
            Err(Error::Playback)
        }
    }
}

async fn read_output(
    mut input: impl AsyncRead + Unpin,
    id: i32,
    updates: watch::Sender<Status>,
) -> Result<(), Error> {
    let mut matched = 0;
    for _ in 0..256 {
        let byte = input.read_u8().await.map_err(|_| Error::Playback)?;
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
        return Err(Error::Playback);
    }
    let mut status = Status {
        id,
        phase: 1,
        ..Default::default()
    };
    loop {
        match input.read_u8().await.map_err(|_| Error::Playback)? {
            0 => {
                let len = input.read_u32_le().await.map_err(|_| Error::Playback)? as usize;
                if len > 4096 {
                    return Err(Error::Playback);
                }
                let mut data = vec![0; len];
                input
                    .read_exact(&mut data)
                    .await
                    .map_err(|_| Error::Playback)?;
                let p: Progress = serde_json::from_slice(&data).map_err(|_| Error::Playback)?;
                if !p.position.is_finite()
                    || !p.duration.is_finite()
                    || !(0.0..=video::MAX_SECONDS).contains(&p.position)
                    || !(0.0..=video::MAX_SECONDS).contains(&p.duration)
                    || !(0..=4).contains(&p.phase)
                {
                    return Err(Error::Playback);
                }
                status.phase = p.phase;
                status.position = p.position;
                status.duration = p.duration;
                status.muted = p.muted;
                status.error = p.error;
                updates.send_replace(status.clone());
                if let Some(error) = status.error {
                    return Err(error);
                }
            }
            1 => {
                let width = input.read_u32_le().await.map_err(|_| Error::Playback)?;
                let height = input.read_u32_le().await.map_err(|_| Error::Playback)?;
                if width == 0 || height == 0 || width > video::EDGE || height > video::EDGE {
                    return Err(Error::Playback);
                }
                let mut rgba = vec![0; (width * height * 4) as usize];
                input
                    .read_exact(&mut rgba)
                    .await
                    .map_err(|_| Error::Playback)?;
                status.frame = Some(Arc::new(Pixels {
                    width,
                    height,
                    rgba,
                }));
                status.frame_number += 1;
                updates.send_replace(status.clone());
            }
            2 => return Ok(()),
            _ => return Err(Error::Playback),
        }
    }
}

/// The child writes on one thread. There are no log messages on this pipe.
pub struct Emitter<W: Write>(pub W);
impl<W: Write> Emitter<W> {
    pub fn progress(&mut self, p: &Progress) -> Result<(), Error> {
        let json = serde_json::to_vec(p).map_err(|_| Error::Playback)?;
        self.0
            .write_all(&[0])
            .and_then(|_| self.0.write_all(&(json.len() as u32).to_le_bytes()))
            .and_then(|_| self.0.write_all(&json))
            .and_then(|_| self.0.flush())
            .map_err(|_| Error::Playback)
    }
    pub fn frame(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<(), Error> {
        if width == 0
            || height == 0
            || width > video::EDGE
            || height > video::EDGE
            || rgba.len() != (width * height * 4) as usize
        {
            return Err(Error::Playback);
        }
        self.0
            .write_all(&[1])
            .and_then(|_| self.0.write_all(&width.to_le_bytes()))
            .and_then(|_| self.0.write_all(&height.to_le_bytes()))
            .and_then(|_| self.0.write_all(rgba))
            .and_then(|_| self.0.flush())
            .map_err(|_| Error::Playback)
    }
}

pub fn child() -> Result<(), Error> {
    use std::io::BufRead;
    let mut input = std::io::stdin().lock();
    let mut word = [0; 4];
    input.read_exact(&mut word).map_err(|_| Error::Playback)?;
    let len = u32::from_le_bytes(word) as usize;
    if len > 16 * 1024 {
        return Err(Error::Playback);
    }
    let mut json = vec![0; len];
    input.read_exact(&mut json).map_err(|_| Error::Playback)?;
    let header: Header = serde_json::from_slice(&json).map_err(|_| Error::Playback)?;
    input.read_exact(&mut word).map_err(|_| Error::Playback)?;
    let len = u32::from_le_bytes(word) as usize;
    if len > video::MAX_BYTES {
        return Err(Error::TooLarge);
    }
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes).map_err(|_| Error::Playback)?;
    drop(input);
    let (sender, mut control) = watch::channel(Control::default());
    std::thread::Builder::new()
        .name("heya-video-control".into())
        .spawn(move || {
            let mut input = std::io::stdin().lock();
            loop {
                let mut json = Vec::new();
                if input
                    .by_ref()
                    .take(4097)
                    .read_until(b'\n', &mut json)
                    .ok()
                    .is_none_or(|n| n == 0 || n > 4096)
                {
                    break;
                }
                let Ok(next) = serde_json::from_slice::<Control>(&json) else {
                    break;
                };
                if next.seek.is_some_and(|(_, t)| {
                    !t.is_finite() || !(0.0..=video::MAX_SECONDS).contains(&t)
                }) {
                    break;
                }
                let stop = next.stop;
                sender.send_replace(next);
                if stop {
                    return;
                }
            }
            sender.send_modify(|c| c.stop = true);
        })
        .map_err(|_| Error::Playback)?;
    let mut output = Emitter(std::io::stdout().lock());
    output.0.write_all(MAGIC).map_err(|_| Error::Playback)?;
    let result = if header.demo {
        synthetic(&mut control, &mut output)
    } else {
        platform(bytes, header, &mut control, &mut output)
    };
    if let Err(error) = result {
        let _ = output.progress(&Progress {
            phase: 4,
            error: Some(error),
            ..Default::default()
        });
    }
    output
        .0
        .write_all(&[2])
        .and_then(|_| output.0.flush())
        .map_err(|_| Error::Playback)?;
    result
}

fn platform(
    bytes: Vec<u8>,
    header: Header,
    control: &mut watch::Receiver<Control>,
    output: &mut Emitter<impl Write>,
) -> Result<(), Error> {
    #[cfg(all(target_os = "linux", feature = "media-gstreamer"))]
    {
        crate::gstreamer_video::play(bytes, header, control, output)
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos_video::play(bytes, header, control, output)
    }
    #[cfg(not(any(
        target_os = "macos",
        all(target_os = "linux", feature = "media-gstreamer")
    )))]
    {
        let _ = (bytes, header, control, output);
        Err(Error::NotBuilt)
    }
}

fn synthetic(
    control: &mut watch::Receiver<Control>,
    output: &mut Emitter<impl Write>,
) -> Result<(), Error> {
    let mut position = 0.0;
    let mut last = Instant::now();
    let mut seek = None;
    let mut rendered = -1.0_f64;
    loop {
        let c = control.borrow_and_update().clone();
        if c.stop || control.has_changed().is_err() {
            return Ok(());
        }
        if c.seek != seek {
            if let Some((_, t)) = c.seek {
                position = t.min(5.);
            }
            seek = c.seek;
        }
        if c.playing {
            position += last.elapsed().as_secs_f64();
        }
        last = Instant::now();
        output.progress(&Progress {
            phase: if c.playing { 2 } else { 3 },
            position: position.min(5.),
            duration: 5.,
            muted: c.muted,
            error: None,
        })?;
        if (position - rendered).abs() > 0.02 {
            let frame = crate::fixtures::video_pixels(position);
            output.frame(frame.width, frame.height, &frame.rgba)?;
            rendered = position;
        }
        if position >= 5. {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

#[cfg(test)]
#[path = "video_process_tests.rs"]
mod tests;
