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
use tokio::sync::RwLock;

use crate::config::OidcConfig;
use crate::principal::Principal;

/// How long a fetched key set is trusted before being refreshed.
const JWKS_TTL: Duration = Duration::from_secs(60 * 60);

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

/// Validates access tokens against a configured provider.
pub struct TokenValidator {
    config: OidcConfig,
    http: reqwest::Client,
    cache: RwLock<Option<KeyCache>>,
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
            cache: RwLock::new(None),
        })
    }

    /// Validate `token` and return who it belongs to.
    pub async fn validate(&self, token: &str) -> Result<Principal, TokenError> {
        let header = decode_header(token).map_err(|_| TokenError::Malformed)?;
        let kid = header.kid.ok_or(TokenError::Malformed)?;

        let key = match self.key(&kid, false).await? {
            Some(key) => key,
            // An unknown key id is the normal appearance of a provider rotating its
            // signing keys. Refetching once turns what would be an outage into a
            // single slow request.
            None => self.key(&kid, true).await?.ok_or(TokenError::UnknownKey)?,
        };

        let mut validation = Validation::new(header.alg);
        // Only RSA: accepting whatever the token names would let an attacker choose a
        // weaker algorithm, and `none` is the classic version of that attack.
        if !matches!(
            header.alg,
            Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512
        ) {
            return Err(TokenError::Rejected);
        }
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
        if claims.iss != self.config.issuer || !aud_ok {
            return Err(TokenError::Rejected);
        }

        if let Some(expected) = &self.config.tenant_id
            && claims.tid.as_deref() != Some(expected.as_str())
        {
            return Err(TokenError::Rejected);
        }

        if !self.has_scope(&claims) {
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

    /// Look up a signing key, optionally forcing a refresh first.
    async fn key(&self, kid: &str, force: bool) -> Result<Option<DecodingKey>, TokenError> {
        if !force {
            let cache = self.cache.read().await;
            if let Some(cache) = cache.as_ref()
                && cache.fetched_at.elapsed() < JWKS_TTL
            {
                return Ok(cache.keys.get(kid).cloned());
            }
        }

        let keys = self.fetch_keys().await?;
        let found = keys.get(kid).cloned();
        *self.cache.write().await = Some(KeyCache {
            keys,
            fetched_at: Instant::now(),
        });
        Ok(found)
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

#[cfg(test)]
mod tests {
    use super::*;

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

    fn base64_url(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }
}
