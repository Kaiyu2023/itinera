//! Membership and invitation lifecycle operations.

use super::*;

impl DynamoUserRepo {
    async fn accept_invite_lookup(
        &self,
        user: &User,
        joined_at: &str,
        trip_id: &str,
        lookup_sort_key: &str,
        invite_sort_key: &str,
    ) -> Result<(), TripRepoError> {
        let lookup_pk = invitee_pk(&user.email);
        let trip_partition = trip_pk(trip_id);
        let expected_lookup_sk = invite_lookup_sk(trip_id);
        let expected_invite_sk = invite_sk(&user.email);
        if lookup_sort_key != expected_lookup_sk || invite_sort_key != expected_invite_sk {
            return Err(TripRepoError::CorruptData);
        }

        for _ in 0..INVITE_ACCEPT_ATTEMPTS {
            let Some(lookup_item) = self.trip_get(&lookup_pk, lookup_sort_key).await? else {
                // Another `/me` may have atomically accepted this invite after
                // the initial query. Confirm its terminal state rather than
                // surfacing a spurious conflict.
                if let Some(invite_item) = self.trip_get(&trip_partition, invite_sort_key).await? {
                    let invite: Stored<Invite> = decode_record(
                        &invite_item,
                        &trip_partition,
                        invite_sort_key,
                        INVITE_ENTITY,
                    )?;
                    if invite.value.trip_id != trip_id || invite.value.email != user.email.as_str()
                    {
                        return Err(TripRepoError::CorruptData);
                    }
                    if invite.value.status == InviteStatus::Accepted {
                        return Ok(());
                    }
                }
                // A simultaneous reinvite can recreate the lookup between the
                // two strong reads. Retry the complete state transition.
                continue;
            };
            let lookup: Stored<InviteLookup> = decode_record(
                &lookup_item,
                &lookup_pk,
                lookup_sort_key,
                INVITE_LOOKUP_ENTITY,
            )?;
            if lookup.value.trip_id != trip_id || lookup.value.invite_sort_key != invite_sort_key {
                return Err(TripRepoError::CorruptData);
            }

            let invite_item = self
                .trip_get(&trip_partition, invite_sort_key)
                .await?
                .ok_or(TripRepoError::CorruptData)?;
            let stored_invite: Stored<Invite> = decode_record(
                &invite_item,
                &trip_partition,
                invite_sort_key,
                INVITE_ENTITY,
            )?;
            if stored_invite.value.trip_id != trip_id
                || stored_invite.value.email != user.email.as_str()
            {
                return Err(TripRepoError::CorruptData);
            }
            if stored_invite.value.status == InviteStatus::Accepted {
                // This can only be a cross-item read racing the atomic delete;
                // the accepted invite is the authoritative terminal state.
                return Ok(());
            }

            let member = self.get_member_record(trip_id, &user.id).await?;
            let mut accepted = stored_invite.value;
            accepted.status = InviteStatus::Accepted;
            let delete_lookup = Delete::builder()
                .table_name(&self.table_name)
                .key(PK, AttributeValue::S(lookup_pk.clone()))
                .key(SK, AttributeValue::S(lookup_sort_key.to_string()))
                .condition_expression("#revision = :revision")
                .expression_attribute_names("#revision", REVISION)
                .expression_attribute_values(
                    ":revision",
                    AttributeValue::N(lookup.revision.to_string()),
                )
                .build()
                .expect("lookup delete is complete");

            let result = if member.is_some() {
                // Recheck that membership still exists in the same transaction;
                // otherwise retry through the membership-creation branch.
                self.client
                    .transact_write_items()
                    .transact_items(action_condition(member_condition(
                        &self.table_name,
                        trip_id,
                        &user.id,
                        RequiredRole::Any,
                    )))
                    .transact_items(action_put(revision_put(
                        &self.table_name,
                        encode_record(
                            trip_partition.clone(),
                            invite_sort_key.to_string(),
                            INVITE_ENTITY,
                            &accepted,
                            stored_invite.revision + 1,
                        )?,
                        stored_invite.revision,
                    )))
                    .transact_items(TransactWriteItem::builder().delete(delete_lookup).build())
                    .send()
                    .await
            } else {
                let stored_meta = self.get_trip_meta(trip_id).await?;
                let mut meta = stored_meta.value;
                meta.member_count = meta
                    .member_count
                    .checked_add(1)
                    .ok_or(TripRepoError::CorruptData)?;
                let member = TripMember {
                    user_id: user.id.0.clone(),
                    role: TripRole::Member,
                    joined_at: joined_at.to_string(),
                };
                self.client
                    .transact_write_items()
                    .transact_items(action_put(create_put(
                        &self.table_name,
                        encode_member(trip_id, &member)?,
                    )))
                    .transact_items(action_put(revision_put(
                        &self.table_name,
                        encode_trip_meta(&meta, stored_meta.revision + 1)?,
                        stored_meta.revision,
                    )))
                    .transact_items(action_put(revision_put(
                        &self.table_name,
                        encode_record(
                            trip_partition.clone(),
                            invite_sort_key.to_string(),
                            INVITE_ENTITY,
                            &accepted,
                            stored_invite.revision + 1,
                        )?,
                        stored_invite.revision,
                    )))
                    .transact_items(TransactWriteItem::builder().delete(delete_lookup).build())
                    .transact_items(action_update(user_membership_count_update(
                        &self.table_name,
                        &user.id,
                        true,
                    )))
                    .send()
                    .await
            };

            match result {
                Ok(_) => return Ok(()),
                Err(error) if transaction_condition_failed(error.as_service_error()) => {
                    // Membership, invite, lookup, or trip metadata changed.
                    // Re-read all of them; ordinary contention and concurrent
                    // `/me` requests must remain idempotent.
                }
                Err(_) => return Err(TripRepoError::Unavailable),
            }
        }

        // Persistent contention is retryable and `/me` documents 503, not a
        // domain-level 409 for account bootstrap.
        Err(TripRepoError::Unavailable)
    }

    pub(super) async fn get_members_for_trip(
        &self,
        trip_id: &str,
    ) -> Result<Vec<TripMember>, TripRepoError> {
        let pk = trip_pk(trip_id);
        self.query_partition(&pk, "MEMBER#", TRIP_COLLECTION_PAGE_SIZE)
            .await?
            .into_iter()
            .map(|item| {
                let sk = string(&item, SK)?;
                let stored: Stored<TripMember> = decode_record(&item, &pk, &sk, MEMBER_ENTITY)?;
                let user_id = UserId(stored.value.user_id.clone());
                if sk == member_sk(&user_id)
                    && string(&item, USER_ID)? == user_id.0
                    && string(&item, ROLE)? == role_value(stored.value.role)
                    && string(&item, GSI1PK)? == user_partition_key(&user_id)
                    && string(&item, GSI1SK)? == format!("TRIP#{trip_id}")
                {
                    Ok(stored.value)
                } else {
                    Err(TripRepoError::CorruptData)
                }
            })
            .collect()
    }
}

pub(super) async fn get_members(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    users: &dyn UserRepo,
) -> Result<Vec<User>, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Any).await?;
    let members = repo.get_members_for_trip(trip_id).await?;
    let mut result = Vec::with_capacity(members.len());
    for member in members {
        result.push(
            users
                .find_by_id(&UserId(member.user_id))
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
    Ok(result)
}

pub(super) async fn remove_member(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    target: &UserId,
) -> Result<(), TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Leader).await?;
    let target_member = repo
        .get_member_record(trip_id, target)
        .await?
        .ok_or(TripRepoError::NotFound)?;
    let stored_meta = repo.get_trip_meta(trip_id).await?;
    if target_member.value.role == TripRole::Leader && stored_meta.value.leader_count <= 1 {
        return Err(TripRepoError::Conflict);
    }
    let mut meta = stored_meta.value;
    meta.member_count = meta
        .member_count
        .checked_sub(1)
        .ok_or(TripRepoError::CorruptData)?;
    if target_member.value.role == TripRole::Leader {
        meta.leader_count = meta
            .leader_count
            .checked_sub(1)
            .ok_or(TripRepoError::CorruptData)?;
    }
    let mut tx = repo.client.transact_write_items();
    if actor != target {
        tx = tx.transact_items(action_condition(member_condition(
            &repo.table_name,
            trip_id,
            actor,
            RequiredRole::Leader,
        )));
    }
    let target_delete = Delete::builder()
        .table_name(&repo.table_name)
        .key(PK, AttributeValue::S(trip_pk(trip_id)))
        .key(SK, AttributeValue::S(member_sk(target)))
        .condition_expression("#entity = :member AND #role = :role")
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#role", ROLE)
        .expression_attribute_values(":member", AttributeValue::S(MEMBER_ENTITY.into()))
        .expression_attribute_values(
            ":role",
            AttributeValue::S(role_value(target_member.value.role).into()),
        )
        .build()
        .expect("delete is complete");
    tx = tx
        .transact_items(TransactWriteItem::builder().delete(target_delete).build())
        .transact_items(action_put(revision_put(
            &repo.table_name,
            encode_trip_meta(&meta, stored_meta.revision + 1)?,
            stored_meta.revision,
        )))
        .transact_items(action_update(user_membership_count_update(
            &repo.table_name,
            target,
            false,
        )));
    if let Err(error) = tx.send().await {
        if !transaction_condition_failed(error.as_service_error()) {
            return Err(TripRepoError::Unavailable);
        }
        repo.authorize(trip_id, actor, RequiredRole::Leader).await?;
        return Err(TripRepoError::Conflict);
    }
    Ok(())
}

pub(super) async fn create_invite(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    invite: Invite,
) -> Result<Invite, TripRepoError> {
    repo.authorize(trip_id, actor, RequiredRole::Leader).await?;
    let email = Email::parse(&invite.email).map_err(|_| TripRepoError::CorruptData)?;
    if invite.trip_id != trip_id
        || invite.email != email.as_str()
        || invite.invited_by != actor.0
        || invite.status != InviteStatus::Pending
    {
        return Err(TripRepoError::CorruptData);
    }
    let trip_sort_key = invite_sk(&email);
    let trip_partition = trip_pk(trip_id);
    let existing = repo
        .trip_get(&trip_partition, &trip_sort_key)
        .await?
        .map(|item| decode_record::<Invite>(&item, &trip_partition, &trip_sort_key, INVITE_ENTITY))
        .transpose()?;
    if existing
        .as_ref()
        .is_some_and(|stored| stored.value.status == InviteStatus::Pending)
    {
        return Err(TripRepoError::DuplicateInvite);
    }
    if existing.as_ref().is_some_and(|stored| {
        stored.value.trip_id != trip_id || stored.value.email != email.as_str()
    }) {
        return Err(TripRepoError::CorruptData);
    }
    let lookup = InviteLookup {
        trip_id: trip_id.to_string(),
        invite_sort_key: trip_sort_key.clone(),
    };
    let invite_item = encode_record(
        trip_partition,
        trip_sort_key,
        INVITE_ENTITY,
        &invite,
        existing.as_ref().map_or(1, |stored| stored.revision + 1),
    )?;
    let invite_put = match existing {
        Some(stored) => revision_put(&repo.table_name, invite_item, stored.revision),
        None => create_put(&repo.table_name, invite_item),
    };
    let result = repo
        .client
        .transact_write_items()
        .transact_items(action_condition(member_condition(
            &repo.table_name,
            trip_id,
            actor,
            RequiredRole::Leader,
        )))
        .transact_items(action_put(invite_put))
        .transact_items(action_put(create_put(
            &repo.table_name,
            encode_record(
                invitee_pk(&email),
                invite_lookup_sk(trip_id),
                INVITE_LOOKUP_ENTITY,
                &lookup,
                1,
            )?,
        )))
        .send()
        .await;
    match result {
        Ok(_) => Ok(invite),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.authorize(trip_id, actor, RequiredRole::Leader).await?;
            let current = repo
                .trip_get(&trip_pk(trip_id), &invite_sk(&email))
                .await?
                .map(|item| {
                    decode_record::<Invite>(
                        &item,
                        &trip_pk(trip_id),
                        &invite_sk(&email),
                        INVITE_ENTITY,
                    )
                })
                .transpose()?;
            if current.is_some_and(|stored| stored.value.status == InviteStatus::Pending) {
                Err(TripRepoError::DuplicateInvite)
            } else {
                Err(TripRepoError::Conflict)
            }
        }
        Err(_) => Err(TripRepoError::Unavailable),
    }
}

pub(super) async fn accept_pending_invites(
    repo: &DynamoUserRepo,
    user: &User,
    joined_at: &str,
) -> Result<(), TripRepoError> {
    let lookup_pk = invitee_pk(&user.email);
    let lookups = repo
        .query_partition(&lookup_pk, "TRIP#", USER_TRIPS_PAGE_SIZE)
        .await?;
    for item in lookups {
        let lookup_sk = string(&item, SK)?;
        let lookup: Stored<InviteLookup> =
            decode_record(&item, &lookup_pk, &lookup_sk, INVITE_LOOKUP_ENTITY)?;
        repo.accept_invite_lookup(
            user,
            joined_at,
            &lookup.value.trip_id,
            &lookup_sk,
            &lookup.value.invite_sort_key,
        )
        .await?;
    }
    Ok(())
}
