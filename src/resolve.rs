//! Object-name extraction and matching for workspace-wide navigation.
//!
//! This module identifies the names the language server can navigate to
//! (`textDocument/definition` and `textDocument/references`): object
//! *definitions* such as `CREATE TABLE t (...)`, and object *references* in
//! clauses such as `SELECT ... FROM t`, `JOIN t`, `INSERT INTO t`,
//! `UPDATE t`, `DELETE FROM t`, `DROP TABLE t`, `ALTER TABLE t`,
//! `DESCRIBE t`, `OPTIMIZE t` and `VACUUM t`.
//!
//! Like the rest of the crate this is purely lexical: names are recognised
//! from the surrounding keywords, not from a full parser, so the navigation
//! index can never drift from the lexer. Names must be written as unquoted
//! identifiers (backticked identifiers are not picked up yet).

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
}