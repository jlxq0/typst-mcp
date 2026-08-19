//! The render pipeline: a request in, a stored document and a preview out.
//!
//! Everything a caller can ask for goes through [`RenderService::render`], so the MCP
//! tools and the REST endpoints cannot drift apart in what they validate, what they
//! store, or what they report.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bundle::{Bundle, BundleError, BundleFile, FileContent, MAX_FILES};
use crate::config::Config;
use crate::diagnostics::Diagnostic;
use crate::job_io::{PDF_NAME, PreparedJob};
use crate::principal::TenantId;
use crate::protocol::{JobLimits, JobResult};
use crate::signing::Signer;
use crate::spawn::{CompileService, SpawnError};
use crate::store::{Kind, Meta, Store, StoreError};
use crate::templates::{
    STORED_ARCHIVE, SourceMap, Template, TemplateError, TemplateKind, TemplateSet,
};

/// Filename of the rendered document within its store entry.
pub const DOCUMENT_NAME: &str = PDF_NAME;

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
    /// Text replacements for existing files inside a named template.
    #[serde(default)]
    pub overrides: Vec<InputFile>,

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

/// A successful compile before any output is persisted.
#[derive(Debug, Clone)]
pub struct CompiledDocument {
    pub pages: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub pdf: Vec<u8>,
    pub previews: Vec<RenderedPreview>,
}

/// The stable identity and lifetime of an uploaded template.
#[derive(Debug, Clone, Serialize)]
pub struct UploadedTemplate {
    pub id: String,
    pub name: String,
    pub expires_at: u64,
    pub warnings: Vec<String>,
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
    #[error("`overrides` can only be used with a named template")]
    OverridesWithoutTemplate,
    #[error("duplicate asset id {0:?}")]
    DuplicateAsset(String),
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
    #[error("compile workspace I/O failed")]
    Workspace(#[source] std::io::Error),
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

    /// Compile `request` without persisting any output.
    pub async fn compile(
        &self,
        tenant: &TenantId,
        request: &RenderRequest,
    ) -> Result<CompiledDocument, RenderError> {
        let (bundle, source_map) = self.assemble(tenant, request)?;
        self.compile_bundle(bundle, source_map, self.job_limits(request))
            .await
    }

    async fn compile_bundle(
        &self,
        bundle: Bundle,
        source_map: SourceMap,
        limits: JobLimits,
    ) -> Result<CompiledDocument, RenderError> {
        let prepared = PreparedJob::stage(
            &self.config.data_dir.join("tmp"),
            &bundle,
            self.font_dirs.clone(),
            limits,
        )
        .map_err(RenderError::Workspace)?;

        let result = self.compiler.compile(&prepared).await?;
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
                pages,
                previews,
                mut diagnostics,
            } => {
                source_map.apply(&mut diagnostics);
                let outputs = prepared
                    .read_outputs(&previews)
                    .map_err(RenderError::Workspace)?;
                let previews = outputs
                    .previews
                    .into_iter()
                    .map(|preview| RenderedPreview {
                        page: preview.page,
                        width: preview.width,
                        height: preview.height,
                        png: preview.png,
                    })
                    .collect();

                Ok(CompiledDocument {
                    pages,
                    diagnostics,
                    pdf: outputs.pdf,
                    previews,
                })
            }
            JobResult::Internal => Err(RenderError::Protocol(
                "compile worker could not process its staged files".into(),
            )),
        }
    }

    /// Resolve a permanent template name or a tenant-scoped ephemeral id.
    pub fn template(&self, tenant: &TenantId, reference: &str) -> Result<Template, RenderError> {
        // Baked names win by construction. Ephemeral ids cannot collide because the
        // store validates the `tpl_` prefix and baked template names are human names.
        if let Some(template) = self.templates.get(reference) {
            return Ok(template.clone());
        }
        if reference.starts_with(Kind::Template.prefix()) {
            let bytes = self
                .store
                .get(tenant, Kind::Template, reference, STORED_ARCHIVE)?;
            return Ok(Template::from_archive(
                &bytes,
                self.config.max_bundle_bytes,
            )?);
        }
        Err(RenderError::UnknownTemplate {
            name: reference.to_owned(),
            available: self.template_names(tenant).join(", "),
        })
    }

    /// Baked names followed by this tenant's live ephemeral ids.
    pub fn template_names(&self, tenant: &TenantId) -> Vec<String> {
        let mut names: Vec<String> = self
            .templates
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        names.extend(
            self.store
                .list(tenant, Kind::Template)
                .into_iter()
                .map(|entry| entry.id),
        );
        names
    }

    /// Validate, fixture-compile and atomically persist an ephemeral template.
    pub async fn upload_template(
        &self,
        tenant: &TenantId,
        requested_name: Option<&str>,
        mut files: Vec<BundleFile>,
        assets: &[String],
    ) -> Result<UploadedTemplate, RenderError> {
        files.extend(self.load_assets(tenant, assets)?);
        let template = Template::from_source_files(files, self.config.max_bundle_bytes)?;
        if let Some(requested) = requested_name
            && requested != template.name()
        {
            return Err(TemplateError::NameMismatch {
                requested: requested.to_owned(),
                manifest: template.name().to_owned(),
            }
            .into());
        }

        let mut warnings = Vec::new();
        if template.example().is_some() || template.example_body().is_some() {
            let data = template
                .example()
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let body = match template.manifest.kind {
                TemplateKind::Wrapper => template.example_body(),
                TemplateKind::Data => None,
            };
            let assembled =
                template.assemble(&data, body, Vec::new(), self.config.max_bundle_bytes)?;
            let limits = JobLimits {
                max_bundle_bytes: self.config.max_bundle_bytes,
                max_pages: self.config.max_pages,
                preview_pages: Vec::new(),
                preview_scale_millis: 1000,
                preview_max_px: self.config.preview_max_px,
                memory_bytes: self.config.worker_memory_bytes,
            };
            let compiled = self
                .compile_bundle(assembled.bundle, assembled.source_map, limits)
                .await?;
            warnings.extend(compiled.diagnostics.into_iter().map(|d| d.message));
        }

        let archive = template.to_archive()?;
        let entry = self.store.put(
            tenant,
            Kind::Template,
            STORED_ARCHIVE,
            &archive,
            Meta::new(STORED_ARCHIVE, "application/x-tar"),
        )?;
        Ok(UploadedTemplate {
            id: entry.id,
            name: template.name().to_owned(),
            expires_at: entry.expires_at,
            warnings,
        })
    }

    pub fn delete_template(&self, tenant: &TenantId, id: &str) -> Result<(), RenderError> {
        self.store.delete(tenant, Kind::Template, id)?;
        Ok(())
    }

    /// Compile `request`, persist the result, and return stored-document metadata.
    pub async fn render(
        &self,
        tenant: &TenantId,
        request: &RenderRequest,
    ) -> Result<RenderResult, RenderError> {
        let compiled = self.compile(tenant, request).await?;
        self.store_result(
            tenant,
            compiled.pdf,
            compiled.pages,
            compiled.previews,
            compiled.diagnostics,
        )
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
        if request.template.is_none() && !request.overrides.is_empty() {
            return Err(RenderError::OverridesWithoutTemplate);
        }

        let assets = self.load_assets(tenant, &request.assets)?;

        if let Some(name) = &request.template {
            let template = self.template(tenant, name)?;
            let data = request.data.clone().unwrap_or(serde_json::json!({}));
            let body = match template.manifest.kind {
                TemplateKind::Wrapper => request.body.as_deref(),
                TemplateKind::Data => None,
            };
            let overrides = request
                .overrides
                .iter()
                .map(|file| BundleFile::text(&file.path, &file.text))
                .collect();
            let assembled = template.assemble_with_overrides(
                &data,
                body,
                assets,
                overrides,
                self.config.max_bundle_bytes,
            )?;
            return Ok((
                assembled.bundle.with_inputs(request.inputs.clone()),
                assembled.source_map,
            ));
        }

        let mut files: Vec<BundleFile> = assets;
        if let Some(source) = &request.source {
            files.push(BundleFile::text("main.typ", source.clone()));
        }
        for file in &request.files {
            files.push(BundleFile::text(&file.path, &file.text));
        }
        if let Some(data) = &request.data {
            files.push(BundleFile::text(
                "data.json",
                serde_json::to_string(data).unwrap_or_else(|_| "{}".into()),
            ));
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
        // Preflight the complete caller-controlled list using metadata only. Reading as
        // we iterate would let repeated ids allocate the same large file without bound
        // before Bundle::new sees the duplicate path or enforces its aggregate limits.
        if ids.len() > MAX_FILES {
            return Err(BundleError::TooManyFiles { count: ids.len() }.into());
        }

        let mut seen = HashSet::with_capacity(ids.len());
        let mut total_bytes = 0_u64;
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            if !seen.insert(id.as_str()) {
                return Err(RenderError::DuplicateAsset(id.clone()));
            }

            // Scoped to this tenant: an id belonging to someone else simply is not
            // found, which is the isolation working rather than a check.
            let entry = self.store.entry(tenant, Kind::Asset, id)?;
            total_bytes = total_bytes.saturating_add(entry.bytes);
            if total_bytes > self.config.max_bundle_bytes as u64 {
                return Err(BundleError::TooLarge {
                    actual: usize::try_from(total_bytes).unwrap_or(usize::MAX),
                    limit: self.config.max_bundle_bytes,
                }
                .into());
            }
            entries.push((id, entry));
        }

        entries
            .into_iter()
            .map(|(id, entry)| {
                let name = entry.filename.unwrap_or_else(|| id.clone());
                let bytes = self.store.get(tenant, Kind::Asset, id, &name)?;
                Ok(BundleFile {
                    path: name,
                    content: FileContent::Binary(bytes),
                })
            })
            .collect()
    }

    fn job_limits(&self, request: &RenderRequest) -> JobLimits {
        JobLimits {
            max_bundle_bytes: self.config.max_bundle_bytes,
            max_pages: self.config.max_pages,
            preview_pages: request.preview_pages.clone().unwrap_or_else(|| vec![1]),
            preview_scale_millis: 1000,
            preview_max_px: self.config.preview_max_px,
            memory_bytes: self.config.worker_memory_bytes,
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
