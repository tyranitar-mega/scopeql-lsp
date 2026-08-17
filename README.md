# scopeql-lsp

A self-contained language server for [ScopeQL](https://docs.scopedb.io),
the query language of ScopeDB, together with a **vim plugin** that gives
`.scopeql` files semantic (LSP-based) syntax highlighting in coc.nvim.

The server is deliberately **independent of the ScopeDB source tree**: it
ships its own vendored ScopeQL lexer, so it can be built, hosted and evolved
in this repository on its own.

## Features

- **Semantic tokens** (`textDocument/semanticTokens/full`) — the editor
  colors keywords, type names, object names, columns, functions, strings,
  numbers, operators and comments from a stable LSP legend. Object-defining
  keywords (`CREATE`, `DROP`, `ALTER`, ...) carry the `declaration` modifier.
- **Go to definition** (`textDocument/definition`) — `gd` on a table (or
  view / schema / database / index / job) name jumps to its `CREATE ...`
  site, even across files.
- **Find references** (`textDocument/references`) — `gr` on an object name
  lists every mention of it in the workspace, including its definition. Both
  work on qualified names (`FROM sales.orders` resolves `CREATE TABLE
  sales.orders ...`) and are case-insensitive.
- **Diagnostics** (`textDocument/publishDiagnostics`) — lexical errors
  (unrecognized input) are pushed to the editor as errors.
- **Hover** (`textDocument/hover`) — a short description for keywords, types
  and literals.
- **Full document sync** — the server re-lexes on every keystroke; semantic
  highlighting is stateless and fast. Navigation builds a small workspace
  index on demand by scanning the workspace root(s) for `.scopeql` files and
  overlaying the open documents, so it always reflects the files currently
  on disk.

Semantic classification is lexical: identifiers right before `(` read as
function calls, names after `CREATE TABLE`/`FROM`/`JOIN`/`INTO`/... read as
object names, members after `.` and *"column positions"* (after `SELECT`,
`WHERE`, `,`, `(`, `=`) read as columns. This gives genuinely useful colors
without name resolution, and it cannot drift from the lexer.

## Installation (private repository)

This repository is **private**, so it cannot be cloned anonymously — GitHub
will prompt for credentials. Pick one of the two authenticated transports:

```bash
# SSH — requires a GitHub SSH key on your machine (recommended)
git clone git@github.com:tyranitar-mega/scopeql-lsp.git

# HTTPS — prompts for your GitHub username and a personal access token (PAT)
git clone https://github.com/tyranitar-mega/scopeql-lsp.git
```

> For HTTPS, use a PAT with at least `Contents: Read` scope as the password —
> GitHub no longer accepts account passwords for git. If you prefer not to
> issue tokens, the repository owner can add you as a collaborator
> (**Settings → Collaborators** on the GitHub repo page) and you can then
> clone over SSH with your key.

Once cloned, build the server binary as described in
[Building](#building) below, then register the plugin in vim as shown in
[vim + coc.nvim](#vim--cocnvim-recommended).

To pull later updates from the repository:

```bash
git pull
```

## Building

Requires Rust **1.88+** (edition 2024).

```bash
cargo build --release
# binary: target/release/scopeql-lsp
```

Run the unit tests and the end-to-end LSP smoke test:

```bash
cargo test
python3 scripts/smoke_test.py target/release/scopeql-lsp
```

## Using the language server

Any LSP client can connect to `scopeql-lsp` over stdio. It advertises:

| capability | value |
| --- | --- |
| `textDocumentSync` | full |
| `semanticTokensProvider` | full document |
| `hoverProvider` | true |
| `definitionProvider` | true (workspace-wide) |
| `referencesProvider` | true (workspace-wide) |
| diagnostics | push via `publishDiagnostics` |

### Navigating objects

Point the cursor at an object name — after `FROM`/`JOIN`/`INSERT INTO`/
`UPDATE`/`DELETE FROM`/`DROP`/`ALTER`/`DESCRIBE`/..., or on its own
`CREATE` site — and:

- `gd` jumps to the definition (`:CocAction('jumpDefinition')`).
- `gr` lists all references (`:CocAction('jumpReferences')`).

The index is built from every `.scopeql` file under the workspace root(s)
reported by the client (or the current file's directory when none are
reported), so navigation works across files. Unqualified and qualified
spellings are matched case-insensitively and by dot-component suffix:
`FROM t` finds `CREATE TABLE sales.t`, and vice versa.

### Known limitations

- Only *object* names are resolved (tables, views, schemas, databases,
  indexes, jobs, nodegroups). Columns and aliases are not — `gd`/`gr` on a
  column returns nothing.
- Names must be written as unquoted identifiers; backticked identifiers are
  not indexed yet.
- In a multi-table `FROM a, b` list only the name following `FROM` (and each
  `JOIN`) is collected; later comma-separated tables in the same `FROM` are
  not.

### vim + coc.nvim (recommended)

1. Clone the repository locally (see
   [Installation](#installation-private-repository); the repo is private, so
   an SSH key or PAT is required), then install the plugin with
   [vim-plug](https://github.com/junegunn/vim-plug), pointing at your clone:

   ```vim
   Plug '~/code/scopeql-lsp'   " use the path of your local clone
   ```

   or add that path to your `runtimepath` manually.

2. Make sure `scopeql-lsp` is in `$PATH`, or point the config at the built
   binary (see `g:scopeql_lsp_command`). For example:

   ```bash
   export PATH="$HOME/code/scopeql-lsp/target/release:$PATH"
   ```

3. Register the server and enable semantic tokens in **coc-settings.json**
   (project-local `.vim/coc-settings.json` or `:CocConfig`):

   ```json
   {
     "semanticTokens.enable": true,
     "languageserver": {
       "scopeql": {
         "command": "scopeql-lsp",
         "filetypes": ["scopeql"],
         "rootPatterns": ["**/*.scopeql"]
       }
     }
   }
   ```

   Run `:CocRestart` afterwards. You can also open a `.scopeql` file and use
   the `:ScopeQLSetupCoc` command to print this snippet.

4. The plugin maps the LSP token types onto vim highlight groups. Override
   any of them in your `vimrc`:

   ```vim
   highlight CocSemTypeFunction guifg=#ff8700
   highlight CocSemTypeType gui=bold
   ```

   Without coc (or before it connects) the fallback `syntax/scopeql.vim`
   still highlights keywords, types, strings, numbers and comments.

### Other editors

`scopeql-lsp` speaks standard LSP, so it also works with neovim's built-in
client, Sublime LSP, helix, and any other LSP-capable editor — just register
it for the `scopeql` filetype.

## Project layout

```
scopeql-lsp/
├── Cargo.toml / Cargo.lock   Rust package (binary + library)
├── src/
│   ├── lexer.rs              vendored ScopeQL lexer (logos)
│   ├── highlight.rs          token → semantic token classification + legend
│   ├── resolve.rs            object-name extraction + matching (gd / gr)
│   ├── doc.rs                byte-offset ↔ LSP position mapping
│   ├── main.rs               LSP message loop, workspace index, request handlers
│   └── lib.rs                library facade
├── plugin/                   vim plugin: coc semantic-token highlight links
├── ftdetect/                 .scopeql filetype detection
├── ftplugin/                 scopeql editing options
├── syntax/                   fallback lexical syntax highlighting
└── scripts/smoke_test.py     stdio LSP smoke test (tokens, hover, gd, gr)
```

## License

Apache-2.0, with attribution to [ScopeDB](https://github.com/scopedb/scopedb)
for the derived ScopeQL token vocabulary. See [LICENSE](LICENSE).
