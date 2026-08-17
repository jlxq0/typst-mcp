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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use rand::RngCore;
use tracing::warn;
use url::Url;

use crate::oauth_metadata::{entra_authorize_url, entra_token_url, qualified_api_scope};
use crate::oauth_redirect::is_allowed_redirect_uri;

#[allow(clippy::duration_suboptimal_units)]
const PENDING_TTL: Duration = Duration::from_secs(600);
const PENDING_CAP: usize = 2048;

#[derive(Clone)]
pub struct OAuthProxyState {
    inner: Arc<Inner>,
}

struct Inner {
    entra_authorize: String,
    entra_token: String,
    callback_url: String,
    mcp_resource: String,
    api_scope: String,
    http: reqwest::Client,
    allowed_redirect_uris: Vec<String>,
    pending: Mutex<HashMap<String, Pending>>,
}

struct Pending {
    client_redirect_uri: String,
    client_state: Option<String>,
    created: Instant,
}

impl OAuthProxyState {
    pub fn new(
        issuer: &str,
        public_url: &str,
        mcp_resource: &str,
        audience: &str,
        api_scope: &str,
        allowed_redirect_uris: Vec<String>,
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
                http,
                allowed_redirect_uris,
                pending: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn insert(&self, state: String, pending: Pending) {
        if let Ok(mut g) = self.inner.pending.lock() {
            if g.len() >= PENDING_CAP {
                let now = Instant::now();
                g.retain(|_, p| now.duration_since(p.created) < PENDING_TTL);
            }
            g.insert(state, pending);
        }
    }

    fn take(&self, state: &str) -> Option<Pending> {
        let p = {
            let mut g = self.inner.pending.lock().ok()?;
            g.remove(state)?
        };
        if Instant::now().duration_since(p.created) >= PENDING_TTL {
            return None;
        }
        Some(p)
    }
}

fn random_state() -> String {
    let mut b = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

fn parse_pairs(q: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(q.as_bytes())
        .into_owned()
        .collect()
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

    let Some(client_redirect_uri) = pairs
        .iter()
        .find(|(k, _)| k == "redirect_uri")
        .map(|(_, v)| v.clone())
    else {
        return (StatusCode::BAD_REQUEST, "missing redirect_uri\n").into_response();
    };
    if !is_allowed_redirect_uri(&st.inner.allowed_redirect_uris, &client_redirect_uri) {
        return (StatusCode::BAD_REQUEST, "unregistered redirect_uri\n").into_response();
    }

    let client_state = pairs
        .iter()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.clone());

    let proxy_state = random_state();
    st.insert(
        proxy_state.clone(),
        Pending {
            client_redirect_uri,
            client_state,
            created: Instant::now(),
        },
    );

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
    let pairs: HashMap<String, String> = parse_pairs(&q.unwrap_or_default()).into_iter().collect();

    let Some(state) = pairs.get("state") else {
        return (StatusCode::BAD_REQUEST, "missing state\n").into_response();
    };
    let Some(pending) = st.take(state) else {
        return (StatusCode::BAD_REQUEST, "unknown or expired state\n").into_response();
    };
    let Ok(mut url) = Url::parse(&pending.client_redirect_uri) else {
        return (StatusCode::BAD_REQUEST, "bad client redirect_uri\n").into_response();
    };

    {
        let mut qp = url.query_pairs_mut();
        if let Some(code) = pairs.get("code") {
            qp.append_pair("code", code);
        }
        if let Some(err) = pairs.get("error") {
            qp.append_pair("error", err);
        }
        if let Some(desc) = pairs.get("error_description") {
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
    let is_authorization_code = pairs
        .iter()
        .any(|(k, v)| k == "grant_type" && v == "authorization_code");
    let mut saw_redirect_uri = false;
    let mut saw_scope = false;
    let mut drop_resource = false;
    for (k, v) in &mut pairs {
        if k == "redirect_uri" {
            if !is_allowed_redirect_uri(&st.inner.allowed_redirect_uris, v) {
                return (StatusCode::BAD_REQUEST, "unregistered redirect_uri\n").into_response();
            }
            saw_redirect_uri = true;
            v.clone_from(&st.inner.callback_url);
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
    if is_authorization_code && !saw_redirect_uri {
        return (StatusCode::BAD_REQUEST, "missing redirect_uri\n").into_response();
    }
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

    fn state() -> OAuthProxyState {
        OAuthProxyState::new(
            "https://login.microsoftonline.com/abc/v2.0",
            "https://typst-mcp.example.test",
            "https://typst-mcp.example.test/mcp",
            "api://typst-mcp",
            "render",
            vec!["https://claude.ai/api/mcp/auth_callback".to_owned()],
        )
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
            RawQuery(Some(
                "client_id=abc&redirect_uri=https%3A%2F%2Fattacker.example%2Fcb&response_type=code"
                    .to_owned(),
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn authorize_forwards_to_entra_with_our_callback() {
        let st = state();
        let response = authorize(
            State(st.clone()),
            RawQuery(Some(
                "client_id=abc&redirect_uri=https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback&state=client-state&scope=render&resource=https%3A%2F%2Ftypst-mcp.example.test%2Fmcp"
                    .to_owned(),
            )),
        )
        .await;
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
        let pending = st.take(proxy_state).expect("pending state");
        assert_eq!(
            pending.client_redirect_uri,
            "https://claude.ai/api/mcp/auth_callback"
        );
        assert_eq!(pending.client_state.as_deref(), Some("client-state"));
    }

    #[tokio::test]
    async fn token_rejects_unregistered_authorization_code_redirect_uri() {
        let response = token(
            State(state()),
            "grant_type=authorization_code&code=abc&redirect_uri=https%3A%2F%2Fattacker.example%2Fcb&code_verifier=v"
                .to_owned(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
