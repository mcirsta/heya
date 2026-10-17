//! Application-owned values for explicit message operations.
use crate::message_content::Content;

pub const EDIT_WINDOW: i64 = 15 * 60;
pub const REVOKE_WINDOW: i64 = 2 * 24 * 60 * 60;
pub const MAX_SELECTION: usize = 100;

pub fn within_window(stamp: i64, now: i64, window: i64) -> bool {
    stamp > 0 && now >= stamp && now - stamp <= window
}
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub wire: String,
    pub stamp: i64,
    pub delivery: String,
    pub pending: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    Edit,
    Revoke,
    Hide,
}
impl ControlKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Revoke => "Delete",
            Self::Hide => "Delete for me",
        }
    }
}
#[derive(Clone)]
pub struct Control {
    pub kind: ControlKind,
    pub wire: String,
    pub sender: String,
    pub outgoing: bool,
    pub stamp: i64,
    pub text: String,
    pub version: i64,
}
#[derive(Clone, Default)]
pub struct InfoRow {
    pub name: String,
    pub detail: String,
}
#[derive(Clone, Default)]
pub struct Info {
    pub message: i32,
    pub wire: String,
    pub sent: String,
    pub status: String,
    pub rows: Vec<InfoRow>,
}

// Deliberately no Debug: raw envelopes and previews are private account data.
#[derive(Clone)]
pub struct Source {
    pub id: i32,
    pub chat: i32,
    pub jid: String,
    pub sender: String,
    pub wire: String,
    pub outgoing: bool,
    pub body: String,
    pub raw: Vec<u8>,
}
pub enum Operation {
    Reply(String),
    React(String),
    Forward,
}
pub struct Prepared {
    pub raw: Vec<u8>,
    pub text: String,
    pub content: Content,
    pub thumbnail: Vec<u8>,
    pub visible: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplyDraft {
    pub id: i32,
    pub sender: String,
    pub text: String,
}
#[derive(Clone, Default)]
pub struct Destination {
    pub id: i32,
    pub title: String,
}
#[derive(Clone, Default)]
pub struct OperationReply {
    pub request: i32,
    pub kind: u8,
    pub saved: bool,
    pub error: String,
    pub info: Option<Info>,
}
pub enum Command {
    Edit {
        chat: i32,
        message: i32,
        expected: String,
        text: String,
        request: i32,
    },
    Revoke {
        chat: i32,
        message: i32,
        request: i32,
    },
    Hide {
        chat: i32,
        messages: Vec<i32>,
        request: i32,
    },
    Info {
        chat: i32,
        message: i32,
        request: i32,
    },
    CloseInfo,
    ForwardMany {
        chat: i32,
        messages: Vec<i32>,
        destination: i32,
        request: i32,
    },
    Reply {
        chat: i32,
        message: i32,
        request: i32,
    },
    React {
        chat: i32,
        message: i32,
        emoji: String,
        request: i32,
    },
    Forward {
        chat: i32,
        message: i32,
        destination: i32,
        request: i32,
    },
    Destinations {
        request: i32,
    },
}
