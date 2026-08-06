use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use aws_sdk_dynamodb::operation::{
    get_item::GetItemOutput, query::QueryOutput, transact_write_items::TransactWriteItemsOutput,
};
use aws_sdk_dynamodb::types::{CancellationReason, error::TransactionCanceledException};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use itinera_core::domain::trip::PlaceKind;

use super::*;

const TABLE: &str = "itinera-test";

fn leader() -> TripMember {
    TripMember {
        user_id: "u-leader".into(),
        role: TripRole::Leader,
        joined_at: "2026-08-01T00:00:00Z".into(),
    }
}

fn trip() -> Trip {
    Trip {
        id: "trip-a".into(),
        name: "Japan".into(),
        cover_photo_url: None,
        accent_color: None,
        stop_kind_labels: None,
        status: TripStatus::Dreaming,
        start_date: "2026-11-01".into(),
        end_date: "2026-11-03".into(),
        base_currency: "GBP".into(),
        soft_budget: None,
        members: vec![leader()],
        current_plan_id: None,
        created_at: "2026-08-01T00:00:00Z".into(),
    }
}

fn pending_invite() -> Invite {
    Invite {
        id: "invite-new".into(),
        trip_id: "trip-a".into(),
        email: "friend@example.test".into(),
        invited_by: "u-leader".into(),
        status: InviteStatus::Pending,
        created_at: "2026-08-05T00:00:00Z".into(),
    }
}

fn cancelled_transaction(codes: &[&str]) -> TransactWriteItemsError {
    let mut builder = TransactionCanceledException::builder();
    for code in codes {
        builder = builder.cancellation_reasons(CancellationReason::builder().code(*code).build());
    }
    TransactWriteItemsError::TransactionCanceledException(builder.build())
}

#[test]
fn trip_owned_keys_include_the_authoritative_trip_partition() {
    assert_eq!(trip_pk("trip-a"), "TRIP#trip-a");
    assert_eq!(candidate_sk("candidate-a"), "CANDIDATE#candidate-a");
    assert_eq!(plan_sk(7), "PLAN#0000000007#META");
}

#[test]
fn invite_lookup_keys_do_not_disclose_the_email() {
    let email = Email::parse("cloud.strife@proton.me").expect("valid email");
    let key = invitee_pk(&email);
    assert!(key.starts_with("INVITEE#"));
    assert!(!key.contains("cloud"));
    assert!(!key.contains('@'));
}

#[test]
fn records_validate_key_type_schema_and_json() {
    let member = TripMember {
        user_id: "u-1".into(),
        role: TripRole::Leader,
        joined_at: "2026-08-01T00:00:00Z".into(),
    };
    let item = encode_member("trip-a", &member).expect("encode");
    let decoded: Stored<TripMember> =
        decode_record(&item, "TRIP#trip-a", "MEMBER#u-1", MEMBER_ENTITY).expect("decode");
    assert_eq!(decoded.value, member);
    assert!(
        decode_record::<TripMember>(&item, "TRIP#trip-b", "MEMBER#u-1", MEMBER_ENTITY).is_err()
    );
}

#[test]
fn only_conditional_cancellations_are_domain_conflicts() {
    let conditional = cancelled_transaction(&["None", CONDITIONAL_FAILURE, "None"]);
    let transaction_conflict = cancelled_transaction(&["None", "TransactionConflict"]);
    let throttled = cancelled_transaction(&["ThrottlingError"]);

    assert!(transaction_condition_failed(Some(&conditional)));
    assert!(!transaction_condition_failed(Some(&transaction_conflict)));
    assert!(!transaction_condition_failed(Some(&throttled)));
    assert!(!transaction_condition_failed(None));
}

#[tokio::test]
async fn partition_queries_follow_continuation_keys() {
    let cursor = HashMap::from([
        (PK.to_string(), AttributeValue::S("INVITEE#hash".into())),
        (SK.to_string(), AttributeValue::S("TRIP#trip-a".into())),
    ]);
    let first_cursor = cursor.clone();
    let first_rule = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| request.exclusive_start_key().is_none())
        .then_output(move || {
            QueryOutput::builder()
                .items(HashMap::from([(
                    SK.to_string(),
                    AttributeValue::S("TRIP#trip-a".into()),
                )]))
                .set_last_evaluated_key(Some(first_cursor.clone()))
                .build()
        });
    let second_cursor = cursor.clone();
    let second_rule = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| request.exclusive_start_key() == Some(&second_cursor))
        .then_output(|| {
            QueryOutput::builder()
                .items(HashMap::from([(
                    SK.to_string(),
                    AttributeValue::S("TRIP#trip-b".into()),
                )]))
                .build()
        });
    let client = mock_client!(aws_sdk_dynamodb, [&first_rule, &second_rule]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let items = repo
        .query_partition("INVITEE#hash", "TRIP#", 1)
        .await
        .expect("all pages");

    assert_eq!(items.len(), 2);
    assert_eq!(first_rule.num_calls(), 1);
    assert_eq!(second_rule.num_calls(), 1);
}

#[tokio::test]
async fn an_accepted_invite_can_be_renewed() {
    let member_item = encode_member("trip-a", &leader()).expect("member item");
    let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
            })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(member_item.clone()))
                .build()
        });
    let mut accepted = pending_invite();
    accepted.id = "invite-old".into();
    accepted.status = InviteStatus::Accepted;
    let email = Email::parse(&accepted.email).expect("email");
    let accepted_item = encode_record(
        trip_pk("trip-a"),
        invite_sk(&email),
        INVITE_ENTITY,
        &accepted,
        4,
    )
    .expect("accepted invite item");
    let invite_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK)
                    == Some(&AttributeValue::S(invite_sk(
                        &Email::parse("friend@example.test").expect("email"),
                    )))
            })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(accepted_item.clone()))
                .build()
        });
    let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[1].put().is_some_and(|put| {
                    put.item().get(REVISION) == Some(&AttributeValue::N("5".into()))
                        && put.condition_expression() == Some("#revision = :expected_revision")
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(ENTITY_TYPE)
                        == Some(&AttributeValue::S(INVITE_LOOKUP_ENTITY.into()))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        [&member_rule, &invite_rule, &transaction_rule]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let renewed = repo
        .create_invite("trip-a", &UserId("u-leader".into()), pending_invite())
        .await
        .expect("renewed invite");

    assert_eq!(renewed.id, "invite-new");
    assert_eq!(renewed.status, InviteStatus::Pending);
    assert_eq!(transaction_rule.num_calls(), 1);
}

#[tokio::test]
async fn concurrent_invite_acceptance_is_idempotent() {
    let email = Email::parse("friend@example.test").expect("email");
    let lookup_pk = invitee_pk(&email);
    let lookup_sk = invite_lookup_sk("trip-a");
    let invite_sk = invite_sk(&email);
    let lookup = InviteLookup {
        trip_id: "trip-a".into(),
        invite_sort_key: invite_sk.clone(),
    };
    let lookup_item = encode_record(
        lookup_pk.clone(),
        lookup_sk.clone(),
        INVITE_LOOKUP_ENTITY,
        &lookup,
        1,
    )
    .expect("lookup item");
    let query_rule = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request
                .expression_attribute_values()
                .and_then(|values| values.get(":pk"))
                == Some(&AttributeValue::S(lookup_pk.clone()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![lookup_item.clone()]))
                .build()
        });
    let lookup_get_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request
                .key()
                .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(lookup_sk.clone())))
        })
        // The other request has already deleted the lookup.
        .then_output(|| GetItemOutput::builder().build());
    let mut accepted = pending_invite();
    accepted.status = InviteStatus::Accepted;
    let accepted_item = encode_record(
        trip_pk("trip-a"),
        invite_sk.clone(),
        INVITE_ENTITY,
        &accepted,
        2,
    )
    .expect("accepted invite");
    let invite_get_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request
                .key()
                .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(invite_sk.clone())))
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(accepted_item.clone()))
                .build()
        });
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&query_rule, &lookup_get_rule, &invite_get_rule]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let user = User {
        id: UserId("u-friend".into()),
        email,
        display_name: None,
    };

    repo.accept_pending_invites(&user, "2026-08-05T00:00:00Z")
        .await
        .expect("concurrent acceptance is success");

    assert_eq!(query_rule.num_calls(), 1);
    assert_eq!(lookup_get_rule.num_calls(), 1);
    assert_eq!(invite_get_rule.num_calls(), 1);
}

#[tokio::test]
async fn plan_initialization_conditions_the_shortlisted_candidate_revision() {
    let member_item = encode_member("trip-a", &leader()).expect("member item");
    let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
            })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(member_item.clone()))
                .build()
        });
    let meta_item = encode_trip_meta(&TripMeta::from_trip(&trip()), 1).expect("meta item");
    let meta_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request
                .key()
                .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(META_SK.into())))
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(meta_item.clone()))
                .build()
        });
    let place = Place {
        id: "place-a".into(),
        name: "Kyoto".into(),
        kind: PlaceKind::Sight,
        lat: 35.0,
        lng: 135.0,
        tz: "Asia/Tokyo".into(),
        country_code: "JP".into(),
        admin_area: "Kyoto".into(),
        city: "Kyoto".into(),
        address: "Kyoto".into(),
        external_ref: None,
        website: None,
        phone: None,
        rating: None,
        price_level: None,
        opening_hours: None,
        photo_urls: vec![],
        guide: None,
    };
    let place_item = encode_record(
        trip_pk("trip-a"),
        place_sk("place-a"),
        PLACE_ENTITY,
        &place,
        1,
    )
    .expect("place item");
    let place_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request
                .key()
                .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S("PLACE#place-a".into())))
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(place_item.clone()))
                .build()
        });
    let candidate = Candidate {
        id: "candidate-a".into(),
        trip_id: "trip-a".into(),
        source_place_id: None,
        place_id: "place-a".into(),
        proposed_by: "u-leader".into(),
        created_at: "2026-08-05T00:00:00Z".into(),
        pitch: "Anchor".into(),
        tags: vec![],
        status: CandidateStatus::Shortlisted,
    };
    let candidate_item = encode_record(
        trip_pk("trip-a"),
        candidate_sk("candidate-a"),
        CANDIDATE_ENTITY,
        &candidate,
        7,
    )
    .expect("candidate item");
    let candidate_rule = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request
                .expression_attribute_values()
                .and_then(|values| values.get(":prefix"))
                == Some(&AttributeValue::S("CANDIDATE#".into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![candidate_item.clone()]))
                .build()
        });
    let plan = Plan {
        id: "plan-a".into(),
        trip_id: "trip-a".into(),
        version: 1,
        created_from_proposal_id: None,
        created_at: "2026-08-05T00:00:00Z".into(),
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
    let plan_item =
        encode_record(trip_pk("trip-a"), plan_sk(1), PLAN_ENTITY, &plan, 1).expect("plan item");
    let day_item =
        encode_record(trip_pk("trip-a"), day_sk(1, &day), DAY_ENTITY, &day, 1).expect("day item");
    let detail_rule = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request
                .expression_attribute_values()
                .and_then(|values| values.get(":prefix"))
                == Some(&AttributeValue::S("PLAN#0000000001#".into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![plan_item.clone(), day_item.clone()]))
                .build()
        });
    let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 5
                && items[1].condition_check().is_some_and(|condition| {
                    condition.key().get(SK)
                        == Some(&AttributeValue::S("CANDIDATE#candidate-a".into()))
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("7".into()))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &member_rule,
            &meta_rule,
            &place_rule,
            &candidate_rule,
            &detail_rule,
            &transaction_rule
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let initialized = repo
        .initialize_plan(
            "trip-a",
            &UserId("u-leader".into()),
            "place-a",
            plan,
            vec![day],
        )
        .await
        .expect("initialized plan");

    assert_eq!(initialized.plan.id, "plan-a");
    assert_eq!(transaction_rule.num_calls(), 1);
}

#[tokio::test]
async fn day_edits_are_conditioned_on_the_current_plan_revision() {
    let member_item = encode_member("trip-a", &leader()).expect("member item");
    let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
            })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(member_item.clone()))
                .build()
        });
    let mut meta = TripMeta::from_trip(&trip());
    meta.current_plan_id = Some("plan-a".into());
    meta.current_plan_version = Some(1);
    meta.cities = vec!["Kyoto".into()];
    let meta_item = encode_trip_meta(&meta, 7).expect("meta item");
    let meta_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request
                .key()
                .is_some_and(|key| key.get(SK) == Some(&AttributeValue::S(META_SK.into())))
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(meta_item.clone()))
                .build()
        });
    let day = Day {
        id: "day-a".into(),
        plan_id: "plan-a".into(),
        date: "2026-11-01".into(),
        city_hint: "Kyoto".into(),
        tz: "Asia/Tokyo".into(),
        window_start: "09:00".into(),
        window_end: "21:00".into(),
    };
    let day_item =
        encode_record(trip_pk("trip-a"), day_sk(1, &day), DAY_ENTITY, &day, 3).expect("day item");
    let day_rule = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request
                .expression_attribute_values()
                .and_then(|values| values.get(":prefix"))
                == Some(&AttributeValue::S("PLAN#0000000001#DAY#".into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![day_item.clone()]))
                .build()
        });
    let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 4
                && items[2].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("7".into()))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member_rule, &meta_rule, &day_rule, &transaction_rule]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let updated = repo
        .update_day(
            "trip-a",
            &UserId("u-leader".into()),
            "day-a",
            DayPatch {
                window_start: Some("10:00".into()),
                window_end: None,
                city_hint: None,
            },
            "2026-08-05T00:00:00Z",
            "change-a",
        )
        .await
        .expect("day update");

    assert_eq!(updated.window_start, "10:00");
    assert_eq!(transaction_rule.num_calls(), 1);
}

#[tokio::test]
async fn create_trip_atomically_writes_meta_membership_and_reverse_count() {
    let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].put().is_some_and(|put| {
                    put.item().get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                        && put.item().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && put.item().get(LEADER_COUNT) == Some(&AttributeValue::N("1".into()))
                })
                && items[1].put().is_some_and(|put| {
                    put.item().get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                        && put.item().get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                        && put.item().get(GSI1PK)
                            == Some(&AttributeValue::S("USER#u-leader".into()))
                        && put.item().get(ROLE) == Some(&AttributeValue::S("leader".into()))
                })
                && items[2].update().is_some_and(|update| {
                    update.key().get(PK) == Some(&AttributeValue::S("USER#u-leader".into()))
                        && update.key().get(SK) == Some(&AttributeValue::S(USER_PROFILE_SK.into()))
                        && update.update_expression().contains("if_not_exists")
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(aws_sdk_dynamodb, [&transaction_rule]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let created = repo.create_trip(trip()).await.expect("create trip");

    assert_eq!(created.id, "trip-a");
    assert_eq!(transaction_rule.num_calls(), 1);
}

#[tokio::test]
async fn get_trip_authorizes_with_a_strong_direct_read_before_loading_data() {
    let member_item = encode_member("trip-a", &leader()).expect("member item");
    let member_for_get = member_item.clone();
    let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                        && key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(member_for_get.clone()))
                .build()
        });
    let meta = TripMeta::from_trip(&trip());
    let meta_item = encode_trip_meta(&meta, 1).expect("meta item");
    let meta_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                        && key.get(SK) == Some(&AttributeValue::S(META_SK.into()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(meta_item.clone()))
                .build()
        });
    let members_rule = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.index_name().is_none()
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":pk"))
                    == Some(&AttributeValue::S("TRIP#trip-a".into()))
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("MEMBER#".into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![member_item.clone()]))
                .build()
        });
    let client = mock_client!(aws_sdk_dynamodb, [&member_rule, &meta_rule, &members_rule]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let loaded = repo
        .get_trip("trip-a", &UserId("u-leader".into()))
        .await
        .expect("authorized trip");

    assert_eq!(loaded, trip());
    assert_eq!(member_rule.num_calls(), 1);
    assert_eq!(meta_rule.num_calls(), 1);
    assert_eq!(members_rule.num_calls(), 1);
}

#[tokio::test]
async fn an_absent_direct_membership_stops_a_cross_trip_read() {
    let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| request.consistent_read() == Some(true))
        .then_output(|| GetItemOutput::builder().build());
    let client = mock_client!(aws_sdk_dynamodb, [&member_rule]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let error = repo
        .get_trip("trip-a", &UserId("u-stranger".into()))
        .await
        .expect_err("non-member must not load trip metadata");

    assert_eq!(error, TripRepoError::NotFound);
    assert_eq!(member_rule.num_calls(), 1);
}

#[tokio::test]
async fn status_change_rechecks_editor_role_inside_the_atomic_write() {
    let member_item = encode_member("trip-a", &leader()).expect("member item");
    let member_for_get = member_item.clone();
    let member_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                        && key.get(SK) == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(member_for_get.clone()))
                .build()
        });

    let old_meta = TripMeta::from_trip(&trip());
    let mut new_meta = old_meta.clone();
    new_meta.status = TripStatus::Booked;
    let old_meta_item = encode_trip_meta(&old_meta, 1).expect("old meta item");
    let new_meta_item = encode_trip_meta(&new_meta, 2).expect("new meta item");
    let meta_reads = Arc::new(AtomicUsize::new(0));
    let reads = Arc::clone(&meta_reads);
    let meta_rule = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                        && key.get(SK) == Some(&AttributeValue::S(META_SK.into()))
                })
        })
        .then_output(move || {
            let item = if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                old_meta_item.clone()
            } else {
                new_meta_item.clone()
            };
            GetItemOutput::builder().set_item(Some(item)).build()
        });

    let members_rule = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.index_name().is_none()
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("MEMBER#".into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![member_item.clone()]))
                .build()
        });

    let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].condition_check().is_some_and(|condition| {
                    condition.key().get(PK) == Some(&AttributeValue::S("TRIP#trip-a".into()))
                        && condition.key().get(SK)
                            == Some(&AttributeValue::S("MEMBER#u-leader".into()))
                        && condition.condition_expression().contains("#role")
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());

    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member_rule, &meta_rule, &members_rule, &transaction_rule]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let updated = repo
        .set_trip_status(
            "trip-a",
            &UserId("u-leader".into()),
            TripStatus::Booked,
            "2026-08-05T00:00:00Z",
            "change-a",
        )
        .await
        .expect("status update");

    assert_eq!(updated.status, TripStatus::Booked);
    assert_eq!(member_rule.num_calls(), 2);
    assert_eq!(meta_rule.num_calls(), 2);
    assert_eq!(transaction_rule.num_calls(), 1);
}
