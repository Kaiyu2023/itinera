use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate};
use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

use super::{
    currency::{CurrencyCode, InvalidCurrencyCode},
    user::Email,
};

pub const MAX_TRIP_MEMBERS: usize = 1_000;
const MAX_TRIP_DAYS: i64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TripValidationError {
    #[error("trip id must be non-empty, normalized, and at most 200 characters")]
    InvalidTripId,
    #[error("name is required and must be at most 120 characters")]
    InvalidName,
    #[error("cover photo URL must be an absolute HTTP or HTTPS URL")]
    InvalidCoverPhotoUrl,
    #[error("accent color must be non-empty, normalized, and at most 128 characters")]
    InvalidAccentColor,
    #[error("stop-kind labels must be normalized and bounded")]
    InvalidStopKindLabels,
    #[error("date must use YYYY-MM-DD")]
    InvalidDate,
    #[error("endDate must not be before startDate")]
    EndBeforeStart,
    #[error("a trip may contain at most 90 days")]
    DateRangeTooLong,
    #[error(transparent)]
    InvalidCurrency(#[from] InvalidCurrencyCode),
    #[error("soft budget must have a non-negative finite amount and canonical currency")]
    InvalidSoftBudget,
    #[error("a trip must have at least one member")]
    MissingMember,
    #[error("a trip may have at most 1,000 current members")]
    TooManyMembers,
    #[error("trip members must have unique user ids")]
    DuplicateMember,
    #[error("a trip must have at least one leader")]
    MissingLeader,
    #[error("current plan id must be non-empty, normalized, and at most 200 characters")]
    InvalidCurrentPlanId,
    #[error("createdAt must be a canonical UTC RFC 3339 timestamp")]
    InvalidCreatedAt,
    #[error("member user id must be non-empty, normalized, and at most 200 characters")]
    InvalidMemberId,
    #[error("joinedAt must be a canonical UTC RFC 3339 timestamp")]
    InvalidJoinedAt,
    #[error("invite id must be non-empty, normalized, and at most 200 characters")]
    InvalidInviteId,
    #[error("invite trip id must be non-empty, normalized, and at most 200 characters")]
    InvalidInviteTripId,
    #[error("invite email must be canonical and at most 320 bytes")]
    InvalidInviteEmail,
    #[error("inviter id must be non-empty, normalized, and at most 200 characters")]
    InvalidInviterId,
    #[error("invite createdAt must be a canonical UTC RFC 3339 timestamp")]
    InvalidInviteCreatedAt,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TripMember {
    user_id: String,
    role: TripRole,
    joined_at: String,
}

impl TripMember {
    pub fn rehydrate(
        user_id: String,
        role: TripRole,
        joined_at: String,
    ) -> Result<Self, TripValidationError> {
        if !valid_id(&user_id) {
            return Err(TripValidationError::InvalidMemberId);
        }
        if !valid_utc(&joined_at) {
            return Err(TripValidationError::InvalidJoinedAt);
        }
        Ok(Self {
            user_id,
            role,
            joined_at,
        })
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn role(&self) -> TripRole {
        self.role
    }

    pub fn joined_at(&self) -> &str {
        &self.joined_at
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoftBudget {
    amount: f64,
    currency: CurrencyCode,
}

impl SoftBudget {
    pub fn rehydrate(amount: f64, currency: String) -> Result<Self, TripValidationError> {
        if !amount.is_finite() || amount < 0.0 {
            return Err(TripValidationError::InvalidSoftBudget);
        }
        Ok(Self {
            amount,
            currency: CurrencyCode::try_from(currency)?,
        })
    }

    pub fn amount(&self) -> f64 {
        self.amount
    }

    pub fn currency(&self) -> &CurrencyCode {
        &self.currency
    }
}

impl<'de> Deserialize<'de> for SoftBudget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct RawSoftBudget {
            amount: f64,
            currency: String,
        }

        let raw = RawSoftBudget::deserialize(deserializer)?;
        Self::rehydrate(raw.amount, raw.currency).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trip {
    id: String,
    name: String,
    cover_photo_url: Option<String>,
    accent_color: Option<String>,
    stop_kind_labels: Option<HashMap<StopKind, String>>,
    status: TripStatus,
    #[serde(flatten)]
    dates: TripDates,
    base_currency: CurrencyCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    soft_budget: Option<SoftBudget>,
    members: Vec<TripMember>,
    current_plan_id: Option<String>,
    created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TripDates {
    start_date: String,
    end_date: String,
    #[serde(skip)]
    start: NaiveDate,
    #[serde(skip)]
    end: NaiveDate,
}

impl TripDates {
    fn rehydrate(start_date: String, end_date: String) -> Result<Self, TripValidationError> {
        let start = parse_date(&start_date)?;
        let end = parse_date(&end_date)?;
        let span = end.signed_duration_since(start).num_days();
        if span < 0 {
            return Err(TripValidationError::EndBeforeStart);
        }
        if span >= MAX_TRIP_DAYS {
            return Err(TripValidationError::DateRangeTooLong);
        }
        Ok(Self {
            start_date,
            end_date,
            start,
            end,
        })
    }

    fn values(&self) -> Vec<NaiveDate> {
        let span = self.end.signed_duration_since(self.start).num_days();
        (0..=span)
            .map(|offset| {
                self.start
                    .checked_add_days(chrono::Days::new(offset as u64))
                    .expect("the validated end date proves every included day is representable")
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTripInput {
    pub id: String,
    pub name: String,
    pub start_date: String,
    pub end_date: String,
    pub base_currency: String,
    pub creator_id: String,
    pub created_at: String,
}

/// A trip that is valid specifically for first persistence.
///
/// Rehydrated mature trips cannot be passed to `TripRepo::create_trip`, so the
/// creator-only leader and empty-plan rules do not depend on a repository
/// remembering another validation step.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTrip(Trip);

impl NewTrip {
    pub fn as_trip(&self) -> &Trip {
        &self.0
    }

    pub fn into_trip(self) -> Trip {
        self.0
    }
}

/// Raw aggregate state supplied by a persistence codec.
///
/// `Trip::rehydrate` is the only way this state becomes a `Trip`, so stored
/// scalar and cross-field invariants cannot be skipped by a repository.
#[derive(Debug, Clone, PartialEq)]
pub struct TripState {
    pub id: String,
    pub name: String,
    pub cover_photo_url: Option<String>,
    pub accent_color: Option<String>,
    pub stop_kind_labels: Option<HashMap<StopKind, String>>,
    pub status: TripStatus,
    pub start_date: String,
    pub end_date: String,
    pub base_currency: String,
    pub soft_budget: Option<SoftBudget>,
    pub members: Vec<TripMember>,
    pub current_plan_id: Option<String>,
    pub created_at: String,
}

impl Trip {
    pub fn create(input: NewTripInput) -> Result<NewTrip, TripValidationError> {
        let name = input.name.trim().to_string();
        let base_currency = CurrencyCode::from_user_input(&input.base_currency)?;
        let dates = TripDates::rehydrate(input.start_date, input.end_date)?;
        let creator =
            TripMember::rehydrate(input.creator_id, TripRole::Leader, input.created_at.clone())?;
        let trip = Self {
            id: input.id,
            name,
            cover_photo_url: None,
            accent_color: None,
            stop_kind_labels: None,
            status: TripStatus::Dreaming,
            dates,
            base_currency,
            soft_budget: None,
            members: vec![creator],
            current_plan_id: None,
            created_at: input.created_at,
        };
        trip.validate()?;
        Ok(NewTrip(trip))
    }

    pub fn rehydrate(state: TripState) -> Result<Self, TripValidationError> {
        let dates = TripDates::rehydrate(state.start_date, state.end_date)?;
        let trip = Self {
            id: state.id,
            name: state.name,
            cover_photo_url: state.cover_photo_url,
            accent_color: state.accent_color,
            stop_kind_labels: state.stop_kind_labels,
            status: state.status,
            dates,
            base_currency: CurrencyCode::try_from(state.base_currency)?,
            soft_budget: state.soft_budget,
            members: state.members,
            current_plan_id: state.current_plan_id,
            created_at: state.created_at,
        };
        trip.validate()?;
        Ok(trip)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn cover_photo_url(&self) -> Option<&str> {
        self.cover_photo_url.as_deref()
    }

    pub fn accent_color(&self) -> Option<&str> {
        self.accent_color.as_deref()
    }

    pub fn stop_kind_labels(&self) -> Option<&HashMap<StopKind, String>> {
        self.stop_kind_labels.as_ref()
    }

    pub fn status(&self) -> TripStatus {
        self.status
    }

    pub fn start_date(&self) -> &str {
        &self.dates.start_date
    }

    pub fn end_date(&self) -> &str {
        &self.dates.end_date
    }

    pub fn dates(&self) -> Vec<NaiveDate> {
        self.dates.values()
    }

    pub fn base_currency(&self) -> &CurrencyCode {
        &self.base_currency
    }

    pub fn soft_budget(&self) -> Option<&SoftBudget> {
        self.soft_budget.as_ref()
    }

    pub fn members(&self) -> &[TripMember] {
        &self.members
    }

    pub fn current_plan_id(&self) -> Option<&str> {
        self.current_plan_id.as_deref()
    }

    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    pub fn set_status(&mut self, status: TripStatus) {
        self.status = status;
    }

    pub fn set_current_plan_id(
        &mut self,
        current_plan_id: Option<String>,
    ) -> Result<(), TripValidationError> {
        if current_plan_id.as_deref().is_some_and(|id| !valid_id(id)) {
            return Err(TripValidationError::InvalidCurrentPlanId);
        }
        self.current_plan_id = current_plan_id;
        Ok(())
    }

    pub fn add_member(&mut self, member: TripMember) -> Result<(), TripValidationError> {
        if self.members.len() >= MAX_TRIP_MEMBERS {
            return Err(TripValidationError::TooManyMembers);
        }
        if self
            .members
            .iter()
            .any(|existing| existing.user_id == member.user_id)
        {
            return Err(TripValidationError::DuplicateMember);
        }
        self.members.push(member);
        Ok(())
    }

    pub fn remove_member(
        &mut self,
        user_id: &str,
    ) -> Result<Option<TripMember>, TripValidationError> {
        let Some(index) = self
            .members
            .iter()
            .position(|member| member.user_id == user_id)
        else {
            return Ok(None);
        };
        if self.members[index].role == TripRole::Leader
            && self
                .members
                .iter()
                .filter(|member| member.role == TripRole::Leader)
                .count()
                == 1
        {
            return Err(TripValidationError::MissingLeader);
        }
        Ok(Some(self.members.remove(index)))
    }

    fn validate(&self) -> Result<(), TripValidationError> {
        if !valid_id(&self.id) {
            return Err(TripValidationError::InvalidTripId);
        }
        if !valid_text(&self.name, 120) {
            return Err(TripValidationError::InvalidName);
        }
        if self
            .cover_photo_url
            .as_deref()
            .is_some_and(|value| !valid_http_url(value))
        {
            return Err(TripValidationError::InvalidCoverPhotoUrl);
        }
        if self
            .accent_color
            .as_deref()
            .is_some_and(|value| !valid_text(value, 128))
        {
            return Err(TripValidationError::InvalidAccentColor);
        }
        if self.stop_kind_labels.as_ref().is_some_and(|labels| {
            labels.len() > 5 || labels.values().any(|value| !valid_text(value, 200))
        }) {
            return Err(TripValidationError::InvalidStopKindLabels);
        }
        if self.members.is_empty() {
            return Err(TripValidationError::MissingMember);
        }
        if self.members.len() > MAX_TRIP_MEMBERS {
            return Err(TripValidationError::TooManyMembers);
        }
        let unique_members = self
            .members
            .iter()
            .map(|member| member.user_id.as_str())
            .collect::<HashSet<_>>();
        if unique_members.len() != self.members.len() {
            return Err(TripValidationError::DuplicateMember);
        }
        if !self
            .members
            .iter()
            .any(|member| member.role == TripRole::Leader)
        {
            return Err(TripValidationError::MissingLeader);
        }
        if self
            .current_plan_id
            .as_deref()
            .is_some_and(|id| !valid_id(id))
        {
            return Err(TripValidationError::InvalidCurrentPlanId);
        }
        if !valid_utc(&self.created_at) {
            return Err(TripValidationError::InvalidCreatedAt);
        }
        Ok(())
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    id: String,
    trip_id: String,
    email: String,
    invited_by: String,
    status: InviteStatus,
    created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInviteInput {
    pub id: String,
    pub trip_id: String,
    pub email: Email,
    pub invited_by: String,
    pub created_at: String,
}

/// A validated invite whose lifecycle is guaranteed to be `pending`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInvite(Invite);

impl PendingInvite {
    pub fn as_invite(&self) -> &Invite {
        &self.0
    }

    pub fn into_invite(self) -> Invite {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteState {
    pub id: String,
    pub trip_id: String,
    pub email: String,
    pub invited_by: String,
    pub status: InviteStatus,
    pub created_at: String,
}

impl Invite {
    pub fn create(input: NewInviteInput) -> Result<PendingInvite, TripValidationError> {
        Self::rehydrate(InviteState {
            id: input.id,
            trip_id: input.trip_id,
            email: input.email.to_string(),
            invited_by: input.invited_by,
            status: InviteStatus::Pending,
            created_at: input.created_at,
        })
        .map(PendingInvite)
    }

    pub fn rehydrate(state: InviteState) -> Result<Self, TripValidationError> {
        if !valid_id(&state.id) {
            return Err(TripValidationError::InvalidInviteId);
        }
        if !valid_id(&state.trip_id) {
            return Err(TripValidationError::InvalidInviteTripId);
        }
        let canonical_email =
            Email::parse(&state.email).map_err(|_| TripValidationError::InvalidInviteEmail)?;
        if canonical_email.as_str() != state.email {
            return Err(TripValidationError::InvalidInviteEmail);
        }
        if !valid_id(&state.invited_by) {
            return Err(TripValidationError::InvalidInviterId);
        }
        if !valid_utc(&state.created_at) {
            return Err(TripValidationError::InvalidInviteCreatedAt);
        }
        Ok(Self {
            id: state.id,
            trip_id: state.trip_id,
            email: state.email,
            invited_by: state.invited_by,
            status: state.status,
            created_at: state.created_at,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn trip_id(&self) -> &str {
        &self.trip_id
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn invited_by(&self) -> &str {
        &self.invited_by
    }

    pub fn status(&self) -> InviteStatus {
        self.status
    }

    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    pub fn accept(&mut self) {
        self.status = InviteStatus::Accepted;
    }
}

fn valid_id(value: &str) -> bool {
    valid_text(value, 200)
}

fn valid_text(value: &str, max_chars: usize) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= max_chars
}

fn valid_http_url(value: &str) -> bool {
    valid_text(value, 2_048)
        && Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn valid_utc(value: &str) -> bool {
    value.len() <= 35
        && value.ends_with('Z')
        && value.as_bytes().get(10) == Some(&b'T')
        && DateTime::parse_from_rfc3339(value)
            .is_ok_and(|timestamp| timestamp.offset().local_minus_utc() == 0)
}

fn parse_date(value: &str) -> Result<NaiveDate, TripValidationError> {
    let parsed = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| TripValidationError::InvalidDate)?;
    if parsed.format("%Y-%m-%d").to_string() == value {
        Ok(parsed)
    } else {
        Err(TripValidationError::InvalidDate)
    }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NOW: &str = "2026-08-07T12:00:00Z";

    fn member(user_id: &str, role: TripRole) -> TripMember {
        TripMember::rehydrate(user_id.to_string(), role, NOW.to_string())
            .expect("valid membership fixture")
    }

    fn valid_state() -> TripState {
        TripState {
            id: "trip-a".into(),
            name: "Japan".into(),
            cover_photo_url: Some("https://images.example/trip.jpg".into()),
            accent_color: Some("#123abc".into()),
            stop_kind_labels: Some(HashMap::from([
                (StopKind::Activity, "Onsen".into()),
                (StopKind::Meal, "Food".into()),
            ])),
            status: TripStatus::Planning,
            start_date: "2026-11-01".into(),
            end_date: "2026-11-03".into(),
            base_currency: "GBP".into(),
            soft_budget: Some(
                SoftBudget::rehydrate(500.0, "JPY".into()).expect("valid soft budget"),
            ),
            // Deliberately keep the leader second: member ordering carries no
            // authorization meaning.
            members: vec![
                member("viewer", TripRole::Viewer),
                member("leader", TripRole::Leader),
            ],
            current_plan_id: Some("plan-a".into()),
            created_at: NOW.into(),
        }
    }

    #[test]
    fn creation_normalizes_input_and_owns_initial_aggregate_state() {
        let trip = Trip::create(NewTripInput {
            id: "trip-a".into(),
            name: "  Japan  ".into(),
            start_date: "2026-11-01".into(),
            end_date: "2026-11-03".into(),
            base_currency: " gbp ".into(),
            creator_id: "creator".into(),
            created_at: NOW.into(),
        })
        .expect("valid trip");
        let trip = trip.as_trip();

        assert_eq!(trip.name(), "Japan");
        assert_eq!(trip.base_currency().as_str(), "GBP");
        assert_eq!(trip.status(), TripStatus::Dreaming);
        assert_eq!(trip.current_plan_id(), None);
        assert_eq!(trip.members().len(), 1);
        assert_eq!(trip.members()[0].user_id(), "creator");
        assert_eq!(trip.members()[0].role(), TripRole::Leader);
        assert_eq!(trip.members()[0].joined_at(), NOW);
    }

    #[test]
    fn rehydration_accepts_a_leader_anywhere_in_the_member_collection() {
        let trip = Trip::rehydrate(valid_state()).expect("valid stored aggregate");
        assert_eq!(trip.members()[0].role(), TripRole::Viewer);
        assert_eq!(trip.members()[1].role(), TripRole::Leader);
    }

    #[test]
    fn validated_private_state_keeps_the_existing_wire_shape() {
        let trip = Trip::rehydrate(valid_state()).expect("valid stored aggregate");
        let value = serde_json::to_value(&trip).expect("serialize trip");
        assert_eq!(value["id"], json!("trip-a"));
        assert_eq!(value["baseCurrency"], json!("GBP"));
        assert_eq!(value["softBudget"]["amount"], json!(500.0));
        assert_eq!(value["softBudget"]["currency"], json!("JPY"));
        assert_eq!(value["members"][0]["userId"], json!("viewer"));
        assert_eq!(value["members"][1]["role"], json!("leader"));
    }

    #[test]
    fn scalar_and_nested_values_fail_closed_during_rehydration() {
        let mut state = valid_state();
        state.id = " padded ".into();
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::InvalidTripId)
        );

        let mut state = valid_state();
        state.name = " ".into();
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::InvalidName)
        );

        let mut state = valid_state();
        state.cover_photo_url = Some("javascript:alert(1)".into());
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::InvalidCoverPhotoUrl)
        );

        let mut state = valid_state();
        state.accent_color = Some(String::new());
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::InvalidAccentColor)
        );

        let mut state = valid_state();
        state
            .stop_kind_labels
            .as_mut()
            .unwrap()
            .insert(StopKind::Visit, " padded ".into());
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::InvalidStopKindLabels)
        );

        let mut state = valid_state();
        state.base_currency = "gbp".into();
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::InvalidCurrency(InvalidCurrencyCode))
        );

        assert_eq!(
            SoftBudget::rehydrate(f64::NAN, "GBP".into()),
            Err(TripValidationError::InvalidSoftBudget)
        );
        assert!(
            serde_json::from_value::<SoftBudget>(json!({"amount": 5.0, "currency": "gbp"}))
                .is_err()
        );

        let mut state = valid_state();
        state.current_plan_id = Some(" plan ".into());
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::InvalidCurrentPlanId)
        );

        let mut state = valid_state();
        state.created_at = "2026-08-07T13:00:00+01:00".into();
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::InvalidCreatedAt)
        );
    }

    #[test]
    fn date_cross_field_boundaries_are_part_of_the_aggregate() {
        let mut state = valid_state();
        state.start_date = "2026-02-29".into();
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::InvalidDate)
        );

        let mut state = valid_state();
        state.start_date = "2026-11-04".into();
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::EndBeforeStart)
        );

        let mut state = valid_state();
        state.start_date = "2026-01-01".into();
        state.end_date = "2026-03-31".into();
        let trip = Trip::rehydrate(state).expect("90 inclusive days");
        let dates = trip.dates();
        assert_eq!(dates.len(), 90);
        assert_eq!(dates.first().unwrap().to_string(), "2026-01-01");
        assert_eq!(dates.last().unwrap().to_string(), "2026-03-31");

        let mut state = valid_state();
        state.start_date = "2026-01-01".into();
        state.end_date = "2026-04-01".into();
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::DateRangeTooLong)
        );
    }

    #[test]
    fn membership_cross_field_invariants_are_centralized() {
        let mut state = valid_state();
        state.members.clear();
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::MissingMember)
        );

        let mut state = valid_state();
        state.members = vec![member("viewer", TripRole::Viewer)];
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::MissingLeader)
        );

        let mut state = valid_state();
        state.members = vec![
            member("same", TripRole::Leader),
            member("same", TripRole::Member),
        ];
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::DuplicateMember)
        );

        let mut state = valid_state();
        state.members = (0..=MAX_TRIP_MEMBERS)
            .map(|index| {
                member(
                    &format!("member-{index}"),
                    if index == 0 {
                        TripRole::Leader
                    } else {
                        TripRole::Member
                    },
                )
            })
            .collect();
        assert_eq!(
            Trip::rehydrate(state),
            Err(TripValidationError::TooManyMembers)
        );
    }

    #[test]
    fn membership_and_mutation_apis_preserve_invariants() {
        assert_eq!(
            TripMember::rehydrate(" bad ".into(), TripRole::Member, NOW.into()),
            Err(TripValidationError::InvalidMemberId)
        );
        assert_eq!(
            TripMember::rehydrate(
                "member".into(),
                TripRole::Member,
                "2026-08-07T13:00:00+01:00".into(),
            ),
            Err(TripValidationError::InvalidJoinedAt)
        );

        let mut trip = Trip::create(NewTripInput {
            id: "trip-a".into(),
            name: "Japan".into(),
            start_date: "2026-11-01".into(),
            end_date: "2026-11-03".into(),
            base_currency: "GBP".into(),
            creator_id: "leader-a".into(),
            created_at: NOW.into(),
        })
        .unwrap()
        .into_trip();
        assert_eq!(
            trip.add_member(member("leader-a", TripRole::Member)),
            Err(TripValidationError::DuplicateMember)
        );
        assert_eq!(
            trip.remove_member("leader-a"),
            Err(TripValidationError::MissingLeader)
        );
        trip.add_member(member("leader-b", TripRole::Leader))
            .unwrap();
        assert!(trip.remove_member("leader-a").unwrap().is_some());
        assert_eq!(trip.members()[0].user_id(), "leader-b");

        assert_eq!(
            trip.set_current_plan_id(Some(" invalid ".into())),
            Err(TripValidationError::InvalidCurrentPlanId)
        );
        assert_eq!(trip.current_plan_id(), None);
    }

    #[test]
    fn invites_are_created_pending_and_rehydrated_canonically() {
        let email = Email::parse("Friend@Example.COM").unwrap();
        let mut invite = Invite::create(NewInviteInput {
            id: "invite-a".into(),
            trip_id: "trip-a".into(),
            email,
            invited_by: "leader".into(),
            created_at: NOW.into(),
        })
        .expect("valid invite")
        .into_invite();
        assert_eq!(invite.email(), "friend@example.com");
        assert_eq!(invite.status(), InviteStatus::Pending);
        invite.accept();
        assert_eq!(invite.status(), InviteStatus::Accepted);

        let mut state = InviteState {
            id: "invite-a".into(),
            trip_id: "trip-a".into(),
            email: "Friend@example.com".into(),
            invited_by: "leader".into(),
            status: InviteStatus::Pending,
            created_at: NOW.into(),
        };
        assert_eq!(
            Invite::rehydrate(state.clone()),
            Err(TripValidationError::InvalidInviteEmail)
        );
        state.email = "friend@example.com".into();
        state.created_at = "not-a-time".into();
        assert_eq!(
            Invite::rehydrate(state),
            Err(TripValidationError::InvalidInviteCreatedAt)
        );
    }
}
