use crate::{
    domain::content_history::Edit,
    ports::{
        authorization::TripAuthorizationContext,
        clock::Clock,
        content_history::{ContentHistoryRepo, ContentHistoryRepoError},
        id_gen::IdGen,
    },
};

use super::validation::{ValidationError, required_text};

#[derive(Debug, thiserror::Error)]
pub enum ContentHistoryServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] ContentHistoryRepoError),
}

pub async fn get_history(
    repo: &dyn ContentHistoryRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<Edit>, ContentHistoryServiceError> {
    repo.list_history(trip_id, authorization)
        .await
        .map_err(Into::into)
}

pub async fn revert_edit(
    repo: &dyn ContentHistoryRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    edit_id: &str,
) -> Result<(), ContentHistoryServiceError> {
    if authorization.human_user_id().is_none() {
        return Err(ContentHistoryRepoError::Forbidden.into());
    }
    let edit_id = required_text(
        edit_id.to_string(),
        "editId is required and must be at most 200 characters",
        200,
    )?;
    repo.revert_edit(
        trip_id,
        authorization,
        &edit_id,
        &clock.now(),
        &ids.new_id(),
    )
    .await
    .map_err(Into::into)
}
