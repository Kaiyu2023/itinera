//! Audit record construction shared by content mutations.

use itinera_core::domain::{
    content_history::{ChangeSource, Edit, EditEntity, EditStatus},
    user::UserId,
};
use serde_json::Value;

pub(in crate::dynamodb) struct AuditChange<'a> {
    pub(in crate::dynamodb) entity: &'a str,
    pub(in crate::dynamodb) entity_id: &'a str,
    pub(in crate::dynamodb) field: &'a str,
    pub(in crate::dynamodb) old_value: Value,
    pub(in crate::dynamodb) new_value: Value,
}

pub(in crate::dynamodb) fn audit(
    trip_id: &str,
    actor: &UserId,
    changed_at: &str,
    change_id: &str,
    change: AuditChange<'_>,
) -> Edit {
    Edit {
        id: change_id.to_string(),
        trip_id: trip_id.to_string(),
        entity: match change.entity {
            "stop" => EditEntity::Stop,
            "day" => EditEntity::Day,
            "candidate" => EditEntity::Candidate,
            "notice" => EditEntity::Notice,
            "trip" => EditEntity::Trip,
            _ => unreachable!("audit callers use the closed EditEntity vocabulary"),
        },
        entity_id: change.entity_id.to_string(),
        field: change.field.to_string(),
        old_value: change.old_value,
        new_value: change.new_value,
        author: actor.0.clone(),
        source: ChangeSource::Web {},
        status: EditStatus::Applied,
        created_at: changed_at.to_string(),
        reverted_by: None,
        reverted_at: None,
        revert_edit_id: None,
        reverts_edit_id: None,
    }
}

pub(in crate::dynamodb) fn suffixed_id(base: &str, index: usize) -> String {
    format!("{base}-{index:02}")
}
