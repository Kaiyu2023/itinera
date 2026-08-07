use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeCategory {
    Visa,
    Safety,
    Health,
    Money,
    Connectivity,
    Packing,
    Custom,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeStatus {
    #[default]
    Active,
    Resolved,
    Archived,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistMode {
    #[default]
    Each,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChecklistItem {
    pub id: String,
    pub text: String,
    pub done_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(default)]
    pub mode: ChecklistMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Notice {
    pub id: String,
    pub trip_id: String,
    pub created_by: String,
    pub category: NoticeCategory,
    pub title: String,
    pub body: String,
    pub source_url: Option<String>,
    pub pinned: bool,
    #[serde(default)]
    pub status: NoticeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<String>>,
    pub checklist_items: Vec<ChecklistItem>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn notice_types_match_the_frontend_wire_contract() {
        let notice = Notice {
            id: "notice-a".into(),
            trip_id: "trip-a".into(),
            created_by: "user-a".into(),
            category: NoticeCategory::Safety,
            title: "Register the trip".into(),
            body: "Save the emergency number.".into(),
            source_url: Some("https://example.com/advice".into()),
            pinned: true,
            status: NoticeStatus::Active,
            audience: Some(vec!["user-a".into()]),
            checklist_items: vec![ChecklistItem {
                id: "item-a".into(),
                text: "Register".into(),
                done_by: vec![],
                due_date: None,
                mode: ChecklistMode::Each,
            }],
        };
        assert_eq!(
            serde_json::to_value(notice).expect("notice serializes"),
            json!({
                "id": "notice-a",
                "tripId": "trip-a",
                "createdBy": "user-a",
                "category": "safety",
                "title": "Register the trip",
                "body": "Save the emergency number.",
                "sourceUrl": "https://example.com/advice",
                "pinned": true,
                "status": "active",
                "audience": ["user-a"],
                "checklistItems": [{
                    "id": "item-a",
                    "text": "Register",
                    "doneBy": [],
                    "mode": "each"
                }]
            })
        );
    }

    #[test]
    fn stored_notice_rows_reject_unknown_fields_and_default_legacy_options() {
        let legacy: Notice = serde_json::from_value(json!({
            "id": "notice-a",
            "tripId": "trip-a",
            "createdBy": "user-a",
            "category": "visa",
            "title": "Visa",
            "body": "Check requirements",
            "sourceUrl": null,
            "pinned": false,
            "checklistItems": [{"id": "item-a", "text": "Check", "doneBy": []}]
        }))
        .expect("legacy optional fields default");
        assert_eq!(legacy.status, NoticeStatus::Active);
        assert_eq!(legacy.audience, None);
        assert_eq!(legacy.checklist_items[0].mode, ChecklistMode::Each);

        let mut encoded = serde_json::to_value(legacy).expect("serializes");
        encoded["replacementValue"] = json!("forged");
        assert!(serde_json::from_value::<Notice>(encoded).is_err());
    }
}
