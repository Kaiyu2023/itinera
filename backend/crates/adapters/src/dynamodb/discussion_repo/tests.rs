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
        discussion::{Comment, DiscussionThread, Reaction, ThreadAnchor},
        poll::{Poll, PollKind, PollOption, PollStatus},
        trip::{
            Candidate, CandidateStatus, Day, Plan, Stop, StopKind, TripMember, TripRole, TripStatus,
        },
        user::UserId,
    },
    ports::discussion::{DiscussionRepo, DiscussionRepoError, NewComment, NewThread},
};

use crate::dynamodb::{
    CONDITIONAL_FAILURE, CREATE_ONLY_CONDITION, DynamoUserRepo, PK, REVISION, SK,
    poll_repo::records::{encode_poll, poll_sk},
    trip_repo::records::{
        CANDIDATE_ENTITY, DAY_ENTITY, META_SK, PLAN_ENTITY, STOP_ENTITY, TripMeta, candidate_sk,
        day_sk, encode_member, encode_record, encode_trip_meta, plan_prefix, plan_sk, stop_sk,
        trip_pk,
    },
};

use super::{
    access::MAX_DISCUSSION_BYTES,
    records::{
        AnchorClaimRecord, COMMENT_ENTITY, DISCUSSION_META_ENTITY, DISCUSSION_META_SK,
        DiscussionMetaRecord, THREAD_ANCHOR_ENTITY, THREAD_ANCHOR_PREFIX, THREAD_ENTITY,
        THREAD_PREFIX, anchor_claim_sk, comment_prefix, comment_sk, decode_anchor_claim,
        decode_comment, decode_discussion_meta, decode_thread, encode_anchor_claim, encode_comment,
        encode_discussion_meta, encode_thread, thread_sk,
    },
};

const TABLE: &str = "itinera-test";
const TRIP_ID: &str = "trip-a";
const ACTOR_ID: &str = "member-a";
const CREATED_AT: &str = "2026-08-06T10:00:00Z";
const LATER_AT: &str = "2026-08-06T10:01:00Z";

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

fn trip_meta() -> TripMeta {
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
        current_plan_id: Some("plan-a".into()),
        current_plan_version: Some(1),
        created_at: "2026-08-01T00:00:00Z".into(),
        member_count: 3,
        leader_count: 1,
        cities: vec!["Kyoto".into()],
    }
}

fn thread(id: &str, anchor: ThreadAnchor, count: u32, activity: &str) -> DiscussionThread {
    DiscussionThread {
        id: id.into(),
        trip_id: TRIP_ID.into(),
        anchor,
        title: format!("Thread {id}"),
        comment_count: count,
        last_activity_at: activity.into(),
    }
}

fn comment(id: &str, thread_id: &str, author: &str, at: &str) -> Comment {
    Comment {
        id: id.into(),
        thread_id: thread_id.into(),
        author: author.into(),
        body: format!("Comment {id}"),
        created_at: at.into(),
        reactions: vec![],
    }
}

fn new_thread(anchor: ThreadAnchor) -> NewThread {
    NewThread {
        id: "thread-new".into(),
        first_comment_id: "comment-first".into(),
        anchor,
        title: "General".into(),
        body: "First thought".into(),
        created_at: CREATED_AT.into(),
    }
}

fn new_comment() -> NewComment {
    NewComment {
        id: "comment-new".into(),
        body: "Follow-up".into(),
        created_at: LATER_AT.into(),
    }
}

fn membership_rule(role: TripRole) -> aws_smithy_mocks::Rule {
    let item = encode_member(TRIP_ID, &member(role)).expect("member encodes");
    get_rule(format!("MEMBER#{ACTOR_ID}"), Some(item))
}

fn trip_meta_rule(revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_trip_meta(&trip_meta(), revision).expect("trip meta encodes");
    get_rule(META_SK.into(), Some(item))
}

fn discussion_meta_rule(count: Option<u32>, revision: u64) -> aws_smithy_mocks::Rule {
    let item = count.map(|thread_count| {
        encode_discussion_meta(
            &DiscussionMetaRecord {
                trip_id: TRIP_ID.into(),
                thread_count,
            },
            revision,
        )
        .expect("discussion meta encodes")
    });
    get_rule(DISCUSSION_META_SK.into(), item)
}

fn thread_get_rule(
    value: DiscussionThread,
    created_at: &str,
    revision: u64,
) -> aws_smithy_mocks::Rule {
    let sk = thread_sk(&value.id);
    let item = encode_thread(&value, created_at, revision).expect("thread encodes");
    get_rule(sk, Some(item))
}

fn claim_get_rule(anchor: ThreadAnchor, thread_id: &str) -> aws_smithy_mocks::Rule {
    let claim = AnchorClaimRecord {
        trip_id: TRIP_ID.into(),
        anchor,
        thread_id: thread_id.into(),
    };
    let sk = anchor_claim_sk(&claim.anchor).expect("anchor key");
    let item = encode_anchor_claim(&claim).expect("claim encodes");
    get_rule(sk, Some(item))
}

fn missing_claim_rule(anchor: &ThreadAnchor) -> aws_smithy_mocks::Rule {
    get_rule(anchor_claim_sk(anchor).expect("anchor key"), None)
}

fn get_rule(
    sort_key: String,
    item: Option<HashMap<String, AttributeValue>>,
) -> aws_smithy_mocks::Rule {
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.table_name() == Some(TABLE)
                && request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(sort_key.clone()))
                })
        })
        .then_output(move || GetItemOutput::builder().set_item(item.clone()).build())
}

fn query_rule(
    prefix: impl Into<String>,
    items: Vec<HashMap<String, AttributeValue>>,
) -> aws_smithy_mocks::Rule {
    let prefix = prefix.into();
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.table_name() == Some(TABLE)
                && request.consistent_read() == Some(true)
                && request.limit() == Some(100)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":pk"))
                    == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
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

fn query_sequence_rule(
    prefix: impl Into<String>,
    outputs: Vec<Vec<HashMap<String, AttributeValue>>>,
) -> (aws_smithy_mocks::Rule, Arc<AtomicUsize>) {
    let prefix = prefix.into();
    let calls = Arc::new(AtomicUsize::new(0));
    let sequence = calls.clone();
    let rule = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.table_name() == Some(TABLE)
                && request.consistent_read() == Some(true)
                && request.limit() == Some(100)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":pk"))
                    == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(prefix.clone()))
        })
        .then_output(move || {
            let index = sequence
                .fetch_add(1, Ordering::SeqCst)
                .min(outputs.len().saturating_sub(1));
            QueryOutput::builder()
                .set_items(Some(outputs[index].clone()))
                .build()
        });
    (rule, calls)
}

fn cancelled_transaction() -> TransactWriteItemsError {
    TransactWriteItemsError::TransactionCanceledException(
        TransactionCanceledException::builder()
            .cancellation_reasons(
                CancellationReason::builder()
                    .code(CONDITIONAL_FAILURE)
                    .build(),
            )
            .build(),
    )
}

fn plan_rows(
    include_stop: bool,
) -> (
    Plan,
    Day,
    Option<Stop>,
    Vec<HashMap<String, AttributeValue>>,
) {
    let plan = Plan {
        id: "plan-a".into(),
        trip_id: TRIP_ID.into(),
        version: 1,
        created_from_proposal_id: None,
        created_at: "2026-08-01T00:00:00Z".into(),
    };
    let day = Day {
        id: "day-a".into(),
        plan_id: "plan-a".into(),
        date: "2026-11-01".into(),
        city_hint: "Kyoto".into(),
        tz: "Asia/Tokyo".into(),
        window_start: "09:00".into(),
        window_end: "21:00".into(),
    };
    let stop = include_stop.then(|| Stop {
        id: "stop-a".into(),
        day_id: "day-a".into(),
        place_id: "place-a".into(),
        seq: 1.0,
        stop_kind: StopKind::Visit,
        planned_arrival: "10:00".into(),
        duration_min: 60,
        notes: String::new(),
        booking: None,
    });
    let mut rows = vec![
        encode_record(trip_pk(TRIP_ID), plan_sk(1), PLAN_ENTITY, &plan, 4).expect("plan encodes"),
        encode_record(trip_pk(TRIP_ID), day_sk(1, &day), DAY_ENTITY, &day, 7).expect("day encodes"),
    ];
    if let Some(stop) = &stop {
        rows.push(
            encode_record(
                trip_pk(TRIP_ID),
                stop_sk(1, &day, stop),
                STOP_ENTITY,
                stop,
                9,
            )
            .expect("stop encodes"),
        );
    }
    (plan, day, stop, rows)
}

fn candidate() -> Candidate {
    Candidate {
        id: "candidate-a".into(),
        trip_id: TRIP_ID.into(),
        source_place_id: None,
        place_id: "place-a".into(),
        proposed_by: ACTOR_ID.into(),
        created_at: CREATED_AT.into(),
        pitch: "Worth a visit".into(),
        tags: vec!["quiet".into()],
        status: CandidateStatus::Shortlisted,
    }
}

fn poll() -> Poll {
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
        closes_at: "2026-08-07T10:00:00Z".into(),
        decided_at: None,
        quorum: 1,
        allow_multi: false,
        status: PollStatus::Open,
        votes: vec![],
        resolution_note: None,
    }
}

async fn assert_corrupt_plan_anchor(
    rows: Vec<HashMap<String, AttributeValue>>,
    anchor: ThreadAnchor,
) {
    let membership = membership_rule(TripRole::Member);
    let discussion_meta = discussion_meta_rule(None, 1);
    let threads = query_rule(THREAD_PREFIX, vec![]);
    let claims = query_rule(THREAD_ANCHOR_PREFIX, vec![]);
    let trip_meta = trip_meta_rule(9);
    let plan_query = query_rule(format!("{}#", plan_prefix(1)), rows);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &discussion_meta,
            &threads,
            &claims,
            &trip_meta,
            &plan_query
        ]
    );
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .create_thread(TRIP_ID, &actor(), new_thread(anchor))
            .await,
        Err(DiscussionRepoError::CorruptData)
    );
}

#[test]
fn discussion_keys_and_codecs_are_strict_and_trip_partitioned() {
    let trip_anchor = ThreadAnchor::Trip;
    let stop_anchor = ThreadAnchor::Stop {
        stop_id: "stop-a".into(),
    };
    assert_eq!(thread_sk("thread-a"), "THREAD_META#thread-a");
    assert_eq!(comment_prefix("thread-a"), "THREAD#thread-a#COMMENT#");
    assert_eq!(
        comment_sk("thread-a", "comment-a"),
        "THREAD#thread-a#COMMENT#comment-a"
    );
    assert!(
        anchor_claim_sk(&trip_anchor)
            .expect("trip anchor")
            .starts_with(THREAD_ANCHOR_PREFIX)
    );
    assert_ne!(
        anchor_claim_sk(&trip_anchor).expect("trip anchor"),
        anchor_claim_sk(&stop_anchor).expect("stop anchor")
    );
    assert_ne!(DISCUSSION_META_ENTITY, THREAD_ENTITY);
    assert_ne!(THREAD_ANCHOR_ENTITY, COMMENT_ENTITY);

    let value = thread("thread-a", trip_anchor.clone(), 1, CREATED_AT);
    let mut item = encode_thread(&value, CREATED_AT, 3).expect("thread encodes");
    assert_eq!(
        decode_thread(&item, TRIP_ID)
            .expect("thread decodes")
            .revision,
        3
    );
    item.insert(PK.into(), AttributeValue::S(trip_pk("trip-b")));
    assert!(matches!(
        decode_thread(&item, TRIP_ID),
        Err(DiscussionRepoError::CorruptData)
    ));

    let claim = AnchorClaimRecord {
        trip_id: TRIP_ID.into(),
        anchor: stop_anchor,
        thread_id: "thread-a".into(),
    };
    let mut claim_item = encode_anchor_claim(&claim).expect("claim encodes");
    claim_item.insert(SK.into(), AttributeValue::S("THREAD_ANCHOR#forged".into()));
    assert!(matches!(
        decode_anchor_claim(&claim_item, TRIP_ID),
        Err(DiscussionRepoError::CorruptData)
    ));

    let mut malformed = comment("comment-a", "thread-a", ACTOR_ID, CREATED_AT);
    malformed.reactions = vec![Reaction {
        emoji: "👍".into(),
        user_ids: vec![ACTOR_ID.into(), ACTOR_ID.into()],
    }];
    assert!(matches!(
        encode_comment(TRIP_ID, &malformed, 1),
        Err(DiscussionRepoError::CorruptData)
    ));
}

#[tokio::test]
async fn viewers_read_but_cannot_create_comment_or_react() {
    let membership = membership_rule(TripRole::Viewer);
    let meta = discussion_meta_rule(None, 1);
    let threads = query_rule(THREAD_PREFIX, vec![]);
    let claims = query_rule(THREAD_ANCHOR_PREFIX, vec![]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &threads, &claims]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(repo.list_threads(TRIP_ID, &actor()).await, Ok(vec![]));
    assert_eq!(
        repo.create_thread(TRIP_ID, &actor(), new_thread(ThreadAnchor::Trip))
            .await,
        Err(DiscussionRepoError::Forbidden)
    );
    assert_eq!(
        repo.add_comment(TRIP_ID, &actor(), "thread-a", new_comment())
            .await,
        Err(DiscussionRepoError::Forbidden)
    );
    assert_eq!(
        repo.set_reaction(TRIP_ID, &actor(), "thread-a", "comment-a", "👍", true)
            .await,
        Err(DiscussionRepoError::Forbidden)
    );
    assert_eq!(membership.num_calls(), 4);
}

#[tokio::test]
async fn list_threads_validates_claim_bijection_counts_and_order() {
    let membership = membership_rule(TripRole::Viewer);
    let meta = discussion_meta_rule(Some(2), 2);
    let older = thread("thread-a", ThreadAnchor::Trip, 1, CREATED_AT);
    let newer = thread(
        "thread-b",
        ThreadAnchor::Candidate {
            candidate_id: "candidate-a".into(),
        },
        1,
        LATER_AT,
    );
    let thread_items = vec![
        encode_thread(&older, CREATED_AT, 1).expect("older thread"),
        encode_thread(&newer, LATER_AT, 2).expect("newer thread"),
    ];
    let claim_items = vec![
        encode_anchor_claim(&AnchorClaimRecord {
            trip_id: TRIP_ID.into(),
            anchor: older.anchor.clone(),
            thread_id: older.id.clone(),
        })
        .expect("older claim"),
        encode_anchor_claim(&AnchorClaimRecord {
            trip_id: TRIP_ID.into(),
            anchor: newer.anchor.clone(),
            thread_id: newer.id.clone(),
        })
        .expect("newer claim"),
    ];
    let threads = query_rule(THREAD_PREFIX, thread_items);
    let claims = query_rule(THREAD_ANCHOR_PREFIX, claim_items);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &threads, &claims]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let listed = repo
        .list_threads(TRIP_ID, &actor())
        .await
        .expect("threads list");
    assert_eq!(
        listed
            .iter()
            .map(|value| value.id.as_str())
            .collect::<Vec<_>>(),
        ["thread-b", "thread-a"]
    );

    let membership = membership_rule(TripRole::Viewer);
    let meta = discussion_meta_rule(Some(2), 2);
    let threads = query_rule(
        THREAD_PREFIX,
        vec![encode_thread(&older, CREATED_AT, 1).expect("older thread")],
    );
    let claims = query_rule(
        THREAD_ANCHOR_PREFIX,
        vec![
            encode_anchor_claim(&AnchorClaimRecord {
                trip_id: TRIP_ID.into(),
                anchor: older.anchor.clone(),
                thread_id: older.id.clone(),
            })
            .expect("older claim"),
        ],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &threads, &claims]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.list_threads(TRIP_ID, &actor()).await,
        Err(DiscussionRepoError::CorruptData)
    );
    assert_eq!(
        meta.num_calls(),
        2,
        "a cross-read race gets one bounded retry"
    );
}

#[tokio::test]
async fn trip_thread_creation_has_exact_atomic_role_claim_and_record_guards() {
    let membership = membership_rule(TripRole::Member);
    let trip_meta = trip_meta_rule(9);
    let claim = missing_claim_rule(&ThreadAnchor::Trip);
    let discussion_meta = discussion_meta_rule(None, 1);
    let threads = query_rule(THREAD_PREFIX, vec![]);
    let claims = query_rule(THREAD_ANCHOR_PREFIX, vec![]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 6
                && items[0].condition_check().is_some_and(|condition| {
                    condition.table_name() == TABLE
                        && condition.key().get(SK)
                            == Some(&AttributeValue::S(format!("MEMBER#{ACTOR_ID}")))
                        && condition.condition_expression()
                            == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && condition.condition_expression()
                            == "#entity = :entity AND #revision = :revision"
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("9".into()))
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(DISCUSSION_META_SK.into()))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("1".into()))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && decode_discussion_meta(put.item(), TRIP_ID)
                            .is_ok_and(|loaded| loaded.value.thread_count == 1)
                })
                && items[3].put().is_some_and(|put| {
                    put.item()
                        .get(SK)
                        .and_then(|value| value.as_s().ok())
                        .is_some_and(|sk| sk.starts_with(THREAD_ANCHOR_PREFIX))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && decode_anchor_claim(put.item(), TRIP_ID).is_ok_and(|loaded| {
                            loaded.value.anchor == ThreadAnchor::Trip
                                && loaded.value.thread_id == "thread-new"
                        })
                })
                && items[4].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(thread_sk("thread-new")))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && decode_thread(put.item(), TRIP_ID).is_ok_and(|loaded| {
                            loaded.revision == 1
                                && loaded.thread.comment_count == 1
                                && loaded.thread.last_activity_at == CREATED_AT
                        })
                })
                && items[5].put().is_some_and(|put| {
                    put.item().get(SK)
                        == Some(&AttributeValue::S(comment_sk(
                            "thread-new",
                            "comment-first",
                        )))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && decode_comment(put.item(), TRIP_ID, "thread-new").is_ok_and(|loaded| {
                            loaded.revision == 1
                                && loaded.value.author == ACTOR_ID
                                && loaded.value.body == "First thought"
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip_meta,
            &claim,
            &discussion_meta,
            &threads,
            &claims,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let created = repo
        .create_thread(TRIP_ID, &actor(), new_thread(ThreadAnchor::Trip))
        .await
        .expect("thread commits");
    assert_eq!(created.comment_count, 1);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn current_day_anchor_conditions_trip_plan_and_child_revisions_exactly() {
    let membership = membership_rule(TripRole::Member);
    let trip_meta = trip_meta_rule(11);
    let (_, day, _, rows) = plan_rows(false);
    let plan_query = query_rule(format!("{}#", plan_prefix(1)), rows);
    let anchor = ThreadAnchor::Day {
        day_id: day.id.clone(),
    };
    let claim = missing_claim_rule(&anchor);
    let discussion_meta = discussion_meta_rule(None, 1);
    let threads = query_rule(THREAD_PREFIX, vec![]);
    let claims = query_rule(THREAD_ANCHOR_PREFIX, vec![]);
    let expected_day_sk = day_sk(1, &day);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(move |request| {
            let items = request.transact_items();
            items.len() == 8
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
                            == Some(&AttributeValue::N("11".into()))
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":plan_id"))
                            == Some(&AttributeValue::S("plan-a".into()))
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":plan_version"))
                            == Some(&AttributeValue::N("1".into()))
                })
                && items[2].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(plan_sk(1)))
                        && condition.condition_expression()
                            == "#entity = :entity AND #revision = :revision"
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":entity"))
                            == Some(&AttributeValue::S(PLAN_ENTITY.into()))
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("4".into()))
                })
                && items[3].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(expected_day_sk.clone()))
                        && condition.condition_expression()
                            == "#entity = :entity AND #revision = :revision"
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":entity"))
                            == Some(&AttributeValue::S(DAY_ENTITY.into()))
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("7".into()))
                })
                && items[4..]
                    .iter()
                    .all(|item| item.put().is_some())
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip_meta,
            &plan_query,
            &claim,
            &discussion_meta,
            &threads,
            &claims,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert!(
        repo.create_thread(TRIP_ID, &actor(), new_thread(anchor))
            .await
            .is_ok()
    );
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn candidate_poll_and_current_stop_anchors_resolve_only_in_the_trip() {
    let candidate_anchor = ThreadAnchor::Candidate {
        candidate_id: "candidate-a".into(),
    };
    let membership = membership_rule(TripRole::Member);
    let candidate_row = encode_record(
        trip_pk(TRIP_ID),
        candidate_sk("candidate-a"),
        CANDIDATE_ENTITY,
        &candidate(),
        5,
    )
    .expect("candidate encodes");
    let candidate_get = get_rule(candidate_sk("candidate-a"), Some(candidate_row));
    let claim = missing_claim_rule(&candidate_anchor);
    let meta = discussion_meta_rule(None, 1);
    let threads = query_rule(THREAD_PREFIX, vec![]);
    let claims = query_rule(THREAD_ANCHOR_PREFIX, vec![]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &candidate_get,
            &claim,
            &meta,
            &threads,
            &claims,
            &transaction
        ]
    );
    assert!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .create_thread(TRIP_ID, &actor(), new_thread(candidate_anchor))
            .await
            .is_ok()
    );

    let poll_anchor = ThreadAnchor::Poll {
        poll_id: "poll-a".into(),
    };
    let membership = membership_rule(TripRole::Member);
    let poll_row = encode_poll(&poll(), CREATED_AT, 6).expect("poll encodes");
    let poll_get = get_rule(poll_sk("poll-a"), Some(poll_row));
    let claim = missing_claim_rule(&poll_anchor);
    let meta = discussion_meta_rule(None, 1);
    let threads = query_rule(THREAD_PREFIX, vec![]);
    let claims = query_rule(THREAD_ANCHOR_PREFIX, vec![]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &poll_get,
            &claim,
            &meta,
            &threads,
            &claims,
            &transaction
        ]
    );
    assert!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .create_thread(TRIP_ID, &actor(), new_thread(poll_anchor))
            .await
            .is_ok()
    );

    let membership = membership_rule(TripRole::Member);
    let trip_meta = trip_meta_rule(9);
    let (_, _, stop, rows) = plan_rows(true);
    let stop = stop.expect("stop fixture");
    let plan_query = query_rule(format!("{}#", plan_prefix(1)), rows);
    let stop_anchor = ThreadAnchor::Stop {
        stop_id: stop.id.clone(),
    };
    let claim = missing_claim_rule(&stop_anchor);
    let meta = discussion_meta_rule(None, 1);
    let threads = query_rule(THREAD_PREFIX, vec![]);
    let claims = query_rule(THREAD_ANCHOR_PREFIX, vec![]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip_meta,
            &plan_query,
            &claim,
            &meta,
            &threads,
            &claims,
            &transaction
        ]
    );
    assert!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .create_thread(TRIP_ID, &actor(), new_thread(stop_anchor))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn duplicate_or_orphan_anchor_claims_fail_without_writing() {
    let membership = membership_rule(TripRole::Member);
    let trip_meta = trip_meta_rule(9);
    let existing = thread("thread-existing", ThreadAnchor::Trip, 1, CREATED_AT);
    let existing_claim = AnchorClaimRecord {
        trip_id: TRIP_ID.into(),
        anchor: ThreadAnchor::Trip,
        thread_id: existing.id.clone(),
    };
    let meta = discussion_meta_rule(Some(1), 1);
    let threads = query_rule(
        THREAD_PREFIX,
        vec![encode_thread(&existing, CREATED_AT, 1).expect("thread encodes")],
    );
    let claims = query_rule(
        THREAD_ANCHOR_PREFIX,
        vec![encode_anchor_claim(&existing_claim).expect("claim encodes")],
    );
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-existing");
    let thread_get = thread_get_rule(existing, CREATED_AT, 1);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &meta,
            &threads,
            &claims,
            &trip_meta,
            &claim,
            &thread_get
        ]
    );
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .create_thread(TRIP_ID, &actor(), new_thread(ThreadAnchor::Trip))
            .await,
        Err(DiscussionRepoError::Conflict)
    );

    let membership = membership_rule(TripRole::Member);
    let meta = discussion_meta_rule(Some(1), 1);
    let threads = query_rule(THREAD_PREFIX, vec![]);
    let orphan_claim = AnchorClaimRecord {
        trip_id: TRIP_ID.into(),
        anchor: ThreadAnchor::Trip,
        thread_id: "orphan-thread".into(),
    };
    let claims = query_rule(
        THREAD_ANCHOR_PREFIX,
        vec![encode_anchor_claim(&orphan_claim).expect("claim encodes")],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &threads, &claims]
    );
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .create_thread(TRIP_ID, &actor(), new_thread(ThreadAnchor::Trip))
            .await,
        Err(DiscussionRepoError::CorruptData)
    );
    assert_eq!(meta.num_calls(), 2, "corrupt graphs receive one reread");
}

#[tokio::test]
async fn comment_creation_revision_guards_thread_and_server_owned_comment_atomically() {
    let membership = membership_rule(TripRole::Member);
    let original = thread("thread-a", ThreadAnchor::Trip, 1, CREATED_AT);
    let thread_get = thread_get_rule(original.clone(), CREATED_AT, 5);
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-a");
    let existing = comment("comment-old", "thread-a", ACTOR_ID, CREATED_AT);
    let comments = query_rule(
        comment_prefix("thread-a"),
        vec![encode_comment(TRIP_ID, &existing, 1).expect("comment encodes")],
    );
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(thread_sk("thread-a")))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression() == Some("#revision = :revision")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("5".into()))
                        && decode_thread(put.item(), TRIP_ID).is_ok_and(|loaded| {
                            loaded.thread.comment_count == 2
                                && loaded.thread.last_activity_at == LATER_AT
                        })
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(SK)
                        == Some(&AttributeValue::S(comment_sk("thread-a", "comment-new")))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && decode_comment(put.item(), TRIP_ID, "thread-a").is_ok_and(|loaded| {
                            loaded.value.author == ACTOR_ID
                                && loaded.value.created_at == LATER_AT
                                && loaded.value.reactions.is_empty()
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &thread_get, &claim, &comments, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let created = repo
        .add_comment(TRIP_ID, &actor(), "thread-a", new_comment())
        .await
        .expect("comment commits");
    assert_eq!(created.id, "comment-new");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn comment_clock_rollback_and_cross_trip_thread_ids_fail_closed() {
    let membership = membership_rule(TripRole::Member);
    let current = thread("thread-a", ThreadAnchor::Trip, 1, LATER_AT);
    let thread_get = thread_get_rule(current, CREATED_AT, 2);
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-a");
    let latest = comment("comment-current", "thread-a", ACTOR_ID, LATER_AT);
    let comments = query_rule(
        comment_prefix("thread-a"),
        vec![encode_comment(TRIP_ID, &latest, 1).expect("comment encodes")],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &thread_get, &claim, &comments]
    );
    let rollback = NewComment {
        id: "comment-new".into(),
        body: "rollback".into(),
        created_at: CREATED_AT.into(),
    };
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .add_comment(TRIP_ID, &actor(), "thread-a", rollback)
            .await,
        Err(DiscussionRepoError::Conflict)
    );

    let membership = membership_rule(TripRole::Member);
    let missing = get_rule(thread_sk("foreign-thread"), None);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &missing]
    );
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .get_comments(TRIP_ID, &actor(), "foreign-thread")
            .await,
        Err(DiscussionRepoError::NotFound)
    );
    assert_eq!(missing.num_calls(), 1);
}

#[tokio::test]
async fn repeated_comment_id_is_success_only_when_the_whole_thread_is_consistent() {
    let membership = membership_rule(TripRole::Member);
    let initial = thread("thread-a", ThreadAnchor::Trip, 1, CREATED_AT);
    let latest = thread("thread-a", ThreadAnchor::Trip, 2, LATER_AT);
    let initial_item = encode_thread(&initial, CREATED_AT, 5).expect("initial thread");
    let latest_item = encode_thread(&latest, CREATED_AT, 6).expect("latest thread");
    let calls = Arc::new(AtomicUsize::new(0));
    let sequence = calls.clone();
    let thread_get = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(thread_sk("thread-a")))
                })
        })
        .then_output(move || {
            let item = if sequence.fetch_add(1, Ordering::SeqCst) == 0 {
                initial_item.clone()
            } else {
                latest_item.clone()
            };
            GetItemOutput::builder().set_item(Some(item)).build()
        });
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-a");
    let expected = Comment {
        id: "comment-new".into(),
        thread_id: "thread-a".into(),
        author: ACTOR_ID.into(),
        body: "Follow-up".into(),
        created_at: LATER_AT.into(),
        reactions: vec![],
    };
    let old = comment("comment-old", "thread-a", ACTOR_ID, CREATED_AT);
    let old_item = encode_comment(TRIP_ID, &old, 1).expect("old comment");
    let expected_item = encode_comment(TRIP_ID, &expected, 1).expect("expected comment");
    let (comments, comment_queries) = query_sequence_rule(
        comment_prefix("thread-a"),
        vec![vec![old_item.clone()], vec![old_item, expected_item]],
    );
    let transaction =
        mock!(aws_sdk_dynamodb::Client::transact_write_items).then_error(cancelled_transaction);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &thread_get, &claim, &comments, &transaction]
    );
    let returned = DynamoUserRepo::new(client, TABLE)
        .expect("repo")
        .add_comment(TRIP_ID, &actor(), "thread-a", new_comment())
        .await
        .expect("retry classifies committed comment");
    assert_eq!(returned, expected);
    assert_eq!(
        membership.num_calls(),
        2,
        "role is reloaded after contention"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(comment_queries.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn reaction_write_is_desired_state_revision_guarded_and_idempotent() {
    let membership = membership_rule(TripRole::Member);
    let thread_value = thread("thread-a", ThreadAnchor::Trip, 1, CREATED_AT);
    let thread_get = thread_get_rule(thread_value, CREATED_AT, 8);
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-a");
    let current = comment("comment-a", "thread-a", "leader-a", CREATED_AT);
    let comments = query_rule(
        comment_prefix("thread-a"),
        vec![encode_comment(TRIP_ID, &current, 3).expect("comment encodes")],
    );
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(thread_sk("thread-a")))
                        && condition.condition_expression()
                            == "#entity = :entity AND #revision = :revision"
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("8".into()))
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(SK)
                        == Some(&AttributeValue::S(comment_sk("thread-a", "comment-a")))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("4".into()))
                        && put.condition_expression() == Some("#revision = :revision")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("3".into()))
                        && decode_comment(put.item(), TRIP_ID, "thread-a").is_ok_and(|loaded| {
                            loaded.value.reactions
                                == vec![Reaction {
                                    emoji: "👍".into(),
                                    user_ids: vec![ACTOR_ID.into()],
                                }]
                        })
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &thread_get, &claim, &comments, &transaction]
    );
    let updated = DynamoUserRepo::new(client, TABLE)
        .expect("repo")
        .set_reaction(TRIP_ID, &actor(), "thread-a", "comment-a", "👍", true)
        .await
        .expect("reaction commits");
    assert_eq!(updated.reactions[0].user_ids, [ACTOR_ID]);
    assert_eq!(transaction.num_calls(), 1);

    let membership = membership_rule(TripRole::Member);
    let thread_value = thread("thread-a", ThreadAnchor::Trip, 1, CREATED_AT);
    let thread_get = thread_get_rule(thread_value, CREATED_AT, 8);
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-a");
    let mut already = comment("comment-a", "thread-a", "leader-a", CREATED_AT);
    already.reactions.push(Reaction {
        emoji: "👍".into(),
        user_ids: vec![ACTOR_ID.into()],
    });
    let comments = query_rule(
        comment_prefix("thread-a"),
        vec![encode_comment(TRIP_ID, &already, 4).expect("comment encodes")],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &thread_get, &claim, &comments]
    );
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .set_reaction(TRIP_ID, &actor(), "thread-a", "comment-a", "👍", true)
            .await,
        Ok(already),
        "repeating a desired state performs no write"
    );
}

#[tokio::test]
async fn concurrent_reaction_returns_success_only_if_the_desired_state_won() {
    let membership = membership_rule(TripRole::Member);
    let thread = thread("thread-a", ThreadAnchor::Trip, 1, CREATED_AT);
    let thread_get = thread_get_rule(thread, CREATED_AT, 8);
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-a");
    let initial = comment("comment-a", "thread-a", "leader-a", CREATED_AT);
    let mut latest = initial.clone();
    latest.reactions.push(Reaction {
        emoji: "👍".into(),
        user_ids: vec![ACTOR_ID.into()],
    });
    let initial_item = encode_comment(TRIP_ID, &initial, 3).expect("initial comment");
    let latest_item = encode_comment(TRIP_ID, &latest, 4).expect("latest comment");
    let (comments, calls) = query_sequence_rule(
        comment_prefix("thread-a"),
        vec![vec![initial_item], vec![latest_item]],
    );
    let transaction =
        mock!(aws_sdk_dynamodb::Client::transact_write_items).then_error(cancelled_transaction);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &thread_get, &claim, &comments, &transaction]
    );
    let result = DynamoUserRepo::new(client, TABLE)
        .expect("repo")
        .set_reaction(TRIP_ID, &actor(), "thread-a", "comment-a", "👍", true)
        .await
        .expect("concurrent winner reached desired state");
    assert_eq!(result, latest);
    assert_eq!(membership.num_calls(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn malformed_collections_and_storage_or_response_limits_fail_closed() {
    let membership = membership_rule(TripRole::Viewer);
    let thread = thread("thread-a", ThreadAnchor::Trip, 2, LATER_AT);
    let thread_get = thread_get_rule(thread, CREATED_AT, 2);
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-a");
    let only_one = comment("comment-a", "thread-a", ACTOR_ID, CREATED_AT);
    let comments = query_rule(
        comment_prefix("thread-a"),
        vec![encode_comment(TRIP_ID, &only_one, 1).expect("comment")],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &thread_get, &claim, &comments]
    );
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .get_comments(TRIP_ID, &actor(), "thread-a")
            .await,
        Err(DiscussionRepoError::CorruptData)
    );
    assert_eq!(comments.num_calls(), 2, "count races receive one retry");

    let mut oversized = Vec::new();
    for index in 0..430 {
        let value = Comment {
            id: format!("comment-{index:03}"),
            thread_id: "thread-a".into(),
            author: ACTOR_ID.into(),
            body: "x".repeat(10_000),
            created_at: CREATED_AT.into(),
            reactions: vec![],
        };
        oversized.push(encode_comment(TRIP_ID, &value, 1).expect("large comment"));
    }
    let query = query_rule(comment_prefix("thread-a"), oversized);
    let client = mock_client!(aws_sdk_dynamodb, RuleMode::MatchAny, [&query]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.discussion_query(&trip_pk(TRIP_ID), &comment_prefix("thread-a"), 1_000,)
            .await,
        Err(DiscussionRepoError::SafetyLimitExceeded)
    );
    assert_eq!(MAX_DISCUSSION_BYTES, 4 * 1024 * 1024);
}

#[tokio::test]
async fn normal_mutations_reject_missing_and_orphan_comment_rows() {
    let membership = membership_rule(TripRole::Member);
    let value = thread("thread-a", ThreadAnchor::Trip, 2, LATER_AT);
    let thread_get = thread_get_rule(value, CREATED_AT, 2);
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-a");
    let only_one = comment("comment-old", "thread-a", ACTOR_ID, CREATED_AT);
    let comments = query_rule(
        comment_prefix("thread-a"),
        vec![encode_comment(TRIP_ID, &only_one, 1).expect("comment encodes")],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &thread_get, &claim, &comments]
    );
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .add_comment(TRIP_ID, &actor(), "thread-a", new_comment())
            .await,
        Err(DiscussionRepoError::CorruptData)
    );
    assert_eq!(comments.num_calls(), 2, "corrupt counts receive one reread");

    let membership = membership_rule(TripRole::Member);
    let value = thread("thread-a", ThreadAnchor::Trip, 1, CREATED_AT);
    let thread_get = thread_get_rule(value, CREATED_AT, 2);
    let claim = claim_get_rule(ThreadAnchor::Trip, "thread-a");
    let valid = comment("comment-valid", "thread-a", ACTOR_ID, CREATED_AT);
    let orphan = comment("comment-orphan", "thread-a", ACTOR_ID, CREATED_AT);
    let comments = query_rule(
        comment_prefix("thread-a"),
        vec![
            encode_comment(TRIP_ID, &valid, 1).expect("comment encodes"),
            encode_comment(TRIP_ID, &orphan, 1).expect("comment encodes"),
        ],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &thread_get, &claim, &comments]
    );
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .set_reaction(
                TRIP_ID,
                &actor(),
                "thread-a",
                "comment-orphan",
                "馃憤",
                true,
            )
            .await,
        Err(DiscussionRepoError::CorruptData)
    );
    assert_eq!(comments.num_calls(), 2, "orphan rows receive one reread");
}

#[tokio::test]
async fn malformed_plan_day_and_stop_payloads_cannot_authorize_anchors() {
    let (mut plan, day, _, mut rows) = plan_rows(false);
    plan.created_at = "2026-08-01T01:00:00+01:00".into();
    rows[0] =
        encode_record(trip_pk(TRIP_ID), plan_sk(1), PLAN_ENTITY, &plan, 4).expect("plan encodes");
    assert_corrupt_plan_anchor(
        rows,
        ThreadAnchor::Day {
            day_id: day.id.clone(),
        },
    )
    .await;

    let (_, mut day, _, mut rows) = plan_rows(false);
    day.window_end = "08:00".into();
    rows[1] =
        encode_record(trip_pk(TRIP_ID), day_sk(1, &day), DAY_ENTITY, &day, 7).expect("day encodes");
    assert_corrupt_plan_anchor(
        rows,
        ThreadAnchor::Day {
            day_id: day.id.clone(),
        },
    )
    .await;
    let (_, day, stop, mut rows) = plan_rows(true);
    let mut stop = stop.expect("stop fixture");
    stop.planned_arrival = "9:00".into();
    rows[2] = encode_record(
        trip_pk(TRIP_ID),
        stop_sk(1, &day, &stop),
        STOP_ENTITY,
        &stop,
        9,
    )
    .expect("stop encodes");
    assert_corrupt_plan_anchor(
        rows,
        ThreadAnchor::Stop {
            stop_id: stop.id.clone(),
        },
    )
    .await;
}

#[tokio::test]
async fn unknown_or_duplicate_current_plan_rows_are_corrupt_not_anchors() {
    let membership = membership_rule(TripRole::Member);
    let discussion_meta = discussion_meta_rule(None, 1);
    let threads = query_rule(THREAD_PREFIX, vec![]);
    let claims = query_rule(THREAD_ANCHOR_PREFIX, vec![]);
    let trip_meta = trip_meta_rule(9);
    let (_, day, _, mut rows) = plan_rows(false);
    rows.push(
        encode_record(
            trip_pk(TRIP_ID),
            format!("{}#UNKNOWN", plan_prefix(1)),
            "UNKNOWN",
            &serde_json::json!({"id": "forged"}),
            1,
        )
        .expect("unknown row encodes"),
    );
    let plan_query = query_rule(format!("{}#", plan_prefix(1)), rows);
    let anchor = ThreadAnchor::Day {
        day_id: day.id.clone(),
    };
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &discussion_meta,
            &threads,
            &claims,
            &trip_meta,
            &plan_query
        ]
    );
    assert_eq!(
        DynamoUserRepo::new(client, TABLE)
            .expect("repo")
            .create_thread(TRIP_ID, &actor(), new_thread(anchor))
            .await,
        Err(DiscussionRepoError::CorruptData)
    );
}
