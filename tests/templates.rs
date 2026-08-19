//! The shipped templates, rendered through the real pipeline.
//!
//! These are the tests that decide whether the product works: a caller names a
//! template, supplies metadata and body markup, and gets a branded PDF. Everything
//! else in the codebase exists to make this safe.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use typst_mcp::compile::{CompileOptions, compile};
use typst_mcp::fonts::FontLibrary;
use typst_mcp::templates::{TemplateKind, TemplateSet};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn templates() -> TemplateSet {
    TemplateSet::load(&repo("templates")).expect("the shipped templates must load")
}

/// The fonts the container will have: Typst's embedded set plus the baked brand family.
fn fonts() -> Arc<FontLibrary> {
    Arc::new(FontLibrary::new(&[repo("fonts")]))
}

#[test]
fn every_shipped_template_loads() {
    // A broken manifest must fail here, in CI, rather than on a caller's first request.
    let set = templates();
    assert!(!set.is_empty(), "no templates found under templates/");
    for template in set.iter() {
        assert!(!template.name().is_empty());
        assert!(
            template.schema().is_some(),
            "{} has no schema.json; callers would have nothing to validate against",
            template.name()
        );
        assert!(
            template.example().is_some(),
            "{} has no fixture.json; the tool's `example` would be empty",
            template.name()
        );
    }
}

#[test]
fn the_brand_font_is_available() {
    // A missing family does not fail a compile — Typst silently substitutes, and the
    // document comes out subtly wrong. So it has to be asserted directly.
    let fonts = fonts();
    assert!(
        fonts.has_family("Figtree"),
        "Figtree is not in the baked font set; documents would fall back. Have: {:?}",
        fonts
            .families()
            .iter()
            .map(|f| &f.name)
            .take(20)
            .collect::<Vec<_>>()
    );
    for family in [
        "Figtree",
        "Inter",
        "Passion One",
        "Space Grotesk",
        "Roboto",
        "Source Sans 3",
        "Roboto Slab",
        "IBM Plex Sans",
        "IBM Plex Serif",
        "IBM Plex Mono",
        "JetBrains Mono",
        "Source Serif 4",
        "EB Garamond",
        "Noto Sans",
        "Noto Serif",
    ] {
        assert!(
            fonts.has_family(family),
            "{family} is not in the baked font set; a branded document would fall back"
        );
    }
}

#[test]
fn every_template_renders_from_its_own_fixture() {
    // The fixture doubles as the tool's `example`, so this also proves the example we
    // hand a model is one that actually works.
    let set = templates();
    let fonts = fonts();

    for template in set.iter() {
        let data = template.example().expect("fixture.json").clone();
        let body = match template.manifest.kind {
            TemplateKind::Wrapper => Some(template.example_body().unwrap_or_else(|| {
                panic!(
                    "{} is a wrapper but has no fixture.body.typ",
                    template.name()
                )
            })),
            TemplateKind::Data => None,
        };

        let assembled = template
            .assemble(&data, body, vec![], 8 * 1024 * 1024)
            .unwrap_or_else(|e| panic!("{} failed to assemble: {e}", template.name()));

        let out = compile(
            &assembled.bundle,
            Arc::clone(&fonts),
            &CompileOptions::default(),
        )
        .unwrap_or_else(|e| {
            panic!(
                "{} failed to compile: {e}\n{:#?}",
                template.name(),
                e.diagnostics()
            )
        });

        assert!(
            out.pdf.starts_with(b"%PDF-"),
            "{} produced no PDF",
            template.name()
        );
        assert!(
            out.pages >= 1,
            "{} produced an empty document",
            template.name()
        );
        assert_eq!(
            out.previews.len(),
            1,
            "{} produced no preview",
            template.name()
        );

        // Warnings are allowed, but errors would have failed the compile above; this
        // catches a template that renders while complaining.
        for diagnostic in &out.diagnostics {
            assert_ne!(
                diagnostic.severity,
                typst_mcp::diagnostics::Severity::Error,
                "{} emitted an error diagnostic: {diagnostic:?}",
                template.name()
            );
            assert!(
                !diagnostic.message.contains("unknown font family"),
                "{} fell back to a substitute font: {diagnostic:?}",
                template.name()
            );
        }
    }
}

#[test]
fn the_ksc_template_embeds_both_brand_families() {
    let set = templates();
    let template = set.get("ksc").expect("the KSC template must be present");
    let assembled = template
        .assemble(
            template.example().expect("KSC fixture"),
            template.example_body(),
            vec![],
            8 * 1024 * 1024,
        )
        .expect("KSC assembles");
    let out = compile(&assembled.bundle, fonts(), &CompileOptions::default())
        .unwrap_or_else(|e| panic!("KSC compile failed: {e}\n{:#?}", e.diagnostics()));
    let pdf = String::from_utf8_lossy(&out.pdf);
    for family in ["Inter", "PassionOne"] {
        assert!(
            pdf.contains(family),
            "KSC PDF has no embedded {family} subset"
        );
    }
    assert!(out.pages >= 3, "expected KSC cover, contents and body");
}

#[test]
fn the_lenno_template_embeds_both_brand_families() {
    let set = templates();
    let template = set
        .get("lenno")
        .expect("the Lenno template must be present");
    let assembled = template
        .assemble(
            template.example().expect("Lenno fixture"),
            template.example_body(),
            vec![],
            8 * 1024 * 1024,
        )
        .expect("Lenno assembles");
    let out = compile(&assembled.bundle, fonts(), &CompileOptions::default())
        .unwrap_or_else(|e| panic!("Lenno compile failed: {e}\n{:#?}", e.diagnostics()));
    let pdf = String::from_utf8_lossy(&out.pdf);
    for family in ["Roboto", "SpaceGrotesk"] {
        assert!(
            pdf.contains(family),
            "Lenno PDF has no embedded {family} subset"
        );
    }
    assert!(out.pages >= 3, "expected Lenno cover, contents and body");
}

#[test]
fn the_hanso_template_produces_a_real_document() {
    let set = templates();
    let template = set
        .get("hanso")
        .expect("the hanso template must be present");

    let data = serde_json::json!({
        "title": "Integration Test Report",
        "author": "typst-mcp",
        "date": "2026-08-15",
        "footer_style": "simple",
    });
    let body = "= First Chapter\n\n\
                #quote(block: true)[A standfirst under the chapter title.]\n\n\
                == A section\n\nBody text with a #footnote[footnote] and a list:\n\n\
                - one\n- two\n";

    let assembled = template
        .assemble(&data, Some(body), vec![], 8 * 1024 * 1024)
        .expect("assembles");
    let out = compile(&assembled.bundle, fonts(), &CompileOptions::default())
        .unwrap_or_else(|e| panic!("compile failed: {e}\n{:#?}", e.diagnostics()));

    // A cover, a table of contents and the body: the branded template is doing its
    // job rather than just rendering the markup it was handed.
    assert!(
        out.pages >= 3,
        "expected cover + contents + body, got {} page(s)",
        out.pages
    );

    // Figtree subsets must be embedded, or the PDF renders with substituted fonts on
    // any machine that lacks the family.
    let pdf = String::from_utf8_lossy(&out.pdf);
    assert!(
        pdf.contains("Figtree"),
        "no Figtree subset embedded; the document would not be on-brand"
    );
}

#[test]
fn the_theme_argument_changes_the_document() {
    // Proves the ident mapping reaches the template as a binding rather than a string:
    // if it did not, both renders would be byte-identical.
    let set = templates();
    let template = set.get("hanso").expect("hanso");
    let fonts = fonts();
    let body = "= Chapter\n\nText.";

    let mut rendered = Vec::new();
    for theme in ["light", "dark"] {
        let data = serde_json::json!({ "title": "Theme", "date": "2026-08-15", "theme": theme });
        let assembled = template
            .assemble(&data, Some(body), vec![], 1 << 20)
            .expect("assembles");
        let out = compile(
            &assembled.bundle,
            Arc::clone(&fonts),
            &CompileOptions {
                preview_pages: vec![],
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{theme} failed: {e}\n{:#?}", e.diagnostics()));
        rendered.push(out.pdf);
    }

    assert_ne!(
        rendered[0], rendered[1],
        "light and dark rendered identically"
    );
}

#[test]
fn caller_data_cannot_break_the_branding() {
    // The threat this template layer is built around: an invoice renders a customer
    // name from an untrusted source. That name must not be able to redefine anything.
    let set = templates();
    let template = set.get("hanso").expect("hanso");

    let data = serde_json::json!({
        "title": r#"ACME") ; #let hanso-doc = (..a) => [owned] ; #panic("pwned") ; #("#,
        "author": "\u{2028}#panic(\"line separator\")",
    });

    let assembled = template
        .assemble(&data, Some("Body."), vec![], 1 << 20)
        .expect("assembles");
    let out = compile(
        &assembled.bundle,
        fonts(),
        &CompileOptions {
            preview_pages: vec![],
            ..Default::default()
        },
    )
    .expect("the injected code must render as text, not run");

    assert!(out.pdf.starts_with(b"%PDF-"));
}

#[test]
fn unknown_data_fields_are_rejected() {
    // `additionalProperties: false` in the schema. A typo should be a clear error
    // rather than a field that silently does nothing.
    let set = templates();
    let template = set.get("hanso").expect("hanso");
    let err = template
        .assemble(
            &serde_json::json!({ "title": "x", "titel": "typo" }),
            Some("Body."),
            vec![],
            1 << 20,
        )
        .expect_err("unknown field must be refused");
    assert!(
        err.to_string().contains("titel") || err.to_string().contains("additional"),
        "{err}"
    );
}

#[test]
fn a_missing_title_names_the_field() {
    let set = templates();
    let template = set.get("hanso").expect("hanso");
    let err = template
        .assemble(
            &serde_json::json!({ "author": "x" }),
            Some("Body."),
            vec![],
            1 << 20,
        )
        .expect_err("title is required");
    assert!(
        err.to_string().contains("title"),
        "the error must name the field: {err}"
    );
}

#[test]
fn body_diagnostics_point_at_the_callers_own_lines() {
    // The body is mounted as its own file so a model reading the error sees its own
    // line numbers, not an offset into a generated entrypoint it never wrote.
    let set = templates();
    let template = set.get("hanso").expect("hanso");

    let body = "= Fine\n\nAlso fine.\n\n#let broken =\n";
    let assembled = template
        .assemble(
            &serde_json::json!({ "title": "x", "date": "2026-08-15" }),
            Some(body),
            vec![],
            1 << 20,
        )
        .expect("assembles");

    let err = compile(&assembled.bundle, fonts(), &CompileOptions::default())
        .expect_err("body is broken");
    let mut diagnostics = err.diagnostics();
    assembled.source_map.apply(&mut diagnostics);
    let first = diagnostics.first().expect("a diagnostic");

    assert_eq!(
        first.file.as_deref(),
        Some("body.typ"),
        "diagnostics must point at the body under a caller-facing name: {first:?}"
    );
    assert_eq!(
        first.line,
        Some(5),
        "the import prelude must be subtracted so the line is the caller's own: {first:?}"
    );
}
