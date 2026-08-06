//! DynamoDB structural-proposal repository.
//!
//! Governance persistence is deliberately separate from `TripRepo`. This
//! capability owns proposal lifecycle validation, direct membership checks,
//! immutable plan cloning, and the complete compare-and-swap transaction that
//! publishes a proposal.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use aws_sdk_dynamodb::{
    operation::transact_write_items::TransactWriteItemsError,
    types::{AttributeValue, ConditionCheck, Put, TransactWriteItem},
};
use itinera_core::{
    domain::{
        proposal::{ChangeOp, Proposal, ProposalDecision, ProposalRoute, ProposalStatus},
        trip::{
            Candidate, CandidateStatus, Day, Place, Plan, PlanDetail, Stop, TripMember, TripRole,
        },
        user::UserId,
    },
    ports::proposal::{ProposalApplicationIds, ProposalRepo, ProposalRepoError},
    services::{
        candidates::validate_stored_candidate,
        proposals::{
            ChangeApplicationError, PlanApplication, apply_change_set, validate_stored_proposal,
        },
        validation::validate_place_snapshot,
    },
};
use serde::de::DeserializeOwned;

use super::trip_repo::records::{
    CANDIDATE_ENTITY, CURRENT_PLAN_ID, CURRENT_PLAN_VERSION, DATA, DAY_ENTITY, GSI1PK, GSI1SK,
    LEADER_COUNT, MEMBER_COUNT, MEMBER_ENTITY, META_SK, PLACE_ENTITY, PLAN_ENTITY, REVISION, ROLE,
    STOP_ENTITY, Stored, TRIP_COLLECTION_PAGE_SIZE, TRIP_ENTITY, TripMeta, candidate_sk, day_sk,
    decode_record, encode_record, encode_trip_meta, member_sk, number_u64, place_sk, plan_prefix,
    plan_sk, role_value, stop_sk, string, trip_pk,
};
use super::{
    CONDITIONAL_FAILURE, DynamoUserRepo, ENTITY_TYPE, PK, SK, USER_ID, user_partition_key,
};

mod access;
mod application;
mod operations;
mod records;

#[async_trait]
impl ProposalRepo for DynamoUserRepo {
    async fn list_proposals(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Proposal>, ProposalRepoError> {
        operations::list_proposals(self, trip_id, actor).await
    }

    async fn create_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal: Proposal,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError> {
        operations::create_proposal(self, trip_id, actor, proposal, application_ids).await
    }

    async fn approve_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        applied_at: &str,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError> {
        operations::approve_proposal(
            self,
            trip_id,
            actor,
            proposal_id,
            applied_at,
            application_ids,
        )
        .await
    }

    async fn reject_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        reason: &str,
    ) -> Result<Proposal, ProposalRepoError> {
        operations::reject_proposal(self, trip_id, actor, proposal_id, reason).await
    }
}

fn record_error(_: itinera_core::ports::trip::TripRepoError) -> ProposalRepoError {
    ProposalRepoError::CorruptData
}

fn application_error(error: ChangeApplicationError) -> ProposalRepoError {
    match error {
        ChangeApplicationError::CorruptData => ProposalRepoError::CorruptData,
        ChangeApplicationError::NotFound => ProposalRepoError::NotFound,
        ChangeApplicationError::InvalidChange => ProposalRepoError::InvalidChange,
    }
}

#[cfg(test)]
mod tests;
