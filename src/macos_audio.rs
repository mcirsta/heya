//! Apple audio objects stay on their owning worker; only bounded buffers cross FFI.
//! Capture/encoding live in the disposable media process, never on the UI thread.
#![allow(unsafe_code)]

use crate::{
    audio::{Error as PlaybackError, Status},
    audio_devices::{Device, Settings},
    audio_player::{Desired as PlaybackDesired, Output},
    voice_recorder::{Clip, Desired, Error, Input, MAX_BYTES, MAX_SECONDS, Progress},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, c_char, c_void},
    io::{Cursor, Read, Write},
    ptr::NonNull,
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

unsafe extern "C" {
    fn heya_native_free(bytes: *mut c_void);
    fn heya_audio_devices(output: *mut *mut u8, length: *mut usize) -> i32;
    fn heya_audio_player_new(
        bytes: *const u8,
        length: usize,
        uid: *const c_char,
        volume: f32,
        silent: i32,
        duration: *mut f64,
        error: *mut i32,
    ) -> *mut c_void;
    fn heya_audio_player_control(
        handle: *mut c_void,
        playing: i32,
        position: f64,
        speed: f32,
    ) -> i32;
    fn heya_audio_player_position(handle: *mut c_void, position: *mut f64) -> i32;
    fn heya_audio_player_free(handle: *mut c_void);
    fn heya_audio_encoder_new(rate: f64, channels: u32) -> *mut c_void;
    fn heya_audio_encoder_write(handle: *mut c_void, samples: *const f32, frames: u32) -> i32;
    fn heya_audio_encoder_finish(
        handle: *mut c_void,
        output: *mut *mut u8,
        length: *mut usize,
    ) -> i32;
    fn heya_audio_encoder_free(handle: *mut c_void);
    fn heya_audio_capture_new(
        uid: *const c_char,
        rate: *mut f64,
        channels: *mut u32,
        error: *mut i32,
    ) -> *mut c_void;
    fn heya_audio_capture_control(handle: *mut c_void, command: i32) -> i32;
    fn heya_audio_capture_read(
        handle: *mut c_void,
        samples: *mut f32,
        capacity: u32,
        count: *mut u32,
    ) -> i32;
    fn heya_audio_capture_free(handle: *mut c_void);
}

struct Native {
    pointer: NonNull<c_void>,
    free: unsafe extern "C" fn(*mut c_void),
}
impl Native {
    fn new(pointer: *mut c_void, free: unsafe extern "C" fn(*mut c_void)) -> Option<Self> {
        Some(Self {
            pointer: NonNull::new(pointer)?,
            free,
        })
    }
    fn pointer(&self) -> *mut c_void {
        self.pointer.as_ptr()
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        // Every handle was retained by its constructor and is released exactly
        // once, on the same worker, after all synchronous FFI calls return.
        unsafe { (self.free)(self.pointer()) };
    }
}

fn native_bytes(
    limit: usize,
    call: impl FnOnce(*mut *mut u8, *mut usize) -> i32,
) -> Result<Vec<u8>, Error> {
    let mut pointer = std::ptr::null_mut();
    let mut length = 0;
    let ok = call(&mut pointer, &mut length);
    let result = if ok == 0 || pointer.is_null() || length > limit {
        Err(Error::Codec)
    } else {
        // The bridge allocated `length` bytes and retains them until free below.
        Ok(unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec())
    };
    if !pointer.is_null() {
        unsafe { heya_native_free(pointer.cast()) };
    }
    result
}

fn raw_devices() -> Result<Vec<Device>, Error> {
    let bytes = native_bytes(512 * 1024, |out, len| unsafe {
        heya_audio_devices(out, len)
    })?;
    let devices: Vec<Device> = serde_json::from_slice(&bytes).map_err(|_| Error::Device)?;
    if devices.len() > 128
        || devices
            .iter()
            .any(|d| d.id.len() > 4096 || d.name.len() > 4096)
    {
        return Err(Error::Device);
    }
    Ok(devices)
}
fn device_id(device: &Device) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("macos:{}:{}", device.input, device.id))
    )
}
pub fn devices() -> Result<Vec<Device>, Error> {
    Ok(raw_devices()?
        .into_iter()
        .map(|d| Device {
            id: device_id(&d),
            name: d
                .name
                .chars()
                .filter(|c| !c.is_control())
                .take(240)
                .collect(),
            input: d.input,
        })
        .collect())
}
fn selected_uid(input: bool, id: &str) -> Result<CString, Error> {
    if id.is_empty() {
        return Ok(CString::default());
    }
    let device = raw_devices()?
        .into_iter()
        .find(|d| d.input == input && device_id(d) == id)
        .ok_or(if input {
            Error::NoInput
        } else {
            Error::Playback
        })?;
    CString::new(device.id).map_err(|_| Error::Device)
}

pub fn play(
    mut desired: watch::Receiver<PlaybackDesired>,
    updates: watch::Sender<Status>,
    output: Output,
) -> Status {
    let initial = desired.borrow_and_update().clone();
    let mut status = Status {
        generation: initial.generation,
        id: initial.id,
        speed: initial.speed,
        phase: 1,
        ..Default::default()
    };
    let result = play_inner(&mut desired, &updates, output, &mut status);
    if let Err(error) = result {
        status.fail(error);
    }
    updates.send_replace(status.clone());
    status
}
fn play_inner(
    desired: &mut watch::Receiver<PlaybackDesired>,
    updates: &watch::Sender<Status>,
    output: Output,
    status: &mut Status,
) -> Result<(), PlaybackError> {
    let initial = desired.borrow().clone();
    let bytes = initial.bytes.as_ref().ok_or(PlaybackError::Unavailable)?;
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(PlaybackError::TooLarge);
    }
    let (silent, uid, volume) = match output {
        Output::Silent => (true, CString::default(), 1.),
        Output::Desktop(settings) => {
            if !settings.valid() {
                return Err(PlaybackError::Playback);
            }
            (
                false,
                selected_uid(false, &settings.output.id).map_err(|_| PlaybackError::Playback)?,
                settings.output_volume as f32 / 100.,
            )
        }
    };
    let mut error = 0;
    let pointer = unsafe {
        heya_audio_player_new(
            bytes.as_ptr(),
            bytes.len(),
            uid.as_ptr(),
            volume,
            i32::from(silent),
            &mut status.duration,
            &mut error,
        )
    };
    let player = Native::new(pointer, heya_audio_player_free).ok_or(match error {
        9 => PlaybackError::TooLong,
        5 => PlaybackError::Playback,
        _ => PlaybackError::Codec,
    })?;
    let runtime = tokio::runtime::Handle::current();
    let mut applied_playing = None;
    let mut applied_seek = None;
    let mut applied_speed = None;
    let mut last = Instant::now();
    loop {
        if desired.has_changed().is_err() {
            status.phase = 0;
            break;
        }
        let current = desired.borrow_and_update().clone();
        if current.generation != initial.generation || current.bytes.is_none() {
            status.phase = 0;
            break;
        }
        let elapsed = last.elapsed().as_secs_f64();
        last = Instant::now();
        if silent && status.phase == 2 {
            status.position += elapsed * status.speed;
        }
        if !current.speed.is_finite() || !(0.5..=2.).contains(&current.speed) {
            return Err(PlaybackError::Seek);
        }
        if applied_playing != Some(current.playing)
            || applied_seek != current.seek
            || applied_speed != Some(current.speed)
        {
            let seek_changed = applied_seek != current.seek;
            let seek = if seek_changed {
                current.seek.map(|(_, fraction)| fraction * status.duration)
            } else {
                None
            };
            if let Some(position) = seek {
                if !position.is_finite() {
                    return Err(PlaybackError::Seek);
                }
                status.position = position.clamp(0., status.duration);
            }
            if !silent
                && unsafe {
                    heya_audio_player_control(
                        player.pointer(),
                        i32::from(current.playing),
                        seek.map_or(-1., |_| status.position),
                        current.speed as f32,
                    )
                } == 0
            {
                return Err(PlaybackError::Playback);
            }
            applied_playing = Some(current.playing);
            applied_seek = current.seek;
            applied_speed = Some(current.speed);
        }
        status.speed = current.speed;
        status.phase = if current.playing { 2 } else { 3 };
        if !silent {
            match unsafe { heya_audio_player_position(player.pointer(), &mut status.position) } {
                -1 => return Err(PlaybackError::Playback),
                1 => {
                    status.phase = 0;
                    status.position = status.duration;
                }
                _ => {}
            }
        } else if status.position >= status.duration {
            status.phase = 0;
            status.position = status.duration;
        }
        updates.send_if_modified(|old| {
            if *old == *status {
                false
            } else {
                *old = status.clone();
                true
            }
        });
        if status.phase == 0 {
            break;
        }
        let playing = status.phase == 2;
        if runtime.block_on(async {
            tokio::select! {
                result = desired.changed() => result.is_err(),
                _ = tokio::time::sleep(Duration::from_millis(100)), if playing => false,
            }
        }) {
            status.phase = 0;
            break;
        }
    }
    // Release the native player before the Worker publishes completion.
    drop(player);
    Ok(())
}

pub fn capture(
    input: Input,
    settings: Settings,
    mut publish: impl FnMut(Progress),
    commands: mpsc::Receiver<Desired>,
) -> Result<Clip, Error> {
    if !settings.valid() {
        return Err(Error::Device);
    }
    if matches!(input, Input::Unavailable) {
        return Err(Error::NoInput);
    }
    let mut rate = 48000.;
    let mut channels = 1;
    let microphone = if matches!(input, Input::Microphone) {
        let uid = selected_uid(true, &settings.input.id)?;
        let mut error = 6;
        let pointer =
            unsafe { heya_audio_capture_new(uid.as_ptr(), &mut rate, &mut channels, &mut error) };
        Some(
            Native::new(pointer, heya_audio_capture_free).ok_or(match error {
                7 => Error::NoInput,
                8 => Error::Permission,
                _ => Error::Device,
            })?,
        )
    } else {
        None
    };
    let encoder = Native::new(
        unsafe { heya_audio_encoder_new(rate, channels) },
        heya_audio_encoder_free,
    )
    .ok_or(Error::Codec)?;
    if let Some(mic) = &microphone
        && unsafe { heya_audio_capture_control(mic.pointer(), 0) } == 0
    {
        return Err(Error::Device);
    }
    let mut buffer = vec![0f32; 8192 * channels as usize];
    let mut frames = 0u64;
    let maximum_frames = (rate * f64::from(MAX_SECONDS)).round() as u64;
    let mut levels: Vec<u8> = Vec::new();
    let mut paused = false;
    let mut finishing = false;
    let mut published = Instant::now();
    let mut last_input = Instant::now();
    loop {
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(Desired::Cancel) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::Cancelled);
            }
            Ok(command) => {
                if command == Desired::Finish {
                    finishing = true;
                } else {
                    paused = command == Desired::Pause;
                    if !paused {
                        last_input = Instant::now();
                    }
                }
                if let Some(mic) = &microphone {
                    let command = if finishing {
                        2
                    } else if paused {
                        1
                    } else {
                        0
                    };
                    if unsafe { heya_audio_capture_control(mic.pointer(), command) } == 0 {
                        return Err(Error::Device);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // Drain pre-pause/stop buffers too; they contain samples recorded before
        // the user's command. Synthetic input never opens a hardware source.
        loop {
            let count = if let Some(mic) = &microphone {
                let mut count = 0;
                if unsafe {
                    heya_audio_capture_read(
                        mic.pointer(),
                        buffer.as_mut_ptr(),
                        buffer.len() as u32,
                        &mut count,
                    )
                } == 0
                {
                    return Err(Error::Device);
                }
                count as usize
            } else if !paused && !finishing {
                let count = 960;
                for (index, sample) in buffer[..count].iter_mut().enumerate() {
                    let t = (frames + index as u64) as f64 / 48000.;
                    *sample = ((t * 440. * std::f64::consts::TAU).sin()
                        * (0.04 + ((t * 10.) as u64 % 10) as f64 * 0.03))
                        as f32;
                }
                count
            } else {
                0
            };
            if count == 0 {
                break;
            }
            if count > buffer.len() || count % channels as usize != 0 {
                return Err(Error::Device);
            }
            last_input = Instant::now();
            let count =
                count.min(((maximum_frames - frames).min(8192) * u64::from(channels)) as usize);
            let gain = settings.input_volume as f32 / 100.;
            for frame in buffer[..count].chunks_exact_mut(channels as usize) {
                let mut peak = 0f32;
                for sample in frame {
                    if !sample.is_finite() {
                        return Err(Error::Device);
                    }
                    *sample = (*sample * gain).clamp(-1., 1.);
                    peak = peak.max(sample.abs());
                }
                let bucket = (frames as f64 / rate * 10.) as usize;
                if bucket > MAX_SECONDS as usize * 10 {
                    return Err(Error::Limit);
                }
                if levels.len() <= bucket {
                    levels.resize(bucket + 1, 0);
                }
                levels[bucket] = levels[bucket].max((peak.sqrt() * 100.).min(100.) as u8);
                frames += 1;
            }
            if unsafe {
                heya_audio_encoder_write(
                    encoder.pointer(),
                    buffer.as_ptr(),
                    (count / channels as usize) as u32,
                )
            } == 0
            {
                return Err(Error::Codec);
            }
            if frames >= maximum_frames {
                finishing = true;
                if let Some(mic) = &microphone
                    && unsafe { heya_audio_capture_control(mic.pointer(), 2) } == 0
                {
                    return Err(Error::Device);
                }
                break;
            }
            if microphone.is_none() {
                break;
            }
        }
        if finishing {
            break;
        }
        if !paused && last_input.elapsed() > Duration::from_secs(5) {
            return Err(Error::Device);
        }
        if published.elapsed() >= Duration::from_millis(100) {
            published = Instant::now();
            let recent = &levels[levels.len().saturating_sub(64)..];
            let mut bars = vec![0; 64 - recent.len()];
            bars.extend_from_slice(recent);
            publish(Progress {
                seconds: frames as f64 / rate,
                levels: bars,
            });
        }
    }
    drop(microphone);
    if frames as f64 / rate < 0.1 {
        return Err(Error::Empty);
    }
    let encoded = native_bytes(MAX_BYTES, |out, len| unsafe {
        heya_audio_encoder_finish(encoder.pointer(), out, len)
    })?;
    drop(encoder);
    let (bytes, samples) = ogg_opus(&encoded)?;
    Ok(Clip {
        bytes: bytes.into(),
        seconds: samples.div_ceil(48000) as u32,
        waveform: crate::voice_recorder::compact(&levels),
    })
}

struct LimitedBytes(Vec<u8>);
impl Write for LimitedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_BYTES {
            return Err(std::io::Error::other("audio limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn ogg_opus(encoded: &[u8]) -> Result<(Vec<u8>, u64), Error> {
    use ogg::writing::{PacketWriteEndInfo as End, PacketWriter};
    let mut input = Cursor::new(encoded);
    let mut header = [0; 16];
    input.read_exact(&mut header).map_err(|_| Error::Codec)?;
    let samples = u64::from_le_bytes(header[..8].try_into().unwrap());
    let priming = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let count = u32::from_le_bytes(header[12..].try_into().unwrap());
    if samples == 0
        || samples > u64::from(MAX_SECONDS) * 48000
        || priming > u32::from(u16::MAX)
        || count == 0
        || count > MAX_SECONDS * 50 + 32
        || samples + u64::from(priming) > u64::from(count) * 960
    {
        return Err(Error::Codec);
    }
    let mut serial = [0; 4];
    getrandom::fill(&mut serial).map_err(|_| Error::Codec)?;
    let serial = u32::from_le_bytes(serial);
    let mut writer = PacketWriter::new(LimitedBytes(Vec::new()));
    let mut head = b"OpusHead".to_vec();
    head.extend([1, 1]);
    head.extend((priming as u16).to_le_bytes());
    head.extend(48000u32.to_le_bytes());
    head.extend([0, 0, 0]);
    writer
        .write_packet(head, serial, End::EndPage, 0)
        .map_err(|_| Error::Limit)?;
    let vendor = b"Heya native audio";
    let mut tags = b"OpusTags".to_vec();
    tags.extend((vendor.len() as u32).to_le_bytes());
    tags.extend(vendor);
    tags.extend(0u32.to_le_bytes());
    writer
        .write_packet(tags, serial, End::EndPage, 0)
        .map_err(|_| Error::Limit)?;
    for index in 0..count {
        let mut length = [0; 4];
        input.read_exact(&mut length).map_err(|_| Error::Codec)?;
        let length = u32::from_le_bytes(length) as usize;
        if length == 0
            || length > 65536
            || length > encoded.len().saturating_sub(input.position() as usize)
        {
            return Err(Error::Codec);
        }
        let mut packet = vec![0; length];
        input.read_exact(&mut packet).map_err(|_| Error::Codec)?;
        let last = index + 1 == count;
        let granule = if last {
            samples + u64::from(priming)
        } else {
            u64::from(index + 1) * 960
        };
        let end = if last {
            End::EndStream
        } else if (index + 1) % 50 == 0 {
            End::EndPage
        } else {
            End::NormalPacket
        };
        writer
            .write_packet(packet, serial, end, granule)
            .map_err(|_| Error::Limit)?;
    }
    if input.position() != encoded.len() as u64 {
        return Err(Error::Codec);
    }
    Ok((writer.into_inner().0, samples))
}

#[cfg(test)]
pub(crate) fn test_opus(rate: u32, channels: u32, seconds: u32) -> Vec<u8> {
    let encoder = Native::new(
        unsafe { heya_audio_encoder_new(f64::from(rate), channels) },
        heya_audio_encoder_free,
    )
    .expect("native Opus encoder");
    let frames = rate / 10;
    let mut samples = vec![0f32; (frames * channels) as usize];
    for chunk in 0..seconds * 10 {
        for (index, frame) in samples.chunks_exact_mut(channels as usize).enumerate() {
            let time = (chunk * frames + index as u32) as f64 / f64::from(rate);
            frame.fill(((time * 440. * std::f64::consts::TAU).sin() * 0.2) as f32);
        }
        assert_ne!(
            unsafe { heya_audio_encoder_write(encoder.pointer(), samples.as_ptr(), frames) },
            0
        );
    }
    let encoded = native_bytes(MAX_BYTES, |out, length| unsafe {
        heya_audio_encoder_finish(encoder.pointer(), out, length)
    })
    .expect("native Opus finalization");
    ogg_opus(&encoded).expect("Ogg Opus packaging").0
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe extern "C" {
        fn heya_audio_decode_test(
            bytes: *const u8,
            length: usize,
            frames: *mut u64,
            peak: *mut f32,
        ) -> i32;
    }
    #[test]
    fn native_opus_resamples_downmixes_and_roundtrips_without_audio_devices() {
        for (rate, channels) in [(48000, 1), (44100, 1), (44100, 2), (48000, 2)] {
            let bytes = test_opus(rate, channels, 1);
            let mut reader = ogg::PacketReader::new(Cursor::new(&bytes));
            let header = reader.read_packet_expected().unwrap();
            assert_eq!(&header.data[..8], b"OpusHead");
            assert_eq!(header.data[9], 1, "voice output must be mono");
            assert!(
                reader
                    .read_packet_expected()
                    .unwrap()
                    .data
                    .starts_with(b"OpusTags")
            );
            let mut packets = 0;
            while let Some(packet) = reader.read_packet().unwrap() {
                assert!(!packet.data.is_empty());
                packets += 1;
            }
            assert!(packets >= 50);
            let mut frames = 0;
            let mut peak = 0.;
            assert_ne!(
                unsafe {
                    heya_audio_decode_test(bytes.as_ptr(), bytes.len(), &mut frames, &mut peak)
                },
                0
            );
            assert_eq!(
                frames, 48000,
                "native decode must honor priming/end trimming"
            );
            assert!(
                (0.1..0.4).contains(&peak),
                "native decoder must recover the synthetic tone"
            );
        }
    }
    #[test]
    fn rejects_invalid_native_packet_frames_and_keeps_device_ids_stable() {
        assert!(ogg_opus(&[]).is_err());
        let mut header = Vec::new();
        header.extend(48000u64.to_le_bytes());
        header.extend(312u32.to_le_bytes());
        header.extend(u32::MAX.to_le_bytes());
        assert!(ogg_opus(&header).is_err());
        let mut device = Device {
            id: "synthetic-uid".into(),
            name: "Before".into(),
            input: true,
        };
        let id = device_id(&device);
        device.name = "Renamed".into();
        assert_eq!(id, device_id(&device));
        device.input = false;
        assert_ne!(id, device_id(&device));
        assert_eq!(id.len(), 64);
        assert!(selected_uid(true, "").unwrap().as_bytes().is_empty());
    }
}
