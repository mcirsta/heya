//! Private streaming staging files. Only authenticated, complete output is published.
#[cfg(test)]
#[path = "downloads_file_tests.rs"]
mod tests;
use crate::downloads::{Control, Error, MAX_BYTES, Progress};
use std::{
    fs::{self, File, Metadata},
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Instant,
};
use tempfile::TempPath;
use tokio::sync::watch;

pub struct Sink {
    file: File,
    pub bytes: u64,
    control: Control,
    progress: watch::Sender<Progress>,
    last: Instant,
}
pub struct Partial {
    path: TempPath,
    destination: PathBuf,
    previous: Option<Metadata>,
}
impl Partial {
    pub fn create(
        destination: PathBuf,
        control: Control,
        progress: watch::Sender<Progress>,
    ) -> Result<(Self, Sink), Error> {
        let parent = destination
            .parent()
            .ok_or(Error::Destination)?
            .canonicalize()
            .map_err(|_| Error::Destination)?;
        let name = destination.file_name().ok_or(Error::Destination)?;
        let destination = parent.join(name);
        let previous = metadata(&destination)?;
        if previous.as_ref().is_some_and(|m| !m.is_file()) {
            return Err(Error::Destination);
        }
        let temp = tempfile::Builder::new()
            .prefix(".heya-download-")
            .tempfile_in(parent)
            .map_err(|_| Error::Storage)?;
        let (file, path) = temp.into_parts();
        Ok((
            Self {
                path,
                destination,
                previous,
            },
            Sink {
                file,
                bytes: 0,
                control,
                progress,
                last: Instant::now(),
            },
        ))
    }
    pub fn publish(self, mut sink: Sink) -> Result<(), Error> {
        sink.check().map_err(|_| Error::Cancelled)?;
        sink.file.flush().map_err(|_| Error::Storage)?;
        sink.file.sync_all().map_err(|_| Error::Storage)?;
        if !same(
            self.previous.as_ref(),
            metadata(&self.destination)?.as_ref(),
        ) {
            return Err(Error::DestinationChanged);
        }
        sink.check().map_err(|_| Error::Cancelled)?;
        // Dropping the future before this stage unlinks the partial immediately,
        // even if an SDK blocking HTTP reader still holds its file descriptor.
        drop(sink.file);
        if self.previous.is_some() {
            self.path
                .persist(&self.destination)
                .map_err(|_| Error::Storage)?;
        } else {
            self.path
                .persist_noclobber(&self.destination)
                .map_err(|_| Error::DestinationChanged)?;
        }
        Ok(())
    }
}
fn metadata(path: &Path) -> Result<Option<Metadata>, Error> {
    match fs::symlink_metadata(path) {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Error::Destination),
    }
}
fn same(a: Option<&Metadata>, b: Option<&Metadata>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if a.dev() != b.dev() || a.ino() != b.ino() {
                    return false;
                }
            }
            a.is_file()
                && b.is_file()
                && a.len() == b.len()
                && a.modified()
                    .ok()
                    .zip(b.modified().ok())
                    .is_some_and(|(a, b)| a == b)
        }
        _ => false,
    }
}
impl Sink {
    pub fn control_cancelled(&self) -> bool {
        self.control.is_cancelled()
    }
    fn check(&self) -> io::Result<()> {
        if self.control.is_cancelled() {
            Err(io::Error::other("download cancelled"))
        } else {
            Ok(())
        }
    }
    pub fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)?;
        self.bytes = len;
        self.progress.send_replace(Progress { bytes: len });
        Ok(())
    }
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.check()?;
        if self.bytes.saturating_add(bytes.len() as u64) > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "download limit",
            ));
        }
        let written = self.file.write(bytes)?;
        self.bytes += written as u64;
        if self.last.elapsed() >= std::time::Duration::from_millis(100) {
            self.last = Instant::now();
            self.progress.send_replace(Progress { bytes: self.bytes });
        }
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl Seek for Sink {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.bytes = self.file.seek(position)?;
        Ok(self.bytes)
    }
}
