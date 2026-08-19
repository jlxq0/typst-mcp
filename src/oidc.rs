//! Validating OIDC access tokens, for the MCP endpoint.
//!
//! Written against Microsoft Entra but provider-agnostic: everything specific is
//! configuration. Discovery and JWKS are fetched at runtime and cached, with a forced
//! refresh on an unknown key id so a provider's key rotation does not become an outage.
//!
//! Four claims are checked, and each one closes a different door:
//!
//! * `iss` — the token came from the provider we configured, not another one.
//! * `tid` — it came from *our* directory. Without this, a token minted in any other
//!   Entra tenant validates against the shared `login.microsoftonline.com` issuer.
//! * `aud` — it was minted for *this* application, not another one the user can reach.
//!   Skipping this is how a token for an unrelated app becomes a credential here.
//! * `scp` / `roles` — it carries the permission this endpoint requires.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config::OidcConfig;
use crate::principal::Principal;

/// How long a fetched key set is trusted before being refreshed.
const JWKS_TTL: Duration = Duration::from_secs(60 * 60);

/// No attacker-controlled key id may force more than one refresh in this window.
const JWKS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// Provider failures are shared across requests instead of retried in parallel.
const FETCH_FAILURE_BACKOFF: Duration = Duration::from_secs(5);

/// Recently absent key ids are remembered briefly and in bounded space.
const UNKNOWN_KID_TTL: Duration = Duration::from_secs(30);
const UNKNOWN_KID_CAP: usize = 1024;
const MAX_KID_BYTES: usize = 256;

/// Cap on a discovery or JWKS fetch.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Tolerance for clock skew between us and the provider, in seconds.
///
/// Set explicitly rather than inheriting jsonwebtoken's default (also 60), so the
/// tolerance is a decision here and a dependency bump cannot quietly widen it. Zero
/// would be stricter but would reject perfectly good tokens whenever the two clocks
/// disagree by a second, which they routinely do.
const CLOCK_SKEW_SECONDS: u64 = 60;

/// Why a token was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// Not a JWT at all, or unreadable.
    Malformed,
    /// Signed by a key we could not obtain.
    UnknownKey,
    /// Signature, expiry, issuer, audience, tenant or scope failed.
    Rejected,
    /// The provider could not be reached. Distinct from a rejection: this is our
    /// problem, not the caller's, and it should not read as a bad credential.
    ProviderUnavailable,
}

/// Claims we care about. Everything else in the token is ignored.
#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    #[serde(default)]
    aud: Audience,
    /// Entra directory id.
    #[serde(default)]
    tid: Option<String>,
    /// Delegated scopes, space-separated.
    #[serde(default)]
    scp: Option<String>,
    /// Application roles, for client-credentials tokens.
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    name: Option<String>,
}

/// `aud` is a string or an array depending on the provider and flow.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
    #[default]
    None,
}

impl Audience {
    fn contains(&self, wanted: &str) -> bool {
        match self {
            Self::One(value) => value == wanted,
            Self::Many(values) => values.iter().any(|v| v == wanted),
            Self::None => false,
        }
    }
}

impl std::fmt::Display for Audience {
    /// For the rejection log line. An `aud` claim names an application; it is not a
    /// secret, and printing it is the difference between a five-minute fix and an
    /// afternoon of archaeology.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::One(value) => f.write_str(value),
            Self::Many(values) => f.write_str(&values.join(",")),
            Self::None => f.write_str("<absent>"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: String,
    #[serde(default)]
    kty: String,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Discovery {
    jwks_uri: String,
    issuer: String,
}

struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Instant,
}

#[derive(Default)]
struct KeyState {
    cache: Option<KeyCache>,
    unknown_kids: HashMap<String, Instant>,
    last_failure: Option<Instant>,
}

/// Validates access tokens against a configured provider.
pub struct TokenValidator {
    config: OidcConfig,
    http: reqwest::Client,
    /// Short-lived access to cached keys and refresh bookkeeping. Never held across
    /// provider I/O, so a refresh cannot stall a token whose key is already cached.
    cache: Mutex<KeyState>,
    /// Single-flight ownership for discovery/JWKS refreshes. A waiter re-checks the
    /// cache after acquiring it because the preceding owner may have satisfied it.
    refresh: Mutex<()>,
}

impl TokenValidator {
    pub fn new(config: OidcConfig) -> Arc<Self> {
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .build()
            .unwrap_or_default();
        Arc::new(Self {
            config,
            http,
            cache: Mutex::new(KeyState::default()),
            refresh: Mutex::new(()),
        })
    }

    /// Validate `token` and return who it belongs to.
    pub async fn validate(&self, token: &str) -> Result<Principal, TokenError> {
        let header = decode_header(token).map_err(|_| TokenError::Malformed)?;
        // Only RSA: accepting whatever the token names would let an attacker choose a
        // weaker algorithm, and `none` is the classic version of that attack. Check
        // this before key lookup so a token we will never accept cannot cause I/O.
        if !matches!(
            header.alg,
            Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512
        ) {
            return Err(TokenError::Rejected);
        }
        let kid = header.kid.ok_or(TokenError::Malformed)?;
        if kid.len() > MAX_KID_BYTES {
            return Err(TokenError::Malformed);
        }

        let key = self.key(&kid).await?.ok_or(TokenError::UnknownKey)?;

        let mut validation = Validation::new(header.alg);
        let mut audiences = vec![self.config.audience.clone()];
        audiences.extend(self.config.extra_audiences.iter().cloned());
        validation.set_audience(&audiences);
        validation.set_issuer(&[&self.config.issuer]);
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.leeway = CLOCK_SKEW_SECONDS;

        let data = decode::<Claims>(token, &key, &validation).map_err(|_| TokenError::Rejected)?;
        let claims = data.claims;

        // jsonwebtoken checks `iss` and `aud`, but re-checking here means the guarantee
        // does not depend on a validation flag staying set through a refactor.
        let aud_ok = claims.aud.contains(&self.config.audience)
            || self
                .config
                .extra_audiences
                .iter()
                .any(|a| claims.aud.contains(a));
        // Log *which* check failed, with the claim values. A token is a credential and
        // is never logged; `aud`/`iss`/`tid`/`scp` are identifiers and scope names, and
        // without them a rejection is indistinguishable from a broken client. The 401
        // the caller gets stays deliberately vague — this line is for the operator.
        if claims.iss != self.config.issuer || !aud_ok {
            tracing::warn!(
                reason = if aud_ok { "issuer" } else { "audience" },
                token_aud = %claims.aud,
                token_iss = %claims.iss,
                expected_aud = %self.config.audience,
                also_accepted = ?self.config.extra_audiences,
                expected_iss = %self.config.issuer,
                "rejected a token"
            );
            return Err(TokenError::Rejected);
        }

        if let Some(expected) = &self.config.tenant_id
            && claims.tid.as_deref() != Some(expected.as_str())
        {
            tracing::warn!(
                reason = "tenant",
                token_tid = ?claims.tid,
                expected_tid = %expected,
                "rejected a token"
            );
            return Err(TokenError::Rejected);
        }

        if !self.has_scope(&claims) {
            tracing::warn!(
                reason = "scope",
                token_scp = ?claims.scp,
                token_roles = ?claims.roles,
                expected_scope = %self.config.scope,
                "rejected a token"
            );
            return Err(TokenError::Rejected);
        }

        Ok(Principal::User {
            subject: claims.sub,
            issuer: claims.iss,
            name: claims.name,
        })
    }

    /// Whether the token carries the required permission.
    ///
    /// Delegated tokens use `scp`, application tokens use `roles`; a deployment may
    /// legitimately use either, so both are accepted.
    fn has_scope(&self, claims: &Claims) -> bool {
        let wanted = &self.config.scope;
        // Entra prefixes delegated scopes with the App ID URI in some configurations,
        // so match either the bare name or the qualified form.
        let matches = |value: &str| {
            value == wanted || value.rsplit('/').next().is_some_and(|tail| tail == wanted)
        };
        claims
            .scp
            .as_deref()
            .is_some_and(|scp| scp.split_whitespace().any(matches))
            || claims.roles.iter().any(|r| matches(r))
    }

    /// Look up a signing key, refreshing at most once per interval for an unknown id.
    async fn key(&self, kid: &str) -> Result<Option<DecodingKey>, TokenError> {
        {
            let mut state = self.cache.lock().await;
            if let Some(result) = inspect_key_state(&mut state, kid, Instant::now()) {
                return result;
            }
        }

        // Only refresh ownership spans network I/O. Cache access stays independent so
        // known keys continue authenticating during a slow or unavailable provider.
        let _refresh = self.refresh.lock().await;

        // Another request may have refreshed while this one waited for ownership.
        {
            let mut state = self.cache.lock().await;
            if let Some(result) = inspect_key_state(&mut state, kid, Instant::now()) {
                return result;
            }
        }

        let fetched = self.fetch_keys().await;
        let mut state = self.cache.lock().await;
        match fetched {
            Ok(keys) => {
                let found = keys.get(kid).cloned();
                state.cache = Some(KeyCache {
                    keys,
                    fetched_at: Instant::now(),
                });
                state.last_failure = None;
                if found.is_none() {
                    remember_unknown(&mut state, kid, Instant::now());
                }
                Ok(found)
            }
            Err(err) => {
                state.last_failure = Some(Instant::now());
                Err(err)
            }
        }
    }

    async fn fetch_keys(&self) -> Result<HashMap<String, DecodingKey>, TokenError> {
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            self.config.issuer.trim_end_matches('/')
        );
        let discovery: Discovery = self
            .http
            .get(&discovery_url)
            .send()
            .await
            .map_err(|_| TokenError::ProviderUnavailable)?
            .json()
            .await
            .map_err(|_| TokenError::ProviderUnavailable)?;

        // The discovery document is fetched over TLS from the configured issuer, but
        // checking that it agrees about its own identity costs nothing and catches a
        // misconfigured issuer URL before it becomes a confusing signature failure.
        if discovery.issuer.trim_end_matches('/') != self.config.issuer.trim_end_matches('/') {
            return Err(TokenError::ProviderUnavailable);
        }

        let jwks: Jwks = self
            .http
            .get(&discovery.jwks_uri)
            .send()
            .await
            .map_err(|_| TokenError::ProviderUnavailable)?
            .json()
            .await
            .map_err(|_| TokenError::ProviderUnavailable)?;

        Ok(jwks
            .keys
            .into_iter()
            .filter(|k| k.kty == "RSA")
            .filter_map(|k| {
                let (n, e) = (k.n.as_deref()?, k.e.as_deref()?);
                DecodingKey::from_rsa_components(n, e)
                    .ok()
                    .map(|key| (k.kid, key))
            })
            .collect())
    }
}

/// Return a cached answer, or `None` when this caller should attempt a refresh.
fn inspect_key_state(
    state: &mut KeyState,
    kid: &str,
    now: Instant,
) -> Option<Result<Option<DecodingKey>, TokenError>> {
    state
        .unknown_kids
        .retain(|_, seen| now.duration_since(*seen) < UNKNOWN_KID_TTL);
    if state.unknown_kids.contains_key(kid) {
        return Some(Ok(None));
    }

    if let Some(cache) = state.cache.as_ref()
        && cache.fetched_at.elapsed() < JWKS_TTL
    {
        if let Some(key) = cache.keys.get(kid) {
            return Some(Ok(Some(key.clone())));
        }
        if cache.fetched_at.elapsed() < JWKS_REFRESH_INTERVAL {
            remember_unknown(state, kid, now);
            return Some(Ok(None));
        }
    }

    if state
        .last_failure
        .is_some_and(|failed| failed.elapsed() < FETCH_FAILURE_BACKOFF)
    {
        return Some(Err(TokenError::ProviderUnavailable));
    }

    None
}

fn remember_unknown(state: &mut KeyState, kid: &str, now: Instant) {
    if state.unknown_kids.len() < UNKNOWN_KID_CAP || state.unknown_kids.contains_key(kid) {
        state.unknown_kids.insert(kid.to_owned(), now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::{Json, routing::get};
    use tokio::sync::Notify;

    fn config() -> OidcConfig {
        OidcConfig {
            issuer: "https://login.microsoftonline.com/abc/v2.0".into(),
            tenant_id: Some("abc".into()),
            audience: "api://typst-mcp".into(),
            scope: "render".into(),
            extra_audiences: vec![],
        }
    }

    fn claims(scp: Option<&str>, roles: &[&str]) -> Claims {
        Claims {
            sub: "user-1".into(),
            iss: config().issuer,
            aud: Audience::One("api://typst-mcp".into()),
            tid: Some("abc".into()),
            scp: scp.map(str::to_owned),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            name: None,
        }
    }

    fn validator() -> Arc<TokenValidator> {
        TokenValidator::new(config())
    }

    fn token_with_header(alg: &str, kid: &str) -> String {
        let header = serde_json::json!({ "alg": alg, "typ": "JWT", "kid": kid });
        format!(
            "{}.{}.eA",
            base64_url(header.to_string().as_bytes()),
            base64_url(br#"{"sub":"attacker"}"#)
        )
    }

    async fn counting_validator() -> (Arc<TokenValidator>, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let issuer = format!("http://{}", listener.local_addr().expect("addr"));
        let requests = Arc::new(AtomicUsize::new(0));
        let discovery_issuer = issuer.clone();
        let discovery_requests = Arc::clone(&requests);
        let key_requests = Arc::clone(&requests);
        let app = axum::Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || {
                    let issuer = discovery_issuer.clone();
                    let requests = Arc::clone(&discovery_requests);
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!({
                            "issuer": issuer,
                            "jwks_uri": format!("{issuer}/keys"),
                        }))
                    }
                }),
            )
            .route(
                "/keys",
                get(move || {
                    let requests = Arc::clone(&key_requests);
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!({ "keys": [] }))
                    }
                }),
            );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut oidc = config();
        oidc.issuer = issuer;
        (TokenValidator::new(oidc), requests)
    }

    async fn failing_validator() -> (Arc<TokenValidator>, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let issuer = format!("http://{}", listener.local_addr().expect("addr"));
        let requests = Arc::new(AtomicUsize::new(0));
        let handler_requests = Arc::clone(&requests);
        let app = axum::Router::new().route(
            "/.well-known/openid-configuration",
            get(move || {
                let requests = Arc::clone(&handler_requests);
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    (axum::http::StatusCode::SERVICE_UNAVAILABLE, "unavailable")
                }
            }),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut oidc = config();
        oidc.issuer = issuer;
        (TokenValidator::new(oidc), requests)
    }

    async fn blocking_validator() -> (Arc<TokenValidator>, Arc<Notify>, Arc<Notify>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let issuer = format!("http://{}", listener.local_addr().expect("addr"));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler_started = Arc::clone(&started);
        let handler_release = Arc::clone(&release);
        let discovery_issuer = issuer.clone();
        let app = axum::Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || {
                    let started = Arc::clone(&handler_started);
                    let release = Arc::clone(&handler_release);
                    let issuer = discovery_issuer.clone();
                    async move {
                        started.notify_one();
                        release.notified().await;
                        Json(serde_json::json!({
                            "issuer": issuer,
                            "jwks_uri": format!("{issuer}/keys"),
                        }))
                    }
                }),
            )
            .route(
                "/keys",
                get(|| async { Json(serde_json::json!({ "keys": [] })) }),
            );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut oidc = config();
        oidc.issuer = issuer;
        (TokenValidator::new(oidc), started, release)
    }

    #[test]
    fn audience_accepts_both_wire_forms() {
        // Providers differ, and a flow can change which one you get.
        assert!(Audience::One("api://typst-mcp".into()).contains("api://typst-mcp"));
        assert!(
            Audience::Many(vec!["other".into(), "api://typst-mcp".into()])
                .contains("api://typst-mcp")
        );
        assert!(!Audience::Many(vec!["other".into()]).contains("api://typst-mcp"));
        assert!(!Audience::None.contains("api://typst-mcp"));
        // Substrings must not match, or `api://typst-mcp-staging` would authenticate.
        assert!(!Audience::One("api://typst-mcp-staging".into()).contains("api://typst-mcp"));
    }

    #[test]
    fn a_delegated_scope_is_accepted_bare_or_qualified() {
        let v = validator();
        assert!(v.has_scope(&claims(Some("render"), &[])));
        assert!(v.has_scope(&claims(Some("api://typst-mcp/render"), &[])));
        // Space-separated lists are the normal shape.
        assert!(v.has_scope(&claims(Some("openid profile render"), &[])));
    }

    #[test]
    fn an_application_role_is_accepted() {
        // Client-credentials tokens carry `roles` rather than `scp`.
        assert!(validator().has_scope(&claims(None, &["render"])));
    }

    #[test]
    fn a_token_without_the_scope_is_refused() {
        let v = validator();
        assert!(!v.has_scope(&claims(None, &[])));
        assert!(!v.has_scope(&claims(Some("openid profile"), &[])));
        assert!(!v.has_scope(&claims(Some("renderer"), &[])));
        // A scope for a different resource must not satisfy ours.
        assert!(!v.has_scope(&claims(Some("api://other/write"), &[])));
    }

    #[tokio::test]
    async fn a_malformed_token_is_refused_without_contacting_the_provider() {
        // No network here: garbage must be rejected on inspection.
        let v = validator();
        for token in ["", "not-a-jwt", "a.b", "a.b.c"] {
            assert_eq!(
                v.validate(token).await.unwrap_err(),
                TokenError::Malformed,
                "{token:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_token_with_no_key_id_is_refused() {
        // `alg: none` with no `kid` is the classic forged-token shape.
        let header = base64_url(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = base64_url(br#"{"sub":"attacker"}"#);
        let token = format!("{header}.{payload}.");
        assert_eq!(
            validator().validate(&token).await.unwrap_err(),
            TokenError::Malformed
        );
    }

    #[tokio::test]
    async fn a_disallowed_algorithm_is_rejected_before_network_work() {
        let token = token_with_header("HS256", "attacker-key");
        let mut oidc = config();
        oidc.issuer = "http://127.0.0.1:9".to_owned();
        assert_eq!(
            TokenValidator::new(oidc)
                .validate(&token)
                .await
                .unwrap_err(),
            TokenError::Rejected
        );
    }

    #[tokio::test]
    async fn concurrent_unknown_keys_share_one_fetch_and_are_negative_cached() {
        let (validator, requests) = counting_validator().await;
        let token = token_with_header("RS256", "unknown-key");
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let validator = Arc::clone(&validator);
            let token = token.clone();
            tasks.push(tokio::spawn(
                async move { validator.validate(&token).await },
            ));
        }
        for task in tasks {
            assert_eq!(
                task.await.expect("join").unwrap_err(),
                TokenError::UnknownKey
            );
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);

        assert_eq!(
            validator.validate(&token).await.unwrap_err(),
            TokenError::UnknownKey
        );
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_provider_failures_are_single_flight_and_throttled() {
        let (validator, requests) = failing_validator().await;
        let token = token_with_header("RS256", "unknown-key");
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let validator = Arc::clone(&validator);
            let token = token.clone();
            tasks.push(tokio::spawn(
                async move { validator.validate(&token).await },
            ));
        }
        for task in tasks {
            assert_eq!(
                task.await.expect("join").unwrap_err(),
                TokenError::ProviderUnavailable
            );
        }
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        assert_eq!(
            validator.validate(&token).await.unwrap_err(),
            TokenError::ProviderUnavailable
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_refresh_does_not_block_an_already_cached_key() {
        let (validator, started, release) = blocking_validator().await;
        {
            let mut state = validator.cache.lock().await;
            state.cache = Some(KeyCache {
                keys: HashMap::from([(
                    "known-key".to_owned(),
                    DecodingKey::from_secret(b"test-only"),
                )]),
                // Old enough that an unknown id is allowed to start a refresh, but
                // young enough that a known key remains usable.
                fetched_at: Instant::now() - JWKS_REFRESH_INTERVAL,
            });
        }

        let refreshing = {
            let validator = Arc::clone(&validator);
            tokio::spawn(async move { validator.key("unknown-key").await })
        };
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("refresh started");

        let known = tokio::time::timeout(Duration::from_millis(100), validator.key("known-key"))
            .await
            .expect("cached key must not wait for provider I/O")
            .expect("lookup");
        assert!(known.is_some());

        release.notify_one();
        assert!(refreshing.await.expect("join").expect("refresh").is_none());
    }

    #[test]
    fn unknown_key_cache_is_bounded() {
        let mut state = KeyState::default();
        let now = Instant::now();
        for n in 0..=UNKNOWN_KID_CAP {
            remember_unknown(&mut state, &format!("unknown-{n}"), now);
        }
        assert_eq!(state.unknown_kids.len(), UNKNOWN_KID_CAP);
    }

    fn base64_url(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }
}
