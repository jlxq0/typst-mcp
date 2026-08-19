//! The HTTP surface, driven end to end.
//!
//! A real server on a real port, real subprocess compiles, real files on disk. The
//! cheaper alternative — calling handlers directly — would not exercise the routing,
//! the middleware order, or the extractors, which is where this kind of service
//! actually goes wrong.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use typst_mcp::config::Config;
use typst_mcp::server::Server;

const ALICE: &str = "sk_alice_0123456789abcdef0123456789abcdef";
const BOB: &str = "sk_bob_0123456789abcdef0123456789abcdef";
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

    async fn post_bytes(
        &self,
        path: &str,
        key: Option<&str>,
        content_type: &str,
        body: Vec<u8>,
    ) -> reqwest::Response {
        let mut request = self
            .client
            .post(self.url(path))
            .header("content-type", content_type)
            .body(body);
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        request.send().await.expect("request")
    }

    async fn delete(&self, path: &str, key: Option<&str>) -> reqwest::Response {
        let mut request = self.client.delete(self.url(path));
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

    fn remove_asset_bytes(&self, id: &str, name: &str) {
        let tenant_dir = std::fs::read_dir(self._data.path())
            .expect("read data dir")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with("t_"))
            .expect("tenant dir");
        std::fs::remove_file(tenant_dir.path().join("assets").join(id).join(name))
            .expect("remove asset bytes");
    }

    async fn store_bytes(&self) -> u64 {
        self.get("/health", None)
            .await
            .json::<serde_json::Value>()
            .await
            .expect("health json")["store_bytes"]
            .as_u64()
            .expect("store bytes")
    }

    fn compile_workspaces(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self._data.path().join("tmp"))
            .expect("read tmp")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("compile-"))
            })
            .collect()
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

fn draft_template_archive() -> Vec<u8> {
    let files = [
        (
            "template.toml",
            "name = \"draft\"\nkind = \"wrapper\"\nentrypoint = \"draft.typ\"\nwrapper_fn = \"draft\"\ndescription = \"Ephemeral test\"\n",
        ),
        (
            "draft.typ",
            "#let draft(body) = { set page(width: 100mm, height: 100mm); body }\n",
        ),
        ("fixture.json", "{}\n"),
        ("fixture.body.typ", "= Upload fixture\n\nIt compiles.\n"),
    ];
    let mut builder = tar::Builder::new(Vec::new());
    for (path, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, text.as_bytes())
            .expect("tar member");
    }
    builder.into_inner().expect("tar")
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

#[tokio::test]
async fn ephemeral_templates_are_tenant_scoped_renderable_and_deletable() {
    let server = TestServer::start().await;
    let response = server
        .post_bytes(
            "/api/v1/templates",
            Some(ALICE),
            "application/x-tar",
            draft_template_archive(),
        )
        .await;
    assert_eq!(response.status(), 201);
    let uploaded: serde_json::Value = response.json().await.expect("upload JSON");
    let id = uploaded["id"].as_str().expect("template id");
    assert!(id.starts_with("tpl_"));
    assert_eq!(uploaded["name"], "draft");

    let alice: serde_json::Value = server
        .get("/api/v1/templates", Some(ALICE))
        .await
        .json()
        .await
        .expect("Alice list");
    assert!(
        alice["templates"]
            .as_array()
            .is_some_and(|templates| templates.iter().any(|t| t["id"] == id))
    );
    let bob: serde_json::Value = server
        .get("/api/v1/templates", Some(BOB))
        .await
        .json()
        .await
        .expect("Bob list");
    assert!(
        bob["templates"]
            .as_array()
            .is_some_and(|templates| templates.iter().all(|t| t["id"] != id))
    );
    assert_eq!(
        server
            .get(&format!("/api/v1/templates/{id}"), Some(BOB))
            .await
            .status(),
        404
    );

    let rendered = server
        .post_json(
            "/api/v1/render?output=pdf",
            Some(ALICE),
            serde_json::json!({
                "template": id,
                "body": "= Tenant draft\n\nRendered through its tpl id."
            }),
        )
        .await;
    assert_eq!(rendered.status(), 200);
    assert!(rendered.bytes().await.expect("PDF").starts_with(b"%PDF-"));

    assert_eq!(
        server
            .delete(&format!("/api/v1/templates/{id}"), Some(BOB))
            .await
            .status(),
        404
    );
    assert_eq!(
        server
            .delete("/api/v1/templates/hanso", Some(ALICE))
            .await
            .status(),
        403
    );
    assert_eq!(
        server
            .delete(&format!("/api/v1/templates/{id}"), Some(ALICE))
            .await
            .status(),
        204
    );
}

#[tokio::test]
async fn template_expiry_and_quota_have_stable_statuses() {
    let expired = TestServer::start_with(&[("TEMPLATE_TTL", "0s")]).await;
    let response = expired
        .post_bytes(
            "/api/v1/templates",
            Some(ALICE),
            "application/x-tar",
            draft_template_archive(),
        )
        .await;
    assert_eq!(response.status(), 201);
    let body: serde_json::Value = response.json().await.expect("upload JSON");
    let id = body["id"].as_str().expect("id");
    let gone = expired
        .get(&format!("/api/v1/templates/{id}"), Some(ALICE))
        .await;
    assert_eq!(gone.status(), 410);
    assert_eq!(
        gone.json::<serde_json::Value>().await.unwrap()["error"],
        "expired"
    );

    let full = TestServer::start_with(&[("MAX_TENANT_BYTES", "100")]).await;
    let response = full
        .post_bytes(
            "/api/v1/templates",
            Some(ALICE),
            "application/x-tar",
            draft_template_archive(),
        )
        .await;
    assert_eq!(response.status(), 507);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["error"],
        "quota_exceeded"
    );
}

#[tokio::test]
async fn template_tar_traversal_is_rejected_before_storage() {
    let server = TestServer::start().await;
    let mut header = tar::Header::new_gnu();
    let hostile = "../outside.typ";
    header.as_mut_bytes()[..hostile.len()].copy_from_slice(hostile.as_bytes());
    header.set_size(1);
    header.set_mode(0o644);
    header.set_cksum();
    let mut builder = tar::Builder::new(Vec::new());
    builder.append(&header, b"x" as &[u8]).expect("raw member");
    let archive = builder.into_inner().expect("tar");

    let before = server.store_bytes().await;
    let response = server
        .post_bytes(
            "/api/v1/templates",
            Some(ALICE),
            "application/x-tar",
            archive,
        )
        .await;
    assert_eq!(response.status(), 422);
    assert_eq!(server.store_bytes().await, before);
}

#[tokio::test]
async fn gzip_template_uploads_work_and_broken_fixtures_are_not_stored() {
    let server = TestServer::start().await;
    let response = server
        .post_bytes(
            "/api/v1/templates",
            Some(ALICE),
            "application/gzip",
            gzip(&draft_template_archive()),
        )
        .await;
    assert_eq!(response.status(), 201);
    let after_good = server.store_bytes().await;

    let files = [
        (
            "template.toml",
            "name = \"broken\"\nkind = \"wrapper\"\nentrypoint = \"broken.typ\"\nwrapper_fn = \"broken\"\n",
        ),
        ("broken.typ", "#let broken(body) = body\n"),
        ("fixture.json", "{}\n"),
        ("fixture.body.typ", "#let nope =\n"),
    ];
    let mut builder = tar::Builder::new(Vec::new());
    for (path, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, text.as_bytes())
            .expect("tar member");
    }
    let broken = builder.into_inner().expect("tar");
    let response = server
        .post_bytes(
            "/api/v1/templates",
            Some(ALICE),
            "application/x-tar",
            broken,
        )
        .await;
    assert_eq!(response.status(), 422);
    let error: serde_json::Value = response.json().await.expect("error JSON");
    assert_eq!(error["error"], "compile_failed");
    assert_eq!(server.store_bytes().await, after_good);
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
    let before = server.store_bytes().await;
    assert!(server.compile_workspaces().is_empty());
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
    assert_eq!(server.store_bytes().await, before);
    assert!(
        server.compile_workspaces().is_empty(),
        "direct PDF must clean up its compile workspace"
    );
}

#[tokio::test]
async fn an_unknown_output_mode_is_rejected() {
    let server = TestServer::start().await;
    let response = server
        .post_json(
            "/api/v1/render?output=docx",
            Some(ALICE),
            serde_json::json!({ "source": "= Not DOCX" }),
        )
        .await;

    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(body["error"], "bad_request");
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

    // With no proof at all, tell a legitimate client how to authenticate. This differs
    // from Bob's valid wrong-tenant proof above, which must reveal nothing.
    assert_eq!(
        server
            .client
            .get(&url)
            .send()
            .await
            .expect("fetch")
            .status(),
        401
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

    let signature = url
        .split_once("sig=")
        .map(|(_, signature)| signature)
        .expect("signature");
    let expired = format!(
        "{}?exp=1&sig={signature}",
        url.split_once('?').map(|(base, _)| base).expect("query")
    );
    let response = server.client.get(expired).send().await.expect("expired");
    assert_eq!(response.status(), 410);
    let body: serde_json::Value = response.json().await.expect("expired envelope");
    assert_eq!(body["error"], "expired");
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
async fn an_uploaded_font_is_available_to_one_job_only() {
    let server = TestServer::start_with(&[("FONT_DIRS", "")]).await;
    let font = std::fs::read(repo("fonts/Figtree-Regular.ttf")).expect("font fixture");
    let response = server
        .post_bytes(
            "/api/v1/assets?path=fonts/Figtree-Regular.ttf&kind=font",
            Some(ALICE),
            "font/ttf",
            font,
        )
        .await;
    assert_eq!(response.status(), 200);
    let uploaded: serde_json::Value = response.json().await.expect("upload JSON");
    assert_eq!(uploaded["kind"], "font");
    let id = uploaded["id"].as_str().expect("font id");

    let listed: serde_json::Value = server
        .get("/api/v1/assets?kind=font&limit=1", Some(ALICE))
        .await
        .json()
        .await
        .expect("font list");
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["assets"][0]["id"], id);
    assert_eq!(listed["assets"][0]["kind"], "font");

    let source = "#set text(font: \"Figtree\")\n= Per-job font";
    let with_font: serde_json::Value = server
        .post_json(
            "/api/v1/compile",
            Some(ALICE),
            serde_json::json!({ "source": source, "assets": [id], "preview_pages": [] }),
        )
        .await
        .json()
        .await
        .expect("compile with font");
    assert!(
        with_font.get("diagnostics").is_none()
            || with_font["diagnostics"]
                .as_array()
                .is_some_and(Vec::is_empty),
        "uploaded Figtree did not resolve: {with_font}"
    );

    let without_font: serde_json::Value = server
        .post_json(
            "/api/v1/compile",
            Some(ALICE),
            serde_json::json!({ "source": source, "preview_pages": [] }),
        )
        .await
        .json()
        .await
        .expect("compile without font");
    assert!(
        without_font["diagnostics"]
            .as_array()
            .is_some_and(
                |diagnostics| diagnostics.iter().any(|diagnostic| diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("unknown font family")))
            ),
        "the font leaked into a later job: {without_font}"
    );
}

#[tokio::test]
async fn repeated_asset_ids_are_rejected_before_reading_asset_bytes() {
    let server = TestServer::start().await;
    let uploaded: serde_json::Value = server
        .client
        .post(server.url("/api/v1/assets?path=payload.bin"))
        .bearer_auth(ALICE)
        .body("asset bytes")
        .send()
        .await
        .expect("upload")
        .json()
        .await
        .expect("json");
    let id = uploaded["id"].as_str().expect("id");

    // Leave the metadata indexed but remove the content. A duplicate-id error proves
    // the whole request was validated before the first asset file was read.
    server.remove_asset_bytes(id, "payload.bin");
    let response = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({
                "source": "= Document",
                "assets": [id, id],
            }),
        )
        .await;

    assert_eq!(response.status(), 422);
    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(body["error"], "invalid_bundle");
    assert!(body["message"].as_str().expect("message").contains(id));
}

#[tokio::test]
async fn oversized_asset_metadata_is_rejected_before_reading_asset_bytes() {
    let server = TestServer::start_with(&[("MAX_BUNDLE_BYTES", "4")]).await;
    let uploaded: serde_json::Value = server
        .client
        .post(server.url("/api/v1/assets?path=payload.bin"))
        .bearer_auth(ALICE)
        .body("eight123")
        .send()
        .await
        .expect("upload")
        .json()
        .await
        .expect("json");
    let id = uploaded["id"].as_str().expect("id");

    // As above, a bundle-size error instead of a store read error proves metadata was
    // accumulated and checked before loading any content into the parent process.
    server.remove_asset_bytes(id, "payload.bin");
    let response = server
        .post_json(
            "/api/v1/render",
            Some(ALICE),
            serde_json::json!({
                "source": "= Document",
                "assets": [id],
            }),
        )
        .await;

    assert_eq!(response.status(), 413);
    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(body["error"], "payload_too_large");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("limit is 4")
    );
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
        body["authorization_servers"][0], server.base,
        "authorization_servers must be this origin so Claude finds DCR"
    );
    assert_eq!(body["bearer_methods_supported"][0], "header");
    assert!(
        body["scopes_supported"]
            .as_array()
            .expect("scopes")
            .iter()
            .any(|s| s == "render"),
        "scopes_supported must still advertise render: {body}"
    );
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
                // Nested ranges on purpose: one `range(300000000)` is a memory bomb
                // (Typst materialises the array), and under the worker's RLIMIT on
                // Linux it dies allocating — 500, not the 504 this test is about.
                // See the RUNAWAY constant in tests/sandbox.rs.
                "source": "#let acc = 0\n#for i in range(20000) { for j in range(20000) { acc = acc + 1 } }\n#acc",
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
async fn oauth_as_metadata_and_dcr_are_public() {
    let server = TestServer::start_with(&[
        ("OIDC_ISSUER", "https://login.microsoftonline.com/abc/v2.0"),
        ("OIDC_AUDIENCE", "api://typst-mcp"),
        ("DCR_CLIENT_ID", "e20d1345-7e4e-4298-bf8d-6d4606b1ecb4"),
        (
            "OAUTH_REDIRECT_URIS",
            "https://claude.ai/api/mcp/auth_callback,https://claude.com/api/mcp/auth_callback,https://www.cursor.com/agents/mcp/oauth/callback,cursor://anysphere.cursor-mcp/oauth/callback,grokbot://mcp/oauth/callback,http://localhost:8787/callback",
        ),
    ])
    .await;

    let as_meta = server
        .get("/.well-known/oauth-authorization-server", None)
        .await;
    assert_eq!(
        as_meta.status(),
        200,
        "AS metadata must not require a bearer"
    );
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
    assert_eq!(body["token_endpoint"], format!("{}/token", server.base));
    assert!(
        body["code_challenge_methods_supported"]
            .as_array()
            .expect("pkce")
            .iter()
            .any(|m| m == "S256")
    );

    let created = server
        .client
        .post(server.url("/register"))
        .json(&serde_json::json!({
            "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"]
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(created.status(), 201, "DCR must not require a bearer");
    let reg: serde_json::Value = created.json().await.expect("json");
    assert_eq!(reg["client_id"], "e20d1345-7e4e-4298-bf8d-6d4606b1ecb4");
    assert_eq!(reg["token_endpoint_auth_method"], "none");

    let rejected = server
        .client
        .post(server.url("/register"))
        .json(&serde_json::json!({
            "redirect_uris": ["https://attacker.example/cb"]
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(rejected.status(), 400);

    let mcp = server.get("/mcp", None).await;
    assert_eq!(mcp.status(), 401);
}

/// A native client does not agree with us about where discovery lives. Claude
/// reads the bare RFC 8414 path, others path-insert after the resource or ask
/// for the OIDC spelling. All four must answer, and answer the same thing —
/// a client that probes the "wrong" one must not conclude there is no
/// authorization server and fall back to "automatic registration unsupported".
#[tokio::test]
async fn every_discovery_path_a_native_client_probes_answers() {
    let server = TestServer::start_with(&[
        ("OIDC_ISSUER", "https://login.microsoftonline.com/abc/v2.0"),
        ("OIDC_AUDIENCE", "api://typst-mcp"),
        ("DCR_CLIENT_ID", "e20d1345-7e4e-4298-bf8d-6d4606b1ecb4"),
        ("OAUTH_REDIRECT_URIS", "grokbot://mcp/oauth/callback"),
    ])
    .await;

    for path in [
        "/.well-known/oauth-authorization-server",
        "/.well-known/oauth-authorization-server/mcp",
        "/.well-known/openid-configuration",
        "/.well-known/openid-configuration/mcp",
    ] {
        let response = server.get(path, None).await;
        assert_eq!(response.status(), 200, "{path} must be public");
        let body: serde_json::Value = response.json().await.expect("json");
        assert_eq!(body["issuer"], server.base, "{path}");
        assert_eq!(
            body["registration_endpoint"],
            format!("{}/register", server.base),
            "{path}"
        );
    }
}

/// Grok Bot and Cursor call back to a private-use scheme. Entra cannot be the
/// party that accepts those — it only ever sees our `/oauth/callback` — so the
/// DCR shim and the authorize proxy are what must treat them as first-class.
#[tokio::test]
async fn native_client_callbacks_register_and_authorize() {
    let server = TestServer::start_with(&[
        ("OIDC_ISSUER", "https://login.microsoftonline.com/abc/v2.0"),
        ("OIDC_AUDIENCE", "api://typst-mcp"),
        ("DCR_CLIENT_ID", "e20d1345-7e4e-4298-bf8d-6d4606b1ecb4"),
        (
            "OAUTH_REDIRECT_URIS",
            "grokbot://mcp/oauth/callback,\
             cursor://anysphere.cursor-mcp/oauth/callback,\
             http://localhost:8787/callback,\
             https://www.cursor.com/agents/mcp/oauth/callback",
        ),
    ])
    .await;

    for uri in [
        "grokbot://mcp/oauth/callback",
        "cursor://anysphere.cursor-mcp/oauth/callback",
        "http://localhost:8787/callback",
        "https://www.cursor.com/agents/mcp/oauth/callback",
    ] {
        let created = server
            .client
            .post(server.url("/register"))
            .json(&serde_json::json!({ "redirect_uris": [uri] }))
            .send()
            .await
            .expect("register");
        assert_eq!(created.status(), 201, "DCR must accept {uri}");
        let reg: serde_json::Value = created.json().await.expect("json");
        assert_eq!(reg["redirect_uris"][0], uri);

        // ...and the authorize proxy must send the browser to Entra with our
        // own callback substituted, never the private-use scheme. A client that
        // followed the redirect would leave the test and hit real Entra, so this
        // one stops at the 303.
        let no_follow = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client");
        let authorize = no_follow
            .get(server.url(&format!(
                "/authorize?response_type=code&client_id=e20d1345-7e4e-4298-bf8d-6d4606b1ecb4\
                 &redirect_uri={}&state=s&code_challenge=x&code_challenge_method=S256",
                urlencoding(uri)
            )))
            .send()
            .await
            .expect("authorize");
        assert_eq!(authorize.status(), 303, "authorize must redirect for {uri}");
        let location = authorize
            .headers()
            .get("location")
            .expect("location")
            .to_str()
            .expect("ascii");
        assert!(
            location.starts_with("https://login.microsoftonline.com/abc/oauth2/v2.0/authorize?"),
            "{uri} -> {location}"
        );
        assert!(
            location.contains(&urlencoding(&format!("{}/oauth/callback", server.base))),
            "Entra must be handed our callback, not {uri}: {location}"
        );
    }

    // An unlisted scheme is still refused — the allowlist is the control, not
    // the scheme itself.
    let rejected = server
        .client
        .post(server.url("/register"))
        .json(&serde_json::json!({ "redirect_uris": ["evil://mcp/oauth/callback"] }))
        .send()
        .await
        .expect("register");
    assert_eq!(rejected.status(), 400);
}

/// Percent-encode a query-parameter value. Hand-rolled rather than pulled from
/// `url`, which is a normal dependency and so invisible to an integration test.
fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[tokio::test]
async fn unauthenticated_mcp_is_401() {
    let server = TestServer::start_with(&[
        ("OIDC_ISSUER", "https://login.microsoftonline.com/abc/v2.0"),
        ("OIDC_AUDIENCE", "api://typst-mcp"),
    ])
    .await;
    assert_eq!(server.get("/mcp", None).await.status(), 401);
    assert_eq!(server.get("/health", None).await.status(), 200);
}
