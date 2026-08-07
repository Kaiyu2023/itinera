use std::collections::{HashMap, HashSet};

use aws_sdk_dynamodb::types::{AttributeValue, ConditionCheck};
use itinera_core::{
    domain::{
        trip::{TripMember, TripRole},
        user::UserId,
    },
    ports::notice::NoticeRepoError,
    services::notices::MAX_CHECKLIST_COMPLETIONS,
};

use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE, USER_ID,
    primitives::{encoded_item_bytes, item_key},
    trip_repo::records::{
        DATA, GSI1PK, GSI1SK, MEMBER_ENTITY, META_SK, ROLE, Stored, TRIP_ENTITY, decode_record,
        decode_trip_meta, member_sk, role_value, string, trip_pk,
    },
    user_partition_key,
};

use super::record_error;

pub(super) const NOTICE_PAGE_SIZE: i32 = 100;
pub(super) const MAX_NOTICE_BYTES: usize = 4 * 1_024 * 1_024;
const MEMBER_PREFIX: &str = "MEMBER#";

pub(in crate::dynamodb) struct NoticeMembershipSnapshot {
    pub(in crate::dynamodb) member_ids: HashSet<String>,
    meta_revision: u64,
    meta_data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequiredNoticeRole {
    Any,
    Editor,
    Leader,
}

impl DynamoUserRepo {
    pub(super) async fn notice_get(
        &self,
        partition_key: &str,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, NoticeRepoError> {
        self.consistent_get(partition_key, sort_key)
            .send()
            .await
            .map(|output| output.item)
            .map_err(|_| NoticeRepoError::Unavailable)
    }

    pub(super) async fn notice_query(
        &self,
        partition_key: &str,
        prefix: &str,
        max_records: usize,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, NoticeRepoError> {
        let mut items = Vec::new();
        let mut bytes = 0_usize;
        let mut cursor = None;
        loop {
            let output = self
                .partition_prefix_query(partition_key, prefix)
                .limit(NOTICE_PAGE_SIZE)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|_| NoticeRepoError::Unavailable)?;
            let next = output
                .last_evaluated_key()
                .filter(|key| !key.is_empty())
                .cloned();
            let page = output.items.unwrap_or_default();
            if page.len() > max_records.saturating_sub(items.len()) {
                return Err(NoticeRepoError::SafetyLimitExceeded);
            }
            for item in &page {
                bytes = bytes
                    .checked_add(
                        encoded_item_bytes(item).ok_or(NoticeRepoError::SafetyLimitExceeded)?,
                    )
                    .ok_or(NoticeRepoError::SafetyLimitExceeded)?;
                if bytes > MAX_NOTICE_BYTES {
                    return Err(NoticeRepoError::SafetyLimitExceeded);
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

    pub(super) async fn notice_authorize(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredNoticeRole,
    ) -> Result<TripRole, NoticeRepoError> {
        let item = self
            .notice_get(&trip_pk(trip_id), &member_sk(actor))
            .await?
            .ok_or(NoticeRepoError::NotFound)?;
        let member = decode_member(&item, trip_id, actor)?;
        let permitted = match required {
            RequiredNoticeRole::Any => true,
            RequiredNoticeRole::Editor => member.role.can_edit(),
            RequiredNoticeRole::Leader => member.role == TripRole::Leader,
        };
        if !permitted {
            return Err(NoticeRepoError::Forbidden);
        }
        Ok(member.role)
    }

    pub(in crate::dynamodb) async fn notice_membership_snapshot(
        &self,
        trip_id: &str,
    ) -> Result<NoticeMembershipSnapshot, NoticeRepoError> {
        let partition_key = trip_pk(trip_id);
        for _ in 0..2 {
            let first_item = self
                .notice_get(&partition_key, META_SK)
                .await?
                .ok_or(NoticeRepoError::NotFound)?;
            let first = decode_trip_meta(&first_item, trip_id).map_err(record_error)?;
            if first.value.member_count as usize > MAX_CHECKLIST_COMPLETIONS {
                return Err(NoticeRepoError::SafetyLimitExceeded);
            }
            let members = self
                .notice_query(&partition_key, MEMBER_PREFIX, MAX_CHECKLIST_COMPLETIONS)
                .await?;
            let second_item = self
                .notice_get(&partition_key, META_SK)
                .await?
                .ok_or(NoticeRepoError::CorruptData)?;
            let second = decode_trip_meta(&second_item, trip_id).map_err(record_error)?;
            let first_data = string(&first_item, DATA).map_err(record_error)?;
            let second_data = string(&second_item, DATA).map_err(record_error)?;
            if first.revision != second.revision || first_data != second_data {
                continue;
            }

            let mut member_ids = HashSet::with_capacity(members.len());
            let mut leader_count = 0_usize;
            for item in members {
                let sort_key = string(&item, crate::dynamodb::SK).map_err(record_error)?;
                let user_id = sort_key
                    .strip_prefix(MEMBER_PREFIX)
                    .filter(|user_id| !user_id.is_empty())
                    .ok_or(NoticeRepoError::CorruptData)?;
                let user_id = UserId(user_id.to_string());
                let member = decode_member(&item, trip_id, &user_id)?;
                if !member_ids.insert(member.user_id) {
                    return Err(NoticeRepoError::CorruptData);
                }
                if member.role == TripRole::Leader {
                    leader_count += 1;
                }
            }
            if member_ids.len() != first.value.member_count as usize
                || leader_count != first.value.leader_count as usize
            {
                return Err(NoticeRepoError::CorruptData);
            }
            return Ok(NoticeMembershipSnapshot {
                member_ids,
                meta_revision: first.revision,
                meta_data: first_data,
            });
        }
        Err(NoticeRepoError::Conflict)
    }

    pub(in crate::dynamodb) fn notice_membership_snapshot_condition(
        &self,
        trip_id: &str,
        snapshot: &NoticeMembershipSnapshot,
    ) -> ConditionCheck {
        self.entity_revision_data_condition(
            trip_pk(trip_id),
            META_SK,
            TRIP_ENTITY,
            snapshot.meta_revision,
            &snapshot.meta_data,
        )
    }

    pub(super) fn notice_membership_condition(
        &self,
        trip_id: &str,
        user_id: &UserId,
        required: RequiredNoticeRole,
    ) -> ConditionCheck {
        let expression = match required {
            RequiredNoticeRole::Any => "#entity = :member",
            RequiredNoticeRole::Editor => {
                "#entity = :member AND (#role = :leader OR #role = :member_role)"
            }
            RequiredNoticeRole::Leader => "#entity = :member AND #role = :leader",
        };
        let mut builder = ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(trip_pk(trip_id), member_sk(user_id))))
            .condition_expression(expression)
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()));
        if required != RequiredNoticeRole::Any {
            builder = builder
                .expression_attribute_names("#role", ROLE)
                .expression_attribute_values(":leader", AttributeValue::S("leader".into()));
        }
        if required == RequiredNoticeRole::Editor {
            builder = builder
                .expression_attribute_values(":member_role", AttributeValue::S("member".into()));
        }
        builder
            .build()
            .expect("notice membership condition is complete")
    }
}

fn decode_member(
    item: &HashMap<String, AttributeValue>,
    trip_id: &str,
    expected_user_id: &UserId,
) -> Result<TripMember, NoticeRepoError> {
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
        return Err(NoticeRepoError::CorruptData);
    }
    Ok(stored.value)
}
