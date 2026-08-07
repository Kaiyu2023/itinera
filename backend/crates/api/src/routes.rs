use axum::{body::Bytes, extract::rejection::BytesRejection};

use crate::error::ApiError;

pub mod candidates;
pub mod content_history;
pub mod discussions;
pub mod me;
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
