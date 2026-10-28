//! `idelink.nzkide.rs` — the IDE's link to the host graph.
//!
//! The pane owns rendering, layout, editing and highlighting; it deliberately
//! owns **no graph**. The host supplies the semantic answers through
//! [`Language`], so the same GUI paints a document with or without a graph
//! (the detached window and the headless evidence path have none).
//!
//! This mirrors the split the Scratch addon uses: `gui.*` paints, `idelink.*`
//! is the pure data/link between the GUI and the host.

use crate::gui_nzkide::{RefRow, SymbolHit};

/// The host's semantic index, as far as the pane needs it.
///
/// Implementations return only real graph facts. An ambiguous token is `None`,
/// never a guessed jump; an unknown node is an empty list, never fabricated
/// rows.
pub trait Language {
    /// Resolve a token under the pointer to one unambiguous node.
    fn symbol_for_token(&self, doc_file: &str, token: &str) -> Option<SymbolHit>;

    /// Every node with an edge pointing at `node_ref`, sorted by site.
    fn references_for(&self, node_ref: &str) -> Vec<RefRow>;
}

/// A host with no graph (detached windows, screenshots) answers nothing.
pub struct NoLanguage;

impl Language for NoLanguage {
    fn symbol_for_token(&self, _doc_file: &str, _token: &str) -> Option<SymbolHit> {
        None
    }

    fn references_for(&self, _node_ref: &str) -> Vec<RefRow> {
        Vec::new()
    }
}
