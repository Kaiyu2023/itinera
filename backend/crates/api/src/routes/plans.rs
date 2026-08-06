use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
};
use itinera_core::{
    domain::trip::{Booking, Day, DayPatch, Money, Plan, PlanDetail, Stop, StopPatch},
    services::plans,
};
use serde::{Deserialize, Deserializer};

use crate::{auth::AuthenticatedUser, error::ApiError, state::AppState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializePlanRequest {
    anchor_place_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DayPatchRequest {
    #[serde(default)]
    window_start: OptionalPatchValue<String>,
    #[serde(default)]
    window_end: OptionalPatchValue<String>,
    #[serde(default)]
    city_hint: OptionalPatchValue<String>,
}

#[derive(Debug, Default)]
enum OptionalPatchValue<T> {
    #[default]
    Absent,
    Present(T),
}

impl<'de, T> Deserialize<'de> for OptionalPatchValue<T>
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

impl<T> OptionalPatchValue<T> {
    fn into_patch(self) -> Option<T> {
        match self {
            Self::Absent => None,
            Self::Present(value) => Some(value),
        }
    }
}

#[derive(Debug, Default)]
enum NullablePatchValue<T> {
    #[default]
    Absent,
    Present(Option<T>),
}

impl<'de, T> Deserialize<'de> for NullablePatchValue<T>
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

impl<T> NullablePatchValue<T> {
    fn into_patch(self) -> Option<Option<T>> {
        match self {
            Self::Absent => None,
            Self::Present(value) => Some(value),
        }
    }
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BookingRequest {
    #[serde(rename = "ref")]
    reference: String,
    #[serde(default)]
    url: RequiredNullable<String>,
    #[serde(default)]
    cost: RequiredNullable<Money>,
    #[serde(default)]
    ledger_entry_id: RequiredNullable<String>,
}

impl TryFrom<BookingRequest> for Booking {
    type Error = ApiError;

    fn try_from(value: BookingRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            reference: value.reference,
            url: value.url.into_required("booking.url")?,
            cost: value.cost.into_required("booking.cost")?,
            ledger_entry_id: value
                .ledger_entry_id
                .into_required("booking.ledgerEntryId")?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StopPatchRequest {
    #[serde(default)]
    planned_arrival: OptionalPatchValue<String>,
    #[serde(default)]
    duration_min: OptionalPatchValue<u32>,
    #[serde(default)]
    notes: OptionalPatchValue<String>,
    #[serde(default)]
    booking: NullablePatchValue<BookingRequest>,
}

pub async fn get_current_plan(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
) -> Result<Json<PlanDetail>, ApiError> {
    Ok(Json(
        plans::get_current_plan(&*state.trips, &trip_id, &actor.id).await?,
    ))
}

pub async fn initialize_plan(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
    payload: Result<Json<InitializePlanRequest>, JsonRejection>,
) -> Result<Json<PlanDetail>, ApiError> {
    let Json(request) = payload?;
    Ok(Json(
        plans::initialize_plan(
            &*state.trips,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &actor.id,
            &request.anchor_place_id,
        )
        .await?,
    ))
}

pub async fn list_plan_versions(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
) -> Result<Json<Vec<Plan>>, ApiError> {
    Ok(Json(
        plans::list_plan_versions(&*state.trips, &trip_id, &actor.id).await?,
    ))
}

pub async fn update_day(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path((trip_id, day_id)): Path<(String, String)>,
    payload: Result<Json<DayPatchRequest>, JsonRejection>,
) -> Result<Json<Day>, ApiError> {
    let Json(request) = payload?;
    Ok(Json(
        plans::update_day(
            &*state.trips,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &actor.id,
            &day_id,
            DayPatch {
                window_start: request.window_start.into_patch(),
                window_end: request.window_end.into_patch(),
                city_hint: request.city_hint.into_patch(),
            },
        )
        .await?,
    ))
}

pub async fn update_stop(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path((trip_id, stop_id)): Path<(String, String)>,
    payload: Result<Json<StopPatchRequest>, JsonRejection>,
) -> Result<Json<Stop>, ApiError> {
    let Json(request) = payload?;
    let booking = match request.booking.into_patch() {
        None => None,
        Some(None) => Some(None),
        Some(Some(booking)) => Some(Some(booking.try_into()?)),
    };
    Ok(Json(
        plans::update_stop(
            &*state.trips,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &actor.id,
            &stop_id,
            StopPatch {
                planned_arrival: request.planned_arrival.into_patch(),
                duration_min: request.duration_min.into_patch(),
                notes: request.notes.into_patch(),
                booking,
            },
        )
        .await?,
    ))
}
