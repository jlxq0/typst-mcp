//! Turning a request into a [`Principal`], or refusing it.
//!
//! One rule runs through this module: a credential is read from the `Authorization`
//! header and from nowhere else. Never a query parameter — those land in access logs,
//! browser history and `Referer` headers, and a key that reaches any of those is spent.
//! Document links get a short-lived signature instead ([`crate::signing`]).

use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use std::sync::Arc;

use crate::config::OidcConfig;
use crate::oidc::{TokenError, TokenValidator};
use crate::principal::{ApiKeys, Principal};

/// Extracted from a request by the middleware and available to handlers.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub principal: Principal,
    /// Non-reversible handle for logs and metric labels.
    pub fingerprint: String,
}

/// How a request failed to authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// No `Authorization` header at all.
    Missing,
    /// Present but not a well-formed `Bearer` credential.
    Malformed,
    /// Well-formed but not recognised.
    Rejected,
    /// This door is not configured on this deployment.
    Unavailable,
    /// The identity provider could not be reached.
    ProviderUnavailable,
}

impl AuthError {
    /// What to tell the caller.
    ///
    /// Deliberately vague about *why* a credential was rejected — distinguishing
    /// "unknown key" from "expired token" tells an attacker which guesses are closer.
    /// The `Unavailable` case is different: a caller hitting a door this deployment
    /// does not have needs to know that, and it reveals nothing about a secret.
    fn message(self) -> &'static str {
        match self {
            Self::Missing => "this endpoint requires an Authorization: Bearer credential",
            Self::Malformed => "the Authorization header must be `Bearer <credential>`",
            Self::Rejected => "the credential was not accepted",
            Self::Unavailable => "this endpoint is not configured on this server",
            Self::ProviderUnavailable => {
                "the identity provider could not be reached; this is a server-side \
                 problem, so retry rather than re-authenticating"
            }
        }
    }

    /// The status to answer with.
    ///
    /// A provider we cannot reach is a 503: answering 401 would send a caller off to
    /// re-authenticate against a problem that is not theirs.
    pub fn status(self) -> StatusCode {
        match self {
            Self::ProviderUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::UNAUTHORIZED,
        }
    }
}

/// Pull the bearer credential out of a header map.
///
/// The scheme is matched case-insensitively per RFC 7235, but the credential itself is
/// taken verbatim: trimming or unquoting it would let two different strings authenticate
/// as the same key.
pub fn bearer(headers: &HeaderMap) -> Result<&str, AuthError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .ok_or(AuthError::Missing)?
        .to_str()
        .map_err(|_| AuthError::Malformed)?;

    let (scheme, credential) = value.split_once(' ').ok_or(AuthError::Malformed)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(AuthError::Malformed);
    }
    let credential = credential.trim_start();
    if credential.is_empty() {
        return Err(AuthError::Malformed);
    }
    Ok(credential)
}

/// The static-key door, used by `/api/v1`.
#[derive(Clone)]
pub struct ApiKeyAuth {
    keys: ApiKeys,
    challenge: String,
}

impl ApiKeyAuth {
    pub fn new(keys: ApiKeys) -> Self {
        Self {
            keys,
            challenge: "Bearer".to_owned(),
        }
    }

    pub fn authenticate(&self, headers: &HeaderMap) -> Result<Authenticated, AuthError> {
        if self.keys.is_empty() {
            return Err(AuthError::Unavailable);
        }
        let presented = bearer(headers)?;
        let key = self
            .keys
            .authenticate(presented)
            .ok_or(AuthError::Rejected)?;
        Ok(Authenticated {
            principal: key.principal(),
            fingerprint: key.fingerprint().to_owned(),
        })
    }

    /// The `WWW-Authenticate` value to send with a 401.
    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

/// The OIDC door, used by `/mcp`.
///
/// Carries the RFC 9728 metadata URL so a 401 can point a client at discovery — the
/// one thing the MCP authorization spec requires a server to implement.
#[derive(Clone)]
pub struct OidcAuth {
    config: Option<OidcConfig>,
    validator: Option<Arc<TokenValidator>>,
    challenge: String,
}

impl OidcAuth {
    pub fn new(config: Option<OidcConfig>, metadata_url: &str) -> Self {
        let challenge = match &config {
            Some(cfg) => format!(
                "Bearer resource_metadata=\"{metadata_url}\", scope=\"{}\"",
                cfg.scope
            ),
            None => "Bearer".to_owned(),
        };
        let validator = config.clone().map(TokenValidator::new);
        Self {
            config,
            validator,
            challenge,
        }
    }

    /// Validate the request's bearer token.
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Authenticated, AuthError> {
        let Some(validator) = &self.validator else {
            return Err(AuthError::Unavailable);
        };
        let token = bearer(headers)?;
        match validator.validate(token).await {
            Ok(principal) => Ok(Authenticated {
                fingerprint: crate::principal::fingerprint(principal.display()),
                principal,
            }),
            // A provider we cannot reach is our failure, not a bad credential — telling
            // the caller their token was rejected would send them to re-authenticate
            // for no reason.
            Err(TokenError::ProviderUnavailable) => Err(AuthError::ProviderUnavailable),
            Err(TokenError::Malformed) => Err(AuthError::Malformed),
            Err(_) => Err(AuthError::Rejected),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.config.is_some()
    }

    pub fn config(&self) -> Option<&OidcConfig> {
        self.config.as_ref()
    }

    /// The `WWW-Authenticate` value to send with a 401.
    ///
    /// Carries `resource_metadata` so a client can discover the authorization server,
    /// which is what the MCP authorization spec requires of a protected resource.
    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

/// A 401 with a `WWW-Authenticate` challenge.
pub fn unauthorized(error: AuthError, challenge: &str) -> Response {
    let body = serde_json::json!({
        "error": "unauthorized",
        "message": error.message(),
    });
    (
        error.status(),
        [(header::WWW_AUTHENTICATE, challenge)],
        axum::Json(body),
    )
        .into_response()
}

/// Middleware for `/api/v1`: static key required.
pub async fn require_api_key(
    axum::extract::State(auth): axum::extract::State<ApiKeyAuth>,
    mut request: Request,
    next: Next,
) -> Response {
    match auth.authenticate(request.headers()) {
        Ok(authenticated) => {
            request.extensions_mut().insert(authenticated);
            next.run(request).await
        }
        Err(error) => unauthorized(error, auth.challenge()),
    }
}

/// Middleware for `/mcp`: an OIDC token required.
pub async fn require_oidc(
    axum::extract::State(auth): axum::extract::State<OidcAuth>,
    mut request: Request,
    next: Next,
) -> Response {
    match auth.authenticate(request.headers()).await {
        Ok(authenticated) => {
            request.extensions_mut().insert(authenticated);
            next.run(request).await
        }
        Err(error) => unauthorized(error, auth.challenge()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert(header::AUTHORIZATION, value.parse().expect("header"));
        }
        headers
    }

    fn auth() -> ApiKeyAuth {
        ApiKeyAuth::new(ApiKeys::parse("alice:sk_alice,bob:sk_bob"))
    }

    #[test]
    fn a_valid_key_authenticates_as_its_label() {
        let result = auth()
            .authenticate(&headers(Some("Bearer sk_bob")))
            .expect("accepted");
        assert_eq!(
            result.principal,
            Principal::Service {
                label: "bob".into()
            }
        );
        assert_eq!(result.fingerprint.len(), 16);
    }

    #[test]
    fn the_scheme_is_case_insensitive() {
        // RFC 7235 says the scheme is case-insensitive, and clients do vary.
        for value in ["Bearer sk_bob", "bearer sk_bob", "BEARER sk_bob"] {
            assert!(
                auth().authenticate(&headers(Some(value))).is_ok(),
                "{value}"
            );
        }
    }

    #[test]
    fn a_missing_or_malformed_header_is_distinguished_from_a_bad_key() {
        let auth = auth();
        assert_eq!(
            auth.authenticate(&headers(None)).unwrap_err(),
            AuthError::Missing
        );
        for value in ["sk_bob", "Basic sk_bob", "Bearer", "Bearer   "] {
            assert_eq!(
                auth.authenticate(&headers(Some(value))).unwrap_err(),
                AuthError::Malformed,
                "{value:?}"
            );
        }
        assert_eq!(
            auth.authenticate(&headers(Some("Bearer nope")))
                .unwrap_err(),
            AuthError::Rejected
        );
    }

    #[test]
    fn the_credential_is_taken_verbatim() {
        // Trimming the credential would let "sk_bob " and "sk_bob" both authenticate,
        // which quietly widens what counts as the key.
        let auth = auth();
        assert!(auth.authenticate(&headers(Some("Bearer sk_bob "))).is_err());
        // Only the separator between scheme and credential is flexible.
        assert!(auth.authenticate(&headers(Some("Bearer  sk_bob"))).is_ok());
    }

    #[test]
    fn no_configured_keys_means_the_door_is_closed() {
        // Not "everything authenticates" — an empty key list must reject everything.
        let auth = ApiKeyAuth::new(ApiKeys::parse(""));
        assert_eq!(
            auth.authenticate(&headers(Some("Bearer anything")))
                .unwrap_err(),
            AuthError::Unavailable
        );
        assert_eq!(
            auth.authenticate(&headers(None)).unwrap_err(),
            AuthError::Unavailable
        );
    }

    #[test]
    fn rejection_messages_do_not_explain_why() {
        // "unknown key" vs "expired token" tells an attacker which guesses are warmer.
        let message = AuthError::Rejected.message();
        for leak in ["unknown", "expired", "not found", "invalid signature"] {
            assert!(!message.contains(leak), "{message:?} leaks {leak:?}");
        }
    }

    #[test]
    fn the_oidc_challenge_points_at_discovery() {
        // The one thing the MCP authorization spec requires of a server: a 401 that
        // tells the client where the protected-resource metadata is.
        let auth = OidcAuth::new(
            Some(OidcConfig {
                issuer: "https://login.microsoftonline.com/abc/v2.0".into(),
                tenant_id: Some("abc".into()),
                audience: "api://typst-mcp".into(),
                scope: "render".into(),
            }),
            "https://typst.example.com/.well-known/oauth-protected-resource/mcp",
        );
        let challenge = auth.challenge();
        assert!(challenge.starts_with("Bearer "), "{challenge}");
        assert!(challenge.contains("resource_metadata="), "{challenge}");
        assert!(
            challenge
                .contains("https://typst.example.com/.well-known/oauth-protected-resource/mcp"),
            "{challenge}"
        );
        assert!(challenge.contains("scope=\"render\""), "{challenge}");
    }

    #[test]
    fn an_unconfigured_oidc_door_reports_itself_as_such() {
        let auth = OidcAuth::new(None, "https://typst.example.com/.well-known/x");
        assert!(!auth.is_configured());
        assert_eq!(auth.challenge(), "Bearer");
    }
}
