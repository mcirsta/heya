//! SDK-only boundary for own-device presence and profile text.
use crate::own_account::{Source, Visibility};
use std::sync::Arc;
pub struct Account(pub Arc<whatsapp_rust::Client>);
#[async_trait::async_trait]
impl Source for Account {
    async fn presence(&self, visibility: Visibility) -> Result<(), &'static str> {
        self.0
            .wait_for_session_ready(std::time::Duration::from_secs(8))
            .await
            .map_err(|_| "WhatsApp is not ready. Please try again shortly.")?;
        self.0
            .presence()
            .set(match visibility {
                Visibility::Online => whatsapp_rust::PresenceStatus::Available,
                Visibility::Invisible => whatsapp_rust::PresenceStatus::Unavailable,
            })
            .await
            .map_err(|_| "Your visibility could not be updated. Please try again.")
    }
    async fn about(&self) -> Result<String, &'static str> {
        let jid = self
            .0
            .persistence_manager()
            .get_device_snapshot()
            .pn
            .clone()
            .ok_or("Your profile is not ready yet.")?
            .to_non_ad();
        let users = self
            .0
            .contacts()
            .get_user_info(std::slice::from_ref(&jid))
            .await
            .map_err(|_| "Your status message could not be loaded.")?;
        users
            .get(&jid)
            .and_then(|info| info.status.clone())
            .ok_or("Your status message is unavailable.")
    }
    async fn save_about(&self, text: &str) -> Result<(), &'static str> {
        self.0
            .profile()
            .set_status_text(text)
            .await
            .map_err(|_| "Your status message could not be saved. Your text has been kept.")
    }
}
