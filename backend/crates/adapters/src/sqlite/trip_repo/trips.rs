//! Trip creation and bounded private reads.

use std::collections::HashSet;

use super::{
    access::{
        RequiredRole, authorize, load_members_and_validate_capacity, load_profile_by_id, load_trip,
        member_values, require_human_authorization, user_distinct_trip_ids,
        validate_trip_plan_pointer,
    },
    plans::load_plan_detail,
    records::{
        COLLECTION_QUERY_LIMIT, MAX_COLLECTION_ITEMS, MAX_COLLECTION_RESPONSE_BYTES,
        TripNavigationRow, encode_soft_budget, encode_stop_kind_labels, summary,
    },
};
use crate::sqlite::{
    SqliteDb,
    codec::{email_digest, ensure_encoded_size, validate_id},
    row::SqliteRowExt,
};
use itinera_core::{
    domain::{
        trip::{Trip, TripRole, TripSummary},
        user::UserId,
    },
    ports::{authorization::TripAuthorizationContext, trip::TripRepoError},
};

pub(super) async fn create_trip(
    db: &SqliteDb,
    authorization: &TripAuthorizationContext,
    trip: Trip,
) -> Result<Trip, TripRepoError> {
    // Trip creation never imports an existing plan graph. Plan v1 must be
    // initialized through its own writer so the plan, days, and exact pointer
    // are committed together.
    if trip.current_plan_id().is_some() {
        return Err(TripRepoError::Unavailable);
    }
    let labels = encode_stop_kind_labels(trip.stop_kind_labels())?;
    let budget = encode_soft_budget(trip.soft_budget())?;
    let mut transaction = db
        .begin_immediate()
        .await
        .map_err(|_| TripRepoError::Unavailable)?;
    let actor = require_human_authorization(authorization)?;
    validate_id(&actor.0).map_err(|_| TripRepoError::CorruptData)?;
    if !trip
        .members()
        .iter()
        .any(|member| member.user_id() == actor.0 && member.role() == TripRole::Leader)
    {
        return Err(TripRepoError::Forbidden);
    }

    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trips WHERE id = ?")
        .bind(trip.id())
        .fetch_one(&mut *transaction)
        .await
        .map_err(unavailable)?;
    if existing != 0 {
        return Err(TripRepoError::Conflict);
    }

    for membership in trip.members() {
        let user_id = UserId(membership.user_id().to_string());
        let profile = load_profile_by_id(&mut transaction, &user_id)
            .await?
            .ok_or(TripRepoError::CorruptData)?;
        let digest = email_digest(&profile.email);
        let (mut trip_ids, _) =
            user_distinct_trip_ids(&mut transaction, &profile.email, &digest).await?;
        trip_ids.insert(trip.id().to_string());
        if trip_ids.len() > MAX_COLLECTION_ITEMS {
            return Err(TripRepoError::Conflict);
        }
    }

    sqlx::query(
        "INSERT INTO trips (\
            id, name, cover_photo_url, accent_color, stop_kind_labels_json, status, \
            start_date, end_date, base_currency, soft_budget_json, current_plan_id, \
            current_plan_version, created_at, revision\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, 1)",
    )
    .bind(trip.id())
    .bind(trip.name())
    .bind(trip.cover_photo_url())
    .bind(trip.accent_color())
    .bind(labels)
    .bind(trip.status().as_ref())
    .bind(trip.start_date())
    .bind(trip.end_date())
    .bind(trip.base_currency().as_str())
    .bind(budget)
    .bind(trip.created_at())
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    for membership in trip.members() {
        sqlx::query(
            "INSERT INTO trip_memberships \
             (trip_id, user_id, role, joined_at, revision) VALUES (?, ?, ?, ?, 1)",
        )
        .bind(trip.id())
        .bind(membership.user_id())
        .bind(membership.role().as_ref())
        .bind(membership.joined_at())
        .execute(&mut *transaction)
        .await
        .map_err(unavailable)?;
    }

    db.commit(transaction).await.map_err(unavailable)?;
    Ok(trip)
}

pub(super) async fn list_trips(
    db: &SqliteDb,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<TripSummary>, TripRepoError> {
    let actor = require_human_authorization(authorization)?;
    validate_id(&actor.0).map_err(|_| TripRepoError::CorruptData)?;
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    let rows = sqlx::query(
        "SELECT navigation.trip_id AS navigation_trip_id, \
                t.id, t.name, t.cover_photo_url, t.accent_color, \
                t.stop_kind_labels_json, t.status, t.start_date, t.end_date, \
                t.base_currency, t.soft_budget_json, t.current_plan_id, \
                t.current_plan_version, t.created_at, t.revision \
         FROM trip_memberships AS navigation \
         LEFT JOIN trips AS t ON t.id = navigation.trip_id \
         WHERE navigation.user_id = ? \
         ORDER BY t.start_date, navigation.trip_id \
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
        let navigation_row: TripNavigationRow = row.decode()?;
        let trip_row = navigation_row.into_trip_row()?;
        let trip_id = trip_row.id().to_string();
        // The navigation index/join is never the final authorization check.
        authorize(
            &mut transaction,
            &trip_id,
            authorization,
            RequiredRole::AnyMember,
        )
        .await?;
        validate_trip_plan_pointer(&mut transaction, &trip_row).await?;
        let current_plan = trip_row
            .current_plan_pointer()?
            .map(|(id, version)| (id.to_string(), version));
        let profiles = load_members_and_validate_capacity(&mut transaction, &trip_id).await?;
        let trip = trip_row.into_trip(member_values(&profiles))?.value;
        let mut trip_summary = summary(&trip, profiles.len())?;
        if let Some((plan_id, version)) = current_plan {
            let plan = load_plan_detail(&mut transaction, &trip_id, &plan_id, version).await?;
            let mut seen = HashSet::new();
            trip_summary.cities = plan
                .days
                .into_iter()
                .map(|day| day.city_hint)
                .filter(|city| seen.insert(city.clone()))
                .collect();
        }
        result.push(trip_summary);
    }
    ensure_encoded_size(&result, MAX_COLLECTION_RESPONSE_BYTES)
        .map_err(|_| TripRepoError::CorruptData)?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn get_trip(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Trip, TripRepoError> {
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
    let trip = trip_row.into_trip(member_values(&profiles))?.value;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(trip)
}

fn unavailable(_error: sqlx::Error) -> TripRepoError {
    TripRepoError::Unavailable
}
