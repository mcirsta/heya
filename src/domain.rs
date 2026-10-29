//! Application state. No toolkit or protocol types cross this boundary.

pub type ChatId = i32;
pub type MessageId = i32;
pub const PAGE_SIZE: usize = 100;
pub const DELETED_MESSAGE: &str = "This message was deleted";

// Bound sidebar shaping and copying independently of the full message body.
// Slint still elides this summary to the available width.
pub const SUMMARY_CHARS: usize = 256;

pub fn phone_number(value: &str) -> Option<String> {
    if !value
        .chars()
        .all(|c| c.is_ascii_digit() || "+ ()-".contains(c))
    {
        return None;
    }
    let digits: String = value.chars().filter(char::is_ascii_digit).collect();
    ((7..=15).contains(&digits.len()) && !digits.starts_with('0')).then_some(digits)
}

pub fn initials(title: &str) -> String {
    let mut letters = title
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()));
    let Some(first) = letters.next() else {
        return "#".into();
    };
    let mut result: String = first.to_uppercase().collect();
    if let Some(last) = letters.next_back() {
        result.extend(last.to_uppercase());
    }
    result
}

pub fn single_line_summary(text: &str) -> String {
    let mut chars = text.chars();
    let mut summary = String::new();
    let mut space = false;
    for ch in chars.by_ref().take(SUMMARY_CHARS) {
        if ch.is_whitespace() {
            space = !summary.is_empty();
        } else if !ch.is_control() {
            if space {
                summary.push(' ');
                space = false;
            }
            summary.push(ch);
        }
    }
    if chars.next().is_some() {
        summary.push('…');
    }
    summary
}

/// A profile name is supplied by the participant, unlike a saved contact name.
pub fn participant_name(name: &str, saved: bool, identity: &str) -> String {
    if name.trim().is_empty() {
        identity.split('@').next().unwrap_or(identity).into()
    } else if saved {
        name.into()
    } else {
        format!("~{name}")
    }
}

#[test]
fn participant_labels_distinguish_saved_profiles_and_numbers() {
    assert_eq!(
        participant_name("Alex", true, "15550000001@s.whatsapp.net"),
        "Alex"
    );
    assert_eq!(
        participant_name("Alex", false, "15550000001@s.whatsapp.net"),
        "~Alex"
    );
    assert_eq!(
        participant_name("  ", false, "15550000001@s.whatsapp.net"),
        "15550000001"
    );
}

/// Sidebar-only wording, shared by the archive and synthetic demo. The full
/// message/caption stays untouched. A sender is supplied only for incoming groups.
/// The open chat keeps its last-message preview: its draft is already visible
/// in the composer. This display rule must never control draft persistence.
pub fn chat_preview(
    draft: &str,
    is_open: bool,
    text: &str,
    photo_caption: Option<&str>,
    group_sender: Option<&str>,
) -> String {
    let mut preview = String::new();
    let mut append = |part: &str| {
        let remaining = (SUMMARY_CHARS + 1).saturating_sub(preview.chars().count());
        preview.extend(part.chars().take(remaining));
    };
    if !is_open && !draft.is_empty() {
        append("Draft: ");
        append(draft);
    } else {
        if let Some(sender) = group_sender.filter(|_| photo_caption.is_some() || !text.is_empty()) {
            let sender = single_line_summary(sender);
            let short = if sender.starts_with('+') {
                sender.as_str()
            } else {
                sender.split_whitespace().next().unwrap_or_default()
            };
            if !short.is_empty() {
                append(short);
                append(": ");
            }
        }
        if let Some(caption) = photo_caption {
            append("Photo");
            let line: String = caption
                .chars()
                .take(SUMMARY_CHARS + 1)
                .take_while(|c| !matches!(c, '\r' | '\n'))
                .collect();
            let line = single_line_summary(&line);
            if !line.is_empty() {
                append(": ");
                append(&line);
            }
        } else {
            append(text);
        }
    }
    single_line_summary(&preview)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: MessageId,
    pub sender: String,
    /// Canonical participant identity, independent of the displayed name.
    pub sender_id: String,
    pub text: String,
    pub time: String,
    /// Local calendar day, computed before publishing the view to the UI.
    pub date: Option<chrono::NaiveDate>,
    pub outgoing: bool,
    pub local: bool,
    pub content: std::sync::Arc<crate::message_content::Content>,
}

impl Message {
    pub fn starts_day(&self, previous: Option<&Self>) -> bool {
        self.date.is_some() && previous.is_none_or(|p| p.date != self.date)
    }

    pub fn starts_run(&self, previous: Option<&Self>) -> bool {
        self.content.notice.is_some()
            || previous.is_some_and(|p| p.content.notice.is_some())
            || self.starts_day(previous)
            || previous.is_none_or(|p| {
                p.outgoing != self.outgoing || (!self.outgoing && p.sender_id != self.sender_id)
            })
    }

    pub fn transcript_line(&self) -> String {
        let date = self
            .date
            .map(|d| format!("{} ", d.format("%Y-%m-%d")))
            .unwrap_or_default();
        if let Some(notice) = self.content.notice {
            return format!("[{date}{}] {}", self.time, notice.label(&self.sender));
        }
        let text = self
            .content
            .document
            .as_ref()
            .map(|d| d.transcript(self.content.body.as_deref().unwrap_or_default()));
        format!(
            "[{date}{}] {}: {}",
            self.time,
            self.sender,
            text.as_deref().unwrap_or(&self.text)
        )
    }
}

#[derive(Clone, Debug)]
pub struct Chat {
    pub id: ChatId,
    pub title: String,
    pub initials: String,
    pub tint: u32,
    pub is_group: bool,
    pub unread: usize,
    pub messages: Vec<Message>,
    pub draft: String,
    pub visible_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatSummary {
    pub id: ChatId,
    pub title: String,
    pub initials: String,
    pub tint: u32,
    pub preview: String,
    /// 0 ordinary summary, 1 voice/unknown call, 2 video call.
    pub call_kind: i32,
    pub time: String,
    pub unread: usize,
    pub marked_unread: bool,
    pub muted: bool,
    pub archived: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryReply {
    pub chat: ChatId,
    pub request: i32,
    pub success: bool,
}

#[derive(Clone, Debug)]
pub struct Conversation {
    pub chat: ChatSummary,
    pub is_group: bool,
    pub messages: std::sync::Arc<[Message]>,
    pub draft: String,
    pub reply: Option<crate::message_actions::ReplyDraft>,
    pub older_count: usize,
    pub transcript: std::sync::Arc<str>,
}

pub struct DemoState {
    reply_drafts: std::collections::HashMap<ChatId, crate::message_actions::ReplyDraft>,
    pub chats: Vec<Chat>,
    pub selected: ChatId,
    pub query: String,
    pub unread_only: bool,
    next_message_id: MessageId,
    pub navigation_end: Option<usize>,
    pub quote_targets: std::collections::HashMap<MessageId, MessageId>,
}

impl Chat {
    fn summary(&self, selected: ChatId) -> ChatSummary {
        let last = self.messages.last();
        let notice = last.and_then(|m| {
            m.content
                .notice
                .map(|n| n.label(&single_line_summary(&m.sender)))
        });
        ChatSummary {
            id: self.id,
            title: self.title.clone(),
            initials: self.initials.clone(),
            tint: self.tint,
            call_kind: if self.id != selected && !self.draft.is_empty() {
                0
            } else {
                match last.and_then(|m| m.content.notice) {
                    Some(crate::message_content::Notice::Call(c)) => {
                        if c.video == Some(true) {
                            2
                        } else {
                            1
                        }
                    }
                    _ => 0,
                }
            },
            preview: chat_preview(
                &self.draft,
                self.id == selected,
                notice
                    .as_deref()
                    .unwrap_or_else(|| last.map_or("", |m| m.text.as_str())),
                last.filter(|m| m.content.photo.is_some())
                    .map(|m| m.content.body.as_deref().unwrap_or_default()),
                last.filter(|m| self.is_group && !m.outgoing && m.content.notice.is_none())
                    .map(|m| m.sender.as_str()),
            ),
            time: last.map_or_else(String::new, |m| m.time.clone()),
            unread: self.unread,
            marked_unread: false,
            muted: false,
            archived: false,
        }
    }
}

impl DemoState {
    pub fn new(chats: Vec<Chat>) -> Self {
        let selected = chats.first().expect("demo fixture contains chats").id;
        let next_message_id = chats
            .iter()
            .flat_map(|c| &c.messages)
            .map(|m| m.id)
            .max()
            .unwrap_or(0)
            + 1;
        Self {
            reply_drafts: Default::default(),
            chats,
            selected,
            query: String::new(),
            unread_only: false,
            next_message_id,
            navigation_end: None,
            quote_targets: Default::default(),
        }
    }

    pub fn sidebar(&self) -> Vec<ChatSummary> {
        let query = self.query.trim().to_lowercase();
        self.chats
            .iter()
            .filter(|chat| !self.unread_only || chat.unread > 0)
            .filter(|chat| chat.title.to_lowercase().contains(&query))
            .map(|chat| chat.summary(self.selected))
            .collect()
    }

    pub fn conversation(&self) -> Conversation {
        let chat = self
            .chats
            .iter()
            .find(|c| c.id == self.selected)
            .expect("selected chat exists");
        let end = self
            .navigation_end
            .unwrap_or(chat.messages.len())
            .min(chat.messages.len());
        let start = end.saturating_sub(chat.visible_count);
        let messages = chat.messages[start..end].to_vec();
        let transcript = messages
            .iter()
            .map(Message::transcript_line)
            .collect::<Vec<_>>()
            .join("\n\n");
        Conversation {
            chat: chat.summary(self.selected),
            is_group: chat.is_group,
            messages: messages.into(),
            draft: chat.draft.clone(),
            reply: self.reply_drafts.get(&chat.id).cloned(),
            older_count: start,
            transcript: transcript.into(),
        }
    }

    pub fn select(&mut self, id: ChatId) -> bool {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == id) {
            chat.unread = 0;
            self.selected = id;
            self.navigation_end = None;
            true
        } else {
            false
        }
    }

    pub fn set_draft(&mut self, id: ChatId, text: String) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == id) {
            chat.draft = text;
        }
    }

    pub fn send(&mut self, id: ChatId, text: String) -> Option<Message> {
        if text.trim().is_empty() {
            return None;
        }
        let chat = self.chats.iter_mut().find(|c| c.id == id)?;
        let now = chrono::Local::now();
        let mut message = Message {
            id: self.next_message_id,
            sender: "You".into(),
            sender_id: "#self".into(),
            text,
            time: now.format("%H:%M").to_string(),
            date: Some(now.date_naive()),
            outgoing: true,
            local: true,
            content: Default::default(),
        };
        if let Some(reply) = self.reply_drafts.remove(&id) {
            self.quote_targets.insert(message.id, reply.id);
            std::sync::Arc::make_mut(&mut message.content).quote =
                Some(crate::message_content::Quote {
                    sender: reply.sender,
                    text: reply.text,
                });
        }
        std::sync::Arc::make_mut(&mut message.content).metadata =
            crate::message_actions::Metadata {
                wire: format!("DEMO-{}", message.id),
                stamp: now.timestamp(),
                delivery: "accepted".into(),
                ..Default::default()
            };
        self.next_message_id += 1;
        // A queued send must not erase a newer draft for the same chat.
        if chat.draft == message.text {
            chat.draft.clear();
        }
        chat.messages.push(message.clone());
        chat.visible_count += 1;
        Some(message)
    }

    pub fn load_older(&mut self, id: ChatId) -> bool {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == id) {
            chat.visible_count = (chat.visible_count + PAGE_SIZE).min(chat.messages.len());
            true
        } else {
            false
        }
    }

    pub fn message_action(
        &mut self,
        command: crate::message_actions::Command,
    ) -> (
        crate::message_actions::OperationReply,
        std::sync::Arc<[crate::message_actions::Destination]>,
    ) {
        use crate::message_actions::{Command, Destination, OperationReply, ReplyDraft};
        let mut destinations = Vec::new();
        let (request, kind, saved) = match command {
            Command::Star {
                chat,
                message,
                starred,
                request,
            } => {
                let source = self
                    .chats
                    .iter_mut()
                    .find(|c| c.id == chat)
                    .and_then(|c| c.messages.iter_mut().find(|m| m.id == message));
                if let Some(source) = source
                    && !source.content.unavailable
                    && source.content.notice.is_none()
                {
                    std::sync::Arc::make_mut(&mut source.content)
                        .metadata
                        .starred = starred;
                    (request, 8, true)
                } else {
                    (request, 8, false)
                }
            }
            Command::Pin {
                chat,
                message,
                seconds,
                request,
            } => {
                let source = self
                    .chats
                    .iter_mut()
                    .find(|c| c.id == chat)
                    .and_then(|c| c.messages.iter_mut().find(|m| m.id == message));
                if let Some(source) = source
                    && matches!(seconds, 0 | 86400 | 604800 | 2592000)
                    && !source.content.unavailable
                    && source.content.notice.is_none()
                {
                    std::sync::Arc::make_mut(&mut source.content)
                        .metadata
                        .pinned = seconds > 0;
                    (request, 9, true)
                } else {
                    (request, 9, false)
                }
            }
            Command::CloseInfo => (0, 7, true),
            Command::Info {
                chat,
                message,
                request,
            } => {
                let source = self
                    .chats
                    .iter()
                    .find(|c| c.id == chat)
                    .and_then(|c| c.messages.iter().find(|m| m.id == message));
                let info = source
                    .filter(|m| m.outgoing)
                    .map(|m| crate::message_actions::Info {
                        message,
                        wire: m.content.metadata.wire.clone(),
                        sent: "Synthetic demo message".into(),
                        status: "Demo".into(),
                        rows: Vec::new(),
                    });
                return (
                    OperationReply {
                        request,
                        kind: 7,
                        saved: info.is_some(),
                        info,
                        error: String::new(),
                    },
                    destinations.into(),
                );
            }
            Command::Edit {
                chat,
                message,
                expected,
                text,
                request,
            } => {
                let source = self
                    .chats
                    .iter_mut()
                    .find(|c| c.id == chat)
                    .and_then(|c| c.messages.iter_mut().find(|m| m.id == message));
                let saved = if let Some(source) = source
                    && source.outgoing
                    && source.text == expected
                    && !text.trim().is_empty()
                    && text.len() <= 64 * 1024
                {
                    source.text = text;
                    let content = std::sync::Arc::make_mut(&mut source.content);
                    content.body = None;
                    content.preview = None;
                    true
                } else {
                    false
                };
                (request, 4, saved)
            }
            Command::Revoke {
                chat,
                message,
                request,
            } => {
                let source = self
                    .chats
                    .iter_mut()
                    .find(|c| c.id == chat)
                    .and_then(|c| c.messages.iter_mut().find(|m| m.id == message));
                let saved = if let Some(source) = source
                    && source.outgoing
                {
                    source.text = DELETED_MESSAGE.into();
                    let metadata = source.content.metadata.clone();
                    source.content = std::sync::Arc::new(crate::message_content::Content {
                        metadata,
                        unavailable: true,
                        ..Default::default()
                    });
                    true
                } else {
                    false
                };
                (request, 5, saved)
            }
            Command::Hide {
                chat,
                messages,
                request,
            } => {
                let saved = if let Some(chat) = self.chats.iter_mut().find(|c| c.id == chat)
                    && !messages.is_empty()
                    && messages.len() <= crate::message_actions::MAX_SELECTION
                    && messages
                        .iter()
                        .all(|id| chat.messages.iter().any(|m| m.id == *id))
                {
                    chat.messages.retain(|m| !messages.contains(&m.id));
                    if self
                        .reply_drafts
                        .get(&chat.id)
                        .is_some_and(|r| messages.contains(&r.id))
                    {
                        self.reply_drafts.remove(&chat.id);
                    }
                    true
                } else {
                    false
                };
                (request, 6, saved)
            }
            Command::ForwardMany {
                chat,
                messages,
                destination,
                request,
            } => {
                let valid = !messages.is_empty()
                    && messages.len() <= crate::message_actions::MAX_SELECTION
                    && self.chats.iter().any(|c| c.id == destination)
                    && self.chats.iter().find(|c| c.id == chat).is_some_and(|c| {
                        messages
                            .iter()
                            .all(|id| c.messages.iter().any(|m| m.id == *id))
                    });
                if valid {
                    for message in messages {
                        self.message_action(Command::Forward {
                            chat,
                            message,
                            destination,
                            request,
                        });
                    }
                }
                (request, 2, valid)
            }
            Command::Destinations { request } => {
                destinations = self
                    .chats
                    .iter()
                    .map(|c| Destination {
                        id: c.id,
                        title: c.title.clone(),
                    })
                    .collect();
                (request, 3, true)
            }
            Command::Reply {
                chat,
                message,
                request,
            } => {
                if message == 0 {
                    self.reply_drafts.remove(&chat);
                    (request, 0, true)
                } else if let Some(source) = self
                    .chats
                    .iter()
                    .find(|c| c.id == chat)
                    .and_then(|c| c.messages.iter().find(|m| m.id == message))
                {
                    self.reply_drafts.insert(
                        chat,
                        ReplyDraft {
                            id: message,
                            sender: source.sender.clone(),
                            text: single_line_summary(&source.text),
                        },
                    );
                    (request, 0, true)
                } else {
                    (request, 0, false)
                }
            }
            Command::React {
                chat,
                message,
                emoji,
                request,
            } => {
                let source = self
                    .chats
                    .iter_mut()
                    .find(|c| c.id == chat)
                    .and_then(|c| c.messages.iter_mut().find(|m| m.id == message));
                if let Some(source) = source
                    && (emoji.is_empty() || emojis::get(&emoji).is_some())
                {
                    let content = std::sync::Arc::make_mut(&mut source.content);
                    content.own_reaction = emoji.clone();
                    content.reaction_summary = emoji;
                    (request, 1, true)
                } else {
                    (request, 1, false)
                }
            }
            Command::Forward {
                chat,
                message,
                destination,
                request,
            } => {
                let source = self
                    .chats
                    .iter()
                    .find(|c| c.id == chat)
                    .and_then(|c| c.messages.iter().find(|m| m.id == message))
                    .cloned();
                if let Some(mut source) = source
                    && let Some(target) = self.chats.iter_mut().find(|c| c.id == destination)
                {
                    source.id = self.next_message_id;
                    self.next_message_id += 1;
                    source.sender = "You".into();
                    source.sender_id = "#self".into();
                    source.outgoing = true;
                    source.local = true;
                    let now = chrono::Local::now();
                    source.date = Some(now.date_naive());
                    source.time = now.format("%H:%M").to_string();
                    let content = std::sync::Arc::make_mut(&mut source.content);
                    content.forwarded = true;
                    content.metadata = crate::message_actions::Metadata {
                        wire: format!("DEMO-{}", source.id),
                        stamp: now.timestamp(),
                        delivery: "accepted".into(),
                        pending: String::new(),
                        ..Default::default()
                    };
                    content.quote = None;
                    // Business action envelopes forward as their full readable
                    // transcript, matching the protocol adapter's policy.
                    if !content.actions.is_empty() {
                        content.actions.clear();
                        content.body = None;
                    }
                    content.reaction_status.clear();
                    content.own_reaction.clear();
                    content.reaction_summary.clear();
                    target.messages.push(source);
                    target.visible_count += 1;
                    (request, 2, true)
                } else {
                    (request, 2, false)
                }
            }
        };
        (
            OperationReply {
                info: None,
                request,
                kind,
                saved,
                error: if saved {
                    String::new()
                } else {
                    "This message is no longer available.".into()
                },
            },
            destinations.into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn sidebar_summaries_are_bounded_single_lines_without_changing_messages() {
        let text = " \tHello 👩🏽‍💻\r\n\nمرحبا\u{2028}こんにちは\u{2029}end\0 \n";
        assert_eq!(single_line_summary(text), "Hello 👩🏽‍💻 مرحبا こんにちは end");
        assert_eq!(single_line_summary("\n \t"), "");
        let long = "日".repeat(100_000);
        let summary = single_line_summary(&long);
        assert_eq!(summary.chars().count(), SUMMARY_CHARS + 1);
        assert!(summary.ends_with('…'));
        assert_eq!(single_line_summary(&summary), summary);
        assert_eq!(
            single_line_summary(&"x".repeat(SUMMARY_CHARS)).len(),
            SUMMARY_CHARS
        );
        let mut state = fixtures::demo();
        state.send(1, text.into()).unwrap();
        assert_eq!(state.sidebar()[0].preview, single_line_summary(text));
        assert_eq!(state.conversation().messages.last().unwrap().text, text);
    }

    #[test]
    fn photo_previews_share_caption_and_group_sender_rules_with_the_demo() {
        assert_eq!(
            chat_preview("", false, "[Photo]", Some(""), Some("~Sam Sample")),
            "~Sam: Photo"
        );
        assert_eq!(
            chat_preview("", false, "[Photo]", Some(" \nSecond line"), None),
            "Photo"
        );
        assert_eq!(
            chat_preview(
                "",
                false,
                "[Photo]",
                Some("Lunch 👩🏽‍💻\r\nFull caption"),
                Some("Mira Saved")
            ),
            "Mira: Photo: Lunch 👩🏽‍💻"
        );
        assert_eq!(
            chat_preview("", false, "[Photo] is literal text", None, None),
            "[Photo] is literal text"
        );
        assert_eq!(
            chat_preview("", false, "Hi", None, Some("+1 555 000 0001")),
            "+1 555 000 0001: Hi"
        );
        let long = "Caption\n".repeat(100_000);
        assert_eq!(
            chat_preview("", false, "", Some(&long), None),
            "Photo: Caption"
        );
        assert_eq!(
            chat_preview(&long, false, "", Some(""), Some("~Sam")),
            single_line_summary(&format!("Draft: {long}"))
        );
        let state = fixtures::demo();
        assert_eq!(
            state.sidebar().iter().find(|c| c.id == 4).unwrap().preview,
            "~Sam: Photo"
        );
        assert_eq!(state.chats[3].messages.last().unwrap().text, "[Photo]");
    }

    #[test]
    fn queued_send_retains_destination_and_newer_draft() {
        let mut state = fixtures::demo();
        state.set_draft(1, "old message".into());
        state.select(2);
        state.set_draft(1, "next message".into());
        let sent = state.send(1, "old message".into()).unwrap();
        assert_eq!(state.selected, 2);
        assert_eq!(state.chats[0].messages.last().unwrap().id, sent.id);
        assert_eq!(state.chats[0].draft, "next message");
        assert!(!state.chats[1].messages.iter().any(|m| m.id == sent.id));
    }

    #[test]
    fn draft_roundtrip_and_send_preserve_unicode_and_newlines() {
        let mut state = fixtures::demo();
        let text = "  Hello 👩🏽‍💻\nمرحبا · こんにちは\n  ";
        let before = state.sidebar();
        state.set_draft(1, text.into());
        assert_eq!(
            state.sidebar(),
            before,
            "typing must not invalidate sidebar rows"
        );
        state.select(2);
        assert_eq!(
            state.sidebar()[0].preview,
            single_line_summary(&format!("Draft: {text}"))
        );
        state.select(1);
        assert_eq!(state.sidebar()[0].preview, before[0].preview);
        assert_eq!(state.conversation().draft, text);
        let sent = state.send(1, text.into()).unwrap();
        assert_eq!(sent.text, text);
        assert!(state.conversation().draft.is_empty());
        state.select(2);
        assert_eq!(state.sidebar()[0].preview, single_line_summary(text));
        assert!(state.send(1, " \n\t".into()).is_none());
        assert!(state.send(999, "hello".into()).is_none());
    }

    #[test]
    fn history_pages_keep_order_without_duplicates() {
        let mut state = fixtures::demo();
        state.select(6);
        let recent = state.conversation();
        assert_eq!(recent.messages.len(), PAGE_SIZE);
        assert_eq!(recent.older_count, 10_000 - PAGE_SIZE);
        state.load_older(6);
        let older = state.conversation();
        assert_eq!(older.messages[PAGE_SIZE].id, recent.messages[0].id);
        assert!(older.messages.windows(2).all(|m| m[0].id < m[1].id));
        for _ in 0..110 {
            state.load_older(6);
        }
        assert_eq!(state.conversation().messages.len(), 10_000);
        assert_eq!(state.conversation().older_count, 0);
    }

    #[test]
    fn filters_intersect_and_selection_marks_read() {
        let mut state = fixtures::demo();
        state.query = "  RUST  ".into();
        state.unread_only = true;
        assert_eq!(state.sidebar().len(), 1);
        let id = state.sidebar()[0].id;
        state.select(id);
        assert!(state.sidebar().is_empty());
        assert!(!state.select(999));
        assert_eq!(state.selected, id);
    }
}
