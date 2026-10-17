use super::*;

fn staging(path: PathBuf) -> (Partial, Sink, Control) {
    let control = Control::default();
    let (partial, sink) =
        Partial::create(path, control.clone(), watch::channel(Progress::default()).0).unwrap();
    (partial, sink, control)
}
fn clean(path: &Path) {
    assert!(!fs::read_dir(path).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".heya-download-")
    }));
}

#[test]
fn files_stream_past_the_preview_limit_and_publish_only_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("Synthetic 你好.bin");
    let (partial, mut sink, _) = staging(destination.clone());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&partial.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let chunk = [0xa5; 64 * 1024];
    for _ in 0..512 {
        sink.write_all(&chunk).unwrap();
    }
    assert_eq!(sink.bytes, 32 * 1024 * 1024);
    assert!(!destination.exists());
    partial.publish(sink).unwrap();
    assert_eq!(fs::metadata(&destination).unwrap().len(), 32 * 1024 * 1024);
    clean(dir.path());
}
#[test]
fn cancelled_and_failed_saves_preserve_existing_files_and_unlink_partials() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("existing.txt");
    fs::write(&destination, b"original user content").unwrap();
    let (partial, mut sink, control) = staging(destination.clone());
    sink.write_all(b"unverified new content").unwrap();
    control.cancel();
    assert_eq!(
        sink.write_all(b"late write").unwrap_err().kind(),
        io::ErrorKind::Other
    );
    drop(partial);
    clean(dir.path()); // The HTTP reader can still own the sink.
    assert_eq!(fs::read(&destination).unwrap(), b"original user content");
    drop(sink);
    let (partial, mut sink, _) = staging(destination.clone());
    sink.write_all(b"verified output").unwrap();
    fs::write(&destination, b"changed while downloading").unwrap();
    assert_eq!(partial.publish(sink), Err(Error::DestinationChanged));
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"changed while downloading"
    );
    clean(dir.path());
    let fresh = dir.path().join("appeared.txt");
    let (partial, mut sink, _) = staging(fresh.clone());
    sink.write_all(b"ours").unwrap();
    fs::write(&fresh, b"somebody else's file").unwrap();
    assert_eq!(partial.publish(sink), Err(Error::DestinationChanged));
    assert_eq!(fs::read(fresh).unwrap(), b"somebody else's file");
    clean(dir.path());
}
#[test]
fn retries_truncate_stale_bytes_and_enforce_limits_without_large_allocations() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("retry.bin");
    let (partial, mut sink, _) = staging(destination.clone());
    sink.write_all(b"a longer failed transfer").unwrap();
    sink.truncate(0).unwrap();
    sink.rewind().unwrap();
    sink.write_all(b"fresh").unwrap();
    sink.seek(SeekFrom::Start(MAX_BYTES - 1)).unwrap();
    assert_eq!(
        sink.write_all(b"xx").unwrap_err().kind(),
        io::ErrorKind::FileTooLarge
    );
    sink.seek(SeekFrom::Start(5)).unwrap();
    partial.publish(sink).unwrap();
    assert_eq!(fs::read(destination).unwrap(), b"fresh");
    clean(dir.path());
}
#[cfg(unix)]
#[test]
fn directories_and_symlinks_are_not_silently_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original");
    fs::write(&original, b"untouched").unwrap();
    let link = dir.path().join("symlink");
    std::os::unix::fs::symlink(&original, &link).unwrap();
    for path in [
        dir.path().to_path_buf(),
        link,
        dir.path().join("missing/file"),
    ] {
        assert!(
            Partial::create(
                path,
                Control::default(),
                watch::channel(Progress::default()).0
            )
            .is_err()
        );
    }
    assert_eq!(fs::read(original).unwrap(), b"untouched");
    clean(dir.path());
}
#[cfg(target_os = "linux")]
#[test]
fn disk_write_failure_leaves_the_destination_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("existing");
    fs::write(&destination, b"original").unwrap();
    let (partial, mut sink, _) = staging(destination.clone());
    sink.file = File::options().write(true).open("/dev/full").unwrap();
    assert!(sink.write_all(b"synthetic bytes").is_err());
    drop(partial);
    drop(sink);
    assert_eq!(fs::read(destination).unwrap(), b"original");
    clean(dir.path());
}
