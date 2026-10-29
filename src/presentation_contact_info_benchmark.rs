//! Synthetic SQLCipher-to-contact-panel measurements on a private display.
use super::*;
use crate::{
    archive::Archive,
    message_content::{Content, Document, Photo, Preview},
    photos, presentation_contact_info, presentation_photos, storage,
};
use std::sync::Mutex;

const CACHED: [i32; 4] = [9596, 9592, 9588, 9584];

fn prepare() -> (tempfile::TempDir, Archive) {
    struct Vault;
    impl storage::SecretVault for Vault {
        fn read(&self) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, crate::connection::Failure> {
            Ok(Some(zeroize::Zeroizing::new(vec![43; 32])))
        }
        fn write(&self, _: &[u8]) -> Result<(), crate::connection::Failure> {
            unreachable!()
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let files = storage::prepare_at(temp.path(), &Vault).unwrap();
    let db = Archive::open(files.clone(), &files.key_sql).unwrap();
    let previews = (0..4)
        .map(|i| {
            let pixels = fixtures::photo_pixels(i, photos::INLINE_EDGE);
            let mut bytes = Vec::new();
            let mut png = png::Encoder::new(&mut bytes, pixels.width, pixels.height);
            png.set_color(png::ColorType::Rgba);
            png.set_depth(png::BitDepth::Eight);
            png.write_header()
                .unwrap()
                .write_image_data(&pixels.rgba)
                .unwrap();
            bytes
        })
        .collect::<Vec<_>>();
    db.blocking(|connection| {
        let tx = connection.transaction()?;
        for id in 1..=500 {
            tx.execute(
                "INSERT INTO chats(id,jid,name,listed) VALUES(?1,?2,?3,1)",
                rusqlite::params![
                    id,
                    if id == 1 {
                        "15550000001@s.whatsapp.net".into()
                    } else {
                        format!("synthetic-contact-{id}")
                    },
                    format!("Synthetic contact {id} · こんにちは")
                ],
            )?;
        }
        for id in 1..=10_000 {
            let mut content = Content {
                body: Some(format!("Synthetic message {id} · 👩🏽‍💻 ăâîșț")),
                ..Default::default()
            };
            // The loaded final page contains text only, so closing the panel
            // can release every contact preview rather than sharing it with
            // a legitimately visible photo bubble.
            if (8000..9600).contains(&id) {
                match id % 4 {
                    0 => {
                        content.photo = Some(Photo {
                            width: 900,
                            height: 1200,
                            size: Some(12345),
                        });
                    }
                    1 => {
                        content.document = Some(Document {
                            file_name: format!("Synthetic report {id} · 東京.pdf"),
                            mime_type: "application/pdf".into(),
                            size: Some(45678),
                            pages: Some(4),
                            has_thumbnail: false,
                        });
                    }
                    2 => {
                        content.preview = Some(Preview {
                            url: format!("https://example.test/synthetic/{id}"),
                            title: format!("Synthetic shared link {id}"),
                            description: "A saved synthetic link; never opened.".into(),
                            has_thumbnail: false,
                        });
                    }
                    _ => {}
                }
            }
            tx.execute(
                "INSERT INTO messages(id,chat_id,sender,wire_id,stamp,outgoing,sender_name,
                 body,raw,visible,source_chat,source_sender,content)
                 VALUES(?1,1,'synthetic',?1,1700000000+?1,0,'Mira Sample',?2,X'00',1,
                   'synthetic','synthetic',?3)",
                rusqlite::params![
                    id,
                    content.body.as_deref().unwrap_or_default(),
                    serde_json::to_string(&content).unwrap()
                ],
            )?;
        }
        for (id, bytes) in CACHED.iter().zip(&previews) {
            tx.execute(
                "INSERT INTO message_photos(message_id,original,preview,state,revision)
                 VALUES(?1,?2,?2,2,1)",
                rusqlite::params![id, bytes],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    (temp, db)
}

#[derive(Clone, Copy)]
enum Operation {
    Profile,
    Browser,
    Close,
}
struct Pending {
    request: i32,
    started: Instant,
    operation: Operation,
    data_seen: bool,
}
#[derive(Default)]
struct Transit {
    pending: Option<Pending>,
    render_pending: Option<(i32, Instant, bool)>,
    data: Vec<f64>,
    previews: Vec<f64>,
    closed: Vec<f64>,
    rendered: Vec<f64>,
}
#[derive(Default)]
struct Publications {
    apply: Vec<f64>,
    max_rows: usize,
}

fn dispatch(ui: &AppWindow, transit: &mut Transit, operation: Operation, tab: i32) {
    let started = Instant::now();
    match operation {
        Operation::Profile => ui.invoke_show_contact_info(),
        Operation::Browser => ui.invoke_request_contact_media(tab),
        Operation::Close => ui.invoke_close_contact_info(),
    }
    let request = ui.get_contact_info_request();
    transit.pending = Some(Pending {
        request,
        started,
        operation,
        data_seen: false,
    });
    transit.render_pending = Some((request, started, matches!(operation, Operation::Close)));
}

fn observe(ui: &AppWindow, transit: &mut Transit) {
    let Some(pending) = &mut transit.pending else {
        return;
    };
    if pending.request != ui.get_contact_info_request() {
        return;
    }
    let ready = !ui.get_contact_info_loading()
        && ui.get_contact_info_chat() == 1
        && ui.get_contact_info_error().is_empty();
    if !matches!(pending.operation, Operation::Close) && ready && !pending.data_seen {
        transit
            .data
            .push(pending.started.elapsed().as_secs_f64() * 1000.);
        pending.data_seen = true;
    }
    let done = match pending.operation {
        Operation::Profile => {
            ready
                && ui.get_contact_info_media().row_count() == 4
                && ui
                    .get_contact_info_media()
                    .iter()
                    .all(|item| item.thumbnail.size().width > 0)
        }
        Operation::Browser => ready && ui.get_contact_info_items().row_count() == 100,
        Operation::Close => {
            !ui.get_contact_info_open()
                && ui.get_contact_info_media().row_count() == 0
                && ui.get_contact_info_items().row_count() == 0
                && CACHED.iter().all(|&id| {
                    crate::presentation::timeline::cached_photo(ui, id)
                        .size()
                        .width
                        == 0
                })
        }
    };
    if done {
        let milliseconds = pending.started.elapsed().as_secs_f64() * 1000.;
        match pending.operation {
            Operation::Profile => transit.previews.push(milliseconds),
            Operation::Browser => {}
            Operation::Close => transit.closed.push(milliseconds),
        }
        transit.pending = None;
    }
}

pub fn start(
    ui: &AppWindow,
    runtime: &Runtime,
) -> Result<Benchmark, slint::SetRenderingNotifierError> {
    let (temp, db) = runtime.block_on(runtime.spawn_blocking(prepare)).unwrap();
    let view = runtime
        .block_on(db.view_window(1, 100, (String::new(), false, false), None, false))
        .unwrap();
    ui.on_load_older(|_, _| false);
    crate::presentation::sidebar(ui, view.chats);
    crate::presentation::conversation(ui, &view.conversation.unwrap(), true);
    ui.set_show_conversation(true);
    ui.set_auto_download_photos(false);
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let messages = ui.get_messages();
    let revision = ui.get_view_revision();
    let (archives, archive_rx) = watch::channel(Some(db.clone()));
    let info = runtime.spawn(presentation_contact_info::attach(ui, archive_rx.clone()));
    let demand = presentation_photos::bind(ui);
    let observed_demand = demand.clone();
    let (updates, snapshots) = watch::channel(Arc::new(photos::Snapshot::default()));
    let producer = runtime.spawn(async move {
        let _keep = (temp, archives);
        photos::present(archive_rx, demand, updates).await;
    });
    let publications = Arc::new(Mutex::new(Publications::default()));
    let published = publications.clone();
    let pump = runtime.spawn(live_presentation::present_latest(
        ui.as_weak(),
        snapshots,
        move |ui, snapshot| {
            let started = Instant::now();
            presentation_photos::publish(ui, snapshot);
            let mut metrics = published.lock().unwrap();
            metrics.apply.push(started.elapsed().as_secs_f64() * 1000.);
            metrics.max_rows = metrics.max_rows.max(snapshot.rows.len());
        },
    ));
    let samples = Rc::new(RefCell::new(Samples::default()));
    let rendered = samples.clone();
    let transit = Rc::new(RefCell::new(Transit::default()));
    let measured = transit.clone();
    let renderer_seen = Rc::new(std::cell::Cell::new(false));
    let rendering = renderer_seen.clone();
    let weak = ui.as_weak();
    let mut render_start = None;
    let mut previous = None;
    ui.window()
        .set_rendering_notifier(move |state, _| match state {
            RenderingState::BeforeRendering => {
                let now = Instant::now();
                if let Some(before) = previous.replace(now) {
                    rendered
                        .borrow_mut()
                        .frame_ms
                        .push(now.duration_since(before).as_secs_f64() * 1000.);
                }
                render_start = Some(now);
                if let Some(ui) = weak.upgrade() {
                    observe(&ui, &mut measured.borrow_mut());
                }
            }
            RenderingState::AfterRendering => {
                rendering.set(true);
                if let Some(started) = render_start.take() {
                    rendered
                        .borrow_mut()
                        .render_ms
                        .push(started.elapsed().as_secs_f64() * 1000.);
                }
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                let mut measured = measured.borrow_mut();
                if let Some((request, started, closed)) = measured.render_pending
                    && request == ui.get_contact_info_request()
                    && (closed && !ui.get_contact_info_open()
                        || !closed
                            && !ui.get_contact_info_loading()
                            && ui.get_contact_info_chat() == 1
                            && ui.get_contact_info_error().is_empty())
                {
                    measured
                        .rendered
                        .push(started.elapsed().as_secs_f64() * 1000.);
                    measured.render_pending = None;
                }
            }
            _ => {}
        })?;
    dispatch(ui, &mut transit.borrow_mut(), Operation::Profile, 0);
    let failure = Arc::new(AtomicBool::new(false));
    let failed = failure.clone();
    let timer = Timer::default();
    let weak = ui.as_weak();
    let started = Instant::now();
    let mut warmed = None;
    let mut last = started;
    let mut phase = 0;
    let mut phase_started = started;
    let mut step = 0;
    let mut max_demand = 0;
    const NAMES: [&str; 7] = [
        "contact-profile-idle",
        "contact-tabs",
        "contact-resize",
        "contact-media-idle",
        "contact-open-close",
        "contact-closed-resize",
        "contact-closed-idle",
    ];
    const ENDS: [u64; 7] = [2, 6, 10, 13, 17, 20, 23];
    println!(
        "Contact info benchmark: SQLCipher, 500 chats and a 10,000-message conversation with mixed saved media/links/docs; 100 fixed loaded text rows, four cached photo previews. Uses actual contact-info attach, archive queries, photo decoder and UI publisher; no account/network. Contact-info publication is opaque: command-to-observed-data-ready includes worker and event-loop scheduling, not a separate UI-apply measurement. Snapshot-apply samples below measure photo publication only. Offscreen rendering excludes GPU completion and compositor presentation."
    );
    timer.start(TimerMode::Repeated, TICK, move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let now = Instant::now();
        let mut transit = transit.borrow_mut();
        observe(&ui, &mut transit);
        if warmed.is_none() {
            if transit.pending.is_none() && started.elapsed() >= Duration::from_millis(500) {
                warmed = Some(now);
                phase_started = now;
                last = now;
                samples.replace(Samples::default());
                transit.data.clear();
                transit.previews.clear();
                transit.closed.clear();
                transit.rendered.clear();
                publications.lock().unwrap().apply.clear();
                crate::presentation::timeline::take_samples();
            } else if started.elapsed() > Duration::from_secs(15) {
                eprintln!("Contact info benchmark failed to warm up: loading={}, previews={}",ui.get_contact_info_loading(),ui.get_contact_info_media().row_count());
                failed.store(true, Ordering::Release);
                let _ = slint::quit_event_loop();
                return;
            } else {
                return;
            }
        }
        if ui.get_messages() != messages || ui.get_view_revision() != revision {
            eprintln!("Contact info benchmark rebuilt/replaced the conversation model.");
            failed.store(true, Ordering::Release);
        }
        max_demand = max_demand.max(observed_demand.borrow().visible.len());
        if max_demand > photos::MAX_DEMAND
            || ui.get_contact_info_media().row_count() > 4
            || ui.get_contact_info_items().row_count() > 100
            || publications.lock().unwrap().max_rows > photos::MAX_DEMAND
        {
            eprintln!("Contact info benchmark exceeded a media/model bound.");
            failed.store(true, Ordering::Release);
        }
        if !ui.get_contact_info_error().is_empty() {
            eprintln!("Contact info benchmark received a synthetic archive error.");
            failed.store(true, Ordering::Release);
        }
        let mut samples = samples.borrow_mut();
        samples.tick_late_ms.push(now.duration_since(last).saturating_sub(TICK).as_secs_f64() * 1000.);
        samples.apply_ms.append(&mut publications.lock().unwrap().apply);
        last = now;
        let begun = warmed.unwrap();
        let seconds = begun.elapsed().as_secs();
        let next = ENDS.iter().position(|&end| seconds < end).unwrap_or(NAMES.len());
        if next != phase {
            report(NAMES[phase], &mut samples);
            if next == NAMES.len() {
                println!("Contact command-to-observed-data-ready: samples={}, p50={:.2}ms p95={:.2}ms max={:.2}ms",transit.data.len(),percentile(&mut transit.data,0.5),percentile(&mut transit.data,0.95),percentile(&mut transit.data,1.));
                println!("Contact profile-to-four-textured-previews: samples={}, p50={:.2}ms p95={:.2}ms max={:.2}ms",transit.previews.len(),percentile(&mut transit.previews,0.5),percentile(&mut transit.previews,0.95),percentile(&mut transit.previews,1.));
                println!("Contact close-to-preview-release: samples={}, p50={:.2}ms p95={:.2}ms max={:.2}ms",transit.closed.len(),percentile(&mut transit.closed,0.5),percentile(&mut transit.closed,0.95),percentile(&mut transit.closed,1.));
                if renderer_seen.get() {
                    println!("Contact command-to-render-complete: samples={}, p50={:.2}ms p95={:.2}ms max={:.2}ms",transit.rendered.len(),percentile(&mut transit.rendered,0.5),percentile(&mut transit.rendered,0.95),percentile(&mut transit.rendered,1.));
                } else {
                    println!("Contact command-to-render timing unavailable on this backend.");
                }
                println!("Contact bounds: max demand={max_demand}, max photo snapshot rows={}, unchanged conversation rows={}",publications.lock().unwrap().max_rows,ui.get_messages().row_count());
                if transit.data.len() < 5 || transit.previews.len() < 2 || transit.closed.len() < 2
                    || ui.get_contact_info_open() || ui.get_contact_info_media().row_count() != 0
                    || ui.get_contact_info_items().row_count() != 0 || !observed_demand.borrow().visible.is_empty()
                    || CACHED.iter().any(|&id| crate::presentation::timeline::cached_photo(&ui,id).size().width != 0)
                {
                    eprintln!("Contact info benchmark did not finish readiness/cleanup assertions.");
                    failed.store(true, Ordering::Release);
                }
                let _ = slint::quit_event_loop();
                return;
            }
            *samples = Samples::default();
            phase = next;
            phase_started = now;
            step = 0;
            match phase {
                2 => dispatch(&ui, &mut transit, Operation::Profile, 0),
                3 => dispatch(&ui, &mut transit, Operation::Browser, 0),
                5 => dispatch(&ui, &mut transit, Operation::Close, 0),
                _ => {}
            }
        }
        if matches!(phase, 1 | 4) && transit.pending.is_none() {
            let new_step = phase_started.elapsed().as_millis() / 400 + 1;
            if new_step != step {
                step = new_step;
                if phase == 4 {
                    let operation = if ui.get_contact_info_open() { Operation::Close } else { Operation::Profile };
                    dispatch(&ui, &mut transit, operation, 0);
                } else {
                    match step % 5 {
                        0 => dispatch(&ui, &mut transit, Operation::Profile, 0),
                        1 => dispatch(&ui, &mut transit, Operation::Browser, 0),
                        2 => dispatch(&ui, &mut transit, Operation::Browser, 1),
                        3 => dispatch(&ui, &mut transit, Operation::Browser, 2),
                        _ => dispatch(&ui, &mut transit, Operation::Profile, 0),
                    }
                }
            }
        }
        if matches!(phase, 2 | 5) {
            let n = (phase_started.elapsed().as_millis() / 48 % 32) as f32;
            ui.window().set_size(slint::LogicalSize::new(640. + n * 18., 500. + n * 8.));
        }
        if phase == 6 {
            // A closed contact service must remain closed across invalidations;
            // this does not alter message contents or rebuild the conversation.
            let new_step = phase_started.elapsed().as_millis() / 250 + 1;
            if new_step != step {
                step = new_step;
                db.changed();
            }
        }
    });
    Ok(Benchmark {
        failure: Some(failure),
        _timer: timer,
        producer,
        pump,
        photos: vec![info],
    })
}
