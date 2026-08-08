//! Query-shaped codecs for candidate-owned place snapshots.

use itinera_core::{
    domain::trip::{Candidate, CandidateStatus, CandidateWithPlace, Place, PlaceKind},
    ports::trip::TripRepoError,
    services::{candidates::validate_stored_candidate, validation::validate_place_snapshot},
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::FromRow;

use crate::sqlite::codec::{checked_revision, decode_json, encode_json};

pub(super) const MAX_PLACE_SEARCH_ITEMS: usize = 100;
pub(super) const PLACE_SEARCH_QUERY_LIMIT: i64 = 101;
pub(super) const MAX_CANDIDATE_ITEMS: usize = 1_000;
pub(super) const CANDIDATE_QUERY_LIMIT: i64 = 1_001;
pub(super) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

const MAX_EXTERNAL_REF_JSON_BYTES: usize = 1_024;
const MAX_OPENING_HOURS_JSON_BYTES: usize = 4_096;
const MAX_TAGS_JSON_BYTES: usize = 4_096;
const MAX_PLACE_LIST_JSON_BYTES: usize = 65_536;

#[derive(Debug, FromRow)]
pub(super) struct PlaceRow {
    place_trip_id: String,
    place_id: String,
    place_name: String,
    place_kind: String,
    place_lat: f64,
    place_lng: f64,
    place_tz: String,
    place_country_code: String,
    place_admin_area: String,
    place_city: String,
    place_address: String,
    place_external_ref_json: Option<String>,
    place_website: Option<String>,
    place_phone: Option<String>,
    place_rating: Option<f64>,
    place_price_level: Option<i64>,
    place_opening_hours_json: Option<String>,
    place_photo_urls_json: String,
    place_guide_json: Option<String>,
    place_revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct CandidateRow {
    candidate_trip_id: String,
    candidate_id: String,
    candidate_place_id: String,
    source_catalog_place_id: Option<String>,
    source_trip_place_id: Option<String>,
    proposed_by: String,
    candidate_created_at: String,
    candidate_pitch: String,
    candidate_tags_json: String,
    candidate_status: String,
    candidate_revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct CandidatePlaceRow {
    #[sqlx(flatten)]
    candidate: CandidateRow,
    #[sqlx(flatten)]
    place: PlaceRow,
}

pub(super) struct EncodedPlace {
    pub(super) external_ref_json: Option<String>,
    pub(super) opening_hours_json: Option<String>,
    pub(super) photo_urls_json: String,
    pub(super) guide_json: Option<String>,
}

impl PlaceRow {
    pub(super) fn into_place(self, expected_trip_id: &str) -> Result<Place, TripRepoError> {
        if self.place_trip_id != expected_trip_id {
            return Err(TripRepoError::CorruptData);
        }
        checked_revision(self.place_revision).map_err(corrupt)?;
        let price_level = self
            .place_price_level
            .map(u8::try_from)
            .transpose()
            .map_err(corrupt)?;
        let place = Place {
            id: self.place_id,
            name: self.place_name,
            kind: decode_place_kind(&self.place_kind)?,
            lat: self.place_lat,
            lng: self.place_lng,
            tz: self.place_tz,
            country_code: self.place_country_code,
            admin_area: self.place_admin_area,
            city: self.place_city,
            address: self.place_address,
            external_ref: decode_optional_json(
                self.place_external_ref_json,
                MAX_EXTERNAL_REF_JSON_BYTES,
            )?,
            website: self.place_website,
            phone: self.place_phone,
            rating: self.place_rating,
            price_level,
            opening_hours: decode_optional_json(
                self.place_opening_hours_json,
                MAX_OPENING_HOURS_JSON_BYTES,
            )?,
            photo_urls: decode_required_json(
                self.place_photo_urls_json,
                MAX_PLACE_LIST_JSON_BYTES,
            )?,
            guide: decode_optional_json(self.place_guide_json, MAX_PLACE_LIST_JSON_BYTES)?,
        };
        validate_place_snapshot(&place).map_err(corrupt)?;
        Ok(place)
    }
}

impl CandidateRow {
    fn into_candidate(self, expected_trip_id: &str) -> Result<Candidate, TripRepoError> {
        checked_revision(self.candidate_revision).map_err(corrupt)?;
        let source_place_id = match (self.source_catalog_place_id, self.source_trip_place_id) {
            (Some(_), Some(_)) => return Err(TripRepoError::CorruptData),
            (Some(id), None) | (None, Some(id)) => Some(id),
            (None, None) => None,
        };
        let candidate = Candidate {
            id: self.candidate_id,
            trip_id: self.candidate_trip_id,
            source_place_id,
            place_id: self.candidate_place_id,
            proposed_by: self.proposed_by,
            created_at: self.candidate_created_at,
            pitch: self.candidate_pitch,
            tags: decode_required_json(self.candidate_tags_json, MAX_TAGS_JSON_BYTES)?,
            status: decode_candidate_status(&self.candidate_status)?,
        };
        validate_stored_candidate(expected_trip_id, &candidate).map_err(corrupt)?;
        if candidate.source_place_id.as_deref() == Some(candidate.place_id.as_str()) {
            return Err(TripRepoError::CorruptData);
        }
        Ok(candidate)
    }
}

impl CandidatePlaceRow {
    pub(super) fn source_trip_place_id(&self) -> Option<&str> {
        self.candidate.source_trip_place_id.as_deref()
    }

    pub(super) fn into_candidate(
        self,
        expected_trip_id: &str,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        let candidate = self.candidate.into_candidate(expected_trip_id)?;
        let place = self.place.into_place(expected_trip_id)?;
        if candidate.place_id != place.id {
            return Err(TripRepoError::CorruptData);
        }
        Ok(CandidateWithPlace { candidate, place })
    }
}

pub(super) fn encode_place(place: &Place) -> Result<EncodedPlace, TripRepoError> {
    validate_place_snapshot(place).map_err(corrupt)?;
    Ok(EncodedPlace {
        external_ref_json: encode_optional_json(
            place.external_ref.as_ref(),
            MAX_EXTERNAL_REF_JSON_BYTES,
        )?,
        opening_hours_json: encode_optional_json(
            place.opening_hours.as_ref(),
            MAX_OPENING_HOURS_JSON_BYTES,
        )?,
        photo_urls_json: encode_required_json(&place.photo_urls, MAX_PLACE_LIST_JSON_BYTES)?,
        guide_json: encode_optional_json(place.guide.as_ref(), MAX_PLACE_LIST_JSON_BYTES)?,
    })
}

pub(super) fn encode_tags(tags: &[String]) -> Result<String, TripRepoError> {
    encode_required_json(tags, MAX_TAGS_JSON_BYTES)
}

pub(super) fn encode_place_kind(kind: PlaceKind) -> &'static str {
    match kind {
        PlaceKind::Sight => "sight",
        PlaceKind::Food => "food",
        PlaceKind::Lodging => "lodging",
        PlaceKind::Activity => "activity",
        PlaceKind::TransportHub => "transport_hub",
    }
}

pub(super) fn encode_candidate_status(status: CandidateStatus) -> &'static str {
    match status {
        CandidateStatus::Shortlisted => "shortlisted",
        CandidateStatus::InPlan => "in_plan",
        CandidateStatus::Rejected => "rejected",
    }
}

pub(super) fn encoded_size<T: Serialize + ?Sized>(value: &T) -> Result<usize, TripRepoError> {
    serde_json::to_vec(value)
        .map(|encoded| encoded.len())
        .map_err(corrupt)
}

fn decode_place_kind(value: &str) -> Result<PlaceKind, TripRepoError> {
    match value {
        "sight" => Ok(PlaceKind::Sight),
        "food" => Ok(PlaceKind::Food),
        "lodging" => Ok(PlaceKind::Lodging),
        "activity" => Ok(PlaceKind::Activity),
        "transport_hub" => Ok(PlaceKind::TransportHub),
        _ => Err(TripRepoError::CorruptData),
    }
}

fn decode_candidate_status(value: &str) -> Result<CandidateStatus, TripRepoError> {
    match value {
        "shortlisted" => Ok(CandidateStatus::Shortlisted),
        "in_plan" => Ok(CandidateStatus::InPlan),
        "rejected" => Ok(CandidateStatus::Rejected),
        _ => Err(TripRepoError::CorruptData),
    }
}

fn encode_optional_json<T: Serialize>(
    value: Option<&T>,
    maximum: usize,
) -> Result<Option<String>, TripRepoError> {
    value
        .map(|value| encode_required_json(value, maximum))
        .transpose()
}

fn encode_required_json<T: Serialize + ?Sized>(
    value: &T,
    maximum: usize,
) -> Result<String, TripRepoError> {
    let encoded = encode_json(value).map_err(corrupt)?;
    if encoded.len() > maximum {
        return Err(TripRepoError::CorruptData);
    }
    Ok(encoded)
}

fn decode_optional_json<T: DeserializeOwned + Serialize>(
    value: Option<String>,
    maximum: usize,
) -> Result<Option<T>, TripRepoError> {
    value
        .map(|value| decode_required_json(value, maximum))
        .transpose()
}

fn decode_required_json<T: DeserializeOwned + Serialize>(
    value: String,
    maximum: usize,
) -> Result<T, TripRepoError> {
    if value.len() > maximum {
        return Err(TripRepoError::CorruptData);
    }
    let decoded = decode_json::<T>(&value).map_err(corrupt)?;
    if encode_required_json(&decoded, maximum)? != value {
        return Err(TripRepoError::CorruptData);
    }
    Ok(decoded)
}

fn corrupt<T>(_error: T) -> TripRepoError {
    TripRepoError::CorruptData
}
