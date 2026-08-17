//! Name extraction and matching for workspace-wide navigation.
//!
//! This module identifies the names the language server can navigate to
//! (`textDocument/definition` and `textDocument/references`):
//!
//! * **Objects** — definitions such as `CREATE TABLE t (...)`, and
//!   references in clauses such as `SELECT ... FROM t`, `JOIN t`,
//!   `INSERT INTO t`, `UPDATE t`, `DELETE FROM t`, `DROP TABLE t`,
//!   `ALTER TABLE t`, `DESCRIBE t`, `OPTIMIZE t`, `VACUUM t` and
//!   `CREATE ... INDEX ON t`.
//! * **Columns** — definitions in `CREATE TABLE t (col type, ...)` and
//!   `ALTER TABLE t ADD COLUMN col type`, and references resolved per
//!   statement against the visible tables of that statement (`FROM`/`JOIN`
//!   targets with their aliases). Column references are kept only when the
//!   referenced table actually declares the column, and ambiguous
//!   unqualified names keep every candidate table.
//!
//! Like the rest of the crate this is purely lexical: names are recognised
//! from the surrounding tokens, not from a full parser, so the navigation
//! index can never drift from the lexer. Names must be written as unquoted
//! identifiers (backticked identifiers are not picked up yet).

use std::collections::{HashMap, HashSet};

use crate::lexer::{Span, Token, TokenKind, lex};

/// How a name is used in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRole {
    /// `CREATE TABLE t` — the name binds the object.
    Definition,
    /// Anywhere else the object is referred to.
    Reference,
}

/// Rough object category, used for reporting; resolution treats every kind
/// as one namespace (a `CREATE SCHEMA`/`CREATE DATABASE` can qualify names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Table,
    View,
    Schema,
    Database,
    Index,
    Job,
    Nodegroup,
    Other,
}

/// One object name occurrence found in a ScopeQL document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectName {
    /// Lower-cased, dot-joined name path, e.g. `sales.order_events`.
    pub name: String,
    pub kind: ObjectKind,
    pub role: ObjectRole,
    /// Byte range of the name path in the source.
    pub span: Span,
}

/// Object-category keywords. `PARTITION`/`CLUSTER`/`COLUMN`/`KEY` are left
/// out on purpose: in ScopeQL they appear in `PARTITION BY`/`CLUSTER BY`
/// clauses and column lists, not as object names.
const OBJECT_KINDS: &[(&str, ObjectKind)] = &[
    ("table", ObjectKind::Table),
    ("view", ObjectKind::View),
    ("schema", ObjectKind::Schema),
    ("database", ObjectKind::Database),
    ("index", ObjectKind::Index),
    ("job", ObjectKind::Job),
    ("nodegroup", ObjectKind::Nodegroup),
];

/// Index-kind qualifiers that can sit between `CREATE` and the object-kind
/// keyword: `CREATE POINT INDEX`, `CREATE RANGE INDEX`, `CREATE SEARCH
/// INDEX`, `CREATE MATERIALIZED INDEX`, `CREATE MATERIALIZED VIEW`.
const KIND_QUALIFIERS: &[&str] = &["point", "range", "search", "materialized"];

fn kind_of(word: &str) -> Option<ObjectKind> {
    OBJECT_KINDS
        .iter()
        .find(|(w, _)| *w == word)
        .map(|(_, k)| *k)
}

/// Whether two name paths refer to the same object.
///
/// Matching is case-insensitive (names are stored lower-cased) and tolerates
/// different degrees of qualification: `t` matches `sales.t` and
/// `analytics.daily.t` (the shorter path is a dot-component suffix of the
/// longer one), so `FROM t` resolves to `CREATE TABLE sales.t` and vice
/// versa.
pub fn names_match(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

/// Extract every object name occurrence in `source`.
pub fn object_names(source: &str) -> Vec<ObjectName> {
    let (tokens, _) = lex(source);
    let mut out = Vec::new();

    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].kind == TokenKind::Keyword {
            let word = word_at(&tokens[i], source);
            match word.as_str() {
                // `CREATE [OR REPLACE] [POINT|RANGE|SEARCH|MATERIALIZED]
                // <kind> [IF NOT EXISTS] name`, where ScopeQL indexes are
                // anonymous: `CREATE ... INDEX ON table (...)` references the
                // table named after `ON` instead of defining an index name.
                "create" => {
                    let mut j = i + 1;
                    while j < tokens.len()
                        && tokens[j].kind == TokenKind::Keyword
                        && {
                            let w = word_at(&tokens[j], source);
                            matches!(w.as_str(), "or" | "replace")
                                || KIND_QUALIFIERS.contains(&w.as_str())
                        }
                    {
                        j += 1;
                    }
                    if j < tokens.len()
                        && tokens[j].kind == TokenKind::Keyword
                        && let Some(kind) = kind_of(&word_at(&tokens[j], source))
                    {
                        match kind {
                            ObjectKind::Index => {
                                // `CREATE ... INDEX ON table` has no index
                                // name; `ON <table>` is a table reference.
                                let mut k = j + 1;
                                let on_table = k < tokens.len()
                                    && tokens[k].kind == TokenKind::Keyword
                                    && word_at(&tokens[k], source) == "on";
                                if on_table {
                                    k += 1;
                                }
                                if let Some((name, _, span, after)) =
                                    read_object_name(&tokens, k, source)
                                {
                                    out.push(ObjectName {
                                        name,
                                        kind: if on_table {
                                            ObjectKind::Other
                                        } else {
                                            ObjectKind::Index
                                        },
                                        role: if on_table {
                                            ObjectRole::Reference
                                        } else {
                                            ObjectRole::Definition
                                        },
                                        span,
                                    });
                                    i = after;
                                    continue;
                                }
                            }
                            _ => {
                                if let Some((name, _, span, after)) =
                                    read_object_name(&tokens, j, source)
                                {
                                    out.push(ObjectName {
                                        name,
                                        kind,
                                        role: ObjectRole::Definition,
                                        span,
                                    });
                                    i = after;
                                    continue;
                                }
                            }
                        }
                    }
                }
                // `DROP [kind] [IF EXISTS] name`, `ALTER [kind] name`.
                "drop" | "alter" => {
                    if let Some((name, kind, span, after)) = read_object_name(&tokens, i + 1, source) {
                        out.push(ObjectName {
                            name,
                            kind,
                            role: ObjectRole::Reference,
                            span,
                        });
                        i = after;
                        continue;
                    }
                }
                // `DELETE [FROM] name`.
                "delete" => {
                    let mut j = i + 1;
                    if j < tokens.len()
                        && tokens[j].kind == TokenKind::Keyword
                        && word_at(&tokens[j], source) == "from"
                    {
                        j += 1;
                    }
                    if let Some((name, kind, span, after)) = read_object_name(&tokens, j, source) {
                        out.push(ObjectName {
                            name,
                            kind,
                            role: ObjectRole::Reference,
                            span,
                        });
                        i = after;
                        continue;
                    }
                }
                // Clauses that take an object name right after the keyword.
                "from" | "join" | "into" | "update" | "describe" | "optimize" | "vacuum" => {
                    if let Some((name, kind, span, after)) = read_object_name(&tokens, i + 1, source) {
                        out.push(ObjectName {
                            name,
                            kind,
                            role: ObjectRole::Reference,
                            span,
                        });
                        i = after;
                        continue;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }

    out
}

/// Skip object-kind keywords and `IF NOT EXISTS`-style preambles, then read
/// the identifier path that follows. `start` points at the first candidate
/// token (right after the trigger keyword).
fn read_object_name(
    tokens: &[Token],
    start: usize,
    source: &str,
) -> Option<(String, ObjectKind, Span, usize)> {
    let mut kind = ObjectKind::Other;
    let mut j = start;
    while j < tokens.len() {
        if tokens[j].kind != TokenKind::Keyword {
            break;
        }
        let w = word_at(&tokens[j], source);
        if let Some(k) = kind_of(&w) {
            kind = k;
            j += 1;
            continue;
        }
        if matches!(w.as_str(), "if" | "not" | "exists") {
            j += 1;
            continue;
        }
        break;
    }
    let (name, span, after) = read_name_path(tokens, j, source)?;
    Some((name, kind, span, after))
}

/// Read a dot-joined path of identifiers: `a`, `a.b`, `a.b.c`.
fn read_name_path(tokens: &[Token], start: usize, source: &str) -> Option<(String, Span, usize)> {
    if tokens.get(start)?.kind != TokenKind::Ident {
        return None;
    }
    let span_start = tokens[start].span.start;
    let mut end = start;
    let mut parts = Vec::new();
    loop {
        if tokens[end].kind != TokenKind::Ident {
            break;
        }
        parts.push(word_at(&tokens[end], source));
        end += 1;
        if end + 1 < tokens.len()
            && is_dot(&tokens[end], source)
            && tokens[end + 1].kind == TokenKind::Ident
        {
            end += 1;
            continue;
        }
        break;
    }
    let span = Span::new(span_start as usize, tokens[end - 1].span.end as usize);
    Some((parts.join("."), span, end))
}

fn word_at(tok: &Token, source: &str) -> String {
    source[tok.span.start as usize..tok.span.end as usize]
        .to_ascii_lowercase()
}

fn is_dot(tok: &Token, source: &str) -> bool {
    if tok.kind != TokenKind::Operator {
        return false;
    }
    let text = &source[tok.span.start as usize..tok.span.end as usize];
    text == "."
}

// ---------------------------------------------------------------------------
// Column navigation
// ---------------------------------------------------------------------------

/// A column definition: `column` of table `table` (lower-cased dotted path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    pub table: String,
    pub column: String,
    /// Byte range of the *column name* in the source.
    pub span: Span,
}

impl ColumnDef {
    /// The last dot-component of a table path (`sales.logs` -> `logs`).
    pub fn table_key(path: &str) -> String {
        path.rsplit('.').next().unwrap_or(path).to_string()
    }
}

/// A resolved column reference: `column` of table `table` at a reference site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnRef {
    pub table: String,
    pub column: String,
    /// Byte range of the *column name* in the source.
    pub span: Span,
}

/// A table that is in scope for one statement: the path as written plus the
/// alias it was given (`FROM sales.orders o` -> path `sales.orders`,
/// alias `o`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleTable {
    pub path: String,
    pub alias: Option<String>,
}

/// Keywords after which a bare identifier reads as a column reference in a
/// query (`SELECT col`, `WHERE col`, `GROUP BY col`, `SET col = ...`, ...).
/// Mirrors the "column position" heuristic used by [`crate::highlight`].
const COLUMN_POSITION_KEYWORDS: &[&str] = &[
    "select", "where", "on", "by", "group", "order", "having", "limit",
    "offset", "set", "window", "aggregate", "distinct", "values", "between",
    "when", "then", "case",
];

/// Extract column definitions from `source`:
/// `CREATE TABLE t (col type, ...)` column lists and
/// `ALTER TABLE t ADD COLUMN col type`.
pub fn column_definitions(source: &str) -> Vec<ColumnDef> {
    let (tokens, _) = lex(source);
    let mut out = Vec::new();

    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].kind != TokenKind::Keyword {
            i += 1;
            continue;
        }
        match word_at(&tokens[i], source).as_str() {
            "create" => {
                let mut j = i + 1;
                while j < tokens.len()
                    && tokens[j].kind == TokenKind::Keyword
                    && {
                        let w = word_at(&tokens[j], source);
                        matches!(w.as_str(), "or" | "replace")
                            || KIND_QUALIFIERS.contains(&w.as_str())
                    }
                {
                    j += 1;
                }
                let kind_is_table = j < tokens.len()
                    && tokens[j].kind == TokenKind::Keyword
                    && word_at(&tokens[j], source) == "table";
                if !kind_is_table {
                    i += 1;
                    continue;
                }
                let mut k = j + 1;
                while k < tokens.len()
                    && tokens[k].kind == TokenKind::Keyword
                    && matches!(word_at(&tokens[k], source).as_str(), "if" | "not" | "exists")
                {
                    k += 1;
                }
                let Some((table, _, _)) = read_name_path(&tokens, k, source) else {
                    i += 1;
                    continue;
                };
                // The column list is the parenthesised block after the name.
                if let Some(open) = find_lparen(&tokens, k, source) {
                    for cd in parse_column_list(&tokens, open, source, &table) {
                        out.push(cd);
                    }
                }
                i = k + 1;
            }
            "alter" => {
                // `ALTER TABLE <path> ADD COLUMN <col> <type>`.
                let mut j = i + 1;
                while j < tokens.len()
                    && tokens[j].kind == TokenKind::Keyword
                    && (kind_of(&word_at(&tokens[j], source)).is_some()
                        || matches!(word_at(&tokens[j], source).as_str(), "if" | "not" | "exists"))
                {
                    j += 1;
                }
                let Some((table, _, after)) = read_name_path(&tokens, j, source) else {
                    i += 1;
                    continue;
                };
                let mut k = after;
                while k + 2 < tokens.len() {
                    if tokens[k].kind == TokenKind::Keyword
                        && word_at(&tokens[k], source) == "add"
                        && tokens[k + 1].kind == TokenKind::Keyword
                        && word_at(&tokens[k + 1], source) == "column"
                        && tokens[k + 2].kind == TokenKind::Ident
                    {
                        out.push(ColumnDef {
                            table: table.clone(),
                            column: word_at(&tokens[k + 2], source),
                            span: tokens[k + 2].span,
                        });
                        break;
                    }
                    k += 1;
                }
                i = after;
            }
            _ => i += 1,
        }
        i += 1;
    }

    out
}

/// Find the first `(` at or after `start` (usually right after a table name).
fn find_lparen(tokens: &[Token], start: usize, source: &str) -> Option<usize> {
    tokens[start..]
        .iter()
        .position(|t| t.kind == TokenKind::Operator && punct_char(t, source) == Some('('))
        .map(|off| start + off)
}

/// Parse the parenthesised column list of `CREATE TABLE`:
/// every `<col> <type>` pair. Non-pair tokens (constraints, nested parens)
/// are ignored.
fn parse_column_list(
    tokens: &[Token],
    open: usize,
    source: &str,
    table: &str,
) -> Vec<ColumnDef> {
    let mut out = Vec::new();
    let mut depth = 1usize;
    let mut i = open + 1;
    while i < tokens.len() && depth > 0 {
        let t = &tokens[i];
        if t.kind == TokenKind::Operator {
            match punct_char(t, source) {
                Some('(') => {
                    depth += 1;
                    i += 1;
                    continue;
                }
                Some(')') => {
                    depth -= 1;
                    i += 1;
                    continue;
                }
                _ => {}
            }
        }
        if t.kind == TokenKind::Ident
            && tokens
                .get(i + 1)
                .is_some_and(|n| matches!(n.kind, TokenKind::Ident | TokenKind::Type))
        {
            out.push(ColumnDef {
                table: table.to_string(),
                column: word_at(t, source),
                span: t.span,
            });
        }
        i += 1;
    }
    out
}

/// The token ranges of the statements in `tokens`, split on `;`.
/// A trailing unterminated statement is included.
pub fn statements(tokens: &[Token], source: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (idx, t) in tokens.iter().enumerate() {
        if t.kind == TokenKind::Operator && punct_char(t, source) == Some(';') {
            ranges.push((start, idx));
            start = idx + 1;
        }
    }
    if start < tokens.len() {
        ranges.push((start, tokens.len()));
    }
    ranges
}

/// The token range of the statement containing byte `byte`.
pub fn statement_range(tokens: &[Token], byte: usize, source: &str) -> (usize, usize) {
    let mut from = 0;
    let mut to = tokens.len();
    for (idx, t) in tokens.iter().enumerate() {
        if t.kind == TokenKind::Operator && punct_char(t, source) == Some(';') {
            if (t.span.start as usize) < byte {
                from = idx + 1;
            } else if to == tokens.len() {
                to = idx;
            }
        }
    }
    (from, to)
}

/// The tables visible in a statement: the targets of `FROM`, `JOIN`,
/// `INSERT INTO`, `UPDATE`, `DELETE [FROM]`, and the `ON` target of
/// `CREATE ... INDEX ON t`, together with their aliases (`FROM t AS x` or
/// a bare alias `FROM t x`).
pub fn visible_tables(tokens: &[Token], from: usize, to: usize, source: &str) -> Vec<VisibleTable> {
    let mut out = Vec::new();
    let mut i = from;
    while i < to {
        if tokens[i].kind != TokenKind::Keyword {
            i += 1;
            continue;
        }
        let word = word_at(&tokens[i], source);
        let (pick, start) = match word.as_str() {
            "delete" => {
                let mut s = i + 1;
                if s < to
                    && tokens[s].kind == TokenKind::Keyword
                    && word_at(&tokens[s], source) == "from"
                {
                    s += 1;
                }
                (true, s)
            }
            "from" | "join" | "into" | "update" => (true, i + 1),
            // `CREATE ... INDEX ON t` makes `t` visible (index statements
            // index its columns).
            "create" => {
                let mut j = i + 1;
                while j < to
                    && tokens[j].kind == TokenKind::Keyword
                    && {
                        let w = word_at(&tokens[j], source);
                        matches!(w.as_str(), "or" | "replace")
                            || KIND_QUALIFIERS.contains(&w.as_str())
                    }
                {
                    j += 1;
                }
                if j + 1 < to
                    && tokens[j].kind == TokenKind::Keyword
                    && word_at(&tokens[j], source) == "index"
                    && tokens[j + 1].kind == TokenKind::Keyword
                    && word_at(&tokens[j + 1], source) == "on"
                {
                    (true, j + 2)
                } else {
                    (false, 0)
                }
            }
            _ => (false, 0),
        };
        if !pick {
            i += 1;
            continue;
        }
        let Some((path, _, after)) = read_name_path(tokens, start, source) else {
            i += 1;
            continue;
        };
        let mut alias = None;
        let mut a = after;
        if a < to && tokens[a].kind == TokenKind::Keyword && word_at(&tokens[a], source) == "as" {
            a += 1;
        }
        if a < to
            && tokens[a].kind == TokenKind::Ident
            && !(a + 1 < to
                && tokens[a + 1].kind == TokenKind::Operator
                && punct_char(&tokens[a + 1], source) == Some('('))
        {
            alias = Some(word_at(&tokens[a], source));
        }
        out.push(VisibleTable { path, alias });
        i = after;
    }
    out
}

/// Whether `tokens[idx]` (an identifier) sits in a column-reference position:
/// right after a query-clause keyword, or after `(`, `,`, `=`, `[` or `:`.
pub fn is_column_position(tokens: &[Token], idx: usize, source: &str) -> bool {
    let Some(prev) = idx.checked_sub(1).map(|p| &tokens[p]) else {
        return false;
    };
    match prev.kind {
        TokenKind::Keyword => {
            let w = word_at(prev, source);
            COLUMN_POSITION_KEYWORDS.contains(&w.as_str())
        }
        TokenKind::Operator => matches!(
            punct_char(prev, source),
            Some('(') | Some(',') | Some('=') | Some('[') | Some(':')
        ),
        _ => false,
    }
}

/// The dot-joined identifier chain containing `tokens[idx]` and the position
/// of `idx` inside it (`sales.order_events` with the cursor on `order` ->
/// parts `["sales", "order", "events"]`, index 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentChain {
    pub parts: Vec<String>,
    pub idx_in_chain: usize,
}

pub fn ident_chain(tokens: &[Token], idx: usize, source: &str) -> IdentChain {
    let mut start = idx;
    while start >= 2
        && is_dot(&tokens[start - 1], source)
        && tokens[start - 2].kind == TokenKind::Ident
    {
        start -= 2;
    }
    let mut end = idx + 1;
    while end + 1 < tokens.len()
        && is_dot(&tokens[end], source)
        && tokens[end + 1].kind == TokenKind::Ident
    {
        end += 2;
    }
    let mut parts = Vec::new();
    let mut idx_in_chain = 0;
    for (k, t) in tokens[start..end].iter().enumerate() {
        if t.kind == TokenKind::Ident {
            if start + k == idx {
                idx_in_chain = parts.len();
            }
            parts.push(word_at(t, source));
        }
    }
    IdentChain {
        parts,
        idx_in_chain,
    }
}

/// Extract column references from `source`. `known` maps a table's last
/// path component to the set of columns it declares (from
/// [`column_definitions`] across the workspace); a reference is kept only
/// when the table it resolves to actually declares the column.
pub fn column_references(
    source: &str,
    known: &HashMap<String, HashSet<String>>,
) -> Vec<ColumnRef> {
    let (tokens, _) = lex(source);
    let mut out = Vec::new();

    for (from, to) in statements(&tokens, source) {
        let visible = visible_tables(&tokens, from, to, source);
        if visible.is_empty() {
            continue;
        }
        let mut i = from;
        while i < to {
            if tokens[i].kind != TokenKind::Ident {
                i += 1;
                continue;
            }
            // Function calls are not columns.
            if i + 1 < to
                && tokens[i + 1].kind == TokenKind::Operator
                && punct_char(&tokens[i + 1], source) == Some('(')
            {
                i += 1;
                continue;
            }
            // Qualified member access: `alias.col` or `table.col`.
            if i >= 2
                && is_dot(&tokens[i - 1], source)
                && tokens[i - 2].kind == TokenKind::Ident
            {
                let qualifier = word_at(&tokens[i - 2], source);
                let column = word_at(&tokens[i], source);
                for v in &visible {
                    let qualifier_matches = v.alias.as_deref() == Some(qualifier.as_str())
                        || names_match(&v.path, &qualifier);
                    if qualifier_matches && known_has(known, &v.path, &column) {
                        out.push(ColumnRef {
                            table: v.path.clone(),
                            column: column.clone(),
                            span: tokens[i].span,
                        });
                    }
                }
                i += 1;
                continue;
            }
            // Bare identifier in a column position: candidate for every
            // visible table (ambiguous joins keep all candidates).
            if is_column_position(&tokens, i, source) {
                let column = word_at(&tokens[i], source);
                for v in &visible {
                    if known_has(known, &v.path, &column) {
                        out.push(ColumnRef {
                            table: v.path.clone(),
                            column: column.clone(),
                            span: tokens[i].span,
                        });
                    }
                }
            }
            i += 1;
        }
    }

    out
}

fn known_has(known: &HashMap<String, HashSet<String>>, table_path: &str, column: &str) -> bool {
    known
        .get(&ColumnDef::table_key(table_path))
        .is_some_and(|cols| cols.contains(column))
}

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

    /// `(name, role)` pairs in source order.
    fn names(source: &str) -> Vec<(String, ObjectRole)> {
        object_names(source)
            .into_iter()
            .map(|n| (n.name, n.role))
            .collect()
    }

    #[test]
    fn extracts_create_table_definition() {
        let out = names("CREATE TABLE employee_table (\n  employee_id int,\n);");
        assert_eq!(
            out,
            vec![("employee_table".to_string(), ObjectRole::Definition)]
        );
    }

    #[test]
    fn handles_create_preamble_and_kinds() {
        assert_eq!(
            names("CREATE OR REPLACE VIEW v AS SELECT 1;"),
            vec![("v".to_string(), ObjectRole::Definition)]
        );
        assert_eq!(
            names("CREATE TABLE IF NOT EXISTS t (x int);"),
            vec![("t".to_string(), ObjectRole::Definition)]
        );
        assert_eq!(
            names("CREATE SCHEMA sales;"),
            vec![("sales".to_string(), ObjectRole::Definition)]
        );
        assert_eq!(
            names("CREATE DATABASE analytics;"),
            vec![("analytics".to_string(), ObjectRole::Definition)]
        );
    }

    #[test]
    fn extracts_query_references() {
        let src = "SELECT * FROM events WHERE id > 0 \
                   JOIN orders ON orders.id = events.id \
                   INSERT INTO archive \
                   UPDATE staging SET x = 1 \
                   DELETE FROM trash WHERE x = 0 \
                   DROP TABLE old_events \
                   ALTER TABLE events ADD COLUMN note string;";
        let out = names(src);
        let refs: Vec<&str> = out
            .iter()
            .filter(|(_, r)| *r == ObjectRole::Reference)
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(
            refs,
            vec![
                "events", "orders", "archive", "staging", "trash", "old_events", "events"
            ]
        );
        assert!(out.iter().all(|(_, r)| *r == ObjectRole::Reference));
    }

    #[test]
    fn resolves_qualified_names() {
        // `join sales.orders` produces the dotted path `sales.orders`.
        let out = names("SELECT * FROM analytics.daily.events JOIN sales.orders o ON 1 = 1;");
        assert!(out.contains(&("analytics.daily.events".to_string(), ObjectRole::Reference)));
        assert!(out.contains(&("sales.orders".to_string(), ObjectRole::Reference)));

        assert!(names_match("sales.orders", "orders"));
        assert!(names_match("orders", "sales.orders"));
        assert!(names_match("analytics.daily.events", "events"));
        assert!(names_match("events", "events"));
        assert!(!names_match("sales.orders", "analytics.orders"));
        assert!(!names_match("inventory", "orders"));
    }

    #[test]
    fn ignores_non_object_keywords() {
        let out = names(
            "WITH cte AS (SELECT 1) \
             SELECT a, b FROM cte \
             WINDOW w AS (ORDER BY a) \
             SELECT * FROM t PARTITION BY a CLUSTER BY b \
             ALTER TABLE t ALTER COLUMN c SET DEFAULT 1;",
        );
        let named: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        // `cte` (after WITH), the two `t` references (`FROM t` and
        // `ALTER TABLE t`) are collected; the `ALTER COLUMN c`, `PARTITION
        // BY a` / `CLUSTER BY b` and `WINDOW w` positions must not produce
        // spurious object names.
        assert_eq!(named, vec!["cte", "t", "t"]);
    }

    #[test]
    fn index_statements_reference_the_target_table() {
        // ScopeQL indexes are anonymous: `CREATE <TYPE> INDEX ON table (...)`
        // references the table after `ON`; it does not define an index name.
        let src = "CREATE TABLE logs (id int);\n\
                   CREATE POINT INDEX ON logs (id);\n\
                   CREATE RANGE INDEX ON logs (time);\n\
                   CREATE SEARCH INDEX ON logs (message) WITH ('analyzer' = 'log');\n\
                   CREATE MATERIALIZED INDEX ON logs (var['host']::string);";
        let out = names(src);
        let uses: Vec<(String, ObjectRole)> = out
            .into_iter()
            .filter(|(n, _)| n == "logs")
            .collect();
        let definitions = uses
            .iter()
            .filter(|(_, r)| *r == ObjectRole::Definition)
            .count();
        let references = uses
            .iter()
            .filter(|(_, r)| *r == ObjectRole::Reference)
            .count();
        assert_eq!((definitions, references), (1, 4), "{uses:?}");
        // A plain `CREATE INDEX` without `ON` still registers the name.
        assert_eq!(
            names("CREATE INDEX idx ON logs (id);"),
            vec![("idx".to_string(), ObjectRole::Definition)]
        );
    }

    #[test]
    fn materialized_view_is_a_definition() {
        assert_eq!(
            names("CREATE MATERIALIZED VIEW v AS SELECT 1;"),
            vec![("v".to_string(), ObjectRole::Definition)]
        );
    }

    #[test]
    fn join_on_columns_are_not_tables() {
        // `ON` must only trigger inside index statements; `a.id`/`b.id` in a
        // join condition are column references and must not be collected.
        let out = names("SELECT * FROM a JOIN b ON a.id = b.id JOIN c ON c.x = b.x;");
        let refs: Vec<&str> = out
            .iter()
            .filter(|(_, r)| *r == ObjectRole::Reference)
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(refs, vec!["a", "b", "c"]);
    }

    #[test]
    fn names_are_normalized_to_lowercase() {
        // Extraction lower-cases, so mixed-case spellings still match.
        assert_eq!(
            names("CREATE TABLE Employee_Table (id int);"),
            vec![("employee_table".to_string(), ObjectRole::Definition)]
        );
        assert!(names_match("employee_table", "employee_table"));
        assert!(names_match("sales.orders", "sales.orders"));
    }

    #[test]
    fn names_match_ignores_qualification() {
        assert!(names_match("sales.orders", "orders"));
        assert!(names_match("analytics.daily.events", "events"));
    }

    // --- column navigation ----------------------------------------------

    #[test]
    fn column_definitions_extract_create_table_columns() {
        let src = "CREATE TABLE logs (\n\
                   \x20 id int,\n\
                   \x20 time timestamp,\n\
                   \x20 message string,\n\
                   \x20 var object\n\
                   );";
        let defs = column_definitions(src);
        let cols: Vec<(&str, &str)> = defs
            .iter()
            .map(|d| (d.table.as_str(), d.column.as_str()))
            .collect();
        assert_eq!(
            cols,
            vec![
                ("logs", "id"),
                ("logs", "time"),
                ("logs", "message"),
                ("logs", "var"),
            ]
        );
    }

    #[test]
    fn column_definitions_handle_alter_add_column() {
        let defs = column_definitions("ALTER TABLE logs ADD COLUMN note string;");
        assert_eq!(defs.len(), 1);
        assert_eq!((defs[0].table.as_str(), defs[0].column.as_str()), ("logs", "note"));
    }

    #[test]
    fn visible_tables_cover_from_join_into_and_aliases() {
        let src = "SELECT * FROM sales.events e JOIN customers c ON e.id = c.id \
                   INSERT INTO archive \
                   UPDATE staging SET x = 1 \
                   DELETE FROM trash \
                   CREATE POINT INDEX ON logs (time);";
        let (tokens, _) = lex(src);
        let visible = visible_tables(&tokens, 0, tokens.len(), src);
        let rows: Vec<(String, Option<String>)> = visible
            .iter()
            .map(|v| (v.path.clone(), v.alias.clone()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("sales.events".to_string(), Some("e".to_string())),
                ("customers".to_string(), Some("c".to_string())),
                ("archive".to_string(), None),
                ("staging".to_string(), None),
                ("trash".to_string(), None),
                ("logs".to_string(), None),
            ]
        );
    }

    #[test]
    fn statements_are_split_on_semicolons() {
        let src = "SELECT 1; CREATE TABLE t (a int); SELECT 2";
        let (tokens, _) = lex(src);
        let ranges = statements(&tokens, src);
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0], (0, 2));
        assert_eq!(ranges[2], (11, tokens.len()));
        // statement_range finds the statement owning a byte offset.
        let byte = tokens[8].span.start as usize; // `t` in CREATE TABLE t
        let (from, to) = statement_range(&tokens, byte, src);
        assert_eq!((from, to), ranges[1]);
    }

    #[test]
    fn column_references_resolve_qualified_and_alias_members() {
        let known: HashMap<String, HashSet<String>> = [(
            "customers".to_string(),
            ["id", "name"].into_iter().map(str::to_string).collect(),
        )]
        .into_iter()
        .collect();
        let src = "SELECT * FROM customers c WHERE c.id > 0 AND c.name <> '';";
        let refs = column_references(src, &known);
        let rows: Vec<(&str, &str)> = refs
            .iter()
            .map(|r| (r.table.as_str(), r.column.as_str()))
            .collect();
        assert_eq!(rows, vec![("customers", "id"), ("customers", "name")]);
    }

    #[test]
    fn column_references_keep_anonymous_join_candidates_and_filter_unknown() {
        let known: HashMap<String, HashSet<String>> = [
            ("a".to_string(), ["id"].into_iter().map(str::to_string).collect()),
            ("b".to_string(), ["id"].into_iter().map(str::to_string).collect()),
        ]
        .into_iter()
        .collect();
        let src = "SELECT id, bogus FROM a JOIN b ON a.id = b.id;";
        let refs = column_references(src, &known);
        let rows: Vec<(&str, &str)> = refs
            .iter()
            .map(|r| (r.table.as_str(), r.column.as_str()))
            .collect();
        // `bogus` is filtered out (no table declares it); unambiguous `id`
        // keeps both candidate tables; `ON a.id = b.id` resolves via aliases.
        assert_eq!(
            rows,
            vec![
                ("a", "id"),
                ("b", "id"),
                ("a", "id"),
                ("b", "id"),
            ]
        );
    }

    #[test]
    fn is_column_position_distinguishes_positions() {
        let src = "SELECT service, count(*) FROM t WHERE time > now();";
        let (tokens, _) = lex(src);
        let idx = |w: &str| tokens.iter().position(|t| t.text(src) == w).unwrap();
        assert!(is_column_position(&tokens, idx("service"), src));
        assert!(is_column_position(&tokens, idx("time"), src));
        // `count` sits after `,` (a column-shaped position) but is excluded
        // from references by the function-call check in column_references;
        // `now` follows `>` and is not a column position at all.
        assert!(is_column_position(&tokens, idx("count"), src));
        assert!(!is_column_position(&tokens, idx("now"), src));
    }

    #[test]
    fn ident_chain_finds_dotted_paths() {
        let src = "SELECT * FROM sales.events WHERE a.x > b.y;";
        let (tokens, _) = lex(src);
        let x_idx = tokens.iter().position(|t| t.text(src) == "x").unwrap();
        let chain = ident_chain(&tokens, x_idx, src);
        assert_eq!(chain.parts, vec!["a", "x"]);
        assert_eq!(chain.idx_in_chain, 1);
        let a_idx = tokens.iter().position(|t| t.text(src) == "a").unwrap();
        let chain_a = ident_chain(&tokens, a_idx, src);
        assert_eq!(chain_a.parts, vec!["a", "x"]);
        assert_eq!(chain_a.idx_in_chain, 0);
    }
}