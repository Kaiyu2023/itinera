use axum::{Json, http::StatusCode, response::IntoResponse};
use itinera_core::{
    ports::{
        access_policy::AccessPolicyError, auth::AuthError,
        content_history::ContentHistoryRepoError, place_catalog::PlaceCatalogError,
        proposal::ProposalRepoError, trip::TripRepoError, user::UserRepoError,
    },
    services::{
        candidates::CandidateServiceError, content_history::ContentHistoryServiceError,
        plans::PlanServiceError, proposals::ProposalServiceError, provisioning::ProvisionError,
        trips::TripServiceError,
    },
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

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status_code: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: message.into(),
        }
    }

    pub(crate) fn payload_too_large() -> Self {
        Self {
            status_code: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
            message: "The request body exceeds the allowed size.".to_string(),
        }
    }
}

impl From<axum::extract::rejection::JsonRejection> for ApiError {
    fn from(_: axum::extract::rejection::JsonRejection) -> Self {
        Self::bad_request("The JSON request body is invalid.")
    }
}

impl From<axum::extract::rejection::QueryRejection> for ApiError {
    fn from(_: axum::extract::rejection::QueryRejection) -> Self {
        Self::bad_request("The query parameters are invalid.")
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
            ProvisionError::RepoError(
                UserRepoError::DuplicateEmail(_) | UserRepoError::CorruptData,
            ) => ApiError {
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

impl From<TripRepoError> for ApiError {
    fn from(value: TripRepoError) -> Self {
        match value {
            TripRepoError::Unavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "trip_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            TripRepoError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "trip_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            TripRepoError::NotFound => ApiError {
                status_code: StatusCode::NOT_FOUND,
                code: "not_found",
                message: "The requested resource was not found.".to_string(),
            },
            TripRepoError::Forbidden => ApiError {
                status_code: StatusCode::FORBIDDEN,
                code: "forbidden",
                message: "You do not have permission to perform this operation.".to_string(),
            },
            TripRepoError::Conflict => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "conflict",
                message: "The request conflicts with the resource's current state.".to_string(),
            },
            TripRepoError::DuplicateInvite => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "duplicate_invite",
                message: "An invite for that email already exists.".to_string(),
            },
        }
    }
}

impl From<AccessPolicyError> for ApiError {
    fn from(_: AccessPolicyError) -> Self {
        ApiError {
            status_code: StatusCode::SERVICE_UNAVAILABLE,
            code: "access_policy_unavailable",
            message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
        }
    }
}

impl From<PlaceCatalogError> for ApiError {
    fn from(value: PlaceCatalogError) -> Self {
        match value {
            PlaceCatalogError::Unavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "place_catalog_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            PlaceCatalogError::NotFound => ApiError {
                status_code: StatusCode::NOT_FOUND,
                code: "not_found",
                message: "The requested place was not found.".to_string(),
            },
        }
    }
}

impl From<TripServiceError> for ApiError {
    fn from(value: TripServiceError) -> Self {
        match value {
            TripServiceError::Validation(error) => Self::bad_request(error.to_string()),
            TripServiceError::Repository(error) => error.into(),
            TripServiceError::UserRepository(UserRepoError::UserRepoUnavailable) => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "user_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            TripServiceError::UserRepository(_) => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "user_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            TripServiceError::AccessPolicy(error) => error.into(),
            TripServiceError::InvalidEmail => Self::bad_request("email must be a valid address"),
        }
    }
}

impl From<CandidateServiceError> for ApiError {
    fn from(value: CandidateServiceError) -> Self {
        match value {
            CandidateServiceError::Validation(error) => Self::bad_request(error.to_string()),
            CandidateServiceError::Repository(error) => error.into(),
            CandidateServiceError::Catalog(error) => error.into(),
        }
    }
}

impl From<PlanServiceError> for ApiError {
    fn from(value: PlanServiceError) -> Self {
        match value {
            PlanServiceError::Validation(error) => Self::bad_request(error.to_string()),
            PlanServiceError::Repository(error) => error.into(),
        }
    }
}

impl From<ContentHistoryRepoError> for ApiError {
    fn from(value: ContentHistoryRepoError) -> Self {
        match value {
            ContentHistoryRepoError::Unavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "history_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            ContentHistoryRepoError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "history_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            ContentHistoryRepoError::NotFound => ApiError {
                status_code: StatusCode::NOT_FOUND,
                code: "not_found",
                message: "The requested resource was not found.".to_string(),
            },
            ContentHistoryRepoError::Forbidden => ApiError {
                status_code: StatusCode::FORBIDDEN,
                code: "forbidden",
                message: "You do not have permission to perform this operation.".to_string(),
            },
            ContentHistoryRepoError::Conflict => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "conflict",
                message: "The edit no longer matches the resource's current state.".to_string(),
            },
            ContentHistoryRepoError::Unsupported => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "unsupported_edit",
                message: "This edit target cannot be reverted safely.".to_string(),
            },
            ContentHistoryRepoError::SafetyLimitExceeded => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "history_limit_exceeded",
                message: "This history operation exceeds the current safe processing limit."
                    .to_string(),
            },
        }
    }
}

impl From<ContentHistoryServiceError> for ApiError {
    fn from(value: ContentHistoryServiceError) -> Self {
        match value {
            ContentHistoryServiceError::Validation(error) => Self::bad_request(error.to_string()),
            ContentHistoryServiceError::Repository(error) => error.into(),
        }
    }
}

impl From<ProposalRepoError> for ApiError {
    fn from(value: ProposalRepoError) -> Self {
        match value {
            ProposalRepoError::Unavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "proposal_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            ProposalRepoError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "proposal_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            ProposalRepoError::NotFound => ApiError {
                status_code: StatusCode::NOT_FOUND,
                code: "not_found",
                message: "The requested resource was not found.".to_string(),
            },
            ProposalRepoError::Forbidden => ApiError {
                status_code: StatusCode::FORBIDDEN,
                code: "forbidden",
                message: "You do not have permission to perform this operation.".to_string(),
            },
            ProposalRepoError::Conflict => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "conflict",
                message: "The proposal conflicts with the current plan or lifecycle state."
                    .to_string(),
            },
            ProposalRepoError::InvalidChange => {
                ApiError::bad_request("The ChangeSet is invalid for the trip's current plan.")
            }
            ProposalRepoError::PollsUnavailable => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "poll_route_unavailable",
                message: "Poll-backed proposal routing is not available yet.".to_string(),
            },
            ProposalRepoError::SafetyLimitExceeded => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "proposal_limit_exceeded",
                message: "This proposal exceeds the current safe plan publication limit."
                    .to_string(),
            },
        }
    }
}

impl From<ProposalServiceError> for ApiError {
    fn from(value: ProposalServiceError) -> Self {
        match value {
            ProposalServiceError::Validation(error) => Self::bad_request(error.to_string()),
            ProposalServiceError::Repository(error) => error.into(),
        }
    }
}
