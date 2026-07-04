//! Full grammar lexer. Paradigm agnostic: it never suppresses tokens.

pub mod token;

use crate::diag::{Diagnostic, Span};
use token::{Keyword, Token, TokenKind};

/// Tokenizes `src` into a stream ending in `Eof`. Returns any errors collected
/// along the way; tokens are still produced past errors for recovery.
pub fn lex(src: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    Lexer::new(src).run()
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<Diagnostic>,
    pending_nl: bool,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer {
            src: src.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
            pending_nl: false,
        }
    }

    fn run(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(c) = self.peek() else {
                self.push(TokenKind::Eof, start);
                break;
            };
            match c {
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.ident(start),
                b'0'..=b'9' => self.number(start),
                b'"' => self.string(start),
                b'\'' => self.char_lit(start),
                _ => self.symbol(start, c),
            }
        }
        (self.tokens, self.errors)
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.src.get(self.pos + 1).copied()
    }

    fn peek3(&self) -> Option<u8> {
        self.src.get(self.pos + 2).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    /// Consumes a full UTF-8 scalar starting at the current non-ASCII byte.
    fn bump_scalar(&mut self) -> char {
        let begin = self.pos;
        let lead = self.src.get(begin).copied().unwrap_or(0);
        let end = (begin + utf8_len(lead)).min(self.src.len());
        self.pos = end;
        std::str::from_utf8(&self.src[begin..end])
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or('\u{FFFD}')
    }

    fn text(&self, start: usize) -> String {
        String::from_utf8_lossy(&self.src[start..self.pos]).into_owned()
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        let span = Span::new(start as u32, self.pos as u32);
        self.tokens.push(Token {
            kind,
            span,
            nl_before: self.pending_nl,
        });
        self.pending_nl = false;
    }

    fn error(&mut self, msg: impl Into<String>, start: usize) {
        let span = Span::new(start as u32, self.pos as u32);
        self.errors.push(Diagnostic::new(msg, span));
    }

    fn error_at(&mut self, msg: impl Into<String>, lo: usize) {
        let span = Span::new(lo as u32, self.pos as u32);
        self.errors.push(Diagnostic::new(msg, span));
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b'\n') => {
                    self.pending_nl = true;
                    self.pos += 1;
                }
                Some(b' ' | b'\t' | b'\r') => {
                    self.pos += 1;
                }
                Some(b'/') if self.peek2() == Some(b'/') => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn ident(&mut self, start: usize) {
        while matches!(self.peek(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')) {
            self.pos += 1;
        }
        let text = self.text(start);
        let kind = match text.as_str() {
            "true" => TokenKind::Bool(true),
            "false" => TokenKind::Bool(false),
            _ => match Keyword::lookup(&text) {
                Some(kw) => TokenKind::Kw(kw),
                None => TokenKind::Ident(text),
            },
        };
        self.push(kind, start);
    }

    fn number(&mut self, start: usize) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') && matches!(self.peek2(), Some(b'0'..=b'9')) {
            is_float = true;
            self.pos += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let body = self.text(start);
        let suffix = self.suffix();
        if let Some(s) = &suffix {
            if !valid_suffix(s) {
                self.error(format!("unknown literal suffix '{s}'"), start);
            }
        }
        let kind = if is_float {
            match body.parse::<f64>() {
                Ok(val) => TokenKind::Float { val, suffix },
                Err(_) => {
                    self.error(format!("invalid float literal '{body}'"), start);
                    TokenKind::Float { val: 0.0, suffix }
                }
            }
        } else {
            match body.parse::<i64>() {
                Ok(val) => TokenKind::Int { val, suffix },
                Err(_) => {
                    self.error(format!("invalid integer literal '{body}'"), start);
                    TokenKind::Int { val: 0, suffix }
                }
            }
        };
        self.push(kind, start);
    }

    fn suffix(&mut self) -> Option<String> {
        if !matches!(self.peek(), Some(b'a'..=b'z' | b'A'..=b'Z')) {
            return None;
        }
        let start = self.pos;
        while matches!(self.peek(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9')) {
            self.pos += 1;
        }
        Some(self.text(start))
    }

    fn string(&mut self, start: usize) {
        self.pos += 1;
        let mut buf: Vec<u8> = Vec::new();
        loop {
            match self.bump() {
                None => {
                    self.error("unterminated string literal", start);
                    break;
                }
                Some(b'"') => break,
                Some(b'\\') => {
                    if let Some(e) = self.escape() {
                        let mut tmp = [0u8; 4];
                        buf.extend_from_slice(e.encode_utf8(&mut tmp).as_bytes());
                    }
                }
                Some(c) => buf.push(c),
            }
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        self.push(TokenKind::Str(text), start);
    }

    fn char_lit(&mut self, start: usize) {
        self.pos += 1;
        let ch = match self.peek() {
            None => {
                self.error("unterminated char literal", start);
                '\0'
            }
            Some(b'\'') => {
                self.error("empty char literal", start);
                '\0'
            }
            Some(b'\\') => {
                self.pos += 1;
                self.escape().unwrap_or('\0')
            }
            Some(c) if c < 0x80 => {
                self.pos += 1;
                c as char
            }
            Some(_) => {
                let ch = self.bump_scalar();
                self.error("non-ASCII char literal not supported", start);
                ch
            }
        };
        if self.peek() == Some(b'\'') {
            self.pos += 1;
        } else {
            while !matches!(self.peek(), Some(b'\'') | Some(b'\n') | None) {
                self.pos += 1;
            }
            if self.peek() == Some(b'\'') {
                self.pos += 1;
            }
            self.error("char literal must contain exactly one character", start);
        }
        self.push(TokenKind::Char(ch), start);
    }

    fn escape(&mut self) -> Option<char> {
        let esc = self.pos.saturating_sub(1);
        match self.bump() {
            Some(b'n') => Some('\n'),
            Some(b't') => Some('\t'),
            Some(b'r') => Some('\r'),
            Some(b'0') => Some('\0'),
            Some(b'\\') => Some('\\'),
            Some(b'"') => Some('"'),
            Some(b'\'') => Some('\''),
            other => {
                let what = other.map(|b| b as char).unwrap_or('?');
                self.error_at(format!("unknown escape '\\{what}'"), esc);
                None
            }
        }
    }

    /// Munches an operator or punctuation token. Each lead char consumes the
    /// longest matching form: three char forms (`..=`, `<<=`, `>>=`) first, then
    /// two char, then a single char falls to `one`. The greedy `>>` join is split
    /// back into two `>` by the parser at nested generic closes, and `**` is split
    /// at unary position, so `Wrap<Box<int64>>` and `**pp` still parse.
    fn symbol(&mut self, start: usize, c: u8) {
        if c >= 0x80 {
            let ch = self.bump_scalar();
            self.error(format!("unexpected character '{ch}'"), start);
            return;
        }
        let n = self.peek2();
        let n3 = self.peek3();
        let kind = match c {
            b':' if n == Some(b'=') => self.two(TokenKind::ColonEq),
            b'=' if n == Some(b'>') => self.two(TokenKind::FatArrow),
            b'=' if n == Some(b'=') => self.two(TokenKind::EqEq),
            b'!' if n == Some(b'=') => self.two(TokenKind::Ne),

            b'.' if n == Some(b'.') && n3 == Some(b'=') => self.three(TokenKind::DotDotEq),
            b'.' if n == Some(b'.') => self.two(TokenKind::DotDot),

            b'<' if n == Some(b'<') && n3 == Some(b'=') => self.three(TokenKind::ShlEq),
            b'<' if n == Some(b'<') => self.two(TokenKind::Shl),
            b'<' if n == Some(b'=') => self.two(TokenKind::Le),
            b'<' if n == Some(b'-') => self.two(TokenKind::LArrow),

            b'>' if n == Some(b'>') && n3 == Some(b'=') => self.three(TokenKind::ShrEq),
            b'>' if n == Some(b'>') => self.two(TokenKind::Shr),
            b'>' if n == Some(b'=') => self.two(TokenKind::Ge),

            b'&' if n == Some(b'&') => self.two(TokenKind::AndAnd),
            b'&' if n == Some(b'=') => self.two(TokenKind::AmpEq),

            b'|' if n == Some(b'|') => self.two(TokenKind::OrOr),
            b'|' if n == Some(b'>') => self.two(TokenKind::PipeGt),
            b'|' if n == Some(b'=') => self.two(TokenKind::PipeEq),

            b'^' if n == Some(b'=') => self.two(TokenKind::CaretEq),

            b'+' if n == Some(b'+') => self.two(TokenKind::PlusPlus),
            b'+' if n == Some(b'=') => self.two(TokenKind::PlusEq),

            b'-' if n == Some(b'>') => self.two(TokenKind::Arrow),
            b'-' if n == Some(b'-') => self.two(TokenKind::MinusMinus),
            b'-' if n == Some(b'=') => self.two(TokenKind::MinusEq),

            b'*' if n == Some(b'*') => self.two(TokenKind::StarStar),
            b'*' if n == Some(b'=') => self.two(TokenKind::StarEq),

            b'/' if n == Some(b'=') => self.two(TokenKind::SlashEq),
            b'%' if n == Some(b'=') => self.two(TokenKind::PercentEq),

            _ => match self.one(c) {
                Some(kind) => kind,
                None => {
                    self.pos += 1;
                    self.error(format!("unexpected character '{}'", c as char), start);
                    return;
                }
            },
        };
        self.push(kind, start);
    }

    fn two(&mut self, kind: TokenKind) -> TokenKind {
        self.pos += 2;
        kind
    }

    fn three(&mut self, kind: TokenKind) -> TokenKind {
        self.pos += 3;
        kind
    }

    fn one(&mut self, c: u8) -> Option<TokenKind> {
        let kind = match c {
            b'=' => TokenKind::Assign,
            b'+' => TokenKind::Plus,
            b'-' => TokenKind::Minus,
            b'*' => TokenKind::Star,
            b'/' => TokenKind::Slash,
            b'%' => TokenKind::Percent,
            b'<' => TokenKind::Lt,
            b'>' => TokenKind::Gt,
            b'!' => TokenKind::Bang,
            b'&' => TokenKind::Amp,
            b'|' => TokenKind::Pipe,
            b'^' => TokenKind::Caret,
            b'~' => TokenKind::Tilde,
            b':' => TokenKind::Colon,
            b';' => TokenKind::Semi,
            b',' => TokenKind::Comma,
            b'.' => TokenKind::Dot,
            b'@' => TokenKind::At,
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            _ => return None,
        };
        self.pos += 1;
        Some(kind)
    }
}

/// Byte length of a UTF-8 sequence given its lead byte.
fn utf8_len(lead: u8) -> usize {
    match lead {
        0xF0..=0xF7 => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

/// True for the integer and float type suffixes the spec allows.
fn valid_suffix(s: &str) -> bool {
    matches!(
        s,
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use token::Keyword;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (toks, errs) = lex(src);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        toks.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn keywords_idents_eof() {
        assert_eq!(
            kinds("func main mut x"),
            vec![
                TokenKind::Kw(Keyword::Func),
                TokenKind::Ident("main".into()),
                TokenKind::Kw(Keyword::Mut),
                TokenKind::Ident("x".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn type_names_and_self_are_idents() {
        assert_eq!(
            kinds("self int32 string error"),
            vec![
                TokenKind::Ident("self".into()),
                TokenKind::Ident("int32".into()),
                TokenKind::Ident("string".into()),
                TokenKind::Ident("error".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn multi_char_operators() {
        assert_eq!(
            kinds(":= -> => <- .. == != <= >= && ||"),
            vec![
                TokenKind::ColonEq,
                TokenKind::Arrow,
                TokenKind::FatArrow,
                TokenKind::LArrow,
                TokenKind::DotDot,
                TokenKind::EqEq,
                TokenKind::Ne,
                TokenKind::Le,
                TokenKind::Ge,
                TokenKind::AndAnd,
                TokenKind::OrOr,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn bitwise_shift_and_compound_operators() {
        assert_eq!(
            kinds("& | ^ ~ << >> ** |> ..= ++ --"),
            vec![
                TokenKind::Amp,
                TokenKind::Pipe,
                TokenKind::Caret,
                TokenKind::Tilde,
                TokenKind::Shl,
                TokenKind::Shr,
                TokenKind::StarStar,
                TokenKind::PipeGt,
                TokenKind::DotDotEq,
                TokenKind::PlusPlus,
                TokenKind::MinusMinus,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn compound_assign_operators() {
        assert_eq!(
            kinds("+= -= *= /= %= &= |= ^= <<= >>="),
            vec![
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::StarEq,
                TokenKind::SlashEq,
                TokenKind::PercentEq,
                TokenKind::AmpEq,
                TokenKind::PipeEq,
                TokenKind::CaretEq,
                TokenKind::ShlEq,
                TokenKind::ShrEq,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn inclusive_range_beats_exclusive_and_dot() {
        // `..=` is longer than `..`, and a bare `.` after two dots stays a dot,
        // so `...` lexes as `..` then `.` (spread is excluded, not a token).
        assert_eq!(
            kinds("1..=3 0..2 a...b"),
            vec![
                TokenKind::Int { val: 1, suffix: None },
                TokenKind::DotDotEq,
                TokenKind::Int { val: 3, suffix: None },
                TokenKind::Int { val: 0, suffix: None },
                TokenKind::DotDot,
                TokenKind::Int { val: 2, suffix: None },
                TokenKind::Ident("a".into()),
                TokenKind::DotDot,
                TokenKind::Dot,
                TokenKind::Ident("b".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn double_minus_lexes_as_decrement() {
        // `x--y` now lexes `--` between the identifiers, a documented change from
        // the old `x - (-y)` reading. Spaces are required to subtract a negation.
        assert_eq!(
            kinds("x--y"),
            vec![
                TokenKind::Ident("x".into()),
                TokenKind::MinusMinus,
                TokenKind::Ident("y".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn shift_and_greater_disambiguate_greedily() {
        // The lexer joins `>>` greedily; the parser splits it back at a type
        // close. In expression position `a >> b` is a single shift token.
        assert_eq!(
            kinds("a >> b >= c"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::Shr,
                TokenKind::Ident("b".into()),
                TokenKind::Ge,
                TokenKind::Ident("c".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn int_suffix_and_range() {
        assert_eq!(
            kinds("5u8 1..3"),
            vec![
                TokenKind::Int { val: 5, suffix: Some("u8".into()) },
                TokenKind::Int { val: 1, suffix: None },
                TokenKind::DotDot,
                TokenKind::Int { val: 3, suffix: None },
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn float_with_suffix() {
        assert_eq!(
            kinds("2.5f32"),
            vec![
                TokenKind::Float { val: 2.5, suffix: Some("f32".into()) },
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unknown_suffix_errors() {
        for src in ["0xFF", "5x", "1e10"] {
            let (_toks, errs) = lex(src);
            assert!(!errs.is_empty(), "{src} should error");
        }
    }

    #[test]
    fn member_access_is_dot_not_float() {
        assert_eq!(
            kinds("e.exists"),
            vec![
                TokenKind::Ident("e".into()),
                TokenKind::Dot,
                TokenKind::Ident("exists".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn string_and_escapes() {
        assert_eq!(
            kinds(r#""a\nb""#),
            vec![TokenKind::Str("a\nb".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn non_ascii_string_preserved() {
        assert_eq!(
            kinds("\"café\""),
            vec![TokenKind::Str("café".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn non_ascii_char_reports_one_error() {
        let (_toks, errs) = lex("'é'");
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn directive_and_comment() {
        assert_eq!(
            kinds("@paradigm functional // note\n@import std.io"),
            vec![
                TokenKind::At,
                TokenKind::Ident("paradigm".into()),
                TokenKind::Ident("functional".into()),
                TokenKind::At,
                TokenKind::Ident("import".into()),
                TokenKind::Ident("std".into()),
                TokenKind::Dot,
                TokenKind::Ident("io".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unexpected_char_reports_error_and_continues() {
        let (toks, errs) = lex("a ` b");
        assert_eq!(errs.len(), 1);
        let idents = toks
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Ident(_)))
            .count();
        assert_eq!(idents, 2);
    }
}
