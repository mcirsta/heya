//! Render page one's strip in a short-lived helper: Quartz on macOS, Poppler on Linux.
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
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Invalid,
    Failed,
    Timeout,
}

fn command() -> Command {
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
            "--symlink",
            "usr/lib",
            "/lib",
            "--symlink",
            "usr/lib",
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
            "/usr/bin/pdftoppm",
        ]);
        command
    }
    #[cfg(target_os = "macos")]
    {
        // The helper exits before creating UI, preferences or account services.
        let mut command = Command::new(std::env::current_exe().unwrap_or_default());
        #[cfg(not(test))]
        command.arg("--heya-pdf-worker");
        #[cfg(test)]
        command
            .args([
                "--exact",
                "pdf_preview::tests::native_pdf_child",
                "--ignored",
                "--nocapture",
            ])
            .env("HEYA_TEST_PDF_WORKER", "1");
        command
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Command::new("pdftoppm")
    }
}

/// Cancellation drops the child with kill-on-drop, including its sandbox.
/// Reading output and feeding input concurrently avoids full-pipe deadlocks.
pub async fn render(bytes: Vec<u8>) -> Result<Vec<u8>, Error> {
    if bytes.len() > MAX_PDF_BYTES || !bytes.starts_with(b"%PDF-") {
        return Err(Error::Invalid);
    }
    let command = command();
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
