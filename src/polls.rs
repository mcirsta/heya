//! Poll definitions and derived results. Secrets remain in opaque envelopes.
use serde::{Deserialize, Serialize};
pub const MAX_OPTIONS: usize = 12;
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Poll {
    pub question: String,
    pub options: Vec<String>,
    pub selectable: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<i64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub restriction: String,
    pub ready: bool,
    #[serde(skip)]
    pub view: View,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub counts: Vec<i32>,
    pub selected: Vec<usize>,
    pub voters: usize,
    pub busy: bool,
    pub status: String,
    pub can_vote: bool,
}
/// History supplies authenticated plaintext hashes; live updates retain ciphertext.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ballot {
    pub target: String,
    pub author: String,
    pub remote: String,
    pub stamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashes: Option<Vec<Vec<u8>>>,
}
#[derive(Clone, Default)]
pub struct Draft {
    pub question: String,
    pub options: Vec<String>,
    pub multiple: bool,
}
impl Draft {
    pub fn valid(&self) -> bool {
        !self.question.trim().is_empty()
            && self.question.chars().count() <= 255
            && (2..=MAX_OPTIONS).contains(&self.options.len())
            && self
                .options
                .iter()
                .all(|o| !o.trim().is_empty() && o.chars().count() <= 100)
            && self
                .options
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == self.options.len()
    }
}
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Voter {
    pub jid: String,
    pub name: String,
    pub detail: String,
}
#[derive(Clone, Default)]
pub struct Results {
    pub message: i32,
    pub question: String,
    pub options: Vec<String>,
    pub counts: Vec<i32>,
    pub option: usize,
    pub voters: Vec<Voter>,
    pub more: bool,
}

#[cfg(test)]
#[path = "poll_tests.rs"]
mod tests;
