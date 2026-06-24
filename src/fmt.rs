//! Compile time format string parsing for the `print` and `println` builtins.
//!
//! A format string is a literal with `{}` holes that the arguments fill in order,
//! and `{{` or `}}` for a literal brace. Parsing happens at compile time, so each
//! hole expands to a direct typed print with no runtime format parser and no
//! allocation, and a hole count that does not match the argument count is a
//! compile error. The same parser validates the call in sema and emits it in
//! codegen, so the two never disagree.

/// One piece of a parsed format string, literal text or a hole to fill.
#[derive(Debug, PartialEq)]
pub enum Seg {
    Lit(String),
    Hole,
}

/// Splits a format string into literal and hole segments, or returns an error
/// message for an unmatched brace.
pub fn parse(s: &str) -> Result<Vec<Seg>, String> {
    let mut segs = Vec::new();
    let mut lit = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '{' if it.peek() == Some(&'{') => {
                it.next();
                lit.push('{');
            }
            '}' if it.peek() == Some(&'}') => {
                it.next();
                lit.push('}');
            }
            '{' if it.peek() == Some(&'}') => {
                it.next();
                if !lit.is_empty() {
                    segs.push(Seg::Lit(std::mem::take(&mut lit)));
                }
                segs.push(Seg::Hole);
            }
            '{' => {
                return Err("unmatched '{' in format string, write '{{' for a literal brace".into())
            }
            '}' => {
                return Err("unmatched '}' in format string, write '}}' for a literal brace".into())
            }
            _ => lit.push(c),
        }
    }
    if !lit.is_empty() {
        segs.push(Seg::Lit(lit));
    }
    Ok(segs)
}

/// The number of holes in a parsed format string.
pub fn holes(segs: &[Seg]) -> usize {
    segs.iter().filter(|s| matches!(s, Seg::Hole)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_literals_and_holes() {
        let segs = parse("a {} b {}").unwrap();
        assert_eq!(
            segs,
            vec![
                Seg::Lit("a ".into()),
                Seg::Hole,
                Seg::Lit(" b ".into()),
                Seg::Hole
            ]
        );
        assert_eq!(holes(&segs), 2);
    }

    #[test]
    fn bare_hole_has_no_surrounding_literal() {
        assert_eq!(parse("{}").unwrap(), vec![Seg::Hole]);
    }

    #[test]
    fn escapes_become_literal_braces() {
        assert_eq!(parse("{{}}").unwrap(), vec![Seg::Lit("{}".into())]);
        assert_eq!(holes(&parse("{{}}").unwrap()), 0);
    }

    #[test]
    fn escaped_brace_next_to_a_hole() {
        let segs = parse("{{{}}}").unwrap();
        // `{{` then `{}` then `}}` -> literal `{`, a hole, literal `}`.
        assert_eq!(
            segs,
            vec![Seg::Lit("{".into()), Seg::Hole, Seg::Lit("}".into())]
        );
    }

    #[test]
    fn unmatched_brace_is_an_error() {
        assert!(parse("a { b").is_err());
        assert!(parse("a } b").is_err());
    }
}
