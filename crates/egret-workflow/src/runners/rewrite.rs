//! Rewrites Kestra's operator syntax into syntax minijinja can parse.
//!
//! minijinja has no `??`, `???`, `isIn`, or C-style ternary `? :`. It does
//! have an inline `a if c else b` conditional, and an `in` operator that was
//! once a candidate target for `isIn` and has since been measured and
//! rejected (see below). This module's `rewrite` turns Kestra source
//! into minijinja source; it never evaluates anything, because it has no
//! context to evaluate against — the values live in the caller's render call,
//! not here.
//!
//! # Design overview — read this first
//!
//! **Shape.** `rewrite` walks the input for `{{ }}`/`{% %}` regions
//! ([`split_whitespace_control`] strips any `{{- -}}`/`{%- -%}` markers
//! first). Each region is tokenized ([`tokenize`]), then resolved by two
//! mutually recursive functions: [`expand`] collapses every
//! `(...)`/`[...]`/`{...}` group into one opaque, already-resolved atom
//! (calling [`resolve_flat`], which calls `expand` again for what's inside —
//! this is the recursion that nesting depth grows through), and
//! [`resolve_pieces`] resolves what's left at each flat level by trying a
//! fixed sequence of syntactic boundaries, lowest-precedence first — see its
//! own doc comment for the exact sequence and the full operator-precedence
//! table read from Pebble's bytecode. Both `expand` and `resolve_pieces`
//! share one recursion-depth budget ([`MAX_NESTING_DEPTH`]) because both
//! recurse unboundedly on adversarial input (deep nesting through `expand`,
//! a long flat operator chain through `resolve_pieces`'s own
//! rightmost-split-and-recurse-left pattern) and neither is safe to leave
//! unbounded over `pub` `rewrite`'s user-supplied input.
//!
//! **The constraints that shaped it, in one place:**
//!
//! - *Tokenize, don't regex* — string literals, `???` overlapping `??`,
//!   nested ternaries, and `,`/`{'k':v}` boundaries all defeat a
//!   string-substitution rewrite; see "Why a tokenizer, not a regex" below.
//! - *`??` and `???` differ*, and Kestra registers both as lazy-right-side
//!   operators, not functions — see "The two coalescing operators" below
//!   for why that makes them plain function calls in the output, not
//!   filters.
//! - *A string literal's unterminated-ness is bounded by physical line, not
//!   by the region's own close delimiter* — bounding by `close` broke every
//!   literal that legitimately contains `close`-looking text (e.g. a `jq`
//!   query with `}}` in it); see [`scan_string_literal_line_bounded`].
//! - *`{{ }}` is one expression; `{% %}` is a tag body with its own
//!   structure* (`for`'s `in`, `set`'s `=`), and a ternary spliced into
//!   either needs the whole conditional wrapped in parens or it fails to
//!   parse — but only when a ternary is actually present, since some tag
//!   bodies (`{% filter %}`) are not expression positions at all; see
//!   "`{{ }}` expressions vs. `{% %}` tags" below and [`rewrite_tag_body`].
//! - *Precedence tiers below and at 30 need boundary-awareness even where
//!   this rewrite never touches the operator itself* (`and`/`or`, the
//!   comparison operators, `equals`), or a coalesce search swallows across
//!   them — and `is`/`is not` specifically can only ever extend LEFT
//!   (Pebble's `BinaryOperatorType.TEST`), never right, unlike every other
//!   tier; see [`resolve_pieces`] and [`find_rightmost_tier20`].
//! - *`{% raw %}`/`{% verbatim %}` are not a real, working capability to
//!   port* — measured against the pinned Pebble jar, not assumed; see the
//!   comment at the raw/verbatim handling that used to exist in [`rewrite`].
//! - *`isIn` is a function call*, `kestra_is_in`, not minijinja's `in` —
//!   Task 3 measured the two and they disagree three ways. The four
//!   comparison operators are still left untouched entirely. See
//!   "`isIn` — settled" and "Comparison operators are left alone" below.
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
//! # Pebble's `null` literal, and why it is this module's problem
//!
//! `null` is a **literal** in Pebble and only a name in minijinja, so
//! `{{ null }}` rendered empty there and raised `undefined value` here, and
//! every `{{ null | <filter> }}` went the same way. The knock-on was the
//! expensive half: because the literal and a **typo'd variable** both arrived
//! as `ValueKind::Undefined`, `runners::filters::is_java_null` had to fold
//! `Undefined` into "Kestra's null" — which W2 Task 8 then found turns
//! `{{ nothere | upper }}` from a hard failure into `""`, the one divergence
//! this crate's rules do not allow. Task 8 made the filters strict again and
//! the sixteen renderable `{{ null | F }}` cases broke instead.
//!
//! Both halves close here, and only here: [`tokenize`] translates a bare
//! `null`/`NULL`/`none`/`NONE` to minijinja's `none`, so the literal becomes a
//! `None` and the typo stays an `Undefined`, and
//! `runners::filters::is_java_null` narrows to `is_none()`. The literal
//! renders and the typo still refuses, in the same run.
//!
//! Three things bound the translation, each with a corpus entry behind it:
//! the four spellings are exact (see [`is_null_literal_spelling`] — `NoNe` is
//! a variable), attribute position is excluded across whitespace (see
//! [`follows_an_attribute_dot`]), and a string literal is one token, so
//! `{{ 'null' }}` is never reached.
//!
//! **What this does NOT close, and it is recorded rather than fixed:**
//! minijinja's literal vocabulary is *wider* than Pebble's. `None`, `True`
//! and `False` — Python's spellings — are constants to minijinja's lexer and
//! ordinary (absent) variable names to Pebble, so egret answers where Kestra
//! raises. That is an over-acceptance, it predates this module's null work,
//! and no rewrite of a *name* can reach it: the two are keywords to the lexer
//! this module feeds. `null-literal-python-spelling-is-not-the-literal` and
//! `boolean-literal-python-spelling-is-not-the-literal` are the entries, both
//! baselined.
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
//! a substring, so it never fires inside e.g. `isInvalid` -- which DOES
//! contain the literal substring "isIn" (its first four characters), unlike
//! a same-shaped-looking example that merely reads as if it should).
//! Everything else — identifiers, numbers, whitespace, dots, `|`, the
//! untouched comparison operators — is copied through byte-for-byte.
//!
//! # `isIn` — settled: a function, not minijinja's `in`
//!
//! `x isIn y` is rewritten to `kestra_is_in((x), (y))`, the third custom
//! function alongside the two coalescing ones.
//!
//! This module originally emitted `(x in y)` and said so provisionally,
//! leaving the verdict to Task 3's oracle measurement. **Task 3 measured it
//! and `in` lost**, in three separate ways — `InExpression.evaluate`
//! stringifies both sides via `Object::toString()`, matches a `Map`'s values
//! as well as its keys, and returns `false` for any right-hand side that is
//! not a collection or a map:
//!
//! | corpus entry | expr | Kestra | minijinja `in` |
//! |---|---|---|---|
//! | `isin` | `{{ 'b' isIn ['a','b'] }}` | `true` | `true` (agrees) |
//! | `isin-number-in-strings` | `{{ 1 isIn ['1','2'] }}` | `true` | **`false`** |
//! | `isin-map-value` | `{{ 'v' isIn m }}` | `true` | **`false`** (keys only) |
//! | `isin-string-rhs` | `{{ 'b' isIn 'abc' }}` | `false` | **`true`** (substring) |
//!
//! The `isin` entry that existed when this mapping was chosen agreed **by
//! luck** — same types on both sides, list right-hand — and could not have
//! separated the two. The other three were added in Task 3 for exactly that
//! reason. Semantics live in `runners::env::kestra_is_in`.
//!
//! # Comparison operators are left alone -- by THIS pass
//!
//! **2026-10-08 (W9c Task 4 fix round 1, the user's option A):** this module
//! still writes every comparison through verbatim, and a SECOND pass,
//! [`super::cmp_rewrite`], then compiles each one in its output into a call
//! to `super::compare` (`a > b` -> `kestra_gt(a, b)`), so a `BigDecimal` or
//! `BigInteger` compares exactly and every other pair answers exactly as
//! minijinja's own operator did. It is a separate pass, over minijinja
//! syntax, on purpose: what minijinja makes of THIS module's output is what
//! egret's comparisons have always meant, so that output is the thing to
//! parse, with minijinja's grammar, to keep their grouping; and this
//! module's byte-exact tests still describe this module alone. `env`'s
//! `compile_source` runs the two in order.
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
    #[error(
        "expression nested too deeply (limit {limit}, reached {depth}) -- refusing to keep \
         recursing rather than risk a stack overflow"
    )]
    TooDeeplyNested { depth: usize, limit: usize },
    #[error(
        "expression too long ({bytes} bytes, limit {limit}) -- one `{{{{ }}}}` region this long \
         can nest the template parser deeper than the render stack holds"
    )]
    ExpressionTooLong { bytes: usize, limit: usize },
    /// A `set`, `for` or `with` assigns a name starting with `kestra_`, the
    /// prefix of the functions egret's rewriters call (`kestra_gt`,
    /// `kestra_is_in`, …), which the variable would shadow. Raised by the
    /// second pass, `super::cmp_rewrite` (W9c Task 4 cleanup, N2).
    #[error(
        "`{name}` cannot be assigned: names starting with `kestra_` are reserved for egret's \
         own functions (a `set`, `for` or `with` target)"
    )]
    ReservedName { name: String },
}

/// What one byte of an expression can cost the **template parser's** stack.
///
/// **This is a different parser from the one [`MAX_NESTING_DEPTH`] guards.**
/// That counter is this rewriter's own recursion over brackets and operator
/// chains; this bounds `minijinja`'s expression parser, which egret does not
/// own and which recurses over constructs the bracket counter never sees.
/// Measured by bisecting the depth that overflows the 128 MiB render thread:
///
/// | construct | source bytes per level | stack per level | stack per SOURCE byte |
/// |---|---:|---:|---:|
/// | `{{ ---…---0 }}` unary minus | 1 | 832 | **832** |
/// | `{{ not not …true }}` | 1 | 832 | **832** |
/// | `{{ v.a.a…a }}` attribute chain | 2 | 1 472 | 736 |
/// | `{{ ((((…)))) }}` parens | 1 | — | already refused by [`MAX_NESTING_DEPTH`] |
///
/// Every level of every one of these consumes at least one byte of the
/// region, so **the region's length is an upper bound on the parser's depth**
/// and 832 is the worst cost per byte. Unary minus reached `SIGABRT` at
/// 170 000 bytes — on the thread egret sizes — so sizing the thread was not
/// enough on its own.
const WORST_STACK_PER_EXPRESSION_BYTE: usize = 832;

/// How much of the render stack one region may claim: a quarter.
///
/// The region is not the only thing on that stack — this rewriter, the
/// evaluator, the filters and every value writer share it — and the cost per
/// byte was measured on one construct rather than proven over the grammar.
const EXPRESSION_STACK_MARGIN: usize = 4;

/// The longest a single `{{ }}` or `{% %}` region may be.
///
/// **Derived, not chosen**, the same way `jqsandbox::MAX_SOURCE_BYTES` is:
/// the render's stack, divided by what a byte can cost it, divided by the
/// margin. At 128 MiB that is 134 217 728 / 832 / 4 = **40 329 bytes** for one
/// region.
///
/// It is a length cap, which for a *template* would be the wrong instrument —
/// a 40 KB SQL body with three variables in it is an ordinary flow, and
/// refusing it would be a bug. This is a cap on **one region's insides**,
/// where the SQL body is not: the text outside `{{ }}` is copied through and
/// never counted. A real expression is tens of bytes; the largest in the
/// 726-entry corpus is far under a hundred.
pub const MAX_EXPRESSION_BYTES: usize =
    super::env::RENDER_STACK_BYTES / WORST_STACK_PER_EXPRESSION_BYTE / EXPRESSION_STACK_MARGIN;

/// The recursion-depth ceiling `expand`/`resolve_flat`/`resolve_pieces` share
/// (one shared counter across all three, since they call each other
/// mutually -- see their doc comments). `rewrite` is `pub`, re-exported at
/// the crate root, and its input is user-supplied workflow source; without a
/// bound, both a deeply parenthesized expression
/// (`{{ ((((...))))}}`, walked by `expand`'s recursion into `resolve_flat`)
/// and a very long flat operator chain
/// (`{{ a ?? b ?? c ?? ... ?? z }}`, walked by `resolve_pieces`'s own
/// rightmost-split-and-recurse-left pattern) can recurse arbitrarily deep.
/// In Rust that is not a catchable error -- a real stack overflow aborts the
/// whole process (`fatal runtime error: stack overflow`), and once Task 3
/// wires rendering into a request path, that is a remote process abort from
/// ordinary user input, not a contained one.
///
/// Measured (not assumed) before picking a number: nested parens crash
/// somewhere between 2,000 and 5,000 levels on the default 8 MiB main
/// thread stack, and between 1,000 and 5,000 on the 2 MiB stack tokio gives
/// its worker threads -- the smaller, more realistic bound for a server
/// process. `500` sits comfortably below even that narrower range (2x
/// headroom to its lower edge) while still being far more nesting than any
/// real Kestra expression plausibly needs -- a handful of levels is typical,
/// a few dozen would already be unusual.
const MAX_NESTING_DEPTH: usize = 500;

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
        // **One scan for the earlier of the two openers, not one scan each.**
        // `expr[i..].find("{%")` on a template that contains no `{%` at all
        // reads to the END every time round the loop, so a template that is
        // nothing but `{{ }}` regions cost O(regions x length): 5 000 regions
        // took 0.23 s, 10 000 took 0.87 s, 20 000 took 3.3 s and 40 000 took
        // 13.5 s -- four times the work for twice the input. 200 000 regions,
        // a 1.4 MB template, took **5.5 minutes**. Scanning for `{` once and
        // deciding from the next byte is linear, and picks the same
        // delimiter: the two openers cannot start at the same index, so
        // "earliest, `{{` winning a tie" and "first match while scanning"
        // agree.
        let found = expr[i..].match_indices('{').find_map(|(rel, _)| {
            let rest = &expr[i + rel..];
            if rest.starts_with("{{") {
                Some((rel, "}}"))
            } else if rest.starts_with("{%") {
                Some((rel, "%}"))
            } else {
                None
            }
        });

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
                let raw_inner = &expr[content_start..close_start];
                let (leading_ws_ctrl, inner, trailing_ws_ctrl) =
                    split_whitespace_control(raw_inner);

                // No special case for `{% raw %}`/`{% verbatim %}`:
                // neither is a real, working capability to port. `"raw"` is
                // not a registered Pebble tag at all (fails to PARSE:
                // `Unexpected tag name "raw"`); Kestra registers zero custom
                // tags of its own (`Extension.getTokenParsers()` returns
                // `null`); and Pebble's own differently-named `verbatim` tag
                // (confirmed via `javap` on the pinned jar) does not
                // actually suppress evaluation of its body in this Pebble
                // version either (measured through the oracle harness:
                // `{% verbatim %}{{ a ?? b }}{% endverbatim %}` evaluates
                // the coalesce normally, rather than treating it as
                // literal text). Special-casing either spelling would make
                // this rewriter MORE correct than the engine it ports.
                // **Before the rewriter or the parser walks it.** The
                // bound is on the region's LENGTH because every level of
                // every recursive construct in the template grammar eats at
                // least one byte of it -- see `MAX_EXPRESSION_BYTES`.
                let significant = significant_bytes(inner);
                if significant > MAX_EXPRESSION_BYTES {
                    return Err(RewriteError::ExpressionTooLong {
                        bytes: significant,
                        limit: MAX_EXPRESSION_BYTES,
                    });
                }
                let rewritten = if open == "{{" {
                    rewrite_expression(inner)?
                } else {
                    rewrite_tag_body(inner)?
                };
                out.push_str(open);
                out.push_str(leading_ws_ctrl);
                out.push_str(&rewritten);
                out.push_str(trailing_ws_ctrl);
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

/// W9c Task 5: when `template` is exactly ONE `{{ … }}` region and nothing
/// else -- no text before or after it, not even a space -- its content,
/// without a `-` whitespace-control marker at either end; `None` otherwise.
/// The region's end is found as [`rewrite`] finds it (string literals and
/// brace nesting skipped), so `{{ {'a': {'b': 1}} }}` is one region and
/// `{{ 1 }}{{ 2 }}` is two. An unterminated string answers `None`: the plain
/// render then refuses the template in its own words.
pub(crate) fn sole_expression(template: &str) -> Option<&str> {
    let rest = template.strip_prefix("{{")?;
    let close = find_region_close(template, 2, "}}").ok()??;
    if close + 2 != template.len() {
        return None;
    }
    let (_, inner, _) = split_whitespace_control(&rest[..close - 2]);
    Some(inner)
}

/// W9c Task 5 fix round 1 (review M3): whether `inner` (a
/// [`sole_expression`]) means the same once it is wrapped as `(inner)`: its
/// brackets, read by the rewriter's own tokenizer (string literals skipped),
/// pair up by kind and never close one that `inner` did not open, and no
/// comma stands at the top level. `1) ~ (2` does not (wrapped, its `)` would
/// close the wrapper's `(`, and `(1) ~ (2)` renders where the source is a
/// syntax error); nor does `1, 2` (wrapped, a tuple). An unterminated
/// string answers `false`.
pub(crate) fn parenthesizes_safely(inner: &str) -> bool {
    let Ok(tokens) = tokenize(inner) else {
        return false;
    };
    let mut open: Vec<char> = Vec::new();
    for t in &tokens {
        match t {
            Tok::LParen(_) => open.push(')'),
            Tok::LBracket(_) => open.push(']'),
            Tok::LBrace(_) => open.push('}'),
            Tok::RParen(s) | Tok::RBracket(s) | Tok::RBrace(s) => {
                if open.pop() != s.chars().next() {
                    return false;
                }
            }
            Tok::Text(s) if open.is_empty() && s.contains(',') => return false,
            _ => {}
        }
    }
    open.is_empty()
}

/// Splits a `-` whitespace-control marker off the front and/or back of a
/// `{{ }}` / `{% %}` region's raw content (`Kestra`/Pebble/Jinja's
/// `{{- ... -}}` / `{%- ... -%}` convention for trimming adjacent template
/// whitespace). Returns `(leading, inner, trailing)` where `leading`/
/// `trailing` are each `"-"` or `""`, and `inner` is what should actually be
/// tokenized and rewritten -- the marker itself is never an operator and
/// must never be handed to `tokenize` as content.
///
/// Measured without this: `{%- if a ?? b -%}` rewrote the leading `-` as if
/// it were part of the tag body, reproducing Critical 1's exact shape
/// (`{%- kestra_null_coalesce( if a ,  b -)%}`, `if` swallowed into the
/// rewrite and the trailing `-%}` marker lost entirely) because
/// `rewrite_tag_body`'s "first non-whitespace token is the keyword" logic
/// took `-` itself as the keyword. Stripping the markers here, before either
/// `rewrite_expression` or `rewrite_tag_body` ever sees the content, avoids
/// that class of bug entirely rather than teaching both of those functions
/// about a syntax marker that is not really part of either grammar.
fn split_whitespace_control(content: &str) -> (&'static str, &str, &'static str) {
    let mut inner = content;
    let leading = if let Some(rest) = inner.strip_prefix('-') {
        inner = rest;
        "-"
    } else {
        ""
    };
    let trailing = if let Some(rest) = inner.strip_suffix('-') {
        inner = rest;
        "-"
    } else {
        ""
    };
    (leading, inner, trailing)
}

/// Scans forward from `start` for `close` (`"}}"` or `"%}"`), skipping over
/// string-literal contents and brace-literal nesting, so a map literal's own
/// closing `}` immediately followed by the region's `}}` cannot terminate
/// the region early. Returns the byte offset where `close` begins, or
/// `Ok(None)` if `close` never appears before the end of input.
///
/// **A string literal's content is never inspected for `close` at all** --
/// only for its own closing quote, per [`scan_string_literal_line_bounded`]
/// (see that function's doc for why bounding by physical line, not by
/// `close`, is what makes `{{ 'a}}b' }}` and a `jq` query containing `}}`
/// both work). This is also where an unterminated string literal is
/// actually caught for anything reachable through the public `rewrite`
/// entry point: by the time a region's content is handed to `tokenize`,
/// every quote inside it is already known to be balanced. `tokenize`'s own
/// check stays as defense in depth (and documents the invariant), but it is
/// this function's `Err` that a caller of `rewrite` actually observes for
/// `{{ a ?? 'unterminated }}`.
/// The bytes of a region that can actually make the template parser recurse.
///
/// **A string literal is ONE token however long it is**, so its interior
/// cannot add a level and must not be counted. Counting it would refuse
/// `{{ 'a very long literal…' | upper }}` for a length that costs the parser
/// nothing -- and did: a date test passes a 100 000-character pattern as a
/// literal, which is a real shape a flow can write.
///
/// Everything else is counted, whether or not it could nest, because the
/// claim only needs to be an UPPER bound on the depth: every recursion level
/// in the grammar consumes at least one token, and every token outside a
/// string literal is at least one byte.
fn significant_bytes(inner: &str) -> usize {
    let mut count = 0usize;
    let mut i = 0usize;
    while i < inner.len() {
        let c = inner[i..].chars().next().expect("i < len");
        if c == '\'' || c == '"' {
            let (end, _closed) = scan_string_literal_line_bounded(inner, i, c);
            // The whole literal is one token. An UNTERMINATED one is counted
            // the same way: `rewrite_expression` raises on it a moment later,
            // and counting it as one token cannot under-count the depth of a
            // region that is about to be refused anyway.
            count += 1;
            i = end;
            continue;
        }
        count += c.len_utf8();
        i += c.len_utf8();
    }
    count
}

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
            let (end, closed) = scan_string_literal_line_bounded(src, i, c);
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

/// Like [`scan_string_literal`], but bounded by a newline: a literal that
/// crosses a physical line without closing is treated as unterminated,
/// rather than let some unrelated quote arbitrarily later in the document
/// "rescue" it (this crate already assumes single-line Kestra expressions
/// elsewhere -- see `oracle/README.md`'s note on `date`/`dateAdd`). Used
/// only by [`find_region_close`].
///
/// **Deliberately does NOT bound by the enclosing tag's own `close`
/// delimiter** -- an earlier version did, which broke every literal that
/// legitimately contains `close`-looking text before its own closing quote
/// (see [`find_region_close`]'s doc for the measured examples). Bounding by
/// line is a narrower, correct middle ground: it still stops a literal like
/// `{{ 'a` from silently absorbing an unrelated quote on some *later line*,
/// while no longer misreading `{{ 'a}}b' }}` (closes on the SAME line, no
/// newline crossed) as broken. A coincidental rescue quote on the exact same
/// line as a genuinely unterminated literal (e.g. `{{ 'a }} it's fine`) is
/// no longer caught by this bound specifically -- that narrower guarantee
/// turned out to be unachievable without also breaking terminated literals
/// containing `close`-like text, which is the more common, more real case.
fn scan_string_literal_line_bounded(src: &str, start: usize, quote: char) -> (usize, bool) {
    let len = src.len();
    let mut i = start + quote.len_utf8();
    while i < len {
        let c = src[i..].chars().next().expect("i < len");
        if c == '\n' {
            return (i, false);
        }
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
/// own exact source text.
///
/// A `{{ }}` expression with nothing to rewrite reconstructs byte-for-byte
/// identical to its input (verified directly by
/// `an_operator_inside_a_string_literal_is_left_alone` and others). A
/// `{% %}` tag body does NOT carry the same guarantee even when its
/// value/iterable/condition side has nothing rewritable in it --
/// `rewrite_tag_body` always inserts exactly one guaranteed space at the
/// keyword/target/boundary/value junctions (and `.trim()`s each side)
/// rather than preserving the original spacing there, specifically to avoid
/// gluing the tag keyword to whatever follows it (`ifsome_call(...)` would
/// otherwise tokenize as one identifier). See `rewrite_tag_body`'s own doc
/// comment for the reasoning.
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
                // must not fire inside e.g. `isInvalid` -- which genuinely
                // contains the literal substring "isIn" as its first four
                // characters (confirmed: `"isIn" in "isInvalid"` -- unlike a
                // same-shaped-looking name such as `thisIsInvalid`, which
                // does not actually contain that substring at all, case
                // included, and so would not have demonstrated anything).
                if word == "isIn" {
                    toks.push(Tok::IsIn(word.to_string()));
                } else if is_null_literal_spelling(word) && !follows_an_attribute_dot(&toks) {
                    toks.push(Tok::Text(NULL_LITERAL_IN_MINIJINJA.to_string()));
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

/// minijinja's spelling of the value Pebble writes four ways.
///
/// `none` is the only one of the four minijinja's own lexer already knows, so
/// it is both the rewrite's target and one of its inputs — rewriting `none`
/// to `none` is a deliberate no-op rather than a case the guard has to skip.
const NULL_LITERAL_IN_MINIJINJA: &str = "none";

/// Is this word one of Pebble's four spellings of the null literal?
///
/// **Exactly four, and the case rule is not "case-insensitive".** Measured
/// through the oracle harness against the pinned artifact, one probe per
/// spelling: `null`, `NULL`, `none` and `NONE` all render empty and all
/// compare `== none` as `true`, while `None`, `Null`, `nUll`, `NoNe` and
/// `nulL` are every one of them a
/// `RootAttributeNotFoundException` — i.e. ordinary variable names. Pebble's
/// own boolean literals split the same way (`true`/`TRUE`/`false`/`FALSE` are
/// literals, `True` is a variable), which is what makes "all-lower or
/// all-upper, nothing in between" the rule rather than an artifact of the five
/// negative probes chosen.
///
/// `null-literal`, `null-literal-uppercase`, `null-literal-none-spelling` and
/// `null-literal-minijinja-spelling` are the four positive corpus entries;
/// `null-literal-mixed-case-is-not-the-literal` is the negative one, and a
/// case-insensitive rewrite here would turn it from a refusal into an answer.
fn is_null_literal_spelling(word: &str) -> bool {
    matches!(word, "null" | "NULL" | "none" | "NONE")
}

/// Is the token about to be pushed in **attribute position** — i.e. is the
/// last significant token a `.`?
///
/// `{{ o.null }}` is an attribute named `null` on both engines and already
/// answers `1` here; translating it would produce `o.none`, which minijinja's
/// parser rejects outright because `none` is a literal there and a literal is
/// not a name. So this is the guard that keeps the fix from over-reaching, and
/// `null-is-an-attribute-name` / `null-is-an-attribute-name-uppercase` are its
/// entries.
///
/// **The whitespace skip is measured, not defensive.** `{{ o . null }}`
/// renders `1` through the pinned artifact — Pebble's lexer allows whitespace
/// around the attribute dot — so a guard that only looked at the immediately
/// preceding token would translate that one and break it.
/// `null-is-an-attribute-name-across-whitespace` is that entry.
///
/// A `.` inside a string literal cannot reach this: [`tokenize`] emits the
/// whole literal, quotes included, as one [`Tok::Str`], whose `text()` is
/// therefore never the single character `.`.
fn follows_an_attribute_dot(toks: &[Tok]) -> bool {
    toks.iter()
        .rev()
        .find(|t| !is_whitespace_tok(t))
        .is_some_and(|t| t.text() == ".")
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
fn expand(tokens: &[Tok], depth: usize) -> Result<Vec<Piece>, RewriteError> {
    let mut pieces = Vec::new();
    let mut i = 0usize;

    while i < tokens.len() {
        match &tokens[i] {
            Tok::LParen(open) | Tok::LBracket(open) | Tok::LBrace(open) => {
                match find_matching_close(tokens, i) {
                    Some(close_idx) => {
                        // Checked HERE, before recursing, not only inside
                        // `resolve_pieces` -- entering a paren/bracket/brace
                        // group is one level deeper (see
                        // `MAX_NESTING_DEPTH`'s doc), and `expand` calls
                        // `resolve_flat` calls `expand` for each one, so for
                        // `{{ ((((...))))}}` this mutual recursion walks
                        // ALL THE WAY to the innermost group before
                        // `resolve_pieces` (which only runs once `expand`
                        // returns) ever gets a chance to check anything. A
                        // check placed only in `resolve_pieces` would still
                        // let the stack grow past the limit on the way down
                        // -- it has to happen at the actual recursion site.
                        if depth + 1 > MAX_NESTING_DEPTH {
                            return Err(RewriteError::TooDeeplyNested {
                                depth: depth + 1,
                                limit: MAX_NESTING_DEPTH,
                            });
                        }
                        let inner = resolve_flat(&tokens[i + 1..close_idx], depth + 1)?;
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
fn resolve_flat(tokens: &[Tok], depth: usize) -> Result<String, RewriteError> {
    let pieces = expand(tokens, depth)?;
    resolve_pieces(&pieces, depth)
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
/// 4. Only when this slice actually contains a `??`/`???`/`isIn` somewhere
///    (see `has_rewritable_op` below) -- Pebble 4.1.2's full binary-operator
///    precedence table, read out of the pinned jar's bytecode (not just the
///    subset this rewrite acts on), WITH each operator's `BinaryOperatorType`
///    (also read from the bytecode -- this is not cosmetic, see below):
///
///    ```text
///    or=10 (NORMAL)         and=15 (NORMAL)
///    is=20 (TEST)            is not=20 (TEST)          contains=20 (NORMAL)
///    equals=30 (NORMAL)      ==,!=,>,>=,<,<= =30 (NORMAL)
///    +,- =40 (NORMAL)        *,/,% =60 (NORMAL)
///    | =100 (FILTER)         ~ =110 (NORMAL)            .. =120 (NORMAL)
///    not,unary +/- =500 (UNARY)
///    ```
///
///    `is not` is its OWN registered operator (not `is` followed by a
///    separate `not`), same precedence and type as `is`. Kestra registers
///    `??`/`???`/`isIn` at 30 (`Extension.java:81-87`), the same tier as
///    `==`/`!=`/`>`/`>=`/`<`/`<=` and their word alias `equals`. Everything
///    below 30 (`or`, `and`, `is`/`is not`, `contains`) is a hard boundary a
///    coalesce/`isIn` search must not cross, checked in ascending precedence
///    order (lowest first, outermost) before the tier-30 search itself:
///    `or`, then `and`, then tier 20.
///
///    Tier 20 is NOT simply "rightmost among `is`/`is not`/`contains`, same
///    as every other tier" -- `BinaryOperatorType` is why. `contains` is
///    NORMAL (an ordinary binary operator, general expression on both
///    sides), but `is`/`is not` are TEST: their right side is a fixed
///    test-name-plus-optional-arguments grammar, parsed by
///    `parseTestInvocationExpression()`, which never recurses back into the
///    general expression parser. So `is`/`is not` can only ever extend
///    LEFT, never right, regardless of what precedence follows them --
///    [`find_rightmost_tier20`] implements this one-sided rule specifically
///    (an earlier version treated `is` as ordinary/symmetric, which was a
///    measured regression: `{{ a is defined ?? b }}` -- `(a is defined) ??
///    b` in real Pebble -- rewrote to `a is kestra_null_coalesce(defined,
///    b)`, hoisting the coalesce into test-name position, not valid
///    minijinja at all).
///
///    Within tier 30 itself, the untouched comparison operators (and
///    `equals`) have to be recognised as fellow boundaries -- not
///    rewritten, but not invisible either -- or a coalesce search sees only
///    its own three operators and swallows straight across them. Everything
///    at 40 and above (`+`/`-` through `not`) is invisible to this rewrite
///    by construction: it is never tokenized as anything but ordinary text,
///    so it can never be mistaken for a boundary and never needs its own
///    step here -- it binds tighter than `??`/`???`/`isIn` regardless, so
///    it always ends up correctly nested inside whichever operand it
///    appears in. (`|`'s FILTER type is recorded above for completeness of
///    the table read from the bytecode; this rewrite does not act on it at
///    all, filters already being ordinary `|` syntax minijinja shares.)
///
///    Measured, not assumed: `{{ a ?? b and c ?? d }}` is `(a ?? b) and
///    (c ?? d)` in Pebble; `{{ a ?? b == c }}` is `(a ?? b) == c`;
///    `{{ a ?? b is defined }}` is `true`, not the left operand `a`'s own
///    value; `{{ a is defined ?? b }}` is `(a is defined) ?? b`, not
///    `a is (defined ?? b)`; `{{ list contains a ?? b }}` is
///    `list contains (a ?? b)` -- see [`find_rightmost_word`] (`or`/`and`),
///    [`find_rightmost_tier20`] (`is`/`is not`/`contains`, one-sided for
///    the TEST pair), and [`find_rightmost_tier30`] for how each is split.
/// 5. Nothing left to resolve: reconstruct verbatim, using each remaining
///    `Op` piece's own original text (an operator with no matching partner,
///    e.g. a stray `:` with no preceding `?`, is left untouched rather than
///    guessed at). This is also where a slice with comparison operators but
///    NO `??`/`???`/`isIn` ends up (step 4 is skipped entirely for it), so a
///    plain `{{ 'b' > 'a' }}` still reconstructs byte-for-byte instead of
///    being re-formatted through the tier-30 `{left} {op} {right}` shape for
///    no reason.
fn resolve_pieces(pieces: &[Piece], depth: usize) -> Result<String, RewriteError> {
    if depth > MAX_NESTING_DEPTH {
        return Err(RewriteError::TooDeeplyNested {
            depth,
            limit: MAX_NESTING_DEPTH,
        });
    }

    if let Some(segments) = split_on_top_level_commas(pieces) {
        let resolved: Result<Vec<String>, RewriteError> = segments
            .into_iter()
            .map(|s| resolve_pieces(s, depth + 1))
            .collect();
        return Ok(resolved?.join(","));
    }

    if let Some(colon_idx) = find_map_separator_colon(pieces) {
        let key = resolve_pieces(&pieces[..colon_idx], depth + 1)?;
        let value = resolve_pieces(&pieces[colon_idx + 1..], depth + 1)?;
        return Ok(format!("{key}:{value}"));
    }

    if let Some((q_idx, colon_idx)) = find_ternary(pieces) {
        let cond = resolve_pieces(&pieces[..q_idx], depth + 1)?;
        let then_branch = resolve_pieces(&pieces[q_idx + 1..colon_idx], depth + 1)?;
        let else_branch = resolve_pieces(&pieces[colon_idx + 1..], depth + 1)?;
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
        return Ok(format!("({then_branch}) if ({cond}) else {else_branch}"));
    }

    let has_rewritable_op = pieces.iter().any(|p| {
        matches!(
            p,
            Piece::Op(
                OpKind::NullCoalesce | OpKind::UndefinedCoalesce | OpKind::IsIn,
                _
            )
        )
    });

    if has_rewritable_op {
        // `or`/`and` wrap both sides in parens -- redundant for correctness
        // (minijinja's `or`/`and` are already left-associative, same as
        // Pebble's), but NOT cosmetic: with no wrapping at all, a rightmost
        // split and a leftmost split of the SAME chain produce
        // byte-identical flat text (`A or B or C` either way -- traced by
        // hand for both directions before adding this), so nothing could
        // tell a correct rightmost-split implementation apart from a wrong
        // leftmost-split one by comparing strings. The parens make the
        // actual split point -- and therefore the actual associativity --
        // observable in the output, which is what
        // `chained_or_splits_rightmost_for_left_associativity` below
        // depends on.
        if let Some(idx) = find_rightmost_word(pieces, "or") {
            let left = resolve_pieces(&pieces[..idx], depth + 1)?;
            let right = resolve_pieces(&pieces[idx + 1..], depth + 1)?;
            return Ok(format!("({left}) or ({right})"));
        }

        if let Some(idx) = find_rightmost_word(pieces, "and") {
            let left = resolve_pieces(&pieces[..idx], depth + 1)?;
            let right = resolve_pieces(&pieces[idx + 1..], depth + 1)?;
            return Ok(format!("({left}) and ({right})"));
        }

        // Tier 20: `is`/`is not`/`contains`, still lower precedence than
        // the 30 tier `??`/`???`/`isIn` share with the comparison
        // operators, so this must be checked before `find_rightmost_tier30`
        // for the same reason `or`/`and` are checked before it -- EXCEPT
        // `is`/`is not` are `BinaryOperatorType.TEST`, not `NORMAL` like
        // `contains`: their right side is a fixed test-name (plus optional
        // arguments) grammar, never a general expression, so precedence 20
        // governs only how far an `is` node reaches LEFT, never right --
        // see `find_rightmost_tier20`'s doc for the measured regression
        // this asymmetry fixes. `is not TESTNAME` needs no special
        // two-token handling: splitting at the bare `is` alone leaves
        // `not TESTNAME` as ordinary text on the right side, which
        // `resolve_pieces` reconstructs verbatim, reproducing `is not`
        // correctly with no extra code.
        if let Some(idx) = find_rightmost_tier20(pieces) {
            let is_contains = matches!(&pieces[idx], Piece::Atom(s) if s == "contains");
            let left = resolve_pieces(&pieces[..idx], depth + 1)?;
            let right = resolve_pieces(&pieces[idx + 1..], depth + 1)?;
            // `contains` is NORMAL (both sides general expressions): parens
            // on both sides, same reasoning and same associativity-proof
            // need as `or`/`and` above. `is`/`is not`'s right side is
            // wrapped in NEITHER -- `a is (defined)` is not the same
            // grammar position as a general expression and minijinja's own
            // test-invocation syntax does not accept parens there; its left
            // side needs no parens either, since the one-sided exclusion in
            // `find_rightmost_tier20` (not a rendering trick) is what keeps
            // an `is` node from ever incorrectly becoming the outer split.
            if is_contains {
                return Ok(format!("({left}) contains ({right})"));
            }
            // Only ever `is` here -- `find_rightmost_tier20` returns just
            // `is`/`contains`, and `contains` already returned above, so
            // there is no third value to hold in a variable for.
            return Ok(format!("{left} is {right}"));
        }

        if let Some(idx) = find_rightmost_tier30(pieces) {
            let left = resolve_pieces(&pieces[..idx], depth + 1)?;
            let right = resolve_pieces(&pieces[idx + 1..], depth + 1)?;
            return Ok(match &pieces[idx] {
                Piece::Op(OpKind::NullCoalesce, _) => {
                    format!("kestra_null_coalesce({left}, {right})")
                }
                Piece::Op(OpKind::UndefinedCoalesce, _) => {
                    format!("kestra_undefined_coalesce({left}, {right})")
                }
                // A call, not minijinja's `in` -- see this module's doc
                // comment. The parens on both operands stay for the same
                // reason they were there when this emitted `in`: `left` and
                // `right` are already-resolved text that may contain
                // operators, and a call argument is a whole expression
                // position. A call is inherently atomic, so the SURROUNDING
                // tier-20 concern the old comment described is now handled
                // by the call syntax itself rather than by the outer parens.
                Piece::Op(OpKind::IsIn, _) => {
                    format!("kestra_is_in(({left}), ({right}))")
                }
                // The untouched comparison operators, recognised here only
                // as a tier-30 boundary so a coalesce/isIn search does not
                // cross them -- rendered back out exactly as-is, never
                // rewritten.
                Piece::Atom(op) => format!("{left} {op} {right}"),
                Piece::Op(OpKind::Question | OpKind::Colon, _) => {
                    unreachable!("find_rightmost_tier30 never returns a ternary token")
                }
            });
        }
    }

    Ok(pieces
        .iter()
        .map(|p| match p {
            Piece::Atom(s) => s.as_str(),
            Piece::Op(_, s) => s.as_str(),
        })
        .collect())
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

/// Finds the last (rightmost) top-level bare-word `word` (e.g. `"or"`)
/// among `pieces` -- `or`/`and` are ordinary identifiers at the tokenizer
/// level (same representation as any other bare word; matched here by exact
/// text, same principle as `isIn` never being confused with a bare `in` --
/// see `find_top_level_word`'s doc), and rightmost is the correct split
/// point for left-associative chaining, same reasoning as
/// [`find_rightmost_tier30`].
fn find_rightmost_word(pieces: &[Piece], word: &str) -> Option<usize> {
    pieces.iter().enumerate().rev().find_map(|(i, p)| match p {
        Piece::Atom(s) if s == word => Some(i),
        _ => None,
    })
}

/// True for a piece this rewrite treats as a tier-30 boundary
/// (`??`/`???`/`isIn`, or the untouched `==`/`!=`/`>`/`>=`/`<`/`<=`/
/// `equals`) -- shared by [`find_rightmost_tier20`] (to disqualify an `is`
/// candidate that one of these follows) and [`find_rightmost_tier30`]
/// itself.
fn is_tier30_trigger(p: &Piece) -> bool {
    matches!(
        p,
        Piece::Op(
            OpKind::NullCoalesce | OpKind::UndefinedCoalesce | OpKind::IsIn,
            _
        )
    ) || matches!(
        p,
        Piece::Atom(s) if matches!(s.as_str(), "==" | "!=" | ">" | ">=" | "<" | "<=" | "equals")
    )
}

/// Finds the rightmost tier-20 boundary (`is`/`is not`/`contains`) among
/// `pieces` -- but, unlike every other precedence step in this module, NOT
/// simply "rightmost among all occurrences of these words." `contains`
/// (`BinaryOperatorType.NORMAL`) is ordinary rightmost-among-the-tier, same
/// as `or`/`and`/tier-30; `is`/`is not` (`BinaryOperatorType.TEST`) can only
/// ever extend LEFT, never right -- see [`resolve_pieces`]'s doc comment for
/// the full precedence table and why (this rewrite's I1 regression was
/// exactly this asymmetry: `{{ a is defined ?? b }}` rewritten with `is`
/// treated as symmetric hoists the following coalesce into TEST-NAME
/// position, `a is kestra_null_coalesce(defined, b)`, not valid minijinja).
///
/// Implementation: an explicit loop, not `find_map`, and deliberately NOT
/// equivalent to one. Scanning right to left, a disqualified `is` (a
/// tier-30 trigger -- see [`is_tier30_trigger`] -- appears anywhere to its
/// right) `return`s `None` immediately, suppressing the WHOLE tier-20 step
/// here rather than letting the scan continue further left. A `find_map`
/// version, which treats a disqualified candidate as merely "not a match,
/// keep looking," can walk past it onto a DIFFERENT tier-20 word that is
/// actually part of the disqualified `is`'s own left operand rather than a
/// sibling split point -- measured: `{{ l contains x is defined ?? y }}` is
/// `((l contains x) is defined) ?? y` in Pebble (`true` for a non-empty
/// `l`/matching `x`); the `find_map` version instead accepted the earlier
/// `contains`, grouping it as the OUTER operation (`false`).
fn find_rightmost_tier20(pieces: &[Piece]) -> Option<usize> {
    for (i, p) in pieces.iter().enumerate().rev() {
        match p {
            Piece::Atom(s) if s == "contains" => return Some(i),
            Piece::Atom(s) if s == "is" => {
                // A disqualified `is` (a tier-30 trigger follows it)
                // suppresses the WHOLE tier-20 step here -- it must not
                // fall through to keep searching further left, the way a
                // plain `find_map` naturally would on `None`. Continuing
                // risks landing on a DIFFERENT tier-20 word (typically
                // `contains`) that is actually part of THIS `is`'s own
                // left operand, not a sibling split point at this level.
                // Measured: `{{ l contains x is defined ?? y }}` is
                // `((l contains x) is defined) ?? y` in Pebble; the
                // find_map version instead skipped the disqualified `is`
                // and accepted the earlier `contains`, grouping `contains`
                // as the OUTER operation and producing the `false` reading
                // instead of Pebble's `true`.
                return if pieces[i + 1..].iter().any(is_tier30_trigger) {
                    None
                } else {
                    Some(i)
                };
            }
            _ => {}
        }
    }
    None
}

/// Finds the last (rightmost) precedence-30 boundary among `pieces` --
/// `??`/`???`/`isIn` (which this rewrite acts on) together with
/// `==`/`!=`/`>`/`>=`/`<`/`<=` AND their word alias `equals` (all of which
/// it recognises as fellow same-tier operators but never touches).
/// Rightmost is correct for left-associative chaining among
/// SAME-precedence operators regardless of which specific operator each
/// occurrence is: `a ?? b == c` chains left-to-right through the tier
/// exactly like `a - b + c` does through `+`/`-`'s shared tier in a
/// conventional expression grammar, giving `(a ?? b) == c` -- confirmed
/// against real Pebble, not assumed (see `oracle/corpus.json`'s
/// `coalesce-eq` entry).
fn find_rightmost_tier30(pieces: &[Piece]) -> Option<usize> {
    // `equals` (Pebble's word alias for `==`, same tier) is folded into
    // `is_tier30_trigger` -- omitting it would not silently misgroup
    // anything on its own (an unrecognised bare word is just inert text),
    // but it would let a coalesce search swallow straight across it exactly
    // like the pre-fix `==` gap did.
    pieces
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, p)| is_tier30_trigger(p).then_some(i))
}

/// Tokenizes and rewrites the inside of one `{{ }}` region as a single
/// expression.
fn rewrite_expression(inner: &str) -> Result<String, RewriteError> {
    let tokens = tokenize(inner)?;
    resolve_flat(&tokens, 0)
}

/// Tokenizes and rewrites the inside of one `{% %}` region as a tag body --
/// not a single expression. See this module's doc ("`{{ }}` expressions vs.
/// `{% %}` tags") for the shape.
///
/// Unlike `rewrite_expression`, this does NOT preserve exact original
/// whitespace even when nothing is rewritten: every keyword/target/
/// boundary/value junction gets exactly one inserted space (each side
/// `.trim()`med first), never the original spacing verbatim. This is not
/// cosmetic -- a tag keyword glued directly to what follows it re-tokenizes
/// as one identifier (`ifsome_call(...)` is not `if` followed by an
/// expression), so a guaranteed separator is required at those specific
/// junctions regardless of what the source originally had there.
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
            let value_tokens = &rest[b_idx + 1..];
            let value_pieces = expand(value_tokens, 0)?;
            let value_has_top_level_ternary = find_ternary(&value_pieces).is_some();
            let value = resolve_pieces(&value_pieces, 0)?;
            let value = value.trim();
            // `for`'s iterable needs the same whole-conditional parens as
            // `if`'s condition below, for the same reason (see there) --
            // measured directly: `{% for i in a ? b : c %}` fails to parse
            // the same way `{% if a ? b : c %}` does. `set`'s value does
            // NOT need this -- measured separately: `{% set x = a ? b : c
            // %}` parses fine in minijinja without wrapping, because `set`'s
            // grammar already expects a full expression on the right of
            // `=`, unlike `if`'s condition-position and `for`'s
            // iterable-position grammars. Wrapped only when a top-level
            // ternary is actually present (see the `None` branch below for
            // why "unconditionally" was wrong).
            let value = if keyword == "for" && value_has_top_level_ternary && !value.is_empty() {
                format!("({value})")
            } else {
                value.to_string()
            };
            format!("{} {boundary_text} {value}", target.trim())
        }
        None => {
            let pieces = expand(rest, 0)?;
            let has_top_level_ternary = find_ternary(&pieces).is_some();
            let expr = resolve_pieces(&pieces, 0)?;
            let expr = expr.trim();
            // Whole-conditional parens, but ONLY when a top-level ternary
            // is actually present in `rest` -- checked via `find_ternary`
            // on the un-resolved pieces, before `resolve_pieces` turns it
            // into the `(...) if (...) else ...` shape that needs this
            // wrapping in the first place. An EARLIER version of this
            // branch wrapped unconditionally for every tag that reaches
            // here, on the theory that a parenthesized function call or
            // bare identifier is exactly as valid as the unparenthesized
            // form -- true for `if`/`elseif`/`unless`, but that theory
            // does not hold for every tag whose whole remainder is "one
            // expression" in the sense this branch handles: `{% filter
            // upper %}` is real Pebble (measured through the harness:
            // renders `X` for input `x`), and `upper` there is a
            // filter-chain name in filter-name position, not a general
            // expression -- unconditionally wrapping it produced `{% filter
            // (upper)%}`, which minijinja's `filter` tag grammar rejects
            // ("unexpected '(', expected identifier"). Wrapping only when
            // a ternary is actually present fixes `{% filter %}` (nothing
            // to wrap, since `upper` has no top-level `?`) without losing
            // the original `{% if flag ? 1 : 0 %}` fix (which does have
            // one).
            //
            // Without this, a ternary spliced straight after the tag
            // keyword produces a real parse error, measured directly:
            // `{% if flag ? 1 : 0 %}` rewrites (pre-fix) to
            // `{% if ( 1 ) if ( flag ) else  0%}`, which minijinja's `if`
            // statement grammar rejects outright ("unexpected identifier,
            // expected end of block") -- not a silent semantic drift like
            // I3's then-branch parens, a hard failure every time a
            // ternary appears inside `{% if %}`/`{% elseif %}`/`{% unless
            // %}`.
            if expr.is_empty() {
                String::new()
            } else if has_top_level_ternary {
                format!("({expr})")
            } else {
                expr.to_string()
            }
        }
    };

    Ok(format!("{prefix} {rewritten}"))
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
///
/// **Honesty note (a prior review flagged this exact gap elsewhere in the
/// module and it applies here too):** the depth-tracking that skips a
/// nested `in`/`=` (e.g. inside a `for` tag's iterable expression, `for x
/// in f(y in z)`) and the `==`-grouping's role in keeping `=` unambiguous
/// are both believed necessary by construction, but neither is currently
/// exercised by a test that would fail if it were removed -- no corpus
/// entry or unit test has a nested `in`/`=` inside a `for`/`set` boundary
/// search. Documented as a known gap rather than claimed as verified.
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
    use super::{
        rewrite, RewriteError, EXPRESSION_STACK_MARGIN, MAX_EXPRESSION_BYTES,
        WORST_STACK_PER_EXPRESSION_BYTE,
    };

    /// Every rewrite test routes through this, not through `rewrite(...)
    /// .unwrap()` directly: a test that asserts an exact string is worth
    /// nothing if that string is not what it claims to be -- syntax
    /// minijinja actually accepts. `Environment::template_from_str` only
    /// parses (no function/filter needs registering for it to succeed;
    /// name resolution is a render-time concern, not a parse-time one), so
    /// this is a real, cheap parseability gate, not a render smoke test.
    fn rewrite_and_parse(expr: &str) -> String {
        let out = rewrite(expr).unwrap_or_else(|e| panic!("rewrite({expr:?}) failed: {e}"));
        let env = minijinja::Environment::new();
        if let Err(e) = env.template_from_str(&out) {
            panic!(
                "rewrite({expr:?}) produced output minijinja cannot parse:\n  {out:?}\nparse error: {e}"
            );
        }
        out
    }

    /// Asserts the rewritten STRING only, deliberately bypassing the
    /// minijinja parse gate `rewrite_and_parse` enforces everywhere else in
    /// this module. This exists for exactly one, narrow reason: `equals`
    /// and `contains`, as bare infix words, are not valid minijinja syntax
    /// under ANY spelling -- confirmed directly against
    /// `Environment::template_from_str` (`{{ a equals b }}` and
    /// `{{ a contains b }}` both fail with "unexpected identifier, expected
    /// end of variable block"), independent of this rewrite entirely. That
    /// means `rewrite_and_parse` would panic at the parse gate on BOTH a
    /// correct and a wrong rewrite of any expression using either word,
    /// telling you nothing about whether the rewrite got the boundary in
    /// the right place -- the one thing this helper is for asserting.
    ///
    /// This is NOT a general escape hatch from the parse gate. Every other
    /// test in this module routes through `rewrite_and_parse`, and a new
    /// use of this helper should come with the same justification as the
    /// ones below: the word involved has no minijinja spelling at all, so
    /// parseability cannot be the thing under test here -- shape is.
    fn rewrite_shape_only(expr: &str) -> String {
        rewrite(expr).unwrap_or_else(|e| panic!("rewrite({expr:?}) failed: {e}"))
    }

    /// The rewriter must not touch an operator inside a string literal. A
    /// regex would; that is why this is a tokenizer.
    #[test]
    fn an_operator_inside_a_string_literal_is_left_alone() {
        assert_eq!(rewrite_and_parse("{{ 'a ?? b' }}"), "{{ 'a ?? b' }}");
        assert_eq!(
            rewrite_and_parse(r#"{{ "x ? y : z" }}"#),
            r#"{{ "x ? y : z" }}"#
        );
    }

    /// `???` must not be mistaken for `??` followed by `?`.
    #[test]
    fn the_longer_operator_wins() {
        // Exact match, not `.contains("undefined")` /
        // `!.contains("null_coalesce")`: those substring checks pass for
        // TWO wrong implementations that were never actually ruled out --
        // reversing the operands
        // (`kestra_undefined_coalesce(right, left)`, silently swapping the
        // primary value and its missing-attribute fallback) and emitting a
        // nonexistent function name like `kestra_undefined_coalesce_x`
        // (still contains "undefined", still excludes "null_coalesce").
        // `??`'s own test
        // (`double_question_mark_becomes_the_null_coalescing_call`) has
        // long been pinned this loosely too, but THAT operator is also
        // exact-match-pinned everywhere else in this module (swapping its
        // operands alone kills roughly a dozen other tests); `???` had no
        // exact-string assertion anywhere until this one.
        assert_eq!(
            rewrite_and_parse("{{ a ??? b }}"),
            "{{kestra_undefined_coalesce( a ,  b )}}"
        );
    }

    /// Replaces a weaker, counting-based assertion (`.matches(" if
    /// ").count() == 2`) that a swapped condition/then-branch, or the
    /// missing-then-branch-parens bug this module used to have, would also
    /// have passed. Pinned to the exact output so both mistakes are caught.
    #[test]
    fn nested_ternaries_rewrite_innermost_first() {
        let out = rewrite_and_parse("{{ a ? (b ? 'x' : 'y') : 'z' }}");
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
        let out = rewrite_and_parse("{{ a ? b ? c : d : e }}");
        assert_eq!(out, "{{(( c ) if ( b ) else  d ) if ( a ) else  e }}");
    }

    #[test]
    fn text_outside_an_expression_is_untouched() {
        assert_eq!(rewrite_and_parse("literal ?? text"), "literal ?? text");
    }

    /// `the_longer_operator_wins` above only pins the negative half (`???`
    /// must not look like `??`); this pins that a bare `??` DOES produce the
    /// null-coalescing form -- without it, mapping `??` to nothing at all
    /// would still pass every other test in this module.
    #[test]
    fn double_question_mark_becomes_the_null_coalescing_call() {
        let out = rewrite_and_parse("{{ a ?? b }}");
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
    fn is_in_becomes_a_kestra_is_in_call() {
        // Exact match: the earlier `out.contains(" in ")` /
        // `!out.contains("isIn")` pair is satisfied by
        // `format!("({right} in {left})")` too -- which INVERTS the
        // operator (`x isIn list` silently becoming `list in x`) -- and by
        // a version with the wrapping parens dropped entirely, since
        // neither substring check says anything about operand order or
        // grouping.
        assert_eq!(
            rewrite_and_parse("{{ a isIn b }}"),
            "{{kestra_is_in(( a ), ( b ))}}"
        );

        // Combined forms. Nothing pinned these before -- there was exactly
        // ONE positive `isIn` test in this file -- so a change to the
        // emitted call's shape could have broken `isIn` in composition
        // while the single-operator test stayed green. Each of these was
        // parse-checked by `rewrite_and_parse`, which is what makes them
        // worth having rather than three more string assertions.
        //
        // `??` and `isIn` are the SAME tier (30), and Pebble's binary
        // operators at one tier are left-associative -- which this module
        // gets by splitting at the RIGHTMOST such operator. So whichever of
        // the two is written second is the outer call, and the pair is not
        // symmetric. Both directions are asserted because a rewrite that
        // split leftmost would still pass one of them.
        assert_eq!(
            rewrite_and_parse("{{ a ?? b isIn c }}"),
            "{{kestra_is_in((kestra_null_coalesce( a ,  b )), ( c ))}}"
        );
        assert_eq!(
            rewrite_and_parse("{{ a isIn b ?? c }}"),
            "{{kestra_null_coalesce(kestra_is_in(( a ), ( b )),  c )}}"
        );
        // Lower-tier `and` is a boundary: neither `isIn` may swallow it.
        assert_eq!(
            rewrite_and_parse("{{ a isIn b and c isIn d }}"),
            "{{(kestra_is_in(( a ), ( b ))) and (kestra_is_in(( c ), ( d )))}}"
        );
        // As a ternary condition, and inside a tag body -- the two positions
        // where a bare `in` used to be at risk of colliding with the `for`
        // tag's own `in` keyword. Now that the output is a call, it cannot.
        assert_eq!(
            rewrite_and_parse("{{ a isIn b ? 'y' : 'n' }}"),
            "{{( 'y' ) if (kestra_is_in(( a ), ( b ))) else  'n' }}"
        );
        assert_eq!(
            rewrite_and_parse("{% if a isIn b %}y{% endif %}"),
            "{% if kestra_is_in(( a ), ( b ))%}y{% endif %}"
        );

        // Not a regex: `isIn` inside a string literal survives untouched --
        // a global, quote-blind `str::replace("isIn", "in")` would corrupt
        // this, and nothing else in this module exercised that.
        assert_eq!(rewrite_and_parse("{{ 'a isIn b' }}"), "{{ 'a isIn b' }}");

        // Same principle outside any `{{ }}`/`{% %}` region at all.
        assert_eq!(rewrite_and_parse("literal isIn text"), "literal isIn text");
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

    /// The unterminated-string scan is bounded by a physical line (see
    /// `scan_string_literal_line_bounded`'s doc for the full reasoning): a
    /// literal is unterminated only if it never finds its closing quote
    /// before either a newline or the end of input.
    #[test]
    fn an_unterminated_string_never_closes_before_a_newline_or_eof() {
        // No closing quote anywhere in the document at all.
        assert!(rewrite("{{ 'a }} its fine").is_err());

        // A quote exists, but only on a LATER LINE -- must not rescue a
        // literal that never closed on its own line. This is the part of
        // the original concern (an unrelated quote elsewhere "rescuing" a
        // broken literal) that a line bound can actually guarantee without
        // also breaking terminated-same-line literals -- see the sibling
        // test below for why a same-line rescue is no longer treated as an
        // error (that narrower guarantee is not achievable at the same
        // time as the fix below).
        let multiline = "{{ a ?? 'x }} tail with no quote on this line
but there is one on the next: '";
        let err = rewrite(multiline).unwrap_err();
        assert!(
            matches!(err, RewriteError::UnterminatedString { .. }),
            "{err:?}"
        );
    }

    /// The positive counterpart Critical 2 found missing: a string literal
    /// that DOES close, but merely contains text that looks like the
    /// region's own close delimiter, must not be misdiagnosed as
    /// unterminated. An earlier version of this rewriter bailed out the
    /// moment `close` (`"}}"`/`"%}"`) appeared anywhere while scanning a
    /// string's body -- which incidentally "fixed" the negative case in the
    /// sibling test above, but broke every one of these, all real Pebble
    /// input. The first is not contrived: a nested-object `jq` query is the
    /// same shape the corpus's own `filter-jq` entry already exercises.
    #[test]
    fn a_string_literal_may_contain_the_regions_close_delimiter() {
        assert_eq!(
            rewrite_and_parse("{{ json | jq('{a: {b: .c}}') }}"),
            "{{ json | jq('{a: {b: .c}}') }}"
        );
        assert_eq!(rewrite_and_parse("{{ 'a}}b' }}"), "{{ 'a}}b' }}");
        assert_eq!(
            rewrite_and_parse("{{ x | replace({'}}': 'x'}) }}"),
            "{{ x | replace({'}}': 'x'}) }}"
        );
        let out = rewrite_and_parse("{% if x == 'a%}b' %}y{% endif %}");
        assert_eq!(out, "{% if x == 'a%}b'%}y{% endif %}");
    }

    /// A comma is an operand boundary an operator must not cross: `f(x, a ??
    /// b)` is a two-argument call, not one call whose (wrong-arity) sole
    /// argument is `kestra_null_coalesce(x, a, b)`.
    #[test]
    fn commas_bound_operator_rewrites_inside_a_call() {
        let out = rewrite_and_parse("{{ f(x, a ?? b) }}");
        assert_eq!(out, "{{ f(x,kestra_null_coalesce( a ,  b)) }}");
    }

    /// Same principle for a list literal's elements.
    #[test]
    fn commas_bound_operator_rewrites_inside_a_list() {
        let out = rewrite_and_parse("{{ [1, a ?? b] }}");
        assert_eq!(out, "{{ [1,kestra_null_coalesce( a ,  b)] }}");
    }

    /// A ternary as a later argument must not be glued to an earlier one by
    /// the comma-boundary fix -- each argument still resolves fully on its
    /// own.
    #[test]
    fn a_ternary_argument_after_a_comma_still_resolves() {
        let out = rewrite_and_parse("{{ f(x, c ? a : b) }}");
        assert_eq!(out, "{{ f(x,( a ) if ( c ) else  b) }}");
    }

    /// A map literal's `:` must not be mistaken for a ternary's `:`, and a
    /// map literal must nest like `(`/`[` so an outer ternary's own `?`/`:`
    /// is not confused by an inner map's colon.
    #[test]
    fn map_literals_do_not_confuse_the_ternary() {
        let out = rewrite_and_parse("{{ flag ? {'a': 1} : {'b': 2} }}");
        assert_eq!(out, "{{( {'a': 1} ) if ( flag ) else  {'b': 2} }}");
    }

    /// An operator inside a map literal's *value* must still be rewritten,
    /// without the map's own key/colon syntax being swept into its operand.
    #[test]
    fn an_operator_inside_a_map_value_still_resolves() {
        let out = rewrite_and_parse("{{ {'k': a ?? b} }}");
        assert_eq!(out, "{{ {'k':kestra_null_coalesce( a ,  b)} }}");
    }

    /// `{% if %}` tag bodies must be rewritten as an expression, not left
    /// alone (a documented capability that must not silently do nothing)
    /// and not treated as if the leading `if` keyword were part of the
    /// expression (which produced a parse error before this fix).
    #[test]
    fn if_tag_bodies_rewrite_their_condition() {
        let out = rewrite_and_parse("{% if a ?? b %}y{% endif %}");
        assert_eq!(out, "{% if kestra_null_coalesce( a ,  b )%}y{% endif %}");
    }

    /// `{% set %}`: the target left of `=` is a name, never rewritten; the
    /// value right of `=` is rewritten normally.
    ///
    /// The target is deliberately `isIn`, not a plain `x`: with a target
    /// that has nothing rewritable in it, a mutant that resolves the target
    /// through `resolve_flat` instead of copying its token text verbatim
    /// (i.e. deletes the boundary this test's name claims to prove)
    /// produces byte-identical output and this test cannot tell the
    /// difference. Confirmed by making that exact mutation locally
    /// (`target: String = rest[..b_idx].iter().map(Tok::text).collect()` ->
    /// `resolve_flat(&rest[..b_idx])?`) and re-running: the mutant turns the
    /// lone `isIn` token into `(  in  )` (no left/right operand, so
    /// `resolve_pieces`'s `isIn` case produces an empty-operand call) --
    /// which does not just make `assert_eq!` fail, it fails to PARSE at all
    /// (`rewrite_and_parse` panics), an even stronger kill than expected.
    /// `isIn` is a perfectly ordinary identifier in minijinja, so this is
    /// still realistic syntax, just an unusual variable name chosen to be
    /// discriminating.
    #[test]
    fn set_tag_bodies_rewrite_only_the_value_side() {
        let out = rewrite_and_parse("{% set isIn = a ?? b %}");
        assert_eq!(out, "{% set isIn = kestra_null_coalesce( a ,  b )%}");
    }

    /// `{% for %}`: the loop variable left of `in` is a name, never
    /// rewritten; the iterable right of `in` is rewritten normally. The
    /// tag's own `in` keyword must survive as plain syntax, not be mistaken
    /// for this rewrite's `isIn` output -- so the loop variable is
    /// deliberately named `isIn` too (same reasoning and same mutant-catching
    /// property as the `set` test above), doubling as proof that the tag's
    /// structural `in` boundary search is not confused by a variable whose
    /// name contains the substring `isIn`.
    #[test]
    fn for_tag_bodies_rewrite_only_the_iterable_side() {
        let out = rewrite_and_parse("{% for isIn in list ?? [] %}y{% endfor %}");
        assert_eq!(
            out,
            "{% for isIn in kestra_null_coalesce( list ,  [] )%}y{% endfor %}"
        );
    }

    /// `for`'s iterable-side ternary-parens fix (`{% for i in a ? b : c
    /// %}` needs the same whole-conditional wrapping `{% if flag ? 1 : 0
    /// %}` does, per `rewrite_tag_body`'s doc) had zero coverage: the
    /// sibling test above uses `list ?? []`, which has no ternary in it at
    /// all, so it cannot see this. Real Pebble renders `{% for i in a ? b :
    /// c %}12{% endfor %}` as `12` regardless of which of `b`/`c` is
    /// selected, and the unmutated rewrite already parses; the gap this
    /// test closes is that nothing was pinning `value_has_top_level_
    /// ternary`'s wrapping specifically.
    #[test]
    fn for_tag_iterable_ternary_is_wrapped_in_parens() {
        assert_eq!(
            rewrite_and_parse("{% for i in a ? b : c %}12{% endfor %}"),
            "{% for i in (( b ) if ( a ) else  c)%}12{% endfor %}"
        );
    }

    /// A ternary inside a `{% if %}` tag: proves the tag-body path shares
    /// the same expression rewriter (including the I3 then-branch-parens
    /// fix) as a `{{ }}` region, not a separate, weaker implementation.
    #[test]
    fn a_ternary_inside_an_if_tag_resolves() {
        let out = rewrite_and_parse("{% if flag ? 'a' : 'b' %}y{% endif %}");
        assert_eq!(out, "{% if (( 'a' ) if ( flag ) else  'b')%}y{% endif %}");
    }

    /// F1: `{% filter %}` is real Pebble (measured through the harness:
    /// `{% filter upper %}x{% endfilter %}` renders `X`), and its body is a
    /// filter-chain name in filter-name position, not a general expression
    /// -- unconditionally wrapping tag bodies in parens (an earlier version
    /// of this fix round did exactly that, to fix `{% if flag ? 1 : 0 %}`)
    /// broke it: `{% filter (upper)%}` is not valid minijinja. Wrapping is
    /// now conditional on an actual top-level ternary being present, which
    /// `upper` never has, so this tag is untouched. Also proves the fix
    /// still works when there IS an operator to rewrite inside the
    /// `{% filter %}` block's body (a `{{ }}` region nested inside it,
    /// unrelated to the tag's own name-position argument).
    #[test]
    fn filter_tag_bodies_are_not_wrapped_in_parens() {
        assert_eq!(
            rewrite_and_parse("{% filter upper %}x{% endfilter %}"),
            "{% filter upper%}x{% endfilter %}"
        );
        assert_eq!(
            rewrite_and_parse("{% filter upper %}{{ a ?? b }}{% endfilter %}"),
            "{% filter upper%}{{kestra_null_coalesce( a ,  b )}}{% endfilter %}"
        );
    }

    /// F2: tier 20 (`is`/`contains`, both lower precedence than the 30
    /// `??`/`???`/`isIn` share with the comparison operators) was missing
    /// entirely -- measured against real Pebble: `{{ a ?? b is defined }}`
    /// with `a="A", b="B"` renders `true` in Pebble (`(a ?? b) is defined`,
    /// since `a` is non-null so `a ?? b` is `"A"`, which is defined), but
    /// without a tier-20 boundary this rewrite's old coalesce search
    /// swallowed `is defined` into the RIGHT operand, producing
    /// `kestra_null_coalesce(a, b is defined)` -- which returns `a` itself
    /// (`"A"`) once `a` turns out non-null, not `true`. `is not TESTNAME`
    /// needs no special two-token handling: splitting at the bare `is`
    /// alone leaves `not TESTNAME` as inert text on the right side.
    #[test]
    fn coalesce_does_not_swallow_across_is() {
        assert_eq!(
            rewrite_and_parse("{{ a ?? b is defined }}"),
            "{{kestra_null_coalesce( a ,  b ) is  defined }}"
        );
        // `is not none`, not `is not null` -- disclosure, not a silent
        // substitution: minijinja has no `null` test at all (it parses --
        // "null" is just an identifier in test-name position -- but fails
        // at RENDER with `unknown test: test null is unknown`; confirmed
        // directly). `oracle/corpus.json`'s `coalesce-is-not-null` keeps
        // real Pebble's actual test name (`null`) since the corpus records
        // real Kestra source, not minijinja spelling -- see
        // `oracle/README.md`'s note on why that entry cannot discriminate
        // at render time either, for the same underlying reason. This unit
        // test uses `none` specifically so it exercises a name that will
        // ALSO still mean something once Task 3 renders it, unlike `null`.
        assert_eq!(
            rewrite_and_parse("{{ a ?? b is not none }}"),
            "{{kestra_null_coalesce( a ,  b ) is  not none }}"
        );
    }

    /// I1 regression: `is`/`is not` are `BinaryOperatorType.TEST` in
    /// Pebble, not `NORMAL` -- their right side is a fixed test-name
    /// grammar, never a general expression, so (unlike every other
    /// precedence tier this module handles) they must never be chosen as
    /// the split point when a tier-30 trigger FOLLOWS them: `a is defined`
    /// cannot absorb `?? b` into its test-name slot. Measured against real
    /// Pebble: `{{ a is defined ?? b }}` groups as `(a is defined) ?? b`,
    /// not `a is (defined ?? b)`. An earlier version of this rewrite
    /// treated `is` as an ordinary symmetric tier-20 boundary (correct for
    /// `contains`, wrong for `is`/`is not`), which turned this exact input
    /// into `a is kestra_null_coalesce(defined, b)` -- `unknown test:
    /// kestra_null_coalesce`, a hard parse-time-adjacent failure, not a
    /// silent drift.
    #[test]
    fn is_never_absorbs_a_following_coalesce() {
        assert_eq!(
            rewrite_and_parse("{{ a is defined ?? b }}"),
            "{{kestra_null_coalesce( a is defined ,  b )}}"
        );
        assert_eq!(
            rewrite_and_parse("{{ a is not none ?? b }}"),
            "{{kestra_null_coalesce( a is not none ,  b )}}"
        );
    }

    /// `contains` is `BinaryOperatorType.NORMAL` (unlike `is`/`is not`),
    /// confirmed against the harness: `{{ list contains a ?? b }}` groups
    /// as `list contains (a ?? b)`, ordinary rightmost-among-the-tier
    /// behavior, no left-only restriction needed. Routed through
    /// `rewrite_shape_only`, not `rewrite_and_parse`: `contains`, as a bare
    /// infix word, is not valid minijinja syntax under any spelling (see
    /// that helper's doc) -- this asserts the REWRITE'S shape, which is
    /// independently correct regardless of whether minijinja can parse the
    /// word `contains` itself.
    ///
    /// Also pins two mutations, both confirmed live (mutated and re-run,
    /// not assumed) before finalizing this test:
    ///
    /// - Dropping `"equals"` from `find_rightmost_tier30`'s atom set
    ///   changes the FIRST assertion's output to
    ///   `"{{kestra_null_coalesce( a ,  b equals c )}}"` (the coalesce
    ///   swallows straight across `equals`, same failure class as the
    ///   pre-fix `==` gap).
    /// - Hardcoding the WRONG word in the `contains` branch (i.e. the bug
    ///   "emits a hardcoded word instead of the matched one" actually looks
    ///   like now that `contains` has its own branch -- it renders as
    ///   `is` there, not as the shared `{word}` fallback below it, which is
    ///   only ever reached with `word == "is"` since `contains` returns
    ///   early) changes the SECOND assertion's output to
    ///   `"{{(kestra_null_coalesce( a ,  b )) is ( c )}}"`.
    #[test]
    fn coalesce_does_not_swallow_across_equals_or_contains() {
        assert_eq!(
            rewrite_shape_only("{{ a ?? b equals c }}"),
            "{{kestra_null_coalesce( a ,  b ) equals  c }}"
        );
        let out = rewrite_shape_only("{{ a ?? b contains c }}");
        assert_eq!(out, "{{(kestra_null_coalesce( a ,  b )) contains ( c )}}");
    }

    /// A disqualified `is` (a tier-30 trigger follows it) must suppress
    /// the WHOLE tier-20 step, not just that one candidate --
    /// `find_rightmost_tier20`'s doc comment has the full reasoning.
    /// Measured against real Pebble: `{{ l contains x is defined ?? y }}`
    /// is `true` (grouped `((l contains x) is defined) ?? y`); a version
    /// that let the search continue past the disqualified `is` to accept
    /// the earlier `contains` instead grouped `contains` as the OUTER
    /// operation and produced Pebble's `false` reading. Routed through
    /// `rewrite_shape_only`: `contains` cannot parse in minijinja under any
    /// spelling (see that helper's doc), so this is latent -- provably the
    /// correct SHAPE, not yet renderable -- until `contains` gets a real
    /// minijinja translation.
    #[test]
    fn a_disqualified_is_suppresses_the_whole_tier20_step() {
        assert_eq!(
            rewrite_shape_only("{{ l contains x is defined ?? y }}"),
            "{{kestra_null_coalesce( l contains x is defined ,  y )}}"
        );
    }

    /// `find_rightmost_tier20`'s rightmost-split direction, proven the same
    /// way `chained_or_splits_rightmost_for_left_associativity` proves it
    /// for `or`: three `contains`-chained, `??`-bearing operands. A
    /// rightmost split recurses on the LEFT side (left-deep parens); a
    /// leftmost split would recurse on the right (right-deep) -- see that
    /// sibling test's doc for the full reasoning (identical here). Routed
    /// through `rewrite_shape_only` for the same reason as the sibling test
    /// above.
    ///
    /// **The expected value below cannot be validated against the oracle.**
    /// Confirmed directly: real Pebble rejects this exact input --
    /// `Contains operator can only be used on Collections, Maps and arrays.
    /// Actual type was: java.lang.Boolean` -- because the INNER `contains`
    /// (once evaluated) yields a boolean, and a boolean can never be the
    /// left operand of Pebble's own `contains`. This test legitimately
    /// proves the split direction this rewrite implements (a purely
    /// syntactic property, checkable without evaluating anything), but its
    /// specific three-`contains` shape is not itself valid Kestra input
    /// Pebble could ever render, so there is no golden value to pin it
    /// against and no corpus entry for it.
    #[test]
    fn chained_contains_splits_rightmost_for_left_associativity() {
        assert_eq!(
            rewrite_shape_only("{{ w contains x ?? y contains z }}"),
            "{{(( w ) contains (kestra_null_coalesce( x ,  y ))) contains ( z )}}"
        );
    }

    /// `??`/`???`/`isIn` must not swallow across a LOWER-precedence `and`/
    /// `or` -- Pebble's `CoreExtension` gives `or`=10, `and`=15, both below
    /// the 30 `??`/`???`/`isIn` share with the comparison operators. Not a
    /// regression from any round of this module; unchanged since Task 2's
    /// first submission, and the same failure mode as I3 (valid minijinja,
    /// silently different semantics) rather than a parse error.
    #[test]
    fn coalesce_does_not_swallow_across_and_or_or() {
        assert_eq!(
            rewrite_and_parse("{{ a ?? b and c ?? d }}"),
            "{{(kestra_null_coalesce( a ,  b )) and (kestra_null_coalesce( c ,  d ))}}"
        );
        assert_eq!(
            rewrite_and_parse("{{ a or b ?? c }}"),
            "{{( a ) or (kestra_null_coalesce( b ,  c ))}}"
        );
    }

    /// The sibling test above only ever has ONE `or` (and one `and`) per
    /// assertion, so it cannot tell a rightmost split from a leftmost one --
    /// traced by hand for both directions before writing this: with no
    /// parens at all, `A or B or C` comes out byte-identical regardless of
    /// which `or` is chosen as the split point (each direction's recursion
    /// just reconstructs the same flat text). The parens `or`/`and` now
    /// wrap their operands in specifically make the split point observable:
    /// a rightmost split recurses on the LEFT side, producing a left-deep
    /// `((A or B) or C)` shape; a leftmost split would instead recurse on
    /// the right, producing right-deep `(A or (B or C))`. Chained three
    /// deep, with a `??` in every operand so each one's own position in the
    /// tree is independently visible in the output.
    #[test]
    fn chained_or_splits_rightmost_for_left_associativity() {
        assert_eq!(
            rewrite_and_parse("{{ a ?? w or b ?? x or c ?? y }}"),
            "{{((kestra_null_coalesce( a ,  w )) or (kestra_null_coalesce( b ,  x ))) or (kestra_null_coalesce( c ,  y ))}}"
        );
    }

    /// `??` and `==` share the SAME precedence tier (30) with LEFT
    /// associativity, so `a ?? b == c` chains left-to-right: `??` (leftmost)
    /// groups first, `==` (rightmost) wraps it -- `(a ?? b) == c`, not
    /// `a ?? (b == c)`. `find_rightmost_tier30` must recognise `==` as a
    /// fellow-tier boundary (never rewriting it) for this to come out right;
    /// without that, a coalesce search that only knows about its own three
    /// operators swallows straight across it.
    #[test]
    fn coalesce_and_a_same_tier_comparison_chain_left_to_right() {
        assert_eq!(
            rewrite_and_parse("{{ a ?? b == c }}"),
            "{{kestra_null_coalesce( a ,  b ) ==  c }}"
        );
    }

    /// `==` (above) and `equals` (`coalesce_does_not_swallow_across_equals_
    /// or_contains`) were the only two of `is_tier30_trigger`'s six
    /// comparison-operator entries with any coverage -- deleting any of the
    /// other five (`!=`/`>`/`>=`/`<`/`<=`) individually left all 31 tests
    /// green (round 3 fixed the NAMED mutant, `equals`, rather than the
    /// class this belongs to). All five parse natively in minijinja (unlike
    /// `equals`/`contains`), so this routes through `rewrite_and_parse` like
    /// the `==` case, not `rewrite_shape_only`.
    #[test]
    fn coalesce_does_not_swallow_across_any_tier30_comparison() {
        for op in ["!=", ">", ">=", "<", "<="] {
            let expr = format!("{{{{ a ?? b {op} c }}}}");
            let expected = format!("{{{{kestra_null_coalesce( a ,  b ) {op}  c }}}}");
            assert_eq!(rewrite_and_parse(&expr), expected, "operator {op:?}");
        }
    }

    /// A plain comparison with no `??`/`???`/`isIn` anywhere must stay
    /// untouched, exactly as it did before tier-30 recognition existed --
    /// `find_rightmost_tier30` is gated on `has_rewritable_op` specifically
    /// so recognising `==`/`>`/etc. as boundaries never turns into
    /// reformatting an expression this rewrite has nothing to rewrite in.
    #[test]
    fn a_plain_comparison_with_no_coalesce_stays_untouched() {
        assert_eq!(rewrite_and_parse("{{ 'b' > 'a' }}"), "{{ 'b' > 'a' }}");
        assert_eq!(rewrite_and_parse("{{ a == b }}"), "{{ a == b }}");
    }

    /// I4's `}}` disambiguation (`find_region_close`'s `brace_depth`),
    /// exercised specifically flush against the region's own close with no
    /// separating space -- `{{ {'a': x ?? y}}}` has three `}` in a row: the
    /// map's own close, then the region's `}}`. Without brace-depth
    /// tracking the naive first `"}}"` match lands one character too early
    /// (consuming the map's close as half of it) -- confirmed by deleting
    /// `brace_depth == 0 &&` from `find_region_close` locally and
    /// re-running just this test: the mutant does NOT silently stop
    /// rewriting as originally guessed here before actually running it --
    /// it mis-identifies the region boundary one character short, so the
    /// truncated "inner" content (`{'a': x ?? y}}` missing its last `}`)
    /// gets tokenized, rewritten, and re-assembled into a string minijinja
    /// then rejects outright (`rewrite_and_parse` panics on the parse
    /// error). Either way the mutant is killed; the actual failure mode is
    /// louder than guessed, not quieter.
    #[test]
    fn a_map_literal_flush_against_the_regions_close_still_rewrites() {
        assert_eq!(
            rewrite_and_parse("{{ {'a': x ?? y}}}"),
            "{{ {'a':kestra_null_coalesce( x ,  y)}}}"
        );
    }

    /// Unbounded recursion is not a catchable Rust error -- it aborts the
    /// whole process (`fatal runtime error: stack overflow`), and `rewrite`
    /// is `pub`, re-exported at the crate root, over user-supplied workflow
    /// source. `MAX_NESTING_DEPTH`'s doc comment has the full measurement
    /// (crashes measured between 1,000 and 5,000 levels on a 2 MiB stack);
    /// 600 is comfortably past the 500 limit while staying far short of
    /// that range, so this test cannot itself crash the test binary even if
    /// the limit regressed to "checked, but too late" rather than "removed
    /// entirely" (a real distinction -- see `expand`'s doc comment on why
    /// the check has to happen AT the recursion site, not only inside
    /// `resolve_pieces`, which found this exact bug in an earlier draft of
    /// this fix: a check placed only in `resolve_pieces` still let `expand`
    /// recurse all the way to the bottom of `{{ ((((...))))}}` before ever
    /// firing, which does not protect a small stack at all, it just delays
    /// the same crash by however many frames the recursion needed to reach
    /// bottom). Confirmed by temporarily raising `MAX_NESTING_DEPTH` to
    /// 100,000 and re-running this exact 600-level input: it succeeds
    /// (`Ok`), which is exactly what would make this test's `unwrap_err()`
    /// fail cleanly rather than crash -- restored afterwards.
    #[test]
    fn nesting_past_the_depth_limit_errors_instead_of_overflowing_the_stack() {
        let n = 600;
        let expr = format!("{{{{ {}a ?? b{}}}}}", "(".repeat(n), ")".repeat(n));
        let err = rewrite(&expr).unwrap_err();
        assert!(
            matches!(err, RewriteError::TooDeeplyNested { .. }),
            "{err:?}"
        );
    }

    /// Critical 3's fix (`split_whitespace_control`) had zero executable
    /// coverage before this test -- only prose and two corpus entries the
    /// oracle test does not yet run (`tests/oracle.rs` stays `#[ignore]`d
    /// until Task 3). Proven load-bearing before writing the expected
    /// value below, against all THREE mutations named in review: (1)
    /// replacing the whole body with `return ("", content, "");` produces
    /// `"{{kestra_null_coalesce(- a ,  b -)}}"` (both markers fused into
    /// the coalesce's operands, the exact Critical-3 shape); (2) dropping
    /// only the leading-marker branch (`let leading = "";`) produces
    /// `"{{kestra_null_coalesce(- a ,  b )-}}"`; (3) dropping only the
    /// trailing-marker branch (`let trailing = "";`) produces output that
    /// does not even PARSE (`"{{-kestra_null_coalesce( a ,  b -)}}"`,
    /// `unexpected ')'`). None of the three is the string asserted below.
    /// All three restored before finalizing.
    #[test]
    fn whitespace_control_markers_survive_around_a_rewritten_expression() {
        assert_eq!(
            rewrite_and_parse("{{- a ?? b -}}"),
            "{{-kestra_null_coalesce( a ,  b )-}}"
        );
    }

    /// Same proof, for a `{%- -%}` tag, against the same three mutations:
    /// (1) whole-body revert produces
    /// `"{%- kestra_null_coalesce( if a ,  b -)%}y{%- endif -%}"` (leading
    /// `-` read as the tag keyword, reproducing Critical 1's exact shape,
    /// trailing `-%}` marker lost entirely and does not even parse); (2)
    /// leading-only drop also fails to parse
    /// (`"{%- kestra_null_coalesce( if a ,  b )-%}y{%- endif-%}"`,
    /// `unknown statement kestra_null_coalesce`); (3) trailing-only drop
    /// also fails to parse
    /// (`"{%- if kestra_null_coalesce( a ,  b -)%}y{%- endif -%}"`,
    /// `unexpected ')'`).
    #[test]
    fn whitespace_control_markers_survive_around_a_rewritten_tag() {
        assert_eq!(
            rewrite_and_parse("{%- if a ?? b -%}y{%- endif -%}"),
            "{%- if kestra_null_coalesce( a ,  b )-%}y{%- endif -%}"
        );
    }

    /// **`minijinja`'s parser overflows the render thread, and sizing the
    /// thread is not what stops it.**
    ///
    /// `MAX_NESTING_DEPTH` counts this rewriter's own recursion over brackets
    /// and operator chains. It never sees a unary-minus run, a `not` run or
    /// an attribute chain, and each of those recurses in the template
    /// parser at **one byte a level**. Measured on the shipped path, on the
    /// 128 MiB render thread:
    ///
    /// | construct | result |
    /// |---|---|
    /// | `{{ ---…---0 }}` | `SIGABRT` at 170 000, deterministic |
    /// | `{{ not not …true }}` | `SIGABRT` at 170 000 |
    /// | `{{ v.a.a…a }}` | `SIGABRT` at 100 000 |
    /// | `{{ v \| lower \| … }}` | caught by minijinja's own parse fuel |
    /// | `{{ ((((…)))) }}` | caught by `MAX_NESTING_DEPTH` |
    ///
    /// The filter chain is the case the engine already defends and the one an
    /// earlier round cited when deferring this; the two that abort cost
    /// almost no fuel per level and slip past it.
    #[test]
    fn a_region_long_enough_to_overflow_the_template_parser_is_refused() {
        // Derived from the render stack, not chosen.
        assert_eq!(WORST_STACK_PER_EXPRESSION_BYTE, 832);
        assert_eq!(
            MAX_EXPRESSION_BYTES,
            super::super::env::RENDER_STACK_BYTES
                / WORST_STACK_PER_EXPRESSION_BYTE
                / EXPRESSION_STACK_MARGIN
        );
        assert_eq!(MAX_EXPRESSION_BYTES, 40_329);

        // At the cap it is rewritten; one byte past it is refused. Either
        // side alone admits an off-by-one.
        let at = format!("{{{{ {} }}}}", "-".repeat(MAX_EXPRESSION_BYTES - 2));
        assert!(rewrite(&at).is_ok());
        let past = format!("{{{{ {} }}}}", "-".repeat(MAX_EXPRESSION_BYTES - 1));
        assert!(matches!(
            rewrite(&past),
            Err(RewriteError::ExpressionTooLong { .. })
        ));

        // Every construct that aborted, at the size that aborted.
        for region in [
            format!("{{{{ {}0 }}}}", "-".repeat(200_000)),
            format!("{{{{ {}true }}}}", "not ".repeat(200_000)),
            format!("{{{{ v{} }}}}", ".a".repeat(200_000)),
            // A tag body takes the same cap: `{% if %}` reaches the same
            // parser.
            format!("{{% if {}0 %}}x{{% endif %}}", "-".repeat(200_000)),
        ] {
            assert!(
                matches!(
                    rewrite(&region),
                    Err(RewriteError::ExpressionTooLong { .. })
                ),
                "{}… was not refused",
                &region[..20]
            );
        }

        // **A string literal is one token, and counting its interior would
        // refuse real flows.** A date test passes a 100 000-character pattern
        // this way, and a long literal costs the parser no depth at all.
        let literal = format!("{{{{ '{}' | upper }}}}", "a".repeat(200_000));
        assert!(rewrite(&literal).is_ok());
        // Including a literal made ENTIRELY of the character that aborts.
        let minus_literal = format!("{{{{ '{}' | upper }}}}", "-".repeat(200_000));
        assert!(rewrite(&minus_literal).is_ok());

        // **And the shape a length cap on the TEMPLATE would have broken:**
        // a 50 KB body with many small regions in it. The text outside
        // `{{ }}` is never counted.
        let body = format!(
            "{}{}",
            "SELECT * FROM t WHERE x = 1 AND y = 2; -- padding\n".repeat(1_000),
            "{{ v }}".repeat(50)
        );
        assert!(body.len() > 40_000);
        assert!(rewrite(&body).is_ok());
    }

    /// **A template that is nothing but `{{ }}` regions used to be quadratic
    /// in the number of them**, because the loop asked for the next `{%`
    /// separately and a template with none read to the end every time round.
    ///
    /// | regions | before | after |
    /// |---:|---:|---:|
    /// | 10 000 | 869 ms | 23 ms |
    /// | 40 000 | 13 480 ms | 94 ms |
    /// | 200 000 | **331 572 ms** | 449 ms |
    ///
    /// Five and a half minutes for a 1.4 MB template, all of it inside
    /// `rewrite` rather than the engine — it was found by attacking the round
    /// 5 fix rather than by reading the diff, on the same axis as the two
    /// bounds that round added.
    ///
    /// 20 seconds is a 44x margin over the linear cost and a 16x margin under
    /// the quadratic one, which catches a regression without flaking under
    /// the parallel load that the suite runs at.
    #[test]
    fn many_small_regions_are_linear_in_the_templates_length() {
        let source = "{{ v }}".repeat(200_000);
        let start = std::time::Instant::now();
        let rewritten = rewrite(&source).expect("rewrites");
        assert_eq!(rewritten, source, "a plain variable is copied through");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(20),
            "200 000 regions took {:?}; the second scan is back",
            start.elapsed()
        );
    }

    /// The four spellings of Pebble's `null` literal become minijinja's
    /// `none`, and the boundary that stops the rename over-reaching.
    ///
    /// Every case here goes through `rewrite_and_parse`, so `o.none` -- which
    /// is what an unguarded rename would emit for `{{ o.null }}` -- fails as a
    /// parse error rather than as a wrong string, which is the shape a reader
    /// of this test should expect from breaking the dot guard.
    #[test]
    fn the_null_literal_becomes_minijinjas_none() {
        for spelling in ["null", "NULL", "none", "NONE"] {
            assert_eq!(
                rewrite_and_parse(&format!("{{{{ {spelling} }}}}")),
                "{{ none }}",
                "{spelling} is one of Pebble's four spellings"
            );
        }

        // Operand positions, not just a bare region.
        assert_eq!(rewrite_and_parse("{{ 'a' ~ null }}"), "{{ 'a' ~ none }}");
        assert_eq!(
            rewrite_and_parse("{{ [null, NULL] }}"),
            "{{ [none, none] }}"
        );
        assert_eq!(
            rewrite_and_parse("{{ fromJson(json=null) }}"),
            "{{ fromJson(json=none) }}"
        );
        // A `{% %}` tag body goes through the same tokenizer. (The missing
        // space before `%}` is `rewrite_tag_body`'s existing normalisation --
        // it trims each side of the `=` junction -- not this rename's doing.)
        assert_eq!(rewrite_and_parse("{% set x = null %}"), "{% set x = none%}");
    }

    /// The rename is guarded three ways, and each guard has a corpus entry.
    ///
    /// **Attribute position, across whitespace.** `{{ o . null }}` renders `1`
    /// on the pinned artifact -- Pebble's lexer allows whitespace around the
    /// dot -- so the look-back has to skip whitespace, not just glance at the
    /// previous token. `null-is-an-attribute-name-across-whitespace`.
    ///
    /// **Inside a string literal.** `tokenize` emits a literal as one
    /// `Tok::Str`, so `{{ 'null' }}` never reaches the rename at all.
    /// `null-inside-a-single-quoted-string`.
    ///
    /// **A word that merely contains one of the spellings.** The rename is
    /// applied to a whole identifier, the same rule `isIn` needs so it does
    /// not fire inside `isInvalid`. `null-is-a-prefix-of-a-variable-name`.
    #[test]
    fn the_null_rename_does_not_reach_an_attribute_a_string_or_a_longer_name() {
        for expr in [
            "{{ o.null }}",
            "{{ o.NULL }}",
            "{{ o.NONE }}",
            "{{ o . null }}",
            "{{ o .  null }}",
            "{{ a.b.null }}",
            "{{ 'null' }}",
            "{{ \"NULL\" }}",
            "{{ 'a null b' }}",
            "{{ nullish }}",
            "{{ mynull }}",
            "{{ null_ }}",
            "{{ NONEISH }}",
            "{{ {'null': 1} }}",
        ] {
            assert_eq!(
                rewrite_and_parse(expr),
                expr,
                "{expr} must be copied through untouched"
            );
        }

        // Mixed case is a VARIABLE on Kestra, measured one probe per spelling
        // against the artifact, so the rename must not be case-insensitive.
        for expr in ["{{ None }}", "{{ Null }}", "{{ nUll }}", "{{ NoNe }}"] {
            assert_eq!(rewrite_and_parse(expr), expr, "{expr} is a variable name");
        }

        // The guard is positional, not sticky: a dot earlier in the expression
        // must not protect a later literal.
        assert_eq!(
            rewrite_and_parse("{{ o.null ~ null }}"),
            "{{ o.null ~ none }}"
        );
    }
}
