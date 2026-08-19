//! The MCP tool surface.
//!
//! Shaped around one loop: discover a template, render it, *look at the result*, fix
//! it, render again. The preview image block is what makes that possible — a model
//! that cannot see its own output writes plausible Typst and ships a broken layout
//! with complete confidence.
//!
//! Every tool goes through [`RenderService`], the same path the REST API uses, so the
//! two surfaces cannot disagree about what is valid or what gets stored.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;

use rmcp::RoleServer;
use rmcp::service::RequestContext;

use crate::auth::Authenticated;
use crate::bundle::BundleFile;
use crate::config::Config;
use crate::error::ApiError;
use crate::principal::TenantId;
use crate::render::{InputFile, RenderRequest, RenderService};
use crate::store::{AssetRole, Entry, Kind};
use crate::templates::TemplateKind;

/// The protocol revision this server speaks.
///
/// Set explicitly rather than taking `ProtocolVersion::LATEST`: rmcp 3.x knows
/// 2026-07-28 but still defaults to 2025-11-25 (upstream PR #1105). Pinning it here
/// means the negotiated version is a decision rather than a side effect of a
/// dependency bump.
const PROTOCOL: ProtocolVersion = ProtocolVersion::V_2026_07_28;

/// Cap on preview pages returned in one call.
///
/// Each page is a base64 PNG in the response, and an image costs roughly 1.37 tokens
/// per byte — twice, since it stays in the transcript. Ten full-page previews would
/// cost more than the rest of the conversation put together.
const MAX_PREVIEW_PAGES: usize = 4;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenderArgs {
    /// Template name, e.g. `hanso`. Use `typst_templates` to see what is available.
    pub template: String,
    /// Metadata for the template, matching its schema. See `typst_template_schema`.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    /// Typst markup for the template to wrap. Required for `wrapper` templates.
    ///
    /// Write ordinary Typst: `= Chapter`, `== Section`, `- bullet`, `#figure(..)`.
    /// The template's own helpers and brand colours are in scope.
    #[serde(default)]
    pub body: Option<String>,
    /// `sys.inputs` values available to the template. Strings only, per Typst.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// Replacements for existing text files in the template.
    #[serde(default)]
    pub overrides: Vec<TemplateTextFile>,
    /// Ids of previously uploaded assets to make available to the document.
    #[serde(default)]
    pub assets: Vec<String>,
    /// 1-based pages to return as images. Defaults to the first page.
    #[serde(default)]
    pub preview_pages: Option<Vec<usize>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompileArgs {
    /// A complete Typst document, shorthand for one `main.typ` file.
    #[serde(default)]
    pub source: Option<String>,
    /// A multi-file text bundle. Binary data comes from uploaded assets.
    #[serde(default)]
    pub files: Vec<TemplateTextFile>,
    /// Entrypoint within `files`. Defaults to `main.typ`.
    #[serde(default)]
    pub main: Option<String>,
    /// Structured data mounted as `data.json`.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    /// `sys.inputs` values. Strings only, per Typst.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// Ids of previously uploaded assets to make available to the document.
    #[serde(default)]
    pub assets: Vec<String>,
    /// 1-based pages to return as images. Defaults to the first page.
    #[serde(default)]
    pub preview_pages: Option<Vec<usize>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TemplateArgs {
    /// The template's name.
    pub template: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct TemplateListArgs {
    /// Include this caller's live ephemeral templates. Defaults to true.
    #[serde(default)]
    pub include_ephemeral: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TemplateTextFile {
    /// Bundle-relative path such as `template.toml` or `draft.typ`.
    pub path: String,
    /// UTF-8 text. Binary files must be uploaded through REST first.
    pub text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UploadTemplateArgs {
    /// Draft name; must exactly match `name` in template.toml.
    pub name: String,
    /// Complete text source tree, including template.toml.
    pub files: Vec<TemplateTextFile>,
    /// Existing tenant-scoped binary assets to copy into this template.
    #[serde(default)]
    pub assets: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FontArgs {
    /// Case-insensitive substring to filter family names by.
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct AssetArgs {
    /// Optional role filter: `image`, `font`, or `data`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Maximum number returned. Defaults to 100 and is capped at 1000.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LinkArgs {
    /// A `job_id` from a previous render.
    pub job_id: String,
    /// How long the link should stay valid. Clamped to the server's maximum.
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
}

/// The MCP service.
///
/// Holds no caller identity. The tenant is derived from each request's own
/// authenticated principal — see [`TypstMcp::tenant`] — because rmcp builds one
/// service per session and a session is not a caller. Caching the tenant on the
/// service would make two concurrent sessions able to swap identities.
#[derive(Clone)]
pub struct TypstMcp {
    render: Arc<RenderService>,
    config: Arc<Config>,
    tool_router: ToolRouter<TypstMcp>,
}

#[tool_router]
impl TypstMcp {
    pub fn new(render: Arc<RenderService>, config: Arc<Config>) -> Self {
        Self {
            render,
            config,
            tool_router: Self::tool_router(),
        }
    }

    /// The calling tenant for this request.
    ///
    /// rmcp injects the HTTP request's `Parts` into the tool's context, and the auth
    /// middleware has already put an [`Authenticated`] there. Reading it per request
    /// is what keeps two concurrent sessions from ever seeing each other's storage.
    ///
    /// An absent principal is an error rather than a default: the only way to get here
    /// without one is a routing mistake, and inventing a tenant would turn that mistake
    /// into silent cross-tenant access.
    fn tenant(&self, ctx: &RequestContext<RoleServer>) -> Result<TenantId, McpError> {
        Ok(self.caller(ctx)?.principal.tenant(&self.config.tenant_salt))
    }

    fn caller(&self, ctx: &RequestContext<RoleServer>) -> Result<Authenticated, McpError> {
        ctx.extensions
            .get::<http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Authenticated>())
            .cloned()
            .ok_or_else(|| {
                McpError::invalid_request("this request carried no authenticated caller", None)
            })
    }

    #[tool(
        description = "Render a branded document from a named template and return a link \
                       plus a preview image of the result. Look at the preview: it is how \
                       you catch a layout problem before handing the document over."
    )]
    async fn typst_render(
        &self,
        Parameters(args): Parameters<RenderArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let request = RenderRequest {
            template: Some(args.template),
            data: args.data,
            body: args.body,
            assets: args.assets,
            inputs: args.inputs,
            overrides: args
                .overrides
                .into_iter()
                .map(|file| InputFile {
                    path: file.path,
                    text: file.text,
                })
                .collect(),
            preview_pages: clamp_pages(args.preview_pages),
            ..Default::default()
        };
        let caller = self.caller(&ctx)?;
        self.run(
            &caller.principal.tenant(&self.config.tenant_salt),
            &caller.fingerprint,
            "typst_render",
            request,
        )
        .await
    }

    #[tool(
        description = "Compile a Typst document with no template. Use this for one-off \
                       documents, or to try something out before committing it to a template."
    )]
    async fn typst_compile(
        &self,
        Parameters(args): Parameters<CompileArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let request = RenderRequest {
            source: args.source,
            files: args
                .files
                .into_iter()
                .map(|file| InputFile {
                    path: file.path,
                    text: file.text,
                })
                .collect(),
            main: args.main,
            data: args.data,
            inputs: args.inputs,
            assets: args.assets,
            preview_pages: clamp_pages(args.preview_pages),
            ..Default::default()
        };
        let caller = self.caller(&ctx)?;
        self.run(
            &caller.principal.tenant(&self.config.tenant_salt),
            &caller.fingerprint,
            "typst_compile",
            request,
        )
        .await
    }

    #[tool(
        description = "List the available document templates, with the data fields each \
                       one takes."
    )]
    async fn typst_templates(
        &self,
        Parameters(args): Parameters<TemplateListArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = self.tenant(&ctx)?;
        let mut templates: Vec<serde_json::Value> = self
            .render
            .templates()
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.name(),
                    "name": t.name(),
                    "kind": "baked",
                    "template_kind": kind_name(t.manifest.kind),
                    "version": t.manifest.version,
                    "description": t.manifest.description,
                    "data_fields": t.data_fields(),
                    "notice": t.manifest.notice,
                })
            })
            .collect();
        if args.include_ephemeral.unwrap_or(true) {
            for entry in self.render.store().list(&tenant, Kind::Template) {
                let template = match self.render.template(&tenant, &entry.id) {
                    Ok(template) => template,
                    Err(error) => return ApiError::from(error).into_mcp_result(),
                };
                templates.push(serde_json::json!({
                    "id": entry.id,
                    "name": template.name(),
                    "kind": "ephemeral",
                    "template_kind": kind_name(template.manifest.kind),
                    "version": template.manifest.version,
                    "description": template.manifest.description,
                    "data_fields": template.data_fields(),
                    "notice": template.manifest.notice,
                    "expires_at": entry.expires_at,
                }));
            }
        }
        json_result(serde_json::json!({ "templates": templates }))
    }

    #[tool(
        description = "Get a template's full JSON Schema and a worked example. Call this \
                       before rendering a template you have not used before."
    )]
    async fn typst_template_schema(
        &self,
        Parameters(args): Parameters<TemplateArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let template = match self.render.template(&self.tenant(&ctx)?, &args.template) {
            Ok(template) => template,
            Err(error) => return ApiError::from(error).into_mcp_result(),
        };
        json_result(serde_json::json!({
            "name": template.name(),
            "kind": kind_name(template.manifest.kind),
            "description": template.manifest.description,
            "notice": template.manifest.notice,
            "schema": template.schema(),
            "example_data": template.example(),
            "example_body": template.example_body(),
        }))
    }

    #[tool(
        description = "Create a tenant-scoped ephemeral template from text files. Include \
                       template.toml and the Typst sources; upload binary assets through \
                       REST first and bind their ids here. A supplied fixture is compiled \
                       before the draft is stored."
    )]
    async fn typst_upload_template(
        &self,
        Parameters(args): Parameters<UploadTemplateArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let started = std::time::Instant::now();
        let caller = self.caller(&ctx)?;
        let tenant = caller.principal.tenant(&self.config.tenant_salt);
        let template_name = args.name.clone();
        let source_bytes = args.files.iter().map(|file| file.text.len()).sum();
        let files = args
            .files
            .into_iter()
            .map(|file| BundleFile::text(file.path, file.text))
            .collect();
        match self
            .render
            .upload_template(&tenant, Some(&args.name), files, &args.assets)
            .await
        {
            Ok(uploaded) => {
                crate::audit::AuditEvent {
                    tenant_fp: &caller.fingerprint,
                    operation: "typst_upload_template",
                    job_id: None,
                    template: Some(&template_name),
                    bytes: source_bytes,
                    pages: 0,
                    duration_ms: started.elapsed().as_millis(),
                    outcome: "success",
                    diagnostic_count: uploaded.warnings.len(),
                }
                .emit();
                json_result(
                    serde_json::to_value(uploaded)
                        .map_err(|error| McpError::internal_error(error.to_string(), None))?,
                )
            }
            Err(error) => {
                crate::audit::AuditEvent {
                    tenant_fp: &caller.fingerprint,
                    operation: "typst_upload_template",
                    job_id: None,
                    template: Some(&template_name),
                    bytes: source_bytes,
                    pages: 0,
                    duration_ms: started.elapsed().as_millis(),
                    outcome: "error",
                    diagnostic_count: error.diagnostics().len(),
                }
                .emit();
                ApiError::from(error).into_mcp_result()
            }
        }
    }

    #[tool(
        description = "List the font families installed on this server. Use it instead of \
                       guessing a font name: an unavailable family is substituted silently, \
                       and the document just comes out looking wrong."
    )]
    async fn typst_fonts(
        &self,
        Parameters(args): Parameters<FontArgs>,
    ) -> Result<CallToolResult, McpError> {
        let fonts = crate::fonts::FontLibrary::new(&self.config.font_dirs);
        let needle = args.query.map(|q| q.to_lowercase());
        let families: Vec<String> = fonts
            .families()
            .into_iter()
            .filter(|f| {
                needle
                    .as_ref()
                    .is_none_or(|n| f.name.to_lowercase().contains(n))
            })
            .map(|f| f.name)
            .collect();
        json_result(serde_json::json!({ "total": families.len(), "families": families }))
    }

    #[tool(
        description = "List the assets (images, fonts, data files) uploaded for this caller, \
                       with the ids to reference them by."
    )]
    async fn typst_assets(
        &self,
        Parameters(args): Parameters<AssetArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let filter = match args.kind.as_deref() {
            Some("image") => Some(AssetRole::Image),
            Some("font") => Some(AssetRole::Font),
            Some("data") => Some(AssetRole::Data),
            Some(_) => {
                return ApiError::bad_request("kind must be `image`, `font`, or `data`")
                    .into_mcp_result();
            }
            None => None,
        };
        let limit = args.limit.unwrap_or(100).min(1000);
        let mut assets: Vec<_> = self
            .render
            .store()
            .list(&self.tenant(&ctx)?, Kind::Asset)
            .into_iter()
            .filter(|entry| filter.is_none_or(|role| asset_role(entry) == role))
            .map(|entry| {
                let role = asset_role(&entry).as_str();
                serde_json::json!({
                    "id": entry.id,
                    "filename": entry.filename,
                    "content_type": entry.content_type,
                    "bytes": entry.bytes,
                    "kind": role,
                    "created_at": entry.created_at,
                    "expires_at": entry.expires_at,
                })
            })
            .collect();
        let total = assets.len();
        assets.truncate(limit);
        json_result(serde_json::json!({ "total": total, "assets": assets }))
    }

    #[tool(
        description = "Mint a fresh link to a rendered document that opens in a browser \
                       without a credential. Use this to hand a document to a person."
    )]
    async fn typst_link(
        &self,
        Parameters(args): Parameters<LinkArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .render
            .link(&self.tenant(&ctx)?, &args.job_id, args.ttl_seconds)
        {
            Ok((url, expires_at)) => {
                json_result(serde_json::json!({ "url": url, "expires_at": expires_at }))
            }
            Err(err) => ApiError::from(err).into_mcp_result(),
        }
    }

    /// Run a render and shape the result for a model.
    async fn run(
        &self,
        tenant: &TenantId,
        tenant_fp: &str,
        operation: &'static str,
        request: RenderRequest,
    ) -> Result<CallToolResult, McpError> {
        let started = std::time::Instant::now();
        let template = request.template.clone();
        match self.render.render(tenant, &request).await {
            Ok(result) => {
                crate::audit::AuditEvent {
                    tenant_fp,
                    operation,
                    job_id: Some(&result.job_id),
                    template: template.as_deref(),
                    bytes: result.bytes,
                    pages: result.pages,
                    duration_ms: started.elapsed().as_millis(),
                    outcome: "success",
                    diagnostic_count: result.diagnostics.len(),
                }
                .emit();
                let mut blocks = vec![ContentBlock::json(serde_json::json!({
                    "job_id": result.job_id,
                    "url": result.url,
                    "pages": result.pages,
                    "bytes": result.bytes,
                    "expires_at": result.expires_at,
                    "diagnostics": result.diagnostics,
                }))?];
                // The point of the whole tool: the model sees the page it produced.
                for preview in &result.previews {
                    blocks.push(ContentBlock::image(
                        BASE64.encode(&preview.png),
                        "image/png",
                    ));
                }
                Ok(CallToolResult::success(blocks))
            }
            // Domain failures are tool-error content, not JSON-RPC transport errors:
            // the diagnostics remain available for the model to fix and retry.
            Err(err) => {
                crate::audit::AuditEvent {
                    tenant_fp,
                    operation,
                    job_id: None,
                    template: template.as_deref(),
                    bytes: 0,
                    pages: 0,
                    duration_ms: started.elapsed().as_millis(),
                    outcome: "error",
                    diagnostic_count: err.diagnostics().len(),
                }
                .emit();
                ApiError::from(err).into_mcp_result()
            }
        }
    }
}

// `router = self.tool_router` rather than the macro's default `Self::tool_router()`,
// which would rebuild the whole tool list on every single call.
#[tool_handler(router = self.tool_router)]
impl ServerHandler for TypstMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(PROTOCOL)
            .with_instructions(
                "Renders branded PDFs with Typst.\n\n\
                 Start with `typst_templates` to see what is available, then \
                 `typst_template_schema` for the one you want. `typst_render` takes the \
                 template's metadata as `data` and your content as `body` — ordinary Typst \
                 markup, with the template's brand colours and helpers already in scope.\n\n\
                 Every render returns a preview image. Look at it. It is the only way to \
                 catch a layout problem before handing the document to someone.\n\n\
                 Compile errors come back with a file, a line and a column pointing at your \
                 own markup: read them and try again. Use `typst_link` to produce a link a \
                 person can open."
                    .to_string(),
            )
    }
}

fn kind_name(kind: TemplateKind) -> &'static str {
    match kind {
        TemplateKind::Wrapper => "wrapper",
        TemplateKind::Data => "data",
    }
}

fn asset_role(entry: &Entry) -> AssetRole {
    entry.asset_role.unwrap_or_else(|| {
        let path = entry.filename.as_deref().unwrap_or_default();
        let extension = path.rsplit('.').next().unwrap_or_default();
        if matches!(
            extension.to_ascii_lowercase().as_str(),
            "ttf" | "otf" | "ttc"
        ) {
            AssetRole::Font
        } else if entry
            .content_type
            .as_deref()
            .is_some_and(|content_type| content_type.starts_with("image/"))
        {
            AssetRole::Image
        } else {
            AssetRole::Data
        }
    })
}

/// Bound how many pages a caller can ask to see at once.
fn clamp_pages(pages: Option<Vec<usize>>) -> Option<Vec<usize>> {
    pages.map(|mut pages| {
        pages.truncate(MAX_PREVIEW_PAGES);
        pages
    })
}

fn json_result(value: serde_json::Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::json(value)?]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_protocol_version_is_pinned_to_the_current_spec() {
        // Guards against a dependency bump silently changing what we negotiate.
        assert_eq!(PROTOCOL, ProtocolVersion::V_2026_07_28);
    }

    #[test]
    fn preview_pages_are_capped() {
        // Each page is a base64 PNG in the response and costs roughly 1.37 tokens per
        // byte, twice. An unbounded list would dwarf the rest of the conversation.
        let asked = Some((1..=20).collect::<Vec<usize>>());
        assert_eq!(clamp_pages(asked).expect("some").len(), MAX_PREVIEW_PAGES);
        assert_eq!(clamp_pages(None), None);
        assert_eq!(clamp_pages(Some(vec![2])), Some(vec![2]));
    }

    #[test]
    fn render_arguments_deserialize_from_the_documented_shape() {
        let args: RenderArgs = serde_json::from_value(serde_json::json!({
            "template": "hanso",
            "data": { "title": "Q3 Review" },
            "body": "= Chapter",
        }))
        .expect("deserializes");
        assert_eq!(args.template, "hanso");
        assert!(args.assets.is_empty());
        assert_eq!(args.preview_pages, None);
    }

    #[test]
    fn only_the_template_name_is_required() {
        // A model should be able to call this with the minimum and let the template's
        // own defaults fill in the rest.
        let args: RenderArgs =
            serde_json::from_value(serde_json::json!({ "template": "hanso" })).expect("minimal");
        assert!(args.data.is_none() && args.body.is_none());
    }
}
