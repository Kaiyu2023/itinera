//! Transaction-local authorization and bounded aggregate reads.

use std::collections::HashSet;

use itinera_core::{
    domain::{
        trip::{InviteStatus, TripMember, TripRole},
        user::{Email, User, UserId},
    },
    ports::trip::TripRepoError,
};
use sqlx::{Row, Sqlite, Transaction};

use crate::sqlite::codec::{email_digest, validate_id};

use super::records::{
    COLLECTION_QUERY_LIMIT, MAX_COLLECTION_ITEMS, MAX_PENDING_INVITES_PER_EMAIL,
    PENDING_INVITE_QUERY_LIMIT, StoredInvite, StoredTrip, StoredTripMember, decode_invite_row,
    decode_profile_row, decode_trip_member_row, decode_trip_row,
};

#[derive(Debug, Clone, Copy)]
pub(super) enum RequiredRole {
    AnyMember,
    Leader,
}

#[derive(Debug)]
pub(super) struct MemberProfile {
    pub(super) membership: StoredTripMember,
    pub(super) user: User,
    pub(super) digest: String,
}

pub(super) async fn authorize(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    actor: &UserId,
    required: RequiredRole,
) -> Result<TripRole, TripRepoError> {
    validate_id(trip_id).map_err(|_| TripRepoError::CorruptData)?;
    validate_id(&actor.0).map_err(|_| TripRepoError::CorruptData)?;
    let row = sqlx::query(
        "SELECT trip_id, user_id, role, joined_at, revision AS membership_revision \
         FROM trip_memberships WHERE trip_id = ? AND user_id = ?",
    )
    .bind(trip_id)
    .bind(&actor.0)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let Some(row) = row else {
        // Do not reveal whether another caller's trip exists.
        return Err(TripRepoError::NotFound);
    };
    let role = decode_trip_member_row(&row)?.member.role();
    if matches!(required, RequiredRole::Leader) && role != TripRole::Leader {
        return Err(TripRepoError::Forbidden);
    }
    Ok(role)
}

pub(super) async fn load_trip(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<StoredTrip, TripRepoError> {
    let row = sqlx::query(
        "SELECT id, name, cover_photo_url, accent_color, stop_kind_labels_json, \
                status, start_date, end_date, base_currency, soft_budget_json, \
                current_plan_id, current_plan_version, created_at, revision \
         FROM trips WHERE id = ?",
    )
    .bind(trip_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?
    .ok_or(TripRepoError::CorruptData)?;
    decode_trip_row(&row)
}

pub(super) async fn load_member_profiles(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<MemberProfile>, TripRepoError> {
    let rows = sqlx::query(
        "SELECT m.trip_id, m.user_id, m.role, m.joined_at, \
                m.revision AS membership_revision, \
                u.id AS profile_id, u.email AS profile_email, \
                u.display_name AS profile_display_name, \
                u.revision AS profile_revision, \
                c.email_digest AS claim_digest, c.user_id AS claim_user_id \
         FROM trip_memberships AS m \
         LEFT JOIN users AS u ON u.id = m.user_id \
         LEFT JOIN user_email_claims AS c ON c.user_id = u.id \
         WHERE m.trip_id = ? \
         ORDER BY m.user_id \
         LIMIT ?",
    )
    .bind(trip_id)
    .bind(COLLECTION_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::CorruptData);
    }

    let mut profiles = Vec::with_capacity(rows.len());
    for row in rows {
        let membership = decode_trip_member_row(&row)?;
        let user = decode_profile_row(&row)?;
        if membership.member.user_id() != user.id.0 {
            return Err(TripRepoError::CorruptData);
        }
        let digest = email_digest(&user.email);
        profiles.push(MemberProfile {
            membership,
            user,
            digest,
        });
    }
    Ok(profiles)
}

pub(super) async fn load_members_and_validate_capacity(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<MemberProfile>, TripRepoError> {
    let profiles = load_member_profiles(transaction, trip_id).await?;
    let mut digests = profiles
        .iter()
        .map(|profile| profile.digest.clone())
        .collect::<HashSet<_>>();
    for invite in pending_invites_for_trip(transaction, trip_id).await? {
        digests.insert(invite.digest);
    }
    if digests.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::CorruptData);
    }
    Ok(profiles)
}

pub(super) async fn trip_distinct_digests(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<HashSet<String>, TripRepoError> {
    let profiles = load_member_profiles(transaction, trip_id).await?;
    let mut digests = profiles
        .into_iter()
        .map(|profile| profile.digest)
        .collect::<HashSet<_>>();
    for invite in pending_invites_for_trip(transaction, trip_id).await? {
        digests.insert(invite.digest);
    }
    if digests.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::CorruptData);
    }
    Ok(digests)
}

pub(super) async fn pending_invites_for_trip(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<StoredInvite>, TripRepoError> {
    let rows = sqlx::query(
        "SELECT trip_id, email_digest, id AS invite_id, email AS invite_email, \
                invited_by, status AS invite_status, created_at AS invite_created_at, \
                revision AS invite_revision \
         FROM trip_invites \
         WHERE trip_id = ? AND status = 'pending' \
         ORDER BY email_digest \
         LIMIT ?",
    )
    .bind(trip_id)
    .bind(COLLECTION_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::CorruptData);
    }
    rows.iter().map(decode_invite_row).collect()
}

pub(super) async fn pending_invites_for_email(
    transaction: &mut Transaction<'static, Sqlite>,
    digest: &str,
) -> Result<Vec<StoredInvite>, TripRepoError> {
    let rows = sqlx::query(
        "SELECT i.trip_id, i.email_digest, i.id AS invite_id, \
                i.email AS invite_email, \
                i.invited_by, i.status AS invite_status, \
                i.created_at AS invite_created_at, \
                i.revision AS invite_revision, t.id AS related_trip_id \
         FROM trip_invites AS i \
         LEFT JOIN trips AS t ON t.id = i.trip_id \
         WHERE i.email_digest = ? AND i.status = 'pending' \
         ORDER BY i.trip_id \
         LIMIT ?",
    )
    .bind(digest)
    .bind(PENDING_INVITE_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() > MAX_PENDING_INVITES_PER_EMAIL {
        return Err(TripRepoError::CorruptData);
    }
    let invites = rows
        .iter()
        .map(|row| {
            let invite = decode_invite_row(row)?;
            let related_trip_id: Option<String> = row
                .try_get("related_trip_id")
                .map_err(|_| TripRepoError::CorruptData)?;
            if related_trip_id.as_deref() != Some(invite.invite.trip_id()) {
                return Err(TripRepoError::CorruptData);
            }
            Ok(invite)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if invites
        .iter()
        .any(|invite| invite.digest != digest || invite.invite.status() != InviteStatus::Pending)
    {
        return Err(TripRepoError::CorruptData);
    }
    Ok(invites)
}

pub(super) async fn load_profile_by_id(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: &UserId,
) -> Result<Option<User>, TripRepoError> {
    validate_id(&user_id.0).map_err(|_| TripRepoError::CorruptData)?;
    let row = sqlx::query(
        "SELECT u.id AS profile_id, u.email AS profile_email, \
                u.display_name AS profile_display_name, u.revision AS profile_revision, \
                c.email_digest AS claim_digest, c.user_id AS claim_user_id \
         FROM users AS u \
         LEFT JOIN user_email_claims AS c ON c.user_id = u.id \
         WHERE u.id = ?",
    )
    .bind(&user_id.0)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?;
    row.as_ref().map(decode_profile_row).transpose()
}

pub(super) async fn load_profile_by_digest(
    transaction: &mut Transaction<'static, Sqlite>,
    email: &Email,
    digest: &str,
) -> Result<Option<User>, TripRepoError> {
    let row = sqlx::query(
        "SELECT u.id AS profile_id, u.email AS profile_email, \
                u.display_name AS profile_display_name, u.revision AS profile_revision, \
                c.email_digest AS claim_digest, c.user_id AS claim_user_id \
         FROM user_email_claims AS c \
         LEFT JOIN users AS u ON u.id = c.user_id \
         WHERE c.email_digest = ?",
    )
    .bind(digest)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let Some(row) = row else {
        let orphan: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email = ?")
            .bind(email.as_str())
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
        return if orphan == 0 {
            Ok(None)
        } else {
            Err(TripRepoError::CorruptData)
        };
    };
    let user = decode_profile_row(&row)?;
    if &user.email != email || email_digest(&user.email) != digest {
        return Err(TripRepoError::CorruptData);
    }
    Ok(Some(user))
}

pub(super) async fn validate_stored_user(
    transaction: &mut Transaction<'static, Sqlite>,
    expected: &User,
) -> Result<(), TripRepoError> {
    let stored = load_profile_by_id(transaction, &expected.id)
        .await?
        .ok_or(TripRepoError::CorruptData)?;
    if &stored == expected {
        Ok(())
    } else {
        Err(TripRepoError::CorruptData)
    }
}

pub(super) async fn user_distinct_trip_ids(
    transaction: &mut Transaction<'static, Sqlite>,
    email: &Email,
    digest: &str,
) -> Result<(HashSet<String>, Vec<StoredInvite>), TripRepoError> {
    let profile = load_profile_by_digest(transaction, email, digest).await?;
    let mut trip_ids = HashSet::new();
    if let Some(profile) = profile {
        let rows = sqlx::query(
            "SELECT m.trip_id, m.user_id, m.role, m.joined_at, \
                    m.revision AS membership_revision, t.id AS related_trip_id \
             FROM trip_memberships AS m \
             LEFT JOIN trips AS t ON t.id = m.trip_id \
             WHERE m.user_id = ? \
             ORDER BY m.trip_id \
             LIMIT ?",
        )
        .bind(&profile.id.0)
        .bind(COLLECTION_QUERY_LIMIT)
        .fetch_all(&mut **transaction)
        .await
        .map_err(unavailable)?;
        if rows.len() > MAX_COLLECTION_ITEMS {
            return Err(TripRepoError::CorruptData);
        }
        for row in rows {
            let membership = decode_trip_member_row(&row)?;
            if membership.member.user_id() != profile.id.0 {
                return Err(TripRepoError::CorruptData);
            }
            let trip_id: String = row
                .try_get("trip_id")
                .map_err(|_| TripRepoError::CorruptData)?;
            let related_trip_id: Option<String> = row
                .try_get("related_trip_id")
                .map_err(|_| TripRepoError::CorruptData)?;
            if related_trip_id.as_deref() != Some(trip_id.as_str()) {
                return Err(TripRepoError::CorruptData);
            }
            trip_ids.insert(trip_id);
        }
    }
    let pending = pending_invites_for_email(transaction, digest).await?;
    for invite in &pending {
        if invite.invite.email() != email.as_str() {
            return Err(TripRepoError::CorruptData);
        }
        trip_ids.insert(invite.invite.trip_id().to_string());
    }
    if trip_ids.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::CorruptData);
    }
    Ok((trip_ids, pending))
}

pub(super) fn member_values(profiles: &[MemberProfile]) -> Vec<TripMember> {
    profiles
        .iter()
        .map(|profile| profile.membership.member.clone())
        .collect()
}

fn unavailable(_error: sqlx::Error) -> TripRepoError {
    TripRepoError::Unavailable
}
