//! Plan, day, and stop persistence operations.

use std::collections::HashSet;

use itinera_core::{
    domain::{
        trip::{
            Candidate, CandidateStatus, Day, DayFeasibility, DayPatch, Feasibility, Plan,
            PlanDetail, Stop, StopPatch, TripStatus,
        },
        user::UserId,
    },
    ports::trip::TripRepoError,
};
use serde_json::json;

use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE, SK,
    history_repo::reservation::reserve_history_append,
    ledger_repo::records::{
        Loaded as LedgerLoaded, STOP_LINK_ENTITY, StopLinkRecord, decode_stop_link, stop_link_sk,
    },
    primitives::{condition_action, put_action, transaction_condition_failed},
};

use super::{
    audit::{AuditChange, audit, suffixed_id},
    history_reservation_error,
    records::{
        AUDIT_ENTITY, CANDIDATE_ENTITY, DAY_ENTITY, META_SK, PLAN_ENTITY, STOP_ENTITY, Stored,
        TRIP_COLLECTION_PAGE_SIZE, TRIP_ENTITY, TripMeta, audit_sk, day_sk, decode_record,
        encode_record, encode_trip_meta, plan_prefix, plan_sk, string, trip_pk,
    },
    store::RequiredRole,
};

impl DynamoUserRepo {
    async fn get_plan_detail_unchecked(
        &self,
        trip_id: &str,
        meta: &TripMeta,
    ) -> Result<PlanDetail, TripRepoError> {
        let version = meta.current_plan_version.ok_or(TripRepoError::NotFound)?;
        let expected_plan_id = meta
            .current_plan_id
            .as_ref()
            .ok_or(TripRepoError::CorruptData)?;
        let pk = trip_pk(trip_id);
        let prefix = format!("{}#", plan_prefix(version));
        let items = self
            .query_partition(&pk, &prefix, TRIP_COLLECTION_PAGE_SIZE)
            .await?;
        let mut plan = None;
        let mut days = Vec::new();
        let mut stops = Vec::new();
        for item in items {
            let sk = string(&item, SK)?;
            match string(&item, ENTITY_TYPE)?.as_str() {
                PLAN_ENTITY => {
                    let value: Stored<Plan> = decode_record(&item, &pk, &sk, PLAN_ENTITY)?;
                    plan = Some(value.value);
                }
                DAY_ENTITY => {
                    let value: Stored<Day> = decode_record(&item, &pk, &sk, DAY_ENTITY)?;
                    days.push(value.value);
                }
                STOP_ENTITY => {
                    let value: Stored<Stop> = decode_record(&item, &pk, &sk, STOP_ENTITY)?;
                    stops.push(value.value);
                }
                _ => return Err(TripRepoError::CorruptData),
            }
        }
        let plan = plan.ok_or(TripRepoError::CorruptData)?;
        if &plan.id != expected_plan_id || plan.trip_id != trip_id || plan.version != version {
            return Err(TripRepoError::CorruptData);
        }
        if days.iter().any(|day| day.plan_id != plan.id) {
            return Err(TripRepoError::CorruptData);
        }
        let day_ids = days
            .iter()
            .map(|day| day.id.as_str())
            .collect::<HashSet<_>>();
        if stops
            .iter()
            .any(|stop| !day_ids.contains(stop.day_id.as_str()))
        {
            return Err(TripRepoError::CorruptData);
        }
        days.sort_by(|a, b| a.date.cmp(&b.date));
        stops.sort_by(|a, b| {
            a.day_id
                .cmp(&b.day_id)
                .then_with(|| a.seq.total_cmp(&b.seq))
        });
        let mut places = Vec::new();
        let mut seen = HashSet::new();
        for stop in &stops {
            if seen.insert(stop.place_id.clone()) {
                places.push(
                    self.get_place_record(trip_id, &stop.place_id)
                        .await?
                        .ok_or(TripRepoError::CorruptData)?
                        .value,
                );
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

pub(super) async fn get_current_plan(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<PlanDetail, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Any).await?;
    let meta = repo.get_trip_meta(trip_id).await?.value;
    repo.get_plan_detail_unchecked(trip_id, &meta).await
}

pub(super) async fn initialize_plan(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    anchor_place_id: &str,
    plan: Plan,
    days: Vec<Day>,
) -> Result<PlanDetail, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
    let stored_meta = repo.get_trip_meta(trip_id).await?;
    if stored_meta.value.current_plan_id.is_some() {
        return repo
            .get_plan_detail_unchecked(trip_id, &stored_meta.value)
            .await;
    }
    let anchor = repo
        .query_partition(&trip_pk(trip_id), "CANDIDATE#", TRIP_COLLECTION_PAGE_SIZE)
        .await?
        .into_iter()
        .map(|item| {
            let sk = string(&item, SK)?;
            decode_record::<Candidate>(&item, &trip_pk(trip_id), &sk, CANDIDATE_ENTITY)
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|candidate| {
            candidate.value.place_id == anchor_place_id
                && candidate.value.status == CandidateStatus::Shortlisted
        })
        .ok_or(TripRepoError::NotFound)?;
    if repo
        .get_place_record(trip_id, &anchor.value.place_id)
        .await?
        .is_none()
    {
        return Err(TripRepoError::CorruptData);
    }
    let mut meta = stored_meta.value;
    meta.current_plan_id = Some(plan.id.clone());
    meta.current_plan_version = Some(plan.version);
    meta.status = TripStatus::Planning;
    let mut seen = HashSet::new();
    meta.cities = days
        .iter()
        .map(|day| day.city_hint.clone())
        .filter(|city| seen.insert(city.clone()))
        .collect();
    let mut tx = repo
        .transaction()
        .transact_items(condition_action(repo.member_condition(
            trip_id,
            actor,
            RequiredRole::Editor,
        )))
        .transact_items(condition_action(repo.entity_revision_condition(
            trip_pk(trip_id),
            anchor.sort_key,
            CANDIDATE_ENTITY,
            anchor.revision,
        )))
        .transact_items(put_action(repo.revision_put(
            encode_trip_meta(&meta, stored_meta.revision + 1)?,
            stored_meta.revision,
        )))
        .transact_items(put_action(repo.create_only_put(encode_record(
            trip_pk(trip_id),
            plan_sk(plan.version),
            PLAN_ENTITY,
            &plan,
            1,
        )?)));
    for day in &days {
        tx = tx.transact_items(put_action(repo.create_only_put(encode_record(
            trip_pk(trip_id),
            day_sk(plan.version, day),
            DAY_ENTITY,
            day,
            1,
        )?)));
    }
    if let Err(error) = tx.send().await {
        if !transaction_condition_failed(error.as_service_error()) {
            return Err(TripRepoError::Unavailable);
        }
        repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
        let latest = repo.get_trip_meta(trip_id).await?;
        if latest.value.current_plan_id.is_some() {
            return repo.get_plan_detail_unchecked(trip_id, &latest.value).await;
        }
        return Err(TripRepoError::Conflict);
    }
    repo.get_plan_detail_unchecked(trip_id, &meta).await
}

pub(super) async fn list_plan_versions(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<Plan>, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Any).await?;
    let pk = trip_pk(trip_id);
    let mut plans = repo
        .query_partition(&pk, "PLAN#", TRIP_COLLECTION_PAGE_SIZE)
        .await?
        .into_iter()
        .filter(|item| string(item, ENTITY_TYPE).is_ok_and(|entity| entity == PLAN_ENTITY))
        .map(|item| {
            let sk = string(&item, SK)?;
            decode_record::<Plan>(&item, &pk, &sk, PLAN_ENTITY).map(|stored| stored.value)
        })
        .collect::<Result<Vec<_>, _>>()?;
    plans.sort_by_key(|plan| plan.version);
    Ok(plans)
}

pub(super) async fn update_day(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    day_id: &str,
    patch: DayPatch,
    changed_at: &str,
    change_id: &str,
) -> Result<Day, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
    let stored_meta = repo.get_trip_meta(trip_id).await?;
    let version = stored_meta
        .value
        .current_plan_version
        .ok_or(TripRepoError::NotFound)?;
    let pk = trip_pk(trip_id);
    let mut stored_day = None;
    let mut all_days = Vec::new();
    for item in repo
        .query_partition(
            &pk,
            &format!("{}#DAY#", plan_prefix(version)),
            TRIP_COLLECTION_PAGE_SIZE,
        )
        .await?
    {
        let sk = string(&item, SK)?;
        let day: Stored<Day> = decode_record(&item, &pk, &sk, DAY_ENTITY)?;
        if day.value.id == day_id {
            stored_day = Some(Stored {
                value: day.value.clone(),
                revision: day.revision,
                sort_key: sk,
            });
        }
        all_days.push(day.value);
    }
    let stored_day = stored_day.ok_or(TripRepoError::NotFound)?;
    let mut day = stored_day.value;
    let mut changes = Vec::new();
    if let Some(value) = patch.window_start {
        if day.window_start != value {
            changes.push((
                "windowStart",
                json!(day.window_start.clone()),
                json!(value.clone()),
            ));
        }
        day.window_start = value;
    }
    if let Some(value) = patch.window_end {
        if day.window_end != value {
            changes.push((
                "windowEnd",
                json!(day.window_end.clone()),
                json!(value.clone()),
            ));
        }
        day.window_end = value;
    }
    if let Some(value) = patch.city_hint {
        if day.city_hint != value {
            changes.push((
                "cityHint",
                json!(day.city_hint.clone()),
                json!(value.clone()),
            ));
        }
        day.city_hint = value;
    }
    // The service validates the request against a snapshot for a useful
    // 400 response. Recheck against the exact revision being written so a
    // concurrent complementary patch cannot persist an inverted window.
    if !day.window_is_ordered() {
        return Err(TripRepoError::Conflict);
    }
    if changes.is_empty() {
        return Ok(day);
    }
    let city_changed = changes.iter().any(|(field, _, _)| *field == "cityHint");
    for item in &mut all_days {
        if item.id == day.id {
            *item = day.clone();
        }
    }
    let mut meta = stored_meta.value;
    if city_changed {
        let mut seen = HashSet::new();
        meta.cities = all_days
            .into_iter()
            .map(|day| day.city_hint)
            .filter(|city| seen.insert(city.clone()))
            .collect();
    }
    let mut audit_items = Vec::with_capacity(changes.len());
    for (index, (field, old_value, new_value)) in changes.into_iter().enumerate() {
        let event_id = suffixed_id(change_id, index);
        let change = audit(
            trip_id,
            actor,
            changed_at,
            &event_id,
            AuditChange {
                entity: "day",
                entity_id: day_id,
                field,
                old_value,
                new_value,
            },
        );
        audit_items.push(encode_record(
            pk.clone(),
            audit_sk(changed_at, &event_id),
            AUDIT_ENTITY,
            &change,
            1,
        )?);
    }
    let reservation_actions = reserve_history_append(repo, trip_id, &audit_items)
        .await
        .map_err(history_reservation_error)?;
    let mut tx = repo
        .transaction()
        .transact_items(condition_action(repo.member_condition(
            trip_id,
            actor,
            RequiredRole::Editor,
        )))
        .transact_items(put_action(repo.revision_put(
            encode_record(
                pk.clone(),
                stored_day.sort_key,
                DAY_ENTITY,
                &day,
                stored_day.revision + 1,
            )?,
            stored_day.revision,
        )));
    if city_changed {
        tx = tx.transact_items(put_action(repo.revision_put(
            encode_trip_meta(&meta, stored_meta.revision + 1)?,
            stored_meta.revision,
        )));
    } else {
        // Pin the child write to the plan that was current when it was
        // read. Phase 3 can then publish a new immutable plan version
        // without an in-flight content edit mutating the old one.
        tx = tx.transact_items(condition_action(repo.entity_revision_condition(
            pk.clone(),
            META_SK,
            TRIP_ENTITY,
            stored_meta.revision,
        )));
    }
    for action in reservation_actions {
        tx = tx.transact_items(action);
    }
    for item in audit_items {
        tx = tx.transact_items(put_action(repo.create_only_put(item)));
    }
    let result = tx.send().await;
    if let Err(error) = result {
        if !transaction_condition_failed(error.as_service_error()) {
            return Err(TripRepoError::Unavailable);
        }
        repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
        return Err(TripRepoError::Conflict);
    }
    Ok(day)
}

pub(super) async fn update_stop(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    stop_id: &str,
    patch: StopPatch,
    changed_at: &str,
    change_id: &str,
) -> Result<Stop, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
    let stored_meta = repo.get_trip_meta(trip_id).await?;
    let version = stored_meta
        .value
        .current_plan_version
        .ok_or(TripRepoError::NotFound)?;
    let pk = trip_pk(trip_id);
    let mut stored_stop = None;
    for item in repo
        .query_partition(
            &pk,
            &format!("{}#", plan_prefix(version)),
            TRIP_COLLECTION_PAGE_SIZE,
        )
        .await?
    {
        if string(&item, ENTITY_TYPE)? != STOP_ENTITY {
            continue;
        }
        let sk = string(&item, SK)?;
        let stop: Stored<Stop> = decode_record(&item, &pk, &sk, STOP_ENTITY)?;
        if stop.value.id == stop_id {
            stored_stop = Some(stop);
            break;
        }
    }
    let stored = stored_stop.ok_or(TripRepoError::NotFound)?;
    let mut stop = stored.value;
    let current_ledger_entry_id = stop
        .booking
        .as_ref()
        .and_then(|booking| booking.ledger_entry_id.clone());
    let mut changes = Vec::new();
    if let Some(value) = patch.planned_arrival {
        if stop.planned_arrival != value {
            changes.push((
                "plannedArrival",
                json!(stop.planned_arrival.clone()),
                json!(value.clone()),
            ));
        }
        stop.planned_arrival = value;
    }
    if let Some(value) = patch.duration_min {
        if stop.duration_min != value {
            changes.push(("durationMin", json!(stop.duration_min), json!(value)));
        }
        stop.duration_min = value;
    }
    if let Some(value) = patch.notes {
        if stop.notes != value {
            changes.push(("notes", json!(stop.notes.clone()), json!(value.clone())));
        }
        stop.notes = value;
    }
    if let Some(value) = patch.booking {
        let requested_ledger_entry_id = value
            .as_ref()
            .and_then(|booking| booking.ledger_entry_id.as_ref());
        if requested_ledger_entry_id != current_ledger_entry_id.as_ref() {
            return Err(TripRepoError::CorruptData);
        }
        if stop.booking != value {
            changes.push(("booking", json!(stop.booking.clone()), json!(value.clone())));
        }
        stop.booking = value;
    }
    if changes.is_empty() {
        return Ok(stop);
    }
    let link_claim = load_stop_link_claim(repo, trip_id, stop_id).await?;
    match (current_ledger_entry_id.as_deref(), link_claim.as_ref()) {
        (Some(expense_id), Some(claim)) if claim.value.expense_id == expense_id => {}
        (None, None) => {}
        _ => return Err(TripRepoError::CorruptData),
    }
    let mut audit_items = Vec::with_capacity(changes.len());
    for (index, (field, old_value, new_value)) in changes.into_iter().enumerate() {
        let event_id = suffixed_id(change_id, index);
        let change = audit(
            trip_id,
            actor,
            changed_at,
            &event_id,
            AuditChange {
                entity: "stop",
                entity_id: stop_id,
                field,
                old_value,
                new_value,
            },
        );
        audit_items.push(encode_record(
            pk.clone(),
            audit_sk(changed_at, &event_id),
            AUDIT_ENTITY,
            &change,
            1,
        )?);
    }
    let reservation_actions = reserve_history_append(repo, trip_id, &audit_items)
        .await
        .map_err(history_reservation_error)?;
    let mut tx = repo
        .transaction()
        .transact_items(condition_action(repo.member_condition(
            trip_id,
            actor,
            RequiredRole::Editor,
        )))
        .transact_items(condition_action(repo.entity_revision_condition(
            pk.clone(),
            META_SK,
            TRIP_ENTITY,
            stored_meta.revision,
        )));
    if let Some(claim) = link_claim {
        tx = tx.transact_items(condition_action(repo.entity_revision_data_condition(
            pk.clone(),
            stop_link_sk(stop_id),
            STOP_LINK_ENTITY,
            claim.revision,
            &claim.raw_data,
        )));
    }
    let next_revision = stored
        .revision
        .checked_add(1)
        .ok_or(TripRepoError::CorruptData)?;
    tx = tx.transact_items(put_action(repo.revision_put(
        encode_record(
            pk.clone(),
            stored.sort_key,
            STOP_ENTITY,
            &stop,
            next_revision,
        )?,
        stored.revision,
    )));
    for action in reservation_actions {
        tx = tx.transact_items(action);
    }
    for item in audit_items {
        tx = tx.transact_items(put_action(repo.create_only_put(item)));
    }
    let result = tx.send().await;
    if let Err(error) = result {
        if !transaction_condition_failed(error.as_service_error()) {
            return Err(TripRepoError::Unavailable);
        }
        repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
        return Err(TripRepoError::Conflict);
    }
    Ok(stop)
}

async fn load_stop_link_claim(
    repo: &DynamoUserRepo,
    trip_id: &str,
    stop_id: &str,
) -> Result<Option<LedgerLoaded<StopLinkRecord>>, TripRepoError> {
    let item = repo
        .consistent_get(trip_pk(trip_id), stop_link_sk(stop_id))
        .send()
        .await
        .map_err(|_| TripRepoError::Unavailable)?
        .item;
    item.map(|item| decode_stop_link(&item, trip_id).map_err(|_| TripRepoError::CorruptData))
        .transpose()
}
