//! Font discovery.
//!
//! Built once and shared. Font indexing is the one meaningful fixed cost in a compile
//! (measured at ~60 ms for a full brand family), so it must not happen per document
//! inside a long-lived process.

use std::path::Path;

use typst::utils::LazyHash;
use typst_kit::fonts::{self, FontStore};
use typst_library::text::{Font, FontBook};

/// A font family as reported to callers.
///
/// Exists so a model can look up what is actually installed instead of guessing a
/// name: `#set text(font: "Helvetica")` that silently falls back is the single most
/// common way a document comes out subtly wrong.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FontFamily {
    pub name: String,
    pub variants: usize,
}

/// The fonts available to a compile: Typst's embedded defaults plus any scanned dirs.
pub struct FontLibrary {
    store: FontStore,
}

impl FontLibrary {
    /// Embedded defaults only. Used by tests and by anything that does not ship fonts.
    pub fn embedded_only() -> Self {
        let mut store = FontStore::new();
        store.extend(fonts::embedded());
        Self { store }
    }

    /// Embedded defaults plus every font found under `dirs`.
    ///
    /// Directories are scanned in order, and a missing directory is not an error —
    /// the same binary runs in a container with baked fonts and on a laptop without.
    pub fn new<P: AsRef<Path>>(dirs: &[P]) -> Self {
        let mut store = FontStore::new();
        store.extend(fonts::embedded());
        for dir in dirs {
            store.extend(fonts::scan(dir.as_ref()));
        }
        Self { store }
    }

    /// Metadata for every known font. Satisfies `World::book`.
    pub fn book(&self) -> &LazyHash<FontBook> {
        self.store.book()
    }

    /// Load one font by index. Satisfies `World::font`.
    pub fn font(&self, index: usize) -> Option<Font> {
        self.store.font(index)
    }

    /// Every family, sorted, with how many variants each has.
    pub fn families(&self) -> Vec<FontFamily> {
        let mut families: Vec<FontFamily> = self
            .book()
            .families()
            .map(|(name, infos)| FontFamily {
                name: name.to_owned(),
                variants: infos.count(),
            })
            .collect();
        families.sort_by(|a, b| a.name.cmp(&b.name));
        families
    }

    /// Whether a family is present, matched the way Typst matches it.
    pub fn has_family(&self, name: &str) -> bool {
        self.book()
            .select_family(&name.to_lowercase())
            .next()
            .is_some()
    }
}

impl Default for FontLibrary {
    fn default() -> Self {
        Self::embedded_only()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_are_available() {
        let fonts = FontLibrary::embedded_only();
        let families = fonts.families();
        assert!(
            !families.is_empty(),
            "typst-assets should provide embedded fonts"
        );
        // Whatever else moves, Typst's own default family ships with the crate.
        assert!(
            fonts.has_family("libertinus serif") || fonts.has_family("new computer modern"),
            "expected a Typst default family, got: {:?}",
            families.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn families_are_sorted_and_deduplicated() {
        let fonts = FontLibrary::embedded_only();
        let names: Vec<_> = fonts.families().into_iter().map(|f| f.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
    }

    #[test]
    fn missing_font_directories_are_not_fatal() {
        let fonts = FontLibrary::new(&["/nonexistent/font/dir"]);
        assert!(!fonts.families().is_empty());
    }
}
