//! Timestamps: the RFC 3339 UTC form every timestamp in this API takes.

/// The current time as whole unix seconds.
pub(crate) fn unix_now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// The current time as an RFC 3339 UTC string.
pub(crate) fn rfc3339_now() -> String {
    rfc3339(unix_now_seconds())
}

/// Formats unix seconds as an RFC 3339 UTC string.
///
/// The date part uses the civil-from-days conversion: shift the epoch to
/// 1 March 0000 so leap days land at the end of the year, then count
/// 400-year, 100-year, and 4-year cycles.
pub(crate) fn rfc3339(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds_of_day = unix_seconds % 86_400;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z",
        hour = seconds_of_day / 3600,
        minute = (seconds_of_day % 3600) / 60,
        second = seconds_of_day % 60,
    )
}

#[cfg(test)]
mod tests {
    use super::rfc3339;

    #[test]
    fn unix_seconds_become_rfc_3339_utc_strings() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_000_000_000), "2001-09-09T01:46:40Z");
        // 29 February 2024: a leap day in a leap century rule year.
        assert_eq!(rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_735_689_599), "2024-12-31T23:59:59Z");
    }
}
