//! Render a shipped template from its fixture, for eyeballing the result.
//!
//! `cargo run --example render_template -- <name> [out-dir]`

use std::path::{Path, PathBuf};
use std::sync::Arc;

use typst_mcp::compile::{CompileOptions, compile};
use typst_mcp::fonts::FontLibrary;
use typst_mcp::templates::{TemplateKind, TemplateSet};

fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| "hanso".into());
    let out_dir = PathBuf::from(std::env::args().nth(2).unwrap_or_else(|| ".".into()));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let set = TemplateSet::load(&root.join("templates"))?;
    let template = set
        .get(&name)
        .unwrap_or_else(|| panic!("no template {name:?}; have {:?}", set.names()));

    let data = template.example().cloned().unwrap_or(serde_json::json!({}));
    let body = match template.manifest.kind {
        TemplateKind::Wrapper => template.example_body(),
        TemplateKind::Data => None,
    };

    let assembled = template.assemble(&data, body, vec![], 8 * 1024 * 1024)?;
    let fonts = Arc::new(FontLibrary::new(&[root.join("fonts")]));
    let opts = CompileOptions {
        preview_pages: vec![1, 2, 3],
        ..Default::default()
    };

    match compile(&assembled.bundle, fonts, &opts) {
        Ok(out) => {
            std::fs::write(out_dir.join(format!("{name}.pdf")), &out.pdf)?;
            for preview in &out.previews {
                std::fs::write(
                    out_dir.join(format!("{name}-page-{}.png", preview.page)),
                    &preview.png,
                )?;
            }
            println!("{name}: {} pages, {} bytes", out.pages, out.pdf.len());
            Ok(())
        }
        Err(err) => {
            let mut diagnostics = err.diagnostics();
            assembled.source_map.apply(&mut diagnostics);
            eprintln!("{name} failed: {err}");
            for diagnostic in &diagnostics {
                eprintln!("  {diagnostic:?}");
            }
            std::process::exit(1);
        }
    }
}
