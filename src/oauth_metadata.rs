//! RFC 9728 protected-resource metadata, RFC 8414 authorization-server
//! metadata, and the RFC 7591 static-client DCR shim.
//!
//! We front Hanso Entra the same way jmap-mcp / m365-mcp front their IdPs:
//! `authorization_servers` is *this* origin so Claude fetches our
//! `/.well-known/oauth-authorization-server` (which advertises
//! `registration_endpoint`). Entra itself has no DCR.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Value, json};

use crate::api::AppState;
use crate::oauth_redirect::is_allowed_redirect_uri;

fn resource_scopes(state: &AppState) -> Vec<String> {
    state
        .oidc_auth
        .config()
        .map(|oidc| vec![oidc.scope.clone()])
        .unwrap_or_default()
}

fn as_scopes(state: &AppState) -> Vec<String> {
    let mut scopes = vec!["openid".to_owned(), "offline_access".to_owned()];
    for scope in resource_scopes(state) {
        if !scopes.iter().any(|existing| existing == &scope) {
            scopes.push(scope);
        }
    }
    scopes
}

#[derive(Debug, Serialize)]
pub struct ProtectedResourceMetadata {
    pub resource: String,
    pub authorization_servers: Vec<String>,
    pub bearer_methods_supported: Vec<&'static str>,
    pub scopes_supported: Vec<String>,
}

impl ProtectedResourceMetadata {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            resource: state.config.mcp_resource_url(),
            // Point at ourselves, not Entra: Claude then fetches our AS
            // metadata and finds /register.
            authorization_servers: vec![state.config.public_url.clone()],
            bearer_methods_supported: vec!["header"],
            scopes_supported: resource_scopes(state),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AuthorizationServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    pub response_types_supported: Vec<&'static str>,
    pub grant_types_supported: Vec<&'static str>,
    pub code_challenge_methods_supported: Vec<&'static str>,
    pub token_endpoint_auth_methods_supported: Vec<&'static str>,
    pub scopes_supported: Vec<String>,
}

impl AuthorizationServerMetadata {
    pub fn from_state(state: &AppState) -> Self {
        let us = &state.config.public_url;
        Self {
            issuer: us.clone(),
            authorization_endpoint: format!("{us}/authorize"),
            token_endpoint: format!("{us}/token"),
            registration_endpoint: state
                .config
                .dcr_client_id
                .as_ref()
                .map(|_| format!("{us}/register")),
            response_types_supported: vec!["code"],
            grant_types_supported: vec!["authorization_code", "refresh_token"],
            code_challenge_methods_supported: vec!["S256"],
            token_endpoint_auth_methods_supported: vec!["none"],
            scopes_supported: as_scopes(state),
        }
    }
}

pub async fn protected_resource_metadata(State(state): State<AppState>) -> Response {
    if state.oidc_auth.config().is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "not_configured",
                "message": "this server does not accept OIDC credentials",
            })),
        )
            .into_response();
    }
    Json(ProtectedResourceMetadata::from_state(&state)).into_response()
}

pub async fn authorization_server_metadata(State(state): State<AppState>) -> Response {
    Json(AuthorizationServerMetadata::from_state(&state)).into_response()
}

/// RFC 7591 DCR shim. Returns the pre-provisioned Entra public client.
pub async fn register(State(state): State<AppState>, body: Option<Json<Value>>) -> Response {
    let Some(client_id) = state.config.dcr_client_id.clone() else {
        return (
            StatusCode::NOT_FOUND,
            "dynamic client registration is not configured\n",
        )
            .into_response();
    };

    let redirect_uris: Vec<String> = body
        .as_ref()
        .and_then(|Json(v)| v.get("redirect_uris"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    if redirect_uris
        .iter()
        .any(|uri| !is_allowed_redirect_uri(&state.config.oauth_redirect_uris, uri))
    {
        return (StatusCode::BAD_REQUEST, "unregistered redirect_uri\n").into_response();
    }

    let resp = json!({
        "client_id": client_id,
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "redirect_uris": redirect_uris,
        "scope": as_scopes(&state).join(" "),
        "client_id_issued_at": now_unix(),
    });
    (StatusCode::CREATED, Json(resp)).into_response()
}

fn now_unix() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
    .unwrap_or(i64::MAX)
}
