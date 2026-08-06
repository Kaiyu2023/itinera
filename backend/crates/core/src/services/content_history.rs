use crate::{
    domain::{content_history::Edit, user::UserId},
    ports::{
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
    actor: &UserId,
) -> Result<Vec<Edit>, ContentHistoryServiceError> {
    repo.list_history(trip_id, actor).await.map_err(Into::into)
}

pub async fn revert_edit(
    repo: &dyn ContentHistoryRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    edit_id: &str,
) -> Result<(), ContentHistoryServiceError> {
    let edit_id = required_text(
        edit_id.to_string(),
        "editId is required and must be at most 200 characters",
        200,
    )?;
    repo.revert_edit(trip_id, actor, &edit_id, &clock.now(), &ids.new_id())
        .await
        .map_err(Into::into)
}
