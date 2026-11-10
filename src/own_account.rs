//! Own-device visibility and profile About. One finite operation at a time.
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Online,
    Invisible,
}

#[cfg(test)]
#[path = "own_account_tests.rs"]
mod tests;
impl Visibility {
    pub fn from_preference(invisible: bool) -> Self {
        if invisible {
            Self::Invisible
        } else {
            Self::Online
        }
    }
}
#[derive(Clone)]
pub enum Action {
    Presence(Visibility),
    LoadAbout,
    SaveAbout(String),
}
pub struct Command {
    pub request: i32,
    pub action: Action,
}
#[derive(Clone)]
pub struct Reply {
    pub request: i32,
    pub kind: i32,
    pub error: String,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub connected: bool,
    pub visibility: Option<Visibility>,
    pub about: Option<String>,
    pub busy: bool,
    pub warning: String,
    // Automatic reconnect updates must not erase a user's completion.
    pub reply: Option<Reply>,
}
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    async fn presence(&self, visibility: Visibility) -> Result<(), &'static str>;
    async fn about(&self) -> Result<String, &'static str>;
    async fn save_about(&self, text: &str) -> Result<(), &'static str>;
}
pub type Sources = Option<Arc<dyn Source>>;

pub async fn run(
    mut commands: mpsc::Receiver<Command>,
    mut sources: watch::Receiver<Sources>,
    mut preferences: watch::Receiver<crate::preferences::Preferences>,
    updates: watch::Sender<Snapshot>,
) {
    sources.mark_changed();
    let mut persisted = preferences.borrow().appear_offline;
    let mut desired = Visibility::from_preference(persisted);
    let mut state = Snapshot::default();
    let mut preferences_open = true;
    loop {
        let (request, action) = tokio::select! {
            biased;
            changed = preferences.changed(), if preferences_open => {
                if changed.is_err() { preferences_open = false; continue; }
                let next = preferences.borrow_and_update().appear_offline;
                if next == persisted { continue; }
                persisted = next;
                desired = Visibility::from_preference(next);
                if !state.connected || state.visibility == Some(desired) { continue; }
                (None, Action::Presence(desired))
            }
            changed = sources.changed() => {
                if changed.is_err() { return; }
                state.connected = sources.borrow_and_update().is_some();
                state.visibility = None;
                if !state.connected {
                    state.busy = false;
                    state.about = None;
                    state.warning.clear();
                    updates.send_replace(state.clone());
                    continue;
                }
                (None, Action::Presence(desired))
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                (Some(command.request), command.action)
            }

        };
        let kind = match action {
            Action::Presence(_) => 0,
            Action::LoadAbout => 1,
            Action::SaveAbout(_) => 2,
        };
        let source = sources.borrow().clone();
        state.connected = source.is_some();
        state.busy = true;
        if kind == 0 {
            state.warning.clear();
        }
        updates.send_replace(state.clone());
        let result = if let Some(source) = source {
            let mut connection = sources.clone();
            // This clone owns the observed generation; the original still sees
            // disconnect/reconnect after a cancelled operation.
            let work = async {
                match &action {
                    Action::Presence(visibility) => {
                        source.presence(*visibility).await.map(|_| None)
                    }
                    Action::LoadAbout => source.about().await.map(Some),
                    Action::SaveAbout(text) => {
                        if text.chars().count() > 139 {
                            return Err("The status message can contain up to 139 characters.");
                        }
                        source.save_about(text).await.map(|_| Some(text.clone()))
                    }
                }
            };
            tokio::select! {
                biased;
                _ = connection.changed() => Err("The connection changed. Please try again."),
                result = tokio::time::timeout(Duration::from_secs(12), work) => result.unwrap_or(Err("WhatsApp did not respond. Please try again.")),
            }
        } else {
            Err("Connect to WhatsApp first.")
        };
        state.busy = false;
        match result {
            Ok(about) => {
                if let Action::Presence(visibility) = action {
                    desired = visibility;
                    state.visibility = Some(visibility);
                }
                if let Some(about) = about {
                    state.about = Some(about);
                }
                if let Some(request) = request {
                    state.reply = Some(Reply {
                        request,
                        kind,
                        error: String::new(),
                    });
                }
            }
            Err(error) => {
                if kind == 0 {
                    state.warning = error.into();
                }
                if let Some(request) = request {
                    state.reply = Some(Reply {
                        request,
                        kind,
                        error: error.into(),
                    });
                }
            }
        }
        updates.send_replace(state.clone());
    }
}
