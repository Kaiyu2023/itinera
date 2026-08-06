//! Stateful fake for API route tests only.
//!
//! Production and development builds always use DynamoDB. This fake preserves
//! fast router coverage without becoming a selectable persistence adapter.

use std::{
    collections::{HashMap, HashSet},
    sync::RwLock,
};

use async_trait::async_trait;
use itinera_core::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        proposal::{ChangeOp, Proposal, ProposalDecision, ProposalRoute, ProposalStatus},
        trip::{
            Candidate, CandidateDisposition, CandidateStatus, CandidateWithPlace, Day,
            DayFeasibility, DayPatch, Feasibility, Invite, InviteStatus, Place, Plan, PlanDetail,
            Stop, StopPatch, Trip, TripRole, TripStatus, TripSummary,
        },
        user::{User, UserId},
    },
    ports::{
        content_history::{ContentHistoryRepo, ContentHistoryRepoError},
        proposal::{ProposalApplicationIds, ProposalRepo, ProposalRepoError},
        trip::{CandidateUpdate, TripRepo, TripRepoError},
        user::{UserRepo, UserRepoError},
    },
    services::proposals::{apply_change_set, validate_stored_proposal},
};

#[derive(Default)]
struct State {
    trips: HashMap<String, Trip>,
    invites: HashMap<(String, String), Invite>,
    candidates: HashMap<(String, String), Candidate>,
    places: HashMap<(String, String), Place>,
    plans: HashMap<(String, u32), Plan>,
    days: HashMap<(String, String), Day>,
    stops: HashMap<(String, String), Stop>,
    edits: HashMap<(String, String), Edit>,
    proposals: HashMap<(String, String), Proposal>,
}

#[derive(Default)]
pub struct TestTripRepo {
    state: RwLock<State>,
}

impl TestTripRepo {
    pub fn new() -> Self {
        Self::default()
    }
}

fn role(state: &State, trip_id: &str, actor: &UserId) -> Result<TripRole, TripRepoError> {
    state
        .trips
        .get(trip_id)
        .ok_or(TripRepoError::NotFound)?
        .members
        .iter()
        .find(|member| member.user_id == actor.0)
        .map(|member| member.role)
        // Do not reveal whether another person's trip exists.
        .ok_or(TripRepoError::NotFound)
}

fn require_editor(state: &State, trip_id: &str, actor: &UserId) -> Result<(), TripRepoError> {
    if role(state, trip_id, actor)?.can_edit() {
        Ok(())
    } else {
        Err(TripRepoError::Forbidden)
    }
}

fn require_leader(state: &State, trip_id: &str, actor: &UserId) -> Result<(), TripRepoError> {
    if role(state, trip_id, actor)? == TripRole::Leader {
        Ok(())
    } else {
        Err(TripRepoError::Forbidden)
    }
}

fn summary(state: &State, trip: &Trip) -> TripSummary {
    let mut cities = trip
        .current_plan_id
        .as_ref()
        .map(|plan_id| {
            state
                .days
                .iter()
                .filter(|((trip_id, _), day)| trip_id == &trip.id && &day.plan_id == plan_id)
                .map(|(_, day)| day.city_hint.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut seen = HashSet::new();
    cities.retain(|city| seen.insert(city.clone()));
    TripSummary {
        id: trip.id.clone(),
        name: trip.name.clone(),
        cover_photo_url: trip.cover_photo_url.clone(),
        accent_color: trip.accent_color.clone(),
        status: trip.status,
        start_date: trip.start_date.clone(),
        end_date: trip.end_date.clone(),
        member_count: trip.members.len() as u32,
        cities,
    }
}

fn plan_detail(state: &State, trip_id: &str) -> Result<PlanDetail, TripRepoError> {
    let trip = state.trips.get(trip_id).ok_or(TripRepoError::NotFound)?;
    let current_plan_id = trip
        .current_plan_id
        .as_ref()
        .ok_or(TripRepoError::NotFound)?;
    let plan = state
        .plans
        .values()
        .find(|plan| plan.trip_id == trip_id && plan.id.as_str() == current_plan_id.as_str())
        .cloned()
        .ok_or(TripRepoError::CorruptData)?;
    let mut days = state
        .days
        .iter()
        .filter(|((stored_trip_id, _), day)| stored_trip_id == trip_id && day.plan_id == plan.id)
        .map(|(_, day)| day.clone())
        .collect::<Vec<_>>();
    days.sort_by(|a, b| a.date.cmp(&b.date));
    let day_ids = days
        .iter()
        .map(|day| day.id.as_str())
        .collect::<HashSet<_>>();
    let mut stops = state
        .stops
        .iter()
        .filter(|((stored_trip_id, _), stop)| {
            stored_trip_id == trip_id && day_ids.contains(stop.day_id.as_str())
        })
        .map(|(_, stop)| stop.clone())
        .collect::<Vec<_>>();
    stops.sort_by(|a, b| {
        a.day_id
            .cmp(&b.day_id)
            .then_with(|| a.seq.total_cmp(&b.seq))
    });
    let mut places = Vec::new();
    let mut place_ids = HashSet::new();
    for stop in &stops {
        if place_ids.insert(stop.place_id.clone()) {
            let place = state
                .places
                .get(&(trip_id.to_string(), stop.place_id.clone()))
                .cloned()
                .ok_or(TripRepoError::CorruptData)?;
            places.push(place);
        }
    }
    let day_feasibility = days
        .iter()
        .map(|day| DayFeasibility {
            day_id: day.id.clone(),
            feasibility: Feasibility::Ok,
            used_min: 0,
            window_min: window_minutes(&day.window_start, &day.window_end).unwrap_or(0),
            notes: vec![],
        })
        .collect();
    Ok(PlanDetail {
        plan,
        days,
        stops,
        legs: vec![],
        day_feasibility,
        places,
    })
}

fn window_minutes(start: &str, end: &str) -> Option<u32> {
    fn minutes(value: &str) -> Option<u32> {
        let (hours, minutes) = value.split_once(':')?;
        Some(hours.parse::<u32>().ok()? * 60 + minutes.parse::<u32>().ok()?)
    }
    let start = minutes(start)?;
    let end = minutes(end)?;
    (end >= start).then_some(end - start)
}

#[async_trait]
impl TripRepo for TestTripRepo {
    async fn create_trip(&self, trip: Trip) -> Result<Trip, TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        if state.trips.contains_key(&trip.id) {
            return Err(TripRepoError::Conflict);
        }
        if trip.members.is_empty()
            || !trip
                .members
                .iter()
                .any(|member| member.role == TripRole::Leader)
        {
            return Err(TripRepoError::CorruptData);
        }
        state.trips.insert(trip.id.clone(), trip.clone());
        Ok(trip)
    }

    async fn list_trips(&self, actor: &UserId) -> Result<Vec<TripSummary>, TripRepoError> {
        let state = self.state.read().map_err(|_| TripRepoError::Unavailable)?;
        let mut trips = state
            .trips
            .values()
            .filter(|trip| trip.members.iter().any(|member| member.user_id == actor.0))
            .map(|trip| summary(&state, trip))
            .collect::<Vec<_>>();
        trips.sort_by(|a, b| {
            a.start_date
                .cmp(&b.start_date)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(trips)
    }

    async fn get_trip(&self, trip_id: &str, actor: &UserId) -> Result<Trip, TripRepoError> {
        let state = self.state.read().map_err(|_| TripRepoError::Unavailable)?;
        role(&state, trip_id, actor)?;
        state
            .trips
            .get(trip_id)
            .cloned()
            .ok_or(TripRepoError::NotFound)
    }

    async fn set_trip_status(
        &self,
        trip_id: &str,
        actor: &UserId,
        status: TripStatus,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Trip, TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor)?;
        let (old, updated) = {
            let trip = state
                .trips
                .get_mut(trip_id)
                .ok_or(TripRepoError::NotFound)?;
            let old = trip.status;
            if old != status {
                trip.status = status;
            }
            (old, trip.clone())
        };
        if old != status {
            let edit = Edit {
                id: change_id.to_string(),
                trip_id: trip_id.to_string(),
                entity: EditEntity::Trip,
                entity_id: trip_id.to_string(),
                field: "status".into(),
                old_value: serde_json::json!(old),
                new_value: serde_json::json!(status),
                author: actor.0.clone(),
                source: ChangeSource::Web,
                status: EditStatus::Applied,
                created_at: changed_at.to_string(),
                reverted_by: None,
                reverted_at: None,
                revert_edit_id: None,
                reverts_edit_id: None,
            };
            state
                .edits
                .insert((trip_id.to_string(), change_id.to_string()), edit);
        }
        Ok(updated)
    }

    async fn get_members(
        &self,
        trip_id: &str,
        actor: &UserId,
        users: &dyn UserRepo,
    ) -> Result<Vec<User>, TripRepoError> {
        let member_ids = {
            let state = self.state.read().map_err(|_| TripRepoError::Unavailable)?;
            role(&state, trip_id, actor)?;
            state
                .trips
                .get(trip_id)
                .ok_or(TripRepoError::NotFound)?
                .members
                .iter()
                .map(|member| UserId(member.user_id.clone()))
                .collect::<Vec<_>>()
        };
        let mut found = Vec::with_capacity(member_ids.len());
        for member_id in member_ids {
            found.push(
                users
                    .find_by_id(&member_id)
                    .await
                    .map_err(|error| match error {
                        UserRepoError::UserRepoUnavailable => TripRepoError::Unavailable,
                        UserRepoError::CorruptData | UserRepoError::DuplicateEmail(_) => {
                            TripRepoError::CorruptData
                        }
                    })?
                    .ok_or(TripRepoError::CorruptData)?,
            );
        }
        Ok(found)
    }

    async fn remove_member(
        &self,
        trip_id: &str,
        actor: &UserId,
        target: &UserId,
    ) -> Result<(), TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        require_leader(&state, trip_id, actor)?;
        let trip = state
            .trips
            .get_mut(trip_id)
            .ok_or(TripRepoError::NotFound)?;
        let target_index = trip
            .members
            .iter()
            .position(|member| member.user_id == target.0)
            .ok_or(TripRepoError::NotFound)?;
        if trip.members[target_index].role == TripRole::Leader
            && trip
                .members
                .iter()
                .filter(|member| member.role == TripRole::Leader)
                .count()
                == 1
        {
            return Err(TripRepoError::Conflict);
        }
        trip.members.remove(target_index);
        Ok(())
    }

    async fn create_invite(
        &self,
        trip_id: &str,
        actor: &UserId,
        invite: Invite,
    ) -> Result<Invite, TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        require_leader(&state, trip_id, actor)?;
        let key = (trip_id.to_string(), invite.email.clone());
        if state
            .invites
            .get(&key)
            .is_some_and(|existing| existing.status == InviteStatus::Pending)
        {
            return Err(TripRepoError::DuplicateInvite);
        }
        state.invites.insert(key, invite.clone());
        Ok(invite)
    }

    async fn accept_pending_invites(
        &self,
        user: &User,
        joined_at: &str,
    ) -> Result<(), TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        let email = user.email.to_string();
        let trip_ids = state
            .invites
            .iter()
            .filter(|((_, invite_email), invite)| {
                invite_email == &email && invite.status == InviteStatus::Pending
            })
            .map(|((trip_id, _), _)| trip_id.clone())
            .collect::<Vec<_>>();
        for trip_id in trip_ids {
            let trip = state
                .trips
                .get_mut(&trip_id)
                .ok_or(TripRepoError::CorruptData)?;
            if !trip
                .members
                .iter()
                .any(|member| member.user_id == user.id.0)
            {
                trip.members.push(itinera_core::domain::trip::TripMember {
                    user_id: user.id.0.clone(),
                    role: TripRole::Member,
                    joined_at: joined_at.to_string(),
                });
            }
            let invite = state
                .invites
                .get_mut(&(trip_id, email.clone()))
                .ok_or(TripRepoError::CorruptData)?;
            invite.status = InviteStatus::Accepted;
        }
        Ok(())
    }

    async fn search_saved_places(
        &self,
        trip_id: &str,
        actor: &UserId,
        query: &str,
    ) -> Result<Vec<Place>, TripRepoError> {
        let state = self.state.read().map_err(|_| TripRepoError::Unavailable)?;
        role(&state, trip_id, actor)?;
        let query = query.to_lowercase();
        let adopted_place_ids = state
            .stops
            .iter()
            .filter(|((stored_trip_id, _), _)| stored_trip_id == trip_id)
            .map(|(_, stop)| stop.place_id.as_str())
            .collect::<HashSet<_>>();
        Ok(state
            .places
            .iter()
            .filter(|((stored_trip_id, _), place)| {
                stored_trip_id == trip_id
                    && adopted_place_ids.contains(place.id.as_str())
                    && format!("{} {} {}", place.name, place.city, place.address)
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|(_, place)| place.clone())
            .collect())
    }

    async fn find_place(
        &self,
        trip_id: &str,
        actor: &UserId,
        place_id: &str,
    ) -> Result<Option<Place>, TripRepoError> {
        let state = self.state.read().map_err(|_| TripRepoError::Unavailable)?;
        role(&state, trip_id, actor)?;
        Ok(state
            .places
            .get(&(trip_id.to_string(), place_id.to_string()))
            .cloned())
    }

    async fn list_candidates(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<CandidateWithPlace>, TripRepoError> {
        let state = self.state.read().map_err(|_| TripRepoError::Unavailable)?;
        role(&state, trip_id, actor)?;
        let mut candidates = state
            .candidates
            .iter()
            .filter(|((stored_trip_id, _), _)| stored_trip_id == trip_id)
            .map(|(_, candidate)| {
                let place = state
                    .places
                    .get(&(trip_id.to_string(), candidate.place_id.clone()))
                    .cloned()
                    .ok_or(TripRepoError::CorruptData)?;
                Ok(CandidateWithPlace {
                    candidate: candidate.clone(),
                    place,
                })
            })
            .collect::<Result<Vec<_>, TripRepoError>>()?;
        candidates.sort_by(|a, b| a.candidate.created_at.cmp(&b.candidate.created_at));
        Ok(candidates)
    }

    async fn add_candidate(
        &self,
        trip_id: &str,
        actor: &UserId,
        candidate: Candidate,
        place: Place,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor)?;
        let candidate_key = (trip_id.to_string(), candidate.id.clone());
        let place_key = (trip_id.to_string(), place.id.clone());
        if state.candidates.contains_key(&candidate_key) || state.places.contains_key(&place_key) {
            return Err(TripRepoError::Conflict);
        }
        state.candidates.insert(candidate_key, candidate.clone());
        state.places.insert(place_key, place.clone());
        Ok(CandidateWithPlace { candidate, place })
    }

    async fn update_candidate(
        &self,
        trip_id: &str,
        actor: &UserId,
        candidate_id: &str,
        update: CandidateUpdate,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        let CandidateUpdate {
            place, pitch, tags, ..
        } = update;
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor)?;
        let place_key = (trip_id.to_string(), place.id.clone());
        if state.places.contains_key(&place_key) {
            return Err(TripRepoError::Conflict);
        }
        let candidate = state
            .candidates
            .get_mut(&(trip_id.to_string(), candidate_id.to_string()))
            .ok_or(TripRepoError::NotFound)?;
        candidate.place_id = place.id.clone();
        candidate.pitch = pitch;
        candidate.tags = tags;
        let candidate = candidate.clone();
        state.places.insert(place_key, place.clone());
        Ok(CandidateWithPlace { candidate, place })
    }

    async fn set_candidate_status(
        &self,
        trip_id: &str,
        actor: &UserId,
        candidate_id: &str,
        status: CandidateDisposition,
        _changed_at: &str,
        _change_id: &str,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor)?;
        let candidate = state
            .candidates
            .get_mut(&(trip_id.to_string(), candidate_id.to_string()))
            .ok_or(TripRepoError::NotFound)?;
        if candidate.status == CandidateStatus::InPlan {
            return Err(TripRepoError::Conflict);
        }
        candidate.status = status.into();
        let candidate = candidate.clone();
        let place = state
            .places
            .get(&(trip_id.to_string(), candidate.place_id.clone()))
            .cloned()
            .ok_or(TripRepoError::CorruptData)?;
        Ok(CandidateWithPlace { candidate, place })
    }

    async fn get_current_plan(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<PlanDetail, TripRepoError> {
        let state = self.state.read().map_err(|_| TripRepoError::Unavailable)?;
        role(&state, trip_id, actor)?;
        plan_detail(&state, trip_id)
    }

    async fn initialize_plan(
        &self,
        trip_id: &str,
        actor: &UserId,
        anchor_place_id: &str,
        plan: Plan,
        days: Vec<Day>,
    ) -> Result<PlanDetail, TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor)?;
        if state
            .trips
            .get(trip_id)
            .and_then(|trip| trip.current_plan_id.as_ref())
            .is_some()
        {
            return plan_detail(&state, trip_id);
        }
        let anchor_exists = state.candidates.values().any(|candidate| {
            candidate.trip_id == trip_id
                && candidate.place_id == anchor_place_id
                && candidate.status == CandidateStatus::Shortlisted
        });
        if !anchor_exists {
            return Err(TripRepoError::NotFound);
        }
        if state
            .plans
            .contains_key(&(trip_id.to_string(), plan.version))
        {
            return Err(TripRepoError::Conflict);
        }
        state
            .plans
            .insert((trip_id.to_string(), plan.version), plan.clone());
        for day in days {
            state
                .days
                .insert((trip_id.to_string(), day.id.clone()), day);
        }
        let trip = state
            .trips
            .get_mut(trip_id)
            .ok_or(TripRepoError::NotFound)?;
        trip.current_plan_id = Some(plan.id);
        trip.status = TripStatus::Planning;
        plan_detail(&state, trip_id)
    }

    async fn list_plan_versions(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Plan>, TripRepoError> {
        let state = self.state.read().map_err(|_| TripRepoError::Unavailable)?;
        role(&state, trip_id, actor)?;
        let mut plans = state
            .plans
            .iter()
            .filter(|((stored_trip_id, _), _)| stored_trip_id == trip_id)
            .map(|(_, plan)| plan.clone())
            .collect::<Vec<_>>();
        plans.sort_by_key(|plan| plan.version);
        Ok(plans)
    }

    async fn update_day(
        &self,
        trip_id: &str,
        actor: &UserId,
        day_id: &str,
        patch: DayPatch,
        _changed_at: &str,
        _change_id: &str,
    ) -> Result<Day, TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor)?;
        let current_plan_id = state
            .trips
            .get(trip_id)
            .and_then(|trip| trip.current_plan_id.clone())
            .ok_or(TripRepoError::NotFound)?;
        let day = state
            .days
            .get_mut(&(trip_id.to_string(), day_id.to_string()))
            .filter(|day| day.plan_id == current_plan_id)
            .ok_or(TripRepoError::NotFound)?;
        let mut updated = day.clone();
        if let Some(value) = patch.window_start {
            updated.window_start = value;
        }
        if let Some(value) = patch.window_end {
            updated.window_end = value;
        }
        if let Some(value) = patch.city_hint {
            updated.city_hint = value;
        }
        if !updated.window_is_ordered() {
            return Err(TripRepoError::Conflict);
        }
        *day = updated.clone();
        Ok(updated)
    }

    async fn update_stop(
        &self,
        trip_id: &str,
        actor: &UserId,
        stop_id: &str,
        patch: StopPatch,
        _changed_at: &str,
        _change_id: &str,
    ) -> Result<Stop, TripRepoError> {
        let mut state = self.state.write().map_err(|_| TripRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor)?;
        let current_plan_id = state
            .trips
            .get(trip_id)
            .and_then(|trip| trip.current_plan_id.clone())
            .ok_or(TripRepoError::NotFound)?;
        let current_day_ids = state
            .days
            .iter()
            .filter(|((stored_trip_id, _), day)| {
                stored_trip_id == trip_id && day.plan_id == current_plan_id
            })
            .map(|(_, day)| day.id.clone())
            .collect::<HashSet<_>>();
        let stop = state
            .stops
            .get_mut(&(trip_id.to_string(), stop_id.to_string()))
            .filter(|stop| current_day_ids.contains(&stop.day_id))
            .ok_or(TripRepoError::NotFound)?;
        if let Some(value) = patch.planned_arrival {
            stop.planned_arrival = value;
        }
        if let Some(value) = patch.duration_min {
            stop.duration_min = value;
        }
        if let Some(value) = patch.notes {
            stop.notes = value;
        }
        if let Some(value) = patch.booking {
            stop.booking = value;
        }
        Ok(stop.clone())
    }
}

#[async_trait]
impl ProposalRepo for TestTripRepo {
    async fn list_proposals(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Proposal>, ProposalRepoError> {
        let state = self
            .state
            .read()
            .map_err(|_| ProposalRepoError::Unavailable)?;
        role(&state, trip_id, actor).map_err(map_proposal_error)?;
        let mut proposals = state
            .proposals
            .iter()
            .filter(|((stored_trip_id, _), _)| stored_trip_id == trip_id)
            .map(|(_, proposal)| proposal.clone())
            .collect::<Vec<_>>();
        proposals.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(proposals)
    }

    async fn create_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal: Proposal,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ProposalRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_proposal_error)?;
        if proposal.route == ProposalRoute::Poll {
            return Err(ProposalRepoError::PollsUnavailable);
        }
        if proposal.trip_id != trip_id
            || proposal.created_by != actor.0
            || proposal.status != ProposalStatus::Pending
            || validate_stored_proposal(trip_id, &proposal).is_err()
        {
            return Err(ProposalRepoError::CorruptData);
        }
        let key = (trip_id.to_string(), proposal.id.clone());
        if state.proposals.contains_key(&key) {
            return Err(ProposalRepoError::Conflict);
        }
        let application = prepare_fake_application(
            &state,
            trip_id,
            &proposal,
            &proposal.created_at,
            application_ids,
        )?;
        if role(&state, trip_id, actor).map_err(map_proposal_error)? == TripRole::Leader {
            let mut applied = proposal;
            applied.status = ProposalStatus::Applied;
            applied.decided_by = Some(ProposalDecision::Leader {
                user_id: actor.0.clone(),
            });
            commit_fake_application(&mut state, trip_id, application)?;
            state.proposals.insert(key, applied.clone());
            Ok(applied)
        } else {
            state.proposals.insert(key, proposal.clone());
            Ok(proposal)
        }
    }

    async fn approve_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        applied_at: &str,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ProposalRepoError::Unavailable)?;
        require_leader(&state, trip_id, actor).map_err(map_proposal_error)?;
        let key = (trip_id.to_string(), proposal_id.to_string());
        let proposal = state
            .proposals
            .get(&key)
            .cloned()
            .ok_or(ProposalRepoError::NotFound)?;
        match proposal.status {
            ProposalStatus::Applied => return Ok(proposal),
            ProposalStatus::Pending => {}
            ProposalStatus::Rejected | ProposalStatus::Stale => {
                return Err(ProposalRepoError::Conflict);
            }
            ProposalStatus::Draft | ProposalStatus::Approved => {
                return Err(ProposalRepoError::CorruptData);
            }
        }
        let current = plan_detail(&state, trip_id).map_err(map_proposal_error)?;
        if current.plan.version != proposal.change_set.base_plan_version {
            let stale = state
                .proposals
                .get_mut(&key)
                .ok_or(ProposalRepoError::NotFound)?;
            stale.status = ProposalStatus::Stale;
            return Err(ProposalRepoError::Conflict);
        }
        let application =
            prepare_fake_application(&state, trip_id, &proposal, applied_at, application_ids)?;
        let mut applied = proposal;
        applied.status = ProposalStatus::Applied;
        applied.decided_by = Some(ProposalDecision::Leader {
            user_id: actor.0.clone(),
        });
        commit_fake_application(&mut state, trip_id, application)?;
        state.proposals.insert(key, applied.clone());
        Ok(applied)
    }

    async fn reject_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        reason: &str,
    ) -> Result<Proposal, ProposalRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ProposalRepoError::Unavailable)?;
        require_leader(&state, trip_id, actor).map_err(map_proposal_error)?;
        let proposal = state
            .proposals
            .get_mut(&(trip_id.to_string(), proposal_id.to_string()))
            .ok_or(ProposalRepoError::NotFound)?;
        match proposal.status {
            ProposalStatus::Rejected => return Ok(proposal.clone()),
            ProposalStatus::Pending => {}
            ProposalStatus::Applied | ProposalStatus::Stale => {
                return Err(ProposalRepoError::Conflict);
            }
            ProposalStatus::Draft | ProposalStatus::Approved => {
                return Err(ProposalRepoError::CorruptData);
            }
        }
        proposal.status = ProposalStatus::Rejected;
        proposal.decided_by = Some(ProposalDecision::Leader {
            user_id: actor.0.clone(),
        });
        proposal.rejection_reason = Some(reason.to_string());
        Ok(proposal.clone())
    }
}

fn prepare_fake_application(
    state: &State,
    trip_id: &str,
    proposal: &Proposal,
    applied_at: &str,
    application_ids: ProposalApplicationIds,
) -> Result<itinera_core::services::proposals::PlanApplication, ProposalRepoError> {
    let current = plan_detail(state, trip_id).map_err(map_proposal_error)?;
    if current.plan.version != proposal.change_set.base_plan_version {
        return Err(ProposalRepoError::Conflict);
    }
    let mut resolved = HashMap::new();
    for op in &proposal.change_set.ops {
        let place_id = match op {
            ChangeOp::AddStop { place_id, .. } => Some(place_id),
            ChangeOp::SwapPlace { new_place_id, .. } => Some(new_place_id),
            _ => None,
        };
        if let Some(place_id) = place_id {
            let place = state
                .places
                .get(&(trip_id.to_string(), place_id.clone()))
                .cloned()
                .ok_or(ProposalRepoError::NotFound)?;
            resolved.insert(place_id.clone(), place);
        }
    }
    let application = apply_change_set(
        &current,
        trip_id,
        &proposal.id,
        &proposal.change_set,
        &resolved,
        applied_at,
        application_ids,
    )
    .map_err(|error| match error {
        itinera_core::services::proposals::ChangeApplicationError::CorruptData => {
            ProposalRepoError::CorruptData
        }
        itinera_core::services::proposals::ChangeApplicationError::NotFound => {
            ProposalRepoError::NotFound
        }
        itinera_core::services::proposals::ChangeApplicationError::InvalidChange => {
            ProposalRepoError::InvalidChange
        }
    })?;
    let candidate_changes = state
        .candidates
        .iter()
        .filter(|((stored_trip_id, _), candidate)| {
            stored_trip_id == trip_id
                && candidate.status != CandidateStatus::Rejected
                && application
                    .stops
                    .iter()
                    .any(|stop| stop.place_id == candidate.place_id)
                    != (candidate.status == CandidateStatus::InPlan)
        })
        .count();
    let action_count = 5
        + current.days.len()
        + current.stops.len()
        + application.days.len()
        + application.stops.len()
        + application.new_places.len()
        + candidate_changes;
    if action_count > 100 {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    Ok(application)
}

fn commit_fake_application(
    state: &mut State,
    trip_id: &str,
    application: itinera_core::services::proposals::PlanApplication,
) -> Result<(), ProposalRepoError> {
    let resulting_places = application
        .stops
        .iter()
        .map(|stop| stop.place_id.clone())
        .collect::<HashSet<_>>();
    if state
        .candidates
        .iter()
        .any(|((stored_trip_id, _), candidate)| {
            stored_trip_id == trip_id
                && candidate.status == CandidateStatus::Rejected
                && resulting_places.contains(&candidate.place_id)
        })
    {
        return Err(ProposalRepoError::InvalidChange);
    }
    let current_plan_id = state
        .trips
        .get(trip_id)
        .and_then(|trip| trip.current_plan_id.clone())
        .ok_or(ProposalRepoError::CorruptData)?;
    let current_day_ids = state
        .days
        .iter()
        .filter(|((stored_trip_id, _), day)| {
            stored_trip_id == trip_id && day.plan_id == current_plan_id
        })
        .map(|(_, day)| day.id.clone())
        .collect::<HashSet<_>>();
    state.days.retain(|(stored_trip_id, _), day| {
        stored_trip_id != trip_id || day.plan_id != current_plan_id
    });
    state.stops.retain(|(stored_trip_id, _), stop| {
        stored_trip_id != trip_id || !current_day_ids.contains(&stop.day_id)
    });
    state.plans.insert(
        (trip_id.to_string(), application.plan.version),
        application.plan.clone(),
    );
    for day in application.days {
        state
            .days
            .insert((trip_id.to_string(), day.id.clone()), day);
    }
    for stop in application.stops {
        state
            .stops
            .insert((trip_id.to_string(), stop.id.clone()), stop);
    }
    for place in application.new_places {
        state
            .places
            .insert((trip_id.to_string(), place.id.clone()), place);
    }
    for ((stored_trip_id, _), candidate) in &mut state.candidates {
        if stored_trip_id != trip_id || candidate.status == CandidateStatus::Rejected {
            continue;
        }
        candidate.status = if resulting_places.contains(&candidate.place_id) {
            CandidateStatus::InPlan
        } else if candidate.status == CandidateStatus::InPlan {
            CandidateStatus::Shortlisted
        } else {
            candidate.status
        };
    }
    state
        .trips
        .get_mut(trip_id)
        .ok_or(ProposalRepoError::CorruptData)?
        .current_plan_id = Some(application.plan.id);
    Ok(())
}

fn map_proposal_error(error: TripRepoError) -> ProposalRepoError {
    match error {
        TripRepoError::Unavailable => ProposalRepoError::Unavailable,
        TripRepoError::CorruptData => ProposalRepoError::CorruptData,
        TripRepoError::NotFound => ProposalRepoError::NotFound,
        TripRepoError::Forbidden => ProposalRepoError::Forbidden,
        TripRepoError::Conflict | TripRepoError::DuplicateInvite => ProposalRepoError::Conflict,
    }
}

#[async_trait]
impl ContentHistoryRepo for TestTripRepo {
    async fn list_history(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Edit>, ContentHistoryRepoError> {
        let state = self
            .state
            .read()
            .map_err(|_| ContentHistoryRepoError::Unavailable)?;
        role(&state, trip_id, actor).map_err(map_history_error)?;
        let mut edits = state
            .edits
            .iter()
            .filter(|((stored_trip_id, _), edit)| {
                stored_trip_id == trip_id
                    && matches!(edit.status, EditStatus::Applied | EditStatus::Reverted)
            })
            .map(|(_, edit)| edit.clone())
            .collect::<Vec<_>>();
        edits.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(edits)
    }

    async fn revert_edit(
        &self,
        trip_id: &str,
        actor: &UserId,
        edit_id: &str,
        reverted_at: &str,
        compensating_edit_id: &str,
    ) -> Result<(), ContentHistoryRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ContentHistoryRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_history_error)?;
        let key = (trip_id.to_string(), edit_id.to_string());
        let original = state
            .edits
            .get(&key)
            .filter(|edit| matches!(edit.status, EditStatus::Applied | EditStatus::Reverted))
            .cloned()
            .ok_or(ContentHistoryRepoError::NotFound)?;
        if original.status == EditStatus::Reverted {
            return Ok(());
        }
        debug_assert_eq!(original.status, EditStatus::Applied);
        if original.entity != EditEntity::Trip
            || original.entity_id != trip_id
            || original.field != "status"
        {
            return Err(ContentHistoryRepoError::Unsupported);
        }
        let old: TripStatus = serde_json::from_value(original.old_value.clone())
            .map_err(|_| ContentHistoryRepoError::CorruptData)?;
        let new: TripStatus = serde_json::from_value(original.new_value.clone())
            .map_err(|_| ContentHistoryRepoError::CorruptData)?;
        if state
            .edits
            .contains_key(&(trip_id.to_string(), compensating_edit_id.to_string()))
        {
            return Err(ContentHistoryRepoError::Conflict);
        }
        {
            let trip = state
                .trips
                .get_mut(trip_id)
                .ok_or(ContentHistoryRepoError::CorruptData)?;
            if trip.status != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            trip.status = old;
        }
        let mut reverted = original.clone();
        reverted.status = EditStatus::Reverted;
        reverted.reverted_by = Some(actor.0.clone());
        reverted.reverted_at = Some(reverted_at.to_string());
        reverted.revert_edit_id = Some(compensating_edit_id.to_string());
        let compensation = Edit {
            id: compensating_edit_id.to_string(),
            trip_id: trip_id.to_string(),
            entity: original.entity,
            entity_id: original.entity_id,
            field: original.field,
            old_value: original.new_value,
            new_value: original.old_value,
            author: actor.0.clone(),
            source: ChangeSource::Web,
            status: EditStatus::Applied,
            created_at: reverted_at.to_string(),
            reverted_by: None,
            reverted_at: None,
            revert_edit_id: None,
            reverts_edit_id: Some(edit_id.to_string()),
        };
        state.edits.insert(key, reverted);
        state.edits.insert(
            (trip_id.to_string(), compensating_edit_id.to_string()),
            compensation,
        );
        Ok(())
    }
}

fn map_history_error(error: TripRepoError) -> ContentHistoryRepoError {
    match error {
        TripRepoError::Unavailable => ContentHistoryRepoError::Unavailable,
        TripRepoError::CorruptData => ContentHistoryRepoError::CorruptData,
        TripRepoError::NotFound => ContentHistoryRepoError::NotFound,
        TripRepoError::Forbidden => ContentHistoryRepoError::Forbidden,
        TripRepoError::Conflict | TripRepoError::DuplicateInvite => {
            ContentHistoryRepoError::Conflict
        }
    }
}
