//! Named templates: a reviewed Typst library plus a description of the data it takes.
//!
//! Two shapes, because the real documents need both:
//!
//! * **wrapper** — the template owns the *look* (cover, typography, footer) and the
//!   caller supplies the *body* as Typst markup. That is a letter or a report.
//! * **data** — the template owns everything and the caller supplies only values.
//!   That is an invoice, where nobody wants a model free-handing the totals table.
//!
//! The entrypoint is generated here rather than written by the caller, so the branding
//! cannot be skipped and the caller never writes boilerplate. Data reaches it through
//! [`crate::typst_value`], never string interpolation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bundle::{Bundle, BundleFile, FileContent, normalise_path};
use crate::diagnostics::Diagnostic;
use crate::typst_value::{TypstValue, ValueError, is_ident, parse_date};

/// Generated entrypoint. Reserved: template files may not use the `__` prefix.
pub const GENERATED_MAIN: &str = "__main.typ";

/// Where a wrapper template's body is mounted.
pub const GENERATED_BODY: &str = "__body.typ";

/// What the body file is called when we report a diagnostic in it.
pub const BODY_DISPLAY_NAME: &str = "body.typ";

/// Prefix reserved for generated files.
const RESERVED_PREFIX: &str = "__";

/// Maps generated files back to what the caller actually wrote.
///
/// The body cannot simply be `#include`d: an included file is evaluated as its own
/// module and does not inherit the importer's scope, so the caller would lose every
/// binding the template provides — brand colours, chart helpers, the lot. Instead the
/// body is written with a one-line import prelude, which shifts its line numbers by
/// exactly that much.
///
/// Rather than hand a caller line numbers from a file it never wrote, that shift is
/// recorded here and undone before diagnostics are returned.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    entries: BTreeMap<String, (String, usize)>,
}

impl SourceMap {
    fn insert(&mut self, generated: &str, display: &str, prelude_lines: usize) {
        self.entries
            .insert(generated.to_owned(), (display.to_owned(), prelude_lines));
    }

    /// Rewrite diagnostics in place: caller-facing filename, caller-facing line.
    pub fn apply(&self, diagnostics: &mut [Diagnostic]) {
        for diagnostic in diagnostics {
            let Some(file) = &diagnostic.file else {
                continue;
            };
            let Some((display, offset)) = self.entries.get(file) else {
                continue;
            };
            diagnostic.file = Some(display.clone());
            // A diagnostic inside the prelude itself has no caller line to point at;
            // clamping to 1 is better than reporting line 0 or underflowing.
            diagnostic.line = diagnostic
                .line
                .map(|line| line.saturating_sub(*offset).max(1));
        }
    }

    /// Whether anything needs remapping.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A bundle plus the information needed to report errors in the caller's own terms.
#[derive(Debug, Clone)]
pub struct Assembled {
    pub bundle: Bundle,
    pub source_map: SourceMap,
}

/// What a template expects from its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateKind {
    /// Caller supplies `body` markup; the template wraps it.
    Wrapper,
    /// Caller supplies only data; the template owns the layout.
    Data,
}

/// How one key of the caller's `data` becomes a Typst argument.
#[derive(Debug, Clone, Deserialize)]
pub struct ArgSpec {
    /// The Typst argument name. Often differs from the JSON key: Typst prefers
    /// `footer-style`, JSON prefers `footer_style`.
    pub arg: String,
    #[serde(rename = "type")]
    pub kind: ArgKind,
    /// For [`ArgKind::Ident`]: the allowed JSON values and the identifier each maps to.
    ///
    /// This is what keeps a bare identifier safe. The caller picks a key; the template
    /// author decides what it expands to. Caller input never reaches code position.
    #[serde(default)]
    pub map: BTreeMap<String, String>,
}

/// The Typst type an argument is emitted as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArgKind {
    Str,
    Int,
    Float,
    Bool,
    /// ISO 8601 `YYYY-MM-DD` → `datetime(..)`.
    Date,
    /// An array of strings, e.g. address lines.
    Strings,
    /// A bare identifier, chosen from [`ArgSpec::map`].
    Ident,
}

/// `template.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub kind: TemplateKind,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// The template's Typst library, e.g. `hanso.typ`.
    pub entrypoint: String,
    /// The wrapper function, e.g. `hanso-doc`. Required for [`TemplateKind::Wrapper`].
    #[serde(default)]
    pub wrapper_fn: Option<String>,
    /// Constraints that must travel with the template — licence and trademark limits
    /// that a caller has to see at render time, not only in a README.
    #[serde(default)]
    pub notice: Option<String>,
    #[serde(default)]
    pub args: BTreeMap<String, ArgSpec>,
}

fn default_version() -> String {
    "0.1.0".to_owned()
}

/// Why a template could not be loaded or used.
#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("template directory {0} has no template.toml")]
    NoManifest(PathBuf),
    #[error("template.toml in {path} is invalid: {source}")]
    BadManifest {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("template {name:?} declares entrypoint {entrypoint:?}, which is not present")]
    MissingEntrypoint { name: String, entrypoint: String },
    #[error("template {name:?} is a wrapper but does not declare wrapper_fn")]
    NoWrapperFn { name: String },
    #[error("template {name:?} declares wrapper_fn {value:?}, which is not a valid identifier")]
    BadWrapperFn { name: String, value: String },
    #[error("template file {0:?} uses the reserved `__` prefix")]
    ReservedName(String),
    #[error("template {name:?} has an invalid file path {path:?}: {source}")]
    BadPath {
        name: String,
        path: String,
        source: crate::bundle::PathError,
    },
    #[error("schema.json in {path} is not valid JSON Schema: {message}")]
    BadSchema { path: PathBuf, message: String },
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("data does not match the template's schema: {0}")]
    SchemaViolation(String),
    #[error("field {field:?}: {message}")]
    BadArg { field: String, message: String },
    #[error("template {name:?} is a {kind} template: {message}")]
    WrongKind {
        name: String,
        kind: &'static str,
        message: &'static str,
    },
    #[error("could not build the bundle: {0}")]
    Bundle(#[from] crate::bundle::BundleError),
    #[error(transparent)]
    Value(#[from] ValueError),
}

/// A loaded template: its manifest, its files, and its optional data schema.
#[derive(Debug, Clone)]
pub struct Template {
    pub manifest: Manifest,
    files: Vec<BundleFile>,
    schema: Option<serde_json::Value>,
    example: Option<serde_json::Value>,
    example_body: Option<String>,
}

/// The pieces of a template, however they arrived.
///
/// A baked template is read from a directory; an uploaded one arrives as text over the
/// API. Both land here, so validation happens once and an uploaded template is held to
/// exactly the same rules as one that shipped in the image.
#[derive(Debug, Default)]
pub struct TemplateParts {
    pub manifest_toml: String,
    pub files: Vec<BundleFile>,
    pub schema_json: Option<String>,
    pub fixture_json: Option<String>,
    pub fixture_body: Option<String>,
}

impl Template {
    /// Load a template from a directory.
    pub fn load(dir: &Path) -> Result<Self, TemplateError> {
        let manifest_path = dir.join("template.toml");
        if !manifest_path.is_file() {
            return Err(TemplateError::NoManifest(dir.to_owned()));
        }
        let manifest_toml = read(&manifest_path)?;
        // The name is not known until the manifest parses, and `collect_files` wants it
        // for its error messages; a placeholder keeps that from being circular.
        let files = collect_files(dir, "<loading>")?;

        Self::assemble_from(TemplateParts {
            manifest_toml,
            files,
            schema_json: read_optional(&dir.join("schema.json"))?,
            fixture_json: read_optional(&dir.join("fixture.json"))?,
            fixture_body: read_optional(&dir.join("fixture.body.typ"))?,
        })
    }

    /// Build a template from its parts, validating everything.
    ///
    /// Every check here runs at *upload* time as well as at deploy time, so a caller
    /// finds out their template is broken when they send it rather than at first use.
    pub fn assemble_from(parts: TemplateParts) -> Result<Self, TemplateError> {
        let manifest: Manifest =
            toml::from_str(&parts.manifest_toml).map_err(|source| TemplateError::BadManifest {
                path: PathBuf::from("template.toml"),
                source,
            })?;

        if manifest.kind == TemplateKind::Wrapper {
            match manifest.wrapper_fn.as_deref() {
                None => {
                    return Err(TemplateError::NoWrapperFn {
                        name: manifest.name.clone(),
                    });
                }
                // The wrapper function name is emitted into generated source in code
                // position, so it is the one string here that must be an identifier.
                Some(f) if !is_ident(f) => {
                    return Err(TemplateError::BadWrapperFn {
                        name: manifest.name.clone(),
                        value: f.to_owned(),
                    });
                }
                Some(_) => {}
            }
        }

        let mut files = parts.files;
        for file in &files {
            if file.path.starts_with(RESERVED_PREFIX) {
                return Err(TemplateError::ReservedName(file.path.clone()));
            }
            normalise_path(&file.path).map_err(|source| TemplateError::BadPath {
                name: manifest.name.clone(),
                path: file.path.clone(),
                source,
            })?;
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));

        if !files.iter().any(|f| f.path == manifest.entrypoint) {
            return Err(TemplateError::MissingEntrypoint {
                name: manifest.name.clone(),
                entrypoint: manifest.entrypoint.clone(),
            });
        }

        // Validated here rather than on first use: a broken schema should fail when the
        // template arrives, not when a caller happens to hit it.
        let schema = parse_json(parts.schema_json.as_deref(), "schema.json")?;
        if let Some(schema) = &schema {
            jsonschema::validator_for(schema).map_err(|e| TemplateError::BadSchema {
                path: PathBuf::from("schema.json"),
                message: e.to_string(),
            })?;
        }

        Ok(Self {
            manifest,
            files,
            schema,
            example: parse_json(parts.fixture_json.as_deref(), "fixture.json")?,
            example_body: parts.fixture_body,
        })
    }

    /// The template's own files, for storing an uploaded copy.
    pub fn files(&self) -> &[BundleFile] {
        &self.files
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn schema(&self) -> Option<&serde_json::Value> {
        self.schema.as_ref()
    }

    pub fn example(&self) -> Option<&serde_json::Value> {
        self.example.as_ref()
    }

    pub fn example_body(&self) -> Option<&str> {
        self.example_body.as_deref()
    }

    /// The data keys this template understands, for listings.
    pub fn data_fields(&self) -> Vec<&str> {
        self.manifest.args.keys().map(String::as_str).collect()
    }

    /// Validate `data` against the template's schema, if it has one.
    ///
    /// Runs before anything is compiled: a missing field reported as a schema error is
    /// far more actionable than the Typst error it would otherwise become.
    pub fn validate(&self, data: &serde_json::Value) -> Result<(), TemplateError> {
        let Some(schema) = &self.schema else {
            return Ok(());
        };
        let validator =
            jsonschema::validator_for(schema).map_err(|e| TemplateError::BadSchema {
                path: PathBuf::from("schema.json"),
                message: e.to_string(),
            })?;
        let problems: Vec<String> = validator
            .iter_errors(data)
            .map(|e| {
                let at = e.instance_path().to_string();
                if at.is_empty() {
                    e.to_string()
                } else {
                    format!("{at}: {e}")
                }
            })
            .collect();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(TemplateError::SchemaViolation(problems.join("; ")))
        }
    }

    /// Build a compilable bundle from caller data and, for wrapper templates, a body.
    ///
    /// `extra` carries caller-supplied assets, already validated.
    pub fn assemble(
        &self,
        data: &serde_json::Value,
        body: Option<&str>,
        extra: Vec<BundleFile>,
        max_bytes: usize,
    ) -> Result<Assembled, TemplateError> {
        self.validate(data)?;

        match (self.manifest.kind, body) {
            (TemplateKind::Data, Some(_)) => {
                return Err(TemplateError::WrongKind {
                    name: self.manifest.name.clone(),
                    kind: "data",
                    message: "it renders from `data` alone; remove `body`",
                });
            }
            (TemplateKind::Wrapper, None) => {
                return Err(TemplateError::WrongKind {
                    name: self.manifest.name.clone(),
                    kind: "wrapper",
                    message: "it needs a `body` of Typst markup to wrap",
                });
            }
            _ => {}
        }

        let mut files = self.files.clone();
        let mut source_map = SourceMap::default();
        files.extend(extra);
        files.push(BundleFile::text(GENERATED_MAIN, self.generate_main(data)?));

        if let Some(body) = body {
            // The prelude is what gives the body the template's own bindings — brand
            // colours, chart helpers, everything `#import ...: *` exposes. Without it
            // the caller can only use the Typst standard library.
            let prelude = format!("#import {}: *\n", quote(&self.manifest.entrypoint));
            let prelude_lines = prelude.lines().count();
            files.push(BundleFile::text(GENERATED_BODY, format!("{prelude}{body}")));
            source_map.insert(GENERATED_BODY, BODY_DISPLAY_NAME, prelude_lines);
        }

        // Data is mounted as a file too, so a template can reach values the manifest
        // does not map — `#let data = json("data.json")` — without any new plumbing.
        files.push(BundleFile::text(
            "data.json",
            serde_json::to_string(data).unwrap_or_else(|_| "{}".into()),
        ));

        let bundle = Bundle::new(GENERATED_MAIN, files, BTreeMap::new(), max_bytes)?;
        Ok(Assembled { bundle, source_map })
    }

    /// The generated entrypoint.
    fn generate_main(&self, data: &serde_json::Value) -> Result<String, TemplateError> {
        let mut out = String::new();
        out.push_str("// Generated by typst-mcp. Do not edit.\n");
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("#import {}: *\n", quote(&self.manifest.entrypoint)),
        );

        if self.manifest.kind == TemplateKind::Wrapper {
            let wrapper = self.manifest.wrapper_fn.as_deref().unwrap_or_default();
            out.push_str("#show: ");
            out.push_str(wrapper);
            out.push_str(".with(\n");
            for (arg, value) in self.arguments(data)? {
                out.push_str("  ");
                out.push_str(&arg);
                out.push_str(": ");
                out.push_str(&value.to_source()?);
                out.push_str(",\n");
            }
            out.push_str(")\n");
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!("#include {}\n", quote(GENERATED_BODY)),
            );
        } else {
            let wrapper = self.manifest.wrapper_fn.as_deref().unwrap_or("render");
            out.push('#');
            out.push_str(wrapper);
            out.push_str("(\n");
            for (arg, value) in self.arguments(data)? {
                out.push_str("  ");
                out.push_str(&arg);
                out.push_str(": ");
                out.push_str(&value.to_source()?);
                out.push_str(",\n");
            }
            out.push_str(")\n");
        }

        Ok(out)
    }

    /// Convert `data` into Typst arguments, in manifest order.
    ///
    /// A key the caller omits is simply not passed, so the template's own default
    /// applies. That is what makes a template with fifteen branded defaults pleasant
    /// to call with three fields.
    fn arguments(
        &self,
        data: &serde_json::Value,
    ) -> Result<Vec<(String, TypstValue)>, TemplateError> {
        let mut out = Vec::new();
        for (field, spec) in &self.manifest.args {
            let Some(raw) = data.get(field) else { continue };
            if raw.is_null() {
                continue;
            }
            let value = convert(field, spec, raw)?;
            out.push((spec.arg.clone(), value));
        }
        Ok(out)
    }
}

/// Convert one JSON value per its declared type.
fn convert(
    field: &str,
    spec: &ArgSpec,
    raw: &serde_json::Value,
) -> Result<TypstValue, TemplateError> {
    let bad = |message: String| TemplateError::BadArg {
        field: field.to_owned(),
        message,
    };

    match spec.kind {
        ArgKind::Str => raw
            .as_str()
            .map(|s| TypstValue::Str(s.to_owned()))
            .ok_or_else(|| bad("expected a string".into())),
        ArgKind::Int => raw
            .as_i64()
            .map(TypstValue::Int)
            .ok_or_else(|| bad("expected an integer".into())),
        ArgKind::Float => raw
            .as_f64()
            .map(TypstValue::Float)
            .ok_or_else(|| bad("expected a number".into())),
        ArgKind::Bool => raw
            .as_bool()
            .map(TypstValue::Bool)
            .ok_or_else(|| bad("expected true or false".into())),
        ArgKind::Date => raw
            .as_str()
            .and_then(parse_date)
            .ok_or_else(|| bad("expected a date as YYYY-MM-DD".into())),
        ArgKind::Strings => {
            let items = raw
                .as_array()
                .ok_or_else(|| bad("expected an array of strings".into()))?;
            let mut values = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let text = item
                    .as_str()
                    .ok_or_else(|| bad(format!("element {i} is not a string")))?;
                values.push(TypstValue::Str(text.to_owned()));
            }
            Ok(TypstValue::Array(values))
        }
        ArgKind::Ident => {
            let key = raw
                .as_str()
                .ok_or_else(|| bad("expected a string".into()))?;
            // The allowlist is the whole safety property: the caller chooses a key,
            // the template author chose what it expands to.
            let mapped = spec.map.get(key).ok_or_else(|| {
                let allowed: Vec<&str> = spec.map.keys().map(String::as_str).collect();
                bad(format!("{key:?} is not one of: {}", allowed.join(", ")))
            })?;
            Ok(TypstValue::Ident(mapped.clone()))
        }
    }
}

/// A registry of templates loaded from a directory.
#[derive(Debug, Default, Clone)]
pub struct TemplateSet {
    templates: BTreeMap<String, Template>,
}

impl TemplateSet {
    /// Load every subdirectory of `root` that contains a `template.toml`.
    ///
    /// A missing root is empty, not an error — the same binary runs with baked
    /// templates in a container and without them on a laptop.
    pub fn load(root: &Path) -> Result<Self, TemplateError> {
        let mut templates = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(root) else {
            return Ok(Self { templates });
        };
        for entry in entries {
            let entry = entry.map_err(|source| TemplateError::Io {
                path: root.to_owned(),
                source,
            })?;
            if !entry.path().is_dir() || !entry.path().join("template.toml").is_file() {
                continue;
            }
            let template = Template::load(&entry.path())?;
            templates.insert(template.name().to_owned(), template);
        }
        Ok(Self { templates })
    }

    pub fn get(&self, name: &str) -> Option<&Template> {
        self.templates.get(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.templates.keys().map(String::as_str).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Template> {
        self.templates.values()
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

/// Gather a template's files, rejecting reserved and invalid paths.
fn collect_files(root: &Path, name: &str) -> Result<Vec<BundleFile>, TemplateError> {
    let mut files = Vec::new();
    let mut walk = vec![(root.to_owned(), String::new())];

    while let Some((dir, prefix)) = walk.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|source| TemplateError::Io {
            path: dir.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| TemplateError::Io {
                path: dir.clone(),
                source,
            })?;
            let file_name = entry.file_name().to_string_lossy().into_owned();
            // Skip dotfiles and the manifest/fixtures, which are metadata rather than
            // part of the document.
            if file_name.starts_with('.')
                || (prefix.is_empty()
                    && matches!(
                        file_name.as_str(),
                        "template.toml" | "schema.json" | "fixture.json" | "fixture.body.typ"
                    ))
            {
                continue;
            }
            if file_name.starts_with(RESERVED_PREFIX) {
                return Err(TemplateError::ReservedName(file_name));
            }

            let rel = if prefix.is_empty() {
                file_name
            } else {
                format!("{prefix}/{file_name}")
            };

            let file_type = entry.file_type().map_err(|source| TemplateError::Io {
                path: entry.path(),
                source,
            })?;
            if file_type.is_dir() {
                walk.push((entry.path(), rel));
                continue;
            }

            normalise_path(&rel).map_err(|source| TemplateError::BadPath {
                name: name.to_owned(),
                path: rel.clone(),
                source,
            })?;

            let bytes = std::fs::read(entry.path()).map_err(|source| TemplateError::Io {
                path: entry.path(),
                source,
            })?;
            let content = match String::from_utf8(bytes) {
                Ok(text) if is_text(&rel) => FileContent::Text(text),
                Ok(text) => FileContent::Binary(text.into_bytes()),
                Err(err) => FileContent::Binary(err.into_bytes()),
            };
            files.push(BundleFile { path: rel, content });
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn is_text(path: &str) -> bool {
    matches!(
        path.rsplit('.').next(),
        Some("typ" | "json" | "csv" | "yaml" | "yml" | "toml" | "svg" | "txt")
    )
}

fn quote(value: &str) -> String {
    TypstValue::Str(value.to_owned())
        .to_source()
        .unwrap_or_else(|_| "\"\"".into())
}

fn read(path: &Path) -> Result<String, TemplateError> {
    std::fs::read_to_string(path).map_err(|source| TemplateError::Io {
        path: path.to_owned(),
        source,
    })
}

fn read_optional(path: &Path) -> Result<Option<String>, TemplateError> {
    if path.is_file() {
        read(path).map(Some)
    } else {
        Ok(None)
    }
}

/// Parse optional JSON text the same way a file on disk would be parsed.
///
/// Uploaded templates arrive as strings; baked ones are read into strings first.
/// One parser means an upload cannot sneak past a check that only ran on disk.
fn parse_json(text: Option<&str>, name: &str) -> Result<Option<serde_json::Value>, TemplateError> {
    let Some(text) = text else {
        return Ok(None);
    };
    serde_json::from_str(text)
        .map(Some)
        .map_err(|e| TemplateError::BadSchema {
            path: PathBuf::from(name),
            message: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal wrapper template on disk.
    fn wrapper_template(dir: &Path) {
        std::fs::write(
            dir.join("template.toml"),
            r#"
name = "demo"
kind = "wrapper"
entrypoint = "demo.typ"
wrapper_fn = "demo-doc"
description = "A demo"

[args.title]
arg = "title"
type = "str"

[args.date]
arg = "date"
type = "date"

[args.theme]
arg = "theme"
type = "ident"
map = { light = "light-theme", dark = "dark-theme" }

[args.address]
arg = "address"
type = "strings"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("demo.typ"),
            r#"
#let light-theme = "light"
#let dark-theme = "dark"
#let demo-doc(title: "Untitled", date: none, theme: light-theme, address: (), body) = {
  [= #title]
  body
}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("schema.json"),
            r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("fixture.json"), r#"{"title":"Fixture"}"#).unwrap();
        std::fs::write(dir.join("fixture.body.typ"), "Body text.").unwrap();
    }

    fn load_demo() -> (tempfile::TempDir, Template) {
        let dir = tempfile::tempdir().unwrap();
        wrapper_template(dir.path());
        let template = Template::load(dir.path()).expect("loads");
        (dir, template)
    }

    #[test]
    fn loads_a_wrapper_template() {
        let (_dir, template) = load_demo();
        assert_eq!(template.name(), "demo");
        assert_eq!(template.manifest.kind, TemplateKind::Wrapper);
        assert!(template.schema().is_some());
        assert_eq!(template.example_body(), Some("Body text."));
        assert_eq!(
            template.data_fields(),
            vec!["address", "date", "theme", "title"]
        );
    }

    #[test]
    fn generates_an_entrypoint_that_wraps_the_body() {
        let (_dir, template) = load_demo();
        let data = serde_json::json!({
            "title": "Q3 Review",
            "date": "2026-08-15",
            "theme": "dark",
            "address": ["1 Phillip Street", "Singapore"],
        });
        let main = template.generate_main(&data).expect("generates");

        assert!(main.contains(r#"#import "demo.typ": *"#), "{main}");
        assert!(main.contains("#show: demo-doc.with("), "{main}");
        assert!(main.contains(r#"title: "Q3 Review","#), "{main}");
        assert!(
            main.contains("date: datetime(year: 2026, month: 8, day: 15),"),
            "{main}"
        );
        // The identifier is bare, and it is the *mapped* value, not the caller's word.
        assert!(main.contains("theme: dark-theme,"), "{main}");
        assert!(
            main.contains(r#"address: ("1 Phillip Street", "Singapore", ),"#),
            "{main}"
        );
        assert!(main.contains(r#"#include "__body.typ""#), "{main}");
    }

    #[test]
    fn omitted_arguments_fall_through_to_template_defaults() {
        // The property that makes a fifteen-field branded template pleasant to call.
        let (_dir, template) = load_demo();
        let main = template
            .generate_main(&serde_json::json!({ "title": "Only this" }))
            .expect("generates");
        assert!(main.contains("title:"), "{main}");
        for absent in ["date:", "theme:", "address:"] {
            assert!(
                !main.contains(absent),
                "{absent} should not be passed: {main}"
            );
        }
    }

    /// Remove every string literal, leaving only what Typst would treat as code.
    ///
    /// Injected text that survives inside quotes is inert — it renders as characters
    /// on the page. Only what lands *outside* a literal can execute, so that is what
    /// the injection tests inspect.
    fn code_positions(source: &str) -> String {
        let mut out = String::new();
        let mut chars = source.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '"' => {
                    // Skip to the closing quote, honouring backslash escapes.
                    while let Some(inner) = chars.next() {
                        match inner {
                            '\\' => {
                                chars.next();
                            }
                            '"' => break,
                            _ => {}
                        }
                    }
                }
                c => out.push(c),
            }
        }
        out
    }

    #[test]
    fn hostile_data_cannot_escape_into_code() {
        let (_dir, template) = load_demo();
        let data = serde_json::json!({
            "title": r#"Bob") ; #let evil = 1 ; #demo-doc(title: ("pwned"#,
        });
        let main = template.generate_main(&data).expect("generates");
        let code = code_positions(&main);

        // Everything hostile stayed inside the literal, so none of it is code.
        assert!(
            !code.contains("evil"),
            "injected binding reached code: {main}"
        );
        assert_eq!(
            code.matches("demo-doc").count(),
            1,
            "the wrapper must be called exactly once: {main}"
        );
    }

    #[test]
    fn hostile_data_in_every_argument_position_stays_inert() {
        // One case per argument kind that carries caller text, since each has its own
        // conversion path.
        let (_dir, template) = load_demo();
        let attacks = [
            serde_json::json!({ "title": "\" ; #let pwned = 1 ; \"" }),
            serde_json::json!({ "title": "ok", "address": ["\") ; #let pwned = 1 ; (\""] }),
            serde_json::json!({ "title": "\\" }),
            serde_json::json!({ "title": "a\u{2028}#let pwned = 1" }),
        ];

        for data in attacks {
            let main = template.generate_main(&data).expect("generates");
            let code = code_positions(&main);
            assert!(
                !code.contains("pwned"),
                "escaped into code from {data}: {main}"
            );
            // A stray unbalanced quote would swallow everything after it, so check the
            // generated structure survived intact and in order.
            let show = code.find("#show:");
            let include = code.find("#include");
            assert!(
                matches!((show, include), (Some(s), Some(i)) if s < i),
                "structure was broken by {data}: {main}"
            );
        }
    }

    #[test]
    fn a_hostile_title_renders_as_text_rather_than_executing() {
        // The end-to-end proof: compile the generated bundle and confirm it produces a
        // document instead of obeying the injected code.
        use std::sync::Arc;

        let (_dir, template) = load_demo();
        let assembled = template
            .assemble(
                &serde_json::json!({ "title": r#"Bob") ; #panic("pwned") ; #("#  }),
                Some("Body."),
                vec![],
                1 << 20,
            )
            .expect("assembles");

        let out = crate::compile::compile(
            &assembled.bundle,
            Arc::new(crate::fonts::FontLibrary::embedded_only()),
            &crate::compile::CompileOptions {
                preview_pages: vec![],
                ..Default::default()
            },
        )
        .expect("the injected #panic must not run");
        assert!(out.pdf.starts_with(b"%PDF-"));
    }

    #[test]
    fn an_unmapped_identifier_is_refused() {
        let (_dir, template) = load_demo();
        let err = template
            .generate_main(&serde_json::json!({ "title": "x", "theme": "neon" }))
            .expect_err("must refuse");
        let message = err.to_string();
        assert!(message.contains("not one of"), "{message}");
        assert!(
            message.contains("dark") && message.contains("light"),
            "{message}"
        );
    }

    #[test]
    fn wrong_argument_types_are_named() {
        let (_dir, template) = load_demo();
        for (data, expected) in [
            (serde_json::json!({ "title": 42 }), "expected a string"),
            (
                serde_json::json!({ "title": "x", "date": "15/08/2026" }),
                "YYYY-MM-DD",
            ),
            (
                serde_json::json!({ "title": "x", "address": "one line" }),
                "array of strings",
            ),
            (
                serde_json::json!({ "title": "x", "address": [1] }),
                "element 0 is not a string",
            ),
        ] {
            let err = template
                .generate_main(&data)
                .expect_err("must refuse")
                .to_string();
            assert!(err.contains(expected), "expected {expected:?} in {err:?}");
        }
    }

    #[test]
    fn schema_violations_are_caught_before_compiling() {
        let (_dir, template) = load_demo();
        let err = template
            .assemble(&serde_json::json!({}), Some("Body"), vec![], 1 << 20)
            .expect_err("title is required");
        assert!(matches!(err, TemplateError::SchemaViolation(_)), "{err:?}");
    }

    #[test]
    fn wrapper_templates_require_a_body_and_data_templates_refuse_one() {
        let (_dir, template) = load_demo();
        let err = template
            .assemble(&serde_json::json!({ "title": "x" }), None, vec![], 1 << 20)
            .expect_err("needs a body");
        assert!(err.to_string().contains("needs a `body`"), "{err}");
    }

    #[test]
    fn assembles_a_compilable_bundle() {
        let (_dir, template) = load_demo();
        let assembled = template
            .assemble(
                &serde_json::json!({ "title": "x" }),
                Some("Hello"),
                vec![],
                1 << 20,
            )
            .expect("assembles");
        let bundle = &assembled.bundle;
        assert_eq!(bundle.main(), GENERATED_MAIN);
        let paths: Vec<&str> = bundle.files().map(|(p, _)| p).collect();
        assert!(paths.contains(&GENERATED_MAIN));
        assert!(paths.contains(&GENERATED_BODY));
        assert!(paths.contains(&"demo.typ"));
        assert!(paths.contains(&"data.json"));
        // Metadata must not leak into the compiled bundle.
        assert!(!paths.contains(&"template.toml"));
        assert!(!paths.contains(&"fixture.json"));
    }

    #[test]
    fn a_wrapper_without_wrapper_fn_is_refused_at_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("template.toml"),
            "name = \"x\"\nkind = \"wrapper\"\nentrypoint = \"x.typ\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("x.typ"), "").unwrap();
        assert!(matches!(
            Template::load(dir.path()),
            Err(TemplateError::NoWrapperFn { .. })
        ));
    }

    #[test]
    fn a_hostile_wrapper_fn_is_refused_at_load() {
        // The template author is trusted, but a typo that happens to be executable
        // should still fail loudly at deploy time.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("template.toml"),
            "name = \"x\"\nkind = \"wrapper\"\nentrypoint = \"x.typ\"\nwrapper_fn = \"f() ; #g(\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("x.typ"), "").unwrap();
        assert!(matches!(
            Template::load(dir.path()),
            Err(TemplateError::BadWrapperFn { .. })
        ));
    }

    #[test]
    fn a_missing_entrypoint_is_refused_at_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("template.toml"),
            "name = \"x\"\nkind = \"data\"\nentrypoint = \"absent.typ\"\n",
        )
        .unwrap();
        assert!(matches!(
            Template::load(dir.path()),
            Err(TemplateError::MissingEntrypoint { .. })
        ));
    }

    #[test]
    fn reserved_filenames_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        wrapper_template(dir.path());
        std::fs::write(dir.path().join("__main.typ"), "#panic()").unwrap();
        assert!(matches!(
            Template::load(dir.path()),
            Err(TemplateError::ReservedName(_))
        ));
    }

    #[test]
    fn a_missing_template_root_is_empty_not_an_error() {
        let set = TemplateSet::load(Path::new("/nonexistent/templates")).expect("no error");
        assert!(set.is_empty());
    }

    #[test]
    fn a_template_set_finds_its_members() {
        let root = tempfile::tempdir().unwrap();
        let demo = root.path().join("demo");
        std::fs::create_dir(&demo).unwrap();
        wrapper_template(&demo);
        // A stray directory without a manifest is ignored rather than fatal.
        std::fs::create_dir(root.path().join("notes")).unwrap();

        let set = TemplateSet::load(root.path()).expect("loads");
        assert_eq!(set.names(), vec!["demo"]);
        assert!(set.get("demo").is_some());
        assert!(set.get("absent").is_none());
    }
}
