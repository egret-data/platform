//! Rewrites Kestra's operator syntax into syntax minijinja can parse.
//!
//! minijinja has no `??`, `???`, or C-style ternary `? :`. It does have `in`
//! (a candidate, provisional target for `isIn` — see below) and an inline
//! `a if c else b` conditional. This module's `rewrite` turns Kestra source
//! into minijinja source; it never evaluates anything, because it has no
//! context to evaluate against — the values live in the caller's render call,
//! not here.
//!
//! # The two coalescing operators are not the same
//!
//! Read from `core/src/main/java/io/kestra/core/runners/pebble/expression/`
//! in the `kestra` checkout (pinned `96893b589`):
//!
//! - **`??`** (`NullCoalescingExpression`) — evaluate the left side. If it
//!   evaluates to a non-null value, return it. If it evaluates to `null`
//!   **or** throws `AttributeNotFoundException` (the attribute/variable is
//!   missing), return the right side instead. Null and "missing" are treated
//!   the same.
//! - **`???`** (`UndefinedCoalescingExpression`) — evaluate the left side and
//!   return it *even when the result is `null`*. Only an
//!   `AttributeNotFoundException` — the attribute is missing, not merely
//!   `null` — falls through to the right side.
//!
//! `oracle/golden.json`'s `coalesce-null` (`missing ?? 'fallback'` with
//! `missing: null`) is `"fallback"`, while `undef-coalesce-null`
//! (`missing ??? 'fallback'`, same context) is `""` (Pebble's empty-string
//! rendering of `null`) — the measured proof that collapsing the two
//! operators is wrong. `coalesce-absent` and `undef-coalesce-absent` (the key
//! is not in the context at all) both resolve to `"fallback"`, because a
//! missing attribute is the one case both operators agree on.
//!
//! Kestra registers these as binary operators, not functions
//! (`Extension.getBinaryOperators()`), but minijinja functions receive their
//! arguments already evaluated — there is no lazy "don't evaluate the right
//! side unless needed" hook available to a rewrite that only reshapes text.
//! `rewrite` therefore produces plain function *calls* —
//! `kestra_null_coalesce(a, b)` / `kestra_undefined_coalesce(a, b)` — and
//! leaves it to whatever registers those functions (Task 3) to give them the
//! right short-circuit/attribute-missing semantics. This module does not
//! attempt that; it only reshapes syntax.
//!
//! # Why a tokenizer, not a regex
//!
//! Things that defeat a string-substitution rewrite, and are live corpus
//! entries (`oracle/corpus.json`) or measured review findings:
//!
//! - `??` (or `? ... :`) can appear *inside a string literal* —
//!   `{{ 'a ?? b' }}` must render `a ?? b` unchanged, not
//!   `a kestra_null_coalesce b`. A regex has no concept of "inside a string."
//! - `???` overlaps `??` — a naive left-to-right regex replace of `??` first
//!   would consume the first two `?` of a `???` and misread the operator.
//!   Matching must be longest-first.
//! - Ternaries nest, with or without parens (`a ? (b ? 'x' : 'y') : 'z'`),
//!   and Kestra's ternary (like Java's) is right-associative
//!   (`a ? b : c ? d : e` is `a ? b : (c ? d : e)`).
//! - `,` inside a call/list/map is not an operand boundary a naive split on
//!   an operator would respect — `f(x, a ?? b)` must rewrite the second
//!   *argument*, not swallow `x` into the coalesce's left operand.
//! - `{'k': v}` map literals use `:`, the same character the ternary uses,
//!   and nest inside `{{ }}` alongside `(`/`[` groups.
//!
//! So this module tokenizes: string literals (both quote styles, with
//! backslash escapes), `(`/`)`/`[`/`]`/`{`/`}` for nesting, and the operators
//! matched longest-first (`???` before `??` before `?`; `==`/`!=`/`<=`/`>=`
//! before their single-char prefix, so a lone `=` unambiguously means
//! assignment, never half of `==`; `isIn` matched as a whole identifier, not
//! a substring, so it never fires inside e.g. `thisIsInvalid`). Everything
//! else — identifiers, numbers, whitespace, dots, `|`, the untouched
//! comparison operators — is copied through byte-for-byte.
//!
//! # `isIn` — provisional
//!
//! `x isIn y` is rewritten to `(x in y)`. Whether minijinja's `in` actually
//! agrees with Kestra's `InExpression` on every operand pair (`InExpression`
//! stringifies both sides via `Object::toString()` before comparing, which
//! `in` may or may not do) is **not** decided here — it is Task 3's oracle
//! measurement. If it disagrees, `isIn` becomes a third custom function,
//! same shape as the two coalescing operators above.
//!
//! # Comparison operators are left alone
//!
//! `>`, `>=`, `<`, `<=` already parse in minijinja and this module does not
//! touch them. Kestra overrides all four (`GreaterThanExpression` and its
//! three siblings) to add string comparison
//! (`sLeft.compareTo(sRight) > 0`) on top of Pebble's numeric-only default,
//! falling through to `OperatorUtils.gt` (numeric) for anything else —
//! including a same-shape `PebbleException` when the operands are a
//! String/Number mix that default cannot compare (`oracle/golden.json`'s
//! `gt-mixed`, `{{ '2' > 10 }}`, is captured as an `error`, not a coerced
//! value). Whether minijinja's own `>` agrees with any of this is Task 3's
//! measurement to make, not this rewrite's to guess at. `==`/`!=`/`<=`/`>=`
//! are tokenized as single units purely so a bare `=` (see `{% set %}` below)
//! is never ambiguous with the first half of `==` -- they are not otherwise
//! acted on.
//!
//! # `{{ }}` expressions vs. `{% %}` tags
//!
//! A `{{ }}` region's entire content is one expression — [`rewrite_expression`]
//! tokenizes and resolves it as such.
//!
//! A `{% %}` tag's body is *not* one expression: it opens with a keyword
//! (`if`, `set`, `for`, `endif`, ...) that is never itself an operand, and
//! two of Pebble's tags have their own structural keyword splitting the
//! body in two: `{% set x = <expr> %}` (the target(s) left of `=` are names,
//! not rewritten; the value right of `=` is) and
//! `{% for x in <expr> %}` (the loop variable(s) left of `in` are names; the
//! iterable right of `in` is). [`rewrite_tag_body`] skips the leading
//! keyword, and for `for`/`set` specifically, further finds that keyword's
//! own top-level boundary token (a bare `in`/`=`, not nested inside a
//! group) before rewriting only the value/iterable side. Every other tag
//! (`if`, `elseif`, `unless`, `endif`, `else`, ...) has its entire remainder
//! rewritten as one expression, same as a `{{ }}` region.

use thiserror::Error;

/// The rewriter failed to make sense of an expression's tokens.
///
/// This is deliberately narrow: unrecognised syntax (comparison operators,
/// filters, function calls, arbitrary identifiers, unmatched parens/brackets)
/// is left untouched rather than rejected — see the module doc. The one case
/// this rewriter treats as a hard error is a string literal that never finds
/// its closing quote before its enclosing `{{ }}` / `{% %}` block ends: at
/// that point the tokenizer genuinely cannot tell where the literal ends, and
/// producing "rewritten" output from that guess would be worse than refusing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RewriteError {
    #[error("unterminated string literal (opened with {quote:?}) in expression: {snippet}")]
    UnterminatedString { quote: char, snippet: String },
}

/// Rewrites a Kestra expression template into one minijinja can parse.
///
/// `expr` is a whole template string — plain text interleaved with zero or
/// more `{{ ... }}` / `{% ... %}` regions. Only the insides of those regions
/// are tokenized and rewritten; everything else (including a bare `??` that
/// never appears inside `{{ }}`/`{% %}` at all) is copied through unchanged.
/// A `{{ }}` region is rewritten as one expression
/// ([`rewrite_expression`]); a `{% %}` region is rewritten as a tag body
/// ([`rewrite_tag_body`]) — see this module's doc for why those are not the
/// same job.
pub fn rewrite(expr: &str) -> Result<String, RewriteError> {
    let mut out = String::with_capacity(expr.len());
    let len = expr.len();
    let mut i = 0usize;

    loop {
        let next_expr = expr[i..].find("{{");
        let next_stmt = expr[i..].find("{%");
        let found = match (next_expr, next_stmt) {
            (None, None) => None,
            (Some(e), None) => Some((e, "}}")),
            (None, Some(s)) => Some((s, "%}")),
            (Some(e), Some(s)) if e <= s => Some((e, "}}")),
            (Some(_), Some(s)) => Some((s, "%}")),
        };

        let Some((rel, close)) = found else {
            out.push_str(&expr[i..]);
            break;
        };

        let delim_start = i + rel;
        out.push_str(&expr[i..delim_start]);
        let open = &expr[delim_start..delim_start + 2];
        let content_start = delim_start + 2;

        match find_region_close(expr, content_start, close)? {
            Some(close_start) => {
                let inner = &expr[content_start..close_start];
                let rewritten = if open == "{{" {
                    rewrite_expression(inner)?
                } else {
                    rewrite_tag_body(inner)?
                };
                out.push_str(open);
                out.push_str(&rewritten);
                out.push_str(close);
                i = close_start + close.len();
            }
            // No closing delimiter before the end of input, and no
            // unterminated string caused that (find_region_close would have
            // raised `?` above for that case): a tag that just never closes.
            // Not this rewriter's job to diagnose (the parser it feeds will)
            // -- copy the remainder through untouched.
            None => {
                out.push_str(&expr[delim_start..]);
                break;
            }
        }

        if i >= len {
            break;
        }
    }

    Ok(out)
}

/// Scans forward from `start` for `close` (`"}}"` or `"%}"`), skipping over
/// string-literal contents (bounded to this same tag -- see
/// [`scan_string_literal_bounded`]) and brace-literal nesting, so neither a
/// quoted `}}` nor a map literal's own closing `}` immediately followed by
/// the region's `}}` can terminate the region early or late. Returns the
/// byte offset where `close` begins, or `Ok(None)` if `close` never appears
/// before the end of input.
///
/// This is also where an unterminated string literal is actually caught for
/// anything reachable through the public `rewrite` entry point: by the time
/// a region's content is handed to `tokenize`, every quote inside it is
/// already known to be balanced (bounded to this tag, not the rest of the
/// document -- see below). `tokenize`'s own check stays as defense in depth
/// (and documents the invariant), but it is this function's `Err` that a
/// caller of `rewrite` actually observes for `{{ a ?? 'unterminated }}`.
fn find_region_close(src: &str, start: usize, close: &str) -> Result<Option<usize>, RewriteError> {
    let len = src.len();
    let mut i = start;
    let mut brace_depth = 0i32;
    while i < len {
        if brace_depth == 0 && src[i..].starts_with(close) {
            return Ok(Some(i));
        }
        let c = src[i..].chars().next().expect("i < len");
        if c == '\'' || c == '"' {
            let str_start = i;
            let (end, closed) = scan_string_literal_bounded(src, i, c, close);
            if !closed {
                return Err(RewriteError::UnterminatedString {
                    quote: c,
                    snippet: src[str_start..end].to_string(),
                });
            }
            i = end;
            continue;
        }
        if c == '{' {
            brace_depth += 1;
            i += c.len_utf8();
            continue;
        }
        if c == '}' {
            if brace_depth > 0 {
                brace_depth -= 1;
            }
            i += c.len_utf8();
            continue;
        }
        i += c.len_utf8();
    }
    Ok(None)
}

/// Scans a string literal's body starting at `start` (the opening quote).
/// Returns the byte offset right after the literal and whether it actually
/// found a matching closing `quote` before the end of input — `false` means
/// the scan ran off the end of `src` looking for one.
fn scan_string_literal(src: &str, start: usize, quote: char) -> (usize, bool) {
    let len = src.len();
    let mut i = start + quote.len_utf8();
    while i < len {
        let c = src[i..].chars().next().expect("i < len");
        if c == '\\' {
            i += c.len_utf8();
            if i < len {
                let escaped = src[i..].chars().next().expect("i < len");
                i += escaped.len_utf8();
            }
            continue;
        }
        i += c.len_utf8();
        if c == quote {
            return (i, true);
        }
    }
    (i, false)
}

/// Like [`scan_string_literal`], but additionally bounded by `close` (the
/// enclosing tag's own `"}}"`/`"%}"`): the literal is considered
/// unterminated the moment `close` appears before a matching quote does,
/// regardless of whether some later, unrelated quote elsewhere in the
/// document would otherwise have "closed" it. Without this bound, whether a
/// broken literal inside one tag reports an error depends on accidents of
/// unrelated later text (an apostrophe in a contraction, say) -- see this
/// module's tests for the case that motivated it. Used only by
/// [`find_region_close`], which is the only caller with a `close` to bound
/// against.
fn scan_string_literal_bounded(src: &str, start: usize, quote: char, close: &str) -> (usize, bool) {
    let len = src.len();
    let mut i = start + quote.len_utf8();
    while i < len {
        if src[i..].starts_with(close) {
            return (i, false);
        }
        let c = src[i..].chars().next().expect("i < len");
        if c == '\\' {
            i += c.len_utf8();
            if i < len {
                let escaped = src[i..].chars().next().expect("i < len");
                i += escaped.len_utf8();
            }
            continue;
        }
        i += c.len_utf8();
        if c == quote {
            return (i, true);
        }
    }
    (i, false)
}

/// One lexical unit inside a `{{ }}` / `{% %}` region. Every token stores its
/// own exact source text, so a region with nothing to rewrite reconstructs
/// byte-for-byte identical to the input.
#[derive(Debug, Clone)]
enum Tok {
    Str(String),
    LParen(String),
    RParen(String),
    LBracket(String),
    RBracket(String),
    LBrace(String),
    RBrace(String),
    NullCoalesce(String),
    UndefinedCoalesce(String),
    Question(String),
    Colon(String),
    IsIn(String),
    /// Identifiers, numbers, whitespace, and any other punctuation this
    /// rewrite does not act on (`.`, `,`, `|`, `>`, `<`, `==`, `!=`, `<=`,
    /// `>=`, a bare `=`, etc).
    Text(String),
}

impl Tok {
    fn text(&self) -> &str {
        match self {
            Tok::Str(s)
            | Tok::LParen(s)
            | Tok::RParen(s)
            | Tok::LBracket(s)
            | Tok::RBracket(s)
            | Tok::LBrace(s)
            | Tok::RBrace(s)
            | Tok::NullCoalesce(s)
            | Tok::UndefinedCoalesce(s)
            | Tok::Question(s)
            | Tok::Colon(s)
            | Tok::IsIn(s)
            | Tok::Text(s) => s,
        }
    }
}

/// Tokenizes the inside of one `{{ }}` / `{% %}` region.
fn tokenize(src: &str) -> Result<Vec<Tok>, RewriteError> {
    let mut toks = Vec::new();
    let len = src.len();
    let mut i = 0usize;

    while i < len {
        let c = src[i..].chars().next().expect("i < len");
        match c {
            '\'' | '"' => {
                let start = i;
                let quote = c;
                // Reached only for content `find_region_close` already
                // walked (see its doc comment) -- every quote here is
                // already known to close. Kept as defense in depth: if
                // `tokenize` is ever called on unvalidated input, this still
                // refuses to guess rather than silently rewrite past a
                // broken literal.
                let (end, closed) = scan_string_literal(src, i, quote);
                if !closed {
                    return Err(RewriteError::UnterminatedString {
                        quote,
                        snippet: src[start..].to_string(),
                    });
                }
                toks.push(Tok::Str(src[start..end].to_string()));
                i = end;
            }
            '(' => {
                toks.push(Tok::LParen("(".to_string()));
                i += 1;
            }
            ')' => {
                toks.push(Tok::RParen(")".to_string()));
                i += 1;
            }
            '[' => {
                toks.push(Tok::LBracket("[".to_string()));
                i += 1;
            }
            ']' => {
                toks.push(Tok::RBracket("]".to_string()));
                i += 1;
            }
            '{' => {
                toks.push(Tok::LBrace("{".to_string()));
                i += 1;
            }
            '}' => {
                toks.push(Tok::RBrace("}".to_string()));
                i += 1;
            }
            '?' => {
                // Longest-match-first: `???` must never be read as `??`
                // followed by a bare `?`.
                if src[i..].starts_with("???") {
                    toks.push(Tok::UndefinedCoalesce("???".to_string()));
                    i += 3;
                } else if src[i..].starts_with("??") {
                    toks.push(Tok::NullCoalesce("??".to_string()));
                    i += 2;
                } else {
                    toks.push(Tok::Question("?".to_string()));
                    i += 1;
                }
            }
            ':' => {
                toks.push(Tok::Colon(":".to_string()));
                i += 1;
            }
            // `==`/`!=`/`<=`/`>=` grouped as single tokens -- longest-match,
            // same principle as `???`/`??`/`?` -- purely so a standalone `=`
            // (the `{% set x = ... %}` boundary) is never ambiguous with
            // half of `==`. Not otherwise acted on: comparison operators are
            // left alone (see module doc).
            '=' => {
                if src[i..].starts_with("==") {
                    toks.push(Tok::Text("==".to_string()));
                    i += 2;
                } else {
                    toks.push(Tok::Text("=".to_string()));
                    i += 1;
                }
            }
            '!' => {
                if src[i..].starts_with("!=") {
                    toks.push(Tok::Text("!=".to_string()));
                    i += 2;
                } else {
                    toks.push(Tok::Text("!".to_string()));
                    i += 1;
                }
            }
            '<' => {
                if src[i..].starts_with("<=") {
                    toks.push(Tok::Text("<=".to_string()));
                    i += 2;
                } else {
                    toks.push(Tok::Text("<".to_string()));
                    i += 1;
                }
            }
            '>' => {
                if src[i..].starts_with(">=") {
                    toks.push(Tok::Text(">=".to_string()));
                    i += 2;
                } else {
                    toks.push(Tok::Text(">".to_string()));
                    i += 1;
                }
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                while i < len {
                    let c2 = src[i..].chars().next().expect("i < len");
                    if c2.is_alphanumeric() || c2 == '_' {
                        i += c2.len_utf8();
                    } else {
                        break;
                    }
                }
                let word = &src[start..i];
                // Matched as a whole identifier, not a substring -- `isIn`
                // must not fire inside e.g. `thisIsInvalid`.
                if word == "isIn" {
                    toks.push(Tok::IsIn(word.to_string()));
                } else {
                    toks.push(Tok::Text(word.to_string()));
                }
            }
            other => {
                toks.push(Tok::Text(other.to_string()));
                i += other.len_utf8();
            }
        }
    }

    Ok(toks)
}

/// A flattened unit at one nesting level: either verbatim text (including a
/// fully-resolved, already-rewritten parenthesized/bracketed/braced group)
/// or one of the operator tokens this rewrite acts on. A `,` (used to find
/// top-level argument/element/pair boundaries) and a map-literal `:` are
/// both plain `Atom`s -- see [`split_on_top_level_commas`] and
/// [`find_map_separator_colon`], which recognise them by their exact text
/// rather than needing their own `OpKind`.
enum Piece {
    Atom(String),
    Op(OpKind, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpKind {
    NullCoalesce,
    UndefinedCoalesce,
    IsIn,
    Question,
    Colon,
}

/// Resolves every `(...)`/`[...]`/`{...}` group in `tokens` (recursively,
/// innermost first) into a single opaque [`Piece::Atom`], leaving only the
/// tokens that are directly at this nesting level as [`Piece::Op`]s for
/// [`resolve_pieces`] to act on.
fn expand(tokens: &[Tok]) -> Result<Vec<Piece>, RewriteError> {
    let mut pieces = Vec::new();
    let mut i = 0usize;

    while i < tokens.len() {
        match &tokens[i] {
            Tok::LParen(open) | Tok::LBracket(open) | Tok::LBrace(open) => {
                match find_matching_close(tokens, i) {
                    Some(close_idx) => {
                        let inner = resolve_flat(&tokens[i + 1..close_idx])?;
                        let close_text = tokens[close_idx].text();
                        pieces.push(Piece::Atom(format!("{open}{inner}{close_text}")));
                        i = close_idx + 1;
                    }
                    // Unmatched opener: leave the rest of this level untouched
                    // rather than guess where it was meant to close.
                    None => {
                        for t in &tokens[i..] {
                            pieces.push(Piece::Atom(t.text().to_string()));
                        }
                        i = tokens.len();
                    }
                }
            }
            // An unmatched closer (no opener) is passed through as-is.
            Tok::RParen(s) | Tok::RBracket(s) | Tok::RBrace(s) => {
                pieces.push(Piece::Atom(s.clone()));
                i += 1;
            }
            Tok::NullCoalesce(s) => {
                pieces.push(Piece::Op(OpKind::NullCoalesce, s.clone()));
                i += 1;
            }
            Tok::UndefinedCoalesce(s) => {
                pieces.push(Piece::Op(OpKind::UndefinedCoalesce, s.clone()));
                i += 1;
            }
            Tok::IsIn(s) => {
                pieces.push(Piece::Op(OpKind::IsIn, s.clone()));
                i += 1;
            }
            Tok::Question(s) => {
                pieces.push(Piece::Op(OpKind::Question, s.clone()));
                i += 1;
            }
            Tok::Colon(s) => {
                pieces.push(Piece::Op(OpKind::Colon, s.clone()));
                i += 1;
            }
            Tok::Str(s) | Tok::Text(s) => {
                pieces.push(Piece::Atom(s.clone()));
                i += 1;
            }
        }
    }

    Ok(pieces)
}

/// Finds the token index that closes the opener at `open_idx`, counting
/// `(`/`[`/`{` as +1 depth and `)`/`]`/`}` as -1 (bracket *kind* is not
/// cross-checked -- malformed mismatched brackets are not this rewrite's
/// concern; the parser it feeds will reject them).
fn find_matching_close(tokens: &[Tok], open_idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (j, t) in tokens.iter().enumerate().skip(open_idx) {
        match t {
            Tok::LParen(_) | Tok::LBracket(_) | Tok::LBrace(_) => depth += 1,
            Tok::RParen(_) | Tok::RBracket(_) | Tok::RBrace(_) => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}

/// Tokenizes and fully resolves one slice of expression source.
fn resolve_flat(tokens: &[Tok]) -> Result<String, RewriteError> {
    let pieces = expand(tokens)?;
    Ok(resolve_pieces(&pieces))
}

/// Rewrites a flat (no unresolved parens/brackets/braces) sequence of pieces
/// into minijinja source.
///
/// In priority order:
///
/// 1. A top-level `,` splits into independent segments (each resolved
///    recursively, rejoined with `,`) -- a call's arguments, a list's
///    elements, a map's pairs. Checked first because a comma is a syntactic
///    boundary no operator below may cross: `f(x, a ?? b)` must rewrite the
///    *second argument*, not swallow `x` into the coalesce's left operand.
/// 2. Within a comma-free segment, a `:` that appears before any `?` (or
///    with no `?` at all) is a map literal's key/value separator, not a
///    ternary's -- a ternary's own `:` always follows its own `?`. Splits
///    into key (left, resolved for consistency though normally inert) and
///    value (right, resolved -- this is where `{'k': a ?? b}` gets its `??`
///    rewritten without the map syntax around it being swept in).
/// 3. Ternary: the first (leftmost) top-level `?` and its depth-matched `:`
///    -- see [`find_ternary`]'s doc for how that handles right-associative
///    nesting. The then-branch (and defensively the condition) are wrapped
///    in parens on the way out; see the comment at the `format!` below for
///    why that parenthesization is not cosmetic.
/// 4. `??`/`???`/`isIn`, split at the *rightmost* occurrence (Kestra
///    registers all three at the same precedence with `Associativity.LEFT`,
///    so `a ?? b ?? c` must become
///    `kestra_null_coalesce(kestra_null_coalesce(a, b), c)`, which splitting
///    at the last occurrence and recursing left produces).
/// 5. Nothing left to resolve: reconstruct verbatim, using each remaining
///    `Op` piece's own original text (an operator with no matching partner,
///    e.g. a stray `:` with no preceding `?`, is left untouched rather than
///    guessed at).
fn resolve_pieces(pieces: &[Piece]) -> String {
    if let Some(segments) = split_on_top_level_commas(pieces) {
        return segments
            .into_iter()
            .map(resolve_pieces)
            .collect::<Vec<_>>()
            .join(",");
    }

    if let Some(colon_idx) = find_map_separator_colon(pieces) {
        let key = resolve_pieces(&pieces[..colon_idx]);
        let value = resolve_pieces(&pieces[colon_idx + 1..]);
        return format!("{key}:{value}");
    }

    if let Some((q_idx, colon_idx)) = find_ternary(pieces) {
        let cond = resolve_pieces(&pieces[..q_idx]);
        let then_branch = resolve_pieces(&pieces[q_idx + 1..colon_idx]);
        let else_branch = resolve_pieces(&pieces[colon_idx + 1..]);
        // Parens on the then-branch, and defensively on the condition too.
        // Pebble's `parseTernaryExpression` parses the then-branch at full
        // expression precedence, so in real Pebble `a ? b ? c : d : e` means
        // `a ? (b ? c : d) : e`. `find_ternary`'s depth counter gets that
        // grouping right -- but minijinja's inline-if has no grouping
        // construct of its own for the then-branch (only its else-chain is
        // naturally right-associative, which already matches Pebble for
        // free): an unparenthesized nested conditional emitted there
        // re-parses with the wrong `if`/`else` binding to the wrong
        // condition. This was the one bug in this module that produced
        // *valid* minijinja with *silently different* semantics rather than
        // failing loudly -- verified with `a=false, b=true`: Pebble gives
        // `e`, the unparenthesized form gave `c`.
        return format!("({then_branch}) if ({cond}) else {else_branch}");
    }

    if let Some(idx) = find_rightmost_binary(pieces) {
        let kind = match &pieces[idx] {
            Piece::Op(kind, _) => *kind,
            Piece::Atom(_) => unreachable!("find_rightmost_binary only returns Op indices"),
        };
        let left = resolve_pieces(&pieces[..idx]);
        let right = resolve_pieces(&pieces[idx + 1..]);
        return match kind {
            OpKind::NullCoalesce => format!("kestra_null_coalesce({left}, {right})"),
            OpKind::UndefinedCoalesce => format!("kestra_undefined_coalesce({left}, {right})"),
            // Provisional -- see this module's doc comment.
            OpKind::IsIn => format!("({left} in {right})"),
            OpKind::Question | OpKind::Colon => {
                unreachable!("find_rightmost_binary never returns a ternary token")
            }
        };
    }

    pieces
        .iter()
        .map(|p| match p {
            Piece::Atom(s) => s.as_str(),
            Piece::Op(_, s) => s.as_str(),
        })
        .collect()
}

/// Splits `pieces` on every top-level `,` (there are none nested inside a
/// group by construction -- [`expand`] has already collapsed every
/// `(...)`/`[...]`/`{...}` into one opaque atom, so any comma piece seen
/// here is genuinely at this exact nesting level). Returns `None` if there
/// is no top-level comma at all, so a comma-free slice does not recurse
/// through this branch pointlessly.
fn split_on_top_level_commas(pieces: &[Piece]) -> Option<Vec<&[Piece]>> {
    let comma_positions: Vec<usize> = pieces
        .iter()
        .enumerate()
        .filter_map(|(i, p)| match p {
            Piece::Atom(s) if s == "," => Some(i),
            _ => None,
        })
        .collect();
    if comma_positions.is_empty() {
        return None;
    }
    let mut segments = Vec::with_capacity(comma_positions.len() + 1);
    let mut start = 0usize;
    for &pos in &comma_positions {
        segments.push(&pieces[start..pos]);
        start = pos + 1;
    }
    segments.push(&pieces[start..]);
    Some(segments)
}

/// Finds a map literal's key/value `:` -- distinguished from a ternary's `:`
/// by position alone: a ternary's `:` always follows its own `?`, so a `:`
/// that appears with no `?` before it in this (already comma-isolated)
/// slice cannot be one.
fn find_map_separator_colon(pieces: &[Piece]) -> Option<usize> {
    let first_colon = pieces
        .iter()
        .position(|p| matches!(p, Piece::Op(OpKind::Colon, _)));
    let first_question = pieces
        .iter()
        .position(|p| matches!(p, Piece::Op(OpKind::Question, _)));
    match (first_colon, first_question) {
        (Some(c), None) => Some(c),
        (Some(c), Some(q)) if c < q => Some(c),
        _ => None,
    }
}

/// Finds the first (leftmost) `?` and its depth-matched `:` among `pieces`.
/// "Depth-matched" handles right-nested, unparenthesized ternaries
/// (`a ? b : c ? d : e`): each further `?` before the match increases the
/// depth the matching `:` must first unwind.
fn find_ternary(pieces: &[Piece]) -> Option<(usize, usize)> {
    let mut first_question = None;
    let mut depth = 0i32;
    for (i, p) in pieces.iter().enumerate() {
        match p {
            Piece::Op(OpKind::Question, _) => {
                if first_question.is_none() {
                    first_question = Some(i);
                }
                depth += 1;
            }
            Piece::Op(OpKind::Colon, _) if first_question.is_some() => {
                depth -= 1;
                if depth == 0 {
                    return Some((first_question.expect("checked is_some above"), i));
                }
            }
            _ => {}
        }
    }
    None
}

/// Finds the last (rightmost) `??`/`???`/`isIn` among `pieces` -- see
/// [`resolve_pieces`] for why rightmost is the correct split point for
/// left-associative operators.
fn find_rightmost_binary(pieces: &[Piece]) -> Option<usize> {
    pieces.iter().enumerate().rev().find_map(|(i, p)| match p {
        Piece::Op(OpKind::NullCoalesce | OpKind::UndefinedCoalesce | OpKind::IsIn, _) => Some(i),
        _ => None,
    })
}

/// Tokenizes and rewrites the inside of one `{{ }}` region as a single
/// expression.
fn rewrite_expression(inner: &str) -> Result<String, RewriteError> {
    let tokens = tokenize(inner)?;
    resolve_flat(&tokens)
}

/// Tokenizes and rewrites the inside of one `{% %}` region as a tag body --
/// not a single expression. See this module's doc ("`{{ }}` expressions vs.
/// `{% %}` tags") for the shape.
fn rewrite_tag_body(body: &str) -> Result<String, RewriteError> {
    let tokens = tokenize(body)?;

    let Some(keyword_idx) = tokens.iter().position(|t| !is_whitespace_tok(t)) else {
        // Nothing but whitespace (or fully empty), e.g. `{%  %}`. No
        // keyword, nothing to rewrite.
        return Ok(tokens.iter().map(Tok::text).collect());
    };

    let keyword = tokens[keyword_idx].text().to_string();
    let prefix: String = tokens[..=keyword_idx].iter().map(Tok::text).collect();
    let rest = &tokens[keyword_idx + 1..];

    // Only `for` and `set` have a further structural boundary splitting
    // their body into a name-list side (never rewritten) and an
    // expression side (rewritten). Every other tag -- `if`, `elseif`,
    // `unless`, `endif`, `else`, ... -- has its whole remainder rewritten as
    // one expression, same as a `{{ }}` region.
    let boundary = match keyword.as_str() {
        "for" => find_top_level_word(rest, "in"),
        "set" => find_top_level_word(rest, "="),
        _ => None,
    };

    let rewritten = match boundary {
        Some(b_idx) => {
            let target: String = rest[..b_idx].iter().map(Tok::text).collect();
            let boundary_text = rest[b_idx].text();
            let value = resolve_flat(&rest[b_idx + 1..])?;
            format!("{} {boundary_text} {}", target.trim(), value.trim())
        }
        None => resolve_flat(rest)?,
    };

    Ok(format!("{prefix} {}", rewritten.trim()))
}

fn is_whitespace_tok(t: &Tok) -> bool {
    let s = t.text();
    !s.is_empty() && s.chars().all(char::is_whitespace)
}

/// Finds the first occurrence of `Tok::Text(word)` at the top nesting level
/// of `tokens` (not inside a `(`/`[`/`{` group) -- used by
/// [`rewrite_tag_body`] to find `for`'s `in` and `set`'s `=` boundary. A
/// bare `in` is a plain word token (never confused with this rewrite's
/// `isIn` output, which `tokenize` only ever produces from the source
/// substring `isIn`, a different word); a bare `=` is only ever a single
/// token here because `tokenize` groups `==` (and `!=`/`<=`/`>=`) as their
/// own token first, so it can never be mistaken for half of `==`.
fn find_top_level_word(tokens: &[Tok], word: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, t) in tokens.iter().enumerate() {
        match t {
            Tok::LParen(_) | Tok::LBracket(_) | Tok::LBrace(_) => depth += 1,
            Tok::RParen(_) | Tok::RBracket(_) | Tok::RBrace(_) => depth -= 1,
            Tok::Text(s) if depth == 0 && s == word => return Some(i),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{rewrite, RewriteError};

    /// The rewriter must not touch an operator inside a string literal. A
    /// regex would; that is why this is a tokenizer.
    #[test]
    fn an_operator_inside_a_string_literal_is_left_alone() {
        assert_eq!(rewrite("{{ 'a ?? b' }}").unwrap(), "{{ 'a ?? b' }}");
        assert_eq!(
            rewrite(r#"{{ "x ? y : z" }}"#).unwrap(),
            r#"{{ "x ? y : z" }}"#
        );
    }

    /// `???` must not be mistaken for `??` followed by `?`.
    #[test]
    fn the_longer_operator_wins() {
        let out = rewrite("{{ a ??? b }}").unwrap();
        assert!(
            out.contains("undefined"),
            "expected the undefined-coalescing form, got {out}"
        );
        assert!(!out.contains("null_coalesce"), "{out}");
    }

    /// Replaces a weaker, counting-based assertion (`.matches(" if
    /// ").count() == 2`) that a swapped condition/then-branch, or the
    /// missing-then-branch-parens bug this module used to have, would also
    /// have passed. Pinned to the exact output so both mistakes are caught.
    #[test]
    fn nested_ternaries_rewrite_innermost_first() {
        let out = rewrite("{{ a ? (b ? 'x' : 'y') : 'z' }}").unwrap();
        assert_eq!(
            out,
            "{{( (( 'x' ) if (b ) else  'y') ) if ( a ) else  'z' }}"
        );
    }

    /// The UNPARENTHESIZED form of the same nesting -- `nested_ternaries_
    /// rewrite_innermost_first` above uses `a ? (b ? 'x' : 'y') : 'z'`,
    /// whose source parens already make the grouping explicit and so would
    /// still pass even without the I3 then-branch-parens fix. This is the
    /// exact input the I3 finding was about
    /// (`a ? b ? c : d : e`, `a=false, b=true`: Pebble's own
    /// `parseTernaryExpression` grouping gives `a ? (b ? c : d) : e`, so
    /// with `a` false the result is `e`; the bug this module used to have,
    /// before wrapping the then-branch in parens, silently re-parsed this in
    /// minijinja as `c if b else (d if a else e)`, giving `c` instead --
    /// confirmed by deleting the parens at the `format!` in `resolve_pieces`
    /// and re-running this exact input: the output changed from the string
    /// below to `"{{ c  if  b  else  d  if  a  else  e }}"`, which
    /// evaluates to the wrong branch.
    #[test]
    fn unparenthesized_nested_ternary_keeps_pebbles_grouping() {
        let out = rewrite("{{ a ? b ? c : d : e }}").unwrap();
        assert_eq!(out, "{{(( c ) if ( b ) else  d ) if ( a ) else  e }}");
    }

    #[test]
    fn text_outside_an_expression_is_untouched() {
        assert_eq!(rewrite("literal ?? text").unwrap(), "literal ?? text");
    }

    /// `the_longer_operator_wins` above only pins the negative half (`???`
    /// must not look like `??`); this pins that a bare `??` DOES produce the
    /// null-coalescing form -- without it, mapping `??` to nothing at all
    /// would still pass every other test in this module.
    #[test]
    fn double_question_mark_becomes_the_null_coalescing_call() {
        let out = rewrite("{{ a ?? b }}").unwrap();
        assert!(
            out.contains("kestra_null_coalesce("),
            "expected the null-coalescing call form, got {out}"
        );
        assert!(!out.contains("undefined"), "{out}");
    }

    /// `isIn` is the fourth operator this module's doc comment claims to
    /// handle. Provisional per the module doc: whether minijinja's `in`
    /// actually agrees with Kestra's `InExpression` on every operand pair is
    /// Task 3's oracle measurement -- this only pins the syntactic rewrite.
    #[test]
    fn is_in_becomes_the_in_operator() {
        let out = rewrite("{{ a isIn b }}").unwrap();
        assert!(out.contains(" in "), "{out}");
        assert!(!out.contains("isIn"), "{out}");

        // Not a regex: `isIn` inside a string literal survives untouched --
        // a global, quote-blind `str::replace("isIn", "in")` would corrupt
        // this, and nothing else in this module exercised that.
        assert_eq!(rewrite("{{ 'a isIn b' }}").unwrap(), "{{ 'a isIn b' }}");

        // Same principle outside any `{{ }}`/`{% %}` region at all.
        assert_eq!(rewrite("literal isIn text").unwrap(), "literal isIn text");
    }

    /// The rewriter's one error case: a string literal that never finds its
    /// closing quote. Both halves matter -- the error comes back (not a
    /// best-effort `Ok`), and the `??` that sits inside the broken region is
    /// never rewritten on the way there. `find_region_close` (see its doc
    /// comment) is what actually raises this for input reachable through the
    /// public `rewrite` function: the quote-skip that looks for `}}` runs
    /// off the end of input hunting a closing quote that does not exist, so
    /// the error fires before `tokenize` -- and any operator rewrite -- ever
    /// runs on this expression at all.
    #[test]
    fn an_unterminated_string_literal_is_reported_not_silently_rewritten() {
        let err = rewrite("{{ a ?? 'oops }}").unwrap_err();
        assert!(
            matches!(err, RewriteError::UnterminatedString { quote: '\'', .. }),
            "{err:?}"
        );
    }

    /// The unterminated-string scan is bounded to the enclosing tag: whether
    /// this reports an error must not depend on some unrelated apostrophe
    /// (e.g. in a contraction) appearing later in the document, and an
    /// operator inside a genuinely broken tag must never be silently passed
    /// through unrewritten just because such an apostrophe happens to exist.
    #[test]
    fn an_unterminated_string_is_bounded_to_its_own_tag() {
        // No closing quote anywhere in the document at all.
        assert!(rewrite("{{ 'a }} its fine").is_err());

        // A LATER apostrophe exists (in "it's"), but it must not "rescue" a
        // literal that never closed within this tag.
        assert!(rewrite("{{ 'a }} it's fine").is_err());

        // The exact failure mode under review: an operator sitting before
        // the broken literal must not be silently left unrewritten with no
        // error, just because a later quote exists somewhere in the tail.
        let err = rewrite("{{ a ?? 'x }} tail with ' quote and more").unwrap_err();
        assert!(
            matches!(err, RewriteError::UnterminatedString { .. }),
            "{err:?}"
        );
    }

    /// A comma is an operand boundary an operator must not cross: `f(x, a ??
    /// b)` is a two-argument call, not one call whose (wrong-arity) sole
    /// argument is `kestra_null_coalesce(x, a, b)`.
    #[test]
    fn commas_bound_operator_rewrites_inside_a_call() {
        let out = rewrite("{{ f(x, a ?? b) }}").unwrap();
        assert_eq!(out, "{{ f(x,kestra_null_coalesce( a ,  b)) }}");
    }

    /// Same principle for a list literal's elements.
    #[test]
    fn commas_bound_operator_rewrites_inside_a_list() {
        let out = rewrite("{{ [1, a ?? b] }}").unwrap();
        assert_eq!(out, "{{ [1,kestra_null_coalesce( a ,  b)] }}");
    }

    /// A ternary as a later argument must not be glued to an earlier one by
    /// the comma-boundary fix -- each argument still resolves fully on its
    /// own.
    #[test]
    fn a_ternary_argument_after_a_comma_still_resolves() {
        let out = rewrite("{{ f(x, c ? a : b) }}").unwrap();
        assert_eq!(out, "{{ f(x,( a ) if ( c ) else  b) }}");
    }

    /// A map literal's `:` must not be mistaken for a ternary's `:`, and a
    /// map literal must nest like `(`/`[` so an outer ternary's own `?`/`:`
    /// is not confused by an inner map's colon.
    #[test]
    fn map_literals_do_not_confuse_the_ternary() {
        let out = rewrite("{{ flag ? {'a': 1} : {'b': 2} }}").unwrap();
        assert_eq!(out, "{{( {'a': 1} ) if ( flag ) else  {'b': 2} }}");
    }

    /// An operator inside a map literal's *value* must still be rewritten,
    /// without the map's own key/colon syntax being swept into its operand.
    #[test]
    fn an_operator_inside_a_map_value_still_resolves() {
        let out = rewrite("{{ {'k': a ?? b} }}").unwrap();
        assert_eq!(out, "{{ {'k':kestra_null_coalesce( a ,  b)} }}");
    }

    /// `{% if %}` tag bodies must be rewritten as an expression, not left
    /// alone (a documented capability that must not silently do nothing)
    /// and not treated as if the leading `if` keyword were part of the
    /// expression (which produced a parse error before this fix).
    #[test]
    fn if_tag_bodies_rewrite_their_condition() {
        let out = rewrite("{% if a ?? b %}y{% endif %}").unwrap();
        assert_eq!(out, "{% if kestra_null_coalesce( a ,  b )%}y{% endif %}");
    }

    /// `{% set %}`: the target left of `=` is a name, never rewritten; the
    /// value right of `=` is rewritten normally.
    #[test]
    fn set_tag_bodies_rewrite_only_the_value_side() {
        let out = rewrite("{% set x = a ?? b %}").unwrap();
        assert_eq!(out, "{% set x = kestra_null_coalesce( a ,  b )%}");
    }

    /// `{% for %}`: the loop variable left of `in` is a name, never
    /// rewritten; the iterable right of `in` is rewritten normally. The
    /// tag's own `in` keyword must survive as plain syntax, not be mistaken
    /// for this rewrite's `isIn` output.
    #[test]
    fn for_tag_bodies_rewrite_only_the_iterable_side() {
        let out = rewrite("{% for x in list ?? [] %}y{% endfor %}").unwrap();
        assert_eq!(
            out,
            "{% for x in kestra_null_coalesce( list ,  [] )%}y{% endfor %}"
        );
    }

    /// A ternary inside a `{% if %}` tag: proves the tag-body path shares
    /// the same expression rewriter (including the I3 then-branch-parens
    /// fix) as a `{{ }}` region, not a separate, weaker implementation.
    #[test]
    fn a_ternary_inside_an_if_tag_resolves() {
        let out = rewrite("{% if flag ? 'a' : 'b' %}y{% endif %}").unwrap();
        assert_eq!(out, "{% if ( 'a' ) if ( flag ) else  'b'%}y{% endif %}");
    }
}
