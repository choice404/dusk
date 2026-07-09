//! Source spans and diagnostics.

/// A half open byte range `lo..hi` into a single source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    pub fn new(lo: u32, hi: u32) -> Self {
        Span { lo, hi }
    }
}

/// A compile error: a message and the span it points at.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub msg: String,
    pub span: Span,
}

impl Diagnostic {
    pub fn new(msg: impl Into<String>, span: Span) -> Self {
        Diagnostic {
            msg: msg.into(),
            span,
        }
    }

    /// Renders against source text as a header line, `line:col: error: msg`,
    /// then the source line the span starts on and a caret run under it.
    pub fn render(&self, src: &str) -> String {
        self.render_local(src, self.span.lo)
    }

    /// Renders like `render`, taking the position from `local`, the span start
    /// with any merge base already removed. Used when several files are merged
    /// into one program and each keeps its own source for rendering. The caret
    /// run's width still comes from `span.hi - span.lo`; a shared merge base
    /// cancels out of that difference, so it needs no shifting here.
    pub fn render_local(&self, src: &str, local: u32) -> String {
        let loc = Loc::at(src, local);
        let header = format!("{}:{}: error: {}", loc.line, loc.col, self.msg);
        let run = self.span.hi.saturating_sub(self.span.lo) as usize;
        let hi = clamp_boundary(src, (local as usize).saturating_add(run));
        format!("{header}\n{}", caret_block(src, &loc, hi))
    }
}

/// A byte offset located within its source: 1 based line and Unicode scalar
/// column, plus the byte range of the line it falls on, so a caller can slice
/// the line text and the run before it without rescanning the source.
struct Loc {
    line: u32,
    col: u32,
    off: usize,
    line_start: usize,
    line_end: usize,
}

impl Loc {
    /// Locates a byte offset in `src`. The offset is clamped to a valid
    /// character boundary first (see `clamp_boundary`), so an out of range or
    /// mid codepoint offset, only reachable through a malformed span, never
    /// panics here.
    fn at(src: &str, off: u32) -> Loc {
        let off = clamp_boundary(src, off as usize);
        let bytes = src.as_bytes();
        let mut line = 1u32;
        let mut line_start = 0usize;
        for (i, &b) in bytes[..off].iter().enumerate() {
            if b == b'\n' {
                line += 1;
                line_start = i + 1;
            }
        }
        let line_end = bytes[off..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| off + p)
            .unwrap_or(src.len());
        // The column is a scalar count, not a byte count, so a multibyte
        // character earlier on the line does not overcount it.
        let col = src[line_start..off].chars().count() as u32 + 1;
        Loc {
            line,
            col,
            off,
            line_start,
            line_end,
        }
    }
}

/// Clamps a byte offset to at most `src.len()`, then walks back to the nearest
/// character boundary. Reachable only through a malformed or EOF adjacent
/// span; a well formed one is already a boundary and passes through untouched.
fn clamp_boundary(src: &str, off: usize) -> usize {
    let mut off = off.min(src.len());
    while off > 0 && !src.is_char_boundary(off) {
        off -= 1;
    }
    off
}

/// Converts a byte offset to 1 based line and column, the column a Unicode
/// scalar count rather than a byte count.
fn line_col(src: &str, off: u32) -> (u32, u32) {
    let loc = Loc::at(src, off);
    (loc.line, loc.col)
}

/// Builds the two line block under a diagnostic header: the full source line
/// the span starts on, verbatim, then a caret run beneath it. The pad before
/// the caret mirrors each character the span follows: a tab stays a tab, so
/// the caret still lines up in a terminal that renders tabs wide; everything
/// else becomes a space. The caret run is at least one wide, otherwise as wide
/// as the span's scalar length, and clamps to the line's end, so a span
/// crossing a newline only marks its first line.
fn caret_block(src: &str, loc: &Loc, hi: usize) -> String {
    let line_text = &src[loc.line_start..loc.line_end];
    let pad: String = src[loc.line_start..loc.off]
        .chars()
        .map(|c| if c == '\t' { '\t' } else { ' ' })
        .collect();
    let caret_end = hi.max(loc.off).min(loc.line_end);
    let width = src[loc.off..caret_end].chars().count().max(1);
    let carets = format!("^{}", "~".repeat(width - 1));
    format!("  {line_text}\n  {pad}{carets}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basic() {
        let src = "ab\ncd";
        assert_eq!(line_col(src, 0), (1, 1));
        assert_eq!(line_col(src, 1), (1, 2));
        assert_eq!(line_col(src, 3), (2, 1));
        assert_eq!(line_col(src, 4), (2, 2));
    }

    #[test]
    fn ascii_caret_aligns_under_the_span() {
        let src = "let x = bad\n";
        let idx = src.find("bad").unwrap() as u32;
        let d = Diagnostic::new("oops", Span::new(idx, idx + 3));
        let out = d.render(src);
        assert_eq!(out, "1:9: error: oops\n  let x = bad\n          ^~~");
    }

    #[test]
    fn multibyte_before_span_counts_scalars_not_bytes() {
        // 'e' with an acute accent is two bytes but one scalar. A byte counting
        // column would land one past the true one, on 'a' instead of 'b'.
        let src = "caf\u{e9} := bad\n";
        let idx = src.find("bad").unwrap() as u32;
        let d = Diagnostic::new("oops", Span::new(idx, idx + 3));
        let out = d.render(src);
        assert!(out.starts_with("1:9: error: oops\n"), "{out}");
        assert!(out.ends_with("^~~"), "{out}");
    }

    #[test]
    fn tab_before_span_pads_with_a_tab() {
        let src = "\tx := bad\n";
        let idx = src.find("bad").unwrap() as u32;
        let d = Diagnostic::new("oops", Span::new(idx, idx + 3));
        let out = d.render(src);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out:?}");
        assert!(
            lines[2].starts_with("  \t"),
            "pad must keep the tab: {:?}",
            lines[2]
        );
        assert!(lines[2].ends_with("^~~"), "{:?}", lines[2]);
    }

    #[test]
    fn span_crossing_a_newline_clamps_to_the_first_line() {
        let src = "abc\ndef\n";
        // Covers "c\nd": starts on line 1, ends on line 2.
        let d = Diagnostic::new("oops", Span::new(2, 5));
        let out = d.render(src);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[1], "  abc", "the source line must not include line 2");
        assert_eq!(lines[2], "    ^", "the caret must stop at line 1's end");
    }

    #[test]
    fn span_past_eof_clamps_without_panicking() {
        let src = "abc";
        let d = Diagnostic::new("oops", Span::new(3, 5));
        let out = d.render(src);
        assert_eq!(out, "1:4: error: oops\n  abc\n     ^");
    }

    #[test]
    fn one_char_span_produces_a_single_caret() {
        let src = "x\n";
        let d = Diagnostic::new("oops", Span::new(0, 1));
        let out = d.render(src);
        assert_eq!(out, "1:1: error: oops\n  x\n  ^");
    }
}
