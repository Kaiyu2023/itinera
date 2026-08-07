use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    domain::{
        content_history::ChangeSource,
        proposal::{
            ChangeOp, ChangeSet, NewPlaceDraft, Proposal, ProposalDecision, ProposalRoute,
            ProposalStatus,
        },
        trip::{Day, Place, Plan, PlanDetail, Stop},
        user::UserId,
    },
    ports::{
        clock::Clock,
        id_gen::IdGen,
        poll::{PollRepo, PollRepoError},
        proposal::{ProposalApplicationIds, ProposalRepo, ProposalRepoError},
    },
};

use super::{
    plans::validate_stored_plan_graph,
    polls::new_plan_change_poll,
    validation::{
        ValidationError, date, http_url, required_text, text_len, validate_place_snapshot,
    },
};

pub const MAX_CHANGE_OPS: usize = 20;
pub const MAX_REORDER_STOPS: usize = 50;
const RESERVED_ENTITY_IDS_PER_OP: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProposalInput {
    pub title: String,
    pub rationale: String,
    pub change_set: ChangeSet,
    pub route: ProposalRoute,
}

#[derive(Debug, thiserror::Error)]
pub enum ProposalServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] ProposalRepoError),
    #[error(transparent)]
    PollRepository(#[from] PollRepoError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanApplication {
    pub plan: Plan,
    pub days: Vec<Day>,
    pub stops: Vec<Stop>,
    pub new_places: Vec<Place>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChangeApplicationError {
    #[error("the current plan data is corrupt")]
    CorruptData,
    #[error("a referenced trip-owned entity does not exist in the current plan")]
    NotFound,
    #[error("the ChangeSet is invalid for the current plan")]
    InvalidChange,
}

pub async fn list_proposals(
    repo: &dyn ProposalRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<Proposal>, ProposalServiceError> {
    validate_id(trip_id, "tripId is invalid")?;
    repo.list_proposals(trip_id, actor)
        .await
        .map_err(Into::into)
}

pub async fn create_proposal(
    repo: &dyn ProposalRepo,
    polls: &dyn PollRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    input: CreateProposalInput,
) -> Result<Proposal, ProposalServiceError> {
    validate_id(trip_id, "tripId is invalid")?;
    let input = normalise_create_input(input)?;
    let proposal = Proposal {
        id: ids.new_id(),
        trip_id: trip_id.to_string(),
        created_by: actor.0.clone(),
        source: ChangeSource::Web {},
        title: input.title,
        rationale: input.rationale,
        change_set: input.change_set,
        route: input.route,
        status: ProposalStatus::Pending,
        decided_by: None,
        rejection_reason: None,
        created_at: clock.now(),
    };
    if proposal.route == ProposalRoute::Poll {
        let poll = new_plan_change_poll(ids, &proposal.created_at)?;
        return Ok(polls
            .create_proposal_poll(trip_id, actor, proposal, poll, reserve_application_ids(ids))
            .await?);
    }
    Ok(repo
        .create_proposal(trip_id, actor, proposal, reserve_application_ids(ids))
        .await?)
}

pub async fn approve_proposal(
    repo: &dyn ProposalRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    proposal_id: &str,
) -> Result<Proposal, ProposalServiceError> {
    validate_id(trip_id, "tripId is invalid")?;
    validate_id(proposal_id, "proposalId is invalid")?;
    repo.approve_proposal(
        trip_id,
        actor,
        proposal_id,
        &clock.now(),
        reserve_application_ids(ids),
    )
    .await
    .map_err(Into::into)
}

pub async fn reject_proposal(
    repo: &dyn ProposalRepo,
    trip_id: &str,
    actor: &UserId,
    proposal_id: &str,
    reason: String,
) -> Result<Proposal, ProposalServiceError> {
    validate_id(trip_id, "tripId is invalid")?;
    validate_id(proposal_id, "proposalId is invalid")?;
    let reason = required_text(
        reason,
        "reason is required and must be at most 2,000 characters",
        2_000,
    )?;
    repo.reject_proposal(trip_id, actor, proposal_id, &reason)
        .await
        .map_err(Into::into)
}

pub fn validate_stored_proposal(
    expected_trip_id: &str,
    proposal: &Proposal,
) -> Result<(), ChangeApplicationError> {
    if validate_id(&proposal.id, "proposal id is invalid").is_err()
        || proposal.trip_id != expected_trip_id
        || validate_id(&proposal.created_by, "proposal creator is invalid").is_err()
        || required_text(proposal.title.clone(), "proposal title is invalid", 200).is_err()
        || proposal.title.trim() != proposal.title
        || proposal.rationale.chars().count() > 4_000
        || proposal.rationale.trim() != proposal.rationale
        || normalise_change_set(proposal.change_set.clone()).as_ref() != Ok(&proposal.change_set)
        || !matches!(proposal.source, ChangeSource::Web {})
    {
        return Err(ChangeApplicationError::CorruptData);
    }

    let valid_decision = |decision: &ProposalDecision| match decision {
        ProposalDecision::Leader { user_id } | ProposalDecision::Poll { poll_id: user_id } => {
            validate_id(user_id, "decision id is invalid").is_ok()
        }
    };
    let decision_matches_route = |decision: &ProposalDecision| {
        matches!(
            (proposal.route, decision),
            (
                ProposalRoute::LeaderApproval,
                ProposalDecision::Leader { .. }
            ) | (ProposalRoute::Poll, ProposalDecision::Poll { .. })
        ) && valid_decision(decision)
    };
    let decision_valid = match proposal.status {
        ProposalStatus::Pending => {
            (match proposal.route {
                ProposalRoute::LeaderApproval => proposal.decided_by.is_none(),
                ProposalRoute::Poll => proposal
                    .decided_by
                    .as_ref()
                    .is_some_and(decision_matches_route),
            }) && proposal.rejection_reason.is_none()
        }
        ProposalStatus::Rejected => {
            proposal
                .decided_by
                .as_ref()
                .is_some_and(decision_matches_route)
                && proposal.rejection_reason.as_ref().is_some_and(|reason| {
                    required_text(reason.clone(), "rejection reason is invalid", 2_000)
                        .is_ok_and(|normalised| normalised == *reason)
                })
        }
        ProposalStatus::Applied => {
            proposal
                .decided_by
                .as_ref()
                .is_some_and(decision_matches_route)
                && proposal.rejection_reason.is_none()
        }
        ProposalStatus::Stale => {
            proposal.rejection_reason.is_none()
                && match proposal.route {
                    ProposalRoute::LeaderApproval => proposal
                        .decided_by
                        .as_ref()
                        .is_none_or(decision_matches_route),
                    ProposalRoute::Poll => proposal
                        .decided_by
                        .as_ref()
                        .is_some_and(decision_matches_route),
                }
        }
        ProposalStatus::Draft | ProposalStatus::Approved => false,
    };
    if !decision_valid {
        return Err(ChangeApplicationError::CorruptData);
    }
    Ok(())
}

pub fn apply_change_set(
    current: &PlanDetail,
    expected_trip_id: &str,
    proposal_id: &str,
    change_set: &ChangeSet,
    resolved_places: &HashMap<String, Place>,
    applied_at: &str,
    application_ids: ProposalApplicationIds,
) -> Result<PlanApplication, ChangeApplicationError> {
    validate_current_plan(current, expected_trip_id, change_set.base_plan_version)?;
    if normalise_change_set(change_set.clone()).as_ref() != Ok(change_set) {
        return Err(ChangeApplicationError::CorruptData);
    }

    let ProposalApplicationIds {
        plan_id,
        entity_ids,
    } = application_ids;
    if validate_id(&plan_id, "plan id is invalid").is_err() {
        return Err(ChangeApplicationError::CorruptData);
    }
    let mut days = current.days.clone();
    let mut stops = current.stops.clone();
    let mut places = current
        .places
        .iter()
        .chain(resolved_places.values())
        .map(|place| (place.id.clone(), place.clone()))
        .collect::<HashMap<_, _>>();
    for place in places.values() {
        validate_place_snapshot(place).map_err(|_| ChangeApplicationError::CorruptData)?;
    }
    let mut ids = VecDeque::from(entity_ids);
    let mut used_ids = HashSet::from([current.plan.id.clone()]);
    used_ids.extend(days.iter().map(|day| day.id.clone()));
    used_ids.extend(stops.iter().map(|stop| stop.id.clone()));
    used_ids.extend(places.keys().cloned());
    if !used_ids.insert(plan_id.clone()) {
        return Err(ChangeApplicationError::CorruptData);
    }
    let mut new_places = Vec::new();
    let mut changed = false;

    for op in &change_set.ops {
        match op {
            ChangeOp::AddStop {
                day_id,
                place_id,
                seq,
                stop_kind,
            } => {
                require_day(&days, day_id)?;
                if !places.contains_key(place_id) {
                    return Err(ChangeApplicationError::NotFound);
                }
                let stop_id = take_id(&mut ids, &mut used_ids)?;
                stops.push(Stop {
                    id: stop_id,
                    day_id: day_id.clone(),
                    seq: *seq,
                    place_id: place_id.clone(),
                    stop_kind: *stop_kind,
                    planned_arrival: "12:00".into(),
                    duration_min: 60,
                    booking: None,
                    notes: String::new(),
                });
                resequence(&mut stops, day_id);
                changed = true;
            }
            ChangeOp::AddPlaceStop {
                day_id,
                seq,
                stop_kind,
                draft,
            } => {
                let day = require_day(&days, day_id)?.clone();
                let place_id = take_id(&mut ids, &mut used_ids)?;
                let stop_id = take_id(&mut ids, &mut used_ids)?;
                let place = materialise_draft(place_id, draft, &day);
                validate_place_snapshot(&place).map_err(|_| ChangeApplicationError::CorruptData)?;
                places.insert(place.id.clone(), place.clone());
                new_places.push(place.clone());
                stops.push(Stop {
                    id: stop_id,
                    day_id: day_id.clone(),
                    seq: *seq,
                    place_id: place.id,
                    stop_kind: *stop_kind,
                    planned_arrival: "12:00".into(),
                    duration_min: 60,
                    booking: None,
                    notes: draft.note.clone(),
                });
                resequence(&mut stops, day_id);
                changed = true;
            }
            ChangeOp::RemoveStop { stop_id } => {
                let index = require_stop_index(&stops, stop_id)?;
                stops.remove(index);
                changed = true;
            }
            ChangeOp::MoveStop {
                stop_id,
                to_day_id,
                seq,
            } => {
                require_day(&days, to_day_id)?;
                let index = require_stop_index(&stops, stop_id)?;
                let from_day_id = stops[index].day_id.clone();
                changed |= stops[index].day_id != *to_day_id || stops[index].seq != *seq;
                stops[index].day_id = to_day_id.clone();
                stops[index].seq = *seq;
                resequence(&mut stops, &from_day_id);
                resequence(&mut stops, to_day_id);
            }
            ChangeOp::Reorder {
                day_id,
                stop_ids_in_order,
            } => {
                require_day(&days, day_id)?;
                let current_ids = ordered_stop_ids(&stops, day_id);
                let requested = stop_ids_in_order.iter().collect::<HashSet<_>>();
                if requested.len() != stop_ids_in_order.len()
                    || current_ids.len() != stop_ids_in_order.len()
                    || current_ids.iter().any(|id| !requested.contains(id))
                {
                    return Err(ChangeApplicationError::InvalidChange);
                }
                changed |= current_ids != *stop_ids_in_order;
                for (index, stop_id) in stop_ids_in_order.iter().enumerate() {
                    let stop_index = require_stop_index(&stops, stop_id)?;
                    let stop = &mut stops[stop_index];
                    if stop.day_id != *day_id {
                        return Err(ChangeApplicationError::InvalidChange);
                    }
                    stop.seq = (index + 1) as f64;
                }
            }
            ChangeOp::SwapPlace {
                stop_id,
                new_place_id,
            } => {
                if !places.contains_key(new_place_id) {
                    return Err(ChangeApplicationError::NotFound);
                }
                let index = require_stop_index(&stops, stop_id)?;
                changed |= stops[index].place_id != *new_place_id;
                stops[index].place_id = new_place_id.clone();
            }
            ChangeOp::AddDay { date, city_hint } => {
                if days.iter().any(|day| day.date == *date) {
                    return Err(ChangeApplicationError::InvalidChange);
                }
                let day_id = take_id(&mut ids, &mut used_ids)?;
                let tz = days
                    .iter()
                    .find(|day| day.city_hint == *city_hint)
                    .or_else(|| days.first())
                    .map_or_else(|| "UTC".to_string(), |day| day.tz.clone());
                days.push(Day {
                    id: day_id,
                    plan_id: plan_id.clone(),
                    date: date.clone(),
                    city_hint: city_hint.clone(),
                    tz,
                    window_start: "09:00".into(),
                    window_end: "21:00".into(),
                });
                changed = true;
            }
            ChangeOp::RemoveDay { day_id } => {
                let index = days
                    .iter()
                    .position(|day| day.id == *day_id)
                    .ok_or(ChangeApplicationError::NotFound)?;
                days.remove(index);
                stops.retain(|stop| stop.day_id != *day_id);
                changed = true;
            }
        }
    }

    if !changed || days.is_empty() {
        return Err(ChangeApplicationError::InvalidChange);
    }
    // Every published version uses canonical integer ordering even when the
    // request used a fractional insertion hint. This keeps persisted stop keys
    // sortable and prevents two equal hints from colliding.
    for day_id in days.iter().map(|day| day.id.clone()).collect::<Vec<_>>() {
        resequence(&mut stops, &day_id);
    }
    let resulting_place_ids = stops
        .iter()
        .map(|stop| stop.place_id.as_str())
        .collect::<HashSet<_>>();
    new_places.retain(|place| resulting_place_ids.contains(place.id.as_str()));
    if days == current.days && stops == current.stops {
        return Err(ChangeApplicationError::InvalidChange);
    }
    let day_ids = days
        .iter()
        .map(|day| day.id.as_str())
        .collect::<HashSet<_>>();
    let stop_ids = stops
        .iter()
        .map(|stop| stop.id.as_str())
        .collect::<HashSet<_>>();
    if day_ids.len() != days.len()
        || stop_ids.len() != stops.len()
        || stops.iter().any(|stop| {
            !day_ids.contains(stop.day_id.as_str()) || !places.contains_key(&stop.place_id)
        })
    {
        return Err(ChangeApplicationError::CorruptData);
    }

    for day in &mut days {
        day.plan_id.clone_from(&plan_id);
    }
    days.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| left.id.cmp(&right.id))
    });
    stops.sort_by(|left, right| {
        left.day_id
            .cmp(&right.day_id)
            .then_with(|| left.seq.total_cmp(&right.seq))
            .then_with(|| left.id.cmp(&right.id))
    });
    let version = current
        .plan
        .version
        .checked_add(1)
        .ok_or(ChangeApplicationError::InvalidChange)?;
    Ok(PlanApplication {
        plan: Plan {
            id: plan_id,
            trip_id: expected_trip_id.to_string(),
            version,
            created_from_proposal_id: Some(proposal_id.to_string()),
            created_at: applied_at.to_string(),
        },
        days,
        stops,
        new_places,
    })
}

fn normalise_create_input(
    input: CreateProposalInput,
) -> Result<CreateProposalInput, ValidationError> {
    Ok(CreateProposalInput {
        title: required_text(
            input.title,
            "title is required and must be at most 200 characters",
            200,
        )?,
        rationale: normalise_optional_text(input.rationale, 4_000)?,
        change_set: normalise_change_set(input.change_set)?,
        route: input.route,
    })
}

fn normalise_change_set(mut change_set: ChangeSet) -> Result<ChangeSet, ValidationError> {
    if change_set.base_plan_version == 0 {
        return Err(ValidationError("basePlanVersion must be at least 1"));
    }
    if change_set.ops.is_empty() || change_set.ops.len() > MAX_CHANGE_OPS {
        return Err(ValidationError(
            "a proposal must contain between 1 and 20 operations",
        ));
    }
    for op in &mut change_set.ops {
        match op {
            ChangeOp::AddStop {
                day_id,
                place_id,
                seq,
                ..
            } => {
                normalise_id(day_id, "dayId is invalid")?;
                normalise_id(place_id, "placeId is invalid")?;
                validate_seq(*seq)?;
            }
            ChangeOp::AddPlaceStop {
                day_id, seq, draft, ..
            } => {
                normalise_id(day_id, "dayId is invalid")?;
                validate_seq(*seq)?;
                normalise_draft(draft)?;
            }
            ChangeOp::RemoveStop { stop_id } => normalise_id(stop_id, "stopId is invalid")?,
            ChangeOp::MoveStop {
                stop_id,
                to_day_id,
                seq,
            } => {
                normalise_id(stop_id, "stopId is invalid")?;
                normalise_id(to_day_id, "toDayId is invalid")?;
                validate_seq(*seq)?;
            }
            ChangeOp::Reorder {
                day_id,
                stop_ids_in_order,
            } => {
                normalise_id(day_id, "dayId is invalid")?;
                if stop_ids_in_order.is_empty() || stop_ids_in_order.len() > MAX_REORDER_STOPS {
                    return Err(ValidationError(
                        "stopIdsInOrder must contain between 1 and 50 ids",
                    ));
                }
                for stop_id in stop_ids_in_order {
                    normalise_id(stop_id, "stopIdsInOrder contains an invalid id")?;
                }
            }
            ChangeOp::SwapPlace {
                stop_id,
                new_place_id,
            } => {
                normalise_id(stop_id, "stopId is invalid")?;
                normalise_id(new_place_id, "newPlaceId is invalid")?;
            }
            ChangeOp::AddDay {
                date: value,
                city_hint,
            } => {
                date(value)?;
                *city_hint = required_text(
                    std::mem::take(city_hint),
                    "cityHint is required and must be at most 120 characters",
                    120,
                )?;
            }
            ChangeOp::RemoveDay { day_id } => normalise_id(day_id, "dayId is invalid")?,
        }
    }
    Ok(change_set)
}

fn normalise_draft(draft: &mut NewPlaceDraft) -> Result<(), ValidationError> {
    draft.name = required_text(
        std::mem::take(&mut draft.name),
        "draft.name is required and must be at most 200 characters",
        200,
    )?;
    draft.city = required_text(
        std::mem::take(&mut draft.city),
        "draft.city is required and must be at most 120 characters",
        120,
    )?;
    text_len(&draft.note, 10_000)?;
    draft.url = http_url(draft.url.take())?;
    if draft.lat.is_some() != draft.lng.is_some()
        || draft
            .lat
            .is_some_and(|value| !value.is_finite() || !(-90.0..=90.0).contains(&value))
        || draft
            .lng
            .is_some_and(|value| !value.is_finite() || !(-180.0..=180.0).contains(&value))
    {
        return Err(ValidationError(
            "draft coordinates must be a valid latitude/longitude pair or both null",
        ));
    }
    Ok(())
}

fn normalise_optional_text(value: String, max_len: usize) -> Result<String, ValidationError> {
    let value = value.trim().to_string();
    if value.chars().count() > max_len {
        return Err(ValidationError("text exceeds the allowed length"));
    }
    Ok(value)
}

fn validate_id(value: &str, error: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() || value.trim() != value || value.chars().count() > 200 {
        Err(ValidationError(error))
    } else {
        Ok(())
    }
}

fn normalise_id(value: &mut String, error: &'static str) -> Result<(), ValidationError> {
    *value = required_text(std::mem::take(value), error, 200)?;
    Ok(())
}

fn validate_seq(value: f64) -> Result<(), ValidationError> {
    if value.is_finite() && value > 0.0 && value <= 1_000_000.0 {
        Ok(())
    } else {
        Err(ValidationError(
            "seq must be a finite number greater than 0 and at most 1,000,000",
        ))
    }
}

pub fn reserve_application_ids(ids: &dyn IdGen) -> ProposalApplicationIds {
    ProposalApplicationIds {
        plan_id: ids.new_id(),
        entity_ids: (0..MAX_CHANGE_OPS * RESERVED_ENTITY_IDS_PER_OP)
            .map(|_| ids.new_id())
            .collect(),
    }
}

fn validate_current_plan(
    current: &PlanDetail,
    expected_trip_id: &str,
    expected_version: u32,
) -> Result<(), ChangeApplicationError> {
    if validate_stored_plan_graph(
        &current.plan,
        &current.days,
        &current.stops,
        expected_trip_id,
        expected_version,
    )
    .is_err()
    {
        return Err(ChangeApplicationError::CorruptData);
    }
    let place_ids = current
        .places
        .iter()
        .map(|place| place.id.as_str())
        .collect::<HashSet<_>>();
    if place_ids.len() != current.places.len()
        || current
            .stops
            .iter()
            .any(|stop| !place_ids.contains(stop.place_id.as_str()))
    {
        return Err(ChangeApplicationError::CorruptData);
    }
    Ok(())
}

fn require_day<'a>(days: &'a [Day], day_id: &str) -> Result<&'a Day, ChangeApplicationError> {
    days.iter()
        .find(|day| day.id == day_id)
        .ok_or(ChangeApplicationError::NotFound)
}

fn require_stop_index(stops: &[Stop], stop_id: &str) -> Result<usize, ChangeApplicationError> {
    stops
        .iter()
        .position(|stop| stop.id == stop_id)
        .ok_or(ChangeApplicationError::NotFound)
}

fn ordered_stop_ids(stops: &[Stop], day_id: &str) -> Vec<String> {
    let mut day_stops = stops
        .iter()
        .filter(|stop| stop.day_id == day_id)
        .collect::<Vec<_>>();
    day_stops.sort_by(|left, right| {
        left.seq
            .total_cmp(&right.seq)
            .then_with(|| left.id.cmp(&right.id))
    });
    day_stops.into_iter().map(|stop| stop.id.clone()).collect()
}

fn resequence(stops: &mut [Stop], day_id: &str) {
    let mut indices = stops
        .iter()
        .enumerate()
        .filter(|(_, stop)| stop.day_id == day_id)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        stops[*left]
            .seq
            .total_cmp(&stops[*right].seq)
            .then_with(|| stops[*left].id.cmp(&stops[*right].id))
    });
    for (sequence, index) in indices.into_iter().enumerate() {
        stops[index].seq = (sequence + 1) as f64;
    }
}

fn take_id(
    ids: &mut VecDeque<String>,
    used: &mut HashSet<String>,
) -> Result<String, ChangeApplicationError> {
    let id = ids.pop_front().ok_or(ChangeApplicationError::CorruptData)?;
    if validate_id(&id, "generated id is invalid").is_err() || !used.insert(id.clone()) {
        return Err(ChangeApplicationError::CorruptData);
    }
    Ok(id)
}

fn materialise_draft(id: String, draft: &NewPlaceDraft, day: &Day) -> Place {
    Place {
        id,
        name: draft.name.clone(),
        kind: draft.kind,
        lat: draft.lat.unwrap_or(0.0),
        lng: draft.lng.unwrap_or(0.0),
        tz: day.tz.clone(),
        country_code: String::new(),
        admin_area: draft.city.clone(),
        city: draft.city.clone(),
        address: draft.city.clone(),
        external_ref: None,
        website: draft.url.clone(),
        phone: None,
        rating: None,
        price_level: None,
        opening_hours: None,
        photo_urls: vec![],
        guide: None,
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::trip::{Feasibility, PlaceKind, StopKind};

    use super::*;

    fn detail() -> PlanDetail {
        PlanDetail {
            plan: Plan {
                id: "plan-1".into(),
                trip_id: "trip-a".into(),
                version: 1,
                created_from_proposal_id: None,
                created_at: "2026-08-01T00:00:00Z".into(),
            },
            days: vec![Day {
                id: "day-a".into(),
                plan_id: "plan-1".into(),
                date: "2026-11-01".into(),
                city_hint: "Kyoto".into(),
                tz: "Asia/Tokyo".into(),
                window_start: "09:00".into(),
                window_end: "21:00".into(),
            }],
            stops: vec![Stop {
                id: "stop-a".into(),
                day_id: "day-a".into(),
                seq: 1.0,
                place_id: "place-a".into(),
                stop_kind: StopKind::Visit,
                planned_arrival: "10:00".into(),
                duration_min: 60,
                booking: None,
                notes: String::new(),
            }],
            legs: vec![],
            day_feasibility: vec![crate::domain::trip::DayFeasibility {
                day_id: "day-a".into(),
                feasibility: Feasibility::Ok,
                used_min: 60,
                window_min: 720,
                notes: vec![],
            }],
            places: vec![place("place-a", "Temple")],
        }
    }

    fn place(id: &str, name: &str) -> Place {
        Place {
            id: id.into(),
            name: name.into(),
            kind: PlaceKind::Sight,
            lat: 35.0,
            lng: 135.0,
            tz: "Asia/Tokyo".into(),
            country_code: "JP".into(),
            admin_area: "Kyoto".into(),
            city: "Kyoto".into(),
            address: "Kyoto".into(),
            external_ref: None,
            website: None,
            phone: None,
            rating: None,
            price_level: None,
            opening_hours: None,
            photo_urls: vec![],
            guide: None,
        }
    }

    fn ids() -> ProposalApplicationIds {
        ProposalApplicationIds {
            plan_id: "plan-2".into(),
            entity_ids: vec!["generated-a".into(), "generated-b".into()],
        }
    }

    #[test]
    fn application_clones_the_plan_and_keeps_the_old_version_unchanged() {
        let current = detail();
        let before = current.clone();
        let extra = HashMap::from([("place-b".into(), place("place-b", "Garden"))]);
        let change_set = ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::AddStop {
                day_id: "day-a".into(),
                place_id: "place-b".into(),
                seq: 2.0,
                stop_kind: StopKind::Visit,
            }],
        };

        let applied = apply_change_set(
            &current,
            "trip-a",
            "proposal-a",
            &change_set,
            &extra,
            "2026-08-06T12:00:00Z",
            ids(),
        )
        .expect("change applies");

        assert_eq!(current, before);
        assert_eq!(applied.plan.version, 2);
        assert_eq!(
            applied.plan.created_from_proposal_id.as_deref(),
            Some("proposal-a")
        );
        assert!(applied.days.iter().all(|day| day.plan_id == "plan-2"));
        assert_eq!(applied.stops.len(), 2);
    }

    #[test]
    fn validation_is_order_aware_and_rejects_removed_references() {
        let current = detail();
        let change_set = ChangeSet {
            base_plan_version: 1,
            ops: vec![
                ChangeOp::RemoveStop {
                    stop_id: "stop-a".into(),
                },
                ChangeOp::MoveStop {
                    stop_id: "stop-a".into(),
                    to_day_id: "day-a".into(),
                    seq: 1.0,
                },
            ],
        };

        assert_eq!(
            apply_change_set(
                &current,
                "trip-a",
                "proposal-a",
                &change_set,
                &HashMap::new(),
                "2026-08-06T12:00:00Z",
                ids(),
            ),
            Err(ChangeApplicationError::NotFound)
        );
    }

    #[test]
    fn a_hand_typed_place_uses_neutral_provider_facts() {
        let current = detail();
        let change_set = ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::AddPlaceStop {
                day_id: "day-a".into(),
                seq: 2.0,
                stop_kind: StopKind::Meal,
                draft: NewPlaceDraft {
                    name: "Cafe".into(),
                    kind: PlaceKind::Food,
                    city: "Kyoto".into(),
                    note: "Try breakfast".into(),
                    url: None,
                    lat: None,
                    lng: None,
                },
            }],
        };

        let applied = apply_change_set(
            &current,
            "trip-a",
            "proposal-a",
            &change_set,
            &HashMap::new(),
            "2026-08-06T12:00:00Z",
            ids(),
        )
        .expect("change applies");
        let place = &applied.new_places[0];
        assert_eq!((place.lat, place.lng), (0.0, 0.0));
        assert_eq!(place.tz, "Asia/Tokyo");
        assert!(place.external_ref.is_none());
        assert_eq!(
            applied.stops.last().expect("new stop").notes,
            "Try breakfast"
        );
    }

    #[test]
    fn corrupt_source_content_and_duplicate_dates_fail_closed() {
        let mut corrupt = detail();
        corrupt.stops[0].planned_arrival = "25:00".into();
        let remove = ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::RemoveStop {
                stop_id: "stop-a".into(),
            }],
        };
        assert_eq!(
            apply_change_set(
                &corrupt,
                "trip-a",
                "proposal-a",
                &remove,
                &HashMap::new(),
                "2026-08-06T12:00:00Z",
                ids(),
            ),
            Err(ChangeApplicationError::CorruptData)
        );

        let duplicate_date = ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::AddDay {
                date: "2026-11-01".into(),
                city_hint: "Kyoto".into(),
            }],
        };
        assert_eq!(
            apply_change_set(
                &detail(),
                "trip-a",
                "proposal-a",
                &duplicate_date,
                &HashMap::new(),
                "2026-08-06T12:00:00Z",
                ids(),
            ),
            Err(ChangeApplicationError::InvalidChange)
        );
    }

    #[test]
    fn canonical_semantic_no_ops_are_rejected() {
        let same_slot = ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::MoveStop {
                stop_id: "stop-a".into(),
                to_day_id: "day-a".into(),
                seq: 2.0,
            }],
        };
        assert_eq!(
            apply_change_set(
                &detail(),
                "trip-a",
                "proposal-a",
                &same_slot,
                &HashMap::new(),
                "2026-08-06T12:00:00Z",
                ids(),
            ),
            Err(ChangeApplicationError::InvalidChange)
        );

        let net_cancelling = ChangeSet {
            base_plan_version: 1,
            ops: vec![
                ChangeOp::SwapPlace {
                    stop_id: "stop-a".into(),
                    new_place_id: "place-b".into(),
                },
                ChangeOp::SwapPlace {
                    stop_id: "stop-a".into(),
                    new_place_id: "place-a".into(),
                },
            ],
        };
        assert_eq!(
            apply_change_set(
                &detail(),
                "trip-a",
                "proposal-a",
                &net_cancelling,
                &HashMap::from([("place-b".into(), place("place-b", "Garden"))]),
                "2026-08-06T12:00:00Z",
                ids(),
            ),
            Err(ChangeApplicationError::InvalidChange)
        );
    }

    #[test]
    fn generated_ids_cannot_collide_with_the_source_aggregate() {
        let change_set = ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::AddStop {
                day_id: "day-a".into(),
                place_id: "place-b".into(),
                seq: 2.0,
                stop_kind: StopKind::Visit,
            }],
        };
        let places = HashMap::from([("place-b".into(), place("place-b", "Garden"))]);

        let colliding_plan = ProposalApplicationIds {
            plan_id: "plan-1".into(),
            entity_ids: vec!["stop-b".into()],
        };
        assert_eq!(
            apply_change_set(
                &detail(),
                "trip-a",
                "proposal-a",
                &change_set,
                &places,
                "2026-08-06T12:00:00Z",
                colliding_plan,
            ),
            Err(ChangeApplicationError::CorruptData)
        );

        let colliding_stop = ProposalApplicationIds {
            plan_id: "plan-2".into(),
            entity_ids: vec!["stop-a".into()],
        };
        assert_eq!(
            apply_change_set(
                &detail(),
                "trip-a",
                "proposal-a",
                &change_set,
                &places,
                "2026-08-06T12:00:00Z",
                colliding_stop,
            ),
            Err(ChangeApplicationError::CorruptData)
        );
    }
}
