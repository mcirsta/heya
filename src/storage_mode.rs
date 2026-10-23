//! A single manifest selects both databases. Conversion never rewrites the
//! active files; publishing the verified replacement is one atomic rename.
use super::*;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Encrypted,
    Unencrypted,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Slot {
    #[default]
    Legacy,
    A,
    B,
}

#[derive(Clone, Copy, Default)]
pub struct Status {
    pub mode: Mode,
    pub configured: bool,
    slot: Slot,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    mode: Mode,
    slot: Slot,
}

impl Status {
    pub(super) fn directory(&self, root: &Path) -> PathBuf {
        match self.slot {
            Slot::Legacy => root.to_path_buf(),
            Slot::A => root.join("data-a"),
            Slot::B => root.join("data-b"),
        }
    }
}

pub fn status() -> Result<Status, Failure> {
    read(&profile_directory()?)
}

pub(super) fn read(root: &Path) -> Result<Status, Failure> {
    let file = match File::open(root.join("storage.json")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Status {
                configured: databases_exist(root)?,
                ..Default::default()
            });
        }
        Err(_) => return Err(Failure::DataDirectory),
    };
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::DataDirectory)?;
    if bytes.len() > 4096 {
        return Err(Failure::DataDirectory);
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| Failure::DataDirectory)?;
    if manifest.version != 1 {
        return Err(Failure::DataDirectory);
    }
    Ok(Status {
        mode: manifest.mode,
        slot: manifest.slot,
        configured: true,
    })
}

fn publish(root: &Path, state: Status) -> Result<(), Failure> {
    let mut file = tempfile::NamedTempFile::new_in(root).map_err(|_| Failure::DataDirectory)?;
    serde_json::to_writer(
        &mut file,
        &Manifest {
            version: 1,
            mode: state.mode,
            slot: state.slot,
        },
    )
    .map_err(|_| Failure::DataDirectory)?;
    file.write_all(b"\n").map_err(|_| Failure::DataDirectory)?;
    file.as_file()
        .sync_all()
        .map_err(|_| Failure::DataDirectory)?;
    file.persist(root.join("storage.json"))
        .map_err(|_| Failure::DataDirectory)?;
    Ok(())
}

fn sync_directory(root: &Path) -> Result<(), Failure> {
    #[cfg(unix)]
    File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|_| Failure::DataDirectory)?;
    #[cfg(not(unix))]
    let _ = root;
    Ok(())
}

/// First-run choice: no key-store access and no account database creation.
pub fn configure(mode: Mode) -> Result<Status, Failure> {
    configure_at(&profile_directory()?, mode)
}
fn configure_at(root: &Path, mode: Mode) -> Result<Status, Failure> {
    let _lock = lock_directory(root)?;
    let state = read(root)?;
    if state.configured {
        return Err(Failure::StorageConversion);
    }
    let state = Status {
        mode,
        configured: true,
        slot: Slot::Legacy,
    };
    publish(root, state)?;
    sync_directory(root)?;
    Ok(state)
}

pub fn change_mode(mode: Mode) -> Result<Status, Failure> {
    change_at(&profile_directory()?, mode, &desktop_vault())
}

fn remove_file(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
fn remove_directory(path: &Path) -> std::io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Also runs after an interrupted conversion, before opening account data.
pub(super) fn cleanup(root: &Path, state: &Status) -> Result<(), Failure> {
    for slot in [Slot::A, Slot::B] {
        if state.slot != slot {
            remove_directory(&Status { slot, ..*state }.directory(root))
                .map_err(|_| Failure::StorageCleanup)?;
        }
    }
    if state.slot != Slot::Legacy {
        for name in ["session.db", "archive.db"] {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                remove_file(&root.join(format!("{name}{suffix}")))
                    .map_err(|_| Failure::StorageCleanup)?;
            }
        }
    }
    Ok(())
}

struct Staging(PathBuf, bool);
impl Drop for Staging {
    fn drop(&mut self) {
        if !self.1 {
            let _ = remove_directory(&self.0);
        }
    }
}

pub(super) fn change_at(
    root: &Path,
    target: Mode,
    vault: &impl SecretVault,
) -> Result<Status, Failure> {
    let _lock = lock_directory(root)?;
    let current = read(root)?;
    cleanup(root, &current)?;
    if current.mode == target && current.configured {
        return Ok(current);
    }
    let source = current.directory(root);
    if !current.configured || !databases_exist(&source)? {
        let state = Status {
            mode: target,
            configured: true,
            ..current
        };
        publish(root, state)?;
        sync_directory(root)?;
        return Ok(state);
    }
    // A plaintext profile may create a key; an existing encrypted profile must
    // recover its original key. Never replace a missing key over encrypted data.
    let master = master_key(
        current.mode == Mode::Encrypted && databases_exist(&source)?,
        vault,
    )?;
    let next = Status {
        mode: target,
        configured: true,
        slot: if current.slot == Slot::A {
            Slot::B
        } else {
            Slot::A
        },
    };
    let destination = next.directory(root);
    fs::create_dir(&destination).map_err(|_| Failure::DataDirectory)?;
    let mut staging = Staging(destination.clone(), false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o700))
            .map_err(|_| Failure::DataDirectory)?;
    }
    for (name, context) in [
        ("session.db", b"session-v1".as_slice()),
        ("archive.db", b"archive-v1".as_slice()),
    ] {
        if !source
            .join(name)
            .try_exists()
            .map_err(|_| Failure::DataDirectory)?
        {
            for suffix in ["-wal", "-shm", "-journal"] {
                if source
                    .join(format!("{name}{suffix}"))
                    .try_exists()
                    .map_err(|_| Failure::DataDirectory)?
                {
                    return Err(Failure::StorageConversion);
                }
            }
            continue;
        }
        let key = database_key(&master, context)?;
        export(
            &source.join(name),
            &destination.join(name),
            if current.mode == Mode::Encrypted {
                &key
            } else {
                ""
            },
            if target == Mode::Encrypted { &key } else { "" },
        )?;
    }
    sync_directory(&destination)?;
    publish(root, next)?;
    staging.1 = true; // The manifest now owns this directory, even if fsync fails.
    sync_directory(root)?;
    cleanup(root, &next)?;
    sync_directory(root)?;
    Ok(next)
}

fn export(
    source: &Path,
    destination: &Path,
    source_key: &str,
    target_key: &str,
) -> Result<(), Failure> {
    use rusqlite::{Connection, OpenFlags, params};
    let operation = || -> rusqlite::Result<()> {
        let db = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        if !source_key.is_empty() {
            db.pragma_update(None, "key", source_key)?;
        }
        db.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })?;
        let (busy, _, _): (i64, i64, i64) =
            db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        if busy != 0 {
            return Err(rusqlite::Error::ExecuteReturnedResults);
        }
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let application: i64 = db.pragma_query_value(None, "application_id", |r| r.get(0))?;
        private_file(destination)
            .map_err(|_| rusqlite::Error::InvalidPath(destination.to_path_buf()))?;
        db.execute(
            "ATTACH DATABASE ?1 AS converted KEY ?2",
            params![destination.to_str(), target_key],
        )?;
        db.query_row("SELECT sqlcipher_export('converted')", [], |_| Ok(()))?;
        db.pragma_update(Some("converted"), "user_version", version)?;
        db.pragma_update(Some("converted"), "application_id", application)?;
        db.execute_batch("DETACH DATABASE converted")?;
        drop(db);
        let verified = Connection::open_with_flags(destination, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        if !target_key.is_empty() {
            verified.pragma_update(None, "key", target_key)?;
        }
        let integrity: String = verified.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    };
    operation().map_err(|_| Failure::StorageConversion)?;
    private_file(destination)
        .map_err(|_| Failure::StorageConversion)?
        .sync_all()
        .map_err(|_| Failure::StorageConversion)?;
    Ok(())
}

#[cfg(test)]
#[path = "storage_mode_tests.rs"]
mod tests;
