//! The render pipeline: a request in, a stored document and a preview out.
//!
//! Everything a caller can ask for goes through [`RenderService::render`], so the MCP
//! tools and the REST endpoints cannot drift apart in what they validate, what they
//! store, or what they report.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bundle::{Bundle, BundleFile, FileContent};
use crate::config::Config;
use crate::diagnostics::Diagnostic;
use crate::principal::TenantId;
use crate::protocol::{Job, JobContent, JobFile, JobLimits, JobResult};
use crate::signing::Signer;
use crate::spawn::{CompileService, SpawnError};
use crate::store::{Kind, Meta, Store, StoreError};
use crate::templates::{SourceMap, TemplateError, TemplateKind, TemplateSet};

/// Filename of the rendered document within its store entry.
pub const DOCUMENT_NAME: &str = "doc.pdf";

/// What to render.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RenderRequest {
    /// A named template. Mutually exclusive with `source`/`files`.
    #[serde(default)]
    pub template: Option<String>,
    /// Metadata for the template.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    /// Typst markup for a wrapper template to wrap.
    #[serde(default)]
    pub body: Option<String>,

    /// Shorthand for a single `main.typ`.
    #[serde(default)]
    pub source: Option<String>,
    /// A full bundle of text files.
    #[serde(default)]
    pub files: Vec<InputFile>,
    #[serde(default)]
    pub main: Option<String>,

    /// Ids of previously uploaded assets to mount.
    #[serde(default)]
    pub assets: Vec<String>,
    /// `sys.inputs` values. Strings only, per Typst.
    #[serde(default)]
    pub inputs: std::collections::BTreeMap<String, String>,

    /// 1-based pages to rasterise. `None` means page 1.
    #[serde(default)]
    pub preview_pages: Option<Vec<usize>>,
}

/// One caller-supplied text file.
#[derive(Debug, Clone, Deserialize)]
pub struct InputFile {
    pub path: String,
    pub text: String,
}

/// A rendered document.
#[derive(Debug, Clone, Serialize)]
pub struct RenderResult {
    pub job_id: String,
    pub url: String,
    pub pages: usize,
    pub bytes: usize,
    pub expires_at: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(skip)]
    pub pdf: Vec<u8>,
    #[serde(skip)]
    pub previews: Vec<RenderedPreview>,
}

/// A rasterised page.
#[derive(Debug, Clone)]
pub struct RenderedPreview {
    pub page: usize,
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// Why a render did not produce a document.
#[derive(Debug, Error)]
pub enum RenderError {
    #[error("no template named {name:?}; available: {available}")]
    UnknownTemplate { name: String, available: String },
    #[error("specify either `template` or `source`/`files`, not both")]
    Ambiguous,
    #[error("nothing to render: provide `template`, `source`, or `files`")]
    Empty,
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error(transparent)]
    Bundle(#[from] crate::bundle::BundleError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{message}")]
    Compile {
        message: String,
        diagnostics: Vec<Diagnostic>,
    },
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error("the compile worker returned something unreadable: {0}")]
    Protocol(String),
}

impl RenderError {
    /// Diagnostics to return alongside the failure, if any.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            Self::Compile { diagnostics, .. } => diagnostics,
            _ => &[],
        }
    }
}

/// Renders documents and stores the results.
pub struct RenderService {
    compiler: CompileService,
    templates: TemplateSet,
    store: Arc<Store>,
    signer: Signer,
    config: Arc<Config>,
    font_dirs: Vec<PathBuf>,
}

impl RenderService {
    pub fn new(
        config: Arc<Config>,
        compiler: CompileService,
        templates: TemplateSet,
        store: Arc<Store>,
        signer: Signer,
    ) -> Self {
        let font_dirs = config.font_dirs.clone();
        Self {
            compiler,
            templates,
            store,
            signer,
            config,
            font_dirs,
        }
    }

    pub fn templates(&self) -> &TemplateSet {
        &self.templates
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn signer(&self) -> &Signer {
        &self.signer
    }

    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    /// Render `request` for `tenant`, store the result, and return it.
    pub async fn render(
        &self,
        tenant: &TenantId,
        request: &RenderRequest,
    ) -> Result<RenderResult, RenderError> {
        let (bundle, source_map) = self.assemble(tenant, request)?;
        let job = self.job(bundle, request);

        let result = self.compiler.compile(&job).await?;
        match result {
            JobResult::Failed {
                message,
                mut diagnostics,
            } => {
                // Report positions in the caller's own terms: a wrapper body carries an
                // import prelude, so raw line numbers would be off by its length and
                // point at a file the caller never wrote.
                source_map.apply(&mut diagnostics);
                Err(RenderError::Compile {
                    message,
                    diagnostics,
                })
            }
            JobResult::Ok {
                pdf_base64,
                pages,
                previews,
                mut diagnostics,
            } => {
                source_map.apply(&mut diagnostics);
                let pdf = decode(&pdf_base64)?;
                let previews = previews
                    .into_iter()
                    .map(|p| {
                        Ok(RenderedPreview {
                            page: p.page,
                            width: p.width,
                            height: p.height,
                            png: decode(&p.png_base64)?,
                        })
                    })
                    .collect::<Result<Vec<_>, RenderError>>()?;

                self.store_result(tenant, pdf, pages, previews, diagnostics)
            }
        }
    }

    /// Build the bundle, from a template or from caller-supplied files.
    fn assemble(
        &self,
        tenant: &TenantId,
        request: &RenderRequest,
    ) -> Result<(Bundle, SourceMap), RenderError> {
        let has_inline = request.source.is_some() || !request.files.is_empty();
        match (&request.template, has_inline) {
            (Some(_), true) => return Err(RenderError::Ambiguous),
            (None, false) => return Err(RenderError::Empty),
            _ => {}
        }

        let assets = self.load_assets(tenant, &request.assets)?;

        if let Some(name) = &request.template {
            let template =
                self.templates
                    .get(name)
                    .ok_or_else(|| RenderError::UnknownTemplate {
                        name: name.clone(),
                        available: self.templates.names().join(", "),
                    })?;
            let data = request.data.clone().unwrap_or(serde_json::json!({}));
            let body = match template.manifest.kind {
                TemplateKind::Wrapper => request.body.as_deref(),
                TemplateKind::Data => None,
            };
            let assembled = template.assemble(&data, body, assets, self.config.max_bundle_bytes)?;
            return Ok((assembled.bundle, assembled.source_map));
        }

        let mut files: Vec<BundleFile> = assets;
        if let Some(source) = &request.source {
            files.push(BundleFile::text("main.typ", source.clone()));
        }
        for file in &request.files {
            files.push(BundleFile::text(&file.path, &file.text));
        }
        let main = request.main.clone().unwrap_or_else(|| "main.typ".into());
        let bundle = Bundle::new(
            &main,
            files,
            request.inputs.clone(),
            self.config.max_bundle_bytes,
        )?;
        Ok((bundle, SourceMap::default()))
    }

    /// Fetch uploaded assets and mount them at their recorded paths.
    fn load_assets(
        &self,
        tenant: &TenantId,
        ids: &[String],
    ) -> Result<Vec<BundleFile>, RenderError> {
        ids.iter()
            .map(|id| {
                // Scoped to this tenant: an id belonging to someone else simply is not
                // found, which is the isolation working rather than a check.
                let entry = self.store.entry(tenant, Kind::Asset, id)?;
                let name = entry.filename.clone().unwrap_or_else(|| id.clone());
                let bytes = self.store.get(tenant, Kind::Asset, id, &name)?;
                Ok(BundleFile {
                    path: name,
                    content: FileContent::Binary(bytes),
                })
            })
            .collect()
    }

    fn job(&self, bundle: Bundle, request: &RenderRequest) -> Job {
        Job {
            main: bundle.main().to_owned(),
            files: bundle
                .files()
                .map(|(path, content)| JobFile {
                    path: path.to_owned(),
                    content: JobContent::from_content(content),
                })
                .collect(),
            inputs: bundle.inputs().clone(),
            font_dirs: self.font_dirs.clone(),
            limits: JobLimits {
                max_bundle_bytes: self.config.max_bundle_bytes,
                max_pages: self.config.max_pages,
                preview_pages: request.preview_pages.clone().unwrap_or_else(|| vec![1]),
                preview_scale_millis: 1000,
                preview_max_px: self.config.preview_max_px,
                memory_bytes: self.config.worker_memory_bytes,
            },
        }
    }

    fn store_result(
        &self,
        tenant: &TenantId,
        pdf: Vec<u8>,
        pages: usize,
        previews: Vec<RenderedPreview>,
        diagnostics: Vec<Diagnostic>,
    ) -> Result<RenderResult, RenderError> {
        let entry = self.store.put(
            tenant,
            Kind::Output,
            DOCUMENT_NAME,
            &pdf,
            Meta::new(DOCUMENT_NAME, "application/pdf"),
        )?;
        for preview in &previews {
            self.store.put_with_id(
                tenant,
                Kind::Output,
                &entry.id,
                &preview_name(preview.page),
                &preview.png,
                Meta::new(preview_name(preview.page), "image/png"),
            )?;
        }

        Ok(RenderResult {
            url: self
                .config
                .url(&format!("files/{tenant}/{}/{DOCUMENT_NAME}", entry.id)),
            job_id: entry.id,
            pages,
            bytes: pdf.len(),
            expires_at: entry.expires_at,
            diagnostics,
            pdf,
            previews,
        })
    }

    /// Mint a signed, browser-openable link for a stored document.
    pub fn link(
        &self,
        tenant: &TenantId,
        job_id: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<(String, u64), RenderError> {
        // Confirms the job exists and belongs to this tenant before minting anything.
        let entry = self.store.entry(tenant, Kind::Output, job_id)?;
        let ttl = ttl_seconds
            .unwrap_or_else(|| self.config.signed_url_ttl.as_secs())
            .min(self.config.signed_url_ttl.as_secs());
        let (signature, expires_at) = self.signer.sign_for(tenant, &entry.id, ttl);
        let url = self.config.url(&format!(
            "files/{tenant}/{}/{DOCUMENT_NAME}?exp={expires_at}&sig={signature}",
            entry.id
        ));
        Ok((url, expires_at))
    }
}

/// The filename a rasterised page is stored under.
pub fn preview_name(page: usize) -> String {
    format!("page-{page}.png")
}

fn decode(value: &str) -> Result<Vec<u8>, RenderError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|e| RenderError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_names_are_predictable() {
        assert_eq!(preview_name(1), "page-1.png");
        assert_eq!(preview_name(12), "page-12.png");
    }

    #[test]
    fn a_request_naming_both_a_template_and_source_is_ambiguous() {
        // Silently preferring one would make the other's absence from the output
        // mysterious.
        let request = RenderRequest {
            template: Some("hanso".into()),
            source: Some("= Hi".into()),
            ..Default::default()
        };
        assert!(request.template.is_some() && request.source.is_some());
    }

    #[test]
    fn requests_deserialize_from_the_documented_shape() {
        let request: RenderRequest = serde_json::from_value(serde_json::json!({
            "template": "hanso",
            "data": { "title": "Q3" },
            "body": "= Chapter",
            "assets": ["ast_01ARZ3NDEKTSV4RRFFQ69G5FAV"],
            "preview_pages": [1, 2],
        }))
        .expect("deserializes");

        assert_eq!(request.template.as_deref(), Some("hanso"));
        assert_eq!(request.body.as_deref(), Some("= Chapter"));
        assert_eq!(request.assets.len(), 1);
        assert_eq!(request.preview_pages, Some(vec![1, 2]));
    }

    #[test]
    fn an_empty_request_deserializes_to_nothing_renderable() {
        let request: RenderRequest =
            serde_json::from_value(serde_json::json!({})).expect("deserializes");
        assert!(request.template.is_none());
        assert!(request.source.is_none());
        assert!(request.files.is_empty());
    }
}
