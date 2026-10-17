//! Shared authenticated, bounded media transport. The writer retains its permit
//! and cancellation flag while the SDK finishes any blocking HTTP read.
#[derive(Clone, Copy)]
pub enum Error {
    Failed,
    TooLarge,
    Unavailable,
}
use std::{
    io::{self, Cursor, Seek, SeekFrom, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use whatsapp_rust::{
    Client,
    download::{ClientDownloadError, DownloadWriter, Downloadable},
};
// Held by the writer even if the async caller is cancelled while the SDK's
// blocking HTTP read is still unwinding. Reconnect cannot multiply downloads.
static DOWNLOAD: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
struct Bounded {
    bytes: Cursor<Vec<u8>>,
    cancelled: Arc<AtomicBool>,
    limit: usize,
    _permit: tokio::sync::SemaphorePermit<'static>,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(io::Error::other("media cancelled"));
        }
        if self.bytes.position().saturating_add(bytes.len() as u64) > self.limit as u64 {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "media size limit",
            ));
        }
        self.bytes.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Seek for Bounded {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.bytes.seek(pos)
    }
}
impl DownloadWriter for Bounded {
    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.bytes.get_mut().truncate(len as usize);
        Ok(())
    }
}

pub async fn download(
    client: &Client,
    media: &dyn Downloadable,
    limit: usize,
) -> Result<Vec<u8>, Error> {
    let permit = DOWNLOAD.acquire().await.map_err(|_| Error::Failed)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel = Cancel(cancelled.clone());
    let writer = Bounded {
        bytes: Cursor::new(Vec::new()),
        cancelled,
        _permit: permit,
        limit,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(45),
        client.download_to_writer(media, writer),
    )
    .await;
    match result {
        Ok(Ok(writer)) => Ok(writer.bytes.into_inner()),
        Ok(Err(
            ClientDownloadError::ReferenceRejected(_) | ClientDownloadError::Preparation(_),
        )) => Err(Error::Unavailable),
        Ok(Err(ClientDownloadError::WriterIo(_))) => Err(Error::TooLarge),
        _ => Err(Error::Failed),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn streaming_writer_bounds_unknown_lengths_cleans_retries_and_cancels() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = Cancel(cancelled.clone());
        let mut sink = Bounded {
            bytes: Cursor::new(Vec::new()),
            cancelled,
            _permit: DOWNLOAD.acquire().await.unwrap(),
            limit: crate::photos::MAX_BYTES,
        };
        sink.write_all(b"synthetic encrypted-media output").unwrap();
        sink.truncate(0).unwrap();
        sink.rewind().unwrap();
        sink.write_all(b"fresh retry").unwrap();
        assert_eq!(sink.bytes.get_ref(), b"fresh retry");
        sink.seek(SeekFrom::Start(crate::photos::MAX_BYTES as u64 - 1))
            .unwrap();
        assert_eq!(
            sink.write(b"xx").unwrap_err().kind(),
            io::ErrorKind::FileTooLarge
        );
        assert_eq!(sink.bytes.get_ref().len(), 11);
        drop(cancel);
        sink.rewind().unwrap();
        assert_eq!(
            sink.write_all(b"late result").unwrap_err().kind(),
            io::ErrorKind::Other
        );
        sink.truncate(0).unwrap();
        assert!(sink.bytes.get_ref().is_empty());
    }
}
