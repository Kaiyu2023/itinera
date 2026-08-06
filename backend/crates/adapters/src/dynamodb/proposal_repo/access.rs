//! Strong direct-membership reads and proposal transaction conditions.

use super::*;

pub(super) const PROPOSAL_PAGE_SIZE: i32 = 100;
pub(super) const MAX_PROPOSAL_RECORDS: usize = 1_000;
pub(super) const MAX_PROPOSAL_BYTES: usize = 4 * 1_024 * 1_024;
pub(super) const MAX_TRANSACTION_ACTIONS: usize = 100;
pub(super) const MAX_TRANSACTION_DATA_BYTES: usize = 3 * 1_024 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequiredProposalRole {
    Any,
    Editor,
    Leader,
}

pub(super) struct Loaded<T> {
    pub(super) value: T,
    pub(super) revision: u64,
    pub(super) sort_key: String,
}

pub(super) fn decode_loaded<T: DeserializeOwned>(
    item: &HashMap<String, AttributeValue>,
    expected_pk: &str,
    expected_sk: &str,
    expected_entity: &str,
) -> Result<Loaded<T>, ProposalRepoError> {
    string(item, DATA).map_err(record_error)?;
    let Stored {
        value,
        revision,
        sort_key,
    } = decode_record(item, expected_pk, expected_sk, expected_entity).map_err(record_error)?;
    if revision == 0 {
        return Err(ProposalRepoError::CorruptData);
    }
    Ok(Loaded {
        value,
        revision,
        sort_key,
    })
}

impl DynamoUserRepo {
    pub(super) async fn proposal_get(
        &self,
        partition_key: &str,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, ProposalRepoError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key(PK, AttributeValue::S(partition_key.to_string()))
            .key(SK, AttributeValue::S(sort_key.to_string()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| ProposalRepoError::Unavailable)?;
        Ok(output.item)
    }

    pub(super) async fn proposal_query(
        &self,
        partition_key: &str,
        prefix: &str,
        page_size: i32,
        max_items: usize,
        max_data_bytes: usize,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, ProposalRepoError> {
        let mut items = Vec::new();
        let mut data_bytes = 0_usize;
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
                .limit(page_size)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|_| ProposalRepoError::Unavailable)?;
            let next = output
                .last_evaluated_key()
                .filter(|key| !key.is_empty())
                .cloned();
            let page = output.items.unwrap_or_default();
            if page.len() > max_items.saturating_sub(items.len()) {
                return Err(ProposalRepoError::SafetyLimitExceeded);
            }
            for item in &page {
                data_bytes = data_bytes
                    .checked_add(string(item, DATA).map_err(record_error)?.len())
                    .ok_or(ProposalRepoError::SafetyLimitExceeded)?;
                if data_bytes > max_data_bytes {
                    return Err(ProposalRepoError::SafetyLimitExceeded);
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

    pub(super) async fn proposal_authorize(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredProposalRole,
    ) -> Result<TripRole, ProposalRepoError> {
        let pk = trip_pk(trip_id);
        let sk = member_sk(actor);
        let item = self
            .proposal_get(&pk, &sk)
            .await?
            .ok_or(ProposalRepoError::NotFound)?;
        let member: Loaded<TripMember> = decode_loaded(&item, &pk, &sk, MEMBER_ENTITY)?;
        if member.value.user_id != actor.0
            || string(&item, USER_ID).map_err(record_error)? != actor.0
            || string(&item, ROLE).map_err(record_error)? != role_value(member.value.role)
            || string(&item, GSI1PK).map_err(record_error)? != user_partition_key(actor)
            || string(&item, GSI1SK).map_err(record_error)? != format!("TRIP#{trip_id}")
        {
            return Err(ProposalRepoError::CorruptData);
        }
        let allowed = match required {
            RequiredProposalRole::Any => true,
            RequiredProposalRole::Editor => member.value.role.can_edit(),
            RequiredProposalRole::Leader => member.value.role == TripRole::Leader,
        };
        if !allowed {
            return Err(ProposalRepoError::Forbidden);
        }
        Ok(member.value.role)
    }

    pub(super) async fn proposal_trip_meta(
        &self,
        trip_id: &str,
    ) -> Result<Loaded<TripMeta>, ProposalRepoError> {
        let pk = trip_pk(trip_id);
        let item = self
            .proposal_get(&pk, META_SK)
            .await?
            .ok_or(ProposalRepoError::CorruptData)?;
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
            return Err(ProposalRepoError::CorruptData);
        }
        Ok(meta)
    }
}

pub(super) fn membership_condition(
    table_name: &str,
    trip_id: &str,
    actor: &UserId,
    required: RequiredProposalRole,
) -> ConditionCheck {
    let expression = match required {
        RequiredProposalRole::Any => "#entity = :member",
        RequiredProposalRole::Editor => {
            "#entity = :member AND (#role = :leader OR #role = :member_role)"
        }
        RequiredProposalRole::Leader => "#entity = :member AND #role = :leader",
    };
    let mut builder = ConditionCheck::builder()
        .table_name(table_name)
        .key(PK, AttributeValue::S(trip_pk(trip_id)))
        .key(SK, AttributeValue::S(member_sk(actor)))
        .condition_expression(expression)
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()));
    if required != RequiredProposalRole::Any {
        builder = builder
            .expression_attribute_names("#role", ROLE)
            .expression_attribute_values(":leader", AttributeValue::S("leader".into()));
    }
    if required == RequiredProposalRole::Editor {
        builder =
            builder.expression_attribute_values(":member_role", AttributeValue::S("member".into()));
    }
    builder.build().expect("membership condition is complete")
}

pub(super) fn record_revision_condition(
    table_name: &str,
    partition_key: &str,
    sort_key: &str,
    entity: &str,
    revision: u64,
) -> ConditionCheck {
    ConditionCheck::builder()
        .table_name(table_name)
        .key(PK, AttributeValue::S(partition_key.to_string()))
        .key(SK, AttributeValue::S(sort_key.to_string()))
        .condition_expression("#entity = :entity AND #revision = :revision")
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#revision", REVISION)
        .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
        .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
        .build()
        .expect("record revision condition is complete")
}

pub(super) fn current_plan_condition(
    table_name: &str,
    trip_id: &str,
    revision: u64,
    plan_id: &str,
    version: u32,
) -> ConditionCheck {
    ConditionCheck::builder()
        .table_name(table_name)
        .key(PK, AttributeValue::S(trip_pk(trip_id)))
        .key(SK, AttributeValue::S(META_SK.into()))
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

pub(super) fn stale_plan_condition(
    table_name: &str,
    trip_id: &str,
    base_version: u32,
) -> ConditionCheck {
    ConditionCheck::builder()
        .table_name(table_name)
        .key(PK, AttributeValue::S(trip_pk(trip_id)))
        .key(SK, AttributeValue::S(META_SK.into()))
        .condition_expression("#entity = :trip AND #plan_version <> :base_version")
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#plan_version", CURRENT_PLAN_VERSION)
        .expression_attribute_values(":trip", AttributeValue::S(TRIP_ENTITY.into()))
        .expression_attribute_values(":base_version", AttributeValue::N(base_version.to_string()))
        .build()
        .expect("stale plan condition is complete")
}

pub(super) fn revision_put(
    table_name: &str,
    item: HashMap<String, AttributeValue>,
    expected_revision: u64,
) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression("#revision = :revision")
        .expression_attribute_names("#revision", REVISION)
        .expression_attribute_values(
            ":revision",
            AttributeValue::N(expected_revision.to_string()),
        )
        .build()
        .expect("revision put is complete")
}

pub(super) fn current_plan_revision_put(
    table_name: &str,
    item: HashMap<String, AttributeValue>,
    expected_revision: u64,
    expected_plan_id: &str,
    expected_plan_version: u32,
) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression(
            "#entity = :trip AND #revision = :revision AND #plan_id = :plan_id AND #plan_version = :plan_version",
        )
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#revision", REVISION)
        .expression_attribute_names("#plan_id", CURRENT_PLAN_ID)
        .expression_attribute_names("#plan_version", CURRENT_PLAN_VERSION)
        .expression_attribute_values(":trip", AttributeValue::S(TRIP_ENTITY.into()))
        .expression_attribute_values(":revision", AttributeValue::N(expected_revision.to_string()))
        .expression_attribute_values(":plan_id", AttributeValue::S(expected_plan_id.into()))
        .expression_attribute_values(
            ":plan_version",
            AttributeValue::N(expected_plan_version.to_string()),
        )
        .build()
        .expect("current plan revision put is complete")
}

pub(super) fn create_put(table_name: &str, item: HashMap<String, AttributeValue>) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
        .expression_attribute_names("#pk", PK)
        .expression_attribute_names("#sk", SK)
        .build()
        .expect("create put is complete")
}

pub(super) fn action_condition(condition: ConditionCheck) -> TransactWriteItem {
    TransactWriteItem::builder()
        .condition_check(condition)
        .build()
}

pub(super) fn action_put(put: Put) -> TransactWriteItem {
    TransactWriteItem::builder().put(put).build()
}

pub(super) fn transaction_condition_failed(error: Option<&TransactWriteItemsError>) -> bool {
    let Some(TransactWriteItemsError::TransactionCanceledException(cancellation)) = error else {
        return false;
    };
    let mut saw_condition = false;
    for reason in cancellation.cancellation_reasons() {
        match reason.code() {
            None | Some("None") => {}
            Some(CONDITIONAL_FAILURE) => saw_condition = true,
            Some(_) => return false,
        }
    }
    saw_condition
}

pub(super) fn transaction_data_bytes(
    items: &[HashMap<String, AttributeValue>],
) -> Result<usize, ProposalRepoError> {
    items.iter().try_fold(0_usize, |total, item| {
        let data = string(item, DATA).map_err(record_error)?;
        total
            .checked_add(data.len())
            .ok_or(ProposalRepoError::SafetyLimitExceeded)
    })
}

pub(super) fn enforce_transaction_data_limit(
    items: &[HashMap<String, AttributeValue>],
) -> Result<(), ProposalRepoError> {
    if transaction_data_bytes(items)? > MAX_TRANSACTION_DATA_BYTES {
        Err(ProposalRepoError::SafetyLimitExceeded)
    } else {
        Ok(())
    }
}

pub(super) fn enforce_transaction_action_limit(count: usize) -> Result<(), ProposalRepoError> {
    if count > MAX_TRANSACTION_ACTIONS {
        Err(ProposalRepoError::SafetyLimitExceeded)
    } else {
        Ok(())
    }
}
