use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use aws_sdk_dynamodb::{
    operation::transact_write_items::TransactWriteItemsError,
    types::{AttributeValue, ConditionCheck, Delete, Put, TransactWriteItem, Update},
};
use itinera_core::{
    domain::{
        trip::{
            Candidate, CandidateDisposition, CandidateStatus, CandidateWithPlace, Day,
            DayFeasibility, DayPatch, Feasibility, Invite, InviteStatus, Place, Plan, PlanDetail,
            SoftBudget, Stop, StopKind, StopPatch, Trip, TripMember, TripRole, TripStatus,
            TripSummary,
        },
        user::{Email, User, UserId},
    },
    ports::{
        trip::{CandidateUpdate, TripRepo, TripRepoError},
        user::{UserRepo, UserRepoError},
    },
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    CONDITIONAL_FAILURE, CURRENT_SCHEMA_VERSION, DynamoUserRepo, ENTITY_TYPE, MEMBERSHIP_COUNT, PK,
    SCHEMA_VERSION, SK, USER_ID, USER_PROFILE_ENTITY, USER_PROFILE_SK, user_partition_key,
};

const DATA: &str = "data";
const REVISION: &str = "revision";
const ROLE: &str = "role";
const GSI1PK: &str = "gsi1pk";
const GSI1SK: &str = "gsi1sk";
const MEMBER_COUNT: &str = "member_count";
const LEADER_COUNT: &str = "leader_count";
const CURRENT_PLAN_ID: &str = "current_plan_id";
const CURRENT_PLAN_VERSION: &str = "current_plan_version";

const META_SK: &str = "META";
const TRIP_ENTITY: &str = "TRIP";
const MEMBER_ENTITY: &str = "TRIP_MEMBER";
const INVITE_ENTITY: &str = "TRIP_INVITE";
const INVITE_LOOKUP_ENTITY: &str = "INVITE_LOOKUP";
const PLACE_ENTITY: &str = "PLACE";
const CANDIDATE_ENTITY: &str = "CANDIDATE";
const PLAN_ENTITY: &str = "PLAN";
const DAY_ENTITY: &str = "DAY";
const STOP_ENTITY: &str = "STOP";
const AUDIT_ENTITY: &str = "CONTENT_AUDIT";

const GSI_NAME: &str = "gsi1";
const USER_TRIPS_PAGE_SIZE: i32 = 50;
const TRIP_COLLECTION_PAGE_SIZE: i32 = 500;
const INVITE_ACCEPT_ATTEMPTS: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TripMeta {
    id: String,
    name: String,
    cover_photo_url: Option<String>,
    accent_color: Option<String>,
    stop_kind_labels: Option<HashMap<StopKind, String>>,
    status: TripStatus,
    start_date: String,
    end_date: String,
    base_currency: String,
    soft_budget: Option<SoftBudget>,
    current_plan_id: Option<String>,
    current_plan_version: Option<u32>,
    created_at: String,
    member_count: u32,
    leader_count: u32,
    cities: Vec<String>,
}

impl TripMeta {
    fn from_trip(trip: &Trip) -> Self {
        Self {
            id: trip.id.clone(),
            name: trip.name.clone(),
            cover_photo_url: trip.cover_photo_url.clone(),
            accent_color: trip.accent_color.clone(),
            stop_kind_labels: trip.stop_kind_labels.clone(),
            status: trip.status,
            start_date: trip.start_date.clone(),
            end_date: trip.end_date.clone(),
            base_currency: trip.base_currency.clone(),
            soft_budget: trip.soft_budget.clone(),
            current_plan_id: trip.current_plan_id.clone(),
            current_plan_version: None,
            created_at: trip.created_at.clone(),
            member_count: trip.members.len() as u32,
            leader_count: trip
                .members
                .iter()
                .filter(|member| member.role == TripRole::Leader)
                .count() as u32,
            cities: vec![],
        }
    }

    fn into_trip(self, members: Vec<TripMember>) -> Trip {
        Trip {
            id: self.id,
            name: self.name,
            cover_photo_url: self.cover_photo_url,
            accent_color: self.accent_color,
            stop_kind_labels: self.stop_kind_labels,
            status: self.status,
            start_date: self.start_date,
            end_date: self.end_date,
            base_currency: self.base_currency,
            soft_budget: self.soft_budget,
            members,
            current_plan_id: self.current_plan_id,
            created_at: self.created_at,
        }
    }

    fn summary(&self) -> TripSummary {
        TripSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            cover_photo_url: self.cover_photo_url.clone(),
            accent_color: self.accent_color.clone(),
            status: self.status,
            start_date: self.start_date.clone(),
            end_date: self.end_date.clone(),
            member_count: self.member_count,
            cities: self.cities.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InviteLookup {
    trip_id: String,
    invite_sort_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentAudit {
    id: String,
    trip_id: String,
    entity: String,
    entity_id: String,
    field: String,
    old_value: Value,
    new_value: Value,
    author: String,
    source: AuditSource,
    status: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditSource {
    via: String,
}

struct Stored<T> {
    value: T,
    revision: u64,
    sort_key: String,
}

fn trip_pk(trip_id: &str) -> String {
    format!("TRIP#{trip_id}")
}

fn member_sk(user_id: &UserId) -> String {
    format!("MEMBER#{}", user_id.0)
}

fn place_sk(place_id: &str) -> String {
    format!("PLACE#{place_id}")
}

fn candidate_sk(candidate_id: &str) -> String {
    format!("CANDIDATE#{candidate_id}")
}

fn plan_prefix(version: u32) -> String {
    format!("PLAN#{version:010}")
}

fn plan_sk(version: u32) -> String {
    format!("{}#META", plan_prefix(version))
}

fn day_sk(version: u32, day: &Day) -> String {
    format!("{}#DAY#{}#{}", plan_prefix(version), day.date, day.id)
}

fn audit_sk(at: &str, id: &str) -> String {
    format!("AUDIT#{at}#{id}")
}

fn email_digest(email: &Email) -> String {
    let digest = Sha256::digest(email.as_str().as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invite_sk(email: &Email) -> String {
    format!("INVITE#{}", email_digest(email))
}

fn invitee_pk(email: &Email) -> String {
    format!("INVITEE#{}", email_digest(email))
}

fn invite_lookup_sk(trip_id: &str) -> String {
    format!("TRIP#{trip_id}")
}

fn string(item: &HashMap<String, AttributeValue>, name: &str) -> Result<String, TripRepoError> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .cloned()
        .ok_or(TripRepoError::CorruptData)
}

fn number_u64(item: &HashMap<String, AttributeValue>, name: &str) -> Result<u64, TripRepoError> {
    item.get(name)
        .and_then(|value| value.as_n().ok())
        .and_then(|value| value.parse().ok())
        .ok_or(TripRepoError::CorruptData)
}

fn encode_record<T: Serialize>(
    partition_key: String,
    sort_key: String,
    entity: &str,
    value: &T,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, TripRepoError> {
    let data = serde_json::to_string(value).map_err(|_| TripRepoError::CorruptData)?;
    Ok(HashMap::from([
        (PK.to_string(), AttributeValue::S(partition_key)),
        (SK.to_string(), AttributeValue::S(sort_key)),
        (
            ENTITY_TYPE.to_string(),
            AttributeValue::S(entity.to_string()),
        ),
        (
            SCHEMA_VERSION.to_string(),
            AttributeValue::N(CURRENT_SCHEMA_VERSION.to_string()),
        ),
        (
            REVISION.to_string(),
            AttributeValue::N(revision.to_string()),
        ),
        (DATA.to_string(), AttributeValue::S(data)),
    ]))
}

fn decode_record<T: DeserializeOwned>(
    item: &HashMap<String, AttributeValue>,
    expected_pk: &str,
    expected_sk: &str,
    expected_entity: &str,
) -> Result<Stored<T>, TripRepoError> {
    if string(item, PK)? != expected_pk
        || string(item, SK)? != expected_sk
        || string(item, ENTITY_TYPE)? != expected_entity
        || number_u64(item, SCHEMA_VERSION)?.to_string() != CURRENT_SCHEMA_VERSION
    {
        return Err(TripRepoError::CorruptData);
    }
    let value =
        serde_json::from_str(&string(item, DATA)?).map_err(|_| TripRepoError::CorruptData)?;
    Ok(Stored {
        value,
        revision: number_u64(item, REVISION)?,
        sort_key: expected_sk.to_string(),
    })
}

fn add_trip_meta_attributes(item: &mut HashMap<String, AttributeValue>, meta: &TripMeta) {
    item.insert(
        MEMBER_COUNT.to_string(),
        AttributeValue::N(meta.member_count.to_string()),
    );
    item.insert(
        LEADER_COUNT.to_string(),
        AttributeValue::N(meta.leader_count.to_string()),
    );
    if let Some(id) = &meta.current_plan_id {
        item.insert(CURRENT_PLAN_ID.to_string(), AttributeValue::S(id.clone()));
    }
    if let Some(version) = meta.current_plan_version {
        item.insert(
            CURRENT_PLAN_VERSION.to_string(),
            AttributeValue::N(version.to_string()),
        );
    }
}

fn encode_trip_meta(
    meta: &TripMeta,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, TripRepoError> {
    let mut item = encode_record(
        trip_pk(&meta.id),
        META_SK.into(),
        TRIP_ENTITY,
        meta,
        revision,
    )?;
    add_trip_meta_attributes(&mut item, meta);
    Ok(item)
}

fn encode_member(
    trip_id: &str,
    member: &TripMember,
) -> Result<HashMap<String, AttributeValue>, TripRepoError> {
    let user_id = UserId(member.user_id.clone());
    let mut item = encode_record(
        trip_pk(trip_id),
        member_sk(&user_id),
        MEMBER_ENTITY,
        member,
        1,
    )?;
    item.insert(
        USER_ID.to_string(),
        AttributeValue::S(member.user_id.clone()),
    );
    item.insert(
        ROLE.to_string(),
        AttributeValue::S(role_value(member.role).to_string()),
    );
    item.insert(
        GSI1PK.to_string(),
        AttributeValue::S(user_partition_key(&user_id)),
    );
    item.insert(
        GSI1SK.to_string(),
        AttributeValue::S(format!("TRIP#{trip_id}")),
    );
    Ok(item)
}

fn role_value(role: TripRole) -> &'static str {
    match role {
        TripRole::Leader => "leader",
        TripRole::Member => "member",
        TripRole::Viewer => "viewer",
    }
}

fn create_put(table_name: &str, item: HashMap<String, AttributeValue>) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
        .expression_attribute_names("#pk", PK)
        .expression_attribute_names("#sk", SK)
        .build()
        .expect("table and item are present")
}

fn revision_put(
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

fn member_condition(
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

fn record_revision_condition(
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

fn user_membership_count_update(table_name: &str, user_id: &UserId, increment: bool) -> Update {
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

fn transaction_condition_failed(error: Option<&TransactWriteItemsError>) -> bool {
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
enum RequiredRole {
    Any,
    Editor,
    Leader,
}

impl DynamoUserRepo {
    async fn trip_get(
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

    async fn query_partition(
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

    async fn get_member_record(
        &self,
        trip_id: &str,
        user_id: &UserId,
    ) -> Result<Option<Stored<TripMember>>, TripRepoError> {
        let pk = trip_pk(trip_id);
        let sk = member_sk(user_id);
        let Some(item) = self.trip_get(&pk, &sk).await? else {
            return Ok(None);
        };
        let stored: Stored<TripMember> = decode_record(&item, &pk, &sk, MEMBER_ENTITY)?;
        if stored.value.user_id != user_id.0
            || string(&item, USER_ID)? != user_id.0
            || string(&item, ROLE)? != role_value(stored.value.role)
            || string(&item, GSI1PK)? != user_partition_key(user_id)
            || string(&item, GSI1SK)? != format!("TRIP#{trip_id}")
        {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Some(stored))
    }

    async fn authorize(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredRole,
    ) -> Result<TripRole, TripRepoError> {
        let role = self
            .get_member_record(trip_id, actor)
            .await?
            .ok_or(TripRepoError::NotFound)?
            .value
            .role;
        match required {
            RequiredRole::Any => Ok(role),
            RequiredRole::Editor if role.can_edit() => Ok(role),
            RequiredRole::Leader if role == TripRole::Leader => Ok(role),
            _ => Err(TripRepoError::Forbidden),
        }
    }

    async fn get_trip_meta(&self, trip_id: &str) -> Result<Stored<TripMeta>, TripRepoError> {
        let pk = trip_pk(trip_id);
        let item = self
            .trip_get(&pk, META_SK)
            .await?
            .ok_or(TripRepoError::NotFound)?;
        let stored: Stored<TripMeta> = decode_record(&item, &pk, META_SK, TRIP_ENTITY)?;
        let current_id_matches = match &stored.value.current_plan_id {
            Some(id) => string(&item, CURRENT_PLAN_ID).is_ok_and(|stored_id| stored_id == *id),
            None => !item.contains_key(CURRENT_PLAN_ID),
        };
        let current_version_matches = match stored.value.current_plan_version {
            Some(version) => number_u64(&item, CURRENT_PLAN_VERSION) == Ok(version.into()),
            None => !item.contains_key(CURRENT_PLAN_VERSION),
        };
        if stored.value.id != trip_id
            || stored.value.member_count == 0
            || stored.value.leader_count == 0
            || number_u64(&item, MEMBER_COUNT) != Ok(stored.value.member_count.into())
            || number_u64(&item, LEADER_COUNT) != Ok(stored.value.leader_count.into())
            || !current_id_matches
            || !current_version_matches
        {
            return Err(TripRepoError::CorruptData);
        }
        Ok(stored)
    }

    async fn accept_invite_lookup(
        &self,
        user: &User,
        joined_at: &str,
        trip_id: &str,
        lookup_sort_key: &str,
        invite_sort_key: &str,
    ) -> Result<(), TripRepoError> {
        let lookup_pk = invitee_pk(&user.email);
        let trip_partition = trip_pk(trip_id);
        let expected_lookup_sk = invite_lookup_sk(trip_id);
        let expected_invite_sk = invite_sk(&user.email);
        if lookup_sort_key != expected_lookup_sk || invite_sort_key != expected_invite_sk {
            return Err(TripRepoError::CorruptData);
        }

        for _ in 0..INVITE_ACCEPT_ATTEMPTS {
            let Some(lookup_item) = self.trip_get(&lookup_pk, lookup_sort_key).await? else {
                // Another `/me` may have atomically accepted this invite after
                // the initial query. Confirm its terminal state rather than
                // surfacing a spurious conflict.
                if let Some(invite_item) = self.trip_get(&trip_partition, invite_sort_key).await? {
                    let invite: Stored<Invite> = decode_record(
                        &invite_item,
                        &trip_partition,
                        invite_sort_key,
                        INVITE_ENTITY,
                    )?;
                    if invite.value.trip_id != trip_id || invite.value.email != user.email.as_str()
                    {
                        return Err(TripRepoError::CorruptData);
                    }
                    if invite.value.status == InviteStatus::Accepted {
                        return Ok(());
                    }
                }
                // A simultaneous reinvite can recreate the lookup between the
                // two strong reads. Retry the complete state transition.
                continue;
            };
            let lookup: Stored<InviteLookup> = decode_record(
                &lookup_item,
                &lookup_pk,
                lookup_sort_key,
                INVITE_LOOKUP_ENTITY,
            )?;
            if lookup.value.trip_id != trip_id || lookup.value.invite_sort_key != invite_sort_key {
                return Err(TripRepoError::CorruptData);
            }

            let invite_item = self
                .trip_get(&trip_partition, invite_sort_key)
                .await?
                .ok_or(TripRepoError::CorruptData)?;
            let stored_invite: Stored<Invite> = decode_record(
                &invite_item,
                &trip_partition,
                invite_sort_key,
                INVITE_ENTITY,
            )?;
            if stored_invite.value.trip_id != trip_id
                || stored_invite.value.email != user.email.as_str()
            {
                return Err(TripRepoError::CorruptData);
            }
            if stored_invite.value.status == InviteStatus::Accepted {
                // This can only be a cross-item read racing the atomic delete;
                // the accepted invite is the authoritative terminal state.
                return Ok(());
            }

            let member = self.get_member_record(trip_id, &user.id).await?;
            let mut accepted = stored_invite.value;
            accepted.status = InviteStatus::Accepted;
            let delete_lookup = Delete::builder()
                .table_name(&self.table_name)
                .key(PK, AttributeValue::S(lookup_pk.clone()))
                .key(SK, AttributeValue::S(lookup_sort_key.to_string()))
                .condition_expression("#revision = :revision")
                .expression_attribute_names("#revision", REVISION)
                .expression_attribute_values(
                    ":revision",
                    AttributeValue::N(lookup.revision.to_string()),
                )
                .build()
                .expect("lookup delete is complete");

            let result = if member.is_some() {
                // Recheck that membership still exists in the same transaction;
                // otherwise retry through the membership-creation branch.
                self.client
                    .transact_write_items()
                    .transact_items(action_condition(member_condition(
                        &self.table_name,
                        trip_id,
                        &user.id,
                        RequiredRole::Any,
                    )))
                    .transact_items(action_put(revision_put(
                        &self.table_name,
                        encode_record(
                            trip_partition.clone(),
                            invite_sort_key.to_string(),
                            INVITE_ENTITY,
                            &accepted,
                            stored_invite.revision + 1,
                        )?,
                        stored_invite.revision,
                    )))
                    .transact_items(TransactWriteItem::builder().delete(delete_lookup).build())
                    .send()
                    .await
            } else {
                let stored_meta = self.get_trip_meta(trip_id).await?;
                let mut meta = stored_meta.value;
                meta.member_count = meta
                    .member_count
                    .checked_add(1)
                    .ok_or(TripRepoError::CorruptData)?;
                let member = TripMember {
                    user_id: user.id.0.clone(),
                    role: TripRole::Member,
                    joined_at: joined_at.to_string(),
                };
                self.client
                    .transact_write_items()
                    .transact_items(action_put(create_put(
                        &self.table_name,
                        encode_member(trip_id, &member)?,
                    )))
                    .transact_items(action_put(revision_put(
                        &self.table_name,
                        encode_trip_meta(&meta, stored_meta.revision + 1)?,
                        stored_meta.revision,
                    )))
                    .transact_items(action_put(revision_put(
                        &self.table_name,
                        encode_record(
                            trip_partition.clone(),
                            invite_sort_key.to_string(),
                            INVITE_ENTITY,
                            &accepted,
                            stored_invite.revision + 1,
                        )?,
                        stored_invite.revision,
                    )))
                    .transact_items(TransactWriteItem::builder().delete(delete_lookup).build())
                    .transact_items(action_update(user_membership_count_update(
                        &self.table_name,
                        &user.id,
                        true,
                    )))
                    .send()
                    .await
            };

            match result {
                Ok(_) => return Ok(()),
                Err(error) if transaction_condition_failed(error.as_service_error()) => {
                    // Membership, invite, lookup, or trip metadata changed.
                    // Re-read all of them; ordinary contention and concurrent
                    // `/me` requests must remain idempotent.
                }
                Err(_) => return Err(TripRepoError::Unavailable),
            }
        }

        // Persistent contention is retryable and `/me` documents 503, not a
        // domain-level 409 for account bootstrap.
        Err(TripRepoError::Unavailable)
    }

    async fn get_members_for_trip(&self, trip_id: &str) -> Result<Vec<TripMember>, TripRepoError> {
        let pk = trip_pk(trip_id);
        self.query_partition(&pk, "MEMBER#", TRIP_COLLECTION_PAGE_SIZE)
            .await?
            .into_iter()
            .map(|item| {
                let sk = string(&item, SK)?;
                let stored: Stored<TripMember> = decode_record(&item, &pk, &sk, MEMBER_ENTITY)?;
                let user_id = UserId(stored.value.user_id.clone());
                if sk == member_sk(&user_id)
                    && string(&item, USER_ID)? == user_id.0
                    && string(&item, ROLE)? == role_value(stored.value.role)
                    && string(&item, GSI1PK)? == user_partition_key(&user_id)
                    && string(&item, GSI1SK)? == format!("TRIP#{trip_id}")
                {
                    Ok(stored.value)
                } else {
                    Err(TripRepoError::CorruptData)
                }
            })
            .collect()
    }

    async fn get_candidate_record(
        &self,
        trip_id: &str,
        candidate_id: &str,
    ) -> Result<Option<Stored<Candidate>>, TripRepoError> {
        let pk = trip_pk(trip_id);
        let sk = candidate_sk(candidate_id);
        let Some(item) = self.trip_get(&pk, &sk).await? else {
            return Ok(None);
        };
        let stored: Stored<Candidate> = decode_record(&item, &pk, &sk, CANDIDATE_ENTITY)?;
        if stored.value.id != candidate_id || stored.value.trip_id != trip_id {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Some(stored))
    }

    async fn get_place_record(
        &self,
        trip_id: &str,
        place_id: &str,
    ) -> Result<Option<Stored<Place>>, TripRepoError> {
        let pk = trip_pk(trip_id);
        let sk = place_sk(place_id);
        let Some(item) = self.trip_get(&pk, &sk).await? else {
            return Ok(None);
        };
        let stored: Stored<Place> = decode_record(&item, &pk, &sk, PLACE_ENTITY)?;
        if stored.value.id != place_id {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Some(stored))
    }

    async fn get_plan_detail_unchecked(
        &self,
        trip_id: &str,
        meta: &TripMeta,
    ) -> Result<PlanDetail, TripRepoError> {
        let version = meta.current_plan_version.ok_or(TripRepoError::NotFound)?;
        let expected_plan_id = meta
            .current_plan_id
            .as_ref()
            .ok_or(TripRepoError::CorruptData)?;
        let pk = trip_pk(trip_id);
        let prefix = format!("{}#", plan_prefix(version));
        let items = self
            .query_partition(&pk, &prefix, TRIP_COLLECTION_PAGE_SIZE)
            .await?;
        let mut plan = None;
        let mut days = Vec::new();
        let mut stops = Vec::new();
        for item in items {
            let sk = string(&item, SK)?;
            match string(&item, ENTITY_TYPE)?.as_str() {
                PLAN_ENTITY => {
                    let value: Stored<Plan> = decode_record(&item, &pk, &sk, PLAN_ENTITY)?;
                    plan = Some(value.value);
                }
                DAY_ENTITY => {
                    let value: Stored<Day> = decode_record(&item, &pk, &sk, DAY_ENTITY)?;
                    days.push(value.value);
                }
                STOP_ENTITY => {
                    let value: Stored<Stop> = decode_record(&item, &pk, &sk, STOP_ENTITY)?;
                    stops.push(value.value);
                }
                _ => return Err(TripRepoError::CorruptData),
            }
        }
        let plan = plan.ok_or(TripRepoError::CorruptData)?;
        if &plan.id != expected_plan_id || plan.trip_id != trip_id || plan.version != version {
            return Err(TripRepoError::CorruptData);
        }
        if days.iter().any(|day| day.plan_id != plan.id) {
            return Err(TripRepoError::CorruptData);
        }
        let day_ids = days
            .iter()
            .map(|day| day.id.as_str())
            .collect::<HashSet<_>>();
        if stops
            .iter()
            .any(|stop| !day_ids.contains(stop.day_id.as_str()))
        {
            return Err(TripRepoError::CorruptData);
        }
        days.sort_by(|a, b| a.date.cmp(&b.date));
        stops.sort_by(|a, b| {
            a.day_id
                .cmp(&b.day_id)
                .then_with(|| a.seq.total_cmp(&b.seq))
        });
        let mut places = Vec::new();
        let mut seen = HashSet::new();
        for stop in &stops {
            if seen.insert(stop.place_id.clone()) {
                places.push(
                    self.get_place_record(trip_id, &stop.place_id)
                        .await?
                        .ok_or(TripRepoError::CorruptData)?
                        .value,
                );
            }
        }
        let day_feasibility = days
            .iter()
            .map(|day| DayFeasibility {
                day_id: day.id.clone(),
                feasibility: Feasibility::Ok,
                used_min: 0,
                window_min: window_minutes(&day.window_start, &day.window_end).unwrap_or(0),
                notes: vec![],
            })
            .collect();
        Ok(PlanDetail {
            plan,
            days,
            stops,
            legs: vec![],
            day_feasibility,
            places,
        })
    }
}

fn window_minutes(start: &str, end: &str) -> Option<u32> {
    fn minutes(value: &str) -> Option<u32> {
        let (hours, minutes) = value.split_once(':')?;
        Some(hours.parse::<u32>().ok()? * 60 + minutes.parse::<u32>().ok()?)
    }
    let start = minutes(start)?;
    let end = minutes(end)?;
    (end >= start).then_some(end - start)
}

struct AuditChange<'a> {
    entity: &'a str,
    entity_id: &'a str,
    field: &'a str,
    old_value: Value,
    new_value: Value,
}

fn audit(
    trip_id: &str,
    actor: &UserId,
    changed_at: &str,
    change_id: &str,
    change: AuditChange<'_>,
) -> ContentAudit {
    ContentAudit {
        id: change_id.to_string(),
        trip_id: trip_id.to_string(),
        entity: change.entity.to_string(),
        entity_id: change.entity_id.to_string(),
        field: change.field.to_string(),
        old_value: change.old_value,
        new_value: change.new_value,
        author: actor.0.clone(),
        source: AuditSource { via: "web".into() },
        status: "applied".into(),
        created_at: changed_at.to_string(),
    }
}

fn suffixed_id(base: &str, index: usize) -> String {
    format!("{base}-{index:02}")
}

fn action_put(put: Put) -> TransactWriteItem {
    TransactWriteItem::builder().put(put).build()
}

fn action_condition(condition: ConditionCheck) -> TransactWriteItem {
    TransactWriteItem::builder()
        .condition_check(condition)
        .build()
}

fn action_update(update: Update) -> TransactWriteItem {
    TransactWriteItem::builder().update(update).build()
}

#[async_trait]
impl TripRepo for DynamoUserRepo {
    async fn create_trip(&self, trip: Trip) -> Result<Trip, TripRepoError> {
        if trip.members.len() != 1 || trip.members[0].role != TripRole::Leader {
            return Err(TripRepoError::CorruptData);
        }
        let meta = TripMeta::from_trip(&trip);
        let actor = UserId(trip.members[0].user_id.clone());
        let result = self
            .client
            .transact_write_items()
            .transact_items(action_put(create_put(
                &self.table_name,
                encode_trip_meta(&meta, 1)?,
            )))
            .transact_items(action_put(create_put(
                &self.table_name,
                encode_member(&trip.id, &trip.members[0])?,
            )))
            .transact_items(action_update(user_membership_count_update(
                &self.table_name,
                &actor,
                true,
            )))
            .send()
            .await;
        match result {
            Ok(_) => Ok(trip),
            Err(error) if transaction_condition_failed(error.as_service_error()) => {
                Err(TripRepoError::Conflict)
            }
            Err(_) => Err(TripRepoError::Unavailable),
        }
    }

    async fn list_trips(&self, actor: &UserId) -> Result<Vec<TripSummary>, TripRepoError> {
        let mut index_items = Vec::new();
        let mut cursor = None;
        loop {
            let output = self
                .client
                .query()
                .table_name(&self.table_name)
                .index_name(GSI_NAME)
                .key_condition_expression("#gsi_pk = :user AND begins_with(#gsi_sk, :trip)")
                .expression_attribute_names("#gsi_pk", GSI1PK)
                .expression_attribute_names("#gsi_sk", GSI1SK)
                .expression_attribute_values(":user", AttributeValue::S(user_partition_key(actor)))
                .expression_attribute_values(":trip", AttributeValue::S("TRIP#".into()))
                .limit(USER_TRIPS_PAGE_SIZE)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(|_| TripRepoError::Unavailable)?;
            let next = output
                .last_evaluated_key()
                .filter(|key| !key.is_empty())
                .cloned();
            index_items.extend(output.items.unwrap_or_default());
            let Some(next) = next else {
                break;
            };
            cursor = Some(next);
        }
        let mut trip_ids = index_items
            .iter()
            .map(|item| string(item, GSI1SK))
            .collect::<Result<Vec<_>, _>>()?;
        trip_ids.sort();
        trip_ids.dedup();
        let mut summaries = Vec::new();
        for value in trip_ids {
            let Some(trip_id) = value.strip_prefix("TRIP#") else {
                return Err(TripRepoError::CorruptData);
            };
            // The GSI is navigation only. A stale row after revocation is
            // discarded by this strongly consistent direct read.
            if self.get_member_record(trip_id, actor).await?.is_none() {
                continue;
            }
            summaries.push(self.get_trip_meta(trip_id).await?.value.summary());
        }
        summaries.sort_by(|a, b| {
            a.start_date
                .cmp(&b.start_date)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(summaries)
    }

    async fn get_trip(&self, trip_id: &str, actor: &UserId) -> Result<Trip, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Any).await?;
        let meta = self.get_trip_meta(trip_id).await?.value;
        let members = self.get_members_for_trip(trip_id).await?;
        if members.len() as u32 != meta.member_count
            || members
                .iter()
                .filter(|member| member.role == TripRole::Leader)
                .count() as u32
                != meta.leader_count
        {
            return Err(TripRepoError::CorruptData);
        }
        Ok(meta.into_trip(members))
    }

    async fn set_trip_status(
        &self,
        trip_id: &str,
        actor: &UserId,
        status: TripStatus,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Trip, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Editor).await?;
        let stored = self.get_trip_meta(trip_id).await?;
        if stored.value.status == status {
            return self.get_trip(trip_id, actor).await;
        }
        let mut meta = stored.value;
        let old = meta.status;
        meta.status = status;
        let audit = audit(
            trip_id,
            actor,
            changed_at,
            change_id,
            AuditChange {
                entity: "trip",
                entity_id: trip_id,
                field: "status",
                old_value: json!(old),
                new_value: json!(status),
            },
        );
        let result = self
            .client
            .transact_write_items()
            .transact_items(action_condition(member_condition(
                &self.table_name,
                trip_id,
                actor,
                RequiredRole::Editor,
            )))
            .transact_items(action_put(revision_put(
                &self.table_name,
                encode_trip_meta(&meta, stored.revision + 1)?,
                stored.revision,
            )))
            .transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    audit_sk(changed_at, change_id),
                    AUDIT_ENTITY,
                    &audit,
                    1,
                )?,
            )))
            .send()
            .await;
        if let Err(error) = result {
            if !transaction_condition_failed(error.as_service_error()) {
                return Err(TripRepoError::Unavailable);
            }
            self.authorize(trip_id, actor, RequiredRole::Editor).await?;
            return Err(TripRepoError::Conflict);
        }
        self.get_trip(trip_id, actor).await
    }

    async fn get_members(
        &self,
        trip_id: &str,
        actor: &UserId,
        users: &dyn UserRepo,
    ) -> Result<Vec<User>, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Any).await?;
        let members = self.get_members_for_trip(trip_id).await?;
        let mut result = Vec::with_capacity(members.len());
        for member in members {
            result.push(
                users
                    .find_by_id(&UserId(member.user_id))
                    .await
                    .map_err(|error| match error {
                        UserRepoError::UserRepoUnavailable => TripRepoError::Unavailable,
                        UserRepoError::CorruptData | UserRepoError::DuplicateEmail(_) => {
                            TripRepoError::CorruptData
                        }
                    })?
                    .ok_or(TripRepoError::CorruptData)?,
            );
        }
        Ok(result)
    }

    async fn remove_member(
        &self,
        trip_id: &str,
        actor: &UserId,
        target: &UserId,
    ) -> Result<(), TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Leader).await?;
        let target_member = self
            .get_member_record(trip_id, target)
            .await?
            .ok_or(TripRepoError::NotFound)?;
        let stored_meta = self.get_trip_meta(trip_id).await?;
        if target_member.value.role == TripRole::Leader && stored_meta.value.leader_count <= 1 {
            return Err(TripRepoError::Conflict);
        }
        let mut meta = stored_meta.value;
        meta.member_count = meta
            .member_count
            .checked_sub(1)
            .ok_or(TripRepoError::CorruptData)?;
        if target_member.value.role == TripRole::Leader {
            meta.leader_count = meta
                .leader_count
                .checked_sub(1)
                .ok_or(TripRepoError::CorruptData)?;
        }
        let mut tx = self.client.transact_write_items();
        if actor != target {
            tx = tx.transact_items(action_condition(member_condition(
                &self.table_name,
                trip_id,
                actor,
                RequiredRole::Leader,
            )));
        }
        let target_delete = Delete::builder()
            .table_name(&self.table_name)
            .key(PK, AttributeValue::S(trip_pk(trip_id)))
            .key(SK, AttributeValue::S(member_sk(target)))
            .condition_expression("#entity = :member AND #role = :role")
            .expression_attribute_names("#entity", ENTITY_TYPE)
            .expression_attribute_names("#role", ROLE)
            .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()))
            .expression_attribute_values(
                ":role",
                AttributeValue::S(role_value(target_member.value.role).into()),
            )
            .build()
            .expect("delete is complete");
        tx = tx
            .transact_items(TransactWriteItem::builder().delete(target_delete).build())
            .transact_items(action_put(revision_put(
                &self.table_name,
                encode_trip_meta(&meta, stored_meta.revision + 1)?,
                stored_meta.revision,
            )))
            .transact_items(action_update(user_membership_count_update(
                &self.table_name,
                target,
                false,
            )));
        if let Err(error) = tx.send().await {
            if !transaction_condition_failed(error.as_service_error()) {
                return Err(TripRepoError::Unavailable);
            }
            self.authorize(trip_id, actor, RequiredRole::Leader).await?;
            return Err(TripRepoError::Conflict);
        }
        Ok(())
    }

    async fn create_invite(
        &self,
        trip_id: &str,
        actor: &UserId,
        invite: Invite,
    ) -> Result<Invite, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Leader).await?;
        let email = Email::parse(&invite.email).map_err(|_| TripRepoError::CorruptData)?;
        if invite.trip_id != trip_id
            || invite.email != email.as_str()
            || invite.invited_by != actor.0
            || invite.status != InviteStatus::Pending
        {
            return Err(TripRepoError::CorruptData);
        }
        let trip_sort_key = invite_sk(&email);
        let trip_partition = trip_pk(trip_id);
        let existing = self
            .trip_get(&trip_partition, &trip_sort_key)
            .await?
            .map(|item| {
                decode_record::<Invite>(&item, &trip_partition, &trip_sort_key, INVITE_ENTITY)
            })
            .transpose()?;
        if existing
            .as_ref()
            .is_some_and(|stored| stored.value.status == InviteStatus::Pending)
        {
            return Err(TripRepoError::DuplicateInvite);
        }
        if existing.as_ref().is_some_and(|stored| {
            stored.value.trip_id != trip_id || stored.value.email != email.as_str()
        }) {
            return Err(TripRepoError::CorruptData);
        }
        let lookup = InviteLookup {
            trip_id: trip_id.to_string(),
            invite_sort_key: trip_sort_key.clone(),
        };
        let invite_item = encode_record(
            trip_partition,
            trip_sort_key,
            INVITE_ENTITY,
            &invite,
            existing.as_ref().map_or(1, |stored| stored.revision + 1),
        )?;
        let invite_put = match existing {
            Some(stored) => revision_put(&self.table_name, invite_item, stored.revision),
            None => create_put(&self.table_name, invite_item),
        };
        let result = self
            .client
            .transact_write_items()
            .transact_items(action_condition(member_condition(
                &self.table_name,
                trip_id,
                actor,
                RequiredRole::Leader,
            )))
            .transact_items(action_put(invite_put))
            .transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    invitee_pk(&email),
                    invite_lookup_sk(trip_id),
                    INVITE_LOOKUP_ENTITY,
                    &lookup,
                    1,
                )?,
            )))
            .send()
            .await;
        match result {
            Ok(_) => Ok(invite),
            Err(error) if transaction_condition_failed(error.as_service_error()) => {
                self.authorize(trip_id, actor, RequiredRole::Leader).await?;
                let current = self
                    .trip_get(&trip_pk(trip_id), &invite_sk(&email))
                    .await?
                    .map(|item| {
                        decode_record::<Invite>(
                            &item,
                            &trip_pk(trip_id),
                            &invite_sk(&email),
                            INVITE_ENTITY,
                        )
                    })
                    .transpose()?;
                if current.is_some_and(|stored| stored.value.status == InviteStatus::Pending) {
                    Err(TripRepoError::DuplicateInvite)
                } else {
                    Err(TripRepoError::Conflict)
                }
            }
            Err(_) => Err(TripRepoError::Unavailable),
        }
    }

    async fn accept_pending_invites(
        &self,
        user: &User,
        joined_at: &str,
    ) -> Result<(), TripRepoError> {
        let lookup_pk = invitee_pk(&user.email);
        let lookups = self
            .query_partition(&lookup_pk, "TRIP#", USER_TRIPS_PAGE_SIZE)
            .await?;
        for item in lookups {
            let lookup_sk = string(&item, SK)?;
            let lookup: Stored<InviteLookup> =
                decode_record(&item, &lookup_pk, &lookup_sk, INVITE_LOOKUP_ENTITY)?;
            self.accept_invite_lookup(
                user,
                joined_at,
                &lookup.value.trip_id,
                &lookup_sk,
                &lookup.value.invite_sort_key,
            )
            .await?;
        }
        Ok(())
    }

    async fn search_saved_places(
        &self,
        trip_id: &str,
        actor: &UserId,
        query: &str,
    ) -> Result<Vec<Place>, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Any).await?;
        let pk = trip_pk(trip_id);
        let query = query.to_lowercase();
        let mut adopted_place_ids = HashSet::new();
        for item in self
            .query_partition(&pk, "PLAN#", TRIP_COLLECTION_PAGE_SIZE)
            .await?
        {
            if string(&item, ENTITY_TYPE).is_ok_and(|entity| entity == STOP_ENTITY) {
                let sk = string(&item, SK)?;
                let stop: Stored<Stop> = decode_record(&item, &pk, &sk, STOP_ENTITY)?;
                adopted_place_ids.insert(stop.value.place_id);
            }
        }
        if adopted_place_ids.is_empty() {
            return Ok(vec![]);
        }
        self.query_partition(&pk, "PLACE#", TRIP_COLLECTION_PAGE_SIZE)
            .await?
            .into_iter()
            .map(|item| {
                let sk = string(&item, SK)?;
                decode_record::<Place>(&item, &pk, &sk, PLACE_ENTITY).map(|stored| stored.value)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|places| {
                places
                    .into_iter()
                    .filter(|place| {
                        adopted_place_ids.contains(&place.id)
                            && format!("{} {} {}", place.name, place.city, place.address)
                                .to_lowercase()
                                .contains(&query)
                    })
                    .collect()
            })
    }

    async fn find_place(
        &self,
        trip_id: &str,
        actor: &UserId,
        place_id: &str,
    ) -> Result<Option<Place>, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Any).await?;
        Ok(self
            .get_place_record(trip_id, place_id)
            .await?
            .map(|stored| stored.value))
    }

    async fn list_candidates(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<CandidateWithPlace>, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Any).await?;
        let pk = trip_pk(trip_id);
        let items = self
            .query_partition(&pk, "CANDIDATE#", TRIP_COLLECTION_PAGE_SIZE)
            .await?;
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            let sk = string(&item, SK)?;
            let candidate: Stored<Candidate> = decode_record(&item, &pk, &sk, CANDIDATE_ENTITY)?;
            if candidate.value.trip_id != trip_id || sk != candidate_sk(&candidate.value.id) {
                return Err(TripRepoError::CorruptData);
            }
            let place = self
                .get_place_record(trip_id, &candidate.value.place_id)
                .await?
                .ok_or(TripRepoError::CorruptData)?
                .value;
            result.push(CandidateWithPlace {
                candidate: candidate.value,
                place,
            });
        }
        result.sort_by(|a, b| a.candidate.created_at.cmp(&b.candidate.created_at));
        Ok(result)
    }

    async fn add_candidate(
        &self,
        trip_id: &str,
        actor: &UserId,
        candidate: Candidate,
        place: Place,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Editor).await?;
        let result = self
            .client
            .transact_write_items()
            .transact_items(action_condition(member_condition(
                &self.table_name,
                trip_id,
                actor,
                RequiredRole::Editor,
            )))
            .transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    candidate_sk(&candidate.id),
                    CANDIDATE_ENTITY,
                    &candidate,
                    1,
                )?,
            )))
            .transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    place_sk(&place.id),
                    PLACE_ENTITY,
                    &place,
                    1,
                )?,
            )))
            .send()
            .await;
        if let Err(error) = result {
            if !transaction_condition_failed(error.as_service_error()) {
                return Err(TripRepoError::Unavailable);
            }
            self.authorize(trip_id, actor, RequiredRole::Editor).await?;
            return Err(TripRepoError::Conflict);
        }
        Ok(CandidateWithPlace { candidate, place })
    }

    async fn update_candidate(
        &self,
        trip_id: &str,
        actor: &UserId,
        candidate_id: &str,
        update: CandidateUpdate,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        let CandidateUpdate {
            place,
            pitch,
            tags,
            changed_at,
            change_id,
        } = update;
        self.authorize(trip_id, actor, RequiredRole::Editor).await?;
        let stored = self
            .get_candidate_record(trip_id, candidate_id)
            .await?
            .ok_or(TripRepoError::NotFound)?;
        let old_place = self
            .get_place_record(trip_id, &stored.value.place_id)
            .await?
            .ok_or(TripRepoError::CorruptData)?
            .value;
        let mut candidate = stored.value;
        let old_pitch = candidate.pitch.clone();
        let old_tags = candidate.tags.clone();
        candidate.place_id = place.id.clone();
        candidate.pitch = pitch;
        candidate.tags = tags;
        let mut changes = vec![(
            "place",
            serde_json::to_value(old_place).map_err(|_| TripRepoError::CorruptData)?,
            serde_json::to_value(&place).map_err(|_| TripRepoError::CorruptData)?,
        )];
        if old_pitch != candidate.pitch {
            changes.push(("pitch", json!(old_pitch), json!(candidate.pitch.clone())));
        }
        if old_tags != candidate.tags {
            changes.push(("tags", json!(old_tags), json!(candidate.tags.clone())));
        }
        let mut tx = self
            .client
            .transact_write_items()
            .transact_items(action_condition(member_condition(
                &self.table_name,
                trip_id,
                actor,
                RequiredRole::Editor,
            )))
            .transact_items(action_put(revision_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    candidate_sk(candidate_id),
                    CANDIDATE_ENTITY,
                    &candidate,
                    stored.revision + 1,
                )?,
                stored.revision,
            )))
            .transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    place_sk(&place.id),
                    PLACE_ENTITY,
                    &place,
                    1,
                )?,
            )));
        for (index, (field, old_value, new_value)) in changes.into_iter().enumerate() {
            let event_id = suffixed_id(&change_id, index);
            let change = audit(
                trip_id,
                actor,
                &changed_at,
                &event_id,
                AuditChange {
                    entity: "candidate",
                    entity_id: candidate_id,
                    field,
                    old_value,
                    new_value,
                },
            );
            tx = tx.transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    audit_sk(&changed_at, &event_id),
                    AUDIT_ENTITY,
                    &change,
                    1,
                )?,
            )));
        }
        let result = tx.send().await;
        if let Err(error) = result {
            if !transaction_condition_failed(error.as_service_error()) {
                return Err(TripRepoError::Unavailable);
            }
            self.authorize(trip_id, actor, RequiredRole::Editor).await?;
            return Err(TripRepoError::Conflict);
        }
        Ok(CandidateWithPlace { candidate, place })
    }

    async fn set_candidate_status(
        &self,
        trip_id: &str,
        actor: &UserId,
        candidate_id: &str,
        status: CandidateDisposition,
        changed_at: &str,
        change_id: &str,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Editor).await?;
        let stored = self
            .get_candidate_record(trip_id, candidate_id)
            .await?
            .ok_or(TripRepoError::NotFound)?;
        if stored.value.status == CandidateStatus::InPlan {
            return Err(TripRepoError::Conflict);
        }
        let mut candidate = stored.value;
        let old = candidate.status;
        let desired = CandidateStatus::from(status);
        let place = self
            .get_place_record(trip_id, &candidate.place_id)
            .await?
            .ok_or(TripRepoError::CorruptData)?
            .value;
        if old == desired {
            return Ok(CandidateWithPlace { candidate, place });
        }
        candidate.status = desired;
        let change = audit(
            trip_id,
            actor,
            changed_at,
            change_id,
            AuditChange {
                entity: "candidate",
                entity_id: candidate_id,
                field: "status",
                old_value: json!(old),
                new_value: json!(candidate.status),
            },
        );
        let result = self
            .client
            .transact_write_items()
            .transact_items(action_condition(member_condition(
                &self.table_name,
                trip_id,
                actor,
                RequiredRole::Editor,
            )))
            .transact_items(action_put(revision_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    candidate_sk(candidate_id),
                    CANDIDATE_ENTITY,
                    &candidate,
                    stored.revision + 1,
                )?,
                stored.revision,
            )))
            .transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    audit_sk(changed_at, change_id),
                    AUDIT_ENTITY,
                    &change,
                    1,
                )?,
            )))
            .send()
            .await;
        if let Err(error) = result {
            if !transaction_condition_failed(error.as_service_error()) {
                return Err(TripRepoError::Unavailable);
            }
            self.authorize(trip_id, actor, RequiredRole::Editor).await?;
            return Err(TripRepoError::Conflict);
        }
        Ok(CandidateWithPlace { candidate, place })
    }

    async fn get_current_plan(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<PlanDetail, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Any).await?;
        let meta = self.get_trip_meta(trip_id).await?.value;
        self.get_plan_detail_unchecked(trip_id, &meta).await
    }

    async fn initialize_plan(
        &self,
        trip_id: &str,
        actor: &UserId,
        anchor_place_id: &str,
        plan: Plan,
        days: Vec<Day>,
    ) -> Result<PlanDetail, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Editor).await?;
        let stored_meta = self.get_trip_meta(trip_id).await?;
        if stored_meta.value.current_plan_id.is_some() {
            return self
                .get_plan_detail_unchecked(trip_id, &stored_meta.value)
                .await;
        }
        let anchor = self
            .query_partition(&trip_pk(trip_id), "CANDIDATE#", TRIP_COLLECTION_PAGE_SIZE)
            .await?
            .into_iter()
            .map(|item| {
                let sk = string(&item, SK)?;
                decode_record::<Candidate>(&item, &trip_pk(trip_id), &sk, CANDIDATE_ENTITY)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|candidate| {
                candidate.value.place_id == anchor_place_id
                    && candidate.value.status == CandidateStatus::Shortlisted
            })
            .ok_or(TripRepoError::NotFound)?;
        if self
            .get_place_record(trip_id, &anchor.value.place_id)
            .await?
            .is_none()
        {
            return Err(TripRepoError::CorruptData);
        }
        let mut meta = stored_meta.value;
        meta.current_plan_id = Some(plan.id.clone());
        meta.current_plan_version = Some(plan.version);
        meta.status = TripStatus::Planning;
        let mut seen = HashSet::new();
        meta.cities = days
            .iter()
            .map(|day| day.city_hint.clone())
            .filter(|city| seen.insert(city.clone()))
            .collect();
        let mut tx = self
            .client
            .transact_write_items()
            .transact_items(action_condition(member_condition(
                &self.table_name,
                trip_id,
                actor,
                RequiredRole::Editor,
            )))
            .transact_items(action_condition(record_revision_condition(
                &self.table_name,
                trip_pk(trip_id),
                anchor.sort_key,
                CANDIDATE_ENTITY,
                anchor.revision,
            )))
            .transact_items(action_put(revision_put(
                &self.table_name,
                encode_trip_meta(&meta, stored_meta.revision + 1)?,
                stored_meta.revision,
            )))
            .transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    plan_sk(plan.version),
                    PLAN_ENTITY,
                    &plan,
                    1,
                )?,
            )));
        for day in &days {
            tx = tx.transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    trip_pk(trip_id),
                    day_sk(plan.version, day),
                    DAY_ENTITY,
                    day,
                    1,
                )?,
            )));
        }
        if let Err(error) = tx.send().await {
            if !transaction_condition_failed(error.as_service_error()) {
                return Err(TripRepoError::Unavailable);
            }
            self.authorize(trip_id, actor, RequiredRole::Editor).await?;
            let latest = self.get_trip_meta(trip_id).await?;
            if latest.value.current_plan_id.is_some() {
                return self.get_plan_detail_unchecked(trip_id, &latest.value).await;
            }
            return Err(TripRepoError::Conflict);
        }
        self.get_plan_detail_unchecked(trip_id, &meta).await
    }

    async fn list_plan_versions(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Plan>, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Any).await?;
        let pk = trip_pk(trip_id);
        let mut plans = self
            .query_partition(&pk, "PLAN#", TRIP_COLLECTION_PAGE_SIZE)
            .await?
            .into_iter()
            .filter(|item| string(item, ENTITY_TYPE).is_ok_and(|entity| entity == PLAN_ENTITY))
            .map(|item| {
                let sk = string(&item, SK)?;
                decode_record::<Plan>(&item, &pk, &sk, PLAN_ENTITY).map(|stored| stored.value)
            })
            .collect::<Result<Vec<_>, _>>()?;
        plans.sort_by_key(|plan| plan.version);
        Ok(plans)
    }

    async fn update_day(
        &self,
        trip_id: &str,
        actor: &UserId,
        day_id: &str,
        patch: DayPatch,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Day, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Editor).await?;
        let stored_meta = self.get_trip_meta(trip_id).await?;
        let version = stored_meta
            .value
            .current_plan_version
            .ok_or(TripRepoError::NotFound)?;
        let pk = trip_pk(trip_id);
        let mut stored_day = None;
        let mut all_days = Vec::new();
        for item in self
            .query_partition(
                &pk,
                &format!("{}#DAY#", plan_prefix(version)),
                TRIP_COLLECTION_PAGE_SIZE,
            )
            .await?
        {
            let sk = string(&item, SK)?;
            let day: Stored<Day> = decode_record(&item, &pk, &sk, DAY_ENTITY)?;
            if day.value.id == day_id {
                stored_day = Some(Stored {
                    value: day.value.clone(),
                    revision: day.revision,
                    sort_key: sk,
                });
            }
            all_days.push(day.value);
        }
        let stored_day = stored_day.ok_or(TripRepoError::NotFound)?;
        let mut day = stored_day.value;
        let mut changes = Vec::new();
        if let Some(value) = patch.window_start {
            if day.window_start != value {
                changes.push((
                    "windowStart",
                    json!(day.window_start.clone()),
                    json!(value.clone()),
                ));
            }
            day.window_start = value;
        }
        if let Some(value) = patch.window_end {
            if day.window_end != value {
                changes.push((
                    "windowEnd",
                    json!(day.window_end.clone()),
                    json!(value.clone()),
                ));
            }
            day.window_end = value;
        }
        if let Some(value) = patch.city_hint {
            if day.city_hint != value {
                changes.push((
                    "cityHint",
                    json!(day.city_hint.clone()),
                    json!(value.clone()),
                ));
            }
            day.city_hint = value;
        }
        // The service validates the request against a snapshot for a useful
        // 400 response. Recheck against the exact revision being written so a
        // concurrent complementary patch cannot persist an inverted window.
        if !day.window_is_ordered() {
            return Err(TripRepoError::Conflict);
        }
        if changes.is_empty() {
            return Ok(day);
        }
        let city_changed = changes.iter().any(|(field, _, _)| *field == "cityHint");
        for item in &mut all_days {
            if item.id == day.id {
                *item = day.clone();
            }
        }
        let mut meta = stored_meta.value;
        if city_changed {
            let mut seen = HashSet::new();
            meta.cities = all_days
                .into_iter()
                .map(|day| day.city_hint)
                .filter(|city| seen.insert(city.clone()))
                .collect();
        }
        let mut tx = self
            .client
            .transact_write_items()
            .transact_items(action_condition(member_condition(
                &self.table_name,
                trip_id,
                actor,
                RequiredRole::Editor,
            )))
            .transact_items(action_put(revision_put(
                &self.table_name,
                encode_record(
                    pk.clone(),
                    stored_day.sort_key,
                    DAY_ENTITY,
                    &day,
                    stored_day.revision + 1,
                )?,
                stored_day.revision,
            )));
        if city_changed {
            tx = tx.transact_items(action_put(revision_put(
                &self.table_name,
                encode_trip_meta(&meta, stored_meta.revision + 1)?,
                stored_meta.revision,
            )));
        } else {
            // Pin the child write to the plan that was current when it was
            // read. Phase 3 can then publish a new immutable plan version
            // without an in-flight content edit mutating the old one.
            tx = tx.transact_items(action_condition(record_revision_condition(
                &self.table_name,
                pk.clone(),
                META_SK.into(),
                TRIP_ENTITY,
                stored_meta.revision,
            )));
        }
        for (index, (field, old_value, new_value)) in changes.into_iter().enumerate() {
            let event_id = suffixed_id(change_id, index);
            let change = audit(
                trip_id,
                actor,
                changed_at,
                &event_id,
                AuditChange {
                    entity: "day",
                    entity_id: day_id,
                    field,
                    old_value,
                    new_value,
                },
            );
            tx = tx.transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    pk.clone(),
                    audit_sk(changed_at, &event_id),
                    AUDIT_ENTITY,
                    &change,
                    1,
                )?,
            )));
        }
        let result = tx.send().await;
        if let Err(error) = result {
            if !transaction_condition_failed(error.as_service_error()) {
                return Err(TripRepoError::Unavailable);
            }
            self.authorize(trip_id, actor, RequiredRole::Editor).await?;
            return Err(TripRepoError::Conflict);
        }
        Ok(day)
    }

    async fn update_stop(
        &self,
        trip_id: &str,
        actor: &UserId,
        stop_id: &str,
        patch: StopPatch,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Stop, TripRepoError> {
        self.authorize(trip_id, actor, RequiredRole::Editor).await?;
        let stored_meta = self.get_trip_meta(trip_id).await?;
        let version = stored_meta
            .value
            .current_plan_version
            .ok_or(TripRepoError::NotFound)?;
        let pk = trip_pk(trip_id);
        let mut stored_stop = None;
        for item in self
            .query_partition(
                &pk,
                &format!("{}#", plan_prefix(version)),
                TRIP_COLLECTION_PAGE_SIZE,
            )
            .await?
        {
            if string(&item, ENTITY_TYPE)? != STOP_ENTITY {
                continue;
            }
            let sk = string(&item, SK)?;
            let stop: Stored<Stop> = decode_record(&item, &pk, &sk, STOP_ENTITY)?;
            if stop.value.id == stop_id {
                stored_stop = Some(stop);
                break;
            }
        }
        let stored = stored_stop.ok_or(TripRepoError::NotFound)?;
        let mut stop = stored.value;
        let mut changes = Vec::new();
        if let Some(value) = patch.planned_arrival {
            if stop.planned_arrival != value {
                changes.push((
                    "plannedArrival",
                    json!(stop.planned_arrival.clone()),
                    json!(value.clone()),
                ));
            }
            stop.planned_arrival = value;
        }
        if let Some(value) = patch.duration_min {
            if stop.duration_min != value {
                changes.push(("durationMin", json!(stop.duration_min), json!(value)));
            }
            stop.duration_min = value;
        }
        if let Some(value) = patch.notes {
            if stop.notes != value {
                changes.push(("notes", json!(stop.notes.clone()), json!(value.clone())));
            }
            stop.notes = value;
        }
        if let Some(value) = patch.booking {
            if stop.booking != value {
                changes.push(("booking", json!(stop.booking.clone()), json!(value.clone())));
            }
            stop.booking = value;
        }
        if changes.is_empty() {
            return Ok(stop);
        }
        let mut tx = self
            .client
            .transact_write_items()
            .transact_items(action_condition(member_condition(
                &self.table_name,
                trip_id,
                actor,
                RequiredRole::Editor,
            )))
            .transact_items(action_condition(record_revision_condition(
                &self.table_name,
                pk.clone(),
                META_SK.into(),
                TRIP_ENTITY,
                stored_meta.revision,
            )))
            .transact_items(action_put(revision_put(
                &self.table_name,
                encode_record(
                    pk.clone(),
                    stored.sort_key,
                    STOP_ENTITY,
                    &stop,
                    stored.revision + 1,
                )?,
                stored.revision,
            )));
        for (index, (field, old_value, new_value)) in changes.into_iter().enumerate() {
            let event_id = suffixed_id(change_id, index);
            let change = audit(
                trip_id,
                actor,
                changed_at,
                &event_id,
                AuditChange {
                    entity: "stop",
                    entity_id: stop_id,
                    field,
                    old_value,
                    new_value,
                },
            );
            tx = tx.transact_items(action_put(create_put(
                &self.table_name,
                encode_record(
                    pk.clone(),
                    audit_sk(changed_at, &event_id),
                    AUDIT_ENTITY,
                    &change,
                    1,
                )?,
            )));
        }
        let result = tx.send().await;
        if let Err(error) = result {
            if !transaction_condition_failed(error.as_service_error()) {
                return Err(TripRepoError::Unavailable);
            }
            self.authorize(trip_id, actor, RequiredRole::Editor).await?;
            return Err(TripRepoError::Conflict);
        }
        Ok(stop)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use aws_sdk_dynamodb::operation::{
        get_item::GetItemOutput, query::QueryOutput, transact_write_items::TransactWriteItemsOutput,
    };
    use aws_sdk_dynamodb::types::{CancellationReason, error::TransactionCanceledException};
    use aws_smithy_mocks::{RuleMode, mock, mock_client};
    use itinera_core::domain::trip::PlaceKind;

    use super::*;

    const TABLE: &str = "itinera-test";

    fn leader() -> TripMember {
        TripMember {
            user_id: "u-leader".into(),
            role: TripRole::Leader,
            joined_at: "2026-08-01T00:00:00Z".into(),
        }
    }

    fn trip() -> Trip {
        Trip {
            id: "trip-a".into(),
            name: "Japan".into(),
            cover_photo_url: None,
            accent_color: None,
            stop_kind_labels: None,
            status: TripStatus::Dreaming,
            start_date: "2026-11-01".into(),
            end_date: "2026-11-03".into(),
            base_currency: "GBP".into(),
            soft_budget: None,
            members: vec![leader()],
            current_plan_id: None,
            created_at: "2026-08-01T00:00:00Z".into(),
        }
    }

    fn pending_invite() -> Invite {
        Invite {
            id: "invite-new".into(),
            trip_id: "trip-a".into(),
            email: "friend@example.test".into(),
            invited_by: "u-leader".into(),
            status: InviteStatus::Pending,
            created_at: "2026-08-05T00:00:00Z".into(),
        }
    }

    fn cancelled_transaction(codes: &[&str]) -> TransactWriteItemsError {
        let mut builder = TransactionCanceledException::builder();
        for code in codes {
            builder =
                builder.cancellation_reasons(CancellationReason::builder().code(*code).build());
        }
        TransactWriteItemsError::TransactionCanceledException(builder.build())
    }

    #[test]
    fn trip_owned_keys_include_the_authoritative_trip_partition() {
        assert_eq!(trip_pk("trip-a"), "TRIP#trip-a");
        assert_eq!(candidate_sk("candidate-a"), "CANDIDATE#candidate-a");
        assert_eq!(plan_sk(7), "PLAN#0000000007#META");
    }

    #[test]
    fn invite_lookup_keys_do_not_disclose_the_email() {
        let email = Email::parse("cloud.strife@proton.me").expect("valid email");
        let key = invitee_pk(&email);
        assert!(key.starts_with("INVITEE#"));
        assert!(!key.contains("cloud"));
        assert!(!key.contains('@'));
    }

    #[test]
    fn records_validate_key_type_schema_and_json() {
        let member = TripMember {
            user_id: "u-1".into(),
            role: TripRole::Leader,
            joined_at: "2026-08-01T00:00:00Z".into(),
        };
        let item = encode_member("trip-a", &member).expect("encode");
        let decoded: Stored<TripMember> =
            decode_record(&item, "TRIP#trip-a", "MEMBER#u-1", MEMBER_ENTITY).expect("decode");
        assert_eq!(decoded.value, member);
        assert!(
            decode_record::<TripMember>(&item, "TRIP#trip-b", "MEMBER#u-1", MEMBER_ENTITY).is_err()
        );
    }

    #[test]
    fn only_conditional_cancellations_are_domain_conflicts() {
        let conditional = cancelled_transaction(&["None", CONDITIONAL_FAILURE, "None"]);
        let transaction_conflict = cancelled_transaction(&["None", "TransactionConflict"]);
        let throttled = cancelled_transaction(&["ThrottlingError"]);

        assert!(transaction_condition_failed(Some(&conditional)));
        assert!(!transaction_condition_failed(Some(&transaction_conflict)));
        assert!(!transaction_condition_failed(Some(&throttled)));
        assert!(!transaction_condition_failed(None));
    }

    #[tokio::test]
    async fn partition_queries_follow_continuation_keys() {
        let cursor = HashMap::from([
            (PK.to_string(), AttributeValue::S("INVITEE#hash".into())),
            (SK.to_string(), AttributeValue::S("TRIP#trip-a".into())),
        ]);
        let first_cursor = cursor.clone();
        let first_rule = mock!(aws_sdk_dynamodb::Client::query)
            .match_requests(|request| request.exclusive_start_key().is_none())
            .then_output(move || {
                QueryOutput::builder()
                    .items(HashMap::from([(
                        SK.to_string(),
                        AttributeValue::S("TRIP#trip-a".into()),
                    )]))
                    .set_last_evaluated_key(Some(first_cursor.clone()))
                    .build()
            });
        let second_cursor = cursor.clone();
        let second_rule = mock!(aws_sdk_dynamodb::Client::query)
            .match_requests(move |request| request.exclusive_start_key() == Some(&second_cursor))
            .then_output(|| {
                QueryOutput::builder()
                    .items(HashMap::from([(
                        SK.to_string(),
                        AttributeValue::S("TRIP#trip-b".into()),
                    )]))
                    .build()
            });
        let client = mock_client!(aws_sdk_dynamodb, [&first_rule, &second_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        let items = repo
            .query_partition("INVITEE#hash", "TRIP#", 1)
            .await
            .expect("all pages");

        assert_eq!(items.len(), 2);
        assert_eq!(first_rule.num_calls(), 1);
        assert_eq!(second_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn an_accepted_invite_can_be_renewed() {
        let member_item = encode_member("trip-a", &leader()).expect("member item");
        let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.key().is_some_and(|key| {
                    key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(member_item.clone()))
                    .build()
            });
        let mut accepted = pending_invite();
        accepted.id = "invite-old".into();
        accepted.status = InviteStatus::Accepted;
        let email = Email::parse(&accepted.email).expect("email");
        let accepted_item = encode_record(
            trip_pk("trip-a"),
            invite_sk(&email),
            INVITE_ENTITY,
            &accepted,
            4,
        )
        .expect("accepted invite item");
        let invite_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.key().is_some_and(|key| {
                    key.get(SK)
                        == Some(&AttributeValue::S(invite_sk(
                            &Email::parse("friend@example.test").expect("email"),
                        )))
                })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(accepted_item.clone()))
                    .build()
            });
        let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .match_requests(|request| {
                let items = request.transact_items();
                items.len() == 3
                    && items[1].put().is_some_and(|put| {
                        put.item().get(REVISION) == Some(&AttributeValue::N("5".into()))
                            && put.condition_expression() == Some("#revision = :expected_revision")
                    })
                    && items[2].put().is_some_and(|put| {
                        put.item().get(ENTITY_TYPE)
                            == Some(&AttributeValue::S(INVITE_LOOKUP_ENTITY.into()))
                    })
            })
            .then_output(|| TransactWriteItemsOutput::builder().build());
        let client = mock_client!(
            aws_sdk_dynamodb,
            [&member_rule, &invite_rule, &transaction_rule]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        let renewed = repo
            .create_invite("trip-a", &UserId("u-leader".into()), pending_invite())
            .await
            .expect("renewed invite");

        assert_eq!(renewed.id, "invite-new");
        assert_eq!(renewed.status, InviteStatus::Pending);
        assert_eq!(transaction_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn concurrent_invite_acceptance_is_idempotent() {
        let email = Email::parse("friend@example.test").expect("email");
        let lookup_pk = invitee_pk(&email);
        let lookup_sk = invite_lookup_sk("trip-a");
        let invite_sk = invite_sk(&email);
        let lookup = InviteLookup {
            trip_id: "trip-a".into(),
            invite_sort_key: invite_sk.clone(),
        };
        let lookup_item = encode_record(
            lookup_pk.clone(),
            lookup_sk.clone(),
            INVITE_LOOKUP_ENTITY,
            &lookup,
            1,
        )
        .expect("lookup item");
        let query_rule = mock!(aws_sdk_dynamodb::Client::query)
            .match_requests(move |request| {
                request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":pk"))
                    == Some(&AttributeValue::S(lookup_pk.clone()))
            })
            .then_output(move || {
                QueryOutput::builder()
                    .set_items(Some(vec![lookup_item.clone()]))
                    .build()
            });
        let lookup_get_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(move |request| {
                request
                    .key()
                    .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(lookup_sk.clone())))
            })
            // The other request has already deleted the lookup.
            .then_output(|| GetItemOutput::builder().build());
        let mut accepted = pending_invite();
        accepted.status = InviteStatus::Accepted;
        let accepted_item = encode_record(
            trip_pk("trip-a"),
            invite_sk.clone(),
            INVITE_ENTITY,
            &accepted,
            2,
        )
        .expect("accepted invite");
        let invite_get_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(move |request| {
                request
                    .key()
                    .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(invite_sk.clone())))
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(accepted_item.clone()))
                    .build()
            });
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&query_rule, &lookup_get_rule, &invite_get_rule]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
        let user = User {
            id: UserId("u-friend".into()),
            email,
            display_name: None,
        };

        repo.accept_pending_invites(&user, "2026-08-05T00:00:00Z")
            .await
            .expect("concurrent acceptance is success");

        assert_eq!(query_rule.num_calls(), 1);
        assert_eq!(lookup_get_rule.num_calls(), 1);
        assert_eq!(invite_get_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn plan_initialization_conditions_the_shortlisted_candidate_revision() {
        let member_item = encode_member("trip-a", &leader()).expect("member item");
        let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.key().is_some_and(|key| {
                    key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(member_item.clone()))
                    .build()
            });
        let meta_item = encode_trip_meta(&TripMeta::from_trip(&trip()), 1).expect("meta item");
        let meta_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request
                    .key()
                    .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(META_SK.into())))
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(meta_item.clone()))
                    .build()
            });
        let place = Place {
            id: "place-a".into(),
            name: "Kyoto".into(),
            kind: PlaceKind::Sight,
            lat: 35.0,
            lng: 135.0,
            tz: "Asia/Tokyo".into(),
            country_code: "JP".into(),
            admin_area: "Kyoto".into(),
            city: "Kyoto".into(),
            address: "Kyoto".into(),
            external_ref: None,
            website: None,
            phone: None,
            rating: None,
            price_level: None,
            opening_hours: None,
            photo_urls: vec![],
            guide: None,
        };
        let place_item = encode_record(
            trip_pk("trip-a"),
            place_sk("place-a"),
            PLACE_ENTITY,
            &place,
            1,
        )
        .expect("place item");
        let place_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.key().is_some_and(|key| {
                    key.get(SK) == Some(&AttributeValue::S("PLACE#place-a".into()))
                })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(place_item.clone()))
                    .build()
            });
        let candidate = Candidate {
            id: "candidate-a".into(),
            trip_id: "trip-a".into(),
            source_place_id: None,
            place_id: "place-a".into(),
            proposed_by: "u-leader".into(),
            created_at: "2026-08-05T00:00:00Z".into(),
            pitch: "Anchor".into(),
            tags: vec![],
            status: CandidateStatus::Shortlisted,
        };
        let candidate_item = encode_record(
            trip_pk("trip-a"),
            candidate_sk("candidate-a"),
            CANDIDATE_ENTITY,
            &candidate,
            7,
        )
        .expect("candidate item");
        let candidate_rule = mock!(aws_sdk_dynamodb::Client::query)
            .match_requests(|request| {
                request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("CANDIDATE#".into()))
            })
            .then_output(move || {
                QueryOutput::builder()
                    .set_items(Some(vec![candidate_item.clone()]))
                    .build()
            });
        let plan = Plan {
            id: "plan-a".into(),
            trip_id: "trip-a".into(),
            version: 1,
            created_from_proposal_id: None,
            created_at: "2026-08-05T00:00:00Z".into(),
        };
        let day = Day {
            id: "day-a".into(),
            plan_id: "plan-a".into(),
            date: "2026-11-01".into(),
            city_hint: "Kyoto".into(),
            tz: "Asia/Tokyo".into(),
            window_start: "09:00".into(),
            window_end: "21:00".into(),
        };
        let plan_item =
            encode_record(trip_pk("trip-a"), plan_sk(1), PLAN_ENTITY, &plan, 1).expect("plan item");
        let day_item = encode_record(trip_pk("trip-a"), day_sk(1, &day), DAY_ENTITY, &day, 1)
            .expect("day item");
        let detail_rule = mock!(aws_sdk_dynamodb::Client::query)
            .match_requests(|request| {
                request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("PLAN#0000000001#".into()))
            })
            .then_output(move || {
                QueryOutput::builder()
                    .set_items(Some(vec![plan_item.clone(), day_item.clone()]))
                    .build()
            });
        let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .match_requests(|request| {
                let items = request.transact_items();
                items.len() == 5
                    && items[1].condition_check().is_some_and(|condition| {
                        condition.key().get(SK)
                            == Some(&AttributeValue::S("CANDIDATE#candidate-a".into()))
                            && condition
                                .expression_attribute_values()
                                .and_then(|values| values.get(":revision"))
                                == Some(&AttributeValue::N("7".into()))
                    })
            })
            .then_output(|| TransactWriteItemsOutput::builder().build());
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [
                &member_rule,
                &meta_rule,
                &place_rule,
                &candidate_rule,
                &detail_rule,
                &transaction_rule
            ]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        let initialized = repo
            .initialize_plan(
                "trip-a",
                &UserId("u-leader".into()),
                "place-a",
                plan,
                vec![day],
            )
            .await
            .expect("initialized plan");

        assert_eq!(initialized.plan.id, "plan-a");
        assert_eq!(transaction_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn day_edits_are_conditioned_on_the_current_plan_revision() {
        let member_item = encode_member("trip-a", &leader()).expect("member item");
        let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.key().is_some_and(|key| {
                    key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(member_item.clone()))
                    .build()
            });
        let mut meta = TripMeta::from_trip(&trip());
        meta.current_plan_id = Some("plan-a".into());
        meta.current_plan_version = Some(1);
        meta.cities = vec!["Kyoto".into()];
        let meta_item = encode_trip_meta(&meta, 7).expect("meta item");
        let meta_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request
                    .key()
                    .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(META_SK.into())))
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(meta_item.clone()))
                    .build()
            });
        let day = Day {
            id: "day-a".into(),
            plan_id: "plan-a".into(),
            date: "2026-11-01".into(),
            city_hint: "Kyoto".into(),
            tz: "Asia/Tokyo".into(),
            window_start: "09:00".into(),
            window_end: "21:00".into(),
        };
        let day_item = encode_record(trip_pk("trip-a"), day_sk(1, &day), DAY_ENTITY, &day, 3)
            .expect("day item");
        let day_rule = mock!(aws_sdk_dynamodb::Client::query)
            .match_requests(|request| {
                request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("PLAN#0000000001#DAY#".into()))
            })
            .then_output(move || {
                QueryOutput::builder()
                    .set_items(Some(vec![day_item.clone()]))
                    .build()
            });
        let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .match_requests(|request| {
                let items = request.transact_items();
                items.len() == 4
                    && items[2].condition_check().is_some_and(|condition| {
                        condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                            && condition
                                .expression_attribute_values()
                                .and_then(|values| values.get(":revision"))
                                == Some(&AttributeValue::N("7".into()))
                    })
            })
            .then_output(|| TransactWriteItemsOutput::builder().build());
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&member_rule, &meta_rule, &day_rule, &transaction_rule]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        let updated = repo
            .update_day(
                "trip-a",
                &UserId("u-leader".into()),
                "day-a",
                DayPatch {
                    window_start: Some("10:00".into()),
                    window_end: None,
                    city_hint: None,
                },
                "2026-08-05T00:00:00Z",
                "change-a",
            )
            .await
            .expect("day update");

        assert_eq!(updated.window_start, "10:00");
        assert_eq!(transaction_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn create_trip_atomically_writes_meta_membership_and_reverse_count() {
        let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .match_requests(|request| {
                let items = request.transact_items();
                items.len() == 3
                    && items[0].put().is_some_and(|put| {
                        put.item().get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                            && put.item().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                            && put.item().get(LEADER_COUNT) == Some(&AttributeValue::N("1".into()))
                    })
                    && items[1].put().is_some_and(|put| {
                        put.item().get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                            && put.item().get(SK)
                                == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                            && put.item().get(GSI1PK)
                                == Some(&AttributeValue::S("USER#u-leader".into()))
                            && put.item().get(ROLE) == Some(&AttributeValue::S("leader".into()))
                    })
                    && items[2].update().is_some_and(|update| {
                        update.key().get(PK) == Some(&AttributeValue::S("USER#u-leader".into()))
                            && update.key().get(SK)
                                == Some(&AttributeValue::S(USER_PROFILE_SK.into()))
                            && update.update_expression().contains("if_not_exists")
                    })
            })
            .then_output(|| TransactWriteItemsOutput::builder().build());
        let client = mock_client!(aws_sdk_dynamodb, [&transaction_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        let created = repo.create_trip(trip()).await.expect("create trip");

        assert_eq!(created.id, "trip-a");
        assert_eq!(transaction_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn get_trip_authorizes_with_a_strong_direct_read_before_loading_data() {
        let member_item = encode_member("trip-a", &leader()).expect("member item");
        let member_for_get = member_item.clone();
        let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.consistent_read() == Some(true)
                    && request.key().is_some_and(|key| {
                        key.get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                            && key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                    })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(member_for_get.clone()))
                    .build()
            });
        let meta = TripMeta::from_trip(&trip());
        let meta_item = encode_trip_meta(&meta, 1).expect("meta item");
        let meta_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.consistent_read() == Some(true)
                    && request.key().is_some_and(|key| {
                        key.get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                            && key.get(SK) == Some(&AttributeValue::S(META_SK.into()))
                    })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(meta_item.clone()))
                    .build()
            });
        let members_rule = mock!(aws_sdk_dynamodb::Client::query)
            .match_requests(|request| {
                request.consistent_read() == Some(true)
                    && request.index_name().is_none()
                    && request
                        .expression_attribute_values()
                        .and_then(|values| values.get(":pk"))
                        == Some(&AttributeValue::S("TRIP#trip-a".into()))
                    && request
                        .expression_attribute_values()
                        .and_then(|values| values.get(":prefix"))
                        == Some(&AttributeValue::S("MEMBER#".into()))
            })
            .then_output(move || {
                QueryOutput::builder()
                    .set_items(Some(vec![member_item.clone()]))
                    .build()
            });
        let client = mock_client!(aws_sdk_dynamodb, [&member_rule, &meta_rule, &members_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        let loaded = repo
            .get_trip("trip-a", &UserId("u-leader".into()))
            .await
            .expect("authorized trip");

        assert_eq!(loaded, trip());
        assert_eq!(member_rule.num_calls(), 1);
        assert_eq!(meta_rule.num_calls(), 1);
        assert_eq!(members_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn an_absent_direct_membership_stops_a_cross_trip_read() {
        let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| request.consistent_read() == Some(true))
            .then_output(|| GetItemOutput::builder().build());
        let client = mock_client!(aws_sdk_dynamodb, [&member_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        let error = repo
            .get_trip("trip-a", &UserId("u-stranger".into()))
            .await
            .expect_err("non-member must not load trip metadata");

        assert_eq!(error, TripRepoError::NotFound);
        assert_eq!(member_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn status_change_rechecks_editor_role_inside_the_atomic_write() {
        let member_item = encode_member("trip-a", &leader()).expect("member item");
        let member_for_get = member_item.clone();
        let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.consistent_read() == Some(true)
                    && request.key().is_some_and(|key| {
                        key.get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                            && key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                    })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(member_for_get.clone()))
                    .build()
            });

        let old_meta = TripMeta::from_trip(&trip());
        let mut new_meta = old_meta.clone();
        new_meta.status = TripStatus::Booked;
        let old_meta_item = encode_trip_meta(&old_meta, 1).expect("old meta item");
        let new_meta_item = encode_trip_meta(&new_meta, 2).expect("new meta item");
        let meta_reads = Arc::new(AtomicUsize::new(0));
        let reads = Arc::clone(&meta_reads);
        let meta_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.consistent_read() == Some(true)
                    && request.key().is_some_and(|key| {
                        key.get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                            && key.get(SK) == Some(&AttributeValue::S(META_SK.into()))
                    })
            })
            .then_output(move || {
                let item = if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    old_meta_item.clone()
                } else {
                    new_meta_item.clone()
                };
                GetItemOutput::builder().set_item(Some(item)).build()
            });

        let members_rule = mock!(aws_sdk_dynamodb::Client::query)
            .match_requests(|request| {
                request.consistent_read() == Some(true)
                    && request.index_name().is_none()
                    && request
                        .expression_attribute_values()
                        .and_then(|values| values.get(":prefix"))
                        == Some(&AttributeValue::S("MEMBER#".into()))
            })
            .then_output(move || {
                QueryOutput::builder()
                    .set_items(Some(vec![member_item.clone()]))
                    .build()
            });

        let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .match_requests(|request| {
                let items = request.transact_items();
                items.len() == 3
                    && items[0].condition_check().is_some_and(|condition| {
                        condition.key().get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                            && condition.key().get(SK)
                                == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                            && condition.condition_expression().contains("#role")
                    })
            })
            .then_output(|| TransactWriteItemsOutput::builder().build());

        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&member_rule, &meta_rule, &members_rule, &transaction_rule]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        let updated = repo
            .set_trip_status(
                "trip-a",
                &UserId("u-leader".into()),
                TripStatus::Booked,
                "2026-08-05T00:00:00Z",
                "change-a",
            )
            .await
            .expect("status update");

        assert_eq!(updated.status, TripStatus::Booked);
        assert_eq!(member_rule.num_calls(), 2);
        assert_eq!(meta_rule.num_calls(), 2);
        assert_eq!(transaction_rule.num_calls(), 1);
    }
}
