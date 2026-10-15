//! Sitemap date formatting after shared date and timestamp validation.

use tola_packages::date::{DateError, Timestamp, format_calendar_date, parse_date};
use typst::foundations::{Datetime, Value};

/// Calendar dates stay date-only; native timestamps use UTC and strings keep their offset.
pub(super) fn lastmod(value: &Value) -> Result<String, DateError> {
    if matches!(value, Value::Datetime(Datetime::Date(_)))
        || matches!(value, Value::Str(text) if text.len() == 10)
    {
        return Ok(format_calendar_date(&parse_date(value)?));
    }
    let timestamp = Timestamp::parse(value)?;
    Ok(if matches!(value, Value::Datetime(_)) {
        timestamp.rfc3339().to_owned()
    } else {
        timestamp.to_fixed().to_rfc3339()
    })
}
