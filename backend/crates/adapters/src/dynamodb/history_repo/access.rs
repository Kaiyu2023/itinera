//! Strong direct-membership checks and shared history storage reads.

use super::*;

pub(super) const HISTORY_PAGE_SIZE: i32 = 100;
pub(super) const MAX_HISTORY_RECORDS: usize = 1_000;
pub(super) const MAX_REVERT_PLAN_RECORDS: usize = 1_000;

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
    })
}

impl DynamoUserRepo {
    pub(super) async fn history_get(
        &self,
        partition_key: &str,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, ContentHistoryRepoError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key(PK, AttributeValue::S(partition_key.to_string()))
            .key(SK, AttributeValue::S(sort_key.to_string()))
            .consistent_read(true)
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
    ) -> Result<Vec<HashMap<String, AttributeValue>>, ContentHistoryRepoError> {
        let mut items = Vec::new();
        let mut cursor = None;
        loop {
            let output = self
                .client
                .query()
                .table_name(&self.table_name)
                .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
                .expression_attribute_names("#pk", PK)
                .expression_attribute_names("#sk", SK)
                .expression_attribute_values(":pk", AttributeValue::S(partition_key.to_string()))
                .expression_attribute_values(":prefix", AttributeValue::S(prefix.to_string()))
                .consistent_read(true)
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
}

pub(super) fn editor_membership_condition(
    table_name: &str,
    trip_id: &str,
    actor: &UserId,
) -> ConditionCheck {
    ConditionCheck::builder()
        .table_name(table_name)
        .key(PK, AttributeValue::S(trip_pk(trip_id)))
        .key(SK, AttributeValue::S(member_sk(actor)))
        .condition_expression("#entity = :member AND (#role = :leader OR #role = :member_role)")
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#role", ROLE)
        .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()))
        .expression_attribute_values(":leader", AttributeValue::S("leader".into()))
        .expression_attribute_values(":member_role", AttributeValue::S("member".into()))
        .build()
        .expect("editor membership condition is complete")
}
