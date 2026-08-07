use axum::{body::Bytes, extract::rejection::BytesRejection, http::HeaderMap};

use crate::error::ApiError;

pub mod candidates;
pub mod content_history;
pub mod discussions;
pub mod ledger;
pub mod me;
pub mod notices;
pub mod plans;
pub mod polls;
pub mod proposals;
pub mod trips;

pub(crate) fn require_empty_body(body: Result<Bytes, BytesRejection>) -> Result<(), ApiError> {
    let body = body.map_err(|_| ApiError::payload_too_large())?;
    if body.is_empty() {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "This operation does not accept a request body.",
        ))
    }
}

pub(crate) fn required_idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let mut values = headers.get_all("idempotency-key").iter();
    let value = values
        .next()
        .ok_or_else(|| ApiError::bad_request("Idempotency-Key header is required"))?;
    if values.next().is_some() {
        return Err(ApiError::bad_request(
            "Idempotency-Key header must appear exactly once",
        ));
    }
    value
        .to_str()
        .map(str::to_string)
        .map_err(|_| ApiError::bad_request("Idempotency-Key header is invalid"))
}
