//! DynamoDB content-history repository.
//!
//! Audit reads and safe revert are a separate repository capability. They
//! share the one-table record codec with trip persistence, but own their
//! authorization reads, audit validation, allowlist, and complete revert
//! transaction here.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use aws_sdk_dynamodb::{
    operation::transact_write_items::TransactWriteItemsError,
    types::{AttributeValue, ConditionCheck, Put},
};
use itinera_core::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        trip::{
            Booking, Candidate, CandidateStatus, Day, Place, Stop, TripMember, TripRole, TripStatus,
        },
        user::UserId,
    },
    ports::{
        content_history::{ContentHistoryRepo, ContentHistoryRepoError},
        trip::TripRepoError,
    },
    services::validation::{
        duration_min as canonical_duration_min, exact_bounded_strings as canonical_bounded_strings,
        exact_required_text as canonical_required_text, local_time as canonical_local_time,
        text_len as canonical_text_len, time_window as canonical_time_window,
        validate_booking as canonical_booking, validate_place_snapshot as canonical_place,
    },
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::trip_repo::records::{
    AUDIT_ENTITY, CANDIDATE_ENTITY, CURRENT_PLAN_ID, CURRENT_PLAN_VERSION, DATA, DAY_ENTITY,
    GSI1PK, GSI1SK, LEADER_COUNT, MEMBER_COUNT, MEMBER_ENTITY, META_SK, PLACE_ENTITY, REVISION,
    ROLE, STOP_ENTITY, Stored, TRIP_COLLECTION_PAGE_SIZE, TRIP_ENTITY, TripMeta, audit_sk,
    candidate_sk, decode_record, encode_record, encode_trip_meta, member_sk, number_u64, place_sk,
    plan_prefix, role_value, string, trip_pk,
};
use super::{
    CONDITIONAL_FAILURE, DynamoUserRepo, ENTITY_TYPE, PK, SK, USER_ID, user_partition_key,
};

mod access;
mod audit;
mod revert;

#[async_trait]
impl ContentHistoryRepo for DynamoUserRepo {
    async fn list_history(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Edit>, ContentHistoryRepoError> {
        audit::list_history(self, trip_id, actor).await
    }

    async fn revert_edit(
        &self,
        trip_id: &str,
        actor: &UserId,
        edit_id: &str,
        reverted_at: &str,
        compensating_edit_id: &str,
    ) -> Result<(), ContentHistoryRepoError> {
        revert::revert_edit(
            self,
            trip_id,
            actor,
            edit_id,
            reverted_at,
            compensating_edit_id,
        )
        .await
    }
}

fn record_error(error: TripRepoError) -> ContentHistoryRepoError {
    match error {
        TripRepoError::Unavailable => ContentHistoryRepoError::Unavailable,
        _ => ContentHistoryRepoError::CorruptData,
    }
}

#[cfg(test)]
mod tests;
