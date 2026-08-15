//! `BundleWorld` — the sandbox.
//!
//! Typst reaches the outside world only through the [`World`] trait. Implementing it
//! over an in-memory map means there is no filesystem to escape and no network to
//! reach, because neither is ever handed over. `#read("/etc/passwd")` and
//! `#image("../../secrets.png")` both arrive at [`BundleWorld::file`] and get a plain
//! "file not found" — that one lookup *is* the containment.
//!
//! Everything is populated before `compile()` is called: no interior mutability, no
//! lazy disk reads part-way through a compile, and no question about what a `World`
//! method might touch while the compiler holds it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use typst::LibraryExt;
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::utils::LazyHash;
use typst_library::diag::{FileError, FileResult};
use typst_library::foundations::{Bytes, Datetime, Dict, Duration, IntoValue, Str};
use typst_library::text::{Font, FontBook};
use typst_library::{Library, World};

use crate::bundle::{Bundle, FileContent};
use crate::fonts::FontLibrary;

/// Compile-time view of one job's files, fonts and inputs.
pub struct BundleWorld {
    library: LazyHash<Library>,
    fonts: Arc<FontLibrary>,
    main: FileId,
    sources: HashMap<FileId, Source>,
    files: HashMap<FileId, Bytes>,
    today: Option<Datetime>,
}

impl BundleWorld {
    /// Build a world from a validated bundle.
    ///
    /// `today` is fixed here rather than read per call so a document is internally
    /// consistent — and so a compile can be made reproducible by pinning it.
    pub fn new(
        bundle: &Bundle,
        fonts: Arc<FontLibrary>,
        today: Option<Datetime>,
    ) -> Result<Self, WorldError> {
        let inputs: Dict = bundle
            .inputs()
            .iter()
            .map(|(k, v)| (Str::from(k.as_str()), v.as_str().into_value()))
            .collect();

        let library = Library::builder().with_inputs(inputs).build();

        let mut sources = HashMap::new();
        let mut files = HashMap::new();

        for (path, content) in bundle.files() {
            let id = project_file(path)?;
            // Every file is readable as bytes; only Typst sources are parsed. Parsing
            // a large JSON as markup would be pure waste, and `source()` on a non-Typst
            // file is a caller mistake worth reporting as such.
            files.insert(id, Bytes::new(content.as_bytes().to_vec()));
            if let FileContent::Text(text) = content
                && path.ends_with(".typ")
            {
                sources.insert(id, Source::new(id, text.clone()));
            }
        }

        Ok(Self {
            library: LazyHash::new(library),
            fonts,
            main: project_file(bundle.main())?,
            sources,
            files,
            today,
        })
    }

    /// The entrypoint's id, for callers that need to resolve spans against it.
    pub fn main_id(&self) -> FileId {
        self.main
    }

    /// Why a lookup failed, phrased for whoever has to fix the document.
    fn missing(id: FileId) -> FileError {
        // A package import is a different failure from a typo, and saying so saves a
        // model from retrying the same `@preview` import forever.
        if matches!(id.root(), VirtualRoot::Package(_)) {
            return FileError::Other(Some(
                "Typst packages (`@preview/...`) are not available on this server; \
                 everything the document needs must be in the bundle"
                    .into(),
            ));
        }
        FileError::NotFound(PathBuf::from(id.vpath().get_without_slash()))
    }
}

/// Something the compiler refused before a compile could even start.
#[derive(Debug, thiserror::Error)]
pub enum WorldError {
    #[error("path {path:?} was rejected by the compiler: {source}")]
    Path {
        path: String,
        source: typst::syntax::PathError,
    },
}

/// Intern a bundle-relative path as a project-rooted [`FileId`].
///
/// Typst validates paths itself as well, so this is fallible even though
/// `bundle::normalise_path` has already run — belt and braces, and a `Result` rather
/// than an `expect` because a panic here would abort the process.
///
/// Never `FileId::unique()`: that skips the interner's dedup map and leaks a slot on
/// every call, and the interner is a permanently-leaked `u16` capped at 65 535.
fn project_file(path: &str) -> Result<FileId, WorldError> {
    let vpath = VirtualPath::new(path).map_err(|source| WorldError::Path {
        path: path.to_owned(),
        source,
    })?;
    Ok(RootedPath::new(VirtualRoot::Project, vpath).intern())
}

impl World for BundleWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.sources
            .get(&id)
            .cloned()
            .ok_or_else(|| Self::missing(id))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files
            .get(&id)
            .cloned()
            .ok_or_else(|| Self::missing(id))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        // The offset is ignored on purpose: a document renders against one fixed date
        // regardless of the caller's timezone, which is what makes output reproducible.
        self.today
    }
}

/// Today's date in UTC, without pulling in a date library.
pub fn utc_today() -> Option<Datetime> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    Datetime::from_ymd(year, month, day)
}

/// Days since the Unix epoch → (year, month, day).
///
/// Howard Hinnant's `civil_from_days`, which is exact for the whole proleptic
/// Gregorian range and needs no dependency.
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8; // [1, 12]
    ((y + i64::from(m <= 2)) as i32, m, d)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::bundle::{Bundle, BundleFile};

    fn world(files: Vec<BundleFile>) -> BundleWorld {
        let bundle = Bundle::new("main.typ", files, BTreeMap::new(), 1 << 20).expect("bundle");
        BundleWorld::new(&bundle, Arc::new(FontLibrary::embedded_only()), None).expect("world")
    }

    #[test]
    fn resolves_bundled_sources_and_files() {
        let w = world(vec![
            BundleFile::text("main.typ", "= Hi"),
            BundleFile::binary("logo.png", vec![1, 2, 3]),
        ]);
        assert!(w.source(w.main_id()).is_ok());
        assert_eq!(
            w.file(project_file("logo.png").unwrap())
                .unwrap()
                .as_slice(),
            &[1, 2, 3]
        );
    }

    #[test]
    fn unknown_files_are_not_found_rather_than_read() {
        let w = world(vec![BundleFile::text("main.typ", "= Hi")]);
        // The path a traversal attempt would produce still resolves to a plain miss.
        for path in ["etc/passwd", "secrets.png", "assets/logo.svg"] {
            assert!(
                matches!(
                    w.file(project_file(path).unwrap()),
                    Err(FileError::NotFound(_))
                ),
                "{path}"
            );
        }
    }

    #[test]
    fn package_imports_explain_themselves() {
        let w = world(vec![BundleFile::text("main.typ", "= Hi")]);
        let spec = "@preview/cetz:0.3.1".parse().expect("package spec");
        let vpath = VirtualPath::new("lib.typ").expect("valid path");
        let id = RootedPath::new(VirtualRoot::Package(spec), vpath).intern();
        let err = w.file(id).unwrap_err();
        assert!(format!("{err:?}").contains("not available"), "got {err:?}");
    }

    #[test]
    fn non_typst_text_is_bytes_but_not_a_source() {
        let w = world(vec![
            BundleFile::text("main.typ", "= Hi"),
            BundleFile::text("data.json", r#"{"a":1}"#),
        ]);
        let id = project_file("data.json").unwrap();
        assert!(w.file(id).is_ok(), "readable as bytes");
        assert!(w.source(id).is_err(), "not parsed as Typst");
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29)); // leap day
        assert_eq!(civil_from_days(20_680), (2026, 8, 15));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn today_is_a_real_date() {
        let today = utc_today().expect("a date");
        assert!(today.year().is_some_and(|y| y >= 2026));
    }
}
