//! Times in UTC, written by hand so nothing needs a calendar crate. The service writes each machine's
//! events with one, and the app names a report's folder with one.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since 1970 now, or 0 if the clock is before it.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// Seconds since 1970 as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn utc(seconds: u64) -> String {
    let (year, month, day, hour, minute, second) = civil(seconds);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// Seconds since 1970 as `YYYYMMDD-HHMMSS`, for a file's name.
pub fn stamp(seconds: u64) -> String {
    let (year, month, day, hour, minute, second) = civil(seconds);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

fn civil(seconds: u64) -> (i64, u32, u32, u64, u64, u64) {
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let rest = seconds % 86_400;
    (year, month, day, rest / 3600, rest % 3600 / 60, rest % 60)
}

/// The year, month and day that is `days` after 1970-01-01, in the proleptic Gregorian calendar.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_written_in_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(951_782_400 + 86_399), "2000-02-29 23:59:59 UTC");
        assert_eq!(utc(1_767_225_600), "2026-01-01 00:00:00 UTC");
        assert_eq!(utc(1_709_251_199), "2024-02-29 23:59:59 UTC");
        assert_eq!(utc(4_102_444_799), "2099-12-31 23:59:59 UTC");
    }

    #[test]
    fn a_stamp_is_for_a_file_name() {
        assert_eq!(stamp(1_767_225_600 + 3661), "20260101-010101");
    }

    #[test]
    fn now_is_after_this_was_written() {
        assert!(now() > 1_767_225_600);
    }
}
