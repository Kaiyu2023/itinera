//! Strict persisted-record codecs for the trip capability.

use std::collections::HashMap;

use itinera_core::{
    domain::{
        currency::CurrencyCode,
        trip::{
            DateRange, Invite, InviteData, SoftBudget, StopKind, Trip, TripData, TripMember,
            TripSummary,
        },
        user::{Email, User, UserId},
    },
    ports::trip::TripRepoError,
};
use serde::Serialize;
use sqlx::{FromRow, sqlite::SqliteRow};

use crate::sqlite::{
    codec::{
        checked_revision, decode_json, email_digest, encode_json, validate_id,
        validate_optional_text,
    },
    row::SqliteRecord,
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
    pub(super) trip_id: String,
    pub(super) member: TripMember,
    pub(super) revision: i64,
}

#[derive(Debug)]
pub(super) struct StoredInvite {
    pub(super) invite: Invite,
    pub(super) digest: String,
    pub(super) revision: i64,
}

#[derive(Debug)]
pub(super) struct StoredProfile {
    pub(super) user: User,
    pub(super) digest: String,
}

#[derive(Debug)]
pub(super) struct StoredTripNavigation {
    pub(super) trip: StoredTrip,
}

#[derive(Debug)]
pub(super) struct StoredInviteWithTrip {
    pub(super) invite: StoredInvite,
}

#[derive(Debug)]
pub(super) struct StoredTripMemberWithTrip {
    pub(super) membership: StoredTripMember,
}

#[derive(FromRow)]
struct TripRow {
    id: String,
    name: String,
    cover_photo_url: Option<String>,
    accent_color: Option<String>,
    stop_kind_labels_json: Option<String>,
    status: String,
    start_date: String,
    end_date: String,
    base_currency: String,
    soft_budget_json: Option<String>,
    current_plan_id: Option<String>,
    current_plan_version: Option<i64>,
    created_at: String,
    revision: i64,
}

#[derive(FromRow)]
struct TripMemberRow {
    trip_id: String,
    user_id: String,
    role: String,
    joined_at: String,
    membership_revision: i64,
}

#[derive(FromRow)]
struct InviteRow {
    trip_id: String,
    email_digest: String,
    invite_id: String,
    invite_email: String,
    invited_by: String,
    invite_status: String,
    invite_created_at: String,
    invite_revision: i64,
}

#[derive(FromRow)]
struct ProfileRow {
    profile_id: String,
    profile_email: String,
    profile_display_name: Option<String>,
    profile_revision: i64,
    claim_digest: Option<String>,
    claim_user_id: Option<String>,
}

#[derive(FromRow)]
struct TripNavigationRow {
    navigation_trip_id: String,
    #[sqlx(flatten)]
    trip: TripRow,
}

#[derive(FromRow)]
struct InviteWithTripRow {
    #[sqlx(flatten)]
    invite: InviteRow,
    related_trip_id: Option<String>,
}

#[derive(FromRow)]
struct TripMemberWithTripRow {
    #[sqlx(flatten)]
    membership: TripMemberRow,
    related_trip_id: Option<String>,
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

impl StoredTrip {
    fn try_from_row(row: TripRow) -> Result<Self, TripRepoError> {
        if row.current_plan_id.is_some() != row.current_plan_version.is_some() {
            return Err(TripRepoError::CorruptData);
        }
        if let Some(version) = row.current_plan_version {
            checked_revision(version).map_err(corrupt)?;
        }
        // Plans are deliberately outside this capability slice. A non-null pointer
        // cannot yet be proven against an authoritative plan row.
        if row.current_plan_id.is_some() {
            return Err(TripRepoError::CorruptData);
        }

        let base_currency = row.base_currency.parse::<CurrencyCode>().map_err(corrupt)?;
        if base_currency.as_str() != row.base_currency {
            return Err(TripRepoError::CorruptData);
        }
        let data = TripData {
            id: row.id,
            name: row.name,
            cover_photo_url: row.cover_photo_url,
            accent_color: row.accent_color,
            stop_kind_labels: decode_stop_kind_labels(row.stop_kind_labels_json.as_deref())?,
            status: row.status.parse().map_err(corrupt)?,
            dates: DateRange::try_new(row.start_date, row.end_date).map_err(corrupt)?,
            base_currency,
            soft_budget: decode_soft_budget(row.soft_budget_json.as_deref())?,
            members: Vec::new(),
            current_plan_id: row.current_plan_id,
            created_at: row.created_at,
        };
        checked_revision(row.revision).map_err(corrupt)?;
        Ok(Self {
            data,
            revision: row.revision,
        })
    }
}

impl SqliteRecord for StoredTrip {
    type Error = TripRepoError;

    fn try_from_sqlite_row(row: &SqliteRow) -> Result<Self, Self::Error> {
        Self::try_from_row(TripRow::from_row(row).map_err(corrupt)?)
    }
}

impl SqliteRecord for StoredTripNavigation {
    type Error = TripRepoError;

    fn try_from_sqlite_row(row: &SqliteRow) -> Result<Self, Self::Error> {
        let row = TripNavigationRow::from_row(row).map_err(corrupt)?;
        validate_id(&row.navigation_trip_id).map_err(corrupt)?;
        let trip = StoredTrip::try_from_row(row.trip)?;
        if trip.data.id != row.navigation_trip_id {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Self { trip })
    }
}

pub(super) fn assemble_trip(
    stored: &StoredTrip,
    members: Vec<TripMember>,
) -> Result<Trip, TripRepoError> {
    let mut data = stored.data.clone();
    data.members = members;
    Trip::try_from(data).map_err(corrupt)
}

impl StoredTripMember {
    fn try_from_row(row: TripMemberRow) -> Result<Self, TripRepoError> {
        validate_id(&row.trip_id).map_err(corrupt)?;
        checked_revision(row.membership_revision).map_err(corrupt)?;
        let member = TripMember::try_new(
            row.user_id,
            row.role.parse().map_err(corrupt)?,
            row.joined_at,
        )
        .map_err(corrupt)?;
        Ok(Self {
            trip_id: row.trip_id,
            member,
            revision: row.membership_revision,
        })
    }
}

impl SqliteRecord for StoredTripMember {
    type Error = TripRepoError;

    fn try_from_sqlite_row(row: &SqliteRow) -> Result<Self, Self::Error> {
        Self::try_from_row(TripMemberRow::from_row(row).map_err(corrupt)?)
    }
}

impl SqliteRecord for StoredTripMemberWithTrip {
    type Error = TripRepoError;

    fn try_from_sqlite_row(row: &SqliteRow) -> Result<Self, Self::Error> {
        let row = TripMemberWithTripRow::from_row(row).map_err(corrupt)?;
        let membership = StoredTripMember::try_from_row(row.membership)?;
        if row.related_trip_id.as_deref() != Some(membership.trip_id.as_str()) {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Self { membership })
    }
}

impl StoredInvite {
    fn try_from_row(row: InviteRow) -> Result<Self, TripRepoError> {
        let invite = Invite::try_from(InviteData {
            id: row.invite_id,
            trip_id: row.trip_id.clone(),
            email: row.invite_email,
            invited_by: row.invited_by,
            status: row.invite_status.parse().map_err(corrupt)?,
            created_at: row.invite_created_at,
        })
        .map_err(corrupt)?;
        let (_, expected_digest) = invite_key(&row.trip_id, &invite, row.invite_revision)?;
        if row.email_digest != expected_digest {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Self {
            invite,
            digest: row.email_digest,
            revision: row.invite_revision,
        })
    }
}

impl SqliteRecord for StoredInvite {
    type Error = TripRepoError;

    fn try_from_sqlite_row(row: &SqliteRow) -> Result<Self, Self::Error> {
        Self::try_from_row(InviteRow::from_row(row).map_err(corrupt)?)
    }
}

impl SqliteRecord for StoredInviteWithTrip {
    type Error = TripRepoError;

    fn try_from_sqlite_row(row: &SqliteRow) -> Result<Self, Self::Error> {
        let row = InviteWithTripRow::from_row(row).map_err(corrupt)?;
        let invite = StoredInvite::try_from_row(row.invite)?;
        if row.related_trip_id.as_deref() != Some(invite.invite.trip_id()) {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Self { invite })
    }
}

impl SqliteRecord for StoredProfile {
    type Error = TripRepoError;

    fn try_from_sqlite_row(row: &SqliteRow) -> Result<Self, Self::Error> {
        let row = ProfileRow::from_row(row).map_err(corrupt)?;
        let email = Email::parse_canonical(&row.profile_email).map_err(corrupt)?;
        validate_id(&row.profile_id).map_err(corrupt)?;
        validate_optional_text(row.profile_display_name.as_deref(), 200).map_err(corrupt)?;
        checked_revision(row.profile_revision).map_err(corrupt)?;
        let digest = email_digest(&email);
        if row.claim_user_id.as_deref() != Some(row.profile_id.as_str())
            || row.claim_digest.as_deref() != Some(digest.as_str())
        {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Self {
            user: User {
                id: UserId(row.profile_id),
                email,
                display_name: row.profile_display_name,
            },
            digest,
        })
    }
}

pub(super) fn encode_stop_kind_labels(
    labels: Option<&HashMap<StopKind, String>>,
) -> Result<Option<String>, TripRepoError> {
    let Some(labels) = labels else {
        return Ok(None);
    };
    let mut normalized = serde_json::Map::new();
    for (kind, value) in labels {
        normalized.insert(kind.as_ref().to_string(), value.clone().into());
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
