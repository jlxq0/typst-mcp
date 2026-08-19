//! The MCP tool surface.
//!
//! Shaped around one loop: discover a template, render it, *look at the result*, fix
//! it, render again. The preview image block is what makes that possible — a model
//! that cannot see its own output writes plausible Typst and ships a broken layout
//! with complete confidence.
//!
//! Every tool goes through [`RenderService`], the same path the REST API uses, so the
//! two surfaces cannot disagree about what is valid or what gets stored.

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
use crate::config::Config;
use crate::principal::TenantId;
use crate::render::{RenderError, RenderRequest, RenderService};
use crate::store::Kind;
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
    /// Ids of previously uploaded assets to make available to the document.
    #[serde(default)]
    pub assets: Vec<String>,
    /// 1-based pages to return as images. Defaults to the first page.
    #[serde(default)]
    pub preview_pages: Option<Vec<usize>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompileArgs {
    /// A complete Typst document.
    pub source: String,
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FontArgs {
    /// Case-insensitive substring to filter family names by.
    #[serde(default)]
    pub query: Option<String>,
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
        ctx.extensions
            .get::<http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Authenticated>())
            .map(|caller| caller.principal.tenant(&self.config.tenant_salt))
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
            preview_pages: clamp_pages(args.preview_pages),
            ..Default::default()
        };
        self.run(&self.tenant(&ctx)?, request).await
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
            source: Some(args.source),
            assets: args.assets,
            preview_pages: clamp_pages(args.preview_pages),
            ..Default::default()
        };
        self.run(&self.tenant(&ctx)?, request).await
    }

    #[tool(
        description = "List the available document templates, with the data fields each \
                       one takes."
    )]
    async fn typst_templates(&self) -> Result<CallToolResult, McpError> {
        let templates: Vec<serde_json::Value> = self
            .render
            .templates()
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "kind": kind_name(t.manifest.kind),
                    "description": t.manifest.description,
                    "data_fields": t.data_fields(),
                    "notice": t.manifest.notice,
                })
            })
            .collect();
        json_result(serde_json::json!({ "templates": templates }))
    }

    #[tool(
        description = "Get a template's full JSON Schema and a worked example. Call this \
                       before rendering a template you have not used before."
    )]
    async fn typst_template_schema(
        &self,
        Parameters(args): Parameters<TemplateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let Some(template) = self.render.templates().get(&args.template) else {
            // Naming the alternatives turns a dead end into a next step.
            return json_result(serde_json::json!({
                "error": "unknown_template",
                "available": self.render.templates().names(),
            }));
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
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let assets = self.render.store().list(&self.tenant(&ctx)?, Kind::Asset);
        json_result(serde_json::json!({ "assets": assets }))
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
            Err(err) => json_result(serde_json::json!({
                "error": "not_available",
                "message": err.to_string(),
            })),
        }
    }

    /// Run a render and shape the result for a model.
    async fn run(
        &self,
        tenant: &TenantId,
        request: RenderRequest,
    ) -> Result<CallToolResult, McpError> {
        match self.render.render(tenant, &request).await {
            Ok(result) => {
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
            // A document that failed to compile is not a tool error: the diagnostics
            // are the useful result, and the model is expected to read them and try
            // again. Reporting it as a protocol error would throw them away.
            Err(err) => {
                let body = serde_json::json!({
                    "ok": false,
                    "error": error_code(&err),
                    "message": err.to_string(),
                    "diagnostics": err.diagnostics(),
                });
                Ok(CallToolResult::error(vec![ContentBlock::json(body)?]))
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

fn error_code(err: &RenderError) -> &'static str {
    match err {
        RenderError::UnknownTemplate { .. } => "unknown_template",
        RenderError::Ambiguous | RenderError::Empty => "bad_request",
        RenderError::Template(_) => "invalid_data",
        RenderError::DuplicateAsset(_) | RenderError::Bundle(_) => "invalid_bundle",
        RenderError::Compile { .. } => "compile_failed",
        RenderError::Store(_) => "not_found",
        RenderError::Spawn(crate::spawn::SpawnError::Timeout { .. }) => "timeout",
        RenderError::Spawn(crate::spawn::SpawnError::Overloaded) => "overloaded",
        RenderError::Spawn(_) | RenderError::Protocol(_) => "internal",
    }
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
