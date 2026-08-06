//! Audit record construction shared by content mutations.

use super::*;

pub(super) struct AuditChange<'a> {
    pub(super) entity: &'a str,
    pub(super) entity_id: &'a str,
    pub(super) field: &'a str,
    pub(super) old_value: Value,
    pub(super) new_value: Value,
}

pub(super) fn audit(
    trip_id: &str,
    actor: &UserId,
    changed_at: &str,
    change_id: &str,
    change: AuditChange<'_>,
) -> ContentAudit {
    ContentAudit {
        id: change_id.to_string(),
        trip_id: trip_id.to_string(),
        entity: change.entity.to_string(),
        entity_id: change.entity_id.to_string(),
        field: change.field.to_string(),
        old_value: change.old_value,
        new_value: change.new_value,
        author: actor.0.clone(),
        source: AuditSource { via: "web".into() },
        status: "applied".into(),
        created_at: changed_at.to_string(),
    }
}

pub(super) fn suffixed_id(base: &str, index: usize) -> String {
    format!("{base}-{index:02}")
}
