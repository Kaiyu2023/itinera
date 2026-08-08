//! Query-shaped codecs for immutable plan versions and their current content rows.

use itinera_core::{
    domain::trip::{Booking, Day, Money, Plan, Stop},
    ports::trip::TripRepoError,
};
use sqlx::FromRow;

use crate::sqlite::codec::{checked_revision, validate_id};

pub(super) const MAX_PLAN_VERSIONS: usize = 1_000;
pub(super) const PLAN_VERSION_QUERY_LIMIT: i64 = 1_001;
pub(super) const MAX_PLAN_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, FromRow)]
pub(super) struct PlanRow {
    plan_trip_id: String,
    plan_version: i64,
    plan_id: String,
    created_from_proposal_id: Option<String>,
    plan_created_at: String,
    plan_revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct DayRow {
    day_trip_id: String,
    day_plan_version: i64,
    day_id: String,
    day_plan_id: String,
    day_date: String,
    day_city_hint: String,
    day_tz: String,
    day_window_start: String,
    day_window_end: String,
    day_revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct StopRow {
    stop_trip_id: String,
    stop_plan_version: i64,
    stop_id: String,
    stop_day_id: String,
    stop_seq: f64,
    stop_place_id: String,
    stop_kind: String,
    stop_planned_arrival: String,
    stop_duration_min: i64,
    booking_ref: Option<String>,
    booking_url: Option<String>,
    booking_cost_amount: Option<f64>,
    booking_cost_currency: Option<String>,
    stop_notes: String,
    stop_revision: i64,
}

impl PlanRow {
    pub(super) fn into_plan(self, expected_trip_id: &str) -> Result<Plan, TripRepoError> {
        if self.plan_trip_id != expected_trip_id {
            return Err(TripRepoError::CorruptData);
        }
        validate_id(&self.plan_id).map_err(corrupt)?;
        if let Some(proposal_id) = self.created_from_proposal_id.as_deref() {
            validate_id(proposal_id).map_err(corrupt)?;
        }
        checked_revision(self.plan_revision).map_err(corrupt)?;
        let version = u32::try_from(self.plan_version)
            .ok()
            .filter(|version| *version > 0)
            .ok_or(TripRepoError::CorruptData)?;
        Ok(Plan {
            id: self.plan_id,
            trip_id: self.plan_trip_id,
            version,
            created_from_proposal_id: self.created_from_proposal_id,
            created_at: self.plan_created_at,
        })
    }
}

impl DayRow {
    pub(super) fn into_day(
        self,
        expected_trip_id: &str,
        expected_version: u32,
    ) -> Result<Day, TripRepoError> {
        if self.day_trip_id != expected_trip_id
            || u32::try_from(self.day_plan_version).ok() != Some(expected_version)
        {
            return Err(TripRepoError::CorruptData);
        }
        checked_revision(self.day_revision).map_err(corrupt)?;
        Ok(Day {
            id: self.day_id,
            plan_id: self.day_plan_id,
            date: self.day_date,
            city_hint: self.day_city_hint,
            tz: self.day_tz,
            window_start: self.day_window_start,
            window_end: self.day_window_end,
        })
    }
}

impl StopRow {
    pub(super) fn into_stop(
        self,
        expected_trip_id: &str,
        expected_version: u32,
    ) -> Result<Stop, TripRepoError> {
        if self.stop_trip_id != expected_trip_id
            || u32::try_from(self.stop_plan_version).ok() != Some(expected_version)
        {
            return Err(TripRepoError::CorruptData);
        }
        checked_revision(self.stop_revision).map_err(corrupt)?;
        let duration_min = u32::try_from(self.stop_duration_min).map_err(corrupt)?;
        let booking = match (
            self.booking_ref,
            self.booking_url,
            self.booking_cost_amount,
            self.booking_cost_currency,
        ) {
            (None, None, None, None) => None,
            (Some(reference), url, None, None) => Some(Booking {
                reference,
                url,
                cost: None,
                ledger_entry_id: None,
            }),
            (Some(reference), url, Some(amount), Some(currency)) => Some(Booking {
                reference,
                url,
                cost: Some(Money { amount, currency }),
                ledger_entry_id: None,
            }),
            _ => return Err(TripRepoError::CorruptData),
        };
        Ok(Stop {
            id: self.stop_id,
            day_id: self.stop_day_id,
            seq: self.stop_seq,
            place_id: self.stop_place_id,
            stop_kind: self.stop_kind.parse().map_err(corrupt)?,
            planned_arrival: self.stop_planned_arrival,
            duration_min,
            booking,
            notes: self.stop_notes,
        })
    }
}

fn corrupt<T>(_error: T) -> TripRepoError {
    TripRepoError::CorruptData
}
