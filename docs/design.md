# scopeql-lsp: Navigation Design

This document describes how `scopeql-lsp` implements `textDocument/definition`
and `textDocument/references` for the ScopeQL language. The design goal is to
stay *lexical*: names are recognised from the surrounding token stream, not
from a full SQL parser, so the navigation index can never drift from the
vendored lexer and stays cheap to compute.

## 1. Model

A ScopeQL source is tokenized by `src/lexer.rs` into a stream of `Token`s
with byte spans. Two independent extraction passes run over that stream:

| pass | produces | used for |
| --- | --- | --- |
| `resolve::object_names` | object names: definitions and references | navigating tables, views, schemas, databases, indexes, jobs |
| `resolve::column_definitions` | `(table, column)` from `CREATE TABLE` / `ALTER TABLE ... ADD COLUMN` | jumping to column definitions |
| `resolve::column_references` | resolved `(table, column)` references per statement | finding column references |

The server (`src/main.rs`) builds a flat workspace index from these passes
over every `.scopeql` file under the workspace root(s) plus all open
documents, then answers navigation requests by filtering that index.

## 2. Object names (tables, views, schemas, ...)

Object positions are **syntactically anchored**: a name after one of a small
set of keywords is an object reference, and a name after `CREATE <kind>` is
an object definition.

| role | pattern |
| --- | --- |
| definition | `CREATE [OR REPLACE] [POINT\|RANGE\|SEARCH\|MATERIALIZED] <kind> [IF NOT EXISTS] name` |
| reference | `FROM name`, `JOIN name`, `INSERT INTO name`, `UPDATE name`, `DELETE [FROM] name`, `DROP [kind] name`, `ALTER [kind] name`, `DESCRIBE name`, `OPTIMIZE name`, `VACUUM name` |
| reference | `CREATE ... INDEX ON name` (ScopeQL indexes are anonymous; the `ON` target is a reference to the table it indexes) |

A name path may be qualified: `sales.orders` is read as one dotted path.
`PARTITION BY` / `CLUSTER BY` / `ALTER COLUMN c` are deliberately **not**
object positions, so columns and partitioning expressions are not mistaken
for objects.

**Matching.** Names are lower-cased and compared by `resolve::names_match`:
two paths match when they are equal or one is a dot-component suffix of the
other (`t` matches `sales.t`). Comparison is case-insensitive.

## 3. Column names

Column positions are **not** syntactically anchored — a bare identifier in
an expression can be a column, an alias, a function name or a CTE name. The
server resolves columns in two stages.

### 3.1 Definitions

`resolve::column_definitions` scans `CREATE TABLE t (col type, ...)`
parenthesised column lists (an `Ident` immediately followed by an
`Ident`/`Type` token is a column) and `ALTER TABLE t ADD COLUMN col type`.
Each definition carries the owning table's path.

### 3.2 Per-statement scope (visible tables)

`resolve::visible_tables` computes the tables in scope for one statement
(tokens between top-level `;`s):

* `FROM t` / `JOIN t` / `INSERT INTO t` / `UPDATE t` / `DELETE [FROM] t`
* `CREATE ... INDEX ON t` (an index statement can reference the indexed
  table's columns)
* aliases: `FROM t AS x` or `FROM t x`; a bare alias is only accepted when
  the token after the table path is an identifier that is not followed by
  `(` (so `FROM t WHERE ...` does not treat `WHERE` as an alias).

### 3.3 References

`resolve::column_references` walks each statement and treats:

* `alias.col` / `table.col` — a member after a dot, where the qualifier
  resolves to a visible table (by alias or path);
* bare identifiers in *column positions* — after the clause keywords
  `SELECT WHERE ON BY GROUP ORDER HAVING LIMIT OFFSET SET WINDOW AGGREGATE
  DISTINCT VALUES BETWEEN WHEN THEN CASE`, or after `(`, `,`, `=`, `[`, `:`;
* .. unless the identifier is a function call (`ident(`).

Each candidate is recorded as `(table, column)`. A reference is kept only
when the workspace's column definitions confirm the table declares that
column (filtered against a `table-last-component -> set(columns)` map), so
typos and unknown columns produce no phantom navigation targets. Unqualified
names in a join keep **all** candidate tables — ambiguity is surfaced to the
client as multiple locations rather than resolved arbitrarily.

## 4. The workspace index

`Server::build_index` runs on demand for every navigation request:

1. Walk the workspace root(s) for `*.scopeql` files (skipping VCS, build,
   virtualenv and other noise directories), reading only files whose
   mtime/size changed since the last request (cached in `fs_cache`).
2. Overlay open documents on top (an open buffer's text wins over the disk
   copy of the same URI).
3. Phase A: extract object names and column definitions; build the
   known-columns map.
4. Phase B: extract column references, filtered by the known-columns map.

Roots come from the client's `workspaceFolders`, falling back to `rootUri`,
`rootPath`, then the queried document's directory.

## 5. Resolving a request

`cursor_target` classifies the identifier under the cursor:

| cursor position | target |
| --- | --- |
| qualifier of a dotted path (`sales` in `sales.orders`) | object (`sales`) |
| last member of a dotted path (`orders` in `sales.orders`) | column of table `sales` |
| bare identifier after a clause keyword / punctuation | column of every visible table |
| any other bare identifier | object |

For a column target, `cursor_tables` re-derives the visible tables of the
cursor's statement, resolves the qualifier (alias → its target path;
fallback: the qualifier itself), and — when nothing is visible — falls back
to the column's own defining table (so `gd` on a definition finds itself).
The index is then filtered by role (`Definition` for `gd`; both for `gr`)
and by `names_match` on the table paths.

`gd` returns a scalar location when unambiguous and an array when several
definitions match; unknown targets return `null`. `gr` returns the sorted,
de-duplicated list of definitions and references.

## 5a. Rename

`textDocument/rename` reuses the same matching as references. For every
entry whose name matches the cursor target (objects by `names_match` on the
path; columns by name plus owning table), a `TextEdit` is emitted for the
entry's **`last_span`** — the final identifier of an object path, or the
column name. Replacing the final identifier instead of the whole path keeps
schema qualifiers intact: renaming `sales.orders` produces `sales.new_name`.
Edits are grouped by URI into a `WorkspaceEdit`, sorted by position within
each file, and de-duplicated. Because matching is workspace-wide and
alias-aware, renaming a table or column updates its definition, every
reference (including those written through aliases such as `c.id`) and every
index `ON t (col)` target — across all `.scopeql` files under the workspace
root.

## 6. Known limitations

* Only the *first* table of a comma-separated `FROM a, b` list is visible;
  later tables in the same `FROM` are not collected.
* CTE scopes (`WITH cte AS (...)`) are not tracked: `SELECT ... FROM cte`
  resolves against stored objects only, and a subquery's `FROM` leaks into
  the enclosing statement's visible set.
* Columns are only indexed when their `CREATE TABLE`/`ALTER ... ADD COLUMN`
  is in the workspace; a column used before its table is indexed yields no
  references.
* Backticked identifiers are not indexed.
* Nested navigation (cursor on a semi-structured bracket path like
  `var['host']`) resolves `var` as a column of the visible table; the member
  inside the brackets is not resolved.

## 7. Future work

* Multi-table `FROM` support and real scoping for subqueries / CTEs.
* `INSERT INTO t (col, ...) VALUES ...` target columns are already captured
  implicitly (they sit in column positions); explicit support would make
  this robust.
* Incremental indexing keyed on file watchers instead of per-request scans.