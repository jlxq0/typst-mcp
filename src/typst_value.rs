//! Emitting caller data as Typst literals.
//!
//! Templates are trusted and reviewed; the data fed into them is not. An invoice
//! renders a customer name supplied by whoever called the API, so if that name could
//! escape its string literal it could redefine the template's own bindings — change
//! a total, move the branding, redirect the bank details. Every value therefore goes
//! through this module and is emitted as a *typed literal*, never pasted into the
//! generated source.
//!
//! Identifiers are the one construct that must appear bare (a theme name is a binding,
//! not a string), and those are only ever produced from an allowlist the template
//! author wrote — never from caller input.

use std::fmt::Write as _;

use thiserror::Error;

/// A value that can be written into generated Typst source.
#[derive(Debug, Clone, PartialEq)]
pub enum TypstValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// A bare identifier, e.g. a binding the template defines. Only ever built from a
    /// template-author allowlist — see [`crate::templates`].
    Ident(String),
    Array(Vec<TypstValue>),
    Dict(Vec<(String, TypstValue)>),
    /// `datetime(year:, month:, day:)`.
    Date {
        year: i32,
        month: u8,
        day: u8,
    },
}

/// A value that cannot be represented in Typst source.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum ValueError {
    #[error("{0} is not a finite number; Typst has no literal for it")]
    NonFinite(String),
    #[error("{0:?} is not a valid Typst identifier")]
    BadIdent(String),
    #[error("{0:?} is not a valid dictionary key")]
    BadKey(String),
    #[error("date {year}-{month}-{day} is out of range")]
    BadDate { year: i32, month: u8, day: u8 },
}

impl TypstValue {
    /// Render as Typst source.
    pub fn to_source(&self) -> Result<String, ValueError> {
        let mut out = String::new();
        self.write(&mut out)?;
        Ok(out)
    }

    fn write(&self, out: &mut String) -> Result<(), ValueError> {
        match self {
            Self::None => out.push_str("none"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Self::Float(f) => {
                if !f.is_finite() {
                    return Err(ValueError::NonFinite(f.to_string()));
                }
                // `{:?}` round-trips exactly and always emits a decimal point, so an
                // integral float stays a float rather than silently changing type.
                let _ = write!(out, "{f:?}");
            }
            Self::Str(s) => write_string(s, out),
            Self::Ident(name) => {
                if !is_ident(name) {
                    return Err(ValueError::BadIdent(name.clone()));
                }
                out.push_str(name);
            }
            Self::Array(items) => {
                out.push('(');
                for item in items {
                    item.write(out)?;
                    // Always trailing: `(x)` is parenthesised x, `(x,)` is an array.
                    out.push_str(", ");
                }
                out.push(')');
            }
            Self::Dict(pairs) => {
                if pairs.is_empty() {
                    // `()` would be an empty array.
                    out.push_str("(:)");
                    return Ok(());
                }
                out.push('(');
                for (key, value) in pairs {
                    if !is_ident(key) {
                        return Err(ValueError::BadKey(key.clone()));
                    }
                    out.push_str(key);
                    out.push_str(": ");
                    value.write(out)?;
                    out.push_str(", ");
                }
                out.push(')');
            }
            Self::Date { year, month, day } => {
                if !(1..=12).contains(month) || !(1..=31).contains(day) {
                    return Err(ValueError::BadDate {
                        year: *year,
                        month: *month,
                        day: *day,
                    });
                }
                let _ = write!(out, "datetime(year: {year}, month: {month}, day: {day})");
            }
        }
        Ok(())
    }
}

/// Write a Typst string literal.
///
/// Everything outside a small set of printable characters is escaped as `\u{...}`.
/// An allowlist rather than "escape the dangerous ones", so a character nobody
/// thought about is escaped rather than passed through.
fn write_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Control characters, line/paragraph separators, and every other format
            // character get escaped: some of them are invisible in an editor but
            // meaningful to a parser.
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Whether `name` is a Typst identifier we are willing to emit bare.
///
/// Deliberately narrower than Typst's own grammar (which allows Unicode): ASCII
/// letters, digits, `_` and `-`, not starting with a digit or `-`.
pub fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Parse an ISO 8601 calendar date (`YYYY-MM-DD`).
pub fn parse_date(text: &str) -> Option<TypstValue> {
    let mut parts = text.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let day: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(TypstValue::Date { year, month, day })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(value: TypstValue) -> String {
        value.to_source().expect("valid value")
    }

    #[test]
    fn scalars_render_as_literals() {
        assert_eq!(src(TypstValue::None), "none");
        assert_eq!(src(TypstValue::Bool(true)), "true");
        assert_eq!(src(TypstValue::Int(-42)), "-42");
        assert_eq!(src(TypstValue::Float(1.5)), "1.5");
        assert_eq!(src(TypstValue::Str("hi".into())), r#""hi""#);
    }

    #[test]
    fn strings_cannot_escape_their_literal() {
        // The attack this module exists to stop: a "name" that closes the string and
        // appends code. Everything must stay inside the quotes.
        let hostile = r#"Bob" ; #let total = 0 ; #"#;
        let rendered = src(TypstValue::Str(hostile.into()));
        assert!(rendered.starts_with('"') && rendered.ends_with('"'));
        // Exactly two unescaped quotes: the delimiters.
        let unescaped = rendered
            .char_indices()
            .filter(|(i, c)| *c == '"' && (*i == 0 || !rendered[..*i].ends_with('\\')))
            .count();
        assert_eq!(unescaped, 2, "string broke out: {rendered}");
        assert!(
            rendered.contains(r#"\""#),
            "the inner quote must be escaped: {rendered}"
        );
    }

    #[test]
    fn backslashes_are_escaped_so_they_cannot_form_an_escape() {
        // `\` followed by `"` would otherwise let a trailing backslash swallow the
        // closing quote.
        assert_eq!(src(TypstValue::Str(r"a\".into())), r#""a\\""#);
    }

    #[test]
    fn invisible_characters_are_escaped() {
        for (input, expected) in [
            ("a\u{0}b", r#""a\u{0}b""#),
            ("a\u{2028}b", r#""a\u{2028}b""#),
            ("a\u{feff}b", r#""a\u{feff}b""#),
            ("a\nb", r#""a\nb""#),
        ] {
            assert_eq!(src(TypstValue::Str(input.into())), expected);
        }
    }

    #[test]
    fn unicode_text_is_preserved() {
        // Escaping must not mangle ordinary international text.
        assert_eq!(
            src(TypstValue::Str("Müller – 東京".into())),
            r#""Müller – 東京""#
        );
    }

    #[test]
    fn arrays_always_carry_a_trailing_comma() {
        // `(x)` is a parenthesised expression; only `(x,)` is a one-element array.
        assert_eq!(src(TypstValue::Array(vec![TypstValue::Int(1)])), "(1, )");
        assert_eq!(src(TypstValue::Array(vec![])), "()");
    }

    #[test]
    fn empty_dictionaries_use_the_colon_form() {
        assert_eq!(src(TypstValue::Dict(vec![])), "(:)");
        assert_eq!(
            src(TypstValue::Dict(vec![("a".into(), TypstValue::Int(1))])),
            "(a: 1, )"
        );
    }

    #[test]
    fn identifiers_are_validated() {
        assert_eq!(src(TypstValue::Ident("dark-theme".into())), "dark-theme");
        for bad in ["1abc", "-x", "a b", "a;b", "", "a\"b", "#evil()"] {
            assert!(
                TypstValue::Ident(bad.into()).to_source().is_err(),
                "{bad:?} must be refused as an identifier"
            );
        }
    }

    #[test]
    fn dictionary_keys_are_validated() {
        let bad = TypstValue::Dict(vec![("a: 1, evil".into(), TypstValue::Int(0))]);
        assert!(matches!(bad.to_source(), Err(ValueError::BadKey(_))));
    }

    #[test]
    fn non_finite_floats_are_refused() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                TypstValue::Float(value).to_source(),
                Err(ValueError::NonFinite(_))
            ));
        }
    }

    #[test]
    fn integral_floats_stay_floats() {
        // `1` would be an integer and could change how the template does arithmetic.
        assert_eq!(src(TypstValue::Float(1.0)), "1.0");
    }

    #[test]
    fn dates_render_as_constructor_calls() {
        assert_eq!(
            src(parse_date("2026-08-15").expect("valid")),
            "datetime(year: 2026, month: 8, day: 15)"
        );
    }

    #[test]
    fn malformed_dates_are_rejected() {
        for bad in [
            "2026-13-01",
            "2026-00-01",
            "2026-08-32",
            "2026-08",
            "2026-08-15-01",
            "x-y-z",
            "",
        ] {
            assert!(parse_date(bad).is_none(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn nested_structures_round_trip() {
        let value = TypstValue::Dict(vec![
            ("name".into(), TypstValue::Str("ACME".into())),
            (
                "lines".into(),
                TypstValue::Array(vec![
                    TypstValue::Str("one".into()),
                    TypstValue::Str("two".into()),
                ]),
            ),
        ]);
        assert_eq!(src(value), r#"(name: "ACME", lines: ("one", "two", ), )"#);
    }
}
