use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PollKind {
    Decision,
    PlanChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PollStatus {
    Draft,
    Scheduled,
    Open,
    Passed,
    Failed,
    Expired,
}

impl PollStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Passed | Self::Failed | Self::Expired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PollOption {
    pub id: String,
    pub label: String,
    pub proposal_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PollVote {
    pub user_id: String,
    pub option_id: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Poll {
    pub id: String,
    pub trip_id: String,
    pub created_by: String,
    pub kind: PollKind,
    pub title: String,
    pub description: String,
    pub options: Vec<PollOption>,
    pub opens_at: Option<String>,
    pub closes_at: String,
    pub decided_at: Option<String>,
    pub quorum: u32,
    pub allow_multi: bool,
    pub status: PollStatus,
    pub votes: Vec<PollVote>,
    pub resolution_note: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Poll, PollKind, PollOption, PollStatus, PollVote};

    #[test]
    fn poll_types_match_the_wire_contract() {
        let poll = Poll {
            id: "poll-a".into(),
            trip_id: "trip-a".into(),
            created_by: "user-a".into(),
            kind: PollKind::Decision,
            title: "Dinner?".into(),
            description: String::new(),
            options: vec![PollOption {
                id: "option-a".into(),
                label: "Ramen".into(),
                proposal_id: None,
            }],
            opens_at: None,
            closes_at: "2026-08-08T12:00:00Z".into(),
            decided_at: None,
            quorum: 2,
            allow_multi: false,
            status: PollStatus::Open,
            votes: vec![PollVote {
                user_id: "user-a".into(),
                option_id: "option-a".into(),
                at: "2026-08-06T12:00:00Z".into(),
            }],
            resolution_note: None,
        };

        assert_eq!(
            serde_json::to_value(poll).expect("poll serializes"),
            json!({
                "id": "poll-a",
                "tripId": "trip-a",
                "createdBy": "user-a",
                "kind": "decision",
                "title": "Dinner?",
                "description": "",
                "options": [{"id": "option-a", "label": "Ramen", "proposalId": null}],
                "opensAt": null,
                "closesAt": "2026-08-08T12:00:00Z",
                "decidedAt": null,
                "quorum": 2,
                "allowMulti": false,
                "status": "open",
                "votes": [{"userId": "user-a", "optionId": "option-a", "at": "2026-08-06T12:00:00Z"}],
                "resolutionNote": null
            })
        );
    }
}
