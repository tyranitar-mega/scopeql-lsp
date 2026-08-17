//! A self-contained LSP server for the ScopeQL language.
//!
//! This crate provides a language server that powers the vim scopeql plugin.
//! It lexes ScopeQL source with a vendor-local lexer and exposes the resulting
//! tokens through the LSP semantic-token endpoints, plus lexer diagnostics.

pub mod doc;
pub mod highlight;
pub mod lexer;
