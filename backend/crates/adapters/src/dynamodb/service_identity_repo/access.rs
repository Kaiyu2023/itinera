use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use itinera_core::ports::service_identity::ServiceIdentityRepoError;

use crate::dynamodb::{DynamoUserRepo, primitives::encoded_item_bytes};

use super::records::{SERVICE_PREFIX, USAGE_PREFIX};

const QUERY_PAGE_SIZE: i32 = 50;
const MAX_QUERY_BYTES: usize = 4 * 1_024 * 1_024;

impl DynamoUserRepo {
    pub(super) async fn service_get(
        &self,
        partition_key: impl Into<String>,
        sort_key: impl Into<String>,
    ) -> Result<Option<HashMap<String, AttributeValue>>, ServiceIdentityRepoError> {
        self.consistent_get(partition_key, sort_key)
            .send()
            .await
            .map(|output| output.item)
            .map_err(|_| ServiceIdentityRepoError::Unavailable)
    }

    pub(super) async fn service_mapping_query(
        &self,
        partition_key: &str,
        maximum: usize,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, ServiceIdentityRepoError> {
        let mut items = Vec::new();
        let mut bytes = 0_usize;
        let mut cursor = None;
        loop {
            let output = self
                .partition_prefix_query(partition_key, SERVICE_PREFIX)
                .limit(QUERY_PAGE_SIZE)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|_| ServiceIdentityRepoError::Unavailable)?;
            let next = output
                .last_evaluated_key()
                .filter(|key| !key.is_empty())
                .cloned();
            let page = output.items.unwrap_or_default();
            if page.len() > maximum.saturating_sub(items.len()) {
                return Err(ServiceIdentityRepoError::SafetyLimitExceeded);
            }
            for item in &page {
                bytes = bytes
                    .checked_add(
                        encoded_item_bytes(item)
                            .ok_or(ServiceIdentityRepoError::SafetyLimitExceeded)?,
                    )
                    .ok_or(ServiceIdentityRepoError::SafetyLimitExceeded)?;
                if bytes > MAX_QUERY_BYTES {
                    return Err(ServiceIdentityRepoError::SafetyLimitExceeded);
                }
            }
            items.extend(page);
            let Some(next) = next else { break };
            cursor = Some(next);
        }
        Ok(items)
    }

    pub(super) async fn latest_service_usage(
        &self,
        partition_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, ServiceIdentityRepoError> {
        self.partition_prefix_query(partition_key, USAGE_PREFIX)
            .scan_index_forward(false)
            .limit(1)
            .send()
            .await
            .map(|output| output.items.and_then(|mut items| items.pop()))
            .map_err(|_| ServiceIdentityRepoError::Unavailable)
    }
}
