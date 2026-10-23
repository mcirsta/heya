//! Application-owned connection state. No toolkit or protocol types cross here.
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    DataDirectory,
    AlreadyRunning,
    Keyring,
    MissingKey,
    InvalidKey,
    Database,
    Protocol,
    Pairing,
    LoggedOut,
    Outdated,
    Conflict,
    QrExpired,
    Shutdown,
    Archive,
    ArchiveIdentity,
    LogoutUnconfirmed,
}

impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::Archive => {
                "The encrypted chat archive could not be opened or saved. Check the keyring, free space and permissions before reconnecting. Unconfirmed sends have been retained."
            }
            Self::ArchiveIdentity => {
                "This archive belongs to a different account. The connection has been stopped and the archive left intact."
            }
            Self::DataDirectory => {
                "Heya could not open its private data folder. Check folder permissions and available disk space."
            }
            Self::AlreadyRunning => {
                "This session is already open in another Heya window. Close that window before reconnecting."
            }
            Self::Keyring => {
                "The desktop keyring is unavailable or locked. Unlock it, then try again."
            }
            Self::MissingKey => {
                "A saved database's encryption key is missing from the desktop keyring. Your saved data has been left untouched."
            }
            Self::InvalidKey => {
                "The saved encryption key is invalid. Your saved data has been left untouched."
            }
            Self::Database => {
                "The encrypted session could not be opened. Check the keyring, disk space and folder permissions. The session has not been reset."
            }
            Self::Protocol => {
                "The connection stopped. Check your internet connection, then try again."
            }
            Self::Pairing => {
                "The phone could not complete pairing. Try again from WhatsApp's Linked devices screen."
            }
            Self::LoggedOut => {
                "This device is no longer linked. Link the same WhatsApp account again to keep using your saved chats."
            }
            Self::LogoutUnconfirmed => {
                "WhatsApp did not confirm unlinking this device. The connection is closed and your data is retained. Reconnect to try again, or remove Heya from Linked devices on your phone."
            }
            Self::Outdated => {
                "WhatsApp requires a newer protocol version. Heya needs an update before connecting."
            }
            Self::Conflict => {
                "This linked session was opened elsewhere. Close the other connection before reconnecting."
            }
            Self::QrExpired => "The pairing codes expired. Connect again for a new code.",
            Self::Shutdown => {
                "The session did not finish saving or releasing its files. Close Heya before trying again."
            }
        }
    }
}

/// Latest desired lifecycle action. Relink is accepted only after a stopped,
/// revoked or mismatched session; it never resets a running protocol client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Idle,
    Startup,
    OpenOffline,
    Connect,
    Disconnect,
    Relink,
    Logout,
}
impl Request {
    /// Manual startup waits without touching the vault. An explicit offline
    /// launch still opens the encrypted archive and therefore needs its key.
    pub fn startup(offline: bool, connect_automatically: bool) -> Self {
        if offline {
            Self::OpenOffline
        } else if connect_automatically {
            Self::Startup
        } else {
            Self::Idle
        }
    }

    pub fn connects(self) -> bool {
        matches!(self, Self::Startup | Self::Connect | Self::Relink)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Opening,
    Connecting,
    Qr,
    Pairing,
    Connected,
    Reconnecting,
    Stopping,
    Stopped,
    Failed(Failure),
}

// Intentionally no Debug implementation: a QR image is an account credential.
#[derive(Clone)]
pub struct QrImage {
    pub width: u32,
    pub pixels: Arc<[u8]>,
}

#[derive(Clone)]
pub struct Snapshot {
    pub phase: Phase,
    pub qr: Option<QrImage>,
    pub seconds_left: u32,
}

impl Snapshot {
    pub fn new(phase: Phase) -> Self {
        Self {
            phase,
            qr: None,
            seconds_left: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Request;

    #[test]
    fn startup_respects_the_preference_and_explicit_offline_launches() {
        assert_eq!(Request::startup(false, true), Request::Startup);
        assert_eq!(Request::startup(false, false), Request::Idle);
        for connect_automatically in [false, true] {
            let request = Request::startup(true, connect_automatically);
            assert_eq!(request, Request::OpenOffline);
            assert!(!request.connects());
        }
        assert!(
            Request::Connect.connects(),
            "manual Connect is always available"
        );
    }
}
