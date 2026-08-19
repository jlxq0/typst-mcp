//! The wire format between the server and a compile worker.
//!
//! Length-prefixed JSON: a `u32` little-endian byte count, then the payload. Both
//! sides are this binary, so the format only has to survive a process boundary, not
//! a version skew.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::diagnostics::Diagnostic;

/// Refuse to even allocate for a frame larger than this.
///
/// A length prefix read off a pipe is attacker-influenced input in the same way a
/// network header is; without a ceiling, a bad `u32` is a 4 GiB allocation.
pub const MAX_FRAME_BYTES: u32 = 64 * 1024 * 1024;

/// How the worker reconstructs a staged bundle file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobFileKind {
    Text,
    Binary,
}

/// One validated virtual file and its parent-created staging path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobFile {
    pub path: String,
    pub source: PathBuf,
    pub kind: JobFileKind,
}

/// Everything a worker needs to produce one document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Job {
    pub main: String,
    pub files: Vec<JobFile>,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    #[serde(default)]
    pub font_dirs: Vec<PathBuf>,
    /// Parent-created directory in which the worker writes fixed output names.
    pub output_dir: PathBuf,
    pub limits: JobLimits,
}

impl AsRef<Job> for Job {
    fn as_ref(&self) -> &Job {
        self
    }
}

/// Bounds the worker enforces on itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobLimits {
    pub max_bundle_bytes: usize,
    pub max_pages: usize,
    pub preview_pages: Vec<usize>,
    pub preview_scale_millis: u32,
    pub preview_max_px: u32,
    /// `RLIMIT_AS` for the worker process, in bytes.
    pub memory_bytes: u64,
}

impl Default for JobLimits {
    fn default() -> Self {
        Self {
            max_bundle_bytes: 8 * 1024 * 1024,
            max_pages: 200,
            preview_pages: vec![1],
            // Scale travels as an integer because floats do not round-trip through
            // JSON identically on every platform, and this frame must be stable.
            preview_scale_millis: 1000,
            preview_max_px: 2000,
            memory_bytes: 512 * 1024 * 1024,
        }
    }
}

/// A rendered page's metadata. The PNG itself stays in the job workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobPreview {
    pub page: usize,
    pub width: u32,
    pub height: u32,
}

/// What a worker sends back. Failure is a value, not an exit code, so the parent can
/// tell "your document is broken" apart from "the worker died".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum JobResult {
    Ok {
        pages: usize,
        previews: Vec<JobPreview>,
        diagnostics: Vec<Diagnostic>,
    },
    Failed {
        message: String,
        diagnostics: Vec<Diagnostic>,
    },
    /// Staging or output I/O failed. Details stay on worker stderr and never become a
    /// caller-visible document error.
    Internal,
}

/// Write a length-prefixed JSON frame.
///
/// Enforces the same ceiling as [`read_frame`]. An asymmetric limit is worse than no
/// limit: the writer would succeed, the reader would refuse what it just received, and
/// the failure would surface far from its cause with the payload already spent.
pub fn write_frame<W: Write, T: Serialize>(mut out: W, value: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(value)?;
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|n| *n <= MAX_FRAME_BYTES)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "frame of {} bytes exceeds the {MAX_FRAME_BYTES} byte limit",
                    payload.len()
                ),
            )
        })?;
    out.write_all(&len.to_le_bytes())?;
    out.write_all(&payload)?;
    out.flush()
}

/// Read a length-prefixed JSON frame.
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(mut input: R) -> io::Result<T> {
    let mut len = [0u8; 4];
    input.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the {MAX_FRAME_BYTES} byte limit"),
        ));
    }
    let mut payload = vec![0u8; len as usize];
    input.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_job() -> Job {
        Job {
            main: "main.typ".into(),
            files: vec![
                JobFile {
                    path: "main.typ".into(),
                    source: "/tmp/job/input/0".into(),
                    kind: JobFileKind::Text,
                },
                JobFile {
                    path: "logo.png".into(),
                    source: "/tmp/job/input/1".into(),
                    kind: JobFileKind::Binary,
                },
            ],
            inputs: BTreeMap::from([("locale".into(), "de".into())]),
            font_dirs: vec![],
            output_dir: "/tmp/job/output".into(),
            limits: JobLimits::default(),
        }
    }

    #[test]
    fn frames_round_trip() {
        let job = sample_job();
        let mut buf = Vec::new();
        write_frame(&mut buf, &job).expect("write");
        let back: Job = read_frame(buf.as_slice()).expect("read");
        assert_eq!(job, back);
    }

    #[test]
    fn frames_contain_paths_and_metadata_not_bulk_bytes() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &sample_job()).expect("write");
        let frame = String::from_utf8(buf[4..].to_vec()).expect("json");
        assert!(frame.contains("/tmp/job/input/1"));
        assert!(!frame.contains("base64"));
        assert!(!frame.contains("= Hi"));
    }

    #[test]
    fn result_frames_contain_metadata_not_rendered_bytes() {
        let result = JobResult::Ok {
            pages: 1,
            previews: vec![JobPreview {
                page: 1,
                width: 800,
                height: 600,
            }],
            diagnostics: vec![],
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &result).expect("write");
        let frame = String::from_utf8(buf[4..].to_vec()).expect("json");
        assert!(!frame.contains("%PDF-"));
        assert!(!frame.contains("png_base64"));
        assert!(!frame.contains("pdf_base64"));
    }

    #[test]
    fn oversized_frames_are_refused_before_allocating() {
        // A hostile length prefix must not become a 4 GiB allocation.
        let mut buf = u32::MAX.to_le_bytes().to_vec();
        buf.extend_from_slice(b"{}");
        let err = read_frame::<_, Job>(buf.as_slice()).expect_err("must refuse");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn truncated_frames_are_an_error_not_a_hang() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &sample_job()).expect("write");
        buf.truncate(buf.len() / 2);
        assert!(read_frame::<_, Job>(buf.as_slice()).is_err());
    }

    #[test]
    fn oversized_frames_are_refused_on_write_too() {
        // The limit has to be symmetric, or the worker spends a whole compile producing
        // a frame the parent will then reject.
        let huge = JobResult::Failed {
            message: "A".repeat(MAX_FRAME_BYTES as usize + 1),
            diagnostics: vec![],
        };
        let err = write_frame(Vec::new(), &huge).expect_err("must refuse");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn results_round_trip() {
        let result = JobResult::Failed {
            message: "nope".into(),
            diagnostics: vec![crate::diagnostics::Diagnostic::bare(
                crate::diagnostics::Severity::Error,
                "boom",
            )],
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &result).expect("write");
        assert_eq!(
            read_frame::<_, JobResult>(buf.as_slice()).expect("read"),
            result
        );
    }
}
