//! One duplex pipeline, confined to the finite call-audio helper.
use crate::{
    audio_devices::{self, Settings},
    call_audio::{Device, SAMPLES},
    voice_recorder::Error,
};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSrc};
use std::collections::VecDeque;

pub struct Audio {
    pipeline: gst::Pipeline,
    microphone: AppSink,
    speaker: AppSrc,
    captured: VecDeque<i16>,
}
impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
impl Audio {
    pub fn new(settings: Settings) -> Result<Self, Error> {
        gst::init().map_err(|_| Error::Codec)?;
        // Check input before creating a pipeline; a monitor/loopback isn't a mic.
        let input = audio_devices::element(true, &settings.input.id)?;
        let output = audio_devices::element(false, &settings.output.id)?;
        Self::build(input, output, settings)
    }
    #[cfg(test)]
    pub fn synthetic() -> Result<Self, Error> {
        gst::init().map_err(|_| Error::Codec)?;
        let input = gst::ElementFactory::make("audiotestsrc")
            .property("is-live", true)
            .build()
            .map_err(|_| Error::Codec)?;
        let output = gst::ElementFactory::make("fakesink")
            .property("sync", true)
            .build()
            .map_err(|_| Error::Codec)?;
        Self::build(input, output, Settings::default())
    }
    fn build(input: gst::Element, output: gst::Element, settings: Settings) -> Result<Self, Error> {
        let make = |name| {
            gst::ElementFactory::make(name)
                .build()
                .map_err(|_| Error::Codec)
        };
        let capture_convert = make("audioconvert")?;
        let capture_resample = make("audioresample")?;
        let capture_caps = make("capsfilter")?;
        let gain = make("volume")?;
        gain.set_property("volume", settings.input_volume as f64 / 100.);
        let pcm = gst::Caps::builder("audio/x-raw")
            .field("format", "S16LE")
            .field("rate", 16000i32)
            .field("channels", 1i32)
            .field("layout", "interleaved")
            .build();
        capture_caps.set_property("caps", &pcm);
        let microphone = AppSink::builder()
            .max_buffers(3)
            .drop(true)
            .sync(false)
            .build();
        let speaker = AppSrc::builder()
            .caps(&pcm)
            .format(gst::Format::Time)
            .is_live(true)
            .do_timestamp(true)
            .block(false)
            .max_bytes(16_000 * 2 / 5)
            .build();
        let play_gain = make("volume")?;
        play_gain.set_property("volume", settings.output_volume as f64 / 100.);
        let play_convert = make("audioconvert")?;
        let play_resample = make("audioresample")?;
        let pipeline = gst::Pipeline::new();
        let audio = Self {
            pipeline,
            microphone,
            speaker,
            captured: VecDeque::with_capacity(4096),
        };
        let mut capture = vec![input, capture_convert, capture_resample, capture_caps, gain];
        let mut playback = vec![audio.speaker.clone().upcast(), play_gain];
        // The remote may still be ringing: speaker preroll cannot hold up capture.
        if output.find_property("async").is_some() {
            output.set_property("async", false);
        }
        // DSP and its reverse-stream probe share a pipeline and the same PCM
        // format, allowing echo cancellation without a second capture stream.
        if gst::ElementFactory::find("webrtcdsp").is_some()
            && gst::ElementFactory::find("webrtcechoprobe").is_some()
        {
            let probe = gst::ElementFactory::make("webrtcechoprobe")
                .name("heya-call-echo")
                .build()
                .map_err(|_| Error::Codec)?;
            let dsp = make("webrtcdsp")?;
            dsp.set_property("probe", "heya-call-echo");
            dsp.set_property("gain-control", false);
            capture.push(dsp);
            playback.push(probe);
        }
        capture.push(audio.microphone.clone().upcast());
        playback.extend([play_convert, play_resample, output]);
        for branch in [&capture, &playback] {
            audio.pipeline.add_many(branch).map_err(|_| Error::Device)?;
            gst::Element::link_many(branch).map_err(|_| Error::Codec)?;
        }
        audio
            .pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| Error::Device)?;
        Ok(audio)
    }
}
impl Device for Audio {
    fn capture(&mut self) -> Result<Option<Vec<i16>>, Error> {
        if self.pipeline.bus().is_some_and(|bus| {
            bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos])
                .is_some()
        }) {
            return Err(Error::Device);
        }
        if let Some(sample) = self
            .microphone
            .try_pull_sample(gst::ClockTime::from_mseconds(10))
        {
            let buffer = sample.buffer().ok_or(Error::Device)?;
            let data = buffer.map_readable().map_err(|_| Error::Device)?;
            if data.len() % 2 != 0 || data.len() > 32768 {
                return Err(Error::Device);
            }
            self.captured.extend(
                data.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|s| i16::from_le_bytes([s[0], s[1]])),
            );
            while self.captured.len() > SAMPLES * 3 {
                self.captured.drain(..SAMPLES);
            }
        }
        Ok((self.captured.len() >= SAMPLES).then(|| self.captured.drain(..SAMPLES).collect()))
    }
    fn play(&mut self, pcm: &[i16]) -> Result<(), Error> {
        if self.speaker.current_level_bytes() >= 16_000 * 2 / 5 {
            return Ok(());
        }
        let data: Vec<_> = pcm.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut buffer = gst::Buffer::from_mut_slice(data);
        let writable = buffer.get_mut().ok_or(Error::Playback)?;
        writable.set_duration(gst::ClockTime::from_nseconds(
            pcm.len() as u64 * 1_000_000_000 / 16000,
        ));
        self.speaker
            .push_buffer(buffer)
            .map_err(|_| Error::Playback)?;
        Ok(())
    }
}
