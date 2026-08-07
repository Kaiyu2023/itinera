//! Trip creation and bounded private reads.

use crate::sqlite::{
    SqliteDb,
    codec::{email_digest, ensure_encoded_size, validate_id},
};
use itinera_core::{
    domain::{
        trip::{Trip, TripSummary},
        user::UserId,
    },
    ports::trip::TripRepoError,
};

use super::{
    access::{
        RequiredRole, authorize, load_members_and_validate_capacity, load_profile_by_id, load_trip,
        member_values, user_distinct_trip_ids,
    },
    records::{
        COLLECTION_QUERY_LIMIT, MAX_COLLECTION_ITEMS, MAX_COLLECTION_RESPONSE_BYTES,
        encode_soft_budget, encode_stop_kind_labels, role_value, summary, trip_status_value,
        validate_new_trip,
    },
};

pub(super) async fn create_trip(db: &SqliteDb, trip: Trip) -> Result<Trip, TripRepoError> {
    validate_new_trip(&trip)?;
    let creator = UserId(trip.members[0].user_id.clone());
    let mut transaction = db
        .begin_immediate()
        .await
        .map_err(|_| TripRepoError::Unavailable)?;

    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trips WHERE id = ?")
        .bind(&trip.id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(unavailable)?;
    if existing != 0 {
        return Err(TripRepoError::Conflict);
    }

    let profile = load_profile_by_id(&mut transaction, &creator)
        .await?
        .ok_or(TripRepoError::CorruptData)?;
    let digest = email_digest(&profile.email);
    let (mut trip_ids, _) =
        user_distinct_trip_ids(&mut transaction, &profile.email, &digest).await?;
    trip_ids.insert(trip.id.clone());
    if trip_ids.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::Conflict);
    }

    let labels = encode_stop_kind_labels(trip.stop_kind_labels.as_ref())?;
    let budget = encode_soft_budget(trip.soft_budget.as_ref())?;
    sqlx::query(
        "INSERT INTO trips (\
            id, name, cover_photo_url, accent_color, stop_kind_labels_json, status, \
            start_date, end_date, base_currency, soft_budget_json, current_plan_id, \
            current_plan_version, created_at, revision\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, 1)",
    )
    .bind(&trip.id)
    .bind(&trip.name)
    .bind(&trip.cover_photo_url)
    .bind(&trip.accent_color)
    .bind(labels)
    .bind(trip_status_value(trip.status))
    .bind(&trip.start_date)
    .bind(&trip.end_date)
    .bind(&trip.base_currency)
    .bind(budget)
    .bind(&trip.created_at)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    sqlx::query(
        "INSERT INTO trip_memberships \
         (trip_id, user_id, role, joined_at, revision) VALUES (?, ?, ?, ?, 1)",
    )
    .bind(&trip.id)
    .bind(&creator.0)
    .bind(role_value(trip.members[0].role))
    .bind(&trip.members[0].joined_at)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;

    db.commit(transaction).await.map_err(unavailable)?;
    Ok(trip)
}

pub(super) async fn list_trips(
    db: &SqliteDb,
    actor: &UserId,
) -> Result<Vec<TripSummary>, TripRepoError> {
    validate_id(&actor.0).map_err(|_| TripRepoError::CorruptData)?;
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    let rows = sqlx::query(
        "SELECT t.id, t.name, t.cover_photo_url, t.accent_color, \
                t.stop_kind_labels_json, t.status, t.start_date, t.end_date, \
                t.base_currency, t.soft_budget_json, t.current_plan_id, \
                t.current_plan_version, t.created_at, t.revision \
         FROM trip_memberships AS navigation \
         JOIN trips AS t ON t.id = navigation.trip_id \
         WHERE navigation.user_id = ? \
         ORDER BY t.start_date, t.id \
         LIMIT ?",
    )
    .bind(&actor.0)
    .bind(COLLECTION_QUERY_LIMIT)
    .fetch_all(&mut *transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() > MAX_COLLECTION_ITEMS {
        return Err(TripRepoError::CorruptData);
    }

    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let stored = super::records::decode_trip_row(&row)?;
        // The navigation index/join is never the final authorization check.
        authorize(
            &mut transaction,
            &stored.trip.id,
            actor,
            RequiredRole::AnyMember,
        )
        .await?;
        let profiles =
            load_members_and_validate_capacity(&mut transaction, &stored.trip.id).await?;
        result.push(summary(&stored.trip, profiles.len())?);
    }
    ensure_encoded_size(&result, MAX_COLLECTION_RESPONSE_BYTES)
        .map_err(|_| TripRepoError::CorruptData)?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn get_trip(
    db: &SqliteDb,
    trip_id: &str,
    actor: &UserId,
) -> Result<Trip, TripRepoError> {
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize(&mut transaction, trip_id, actor, RequiredRole::AnyMember).await?;
    let mut stored = load_trip(&mut transaction, trip_id).await?;
    let profiles = load_members_and_validate_capacity(&mut transaction, trip_id).await?;
    stored.trip.members = member_values(&profiles);
    super::records::ensure_unique_member_ids(&stored.trip.members)?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(stored.trip)
}

fn unavailable(_error: sqlx::Error) -> TripRepoError {
    TripRepoError::Unavailable
}
