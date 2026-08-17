//! OAuth 2.0 metadata + a Dynamic Client Registration shim.
//!
//! Claude Desktop/Cowork onboards a remote MCP server through OAuth discovery +
//! **Dynamic Client Registration** (RFC 7591). Entra exposes no DCR endpoint,
//! so a bare `authorization_servers: [Entra]` dead-ends at "Automatic client
//! registration isn't supported".
//!
//! Fix: front Entra. We advertise *ourselves* as the authorization server in
//! the protected-resource metadata (RFC 9728), then serve RFC 8414
//! authorization-server metadata whose `authorize`/`token` endpoints are
//! same-origin (proxied to Entra) and whose `registration_endpoint` points at
//! our `/register` shim. The shim hands every caller one pre-provisioned Entra
//! public SPA client (`TYPST_MCP_DCR_CLIENT_ID`). The access token Claude
//! presents is still an Entra JWT for `api://typst-mcp`.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;
use serde_json::{Value, json};

use crate::api::AppState;
use crate::config::{Config, OidcConfig};
use crate::oauth_redirect::is_allowed_redirect_uri;

fn scopes_supported(oidc: &OidcConfig) -> Vec<String> {
    vec![
        "openid".to_owned(),
        "profile".to_owned(),
        "offline_access".to_owned(),
        oidc.scope.clone(),
    ]
}

// ---------------------------------------------------------------------------
// Protected Resource Metadata (RFC 9728)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ProtectedResourceMetadata {
    pub resource: String,
    pub authorization_servers: Vec<String>,
    pub bearer_methods_supported: Vec<&'static str>,
    pub scopes_supported: Vec<String>,
}

impl ProtectedResourceMetadata {
    pub fn from_config(cfg: &Config, oidc: &OidcConfig) -> Self {
        Self {
            resource: cfg.mcp_resource_url(),
            // Point at OURSELVES, not Entra: Claude then fetches our
            // /.well-known/oauth-authorization-server (which carries a
            // registration_endpoint). authorize/token in that document
            // are same-origin and proxy to Entra.
            authorization_servers: vec![cfg.public_url.clone()],
            bearer_methods_supported: vec!["header"],
            scopes_supported: scopes_supported(oidc),
        }
    }
}

#[allow(clippy::unused_async)]
pub async fn protected_resource_metadata(State(state): State<AppState>) -> impl IntoResponse {
    let Some(oidc) = state.oidc_auth.config() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "not_configured",
                "message": "this server does not accept OIDC credentials",
            })),
        )
            .into_response();
    };
    Json(ProtectedResourceMetadata::from_config(&state.config, oidc)).into_response()
}

// ---------------------------------------------------------------------------
// Authorization Server Metadata (RFC 8414)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AuthorizationServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    pub response_types_supported: Vec<&'static str>,
    pub grant_types_supported: Vec<&'static str>,
    pub code_challenge_methods_supported: Vec<&'static str>,
    pub token_endpoint_auth_methods_supported: Vec<&'static str>,
    pub scopes_supported: Vec<String>,
}

impl AuthorizationServerMetadata {
    pub fn from_config(cfg: &Config, oidc: &OidcConfig) -> Self {
        let us = &cfg.public_url;
        Self {
            issuer: us.clone(),
            authorization_endpoint: format!("{us}/authorize"),
            token_endpoint: format!("{us}/token"),
            jwks_uri: entra_jwks_uri(&oidc.issuer),
            registration_endpoint: cfg.dcr_client_id.as_ref().map(|_| format!("{us}/register")),
            response_types_supported: vec!["code"],
            grant_types_supported: vec!["authorization_code", "refresh_token"],
            code_challenge_methods_supported: vec!["S256"],
            token_endpoint_auth_methods_supported: vec!["none"],
            scopes_supported: scopes_supported(oidc),
        }
    }
}

/// Entra v2 JWKS. Issuer is `https://login.microsoftonline.com/{tid}/v2.0`;
/// keys live at `{tenant}/discovery/v2.0/keys`.
pub fn entra_jwks_uri(issuer: &str) -> String {
    format!("{}/discovery/v2.0/keys", entra_tenant_base(issuer))
}

/// Strip a trailing `/v2.0` so we can build `/oauth2/v2.0/…` paths.
pub fn entra_tenant_base(issuer: &str) -> String {
    issuer
        .trim_end_matches('/')
        .trim_end_matches("/v2.0")
        .to_owned()
}

pub fn entra_authorize_url(issuer: &str) -> String {
    format!("{}/oauth2/v2.0/authorize", entra_tenant_base(issuer))
}

pub fn entra_token_url(issuer: &str) -> String {
    format!("{}/oauth2/v2.0/token", entra_tenant_base(issuer))
}

#[allow(clippy::unused_async)]
pub async fn authorization_server_metadata(State(state): State<AppState>) -> impl IntoResponse {
    let Some(oidc) = state.oidc_auth.config() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "not_configured",
                "message": "this server does not accept OIDC credentials",
            })),
        )
            .into_response();
    };
    Json(AuthorizationServerMetadata::from_config(
        &state.config,
        oidc,
    ))
    .into_response()
}

// ---------------------------------------------------------------------------
// Dynamic Client Registration shim (RFC 7591)
// ---------------------------------------------------------------------------

#[allow(clippy::unused_async)]
pub async fn register(
    State(state): State<AppState>,
    body: Option<Json<Value>>,
) -> impl IntoResponse {
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

    let scope = state
        .oidc_auth
        .config()
        .map(|oidc| {
            format!(
                "openid profile offline_access {}",
                qualified_api_scope(&oidc.audience, &oidc.scope)
            )
        })
        .unwrap_or_else(|| "openid profile offline_access".to_owned());

    let resp = json!({
        "client_id": client_id,
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "redirect_uris": redirect_uris,
        "scope": scope,
        "client_id_issued_at": now_unix(),
    });
    (StatusCode::CREATED, Json(resp)).into_response()
}

/// `api://typst-mcp` + `render` → `api://typst-mcp/render`.
/// If the configured scope is already qualified, leave it alone.
pub fn qualified_api_scope(audience: &str, scope: &str) -> String {
    if scope.contains("://") || scope.starts_with(audience) {
        scope.to_owned()
    } else {
        format!("{}/{}", audience.trim_end_matches('/'), scope)
    }
}

fn now_unix() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn entra_endpoints_are_derived_from_v2_issuer() {
        let iss = "https://login.microsoftonline.com/abc/v2.0";
        assert_eq!(
            entra_authorize_url(iss),
            "https://login.microsoftonline.com/abc/oauth2/v2.0/authorize"
        );
        assert_eq!(
            entra_token_url(iss),
            "https://login.microsoftonline.com/abc/oauth2/v2.0/token"
        );
        assert_eq!(
            entra_jwks_uri(iss),
            "https://login.microsoftonline.com/abc/discovery/v2.0/keys"
        );
    }

    #[test]
    fn qualifies_bare_scope_against_audience() {
        assert_eq!(
            qualified_api_scope("api://typst-mcp", "render"),
            "api://typst-mcp/render"
        );
        assert_eq!(
            qualified_api_scope("api://typst-mcp", "api://typst-mcp/render"),
            "api://typst-mcp/render"
        );
    }
}
