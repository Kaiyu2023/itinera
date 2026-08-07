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
use itinera_core::{
    domain::{
        content_history::ChangeSource,
        poll::{Poll, PollKind, PollOption, PollStatus},
        proposal::{
            ChangeOp, ChangeSet, Proposal, ProposalDecision, ProposalRoute, ProposalStatus,
        },
        trip::{
            Candidate, CandidateStatus, Day, Place, PlaceKind, Plan, Stop, StopKind, TripMember,
            TripRole, TripStatus,
        },
        user::UserId,
    },
    ports::{
        poll::{NewDecisionPoll, NewPlanChangePoll, PollRepo, PollRepoError},
        proposal::ProposalApplicationIds,
    },
};

use crate::dynamodb::{
    CONDITIONAL_FAILURE, DynamoUserRepo, ENTITY_TYPE, PK, REVISION, SK,
    ledger_repo::records::{EXPENSE_PREFIX, LEDGER_META_SK, STOP_LINK_PREFIX},
    proposal_repo::records::{encode_proposal, proposal_sk},
    trip_repo::records::{
        CANDIDATE_ENTITY, DATA, DAY_ENTITY, META_SK, PLACE_ENTITY, PLAN_ENTITY, TripMeta,
        candidate_sk, day_sk, encode_member, encode_record, encode_trip_meta, place_sk,
        plan_prefix, plan_sk, trip_pk,
    },
};

use super::{
    access::MAX_POLL_BYTES,
    records::{
        BALLOT_ENTITY, BallotRecord, LoadedBallot, LoadedPoll, POLL_ENTITY, attach_ballots,
        ballot_sk, decode_poll, encode_ballot, encode_poll, poll_sk, validate_ballot_for_poll,
    },
};

const TABLE: &str = "itinera-test";
const TRIP_ID: &str = "trip-a";
const ACTOR_ID: &str = "member-a";
const CREATED_AT: &str = "2026-08-06T10:00:00Z";
const VOTED_AT: &str = "2026-08-06T10:30:00Z";
const DECIDED_AT: &str = "2026-08-06T11:00:00Z";
const CLOSES_AT: &str = "2026-08-06T12:00:00Z";

fn actor() -> UserId {
    UserId(ACTOR_ID.into())
}

fn member(user_id: &str, role: TripRole) -> TripMember {
    TripMember {
        user_id: user_id.into(),
        role,
        joined_at: "2026-08-01T00:00:00Z".into(),
    }
}

fn meta() -> TripMeta {
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
        current_plan_id: Some("plan-1".into()),
        current_plan_version: Some(1),
        created_at: "2026-08-01T00:00:00Z".into(),
        member_count: 4,
        leader_count: 1,
        cities: vec!["Kyoto".into()],
    }
}

fn decision_poll(status: PollStatus) -> Poll {
    Poll {
        id: "poll-a".into(),
        trip_id: TRIP_ID.into(),
        created_by: ACTOR_ID.into(),
        kind: PollKind::Decision,
        title: "Dinner".into(),
        description: "Pick one".into(),
        options: vec![
            PollOption {
                id: "ramen".into(),
                label: "Ramen".into(),
                proposal_id: None,
            },
            PollOption {
                id: "sushi".into(),
                label: "Sushi".into(),
                proposal_id: None,
            },
        ],
        opens_at: None,
        closes_at: CLOSES_AT.into(),
        decided_at: status.is_terminal().then(|| DECIDED_AT.into()),
        quorum: 2,
        allow_multi: false,
        status,
        votes: vec![],
        resolution_note: None,
    }
}

fn plan_change_poll() -> Poll {
    Poll {
        kind: PollKind::PlanChange,
        options: vec![
            PollOption {
                id: "adopt".into(),
                label: "Adopt the proposed plan change".into(),
                proposal_id: Some("proposal-a".into()),
            },
            PollOption {
                id: "keep".into(),
                label: "Keep the current plan".into(),
                proposal_id: None,
            },
        ],
        allow_multi: false,
        ..decision_poll(PollStatus::Open)
    }
}

fn terminal_plan_change_poll(status: PollStatus) -> Poll {
    let mut poll = plan_change_poll();
    poll.status = status;
    poll.decided_at = Some(DECIDED_AT.into());
    poll.resolution_note = (status != PollStatus::Passed).then(|| "No decision recorded.".into());
    poll
}

fn replacement_plan_change_poll(status: PollStatus) -> Poll {
    let mut poll = terminal_plan_change_poll(status);
    poll.id = "poll-b".into();
    poll.options[0].id = "adopt-b".into();
    poll.options[1].id = "keep-b".into();
    poll
}

fn ballot(user_id: &str, option_id: &str) -> BallotRecord {
    ballot_for("poll-a", user_id, option_id)
}

fn ballot_for(poll_id: &str, user_id: &str, option_id: &str) -> BallotRecord {
    BallotRecord {
        poll_id: poll_id.into(),
        user_id: user_id.into(),
        option_ids: vec![option_id.into()],
        at: VOTED_AT.into(),
    }
}

fn new_poll() -> NewDecisionPoll {
    NewDecisionPoll {
        id: "poll-a".into(),
        title: "Dinner".into(),
        description: "Pick one".into(),
        options: decision_poll(PollStatus::Open).options,
        closes_at: CLOSES_AT.into(),
        allow_multi: false,
        created_at: CREATED_AT.into(),
    }
}

fn application_ids() -> ProposalApplicationIds {
    ProposalApplicationIds {
        plan_id: "plan-2".into(),
        entity_ids: (0..40).map(|index| format!("entity-{index}")).collect(),
    }
}

fn pending_poll_proposal(op: ChangeOp) -> Proposal {
    Proposal {
        id: "proposal-a".into(),
        trip_id: TRIP_ID.into(),
        created_by: ACTOR_ID.into(),
        source: ChangeSource::Web,
        title: "Add a day".into(),
        rationale: "Make room".into(),
        change_set: ChangeSet {
            base_plan_version: 1,
            ops: vec![op],
        },
        route: ProposalRoute::Poll,
        status: ProposalStatus::Pending,
        decided_by: None,
        rejection_reason: None,
        created_at: CREATED_AT.into(),
    }
}

fn linked_poll_proposal(op: ChangeOp) -> Proposal {
    let mut proposal = pending_poll_proposal(op);
    proposal.decided_by = Some(ProposalDecision::Poll {
        poll_id: "poll-a".into(),
    });
    proposal
}

fn applied_by_replacement_proposal() -> Proposal {
    let mut proposal = linked_poll_proposal(ChangeOp::AddDay {
        date: "2026-11-02".into(),
        city_hint: "Osaka".into(),
    });
    proposal.status = ProposalStatus::Applied;
    proposal.decided_by = Some(ProposalDecision::Poll {
        poll_id: "poll-b".into(),
    });
    proposal
}

fn new_plan_change_poll() -> NewPlanChangePoll {
    NewPlanChangePoll {
        poll_id: "poll-a".into(),
        adopt_option_id: "adopt".into(),
        keep_option_id: "keep".into(),
        created_at: CREATED_AT.into(),
        closes_at: "2026-08-13T10:00:00Z".into(),
    }
}

fn new_repoll() -> NewPlanChangePoll {
    NewPlanChangePoll {
        poll_id: "poll-b".into(),
        adopt_option_id: "adopt-b".into(),
        keep_option_id: "keep-b".into(),
        created_at: CREATED_AT.into(),
        closes_at: "2026-08-13T10:00:00Z".into(),
    }
}

fn membership_rule(role: TripRole) -> aws_smithy_mocks::Rule {
    let item = encode_member(TRIP_ID, &member(ACTOR_ID, role)).expect("member encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
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

fn meta_rule(revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_trip_meta(&meta(), revision).expect("meta encodes");
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

fn newer_meta_rule(revision: u64) -> aws_smithy_mocks::Rule {
    let mut value = meta();
    value.current_plan_id = Some("plan-2".into());
    value.current_plan_version = Some(2);
    let item = encode_trip_meta(&value, revision).expect("meta encodes");
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

fn proposal_rule(proposal: Proposal, revision: u64) -> aws_smithy_mocks::Rule {
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

fn members_rule() -> aws_smithy_mocks::Rule {
    let items = vec![
        encode_member(TRIP_ID, &member(ACTOR_ID, TripRole::Member)).expect("member encodes"),
        encode_member(TRIP_ID, &member("member-b", TripRole::Member)).expect("member encodes"),
        encode_member(TRIP_ID, &member("leader-a", TripRole::Leader)).expect("member encodes"),
        encode_member(TRIP_ID, &member("viewer-a", TripRole::Viewer)).expect("member encodes"),
    ];
    query_rule("MEMBER#", items)
}

fn poll_rule(poll: Poll, revision: u64) -> aws_smithy_mocks::Rule {
    poll_rule_for(poll, revision)
}

fn poll_rule_for(poll: Poll, revision: u64) -> aws_smithy_mocks::Rule {
    let poll_id = poll.id.clone();
    let item = encode_poll(&poll, CREATED_AT, revision).expect("poll encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(poll_sk(&poll_id)))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn missing_poll_rule() -> aws_smithy_mocks::Rule {
    missing_poll_rule_for("foreign-poll")
}

fn missing_poll_rule_for(poll_id: &str) -> aws_smithy_mocks::Rule {
    let poll_id = poll_id.to_string();
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(poll_sk(&poll_id)))
                })
        })
        .then_output(|| GetItemOutput::builder().build())
}

fn query_rule(
    prefix: &'static str,
    items: Vec<HashMap<String, AttributeValue>>,
) -> aws_smithy_mocks::Rule {
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":pk"))
                    == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(prefix.into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(items.clone()))
                .build()
        })
}

fn no_ledger_meta_rule() -> aws_smithy_mocks::Rule {
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(LEDGER_META_SK.into()))
                })
        })
        .then_output(|| GetItemOutput::builder().build())
}

fn empty_ledger_query_rule() -> aws_smithy_mocks::Rule {
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && matches!(
                    request
                        .expression_attribute_values()
                        .and_then(|values| values.get(":prefix")),
                    Some(AttributeValue::S(prefix))
                        if prefix == EXPENSE_PREFIX || prefix == STOP_LINK_PREFIX
                )
        })
        .then_output(|| QueryOutput::builder().build())
}

async fn list_historical_with_replacement(
    predecessor: Poll,
    predecessor_ballots: Vec<BallotRecord>,
    replacement: Option<Poll>,
    replacement_ballots: Vec<BallotRecord>,
    include_replacement_in_list: bool,
) -> Result<Vec<Poll>, PollRepoError> {
    let membership = membership_rule(TripRole::Viewer);
    let meta = meta_rule(9);
    let proposal = proposal_rule(applied_by_replacement_proposal(), 5);
    let mut items = vec![encode_poll(&predecessor, CREATED_AT, 5).expect("predecessor encodes")];
    items.extend(
        predecessor_ballots
            .iter()
            .map(|ballot| encode_ballot(TRIP_ID, ballot, 1).expect("predecessor ballot encodes")),
    );
    if include_replacement_in_list {
        let replacement = replacement.as_ref().expect("listed replacement exists");
        items.push(encode_poll(replacement, CREATED_AT, 6).expect("replacement encodes"));
        items.extend(
            replacement_ballots.iter().map(|ballot| {
                encode_ballot(TRIP_ID, ballot, 1).expect("replacement ballot encodes")
            }),
        );
    }
    let polls = query_rule("POLL#", items);
    let replacement_get = replacement.map_or_else(
        || missing_poll_rule_for("poll-b"),
        |poll| poll_rule_for(poll, 6),
    );
    let replacement_votes = query_rule(
        "POLL#poll-b#VOTE#",
        replacement_ballots
            .iter()
            .map(|ballot| encode_ballot(TRIP_ID, ballot, 1).expect("replacement ballot encodes"))
            .collect(),
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &meta,
            &proposal,
            &polls,
            &replacement_get,
            &replacement_votes
        ]
    );
    DynamoUserRepo::new(client, TABLE)
        .expect("repo")
        .list_polls(TRIP_ID, &actor())
        .await
}

fn plan_rule() -> aws_smithy_mocks::Rule {
    let plan = Plan {
        id: "plan-1".into(),
        trip_id: TRIP_ID.into(),
        version: 1,
        created_from_proposal_id: None,
        created_at: "2026-08-01T00:00:00Z".into(),
    };
    let day = Day {
        id: "day-a".into(),
        plan_id: "plan-1".into(),
        date: "2026-11-01".into(),
        city_hint: "Kyoto".into(),
        tz: "Asia/Tokyo".into(),
        window_start: "09:00".into(),
        window_end: "21:00".into(),
    };
    let items = vec![
        encode_record(trip_pk(TRIP_ID), plan_sk(1), PLAN_ENTITY, &plan, 4).expect("plan encodes"),
        encode_record(trip_pk(TRIP_ID), day_sk(1, &day), DAY_ENTITY, &day, 7).expect("day encodes"),
    ];
    let prefix = format!("{}#", plan_prefix(1));
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(prefix.clone()))
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

fn cancelled_transaction(codes: &[&str]) -> TransactWriteItemsError {
    let mut builder = TransactionCanceledException::builder();
    for code in codes {
        builder = builder.cancellation_reasons(CancellationReason::builder().code(*code).build());
    }
    TransactWriteItemsError::TransactionCanceledException(builder.build())
}

#[test]
fn poll_and_ballot_keys_are_trip_partitioned_and_disjoint() {
    assert_eq!(poll_sk("poll-a"), "POLL#poll-a");
    assert_eq!(ballot_sk("poll-a", ACTOR_ID), "POLL#poll-a#VOTE#member-a");
    assert_ne!(POLL_ENTITY, BALLOT_ENTITY);
    assert!(!poll_sk("poll-a").starts_with("AUDIT#"));
}

#[test]
fn codecs_reject_cross_trip_corruption_unsupported_choices_and_impossible_results() {
    let poll = decision_poll(PollStatus::Open);
    let mut item = encode_poll(&poll, CREATED_AT, 4).expect("poll encodes");
    item.insert(PK.into(), AttributeValue::S(trip_pk("trip-b")));
    assert!(matches!(
        decode_poll(&item, TRIP_ID),
        Err(PollRepoError::CorruptData)
    ));

    assert_eq!(
        validate_ballot_for_poll(&poll, &["foreign-option".into()]),
        Err(PollRepoError::InvalidVote)
    );
    assert_eq!(
        validate_ballot_for_poll(&poll, &["ramen".into(), "ramen".into()]),
        Err(PollRepoError::InvalidVote)
    );
    assert_eq!(
        validate_ballot_for_poll(&poll, &[]),
        Err(PollRepoError::InvalidVote)
    );

    let mut misleading = plan_change_poll();
    misleading.options[0].label = "Keep the current plan".into();
    assert!(matches!(
        encode_poll(&misleading, CREATED_AT, 1),
        Err(PollRepoError::CorruptData)
    ));

    let mut loaded = LoadedPoll {
        poll: decision_poll(PollStatus::Passed),
        revision: 6,
        created_at: CREATED_AT.into(),
    };
    let tied = [
        LoadedBallot {
            ballot: ballot(ACTOR_ID, "ramen"),
            revision: 1,
        },
        LoadedBallot {
            ballot: ballot("member-b", "sushi"),
            revision: 1,
        },
    ];
    assert_eq!(
        attach_ballots(&mut loaded, tied),
        Err(PollRepoError::CorruptData),
        "a stored passed result cannot encode a tie"
    );

    let mut loaded = LoadedPoll {
        poll: poll.clone(),
        revision: 4,
        created_at: CREATED_AT.into(),
    };
    let mut late = ballot(ACTOR_ID, "ramen");
    late.at = CLOSES_AT.into();
    assert_eq!(
        attach_ballots(
            &mut loaded,
            [LoadedBallot {
                ballot: late,
                revision: 1,
            }]
        ),
        Err(PollRepoError::CorruptData),
        "a ballot at or after the deadline is corrupt"
    );
}

#[tokio::test]
async fn plan_change_records_bind_terminal_status_to_the_actual_winner() {
    let membership = membership_rule(TripRole::Viewer);
    let meta = meta_rule(9);
    let mut applied = linked_poll_proposal(ChangeOp::AddDay {
        date: "2026-11-02".into(),
        city_hint: "Osaka".into(),
    });
    applied.status = ProposalStatus::Applied;
    let proposal = proposal_rule(applied, 4);
    let items = vec![
        encode_poll(
            &terminal_plan_change_poll(PollStatus::Passed),
            CREATED_AT,
            6,
        )
        .expect("poll encodes"),
        encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "keep"), 1).expect("ballot encodes"),
        encode_ballot(TRIP_ID, &ballot("member-b", "keep"), 1).expect("ballot encodes"),
    ];
    let polls = query_rule("POLL#", items);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &proposal, &polls]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.list_polls(TRIP_ID, &actor()).await,
        Err(PollRepoError::CorruptData),
        "a keep winner cannot be stored as passed with an applied proposal"
    );

    let membership = membership_rule(TripRole::Viewer);
    let meta = meta_rule(9);
    let mut rejected = linked_poll_proposal(ChangeOp::AddDay {
        date: "2026-11-02".into(),
        city_hint: "Osaka".into(),
    });
    rejected.status = ProposalStatus::Rejected;
    rejected.rejection_reason = Some("Rejected".into());
    let proposal = proposal_rule(rejected, 4);
    let items = vec![
        encode_poll(
            &terminal_plan_change_poll(PollStatus::Failed),
            CREATED_AT,
            6,
        )
        .expect("poll encodes"),
        encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "adopt"), 1).expect("ballot encodes"),
        encode_ballot(TRIP_ID, &ballot("member-b", "adopt"), 1).expect("ballot encodes"),
    ];
    let polls = query_rule("POLL#", items);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &proposal, &polls]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.list_polls(TRIP_ID, &actor()).await,
        Err(PollRepoError::CorruptData),
        "an adopt winner may fail only because its linked proposal became stale"
    );
}

#[tokio::test]
async fn historical_no_decision_poll_stays_readable_after_replacement_resolves() {
    let membership = membership_rule(TripRole::Viewer);
    let meta = meta_rule(9);
    let proposal = proposal_rule(applied_by_replacement_proposal(), 5);

    let old_poll = terminal_plan_change_poll(PollStatus::Failed);
    let replacement_poll = replacement_plan_change_poll(PollStatus::Passed);
    let old_ballots = [
        ballot_for("poll-a", ACTOR_ID, "adopt"),
        ballot_for("poll-a", "member-b", "keep"),
    ];
    let replacement_ballots = [
        ballot_for("poll-b", ACTOR_ID, "adopt-b"),
        ballot_for("poll-b", "member-b", "adopt-b"),
    ];
    let mut items = vec![
        encode_poll(&old_poll, CREATED_AT, 5).expect("old poll encodes"),
        encode_poll(&replacement_poll, CREATED_AT, 6).expect("replacement poll encodes"),
    ];
    items.extend(
        old_ballots
            .iter()
            .chain(&replacement_ballots)
            .map(|ballot| encode_ballot(TRIP_ID, ballot, 1).expect("ballot encodes")),
    );
    let polls = query_rule("POLL#", items);
    let replacement_get = poll_rule_for(replacement_poll, 6);
    let replacement_votes = query_rule(
        "POLL#poll-b#VOTE#",
        replacement_ballots
            .iter()
            .map(|ballot| encode_ballot(TRIP_ID, ballot, 1).expect("ballot encodes"))
            .collect(),
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &meta,
            &proposal,
            &polls,
            &replacement_get,
            &replacement_votes
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let listed = repo
        .list_polls(TRIP_ID, &actor())
        .await
        .expect("both generations remain valid governance history");
    assert_eq!(
        listed
            .iter()
            .map(|poll| poll.id.as_str())
            .collect::<Vec<_>>(),
        ["poll-b", "poll-a"]
    );
    assert_eq!(listed[0].status, PollStatus::Passed);
    assert_eq!(listed[1].status, PollStatus::Failed);
    assert_eq!(replacement_get.num_calls(), 1);
}

#[tokio::test]
async fn expired_poll_stays_readable_after_replacement_resolves() {
    let listed = list_historical_with_replacement(
        terminal_plan_change_poll(PollStatus::Expired),
        vec![],
        Some(replacement_plan_change_poll(PollStatus::Passed)),
        vec![
            ballot_for("poll-b", ACTOR_ID, "adopt-b"),
            ballot_for("poll-b", "member-b", "adopt-b"),
        ],
        true,
    )
    .await
    .expect("expired predecessor remains valid history");
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|poll| poll.status == PollStatus::Expired));
}

#[tokio::test]
async fn historical_poll_rejects_a_missing_replacement() {
    assert_eq!(
        list_historical_with_replacement(
            terminal_plan_change_poll(PollStatus::Failed),
            vec![
                ballot_for("poll-a", ACTOR_ID, "adopt"),
                ballot_for("poll-a", "member-b", "keep"),
            ],
            None,
            vec![],
            false,
        )
        .await,
        Err(PollRepoError::CorruptData)
    );
}

#[tokio::test]
async fn historical_poll_rejects_a_non_plan_change_replacement() {
    let mut replacement = decision_poll(PollStatus::Open);
    replacement.id = "poll-b".into();
    assert_eq!(
        list_historical_with_replacement(
            terminal_plan_change_poll(PollStatus::Failed),
            vec![
                ballot_for("poll-a", ACTOR_ID, "adopt"),
                ballot_for("poll-a", "member-b", "keep"),
            ],
            Some(replacement),
            vec![],
            false,
        )
        .await,
        Err(PollRepoError::CorruptData)
    );
}

#[tokio::test]
async fn historical_poll_rejects_a_replacement_result_that_contradicts_the_proposal() {
    assert_eq!(
        list_historical_with_replacement(
            terminal_plan_change_poll(PollStatus::Failed),
            vec![
                ballot_for("poll-a", ACTOR_ID, "adopt"),
                ballot_for("poll-a", "member-b", "keep"),
            ],
            Some(replacement_plan_change_poll(PollStatus::Passed)),
            vec![
                ballot_for("poll-b", ACTOR_ID, "keep-b"),
                ballot_for("poll-b", "member-b", "keep-b"),
            ],
            false,
        )
        .await,
        Err(PollRepoError::CorruptData)
    );
}

#[tokio::test]
async fn poll_routed_proposal_is_preflighted_then_created_with_its_poll_atomically() {
    let membership = membership_rule(TripRole::Member);
    let meta = meta_rule(9);
    let members = members_rule();
    let plan = plan_rule();
    let candidates = query_rule("CANDIDATE#", vec![]);
    let ledger_meta = no_ledger_meta_rule();
    let ledger_queries = empty_ledger_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 4
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
                            .and_then(|values| values.get(":plan_id"))
                            == Some(&AttributeValue::S("plan-1".into()))
                })
                && items[2].put().is_some_and(|put| {
                    let proposal = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Proposal>(data).ok());
                    put.item().get(SK)
                        == Some(&AttributeValue::S("PROPOSAL#proposal-a".into()))
                        && put.condition_expression()
                            == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                        && proposal.is_some_and(|proposal| {
                            proposal.status == ProposalStatus::Pending
                                && proposal.route == ProposalRoute::Poll
                                && proposal.decided_by
                                    == Some(ProposalDecision::Poll {
                                        poll_id: "poll-a".into(),
                                    })
                        })
                })
                && items[3].put().is_some_and(|put| {
                    let loaded = decode_poll(put.item(), TRIP_ID).ok();
                    put.item().get(SK) == Some(&AttributeValue::S(poll_sk("poll-a")))
                        && put.condition_expression()
                            == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                        && loaded.is_some_and(|loaded| {
                            loaded.poll.kind == PollKind::PlanChange
                                && loaded.poll.status == PollStatus::Open
                                && loaded.poll.quorum == 2
                                && loaded.poll.options.iter().any(|option| {
                                    option.proposal_id.as_deref() == Some("proposal-a")
                                        && option.label == "Adopt the proposed plan change"
                                })
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &meta,
            &members,
            &plan,
            &candidates,
            &ledger_meta,
            &ledger_queries,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let proposal = pending_poll_proposal(ChangeOp::AddDay {
        date: "2026-11-02".into(),
        city_hint: "Osaka".into(),
    });

    let created = repo
        .create_proposal_poll(
            TRIP_ID,
            &actor(),
            proposal,
            new_plan_change_poll(),
            application_ids(),
        )
        .await
        .expect("proposal and poll commit together");
    assert_eq!(
        created.decided_by,
        Some(ProposalDecision::Poll {
            poll_id: "poll-a".into()
        })
    );
    assert_eq!(plan.num_calls(), 1, "the ChangeSet is preflighted");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn invalid_poll_routed_proposal_writes_neither_record() {
    let membership = membership_rule(TripRole::Member);
    let meta = meta_rule(9);
    let members = members_rule();
    let plan = plan_rule();
    let ledger_meta = no_ledger_meta_rule();
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &members, &plan, &ledger_meta]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let proposal = pending_poll_proposal(ChangeOp::RemoveStop {
        stop_id: "foreign-stop".into(),
    });

    assert_eq!(
        repo.create_proposal_poll(
            TRIP_ID,
            &actor(),
            proposal,
            new_plan_change_poll(),
            application_ids(),
        )
        .await,
        Err(PollRepoError::NotFound)
    );
}

#[tokio::test]
async fn failed_repoll_does_not_return_the_unchanged_terminal_poll() {
    let membership = membership_rule(TripRole::Leader);
    let proposal = proposal_rule(
        linked_poll_proposal(ChangeOp::AddDay {
            date: "2026-11-02".into(),
            city_hint: "Osaka".into(),
        }),
        3,
    );
    let poll = poll_rule(terminal_plan_change_poll(PollStatus::Failed), 5);
    let ballots = query_rule(
        "POLL#poll-a#VOTE#",
        vec![
            encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "adopt"), 1).expect("ballot encodes"),
            encode_ballot(TRIP_ID, &ballot("member-b", "keep"), 1).expect("ballot encodes"),
        ],
    );
    let meta = meta_rule(9);
    let members = members_rule();
    let plan = plan_rule();
    let candidates = query_rule("CANDIDATE#", vec![]);
    let ledger_meta = no_ledger_meta_rule();
    let ledger_queries = empty_ledger_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_error(|| cancelled_transaction(&[CONDITIONAL_FAILURE]));
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &proposal,
            &poll,
            &ballots,
            &meta,
            &members,
            &plan,
            &candidates,
            &ledger_meta,
            &ledger_queries,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.route_proposal_to_poll(
            TRIP_ID,
            &actor(),
            "proposal-a",
            new_repoll(),
            application_ids(),
        )
        .await,
        Err(PollRepoError::Conflict),
        "a current-plan collision cannot masquerade as a successful re-poll"
    );
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn failed_repoll_returns_a_concurrently_created_replacement_poll() {
    let membership = membership_rule(TripRole::Leader);
    let old_proposal = linked_poll_proposal(ChangeOp::AddDay {
        date: "2026-11-02".into(),
        city_hint: "Osaka".into(),
    });
    let mut new_proposal = old_proposal.clone();
    new_proposal.decided_by = Some(ProposalDecision::Poll {
        poll_id: "poll-b".into(),
    });
    let old_proposal_item = encode_proposal(&old_proposal, 3).expect("proposal encodes");
    let new_proposal_item = encode_proposal(&new_proposal, 4).expect("proposal encodes");
    let proposal_calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&proposal_calls);
    let proposal = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S(proposal_sk("proposal-a")))
            })
        })
        .then_output(move || {
            let item = if observed_calls.fetch_add(1, Ordering::SeqCst) < 2 {
                old_proposal_item.clone()
            } else {
                new_proposal_item.clone()
            };
            GetItemOutput::builder().set_item(Some(item)).build()
        });

    let old_poll = terminal_plan_change_poll(PollStatus::Failed);
    let mut replacement_poll = plan_change_poll();
    replacement_poll.id = "poll-b".into();
    replacement_poll.options[0].id = "adopt-b".into();
    replacement_poll.options[1].id = "keep-b".into();
    replacement_poll.closes_at = "2026-08-13T10:00:00Z".into();
    let old_poll_item = encode_poll(&old_poll, CREATED_AT, 5).expect("poll encodes");
    let replacement_poll_item =
        encode_poll(&replacement_poll, CREATED_AT, 1).expect("poll encodes");
    let old_poll_get = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request
                .key()
                .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(poll_sk("poll-a"))))
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(old_poll_item.clone()))
                .build()
        });
    let replacement_poll_get = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request
                .key()
                .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(poll_sk("poll-b"))))
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(replacement_poll_item.clone()))
                .build()
        });
    let old_ballots = query_rule(
        "POLL#poll-a#VOTE#",
        vec![
            encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "adopt"), 1).expect("ballot encodes"),
            encode_ballot(TRIP_ID, &ballot("member-b", "keep"), 1).expect("ballot encodes"),
        ],
    );
    let replacement_ballots = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":pk"))
                    == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("POLL#poll-b#VOTE#".into()))
        })
        .then_output(|| QueryOutput::builder().build());
    let meta = meta_rule(9);
    let members = members_rule();
    let plan = plan_rule();
    let candidates = query_rule("CANDIDATE#", vec![]);
    let ledger_meta = no_ledger_meta_rule();
    let ledger_queries = empty_ledger_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_error(|| cancelled_transaction(&[CONDITIONAL_FAILURE]));
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &proposal,
            &old_poll_get,
            &replacement_poll_get,
            &old_ballots,
            &replacement_ballots,
            &meta,
            &members,
            &plan,
            &candidates,
            &ledger_meta,
            &ledger_queries,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let concurrent = repo
        .route_proposal_to_poll(
            TRIP_ID,
            &actor(),
            "proposal-a",
            new_repoll(),
            application_ids(),
        )
        .await
        .expect("the concurrent replacement is returned");
    assert_eq!(concurrent.id, "poll-b");
    assert_eq!(proposal_calls.load(Ordering::SeqCst), 4);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn viewer_can_list_but_cannot_create_a_poll() {
    let membership = membership_rule(TripRole::Viewer);
    let meta = meta_rule(9);
    let polls = query_rule("POLL#", vec![]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &polls]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.list_polls(TRIP_ID, &actor()).await,
        Ok(vec![]),
        "viewers may read trip governance history"
    );
    assert_eq!(
        repo.create_decision_poll(TRIP_ID, &actor(), new_poll())
            .await,
        Err(PollRepoError::Forbidden),
        "viewers remain read-only"
    );
}

#[tokio::test]
async fn decision_creation_freezes_editor_quorum_with_exact_atomic_guards() {
    let membership = membership_rule(TripRole::Member);
    let meta = meta_rule(9);
    let members = members_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].condition_check().is_some_and(|condition| {
                    condition.key().get(SK)
                        == Some(&AttributeValue::S(format!("MEMBER#{ACTOR_ID}")))
                        && condition.condition_expression()
                            == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && condition.condition_expression()
                            == "#entity = :trip AND #revision = :revision AND #member_count = :member_count"
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("9".into()))
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":member_count"))
                            == Some(&AttributeValue::N("4".into()))
                })
                && items[2].put().is_some_and(|put| {
                    let loaded = decode_poll(put.item(), TRIP_ID).ok();
                    put.item().get(SK) == Some(&AttributeValue::S(poll_sk("poll-a")))
                        && put.condition_expression()
                            == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                        && loaded.is_some_and(|loaded| {
                            loaded.revision == 1
                                && loaded.poll.quorum == 2
                                && loaded.poll.status == PollStatus::Open
                                && loaded.poll.created_by == ACTOR_ID
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &members, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let created = repo
        .create_decision_poll(TRIP_ID, &actor(), new_poll())
        .await
        .expect("poll created");
    assert_eq!(created.quorum, 2, "the viewer is excluded from quorum");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn ballot_write_is_actor_owned_revision_guarded_and_atomic() {
    let membership = membership_rule(TripRole::Member);
    let poll = poll_rule(decision_poll(PollStatus::Open), 5);
    let ballots = query_rule("POLL#poll-a#VOTE#", vec![]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(poll_sk("poll-a")))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression() == Some("#revision = :revision")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("5".into()))
                })
                && items[2].put().is_some_and(|put| {
                    let ballot = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<BallotRecord>(data).ok());
                    put.item().get(ENTITY_TYPE) == Some(&AttributeValue::S(BALLOT_ENTITY.into()))
                        && put.item().get(SK)
                            == Some(&AttributeValue::S(ballot_sk("poll-a", ACTOR_ID)))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("1".into()))
                        && put.condition_expression()
                            == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                        && ballot.is_some_and(|ballot| {
                            ballot.user_id == ACTOR_ID
                                && ballot.option_ids == ["ramen"]
                                && ballot.at == VOTED_AT
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &poll, &ballots, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let voted = repo
        .cast_vote(TRIP_ID, &actor(), "poll-a", &["ramen".into()], VOTED_AT)
        .await
        .expect("vote commits");
    assert_eq!(voted.votes.len(), 1);
    assert_eq!(voted.votes[0].user_id, ACTOR_ID);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn clock_rollback_vote_is_rejected_before_any_write() {
    let membership = membership_rule(TripRole::Member);
    let poll = poll_rule(decision_poll(PollStatus::Open), 5);
    let ballots = query_rule("POLL#poll-a#VOTE#", vec![]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &poll, &ballots, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.cast_vote(
            TRIP_ID,
            &actor(),
            "poll-a",
            &["ramen".into()],
            "2026-08-06T09:59:59Z",
        )
        .await,
        Err(PollRepoError::Conflict)
    );
    assert_eq!(transaction.num_calls(), 0, "invalid time must not persist");
}

#[tokio::test]
async fn concurrent_ballot_collision_fails_without_accepting_stale_state() {
    let membership = membership_rule(TripRole::Member);
    let poll = poll_rule(decision_poll(PollStatus::Open), 5);
    let ballots = query_rule("POLL#poll-a#VOTE#", vec![]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_error(|| cancelled_transaction(&[CONDITIONAL_FAILURE]));
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &poll, &ballots, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.cast_vote(TRIP_ID, &actor(), "poll-a", &["ramen".into()], VOTED_AT)
            .await,
        Err(PollRepoError::Conflict)
    );
    assert_eq!(
        membership.num_calls(),
        2,
        "role is rechecked after collision"
    );
    assert_eq!(poll.num_calls(), 2, "the winning state is reloaded");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn close_uses_one_ballot_snapshot_and_exact_leader_revision_guard() {
    let membership = membership_rule(TripRole::Leader);
    let poll = poll_rule(decision_poll(PollStatus::Open), 5);
    let ballots = query_rule(
        "POLL#poll-a#VOTE#",
        vec![
            encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "ramen"), 1).expect("ballot encodes"),
            encode_ballot(TRIP_ID, &ballot("member-b", "ramen"), 1).expect("ballot encodes"),
        ],
    );
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 2
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression() == "#entity = :member AND #role = :leader"
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":leader"))
                            == Some(&AttributeValue::S("leader".into()))
                })
                && items[1].put().is_some_and(|put| {
                    let loaded = decode_poll(put.item(), TRIP_ID).ok();
                    put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression() == Some("#revision = :revision")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("5".into()))
                        && loaded.is_some_and(|loaded| {
                            loaded.poll.status == PollStatus::Passed
                                && loaded.poll.decided_at.as_deref() == Some(DECIDED_AT)
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &poll, &ballots, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let closed = repo
        .close_poll(TRIP_ID, &actor(), "poll-a", DECIDED_AT, application_ids())
        .await
        .expect("leader close commits");
    assert_eq!(closed.status, PollStatus::Passed);
    assert_eq!(closed.votes.len(), 2);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn keep_result_rejects_the_linked_proposal_in_the_same_transaction() {
    let membership = membership_rule(TripRole::Leader);
    let poll = poll_rule(plan_change_poll(), 5);
    let proposal = proposal_rule(
        linked_poll_proposal(ChangeOp::AddDay {
            date: "2026-11-02".into(),
            city_hint: "Osaka".into(),
        }),
        3,
    );
    let ballots = query_rule(
        "POLL#poll-a#VOTE#",
        vec![
            encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "keep"), 1).expect("ballot encodes"),
            encode_ballot(TRIP_ID, &ballot("member-b", "keep"), 1).expect("ballot encodes"),
        ],
    );
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression() == "#entity = :member AND #role = :leader"
                })
                && items[1].put().is_some_and(|put| {
                    let loaded = decode_poll(put.item(), TRIP_ID).ok();
                    put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression() == Some("#revision = :revision")
                        && loaded.is_some_and(|loaded| {
                            loaded.poll.status == PollStatus::Failed
                                && loaded.poll.resolution_note.as_deref()
                                    == Some("The group chose to keep the current plan.")
                        })
                })
                && items[2].put().is_some_and(|put| {
                    let proposal = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Proposal>(data).ok());
                    put.item().get(REVISION) == Some(&AttributeValue::N("4".into()))
                        && put.condition_expression() == Some("#revision = :revision")
                        && proposal.is_some_and(|proposal| {
                            proposal.status == ProposalStatus::Rejected
                                && proposal.rejection_reason.as_deref()
                                    == Some("The group chose to keep the current plan.")
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &poll, &proposal, &ballots, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let closed = repo
        .close_poll(TRIP_ID, &actor(), "poll-a", DECIDED_AT, application_ids())
        .await
        .expect("keep result commits");
    assert_eq!(closed.status, PollStatus::Failed);
    assert_eq!(
        proposal.num_calls(),
        2,
        "the link is checked before deciding"
    );
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn adopt_result_publishes_plan_proposal_candidate_and_poll_in_one_transaction() {
    let membership = membership_rule(TripRole::Leader);
    let poll = poll_rule(plan_change_poll(), 5);
    let proposal = proposal_rule(
        linked_poll_proposal(ChangeOp::AddStop {
            day_id: "day-a".into(),
            place_id: "place-candidate".into(),
            seq: 1.0,
            stop_kind: StopKind::Meal,
        }),
        3,
    );
    let ballots = query_rule(
        "POLL#poll-a#VOTE#",
        vec![
            encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "adopt"), 1).expect("ballot encodes"),
            encode_ballot(TRIP_ID, &ballot("member-b", "adopt"), 1).expect("ballot encodes"),
        ],
    );
    let meta = meta_rule(9);
    let plan = plan_rule();
    let place = place_get_rule(candidate_place());
    let candidates = candidate_query_rule(candidate(CandidateStatus::Shortlisted), 11);
    let ledger_meta = no_ledger_meta_rule();
    let ledger_queries = empty_ledger_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            if items.len() != 11 {
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
                    && meta.is_some_and(|meta| {
                        meta.current_plan_id.as_deref() == Some("plan-2")
                            && meta.current_plan_version == Some(2)
                    })
            });
            let proposal_ok = items[2].put().is_some_and(|put| {
                let proposal = put
                    .item()
                    .get(DATA)
                    .and_then(|value| value.as_s().ok())
                    .and_then(|data| serde_json::from_str::<Proposal>(data).ok());
                put.item().get(REVISION) == Some(&AttributeValue::N("4".into()))
                    && put.condition_expression() == Some("#revision = :revision")
                    && put
                        .expression_attribute_values()
                        .and_then(|values| values.get(":revision"))
                        == Some(&AttributeValue::N("3".into()))
                    && proposal.is_some_and(|proposal| {
                        proposal.status == ProposalStatus::Applied
                            && proposal.decided_by
                                == Some(ProposalDecision::Poll {
                                    poll_id: "poll-a".into(),
                                })
                    })
            });
            let source_plan_ok = items[3].condition_check().is_some_and(|condition| {
                condition.key().get(SK) == Some(&AttributeValue::S(plan_sk(1)))
                    && condition.condition_expression()
                        == "#entity = :entity AND #revision = :revision"
                    && condition
                        .expression_attribute_values()
                        .and_then(|values| values.get(":revision"))
                        == Some(&AttributeValue::N("4".into()))
            });
            let source_day_ok = items[4].condition_check().is_some_and(|condition| {
                condition
                    .expression_attribute_values()
                    .and_then(|values| values.get(":revision"))
                    == Some(&AttributeValue::N("7".into()))
            });
            let ledger_ok = items[5].condition_check().is_some_and(|condition| {
                condition.key().get(SK) == Some(&AttributeValue::S(LEDGER_META_SK.into()))
                    && condition.condition_expression()
                        == "attribute_not_exists(#pk) AND attribute_not_exists(#sk)"
            });
            let plan_ok = items[6].put().is_some_and(|put| {
                put.condition_expression()
                    == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                    && put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Plan>(data).ok())
                        .is_some_and(|plan| plan.id == "plan-2" && plan.version == 2)
            });
            let day_ok = items[7].put().is_some_and(|put| {
                put.condition_expression()
                    == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                    && put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Day>(data).ok())
                        .is_some_and(|day| day.plan_id == "plan-2")
            });
            let stop_ok = items[8].put().is_some_and(|put| {
                let stop = put
                    .item()
                    .get(DATA)
                    .and_then(|value| value.as_s().ok())
                    .and_then(|data| serde_json::from_str::<Stop>(data).ok());
                put.condition_expression()
                    == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                    && stop.is_some_and(|stop| {
                        stop.id == "entity-0"
                            && stop.day_id == "day-a"
                            && stop.place_id == "place-candidate"
                    })
            });
            let candidate_ok = items[9].put().is_some_and(|put| {
                let candidate = put
                    .item()
                    .get(DATA)
                    .and_then(|value| value.as_s().ok())
                    .and_then(|data| serde_json::from_str::<Candidate>(data).ok());
                put.item().get(SK) == Some(&AttributeValue::S(candidate_sk("candidate-a")))
                    && put.item().get(REVISION) == Some(&AttributeValue::N("12".into()))
                    && put.condition_expression() == Some("#revision = :revision")
                    && candidate
                        .is_some_and(|candidate| candidate.status == CandidateStatus::InPlan)
            });
            let poll_ok = items[10].put().is_some_and(|put| {
                let poll = decode_poll(put.item(), TRIP_ID).ok();
                put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                    && put.condition_expression() == Some("#revision = :revision")
                    && put
                        .expression_attribute_values()
                        .and_then(|values| values.get(":revision"))
                        == Some(&AttributeValue::N("5".into()))
                    && poll.is_some_and(|poll| {
                        poll.poll.status == PollStatus::Passed
                            && poll.poll.decided_at.as_deref() == Some(DECIDED_AT)
                    })
            });
            member_ok
                && meta_ok
                && proposal_ok
                && source_plan_ok
                && source_day_ok
                && ledger_ok
                && plan_ok
                && day_ok
                && stop_ok
                && candidate_ok
                && poll_ok
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &poll,
            &proposal,
            &ballots,
            &meta,
            &plan,
            &place,
            &candidates,
            &ledger_meta,
            &ledger_queries,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let closed = repo
        .close_poll(TRIP_ID, &actor(), "poll-a", DECIDED_AT, application_ids())
        .await
        .expect("adopt publishes every related aggregate atomically");
    assert_eq!(closed.status, PollStatus::Passed);
    assert_eq!(place.num_calls(), 2, "candidate ownership is revalidated");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn stale_adopt_result_marks_poll_and_proposal_without_plan_writes() {
    let membership = membership_rule(TripRole::Leader);
    let poll = poll_rule(plan_change_poll(), 5);
    let proposal = proposal_rule(
        linked_poll_proposal(ChangeOp::AddDay {
            date: "2026-11-02".into(),
            city_hint: "Osaka".into(),
        }),
        3,
    );
    let ballots = query_rule(
        "POLL#poll-a#VOTE#",
        vec![
            encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "adopt"), 1).expect("ballot encodes"),
            encode_ballot(TRIP_ID, &ballot("member-b", "adopt"), 1).expect("ballot encodes"),
        ],
    );
    let meta = newer_meta_rule(10);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 4
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
                    let loaded = decode_poll(put.item(), TRIP_ID).ok();
                    loaded.is_some_and(|loaded| {
                        loaded.poll.status == PollStatus::Failed
                            && loaded.poll.resolution_note.as_deref()
                                == Some(
                                    "The winning proposal was based on an outdated plan - no plan change was applied."
                                )
                    })
                })
                && items[3].put().is_some_and(|put| {
                    let proposal = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Proposal>(data).ok());
                    proposal.is_some_and(|proposal| {
                        proposal.status == ProposalStatus::Stale
                            && proposal.decided_by
                                == Some(ProposalDecision::Poll {
                                    poll_id: "poll-a".into(),
                                })
                    })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &poll, &proposal, &ballots, &meta, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let closed = repo
        .close_poll(TRIP_ID, &actor(), "poll-a", DECIDED_AT, application_ids())
        .await
        .expect("stale result commits without publishing a plan");
    assert_eq!(closed.status, PollStatus::Failed);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn terminal_close_and_repeated_ballot_are_idempotent_without_writes() {
    let membership = membership_rule(TripRole::Leader);
    let poll = poll_rule(decision_poll(PollStatus::Passed), 6);
    let ballots = query_rule(
        "POLL#poll-a#VOTE#",
        vec![
            encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "ramen"), 1).expect("ballot encodes"),
            encode_ballot(TRIP_ID, &ballot("member-b", "ramen"), 1).expect("ballot encodes"),
        ],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &poll, &ballots]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let closed = repo
        .close_poll(TRIP_ID, &actor(), "poll-a", DECIDED_AT, application_ids())
        .await
        .expect("terminal retry is a read-only success");
    assert_eq!(closed.status, PollStatus::Passed);

    let membership = membership_rule(TripRole::Member);
    let poll = poll_rule(decision_poll(PollStatus::Open), 5);
    let ballots = query_rule(
        "POLL#poll-a#VOTE#",
        vec![encode_ballot(TRIP_ID, &ballot(ACTOR_ID, "ramen"), 3).expect("ballot encodes")],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &poll, &ballots]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let repeated = repo
        .cast_vote(TRIP_ID, &actor(), "poll-a", &["ramen".into()], VOTED_AT)
        .await
        .expect("same ballot is a read-only success");
    assert_eq!(repeated.votes.len(), 1);
}

#[tokio::test]
async fn foreign_poll_id_is_resolved_only_inside_the_authorized_trip_partition() {
    let membership = membership_rule(TripRole::Member);
    let missing = missing_poll_rule();
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &missing]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.cast_vote(
            TRIP_ID,
            &actor(),
            "foreign-poll",
            &["ramen".into()],
            VOTED_AT,
        )
        .await,
        Err(PollRepoError::NotFound)
    );
    assert_eq!(missing.num_calls(), 1);
}

#[test]
fn response_and_storage_limits_remain_explicit() {
    assert_eq!(MAX_POLL_BYTES, 4 * 1_024 * 1_024);
    let over_limit = serde_json::to_vec(&vec!["x".repeat(MAX_POLL_BYTES)])
        .expect("fixture serializes")
        .len();
    assert!(over_limit > MAX_POLL_BYTES);
}
