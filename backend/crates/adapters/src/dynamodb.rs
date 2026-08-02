//! DynamoDB repository adapters.
//!
//! The table uses a generic `pk` / `sk` key pair so later repositories can
//! colocate complete trip aggregates without changing the physical table. A
//! user profile is addressed by a digest of its canonical email. That makes
//! first-login provisioning a single conditional write: the primary key is
//! also the uniqueness constraint, without an eventually consistent index.

use std::{collections::HashMap, fmt::Write, time::Duration};

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::{Client, types::AttributeValue};
use itinera_core::{
    domain::user::{Email, User, UserId},
    ports::user::{UserRepo, UserRepoError},
};
use sha2::{Digest, Sha256};
use thiserror::Error;

const PK: &str = "pk";
const SK: &str = "sk";
const ENTITY_TYPE: &str = "entity_type";
const SCHEMA_VERSION: &str = "schema_version";
const USER_ID: &str = "user_id";
const EMAIL: &str = "email";
const DISPLAY_NAME: &str = "display_name";
const GSI1_PK: &str = "gsi1pk";
const GSI1_SK: &str = "gsi1sk";

const USER_SK: &str = "PROFILE";
const USER_ENTITY: &str = "USER";
const USER_KEY_PREFIX: &str = "USER_EMAIL#";
const USER_ID_PREFIX: &str = "USER#";
const CURRENT_SCHEMA_VERSION: &str = "1";

/// A production user repository backed by one DynamoDB table.
///
/// `Client` is cheap to clone and owns the SDK connection pool. Construct this
/// repository once during Lambda initialization and share it through
/// `AppState`; constructing it per request would discard connection reuse.
pub struct DynamoUserRepo {
    client: Client,
    table_name: String,
}

impl DynamoUserRepo {
    /// Load the standard AWS region and Lambda execution-role credentials.
    ///
    /// Credential resolution remains lazy, as required by the AWS SDK. A
    /// missing region is caught here because without one the client can never
    /// form a DynamoDB endpoint.
    pub async fn from_environment(
        table_name: impl Into<String>,
    ) -> Result<Self, DynamoUserRepoBuildError> {
        let table_name = validated_table_name(table_name.into())?;
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
        let output = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key(PK, AttributeValue::S(email_partition_key(email)))
            .key(SK, AttributeValue::S(USER_SK.to_string()))
            // Provisioning handles a lost conditional-write race by reading
            // again immediately. A strong read guarantees that winner is
            // visible instead of incorrectly reporting a vanished record.
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;

        output
            .item
            .map(|item| decode_user(&item, email))
            .transpose()
    }

    async fn insert(&self, user: User) -> Result<(), UserRepoError> {
        let result = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(encode_user(&user)))
            .condition_expression("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
            .expression_attribute_names("#pk", PK)
            .expression_attribute_names("#sk", SK)
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|error| error.is_conditional_check_failed_exception()) =>
            {
                Err(UserRepoError::DuplicateEmail(user.email))
            }
            Err(_) => Err(UserRepoError::UserRepoUnavailable),
        }
    }
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
    let mut key = String::with_capacity(USER_KEY_PREFIX.len() + digest.len() * 2);
    key.push_str(USER_KEY_PREFIX);
    for byte in digest {
        // Writing to a String cannot fail. Keeping the conversion here avoids
        // another runtime dependency for a single fixed-size digest.
        write!(&mut key, "{byte:02x}").expect("writing to a String is infallible");
    }
    key
}

fn encode_user(user: &User) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        (
            PK.to_string(),
            AttributeValue::S(email_partition_key(&user.email)),
        ),
        (SK.to_string(), AttributeValue::S(USER_SK.to_string())),
        (
            ENTITY_TYPE.to_string(),
            AttributeValue::S(USER_ENTITY.to_string()),
        ),
        (
            SCHEMA_VERSION.to_string(),
            AttributeValue::N(CURRENT_SCHEMA_VERSION.to_string()),
        ),
        (USER_ID.to_string(), AttributeValue::S(user.id.0.clone())),
        (EMAIL.to_string(), AttributeValue::S(user.email.to_string())),
        (
            GSI1_PK.to_string(),
            AttributeValue::S(format!("{USER_ID_PREFIX}{}", user.id.0)),
        ),
        (GSI1_SK.to_string(), AttributeValue::S(USER_SK.to_string())),
    ]);
    if let Some(display_name) = &user.display_name {
        item.insert(
            DISPLAY_NAME.to_string(),
            AttributeValue::S(display_name.clone()),
        );
    }
    item
}

fn decode_user(
    item: &HashMap<String, AttributeValue>,
    requested_email: &Email,
) -> Result<User, UserRepoError> {
    if string_attribute(item, ENTITY_TYPE)? != USER_ENTITY
        || number_attribute(item, SCHEMA_VERSION)? != CURRENT_SCHEMA_VERSION
        || string_attribute(item, SK)? != USER_SK
    {
        return Err(UserRepoError::CorruptData);
    }

    let email =
        Email::parse(string_attribute(item, EMAIL)?).map_err(|_| UserRepoError::CorruptData)?;
    if &email != requested_email
        || string_attribute(item, PK)? != email_partition_key(requested_email)
    {
        return Err(UserRepoError::CorruptData);
    }

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
        id: UserId(string_attribute(item, USER_ID)?.to_string()),
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
            put_item::{PutItemError, PutItemOutput},
        },
        types::error::ConditionalCheckFailedException,
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
        assert!(canonical.starts_with(USER_KEY_PREFIX));
        assert!(!canonical.contains("cloud"));
        assert!(!canonical.contains('@'));
    }

    #[test]
    fn user_items_round_trip_with_and_without_an_optional_name() {
        for expected in [user(None), user(Some("Cloud"))] {
            let decoded = decode_user(&encode_user(&expected), &expected.email)
                .expect("encoded users should decode");
            assert_eq!(decoded, expected);
        }
    }

    #[test]
    fn a_record_under_the_wrong_email_key_is_rejected() {
        let stored = user(None);
        let other = Email::parse("tifa.lockhart@proton.me").expect("should parse");

        assert!(matches!(
            decode_user(&encode_user(&stored), &other),
            Err(UserRepoError::CorruptData)
        ));
    }

    #[test]
    fn a_record_with_an_unknown_schema_version_is_rejected() {
        let expected = user(None);
        let mut item = encode_user(&expected);
        item.insert(
            SCHEMA_VERSION.to_string(),
            AttributeValue::N("2".to_string()),
        );

        assert!(matches!(
            decode_user(&item, &expected.email),
            Err(UserRepoError::CorruptData)
        ));
    }

    #[tokio::test]
    async fn lookup_is_strongly_consistent_and_uses_only_the_hashed_key() {
        let expected = user(Some("Cloud"));
        let item = encode_user(&expected);
        let get_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .match_requests(|request| {
                request.table_name() == Some(TABLE)
                    && request.consistent_read() == Some(true)
                    && request.key().is_some_and(|key| {
                        key.get(PK) == Some(&AttributeValue::S(email_partition_key(&email())))
                            && key.get(SK) == Some(&AttributeValue::S(USER_SK.to_string()))
                            && !format!("{key:?}").contains(ADDRESS)
                    })
            })
            .then_output(move || {
                GetItemOutput::builder()
                    .set_item(Some(item.clone()))
                    .build()
            });
        let client = mock_client!(aws_sdk_dynamodb, [&get_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        let found = repo.find_by_email(&email()).await.expect("lookup succeeds");

        assert_eq!(found, Some(expected));
        assert_eq!(get_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn an_absent_item_is_an_unknown_user() {
        let get_rule = mock!(aws_sdk_dynamodb::Client::get_item)
            .then_output(|| GetItemOutput::builder().build());
        let client = mock_client!(aws_sdk_dynamodb, [&get_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        assert_eq!(repo.find_by_email(&email()).await.expect("lookup"), None);
    }

    #[tokio::test]
    async fn insert_is_conditional_and_keeps_user_data_out_of_the_expression() {
        let expected = user(None);
        let request_user = expected.clone();
        let put_rule = mock!(aws_sdk_dynamodb::Client::put_item)
            .match_requests(move |request| {
                request.table_name() == Some(TABLE)
                    && request.item() == Some(&encode_user(&request_user))
                    && request.condition_expression()
                        == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                    && request.expression_attribute_names().is_some_and(|names| {
                        names.get("#pk").is_some_and(|name| name == PK)
                            && names.get("#sk").is_some_and(|name| name == SK)
                    })
            })
            .then_output(|| PutItemOutput::builder().build());
        let client = mock_client!(aws_sdk_dynamodb, [&put_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        repo.insert(expected).await.expect("insert should succeed");

        assert_eq!(put_rule.num_calls(), 1);
    }

    #[tokio::test]
    async fn a_failed_uniqueness_condition_is_a_duplicate_email() {
        let put_rule = mock!(aws_sdk_dynamodb::Client::put_item).then_error(|| {
            PutItemError::ConditionalCheckFailedException(
                ConditionalCheckFailedException::builder().build(),
            )
        });
        let client = mock_client!(aws_sdk_dynamodb, [&put_rule]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("valid config");

        let error = repo
            .insert(user(None))
            .await
            .expect_err("duplicate should fail");

        assert!(matches!(error, UserRepoError::DuplicateEmail(_)));
    }
}
