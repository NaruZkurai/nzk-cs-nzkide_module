//! `nzkide_module` — the VSCode-like IDE pane of cs viewer v3.
//!
//! The crate is split the same way the tokenizer and Scratch addons are: the
//! dotted filenames are the real filenames (`#[path]`), not aliases.
//!
//! | module | file | responsibility |
//! |---|---|---|
//! | [`gui_nzkide`] | `gui.nzkide.rs` | theme, document model, pane, editing, galleys |
//! | [`highlight_nzkide`] | `highlight.nzkide.rs` | C# highlighting over the engine mask |
//! | [`idelink_nzkide`] | `idelink.nzkide.rs` | the host-language link (graph facts in) |
//!
//! The word tokenizer itself is the tokenizer addon
//! (`def2mcpfnnumber::syntax`) so the IDE and Scratch cannot drift.
//!
//! Nothing here depends on the host's `Graph`, `CodeLensView` or chrome. The
//! host builds a [`Language`](idelink_nzkide::Language) adapter and calls the
//! pane; the pane returns requests, never performs a selection.

#[path = "gui.nzkide.rs"]
pub mod gui_nzkide;

#[path = "highlight.nzkide.rs"]
pub mod highlight_nzkide;

#[path = "idelink.nzkide.rs"]
pub mod idelink_nzkide;

/// The shared syntax tokenizer, re-exported so callers of this crate and of
/// Scratch name the same one.
pub use def2mcpfnnumber::syntax as token_nzkide;
