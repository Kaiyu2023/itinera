//! Direct-membership authorization and authoritative trip metadata loading.

use super::*;

impl DynamoUserRepo {
    pub(super) async fn get_member_record(
        &self,
        trip_id: &str,
        user_id: &UserId,
    ) -> Result<Option<Stored<TripMember>>, TripRepoError> {
        let pk = trip_pk(trip_id);
        let sk = member_sk(user_id);
        let Some(item) = self.trip_get(&pk, &sk).await? else {
            return Ok(None);
        };
        let stored: Stored<TripMember> = decode_record(&item, &pk, &sk, MEMBER_ENTITY)?;
        if stored.value.user_id != user_id.0
            || string(&item, USER_ID)? != user_id.0
            || string(&item, ROLE)? != role_value(stored.value.role)
            || string(&item, GSI1PK)? != user_partition_key(user_id)
            || string(&item, GSI1SK)? != format!("TRIP#{trip_id}")
        {
            return Err(TripRepoError::CorruptData);
        }
        Ok(Some(stored))
    }

    pub(super) async fn authorize(
        &self,
        trip_id: &str,
        actor: &UserId,
        required: RequiredRole,
    ) -> Result<TripRole, TripRepoError> {
        let role = self
            .get_member_record(trip_id, actor)
            .await?
            .ok_or(TripRepoError::NotFound)?
            .value
            .role;
        match required {
            RequiredRole::Any => Ok(role),
            RequiredRole::Editor if role.can_edit() => Ok(role),
            RequiredRole::Leader if role == TripRole::Leader => Ok(role),
            _ => Err(TripRepoError::Forbidden),
        }
    }

    pub(super) async fn get_trip_meta(
        &self,
        trip_id: &str,
    ) -> Result<Stored<TripMeta>, TripRepoError> {
        let pk = trip_pk(trip_id);
        let item = self
            .trip_get(&pk, META_SK)
            .await?
            .ok_or(TripRepoError::NotFound)?;
        let stored: Stored<TripMeta> = decode_record(&item, &pk, META_SK, TRIP_ENTITY)?;
        let current_id_matches = match &stored.value.current_plan_id {
            Some(id) => string(&item, CURRENT_PLAN_ID).is_ok_and(|stored_id| stored_id == *id),
            None => !item.contains_key(CURRENT_PLAN_ID),
        };
        let current_version_matches = match stored.value.current_plan_version {
            Some(version) => number_u64(&item, CURRENT_PLAN_VERSION) == Ok(version.into()),
            None => !item.contains_key(CURRENT_PLAN_VERSION),
        };
        if stored.value.id != trip_id
            || stored.value.member_count == 0
            || stored.value.leader_count == 0
            || number_u64(&item, MEMBER_COUNT) != Ok(stored.value.member_count.into())
            || number_u64(&item, LEADER_COUNT) != Ok(stored.value.leader_count.into())
            || !current_id_matches
            || !current_version_matches
        {
            return Err(TripRepoError::CorruptData);
        }
        Ok(stored)
    }
}
