//! Same-origin OAuth 2.0 proxy in front of the Hanso Group Entra tenant.
//!
//! Native MCP clients send `cursor://` / `grokbot://` / `claude://` redirects.
//! Entra will not register those. We keep the client's redirect, send Entra
//! only `https://<host>/oauth/callback`, then bounce the code back.

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

use crate::api::AppState;
use crate::config::OidcConfig;
use crate::oauth_redirect::is_allowed_redirect_uri;

#[allow(clippy::duration_suboptimal_units)]
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);
const PENDING_CAP: usize = 2048;

#[derive(Clone)]
pub struct OAuthProxyState {
    inner: Arc<Inner>,
}

struct Inner {
    pending: Mutex<HashMap<String, Pending>>,
    http: reqwest::Client,
}

struct Pending {
    client_redirect_uri: String,
    client_state: Option<String>,
    created: Instant,
}

impl OAuthProxyState {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("typst-mcp/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                pending: Mutex::new(HashMap::new()),
                http,
            }),
        }
    }

    fn insert(&self, state: String, pending: Pending) {
        if let Ok(mut entries) = self.inner.pending.lock() {
            if entries.len() >= PENDING_CAP {
                let now = Instant::now();
                entries.retain(|_, value| now.duration_since(value.created) < PENDING_TTL);
            }
            if entries.len() < PENDING_CAP {
                entries.insert(state, pending);
            }
        }
    }

    fn take(&self, state: &str) -> Option<Pending> {
        let pending = self.inner.pending.lock().ok()?.remove(state)?;
        (pending.created.elapsed() < PENDING_TTL).then_some(pending)
    }

    fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }
}

impl Default for OAuthProxyState {
    fn default() -> Self {
        Self::new()
    }
}

fn random_state() -> String {
    let mut bytes = [0_u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn parse_pairs(raw: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(raw.as_bytes())
        .into_owned()
        .collect()
}

fn upstream_scope(oidc: &OidcConfig) -> String {
    let scoped = if oidc.audience.contains("://") {
        format!("{}/{}", oidc.audience.trim_end_matches('/'), oidc.scope)
    } else {
        oidc.scope.clone()
    };
    format!("openid offline_access {scoped}")
}

pub async fn authorize(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let Some(client_id) = state.config.client_id.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "TYPST_MCP_OIDC_CLIENT_ID is not set. Create the Entra public client and restart.\n",
        )
            .into_response();
    };
    let Some(oidc) = state.config.oidc.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "OIDC is not configured\n",
        )
            .into_response();
    };
    let Some(authorize_url) = state.config.entra_authorize_url() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    let pairs = parse_pairs(&raw.unwrap_or_default());
    let Some(client_redirect_uri) = pairs
        .iter()
        .find(|(key, _)| key == "redirect_uri")
        .map(|(_, value)| value.clone())
    else {
        return (StatusCode::BAD_REQUEST, "missing redirect_uri\n").into_response();
    };
    if !is_allowed_redirect_uri(&state.config.oauth_redirect_uris, &client_redirect_uri) {
        warn!(endpoint = "authorize", attempted = %client_redirect_uri, "rejected redirect_uri");
        return (StatusCode::BAD_REQUEST, "unregistered redirect_uri\n").into_response();
    }
    let client_state = pairs
        .iter()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.clone());
    let proxy_state = random_state();
    state.oauth_proxy.insert(
        proxy_state.clone(),
        Pending {
            client_redirect_uri,
            client_state,
            created: Instant::now(),
        },
    );

    let Ok(mut upstream) = Url::parse(&authorize_url) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    {
        let mut query = upstream.query_pairs_mut();
        query.append_pair("client_id", client_id);
        query.append_pair("response_type", "code");
        query.append_pair("redirect_uri", &state.config.callback_url());
        query.append_pair("response_mode", "query");
        query.append_pair("state", &proxy_state);
        query.append_pair("scope", &upstream_scope(oidc));
        let requested_prompt = pairs
            .iter()
            .find(|(key, _)| key == "prompt")
            .map(|(_, value)| value.as_str());
        query.append_pair(
            "prompt",
            requested_prompt
                .filter(|prompt| *prompt != "consent")
                .unwrap_or("select_account"),
        );
        for key in [
            "code_challenge",
            "code_challenge_method",
            "login_hint",
            "domain_hint",
        ] {
            if let Some(value) = pairs
                .iter()
                .find(|(candidate, _)| candidate == key)
                .map(|(_, value)| value)
            {
                query.append_pair(key, value);
            }
        }
        if pairs.iter().any(|(key, _)| key == "code_challenge")
            && !pairs.iter().any(|(key, _)| key == "code_challenge_method")
        {
            query.append_pair("code_challenge_method", "S256");
        }
    }
    Redirect::to(upstream.as_str()).into_response()
}

pub async fn callback(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let pairs: HashMap<String, String> =
        parse_pairs(&raw.unwrap_or_default()).into_iter().collect();
    let Some(proxy_state) = pairs.get("state") else {
        return (StatusCode::BAD_REQUEST, "missing state\n").into_response();
    };
    let Some(pending) = state.oauth_proxy.take(proxy_state) else {
        return (StatusCode::BAD_REQUEST, "unknown or expired state\n").into_response();
    };
    let Ok(mut destination) = Url::parse(&pending.client_redirect_uri) else {
        return (StatusCode::BAD_REQUEST, "bad client redirect_uri\n").into_response();
    };
    {
        let mut query = destination.query_pairs_mut();
        for key in ["code", "error", "error_description"] {
            if let Some(value) = pairs.get(key) {
                query.append_pair(key, value);
            }
        }
        if let Some(client_state) = pending.client_state {
            query.append_pair("state", &client_state);
        }
    }
    Redirect::to(destination.as_str()).into_response()
}

pub async fn token(State(state): State<AppState>, body: String) -> Response {
    let Some(client_id) = state.config.client_id.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({
                "error": "temporarily_unavailable",
                "error_description": "client_id not configured",
            })),
        )
            .into_response();
    };
    let Some(token_url) = state.config.entra_token_url() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    let incoming: HashMap<String, String> = parse_pairs(&body).into_iter().collect();
    let grant_type = incoming.get("grant_type").map_or("", String::as_str);
    let mut form = vec![
        ("client_id".to_owned(), client_id.clone()),
        ("grant_type".to_owned(), grant_type.to_owned()),
    ];
    match grant_type {
        "authorization_code" => {
            let Some(redirect_uri) = incoming.get("redirect_uri") else {
                return (StatusCode::BAD_REQUEST, "missing redirect_uri\n").into_response();
            };
            if !is_allowed_redirect_uri(&state.config.oauth_redirect_uris, redirect_uri) {
                warn!(endpoint = "token", attempted = %redirect_uri, "rejected redirect_uri");
                return (StatusCode::BAD_REQUEST, "unregistered redirect_uri\n").into_response();
            }
            form.push(("redirect_uri".to_owned(), state.config.callback_url()));
            for key in ["code", "code_verifier"] {
                if let Some(value) = incoming.get(key) {
                    form.push((key.to_owned(), value.clone()));
                }
            }
        }
        "refresh_token" => {
            if let Some(value) = incoming.get("refresh_token") {
                form.push(("refresh_token".to_owned(), value.clone()));
            }
            if let Some(oidc) = state.config.oidc.as_ref() {
                form.push(("scope".to_owned(), upstream_scope(oidc)));
            }
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": "unsupported_grant_type"})),
            )
                .into_response();
        }
    }

    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let response = state
        .oauth_proxy
        .http()
        .post(&token_url)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ACCEPT, "application/json")
        .body(encoded)
        .send()
        .await;
    match response {
        Ok(upstream) => {
            let status = upstream.status();
            let content_type = upstream
                .headers()
                .get(header::CONTENT_TYPE)
                .cloned()
                .unwrap_or_else(|| axum::http::HeaderValue::from_static("application/json"));
            let bytes = upstream.bytes().await.unwrap_or_default();
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
        }
        Err(error) => {
            warn!(error = %error, "token proxy upstream error");
            (StatusCode::BAD_GATEWAY, "token endpoint upstream error\n").into_response()
        }
    }
}
