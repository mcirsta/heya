//! Application commands and archive-backed views, independent of Slint/SDK types.
#[cfg(test)]
#[path = "message_navigation_tests.rs"]
mod navigation_tests;
#[cfg(test)]
#[path = "messaging_phone_tests.rs"]
mod phone_tests;
#[cfg(test)]
#[path = "messaging_send_tests.rs"]
mod send_tests;
use crate::archive::{Archive, Error, View};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

/// Opt-in fixed stages and local request numbers only; never account/message data.
pub(crate) fn trace_send(stage: &str, request: i32) {
    if std::env::var_os("HEYA_TRACE_SEND").is_some() {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        eprintln!(
            "Heya send: stage={stage} request={request} elapsed_ms={}",
            START
                .get_or_init(std::time::Instant::now)
                .elapsed()
                .as_millis()
        );
    }
}

pub enum Command {
    Navigation(crate::message_navigation::Command),
    ManageChat {
        chat: i32,
        action: crate::chat_management::Action,
    },
    Folder {
        archived: bool,
        previous: i32,
        draft: String,
        draft_version: i32,
        revision: i32,
    },
    FetchPhone {
        chat: i32,
        revision: i32,
    },
    ReadVisible {
        chat: i32,
        message: i32,
        revision: i32,
    },
    Action(crate::message_actions::Command),
    Select {
        id: i32,
        previous: i32,
        draft: String,
        draft_version: i32,
        revision: i32,
    },
    Draft(i32, String, i32),
    Search(String),
    Unread(bool),
    Older {
        chat: i32,
        request: i32,
        revision: i32,
    },
    Send {
        chat: i32,
        text: String,
        request: i32,
        draft_version: i32,
    },
    NewChat {
        phone: String,
        previous: i32,
        draft: String,
        draft_version: i32,
        revision: i32,
    },
}
#[derive(Clone)]
pub struct SendReply {
    pub request: i32,
    pub chat: i32,
    pub text: String,
    pub saved: bool,
}
#[derive(Clone)]
pub struct Snapshot {
    pub search: crate::message_navigation::Search,
    pub jump: crate::message_navigation::Jump,
    pub chat_busy: bool,
    pub history: Option<crate::domain::HistoryReply>,
    pub phone: crate::phone_history::Status,
    pub operation: Option<crate::message_actions::OperationReply>,
    pub destinations: Arc<[crate::message_actions::Destination]>,
    pub revision: i32,
    // None with an empty notice carries only a save reply; preserve the UI view.
    pub view: Option<Arc<View>>,
    pub reply: Option<SendReply>,
    pub notice: String,
}
impl Snapshot {
    pub fn empty() -> Self {
        Self {
            search: Default::default(),
            jump: Default::default(),
            chat_busy: false,
            history: None,
            phone: Default::default(),
            operation: None,
            destinations: Arc::from([]),
            revision: 0,
            view: None,
            reply: None,
            notice: String::new(),
        }
    }
}

#[cfg(test)]
pub async fn run(
    archives: watch::Receiver<Option<Archive>>,
    commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Snapshot>,
) {
    let (_sources, sources) = watch::channel(None);
    run_with_history(archives, commands, updates, sources).await;
}
pub async fn run_with_history(
    archives: watch::Receiver<Option<Archive>>,
    commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Snapshot>,
    phone_sources: watch::Receiver<crate::phone_history::Sources>,
) {
    run_with_preferences(
        archives,
        commands,
        updates,
        phone_sources,
        watch::channel(crate::preferences::Preferences::default()).1,
    )
    .await;
}

pub async fn run_with_preferences(
    archives: watch::Receiver<Option<Archive>>,
    commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Snapshot>,
    phone_sources: watch::Receiver<crate::phone_history::Sources>,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) {
    run_with_chat_sources(
        archives,
        commands,
        updates,
        phone_sources,
        preferences,
        watch::channel(None).1,
    )
    .await;
}

pub async fn run_with_chat_sources(
    mut archives: watch::Receiver<Option<Archive>>,
    mut commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Snapshot>,
    mut phone_sources: watch::Receiver<crate::phone_history::Sources>,
    mut preferences: watch::Receiver<crate::preferences::Preferences>,
    chat_sources: watch::Receiver<crate::chat_management::Sources>,
) {
    let mut archive: Option<Archive> = None;
    let mut changed: Option<watch::Receiver<u64>> = None;
    let (mut selected, mut revision, mut limit) = (0, 0, 100usize);
    let (mut query, mut unread) = (String::new(), false);
    let mut archived = false;
    let mut chat_pending: Option<crate::chat_management::Pending> = None;
    let mut reply = None;
    let mut operation = None;
    let mut info_target = None;
    let mut destinations: Arc<[crate::message_actions::Destination]> = Arc::from([]);
    let mut sent_draft: Option<(i32, String, i32)> = None;
    let mut history = None;
    let mut search = crate::message_navigation::Search::default();
    let mut jump = crate::message_navigation::Jump::default();
    let mut window = None;
    let mut return_limit = 100;
    let mut history_boundary: Option<(i32, bool, bool)> = None;
    let mut phone_pending: Option<crate::phone_history::Pending> = None;
    let mut phone_context: Option<(i32, i32, i32)> = None;
    let mut phone_cooldowns = std::collections::HashMap::<i32, tokio::time::Instant>::new();
    let mut phone_sources_open = true;
    let mut archives_open = true;
    let mut preferences_open = true;
    let mut expiry = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        let mut notice = String::new();
        let mut page = None;
        tokio::select! {
            result=async {match &mut chat_pending {Some(pending)=>pending.await,None=>std::future::pending().await}}=>{
                chat_pending=None;
                notice=match result {Ok(text)=>text.into(),Err(error)=>error.message().into()};
            }
            event=preferences.changed(), if preferences_open => {
                preferences_open=event.is_ok();
                let enabled=preferences.borrow_and_update().send_receipts;
                if !enabled && let Some(db)=&archive
                    && let Err(error)=db.discard_receipts().await {notice=error.to_string();}
                if notice.is_empty(){continue;}
            }
            _ = expiry.tick(), if archive.is_some() => {},
            event = archives.changed(), if archives_open => {
                if event.is_err() { archives_open=false;continue; }
                phone_pending=None;
                if let Some((chat,request,_))=phone_context.take(){history=Some(crate::domain::HistoryReply{chat,request,success:false});}
                archive = archives.borrow_and_update().clone();
                window=None;search=Default::default();jump=Default::default();
                changed = archive.as_ref().map(Archive::subscribe);
                let enabled=preferences.borrow().send_receipts;
                if !enabled && let Some(db)=&archive
                    && let Err(error)=db.discard_receipts().await {notice=error.to_string();}
            }
            _ = async { match &mut changed {Some(changed)=>{let _=changed.changed().await;},None=>std::future::pending().await} } => {},
            event=phone_sources.changed(), if phone_sources_open => {
                if event.is_err() {phone_sources_open=false;}
                phone_sources.borrow_and_update();
                phone_pending=None;
                if let Some((chat,request,_))=phone_context.take() {
                    history=Some(crate::domain::HistoryReply{chat,request,success:false});
                }
            }
            outcome=async {match &mut phone_pending {Some(pending)=>pending.await,None=>std::future::pending().await}} => {
                phone_pending=None;
                phone_context=None;
                if outcome.revision!=revision {continue;}
                let same_chat=outcome.chat==selected || if let Some(db)=&archive {
                    db.phone_history_anchor(outcome.chat).await.ok().flatten().is_some_and(|a|a.chat==selected)
                }else{false};
                if !same_chat {continue;}
                match outcome.result {
                    Ok(true)=>{phone_cooldowns.remove(&selected);page=Some((selected,outcome.request,limit.saturating_add(crate::phone_history::BATCH as usize)));}
                    result=>{
                        history=Some(crate::domain::HistoryReply{chat:selected,request:outcome.request,success:false});
                        phone_cooldowns.insert(selected,tokio::time::Instant::now()+crate::phone_history::COOLDOWN);
                        notice=match result {Err(error)=>error.message().into(),_=>"Your phone sent no older messages for this conversation.".into()};
                    }
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return; };
                let Some(db) = &archive else {
                    if let Command::Send {chat,text,request,..}=command {
                        // A disconnect can release storage after UI submission.
                        // Retain the rejection just like a durable-save reply.
                        reply=Some(SendReply{chat,text,request,saved:false});
                        updates.send_modify(|snapshot|snapshot.reply=reply.clone());
                    }
                    continue;
                };
                let result = match command {
                    Command::Navigation(crate::message_navigation::Command::Search {chat,query,request,revision:seen}) => {
                        if chat!=selected || seen!=revision {continue;}
                        search=crate::message_navigation::Search {chat,query,request,..Default::default()};
                        Ok(())
                    }
                    Command::Navigation(crate::message_navigation::Command::Navigate {chat,message,quote,request,revision:seen}) => {
                        if chat!=selected || seen!=revision {continue;}
                        if message==0 {
                            window=None; limit=return_limit;
                            jump=crate::message_navigation::Jump {request,returning:true,..Default::default()};
                            Ok(())
                        } else {
                            match db.navigation_target(chat,message,quote).await {
                                Ok((target,end))=>{
                                    if window.is_none() {return_limit=limit;}
                                    window=Some(end); limit=100;
                                    phone_pending=None;
                                    if let Some((chat,request,_))=phone_context.take(){history=Some(crate::domain::HistoryReply{chat,request,success:false});}
                                    jump=crate::message_navigation::Jump {request,message:target,browsing:true,returning:false};
                                    Ok(())
                                }
                                Err(_)=>{jump=crate::message_navigation::Jump {request,browsing:window.is_some(),..Default::default()};notice="The original message is not available in saved history.".into();Ok(())}
                            }
                        }
                    }
                    Command::ManageChat {chat,action}=>{
                        if chat_pending.is_some() {continue;}
                        chat_pending=Some(crate::chat_management::request(db.clone(),chat_sources.clone(),chat,action));
                        Ok(())
                    }
                    Command::Folder {archived:folder,previous,draft,draft_version,revision:next}=>{
                        revision=next;
                        if let Err(error)=save_draft(db,previous,draft,draft_version,&sent_draft).await {
                            updates.send_replace(Snapshot {chat_busy:chat_pending.is_some(),revision,notice:error.to_string(),..Snapshot::empty()});
                            continue;
                        }
                        archived=folder;selected=0;revision=next;query.clear();unread=false;limit=100;
                        phone_pending=None;phone_context=None;info_target=None;
                        window=None;search=Default::default();jump=Default::default();history_boundary=None;
                        Ok(())
                    }
                    Command::Action(crate::message_actions::Command::CloseInfo)=>{info_target=None;continue;}
                    Command::Action(crate::message_actions::Command::Info {chat,message,request})=>{info_target=Some((chat,message,request));Ok(())}
                    Command::Action(crate::message_actions::Command::Edit {chat,message,expected,text,request})=>{
                        let result=match fresh_id(){Ok(id)=>db.queue_edit(chat,message,id,expected,text).await,Err(e)=>Err(e)};
                        operation=Some(operation_reply(request,4,&result));result
                    }
                    Command::Action(crate::message_actions::Command::Revoke {chat,message,request})=>{
                        let result=match fresh_id(){Ok(id)=>db.queue_revoke(chat,message,id).await,Err(e)=>Err(e)};
                        operation=Some(operation_reply(request,5,&result));result
                    }
                    Command::Action(crate::message_actions::Command::Hide {chat,messages,request})=>{
                        let result=match fresh_ids(messages.len()){Ok(ids)=>db.queue_hides(chat,messages,ids).await,Err(e)=>Err(e)};
                        operation=Some(operation_reply(request,6,&result));result
                    }
                    Command::Action(crate::message_actions::Command::ForwardMany {chat,messages,destination,request})=>{
                        let result=match fresh_ids(messages.len()){Ok(ids)=>db.queue_forwards(chat,messages,destination,ids).await,Err(e)=>Err(e)};
                        operation=Some(operation_reply(request,2,&result));result
                    }
                    Command::ReadVisible {chat,message,revision:seen} => {
                        if window.is_some() || seen!=revision || !updates.borrow().view.as_ref().and_then(|v| v.conversation.as_ref()).is_some_and(|c| c.chat.id==chat) { continue; }
                        let send_receipts=preferences.borrow().send_receipts;
                        db.mark_visible_read_with_receipts(chat,message,send_receipts).await
                    }
                    Command::Action(crate::message_actions::Command::Reply {chat,message,request})=>{
                        let result=db.set_reply(chat,message).await;
                        operation=Some(operation_reply(request,0,&result));result
                    }
                    Command::Action(crate::message_actions::Command::React {chat,message,emoji,request})=>{
                        let result=match fresh_id(){Ok(id)=>db.queue_operation(chat,message,chat,id,crate::message_actions::Operation::React(emoji)).await,Err(e)=>Err(e)};
                        operation=Some(operation_reply(request,1,&result));result
                    }
                    Command::Action(crate::message_actions::Command::Forward {chat,message,destination,request})=>{
                        let result=match fresh_id(){Ok(id)=>db.queue_operation(chat,message,destination,id,crate::message_actions::Operation::Forward).await,Err(e)=>Err(e)};
                        operation=Some(operation_reply(request,2,&result));result
                    }
                    Command::Action(crate::message_actions::Command::Destinations {request})=>{
                        let result=match db.destinations().await {Ok(value)=>{destinations=value.into();Ok(())},Err(e)=>Err(e)};
                        operation=Some(operation_reply(request,3,&result));result
                    }
                    Command::Select { id,previous,draft,draft_version,revision:next } => {
                        info_target=None;phone_pending=None;
                if let Some((chat,request,_))=phone_context.take(){history=Some(crate::domain::HistoryReply{chat,request,success:false});}
                        let saved = save_draft(db,previous,draft,draft_version,&sent_draft).await;
                        if let Err(error)=saved {
                            revision=next;
                            updates.send_replace(Snapshot{chat_busy:chat_pending.is_some(),search:search.clone(),jump,history,phone:Default::default(),operation:operation.clone(),destinations:destinations.clone(),revision,view:None,reply:reply.clone(),notice:error.to_string()});
                            continue;
                        }
                        selected=id;revision=next;limit=100;window=None;
                        search=Default::default();jump=Default::default();saved
                    }
                    Command::Draft(chat,text,version) => {
                        if save_draft(db,chat,text,version,&sent_draft).await.is_err() { notice="The draft could not be saved. Keep this window open and copy your text.".into(); }
                        if notice.is_empty() { continue; } else { Ok(()) }
                    }
                    Command::Search(value) => {query=value;Ok(())}
                    Command::Unread(value) => {unread=value;Ok(())}
                    command @ (Command::Older {..} | Command::FetchPhone {..}) => {
                        let (chat,request,seen,explicit)=match command {
                            Command::Older{chat,request,revision}=>(chat,request,revision,false),
                            Command::FetchPhone{chat,revision}=>(chat,0,revision,true),
                            _=>unreachable!(),
                        };
                        if selected!=chat || revision!=seen {continue;}
                        let local=history_boundary.is_some_and(|(c,local,_)|c==chat && local);
                        if local && !explicit {
                            page=Some((chat,request,limit.saturating_add(100)));
                        } else if phone_pending.is_none() {
                            let allowed=phone_sources_open && phone_sources.borrow().is_some()
                                && history_boundary.is_some_and(|(c,_,phone)|c==chat && phone)
                                && phone_cooldowns.get(&chat).is_none_or(|until|*until<=tokio::time::Instant::now());
                            if allowed {
                                match db.phone_history_anchor(chat).await {
                                    Ok(Some(anchor))=>{
                                        let now=tokio::time::Instant::now();
                                        phone_cooldowns.retain(|_,until|*until>now);
                                        if phone_cooldowns.len()>=64 && let Some(oldest)=phone_cooldowns.iter().min_by_key(|(_,until)|**until).map(|(chat,_)|*chat){phone_cooldowns.remove(&oldest);}
                                        phone_cooldowns.insert(chat,now+crate::phone_history::COOLDOWN);
                                        phone_context=Some((chat,request,seen));
                                        phone_pending=Some(crate::phone_history::request(db.clone(),phone_sources.clone(),anchor,request,seen));
                                    }
                                    _=>{history=Some(crate::domain::HistoryReply{chat,request,success:false});notice=crate::phone_history::Failure::Failed.message().into();}
                                }
                            } else {
                                history=Some(crate::domain::HistoryReply{chat,request,success:false});
                                if explicit {notice=if phone_sources.borrow().is_none(){crate::phone_history::Failure::Offline.message().into()}else{"Older history is unavailable or was just requested. Please try again later.".into()};}
                            }
                        } else {continue;}
                        Ok(())
                    }
                    Command::Send {chat,text,request,draft_version} => {
                        trace_send("archive_begin",request);
                        let result = match fresh_id() { Ok(id)=>db.queue(chat,id,text.clone()).await,Err(error)=>Err(error) };
                        if result.is_ok() { sent_draft=Some((chat,text.clone(),draft_version)); }
                        trace_send(if result.is_ok() {"archive_saved"} else {"archive_failed"},request);
                        reply=Some(SendReply {request,chat,text,saved:result.is_ok()});
                        // Confirm the durable save before building the next view.
                        // Keep the reply in later snapshots too, so UI coalescing
                        // cannot lose it. No view means the current UI stays put.
                        updates.send_replace(Snapshot {chat_busy:chat_pending.is_some(),revision,reply:reply.clone(),..Snapshot::empty()});
                        result
                    }
                    Command::NewChat { phone: value, previous, draft, draft_version, revision: next } => {
                        revision=next;phone_pending=None;
                if let Some((chat,request,_))=phone_context.take(){history=Some(crate::domain::HistoryReply{chat,request,success:false});}
                        if let Err(error) = save_draft(db,previous,draft,draft_version,&sent_draft).await {
                            updates.send_replace(Snapshot {chat_busy:chat_pending.is_some(),search:search.clone(),jump,history,phone:Default::default(),operation:operation.clone(),destinations:destinations.clone(),revision,view:None,reply:reply.clone(),notice:error.to_string()});
                            continue;
                        }
                        match crate::protocol_messages::new_chat(&value) {
                            Ok(jid)=>match db.create_chat(jid).await {Ok(id)=>{selected=id;revision=next;limit=100;window=None;search=Default::default();jump=Default::default();Ok(())},Err(e)=>Err(e)},
                            Err(e)=>{notice="Enter a phone number with its country code, such as +40 …".into();Err(e)}
                        }
                    }
                };
                if let Err(error) = result && notice.is_empty() { notice=error.to_string(); }
            }
        }
        if let Some(db) = &archive {
            if search.chat == selected && !search.query.trim().is_empty() {
                match db
                    .search_messages(selected, search.query.clone(), search.request)
                    .await
                {
                    Ok(value) => search = value,
                    Err(error) => {
                        search.hits.clear();
                        search.more = false;
                        notice = error.to_string();
                    }
                }
            } else {
                search.hits.clear();
                search.more = false;
            }
            if let Some((chat, message, request)) = info_target {
                operation = Some(match db.message_info(chat, message).await {
                    Ok(info) => crate::message_actions::OperationReply {
                        request,
                        kind: 7,
                        saved: true,
                        error: String::new(),
                        info: Some(info),
                    },
                    Err(e) => operation_reply(request, 7, &Err(e)),
                });
            }
            let probe = db.measurements();
            let view_started = probe.as_ref().map(|_| std::time::Instant::now());
            match db
                .view_window(
                    selected,
                    page.map_or(limit, |(_, _, target)| target),
                    (query.clone(), unread, false),
                    window,
                    archived,
                )
                .await
            {
                Ok(view) => {
                    if let Some(conversation) = &view.conversation {
                        selected = conversation.chat.id;
                        history_boundary =
                            Some((selected, conversation.older_count > 0, view.phone_more));
                    } else if selected != 0 {
                        selected = 0;
                        window = None;
                        search = Default::default();
                        jump = Default::default();
                        history_boundary = None;
                        info_target = None;
                        page = None;
                        phone_pending = None;
                        if let Some((chat, request, _)) = phone_context.take() {
                            history = Some(crate::domain::HistoryReply {
                                chat,
                                request,
                                success: false,
                            });
                        }
                    }
                    if let Some((chat, request, target)) = page {
                        limit = target;
                        history = Some(crate::domain::HistoryReply {
                            chat,
                            request,
                            success: true,
                        });
                    }
                    let phone = crate::phone_history::Status {
                        available: selected > 0
                            && phone_sources_open
                            && phone_sources.borrow().is_some()
                            && view.phone_more
                            && phone_pending.is_none()
                            && phone_cooldowns
                                .get(&selected)
                                .is_none_or(|until| *until <= tokio::time::Instant::now()),
                        loading: phone_pending.is_some(),
                    };
                    if let (Some(probe), Some(started)) = (&probe, view_started) {
                        probe.record(
                            crate::performance::Metric::ViewToSnapshot,
                            started.elapsed(),
                        );
                    }
                    updates.send_replace(Snapshot {
                        search: search.clone(),
                        jump,
                        chat_busy: chat_pending.is_some(),
                        phone,
                        history,
                        operation: operation.clone(),
                        destinations: destinations.clone(),
                        revision,
                        view: Some(Arc::new(view)),
                        reply: reply.clone(),
                        notice,
                    });
                }
                Err(error) => {
                    if let Some((chat, request, _)) = page {
                        history = Some(crate::domain::HistoryReply {
                            chat,
                            request,
                            success: false,
                        });
                    }
                    updates.send_replace(Snapshot {
                        search: search.clone(),
                        jump,
                        chat_busy: chat_pending.is_some(),
                        phone: Default::default(),
                        history,
                        operation: operation.clone(),
                        destinations: destinations.clone(),
                        revision,
                        view: None,
                        reply: reply.clone(),
                        notice: error.to_string(),
                    });
                }
            }
        }
    }
}

fn operation_reply(
    request: i32,
    kind: u8,
    result: &Result<(), Error>,
) -> crate::message_actions::OperationReply {
    crate::message_actions::OperationReply {
        info: None,
        request,
        kind,
        saved: result.is_ok(),
        error: result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default(),
    }
}
fn fresh_ids(count: usize) -> Result<Vec<String>, Error> {
    if count == 0 || count > crate::message_actions::MAX_SELECTION {
        return Err(Error::Invalid);
    }
    (0..count).map(|_| fresh_id()).collect()
}
pub(crate) fn fresh_id() -> Result<String, Error> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::Database)?;
    use std::fmt::Write;
    let mut id = String::from("3EB0");
    for byte in bytes {
        write!(id, "{byte:02X}").map_err(|_| Error::Database)?;
    }
    Ok(id)
}

async fn save_draft(
    db: &Archive,
    chat: i32,
    text: String,
    version: i32,
    sent: &Option<(i32, String, i32)>,
) -> Result<(), Error> {
    // Switching chats can queue the old composer before its durable send reply
    // reaches Slint. Do not restore that submitted text as an unsent draft.
    if sent
        .as_ref()
        .is_some_and(|(c, t, v)| *c == chat && *v == version && t == &text)
    {
        return Ok(());
    }
    db.draft(chat, text).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{PN, fixture};
    use std::time::Duration;

    async fn revision(views: &mut watch::Receiver<Snapshot>, expected: i32) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while views.borrow().revision != expected {
                views.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn offline_open_with_receipts_disabled_discards_old_intent_but_keeps_private_sync() {
        let (_temp, db) = fixture();
        db.ingest(crate::archive::Batch {
            messages: vec![crate::archive::tests::record("unread")],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let view = db
            .view(1, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        db.mark_visible_read_with_receipts(1, view.messages[0].id, true)
            .await
            .unwrap();
        assert!(db.next_receipt().await.unwrap().is_some());
        let (archives, rx) = watch::channel(None);
        let (commands, command_rx) = mpsc::channel(4);
        let (updates, mut views) = watch::channel(Snapshot::empty());
        let (_sources, sources) = watch::channel(None);
        let (preferences, prefs) = watch::channel(crate::preferences::Preferences {
            send_receipts: false,
            ..Default::default()
        });
        let task = tokio::spawn(run_with_preferences(
            rx, command_rx, updates, sources, prefs,
        ));
        archives.send_replace(Some(db.clone()));
        tokio::time::timeout(Duration::from_secs(3), async {
            while views.borrow().view.is_none() {
                views.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(db.next_receipt().await.unwrap().is_none());
        assert!(db.next_read_sync().await.unwrap().is_some());
        preferences.send_modify(|p| p.send_receipts = true);
        assert!(db.next_receipt().await.unwrap().is_none());
        drop(commands);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn message_info_follows_receipts_only_while_open_and_commands_stop_cleanly() {
        use crate::message_actions::Command as Action;
        let (_temp, db) = fixture();
        let chat = db.create_chat(PN.into()).await.unwrap();
        db.queue(chat, "source".into(), "Original".into())
            .await
            .unwrap();
        db.delivery(PN.into(), "source".into(), "accepted")
            .await
            .unwrap();
        let id = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .messages[0]
            .id;
        let (archives, rx) = watch::channel(None);
        let (commands, receiver) = mpsc::channel(16);
        let (updates, mut views) = watch::channel(Snapshot::empty());
        let task = tokio::spawn(run(rx, receiver, updates));
        archives.send_replace(Some(db.clone()));
        views.changed().await.unwrap();
        commands
            .send(Command::Action(Action::Info {
                chat,
                message: id,
                request: 41,
            }))
            .await
            .unwrap();
        async fn operation(views: &mut watch::Receiver<Snapshot>, request: i32, rows: usize) {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let found = views.borrow().operation.as_ref().is_some_and(|r| {
                        r.request == request
                            && (r.kind != 7
                                || r.info.as_ref().is_some_and(|i| i.rows.len() == rows))
                    });
                    if found {
                        break;
                    }
                    views.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
        }
        operation(&mut views, 41, 0).await;
        db.recipient_receipts(PN.into(), vec!["source".into()], PN.into(), "read", 200)
            .await
            .unwrap();
        operation(&mut views, 41, 1).await;
        commands
            .send(Command::Action(Action::CloseInfo))
            .await
            .unwrap();
        commands
            .send(Command::Action(Action::Edit {
                chat,
                message: id,
                expected: "Original".into(),
                text: "Edited".into(),
                request: 42,
            }))
            .await
            .unwrap();
        operation(&mut views, 42, 0).await;
        assert_eq!(views.borrow().operation.as_ref().unwrap().kind, 4);
        drop(commands);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn history_pages_acknowledge_success_failure_and_ignore_stale_navigation() {
        use crate::archive::{Batch, tests::record};
        let (_temp, db) = fixture();
        let messages = (0..450)
            .map(|i| {
                let mut message = record(&format!("history-{i}"));
                message.timestamp += i;
                message
            })
            .collect();
        db.ingest(Batch {
            messages,
            ..Default::default()
        })
        .await
        .unwrap();
        let chat = db
            .view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats[0]
            .id;
        let (archives, receiver) = watch::channel(None);
        let (commands, rx) = mpsc::channel(32);
        let (updates, mut views) = watch::channel(Snapshot::empty());
        let task = tokio::spawn(run(receiver, rx, updates));
        archives.send_replace(Some(db.clone()));
        tokio::time::timeout(Duration::from_secs(3), views.changed())
            .await
            .unwrap()
            .unwrap();
        commands
            .send(Command::Select {
                id: chat,
                previous: 0,
                draft: String::new(),
                draft_version: 0,
                revision: 1,
            })
            .await
            .unwrap();
        revision(&mut views, 1).await;

        async fn page(views: &mut watch::Receiver<Snapshot>, request: i32) -> Snapshot {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let current = views.borrow().clone();
                    if current.history.is_some_and(|r| r.request == request) {
                        return current;
                    }
                    views.changed().await.unwrap();
                }
            })
            .await
            .unwrap()
        }
        let count = |snapshot: &Snapshot| {
            snapshot
                .view
                .as_ref()
                .unwrap()
                .conversation
                .as_ref()
                .unwrap()
                .messages
                .len()
        };
        for (destination, seen, request) in [(chat, 0, 1), (chat + 100, 1, 2), (chat, 1, 3)] {
            commands
                .send(Command::Older {
                    chat: destination,
                    revision: seen,
                    request,
                })
                .await
                .unwrap();
        }
        let loaded = page(&mut views, 3).await;
        assert!(loaded.history.unwrap().success);
        assert_eq!(
            count(&loaded),
            200,
            "stale requests cannot expand the current page"
        );

        // Induce a real query failure in this disposable encrypted fixture.
        // Retrying must keep the same page size, not skip/accumulate a page.
        db.call(|c| {
            c.execute_batch("ALTER TABLE messages RENAME TO unavailable_messages")?;
            Ok(())
        })
        .await
        .unwrap();
        commands
            .send(Command::Older {
                chat,
                revision: 1,
                request: 4,
            })
            .await
            .unwrap();
        let failed = page(&mut views, 4).await;
        assert!(!failed.history.unwrap().success);
        assert!(failed.view.is_none());
        assert!(!failed.notice.is_empty());
        db.call(|c| {
            c.execute_batch("ALTER TABLE unavailable_messages RENAME TO messages")?;
            Ok(())
        })
        .await
        .unwrap();
        commands
            .send(Command::Older {
                chat,
                revision: 1,
                request: 5,
            })
            .await
            .unwrap();
        let retried = page(&mut views, 5).await;
        assert!(retried.history.unwrap().success);
        assert_eq!(count(&retried), 300);
        commands
            .send(Command::Older {
                chat,
                revision: 1,
                request: 6,
            })
            .await
            .unwrap();
        assert_eq!(count(&page(&mut views, 6).await), 400);
        commands
            .send(Command::Older {
                chat,
                revision: 1,
                request: 7,
            })
            .await
            .unwrap();
        let end = page(&mut views, 7).await;
        assert_eq!(count(&end), 450);
        assert_eq!(
            end.view.unwrap().conversation.as_ref().unwrap().older_count,
            0
        );
        drop(commands);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn snapshots_do_not_mark_reads_and_only_current_display_commands_advance_them() {
        use crate::archive::{Batch, tests::record};
        let (_temp, db) = fixture();
        let mut a = record("a");
        a.timestamp = 100;
        let mut b = record("b");
        b.timestamp = 200;
        db.ingest(Batch {
            messages: vec![a, b],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let chat = db
            .view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats[0]
            .id;
        let (archives, receiver) = watch::channel(None);
        let (commands, rx) = mpsc::channel(32);
        let (updates, mut views) = watch::channel(Snapshot::empty());
        let task = tokio::spawn(run(receiver, rx, updates));
        archives.send_replace(Some(db.clone()));
        tokio::time::timeout(Duration::from_secs(3), views.changed())
            .await
            .unwrap()
            .unwrap();
        commands
            .send(Command::Select {
                id: chat,
                previous: 0,
                draft: String::new(),
                draft_version: 0,
                revision: 1,
            })
            .await
            .unwrap();
        revision(&mut views, 1).await;
        let a = views
            .borrow()
            .view
            .as_ref()
            .unwrap()
            .conversation
            .as_ref()
            .unwrap()
            .messages[0]
            .id;
        assert!(db.next_read_sync().await.unwrap().is_none());
        assert_eq!(views.borrow().view.as_ref().unwrap().chats[0].unread, 2);
        commands
            .send(Command::ReadVisible {
                chat,
                message: a,
                revision: 0,
            })
            .await
            .unwrap();
        commands
            .send(Command::ReadVisible {
                chat: chat + 100,
                message: a,
                revision: 1,
            })
            .await
            .unwrap();
        commands
            .send(Command::Select {
                id: chat,
                previous: 0,
                draft: String::new(),
                draft_version: 0,
                revision: 2,
            })
            .await
            .unwrap();
        revision(&mut views, 2).await;
        assert!(db.next_read_sync().await.unwrap().is_none());
        commands
            .send(Command::ReadVisible {
                chat,
                message: a,
                revision: 2,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if views
                    .borrow()
                    .view
                    .as_ref()
                    .is_some_and(|v| v.chats[0].unread == 1)
                {
                    break;
                }
                views.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let mut c = record("c");
        c.timestamp = 300;
        db.ingest(Batch {
            messages: vec![c],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if views
                    .borrow()
                    .view
                    .as_ref()
                    .is_some_and(|v| v.chats[0].unread == 2)
                {
                    break;
                }
                views.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(db.next_read_sync().await.unwrap().unwrap().through, 100);
        drop(commands);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn failed_new_chat_keeps_current_draft_and_send_clears_a_missed_edit() {
        let (_temp, db) = fixture();
        let chat = db.create_chat(PN.into()).await.unwrap();
        db.draft(chat, "Older saved text".into()).await.unwrap();
        let (archives, rx) = watch::channel(None);
        let (commands, command_rx) = mpsc::channel(32);
        let (updates, mut views) = watch::channel(Snapshot::empty());
        let task = tokio::spawn(run(rx, command_rx, updates));
        archives.send_replace(Some(db.clone()));
        tokio::time::timeout(Duration::from_secs(3), views.changed())
            .await
            .unwrap()
            .unwrap();
        commands
            .send(Command::Select {
                id: chat,
                previous: 0,
                draft: String::new(),
                draft_version: 0,
                revision: 1,
            })
            .await
            .unwrap();
        revision(&mut views, 1).await;
        db.blocking(|c| {c.execute_batch("CREATE TRIGGER fail_draft BEFORE UPDATE OF draft ON chats BEGIN SELECT RAISE(ABORT,'synthetic write failure'); END;")?;Ok(())}).unwrap();
        commands
            .send(Command::NewChat {
                phone: "+15550000002".into(),
                previous: chat,
                draft: "Unsaved new text".into(),
                draft_version: 1,
                revision: 2,
            })
            .await
            .unwrap();
        revision(&mut views, 2).await;
        assert!(views.borrow().view.is_none());
        assert!(!views.borrow().notice.is_empty());
        let view = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap();
        assert_eq!(
            view.chats.len(),
            1,
            "navigation must stop when saving the old draft fails"
        );
        assert_eq!(view.conversation.unwrap().draft, "Older saved text");
        db.blocking(|c| {
            c.execute_batch("DROP TRIGGER fail_draft;")?;
            Ok(())
        })
        .unwrap();
        // No Draft notification for this edit reached the worker. Send carries
        // its complete text and must not resurrect the older saved draft.
        commands
            .send(Command::Send {
                chat,
                text: "Complete latest text 👩🏽‍💻\nSecond line".into(),
                request: 1,
                draft_version: 2,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !views.borrow().reply.as_ref().is_some_and(|r| r.saved) {
                views.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        drop(commands);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        let view = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert!(view.draft.is_empty());
        assert_eq!(
            view.messages.last().unwrap().text,
            "Complete latest text 👩🏽‍💻\nSecond line"
        );
    }

    #[tokio::test]
    async fn send_then_switch_does_not_restore_submitted_text_or_lose_newer_drafts() {
        let (_temp, db) = fixture();
        let first = db.create_chat(PN.into()).await.unwrap();
        let second = db
            .create_chat("15550000002@s.whatsapp.net".into())
            .await
            .unwrap();
        let (archives, receiver) = watch::channel(None);
        let (commands, rx) = mpsc::channel(32);
        let (updates, mut views) = watch::channel(Snapshot::empty());
        let task = tokio::spawn(run(receiver, rx, updates));
        archives.send_replace(Some(db.clone()));
        tokio::time::timeout(Duration::from_secs(3), views.changed())
            .await
            .unwrap()
            .unwrap();
        commands
            .send(Command::Draft(first, "send me".into(), 1))
            .await
            .unwrap();
        commands
            .send(Command::Send {
                chat: first,
                text: "send me".into(),
                request: 1,
                draft_version: 1,
            })
            .await
            .unwrap();
        commands
            .send(Command::Select {
                id: second,
                previous: first,
                draft: "send me".into(),
                draft_version: 1,
                revision: 1,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                views.changed().await.unwrap();
                if views.borrow().revision == 1 {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(
            db.view(first, 100, String::new(), false, false)
                .await
                .unwrap()
                .conversation
                .unwrap()
                .draft
                .is_empty()
        );
        assert!(views.borrow().reply.as_ref().unwrap().saved);
        assert_eq!(db.claim_next().await.unwrap().unwrap().chat, first);
        commands
            .send(Command::Draft(first, "send me".into(), 2))
            .await
            .unwrap();
        // Channel order guarantees this selection observes the newer draft.
        commands
            .send(Command::Select {
                id: first,
                previous: second,
                draft: String::new(),
                draft_version: 2,
                revision: 2,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                views.changed().await.unwrap();
                if views.borrow().revision == 2 {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            views
                .borrow()
                .view
                .as_ref()
                .unwrap()
                .conversation
                .as_ref()
                .unwrap()
                .draft,
            "send me"
        );
        drop(archives); // Closed archive publisher must not spin or stop commands.
        commands
            .send(Command::Search("no match".into()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                views.changed().await.unwrap();
                if views
                    .borrow()
                    .view
                    .as_ref()
                    .is_some_and(|v| v.chats.is_empty())
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        // Closing the UI drops all senders. The final queued edit still has
        // to commit before the service completes and releases its archive.
        commands
            .send(Command::Draft(first, "Final draft on close 👋".into(), 3))
            .await
            .unwrap();
        drop(commands);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            db.view(first, 10, String::new(), false, false)
                .await
                .unwrap()
                .conversation
                .unwrap()
                .draft,
            "Final draft on close 👋"
        );
    }
}
