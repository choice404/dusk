//! Token kinds for the full grammar.

use crate::diag::Span;

/// A lexed token: a kind, its source span, and whether a newline preceded it.
/// `nl_before` lets the parser treat newlines as statement separators.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub nl_before: bool,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Token {
            kind,
            span,
            nl_before: false,
        }
    }
}

/// Reserved words. Type names like `int32`, `bool`, and `string` are ordinary
/// identifiers resolved later, not keywords.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    Func,
    Lambda,
    Struct,
    Enum,
    Interface,
    Impl,
    Foreign,
    Monad,
    Export,
    Mut,
    Using,
    Defer,
    Match,
    Return,
    If,
    Else,
    For,
    While,
    Do,
    In,
    Break,
    Continue,
}

impl Keyword {
    /// Maps an identifier to a keyword, or None if it is an ordinary identifier.
    pub fn lookup(s: &str) -> Option<Keyword> {
        let kw = match s {
            "func" => Keyword::Func,
            "lambda" => Keyword::Lambda,
            "struct" => Keyword::Struct,
            "enum" => Keyword::Enum,
            "interface" => Keyword::Interface,
            "impl" => Keyword::Impl,
            "foreign" => Keyword::Foreign,
            "monad" => Keyword::Monad,
            "export" => Keyword::Export,
            "mut" => Keyword::Mut,
            "using" => Keyword::Using,
            "defer" => Keyword::Defer,
            "match" => Keyword::Match,
            "return" => Keyword::Return,
            "if" => Keyword::If,
            "else" => Keyword::Else,
            "for" => Keyword::For,
            "while" => Keyword::While,
            "do" => Keyword::Do,
            "in" => Keyword::In,
            "break" => Keyword::Break,
            "continue" => Keyword::Continue,
            _ => return None,
        };
        Some(kw)
    }
}

/// The lexical categories the lexer emits.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Int { val: i64, suffix: Option<String> },
    Float { val: f64, suffix: Option<String> },
    Str(String),
    Char(char),
    Rune(char),
    Bool(bool),
    Kw(Keyword),

    ColonEq,
    Arrow,
    FatArrow,
    LArrow,
    DotDot,
    DotDotEq,
    EqEq,
    Ne,
    Le,
    Ge,
    AndAnd,
    OrOr,

    // Bitwise and shift operators.
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    // Exponent and pipe.
    StarStar,
    PipeGt,
    // Increment and decrement.
    PlusPlus,
    MinusMinus,
    // Compound assignment, one per binary operator.
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,

    Assign,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Lt,
    Gt,
    Bang,

    Colon,
    Semi,
    Comma,
    Dot,
    At,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,

    Eof,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_maps_keywords_only() {
        assert_eq!(Keyword::lookup("func"), Some(Keyword::Func));
        assert_eq!(Keyword::lookup("match"), Some(Keyword::Match));
        assert_eq!(Keyword::lookup("int32"), None);
        assert_eq!(Keyword::lookup("main"), None);
    }
}
