//! Toolkit/SDK-independent display metadata and explicit external actions.
use serde::{Deserialize, Serialize};

pub const MAX_ACTIONS: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    #[serde(skip)]
    pub metadata: crate::message_actions::Metadata,
    /// A timeline event, not a message that can be replied to or forwarded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<Notice>,
    /// Business body without flattened actions. `Message.text` keeps the full
    /// copyable transcript, including action destinations.
    pub body: Option<String>,
    pub preview: Option<Preview>,
    #[serde(default)]
    pub photo: Option<Photo>,
    #[serde(default)]
    pub audio: Option<Audio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<Document>,
    /// Attachments without a dedicated photo/audio projection. Never inferred
    /// from body text. Omit false to preserve existing media cache fingerprints.
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_attachment: bool,
    pub actions: Vec<Action>,
    #[serde(default)]
    pub quote: Option<Quote>,
    #[serde(default)]
    pub forwarded: bool,
    #[serde(default)]
    pub reaction: Option<ReactionEvent>,
    // Joined on the archive worker, never serialized into the original envelope.
    #[serde(skip)]
    pub reaction_summary: String,
    #[serde(skip)]
    pub own_reaction: String,
    #[serde(skip)]
    pub reaction_status: String,
    #[serde(skip)]
    pub unavailable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notice {
    MessagePinned,
    MessageUnpinned,
}
impl Notice {
    pub fn description(self) -> &'static str {
        match self {
            Self::MessagePinned => "pinned a message",
            Self::MessageUnpinned => "unpinned a message",
        }
    }
    pub fn label(self, sender: &str) -> String {
        format!("{sender} {}", self.description())
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub sender: String,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionEvent {
    pub target: String,
    pub emoji: String,
    pub at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audio {
    pub seconds: Option<u32>,
    pub voice: bool,
    pub size: Option<u64>,
    /// Sender-supplied levels, bounded to 64 bars. No decoding during layout.
    pub waveform: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Photo {
    pub width: u32,
    pub height: u32,
    pub size: Option<u64>,
}

/// Sender-supplied metadata only. Displaying a card never opens the document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub file_name: String,
    pub mime_type: String,
    pub size: Option<u64>,
    pub pages: Option<u32>,
    pub has_thumbnail: bool,
}

impl Document {
    pub fn file_type(&self) -> String {
        let mime = self.mime_type.split(';').next().unwrap_or("").trim();
        let known = match mime.to_ascii_lowercase().as_str() {
            "application/pdf" => "PDF",
            "application/msword" => "DOC",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "DOCX",
            "application/vnd.ms-excel" => "XLS",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "XLSX",
            "application/vnd.ms-powerpoint" => "PPT",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "PPTX",
            "text/plain" => "TXT",
            "text/csv" => "CSV",
            "application/zip" => "ZIP",
            _ => "",
        };
        if !known.is_empty() {
            return known.into();
        }
        self.file_name
            .rsplit_once('.')
            .map(|(_, ext)| ext)
            .filter(|ext| {
                !ext.is_empty() && ext.len() <= 8 && ext.bytes().all(|c| c.is_ascii_alphanumeric())
            })
            .map(str::to_ascii_uppercase)
            .unwrap_or_else(|| "File".into())
    }

    pub fn details(&self) -> String {
        let mut parts = Vec::with_capacity(3);
        if let Some(pages) = self.pages.filter(|p| *p > 0) {
            parts.push(format!("{pages} page{}", if pages == 1 { "" } else { "s" }));
        }
        parts.push(self.file_type());
        if let Some(bytes) = self.size {
            parts.push(if bytes < 1000 {
                format!("{bytes} B")
            } else if bytes < 1_000_000 {
                format!("{:.0} kB", bytes as f64 / 1000.)
            } else if bytes < 1_000_000_000 {
                format!("{:.1} MB", bytes as f64 / 1_000_000.)
            } else {
                format!("{:.1} GB", bytes as f64 / 1_000_000_000.)
            });
        }
        parts.join(" • ")
    }

    pub fn transcript(&self, caption: &str) -> String {
        let mut text = format!("[Document] {}", self.file_name);
        if !caption.is_empty() {
            text.push('\n');
            text.push_str(caption);
        }
        text
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub url: String,
    pub title: String,
    pub description: String,
    pub has_thumbnail: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub label: String,
    pub target: Target,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Url(String),
    Copy(String),
    #[default]
    Unavailable,
}

pub fn web_url(value: &str) -> Option<String> {
    if value.len() > 8192
        || value.chars().any(|c| c.is_control() || c == '\\')
        || value.trim().chars().any(char::is_whitespace)
    {
        return None;
    }
    let url = url::Url::parse(value.trim()).ok()?;
    (matches!(url.scheme(), "https" | "http")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}

pub fn caption(value: &str, limit: usize) -> String {
    // Presentation summaries never become executable destinations.
    value
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .take(limit)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn first_link(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|word| {
        web_url(word.trim_matches(|c| matches!(c, '<' | '>' | '"' | '(' | ')' | ',')))
    })
}

pub fn action(label: Option<&str>, value: Option<&str>, kind: &str) -> Action {
    let target = match (kind, value.filter(|v| !v.trim().is_empty())) {
        ("url", Some(value)) => web_url(value).map(Target::Url).unwrap_or_default(),
        ("copy", Some(value)) if value.len() <= 16 * 1024 => Target::Copy(value.into()),
        // Dial/reply/payment/flow handling stays explicit until implemented.
        _ => Target::Unavailable,
    };
    Action {
        label: caption(
            label
                .filter(|v| !v.trim().is_empty())
                .unwrap_or("Open on your phone"),
            256,
        ),
        target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destinations_are_validated_independently_from_display_labels() {
        for bad in [
            "file:///tmp/run",
            "javascript:alert(1)",
            "https://user:pass@example.invalid",
            "https://example.invalid\\evil",
            "https://example.invalid/\n",
            "data:text/html,x",
        ] {
            assert!(web_url(bad).is_none());
            assert_eq!(
                action(Some("Read"), Some(bad), "url").target,
                Target::Unavailable
            );
        }
        assert_eq!(
            web_url("https://example.invalid/path?q=hello%20world").unwrap(),
            "https://example.invalid/path?q=hello%20world"
        );
        assert!(first_link("Look: https://example.invalid/a").is_some());
        assert_eq!(
            action(Some("Copy"), Some("SYNTHETIC"), "copy").target,
            Target::Copy("SYNTHETIC".into())
        );
    }
}
