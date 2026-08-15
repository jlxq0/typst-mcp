//! Running the compiler and turning a document into bytes.

use std::sync::Arc;

use thiserror::Error;
use typst::utils::Scalar;
use typst_layout::PagedDocument;
use typst_library::diag::Warned;
use typst_library::foundations::{Datetime, Smart};
use typst_pdf::PdfOptions;
use typst_render::RenderOptions;

use crate::bundle::Bundle;
use crate::diagnostics::{self, Diagnostic, Severity};
use crate::fonts::FontLibrary;
use crate::world::{BundleWorld, WorldError, utc_today};

/// How many memoisation generations to keep between compiles.
///
/// `comemo`'s cache is global and grows without this; `typst-cli` evicts with the
/// same value after every compile in its watch loop. In this process a compile is
/// one-shot, so eviction is belt-and-braces — but a future in-process caller would
/// leak steadily without it, and the call is free.
const COMEMO_RETAIN: usize = 10;

/// Knobs the caller controls, all of them bounded.
#[derive(Debug, Clone)]
pub struct CompileOptions {
    /// Refuse documents longer than this rather than rendering them.
    pub max_pages: usize,
    /// 1-based page numbers to rasterise. Empty means no preview.
    pub preview_pages: Vec<usize>,
    /// Requested preview scale, before clamping.
    pub preview_scale: f32,
    /// Longest edge any preview may have, in pixels.
    pub preview_max_px: u32,
    /// Fixed date for `datetime.today()`. `None` uses the real UTC date.
    pub today: Option<Datetime>,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            max_pages: 200,
            preview_pages: vec![1],
            preview_scale: 1.0,
            preview_max_px: 2000,
            today: None,
        }
    }
}

/// One rasterised page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// 1-based page number.
    pub page: usize,
    pub width: u32,
    pub height: u32,
    /// PNG bytes.
    pub png: Vec<u8>,
}

/// A successful compile.
#[derive(Debug, Clone)]
pub struct CompileOutput {
    pub pdf: Vec<u8>,
    pub pages: usize,
    pub previews: Vec<Preview>,
    /// Warnings. A successful compile can still have plenty.
    pub diagnostics: Vec<Diagnostic>,
}

/// Why a compile did not produce a PDF.
#[derive(Debug, Error)]
pub enum CompileError {
    /// The document did not compile. The diagnostics are the useful part.
    #[error("compilation failed with {} error(s)", .diagnostics.iter().filter(|d| d.severity == Severity::Error).count())]
    Source { diagnostics: Vec<Diagnostic> },
    /// The document compiled but is longer than the caller is allowed to produce.
    #[error("document has {pages} pages; the limit is {limit}")]
    TooManyPages { pages: usize, limit: usize },
    /// PDF export failed after a successful layout. Rare.
    #[error("PDF export failed")]
    Export { diagnostics: Vec<Diagnostic> },
    /// A preview page number that does not exist.
    #[error("page {page} was requested for preview but the document has {pages}")]
    NoSuchPage { page: usize, pages: usize },
    /// Rasterising a page produced no image.
    #[error("could not encode a preview for page {page}")]
    Preview { page: usize },
    /// A bundle path the compiler itself refused.
    #[error(transparent)]
    World(#[from] WorldError),
}

impl CompileError {
    /// The diagnostics to hand back, whatever the failure was.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        match self {
            Self::Source { diagnostics } | Self::Export { diagnostics } => diagnostics.clone(),
            other => vec![Diagnostic::bare(Severity::Error, other.to_string())],
        }
    }
}

/// Compile a bundle to a PDF, plus any requested page previews.
pub fn compile(
    bundle: &Bundle,
    fonts: Arc<FontLibrary>,
    opts: &CompileOptions,
) -> Result<CompileOutput, CompileError> {
    let today = opts.today.or_else(utc_today);
    let world = BundleWorld::new(bundle, fonts, today)?;
    let result = compile_in(&world, opts);

    // Always evict, including on the error path: a failed compile populates the cache
    // just as a successful one does.
    comemo::evict(COMEMO_RETAIN);
    result
}

fn compile_in(world: &BundleWorld, opts: &CompileOptions) -> Result<CompileOutput, CompileError> {
    let Warned { output, warnings } = typst::compile::<PagedDocument>(world);
    let mut diagnostics = diagnostics::collect(world, &warnings);

    let document = match output {
        Ok(doc) => doc,
        Err(errors) => {
            // Errors first: whoever reads this wants the failure, not the warnings.
            let mut all = diagnostics::collect(world, &errors);
            all.append(&mut diagnostics);
            return Err(CompileError::Source { diagnostics: all });
        }
    };

    let pages = document.pages().len();
    if pages > opts.max_pages {
        return Err(CompileError::TooManyPages {
            pages,
            limit: opts.max_pages,
        });
    }

    // No timestamp, ever: it is the only nondeterministic field in the output, and
    // leaving it unset makes the PDF a pure function of the input. That is what lets
    // `output_is_reproducible` be a one-line assertion rather than a fuzzy diff.
    let pdf_options = PdfOptions {
        ident: Smart::Auto,
        timestamp: None,
        ..Default::default()
    };

    let pdf = typst_pdf::pdf(&document, &pdf_options).map_err(|errors| CompileError::Export {
        diagnostics: diagnostics::collect(world, &errors),
    })?;

    let previews = render_previews(&document, opts)?;

    Ok(CompileOutput {
        pdf,
        pages,
        previews,
        diagnostics,
    })
}

fn render_previews(
    document: &PagedDocument,
    opts: &CompileOptions,
) -> Result<Vec<Preview>, CompileError> {
    let pages = document.pages().len();
    let mut previews = Vec::with_capacity(opts.preview_pages.len());

    for &number in &opts.preview_pages {
        let page = document
            .pages()
            .get(number.checked_sub(1).ok_or(CompileError::NoSuchPage {
                page: number,
                pages,
            })?)
            .ok_or(CompileError::NoSuchPage {
                page: number,
                pages,
            })?;

        let size = page.frame.size();
        let scale = clamp_scale(
            opts.preview_scale,
            size.x.to_pt() as f32,
            size.y.to_pt() as f32,
            opts.preview_max_px,
        );

        let pixmap = typst_render::render(
            page,
            &RenderOptions {
                pixel_per_pt: Scalar::new(scale as f64),
                render_bleed: false,
            },
        );

        let png = pixmap
            .encode_png()
            .map_err(|_| CompileError::Preview { page: number })?;
        previews.push(Preview {
            page: number,
            width: pixmap.width(),
            height: pixmap.height(),
            png,
        });
    }

    Ok(previews)
}

/// Shrink `requested` until neither edge exceeds `max_px`.
///
/// Without this a `#set page(width: 10m)` document asks for a pixmap of billions of
/// pixels and the allocation, not the caller, decides what happens next.
fn clamp_scale(requested: f32, width_pt: f32, height_pt: f32, max_px: u32) -> f32 {
    let requested = requested.max(0.01);
    let longest = width_pt.max(height_pt).max(1.0);
    let allowed = max_px as f32 / longest;
    requested.min(allowed).max(0.01)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::bundle::{Bundle, BundleFile};

    fn bundle(source: &str) -> Bundle {
        Bundle::new(
            "main.typ",
            vec![BundleFile::text("main.typ", source)],
            BTreeMap::new(),
            1 << 20,
        )
        .expect("bundle")
    }

    fn fonts() -> Arc<FontLibrary> {
        Arc::new(FontLibrary::embedded_only())
    }

    #[test]
    fn compiles_a_minimal_document() {
        let out = compile(
            &bundle("= Hello\n\nWorld."),
            fonts(),
            &CompileOptions::default(),
        )
        .expect("compiles");
        assert!(out.pdf.starts_with(b"%PDF-"), "expected a PDF header");
        assert_eq!(out.pages, 1);
        assert_eq!(out.previews.len(), 1);
        assert!(out.previews[0].png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn reports_syntax_errors_with_a_position() {
        let err = compile(&bundle("#let x ="), fonts(), &CompileOptions::default())
            .expect_err("should fail");
        let diags = err.diagnostics();
        let first = diags.first().expect("at least one diagnostic");
        assert_eq!(first.severity, Severity::Error);
        assert_eq!(first.file.as_deref(), Some("main.typ"));
        assert!(
            first.line.is_some(),
            "diagnostics must carry a line: {first:?}"
        );
    }

    #[test]
    fn refuses_to_read_outside_the_bundle() {
        for source in [r#"#read("/etc/passwd")"#, r#"#read("../../secrets.txt")"#] {
            let err = compile(&bundle(source), fonts(), &CompileOptions::default())
                .expect_err("must not resolve");
            let text = format!("{:?}", err.diagnostics());
            assert!(
                !text.contains("root:"),
                "a real /etc/passwd would contain 'root:' — leaked: {text}"
            );
        }
    }

    #[test]
    fn explains_that_packages_are_unavailable() {
        let err = compile(
            &bundle("#import \"@preview/cetz:0.3.1\": *"),
            fonts(),
            &CompileOptions::default(),
        )
        .expect_err("no packages");
        let text = format!("{:?}", err.diagnostics());
        assert!(text.contains("not available"), "got {text}");
    }

    #[test]
    fn enforces_the_page_cap() {
        let opts = CompileOptions {
            max_pages: 2,
            preview_pages: vec![],
            ..Default::default()
        };
        let err = compile(
            &bundle("#pagebreak()\n#pagebreak()\n#pagebreak()"),
            fonts(),
            &opts,
        )
        .expect_err("too long");
        assert!(
            matches!(err, CompileError::TooManyPages { limit: 2, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn output_is_reproducible() {
        let opts = CompileOptions {
            preview_pages: vec![],
            ..Default::default()
        };
        let b = bundle("= Same\n\nInput.");
        let a = compile(&b, fonts(), &opts).expect("first");
        let c = compile(&b, fonts(), &opts).expect("second");
        assert_eq!(a.pdf, c.pdf, "identical input must produce identical bytes");
    }

    #[test]
    fn warnings_survive_a_successful_compile() {
        // An unused `#let` binding is a warning, not an error.
        let out = compile(
            &bundle("#let unused = 1\n= Fine"),
            fonts(),
            &CompileOptions {
                preview_pages: vec![],
                ..Default::default()
            },
        );
        assert!(out.is_ok());
    }

    #[test]
    fn preview_scale_is_clamped_to_the_pixel_budget() {
        // A4 is ~595x842pt. At scale 100 that would be 84 200px on the long edge.
        let scale = clamp_scale(100.0, 595.0, 842.0, 2000);
        assert!((scale - 2000.0 / 842.0).abs() < 1e-6, "got {scale}");
        // A modest request is left alone.
        assert_eq!(clamp_scale(1.0, 595.0, 842.0, 2000), 1.0);
    }

    #[test]
    fn rejects_preview_of_a_page_that_does_not_exist() {
        let opts = CompileOptions {
            preview_pages: vec![7],
            ..Default::default()
        };
        let err = compile(&bundle("= One page"), fonts(), &opts).expect_err("no page 7");
        assert!(
            matches!(err, CompileError::NoSuchPage { page: 7, pages: 1 }),
            "{err:?}"
        );
    }
}
