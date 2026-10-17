use super::*;

#[tokio::test]
async fn actual_pdf_renders_first_page_at_card_resolution_without_plaintext_files() {
    for (name, pdf) in [
        (
            "portrait",
            include_bytes!("../tests/fixtures/preview-portrait.pdf").as_slice(),
        ),
        (
            "landscape",
            include_bytes!("../tests/fixtures/preview-landscape.pdf").as_slice(),
        ),
        (
            "rotated",
            include_bytes!("../tests/fixtures/preview-rotated.pdf").as_slice(),
        ),
    ] {
        let started = std::time::Instant::now();
        let png = render(pdf.to_vec())
            .await
            .expect("Poppler/bubblewrap must be installed for PDF integration tests");
        let pixels = crate::document_previews::decode(&png).unwrap();
        assert_eq!((pixels.width, pixels.height), (WIDTH, HEIGHT), "{name}");
        assert!(crate::document_previews::sharp_enough(&png));
        assert!(
            pixels
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] < 40 && p[1] < 40 && p[2] < 40),
            "text must be rendered"
        );
        if name != "rotated" {
            assert!(
                !pixels
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[0] > 200 && p[1] < 30 && p[2] < 30),
                "lower page/second page must not be rasterized into the strip"
            );
        }
        if name == "portrait" {
            // The page's one-point alternating black/white rules are one pixel
            // at the target width. A stretched 51px thumbnail cannot pass this.
            let row = &pixels.rgba[(230 * WIDTH * 4) as usize..(231 * WIDTH * 4) as usize];
            let transitions = row
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0] < 128)
                .collect::<Vec<_>>()
                .windows(2)
                .filter(|p| p[0] != p[1])
                .count();
            assert!(transitions > 400, "fine vector detail lost: {transitions}");
            verify_archive_and_presenter(&png).await;
        }
        eprintln!(
            "Synthetic PDF {name}: {}x{}, {} bytes, {} ms",
            pixels.width,
            pixels.height,
            png.len(),
            started.elapsed().as_millis()
        );
        if let Some(dir) = std::env::var_os("HEYA_PDF_TEST_OUTPUT") {
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png).unwrap();
        }
    }
    assert_eq!(render(b"not a PDF".to_vec()).await, Err(Error::Invalid));
    assert_eq!(
        render(vec![0; MAX_PDF_BYTES + 1]).await,
        Err(Error::Invalid)
    );
    assert!(
        render(b"%PDF-1.7 corrupt synthetic file".to_vec())
            .await
            .is_err()
    );
}

async fn verify_archive_and_presenter(png: &[u8]) {
    use crate::archive::{
        Batch,
        tests::{fixture, record},
    };
    use tokio::sync::watch;
    let (_temp, db) = fixture();
    let mut message = record("synthetic-rendered-document");
    message.content.document = Some(crate::message_content::Document {
        file_name: "Synthetic.pdf".into(),
        mime_type: "application/pdf".into(),
        size: None,
        pages: Some(2),
        has_thumbnail: true,
    });
    db.ingest(Batch {
        messages: vec![message],
        ..Default::default()
    })
    .await
    .unwrap();
    let id = db
        .blocking(|c| Ok(c.query_row("SELECT id FROM messages", [], |r| r.get::<_, i32>(0))?))
        .unwrap();
    let request = db
        .document_preview_request(vec![id])
        .await
        .unwrap()
        .unwrap();
    assert!(
        db.save_document_preview(&request, Some(png.to_vec()))
            .await
            .unwrap()
    );
    let (_archives, archives) = watch::channel(Some(db));
    let (demand, requests) = watch::channel(vec![id]);
    let (updates, mut images) = watch::channel(crate::message_thumbnails::Snapshot::default());
    let worker = tokio::spawn(crate::message_thumbnails::present(
        archives, requests, updates,
    ));
    tokio::time::timeout(Duration::from_secs(5), images.changed())
        .await
        .unwrap()
        .unwrap();
    let expected = crate::document_previews::decode(png).unwrap();
    assert_eq!(
        images.borrow()[&id].rgba,
        expected.rgba,
        "encrypted cache/presentation must retain rendered detail"
    );
    drop(demand);
    worker.await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn renderer_timeout_output_limits_and_cancellation_reap_the_process() {
    // Controlled helpers exercise the same process/pipe lifecycle as Poppler.
    let mut stalled = Command::new("/usr/bin/sleep");
    stalled.arg("30");
    let started = std::time::Instant::now();
    assert_eq!(
        run(stalled, Vec::new(), Duration::from_millis(80)).await,
        Err(Error::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    let mut flood = Command::new("/usr/bin/head");
    flood.args(["-c", "2097152", "/dev/zero"]);
    assert_eq!(run(flood, Vec::new(), TIMEOUT).await, Err(Error::Failed));

    let temp = tempfile::tempdir().unwrap();
    let pidfile = temp.path().join("pid");
    let mut sleeper = Command::new("/usr/bin/python3");
    sleeper
        .args([
            "-c",
            "import os,sys,time; open(sys.argv[1],'w').write(str(os.getpid())); time.sleep(30)",
        ])
        .arg(&pidfile);
    let job = tokio::spawn(run(sleeper, Vec::new(), TIMEOUT));
    tokio::time::timeout(Duration::from_secs(2), async {
        while !pidfile.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pid = std::fs::read_to_string(pidfile).unwrap();
    job.abort();
    assert!(job.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), async {
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled renderer must not linger or remain a zombie");
}
