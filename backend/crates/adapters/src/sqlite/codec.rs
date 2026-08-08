//! Small provider-neutral value codecs shared by SQLite capabilities.

use std::fmt::Write;

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

pub(crate) fn encode_json<T: Serialize + ?Sized>(value: &T) -> Result<String, CodecError> {
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
    use itinera_core::domain::user::Email;

    use super::{
        CodecError, checked_revision, email_digest, ensure_encoded_size, next_revision, validate_id,
    };

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
    fn scalar_codecs_enforce_exact_boundaries() {
        assert!(validate_id(&"i".repeat(200)).is_ok());
        assert!(validate_id(&"i".repeat(201)).is_err());
        assert!(validate_id(" padded ").is_err());
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
