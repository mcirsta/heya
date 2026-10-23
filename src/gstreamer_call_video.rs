//! Camera capture and H.264 decoding inside the finite call-video helper.
use crate::{
    call_video::{Device, EDGE, Error, Packet},
    photos::Pixels,
};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSrc};

struct Pipeline(gst::Pipeline);
impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}
pub struct Video {
    capture: Pipeline,
    decoder: Pipeline,
    encoded: AppSink,
    preview: AppSink,
    source: AppSrc,
    remote: AppSink,
    orientation: u8,
    lost: bool,
}
impl Video {
    pub fn new(synthetic: bool) -> Result<Self, Error> {
        gst::init().map_err(|_| Error::Device)?;
        let encoder = if gst::ElementFactory::find("x264enc").is_some() {
            "x264enc name=encoder tune=zerolatency speed-preset=ultrafast bitrate=700 key-int-max=15 bframes=0 byte-stream=true"
        } else if gst::ElementFactory::find("openh264enc").is_some() {
            "openh264enc name=encoder bitrate=700000 gop-size=15 complexity=low"
        } else {
            return Err(Error::Codec);
        };
        let camera = if synthetic {
            gst::ElementFactory::make("videotestsrc")
                .property("is-live", true)
                .build()
                .map_err(|_| Error::Codec)?
        } else {
            let monitor = gst::DeviceMonitor::new();
            monitor.add_filter(Some("Video/Source"), None);
            monitor.start().map_err(|_| Error::NoCamera)?;
            let device = monitor.devices().into_iter().next();
            monitor.stop();
            device
                .ok_or(Error::NoCamera)?
                .create_element(None)
                .map_err(|_| Error::Permission)?
        };
        // Raw preview and encoder queues both drop old frames, never accumulate latency.
        let branch = format!(
            "videoconvert ! videoscale ! videorate ! video/x-raw,width=640,height=480,framerate=15/1,pixel-aspect-ratio=1/1 ! tee name=t \
             t. ! queue max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream ! videoconvert ! video/x-raw,format=RGBA ! appsink name=preview max-buffers=1 drop=true sync=false \
             t. ! queue max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream ! {encoder} ! video/x-h264,profile=constrained-baseline ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=encoded max-buffers=2 drop=true sync=false"
        );
        let bin = gst::parse::bin_from_description(&branch, true).map_err(|_| Error::Codec)?;
        let capture = Pipeline(gst::Pipeline::new());
        capture
            .0
            .add_many([&camera, bin.upcast_ref()])
            .map_err(|_| Error::Device)?;
        camera.link(&bin).map_err(|_| Error::Codec)?;
        let encoded = capture
            .0
            .by_name("encoded")
            .ok_or(Error::Codec)?
            .downcast::<AppSink>()
            .map_err(|_| Error::Codec)?;
        let preview = capture
            .0
            .by_name("preview")
            .ok_or(Error::Codec)?
            .downcast::<AppSink>()
            .map_err(|_| Error::Codec)?;
        // Decoder output is bounded in both dimensions and cadence before crossing IPC.
        let decoder = gst::parse::launch(&format!(
            "appsrc name=source is-live=true format=time do-timestamp=true block=false max-bytes=2097152 caps=video/x-h264,stream-format=byte-stream,alignment=au ! h264parse ! decodebin ! videoconvert ! videoscale ! videorate drop-only=true max-rate=15 ! video/x-raw,format=RGBA,width=[1,{EDGE}],height=[1,{EDGE}],pixel-aspect-ratio=1/1 ! appsink name=remote max-buffers=1 drop=true sync=false"
        )).map_err(|_| Error::Codec)?.downcast::<gst::Pipeline>().map_err(|_| Error::Codec)?;
        let decoder = Pipeline(decoder);
        let source = decoder
            .0
            .by_name("source")
            .ok_or(Error::Codec)?
            .downcast::<AppSrc>()
            .map_err(|_| Error::Codec)?;
        source.set_property_from_str("leaky-type", "downstream");
        let remote = decoder
            .0
            .by_name("remote")
            .ok_or(Error::Codec)?
            .downcast::<AppSink>()
            .map_err(|_| Error::Codec)?;
        decoder
            .0
            .set_state(gst::State::Playing)
            .map_err(|_| Error::Codec)?;
        capture
            .0
            .set_state(gst::State::Playing)
            .map_err(|_| Error::Device)?;
        Ok(Self {
            capture,
            decoder,
            encoded,
            preview,
            source,
            remote,
            orientation: 0,
            lost: false,
        })
    }
    fn pixels(sample: gst::Sample, orientation: u8) -> Result<Pixels, Error> {
        let caps = sample
            .caps()
            .and_then(|c| c.structure(0))
            .ok_or(Error::Device)?;
        let width = caps.get::<i32>("width").map_err(|_| Error::Device)? as u32;
        let height = caps.get::<i32>("height").map_err(|_| Error::Device)? as u32;
        if width == 0 || height == 0 || width > EDGE || height > EDGE {
            return Err(Error::Device);
        }
        let buffer = sample
            .buffer()
            .ok_or(Error::Device)?
            .map_readable()
            .map_err(|_| Error::Device)?;
        if buffer.len() != (width * height * 4) as usize {
            return Err(Error::Device);
        }
        let pixels = Pixels {
            width,
            height,
            rgba: buffer.as_slice().to_vec(),
        };
        Ok(crate::call_video::rotate(pixels, orientation))
    }
}
impl Device for Video {
    fn poll(&mut self) -> Result<Vec<Packet>, Error> {
        for pipeline in [&self.capture.0, &self.decoder.0] {
            if pipeline
                .bus()
                .ok_or(Error::Device)?
                .iter()
                .any(|m| matches!(m.view(), gst::MessageView::Error(_)))
            {
                return Err(Error::Device);
            }
        }
        let mut packets = Vec::new();
        if self.lost {
            packets.push(Packet::NeedKeyframe);
            self.lost = false;
        }
        if let Some(sample) = self.encoded.try_pull_sample(gst::ClockTime::ZERO) {
            let buffer = sample.buffer().ok_or(Error::Device)?;
            let timestamp = buffer.pts().map_or(0, |t| {
                (t.nseconds() as u128 * 90_000 / 1_000_000_000) as u32
            });
            let data = buffer
                .map_readable()
                .map_err(|_| Error::Device)?
                .as_slice()
                .to_vec();
            packets.push(Packet::Encoded(data, timestamp));
        }
        if let Some(sample) = self.preview.try_pull_sample(gst::ClockTime::ZERO) {
            packets.push(Packet::Local(Self::pixels(sample, 0)?));
        }
        if let Some(sample) = self.remote.try_pull_sample(gst::ClockTime::ZERO) {
            packets.push(Packet::Remote(Self::pixels(sample, self.orientation)?));
        }
        Ok(packets)
    }
    fn receive(&mut self, data: &[u8], orientation: u8) -> Result<(), Error> {
        self.orientation = orientation;
        if self.source.property::<u64>("current-level-bytes") as usize + data.len()
            > crate::call_video::MAX_UNIT
        {
            self.lost = true;
            return Ok(());
        }
        self.source
            .push_buffer(gst::Buffer::from_mut_slice(data.to_vec()))
            .map_err(|_| Error::Device)?;
        Ok(())
    }
    fn camera(&mut self, enabled: bool) -> Result<(), Error> {
        self.capture
            .0
            .set_state(if enabled {
                gst::State::Playing
            } else {
                gst::State::Null
            })
            .map_err(|_| Error::Device)?;
        Ok(())
    }
    fn keyframe(&mut self) {
        if let Some(encoder) = self.capture.0.by_name("encoder") {
            let event = gst::event::CustomUpstream::new(
                gst::Structure::builder("GstForceKeyUnit")
                    .field("running-time", u64::MAX)
                    .field("all-headers", true)
                    .field("count", 0u32)
                    .build(),
            );
            let _ = encoder.send_event(event);
        }
    }
}
