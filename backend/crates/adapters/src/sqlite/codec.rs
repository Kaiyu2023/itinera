//! Small provider-neutral value codecs shared by SQLite capabilities.

use std::fmt::Write;

use chrono::{DateTime, NaiveDate};
use itinera_core::domain::user::Email;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodecError {
    Invalid,
    LimitExceeded,
}

pub(crate) fn email_digest(email: &Email) -> String {
    let digest = Sha256::digest(email.as_str().as_bytes());
    let mut output = String::with_capacity(64);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

pub(crate) fn validate_id(value: &str) -> Result<(), CodecError> {
    validate_text(value, 200)
}

pub(crate) fn validate_text(value: &str, max_chars: usize) -> Result<(), CodecError> {
    if value.is_empty() || value.trim() != value || value.chars().count() > max_chars {
        Err(CodecError::Invalid)
    } else {
        Ok(())
    }
}

pub(crate) fn validate_optional_text(
    value: Option<&str>,
    max_chars: usize,
) -> Result<(), CodecError> {
    value.map_or(Ok(()), |value| validate_text(value, max_chars))
}

pub(crate) fn validate_email(email: &Email) -> Result<(), CodecError> {
    let parsed = Email::parse(email.as_str()).map_err(|_| CodecError::Invalid)?;
    if &parsed == email && email.as_str().len() <= 320 {
        Ok(())
    } else {
        Err(CodecError::Invalid)
    }
}

pub(crate) fn validate_date(value: &str) -> Result<(), CodecError> {
    let parsed = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| CodecError::Invalid)?;
    if parsed.format("%Y-%m-%d").to_string() == value {
        Ok(())
    } else {
        Err(CodecError::Invalid)
    }
}

pub(crate) fn validate_utc(value: &str) -> Result<(), CodecError> {
    if !value.ends_with('Z') || value.len() > 35 || value.as_bytes().get(10) != Some(&b'T') {
        return Err(CodecError::Invalid);
    }
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| CodecError::Invalid)?;
    if parsed.offset().local_minus_utc() == 0 {
        Ok(())
    } else {
        Err(CodecError::Invalid)
    }
}

pub(crate) fn validate_currency(value: &str) -> Result<(), CodecError> {
    if value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err(CodecError::Invalid)
    }
}

pub(crate) fn checked_revision(value: i64) -> Result<i64, CodecError> {
    if value >= 1 {
        Ok(value)
    } else {
        Err(CodecError::Invalid)
    }
}

pub(crate) fn next_revision(value: i64) -> Result<i64, CodecError> {
    checked_revision(value)?
        .checked_add(1)
        .ok_or(CodecError::Invalid)
}

pub(crate) fn encode_json<T: Serialize>(value: &T) -> Result<String, CodecError> {
    serde_json::to_string(value).map_err(|_| CodecError::Invalid)
}

pub(crate) fn decode_json<T: DeserializeOwned>(value: &str) -> Result<T, CodecError> {
    serde_json::from_str(value).map_err(|_| CodecError::Invalid)
}

pub(crate) fn ensure_encoded_size<T: Serialize>(
    value: &T,
    maximum: usize,
) -> Result<(), CodecError> {
    let encoded = serde_json::to_vec(value).map_err(|_| CodecError::Invalid)?;
    if encoded.len() <= maximum {
        Ok(())
    } else {
        Err(CodecError::LimitExceeded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_size_includes_json_structure_at_the_exact_boundary() {
        let value = vec!["ab"];
        let length = serde_json::to_vec(&value).expect("encode").len();
        assert_eq!(ensure_encoded_size(&value, length), Ok(()));
        assert_eq!(
            ensure_encoded_size(&value, length - 1),
            Err(CodecError::LimitExceeded)
        );
    }

    #[test]
    fn utc_codec_requires_the_zulu_spelling() {
        assert!(validate_utc("2026-08-07T12:00:00.000Z").is_ok());
        assert!(validate_utc("2026-08-07T13:00:00+01:00").is_err());
        assert!(validate_utc("2026-08-07t12:00:00Z").is_err());
    }

    #[test]
    fn scalar_codecs_enforce_exact_boundaries() {
        assert!(validate_id(&"i".repeat(200)).is_ok());
        assert!(validate_id(&"i".repeat(201)).is_err());
        assert!(validate_id(" padded ").is_err());
        assert!(validate_date("2028-02-29").is_ok());
        assert!(validate_date("2027-02-29").is_err());
        assert!(validate_currency("GBP").is_ok());
        assert!(validate_currency("gbp").is_err());
        assert_eq!(checked_revision(i64::MAX), Ok(i64::MAX));
        assert_eq!(next_revision(i64::MAX), Err(CodecError::Invalid));
    }

    #[test]
    fn email_digest_is_lowercase_sha256_of_the_canonical_address() {
        let email = Email::parse("Cloud.Strife@Proton.ME").expect("valid email");
        assert_eq!(
            email_digest(&email),
            "edcf4000d8658323b33594d84bf3a141d08be83288fb71c723bc1556c261e025"
        );
    }
}
