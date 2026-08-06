//! Immutable plan preparation and atomic publication.

use itinera_core::domain::trip::{DayFeasibility, Feasibility};

use super::{access::*, records::*, *};

pub(super) struct SourceRecord {
    sort_key: String,
    entity: &'static str,
    revision: u64,
}

pub(super) struct CandidateChange {
    stored: Loaded<Candidate>,
    updated: Candidate,
}

pub(super) struct PreparedApplication {
    pub(super) meta: Loaded<TripMeta>,
    pub(super) proposal: Proposal,
    application: PlanApplication,
    source_records: Vec<SourceRecord>,
    candidate_changes: Vec<CandidateChange>,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ProposalWrite {
    Create,
    Update { revision: u64 },
}

pub(super) async fn prepare_application(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    mut proposal: Proposal,
    meta: Loaded<TripMeta>,
    applied_at: &str,
    application_ids: ProposalApplicationIds,
) -> Result<PreparedApplication, ProposalRepoError> {
    validate_stored_proposal(trip_id, &proposal).map_err(application_error)?;
    if proposal.status != ProposalStatus::Pending
        || proposal.route != ProposalRoute::LeaderApproval
        || meta.value.current_plan_version != Some(proposal.change_set.base_plan_version)
    {
        return Err(ProposalRepoError::Conflict);
    }

    let loaded_plan = load_current_plan(repo, trip_id, &meta).await?;
    let resolved_places = load_operation_places(repo, trip_id, &proposal.change_set.ops).await?;
    let application = apply_change_set(
        &loaded_plan.detail,
        trip_id,
        &proposal.id,
        &proposal.change_set,
        &resolved_places,
        applied_at,
        application_ids,
    )
    .map_err(application_error)?;
    let candidate_changes = candidate_changes(repo, trip_id, &application).await?;

    proposal.status = ProposalStatus::Applied;
    proposal.decided_by = Some(ProposalDecision::Leader {
        user_id: actor.0.clone(),
    });
    proposal.rejection_reason = None;
    Ok(PreparedApplication {
        meta,
        proposal,
        application,
        source_records: loaded_plan.source_records,
        candidate_changes,
    })
}

pub(super) async fn publish_application(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    prepared: PreparedApplication,
    proposal_write: ProposalWrite,
) -> Result<Proposal, ProposalRepoError> {
    let PreparedApplication {
        meta,
        proposal,
        application,
        source_records,
        candidate_changes,
    } = prepared;
    let expected_plan_id = meta
        .value
        .current_plan_id
        .as_deref()
        .ok_or(ProposalRepoError::CorruptData)?;
    let expected_plan_version = meta
        .value
        .current_plan_version
        .ok_or(ProposalRepoError::CorruptData)?;

    let mut next_meta = meta.value.clone();
    next_meta.current_plan_id = Some(application.plan.id.clone());
    next_meta.current_plan_version = Some(application.plan.version);
    let mut seen = HashSet::new();
    next_meta.cities = application
        .days
        .iter()
        .map(|day| day.city_hint.clone())
        .filter(|city| seen.insert(city.clone()))
        .collect();

    let next_meta_revision = meta
        .revision
        .checked_add(1)
        .ok_or(ProposalRepoError::CorruptData)?;
    let meta_item = encode_trip_meta(&next_meta, next_meta_revision).map_err(record_error)?;
    let proposal_revision = match proposal_write {
        ProposalWrite::Create => 1,
        ProposalWrite::Update { revision } => revision
            .checked_add(1)
            .ok_or(ProposalRepoError::CorruptData)?,
    };
    let proposal_item = encode_proposal(&proposal, proposal_revision)?;
    let plan_item = encode_record(
        trip_pk(trip_id),
        plan_sk(application.plan.version),
        PLAN_ENTITY,
        &application.plan,
        1,
    )
    .map_err(record_error)?;

    let mut day_items = Vec::with_capacity(application.days.len());
    for day in &application.days {
        day_items.push(
            encode_record(
                trip_pk(trip_id),
                day_sk(application.plan.version, day),
                DAY_ENTITY,
                day,
                1,
            )
            .map_err(record_error)?,
        );
    }
    let days_by_id = application
        .days
        .iter()
        .map(|day| (day.id.as_str(), day))
        .collect::<HashMap<_, _>>();
    let mut stop_items = Vec::with_capacity(application.stops.len());
    for stop in &application.stops {
        let day = days_by_id
            .get(stop.day_id.as_str())
            .ok_or(ProposalRepoError::CorruptData)?;
        stop_items.push(
            encode_record(
                trip_pk(trip_id),
                stop_sk(application.plan.version, day, stop),
                STOP_ENTITY,
                stop,
                1,
            )
            .map_err(record_error)?,
        );
    }
    let mut place_items = Vec::with_capacity(application.new_places.len());
    for place in &application.new_places {
        place_items.push(
            encode_record(
                trip_pk(trip_id),
                place_sk(&place.id),
                PLACE_ENTITY,
                place,
                1,
            )
            .map_err(record_error)?,
        );
    }
    let mut candidate_items = Vec::with_capacity(candidate_changes.len());
    for change in &candidate_changes {
        let next_revision = change
            .stored
            .revision
            .checked_add(1)
            .ok_or(ProposalRepoError::CorruptData)?;
        candidate_items.push(
            encode_record(
                trip_pk(trip_id),
                change.stored.sort_key.clone(),
                CANDIDATE_ENTITY,
                &change.updated,
                next_revision,
            )
            .map_err(record_error)?,
        );
    }

    let mut written_items = vec![meta_item.clone(), proposal_item.clone(), plan_item.clone()];
    written_items.extend(day_items.iter().cloned());
    written_items.extend(stop_items.iter().cloned());
    written_items.extend(place_items.iter().cloned());
    written_items.extend(candidate_items.iter().cloned());
    enforce_transaction_data_limit(&written_items)?;

    let mut actions = vec![
        action_condition(membership_condition(
            &repo.table_name,
            trip_id,
            actor,
            RequiredProposalRole::Leader,
        )),
        action_put(current_plan_revision_put(
            &repo.table_name,
            meta_item,
            meta.revision,
            expected_plan_id,
            expected_plan_version,
        )),
        action_put(match proposal_write {
            ProposalWrite::Create => create_put(&repo.table_name, proposal_item),
            ProposalWrite::Update { revision } => {
                revision_put(&repo.table_name, proposal_item, revision)
            }
        }),
    ];
    for source in source_records {
        actions.push(action_condition(record_revision_condition(
            &repo.table_name,
            &trip_pk(trip_id),
            &source.sort_key,
            source.entity,
            source.revision,
        )));
    }
    actions.push(action_put(create_put(&repo.table_name, plan_item)));
    for item in day_items.into_iter().chain(stop_items) {
        actions.push(action_put(create_put(&repo.table_name, item)));
    }
    for item in place_items {
        actions.push(action_put(create_put(&repo.table_name, item)));
    }
    for (change, item) in candidate_changes.into_iter().zip(candidate_items) {
        actions.push(action_put(revision_put(
            &repo.table_name,
            item,
            change.stored.revision,
        )));
    }
    enforce_transaction_action_limit(actions.len())?;

    repo.client
        .transact_write_items()
        .set_transact_items(Some(actions))
        .send()
        .await
        .map_err(|error| {
            if transaction_condition_failed(error.as_service_error()) {
                ProposalRepoError::Conflict
            } else {
                ProposalRepoError::Unavailable
            }
        })?;
    Ok(proposal)
}

struct LoadedPlan {
    detail: PlanDetail,
    source_records: Vec<SourceRecord>,
}

async fn load_current_plan(
    repo: &DynamoUserRepo,
    trip_id: &str,
    meta: &Loaded<TripMeta>,
) -> Result<LoadedPlan, ProposalRepoError> {
    let version = meta
        .value
        .current_plan_version
        .ok_or(ProposalRepoError::Conflict)?;
    let expected_plan_id = meta
        .value
        .current_plan_id
        .as_ref()
        .ok_or(ProposalRepoError::CorruptData)?;
    let pk = trip_pk(trip_id);
    let items = repo
        .proposal_query(
            &pk,
            &format!("{}#", plan_prefix(version)),
            TRIP_COLLECTION_PAGE_SIZE,
            MAX_PROPOSAL_RECORDS,
            MAX_PROPOSAL_BYTES,
        )
        .await?;
    let mut plan = None;
    let mut days = Vec::new();
    let mut stored_stops = Vec::new();
    let mut source_records = Vec::with_capacity(items.len());
    for item in items {
        let sk = string(&item, SK).map_err(record_error)?;
        let entity = string(&item, ENTITY_TYPE).map_err(record_error)?;
        match entity.as_str() {
            PLAN_ENTITY => {
                let loaded: Loaded<Plan> = decode_loaded(&item, &pk, &sk, PLAN_ENTITY)?;
                if sk != plan_sk(version) {
                    return Err(ProposalRepoError::CorruptData);
                }
                source_records.push(SourceRecord {
                    sort_key: sk,
                    entity: PLAN_ENTITY,
                    revision: loaded.revision,
                });
                if plan.replace(loaded.value).is_some() {
                    return Err(ProposalRepoError::CorruptData);
                }
            }
            DAY_ENTITY => {
                let loaded: Loaded<Day> = decode_loaded(&item, &pk, &sk, DAY_ENTITY)?;
                if loaded.value.plan_id != expected_plan_id.as_str()
                    || sk != day_sk(version, &loaded.value)
                {
                    return Err(ProposalRepoError::CorruptData);
                }
                source_records.push(SourceRecord {
                    sort_key: sk,
                    entity: DAY_ENTITY,
                    revision: loaded.revision,
                });
                days.push(loaded.value);
            }
            STOP_ENTITY => {
                let loaded: Loaded<Stop> = decode_loaded(&item, &pk, &sk, STOP_ENTITY)?;
                source_records.push(SourceRecord {
                    sort_key: sk.clone(),
                    entity: STOP_ENTITY,
                    revision: loaded.revision,
                });
                stored_stops.push((sk, loaded.value));
            }
            _ => return Err(ProposalRepoError::CorruptData),
        }
    }
    let plan = plan.ok_or(ProposalRepoError::CorruptData)?;
    if &plan.id != expected_plan_id || plan.trip_id != trip_id || plan.version != version {
        return Err(ProposalRepoError::CorruptData);
    }
    days.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| left.id.cmp(&right.id))
    });
    let days_by_id = days
        .iter()
        .map(|day| (day.id.as_str(), day))
        .collect::<HashMap<_, _>>();
    let mut stops = Vec::with_capacity(stored_stops.len());
    for (sk, stop) in stored_stops {
        let day = days_by_id
            .get(stop.day_id.as_str())
            .ok_or(ProposalRepoError::CorruptData)?;
        if !stop.seq.is_finite()
            || stop.seq <= 0.0
            || stop.seq.fract() != 0.0
            || sk != stop_sk(version, day, &stop)
        {
            return Err(ProposalRepoError::CorruptData);
        }
        stops.push(stop);
    }
    stops.sort_by(|left, right| {
        left.day_id
            .cmp(&right.day_id)
            .then_with(|| left.seq.total_cmp(&right.seq))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut places = Vec::new();
    let mut seen = HashSet::new();
    for stop in &stops {
        if seen.insert(stop.place_id.clone()) {
            places.push(load_place(repo, trip_id, &stop.place_id).await.map_err(
                |error| match error {
                    ProposalRepoError::NotFound => ProposalRepoError::CorruptData,
                    other => other,
                },
            )?);
        }
    }
    let day_feasibility = days
        .iter()
        .map(|day| DayFeasibility {
            day_id: day.id.clone(),
            feasibility: Feasibility::Ok,
            used_min: 0,
            window_min: 0,
            notes: vec![],
        })
        .collect();
    Ok(LoadedPlan {
        detail: PlanDetail {
            plan,
            days,
            stops,
            legs: vec![],
            day_feasibility,
            places,
        },
        source_records,
    })
}

async fn load_operation_places(
    repo: &DynamoUserRepo,
    trip_id: &str,
    ops: &[ChangeOp],
) -> Result<HashMap<String, Place>, ProposalRepoError> {
    let mut ids = HashSet::new();
    for op in ops {
        match op {
            ChangeOp::AddStop { place_id, .. } => {
                ids.insert(place_id.clone());
            }
            ChangeOp::SwapPlace { new_place_id, .. } => {
                ids.insert(new_place_id.clone());
            }
            _ => {}
        }
    }
    let mut places = HashMap::with_capacity(ids.len());
    for id in ids {
        places.insert(id.clone(), load_place(repo, trip_id, &id).await?);
    }
    Ok(places)
}

async fn load_place(
    repo: &DynamoUserRepo,
    trip_id: &str,
    place_id: &str,
) -> Result<Place, ProposalRepoError> {
    let pk = trip_pk(trip_id);
    let sk = place_sk(place_id);
    let item = repo
        .proposal_get(&pk, &sk)
        .await?
        .ok_or(ProposalRepoError::NotFound)?;
    let place: Loaded<Place> = decode_loaded(&item, &pk, &sk, PLACE_ENTITY)?;
    if place.value.id != place_id || validate_place_snapshot(&place.value).is_err() {
        return Err(ProposalRepoError::CorruptData);
    }
    Ok(place.value)
}

async fn candidate_changes(
    repo: &DynamoUserRepo,
    trip_id: &str,
    application: &PlanApplication,
) -> Result<Vec<CandidateChange>, ProposalRepoError> {
    let pk = trip_pk(trip_id);
    let items = repo
        .proposal_query(
            &pk,
            "CANDIDATE#",
            PROPOSAL_PAGE_SIZE,
            MAX_PROPOSAL_RECORDS,
            MAX_PROPOSAL_BYTES,
        )
        .await?;
    let in_plan = application
        .stops
        .iter()
        .map(|stop| stop.place_id.as_str())
        .collect::<HashSet<_>>();
    let mut changes = Vec::new();
    for item in items {
        let sk = string(&item, SK).map_err(record_error)?;
        let stored: Loaded<Candidate> = decode_loaded(&item, &pk, &sk, CANDIDATE_ENTITY)?;
        if validate_stored_candidate(trip_id, &stored.value).is_err()
            || stored.sort_key != candidate_sk(&stored.value.id)
        {
            return Err(ProposalRepoError::CorruptData);
        }
        let adopted = in_plan.contains(stored.value.place_id.as_str());
        // A candidate's place is a trip-owned immutable snapshot. Any row that
        // participates in an adoption/removal decision must still point to a
        // valid Place in this same trip partition before we rewrite it.
        if adopted || stored.value.status == CandidateStatus::InPlan {
            load_place(repo, trip_id, &stored.value.place_id)
                .await
                .map_err(|error| match error {
                    ProposalRepoError::NotFound => ProposalRepoError::CorruptData,
                    other => other,
                })?;
        }
        if adopted && stored.value.status == CandidateStatus::Rejected {
            return Err(ProposalRepoError::InvalidChange);
        }
        let desired = if adopted {
            CandidateStatus::InPlan
        } else if stored.value.status == CandidateStatus::InPlan {
            CandidateStatus::Shortlisted
        } else {
            stored.value.status
        };
        if desired != stored.value.status {
            let mut updated = stored.value.clone();
            updated.status = desired;
            changes.push(CandidateChange { stored, updated });
        }
    }
    Ok(changes)
}
