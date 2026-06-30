//! Local-time helpers for message timestamps.
//!
//! React uses `new Date(seconds * 1000)` and `date-fns` `format`, which always
//! render in the user's local timezone. Store code must not derive clock labels
//! from `ts % 86_400` or UTC `DateTime::from_timestamp` — those ignore locale.

use chrono::{Datelike, Local, TimeZone, Timelike};

pub fn local_datetime(ts: i64) -> Option<chrono::DateTime<Local>> {
    if ts == 0 {
        return None;
    }
    Local.timestamp_opt(ts, 0).single()
}

/// Stable local-calendar key used to group messages and detect day changes.
pub fn local_day_key(ts: i64) -> String {
    local_datetime(ts)
        .map(|dt| format!("{}-{:02}-{:02}", dt.year(), dt.month(), dt.day()))
        .unwrap_or_default()
}

pub fn format_local_time_hhmm(ts: i64) -> String {
    local_datetime(ts)
        .map(|dt| format!("{:02}:{:02}", dt.hour(), dt.minute()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_day_key_matches_local_calendar_date() {
        let ts = 1_609_459_200 + 48_300;
        let dt = local_datetime(ts).expect("valid timestamp");
        assert_eq!(
            local_day_key(ts),
            format!("{}-{:02}-{:02}", dt.year(), dt.month(), dt.day())
        );
    }

    #[test]
    fn format_local_time_hhmm_matches_local_clock() {
        let ts = 1_609_459_200 + 48_300;
        let dt = local_datetime(ts).expect("valid timestamp");
        assert_eq!(
            format_local_time_hhmm(ts),
            format!("{:02}:{:02}", dt.hour(), dt.minute())
        );
    }

    #[test]
    fn zero_timestamp_returns_empty() {
        assert!(local_day_key(0).is_empty());
        assert!(format_local_time_hhmm(0).is_empty());
    }
}
