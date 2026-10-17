//! One microphone/encoder lifetime, created explicitly and torn down before completion.
use std::sync::Arc;
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
use std::time::Duration;
use tokio::sync::watch;
pub const MAX_SECONDS: u32 = 30 * 60;
pub const MAX_BYTES: usize = crate::audio::MAX_BYTES;
#[cfg_attr(
    not(all(feature = "media-gstreamer", target_os = "linux")),
    allow(dead_code)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Disabled,
    Codec,
    Device,
    Empty,
    Limit,
    Cancelled,
    Playback,
    NoInput,
}
impl Error {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Voice recording is disabled in this build.",
            Self::Codec => "Voice recording needs the GStreamer Opus and Ogg plugins.",
            Self::Device => "Could not record audio. Check your microphone and its permissions.",
            Self::Empty => "No audio was recorded. Try recording again.",
            Self::Limit => "The voice message reached the recording limit.",
            Self::Cancelled => "Recording discarded.",
            Self::Playback => "Could not play the recording. Check your audio output.",
            Self::NoInput => "No microphone was found. Connect a microphone and try again.",
        }
    }
}
#[derive(Clone)]
pub struct Clip {
    pub bytes: Arc<[u8]>,
    pub seconds: u32,
    pub waveform: Vec<u8>,
}
#[derive(Clone, Default, PartialEq)]
pub struct Progress {
    pub seconds: f64,
    pub levels: Vec<u8>,
}
#[derive(Clone, Copy, PartialEq)]
pub enum Desired {
    Record,
    Pause,
    Finish,
    Cancel,
}
#[derive(Clone, Copy)]
pub enum Input {
    Microphone,
    Synthetic,
    /// Synthetic missing-device fixture; never opens a hardware source.
    Unavailable,
}
pub struct Worker(crate::media_process::Worker);
impl Worker {
    pub fn start(
        input: Input,
        progress: watch::Sender<Progress>,
        settings: crate::audio_devices::Settings,
    ) -> Result<Self, Error> {
        crate::media_process::Worker::start(
            crate::media_process::Mode::Capture(input, settings),
            progress,
        )
        .map(Self)
    }
    pub fn set(&self, desired: Desired) {
        self.0.set(desired);
    }
    pub async fn finished(&mut self) -> Result<Clip, Error> {
        match self.0.finished().await? {
            crate::media_process::Outcome::Clip(clip) => Ok(clip),
            _ => Err(Error::Device),
        }
    }
}

#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
struct Pipeline(gstreamer::Pipeline);
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
impl Drop for Pipeline {
    fn drop(&mut self) {
        use gstreamer::prelude::*;
        let _ = self.0.set_state(gstreamer::State::Null);
    }
}
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
#[derive(Default)]
struct Data {
    bytes: Vec<u8>,
    samples: u64,
    levels: Vec<u8>,
    error: Option<Error>,
}
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
pub(crate) fn capture(
    input: Input,
    settings: crate::audio_devices::Settings,
    mut publish: impl FnMut(Progress),
    commands: std::sync::mpsc::Receiver<Desired>,
) -> Result<Clip, Error> {
    use gstreamer::{self as gst, prelude::*};
    use gstreamer_app::{AppSink, AppSinkCallbacks};
    use std::{sync::Mutex, time::Instant};
    if matches!(input, Input::Unavailable) {
        return Err(Error::NoInput);
    }
    gst::init().map_err(|_| Error::Codec)?;
    let make = |name| {
        gst::ElementFactory::make(name)
            .build()
            .map_err(|_| Error::Codec)
    };
    // Never let an automatic source fall back to generated/silent audio. Select
    // an enumerated input first; an empty list creates no recording pipeline.
    let source = match input {
        Input::Microphone => crate::audio_devices::element(true, &settings.input.id)?,
        Input::Synthetic => make("audiotestsrc")?,
        Input::Unavailable => return Err(Error::NoInput),
    };
    let pipeline = Pipeline(gst::Pipeline::new());
    if matches!(input, Input::Synthetic) {
        source.set_property("is-live", true);
        source.set_property("volume", 0.2f64);
    }
    let convert = make("audioconvert")?;
    let resample = make("audioresample")?;
    let filter = make("capsfilter")?;
    filter.set_property(
        "caps",
        gst::Caps::builder("audio/x-raw")
            .field("format", "S16LE")
            .field("rate", 48000i32)
            .field("channels", 1i32)
            .field("layout", "interleaved")
            .build(),
    );
    let gain = make("volume")?;
    gain.set_property("volume", f64::from(settings.input_volume) / 100.);
    let encoder = make("opusenc")?;
    encoder.set_property("bitrate", 32000i32);
    encoder.set_property_from_str("audio-type", "voice");
    let mux = make("oggmux")?;
    let sink = AppSink::builder()
        .sync(false)
        .max_buffers(4)
        .wait_on_eos(false)
        .build();
    let data = Arc::new(Mutex::new(Data::default()));
    let raw = data.clone();
    gain.static_pad("src").ok_or(Error::Codec)?.add_probe(
        gst::PadProbeType::BUFFER,
        move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buffer)) = &info.data
                && let Ok(map) = buffer.map_readable()
            {
                let mut state = raw.lock().unwrap();
                let mut peak = 0u16;
                for sample in map.as_chunks::<2>().0 {
                    peak = peak.max(i16::from_le_bytes(*sample).unsigned_abs());
                }
                let bucket = (state.samples / 4800) as usize;
                state.samples += map.len() as u64 / 2;
                let level = ((f64::from(peak) / 32768.).sqrt() * 100.).min(100.) as u8;
                if bucket < MAX_SECONDS as usize * 10 + 1 {
                    if state.levels.len() <= bucket {
                        state.levels.resize(bucket + 1, 0);
                    }
                    state.levels[bucket] = state.levels[bucket].max(level);
                }
            }
            gst::PadProbeReturn::Ok
        },
    );
    let encoded = data.clone();
    sink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let map = sample
                    .buffer()
                    .ok_or(gst::FlowError::Error)?
                    .map_readable()
                    .map_err(|_| gst::FlowError::Error)?;
                let mut state = encoded.lock().unwrap();
                if state.bytes.len().saturating_add(map.len()) > MAX_BYTES {
                    state.error = Some(Error::Limit);
                    return Err(gst::FlowError::Error);
                }
                state.bytes.extend_from_slice(&map);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    pipeline
        .0
        .add_many([
            &source,
            &convert,
            &resample,
            &filter,
            &gain,
            &encoder,
            &mux,
            sink.upcast_ref(),
        ])
        .map_err(|_| Error::Codec)?;
    gst::Element::link_many([
        &source,
        &convert,
        &resample,
        &filter,
        &gain,
        &encoder,
        &mux,
        sink.upcast_ref(),
    ])
    .map_err(|_| Error::Codec)?;
    let bus = pipeline.0.bus().ok_or(Error::Device)?;
    pipeline
        .0
        .set_state(gst::State::Playing)
        .map_err(|_| Error::Device)?;
    let mut started = Instant::now();
    let mut paused = false;
    let mut finishing = None;
    let mut last = Instant::now();
    loop {
        let command = match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(c) => Some(c),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(Error::Cancelled),
            Err(_) => None,
        };
        match command {
            Some(Desired::Cancel) => return Err(Error::Cancelled),
            Some(Desired::Pause) if finishing.is_none() => {
                paused = true;
                pipeline
                    .0
                    .set_state(gst::State::Paused)
                    .map_err(|_| Error::Device)?;
            }
            Some(Desired::Record) if finishing.is_none() => {
                paused = false;
                if data.lock().unwrap().samples == 0 {
                    started = Instant::now();
                }
                pipeline
                    .0
                    .set_state(gst::State::Playing)
                    .map_err(|_| Error::Device)?;
            }
            _ => {}
        }
        if !paused
            && data.lock().unwrap().samples == 0
            && started.elapsed() > Duration::from_secs(10)
        {
            return Err(Error::Device);
        }
        let limit = data.lock().unwrap().samples >= u64::from(MAX_SECONDS) * 48000;
        if finishing.is_none() && (command == Some(Desired::Finish) || limit) {
            // A live pipeline must run to drain the encoder/muxer, even when paused.
            pipeline
                .0
                .set_state(gst::State::Playing)
                .map_err(|_| Error::Device)?;
            if !pipeline.0.send_event(gst::event::Eos::new()) {
                return Err(Error::Device);
            }
            finishing = Some(Instant::now());
        }
        for message in bus.iter().take(64) {
            match message.view() {
                gst::MessageView::Error(_) => {
                    return Err(data.lock().unwrap().error.unwrap_or(Error::Device));
                }
                gst::MessageView::Eos(_) => {
                    // Stop streaming before extracting bytes; NULL waits for the callbacks.
                    pipeline
                        .0
                        .set_state(gst::State::Null)
                        .map_err(|_| Error::Device)?;
                    let mut state = data.lock().unwrap();
                    if state.samples < 4800 || state.bytes.is_empty() {
                        return Err(Error::Empty);
                    }
                    let seconds = state.samples.div_ceil(48000).min(u64::from(MAX_SECONDS)) as u32;
                    let waveform = compact(&state.levels);
                    return Ok(Clip {
                        bytes: std::mem::take(&mut state.bytes).into(),
                        seconds,
                        waveform,
                    });
                }
                _ => {}
            }
        }
        if finishing.is_some_and(|s| s.elapsed() > Duration::from_secs(5)) {
            return Err(Error::Device);
        }
        if last.elapsed() >= Duration::from_millis(100) {
            last = Instant::now();
            let state = data.lock().unwrap();
            let next = Progress {
                seconds: state.samples as f64 / 48000.,
                levels: {
                    let recent = &state.levels[state.levels.len().saturating_sub(64)..];
                    let mut levels = vec![0; 64 - recent.len()];
                    levels.extend_from_slice(recent);
                    levels
                },
            };
            drop(state);
            if matches!(input, Input::Synthetic) {
                // Vary the synthetic tone so automated render checks exercise
                // changing waveform geometry throughout the recording.
                source.set_property(
                    "volume",
                    0.04 + ((next.seconds * 10.) as u64 % 10) as f64 * 0.03,
                );
            }
            publish(next);
        }
    }
}

#[cfg(not(all(feature = "media-gstreamer", target_os = "linux")))]
pub(crate) fn probe_input() -> Result<(), Error> {
    Err(Error::Disabled)
}
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
pub(crate) fn probe_input() -> Result<(), Error> {
    crate::audio_devices::element(true, "").map(|_| ())
}

#[cfg(all(test, feature = "media-gstreamer", target_os = "linux"))]
mod input_tests {
    use super::*;
    #[test]
    fn no_input_and_output_monitors_are_not_recording_sources() {
        gstreamer::init().unwrap();
        assert!(matches!(
            crate::audio_devices::select_empty_input_for_test(),
            Err(Error::NoInput)
        ));
        for (key, value) in [
            ("device.class", "monitor"),
            ("media.class", "Audio/Sink"),
            ("device.name", "speaker.monitor"),
            ("node.name", "output.monitor"),
        ] {
            assert!(!crate::audio_devices::usable_input(
                &gstreamer::Structure::builder("properties")
                    .field(key, value)
                    .build()
            ));
        }
        assert!(crate::audio_devices::usable_input(
            &gstreamer::Structure::builder("properties")
                .field("device.name", "alsa_input.usb-microphone")
                .field("media.class", "Audio/Source")
                .build()
        ));
    }
}
#[cfg(not(all(feature = "media-gstreamer", target_os = "linux")))]
pub(crate) fn capture(
    _: Input,
    _: crate::audio_devices::Settings,
    _: impl FnMut(Progress),
    _: std::sync::mpsc::Receiver<Desired>,
) -> Result<Clip, Error> {
    Err(Error::Disabled)
}
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
fn compact(levels: &[u8]) -> Vec<u8> {
    if levels.is_empty() {
        return vec![0; 64];
    }
    (0..64)
        .map(|i| {
            let from = i * levels.len() / 64;
            let to = ((i + 1) * levels.len() / 64)
                .max(from + 1)
                .min(levels.len());
            levels[from..to].iter().copied().max().unwrap_or(0)
        })
        .collect()
}
