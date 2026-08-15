//! Phase 0/1 harness: compile a `.typ` file or a directory and report what came out.
//!
//! Replaced by the real `serve` / `--compile-worker` dispatch in Phase 2.
//!
//! ```text
//! typst-mcp <file-or-dir> [entrypoint] [--fonts <dir>]...
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use typst_mcp::bundle::{Bundle, BundleFile, FileContent};
use typst_mcp::compile::{CompileOptions, compile};
use typst_mcp::fonts::FontLibrary;

/// Extensions loaded as text; everything else becomes opaque bytes.
const TEXT_EXTENSIONS: &[&str] = &["typ", "json", "csv", "yaml", "yml", "toml", "svg", "txt"];

fn main() -> anyhow::Result<()> {
    let mut positional = Vec::new();
    let mut font_dirs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fonts" => font_dirs.push(args.next().unwrap_or_default()),
            _ => positional.push(arg),
        }
    }

    let target = PathBuf::from(positional.first().map_or("main.typ", String::as_str));
    let (files, default_main) = if target.is_dir() {
        collect_dir(&target)?
    } else {
        let name = file_name(&target);
        (vec![BundleFile::text(&name, std::fs::read_to_string(&target)?)], name)
    };

    let main = positional.get(1).cloned().unwrap_or(default_main);
    let bundle = Bundle::new(&main, files, BTreeMap::new(), 8 * 1024 * 1024)?;
    println!("bundle: {} file(s), entrypoint {main}", bundle.len());

    let fonts = Arc::new(FontLibrary::new(&font_dirs));
    let started = std::time::Instant::now();

    match compile(&bundle, fonts, &CompileOptions::default()) {
        Ok(out) => {
            let elapsed = started.elapsed();
            std::fs::write("out.pdf", &out.pdf)?;
            for preview in &out.previews {
                std::fs::write(format!("out-page-{}.png", preview.page), &preview.png)?;
                println!("preview page {}: {}x{}px", preview.page, preview.width, preview.height);
            }
            println!(
                "ok: {} pages, {} bytes, {} warning(s), {:.0}ms",
                out.pages,
                out.pdf.len(),
                out.diagnostics.len(),
                elapsed.as_secs_f64() * 1000.0
            );
            for diag in &out.diagnostics {
                println!("  {diag:?}");
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("failed: {err}");
            for diag in err.diagnostics() {
                eprintln!("  {diag:?}");
            }
            std::process::exit(1);
        }
    }
}

/// Load a directory as a bundle, one level of subdirectories deep.
fn collect_dir(root: &Path) -> anyhow::Result<(Vec<BundleFile>, String)> {
    let mut files = Vec::new();
    let mut walk = vec![(root.to_path_buf(), String::new())];

    while let Some((dir, prefix)) = walk.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if entry.file_type()?.is_dir() {
                walk.push((entry.path(), rel));
            } else {
                let bytes = std::fs::read(entry.path())?;
                let is_text = entry
                    .path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| TEXT_EXTENSIONS.contains(&e));
                let content = match String::from_utf8(bytes) {
                    Ok(text) if is_text => FileContent::Text(text),
                    Ok(text) => FileContent::Binary(text.into_bytes()),
                    Err(err) => FileContent::Binary(err.into_bytes()),
                };
                files.push(BundleFile { path: rel, content });
            }
        }
    }

    let main = files
        .iter()
        .map(|f| f.path.clone())
        .find(|p| p.ends_with(".typ"))
        .unwrap_or_else(|| "main.typ".into());
    Ok((files, main))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main.typ".into())
}
