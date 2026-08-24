//! Epoch-ms ↔ ISO-8601 / UTC-date conversions for OKF serialization.

use chrono::{DateTime, NaiveDate, SecondsFormat, TimeZone, Utc};

pub fn iso_from_ms(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(|| {
            Utc.timestamp_millis_opt(0)
                .single()
                .expect("epoch is valid")
        })
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn ms_from_iso(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

pub fn utc_date_from_ms(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

pub fn ms_from_utc_date(date: &str) -> Option<i64> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|ndt| Utc.from_utc_datetime(&ndt).timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_fixture_timestamp() {
        assert_eq!(iso_from_ms(1782907200000), "2026-07-01T12:00:00.000Z");
        assert_eq!(ms_from_iso("2026-07-01T12:00:00.000Z"), Some(1782907200000));
    }

    #[test]
    fn date_helpers() {
        assert_eq!(utc_date_from_ms(1783209600000), "2026-07-05");
        assert_eq!(ms_from_utc_date("2026-07-05"), Some(1783209600000));
    }
}
