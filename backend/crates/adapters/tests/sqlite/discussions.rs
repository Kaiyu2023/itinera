use std::{sync::Arc, time::Duration};

use itinera_core::{
    domain::{
        discussion::{Comment, ThreadAnchor},
        trip::{Candidate, CandidateStatus, Day, Place, PlaceKind, Plan, TripRole},
        user::User,
    },
    ports::{
        authorization::TripAuthorizationContext,
        discussion::{DiscussionRepo, DiscussionRepoError, NewComment, NewThread},
        trip::TripRepo,
    },
};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Sqlite, Transaction};

use super::support::{NOW, TestDatabase, raw_connection, seed_trip, seed_user};

const DISCUSSION_TIME: &str = "2026-08-07T13:00:00Z";
const CANONICAL_DISCUSSION_TIME: &str = "2026-08-07T13:00:00.000000000Z";
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

fn human(user: &User) -> TripAuthorizationContext {
    TripAuthorizationContext::human(user.id.clone())
}

fn place() -> Place {
    Place {
        id: "anchor-place".into(),
        name: "Anchor Place".into(),
        kind: PlaceKind::Sight,
        lat: 0.0,
        lng: 0.0,
        tz: "UTC".into(),
        country_code: String::new(),
        admin_area: String::new(),
        city: "London".into(),
        address: "1 Test Street".into(),
        external_ref: None,
        website: None,
        phone: None,
        rating: None,
        price_level: None,
        opening_hours: None,
        photo_urls: Vec::new(),
        guide: None,
    }
}

fn candidate(leader: &User) -> Candidate {
    Candidate {
        id: "candidate-a".into(),
        trip_id: "trip-a".into(),
        source_place_id: None,
        place_id: "anchor-place".into(),
        proposed_by: leader.id.0.clone(),
        created_at: NOW.into(),
        pitch: "Worth discussing".into(),
        tags: Vec::new(),
        status: CandidateStatus::Shortlisted,
    }
}

fn plan() -> Plan {
    Plan {
        id: "plan-1".into(),
        trip_id: "trip-a".into(),
        version: 1,
        created_from_proposal_id: None,
        created_at: NOW.into(),
    }
}

fn days() -> Vec<Day> {
    ["2026-08-07", "2026-08-08", "2026-08-09"]
        .into_iter()
        .enumerate()
        .map(|(index, date)| Day {
            id: format!("day-{}", index + 1),
            plan_id: "plan-1".into(),
            date: date.into(),
            city_hint: "London".into(),
            tz: "UTC".into(),
            window_start: "09:00".into(),
            window_end: "21:00".into(),
        })
        .collect()
}

async fn add_membership(database: &TestDatabase, user: &User, role: TripRole) {
    sqlx::query(
        "INSERT INTO trip_memberships (trip_id, user_id, role, joined_at, revision) \
         VALUES ('trip-a', ?, ?, ?, 1)",
    )
    .bind(&user.id.0)
    .bind(role.as_ref())
    .bind(NOW)
    .execute(database.db.pool())
    .await
    .expect("add membership");
}

async fn seed_graph(database: &TestDatabase) -> (User, User, User, User) {
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    let viewer = seed_user(&users, "viewer", "viewer@example.com").await;
    let outsider = seed_user(&users, "outsider", "outsider@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(database, &member, TripRole::Member).await;
    add_membership(database, &viewer, TripRole::Viewer).await;
    let anchor = place();
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate(&leader),
            anchor.clone(),
        )
        .await
        .expect("seed candidate");
    trips
        .initialize_plan("trip-a", &human(&leader), &anchor.id, plan(), days())
        .await
        .expect("seed plan");
    (leader, member, viewer, outsider)
}

fn new_thread(id: &str, anchor: ThreadAnchor) -> NewThread {
    NewThread {
        id: id.into(),
        first_comment_id: format!("{id}-comment-1"),
        anchor,
        title: format!("Thread {id}"),
        body: "First thought".into(),
        created_at: DISCUSSION_TIME.into(),
    }
}

fn padded_comment_projection(thread_id: &str, first_id: &str) -> Vec<Comment> {
    let mut comments = vec![Comment {
        id: first_id.into(),
        thread_id: thread_id.into(),
        author: "leader".into(),
        body: "x".into(),
        created_at: CANONICAL_DISCUSSION_TIME.into(),
        reactions: Vec::new(),
    }];
    comments.extend((2..=499).map(|index| Comment {
        id: format!("comment-{index:04}"),
        thread_id: thread_id.into(),
        author: "leader".into(),
        body: "x".into(),
        created_at: CANONICAL_DISCUSSION_TIME.into(),
        reactions: Vec::new(),
    }));
    comments.push(Comment {
        id: "z-final".into(),
        thread_id: thread_id.into(),
        author: "leader".into(),
        body: "x".into(),
        created_at: CANONICAL_DISCUSSION_TIME.into(),
        reactions: Vec::new(),
    });
    comments.sort_by(|left, right| left.id.cmp(&right.id));
    let base = serde_json::to_vec(&comments)
        .expect("encode boundary comments")
        .len();
    let mut remaining = MAX_RESPONSE_BYTES - base;
    for comment in comments
        .iter_mut()
        .filter(|comment| comment.id.starts_with("comment-"))
    {
        let available = 10_000 - comment.body.chars().count();
        let addition = remaining.min(available);
        comment.body.push_str(&"x".repeat(addition));
        remaining -= addition;
        if remaining == 0 {
            break;
        }
    }
    assert_eq!(remaining, 0, "comment text limits can reach 4 MiB exactly");
    assert_eq!(
        serde_json::to_vec(&comments)
            .expect("encode exact boundary comments")
            .len(),
        MAX_RESPONSE_BYTES
    );
    comments
}

async fn seed_boundary_comments(database: &TestDatabase, thread_id: &str, comments: &[Comment]) {
    let mut transaction = database
        .db
        .pool()
        .begin()
        .await
        .expect("boundary transaction");
    for comment in comments
        .iter()
        .filter(|comment| comment.id.starts_with("comment-"))
    {
        sqlx::query(
            "INSERT INTO discussion_comments ( \
                 trip_id, thread_id, id, author_id, body, created_at, revision \
             ) VALUES ('trip-a', ?, ?, 'leader', ?, ?, 1)",
        )
        .bind(thread_id)
        .bind(&comment.id)
        .bind(&comment.body)
        .bind(CANONICAL_DISCUSSION_TIME)
        .execute(&mut *transaction)
        .await
        .expect("seed boundary comment");
    }
    transaction
        .commit()
        .await
        .expect("commit boundary comments");
}

async fn insert_seed_thread(
    transaction: &mut Transaction<'_, Sqlite>,
    thread_id: &str,
    anchor_kind: &str,
    anchor_id: Option<&str>,
    anchor_key: &str,
) {
    sqlx::query(
        "INSERT INTO discussion_threads ( \
             trip_id, id, anchor_kind, anchor_id, anchor_key, title, created_at, \
             last_activity_at, revision \
         ) VALUES ('trip-a', ?, ?, ?, ?, 'Boundary thread', ?, ?, 1)",
    )
    .bind(thread_id)
    .bind(anchor_kind)
    .bind(anchor_id)
    .bind(anchor_key)
    .bind(CANONICAL_DISCUSSION_TIME)
    .bind(CANONICAL_DISCUSSION_TIME)
    .execute(&mut **transaction)
    .await
    .expect("insert capacity thread");
    sqlx::query(
        "INSERT INTO discussion_comments ( \
             trip_id, thread_id, id, author_id, body, created_at, revision \
         ) VALUES ('trip-a', ?, ?, 'leader', 'seed', ?, 1)",
    )
    .bind(thread_id)
    .bind(format!("{thread_id}-comment"))
    .bind(CANONICAL_DISCUSSION_TIME)
    .execute(&mut **transaction)
    .await
    .expect("insert capacity first comment");
}

async fn seed_thread_capacity(database: &TestDatabase) {
    let mut transaction = database
        .db
        .pool()
        .begin()
        .await
        .expect("thread capacity transaction");
    for index in 0..999 {
        let place_id = format!("boundary-place-{index:04}");
        let candidate_id = format!("boundary-candidate-{index:04}");
        sqlx::query(
            "INSERT INTO trip_places ( \
                 trip_id, id, name, kind, lat, lng, tz, country_code, admin_area, \
                 city, address, external_ref_json, website, phone, rating, \
                 price_level, opening_hours_json, photo_urls_json, guide_json, revision \
             ) VALUES ( \
                 'trip-a', ?, ?, 'sight', 0.0, 0.0, 'UTC', '', '', 'London', '', \
                 NULL, NULL, NULL, NULL, NULL, NULL, '[]', NULL, 1 \
             )",
        )
        .bind(&place_id)
        .bind(format!("Boundary place {index}"))
        .execute(&mut *transaction)
        .await
        .expect("insert capacity place");
        sqlx::query(
            "INSERT INTO candidates ( \
                 trip_id, id, place_id, source_catalog_place_id, source_trip_place_id, \
                 proposed_by, created_at, pitch, tags_json, status, revision \
             ) VALUES ('trip-a', ?, ?, NULL, NULL, 'leader', ?, 'Boundary candidate', \
                       '[]', 'shortlisted', 1)",
        )
        .bind(&candidate_id)
        .bind(&place_id)
        .bind(NOW)
        .execute(&mut *transaction)
        .await
        .expect("insert capacity candidate");
    }

    insert_seed_thread(
        &mut transaction,
        "boundary-trip-thread",
        "trip",
        None,
        "trip",
    )
    .await;
    insert_seed_thread(
        &mut transaction,
        "boundary-candidate-a-thread",
        "candidate",
        Some("candidate-a"),
        "candidate:candidate-a",
    )
    .await;
    for index in 0..997 {
        let candidate_id = format!("boundary-candidate-{index:04}");
        insert_seed_thread(
            &mut transaction,
            &format!("boundary-thread-{index:04}"),
            "candidate",
            Some(&candidate_id),
            &format!("candidate:{candidate_id}"),
        )
        .await;
    }
    transaction
        .commit()
        .await
        .expect("commit thread capacity fixtures");
}

#[tokio::test]
async fn discussions_persist_ordered_comments_and_idempotent_caller_owned_reactions() {
    let database = TestDatabase::new().await;
    let (leader, member, viewer, _) = seed_graph(&database).await;
    let discussions = database.discussions();

    let thread = discussions
        .create_thread(
            "trip-a",
            &human(&leader),
            new_thread("thread-a", ThreadAnchor::Trip),
        )
        .await
        .expect("create thread and first comment");
    assert_eq!(thread.comment_count, 1);
    assert_eq!(thread.last_activity_at, CANONICAL_DISCUSSION_TIME);
    assert_eq!(
        discussions
            .list_threads("trip-a", &human(&viewer))
            .await
            .expect("viewer reads threads"),
        vec![thread]
    );

    let second = discussions
        .add_comment(
            "trip-a",
            &human(&member),
            "thread-a",
            NewComment {
                id: "comment-2".into(),
                body: "Follow-up".into(),
                created_at: "2026-08-07T13:01:00+00:00".into(),
            },
        )
        .await
        .expect("member adds comment");
    assert_eq!(second.created_at, "2026-08-07T13:01:00.000000000Z");
    let reacted = discussions
        .set_reaction(
            "trip-a",
            &human(&leader),
            "thread-a",
            "comment-2",
            "👍",
            true,
        )
        .await
        .expect("leader reacts");
    assert_eq!(reacted.reactions[0].user_ids, vec!["leader"]);
    assert_eq!(
        discussions
            .set_reaction(
                "trip-a",
                &human(&leader),
                "thread-a",
                "comment-2",
                "👍",
                true,
            )
            .await
            .expect("same desired state is idempotent"),
        reacted
    );
    let member_reacted = discussions
        .set_reaction(
            "trip-a",
            &human(&member),
            "thread-a",
            "comment-2",
            "👍",
            true,
        )
        .await
        .expect("member reacts");
    assert_eq!(
        member_reacted.reactions[0].user_ids,
        vec!["leader", "member"]
    );
    let removed = discussions
        .set_reaction(
            "trip-a",
            &human(&leader),
            "thread-a",
            "comment-2",
            "👍",
            false,
        )
        .await
        .expect("leader removes own reaction");
    assert_eq!(removed.reactions[0].user_ids, vec!["member"]);

    let comments = discussions
        .get_comments("trip-a", &human(&viewer), "thread-a")
        .await
        .expect("viewer reads comments");
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].id, "thread-a-comment-1");
    assert_eq!(comments[1], removed);

    drop(discussions);
    database.shutdown().await;
}

#[tokio::test]
async fn discussion_authorization_is_non_disclosing_and_services_remain_fail_closed() {
    let database = TestDatabase::new().await;
    let (leader, _, viewer, outsider) = seed_graph(&database).await;
    let discussions = database.discussions();
    let service = TripAuthorizationContext::service(leader.id.clone(), "service-a".into());

    assert_eq!(
        discussions.list_threads("trip-a", &human(&outsider)).await,
        Err(DiscussionRepoError::NotFound)
    );
    assert_eq!(
        discussions.list_threads("trip-a", &service).await,
        Err(DiscussionRepoError::Forbidden)
    );
    assert_eq!(
        discussions
            .create_thread(
                "trip-a",
                &human(&viewer),
                new_thread("viewer-thread", ThreadAnchor::Trip),
            )
            .await,
        Err(DiscussionRepoError::Forbidden)
    );
    assert_eq!(
        discussions
            .create_thread(
                "trip-a",
                &human(&leader),
                new_thread(
                    "foreign-anchor",
                    ThreadAnchor::Candidate {
                        candidate_id: "missing-candidate".into(),
                    },
                ),
            )
            .await,
        Err(DiscussionRepoError::NotFound)
    );
    assert_eq!(
        discussions
            .create_thread(
                "trip-a",
                &human(&leader),
                new_thread(
                    "stale-day",
                    ThreadAnchor::Day {
                        day_id: "missing-day".into(),
                    },
                ),
            )
            .await,
        Err(DiscussionRepoError::NotFound)
    );

    drop(discussions);
    database.shutdown().await;
}

#[tokio::test]
async fn thread_creation_is_atomic_and_enforces_one_thread_per_anchor() {
    let database = TestDatabase::new().await;
    let (leader, _, _, _) = seed_graph(&database).await;
    let discussions = database.discussions();
    discussions
        .create_thread(
            "trip-a",
            &human(&leader),
            new_thread(
                "candidate-thread",
                ThreadAnchor::Candidate {
                    candidate_id: "candidate-a".into(),
                },
            ),
        )
        .await
        .expect("create candidate thread");
    assert_eq!(
        discussions
            .create_thread(
                "trip-a",
                &human(&leader),
                new_thread(
                    "duplicate-anchor",
                    ThreadAnchor::Candidate {
                        candidate_id: "candidate-a".into(),
                    },
                ),
            )
            .await,
        Err(DiscussionRepoError::Conflict)
    );

    sqlx::query(
        "CREATE TRIGGER fail_first_discussion_comment \
         BEFORE INSERT ON discussion_comments \
         WHEN NEW.thread_id = 'rollback-thread' \
         BEGIN SELECT RAISE(ABORT, 'injected discussion failure'); END",
    )
    .execute(database.db.pool())
    .await
    .expect("install rollback trigger");
    assert_eq!(
        discussions
            .create_thread(
                "trip-a",
                &human(&leader),
                new_thread("rollback-thread", ThreadAnchor::Trip),
            )
            .await,
        Err(DiscussionRepoError::Unavailable)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discussion_threads WHERE trip_id = 'trip-a' AND id = 'rollback-thread'",
        )
        .fetch_one(database.db.pool())
        .await
        .expect("count rolled back thread"),
        0
    );

    drop(discussions);
    database.shutdown().await;
}

#[tokio::test]
async fn concurrent_final_thread_slot_accepts_exactly_one_thousand() {
    let database = TestDatabase::new().await;
    let (leader, member, viewer, _) = seed_graph(&database).await;
    seed_thread_capacity(&database).await;
    let repository = Arc::new(database.discussions());

    let left = {
        let repository = Arc::clone(&repository);
        let actor = human(&leader);
        tokio::spawn(async move {
            repository
                .create_thread(
                    "trip-a",
                    &actor,
                    new_thread(
                        "boundary-final-left",
                        ThreadAnchor::Candidate {
                            candidate_id: "boundary-candidate-0997".into(),
                        },
                    ),
                )
                .await
        })
    };
    let right = {
        let repository = Arc::clone(&repository);
        let actor = human(&member);
        tokio::spawn(async move {
            repository
                .create_thread(
                    "trip-a",
                    &actor,
                    new_thread(
                        "boundary-final-right",
                        ThreadAnchor::Candidate {
                            candidate_id: "boundary-candidate-0998".into(),
                        },
                    ),
                )
                .await
        })
    };
    let results = [
        left.await.expect("left thread task"),
        right.await.expect("right thread task"),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(DiscussionRepoError::SafetyLimitExceeded)))
            .count(),
        1
    );
    let stored = repository
        .list_threads("trip-a", &human(&viewer))
        .await
        .expect("read exact thread boundary");
    assert_eq!(stored.len(), 1_000);
    assert!(serde_json::to_vec(&stored).expect("encode threads").len() <= MAX_RESPONSE_BYTES);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discussion_threads WHERE trip_id = 'trip-a'",
        )
        .fetch_one(database.db.pool())
        .await
        .expect("count exact thread boundary"),
        1_000
    );

    drop(repository);
    database.shutdown().await;
}

#[tokio::test]
async fn discussion_ids_and_foreign_keys_remain_trip_scoped() {
    let database = TestDatabase::new().await;
    let (leader, _, _, _) = seed_graph(&database).await;
    let trips = database.trips();
    seed_trip(&trips, "trip-b", &leader).await;
    let mut trip_b_place = place();
    trip_b_place.id = "trip-b-anchor-place".into();
    let trip_b_candidate = Candidate {
        id: "trip-b-candidate".into(),
        trip_id: "trip-b".into(),
        source_place_id: None,
        place_id: trip_b_place.id.clone(),
        proposed_by: leader.id.0.clone(),
        created_at: NOW.into(),
        pitch: "Trip B anchor".into(),
        tags: Vec::new(),
        status: CandidateStatus::Shortlisted,
    };
    trips
        .add_candidate("trip-b", &human(&leader), trip_b_candidate, trip_b_place)
        .await
        .expect("seed trip B candidate");
    let discussions = database.discussions();
    for (trip_id, title, body, first_comment_id) in [
        ("trip-a", "Trip A thread", "Trip A body", "shared-comment-a"),
        ("trip-b", "Trip B thread", "Trip B body", "shared-comment-b"),
    ] {
        discussions
            .create_thread(
                trip_id,
                &human(&leader),
                NewThread {
                    id: "shared-thread".into(),
                    first_comment_id: first_comment_id.into(),
                    anchor: ThreadAnchor::Trip,
                    title: title.into(),
                    body: body.into(),
                    created_at: DISCUSSION_TIME.into(),
                },
            )
            .await
            .expect("same thread id is valid in a different trip");
    }
    discussions
        .create_thread(
            "trip-b",
            &human(&leader),
            new_thread(
                "trip-b-only-thread",
                ThreadAnchor::Candidate {
                    candidate_id: "trip-b-candidate".into(),
                },
            ),
        )
        .await
        .expect("create trip B only thread");

    assert_eq!(
        discussions
            .get_comments("trip-a", &human(&leader), "shared-thread")
            .await
            .expect("read trip A shared id")[0]
            .body,
        "Trip A body"
    );
    assert_eq!(
        discussions
            .get_comments("trip-b", &human(&leader), "shared-thread")
            .await
            .expect("read trip B shared id")[0]
            .body,
        "Trip B body"
    );
    assert!(
        sqlx::query(
            "INSERT INTO discussion_comments ( \
                 trip_id, thread_id, id, author_id, body, created_at, revision \
             ) VALUES ('trip-a', 'trip-b-only-thread', 'wrong-trip-comment', \
                       'leader', 'wrong trip', ?, 1)",
        )
        .bind(CANONICAL_DISCUSSION_TIME)
        .execute(database.db.pool())
        .await
        .is_err(),
        "composite thread foreign key accepted a cross-trip parent"
    );
    assert!(
        sqlx::query(
            "INSERT INTO comment_reactions ( \
                 trip_id, thread_id, comment_id, emoji, user_id \
             ) VALUES ('trip-a', 'shared-thread', 'shared-comment-b', 'x', 'leader')",
        )
        .execute(database.db.pool())
        .await
        .is_err(),
        "composite comment foreign key accepted a cross-trip parent"
    );

    drop(discussions);
    drop(trips);
    database.shutdown().await;
}

#[tokio::test]
async fn concurrent_final_comment_slot_has_one_winner_without_partial_state() {
    let database = TestDatabase::new().await;
    let (leader, member, _, _) = seed_graph(&database).await;
    let discussions = database.discussions();
    discussions
        .create_thread(
            "trip-a",
            &human(&leader),
            new_thread("thread-a", ThreadAnchor::Trip),
        )
        .await
        .expect("create thread");
    let mut transaction = database.db.pool().begin().await.expect("seed transaction");
    for index in 2..=999 {
        sqlx::query(
            "INSERT INTO discussion_comments ( \
                 trip_id, thread_id, id, author_id, body, created_at, revision \
             ) VALUES ('trip-a', 'thread-a', ?, 'leader', 'seed', ?, 1)",
        )
        .bind(format!("seed-{index:04}"))
        .bind(CANONICAL_DISCUSSION_TIME)
        .execute(&mut *transaction)
        .await
        .expect("seed comment");
    }
    transaction.commit().await.expect("commit seeded comments");

    let repo = Arc::new(discussions);
    let left = {
        let repo = Arc::clone(&repo);
        let actor = human(&leader);
        tokio::spawn(async move {
            repo.add_comment(
                "trip-a",
                &actor,
                "thread-a",
                NewComment {
                    id: "final-left".into(),
                    body: "left".into(),
                    created_at: DISCUSSION_TIME.into(),
                },
            )
            .await
        })
    };
    let right = {
        let repo = Arc::clone(&repo);
        let actor = human(&member);
        tokio::spawn(async move {
            repo.add_comment(
                "trip-a",
                &actor,
                "thread-a",
                NewComment {
                    id: "final-right".into(),
                    body: "right".into(),
                    created_at: DISCUSSION_TIME.into(),
                },
            )
            .await
        })
    };
    let results = [
        left.await.expect("left task"),
        right.await.expect("right task"),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(DiscussionRepoError::SafetyLimitExceeded)))
            .count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discussion_comments WHERE trip_id = 'trip-a' AND thread_id = 'thread-a'",
        )
        .fetch_one(database.db.pool())
        .await
        .expect("count comments"),
        1_000
    );

    drop(repo);
    database.shutdown().await;
}

#[tokio::test]
async fn reaction_kind_and_user_limits_accept_one_thousand_and_reject_one_more() {
    let database = TestDatabase::new().await;
    let (leader, member, _, _) = seed_graph(&database).await;
    let discussions = database.discussions();
    discussions
        .create_thread(
            "trip-a",
            &human(&leader),
            new_thread("reaction-kinds", ThreadAnchor::Trip),
        )
        .await
        .expect("create reaction-kind thread");
    discussions
        .create_thread(
            "trip-a",
            &human(&leader),
            new_thread(
                "reaction-users",
                ThreadAnchor::Candidate {
                    candidate_id: "candidate-a".into(),
                },
            ),
        )
        .await
        .expect("create reaction-user thread");

    let mut transaction = database
        .db
        .pool()
        .begin()
        .await
        .expect("reaction boundary transaction");
    for index in 0..999 {
        sqlx::query(
            "INSERT INTO comment_reactions ( \
                 trip_id, thread_id, comment_id, emoji, user_id \
             ) VALUES ('trip-a', 'reaction-kinds', 'reaction-kinds-comment-1', \
                       ?, 'leader')",
        )
        .bind(format!("kind-{index:04}"))
        .execute(&mut *transaction)
        .await
        .expect("seed reaction kind");

        let user_id = format!("reaction-user-{index:04}");
        let email = format!("reaction-{index:04}@example.com");
        sqlx::query("INSERT INTO users (id, email, display_name, revision) VALUES (?, ?, NULL, 1)")
            .bind(&user_id)
            .bind(&email)
            .execute(&mut *transaction)
            .await
            .expect("seed reaction user");
        sqlx::query("INSERT INTO user_email_claims (email_digest, user_id) VALUES (?, ?)")
            .bind(format!("{:x}", Sha256::digest(email.as_bytes())))
            .bind(&user_id)
            .execute(&mut *transaction)
            .await
            .expect("seed reciprocal reaction-user claim");
        sqlx::query(
            "INSERT INTO comment_reactions ( \
                 trip_id, thread_id, comment_id, emoji, user_id \
             ) VALUES ('trip-a', 'reaction-users', 'reaction-users-comment-1', \
                       'users', ?)",
        )
        .bind(&user_id)
        .execute(&mut *transaction)
        .await
        .expect("seed reaction user binding");
    }
    transaction
        .commit()
        .await
        .expect("commit reaction boundary fixtures");

    let exact_kinds = discussions
        .set_reaction(
            "trip-a",
            &human(&leader),
            "reaction-kinds",
            "reaction-kinds-comment-1",
            "kind-0999",
            true,
        )
        .await
        .expect("accept the thousandth reaction kind");
    assert_eq!(exact_kinds.reactions.len(), 1_000);
    assert_eq!(
        discussions
            .set_reaction(
                "trip-a",
                &human(&member),
                "reaction-kinds",
                "reaction-kinds-comment-1",
                "kind-over",
                true,
            )
            .await,
        Err(DiscussionRepoError::SafetyLimitExceeded)
    );

    let exact_users = discussions
        .set_reaction(
            "trip-a",
            &human(&leader),
            "reaction-users",
            "reaction-users-comment-1",
            "users",
            true,
        )
        .await
        .expect("accept the thousandth reaction user");
    assert_eq!(exact_users.reactions.len(), 1);
    assert_eq!(exact_users.reactions[0].user_ids.len(), 1_000);
    assert_eq!(
        discussions
            .set_reaction(
                "trip-a",
                &human(&member),
                "reaction-users",
                "reaction-users-comment-1",
                "users",
                true,
            )
            .await,
        Err(DiscussionRepoError::SafetyLimitExceeded)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM comment_reactions \
             WHERE trip_id = 'trip-a' AND thread_id = 'reaction-kinds'",
        )
        .fetch_one(database.db.pool())
        .await
        .expect("count exact reaction kinds"),
        1_000
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM comment_reactions \
             WHERE trip_id = 'trip-a' AND thread_id = 'reaction-users' \
               AND emoji = 'users'",
        )
        .fetch_one(database.db.pool())
        .await
        .expect("count exact reaction users"),
        1_000
    );

    drop(discussions);
    database.shutdown().await;
}

#[tokio::test]
async fn comment_writer_accepts_four_mebibytes_exactly_and_rejects_one_byte_more() {
    let database = TestDatabase::new().await;
    let (leader, _, viewer, _) = seed_graph(&database).await;
    let discussions = database.discussions();

    for (thread_id, anchor, over) in [
        ("exact-thread", ThreadAnchor::Trip, false),
        (
            "over-thread",
            ThreadAnchor::Candidate {
                candidate_id: "candidate-a".into(),
            },
            true,
        ),
    ] {
        let first_id = format!("{thread_id}-comment-1");
        discussions
            .create_thread(
                "trip-a",
                &human(&leader),
                NewThread {
                    id: thread_id.into(),
                    first_comment_id: first_id.clone(),
                    anchor,
                    title: thread_id.into(),
                    body: "x".into(),
                    created_at: DISCUSSION_TIME.into(),
                },
            )
            .await
            .expect("create boundary thread");
        let projected = padded_comment_projection(thread_id, &first_id);
        seed_boundary_comments(&database, thread_id, &projected).await;
        let mut final_body = projected
            .iter()
            .find(|comment| comment.id == "z-final")
            .expect("final boundary comment")
            .body
            .clone();
        if over {
            final_body.push('x');
        }
        let result = discussions
            .add_comment(
                "trip-a",
                &human(&leader),
                thread_id,
                NewComment {
                    id: "z-final".into(),
                    body: final_body,
                    created_at: DISCUSSION_TIME.into(),
                },
            )
            .await;
        if over {
            assert_eq!(result, Err(DiscussionRepoError::SafetyLimitExceeded));
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM discussion_comments \
                     WHERE trip_id = 'trip-a' AND thread_id = ? AND id = 'z-final'",
                )
                .bind(thread_id)
                .fetch_one(database.db.pool())
                .await
                .expect("count rejected final comment"),
                0
            );
        } else {
            result.expect("exact four MiB projection succeeds");
            let stored = discussions
                .get_comments("trip-a", &human(&viewer), thread_id)
                .await
                .expect("read exact boundary");
            assert_eq!(
                serde_json::to_vec(&stored)
                    .expect("encode stored boundary")
                    .len(),
                MAX_RESPONSE_BYTES
            );
            assert_eq!(
                discussions
                    .set_reaction("trip-a", &human(&leader), thread_id, "z-final", "x", true,)
                    .await,
                Err(DiscussionRepoError::SafetyLimitExceeded)
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM comment_reactions \
                     WHERE trip_id = 'trip-a' AND thread_id = ?",
                )
                .bind(thread_id)
                .fetch_one(database.db.pool())
                .await
                .expect("count rejected boundary reaction"),
                0
            );
        }
    }

    drop(discussions);
    database.shutdown().await;
}

#[tokio::test]
async fn discussion_reads_fail_closed_on_activity_anchor_and_response_corruption() {
    let database = TestDatabase::new().await;
    let (leader, _, viewer, _) = seed_graph(&database).await;
    let discussions = database.discussions();
    discussions
        .create_thread(
            "trip-a",
            &human(&leader),
            new_thread("thread-a", ThreadAnchor::Trip),
        )
        .await
        .expect("create thread");

    sqlx::query(
        "UPDATE discussion_threads SET last_activity_at = '2026-08-07T14:00:00.000000000Z' \
         WHERE trip_id = 'trip-a' AND id = 'thread-a'",
    )
    .execute(database.db.pool())
    .await
    .expect("corrupt activity");
    assert_eq!(
        discussions.list_threads("trip-a", &human(&viewer)).await,
        Err(DiscussionRepoError::CorruptData)
    );
    sqlx::query(
        "UPDATE discussion_threads SET last_activity_at = ? \
         WHERE trip_id = 'trip-a' AND id = 'thread-a'",
    )
    .bind(CANONICAL_DISCUSSION_TIME)
    .execute(database.db.pool())
    .await
    .expect("restore activity");

    let body = "x".repeat(10_000);
    let mut transaction = database
        .db
        .pool()
        .begin()
        .await
        .expect("corruption transaction");
    for index in 2..=500 {
        sqlx::query(
            "INSERT INTO discussion_comments ( \
                 trip_id, thread_id, id, author_id, body, created_at, revision \
             ) VALUES ('trip-a', 'thread-a', ?, 'leader', ?, ?, 1)",
        )
        .bind(format!("large-{index:04}"))
        .bind(&body)
        .bind(CANONICAL_DISCUSSION_TIME)
        .execute(&mut *transaction)
        .await
        .expect("insert oversized raw collection");
    }
    transaction
        .commit()
        .await
        .expect("commit corrupt collection");
    assert_eq!(
        discussions
            .get_comments("trip-a", &human(&viewer), "thread-a")
            .await,
        Err(DiscussionRepoError::CorruptData)
    );

    drop(discussions);
    database.shutdown().await;
}

#[tokio::test]
async fn discussion_writers_recheck_roles_after_reserving_the_sqlite_writer() {
    let database = TestDatabase::new().await;
    let (leader, member, _, _) = seed_graph(&database).await;
    let discussions = database.discussions();
    discussions
        .create_thread(
            "trip-a",
            &human(&leader),
            new_thread("thread-a", ThreadAnchor::Trip),
        )
        .await
        .expect("create thread");

    let mut blocker = raw_connection(&database.path).await;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut blocker)
        .await
        .expect("reserve writer");
    sqlx::query(
        "UPDATE trip_memberships SET role = 'viewer', revision = revision + 1 \
         WHERE trip_id = 'trip-a' AND user_id = 'member'",
    )
    .execute(&mut blocker)
    .await
    .expect("stage role revocation");
    let repository = discussions.clone();
    let actor = member.clone();
    let task = tokio::spawn(async move {
        repository
            .add_comment(
                "trip-a",
                &human(&actor),
                "thread-a",
                NewComment {
                    id: "revoked-comment".into(),
                    body: "must not commit".into(),
                    created_at: DISCUSSION_TIME.into(),
                },
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    sqlx::query("COMMIT")
        .execute(&mut blocker)
        .await
        .expect("commit role revocation");
    blocker.close().await.expect("close blocker");
    assert_eq!(
        task.await.expect("writer task"),
        Err(DiscussionRepoError::Forbidden)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discussion_comments \
             WHERE trip_id = 'trip-a' AND id = 'revoked-comment'",
        )
        .fetch_one(database.db.pool())
        .await
        .expect("count revoked write"),
        0
    );

    drop(discussions);
    database.shutdown().await;
}

#[tokio::test]
async fn discussion_schema_domain_codecs_and_revision_edges_fail_closed() {
    let database = TestDatabase::new().await;
    let (leader, _, viewer, _) = seed_graph(&database).await;
    let discussions = database.discussions();
    discussions
        .create_thread(
            "trip-a",
            &human(&leader),
            new_thread("thread-a", ThreadAnchor::Trip),
        )
        .await
        .expect("create thread");

    for statement in [
        "INSERT INTO discussion_threads (trip_id, id, anchor_kind, anchor_id, anchor_key, title, created_at, last_activity_at, revision) VALUES ('trip-a', 'bad-anchor', 'candidate', 'candidate-a', 'candidate:other', 'Bad', '2026-08-07T13:00:00.000000000Z', '2026-08-07T13:00:00.000000000Z', 1)",
        "INSERT INTO discussion_comments (trip_id, thread_id, id, author_id, body, created_at, revision) VALUES ('trip-a', 'thread-a', 'bad-time', 'leader', 'Bad', '2026-08-07T13:00:00+00:00', 1)",
        "INSERT INTO discussion_comments (trip_id, thread_id, id, author_id, body, created_at, revision) VALUES ('trip-a', 'missing-thread', 'orphan', 'leader', 'Bad', '2026-08-07T13:00:00.000000000Z', 1)",
        "INSERT INTO comment_reactions (trip_id, thread_id, comment_id, emoji, user_id) VALUES ('trip-a', 'thread-a', 'thread-a-comment-1', '👍', 'missing-user')",
    ] {
        assert!(
            sqlx::query(statement)
                .execute(database.db.pool())
                .await
                .is_err(),
            "strict discussion schema unexpectedly accepted {statement}"
        );
    }

    sqlx::query(
        "INSERT INTO comment_reactions (trip_id, thread_id, comment_id, emoji, user_id) \
         VALUES ('trip-a', 'thread-a', 'thread-a-comment-1', 'two words', 'leader')",
    )
    .execute(database.db.pool())
    .await
    .expect("schema admits shape for domain corruption test");
    assert_eq!(
        discussions
            .get_comments("trip-a", &human(&viewer), "thread-a")
            .await,
        Err(DiscussionRepoError::CorruptData)
    );
    sqlx::query("DELETE FROM comment_reactions WHERE trip_id = 'trip-a'")
        .execute(database.db.pool())
        .await
        .expect("remove injected domain corruption");

    sqlx::query(
        "UPDATE discussion_threads SET revision = 9223372036854775807 \
         WHERE trip_id = 'trip-a' AND id = 'thread-a'",
    )
    .execute(database.db.pool())
    .await
    .expect("exhaust thread revision");
    assert_eq!(
        discussions
            .add_comment(
                "trip-a",
                &human(&leader),
                "thread-a",
                NewComment {
                    id: "overflow-comment".into(),
                    body: "must roll back".into(),
                    created_at: DISCUSSION_TIME.into(),
                },
            )
            .await,
        Err(DiscussionRepoError::CorruptData)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discussion_comments WHERE id = 'overflow-comment'",
        )
        .fetch_one(database.db.pool())
        .await
        .expect("count rolled-back overflow comment"),
        0
    );
    sqlx::query(
        "UPDATE discussion_threads SET revision = 1 WHERE trip_id = 'trip-a' AND id = 'thread-a';",
    )
    .execute(database.db.pool())
    .await
    .expect("restore thread revision");
    sqlx::query(
        "UPDATE discussion_comments SET revision = 9223372036854775807 \
         WHERE trip_id = 'trip-a' AND thread_id = 'thread-a' AND id = 'thread-a-comment-1'",
    )
    .execute(database.db.pool())
    .await
    .expect("exhaust comment revision");
    assert_eq!(
        discussions
            .set_reaction(
                "trip-a",
                &human(&leader),
                "thread-a",
                "thread-a-comment-1",
                "👍",
                true,
            )
            .await,
        Err(DiscussionRepoError::CorruptData)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM comment_reactions WHERE trip_id = 'trip-a'",
        )
        .fetch_one(database.db.pool())
        .await
        .expect("count rolled-back overflow reaction"),
        0
    );

    drop(discussions);
    database.shutdown().await;
}
