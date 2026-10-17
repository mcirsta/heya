//! Read-only diagnostics: never print keys, database contents or provider errors.
fn main() {
    for user in ["session-v1", "archive-v1"] {
        match keyring::Entry::new("org.heya.Heya", user).and_then(|e| e.get_secret()) {
            Ok(bytes) => {
                let secret = zeroize::Zeroizing::new(bytes);
                let text = std::str::from_utf8(&secret).ok();
                println!(
                    "{user}: present; bytes={}; utf8={}; replacement_characters={}; versioned_text={}",
                    secret.len(),
                    text.is_some(),
                    text.is_some_and(|s| s.contains('\u{fffd}')),
                    secret.starts_with(b"heya-key-v1:")
                );
            }
            Err(keyring::Error::NoEntry) => println!("{user}: absent"),
            Err(_) => println!("{user}: provider error (details suppressed)"),
        }
    }
    if let Some(project) = directories::ProjectDirs::from("org", "heya", "Heya") {
        for file in [
            "session.db",
            "session.db-wal",
            "archive.db",
            "archive.db-wal",
        ] {
            match std::fs::metadata(project.data_local_dir().join("default").join(file)) {
                Ok(metadata) => println!("{file}: present; bytes={}", metadata.len()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => println!("{file}: absent"),
                Err(_) => println!("{file}: metadata unavailable"),
            }
        }
    }
}
