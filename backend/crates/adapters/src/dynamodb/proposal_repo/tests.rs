use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use aws_sdk_dynamodb::{
    operation::{
        get_item::GetItemOutput,
        query::QueryOutput,
        transact_write_items::{TransactWriteItemsError, TransactWriteItemsOutput},
    },
    types::{AttributeValue, CancellationReason, error::TransactionCanceledException},
};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use chrono::{Days, NaiveDate};
use itinera_core::{
    domain::{
        content_history::ChangeSource,
        proposal::{
            ChangeOp, ChangeSet, Proposal, ProposalDecision, ProposalRoute, ProposalStatus,
        },
        trip::{
            Candidate, CandidateStatus, Day, Place, PlaceKind, Plan, Stop, StopKind, TripMember,
            TripRole, TripStatus,
        },
        user::UserId,
    },
    ports::proposal::{ProposalApplicationIds, ProposalRepo, ProposalRepoError},
};

use crate::dynamodb::trip_repo::records::*;
use crate::dynamodb::{CONDITIONAL_FAILURE, DynamoUserRepo, PK, REVISION, SK};

use super::access;
use super::records::{PROPOSAL_ENTITY, encode_proposal, proposal_sk};

const TABLE: &str = "itinera-test";
const TRIP_ID: &str = "trip-a";
const ACTOR_ID: &str = "user-a";
const CREATED_AT: &str = "2026-08-06T10:00:00Z";
const APPLIED_AT: &str = "2026-08-06T11:00:00Z";

fn actor() -> UserId {
    UserId(ACTOR_ID.into())
}

fn member(role: TripRole) -> TripMember {
    TripMember {
        user_id: ACTOR_ID.into(),
        role,
        joined_at: "2026-08-01T00:00:00Z".into(),
    }
}

fn meta(version: u32) -> TripMeta {
    TripMeta {
        id: TRIP_ID.into(),
        name: "Japan".into(),
        cover_photo_url: None,
        accent_color: None,
        stop_kind_labels: None,
        status: TripStatus::Planning,
        start_date: "2026-11-01".into(),
        end_date: "2026-11-03".into(),
        base_currency: "GBP".into(),
        soft_budget: None,
        current_plan_id: Some(format!("plan-{version}")),
        current_plan_version: Some(version),
        created_at: "2026-08-01T00:00:00Z".into(),
        member_count: 3,
        leader_count: 1,
        cities: vec!["Kyoto".into()],
    }
}

fn plan(version: u32) -> Plan {
    Plan {
        id: format!("plan-{version}"),
        trip_id: TRIP_ID.into(),
        version,
        created_from_proposal_id: None,
        created_at: "2026-08-01T00:00:00Z".into(),
    }
}

fn day(version: u32) -> Day {
    Day {
        id: "day-a".into(),
        plan_id: format!("plan-{version}"),
        date: "2026-11-01".into(),
        city_hint: "Kyoto".into(),
        tz: "Asia/Tokyo".into(),
        window_start: "09:00".into(),
        window_end: "21:00".into(),
    }
}

fn pending_proposal() -> Proposal {
    Proposal {
        id: "proposal-a".into(),
        trip_id: TRIP_ID.into(),
        created_by: ACTOR_ID.into(),
        source: ChangeSource::Web,
        title: "Add a second day".into(),
        rationale: "Make room".into(),
        change_set: ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::AddDay {
                date: "2026-11-02".into(),
                city_hint: "Osaka".into(),
            }],
        },
        route: ProposalRoute::LeaderApproval,
        status: ProposalStatus::Pending,
        decided_by: None,
        rejection_reason: None,
        created_at: CREATED_AT.into(),
    }
}

fn applied_proposal() -> Proposal {
    Proposal {
        status: ProposalStatus::Applied,
        decided_by: Some(ProposalDecision::Leader {
            user_id: ACTOR_ID.into(),
        }),
        ..pending_proposal()
    }
}

fn application_ids() -> ProposalApplicationIds {
    ProposalApplicationIds {
        plan_id: "plan-2".into(),
        entity_ids: vec!["day-b".into(), "unused-a".into()],
    }
}

fn membership_rule(role: TripRole) -> aws_smithy_mocks::Rule {
    let item = encode_member(TRIP_ID, &member(role)).expect("member encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(format!("MEMBER#{ACTOR_ID}")))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn meta_rule(version: u32, revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_trip_meta(&meta(version), revision).expect("meta encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(META_SK.into()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn plan_rule(version: u32) -> aws_smithy_mocks::Rule {
    let plan = plan(version);
    let day = day(version);
    let plan_item = encode_record(trip_pk(TRIP_ID), plan_sk(version), PLAN_ENTITY, &plan, 4)
        .expect("plan encodes");
    let day_item = encode_record(trip_pk(TRIP_ID), day_sk(version, &day), DAY_ENTITY, &day, 7)
        .expect("day encodes");
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(format!("{}#", plan_prefix(version))))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![plan_item.clone(), day_item.clone()]))
                .build()
        })
}

fn plan_with_candidate_stop_rule(version: u32) -> aws_smithy_mocks::Rule {
    let plan = plan(version);
    let day = day(version);
    let stop = Stop {
        id: "stop-a".into(),
        day_id: day.id.clone(),
        seq: 1.0,
        place_id: "place-candidate".into(),
        stop_kind: StopKind::Meal,
        planned_arrival: "15:00".into(),
        duration_min: 60,
        booking: None,
        notes: String::new(),
    };
    let items = vec![
        encode_record(trip_pk(TRIP_ID), plan_sk(version), PLAN_ENTITY, &plan, 4)
            .expect("plan encodes"),
        encode_record(trip_pk(TRIP_ID), day_sk(version, &day), DAY_ENTITY, &day, 7)
            .expect("day encodes"),
        encode_record(
            trip_pk(TRIP_ID),
            stop_sk(version, &day, &stop),
            STOP_ENTITY,
            &stop,
            8,
        )
        .expect("stop encodes"),
    ];
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(format!("{}#", plan_prefix(version))))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(items.clone()))
                .build()
        })
}

fn many_days_plan_rule(version: u32, day_count: usize) -> aws_smithy_mocks::Rule {
    let plan = plan(version);
    let mut items = vec![
        encode_record(trip_pk(TRIP_ID), plan_sk(version), PLAN_ENTITY, &plan, 4)
            .expect("plan encodes"),
    ];
    let start = NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid fixture date");
    for index in 0..day_count {
        let day = Day {
            id: format!("day-{index:03}"),
            plan_id: plan.id.clone(),
            date: start
                .checked_add_days(Days::new(index as u64))
                .expect("fixture date is in range")
                .format("%Y-%m-%d")
                .to_string(),
            city_hint: "Kyoto".into(),
            tz: "Asia/Tokyo".into(),
            window_start: "09:00".into(),
            window_end: "21:00".into(),
        };
        items.push(
            encode_record(trip_pk(TRIP_ID), day_sk(version, &day), DAY_ENTITY, &day, 7)
                .expect("day encodes"),
        );
    }
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(format!("{}#", plan_prefix(version))))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(items.clone()))
                .build()
        })
}

fn candidate_place() -> Place {
    Place {
        id: "place-candidate".into(),
        name: "Tea house".into(),
        kind: PlaceKind::Food,
        lat: 35.0,
        lng: 135.0,
        tz: "Asia/Tokyo".into(),
        country_code: "JP".into(),
        admin_area: "Kyoto".into(),
        city: "Kyoto".into(),
        address: "Gion".into(),
        external_ref: None,
        website: None,
        phone: None,
        rating: None,
        price_level: None,
        opening_hours: None,
        photo_urls: vec![],
        guide: None,
    }
}

fn candidate(status: CandidateStatus) -> Candidate {
    Candidate {
        id: "candidate-a".into(),
        trip_id: TRIP_ID.into(),
        source_place_id: Some("catalog-a".into()),
        place_id: "place-candidate".into(),
        proposed_by: ACTOR_ID.into(),
        created_at: CREATED_AT.into(),
        pitch: "A quiet tea break".into(),
        tags: vec!["tea".into()],
        status,
    }
}

fn candidate_query_rule(candidate: Candidate, revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_record(
        trip_pk(TRIP_ID),
        candidate_sk(&candidate.id),
        CANDIDATE_ENTITY,
        &candidate,
        revision,
    )
    .expect("candidate encodes");
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("CANDIDATE#".into()))
        })
        .then_output(move || QueryOutput::builder().items(item.clone()).build())
}

fn place_get_rule(place: Place) -> aws_smithy_mocks::Rule {
    let item = encode_record(
        trip_pk(TRIP_ID),
        place_sk(&place.id),
        PLACE_ENTITY,
        &place,
        3,
    )
    .expect("place encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(place_sk("place-candidate")))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn add_candidate_stop_proposal() -> Proposal {
    Proposal {
        title: "Adopt the tea house".into(),
        rationale: "It fits the afternoon".into(),
        change_set: ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::AddStop {
                day_id: "day-a".into(),
                place_id: "place-candidate".into(),
                seq: 1.0,
                stop_kind: StopKind::Meal,
            }],
        },
        ..pending_proposal()
    }
}

fn remove_candidate_stop_proposal() -> Proposal {
    Proposal {
        title: "Remove the tea house".into(),
        rationale: "Free the afternoon".into(),
        change_set: ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::RemoveStop {
                stop_id: "stop-a".into(),
            }],
        },
        ..pending_proposal()
    }
}

fn stop_application_ids() -> ProposalApplicationIds {
    ProposalApplicationIds {
        plan_id: "plan-2".into(),
        entity_ids: vec!["stop-b".into(), "unused-a".into()],
    }
}

fn empty_candidates_rule() -> aws_smithy_mocks::Rule {
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("CANDIDATE#".into()))
        })
        .then_output(|| QueryOutput::builder().build())
}

fn proposal_get_rule(proposal: Proposal, revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_proposal(&proposal, revision).expect("proposal encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(proposal_sk("proposal-a")))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn cancelled_transaction(codes: &[&str]) -> TransactWriteItemsError {
    let mut builder = TransactionCanceledException::builder();
    for code in codes {
        builder = builder.cancellation_reasons(CancellationReason::builder().code(*code).build());
    }
    TransactWriteItemsError::TransactionCanceledException(builder.build())
}

#[test]
fn proposal_keys_are_trip_partitioned_and_do_not_overlap_content_audit() {
    assert_eq!(proposal_sk("proposal-a"), "PROPOSAL#proposal-a");
    assert!(!proposal_sk("proposal-a").starts_with("AUDIT#"));
    assert_eq!(
        stop_sk(
            2,
            &Day {
                plan_id: "plan-2".into(),
                ..day(1)
            },
            &Stop {
                id: "stop-a".into(),
                day_id: "day-a".into(),
                seq: 3.0,
                place_id: "place-a".into(),
                stop_kind: StopKind::Visit,
                planned_arrival: "12:00".into(),
                duration_min: 60,
                booking: None,
                notes: String::new(),
            }
        ),
        "PLAN#0000000002#DAY#2026-11-01#STOP#000003#stop-a"
    );
}

#[tokio::test]
async fn every_member_role_can_list_but_viewers_cannot_submit() {
    for role in [TripRole::Viewer, TripRole::Member, TripRole::Leader] {
        let membership = membership_rule(role);
        let meta = meta_rule(1, 9);
        let proposal = pending_proposal();
        let proposal_item = encode_proposal(&proposal, 2).expect("proposal encodes");
        let list = mock!(aws_sdk_dynamodb::Client::query)
            .match_requests(|request| {
                request.consistent_read() == Some(true)
                    && request
                        .expression_attribute_values()
                        .and_then(|values| values.get(":prefix"))
                        == Some(&AttributeValue::S("PROPOSAL#".into()))
            })
            .then_output(move || QueryOutput::builder().items(proposal_item.clone()).build());
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&membership, &meta, &list]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        assert_eq!(
            repo.list_proposals(TRIP_ID, &actor())
                .await
                .expect("all members list"),
            vec![proposal]
        );
    }

    let viewer = membership_rule(TripRole::Viewer);
    let client = mock_client!(aws_sdk_dynamodb, [&viewer]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_proposal(TRIP_ID, &actor(), pending_proposal(), application_ids())
            .await,
        Err(ProposalRepoError::Forbidden)
    );
}

#[tokio::test]
async fn only_leaders_can_approve_or_reject() {
    for role in [TripRole::Viewer, TripRole::Member] {
        let membership = membership_rule(role);
        let client = mock_client!(aws_sdk_dynamodb, [&membership]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
        assert_eq!(
            repo.approve_proposal(
                TRIP_ID,
                &actor(),
                "proposal-a",
                APPLIED_AT,
                application_ids(),
            )
            .await,
            Err(ProposalRepoError::Forbidden)
        );

        let membership = membership_rule(role);
        let client = mock_client!(aws_sdk_dynamodb, [&membership]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
        assert_eq!(
            repo.reject_proposal(TRIP_ID, &actor(), "proposal-a", "No")
                .await,
            Err(ProposalRepoError::Forbidden)
        );
    }
}

#[tokio::test]
async fn leader_rejection_has_exact_role_revision_and_provenance_guards() {
    let membership = membership_rule(TripRole::Leader);
    let proposal = proposal_get_rule(pending_proposal(), 5);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 2
                && items[0].condition_check().is_some_and(|condition| {
                    condition.key().get(SK)
                        == Some(&AttributeValue::S(format!("MEMBER#{ACTOR_ID}")))
                        && condition.condition_expression()
                            == "#entity = :member AND #role = :leader"
                })
                && items[1].put().is_some_and(|put| {
                    let rejected = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Proposal>(data).ok());
                    put.item().get(SK) == Some(&AttributeValue::S(proposal_sk("proposal-a")))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression() == Some("#revision = :revision")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("5".into()))
                        && rejected.is_some_and(|proposal| {
                            proposal.status == ProposalStatus::Rejected
                                && proposal.rejection_reason.as_deref() == Some("Not this time")
                                && proposal.decided_by
                                    == Some(ProposalDecision::Leader {
                                        user_id: ACTOR_ID.into(),
                                    })
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &proposal, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let rejected = repo
        .reject_proposal(TRIP_ID, &actor(), "proposal-a", "Not this time")
        .await
        .expect("leader rejection commits");

    assert_eq!(rejected.status, ProposalStatus::Rejected);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn member_submission_preflights_then_writes_only_a_scoped_pending_proposal() {
    let membership = membership_rule(TripRole::Member);
    let meta = meta_rule(1, 9);
    let plan = plan_rule(1);
    let candidates = empty_candidates_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && condition.condition_expression()
                            == "#entity = :trip AND #revision = :revision AND #plan_id = :plan_id AND #plan_version = :plan_version"
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("9".into()))
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":plan_version"))
                            == Some(&AttributeValue::N("1".into()))
                })
                && items[2].put().is_some_and(|put| {
                    let proposal = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Proposal>(data).ok());
                    put.item().get(SK)
                        == Some(&AttributeValue::S(proposal_sk("proposal-a")))
                        && put.condition_expression()
                            == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                        && proposal.is_some_and(|proposal| {
                            proposal.status == ProposalStatus::Pending
                                && proposal.decided_by.is_none()
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let created = repo
        .create_proposal(TRIP_ID, &actor(), pending_proposal(), application_ids())
        .await
        .expect("pending proposal created");

    assert_eq!(created.status, ProposalStatus::Pending);
    assert_eq!(transaction.num_calls(), 1);
    assert_eq!(
        meta.num_calls(),
        2,
        "submission rechecks the base before writing"
    );
}

#[tokio::test]
async fn leader_fast_path_clones_the_plan_with_exact_atomic_guards() {
    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = plan_rule(1);
    let candidates = empty_candidates_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            if items.len() != 8 {
                return false;
            }
            let member_ok = items[0].condition_check().is_some_and(|condition| {
                condition.condition_expression() == "#entity = :member AND #role = :leader"
            });
            let meta_ok = items[1].put().is_some_and(|put| {
                let meta = put
                    .item()
                    .get(DATA)
                    .and_then(|value| value.as_s().ok())
                    .and_then(|data| serde_json::from_str::<TripMeta>(data).ok());
                put.condition_expression()
                    == Some("#entity = :trip AND #revision = :revision AND #plan_id = :plan_id AND #plan_version = :plan_version")
                    && put.item().get(REVISION) == Some(&AttributeValue::N("10".into()))
                    && put
                        .expression_attribute_values()
                        .and_then(|values| values.get(":plan_id"))
                        == Some(&AttributeValue::S("plan-1".into()))
                    && put
                        .expression_attribute_values()
                        .and_then(|values| values.get(":plan_version"))
                        == Some(&AttributeValue::N("1".into()))
                    && meta.is_some_and(|meta| {
                        meta.current_plan_id.as_deref() == Some("plan-2")
                            && meta.current_plan_version == Some(2)
                            && meta.cities == ["Kyoto", "Osaka"]
                    })
            });
            let proposal_ok = items[2].put().is_some_and(|put| {
                let proposal = put
                    .item()
                    .get(DATA)
                    .and_then(|value| value.as_s().ok())
                    .and_then(|data| serde_json::from_str::<Proposal>(data).ok());
                put.condition_expression()
                    == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                    && proposal.is_some_and(|proposal| {
                        proposal.status == ProposalStatus::Applied
                            && proposal.decided_by
                                == Some(ProposalDecision::Leader {
                                    user_id: ACTOR_ID.into(),
                                })
                    })
            });
            let source_plan_ok = items[3].condition_check().is_some_and(|condition| {
                condition.key().get(SK)
                    == Some(&AttributeValue::S(plan_sk(1)))
                    && condition.condition_expression()
                        == "#entity = :entity AND #revision = :revision"
                    && condition
                        .expression_attribute_values()
                        .and_then(|values| values.get(":revision"))
                        == Some(&AttributeValue::N("4".into()))
            });
            let source_day_ok = items[4].condition_check().is_some_and(|condition| {
                condition.key().get(SK)
                    == Some(&AttributeValue::S(day_sk(1, &day(1))))
                    && condition
                        .expression_attribute_values()
                        .and_then(|values| values.get(":revision"))
                        == Some(&AttributeValue::N("7".into()))
            });
            let plan_ok = items[5].put().is_some_and(|put| {
                let plan = put
                    .item()
                    .get(DATA)
                    .and_then(|value| value.as_s().ok())
                    .and_then(|data| serde_json::from_str::<Plan>(data).ok());
                put.item().get(SK) == Some(&AttributeValue::S(plan_sk(2)))
                    && put.condition_expression()
                        == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                    && plan.is_some_and(|plan| {
                        plan.version == 2
                            && plan.id == "plan-2"
                            && plan.created_from_proposal_id.as_deref() == Some("proposal-a")
                    })
            });
            let days_ok = items[6..].iter().all(|item| {
                item.put().is_some_and(|put| {
                    put.item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Day>(data).ok())
                        .is_some_and(|day| day.plan_id == "plan-2")
                        && put.condition_expression()
                            == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                })
            });
            member_ok
                && meta_ok
                && proposal_ok
                && source_plan_ok
                && source_day_ok
                && plan_ok
                && days_ok
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let created = repo
        .create_proposal(TRIP_ID, &actor(), pending_proposal(), application_ids())
        .await
        .expect("leader proposal applies");

    assert_eq!(created.status, ProposalStatus::Applied);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn stale_approval_marks_only_the_trip_scoped_proposal() {
    let membership = membership_rule(TripRole::Leader);
    let proposal = proposal_get_rule(pending_proposal(), 5);
    let meta = meta_rule(2, 10);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression() == "#entity = :member AND #role = :leader"
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && condition.condition_expression()
                            == "#entity = :trip AND #plan_version <> :base_version"
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":base_version"))
                            == Some(&AttributeValue::N("1".into()))
                })
                && items[2].put().is_some_and(|put| {
                    let proposal = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Proposal>(data).ok());
                    put.item().get(SK) == Some(&AttributeValue::S(proposal_sk("proposal-a")))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression() == Some("#revision = :revision")
                        && proposal.is_some_and(|proposal| {
                            proposal.status == ProposalStatus::Stale
                                && proposal.decided_by.is_none()
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &proposal, &meta, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.approve_proposal(
            TRIP_ID,
            &actor(),
            "proposal-a",
            APPLIED_AT,
            application_ids(),
        )
        .await,
        Err(ProposalRepoError::Conflict)
    );
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn approval_that_observes_a_concurrent_winners_plan_is_idempotent() {
    let membership = membership_rule(TripRole::Leader);
    let calls = Arc::new(AtomicUsize::new(0));
    let get_calls = Arc::clone(&calls);
    let pending_item = encode_proposal(&pending_proposal(), 5).expect("pending encodes");
    let applied_item = encode_proposal(&applied_proposal(), 6).expect("applied encodes");
    let proposal = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S(proposal_sk("proposal-a")))
            })
        })
        .then_output(move || {
            let item = if get_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                pending_item.clone()
            } else {
                applied_item.clone()
            };
            GetItemOutput::builder().set_item(Some(item)).build()
        });
    let meta = meta_rule(2, 10);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_error(|| cancelled_transaction(&[CONDITIONAL_FAILURE]));
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &proposal, &meta, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let result = repo
        .approve_proposal(
            TRIP_ID,
            &actor(),
            "proposal-a",
            APPLIED_AT,
            application_ids(),
        )
        .await
        .expect("the winning approval is returned");

    assert_eq!(result.status, ProposalStatus::Applied);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn a_concurrent_winning_approval_is_returned_idempotently() {
    let membership = membership_rule(TripRole::Leader);
    let calls = Arc::new(AtomicUsize::new(0));
    let get_calls = Arc::clone(&calls);
    let pending_item = encode_proposal(&pending_proposal(), 5).expect("pending encodes");
    let applied_item = encode_proposal(&applied_proposal(), 6).expect("applied encodes");
    let proposal = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S(proposal_sk("proposal-a")))
            })
        })
        .then_output(move || {
            let item = if get_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                pending_item.clone()
            } else {
                applied_item.clone()
            };
            GetItemOutput::builder().set_item(Some(item)).build()
        });
    let meta = meta_rule(1, 9);
    let plan = plan_rule(1);
    let candidates = empty_candidates_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_error(|| cancelled_transaction(&[CONDITIONAL_FAILURE]));
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &proposal,
            &meta,
            &plan,
            &candidates,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let result = repo
        .approve_proposal(
            TRIP_ID,
            &actor(),
            "proposal-a",
            APPLIED_AT,
            application_ids(),
        )
        .await
        .expect("concurrent applied result is idempotent");

    assert_eq!(result.status, ProposalStatus::Applied);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn cross_trip_ids_corrupt_rows_and_poll_routes_fail_closed() {
    let membership = membership_rule(TripRole::Leader);
    let missing = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S(proposal_sk("foreign-proposal")))
            })
        })
        .then_output(|| GetItemOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &missing]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.approve_proposal(
            TRIP_ID,
            &actor(),
            "foreign-proposal",
            APPLIED_AT,
            application_ids(),
        )
        .await,
        Err(ProposalRepoError::NotFound)
    );

    let membership = membership_rule(TripRole::Member);
    let client = mock_client!(aws_sdk_dynamodb, [&membership]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let mut poll = pending_proposal();
    poll.route = ProposalRoute::Poll;
    assert_eq!(
        repo.create_proposal(TRIP_ID, &actor(), poll, application_ids())
            .await,
        Err(ProposalRepoError::Conflict)
    );
}

#[tokio::test]
async fn malformed_proposal_rows_fail_closed() {
    let membership = membership_rule(TripRole::Viewer);
    let meta = meta_rule(1, 9);
    let mut foreign = pending_proposal();
    foreign.trip_id = "trip-b".into();
    let corrupt = encode_record(
        trip_pk(TRIP_ID),
        proposal_sk(&foreign.id),
        PROPOSAL_ENTITY,
        &foreign,
        1,
    )
    .expect("corrupt fixture encodes");
    let list = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request
                .expression_attribute_values()
                .and_then(|values| values.get(":prefix"))
                == Some(&AttributeValue::S("PROPOSAL#".into()))
        })
        .then_output(move || QueryOutput::builder().items(corrupt.clone()).build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &list]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.list_proposals(TRIP_ID, &actor()).await,
        Err(ProposalRepoError::CorruptData)
    );
}

#[tokio::test]
async fn malformed_source_plan_keys_fail_before_publication() {
    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = plan(1);
    let day = day(1);
    let plan_item =
        encode_record(trip_pk(TRIP_ID), plan_sk(1), PLAN_ENTITY, &plan, 4).expect("plan encodes");
    let corrupt_day = encode_record(
        trip_pk(TRIP_ID),
        format!("{}#DAY#wrong-key", plan_prefix(1)),
        DAY_ENTITY,
        &day,
        7,
    )
    .expect("corrupt fixture encodes");
    let source = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request
                .expression_attribute_values()
                .and_then(|values| values.get(":prefix"))
                == Some(&AttributeValue::S(format!("{}#", plan_prefix(1))))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![plan_item.clone(), corrupt_day.clone()]))
                .build()
        });
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &source]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.create_proposal(TRIP_ID, &actor(), pending_proposal(), application_ids())
            .await,
        Err(ProposalRepoError::CorruptData)
    );
}

#[tokio::test]
async fn candidate_adoption_is_revision_guarded_in_the_publication_transaction() {
    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = plan_rule(1);
    let place = place_get_rule(candidate_place());
    let candidates = candidate_query_rule(candidate(CandidateStatus::Shortlisted), 11);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 9
                && items.last().is_some_and(|item| {
                    item.put().is_some_and(|put| {
                        let candidate = put
                            .item()
                            .get(DATA)
                            .and_then(|value| value.as_s().ok())
                            .and_then(|data| serde_json::from_str::<Candidate>(data).ok());
                        put.item().get(SK) == Some(&AttributeValue::S(candidate_sk("candidate-a")))
                            && put.item().get(REVISION) == Some(&AttributeValue::N("12".into()))
                            && put.condition_expression() == Some("#revision = :revision")
                            && put
                                .expression_attribute_values()
                                .and_then(|values| values.get(":revision"))
                                == Some(&AttributeValue::N("11".into()))
                            && candidate.is_some_and(|candidate| {
                                candidate.status == CandidateStatus::InPlan
                                    && candidate.trip_id == TRIP_ID
                                    && candidate.place_id == "place-candidate"
                            })
                    })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &place, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let applied = repo
        .create_proposal(
            TRIP_ID,
            &actor(),
            add_candidate_stop_proposal(),
            stop_application_ids(),
        )
        .await
        .expect("candidate adoption commits");

    assert_eq!(applied.status, ProposalStatus::Applied);
    assert_eq!(place.num_calls(), 2, "the owned place is revalidated");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn removing_the_last_candidate_stop_writes_shortlisted_with_revision_guard() {
    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = plan_with_candidate_stop_rule(1);
    let place = place_get_rule(candidate_place());
    let candidates = candidate_query_rule(candidate(CandidateStatus::InPlan), 11);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 9
                && items.last().is_some_and(|item| {
                    item.put().is_some_and(|put| {
                        let candidate = put
                            .item()
                            .get(DATA)
                            .and_then(|value| value.as_s().ok())
                            .and_then(|data| serde_json::from_str::<Candidate>(data).ok());
                        put.item().get(SK) == Some(&AttributeValue::S(candidate_sk("candidate-a")))
                            && put.item().get(REVISION) == Some(&AttributeValue::N("12".into()))
                            && put.condition_expression() == Some("#revision = :revision")
                            && put
                                .expression_attribute_values()
                                .and_then(|values| values.get(":revision"))
                                == Some(&AttributeValue::N("11".into()))
                            && candidate.is_some_and(|candidate| {
                                candidate.status == CandidateStatus::Shortlisted
                                    && candidate.trip_id == TRIP_ID
                                    && candidate.place_id == "place-candidate"
                            })
                    })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &place, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let applied = repo
        .create_proposal(
            TRIP_ID,
            &actor(),
            remove_candidate_stop_proposal(),
            stop_application_ids(),
        )
        .await
        .expect("candidate removal commits");

    assert_eq!(applied.status, ProposalStatus::Applied);
    assert_eq!(place.num_calls(), 2, "the owned place is revalidated");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn malformed_and_rejected_candidates_fail_before_publication() {
    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = plan_rule(1);
    let place = place_get_rule(candidate_place());
    let mut malformed = candidate(CandidateStatus::Shortlisted);
    malformed.pitch = " not canonical".into();
    let candidates = candidate_query_rule(malformed, 11);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &place, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_proposal(
            TRIP_ID,
            &actor(),
            add_candidate_stop_proposal(),
            stop_application_ids(),
        )
        .await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(transaction.num_calls(), 0);

    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = plan_rule(1);
    let place = place_get_rule(candidate_place());
    let candidates = candidate_query_rule(candidate(CandidateStatus::Rejected), 11);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &place, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_proposal(
            TRIP_ID,
            &actor(),
            add_candidate_stop_proposal(),
            stop_application_ids(),
        )
        .await,
        Err(ProposalRepoError::InvalidChange)
    );
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn candidate_owned_place_is_revalidated_before_status_rewrite() {
    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = plan_rule(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let get_calls = Arc::clone(&calls);
    let valid = candidate_place();
    let mut foreign = valid.clone();
    foreign.id = "foreign-place".into();
    let valid_item = encode_record(
        trip_pk(TRIP_ID),
        place_sk("place-candidate"),
        PLACE_ENTITY,
        &valid,
        3,
    )
    .expect("valid place encodes");
    let corrupt_item = encode_record(
        trip_pk(TRIP_ID),
        place_sk("place-candidate"),
        PLACE_ENTITY,
        &foreign,
        3,
    )
    .expect("corrupt place fixture encodes");
    let place = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S(place_sk("place-candidate")))
            })
        })
        .then_output(move || {
            let item = if get_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                valid_item.clone()
            } else {
                corrupt_item.clone()
            };
            GetItemOutput::builder().set_item(Some(item)).build()
        });
    let candidates = candidate_query_rule(candidate(CandidateStatus::Shortlisted), 11);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &place, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.create_proposal(
            TRIP_ID,
            &actor(),
            add_candidate_stop_proposal(),
            stop_application_ids(),
        )
        .await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn exhausted_revisions_fail_closed_without_a_transaction() {
    let membership = membership_rule(TripRole::Leader);
    let proposal = proposal_get_rule(pending_proposal(), u64::MAX);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &proposal, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.reject_proposal(TRIP_ID, &actor(), "proposal-a", "No")
            .await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(transaction.num_calls(), 0);

    let membership = membership_rule(TripRole::Leader);
    let proposal = proposal_get_rule(pending_proposal(), u64::MAX);
    let meta = meta_rule(2, 10);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &proposal, &meta, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.approve_proposal(
            TRIP_ID,
            &actor(),
            "proposal-a",
            APPLIED_AT,
            application_ids(),
        )
        .await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(transaction.num_calls(), 0);

    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, u64::MAX);
    let plan = plan_rule(1);
    let candidates = empty_candidates_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_proposal(TRIP_ID, &actor(), pending_proposal(), application_ids())
            .await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(transaction.num_calls(), 0);

    let membership = membership_rule(TripRole::Leader);
    let proposal = proposal_get_rule(pending_proposal(), u64::MAX);
    let meta = meta_rule(1, 9);
    let plan = plan_rule(1);
    let candidates = empty_candidates_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &proposal,
            &meta,
            &plan,
            &candidates,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.approve_proposal(
            TRIP_ID,
            &actor(),
            "proposal-a",
            APPLIED_AT,
            application_ids(),
        )
        .await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(transaction.num_calls(), 0);

    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = plan_rule(1);
    let place = place_get_rule(candidate_place());
    let candidates = candidate_query_rule(candidate(CandidateStatus::Shortlisted), u64::MAX);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &place, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_proposal(
            TRIP_ID,
            &actor(),
            add_candidate_stop_proposal(),
            stop_application_ids(),
        )
        .await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(transaction.num_calls(), 0);
}

#[test]
fn transaction_limits_accept_the_exact_boundary_and_reject_one_over() {
    assert_eq!(
        access::enforce_transaction_action_limit(access::MAX_TRANSACTION_ACTIONS),
        Ok(())
    );
    assert_eq!(
        access::enforce_transaction_action_limit(access::MAX_TRANSACTION_ACTIONS + 1),
        Err(ProposalRepoError::SafetyLimitExceeded)
    );

    let at_limit = HashMap::from([(
        DATA.to_string(),
        AttributeValue::S("x".repeat(access::MAX_TRANSACTION_DATA_BYTES)),
    )]);
    assert_eq!(access::enforce_transaction_data_limit(&[at_limit]), Ok(()));
    let over_limit = HashMap::from([(
        DATA.to_string(),
        AttributeValue::S("x".repeat(access::MAX_TRANSACTION_DATA_BYTES + 1)),
    )]);
    assert_eq!(
        access::enforce_transaction_data_limit(&[over_limit]),
        Err(ProposalRepoError::SafetyLimitExceeded)
    );
}

#[tokio::test]
async fn publication_accepts_100_actions_and_rejects_an_oversized_transaction() {
    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = many_days_plan_rule(1, 47);
    let candidates = empty_candidates_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| request.transact_items().len() == 100)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let mut proposal = pending_proposal();
    proposal.change_set.ops = vec![ChangeOp::AddDay {
        date: "2027-12-31".into(),
        city_hint: "Nara".into(),
    }];
    let applied = repo
        .create_proposal(TRIP_ID, &actor(), proposal.clone(), application_ids())
        .await
        .expect("the 100-action boundary is allowed");
    assert_eq!(applied.status, ProposalStatus::Applied);
    assert_eq!(transaction.num_calls(), 1);

    let membership = membership_rule(TripRole::Leader);
    let meta = meta_rule(1, 9);
    let plan = many_days_plan_rule(1, 48);
    let candidates = empty_candidates_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &plan, &candidates, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_proposal(TRIP_ID, &actor(), proposal, application_ids())
            .await,
        Err(ProposalRepoError::SafetyLimitExceeded)
    );
    assert_eq!(transaction.num_calls(), 0);
}
