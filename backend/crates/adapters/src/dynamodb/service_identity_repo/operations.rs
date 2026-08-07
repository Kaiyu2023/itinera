use std::collections::HashSet;

use aws_sdk_dynamodb::{
    operation::transact_write_items::TransactWriteItemsError,
    types::{AttributeValue, Update},
};
use chrono::{DateTime, Duration, NaiveDateTime, SecondsFormat, Timelike, Utc};
use itinera_core::{
    domain::{
        service_identity::{ServiceGrant, ServiceIdentity},
        user::UserId,
    },
    ports::{
        auth::ServiceCommonName,
        service_identity::{NewServiceIdentity, ServiceIdentityRepoError},
    },
    services::service_identities::{MAX_SERVICE_IDENTITIES, SERVICE_REQUESTS_PER_HOUR},
};

use crate::dynamodb::{
    CONDITIONAL_FAILURE, CURRENT_SCHEMA_VERSION, DynamoUserRepo, ENTITY_TYPE, PK, REVISION,
    SCHEMA_VERSION, SK,
    primitives::{condition_action, put_action, update_action},
    trip_repo::records::{MEMBER_ENTITY, member_sk, trip_pk},
    user_partition_key,
};

use super::records::{
    BUCKET_EXPIRES_AT, CLAIM_SK, COMMON_NAME_DIGEST, COUNT, ClaimStatus, LAST_USED_AT,
    MembershipSnapshot, SERVICE_CLAIM_ENTITY, SERVICE_MAPPING_ENTITY, SERVICE_META_SK,
    SERVICE_USAGE_ENTITY, ServiceClaimRecord, ServiceMappingRecord, TTL, claim_data, claim_pk,
    common_name_digest, decode_claim, decode_mapping, decode_membership, decode_meta, decode_usage,
    encode_claim, encode_mapping, encode_meta, mapping_data, meta_data, required_role, service_sk,
    usage_pk, usage_sk,
};

const AUTH_TRANSACTION_ATTEMPTS: usize = 4;
const LIST_SNAPSHOT_ATTEMPTS: usize = 2;
const CREATE_ATTEMPTS: usize = 3;
const REVOKE_ATTEMPTS: usize = 3;
const USAGE_RETENTION_HOURS: i64 = 48;
const TRANSACTION_CONFLICT: &str = "TransactionConflict";

pub(super) async fn list_service_identities(
    repo: &DynamoUserRepo,
    owner: &UserId,
) -> Result<Vec<ServiceIdentity>, ServiceIdentityRepoError> {
    let owner_pk = user_partition_key(owner);
    for _ in 0..LIST_SNAPSHOT_ATTEMPTS {
        let first_meta = repo.service_get(&owner_pk, SERVICE_META_SK).await?;
        let items = repo
            .service_mapping_query(&owner_pk, MAX_SERVICE_IDENTITIES)
            .await?;
        let second_meta = repo.service_get(&owner_pk, SERVICE_META_SK).await?;
        if !same_optional_snapshot(&first_meta, &second_meta)? {
            continue;
        }

        let expected_count = match first_meta {
            Some(ref item) => decode_meta(item, owner)?.value.count as usize,
            None => 0,
        };
        if expected_count != items.len() {
            return Err(ServiceIdentityRepoError::CorruptData);
        }
        let mut identities = Vec::with_capacity(items.len());
        let mut ids = HashSet::with_capacity(items.len());
        for item in items {
            let sort_key = item
                .get(SK)
                .and_then(|value| value.as_s().ok())
                .ok_or(ServiceIdentityRepoError::CorruptData)?;
            let service_id = sort_key
                .strip_prefix(super::records::SERVICE_PREFIX)
                .filter(|value| !value.is_empty())
                .ok_or(ServiceIdentityRepoError::CorruptData)?;
            if !ids.insert(service_id.to_string()) {
                return Err(ServiceIdentityRepoError::CorruptData);
            }
            let stored = decode_mapping(&item, owner, service_id)?;
            let last_used_at = latest_usage(repo, &stored.value.common_name_digest).await?;
            identities.push(stored.value.identity(last_used_at));
        }
        identities.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then(left.id.cmp(&right.id))
        });
        return Ok(identities);
    }
    Err(ServiceIdentityRepoError::Conflict)
}

pub(super) async fn create_service_identity(
    repo: &DynamoUserRepo,
    owner: &UserId,
    new_identity: NewServiceIdentity,
) -> Result<ServiceIdentity, ServiceIdentityRepoError> {
    let digest = common_name_digest(new_identity.common_name.as_str());
    let mapping = ServiceMappingRecord::from_new(owner, &new_identity.identity, digest.clone());
    let claim = ServiceClaimRecord {
        common_name_digest: digest,
        owner_id: owner.0.clone(),
        service_id: mapping.id.clone(),
        status: ClaimStatus::Active,
    };
    let require_editor = required_role(&mapping.scopes).can_edit();
    let mut memberships = Vec::with_capacity(mapping.trip_ids.len());
    for trip_id in &mapping.trip_ids {
        let item = repo
            .service_get(trip_pk(trip_id), member_sk(owner))
            .await?
            .ok_or(ServiceIdentityRepoError::NotFound)?;
        memberships.push(decode_membership(&item, trip_id, owner, require_editor)?);
    }

    let mut last_retry_was_transaction_conflict = false;
    for _ in 0..CREATE_ATTEMPTS {
        let meta_item = repo
            .service_get(user_partition_key(owner), SERVICE_META_SK)
            .await?;
        let (old_count, old_revision, old_data) = match &meta_item {
            Some(item) => {
                let stored = decode_meta(item, owner)?;
                (
                    stored.value.count,
                    Some(stored.revision),
                    Some(meta_data(item)?),
                )
            }
            None => (0, None, None),
        };
        if old_count as usize >= MAX_SERVICE_IDENTITIES {
            return Err(ServiceIdentityRepoError::SafetyLimitExceeded);
        }
        let next_meta = encode_meta(
            owner,
            old_count + 1,
            next_revision(old_revision.unwrap_or(0))?,
        )?;
        let meta_put = match (old_revision, old_data) {
            (Some(revision), Some(data)) => repo.entity_snapshot_put(
                next_meta,
                super::records::SERVICE_META_ENTITY,
                revision,
                &data,
            ),
            (None, None) => repo.create_only_put(next_meta),
            _ => return Err(ServiceIdentityRepoError::CorruptData),
        };
        let mapping_put = repo.create_only_put(encode_mapping(owner, &mapping, 1)?);
        let claim_put = repo.create_only_put(encode_claim(&claim, 1)?);
        let mut transaction = repo
            .transaction()
            .transact_items(put_action(meta_put))
            .transact_items(put_action(mapping_put))
            .transact_items(put_action(claim_put));
        for snapshot in &memberships {
            transaction = transaction.transact_items(condition_action(membership_condition(
                repo, owner, snapshot,
            )));
        }
        match transaction.send().await {
            Ok(_) => return Ok(new_identity.identity),
            Err(error) if action_condition_failed(error.as_service_error(), 2) => {
                return Err(ServiceIdentityRepoError::DuplicateCredential);
            }
            Err(error) if action_condition_failed(error.as_service_error(), 0) => {
                last_retry_was_transaction_conflict = false;
                continue;
            }
            Err(error) if any_condition_failed(error.as_service_error()) => {
                return Err(ServiceIdentityRepoError::Conflict);
            }
            Err(error) if any_transaction_conflict(error.as_service_error()) => {
                last_retry_was_transaction_conflict = true;
                tokio::task::yield_now().await;
                continue;
            }
            Err(_) => return Err(ServiceIdentityRepoError::Unavailable),
        }
    }
    if last_retry_was_transaction_conflict {
        Err(ServiceIdentityRepoError::Unavailable)
    } else {
        Err(ServiceIdentityRepoError::Conflict)
    }
}

pub(super) async fn revoke_service_identity(
    repo: &DynamoUserRepo,
    owner: &UserId,
    service_id: &str,
    revoked_at: &str,
) -> Result<(), ServiceIdentityRepoError> {
    for _ in 0..REVOKE_ATTEMPTS {
        let mapping_item = repo
            .service_get(user_partition_key(owner), service_sk(service_id))
            .await?
            .ok_or(ServiceIdentityRepoError::NotFound)?;
        let mut mapping = decode_mapping(&mapping_item, owner, service_id)?;
        let claim_item = repo
            .service_get(claim_pk(&mapping.value.common_name_digest), CLAIM_SK)
            .await?
            .ok_or(ServiceIdentityRepoError::CorruptData)?;
        let mut claim = decode_claim(&claim_item, &mapping.value.common_name_digest)?;
        validate_reciprocal(&mapping.value, &claim.value, owner)?;
        if mapping.value.revoked_at.is_some() {
            return if claim.value.status == ClaimStatus::Revoked {
                Ok(())
            } else {
                Err(ServiceIdentityRepoError::CorruptData)
            };
        }
        if claim.value.status != ClaimStatus::Active {
            return Err(ServiceIdentityRepoError::CorruptData);
        }
        mapping.value.revoked_at = Some(revoked_at.to_string());
        claim.value.status = ClaimStatus::Revoked;
        let mapping_put = repo.entity_snapshot_put(
            encode_mapping(owner, &mapping.value, next_revision(mapping.revision)?)?,
            SERVICE_MAPPING_ENTITY,
            mapping.revision,
            &mapping_data(&mapping_item)?,
        );
        let claim_put = repo.entity_snapshot_put(
            encode_claim(&claim.value, next_revision(claim.revision)?)?,
            SERVICE_CLAIM_ENTITY,
            claim.revision,
            &claim_data(&claim_item)?,
        );
        match repo
            .transaction()
            .transact_items(put_action(mapping_put))
            .transact_items(put_action(claim_put))
            .send()
            .await
        {
            Ok(_) => return Ok(()),
            Err(error) if any_condition_failed(error.as_service_error()) => {
                let current = repo
                    .service_get(user_partition_key(owner), service_sk(service_id))
                    .await?
                    .ok_or(ServiceIdentityRepoError::CorruptData)?;
                let current = decode_mapping(&current, owner, service_id)?;
                if current.value.revoked_at.is_some() {
                    let current_claim = repo
                        .service_get(claim_pk(&current.value.common_name_digest), CLAIM_SK)
                        .await?
                        .ok_or(ServiceIdentityRepoError::CorruptData)?;
                    let current_claim =
                        decode_claim(&current_claim, &current.value.common_name_digest)?;
                    validate_reciprocal(&current.value, &current_claim.value, owner)?;
                    if current_claim.value.status == ClaimStatus::Revoked {
                        return Ok(());
                    }
                }
                return Err(ServiceIdentityRepoError::Conflict);
            }
            Err(error) if any_transaction_conflict(error.as_service_error()) => {
                tokio::task::yield_now().await;
                continue;
            }
            Err(_) => return Err(ServiceIdentityRepoError::Unavailable),
        }
    }
    Err(ServiceIdentityRepoError::Unavailable)
}

pub(super) async fn authenticate_service(
    repo: &DynamoUserRepo,
    common_name: &ServiceCommonName,
    used_at: &str,
) -> Result<ServiceGrant, ServiceIdentityRepoError> {
    let used = utc(used_at)?;
    let digest = common_name_digest(common_name.as_str());
    for _ in 0..AUTH_TRANSACTION_ATTEMPTS {
        let claim_item = repo
            .service_get(claim_pk(&digest), CLAIM_SK)
            .await?
            .ok_or(ServiceIdentityRepoError::CredentialRejected)?;
        let claim = decode_claim(&claim_item, &digest)?;
        if claim.value.status != ClaimStatus::Active {
            return Err(ServiceIdentityRepoError::CredentialRejected);
        }
        let owner = UserId(claim.value.owner_id.clone());
        let mapping_item = repo
            .service_get(
                user_partition_key(&owner),
                service_sk(&claim.value.service_id),
            )
            .await?
            .ok_or(ServiceIdentityRepoError::CorruptData)?;
        let mapping = decode_mapping(&mapping_item, &owner, &claim.value.service_id)?;
        validate_reciprocal(&mapping.value, &claim.value, &owner)?;
        if mapping.value.revoked_at.is_some()
            || utc(&mapping.value.expires_at)? <= used
            || utc(&mapping.value.created_at)? > used
        {
            return Err(ServiceIdentityRepoError::CredentialRejected);
        }

        let bucket = used.format("%Y%m%d%H").to_string();
        let bucket_start = used
            .with_minute(0)
            .and_then(|value| value.with_second(0))
            .and_then(|value| value.with_nanosecond(0))
            .ok_or(ServiceIdentityRepoError::CorruptData)?;
        let bucket_expiry = bucket_start
            .checked_add_signed(Duration::hours(1))
            .ok_or(ServiceIdentityRepoError::CorruptData)?;
        let bucket_expiry_text = bucket_expiry.to_rfc3339_opts(SecondsFormat::AutoSi, true);
        let usage_item = repo
            .service_get(usage_pk(&digest), usage_sk(&bucket))
            .await?;
        if let Some(item) = &usage_item {
            let usage = validated_usage(item, &digest, &bucket, &bucket_expiry_text)?;
            if usage.count >= SERVICE_REQUESTS_PER_HOUR {
                return Err(ServiceIdentityRepoError::RateLimited);
            }
        }
        let claim_condition = repo.entity_revision_data_condition(
            claim_pk(&digest),
            CLAIM_SK,
            SERVICE_CLAIM_ENTITY,
            claim.revision,
            &claim_data(&claim_item)?,
        );
        let mapping_condition = repo.entity_revision_data_condition(
            user_partition_key(&owner),
            service_sk(&mapping.value.id),
            SERVICE_MAPPING_ENTITY,
            mapping.revision,
            &mapping_data(&mapping_item)?,
        );
        let update = usage_update(
            repo,
            &digest,
            &bucket,
            used_at,
            &bucket_expiry_text,
            bucket_expiry
                .checked_add_signed(Duration::hours(USAGE_RETENTION_HOURS))
                .ok_or(ServiceIdentityRepoError::CorruptData)?
                .timestamp(),
        );
        match repo
            .transaction()
            .transact_items(condition_action(claim_condition))
            .transact_items(condition_action(mapping_condition))
            .transact_items(update_action(update))
            .send()
            .await
        {
            Ok(_) => {
                return Ok(ServiceGrant {
                    owner_id: owner,
                    identity: mapping.value.identity(Some(used_at.to_string())),
                });
            }
            Err(error) if action_condition_failed(error.as_service_error(), 2) => {
                let current = repo
                    .service_get(usage_pk(&digest), usage_sk(&bucket))
                    .await?
                    .ok_or(ServiceIdentityRepoError::CorruptData)?;
                let current = validated_usage(&current, &digest, &bucket, &bucket_expiry_text)?;
                if current.count >= SERVICE_REQUESTS_PER_HOUR {
                    return Err(ServiceIdentityRepoError::RateLimited);
                }
                continue;
            }
            Err(error) if any_condition_failed(error.as_service_error()) => {
                return Err(ServiceIdentityRepoError::CredentialRejected);
            }
            Err(error) if any_transaction_conflict(error.as_service_error()) => {
                tokio::task::yield_now().await;
                continue;
            }
            Err(_) => return Err(ServiceIdentityRepoError::Unavailable),
        }
    }
    Err(ServiceIdentityRepoError::Unavailable)
}

async fn latest_usage(
    repo: &DynamoUserRepo,
    digest: &str,
) -> Result<Option<String>, ServiceIdentityRepoError> {
    let Some(item) = repo.latest_service_usage(&usage_pk(digest)).await? else {
        return Ok(None);
    };
    let sort_key = item
        .get(SK)
        .and_then(|value| value.as_s().ok())
        .and_then(|value| value.strip_prefix(super::records::USAGE_PREFIX))
        .filter(|value| value.len() == 10 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or(ServiceIdentityRepoError::CorruptData)?;
    let bucket_start = bucket_start(sort_key)?;
    let expiry = bucket_start
        .checked_add_signed(Duration::hours(1))
        .ok_or(ServiceIdentityRepoError::CorruptData)?
        .to_rfc3339_opts(SecondsFormat::AutoSi, true);
    Ok(Some(
        validated_usage(&item, digest, sort_key, &expiry)?.last_used_at,
    ))
}

fn validated_usage(
    item: &std::collections::HashMap<String, AttributeValue>,
    digest: &str,
    bucket: &str,
    bucket_expiry: &str,
) -> Result<super::records::UsageRecord, ServiceIdentityRepoError> {
    let expiry = utc(bucket_expiry)?;
    let expected_ttl = expiry
        .checked_add_signed(Duration::hours(USAGE_RETENTION_HOURS))
        .ok_or(ServiceIdentityRepoError::CorruptData)?
        .timestamp();
    let usage = decode_usage(item, digest, bucket, bucket_expiry, expected_ttl)?;
    let start = bucket_start(bucket)?;
    let last_used = utc(&usage.last_used_at)?;
    if last_used < start || last_used >= expiry {
        return Err(ServiceIdentityRepoError::CorruptData);
    }
    Ok(usage)
}

fn bucket_start(bucket: &str) -> Result<DateTime<Utc>, ServiceIdentityRepoError> {
    let parsed = NaiveDateTime::parse_from_str(&format!("{bucket}0000"), "%Y%m%d%H%M%S")
        .map_err(|_| ServiceIdentityRepoError::CorruptData)?;
    Ok(DateTime::from_naive_utc_and_offset(parsed, Utc))
}

fn membership_condition(
    repo: &DynamoUserRepo,
    owner: &UserId,
    snapshot: &MembershipSnapshot,
) -> aws_sdk_dynamodb::types::ConditionCheck {
    repo.entity_revision_data_condition(
        trip_pk(&snapshot.trip_id),
        member_sk(owner),
        MEMBER_ENTITY,
        snapshot.revision,
        &snapshot.data,
    )
}

pub(super) fn usage_update(
    repo: &DynamoUserRepo,
    digest: &str,
    bucket: &str,
    used_at: &str,
    bucket_expires_at: &str,
    ttl: i64,
) -> Update {
    Update::builder()
        .table_name(&repo.table_name)
        .set_key(Some(crate::dynamodb::primitives::item_key(
            usage_pk(digest),
            usage_sk(bucket),
        )))
        .update_expression(
            "SET #entity = if_not_exists(#entity, :entity), #schema = if_not_exists(#schema, :schema), #digest = if_not_exists(#digest, :digest), #count = if_not_exists(#count, :zero) + :one, #last = :last, #bucket_expiry = if_not_exists(#bucket_expiry, :bucket_expiry), #ttl = if_not_exists(#ttl, :ttl)",
        )
        .condition_expression(
            "(attribute_not_exists(#pk) AND attribute_not_exists(#sk)) OR (#entity = :entity AND #schema = :schema AND #digest = :digest AND #bucket_expiry = :bucket_expiry AND #ttl = :ttl AND #count < :limit)",
        )
        .expression_attribute_names("#pk", PK)
        .expression_attribute_names("#sk", SK)
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#schema", SCHEMA_VERSION)
        .expression_attribute_names("#digest", COMMON_NAME_DIGEST)
        .expression_attribute_names("#count", COUNT)
        .expression_attribute_names("#last", LAST_USED_AT)
        .expression_attribute_names("#bucket_expiry", BUCKET_EXPIRES_AT)
        .expression_attribute_names("#ttl", TTL)
        .expression_attribute_values(":entity", AttributeValue::S(SERVICE_USAGE_ENTITY.into()))
        .expression_attribute_values(":schema", AttributeValue::N(CURRENT_SCHEMA_VERSION.into()))
        .expression_attribute_values(":digest", AttributeValue::S(digest.into()))
        .expression_attribute_values(":zero", AttributeValue::N("0".into()))
        .expression_attribute_values(":one", AttributeValue::N("1".into()))
        .expression_attribute_values(":last", AttributeValue::S(used_at.into()))
        .expression_attribute_values(":bucket_expiry", AttributeValue::S(bucket_expires_at.into()))
        .expression_attribute_values(":ttl", AttributeValue::N(ttl.to_string()))
        .expression_attribute_values(":limit", AttributeValue::N(SERVICE_REQUESTS_PER_HOUR.to_string()))
        .build()
        .expect("service usage update is complete")
}

fn validate_reciprocal(
    mapping: &ServiceMappingRecord,
    claim: &ServiceClaimRecord,
    owner: &UserId,
) -> Result<(), ServiceIdentityRepoError> {
    if mapping.owner_id != owner.0
        || claim.owner_id != owner.0
        || claim.service_id != mapping.id
        || claim.common_name_digest != mapping.common_name_digest
        || (mapping.revoked_at.is_some()) != (claim.status == ClaimStatus::Revoked)
    {
        return Err(ServiceIdentityRepoError::CorruptData);
    }
    Ok(())
}

fn same_optional_snapshot(
    first: &Option<std::collections::HashMap<String, AttributeValue>>,
    second: &Option<std::collections::HashMap<String, AttributeValue>>,
) -> Result<bool, ServiceIdentityRepoError> {
    match (first, second) {
        (None, None) => Ok(true),
        (Some(first), Some(second)) => {
            Ok(first.get(REVISION) == second.get(REVISION)
                && meta_data(first)? == meta_data(second)?)
        }
        _ => Ok(false),
    }
}

fn utc(value: &str) -> Result<DateTime<Utc>, ServiceIdentityRepoError> {
    let parsed =
        DateTime::parse_from_rfc3339(value).map_err(|_| ServiceIdentityRepoError::CorruptData)?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(ServiceIdentityRepoError::CorruptData);
    }
    Ok(parsed.with_timezone(&Utc))
}

fn action_condition_failed(error: Option<&TransactWriteItemsError>, index: usize) -> bool {
    let Some(TransactWriteItemsError::TransactionCanceledException(cancellation)) = error else {
        return false;
    };
    cancellation
        .cancellation_reasons()
        .get(index)
        .and_then(|reason| reason.code())
        == Some(CONDITIONAL_FAILURE)
}

fn any_condition_failed(error: Option<&TransactWriteItemsError>) -> bool {
    let Some(TransactWriteItemsError::TransactionCanceledException(cancellation)) = error else {
        return false;
    };
    cancellation
        .cancellation_reasons()
        .iter()
        .any(|reason| reason.code() == Some(CONDITIONAL_FAILURE))
}

fn any_transaction_conflict(error: Option<&TransactWriteItemsError>) -> bool {
    let Some(TransactWriteItemsError::TransactionCanceledException(cancellation)) = error else {
        return false;
    };
    cancellation
        .cancellation_reasons()
        .iter()
        .any(|reason| reason.code() == Some(TRANSACTION_CONFLICT))
}

pub(super) fn next_revision(current: u64) -> Result<u64, ServiceIdentityRepoError> {
    current
        .checked_add(1)
        .ok_or(ServiceIdentityRepoError::CorruptData)
}
