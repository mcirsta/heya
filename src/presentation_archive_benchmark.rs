//! Synthetic traffic through the production encrypted archive, worker and UI pump.
use super::*;
use crate::{
    archive::{Archive, Batch, Record},
    performance::{Metric, Probe},
    storage,
};
use std::{
    collections::BTreeMap,
    sync::{Mutex, atomic::AtomicI32},
};
use tokio::sync::mpsc;
const NAMES: [&str; 8] = [
    "idle-100",
    "incoming-100",
    "unrelated-100",
    "paging-to-10000",
    "wheel-10000",
    "incoming-10000",
    "unrelated-10000",
    "navigation",
];
#[derive(Clone, Copy)]
struct Stamp {
    started: Instant,
    phase: usize,
}
#[derive(Default)]
struct Flow {
    starts: BTreeMap<u64, Stamp>,
    commands: BTreeMap<i32, Stamp>,
    seen: u64,
    frame: Option<(u64, Stamp)>,
    command_frame: Option<Stamp>,
    counts: [[usize; 3]; 8], // issued, applied, rendered; discarded values remain visible in counts
}
fn prepare() -> (tempfile::TempDir, Archive) {
    struct Vault;
    impl storage::SecretVault for Vault {
        fn read(&self) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, crate::connection::Failure> {
            Ok(Some(zeroize::Zeroizing::new(vec![31; 32])))
        }
        fn write(&self, _: &[u8]) -> Result<(), crate::connection::Failure> {
            unreachable!()
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let files = storage::prepare_at(temp.path(), &Vault).unwrap();
    let db = Archive::open(files.clone(), &files.key_sql).unwrap();
    let settings = std::env::var_os("HEYA_BENCHMARK_CHAT_SETTINGS").is_some();
    let unlinked = std::env::var_os("HEYA_BENCHMARK_UNLINKED_ALBUM").is_some();
    if unlinked {
        println!(
            "Archive benchmark: unlinked 37-photo batches separated by text; inferred metadata uses production queries"
        );
    }
    if settings {
        println!("Archive benchmark chat settings: 1500 indexed flags, 250 muted chats");
    }
    db.blocking(|c| {
        let tx=c.transaction()?;
        for chat in 1..=500 {
            tx.execute("INSERT INTO chats(id,jid,name) VALUES(?1,?2,?3)",rusqlite::params![chat,format!("{}@s.whatsapp.net",15550000000i64+chat),format!("Synthetic conversation {chat}")])?;
            if settings {
                for kind in 0..3 {
                    tx.execute("INSERT INTO chat_settings(jid,kind,value,stamp) VALUES(?1,?2,?3,1700000000000)",rusqlite::params![format!("{}@s.whatsapp.net",15550000000i64+chat),kind,if kind==0 && chat%2==0 {-1} else {0}])?;
                }
            }
        }
        for i in 0..10500 {
            let chat=if i<10000 {1} else {2+(i-10000)%499};
            let text=match i%4 {
                0=>format!("Synthetic note {i} 👩🏽‍💻 · مرحبا · こんにちは"),
                1=>format!("Synthetic multiline {i}\nFirst line\nSecond line\nA full caption."),
                2=>format!("Synthetic long message {i}. {}","A line that wraps when the window is resized. ".repeat(12)),
                _=>format!("Short message {i}"),
            };
            let photo=unlinked && i<10000 && i%38!=37;
            let content=if photo {serde_json::to_string(&crate::message_content::Content {
                body:Some(String::new()),photo:Some(crate::message_content::Photo {width:900,height:1200,size:Some(12345)}),..Default::default()
            }).unwrap()} else {String::new()};
            tx.execute("INSERT INTO messages(chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,source_chat,source_sender,content) VALUES(?1,'synthetic',?2,?3,0,'Synthetic',?4,X'00',1,'synthetic','synthetic',?5)",rusqlite::params![chat,format!("SEED-{i}"),1700000000+i,if photo {"[Photo]".into()} else {text},content])?;
        }
        tx.commit()?;Ok(())
    }).unwrap();
    (temp, db)
}
fn traffic(seq: u64, chat: i32) -> Record {
    let jid = format!("{}@s.whatsapp.net", 15550000000i64 + i64::from(chat));
    Record {
        chat: jid.clone(),
        sender: jid,
        id: format!("PERF-{seq}"),
        timestamp: 1700020000 + seq as i64,
        outgoing: false,
        sender_name: "Synthetic".into(),
        text: format!("PERF-{seq} A synthetic received message 👩🏽‍💻\nMultiline text."),
        raw: vec![0],
        content: Default::default(),
        album: None,
        thumbnail: Vec::new(),
        visible: true,
        expires: None,
        change: None,
    }
}
async fn wait(
    rx: &mut watch::Receiver<messaging::Snapshot>,
    test: impl Fn(&messaging::Snapshot) -> bool,
) -> Result<(), ()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if test(&rx.borrow_and_update()) {
                return Ok(());
            }
            rx.changed().await.map_err(|_| ())?;
        }
    })
    .await
    .map_err(|_| ())?
}

// Deliberate native checks through the production composer and encrypted outbox.
// This module is entered only by the explicit synthetic --demo benchmark.
async fn check_long_sends(
    window: slint::Weak<AppWindow>,
    snapshots: &mut watch::Receiver<messaging::Snapshot>,
    db: &Archive,
) -> Result<(), ()> {
    const LINE: &str = "Long synthetic message 👩🏽‍💻 ăâîșț.\n";
    for size in [128, 8 * 1024, 32 * 1024, 64 * 1024] {
        let text = LINE.repeat(size / LINE.len());
        let bytes = text.len();
        let expected = text.clone();
        let queued_edits = if std::env::var_os("HEYA_BENCHMARK_TYPING").is_some() {
            200
        } else {
            0
        };
        let setup_started = Instant::now();
        let (answer, result) = tokio::sync::oneshot::channel();
        window
            .upgrade_in_event_loop(move |ui| {
                ui.set_demo_mode(false);
                ui.set_connection_state(5);
                ui.set_show_conversation(true);
                for version in 0..queued_edits {
                    ui.set_draft(format!("{text}{version}").into());
                    ui.invoke_draft_edited(ui.get_draft());
                }
                ui.set_draft(text.into());
                ui.invoke_draft_edited(ui.get_draft());
                ui.invoke_focus_composer();
                let entered = Instant::now();
                for _ in 0..2 {
                    ui.window()
                        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                            text: slint::platform::Key::Return.into(),
                        });
                    ui.window()
                        .dispatch_event(slint::platform::WindowEvent::KeyReleased {
                            text: slint::platform::Key::Return.into(),
                        });
                }
                let _ = answer.send(ui.get_send_pending().then_some((
                    ui.get_send_request_id(),
                    entered,
                    ui.get_view_revision(),
                )));
            })
            .map_err(|_| ())?;
        let (request, started, previous_view) = result.await.map_err(|_| ())?.ok_or(())?;
        wait(snapshots, |s| {
            s.reply
                .as_ref()
                .is_some_and(|r| r.request == request && r.saved)
        })
        .await?;
        let saved = started.elapsed();
        let mut reply_applied = None;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (answer, result) = tokio::sync::oneshot::channel();
                window
                    .upgrade_in_event_loop(move |ui| {
                        let _ = answer.send((
                            !ui.get_send_pending() && ui.get_draft().is_empty(),
                            ui.get_view_revision() > previous_view,
                        ));
                    })
                    .map_err(|_| ())?;
                let (replied, shown) = result.await.map_err(|_| ())?;
                if replied {
                    reply_applied.get_or_insert_with(|| started.elapsed());
                    if shown {
                        return Ok::<_, ()>(());
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| ())??;
        let view_applied = started.elapsed();
        let applied = reply_applied.ok_or(())?;
        let total = setup_started.elapsed();
        let count: i32 = db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM messages WHERE sender='#self' AND body=?1",
                    [expected],
                    |r| r.get(0),
                )?)
            })
            .await
            .map_err(|_| ())?;
        if count != 1 {
            return Err(());
        }
        println!(
            "Native long send: bytes={bytes}, queued_edits={queued_edits}, Enter-to-saved-snapshot={:.2}ms, Enter-to-UI-reply={:.2}ms, Enter-to-UI-view={:.2}ms, composer-setup-to-UI-view={:.2}ms, archived_copies={count}; synthetic account, no network delivery",
            saved.as_secs_f64() * 1000.,
            applied.as_secs_f64() * 1000.,
            view_applied.as_secs_f64() * 1000.,
            total.as_secs_f64() * 1000.
        );
    }
    Ok(())
}

pub fn start(
    ui: &AppWindow,
    runtime: &Runtime,
) -> Result<Benchmark, slint::SetRenderingNotifierError> {
    let (temp, db) = runtime.block_on(runtime.spawn_blocking(prepare)).unwrap();
    let probe = Arc::new(Probe::default());
    db.enable_measurements(probe.clone());
    let flow = Arc::new(Mutex::new(Flow::default()));
    let desired = Arc::new(AtomicI32::new(1));
    let (commands, command_rx) = mpsc::channel(256);
    let (archives, archive_rx) = watch::channel(None);
    let (sources, source_rx) = watch::channel(None);
    let (updates, mut snapshots) = watch::channel(messaging::Snapshot::empty());
    crate::live_presentation::bind_messages(ui, commands.clone());
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let worker = runtime.spawn(messaging::run_with_history(
        archive_rx, command_rx, updates, source_rx,
    ));
    archives.send_replace(Some(db.clone()));
    let pump = {
        let probe = probe.clone();
        let flow = flow.clone();
        let desired = desired.clone();
        runtime.spawn(live_presentation::present_latest(
            ui.as_weak(),
            snapshots.clone(),
            move |ui, snapshot| {
                if snapshot.revision != desired.load(Ordering::Relaxed) {
                    return;
                }
                ui.set_live_revision(snapshot.revision);
                let start = Instant::now();
                live_presentation::publish_messages(ui, snapshot);
                probe.record(Metric::UiApply, start.elapsed());
                let mut flow = flow.lock().unwrap();
                if let Some(stamp) = flow.commands.remove(&snapshot.revision) {
                    flow.command_frame = Some(stamp);
                }
                let seq = snapshot.view.as_ref().and_then(|v| {
                    v.chats
                        .iter()
                        .filter_map(|c| {
                            c.preview
                                .strip_prefix("PERF-")
                                .and_then(|p| p.split_whitespace().next())
                                .and_then(|n| n.parse::<u64>().ok())
                        })
                        .max()
                });
                if let Some(seq) = seq.filter(|seq| *seq > flow.seen)
                    && let Some(stamp) = flow.starts.get(&seq).copied()
                {
                    flow.seen = seq;
                    flow.frame = Some((seq, stamp));
                    flow.counts[stamp.phase][1] += 1;
                    probe.record_in(stamp.phase, Metric::IngestToUi, stamp.started.elapsed());
                }
            },
        ))
    };
    let samples = Rc::new(RefCell::new(Samples::default()));
    let render_samples = samples.clone();
    let render_probe = probe.clone();
    let render_flow = flow.clone();
    let mut render_start = None;
    let mut previous = None;
    ui.window()
        .set_rendering_notifier(move |state, _| match state {
            RenderingState::BeforeRendering => {
                let now = Instant::now();
                if let Some(previous) = previous.replace(now) {
                    render_samples
                        .borrow_mut()
                        .frame_ms
                        .push(now.duration_since(previous).as_secs_f64() * 1000.);
                }
                render_start = Some(now);
            }
            RenderingState::AfterRendering => {
                if let Some(start) = render_start.take() {
                    render_samples
                        .borrow_mut()
                        .render_ms
                        .push(start.elapsed().as_secs_f64() * 1000.);
                }
                let mut flow = render_flow.lock().unwrap();
                if let Some((seq, stamp)) = flow.frame.take() {
                    render_probe.record_in(
                        stamp.phase,
                        Metric::IngestToRender,
                        stamp.started.elapsed(),
                    );
                    flow.counts[stamp.phase][2] += 1;
                    flow.starts.retain(|key, _| *key > seq);
                }
                if let Some(stamp) = flow.command_frame.take() {
                    render_probe.record_in(
                        stamp.phase,
                        Metric::CommandToRender,
                        stamp.started.elapsed(),
                    );
                }
            }
            _ => {}
        })?;
    let failure = Arc::new(AtomicBool::new(false));
    let producer_failure = failure.clone();
    let producer_probe = probe.clone();
    let producer_flow = flow.clone();
    let send_window = ui.as_weak();
    let long_sends = std::env::var_os("HEYA_BENCHMARK_LONG_DRAFT").is_some();
    let producer = runtime.spawn(async move {
        let _keep = (temp, archives, sources);
        let run = async {
            wait(&mut snapshots, |s| s.view.is_some()).await?;
            commands
                .send(messaging::Command::Select {
                    id: 1,
                    previous: 0,
                    draft: String::new(),
                    draft_version: 0,
                    revision: 1,
                })
                .await
                .map_err(|_| ())?;
            wait(&mut snapshots, |s| {
                s.revision == 1 && s.view.as_ref().is_some_and(|v| v.conversation.is_some())
            })
            .await?;
            let mut seq = 0;
            for phase in 0..NAMES.len() {
                producer_probe.set_phase(phase);
                if phase == 3 {
                    let start = Instant::now();
                    for request in 1..=99 {
                        commands
                            .send(messaging::Command::Older {
                                chat: 1,
                                request,
                                revision: 1,
                            })
                            .await
                            .map_err(|_| ())?;
                        wait(&mut snapshots, |s| {
                            s.history.is_some_and(|r| r.request == request && r.success)
                        })
                        .await?;
                    }
                    println!(
                        "Archive benchmark paging: 99 real page commands, {:.2}ms total",
                        start.elapsed().as_secs_f64() * 1000.
                    );
                }
                let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                let mut revision = 1;
                while tokio::time::Instant::now() < deadline {
                    if matches!(phase, 1 | 2 | 5 | 6) {
                        let count = if matches!(phase, 5 | 6) { 20 } else { 1 };
                        let mut messages = Vec::with_capacity(count);
                        let started = Instant::now();
                        for _ in 0..count {
                            seq += 1;
                            let stamp = Stamp { started, phase };
                            {
                                let mut flow = producer_flow.lock().unwrap();
                                flow.starts.insert(seq, stamp);
                                flow.counts[phase][0] += 1;
                            }
                            messages.push(traffic(seq, if matches!(phase, 2 | 6) { 2 } else { 1 }));
                        }
                        db.ingest(Batch {
                            messages,
                            ..Default::default()
                        })
                        .await
                        .map_err(|_| ())?;
                        producer_probe.record_in(phase, Metric::Ingest, started.elapsed());
                    } else if phase == 7 {
                        revision += 1;
                        desired.store(revision, Ordering::Relaxed);
                        producer_flow.lock().unwrap().commands.insert(
                            revision,
                            Stamp {
                                started: Instant::now(),
                                phase,
                            },
                        );
                        commands
                            .send(messaging::Command::Select {
                                id: if revision % 2 == 0 { 2 } else { 1 },
                                previous: 0,
                                draft: String::new(),
                                draft_version: 0,
                                revision,
                            })
                            .await
                            .map_err(|_| ())?;
                    }
                    tokio::time::sleep(Duration::from_millis(if phase == 7 { 250 } else { 50 }))
                        .await;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            if long_sends {
                check_long_sends(send_window, &mut snapshots, &db).await?;
            }
            Ok::<_, ()>(())
        }
        .await;
        if run.is_err() {
            producer_failure.store(true, Ordering::Release);
            eprintln!("Archive benchmark failed: a synthetic operation timed out or was rejected.");
        }
        producer_probe.set_phase(if run.is_ok() {
            NAMES.len()
        } else {
            NAMES.len() + 1
        });
    });
    let timer = Timer::default();
    let weak = ui.as_weak();
    let mut phase = 0;
    let mut last = Instant::now();
    timer.start(TimerMode::Repeated,TICK,move || {
        let Some(ui)=weak.upgrade() else{return;};
        let now=Instant::now();samples.borrow_mut().tick_late_ms.push(now.duration_since(last).saturating_sub(TICK).as_secs_f64()*1000.);last=now;
        let current=probe.phase();
        if current!=phase {
            report(NAMES[phase],&mut samples.borrow_mut());probe.report(phase,NAMES[phase]);
            let counts=flow.lock().unwrap().counts[phase];
            println!("Archive benchmark {}: issued={}, applied={}, rendered={} (latest-value coalescing; unrendered updates are not latency samples)",NAMES[phase],counts[0],counts[1],counts[2]);
            *samples.borrow_mut()=Samples::default();
            if current>=NAMES.len(){let _=slint::quit_event_loop();return;}
            phase=current;
        }
        if phase==4 {
            crate::presentation_scroll::dispatch_wheel(ui.window(),slint::LogicalPosition::new(800.,350.),0.,80.);
        }
    });
    println!(
        "Archive benchmark: 500 chats, 10,000-message encrypted conversation; production SQLCipher/archive/messaging/coalesced UI path. Synthetic traffic only; no account, network, compositor presentation or GPU completion timing."
    );
    Ok(Benchmark {
        failure: Some(failure),
        _timer: timer,
        producer,
        pump,
        photos: vec![worker],
    })
}
