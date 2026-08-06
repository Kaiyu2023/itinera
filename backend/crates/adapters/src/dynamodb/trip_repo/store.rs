//! Shared strongly consistent reads and DynamoDB transaction primitives.

use super::*;

pub(super) fn create_put(table_name: &str, item: HashMap<String, AttributeValue>) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
        .expression_attribute_names("#pk", PK)
        .expression_attribute_names("#sk", SK)
        .build()
        .expect("table and item are present")
}

pub(super) fn revision_put(
    table_name: &str,
    item: HashMap<String, AttributeValue>,
    expected_revision: u64,
) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression("#revision = :expected_revision")
        .expression_attribute_names("#revision", REVISION)
        .expression_attribute_values(
            ":expected_revision",
            AttributeValue::N(expected_revision.to_string()),
        )
        .build()
        .expect("table and item are present")
}

pub(super) fn member_condition(
    table_name: &str,
    trip_id: &str,
    actor: &UserId,
    required: RequiredRole,
) -> ConditionCheck {
    let mut builder = ConditionCheck::builder()
        .table_name(table_name)
        .key(PK, AttributeValue::S(trip_pk(trip_id)))
        .key(SK, AttributeValue::S(member_sk(actor)))
        .condition_expression(match required {
            RequiredRole::Any => "#entity = :member",
            RequiredRole::Editor => "#entity = :member AND (#role = :leader OR #role = :editor)",
            RequiredRole::Leader => "#entity = :member AND #role = :leader",
        })
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()));
    if required != RequiredRole::Any {
        builder = builder
            .expression_attribute_names("#role", ROLE)
            .expression_attribute_values(":leader", AttributeValue::S("leader".into()));
    }
    if required == RequiredRole::Editor {
        builder =
            builder.expression_attribute_values(":editor", AttributeValue::S("member".into()));
    }
    builder.build().expect("condition is complete")
}

pub(super) fn record_revision_condition(
    table_name: &str,
    partition_key: String,
    sort_key: String,
    entity: &str,
    revision: u64,
) -> ConditionCheck {
    ConditionCheck::builder()
        .table_name(table_name)
        .key(PK, AttributeValue::S(partition_key))
        .key(SK, AttributeValue::S(sort_key))
        .condition_expression("#entity = :entity AND #revision = :revision")
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#revision", REVISION)
        .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
        .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
        .build()
        .expect("revision condition is complete")
}

pub(super) fn user_membership_count_update(
    table_name: &str,
    user_id: &UserId,
    increment: bool,
) -> Update {
    let (expression, amount) = if increment {
        ("SET #count = if_not_exists(#count, :zero) + :amount", "1")
    } else {
        ("SET #count = #count - :amount", "1")
    };
    let condition = if increment {
        "#entity = :profile"
    } else {
        "#entity = :profile AND #count >= :amount"
    };
    let mut builder = Update::builder()
        .table_name(table_name)
        .key(PK, AttributeValue::S(user_partition_key(user_id)))
        .key(SK, AttributeValue::S(USER_PROFILE_SK.into()))
        .update_expression(expression)
        .condition_expression(condition)
        .expression_attribute_names("#count", MEMBERSHIP_COUNT)
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_values(":amount", AttributeValue::N(amount.into()))
        .expression_attribute_values(":profile", AttributeValue::S(USER_PROFILE_ENTITY.into()));
    if increment {
        builder = builder.expression_attribute_values(":zero", AttributeValue::N("0".into()));
    }
    builder.build().expect("user membership update is complete")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequiredRole {
    Any,
    Editor,
    Leader,
}

impl DynamoUserRepo {
    pub(super) async fn trip_get(
        &self,
        partition_key: &str,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, TripRepoError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key(PK, AttributeValue::S(partition_key.to_string()))
            .key(SK, AttributeValue::S(sort_key.to_string()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| TripRepoError::Unavailable)?;
        Ok(output.item)
    }

    pub(super) async fn query_partition(
        &self,
        partition_key: &str,
        prefix: &str,
        page_size: i32,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, TripRepoError> {
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
                .limit(page_size)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|_| TripRepoError::Unavailable)?;
            let next = output
                .last_evaluated_key()
                .filter(|key| !key.is_empty())
                .cloned();
            items.extend(output.items.unwrap_or_default());
            let Some(next) = next else {
                break;
            };
            cursor = Some(next);
        }
        Ok(items)
    }
}

pub(super) fn action_put(put: Put) -> TransactWriteItem {
    TransactWriteItem::builder().put(put).build()
}

pub(super) fn action_condition(condition: ConditionCheck) -> TransactWriteItem {
    TransactWriteItem::builder()
        .condition_check(condition)
        .build()
}

pub(super) fn action_update(update: Update) -> TransactWriteItem {
    TransactWriteItem::builder().update(update).build()
}
