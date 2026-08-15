//! Entry point. Two modes:
//!
//! * `--compile-worker` — read one job from stdin, write one result to stdout, exit.
//!   Spawned by the server; not meant to be run by hand.
//! * anything else — the Phase 0/1 harness: compile a file or directory and report.
//!   Replaced by `serve` once the HTTP surface lands.
//!
//! ```text
//! typst-mcp <file-or-dir> [entrypoint] [--fonts <dir>]...
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use typst_mcp::bundle::{Bundle, BundleFile, FileContent};
use typst_mcp::protocol::{Job, JobContent, JobFile, JobLimits, JobResult};
use typst_mcp::spawn::{CompileService, SpawnConfig, WORKER_FLAG};
use typst_mcp::worker;

/// Extensions loaded as text; everything else becomes opaque bytes.
const TEXT_EXTENSIONS: &[&str] = &["typ", "json", "csv", "yaml", "yml", "toml", "svg", "txt"];

fn main() -> anyhow::Result<()> {
    // Checked before anything else and before the async runtime starts: a worker is a
    // plain synchronous process that does one thing.
    if std::env::args().any(|a| a == WORKER_FLAG) {
        std::process::exit(worker::run());
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(harness())
}

async fn harness() -> anyhow::Result<()> {
    let mut positional = Vec::new();
    let mut font_dirs: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fonts" => font_dirs.push(args.next().unwrap_or_default().into()),
            _ => positional.push(arg),
        }
    }

    let target = PathBuf::from(positional.first().map_or("main.typ", String::as_str));
    let (files, default_main) = if target.is_dir() {
        collect_dir(&target)?
    } else {
        let name = file_name(&target);
        (
            vec![BundleFile::text(&name, std::fs::read_to_string(&target)?)],
            name,
        )
    };

    let main = positional.get(1).cloned().unwrap_or(default_main);

    // Validate here as well as in the worker: a bad path should be rejected before a
    // process is spawned for it.
    let bundle = Bundle::new(&main, files, BTreeMap::new(), 8 * 1024 * 1024)?;
    println!("bundle: {} file(s), entrypoint {main}", bundle.len());

    let job = Job {
        main: bundle.main().to_owned(),
        files: bundle
            .files()
            .map(|(path, content)| JobFile {
                path: path.to_owned(),
                content: JobContent::from_content(content),
            })
            .collect(),
        inputs: bundle.inputs().clone(),
        font_dirs,
        limits: JobLimits::default(),
    };

    let service = CompileService::new(SpawnConfig::new()?);
    let started = std::time::Instant::now();
    let result = service.compile(&job).await?;
    let elapsed = started.elapsed();

    match result {
        JobResult::Ok {
            pdf_base64,
            pages,
            previews,
            diagnostics,
        } => {
            use base64::Engine as _;
            let engine = base64::engine::general_purpose::STANDARD;
            let pdf = engine.decode(pdf_base64)?;
            std::fs::write("out.pdf", &pdf)?;
            for preview in &previews {
                let png = engine.decode(&preview.png_base64)?;
                std::fs::write(format!("out-page-{}.png", preview.page), &png)?;
                println!(
                    "preview page {}: {}x{}px",
                    preview.page, preview.width, preview.height
                );
            }
            println!(
                "ok: {pages} pages, {} bytes, {} warning(s), {:.0}ms",
                pdf.len(),
                diagnostics.len(),
                elapsed.as_secs_f64() * 1000.0
            );
            for diag in &diagnostics {
                println!("  {diag:?}");
            }
            Ok(())
        }
        JobResult::Failed {
            message,
            diagnostics,
        } => {
            eprintln!("failed: {message}");
            for diag in &diagnostics {
                eprintln!("  {diag:?}");
            }
            std::process::exit(1);
        }
    }
}

/// Load a directory as a bundle.
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
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
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
