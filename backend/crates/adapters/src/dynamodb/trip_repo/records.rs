//! Persisted record shapes, key construction, and strict codecs.

use super::*;

pub(in crate::dynamodb) const DATA: &str = "data";
pub(in crate::dynamodb) const REVISION: &str = "revision";
pub(in crate::dynamodb) const ROLE: &str = "role";
pub(in crate::dynamodb) const GSI1PK: &str = "gsi1pk";
pub(in crate::dynamodb) const GSI1SK: &str = "gsi1sk";
pub(in crate::dynamodb) const MEMBER_COUNT: &str = "member_count";
pub(in crate::dynamodb) const LEADER_COUNT: &str = "leader_count";
pub(in crate::dynamodb) const CURRENT_PLAN_ID: &str = "current_plan_id";
pub(in crate::dynamodb) const CURRENT_PLAN_VERSION: &str = "current_plan_version";

pub(in crate::dynamodb) const META_SK: &str = "META";
pub(in crate::dynamodb) const TRIP_ENTITY: &str = "TRIP";
pub(in crate::dynamodb) const MEMBER_ENTITY: &str = "TRIP_MEMBER";
pub(super) const INVITE_ENTITY: &str = "TRIP_INVITE";
pub(super) const INVITE_LOOKUP_ENTITY: &str = "INVITE_LOOKUP";
pub(in crate::dynamodb) const PLACE_ENTITY: &str = "PLACE";
pub(in crate::dynamodb) const CANDIDATE_ENTITY: &str = "CANDIDATE";
pub(super) const PLAN_ENTITY: &str = "PLAN";
pub(in crate::dynamodb) const DAY_ENTITY: &str = "DAY";
pub(in crate::dynamodb) const STOP_ENTITY: &str = "STOP";
pub(in crate::dynamodb) const AUDIT_ENTITY: &str = "CONTENT_AUDIT";

pub(super) const GSI_NAME: &str = "gsi1";
pub(super) const USER_TRIPS_PAGE_SIZE: i32 = 50;
pub(in crate::dynamodb) const TRIP_COLLECTION_PAGE_SIZE: i32 = 500;
pub(super) const INVITE_ACCEPT_ATTEMPTS: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::dynamodb) struct TripMeta {
    pub(in crate::dynamodb) id: String,
    pub(in crate::dynamodb) name: String,
    pub(in crate::dynamodb) cover_photo_url: Option<String>,
    pub(in crate::dynamodb) accent_color: Option<String>,
    pub(in crate::dynamodb) stop_kind_labels: Option<HashMap<StopKind, String>>,
    pub(in crate::dynamodb) status: TripStatus,
    pub(in crate::dynamodb) start_date: String,
    pub(in crate::dynamodb) end_date: String,
    pub(in crate::dynamodb) base_currency: String,
    pub(in crate::dynamodb) soft_budget: Option<SoftBudget>,
    pub(in crate::dynamodb) current_plan_id: Option<String>,
    pub(in crate::dynamodb) current_plan_version: Option<u32>,
    pub(in crate::dynamodb) created_at: String,
    pub(in crate::dynamodb) member_count: u32,
    pub(in crate::dynamodb) leader_count: u32,
    pub(in crate::dynamodb) cities: Vec<String>,
}

impl TripMeta {
    pub(super) fn from_trip(trip: &Trip) -> Self {
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

    pub(super) fn into_trip(self, members: Vec<TripMember>) -> Trip {
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

    pub(super) fn summary(&self) -> TripSummary {
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
pub(super) struct InviteLookup {
    pub(super) trip_id: String,
    pub(super) invite_sort_key: String,
}

pub(in crate::dynamodb) struct Stored<T> {
    pub(in crate::dynamodb) value: T,
    pub(in crate::dynamodb) revision: u64,
    pub(in crate::dynamodb) sort_key: String,
}

pub(in crate::dynamodb) fn trip_pk(trip_id: &str) -> String {
    format!("TRIP#{trip_id}")
}

pub(in crate::dynamodb) fn member_sk(user_id: &UserId) -> String {
    format!("MEMBER#{}", user_id.0)
}

pub(in crate::dynamodb) fn place_sk(place_id: &str) -> String {
    format!("PLACE#{place_id}")
}

pub(in crate::dynamodb) fn candidate_sk(candidate_id: &str) -> String {
    format!("CANDIDATE#{candidate_id}")
}

pub(in crate::dynamodb) fn plan_prefix(version: u32) -> String {
    format!("PLAN#{version:010}")
}

pub(super) fn plan_sk(version: u32) -> String {
    format!("{}#META", plan_prefix(version))
}

pub(super) fn day_sk(version: u32, day: &Day) -> String {
    format!("{}#DAY#{}#{}", plan_prefix(version), day.date, day.id)
}

pub(in crate::dynamodb) fn audit_sk(at: &str, id: &str) -> String {
    format!("AUDIT#{at}#{id}")
}

pub(super) fn email_digest(email: &Email) -> String {
    let digest = Sha256::digest(email.as_str().as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn invite_sk(email: &Email) -> String {
    format!("INVITE#{}", email_digest(email))
}

pub(super) fn invitee_pk(email: &Email) -> String {
    format!("INVITEE#{}", email_digest(email))
}

pub(super) fn invite_lookup_sk(trip_id: &str) -> String {
    format!("TRIP#{trip_id}")
}

pub(in crate::dynamodb) fn string(
    item: &HashMap<String, AttributeValue>,
    name: &str,
) -> Result<String, TripRepoError> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .cloned()
        .ok_or(TripRepoError::CorruptData)
}

pub(in crate::dynamodb) fn number_u64(
    item: &HashMap<String, AttributeValue>,
    name: &str,
) -> Result<u64, TripRepoError> {
    item.get(name)
        .and_then(|value| value.as_n().ok())
        .and_then(|value| value.parse().ok())
        .ok_or(TripRepoError::CorruptData)
}

pub(in crate::dynamodb) fn encode_record<T: Serialize>(
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

pub(in crate::dynamodb) fn decode_record<T: DeserializeOwned>(
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

pub(super) fn add_trip_meta_attributes(
    item: &mut HashMap<String, AttributeValue>,
    meta: &TripMeta,
) {
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

pub(in crate::dynamodb) fn encode_trip_meta(
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

pub(in crate::dynamodb) fn encode_member(
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

pub(in crate::dynamodb) fn role_value(role: TripRole) -> &'static str {
    match role {
        TripRole::Leader => "leader",
        TripRole::Member => "member",
        TripRole::Viewer => "viewer",
    }
}
