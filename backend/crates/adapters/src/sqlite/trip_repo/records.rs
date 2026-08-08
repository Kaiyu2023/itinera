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
use sqlx::FromRow;

use crate::sqlite::codec::{
    checked_revision, decode_json, email_digest, encode_json, validate_id, validate_optional_text,
};

pub(super) const MAX_COLLECTION_ITEMS: usize = 1_000;
pub(super) const COLLECTION_QUERY_LIMIT: i64 = 1_001;
pub(super) const MAX_PENDING_INVITES_PER_EMAIL: usize = 100;
pub(super) const PENDING_INVITE_QUERY_LIMIT: i64 = 101;
pub(super) const MAX_COLLECTION_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_VALUE_JSON_BYTES: usize = 4 * 1024;

#[derive(Debug)]
pub(super) struct Versioned<T> {
    pub(super) value: T,
    pub(super) revision: i64,
}

#[derive(Debug, Clone, FromRow)]
pub(super) struct TripRow {
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

#[derive(Debug, FromRow)]
pub(super) struct TripMemberRow {
    trip_id: String,
    user_id: String,
    role: String,
    joined_at: String,
    membership_revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct InviteRow {
    trip_id: String,
    email_digest: String,
    invite_id: String,
    invite_email: String,
    invited_by: String,
    invite_status: String,
    invite_created_at: String,
    invite_revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct ProfileRow {
    profile_id: String,
    profile_email: String,
    profile_display_name: Option<String>,
    profile_revision: i64,
    claim_digest: Option<String>,
    claim_user_id: Option<String>,
}

#[derive(Debug, FromRow)]
pub(super) struct TripNavigationRow {
    navigation_trip_id: String,
    #[sqlx(flatten)]
    trip: TripRow,
}

#[derive(Debug, FromRow)]
pub(super) struct InviteWithTripRow {
    #[sqlx(flatten)]
    invite: InviteRow,
    related_trip_id: Option<String>,
}

#[derive(Debug, FromRow)]
pub(super) struct TripMemberWithTripRow {
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

impl TripRow {
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    pub(super) fn current_plan_pointer(&self) -> Result<Option<(&str, u32)>, TripRepoError> {
        match (&self.current_plan_id, self.current_plan_version) {
            (None, None) => Ok(None),
            (Some(id), Some(version)) => {
                validate_id(id).map_err(corrupt)?;
                let version = u32::try_from(version)
                    .ok()
                    .filter(|version| *version > 0)
                    .ok_or(TripRepoError::CorruptData)?;
                Ok(Some((id, version)))
            }
            _ => Err(TripRepoError::CorruptData),
        }
    }

    pub(super) fn revision(&self) -> Result<i64, TripRepoError> {
        checked_revision(self.revision).map_err(corrupt)
    }

    pub(super) fn into_trip(
        self,
        members: Vec<TripMember>,
    ) -> Result<Versioned<Trip>, TripRepoError> {
        self.current_plan_pointer()?;
        let revision = self.revision()?;

        let base_currency = self
            .base_currency
            .parse::<CurrencyCode>()
            .map_err(corrupt)?;
        if base_currency.as_str() != self.base_currency {
            return Err(TripRepoError::CorruptData);
        }
        let data = TripData {
            id: self.id,
            name: self.name,
            cover_photo_url: self.cover_photo_url,
            accent_color: self.accent_color,
            stop_kind_labels: decode_stop_kind_labels(self.stop_kind_labels_json.as_deref())?,
            status: self.status.parse().map_err(corrupt)?,
            dates: DateRange::try_new(self.start_date, self.end_date).map_err(corrupt)?,
            base_currency,
            soft_budget: decode_soft_budget(self.soft_budget_json.as_deref())?,
            members,
            current_plan_id: self.current_plan_id,
            created_at: self.created_at,
        };
        Ok(Versioned {
            value: Trip::try_from(data).map_err(corrupt)?,
            revision,
        })
    }
}

impl TripNavigationRow {
    pub(super) fn into_trip_row(self) -> Result<TripRow, TripRepoError> {
        validate_id(&self.navigation_trip_id).map_err(corrupt)?;
        if self.trip.id() != self.navigation_trip_id {
            return Err(TripRepoError::CorruptData);
        }
        Ok(self.trip)
    }
}

impl TripMemberRow {
    pub(super) fn into_member(self) -> Result<(String, Versioned<TripMember>), TripRepoError> {
        validate_id(&self.trip_id).map_err(corrupt)?;
        checked_revision(self.membership_revision).map_err(corrupt)?;
        let member = TripMember::try_new(
            self.user_id,
            self.role.parse().map_err(corrupt)?,
            self.joined_at,
        )
        .map_err(corrupt)?;
        Ok((
            self.trip_id,
            Versioned {
                value: member,
                revision: self.membership_revision,
            },
        ))
    }
}

impl TripMemberWithTripRow {
    pub(super) fn into_member(self) -> Result<(String, Versioned<TripMember>), TripRepoError> {
        let (trip_id, member) = self.membership.into_member()?;
        if self.related_trip_id.as_deref() != Some(trip_id.as_str()) {
            return Err(TripRepoError::CorruptData);
        }
        Ok((trip_id, member))
    }
}

impl InviteRow {
    pub(super) fn into_invite(self) -> Result<Versioned<Invite>, TripRepoError> {
        let invite = Invite::try_from(InviteData {
            id: self.invite_id,
            trip_id: self.trip_id.clone(),
            email: self.invite_email,
            invited_by: self.invited_by,
            status: self.invite_status.parse().map_err(corrupt)?,
            created_at: self.invite_created_at,
        })
        .map_err(corrupt)?;
        let (_, expected_digest) = invite_key(&self.trip_id, &invite, self.invite_revision)?;
        if self.email_digest != expected_digest {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Versioned {
            value: invite,
            revision: self.invite_revision,
        })
    }
}

impl InviteWithTripRow {
    pub(super) fn into_invite(self) -> Result<Versioned<Invite>, TripRepoError> {
        let invite = self.invite.into_invite()?;
        if self.related_trip_id.as_deref() != Some(invite.value.trip_id()) {
            return Err(TripRepoError::CorruptData);
        }
        Ok(invite)
    }
}

impl ProfileRow {
    pub(super) fn into_user(self) -> Result<(User, String), TripRepoError> {
        let email = Email::parse_canonical(&self.profile_email).map_err(corrupt)?;
        validate_id(&self.profile_id).map_err(corrupt)?;
        validate_optional_text(self.profile_display_name.as_deref(), 200).map_err(corrupt)?;
        checked_revision(self.profile_revision).map_err(corrupt)?;
        let digest = email_digest(&email);
        if self.claim_user_id.as_deref() != Some(self.profile_id.as_str())
            || self.claim_digest.as_deref() != Some(digest.as_str())
        {
            return Err(TripRepoError::CorruptData);
        }
        Ok((
            User {
                id: UserId(self.profile_id),
                email,
                display_name: self.profile_display_name,
            },
            digest,
        ))
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

    use super::{
        Email, MAX_COLLECTION_ITEMS, MAX_COLLECTION_RESPONSE_BYTES, User, UserId, avatar_color,
        encoded_profiles_size,
    };

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
