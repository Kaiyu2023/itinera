use axum::{Json, http::StatusCode, response::IntoResponse};
use itinera_core::{
    ports::{
        access_policy::AccessPolicyError, auth::AuthError,
        content_history::ContentHistoryRepoError, discussion::DiscussionRepoError,
        fx_rate::FxRateError, ledger::LedgerRepoError, notice::NoticeRepoError,
        place_catalog::PlaceCatalogError, poll::PollRepoError, proposal::ProposalRepoError,
        service_identity::ServiceIdentityRepoError, trip::TripRepoError, user::UserRepoError,
    },
    services::{
        candidates::CandidateServiceError, content_history::ContentHistoryServiceError,
        discussions::DiscussionServiceError, ledger::LedgerServiceError,
        notices::NoticeServiceError, plans::PlanServiceError, polls::PollServiceError,
        proposals::ProposalServiceError, provisioning::ProvisionError,
        service_identities::ServiceIdentityServiceError, trips::TripServiceError,
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

    pub(crate) fn forbidden() -> Self {
        Self {
            status_code: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: "You do not have permission to perform this operation.".to_string(),
        }
    }

    pub(crate) fn not_found() -> Self {
        Self {
            status_code: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "The requested resource was not found.".to_string(),
        }
    }
}

impl From<axum::extract::rejection::JsonRejection> for ApiError {
    fn from(value: axum::extract::rejection::JsonRejection) -> Self {
        match value {
            axum::extract::rejection::JsonRejection::BytesRejection(_) => Self::payload_too_large(),
            _ => Self::bad_request("The JSON request body is invalid."),
        }
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

impl From<UserRepoError> for ApiError {
    fn from(value: UserRepoError) -> Self {
        match value {
            UserRepoError::UserRepoUnavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "user_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            UserRepoError::DuplicateEmail(_) | UserRepoError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "user_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
        }
    }
}

impl From<ServiceIdentityRepoError> for ApiError {
    fn from(value: ServiceIdentityRepoError) -> Self {
        match value {
            ServiceIdentityRepoError::Unavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "service_identity_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            ServiceIdentityRepoError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "service_identity_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            ServiceIdentityRepoError::NotFound => ApiError::not_found(),
            ServiceIdentityRepoError::Forbidden => ApiError::forbidden(),
            ServiceIdentityRepoError::Conflict => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "conflict",
                message: "The service identity conflicts with its current state.".to_string(),
            },
            ServiceIdentityRepoError::DuplicateCredential => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "duplicate_service_credential",
                message: "That Cloudflare service credential is already mapped.".to_string(),
            },
            ServiceIdentityRepoError::CredentialRejected => ApiError {
                status_code: StatusCode::UNAUTHORIZED,
                code: "invalid_service_identity",
                message: "Invalid service credentials.".to_string(),
            },
            ServiceIdentityRepoError::RateLimited => ApiError {
                status_code: StatusCode::TOO_MANY_REQUESTS,
                code: "service_rate_limited",
                message: "This service identity has reached its hourly request limit.".to_string(),
            },
            ServiceIdentityRepoError::SafetyLimitExceeded => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "service_identity_limit_exceeded",
                message: "This service identity operation exceeds the current safe limit."
                    .to_string(),
            },
        }
    }
}

impl From<ServiceIdentityServiceError> for ApiError {
    fn from(value: ServiceIdentityServiceError) -> Self {
        match value {
            ServiceIdentityServiceError::Validation(error) => Self::bad_request(error.to_string()),
            ServiceIdentityServiceError::Repository(error) => error.into(),
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
            ProposalServiceError::PollRepository(error) => error.into(),
        }
    }
}

impl From<PollRepoError> for ApiError {
    fn from(value: PollRepoError) -> Self {
        match value {
            PollRepoError::Unavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "poll_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            PollRepoError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "poll_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            PollRepoError::NotFound => ApiError {
                status_code: StatusCode::NOT_FOUND,
                code: "not_found",
                message: "The requested resource was not found.".to_string(),
            },
            PollRepoError::Forbidden => ApiError {
                status_code: StatusCode::FORBIDDEN,
                code: "forbidden",
                message: "You do not have permission to perform this operation.".to_string(),
            },
            PollRepoError::Conflict => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "conflict",
                message: "The poll conflicts with its current lifecycle state.".to_string(),
            },
            PollRepoError::InvalidVote => {
                ApiError::bad_request("optionIds is not a valid ballot for this poll.")
            }
            PollRepoError::SafetyLimitExceeded => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "poll_limit_exceeded",
                message: "This poll operation exceeds the current safe processing limit."
                    .to_string(),
            },
        }
    }
}

impl From<PollServiceError> for ApiError {
    fn from(value: PollServiceError) -> Self {
        match value {
            PollServiceError::Validation(error) => Self::bad_request(error.to_string()),
            PollServiceError::Repository(error) => error.into(),
        }
    }
}

impl From<DiscussionRepoError> for ApiError {
    fn from(value: DiscussionRepoError) -> Self {
        match value {
            DiscussionRepoError::Unavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "discussion_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            DiscussionRepoError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "discussion_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            DiscussionRepoError::NotFound => ApiError {
                status_code: StatusCode::NOT_FOUND,
                code: "not_found",
                message: "The requested resource was not found.".to_string(),
            },
            DiscussionRepoError::Forbidden => ApiError {
                status_code: StatusCode::FORBIDDEN,
                code: "forbidden",
                message: "You do not have permission to perform this operation.".to_string(),
            },
            DiscussionRepoError::Conflict => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "conflict",
                message: "The discussion conflicts with its current state.".to_string(),
            },
            DiscussionRepoError::SafetyLimitExceeded => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "discussion_limit_exceeded",
                message: "This discussion exceeds the current safe processing limit.".to_string(),
            },
        }
    }
}

impl From<DiscussionServiceError> for ApiError {
    fn from(value: DiscussionServiceError) -> Self {
        match value {
            DiscussionServiceError::Validation(error) => Self::bad_request(error.to_string()),
            DiscussionServiceError::Repository(error) => error.into(),
        }
    }
}

impl From<LedgerRepoError> for ApiError {
    fn from(value: LedgerRepoError) -> Self {
        match value {
            LedgerRepoError::Unavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "ledger_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            LedgerRepoError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "ledger_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            LedgerRepoError::NotFound => ApiError {
                status_code: StatusCode::NOT_FOUND,
                code: "not_found",
                message: "The requested resource was not found.".to_string(),
            },
            LedgerRepoError::Forbidden => ApiError {
                status_code: StatusCode::FORBIDDEN,
                code: "forbidden",
                message: "You do not have permission to perform this operation.".to_string(),
            },
            LedgerRepoError::Conflict => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "conflict",
                message: "The ledger conflicts with its current state.".to_string(),
            },
            LedgerRepoError::SafetyLimitExceeded => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "ledger_limit_exceeded",
                message: "This ledger operation exceeds the current safe processing limit."
                    .to_string(),
            },
        }
    }
}

impl From<FxRateError> for ApiError {
    fn from(value: FxRateError) -> Self {
        match value {
            FxRateError::UnsupportedCurrency => {
                Self::bad_request("The currency pair is not supported.")
            }
            FxRateError::Unavailable | FxRateError::InvalidResponse => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "fx_rate_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
        }
    }
}

impl From<LedgerServiceError> for ApiError {
    fn from(value: LedgerServiceError) -> Self {
        match value {
            LedgerServiceError::Validation(error) => Self::bad_request(error.to_string()),
            LedgerServiceError::Repository(error) => error.into(),
            LedgerServiceError::FxRate(error) => error.into(),
            LedgerServiceError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "ledger_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
        }
    }
}

impl From<NoticeRepoError> for ApiError {
    fn from(value: NoticeRepoError) -> Self {
        match value {
            NoticeRepoError::Unavailable => ApiError {
                status_code: StatusCode::SERVICE_UNAVAILABLE,
                code: "notice_store_unavailable",
                message: SERVICE_UNAVAILABLE_MESSAGE.to_string(),
            },
            NoticeRepoError::CorruptData => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                code: "notice_store_internal_error",
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
            NoticeRepoError::NotFound => ApiError {
                status_code: StatusCode::NOT_FOUND,
                code: "not_found",
                message: "The requested resource was not found.".to_string(),
            },
            NoticeRepoError::Forbidden => ApiError {
                status_code: StatusCode::FORBIDDEN,
                code: "forbidden",
                message: "You do not have permission to perform this operation.".to_string(),
            },
            NoticeRepoError::Conflict => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "conflict",
                message: "The notice conflicts with its current state.".to_string(),
            },
            NoticeRepoError::SafetyLimitExceeded => ApiError {
                status_code: StatusCode::CONFLICT,
                code: "notice_limit_exceeded",
                message: "This notice operation exceeds the current safe processing limit."
                    .to_string(),
            },
        }
    }
}

impl From<NoticeServiceError> for ApiError {
    fn from(value: NoticeServiceError) -> Self {
        match value {
            NoticeServiceError::Validation(error) => Self::bad_request(error.to_string()),
            NoticeServiceError::Repository(error) => error.into(),
        }
    }
}
