//! Parent-owned per-compile workspaces.
//!
//! Bulk source, asset, PDF, and preview bytes never cross the worker's JSON pipes.
//! The parent stages an already validated [`Bundle`] under opaque numeric filenames,
//! sends only paths and metadata, and reads only fixed output names it derives itself.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use tempfile::{Builder, TempDir};

use crate::bundle::{Bundle, FileContent};
use crate::protocol::{Job, JobFile, JobFileKind, JobLimits, JobPreview};

/// Fixed PDF filename inside a private job output directory.
pub const PDF_NAME: &str = "doc.pdf";

/// One preview read back by the parent.
#[derive(Debug, Clone)]
pub struct JobOutputPreview {
    pub page: usize,
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// Bulk compile output after the parent has read its own fixed paths.
#[derive(Debug, Clone)]
pub struct JobOutputs {
    pub pdf: Vec<u8>,
    pub previews: Vec<JobOutputPreview>,
}

/// A staged job whose private directory lives exactly as long as this value.
pub struct PreparedJob {
    workspace: TempDir,
    job: Job,
}

impl PreparedJob {
    /// Stage an already validated bundle below `workspace_parent`.
    pub fn stage(
        workspace_parent: &Path,
        bundle: &Bundle,
        font_dirs: Vec<PathBuf>,
        limits: JobLimits,
    ) -> io::Result<Self> {
        std::fs::create_dir_all(workspace_parent)?;
        let workspace_parent = std::fs::canonicalize(workspace_parent)?;
        let workspace = Builder::new()
            .prefix("compile-")
            .tempdir_in(workspace_parent)?;
        let input_dir = workspace.path().join("input");
        let output_dir = workspace.path().join("output");
        std::fs::create_dir(&input_dir)?;
        std::fs::create_dir(&output_dir)?;

        let mut files = Vec::with_capacity(bundle.len());
        for (index, (path, content)) in bundle.files().enumerate() {
            // Never use a caller's virtual path as a host path. Numeric staging names
            // make traversal impossible even if a future caller bypasses Bundle.
            let source = input_dir.join(index.to_string());
            std::fs::write(&source, content.as_bytes())?;
            let kind = match content {
                FileContent::Text(_) => JobFileKind::Text,
                FileContent::Binary(_) => JobFileKind::Binary,
            };
            files.push(JobFile {
                path: path.to_owned(),
                source,
                kind,
            });
        }

        Ok(Self {
            job: Job {
                main: bundle.main().to_owned(),
                files,
                inputs: bundle.inputs().clone(),
                font_dirs,
                output_dir,
                limits,
            },
            workspace,
        })
    }

    pub fn job(&self) -> &Job {
        &self.job
    }

    pub fn limits_mut(&mut self) -> &mut JobLimits {
        &mut self.job.limits
    }

    /// Exposed for lifecycle verification and operational diagnostics only.
    pub fn workspace_path(&self) -> &Path {
        self.workspace.path()
    }

    /// Read fixed worker outputs. No worker-returned path is ever trusted.
    pub fn read_outputs(&self, previews: &[JobPreview]) -> io::Result<JobOutputs> {
        let pdf = std::fs::read(self.job.output_dir.join(PDF_NAME))?;
        let mut seen = HashSet::with_capacity(previews.len());
        let previews = previews
            .iter()
            .map(|preview| {
                if preview.page == 0 || !seen.insert(preview.page) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "worker returned invalid preview metadata",
                    ));
                }
                Ok(JobOutputPreview {
                    page: preview.page,
                    width: preview.width,
                    height: preview.height,
                    png: std::fs::read(self.job.output_dir.join(preview_filename(preview.page)))?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(JobOutputs { pdf, previews })
    }
}

impl AsRef<Job> for PreparedJob {
    fn as_ref(&self) -> &Job {
        &self.job
    }
}

/// Fixed preview filename inside the job output directory.
pub fn preview_filename(page: usize) -> String {
    format!("page-{page}.png")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::bundle::BundleFile;
    use crate::protocol::write_frame;

    #[test]
    fn a_large_bundle_produces_a_small_path_only_frame_and_cleans_up() {
        let root = tempfile::tempdir().expect("root");
        let marker = vec![0xA5; 4 * 1024 * 1024];
        let bundle = Bundle::new(
            "main.typ",
            vec![
                BundleFile::text("main.typ", "= Hi"),
                BundleFile::binary("large.bin", marker),
            ],
            BTreeMap::new(),
            8 * 1024 * 1024,
        )
        .expect("bundle");

        let prepared =
            PreparedJob::stage(root.path(), &bundle, vec![], JobLimits::default()).expect("stage");
        let workspace = prepared.workspace_path().to_owned();
        let mut frame = Vec::new();
        write_frame(&mut frame, prepared.job()).expect("frame");

        assert!(
            frame.len() < 4096,
            "control frame was {} bytes",
            frame.len()
        );
        assert!(workspace.exists());
        drop(prepared);
        assert!(!workspace.exists(), "workspace must be removed on drop");
    }
}
