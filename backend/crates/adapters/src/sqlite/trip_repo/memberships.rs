//! Membership and invitation lifecycle operations.

use itinera_core::{
    domain::{
        trip::{Invite, InviteStatus, PendingInvite, TripMember, TripRole, TripValidationError},
        user::{User, UserId},
    },
    ports::{authorization::TripAuthorizationContext, trip::TripRepoError},
};
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{
    SqliteDb,
    codec::{email_digest, next_revision, validate_id, validate_optional_text},
    row::SqliteRowExt,
};

use super::{
    access::{
        RequiredRole, authorize, load_members_and_validate_capacity, load_trip, member_values,
        require_human_authorization, trip_distinct_digests, user_distinct_trip_ids,
        validate_stored_user,
    },
    records::{
        InviteRow, MAX_COLLECTION_ITEMS, MAX_COLLECTION_RESPONSE_BYTES,
        MAX_PENDING_INVITES_PER_EMAIL, TripMemberRow, Versioned, encoded_profiles_size, invite_key,
    },
};

pub(super) async fn get_members(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<User>, TripRepoError> {
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::AnyMember,
    )
    .await?;
    let trip_row = load_trip(&mut transaction, trip_id).await?;
    let profiles = load_members_and_validate_capacity(&mut transaction, trip_id).await?;
    trip_row.into_trip(member_values(&profiles))?;
    let users = profiles
        .into_iter()
        .map(|profile| profile.user)
        .collect::<Vec<_>>();
    if encoded_profiles_size(&users)? > MAX_COLLECTION_RESPONSE_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(users)
}

pub(super) async fn remove_member(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    target: &UserId,
) -> Result<(), TripRepoError> {
    validate_id(&target.0).map_err(|_| TripRepoError::CorruptData)?;
    let mut transaction = db
        .begin_immediate()
        .await
        .map_err(|_| TripRepoError::Unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Leader,
    )
    .await?;
    let trip_row = load_trip(&mut transaction, trip_id).await?;
    let profiles = load_members_and_validate_capacity(&mut transaction, trip_id).await?;
    let target_membership_revision = profiles
        .iter()
        .find(|profile| profile.member.user_id() == target.0)
        .map(|profile| profile.membership_revision)
        .ok_or(TripRepoError::NotFound)?;
    let versioned_trip = trip_row.into_trip(member_values(&profiles))?;
    let mut trip = versioned_trip.value;
    trip.remove_member(&target.0)
        .map_err(map_membership_mutation)?;
    let next_trip_revision =
        next_revision(versioned_trip.revision).map_err(|_| TripRepoError::CorruptData)?;

    let deleted = sqlx::query(
        "DELETE FROM trip_memberships \
         WHERE trip_id = ? AND user_id = ? AND revision = ?",
    )
    .bind(trip_id)
    .bind(&target.0)
    .bind(target_membership_revision)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    if deleted.rows_affected() != 1 {
        return Err(TripRepoError::Conflict);
    }
    let updated = sqlx::query("UPDATE trips SET revision = ? WHERE id = ? AND revision = ?")
        .bind(next_trip_revision)
        .bind(trip_id)
        .bind(versioned_trip.revision)
        .execute(&mut *transaction)
        .await
        .map_err(unavailable)?;
    if updated.rows_affected() != 1 {
        return Err(TripRepoError::Conflict);
    }
    db.commit(transaction).await.map_err(unavailable)
}

pub(super) async fn create_invite(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    invite: PendingInvite,
) -> Result<Invite, TripRepoError> {
    let actor = require_human_authorization(authorization)?;
    let invite = invite.into_invite();
    let (email, digest) = invite_key(trip_id, &invite, 1)?;
    if invite.invited_by() != actor.0 {
        return Err(TripRepoError::CorruptData);
    }

    let mut transaction = db
        .begin_immediate()
        .await
        .map_err(|_| TripRepoError::Unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Leader,
    )
    .await?;
    let trip_row = load_trip(&mut transaction, trip_id).await?;
    let profiles = load_members_and_validate_capacity(&mut transaction, trip_id).await?;
    trip_row.into_trip(member_values(&profiles))?;

    let existing = load_invite(&mut transaction, trip_id, &digest).await?;
    if existing
        .as_ref()
        .is_some_and(|stored| stored.value.status() == InviteStatus::Pending)
    {
        return Err(TripRepoError::DuplicateInvite);
    }

    let id_owner: Option<String> =
        sqlx::query_scalar("SELECT email_digest FROM trip_invites WHERE trip_id = ? AND id = ?")
            .bind(trip_id)
            .bind(invite.id())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(unavailable)?;
    if id_owner.as_deref().is_some_and(|owner| owner != digest) {
        return Err(TripRepoError::Conflict);
    }

    let mut trip_digests = trip_distinct_digests(&mut transaction, trip_id).await?;
    trip_digests.insert(digest.clone());
    if trip_digests.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::Conflict);
    }

    let (mut user_trip_ids, pending) =
        user_distinct_trip_ids(&mut transaction, &email, &digest).await?;
    if pending.len() >= MAX_PENDING_INVITES_PER_EMAIL {
        return Err(TripRepoError::Conflict);
    }
    user_trip_ids.insert(trip_id.to_string());
    if user_trip_ids.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::Conflict);
    }

    match existing {
        Some(stored) => {
            let revision =
                next_revision(stored.revision).map_err(|_| TripRepoError::CorruptData)?;
            let updated = sqlx::query(
                "UPDATE trip_invites \
                 SET id = ?, email = ?, invited_by = ?, status = ?, created_at = ?, revision = ? \
                 WHERE trip_id = ? AND email_digest = ? AND revision = ? AND status = 'accepted'",
            )
            .bind(invite.id())
            .bind(invite.email())
            .bind(invite.invited_by())
            .bind(invite.status().as_ref())
            .bind(invite.created_at())
            .bind(revision)
            .bind(trip_id)
            .bind(&digest)
            .bind(stored.revision)
            .execute(&mut *transaction)
            .await
            .map_err(unavailable)?;
            if updated.rows_affected() != 1 {
                return Err(TripRepoError::Conflict);
            }
        }
        None => {
            sqlx::query(
                "INSERT INTO trip_invites (\
                    trip_id, email_digest, id, email, invited_by, status, created_at, revision\
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, 1)",
            )
            .bind(trip_id)
            .bind(&digest)
            .bind(invite.id())
            .bind(invite.email())
            .bind(invite.invited_by())
            .bind(invite.status().as_ref())
            .bind(invite.created_at())
            .execute(&mut *transaction)
            .await
            .map_err(unavailable)?;
        }
    }
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(invite)
}

pub(super) async fn accept_pending_invites(
    db: &SqliteDb,
    authorization: &TripAuthorizationContext,
    user: &User,
    joined_at: &str,
) -> Result<(), TripRepoError> {
    validate_id(&user.id.0).map_err(|_| TripRepoError::CorruptData)?;
    validate_optional_text(user.display_name.as_deref(), 200)
        .map_err(|_| TripRepoError::CorruptData)?;
    let new_member =
        TripMember::try_new(user.id.0.clone(), TripRole::Member, joined_at.to_string())
            .map_err(|_| TripRepoError::CorruptData)?;
    let digest = email_digest(&user.email);

    let mut transaction = db
        .begin_immediate()
        .await
        .map_err(|_| TripRepoError::Unavailable)?;
    let actor = require_human_authorization(authorization)?;
    if actor != &user.id {
        return Err(TripRepoError::Forbidden);
    }
    validate_stored_user(&mut transaction, user).await?;
    let (user_trip_ids, pending) =
        user_distinct_trip_ids(&mut transaction, &user.email, &digest).await?;
    if user_trip_ids.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::CorruptData);
    }
    if pending.is_empty() {
        db.commit(transaction).await.map_err(unavailable)?;
        return Ok(());
    }

    for invite in pending {
        accept_one(&mut transaction, user, &new_member, &invite).await?;
    }
    db.commit(transaction).await.map_err(unavailable)
}

async fn accept_one(
    transaction: &mut Transaction<'static, Sqlite>,
    user: &User,
    new_member: &TripMember,
    invite: &Versioned<Invite>,
) -> Result<(), TripRepoError> {
    if invite.value.email() != user.email.as_str() || invite.value.status() != InviteStatus::Pending
    {
        return Err(TripRepoError::CorruptData);
    }
    let trip_id = invite.value.trip_id();
    let (_, digest) = invite_key(trip_id, &invite.value, invite.revision)?;
    let trip_row = load_trip(transaction, trip_id).await?;
    let profiles = load_members_and_validate_capacity(transaction, trip_id).await?;
    let versioned_trip = trip_row.into_trip(member_values(&profiles))?;
    let mut trip = versioned_trip.value;
    let trip_digests = trip_distinct_digests(transaction, trip_id).await?;
    if trip_digests.len() > MAX_COLLECTION_ITEMS || !trip_digests.contains(&digest) {
        return Err(TripRepoError::CorruptData);
    }

    let existing = sqlx::query(
        "SELECT trip_id, user_id, role, joined_at, revision AS membership_revision \
         FROM trip_memberships WHERE trip_id = ? AND user_id = ?",
    )
    .bind(trip_id)
    .bind(&user.id.0)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?;

    if let Some(row) = existing {
        let membership_row: TripMemberRow = row.decode()?;
        let (membership_trip_id, membership) = membership_row.into_member()?;
        if membership_trip_id != trip_id || membership.value.user_id() != user.id.0 {
            return Err(TripRepoError::CorruptData);
        }
    } else {
        trip.add_member(new_member.clone())
            .map_err(map_membership_mutation)?;
        sqlx::query(
            "INSERT INTO trip_memberships \
             (trip_id, user_id, role, joined_at, revision) VALUES (?, ?, ?, ?, 1)",
        )
        .bind(trip_id)
        .bind(&user.id.0)
        .bind(TripRole::Member.as_ref())
        .bind(new_member.joined_at())
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;

        let next =
            next_revision(versioned_trip.revision).map_err(|_| TripRepoError::CorruptData)?;
        let updated = sqlx::query("UPDATE trips SET revision = ? WHERE id = ? AND revision = ?")
            .bind(next)
            .bind(trip_id)
            .bind(versioned_trip.revision)
            .execute(&mut **transaction)
            .await
            .map_err(unavailable)?;
        if updated.rows_affected() != 1 {
            return Err(TripRepoError::Conflict);
        }
    }

    let next_invite_revision =
        next_revision(invite.revision).map_err(|_| TripRepoError::CorruptData)?;
    let accepted = sqlx::query(
        "UPDATE trip_invites SET status = 'accepted', revision = ? \
         WHERE trip_id = ? AND email_digest = ? AND status = 'pending' AND revision = ?",
    )
    .bind(next_invite_revision)
    .bind(trip_id)
    .bind(&digest)
    .bind(invite.revision)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if accepted.rows_affected() != 1 {
        return Err(TripRepoError::Conflict);
    }
    Ok(())
}

async fn load_invite(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    digest: &str,
) -> Result<Option<Versioned<Invite>>, TripRepoError> {
    let row = sqlx::query(
        "SELECT trip_id, email_digest, id AS invite_id, email AS invite_email, \
                invited_by, status AS invite_status, created_at AS invite_created_at, \
                revision AS invite_revision \
         FROM trip_invites WHERE trip_id = ? AND email_digest = ?",
    )
    .bind(trip_id)
    .bind(digest)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?;
    row.as_ref()
        .map(|row| row.decode::<InviteRow>()?.into_invite())
        .transpose()
}

fn unavailable(_error: sqlx::Error) -> TripRepoError {
    TripRepoError::Unavailable
}

fn map_membership_mutation(error: TripValidationError) -> TripRepoError {
    match error {
        TripValidationError::MissingLeader | TripValidationError::TooManyMembers => {
            TripRepoError::Conflict
        }
        _ => TripRepoError::CorruptData,
    }
}
