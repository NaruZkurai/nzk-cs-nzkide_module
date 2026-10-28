//! C# token highlighting for the native source pane.
//!
//! The engine already owns the authoritative answer to "what is a comment or a
//! string literal": [`csgraph_core::lex::mask_noncode`] blanks every `//`
//! comment, `/* */` comment, char literal and (verbatim / interpolated) string
//! literal while preserving every character offset, and the parser, graph build
//! and document synthesis all run on that masked text. Rather than ship a
//! second, subtly different lexer — and risk text the engine parsed as code
//! being coloured as a comment, or the reverse — this module *starts* from
//! `mask_noncode` and only classifies the code it leaves behind.
//!
//! Span ends reuse the engine's own primitives (`scan_lit`, `find_sub`), so a
//! highlighted span can never end somewhere the mask disagrees with.
//!
//! Everything here is pure: `&str` in, per-line segments out. No egui, no
//! engine index, no I/O — which is what lets the headless PNG path and the
//! window share one highlighter.


// A binary crate sees one subset of this API, so the whole public surface is
// not always used; the shell keeps the rest.
#![allow(dead_code)]
use csgraph_core::lex::{is_literal_start, mask_noncode, scan_lit};
use csgraph_core::model::{find_sub, find_sub_from, to_chars};
use def2mcpfnnumber::syntax::{self, TokenKind};

/// A token class the pane can colour. Values mirror the original viewer's
/// `.tok-*` / Monaco `--src-*` classes (see `frontend/static/style.css`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    /// Ordinary identifier / whitespace (`.tok-plain`).
    Plain,
    Comment,
    Str,
    Keyword,
    /// `if`/`for`/`return`/… — Monaco `keyword.control`.
    Control,
    /// Upper-initial identifier used as a type (`NZK.B`, `System.String`).
    Type,
    Number,
    /// Identifier immediately followed by `(` — Monaco `identifier.function`.
    Function,
    /// Reserved for the `variable` token; currently unused by the heuristics.
    Variable,
    /// Punctuation and operators (`.tok-op`).
    Op,
    /// A preprocessor directive (`#if` / `#endif`); the pane's `ctx == cond`
    /// lines are drawn with this class regardless of the tokeniser.
    Preproc,
}

/// One run of characters with one class.
#[derive(Clone, Debug)]
pub struct Segment {
    pub text: String,
    pub tok: Tok,
}

/// Highlight a whole synthesized document and split it into one `Vec<Segment>`
/// per line.
///
/// The returned vector has exactly `text.split('\n').count()` entries; the
/// newline characters themselves are not part of any segment.
pub fn highlight_document(text: &str) -> Vec<Vec<Segment>> {
    let src = to_chars(text);
    let mask = mask_noncode(&src);
    let spans = coarse_spans(&src, &mask);

    // Line starts (character offsets). The last line is `starts[n-1]..len`.
    let mut starts: Vec<usize> = vec![0];
    for (i, &c) in src.iter().enumerate() {
        if c == '\n' {
            starts.push(i + 1);
        }
    }
    let nlines = starts.len();
    let line_end = |l: usize| -> usize {
        if l + 1 < nlines {
            starts[l + 1] - 1
        } else {
            src.len()
        }
    };

    let mut lines: Vec<Vec<Segment>> = vec![Vec::new(); nlines];
    for span in &spans {
        if span.r.start >= span.r.end {
            continue;
        }
        // The first line whose start offset is <= the span start.
        let mut l = starts.partition_point(|&s| s <= span.r.start).saturating_sub(1);
        while l < nlines && starts[l] < span.r.end {
            let a = span.r.start.max(starts[l]);
            let b = span.r.end.min(line_end(l));
            if a < b {
                match span.class {
                    Class::Comment => push(&mut lines[l], collect(&src, a, b), Tok::Comment),
                    Class::Str => push(&mut lines[l], collect(&src, a, b), Tok::Str),
                    Class::Code => tokenize_code(&src, a, b, &mut lines[l]),
                }
            }
            l += 1;
        }
    }
    lines
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Code,
    Comment,
    Str,
}

struct CoarseSpan {
    r: std::ops::Range<usize>,
    class: Class,
}

/// The engine's non-code regions, as spans.
///
/// The walk mirrors `mask_noncode` branch for branch: a `//` or `/*` start is
/// only taken when the mask says that character was blanked, and a literal only
/// when `is_literal_start` says so and the mask agrees. That makes this a
/// *classification* of the engine's own decision, not a second lexer.
fn coarse_spans(src: &[char], mask: &[char]) -> Vec<CoarseSpan> {
    let n = src.len();
    let mut out: Vec<CoarseSpan> = Vec::new();
    let mut code_start = 0usize;
    let mut i = 0usize;
    while i < n {
        if src[i] == '/' && i + 1 < n && src[i + 1] == '/' && mask[i] == ' ' {
            push_code(&mut out, code_start, i);
            let j = find_sub_from(src, &['\n'], i).unwrap_or(n);
            out.push(CoarseSpan { r: i..j, class: Class::Comment });
            i = j;
            code_start = i;
            continue;
        }
        if src[i] == '/' && i + 1 < n && src[i + 1] == '*' && mask[i] == ' ' {
            push_code(&mut out, code_start, i);
            let j = find_sub(src, &['*', '/'], i + 2, n)
                .map(|v| v + 2)
                .unwrap_or(n);
            out.push(CoarseSpan { r: i..j, class: Class::Comment });
            i = j;
            code_start = i;
            continue;
        }
        if is_literal_start(src, i, n) && mask[i] == ' ' {
            push_code(&mut out, code_start, i);
            let j = scan_lit(src, i, n);
            out.push(CoarseSpan { r: i..j, class: Class::Str });
            i = j;
            code_start = i;
            continue;
        }
        i += 1;
    }
    push_code(&mut out, code_start, n);
    out
}

fn push_code(out: &mut Vec<CoarseSpan>, a: usize, b: usize) {
    if a < b {
        out.push(CoarseSpan { r: a..b, class: Class::Code });
    }
}

fn collect(src: &[char], a: usize, b: usize) -> String {
    src[a.min(src.len())..b.min(src.len())].iter().collect()
}

fn push(out: &mut Vec<Segment>, text: String, tok: Tok) {
    if text.is_empty() {
        return;
    }
    // Merge adjacent runs of the same class so a LayoutJob has few sections.
    if let Some(last) = out.last_mut() {
        if last.tok == tok {
            last.text.push_str(&text);
            return;
        }
    }
    out.push(Segment { text, tok });
}

/// Map the shared tokenizer's class onto this pane's `Tok`.
///
/// The tokenizer lives in the tokenizer addon (`def2mcpfnnumber::syntax`) and
/// is the same table Scratch uses. This pane therefore cannot drift from the
/// puzzle pieces: if a word's colour differs, the theme differs.
fn tok_of(kind: TokenKind) -> Tok {
    match kind {
        TokenKind::Plain => Tok::Plain,
        TokenKind::Comment => Tok::Comment,
        TokenKind::Str => Tok::Str,
        TokenKind::Keyword => Tok::Keyword,
        TokenKind::Control => Tok::Control,
        TokenKind::Type => Tok::Type,
        TokenKind::Number => Tok::Number,
        TokenKind::Function => Tok::Function,
        TokenKind::Variable => Tok::Variable,
        TokenKind::Op => Tok::Op,
        TokenKind::Preproc => Tok::Preproc,
    }
}

/// Classify one range of code-only characters (`[a, b)` contains no comment /
/// literal characters and no newline).
///
/// The ranges come from the engine's `mask_noncode`; the *classification* is
/// the shared tokenizer, which is also what Scratch colours its blocks with.
fn tokenize_code(src: &[char], a: usize, b: usize, out: &mut Vec<Segment>) {
    let text: String = src[a.min(src.len())..b.min(src.len())].iter().collect();
    for span in syntax::tokens(&text) {
        let run = text[span.start..span.end].to_string();
        push(out, run, tok_of(span.kind));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(lines: &[Vec<Segment>]) -> String {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn round_trips_every_character() {
        let text = "namespace NZK{  public static partial class X{\n  public static int A(){return 1;}\n}}\n";
        let hl = highlight_document(text);
        assert_eq!(joined(&hl), text, "highlighting never loses or moves text");
        assert_eq!(hl.len(), text.split('\n').count());
    }

    #[test]
    fn comments_and_strings_match_the_engine_mask() {
        // `//` inside a string is string, not comment; `/*` inside a comment is
        // still one comment. This is exactly mask_noncode's decision.
        let text = "var s = \"http://x\"; // trailing\n/* block\n   still block */ var t = @\"a\"\"b\";\n";
        let hl = highlight_document(text);
        // line 0: the string span is Str, the trailing `// trailing` is Comment.
        assert!(hl[0].iter().any(|s| s.tok == Tok::Str && s.text == "\"http://x\""));
        assert!(hl[0].iter().any(|s| s.tok == Tok::Comment && s.text == "// trailing"));
        // line 1 is entirely inside a block comment.
        assert_eq!(hl[1].len(), 1);
        assert_eq!(hl[1][0].tok, Tok::Comment);
        // line 2: the comment closes, then a verbatim string is one Str span.
        assert!(hl[2].iter().any(|s| s.tok == Tok::Str && s.text == "@\"a\"\"b\""));
    }

    #[test]
    fn classifies_keywords_types_and_numbers() {
        let text = "public static int A(){ return 0x1F; }\n";
        let hl = highlight_document(text);
        assert_eq!(hl.len(), 2, "trailing newline yields an empty last line");
        let flat: Vec<(String, Tok)> = hl[0]
            .iter()
            .map(|s| (s.text.clone(), s.tok))
            .collect();
        assert!(flat.contains(&("public".to_string(), Tok::Keyword)));
        assert!(flat.contains(&("static".to_string(), Tok::Keyword)));
        assert!(flat.contains(&("int".to_string(), Tok::Keyword)));
        assert!(flat.contains(&("return".to_string(), Tok::Control)));
        assert!(flat.contains(&("0x1F".to_string(), Tok::Number)));
        assert!(flat.contains(&("A".to_string(), Tok::Function)));
    }

    /// The reported defect: a synthesized document's `head` lines (the real
    /// `namespace` / class declarations) were painted as comments by the pane.
    /// The highlighter must call `namespace` a keyword and its name a type, so
    /// the row painter cannot inherit a comment colour from the tokenizer.
    #[test]
    fn a_namespace_header_is_code_not_a_comment() {
        let text = "namespace NZK.SPS2 {\n";
        let hl = highlight_document(text);
        assert!(
            hl[0].iter()
                .any(|s| s.tok == Tok::Keyword && s.text == "namespace"),
            "namespace is a keyword: {:?}",
            hl[0]
        );
        assert!(
            hl[0].iter().any(|s| s.tok == Tok::Type && s.text == "NZK"),
            "the namespace name is a type, not comment text: {:?}",
            hl[0]
        );
        assert!(
            !hl[0].iter().any(|s| s.tok == Tok::Comment),
            "nothing on a namespace line is a comment: {:?}",
            hl[0]
        );
    }

    #[test]
    fn doc_dialect_literals_are_coloured() {
        // The viewer dialect: /* */ comments only, fully-qualified names.
        let text = "/* B = Bool helpers. */\nnamespace NZK{  public static partial class B{\n  public static bool t(NZK.B.I.A x){return false;}\n}}\n";
        let hl = highlight_document(text);
        assert_eq!(hl[0][0].tok, Tok::Comment);
        assert!(hl[1].iter().any(|s| s.tok == Tok::Type && s.text == "NZK"));
        assert!(hl[2].iter().any(|s| s.tok == Tok::Keyword && s.text == "bool"));
    }
}
