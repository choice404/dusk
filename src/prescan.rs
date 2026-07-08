//! Pre scan pass: a cheap line oriented sweep collecting `@paradigm` and
//! `@import` directives before lexing and parsing.

use crate::diag::{Diagnostic, Span};

/// A paradigm a file may declare. Default when none is present is `Procedural`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paradigm {
    Functional,
    Procedural,
    Oop,
}

impl Paradigm {
    pub fn lookup(s: &str) -> Option<Paradigm> {
        match s {
            "functional" => Some(Paradigm::Functional),
            "procedural" => Some(Paradigm::Procedural),
            "oop" => Some(Paradigm::Oop),
            _ => None,
        }
    }
}

/// The directives collected from the top of a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Prescan {
    pub paradigms: Vec<Paradigm>,
    pub imports: Vec<String>,
}

impl Prescan {
    /// The effective paradigm set, defaulting to `Procedural` when none are declared.
    pub fn effective(&self) -> Vec<Paradigm> {
        if self.paradigms.is_empty() {
            vec![Paradigm::Procedural]
        } else {
            self.paradigms.clone()
        }
    }
}

/// Collects directives from the leading lines of a file. Scanning stops at the
/// first line that is neither blank, a comment, nor a directive. Reports unknown
/// paradigms and unknown `@` directives as diagnostics.
pub fn scan(src: &str) -> (Prescan, Vec<Diagnostic>) {
    let mut paradigms = Vec::new();
    let mut imports = Vec::new();
    let mut errors = Vec::new();
    let mut offset = 0u32;
    for raw in src.lines() {
        let line_start = offset;
        // Advance past this line's content and its actual terminator. `lines()`
        // strips a `\r\n` or `\n`, so counting only `raw.len() + 1` drifts by one
        // byte on every CRLF line and misplaces a later directive's span. Measure
        // the terminator from the source instead: two bytes for `\r\n`, one for a
        // lone `\n`, and none at end of input with no trailing newline.
        let after = line_start as usize + raw.len();
        let term = if src[after..].starts_with("\r\n") {
            2
        } else if src[after..].starts_with('\n') {
            1
        } else {
            0
        };
        offset = after as u32 + term;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let span = Span::new(line_start, line_start + raw.len() as u32);
        let mut parts = line.splitn(2, char::is_whitespace);
        let head = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        let rest = rest.split("//").next().unwrap_or("").trim();
        match head {
            "@paradigm" => match Paradigm::lookup(rest) {
                Some(p) => paradigms.push(p),
                None => errors.push(Diagnostic::new(format!("unknown paradigm '{rest}'"), span)),
            },
            "@import" => {
                let rest = rest.trim_matches('"');
                if rest.is_empty() {
                    errors.push(Diagnostic::new("empty import path", span));
                } else {
                    imports.push(rest.to_string());
                }
            }
            _ if head.starts_with('@') => {
                errors.push(Diagnostic::new(format!("unknown directive '{head}'"), span));
            }
            _ => break,
        }
    }
    (Prescan { paradigms, imports }, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_paradigms_and_imports() {
        let src = "\
@paradigm procedural
@paradigm functional

@import std.io
@import std.functional.maybe

func main() -> int32 { return 0 }
@import ignored.after.decl";
        let (pre, errs) = scan(src);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        assert_eq!(pre.paradigms, vec![Paradigm::Procedural, Paradigm::Functional]);
        assert_eq!(
            pre.imports,
            vec!["std.io".to_string(), "std.functional.maybe".to_string()]
        );
    }

    #[test]
    fn defaults_to_procedural() {
        let (pre, _) = scan("func main() -> int32 { return 0 }");
        assert_eq!(pre.effective(), vec![Paradigm::Procedural]);
    }

    #[test]
    fn strips_trailing_comment_on_directive() {
        let (pre, errs) = scan("@import std.io // a comment");
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        assert_eq!(pre.imports, vec!["std.io".to_string()]);
    }

    #[test]
    fn unknown_paradigm_errors() {
        let (_pre, errs) = scan("@paradigm functionl");
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn unknown_directive_errors() {
        let (_pre, errs) = scan("@imprt std.io");
        assert_eq!(errs.len(), 1);
    }
}
