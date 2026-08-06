//! Explicitly allowlisted, atomic, stale-safe content reverts.

use aws_sdk_dynamodb::types::TransactWriteItem;

use super::{access::*, audit::*, *};

struct TargetPlan {
    actions: Vec<TransactWriteItem>,
}

const HISTORY_SLOT_ENTITY: &str = "CONTENT_HISTORY_SLOT";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistorySlot {
    record_count: u32,
}

pub(super) async fn revert_edit(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    edit_id: &str,
    reverted_at: &str,
    compensating_edit_id: &str,
) -> Result<(), ContentHistoryRepoError> {
    repo.history_authorize(trip_id, actor, RequiredHistoryRole::Editor)
        .await?;
    if !valid_edit_id(edit_id)
        || !valid_edit_id(compensating_edit_id)
        || edit_id == compensating_edit_id
        || !valid_utc_timestamp(reverted_at)
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }

    let lookup = find_audit_record(repo, trip_id, edit_id).await?;
    let original = lookup.record.ok_or(ContentHistoryRepoError::NotFound)?;
    match original.value.status {
        // Repeating the same server-owned command is a successful no-op. The
        // first transaction's actor and compensation remain authoritative.
        EditStatus::Reverted => return Ok(()),
        EditStatus::Applied => {}
        EditStatus::PendingReview | EditStatus::Rejected => {
            return Err(ContentHistoryRepoError::Conflict);
        }
    }
    if lookup.record_count >= MAX_HISTORY_RECORDS {
        return Err(ContentHistoryRepoError::SafetyLimitExceeded);
    }
    if original.value.old_value == original.value.new_value {
        return Err(ContentHistoryRepoError::CorruptData);
    }

    let target = build_target_plan(repo, trip_id, &original.value).await?;
    let reverted = reverted_original(&original.value, actor, reverted_at, compensating_edit_id);
    let compensation = compensating_edit(&original.value, actor, reverted_at, compensating_edit_id);
    let pk = trip_pk(trip_id);
    let reverted_item = encode_record(
        pk.clone(),
        original.sort_key.clone(),
        AUDIT_ENTITY,
        &reverted,
        next_revision(original.revision)?,
    )
    .map_err(record_error)?;
    let compensation_item = encode_record(
        pk,
        audit_sk(reverted_at, compensating_edit_id),
        AUDIT_ENTITY,
        &compensation,
        1,
    )
    .map_err(record_error)?;
    let reserved_count = lookup
        .record_count
        .checked_add(1)
        .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
    let reservation_item = encode_record(
        trip_pk(trip_id),
        history_slot_sk(reserved_count),
        HISTORY_SLOT_ENTITY,
        &HistorySlot {
            record_count: u32::try_from(reserved_count)
                .map_err(|_| ContentHistoryRepoError::SafetyLimitExceeded)?,
        },
        1,
    )
    .map_err(record_error)?;
    let reverted_bytes = encoded_item_bytes(&reverted_item)?;
    let compensation_bytes = encoded_item_bytes(&compensation_item)?;
    let projected_bytes = lookup
        .encoded_bytes
        .checked_sub(original.encoded_bytes)
        .and_then(|bytes| bytes.checked_add(reverted_bytes))
        .and_then(|bytes| bytes.checked_add(compensation_bytes))
        .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
    if projected_bytes > MAX_HISTORY_BYTES {
        return Err(ContentHistoryRepoError::SafetyLimitExceeded);
    }

    let mut transaction = repo
        .client
        .transact_write_items()
        .transact_items(condition_action(editor_membership_condition(
            &repo.table_name,
            trip_id,
            actor,
        )));
    for action in target.actions {
        transaction = transaction.transact_items(action);
    }
    transaction = transaction
        .transact_items(put_action(conditional_record_put(
            &repo.table_name,
            reverted_item,
            AUDIT_ENTITY,
            original.revision,
            &original.raw_data,
        )))
        .transact_items(put_action(create_record_put(
            &repo.table_name,
            compensation_item,
        )))
        // Reverts that observed the same history length compete for the same
        // permanent create-only slot. Only one may append; a loser reloads the
        // bounded history before a caller retries, so distinct concurrent
        // reverts cannot race past the row or byte ceiling.
        .transact_items(put_action(create_record_put(
            &repo.table_name,
            reservation_item,
        )));

    let result = transaction.send().await;
    let Err(error) = result else {
        return Ok(());
    };

    // A cancellation can mean role revocation, a stale entity, a concurrent
    // revert, or an extremely unlikely generated-id collision. Re-read the
    // two authoritative records rather than translating all of those into one
    // misleading outcome. This also recovers an ambiguous SDK response after
    // DynamoDB committed the transaction.
    repo.history_authorize(trip_id, actor, RequiredHistoryRole::Editor)
        .await?;
    if find_audit_record(repo, trip_id, edit_id)
        .await?
        .record
        .is_some_and(|stored| stored.value.status == EditStatus::Reverted)
    {
        return Ok(());
    }
    if transaction_condition_failed(error.as_service_error()) {
        Err(ContentHistoryRepoError::Conflict)
    } else {
        Err(ContentHistoryRepoError::Unavailable)
    }
}

fn history_slot_sk(record_count: usize) -> String {
    format!("HISTORY#SLOT#{record_count:010}")
}

async fn build_target_plan(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    match edit.entity {
        EditEntity::Trip if edit.field == "status" => revert_trip_status(repo, trip_id, edit).await,
        EditEntity::Candidate => revert_candidate(repo, trip_id, edit).await,
        EditEntity::Day => revert_day(repo, trip_id, edit).await,
        EditEntity::Stop => revert_stop(repo, trip_id, edit).await,
        // Notice author/leader rules cannot be safely inferred by this generic
        // repository before the notice capability exists.
        EditEntity::Notice | EditEntity::Trip => Err(ContentHistoryRepoError::Unsupported),
    }
}

async fn revert_trip_status(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    if edit.entity_id != trip_id {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let old: TripStatus = parse_exact(&edit.old_value)?;
    let new: TripStatus = parse_exact(&edit.new_value)?;
    let mut meta = repo.history_trip_meta(trip_id).await?;
    if meta.value.status != new {
        return Err(ContentHistoryRepoError::Conflict);
    }
    meta.value.status = old;
    let item =
        encode_trip_meta(&meta.value, next_revision(meta.revision)?).map_err(record_error)?;
    Ok(TargetPlan {
        actions: vec![put_action(conditional_record_put(
            &repo.table_name,
            item,
            TRIP_ENTITY,
            meta.revision,
            &meta.raw_data,
        ))],
    })
}

async fn revert_candidate(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    let pk = trip_pk(trip_id);
    let sk = candidate_sk(&edit.entity_id);
    let item = repo
        .history_get(&pk, &sk)
        .await?
        .ok_or(ContentHistoryRepoError::Conflict)?;
    let mut candidate: Loaded<Candidate> = decode_loaded(&item, &pk, &sk, CANDIDATE_ENTITY)?;
    if candidate.value.id != edit.entity_id || candidate.value.trip_id != trip_id {
        return Err(ContentHistoryRepoError::CorruptData);
    }

    let mut guards = Vec::new();
    match edit.field.as_str() {
        "status" => {
            let old: CandidateStatus = parse_exact(&edit.old_value)?;
            let new: CandidateStatus = parse_exact(&edit.new_value)?;
            if old == CandidateStatus::InPlan || new == CandidateStatus::InPlan {
                return Err(ContentHistoryRepoError::Unsupported);
            }
            if candidate.value.status != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            candidate.value.status = old;
        }
        "pitch" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_required_text(&old, 2_000)?;
            validate_required_text(&new, 2_000)?;
            if candidate.value.pitch != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            candidate.value.pitch = old;
        }
        "tags" => {
            let old: Vec<String> = parse_exact(&edit.old_value)?;
            let new: Vec<String> = parse_exact(&edit.new_value)?;
            validate_tags(&old)?;
            validate_tags(&new)?;
            if candidate.value.tags != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            candidate.value.tags = old;
        }
        "place" => {
            let old: Place = parse_exact(&edit.old_value)?;
            let new: Place = parse_exact(&edit.new_value)?;
            validate_place(&old)?;
            validate_place(&new)?;
            if old.id == new.id || candidate.value.place_id != new.id {
                return Err(ContentHistoryRepoError::CorruptData);
            }
            let current = load_place(repo, trip_id, &new.id).await?;
            let previous = load_place(repo, trip_id, &old.id).await?;
            if current.value != new || previous.value != old {
                return Err(ContentHistoryRepoError::CorruptData);
            }
            guards.push(condition_action(record_guard(
                &repo.table_name,
                trip_pk(trip_id),
                current.sort_key,
                PLACE_ENTITY,
                current.revision,
                &current.raw_data,
            )));
            guards.push(condition_action(record_guard(
                &repo.table_name,
                trip_pk(trip_id),
                previous.sort_key,
                PLACE_ENTITY,
                previous.revision,
                &previous.raw_data,
            )));
            candidate.value.place_id = old.id;
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }

    let encoded = encode_record(
        pk,
        candidate.sort_key,
        CANDIDATE_ENTITY,
        &candidate.value,
        next_revision(candidate.revision)?,
    )
    .map_err(record_error)?;
    let mut actions = vec![put_action(conditional_record_put(
        &repo.table_name,
        encoded,
        CANDIDATE_ENTITY,
        candidate.revision,
        &candidate.raw_data,
    ))];
    actions.extend(guards);
    Ok(TargetPlan { actions })
}

async fn load_place(
    repo: &DynamoUserRepo,
    trip_id: &str,
    place_id: &str,
) -> Result<Loaded<Place>, ContentHistoryRepoError> {
    let pk = trip_pk(trip_id);
    let sk = place_sk(place_id);
    let item = repo
        .history_get(&pk, &sk)
        .await?
        .ok_or(ContentHistoryRepoError::CorruptData)?;
    let place = decode_loaded::<Place>(&item, &pk, &sk, PLACE_ENTITY)?;
    if place.value.id != place_id {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    Ok(place)
}

async fn revert_day(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    let mut meta = repo.history_trip_meta(trip_id).await?;
    let version = meta
        .value
        .current_plan_version
        .ok_or(ContentHistoryRepoError::Conflict)?;
    let plan_id = meta
        .value
        .current_plan_id
        .clone()
        .ok_or(ContentHistoryRepoError::CorruptData)?;
    let pk = trip_pk(trip_id);
    let mut days = Vec::new();
    let mut day_ids = HashSet::new();
    let mut nested_stop_day_ids = Vec::new();
    let mut target_index = None;
    for item in repo
        .history_query(
            &pk,
            &format!("{}#DAY#", plan_prefix(version)),
            TRIP_COLLECTION_PAGE_SIZE,
            false,
            MAX_REVERT_PLAN_RECORDS,
            MAX_REVERT_PLAN_BYTES,
        )
        .await?
    {
        let sk = string(&item, SK).map_err(record_error)?;
        match string(&item, ENTITY_TYPE).map_err(record_error)?.as_str() {
            DAY_ENTITY => {
                let day = decode_loaded::<Day>(&item, &pk, &sk, DAY_ENTITY)?;
                if day.value.plan_id != plan_id || !day_ids.insert(day.value.id.clone()) {
                    return Err(ContentHistoryRepoError::CorruptData);
                }
                if day.value.id == edit.entity_id {
                    target_index = Some(days.len());
                }
                days.push(day);
            }
            STOP_ENTITY => {
                // Stop keys are nested below the DAY prefix. Validate their
                // envelope rather than mistaking them for day rows, then keep
                // them out of the city/window aggregate.
                let stop = decode_loaded::<Stop>(&item, &pk, &sk, STOP_ENTITY)?;
                if stop.value.id.is_empty() || stop.value.day_id.is_empty() {
                    return Err(ContentHistoryRepoError::CorruptData);
                }
                nested_stop_day_ids.push(stop.value.day_id);
            }
            _ => {
                return Err(ContentHistoryRepoError::CorruptData);
            }
        }
    }
    if nested_stop_day_ids
        .iter()
        .any(|day_id| !day_ids.contains(day_id))
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let index = target_index.ok_or(ContentHistoryRepoError::Conflict)?;
    let target = &mut days[index];
    match edit.field.as_str() {
        "windowStart" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_local_time(&old)?;
            validate_local_time(&new)?;
            if target.value.window_start != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            target.value.window_start = old;
        }
        "windowEnd" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_local_time(&old)?;
            validate_local_time(&new)?;
            if target.value.window_end != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            target.value.window_end = old;
        }
        "cityHint" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_required_text(&old, 120)?;
            validate_required_text(&new, 120)?;
            if target.value.city_hint != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            target.value.city_hint = old;
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }
    validate_local_time(&target.value.window_start)?;
    validate_local_time(&target.value.window_end)?;
    if canonical_time_window(&target.value.window_start, &target.value.window_end).is_err() {
        return Err(ContentHistoryRepoError::Conflict);
    }

    let day_item = encode_record(
        pk.clone(),
        target.sort_key.clone(),
        DAY_ENTITY,
        &target.value,
        next_revision(target.revision)?,
    )
    .map_err(record_error)?;
    let mut actions = vec![put_action(conditional_record_put(
        &repo.table_name,
        day_item,
        DAY_ENTITY,
        target.revision,
        &target.raw_data,
    ))];
    if edit.field == "cityHint" {
        days.sort_by(|left, right| left.value.date.cmp(&right.value.date));
        let mut seen = HashSet::new();
        meta.value.cities = days
            .into_iter()
            .map(|day| day.value.city_hint)
            .filter(|city| seen.insert(city.clone()))
            .collect();
        let meta_item =
            encode_trip_meta(&meta.value, next_revision(meta.revision)?).map_err(record_error)?;
        actions.push(put_action(conditional_record_put(
            &repo.table_name,
            meta_item,
            TRIP_ENTITY,
            meta.revision,
            &meta.raw_data,
        )));
    } else {
        actions.push(condition_action(record_guard(
            &repo.table_name,
            pk,
            META_SK.into(),
            TRIP_ENTITY,
            meta.revision,
            &meta.raw_data,
        )));
    }
    Ok(TargetPlan { actions })
}

async fn revert_stop(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    let meta = repo.history_trip_meta(trip_id).await?;
    let version = meta
        .value
        .current_plan_version
        .ok_or(ContentHistoryRepoError::Conflict)?;
    let plan_id = meta
        .value
        .current_plan_id
        .as_ref()
        .ok_or(ContentHistoryRepoError::CorruptData)?;
    let pk = trip_pk(trip_id);
    let items = repo
        .history_query(
            &pk,
            &format!("{}#", plan_prefix(version)),
            TRIP_COLLECTION_PAGE_SIZE,
            false,
            MAX_REVERT_PLAN_RECORDS,
            MAX_REVERT_PLAN_BYTES,
        )
        .await?;
    let mut day_ids = HashSet::new();
    let mut target = None;
    for item in items {
        let sk = string(&item, SK).map_err(record_error)?;
        match string(&item, ENTITY_TYPE).map_err(record_error)?.as_str() {
            DAY_ENTITY => {
                let day = decode_loaded::<Day>(&item, &pk, &sk, DAY_ENTITY)?;
                if &day.value.plan_id != plan_id || !day_ids.insert(day.value.id) {
                    return Err(ContentHistoryRepoError::CorruptData);
                }
            }
            STOP_ENTITY => {
                let stop = decode_loaded::<Stop>(&item, &pk, &sk, STOP_ENTITY)?;
                if stop.value.id == edit.entity_id {
                    if target.is_some() {
                        return Err(ContentHistoryRepoError::CorruptData);
                    }
                    target = Some(stop);
                }
            }
            "PLAN" => {}
            _ => return Err(ContentHistoryRepoError::CorruptData),
        }
    }
    let mut stop = target.ok_or(ContentHistoryRepoError::Conflict)?;
    if !day_ids.contains(&stop.value.day_id) {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    match edit.field.as_str() {
        "plannedArrival" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_local_time(&old)?;
            validate_local_time(&new)?;
            if stop.value.planned_arrival != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.value.planned_arrival = old;
        }
        "durationMin" => {
            let old: u32 = parse_exact(&edit.old_value)?;
            let new: u32 = parse_exact(&edit.new_value)?;
            validate_duration(old)?;
            validate_duration(new)?;
            if stop.value.duration_min != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.value.duration_min = old;
        }
        "notes" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_text_len(&old, 10_000)?;
            validate_text_len(&new, 10_000)?;
            if stop.value.notes != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.value.notes = old;
        }
        "booking" => {
            let old: Option<Booking> = parse_exact(&edit.old_value)?;
            let new: Option<Booking> = parse_exact(&edit.new_value)?;
            validate_booking(old.as_ref())?;
            validate_booking(new.as_ref())?;
            if stop.value.booking != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.value.booking = old;
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }
    validate_local_time(&stop.value.planned_arrival)?;
    validate_duration(stop.value.duration_min)?;
    validate_text_len(&stop.value.notes, 10_000)?;
    validate_booking(stop.value.booking.as_ref())?;

    let stop_item = encode_record(
        pk.clone(),
        stop.sort_key,
        STOP_ENTITY,
        &stop.value,
        next_revision(stop.revision)?,
    )
    .map_err(record_error)?;
    Ok(TargetPlan {
        actions: vec![
            put_action(conditional_record_put(
                &repo.table_name,
                stop_item,
                STOP_ENTITY,
                stop.revision,
                &stop.raw_data,
            )),
            condition_action(record_guard(
                &repo.table_name,
                pk,
                META_SK.into(),
                TRIP_ENTITY,
                meta.revision,
                &meta.raw_data,
            )),
        ],
    })
}

fn parse_exact<T>(value: &Value) -> Result<T, ContentHistoryRepoError>
where
    T: DeserializeOwned + Serialize,
{
    let parsed = serde_json::from_value::<T>(value.clone())
        .map_err(|_| ContentHistoryRepoError::CorruptData)?;
    let canonical =
        serde_json::to_value(&parsed).map_err(|_| ContentHistoryRepoError::CorruptData)?;
    if canonical != *value {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    Ok(parsed)
}

pub(super) fn validate_required_text(
    value: &str,
    max: usize,
) -> Result<(), ContentHistoryRepoError> {
    canonical_required_text(value, "stored text must be normalized", max)
        .map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_text_len(value: &str, max: usize) -> Result<(), ContentHistoryRepoError> {
    canonical_text_len(value, max).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_tags(tags: &[String]) -> Result<(), ContentHistoryRepoError> {
    canonical_bounded_strings(tags, 20, 60).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_local_time(value: &str) -> Result<(), ContentHistoryRepoError> {
    canonical_local_time(value).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_duration(value: u32) -> Result<(), ContentHistoryRepoError> {
    canonical_duration_min(value).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_booking(booking: Option<&Booking>) -> Result<(), ContentHistoryRepoError> {
    canonical_booking(booking).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_place(place: &Place) -> Result<(), ContentHistoryRepoError> {
    canonical_place(place).map_err(|_| ContentHistoryRepoError::CorruptData)
}

fn conditional_record_put(
    table_name: &str,
    item: HashMap<String, AttributeValue>,
    entity: &str,
    expected_revision: u64,
    expected_data: &str,
) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression(
            "#entity = :entity AND #revision = :expected_revision AND #data = :expected_data",
        )
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#revision", REVISION)
        .expression_attribute_names("#data", DATA)
        .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
        .expression_attribute_values(
            ":expected_revision",
            AttributeValue::N(expected_revision.to_string()),
        )
        .expression_attribute_values(
            ":expected_data",
            AttributeValue::S(expected_data.to_string()),
        )
        .build()
        .expect("conditional record replacement is complete")
}

fn record_guard(
    table_name: &str,
    partition_key: String,
    sort_key: String,
    entity: &str,
    expected_revision: u64,
    expected_data: &str,
) -> ConditionCheck {
    ConditionCheck::builder()
        .table_name(table_name)
        .key(PK, AttributeValue::S(partition_key))
        .key(SK, AttributeValue::S(sort_key))
        .condition_expression(
            "#entity = :entity AND #revision = :expected_revision AND #data = :expected_data",
        )
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#revision", REVISION)
        .expression_attribute_names("#data", DATA)
        .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
        .expression_attribute_values(
            ":expected_revision",
            AttributeValue::N(expected_revision.to_string()),
        )
        .expression_attribute_values(
            ":expected_data",
            AttributeValue::S(expected_data.to_string()),
        )
        .build()
        .expect("record guard is complete")
}

fn create_record_put(table_name: &str, item: HashMap<String, AttributeValue>) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
        .expression_attribute_names("#pk", PK)
        .expression_attribute_names("#sk", SK)
        .build()
        .expect("create-only audit put is complete")
}

fn transaction_condition_failed(error: Option<&TransactWriteItemsError>) -> bool {
    let Some(TransactWriteItemsError::TransactionCanceledException(cancellation)) = error else {
        return false;
    };
    let mut saw_condition = false;
    for reason in cancellation.cancellation_reasons() {
        match reason.code() {
            None | Some("None") => {}
            Some(CONDITIONAL_FAILURE) => saw_condition = true,
            Some(_) => return false,
        }
    }
    saw_condition
}

fn put_action(put: Put) -> TransactWriteItem {
    TransactWriteItem::builder().put(put).build()
}

fn condition_action(condition: ConditionCheck) -> TransactWriteItem {
    TransactWriteItem::builder()
        .condition_check(condition)
        .build()
}

fn next_revision(current: u64) -> Result<u64, ContentHistoryRepoError> {
    current
        .checked_add(1)
        .ok_or(ContentHistoryRepoError::CorruptData)
}
