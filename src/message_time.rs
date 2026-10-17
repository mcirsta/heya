//! Calendar display rules shared by live messages and synthetic conversations.
use chrono::{DateTime, Local, NaiveDate};

/// Convert once while building the archive view, never during row layout.
pub fn local(stamp: i64) -> Option<DateTime<Local>> {
    (stamp > 0)
        .then(|| DateTime::from_timestamp(stamp, 0))
        .flatten()
        .map(|time| time.with_timezone(&Local))
}

pub fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    match today.signed_duration_since(date).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => date.format("%A").to_string(),
        _ => date.format("%A, %-d %B %Y").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Days, FixedOffset, TimeZone};

    #[test]
    fn labels_follow_calendar_days_including_year_and_clock_changes() {
        let today = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        for (days, expected) in [
            (0, "Today"),
            (1, "Yesterday"),
            (2, "Wednesday"),
            (6, "Saturday"),
            (7, "Friday, 26 December 2025"),
        ] {
            assert_eq!(day_label(today - Days::new(days), today), expected);
        }
        assert_eq!(
            day_label(today + Days::new(1), today),
            "Saturday, 3 January 2026"
        );
        // A local calendar day can be 23 or 25 hours. Never divide elapsed
        // Unix seconds by 86400 to decide whether to insert a separator.
        for (first, second, hours) in [
            ("2026-03-29T00:00:00+02:00", "2026-03-30T00:00:00+03:00", 23),
            ("2026-10-25T00:00:00+03:00", "2026-10-26T00:00:00+02:00", 25),
        ] {
            let a = DateTime::parse_from_rfc3339(first).unwrap();
            let b = DateTime::parse_from_rfc3339(second).unwrap();
            assert_eq!((b - a).num_hours(), hours);
            assert_eq!(day_label(a.date_naive(), b.date_naive()), "Yesterday");
        }
        let utc = DateTime::parse_from_rfc3339("2026-10-14T22:30:00Z").unwrap();
        let east = FixedOffset::east_opt(3 * 3600)
            .unwrap()
            .from_utc_datetime(&utc.naive_utc());
        assert_ne!(utc.date_naive(), east.date_naive());
        assert!(local(0).is_none());
        assert!(local(-1).is_none());
        assert!(local(i64::MAX).is_none());
    }
}
