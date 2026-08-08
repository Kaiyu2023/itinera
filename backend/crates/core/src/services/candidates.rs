use std::collections::HashSet;

use chrono::DateTime;

use crate::{
    domain::trip::{
        Candidate, CandidateDisposition, CandidatePlaceInput, CandidateStatus, CandidateWithPlace,
        OpeningHours, Place,
    },
    ports::{
        authorization::TripAuthorizationContext,
        clock::Clock,
        id_gen::IdGen,
        place_catalog::{PlaceCatalog, PlaceCatalogError},
        trip::{CandidateUpdate, TripRepo, TripRepoError},
    },
};

use super::validation::{
    ValidationError, bounded_strings, exact_bounded_strings, exact_required_text,
    normalise_candidate_place, required_text, validate_place_snapshot,
};

#[derive(Debug, Clone, PartialEq)]
pub struct AddCandidateInput {
    pub source_place_id: Option<String>,
    pub place: CandidatePlaceInput,
    pub pitch: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateCandidateInput {
    pub place: CandidatePlaceInput,
    pub pitch: String,
    pub tags: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CandidateServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] TripRepoError),
    #[error(transparent)]
    Catalog(#[from] PlaceCatalogError),
}

pub async fn search_places(
    repo: &dyn TripRepo,
    catalog: &dyn PlaceCatalog,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    query: String,
) -> Result<Vec<Place>, CandidateServiceError> {
    let query = required_text(
        query,
        "search query is required and must be at most 120 characters",
        120,
    )?;
    let saved = repo
        .search_saved_places(trip_id, authorization, &query)
        .await?;
    let public = catalog.search(&query).await?;
    let mut seen = HashSet::new();
    Ok(saved
        .into_iter()
        .chain(public)
        .filter(|place| seen.insert(place.id.clone()))
        .collect())
}

pub async fn list_candidates(
    repo: &dyn TripRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<CandidateWithPlace>, CandidateServiceError> {
    repo.list_candidates(trip_id, authorization)
        .await
        .map_err(Into::into)
}

pub async fn add_candidate(
    repo: &dyn TripRepo,
    catalog: &dyn PlaceCatalog,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    input: AddCandidateInput,
) -> Result<CandidateWithPlace, CandidateServiceError> {
    let actor = require_human(authorization)?;
    let place_input = normalise_candidate_place(input.place)?;
    let pitch = required_text(
        input.pitch,
        "pitch is required and must be at most 2,000 characters",
        2_000,
    )?;
    let tags = bounded_strings(input.tags, 20, 60)?;
    let source_place_id = input
        .source_place_id
        .map(|value| required_text(value, "sourcePlaceId must not be empty", 200))
        .transpose()?;

    let source = if let Some(source_id) = source_place_id.as_deref() {
        match repo.find_place(trip_id, authorization, source_id).await? {
            Some(place) => Some(place),
            None => catalog
                .find(source_id)
                .await?
                .ok_or(PlaceCatalogError::NotFound)
                .map(Some)?,
        }
    } else {
        // A city name is not provenance. Until geocoding arrives in step 4,
        // manual ideas deliberately carry neutral provider facts instead of
        // borrowing coordinates or provider identity from an unrelated place.
        None
    };

    let place = materialise_place(ids.new_id(), place_input, source.as_ref());
    validate_place_snapshot(&place)?;
    let candidate = Candidate {
        id: ids.new_id(),
        trip_id: trip_id.to_string(),
        source_place_id,
        place_id: place.id.clone(),
        proposed_by: actor.0.clone(),
        created_at: clock.now(),
        pitch,
        tags,
        status: CandidateStatus::Shortlisted,
    };
    repo.add_candidate(trip_id, authorization, candidate, place)
        .await
        .map_err(Into::into)
}

pub async fn update_candidate(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    candidate_id: &str,
    input: UpdateCandidateInput,
) -> Result<CandidateWithPlace, CandidateServiceError> {
    require_human(authorization)?;
    let place_input = normalise_candidate_place(input.place)?;
    let pitch = required_text(
        input.pitch,
        "pitch is required and must be at most 2,000 characters",
        2_000,
    )?;
    let tags = bounded_strings(input.tags, 20, 60)?;
    let current = repo
        .list_candidates(trip_id, authorization)
        .await?
        .into_iter()
        .find(|candidate| candidate.candidate.id == candidate_id)
        .ok_or(TripRepoError::NotFound)?;
    let place = materialise_place(ids.new_id(), place_input, Some(&current.place));
    validate_place_snapshot(&place)?;

    repo.update_candidate(
        trip_id,
        authorization,
        candidate_id,
        CandidateUpdate {
            place,
            pitch,
            tags,
            changed_at: clock.now(),
            change_id: ids.new_id(),
        },
    )
    .await
    .map_err(Into::into)
}

pub async fn set_candidate_status(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    candidate_id: &str,
    status: CandidateDisposition,
) -> Result<CandidateWithPlace, CandidateServiceError> {
    require_human(authorization)?;
    repo.set_candidate_status(
        trip_id,
        authorization,
        candidate_id,
        status,
        &clock.now(),
        &ids.new_id(),
    )
    .await
    .map_err(Into::into)
}

fn require_human(
    authorization: &TripAuthorizationContext,
) -> Result<&crate::domain::user::UserId, CandidateServiceError> {
    authorization
        .human_user_id()
        .ok_or_else(|| TripRepoError::Forbidden.into())
}

/// Validates persisted candidate data before another capability relies on it
/// or writes a new revision. Candidate snapshots are server-owned records, so
/// stored values must already be in their canonical form.
pub fn validate_stored_candidate(
    expected_trip_id: &str,
    candidate: &Candidate,
) -> Result<(), ValidationError> {
    exact_required_text(&candidate.id, "candidate id is invalid", 200)?;
    exact_required_text(&candidate.trip_id, "candidate trip id is invalid", 200)?;
    if candidate.trip_id != expected_trip_id {
        return Err(ValidationError("candidate belongs to another trip"));
    }
    if let Some(source_place_id) = candidate.source_place_id.as_deref() {
        exact_required_text(source_place_id, "source place id is invalid", 200)?;
    }
    exact_required_text(&candidate.place_id, "candidate place id is invalid", 200)?;
    exact_required_text(&candidate.proposed_by, "candidate proposer is invalid", 200)?;
    let timestamp = DateTime::parse_from_rfc3339(&candidate.created_at)
        .map_err(|_| ValidationError("candidate timestamp is invalid"))?;
    if candidate.created_at.len() > 64
        || !candidate.created_at.ends_with('Z')
        || timestamp.offset().local_minus_utc() != 0
    {
        return Err(ValidationError("candidate timestamp is invalid"));
    }
    exact_required_text(&candidate.pitch, "candidate pitch is invalid", 2_000)?;
    exact_bounded_strings(&candidate.tags, 20, 60)?;
    Ok(())
}

fn materialise_place(id: String, input: CandidatePlaceInput, source: Option<&Place>) -> Place {
    Place {
        id,
        name: input.name,
        kind: input.kind,
        lat: source.map_or(0.0, |place| place.lat),
        lng: source.map_or(0.0, |place| place.lng),
        tz: source.map_or_else(|| "UTC".to_string(), |place| place.tz.clone()),
        country_code: source.map_or_else(String::new, |place| place.country_code.clone()),
        admin_area: source.map_or_else(String::new, |place| place.admin_area.clone()),
        city: input.city,
        address: input.address,
        external_ref: source.and_then(|place| place.external_ref.clone()),
        website: input.website,
        phone: input.phone,
        rating: source.and_then(|place| place.rating),
        price_level: source.and_then(|place| place.price_level),
        opening_hours: (!input.opening_hours.is_empty()).then_some(OpeningHours {
            weekday_text: input.opening_hours,
        }),
        photo_urls: input.photo_urls,
        guide: input.guide,
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::trip::{CandidateStatus, PlaceKind};

    use super::*;

    #[test]
    fn candidate_links_reject_active_content_schemes() {
        let input = CandidatePlaceInput {
            name: "Temple".into(),
            kind: PlaceKind::Sight,
            city: "Kyoto".into(),
            address: String::new(),
            website: Some("javascript:alert(1)".into()),
            phone: None,
            opening_hours: vec![],
            photo_urls: vec![],
            guide: None,
        };
        assert!(normalise_candidate_place(input).is_err());
    }

    #[test]
    fn stored_candidates_must_be_canonical_and_trip_scoped() {
        let candidate = Candidate {
            id: "candidate-a".into(),
            trip_id: "trip-a".into(),
            source_place_id: Some("catalog-place".into()),
            place_id: "candidate-place".into(),
            proposed_by: "user-a".into(),
            created_at: "2026-08-06T12:00:00Z".into(),
            pitch: "Worth the detour".into(),
            tags: vec!["quiet".into()],
            status: CandidateStatus::Shortlisted,
        };

        assert!(validate_stored_candidate("trip-a", &candidate).is_ok());
        assert!(validate_stored_candidate("trip-b", &candidate).is_err());

        let mut malformed = candidate.clone();
        malformed.created_at = "2026-08-06T13:00:00+01:00".into();
        assert!(validate_stored_candidate("trip-a", &malformed).is_err());

        malformed = candidate.clone();
        malformed.tags = vec![" not-normalized".into()];
        assert!(validate_stored_candidate("trip-a", &malformed).is_err());

        malformed = candidate;
        malformed.pitch = " ".into();
        assert!(validate_stored_candidate("trip-a", &malformed).is_err());
    }
}
