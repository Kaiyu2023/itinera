//! Mechanical DynamoDB key, read, and transaction constructors shared by repository capabilities.
//!
//! These helpers encode storage idioms, not authorization policy. Capability
//! modules continue to own membership requirements, stale-state rules, and
//! field-level compare-and-swap conditions so those decisions remain visible
//! during security review.

use std::collections::HashMap;

use aws_sdk_dynamodb::{
    Client,
    operation::{
        get_item::builders::GetItemFluentBuilder, query::builders::QueryFluentBuilder,
        transact_write_items::TransactWriteItemsError,
    },
    types::{AttributeValue, ConditionCheck, Delete, Put, TransactWriteItem, Update},
};

use super::{CONDITIONAL_FAILURE, CREATE_ONLY_CONDITION, ENTITY_TYPE, PK, REVISION, SK};

pub(in crate::dynamodb) fn item_key(
    partition_key: impl Into<String>,
    sort_key: impl Into<String>,
) -> HashMap<String, AttributeValue> {
    HashMap::from([
        (PK.to_string(), AttributeValue::S(partition_key.into())),
        (SK.to_string(), AttributeValue::S(sort_key.into())),
    ])
}

pub(in crate::dynamodb) fn consistent_get(
    client: &Client,
    table_name: &str,
    partition_key: impl Into<String>,
    sort_key: impl Into<String>,
) -> GetItemFluentBuilder {
    client
        .get_item()
        .table_name(table_name)
        .set_key(Some(item_key(partition_key, sort_key)))
        .consistent_read(true)
}

pub(in crate::dynamodb) fn partition_prefix_query(
    client: &Client,
    table_name: &str,
    partition_key: &str,
    prefix: &str,
) -> QueryFluentBuilder {
    client
        .query()
        .table_name(table_name)
        .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
        .expression_attribute_names("#pk", PK)
        .expression_attribute_names("#sk", SK)
        .expression_attribute_values(":pk", AttributeValue::S(partition_key.to_string()))
        .expression_attribute_values(":prefix", AttributeValue::S(prefix.to_string()))
        .consistent_read(true)
}

pub(in crate::dynamodb) fn create_only_put(
    table_name: &str,
    item: HashMap<String, AttributeValue>,
) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression(CREATE_ONLY_CONDITION)
        .expression_attribute_names("#pk", PK)
        .expression_attribute_names("#sk", SK)
        .build()
        .expect("create-only put is complete")
}

pub(in crate::dynamodb) fn revision_put(
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
        .expect("revision-guarded put is complete")
}

pub(in crate::dynamodb) fn entity_revision_condition(
    table_name: &str,
    partition_key: impl Into<String>,
    sort_key: impl Into<String>,
    entity: &str,
    revision: u64,
) -> ConditionCheck {
    ConditionCheck::builder()
        .table_name(table_name)
        .set_key(Some(item_key(partition_key, sort_key)))
        .condition_expression("#entity = :entity AND #revision = :revision")
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#revision", REVISION)
        .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
        .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
        .build()
        .expect("entity revision condition is complete")
}

pub(in crate::dynamodb) fn put_action(put: Put) -> TransactWriteItem {
    TransactWriteItem::builder().put(put).build()
}

pub(in crate::dynamodb) fn condition_action(condition: ConditionCheck) -> TransactWriteItem {
    TransactWriteItem::builder()
        .condition_check(condition)
        .build()
}

pub(in crate::dynamodb) fn update_action(update: Update) -> TransactWriteItem {
    TransactWriteItem::builder().update(update).build()
}

pub(in crate::dynamodb) fn delete_action(delete: Delete) -> TransactWriteItem {
    TransactWriteItem::builder().delete(delete).build()
}

pub(in crate::dynamodb) fn transaction_condition_failed(
    error: Option<&TransactWriteItemsError>,
) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "itinera-test";

    #[test]
    fn shared_record_guards_keep_the_exact_storage_contract() {
        let guard = entity_revision_condition(TABLE, "TRIP#trip-a", "PLAN#1", "PLAN", 7);

        assert_eq!(
            guard.key().get(PK),
            Some(&AttributeValue::S("TRIP#trip-a".into()))
        );
        assert_eq!(
            guard.key().get(SK),
            Some(&AttributeValue::S("PLAN#1".into()))
        );
        assert_eq!(
            guard.condition_expression(),
            "#entity = :entity AND #revision = :revision"
        );
        assert_eq!(
            guard
                .expression_attribute_values()
                .and_then(|values| values.get(":revision")),
            Some(&AttributeValue::N("7".into()))
        );
    }

    #[test]
    fn shared_puts_distinguish_creation_from_revision_replacement() {
        let item = item_key("TRIP#trip-a", "PROPOSAL#proposal-a");
        let create = create_only_put(TABLE, item.clone());
        let replace = revision_put(TABLE, item, 4);

        assert_eq!(create.condition_expression(), Some(CREATE_ONLY_CONDITION));
        assert_eq!(
            replace.condition_expression(),
            Some("#revision = :revision")
        );
        assert_eq!(
            replace
                .expression_attribute_values()
                .and_then(|values| values.get(":revision")),
            Some(&AttributeValue::N("4".into()))
        );
    }
}
