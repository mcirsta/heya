//! Owned background service. UI commands always carry their destination.
use crate::domain::{ChatId, ChatSummary, Conversation, DemoState, Message};
use tokio::sync::mpsc;

pub enum Command {
    Action(crate::message_actions::Command),
    Select {
        id: ChatId,
        previous: ChatId,
        draft: String,
    },
    Search(String),
    Unread(bool),
    Send(ChatId, String),
    Older {
        chat: ChatId,
        request: i32,
    },
}

pub enum Update {
    Action {
        view: Conversation,
        result: crate::message_actions::OperationReply,
        destinations: std::sync::Arc<[crate::message_actions::Destination]>,
    },
    Sidebar(Vec<ChatSummary>),
    Conversation(Conversation),
    History {
        view: Conversation,
        reply: crate::domain::HistoryReply,
    },
    Sent {
        chat: ChatId,
        message: Message,
        transcript: std::sync::Arc<str>,
    },
}

pub async fn run(
    mut state: DemoState,
    mut commands: mpsc::Receiver<Command>,
    publish: impl Fn(Update) -> bool,
) {
    while let Some(command) = commands.recv().await {
        let update = match command {
            Command::Action(command) => {
                let (result, destinations) = state.message_action(command);
                if !publish(Update::Sidebar(state.sidebar())) {
                    break;
                }
                Update::Action {
                    view: state.conversation(),
                    result,
                    destinations,
                }
            }
            Command::Select {
                id,
                previous,
                draft,
            } => {
                state.set_draft(previous, draft);
                if !state.select(id) {
                    continue;
                }
                if !publish(Update::Sidebar(state.sidebar())) {
                    break;
                }
                Update::Conversation(state.conversation())
            }
            Command::Search(query) => {
                state.query = query;
                Update::Sidebar(state.sidebar())
            }
            Command::Unread(unread) => {
                state.unread_only = unread;
                Update::Sidebar(state.sidebar())
            }
            Command::Send(id, text) => {
                // The UI submits the current text atomically and clears it only
                // after enqueue succeeds. Superseded saved drafts must not return.
                state.set_draft(id, text.clone());
                let Some(message) = state.send(id, text) else {
                    continue;
                };
                if !publish(Update::Sidebar(state.sidebar())) {
                    break;
                }
                let transcript = if state.selected == id {
                    state.conversation().transcript
                } else {
                    String::new().into()
                };
                Update::Sent {
                    chat: id,
                    message,
                    transcript,
                }
            }
            Command::Older { chat, request } => {
                if state.selected != chat {
                    continue;
                }
                state.load_older(chat);
                Update::History {
                    view: state.conversation(),
                    reply: crate::domain::HistoryReply {
                        chat,
                        request,
                        success: true,
                    },
                }
            }
        };
        if !publish(update) {
            break;
        }
    }
}
