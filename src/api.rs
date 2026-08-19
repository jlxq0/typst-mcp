//! The HTTP surface: `/api/v1`, `/files`, `/health` and OAuth discovery.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::auth::{ApiKeyAuth, AuthError, Authenticated, OidcAuth, unauthorized};
use crate::config::Config;
use crate::diagnostics::Diagnostic;
use crate::oauth_metadata::{authorization_server_metadata, protected_resource_metadata, register};
use crate::oauth_proxy::{self, OAuthProxyState};
use crate::principal::TenantId;
use crate::render::{DOCUMENT_NAME, RenderError, RenderRequest, RenderService};
use crate::signing::SignatureError;
use crate::store::{Kind, Meta, StoreError};
use crate::templates::TemplateKind;

/// Everything the handlers share.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub render: Arc<RenderService>,
    pub api_key_auth: ApiKeyAuth,
    pub oidc_auth: OidcAuth,
}

/// Build the router.
///
/// `/mcp` is mounted only when OIDC is configured. An MCP endpoint that accepted
/// static keys would put a long-lived shared secret into a desktop client's config
/// file, which is exactly what the OAuth flow exists to avoid.
pub fn router(state: AppState) -> axum::Router {
    let protected = axum::Router::new()
        .route("/render", post(render))
        .route("/compile", post(render))
        .route("/templates", get(list_templates))
        .route("/templates/{name}", get(get_template))
        .route("/assets", get(list_assets).post(upload_asset))
        .route("/fonts", get(list_fonts))
        .route("/links", post(create_link))
        .layer(axum::middleware::from_fn_with_state(
            state.api_key_auth.clone(),
            crate::auth::require_api_key,
        ))
        .with_state(state.clone());

    let mut app = axum::Router::new()
        .route("/health", get(health))
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        // RFC 9728 §3.1 path-inserted location for the `{origin}/mcp` resource.
        // Strict clients follow WWW-Authenticate here; we also keep the bare
        // well-known path for clients that probe the origin.
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource_metadata),
        )
        // RFC 8414. The issuer is this origin, so the bare path is the
        // canonical one; the other three are what native MCP clients actually
        // probe (path-inserted after the resource, and the OIDC spelling).
        // One handler behind all four — a client that finds any of them finds
        // the same authorization server.
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server/mcp",
            get(authorization_server_metadata),
        )
        .route(
            "/.well-known/openid-configuration",
            get(authorization_server_metadata),
        )
        .route(
            "/.well-known/openid-configuration/mcp",
            get(authorization_server_metadata),
        )
        .route("/register", post(register))
        // Outside the bearer layer on purpose: this route accepts either a credential
        // or a signature, and does its own check as its first action.
        .route("/files/{tenant}/{job}/{name}", get(download))
        .nest("/api/v1", protected)
        .with_state(state.clone());

    if let Some(oidc) = state.oidc_auth.config() {
        app = app.merge(
            axum::Router::new()
                .route("/authorize", get(oauth_proxy::authorize))
                .route("/oauth/callback", get(oauth_proxy::callback))
                .route("/token", post(oauth_proxy::token))
                .with_state(OAuthProxyState::new(
                    &oidc.issuer,
                    &state.config.public_url,
                    &state.config.mcp_resource_url(),
                    &oidc.audience,
                    &oidc.scope,
                    state.config.oauth_redirect_uris.clone(),
                )),
        );
        app = app.merge(mcp_router(state));
    }
    app
}

/// The MCP endpoint, behind OIDC.
///
/// The service holds no caller identity. rmcp builds one service per *session* and
/// injects each HTTP request's `Parts` into the tool context, so a tool reads the
/// authenticated principal from the request it is actually serving. Baking a tenant
/// into the service at construction would tie identity to the session instead, and two
/// concurrent sessions could then be handed each other's storage.
fn mcp_router(state: AppState) -> axum::Router {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };

    let render = Arc::clone(&state.render);
    let config = Arc::clone(&state.config);

    // rmcp defends against DNS rebinding by refusing any `Host` it does not know,
    // and its default list is loopback only — `localhost`, `127.0.0.1`, `::1`. That
    // is the right default for a server on a laptop and completely wrong for a
    // public deployment: every request arrives as `Host: typst-mcp.hanso.group` and
    // is answered 403 *after* the bearer token has been accepted. A client sees a
    // successful OAuth handshake followed by a connection it cannot open, and
    // reports "failed to load / 0 tools" — which reads as an auth problem and is
    // not one. Nothing on the loopback list can catch this: tests and local runs
    // connect to 127.0.0.1 and pass.
    //
    // The public URL is where clients are told to connect, so it is exactly the
    // Host they will send. Loopback stays on the list for local runs.
    let mcp_config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(allowed_mcp_hosts(&state.config.public_url));

    let service = StreamableHttpService::new(
        move || {
            Ok(crate::mcp::TypstMcp::new(
                Arc::clone(&render),
                Arc::clone(&config),
            ))
        },
        LocalSessionManager::default().into(),
        mcp_config,
    );

    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(
            state.oidc_auth.clone(),
            crate::auth::require_oidc,
        ))
        .with_state(state)
}

/// Hosts the MCP endpoint will answer to: whatever `PUBLIC_URL` names, plus loopback.
///
/// A bare host (no port) matches that host on any port, which is what a deployment
/// behind a proxy needs — the client sends `Host: typst-mcp.hanso.group` while the
/// process itself listens on :3000.
fn allowed_mcp_hosts(public_url: &str) -> Vec<String> {
    let mut hosts = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ];
    if let Ok(url) = url::Url::parse(public_url)
        && let Some(host) = url.host_str()
        && !hosts.iter().any(|h| h == host)
    {
        hosts.push(host.to_owned());
    }
    hosts
}

// -- health & discovery ---------------------------------------------------------

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
    templates: usize,
    store_bytes: u64,
    timestamp: u64,
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        status: "healthy",
        version: env!("CARGO_PKG_VERSION"),
        templates: state.render.templates().len(),
        store_bytes: state.render.store().used_bytes(),
        timestamp: now(),
    })
}

// -- rendering ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RenderQuery {
    /// `json` (default) returns the tool result; `pdf` returns the bytes directly.
    #[serde(default)]
    output: Option<String>,
}

async fn render(
    State(state): State<AppState>,
    Query(query): Query<RenderQuery>,
    caller: Caller,
    Json(request): Json<RenderRequest>,
) -> Response {
    let tenant = caller.tenant(&state);
    match query.output.as_deref() {
        None | Some("json") => match state.render.render(&tenant, &request).await {
            Ok(result) => Json(result).into_response(),
            Err(err) => render_error(err),
        },
        Some("pdf") => {
            // Direct PDF is structurally no-store: it calls compile(), not render(),
            // and asks the worker for no previews that would only be discarded.
            let mut request = request;
            request.preview_pages = Some(vec![]);
            match state.render.compile(&tenant, &request).await {
                Ok(result) => (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "application/pdf".to_owned()),
                        (
                            header::CONTENT_DISPOSITION,
                            format!("inline; filename=\"{DOCUMENT_NAME}\""),
                        ),
                    ],
                    result.pdf,
                )
                    .into_response(),
                Err(err) => render_error(err),
            }
        }
        Some(_) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "bad_request",
                "message": "output must be `json` or `pdf`",
            })),
        )
            .into_response(),
    }
}

/// Map a render failure onto a status and a body.
///
/// A failed *document* is 422 with diagnostics — the diagnostics are the product in
/// that case, and the caller fixes their input and retries. Only an actual server
/// fault is a 5xx.
fn render_error(err: RenderError) -> Response {
    let diagnostics = err.diagnostics().to_vec();
    let (status, code) = match &err {
        RenderError::UnknownTemplate { .. } => (StatusCode::NOT_FOUND, "unknown_template"),
        RenderError::Ambiguous | RenderError::Empty => (StatusCode::BAD_REQUEST, "bad_request"),
        RenderError::Template(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_data"),
        RenderError::DuplicateAsset(_) | RenderError::Bundle(_) => {
            (StatusCode::UNPROCESSABLE_ENTITY, "invalid_bundle")
        }
        RenderError::Compile { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "compile_failed"),
        RenderError::Store(e) => return store_error(e),
        RenderError::Spawn(crate::spawn::SpawnError::Timeout { .. }) => {
            (StatusCode::GATEWAY_TIMEOUT, "timeout")
        }
        RenderError::Spawn(crate::spawn::SpawnError::Overloaded) => {
            (StatusCode::SERVICE_UNAVAILABLE, "overloaded")
        }
        RenderError::Spawn(_) | RenderError::Workspace(_) | RenderError::Protocol(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    };

    let mut body = serde_json::json!({ "error": code, "message": err.to_string() });
    if !diagnostics.is_empty() {
        body["diagnostics"] = serde_json::to_value(&diagnostics).unwrap_or_default();
    }
    (status, Json(body)).into_response()
}

fn store_error(err: &StoreError) -> Response {
    let (status, code) = match err {
        // Expired is deliberately not 404: a caller told "not found" retries the same
        // id forever, where "expired" tells it to produce the thing again.
        StoreError::Expired { .. } => (StatusCode::GONE, "expired"),
        StoreError::NotFound { .. } | StoreError::BadId { .. } => {
            (StatusCode::NOT_FOUND, "not_found")
        }
        StoreError::TenantFull { .. } => (StatusCode::INSUFFICIENT_STORAGE, "quota_exceeded"),
        StoreError::Io { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    };
    (
        status,
        Json(serde_json::json!({ "error": code, "message": err.to_string() })),
    )
        .into_response()
}

// -- templates ------------------------------------------------------------------

#[derive(Serialize)]
struct TemplateSummary {
    name: String,
    kind: &'static str,
    version: String,
    description: String,
    data_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
}

async fn list_templates(State(state): State<AppState>) -> Json<serde_json::Value> {
    let templates: Vec<TemplateSummary> = state
        .render
        .templates()
        .iter()
        .map(|t| TemplateSummary {
            name: t.name().to_owned(),
            kind: match t.manifest.kind {
                TemplateKind::Wrapper => "wrapper",
                TemplateKind::Data => "data",
            },
            version: t.manifest.version.clone(),
            description: t.manifest.description.clone(),
            data_fields: t.data_fields().into_iter().map(str::to_owned).collect(),
            notice: t.manifest.notice.clone(),
        })
        .collect();
    Json(serde_json::json!({ "templates": templates }))
}

async fn get_template(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    let Some(template) = state.render.templates().get(&name) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "unknown_template",
                "available": state.render.templates().names(),
            })),
        )
            .into_response();
    };

    Json(serde_json::json!({
        "name": template.name(),
        "kind": match template.manifest.kind {
            TemplateKind::Wrapper => "wrapper",
            TemplateKind::Data => "data",
        },
        "version": template.manifest.version,
        "description": template.manifest.description,
        "notice": template.manifest.notice,
        "schema": template.schema(),
        "example": template.example(),
        "example_body": template.example_body(),
    }))
    .into_response()
}

// -- assets ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UploadQuery {
    /// The bundle path the asset is mounted at, e.g. `assets/logo.png`.
    path: String,
}

async fn upload_asset(
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    caller: Caller,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // Normalised here, so a hostile path is refused before anything is written.
    let path = match crate::bundle::normalise_path(&query.path) {
        Ok(path) => path,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "bad_path", "message": err.to_string() })),
            )
                .into_response();
        }
    };

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();

    let tenant = caller.tenant(&state);
    match state.render.store().put(
        &tenant,
        Kind::Asset,
        &path,
        &body,
        Meta {
            filename: Some(path.clone()),
            content_type: Some(content_type.clone()),
        },
    ) {
        Ok(entry) => Json(serde_json::json!({
            "id": entry.id,
            "path": path,
            "content_type": content_type,
            "bytes": entry.bytes,
            "expires_at": entry.expires_at,
        }))
        .into_response(),
        Err(err) => store_error(&err),
    }
}

async fn list_assets(State(state): State<AppState>, caller: Caller) -> Json<serde_json::Value> {
    let tenant = caller.tenant(&state);
    Json(serde_json::json!({
        "assets": state.render.store().list(&tenant, Kind::Asset),
    }))
}

// -- fonts ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FontQuery {
    #[serde(default)]
    query: Option<String>,
}

async fn list_fonts(
    State(state): State<AppState>,
    Query(query): Query<FontQuery>,
) -> Json<serde_json::Value> {
    // Built per request rather than held: the font index is only consulted here and by
    // workers, and keeping one alive in the server would be a copy nobody reads.
    let fonts = crate::fonts::FontLibrary::new(&state.config.font_dirs);
    let needle = query.query.map(|q| q.to_lowercase());
    let families: Vec<_> = fonts
        .families()
        .into_iter()
        .filter(|f| {
            needle
                .as_ref()
                .is_none_or(|n| f.name.to_lowercase().contains(n))
        })
        .collect();
    Json(serde_json::json!({ "total": families.len(), "families": families }))
}

// -- links & downloads ----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LinkRequest {
    job_id: String,
    #[serde(default)]
    ttl_seconds: Option<u64>,
}

async fn create_link(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<LinkRequest>,
) -> Response {
    let tenant = caller.tenant(&state);
    match state
        .render
        .link(&tenant, &request.job_id, request.ttl_seconds)
    {
        Ok((url, expires_at)) => {
            Json(serde_json::json!({ "url": url, "expires_at": expires_at })).into_response()
        }
        Err(err) => render_error(err),
    }
}

/// Fetch a stored file.
///
/// Accepts a bearer credential **or** a valid signature, because a browser cannot send
/// a header on a plain link. The tenant in the path must match whichever proof is
/// presented, so a leaked link reaches exactly one document.
async fn download(
    State(state): State<AppState>,
    Path((tenant, job, name)): Path<(String, String, String)>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    // Validated before it is used to build any path.
    let Some(tenant) = TenantId::parse(&tenant) else {
        return not_found();
    };

    let authorised = match state.api_key_auth.authenticate(&headers) {
        // A credential must belong to the tenant in the path, or any valid key could
        // read any tenant's documents.
        Ok(caller) => caller.principal.tenant(&state.config.tenant_salt) == tenant,
        Err(_) => false,
    };

    if !authorised {
        match state.render.signer().verify_params(
            &tenant,
            &job,
            params.get("exp").map(String::as_str),
            params.get("sig").map(String::as_str),
        ) {
            Ok(()) => {}
            Err(SignatureError::Expired) => {
                return (
                    StatusCode::GONE,
                    Json(serde_json::json!({
                        "error": "expired",
                        "message": "this link has expired; request a fresh one",
                    })),
                )
                    .into_response();
            }
            // An invalid or absent signature is indistinguishable from a wrong id on
            // purpose: probing must not reveal which documents exist.
            Err(_) => return not_found(),
        }
    }

    match state.render.store().get(&tenant, Kind::Output, &job, &name) {
        Ok(bytes) => {
            let content_type = if name.ends_with(".png") {
                "image/png"
            } else {
                "application/pdf"
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type.to_owned()),
                    // Private: these are per-tenant documents behind a short-lived
                    // signature, and a shared cache must never serve one to anyone else.
                    (header::CACHE_CONTROL, "private, max-age=60".to_owned()),
                ],
                bytes,
            )
                .into_response()
        }
        Err(err) => store_error(&err),
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "not_found" })),
    )
        .into_response()
}

// -- extractor ------------------------------------------------------------------

/// The authenticated caller, put in place by the middleware.
pub struct Caller(pub Authenticated);

impl Caller {
    fn tenant(&self, state: &AppState) -> TenantId {
        self.0.principal.tenant(&state.config.tenant_salt)
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Caller {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let found = parts.extensions.get::<Authenticated>().cloned();
        async move {
            // Only reachable if a route was mounted without the middleware; failing
            // closed means that mistake is a 401 rather than an anonymous request.
            found
                .map(Caller)
                .ok_or_else(|| unauthorized(AuthError::Missing, "Bearer"))
        }
    }
}

/// Diagnostics as returned by the API.
#[derive(Serialize)]
pub struct DiagnosticsBody {
    pub diagnostics: Vec<Diagnostic>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::allowed_mcp_hosts;

    #[test]
    fn the_public_host_is_allowed_alongside_loopback() {
        let hosts = allowed_mcp_hosts("https://typst-mcp.hanso.group");
        assert!(
            hosts.iter().any(|h| h == "typst-mcp.hanso.group"),
            "the host clients are told to connect to must be allowed: {hosts:?}"
        );
        // Local runs and the test suite connect over loopback; dropping these would
        // trade one 403 for another.
        for loopback in ["localhost", "127.0.0.1", "::1"] {
            assert!(hosts.iter().any(|h| h == loopback), "{loopback}: {hosts:?}");
        }
    }

    #[test]
    fn a_public_url_with_a_port_contributes_the_bare_host() {
        // A bare host matches on any port, which is what a deployment behind a
        // proxy needs: the client says :443, the process listens on :3000.
        let hosts = allowed_mcp_hosts("http://127.0.0.1:34567");
        assert!(hosts.iter().any(|h| h == "127.0.0.1"), "{hosts:?}");
        assert!(
            !hosts.iter().any(|h| h.contains(':') && h != "::1"),
            "{hosts:?}"
        );
    }

    #[test]
    fn an_unparseable_public_url_still_leaves_loopback() {
        let hosts = allowed_mcp_hosts("not a url");
        assert_eq!(hosts.len(), 3, "{hosts:?}");
    }
}
