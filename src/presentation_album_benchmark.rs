//! Synthetic album through the encrypted archive, actual decoder and renderer.
use super::*;
use crate::{archive::Archive, photos, presentation_photos, storage};

#[derive(Default)]
struct Publications {
    apply: Vec<f64>,
    pending: Option<(i32, Instant)>,
    ready: Vec<f64>,
}

pub fn start(
    ui: &AppWindow,
    runtime: &Runtime,
) -> Result<Benchmark, slint::SetRenderingNotifierError> {
    let (temp,db)=runtime.block_on(runtime.spawn_blocking(|| {
        struct Vault;
        impl storage::SecretVault for Vault {
            fn read(&self)->Result<Option<zeroize::Zeroizing<Vec<u8>>>,crate::connection::Failure>{Ok(Some(zeroize::Zeroizing::new(vec![29;32])))}
            fn write(&self,_:&[u8])->Result<(),crate::connection::Failure>{unreachable!()}
        }
        let temp=tempfile::tempdir().unwrap();
        let files=storage::prepare_at(temp.path(),&Vault).unwrap();
        let db=Archive::open(files.clone(),&files.key_sql).unwrap();
        let encode=|edge,index| {
            let p=fixtures::photo_pixels(index,edge);
            let mut bytes=Vec::new();
            let mut encoder=png::Encoder::new(&mut bytes,p.width,p.height);
            encoder.set_color(png::ColorType::Rgba);encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&p.rgba).unwrap();bytes
        };
        let originals=(0..3).map(|i|encode(photos::VIEWER_EDGE,i)).collect::<Vec<_>>();
        let previews=(0..3).map(|i|encode(photos::INLINE_EDGE,i)).collect::<Vec<_>>();
        db.blocking(|c| {
            let tx=c.transaction()?;
            tx.execute("INSERT INTO chats(id,jid,name) VALUES(1,'synthetic-album','Synthetic album')",[])?;
            for id in 1_i32..=10000 {
                let photo=id>9900;
                let content=if photo {serde_json::to_string(&crate::message_content::Content{body:Some(format!("Synthetic album photo {} · こんにちは 👋",id-9900)),photo:Some(crate::message_content::Photo{width:2048,height:1536,size:Some(12345)}),..Default::default()}).unwrap()}else{String::new()};
                tx.execute("INSERT INTO messages(id,chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,source_chat,source_sender,content,album,album_index) VALUES(?1,1,'synthetic',?1,1700000000+?1,0,'Mira Sample','Synthetic text',X'00',1,'synthetic','synthetic',?2,?3,?4)",rusqlite::params![id,content,photo.then_some("explicit-album"),photo.then_some(id-9901)])?;
                if photo {tx.execute("INSERT INTO message_photos(message_id,original,preview,state,revision) VALUES(?1,?2,?3,2,1)",rusqlite::params![id,originals[id as usize%3],previews[id as usize%3]])?;}
            }
            tx.commit()?;Ok(())
        }).unwrap();
        (temp,db)
    })).unwrap();
    // Only the newest photo is loaded in the transcript; every previous click
    // must use the album index rather than growing/rebuilding that transcript.
    let view = runtime
        .block_on(db.view_window(1, 1, (String::new(), false, false), None, false))
        .unwrap()
        .conversation
        .unwrap();
    // This benchmark owns a fixed one-row archive window. Do not route its
    // automatic paging to the separately running offline demo service.
    ui.on_load_older(|_, _| false);
    crate::presentation::conversation(ui, &view, true);
    ui.set_show_conversation(true);
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let demand = presentation_photos::bind(ui);
    let (updates, snapshots) = watch::channel(Arc::new(photos::Snapshot::default()));
    let producer = runtime.spawn(async move {
        let (_archives, archives) = watch::channel(Some(db));
        photos::present(archives, demand, updates).await;
        drop(temp);
    });
    let apply = Arc::new(std::sync::Mutex::new(Publications::default()));
    let applied = apply.clone();
    let pump = runtime.spawn(live_presentation::present_latest(
        ui.as_weak(),
        snapshots,
        move |ui, snapshot| {
            let started = Instant::now();
            presentation_photos::publish(ui, snapshot);
            let mut samples = applied.lock().unwrap();
            samples.apply.push(started.elapsed().as_secs_f64() * 1000.);
            if ui.get_photo_viewer_ready()
                && samples
                    .pending
                    .is_some_and(|(id, _)| ui.get_photo_viewer_id() == id)
            {
                let (_, start) = samples.pending.take().unwrap();
                samples.ready.push(start.elapsed().as_secs_f64() * 1000.);
            }
        },
    ));
    let samples = Rc::new(RefCell::new(Samples::default()));
    let rendered = samples.clone();
    let pending = Rc::new(RefCell::new(None::<(i32, Instant)>));
    let completed = pending.clone();
    let latencies = Rc::new(RefCell::new(Vec::new()));
    let render_times = latencies.clone();
    let renderer_seen = Rc::new(std::cell::Cell::new(false));
    let rendering = renderer_seen.clone();
    let weak = ui.as_weak();
    let mut before = None;
    let mut previous = None;
    ui.window()
        .set_rendering_notifier(move |state, _| match state {
            RenderingState::BeforeRendering => {
                let now = Instant::now();
                if let Some(prev) = previous.replace(now) {
                    rendered
                        .borrow_mut()
                        .frame_ms
                        .push(now.duration_since(prev).as_secs_f64() * 1000.);
                }
                before = Some(now);
            }
            RenderingState::AfterRendering => {
                rendering.set(true);
                if let Some(start) = before.take() {
                    rendered
                        .borrow_mut()
                        .render_ms
                        .push(start.elapsed().as_secs_f64() * 1000.);
                }
                let selection = *completed.borrow();
                if let Some(ui) = weak.upgrade()
                    && let Some((id, start)) = selection
                    && ui.get_photo_viewer_id() == id
                    && ui.get_photo_viewer_ready()
                {
                    render_times
                        .borrow_mut()
                        .push(start.elapsed().as_secs_f64() * 1000.);
                    completed.replace(None);
                }
            }
            _ => {}
        })?;
    ui.invoke_activate_photo(10000);
    *pending.borrow_mut() = Some((10000, Instant::now()));
    let failure = Arc::new(AtomicBool::new(false));
    let failed = failure.clone();
    let timer = Timer::default();
    let weak = ui.as_weak();
    let started = Instant::now();
    let mut last = started;
    let mut warmed = None::<Instant>;
    let mut phase = 0;
    let mut phase_started = started;
    let mut step = 0;
    const NAMES: [&str; 8] = [
        "album-idle",
        "album-navigation",
        "album-resize",
        "album-resize-settled",
        "album-resize-no-strip",
        "album-resize-no-image",
        "album-resize-closed",
        "album-closed",
    ];
    const ENDS: [u64; 8] = [2, 6, 9, 12, 15, 18, 21, 23];
    println!(
        "Album benchmark: SQLCipher with 10,000 messages, 100 explicitly linked photos, ONE loaded timeline row; one 2048px viewer and seven 96px cached thumbnails. Resize repeats the same size sequence with the strip removed, then the photo removed, then the viewer closed. Timings exclude network/GPU completion/compositor presentation; frame callbacks force full redraws."
    );
    timer.start(TimerMode::Repeated, TICK, move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let now = Instant::now();
        if warmed.is_none() {
            if ui.get_photo_viewer_ready() && started.elapsed()>=Duration::from_millis(500) {
                warmed=Some(now);last=now;
                samples.replace(Samples::default());
                latencies.borrow_mut().clear();
                let mut p=apply.lock().unwrap();p.apply.clear();p.ready.clear();
            } else if started.elapsed()>Duration::from_secs(30) {
                eprintln!("Album benchmark warm-up failed: selected={}, ready={}, members={}",ui.get_photo_viewer_id(),ui.get_photo_viewer_ready(),ui.get_photo_gallery_ids().row_count());
                failed.store(true,Ordering::Release);let _=slint::quit_event_loop();return;
            } else {return;}
        }
        let begun=warmed.unwrap();
        let mut samples = samples.borrow_mut();
        samples
            .tick_late_ms
            .push(now.duration_since(last).saturating_sub(TICK).as_secs_f64() * 1000.);
        last = now;
        samples.apply_ms.append(&mut apply.lock().unwrap().apply);
        let seconds = begun.elapsed().as_secs();
        let next = ENDS.iter().position(|&end| seconds < end).unwrap_or(NAMES.len());
        if next != phase {
            report(NAMES[phase], &mut samples);
            if phase == 0
                && (!ui.get_photo_viewer_ready() || ui.get_photo_gallery_ids().row_count() != 100)
            {
                eprintln!("Album benchmark initial selection/membership did not become ready.");
                failed.store(true, Ordering::Release);
            }
            if phase == 7
                && (ui.get_photo_viewer_id() != 0
                    || ui.get_photo_viewer_image().size().width != 0
                    || ui.get_photo_thumbnails().row_count() != 0)
            {
                eprintln!("Album benchmark viewer resources were retained after close.");
                failed.store(true, Ordering::Release);
            }
            if next == NAMES.len() {
                let mut ready=apply.lock().unwrap();
                println!("Album selection-to-UI-ready: samples={}, p50={:.2}ms p95={:.2}ms max={:.2}ms",ready.ready.len(),percentile(&mut ready.ready,0.5),percentile(&mut ready.ready,0.95),percentile(&mut ready.ready,1.));
                if renderer_seen.get() {
                    let mut times=latencies.borrow_mut();
                    println!("Album selection-to-render: samples={}, p50={:.2}ms p95={:.2}ms max={:.2}ms",times.len(),percentile(&mut times,0.5),percentile(&mut times,0.95),percentile(&mut times,1.));
                } else {println!("Album selection-to-render unavailable on this backend; UI-ready samples exclude rendering.");}
                println!("Album retained timeline rows: {}",ui.get_messages().row_count());
                if ready.ready.len()<5 || ui.get_messages().row_count()!=1 {failed.store(true,Ordering::Release);}
                let _ = slint::quit_event_loop();
                return;
            }
            *samples = Samples::default();
            phase = next;
            phase_started = now;
            if phase == 4 {
                ui.set_photo_thumbnails(Default::default());
            }
            if phase == 5 {
                // Clearing only the UI image would be undone by the next
                // timeline resize refresh. Replace the retained viewer too.
                crate::presentation::timeline::photos(&ui, &Arc::new(photos::Snapshot {
                    checked: Some((1, ui.get_photo_viewer_id())),
                    viewer: Some((ui.get_photo_viewer_id(), Arc::new(photos::Pixels {
                        width: 1, height: 1, rgba: vec![0; 4],
                    }))),
                    ..Default::default()
                }));
            }
            if phase == 6 {
                ui.invoke_close_photo();
            }
        }
        if phase == 1 {
            let new_step = begun.elapsed().as_millis() / 220;
            if new_step != step && apply.lock().unwrap().pending.is_none() {
                step = new_step;
                let id = 10000 - (step % 60) as i32;
                let selection=Some((id,Instant::now()));
                *pending.borrow_mut()=selection;
                apply.lock().unwrap().pending=selection;
                ui.invoke_navigate_photo(id);
            }
        }
        if (2..=6).contains(&phase) {
            let n = (phase_started.elapsed().as_millis() / 32 % 40) as f32;
            ui.window()
                .set_size(slint::LogicalSize::new(640. + n * 18., 500. + n * 8.));
        }
    });
    Ok(Benchmark {
        failure: Some(failure),
        _timer: timer,
        producer,
        pump,
        photos: vec![],
    })
}
