//! Feed date formatting for native Typst datetimes.

use typst::foundations::Datetime;

pub(super) fn date_only(value: &Datetime) -> Option<String> {
    let (year, month, day) = (value.year()?, value.month()?, value.day()?);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// Format a complete Typst date with an optional complete UTC time.
pub(super) fn format_datetime(value: &Datetime) -> anyhow::Result<String> {
    let Some(date) = date_only(value) else {
        anyhow::bail!("the datetime needs a year, month, and day");
    };

    match (value.hour(), value.minute(), value.second()) {
        (None, None, None) => Ok(date),
        (Some(hour), Some(minute), Some(second)) => {
            Ok(format!("{date}T{hour:02}:{minute:02}:{second:02}Z"))
        }
        _ => anyhow::bail!(
            "a datetime time needs hour, minute, and second, or must be omitted entirely"
        ),
    }
}

/// Format a Typst datetime as UTC RFC 3339.
///
/// Date-only values use midnight. Partial dates or times are rejected.
pub(super) fn to_rfc3339(value: &Datetime) -> Option<String> {
    let date = date_only(value)?;
    let (hour, minute, second) = time(value)?;
    Some(format!("{date}T{hour:02}:{minute:02}:{second:02}Z"))
}

fn time(value: &Datetime) -> Option<(u8, u8, u8)> {
    match (value.hour(), value.minute(), value.second()) {
        (None, None, None) => Some((0, 0, 0)),
        (Some(hour), Some(minute), Some(second)) => Some((hour, minute, second)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_dates_format_partial_ones_fail() {
        let date_only = Datetime::from_ymd(2024, 1, 15).unwrap();
        let complete = Datetime::from_ymd_hms(2026, 5, 12, 8, 9, 10).unwrap();
        let time_only = Datetime::from_hms(8, 9, 10).unwrap();

        assert_eq!(to_rfc3339(&date_only).unwrap(), "2024-01-15T00:00:00Z");
        assert_eq!(to_rfc3339(&complete).unwrap(), "2026-05-12T08:09:10Z");
        assert_eq!(to_rfc3339(&time_only), None);

        assert_eq!(format_datetime(&date_only).unwrap(), "2024-01-15");
        assert_eq!(format_datetime(&complete).unwrap(), "2026-05-12T08:09:10Z");
        assert!(format_datetime(&time_only).is_err());
    }
}
