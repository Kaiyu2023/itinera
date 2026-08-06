use std::collections::HashMap;

use aws_sdk_dynamodb::types::{AttributeValue, ConditionCheck};
use itinera_core::{
    domain::{
        trip::{TripMember, TripRole},
        user::UserId,
    },
    ports::discussion::DiscussionRepoError,
};

use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE, REVISION, USER_ID,
    primitives::item_key,
    trip_repo::records::{
        CURRENT_PLAN_ID, CURRENT_PLAN_VERSION, DATA, GSI1PK, GSI1SK, MEMBER_ENTITY, META_SK, ROLE,
        Stored, TRIP_ENTITY, TripMeta, decode_record, member_sk, number_u64, role_value, string,
        trip_pk,
    },
    user_partition_key,
};

use super::record_error;

pub(super) const DISCUSSION_PAGE_SIZE: i32 = 100;
pub(super) const MAX_THREADS: usize = 1_000;
pub(super) const MAX_COMMENTS_PER_THREAD: usize = 1_000;
pub(super) const MAX_DISCUSSION_BYTES: usize = 4 * 1_024 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequiredDiscussionRole {
    Any,
    Editor,
}

pub(super) struct LoadedTripMeta {
    pub(super) value: TripMeta,
    pub(super) revision: u64,
}

impl DynamoUserRepo {
    pub(super) async fn discussion_get(
        &self,
        partition_key: &str,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, DiscussionRepoError> {
        let output = self
            .consistent_get(partition_key, sort_key)
            .send()
            .await
            .map_err(|_| DiscussionRepoError::Unavailable)?;
        Ok(output.item)
    }

    pub(super) async fn discussion_query(
        &self,
        partition_key: &str,
        prefix: &str,
        max_records: usize,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, DiscussionRepoError> {
        let mut items = Vec::new();
        let mut bytes = 0_usize;
        let mut cursor = None;
        loop {
            let output = self
                .partition_prefix_query(partition_key, prefix)
                .limit(DISCUSSION_PAGE_SIZE)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|_| DiscussionRepoError::Unavailable)?;
            let next = output
                .last_evaluated_key()
                .filter(|key| !key.is_empty())
                .cloned();
            let page = output.items.unwrap_or_default();
            if page.len() > max_records.saturating_sub(items.len()) {
                return Err(DiscussionRepoError::SafetyLimitExceeded);
            }
            for item in &page {
                bytes = bytes
                    .checked_add(string(item, DATA).map_err(record_error)?.len())
                    .ok_or(DiscussionRepoError::SafetyLimitExceeded)?;
                if bytes > MAX_DISCUSSION_BYTES {
                    return Err(DiscussionRepoError::SafetyLimitExceeded);
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

    pub(super) async fn discussion_authorize(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredDiscussionRole,
    ) -> Result<TripRole, DiscussionRepoError> {
        let pk = trip_pk(trip_id);
        let sk = member_sk(actor);
        let item = self
            .discussion_get(&pk, &sk)
            .await?
            .ok_or(DiscussionRepoError::NotFound)?;
        let stored: Stored<TripMember> =
            decode_record(&item, &pk, &sk, MEMBER_ENTITY).map_err(record_error)?;
        if stored.revision == 0
            || stored.value.user_id != actor.0
            || string(&item, USER_ID).map_err(record_error)? != actor.0
            || string(&item, ROLE).map_err(record_error)? != role_value(stored.value.role)
            || string(&item, GSI1PK).map_err(record_error)? != user_partition_key(actor)
            || string(&item, GSI1SK).map_err(record_error)? != format!("TRIP#{trip_id}")
        {
            return Err(DiscussionRepoError::CorruptData);
        }
        if required == RequiredDiscussionRole::Editor && !stored.value.role.can_edit() {
            return Err(DiscussionRepoError::Forbidden);
        }
        Ok(stored.value.role)
    }

    pub(super) async fn discussion_trip_meta(
        &self,
        trip_id: &str,
    ) -> Result<LoadedTripMeta, DiscussionRepoError> {
        let pk = trip_pk(trip_id);
        let item = self
            .discussion_get(&pk, META_SK)
            .await?
            .ok_or(DiscussionRepoError::CorruptData)?;
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
            || !current_id_matches
            || !current_version_matches
        {
            return Err(DiscussionRepoError::CorruptData);
        }
        Ok(LoadedTripMeta {
            value: stored.value,
            revision: stored.revision,
        })
    }

    pub(super) fn discussion_membership_condition(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredDiscussionRole,
    ) -> ConditionCheck {
        let mut builder = ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(trip_pk(trip_id), member_sk(actor))))
            .condition_expression(match required {
                RequiredDiscussionRole::Any => "#entity = :member",
                RequiredDiscussionRole::Editor => {
                    "#entity = :member AND (#role = :leader OR #role = :member_role)"
                }
            })
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()));
        if required == RequiredDiscussionRole::Editor {
            builder = builder
                .expression_attribute_names("#role", ROLE)
                .expression_attribute_values(":leader", AttributeValue::S("leader".into()))
                .expression_attribute_values(":member_role", AttributeValue::S("member".into()));
        }
        builder
            .build()
            .expect("discussion membership condition is complete")
    }

    pub(super) fn trip_revision_condition(&self, trip_id: &str, revision: u64) -> ConditionCheck {
        self.entity_revision_condition(trip_pk(trip_id), META_SK, TRIP_ENTITY, revision)
    }

    pub(super) fn discussion_current_plan_condition(
        &self,
        trip_id: &str,
        revision: u64,
        plan_id: &str,
        version: u32,
    ) -> ConditionCheck {
        ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(trip_pk(trip_id), META_SK)))
            .condition_expression(
                "#entity = :trip AND #revision = :revision AND #plan_id = :plan_id AND #plan_version = :plan_version",
            )
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#revision", REVISION)
            .expression_attribute_names("#plan_id", CURRENT_PLAN_ID)
            .expression_attribute_names("#plan_version", CURRENT_PLAN_VERSION)
            .expression_attribute_values(":trip", AttributeValue::S(TRIP_ENTITY.into()))
            .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
            .expression_attribute_values(":plan_id", AttributeValue::S(plan_id.into()))
            .expression_attribute_values(":plan_version", AttributeValue::N(version.to_string()))
            .build()
            .expect("current plan condition is complete")
    }
}
