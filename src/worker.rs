//! The child side: one process, one document, then exit.
//!
//! Exiting after every job is the point. It resets two pieces of global state that
//! nothing else can reclaim:
//!
//! * Typst's `FileId` interner `Box::leak`s every distinct path and indexes it with a
//!   `NonZeroU16`, ending in `.expect("out of file ids")`. Never freed, capped at
//!   65 535, and with `panic = "abort"` that would take the whole server down.
//! * `comemo`'s memo cache is global and grows.
//!
//! It also makes the deadline real, because a process can be killed and a thread
//! cannot, and it contains a panic to one document.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::bundle::{Bundle, BundleFile};
use crate::compile::{CompileOptions, compile};
use crate::fonts::FontLibrary;
use crate::protocol::{Job, JobPreview, JobResult, read_frame, write_frame};

/// Read one job, compile it, write one result. Returns the process exit code.
///
/// A failed *document* is a successful run: the result frame carries the diagnostics
/// and the exit code stays 0, so the parent can tell a broken document apart from a
/// broken worker.
pub fn run() -> i32 {
    let job: Job = match read_frame(io::stdin().lock()) {
        Ok(job) => job,
        Err(err) => {
            eprintln!("worker: could not read job: {err}");
            return 2;
        }
    };

    // Self-imposed before any real allocation, so a runaway document hits this rather
    // than the container's memory limit — which would take the server with it.
    apply_memory_limit(job.limits.memory_bytes);

    let result = execute(&job);
    if let Err(err) = write_frame(io::stdout().lock(), &result) {
        eprintln!("worker: could not write result: {err}");
        return 3;
    }
    0
}

fn execute(job: &Job) -> JobResult {
    let mut files = Vec::with_capacity(job.files.len());
    for file in &job.files {
        match file.content.resolve() {
            Ok(content) => files.push(BundleFile {
                path: file.path.clone(),
                content,
            }),
            Err(err) => {
                return JobResult::Failed {
                    message: format!("could not read {}: {err}", file.path),
                    diagnostics: vec![],
                };
            }
        }
    }

    let inputs: BTreeMap<String, String> = job.inputs.clone();
    let bundle = match Bundle::new(&job.main, files, inputs, job.limits.max_bundle_bytes) {
        Ok(bundle) => bundle,
        Err(err) => {
            return JobResult::Failed {
                message: err.to_string(),
                diagnostics: vec![],
            };
        }
    };

    let fonts = Arc::new(FontLibrary::new(&job.font_dirs));
    let options = CompileOptions {
        max_pages: job.limits.max_pages,
        preview_pages: job.limits.preview_pages.clone(),
        preview_scale: job.limits.preview_scale_millis as f32 / 1000.0,
        preview_max_px: job.limits.preview_max_px,
        today: None,
    };

    match compile(&bundle, fonts, &options) {
        Ok(out) => JobResult::Ok {
            pdf_base64: BASE64.encode(&out.pdf),
            pages: out.pages,
            previews: out
                .previews
                .into_iter()
                .map(|p| JobPreview {
                    page: p.page,
                    width: p.width,
                    height: p.height,
                    png_base64: BASE64.encode(&p.png),
                })
                .collect(),
            diagnostics: out.diagnostics,
        },
        Err(err) => JobResult::Failed {
            message: err.to_string(),
            diagnostics: err.diagnostics(),
        },
    }
}

/// Cap the worker's address space.
///
/// Best-effort: a platform that refuses the call still gets the deadline and the
/// container limit, so a failure here is worth a line on stderr and nothing more.
fn apply_memory_limit(bytes: u64) {
    if bytes == 0 {
        return;
    }
    if let Err(err) = rlimit::setrlimit(rlimit::Resource::AS, bytes, bytes) {
        eprintln!("worker: could not set RLIMIT_AS to {bytes}: {err}");
    }
}

/// Write a job frame to a stream, for callers driving a worker by hand.
pub fn send_job<W: Write>(out: W, job: &Job) -> io::Result<()> {
    write_frame(out, job)
}

/// Read a result frame from a stream.
pub fn receive_result<R: Read>(input: R) -> io::Result<JobResult> {
    read_frame(input)
}
