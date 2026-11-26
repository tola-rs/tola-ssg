//! Calendar dates and timestamps shared by metadata schemas and publication formats.

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Timelike};
use typst::foundations::{Datetime, Value};

/// A date or timestamp that violates the supported calendar rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DateError(&'static str);

/// Parses a native date or strict `YYYY-MM-DD` string (year 1..=9999), rejecting times.
pub fn parse_date(val: &Value) -> Result<Datetime, DateError> {
    match val {
        Value::Datetime(Datetime::Date(date)) if (1..=9999).contains(&date.year()) => {
            Ok(Datetime::Date(*date))
        }
        Value::Str(text) => {
            let date = calendar_date(text.as_str())?;
            Datetime::from_ymd(date.year(), date.month() as u8, date.day() as u8).ok_or(DateError(
                "expected a calendar date with a year from 1 through 9999",
            ))
        }
        _ => Err(DateError(
            "expected a calendar date without a time, such as 2026-09-01",
        )),
    }
}

/// Formats a calendar date accepted by [`parse_date`] as `YYYY-MM-DD`.
pub fn format_calendar_date(val: &Datetime) -> String {
    let (year, month, day) = calendar_parts(val);
    format!("{year:04}-{month:02}-{day:02}")
}

fn calendar_parts(val: &Datetime) -> (i32, u8, u8) {
    match val {
        Datetime::Date(date) => (date.year(), date.month() as u8, date.day()),
        _ => unreachable!("parse_date accepts only calendar dates"),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PartialDate {
    Year(i32),
    Month { year: i32, month: u8 },
    Day { year: i32, month: u8, day: u8 },
}

pub(super) fn parse_partial_date(val: &Value) -> Result<PartialDate, DateError> {
    let invalid = DateError("expected a valid calendar date in YYYY, YYYY-MM, or YYYY-MM-DD form");
    if let Value::Str(text) = val {
        match text.len() {
            4 => {
                let date = calendar_date(&format!("{text}-01-01")).map_err(|_| invalid)?;
                return Ok(PartialDate::Year(date.year()));
            }
            7 => {
                let date = calendar_date(&format!("{text}-01")).map_err(|_| invalid)?;
                return Ok(PartialDate::Month {
                    year: date.year(),
                    month: date.month() as u8,
                });
            }
            _ => {}
        }
    }
    let (year, month, day) = calendar_parts(&parse_date(val).map_err(|_| invalid)?);
    Ok(PartialDate::Day { year, month, day })
}

/// A timestamp with its original RFC 3339 spelling and exact fractional second.
#[derive(Debug)]
pub struct Timestamp {
    rfc3339: String,
    whole: DateTime<FixedOffset>,
    subsecond: String,
}

impl Timestamp {
    /// Parses a strict RFC 3339 string or a native datetime with a complete calendar date.
    pub fn parse(val: &Value) -> Result<Self, DateError> {
        match val {
            Value::Str(text) => Self::from_text(text.as_str()),
            Value::Datetime(dt) => {
                let (date, time) = match dt {
                    Datetime::Date(date) => (*date, None),
                    Datetime::Datetime(dt) => (dt.date(), Some(dt.time())),
                    Datetime::Time(_) => {
                        return Err(DateError("a timestamp needs a year, month, and day"));
                    }
                };
                let (h, m, s, ns) = time
                    .map(|t| (t.hour(), t.minute(), t.second(), t.nanosecond()))
                    .unwrap_or((0, 0, 0, 0));
                let mut text = format!(
                    "{:04}-{:02}-{:02}T{h:02}:{m:02}:{s:02}",
                    date.year(),
                    date.month() as u8,
                    date.day(),
                );
                if ns != 0 {
                    text.push('.');
                    text.push_str(format!("{ns:09}").trim_end_matches('0'));
                }
                text.push('Z');
                Self::from_text(&text)
            }
            _ => Err(DateError(
                "expected a calendar datetime or an RFC 3339 timestamp with timezone",
            )),
        }
    }

    fn from_text(text: &str) -> Result<Self, DateError> {
        let invalid =
            DateError("expected an RFC 3339 timestamp with timezone, such as 2026-09-01T12:00:00Z");
        let bytes = text.as_bytes();
        if bytes.len() < 20
            || bytes[..19].iter().enumerate().any(|(i, b)| match i {
                4 | 7 => *b != b'-',
                10 => !matches!(b, b'T' | b't'),
                13 | 16 => *b != b':',
                _ => !b.is_ascii_digit(),
            })
        {
            return Err(invalid);
        }
        if decimal(&bytes[..4]) == 0 {
            return Err(DateError("the calendar year must be from 1 through 9999"));
        }
        if decimal(&bytes[17..19]) > 59 {
            return Err(DateError(
                "timestamp seconds must be from 00 through 59; leap seconds are unsupported",
            ));
        }
        let suffix = &text[19..];
        let (fraction, zone) = if let Some(frac) = suffix.strip_prefix('.') {
            let len = frac.bytes().take_while(u8::is_ascii_digit).count();
            if len == 0 {
                return Err(invalid);
            }
            (&frac[..len], &frac[len..])
        } else {
            ("", suffix)
        };
        if !valid_zone(zone) {
            return Err(invalid);
        }
        // Validate the whole timestamp separately so Chrono preserves arbitrary fractional digits.
        let whole = DateTime::parse_from_rfc3339(&format!("{}{zone}", &text[..19]))
            .map_err(|_| DateError("expected a valid calendar date, clock time, and timezone"))?;
        Ok(Self {
            rfc3339: text.to_owned(),
            whole,
            subsecond: fraction.trim_end_matches('0').to_owned(),
        })
    }

    /// Original RFC 3339 text, or UTC text generated from a native datetime.
    pub fn rfc3339(&self) -> &str {
        &self.rfc3339
    }

    /// Whole seconds since the Unix epoch in UTC.
    pub fn unix_seconds(&self) -> i64 {
        self.whole.timestamp()
    }

    /// Fractional second digits without a decimal point or trailing zeros.
    pub fn subsecond(&self) -> &str {
        &self.subsecond
    }

    /// Converts to a fixed-offset datetime, truncating beyond nanoseconds.
    pub fn to_fixed(&self) -> DateTime<FixedOffset> {
        let len = self.subsecond.len().min(9);
        let ns = decimal(&self.subsecond.as_bytes()[..len]) * 10_u32.pow(9 - len as u32);
        self.whole
            .with_nanosecond(ns)
            .expect("fractional second is below one billion nanoseconds")
    }
}

fn calendar_date(text: &str) -> Result<NaiveDate, DateError> {
    let invalid = DateError("expected a valid calendar date in YYYY-MM-DD form");
    let bytes = text.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, b)| !matches!(i, 4 | 7) && !b.is_ascii_digit())
    {
        return Err(invalid);
    }
    let year = decimal(&bytes[..4]);
    if year == 0 {
        return Err(DateError("the calendar year must be from 1 through 9999"));
    }
    NaiveDate::from_ymd_opt(year as i32, decimal(&bytes[5..7]), decimal(&bytes[8..10]))
        .ok_or(invalid)
}

fn valid_zone(zone: &str) -> bool {
    match zone.as_bytes() {
        [b'Z' | b'z'] => true,
        [sign, ht, hu, b':', mt, mu] => {
            matches!(sign, b'+' | b'-')
                && [ht, hu, mt, mu].iter().all(|b| b.is_ascii_digit())
                && decimal(&zone.as_bytes()[1..3]) <= 23
                && decimal(&zone.as_bytes()[4..6]) <= 59
        }
        _ => false,
    }
}

fn decimal(digits: &[u8]) -> u32 {
    digits
        .iter()
        .fold(0, |acc, d| acc * 10 + u32::from(d - b'0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(val: &str) -> Value {
        Value::Str(val.into())
    }

    #[test]
    fn parse_date_valid() {
        for (src, y, m, d) in [
            ("0001-01-01", 1, 1, 1),
            ("2000-02-29", 2000, 2, 29),
            ("2024-02-29", 2024, 2, 29),
            ("9999-12-31", 9999, 12, 31),
        ] {
            let expected = Datetime::from_ymd(y, m, d).unwrap();
            assert_eq!(parse_date(&text(src)).unwrap(), expected);
            assert_eq!(parse_date(&Value::Datetime(expected)).unwrap(), expected);
        }
    }

    #[test]
    fn parse_partial_date_valid() {
        for (src, expected) in [
            ("0001", PartialDate::Year(1)),
            ("9999", PartialDate::Year(9999)),
            (
                "2024-02",
                PartialDate::Month {
                    year: 2024,
                    month: 2,
                },
            ),
            (
                "2000-02-29",
                PartialDate::Day {
                    year: 2000,
                    month: 2,
                    day: 29,
                },
            ),
        ] {
            assert_eq!(parse_partial_date(&text(src)).unwrap(), expected);
        }
        let date = Datetime::from_ymd(2024, 2, 29).unwrap();
        assert_eq!(
            parse_partial_date(&Value::Datetime(date)).unwrap(),
            PartialDate::Day {
                year: 2024,
                month: 2,
                day: 29,
            },
        );
    }

    #[test]
    fn parse_partial_date_invalid() {
        for src in [
            "",
            "0000",
            "10000",
            "2026-00",
            "2026-13",
            "1900-02-29",
            "2026-02-30",
            "2026-04-31",
            "2026-1",
            "2026-01-1",
            "2026 ",
            " 2026",
            "2026-11-01\n",
            "2026-11-01T08:30:00Z",
            "２０２６",
            "2026-é",
        ] {
            assert!(parse_partial_date(&text(src)).is_err(), "{src}");
        }
        for dt in [
            Datetime::from_ymd(0, 1, 1).unwrap(),
            Datetime::from_hms(8, 30, 0).unwrap(),
            Datetime::from_ymd_hms(2026, 11, 1, 8, 30, 0).unwrap(),
        ] {
            assert!(parse_partial_date(&Value::Datetime(dt)).is_err());
        }
    }

    #[test]
    fn timestamp_offsets() {
        let src = "1970-01-01t00:30:00+01:00";
        let ts = Timestamp::parse(&text(src)).unwrap();
        assert_eq!(ts.rfc3339(), src);
        assert_eq!(ts.unix_seconds(), -1800);
        assert_eq!(ts.to_fixed().offset().local_minus_utc(), 3600);
        for (src, secs) in [
            ("1970-01-01T00:00:00+23:59", -86_340),
            ("1970-01-01T00:00:00-23:59", 86_340),
            ("1970-01-01t00:00:00z", 0),
            ("1970-01-01T00:00:00-00:00", 0),
        ] {
            let ts = Timestamp::parse(&text(src)).unwrap();
            assert_eq!(ts.rfc3339(), src);
            assert_eq!(ts.unix_seconds(), secs);
        }
    }
}
