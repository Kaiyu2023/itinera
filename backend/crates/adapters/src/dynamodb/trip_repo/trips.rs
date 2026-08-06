//! Trip lifecycle operations.

use super::*;

pub(super) async fn create_trip(repo: &DynamoUserRepo, trip: Trip) -> Result<Trip, TripRepoError> {
    if trip.members.len() != 1 || trip.members[0].role != TripRole::Leader {
        return Err(TripRepoError::CorruptData);
    }
    let meta = TripMeta::from_trip(&trip);
    let actor = UserId(trip.members[0].user_id.clone());
    let result = repo
        .client
        .transact_write_items()
        .transact_items(action_put(create_put(
            &repo.table_name,
            encode_trip_meta(&meta, 1)?,
        )))
        .transact_items(action_put(create_put(
            &repo.table_name,
            encode_member(&trip.id, &trip.members[0])?,
        )))
        .transact_items(action_update(user_membership_count_update(
            &repo.table_name,
            &actor,
            true,
        )))
        .send()
        .await;
    match result {
        Ok(_) => Ok(trip),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            Err(TripRepoError::Conflict)
        }
        Err(_) => Err(TripRepoError::Unavailable),
    }
}

pub(super) async fn list_trips(
    repo: &DynamoUserRepo,
    actor: &UserId,
) -> Result<Vec<TripSummary>, TripRepoError> {
    let mut index_items = Vec::new();
    let mut cursor = None;
    loop {
        let output = repo
            .client
            .query()
            .table_name(&repo.table_name)
            .index_name(GSI_NAME)
            .key_condition_expression("#gsi_pk = :user AND begins_with(#gsi_sk, :trip)")
            .expression_attribute_names("#gsi_pk", GSI1PK)
            .expression_attribute_names("#gsi_sk", GSI1SK)
            .expression_attribute_values(":user", AttributeValue::S(user_partition_key(actor)))
            .expression_attribute_values(":trip", AttributeValue::S("TRIP#".into()))
            .limit(USER_TRIPS_PAGE_SIZE)
            .set_exclusive_start_key(cursor)
            .send()
            .await
            .map_err(|_| TripRepoError::Unavailable)?;
        let next = output
            .last_evaluated_key()
            .filter(|key| !key.is_empty())
            .cloned();
        index_items.extend(output.items.unwrap_or_default());
        let Some(next) = next else {
            break;
        };
        cursor = Some(next);
    }
    let mut trip_ids = index_items
        .iter()
        .map(|item| string(item, GSI1SK))
        .collect::<Result<Vec<_>, _>>()?;
    trip_ids.sort();
    trip_ids.dedup();
    let mut summaries = Vec::new();
    for value in trip_ids {
        let Some(trip_id) = value.strip_prefix("TRIP#") else {
            return Err(TripRepoError::CorruptData);
        };
        // The GSI is navigation only. A stale row after revocation is
        // discarded by this strongly consistent direct read.
        if repo.get_member_record(trip_id, actor).await?.is_none() {
            continue;
        }
        summaries.push(repo.get_trip_meta(trip_id).await?.value.summary());
    }
    summaries.sort_by(|a, b| {
        a.start_date
            .cmp(&b.start_date)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(summaries)
}

pub(super) async fn get_trip(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Trip, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Any).await?;
    let meta = repo.get_trip_meta(trip_id).await?.value;
    let members = repo.get_members_for_trip(trip_id).await?;
    if members.len() as u32 != meta.member_count
        || members
            .iter()
            .filter(|member| member.role == TripRole::Leader)
            .count() as u32
            != meta.leader_count
    {
        return Err(TripRepoError::CorruptData);
    }
    Ok(meta.into_trip(members))
}

pub(super) async fn set_trip_status(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    status: TripStatus,
    changed_at: &str,
    change_id: &str,
) -> Result<Trip, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Editor).await?;
    let stored = repo.get_trip_meta(trip_id).await?;
    if stored.value.status == status {
        return repo.get_trip(trip_id, actor).await;
    }
    let mut meta = stored.value;
    let old = meta.status;
    meta.status = status;
    let audit = audit(
        trip_id,
        actor,
        changed_at,
        change_id,
        AuditChange {
            entity: "trip",
            entity_id: trip_id,
            field: "status",
            old_value: json!(old),
            new_value: json!(status),
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
            encode_trip_meta(&meta, stored.revision + 1)?,
            stored.revision,
        )))
        .transact_items(action_put(create_put(
            &repo.table_name,
            encode_record(
                trip_pk(trip_id),
                audit_sk(changed_at, change_id),
                AUDIT_ENTITY,
                &audit,
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
    repo.get_trip(trip_id, actor).await
}
