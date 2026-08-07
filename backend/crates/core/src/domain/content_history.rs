use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditEntity {
    Stop,
    Day,
    Candidate,
    Notice,
    Trip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditStatus {
    Applied,
    PendingReview,
    Rejected,
    Reverted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "via",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ChangeSource {
    Web {},
    Service {
        service_identity_id: String,
        service_identity_name: String,
    },
}

/// One immutable field-level content change.
///
/// A revert never rewrites the original values. It changes the original
/// event's status and provenance, then appends a compensating `Edit` whose
/// `reverts_edit_id` points back to it. The defaults keep schema-version 1
/// audit rows written before safe revert readable; API serialization still
/// emits the four fields as `null` when they do not apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edit {
    pub id: String,
    pub trip_id: String,
    pub entity: EditEntity,
    pub entity_id: String,
    pub field: String,
    pub old_value: Value,
    pub new_value: Value,
    pub author: String,
    pub source: ChangeSource,
    pub status: EditStatus,
    pub created_at: String,
    #[serde(default)]
    pub reverted_by: Option<String>,
    #[serde(default)]
    pub reverted_at: Option<String>,
    #[serde(default)]
    pub revert_edit_id: Option<String>,
    #[serde(default)]
    pub reverts_edit_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn change_source_matches_the_discriminated_wire_contract() {
        assert_eq!(
            serde_json::to_value(ChangeSource::Web {}).expect("web source serializes"),
            json!({"via": "web"})
        );
        assert_eq!(
            serde_json::to_value(ChangeSource::Service {
                service_identity_id: "service-a".into(),
                service_identity_name: "assistant".into(),
            })
            .expect("service source serializes"),
            json!({
                "via": "service",
                "serviceIdentityId": "service-a",
                "serviceIdentityName": "assistant"
            })
        );
    }

    #[test]
    fn legacy_audit_rows_default_new_provenance_to_null() {
        let edit: Edit = serde_json::from_value(json!({
            "id": "edit-a",
            "tripId": "trip-a",
            "entity": "trip",
            "entityId": "trip-a",
            "field": "status",
            "oldValue": "dreaming",
            "newValue": "planning",
            "author": "user-a",
            "source": {"via": "web"},
            "status": "applied",
            "createdAt": "2026-08-06T12:00:00Z"
        }))
        .expect("schema-version 1 audit row should decode");

        assert_eq!(edit.reverted_by, None);
        assert_eq!(edit.reverted_at, None);
        assert_eq!(edit.revert_edit_id, None);
        assert_eq!(edit.reverts_edit_id, None);
        let encoded = serde_json::to_value(edit).expect("edit serializes");
        assert_eq!(encoded["revertedBy"], Value::Null);
        assert_eq!(encoded["revertedAt"], Value::Null);
        assert_eq!(encoded["revertEditId"], Value::Null);
        assert_eq!(encoded["revertsEditId"], Value::Null);
    }

    #[test]
    fn audit_rows_reject_unknown_top_level_fields() {
        let error = serde_json::from_value::<Edit>(json!({
            "id": "edit-a",
            "tripId": "trip-a",
            "entity": "trip",
            "entityId": "trip-a",
            "field": "status",
            "oldValue": "dreaming",
            "newValue": "planning",
            "author": "user-a",
            "source": {"via": "web"},
            "status": "applied",
            "createdAt": "2026-08-06T12:00:00Z",
            "callerSelectedReplacement": "done"
        }))
        .expect_err("unknown stored fields must fail closed");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn audit_rows_reject_unknown_change_source_fields() {
        let error = serde_json::from_value::<Edit>(json!({
            "id": "edit-a",
            "tripId": "trip-a",
            "entity": "trip",
            "entityId": "trip-a",
            "field": "status",
            "oldValue": "dreaming",
            "newValue": "planning",
            "author": "user-a",
            "source": {"via": "web", "forgedServiceIdentityId": "service-a"},
            "status": "applied",
            "createdAt": "2026-08-06T12:00:00Z"
        }))
        .expect_err("unknown nested source fields must fail closed");

        assert!(error.to_string().contains("unknown field"));
    }
}
