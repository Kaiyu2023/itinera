use std::{sync::Arc, time::Duration};

use itinera_core::{
    domain::{
        trip::{
            DateRange, Invite, NewInviteInput, PendingInvite, Trip, TripData, TripMember, TripRole,
            TripStatus, TripSummary,
        },
        user::{Email, User, UserId},
    },
    ports::{
        authorization::TripAuthorizationContext,
        trip::{TripRepo, TripRepoError},
    },
};
use sqlx::{Connection, Sqlite, Transaction};
use tokio::sync::Barrier;

use super::support::{NOW, TestDatabase, digest, raw_connection, seed_trip, seed_user, trip, user};

fn human(user: &User) -> TripAuthorizationContext {
    TripAuthorizationContext::human(user.id.clone())
}

fn invite(id: &str, trip_id: &str, actor: &User, email: &str) -> PendingInvite {
    Invite::create(NewInviteInput {
        id: id.to_string(),
        trip_id: trip_id.to_string(),
        email: Email::parse(email).unwrap(),
        invited_by: actor.id.0.clone(),
        created_at: NOW.to_string(),
    })
    .expect("valid fixture invite")
}

fn trip_with_members(id: &str, members: Vec<TripMember>) -> Trip {
    Trip::try_from(TripData {
        id: id.to_string(),
        name: format!("Trip {id}"),
        cover_photo_url: None,
        accent_color: None,
        stop_kind_labels: None,
        status: TripStatus::Planning,
        dates: DateRange::try_new("2026-08-07".into(), "2026-08-09".into())
            .expect("valid fixture dates"),
        base_currency: "GBP".parse().expect("valid fixture currency"),
        soft_budget: None,
        members,
        current_plan_id: None,
        created_at: NOW.to_string(),
    })
    .expect("valid fixture trip")
}

#[tokio::test]
async fn trip_creation_persists_all_initial_members_without_order_semantics() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let member = seed_user(&users, "a-member", "member@example.com").await;
    let leader = seed_user(&users, "z-leader", "leader@example.com").await;
    let expected = trip_with_members(
        "trip-a",
        vec![
            TripMember::try_new(member.id.0.clone(), TripRole::Member, NOW.into()).unwrap(),
            TripMember::try_new(leader.id.0.clone(), TripRole::Leader, NOW.into()).unwrap(),
        ],
    );

    assert_eq!(
        trips
            .create_trip(&human(&leader), expected.clone())
            .await
            .unwrap(),
        expected,
        "creation accepts a complete valid aggregate, not a one-member type state"
    );
    let stored = trips.get_trip("trip-a", &human(&leader)).await.unwrap();
    assert_eq!(stored.status(), TripStatus::Planning);
    assert_eq!(stored.members().len(), 2);
    assert_eq!(stored.members()[0].user_id(), member.id.0);
    assert_eq!(stored.members()[0].role(), TripRole::Member);
    assert_eq!(stored.members()[1].user_id(), leader.id.0);
    assert_eq!(stored.members()[1].role(), TripRole::Leader);
    assert!(trips.get_trip("trip-a", &human(&member)).await.is_ok());

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn trip_creation_rejects_unsupported_plan_and_missing_profile_without_writes() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;

    let mut planned = trip("planned-trip", &leader);
    planned
        .set_current_plan_id(Some("plan-a".into()))
        .expect("valid domain plan id");
    assert!(matches!(
        trips.create_trip(&human(&leader), planned).await,
        Err(TripRepoError::Unavailable)
    ));

    let missing_profile = trip_with_members(
        "missing-profile-trip",
        vec![
            TripMember::try_new(leader.id.0.clone(), TripRole::Leader, NOW.into()).unwrap(),
            TripMember::try_new("missing-user".into(), TripRole::Member, NOW.into()).unwrap(),
        ],
    );
    assert!(matches!(
        trips.create_trip(&human(&leader), missing_profile).await,
        Err(TripRepoError::CorruptData)
    ));

    for trip_id in ["planned-trip", "missing-profile-trip"] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trips WHERE id = ?")
                .bind(trip_id)
                .fetch_one(database.db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM trip_memberships WHERE trip_id = ?",
            )
            .bind(trip_id)
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
            0
        );
    }

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn creation_and_invite_acceptance_require_the_matching_human_context() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let invitee = seed_user(&users, "invitee", "invitee@example.com").await;
    let expected = trip("trip-a", &leader);

    assert!(matches!(
        trips
            .create_trip(
                &TripAuthorizationContext::service(leader.id.clone(), "service-create".into(),),
                expected.clone(),
            )
            .await,
        Err(TripRepoError::Forbidden)
    ));
    assert!(matches!(
        trips.create_trip(&human(&invitee), expected.clone()).await,
        Err(TripRepoError::Forbidden)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trips WHERE id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0,
        "rejected creation contexts must not leave partial rows"
    );

    trips
        .create_trip(&human(&leader), expected)
        .await
        .expect("matching leader context creates the trip");
    trips
        .create_invite(
            "trip-a",
            &human(&leader),
            invite("invite-1", "trip-a", &leader, invitee.email.as_str()),
        )
        .await
        .expect("create pending invite");

    assert!(matches!(
        trips
            .accept_pending_invites(
                &TripAuthorizationContext::service(invitee.id.clone(), "service-accept".into(),),
                &invitee,
                NOW,
            )
            .await,
        Err(TripRepoError::Forbidden)
    ));
    assert!(matches!(
        trips
            .accept_pending_invites(&human(&leader), &invitee, NOW)
            .await,
        Err(TripRepoError::Forbidden)
    ));
    assert_invite_state(&database, "trip-a", invitee.email.as_str(), "pending", 1).await;
    assert_eq!(membership_count(&database, "trip-a", &invitee.id).await, 0);

    trips
        .accept_pending_invites(&human(&invitee), &invitee, NOW)
        .await
        .expect("matching invitee context accepts the invite");
    assert_invite_state(&database, "trip-a", invitee.email.as_str(), "accepted", 2).await;
    assert_eq!(membership_count(&database, "trip-a", &invitee.id).await, 1);

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn sqlite_trip_repository_contract_enforces_roles_isolation_and_same_snapshot_profiles() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    let viewer = seed_user(&users, "viewer", "viewer@example.com").await;
    let outsider = seed_user(&users, "outsider", "outsider@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    seed_trip(&trips, "trip-b", &outsider).await;
    add_membership(&database, "trip-a", &member, TripRole::Member).await;
    add_membership(&database, "trip-a", &viewer, TripRole::Viewer).await;
    add_membership(&database, "trip-b", &member, TripRole::Member).await;

    let stored = trips.get_trip("trip-a", &human(&leader)).await.unwrap();
    assert_eq!(stored.members().len(), 3);
    assert_eq!(trips.list_trips(&human(&member)).await.unwrap().len(), 2);
    assert!(matches!(
        trips.get_trip("trip-a", &human(&outsider)).await,
        Err(TripRepoError::NotFound)
    ));

    let profiles = trips
        .get_members("trip-a", &human(&viewer))
        .await
        .expect("viewer may read profiles from the same SQLite snapshot");
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.id.0.as_str())
            .collect::<Vec<_>>(),
        ["leader", "member", "viewer"]
    );

    assert!(matches!(
        trips
            .create_invite(
                "trip-a",
                &human(&member),
                invite("member-invite", "trip-a", &member, "new@example.com"),
            )
            .await,
        Err(TripRepoError::Forbidden)
    ));
    assert!(matches!(
        trips
            .remove_member("trip-a", &human(&viewer), &member.id)
            .await,
        Err(TripRepoError::Forbidden)
    ));
    assert!(matches!(
        trips
            .remove_member("trip-a", &human(&leader), &outsider.id)
            .await,
        Err(TripRepoError::NotFound)
    ));
    assert_eq!(
        trips
            .get_trip("trip-b", &human(&outsider))
            .await
            .unwrap()
            .members()
            .len(),
        2,
        "a foreign-trip user ID must not authorize or delete a child row"
    );

    trips
        .remove_member("trip-a", &human(&leader), &member.id)
        .await
        .expect("leader removes member");
    assert!(
        trips
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .members()
            .iter()
            .all(|membership| membership.user_id() != member.id.0)
    );
    assert!(matches!(
        trips
            .remove_member("trip-a", &human(&leader), &leader.id)
            .await,
        Err(TripRepoError::Conflict)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn sqlite_service_context_is_not_reduced_to_its_owner_before_service_storage_exists() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(&database, "trip-a", &member, TripRole::Member).await;
    let authorization = TripAuthorizationContext::service(leader.id.clone(), "service-a".into());

    assert!(matches!(
        trips.list_trips(&authorization).await,
        Err(TripRepoError::Forbidden)
    ));
    assert!(matches!(
        trips.get_trip("trip-a", &authorization).await,
        Err(TripRepoError::Forbidden)
    ));
    assert!(matches!(
        trips.get_members("trip-a", &authorization).await,
        Err(TripRepoError::Forbidden)
    ));
    assert!(matches!(
        trips
            .create_invite(
                "trip-a",
                &authorization,
                invite("service-invite", "trip-a", &leader, "new@example.com"),
            )
            .await,
        Err(TripRepoError::Forbidden)
    ));
    assert!(matches!(
        trips
            .remove_member("trip-a", &authorization, &member.id)
            .await,
        Err(TripRepoError::Forbidden)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trip_invites")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(membership_count(&database, "trip-a", &member.id).await, 1);

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn malformed_requested_ids_are_not_disclosed_as_persistence_corruption() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    for (index, malformed) in [String::new(), " ".to_string(), "x".repeat(201)]
        .into_iter()
        .enumerate()
    {
        assert!(matches!(
            trips.get_trip(&malformed, &human(&leader)).await,
            Err(TripRepoError::NotFound)
        ));
        assert!(matches!(
            trips.get_members(&malformed, &human(&leader)).await,
            Err(TripRepoError::NotFound)
        ));
        assert!(matches!(
            trips
                .remove_member(&malformed, &human(&leader), &leader.id)
                .await,
            Err(TripRepoError::NotFound)
        ));
        assert!(matches!(
            trips
                .create_invite(
                    &malformed,
                    &human(&leader),
                    invite(
                        &format!("malformed-path-{index}"),
                        "trip-a",
                        &leader,
                        &format!("malformed-{index}@example.com"),
                    ),
                )
                .await,
            Err(TripRepoError::NotFound)
        ));
    }

    for malformed_target in [String::new(), " ".to_string(), "x".repeat(201)] {
        assert!(matches!(
            trips
                .remove_member("trip-a", &human(&leader), &UserId(malformed_target))
                .await,
            Err(TripRepoError::NotFound)
        ));
    }
    assert!(matches!(
        trips
            .get_trip(
                "trip-a",
                &TripAuthorizationContext::human(UserId(" ".to_string())),
            )
            .await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn invite_acceptance_is_atomic_idempotent_and_supports_reviewed_reinvites() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let invitee = seed_user(&users, "invitee", "invitee@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    let first = invite("invite-1", "trip-a", &leader, invitee.email.as_str());
    let expected_first = first.as_invite().clone();
    assert_eq!(
        trips
            .create_invite("trip-a", &human(&leader), first.clone())
            .await
            .unwrap(),
        expected_first
    );
    assert!(matches!(
        trips
            .create_invite(
                "trip-a",
                &human(&leader),
                invite(
                    "invite-duplicate",
                    "trip-a",
                    &leader,
                    invitee.email.as_str()
                ),
            )
            .await,
        Err(TripRepoError::DuplicateInvite)
    ));
    trips
        .accept_pending_invites(&human(&invitee), &invitee, NOW)
        .await
        .expect("accept pending invite");
    trips
        .accept_pending_invites(&human(&invitee), &invitee, NOW)
        .await
        .expect("exact repeated /me is a no-op");
    assert_invite_state(&database, "trip-a", invitee.email.as_str(), "accepted", 2).await;
    assert_eq!(membership_count(&database, "trip-a", &invitee.id).await, 1);

    let trip_revision_after_first: i64 =
        sqlx::query_scalar("SELECT revision FROM trips WHERE id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap();
    let second = invite("invite-2", "trip-a", &leader, invitee.email.as_str());
    trips
        .create_invite("trip-a", &human(&leader), second)
        .await
        .expect("accepted row may be renewed");
    assert_invite_state(&database, "trip-a", invitee.email.as_str(), "pending", 3).await;
    trips
        .accept_pending_invites(&human(&invitee), &invitee, NOW)
        .await
        .expect("existing membership accepts renewed invite");
    assert_invite_state(&database, "trip-a", invitee.email.as_str(), "accepted", 4).await;
    assert_eq!(membership_count(&database, "trip-a", &invitee.id).await, 1);
    let trip_revision_after_repeat: i64 =
        sqlx::query_scalar("SELECT revision FROM trips WHERE id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap();
    assert_eq!(trip_revision_after_repeat, trip_revision_after_first);

    let crossed = invite("crossed", "trip-b", &leader, "crossed@example.com");
    assert!(matches!(
        trips
            .create_invite("trip-a", &human(&leader), crossed)
            .await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn pending_trip_invites_use_a_partial_index_over_retained_accepted_history() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    let mut transaction = database.db.pool().begin().await.unwrap();
    for index in 0..2_000 {
        let email = format!("accepted-{index:04}@example.com");
        sqlx::query(
            "INSERT INTO trip_invites (\
                trip_id, email_digest, id, email, invited_by, status, created_at, revision\
             ) VALUES ('trip-a', ?, ?, ?, 'leader', 'accepted', ?, 2)",
        )
        .bind(digest(&email))
        .bind(format!("accepted-{index:04}"))
        .bind(email)
        .bind(NOW)
        .execute(&mut *transaction)
        .await
        .unwrap();
    }
    transaction.commit().await.unwrap();

    trips
        .create_invite(
            "trip-a",
            &human(&leader),
            invite("pending", "trip-a", &leader, "pending@example.com"),
        )
        .await
        .expect("retained accepted history does not consume pending capacity");
    assert_eq!(
        trips
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .members()
            .len(),
        1
    );

    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "EXPLAIN QUERY PLAN \
         SELECT email_digest FROM trip_invites \
         WHERE trip_id = ? AND status = 'pending' \
         ORDER BY email_digest LIMIT ?",
    )
    .bind("trip-a")
    .bind(1_001_i64)
    .fetch_all(database.db.pool())
    .await
    .unwrap();
    assert!(
        plan.iter()
            .any(|(_, _, _, detail)| detail.contains("trip_invites_pending_by_trip")),
        "pending lookup must use the partial index, got {plan:?}"
    );

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn concurrent_inviter_and_acceptor_are_serialized_without_losing_the_new_row() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let invitee = seed_user(&users, "invitee", "invitee@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    seed_trip(&trips, "trip-b", &leader).await;
    trips
        .create_invite(
            "trip-a",
            &human(&leader),
            invite("existing", "trip-a", &leader, invitee.email.as_str()),
        )
        .await
        .unwrap();

    let barrier = Arc::new(Barrier::new(3));
    let accept_repo = trips.clone();
    let accept_user = invitee.clone();
    let accept_barrier = barrier.clone();
    let acceptor = tokio::spawn(async move {
        accept_barrier.wait().await;
        accept_repo
            .accept_pending_invites(&human(&accept_user), &accept_user, NOW)
            .await
    });
    let invite_repo = trips.clone();
    let invite_actor = leader.clone();
    let invite_email = invitee.email.to_string();
    let invite_barrier = barrier.clone();
    let inviter = tokio::spawn(async move {
        invite_barrier.wait().await;
        invite_repo
            .create_invite(
                "trip-b",
                &human(&invite_actor),
                invite("concurrent", "trip-b", &invite_actor, &invite_email),
            )
            .await
    });
    barrier.wait().await;
    acceptor.await.unwrap().expect("acceptor completes");
    inviter.await.unwrap().expect("inviter completes");

    assert_invite_state(&database, "trip-a", invitee.email.as_str(), "accepted", 2).await;
    assert_eq!(membership_count(&database, "trip-a", &invitee.id).await, 1);
    let (status,): (String,) = sqlx::query_as(
        "SELECT status FROM trip_invites WHERE trip_id = 'trip-b' AND email_digest = ?",
    )
    .bind(digest(invitee.email.as_str()))
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    let trip_b_membership = membership_count(&database, "trip-b", &invitee.id).await;
    match status.as_str() {
        "accepted" => assert_eq!(trip_b_membership, 1),
        "pending" => assert_eq!(trip_b_membership, 0),
        other => panic!("unexpected terminal state {other}"),
    }

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn multi_row_trip_user_and_acceptance_failures_roll_back_every_prior_write() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let second_member = seed_user(&users, "second-member", "second-member@example.com").await;
    let two_member_trip = trip_with_members(
        "rolled-back-trip",
        vec![
            TripMember::try_new(leader.id.0.clone(), TripRole::Leader, NOW.into()).unwrap(),
            TripMember::try_new(second_member.id.0.clone(), TripRole::Member, NOW.into()).unwrap(),
        ],
    );

    sqlx::query(
        "CREATE TRIGGER fail_membership_insert \
         BEFORE INSERT ON trip_memberships \
         WHEN NEW.user_id = 'second-member' \
         BEGIN SELECT RAISE(FAIL, 'injected membership failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips.create_trip(&human(&leader), two_member_trip).await,
        Err(TripRepoError::Unavailable)
    ));
    let rolled_back: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM trips WHERE id = 'rolled-back-trip'")
            .fetch_one(database.db.pool())
            .await
            .unwrap();
    assert_eq!(rolled_back, 0);
    let memberships: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trip_memberships WHERE trip_id = 'rolled-back-trip'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    assert_eq!(memberships, 0, "the earlier member insert also rolls back");
    sqlx::query("DROP TRIGGER fail_membership_insert")
        .execute(database.db.pool())
        .await
        .unwrap();

    let invitee = seed_user(&users, "invitee", "invitee@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    trips
        .create_invite(
            "trip-a",
            &human(&leader),
            invite("invite", "trip-a", &leader, invitee.email.as_str()),
        )
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_invite_accept \
         BEFORE UPDATE OF status ON trip_invites \
         WHEN NEW.status = 'accepted' \
         BEGIN SELECT RAISE(FAIL, 'injected accept failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips
            .accept_pending_invites(&human(&invitee), &invitee, NOW)
            .await,
        Err(TripRepoError::Unavailable)
    ));
    assert_eq!(membership_count(&database, "trip-a", &invitee.id).await, 0);
    assert_invite_state(&database, "trip-a", invitee.email.as_str(), "pending", 1).await;
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM trips WHERE id = 'trip-a'")
        .fetch_one(database.db.pool())
        .await
        .unwrap();
    assert_eq!(revision, 1);
    sqlx::query("DROP TRIGGER fail_invite_accept")
        .execute(database.db.pool())
        .await
        .unwrap();

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn authorization_is_rechecked_after_the_immediate_writer_reservation() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let other_leader = seed_user(&users, "other-leader", "other@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(&database, "trip-a", &other_leader, TripRole::Leader).await;

    let mut blocker = raw_connection(&database.path).await;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut blocker)
        .await
        .unwrap();
    sqlx::query("UPDATE trip_memberships SET role = 'member', revision = revision + 1 WHERE trip_id = 'trip-a' AND user_id = 'leader'")
        .execute(&mut blocker)
        .await
        .unwrap();

    let repository = trips.clone();
    let actor = leader.clone();
    let task = tokio::spawn(async move {
        repository
            .create_invite(
                "trip-a",
                &human(&actor),
                invite("late", "trip-a", &actor, "late@example.com"),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();
    blocker.close().await.unwrap();

    assert!(matches!(task.await.unwrap(), Err(TripRepoError::Forbidden)));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trip_invites")
        .fetch_one(database.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn profile_collection_is_never_mixed_across_a_concurrent_profile_transaction() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(&database, "trip-a", &member, TripRole::Member).await;
    sqlx::query("UPDATE users SET display_name = 'generation-0', revision = revision + 1")
        .execute(database.db.pool())
        .await
        .unwrap();

    let pool = database.db.pool().clone();
    let writer = tokio::spawn(async move {
        for generation in 1..=40 {
            let mut transaction = pool.begin().await.unwrap();
            let name = format!("generation-{generation}");
            sqlx::query(
                "UPDATE users SET display_name = ?, revision = revision + 1 WHERE id = 'leader'",
            )
            .bind(&name)
            .execute(&mut *transaction)
            .await
            .unwrap();
            tokio::task::yield_now().await;
            sqlx::query(
                "UPDATE users SET display_name = ?, revision = revision + 1 WHERE id = 'member'",
            )
            .bind(&name)
            .execute(&mut *transaction)
            .await
            .unwrap();
            transaction.commit().await.unwrap();
        }
    });
    for _ in 0..80 {
        let profiles = trips.get_members("trip-a", &human(&leader)).await.unwrap();
        let names = profiles
            .iter()
            .map(|profile| profile.display_name.as_deref().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 2);
        assert_eq!(
            names[0], names[1],
            "one read transaction must see one generation"
        );
        tokio::task::yield_now().await;
    }
    writer.await.unwrap();

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn trip_list_accepts_exactly_four_mib_and_rejects_one_more_byte() {
    const MAX_ITEMS: usize = 1_000;
    const MAX_BYTES: usize = 4 * 1024 * 1024;
    const URL_PREFIX: &str = "https://example.test/";

    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;

    let mut expected = (0..MAX_ITEMS)
        .map(|index| TripSummary {
            id: format!("boundary-trip-{index:04}"),
            name: format!("Boundary trip {index:04}"),
            cover_photo_url: Some(URL_PREFIX.to_string()),
            accent_color: None,
            status: TripStatus::Dreaming,
            start_date: "2026-08-07".to_string(),
            end_date: "2026-08-09".to_string(),
            member_count: 1,
            cities: Vec::new(),
        })
        .collect::<Vec<_>>();
    let base_size = serde_json::to_vec(&expected).unwrap().len();
    assert!(base_size < MAX_BYTES);
    let mut remaining = MAX_BYTES - base_size;
    let per_url_capacity = (2_048 - URL_PREFIX.chars().count()) * 4;
    for summary in &mut expected {
        let addition = remaining.min(per_url_capacity);
        summary
            .cover_photo_url
            .as_mut()
            .unwrap()
            .push_str(&exact_utf8_payload(addition));
        remaining -= addition;
    }
    assert_eq!(remaining, 0, "the domain URL bounds can reach 4 MiB");
    assert_eq!(serde_json::to_vec(&expected).unwrap().len(), MAX_BYTES);

    let mut transaction = database.db.pool().begin().await.unwrap();
    for summary in &expected {
        insert_trip_summary(&mut transaction, summary).await;
        insert_membership(&mut transaction, &summary.id, &leader.id.0, "leader").await;
    }
    transaction.commit().await.unwrap();

    let exact = trips
        .list_trips(&human(&leader))
        .await
        .expect("the exact encoded ceiling is accepted");
    assert_eq!(serde_json::to_vec(&exact).unwrap().len(), MAX_BYTES);

    let extendable = expected
        .iter()
        .find(|summary| {
            summary
                .cover_photo_url
                .as_deref()
                .is_some_and(|url| url.chars().count() < 2_048)
        })
        .expect("at least one URL retains one character of headroom");
    let over_limit_url = format!("{}a", extendable.cover_photo_url.as_deref().unwrap());
    sqlx::query("UPDATE trips SET cover_photo_url = ? WHERE id = ?")
        .bind(over_limit_url)
        .bind(&extendable.id)
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        trips.list_trips(&human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn trip_capacity_enforces_999_1000_1001_and_serializes_the_final_slot() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    seed_trip_members_to(&database, "trip-a", 999).await;

    trips
        .create_invite(
            "trip-a",
            &human(&leader),
            invite("slot-1000", "trip-a", &leader, "slot-a@example.com"),
        )
        .await
        .expect("the 1,000th distinct identity fits");
    assert!(matches!(
        trips
            .create_invite(
                "trip-a",
                &human(&leader),
                invite("slot-1001", "trip-a", &leader, "slot-b@example.com"),
            )
            .await,
        Err(TripRepoError::Conflict)
    ));
    insert_invite_raw(
        &database,
        "trip-a",
        "slot-b@example.com",
        "raw-1001",
        &leader.id.0,
    )
    .await;
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    drop(trips);
    drop(users);
    database.shutdown().await;

    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-race", &leader).await;
    seed_trip_members_to(&database, "trip-race", 999).await;
    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for (id, email) in [
        ("race-a", "race-a@example.com"),
        ("race-b", "race-b@example.com"),
    ] {
        let repo = trips.clone();
        let actor = leader.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            repo.create_invite(
                "trip-race",
                &human(&actor),
                invite(id, "trip-race", &actor, email),
            )
            .await
        }));
    }
    barrier.wait().await;
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(TripRepoError::Conflict)))
            .count(),
        1
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trip_invites WHERE trip_id = 'trip-race' AND status = 'pending'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    assert_eq!(pending, 1);
    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn pending_invite_capacity_enforces_99_100_101_without_partial_acceptance() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let target = seed_user(&users, "target", "target@example.com").await;
    seed_numbered_trips(&database, &leader, 101).await;
    for index in 0..99 {
        insert_invite_raw(
            &database,
            &format!("trip-{index:04}"),
            target.email.as_str(),
            &format!("invite-{index:04}"),
            &leader.id.0,
        )
        .await;
    }
    trips
        .create_invite(
            "trip-0099",
            &human(&leader),
            invite("invite-0099", "trip-0099", &leader, target.email.as_str()),
        )
        .await
        .expect("100th pending invite fits");
    assert!(matches!(
        trips
            .create_invite(
                "trip-0100",
                &human(&leader),
                invite("invite-0100", "trip-0100", &leader, target.email.as_str()),
            )
            .await,
        Err(TripRepoError::Conflict)
    ));
    insert_invite_raw(
        &database,
        "trip-0100",
        target.email.as_str(),
        "raw-0100",
        &leader.id.0,
    )
    .await;
    assert!(matches!(
        trips
            .accept_pending_invites(&human(&target), &target, NOW)
            .await,
        Err(TripRepoError::CorruptData)
    ));
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trip_invites WHERE email_digest = ? AND status = 'pending'",
    )
    .bind(digest(target.email.as_str()))
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    let memberships: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM trip_memberships WHERE user_id = ?")
            .bind(&target.id.0)
            .fetch_one(database.db.pool())
            .await
            .unwrap();
    assert_eq!(pending, 101);
    assert_eq!(memberships, 0);

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn canonical_user_capacity_enforces_999_1000_1001_without_partial_acceptance() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let target = seed_user(&users, "target", "target@example.com").await;
    seed_user_capacity_fixture(&database, &target, 1_001, 999).await;
    let leader_999 = user("capacity-leader-0999", "capacity-leader-0999@example.com");
    let leader_1000 = user("capacity-leader-1000", "capacity-leader-1000@example.com");

    trips
        .create_invite(
            "capacity-trip-0999",
            &human(&leader_999),
            invite(
                "capacity-invite-0999",
                "capacity-trip-0999",
                &leader_999,
                target.email.as_str(),
            ),
        )
        .await
        .expect("the 1,000th distinct trip fits");
    assert!(matches!(
        trips
            .create_invite(
                "capacity-trip-1000",
                &human(&leader_1000),
                invite(
                    "capacity-invite-1000",
                    "capacity-trip-1000",
                    &leader_1000,
                    target.email.as_str(),
                ),
            )
            .await,
        Err(TripRepoError::Conflict)
    ));
    insert_invite_raw(
        &database,
        "capacity-trip-1000",
        target.email.as_str(),
        "raw-capacity-1000",
        &leader_1000.id.0,
    )
    .await;
    assert!(matches!(
        trips
            .accept_pending_invites(&human(&target), &target, NOW)
            .await,
        Err(TripRepoError::CorruptData)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trip_memberships WHERE user_id = ?")
            .bind(&target.id.0)
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        999
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM trip_invites WHERE email_digest = ? AND status = 'pending'",
        )
        .bind(digest(target.email.as_str()))
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        2
    );

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn trip_creation_checks_every_initial_members_user_capacity_atomically() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let target = seed_user(&users, "target", "target@example.com").await;
    seed_user_capacity_fixture(&database, &target, 999, 999).await;
    let first_leader = seed_user(&users, "first-leader", "first-leader@example.com").await;
    let second_leader = seed_user(&users, "second-leader", "second-leader@example.com").await;

    let boundary = trip_with_members(
        "boundary-1000",
        vec![
            TripMember::try_new(first_leader.id.0.clone(), TripRole::Leader, NOW.into()).unwrap(),
            TripMember::try_new(target.id.0.clone(), TripRole::Member, NOW.into()).unwrap(),
        ],
    );
    trips
        .create_trip(&human(&first_leader), boundary)
        .await
        .expect("the target's 1,000th distinct trip fits");

    let over_limit = trip_with_members(
        "boundary-1001",
        vec![
            TripMember::try_new(second_leader.id.0.clone(), TripRole::Leader, NOW.into()).unwrap(),
            TripMember::try_new(target.id.0.clone(), TripRole::Member, NOW.into()).unwrap(),
        ],
    );
    assert!(matches!(
        trips.create_trip(&human(&second_leader), over_limit).await,
        Err(TripRepoError::Conflict)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trips WHERE id = 'boundary-1001'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0,
        "a later member's capacity failure rolls back the whole create"
    );

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn orphan_navigation_rows_fail_closed_instead_of_returning_partial_capacity_or_lists() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    let mut raw = raw_connection(&database.path).await;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut raw)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
            .fetch_one(&mut raw)
            .await
            .unwrap(),
        0
    );
    sqlx::query(
        "INSERT INTO trip_memberships \
         (trip_id, user_id, role, joined_at, revision) \
         VALUES ('missing-trip', 'leader', 'member', ?, 1)",
    )
    .bind(NOW)
    .execute(&mut raw)
    .await
    .unwrap();

    assert!(matches!(
        trips.list_trips(&human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    assert!(matches!(
        trips
            .create_trip(&human(&leader), trip("trip-b", &leader))
            .await,
        Err(TripRepoError::CorruptData)
    ));

    sqlx::query("DELETE FROM trip_memberships WHERE trip_id = 'missing-trip'")
        .execute(&mut raw)
        .await
        .unwrap();
    let orphan_email = "orphan@example.com";
    sqlx::query(
        "INSERT INTO trip_invites (\
            trip_id, email_digest, id, email, invited_by, status, created_at, revision\
         ) VALUES ('missing-trip', ?, 'orphan-invite', ?, 'leader', 'pending', ?, 1)",
    )
    .bind(digest(orphan_email))
    .bind(orphan_email)
    .bind(NOW)
    .execute(&mut raw)
    .await
    .unwrap();
    raw.close().await.unwrap();

    assert!(matches!(
        trips
            .create_invite(
                "trip-a",
                &human(&leader),
                invite("valid-invite", "trip-a", &leader, orphan_email),
            )
            .await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn malformed_persisted_values_and_revision_overflow_fail_closed_without_mutation() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let second = seed_user(&users, "second", "second@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(&database, "trip-a", &second, TripRole::Leader).await;
    add_membership(&database, "trip-a", &member, TripRole::Member).await;

    let mut raw = raw_connection(&database.path).await;
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut raw)
        .await
        .unwrap();
    sqlx::query("UPDATE trips SET status = 'unknown' WHERE id = 'trip-a'")
        .execute(&mut raw)
        .await
        .unwrap();
    raw.close().await.unwrap();
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));

    let mut raw = raw_connection(&database.path).await;
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut raw)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE trips SET status = 'dreaming', stop_kind_labels_json = '{broken' WHERE id = 'trip-a'",
    )
    .execute(&mut raw)
    .await
    .unwrap();
    raw.close().await.unwrap();
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));

    let mut raw = raw_connection(&database.path).await;
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut raw)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE trips SET stop_kind_labels_json = NULL, revision = 9223372036854775807 WHERE id = 'trip-a'",
    )
    .execute(&mut raw)
    .await
    .unwrap();
    raw.close().await.unwrap();
    assert!(matches!(
        trips
            .remove_member("trip-a", &human(&leader), &member.id)
            .await,
        Err(TripRepoError::CorruptData)
    ));
    assert_eq!(membership_count(&database, "trip-a", &member.id).await, 1);

    sqlx::query(
        "UPDATE trips SET revision = 1, current_plan_id = 'dangling', current_plan_version = 1 \
         WHERE id = 'trip-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn stored_trip_rows_are_checked_by_domain_invariants() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    sqlx::query(
        "UPDATE trips SET soft_budget_json = '{\"amount\":-1,\"currency\":\"GBP\"}' \
         WHERE id = 'trip-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query(
        "UPDATE trips SET soft_budget_json = '{\"amount\":1,\"currency\":\"gbp\"}' \
         WHERE id = 'trip-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query("UPDATE trips SET soft_budget_json = NULL WHERE id = 'trip-a'")
        .execute(database.db.pool())
        .await
        .unwrap();

    let mut raw = raw_connection(&database.path).await;
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut raw)
        .await
        .unwrap();
    sqlx::query("UPDATE trips SET base_currency = 'gbp' WHERE id = 'trip-a'")
        .execute(&mut raw)
        .await
        .unwrap();
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query("UPDATE trips SET base_currency = 'GBP' WHERE id = 'trip-a'")
        .execute(&mut raw)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE trips SET start_date = '2026-08-10', end_date = '2026-08-09' \
         WHERE id = 'trip-a'",
    )
    .execute(&mut raw)
    .await
    .unwrap();
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query(
        "UPDATE trips SET start_date = '2026-08-07', end_date = '2026-08-09' \
         WHERE id = 'trip-a'",
    )
    .execute(&mut raw)
    .await
    .unwrap();

    sqlx::query(
        "UPDATE trip_memberships SET joined_at = '2026-08-07T13:00:00+01:00' \
         WHERE trip_id = 'trip-a' AND user_id = 'leader'",
    )
    .execute(&mut raw)
    .await
    .unwrap();
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query(
        "UPDATE trip_memberships SET joined_at = ? \
         WHERE trip_id = 'trip-a' AND user_id = 'leader'",
    )
    .bind(NOW)
    .execute(&mut raw)
    .await
    .unwrap();

    sqlx::query(
        "UPDATE trip_memberships SET role = 'member' \
         WHERE trip_id = 'trip-a' AND user_id = 'leader'",
    )
    .execute(&mut raw)
    .await
    .unwrap();
    raw.close().await.unwrap();
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

async fn add_membership(database: &TestDatabase, trip_id: &str, user: &User, role: TripRole) {
    sqlx::query(
        "INSERT INTO trip_memberships \
         (trip_id, user_id, role, joined_at, revision) VALUES (?, ?, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(&user.id.0)
    .bind(match role {
        TripRole::Leader => "leader",
        TripRole::Member => "member",
        TripRole::Viewer => "viewer",
    })
    .bind(NOW)
    .execute(database.db.pool())
    .await
    .expect("insert membership fixture");
}

async fn membership_count(database: &TestDatabase, trip_id: &str, user_id: &UserId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM trip_memberships WHERE trip_id = ? AND user_id = ?")
        .bind(trip_id)
        .bind(&user_id.0)
        .fetch_one(database.db.pool())
        .await
        .unwrap()
}

async fn assert_invite_state(
    database: &TestDatabase,
    trip_id: &str,
    email: &str,
    status: &str,
    revision: i64,
) {
    let actual: (String, i64) = sqlx::query_as(
        "SELECT status, revision FROM trip_invites WHERE trip_id = ? AND email_digest = ?",
    )
    .bind(trip_id)
    .bind(digest(email))
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    assert_eq!(actual, (status.to_string(), revision));
}

async fn seed_trip_members_to(database: &TestDatabase, trip_id: &str, total: usize) {
    let mut transaction = database.db.pool().begin().await.unwrap();
    for index in 1..total {
        let id = format!("bulk-member-{index:04}");
        let email = format!("bulk-member-{index:04}@example.com");
        insert_profile(&mut transaction, &id, &email).await;
        sqlx::query(
            "INSERT INTO trip_memberships \
             (trip_id, user_id, role, joined_at, revision) VALUES (?, ?, 'member', ?, 1)",
        )
        .bind(trip_id)
        .bind(id)
        .bind(NOW)
        .execute(&mut *transaction)
        .await
        .unwrap();
    }
    transaction.commit().await.unwrap();
}

async fn seed_numbered_trips(database: &TestDatabase, leader: &User, count: usize) {
    let mut transaction = database.db.pool().begin().await.unwrap();
    for index in 0..count {
        let trip_id = format!("trip-{index:04}");
        insert_trip(&mut transaction, &trip_id).await;
        insert_membership(&mut transaction, &trip_id, &leader.id.0, "leader").await;
    }
    transaction.commit().await.unwrap();
}

async fn seed_user_capacity_fixture(
    database: &TestDatabase,
    target: &User,
    trip_count: usize,
    target_memberships: usize,
) {
    let mut transaction = database.db.pool().begin().await.unwrap();
    for index in 0..trip_count {
        let leader_id = format!("capacity-leader-{index:04}");
        let leader_email = format!("capacity-leader-{index:04}@example.com");
        let trip_id = format!("capacity-trip-{index:04}");
        insert_profile(&mut transaction, &leader_id, &leader_email).await;
        insert_trip(&mut transaction, &trip_id).await;
        insert_membership(&mut transaction, &trip_id, &leader_id, "leader").await;
        if index < target_memberships {
            insert_membership(&mut transaction, &trip_id, &target.id.0, "member").await;
        }
    }
    transaction.commit().await.unwrap();
}

async fn insert_profile(transaction: &mut Transaction<'_, Sqlite>, id: &str, email: &str) {
    sqlx::query("INSERT INTO users (id, email, display_name, revision) VALUES (?, ?, ?, 1)")
        .bind(id)
        .bind(email)
        .bind(id)
        .execute(&mut **transaction)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_email_claims (email_digest, user_id) VALUES (?, ?)")
        .bind(digest(email))
        .bind(id)
        .execute(&mut **transaction)
        .await
        .unwrap();
}

async fn insert_trip(transaction: &mut Transaction<'_, Sqlite>, id: &str) {
    sqlx::query(
        "INSERT INTO trips (\
            id, name, status, start_date, end_date, base_currency, created_at, revision\
         ) VALUES (?, ?, 'dreaming', '2026-08-07', '2026-08-09', 'GBP', ?, 1)",
    )
    .bind(id)
    .bind(format!("Trip {id}"))
    .bind(NOW)
    .execute(&mut **transaction)
    .await
    .unwrap();
}

async fn insert_trip_summary(transaction: &mut Transaction<'_, Sqlite>, summary: &TripSummary) {
    sqlx::query(
        "INSERT INTO trips (\
            id, name, cover_photo_url, status, start_date, end_date, base_currency, \
            created_at, revision\
         ) VALUES (?, ?, ?, 'dreaming', ?, ?, 'GBP', ?, 1)",
    )
    .bind(&summary.id)
    .bind(&summary.name)
    .bind(summary.cover_photo_url.as_deref())
    .bind(&summary.start_date)
    .bind(&summary.end_date)
    .bind(NOW)
    .execute(&mut **transaction)
    .await
    .unwrap();
}

fn exact_utf8_payload(bytes: usize) -> String {
    let mut value = "\u{1f9ed}".repeat(bytes / 4);
    value.push_str(match bytes % 4 {
        0 => "",
        1 => "a",
        2 => "é",
        3 => "界",
        _ => unreachable!(),
    });
    assert_eq!(value.len(), bytes);
    value
}

async fn insert_membership(
    transaction: &mut Transaction<'_, Sqlite>,
    trip_id: &str,
    user_id: &str,
    role: &str,
) {
    sqlx::query(
        "INSERT INTO trip_memberships \
         (trip_id, user_id, role, joined_at, revision) VALUES (?, ?, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(user_id)
    .bind(role)
    .bind(NOW)
    .execute(&mut **transaction)
    .await
    .unwrap();
}

async fn insert_invite_raw(
    database: &TestDatabase,
    trip_id: &str,
    email: &str,
    id: &str,
    invited_by: &str,
) {
    sqlx::query(
        "INSERT INTO trip_invites (\
            trip_id, email_digest, id, email, invited_by, status, created_at, revision\
         ) VALUES (?, ?, ?, ?, ?, 'pending', ?, 1)",
    )
    .bind(trip_id)
    .bind(digest(email))
    .bind(id)
    .bind(email)
    .bind(invited_by)
    .bind(NOW)
    .execute(database.db.pool())
    .await
    .unwrap();
}
