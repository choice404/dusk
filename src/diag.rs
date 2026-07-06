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

    /// Renders against source text as `line:col: error: msg`.
    pub fn render(&self, src: &str) -> String {
        let (line, col) = line_col(src, self.span.lo);
        format!("{line}:{col}: error: {}", self.msg)
    }

    /// Renders as `line:col: error: msg`, taking the position from `local`, the
    /// span start with any merge base already removed. Used when several files
    /// are merged into one program and each keeps its own source for rendering.
    pub fn render_local(&self, src: &str, local: u32) -> String {
        let (line, col) = line_col(src, local);
        format!("{line}:{col}: error: {}", self.msg)
    }
}

/// Converts a byte offset to 1 based line and column.
fn line_col(src: &str, off: u32) -> (u32, u32) {
    let off = off as usize;
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, b) in src.bytes().enumerate() {
        if i >= off {
            break;
        }
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
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
}
