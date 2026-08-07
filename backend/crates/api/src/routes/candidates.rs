use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::StatusCode,
};
use itinera_core::{
    domain::trip::{
        CandidateDisposition, CandidatePlaceInput, CandidateWithPlace, Place, PlaceActivityIdea,
        PlaceGuide, PlaceKind,
    },
    services::candidates::{self, AddCandidateInput, UpdateCandidateInput},
};
use serde::{Deserialize, Deserializer};

use crate::{auth::AuthenticatedPrincipal, error::ApiError, state::AppState};

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    q: String,
}

#[derive(Debug, Default)]
enum RequiredNullable<T> {
    #[default]
    Missing,
    Present(Option<T>),
}

impl<'de, T> Deserialize<'de> for RequiredNullable<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Self::Present)
    }
}

impl<T> RequiredNullable<T> {
    fn into_required(self, field: &'static str) -> Result<Option<T>, ApiError> {
        match self {
            Self::Missing => Err(ApiError::bad_request(format!("{field} is required"))),
            Self::Present(value) => Ok(value),
        }
    }
}

#[derive(Debug, Default)]
enum OptionalValue<T> {
    #[default]
    Absent,
    Present(T),
}

impl<'de, T> Deserialize<'de> for OptionalValue<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Present)
    }
}

impl<T> OptionalValue<T> {
    fn into_option(self) -> Option<T> {
        match self {
            Self::Absent => None,
            Self::Present(value) => Some(value),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaceActivityIdeaRequest {
    title: String,
    #[serde(default)]
    details: OptionalValue<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaceGuideRequest {
    summary: String,
    intro: String,
    activity_ideas: Vec<PlaceActivityIdeaRequest>,
    practical_tips: Vec<String>,
}

impl From<PlaceGuideRequest> for PlaceGuide {
    fn from(value: PlaceGuideRequest) -> Self {
        Self {
            summary: value.summary,
            intro: value.intro,
            activity_ideas: value
                .activity_ideas
                .into_iter()
                .map(|idea| PlaceActivityIdea {
                    title: idea.title,
                    details: idea.details.into_option(),
                })
                .collect(),
            practical_tips: value.practical_tips,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidatePlaceRequest {
    name: String,
    kind: PlaceKind,
    city: String,
    address: String,
    #[serde(default)]
    website: RequiredNullable<String>,
    #[serde(default)]
    phone: RequiredNullable<String>,
    opening_hours: Vec<String>,
    photo_urls: Vec<String>,
    #[serde(default)]
    guide: RequiredNullable<PlaceGuideRequest>,
}

impl TryFrom<CandidatePlaceRequest> for CandidatePlaceInput {
    type Error = ApiError;

    fn try_from(value: CandidatePlaceRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            kind: value.kind,
            city: value.city,
            address: value.address,
            website: value.website.into_required("place.website")?,
            phone: value.phone.into_required("place.phone")?,
            opening_hours: value.opening_hours,
            photo_urls: value.photo_urls,
            guide: value.guide.into_required("place.guide")?.map(Into::into),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddCandidateRequest {
    #[serde(default)]
    source_place_id: RequiredNullable<String>,
    place: CandidatePlaceRequest,
    pitch: String,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateCandidateRequest {
    place: CandidatePlaceRequest,
    pitch: String,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateStatusRequest {
    status: CandidateDisposition,
}

pub async fn search_places(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
    query: Result<Query<SearchQuery>, QueryRejection>,
) -> Result<Json<Vec<Place>>, ApiError> {
    let actor = principal.require_trip_read(&trip_id)?;
    let Query(query) = query?;
    Ok(Json(
        candidates::search_places(
            &*state.trips,
            &*state.place_catalog,
            &trip_id,
            &actor.id,
            query.q,
        )
        .await?,
    ))
}

pub async fn list_candidates(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
) -> Result<Json<Vec<CandidateWithPlace>>, ApiError> {
    let actor = principal.require_trip_read(&trip_id)?;
    Ok(Json(
        candidates::list_candidates(&*state.trips, &trip_id, &actor.id).await?,
    ))
}

pub async fn add_candidate(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
    payload: Result<Json<AddCandidateRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<CandidateWithPlace>), ApiError> {
    let actor = principal.require_human()?;
    let Json(request) = payload?;
    let source_place_id = request.source_place_id.into_required("sourcePlaceId")?;
    let candidate = candidates::add_candidate(
        &*state.trips,
        &*state.place_catalog,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &actor.id,
        AddCandidateInput {
            source_place_id,
            place: request.place.try_into()?,
            pitch: request.pitch,
            tags: request.tags,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(candidate)))
}

pub async fn update_candidate(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, candidate_id)): Path<(String, String)>,
    payload: Result<Json<UpdateCandidateRequest>, JsonRejection>,
) -> Result<Json<CandidateWithPlace>, ApiError> {
    let actor = principal.require_human()?;
    let Json(request) = payload?;
    Ok(Json(
        candidates::update_candidate(
            &*state.trips,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &actor.id,
            &candidate_id,
            UpdateCandidateInput {
                place: request.place.try_into()?,
                pitch: request.pitch,
                tags: request.tags,
            },
        )
        .await?,
    ))
}

pub async fn set_candidate_status(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, candidate_id)): Path<(String, String)>,
    payload: Result<Json<CandidateStatusRequest>, JsonRejection>,
) -> Result<Json<CandidateWithPlace>, ApiError> {
    let actor = principal.require_human()?;
    let Json(request) = payload?;
    Ok(Json(
        candidates::set_candidate_status(
            &*state.trips,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &actor.id,
            &candidate_id,
            request.status,
        )
        .await?,
    ))
}
