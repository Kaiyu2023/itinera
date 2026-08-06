use chrono::{NaiveDate, NaiveTime};
use url::Url;

pub const MAX_TRIP_DAYS: i64 = 90;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub &'static str);

pub fn required_text(
    value: String,
    field: &'static str,
    max_len: usize,
) -> Result<String, ValidationError> {
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(ValidationError(field));
    }
    if value.chars().count() > max_len {
        return Err(ValidationError(field));
    }
    Ok(value)
}

pub fn optional_text(
    value: Option<String>,
    max_len: usize,
) -> Result<Option<String>, ValidationError> {
    value
        .map(|value| {
            let value = value.trim().to_string();
            if value.is_empty() {
                Ok(None)
            } else if value.chars().count() > max_len {
                Err(ValidationError("text exceeds the allowed length"))
            } else {
                Ok(Some(value))
            }
        })
        .transpose()
        .map(Option::flatten)
}

pub fn http_url(value: Option<String>) -> Result<Option<String>, ValidationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = required_text(value, "URL must not be empty", 2_048)?;
    if !value.starts_with("http://") && !value.starts_with("https://") {
        return Err(ValidationError("URL must use http or https"));
    }
    let parsed = Url::parse(&value).map_err(|_| ValidationError("URL must be absolute"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ValidationError("URL must use http or https"));
    }
    Ok(Some(value))
}

pub fn currency(value: String) -> Result<String, ValidationError> {
    let value = value.trim().to_ascii_uppercase();
    if value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase()) {
        Ok(value)
    } else {
        Err(ValidationError(
            "currency must be a three-letter ISO 4217 code",
        ))
    }
}

pub fn date(value: &str) -> Result<NaiveDate, ValidationError> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return Err(ValidationError("date must use YYYY-MM-DD"));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| ValidationError("date must use YYYY-MM-DD"))
}

pub fn trip_dates(start: &str, end: &str) -> Result<Vec<NaiveDate>, ValidationError> {
    let start = date(start)?;
    let end = date(end)?;
    let span = end.signed_duration_since(start).num_days();
    if span < 0 {
        return Err(ValidationError("endDate must not be before startDate"));
    }
    if span >= MAX_TRIP_DAYS {
        return Err(ValidationError("a trip may contain at most 90 days"));
    }
    Ok((0..=span)
        .filter_map(|offset| start.checked_add_days(chrono::Days::new(offset as u64)))
        .collect())
}

pub fn local_time(value: &str) -> Result<(), ValidationError> {
    let bytes = value.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 2 && !byte.is_ascii_digit())
    {
        return Err(ValidationError("local time must use HH:MM"));
    }
    NaiveTime::parse_from_str(value, "%H:%M")
        .map(|_| ())
        .map_err(|_| ValidationError("local time must use HH:MM"))
}

pub fn time_window(start: &str, end: &str) -> Result<(), ValidationError> {
    local_time(start)?;
    local_time(end)?;
    let start = NaiveTime::parse_from_str(start, "%H:%M")
        .map_err(|_| ValidationError("local time must use HH:MM"))?;
    let end = NaiveTime::parse_from_str(end, "%H:%M")
        .map_err(|_| ValidationError("local time must use HH:MM"))?;
    if end < start {
        Err(ValidationError("windowEnd must not be before windowStart"))
    } else {
        Ok(())
    }
}

pub fn bounded_strings(
    values: Vec<String>,
    max_items: usize,
    max_len: usize,
) -> Result<Vec<String>, ValidationError> {
    if values.len() > max_items {
        return Err(ValidationError("too many list items"));
    }
    values
        .into_iter()
        .filter_map(|value| {
            let value = value.trim().to_string();
            (!value.is_empty()).then_some(value)
        })
        .map(|value| {
            if value.chars().count() > max_len {
                Err(ValidationError("list item exceeds the allowed length"))
            } else {
                Ok(value)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trip_dates_are_inclusive_and_bounded_for_transactions() {
        let dates = trip_dates("2026-08-01", "2026-08-03").expect("valid range");
        assert_eq!(dates.len(), 3);
        assert!(trip_dates("2026-08-03", "2026-08-01").is_err());
        assert!(trip_dates("2026-01-01", "2026-04-01").is_err());
    }

    #[test]
    fn external_links_are_http_only() {
        assert!(http_url(Some("https://example.test/path".into())).is_ok());
        assert!(http_url(Some("javascript:alert(1)".into())).is_err());
        assert!(http_url(Some("HTTPS://example.test/path".into())).is_err());
        assert!(http_url(Some("   ".into())).is_err());
    }

    #[test]
    fn dates_and_local_times_use_the_exact_wire_shapes() {
        assert!(date("2026-08-05").is_ok());
        assert!(date("2026-8-5").is_err());
        assert!(local_time("09:05").is_ok());
        assert!(local_time("9:05").is_err());
    }
}
