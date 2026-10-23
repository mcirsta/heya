//! Account chat settings and one explicitly requested remote action at a time.
use crate::archive::{Archive, ReadSync};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::watch;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Setting {
    Mute = 0,
    Archive = 1,
    Lock = 2,
}

#[derive(Clone)]
pub struct Update {
    pub jid: String,
    pub setting: Setting,
    /// Mute: zero, -1 (always), or epoch milliseconds. Other flags: zero/one.
    pub value: i64,
    /// None is unversioned history, which cannot overwrite an app-state action.
    pub stamp: Option<i64>,
}
impl Update {
    pub fn supersedes(&self, old: &Self) -> bool {
        old.stamp.is_none() || (self.stamp.is_some() && self.stamp >= old.stamp)
    }
}

#[derive(Clone, Copy)]
pub enum Action {
    MuteHours(u32),
    MuteAlways,
    Unmute,
    Archive(bool),
    Lock,
}
impl Action {
    pub fn from_menu(id: i32) -> Option<Self> {
        Some(match id {
            0 => Self::MuteHours(8),
            1 => Self::MuteHours(24 * 7),
            2 => Self::MuteAlways,
            3 => Self::Unmute,
            4 => Self::Archive(true),
            5 => Self::Archive(false),
            6 => Self::Lock,
            _ => return None,
        })
    }
    pub fn update(self, jid: String, now: i64) -> Update {
        let (setting, value) = match self {
            Self::MuteHours(hours) => (
                Setting::Mute,
                now.saturating_add(i64::from(hours) * 3_600_000),
            ),
            Self::MuteAlways => (Setting::Mute, -1),
            Self::Unmute => (Setting::Mute, 0),
            Self::Archive(value) => (Setting::Archive, i64::from(value)),
            Self::Lock => (Setting::Lock, 1),
        };
        Update {
            jid,
            setting,
            value,
            stamp: Some(now),
        }
    }
    pub fn success(self) -> &'static str {
        match self {
            Self::MuteHours(_) | Self::MuteAlways => "Chat muted.",
            Self::Unmute => "Chat unmuted.",
            Self::Archive(true) => "Chat archived.",
            Self::Archive(false) => "Chat restored.",
            Self::Lock => "Chat locked and hidden. Unlock it in WhatsApp on your phone.",
        }
    }
}
pub struct Target {
    pub jid: String,
    pub range: Option<ReadSync>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Offline,
    Unconfirmed,
    Failed,
}
impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::Offline => "Connect to manage chats. The change was not confirmed.",
            Self::Unconfirmed => {
                "The chat change was not confirmed. Check its state before trying again."
            }
            Self::Failed => "The chat change could not be saved. Please try again.",
        }
    }
}
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    async fn change(&self, target: Target, action: Action) -> Result<Update, Failure>;
}
pub type Sources = Option<Arc<dyn Source>>;
pub type Pending = Pin<Box<dyn Future<Output = Result<&'static str, Failure>> + Send>>;

pub fn request(
    db: Archive,
    sources: watch::Receiver<Sources>,
    chat: i32,
    action: Action,
) -> Pending {
    Box::pin(async move {
        let mut connection = sources;
        // The receiver may still have the initial Connected notification unread.
        // Capture that client and only cancel for a later connection change.
        let source = connection
            .borrow_and_update()
            .clone()
            .ok_or(Failure::Offline)?;
        let work = async {
            let target = db
                .chat_management_target(chat)
                .await
                .map_err(|_| Failure::Failed)?;
            let update = source.change(target, action).await?;
            drop(source);
            db.save_chat_settings(vec![update])
                .await
                .map_err(|_| Failure::Failed)?;
            Ok(action.success())
        };
        tokio::select! {
            biased;
            _ = connection.changed()=>Err(Failure::Offline),
            result = tokio::time::timeout(Duration::from_secs(30),work)=>result.unwrap_or(Err(Failure::Unconfirmed)),
        }
    })
}

// A history field is unsigned seconds on older clients and milliseconds on
// newer ones; the unsigned all-ones value represents an indefinite mute.
pub fn history_mute(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        -1
    } else if value < 1_000_000_000_000 {
        (value as i64).saturating_mul(1000)
    } else {
        value as i64
    }
}

#[cfg(test)]
#[path = "chat_management_tests.rs"]
mod tests;
