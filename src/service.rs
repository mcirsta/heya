//! Owned background service. UI commands always carry their destination.
use crate::domain::{ChatId, ChatSummary, Conversation, DemoState, Message};
use tokio::sync::mpsc;

pub enum Command {
    Navigation(crate::message_navigation::Command),
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
    Search(crate::message_navigation::Search),
    Navigation {
        view: Conversation,
        jump: crate::message_navigation::Jump,
    },
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
    let mut return_count = None;
    while let Some(command) = commands.recv().await {
        let command = if let Command::Navigation(crate::message_navigation::Command::Open {
            chat,
            message,
            previous,
            draft,
            request,
            revision,
            ..
        }) = command
        {
            if !state.chats.iter().any(|c| {
                c.id == chat
                    && c.messages
                        .iter()
                        .any(|m| m.id == message && !m.content.unavailable)
            }) {
                if !publish(Update::Navigation {
                    view: state.conversation(),
                    jump: crate::message_navigation::Jump {
                        request,
                        ..Default::default()
                    },
                }) {
                    break;
                }
                continue;
            }
            state.set_draft(previous, draft);
            if state.selected != chat {
                state.select(chat);
                return_count = None;
            }
            if !publish(Update::Sidebar(state.sidebar())) {
                break;
            }
            Command::Navigation(crate::message_navigation::Command::Navigate {
                chat,
                message,
                quote: false,
                request,
                revision,
            })
        } else {
            command
        };
        let update = match command {
            Command::Navigation(crate::message_navigation::Command::Open { .. }) => unreachable!(),
            Command::Navigation(crate::message_navigation::Command::Search {
                chat,
                query,
                request,
                ..
            }) => {
                if chat == 0 {
                    let search = demo_search(&state, &query, request).unwrap_or_else(|_| {
                        crate::message_navigation::Search {
                            query,
                            request,
                            error: "Search is unavailable.".into(),
                            ..Default::default()
                        }
                    });
                    if !publish(Update::Search(search)) {
                        break;
                    }
                    continue;
                }
                if chat != state.selected {
                    continue;
                }
                let needle = query.trim().to_lowercase();
                let mut search = crate::message_navigation::Search {
                    chat,
                    query,
                    request,
                    ..Default::default()
                };
                if !needle.is_empty() {
                    for message in state
                        .chats
                        .iter()
                        .find(|c| c.id == chat)
                        .unwrap()
                        .messages
                        .iter()
                        .rev()
                        .filter(|m| {
                            !m.content.unavailable && m.text.to_lowercase().contains(&needle)
                        })
                    {
                        if search.hits.len() == 100 {
                            search.more = true;
                            break;
                        }
                        search.hits.push(crate::message_navigation::Hit {
                            id: message.id,
                            chat,
                            title: String::new(),
                            text: crate::domain::single_line_summary(&message.text),
                            time: message.time.clone(),
                        });
                    }
                }
                Update::Search(search)
            }
            Command::Navigation(crate::message_navigation::Command::Navigate {
                chat,
                message,
                quote,
                request,
                ..
            }) => {
                if chat != state.selected {
                    continue;
                }
                let c = state.chats.iter_mut().find(|c| c.id == chat).unwrap();
                let target = if quote {
                    state.quote_targets.get(&message).copied()
                } else {
                    Some(message)
                };
                let mut jump = crate::message_navigation::Jump {
                    request,
                    browsing: state.navigation_end.is_some(),
                    ..Default::default()
                };
                if target == Some(0) {
                    state.navigation_end = None;
                    c.visible_count = return_count.take().unwrap_or(100);
                    jump.returning = true;
                    jump.browsing = false;
                } else if let Some((index, id)) = target.and_then(|id| {
                    c.messages
                        .iter()
                        .enumerate()
                        .find(|(_, m)| m.id == id && !m.content.unavailable)
                        .map(|(i, m)| (i, m.id))
                }) {
                    if state.navigation_end.is_none() {
                        return_count = Some(c.visible_count);
                    }
                    state.navigation_end = Some((index + 51).min(c.messages.len()));
                    c.visible_count = 100;
                    jump.message = id;
                    jump.browsing = true;
                }
                Update::Navigation {
                    view: state.conversation(),
                    jump,
                }
            }
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

// The synthetic demo uses the same tokenizer and query semantics on its worker.
// It has no persistent archive; rebuilding here also includes demo edits/sends.
fn demo_search(
    state: &DemoState,
    query: &str,
    request: i32,
) -> rusqlite::Result<crate::message_navigation::Search> {
    let mut result = crate::message_navigation::Search {
        query: query.into(),
        request,
        ..Default::default()
    };
    if query.trim().is_empty() {
        return Ok(result);
    }
    let mut db = rusqlite::Connection::open_in_memory()?;
    db.execute_batch("PRAGMA temp_store=MEMORY; CREATE VIRTUAL TABLE words USING fts5(body,chat UNINDEXED,title UNINDEXED,time UNINDEXED,tokenize='unicode61 remove_diacritics 2');")?;
    let terms = crate::message_navigation::word_query(&db, query)?;
    if terms.is_empty() {
        return Ok(result);
    }
    let tx = db.transaction()?;
    {
        let mut insert =
            tx.prepare("INSERT INTO words(rowid,body,chat,title,time) VALUES(?1,?2,?3,?4,?5)")?;
        for chat in &state.chats {
            for message in chat.messages.iter().filter(|m| !m.content.unavailable) {
                insert.execute(rusqlite::params![
                    message.id,
                    message.text,
                    chat.id,
                    chat.title,
                    message.time
                ])?;
            }
        }
    }
    tx.commit()?;
    result.hits=db.prepare("SELECT rowid,body,chat,title,time FROM words WHERE words MATCH ?1 ORDER BY rowid DESC LIMIT 100")?
        .query_map([terms],|r|Ok(crate::message_navigation::Hit {id:r.get(0)?,text:crate::domain::single_line_summary(&r.get::<_,String>(1)?),chat:r.get(2)?,title:r.get(3)?,time:r.get(4)?}))?
        .collect::<rusqlite::Result<_>>()?;
    result.more = result.hits.len() == 100;
    Ok(result)
}
