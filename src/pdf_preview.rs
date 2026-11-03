//! Render page one in a short-lived helper: Quartz on macOS, Poppler on Linux.
//! PDF bytes and pixels travel over pipes, never through plaintext cache files.
use crate::document_previews::MAX_BYTES;
#[cfg(any(not(target_os = "macos"), test))]
use crate::document_previews::{HEIGHT, WIDTH};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

pub const MAX_PDF_BYTES: usize = 16 * 1024 * 1024;
/// Longest edge of an outgoing document's thumbnail, as WhatsApp Web sends it.
pub const PAGE_EDGE: u32 = 480;
const TIMEOUT: Duration = Duration::from_secs(8);
/// Sending waits for this render, so it gets WhatsApp Web's shorter budget.
const PAGE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Invalid,
    Failed,
    Timeout,
}

/// Page one of an outgoing PDF, and its page count when the helper reports one.
pub struct Page {
    pub png: Vec<u8>,
    pub pages: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    /// The received-card strip, WIDTH x HEIGHT from the top of the page.
    Strip,
    /// The whole page, longest edge PAGE_EDGE.
    Page,
    #[cfg(not(target_os = "macos"))]
    Info,
}

fn command(job: Job) -> Command {
    #[cfg(target_os = "linux")]
    {
        // Only system libraries/fonts are visible. No account files, network,
        // display, session bus or writable host filesystem enter the sandbox.
        let mut command = Command::new("/usr/bin/bwrap");
        command.args([
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--ro-bind",
            "/usr",
            "/usr",
            // Mirror the host's library layout: /lib64 is not /usr/lib on
            // every distribution (Ubuntu keeps its loader in /usr/lib64).
            "--ro-bind",
            "/lib",
            "/lib",
            "--ro-bind-try",
            "/lib64",
            "/lib64",
            "--ro-bind",
            "/etc/fonts",
            "/etc/fonts",
            "--ro-bind-try",
            "/var/cache/fontconfig",
            "/var/cache/fontconfig",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--size",
            "67108864",
            "--tmpfs",
            "/tmp",
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/bin",
            "--setenv",
            "HOME",
            "/tmp",
            "--setenv",
            "LANG",
            "C.UTF-8",
            "--chdir",
            "/tmp",
            "/usr/bin/prlimit",
            "--as=536870912",
            "--cpu=5",
            // Fontconfig needs a small disposable cache. This can only land
            // in the private /tmp tmpfs, never on the host filesystem.
            "--fsize=33554432",
            "--core=0",
            "--",
            if job == Job::Info {
                "/usr/bin/pdfinfo"
            } else {
                "/usr/bin/pdftoppm"
            },
        ]);
        command
    }
    #[cfg(target_os = "macos")]
    {
        // The helper exits before creating UI, preferences or account services.
        let mut command = Command::new(std::env::current_exe().unwrap_or_default());
        #[cfg(not(test))]
        {
            command.arg("--heya-pdf-worker");
            if job == Job::Page {
                command.arg("page");
            }
        }
        #[cfg(test)]
        command
            .args([
                "--exact",
                "pdf_preview::tests::native_pdf_child",
                "--ignored",
                "--nocapture",
            ])
            .env(
                "HEYA_TEST_PDF_WORKER",
                if job == Job::Page { "page" } else { "1" },
            );
        command
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Command::new(if job == Job::Info {
            "pdfinfo"
        } else {
            "pdftoppm"
        })
    }
}

/// Cancellation drops the child with kill-on-drop, including its sandbox.
/// Reading output and feeding input concurrently avoids full-pipe deadlocks.
pub async fn render(bytes: Vec<u8>) -> Result<Vec<u8>, Error> {
    if bytes.len() > MAX_PDF_BYTES || !bytes.starts_with(b"%PDF-") {
        return Err(Error::Invalid);
    }
    let command = command(Job::Strip);
    #[cfg(not(target_os = "macos"))]
    let mut command = command;
    #[cfg(not(target_os = "macos"))]
    command.args([
        "-f",
        "1",
        "-l",
        "1",
        "-singlefile",
        "-scale-dimension-before-rotation",
        "-scale-to-x",
        &WIDTH.to_string(),
        "-scale-to-y",
        "-1",
        "-x",
        "0",
        "-y",
        "0",
        "-W",
        &WIDTH.to_string(),
        "-H",
        &HEIGHT.to_string(),
        "-cropbox",
        "-png",
        "-q",
        "-",
    ]);
    run(command, bytes, TIMEOUT).await
}

/// The whole first page, scaled so its longest edge is PAGE_EDGE. The page
/// count is best effort: a PDF that renders but hides its count still gets an
/// image.
pub async fn first_page(bytes: Vec<u8>) -> Result<Page, Error> {
    if bytes.len() > MAX_PDF_BYTES || !bytes.starts_with(b"%PDF-") {
        return Err(Error::Invalid);
    }
    #[cfg(target_os = "macos")]
    {
        // The worker prefixes its PNG with the page count, big-endian.
        let output = run(command(Job::Page), bytes, PAGE_TIMEOUT).await?;
        let (count, png) = output.split_first_chunk::<4>().ok_or(Error::Failed)?;
        Ok(Page {
            png: png.to_vec(),
            pages: Some(u32::from_be_bytes(*count)).filter(|n| *n > 0),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut render = command(Job::Page);
        render.args([
            "-f",
            "1",
            "-l",
            "1",
            "-singlefile",
            "-scale-to",
            &PAGE_EDGE.to_string(),
            "-cropbox",
            "-png",
            "-q",
            "-",
        ]);
        let mut info = command(Job::Info);
        info.arg("-");
        let (png, info) = tokio::join!(
            run(render, bytes.clone(), PAGE_TIMEOUT),
            run(info, bytes, PAGE_TIMEOUT)
        );
        Ok(Page {
            png: png?,
            pages: info.ok().as_deref().and_then(page_count),
        })
    }
}

#[cfg(not(target_os = "macos"))]
fn page_count(info: &[u8]) -> Option<u32> {
    // Metadata lines may carry any encoding; only the count line matters.
    String::from_utf8_lossy(info)
        .lines()
        .find_map(|line| line.strip_prefix("Pages:"))?
        .trim()
        .parse()
        .ok()
        .filter(|n| *n > 0)
}

async fn run(mut command: Command, bytes: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, Error> {
    command.stdin(Stdio::piped()).kill_on_drop(true);
    #[cfg(all(target_os = "macos", test))]
    command.stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(not(all(target_os = "macos", test)))]
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn().map_err(|_| Error::Unavailable)?;
    let mut input = child.stdin.take().ok_or(Error::Failed)?;
    // The libtest child writes PNG to stderr so its stdout banners cannot
    // enter the image stream. Production helpers use stdout exclusively.
    #[cfg(all(target_os = "macos", test))]
    let mut output = child
        .stderr
        .take()
        .ok_or(Error::Failed)?
        .take((MAX_BYTES + 1) as u64);
    #[cfg(not(all(target_os = "macos", test)))]
    let mut output = child
        .stdout
        .take()
        .ok_or(Error::Failed)?
        .take((MAX_BYTES + 1) as u64);
    let result = tokio::time::timeout(timeout, async {
        let write = async move {
            input.write_all(&bytes).await?;
            input.shutdown().await?;
            drop(input); // Poppler's stdin reader needs EOF, not just a flush.
            Ok::<_, std::io::Error>(())
        };
        let read = async {
            let mut image = Vec::new();
            output.read_to_end(&mut image).await?;
            Ok::<_, std::io::Error>(image)
        };
        let (_, image) = tokio::try_join!(write, read).map_err(|_| Error::Failed)?;
        if image.is_empty() || image.len() > MAX_BYTES {
            return Err(Error::Failed);
        }
        if !child.wait().await.map_err(|_| Error::Failed)?.success() {
            return Err(Error::Failed);
        }
        Ok(image)
    })
    .await
    .unwrap_or(Err(Error::Timeout));
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}

#[cfg(test)]
#[path = "pdf_preview_tests.rs"]
mod tests;
