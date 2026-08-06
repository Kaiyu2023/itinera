//! Shared strongly consistent reads and DynamoDB transaction primitives.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::{AttributeValue, ConditionCheck, Update};
use itinera_core::{domain::user::UserId, ports::trip::TripRepoError};

use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE, MEMBERSHIP_COUNT, USER_PROFILE_ENTITY, USER_PROFILE_SK,
    primitives::item_key, user_partition_key,
};

use super::records::{MEMBER_ENTITY, ROLE, member_sk, trip_pk};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequiredRole {
    Any,
    Editor,
    Leader,
}

impl DynamoUserRepo {
    pub(super) fn member_condition(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredRole,
    ) -> ConditionCheck {
        let mut builder = ConditionCheck::builder()
            .table_name(&self.table_name)
            .set_key(Some(item_key(trip_pk(trip_id), member_sk(actor))))
            .condition_expression(match required {
                RequiredRole::Any => "#entity = :member",
                RequiredRole::Editor => {
                    "#entity = :member AND (#role = :leader OR #role = :editor)"
                }
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

    pub(super) fn user_membership_count_update(&self, user_id: &UserId, increment: bool) -> Update {
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
            .table_name(&self.table_name)
            .set_key(Some(item_key(user_partition_key(user_id), USER_PROFILE_SK)))
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

    pub(super) async fn trip_get(
        &self,
        partition_key: &str,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, TripRepoError> {
        let output = self
            .consistent_get(partition_key, sort_key)
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
                .partition_prefix_query(partition_key, prefix)
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
