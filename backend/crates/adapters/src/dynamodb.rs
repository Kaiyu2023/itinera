//! DynamoDB repository adapters.
//!
//! The table uses a generic `pk` / `sk` key pair so later repositories can
//! colocate complete trip aggregates without changing the physical table.
//! Users are represented by a stable profile keyed by `UserId` and a separate
//! canonical-email claim. Keeping the alias separate lets a future verified
//! email-change flow replace the claim without moving the profile or changing
//! references in memberships, votes, expenses, or audit history.

use std::{collections::HashMap, fmt::Write, time::Duration};

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::{
    Client,
    operation::transact_write_items::TransactWriteItemsError,
    types::{AttributeValue, Put, TransactWriteItem},
};
use itinera_core::{
    domain::user::{Email, User, UserId},
    ports::user::{UserRepo, UserRepoError},
};
use sha2::{Digest, Sha256};
use thiserror::Error;

mod trip_repo;

const PK: &str = "pk";
const SK: &str = "sk";
const ENTITY_TYPE: &str = "entity_type";
const SCHEMA_VERSION: &str = "schema_version";
const USER_ID: &str = "user_id";
const EMAIL: &str = "email";
const DISPLAY_NAME: &str = "display_name";
const MEMBERSHIP_COUNT: &str = "membership_count";

const EMAIL_CLAIM_SK: &str = "CLAIM";
const USER_PROFILE_SK: &str = "PROFILE";
const EMAIL_CLAIM_ENTITY: &str = "USER_EMAIL_CLAIM";
const USER_PROFILE_ENTITY: &str = "USER_PROFILE";
const EMAIL_KEY_PREFIX: &str = "USER_EMAIL#";
const USER_ID_PREFIX: &str = "USER#";
const CURRENT_SCHEMA_VERSION: &str = "1";
const CONDITIONAL_FAILURE: &str = "ConditionalCheckFailed";
const CREATE_ONLY_CONDITION: &str = "attribute_not_exists(#pk) AND attribute_not_exists(#sk)";

/// A production user repository backed by one DynamoDB table.
///
/// `Client` is cheap to clone and owns the SDK connection pool. Construct this
/// repository once during Lambda initialization and share it through
/// `AppState`; constructing it per request would discard connection reuse.
pub struct DynamoUserRepo {
    pub(crate) client: Client,
    pub(crate) table_name: String,
}

/// The one-table adapter implements both `UserRepo` and `TripRepo`; this alias
/// names that broader role without breaking the existing public constructor.
pub type DynamoDb = DynamoUserRepo;

impl DynamoUserRepo {
    /// Build a client from the standard AWS configuration provider chains.
    ///
    /// The SDK checks its supported environment, shared-config, and workload
    /// identity sources. In Lambda, AWS supplies the region and temporary
    /// execution-role credentials; Itinera never stores static AWS keys.
    /// Credential values remain lazily resolved and refreshable inside the SDK.
    /// A missing region is caught here because the client could not form a
    /// DynamoDB endpoint without one.
    pub async fn from_environment(
        table_name: impl Into<String>,
    ) -> Result<Self, DynamoUserRepoBuildError> {
        let table_name = validated_table_name(table_name.into())?;
        // Four nested limits prevent a slow dependency from consuming most of
        // a Lambda invocation:
        // - connect: opening one socket;
        // - read: waiting from request start for the first response byte;
        // - attempt: one complete try, including connection and response; and
        // - operation: every try plus retry backoff combined.
        let timeout_config = aws_config::timeout::TimeoutConfig::builder()
            .connect_timeout(Duration::from_secs(2))
            .read_timeout(Duration::from_secs(3))
            .operation_attempt_timeout(Duration::from_secs(4))
            .operation_timeout(Duration::from_secs(8))
            .build();
        let config = aws_config::defaults(BehaviorVersion::latest())
            .timeout_config(timeout_config)
            .load()
            .await;
        if config.region().is_none() {
            return Err(DynamoUserRepoBuildError::MissingRegion);
        }
        Ok(Self {
            client: Client::new(&config),
            table_name,
        })
    }

    /// Build from an existing client. Kept public so tests and future local
    /// DynamoDB tooling can inject an endpoint-configured SDK client without
    /// exposing AWS types through the core repository port.
    pub fn new(
        client: Client,
        table_name: impl Into<String>,
    ) -> Result<Self, DynamoUserRepoBuildError> {
        let table_name = validated_table_name(table_name.into())?;
        Ok(Self { client, table_name })
    }

    /// Fetch one exact item with strong consistency.
    ///
    /// The SDK returns `GetItemOutput`; its `item` field is
    /// `Option<HashMap<String, AttributeValue>>`. Returning that field directly
    /// keeps AWS response wrappers out of the repository logic: `None` means
    /// the exact `(pk, sk)` pair does not exist.
    async fn get_item(
        &self,
        partition_key: String,
        sort_key: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, UserRepoError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key(PK, AttributeValue::S(partition_key))
            .key(SK, AttributeValue::S(sort_key.to_string()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;

        Ok(output.item)
    }
}

#[derive(Debug, Error)]
pub enum DynamoUserRepoBuildError {
    #[error("AWS_REGION (or another AWS SDK region source) must be configured")]
    MissingRegion,
    #[error(
        "the DynamoDB table name must be 3-255 characters using only letters, numbers, '.', '-' or '_'"
    )]
    InvalidTableName,
}

#[async_trait]
impl UserRepo for DynamoUserRepo {
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, UserRepoError> {
        // Resolve the replaceable login alias first, then load the stable
        // profile. Both reads are strong: after a lost provisioning race, the
        // caller must immediately see the transaction that won.
        let Some(claim) = self
            .get_item(email_partition_key(email), EMAIL_CLAIM_SK)
            .await?
        else {
            return Ok(None);
        };
        let user_id = decode_email_claim(&claim, email)?;
        let profile = self
            .get_item(user_partition_key(&user_id), USER_PROFILE_SK)
            .await?
            .ok_or(UserRepoError::CorruptData)?;

        decode_user_profile(&profile, &user_id, email).map(Some)
    }

    async fn find_by_id(&self, user_id: &UserId) -> Result<Option<User>, UserRepoError> {
        self.get_item(user_partition_key(user_id), USER_PROFILE_SK)
            .await?
            .map(|profile| decode_user_profile_by_id(&profile, user_id))
            .transpose()
    }

    async fn insert(&self, user: User) -> Result<(), UserRepoError> {
        // The email claim is deliberately first. DynamoDB returns cancellation
        // reasons in request order, so only failure of action 0 means
        // `DuplicateEmail`; a UserId collision is an invariant/storage failure,
        // not a misleading email conflict.
        let claim = create_only_put(&self.table_name, encode_email_claim(&user));
        let profile = create_only_put(&self.table_name, encode_user_profile(&user));
        let result = self
            .client
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().put(claim).build())
            .transact_items(TransactWriteItem::builder().put(profile).build())
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(error) if email_claim_condition_failed(error.as_service_error()) => {
                Err(UserRepoError::DuplicateEmail(user.email))
            }
            Err(_) => Err(UserRepoError::UserRepoUnavailable),
        }
    }
}

fn create_only_put(table_name: &str, item: HashMap<String, AttributeValue>) -> Put {
    Put::builder()
        .table_name(table_name)
        .set_item(Some(item))
        .condition_expression(CREATE_ONLY_CONDITION)
        .expression_attribute_names("#pk", PK)
        .expression_attribute_names("#sk", SK)
        .build()
        .expect("table name and item are present")
}

fn email_claim_condition_failed(error: Option<&TransactWriteItemsError>) -> bool {
    let Some(TransactWriteItemsError::TransactionCanceledException(cancellation)) = error else {
        return false;
    };

    cancellation
        .cancellation_reasons()
        .first()
        .and_then(|reason| reason.code())
        == Some(CONDITIONAL_FAILURE)
}

fn valid_table_name(name: &str) -> bool {
    (3..=255).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn validated_table_name(name: String) -> Result<String, DynamoUserRepoBuildError> {
    valid_table_name(&name)
        .then_some(name)
        .ok_or(DynamoUserRepoBuildError::InvalidTableName)
}

fn email_partition_key(email: &Email) -> String {
    let digest = Sha256::digest(email.as_str().as_bytes());
    let mut key = String::with_capacity(EMAIL_KEY_PREFIX.len() + digest.len() * 2);
    key.push_str(EMAIL_KEY_PREFIX);
    for byte in digest {
        // Writing to a String cannot fail. Keeping the conversion here avoids
        // another runtime dependency for a single fixed-size digest.
        write!(&mut key, "{byte:02x}").expect("writing to a String is infallible");
    }
    key
}

fn user_partition_key(user_id: &UserId) -> String {
    format!("{USER_ID_PREFIX}{}", user_id.0)
}

fn encode_email_claim(user: &User) -> HashMap<String, AttributeValue> {
    HashMap::from([
        (
            PK.to_string(),
            AttributeValue::S(email_partition_key(&user.email)),
        ),
        (
            SK.to_string(),
            AttributeValue::S(EMAIL_CLAIM_SK.to_string()),
        ),
        (
            ENTITY_TYPE.to_string(),
            AttributeValue::S(EMAIL_CLAIM_ENTITY.to_string()),
        ),
        (
            SCHEMA_VERSION.to_string(),
            AttributeValue::N(CURRENT_SCHEMA_VERSION.to_string()),
        ),
        (USER_ID.to_string(), AttributeValue::S(user.id.0.clone())),
    ])
}

fn encode_user_profile(user: &User) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        (
            PK.to_string(),
            AttributeValue::S(user_partition_key(&user.id)),
        ),
        (
            SK.to_string(),
            AttributeValue::S(USER_PROFILE_SK.to_string()),
        ),
        (
            ENTITY_TYPE.to_string(),
            AttributeValue::S(USER_PROFILE_ENTITY.to_string()),
        ),
        (
            SCHEMA_VERSION.to_string(),
            AttributeValue::N(CURRENT_SCHEMA_VERSION.to_string()),
        ),
        (USER_ID.to_string(), AttributeValue::S(user.id.0.clone())),
        (EMAIL.to_string(), AttributeValue::S(user.email.to_string())),
        (
            MEMBERSHIP_COUNT.to_string(),
            AttributeValue::N("0".to_string()),
        ),
    ]);
    if let Some(display_name) = &user.display_name {
        item.insert(
            DISPLAY_NAME.to_string(),
            AttributeValue::S(display_name.clone()),
        );
    }
    item
}

fn decode_email_claim(
    item: &HashMap<String, AttributeValue>,
    requested_email: &Email,
) -> Result<UserId, UserRepoError> {
    if string_attribute(item, ENTITY_TYPE)? != EMAIL_CLAIM_ENTITY
        || number_attribute(item, SCHEMA_VERSION)? != CURRENT_SCHEMA_VERSION
        || string_attribute(item, PK)? != email_partition_key(requested_email)
        || string_attribute(item, SK)? != EMAIL_CLAIM_SK
    {
        return Err(UserRepoError::CorruptData);
    }

    Ok(UserId(string_attribute(item, USER_ID)?.to_string()))
}

fn decode_user_profile(
    item: &HashMap<String, AttributeValue>,
    expected_user_id: &UserId,
    requested_email: &Email,
) -> Result<User, UserRepoError> {
    let user = decode_user_profile_by_id(item, expected_user_id)?;
    if &user.email != requested_email {
        return Err(UserRepoError::CorruptData);
    }
    Ok(user)
}

fn decode_user_profile_by_id(
    item: &HashMap<String, AttributeValue>,
    expected_user_id: &UserId,
) -> Result<User, UserRepoError> {
    if string_attribute(item, ENTITY_TYPE)? != USER_PROFILE_ENTITY
        || number_attribute(item, SCHEMA_VERSION)? != CURRENT_SCHEMA_VERSION
        || string_attribute(item, PK)? != user_partition_key(expected_user_id)
        || string_attribute(item, SK)? != USER_PROFILE_SK
        || string_attribute(item, USER_ID)? != expected_user_id.0.as_str()
    {
        return Err(UserRepoError::CorruptData);
    }

    let email =
        Email::parse(string_attribute(item, EMAIL)?).map_err(|_| UserRepoError::CorruptData)?;
    let display_name = item
        .get(DISPLAY_NAME)
        .map(|value| {
            value
                .as_s()
                .cloned()
                .map_err(|_| UserRepoError::CorruptData)
        })
        .transpose()?;

    Ok(User {
        id: expected_user_id.clone(),
        email,
        display_name,
    })
}

fn string_attribute<'a>(
    item: &'a HashMap<String, AttributeValue>,
    name: &str,
) -> Result<&'a str, UserRepoError> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
        .ok_or(UserRepoError::CorruptData)
}

fn number_attribute<'a>(
    item: &'a HashMap<String, AttributeValue>,
    name: &str,
) -> Result<&'a str, UserRepoError> {
    item.get(name)
        .and_then(|value| value.as_n().ok())
        .map(String::as_str)
        .ok_or(UserRepoError::CorruptData)
}

#[cfg(test)]
mod tests {
    use aws_sdk_dynamodb::{
        operation::{
            get_item::GetItemOutput,
            transact_write_items::{TransactWriteItemsError, TransactWriteItemsOutput},
        },
        types::{CancellationReason, error::TransactionCanceledException},
    };
    use aws_smithy_mocks::{mock, mock_client};

    use super::*;

    const TABLE: &str = "itinera-test";
    const ADDRESS: &str = "cloud.strife@proton.me";

    fn email() -> Email {
        Email::parse(ADDRESS).expect("fixture email should parse")
    }

    fn user(display_name: Option<&str>) -> User {
        User {
            id: UserId("u-cloud".to_string()),
            email: email(),
            display_name: display_name.map(str::to_string),
        }
    }

    #[test]
    fn validates_table_names_before_building_a_client() {
        for valid in ["abc", "itinera-prod", "itinera.prod_2026"] {
            assert!(valid_table_name(valid), "{valid} should be valid");
        }
        for invalid in ["", "ab", "spaces are unsafe", "table/arn", "ééé"] {
            assert!(!valid_table_name(invalid), "{invalid} should be invalid");
        }
    }

    #[test]
    fn email_keys_are_stable_and_do_not_disclose_the_address() {
        let canonical = email_partition_key(&email());
        let variant = email_partition_key(
            &Email::parse("  Cloud.Strife@Proton.ME  ").expect("variant should parse"),
        );

        assert_eq!(canonical, variant);
        assert!(canonical.starts_with(EMAIL_KEY_PREFIX));
        assert!(!canonical.contains("cloud"));
        assert!(!canonical.contains('@'));
    }

    #[test]
    fn profile_and_claim_round_trip_with_and_without_an_optional_name() {
        for expected in [user(None), user(Some("Cloud"))] {
            let claim = encode_email_claim(&expected);
            let user_id =
                decode_email_claim(&claim, &expected.email).expect("encoded claims should decode");
            let decoded =
                decode_user_profile(&encode_user_profile(&expected), &user_id, &expected.email)
                    .expect("encoded profiles should decode");

            assert_eq!(decoded, expected);
            assert_eq!(user_id, expected.id);
            assert_eq!(claim.get(EMAIL), None, "claims do not duplicate raw email");
        }
    }

    #[test]
    fn a_claim_under_the_wrong_email_key_is_rejected() {
        let stored = user(None);
        let other = Email::parse("tifa.lockhart@proton.me").expect("should parse");

        assert!(matches!(
            decode_email_claim(&encode_email_claim(&stored), &other),
            Err(UserRepoError::CorruptData)
        ));
    }

    #[test]
    fn a_record_with_an_unknown_schema_version_is_rejected() {
        let expected = user(None);
        let mut item = encode_user_profile(&expected);
        item.insert(
            SCHEMA_VERSION.to_string(),
            AttributeValue::N("2".to_string()),
        );

        assert!(matches!(
            decode_user_profile(&item, &expected.id, &expected.email),
            Err(UserRepoError::CorruptData)
        ));
    }

    #[tokio::test]
    async fn lookup_uses_strong_reads_for_the_hashed_alias_and_stable_profile() {
        let expected = user(Some("Cloud"));
        let claim = encode_email_claim(&expected);
        let profile = encode_user_profile(&expected);
        let claim_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.table_name() == Some(TABLE)
                    && request.consistent_read() == Some(true)
                    && request.key().is_some_and(|key| {
                        key.get(PK) == Some(&AttributeValue::S(email_partition_key(&email())))
                            && key.get(SK) == Some(&AttributeValue::S(EMAIL_CLAIM_SK.to_string()))
                            && !format!("{key:?}").contains(ADDRESS)
                    })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(claim.clone()))
                    .build()
            });
        let expected_id = expected.id.clone();
        let profile_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(move |request| {
                request.table_name() == Some(TABLE)
                    && request.consistent_read() == Some(true)
                    && request.key().is_some_and(|key| {
                        key.get(PK) == Some(&AttributeValue::S(user_partition_key(&expected_id)))
                            && key.get(SK) == Some(&AttributeValue::S(USER_PROFILE_SK.to_string()))
                            && !format!("{key:?}").contains(ADDRESS)
                    })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(profile.clone()))
                    .build()
            });
        let client = mock_client!(aws_sdk_dynamodb, [&claim_rule, &profile_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        let found = repo.find_by_email(&email()).await.expect("lookup succeeds");

        assert_eq!(found, Some(expected));
        assert_eq!(claim_rule.num_calls(), 1);
        assert_eq!(profile_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn an_absent_claim_is_an_unknown_user_without_a_profile_read() {
        let claim_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .then_output(|| GetItemOutput::builder().build());
        let client = mock_client!(aws_sdk_dynamodb, [&claim_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        assert_eq!(repo.find_by_email(&email()).await.expect("lookup"), None);
        assert_eq!(claim_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn a_claim_without_its_profile_is_corrupt_data() {
        let expected = user(None);
        let claim = encode_email_claim(&expected);
        let claim_rule = mock!(aws_sdk_dynamodb::Client::get_item).then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(claim.clone()))
                .build()
        });
        let profile_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .then_output(|| GetItemOutput::builder().build());
        let client = mock_client!(aws_sdk_dynamodb, [&claim_rule, &profile_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        assert!(matches!(
            repo.find_by_email(&email()).await,
            Err(UserRepoError::CorruptData)
        ));
    }

    #[tokio::test]
    async fn insert_atomically_creates_the_claim_and_profile() {
        let expected = user(None);
        let request_user = expected.clone();
        let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .match_requests(move |request| {
                let items = request.transact_items();
                items.len() == 2
                    && create_put_matches(items[0].put(), &encode_email_claim(&request_user))
                    && create_put_matches(items[1].put(), &encode_user_profile(&request_user))
            })
            .then_output(|| TransactWriteItemsOutput::builder().build());
        let client = mock_client!(aws_sdk_dynamodb, [&transaction_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        repo.insert(expected).await.expect("insert should succeed");

        assert_eq!(transaction_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn a_failed_uniqueness_condition_is_a_duplicate_email() {
        let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .then_error(|| cancelled_transaction([Some(CONDITIONAL_FAILURE), Some("None")]));
        let client = mock_client!(aws_sdk_dynamodb, [&transaction_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        let error = repo
            .insert(user(None))
            .await
            .expect_err("duplicate should fail");

        assert!(matches!(error, UserRepoError::DuplicateEmail(_)));
    }

    #[tokio::test]
    async fn a_profile_key_collision_is_not_misreported_as_a_duplicate_email() {
        let transaction_rule = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .then_error(|| cancelled_transaction([Some("None"), Some(CONDITIONAL_FAILURE)]));
        let client = mock_client!(aws_sdk_dynamodb, [&transaction_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        let error = repo
            .insert(user(None))
            .await
            .expect_err("profile collision should fail");

        assert!(matches!(error, UserRepoError::UserRepoUnavailable));
    }

    fn create_put_matches(
        put: Option<&Put>,
        expected_item: &HashMap<String, AttributeValue>,
    ) -> bool {
        put.is_some_and(|put| {
            put.table_name() == TABLE
                && put.item() == expected_item
                && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                && put.expression_attribute_names().is_some_and(|names| {
                    names.get("#pk").is_some_and(|name| name == PK)
                        && names.get("#sk").is_some_and(|name| name == SK)
                })
        })
    }

    fn cancelled_transaction<const N: usize>(
        reason_codes: [Option<&str>; N],
    ) -> TransactWriteItemsError {
        let mut error = TransactionCanceledException::builder();
        for code in reason_codes {
            let mut reason = CancellationReason::builder();
            if let Some(code) = code {
                reason = reason.code(code);
            }
            error = error.cancellation_reasons(reason.build());
        }

        TransactWriteItemsError::TransactionCanceledException(error.build())
    }
}
