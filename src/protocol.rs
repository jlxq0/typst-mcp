//! The wire format between the server and a compile worker.
//!
//! Length-prefixed JSON: a `u32` little-endian byte count, then the payload. Both
//! sides are this binary, so the format only has to survive a process boundary, not
//! a version skew.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

use crate::bundle::FileContent;
use crate::diagnostics::Diagnostic;

/// Refuse to even allocate for a frame larger than this.
///
/// A length prefix read off a pipe is attacker-influenced input in the same way a
/// network header is; without a ceiling, a bad `u32` is a 4 GiB allocation.
pub const MAX_FRAME_BYTES: u32 = 64 * 1024 * 1024;

/// One file on its way to a worker.
///
/// `Path` exists so bulk bytes can stay out of the pipe once the content store lands:
/// the parent validates a path, the worker opens only paths from the job it was given.
/// Until then everything travels inline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum JobContent {
    Text { text: String },
    Base64 { data: String },
    Path { path: PathBuf },
}

impl JobContent {
    pub fn from_content(content: &FileContent) -> Self {
        match content {
            FileContent::Text(text) => Self::Text { text: text.clone() },
            FileContent::Binary(bytes) => Self::Base64 {
                data: BASE64.encode(bytes),
            },
        }
    }

    /// Materialise the content, reading from disk only for `Path` entries.
    pub fn resolve(&self) -> io::Result<FileContent> {
        match self {
            Self::Text { text } => Ok(FileContent::Text(text.clone())),
            Self::Base64 { data } => BASE64
                .decode(data)
                .map(FileContent::Binary)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Self::Path { path } => std::fs::read(path).map(FileContent::Binary),
        }
    }
}

/// One file of a job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobFile {
    pub path: String,
    #[serde(flatten)]
    pub content: JobContent,
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
    pub limits: JobLimits,
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

/// A rendered page as it crosses the pipe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobPreview {
    pub page: usize,
    pub width: u32,
    pub height: u32,
    pub png_base64: String,
}

/// What a worker sends back. Failure is a value, not an exit code, so the parent can
/// tell "your document is broken" apart from "the worker died".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum JobResult {
    Ok {
        pdf_base64: String,
        pages: usize,
        previews: Vec<JobPreview>,
        diagnostics: Vec<Diagnostic>,
    },
    Failed {
        message: String,
        diagnostics: Vec<Diagnostic>,
    },
}

/// Write a length-prefixed JSON frame.
pub fn write_frame<W: Write, T: Serialize>(mut out: W, value: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(value)?;
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
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
                    content: JobContent::Text {
                        text: "= Hi".into(),
                    },
                },
                JobFile {
                    path: "logo.png".into(),
                    content: JobContent::from_content(&FileContent::Binary(vec![1, 2, 3])),
                },
            ],
            inputs: BTreeMap::from([("locale".into(), "de".into())]),
            font_dirs: vec![],
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
    fn binary_content_survives_the_pipe() {
        let content = FileContent::Binary(vec![0x89, b'P', b'N', b'G', 0, 255]);
        let encoded = JobContent::from_content(&content);
        assert_eq!(encoded.resolve().expect("decode"), content);
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
