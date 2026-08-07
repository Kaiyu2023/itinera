use serde::{Deserialize, Serialize};

use super::{
    content_history::ChangeSource,
    trip::{PlaceKind, StopKind},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewPlaceDraft {
    pub name: String,
    pub kind: PlaceKind,
    pub city: String,
    pub note: String,
    pub url: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ChangeOp {
    AddStop {
        day_id: String,
        place_id: String,
        seq: f64,
        stop_kind: StopKind,
    },
    AddPlaceStop {
        day_id: String,
        seq: f64,
        stop_kind: StopKind,
        draft: NewPlaceDraft,
    },
    RemoveStop {
        stop_id: String,
    },
    MoveStop {
        stop_id: String,
        to_day_id: String,
        seq: f64,
    },
    Reorder {
        day_id: String,
        stop_ids_in_order: Vec<String>,
    },
    SwapPlace {
        stop_id: String,
        new_place_id: String,
    },
    AddDay {
        date: String,
        city_hint: String,
    },
    RemoveDay {
        day_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeSet {
    pub base_plan_version: u32,
    pub ops: Vec<ChangeOp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalRoute {
    LeaderApproval,
    Poll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Draft,
    Pending,
    Approved,
    Rejected,
    Applied,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProposalDecision {
    Leader { user_id: String },
    Poll { poll_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Proposal {
    pub id: String,
    pub trip_id: String,
    pub created_by: String,
    pub source: ChangeSource,
    pub title: String,
    pub rationale: String,
    pub change_set: ChangeSet,
    pub route: ProposalRoute,
    pub status: ProposalStatus,
    pub decided_by: Option<ProposalDecision>,
    pub rejection_reason: Option<String>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn proposal_types_match_the_discriminated_wire_contract() {
        let proposal = Proposal {
            id: "proposal-a".into(),
            trip_id: "trip-a".into(),
            created_by: "user-a".into(),
            source: ChangeSource::Web {},
            title: "Move lunch".into(),
            rationale: "A shorter walk".into(),
            change_set: ChangeSet {
                base_plan_version: 3,
                ops: vec![ChangeOp::MoveStop {
                    stop_id: "stop-a".into(),
                    to_day_id: "day-b".into(),
                    seq: 2.0,
                }],
            },
            route: ProposalRoute::LeaderApproval,
            status: ProposalStatus::Applied,
            decided_by: Some(ProposalDecision::Leader {
                user_id: "user-leader".into(),
            }),
            rejection_reason: None,
            created_at: "2026-08-06T12:00:00Z".into(),
        };

        assert_eq!(
            serde_json::to_value(proposal).expect("proposal serializes"),
            json!({
                "id": "proposal-a",
                "tripId": "trip-a",
                "createdBy": "user-a",
                "source": {"via": "web"},
                "title": "Move lunch",
                "rationale": "A shorter walk",
                "changeSet": {
                    "basePlanVersion": 3,
                    "ops": [{"op": "move_stop", "stopId": "stop-a", "toDayId": "day-b", "seq": 2.0}]
                },
                "route": "leader_approval",
                "status": "applied",
                "decidedBy": {"kind": "leader", "userId": "user-leader"},
                "rejectionReason": null,
                "createdAt": "2026-08-06T12:00:00Z"
            })
        );
    }

    #[test]
    fn change_ops_reject_unknown_write_fields() {
        let error = serde_json::from_value::<ChangeOp>(json!({
            "op": "remove_stop",
            "stopId": "stop-a",
            "replacementPlanId": "caller-owned"
        }))
        .expect_err("unknown fields must fail closed");

        assert!(error.to_string().contains("unknown field"));
    }
}
