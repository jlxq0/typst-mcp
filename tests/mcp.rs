//! The MCP endpoint over its real transport, with a mock identity provider.
//!
//! Driven with raw JSON-RPC and real RS256 tokens rather than a client library, so the
//! test asserts what actually goes over the wire: the negotiated protocol version, the
//! tool list a model will see, and — the one that matters — that a render comes back
//! with an image block the model can look at.
//!
//! The mock provider serves a real OIDC discovery document and JWKS, and tokens are
//! signed with the key in `tests/fixtures/`. That exercises signature verification,
//! issuer, audience, tenant and scope checks for real, rather than stubbing past them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::routing::get;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;
use typst_mcp::config::Config;
use typst_mcp::server::Server;

const SALT: &str = "0123456789abcdef0123456789abcdef";
const AUDIENCE: &str = "api://typst-mcp";
const DIRECTORY: &str = "test-directory-id";
const KID: &str = "test-key-1";

/// The protocol revision these tests speak, matching what the server advertises.
const PROTOCOL: &str = "2026-07-28";

/// The test signing key. Generated for these tests and used nowhere else.
const SIGNING_KEY: &str = include_str!("fixtures/test-signing-key.pem");

/// Its modulus, as the JWKS serves it.
const JWK_N: &str = "59Tsi8qMkutH-wixqmwcx99VjsUVIe5_iclBtDjicEsySKvuO_aaMwrnjvtRV7TyXfi7JYfnS1uxAnWqHlne9pI31r8PjAT1tIGsMlF17IbFY0Oi0Rbb9UXFPV4R0mgxE_-g9rJ3oIWnmffONp8HIMQ0P_J_gpVMOBjjm1z-574H__bzUTWxS-hM12kffKVWjsGTqIYwIYcGLgp-6SWKowa_tkj10z6zQYsMAP5U2JqdpcgPm7EdUXW9Qga7Dicph9SONn7wv4yxCC15-hzdOLx2N_rPMYIgCuQQmwoJmtTSEIpvdnrr82OecDxs2lfM6rmdisEWShUwfcTFz29wPQ";

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

/// Claims for a minted token.
#[derive(Serialize)]
struct Claims {
    sub: String,
    iss: String,
    aud: String,
    tid: String,
    scp: String,
    exp: u64,
    nbf: u64,
    iat: u64,
}

/// A stand-in identity provider: discovery plus JWKS, nothing else.
struct MockIdp {
    issuer: String,
}

impl MockIdp {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let issuer = format!("http://{addr}");

        let discovery_issuer = issuer.clone();
        let jwks_issuer = issuer.clone();
        let app = axum::Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || {
                    let issuer = discovery_issuer.clone();
                    async move {
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
                    let _ = &jwks_issuer;
                    async move {
                        Json(serde_json::json!({
                            "keys": [{
                                "kty": "RSA",
                                "kid": KID,
                                "use": "sig",
                                "alg": "RS256",
                                "n": JWK_N,
                                "e": "AQAB",
                            }],
                        }))
                    }
                }),
            );

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { issuer }
    }

    /// Mint a token, overriding any claim a test wants to break.
    fn token(&self, subject: &str) -> String {
        self.token_with(subject, |_| {})
    }

    fn token_with(&self, subject: &str, adjust: impl FnOnce(&mut Claims)) -> String {
        let mut claims = Claims {
            sub: subject.to_owned(),
            iss: self.issuer.clone(),
            aud: AUDIENCE.to_owned(),
            tid: DIRECTORY.to_owned(),
            scp: "render".to_owned(),
            exp: now() + 3600,
            nbf: now() - 60,
            iat: now() - 60,
        };
        adjust(&mut claims);

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(KID.to_owned());
        let key = EncodingKey::from_rsa_pem(SIGNING_KEY.as_bytes()).expect("test key");
        jsonwebtoken::encode(&header, &claims, &key).expect("encode")
    }
}

struct TestServer {
    base: String,
    client: reqwest::Client,
    idp: MockIdp,
    _data: tempfile::TempDir,
}

impl TestServer {
    async fn start() -> Self {
        Self::start_with(&[]).await
    }

    async fn start_with(overrides: &[(&str, &str)]) -> Self {
        Self::start_inner(overrides, None).await
    }

    /// Start with a `PUBLIC_URL` that is *not* the loopback address bound.
    ///
    /// Everything derived from the public URL — the RFC 9728 resource, and the
    /// `Host` values the MCP endpoint will answer to — then differs from where the
    /// test actually connects, which is the arrangement a real deployment is in
    /// and the only way to test it.
    async fn start_with_public_url(public_url: &str) -> Self {
        Self::start_inner(&[], Some(public_url)).await
    }

    async fn start_inner(overrides: &[(&str, &str)], public_url: Option<&str>) -> Self {
        let idp = MockIdp::start().await;
        let data = tempfile::tempdir().expect("tempdir");

        let mut env: HashMap<String, String> = [
            ("PUBLIC_URL", "http://127.0.0.1:0".to_owned()),
            ("TENANT_SALT", SALT.to_owned()),
            ("SIGNING_SECRET", SALT.to_owned()),
            ("API_KEYS", "alice:sk_alice_0123456789abcdef".to_owned()),
            ("DATA_DIR", data.path().display().to_string()),
            ("TEMPLATE_DIR", repo("templates").display().to_string()),
            ("FONT_DIRS", repo("fonts").display().to_string()),
            ("BIND_ADDR", "127.0.0.1:0".to_owned()),
            ("WORKER_EXE", env!("CARGO_BIN_EXE_typst-mcp").to_owned()),
            ("OIDC_ISSUER", idp.issuer.clone()),
            ("OIDC_AUDIENCE", AUDIENCE.to_owned()),
            ("OIDC_TENANT_ID", DIRECTORY.to_owned()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        for (k, v) in overrides {
            if v.is_empty() {
                env.remove(*k);
            } else {
                env.insert((*k).to_owned(), (*v).to_owned());
            }
        }

        let config = Config::from_source(&move |name| env.get(name).cloned()).expect("config");
        let listener = tokio::net::TcpListener::bind(config.bind_addr)
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let base = format!("http://{addr}");

        let mut config = config;
        config.public_url = public_url.map_or_else(|| base.clone(), str::to_owned);
        let app = Server::build(config).expect("build").router();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self {
            base,
            client: reqwest::Client::new(),
            idp,
            _data: data,
        }
    }

    /// POST a raw JSON-RPC body, exactly as given.
    async fn rpc(&self, token: Option<&str>, body: serde_json::Value) -> reqwest::Response {
        let method = body["method"].as_str().unwrap_or_default().to_owned();
        let mut request = self
            .client
            .post(format!("{}/mcp", self.base))
            // A streamable-HTTP client advertises both; rmcp refuses without it.
            .header("accept", "application/json, text/event-stream")
            .json(&body);
        if !method.is_empty() && method != "initialize" {
            // SEP-2243: from 2026-07-28 a POST must name its method in a header as well
            // as in the body, and the two have to agree. `tools/call` additionally
            // names the tool, so an intermediary can route or audit without parsing
            // the body.
            request = request
                .header("mcp-protocol-version", PROTOCOL)
                .header("mcp-method", &method);
            if method == "tools/call"
                && let Some(name) = body["params"]["name"].as_str()
            {
                request = request.header("mcp-name", name);
            }
        }
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send().await.expect("request")
    }

    /// Send a JSON-RPC call and return the parsed `result`.
    ///
    /// Builds the 2026-07-28 request shape: the protocol removed the `initialize`
    /// handshake and sessions, so every request declares its own protocol version and
    /// client capabilities in `_meta` instead.
    async fn call(
        &self,
        token: &str,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let mut params = params;
        if !params.is_object() {
            params = serde_json::json!({});
        }
        params["_meta"] = serde_json::json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL,
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": { "name": "typst-mcp-tests", "version": "1" },
        });

        let response = self
            .rpc(
                Some(token),
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": method,
                    "params": params,
                }),
            )
            .await;
        assert!(
            response.status().is_success(),
            "{method} failed with {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
        parse_rpc(&response.text().await.expect("body"))
    }
}

/// Parse a JSON-RPC response, which may arrive as JSON or as an SSE frame.
fn parse_rpc(body: &str) -> serde_json::Value {
    let json = if body.starts_with("event:") || body.starts_with("data:") {
        body.lines()
            .find_map(|line| line.strip_prefix("data:"))
            .expect("an SSE data frame")
            .trim()
    } else {
        body.trim()
    };
    let parsed: serde_json::Value = serde_json::from_str(json).expect("json-rpc body");
    assert!(parsed.get("error").is_none(), "json-rpc error: {parsed}");
    parsed["result"].clone()
}

/// The text of every non-image content block in a tool result.
fn text_of(result: &serde_json::Value) -> String {
    result["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn images_in(result: &serde_json::Value) -> Vec<&serde_json::Value> {
    result["content"]
        .as_array()
        .map(|blocks| blocks.iter().filter(|b| b["type"] == "image").collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn mcp_is_not_mounted_without_oidc() {
    // An MCP endpoint on static keys would put a long-lived shared secret into a
    // desktop client's config file, which is what the OAuth flow exists to avoid.
    let server = TestServer::start_with(&[("OIDC_ISSUER", ""), ("OIDC_AUDIENCE", "")]).await;
    let response = server
        .rpc(
            None,
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn a_request_without_a_token_says_where_to_get_one() {
    let server = TestServer::start().await;
    let response = server
        .rpc(
            None,
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .await;

    assert_eq!(response.status(), 401);
    let challenge = response
        .headers()
        .get("www-authenticate")
        .expect("a 401 must carry a challenge")
        .to_str()
        .expect("ascii");
    // Without `resource_metadata` a client cannot discover the auth server, which is
    // precisely what the MCP authorization spec requires here.
    assert!(challenge.contains("resource_metadata="), "{challenge}");
    assert!(
        challenge.contains(".well-known/oauth-protected-resource/mcp"),
        "{challenge}"
    );
}

#[tokio::test]
async fn an_api_key_is_not_accepted_on_mcp() {
    // The two doors stay separate: static keys are for services.
    let server = TestServer::start().await;
    let response = server
        .rpc(
            Some("sk_alice_0123456789abcdef"),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .await;
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn a_token_that_fails_any_check_is_refused() {
    let server = TestServer::start().await;

    let cases = [
        // Well past the clock-skew tolerance, so this tests expiry rather than leeway.
        (
            "expired",
            server.idp.token_with("alice", |c| c.exp = now() - 3600),
        ),
        (
            "not yet valid",
            server.idp.token_with("alice", |c| c.nbf = now() + 3600),
        ),
        // Audience is what stops a token minted for another application being used here.
        (
            "wrong audience",
            server
                .idp
                .token_with("alice", |c| c.aud = "api://something-else".into()),
        ),
        // Without the tid check, a token from any other Entra directory would pass.
        (
            "wrong directory",
            server
                .idp
                .token_with("alice", |c| c.tid = "another-directory".into()),
        ),
        (
            "missing scope",
            server
                .idp
                .token_with("alice", |c| c.scp = "openid profile".into()),
        ),
        (
            "wrong issuer",
            server
                .idp
                .token_with("alice", |c| c.iss = "https://evil.example".into()),
        ),
    ];

    for (what, token) in cases {
        let response = server
            .rpc(
                Some(&token),
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            )
            .await;
        assert_eq!(response.status(), 401, "a token with a {what} was accepted");
    }
}

#[tokio::test]
async fn a_token_just_inside_the_skew_tolerance_is_still_accepted() {
    // Documents the deliberate tolerance: a token that expired seconds ago is honoured,
    // because the alternative is rejecting good tokens whenever two clocks disagree.
    let server = TestServer::start().await;
    let token = server.idp.token_with("alice", |c| c.exp = now() - 5);
    let response = server
        .rpc(
            Some(&token),
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": {"_meta": {
                    "io.modelcontextprotocol/protocolVersion": PROTOCOL,
                    "io.modelcontextprotocol/clientCapabilities": {},
                }},
            }),
        )
        .await;
    assert!(
        response.status().is_success(),
        "a token inside the skew tolerance should be accepted, got {}",
        response.status()
    );
}

#[tokio::test]
async fn a_valid_token_lists_the_tools() {
    let server = TestServer::start().await;
    let token = server.idp.token("alice");
    let result = server
        .call(&token, "tools/list", serde_json::json!({}))
        .await;

    let names: Vec<&str> = result["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();

    for expected in [
        "typst_render",
        "typst_compile",
        "typst_templates",
        "typst_template_schema",
        "typst_fonts",
        "typst_assets",
        "typst_link",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
}

#[tokio::test]
async fn the_protected_resource_metadata_is_reachable_without_a_token() {
    // A client reads this precisely because it has no credential yet.
    let server = TestServer::start().await;
    let response = server
        .client
        .get(format!(
            "{}/.well-known/oauth-protected-resource/mcp",
            server.base
        ))
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(body["authorization_servers"][0], server.base);
    assert_eq!(body["resource"], format!("{}/mcp", server.base));

    // Origin probe returns the same document — clients that skip path-insertion
    // still see resource = {origin}/mcp.
    let origin = server
        .client
        .get(format!(
            "{}/.well-known/oauth-protected-resource",
            server.base
        ))
        .send()
        .await
        .expect("request");
    assert_eq!(origin.status(), 200);
    let origin_body: serde_json::Value = origin.json().await.expect("json");
    assert_eq!(origin_body["resource"], format!("{}/mcp", server.base));
}

#[tokio::test]
async fn rendering_returns_a_link_and_an_image_the_model_can_see() {
    // The point of the whole tool surface. Without the image block a model cannot tell
    // a good layout from a broken one.
    let server = TestServer::start().await;
    let token = server.idp.token("alice");

    let result = server
        .call(
            &token,
            "tools/call",
            serde_json::json!({
                "name": "typst_render",
                "arguments": {
                    "template": "hanso",
                    "data": { "title": "MCP Test", "date": "2026-08-15" },
                    "body": "= Chapter\n\nSome text.",
                },
            }),
        )
        .await;

    let text = text_of(&result);
    assert!(text.contains("job_id"), "{text}");
    assert!(text.contains("/files/"), "{text}");

    let images = images_in(&result);
    assert_eq!(
        images.len(),
        1,
        "expected exactly one preview image: {result}"
    );
    assert_eq!(images[0]["mimeType"], "image/png");
    assert!(!images[0]["data"].as_str().expect("data").is_empty());
}

#[tokio::test]
async fn a_broken_document_returns_diagnostics_rather_than_a_protocol_error() {
    // A model is expected to read these and try again; a protocol error would throw
    // them away.
    let server = TestServer::start().await;
    let token = server.idp.token("alice");

    let result = server
        .call(
            &token,
            "tools/call",
            serde_json::json!({
                "name": "typst_compile",
                "arguments": { "source": "= Fine\n\n#let broken =" },
            }),
        )
        .await;

    assert_eq!(result["isError"], true, "{result}");
    let text = text_of(&result);
    assert!(text.contains("diagnostics"), "{text}");
    assert!(text.contains("main.typ"), "{text}");
    assert!(text.contains("line"), "{text}");
}

#[tokio::test]
async fn the_template_schema_comes_with_a_worked_example() {
    // A schema alone leaves a model guessing at shape; the example is what makes the
    // first call succeed.
    let server = TestServer::start().await;
    let token = server.idp.token("alice");

    let result = server
        .call(
            &token,
            "tools/call",
            serde_json::json!({
                "name": "typst_template_schema",
                "arguments": { "template": "hanso" },
            }),
        )
        .await;

    let text = text_of(&result);
    assert!(text.contains("example_data"), "{text}");
    assert!(text.contains("example_body"), "{text}");
    assert!(text.contains("title"), "{text}");
}

#[tokio::test]
async fn two_callers_never_see_each_others_documents() {
    // The property the per-request tenant derivation exists to guarantee: rmcp builds
    // one service per session, but identity has to come from the request.
    let server = TestServer::start().await;
    let alice = server.idp.token("alice");
    let bob = server.idp.token("bob");

    let rendered = server
        .call(
            &alice,
            "tools/call",
            serde_json::json!({
                "name": "typst_compile",
                "arguments": { "source": "= Alice's document" },
            }),
        )
        .await;
    let text = text_of(&rendered);
    let job_id = text
        .split("job_")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(|id| format!("job_{id}"))
        .expect("a job id");

    // Bob asks for a link to Alice's document, with the exact id.
    let denied = server
        .call(
            &bob,
            "tools/call",
            serde_json::json!({
                "name": "typst_link",
                "arguments": { "job_id": job_id },
            }),
        )
        .await;
    let denied_text = text_of(&denied);
    assert!(
        !denied_text.contains("/files/"),
        "bob was handed a link to alice's document: {denied_text}"
    );
}

#[tokio::test]
async fn fonts_can_be_queried_so_a_model_stops_guessing() {
    let server = TestServer::start().await;
    let token = server.idp.token("alice");
    let result = server
        .call(
            &token,
            "tools/call",
            serde_json::json!({
                "name": "typst_fonts",
                "arguments": { "query": "figtree" },
            }),
        )
        .await;
    assert!(text_of(&result).contains("Figtree"), "{result}");
}

#[tokio::test]
async fn preview_pages_are_capped_even_when_many_are_requested() {
    // Each image costs roughly 1.37 tokens per byte, twice.
    let server = TestServer::start().await;
    let token = server.idp.token("alice");

    let result = server
        .call(
            &token,
            "tools/call",
            serde_json::json!({
                "name": "typst_compile",
                "arguments": {
                    "source": "#for i in range(12) { pagebreak(weak: true); [Page] }",
                    "preview_pages": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
                },
            }),
        )
        .await;

    assert!(images_in(&result).len() <= 4, "over the preview cap");
}

/// A client that speaks an older MCP revision must still get the tool list.
///
/// Grok Bot and Cursor negotiate 2025-06-18 / 2025-11-25: they open with an
/// `initialize` handshake, echo back the `Mcp-Session-Id` they are handed, and
/// send none of the SEP-2243 `Mcp-*` headers that 2026-07-28 requires. The server
/// pins 2026-07-28 for itself, so it would be easy to assume such a client is the
/// reason a connector shows zero tools. It is not — and this test is what keeps
/// that answer trustworthy, so the next "0 tools" report is diagnosed instead of
/// guessed at.
#[tokio::test]
async fn a_client_on_an_older_protocol_revision_still_gets_the_tools() {
    let server = TestServer::start().await;
    let token = server.idp.token("alice");

    let init = server
        .rpc(
            Some(&token),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "older-client", "version": "1" }
                }
            }),
        )
        .await;
    assert!(
        init.status().is_success(),
        "initialize -> {}",
        init.status()
    );
    let session = init
        .headers()
        .get("mcp-session-id")
        .expect("an older client is handed a session id")
        .to_str()
        .expect("ascii")
        .to_owned();

    let listed = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .bearer_auth(&token)
        .json(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .send()
        .await
        .expect("request");
    assert!(
        listed.status().is_success(),
        "tools/list in the old shape -> {}",
        listed.status()
    );
    let body = listed.text().await.expect("body");
    assert!(
        body.contains("typst_render"),
        "an older client must see the tools: {}",
        &body[..body.len().min(300)]
    );
}

/// A request arriving with the deployment's public `Host` must reach the tools.
///
/// rmcp validates `Host` against an allowlist to stop DNS rebinding, and its
/// default is loopback only. On a public deployment that turns every call into a
/// 403 — *after* the bearer token has been accepted, so the client sees OAuth
/// succeed and then "failed to load MCP server / 0 tools", which looks like an
/// auth bug and is not one. Every other test here connects to 127.0.0.1, which
/// is on the default list, so this is the only shape that catches it.
#[tokio::test]
async fn a_request_carrying_the_public_host_is_not_refused() {
    let server = TestServer::start_with_public_url("https://typst-mcp.example.test").await;
    let token = server.idp.token("alice");

    // The server's PUBLIC_URL is its loopback base, so borrow a host that is
    // definitely not loopback and assert the guard is still doing its job...
    let rebinding = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("accept", "application/json, text/event-stream")
        .header("host", "attacker.example")
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "probe", "version": "1"}}
        }))
        .send()
        .await
        .expect("request");
    assert_eq!(
        rebinding.status(),
        403,
        "an unknown Host must still be refused"
    );

    // ...while the host this deployment publishes is accepted. `PUBLIC_URL` is
    // what clients are told to connect to, so it is the Host they will send.
    let allowed = server
        .client
        .post(format!("{}/mcp", server.base))
        .header("accept", "application/json, text/event-stream")
        .header("host", "typst-mcp.example.test")
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "probe", "version": "1"}}
        }))
        .send()
        .await
        .expect("request");
    assert_ne!(
        allowed.status(),
        403,
        "the deployment's own public host must not be treated as a rebinding attempt"
    );
    assert!(allowed.status().is_success(), "-> {}", allowed.status());
}
