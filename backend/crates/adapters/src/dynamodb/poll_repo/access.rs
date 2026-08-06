use std::collections::{HashMap, HashSet};

use aws_sdk_dynamodb::types::{AttributeValue, ConditionCheck, Delete};
use itinera_core::{
    domain::{
        trip::{TripMember, TripRole},
        user::UserId,
    },
    ports::poll::PollRepoError,
};

use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE, REVISION, USER_ID,
    primitives::item_key,
    trip_repo::records::{
        CURRENT_PLAN_ID, CURRENT_PLAN_VERSION, DATA, GSI1PK, GSI1SK, LEADER_COUNT, MEMBER_COUNT,
        MEMBER_ENTITY, META_SK, ROLE, Stored, TRIP_ENTITY, TripMeta, decode_record, member_sk,
        number_u64, role_value, string, trip_pk,
    },
    user_partition_key,
};

use super::record_error;

pub(super) const POLL_PAGE_SIZE: i32 = 100;
pub(super) const MAX_POLL_RECORDS: usize = 1_000;
pub(super) const MAX_POLL_BYTES: usize = 4 * 1_024 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequiredPollRole {
    Any,
    Editor,
    Leader,
}

pub(super) struct Loaded<T> {
    pub(super) value: T,
    pub(super) revision: u64,
}

impl DynamoUserRepo {
    pub(super) async fn poll_get(
        &self,
        partition_key: &str,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, PollRepoError> {
        let output = self
            .consistent_get(partition_key, sort_key)
            .send()
            .await
            .map_err(|_| PollRepoError::Unavailable)?;
        Ok(output.item)
    }

    pub(super) async fn poll_query(
        &self,
        partition_key: &str,
        prefix: &str,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, PollRepoError> {
        let mut items = Vec::new();
        let mut bytes = 0_usize;
        let mut cursor = None;
        loop {
            let output = self
                .partition_prefix_query(partition_key, prefix)
                .limit(POLL_PAGE_SIZE)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|_| PollRepoError::Unavailable)?;
            let next = output
                .last_evaluated_key()
                .filter(|key| !key.is_empty())
                .cloned();
            let page = output.items.unwrap_or_default();
            if page.len() > MAX_POLL_RECORDS.saturating_sub(items.len()) {
                return Err(PollRepoError::SafetyLimitExceeded);
            }
            for item in &page {
                bytes = bytes
                    .checked_add(string(item, DATA).map_err(record_error)?.len())
                    .ok_or(PollRepoError::SafetyLimitExceeded)?;
                if bytes > MAX_POLL_BYTES {
                    return Err(PollRepoError::SafetyLimitExceeded);
                }
            }
            items.extend(page);
            let Some(next) = next else {
                break;
            };
            cursor = Some(next);
        }
        Ok(items)
    }

    pub(super) async fn poll_authorize(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredPollRole,
    ) -> Result<TripRole, PollRepoError> {
        let pk = trip_pk(trip_id);
        let sk = member_sk(actor);
        let item = self
            .poll_get(&pk, &sk)
            .await?
            .ok_or(PollRepoError::NotFound)?;
        let stored: Stored<TripMember> =
            decode_record(&item, &pk, &sk, MEMBER_ENTITY).map_err(record_error)?;
        if stored.revision == 0
            || stored.value.user_id != actor.0
            || string(&item, USER_ID).map_err(record_error)? != actor.0
            || string(&item, ROLE).map_err(record_error)? != role_value(stored.value.role)
            || string(&item, GSI1PK).map_err(record_error)? != user_partition_key(actor)
            || string(&item, GSI1SK).map_err(record_error)? != format!("TRIP#{trip_id}")
        {
            return Err(PollRepoError::CorruptData);
        }
        let allowed = match required {
            RequiredPollRole::Any => true,
            RequiredPollRole::Editor => stored.value.role.can_edit(),
            RequiredPollRole::Leader => stored.value.role == TripRole::Leader,
        };
        if !allowed {
            return Err(PollRepoError::Forbidden);
        }
        Ok(stored.value.role)
    }

    pub(super) async fn poll_trip_meta(
        &self,
        trip_id: &str,
    ) -> Result<Loaded<TripMeta>, PollRepoError> {
        let pk = trip_pk(trip_id);
        let item = self
            .poll_get(&pk, META_SK)
            .await?
            .ok_or(PollRepoError::CorruptData)?;
        let stored: Stored<TripMeta> =
            decode_record(&item, &pk, META_SK, TRIP_ENTITY).map_err(record_error)?;
        let current_id_matches = match &stored.value.current_plan_id {
            Some(id) => string(&item, CURRENT_PLAN_ID).is_ok_and(|value| value == *id),
            None => !item.contains_key(CURRENT_PLAN_ID),
        };
        let current_version_matches = match stored.value.current_plan_version {
            Some(version) => number_u64(&item, CURRENT_PLAN_VERSION) == Ok(version.into()),
            None => !item.contains_key(CURRENT_PLAN_VERSION),
        };
        if stored.revision == 0
            || stored.value.id != trip_id
            || stored.value.member_count == 0
            || stored.value.leader_count == 0
            || number_u64(&item, MEMBER_COUNT) != Ok(stored.value.member_count.into())
            || number_u64(&item, LEADER_COUNT) != Ok(stored.value.leader_count.into())
            || !current_id_matches
            || !current_version_matches
        {
            return Err(PollRepoError::CorruptData);
        }
        Ok(Loaded {
            value: stored.value,
            revision: stored.revision,
        })
    }

    pub(super) async fn eligible_voter_count(
        &self,
        trip_id: &str,
        expected_meta_revision: u64,
        expected_member_count: u32,
    ) -> Result<u32, PollRepoError> {
        let pk = trip_pk(trip_id);
        let items = self.poll_query(&pk, "MEMBER#").await?;
        if items.len() != expected_member_count as usize {
            let latest = self.poll_trip_meta(trip_id).await?;
            return if latest.revision != expected_meta_revision
                || latest.value.member_count != expected_member_count
            {
                Err(PollRepoError::Conflict)
            } else {
                Err(PollRepoError::CorruptData)
            };
        }
        let mut users = HashSet::new();
        let mut eligible = 0_u32;
        for item in items {
            let sk = string(&item, crate::dynamodb::SK).map_err(record_error)?;
            let stored: Stored<TripMember> =
                decode_record(&item, &pk, &sk, MEMBER_ENTITY).map_err(record_error)?;
            let user = UserId(stored.value.user_id.clone());
            if stored.revision == 0
                || sk != member_sk(&user)
                || !users.insert(stored.value.user_id.clone())
                || string(&item, USER_ID).map_err(record_error)? != stored.value.user_id
                || string(&item, ROLE).map_err(record_error)? != role_value(stored.value.role)
                || string(&item, GSI1PK).map_err(record_error)? != user_partition_key(&user)
                || string(&item, GSI1SK).map_err(record_error)? != format!("TRIP#{trip_id}")
            {
                return Err(PollRepoError::CorruptData);
            }
            if stored.value.role.can_edit() {
                eligible = eligible.checked_add(1).ok_or(PollRepoError::CorruptData)?;
            }
        }
        if eligible == 0 {
            return Err(PollRepoError::CorruptData);
        }
        Ok(eligible)
    }

    pub(super) fn poll_membership_condition(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredPollRole,
    ) -> ConditionCheck {
        let expression = match required {
            RequiredPollRole::Any => "#entity = :member",
            RequiredPollRole::Editor => {
                "#entity = :member AND (#role = :leader OR #role = :member_role)"
            }
            RequiredPollRole::Leader => "#entity = :member AND #role = :leader",
        };
        let mut builder = ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(trip_pk(trip_id), member_sk(actor))))
            .condition_expression(expression)
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()));
        if required != RequiredPollRole::Any {
            builder = builder
                .expression_attribute_names("#role", ROLE)
                .expression_attribute_values(":leader", AttributeValue::S("leader".into()));
        }
        if required == RequiredPollRole::Editor {
            builder = builder
                .expression_attribute_values(":member_role", AttributeValue::S("member".into()));
        }
        builder
            .build()
            .expect("poll membership condition is complete")
    }

    pub(super) fn trip_meta_revision_condition(
        &self,
        trip_id: &str,
        revision: u64,
        member_count: u32,
    ) -> ConditionCheck {
        ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(trip_pk(trip_id), META_SK)))
            .condition_expression(
                "#entity = :trip AND #revision = :revision AND #member_count = :member_count",
            )
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#revision", REVISION)
            .expression_attribute_names("#member_count", MEMBER_COUNT)
            .expression_attribute_values(":trip", AttributeValue::S(TRIP_ENTITY.into()))
            .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
            .expression_attribute_values(
                ":member_count",
                AttributeValue::N(member_count.to_string()),
            )
            .build()
            .expect("trip metadata condition is complete")
    }

    pub(super) fn ballot_delete(
        &self,
        trip_id: &str,
        poll_id: &str,
        user_id: &str,
        revision: u64,
    ) -> Delete {
        Delete::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(
                trip_pk(trip_id),
                super::records::ballot_sk(poll_id, user_id),
            )))
            .condition_expression("#entity = :ballot AND #revision = :revision")
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#revision", REVISION)
            .expression_attribute_values(
                ":ballot",
                AttributeValue::S(super::records::BALLOT_ENTITY.into()),
            )
            .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
            .build()
            .expect("ballot delete is complete")
    }
}
