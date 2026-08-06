//! Strong direct-membership checks and shared history storage reads.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::{AttributeValue, ConditionCheck};
use itinera_core::{
    domain::{
        trip::{TripMember, TripRole},
        user::UserId,
    },
    ports::content_history::ContentHistoryRepoError,
};
use serde::de::DeserializeOwned;

use crate::dynamodb::trip_repo::records::{
    CURRENT_PLAN_ID, CURRENT_PLAN_VERSION, DATA, GSI1PK, GSI1SK, LEADER_COUNT, MEMBER_COUNT,
    MEMBER_ENTITY, META_SK, ROLE, Stored, TRIP_ENTITY, TripMeta, decode_record, member_sk,
    number_u64, role_value, string, trip_pk,
};
use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE, USER_ID, primitives::item_key, user_partition_key,
};

use super::record_error;

pub(super) const HISTORY_PAGE_SIZE: i32 = 100;
pub(super) const MAX_HISTORY_RECORDS: usize = 1_000;
pub(super) const MAX_HISTORY_BYTES: usize = 4 * 1_024 * 1_024;
pub(super) const MAX_HISTORY_RESPONSE_BYTES: usize = 4 * 1_024 * 1_024;
pub(super) const MAX_REVERT_PLAN_RECORDS: usize = 1_000;
pub(super) const MAX_REVERT_PLAN_BYTES: usize = 4 * 1_024 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequiredHistoryRole {
    Any,
    Editor,
}

pub(super) struct Loaded<T> {
    pub(super) value: T,
    pub(super) revision: u64,
    pub(super) sort_key: String,
    pub(super) raw_data: String,
    pub(super) encoded_bytes: usize,
}

pub(super) fn decode_loaded<T: DeserializeOwned>(
    item: &HashMap<String, AttributeValue>,
    expected_pk: &str,
    expected_sk: &str,
    expected_entity: &str,
) -> Result<Loaded<T>, ContentHistoryRepoError> {
    let raw_data = string(item, DATA).map_err(record_error)?;
    let Stored {
        value,
        revision,
        sort_key,
    } = decode_record(item, expected_pk, expected_sk, expected_entity).map_err(record_error)?;
    if revision == 0 {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    Ok(Loaded {
        value,
        revision,
        sort_key,
        raw_data,
        encoded_bytes: encoded_item_bytes(item)?,
    })
}

pub(super) fn encoded_item_bytes(
    item: &HashMap<String, AttributeValue>,
) -> Result<usize, ContentHistoryRepoError> {
    let mut bytes = 0_usize;
    for (name, value) in item {
        bytes = bytes
            .checked_add(name.len())
            .and_then(|total| attribute_value_bytes(value).and_then(|size| total.checked_add(size)))
            .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
    }
    Ok(bytes)
}

fn attribute_value_bytes(value: &AttributeValue) -> Option<usize> {
    match value {
        AttributeValue::B(value) => Some(value.as_ref().len()),
        AttributeValue::Bool(_) | AttributeValue::Null(_) => Some(1),
        AttributeValue::Bs(values) => checked_sum(values.iter().map(|value| value.as_ref().len())),
        AttributeValue::L(values) => values.iter().try_fold(0_usize, |total, value| {
            total.checked_add(attribute_value_bytes(value)?)
        }),
        AttributeValue::M(values) => {
            let mut bytes = 0_usize;
            for (name, value) in values {
                bytes = bytes.checked_add(name.len())?;
                bytes = bytes.checked_add(attribute_value_bytes(value)?)?;
            }
            Some(bytes)
        }
        AttributeValue::N(value) | AttributeValue::S(value) => Some(value.len()),
        AttributeValue::Ns(values) | AttributeValue::Ss(values) => {
            checked_sum(values.iter().map(String::len))
        }
        _ => None,
    }
}

fn checked_sum(values: impl IntoIterator<Item = usize>) -> Option<usize> {
    values.into_iter().try_fold(0_usize, usize::checked_add)
}

impl DynamoUserRepo {
    pub(super) async fn history_get(
        &self,
        partition_key: &str,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, ContentHistoryRepoError> {
        let output = self
            .consistent_get(partition_key, sort_key)
            .send()
            .await
            .map_err(|_| ContentHistoryRepoError::Unavailable)?;
        Ok(output.item)
    }

    pub(super) async fn history_query(
        &self,
        partition_key: &str,
        prefix: &str,
        page_size: i32,
        newest_first: bool,
        max_items: usize,
        max_bytes: usize,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, ContentHistoryRepoError> {
        let mut items = Vec::new();
        let mut encoded_bytes = 0_usize;
        let mut cursor = None;
        loop {
            let output = self
                .partition_prefix_query(partition_key, prefix)
                .scan_index_forward(!newest_first)
                .limit(page_size)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|_| ContentHistoryRepoError::Unavailable)?;
            let next = output
                .last_evaluated_key()
                .filter(|key| !key.is_empty())
                .cloned();
            let page = output.items.unwrap_or_default();
            if page.len() > max_items.saturating_sub(items.len()) {
                return Err(ContentHistoryRepoError::SafetyLimitExceeded);
            }
            for item in &page {
                encoded_bytes = encoded_bytes
                    .checked_add(encoded_item_bytes(item)?)
                    .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
                if encoded_bytes > max_bytes {
                    return Err(ContentHistoryRepoError::SafetyLimitExceeded);
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

    pub(super) async fn history_authorize(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredHistoryRole,
    ) -> Result<TripRole, ContentHistoryRepoError> {
        let pk = trip_pk(trip_id);
        let sk = member_sk(actor);
        let item = self
            .history_get(&pk, &sk)
            .await?
            // A direct membership miss deliberately does not reveal whether
            // the trip or a requested edit exists.
            .ok_or(ContentHistoryRepoError::NotFound)?;
        let member: Loaded<TripMember> = decode_loaded(&item, &pk, &sk, MEMBER_ENTITY)?;
        if member.value.user_id != actor.0
            || string(&item, USER_ID).map_err(record_error)? != actor.0
            || string(&item, ROLE).map_err(record_error)? != role_value(member.value.role)
            || string(&item, GSI1PK).map_err(record_error)? != user_partition_key(actor)
            || string(&item, GSI1SK).map_err(record_error)? != format!("TRIP#{trip_id}")
        {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        if required == RequiredHistoryRole::Editor && !member.value.role.can_edit() {
            return Err(ContentHistoryRepoError::Forbidden);
        }
        Ok(member.value.role)
    }

    pub(super) async fn history_trip_meta(
        &self,
        trip_id: &str,
    ) -> Result<Loaded<TripMeta>, ContentHistoryRepoError> {
        let pk = trip_pk(trip_id);
        let item = self
            .history_get(&pk, META_SK)
            .await?
            .ok_or(ContentHistoryRepoError::CorruptData)?;
        let meta: Loaded<TripMeta> = decode_loaded(&item, &pk, META_SK, TRIP_ENTITY)?;
        let current_id_matches = match &meta.value.current_plan_id {
            Some(id) => string(&item, CURRENT_PLAN_ID).is_ok_and(|stored| stored == *id),
            None => !item.contains_key(CURRENT_PLAN_ID),
        };
        let current_version_matches = match meta.value.current_plan_version {
            Some(version) => number_u64(&item, CURRENT_PLAN_VERSION) == Ok(version.into()),
            None => !item.contains_key(CURRENT_PLAN_VERSION),
        };
        if meta.value.id != trip_id
            || meta.value.member_count == 0
            || meta.value.leader_count == 0
            || number_u64(&item, MEMBER_COUNT) != Ok(meta.value.member_count.into())
            || number_u64(&item, LEADER_COUNT) != Ok(meta.value.leader_count.into())
            || !current_id_matches
            || !current_version_matches
        {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        Ok(meta)
    }
    pub(super) fn editor_membership_condition(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> ConditionCheck {
        ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(trip_pk(trip_id), member_sk(actor))))
            .condition_expression("#entity = :member AND (#role = :leader OR #role = :member_role)")
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#role", ROLE)
            .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()))
            .expression_attribute_values(":leader", AttributeValue::S("leader".into()))
            .expression_attribute_values(":member_role", AttributeValue::S("member".into()))
            .build()
            .expect("editor membership condition is complete")
    }
}
