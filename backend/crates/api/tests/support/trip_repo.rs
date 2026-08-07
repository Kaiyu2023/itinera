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
        discussion::{Comment, DiscussionThread, Reaction, ThreadAnchor},
        ledger::{Expense, Settlement},
        poll::{Poll, PollKind, PollOption, PollStatus, PollVote},
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
        discussion::{DiscussionRepo, DiscussionRepoError, NewComment, NewThread},
        ledger::{
            ExpenseReplacement, LedgerData, LedgerRepo, LedgerRepoError, LedgerTripContext,
            NewExpense, NewSettlement, VersionedExpense,
        },
        poll::{NewDecisionPoll, NewPlanChangePoll, PollRepo, PollRepoError},
        proposal::{ProposalApplicationIds, ProposalRepo, ProposalRepoError},
        trip::{CandidateUpdate, TripRepo, TripRepoError},
        user::{UserRepo, UserRepoError},
    },
    services::{
        ledger::expense_participant_ids,
        proposals::{apply_change_set, validate_stored_proposal},
    },
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
    polls: HashMap<(String, String), Poll>,
    poll_created_at: HashMap<(String, String), String>,
    discussion_threads: HashMap<(String, String), DiscussionThread>,
    discussion_anchors: HashMap<(String, ThreadAnchor), String>,
    discussion_comments: HashMap<(String, String, String), Comment>,
    expenses: HashMap<(String, String), Expense>,
    expense_revisions: HashMap<(String, String), u64>,
    settlements: HashMap<(String, String), Settlement>,
    ledger_operations: HashMap<(String, String), TestLedgerOperation>,
}

#[derive(Clone)]
struct TestLedgerOperation {
    actor_id: String,
    request_hash: String,
    result: TestLedgerOperationResult,
}

#[derive(Clone)]
enum TestLedgerOperationResult {
    Expense(Expense),
    Settlement(Settlement),
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
            let current_link = stop
                .booking
                .as_ref()
                .and_then(|booking| booking.ledger_entry_id.as_ref());
            let requested_link = value
                .as_ref()
                .and_then(|booking| booking.ledger_entry_id.as_ref());
            if current_link != requested_link {
                return Err(TripRepoError::CorruptData);
            }
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
            return Err(ProposalRepoError::Conflict);
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
        if proposal.route != ProposalRoute::LeaderApproval {
            return Err(ProposalRepoError::Conflict);
        }
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
        if proposal.route != ProposalRoute::LeaderApproval {
            return Err(ProposalRepoError::Conflict);
        }
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

#[async_trait]
impl PollRepo for TestTripRepo {
    async fn list_polls(&self, trip_id: &str, actor: &UserId) -> Result<Vec<Poll>, PollRepoError> {
        let state = self.state.read().map_err(|_| PollRepoError::Unavailable)?;
        role(&state, trip_id, actor).map_err(map_poll_error)?;
        let mut polls = state
            .polls
            .iter()
            .filter(|((stored_trip_id, _), _)| stored_trip_id == trip_id)
            .map(|(key, poll)| {
                (
                    state.poll_created_at.get(key).cloned().unwrap_or_default(),
                    poll.clone(),
                )
            })
            .collect::<Vec<_>>();
        polls.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.id.cmp(&left.1.id))
        });
        Ok(polls.into_iter().map(|(_, poll)| poll).collect())
    }

    async fn create_decision_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewDecisionPoll,
    ) -> Result<Poll, PollRepoError> {
        let mut state = self.state.write().map_err(|_| PollRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_poll_error)?;
        let quorum = fake_quorum(&state, trip_id)?;
        let poll = Poll {
            id: new.id,
            trip_id: trip_id.to_string(),
            created_by: actor.0.clone(),
            kind: PollKind::Decision,
            title: new.title,
            description: new.description,
            options: new.options,
            opens_at: None,
            closes_at: new.closes_at,
            decided_at: None,
            quorum,
            allow_multi: new.allow_multi,
            status: PollStatus::Open,
            votes: vec![],
            resolution_note: None,
        };
        insert_fake_poll(&mut state, poll.clone(), new.created_at)?;
        Ok(poll)
    }

    async fn create_proposal_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        mut proposal: Proposal,
        new: NewPlanChangePoll,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, PollRepoError> {
        let mut state = self.state.write().map_err(|_| PollRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_poll_error)?;
        if proposal.trip_id != trip_id
            || proposal.created_by != actor.0
            || proposal.route != ProposalRoute::Poll
            || proposal.status != ProposalStatus::Pending
            || proposal.decided_by.is_some()
            || proposal.created_at != new.created_at
        {
            return Err(PollRepoError::CorruptData);
        }
        let current = plan_detail(&state, trip_id).map_err(map_poll_error)?;
        if current.plan.version != proposal.change_set.base_plan_version {
            return Err(PollRepoError::Conflict);
        }
        proposal.decided_by = Some(ProposalDecision::Poll {
            poll_id: new.poll_id.clone(),
        });
        if validate_stored_proposal(trip_id, &proposal).is_err() {
            return Err(PollRepoError::CorruptData);
        }
        prepare_fake_application(&state, trip_id, &proposal, &new.created_at, application_ids)
            .map_err(map_poll_proposal_error)?;
        let poll = fake_plan_change_poll(&state, trip_id, actor, &proposal, &new)?;
        let proposal_key = (trip_id.to_string(), proposal.id.clone());
        if state.proposals.contains_key(&proposal_key) {
            return Err(PollRepoError::Conflict);
        }
        insert_fake_poll(&mut state, poll, new.created_at)?;
        state.proposals.insert(proposal_key, proposal.clone());
        Ok(proposal)
    }

    async fn route_proposal_to_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        new: NewPlanChangePoll,
        application_ids: ProposalApplicationIds,
    ) -> Result<Poll, PollRepoError> {
        let mut state = self.state.write().map_err(|_| PollRepoError::Unavailable)?;
        require_leader(&state, trip_id, actor).map_err(map_poll_error)?;
        let key = (trip_id.to_string(), proposal_id.to_string());
        let mut proposal = state
            .proposals
            .get(&key)
            .cloned()
            .ok_or(PollRepoError::NotFound)?;
        if let Some(ProposalDecision::Poll { poll_id }) = &proposal.decided_by {
            let prior = state
                .polls
                .get(&(trip_id.to_string(), poll_id.clone()))
                .cloned()
                .ok_or(PollRepoError::CorruptData)?;
            if proposal.status != ProposalStatus::Pending || !prior.status.is_terminal() {
                return Ok(prior);
            }
        } else if proposal.decided_by.is_some() {
            return Err(PollRepoError::Conflict);
        }
        if proposal.status != ProposalStatus::Pending {
            return Err(PollRepoError::Conflict);
        }
        let current = plan_detail(&state, trip_id).map_err(map_poll_error)?;
        if current.plan.version != proposal.change_set.base_plan_version {
            return Err(PollRepoError::Conflict);
        }
        proposal.route = ProposalRoute::Poll;
        proposal.decided_by = Some(ProposalDecision::Poll {
            poll_id: new.poll_id.clone(),
        });
        prepare_fake_application(&state, trip_id, &proposal, &new.created_at, application_ids)
            .map_err(map_poll_proposal_error)?;
        let poll = fake_plan_change_poll(&state, trip_id, actor, &proposal, &new)?;
        insert_fake_poll(&mut state, poll.clone(), new.created_at)?;
        state.proposals.insert(key, proposal);
        Ok(poll)
    }

    async fn open_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        poll_id: &str,
        _opened_at: &str,
    ) -> Result<Poll, PollRepoError> {
        let mut state = self.state.write().map_err(|_| PollRepoError::Unavailable)?;
        let actor_role = role(&state, trip_id, actor).map_err(map_poll_error)?;
        if !actor_role.can_edit() {
            return Err(PollRepoError::Forbidden);
        }
        let poll = state
            .polls
            .get_mut(&(trip_id.to_string(), poll_id.to_string()))
            .ok_or(PollRepoError::NotFound)?;
        if actor_role != TripRole::Leader && poll.created_by != actor.0 {
            return Err(PollRepoError::Forbidden);
        }
        match poll.status {
            PollStatus::Open => return Ok(poll.clone()),
            PollStatus::Draft | PollStatus::Scheduled => {}
            PollStatus::Passed | PollStatus::Failed | PollStatus::Expired => {
                return Err(PollRepoError::Conflict);
            }
        }
        poll.status = PollStatus::Open;
        poll.opens_at = None;
        Ok(poll.clone())
    }

    async fn cast_vote(
        &self,
        trip_id: &str,
        actor: &UserId,
        poll_id: &str,
        option_ids: &[String],
        voted_at: &str,
    ) -> Result<Poll, PollRepoError> {
        let mut state = self.state.write().map_err(|_| PollRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_poll_error)?;
        let poll = state
            .polls
            .get_mut(&(trip_id.to_string(), poll_id.to_string()))
            .ok_or(PollRepoError::NotFound)?;
        let available = poll
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<HashSet<_>>();
        let mut requested = HashSet::new();
        if option_ids.len() > poll.options.len()
            || (!poll.allow_multi && option_ids.len() != 1)
            || option_ids
                .iter()
                .any(|id| !available.contains(id.as_str()) || !requested.insert(id))
        {
            return Err(PollRepoError::InvalidVote);
        }
        let mut current = poll
            .votes
            .iter()
            .filter(|vote| vote.user_id == actor.0)
            .map(|vote| vote.option_id.clone())
            .collect::<Vec<_>>();
        let mut desired = option_ids.to_vec();
        current.sort();
        desired.sort();
        if current == desired {
            return Ok(poll.clone());
        }
        if poll.status != PollStatus::Open {
            return Err(PollRepoError::Conflict);
        }
        poll.votes.retain(|vote| vote.user_id != actor.0);
        poll.votes
            .extend(desired.into_iter().map(|option_id| PollVote {
                user_id: actor.0.clone(),
                option_id,
                at: voted_at.to_string(),
            }));
        Ok(poll.clone())
    }

    async fn close_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        poll_id: &str,
        decided_at: &str,
        application_ids: ProposalApplicationIds,
    ) -> Result<Poll, PollRepoError> {
        let mut state = self.state.write().map_err(|_| PollRepoError::Unavailable)?;
        require_leader(&state, trip_id, actor).map_err(map_poll_error)?;
        let key = (trip_id.to_string(), poll_id.to_string());
        let mut poll = state
            .polls
            .get(&key)
            .cloned()
            .ok_or(PollRepoError::NotFound)?;
        if poll.status.is_terminal() {
            return Ok(poll);
        }
        if poll.status != PollStatus::Open {
            return Err(PollRepoError::Conflict);
        }
        poll.decided_at = Some(decided_at.to_string());
        let voters = poll
            .votes
            .iter()
            .map(|vote| vote.user_id.as_str())
            .collect::<HashSet<_>>()
            .len() as u32;
        if voters < poll.quorum {
            poll.status = PollStatus::Expired;
            poll.resolution_note = Some("Closed below quorum - no decision recorded.".into());
            state.polls.insert(key, poll.clone());
            return Ok(poll);
        }
        let mut counts = poll
            .options
            .iter()
            .map(|option| (option.id.clone(), 0_u32))
            .collect::<HashMap<_, _>>();
        for vote in &poll.votes {
            *counts
                .get_mut(&vote.option_id)
                .ok_or(PollRepoError::CorruptData)? += 1;
        }
        let top = counts.values().copied().max().unwrap_or(0);
        let winners = poll
            .options
            .iter()
            .filter(|option| counts.get(&option.id) == Some(&top))
            .cloned()
            .collect::<Vec<_>>();
        if top == 0 || winners.len() != 1 {
            poll.status = PollStatus::Failed;
            poll.resolution_note = Some("Closed with a tied result - no decision recorded.".into());
        } else if top.saturating_mul(2) <= voters {
            poll.status = PollStatus::Failed;
            poll.resolution_note =
                Some("No option reached a majority - no decision recorded.".into());
        } else if poll.kind == PollKind::Decision {
            poll.status = PollStatus::Passed;
        } else if let Some(proposal_id) = &winners[0].proposal_id {
            let proposal_key = (trip_id.to_string(), proposal_id.clone());
            let mut proposal = state
                .proposals
                .get(&proposal_key)
                .cloned()
                .ok_or(PollRepoError::CorruptData)?;
            let current = plan_detail(&state, trip_id).map_err(map_poll_error)?;
            if current.plan.version != proposal.change_set.base_plan_version {
                proposal.status = ProposalStatus::Stale;
                poll.status = PollStatus::Failed;
                poll.resolution_note = Some(
                    "The winning proposal was based on an outdated plan - no plan change was applied."
                        .into(),
                );
            } else {
                let application = prepare_fake_application(
                    &state,
                    trip_id,
                    &proposal,
                    decided_at,
                    application_ids,
                )
                .map_err(map_poll_proposal_error)?;
                commit_fake_application(&mut state, trip_id, application)
                    .map_err(map_poll_proposal_error)?;
                proposal.status = ProposalStatus::Applied;
                poll.status = PollStatus::Passed;
            }
            state.proposals.insert(proposal_key, proposal);
        } else {
            let proposal_id = poll
                .options
                .iter()
                .find_map(|option| option.proposal_id.clone())
                .ok_or(PollRepoError::CorruptData)?;
            let proposal = state
                .proposals
                .get_mut(&(trip_id.to_string(), proposal_id))
                .ok_or(PollRepoError::CorruptData)?;
            proposal.status = ProposalStatus::Rejected;
            proposal.rejection_reason = Some("The group chose to keep the current plan.".into());
            poll.status = PollStatus::Failed;
            poll.resolution_note = Some("The group chose to keep the current plan.".into());
        }
        state.polls.insert(key, poll.clone());
        Ok(poll)
    }
}

fn fake_quorum(state: &State, trip_id: &str) -> Result<u32, PollRepoError> {
    let eligible = state
        .trips
        .get(trip_id)
        .ok_or(PollRepoError::NotFound)?
        .members
        .iter()
        .filter(|member| member.role.can_edit())
        .count();
    let eligible = u32::try_from(eligible).map_err(|_| PollRepoError::CorruptData)?;
    if eligible == 0 {
        return Err(PollRepoError::CorruptData);
    }
    Ok(eligible.div_ceil(2))
}

fn fake_plan_change_poll(
    state: &State,
    trip_id: &str,
    actor: &UserId,
    proposal: &Proposal,
    new: &NewPlanChangePoll,
) -> Result<Poll, PollRepoError> {
    Ok(Poll {
        id: new.poll_id.clone(),
        trip_id: trip_id.to_string(),
        created_by: actor.0.clone(),
        kind: PollKind::PlanChange,
        title: proposal.title.clone(),
        description: proposal.rationale.clone(),
        options: vec![
            PollOption {
                id: new.adopt_option_id.clone(),
                label: "Adopt the proposed plan change".into(),
                proposal_id: Some(proposal.id.clone()),
            },
            PollOption {
                id: new.keep_option_id.clone(),
                label: "Keep the current plan".into(),
                proposal_id: None,
            },
        ],
        opens_at: None,
        closes_at: new.closes_at.clone(),
        decided_at: None,
        quorum: fake_quorum(state, trip_id)?,
        allow_multi: false,
        status: PollStatus::Open,
        votes: vec![],
        resolution_note: None,
    })
}

fn insert_fake_poll(
    state: &mut State,
    poll: Poll,
    created_at: String,
) -> Result<(), PollRepoError> {
    let key = (poll.trip_id.clone(), poll.id.clone());
    if state.polls.contains_key(&key) {
        return Err(PollRepoError::Conflict);
    }
    state.poll_created_at.insert(key.clone(), created_at);
    state.polls.insert(key, poll);
    Ok(())
}

fn map_poll_error(error: TripRepoError) -> PollRepoError {
    match error {
        TripRepoError::Unavailable => PollRepoError::Unavailable,
        TripRepoError::CorruptData => PollRepoError::CorruptData,
        TripRepoError::NotFound => PollRepoError::NotFound,
        TripRepoError::Forbidden => PollRepoError::Forbidden,
        TripRepoError::Conflict | TripRepoError::DuplicateInvite => PollRepoError::Conflict,
    }
}

fn map_poll_proposal_error(error: ProposalRepoError) -> PollRepoError {
    match error {
        ProposalRepoError::Unavailable => PollRepoError::Unavailable,
        ProposalRepoError::CorruptData => PollRepoError::CorruptData,
        ProposalRepoError::NotFound => PollRepoError::NotFound,
        ProposalRepoError::Forbidden => PollRepoError::Forbidden,
        ProposalRepoError::Conflict | ProposalRepoError::InvalidChange => PollRepoError::Conflict,
        ProposalRepoError::SafetyLimitExceeded => PollRepoError::SafetyLimitExceeded,
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

fn validate_fake_discussion_anchor(
    state: &State,
    trip_id: &str,
    anchor: &ThreadAnchor,
) -> Result<(), DiscussionRepoError> {
    let trip = state
        .trips
        .get(trip_id)
        .ok_or(DiscussionRepoError::NotFound)?;
    match anchor {
        ThreadAnchor::Trip => Ok(()),
        ThreadAnchor::Candidate { candidate_id } => state
            .candidates
            .contains_key(&(trip_id.to_string(), candidate_id.clone()))
            .then_some(())
            .ok_or(DiscussionRepoError::NotFound),
        ThreadAnchor::Poll { poll_id } => state
            .polls
            .contains_key(&(trip_id.to_string(), poll_id.clone()))
            .then_some(())
            .ok_or(DiscussionRepoError::NotFound),
        ThreadAnchor::Day { day_id } => {
            let current_plan_id = trip
                .current_plan_id
                .as_deref()
                .ok_or(DiscussionRepoError::NotFound)?;
            state
                .days
                .get(&(trip_id.to_string(), day_id.clone()))
                .filter(|day| day.plan_id == current_plan_id)
                .map(|_| ())
                .ok_or(DiscussionRepoError::NotFound)
        }
        ThreadAnchor::Stop { stop_id } => {
            let current_plan_id = trip
                .current_plan_id
                .as_deref()
                .ok_or(DiscussionRepoError::NotFound)?;
            let stop = state
                .stops
                .get(&(trip_id.to_string(), stop_id.clone()))
                .ok_or(DiscussionRepoError::NotFound)?;
            state
                .days
                .get(&(trip_id.to_string(), stop.day_id.clone()))
                .filter(|day| day.plan_id == current_plan_id)
                .map(|_| ())
                .ok_or(DiscussionRepoError::NotFound)
        }
    }
}

#[async_trait]
impl DiscussionRepo for TestTripRepo {
    async fn list_threads(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<DiscussionThread>, DiscussionRepoError> {
        let state = self
            .state
            .read()
            .map_err(|_| DiscussionRepoError::Unavailable)?;
        role(&state, trip_id, actor).map_err(map_discussion_error)?;
        let mut threads = state
            .discussion_threads
            .iter()
            .filter(|((stored_trip_id, _), _)| stored_trip_id == trip_id)
            .map(|(_, thread)| thread.clone())
            .collect::<Vec<_>>();
        threads.sort_by(|left, right| {
            right
                .last_activity_at
                .cmp(&left.last_activity_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(threads)
    }

    async fn create_thread(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewThread,
    ) -> Result<DiscussionThread, DiscussionRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| DiscussionRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_discussion_error)?;
        validate_fake_discussion_anchor(&state, trip_id, &new.anchor)?;
        let thread_key = (trip_id.to_string(), new.id.clone());
        let anchor_key = (trip_id.to_string(), new.anchor.clone());
        let comment_key = (
            trip_id.to_string(),
            new.id.clone(),
            new.first_comment_id.clone(),
        );
        if state.discussion_threads.contains_key(&thread_key)
            || state.discussion_anchors.contains_key(&anchor_key)
            || state.discussion_comments.contains_key(&comment_key)
        {
            return Err(DiscussionRepoError::Conflict);
        }
        let thread = DiscussionThread {
            id: new.id,
            trip_id: trip_id.to_string(),
            anchor: new.anchor,
            title: new.title,
            comment_count: 1,
            last_activity_at: new.created_at.clone(),
        };
        let comment = Comment {
            id: new.first_comment_id,
            thread_id: thread.id.clone(),
            author: actor.0.clone(),
            body: new.body,
            created_at: new.created_at,
            reactions: vec![],
        };
        state
            .discussion_anchors
            .insert(anchor_key, thread.id.clone());
        state.discussion_threads.insert(thread_key, thread.clone());
        state.discussion_comments.insert(comment_key, comment);
        Ok(thread)
    }

    async fn get_comments(
        &self,
        trip_id: &str,
        actor: &UserId,
        thread_id: &str,
    ) -> Result<Vec<Comment>, DiscussionRepoError> {
        let state = self
            .state
            .read()
            .map_err(|_| DiscussionRepoError::Unavailable)?;
        role(&state, trip_id, actor).map_err(map_discussion_error)?;
        let thread = state
            .discussion_threads
            .get(&(trip_id.to_string(), thread_id.to_string()))
            .ok_or(DiscussionRepoError::NotFound)?;
        let mut comments = state
            .discussion_comments
            .iter()
            .filter(|((stored_trip_id, stored_thread_id, _), _)| {
                stored_trip_id == trip_id && stored_thread_id == thread_id
            })
            .map(|(_, comment)| comment.clone())
            .collect::<Vec<_>>();
        if comments.len() != thread.comment_count as usize {
            return Err(DiscussionRepoError::CorruptData);
        }
        comments.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(comments)
    }

    async fn add_comment(
        &self,
        trip_id: &str,
        actor: &UserId,
        thread_id: &str,
        new: NewComment,
    ) -> Result<Comment, DiscussionRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| DiscussionRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_discussion_error)?;
        let thread_key = (trip_id.to_string(), thread_id.to_string());
        let loaded = state
            .discussion_threads
            .get(&thread_key)
            .cloned()
            .ok_or(DiscussionRepoError::NotFound)?;
        let comment_key = (trip_id.to_string(), thread_id.to_string(), new.id.clone());
        let comment = Comment {
            id: new.id,
            thread_id: thread_id.to_string(),
            author: actor.0.clone(),
            body: new.body,
            created_at: new.created_at,
            reactions: vec![],
        };
        if let Some(existing) = state.discussion_comments.get(&comment_key) {
            return if existing == &comment {
                Ok(existing.clone())
            } else {
                Err(DiscussionRepoError::CorruptData)
            };
        }
        if loaded.comment_count >= 1_000 {
            return Err(DiscussionRepoError::SafetyLimitExceeded);
        }
        if comment.created_at < loaded.last_activity_at {
            return Err(DiscussionRepoError::Conflict);
        }
        let mut updated = loaded;
        updated.comment_count += 1;
        updated.last_activity_at.clone_from(&comment.created_at);
        state.discussion_threads.insert(thread_key, updated);
        state
            .discussion_comments
            .insert(comment_key, comment.clone());
        Ok(comment)
    }

    async fn set_reaction(
        &self,
        trip_id: &str,
        actor: &UserId,
        thread_id: &str,
        comment_id: &str,
        emoji: &str,
        active: bool,
    ) -> Result<Comment, DiscussionRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| DiscussionRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_discussion_error)?;
        if !state
            .discussion_threads
            .contains_key(&(trip_id.to_string(), thread_id.to_string()))
        {
            return Err(DiscussionRepoError::NotFound);
        }
        let comment = state
            .discussion_comments
            .get_mut(&(
                trip_id.to_string(),
                thread_id.to_string(),
                comment_id.to_string(),
            ))
            .ok_or(DiscussionRepoError::NotFound)?;
        let position = comment
            .reactions
            .iter()
            .position(|reaction| reaction.emoji == emoji);
        let currently_active = position.is_some_and(|index| {
            comment.reactions[index]
                .user_ids
                .iter()
                .any(|user_id| user_id == &actor.0)
        });
        if currently_active == active {
            return Ok(comment.clone());
        }
        if let Some(index) = position {
            let reaction = &mut comment.reactions[index];
            reaction.user_ids.retain(|user_id| user_id != &actor.0);
            if active {
                if reaction.user_ids.len() >= 1_000 {
                    return Err(DiscussionRepoError::SafetyLimitExceeded);
                }
                reaction.user_ids.push(actor.0.clone());
                reaction.user_ids.sort();
            }
        } else if active {
            comment.reactions.push(Reaction {
                emoji: emoji.to_string(),
                user_ids: vec![actor.0.clone()],
            });
        }
        comment
            .reactions
            .retain(|reaction| !reaction.user_ids.is_empty());
        comment
            .reactions
            .sort_by(|left, right| left.emoji.cmp(&right.emoji));
        Ok(comment.clone())
    }
}

fn map_discussion_error(error: TripRepoError) -> DiscussionRepoError {
    match error {
        TripRepoError::Unavailable => DiscussionRepoError::Unavailable,
        TripRepoError::CorruptData => DiscussionRepoError::CorruptData,
        TripRepoError::NotFound => DiscussionRepoError::NotFound,
        TripRepoError::Forbidden => DiscussionRepoError::Forbidden,
        TripRepoError::Conflict | TripRepoError::DuplicateInvite => DiscussionRepoError::Conflict,
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

#[async_trait]
impl LedgerRepo for TestTripRepo {
    async fn get_ledger_data(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<LedgerData, LedgerRepoError> {
        let state = self
            .state
            .read()
            .map_err(|_| LedgerRepoError::Unavailable)?;
        role(&state, trip_id, actor).map_err(map_ledger_error)?;
        let trip = state.trips.get(trip_id).ok_or(LedgerRepoError::NotFound)?;
        let mut expenses = state
            .expenses
            .iter()
            .filter(|((stored_trip_id, _), _)| stored_trip_id == trip_id)
            .map(|(_, expense)| expense.clone())
            .collect::<Vec<_>>();
        expenses.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut settlements = state
            .settlements
            .iter()
            .filter(|((stored_trip_id, _), _)| stored_trip_id == trip_id)
            .map(|(_, settlement)| settlement.clone())
            .collect::<Vec<_>>();
        settlements.sort_by(|left, right| {
            left.settled_at
                .cmp(&right.settled_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(LedgerData {
            base_currency: trip.base_currency.clone(),
            current_member_ids: trip
                .members
                .iter()
                .map(|member| member.user_id.clone())
                .collect(),
            expenses,
            settlements,
        })
    }

    async fn get_trip_context(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<LedgerTripContext, LedgerRepoError> {
        let state = self
            .state
            .read()
            .map_err(|_| LedgerRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_ledger_error)?;
        let trip = state.trips.get(trip_id).ok_or(LedgerRepoError::NotFound)?;
        Ok(test_ledger_context(trip))
    }

    async fn replay_expense_creation(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<Option<Expense>, LedgerRepoError> {
        let state = self
            .state
            .read()
            .map_err(|_| LedgerRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_ledger_error)?;
        match replay_test_ledger_operation(&state, trip_id, actor, idempotency_key, request_hash)? {
            None => Ok(None),
            Some(TestLedgerOperationResult::Expense(expense)) => Ok(Some(expense)),
            Some(TestLedgerOperationResult::Settlement(_)) => Err(LedgerRepoError::Conflict),
        }
    }

    async fn replay_settlement_creation(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<Option<Settlement>, LedgerRepoError> {
        let state = self
            .state
            .read()
            .map_err(|_| LedgerRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_ledger_error)?;
        match replay_test_ledger_operation(&state, trip_id, actor, idempotency_key, request_hash)? {
            None => Ok(None),
            Some(TestLedgerOperationResult::Settlement(settlement)) => Ok(Some(settlement)),
            Some(TestLedgerOperationResult::Expense(_)) => Err(LedgerRepoError::Conflict),
        }
    }

    async fn get_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        expense_id: &str,
    ) -> Result<VersionedExpense, LedgerRepoError> {
        let state = self
            .state
            .read()
            .map_err(|_| LedgerRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_ledger_error)?;
        let trip = state.trips.get(trip_id).ok_or(LedgerRepoError::NotFound)?;
        let key = (trip_id.to_string(), expense_id.to_string());
        Ok(VersionedExpense {
            expense: state
                .expenses
                .get(&key)
                .cloned()
                .ok_or(LedgerRepoError::NotFound)?,
            revision: *state
                .expense_revisions
                .get(&key)
                .ok_or(LedgerRepoError::CorruptData)?,
            context: test_ledger_context(trip),
        })
    }

    async fn add_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewExpense,
    ) -> Result<Expense, LedgerRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| LedgerRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_ledger_error)?;
        if let Some(result) = replay_test_ledger_operation(
            &state,
            trip_id,
            actor,
            &new.idempotency_key,
            &new.request_hash,
        )? {
            return match result {
                TestLedgerOperationResult::Expense(expense) => Ok(expense),
                TestLedgerOperationResult::Settlement(_) => Err(LedgerRepoError::Conflict),
            };
        }
        validate_test_context(&state, trip_id, &new.context)?;
        require_expense_members(&state, trip_id, &new.expense)?;
        let key = (trip_id.to_string(), new.expense.id.clone());
        if state.expenses.contains_key(&key) {
            return Err(LedgerRepoError::Conflict);
        }
        if let Some(stop_id) = new.expense.linked_stop_id.as_deref() {
            ensure_unused_stop_link(&state, trip_id, stop_id, None)?;
            set_test_stop_link(&mut state, trip_id, stop_id, &new.expense.id, true)?;
        }
        state.expense_revisions.insert(key.clone(), 1);
        state.expenses.insert(key, new.expense.clone());
        state.ledger_operations.insert(
            (trip_id.to_string(), new.idempotency_key),
            TestLedgerOperation {
                actor_id: actor.0.clone(),
                request_hash: new.request_hash,
                result: TestLedgerOperationResult::Expense(new.expense.clone()),
            },
        );
        Ok(new.expense)
    }

    async fn replace_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        replacement: ExpenseReplacement,
    ) -> Result<Expense, LedgerRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| LedgerRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_ledger_error)?;
        validate_test_context(&state, trip_id, &replacement.context)?;
        require_expense_members(&state, trip_id, &replacement.expense)?;
        let key = (trip_id.to_string(), replacement.expense.id.clone());
        let current = state
            .expenses
            .get(&key)
            .cloned()
            .ok_or(LedgerRepoError::NotFound)?;
        let revision = *state
            .expense_revisions
            .get(&key)
            .ok_or(LedgerRepoError::CorruptData)?;
        if revision != replacement.expected_revision {
            return Err(LedgerRepoError::Conflict);
        }
        if current.linked_stop_id != replacement.expense.linked_stop_id {
            if let Some(stop_id) = replacement.expense.linked_stop_id.as_deref() {
                ensure_unused_stop_link(&state, trip_id, stop_id, Some(&current.id))?;
            }
            if let Some(stop_id) = current.linked_stop_id.as_deref() {
                set_test_stop_link(&mut state, trip_id, stop_id, &current.id, false)?;
            }
            if let Some(stop_id) = replacement.expense.linked_stop_id.as_deref() {
                set_test_stop_link(&mut state, trip_id, stop_id, &replacement.expense.id, true)?;
            }
        }
        state
            .expenses
            .insert(key.clone(), replacement.expense.clone());
        state.expense_revisions.insert(
            key,
            revision
                .checked_add(1)
                .ok_or(LedgerRepoError::CorruptData)?,
        );
        Ok(replacement.expense)
    }

    async fn delete_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        expense_id: &str,
        _audit_id: &str,
        _audit_at: &str,
    ) -> Result<(), LedgerRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| LedgerRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_ledger_error)?;
        let key = (trip_id.to_string(), expense_id.to_string());
        let expense = state
            .expenses
            .get(&key)
            .cloned()
            .ok_or(LedgerRepoError::NotFound)?;
        if let Some(stop_id) = expense.linked_stop_id.as_deref() {
            set_test_stop_link(&mut state, trip_id, stop_id, expense_id, false)?;
        }
        state.expenses.remove(&key);
        state.expense_revisions.remove(&key);
        Ok(())
    }

    async fn add_settlement(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewSettlement,
    ) -> Result<Settlement, LedgerRepoError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| LedgerRepoError::Unavailable)?;
        require_editor(&state, trip_id, actor).map_err(map_ledger_error)?;
        if let Some(result) = replay_test_ledger_operation(
            &state,
            trip_id,
            actor,
            &new.idempotency_key,
            &new.request_hash,
        )? {
            return match result {
                TestLedgerOperationResult::Settlement(settlement) => Ok(settlement),
                TestLedgerOperationResult::Expense(_) => Err(LedgerRepoError::Conflict),
            };
        }
        require_test_members(
            &state,
            trip_id,
            [
                new.settlement.from_user.as_str(),
                new.settlement.to_user.as_str(),
            ],
        )?;
        let key = (trip_id.to_string(), new.settlement.id.clone());
        if state.settlements.contains_key(&key) {
            return Err(LedgerRepoError::Conflict);
        }
        state.settlements.insert(key, new.settlement.clone());
        state.ledger_operations.insert(
            (trip_id.to_string(), new.idempotency_key),
            TestLedgerOperation {
                actor_id: actor.0.clone(),
                request_hash: new.request_hash,
                result: TestLedgerOperationResult::Settlement(new.settlement.clone()),
            },
        );
        Ok(new.settlement)
    }
}

fn test_ledger_context(trip: &Trip) -> LedgerTripContext {
    LedgerTripContext {
        base_currency: trip.base_currency.clone(),
        trip_revision: 1,
    }
}

fn replay_test_ledger_operation(
    state: &State,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<Option<TestLedgerOperationResult>, LedgerRepoError> {
    let Some(operation) = state
        .ledger_operations
        .get(&(trip_id.to_string(), idempotency_key.to_string()))
    else {
        return Ok(None);
    };
    if operation.actor_id != actor.0 || operation.request_hash != request_hash {
        return Err(LedgerRepoError::Conflict);
    }
    Ok(Some(operation.result.clone()))
}

fn validate_test_context(
    state: &State,
    trip_id: &str,
    context: &LedgerTripContext,
) -> Result<(), LedgerRepoError> {
    let trip = state.trips.get(trip_id).ok_or(LedgerRepoError::NotFound)?;
    if &test_ledger_context(trip) == context {
        Ok(())
    } else {
        Err(LedgerRepoError::Conflict)
    }
}

fn require_expense_members(
    state: &State,
    trip_id: &str,
    expense: &Expense,
) -> Result<(), LedgerRepoError> {
    let mut ids = expense_participant_ids(&expense.split);
    ids.push(&expense.paid_by);
    require_test_members(state, trip_id, ids)
}

fn require_test_members<'a>(
    state: &State,
    trip_id: &str,
    user_ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), LedgerRepoError> {
    let trip = state.trips.get(trip_id).ok_or(LedgerRepoError::NotFound)?;
    for user_id in user_ids {
        if !trip.members.iter().any(|member| member.user_id == user_id) {
            return Err(LedgerRepoError::Conflict);
        }
    }
    Ok(())
}

fn ensure_unused_stop_link(
    state: &State,
    trip_id: &str,
    stop_id: &str,
    current_expense_id: Option<&str>,
) -> Result<(), LedgerRepoError> {
    if state
        .expenses
        .iter()
        .any(|((stored_trip_id, expense_id), expense)| {
            stored_trip_id == trip_id
                && Some(expense_id.as_str()) != current_expense_id
                && expense.linked_stop_id.as_deref() == Some(stop_id)
        })
    {
        Err(LedgerRepoError::Conflict)
    } else {
        Ok(())
    }
}

fn set_test_stop_link(
    state: &mut State,
    trip_id: &str,
    stop_id: &str,
    expense_id: &str,
    linked: bool,
) -> Result<(), LedgerRepoError> {
    let trip = state.trips.get(trip_id).ok_or(LedgerRepoError::NotFound)?;
    let current_plan_id = trip
        .current_plan_id
        .as_deref()
        .ok_or(LedgerRepoError::NotFound)?;
    let stop = state
        .stops
        .get(&(trip_id.to_string(), stop_id.to_string()))
        .ok_or(LedgerRepoError::NotFound)?;
    let day_is_current = state.days.iter().any(|((stored_trip_id, _), day)| {
        stored_trip_id == trip_id && day.id == stop.day_id && day.plan_id == current_plan_id
    });
    if !day_is_current {
        return Err(LedgerRepoError::NotFound);
    }
    let stop = state
        .stops
        .get_mut(&(trip_id.to_string(), stop_id.to_string()))
        .ok_or(LedgerRepoError::NotFound)?;
    if let Some(booking) = stop.booking.as_mut() {
        if linked {
            if booking.ledger_entry_id.is_some() {
                return Err(LedgerRepoError::Conflict);
            }
            booking.ledger_entry_id = Some(expense_id.to_string());
        } else if booking.ledger_entry_id.as_deref() == Some(expense_id) {
            booking.ledger_entry_id = None;
        } else if booking.ledger_entry_id.is_some() {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    Ok(())
}

fn map_ledger_error(error: TripRepoError) -> LedgerRepoError {
    match error {
        TripRepoError::Unavailable => LedgerRepoError::Unavailable,
        TripRepoError::CorruptData => LedgerRepoError::CorruptData,
        TripRepoError::NotFound => LedgerRepoError::NotFound,
        TripRepoError::Forbidden => LedgerRepoError::Forbidden,
        TripRepoError::Conflict | TripRepoError::DuplicateInvite => LedgerRepoError::Conflict,
    }
}
