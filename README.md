# nzkide_module

The VSCode-like **source IDE pane** for cs viewer v3, in Rust + `egui`.
It is the third addon beside the tokenizer (`def2mcpfnnumber`) and Scratch
(`scratch_rs_ide_addon`); the v3 toolkit depends on all three by path.

## Files

| File | Role |
|---|---|
| `src/gui.nzkide.rs` | `SourceTheme`, the synthesized `Document` model, `SourceState`, the pane, edit mode and the galley cache. |
| `src/highlight.nzkide.rs` | C# highlighting. Comments/strings come from the engine's own `mask_noncode`; code words are classified by the shared tokenizer. |
| `src/idelink.nzkide.rs` | `Language` — the host's graph facts (hover, go-to-definition, references). Pure data; no `Graph` type here. |
| `src/lib.rs` | `#[path]` declarations for the dotted filenames. |

## Colour contract

The pane does **not** own a private keyword table. `highlight.nzkide.rs` maps
the tokenizer addon's `TokenKind` onto its `Tok`, and `SourceTheme::tok_color`
maps that onto the Dark+ palette. Scratch maps the same `TokenKind` through the
same colours. A word therefore cannot be one colour in the editor and another
in a Scratch piece.

`head` / `tail` rows are real declarations. They keep their token colours; only
the generated `part` marker is a comment and only `cond` is preprocessor.

## Host wiring

```text
host Graph ──adapter──▶ &dyn Language
host Index ──doc_source──▶ Document ──▶ SourceState ──▶ pane
pane click ──▶ Vec<String> (node ids the host may select)
```

The host owns graph selection, the engine `Index`, code-lens computation and
write transport. The pane owns pixels, layout, edit buffer/dirty state and the
cache. That split is why the pane has no `model::Graph` in its dependency tree.

## Tests

```sh
cargo test
```

The suite covers the document payload, theme metrics, the galley cache
(hit/miss and every invalidation key), the edit buffer and exact save payload,
the shared tokenizer's colours, and a headless `egui` paint. The graph-driven
tests live in the host, where the `Language` implementation lives.
