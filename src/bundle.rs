//! Untrusted paths and the in-memory file bundle.
//!
//! This is the only module allowed to interpret a caller-supplied path. Everything
//! downstream works with the normalised strings [`normalise_path`] returns, so there
//! is exactly one place to audit and exactly one place to test.
//!
//! The rule throughout is **reject, never sanitise**. A `..` segment is an error, not
//! something to resolve away: silently rewriting a hostile path hides the attempt and
//! leaves the next reader guessing which layer was responsible.

use std::collections::BTreeMap;

use thiserror::Error;

/// Longest accepted virtual path, in bytes.
pub const MAX_PATH_BYTES: usize = 255;

/// Most files a single bundle may contain.
pub const MAX_FILES: usize = 64;

/// Why a caller-supplied path was refused.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path is longer than {MAX_PATH_BYTES} bytes")]
    TooLong,
    #[error("path is absolute; bundle paths are relative to the bundle root")]
    Absolute,
    #[error("path contains a `..` segment; traversal is rejected, not resolved")]
    Traversal,
    #[error("path contains a `.` segment; write the path without it")]
    DotSegment,
    #[error("path contains an empty segment")]
    EmptySegment,
    #[error("path contains {0:?}, which is not allowed in a bundle path")]
    BadChar(char),
    #[error("path segment {0:?} starts or ends with a space, or ends with a dot")]
    PaddedSegment(String),
}

/// Characters a bundle path may contain.
///
/// A deliberate ASCII allowlist rather than a denylist of dangerous characters.
/// Virtual paths only ever need this much, and restricting the set removes an entire
/// class of problem — Unicode confusables, normalisation mismatches, RTL overrides,
/// and every OS-specific separator — without any code that has to reason about them.
fn is_allowed(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' ' | '/')
}

/// Validate a caller-supplied path and return it in canonical form.
///
/// Accepts relative, slash-separated paths of allowed characters. Everything else is
/// an error naming the specific problem, because these messages are read by a model
/// that has to correct its own output.
pub fn normalise_path(raw: &str) -> Result<String, PathError> {
    if raw.is_empty() {
        return Err(PathError::Empty);
    }
    if raw.len() > MAX_PATH_BYTES {
        return Err(PathError::TooLong);
    }
    if raw.starts_with('/') {
        return Err(PathError::Absolute);
    }
    if let Some(ch) = raw.chars().find(|c| !is_allowed(*c)) {
        return Err(PathError::BadChar(ch));
    }

    let mut segments = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" => return Err(PathError::EmptySegment),
            "." => return Err(PathError::DotSegment),
            ".." => return Err(PathError::Traversal),
            s if s.starts_with(' ') || s.ends_with(' ') || s.ends_with('.') => {
                return Err(PathError::PaddedSegment(s.to_owned()));
            }
            s => segments.push(s),
        }
    }

    Ok(segments.join("/"))
}

/// The contents of one bundle file.
///
/// Text and binary are distinguished because only text can arrive over MCP: a model
/// cannot emit a PNG, and asking it to costs ~1.37 tokens per byte, twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    Text(String),
    Binary(Vec<u8>),
}

impl FileContent {
    /// Size in bytes, for the bundle budget.
    pub fn len(&self) -> usize {
        match self {
            Self::Text(t) => t.len(),
            Self::Binary(b) => b.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The raw bytes, whichever variant this is.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Text(t) => t.as_bytes(),
            Self::Binary(b) => b,
        }
    }
}

/// One file on its way into a bundle, before validation.
#[derive(Debug, Clone)]
pub struct BundleFile {
    pub path: String,
    pub content: FileContent,
}

impl BundleFile {
    pub fn text(path: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: FileContent::Text(text.into()),
        }
    }

    pub fn binary(path: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            content: FileContent::Binary(bytes.into()),
        }
    }
}

/// Why a bundle was refused.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum BundleError {
    #[error("file {path:?}: {source}")]
    Path { path: String, source: PathError },
    #[error("bundle has {count} files; the limit is {MAX_FILES}")]
    TooManyFiles { count: usize },
    #[error("bundle is {actual} bytes; the limit is {limit}")]
    TooLarge { actual: usize, limit: usize },
    #[error("duplicate path {0:?}")]
    Duplicate(String),
    #[error("entrypoint {0:?} is not in the bundle")]
    MissingMain(String),
}

/// A validated set of files plus the entrypoint to compile.
///
/// Construction is the only way in, so holding a `Bundle` is proof that every path
/// was checked and every limit respected.
#[derive(Debug, Clone)]
pub struct Bundle {
    main: String,
    files: BTreeMap<String, FileContent>,
    inputs: BTreeMap<String, String>,
}

impl Bundle {
    /// Validate `files` and `main`, enforcing the path rules and the size budget.
    pub fn new(
        main: &str,
        files: Vec<BundleFile>,
        inputs: BTreeMap<String, String>,
        max_bytes: usize,
    ) -> Result<Self, BundleError> {
        if files.len() > MAX_FILES {
            return Err(BundleError::TooManyFiles { count: files.len() });
        }

        let total: usize = files.iter().map(|f| f.content.len()).sum();
        if total > max_bytes {
            return Err(BundleError::TooLarge {
                actual: total,
                limit: max_bytes,
            });
        }

        let main = normalise_path(main).map_err(|source| BundleError::Path {
            path: main.to_owned(),
            source,
        })?;

        let mut map = BTreeMap::new();
        for file in files {
            let path = normalise_path(&file.path).map_err(|source| BundleError::Path {
                path: file.path.clone(),
                source,
            })?;
            // Two different raw spellings can normalise to the same path, so this
            // has to be checked after normalisation, not before.
            if map.insert(path.clone(), file.content).is_some() {
                return Err(BundleError::Duplicate(path));
            }
        }

        if !map.contains_key(&main) {
            return Err(BundleError::MissingMain(main));
        }

        Ok(Self {
            main,
            files: map,
            inputs,
        })
    }

    pub fn main(&self) -> &str {
        &self.main
    }

    pub fn files(&self) -> impl Iterator<Item = (&str, &FileContent)> {
        self.files.iter().map(|(p, c)| (p.as_str(), c))
    }

    /// Consume the bundle and return its already-normalised files.
    ///
    /// Template ingestion uses this to put every archive member through the same
    /// path, duplicate, count and aggregate-size checks as a compile bundle before
    /// separating metadata files from document files.
    pub fn into_files(self) -> BTreeMap<String, FileContent> {
        self.files
    }

    pub fn inputs(&self) -> &BTreeMap<String, String> {
        &self.inputs
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_paths() {
        for path in ["main.typ", "assets/logo.svg", "a/b/c/d.json", "My File.typ"] {
            assert_eq!(normalise_path(path).as_deref(), Ok(path), "{path}");
        }
    }

    #[test]
    fn rejects_traversal_without_resolving_it() {
        // The point is that these are errors, not silently-rewritten paths.
        for path in ["../secrets.png", "assets/../../etc/passwd", "a/../b"] {
            assert_eq!(normalise_path(path), Err(PathError::Traversal), "{path}");
        }
    }

    #[test]
    fn rejects_absolute_paths() {
        for path in ["/etc/passwd", "/", "//x"] {
            assert_eq!(normalise_path(path), Err(PathError::Absolute), "{path}");
        }
    }

    #[test]
    fn rejects_dot_segments() {
        assert_eq!(normalise_path("./main.typ"), Err(PathError::DotSegment));
        assert_eq!(normalise_path("a/./b"), Err(PathError::DotSegment));
    }

    #[test]
    fn rejects_empty_segments() {
        assert_eq!(normalise_path("a//b"), Err(PathError::EmptySegment));
        assert_eq!(normalise_path("a/"), Err(PathError::EmptySegment));
    }

    #[test]
    fn rejects_empty_and_overlong() {
        assert_eq!(normalise_path(""), Err(PathError::Empty));
        let long = format!("{}.typ", "a".repeat(MAX_PATH_BYTES));
        assert_eq!(normalise_path(&long), Err(PathError::TooLong));
    }

    #[test]
    fn rejects_control_and_exotic_characters() {
        assert_eq!(normalise_path("a\0b"), Err(PathError::BadChar('\0')));
        assert_eq!(normalise_path("a\nb"), Err(PathError::BadChar('\n')));
        assert_eq!(normalise_path("a\\b"), Err(PathError::BadChar('\\')));
        // A Cyrillic 'а' looks exactly like ASCII 'a' in most fonts. Allowlisting
        // ASCII means we never have to reason about that.
        assert_eq!(normalise_path("mаin.typ"), Err(PathError::BadChar('а')));
    }

    #[test]
    fn rejects_padded_segments() {
        // Trailing dots and spaces are silently stripped by some filesystems, so a
        // path that looks distinct here could collide once written out.
        assert!(matches!(
            normalise_path("foo./bar"),
            Err(PathError::PaddedSegment(_))
        ));
        assert!(matches!(
            normalise_path(" foo/bar"),
            Err(PathError::PaddedSegment(_))
        ));
        assert!(matches!(
            normalise_path("foo /bar"),
            Err(PathError::PaddedSegment(_))
        ));
    }

    fn sample(files: Vec<BundleFile>) -> Result<Bundle, BundleError> {
        Bundle::new("main.typ", files, BTreeMap::new(), 8 * 1024 * 1024)
    }

    #[test]
    fn builds_a_valid_bundle() {
        let bundle = sample(vec![
            BundleFile::text("main.typ", "= Hi"),
            BundleFile::binary("assets/logo.png", vec![0x89, b'P']),
        ])
        .expect("valid bundle");
        assert_eq!(bundle.main(), "main.typ");
        assert_eq!(bundle.len(), 2);
    }

    #[test]
    fn rejects_missing_entrypoint() {
        let err = sample(vec![BundleFile::text("other.typ", "= Hi")]).unwrap_err();
        assert_eq!(err, BundleError::MissingMain("main.typ".into()));
    }

    #[test]
    fn rejects_duplicates_after_normalisation() {
        let err = sample(vec![
            BundleFile::text("main.typ", "a"),
            BundleFile::text("main.typ", "b"),
        ])
        .unwrap_err();
        assert_eq!(err, BundleError::Duplicate("main.typ".into()));
    }

    #[test]
    fn rejects_too_many_files() {
        let files = (0..=MAX_FILES)
            .map(|i| BundleFile::text(format!("f{i}.typ"), ""))
            .collect();
        assert!(matches!(
            sample(files),
            Err(BundleError::TooManyFiles { .. })
        ));
    }

    #[test]
    fn rejects_oversized_bundles() {
        let files = vec![BundleFile::text("main.typ", "x".repeat(100))];
        let err = Bundle::new("main.typ", files, BTreeMap::new(), 10).unwrap_err();
        assert_eq!(
            err,
            BundleError::TooLarge {
                actual: 100,
                limit: 10
            }
        );
    }

    #[test]
    fn propagates_the_offending_path_in_bundle_errors() {
        let err = sample(vec![BundleFile::text("../evil.typ", "")]).unwrap_err();
        match err {
            BundleError::Path { path, source } => {
                assert_eq!(path, "../evil.typ");
                assert_eq!(source, PathError::Traversal);
            }
            other => panic!("expected a path error, got {other:?}"),
        }
    }
}
