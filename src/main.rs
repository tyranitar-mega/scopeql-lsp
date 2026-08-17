//! The `scopeql-lsp` language server binary.
//!
//! It speaks the Language Server Protocol over stdio: on startup it advertises
//! semantic-token, hover and diagnostics capabilities, then serves the
//! `textDocument/*` requests for open ScopeQL documents.

use std::collections::HashMap;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as LspNotification,
};
use lsp_types::request::{HoverRequest, Request as _, SemanticTokensFullRequest};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DiagnosticSeverity, Diagnostic as LspDiagnostic, Hover, HoverContents, InitializeParams,
    MarkupContent, MarkupKind, Position, PublishDiagnosticsParams, SemanticToken,
    SemanticTokens, SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions,
    SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions, Uri,
};
use scopeql_lsp::doc::{LineIndex, utf16_len};
use scopeql_lsp::highlight::{LEGEND_MODIFIERS, LEGEND_TYPES, semantic_tokens};
use scopeql_lsp::lexer;

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

struct Server {
    connection: Connection,
    documents: HashMap<Uri, Document>,
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (connection, io_threads) = Connection::stdio();
    let server = Server {
        connection,
        documents: HashMap::new(),
    };
    serve(server)?;
    io_threads.join()?;
    Ok(())
}

fn serve(mut server: Server) -> Result<(), Box<dyn std::error::Error>> {
    let capabilities = server_capabilities();
    let params = server.connection.initialize(serde_json::to_value(capabilities)?)?;
    let _: InitializeParams = serde_json::from_value(params)?;

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
