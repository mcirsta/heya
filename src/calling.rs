//! One active call. Protocol objects and platform audio stay behind narrow ports.
use crate::{
    archive::Archive,
    call_audio::{self, Ports},
    call_tones::{self, Tone},
    preferences::Preferences,
    voice_recorder::{Error as AudioError, Input},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub chat: i32,
    pub peer: String,
    pub title: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub id: String,
    pub target: Target,
    pub expires: tokio::time::Instant,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Audio(AudioError),
    Offline,
    Unavailable,
    Network,
    Timeout,
    Declined,
}
impl Error {
    pub fn label(self) -> &'static str {
        match self {
            Self::Audio(AudioError::NoInput) => AudioError::NoInput.label(),
            Self::Audio(AudioError::Permission) => AudioError::Permission.label(),
            Self::Audio(AudioError::Disabled) => "Audio calling is disabled in this build.",
            Self::Audio(AudioError::Unresponsive) => AudioError::Unresponsive.label(),
            Self::Audio(_) => {
                "Call audio is unavailable. Check your microphone, speakers and audio settings."
            }
            Self::Offline => "Connect to WhatsApp before making a call.",
            Self::Unavailable => "Audio calling is unavailable for this conversation.",
            Self::Network => "The call could not connect. Please try again.",
            Self::Timeout => "The call was not answered.",
            Self::Declined => "Call declined.",
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Accepted,
    Connected,
    Ended,
    Failed(Error),
}
#[async_trait::async_trait]
pub trait Session: Send + Sync {
    async fn event(&mut self) -> Event;
    async fn set_muted(&self, muted: bool);
    fn muted(&self) -> bool;
    async fn end(&self);
}
#[async_trait::async_trait]
pub trait Network: Send + Sync {
    fn offers(&self) -> watch::Receiver<Option<Offer>>;
    async fn open(
        &self,
        target: &Target,
        offer: Option<&str>,
        ports: Ports,
    ) -> Result<Box<dyn Session>, Error>;
    async fn decline(&self, id: &str);
}
pub type Sources = Option<Arc<dyn Network>>;

// A fast answer/failure may already be queued when offer sending completes.
// Consume it before starting ringback; when no sound is pending, only wait.
pub(crate) async fn event_or_ring(call: &mut dyn Session, ring: bool) -> Option<Event> {
    tokio::select! {
        biased;
        event = call.event() => Some(event),
        _ = std::future::ready(()), if ring => None,
    }
}
pub enum Command {
    Start(i32),
    Answer(String),
    Decline(String),
    Mute,
    Dismiss(String),
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub chat: i32,
    pub offer: String,
    pub available: bool,
    /// 0 idle, 1 incoming, 2 preparing, 3 calling, 4 connecting, 5 active, 6 ended.
    /// Outgoing preparation stays outside the call panel until setup succeeds.
    pub phase: i32,
    pub title: String,
    pub seconds: u64,
    pub muted: bool,
    pub can_mute: bool,
    pub error: Option<Error>,
}
fn publish(updates: &watch::Sender<Status>, status: &Status) {
    updates.send_if_modified(|current| {
        if current != status {
            *current = status.clone();
            true
        } else {
            false
        }
    });
}
pub async fn run(
    archives: watch::Receiver<Option<Archive>>,
    sources: watch::Receiver<Sources>,
    commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Status>,
    settings: watch::Receiver<Preferences>,
    input: Input,
    stopped: watch::Receiver<bool>,
) {
    Controller {
        archives,
        sources,
        commands,
        updates,
        settings,
        input,
        stopped,
        status: Status::default(),
    }
    .run()
    .await;
}
struct Controller {
    archives: watch::Receiver<Option<Archive>>,
    sources: watch::Receiver<Sources>,
    commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Status>,
    settings: watch::Receiver<Preferences>,
    input: Input,
    stopped: watch::Receiver<bool>,
    status: Status,
}
impl Controller {
    async fn run(mut self) {
        let mut provider: Sources = None;
        let mut incoming = watch::channel(None).1;
        let mut ringtone = None;
        loop {
            let latest = self.sources.borrow_and_update().clone();
            let same = match (&provider, &latest) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                provider = latest;
                incoming = provider
                    .as_ref()
                    .map(|s| s.offers())
                    .unwrap_or_else(|| watch::channel(None).1);
                incoming.mark_changed();
                self.status.available = provider.is_some()
                    && (crate::audio::SUPPORTED || !matches!(self.input, Input::Microphone));
                if provider.is_none() && self.status.phase == 1 {
                    call_tones::stop(&mut ringtone).await;
                    self.status.phase = 6;
                    self.status.error = Some(Error::Offline);
                }
                publish(&self.updates, &self.status);
            }
            let incoming_deadline = incoming.borrow().as_ref().map(|o| o.expires);
            if self.status.phase == 1
                && self.status.error.is_none()
                && incoming_deadline.is_some_and(|d| d > tokio::time::Instant::now())
            {
                if ringtone.is_none() {
                    ringtone = call_tones::start(
                        Tone::Incoming,
                        self.settings.borrow().audio.clone(),
                        self.input,
                    );
                }
            } else {
                call_tones::stop(&mut ringtone).await;
            }
            let command = tokio::select! {
                biased;
                command=self.commands.recv()=>match command {Some(c)=>Some(c),None=>break},
                changed=self.sources.changed()=>{
                    if changed.is_err(){break;}
                    None
                },
                changed=incoming.changed(),if provider.is_some()=>{
                    if changed.is_err(){provider=None;continue;}
                    let offer=incoming.borrow_and_update().clone();
                    if let Some(offer)=offer {
                    self.status=Status{chat:offer.target.chat,offer:offer.id,available:self.status.available,phase:1,title:offer.target.title,error:(!self.status.available).then_some(Error::Audio(AudioError::Disabled)),..Default::default()};
                    } else if self.status.phase==1 {call_tones::stop(&mut ringtone).await;self.status.phase=6;self.status.error=None;}
                    publish(&self.updates,&self.status);
                    None
                },
                _=async {match incoming_deadline{Some(d)=>tokio::time::sleep_until(d).await,None=>std::future::pending().await}},if self.status.phase==1=>{
                    call_tones::stop(&mut ringtone).await;
                    let offer=incoming.borrow().clone();
                    if let (Some(provider),Some(offer))=(&provider,offer){let _=tokio::time::timeout(Duration::from_secs(3),provider.decline(&offer.id)).await;}
                    self.status.phase=6;self.status.error=Some(Error::Timeout);publish(&self.updates,&self.status);None
                },
                changed=self.archives.changed()=>{if changed.is_err(){break;}None},
            };
            let Some(command) = command else { continue };
            match command {
                Command::Start(chat) => {
                    let archive = self.archives.borrow().clone();
                    let target = match archive {
                        Some(a) => a.call_target(chat).await.map_err(|_| Error::Unavailable),
                        None => Err(Error::Offline),
                    };
                    match (provider.as_ref(), target) {
                        (Some(provider), Ok(target)) => {
                            call_tones::stop(&mut ringtone).await;
                            if !self.one_call(provider.clone(), target, None).await {
                                break;
                            }
                        }
                        (_, target) => {
                            self.status = Status {
                                available: self.status.available,
                                error: Some(target.err().unwrap_or(Error::Offline)),
                                ..Default::default()
                            };
                            publish(&self.updates, &self.status);
                        }
                    }
                }
                Command::Answer(id) => {
                    let offer = incoming.borrow().clone().filter(|o| o.id == id);
                    if let (Some(provider), Some(offer)) = (&provider, offer) {
                        call_tones::stop(&mut ringtone).await;
                        if !self
                            .one_call(provider.clone(), offer.target, Some(offer.id))
                            .await
                        {
                            break;
                        }
                    }
                }
                Command::Decline(id) => {
                    let offer = incoming.borrow().clone().filter(|o| o.id == id);
                    if offer.is_none() {
                        continue;
                    }
                    call_tones::stop(&mut ringtone).await;
                    if let (Some(provider), Some(offer)) = (&provider, offer) {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(3),
                            provider.decline(&offer.id),
                        )
                        .await;
                    }
                    self.status.phase = 6;
                    self.status.error = None;
                    publish(&self.updates, &self.status);
                }
                Command::Dismiss(id) => {
                    if self.status.phase != 6 || self.status.offer != id {
                        continue;
                    }
                    self.status = Status {
                        available: self.status.available,
                        ..Default::default()
                    };
                    publish(&self.updates, &self.status);
                }
                Command::Mute => {}
            }
        }
        call_tones::stop(&mut ringtone).await;
    }
    async fn one_call(
        &mut self,
        provider: Arc<dyn Network>,
        target: Target,
        offer: Option<String>,
    ) -> bool {
        let Self {
            sources,
            commands,
            updates,
            settings,
            input,
            stopped,
            status,
            ..
        } = self;
        let input = *input;

        *status = Status {
            chat: target.chat,
            offer: offer.clone().unwrap_or_default(),
            available: status.available,
            phase: 2,
            title: target.title.clone(),
            ..Default::default()
        };
        publish(updates, status);
        if *stopped.borrow_and_update() {
            status.phase = if offer.is_some() { 6 } else { 0 };
            publish(updates, status);
            return true;
        }
        let (mut audio, ports) =
            match call_audio::Worker::start(settings.borrow().audio.clone(), input) {
                Ok(p) => p,
                Err(e) => {
                    status.phase = if offer.is_some() { 1 } else { 0 };
                    status.error = Some(Error::Audio(e));
                    publish(updates, status);
                    return true;
                }
            };
        let mut open = true;
        let mut cancelled = false;
        let result = {
            // The deadline must include device discovery and opening the device:
            // a plugin can block before it ever produces a microphone frame.
            let ready = tokio::time::timeout(Duration::from_secs(8), audio.ready());
            let starting = provider.open(&target, offer.as_deref(), ports);
            tokio::pin!(ready, starting);
            let mut audio_ready = false;
            let deadline = tokio::time::sleep(Duration::from_secs(90));
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    biased;
                    _=stopped.changed()=>{cancelled=true;break Err(Error::Network);},
                    command=commands.recv()=>match command {
                        Some(Command::Decline(id)) if offer.as_deref()==Some(id.as_str())=>{cancelled=true;break Err(Error::Network);},
                        Some(_)=>{},
                        None=>{open=false;cancelled=true;break Err(Error::Offline);},
                    },
                    _=sources.changed()=>{cancelled=true;break Err(Error::Offline);},
                    result=&mut ready,if !audio_ready=>match result {
                        Ok(Ok(()))=>{audio_ready=true;status.phase=if offer.is_some(){4}else{3};publish(updates,status);},
                        Ok(Err(error))=>break Err(Error::Audio(error)),
                        Err(_)=>break Err(Error::Audio(AudioError::Unresponsive)),
                    },
                    result=audio.finished(),if audio_ready=>break Err(Error::Audio(result.err().unwrap_or(AudioError::Device))),
                    result=&mut starting,if audio_ready=>break result,
                    _=&mut deadline=>break Err(Error::Network),
                }
            }
        };
        let mut call = match result {
            Ok(call) => call,
            Err(error) => {
                if !cancelled {
                    // Fixed categories only: never log SDK errors or device/media data.
                    eprintln!("Heya call setup failed: {error:?}");
                }
                audio.stop().await;
                if cancelled && let Some(id) = &offer {
                    let _ =
                        tokio::time::timeout(Duration::from_secs(3), provider.decline(id)).await;
                }
                status.phase = if offer.is_none() {
                    0
                } else if matches!(error, Error::Audio(_)) {
                    1
                } else {
                    6
                };
                status.error = (!cancelled || error == Error::Offline).then_some(error);
                publish(updates, status);
                return open;
            }
        };
        status.can_mute = true;
        status.muted = call.muted();
        status.phase = if offer.is_some() { 4 } else { 3 };
        publish(updates, status);
        // Only ring back after the SDK sent the offer. Sound playback opens the
        // selected speaker in its own finite helper; it never opens a microphone.
        let mut ringback = None;
        let mut start_ringback = offer.is_none();
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let ringing = tokio::time::sleep(Duration::from_secs(60));
        tokio::pin!(ringing);
        let mut connected: Option<Instant> = None;
        loop {
            tokio::select! {
                biased;
                _=stopped.changed()=>break,
                command=commands.recv()=>match command {
                    Some(Command::Mute)=>{
                        let _=tokio::time::timeout(Duration::from_secs(1),call.set_muted(!call.muted())).await;
                        status.muted=call.muted();publish(updates,status);
                    },
                    None=>{open=false;break;},
                    Some(Command::Decline(id)) if offer.as_deref()==Some(id.as_str())=>break,
                    _=>{},
                },
                _=sources.changed()=>{status.error=Some(Error::Offline);break;},
                result=audio.finished()=>{if let Err(error)=result {status.error=Some(Error::Audio(error));}break;},
                _=&mut ringing,if connected.is_none()=>{status.error=Some(Error::Timeout);break;},
                event=event_or_ring(call.as_mut(),start_ringback)=>match event {
                    None=>{start_ringback=false;ringback=call_tones::start(Tone::Outgoing,settings.borrow().audio.clone(),input);},
                    Some(Event::Accepted)=>{start_ringback=false;call_tones::stop(&mut ringback).await;status.phase=4;publish(updates,status);},
                    Some(Event::Connected)=>{start_ringback=false;call_tones::stop(&mut ringback).await;connected.get_or_insert_with(Instant::now);status.phase=5;publish(updates,status);},
                    Some(Event::Ended)=>break,
                    Some(Event::Failed(error))=>{status.error=Some(error);break;},
                },
                _=timer.tick(),if connected.is_some()=>{
                    status.seconds=connected.map_or(0,|i|i.elapsed().as_secs());publish(updates,status);
                },
            }
        }
        // End transport and device work together; no PCM/call task survives dismissal.
        tokio::join!(audio.stop(), call_tones::stop(&mut ringback), async {
            let _ = tokio::time::timeout(Duration::from_secs(3), call.end()).await;
        });
        status.can_mute = false;
        status.phase = 6;
        publish(updates, status);
        open
    }
}
