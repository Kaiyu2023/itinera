//! Query-shaped codecs for immutable plan versions and their current content rows.

use std::collections::HashSet;

use itinera_core::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        proposal::ChangeSet,
        trip::{Booking, Day, Money, Plan, Stop},
    },
    ports::trip::TripRepoError,
    services::content_history::validate_stored_edit,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::sqlite::codec::{checked_revision, validate_id};

pub(in crate::sqlite) const MAX_PLAN_VERSIONS: usize = 1_000;
pub(super) const PLAN_VERSION_QUERY_LIMIT: i64 = 1_001;
pub(in crate::sqlite) const MAX_PLAN_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
// Duplicated ChangeSets are bounded by the 4 MiB proposal collection,
// structural manifests by the 4 MiB history collection, and 1,000 versions
// can retain at most 40 generated 200-character UTF-8 IDs apiece. Forty-eight
// MiB covers those authoritative maxima plus hashes and JSON framing.
pub(in crate::sqlite) const MAX_PLAN_PROVENANCE_BYTES: usize = 48 * 1024 * 1024;

pub(in crate::sqlite) struct BookingColumns<'a> {
    pub(in crate::sqlite) reference: Option<&'a str>,
    pub(in crate::sqlite) url: Option<&'a str>,
    pub(in crate::sqlite) amount: Option<f64>,
    pub(in crate::sqlite) currency: Option<&'a str>,
}

#[derive(Debug, FromRow)]
pub(super) struct PlanRow {
    plan_trip_id: String,
    plan_version: i64,
    plan_id: String,
    created_from_proposal_id: Option<String>,
    plan_created_at: String,
    applied_change_set_json: Option<String>,
    application_entity_ids_json: Option<String>,
    structural_audits_json: Option<String>,
    base_structure_hash: Option<String>,
    structure_hash: Option<String>,
    plan_revision: i64,
}

#[derive(Debug)]
pub(super) struct StoredPlan {
    pub(super) value: Plan,
    pub(super) applied_change_set_json: Option<String>,
    pub(super) application_entity_ids: Vec<String>,
    pub(in crate::sqlite) structural_audits: Vec<StructuralAuditBinding>,
    pub(super) base_structure_hash: Option<String>,
    pub(super) structure_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::sqlite) struct StructuralAuditBinding {
    pub(in crate::sqlite) edit: Edit,
    pub(in crate::sqlite) candidate_place_id: String,
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

#[derive(Debug, FromRow)]
pub(super) struct PlanStructureRow {
    structure_trip_id: String,
    structure_plan_version: i64,
    structure_kind: i64,
    structure_id: String,
    structure_parent_id: String,
    day_date: Option<String>,
    stop_seq: Option<f64>,
    stop_place_id: Option<String>,
    stop_kind: Option<String>,
    stop_identity_id: Option<String>,
    structure_revision: i64,
}

pub(super) enum PlanStructureValue {
    Day(Day),
    Stop(Stop),
}

impl PlanRow {
    pub(super) fn into_stored_plan(
        self,
        expected_trip_id: &str,
    ) -> Result<StoredPlan, TripRepoError> {
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
        let value = Plan {
            id: self.plan_id,
            trip_id: self.plan_trip_id,
            version,
            created_from_proposal_id: self.created_from_proposal_id,
            created_at: self.plan_created_at,
        };
        let valid_hash = |hash: &str| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        if self
            .base_structure_hash
            .as_deref()
            .is_some_and(|hash| !valid_hash(hash))
            || self
                .structure_hash
                .as_deref()
                .is_some_and(|hash| !valid_hash(hash))
        {
            return Err(TripRepoError::CorruptData);
        }
        let applied_change_set_json = self
            .applied_change_set_json
            .map(|encoded| {
                let change_set = serde_json::from_str::<ChangeSet>(&encoded).map_err(corrupt)?;
                let canonical = serde_json::to_string(&change_set).map_err(corrupt)?;
                if canonical == encoded {
                    Ok(canonical)
                } else {
                    Err(TripRepoError::CorruptData)
                }
            })
            .transpose()?;
        let has_application_entity_ids = self.application_entity_ids_json.is_some();
        let application_entity_ids = self
            .application_entity_ids_json
            .map(|encoded| {
                let ids = serde_json::from_str::<Vec<String>>(&encoded).map_err(corrupt)?;
                if ids.len() > 40
                    || serde_json::to_string(&ids).map_err(corrupt)? != encoded
                    || ids.iter().any(|id| validate_id(id).is_err())
                    || ids.iter().collect::<HashSet<_>>().len() != ids.len()
                {
                    return Err(TripRepoError::CorruptData);
                }
                Ok(ids)
            })
            .transpose()?
            .unwrap_or_default();
        let has_structural_audits = self.structural_audits_json.is_some();
        let structural_audits = self
            .structural_audits_json
            .map(|encoded| decode_structural_audits(&value, &encoded))
            .transpose()?
            .unwrap_or_default();
        let staged_shape = match value.version {
            1 => {
                applied_change_set_json.is_none()
                    && !has_application_entity_ids
                    && application_entity_ids.is_empty()
                    && !has_structural_audits
                    && structural_audits.is_empty()
                    && self.base_structure_hash.is_none()
            }
            _ => {
                applied_change_set_json.is_some()
                    && has_application_entity_ids
                    && has_structural_audits
                    && self.base_structure_hash.is_some()
                    && self.structure_hash.is_some()
            }
        };
        if !staged_shape {
            return Err(TripRepoError::CorruptData);
        }
        Ok(StoredPlan {
            value,
            applied_change_set_json,
            application_entity_ids,
            structural_audits,
            base_structure_hash: self.base_structure_hash,
            structure_hash: self.structure_hash,
        })
    }
}

pub(in crate::sqlite) fn encode_structural_audits(
    plan: &Plan,
    bindings: &[StructuralAuditBinding],
) -> Result<String, TripRepoError> {
    validate_structural_audits(plan, bindings)?;
    serde_json::to_string(bindings).map_err(corrupt)
}

fn decode_structural_audits(
    plan: &Plan,
    encoded: &str,
) -> Result<Vec<StructuralAuditBinding>, TripRepoError> {
    let bindings = serde_json::from_str::<Vec<StructuralAuditBinding>>(encoded).map_err(corrupt)?;
    if serde_json::to_string(&bindings).map_err(corrupt)? != encoded {
        return Err(TripRepoError::CorruptData);
    }
    validate_structural_audits(plan, &bindings)?;
    Ok(bindings)
}

fn validate_structural_audits(
    plan: &Plan,
    bindings: &[StructuralAuditBinding],
) -> Result<(), TripRepoError> {
    if bindings.len() > 100 {
        return Err(TripRepoError::CorruptData);
    }
    let mut edit_ids = HashSet::new();
    let mut candidate_ids = HashSet::new();
    for binding in bindings {
        let edit = &binding.edit;
        validate_stored_edit(&plan.trip_id, edit).map_err(corrupt)?;
        validate_id(&binding.candidate_place_id).map_err(corrupt)?;
        let transition = (edit.old_value.as_str(), edit.new_value.as_str());
        if edit.entity != EditEntity::Candidate
            || edit.field != "status"
            || !matches!(
                transition,
                (Some("shortlisted"), Some("in_plan")) | (Some("in_plan"), Some("shortlisted"))
            )
            || !matches!(edit.source, ChangeSource::Web {})
            || edit.status != EditStatus::Applied
            || edit.created_at != plan.created_at
            || !edit_ids.insert(edit.id.as_str())
            || !candidate_ids.insert(edit.entity_id.as_str())
        {
            return Err(TripRepoError::CorruptData);
        }
    }
    Ok(())
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

impl PlanStructureRow {
    pub(super) fn plan_version(&self) -> Result<u32, TripRepoError> {
        u32::try_from(self.structure_plan_version)
            .ok()
            .filter(|version| *version > 0)
            .ok_or(TripRepoError::CorruptData)
    }

    pub(super) fn into_value(
        self,
        expected_trip_id: &str,
        expected_version: u32,
    ) -> Result<PlanStructureValue, TripRepoError> {
        if self.structure_trip_id != expected_trip_id || self.plan_version()? != expected_version {
            return Err(TripRepoError::CorruptData);
        }
        checked_revision(self.structure_revision).map_err(corrupt)?;
        match (
            self.structure_kind,
            self.day_date,
            self.stop_seq,
            self.stop_place_id,
            self.stop_kind,
            self.stop_identity_id,
        ) {
            (0, Some(date), None, None, None, None) => Ok(PlanStructureValue::Day(Day {
                id: self.structure_id,
                plan_id: self.structure_parent_id,
                date,
                city_hint: "Stored".to_string(),
                tz: "UTC".to_string(),
                window_start: "09:00".to_string(),
                window_end: "21:00".to_string(),
            })),
            (1, None, Some(seq), Some(place_id), Some(stop_kind), Some(identity_id))
                if identity_id == self.structure_id =>
            {
                Ok(PlanStructureValue::Stop(Stop {
                    id: self.structure_id,
                    day_id: self.structure_parent_id,
                    seq,
                    place_id,
                    stop_kind: stop_kind.parse().map_err(corrupt)?,
                    planned_arrival: "12:00".to_string(),
                    duration_min: 60,
                    booking: None,
                    notes: String::new(),
                }))
            }
            _ => Err(TripRepoError::CorruptData),
        }
    }
}

pub(in crate::sqlite) fn encode_booking_columns(
    booking: Option<&Booking>,
) -> Result<BookingColumns<'_>, TripRepoError> {
    let Some(booking) = booking else {
        return Ok(BookingColumns {
            reference: None,
            url: None,
            amount: None,
            currency: None,
        });
    };
    if booking.ledger_entry_id.is_some() {
        return Err(TripRepoError::CorruptData);
    }
    Ok(BookingColumns {
        reference: Some(booking.reference.as_str()),
        url: booking.url.as_deref(),
        amount: booking.cost.as_ref().map(|cost| cost.amount),
        currency: booking.cost.as_ref().map(|cost| cost.currency.as_str()),
    })
}

fn corrupt<T>(_error: T) -> TripRepoError {
    TripRepoError::CorruptData
}
