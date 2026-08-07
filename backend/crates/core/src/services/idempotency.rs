use serde::Serialize;
use sha2::{Digest, Sha256};

use super::validation::ValidationError;

pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;

pub fn validate_idempotency_key(value: &str) -> Result<(), ValidationError> {
    if value.is_empty()
        || value.len() > MAX_IDEMPOTENCY_KEY_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Err(ValidationError(
            "Idempotency-Key must contain 1 to 128 safe ASCII characters",
        ))
    } else {
        Ok(())
    }
}

pub fn request_hash<T: Serialize>(kind: &str, input: &T) -> Result<String, serde_json::Error> {
    let canonical = serde_json::to_vec(input)?;
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update([0]);
    digest.update(canonical);
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::{request_hash, validate_idempotency_key};

    #[derive(Serialize)]
    struct Input<'a> {
        value: &'a str,
    }

    #[test]
    fn keys_are_bounded_and_use_a_log_safe_alphabet() {
        assert!(validate_idempotency_key("retry_2026-08-06:1").is_ok());
        assert!(validate_idempotency_key("").is_err());
        assert!(validate_idempotency_key(&"x".repeat(129)).is_err());
        assert!(validate_idempotency_key("contains a space").is_err());
        assert!(validate_idempotency_key("line\nbreak").is_err());
    }

    #[test]
    fn hashes_are_kind_separated_and_canonical() {
        let input = Input { value: "same" };
        assert_eq!(
            request_hash("notice", &input).expect("hash"),
            request_hash("notice", &input).expect("hash")
        );
        assert_ne!(
            request_hash("notice", &input).expect("hash"),
            request_hash("ledger", &input).expect("hash")
        );
    }
}
