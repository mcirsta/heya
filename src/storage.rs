//! Private session files and OS credential storage; all entry points are blocking.
use crate::connection::Failure;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

#[path = "storage_mode.rs"]
mod mode;
pub use mode::{Mode, Status, change_mode, configure, status};

pub trait SecretVault {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, Failure>;
    fn write(&self, key: &[u8]) -> Result<(), Failure>;
}

struct DesktopVault<'a> {
    service: &'a str,
    name: &'a str,
}
impl DesktopVault<'_> {
    fn entry(&self) -> Result<keyring::Entry, Failure> {
        keyring::Entry::new(self.service, self.name).map_err(keyring_failure)
    }
}

impl SecretVault for DesktopVault<'_> {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
        match self.entry()?.get_secret() {
            Ok(key) => decode_key(Zeroizing::new(key)).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_failure(error)),
        }
    }

    fn write(&self, key: &[u8]) -> Result<(), Failure> {
        // keyring 3.6's Secret Service backend declares even set_secret() data
        // as text/plain. KDE rejects non-UTF-8 bytes (and can leave an empty
        // item behind). Store an explicit text encoding without reducing entropy.
        // Use the byte API on both sides: set_password uses UTF-16 on Windows.
        self.entry()?
            .set_secret(encode_key(key)?.as_bytes())
            .map_err(keyring_failure)
    }
}

// Retry transport/service readiness, never an unlock cancellation or bad key.
fn keyring_failure(error: keyring::Error) -> Failure {
    #[cfg(target_os = "linux")]
    if let keyring::Error::PlatformFailure(cause) = &error {
        use dbus_secret_service::Error;
        let unavailable = match cause.downcast_ref::<Error>() {
            Some(Error::Unavailable) => true,
            Some(Error::Dbus(error)) => matches!(
                error.name(),
                Some(
                    "org.freedesktop.DBus.Error.ServiceUnknown"
                        | "org.freedesktop.DBus.Error.NameHasNoOwner"
                        | "org.freedesktop.DBus.Error.NoServer"
                        | "org.freedesktop.DBus.Error.FileNotFound"
                        | "org.freedesktop.DBus.Error.Disconnected"
                        | "org.freedesktop.DBus.Error.NoReply"
                        | "org.freedesktop.DBus.Error.Timeout"
                        | "org.freedesktop.DBus.Error.TimedOut"
                        | "org.freedesktop.DBus.Error.Spawn.ServiceNotFound"
                        | "org.freedesktop.DBus.Error.Spawn.ChildExited"
                )
            ),
            _ => false,
        };
        if unavailable {
            return Failure::KeyringUnavailable;
        }
    }
    #[cfg(target_os = "macos")]
    if let keyring::Error::NoStorageAccess(cause) | keyring::Error::PlatformFailure(cause) = &error
        && cause
            .downcast_ref::<security_framework::base::Error>()
            .is_some_and(|error| matches!(error.code(), -25291 | -25308))
    // errSecNotAvailable / errSecInteractionNotAllowed; cancellation stays terminal.
    {
        return Failure::KeyringUnavailable;
    }
    let _ = error;
    Failure::Keyring
}

const KEY_PREFIX: &str = "heya-key-v1:";

fn encode_key(key: &[u8]) -> Result<Zeroizing<String>, Failure> {
    if key.len() != 32 {
        return Err(Failure::InvalidKey);
    }
    use std::fmt::Write;
    let mut text = Zeroizing::new(String::from(KEY_PREFIX));
    for byte in key {
        write!(text, "{byte:02x}").map_err(|_| Failure::InvalidKey)?;
    }
    Ok(text)
}

fn decode_key(stored: Zeroizing<Vec<u8>>) -> Result<Zeroizing<Vec<u8>>, Failure> {
    if stored.starts_with(KEY_PREFIX.as_bytes()) {
        let hex = &stored[KEY_PREFIX.len()..];
        if hex.len() != 64 || !hex.iter().all(u8::is_ascii_hexdigit) {
            return Err(Failure::InvalidKey);
        }
        let mut key = Zeroizing::new(Vec::with_capacity(32));
        for pair in hex.as_chunks::<2>().0 {
            let byte = |c: u8| {
                if c.is_ascii_digit() {
                    c - b'0'
                } else {
                    c.to_ascii_lowercase() - b'a' + 10
                }
            };
            key.push(byte(pair[0]) * 16 + byte(pair[1]));
        }
        Ok(key)
    } else {
        // Keep existing raw 32-byte keys readable. Validation also preserves
        // the distinction between an empty failed bootstrap and a malformed key.
        Ok(stored)
    }
}

pub struct SessionFiles {
    pub path: PathBuf,
    pub key_sql: Zeroizing<String>,
    pub archive_key_sql: Zeroizing<String>,
    // Retained by the protocol pool's connection hook, including detached work.
    _lock: File,
}

// Pre-release storage reset: the former profile used two independent keyring
// items. Keep it untouched; this profile uses one master key on every platform.
const PROFILE_DIRECTORY: &str = "default-v2";

pub fn prepare() -> Result<Arc<SessionFiles>, Failure> {
    prepare_at(&profile_directory()?, &desktop_vault())
}

fn desktop_vault() -> DesktopVault<'static> {
    DesktopVault {
        service: "org.heya.Heya",
        name: "master-v1",
    }
}
fn profile_directory() -> Result<PathBuf, Failure> {
    directories::ProjectDirs::from("org", "heya", "Heya")
        .map(|p| p.data_local_dir().join(PROFILE_DIRECTORY))
        .ok_or(Failure::DataDirectory)
}

/// Startup probes only file presence. A first launch does not create keys or a
/// database until the user chooses Connect.
pub fn has_session() -> Result<bool, Failure> {
    let root = profile_directory()?;
    let state = mode::read(&root)?;
    databases_exist(&state.directory(&root))
}

#[cfg(test)]
pub(crate) fn change_mode_at(
    directory: &Path,
    target: Mode,
    vault: &impl SecretVault,
) -> Result<Status, Failure> {
    mode::change_at(directory, target, vault)
}

pub(crate) fn private_file(path: &Path) -> Result<File, Failure> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| Failure::DataDirectory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| Failure::DataDirectory)?;
    }
    Ok(file)
}

pub(crate) fn prepare_at(
    directory: &Path,
    vault: &impl SecretVault,
) -> Result<Arc<SessionFiles>, Failure> {
    let lock = lock_directory(directory)?;
    let state = mode::read(directory)?;
    mode::cleanup(directory, &state)?;
    let directory = state.directory(directory);
    let path = directory.join("session.db");
    let (key_sql, archive_key_sql) = if state.mode == Mode::Encrypted {
        let master = load_master_key(&directory, vault)?;
        (
            derive_database_key(&master, b"session-v1")?,
            derive_database_key(&master, b"archive-v1")?,
        )
    } else {
        (Zeroizing::new(String::new()), Zeroizing::new(String::new()))
    };
    private_file(&path)?;
    Ok(Arc::new(SessionFiles {
        path,
        key_sql,
        archive_key_sql,
        _lock: lock,
    }))
}

fn lock_directory(directory: &Path) -> Result<File, Failure> {
    fs::create_dir_all(directory).map_err(|_| Failure::DataDirectory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| Failure::DataDirectory)?;
    }
    let lock = private_file(&directory.join("session.lock"))?;
    lock.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => Failure::AlreadyRunning,
        std::fs::TryLockError::Error(_) => Failure::DataDirectory,
    })?;
    Ok(lock)
}

fn databases_exist(directory: &Path) -> Result<bool, Failure> {
    let mut exists = false;
    for database in ["session.db", "archive.db"] {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            exists |= directory
                .join(format!("{database}{suffix}"))
                .try_exists()
                .map_err(|_| Failure::DataDirectory)?;
        }
    }
    Ok(exists)
}

fn load_master_key(
    directory: &Path,
    vault: &impl SecretVault,
) -> Result<Zeroizing<Vec<u8>>, Failure> {
    master_key(databases_exist(directory)?, vault)
}
fn master_key(exists: bool, vault: &impl SecretVault) -> Result<Zeroizing<Vec<u8>>, Failure> {
    match vault.read()? {
        Some(key) if key.len() == 32 => Ok(key),
        Some(key) if !key.is_empty() || exists => Err(Failure::InvalidKey),
        None if exists => Err(Failure::MissingKey),
        // Only an empty entry with no database or journal can be the failed
        // first write described above. Never replace a key over stored data.
        Some(_) | None => {
            let mut key = Zeroizing::new(vec![0; 32]);
            getrandom::fill(&mut key).map_err(|_| Failure::Keyring)?;
            vault.write(&key)?;
            // A successful write without a readable durable secret is not enough.
            let stored = vault.read()?.ok_or(Failure::Keyring)?;
            if *stored != *key {
                return Err(Failure::Keyring);
            }
            Ok(key)
        }
    }
}

fn derive_database_key(master: &[u8], context: &[u8]) -> Result<Zeroizing<String>, Failure> {
    Ok(Zeroizing::new(format!(
        "PRAGMA key = \"{}\";",
        database_key(master, context)?.as_str()
    )))
}
fn database_key(master: &[u8], context: &[u8]) -> Result<Zeroizing<String>, Failure> {
    let mut key = Zeroizing::new([0u8; 32]);
    hkdf::Hkdf::<sha2::Sha256>::new(Some(b"org.heya.Heya database keys v1"), master)
        .expand(context, key.as_mut())
        .map_err(|_| Failure::InvalidKey)?;
    let mut key_sql = Zeroizing::new(String::from("x'"));
    use std::fmt::Write;
    for byte in key.iter() {
        write!(key_sql, "{byte:02x}").map_err(|_| Failure::InvalidKey)?;
    }
    key_sql.push('\'');
    Ok(key_sql)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct MemoryVault(pub RefCell<Option<Vec<u8>>>);
    impl SecretVault for MemoryVault {
        fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
            Ok(self.0.borrow().clone().map(Zeroizing::new))
        }
        fn write(&self, key: &[u8]) -> Result<(), Failure> {
            *self.0.borrow_mut() = Some(key.to_vec());
            Ok(())
        }
    }

    #[test]
    fn key_text_encoding_is_lossless_and_legacy_binary_keys_still_work() {
        let bytes: Vec<u8> = (0..32)
            .map(|n| if n % 2 == 0 { 0 } else { 128 + n })
            .collect();
        let encoded = encode_key(&bytes).unwrap();
        assert_eq!(encoded.len(), KEY_PREFIX.len() + 64);
        assert!(encoded.is_ascii());
        let decoded = decode_key(Zeroizing::new(encoded.as_bytes().to_vec())).unwrap();
        assert!(*decoded == bytes);
        assert!(*decode_key(Zeroizing::new(bytes.clone())).unwrap() == bytes);
        for malformed in [
            format!("{KEY_PREFIX}{}", "x".repeat(64)),
            format!("{KEY_PREFIX}1234"),
        ] {
            assert!(matches!(
                decode_key(Zeroizing::new(malformed.into_bytes())),
                Err(Failure::InvalidKey)
            ));
        }
    }

    #[test]
    fn failed_empty_bootstrap_recovers_only_when_no_database_or_journal_exists() {
        let temp = tempfile::tempdir().unwrap();
        let vault = MemoryVault(RefCell::new(Some(Vec::new())));
        assert!(load_master_key(temp.path(), &vault).is_ok());
        assert_eq!(vault.0.borrow().as_ref().unwrap().len(), 32);
        for name in [
            "session.db",
            "session.db-wal",
            "session.db-shm",
            "archive.db",
            "archive.db-wal",
            "archive.db-shm",
        ] {
            *vault.0.borrow_mut() = Some(Vec::new());
            let existing = temp.path().join(name);
            std::fs::write(&existing, b"synthetic stored data").unwrap();
            assert!(matches!(
                load_master_key(temp.path(), &vault),
                Err(Failure::InvalidKey)
            ));
            assert!(vault.0.borrow().as_ref().unwrap().is_empty());
            *vault.0.borrow_mut() = None;
            assert!(matches!(
                load_master_key(temp.path(), &vault),
                Err(Failure::MissingKey)
            ));
            assert!(vault.0.borrow().is_none());
            assert_eq!(std::fs::read(&existing).unwrap(), b"synthetic stored data");
            std::fs::remove_file(existing).unwrap();
        }
    }

    #[test]
    fn one_vault_read_opens_both_databases_with_distinct_durable_keys() {
        use std::cell::Cell;
        struct CountingVault {
            reads: Cell<usize>,
        }
        impl SecretVault for CountingVault {
            fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
                self.reads.set(self.reads.get() + 1);
                Ok(Some(Zeroizing::new(vec![42; 32])))
            }
            fn write(&self, _: &[u8]) -> Result<(), Failure> {
                panic!("must reuse the stored master key")
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let vault = CountingVault {
            reads: Cell::new(0),
        };
        let files = prepare_at(temp.path(), &vault).unwrap();
        assert_ne!(*files.key_sql, *files.archive_key_sql);
        let session_key = files.key_sql.clone();
        let archive_key = files.archive_key_sql.clone();
        for (name, key) in [("session.db", &session_key), ("archive.db", &archive_key)] {
            let db = rusqlite::Connection::open(temp.path().join(name)).unwrap();
            db.execute_batch(key).unwrap();
            db.execute_batch("CREATE TABLE probe(value INTEGER); INSERT INTO probe VALUES(42);")
                .unwrap();
        }
        assert_eq!(
            vault.reads.get(),
            1,
            "opening the archive must not read the vault again"
        );
        drop(files);
        let files = prepare_at(temp.path(), &vault).unwrap();
        assert_eq!(vault.reads.get(), 2, "one vault read per startup");
        assert_eq!(*files.key_sql, *session_key);
        assert_eq!(*files.archive_key_sql, *archive_key);
        for (name, right, wrong) in [
            ("session.db", &files.key_sql, &files.archive_key_sql),
            ("archive.db", &files.archive_key_sql, &files.key_sql),
        ] {
            let path = temp.path().join(name);
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch(right).unwrap();
            assert_eq!(
                db.query_row::<i64, _, _>("SELECT value FROM probe", [], |row| row.get(0))
                    .unwrap(),
                42
            );
            drop(db);
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch(wrong).unwrap();
            assert!(
                db.query_row::<i64, _, _>("SELECT value FROM probe", [], |row| row.get(0))
                    .is_err()
            );
        }
    }

    #[test]
    #[ignore = "requires an unlocked desktop keyring; uses a disposable test entry and database"]
    fn desktop_keyring_encrypts_and_reopens_a_disposable_database() {
        let name = format!(
            "probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let service = "org.heya.Heya.integration-tests";
        struct Cleanup(keyring::Entry);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.delete_credential();
            }
        }
        let cleanup = Cleanup(keyring::Entry::new(service, &name).unwrap());
        let vault = DesktopVault {
            service,
            name: &name,
        };
        // Simulate the exact empty first-write residue returned by ksecretd.
        vault
            .entry()
            .unwrap()
            .set_password("")
            .expect("could not create disposable empty entry");
        let temp = tempfile::tempdir().unwrap();
        let files = prepare_at(temp.path(), &vault).unwrap();
        let path = files.path.clone();
        let stored = Zeroizing::new(
            vault
                .entry()
                .unwrap()
                .get_secret()
                .expect("could not read disposable entry"),
        );
        assert!(stored.starts_with(KEY_PREFIX.as_bytes()));
        assert_eq!(stored.len(), KEY_PREFIX.len() + 64);
        let db = rusqlite::Connection::open(&path).unwrap();
        assert!(db.execute_batch(&files.key_sql).is_ok());
        db.execute_batch("CREATE TABLE probe(value INTEGER); INSERT INTO probe VALUES(42);")
            .unwrap();
        drop(db);
        drop(files);
        // A fresh keyring handle/session and a fresh SQLCipher connection must
        // recover the persisted bytes without relying on a fake vault/cache.
        let reopened = DesktopVault {
            service,
            name: &name,
        };
        let files = prepare_at(temp.path(), &reopened).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        assert!(db.execute_batch(&files.key_sql).is_ok());
        assert_eq!(
            db.query_row::<i64, _, _>("SELECT value FROM probe", [], |r| r.get(0))
                .unwrap(),
            42
        );
        drop(db);
        drop(files);
        cleanup
            .0
            .delete_credential()
            .expect("could not remove disposable test entry");
    }

    #[test]
    fn key_is_reused_and_missing_keys_never_replace_existing_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let vault = MemoryVault::default();
        let files = prepare_at(temp.path(), &vault).unwrap();
        let key = vault.0.borrow().clone();
        assert_eq!(key.as_ref().unwrap().len(), 32);
        assert!(matches!(
            prepare_at(temp.path(), &vault),
            Err(Failure::AlreadyRunning)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&files.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(files);
        drop(prepare_at(temp.path(), &vault).unwrap());
        assert_eq!(*vault.0.borrow(), key);
        *vault.0.borrow_mut() = None;
        assert!(matches!(
            prepare_at(temp.path(), &vault),
            Err(Failure::MissingKey)
        ));
        assert!(vault.0.borrow().is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unavailable_key_store_retries_but_refused_prompts_do_not() {
        use dbus_secret_service::Error;
        assert_eq!(
            keyring_failure(keyring::Error::PlatformFailure(Box::new(
                Error::Unavailable
            ))),
            Failure::KeyringUnavailable
        );
        for cause in [Error::Prompt, Error::Locked, Error::NoResult, Error::Parse] {
            assert_eq!(
                keyring_failure(keyring::Error::PlatformFailure(Box::new(cause))),
                Failure::Keyring
            );
        }
        assert_eq!(
            keyring_failure(keyring::Error::NoStorageAccess(Box::new(Error::Prompt))),
            Failure::Keyring
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unavailable_keychain_retries_but_cancelled_or_denied_prompts_do_not() {
        for (code, expected) in [
            (-25291, Failure::KeyringUnavailable),
            (-25308, Failure::KeyringUnavailable),
            (-128, Failure::Keyring),
            (-25293, Failure::Keyring),
        ] {
            let error = security_framework::base::Error::from_code(code);
            assert_eq!(
                keyring_failure(keyring::macos::decode_error(error)),
                expected
            );
        }
    }

    #[test]
    fn failed_keyring_and_malformed_keys_do_not_create_a_database() {
        struct Locked;
        impl SecretVault for Locked {
            fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
                Err(Failure::Keyring)
            }
            fn write(&self, _: &[u8]) -> Result<(), Failure> {
                panic!("must not replace a key")
            }
        }
        let temp = tempfile::tempdir().unwrap();
        assert!(matches!(
            prepare_at(temp.path(), &Locked),
            Err(Failure::Keyring)
        ));
        let vault = MemoryVault(RefCell::new(Some(vec![1; 4])));
        assert!(matches!(
            prepare_at(temp.path(), &vault),
            Err(Failure::InvalidKey)
        ));
        assert!(!temp.path().join("session.db").exists());
    }
}
