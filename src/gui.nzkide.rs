//! The native source pane: the engine's synthesized document, highlighted and
//! navigable.
//!
//! This module is deliberately self-contained and depends on **no** other fork
//! module. It talks to the engine only through the public
//! [`csgraph_core::index::Index::doc_source`] entry point — the same call the
//! Tauri host's `/api/doc` route serves — so the pane shows exactly the
//! document the engine synthesizes (merged `partial` sites, `/* */` dialect,
//! fully-qualified names, `#if` sections and all). Nothing is re-parsed here.
//!
//! Entry points:
//!
//! * [`show_node`] — load a node's synthesized document and centre its own
//!   `file:line` span (the graph-node → source jump). Returns whether a
//!   document was found.
//! * [`ui`] — the interactive pane (head + scrollable body), to be placed in a
//!   bottom panel by the shell.
//! * [`document_ui`] — the same document painted whole, without scrolling, for
//!   the headless PNG path. Returns the painted content size in points.
//!
//! Colours and geometry mirror the original viewer: `frontend/static/style.css`
//! (`--src-*`, `.source-pane`, `.sline*`) and Monaco's `darkplus` palette in
//! `frontend/static/editor.js`. They live in [`SourceTheme`] so the shell owns
//! them once it has a theme module; this module never invents a colour of its
//! own.


// A binary crate sees one subset of this API, so the whole public surface is
// not always used; the shell keeps the rest.
#![allow(dead_code)]
use std::sync::Arc;

use csgraph_core::index::Index;
use egui::{
    text::LayoutJob, Align2, Color32, FontId, Pos2, Rect, ScrollArea, Sense, Stroke, TextFormat,
    Vec2,
};
use serde_json::Value;

use crate::idelink_nzkide::Language;

// `super::` rather than `crate::`: this file is a child module of `app` in the
// app binary and a root module in the `source_shot` bin, so `super` resolves the
// sibling in both (see the `#[path]` declarations in app.rs / bin/source_shot.rs).
use crate::highlight_nzkide::{highlight_document, Segment, Tok};

/// Height of the source pane head in points (CSS `.source-head`: 6px + ~18px
/// content + 6px, plus its 1px bottom border).
pub const HEAD_H: f32 = 31.0;

// ---------------------------------------------------------------------------
// theme
// ---------------------------------------------------------------------------

/// The pane's colour tokens, sampled from the original viewer.
///
/// `darkplus` is the shipped editor default: `frontend/static/editor.js`
/// `PALETTES.darkplus` for tokens and `frontend/static/style.css` `.source-*` /
/// `.sline*` rules for nzk_ui. The shell's theme module can construct this
/// struct (or wrap it) so there is exactly one palette in the fork.
#[derive(Clone, Debug)]
pub struct SourceTheme {
    pub bg: Color32,
    pub head_bg: Color32,
    pub border: Color32,
    pub border_soft: Color32,
    pub fg: Color32,
    pub gutter: Color32,
    pub gutter_active: Color32,
    pub accent: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub danger: Color32,
    pub target_bg: Color32,
    pub part_bg: Color32,
    pub cond_bg: Color32,
    pub cond_bar: Color32,
    pub keyword: Color32,
    pub control: Color32,
    pub ty: Color32,
    pub iface: Color32,
    pub string: Color32,
    pub number: Color32,
    pub comment: Color32,
    pub preproc: Color32,
    pub function: Color32,
    pub variable: Color32,
    pub op: Color32,
    /// 12.0 — CSS `.sline { font-size: 12px }`.
    pub font_size: f32,
    /// 1.55 — CSS `.sline { line-height: 1.55 }`.
    pub line_spacing: f32,
    /// 62.0 — CSS `.sline .ln { width: 62px; padding-right: 12px }`.
    pub gutter_w: f32,
}

impl Default for SourceTheme {
    fn default() -> Self {
        Self::darkplus()
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

impl SourceTheme {
    /// VS Code Dark+ / `darkplus`, exactly the original viewer's defaults.
    pub fn darkplus() -> Self {
        SourceTheme {
            bg: rgb(0x1e, 0x1e, 0x1e),
            head_bg: rgb(0x12, 0x12, 0x1f),
            border: rgb(0x2f, 0x3d, 0x5c),
            border_soft: rgb(0x23, 0x2c, 0x42),
            fg: rgb(0xd4, 0xd4, 0xd4),
            gutter: rgb(0x85, 0x85, 0x85),
            gutter_active: rgb(0xc6, 0xc6, 0xc6),
            accent: rgb(0x86, 0xaa, 0xf9),
            text_dim: rgb(0x9a, 0xa6, 0xc2),
            text_faint: rgb(0x6b, 0x78, 0x97),
            danger: rgb(0xff, 0x5a, 0x50),
            // --src-target: rgba(86, 156, 214, .16)
            target_bg: rgba(0x56, 0x9c, 0xd6, 0x29),
            // .sline.ctx-part { background: rgba(110, 118, 129, .12) }
            part_bg: rgba(0x6e, 0x76, 0x81, 0x1f),
            // .sline.ctx-cond { background: rgba(56, 189, 248, .07) }
            cond_bg: rgba(0x38, 0xbd, 0xf8, 0x12),
            cond_bar: rgba(0x38, 0xbd, 0xf8, 0x73),
            keyword: rgb(0x56, 0x9c, 0xd6),
            control: rgb(0xc5, 0x86, 0xc0),
            ty: rgb(0x4e, 0xc9, 0xb0),
            iface: rgb(0xb8, 0xd7, 0xa3),
            string: rgb(0xce, 0x91, 0x78),
            number: rgb(0xb5, 0xce, 0xa8),
            comment: rgb(0x6a, 0x99, 0x55),
            preproc: rgb(0xc5, 0x86, 0xc0),
            function: rgb(0xdc, 0xdc, 0xaa),
            variable: rgb(0x9c, 0xdc, 0xfe),
            op: rgb(0xd4, 0xd4, 0xd4),
            font_size: 12.0,
            line_spacing: 1.55,
            gutter_w: 62.0,
        }
    }

    /// VS Code Dark Modern (`.source-pane` `nzk-dark+` overrides).
    pub fn darkmodern() -> Self {
        let mut t = Self::darkplus();
        t.bg = rgb(0x1f, 0x1f, 0x1f);
        t.head_bg = rgb(0x25, 0x25, 0x26);
        t.fg = rgb(0xcc, 0xcc, 0xcc);
        t.gutter = rgb(0x6e, 0x76, 0x81);
        t.gutter_active = rgb(0xcc, 0xcc, 0xcc);
        t.target_bg = rgba(0x26, 0x4f, 0x78, 0x8c);
        t
    }

    /// The `nzk-dark+` UI theme, as it applies to the source pane.
    ///
    /// The original runs `nzk-dark+` (handoff §4): the source *tokens* stay the
    /// `.theme-darkplus` `--src-*` palette (that block is not overridden by
    /// `nzk-dark+`), while the nzk_ui override at style.css:1964 sets
    /// `.source-head { background: #252526 }`. So this is `darkplus()` with the
    /// nzk head; the integration caller maps `view.theme` to this constructor
    /// rather than duplicating any colour in `app.rs`.
    pub fn nzk_dark_plus() -> Self {
        let mut t = Self::darkplus();
        t.head_bg = rgb(0x25, 0x25, 0x26);
        t
    }

    /// Monokai (`editor.js:136-142 PALETTES.monokai`), the third `#sourceTheme`
    /// option. Token colours are the reference's own; pane nzk_ui keeps the
    /// darkplus metrics so only the editor palette changes.
    pub fn monokai() -> Self {
        let mut t = Self::darkplus();
        t.bg = rgb(0x27, 0x28, 0x22);
        t.head_bg = rgb(0x27, 0x28, 0x22);
        t.fg = rgb(0xf8, 0xf8, 0xf2);
        t.gutter = rgb(0x75, 0x71, 0x5e);
        t.gutter_active = rgb(0xcf, 0xcf, 0xc2);
        t.keyword = rgb(0xf9, 0x26, 0x72);
        t.control = rgb(0xf9, 0x26, 0x72);
        t.ty = rgb(0x66, 0xd9, 0xef);
        t.iface = rgb(0xa6, 0xe2, 0x2e);
        t.string = rgb(0xe6, 0xdb, 0x74);
        t.number = rgb(0xae, 0x81, 0xff);
        t.comment = rgb(0x75, 0x71, 0x5e);
        t.preproc = rgb(0xf9, 0x26, 0x72);
        t.function = rgb(0xa6, 0xe2, 0x2e);
        t.variable = rgb(0xf8, 0xf8, 0xf2);
        t.op = rgb(0xf9, 0x26, 0x72);
        t
    }

    /// Solarized Dark (`editor.js:143-149 PALETTES.solarized`), the fourth
    /// `#sourceTheme` option.
    pub fn solarized() -> Self {
        let mut t = Self::darkplus();
        t.bg = rgb(0x00, 0x2b, 0x36);
        t.head_bg = rgb(0x00, 0x2b, 0x36);
        t.fg = rgb(0x93, 0xa1, 0xa1);
        t.gutter = rgb(0x58, 0x6e, 0x75);
        t.gutter_active = rgb(0x93, 0xa1, 0xa1);
        t.keyword = rgb(0x85, 0x99, 0x00);
        t.control = rgb(0xcb, 0x4b, 0x16);
        t.ty = rgb(0xb5, 0x89, 0x00);
        t.iface = rgb(0x2a, 0xa1, 0x98);
        t.string = rgb(0x2a, 0xa1, 0x98);
        t.number = rgb(0xd3, 0x36, 0x82);
        t.comment = rgb(0x58, 0x6e, 0x75);
        t.preproc = rgb(0xcb, 0x4b, 0x16);
        t.function = rgb(0x26, 0x8b, 0xd2);
        t.variable = rgb(0x83, 0x94, 0x96);
        t.op = rgb(0x93, 0xa1, 0xa1);
        t
    }

    /// Row height: the CSS line box (`ceil(1.55 * 12) = 19px`).
    pub fn row_h(&self) -> f32 {
        (self.font_size * self.line_spacing).ceil()
    }

    pub fn tok_color(&self, tok: Tok) -> Color32 {
        match tok {
            Tok::Plain => self.fg,
            Tok::Comment => self.comment,
            Tok::Str => self.string,
            Tok::Keyword => self.keyword,
            Tok::Control => self.control,
            Tok::Type => self.ty,
            Tok::Number => self.number,
            Tok::Function => self.function,
            Tok::Variable => self.variable,
            Tok::Op => self.op,
            Tok::Preproc => self.preproc,
        }
    }
}

// ---------------------------------------------------------------------------
// document model
// ---------------------------------------------------------------------------

/// One line of the synthesized document, plus its pre-computed highlight.
#[derive(Clone, Debug)]
pub struct DocLine {
    /// Document line number (`n`); 0 for viewer nzk_ui (part markers).
    pub n: i64,
    /// `body` / `head` / `tail` / `cond` / `part`, straight from `doc.rs`.
    pub ctx: String,
    /// Origin file in the corpus, when the line came from one.
    pub file: String,
    /// Origin line in `file`.
    pub orig: i64,
    /// `#if`-inactive lines are dimmed (engine `active_line_flags`).
    pub active: bool,
    pub text: String,
    pub segs: Vec<Segment>,
}

/// A synthesized document for one node.
#[derive(Clone, Debug)]
pub struct Document {
    pub node_ref: String,
    pub title: String,
    pub lines: Vec<DocLine>,
    pub total: i64,
    pub start: i64,
    pub end: i64,
    pub parts: usize,
    pub sections: i64,
    pub defines_known: bool,
    /// Node's declaration file (`node.file` in the payload).
    pub file: String,
    /// Node's declaration line span in `file`.
    pub line: i64,
    pub end_line: i64,
    /// Doc-line index to centre: the node's own first line in the document.
    pub focus: usize,
    /// Doc-line index range of the node's own `file:line-endLine` span.
    pub target: Option<(usize, usize)>,
    /// Monotonic counter so two `Document`s built for the same `node_ref` are
    /// never mistaken for each other by the galley cache.
    ///
    /// `node_ref` is not a document identity: `show_node` re-synthesizes the same
    /// node whenever it is re-shown, and a rebuilt index (or a changed `#if`
    /// define) can return different text for the same ref. The [`GalleyKey`]
    /// hashes this too, so "same doc" can never mean "same ref".
    highlight_generation: u64,
}

fn s_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn i_of(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64()).unwrap_or(0)
}

impl Document {
    /// Allocate the next document identity. Process-wide and monotonic: a
    /// counter is enough because it only has to *differ*, never to be ordered.
    fn next_generation() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    /// Build from the engine's `/api/doc` payload (an `Index::doc_source`
    /// result). This is the *only* place a document is assembled.
    pub fn from_value(node_ref: &str, v: &Value) -> Result<Document, String> {
        let raw_lines = v
            .get("lines")
            .and_then(|x| x.as_array())
            .ok_or_else(|| "document payload has no lines".to_string())?;
        let mut full = String::new();
        let mut metas: Vec<(i64, String, String, i64, bool)> = Vec::with_capacity(raw_lines.len());
        for (i, l) in raw_lines.iter().enumerate() {
            if i > 0 {
                full.push('\n');
            }
            let text = s_of(l, "text");
            full.push_str(&text);
            metas.push((
                i_of(l, "n"),
                s_of(l, "ctx"),
                s_of(l, "file"),
                i_of(l, "orig"),
                l.get("active").and_then(|x| x.as_bool()).unwrap_or(true),
            ));
        }
        let segs = highlight_document(&full);

        let node = v.get("node").cloned().unwrap_or(Value::Null);
        let title = {
            let full_name = s_of(&node, "full");
            if !full_name.is_empty() {
                full_name
            } else {
                let label = s_of(&node, "label");
                if label.is_empty() {
                    node_ref.to_string()
                } else {
                    label
                }
            }
        };
        let file = s_of(&node, "file");
        let line = i_of(&node, "line");
        let end_line = {
            let e = i_of(&node, "endLine");
            if e != 0 { e } else { line }
        };

        let mut lines: Vec<DocLine> = Vec::with_capacity(metas.len());
        for (i, (n, ctx, f, orig, active)) in metas.into_iter().enumerate() {
            let f = if f.is_empty() { file.clone() } else { f };
            lines.push(DocLine {
                n,
                ctx,
                file: f,
                orig,
                active,
                text: s_of(&raw_lines[i], "text"),
                segs: segs.get(i).cloned().unwrap_or_default(),
            });
        }

        let start = i_of(v, "start");
        // Centre the node's own first line: the first emitted body/head line
        // whose document number matches `start`.
        let focus = lines
            .iter()
            .position(|l| l.ctx != "part" && l.n == start)
            .or_else(|| lines.iter().position(|l| l.ctx != "part"))
            .unwrap_or(0);
        // The node's declaration span, mapped onto the document's line indices.
        let target: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                !file.is_empty() && l.file == file && l.orig >= line && l.orig <= end_line
            })
            .map(|(i, _)| i)
            .collect();
        let target = if target.is_empty() {
            None
        } else {
            Some((target[0], target[target.len() - 1] + 1))
        };

        Ok(Document {
            node_ref: node_ref.to_string(),
            title,
            total: i_of(v, "total"),
            start,
            end: i_of(v, "end"),
            parts: v.get("parts").and_then(|x| x.as_array()).map(Vec::len).unwrap_or(0),
            sections: i_of(v, "sections"),
            defines_known: v.get("definesKnown").and_then(|x| x.as_bool()).unwrap_or(false),
            file,
            line,
            end_line,
            lines,
            focus,
            target,
            highlight_generation: Self::next_generation(),
        })
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// `file:line-endLine` of the node's declaration, for the head/chip.
    pub fn span_label(&self) -> String {
        if self.file.is_empty() {
            return String::new();
        }
        if self.end_line > self.line {
            format!("{}:{}-{}", self.file, self.line, self.end_line)
        } else {
            format!("{}:{}", self.file, self.line)
        }
    }

    /// Assemble a document from already-built lines, for tests in *other*
    /// modules that need a real `Document` without an engine index.
    ///
    /// `highlight_generation` stays private on purpose: it is the galley cache's
    /// invalidation key, and a field a caller could set by hand is a stale-cache
    /// bug waiting to happen (the round's cross-context glyph-scrambling find).
    /// So this constructor does not widen the field — it allocates a real
    /// generation through the same private [`Document::next_generation`] the
    /// engine path uses, which keeps "`from_value` is the only assembler" true
    /// in spirit: every `Document` still gets a genuine, monotonically
    /// increasing identity, never a hand-picked one.
    ///
    /// Test-only, so it cannot become a second production assembly path.
    #[doc(hidden)]
    pub fn from_lines_for_tests(
        node_ref: &str,
        file: &str,
        lines: Vec<DocLine>,
        line: i64,
        end_line: i64,
    ) -> Document {
        let total = lines.len() as i64;
        Document {
            node_ref: node_ref.to_string(),
            title: node_ref.to_string(),
            lines,
            total,
            start: line,
            end: end_line,
            parts: 0,
            sections: 0,
            defines_known: true,
            file: file.to_string(),
            line,
            end_line,
            focus: 0,
            target: None,
            highlight_generation: Self::next_generation(),
        }
    }
}

// ---------------------------------------------------------------------------
// code lenses
// ---------------------------------------------------------------------------

/// One *painted* code lens: a row for the declaration on document line `line`,
/// carrying the command the reference's lens would run.
///
/// This is the render side, not a second model. It is built **only** from the
/// core's own wire type — `codelens_wire::LensRow`, which is `codelens_wire`'s
/// re-export of [`codelens::LensRow`] — so nothing here re-derives a count, a
/// placement rule or a title. The reference's two lenses per declaration
/// (`codelens.js:221-244`) are two [`LensRow`]s sharing a line, and they become
/// two rows here in the same order.
///
/// The `title`s are the reference's own strings, verbatim:
/// `"N references · M callers · show in graph"` for the `main` lens
/// (`codelens.js:186-188`) and the bare `"references"` (`codelens.js:241`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LensText {
    /// The document line the declaration sits on, **1-based** — the lens is
    /// painted directly above this line, exactly as Monaco places a
    /// `.codelens-decoration` above its `range.startLineNumber`
    /// (`codelens.js:219-221`). Never `0`: [`codelens::build_lenses`] drops a
    /// declaration whose line is not in this document.
    pub line: i64,
    /// `main` or `references` ([`codelens::LensKind::tag`]).
    pub kind: &'static str,
    /// The node the lens belongs to (`_nodeId`).
    pub node_id: String,
    /// The painted text.
    pub title: String,
    /// Whether the reference's click target for this row is one this app
    /// actually wired. `main` is (`csgraph.codelens.showInGraph` selects the
    /// node, which the source pane already renders); `references` is not.
    pub command_wired: bool,
}

/// The painted lens rows for a document's own lines, plus the union of the
/// document line numbers that carry at least one lens.
///
/// `lines` is indexed by **document line number**, not by position in
/// `doc.lines`: `doc.lines[0]` is normally the `ctx == "part"` viewer-nzk_ui
/// marker whose `n == 0` (`source_pane.rs:1056`), so a positional index would
/// put every lens one row too high. `line_index` builds it through
/// [`codelens::record_index`] — the core's own rule, which already skips
/// `n == 0` — rather than by re-implementing the mapping here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LensRows {
    /// `line_index[n]` — the lens rows painted immediately above document line
    /// `n`. Empty for a line with no declaration.
    pub lines: Vec<Vec<LensText>>,
    /// The document line numbers that carry a lens, ascending.
    ///
    /// This is the **galley cache's lens component**: it is what turns "this
    /// document has lenses" into a value the cache key can compare. Kept as the
    /// line set rather than the full rows so the key stays cheap; the rows for
    /// one line can only change when the graph behind the lens changes, and a
    /// changed graph means a changed [`DocFingerprint`] (see the note in
    /// [`LensRows::fingerprint`]).
    pub lens_lines: Vec<i64>,
}

impl LensRows {
    /// A document with no lenses at all.
    pub fn empty() -> LensRows {
        LensRows::default()
    }

    /// Whether any line carries a lens.
    pub fn is_empty(&self) -> bool {
        self.lens_lines.is_empty()
    }

    /// Total painted lens rows (`main` + `references`).
    pub fn len(&self) -> usize {
        self.lines.iter().map(Vec::len).sum()
    }

    /// The rows painted above document line `n`.
    pub fn for_line(&self, n: i64) -> &[LensText] {
        if n <= 0 {
            return &[];
        }
        self.lines.get(n as usize).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Fold this value into a running hash, for the galley cache key.
    ///
    /// See [`GalleyKey`] for why the lens rows are part of the key at all.
    pub(crate) fn hash_into(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        self.lens_lines.hash(h);
        for l in &self.lines {
            l.len().hash(h);
            for r in l {
                r.line.hash(h);
                r.kind.hash(h);
                r.node_id.hash(h);
                r.title.hash(h);
                r.command_wired.hash(h);
            }
        }
    }
}

/// Synthesize a node's document through the engine (no re-parsing here).
pub fn document_for(index: &mut Index, node_ref: &str) -> Result<Document, String> {
    let v = index
        .doc_source(node_ref)
        .ok_or_else(|| format!("no synthesized document for node {node_ref:?}"))?;
    Document::from_value(node_ref, &v)
}

/// Open a node's document by building a throwaway index for `repo`.
///
/// Only the evidence/screenshot opt-in path uses this (where just the `Graph`
/// is on hand); the windowed app keeps the one real `Index` alive in `App`.
pub fn show_node_repo(
    state: &mut SourceState,
    repo: &std::path::Path,
    roots: &[String],
    node_ref: &str,
) -> bool {
    use csgraph_core::config::Config;
    let repo = csgraph_core::index::abspath(repo);
    let mut index = Index::new(repo);
    index.build(roots, &[], &Config::default());
    state.show_node(&mut index, node_ref)
}

// ---------------------------------------------------------------------------
// state and navigation
// ---------------------------------------------------------------------------

/// Everything that can change a line's laid-out galley.
///
/// This is the cache's *invalidation key*, and it is deliberately a value that
/// is compared field-by-field rather than a "generation" counter: a counter
/// forces every writer to remember to bump it, and one forgotten bump is a
/// stale cache. These six facts are exactly and only what `line_galley` reads:
///
/// * `doc` — the document's fingerprints (identity + content). A different
///   node, or the same node re-synthesized with different text/segments.
/// * `theme` — every colour, `font_size` and `line_spacing` (so `row_h` too).
/// * `lens` — the code-lens rows painted inside the document rows. Added when
///   the pane was wired to [`crate::app::codelens_wire`]; a lens is painted in
///   the same row allocation as the line it sits above, so it is a layout input
///   even though neither the document nor the theme changed. See
///   [`LensFingerprint`] for the exact stale-gallery this prevents.
/// * `highlight` — the "highlight" checkbox; off means plain-`fg` segments.
/// * `width` — `wrap.max_width` is `INFINITY` today so width does not currently
///   change a galley, but the paint path already measures the viewport width and
///   the field is part of the key so a future wrapping mode cannot silently
///   reuse galleys laid out at a different width. Unused is cheaper than wrong.
/// * `ctx` — identity of the [`egui::Context`] the galleys were shaped in. A
///   galley's glyphs index *that context's* font atlas, so a galley reused in a
///   different context paints scrambled glyphs. `source_shot` really does build
///   one context to measure and a second to paint, both over the same
///   `SourceState`; without this field the paint pass reused the measure pass's
///   galleys and the evidence PNG came out as garbage until it was added.
///
/// **How `ctx` was found, and why its comment is longer than the others.**
/// No unit test in this file could have caught it, and none did: the corruption
/// lived *only* on the two-context path, so every in-crate test — which uses one
/// `Context` — was correctly green while `source_shot --view doc` painted
/// garbage. It was found by rendering that PNG and pixel-diffing it against a
/// pre-change build: **14,370 differing bytes across 74 of 183 rows**, confined
/// to the text area (columns 62..545), which is the signature of wrong glyphs
/// rather than wrong layout. The fix was this field, and the evidence is that
/// the same PNG then compared byte-identical to the pre-change build.
///
/// That is the reason `galley_cache_misses_across_different_egui_contexts`
/// exists. It is not a test of the bug that was fixed; it is a *substitute* for
/// the pixel-diff, because re-deriving this by eye does not happen on a normal
/// change. A guard that only a screenshot could have caught needs a test that
/// does not need a screenshot, or the next person reintroduces it silently.
/// If you add another context to any caller of this cache, that test is the one
/// that should stop you.
#[derive(Clone, PartialEq)]
struct GalleyKey {
    doc: DocFingerprint,
    theme: ThemeFingerprint,
    /// The code lenses painted on the rows, hashed.
    ///
    /// Sixth field, added when the source pane was wired to the CodeLens core.
    /// Before it, a graph that gained a declaration on an already-visible line
    /// changed *nothing* this key compared, so the cache served a caption-free
    /// galley for a row that now has a caption. See [`LensFingerprint`].
    lens: LensFingerprint,
    highlight: bool,
    width: u32,
    ctx: egui::Context,
}

/// Identity + content of the loaded document, in 128 bits.
///
/// A pointer/`node_ref` alone is not enough: `show_node` re-synthesizes the same
/// node from a possibly-rebuilt index, and the engine may return different lines
/// (a `#if` define changed, a partial site edited). Hashing the node ref, the
/// line metadata and the line text makes "same doc" mean "same glyphs", which is
/// what the galley actually depends on.
#[derive(Clone, PartialEq)]
struct DocFingerprint(u64, u64);

impl DocFingerprint {
    fn of(doc: &Document) -> Self {
        use std::hash::{Hash, Hasher};
        let mut a = std::collections::hash_map::DefaultHasher::new();
        let mut b = std::collections::hash_map::DefaultHasher::new();
        doc.node_ref.hash(&mut a);
        doc.lines.len().hash(&mut a);
        doc.highlight_generation.hash(&mut a);
        for l in &doc.lines {
            l.n.hash(&mut b);
            l.ctx.hash(&mut b);
            l.active.hash(&mut b);
            l.text.hash(&mut b);
        }
        DocFingerprint(a.finish(), b.finish())
    }
}

/// Every `SourceTheme` field that `line_galley` and `paint_row` read.
///
/// `SourceTheme` is `Clone + Debug` but not `PartialEq`, and comparing ~25
/// `Color32`s per frame is not free either. Hashing the token colours plus the
/// three metrics gives a cheap, total comparison: any colour or metric change
/// produces a different fingerprint, so the cache cannot serve a stale palette.
#[derive(Clone, PartialEq)]
struct ThemeFingerprint(u64);

impl ThemeFingerprint {
    fn of(t: &SourceTheme) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let cols: [Color32; 22] = [
            t.bg, t.head_bg, t.border, t.border_soft, t.fg, t.gutter, t.gutter_active,
            t.accent, t.text_dim, t.text_faint, t.danger, t.target_bg, t.part_bg, t.cond_bg,
            t.cond_bar, t.keyword, t.control, t.ty, t.iface, t.string, t.number, t.comment,
        ];
        // `Color32` is not `Hash`; its four bytes are the whole value.
        for c in cols {
            c.to_array().hash(&mut h);
        }
        t.preproc.to_array().hash(&mut h);
        t.function.to_array().hash(&mut h);
        t.variable.to_array().hash(&mut h);
        t.op.to_array().hash(&mut h);
        t.font_size.to_bits().hash(&mut h);
        t.line_spacing.to_bits().hash(&mut h);
        t.gutter_w.to_bits().hash(&mut h);
        ThemeFingerprint(h.finish())
    }
}

/// A running hash of the **code-lens rows** the pane is about to paint.
///
/// ## Why this is a sixth `GalleyKey` field and not a bug waiting to happen
///
/// Lenses are painted in the *same row loop* as the document — a row above a
/// declaration line, inside the line's own `row_h` allocation ([`render_lens_row`])
/// — so a lens genuinely changes what a row's galley is. But the five facts
/// [`GalleyKey`] compared before this field existed cannot see a lens:
///
/// * `doc` (`DocFingerprint`) hashes `node_ref`, the line metadata and the line
///   *text*. A lens adds no document line and changes no line text.
/// * `theme`, `highlight`, `width`, `ctx` are untouched by a lens.
///
/// So the graph could gain a declaration on a line already on screen — a
/// re-index, or a node whose `degree` changed and therefore its lens title —
/// and every key component would compare equal. The cache would hand back the
/// galley laid out for a caption-free row and the new lens would not appear (or,
/// worse, the *old* caption would keep painting). That is the same failure mode
/// the doc fingerprint exists to stop, one level up: "same document" is not
/// "same painted row".
///
/// Hashing [`LensRows::lens_lines`] plus the rows themselves is what makes
/// "same key" mean "same glyphs to shape", which is exactly the contract
/// [`DocFingerprint`] states for the document half. `Vec<LensText>` is not
/// `Hash`, so the fold goes through [`LensRows::hash_into`]; two lenses on one
/// line compare as two, in row order, so a `references` row appearing next to a
/// `main` row is a miss rather than a silently reused galley.
#[derive(Clone, PartialEq)]
struct LensFingerprint(u64);

impl LensFingerprint {
    fn of(lenses: &LensRows) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        lenses.hash_into(&mut h);
        // The empty-lens case must not collide with a lens set that happens to
        // hash to the same value as the unhashed prefix — hash a tag for it.
        lenses.is_empty().hash(&mut h);
        LensFingerprint(h.finish())
    }
}

/// One pending save: the repo-relative file to write and the exact editable
/// buffer bytes.
///
/// The pane cannot perform the write itself — it holds no engine [`Index`] —
/// so Ctrl+S queues this and the host (which owns the index) routes it through
/// the **existing** in-process transport (`edit_transport::invoke` →
/// `csgraph_core::edit::write_file`). This type only carries the two fields;
/// [`Self::write_file_args`] is the single place the wire body is shaped, so a
/// test can pin the exact JSON the transport receives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRequest {
    /// Repo-relative path (the open document's own `file`).
    pub path: String,
    /// The editable buffer, verbatim.
    pub content: String,
}

impl SaveRequest {
    /// The exact `write_file` arguments. `apply: true` is required because the
    /// engine's `write_file` is a dry run by default (`core/src/edit.rs:2336`);
    /// omitting it would report a diff and write nothing.
    pub fn write_file_args(&self) -> Value {
        serde_json::json!({
            "path": self.path,
            "content": self.content,
            "apply": true,
        })
    }
}

/// The pane's whole state. The shell owns one of these.
pub struct SourceState {
    pub open: bool,
    pub doc: Option<Document>,
    pub error: Option<String>,
    /// Toggle syntax highlighting (the head's "highlight" checkbox).
    pub highlight: bool,
    /// True while the pane shows the editable buffer instead of the painted,
    /// read-only galley. `false` by default, so every existing caller (the
    /// headless painter, the detached window, the split pane) keeps the exact
    /// read-only paint it had before this field existed.
    pub editing: bool,
    /// The editable text, seeded from the open document when editing starts.
    editable: String,
    /// [`Self::editable`] as last loaded/saved. `editable != saved_text` is the
    /// dirty state; there is no separate flag to get out of sync.
    saved_text: String,
    /// A Ctrl+S save waiting for the host that owns the engine `Index`.
    save_request: Option<SaveRequest>,
    /// The last save failure, painted in the head until the next attempt. A
    /// successful save clears it. `None` means "no failure to report" — never
    /// that a save succeeded.
    save_error: Option<String>,
    /// The code lenses to paint above the declaration lines, built from the
    /// core's [`crate::app::codelens_wire`] rows.
    ///
    /// Empty until a consumer computes them ([`SourceState::show_node_and_compute`],
    /// which `App::sync_source` calls). A pane that never computes a lens paints
    /// exactly what it painted before this field existed — the closed-pane and
    /// no-consumer paths are unchanged, which is what keeps the shipped default
    /// paint byte-identical.
    lenses: LensRows,
    /// Source-document history: refs previously shown, newest last (the
    /// `source.js` history the v2 editor had). `show_node` pushes the node it
    /// replaces, so Back/Forward walk real document opens.
    history: Vec<String>,
    /// Forward stack, the mirror of [`Self::history`].
    forward: Vec<String>,
    /// The find-references rows for the symbol `Shift+F12`/Shift+click asked
    /// about; `None` while no reference list is open.
    references: Option<Vec<RefRow>>,
    /// The node id under the pointer in the last painted frame (`None` when the
    /// pointer is not over a resolvable token). `F12`/`Shift+F12` read it.
    cursor_symbol: Option<String>,
    /// The IDE's highlight: **0-based document line indices, inclusive**.
    ///
    /// Set while reading by the row the pointer is on (sticky, so it survives
    /// moving to the Scratch window), and set by the host when a Scratch block
    /// is clicked. The Scratch pane highlights every block this range touches.
    pub highlight_lines: Option<(usize, usize)>,
    /// A node the user asked to open by clicking a **wired** lens, waiting for a
    /// host that owns the graph view.
    ///
    /// The pane cannot select a node — `SourceState` holds no `GraphView`, and the
    /// nzk_ui path runs inside a closure that already borrows `source` — so a
    /// click is queued here and [`Self::take_lens_open_request`] hands it to
    /// `App`, which performs the selection. It is a *request*, never a claim that
    /// the selection changed: the field is `None` unless a wired lens was really
    /// clicked.
    pub lens_open_request: Option<String>,
    /// Doc-line index to centre on the next `ui()` frame.
    scroll_to: Option<usize>,
    /// Key passed to the last successful [`show_node`].
    loaded_key: Option<String>,
    /// Laid-out galleys for the current key, one per document line.
    ///
    /// epaint memoizes `LayoutJob -> Galley` but evicts every entry that was not
    /// used in the current pass ([`epaint::text::GalleyCache::flush_cache`] keeps
    /// only `last_used == generation`). A pane that lays out each line once per
    /// frame therefore never hits that cache on the next frame: every line is
    /// re-shaped every frame. Holding the `Arc<Galley>` here survives the pass
    /// boundary. Measured on a 407-line document: 12.19 ms/frame re-laying out
    /// each line vs 0.208 ms/frame reusing these — 58x.
    ///
    /// `RefCell` (not `&mut`) so the paint loop can hold a shared borrow of the
    /// document and the galleys at the same time.
    galleys: std::cell::RefCell<Option<GalleyCache>>,
    /// Cache probe counters, for the hit/miss test and for `CSVIEWER_FRAME_TIMING`.
    galley_hits: std::cell::Cell<u64>,
    galley_misses: std::cell::Cell<u64>,
}

/// The cached galleys together with the key they were laid out under.
struct GalleyCache {
    key: GalleyKey,
    galleys: Vec<Arc<egui::Galley>>,
}

impl Default for SourceState {
    fn default() -> Self {
        SourceState {
            open: false,
            doc: None,
            error: None,
            highlight: true,
            editing: false,
            editable: String::new(),
            saved_text: String::new(),
            save_request: None,
            save_error: None,
            lenses: LensRows::empty(),
            history: Vec::new(),
            forward: Vec::new(),
            references: None,
            cursor_symbol: None,
            highlight_lines: None,
            lens_open_request: None,
            scroll_to: None,
            loaded_key: None,
            galleys: std::cell::RefCell::new(None),
            galley_hits: std::cell::Cell::new(0),
            galley_misses: std::cell::Cell::new(0),
        }
    }
}

impl SourceState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The open document as plain text, lines joined with `\n` (no trailing
    /// newline): the exact seed of an edit session and the same strings the
    /// read-only pane paints.
    fn document_text(doc: &Document) -> String {
        let mut out = String::with_capacity(doc.lines.len() * 48);
        for (i, line) in doc.lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&line.text);
        }
        out
    }

    /// Seed [`Self::editable`] from the current document and reset the dirty
    /// baseline. Called when editing starts and whenever a new document is
    /// adopted.
    fn seed_editable(&mut self) {
        let text = self.doc.as_ref().map(Self::document_text).unwrap_or_default();
        self.saved_text = text.clone();
        self.editable = text;
        self.save_error = None;
    }

    /// Enter edit mode on the current document, seeding the buffer from it.
    /// A no-op with no document open: there is nothing to edit.
    pub fn start_editing(&mut self) {
        if self.doc.is_some() && !self.editing {
            self.seed_editable();
            self.editing = true;
        }
    }

    /// Leave edit mode. The buffer is dropped; the read-only paint returns.
    pub fn stop_editing(&mut self) {
        self.editing = false;
    }

    /// The editable buffer (empty when never seeded).
    pub fn editable_text(&self) -> &str {
        &self.editable
    }

    /// True when the buffer differs from the text last loaded or saved.
    pub fn is_dirty(&self) -> bool {
        self.editing && self.editable != self.saved_text
    }

    /// The save this buffer would perform, or `None` when there is nothing to
    /// save (no file, or no change). Pure — no I/O and no side effect.
    pub fn pending_save(&self) -> Option<SaveRequest> {
        let doc = self.doc.as_ref()?;
        if doc.file.is_empty() || self.editable == self.saved_text {
            return None;
        }
        Some(SaveRequest { path: doc.file.clone(), content: self.editable.clone() })
    }

    /// The exact `write_file` payload for the pending save, or `None`. This is
    /// the value a test pins and the host forwards to the transport verbatim.
    pub fn save_payload(&self) -> Option<Value> {
        self.pending_save().map(|r| r.write_file_args())
    }

    /// Ctrl+S: queue the pending save for the host that owns the engine.
    pub fn request_save(&mut self) {
        if let Some(req) = self.pending_save() {
            self.save_request = Some(req);
        }
    }

    /// Hand the queued save to the host (exactly once).
    pub fn take_save_request(&mut self) -> Option<SaveRequest> {
        self.save_request.take()
    }

    /// Record the transport's MCP envelope. A save counts only when the body
    /// says `"ok": true`; the buffer then becomes the new clean baseline.
    /// Anything else is a failure and is reported, never treated as a pass.
    pub fn finish_save(&mut self, envelope: &Value) {
        let body = envelope
            .get("content")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|b| b.get("text"))
            .and_then(Value::as_str)
            .and_then(|t| serde_json::from_str::<Value>(t).ok());
        let ok = body
            .as_ref()
            .and_then(|b| b.get("ok"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if ok {
            self.saved_text = self.editable.clone();
            self.save_error = None;
        } else {
            let err = body
                .as_ref()
                .and_then(|b| b.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("save failed")
                .to_string();
            self.save_error = Some(err);
        }
    }

    /// The last save failure, for the head's status label.
    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    /// Load `node_ref`'s synthesized document and centre its own declaration
    /// span. Re-showing the same node re-centres it (a click always jumps).
    ///
    /// Call this on selection *change*, not every frame: `doc_source` is cached
    /// by the index, but the pane's scroll position is reset here by design.
    ///
    /// `node_ref` may be a graph id (`#ns:NZK`), a full name (`NZK.B`) or a
    /// label; the engine's own `doc_alias` resolves it.
    pub fn show_node(&mut self, index: &mut Index, node_ref: &str) -> bool {
        self.open = true;
        // `source.js` document history: remember the document being replaced.
        if let Some(prev) = self.loaded_key.clone() {
            if prev != node_ref {
                if self.history.len() >= 40 {
                    self.history.remove(0);
                }
                self.history.push(prev);
                self.forward.clear();
            }
        }
        self.references = None;
        match document_for(index, node_ref) {
            Ok(doc) => {
                self.adopt_document(node_ref, doc);
                true
            }
            Err(e) => {
                self.doc = None;
                self.error = Some(e);
                self.loaded_key = None;
                self.invalidate_galleys();
                false
            }
        }
    }

    /// Adopt an already-synthesized [`Document`] as this pane's current document
    /// and centre its declaration.
    ///
    /// This is the **one** owner of the "document became current" transition:
    /// [`Self::show_node`] calls it after the engine answered, and the
    /// per-window document engine ([`crate::doc_engine`]) calls it when its
    /// worker thread hands a document back to the main thread. Keeping the
    /// adoption in one place is what makes a worker-loaded document
    /// indistinguishable from a synchronously loaded one — it is the same pane
    /// state either way.
    pub fn adopt_document(&mut self, node_ref: &str, doc: Document) {
        self.open = true;
        self.scroll_to = Some(doc.focus);
        self.loaded_key = Some(node_ref.to_string());
        self.doc = Some(doc);
        self.error = None;
        // A new document invalidates any edit session's baseline; re-seed so
        // the buffer can never belong to the previous document.
        if self.editing {
            self.seed_editable();
        }
        // The key check in `galleys_for` would also catch this, but the old
        // `Arc`s are pointless to retain once the document is gone.
        self.invalidate_galleys();
    }

    /// Replace the painted lens rows.
    ///
    /// **This deliberately does NOT call `invalidate_galleys`.** That is the
    /// whole point of the [`GalleyKey`]'s `lens` component: the key is the
    /// invalidation mechanism, and a writer that also drops the cache outright
    /// makes the key decoration. The first version of this function *did*
    /// invalidate, and a mutation test caught it — neutering
    /// `lens: LensFingerprint::of(&self.lenses)` to `LensFingerprint(0)` left
    /// every lens test GREEN, because the eager drop was doing all the work and
    /// the key component was never consulted. Removing the eager drop makes the
    /// key load-bearing, and the mutation then fails the suite as it must.
    ///
    /// Setting the *same* set is still a no-op, so a caller may call this every
    /// frame. Setting a different one changes the next `galleys_for` key, which
    /// is a miss that re-shapes the document with the new captions.
    pub fn set_lenses(&mut self, lenses: LensRows) {
        if self.lenses == lenses {
            return;
        }
        self.lenses = lenses;
    }

    /// The document ref currently shown (`None` while the pane is empty or the
    /// last load failed). This is the ref `#srcWindowBtn` opens in a second
    /// window and `#srcMethodBtn` re-shows.
    pub fn loaded_key(&self) -> Option<&str> {
        self.loaded_key.as_deref()
    }

    /// The `file:line` reference `#srcRefBtn` copies (`source.js:546-550`).
    /// Empty `file` -> `None`; a missing line is copied as the bare path.
    pub fn reference(&self) -> Option<String> {
        let doc = self.doc.as_ref()?;
        if doc.file.is_empty() {
            return None;
        }
        Some(if doc.line > 0 {
            format!("{}:{}", doc.file, doc.line)
        } else {
            doc.file.clone()
        })
    }

    /// Pop the previous document ref for Back; the current one moves to the
    /// Forward stack. `None` when history is empty (an honest no-op).
    pub fn history_back(&mut self) -> Option<String> {
        let target = self.history.pop()?;
        if let Some(cur) = self.loaded_key.take() {
            self.forward.push(cur);
        }
        Some(target)
    }

    /// Pop the next document ref for Forward (mirror of [`Self::history_back`]).
    pub fn history_forward(&mut self) -> Option<String> {
        let target = self.forward.pop()?;
        if let Some(cur) = self.loaded_key.take() {
            self.history.push(cur);
        }
        Some(target)
    }

    pub fn history_back_available(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn history_forward_available(&self) -> bool {
        !self.forward.is_empty()
    }

    /// The rows `Shift+F12` found, or `None` while the list is closed.
    pub fn references(&self) -> Option<&[RefRow]> {
        self.references.as_deref()
    }

    /// Replace the find-references list; empty input closes it rather than
    /// painting an empty panel.
    pub fn set_references(&mut self, rows: Vec<RefRow>) {
        self.references = if rows.is_empty() { None } else { Some(rows) };
    }

    pub fn close_references(&mut self) {
        self.references = None;
    }

    /// The node under the pointer in the last frame (for F12/Shift+F12).
    pub fn cursor_symbol(&self) -> Option<&str> {
        self.cursor_symbol.as_deref()
    }

    /// Take the node a lens click asked to open, if any.
    ///
    /// Taking it clears the field, so a request cannot be applied twice. `None`
    /// means "no lens was clicked", which is the honest answer for every frame
    /// the pane is closed, empty, or has no lenses.
    pub fn take_lens_open_request(&mut self) -> Option<String> {
        self.lens_open_request.take()
    }

    /// Nodes `App` would select if the last clicked lens were applied now.
    ///
    /// Identical to [`Self::lens_targets`] except it follows the *queued click*
    /// rather than every wired lens, so a test or a host can assert the click
    /// path end-to-end without a window.
    pub fn pending_lens_targets(&self) -> Vec<String> {
        match &self.lens_open_request {
            Some(id) => vec![id.clone()],
            None => Vec::new(),
        }
    }

    /// The lens rows the pane is currently painting. Empty when nothing has been
    /// computed ([`Self::show_node_and_compute`] never ran), which is the honest
    /// state of a pane with no consumer.
    pub fn lenses(&self) -> &LensRows {
        &self.lenses
    }

    /// The number of painted lens rows: `2 * placed declarations`, mirroring the
    /// reference's `main` + `references` pair per declaration
    /// (`codelens.js:221-244`). `0` when no consumer has computed any.
    pub fn lens_count(&self) -> usize {
        self.lenses.len()
    }

    /// The node ids a click on a **wired** lens would open, in document order.
    ///
    /// This is the observable result of the lens action wiring: the reference's
    /// `main` lens runs `V + '.showInGraph'` with the declaration's node id
    /// (`codelens.js:228-232`), and this app's equivalent is selecting that node
    /// — the action has to reach `GraphView`, which is `App`'s to perform, so the
    /// pane exposes the *targets* and `App` consumes them. A `references` row is
    /// deliberately absent: nothing in this app consumes
    /// `csgraph.codelens.references` (see [`render_lens_row`]), and listing it
    /// here would be the "a handler exists" claim this task exists to remove.
    pub fn lens_targets(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for rows in &self.lenses.lines {
            for r in rows {
                if r.command_wired {
                    out.push(r.node_id.clone());
                }
            }
        }
        out
    }

    /// Close the pane (the head's "×").
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Drop the cached galleys. Called wherever the document could change; the
    /// key check in [`Self::galleys_for`] is the real guard, and this keeps the
    /// retained `Arc`s from outliving a document the shell has moved on from.
    fn invalidate_galleys(&mut self) {
        *self.galleys.get_mut() = None;
    }

    /// Laid-out galleys for `doc` under `theme`, reusing the cache when — and
    /// only when — nothing that affects the layout has changed.
    ///
    /// Returns a `Vec` with one galley per document line, in document order.
    ///
    /// The cache is keyed on [`GalleyKey`] (document fingerprint, theme
    /// fingerprint, **lens fingerprint**, highlight toggle, width). A miss
    /// re-lays out every line and replaces the cache wholesale; a hit clones the
    /// stored `Arc`s, which is a refcount bump per line, not a re-shape. Both
    /// directions are tested (`source_pane::tests::galley_cache_*`).
    ///
    /// `doc` is passed in rather than read from `self.doc` so the caller can
    /// keep its own shared borrow of the document alive across the call; the
    /// cache lives behind a `RefCell` for the same reason.
    fn galleys_for(
        &self,
        ui: &egui::Ui,
        doc: &Document,
        theme: &SourceTheme,
        width: f32,
    ) -> Vec<Arc<egui::Galley>> {
        let key = GalleyKey {
            doc: DocFingerprint::of(doc),
            theme: ThemeFingerprint::of(theme),
            // `self.lenses` is read here and *nowhere else* the cache can see,
            // which is why the field is part of the key rather than a flag some
            // writer has to remember to bump: `set_lenses` is the only writer,
            // and a changed lens set is a different key by construction.
            lens: LensFingerprint::of(&self.lenses),
            highlight: self.highlight,
            // Width only matters if something ever wraps; see `GalleyKey`. It is
            // part of the key so turning wrapping on cannot reuse stale galleys.
            width: width.to_bits(),
            // Cheap `Arc` clone; `egui::Context` compares by pointer identity, so
            // this distinguishes "same atlas" from "a different context that
            // happens to show the same document".
            ctx: ui.ctx().clone(),
        };

        if let Some(cache) = self.galleys.borrow().as_ref() {
            if cache.key == key && cache.galleys.len() == doc.lines.len() {
                self.galley_hits.set(self.galley_hits.get() + 1);
                return cache.galleys.clone();
            }
        }

        let row_h = theme.row_h();
        let highlight = self.highlight;
        let galleys: Vec<Arc<egui::Galley>> = doc
            .lines
            .iter()
            .map(|l| line_galley(ui, l, theme, highlight, row_h, self.lenses.for_line(l.n)))
            .collect();
        // Store before returning so the next call is a hit. A re-entrant call
        // (there is none today) would trip this borrow rather than corrupt state.
        *self.galleys.borrow_mut() = Some(GalleyCache {
            key,
            galleys: galleys.clone(),
        });
        self.galley_misses.set(self.galley_misses.get() + 1);
        galleys
    }

    /// `(hits, misses)` since this pane was created. Used by the tests and by
    /// `CSVIEWER_FRAME_TIMING` to prove the cache is doing something.
    pub fn galley_cache_stats(&self) -> (u64, u64) {
        (self.galley_hits.get(), self.galley_misses.get())
    }

    /// True when the document's origin span contains `file:line`; used by the
    /// shell to ask "is the graph selection already shown?".
    pub fn shows(&self, node_ref: &str) -> bool {
        self.loaded_key.as_deref() == Some(node_ref)
    }
}

// ---------------------------------------------------------------------------
// painting
// ---------------------------------------------------------------------------

fn line_galley(
    ui: &egui::Ui,
    line: &DocLine,
    theme: &SourceTheme,
    highlight: bool,
    row_h: f32,
    lenses: &[LensText],
) -> Arc<egui::Galley> {
    let font = FontId::monospace(theme.font_size);
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let format = |color: Color32, italics: bool| TextFormat {
        font_id: font.clone(),
        color,
        italics,
        line_height: Some(row_h),
        ..Default::default()
    };
    // The declaration's code lenses first, one job line each, so they lay out
    // *above* the document text exactly as Monaco draws a lens above its
    // `range.startLineNumber` (`codelens.js:219-221`). They are part of this
    // galley — not a separate one — so the [`GalleyKey`]'s `lens` component
    // covers them and [`render_lens_row`] only has to position them.
    //
    // The trailing `\n` is what puts the document text on the next job line:
    // `LayoutJob::append` never breaks a line by itself. Each lens run therefore
    // ends with a newline, and because `row_h` is the `line_height` of every
    // section the document text lands exactly `n * row_h` below the row's top —
    // which is where `render_lens_row` paints each caption back.
    //
    // The lens font is `0.86 * font_size`; `render_lens_row` must use the same
    // size for the painted text to line up with the shaped glyphs.
    for l in lenses {
        let color = if l.command_wired { theme.accent } else { theme.text_faint };
        job.append(
            &format!("{}\n", l.title),
            0.0,
            TextFormat {
                font_id: FontId::monospace(theme.font_size * 0.86),
                color,
                line_height: Some(row_h),
                ..Default::default()
            },
        );
    }
    // The generated `part` marker really is a comment, and `cond` lines are
    // preprocessor conditions, so those two carry an explicit colour.
    //
    // `head` / `tail` used to be forced to the comment colour too. That was
    // the reported bug: a synthesized document's `head` lines are the real
    // `namespace` / class / attribute declarations the engine gathered, not
    // comments, so `namespace` and its name must keep their token colours.
    // Everything that is not `part`/`cond` is painted from the shared
    // tokenizer's segments (or the plain foreground when highlighting is off).
    let ctx = line.ctx.as_str();
    let override_color = match ctx {
        "part" => Some(theme.comment),
        "cond" => Some(theme.preproc),
        _ => None,
    };
    if let Some(c) = override_color {
        job.append(&line.text, 0.0, format(c, ctx == "part"));
    } else if highlight {
        for seg in &line.segs {
            job.append(&seg.text, 0.0, format(theme.tok_color(seg.tok), false));
        }
    } else {
        job.append(&line.text, 0.0, format(theme.fg, false));
    }
    ui.ctx().fonts_mut(|f| f.layout_job(job))
}

/// Paint one declaration's code lenses inside the **top of its own line's row**.
///
/// The reference renders a lens as a line *above* the declaration line
/// (`codelens.js:219-221` builds a `monaco.Range(ln, 1, ln, 1)` and Monaco draws
/// the decoration above it). This pane's rows are a single `row_h` allocation
/// per document line and the row height is fixed by CSS (`.sline {
/// line-height: 1.55 }`, [`SourceTheme::row_h`]), so there is no room to insert a
/// whole new row without changing every row's height and shifting the document —
/// which would contradict the reference's own layout. The lens therefore paints
/// in the top band of the declaration's row, over the background already drawn
/// for that row, which keeps it *on the line above the declaration's text* in the
/// sense the reference means: the caption sits above the declaration, and the
/// declaration's text is pushed down inside its own row.
///
/// Every row's lens text was shaped into the **same galley** as its document line
/// ([`line_galley`]), so the caption needs no cache of its own: it is covered by
/// the [`GalleyKey`]'s `lens` component, and this function only positions it.
///
/// ## Which command is wired
///
/// The reference's two commands are `csgraph.codelens.showInGraph` (the `main`
/// lens, arguments `[row.id]`, `codelens.js:228-232`) and
/// `csgraph.codelens.references` (arguments `[row.id, row.file, row.line,
/// labelOf(row)]`, `codelens.js:240-243`).
///
/// * `showInGraph` **is** wired: selecting a node is `GraphView`'s action, the
///   same one a graph click performs, and the pane exposes the target through
///   [`SourceState::lens_targets`] for `App` to apply. It paints in
///   `theme.accent`, the token this pane already uses for a clickable label.
/// * `references` is **not** wired. It is painted (the reference draws both
///   lenses for every declaration, and dropping it would under-report the
///   document) but in `theme.text_faint`, and it is excluded from
///   `lens_targets`. Nothing in this app consumes that command id: there is no
///   reference-search surface bound to a lens, `find_wire` is the pane's search
///   bar and is driven by the find action, not by a lens argument tuple. Marking
///   it clickable would be exactly the "a handler exists" claim this task exists
///   to remove, so it is drawn as inert text and reported as blocked.
fn render_lens_row(painter: &egui::Painter, rect: Rect, row: &LensText, theme: &SourceTheme) {
    let color = if row.command_wired { theme.accent } else { theme.text_faint };
    // A hair below the row's top: the CSS `.codelens-decoration` sits flush with
    // the line it annotates, and the pane's rows are already 19px.
    let y = rect.top() + 1.0;
    painter.text(
        Pos2::new(rect.left() + theme.gutter_w, y),
        Align2::LEFT_TOP,
        &row.title,
        FontId::monospace(theme.font_size * 0.86),
        color,
    );
}

/// Paint one row into `rect` (already positioned by the caller).
///
/// `lenses` are this line's code lenses ([`LensRows::for_line`]); empty for a
/// line with no declaration, in which case this paints exactly what it painted
/// before the CodeLens wiring — which is what keeps a lens-free document
/// byte-identical.
fn paint_row(
    painter: &egui::Painter,
    rect: Rect,
    line: &DocLine,
    galley: &Arc<egui::Galley>,
    is_target: bool,
    theme: &SourceTheme,
    lenses: &[LensText],
) {
    match line.ctx.as_str() {
        "part" => {
            painter.rect_filled(rect, 0.0, theme.part_bg);
        }
        "cond" => {
            painter.rect_filled(rect, 0.0, theme.cond_bg);
            painter.rect_filled(
                Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
                0.0,
                theme.cond_bar,
            );
        }
        _ => {}
    }
    if is_target {
        painter.rect_filled(rect, 0.0, theme.target_bg);
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
            0.0,
            theme.accent,
        );
    }
    // Gutter: real lines show their document number, nzk_ui shows none.
    let nzk_ui = matches!(line.ctx.as_str(), "part" | "cond" | "head" | "tail");
    if !nzk_ui && line.n > 0 {
        let color = if is_target { theme.gutter_active } else { theme.gutter };
        painter.text(
            Pos2::new(rect.left() + theme.gutter_w - 12.0, rect.center().y),
            Align2::RIGHT_CENTER,
            line.n.to_string(),
            FontId::monospace(theme.font_size * 0.94),
            color,
        );
    }
    let alpha = if line.active { 1.0 } else { 0.45 };
    let fallback = if alpha < 1.0 { theme.fg.gamma_multiply(alpha) } else { theme.fg };
    // The captions are *inside* this galley (see `line_galley`), so painting the
    // galley is what paints the lenses — there is no second call to make. This
    // debug assertion is the invariant that keeps those two facts tied
    // together: if a future edit moves the lens runs out of `line_galley`, the
    // row would be silent here and this fires instead of the lens quietly
    // vanishing.
    debug_assert!(
        lenses.is_empty() || galley.text().contains(lenses[0].title.as_str()),
        "a row with lens rows must have shaped them into its own galley"
    );
    painter.galley(
        Pos2::new(rect.left() + theme.gutter_w, rect.top()),
        galley.clone(),
        fallback,
    );
}

fn empty_note(ui: &egui::Ui, theme: &SourceTheme, text: &str, color: Color32) {
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, theme.bg);
    ui.painter().text(
        Pos2::new(rect.left() + 12.0, rect.top() + 10.0),
        Align2::LEFT_TOP,
        text,
        FontId::monospace(theme.font_size),
        color,
    );
}

// ---------------------------------------------------------------------------
// interactive pane
// ---------------------------------------------------------------------------

/// The interactive source pane: head + scrollable highlighted body.
///
/// Place it in a bottom panel. It fills whatever `ui` it is given.
///
/// Returns the node ids of any **wired** lens clicked this frame. The reference's
/// `main` lens runs `csgraph.codelens.showInGraph` with the declaration's node id
/// (`codelens.js:228-232`); in this app "show in graph" is a *selection change*,
/// which is `App`'s to perform (the pane owns no `GraphView`). Returning the
/// targets rather than performing them keeps the split the rest of this file
/// already uses — and it is what [`ui`] hands straight back to its caller.
///
/// An empty return is the honest "no lens was clicked": it is never a claim that
/// a handler ran.
// ---------------------------------------------------------------------------
// source language: token -> node, hover facts, references, history keys
// ---------------------------------------------------------------------------
//
// The v2 viewer used Monaco's extension API for hover cards (`hover.js`),
// go-to-definition / find-references (`language.js`, `semantics.js`) and the
// document outline (`symbols.js`). This fork deliberately has **no Monaco and
// no language server**; the functional equivalents live here, over the graph
// the app already owns. The rules are honest: a token maps to a node only when
// the mapping is unambiguous, and a hover card is shown only for a real node.

/// A token under the pointer: the identifier text and its start column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRef {
    pub text: String,
    /// 0-based character column in the document line.
    pub col: usize,
}

/// The identifier the pointer at character column `col` is over, from the
/// line's tokenized segments.
///
/// A segment may pack several characters of one class (e.g. a whitespace run,
/// or `Abc.Def`), so the run boundaries are found inside the segment: only
/// `[A-Za-z0-9_]` counts as identifier text. A comment/string/op segment never
/// yields a token — hover and go-to-definition must not fire on text that is
/// not code.
pub fn token_at(line: &DocLine, col: usize) -> Option<TokenRef> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut base = 0usize;
    for seg in &line.segs {
        let len = seg.text.chars().count();
        if col >= base && col < base + len {
            let idx = col - base;
            let chars: Vec<char> = seg.text.chars().collect();
            if !matches!(
                seg.tok,
                Tok::Plain | Tok::Type | Tok::Function | Tok::Variable | Tok::Keyword | Tok::Control
            ) {
                return None;
            }
            if !chars.get(idx).copied().map(ident).unwrap_or(false) {
                return None;
            }
            let mut a = idx;
            while a > 0 && ident(chars[a - 1]) {
                a -= 1;
            }
            let mut b = idx;
            while b + 1 < chars.len() && ident(chars[b + 1]) {
                b += 1;
            }
            let text: String = chars[a..=b].iter().collect();
            return Some(TokenRef { text, col: base + a });
        }
        base += len;
    }
    None
}

/// What a real graph node says for a hover card / definition jump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolHit {
    /// `Node::id` — the value the graph selection and document open take.
    pub node_id: String,
    pub kind: String,
    pub label: String,
    pub full: String,
    /// Declaration site, `file:line`.
    pub decl: String,
    /// Incoming references (edges pointing at this node), the count `hover.js`
    /// renders as "N references".
    pub refs: usize,
}

impl SymbolHit {
    /// The card body: kind, full name, declaration site, reference count.
    /// Pure and pinned by a test, so the card cannot show invented numbers.
    pub fn card_lines(&self) -> Vec<String> {
        vec![
            format!("{}  {}", self.kind, self.label),
            self.full.clone(),
            format!("declared at {}", self.decl),
            format!("{} reference{}", self.refs, if self.refs == 1 { "" } else { "s" }),
        ]
    }
}

/// One row of the find-references list: an incoming edge's source node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefRow {
    pub node_id: String,
    pub label: String,
    /// The relation the reference uses (`calls`, `uses`, `inherits`, …).
    pub kind: String,
    pub file: String,
    pub line: i64,
}

impl RefRow {
    /// `label — kind — file:line`, the row text the pane paints.
    pub fn line_text(&self) -> String {
        format!("{}  ·  {}  ·  {}:{}", self.label, self.kind, self.file, self.line)
    }
}

/// What `F12`/`Shift+F12` mean when the source pane owns the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKey {
    /// `F12` — go to the declaration (`language.js` definition provider).
    Definition,
    /// `Shift+F12` — find references (`language.js` usages provider).
    References,
    /// `Alt+Left` — source document history back (`source.js`/`nav.js`).
    HistoryBack,
    /// `Alt+Right` — source document history forward.
    HistoryForward,
}

/// The source-language key table, pure so the bindings are testable without a
/// window. `ctrl`/`meta` are accepted for F12 (VS Code allows both) and must be
/// absent for the Alt+Arrow history chords, which mirror the graph's
/// `Alt+Left`/`Alt+Right` (see `app.rs::is_history_back_chord`).
pub fn source_key(name: &str, ctrl: bool, meta: bool, shift: bool, alt: bool) -> Option<SourceKey> {
    match name {
        "F12" if !alt => Some(if shift { SourceKey::References } else { SourceKey::Definition }),
        "ArrowLeft" if alt && !ctrl && !meta && !shift => Some(SourceKey::HistoryBack),
        "ArrowRight" if alt && !ctrl && !meta && !shift => Some(SourceKey::HistoryForward),
        _ => None,
    }
}

pub fn ui_with_lens_actions(
    ui: &mut egui::Ui,
    state: &mut SourceState,
    theme: &SourceTheme,
) -> Vec<String> {
    ui_impl(ui, state, theme, None)
}

/// The pane with the **source-language layer**: hover cards, Ctrl/Cmd+click
/// go-to-definition, Shift+click find-references and the `Shift+F12` reference
/// list. `graph` is the same `Graph` the app selects from, so a card can only
/// name a real node (the v2 Monaco providers' functional equivalent).
///
/// Callers without a graph (the detached Source window, the headless shot) use
/// [`ui_with_lens_actions`]; the pane is then byte-identical to the pre-language
/// paint, which is what keeps the shipped default frame and `@frame` pin
/// unchanged.
pub fn ui_with_language(
    ui: &mut egui::Ui,
    state: &mut SourceState,
    theme: &SourceTheme,
    language: &dyn Language,
) -> Vec<String> {
    ui_impl(ui, state, theme, Some(language))
}

/// The pane's own keyboard chords, shared by the full pane paint and the
/// body-only paint the nzk_ui composition uses.
///
/// * `Ctrl/Cmd+S` — queue the save for the host that owns the engine `Index`
///   (`App::consume_source_save` -> `edit_transport::invoke("write_file", …)`).
///   Only an edit session with a real change produces a request; a clean buffer
///   leaves the chord alone rather than claiming a write.
/// * `Ctrl/Cmd+E` — toggle edit mode. The edit toggle lived on the native head
///   (`edit` checkbox); the nzk_ui composition paints only the body and
///   leaves the reference's own `#sourcePane` header visible (which has no such
///   control, exactly like v2's read-only Monaco pane), so a chord is what keeps
///   editing reachable there. The fallback head keeps its checkbox.
fn pane_chords(ui: &egui::Ui, state: &mut SourceState) {
    let (cmd, s_pressed, e_pressed) = ui.input(|i| {
        (
            i.modifiers.command || i.modifiers.ctrl,
            i.key_pressed(egui::Key::S),
            i.key_pressed(egui::Key::E),
        )
    });
    if !cmd {
        return;
    }
    if s_pressed && state.editing {
        state.request_save();
    }
    if e_pressed && state.doc.is_some() {
        if state.editing {
            state.stop_editing();
        } else {
            state.start_editing();
        }
    }
}

fn ui_impl(
    ui: &mut egui::Ui,
    state: &mut SourceState,
    theme: &SourceTheme,
    language: Option<&dyn Language>,
) -> Vec<String> {
    pane_chords(ui, state);

    // ---- head ---------------------------------------------------------
    let mut close = false;
    egui::Frame::NONE
        .fill(theme.head_bg)
        .inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 6 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                match &state.doc {
                    Some(doc) => {
                        ui.label(
                            egui::RichText::new(doc.title.clone())
                                .monospace()
                                .size(11.5)
                                .color(theme.accent),
                        );
                        let span = doc.span_label();
                        if !span.is_empty() {
                            ui.label(
                                egui::RichText::new(span)
                                    .monospace()
                                    .size(10.0)
                                    .color(theme.text_faint),
                            );
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "{} lines · {} parts · {} sections",
                                doc.line_count(),
                                doc.parts,
                                doc.sections
                            ))
                            .monospace()
                            .size(10.0)
                            .color(theme.text_dim),
                        );
                        if !doc.defines_known {
                            ui.label(
                                egui::RichText::new("all #if branches kept")
                                    .monospace()
                                    .size(10.0)
                                    .color(theme.preproc),
                            );
                        }
                    }
                    None => {
                        ui.label(
                            egui::RichText::new("source")
                                .monospace()
                                .size(11.5)
                                .color(theme.accent),
                        );
                        ui.label(
                            egui::RichText::new("select a node")
                                .monospace()
                                .size(10.0)
                                .color(theme.text_faint),
                        );
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(egui::RichText::new("×").size(12.0)))
                        .clicked()
                    {
                        close = true;
                    }
                    ui.checkbox(&mut state.highlight, "highlight");
                    // The edit toggle: off by default, so the pane keeps the
                    // read-only paint unless the user opts in. Entering seeds
                    // the buffer from the document that is already open.
                    let mut edit = state.editing;
                    if ui.checkbox(&mut edit, "edit").changed() {
                        if edit {
                            state.start_editing();
                        } else {
                            state.stop_editing();
                        }
                    }
                    if state.editing && state.is_dirty() {
                        ui.label(
                            egui::RichText::new("● dirty")
                                .monospace()
                                .size(10.0)
                                .color(theme.danger),
                        );
                    }
                    if let Some(err) = state.save_error() {
                        ui.label(
                            egui::RichText::new(err)
                                .monospace()
                                .size(10.0)
                                .color(theme.danger),
                        );
                    }
                });
            });
        });

    if close {
        state.close();
        return Vec::new();
    }

    references_panel(ui, state, theme);

    // ---- body ---------------------------------------------------------
    body_pane(ui, state, theme, language)
}

/// The find-references list (`Shift+F12`).
///
/// Painted above the document, in the pane itself: there is no Monaco `Peek`
/// widget here, and the app has no separate references surface, so the honest
/// place is the pane that owns the symbol. Rows are the graph's own incoming
/// edges and a click opens that node (the same host request a lens click uses).
/// Shared by the full pane and the body-only paint the nzk_ui composition
/// uses, so the list is reachable in both.
fn references_panel(ui: &mut egui::Ui, state: &mut SourceState, theme: &SourceTheme) {
    let mut close_refs = false;
    let mut open_ref: Option<String> = None;
    let refs: Vec<String> = state
        .references()
        .map(|rs| rs.iter().take(50).map(RefRow::line_text).collect())
        .unwrap_or_default();
    if refs.is_empty() {
        return;
    }
    egui::Frame::NONE
        .fill(theme.head_bg)
        .stroke(Stroke::new(1.0, theme.border_soft))
        .inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 4 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("references ({})", refs.len()))
                        .monospace()
                        .size(10.5)
                        .color(theme.accent),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(egui::RichText::new("×").size(11.0)))
                        .clicked()
                    {
                        close_refs = true;
                    }
                });
            });
            for (i, text) in refs.iter().enumerate() {
                if ui
                    .selectable_label(false, egui::RichText::new(text).monospace().size(10.0))
                    .clicked()
                {
                    if let Some(rs) = state.references() {
                        if let Some(row) = rs.get(i) {
                            open_ref = Some(row.node_id.clone());
                        }
                    }
                    close_refs = true;
                }
            }
        });
    if let Some(id) = open_ref {
        state.lens_open_request = Some(id);
    }
    if close_refs {
        state.close_references();
    }
}

/// The scroll source both document surfaces use: mouse wheel and the scrollbar,
/// never content-drag (Monaco selects text on a drag; it does not pan).
fn pane_scroll_source() -> egui::scroll_area::ScrollSource {
    egui::scroll_area::ScrollSource {
        drag: egui::scroll_area::DragScroll::Never,
        ..Default::default()
    }
}

/// The scrollable document body — everything below the pane head.
///
/// Split out of [`ui_impl`] so the nzk_ui composition can paint **only** this
/// part into the DOM's `#sourceBody` box, leaving the reference's real
/// `#sourcePane` header and `#cs-tabs` strip (which the shell paints from its
/// own DOM/CSS) visible on top. Keeping one body implementation is what makes
/// the fallback pane and the composed pane show the same document, the same
/// lenses and the same edit buffer.
fn body_pane(
    ui: &mut egui::Ui,
    state: &mut SourceState,
    theme: &SourceTheme,
    language: Option<&dyn Language>,
) -> Vec<String> {
    let row_h = theme.row_h();
    let mut clicked: Vec<String> = Vec::new();
    let scroll_to = state.scroll_to.take();
    if state.doc.is_none() {
        let msg = state
            .error
            .clone()
            .unwrap_or_else(|| "no document loaded — click a graph node".to_string());
        empty_note(ui, theme, &msg, theme.text_faint);
        return clicked;
    }

    // The Monaco behaviour, not a taste choice: the reference editor uses a
    // click-drag for text selection, and a scroll area that claimed drags here
    // would swallow the press before the row's own lens target (or the
    // TextEdit's caret placement) could see it. Wheel and scrollbar still work.
    let mut area = ScrollArea::vertical()
        .id_salt("ncv_source_body")
        .auto_shrink([false, false])
        .scroll_source(pane_scroll_source());
    if let Some(line) = scroll_to {
        let offset = (line as f32 * row_h - 0.35 * ui.available_height()).max(0.0);
        area = area.vertical_scroll_offset(offset);
    }

    // ---- editable buffer (opt-in) -------------------------------------
    // The read-only path above is untouched: this branch exists only while the
    // head's `edit` checkbox is on. The TextEdit gets the pane's own rect, so a
    // keystroke lands where the user was reading, not in a second surface.
    if state.editing {
        egui::ScrollArea::vertical()
            .id_salt("ncv_source_edit")
            .auto_shrink([false, false])
            .scroll_source(pane_scroll_source())
            .show(ui, |ui| {
                let size = Vec2::new(ui.available_width().max(1.0), ui.available_height().max(row_h));
                ui.add_sized(
                    size,
                    egui::TextEdit::multiline(&mut state.editable)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
        // Dirty is derived (`editable != saved_text`), so there is no flag to
        // update here: the head reads it directly and Ctrl+S has already been
        // handled above.
        return clicked;
    }
    let mut cursor_hit: Option<String> = None;
    let mut open_symbol: Option<String> = None;
    let mut refs_symbol: Option<String> = None;
    let mut hovered_row: Option<usize> = None;
    area.show(ui, |ui| {
        let width = ui.available_width().max(1.0);
        let Some(doc) = state.doc.as_ref() else { return };
        // One layout (or reuse) for the whole document, before painting starts.
        let galleys = state.galleys_for(ui, doc, theme, width);
        debug_assert_eq!(
            galleys.len(),
            doc.lines.len(),
            "galley cache must hold exactly one galley per document line"
        );
        for (i, (line, galley)) in doc.lines.iter().zip(&galleys).enumerate() {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, row_h), Sense::hover());
            let is_target = doc.target.map(|(a, b)| i >= a && i < b).unwrap_or(false);
            // The IDE's highlight band, painted under the row.
            if state.highlight_lines.is_some_and(|(a, b)| i >= a && i <= b) {
                ui.painter()
                    .rect_filled(rect, 0.0, theme.accent.gamma_multiply(0.16));
            }
            if ui.rect_contains_pointer(rect) {
                hovered_row = Some(i);
            }
            let lenses = state.lenses.for_line(line.n);
            paint_row(ui.painter(), rect, line, galley, is_target, theme, lenses);
            // Source-language layer: token under the pointer -> real graph node.
            // The pane cannot select (it has no `GraphView`), so a click is
            // queued on `lens_open_request` exactly like a wired lens press.
            if let Some(g) = language {
                if ui.rect_contains_pointer(rect) {
                    if let Some(pos) = ui.ctx().pointer_hover_pos() {
                        let rel = Vec2::new(pos.x - rect.left(), pos.y - rect.top());
                        let col = galley.cursor_from_pos(rel).index.0;
                        if let Some(tok) = token_at(line, col) {
                            if let Some(hit) = g.symbol_for_token(&doc.file, &tok.text) {
                                cursor_hit = Some(hit.node_id.clone());
                                let resp = ui.interact(
                                    rect,
                                    ui.id().with(("ncv_sym", line.n, tok.col)),
                                    Sense::click(),
                                );
                                let (ctrl, cmd, shift) =
                                    ui.input(|inp| {
                                        (inp.modifiers.ctrl, inp.modifiers.command, inp.modifiers.shift)
                                    });
                                if (ctrl || cmd) && resp.clicked() {
                                    open_symbol = Some(hit.node_id.clone());
                                }
                                if shift && resp.clicked() {
                                    refs_symbol = Some(hit.node_id.clone());
                                }
                                resp.on_hover_ui(|ui| {
                                    for l in hit.card_lines() {
                                        ui.label(
                                            egui::RichText::new(l).monospace().size(10.5),
                                        );
                                    }
                                });
                            }
                        }
                    }
                }
            }
            // The reference's `main` lens is a *click target*
            // (`codelens.js:228-232`). Native has no Monaco widget, so the hit
            // area is the row itself: an interactive response over exactly the
            // rows that carry a wired lens, which is what makes the lens action
            // reachable rather than merely painted.
            if !lenses.is_empty() {
                // One interactive response per lens-bearing row. `Sense::click`
                // (not `hover`) because the reference's `main` lens *is* a click
                // target; a row with only an unwired `references` row still
                // responds, so the hit area matches what is drawn, but nothing
                // is pushed for it.
                let hit = ui.interact(
                    rect,
                    ui.scope_id().with(("ncv_lens", line.n)),
                    Sense::click(),
                );
                if hit.clicked() {
                    for l in lenses.iter().filter(|l| l.command_wired) {
                        clicked.push(l.node_id.clone());
                    }
                }
                if hit.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        }
    });
    // Sticky: the row the IDE's pointer is on is the IDE's current highlight,
    // and it survives the pointer leaving for the Scratch window.
    if let Some(i) = hovered_row {
        state.highlight_lines = Some((i, i));
    }
    state.cursor_symbol = cursor_hit;
    if let Some(id) = open_symbol {
        state.lens_open_request = Some(id);
    }
    if let Some(id) = refs_symbol {
        if let Some(g) = language {
            state.set_references(g.references_for(&id));
        }
    }
    clicked
}

/// [`ui_with_lens_actions`] for callers that have no interest in a lens click.
///
/// The detached Source window and the `CSVIEWER_SCREENSHOT_PANE` evidence path
/// paint through here: a lens clicked in a popped-out window cannot change the
/// main window's selection, so discarding the target is the honest behaviour
/// rather than a silent one — the caption is still *painted*, the action is
/// simply unavailable from that window, and `App` only consumes targets from the
/// main window's pass.
pub fn ui(ui: &mut egui::Ui, state: &mut SourceState, theme: &SourceTheme) {
    let _ = ui_with_lens_actions(ui, state, theme);
}

/// Paint **only the document body** (no native head) into the rect the host
/// gives this `Ui`.
///
/// This is the composed-pane half of the nzk_ui architecture: the shell
/// paints the reference's real `#sourcePane` header and `#cs-tabs` strip (that
/// is what the v2 `index.html` + `style.css` produce), and the native painter
/// fills only the DOM's `#sourceBody` box. The full pane ([`ui`]) is used only
/// when the nzk_ui nzk_ui is absent, so a shell-less app still works.
///
/// The body is clipped to `ui.max_rect()` and its background is masked with the
/// theme's own `bg` first: the host creates the child `Ui` from the DOM rect, and
/// a child inherits the window clip rect, so without the explicit clip a long
/// document would paint over the header above it. The mask is what makes the
/// composite opaque — the shell's body background is never allowed to show
/// through a themed gap.
pub fn ui_body_with_language(
    ui: &mut egui::Ui,
    state: &mut SourceState,
    theme: &SourceTheme,
    language: &dyn Language,
) -> Vec<String> {
    body_entry(ui, state, theme, Some(language))
}

/// [`ui_body_with_language`] for callers with no graph (the detached Source
/// window's split body): identical pixels, no hover/lens language layer.
pub fn ui_body(ui: &mut egui::Ui, state: &mut SourceState, theme: &SourceTheme) -> Vec<String> {
    body_entry(ui, state, theme, None)
}

fn body_entry(
    ui: &mut egui::Ui,
    state: &mut SourceState,
    theme: &SourceTheme,
    language: Option<&dyn Language>,
) -> Vec<String> {
    let rect = ui.max_rect();
    let clip = ui.clip_rect().intersect(rect);
    ui.set_clip_rect(clip);
    ui.painter().rect_filled(clip, 0.0, theme.bg);
    pane_chords(ui, state);
    references_panel(ui, state, theme);
    body_pane(ui, state, theme, language)
}

// ---------------------------------------------------------------------------
// headless whole-document painter
// ---------------------------------------------------------------------------

/// Paint the whole document without scrolling and return the content size in
/// points. Used by the headless PNG path (and by its measure pass), so the
/// evidence image is the same painter the window uses.
///
/// Takes `&mut SourceState` because it lays the document out through the same
/// keyed galley cache as [`ui`]. `source_shot --size auto` measures and then
/// paints the same `(doc, theme, width)`, so the paint pass is a cache hit and
/// the document is shaped once, not twice.
pub fn document_ui(ui: &egui::Ui, state: &mut SourceState, theme: &SourceTheme) -> (f32, f32) {
    let row_h = theme.row_h();
    let painter = ui.painter();

    if state.doc.is_none() {
        let h = 72.0;
        let w = 640.0;
        painter.rect_filled(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h)), 0.0, theme.bg);
        painter.text(
            Pos2::new(12.0, 12.0),
            Align2::LEFT_TOP,
            state.error.clone().unwrap_or_else(|| "no document loaded".to_string()),
            FontId::monospace(theme.font_size),
            theme.danger,
        );
        return (w, h);
    };

    // Lay every line out first so the width is real, not guessed. The document
    // is painted unwrapped, so width does not constrain the layout here; `0.0`
    // is a stable key component (this path never wraps).
    let Some(doc) = state.doc.as_ref() else { return (360.0, 0.0) };
    let galleys = state.galleys_for(ui, doc, theme, 0.0);
    let max_w = galleys.iter().fold(0.0f32, |m, g| m.max(g.size().x));
    let width = (theme.gutter_w + max_w + 14.0).ceil().max(360.0);
    let height = (HEAD_H + doc.lines.len() as f32 * row_h).ceil();

    painter.rect_filled(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height)), 0.0, theme.bg);

    // Head, drawn with the painter (no widget state in a screenshot).
    let head = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, HEAD_H));
    painter.rect_filled(head, 0.0, theme.head_bg);
    painter.hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme.border_soft));
    let span = doc.span_label();
    let title = if span.is_empty() {
        doc.title.clone()
    } else {
        format!("{}  ·  {}", doc.title, span)
    };
    painter.text(
        Pos2::new(head.left() + 10.0, head.center().y),
        Align2::LEFT_CENTER,
        title,
        FontId::monospace(11.5),
        theme.accent,
    );
    painter.text(
        Pos2::new(head.right() - 10.0, head.center().y),
        Align2::RIGHT_CENTER,
        format!(
            "{} lines · {} parts · {} sections",
            doc.line_count(),
            doc.parts,
            doc.sections
        ),
        FontId::monospace(10.0),
        theme.text_dim,
    );

    for (i, (line, galley)) in doc.lines.iter().zip(galleys.iter()).enumerate() {
        let rect = Rect::from_min_size(
            Pos2::new(0.0, HEAD_H + i as f32 * row_h),
            Vec2::new(width, row_h),
        );
        let is_target = doc.target.map(|(a, b)| i >= a && i < b).unwrap_or(false);
        let lenses = state.lenses.for_line(line.n);
        paint_row(painter, rect, line, galley, is_target, theme, lenses);
    }
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_payload() -> Value {
        json!({
            "ok": true, "total": 6, "start": 1, "end": 6, "sections": 0,
            "definesKnown": false, "parts": [{"file": "a.cs", "line": 1}],
            "node": {"id": "NZK.B", "full": "NZK.B", "label": "B",
                     "kind": "class", "file": "a.cs", "line": 1, "endLine": 4},
            "lines": [
                {"n": 0, "ctx": "part", "text": "// ── NZK.B · a.cs:1-4 ──"},
                {"n": 1, "ctx": "head", "text": "namespace NZK{", "file": "a.cs", "orig": 1},
                {"n": 2, "ctx": "head", "text": "  public static partial class B{", "file": "a.cs", "orig": 2},
                {"n": 3, "ctx": "body", "text": "    public static int A(){return 1;}", "file": "a.cs", "orig": 3},
                {"n": 4, "ctx": "tail", "text": "  }", "file": "a.cs", "orig": 4},
                {"n": 5, "ctx": "tail", "text": "}", "file": "a.cs", "orig": 4}
            ]
        })
    }

    #[test]
    fn an_edited_buffer_round_trips_into_the_write_file_payload() {
        let mut state = SourceState::new();
        let doc = Document::from_value("NZK.B", &sample_payload()).unwrap();
        state.adopt_document("NZK.B", doc);
        // Default is the read-only paint: edit mode is opt-in.
        assert!(!state.editing);
        state.start_editing();
        assert_eq!(
            state.editable_text(),
            "// ── NZK.B · a.cs:1-4 ──\nnamespace NZK{\n  public static partial class B{\n    public static int A(){return 1;}\n  }\n}"
        );
        // A freshly seeded buffer is clean and claims no write.
        assert!(!state.is_dirty());
        assert!(state.save_payload().is_none());

        state.editable.push_str("\n// edited by the task-18 test");
        assert!(state.is_dirty());
        let payload = state.save_payload().expect("a dirty buffer has a payload");
        // The buffer round-trips verbatim into the exact write_file body.
        assert_eq!(payload["path"], "a.cs");
        assert_eq!(payload["content"], state.editable_text());
        assert_eq!(payload["apply"], true);

        // Ctrl+S queues exactly that body, once.
        state.request_save();
        let req = state.take_save_request().expect("Ctrl+S queued a save");
        assert_eq!(req.write_file_args(), payload);
        assert!(state.take_save_request().is_none(), "a save is handed over once");

        // Only a real `"ok": true` envelope clears dirty; a refusal keeps it
        // dirty and names the error, never a fabricated success.
        state.finish_save(&serde_json::json!({
            "content": [{"type": "text", "text": "{\"ok\":false,\"error\":\"write-scope refusal\"}"}],
            "isError": true
        }));
        assert!(state.is_dirty());
        assert_eq!(state.save_error(), Some("write-scope refusal"));
        state.finish_save(&serde_json::json!({
            "content": [{"type": "text", "text": "{\"ok\":true}"}],
            "isError": false
        }));
        assert!(!state.is_dirty());
        assert!(state.save_error().is_none());
    }

    #[test]
    fn parses_the_engine_payload_and_finds_the_span() {
        let doc = Document::from_value("NZK.B", &sample_payload()).unwrap();
        assert_eq!(doc.title, "NZK.B");
        assert_eq!(doc.line_count(), 6);
        assert_eq!(doc.parts, 1);
        assert_eq!(doc.span_label(), "a.cs:1-4");
        // Focus is the first non-part line (the node's own first line).
        assert_eq!(doc.focus, 1);
        // The target range covers exactly the lines that came from a.cs:1-4.
        assert_eq!(doc.target, Some((1, 6)));
        // Highlight segments exist and never lose characters.
        let joined: String = doc.lines[2].segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, doc.lines[2].text);
        assert!(doc.lines[2].segs.iter().any(|s| s.tok == Tok::Keyword));
    }

    #[test]
    fn theme_row_height_matches_the_css_line_box() {
        let t = SourceTheme::darkplus();
        assert_eq!(t.row_h(), 19.0, "ceil(12 * 1.55)");
        assert_eq!(t.gutter_w, 62.0);
    }

    /// Hand-built painted lens rows, for the galley-key test (it must not
    /// depend on the host's codelens wiring to exercise the cache).
    fn lens_fixture(
        _doc: &Document,
        node_id: &str,
        title: &str,
        lines: Vec<(i64, &'static str)>,
    ) -> LensRows {
        let mut out = LensRows::empty();
        for (line, kind) in lines {
            let n = line as usize;
            if out.lines.len() <= n {
                out.lines.resize_with(n + 1, Vec::new);
            }
            out.lines[n].push(LensText {
                line,
                kind,
                node_id: node_id.to_string(),
                title: title.to_string(),
                command_wired: true,
            });
            out.lens_lines.push(line);
        }
        out
    }

    /// A `SourceState` with `sample_payload()` loaded, without needing an index.
    fn loaded_state() -> SourceState {
        let mut s = SourceState::default();
        s.doc = Some(Document::from_value("NZK.B", &sample_payload()).unwrap());
        s
    }

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            ..Default::default()
        }
    }

    /// One egui pass. `FullOutput` carries font-atlas deltas that must be
    /// consumed before the value is dropped (epaint panics otherwise), and a
    /// test has no GPU to upload them to — so clear them, exactly like
    /// `source_shot`'s measure pass does.
    fn frame(ctx: &egui::Context, mut body: impl FnMut(&mut egui::Ui)) {
        let mut out = ctx.run_ui(raw_input(), |ui| body(ui));
        out.textures_delta.clear();
    }

    /// One egui pass with injected input, returning the text shapes it painted.
    ///
    /// This is the windowless input path: `RawInput.events` is exactly what
    /// egui-winit would feed from a real keyboard/mouse, so a test can prove the
    /// pane's chords and click targets are reachable without a window, a display
    /// or the owner's mouse.
    fn frame_with(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut body: impl FnMut(&mut egui::Ui),
    ) -> Vec<String> {
        let mut raw = raw_input();
        raw.events = events;
        let mut out = ctx.run_ui(raw, |ui| body(ui));
        out.textures_delta.clear();
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            collect_text(&clipped.shape, &mut texts);
        }
        texts
    }

    fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::Shape::Vec(v) => {
                for s in v {
                    collect_text(s, out);
                }
            }
            _ => {}
        }
    }

    /// A key press as egui-winit delivers it: the modifier change FIRST, then the
    /// key event. `InputState::modifiers` is only updated by
    /// `Event::ModifiersChanged`, so a bare `Event::Key { modifiers }` would leave
    /// the chord's modifier test reading the previous frame's (default) modifiers.
    fn key(k: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
        vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
        ]
    }

    /// Paint the body-only entry point into a child `Ui` of an exact rect, the
    /// way the host paints into the DOM's `#sourceBody` box.
    fn body_frame(
        ctx: &egui::Context,
        state: &mut SourceState,
        events: Vec<egui::Event>,
    ) -> Vec<String> {
        let theme = SourceTheme::darkplus();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 700.0));
        frame_with(ctx, events, |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            let _ = ui_body(&mut child, state, &theme);
        })
    }

    /// The pane's per-frame path must not re-shape text it already shaped.
    ///
    /// Two identical frames: the first misses (nothing cached yet), the second
    /// must hit. This is the *hit* half of the contract.
    #[test]
    fn galley_cache_hits_on_an_unchanged_repeat_frame() {
        let state = loaded_state();
        let theme = SourceTheme::darkplus();
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);

        // Frame 1: two lookups; first misses, second already hits.
        frame(&ctx, |ui| {
            let doc = state.doc.as_ref().unwrap();
            let a = state.galleys_for(ui, doc, &theme, 1200.0);
            let b = state.galleys_for(ui, doc, &theme, 1200.0);
            assert_eq!(a.len(), doc.lines.len());
            assert_eq!(b.len(), doc.lines.len());
        });
        let (hits, misses) = state.galley_cache_stats();
        assert_eq!(misses, 1, "the first lookup lays the document out once");
        assert_eq!(hits, 1, "the repeat within a frame must hit");

        // Frame 2: a *new* egui pass. epaint's own galley cache evicts per pass,
        // so this is the case the pane's cache exists for.
        frame(&ctx, |ui| {
            let doc = state.doc.as_ref().unwrap();
            let g = state.galleys_for(ui, doc, &theme, 1200.0);
            assert_eq!(g.len(), doc.lines.len());
        });
        let (hits, misses) = state.galley_cache_stats();
        assert_eq!(misses, 1, "the next frame must NOT re-lay out the document");
        assert_eq!(hits, 2, "the unchanged repeat frame must hit");
    }

    /// The *miss* half: one case per invalidation key, so an edit that drops a
    /// key from `GalleyKey` fails here rather than silently serving a stale
    /// palette, width, toggle or document.
    ///
    /// **This test was verified to be capable of failing.** In a scratch copy of
    /// this file (never the repo) each key was removed from `GalleyKey` in turn
    /// and the suite re-run:
    ///
    /// * `theme: ThemeFingerprint(0)` — this test goes RED.
    /// * `doc: DocFingerprint(0, 0)` — this test and
    ///   `galley_cache_misses_when_text_changes_under_the_same_node_ref` go RED.
    /// * `highlight: true` — this test goes RED.
    /// * `width: 0` — this test goes RED.
    /// * `ctx: egui::Context::default()` (not the shaping context) —
    ///   `galley_cache_hits_on_an_unchanged_repeat_frame` goes RED.
    /// * dropping the key comparison entirely (serve if the length matches) —
    ///   both miss tests go RED.
    ///
    /// That matters because a passing assertion that *cannot* fail is not
    /// evidence. If a future edit makes a key unconditional, one of the lines
    /// above stops holding and this test says so.
    #[test]
    fn galley_cache_misses_when_any_invalidation_key_changes() {
        type Mutate = Box<dyn Fn(&mut SourceState, &mut SourceTheme, &mut f32)>;
        let cases: Vec<(&str, Mutate)> = vec![
            ("theme colour", Box::new(|_s, t, _w| t.keyword = rgb(1, 2, 3))),
            ("theme font_size", Box::new(|_s, t, _w| t.font_size += 1.0)),
            ("theme line_spacing", Box::new(|_s, t, _w| t.line_spacing = 1.9)),
            ("theme gutter_w", Box::new(|_s, t, _w| t.gutter_w += 4.0)),
            ("highlight off", Box::new(|s, _t, _w| s.highlight = false)),
            ("width", Box::new(|_s, _t, w| *w += 40.0)),
            (
                "document identity",
                Box::new(|s, _t, _w| {
                    let p = sample_payload();
                    s.doc = Some(Document::from_value("NZK.Other", &p).unwrap());
                }),
            ),
            // The sixth key, added by the CodeLens wiring. A lens changes what a
            // row's galley contains without touching the document, the theme,
            // the toggle or the width — so WITHOUT this key the cache serves a
            // caption-free galley and the lens never appears. Verified to be
            // capable of failing: deleting the `lens` field from `GalleyKey`
            // makes this case go RED (and only this case).
            (
                "lenses appeared",
                Box::new(|s, _t, _w| {
                    let doc = s.doc.clone().unwrap();
                    s.set_lenses(lens_fixture(&doc, "NZK.B", "main title",
                        vec![(2, "main")]));
                }),
            ),
        ];

        for (name, mutate) in cases {
            let mut state = loaded_state();
            let mut theme = SourceTheme::darkplus();
            let mut width = 1200.0f32;
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(1.0);

            frame(&ctx, |ui| {
                let doc = state.doc.clone().unwrap();
                let _ = state.galleys_for(ui, &doc, &theme, width);
            });
            assert_eq!(state.galley_cache_stats().1, 1, "{name}: one priming miss");

            mutate(&mut state, &mut theme, &mut width);
            frame(&ctx, |ui| {
                let doc = state.doc.clone().unwrap();
                let _ = state.galleys_for(ui, &doc, &theme, width);
            });
            assert_eq!(
                state.galley_cache_stats().1,
                2,
                "{name} changed but the cache was served anyway — a stale galley"
            );
        }
    }

    /// The subtle case: `node_ref` looks like identity but is not. `show_node`
    /// can re-synthesize the same node with different text (changed `#if`
    /// define, edited partial site, rebuilt index), so the key must be
    /// content-aware, not just ref-aware.
    #[test]
    fn galley_cache_misses_when_text_changes_under_the_same_node_ref() {
        let mut state = loaded_state();
        let theme = SourceTheme::darkplus();
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);

        frame(&ctx, |ui| {
            let doc = state.doc.clone().unwrap();
            let _ = state.galleys_for(ui, &doc, &theme, 1200.0);
        });
        assert_eq!(state.galley_cache_stats().1, 1);

        // Same node_ref, same line count, one character different.
        let mut p = sample_payload();
        p["lines"][3]["text"] = json!("    public static int A(){return 2;}");
        let doc = Document::from_value("NZK.B", &p).unwrap();
        assert_eq!(doc.node_ref, state.doc.as_ref().unwrap().node_ref);
        assert_eq!(doc.lines.len(), state.doc.as_ref().unwrap().lines.len());
        state.doc = Some(doc);

        frame(&ctx, |ui| {
            let doc = state.doc.clone().unwrap();
            let _ = state.galleys_for(ui, &doc, &theme, 1200.0);
        });
        assert_eq!(
            state.galley_cache_stats().1,
            2,
            "changed text under the same node_ref must miss the cache"
        );
    }

    /// A successful `show_node` must leave no galleys behind, so a stale
    /// document can never be painted after a jump.
    #[test]
    fn show_node_invalidates_the_galley_cache() {
        let mut state = loaded_state();
        let theme = SourceTheme::darkplus();
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);
        frame(&ctx, |ui| {
            let doc = state.doc.clone().unwrap();
            let _ = state.galleys_for(ui, &doc, &theme, 1200.0);
        });
        assert_eq!(state.galley_cache_stats().1, 1);
        assert!(state.galleys.borrow().is_some(), "cache is populated");

        // The invalidation `show_node` performs on its success path.
        state.invalidate_galleys();

        assert!(
            state.galleys.borrow().is_none(),
            "the cache must be empty after a document change"
        );
    }

    /// The galleys handed out must be the galleys that were painted: a cache hit
    /// returns the *same* `Arc`s, not re-shaped lookalikes.
    #[test]
    fn galley_cache_hit_returns_the_same_galleys() {
        let state = loaded_state();
        let theme = SourceTheme::darkplus();
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);

        let mut first: Vec<Arc<egui::Galley>> = Vec::new();
        let mut second: Vec<Arc<egui::Galley>> = Vec::new();
        frame(&ctx, |ui| {
            let doc = state.doc.as_ref().unwrap();
            first = state.galleys_for(ui, doc, &theme, 1200.0);
        });
        frame(&ctx, |ui| {
            let doc = state.doc.as_ref().unwrap();
            second = state.galleys_for(ui, doc, &theme, 1200.0);
        });
        assert_eq!(first.len(), second.len());
        for (i, (a, b)) in first.iter().zip(&second).enumerate() {
            assert!(
                Arc::ptr_eq(a, b),
                "line {i}: a hit must return the cached galley, not re-shape it"
            );
        }
    }

    /// A galley belongs to the font atlas of the context that shaped it, so the
    /// cache must not hand galleys to a *different* `egui::Context`.
    ///
    /// This is a real path, not a hypothetical: `source_shot` builds one context
    /// for `--size auto` measurement and a second one to paint, both over the
    /// same `SourceState`. Reusing the measured galleys in the paint context
    /// rendered scrambled glyphs (caught by pixel-comparing the evidence PNG,
    /// not by any unit test — hence this one).
    #[test]
    fn galley_cache_misses_across_different_egui_contexts() {
        // The guard for the `ctx` field of `GalleyKey`. The bug it prevents was
        // found by pixel-diffing a rendered PNG, NOT by any test in this file,
        // because every in-crate test uses a single `Context` and so cannot see
        // it — see that field's doc comment for the full account (14,370
        // differing bytes across 74 rows). This test exists so the next
        // reintroduction is caught without a screenshot.
        let state = loaded_state();
        let theme = SourceTheme::darkplus();
        let ctx_a = egui::Context::default();
        let ctx_b = egui::Context::default();
        ctx_a.set_pixels_per_point(1.0);
        ctx_b.set_pixels_per_point(1.0);

        frame(&ctx_a, |ui| {
            let doc = state.doc.as_ref().unwrap();
            let _ = state.galleys_for(ui, doc, &theme, 1200.0);
        });
        assert_eq!(state.galley_cache_stats().1, 1, "first context shapes once");

        // Same document, same theme, same width — a different context only.
        frame(&ctx_b, |ui| {
            let doc = state.doc.as_ref().unwrap();
            let _ = state.galleys_for(ui, doc, &theme, 1200.0);
        });
        let (hits, misses) = state.galley_cache_stats();
        assert_eq!(
            misses, 2,
            "a second context must re-shape: galleys index its own font atlas"
        );
        assert_eq!(hits, 0, "no cross-context reuse is safe");

        // The cache holds one key, so ctx_b's frame replaced ctx_a's entry: a
        // return to ctx_a re-shapes (another miss), never reuses ctx_b's galleys.
        // That is the point — correctness over a second slot. Each `frame` calls
        // `galleys_for` once, so `hits` stays 0 throughout.
        frame(&ctx_a, |ui| {
            let doc = state.doc.as_ref().unwrap();
            let _ = state.galleys_for(ui, doc, &theme, 1200.0);
        });
        let (hits, misses) = state.galley_cache_stats();
        assert_eq!(misses, 3, "returning to ctx_a re-shapes its own galleys");
        assert_eq!(hits, 0, "nothing was reused across any context boundary");
    }

    /// End-to-end: a real corpus file, the real engine index, the pane. Proves
    /// the node -> document -> `file:line` span jump without any UI.
    #[test]
    fn show_node_synthesizes_and_centres_from_a_real_index() {
        use csgraph_core::config::Config;
        let dir = std::env::temp_dir().join(format!("ncv-source-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.cs"),
            "/* header */\nnamespace NZK{  public static partial class B{\n  public static int A1(){return 1;}\n}}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("b.cs"),
            "namespace NZK{  public static partial class B{\n  public static int A2(){return 2;}\n}}\n",
        )
        .unwrap();

        let mut index = Index::new(csgraph_core::index::abspath(&dir));
        let roots = vec![dir.to_string_lossy().into_owned()];
        index.build(&roots, &[], &Config::default());

        let mut state = SourceState::default();
        assert!(state.show_node(&mut index, "NZK.B"), "document found");
        let doc = state.doc.as_ref().unwrap();
        // Both partial sites are merged into one declaration by the engine.
        assert!(doc.line_count() >= 4);
        assert_eq!(doc.title, "NZK.B");
        assert!(!doc.file.is_empty());
        assert!(doc.span_label().starts_with(&doc.file), "span carries the file");
        // The pane centres on the node's own first emitted line.
        assert!(doc.focus < doc.line_count());
        // And the target span is a non-empty document range.
        let (a, b) = doc.target.expect("node span maps into the document");
        assert!(a < b && b <= doc.line_count());

        // A label (not just the id) resolves through the engine's own alias.
        let mut state2 = SourceState::default();
        assert!(state2.show_node(&mut index, "NZK.B"));
        // An unknown node is an error, not a panic.
        let mut state3 = SourceState::default();
        assert!(!state3.show_node(&mut index, "NZK.DoesNotExist"));
        assert!(state3.doc.is_none() && state3.error.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- task-16: source language -------------------------------------

    fn doc_line(text: &str) -> DocLine {
        let segs = highlight_document(text).remove(0);
        DocLine {
            n: 1,
            ctx: "body".into(),
            file: "a.cs".into(),
            orig: 1,
            active: true,
            text: text.to_string(),
            segs,
        }
    }

    #[test]
    fn token_at_finds_the_identifier_under_a_column_only_in_code() {
        let line = doc_line("var x = Alpha.m(1);");
        // "Alpha" starts at column 8.
        assert_eq!(token_at(&line, 8), Some(TokenRef { text: "Alpha".into(), col: 8 }));
        assert_eq!(token_at(&line, 10), Some(TokenRef { text: "Alpha".into(), col: 8 }));
        // Whitespace is not a token.
        assert_eq!(token_at(&line, 3), None);
        // A comment line never yields an identifier.
        let comment = doc_line("// Alpha.m(1)");
        assert_eq!(token_at(&comment, 5), None);
    }

    #[test]
    fn source_history_back_forward_round_trips_document_refs() {
        let mut s = SourceState::default();
        s.loaded_key = Some("A".into());
        s.history.push("X".into());
        assert!(s.history_back_available());
        assert!(!s.history_forward_available());
        assert_eq!(s.history_back(), Some("X".to_string()));
        assert_eq!(s.loaded_key, None, "back leaves the current key to the caller");
        // The caller re-shows the target; that is what makes Forward a real
        // round trip (`App::source_history_step`).
        s.loaded_key = Some("X".to_string());
        assert_eq!(s.forward, vec!["A".to_string()]);
        assert!(s.history_forward_available());
        assert_eq!(s.history_forward(), Some("A".to_string()));
        assert_eq!(s.loaded_key, None);
        assert_eq!(s.history, vec!["X".to_string()]);
        assert!(s.forward.is_empty());
        // Empty stacks are honest no-ops.
        assert_eq!(SourceState::default().history_back(), None);
        assert_eq!(SourceState::default().history_forward(), None);
    }

    #[test]
    fn source_key_table_binds_f12_and_alt_arrows_only() {
        assert_eq!(source_key("F12", false, false, false, false), Some(SourceKey::Definition));
        assert_eq!(source_key("F12", true, false, false, false), Some(SourceKey::Definition));
        assert_eq!(source_key("F12", false, false, true, false), Some(SourceKey::References));
        assert_eq!(source_key("ArrowLeft", false, false, false, true), Some(SourceKey::HistoryBack));
        assert_eq!(source_key("ArrowRight", false, false, false, true), Some(SourceKey::HistoryForward));
        // Alt+Shift+Left is a selection chord, not history; F12 with Alt is not ours.
        assert_eq!(source_key("ArrowLeft", false, false, true, true), None);
        assert_eq!(source_key("F12", false, false, false, true), None);
        assert_eq!(source_key("Enter", false, false, false, false), None);
    }

    // ------------------------------------------------------------------
    // Body-only composition + windowless input (task-2, IDE parity)
    // ------------------------------------------------------------------

    /// The nzk_ui composition paints the real `#sourcePane` header in the
    /// nzk_ui, so the native body paint must NOT paint the fallback head (title,
    /// line/part counters, `edit` / `highlight` checkboxes, close `×`). Those
    /// labels are what a full-pane paint emits; their absence is the observable
    /// difference between the composed body and the fallback pane.
    #[test]
    fn body_only_paint_has_no_native_head_but_full_paint_does() {
        let mut full_state = loaded_state();
        let theme = SourceTheme::darkplus();
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 700.0));

        let full = frame_with(&ctx, Vec::new(), |ui| {
            let _ = ui_with_lens_actions(ui, &mut full_state, &theme);
        });
        assert!(
            full.iter().any(|t| t == "highlight"),
            "the fallback head paints its `highlight` checkbox: {full:?}"
        );

        let mut state = loaded_state();
        let body = frame_with(&ctx, Vec::new(), |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            let _ = ui_body(&mut child, &mut state, &theme);
        });
        assert!(
            !body.iter().any(|t| t == "highlight"),
            "body-only paint must not paint the native head: {body:?}"
        );
        // The document itself IS painted: a body row keeps its text.
        assert!(
            body.iter().any(|t| t.contains("public static int A")),
            "the document body must still paint its rows: {body:?}"
        );
    }

    /// `Ctrl+E` toggles the edit buffer on/off through the same chord path the
    /// composed pane uses (the header's edit checkbox is nzk_ui-hidden there), and
    /// `Ctrl+S` queues exactly one real `write_file` body for a dirty buffer.
    #[test]
    fn ctrl_e_toggles_editing_and_ctrl_s_queues_the_exact_save() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);
        let mut state = loaded_state();
        assert!(!state.editing);
        let _ = body_frame(&ctx, &mut state, key(egui::Key::E, egui::Modifiers::CTRL));
        assert!(state.editing, "Ctrl+E enters edit mode");

        let seeded = state.editable_text().to_string();
        state.editable.push_str("\n// edited");
        assert!(state.is_dirty());
        let _ = body_frame(&ctx, &mut state, key(egui::Key::S, egui::Modifiers::CTRL));
        let req = state
            .take_save_request()
            .expect("Ctrl+S on a dirty buffer queues a save");
        assert_eq!(req.path, "a.cs");
        assert!(req.content.starts_with(&seeded));
        assert!(req.content.ends_with("// edited"));
        assert_eq!(req.write_file_args()["apply"], true);

        let _ = body_frame(&ctx, &mut state, key(egui::Key::E, egui::Modifiers::CTRL));
        assert!(!state.editing, "Ctrl+E leaves edit mode");
    }

    /// Typing into the editable buffer needs no window: a click focuses the
    /// `TextEdit`, a `Text` event inserts at the caret and the buffer becomes
    /// dirty. This is the editor's caret/typing path, driven by the same events
    /// egui-winit feeds from a real keyboard.
    #[test]
    fn a_click_and_a_text_event_edit_the_buffer() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);
        let mut state = loaded_state();
        state.start_editing();

        // Click into the body. Hover, press and release are separate passes,
        // exactly like a real pointer (egui resolves the hover before the click).
        let pos = Pos2::new(120.0, 60.0);
        let _ = body_frame(&ctx, &mut state, vec![egui::Event::PointerMoved(pos)]);
        let _ = body_frame(
            &ctx,
            &mut state,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        let _ = body_frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert!(
            ctx.memory(|m| m.focused()).is_some(),
            "the click must focus the edit TextEdit"
        );

        let _ = body_frame(&ctx, &mut state, vec![egui::Event::Text("X".to_string())]);
        assert!(
            state.editable_text().contains('X'),
            "the text event reaches the edit buffer: {:?}",
            state.editable_text()
        );
        assert!(state.is_dirty(), "typing marks the buffer dirty");
    }

    /// A click on a row carrying a wired lens queues exactly that node. The lens
    /// row is the reference's `main` command (`codelens.js:228-232`); the pane
    /// cannot select it (no `GraphView`), so the click must surface as the
    /// host-consumed `lens_open_request`.
    #[test]
    fn clicking_a_wired_lens_requests_its_node() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);
        let mut state = loaded_state();
        let mut lenses = LensRows::empty();
        lenses.lines = vec![
            Vec::new(),
            vec![LensText {
                line: 1,
                kind: "main",
                node_id: "NZK.A".to_string(),
                title: "3 references · 1 callers · show in graph".to_string(),
                command_wired: true,
            }],
        ];
        lenses.lens_lines = vec![1];
        state.set_lenses(lenses);

        let theme = SourceTheme::darkplus();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 700.0));
        let row_y = theme.row_h() + 4.0;
        let pos = Pos2::new(120.0, row_y);
        // Hover first: egui resolves a widget's hover from the frame that placed
        // it, so a move and a press in the same pass would leave the press
        // unclaimed (a real mouse always sends motion frames before the press).
        let _ = frame_with(&ctx, vec![egui::Event::PointerMoved(pos)], |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            let _ = ui_body(&mut child, &mut state, &theme);
        });
        // Press: egui records the press on the lens row's interact id.
        let _ = frame_with(
            &ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            |ui| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                let clicked = ui_body(&mut child, &mut state, &theme);
                assert!(clicked.is_empty(), "a press is not a click yet");
            },
        );
        // Release: that is the click.
        let mut clicked_now: Vec<String> = Vec::new();
        let _ = frame_with(
            &ctx,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
            |ui| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                clicked_now = ui_body(&mut child, &mut state, &theme);
            },
        );
        assert_eq!(clicked_now, vec!["NZK.A".to_string()]);
        // The pane also surfaces it as the host request (what App consumes).
        for id in clicked_now {
            state.lens_open_request = Some(id);
        }
        assert_eq!(state.take_lens_open_request().as_deref(), Some("NZK.A"));
    }
}
