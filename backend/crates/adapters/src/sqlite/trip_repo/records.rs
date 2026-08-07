//! Strict persisted-record codecs for the trip capability.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use itinera_core::{
    domain::{
        trip::{
            Invite, InviteStatus, SoftBudget, StopKind, Trip, TripMember, TripRole, TripStatus,
            TripSummary,
        },
        user::{Email, User, UserId},
    },
    ports::trip::TripRepoError,
};
use serde::Serialize;
use sqlx::{Row, sqlite::SqliteRow};
use url::Url;

use crate::sqlite::codec::{
    checked_revision, decode_json, email_digest, encode_json, validate_currency, validate_date,
    validate_email, validate_id, validate_optional_text, validate_text, validate_utc,
};

pub(super) const MAX_COLLECTION_ITEMS: usize = 1_000;
pub(super) const COLLECTION_QUERY_LIMIT: i64 = 1_001;
pub(super) const MAX_PENDING_INVITES_PER_EMAIL: usize = 100;
pub(super) const PENDING_INVITE_QUERY_LIMIT: i64 = 101;
pub(super) const MAX_COLLECTION_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_VALUE_JSON_BYTES: usize = 4 * 1024;

#[derive(Debug)]
pub(super) struct StoredTrip {
    pub(super) trip: Trip,
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

pub(super) fn validate_new_trip(trip: &Trip) -> Result<(), TripRepoError> {
    if trip.members.len() != 1
        || trip.members[0].role != TripRole::Leader
        || trip.current_plan_id.is_some()
    {
        return Err(TripRepoError::CorruptData);
    }
    validate_trip_fields(trip)?;
    validate_trip_member(&trip.id, &trip.members[0], 1)?;
    Ok(())
}

pub(super) fn validate_trip_fields(trip: &Trip) -> Result<(), TripRepoError> {
    validate_id(&trip.id).map_err(corrupt)?;
    validate_text(&trip.name, 120).map_err(corrupt)?;
    validate_optional_text(trip.accent_color.as_deref(), 128).map_err(corrupt)?;
    validate_cover_url(trip.cover_photo_url.as_deref())?;
    validate_date(&trip.start_date).map_err(corrupt)?;
    validate_date(&trip.end_date).map_err(corrupt)?;
    let start = NaiveDate::parse_from_str(&trip.start_date, "%Y-%m-%d")
        .map_err(|_| TripRepoError::CorruptData)?;
    let end = NaiveDate::parse_from_str(&trip.end_date, "%Y-%m-%d")
        .map_err(|_| TripRepoError::CorruptData)?;
    if !(0..90).contains(&end.signed_duration_since(start).num_days()) {
        return Err(TripRepoError::CorruptData);
    }
    validate_currency(&trip.base_currency).map_err(corrupt)?;
    validate_utc(&trip.created_at).map_err(corrupt)?;
    encode_stop_kind_labels(trip.stop_kind_labels.as_ref())?;
    encode_soft_budget(trip.soft_budget.as_ref())?;
    if let Some(plan_id) = trip.current_plan_id.as_deref() {
        validate_id(plan_id).map_err(corrupt)?;
    }
    Ok(())
}

pub(super) fn validate_trip_member(
    trip_id: &str,
    member: &TripMember,
    revision: i64,
) -> Result<(), TripRepoError> {
    validate_id(trip_id).map_err(corrupt)?;
    validate_id(&member.user_id).map_err(corrupt)?;
    validate_utc(&member.joined_at).map_err(corrupt)?;
    checked_revision(revision).map_err(corrupt)?;
    Ok(())
}

pub(super) fn validate_invite(
    trip_id: &str,
    invite: &Invite,
    revision: i64,
) -> Result<(Email, String), TripRepoError> {
    validate_id(trip_id).map_err(corrupt)?;
    validate_id(&invite.id).map_err(corrupt)?;
    validate_id(&invite.trip_id).map_err(corrupt)?;
    validate_id(&invite.invited_by).map_err(corrupt)?;
    validate_utc(&invite.created_at).map_err(corrupt)?;
    checked_revision(revision).map_err(corrupt)?;
    let email = Email::parse(&invite.email).map_err(|_| TripRepoError::CorruptData)?;
    validate_email(&email).map_err(corrupt)?;
    if invite.trip_id != trip_id || invite.email != email.as_str() {
        return Err(TripRepoError::CorruptData);
    }
    let digest = email_digest(&email);
    Ok((email, digest))
}

pub(super) fn decode_trip_row(row: &SqliteRow) -> Result<StoredTrip, TripRepoError> {
    let current_plan_id: Option<String> = required_column(row, "current_plan_id")?;
    let current_plan_version: Option<i64> = required_column(row, "current_plan_version")?;
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

    let labels_json: Option<String> = required_column(row, "stop_kind_labels_json")?;
    let budget_json: Option<String> = required_column(row, "soft_budget_json")?;
    let trip = Trip {
        id: required_column(row, "id")?,
        name: required_column(row, "name")?,
        cover_photo_url: required_column(row, "cover_photo_url")?,
        accent_color: required_column(row, "accent_color")?,
        stop_kind_labels: decode_stop_kind_labels(labels_json.as_deref())?,
        status: parse_trip_status(&required_column::<String>(row, "status")?)?,
        start_date: required_column(row, "start_date")?,
        end_date: required_column(row, "end_date")?,
        base_currency: required_column(row, "base_currency")?,
        soft_budget: decode_soft_budget(budget_json.as_deref())?,
        members: Vec::new(),
        current_plan_id,
        created_at: required_column(row, "created_at")?,
    };
    let revision: i64 = required_column(row, "revision")?;
    checked_revision(revision).map_err(corrupt)?;
    validate_trip_fields(&trip)?;
    Ok(StoredTrip { trip, revision })
}

pub(super) fn decode_trip_member_row(row: &SqliteRow) -> Result<StoredTripMember, TripRepoError> {
    let trip_id: String = required_column(row, "trip_id")?;
    let member = TripMember {
        user_id: required_column(row, "user_id")?,
        role: parse_trip_role(&required_column::<String>(row, "role")?)?,
        joined_at: required_column(row, "joined_at")?,
    };
    let revision: i64 = required_column(row, "membership_revision")?;
    validate_trip_member(&trip_id, &member, revision)?;
    Ok(StoredTripMember { member, revision })
}

pub(super) fn decode_invite_row(row: &SqliteRow) -> Result<StoredInvite, TripRepoError> {
    let trip_id: String = required_column(row, "trip_id")?;
    let digest: String = required_column(row, "email_digest")?;
    let invite = Invite {
        id: required_column(row, "invite_id")?,
        trip_id: trip_id.clone(),
        email: required_column(row, "invite_email")?,
        invited_by: required_column(row, "invited_by")?,
        status: parse_invite_status(&required_column::<String>(row, "invite_status")?)?,
        created_at: required_column(row, "invite_created_at")?,
    };
    let revision: i64 = required_column(row, "invite_revision")?;
    let (_, expected_digest) = validate_invite(&trip_id, &invite, revision)?;
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
    let id: String = required_column(row, "profile_id")?;
    let email_raw: String = required_column(row, "profile_email")?;
    let display_name: Option<String> = required_column(row, "profile_display_name")?;
    let revision: i64 = required_column(row, "profile_revision")?;
    let claim_digest: Option<String> = required_column(row, "claim_digest")?;
    let claim_user_id: Option<String> = required_column(row, "claim_user_id")?;
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
    if labels.len() > 5 {
        return Err(TripRepoError::CorruptData);
    }
    let mut normalized = serde_json::Map::new();
    for (kind, value) in labels {
        validate_text(value, 200).map_err(corrupt)?;
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
    if !budget.amount.is_finite() || budget.amount < 0.0 {
        return Err(TripRepoError::CorruptData);
    }
    validate_currency(&budget.currency).map_err(corrupt)?;
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
        id: trip.id.clone(),
        name: trip.name.clone(),
        cover_photo_url: trip.cover_photo_url.clone(),
        accent_color: trip.accent_color.clone(),
        status: trip.status,
        start_date: trip.start_date.clone(),
        end_date: trip.end_date.clone(),
        member_count,
        cities: Vec::new(),
    })
}

pub(super) fn ensure_unique_member_ids(members: &[TripMember]) -> Result<(), TripRepoError> {
    let unique = members
        .iter()
        .map(|member| member.user_id.as_str())
        .collect::<HashSet<_>>();
    if unique.len() == members.len() {
        Ok(())
    } else {
        Err(TripRepoError::CorruptData)
    }
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

fn validate_cover_url(value: Option<&str>) -> Result<(), TripRepoError> {
    let Some(value) = value else {
        return Ok(());
    };
    validate_text(value, 2_048).map_err(corrupt)?;
    let parsed = Url::parse(value).map_err(|_| TripRepoError::CorruptData)?;
    if matches!(parsed.scheme(), "http" | "https") {
        Ok(())
    } else {
        Err(TripRepoError::CorruptData)
    }
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

fn required_column<T>(row: &SqliteRow, name: &str) -> Result<T, TripRepoError>
where
    T: for<'decode> sqlx::Decode<'decode, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(name).map_err(|_| TripRepoError::CorruptData)
}

fn corrupt<T>(_error: T) -> TripRepoError {
    TripRepoError::CorruptData
}
