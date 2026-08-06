use std::collections::HashSet;

use crate::{
    domain::{
        trip::{
            Candidate, CandidateDisposition, CandidatePlaceInput, CandidateStatus,
            CandidateWithPlace, OpeningHours, Place, PlaceActivityIdea, PlaceGuide,
        },
        user::UserId,
    },
    ports::{
        clock::Clock,
        id_gen::IdGen,
        place_catalog::{PlaceCatalog, PlaceCatalogError},
        trip::{CandidateUpdate, TripRepo, TripRepoError},
    },
};

use super::validation::{ValidationError, bounded_strings, http_url, optional_text, required_text};

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
    actor: &UserId,
    query: String,
) -> Result<Vec<Place>, CandidateServiceError> {
    let query = required_text(
        query,
        "search query is required and must be at most 120 characters",
        120,
    )?;
    let saved = repo.search_saved_places(trip_id, actor, &query).await?;
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
    actor: &UserId,
) -> Result<Vec<CandidateWithPlace>, CandidateServiceError> {
    repo.list_candidates(trip_id, actor)
        .await
        .map_err(Into::into)
}

pub async fn add_candidate(
    repo: &dyn TripRepo,
    catalog: &dyn PlaceCatalog,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    input: AddCandidateInput,
) -> Result<CandidateWithPlace, CandidateServiceError> {
    let place_input = normalise_place(input.place)?;
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
        match repo.find_place(trip_id, actor, source_id).await? {
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
    repo.add_candidate(trip_id, actor, candidate, place)
        .await
        .map_err(Into::into)
}

pub async fn update_candidate(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    candidate_id: &str,
    input: UpdateCandidateInput,
) -> Result<CandidateWithPlace, CandidateServiceError> {
    let place_input = normalise_place(input.place)?;
    let pitch = required_text(
        input.pitch,
        "pitch is required and must be at most 2,000 characters",
        2_000,
    )?;
    let tags = bounded_strings(input.tags, 20, 60)?;
    let current = repo
        .list_candidates(trip_id, actor)
        .await?
        .into_iter()
        .find(|candidate| candidate.candidate.id == candidate_id)
        .ok_or(TripRepoError::NotFound)?;
    let place = materialise_place(ids.new_id(), place_input, Some(&current.place));

    repo.update_candidate(
        trip_id,
        actor,
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
    actor: &UserId,
    candidate_id: &str,
    status: CandidateDisposition,
) -> Result<CandidateWithPlace, CandidateServiceError> {
    repo.set_candidate_status(
        trip_id,
        actor,
        candidate_id,
        status,
        &clock.now(),
        &ids.new_id(),
    )
    .await
    .map_err(Into::into)
}

fn normalise_place(input: CandidatePlaceInput) -> Result<CandidatePlaceInput, ValidationError> {
    let guide = input.guide.map(normalise_guide).transpose()?;
    Ok(CandidatePlaceInput {
        name: required_text(
            input.name,
            "place name is required and must be at most 200 characters",
            200,
        )?,
        kind: input.kind,
        city: required_text(
            input.city,
            "city is required and must be at most 120 characters",
            120,
        )?,
        address: optional_text(Some(input.address), 500)?.unwrap_or_default(),
        website: http_url(input.website)?,
        phone: optional_text(input.phone, 80)?,
        opening_hours: bounded_strings(input.opening_hours, 14, 200)?,
        photo_urls: bounded_strings(input.photo_urls, 20, 2_048)?,
        guide,
    })
}

fn normalise_guide(guide: PlaceGuide) -> Result<PlaceGuide, ValidationError> {
    if guide.activity_ideas.len() > 20 {
        return Err(ValidationError(
            "a guide may contain at most 20 activity ideas",
        ));
    }
    let activity_ideas = guide
        .activity_ideas
        .into_iter()
        .map(|idea| {
            Ok(PlaceActivityIdea {
                title: required_text(idea.title, "activity title is required", 160)?,
                details: optional_text(idea.details, 1_000)?,
            })
        })
        .collect::<Result<_, ValidationError>>()?;
    Ok(PlaceGuide {
        summary: required_text(guide.summary, "guide summary is required", 500)?,
        intro: required_text(guide.intro, "guide introduction is required", 4_000)?,
        activity_ideas,
        practical_tips: bounded_strings(guide.practical_tips, 30, 500)?,
    })
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
    use crate::domain::trip::PlaceKind;

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
        assert!(normalise_place(input).is_err());
    }
}
