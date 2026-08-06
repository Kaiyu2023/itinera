use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TripRole {
    Leader,
    Member,
    Viewer,
}

impl TripRole {
    pub fn can_edit(self) -> bool {
        matches!(self, Self::Leader | Self::Member)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TripStatus {
    Dreaming,
    Planning,
    Booked,
    Ongoing,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TripMember {
    pub user_id: String,
    pub role: TripRole,
    pub joined_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoftBudget {
    pub amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trip {
    pub id: String,
    pub name: String,
    pub cover_photo_url: Option<String>,
    pub accent_color: Option<String>,
    pub stop_kind_labels: Option<HashMap<StopKind, String>>,
    pub status: TripStatus,
    pub start_date: String,
    pub end_date: String,
    pub base_currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft_budget: Option<SoftBudget>,
    pub members: Vec<TripMember>,
    pub current_plan_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TripSummary {
    pub id: String,
    pub name: String,
    pub cover_photo_url: Option<String>,
    pub accent_color: Option<String>,
    pub status: TripStatus,
    pub start_date: String,
    pub end_date: String,
    pub member_count: u32,
    pub cities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteStatus {
    Pending,
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub id: String,
    pub trip_id: String,
    pub email: String,
    pub invited_by: String,
    pub status: InviteStatus,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceKind {
    Sight,
    Food,
    Lodging,
    Activity,
    TransportHub,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceActivityIdea {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceGuide {
    pub summary: String,
    pub intro: String,
    pub activity_ideas: Vec<PlaceActivityIdea>,
    pub practical_tips: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalPlaceRef {
    pub provider: String,
    pub place_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpeningHours {
    pub weekday_text: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub id: String,
    pub name: String,
    pub kind: PlaceKind,
    pub lat: f64,
    pub lng: f64,
    pub tz: String,
    pub country_code: String,
    pub admin_area: String,
    pub city: String,
    pub address: String,
    pub external_ref: Option<ExternalPlaceRef>,
    pub website: Option<String>,
    pub phone: Option<String>,
    pub rating: Option<f64>,
    pub price_level: Option<u8>,
    pub opening_hours: Option<OpeningHours>,
    pub photo_urls: Vec<String>,
    pub guide: Option<PlaceGuide>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Shortlisted,
    InPlan,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDisposition {
    Shortlisted,
    Rejected,
}

impl From<CandidateDisposition> for CandidateStatus {
    fn from(value: CandidateDisposition) -> Self {
        match value {
            CandidateDisposition::Shortlisted => Self::Shortlisted,
            CandidateDisposition::Rejected => Self::Rejected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    pub trip_id: String,
    pub source_place_id: Option<String>,
    pub place_id: String,
    pub proposed_by: String,
    pub created_at: String,
    pub pitch: String,
    pub tags: Vec<String>,
    pub status: CandidateStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateWithPlace {
    #[serde(flatten)]
    pub candidate: Candidate,
    pub place: Place,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub id: String,
    pub trip_id: String,
    pub version: u32,
    pub created_from_proposal_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub id: String,
    pub plan_id: String,
    pub date: String,
    pub city_hint: String,
    pub tz: String,
    pub window_start: String,
    pub window_end: String,
}

impl Day {
    /// Wire-level validation guarantees canonical `HH:MM` values, so lexical
    /// ordering also enforces the aggregate's start-before-end invariant.
    pub fn window_is_ordered(&self) -> bool {
        self.window_start <= self.window_end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    Visit,
    Meal,
    Lodging,
    Activity,
    Transit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Money {
    pub amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Booking {
    #[serde(rename = "ref")]
    pub reference: String,
    pub url: Option<String>,
    pub cost: Option<Money>,
    pub ledger_entry_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stop {
    pub id: String,
    pub day_id: String,
    pub seq: f64,
    pub place_id: String,
    pub stop_kind: StopKind,
    pub planned_arrival: String,
    pub duration_min: u32,
    pub booking: Option<Booking>,
    pub notes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TravelMode {
    Walk,
    Transit,
    Drive,
    Flight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feasibility {
    Ok,
    Tight,
    Unreasonable,
    Impossible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Leg {
    pub from_stop_id: String,
    pub to_stop_id: String,
    pub mode: TravelMode,
    pub distance_m: f64,
    pub duration_min: u32,
    pub feasibility: Feasibility,
    pub feasibility_note: Option<String>,
    pub provider_snapshot_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayFeasibility {
    pub day_id: String,
    pub feasibility: Feasibility,
    pub used_min: u32,
    pub window_min: u32,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDetail {
    pub plan: Plan,
    pub days: Vec<Day>,
    pub stops: Vec<Stop>,
    pub legs: Vec<Leg>,
    pub day_feasibility: Vec<DayFeasibility>,
    pub places: Vec<Place>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidatePlaceInput {
    pub name: String,
    pub kind: PlaceKind,
    pub city: String,
    pub address: String,
    pub website: Option<String>,
    pub phone: Option<String>,
    pub opening_hours: Vec<String>,
    pub photo_urls: Vec<String>,
    pub guide: Option<PlaceGuide>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayPatch {
    pub window_start: Option<String>,
    pub window_end: Option<String>,
    pub city_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StopPatch {
    pub planned_arrival: Option<String>,
    pub duration_min: Option<u32>,
    pub notes: Option<String>,
    pub booking: Option<Option<Booking>>,
}
