//! The HTTP surface, driven end to end.
//!
//! A real server on a real port, real subprocess compiles, real files on disk. The
//! cheaper alternative — calling handlers directly — would not exercise the routing,
//! the middleware order, or the extractors, which is where this kind of service
//! actually goes wrong.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use typst_mcp::config::Config;
use typst_mcp::server::Server;

const ALICE: &str = "sk_alice_0123456789abcdef";
const BOB: &str = "sk_bob_0123456789abcdef";
const SALT: &str = "0123456789abcdef0123456789abcdef";

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// A running server plus its temp data directory.
struct TestServer {
    base: String,
    client: reqwest::Client,
    _data: tempfile::TempDir,
}

impl TestServer {
    async fn start() -> Self {
        Self::start_with(&[]).await
    }

    async fn start_with(overrides: &[(&str, &str)]) -> Self {
        let data = tempfile::tempdir().expect("tempdir");

        let mut env: HashMap<String, String> = [
            ("PUBLIC_URL", "http://127.0.0.1:0".to_owned()),
            ("TENANT_SALT", SALT.to_owned()),
            ("SIGNING_SECRET", SALT.to_owned()),
            ("API_KEYS", format!("alice:{ALICE},bob:{BOB}")),
            ("DATA_DIR", data.path().display().to_string()),
            ("TEMPLATE_DIR", repo("templates").display().to_string()),
            ("FONT_DIRS", repo("fonts").display().to_string()),
            // Port 0 lets the OS pick, so parallel tests cannot collide.
            ("BIND_ADDR", "127.0.0.1:0".to_owned()),
            ("COMPILE_TIMEOUT", "30s".to_owned()),
            // current_exe() here is libtest, so the worker has to be named.
            ("WORKER_EXE", env!("CARGO_BIN_EXE_typst-mcp").to_owned()),
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

        // The public URL has to match the bound port, or returned links point nowhere.
        let mut config = config;
        config.public_url = base.clone();

        let server = Server::build(config).expect("build");
        let app = server.router();

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self {
            base,
            client: reqwest::Client::new(),
            _data: data,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    async fn post_json(
        &self,
        path: &str,
        key: Option<&str>,
        body: serde_json::Value,
    ) -> reqwest::Response {
        let mut request = self.client.post(self.url(path)).json(&body);
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        request.send().await.expect("request")
    }

    async fn get(&self, path: &str, key: Option<&str>) -> reqwest::Response {
        let mut request = self.client.get(self.url(path));
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        request.send().await.expect("request")
    }
}

#[tokio::test]
async fn health_is_public_and_reports_the_templates() {
    let server = TestServer::start().await;
    let response = server.get("/health", None).await;
    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(body["status"], "healthy");
    assert!(body["templates"].as_u64().expect("templates") >= 1);
}

#[tokio::test]
async fn the_api_requires_a_credential() {
    let server = TestServer::start().await;

    for (method, path) in [
        ("POST", "/api/v1/render"),
        ("GET", "/api/v1/templates"),
        ("GET", "/api/v1/assets"),
        ("GET", "/api/v1/fonts"),
    ] {
        let response = if method == "POST" {
            server.post_json(path, None, serde_json::json!({})).await
        } else {
            server.get(path, None).await
        };
        assert_eq!(response.status(), 401, "{method} {path}");
        // A 401 without a challenge leaves a client with nothing to act on.
        assert!(
            response.headers().contains_key("www-authenticate"),
            "{method} {path} has no WWW-Authenticate header"
        );
    }
}

#[tokio::test]
async fn a_credential_is_never_accepted_from_the_query_string() {
    // Query strings land in access logs, browser history and Referer headers. A key
    // that reaches any of those is spent, so this must not work even once.
    let server = TestServer::start().await;
    for path in [
        &format!("/api/v1/templates?token={ALICE}"),
        &format!("/api/v1/templates?api_key={ALICE}"),
        &format!("/api/v1/templates?key={ALICE}"),
    ] {
        assert_eq!(server.get(path, None).await.status(), 401, "{path}");
    }
}

#[tokio::test]
async fn a_wrong_key_is_rejected() {
    let server = TestServer::start().await;
    let response = server.get("/api/v1/templates", Some("sk_not_a_key")).await;
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn templates_can_be_listed_and_inspected() {
    let server = TestServer::start().await;

    let body: serde_json::Value = server
        .get("/api/v1/templates", Some(ALICE))
        .await
        .json()
        .await
        .expect("json");
    let templates = body["templates"].as_array().expect("array");
    assert!(templates.iter().any(|t| t["name"] == "hanso"));

    let body: serde_json::Value = server
        .get("/api/v1/templates/hanso", Some(ALICE))
        .await
        .json()
        .await
        .expect("json");
    assert_eq!(body["kind"], "wrapper");
    // The schema and a working example are what let a caller use this unattended.
    assert!(body["schema"]["properties"]["title"].is_object());
    assert!(body["example"]["title"].is_string());
    assert!(body["example_body"].is_string());
}

#[tokio::test]
async fn an_unknown_template_lists_what_is_available() {
    let server = TestServer::start().await;
    let response = server.get("/api/v1/templates/nope", Some(ALICE)).await;
    assert_eq!(response.status(), 404);

    let body: serde_json::Value = response.json().await.expect("json");
    assert!(
        body["available"]
            .as_array()
            .expect("available")
            .iter()
            .any(|n| n == "hanso"),
        "the error should name the real templates: {body}"
    );
}

#[tokio::test]
async fn rendering_the_hanso_template_returns_a_url_and_the_pdf() {
    let server = TestServer::start().await;

    let response = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({
                "template": "hanso",
                "data": { "title": "HTTP Test", "date": "2026-08-15" },
                "body": "= Chapter\n\nBody text.",
            }),
        )
        .await;
    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.expect("json");
    let url = body["url"].as_str().expect("url");
    assert!(body["pages"].as_u64().expect("pages") >= 2);
    assert!(body["job_id"].as_str().expect("job_id").starts_with("job_"));

    // The URL must actually serve the document to its owner.
    let pdf = server
        .client
        .get(url)
        .bearer_auth(ALICE)
        .send()
        .await
        .expect("fetch");
    assert_eq!(pdf.status(), 200);
    assert_eq!(pdf.headers()["content-type"], "application/pdf");
    // Never a shared cache: these are per-tenant documents.
    assert!(
        pdf.headers()["cache-control"]
            .to_str()
            .expect("header")
            .contains("private"),
    );
    assert!(pdf.bytes().await.expect("bytes").starts_with(b"%PDF-"));
}

#[tokio::test]
async fn output_pdf_returns_the_bytes_directly() {
    // The "just give me the file" path a backend service would call.
    let server = TestServer::start().await;
    let response = server
        .post_json(
            "/api/v1/render?output=pdf",
            Some(ALICE),
            serde_json::json!({ "source": "= Direct\n\nBytes." }),
        )
        .await;

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/pdf");
    assert!(response.bytes().await.expect("bytes").starts_with(b"%PDF-"));
}

#[tokio::test]
async fn one_tenant_cannot_fetch_anothers_document() {
    // G7: the isolation that matters, exercised through the real HTTP path rather
    // than at the store API.
    let server = TestServer::start().await;

    let body: serde_json::Value = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({ "source": "= Alice's private document" }),
        )
        .await
        .json()
        .await
        .expect("json");
    let url = body["url"].as_str().expect("url").to_owned();

    // Bob has a valid key and the exact URL, and still must not get the document.
    let response = server
        .client
        .get(&url)
        .bearer_auth(BOB)
        .send()
        .await
        .expect("fetch");
    assert_eq!(response.status(), 404, "bob read alice's document");

    // Nor without any credential.
    assert_eq!(
        server
            .client
            .get(&url)
            .send()
            .await
            .expect("fetch")
            .status(),
        404
    );
}

#[tokio::test]
async fn a_signed_link_works_without_any_header_and_expires() {
    let server = TestServer::start().await;

    let rendered: serde_json::Value = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({ "source": "= Signed" }),
        )
        .await
        .json()
        .await
        .expect("json");
    let job_id = rendered["job_id"].as_str().expect("job_id");

    let link: serde_json::Value = server
        .post_json(
            "/api/v1/links",
            Some(ALICE),
            serde_json::json!({ "job_id": job_id }),
        )
        .await
        .json()
        .await
        .expect("json");
    let url = link["url"].as_str().expect("url");
    assert!(url.contains("sig="), "{url}");
    // The credential must never appear in the URL.
    assert!(!url.contains(ALICE), "the link leaked the API key: {url}");

    // A browser sends no Authorization header, which is the whole point.
    let response = server.client.get(url).send().await.expect("fetch");
    assert_eq!(response.status(), 200);
    assert!(response.bytes().await.expect("bytes").starts_with(b"%PDF-"));

    // Tampering with the expiry must not extend it.
    let tampered = url.replace("exp=", "exp=9");
    assert_ne!(
        server
            .client
            .get(&tampered)
            .send()
            .await
            .expect("fetch")
            .status(),
        200
    );
}

#[tokio::test]
async fn a_broken_document_is_422_with_positioned_diagnostics() {
    // The diagnostics are the product here: the caller reads them and fixes the input.
    let server = TestServer::start().await;
    let response = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({ "source": "= Fine\n\n#let broken =\n" }),
        )
        .await;

    assert_eq!(response.status(), 422);
    let body: serde_json::Value = response.json().await.expect("json");
    let diagnostics = body["diagnostics"].as_array().expect("diagnostics");
    let first = &diagnostics[0];
    assert_eq!(first["severity"], "error");
    assert_eq!(first["file"], "main.typ");
    assert!(first["line"].as_u64().is_some(), "{first}");
}

#[tokio::test]
async fn body_errors_point_at_the_callers_own_line_numbers() {
    // A wrapper body carries a generated import prelude; reporting raw line numbers
    // would point a caller at a file it never wrote.
    let server = TestServer::start().await;
    let response = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({
                "template": "hanso",
                "data": { "title": "x", "date": "2026-08-15" },
                "body": "= Fine\n\nAlso fine.\n\n#let broken =\n",
            }),
        )
        .await;

    assert_eq!(response.status(), 422);
    let body: serde_json::Value = response.json().await.expect("json");
    let first = &body["diagnostics"][0];
    assert_eq!(first["file"], "body.typ", "{first}");
    assert_eq!(first["line"], 5, "{first}");
}

#[tokio::test]
async fn invalid_template_data_is_rejected_before_compiling() {
    let server = TestServer::start().await;
    let response = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({
                "template": "hanso",
                "data": { "titel": "typo" },
                "body": "= x",
            }),
        )
        .await;

    assert_eq!(response.status(), 422);
    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(body["error"], "invalid_data");
}

#[tokio::test]
async fn an_asset_can_be_uploaded_and_used_in_a_document() {
    // The path that lets a caller put a bitmap into a document: bytes go up over REST,
    // and the render references the returned id.
    let server = TestServer::start().await;

    // A 1x1 red PNG.
    let png: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    let uploaded: serde_json::Value = server
        .client
        .post(server.url("/api/v1/assets?path=logo.png"))
        .bearer_auth(ALICE)
        .header("content-type", "image/png")
        .body(png)
        .send()
        .await
        .expect("upload")
        .json()
        .await
        .expect("json");

    let id = uploaded["id"].as_str().expect("id");
    assert!(id.starts_with("ast_"));

    let response = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({
                "source": "#image(\"logo.png\", width: 2cm)",
                "assets": [id],
            }),
        )
        .await;
    assert_eq!(response.status(), 200, "{:?}", response.text().await);
}

#[tokio::test]
async fn one_tenant_cannot_reference_anothers_asset() {
    let server = TestServer::start().await;

    let uploaded: serde_json::Value = server
        .client
        .post(server.url("/api/v1/assets?path=secret.txt"))
        .bearer_auth(ALICE)
        .body("alice's data")
        .send()
        .await
        .expect("upload")
        .json()
        .await
        .expect("json");
    let id = uploaded["id"].as_str().expect("id");

    let response = server
        .post_json(
            "/api/v1/render",
            Some(BOB),
            serde_json::json!({
                "source": "#read(\"secret.txt\")",
                "assets": [id],
            }),
        )
        .await;
    assert_eq!(response.status(), 404, "bob used alice's asset");
}

#[tokio::test]
async fn a_hostile_asset_path_is_refused() {
    let server = TestServer::start().await;
    for path in ["../escape.png", "/etc/passwd", "a/../../b"] {
        let response = server
            .client
            .post(server.url(&format!("/api/v1/assets?path={path}")))
            .bearer_auth(ALICE)
            .body("x")
            .send()
            .await
            .expect("upload");
        assert_eq!(response.status(), 400, "{path} was accepted");
    }
}

#[tokio::test]
async fn fonts_can_be_listed_and_filtered() {
    let server = TestServer::start().await;
    let body: serde_json::Value = server
        .get("/api/v1/fonts?query=figtree", Some(ALICE))
        .await
        .json()
        .await
        .expect("json");

    let families = body["families"].as_array().expect("families");
    assert!(
        families.iter().any(|f| f["name"] == "Figtree"),
        "the brand font must be listed: {body}"
    );
}

#[tokio::test]
async fn oauth_metadata_is_served_when_oidc_is_configured() {
    // The one thing the MCP authorization spec requires a server to implement.
    let server = TestServer::start_with(&[
        ("OIDC_ISSUER", "https://login.microsoftonline.com/abc/v2.0"),
        ("OIDC_AUDIENCE", "api://typst-mcp"),
    ])
    .await;

    let response = server
        .get("/.well-known/oauth-protected-resource", None)
        .await;
    assert_eq!(
        response.status(),
        200,
        "must be reachable without a credential"
    );

    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(
        body["authorization_servers"][0],
        server.base
    );
    assert_eq!(body["bearer_methods_supported"][0], "header");
    assert_eq!(body["scopes_supported"][0], "render");
    assert_eq!(
        body["resource"],
        format!("{}/mcp", server.base),
        "RFC 9728 resource must be the /mcp endpoint, not the origin"
    );

    let inserted = server
        .get("/.well-known/oauth-protected-resource/mcp", None)
        .await;
    assert_eq!(inserted.status(), 200);
    let inserted_body: serde_json::Value = inserted.json().await.expect("json");
    assert_eq!(inserted_body["resource"], format!("{}/mcp", server.base));
}

#[tokio::test]
async fn oauth_metadata_is_absent_when_oidc_is_not_configured() {
    let server = TestServer::start().await;
    let response = server
        .get("/.well-known/oauth-protected-resource", None)
        .await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn a_runaway_document_times_out_and_the_server_keeps_serving() {
    let server = TestServer::start_with(&[("COMPILE_TIMEOUT", "3s")]).await;

    let response = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({
                "source": "#let acc = 0\n#for i in range(300000000) { acc = acc + i }\n#acc",
            }),
        )
        .await;
    assert_eq!(response.status(), 504);

    // The service must still work afterwards — containment that wedges the server has
    // contained nothing.
    let after = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({ "source": "= Still here" }),
        )
        .await;
    assert_eq!(after.status(), 200);
}

#[tokio::test]
async fn a_document_cannot_read_the_filesystem_over_http() {
    let server = TestServer::start().await;
    let response = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({ "source": "#read(\"/etc/passwd\")" }),
        )
        .await;

    assert_eq!(response.status(), 422);
    let text = response.text().await.expect("text");
    assert!(!text.contains("root:"), "leaked /etc/passwd: {text}");
}

#[tokio::test]
async fn a_request_with_neither_template_nor_source_says_so() {
    let server = TestServer::start().await;
    let response = server
        .post_json("/api/v1/render", Some(ALICE), serde_json::json!({}))
        .await;
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("json");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("template"),
        "{body}"
    );
}

#[tokio::test]
async fn authorization_server_metadata_and_dcr_are_unauthenticated() {
    let server = TestServer::start_with(&[
        ("OIDC_ISSUER", "https://login.microsoftonline.com/abc/v2.0"),
        ("OIDC_AUDIENCE", "api://typst-mcp"),
        ("OIDC_CLIENT_ID", "entra-public-client"),
    ])
    .await;

    let as_meta = server
        .get("/.well-known/oauth-authorization-server", None)
        .await;
    assert_eq!(as_meta.status(), 200, "AS metadata must not 401");
    let body: serde_json::Value = as_meta.json().await.expect("json");
    assert_eq!(body["issuer"], server.base);
    assert_eq!(
        body["registration_endpoint"],
        format!("{}/register", server.base)
    );
    assert_eq!(
        body["authorization_endpoint"],
        format!("{}/authorize", server.base)
    );

    let register = server
        .post_json(
            "/register",
            None,
            serde_json::json!({
                "client_name": "spec-probe-public",
                "redirect_uris": ["cursor://anysphere.cursor-mcp/oauth/callback"],
                "token_endpoint_auth_method": "none",
            }),
        )
        .await;
    assert_eq!(register.status(), 201);
    let created: serde_json::Value = register.json().await.expect("json");
    assert_eq!(created["client_id"], "entra-public-client");
    assert!(created.get("client_secret").is_none());

    let rejected = server
        .post_json(
            "/register",
            None,
            serde_json::json!({"redirect_uris": ["https://attacker.example/cb"]}),
        )
        .await;
    assert_eq!(rejected.status(), 400);
}
