//! One GStreamer pipeline inside the finite decoder helper; never a UI control.
use crate::{
    video::{self, Error},
    video_process::{Control, Emitter, Header, Progress},
};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::AppSink;
use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;

struct Pipeline(gst::Pipeline);
impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

pub fn play(
    bytes: Vec<u8>,
    header: Header,
    control: &mut watch::Receiver<Control>,
    output: &mut Emitter<impl Write>,
) -> Result<(), Error> {
    gst::init().map_err(|_| Error::Playback)?;
    let make = |name| {
        gst::ElementFactory::make(name)
            .build()
            .map_err(|_| Error::Codec)
    };
    let pipeline = Pipeline(gst::Pipeline::new());
    let source = crate::gstreamer_memory::source(Arc::from(bytes));
    let decode = make("decodebin")?;
    let queue = make("queue")?;
    queue.set_property("max-size-buffers", 2u32);
    queue.set_property("max-size-bytes", 0u32);
    queue.set_property("max-size-time", 0u64);
    let convert = make("videoconvert")?;
    let flip = make("videoflip")?;
    flip.set_property_from_str("video-direction", "auto");
    let scale = make("videoscale")?;
    let rate = make("videorate")?;
    rate.set_property("drop-only", true);
    rate.set_property("max-rate", 25i32);
    let sink = AppSink::builder()
        .caps(
            &gst::Caps::builder("video/x-raw")
                .field("format", "RGBA")
                .field("width", gst::IntRange::<i32>::new(1, video::EDGE as i32))
                .field("height", gst::IntRange::<i32>::new(1, video::EDGE as i32))
                .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                .build(),
        )
        .max_buffers(1)
        .drop(true)
        .sync(true)
        .wait_on_eos(false)
        .build();
    pipeline
        .0
        .add_many([
            source.upcast_ref(),
            &decode,
            &queue,
            &convert,
            &flip,
            &scale,
            &rate,
            sink.upcast_ref(),
        ])
        .map_err(|_| Error::Playback)?;
    source.link(&decode).map_err(|_| Error::Playback)?;
    gst::Element::link_many([&queue, &convert, &flip, &scale, &rate, sink.upcast_ref()])
        .map_err(|_| Error::Playback)?;
    let video_input = queue.static_pad("sink").ok_or(Error::Playback)?;
    let audio_present = Arc::new(AtomicBool::new(false));
    let audio_failed = Arc::new(AtomicBool::new(false));
    let current_mute = Arc::new(AtomicBool::new(false));
    let audio_volume: Arc<Mutex<Option<gst::Element>>> = Arc::new(Mutex::new(None));
    let (present, failed, muted, volume_slot) = (
        audio_present.clone(),
        audio_failed.clone(),
        current_mute.clone(),
        audio_volume.clone(),
    );
    let weak_pipeline = pipeline.0.downgrade();
    decode.connect_pad_added(move |_, pad| {
        let Some(caps) = pad.current_caps() else {
            return;
        };
        let Some(s) = caps.structure(0) else {
            return;
        };
        if s.name().starts_with("video/") {
            if !video_input.is_linked() {
                let _ = pad.link(&video_input);
            }
        } else if s.name().starts_with("audio/") && !present.swap(true, Ordering::AcqRel) {
            // Create the output only after a real audio track is discovered.
            // Silent videos never reserve an audio device.
            let result = (|| -> Result<(), Error> {
                let pipeline = weak_pipeline.upgrade().ok_or(Error::Playback)?;
                let queue = make("queue")?;
                let convert = make("audioconvert")?;
                let resample = make("audioresample")?;
                let volume = make("volume")?;
                volume.set_property("volume", f64::from(header.settings.output_volume) / 100.);
                volume.set_property("mute", muted.load(Ordering::Acquire));
                let sink = if header.silent {
                    make("fakesink")?
                } else {
                    crate::audio_devices::element(false, &header.settings.output.id)
                        .map_err(|_| Error::Playback)?
                };
                sink.set_property("sync", true);
                let elements = [&queue, &convert, &resample, &volume, &sink];
                pipeline.add_many(elements).map_err(|_| Error::Playback)?;
                gst::Element::link_many(elements).map_err(|_| Error::Playback)?;
                pad.link(&queue.static_pad("sink").ok_or(Error::Playback)?)
                    .map_err(|_| Error::Playback)?;
                *volume_slot.lock().unwrap() = Some(volume.clone());
                for element in elements {
                    element
                        .sync_state_with_parent()
                        .map_err(|_| Error::Playback)?;
                }
                Ok(())
            })();
            if result.is_err() {
                failed.store(true, Ordering::Release);
            }
        }
    });
    pipeline
        .0
        .set_state(gst::State::Playing)
        .map_err(|_| Error::Codec)?;
    let bus = pipeline.0.bus().ok_or(Error::Playback)?;
    let mut previous = Control::default();
    let mut seek = None;
    let mut need_preroll = false;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    let started = Instant::now();
    let mut saw_frame = false;
    loop {
        let desired = control.borrow_and_update().clone();
        if desired.stop || control.has_changed().is_err() {
            return Ok(());
        }
        if desired.playing != previous.playing {
            pipeline
                .0
                .set_state(if desired.playing {
                    gst::State::Playing
                } else {
                    gst::State::Paused
                })
                .map_err(|_| Error::Playback)?;
        }
        if desired.muted != previous.muted {
            current_mute.store(desired.muted, Ordering::Release);
            if let Some(volume) = audio_volume.lock().unwrap().as_ref() {
                volume.set_property("mute", desired.muted);
            }
        }
        if audio_failed.load(Ordering::Acquire) {
            return Err(Error::Playback);
        }
        if desired.seek != seek {
            if let Some((_, seconds)) = desired.seek {
                pipeline
                    .0
                    .seek_simple(
                        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                        gst::ClockTime::from_nseconds((seconds * 1e9) as u64),
                    )
                    .map_err(|_| Error::Seek)?;
            }
            seek = desired.seek;
            need_preroll = true;
        }
        previous = desired.clone();
        for message in bus.iter() {
            match message.view() {
                gst::MessageView::Error(_) => return Err(Error::Codec),
                gst::MessageView::Eos(_) => {
                    return if saw_frame { Ok(()) } else { Err(Error::Codec) };
                }
                _ => {}
            }
        }
        if saw_frame && sink.is_eos() && !audio_present.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(40)) {
            let caps = sample
                .caps()
                .and_then(|c| c.structure(0))
                .ok_or(Error::Playback)?;
            let width = caps.get::<i32>("width").map_err(|_| Error::Playback)? as u32;
            let height = caps.get::<i32>("height").map_err(|_| Error::Playback)? as u32;
            let buffer = sample
                .buffer()
                .ok_or(Error::Playback)?
                .map_readable()
                .map_err(|_| Error::Playback)?;
            output.frame(width, height, buffer.as_slice())?;
            saw_frame = true;
        } else if !desired.playing && need_preroll {
            // appsink has a separate preroll sample when a seek completes paused.
            if let Some(sample) = sink.try_pull_preroll(gst::ClockTime::ZERO) {
                let caps = sample
                    .caps()
                    .and_then(|c| c.structure(0))
                    .ok_or(Error::Playback)?;
                let width = caps.get::<i32>("width").map_err(|_| Error::Playback)? as u32;
                let height = caps.get::<i32>("height").map_err(|_| Error::Playback)? as u32;
                let buffer = sample
                    .buffer()
                    .ok_or(Error::Playback)?
                    .map_readable()
                    .map_err(|_| Error::Playback)?;
                output.frame(width, height, buffer.as_slice())?;
                saw_frame = true;
                need_preroll = false;
            }
        }
        if !saw_frame && started.elapsed() > Duration::from_secs(15) {
            return Err(Error::Codec);
        }
        if last_progress.elapsed() >= Duration::from_millis(100) {
            let position = pipeline
                .0
                .query_position::<gst::ClockTime>()
                .map_or(0., |t| t.nseconds() as f64 / 1e9);
            let duration = pipeline
                .0
                .query_duration::<gst::ClockTime>()
                .map_or(0., |t| t.nseconds() as f64 / 1e9);
            if duration > video::MAX_SECONDS || position > video::MAX_SECONDS {
                return Err(Error::TooLarge);
            }
            output.progress(&Progress {
                phase: if !saw_frame {
                    1
                } else if desired.playing {
                    2
                } else {
                    3
                },
                position,
                duration,
                muted: desired.muted,
                error: None,
            })?;
            last_progress = Instant::now();
        }
    }
}
