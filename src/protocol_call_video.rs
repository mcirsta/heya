//! H.264 endpoint conversion at the protocol boundary. No camera or UI objects.
use crate::{call_video, calling::Error};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use whatsapp_rust::voip::{CallHandle, TimedVideoFrame, VideoFrame, VideoSource};

#[derive(Clone)]
pub struct Source(async_channel::Receiver<TimedVideoFrame>);
impl VideoSource for Source {
    fn frames(&self) -> async_channel::Receiver<Vec<u8>> {
        let (_, empty) = async_channel::bounded(1);
        empty.close();
        empty
    }
    fn timed_frames(&self) -> Option<async_channel::Receiver<TimedVideoFrame>> {
        Some(self.0.clone())
    }
}
pub struct Video {
    pub source: Source,
    pub sink: async_channel::Sender<VideoFrame>,
    outgoing: async_channel::Sender<TimedVideoFrame>,
    incoming: async_channel::Receiver<VideoFrame>,
    ports: call_video::Ports,
    sending: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Video {
    pub fn new(ports: call_video::Ports) -> Self {
        let (outgoing, capture) = async_channel::bounded(2);
        let (sink, incoming) = async_channel::bounded(2);
        Self {
            source: Source(capture),
            sink,
            outgoing,
            incoming,
            ports,
            sending: Arc::new(AtomicBool::new(true)),
            task: None,
        }
    }
    pub fn start(&mut self, handle: CallHandle) {
        let (ports, outgoing, incoming, sending) = (
            self.ports.clone(),
            self.outgoing.clone(),
            self.incoming.clone(),
            self.sending.clone(),
        );
        self.task = Some(tokio::spawn(async move {
            let mut need_keyframe = true;
            loop {
                tokio::select! {
                    biased;
                    _ = handle.wait_ended() => break,
                    request = ports.peer_keyframes.recv() => {
                        if request.is_err() {break;}
                        handle.request_peer_keyframe(whatsapp_rust::wacore::voip_control::MediaKeyframeUrgency::Immediate);
                    },
                    packet = ports.capture.recv() => {
                        let Ok(packet) = packet else {break;};
                        if !sending.load(Ordering::Acquire) {need_keyframe = true; continue;}
                        if need_keyframe && !packet.keyframe {ports.keyframe(); continue;}
                        let frame = TimedVideoFrame::builder().data(packet.data).timestamp(packet.timestamp).build();
                        need_keyframe = outgoing.try_send(frame).is_err();
                        if need_keyframe {ports.keyframe();}
                    },
                    packet = incoming.recv() => {
                        let Ok(packet) = packet else {break;};
                        if packet.data.len() > call_video::MAX_UNIT || packet.orientation > 3 {continue;}
                        let packet = call_video::Encoded {data: packet.data, timestamp: packet.timestamp, orientation: packet.orientation, keyframe: packet.keyframe};
                        if ports.playout.try_send(packet).is_err() {
                            handle.request_peer_keyframe(whatsapp_rust::wacore::voip_control::MediaKeyframeUrgency::Immediate);
                        }
                    },
                }
            }
        }));
    }
    pub async fn camera(&mut self, handle: &CallHandle, enabled: bool) -> Result<(), Error> {
        self.sending.store(false, Ordering::Release);
        if enabled {
            while self.source.0.try_recv().is_ok() {}
            handle
                .resume_video(self.source.clone(), self.sink.clone())
                .await
                .map_err(|_| Error::Network)?;
            self.sending.store(true, Ordering::Release);
            self.ports.keyframe();
        } else {
            handle.stop_video().await.map_err(|_| Error::Network)?;
        }
        Ok(())
    }
    pub async fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}
impl Drop for Video {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
