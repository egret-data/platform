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
//! Three things defeat a string-substitution rewrite, and all three are live
//! corpus entries (`oracle/corpus.json`):
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
//!
//! So this module tokenizes: string literals (both quote styles, with
//! backslash escapes), `(`/`)`/`[`/`]` for nesting, and the four operators
//! matched longest-first (`???` before `??` before `?`; `isIn` matched as a
//! whole identifier, not a substring, so it never fires inside e.g.
//! `thisIsInvalid`). Everything else — identifiers, numbers, whitespace,
//! commas, dots, `|`, `=`, the untouched comparison operators — is copied
//! through byte-for-byte.
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
//! measurement to make, not this rewrite's to guess at.

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
                let rewritten = rewrite_expression(inner)?;
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
/// string-literal contents so a quoted `}}` can never terminate the region
/// early. Returns the byte offset where `close` begins, or `Ok(None)` if
/// `close` never appears before the end of input.
///
/// This is also where an unterminated string literal is actually caught for
/// anything reachable through the public `rewrite` entry point: skipping a
/// string's contents to look past it for `close` uses the same
/// [`scan_string_literal`] `tokenize` uses, so by the time a region's
/// content is handed to `tokenize`, every quote inside it is already known
/// to be balanced. `tokenize`'s own check stays as defense in depth (and
/// documents the invariant), but it is this function's `Err` that a caller
/// of `rewrite` actually observes for `{{ a ?? 'unterminated }}` — the
/// quote-skip here runs off the end of the entire input looking for a match,
/// discovers there is none, and reports the error before ever reaching
/// `close_start`, `}}` or not.
fn find_region_close(src: &str, start: usize, close: &str) -> Result<Option<usize>, RewriteError> {
    let len = src.len();
    let mut i = start;
    while i < len {
        if src[i..].starts_with(close) {
            return Ok(Some(i));
        }
        let c = src[i..].chars().next().expect("i < len");
        if c == '\'' || c == '"' {
            let str_start = i;
            let (end, closed) = scan_string_literal(src, i, c);
            if !closed {
                return Err(RewriteError::UnterminatedString {
                    quote: c,
                    snippet: src[str_start..].to_string(),
                });
            }
            i = end;
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
    NullCoalesce(String),
    UndefinedCoalesce(String),
    Question(String),
    Colon(String),
    IsIn(String),
    /// Identifiers, numbers, whitespace, and any other punctuation this
    /// rewrite does not act on (`.`, `,`, `|`, `=`, `>`, `<`, etc).
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
/// fully-resolved, already-rewritten parenthesized/bracketed group) or one of
/// the five operator tokens this rewrite acts on.
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

/// Resolves every `(...)`/`[...]` group in `tokens` (recursively, innermost
/// first) into a single opaque [`Piece::Atom`], leaving only the tokens that
/// are directly at this nesting level as [`Piece::Op`]s for
/// [`resolve_pieces`] to act on.
fn expand(tokens: &[Tok]) -> Result<Vec<Piece>, RewriteError> {
    let mut pieces = Vec::new();
    let mut i = 0usize;

    while i < tokens.len() {
        match &tokens[i] {
            Tok::LParen(open) | Tok::LBracket(open) => match find_matching_close(tokens, i) {
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
            },
            // An unmatched closer (no opener) is passed through as-is.
            Tok::RParen(s) | Tok::RBracket(s) => {
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
/// `(`/`[` as +1 depth and `)`/`]` as -1 (bracket *kind* is not cross-checked
/// -- malformed mismatched brackets are not this rewrite's concern; the
/// parser it feeds will reject them).
fn find_matching_close(tokens: &[Tok], open_idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (j, t) in tokens.iter().enumerate().skip(open_idx) {
        match t {
            Tok::LParen(_) | Tok::LBracket(_) => depth += 1,
            Tok::RParen(_) | Tok::RBracket(_) => {
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

/// Rewrites a flat (no unresolved parens/brackets) sequence of pieces into
/// minijinja source.
///
/// Ternary is resolved before the binary operators: it is checked first, and
/// its condition/then/else slices are each resolved recursively, so
/// `a ?? b ? c : d` (coalesce binding tighter than ternary, the usual
/// C-like precedence) correctly becomes `c if kestra_null_coalesce(a, b) else
/// d`, not the other way around. Nested ternaries resolve innermost first
/// because a parenthesized inner ternary is already a finished [`Piece::Atom`]
/// string by the time this function ever sees it (via [`expand`]), and an
/// unparenthesized right-nested one (`a ? b : c ? d : e`) falls out of taking
/// the *first* `?` and its depth-matched `:`, leaving the remainder (which
/// still contains the inner ternary) to this same function, recursively, as
/// the `else` branch.
fn resolve_pieces(pieces: &[Piece]) -> String {
    if let Some((q_idx, colon_idx)) = find_ternary(pieces) {
        let cond = resolve_pieces(&pieces[..q_idx]);
        let then_branch = resolve_pieces(&pieces[q_idx + 1..colon_idx]);
        let else_branch = resolve_pieces(&pieces[colon_idx + 1..]);
        return format!("{then_branch} if {cond} else {else_branch}");
    }

    if let Some(idx) = find_rightmost_binary(pieces) {
        let kind = match &pieces[idx] {
            Piece::Op(kind, _) => *kind,
            Piece::Atom(_) => unreachable!("find_rightmost_binary only returns Op indices"),
        };
        // Rightmost split for left-associativity: Kestra registers `??`,
        // `???`, and `isIn` all at the same precedence (30) with
        // `Associativity.LEFT`, so `a ?? b ?? c` must become
        // `kestra_null_coalesce(kestra_null_coalesce(a, b), c)`, which
        // splitting at the *last* occurrence and recursing on the left
        // produces.
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

    // No operator left at this level: reconstruct verbatim. Any `Op` piece
    // still present here is an operator with no matching partner (e.g. a
    // stray `:` with no preceding `?`) -- left untouched, using its own
    // original source text, rather than guessed at.
    pieces
        .iter()
        .map(|p| match p {
            Piece::Atom(s) => s.as_str(),
            Piece::Op(_, s) => s.as_str(),
        })
        .collect()
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

/// Tokenizes and rewrites the inside of one `{{ }}` / `{% %}` region.
fn rewrite_expression(inner: &str) -> Result<String, RewriteError> {
    let tokens = tokenize(inner)?;
    resolve_flat(&tokens)
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

    #[test]
    fn nested_ternaries_rewrite_innermost_first() {
        let out = rewrite("{{ a ? (b ? 'x' : 'y') : 'z' }}").unwrap();
        // Both conditionals survive, and no `?` or `:` is left as an operator.
        assert_eq!(out.matches(" if ").count(), 2, "{out}");
        assert!(!out.contains(" ? "), "{out}");
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
}
