use anyhow::{Result, bail};

/// UTC datetime without timezone complexity
#[derive(Debug, Clone, Copy)]
pub struct DateTimeUtc {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[allow(dead_code)]
impl DateTimeUtc {
    pub const fn new(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Self {
        Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }

    pub const fn from_ymd(year: u16, month: u8, day: u8) -> Self {
        Self::new(year, month, day, 0, 0, 0)
    }

    /// Parse from "YYYY-MM-DD" or "YYYY-MM-DDTHH:MM:SSZ" format
    pub fn parse(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();

        // Minimum: "YYYY-MM-DD" (10 chars)
        if bytes.len() < 10 {
            return None;
        }

        // Parse date part
        let year = parse_u16(&bytes[0..4])?;
        if bytes[4] != b'-' {
            return None;
        }
        let month = parse_u8(&bytes[5..7])?;
        if bytes[7] != b'-' {
            return None;
        }
        let day = parse_u8(&bytes[8..10])?;

        // Check for time part (RFC3339)
        let (hour, minute, second) = if bytes.len() >= 20 && bytes[10] == b'T' && bytes[19] == b'Z'
        {
            if bytes[13] != b':' || bytes[16] != b':' {
                return None;
            }
            (
                parse_u8(&bytes[11..13])?,
                parse_u8(&bytes[14..16])?,
                parse_u8(&bytes[17..19])?,
            )
        } else if bytes.len() == 10 {
            (0, 0, 0)
        } else {
            return None;
        };

        let dt = Self::new(year, month, day, hour, minute, second);
        dt.validate().ok()?;
        Some(dt)
    }

    pub fn validate(&self) -> Result<()> {
        let Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        } = *self;

        if !(1..=12).contains(&month) {
            bail!("month is invalid: {month}");
        }

        let max_days = Self::days_in_month(year, month);
        if day == 0 || day > max_days {
            bail!("day is invalid: {day}");
        }
        if hour > 23 {
            bail!("hour is invalid: {hour}");
        }
        if minute > 59 {
            bail!("minute is invalid: {minute}");
        }
        if second > 59 {
            bail!("second is invalid: {second}");
        }

        Ok(())
    }

    #[inline]
    fn is_leap_year(year: u16) -> bool {
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    }

    #[inline]
    fn days_in_month(year: u16, month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if Self::is_leap_year(year) => 29,
            2 => 28,
            _ => 0,
        }
    }

    pub fn to_rfc2822(self) -> String {
        const WEEKDAYS: [&str; 7] = ["Sat", "Sun", "Mon", "Tue", "Wed", "Thu", "Fri"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];

        // Zeller's congruence for weekday calculation
        let weekday = self.weekday_index();

        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            WEEKDAYS[weekday],
            self.day,
            MONTHS[(self.month - 1) as usize],
            self.year,
            self.hour,
            self.minute,
            self.second
        )
    }

    #[inline]
    fn weekday_index(&self) -> usize {
        let (y, m) = if self.month < 3 {
            (self.year as i32 - 1, self.month as i32 + 12)
        } else {
            (self.year as i32, self.month as i32)
        };
        let d = self.day as i32;
        ((d + (13 * (m + 1)) / 5 + y + y / 4 - y / 100 + y / 400) % 7) as usize
    }
}

/// Parse 2-digit ASCII number
#[inline]
fn parse_u8(bytes: &[u8]) -> Option<u8> {
    if bytes.len() != 2 {
        return None;
    }
    let d1 = bytes[0].wrapping_sub(b'0');
    let d2 = bytes[1].wrapping_sub(b'0');
    if d1 > 9 || d2 > 9 {
        return None;
    }
    Some(d1 * 10 + d2)
}

/// Parse 4-digit ASCII number
#[inline]
fn parse_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    let mut result = 0u16;
    for &b in bytes {
        let d = b.wrapping_sub(b'0');
        if d > 9 {
            return None;
        }
        result = result * 10 + d as u16;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_datetime_utc_new() {
        let dt = DateTimeUtc::new(2024, 6, 15, 14, 30, 45);
        assert_eq!(dt.year, 2024);
        assert_eq!(dt.month, 6);
        assert_eq!(dt.day, 15);
        assert_eq!(dt.hour, 14);
        assert_eq!(dt.minute, 30);
        assert_eq!(dt.second, 45);
    }

    #[test]
    fn test_datetime_utc_from_ymd() {
        let dt = DateTimeUtc::from_ymd(2024, 12, 25);
        assert_eq!(dt.year, 2024);
        assert_eq!(dt.month, 12);
        assert_eq!(dt.day, 25);
        assert_eq!(dt.hour, 0);
        assert_eq!(dt.minute, 0);
        assert_eq!(dt.second, 0);
    }

    #[test]
    fn test_datetime_utc_validate_valid() {
        // Valid date
        assert!(DateTimeUtc::new(2024, 6, 15, 14, 30, 45).validate().is_ok());

        // Edge cases - start of day
        assert!(DateTimeUtc::new(2024, 1, 1, 0, 0, 0).validate().is_ok());

        // Edge cases - end of day
        assert!(
            DateTimeUtc::new(2024, 12, 31, 23, 59, 59)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn test_datetime_utc_validate_invalid_month() {
        // Month 0
        assert!(DateTimeUtc::new(2024, 0, 15, 12, 0, 0).validate().is_err());

        // Month 13
        assert!(DateTimeUtc::new(2024, 13, 15, 12, 0, 0).validate().is_err());
    }

    #[test]
    fn test_datetime_utc_validate_invalid_day() {
        // Day 0
        assert!(DateTimeUtc::new(2024, 6, 0, 12, 0, 0).validate().is_err());

        // Day 32 in a 31-day month
        assert!(DateTimeUtc::new(2024, 1, 32, 12, 0, 0).validate().is_err());

        // Day 31 in a 30-day month
        assert!(DateTimeUtc::new(2024, 4, 31, 12, 0, 0).validate().is_err());

        // Day 30 in February (leap year)
        assert!(DateTimeUtc::new(2024, 2, 30, 12, 0, 0).validate().is_err());

        // Day 29 in February (non-leap year)
        assert!(DateTimeUtc::new(2023, 2, 29, 12, 0, 0).validate().is_err());
    }

    #[test]
    fn test_datetime_utc_validate_leap_year() {
        // Leap year - Feb 29 is valid
        assert!(DateTimeUtc::new(2024, 2, 29, 12, 0, 0).validate().is_ok());
        assert!(DateTimeUtc::new(2000, 2, 29, 12, 0, 0).validate().is_ok()); // divisible by 400

        // Non-leap year - Feb 29 is invalid
        assert!(DateTimeUtc::new(2023, 2, 29, 12, 0, 0).validate().is_err());
        assert!(DateTimeUtc::new(1900, 2, 29, 12, 0, 0).validate().is_err()); // divisible by 100 but not 400
    }

    #[test]
    fn test_datetime_utc_validate_invalid_hour() {
        // Hour 24
        assert!(DateTimeUtc::new(2024, 6, 15, 24, 0, 0).validate().is_err());
    }

    #[test]
    fn test_datetime_utc_validate_invalid_minute() {
        // Minute 60
        assert!(DateTimeUtc::new(2024, 6, 15, 12, 60, 0).validate().is_err());
    }

    #[test]
    fn test_datetime_utc_validate_invalid_second() {
        // Second 60
        assert!(
            DateTimeUtc::new(2024, 6, 15, 12, 30, 60)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn test_datetime_utc_to_rfc2822() {
        // Test a known date
        let dt = DateTimeUtc::new(2024, 1, 15, 10, 30, 45);
        let rfc2822 = dt.to_rfc2822();

        // Should contain date parts
        assert!(rfc2822.contains("15"));
        assert!(rfc2822.contains("Jan"));
        assert!(rfc2822.contains("2024"));
        assert!(rfc2822.contains("10:30:45"));
        assert!(rfc2822.contains("GMT"));
    }

    #[test]
    fn test_datetime_utc_to_rfc2822_format() {
        let dt = DateTimeUtc::new(2024, 6, 15, 14, 30, 45);
        let rfc2822 = dt.to_rfc2822();

        // Check the general format: "Day, DD Mon YYYY HH:MM:SS GMT"
        let parts: Vec<&str> = rfc2822.split(' ').collect();
        assert_eq!(parts.len(), 6);
        assert!(parts[0].ends_with(','));
        assert_eq!(parts[5], "GMT");
    }

    #[test]
    fn test_datetime_utc_all_months() {
        let months = [
            (1, "Jan"),
            (2, "Feb"),
            (3, "Mar"),
            (4, "Apr"),
            (5, "May"),
            (6, "Jun"),
            (7, "Jul"),
            (8, "Aug"),
            (9, "Sep"),
            (10, "Oct"),
            (11, "Nov"),
            (12, "Dec"),
        ];

        for (month_num, month_name) in months {
            let dt = DateTimeUtc::new(2024, month_num, 15, 12, 0, 0);
            assert!(dt.validate().is_ok());
            let rfc2822 = dt.to_rfc2822();
            assert!(
                rfc2822.contains(month_name),
                "Month {} should contain {}",
                month_num,
                month_name
            );
        }
    }
}
