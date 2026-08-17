//! A self-contained ScopeQL lexer.
//!
//! This module re-implements the tokenizer used by ScopeDB's `ast` crate so that
//! `scopeql-lsp` stays independent of the rest of the repository and can be
//! hosted in its own project. It intentionally mirrors the keyword set and
//! token shapes of `crates/ast/src/parser/token.rs` but keeps no dependency on
//! it. It is the single source of truth for lexical (and, via
//! [`crate::highlight`], semantic) highlighting.

use std::fmt;

use logos::Logos;

/// A byte range into the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self {
            start: start as u32,
            end: end as u32,
        }
    }
}

/// A classification of a token that is useful for editors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    // Lexical leaves.
    Ident,
    String,
    Number,
    Operator,
    Comment,
    // Keyword families.
    Keyword,
    /// Type names such as `int`, `string`, `timestamp`.
    Type,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Ident => "identifier",
            Self::String => "string",
            Self::Number => "number",
            Self::Operator => "operator",
            Self::Comment => "comment",
            Self::Keyword => "keyword",
            Self::Type => "type",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.span.start as usize..self.span.end as usize]
    }
}

// Uppercase keyword variants keep the lexer aligned with ScopeQL spelling.
#[allow(clippy::upper_case_acronyms)]
#[derive(Logos, Clone, Copy, Debug, PartialEq, Eq)]
#[logos(skip r"[ \t\r\n\f]+")]
enum RawToken {
    // Comments.
    #[regex(r"--[^\r\n\f]*")]
    LineComment,
    #[regex(r"/\*([^\*]|(\*[^/]))*\*/")]
    BlockComment,

    // Identifiers and literals.
    #[regex(r"[_a-zA-Z][_a-zA-Z0-9]*")]
    Ident,
    #[regex(r#"'([^'\\]|\\.|'')*'"#)]
    #[regex(r#""([^"\\]|\\.|"")*""#)]
    #[regex(r"`([^`\\]|\\.|``)*`")]
    LiteralString,
    #[regex(r"[0-9]+(_|[0-9])*")]
    LiteralInteger,
    #[regex(r"0[xX][a-fA-F0-9]+")]
    LiteralHexInteger,
    #[regex(r"[0-9]+[eE][+-]?[0-9]+")]
    #[regex(r"[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?")]
    LiteralFloat,

    // Symbols.
    #[token("<>")]
    NotEq,
    #[token("!=")]
    NotEqBang,
    #[token("<=")]
    Lte,
    #[token(">=")]
    Gte,
    #[token("||")]
    Concat,
    #[token("::")]
    DoubleColon,
    #[token("=>")]
    Arrow,
    #[token("=")]
    Eq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Multiply,
    #[token("/")]
    Divide,
    #[token("%")]
    Modulo,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token(":")]
    Colon,
    #[token(";")]
    SemiColon,
    #[token("$")]
    Dollar,

    // Case-insensitive keywords.
    #[token("ADD", ignore(case))]
    Add,
    #[token("AGGREGATE", ignore(case))]
    Aggregate,
    #[token("ALL", ignore(case))]
    All,
    #[token("ALTER", ignore(case))]
    Alter,
    #[token("ANALYZE", ignore(case))]
    Analyze,
    #[token("AND", ignore(case))]
    And,
    #[token("ANY", ignore(case))]
    Any,
    #[token("ARRAY", ignore(case))]
    Array,
    #[token("AS", ignore(case))]
    As,
    #[token("ASC", ignore(case))]
    Asc,
    #[token("BEGIN", ignore(case))]
    Begin,
    #[token("BETWEEN", ignore(case))]
    Between,
    #[token("BINARY", ignore(case))]
    Binary,
    #[token("BOOLEAN", ignore(case))]
    Boolean,
    #[token("BY", ignore(case))]
    By,
    #[token("CASE", ignore(case))]
    Case,
    #[token("CAST", ignore(case))]
    Cast,
    #[token("CLUSTER", ignore(case))]
    Cluster,
    #[token("COLUMN", ignore(case))]
    Column,
    #[token("COMMENT", ignore(case))]
    Comment,
    #[token("CREATE", ignore(case))]
    Create,
    #[token("DATA", ignore(case))]
    Data,
    #[token("DATABASES", ignore(case))]
    Databases,
    #[token("DATABASE", ignore(case))]
    Database,
    #[token("DAY", ignore(case))]
    Day,
    #[token("DELETE", ignore(case))]
    Delete,
    #[token("DESC", ignore(case))]
    Desc,
    #[token("DESCRIBE", ignore(case))]
    Describe,
    #[token("DISTINCT", ignore(case))]
    Distinct,
    #[token("DROP", ignore(case))]
    Drop,
    #[token("ELSE", ignore(case))]
    Else,
    #[token("END", ignore(case))]
    End,
    #[token("EXCLUDE", ignore(case))]
    Exclude,
    #[token("EXEC", ignore(case))]
    Exec,
    #[token("EXISTS", ignore(case))]
    Exists,
    #[token("EXPLAIN", ignore(case))]
    Explain,
    #[token("FALSE", ignore(case))]
    False,
    #[token("FIRST", ignore(case))]
    First,
    #[token("FLOAT", ignore(case))]
    Float,
    #[token("FROM", ignore(case))]
    From,
    #[token("FULL", ignore(case))]
    Full,
    #[token("GROUP", ignore(case))]
    Group,
    #[token("IF", ignore(case))]
    If,
    #[token("IN", ignore(case))]
    In,
    #[token("INDEX", ignore(case))]
    Index,
    #[token("INNER", ignore(case))]
    Inner,
    #[token("INSERT", ignore(case))]
    Insert,
    #[token("INT", ignore(case))]
    Int,
    #[token("INTERVAL", ignore(case))]
    Interval,
    #[token("INTO", ignore(case))]
    Into,
    #[token("IS", ignore(case))]
    Is,
    #[token("JOB", ignore(case))]
    Job,
    #[token("JOBS", ignore(case))]
    Jobs,
    #[token("JOIN", ignore(case))]
    Join,
    #[token("KEY", ignore(case))]
    Key,
    #[token("LAST", ignore(case))]
    Last,
    #[token("LEFT", ignore(case))]
    Left,
    #[token("LIMIT", ignore(case))]
    Limit,
    #[token("MATERIALIZED", ignore(case))]
    Materialized,
    #[token("NODEGROUP", ignore(case))]
    Nodegroup,
    #[token("NOT", ignore(case))]
    Not,
    #[token("NULL", ignore(case))]
    Null,
    #[token("NULLS", ignore(case))]
    Nulls,
    #[token("OBJECT", ignore(case))]
    Object,
    #[token("OFFSET", ignore(case))]
    Offset,
    #[token("ON", ignore(case))]
    On,
    #[token("OPTIMIZE", ignore(case))]
    Optimize,
    #[token("OR", ignore(case))]
    Or,
    #[token("ORDER", ignore(case))]
    Order,
    #[token("OUTER", ignore(case))]
    Outer,
    #[token("PARTITION", ignore(case))]
    Partition,
    #[token("PATTERN", ignore(case))]
    Pattern,
    #[token("PERCENT", ignore(case))]
    Percent,
    #[token("PLAN", ignore(case))]
    Plan,
    #[token("POINT", ignore(case))]
    Point,
    #[token("RANGE", ignore(case))]
    Range,
    #[token("RENAME", ignore(case))]
    Rename,
    #[token("REPLACE", ignore(case))]
    Replace,
    #[token("RESUME", ignore(case))]
    Resume,
    #[token("RETENTION", ignore(case))]
    Retention,
    #[token("RIGHT", ignore(case))]
    Right,
    #[token("SAMPLE", ignore(case))]
    Sample,
    #[token("SCHEDULE", ignore(case))]
    Schedule,
    #[token("SCHEMAS", ignore(case))]
    Schemas,
    #[token("SCHEMA", ignore(case))]
    Schema,
    #[token("SEARCH", ignore(case))]
    Search,
    #[token("SELECT", ignore(case))]
    Select,
    #[token("SET", ignore(case))]
    Set,
    #[token("SHOW", ignore(case))]
    Show,
    #[token("STATEMENTS", ignore(case))]
    Statements,
    #[token("STRING", ignore(case))]
    String,
    #[token("SUSPEND", ignore(case))]
    Suspend,
    #[token("TABLE", ignore(case))]
    Table,
    #[token("TABLES", ignore(case))]
    Tables,
    #[token("THEN", ignore(case))]
    Then,
    #[token("TIMESTAMP", ignore(case))]
    Timestamp,
    #[token("TO", ignore(case))]
    To,
    #[token("TRUE", ignore(case))]
    True,
    #[token("UINT", ignore(case))]
    Uint,
    #[token("UNION", ignore(case))]
    Union,
    #[token("UPDATE", ignore(case))]
    Update,
    #[token("VACUUM", ignore(case))]
    Vacuum,
    #[token("VALUES", ignore(case))]
    Values,
    #[token("VIEW", ignore(case))]
    View,
    #[token("VIEWS", ignore(case))]
    Views,
    #[token("WHEN", ignore(case))]
    When,
    #[token("WHERE", ignore(case))]
    Where,
    #[token("WINDOW", ignore(case))]
    Window,
    #[token("WITH", ignore(case))]
    With,
    #[token("WITHIN", ignore(case))]
    Within,
    #[token("XOR", ignore(case))]
    Xor,
}

impl RawToken {
    /// Whether this raw token reads as an operator or punctuation symbol.
    fn is_symbol(self) -> bool {
        use RawToken::{
            Arrow, Colon, Comma, Concat, Divide, Dollar, Dot, DoubleColon, Eq, Gt, Gte, LBrace,
            LBracket, LParen, Lt, Lte, Minus, Modulo, Multiply, NotEq, NotEqBang, Plus, RBrace,
            RBracket, RParen, SemiColon,
        };
        matches!(
            self,
            Eq | NotEq
                | NotEqBang
                | Lt
                | Gt
                | Lte
                | Gte
                | Plus
                | Minus
                | Multiply
                | Divide
                | Modulo
                | Concat
                | LParen
                | RParen
                | LBracket
                | RBracket
                | LBrace
                | RBrace
                | Comma
                | Dot
                | Colon
                | DoubleColon
                | SemiColon
                | Dollar
                | Arrow
        )
    }

    /// Whether this raw token names a ScopeQL type (`int`, `string`, ...).
    fn is_type_name(self) -> bool {
        use RawToken::{Any, Array, Binary, Float, Int, Interval, Object, String, Uint, Boolean, Timestamp};
        matches!(
            self,
            Binary | Int | Uint | Float | String | Boolean | Timestamp | Interval | Object | Array | Any
        )
    }
}

/// Tokenize a ScopeQL source string.
///
/// Returns the lexed tokens plus any lexical errors `(span, message)`.
pub fn lex(source: &str) -> (Vec<Token>, Vec<(Span, String)>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    let mut lexer = RawToken::lexer(source);
    while let Some(result) = lexer.next() {
        let span = Span::new(lexer.span().start, lexer.span().end);
        match result {
            Ok(kind) => tokens.push(Token {
                kind: classify(kind),
                span,
            }),
            Err(()) => {
                errors.push((
                    span,
                    "failed to recognize the remaining input".to_string(),
                ));
            }
        }
    }

    (tokens, errors)
}

fn classify(kind: RawToken) -> TokenKind {
    use RawToken::*;

    match kind {
        Ident => TokenKind::Ident,
        LiteralString => TokenKind::String,
        LiteralInteger | LiteralHexInteger | LiteralFloat => TokenKind::Number,
        LineComment | BlockComment => TokenKind::Comment,
        _ if kind.is_symbol() => TokenKind::Operator,
        _ if kind.is_type_name() => TokenKind::Type,
        _ => TokenKind::Keyword,
    }
}
