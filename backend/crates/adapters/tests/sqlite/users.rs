use std::sync::Arc;

use itinera_core::{
    domain::user::{Email, User, UserId},
    ports::user::{UserRepo, UserRepoError},
};
use sqlx::Connection;
use tokio::sync::Barrier;

use super::support::{TestDatabase, raw_connection, user};

#[tokio::test]
async fn user_repository_round_trips_both_keys_and_preserves_duplicate_semantics() {
    let database = TestDatabase::new().await;
    let repo = database.users();
    let cloud = user("user-cloud", "Cloud.Strife@Proton.ME");
    repo.insert(cloud.clone()).await.expect("insert profile");

    assert_eq!(
        repo.find_by_email(&Email::parse("cloud.strife@proton.me").unwrap())
            .await
            .expect("find by email"),
        Some(cloud.clone())
    );
    assert_eq!(
        repo.find_by_id(&cloud.id).await.expect("find by id"),
        Some(cloud.clone())
    );
    assert!(
        repo.find_by_id(&UserId("missing".into()))
            .await
            .expect("missing lookup")
            .is_none()
    );

    let duplicate = User {
        id: UserId("another-id".into()),
        ..cloud.clone()
    };
    assert!(matches!(
        repo.insert(duplicate).await,
        Err(UserRepoError::DuplicateEmail(email)) if email == cloud.email
    ));

    let id_collision = User {
        id: cloud.id.clone(),
        email: Email::parse("different@example.com").unwrap(),
        display_name: Some("Different".into()),
    };
    assert!(matches!(
        repo.insert(id_collision).await,
        Err(UserRepoError::UserRepoUnavailable)
    ));

    drop(repo);
    database.shutdown().await;
}

#[tokio::test]
async fn concurrent_first_login_creates_exactly_one_reciprocal_profile_and_claim() {
    let database = TestDatabase::new().await;
    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for id in ["racer-a", "racer-b"] {
        let repo = database.users();
        let barrier = barrier.clone();
        let value = user(id, "race@example.com");
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            repo.insert(value).await
        }));
    }
    barrier.wait().await;
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.expect("join insert"));
    }
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(UserRepoError::DuplicateEmail(_))))
            .count(),
        1
    );
    let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(database.db.pool())
        .await
        .unwrap();
    let claims: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_email_claims")
        .fetch_one(database.db.pool())
        .await
        .unwrap();
    assert_eq!((profiles, claims), (1, 1));
    database.shutdown().await;
}

#[tokio::test]
async fn profile_and_claim_insert_rolls_back_when_the_second_write_fails() {
    let database = TestDatabase::new().await;
    sqlx::query(
        "CREATE TRIGGER fail_claim_insert \
         BEFORE INSERT ON user_email_claims \
         BEGIN SELECT RAISE(FAIL, 'injected claim failure'); END",
    )
    .execute(database.db.pool())
    .await
    .expect("install fault trigger");
    let repo = database.users();
    assert!(matches!(
        repo.insert(user("rollback-user", "rollback@example.com"))
            .await,
        Err(UserRepoError::UserRepoUnavailable)
    ));
    let rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM users) + \
                (SELECT COUNT(*) FROM user_email_claims)",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    assert_eq!(rows, 0);
    sqlx::query("DROP TRIGGER fail_claim_insert")
        .execute(database.db.pool())
        .await
        .unwrap();
    drop(repo);
    database.shutdown().await;
}

#[tokio::test]
async fn crossed_missing_and_invalid_revision_claim_profiles_fail_closed() {
    let database = TestDatabase::new().await;
    let repo = database.users();
    let value = user("corrupt-user", "corrupt@example.com");
    repo.insert(value.clone()).await.unwrap();

    sqlx::query(
        "UPDATE user_email_claims \
         SET email_digest = '0000000000000000000000000000000000000000000000000000000000000000' \
         WHERE user_id = ?",
    )
    .bind(&value.id.0)
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        repo.find_by_id(&value.id).await,
        Err(UserRepoError::CorruptData)
    ));
    assert!(matches!(
        repo.find_by_email(&value.email).await,
        Err(UserRepoError::CorruptData)
    ));

    let other = user("missing-claim", "missing-claim@example.com");
    repo.insert(other.clone()).await.unwrap();
    sqlx::query("DELETE FROM user_email_claims WHERE user_id = ?")
        .bind(&other.id.0)
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        repo.find_by_id(&other.id).await,
        Err(UserRepoError::CorruptData)
    ));

    let invalid_revision = user("invalid-revision", "invalid-revision@example.com");
    repo.insert(invalid_revision.clone()).await.unwrap();
    let mut raw = raw_connection(&database.path).await;
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut raw)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET revision = 0 WHERE id = ?")
        .bind(&invalid_revision.id.0)
        .execute(&mut raw)
        .await
        .unwrap();
    raw.close().await.unwrap();
    assert!(matches!(
        repo.find_by_id(&invalid_revision.id).await,
        Err(UserRepoError::CorruptData)
    ));

    let noncanonical = user("noncanonical", "noncanonical@example.com");
    repo.insert(noncanonical.clone()).await.unwrap();
    let mut raw = raw_connection(&database.path).await;
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut raw)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET email = 'NonCanonical@Example.com' WHERE id = ?")
        .bind(&noncanonical.id.0)
        .execute(&mut raw)
        .await
        .unwrap();
    raw.close().await.unwrap();
    assert!(matches!(
        repo.find_by_id(&noncanonical.id).await,
        Err(UserRepoError::CorruptData)
    ));

    drop(repo);
    database.shutdown().await;
}

#[tokio::test]
async fn user_codec_accepts_exact_text_boundaries_and_rejects_the_next_value() {
    let database = TestDatabase::new().await;
    let repo = database.users();
    let exact_email = format!("{}@x", "a".repeat(318));
    let exact = User {
        id: UserId("i".repeat(200)),
        email: Email::parse(&exact_email).unwrap(),
        display_name: Some("d".repeat(200)),
    };
    repo.insert(exact.clone())
        .await
        .expect("exact limits accepted");
    assert_eq!(repo.find_by_id(&exact.id).await.unwrap(), Some(exact));

    let oversized_id = User {
        id: UserId("i".repeat(201)),
        email: Email::parse("large-id@example.com").unwrap(),
        display_name: None,
    };
    assert!(matches!(
        repo.insert(oversized_id).await,
        Err(UserRepoError::CorruptData)
    ));
    let oversized_name = User {
        id: UserId("large-name".into()),
        email: Email::parse("large-name@example.com").unwrap(),
        display_name: Some("d".repeat(201)),
    };
    assert!(matches!(
        repo.insert(oversized_name).await,
        Err(UserRepoError::CorruptData)
    ));
    assert!(
        Email::parse(&format!("{}@x", "a".repeat(319))).is_err(),
        "the domain type prevents an oversized email from reaching persistence"
    );

    drop(repo);
    database.shutdown().await;
}
