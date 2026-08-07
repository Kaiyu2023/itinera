use std::collections::{HashMap, HashSet};

use aws_sdk_dynamodb::types::{AttributeValue, ConditionCheck};
use itinera_core::{
    domain::{
        trip::{TripMember, TripRole},
        user::UserId,
    },
    ports::ledger::LedgerRepoError,
    services::ledger::validate_currency,
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

pub(super) const LEDGER_PAGE_SIZE: i32 = 100;
pub(super) const MAX_LEDGER_ROWS: usize = 1_000;
pub(super) const MAX_LEDGER_MEMBERS: usize = 1_000;
pub(super) const MAX_LEDGER_BYTES: usize = 4 * 1_024 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequiredLedgerRole {
    Any,
    Editor,
}

#[derive(Debug, Clone)]
pub(super) struct LoadedTripMeta {
    pub(super) value: TripMeta,
    pub(super) revision: u64,
}

#[derive(Debug, Default)]
pub(super) struct LedgerReadBudget {
    bytes: usize,
}

impl LedgerReadBudget {
    fn include(&mut self, item: &HashMap<String, AttributeValue>) -> Result<(), LedgerRepoError> {
        let bytes = string(item, DATA).map_err(record_error)?.len();
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(LedgerRepoError::SafetyLimitExceeded)?;
        if self.bytes > MAX_LEDGER_BYTES {
            return Err(LedgerRepoError::SafetyLimitExceeded);
        }
        Ok(())
    }
}

impl DynamoUserRepo {
    pub(super) async fn ledger_get(
        &self,
        partition_key: &str,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, LedgerRepoError> {
        let output = self
            .consistent_get(partition_key, sort_key)
            .send()
            .await
            .map_err(|_| LedgerRepoError::Unavailable)?;
        Ok(output.item)
    }

    pub(super) async fn ledger_query(
        &self,
        partition_key: &str,
        prefix: &str,
        max_records: usize,
        budget: &mut LedgerReadBudget,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, LedgerRepoError> {
        let mut items = Vec::new();
        let mut cursor = None;
        loop {
            let output = self
                .partition_prefix_query(partition_key, prefix)
                .limit(LEDGER_PAGE_SIZE)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|_| LedgerRepoError::Unavailable)?;
            let next = output
                .last_evaluated_key()
                .filter(|key| !key.is_empty())
                .cloned();
            let page = output.items.unwrap_or_default();
            if page.len() > max_records.saturating_sub(items.len()) {
                return Err(LedgerRepoError::SafetyLimitExceeded);
            }
            for item in &page {
                budget.include(item)?;
            }
            items.extend(page);
            let Some(next) = next else {
                break;
            };
            cursor = Some(next);
        }
        Ok(items)
    }

    pub(super) async fn ledger_authorize(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredLedgerRole,
    ) -> Result<TripRole, LedgerRepoError> {
        let pk = trip_pk(trip_id);
        let sk = member_sk(actor);
        let item = self
            .ledger_get(&pk, &sk)
            .await?
            .ok_or(LedgerRepoError::NotFound)?;
        let member = decode_member(&item, trip_id, actor)?;
        if required == RequiredLedgerRole::Editor && !member.role.can_edit() {
            return Err(LedgerRepoError::Forbidden);
        }
        Ok(member.role)
    }

    pub(super) async fn ledger_trip_meta(
        &self,
        trip_id: &str,
    ) -> Result<LoadedTripMeta, LedgerRepoError> {
        let pk = trip_pk(trip_id);
        let item = self
            .ledger_get(&pk, META_SK)
            .await?
            .ok_or(LedgerRepoError::CorruptData)?;
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
            || stored.value.leader_count > stored.value.member_count
            || number_u64(&item, MEMBER_COUNT) != Ok(stored.value.member_count.into())
            || number_u64(&item, LEADER_COUNT) != Ok(stored.value.leader_count.into())
            || validate_currency(&stored.value.base_currency).is_err()
            || !current_id_matches
            || !current_version_matches
            || stored.value.current_plan_id.is_some() != stored.value.current_plan_version.is_some()
        {
            return Err(LedgerRepoError::CorruptData);
        }
        Ok(LoadedTripMeta {
            value: stored.value,
            revision: stored.revision,
        })
    }

    pub(super) async fn ledger_current_members(
        &self,
        trip_id: &str,
        expected_count: u32,
        budget: &mut LedgerReadBudget,
    ) -> Result<Vec<String>, LedgerRepoError> {
        if expected_count == 0 || expected_count as usize > MAX_LEDGER_MEMBERS {
            return Err(LedgerRepoError::SafetyLimitExceeded);
        }
        let pk = trip_pk(trip_id);
        let items = self
            .ledger_query(&pk, "MEMBER#", MAX_LEDGER_MEMBERS, budget)
            .await?;
        if items.len() != expected_count as usize {
            return Err(LedgerRepoError::CorruptData);
        }
        let mut ids = HashSet::new();
        for item in items {
            let user_id = string(&item, USER_ID).map_err(record_error)?;
            let actor = UserId(user_id.clone());
            let member = decode_member(&item, trip_id, &actor)?;
            if !ids.insert(member.user_id) {
                return Err(LedgerRepoError::CorruptData);
            }
        }
        let mut ids = ids.into_iter().collect::<Vec<_>>();
        ids.sort();
        Ok(ids)
    }

    pub(super) async fn ledger_require_members(
        &self,
        trip_id: &str,
        user_ids: &HashSet<String>,
    ) -> Result<(), LedgerRepoError> {
        for user_id in user_ids {
            let user_id = UserId(user_id.clone());
            let pk = trip_pk(trip_id);
            let sk = member_sk(&user_id);
            let Some(item) = self.ledger_get(&pk, &sk).await? else {
                return Err(LedgerRepoError::Conflict);
            };
            decode_member(&item, trip_id, &user_id)?;
        }
        Ok(())
    }

    pub(super) fn ledger_membership_condition(
        &self,
        trip_id: &str,
        user_id: &UserId,
        required: RequiredLedgerRole,
    ) -> ConditionCheck {
        let mut builder = ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(trip_pk(trip_id), member_sk(user_id))))
            .condition_expression(match required {
                RequiredLedgerRole::Any => "#entity = :member",
                RequiredLedgerRole::Editor => {
                    "#entity = :member AND (#role = :leader OR #role = :member_role)"
                }
            })
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()));
        if required == RequiredLedgerRole::Editor {
            builder = builder
                .expression_attribute_names("#role", ROLE)
                .expression_attribute_values(":leader", AttributeValue::S("leader".into()))
                .expression_attribute_values(":member_role", AttributeValue::S("member".into()));
        }
        builder
            .build()
            .expect("ledger membership condition is complete")
    }

    pub(super) fn ledger_trip_condition(
        &self,
        trip_id: &str,
        meta: &LoadedTripMeta,
    ) -> ConditionCheck {
        let mut expression = "#entity = :trip AND #revision = :revision".to_string();
        let mut builder = ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(trip_pk(trip_id), META_SK)))
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#revision", REVISION)
            .expression_attribute_values(":trip", AttributeValue::S(TRIP_ENTITY.into()))
            .expression_attribute_values(":revision", AttributeValue::N(meta.revision.to_string()));
        match (
            meta.value.current_plan_id.as_deref(),
            meta.value.current_plan_version,
        ) {
            (Some(plan_id), Some(version)) => {
                expression.push_str(" AND #plan_id = :plan_id AND #plan_version = :plan_version");
                builder = builder
                    .expression_attribute_names("#plan_id", CURRENT_PLAN_ID)
                    .expression_attribute_names("#plan_version", CURRENT_PLAN_VERSION)
                    .expression_attribute_values(":plan_id", AttributeValue::S(plan_id.into()))
                    .expression_attribute_values(
                        ":plan_version",
                        AttributeValue::N(version.to_string()),
                    );
            }
            (None, None) => {
                expression.push_str(
                    " AND attribute_not_exists(#plan_id) AND attribute_not_exists(#plan_version)",
                );
                builder = builder
                    .expression_attribute_names("#plan_id", CURRENT_PLAN_ID)
                    .expression_attribute_names("#plan_version", CURRENT_PLAN_VERSION);
            }
            _ => unreachable!("trip metadata validation requires paired current-plan fields"),
        }
        builder
            .condition_expression(expression)
            .build()
            .expect("ledger trip condition is complete")
    }
}

fn decode_member(
    item: &HashMap<String, AttributeValue>,
    trip_id: &str,
    expected_user_id: &UserId,
) -> Result<TripMember, LedgerRepoError> {
    let pk = trip_pk(trip_id);
    let sk = member_sk(expected_user_id);
    let stored: Stored<TripMember> =
        decode_record(item, &pk, &sk, MEMBER_ENTITY).map_err(record_error)?;
    if stored.revision == 0
        || stored.value.user_id != expected_user_id.0
        || string(item, USER_ID).map_err(record_error)? != expected_user_id.0
        || string(item, ROLE).map_err(record_error)? != role_value(stored.value.role)
        || string(item, GSI1PK).map_err(record_error)? != user_partition_key(expected_user_id)
        || string(item, GSI1SK).map_err(record_error)? != format!("TRIP#{trip_id}")
    {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(stored.value)
}
