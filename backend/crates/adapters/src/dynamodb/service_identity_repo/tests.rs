use std::collections::HashMap;

use aws_sdk_dynamodb::{
    operation::{
        get_item::GetItemOutput,
        transact_write_items::{TransactWriteItemsError, TransactWriteItemsOutput},
    },
    types::{AttributeValue, CancellationReason, error::TransactionCanceledException},
};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use itinera_core::{
    domain::{
        service_identity::{ServiceIdentity, ServiceScope},
        trip::{TripMember, TripRole},
        user::UserId,
    },
    ports::{
        auth::ServiceCommonName,
        service_identity::{NewServiceIdentity, ServiceIdentityRepo, ServiceIdentityRepoError},
    },
};

use super::records::*;
use crate::dynamodb::{
    CONDITIONAL_FAILURE, CREATE_ONLY_CONDITION, CURRENT_SCHEMA_VERSION, DynamoUserRepo,
    ENTITY_TYPE, PK, REVISION, SCHEMA_VERSION, SK, USER_ID,
    trip_repo::records::{
        DATA, GSI1PK, GSI1SK, MEMBER_ENTITY, ROLE, encode_record, member_sk, role_value, trip_pk,
    },
    user_partition_key,
};

const TABLE: &str = "itinera-test";
const OWNER: &str = "owner-a";
const TRIP: &str = "trip-a";
const SERVICE: &str = "service-a";
const COMMON_NAME: &str = "1234567890abcdef1234567890abcdef.access";
const CREATED: &str = "2026-08-06T12:00:00Z";
const EXPIRES: &str = "2026-08-07T12:00:00Z";
const USED: &str = "2026-08-06T12:30:00Z";

fn repo() -> DynamoUserRepo {
    DynamoUserRepo::new(mock_client!(aws_sdk_dynamodb, &[]), TABLE).expect("valid repo")
}

fn owner() -> UserId {
    UserId(OWNER.into())
}

fn identity(scopes: Vec<ServiceScope>) -> ServiceIdentity {
    ServiceIdentity {
        id: SERVICE.into(),
        name: "assistant".into(),
        client_id_hint: "12345678".into(),
        scopes,
        trip_ids: vec![TRIP.into()],
        expires_at: EXPIRES.into(),
        last_used_at: None,
        revoked_at: None,
        created_at: CREATED.into(),
    }
}

fn mapping(scopes: Vec<ServiceScope>) -> ServiceMappingRecord {
    ServiceMappingRecord::from_new(&owner(), &identity(scopes), common_name_digest(COMMON_NAME))
}

fn claim(status: ClaimStatus) -> ServiceClaimRecord {
    ServiceClaimRecord {
        common_name_digest: common_name_digest(COMMON_NAME),
        owner_id: OWNER.into(),
        service_id: SERVICE.into(),
        status,
    }
}

fn membership_item(role: TripRole) -> HashMap<String, AttributeValue> {
    let member = TripMember {
        user_id: OWNER.into(),
        role,
        joined_at: CREATED.into(),
    };
    let mut item = encode_record(
        trip_pk(TRIP),
        member_sk(&owner()),
        MEMBER_ENTITY,
        &member,
        4,
    )
    .unwrap();
    item.insert(USER_ID.into(), AttributeValue::S(OWNER.into()));
    item.insert(ROLE.into(), AttributeValue::S(role_value(role).into()));
    item.insert(
        GSI1PK.into(),
        AttributeValue::S(user_partition_key(&owner())),
    );
    item.insert(GSI1SK.into(), AttributeValue::S(format!("TRIP#{TRIP}")));
    item
}

fn get_rule(
    partition_key: String,
    sort_key: String,
    item: Option<HashMap<String, AttributeValue>>,
) -> aws_smithy_mocks::Rule {
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.table_name() == Some(TABLE)
                && request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(partition_key.clone()))
                        && key.get(SK) == Some(&AttributeValue::S(sort_key.clone()))
                })
        })
        .then_output(move || GetItemOutput::builder().set_item(item.clone()).build())
}

fn membership_rule(role: TripRole) -> aws_smithy_mocks::Rule {
    get_rule(
        trip_pk(TRIP),
        member_sk(&owner()),
        Some(membership_item(role)),
    )
}

fn new_identity(scopes: Vec<ServiceScope>) -> NewServiceIdentity {
    NewServiceIdentity {
        identity: identity(scopes),
        common_name: ServiceCommonName::parse(COMMON_NAME).unwrap(),
    }
}

fn usage_item(count: u32) -> HashMap<String, AttributeValue> {
    let digest = common_name_digest(COMMON_NAME);
    HashMap::from([
        (PK.into(), AttributeValue::S(usage_pk(&digest))),
        (SK.into(), AttributeValue::S(usage_sk("2026080612"))),
        (
            ENTITY_TYPE.into(),
            AttributeValue::S(SERVICE_USAGE_ENTITY.into()),
        ),
        (
            SCHEMA_VERSION.into(),
            AttributeValue::N(CURRENT_SCHEMA_VERSION.into()),
        ),
        (COMMON_NAME_DIGEST.into(), AttributeValue::S(digest)),
        (COUNT.into(), AttributeValue::N(count.to_string())),
        (LAST_USED_AT.into(), AttributeValue::S(USED.into())),
        (
            BUCKET_EXPIRES_AT.into(),
            AttributeValue::S("2026-08-06T13:00:00Z".into()),
        ),
        (TTL.into(), AttributeValue::N("1786194000".into())),
    ])
}

fn cancellation_at(index: usize) -> TransactWriteItemsError {
    cancellation_with_code(index, CONDITIONAL_FAILURE)
}

fn cancellation_with_code(index: usize, code: &'static str) -> TransactWriteItemsError {
    let mut reasons = Vec::new();
    for action in 0..=index {
        reasons.push(
            CancellationReason::builder()
                .code(if action == index { code } else { "None" })
                .build(),
        );
    }
    TransactWriteItemsError::TransactionCanceledException(
        TransactionCanceledException::builder()
            .set_cancellation_reasons(Some(reasons))
            .build(),
    )
}

#[test]
fn service_records_never_persist_the_raw_cloudflare_client_id() {
    let owner = UserId("owner-a".into());
    let raw = "1234567890abcdef1234567890abcdef.access";
    let digest = common_name_digest(raw);
    let mapping = ServiceMappingRecord {
        id: "service-a".into(),
        owner_id: owner.0.clone(),
        name: "assistant".into(),
        client_id_hint: "12345678".into(),
        common_name_digest: digest.clone(),
        scopes: vec![ServiceScope::Read],
        trip_ids: vec!["trip-a".into()],
        expires_at: "2026-08-07T12:00:00Z".into(),
        revoked_at: None,
        created_at: "2026-08-06T12:00:00Z".into(),
    };
    let claim = ServiceClaimRecord {
        common_name_digest: digest,
        owner_id: owner.0.clone(),
        service_id: mapping.id.clone(),
        status: ClaimStatus::Active,
    };

    let encoded = format!(
        "{:?}{:?}",
        encode_mapping(&owner, &mapping, 1).unwrap(),
        encode_claim(&claim, 1).unwrap()
    );
    assert!(!encoded.contains(raw));
    assert!(encoded.contains("12345678"));
}

#[test]
fn hourly_update_has_an_exact_atomic_limit_and_create_or_validate_condition() {
    let digest = common_name_digest(COMMON_NAME);
    let update = super::operations::usage_update(
        &repo(),
        &digest,
        "2026080612",
        "2026-08-06T12:30:00Z",
        "2026-08-06T13:00:00Z",
        1_000,
    );

    assert_eq!(
        update.condition_expression(),
        Some(
            "(attribute_not_exists(#pk) AND attribute_not_exists(#sk)) OR (#entity = :entity AND #schema = :schema AND #digest = :digest AND #bucket_expiry = :bucket_expiry AND #ttl = :ttl AND #count < :limit)"
        )
    );
    assert_eq!(
        update
            .expression_attribute_values()
            .and_then(|values| values.get(":limit")),
        Some(&aws_sdk_dynamodb::types::AttributeValue::N("300".into()))
    );
}

#[test]
fn common_names_are_hashed_stably_and_case_sensitively() {
    let exact = ServiceCommonName::parse(COMMON_NAME).unwrap();
    assert_eq!(common_name_digest(exact.as_str()).len(), 64);
    assert_ne!(
        common_name_digest(COMMON_NAME),
        common_name_digest("1234567890ABCDEF1234567890ABCDEF.access")
    );
    assert!(ServiceCommonName::parse("1234567890ABCDEF1234567890ABCDEF.access").is_err());
}

#[tokio::test]
async fn creation_rechecks_the_exact_direct_membership_snapshot_in_the_same_transaction() {
    let membership = membership_rule(TripRole::Member);
    let missing_meta = get_rule(user_partition_key(&owner()), SERVICE_META_SK.into(), None);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 4
                && items[0].put().is_some_and(|put| {
                    put.table_name() == TABLE
                        && put.item().get(ENTITY_TYPE)
                            == Some(&AttributeValue::S(SERVICE_META_ENTITY.into()))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("1".into()))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
                && items[1].put().is_some_and(|put| {
                    put.item().get(ENTITY_TYPE)
                        == Some(&AttributeValue::S(SERVICE_MAPPING_ENTITY.into()))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && !format!("{:?}", put.item()).contains(COMMON_NAME)
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(ENTITY_TYPE)
                        == Some(&AttributeValue::S(SERVICE_CLAIM_ENTITY.into()))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
                && items[3].condition_check().is_some_and(|condition| {
                    condition.table_name() == TABLE
                        && condition.key().get(PK) == Some(&AttributeValue::S(trip_pk(TRIP)))
                        && condition.key().get(SK) == Some(&AttributeValue::S(member_sk(&owner())))
                        && condition.condition_expression()
                            == "#entity = :entity AND #revision = :revision AND #data = :data"
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("4".into()))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &missing_meta, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();

    let created = repo
        .create_service_identity(&owner(), new_identity(vec![ServiceScope::Propose]))
        .await
        .expect("member can register propose scope");

    assert_eq!(created.id, SERVICE);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn read_registration_accepts_a_viewer_and_still_conditions_membership() {
    let membership = membership_rule(TripRole::Viewer);
    let missing_meta = get_rule(user_partition_key(&owner()), SERVICE_META_SK.into(), None);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            request.transact_items().get(3).is_some_and(|action| {
                action.condition_check().is_some_and(|condition| {
                    condition.key().get(PK) == Some(&AttributeValue::S(trip_pk(TRIP)))
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":data"))
                            == membership_item(TripRole::Viewer).get(DATA)
                })
            })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &missing_meta, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();

    repo.create_service_identity(&owner(), new_identity(vec![ServiceScope::Read]))
        .await
        .expect("viewer can register read scope");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn propose_registration_rejects_a_viewer_before_any_write() {
    let membership = membership_rule(TripRole::Viewer);
    let client = mock_client!(aws_sdk_dynamodb, [&membership]);
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();

    assert_eq!(
        repo.create_service_identity(&owner(), new_identity(vec![ServiceScope::Propose]))
            .await,
        Err(ServiceIdentityRepoError::Forbidden)
    );
}

#[tokio::test]
async fn a_global_claim_collision_is_not_misreported_as_a_membership_or_meta_conflict() {
    let membership = membership_rule(TripRole::Viewer);
    let missing_meta = get_rule(user_partition_key(&owner()), SERVICE_META_SK.into(), None);
    let transaction =
        mock!(aws_sdk_dynamodb::Client::transact_write_items).then_error(|| cancellation_at(2));
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &missing_meta, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();

    assert_eq!(
        repo.create_service_identity(&owner(), new_identity(vec![ServiceScope::Read]))
            .await,
        Err(ServiceIdentityRepoError::DuplicateCredential)
    );
}

#[tokio::test]
async fn authentication_conditions_both_mapping_records_and_increments_only_the_hour_bucket() {
    let digest = common_name_digest(COMMON_NAME);
    let claim_item = encode_claim(&claim(ClaimStatus::Active), 3).unwrap();
    let mapping_item = encode_mapping(&owner(), &mapping(vec![ServiceScope::Read]), 7).unwrap();
    let claim_read = get_rule(claim_pk(&digest), CLAIM_SK.into(), Some(claim_item));
    let mapping_read = get_rule(
        user_partition_key(&owner()),
        service_sk(SERVICE),
        Some(mapping_item),
    );
    let usage_read = get_rule(usage_pk(&digest), usage_sk("2026080612"), None);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 3
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :entity AND #revision = :revision AND #data = :data"
                        && condition.key().get(PK)
                            == Some(&AttributeValue::S(claim_pk(&common_name_digest(
                                COMMON_NAME,
                            ))))
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :entity AND #revision = :revision AND #data = :data"
                        && condition.key().get(SK) == Some(&AttributeValue::S(service_sk(SERVICE)))
                })
                && items[2].update().is_some_and(|update| {
                    update.key().get(PK)
                        == Some(&AttributeValue::S(usage_pk(&common_name_digest(
                            COMMON_NAME,
                        ))))
                        && update.key().get(SK) == Some(&AttributeValue::S(usage_sk("2026080612")))
                        && update.condition_expression().is_some_and(|expression| {
                            expression.contains("#count < :limit")
                                && expression.contains("attribute_not_exists(#pk)")
                        })
                        && update
                            .expression_attribute_values()
                            .and_then(|values| values.get(":limit"))
                            == Some(&AttributeValue::N("300".into()))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&claim_read, &mapping_read, &usage_read, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();

    let grant = repo
        .authenticate_service(&ServiceCommonName::parse(COMMON_NAME).unwrap(), USED)
        .await
        .expect("active credential authenticates");

    assert_eq!(grant.owner_id, owner());
    assert_eq!(grant.identity.last_used_at.as_deref(), Some(USED));
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn rate_limit_and_cross_record_corruption_fail_closed_without_a_write() {
    let digest = common_name_digest(COMMON_NAME);
    let claim_read = get_rule(
        claim_pk(&digest),
        CLAIM_SK.into(),
        Some(encode_claim(&claim(ClaimStatus::Active), 1).unwrap()),
    );
    let mapping_read = get_rule(
        user_partition_key(&owner()),
        service_sk(SERVICE),
        Some(encode_mapping(&owner(), &mapping(vec![ServiceScope::Read]), 1).unwrap()),
    );
    let usage_read = get_rule(
        usage_pk(&digest),
        usage_sk("2026080612"),
        Some(usage_item(300)),
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&claim_read, &mapping_read, &usage_read]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();
    assert_eq!(
        repo.authenticate_service(&ServiceCommonName::parse(COMMON_NAME).unwrap(), USED)
            .await,
        Err(ServiceIdentityRepoError::RateLimited)
    );

    let claim_read = get_rule(
        claim_pk(&digest),
        CLAIM_SK.into(),
        Some(encode_claim(&claim(ClaimStatus::Active), 1).unwrap()),
    );
    let mapping_read = get_rule(
        user_partition_key(&owner()),
        service_sk(SERVICE),
        Some(encode_mapping(&owner(), &mapping(vec![ServiceScope::Read]), 1).unwrap()),
    );
    let mut corrupt_usage = usage_item(1);
    corrupt_usage.insert(TTL.into(), AttributeValue::N("1786193999".into()));
    let usage_read = get_rule(
        usage_pk(&digest),
        usage_sk("2026080612"),
        Some(corrupt_usage),
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&claim_read, &mapping_read, &usage_read]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();
    assert_eq!(
        repo.authenticate_service(&ServiceCommonName::parse(COMMON_NAME).unwrap(), USED)
            .await,
        Err(ServiceIdentityRepoError::CorruptData)
    );

    let mut foreign_claim = claim(ClaimStatus::Active);
    foreign_claim.service_id = "foreign-service".into();
    let claim_read = get_rule(
        claim_pk(&digest),
        CLAIM_SK.into(),
        Some(encode_claim(&foreign_claim, 1).unwrap()),
    );
    let missing_mapping = get_rule(
        user_partition_key(&owner()),
        service_sk("foreign-service"),
        None,
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&claim_read, &missing_mapping]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();
    assert_eq!(
        repo.authenticate_service(&ServiceCommonName::parse(COMMON_NAME).unwrap(), USED)
            .await,
        Err(ServiceIdentityRepoError::CorruptData)
    );
}

#[tokio::test]
async fn transaction_conflicts_are_retried_for_create_authenticate_and_revoke() {
    let membership = membership_rule(TripRole::Viewer);
    let missing_meta = get_rule(user_partition_key(&owner()), SERVICE_META_SK.into(), None);
    let create_transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .sequence()
        .error(|| cancellation_with_code(0, "TransactionConflict"))
        .output(|| TransactWriteItemsOutput::builder().build())
        .build();
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &missing_meta, &create_transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();
    repo.create_service_identity(&owner(), new_identity(vec![ServiceScope::Read]))
        .await
        .expect("create retries a transaction conflict");
    assert_eq!(create_transaction.num_calls(), 2);
    assert_eq!(missing_meta.num_calls(), 2);

    let digest = common_name_digest(COMMON_NAME);
    let claim_read = get_rule(
        claim_pk(&digest),
        CLAIM_SK.into(),
        Some(encode_claim(&claim(ClaimStatus::Active), 1).unwrap()),
    );
    let mapping_read = get_rule(
        user_partition_key(&owner()),
        service_sk(SERVICE),
        Some(encode_mapping(&owner(), &mapping(vec![ServiceScope::Read]), 1).unwrap()),
    );
    let usage_read = get_rule(usage_pk(&digest), usage_sk("2026080612"), None);
    let auth_transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .sequence()
        .error(|| cancellation_with_code(2, "TransactionConflict"))
        .output(|| TransactWriteItemsOutput::builder().build())
        .build();
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&claim_read, &mapping_read, &usage_read, &auth_transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();
    repo.authenticate_service(&ServiceCommonName::parse(COMMON_NAME).unwrap(), USED)
        .await
        .expect("authentication retries a transaction conflict");
    assert_eq!(auth_transaction.num_calls(), 2);
    assert_eq!(claim_read.num_calls(), 2);
    assert_eq!(mapping_read.num_calls(), 2);
    assert_eq!(usage_read.num_calls(), 2);

    let mapping_read = get_rule(
        user_partition_key(&owner()),
        service_sk(SERVICE),
        Some(encode_mapping(&owner(), &mapping(vec![ServiceScope::Read]), 2).unwrap()),
    );
    let claim_read = get_rule(
        claim_pk(&digest),
        CLAIM_SK.into(),
        Some(encode_claim(&claim(ClaimStatus::Active), 5).unwrap()),
    );
    let revoke_transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .sequence()
        .error(|| cancellation_with_code(0, "TransactionConflict"))
        .output(|| TransactWriteItemsOutput::builder().build())
        .build();
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&mapping_read, &claim_read, &revoke_transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();
    repo.revoke_service_identity(&owner(), SERVICE, USED)
        .await
        .expect("revoke retries a transaction conflict");
    assert_eq!(revoke_transaction.num_calls(), 2);
    assert_eq!(mapping_read.num_calls(), 2);
    assert_eq!(claim_read.num_calls(), 2);
}

#[test]
fn revision_overflow_fails_closed() {
    assert_eq!(
        super::operations::next_revision(u64::MAX),
        Err(ServiceIdentityRepoError::CorruptData)
    );
}

#[tokio::test]
async fn revocation_updates_mapping_and_claim_together_and_an_exact_retry_is_idempotent() {
    let digest = common_name_digest(COMMON_NAME);
    let mapping_item = encode_mapping(&owner(), &mapping(vec![ServiceScope::Read]), 2).unwrap();
    let claim_item = encode_claim(&claim(ClaimStatus::Active), 5).unwrap();
    let mapping_read = get_rule(
        user_partition_key(&owner()),
        service_sk(SERVICE),
        Some(mapping_item),
    );
    let claim_read = get_rule(claim_pk(&digest), CLAIM_SK.into(), Some(claim_item));
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 2
                && items.iter().all(|action| {
                    action.put().is_some_and(|put| {
                        put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                    })
                })
                && items[0].put().is_some_and(|put| {
                    put.item().get(REVISION) == Some(&AttributeValue::N("3".into()))
                })
                && items[1].put().is_some_and(|put| {
                    put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put
                            .item()
                            .get(DATA)
                            .and_then(|value| value.as_s().ok())
                            .is_some_and(|data| data.contains("revoked"))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&mapping_read, &claim_read, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();
    repo.revoke_service_identity(&owner(), SERVICE, USED)
        .await
        .expect("revocation succeeds");

    let mut revoked_mapping = mapping(vec![ServiceScope::Read]);
    revoked_mapping.revoked_at = Some(USED.into());
    let mapping_read = get_rule(
        user_partition_key(&owner()),
        service_sk(SERVICE),
        Some(encode_mapping(&owner(), &revoked_mapping, 3).unwrap()),
    );
    let claim_read = get_rule(
        claim_pk(&digest),
        CLAIM_SK.into(),
        Some(encode_claim(&claim(ClaimStatus::Revoked), 6).unwrap()),
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&mapping_read, &claim_read]
    );
    let repo = DynamoUserRepo::new(client, TABLE).unwrap();
    repo.revoke_service_identity(&owner(), SERVICE, "2026-08-06T12:45:00Z")
        .await
        .expect("repeated revocation is idempotent");
}
