//! Cloudflare Access application-token verification.
//!
//! Access signs browser assertions with an account RSA key and exposes the
//! current and previous keys as JWKS. The adapter pins RS256, validates the
//! application issuer and audience, and only returns an identity after the
//! token's signature and time claims have passed validation.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use itinera_core::{
    domain::user::Email,
    ports::auth::{AuthError, Identity, IdentityProvider},
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, errors::ErrorKind};
use reqwest::{Client, Url};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

const ACCESS_HOST_SUFFIX: &str = ".cloudflareaccess.com";
const CERTS_PATH: &str = "/cdn-cgi/access/certs";
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ASSERTION_BYTES: usize = 16 * 1024;
const MAX_KEY_ID_BYTES: usize = 256;
const MAX_REJECTED_KEY_IDS: usize = 64;

#[derive(Debug, Error)]
pub enum CloudflareAccessBuildError {
    #[error(
        "Cloudflare Access team domain must be an HTTPS origin like \
         https://your-team.cloudflareaccess.com"
    )]
    InvalidTeamDomain,
    #[error("Cloudflare Access application audience must not be empty")]
    MissingAudience,
    #[error("failed to build the Cloudflare Access HTTP client")]
    HttpClient(#[source] reqwest::Error),
}

#[derive(Clone, Debug)]
struct CloudflareAccessConfig {
    issuer: String,
    audience: String,
    jwks_url: Url,
}

impl CloudflareAccessConfig {
    fn try_new(team_domain: &str, audience: &str) -> Result<Self, CloudflareAccessBuildError> {
        let team_domain = team_domain.trim().trim_end_matches('/');
        let mut url =
            Url::parse(team_domain).map_err(|_| CloudflareAccessBuildError::InvalidTeamDomain)?;
        let host = url
            .host_str()
            .filter(|host| {
                host.ends_with(ACCESS_HOST_SUFFIX) && host.len() > ACCESS_HOST_SUFFIX.len()
            })
            .ok_or(CloudflareAccessBuildError::InvalidTeamDomain)?
            .to_owned();

        let is_origin = url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none();
        if !is_origin {
            return Err(CloudflareAccessBuildError::InvalidTeamDomain);
        }

        let audience = audience.trim();
        if audience.is_empty() {
            return Err(CloudflareAccessBuildError::MissingAudience);
        }

        let issuer = format!("https://{host}");
        url.set_path(CERTS_PATH);

        Ok(Self {
            issuer,
            audience: audience.to_owned(),
            jwks_url: url,
        })
    }
}

#[derive(Clone, Copy)]
struct CachePolicy {
    fresh_for: Duration,
    stale_for: Duration,
    reject_key_for: Duration,
    retry_after_failure: Duration,
}

const DEFAULT_CACHE_POLICY: CachePolicy = CachePolicy {
    fresh_for: Duration::from_secs(60 * 60),
    stale_for: Duration::from_secs(24 * 60 * 60),
    reject_key_for: Duration::from_secs(30),
    retry_after_failure: Duration::from_secs(5),
};

/// Production `IdentityProvider` for Cloudflare Access browser assertions.
pub struct CloudflareAccessIdentityProvider {
    validation: Validation,
    jwks: Arc<dyn JwksSource>,
    cache: RwLock<KeyCache>,
    refresh: Mutex<()>,
    cache_policy: CachePolicy,
}

impl CloudflareAccessIdentityProvider {
    pub fn new(team_domain: &str, audience: &str) -> Result<Self, CloudflareAccessBuildError> {
        let config = CloudflareAccessConfig::try_new(team_domain, audience)?;
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .timeout(HTTP_REQUEST_TIMEOUT)
            .build()
            .map_err(CloudflareAccessBuildError::HttpClient)?;
        let jwks_url = config.jwks_url.clone();

        Ok(Self::from_parts(
            config,
            Arc::new(HttpJwksSource { client, jwks_url }),
            DEFAULT_CACHE_POLICY,
        ))
    }

    fn from_parts(
        config: CloudflareAccessConfig,
        jwks: Arc<dyn JwksSource>,
        cache_policy: CachePolicy,
    ) -> Self {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[config.audience]);
        validation.set_issuer(&[config.issuer]);
        validation.set_required_spec_claims(&["exp", "nbf", "aud", "iss"]);
        validation.validate_nbf = true;

        Self {
            validation,
            jwks,
            cache: RwLock::new(KeyCache::default()),
            refresh: Mutex::new(()),
            cache_policy,
        }
    }

    async fn decoding_key(&self, key_id: &str) -> Result<DecodingKey, AuthError> {
        let now = Instant::now();
        if let Some(key) = self.fresh_cached_key(key_id, now).await {
            return Ok(key);
        }

        // Only one request may refresh the shared key set. Callers waiting on
        // the lock re-check the cache before making another network request.
        let _refresh_guard = self.refresh.lock().await;
        let now = Instant::now();
        if let Some(key) = self.fresh_cached_key(key_id, now).await {
            return Ok(key);
        }

        {
            let cache = self.cache.read().await;
            if cache.rejected_key_is_fresh(key_id, now, self.cache_policy.reject_key_for) {
                return Err(AuthError::InvalidToken);
            }

            if cache
                .retry_after
                .is_some_and(|retry_after| now < retry_after)
            {
                return cache
                    .stale_key(key_id, now, self.cache_policy.stale_for)
                    .ok_or(AuthError::IdentityProviderUnavailable);
            }
        }

        let refreshed_keys = match self.jwks.fetch().await {
            Ok(document) => parse_keys(document),
            Err(_) => Err(()),
        };

        match refreshed_keys {
            Ok(keys) => {
                let key = keys.get(key_id).cloned();
                let mut cache = self.cache.write().await;
                cache.keys = keys;
                cache.refreshed_at = Some(now);
                cache.retry_after = None;
                cache.prune_rejected_keys(now, self.cache_policy.reject_key_for);

                if key.is_none() {
                    cache.remember_rejected_key(key_id.to_owned(), now);
                }

                key.ok_or(AuthError::InvalidToken)
            }
            Err(()) => {
                let mut cache = self.cache.write().await;
                cache.retry_after = Some(now + self.cache_policy.retry_after_failure);
                cache
                    .stale_key(key_id, now, self.cache_policy.stale_for)
                    .ok_or(AuthError::IdentityProviderUnavailable)
            }
        }
    }

    async fn fresh_cached_key(&self, key_id: &str, now: Instant) -> Option<DecodingKey> {
        let cache = self.cache.read().await;
        cache.fresh_key(key_id, now, self.cache_policy.fresh_for)
    }
}

#[async_trait]
impl IdentityProvider for CloudflareAccessIdentityProvider {
    async fn authenticate(&self, assertion: &str) -> Result<Identity, AuthError> {
        if assertion.is_empty() || assertion.len() > MAX_ASSERTION_BYTES {
            return Err(AuthError::InvalidToken);
        }

        let header = decode_header(assertion).map_err(|_| AuthError::InvalidToken)?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::InvalidToken);
        }

        let key_id = header
            .kid
            .filter(|key_id| {
                !key_id.is_empty() && key_id.len() <= MAX_KEY_ID_BYTES && key_id.trim() == key_id
            })
            .ok_or(AuthError::InvalidToken)?;
        let key = self.decoding_key(&key_id).await?;
        let token = decode::<AccessClaims>(assertion, &key, &self.validation).map_err(|error| {
            if matches!(error.kind(), ErrorKind::ExpiredSignature) {
                AuthError::ExpiredToken
            } else {
                AuthError::InvalidToken
            }
        })?;

        if token.claims.token_type != "app" {
            return Err(AuthError::InvalidToken);
        }

        let email = Email::parse(&token.claims.email).map_err(|_| AuthError::InvalidToken)?;
        Ok(Identity { email })
    }
}

#[derive(Debug, Deserialize)]
struct AccessClaims {
    email: String,
    #[serde(rename = "type")]
    token_type: String,
}

#[derive(Default)]
struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    refreshed_at: Option<Instant>,
    rejected_keys: HashMap<String, Instant>,
    retry_after: Option<Instant>,
}

impl KeyCache {
    fn fresh_key(&self, key_id: &str, now: Instant, fresh_for: Duration) -> Option<DecodingKey> {
        self.refreshed_at
            .filter(|refreshed_at| now.duration_since(*refreshed_at) <= fresh_for)
            .and_then(|_| self.keys.get(key_id).cloned())
    }

    fn stale_key(&self, key_id: &str, now: Instant, stale_for: Duration) -> Option<DecodingKey> {
        self.refreshed_at
            .filter(|refreshed_at| now.duration_since(*refreshed_at) <= stale_for)
            .and_then(|_| self.keys.get(key_id).cloned())
    }

    fn rejected_key_is_fresh(&self, key_id: &str, now: Instant, reject_key_for: Duration) -> bool {
        self.rejected_keys
            .get(key_id)
            .is_some_and(|rejected_at| now.duration_since(*rejected_at) <= reject_key_for)
    }

    fn prune_rejected_keys(&mut self, now: Instant, reject_key_for: Duration) {
        self.rejected_keys
            .retain(|_, rejected_at| now.duration_since(*rejected_at) <= reject_key_for);
    }

    fn remember_rejected_key(&mut self, key_id: String, now: Instant) {
        if self.rejected_keys.len() >= MAX_REJECTED_KEY_IDS
            && let Some(oldest) = self
                .rejected_keys
                .iter()
                .min_by_key(|(_, rejected_at)| *rejected_at)
                .map(|(key_id, _)| key_id.clone())
        {
            self.rejected_keys.remove(&oldest);
        }
        self.rejected_keys.insert(key_id, now);
    }
}

#[derive(Clone, Debug, Deserialize)]
struct JwksDocument {
    keys: Vec<JwkDocument>,
}

#[derive(Clone, Debug, Deserialize)]
struct JwkDocument {
    kid: String,
    kty: String,
    alg: String,
    #[serde(rename = "use")]
    purpose: String,
    n: String,
    e: String,
}

fn parse_keys(document: JwksDocument) -> Result<HashMap<String, DecodingKey>, ()> {
    let mut keys = HashMap::new();

    for jwk in document.keys {
        if jwk.kty != "RSA" || jwk.alg != "RS256" || jwk.purpose != "sig" {
            continue;
        }
        if jwk.kid.is_empty() || jwk.kid.trim() != jwk.kid {
            return Err(());
        }

        let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e).map_err(|_| ())?;
        if keys.insert(jwk.kid, key).is_some() {
            return Err(());
        }
    }

    if keys.is_empty() { Err(()) } else { Ok(keys) }
}

#[async_trait]
trait JwksSource: Send + Sync {
    async fn fetch(&self) -> Result<JwksDocument, JwksSourceError>;
}

struct HttpJwksSource {
    client: Client,
    jwks_url: Url,
}

#[async_trait]
impl JwksSource for HttpJwksSource {
    async fn fetch(&self) -> Result<JwksDocument, JwksSourceError> {
        let response = self
            .client
            .get(self.jwks_url.clone())
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|_| JwksSourceError)?;
        response.json().await.map_err(|_| JwksSourceError)
    }
}

#[derive(Clone, Copy, Debug)]
struct JwksSourceError;

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex as StdMutex, OnceLock,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use jsonwebtoken::{EncodingKey, Header, encode, get_current_timestamp};
    use rand::rngs::OsRng;
    use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
    use serde::Serialize;

    use super::*;

    const ISSUER: &str = "https://itinera-test.cloudflareaccess.com";
    const AUDIENCE: &str = "itinera-audience";
    const EMAIL: &str = "cloud.strife@proton.me";

    struct TestKey {
        key_id: &'static str,
        encoding: EncodingKey,
        jwk: JwkDocument,
    }

    fn test_key_one() -> &'static TestKey {
        static KEY: OnceLock<TestKey> = OnceLock::new();
        KEY.get_or_init(|| make_test_key("key-one"))
    }

    fn test_key_two() -> &'static TestKey {
        static KEY: OnceLock<TestKey> = OnceLock::new();
        KEY.get_or_init(|| make_test_key("key-two"))
    }

    fn make_test_key(key_id: &'static str) -> TestKey {
        let private = RsaPrivateKey::new(&mut OsRng, 2048).expect("should generate a test key");
        let public = private.to_public_key();
        let private_der = private
            .to_pkcs1_der()
            .expect("should encode the test private key");

        TestKey {
            key_id,
            encoding: EncodingKey::from_rsa_der(private_der.as_bytes()),
            jwk: JwkDocument {
                kid: key_id.to_owned(),
                kty: "RSA".to_owned(),
                alg: "RS256".to_owned(),
                purpose: "sig".to_owned(),
                n: URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
                e: URL_SAFE_NO_PAD.encode(public.e().to_bytes_be()),
            },
        }
    }

    #[derive(Serialize)]
    struct TestClaims<'a> {
        aud: [&'a str; 1],
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<&'a str>,
        exp: u64,
        iat: u64,
        nbf: u64,
        iss: &'a str,
        #[serde(rename = "type")]
        token_type: &'a str,
        sub: &'a str,
    }

    fn valid_claims(email: Option<&str>) -> TestClaims<'_> {
        let now = get_current_timestamp();
        TestClaims {
            aud: [AUDIENCE],
            email,
            exp: now + 300,
            iat: now,
            nbf: now - 30,
            iss: ISSUER,
            token_type: "app",
            sub: "user-id",
        }
    }

    fn sign(key: &TestKey, key_id: &str, claims: &TestClaims<'_>) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(key_id.to_owned());
        encode(&header, claims, &key.encoding).expect("should sign a test token")
    }

    fn jwks(keys: &[&TestKey]) -> JwksDocument {
        JwksDocument {
            keys: keys.iter().map(|key| key.jwk.clone()).collect(),
        }
    }

    struct FakeJwksSource {
        responses: StdMutex<VecDeque<Result<JwksDocument, JwksSourceError>>>,
        calls: AtomicUsize,
    }

    impl FakeJwksSource {
        fn new(responses: Vec<Result<JwksDocument, JwksSourceError>>) -> Self {
            Self {
                responses: StdMutex::new(responses.into()),
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl JwksSource for FakeJwksSource {
        async fn fetch(&self) -> Result<JwksDocument, JwksSourceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses
                .lock()
                .expect("fake response queue should not be poisoned")
                .pop_front()
                .unwrap_or(Err(JwksSourceError))
        }
    }

    fn provider(source: Arc<dyn JwksSource>) -> CloudflareAccessIdentityProvider {
        provider_with_policy(source, DEFAULT_CACHE_POLICY)
    }

    fn provider_with_policy(
        source: Arc<dyn JwksSource>,
        policy: CachePolicy,
    ) -> CloudflareAccessIdentityProvider {
        let config = CloudflareAccessConfig::try_new(ISSUER, AUDIENCE)
            .expect("test configuration should be valid");
        CloudflareAccessIdentityProvider::from_parts(config, source, policy)
    }

    #[test]
    fn configuration_accepts_only_a_cloudflare_access_https_origin() {
        for invalid in [
            "",
            "http://team.cloudflareaccess.com",
            "https://cloudflareaccess.com",
            "https://team.example.com",
            "https://team.cloudflareaccess.com:8443",
            "https://team.cloudflareaccess.com/path",
            "https://team.cloudflareaccess.com?query=yes",
        ] {
            assert!(
                CloudflareAccessConfig::try_new(invalid, AUDIENCE).is_err(),
                "expected {invalid:?} to be rejected"
            );
        }

        assert!(CloudflareAccessConfig::try_new(ISSUER, " ").is_err());
        assert!(CloudflareAccessConfig::try_new(&format!("{ISSUER}/"), AUDIENCE).is_ok());
        assert!(CloudflareAccessIdentityProvider::new(ISSUER, AUDIENCE).is_ok());
    }

    #[test]
    fn cloudflare_jwks_json_shape_deserializes_into_a_usable_key() {
        let key = test_key_one();
        let document: JwksDocument = serde_json::from_value(serde_json::json!({
            "keys": [{
                "kid": key.key_id,
                "kty": "RSA",
                "alg": "RS256",
                "use": "sig",
                "n": key.jwk.n,
                "e": key.jwk.e,
            }],
            "public_cert": {
                "kid": key.key_id,
                "cert": "unused by the JWK verifier",
            },
        }))
        .expect("Cloudflare's JWKS response should deserialize");

        let parsed = parse_keys(document).expect("the RSA signing key should be usable");
        assert!(parsed.contains_key(key.key_id));
    }

    #[tokio::test]
    async fn a_valid_token_returns_a_canonical_identity_and_reuses_the_cached_key() {
        let source = Arc::new(FakeJwksSource::new(vec![Ok(jwks(&[test_key_one()]))]));
        let provider = provider(source.clone());
        let token = sign(
            test_key_one(),
            test_key_one().key_id,
            &valid_claims(Some("  Cloud.Strife@Proton.ME  ")),
        );

        let first = provider
            .authenticate(&token)
            .await
            .expect("the token should authenticate");
        let second = provider
            .authenticate(&token)
            .await
            .expect("the cached key should authenticate");

        assert_eq!(first.email.as_str(), EMAIL);
        assert_eq!(second, first);
        assert_eq!(source.calls(), 1);
    }

    #[tokio::test]
    async fn an_unseen_key_id_refreshes_the_jwks_for_rotation() {
        let source = Arc::new(FakeJwksSource::new(vec![
            Ok(jwks(&[test_key_one()])),
            Ok(jwks(&[test_key_one(), test_key_two()])),
        ]));
        let provider = provider(source.clone());

        provider
            .authenticate(&sign(
                test_key_one(),
                test_key_one().key_id,
                &valid_claims(Some(EMAIL)),
            ))
            .await
            .expect("the original key should authenticate");
        provider
            .authenticate(&sign(
                test_key_two(),
                test_key_two().key_id,
                &valid_claims(Some(EMAIL)),
            ))
            .await
            .expect("the rotated key should authenticate after refresh");

        assert_eq!(source.calls(), 2);
    }

    #[tokio::test]
    async fn a_missing_key_id_is_negatively_cached() {
        let source = Arc::new(FakeJwksSource::new(vec![Ok(jwks(&[test_key_one()]))]));
        let provider = provider(source.clone());
        let token = sign(
            test_key_two(),
            test_key_two().key_id,
            &valid_claims(Some(EMAIL)),
        );

        assert_eq!(
            provider.authenticate(&token).await,
            Err(AuthError::InvalidToken)
        );
        assert_eq!(
            provider.authenticate(&token).await,
            Err(AuthError::InvalidToken)
        );
        assert_eq!(source.calls(), 1);
    }

    #[tokio::test]
    async fn a_brief_jwks_outage_can_use_a_recently_cached_matching_key() {
        let source = Arc::new(FakeJwksSource::new(vec![
            Ok(jwks(&[test_key_one()])),
            Err(JwksSourceError),
        ]));
        let policy = CachePolicy {
            fresh_for: Duration::ZERO,
            ..DEFAULT_CACHE_POLICY
        };
        let provider = provider_with_policy(source.clone(), policy);
        let token = sign(
            test_key_one(),
            test_key_one().key_id,
            &valid_claims(Some(EMAIL)),
        );

        provider
            .authenticate(&token)
            .await
            .expect("the initial JWKS should authenticate");
        provider
            .authenticate(&token)
            .await
            .expect("the recent cached key should survive a brief outage");
        assert_eq!(source.calls(), 2);
    }

    #[tokio::test]
    async fn an_initial_jwks_failure_is_reported_as_provider_unavailable() {
        let source = Arc::new(FakeJwksSource::new(vec![Err(JwksSourceError)]));
        let provider = provider(source);
        let token = sign(
            test_key_one(),
            test_key_one().key_id,
            &valid_claims(Some(EMAIL)),
        );

        assert_eq!(
            provider.authenticate(&token).await,
            Err(AuthError::IdentityProviderUnavailable)
        );
    }

    #[tokio::test]
    async fn malformed_jwks_is_reported_as_provider_unavailable() {
        let mut malformed = test_key_one().jwk.clone();
        malformed.n = "not-base64url".to_owned();
        let source = Arc::new(FakeJwksSource::new(vec![Ok(JwksDocument {
            keys: vec![malformed],
        })]));
        let provider = provider(source);
        let token = sign(
            test_key_one(),
            test_key_one().key_id,
            &valid_claims(Some(EMAIL)),
        );

        assert_eq!(
            provider.authenticate(&token).await,
            Err(AuthError::IdentityProviderUnavailable)
        );
    }

    #[tokio::test]
    async fn an_expired_token_has_a_distinct_error() {
        let source = Arc::new(FakeJwksSource::new(vec![Ok(jwks(&[test_key_one()]))]));
        let provider = provider(source);
        let mut claims = valid_claims(Some(EMAIL));
        claims.exp = get_current_timestamp() - 120;
        let token = sign(test_key_one(), test_key_one().key_id, &claims);

        assert_eq!(
            provider.authenticate(&token).await,
            Err(AuthError::ExpiredToken)
        );
    }

    #[tokio::test]
    async fn issuer_audience_signature_and_not_before_are_validated() {
        let cases = [
            {
                let mut claims = valid_claims(Some(EMAIL));
                claims.iss = "https://other.cloudflareaccess.com";
                sign(test_key_one(), test_key_one().key_id, &claims)
            },
            {
                let mut claims = valid_claims(Some(EMAIL));
                claims.aud = ["another-audience"];
                sign(test_key_one(), test_key_one().key_id, &claims)
            },
            {
                let mut claims = valid_claims(Some(EMAIL));
                claims.nbf = get_current_timestamp() + 120;
                sign(test_key_one(), test_key_one().key_id, &claims)
            },
            sign(
                test_key_two(),
                test_key_one().key_id,
                &valid_claims(Some(EMAIL)),
            ),
        ];

        for token in cases {
            let source = Arc::new(FakeJwksSource::new(vec![Ok(jwks(&[test_key_one()]))]));
            let provider = provider(source);
            assert_eq!(
                provider.authenticate(&token).await,
                Err(AuthError::InvalidToken)
            );
        }
    }

    #[tokio::test]
    async fn non_identity_and_non_rs256_tokens_are_rejected() {
        let mut no_email = valid_claims(None);
        let no_email_token = sign(test_key_one(), test_key_one().key_id, &no_email);
        no_email.email = Some(EMAIL);
        no_email.token_type = "org";
        let organisation_token = sign(test_key_one(), test_key_one().key_id, &no_email);

        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(test_key_one().key_id.to_owned());
        let hmac_token = encode(
            &header,
            &valid_claims(Some(EMAIL)),
            &EncodingKey::from_secret(b"not-an-access-key"),
        )
        .expect("should sign a test HMAC token");

        for token in [no_email_token, organisation_token, hmac_token] {
            let source = Arc::new(FakeJwksSource::new(vec![Ok(jwks(&[test_key_one()]))]));
            let provider = provider(source);
            assert_eq!(
                provider.authenticate(&token).await,
                Err(AuthError::InvalidToken)
            );
        }
    }

    #[tokio::test]
    async fn malformed_headers_are_rejected_before_a_jwks_fetch() {
        let source = Arc::new(FakeJwksSource::new(vec![]));
        let provider = provider(source.clone());

        let mut missing_key_header = Header::new(Algorithm::RS256);
        missing_key_header.kid = None;
        let missing_key = encode(
            &missing_key_header,
            &valid_claims(Some(EMAIL)),
            &test_key_one().encoding,
        )
        .expect("should sign a token without a key id");

        let mut padded_key_header = Header::new(Algorithm::RS256);
        padded_key_header.kid = Some(" padded-key ".to_owned());
        let padded_key = encode(
            &padded_key_header,
            &valid_claims(Some(EMAIL)),
            &test_key_one().encoding,
        )
        .expect("should sign a token with a padded key id");

        for token in [
            String::new(),
            "x".repeat(MAX_ASSERTION_BYTES + 1),
            missing_key,
            padded_key,
        ] {
            assert_eq!(
                provider.authenticate(&token).await,
                Err(AuthError::InvalidToken)
            );
        }
        assert_eq!(source.calls(), 0);
    }
}
