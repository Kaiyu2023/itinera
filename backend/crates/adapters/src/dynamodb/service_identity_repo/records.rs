use std::{collections::HashMap, fmt::Write};

use aws_sdk_dynamodb::types::AttributeValue;
use itinera_core::{
    domain::{
        service_identity::{ServiceIdentity, ServiceScope},
        trip::{TripMember, TripRole},
        user::UserId,
    },
    ports::service_identity::ServiceIdentityRepoError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dynamodb::{
    CURRENT_SCHEMA_VERSION, ENTITY_TYPE, PK, SCHEMA_VERSION, SK, USER_ID,
    trip_repo::records::{
        DATA, GSI1PK, GSI1SK, MEMBER_ENTITY, ROLE, Stored, decode_record, encode_record, member_sk,
        role_value, string, trip_pk,
    },
    user_partition_key,
};

pub(super) const SERVICE_META_SK: &str = "SERVICE_IDENTITY_META";
pub(super) const SERVICE_PREFIX: &str = "SERVICE_IDENTITY#";
pub(super) const CLAIM_SK: &str = "CLAIM";
pub(super) const USAGE_PREFIX: &str = "BUCKET#";
pub(super) const SERVICE_META_ENTITY: &str = "SERVICE_IDENTITY_META";
pub(super) const SERVICE_MAPPING_ENTITY: &str = "SERVICE_IDENTITY";
pub(super) const SERVICE_CLAIM_ENTITY: &str = "SERVICE_IDENTITY_CLAIM";
pub(super) const SERVICE_USAGE_ENTITY: &str = "SERVICE_IDENTITY_USAGE";
pub(super) const COMMON_NAME_DIGEST: &str = "common_name_digest";
pub(super) const COUNT: &str = "request_count";
pub(super) const LAST_USED_AT: &str = "last_used_at";
pub(super) const BUCKET_EXPIRES_AT: &str = "bucket_expires_at";
pub(super) const TTL: &str = "ttl";

const CLAIM_KEY_PREFIX: &str = "SERVICE_CLAIM#";
const USAGE_KEY_PREFIX: &str = "SERVICE_USAGE#";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ServiceMetaRecord {
    pub(super) owner_id: String,
    pub(super) count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ServiceMappingRecord {
    pub(super) id: String,
    pub(super) owner_id: String,
    pub(super) name: String,
    pub(super) client_id_hint: String,
    pub(super) common_name_digest: String,
    pub(super) scopes: Vec<ServiceScope>,
    pub(super) trip_ids: Vec<String>,
    pub(super) expires_at: String,
    pub(super) revoked_at: Option<String>,
    pub(super) created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ClaimStatus {
    Active,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ServiceClaimRecord {
    pub(super) common_name_digest: String,
    pub(super) owner_id: String,
    pub(super) service_id: String,
    pub(super) status: ClaimStatus,
}

#[derive(Debug, Clone)]
pub(super) struct MembershipSnapshot {
    pub(super) trip_id: String,
    pub(super) revision: u64,
    pub(super) data: String,
}

#[derive(Debug, Clone)]
pub(super) struct UsageRecord {
    pub(super) count: u32,
    pub(super) last_used_at: String,
}

impl ServiceMappingRecord {
    pub(super) fn from_new(
        owner: &UserId,
        identity: &ServiceIdentity,
        common_name_digest: String,
    ) -> Self {
        Self {
            id: identity.id.clone(),
            owner_id: owner.0.clone(),
            name: identity.name.clone(),
            client_id_hint: identity.client_id_hint.clone(),
            common_name_digest,
            scopes: identity.scopes.clone(),
            trip_ids: identity.trip_ids.clone(),
            expires_at: identity.expires_at.clone(),
            revoked_at: identity.revoked_at.clone(),
            created_at: identity.created_at.clone(),
        }
    }

    pub(super) fn identity(&self, last_used_at: Option<String>) -> ServiceIdentity {
        ServiceIdentity {
            id: self.id.clone(),
            name: self.name.clone(),
            client_id_hint: self.client_id_hint.clone(),
            scopes: self.scopes.clone(),
            trip_ids: self.trip_ids.clone(),
            expires_at: self.expires_at.clone(),
            last_used_at,
            revoked_at: self.revoked_at.clone(),
            created_at: self.created_at.clone(),
        }
    }
}

pub(super) fn service_sk(service_id: &str) -> String {
    format!("{SERVICE_PREFIX}{service_id}")
}

pub(super) fn claim_pk(digest: &str) -> String {
    format!("{CLAIM_KEY_PREFIX}{digest}")
}

pub(super) fn usage_pk(digest: &str) -> String {
    format!("{USAGE_KEY_PREFIX}{digest}")
}

pub(super) fn usage_sk(bucket: &str) -> String {
    format!("{USAGE_PREFIX}{bucket}")
}

pub(super) fn common_name_digest(common_name: &str) -> String {
    let digest = Sha256::digest(common_name.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String is infallible");
    }
    encoded
}

pub(super) fn encode_meta(
    owner: &UserId,
    count: u32,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, ServiceIdentityRepoError> {
    encode_record(
        user_partition_key(owner),
        SERVICE_META_SK.into(),
        SERVICE_META_ENTITY,
        &ServiceMetaRecord {
            owner_id: owner.0.clone(),
            count,
        },
        revision,
    )
    .map_err(record_error)
}

pub(super) fn encode_mapping(
    owner: &UserId,
    record: &ServiceMappingRecord,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, ServiceIdentityRepoError> {
    encode_record(
        user_partition_key(owner),
        service_sk(&record.id),
        SERVICE_MAPPING_ENTITY,
        record,
        revision,
    )
    .map_err(record_error)
}

pub(super) fn encode_claim(
    record: &ServiceClaimRecord,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, ServiceIdentityRepoError> {
    encode_record(
        claim_pk(&record.common_name_digest),
        CLAIM_SK.into(),
        SERVICE_CLAIM_ENTITY,
        record,
        revision,
    )
    .map_err(record_error)
}

pub(super) fn decode_meta(
    item: &HashMap<String, AttributeValue>,
    owner: &UserId,
) -> Result<Stored<ServiceMetaRecord>, ServiceIdentityRepoError> {
    let stored: Stored<ServiceMetaRecord> = decode_record(
        item,
        &user_partition_key(owner),
        SERVICE_META_SK,
        SERVICE_META_ENTITY,
    )
    .map_err(record_error)?;
    if stored.revision == 0 || stored.value.owner_id != owner.0 {
        return Err(ServiceIdentityRepoError::CorruptData);
    }
    Ok(stored)
}

pub(super) fn decode_mapping(
    item: &HashMap<String, AttributeValue>,
    owner: &UserId,
    service_id: &str,
) -> Result<Stored<ServiceMappingRecord>, ServiceIdentityRepoError> {
    let stored: Stored<ServiceMappingRecord> = decode_record(
        item,
        &user_partition_key(owner),
        &service_sk(service_id),
        SERVICE_MAPPING_ENTITY,
    )
    .map_err(record_error)?;
    if stored.revision == 0
        || stored.value.id != service_id
        || stored.value.owner_id != owner.0
        || stored.value.common_name_digest.len() != 64
        || !stored
            .value
            .common_name_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ServiceIdentityRepoError::CorruptData);
    }
    Ok(stored)
}

pub(super) fn decode_claim(
    item: &HashMap<String, AttributeValue>,
    digest: &str,
) -> Result<Stored<ServiceClaimRecord>, ServiceIdentityRepoError> {
    let stored: Stored<ServiceClaimRecord> =
        decode_record(item, &claim_pk(digest), CLAIM_SK, SERVICE_CLAIM_ENTITY)
            .map_err(record_error)?;
    if stored.revision == 0
        || stored.value.common_name_digest != digest
        || stored.value.owner_id.is_empty()
        || stored.value.service_id.is_empty()
    {
        return Err(ServiceIdentityRepoError::CorruptData);
    }
    Ok(stored)
}

pub(super) fn decode_membership(
    item: &HashMap<String, AttributeValue>,
    trip_id: &str,
    owner: &UserId,
    require_editor: bool,
) -> Result<MembershipSnapshot, ServiceIdentityRepoError> {
    let pk = trip_pk(trip_id);
    let sk = member_sk(owner);
    let stored: Stored<TripMember> =
        decode_record(item, &pk, &sk, MEMBER_ENTITY).map_err(record_error)?;
    if stored.revision == 0
        || stored.value.user_id != owner.0
        || string(item, USER_ID).map_err(record_error)? != owner.0
        || string(item, ROLE).map_err(record_error)? != role_value(stored.value.role)
        || string(item, GSI1PK).map_err(record_error)? != user_partition_key(owner)
        || string(item, GSI1SK).map_err(record_error)? != format!("TRIP#{trip_id}")
    {
        return Err(ServiceIdentityRepoError::CorruptData);
    }
    if require_editor && !stored.value.role.can_edit() {
        return Err(ServiceIdentityRepoError::Forbidden);
    }
    Ok(MembershipSnapshot {
        trip_id: trip_id.to_string(),
        revision: stored.revision,
        data: string(item, DATA).map_err(record_error)?,
    })
}

pub(super) fn decode_usage(
    item: &HashMap<String, AttributeValue>,
    digest: &str,
    bucket: &str,
    expected_bucket_expiry: &str,
    expected_ttl: i64,
) -> Result<UsageRecord, ServiceIdentityRepoError> {
    let expected_ttl =
        u64::try_from(expected_ttl).map_err(|_| ServiceIdentityRepoError::CorruptData)?;
    if string_value(item, PK)? != usage_pk(digest)
        || string_value(item, SK)? != usage_sk(bucket)
        || string_value(item, ENTITY_TYPE)? != SERVICE_USAGE_ENTITY
        || number_string(item, SCHEMA_VERSION)? != CURRENT_SCHEMA_VERSION
        || string_value(item, COMMON_NAME_DIGEST)? != digest
        || string_value(item, BUCKET_EXPIRES_AT)? != expected_bucket_expiry
        || number_u64(item, TTL)? != expected_ttl
    {
        return Err(ServiceIdentityRepoError::CorruptData);
    }
    let count = number_u64(item, COUNT)?
        .try_into()
        .map_err(|_| ServiceIdentityRepoError::CorruptData)?;
    if count == 0 {
        return Err(ServiceIdentityRepoError::CorruptData);
    }
    Ok(UsageRecord {
        count,
        last_used_at: string_value(item, LAST_USED_AT)?,
    })
}

pub(super) fn mapping_data(
    item: &HashMap<String, AttributeValue>,
) -> Result<String, ServiceIdentityRepoError> {
    string_value(item, DATA)
}

pub(super) fn meta_data(
    item: &HashMap<String, AttributeValue>,
) -> Result<String, ServiceIdentityRepoError> {
    string_value(item, DATA)
}

pub(super) fn claim_data(
    item: &HashMap<String, AttributeValue>,
) -> Result<String, ServiceIdentityRepoError> {
    string_value(item, DATA)
}

fn string_value(
    item: &HashMap<String, AttributeValue>,
    name: &str,
) -> Result<String, ServiceIdentityRepoError> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .cloned()
        .ok_or(ServiceIdentityRepoError::CorruptData)
}

fn number_string<'a>(
    item: &'a HashMap<String, AttributeValue>,
    name: &str,
) -> Result<&'a str, ServiceIdentityRepoError> {
    item.get(name)
        .and_then(|value| value.as_n().ok())
        .map(String::as_str)
        .ok_or(ServiceIdentityRepoError::CorruptData)
}

fn number_u64(
    item: &HashMap<String, AttributeValue>,
    name: &str,
) -> Result<u64, ServiceIdentityRepoError> {
    number_string(item, name)?
        .parse()
        .map_err(|_| ServiceIdentityRepoError::CorruptData)
}

fn record_error(_: itinera_core::ports::trip::TripRepoError) -> ServiceIdentityRepoError {
    ServiceIdentityRepoError::CorruptData
}

pub(super) fn required_role(scopes: &[ServiceScope]) -> TripRole {
    if scopes.contains(&ServiceScope::Propose) {
        TripRole::Member
    } else {
        TripRole::Viewer
    }
}
