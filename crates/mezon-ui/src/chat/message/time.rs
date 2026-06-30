//! Message timestamp formatting — parity with React `convertUnixSecondsToTimeString`
//! and `convertDateStringI18n` in `@mezon/utils`.

use chrono::{Datelike, Duration, Local};
use mezon_store::message_time::{format_local_time_hhmm, local_datetime};

const MONTH_KEYS: [&str; 12] = [
    "common.timeFormat.months.jan",
    "common.timeFormat.months.feb",
    "common.timeFormat.months.mar",
    "common.timeFormat.months.apr",
    "common.timeFormat.months.may",
    "common.timeFormat.months.jun",
    "common.timeFormat.months.jul",
    "common.timeFormat.months.aug",
    "common.timeFormat.months.sep",
    "common.timeFormat.months.oct",
    "common.timeFormat.months.nov",
    "common.timeFormat.months.dec",
];

const WEEKDAY_KEYS: [&str; 7] = [
    "common.timeFormat.daysOfWeek.sun",
    "common.timeFormat.daysOfWeek.mon",
    "common.timeFormat.daysOfWeek.tue",
    "common.timeFormat.daysOfWeek.wed",
    "common.timeFormat.daysOfWeek.thu",
    "common.timeFormat.daysOfWeek.fri",
    "common.timeFormat.daysOfWeek.sat",
];

/// Clock label beside the sender name (React `MessageHead` / `MessageLineSystem`).
pub fn format_message_time(ts: i64, locale: &str) -> String {
    let Some(dt) = local_datetime(ts) else {
        return String::new();
    };

    let now = Local::now();
    let today = now.date_naive();
    let yesterday = today - Duration::days(1);
    let msg_date = dt.date_naive();
    let time = format_local_time_hhmm(ts);

    if msg_date == today {
        time
    } else if msg_date == yesterday {
        format!("{} {}", mezon_i18n::t(locale, "common.yesterdayAt"), time)
    } else {
        format!(
            "{:02}/{:02}/{}, {}",
            dt.day(),
            dt.month(),
            dt.year(),
            time
        )
    }
}

/// Date separator between message groups (React `MessageDateDivider`).
pub fn format_date_divider(ts: i64, locale: &str) -> String {
    let Some(dt) = local_datetime(ts) else {
        return String::new();
    };

    let month = mezon_i18n::t(locale, MONTH_KEYS[dt.month0() as usize]);
    let formatted = format!("{:02} {} {}", dt.day(), month, dt.year());

    let today = Local::now().date_naive();
    if dt.date_naive() == today {
        format!("{}, {}", mezon_i18n::t(locale, "common.today"), formatted)
    } else {
        let weekday = mezon_i18n::t(locale, WEEKDAY_KEYS[dt.weekday().num_days_from_sunday() as usize]);
        format!("{weekday}, {formatted}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn message_time_today_is_hhmm_only() {
        let now = Local::now();
        let ts = now.timestamp();
        let label = format_message_time(ts, "en");
        assert_eq!(
            label,
            format!("{:02}:{:02}", now.hour(), now.minute())
        );
    }

    #[test]
    fn date_divider_today_prefixes_common_today() {
        let now = Local::now();
        let label = format_date_divider(now.timestamp(), "en");
        assert!(label.starts_with("Today,"));
    }
}
