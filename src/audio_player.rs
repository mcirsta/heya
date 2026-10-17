//! One GStreamer pipeline on a thread created for playback, never by a chat row.
//! Compressed data is fed from bounded memory; no plaintext temporary files.
use crate::audio::{Error, Status};
#[cfg(feature = "media-gstreamer")]
use gstreamer::{self as gst, prelude::*};
#[cfg(feature = "media-gstreamer")]
use gstreamer_app::{AppSrc, AppSrcCallbacks, AppStreamType};
use std::sync::Arc;
#[cfg(feature = "media-gstreamer")]
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, watch};

#[derive(Clone)]
pub enum Output {
    #[cfg_attr(not(feature = "media-gstreamer"), allow(dead_code))]
    Desktop(crate::audio_devices::Settings),
    Silent,
}

/// Owns one playback lifetime. Dropping the control channel requests teardown;
/// completion acknowledges pipeline teardown before another player may start.
/// A dedicated thread exits with the player instead of parking in a thread pool.
pub struct Worker {
    pub generation: u64,
    control: watch::Sender<Desired>,
    done: oneshot::Receiver<Status>,
}
impl Worker {
    pub fn start(
        initial: Desired,
        updates: watch::Sender<Status>,
        output: Output,
    ) -> Result<Self, Error> {
        Self::spawn(initial, updates, move |desired, updates| {
            run(desired, updates, output)
        })
    }

    pub(super) fn spawn(
        initial: Desired,
        updates: watch::Sender<Status>,
        play: impl FnOnce(watch::Receiver<Desired>, watch::Sender<Status>) -> Status + Send + 'static,
    ) -> Result<Self, Error> {
        if initial.bytes.is_none() {
            return Err(Error::Unavailable);
        }
        let generation = initial.generation;
        let (control, desired) = watch::channel(initial);
        let (finished, done) = oneshot::channel();
        let runtime = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("heya-audio".into())
            .spawn(move || {
                let result = {
                    let _runtime = runtime.enter();
                    play(desired, updates)
                };
                // This is the last action on the thread, after player teardown.
                // A panic drops the sender, which the coordinator reports as failure.
                let _ = finished.send(result);
            })
            .map_err(|_| Error::Playback)?;
        Ok(Self {
            generation,
            control,
            done,
        })
    }

    pub fn update(&self, desired: &Desired) {
        if desired.generation != self.generation || desired.bytes.is_none() {
            self.stop();
            return;
        }
        self.control.send_if_modified(|current| {
            // Bytes/id are immutable within one generation. Compare only the
            // controls; progress notifications must not wake the player again.
            if current.playing == desired.playing
                && current.seek == desired.seek
                && current.speed == desired.speed
            {
                false
            } else {
                *current = desired.clone();
                true
            }
        });
    }

    pub fn stop(&self) {
        self.control.send_if_modified(|current| {
            if current.bytes.is_none() {
                false
            } else {
                current.clear();
                true
            }
        });
    }

    pub async fn finished(&mut self) -> Result<Status, Error> {
        (&mut self.done).await.map_err(|_| Error::Playback)
    }
}

#[cfg_attr(not(feature = "media-gstreamer"), allow(dead_code))]
#[derive(Clone, Default)]
pub struct Desired {
    pub generation: u64,
    pub id: i32,
    pub bytes: Option<Arc<[u8]>>,
    pub playing: bool,
    pub seek: Option<(u64, f64)>,
    pub speed: f64,
}
impl Desired {
    pub fn clear(&mut self) {
        *self = Self {
            generation: self.generation.wrapping_add(1),
            speed: self.speed,
            ..Self::default()
        };
    }
}

#[cfg(feature = "media-gstreamer")]
struct Pipeline(gst::Pipeline);
#[cfg(feature = "media-gstreamer")]
impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

#[cfg(feature = "media-gstreamer")]
impl Pipeline {
    fn new(bytes: Arc<[u8]>, output: Output) -> Result<Self, Error> {
        gst::init().map_err(|_| Error::Playback)?;
        let pipeline = Self(gst::Pipeline::new());
        let source = AppSrc::builder()
            .format(gst::Format::Bytes)
            .stream_type(AppStreamType::RandomAccess)
            .size(bytes.len() as i64)
            .max_bytes(256 * 1024)
            .build();
        let cursor = Arc::new(Mutex::new(0usize));
        let seek_cursor = cursor.clone();
        let len = bytes.len();
        source.set_callbacks(
            AppSrcCallbacks::builder()
                .need_data(move |src, length| {
                    let buffer = {
                        let mut at = cursor.lock().unwrap();
                        if *at >= bytes.len() {
                            None
                        } else {
                            // Random-access demuxers request an exact byte range.
                            let length = if length == u32::MAX {
                                64 * 1024
                            } else {
                                length as usize
                            };
                            let end = at.saturating_add(length).min(bytes.len());
                            let buffer = gst::Buffer::from_mut_slice(bytes[*at..end].to_vec());
                            *at = end;
                            Some(buffer)
                        }
                    };
                    if let Some(buffer) = buffer {
                        let _ = src.push_buffer(buffer);
                    } else {
                        let _ = src.end_of_stream();
                    }
                })
                .seek_data(move |_, offset| {
                    if offset > len as u64 {
                        return false;
                    }
                    *seek_cursor.lock().unwrap() = offset as usize;
                    true
                })
                .build(),
        );
        let make = |name| {
            gst::ElementFactory::make(name)
                .build()
                .map_err(|_| Error::Codec)
        };
        let decode = make("decodebin")?;
        let convert = make("audioconvert")?;
        let resample = make("audioresample")?;
        let tempo = make("scaletempo")?;
        let sink = match &output {
            Output::Desktop(settings) => crate::audio_devices::element(false, &settings.output.id)
                .map_err(|_| Error::Playback)?,
            Output::Silent => make("fakesink")?,
        };
        let volume = make("volume")?;
        volume.set_property(
            "volume",
            match &output {
                Output::Desktop(s) => f64::from(s.output_volume) / 100.,
                Output::Silent => 1.,
            },
        );
        if matches!(output, Output::Silent) {
            sink.set_property("sync", true);
        }
        pipeline
            .0
            .add_many([
                source.upcast_ref(),
                &decode,
                &convert,
                &resample,
                &tempo,
                &volume,
                &sink,
            ])
            .map_err(|_| Error::Playback)?;
        source.link(&decode).map_err(|_| Error::Playback)?;
        gst::Element::link_many([&convert, &resample, &tempo, &volume, &sink])
            .map_err(|_| Error::Playback)?;
        let input = convert.static_pad("sink").ok_or(Error::Playback)?;
        decode.connect_pad_added(move |_, pad| {
            let audio = pad.current_caps().is_some_and(|caps| {
                caps.structure(0)
                    .is_some_and(|s| s.name().starts_with("audio/"))
            });
            if audio && !input.is_linked() {
                let _ = pad.link(&input);
            }
        });
        pipeline
            .0
            .set_state(gst::State::Paused)
            .map_err(|_| Error::Playback)?;
        Ok(pipeline)
    }
    fn seek(&self, seconds: f64, rate: f64) -> Result<(), Error> {
        self.0
            .seek(
                rate,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_mseconds((seconds.max(0.) * 1000.) as u64),
                gst::SeekType::None,
                None::<gst::ClockTime>,
            )
            .map_err(|_| Error::Seek)
    }
}

/// Commands coalesce instead of accumulating while a decoder starts or seeks.
/// Stop, completion and failure end the thread. Paused players wait for controls
/// without polling; progress/preroll updates are at most 10 Hz.
#[cfg(feature = "media-gstreamer")]
fn run(
    mut desired: watch::Receiver<Desired>,
    updates: watch::Sender<Status>,
    output: Output,
) -> Status {
    let runtime = tokio::runtime::Handle::current();
    let initial = desired.borrow_and_update().clone();
    let mut status = Status {
        generation: initial.generation,
        id: initial.id,
        phase: 1,
        speed: initial.speed,
        ..Default::default()
    };
    let Some(bytes) = initial.bytes else {
        return Status::default();
    };
    if desired.has_changed().is_err() {
        return Status::default();
    }
    let pipeline = match Pipeline::new(bytes, output) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            status.fail(error);
            updates.send_replace(status.clone());
            return status;
        }
    };
    let mut ready = false;
    let mut started = Instant::now();
    let mut applied_seek = None;
    let mut applied_speed = 1.;
    let mut applied_playing = None;
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
        {
            let player = &pipeline;
            if let Some(bus) = player.0.bus() {
                // Bounded per iteration even for a malformed plugin's message burst.
                for message in bus.iter().take(64) {
                    match message.view() {
                        gst::MessageView::Error(_) => status.fail(Error::Playback),
                        gst::MessageView::Eos(_) => {
                            status.phase = 0;
                            status.position = status.duration;
                        }
                        gst::MessageView::AsyncDone(_) => ready = true,
                        _ => {}
                    }
                }
            }
            if !ready && started.elapsed() > Duration::from_secs(10) {
                status.fail(Error::Playback);
            }
            if ready && !matches!(status.phase, 0 | 4) {
                status.duration = player
                    .0
                    .query_duration::<gst::ClockTime>()
                    .map_or(status.duration, |v| v.mseconds() as f64 / 1000.);
                if status.duration > 6. * 3600. {
                    status.fail(Error::TooLong);
                }
                if current.seek != applied_seek || current.speed != applied_speed {
                    let position = if current.seek != applied_seek {
                        current.seek.map_or(status.position, |(_, fraction)| {
                            fraction.clamp(0., 1.) * status.duration
                        })
                    } else {
                        status.position
                    };
                    if let Err(error) = player.seek(position, current.speed) {
                        status.fail(error);
                    } else {
                        // A successful seek starts asynchronous preroll. Do not
                        // publish a stale position or issue another seek until
                        // the new segment is ready (ASYNC_DONE).
                        ready = false;
                        started = Instant::now();
                        status.phase = 1;
                    }
                    applied_seek = current.seek;
                    applied_speed = current.speed;
                }
                if status.phase != 4 && ready {
                    let state = if current.playing {
                        gst::State::Playing
                    } else {
                        gst::State::Paused
                    };
                    if applied_playing != Some(current.playing)
                        && player.0.set_state(state).is_err()
                    {
                        status.fail(Error::Playback);
                    } else {
                        applied_playing = Some(current.playing);
                        status.phase = if current.playing { 2 } else { 3 };
                        status.position = player
                            .0
                            .query_position::<gst::ClockTime>()
                            .map_or(status.position, |v| v.mseconds() as f64 / 1000.);
                        status.speed = current.speed;
                    }
                }
            }
        }
        if matches!(status.phase, 0 | 4) {
            break;
        }
        updates.send_if_modified(|old| {
            if *old == status {
                false
            } else {
                *old = status.clone();
                true
            }
        });
        let poll = status.phase != 3;
        let closed = runtime.block_on(async {
            tokio::select! {
                result=desired.changed()=>result.is_err(),
                _=tokio::time::sleep(Duration::from_millis(100)),if poll=>false,
            }
        });
        if closed {
            status.phase = 0;
            break;
        }
    }
    drop(pipeline);
    updates.send_replace(status.clone());
    status
}

/// An explicit unavailable result for UI/domain-only builds, never fake playback.
#[cfg(not(feature = "media-gstreamer"))]
fn run(
    desired: watch::Receiver<Desired>,
    updates: watch::Sender<Status>,
    _output: Output,
) -> Status {
    let next = desired.borrow();
    let mut status = Status {
        generation: next.generation,
        id: next.id,
        ..Default::default()
    };
    status.fail(Error::NotBuilt);
    updates.send_replace(status.clone());
    status
}
