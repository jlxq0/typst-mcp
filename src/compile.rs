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
pub enum CompileErrorKind {
    /// The document did not compile. The diagnostics are the useful part.
    #[error("compilation failed")]
    Source,
    /// The document compiled but is longer than the caller is allowed to produce.
    #[error("document has {pages} pages; the limit is {limit}")]
    TooManyPages { pages: usize, limit: usize },
    /// PDF export failed after a successful layout. Rare.
    #[error("PDF export failed")]
    Export,
    /// A preview page number that does not exist.
    #[error("page {page} was requested for preview but the document has {pages}")]
    NoSuchPage { page: usize, pages: usize },
    /// The page is too large to rasterise within the pixel budget.
    #[error(
        "page {page} is {width_pt:.0}x{height_pt:.0}pt, which cannot be previewed within \
         {max_px}px per side"
    )]
    PageTooLarge {
        page: usize,
        width_pt: f32,
        height_pt: f32,
        max_px: u32,
    },
    /// Rasterising a page produced no image.
    #[error("could not encode a preview for page {page}")]
    Preview { page: usize },
    /// A bundle path the compiler itself refused.
    #[error(transparent)]
    World(#[from] WorldError),
}

/// A failed compile, with everything the compiler had to say about it.
///
/// One shape for every failure, carrying diagnostics uniformly. An earlier version
/// put diagnostics only on the variants that obviously needed them, and warnings
/// gathered before a late failure — a page-cap trip, an export error — were silently
/// dropped. Keeping them in one place makes that impossible rather than merely fixed.
#[derive(Debug, Error)]
#[error("{kind}")]
pub struct CompileError {
    pub kind: CompileErrorKind,
    /// Errors first, then warnings. For [`CompileErrorKind::Source`] this *is* the
    /// result: the caller reads it and fixes their document.
    diagnostics: Vec<Diagnostic>,
}

impl CompileError {
    fn new(kind: impl Into<CompileErrorKind>) -> Self {
        Self {
            kind: kind.into(),
            diagnostics: Vec::new(),
        }
    }

    fn with(kind: impl Into<CompileErrorKind>, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            kind: kind.into(),
            diagnostics,
        }
    }

    /// Append warnings gathered before the failure.
    fn extend(mut self, warnings: Vec<Diagnostic>) -> Self {
        self.diagnostics.extend(warnings);
        self
    }

    /// Everything to report: the compiler's own diagnostics, or a synthesised one
    /// describing the failure when the compiler had nothing to say.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        if self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
        {
            return self.diagnostics.clone();
        }
        let mut all = vec![Diagnostic::bare(Severity::Error, self.kind.to_string())];
        all.extend(self.diagnostics.iter().cloned());
        all
    }
}

impl From<WorldError> for CompileError {
    fn from(err: WorldError) -> Self {
        Self::new(CompileErrorKind::World(err))
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
    let warnings = diagnostics::collect(world, &warnings);

    let document = match output {
        Ok(doc) => doc,
        Err(errors) => {
            // Errors first: whoever reads this wants the failure, not the warnings.
            let errors = diagnostics::collect(world, &errors);
            return Err(CompileError::with(CompileErrorKind::Source, errors).extend(warnings));
        }
    };

    // Everything past here can still fail, and the warnings gathered above have to
    // survive that: `.extend(warnings)` on the single error path is what guarantees it.
    export(&document, world, opts)
        .map_err(|err| err.extend(warnings.clone()))
        .map(|(pdf, pages, previews)| CompileOutput {
            pdf,
            pages,
            previews,
            diagnostics: warnings,
        })
}

/// Everything after a successful layout: cap, PDF, previews.
fn export(
    document: &PagedDocument,
    world: &BundleWorld,
    opts: &CompileOptions,
) -> Result<(Vec<u8>, usize, Vec<Preview>), CompileError> {
    let pages = document.pages().len();
    if pages > opts.max_pages {
        return Err(CompileError::new(CompileErrorKind::TooManyPages {
            pages,
            limit: opts.max_pages,
        }));
    }

    // No timestamp, ever: it is the only nondeterministic field in the output, and
    // leaving it unset makes the PDF a pure function of the input. That is what lets
    // `output_is_reproducible` be a one-line assertion rather than a fuzzy diff.
    let pdf_options = PdfOptions {
        ident: Smart::Auto,
        timestamp: None,
        ..Default::default()
    };

    let pdf = typst_pdf::pdf(document, &pdf_options).map_err(|errors| {
        CompileError::with(
            CompileErrorKind::Export,
            diagnostics::collect(world, &errors),
        )
    })?;

    let previews = render_previews(document, opts)?;
    Ok((pdf, pages, previews))
}

fn render_previews(
    document: &PagedDocument,
    opts: &CompileOptions,
) -> Result<Vec<Preview>, CompileError> {
    let pages = document.pages().len();
    let mut previews = Vec::with_capacity(opts.preview_pages.len());

    for &number in &opts.preview_pages {
        let no_such_page = || {
            CompileError::new(CompileErrorKind::NoSuchPage {
                page: number,
                pages,
            })
        };
        let page = document
            .pages()
            .get(number.checked_sub(1).ok_or_else(no_such_page)?)
            .ok_or_else(no_such_page)?;

        let size = page.frame.size();
        let (width_pt, height_pt) = (size.x.to_pt() as f32, size.y.to_pt() as f32);
        let scale = clamp_scale(opts.preview_scale, width_pt, height_pt, opts.preview_max_px);

        // Predict what typst-render will allocate and refuse it here if it is out of
        // budget. `typst_render::render` ends in `Pixmap::new(w, h).unwrap()`, so a
        // page big enough to saturate the `as u32` cast is a panic — and with
        // `panic = "abort"` in release that is the whole worker. Clamping the scale
        // should already prevent it; this is the check that makes it true rather than
        // likely.
        let (px_w, px_h) = predicted_pixels(width_pt, height_pt, scale).ok_or_else(|| {
            CompileError::new(CompileErrorKind::PageTooLarge {
                page: number,
                width_pt,
                height_pt,
                max_px: opts.preview_max_px,
            })
        })?;
        if px_w > opts.preview_max_px || px_h > opts.preview_max_px {
            return Err(CompileError::new(CompileErrorKind::PageTooLarge {
                page: number,
                width_pt,
                height_pt,
                max_px: opts.preview_max_px,
            }));
        }

        let pixmap = typst_render::render(
            page,
            &RenderOptions {
                pixel_per_pt: Scalar::new(scale as f64),
                render_bleed: false,
            },
        );

        let png = pixmap
            .encode_png()
            .map_err(|_| CompileError::new(CompileErrorKind::Preview { page: number }))?;
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
/// Without this a document that sets an enormous page asks for a pixmap of billions
/// of pixels, and the allocator — not the caller — decides what happens next.
///
/// The result is never raised back above the computed ceiling. An earlier version
/// ended in `.max(0.01)`, which silently discarded the cap for any page longer than
/// `max_px / 0.01` points: at the 2000px default, a 400000pt page still rendered at
/// 4000x4000. Sanitising the *input* is a separate step from bounding the *output*,
/// and doing both with one `max` conflated them.
fn clamp_scale(requested: f32, width_pt: f32, height_pt: f32, max_px: u32) -> f32 {
    // Sanitise first: a non-finite or non-positive request means "use the default".
    let requested = if requested.is_finite() && requested > 0.0 {
        requested
    } else {
        1.0
    };

    let longest = width_pt.max(height_pt);
    if !longest.is_finite() || longest <= 0.0 {
        // A degenerate page cannot overflow anything; typst-render floors at 1px.
        return requested;
    }

    // Then bound, and never undo it.
    (max_px as f32 / longest).min(requested)
}

/// The pixmap dimensions `typst_render::render` will allocate, or `None` if they are
/// not representable.
///
/// Mirrors typst-render 0.15.1 exactly — `(pixel_per_pt * size).round().max(1.0) as u32`
/// — because the point is to predict its behaviour, not to approximate it. The `as u32`
/// cast there saturates, so a value beyond `u32::MAX` reaches `Pixmap::new` and panics;
/// checking finiteness and range here is what turns that into a diagnostic.
fn predicted_pixels(width_pt: f32, height_pt: f32, scale: f32) -> Option<(u32, u32)> {
    let axis = |pt: f32| -> Option<u32> {
        let px = (scale * pt).round().max(1.0);
        // `as u32` saturates rather than wrapping, so an out-of-range float would
        // silently become u32::MAX. Reject it instead.
        (px.is_finite() && px <= u32::MAX as f32).then_some(px as u32)
    };
    Some((axis(width_pt)?, axis(height_pt)?))
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
            matches!(err.kind, CompileErrorKind::TooManyPages { limit: 2, .. }),
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
    fn a_missing_font_is_reported_as_a_warning() {
        // The most valuable warning this service can surface: Typst substitutes a
        // missing family silently, so without this the document simply comes out
        // looking wrong with no indication why.
        let out = compile(
            &bundle("#set text(font: \"NoSuchFontFamily\")\n= Fine"),
            fonts(),
            &CompileOptions {
                preview_pages: vec![],
                ..Default::default()
            },
        )
        .expect("a missing font is not fatal");

        assert!(
            out.diagnostics
                .iter()
                .any(|d| d.severity == Severity::Warning && d.message.contains("font")),
            "expected a font warning, got {:?}",
            out.diagnostics
        );
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
    fn the_pixel_budget_holds_for_absurdly_large_pages() {
        // Regression: a trailing `.max(0.01)` used to undo the clamp for any page
        // longer than max_px/0.01 pt, so a 400000pt page rendered at 4000x4000 against
        // a 2000px cap. The A4 case above passes either way, which is why the bug
        // survived — these are the sizes that actually exercise the ceiling.
        for longest in [200_000.0, 400_000.0, 2_000_000.0, 1e12, f32::MAX] {
            let scale = clamp_scale(1.0, longest, longest, 2000);
            let (w, h) = predicted_pixels(longest, longest, scale)
                .unwrap_or_else(|| panic!("{longest}pt produced unrepresentable pixels"));
            assert!(
                w <= 2000 && h <= 2000,
                "{longest}pt rendered {w}x{h}px, over the 2000px cap (scale {scale})"
            );
        }
    }

    #[test]
    fn predicted_pixels_matches_typst_render_and_rejects_the_unrepresentable() {
        // Mirrors typst-render's own arithmetic; if that ever changes, this is the test
        // that should fail rather than a panic in production.
        assert_eq!(predicted_pixels(595.0, 842.0, 1.0), Some((595, 842)));
        assert_eq!(predicted_pixels(100.0, 100.0, 2.0), Some((200, 200)));
        // typst-render floors at one pixel, so a vanishing scale is not an error.
        assert_eq!(predicted_pixels(100.0, 100.0, 1e-9), Some((1, 1)));
        // Beyond u32 the `as` cast would saturate and Pixmap::new would panic.
        assert_eq!(predicted_pixels(f32::MAX, f32::MAX, 1.0), None);
        assert_eq!(predicted_pixels(1e30, 1e30, 1.0), None);
    }

    #[test]
    fn absurd_page_sizes_preview_within_budget_instead_of_aborting() {
        // Regression: these used to reach `Pixmap::new(..).unwrap()` inside typst-render
        // and abort the process — which `panic = "abort"` makes fatal to the worker.
        // With the clamp fixed they render correctly, just very small.
        let opts = CompileOptions {
            preview_pages: vec![1],
            preview_max_px: 2000,
            ..Default::default()
        };
        for size in ["1000000pt", "1000000000000pt"] {
            let source = format!("#set page(width: {size}, height: {size})\n= Big");
            let out = compile(&bundle(&source), fonts(), &opts)
                .unwrap_or_else(|e| panic!("{size} failed: {e}"));
            let preview = out.previews.first().expect("a preview");
            assert!(
                preview.width <= 2000 && preview.height <= 2000,
                "{size} rendered {}x{}px, over the cap",
                preview.width,
                preview.height
            );
        }
    }

    #[test]
    fn a_degenerate_scale_request_falls_back_rather_than_exploding() {
        for requested in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let scale = clamp_scale(requested, 595.0, 842.0, 2000);
            assert!(scale.is_finite() && scale > 0.0, "{requested} gave {scale}");
            let (w, h) = predicted_pixels(595.0, 842.0, scale).expect("representable");
            assert!(w <= 2000 && h <= 2000, "{requested} gave {w}x{h}");
        }
    }

    #[test]
    fn warnings_survive_a_late_failure() {
        // Regression: warnings gathered during a successful layout were dropped when a
        // later stage failed, so a page-cap trip reported the cap and silently lost
        // everything the compiler had said.
        let opts = CompileOptions {
            max_pages: 1,
            preview_pages: vec![],
            ..Default::default()
        };
        // An unknown font family warns; the pagebreaks then trip the page cap.
        let source = "#set text(font: \"NoSuchFontFamily\")\n#pagebreak()\n#pagebreak()";
        let err = compile(&bundle(source), fonts(), &opts).expect_err("over the cap");
        assert!(
            matches!(err.kind, CompileErrorKind::TooManyPages { .. }),
            "{err:?}"
        );

        let diagnostics = err.diagnostics();
        assert!(
            diagnostics.iter().any(|d| d.severity == Severity::Error),
            "the failure itself must be reported: {diagnostics:?}"
        );
        assert!(
            diagnostics.iter().any(|d| d.severity == Severity::Warning),
            "warnings from before the failure must survive: {diagnostics:?}"
        );
    }

    #[test]
    fn rejects_preview_of_a_page_that_does_not_exist() {
        let opts = CompileOptions {
            preview_pages: vec![7],
            ..Default::default()
        };
        let err = compile(&bundle("= One page"), fonts(), &opts).expect_err("no page 7");
        assert!(
            matches!(err.kind, CompileErrorKind::NoSuchPage { page: 7, pages: 1 }),
            "{err:?}"
        );
    }
}
