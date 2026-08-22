//! Transparent OAuth 2.0 proxy fronting Microsoft Entra.
//!
//! Claude's connector requires the authorization server's `authorize` and
//! `token` endpoints to be **same-origin as the issuer**. Entra lives on
//! login.microsoftonline.com, so we proxy on our origin and broker to Entra.
//! Entra only ever sees our `/oauth/callback`; client redirect URIs (including
//! private-use schemes) are stored here and restored on the way back.
//!
//! Entra v2 expresses the API audience through `scope` (`api://typst-mcp/render`),
//! not Logto's `/auth` + origin `resource`. We qualify a bare `render` scope and
//! pass Claude's RFC 8707 `resource` through when it matches our MCP URL.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use serde::{Deserialize, Serialize};
use tracing::warn;
use url::Url;

use crate::oauth_metadata::{entra_authorize_url, entra_token_url, qualified_api_scope};
use crate::oauth_redirect::is_allowed_redirect_uri;

#[allow(clippy::duration_suboptimal_units)]
const PENDING_TTL: Duration = Duration::from_secs(600);
const MAX_CLIENT_STATE_BYTES: usize = 4096;
const MAX_UPSTREAM_CODE_BYTES: usize = 8192;

#[derive(Clone)]
pub struct OAuthProxyState {
    inner: Arc<Inner>,
}

pub struct OAuthClientConfig {
    pub client_id: String,
    pub allowed_redirect_uris: Vec<String>,
    pub state_key: Vec<u8>,
}

struct Inner {
    entra_authorize: String,
    entra_token: String,
    callback_url: String,
    mcp_resource: String,
    api_scope: String,
    client_id: String,
    http: reqwest::Client,
    allowed_redirect_uris: Vec<String>,
    state_key: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    client_redirect_uri: String,
    client_state: Option<String>,
    code_challenge: String,
    issued_at: u64,
}

#[derive(Serialize, Deserialize)]
struct PendingCode {
    upstream_code: String,
    client_redirect_uri: String,
    code_challenge: String,
    issued_at: u64,
}

impl OAuthProxyState {
    pub fn new(
        issuer: &str,
        public_url: &str,
        mcp_resource: &str,
        audience: &str,
        api_scope: &str,
        client: OAuthClientConfig,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("typst-mcp/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                entra_authorize: entra_authorize_url(issuer),
                entra_token: entra_token_url(issuer),
                callback_url: format!("{}/oauth/callback", public_url.trim_end_matches('/')),
                mcp_resource: mcp_resource.to_owned(),
                api_scope: qualified_api_scope(audience, api_scope),
                client_id: client.client_id,
                http,
                allowed_redirect_uris: client.allowed_redirect_uris,
                state_key: client.state_key,
            }),
        }
    }

    fn encode_state(&self, pending: &Pending) -> Option<String> {
        use hmac::Mac as _;
        let payload = serde_json::to_vec(pending).ok()?;
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&self.inner.state_key).ok()?;
        mac.update(b"oauth-state\0");
        mac.update(&payload);
        let signature = mac.finalize().into_bytes();
        Some(format!(
            "{}.{}",
            BASE64URL.encode(payload),
            BASE64URL.encode(signature)
        ))
    }

    fn decode_state(&self, state: &str) -> Option<Pending> {
        use hmac::Mac as _;
        let (payload, signature) = state.split_once('.')?;
        let payload = BASE64URL.decode(payload).ok()?;
        let signature = BASE64URL.decode(signature).ok()?;
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&self.inner.state_key).ok()?;
        mac.update(b"oauth-state\0");
        mac.update(&payload);
        mac.verify_slice(&signature).ok()?;
        let pending: Pending = serde_json::from_slice(&payload).ok()?;
        let now = now_unix();
        if pending.issued_at > now.saturating_add(60)
            || now.saturating_sub(pending.issued_at) >= PENDING_TTL.as_secs()
        {
            return None;
        }
        Some(pending)
    }

    fn encode_code(&self, pending: &PendingCode) -> Option<String> {
        use hmac::Mac as _;
        let payload = serde_json::to_vec(pending).ok()?;
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&self.inner.state_key).ok()?;
        mac.update(b"oauth-code\0");
        mac.update(&payload);
        let signature = mac.finalize().into_bytes();
        Some(format!(
            "{}.{}",
            BASE64URL.encode(payload),
            BASE64URL.encode(signature)
        ))
    }

    fn decode_code(&self, code: &str) -> Option<PendingCode> {
        use hmac::Mac as _;
        let (payload, signature) = code.split_once('.')?;
        let payload = BASE64URL.decode(payload).ok()?;
        let signature = BASE64URL.decode(signature).ok()?;
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&self.inner.state_key).ok()?;
        mac.update(b"oauth-code\0");
        mac.update(&payload);
        mac.verify_slice(&signature).ok()?;
        let pending: PendingCode = serde_json::from_slice(&payload).ok()?;
        let now = now_unix();
        if pending.issued_at > now.saturating_add(60)
            || now.saturating_sub(pending.issued_at) >= PENDING_TTL.as_secs()
        {
            return None;
        }
        Some(pending)
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_pairs(q: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(q.as_bytes())
        .into_owned()
        .collect()
}

fn exactly_one<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let mut values = pairs
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.as_str());
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn valid_code_challenge(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_code_verifier(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

fn challenge_for(verifier: &str) -> String {
    use sha2::Digest as _;
    BASE64URL.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// Qualify Entra v2 scopes: bare `render` becomes `api://typst-mcp/render`,
/// and we always ask for `openid` + `offline_access` so Claude can refresh.
fn rewrite_scope(raw: &str, api_scope: &str) -> String {
    let mut parts: Vec<String> = raw
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s == "render" || s == api_scope.rsplit('/').next().unwrap_or(s) {
                api_scope.to_owned()
            } else {
                s.to_owned()
            }
        })
        .collect();
    if !parts.iter().any(|s| s == api_scope) {
        parts.push(api_scope.to_owned());
    }
    for extra in ["openid", "offline_access"] {
        if !parts.iter().any(|s| s == extra) {
            parts.push(extra.to_owned());
        }
    }
    parts.join(" ")
}

/// Keep Claude's RFC 8707 resource when it is our MCP URL (or the origin
/// form). Anything else is dropped so we never mint a token for a stranger.
fn rewrite_resource(raw: &str, mcp_resource: &str) -> Option<String> {
    let trimmed = raw.trim_end_matches('/');
    let origin = mcp_resource.trim_end_matches('/').trim_end_matches("/mcp");
    if trimmed == mcp_resource.trim_end_matches('/') || trimmed == origin {
        Some(mcp_resource.to_owned())
    } else {
        None
    }
}

pub async fn authorize(State(st): State<OAuthProxyState>, RawQuery(q): RawQuery) -> Response {
    let mut pairs = parse_pairs(&q.unwrap_or_default());

    let Some(client_id) = exactly_one(&pairs, "client_id") else {
        return (StatusCode::BAD_REQUEST, "missing or duplicate client_id\n").into_response();
    };
    if client_id != st.inner.client_id {
        return (StatusCode::BAD_REQUEST, "unregistered client_id\n").into_response();
    }
    if exactly_one(&pairs, "response_type") != Some("code") {
        return (StatusCode::BAD_REQUEST, "response_type must be code\n").into_response();
    }
    let Some(client_redirect_uri) = exactly_one(&pairs, "redirect_uri").map(str::to_owned) else {
        return (StatusCode::BAD_REQUEST, "missing redirect_uri\n").into_response();
    };
    if !is_allowed_redirect_uri(&st.inner.allowed_redirect_uris, &client_redirect_uri) {
        return (StatusCode::BAD_REQUEST, "unregistered redirect_uri\n").into_response();
    }

    let client_state = match pairs.iter().filter(|(key, _)| key == "state").count() {
        0 => None,
        1 => exactly_one(&pairs, "state").map(str::to_owned),
        _ => return (StatusCode::BAD_REQUEST, "duplicate state\n").into_response(),
    };
    if client_state
        .as_ref()
        .is_some_and(|state| state.len() > MAX_CLIENT_STATE_BYTES)
    {
        return (StatusCode::BAD_REQUEST, "state is too long\n").into_response();
    }
    let Some(code_challenge) = exactly_one(&pairs, "code_challenge").map(str::to_owned) else {
        return (
            StatusCode::BAD_REQUEST,
            "missing or duplicate code_challenge\n",
        )
            .into_response();
    };
    if !valid_code_challenge(&code_challenge)
        || exactly_one(&pairs, "code_challenge_method") != Some("S256")
    {
        return (
            StatusCode::BAD_REQUEST,
            "code_challenge_method must be S256 with a valid challenge\n",
        )
            .into_response();
    }

    let Some(proxy_state) = st.encode_state(&Pending {
        client_redirect_uri,
        client_state,
        code_challenge,
        issued_at: now_unix(),
    }) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    let mut saw_state = false;
    let mut saw_scope = false;
    let mut saw_resource = false;
    let mut drop_resource = false;
    for (k, v) in &mut pairs {
        if k == "redirect_uri" {
            v.clone_from(&st.inner.callback_url);
        } else if k == "state" {
            v.clone_from(&proxy_state);
            saw_state = true;
        } else if k == "scope" {
            *v = rewrite_scope(v, &st.inner.api_scope);
            saw_scope = true;
        } else if k == "resource" {
            if let Some(resource) = rewrite_resource(v, &st.inner.mcp_resource) {
                *v = resource;
                saw_resource = true;
            } else {
                drop_resource = true;
            }
        }
    }
    if drop_resource {
        pairs.retain(|(k, _)| k != "resource");
    }
    if !saw_state {
        pairs.push(("state".to_owned(), proxy_state));
    }
    if !saw_scope {
        pairs.push(("scope".to_owned(), rewrite_scope("", &st.inner.api_scope)));
    }
    if !saw_resource {
        pairs.push(("resource".to_owned(), st.inner.mcp_resource.clone()));
    }

    let qs = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    Redirect::to(&format!("{}?{}", st.inner.entra_authorize, qs)).into_response()
}

pub async fn callback(State(st): State<OAuthProxyState>, RawQuery(q): RawQuery) -> Response {
    let pairs = parse_pairs(&q.unwrap_or_default());

    let Some(state) = exactly_one(&pairs, "state") else {
        return (StatusCode::BAD_REQUEST, "missing or duplicate state\n").into_response();
    };
    let Some(pending) = st.decode_state(state) else {
        return (StatusCode::BAD_REQUEST, "unknown or expired state\n").into_response();
    };
    let Ok(mut url) = Url::parse(&pending.client_redirect_uri) else {
        return (StatusCode::BAD_REQUEST, "bad client redirect_uri\n").into_response();
    };

    {
        let mut qp = url.query_pairs_mut();
        if let Some(code) = exactly_one(&pairs, "code") {
            if code.len() > MAX_UPSTREAM_CODE_BYTES {
                return (StatusCode::BAD_REQUEST, "authorization code is too long\n")
                    .into_response();
            }
            let Some(proxy_code) = st.encode_code(&PendingCode {
                upstream_code: code.to_owned(),
                client_redirect_uri: pending.client_redirect_uri,
                code_challenge: pending.code_challenge,
                issued_at: now_unix(),
            }) else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            qp.append_pair("code", &proxy_code);
        }
        if let Some(err) = exactly_one(&pairs, "error") {
            qp.append_pair("error", err);
        }
        if let Some(desc) = exactly_one(&pairs, "error_description") {
            qp.append_pair("error_description", desc);
        }
        if let Some(cs) = &pending.client_state {
            qp.append_pair("state", cs);
        }
    }
    Redirect::to(url.as_str()).into_response()
}

pub async fn token(State(st): State<OAuthProxyState>, body: String) -> Response {
    let mut pairs = parse_pairs(&body);
    let Some(client_id) = exactly_one(&pairs, "client_id") else {
        return (StatusCode::BAD_REQUEST, "missing or duplicate client_id\n").into_response();
    };
    if client_id != st.inner.client_id {
        return (StatusCode::BAD_REQUEST, "unregistered client_id\n").into_response();
    }
    let Some(grant_type) = exactly_one(&pairs, "grant_type") else {
        return (StatusCode::BAD_REQUEST, "missing or duplicate grant_type\n").into_response();
    };
    if !matches!(grant_type, "authorization_code" | "refresh_token") {
        return (StatusCode::BAD_REQUEST, "unsupported grant_type\n").into_response();
    }
    let is_authorization_code = grant_type == "authorization_code";
    let authorization = if is_authorization_code {
        let Some(redirect_uri) = exactly_one(&pairs, "redirect_uri").map(str::to_owned) else {
            return (
                StatusCode::BAD_REQUEST,
                "missing or duplicate redirect_uri\n",
            )
                .into_response();
        };
        if !is_allowed_redirect_uri(&st.inner.allowed_redirect_uris, &redirect_uri) {
            return (StatusCode::BAD_REQUEST, "unregistered redirect_uri\n").into_response();
        }
        let Some(code) = exactly_one(&pairs, "code").map(str::to_owned) else {
            return (StatusCode::BAD_REQUEST, "missing or duplicate code\n").into_response();
        };
        let Some(verifier) = exactly_one(&pairs, "code_verifier").map(str::to_owned) else {
            return (
                StatusCode::BAD_REQUEST,
                "missing or duplicate code_verifier\n",
            )
                .into_response();
        };
        if !valid_code_verifier(&verifier) {
            return (StatusCode::BAD_REQUEST, "invalid code_verifier\n").into_response();
        }
        let Some(pending) = st.decode_code(&code) else {
            return (
                StatusCode::BAD_REQUEST,
                "unknown or expired authorization code\n",
            )
                .into_response();
        };
        use subtle::ConstantTimeEq as _;
        let challenge_matches: bool = challenge_for(&verifier)
            .as_bytes()
            .ct_eq(pending.code_challenge.as_bytes())
            .into();
        if pending.client_redirect_uri != redirect_uri || !challenge_matches {
            return (
                StatusCode::BAD_REQUEST,
                "authorization code binding mismatch\n",
            )
                .into_response();
        }
        Some((pending.upstream_code, redirect_uri))
    } else {
        None
    };
    let mut saw_redirect_uri = false;
    let mut saw_scope = false;
    let mut drop_resource = false;
    for (k, v) in &mut pairs {
        if k == "redirect_uri" {
            saw_redirect_uri = true;
            v.clone_from(&st.inner.callback_url);
        } else if k == "code"
            && let Some((upstream_code, _)) = &authorization
        {
            v.clone_from(upstream_code);
        } else if k == "scope" {
            *v = rewrite_scope(v, &st.inner.api_scope);
            saw_scope = true;
        } else if k == "resource" {
            if let Some(resource) = rewrite_resource(v, &st.inner.mcp_resource) {
                *v = resource;
            } else {
                drop_resource = true;
            }
        }
    }
    if drop_resource {
        pairs.retain(|(k, _)| k != "resource");
    }
    debug_assert!(!is_authorization_code || saw_redirect_uri);
    if !saw_scope && !is_authorization_code {
        pairs.push(("scope".to_owned(), rewrite_scope("", &st.inner.api_scope)));
    }
    let form = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();

    let resp = st
        .inner
        .http
        .post(&st.inner.entra_token)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status = r.status();
            let content_type = r
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_owned();
            let bytes = r.bytes().await.unwrap_or_default();
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
        }
        Err(e) => {
            warn!(error = %e, "token proxy upstream error");
            (StatusCode::BAD_GATEWAY, "token endpoint upstream error\n").into_response()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const VERIFIER: &str = "test-verifier-abcdefghijklmnopqrstuvwxyz-0123456789";

    fn authorize_query(redirect_uri: &str) -> String {
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("response_type", "code")
            .append_pair("client_id", "test-client")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("code_challenge", &challenge_for(VERIFIER))
            .append_pair("code_challenge_method", "S256")
            .finish()
    }

    fn state() -> OAuthProxyState {
        OAuthProxyState::new(
            "https://login.microsoftonline.com/abc/v2.0",
            "https://typst-mcp.example.test",
            "https://typst-mcp.example.test/mcp",
            "api://typst-mcp",
            "render",
            OAuthClientConfig {
                client_id: "test-client".to_owned(),
                allowed_redirect_uris: vec!["https://claude.ai/api/mcp/auth_callback".to_owned()],
                state_key: b"test signing key with at least thirty two bytes".to_vec(),
            },
        )
    }

    fn pending(issued_at: u64) -> Pending {
        Pending {
            client_redirect_uri: "https://claude.ai/api/mcp/auth_callback".to_owned(),
            client_state: Some("client-state".to_owned()),
            code_challenge: challenge_for(VERIFIER),
            issued_at,
        }
    }

    #[test]
    fn entra_urls_and_callback_are_derived() {
        let st = state();
        assert_eq!(
            st.inner.entra_authorize,
            "https://login.microsoftonline.com/abc/oauth2/v2.0/authorize"
        );
        assert_eq!(
            st.inner.entra_token,
            "https://login.microsoftonline.com/abc/oauth2/v2.0/token"
        );
        assert_eq!(
            st.inner.callback_url,
            "https://typst-mcp.example.test/oauth/callback"
        );
        assert_eq!(st.inner.api_scope, "api://typst-mcp/render");
    }

    #[test]
    fn rewrite_scope_qualifies_bare_render() {
        assert_eq!(
            rewrite_scope("render", "api://typst-mcp/render"),
            "api://typst-mcp/render openid offline_access"
        );
        assert_eq!(
            rewrite_scope("openid render", "api://typst-mcp/render"),
            "openid api://typst-mcp/render offline_access"
        );
    }

    #[test]
    fn rewrite_resource_accepts_origin_or_mcp() {
        let mcp = "https://typst-mcp.example.test/mcp";
        assert_eq!(
            rewrite_resource("https://typst-mcp.example.test/mcp", mcp).as_deref(),
            Some(mcp)
        );
        assert_eq!(
            rewrite_resource("https://typst-mcp.example.test/", mcp).as_deref(),
            Some(mcp)
        );
        assert_eq!(rewrite_resource("https://attacker.example/mcp", mcp), None);
    }

    #[tokio::test]
    async fn authorize_rejects_unregistered_redirect_uri() {
        let response = authorize(
            State(state()),
            RawQuery(Some(authorize_query("https://attacker.example/cb"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn authorize_requires_the_registered_client_and_s256_pkce() {
        for query in [
            "response_type=code&redirect_uri=https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback&code_challenge=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&code_challenge_method=S256",
            "response_type=code&client_id=wrong&redirect_uri=https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback&code_challenge=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&code_challenge_method=S256",
            "response_type=code&client_id=test-client&redirect_uri=https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback&code_challenge=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&code_challenge_method=plain",
        ] {
            let response = authorize(State(state()), RawQuery(Some(query.to_owned()))).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        }
    }

    #[test]
    fn stateless_pending_authorization_round_trips_and_rejects_tampering() {
        let st = state();
        let encoded = st.encode_state(&pending(now_unix())).expect("encodes");
        let decoded = st.decode_state(&encoded).expect("decodes");
        assert_eq!(decoded.client_state.as_deref(), Some("client-state"));

        let mut tampered = encoded.into_bytes();
        tampered[0] = if tampered[0] == b'A' { b'B' } else { b'A' };
        assert!(
            st.decode_state(std::str::from_utf8(&tampered).unwrap())
                .is_none()
        );
    }

    #[test]
    fn stateless_pending_authorization_expires() {
        let st = state();
        let encoded = st
            .encode_state(&pending(now_unix() - PENDING_TTL.as_secs()))
            .expect("encodes");
        assert!(st.decode_state(&encoded).is_none());
    }

    #[tokio::test]
    async fn authorize_rejects_oversized_client_state_without_storing_it() {
        let st = state();
        let oversized = "x".repeat(4097);
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", "test-client")
            .append_pair("redirect_uri", "https://claude.ai/api/mcp/auth_callback")
            .append_pair("response_type", "code")
            .append_pair("code_challenge", &challenge_for(VERIFIER))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &oversized)
            .finish();
        let response = authorize(State(st.clone()), RawQuery(Some(query))).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn authorize_forwards_to_entra_with_our_callback() {
        let st = state();
        let query = url::form_urlencoded::Serializer::new(authorize_query(
            "https://claude.ai/api/mcp/auth_callback",
        ))
        .append_pair("state", "client-state")
        .append_pair("scope", "render")
        .append_pair("resource", "https://typst-mcp.example.test/mcp")
        .finish();
        let response = authorize(State(st.clone()), RawQuery(Some(query))).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .expect("redirect location")
            .to_str()
            .unwrap();
        let upstream = Url::parse(location).unwrap();
        assert_eq!(
            upstream.as_str().split('?').next().unwrap(),
            "https://login.microsoftonline.com/abc/oauth2/v2.0/authorize"
        );
        let params: HashMap<String, String> = upstream.query_pairs().into_owned().collect();
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some("https://typst-mcp.example.test/oauth/callback")
        );
        assert_eq!(
            params.get("resource").map(String::as_str),
            Some("https://typst-mcp.example.test/mcp")
        );
        assert!(
            params.get("scope").is_some_and(
                |s| s.contains("api://typst-mcp/render") && s.contains("offline_access")
            )
        );
        let proxy_state = params.get("state").expect("proxy state");
        let pending = st.decode_state(proxy_state).expect("pending state");
        assert_eq!(
            pending.client_redirect_uri,
            "https://claude.ai/api/mcp/auth_callback"
        );
        assert_eq!(pending.client_state.as_deref(), Some("client-state"));
        assert_eq!(pending.code_challenge, challenge_for(VERIFIER));
    }

    #[tokio::test]
    async fn callback_brokers_the_upstream_code_and_binds_it_to_pkce() {
        let st = state();
        let proxy_state = st.encode_state(&pending(now_unix())).expect("proxy state");
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("state", &proxy_state)
            .append_pair("code", "entra-secret-code")
            .finish();
        let response = callback(State(st.clone()), RawQuery(Some(query))).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response.headers()[header::LOCATION].to_str().unwrap();
        assert!(!location.contains("entra-secret-code"));
        let redirect = Url::parse(location).unwrap();
        let values: HashMap<String, String> = redirect.query_pairs().into_owned().collect();
        assert_eq!(
            values.get("state").map(String::as_str),
            Some("client-state")
        );
        let code = values.get("code").expect("brokered code");
        let pending = st.decode_code(code).expect("valid brokered code");
        assert_eq!(pending.upstream_code, "entra-secret-code");
        assert_eq!(pending.code_challenge, challenge_for(VERIFIER));

        let mut tampered = code.as_bytes().to_vec();
        tampered[0] = if tampered[0] == b'A' { b'B' } else { b'A' };
        assert!(
            st.decode_code(std::str::from_utf8(&tampered).unwrap())
                .is_none()
        );
    }

    #[tokio::test]
    async fn token_rejects_unregistered_authorization_code_redirect_uri() {
        let response = token(
            State(state()),
            format!(
                "grant_type=authorization_code&client_id=test-client&code=abc&redirect_uri=https%3A%2F%2Fattacker.example%2Fcb&code_verifier={VERIFIER}"
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
