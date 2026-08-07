use chrono::{NaiveDate, NaiveTime};
use url::Url;

use crate::domain::trip::{Booking, CandidatePlaceInput, Place, PlaceActivityIdea, PlaceGuide};

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

pub fn exact_required_text(
    value: &str,
    field: &'static str,
    max_len: usize,
) -> Result<(), ValidationError> {
    if required_text(value.to_string(), field, max_len)? == value {
        Ok(())
    } else {
        Err(ValidationError(field))
    }
}

pub fn exact_bounded_strings(
    values: &[String],
    max_items: usize,
    max_len: usize,
) -> Result<(), ValidationError> {
    if bounded_strings(values.to_vec(), max_items, max_len)? == values {
        Ok(())
    } else {
        Err(ValidationError("list values must already be normalized"))
    }
}

pub fn text_len(value: &str, max_len: usize) -> Result<(), ValidationError> {
    if value.chars().count() <= max_len {
        Ok(())
    } else {
        Err(ValidationError("text exceeds the allowed length"))
    }
}

pub fn duration_min(value: u32) -> Result<(), ValidationError> {
    if (1..=1_440).contains(&value) {
        Ok(())
    } else {
        Err(ValidationError("durationMin must be between 1 and 1,440"))
    }
}

pub fn normalise_booking(mut booking: Booking) -> Result<Booking, ValidationError> {
    booking.reference = required_text(
        booking.reference,
        "booking ref is required and must be at most 200 characters",
        200,
    )?;
    booking.url = http_url(booking.url)?;
    booking.ledger_entry_id = optional_text(booking.ledger_entry_id, 200)?;
    if let Some(cost) = booking.cost.as_mut() {
        if !cost.amount.is_finite() || cost.amount < 0.0 {
            return Err(ValidationError(
                "booking cost must be a non-negative number",
            ));
        }
        cost.currency = currency(std::mem::take(&mut cost.currency))?;
    }
    Ok(booking)
}

pub fn validate_booking(booking: Option<&Booking>) -> Result<(), ValidationError> {
    let Some(booking) = booking else {
        return Ok(());
    };
    if normalise_booking(booking.clone())? == *booking {
        Ok(())
    } else {
        Err(ValidationError("booking values must already be normalized"))
    }
}

pub fn normalise_candidate_place(
    input: CandidatePlaceInput,
) -> Result<CandidatePlaceInput, ValidationError> {
    let guide = input.guide.map(normalise_guide).transpose()?;
    Ok(CandidatePlaceInput {
        name: required_text(
            input.name,
            "place name is required and must be at most 200 characters",
            200,
        )?,
        kind: input.kind,
        city: required_text(
            input.city,
            "city is required and must be at most 120 characters",
            120,
        )?,
        address: optional_text(Some(input.address), 500)?.unwrap_or_default(),
        website: http_url(input.website)?,
        phone: optional_text(input.phone, 80)?,
        opening_hours: bounded_strings(input.opening_hours, 14, 200)?,
        photo_urls: bounded_strings(input.photo_urls, 20, 2_048)?,
        guide,
    })
}

pub fn validate_place_snapshot(place: &Place) -> Result<(), ValidationError> {
    exact_required_text(&place.id, "place id must be normalized", 200)?;
    if !place.lat.is_finite()
        || !(-90.0..=90.0).contains(&place.lat)
        || !place.lng.is_finite()
        || !(-180.0..=180.0).contains(&place.lng)
        || place
            .rating
            .is_some_and(|rating| !rating.is_finite() || !(0.0..=5.0).contains(&rating))
        || place
            .price_level
            .is_some_and(|level| !(1..=4).contains(&level))
        || place.tz.chars().count() > 100
        || place.country_code.chars().count() > 2
        || place.admin_area.chars().count() > 200
        || place.external_ref.as_ref().is_some_and(|reference| {
            exact_required_text(&reference.provider, "provider must be normalized", 100).is_err()
                || exact_required_text(
                    &reference.place_id,
                    "provider place id must be normalized",
                    500,
                )
                .is_err()
        })
        || place
            .opening_hours
            .as_ref()
            .is_some_and(|hours| hours.weekday_text.is_empty())
    {
        return Err(ValidationError("place snapshot is invalid"));
    }

    let authored = CandidatePlaceInput {
        name: place.name.clone(),
        kind: place.kind,
        city: place.city.clone(),
        address: place.address.clone(),
        website: place.website.clone(),
        phone: place.phone.clone(),
        opening_hours: place
            .opening_hours
            .as_ref()
            .map_or_else(Vec::new, |hours| hours.weekday_text.clone()),
        photo_urls: place.photo_urls.clone(),
        guide: place.guide.clone(),
    };
    if normalise_candidate_place(authored.clone())? != authored {
        return Err(ValidationError(
            "place snapshot values must already be normalized",
        ));
    }
    Ok(())
}

fn normalise_guide(guide: PlaceGuide) -> Result<PlaceGuide, ValidationError> {
    if guide.activity_ideas.len() > 20 {
        return Err(ValidationError(
            "a guide may contain at most 20 activity ideas",
        ));
    }
    let activity_ideas = guide
        .activity_ideas
        .into_iter()
        .map(|idea| {
            Ok(PlaceActivityIdea {
                title: required_text(idea.title, "activity title is required", 160)?,
                details: optional_text(idea.details, 1_000)?,
            })
        })
        .collect::<Result<_, ValidationError>>()?;
    Ok(PlaceGuide {
        summary: required_text(guide.summary, "guide summary is required", 500)?,
        intro: required_text(guide.intro, "guide introduction is required", 4_000)?,
        activity_ideas,
        practical_tips: bounded_strings(guide.practical_tips, 30, 500)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
