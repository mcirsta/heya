//! Finite camera/codec helper. Only bounded H.264 and RGBA frames cross its pipes.
use crate::{photos::Pixels, voice_recorder::Input};
use std::{
    io::{Read, Write},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{oneshot, watch},
};

pub const EDGE: u32 = 960;
pub const MAX_UNIT: usize = 2 * 1024 * 1024;
const MAGIC: &[u8] = b"HEYA-CALL-VIDEO/1\n";
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Error {
    Disabled,
    NoCamera,
    Permission,
    Codec,
    Device,
    Unresponsive,
}
impl Error {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Video calling is unavailable in this build.",
            Self::NoCamera => "No camera was found. Connect a camera and try again.",
            Self::Permission => {
                "Camera access was denied. Allow Heya to use your camera in system settings."
            }
            Self::Codec => "The H.264 video encoder or decoder is unavailable.",
            Self::Device => "The camera or video decoder stopped. Check your camera and try again.",
            Self::Unresponsive => {
                "The camera or video codec is not responding. Check your camera and try again."
            }
        }
    }
}
#[derive(Clone)]
pub struct Encoded {
    pub data: Vec<u8>,
    pub timestamp: u32,
    pub orientation: u8,
    pub keyframe: bool,
}
#[derive(Clone, Default)]
pub struct Frames {
    pub local: Option<Arc<Pixels>>,
    pub remote: Option<Arc<Pixels>>,
    pub camera_on: bool,
    pub peer_on: bool,
    pub local_number: i32,
    pub remote_number: i32,
}
#[derive(Clone)]
pub struct Ports {
    pub capture: async_channel::Receiver<Encoded>,
    pub playout: async_channel::Sender<Encoded>,
    pub peer_keyframes: async_channel::Receiver<()>,
    controls: watch::Sender<Control>,
}
impl Ports {
    pub fn keyframe(&self) {
        self.controls
            .send_modify(|c| c.keyframe = c.keyframe.wrapping_add(1));
    }
}
#[derive(Clone, Default)]
struct Control {
    stop: bool,
    camera: bool,
    keyframe: u64,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Header {
    synthetic: bool,
    unavailable: bool,
    native: bool,
    #[serde(default)]
    stalled: bool,
}
pub struct Worker {
    controls: watch::Sender<Control>,
    frames: watch::Sender<Frames>,
    ready: Option<oneshot::Receiver<Result<(), Error>>>,
    task: tokio::task::JoinHandle<Result<(), Error>>,
    done: bool,
}
impl Worker {
    pub fn start(input: Input, frames: watch::Sender<Frames>) -> Result<(Self, Ports), Error> {
        if matches!(input, Input::Microphone) && !crate::video::SUPPORTED {
            return Err(Error::Disabled);
        }
        frames.send_replace(Frames {
            peer_on: true,
            ..Default::default()
        });
        let (controls, commands) = watch::channel(Control {
            camera: true,
            ..Default::default()
        });
        let (ready, prepared) = oneshot::channel();
        let (captured, capture) = async_channel::bounded(2);
        let (playout, received) = async_channel::bounded(2);
        let (requests, peer_keyframes) = async_channel::bounded(1);
        let header = Header {
            synthetic: matches!(input, Input::Synthetic),
            unavailable: matches!(input, Input::Unavailable),
            native: !cfg!(test) || std::env::var_os("HEYA_TEST_CALL_VIDEO_NATIVE").is_some(),
            #[cfg(test)]
            stalled: matches!(input, Input::Stalled),
            #[cfg(not(test))]
            stalled: false,
        };
        let task = tokio::spawn(pump(
            header,
            commands,
            frames.clone(),
            ready,
            (captured, capture.clone()),
            received,
            requests,
        ));
        let ports = Ports {
            capture,
            playout,
            peer_keyframes,
            controls: controls.clone(),
        };
        Ok((
            Self {
                controls,
                frames,
                ready: Some(prepared),
                task,
                done: false,
            },
            ports,
        ))
    }
    pub fn ready(&mut self) -> impl std::future::Future<Output = Result<(), Error>> + Send + use<> {
        let ready = self.ready.take();
        async move {
            ready
                .ok_or(Error::Device)?
                .await
                .map_err(|_| Error::Device)?
        }
    }
    pub fn camera(
        &self,
        enabled: bool,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send + use<> {
        let mut frames = self.frames.subscribe();
        self.controls.send_modify(|c| c.camera = enabled);
        async move {
            loop {
                if frames.borrow_and_update().camera_on == enabled {
                    return Ok(());
                }
                frames.changed().await.map_err(|_| Error::Device)?;
            }
        }
    }
    pub fn keyframe(&self) {
        self.controls
            .send_modify(|c| c.keyframe = c.keyframe.wrapping_add(1));
    }
    pub fn peer_camera(&self, enabled: bool) {
        self.frames.send_modify(|f| {
            f.peer_on = enabled;
            if !enabled {
                f.remote = None;
            }
        });
    }
    pub async fn finished(&mut self) -> Result<(), Error> {
        let result = (&mut self.task).await.map_err(|_| Error::Device)?;
        self.done = true;
        result
    }
    pub async fn stop(&mut self) {
        if !self.done {
            self.controls.send_modify(|c| c.stop = true);
            if tokio::time::timeout(Duration::from_secs(3), &mut self.task)
                .await
                .is_err()
            {
                self.task.abort();
                let _ = (&mut self.task).await;
            }
            self.done = true;
        }
        self.frames.send_replace(Frames::default());
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.controls.send_modify(|c| c.stop = true);
    }
}

async fn pump(
    header: Header,
    mut controls: watch::Receiver<Control>,
    frames: watch::Sender<Frames>,
    ready: oneshot::Sender<Result<(), Error>>,
    (captured, trim): (
        async_channel::Sender<Encoded>,
        async_channel::Receiver<Encoded>,
    ),
    received: async_channel::Receiver<Encoded>,
    requests: async_channel::Sender<()>,
) -> Result<(), Error> {
    let mut command =
        tokio::process::Command::new(std::env::current_exe().map_err(|_| Error::Device)?);
    #[cfg(not(test))]
    command.arg("--heya-call-video-worker");
    #[cfg(test)]
    command
        .args([
            "--exact",
            "call_video::tests::child",
            "--ignored",
            "--nocapture",
        ])
        .env("HEYA_TEST_CALL_VIDEO_CHILD", "1");
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::Device)?;
    let result = async {
        let mut input = child.stdin.take().ok_or(Error::Device)?;
        let mut output = child.stdout.take().ok_or(Error::Device)?;
        let json = serde_json::to_vec(&header).map_err(|_| Error::Device)?;
        input.write_u32_le(json.len() as u32).await.map_err(|_| Error::Device)?;
        input.write_all(&json).await.map_err(|_| Error::Device)?;
        let read = async {
            let mut matched = 0;
            for _ in 0..256 {
                let b = output.read_u8().await.map_err(|_| Error::Device)?;
                matched = if b == MAGIC[matched] {matched + 1} else {usize::from(b == MAGIC[0])};
                if matched == MAGIC.len() {break;}
            }
            if matched != MAGIC.len() {return Err(Error::Device);}
            let mut ready = Some(ready);
            let mut last_capture = tokio::time::Instant::now();
            loop {
                let packet = async {
                    match output.read_u8().await.map_err(|_| Error::Device)? {
                        1 => {
                            let timestamp = output.read_u32_le().await.map_err(|_| Error::Device)?;
                            let len = output.read_u32_le().await.map_err(|_| Error::Device)? as usize;
                            if len == 0 || len > MAX_UNIT {return Err(Error::Device);}
                            let mut data = vec![0; len];
                            output.read_exact(&mut data).await.map_err(|_| Error::Device)?;
                            let keyframe = data.windows(4).any(|w| w[..3] == [0,0,1] && w[3] & 31 == 5);
                            let packet = Encoded {data, timestamp, orientation: 0, keyframe};
                            if let Err(async_channel::TrySendError::Full(packet)) = captured.try_send(packet) {
                                let _ = trim.try_recv(); let _ = captured.try_send(packet);
                            }
                            last_capture = tokio::time::Instant::now();
                        },
                        tag @ (2 | 3) => {
                            let width = output.read_u32_le().await.map_err(|_| Error::Device)?;
                            let height = output.read_u32_le().await.map_err(|_| Error::Device)?;
                            if width == 0 || height == 0 || width > EDGE || height > EDGE {return Err(Error::Device);}
                            let mut rgba = vec![0; (width * height * 4) as usize];
                            output.read_exact(&mut rgba).await.map_err(|_| Error::Device)?;
                            let pixels = Arc::new(Pixels {width, height, rgba});
                            frames.send_modify(|f| {
                                if tag == 2 {f.local = Some(pixels); f.local_number = f.local_number.wrapping_add(1);} else if f.peer_on {f.remote = Some(pixels); f.remote_number = f.remote_number.wrapping_add(1);}
                            });
                        },
                        4 => {
                            let on = output.read_u8().await.map_err(|_| Error::Device)? != 0;
                            frames.send_modify(|f| {f.camera_on = on; if !on {f.local = None;}});
                            if on && let Some(ready) = ready.take() {let _ = ready.send(Ok(()));}
                            last_capture = tokio::time::Instant::now();
                        },
                        5 => {
                            let error = decode_error(output.read_u8().await.map_err(|_| Error::Device)?);
                            if let Some(ready) = ready.take() {let _ = ready.send(Err(error));}
                            return Err(error);
                        },
                        6 => {},
                        7 => {let _ = requests.try_send(());},
                        _ => return Err(Error::Device),
                    }
                    Ok(())
                };
                tokio::time::timeout(Duration::from_secs(5), packet).await.map_err(|_| Error::Unresponsive)??;
                if frames.borrow().camera_on && last_capture.elapsed() > Duration::from_secs(5) {return Err(Error::Unresponsive);}
            }
        };
        tokio::pin!(read);
        let mut keyframe = 0;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                biased;
                changed = controls.changed() => {
                    if changed.is_err() {break Ok(());}
                    let control = controls.borrow_and_update().clone();
                    if control.stop {break Ok(());}
                    input.write_u8(2).await.map_err(|_| Error::Device)?;
                    input.write_u8(u8::from(control.camera)).await.map_err(|_| Error::Device)?;
                    if keyframe != control.keyframe {
                        keyframe = control.keyframe;
                        input.write_u8(3).await.map_err(|_| Error::Device)?;
                    }
                },
                result = &mut read => break result,
                packet = received.recv() => {
                    let packet = packet.map_err(|_| Error::Device)?;
                    if packet.data.is_empty() || packet.data.len() > MAX_UNIT || packet.orientation > 3 {break Err(Error::Device);}
                    input.write_u8(1).await.map_err(|_| Error::Device)?;
                    input.write_u8(packet.orientation).await.map_err(|_| Error::Device)?;
                    input.write_u32_le(packet.data.len() as u32).await.map_err(|_| Error::Device)?;
                    input.write_all(&packet.data).await.map_err(|_| Error::Device)?;
                },
                _ = heartbeat.tick() => {
                    // A finite helper replies even when the local camera is off.
                    input.write_u8(4).await.map_err(|_| Error::Device)?;
                },
            }
        }
    }.await;
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) if status.success() => result,
        _ => {
            let _ = child.kill().await;
            result.and(Err(Error::Device))
        }
    }
}

fn decode_error(code: u8) -> Error {
    match code {
        0 => Error::Disabled,
        1 => Error::NoCamera,
        2 => Error::Permission,
        3 => Error::Codec,
        5 => Error::Unresponsive,
        _ => Error::Device,
    }
}
fn error_code(error: Error) -> u8 {
    match error {
        Error::Disabled => 0,
        Error::NoCamera => 1,
        Error::Permission => 2,
        Error::Codec => 3,
        Error::Device => 4,
        Error::Unresponsive => 5,
    }
}
#[cfg_attr(
    all(target_os = "linux", not(feature = "media-gstreamer")),
    allow(dead_code)
)]
pub enum Packet {
    Encoded(Vec<u8>, u32),
    Local(Pixels),
    Remote(Pixels),
    NeedKeyframe,
}
pub trait Device {
    fn poll(&mut self) -> Result<Vec<Packet>, Error>;
    fn receive(&mut self, data: &[u8], orientation: u8) -> Result<(), Error>;
    fn camera(&mut self, enabled: bool) -> Result<(), Error>;
    fn keyframe(&mut self);
}
enum Command {
    Receive(Vec<u8>, u8),
    Camera(bool),
    Keyframe,
    Ping,
}
pub fn child_main() -> ! {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(MAGIC).and_then(|_| out.flush());
    if let Err(error) = run_child(&mut out) {
        let _ = out
            .write_all(&[5, error_code(error)])
            .and_then(|_| out.flush());
    }
    std::process::exit(0);
}
fn run_child(out: &mut impl Write) -> Result<(), Error> {
    let mut input = std::io::stdin();
    let mut length = [0; 4];
    input.read_exact(&mut length).map_err(|_| Error::Device)?;
    let len = u32::from_le_bytes(length) as usize;
    if len == 0 || len > 8192 {
        return Err(Error::Device);
    }
    let mut json = vec![0; len];
    input.read_exact(&mut json).map_err(|_| Error::Device)?;
    let header: Header = serde_json::from_slice(&json).map_err(|_| Error::Device)?;
    if header.unavailable {
        return Err(Error::NoCamera);
    }
    let (sender, commands) = std::sync::mpsc::sync_channel(4);
    let (desired, mut control) = watch::channel(Control {
        camera: true,
        ..Default::default()
    });
    let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ping = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (lost_writer, ping_writer) = (lost.clone(), ping.clone());
    std::thread::Builder::new()
        .name("heya-camera-pipe".into())
        .spawn(move || {
            let receive = || -> Option<Command> {
                let mut kind = [0];
                input.read_exact(&mut kind).ok()?;
                Some(match kind[0] {
                    1 => {
                        let mut orientation = [0];
                        input.read_exact(&mut orientation).ok()?;
                        input.read_exact(&mut length).ok()?;
                        let len = u32::from_le_bytes(length) as usize;
                        if len == 0 || len > MAX_UNIT || orientation[0] > 3 {
                            return None;
                        }
                        let mut data = vec![0; len];
                        input.read_exact(&mut data).ok()?;
                        Command::Receive(data, orientation[0])
                    }
                    2 => {
                        let mut on = [0];
                        input.read_exact(&mut on).ok()?;
                        Command::Camera(on[0] != 0)
                    }
                    3 => Command::Keyframe,
                    4 => Command::Ping,
                    _ => return None,
                })
            };
            let mut receive = receive;
            loop {
                let Some(command) = receive() else {
                    std::process::exit(0)
                };
                // Keep controls and pipe closure independent of a stalled decoder.
                match command {
                    Command::Camera(on) => {
                        desired.send_modify(|c| c.camera = on);
                    }
                    Command::Keyframe => {
                        desired.send_modify(|c| c.keyframe = c.keyframe.wrapping_add(1));
                    }
                    Command::Ping => {
                        ping_writer.store(true, std::sync::atomic::Ordering::Release);
                    }
                    command => {
                        if sender.try_send(command).is_err() {
                            lost_writer.store(true, std::sync::atomic::Ordering::Release);
                        }
                    }
                }
            }
        })
        .map_err(|_| Error::Device)?;
    if header.stalled {
        loop {
            std::thread::park();
        }
    }
    let mut device: Box<dyn Device> = if header.native {
        platform(header.synthetic)?
    } else {
        Box::new(Synthetic::default())
    };
    let mut on = true;
    let mut announce = true;
    let mut keyframe = 0;
    let mut probe_started = false;
    let mut probed = false;
    let mut peer_received = false;
    loop {
        let desired = control.borrow_and_update().clone();
        if on != desired.camera {
            device.camera(desired.camera)?;
            on = desired.camera;
            announce = on;
            if !on {
                out.write_all(&[4, 0]).map_err(|_| Error::Device)?;
            }
        }
        if keyframe != desired.keyframe {
            keyframe = desired.keyframe;
            device.keyframe();
        }
        if ping.swap(false, std::sync::atomic::Ordering::AcqRel) {
            out.write_all(&[6]).map_err(|_| Error::Device)?;
        }
        if lost.swap(false, std::sync::atomic::Ordering::AcqRel) {
            out.write_all(&[7]).map_err(|_| Error::Device)?;
        }
        for command in commands.try_iter().take(4) {
            if let Command::Receive(data, orientation) = command {
                peer_received = true;
                device.receive(&data, orientation)?;
            }
        }
        for packet in device.poll()? {
            match packet {
                Packet::Encoded(data, timestamp) if on => {
                    if data.is_empty() || data.len() > MAX_UNIT {
                        return Err(Error::Device);
                    }
                    if !probe_started {
                        device.receive(&data, 0)?;
                        probe_started = true;
                    }
                    if announce && probed {
                        out.write_all(&[4, 1]).map_err(|_| Error::Device)?;
                        announce = false;
                    }
                    out.write_all(&[1])
                        .and_then(|_| out.write_all(&timestamp.to_le_bytes()))
                        .and_then(|_| out.write_all(&(data.len() as u32).to_le_bytes()))
                        .and_then(|_| out.write_all(&data))
                        .map_err(|_| Error::Device)?;
                }
                Packet::Local(pixels) if on => emit(out, 2, pixels)?,
                Packet::Remote(pixels) => {
                    if !probed {
                        probed = true;
                        if announce {
                            out.write_all(&[4, 1]).map_err(|_| Error::Device)?;
                            announce = false;
                        }
                    } else if peer_received {
                        emit(out, 3, pixels)?;
                    }
                }
                Packet::NeedKeyframe => {
                    out.write_all(&[7]).map_err(|_| Error::Device)?;
                }
                _ => {}
            }
        }
        out.flush().map_err(|_| Error::Device)?;
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn emit(out: &mut impl Write, tag: u8, pixels: Pixels) -> Result<(), Error> {
    if pixels.width == 0
        || pixels.height == 0
        || pixels.width > EDGE
        || pixels.height > EDGE
        || pixels.rgba.len() != (pixels.width * pixels.height * 4) as usize
    {
        return Err(Error::Device);
    }
    out.write_all(&[tag])
        .and_then(|_| out.write_all(&pixels.width.to_le_bytes()))
        .and_then(|_| out.write_all(&pixels.height.to_le_bytes()))
        .and_then(|_| out.write_all(&pixels.rgba))
        .map_err(|_| Error::Device)
}
#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", feature = "media-gstreamer")
))]
pub fn rotate(pixels: Pixels, orientation: u8) -> Pixels {
    if orientation == 0 {
        return pixels;
    }
    let image = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.rgba).unwrap();
    let image = match orientation {
        1 => image::imageops::rotate90(&image),
        2 => image::imageops::rotate180(&image),
        _ => image::imageops::rotate270(&image),
    };
    Pixels {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    }
}
fn platform(synthetic: bool) -> Result<Box<dyn Device>, Error> {
    #[cfg(all(target_os = "linux", feature = "media-gstreamer"))]
    {
        Ok(Box::new(crate::gstreamer_call_video::Video::new(
            synthetic,
        )?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(crate::macos_call_video::Video::new(synthetic)?))
    }
    #[cfg(not(any(
        target_os = "macos",
        all(target_os = "linux", feature = "media-gstreamer")
    )))]
    {
        let _ = synthetic;
        Err(Error::Disabled)
    }
}
#[derive(Default)]
struct Synthetic {
    tick: u32,
    on: bool,
    remote: bool,
}
impl Device for Synthetic {
    fn poll(&mut self) -> Result<Vec<Packet>, Error> {
        self.tick += 1;
        let mut packets = Vec::new();
        if self.tick % 7 == 1 {
            if self.on || self.tick == 1 {
                self.on = true;
                packets.push(Packet::Encoded(vec![0, 0, 0, 1, 0x65, 1], self.tick * 900));
                packets.push(Packet::Local(Pixels {
                    width: 64,
                    height: 48,
                    rgba: vec![150; 64 * 48 * 4],
                }));
            }
            if self.remote {
                packets.push(Packet::Remote(Pixels {
                    width: 64,
                    height: 48,
                    rgba: vec![120; 64 * 48 * 4],
                }));
                self.remote = false;
            }
        }
        Ok(packets)
    }
    fn receive(&mut self, _: &[u8], _: u8) -> Result<(), Error> {
        self.remote = true;
        Ok(())
    }
    fn camera(&mut self, enabled: bool) -> Result<(), Error> {
        self.on = enabled;
        Ok(())
    }
    fn keyframe(&mut self) {}
}

#[cfg(test)]
#[path = "call_video_tests.rs"]
mod tests;
