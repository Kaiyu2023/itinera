use axum::{Json, http::StatusCode, response::IntoResponse};
use itinera_core::{
    ports::{auth::AuthError, user::UserRepoError},
    services::provisioning::ProvisionError,
};
use serde::Serialize;

const SERVICE_UNAVAILABLE_MESSAGE: &str = "Service unavailable, please try again later.";
const INTERNAL_SERVER_ERROR_MESSAGE: &str =
    "Something went wrong. If this persists, please contact support.";

#[derive(Debug, Clone, Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status_code: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn missing_credentials() -> Self {
        ApiError {
            status_code: StatusCode::UNAUTHORIZED,
            code: "missing_credentials",
            message: "No credentials are provided.".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = ErrorBody {
            error: self.code,
            message: self.message,
        };
        (self.status_code, Json(body)).into_response()
    }
}

impl From<AuthError> for ApiError {
    fn from(value: AuthError) -> Self {
        match value {
            AuthError::IdentityProviderUnavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "identity_provider_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            AuthError::ExpiredToken => ApiError {
                status_code: StatusCode::UNAUTHORIZED,
                code: "expired_token",
                message: "Credentials expired, please log in again".to_string(),
            },
            AuthError::InvalidToken => ApiError {
                status_code: StatusCode::UNAUTHORIZED,
                code: "invalid_token",
                message: "Invalid credentials.".to_string(),
            },
        }
    }
}

impl From<ProvisionError> for ApiError {
    fn from(value: ProvisionError) -> Self {
        match value {
            ProvisionError::RepoError(UserRepoError::UserRepoUnavailable) => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "user_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            ProvisionError::RepoError(UserRepoError::DuplicateEmail(_)) => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "user_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            ProvisionError::VanishedAfterConflict(_) => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "provision_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
        }
    }
}
