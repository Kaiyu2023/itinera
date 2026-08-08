//! Candidate creation and bounded candidate/place reads.

use std::collections::HashMap;

use itinera_core::{
    domain::trip::{Candidate, CandidateStatus, CandidateWithPlace, Place},
    ports::{authorization::TripAuthorizationContext, trip::TripRepoError},
    services::candidates::{validate_candidate_place_provenance, validate_stored_candidate},
};
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{SqliteDb, codec::validate_id};

use super::{
    access::{
        RequiredRole, authorize, load_members_and_validate_capacity, load_trip, member_values,
    },
    candidate_records::{
        CANDIDATE_QUERY_LIMIT, CandidatePlaceRow, EncodedPlace, MAX_CANDIDATE_ITEMS,
        MAX_PLACE_SEARCH_ITEMS, MAX_RESPONSE_BYTES, PLACE_SEARCH_QUERY_LIMIT, PlaceRow,
        encode_candidate_status, encode_place, encode_place_kind, encode_tags, encoded_size,
    },
};

pub(super) async fn search_saved_places(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    query: &str,
) -> Result<Vec<Place>, TripRepoError> {
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::AnyMember,
    )
    .await?;
    validate_trip(&mut transaction, trip_id).await?;
    let rows = sqlx::query_as::<_, PlaceRow>(
        "SELECT p.trip_id AS place_trip_id, p.id AS place_id, \
                p.name AS place_name, p.kind AS place_kind, \
                p.lat AS place_lat, p.lng AS place_lng, p.tz AS place_tz, \
                p.country_code AS place_country_code, \
                p.admin_area AS place_admin_area, p.city AS place_city, \
                p.address AS place_address, \
                p.external_ref_json AS place_external_ref_json, \
                p.website AS place_website, p.phone AS place_phone, \
                p.rating AS place_rating, p.price_level AS place_price_level, \
                p.opening_hours_json AS place_opening_hours_json, \
                p.photo_urls_json AS place_photo_urls_json, \
                p.guide_json AS place_guide_json, p.revision AS place_revision \
         FROM trip_places AS p \
         WHERE p.trip_id = ? \
           AND EXISTS ( \
               SELECT 1 \
               FROM trips AS t \
               JOIN plans AS current_plan \
                 ON current_plan.trip_id = t.id \
                AND current_plan.id = t.current_plan_id \
                AND current_plan.version = t.current_plan_version \
                AND current_plan.version = 1 \
                AND current_plan.created_from_proposal_id IS NULL \
               JOIN plan_stops AS s \
                 ON s.trip_id = current_plan.trip_id \
                AND s.plan_version = current_plan.version \
               WHERE t.id = p.trip_id AND s.place_id = p.id \
           ) \
           AND instr(lower(p.name || ' ' || p.city || ' ' || p.address), lower(?)) > 0 \
         ORDER BY lower(p.name), p.id \
         LIMIT ?",
    )
    .bind(trip_id)
    .bind(query)
    .bind(PLACE_SEARCH_QUERY_LIMIT)
    .fetch_all(&mut *transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() > MAX_PLACE_SEARCH_ITEMS {
        return Err(TripRepoError::CorruptData);
    }
    let places = rows
        .into_iter()
        .map(|row| row.into_place(trip_id))
        .collect::<Result<Vec<_>, _>>()?;
    if encoded_size(&places)? > MAX_RESPONSE_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(places)
}

pub(super) async fn find_place(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    place_id: &str,
) -> Result<Option<Place>, TripRepoError> {
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::AnyMember,
    )
    .await?;
    validate_trip(&mut transaction, trip_id).await?;
    let place = load_place(&mut transaction, trip_id, place_id).await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(place)
}

pub(super) async fn list_candidates(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<CandidateWithPlace>, TripRepoError> {
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::AnyMember,
    )
    .await?;
    validate_trip(&mut transaction, trip_id).await?;
    let candidates = load_candidates(&mut transaction, trip_id).await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(candidates)
}

pub(super) async fn add_candidate(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    candidate: Candidate,
    place: Place,
) -> Result<CandidateWithPlace, TripRepoError> {
    validate_id(trip_id).map_err(corrupt)?;
    validate_stored_candidate(trip_id, &candidate).map_err(corrupt)?;
    if candidate.status != CandidateStatus::Shortlisted || candidate.place_id != place.id {
        return Err(TripRepoError::CorruptData);
    }
    validate_candidate_place_provenance(&candidate, &place, None).map_err(corrupt)?;
    let encoded_place = encode_place(&place)?;
    let tags_json = encode_tags(&candidate.tags)?;

    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await?;
    let actor = authorization
        .human_user_id()
        .ok_or(TripRepoError::Forbidden)?;
    if candidate.proposed_by != actor.0 {
        return Err(TripRepoError::CorruptData);
    }
    validate_trip(&mut transaction, trip_id).await?;

    let (source_catalog_place_id, source_trip_place_id) =
        if let Some(source_id) = candidate.source_place_id.as_deref() {
            if source_id == place.id {
                return Err(TripRepoError::CorruptData);
            }
            match load_place(&mut transaction, trip_id, source_id).await? {
                Some(source) => {
                    validate_candidate_place_provenance(&candidate, &place, Some(&source))
                        .map_err(corrupt)?;
                    (None, Some(source_id))
                }
                None => (Some(source_id), None),
            }
        } else {
            (None, None)
        };

    let collision: i64 = sqlx::query_scalar(
        "SELECT \
             EXISTS(SELECT 1 FROM candidates WHERE trip_id = ? AND id = ?) \
             OR EXISTS(SELECT 1 FROM trip_places WHERE trip_id = ? AND id = ?)",
    )
    .bind(trip_id)
    .bind(&candidate.id)
    .bind(trip_id)
    .bind(&place.id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(unavailable)?;
    if collision != 0 {
        return Err(TripRepoError::Conflict);
    }

    let mut projected = load_candidates(&mut transaction, trip_id).await?;
    if projected.len() >= MAX_CANDIDATE_ITEMS {
        return Err(TripRepoError::Conflict);
    }
    let result = CandidateWithPlace {
        candidate: candidate.clone(),
        place: place.clone(),
    };
    projected.push(result.clone());
    if encoded_size(&projected)? > MAX_RESPONSE_BYTES {
        return Err(TripRepoError::Conflict);
    }

    insert_place(&mut transaction, trip_id, &place, encoded_place).await?;
    sqlx::query(
        "INSERT INTO candidates ( \
             trip_id, id, place_id, source_catalog_place_id, \
             source_trip_place_id, proposed_by, created_at, pitch, tags_json, \
             status, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(&candidate.id)
    .bind(&candidate.place_id)
    .bind(source_catalog_place_id)
    .bind(source_trip_place_id)
    .bind(&candidate.proposed_by)
    .bind(&candidate.created_at)
    .bind(&candidate.pitch)
    .bind(tags_json)
    .bind(encode_candidate_status(candidate.status))
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn load_place(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    place_id: &str,
) -> Result<Option<Place>, TripRepoError> {
    let row = sqlx::query_as::<_, PlaceRow>(
        "SELECT p.trip_id AS place_trip_id, p.id AS place_id, \
                p.name AS place_name, p.kind AS place_kind, \
                p.lat AS place_lat, p.lng AS place_lng, p.tz AS place_tz, \
                p.country_code AS place_country_code, \
                p.admin_area AS place_admin_area, p.city AS place_city, \
                p.address AS place_address, \
                p.external_ref_json AS place_external_ref_json, \
                p.website AS place_website, p.phone AS place_phone, \
                p.rating AS place_rating, p.price_level AS place_price_level, \
                p.opening_hours_json AS place_opening_hours_json, \
                p.photo_urls_json AS place_photo_urls_json, \
                p.guide_json AS place_guide_json, p.revision AS place_revision \
         FROM trip_places AS p WHERE p.trip_id = ? AND p.id = ?",
    )
    .bind(trip_id)
    .bind(place_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?;
    row.map(|row| row.into_place(trip_id)).transpose()
}

pub(super) async fn load_candidates(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<CandidateWithPlace>, TripRepoError> {
    let stored_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM candidates WHERE trip_id = ?")
        .bind(trip_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(unavailable)?;
    let stored_count = usize::try_from(stored_count).map_err(corrupt)?;
    if stored_count > MAX_CANDIDATE_ITEMS {
        return Err(TripRepoError::CorruptData);
    }
    let rows = sqlx::query_as::<_, CandidatePlaceRow>(
        "SELECT c.trip_id AS candidate_trip_id, c.id AS candidate_id, \
                c.place_id AS candidate_place_id, c.source_catalog_place_id, \
                c.source_trip_place_id, c.proposed_by, \
                c.created_at AS candidate_created_at, \
                c.pitch AS candidate_pitch, c.tags_json AS candidate_tags_json, \
                c.status AS candidate_status, c.revision AS candidate_revision, \
                p.trip_id AS place_trip_id, p.id AS place_id, \
                p.name AS place_name, p.kind AS place_kind, \
                p.lat AS place_lat, p.lng AS place_lng, p.tz AS place_tz, \
                p.country_code AS place_country_code, \
                p.admin_area AS place_admin_area, p.city AS place_city, \
                p.address AS place_address, \
                p.external_ref_json AS place_external_ref_json, \
                p.website AS place_website, p.phone AS place_phone, \
                p.rating AS place_rating, p.price_level AS place_price_level, \
                p.opening_hours_json AS place_opening_hours_json, \
                p.photo_urls_json AS place_photo_urls_json, \
                p.guide_json AS place_guide_json, p.revision AS place_revision \
         FROM candidates AS c \
         JOIN trip_places AS p \
           ON p.trip_id = c.trip_id AND p.id = c.place_id \
         WHERE c.trip_id = ? \
         ORDER BY c.created_at, c.id \
         LIMIT ?",
    )
    .bind(trip_id)
    .bind(CANDIDATE_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() != stored_count {
        return Err(TripRepoError::CorruptData);
    }
    let expected_source_count = rows
        .iter()
        .filter(|row| row.source_trip_place_id().is_some())
        .count();
    let source_rows = sqlx::query_as::<_, PlaceRow>(
        "SELECT p.trip_id AS place_trip_id, p.id AS place_id, \
                p.name AS place_name, p.kind AS place_kind, \
                p.lat AS place_lat, p.lng AS place_lng, p.tz AS place_tz, \
                p.country_code AS place_country_code, \
                p.admin_area AS place_admin_area, p.city AS place_city, \
                p.address AS place_address, \
                p.external_ref_json AS place_external_ref_json, \
                p.website AS place_website, p.phone AS place_phone, \
                p.rating AS place_rating, p.price_level AS place_price_level, \
                p.opening_hours_json AS place_opening_hours_json, \
                p.photo_urls_json AS place_photo_urls_json, \
                p.guide_json AS place_guide_json, p.revision AS place_revision \
         FROM candidates AS c \
         JOIN trip_places AS p \
           ON p.trip_id = c.trip_id AND p.id = c.source_trip_place_id \
         WHERE c.trip_id = ? AND c.source_trip_place_id IS NOT NULL \
         ORDER BY c.id LIMIT ?",
    )
    .bind(trip_id)
    .bind(CANDIDATE_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if source_rows.len() != expected_source_count {
        return Err(TripRepoError::CorruptData);
    }
    let mut sources = HashMap::new();
    for row in source_rows {
        let source = row.into_place(trip_id)?;
        if sources
            .insert(source.id.clone(), source.clone())
            .is_some_and(|previous| previous != source)
        {
            return Err(TripRepoError::CorruptData);
        }
    }
    let candidates = rows
        .into_iter()
        .map(|row| {
            let source_id = row.source_trip_place_id().map(str::to_string);
            let candidate = row.into_candidate(trip_id)?;
            // Migration 0002 can only create shortlisted candidates. Rejected
            // becomes authoritative with audited status mutations, and in_plan
            // only with structural proposal publication. Until those slices
            // exist, either stored value has unsupported provenance.
            if candidate.candidate.status != CandidateStatus::Shortlisted {
                return Err(TripRepoError::CorruptData);
            }
            if let Some(source_id) = source_id {
                let source = sources.get(&source_id).ok_or(TripRepoError::CorruptData)?;
                validate_candidate_place_provenance(
                    &candidate.candidate,
                    &candidate.place,
                    Some(source),
                )
                .map_err(corrupt)?;
            } else {
                validate_candidate_place_provenance(&candidate.candidate, &candidate.place, None)
                    .map_err(corrupt)?;
            }
            Ok(candidate)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if encoded_size(&candidates)? > MAX_RESPONSE_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    Ok(candidates)
}

pub(super) async fn insert_place(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    place: &Place,
    encoded: EncodedPlace,
) -> Result<(), TripRepoError> {
    sqlx::query(
        "INSERT INTO trip_places ( \
             trip_id, id, name, kind, lat, lng, tz, country_code, admin_area, \
             city, address, external_ref_json, website, phone, rating, \
             price_level, opening_hours_json, photo_urls_json, guide_json, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(&place.id)
    .bind(&place.name)
    .bind(encode_place_kind(place.kind))
    .bind(place.lat)
    .bind(place.lng)
    .bind(&place.tz)
    .bind(&place.country_code)
    .bind(&place.admin_area)
    .bind(&place.city)
    .bind(&place.address)
    .bind(encoded.external_ref_json)
    .bind(&place.website)
    .bind(&place.phone)
    .bind(place.rating)
    .bind(place.price_level.map(i64::from))
    .bind(encoded.opening_hours_json)
    .bind(encoded.photo_urls_json)
    .bind(encoded.guide_json)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    Ok(())
}

async fn validate_trip(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<(), TripRepoError> {
    let trip = load_trip(transaction, trip_id).await?;
    let profiles = load_members_and_validate_capacity(transaction, trip_id).await?;
    trip.into_trip(member_values(&profiles)).map(|_| ())
}

fn unavailable(_error: sqlx::Error) -> TripRepoError {
    TripRepoError::Unavailable
}

fn corrupt<T>(_error: T) -> TripRepoError {
    TripRepoError::CorruptData
}
