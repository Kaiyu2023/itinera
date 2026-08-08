//! Strict persisted-record codecs for the trip capability.

use std::collections::HashMap;

use itinera_core::{
    domain::{
        currency::CurrencyCode,
        trip::{
            DateRange, Invite, InviteData, InviteStatus, SoftBudget, StopKind, Trip, TripData,
            TripMember, TripRole, TripStatus, TripSummary,
        },
        user::{Email, User, UserId},
    },
    ports::trip::TripRepoError,
};
use serde::Serialize;
use sqlx::{Row, sqlite::SqliteRow};

use crate::sqlite::codec::{
    checked_revision, decode_json, email_digest, encode_json, validate_email, validate_id,
    validate_optional_text,
};

pub(super) const MAX_COLLECTION_ITEMS: usize = 1_000;
pub(super) const COLLECTION_QUERY_LIMIT: i64 = 1_001;
pub(super) const MAX_PENDING_INVITES_PER_EMAIL: usize = 100;
pub(super) const PENDING_INVITE_QUERY_LIMIT: i64 = 101;
pub(super) const MAX_COLLECTION_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_VALUE_JSON_BYTES: usize = 4 * 1024;

#[derive(Debug)]
pub(super) struct StoredTrip {
    /// Validated nested values plus aggregate fields. Membership rows are
    /// attached before this data is converted into a domain trip.
    pub(super) data: TripData,
    pub(super) revision: i64,
}

#[derive(Debug)]
pub(super) struct StoredTripMember {
    pub(super) member: TripMember,
    pub(super) revision: i64,
}

#[derive(Debug)]
pub(super) struct StoredInvite {
    pub(super) invite: Invite,
    pub(super) digest: String,
    pub(super) revision: i64,
}

pub(super) fn invite_key(
    trip_id: &str,
    invite: &Invite,
    revision: i64,
) -> Result<(Email, String), TripRepoError> {
    validate_id(trip_id).map_err(corrupt)?;
    checked_revision(revision).map_err(corrupt)?;
    if invite.trip_id() != trip_id {
        return Err(TripRepoError::CorruptData);
    }
    let email = Email::parse(invite.email()).map_err(corrupt)?;
    let digest = email_digest(&email);
    Ok((email, digest))
}

pub(super) fn decode_trip_row(row: &SqliteRow) -> Result<StoredTrip, TripRepoError> {
    let current_plan_id: Option<String> = row.decode("current_plan_id")?;
    let current_plan_version: Option<i64> = row.decode("current_plan_version")?;
    if current_plan_id.is_some() != current_plan_version.is_some() {
        return Err(TripRepoError::CorruptData);
    }
    if let Some(version) = current_plan_version {
        checked_revision(version).map_err(corrupt)?;
    }
    // Plans are deliberately outside this capability slice. A non-null pointer
    // cannot yet be proven against an authoritative plan row.
    if current_plan_id.is_some() {
        return Err(TripRepoError::CorruptData);
    }

    let labels_json: Option<String> = row.decode("stop_kind_labels_json")?;
    let budget_json: Option<String> = row.decode("soft_budget_json")?;
    let start_date: String = row.decode("start_date")?;
    let end_date: String = row.decode("end_date")?;
    let currency_raw: String = row.decode("base_currency")?;
    let base_currency = currency_raw.parse::<CurrencyCode>().map_err(corrupt)?;
    if base_currency.as_str() != currency_raw {
        return Err(TripRepoError::CorruptData);
    }
    let data = TripData {
        id: row.decode("id")?,
        name: row.decode("name")?,
        cover_photo_url: row.decode("cover_photo_url")?,
        accent_color: row.decode("accent_color")?,
        stop_kind_labels: decode_stop_kind_labels(labels_json.as_deref())?,
        status: parse_trip_status(&row.decode::<String>("status")?)?,
        dates: DateRange::try_new(start_date, end_date).map_err(corrupt)?,
        base_currency,
        soft_budget: decode_soft_budget(budget_json.as_deref())?,
        members: Vec::new(),
        current_plan_id,
        created_at: row.decode("created_at")?,
    };
    let revision: i64 = row.decode("revision")?;
    checked_revision(revision).map_err(corrupt)?;
    Ok(StoredTrip { data, revision })
}

pub(super) fn assemble_trip(
    stored: &StoredTrip,
    members: Vec<TripMember>,
) -> Result<Trip, TripRepoError> {
    let mut data = stored.data.clone();
    data.members = members;
    Trip::try_from(data).map_err(corrupt)
}

pub(super) fn decode_trip_member_row(row: &SqliteRow) -> Result<StoredTripMember, TripRepoError> {
    let trip_id: String = row.decode("trip_id")?;
    validate_id(&trip_id).map_err(corrupt)?;
    let revision: i64 = row.decode("membership_revision")?;
    checked_revision(revision).map_err(corrupt)?;
    let member = TripMember::try_new(
        row.decode("user_id")?,
        parse_trip_role(&row.decode::<String>("role")?)?,
        row.decode("joined_at")?,
    )
    .map_err(corrupt)?;
    Ok(StoredTripMember { member, revision })
}

pub(super) fn decode_invite_row(row: &SqliteRow) -> Result<StoredInvite, TripRepoError> {
    let trip_id: String = row.decode("trip_id")?;
    let digest: String = row.decode("email_digest")?;
    let invite = Invite::try_from(InviteData {
        id: row.decode("invite_id")?,
        trip_id: trip_id.clone(),
        email: row.decode("invite_email")?,
        invited_by: row.decode("invited_by")?,
        status: parse_invite_status(&row.decode::<String>("invite_status")?)?,
        created_at: row.decode("invite_created_at")?,
    })
    .map_err(corrupt)?;
    let revision: i64 = row.decode("invite_revision")?;
    let (_, expected_digest) = invite_key(&trip_id, &invite, revision)?;
    if digest != expected_digest {
        return Err(TripRepoError::CorruptData);
    }
    Ok(StoredInvite {
        invite,
        digest,
        revision,
    })
}

pub(super) fn decode_profile_row(row: &SqliteRow) -> Result<User, TripRepoError> {
    let id: String = row.decode("profile_id")?;
    let email_raw: String = row.decode("profile_email")?;
    let display_name: Option<String> = row.decode("profile_display_name")?;
    let revision: i64 = row.decode("profile_revision")?;
    let claim_digest: Option<String> = row.decode("claim_digest")?;
    let claim_user_id: Option<String> = row.decode("claim_user_id")?;
    let email = Email::parse(&email_raw).map_err(|_| TripRepoError::CorruptData)?;
    validate_id(&id).map_err(corrupt)?;
    validate_email(&email).map_err(corrupt)?;
    validate_optional_text(display_name.as_deref(), 200).map_err(corrupt)?;
    checked_revision(revision).map_err(corrupt)?;
    if email_raw != email.as_str()
        || claim_user_id.as_deref() != Some(id.as_str())
        || claim_digest.as_deref() != Some(email_digest(&email).as_str())
    {
        return Err(TripRepoError::CorruptData);
    }
    Ok(User {
        id: UserId(id),
        email,
        display_name,
    })
}

pub(super) fn encode_stop_kind_labels(
    labels: Option<&HashMap<StopKind, String>>,
) -> Result<Option<String>, TripRepoError> {
    let Some(labels) = labels else {
        return Ok(None);
    };
    let mut normalized = serde_json::Map::new();
    for (kind, value) in labels {
        normalized.insert(stop_kind_value(*kind).to_string(), value.clone().into());
    }
    let encoded = encode_json(&normalized).map_err(corrupt)?;
    if encoded.len() > MAX_VALUE_JSON_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    Ok(Some(encoded))
}

pub(super) fn encode_soft_budget(
    budget: Option<&SoftBudget>,
) -> Result<Option<String>, TripRepoError> {
    let Some(budget) = budget else {
        return Ok(None);
    };
    let encoded = encode_json(budget).map_err(corrupt)?;
    if encoded.len() > MAX_VALUE_JSON_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    Ok(Some(encoded))
}

fn decode_stop_kind_labels(
    value: Option<&str>,
) -> Result<Option<HashMap<StopKind, String>>, TripRepoError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() > MAX_VALUE_JSON_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    let labels: HashMap<StopKind, String> = decode_json(value).map_err(corrupt)?;
    if encode_stop_kind_labels(Some(&labels))?.as_deref() != Some(value) {
        return Err(TripRepoError::CorruptData);
    }
    Ok(Some(labels))
}

fn decode_soft_budget(value: Option<&str>) -> Result<Option<SoftBudget>, TripRepoError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() > MAX_VALUE_JSON_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    let budget: SoftBudget = decode_json(value).map_err(corrupt)?;
    if encode_soft_budget(Some(&budget))?.as_deref() != Some(value) {
        return Err(TripRepoError::CorruptData);
    }
    Ok(Some(budget))
}

pub(super) fn trip_status_value(status: TripStatus) -> &'static str {
    match status {
        TripStatus::Dreaming => "dreaming",
        TripStatus::Planning => "planning",
        TripStatus::Booked => "booked",
        TripStatus::Ongoing => "ongoing",
        TripStatus::Done => "done",
    }
}

pub(super) fn role_value(role: TripRole) -> &'static str {
    match role {
        TripRole::Leader => "leader",
        TripRole::Member => "member",
        TripRole::Viewer => "viewer",
    }
}

pub(super) fn invite_status_value(status: InviteStatus) -> &'static str {
    match status {
        InviteStatus::Pending => "pending",
        InviteStatus::Accepted => "accepted",
    }
}

pub(super) fn summary(trip: &Trip, member_count: usize) -> Result<TripSummary, TripRepoError> {
    let member_count = u32::try_from(member_count).map_err(|_| TripRepoError::CorruptData)?;
    Ok(TripSummary {
        id: trip.id().to_string(),
        name: trip.name().to_string(),
        cover_photo_url: trip.cover_photo_url().map(str::to_string),
        accent_color: trip.accent_color().map(str::to_string),
        status: trip.status(),
        start_date: trip.start_date().to_string(),
        end_date: trip.end_date().to_string(),
        member_count,
        cities: Vec::new(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EncodedProfile<'a> {
    id: &'a str,
    email: &'a str,
    display_name: &'a str,
    avatar_color: &'static str,
}

pub(super) fn encoded_profiles_size(users: &[User]) -> Result<usize, TripRepoError> {
    let values = users
        .iter()
        .map(|user| {
            let email = user.email.as_str();
            EncodedProfile {
                id: &user.id.0,
                email,
                display_name: user
                    .display_name
                    .as_deref()
                    .unwrap_or_else(|| email.split_once('@').map_or(email, |(local, _)| local)),
                avatar_color: avatar_color(email),
            }
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&values)
        .map(|encoded| encoded.len())
        .map_err(|_| TripRepoError::CorruptData)
}

fn avatar_color(email: &str) -> &'static str {
    const PALETTE: [&str; 6] = [
        "#6b5bd2", "#a0522d", "#e6b422", "#e05263", "#3b6fd4", "#4fb06d",
    ];
    let fold = email.bytes().fold(0u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte.into())
    });
    PALETTE[fold as usize % PALETTE.len()]
}

fn parse_trip_status(value: &str) -> Result<TripStatus, TripRepoError> {
    match value {
        "dreaming" => Ok(TripStatus::Dreaming),
        "planning" => Ok(TripStatus::Planning),
        "booked" => Ok(TripStatus::Booked),
        "ongoing" => Ok(TripStatus::Ongoing),
        "done" => Ok(TripStatus::Done),
        _ => Err(TripRepoError::CorruptData),
    }
}

fn parse_trip_role(value: &str) -> Result<TripRole, TripRepoError> {
    match value {
        "leader" => Ok(TripRole::Leader),
        "member" => Ok(TripRole::Member),
        "viewer" => Ok(TripRole::Viewer),
        _ => Err(TripRepoError::CorruptData),
    }
}

fn parse_invite_status(value: &str) -> Result<InviteStatus, TripRepoError> {
    match value {
        "pending" => Ok(InviteStatus::Pending),
        "accepted" => Ok(InviteStatus::Accepted),
        _ => Err(TripRepoError::CorruptData),
    }
}

fn stop_kind_value(value: StopKind) -> &'static str {
    match value {
        StopKind::Visit => "visit",
        StopKind::Meal => "meal",
        StopKind::Lodging => "lodging",
        StopKind::Activity => "activity",
        StopKind::Transit => "transit",
    }
}

trait SqliteRowExt {
    fn decode<T>(&self, column: &str) -> Result<T, TripRepoError>
    where
        T: for<'row> sqlx::Decode<'row, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>;
}

impl SqliteRowExt for SqliteRow {
    fn decode<T>(&self, column: &str) -> Result<T, TripRepoError>
    where
        T: for<'row> sqlx::Decode<'row, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
    {
        self.try_get(column).map_err(|_| TripRepoError::CorruptData)
    }
}

fn corrupt<T>(_error: T) -> TripRepoError {
    TripRepoError::CorruptData
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn profile_size_model_matches_the_authoritative_user_response_shape() {
        let user = User {
            id: UserId("u".to_string()),
            email: Email::parse("a@b").expect("valid email"),
            display_name: None,
        };
        let wire_value = json!([{
            "id": "u",
            "email": "a@b",
            "displayName": "a",
            "avatarColor": "#a0522d",
        }]);

        assert_eq!(avatar_color("a@b"), "#a0522d");
        assert_eq!(
            encoded_profiles_size(&[user]).unwrap(),
            serde_json::to_vec(&wire_value).unwrap().len()
        );
    }

    #[test]
    fn maximum_profile_collection_remains_below_the_four_mib_safety_ceiling() {
        let suffix = "@b";
        let email = format!("{}{}", "a".repeat(320 - suffix.len()), suffix);
        let maximal = User {
            id: UserId("\u{1f9ed}".repeat(200)),
            email: Email::parse(&email).expect("boundary email"),
            display_name: Some("\u{1f9ed}".repeat(200)),
        };
        let users = vec![maximal; MAX_COLLECTION_ITEMS];

        assert!(encoded_profiles_size(&users).unwrap() < MAX_COLLECTION_RESPONSE_BYTES);
    }
}
