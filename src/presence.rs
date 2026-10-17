//! Transient header presence. No SDK/UI types and no persisted activity history.
use chrono::{DateTime, Local, NaiveDateTime};
use tokio::sync::watch;

#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub chat: i32,
    pub jid: String,
    pub addresses: Vec<String>,
}
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Update {
    pub chat: i32,
    pub online: bool,
    pub last_seen: Option<i64>,
}
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub chat: i32,
    pub online: bool,
    pub label: String,
}
#[derive(Clone)]
pub struct Channels {
    pub selected: watch::Receiver<i32>,
    pub snapshots: watch::Sender<Snapshot>,
}
impl Update {
    pub fn snapshot(&self, now: DateTime<Local>) -> Snapshot {
        let label = if self.online {
            "online".into()
        } else {
            self.last_seen
                .filter(|stamp| *stamp <= now.timestamp().saturating_add(60))
                .and_then(crate::message_time::local)
                .map(|seen| last_seen(seen.naive_local(), now.naive_local()))
                .unwrap_or_default()
        };
        Snapshot {
            chat: self.chat,
            online: self.online,
            label,
        }
    }
}
fn last_seen(seen: NaiveDateTime, now: NaiveDateTime) -> String {
    let day = match now.date().signed_duration_since(seen.date()).num_days() {
        0 => "today".into(),
        1 => "yesterday".into(),
        _ => seen.format("%-d %b %Y").to_string(),
    };
    format!("last seen {day} at {}", seen.format("%H:%M"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn last_seen_uses_local_calendar_days_and_rejects_unknown_or_invalid_timestamps() {
        let now = "2026-01-01T12:00:00".parse::<NaiveDateTime>().unwrap();
        for (seen, expected) in [
            ("2026-01-01T10:25:00", "last seen today at 10:25"),
            ("2025-12-31T23:58:00", "last seen yesterday at 23:58"),
            ("2025-12-30T08:03:00", "last seen 30 Dec 2025 at 08:03"),
        ] {
            assert_eq!(last_seen(seen.parse().unwrap(), now), expected);
        }
        for stamp in [None, Some(0), Some(-1), Some(i64::MAX)] {
            assert!(
                Update {
                    last_seen: stamp,
                    ..Default::default()
                }
                .snapshot(Local::now())
                .label
                .is_empty()
            );
        }
        assert_eq!(
            Update {
                online: true,
                ..Default::default()
            }
            .snapshot(Local::now())
            .label,
            "online"
        );
    }
}
