//! Semantic token analysis at the lexical level.
//!
//! This module walks the token stream produced by [`crate::lexer`] and assigns
//! each token a semantic token type (and optional modifier) from the LSP legend.
//! It uses lightweight, purely lexical context (surrounding keywords and
//! punctuation) to distinguish identifiers that read as function calls, object
//! names, columns and generic aliases. It does not attempt name resolution, but
//! it is stable across re-parse and cost-free to compute.

use lsp_types::SemanticTokenModifier;
use lsp_types::SemanticTokenType;

use crate::lexer::{Token, TokenKind, lex};

/// A token annotated with its semantic type index and modifier bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticTokenInfo {
    pub start: u32,
    pub end: u32,
    pub token_type: u32,
    pub modifiers: u32,
}

/// The fixed legend `scopeql-lsp` uses for semantic tokens.
///
/// Every type listed here is one the coc.nvim LSP client declares as supported,
/// so an editor can map it to a highlight group.
pub const LEGEND_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,
    SemanticTokenType::TYPE,
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::CLASS,
    SemanticTokenType::STRUCT,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::COMMENT,
];

/// Modifiers used by `scopeql-lsp`.
pub const LEGEND_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,
    SemanticTokenModifier::READONLY,
];

const KEYWORD_TYPE: u32 = 0;
const TYPE_TYPE: u32 = 1;
const PROPERTY_TYPE: u32 = 5;
const VARIABLE_TYPE: u32 = 6;
const FUNCTION_TYPE: u32 = 7;
const STRING_TYPE: u32 = 8;
const NUMBER_TYPE: u32 = 9;
const OPERATOR_TYPE: u32 = 10;
const COMMENT_TYPE: u32 = 11;

const DECLARATION_MODIFIER: u32 = 1 << 0;

/// Compute semantic tokens for a ScopeQL document.
pub fn semantic_tokens(source: &str) -> Vec<SemanticTokenInfo> {
    let (tokens, _) = lex(source);
    let mut out = Vec::with_capacity(tokens.len());

    for (i, tok) in tokens.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| &tokens[p]);
        let next = tokens.get(i + 1);
        out.push(classify(tok, prev, next, source));
    }

    out
}

fn classify(
    tok: &Token,
    prev: Option<&Token>,
    next: Option<&Token>,
    source: &str,
) -> SemanticTokenInfo {
    let text = &source[tok.span.start as usize..tok.span.end as usize];

    let (token_type, modifiers) = match tok.kind {
        TokenKind::Comment => (COMMENT_TYPE, 0),
        TokenKind::String => (STRING_TYPE, 0),
        TokenKind::Number => (NUMBER_TYPE, 0),
        TokenKind::Operator => (OPERATOR_TYPE, 0),
        // Type names are a keyword family already tagged by the lexer.
        TokenKind::Type => (TYPE_TYPE, 0),
        TokenKind::Keyword => keyword_type(text),
        TokenKind::Ident => ident_type(prev, next, source),
    };

    SemanticTokenInfo {
        start: tok.span.start,
        end: tok.span.end,
        token_type,
        modifiers,
    }
}

/// Decide the semantic type and modifier for a keyword token.
fn keyword_type(text: &str) -> (u32, u32) {
    let lower = text.to_ascii_lowercase();
    let is_declaration = matches!(
        lower.as_str(),
        "create" | "drop" | "alter" | "rename" | "replace"
    );
    (KEYWORD_TYPE, if is_declaration { DECLARATION_MODIFIER } else { 0 })
}

/// Decide the semantic type for an identifier using its neighbors.
fn ident_type(prev: Option<&Token>, next: Option<&Token>, source: &str) -> (u32, u32) {
    use TokenKind as K;

    // Keywords that introduce an object name: the identifier that follows them
    // names the object (table, view, ...), even when a parenthesised column
    // list follows (e.g. `create table events (...)`).
    if let Some(prev) = prev
        && prev.kind == K::Keyword
        && is_object_kind(&prev_text(prev, source))
    {
        return (TYPE_TYPE, 0);
    }

    // Function call: an identifier directly followed by `(`.
    if let Some(next) = next
        && next.kind == K::Operator
        && punct_char(next, source) == Some('(')
    {
        return (FUNCTION_TYPE, 0);
    }

    if let Some(prev) = prev {
        // A member after `.` reads as a column reference: `events.name`.
        if prev.kind == K::Operator && punct_char(prev, source) == Some('.') {
            return (PROPERTY_TYPE, 0);
        }
        match prev.kind {
            K::Keyword => {
                let word = prev_text(prev, source);
                // `AS` introduces an alias, which reads as a variable.
                if word == "as" {
                    return (VARIABLE_TYPE, 0);
                }
                // Query-clause keywords introduce column references.
                if is_clause_kind(&word) {
                    return (PROPERTY_TYPE, 0);
                }
                (TYPE_TYPE, 0)
            }
            // Inside a parenthesised list (arguments, column lists, tuples)
            // an identifier reads as a column reference.
            K::Operator => {
                let c = punct_char(prev, source).unwrap_or(' ');
                if matches!(c, '(' | ',' | '=' | '[') {
                    (PROPERTY_TYPE, 0)
                } else {
                    (VARIABLE_TYPE, 0)
                }
            }
            K::Type => (TYPE_TYPE, 0),
            _ => (VARIABLE_TYPE, 0),
        }
    } else {
        (VARIABLE_TYPE, 0)
    }
}

/// Whether a keyword names an object category (table, view, schema, ...).
fn is_object_kind(word: &str) -> bool {
    matches!(
        word,
        "table"
            | "view"
            | "schema"
            | "database"
            | "index"
            | "job"
            | "nodegroup"
            | "key"
            | "partition"
            | "cluster"
            | "column"
            | "from"
            | "join"
            | "into"
            | "update"
            | "delete"
            | "describe"
            | "optimize"
            | "vacuum"
    )
}

/// Whether a keyword introduces a column-reference position in a query.
fn is_clause_kind(word: &str) -> bool {
    matches!(
        word,
        "select"
            | "where"
            | "on"
            | "by"
            | "group"
            | "order"
            | "having"
            | "limit"
            | "offset"
            | "set"
            | "window"
            | "aggregate"
            | "distinct"
            | "values"
            | "between"
            | "when"
            | "then"
            | "case"
    )
}

fn prev_text(tok: &Token, source: &str) -> String {
    source[tok.span.start as usize..tok.span.end as usize]
        .to_ascii_lowercase()
}

/// Return the punctuation character an operator token spans, if any.
fn punct_char(tok: &Token, source: &str) -> Option<char> {
    if tok.kind != TokenKind::Operator {
        return None;
    }
    let text = &source[tok.span.start as usize..tok.span.end as usize];
    let mut chars = text.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_types(source: &str) -> Vec<(u32, u32)> {
        semantic_tokens(source)
            .into_iter()
            .map(|t| (t.token_type, t.modifiers))
            .collect()
    }

    #[test]
    fn highlights_keywords_numbers_and_comments() {
        let out = token_types("select 1; -- hi");
        assert_eq!(out[0].0, KEYWORD_TYPE);
        assert_eq!(out[1].0, NUMBER_TYPE);
        assert_eq!(out[3].0, COMMENT_TYPE);
    }

    #[test]
    fn classifies_function_calls() {
        // `lower(` is a function call; the `name` argument reads as a column.
        let out = token_types("select lower(name);");
        assert_eq!(out[1].0, FUNCTION_TYPE);
        assert_eq!(out[3].0, PROPERTY_TYPE);
    }

    #[test]
    fn classifies_object_names() {
        // The name after `create table` reads as an object (type) name; the
        // column in the parens reads as a property; the datatype as a type.
        let out = token_types("create table events (id int);");
        assert_eq!(out[2].0, TYPE_TYPE);
        assert_eq!(out[4].0, PROPERTY_TYPE);
        assert_eq!(out[5].0, TYPE_TYPE);
    }
}
