//! The `scopeql-lsp` language server binary.
//!
//! It speaks the Language Server Protocol over stdio: on startup it advertises
//! semantic-token, hover, diagnostics, definition and references
//! capabilities, then serves the `textDocument/*` requests for open ScopeQL
//! documents. Definitions and references are workspace-wide: the server
//! scans the workspace root(s) for `.scopeql` files on demand and overlays
//! the open documents on top.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as LspNotification,
};
use lsp_types::request::{
    GotoDefinition, HoverRequest, References, Request as _, SemanticTokensFullRequest,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DiagnosticSeverity, Diagnostic as LspDiagnostic, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverContents, InitializeParams, Location, MarkupContent, MarkupKind, OneOf, Position,
    PublishDiagnosticsParams, ReferenceParams, SemanticToken, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensResult,
    SemanticTokensServerCapabilities, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, Uri,
};
use scopeql_lsp::doc::{LineIndex, utf16_len};
use scopeql_lsp::highlight::{LEGEND_MODIFIERS, LEGEND_TYPES, semantic_tokens};
use scopeql_lsp::lexer::{self, Token, TokenKind};
use scopeql_lsp::resolve::{self, ObjectRole};

fn main() {
    if let Err(e) = run() {
        eprintln!("scopeql-lsp: {e}");
        std::process::exit(1);
    }
}

/// A tracked open document.
#[derive(Default)]
struct Document {
    text: String,
}

/// Cached on-disk content for a workspace file.
#[derive(Default)]
struct FsEntry {
    mtime: Option<std::time::SystemTime>,
    len: u64,
    text: String,
}

/// One indexed source: the URI plus the text it was parsed from.
struct IndexFile {
    uri: Uri,
    text: String,
}

/// One object-name occurrence in an indexed source.
struct IndexEntry {
    name: String,
    role: ObjectRole,
    span: scopeql_lsp::lexer::Span,
    /// Index into [`WorkspaceIndex::files`].
    file: usize,
}

/// The workspace-wide navigation index: every object-name occurrence in
/// every `.scopeql` file under the workspace root(s), plus all open docs.
struct WorkspaceIndex {
    files: Vec<IndexFile>,
    entries: Vec<IndexEntry>,
}

impl WorkspaceIndex {
    /// The LSP location of an indexed entry.
    fn location_of(&self, entry: &IndexEntry) -> Option<Location> {
        let file = self.files.get(entry.file)?;
        let line_index = LineIndex::new(&file.text);
        Some(Location {
            uri: file.uri.clone(),
            range: line_index.to_range(
                entry.span.start as usize,
                entry.span.end as usize,
                &file.text,
            ),
        })
    }
}

struct Server {
    connection: Connection,
    documents: HashMap<Uri, Document>,
    /// Workspace roots reported by the client at `initialize`.
    workspace_roots: Vec<PathBuf>,
    /// (path -> cached content) so unchanged files are not re-read on every
    /// navigation request.
    fs_cache: HashMap<PathBuf, FsEntry>,
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (connection, io_threads) = Connection::stdio();
    let server = Server {
        connection,
        documents: HashMap::new(),
        workspace_roots: Vec::new(),
        fs_cache: HashMap::new(),
    };
    serve(server)?;
    io_threads.join()?;
    Ok(())
}

fn serve(mut server: Server) -> Result<(), Box<dyn std::error::Error>> {
    let capabilities = server_capabilities();
    let params = server.connection.initialize(serde_json::to_value(capabilities)?)?;
    let params: InitializeParams = serde_json::from_value(params)?;
    server.workspace_roots = workspace_roots(&params);

    loop {
        match server.connection.receiver.recv()? {
            Message::Request(req) => {
                if server.connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                let response = handle_request(&mut server, req);
                server.connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(not) => {
                if let Err(e) = handle_notification(&mut server, not) {
                    eprintln!("scopeql-lsp: notification error: {e}");
                }
            }
            Message::Response(_) => {}
        }
    }
}

/// The workspace root(s) the client reports at `initialize`, in order of
/// preference: workspace folders, then the (deprecated) root URI, then the
/// (deprecated) root path.
#[allow(deprecated)]
fn workspace_roots(params: &InitializeParams) -> Vec<PathBuf> {
    if let Some(folders) = &params.workspace_folders {
        let roots: Vec<PathBuf> = folders
            .iter()
            .filter_map(|f| file_uri_to_path(&f.uri))
            .collect();
        if !roots.is_empty() {
            return roots;
        }
    }
    if let Some(uri) = params.root_uri.as_ref()
        && let Some(path) = file_uri_to_path(uri)
    {
        return vec![path];
    }
    if let Some(path) = params.root_path.as_deref()
        && !path.is_empty()
    {
        return vec![PathBuf::from(path)];
    }
    Vec::new()
}

/// Convert a `file://` URI to a filesystem path. `lsp_types::Uri` (a newtype
/// over `fluent_uri`) does not expose URL helpers, so decode the few forms
/// LSP clients actually send: `file:///abs/path` and `file://localhost/...`.
fn file_uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    let path = if let Some(no_host) = rest.strip_prefix("localhost") {
        no_host
    } else if rest.starts_with('/') {
        rest
    } else {
        // `file://relative` or `file://host/path` forms are not useful here.
        return None;
    };
    Some(PathBuf::from(percent_decode(path)))
}

/// Build a `file://` URI from a filesystem path.
fn path_to_file_uri(path: &Path) -> Option<Uri> {
    use std::str::FromStr;
    let s = path.to_str()?;
    let mut encoded = String::with_capacity(s.len() + 8);
    for &b in s.as_bytes() {
        // Unreserved characters plus `/` and `:` are kept verbatim; anything
        // else (spaces, non-ASCII, `%`, `#`, `?`, ...) is percent-encoded.
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':') {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    Uri::from_str(&format!("file://{encoded}")).ok()
}

/// Percent-decode a URI path component.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The LSP capabilities the server advertises.
fn server_capabilities() -> ServerCapabilities {
    let legend = SemanticTokensLegend {
        token_types: LEGEND_TYPES.to_vec(),
        token_modifiers: LEGEND_MODIFIERS.to_vec(),
    };
    let semantic_tokens = SemanticTokensOptions {
        work_done_progress_options: Default::default(),
        legend,
        range: None,
        full: Some(SemanticTokensFullOptions::Bool(true)),
    };

    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::FULL),
            will_save: None,
            will_save_wait_until: None,
            save: None,
        })),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            semantic_tokens,
        )),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }
}

fn handle_request(server: &mut Server, req: Request) -> Response {
    match req.method.as_str() {
        SemanticTokensFullRequest::METHOD => {
            let (id, params) = extract::<lsp_types::SemanticTokensParams>(
                req,
                SemanticTokensFullRequest::METHOD,
            );
            let result = semantic_tokens_for(server, &params.text_document.uri);
            match result {
                Ok(tokens) => {
                    let value = tokens.map(SemanticTokensResult::Tokens);
                    Response::new_ok(id, value)
                }
                Err(e) => internal_error(id, e),
            }
        }
        HoverRequest::METHOD => {
            let (id, params) = extract::<lsp_types::HoverParams>(req, HoverRequest::METHOD);
            let result = hover_for(server, &params);
            match result {
                Ok(hover) => Response::new_ok(id, hover),
                Err(e) => internal_error(id, e),
            }
        }
        GotoDefinition::METHOD => {
            let (id, params) = extract::<GotoDefinitionParams>(req, GotoDefinition::METHOD);
            match definition_for(server, &params) {
                Ok(result) => Response::new_ok(id, result),
                Err(e) => internal_error(id, e),
            }
        }
        References::METHOD => {
            let (id, params) = extract::<ReferenceParams>(req, References::METHOD);
            match references_for(server, &params) {
                Ok(result) => Response::new_ok(id, result),
                Err(e) => internal_error(id, e),
            }
        }
        _ => Response::new_err(
            req.id,
            lsp_server::ErrorCode::MethodNotFound as i32,
            format!("method not found: {}", req.method),
        ),
    }
}

/// Deserialize `req` as `P`; the method has already been matched to `method`.
fn extract<P: serde::de::DeserializeOwned>(req: Request, method: &str) -> (RequestId, P) {
    let id = req.id.clone();
    match req.extract::<P>(method) {
        Ok((_, params)) => (id, params),
        Err(e) => {
            // A method mismatch cannot happen here because we dispatch on the
            // method first; a JSON error would be a client bug, so fail loudly.
            panic!("scopeql-lsp: failed to extract {method}: {e}")
        }
    }
}

fn internal_error(id: RequestId, e: Box<dyn std::error::Error>) -> Response {
    Response::new_err(id, lsp_server::ErrorCode::InternalError as i32, e.to_string())
}

fn handle_notification(
    server: &mut Server,
    not: Notification,
) -> Result<(), Box<dyn std::error::Error>> {
    match not.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = not.extract::<DidOpenTextDocumentParams>(DidOpenTextDocument::METHOD)?;
            server.documents.entry(params.text_document.uri.clone()).or_default().text =
                params.text_document.text;
            publish_diagnostics(
                server,
                params.text_document.uri,
                Some(params.text_document.version),
            )?;
        }
        DidChangeTextDocument::METHOD => {
            let params =
                not.extract::<DidChangeTextDocumentParams>(DidChangeTextDocument::METHOD)?;
            // Full sync: the client sends the entire document on every change.
            if let Some(change) = params.content_changes.last()
                && let Some(doc) = server.documents.get_mut(&params.text_document.uri)
            {
                doc.text = change.text.clone();
            }
            publish_diagnostics(
                server,
                params.text_document.uri,
                Some(params.text_document.version),
            )?;
        }
        DidCloseTextDocument::METHOD => {
            let params = not.extract::<DidCloseTextDocumentParams>(DidCloseTextDocument::METHOD)?;
            server.documents.remove(&params.text_document.uri);
        }
        _ => {}
    }
    Ok(())
}

fn semantic_tokens_for(
    server: &Server,
    uri: &Uri,
) -> Result<Option<SemanticTokens>, Box<dyn std::error::Error>> {
    let Some(doc) = server.documents.get(uri) else {
        return Ok(None);
    };
    let line_index = LineIndex::new(&doc.text);
    let tokens = encode_tokens(semantic_tokens(&doc.text), &doc.text, &line_index);
    Ok(Some(SemanticTokens {
        result_id: None,
        data: tokens,
    }))
}

/// Encode semantic token infos into the LSP delta stream.
fn encode_tokens(
    infos: Vec<scopeql_lsp::highlight::SemanticTokenInfo>,
    text: &str,
    line_index: &LineIndex,
) -> Vec<SemanticToken> {
    let mut out = Vec::with_capacity(infos.len());
    let (mut last_line, mut last_start) = (0u32, 0u32);

    for info in infos.iter() {
        let start = line_index.to_position(info.start as usize, text);
        let length = utf16_len(&text[info.start as usize..info.end as usize]) as u32;
        let delta_line = start.line - last_line;
        let delta_start = if delta_line == 0 {
            start.character - last_start
        } else {
            start.character
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: info.token_type,
            token_modifiers_bitset: info.modifiers,
        });
        last_line = start.line;
        last_start = start.character;
    }

    out
}

fn hover_for(server: &Server, params: &lsp_types::HoverParams) -> Result<Option<Hover>, Box<dyn std::error::Error>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let Some(doc) = server.documents.get(uri) else {
        return Ok(None);
    };
    let line_index = LineIndex::new(&doc.text);
    let Some(byte) = byte_offset(&line_index, pos, &doc.text) else {
        return Ok(None);
    };

    let (tokens, _) = lexer::lex(&doc.text);
    let Some(tok) = tokens.iter().find(|t| {
        (t.span.start as usize) <= byte && byte < (t.span.end as usize)
    }) else {
        return Ok(None);
    };
    let word = &doc.text[tok.span.start as usize..tok.span.end as usize];

    Ok(Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover_text(tok.kind, word),
        }),
        range: Some(line_index.to_range(tok.span.start as usize, tok.span.end as usize, &doc.text)),
    }))
}

fn hover_text(kind: lexer::TokenKind, word: &str) -> String {
    use lexer::TokenKind as K;
    match kind {
        K::Type => format!("**ScopeQL type** `{word}`"),
        K::Keyword => {
            let hint = match word.to_ascii_lowercase().as_str() {
                "create" => "Create an object (table, view, database, schema, index...).",
                "select" | "from" | "where" | "join" | "insert" | "into" | "update" | "delete"
                | "group" | "order" | "by" | "having" | "limit" | "offset" | "union" | "sample"
                | "window" | "aggregate" | "distinct" => "ScopeQL query clause.",
                _ => "ScopeQL keyword.",
            };
            format!("**`{word}`** — {hint}")
        }
        K::String => "string literal".to_string(),
        K::Number => "numeric literal".to_string(),
        _ => format!("`{word}`"),
    }
}

/// The lower-cased, dot-joined object path under the cursor, if the cursor is
/// on an identifier — e.g. `sales.orders` when the cursor is on `orders` in
/// `FROM sales.orders`. Columns / keyword / literal positions return `None`.
fn path_at(text: &str, position: Position) -> Option<String> {
    let (tokens, _) = lexer::lex(text);
    let line_index = LineIndex::new(text);
    let byte = byte_offset(&line_index, position, text)?;
    let idx = tokens
        .iter()
        .position(|t| t.span.start as usize <= byte && byte < t.span.end as usize)?;
    if tokens[idx].kind != TokenKind::Ident {
        return None;
    }

    let is_dot = |t: &Token, source: &str| {
        t.kind == TokenKind::Operator && &source[t.span.start as usize..t.span.end as usize] == "."
    };

    // Extend left over `ident . ident` so the cursor on `orders` in
    // `sales.orders` still resolves the whole path.
    let mut start = idx;
    while start >= 2 && is_dot(&tokens[start - 1], text) && tokens[start - 2].kind == TokenKind::Ident
    {
        start -= 2;
    }
    let mut end = idx + 1;
    while end + 1 < tokens.len() && is_dot(&tokens[end], text) && tokens[end + 1].kind == TokenKind::Ident
    {
        end += 2;
    }

    let mut parts = Vec::new();
    for t in &tokens[start..end] {
        if t.kind == TokenKind::Ident {
            parts.push(text[t.span.start as usize..t.span.end as usize].to_ascii_lowercase());
        }
    }
    Some(parts.join("."))
}

/// `textDocument/definition`: jump from a table reference (or its own
/// definition) to the matching `CREATE ...` sites.
fn definition_for(
    server: &mut Server,
    params: &GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, Box<dyn std::error::Error>> {
    let uri = params.text_document_position_params.text_document.uri.clone();
    let position = params.text_document_position_params.position;
    let Some(doc) = server.documents.get(&uri) else {
        return Ok(None);
    };
    let Some(path) = path_at(&doc.text, position) else {
        return Ok(None);
    };

    let extra_roots = request_roots(server, &uri);
    let index = server.build_index(&extra_roots);
    let mut locations: Vec<Location> = index
        .entries
        .iter()
        .filter(|e| e.role == ObjectRole::Definition && resolve::names_match(&path, &e.name))
        .filter_map(|e| index.location_of(e))
        .collect();
    sort_locations(&mut locations);
    locations.dedup();

    Ok(match locations.len() {
        0 => None,
        1 => Some(GotoDefinitionResponse::Scalar(locations.remove(0))),
        _ => Some(GotoDefinitionResponse::Array(locations)),
    })
}

/// `textDocument/references`: every mention of the object under the cursor,
/// including its definition(s) and all references across the workspace.
fn references_for(
    server: &mut Server,
    params: &ReferenceParams,
) -> Result<Vec<Location>, Box<dyn std::error::Error>> {
    let uri = params.text_document_position.text_document.uri.clone();
    let position = params.text_document_position.position;
    let Some(doc) = server.documents.get(&uri) else {
        return Ok(Vec::new());
    };
    let Some(path) = path_at(&doc.text, position) else {
        return Ok(Vec::new());
    };

    let extra_roots = request_roots(server, &uri);
    let index = server.build_index(&extra_roots);
    let mut locations: Vec<Location> = index
        .entries
        .iter()
        .filter(|e| resolve::names_match(&path, &e.name))
        .filter_map(|e| index.location_of(e))
        .collect();
    sort_locations(&mut locations);
    locations.dedup();
    Ok(locations)
}

fn sort_locations(locations: &mut [Location]) {
    locations.sort_by(|a, b| {
        a.uri
            .cmp(&b.uri)
            .then_with(|| a.range.start.line.cmp(&b.range.start.line))
            .then_with(|| a.range.start.character.cmp(&b.range.start.character))
            .then_with(|| a.range.end.line.cmp(&b.range.end.line))
            .then_with(|| a.range.end.character.cmp(&b.range.end.character))
    });
}

/// Workspace roots for a request: the client-reported roots, or — when the
/// client sent none — the directory of the document being queried so that
/// navigation still works out of the box.
fn request_roots(server: &Server, uri: &Uri) -> Vec<PathBuf> {
    if !server.workspace_roots.is_empty() {
        return server.workspace_roots.clone();
    }
    if let Some(path) = file_uri_to_path(uri)
        && let Some(parent) = path.parent()
    {
        return vec![parent.to_path_buf()];
    }
    Vec::new()
}

/// Build the workspace navigation index: scan every workspace root for
/// `.scopeql` files (reading only files that changed since last cached),
/// then overlay the open documents, then extract every object-name
/// occurrence.
impl Server {
    fn build_index(&mut self, extra_roots: &[PathBuf]) -> WorkspaceIndex {
        let mut files: Vec<IndexFile> = Vec::new();
        let mut uri_to_file: HashMap<String, usize> = HashMap::new();

        let mut roots = self.workspace_roots.clone();
        roots.extend_from_slice(extra_roots);
        roots.sort();
        roots.dedup();
        for root in roots {
            self.scan_dir(&root, &mut files, &mut uri_to_file);
        }

        // Open documents override on-disk content with the same URI.
        // (Order does not matter: each URI is assigned to exactly one file.)
        for (uri, doc) in &self.documents {
            let text = doc.text.clone();
            if let Some(&file) = uri_to_file.get(uri.as_str()) {
                files[file].text = text;
            } else {
                uri_to_file.insert(uri.as_str().to_string(), files.len());
                files.push(IndexFile { uri: uri.clone(), text });
            }
        }

        let mut entries = Vec::new();
        for (file, source) in files.iter().enumerate() {
            for object in resolve::object_names(&source.text) {
                entries.push(IndexEntry {
                    name: object.name,
                    role: object.role,
                    span: object.span,
                    file,
                });
            }
        }
        WorkspaceIndex { files, entries }
    }

    fn scan_dir(
        &mut self,
        dir: &Path,
        files: &mut Vec<IndexFile>,
        uri_to_file: &mut HashMap<String, usize>,
    ) {
        let Ok(reader) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in reader.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_dir() {
                // Skip VCS, build dirs, virtualenvs and other noise.
                if name.starts_with('.')
                    || matches!(
                        name.as_ref(),
                        "target" | "node_modules" | "dist" | "build" | "vendor" | "__pycache__"
                    )
                {
                    continue;
                }
                self.scan_dir(&path, files, uri_to_file);
            } else if file_type.is_file() && name.ends_with(".scopeql") {
                let Some(text) = self.read_cached(&path) else {
                    continue;
                };
                let Some(uri) = path_to_file_uri(&path) else {
                    continue;
                };
                if !uri_to_file.contains_key(uri.as_str()) {
                    uri_to_file.insert(uri.as_str().to_string(), files.len());
                    files.push(IndexFile { uri, text });
                }
            }
        }
    }

    /// Read a workspace file, reusing the cached content while mtime and size
    /// are unchanged.
    fn read_cached(&mut self, path: &Path) -> Option<String> {
        let metadata = std::fs::metadata(path).ok()?;
        let mtime = metadata.modified().ok();
        let len = metadata.len();
        if let Some(entry) = self.fs_cache.get(path)
            && entry.mtime == mtime
            && entry.len == len
        {
            return Some(entry.text.clone());
        }
        let text = std::fs::read_to_string(path).ok()?;
        self.fs_cache.insert(
            path.to_path_buf(),
            FsEntry {
                mtime,
                len,
                text: text.clone(),
            },
        );
        Some(text)
    }
}

fn byte_offset(line_index: &LineIndex, pos: Position, text: &str) -> Option<usize> {
    let starts = line_index.line_starts();
    let line_start = *starts.get(pos.line as usize)?;
    let slice = &text[line_start..];
    let mut utf16 = 0usize;
    for (i, c) in slice.char_indices() {
        if utf16 >= pos.character as usize {
            return Some(line_start + i);
        }
        utf16 += c.len_utf16();
    }
    Some(text.len())
}

fn publish_diagnostics(
    server: &Server,
    uri: Uri,
    version: Option<i32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(doc) = server.documents.get(&uri) else {
        return Ok(());
    };
    let line_index = LineIndex::new(&doc.text);
    let (_, errors) = lexer::lex(&doc.text);
    let diagnostics = errors
        .iter()
        .map(|(span, message)| LspDiagnostic {
            range: line_index.to_range(span.start as usize, span.end as usize, &doc.text),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("scopeql-lsp".to_string()),
            message: message.clone(),
            ..Default::default()
        })
        .collect();

    let params = PublishDiagnosticsParams {
        uri,
        diagnostics,
        version,
    };
    let method = lsp_types::notification::PublishDiagnostics::METHOD;
    server
        .connection
        .sender
        .send(Message::Notification(Notification::new(
            method.to_string(),
            params,
        )))?;
    Ok(())
}
