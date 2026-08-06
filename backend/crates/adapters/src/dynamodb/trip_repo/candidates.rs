//! Candidate-owned place snapshots and candidate lifecycle operations.

use super::*;

impl DynamoUserRepo {
    async fn get_candidate_record(
        &self,
        trip_id: &str,
        candidate_id: &str,
    ) -> Result<Option<Stored<Candidate>>, TripRepoError> {
        let pk = trip_pk(trip_id);
        let sk = candidate_sk(candidate_id);
        let Some(item) = self.trip_get(&pk, &sk).await? else {
            return Ok(None);
        };
        let stored: Stored<Candidate> = decode_record(&item, &pk, &sk, CANDIDATE_ENTITY)?;
        if stored.value.id != candidate_id
            || validate_stored_candidate(trip_id, &stored.value).is_err()
        {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Some(stored))
    }

    pub(super) async fn get_place_record(
        &self,
        trip_id: &str,
        place_id: &str,
    ) -> Result<Option<Stored<Place>>, TripRepoError> {
        let pk = trip_pk(trip_id);
        let sk = place_sk(place_id);
        let Some(item) = self.trip_get(&pk, &sk).await? else {
            return Ok(None);
        };
        let stored: Stored<Place> = decode_record(&item, &pk, &sk, PLACE_ENTITY)?;
        if stored.value.id != place_id || validate_place_snapshot(&stored.value).is_err() {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Some(stored))
    }
}

pub(super) async fn search_saved_places(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    query: &str,
) -> Result<Vec<Place>, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Any).await?;
    let pk = trip_pk(trip_id);
    let query = query.to_lowercase();
    let mut adopted_place_ids = HashSet::new();
    for item in repo
        .query_partition(&pk, "PLAN#", TRIP_COLLECTION_PAGE_SIZE)
        .await?
    {
        if string(&item, ENTITY_TYPE).is_ok_and(|entity| entity == STOP_ENTITY) {
            let sk = string(&item, SK)?;
            let stop: Stored<Stop> = decode_record(&item, &pk, &sk, STOP_ENTITY)?;
            adopted_place_ids.insert(stop.value.place_id);
        }
    }
    if adopted_place_ids.is_empty() {
        return Ok(vec![]);
    }
    repo.query_partition(&pk, "PLACE#", TRIP_COLLECTION_PAGE_SIZE)
        .await?
        .into_iter()
        .map(|item| {
            let sk = string(&item, SK)?;
            decode_record::<Place>(&item, &pk, &sk, PLACE_ENTITY).map(|stored| stored.value)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|places| {
            places
                .into_iter()
                .filter(|place| {
                    adopted_place_ids.contains(&place.id)
                        && format!("{} {} {}", place.name, place.city, place.address)
                            .to_lowercase()
                            .contains(&query)
                })
                .collect()
        })
}

pub(super) async fn find_place(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    place_id: &str,
) -> Result<Option<Place>, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Any).await?;
    Ok(repo
        .get_place_record(trip_id, place_id)
        .await?
        .map(|stored| stored.value))
}

pub(super) async fn list_candidates(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<CandidateWithPlace>, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Any).await?;
    let pk = trip_pk(trip_id);
    let items = repo
        .query_partition(&pk, "CANDIDATE#", TRIP_COLLECTION_PAGE_SIZE)
        .await?;
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let sk = string(&item, SK)?;
        let candidate: Stored<Candidate> = decode_record(&item, &pk, &sk, CANDIDATE_ENTITY)?;
        if validate_stored_candidate(trip_id, &candidate.value).is_err()
            || sk != candidate_sk(&candidate.value.id)
        {
            return Err(TripRepoError::CorruptData);
        }
        let place = repo
            .get_place_record(trip_id, &candidate.value.place_id)
            .await?
            .ok_or(TripRepoError::CorruptData)?
            .value;
        result.push(CandidateWithPlace {
            candidate: candidate.value,
            place,
        });
    }
    result.sort_by(|a, b| a.candidate.created_at.cmp(&b.candidate.created_at));
    Ok(result)
}

pub(super) async fn add_candidate(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    candidate: Candidate,
    place: Place,
) -> Result<CandidateWithPlace, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
    let result = repo
        .client
        .transact_write_items()
        .transact_items(action_condition(member_condition(
            &repo.table_name,
            trip_id,
            actor,
            RequiredRole::Editor,
        )))
        .transact_items(action_put(create_put(
            &repo.table_name,
            encode_record(
                trip_pk(trip_id),
                candidate_sk(&candidate.id),
                CANDIDATE_ENTITY,
                &candidate,
                1,
            )?,
        )))
        .transact_items(action_put(create_put(
            &repo.table_name,
            encode_record(
                trip_pk(trip_id),
                place_sk(&place.id),
                PLACE_ENTITY,
                &place,
                1,
            )?,
        )))
        .send()
        .await;
    if let Err(error) = result {
        if !transaction_condition_failed(error.as_service_error()) {
            return Err(TripRepoError::Unavailable);
        }
        repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
        return Err(TripRepoError::Conflict);
    }
    Ok(CandidateWithPlace { candidate, place })
}

pub(super) async fn update_candidate(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    candidate_id: &str,
    update: CandidateUpdate,
) -> Result<CandidateWithPlace, TripRepoError> {
    let CandidateUpdate {
        place,
        pitch,
        tags,
        changed_at,
        change_id,
    } = update;
    repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
    let stored = repo
        .get_candidate_record(trip_id, candidate_id)
        .await?
        .ok_or(TripRepoError::NotFound)?;
    let old_place = repo
        .get_place_record(trip_id, &stored.value.place_id)
        .await?
        .ok_or(TripRepoError::CorruptData)?
        .value;
    let next_revision = stored
        .revision
        .checked_add(1)
        .ok_or(TripRepoError::CorruptData)?;
    let mut candidate = stored.value;
    let old_pitch = candidate.pitch.clone();
    let old_tags = candidate.tags.clone();
    candidate.place_id = place.id.clone();
    candidate.pitch = pitch;
    candidate.tags = tags;
    let mut changes = vec![(
        "place",
        serde_json::to_value(old_place).map_err(|_| TripRepoError::CorruptData)?,
        serde_json::to_value(&place).map_err(|_| TripRepoError::CorruptData)?,
    )];
    if old_pitch != candidate.pitch {
        changes.push(("pitch", json!(old_pitch), json!(candidate.pitch.clone())));
    }
    if old_tags != candidate.tags {
        changes.push(("tags", json!(old_tags), json!(candidate.tags.clone())));
    }
    let mut tx = repo
        .client
        .transact_write_items()
        .transact_items(action_condition(member_condition(
            &repo.table_name,
            trip_id,
            actor,
            RequiredRole::Editor,
        )))
        .transact_items(action_put(revision_put(
            &repo.table_name,
            encode_record(
                trip_pk(trip_id),
                candidate_sk(candidate_id),
                CANDIDATE_ENTITY,
                &candidate,
                next_revision,
            )?,
            stored.revision,
        )))
        .transact_items(action_put(create_put(
            &repo.table_name,
            encode_record(
                trip_pk(trip_id),
                place_sk(&place.id),
                PLACE_ENTITY,
                &place,
                1,
            )?,
        )));
    for (index, (field, old_value, new_value)) in changes.into_iter().enumerate() {
        let event_id = suffixed_id(&change_id, index);
        let change = audit(
            trip_id,
            actor,
            &changed_at,
            &event_id,
            AuditChange {
                entity: "candidate",
                entity_id: candidate_id,
                field,
                old_value,
                new_value,
            },
        );
        tx = tx.transact_items(action_put(create_put(
            &repo.table_name,
            encode_record(
                trip_pk(trip_id),
                audit_sk(&changed_at, &event_id),
                AUDIT_ENTITY,
                &change,
                1,
            )?,
        )));
    }
    let result = tx.send().await;
    if let Err(error) = result {
        if !transaction_condition_failed(error.as_service_error()) {
            return Err(TripRepoError::Unavailable);
        }
        repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
        return Err(TripRepoError::Conflict);
    }
    Ok(CandidateWithPlace { candidate, place })
}

pub(super) async fn set_candidate_status(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    candidate_id: &str,
    status: CandidateDisposition,
    changed_at: &str,
    change_id: &str,
) -> Result<CandidateWithPlace, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
    let stored = repo
        .get_candidate_record(trip_id, candidate_id)
        .await?
        .ok_or(TripRepoError::NotFound)?;
    if stored.value.status == CandidateStatus::InPlan {
        return Err(TripRepoError::Conflict);
    }
    let mut candidate = stored.value;
    let old = candidate.status;
    let desired = CandidateStatus::from(status);
    let place = repo
        .get_place_record(trip_id, &candidate.place_id)
        .await?
        .ok_or(TripRepoError::CorruptData)?
        .value;
    if old == desired {
        return Ok(CandidateWithPlace { candidate, place });
    }
    let next_revision = stored
        .revision
        .checked_add(1)
        .ok_or(TripRepoError::CorruptData)?;
    candidate.status = desired;
    let change = audit(
        trip_id,
        actor,
        changed_at,
        change_id,
        AuditChange {
            entity: "candidate",
            entity_id: candidate_id,
            field: "status",
            old_value: json!(old),
            new_value: json!(candidate.status),
        },
    );
    let result = repo
        .client
        .transact_write_items()
        .transact_items(action_condition(member_condition(
            &repo.table_name,
            trip_id,
            actor,
            RequiredRole::Editor,
        )))
        .transact_items(action_put(revision_put(
            &repo.table_name,
            encode_record(
                trip_pk(trip_id),
                candidate_sk(candidate_id),
                CANDIDATE_ENTITY,
                &candidate,
                next_revision,
            )?,
            stored.revision,
        )))
        .transact_items(action_put(create_put(
            &repo.table_name,
            encode_record(
                trip_pk(trip_id),
                audit_sk(changed_at, change_id),
                AUDIT_ENTITY,
                &change,
                1,
            )?,
        )))
        .send()
        .await;
    if let Err(error) = result {
        if !transaction_condition_failed(error.as_service_error()) {
            return Err(TripRepoError::Unavailable);
        }
        repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
        return Err(TripRepoError::Conflict);
    }
    Ok(CandidateWithPlace { candidate, place })
}
