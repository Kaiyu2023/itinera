//! Verification that a request passed through the approved Cloudflare Worker.
//!
//! Cloudflare holds the high-entropy plaintext as a Worker secret. Lambda gets
//! only one or two SHA-256 digests, so neither Terraform state nor Lambda
//! configuration contains a credential that can be replayed at the origin.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::{Choice, ConstantTimeEq};
use thiserror::Error;

pub const ORIGIN_HEADER: &str = "x-itinera-origin-verification";
const SHA256_HEX_LENGTH: usize = 64;
const SHA256_BYTES: usize = 32;
const MAX_ACTIVE_HASHES: usize = 2;
const MIN_SECRET_LENGTH: usize = 32;
const MAX_SECRET_LENGTH: usize = 128;

#[derive(Clone)]
pub struct OriginGuard {
    accepted_hashes: Arc<[[u8; SHA256_BYTES]]>,
}

impl OriginGuard {
    /// Parse the comma-separated lowercase SHA-256 digests supplied by the
    /// deployment. Two digests are accepted only to rotate without downtime:
    /// deploy old+new, switch the Worker secret, then deploy new only.
    pub fn from_sha256_hashes(value: &str) -> Result<Self, OriginGuardBuildError> {
        let encoded_hashes: Vec<_> = value.split(',').collect();
        if encoded_hashes.is_empty() || encoded_hashes.iter().any(|hash| hash.is_empty()) {
            return Err(OriginGuardBuildError::MissingHash);
        }
        if encoded_hashes.len() > MAX_ACTIVE_HASHES {
            return Err(OriginGuardBuildError::TooManyHashes);
        }

        let accepted_hashes = encoded_hashes
            .into_iter()
            .map(decode_sha256)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            accepted_hashes: accepted_hashes.into(),
        })
    }

    fn accepts(&self, headers: &HeaderMap) -> bool {
        let mut values = headers.get_all(ORIGIN_HEADER).iter();
        let Some(value) = values.next() else {
            return false;
        };
        // Ambiguous duplicate security headers are rejected rather than
        // relying on different proxy/parser choices about first versus last.
        if values.next().is_some() {
            return false;
        }
        let Ok(secret) = value.to_str() else {
            return false;
        };
        if !(MIN_SECRET_LENGTH..=MAX_SECRET_LENGTH).contains(&secret.len()) {
            return false;
        }

        let candidate = Sha256::digest(secret.as_bytes());
        let matches = self
            .accepted_hashes
            .iter()
            .fold(Choice::from(0), |matched, expected| {
                matched | candidate[..].ct_eq(&expected[..])
            });
        bool::from(matches)
    }
}

pub async fn require_verified_origin(
    State(guard): State<OriginGuard>,
    request: Request,
    next: Next,
) -> Response {
    if !guard.accepts(request.headers()) {
        // Never log the supplied value. Function URL Url4xxCount provides an
        // aggregate count without creating attacker-controlled log volume.
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "unverified_origin",
                "message": "The request could not be accepted."
            })),
        )
            .into_response();
    }

    next.run(request).await
}

fn decode_sha256(value: &str) -> Result<[u8; SHA256_BYTES], OriginGuardBuildError> {
    if value.len() != SHA256_HEX_LENGTH
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OriginGuardBuildError::InvalidHash);
    }

    let mut decoded = [0; SHA256_BYTES];
    for (index, byte) in decoded.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&value[start..start + 2], 16)
            .map_err(|_| OriginGuardBuildError::InvalidHash)?;
    }
    Ok(decoded)
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum OriginGuardBuildError {
    #[error("at least one origin-secret SHA-256 hash is required")]
    MissingHash,
    #[error("at most two origin-secret SHA-256 hashes may be active")]
    TooManyHashes,
    #[error("origin-secret hashes must be 64 lowercase hexadecimal characters")]
    InvalidHash,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const SECRET: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG";

    fn digest(secret: &str) -> String {
        format!("{:x}", Sha256::digest(secret.as_bytes()))
    }

    #[test]
    fn exact_secret_is_accepted_without_storing_plaintext() {
        let guard = OriginGuard::from_sha256_hashes(&digest(SECRET)).expect("valid hash");
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN_HEADER, HeaderValue::from_static(SECRET));

        assert!(guard.accepts(&headers));
    }

    #[test]
    fn missing_wrong_and_duplicate_headers_are_rejected() {
        let guard = OriginGuard::from_sha256_hashes(&digest(SECRET)).expect("valid hash");
        assert!(!guard.accepts(&HeaderMap::new()));

        let mut wrong = HeaderMap::new();
        wrong.insert(
            ORIGIN_HEADER,
            HeaderValue::from_static("0123456789abcdefghijklmnopqrstuvwxyzWRONGGG"),
        );
        assert!(!guard.accepts(&wrong));

        let mut duplicate = HeaderMap::new();
        duplicate.append(ORIGIN_HEADER, HeaderValue::from_static(SECRET));
        duplicate.append(ORIGIN_HEADER, HeaderValue::from_static(SECRET));
        assert!(!guard.accepts(&duplicate));
    }

    #[test]
    fn two_hashes_support_rotation_but_a_third_is_rejected() {
        let next = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghi";
        let hashes = format!("{},{}", digest(SECRET), digest(next));
        let guard = OriginGuard::from_sha256_hashes(&hashes).expect("two hashes are valid");
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN_HEADER, HeaderValue::from_static(next));
        assert!(guard.accepts(&headers));

        assert_eq!(
            OriginGuard::from_sha256_hashes(&format!("{hashes},{}", digest("third")))
                .err()
                .expect("three hashes must fail"),
            OriginGuardBuildError::TooManyHashes
        );
    }

    #[test]
    fn hashes_have_one_canonical_encoding() {
        assert_eq!(
            OriginGuard::from_sha256_hashes("")
                .err()
                .expect("empty must fail"),
            OriginGuardBuildError::MissingHash
        );
        assert_eq!(
            OriginGuard::from_sha256_hashes(&"A".repeat(SHA256_HEX_LENGTH))
                .err()
                .expect("uppercase must fail"),
            OriginGuardBuildError::InvalidHash
        );
    }
}
