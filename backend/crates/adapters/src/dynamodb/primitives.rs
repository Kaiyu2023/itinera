//! Mechanical DynamoDB key, read, and transaction constructors shared by repository capabilities.
//!
//! These helpers encode storage idioms, not authorization policy. Capability
//! modules continue to own membership requirements, stale-state rules, and
//! field-level compare-and-swap conditions so those decisions remain visible
//! during security review. Table-bound constructors are repository methods so
//! callers cannot detach the configured client from its table name.

use std::collections::HashMap;

use aws_sdk_dynamodb::{
    operation::{
        get_item::builders::GetItemFluentBuilder,
        query::builders::QueryFluentBuilder,
        transact_write_items::{
            TransactWriteItemsError, builders::TransactWriteItemsFluentBuilder,
        },
    },
    types::{AttributeValue, ConditionCheck, Delete, Put, TransactWriteItem, Update},
};

use super::{
    CONDITIONAL_FAILURE, CREATE_ONLY_CONDITION, DynamoUserRepo, ENTITY_TYPE, PK, REVISION, SK,
    trip_repo::records::DATA,
};

impl DynamoUserRepo {
    pub(in crate::dynamodb) fn consistent_get(
        &self,
        partition_key: impl Into<String>,
        sort_key: impl Into<String>,
    ) -> GetItemFluentBuilder {
        self.client
            .get_item()
            .table_name(&self.table_name)
            .set_key(Some(item_key(partition_key, sort_key)))
            .consistent_read(true)
    }

    pub(in crate::dynamodb) fn partition_prefix_query(
        &self,
        partition_key: &str,
        prefix: &str,
    ) -> QueryFluentBuilder {
        self.client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
            .expression_attribute_names("#pk", PK)
            .expression_attribute_names("#sk", SK)
            .expression_attribute_values(":pk", AttributeValue::S(partition_key.to_string()))
            .expression_attribute_values(":prefix", AttributeValue::S(prefix.to_string()))
            .consistent_read(true)
    }

    pub(in crate::dynamodb) fn table_query(&self) -> QueryFluentBuilder {
        self.client.query().table_name(&self.table_name)
    }

    pub(in crate::dynamodb) fn transaction(&self) -> TransactWriteItemsFluentBuilder {
        self.client.transact_write_items()
    }

    pub(in crate::dynamodb) fn create_only_put(
        &self,
        item: HashMap<String, AttributeValue>,
    ) -> Put {
        Put::builder()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .condition_expression(CREATE_ONLY_CONDITION)
            .expression_attribute_names("#pk", PK)
            .expression_attribute_names("#sk", SK)
            .build()
            .expect("create-only put is complete")
    }

    pub(in crate::dynamodb) fn revision_put(
        &self,
        item: HashMap<String, AttributeValue>,
        expected_revision: u64,
    ) -> Put {
        Put::builder()
            .table_name(&self.table_name)
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

    pub(in crate::dynamodb) fn snapshot_put(
        &self,
        item: HashMap<String, AttributeValue>,
        expected_revision: u64,
        expected_data: &str,
    ) -> Put {
        Put::builder()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .condition_expression("#revision = :revision AND #data = :data")
            .expression_attribute_names("#revision", REVISION)
            .expression_attribute_names("#data", DATA)
            .expression_attribute_values(
                ":revision",
                AttributeValue::N(expected_revision.to_string()),
            )
            .expression_attribute_values(":data", AttributeValue::S(expected_data.to_string()))
            .build()
            .expect("snapshot-guarded put is complete")
    }

    pub(in crate::dynamodb) fn entity_snapshot_put(
        &self,
        item: HashMap<String, AttributeValue>,
        entity: &str,
        expected_revision: u64,
        expected_data: &str,
    ) -> Put {
        Put::builder()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .condition_expression("#entity = :entity AND #revision = :revision AND #data = :data")
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#revision", REVISION)
            .expression_attribute_names("#data", DATA)
            .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
            .expression_attribute_values(
                ":revision",
                AttributeValue::N(expected_revision.to_string()),
            )
            .expression_attribute_values(":data", AttributeValue::S(expected_data.to_string()))
            .build()
            .expect("entity snapshot-guarded put is complete")
    }

    pub(in crate::dynamodb) fn entity_snapshot_delete(
        &self,
        partition_key: impl Into<String>,
        sort_key: impl Into<String>,
        entity: &str,
        expected_revision: u64,
        expected_data: &str,
    ) -> Delete {
        Delete::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(partition_key, sort_key)))
            .condition_expression("#entity = :entity AND #revision = :revision AND #data = :data")
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#revision", REVISION)
            .expression_attribute_names("#data", DATA)
            .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
            .expression_attribute_values(
                ":revision",
                AttributeValue::N(expected_revision.to_string()),
            )
            .expression_attribute_values(":data", AttributeValue::S(expected_data.to_string()))
            .build()
            .expect("entity snapshot-guarded delete is complete")
    }

    pub(in crate::dynamodb) fn entity_revision_condition(
        &self,
        partition_key: impl Into<String>,
        sort_key: impl Into<String>,
        entity: &str,
        revision: u64,
    ) -> ConditionCheck {
        ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(partition_key, sort_key)))
            .condition_expression("#entity = :entity AND #revision = :revision")
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#revision", REVISION)
            .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
            .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
            .build()
            .expect("entity revision condition is complete")
    }

    pub(in crate::dynamodb) fn entity_revision_data_condition(
        &self,
        partition_key: impl Into<String>,
        sort_key: impl Into<String>,
        entity: &str,
        revision: u64,
        expected_data: &str,
    ) -> ConditionCheck {
        ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(partition_key, sort_key)))
            .condition_expression("#entity = :entity AND #revision = :revision AND #data = :data")
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#revision", REVISION)
            .expression_attribute_names("#data", DATA)
            .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
            .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
            .expression_attribute_values(":data", AttributeValue::S(expected_data.to_string()))
            .build()
            .expect("entity revision data condition is complete")
    }

    pub(in crate::dynamodb) fn record_absent_condition(
        &self,
        partition_key: impl Into<String>,
        sort_key: impl Into<String>,
    ) -> ConditionCheck {
        ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(partition_key, sort_key)))
            .condition_expression(CREATE_ONLY_CONDITION)
            .expression_attribute_names("#pk", PK)
            .expression_attribute_names("#sk", SK)
            .build()
            .expect("record absence condition is complete")
    }
}

pub(in crate::dynamodb) fn item_key(
    partition_key: impl Into<String>,
    sort_key: impl Into<String>,
) -> HashMap<String, AttributeValue> {
    HashMap::from([
        (PK.to_string(), AttributeValue::S(partition_key.into())),
        (SK.to_string(), AttributeValue::S(sort_key.into())),
    ])
}

pub(in crate::dynamodb) fn encoded_item_bytes(
    item: &HashMap<String, AttributeValue>,
) -> Option<usize> {
    let mut bytes = 0_usize;
    for (name, value) in item {
        bytes = bytes.checked_add(name.len())?;
        bytes = bytes.checked_add(attribute_value_bytes(value)?)?;
    }
    Some(bytes)
}

fn attribute_value_bytes(value: &AttributeValue) -> Option<usize> {
    match value {
        AttributeValue::B(value) => Some(value.as_ref().len()),
        AttributeValue::Bool(_) | AttributeValue::Null(_) => Some(1),
        AttributeValue::Bs(values) => checked_sum(values.iter().map(|value| value.as_ref().len())),
        AttributeValue::L(values) => values
            .iter()
            .try_fold(nested_collection_overhead(values.len())?, |total, value| {
                total.checked_add(attribute_value_bytes(value)?)
            }),
        AttributeValue::M(values) => {
            let mut bytes = nested_collection_overhead(values.len())?;
            for (name, value) in values {
                bytes = bytes.checked_add(name.len())?;
                bytes = bytes.checked_add(attribute_value_bytes(value)?)?;
            }
            Some(bytes)
        }
        // DynamoDB numbers use roughly one byte per two significant digits plus
        // one byte. The request string length is conservative except for a
        // one-character number, which still consumes two bytes.
        AttributeValue::N(value) => Some(value.len().max(2)),
        AttributeValue::Ns(values) => checked_sum(values.iter().map(|value| value.len().max(2))),
        AttributeValue::S(value) => Some(value.len()),
        AttributeValue::Ss(values) => checked_sum(values.iter().map(String::len)),
        _ => None,
    }
}

fn nested_collection_overhead(element_count: usize) -> Option<usize> {
    // DynamoDB charges three bytes for every list/map plus one byte for each
    // nested element, even when that element's value is empty.
    3_usize.checked_add(element_count)
}

fn checked_sum(values: impl IntoIterator<Item = usize>) -> Option<usize> {
    values.into_iter().try_fold(0_usize, usize::checked_add)
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
    use std::collections::HashMap;

    use aws_smithy_mocks::mock_client;

    use super::*;

    const TABLE: &str = "itinera-test";

    fn repo() -> DynamoUserRepo {
        DynamoUserRepo::new(mock_client!(aws_sdk_dynamodb, &[]), TABLE).expect("valid table")
    }

    #[test]
    fn encoded_sizes_include_nested_container_and_empty_element_overhead() {
        let item = HashMap::from([(
            "nested".into(),
            AttributeValue::L(vec![
                AttributeValue::S(String::new()),
                AttributeValue::M(HashMap::from([(
                    "empty".into(),
                    AttributeValue::S(String::new()),
                )])),
                AttributeValue::L(vec![AttributeValue::Null(true)]),
            ]),
        )]);

        // name(6) + outer list(3 + 3 elements) + empty string(0)
        // + map(3 + 1 element + name(5)) + inner list(3 + 1 element + null(1)).
        assert_eq!(encoded_item_bytes(&item), Some(26));
    }

    #[test]
    fn large_nested_empty_values_cannot_disappear_from_a_byte_budget() {
        let element_count = 100_000;
        let item = HashMap::from([(
            "nested".into(),
            AttributeValue::L(vec![AttributeValue::S(String::new()); element_count]),
        )]);

        assert_eq!(
            encoded_item_bytes(&item),
            Some("nested".len() + 3 + element_count)
        );
        assert_eq!(
            encoded_item_bytes(&HashMap::from([(
                "number".into(),
                AttributeValue::N("1".into()),
            )])),
            Some("number".len() + 2)
        );
    }

    #[test]
    fn shared_record_guards_keep_the_exact_storage_contract() {
        let guard = repo().entity_revision_condition("TRIP#trip-a", "PLAN#1", "PLAN", 7);

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
        let repo = repo();
        let create = repo.create_only_put(item.clone());
        let replace = repo.revision_put(item.clone(), 4);
        let snapshot = repo.entity_snapshot_put(item, "PROPOSAL", 4, "{\"status\":\"pending\"}");

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
        assert_eq!(
            snapshot.condition_expression(),
            Some("#entity = :entity AND #revision = :revision AND #data = :data")
        );
        assert_eq!(
            snapshot
                .expression_attribute_values()
                .and_then(|values| values.get(":entity")),
            Some(&AttributeValue::S("PROPOSAL".into()))
        );
        assert_eq!(
            snapshot
                .expression_attribute_values()
                .and_then(|values| values.get(":data")),
            Some(&AttributeValue::S("{\"status\":\"pending\"}".into()))
        );
    }
}
