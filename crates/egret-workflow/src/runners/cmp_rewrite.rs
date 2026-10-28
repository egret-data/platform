//! The rewriter's second pass (W9c Task 4 fix round 1, the user's option A,
//! 2026-10-08): every comparison in the minijinja source becomes a call to
//! `super::compare`'s functions. The same parser also runs a third pass
//! (package #5a, Task A3, [`compile_arithmetic`]) over the second pass's
//! output, which turns every binary `+`, `-`, `*`, `/` and `%` into a call to
//! `super::arith`'s functions; everything below holds for both passes.
//!
//! # Why a second pass, and why it parses minijinja rather than Pebble
//!
//! [`super::rewrite`] reads KESTRA syntax and writes minijinja source; it
//! leaves `==`, `!=`, `<`, `<=`, `>`, `>=` and `in` where they are (see its
//! "Comparison operators" section). What minijinja then makes of that source
//! is, by definition, what egret's comparisons have meant until now. So this
//! pass reads the rewriter's OUTPUT with **minijinja's own grammar** --
//! `compiler/parser.rs` of the pinned 2.24.0, mirrored function for function
//! below -- and changes nothing but the comparisons:
//!
//! | minijinja source | becomes |
//! |---|---|
//! | `a > b` | `kestra_gt(a, b)` |
//! | `a not in b` | `kestra_not_in(a, b)` |
//! | `a < b <= c` (minijinja chains it) | `kestra_le(kestra_lt(a, b), c)` |
//!
//! **A chain is compiled left-associatively** (package #5a, spec D1). Pebble
//! has no chained comparison: Kestra's `==`, `!=`, `<`, `<=`, `>`, `>=` and
//! `isIn` all sit at precedence 30, left-associative, so `a < b < c` is
//! `(a < b) < c` there, and the outer comparison sees a boolean and fails;
//! `1 < 2 == true` is `true`. minijinja reads the same source as `a < b and
//! b < c`; this pass writes Pebble's reading instead.
//!
//! Each operand is exactly the token range minijinja's parser gives that
//! comparison, so precedence is minijinja's by construction. minijinja's
//! comparison tier matches Pebble's against the binary operators (one level,
//! below `+`/`-`/`~`): `a ~ b == c` stays `(a ~ b) == c`, a filter or a test
//! binds tighter, a parenthesised group is one operand. It does NOT match
//! Pebble's `not`: Pebble registers `not` at 500 (`javap -c CoreExtension`),
//! tighter than every binary operator, so `not a == b` is `(not a) == b` in
//! Kestra, while minijinja (and so this pass) reads `not (a == b)`; the same
//! holds for `not a contains b`, `not a ?? b` and `not a + 1` (spec §6.1,
//! recorded divergence 5; fix wave #5a M1). A call argument is a full expression, so no operand can bind
//! differently inside the call than it did beside the operator.
//!
//! # What is preserved besides the value
//!
//! - **Every byte outside a comparison**, including all whitespace and every
//!   newline: the call name goes right after the token that precedes the
//!   comparison, and each operator is replaced by a `,` in place. minijinja
//!   attributes an operator's error and a call's alike to the line of the
//!   operand it compiled last, so keeping every newline where it was keeps
//!   the line an error names (measured both ways by
//!   `compare::tests::an_undefined_operand_raises_the_same_error_on_the_same_line`).
//! - **What minijinja would refuse is left for minijinja to refuse.** A region
//!   this pass cannot parse the way minijinja would (a syntax error, an
//!   unbalanced bracket, a construct outside the mirrored grammar) is copied
//!   through untouched, so it fails -- or answers -- exactly as it did. A
//!   template this pass cannot even LEX is returned whole and unchanged.
//! - `{% do %}` is left untouched on purpose: minijinja refuses a `do` whose
//!   expression is not a call, and turning `{% do a > b %}` into a call
//!   would turn that refusal into an answer. `do` is not Pebble syntax
//!   (Kestra refuses the tag), and the value of the one form minijinja takes,
//!   `{% do f(..) %}`, is discarded, so an operator in its arguments can
//!   change only which error a failing argument raises. `{% macro %}` and
//!   `{% call %}` are not reachable at all: the `macros` feature is off, and
//!   minijinja refuses them as unknown statements.
//! - **A `{% raw %}` block is copied through whole**, found the way
//!   minijinja's lexer finds it (`skip_basic_tag`), whatever its whitespace
//!   control. egret's own raw protection (`env::replace_raw_tags`, Kestra's
//!   `RAW_PATTERN`) does not know the `+` form, so `{%+ raw %}` reaches this
//!   pass; skipping just that block keeps every comparison around it
//!   compiled (W9c Task 4 cleanup, N1).
//!
//! # The one refusal of its own: a `kestra_` assignment target
//!
//! The calls this pass and [`super::rewrite`] write are named `kestra_…`
//! and are looked up like any other name, so a template variable of that
//! name would stand in for the function: `{% set kestra_gt = 1 %}{{ a > b }}`
//! would call `1`. The prefix is therefore reserved: a `set`, `for` or
//! `with` that assigns a name starting with `kestra_` is refused at compile
//! time with [`RewriteError::ReservedName`] (W9c Task 4 cleanup, N2). An
//! attribute (`x.kestra_gt`) is not a variable and is not refused.
//!
//! The functions answer `==`, `!=`, `in` and `not in` with minijinja's own
//! `PartialEq` for every pair that is not a `BigNumber` against a number,
//! after the same undefined checks in the same order (`super::compare`'s doc),
//! so for those the pass changes the spelling and not the answer. The four
//! relational operators answer as Kestra does (`super::compare::relational`).

use super::arith::ArithOp;
use super::compare::CmpOp;
use super::rewrite::RewriteError;

/// The deepest bracket nesting this pass follows. minijinja's parser refuses
/// past 150 levels of `parse_expr`/`parse_primary` (`MAX_RECURSION`), and
/// every bracket level is at least one of those, so anything deeper is a
/// template minijinja refuses anyway -- it is copied through untouched for
/// minijinja to refuse, and this pass's own recursion stays bounded.
const MAX_GROUP_DEPTH: usize = 160;

/// The prefix of every function name egret's rewriters write, which a
/// template may therefore not assign (see the module doc).
pub(crate) const RESERVED_PREFIX: &str = "kestra_";

/// Compiles every comparison in `source` (the rewriter's output) to a call.
///
/// Its one refusal is a `kestra_` assignment target
/// ([`RewriteError::ReservedName`]). Anything else this pass does not
/// understand is left as it was.
pub(crate) fn compile_comparisons(source: &str) -> Result<String, RewriteError> {
    compile_pass(source, Pass::Comparisons)
}

/// The third pass (package #5a, Task A3, spec D2): every binary `+`, `-`,
/// `*`, `/` and `%` in `source` (the comparison pass's output) becomes a call
/// to one of `super::arith`'s `kestra_add` .. `kestra_mod`.
///
/// It reads the source with the same mirror of minijinja's grammar as the
/// comparison pass and records the operators minijinja's `parse_math1`
/// (`+`, `-`) and `parse_math2` (`*`, `/`, `%`) would build, so every operand
/// is exactly the token range minijinja gives it and precedence is
/// minijinja's by construction: `a + b * c` is `kestra_add(a, kestra_mul(b,
/// c))`, `a - b + c` is `kestra_add(kestra_sub(a, b), c)` (left-associative,
/// as Pebble's `+`/`-` and `*`/`/`/`%` are), `-a / b` divides the negated
/// `a`, and `a ~ b + c` is `(a ~ b) + c` in both engines (`~` is 110 in
/// Pebble, above `+`). A unary minus on anything but a number literal
/// becomes `kestra_mul(-1, x)`, Pebble's `unaryMinus` (Task A8); `~`, `//`
/// and `**` are not touched.
/// Everything the module doc says about preserved bytes, unparsed regions,
/// `{% do %}` (its discarded call arguments keep minijinja's operators),
/// raw blocks and the reserved prefix holds for this pass too. `{% with %}`
/// and `{% filter %}` are mirrored, so they get Pebble's arithmetic.
pub(crate) fn compile_arithmetic(source: &str) -> Result<String, RewriteError> {
    compile_pass(source, Pass::Arithmetic)
}

/// Which operators a pass compiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    Comparisons,
    Arithmetic,
}

fn compile_pass(source: &str, pass: Pass) -> Result<String, RewriteError> {
    match compile(source, pass) {
        Some(Ok(out)) => Ok(out),
        Some(Err(name)) => Err(RewriteError::ReservedName { name }),
        None => Ok(source.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ident,
    Str,
    /// `int_like`: an `Int`/`Int128` token, which is what may follow a `.`.
    Num {
        int_like: bool,
    },
    Op(&'static str),
    Open(u8),
    Close(u8),
}

#[derive(Debug, Clone, Copy)]
struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
    /// For an `Open`, the index of its `Close`.
    pair: usize,
}

/// The whole template: copy the data, rewrite each `{{ }}` and `{% %}`.
/// `None` when the lexer cannot follow minijinja's; `Some(Err(name))` for a
/// reserved assignment target in a region that parsed.
fn compile(src: &str, pass: Pass) -> Option<Result<String, String>> {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len() + src.len() / 4);
    let mut i = 0usize;
    loop {
        // `find_start_marker_memchr`: the next `{` followed by `{`, `%` or `#`.
        let Some(found) = (i..bytes.len().saturating_sub(1))
            .find(|&k| bytes[k] == b'{' && matches!(bytes[k + 1], b'{' | b'%' | b'#'))
        else {
            out.push_str(&src[i..]);
            return Some(Ok(out));
        };
        out.push_str(&src[i..found]);
        let marker = bytes[found + 1];
        // `Whitespace::from_byte`: a `-` or `+` right after the marker is part
        // of it.
        let mut content_start = found + 2;
        if matches!(bytes.get(content_start), Some(b'-' | b'+')) {
            content_start += 1;
        }
        if marker == b'#' {
            // `handle_start_marker(Comment)`: up to the first `#}`.
            let end = src[content_start..].find("#}")? + content_start + 2;
            out.push_str(&src[found..end]);
            i = end;
            continue;
        }
        if marker == b'%' {
            // `handle_start_marker(Block)`: a raw tag starts a block that
            // minijinja reads as data up to its `endraw`, so it is copied
            // through whole and the pass goes on after it.
            if let Some(open) = skip_basic_tag(&src[content_start..], "raw", false) {
                // No `endraw`: minijinja refuses ("unexpected end of raw
                // block"), so the template is left for it to refuse.
                let end = raw_block_end(src, content_start + open)?;
                out.push_str(&src[found..end]);
                i = end;
                continue;
            }
        }
        let close: &str = if marker == b'{' { "}}" } else { "%}" };
        let (toks, content_end, region_end) = lex_region(src, content_start, close)?;
        if marker == b'%'
            && toks
                .first()
                .is_some_and(|t| t.kind == Kind::Ident && &src[t.start..t.end] == "raw")
        {
            // A `raw` that is not a raw tag (`{% raw x %}`): minijinja
            // refuses it, so leave the whole template for it to refuse.
            return None;
        }
        out.push_str(&src[found..content_start]);
        let mut parser = Parser {
            src,
            toks: &toks,
            pos: 0,
            end: toks.len(),
            depth: 0,
            sites: Vec::new(),
            reserved: None,
            pass,
        };
        let parsed = if marker == b'{' {
            parser.whole_expression()
        } else {
            parser.block()
        };
        if let (Ok(()), Some(at)) = (parsed, parser.reserved) {
            return Some(Err(parser.text_at(at).to_string()));
        }
        match parsed {
            Ok(()) => emit(
                src,
                &toks,
                &parser.sites,
                content_start,
                content_end,
                &mut out,
            ),
            Err(()) => out.push_str(&src[content_start..content_end]),
        }
        out.push_str(&src[content_end..region_end]);
        i = region_end;
    }
}

/// Lexes one `{{ }}`/`{% %}` region from `start` (just past the opening
/// marker), as `tokenize_block_or_var` does. Returns the tokens, where the
/// content ends (the closing marker, including a `-`/`+`, starts there) and
/// where the region ends.
fn lex_region(src: &str, start: usize, close: &str) -> Option<(Vec<Tok>, usize, usize)> {
    let bytes = src.as_bytes();
    let mut toks: Vec<Tok> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut balance: i64 = 0;
    let mut p = start;
    loop {
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if p >= bytes.len() {
            return None;
        }
        let rest = &src[p..];
        if balance == 0 {
            if matches!(bytes[p], b'-' | b'+') && rest[1..].starts_with(close) {
                return finish(toks, &stack, p, p + 1 + close.len());
            }
            if rest.starts_with(close) {
                return finish(toks, &stack, p, p + close.len());
            }
        }
        let two = rest.get(..2);
        let op2 = match two {
            Some("//") => Some("//"),
            Some("**") => Some("**"),
            Some("==") => Some("=="),
            Some("!=") => Some("!="),
            Some(">=") => Some(">="),
            Some("<=") => Some("<="),
            _ => None,
        };
        if let Some(op) = op2 {
            toks.push(tok(Kind::Op(op), p, p + 2));
            p += 2;
            continue;
        }
        let c = bytes[p];
        let op1 = match c {
            b'+' => Some("+"),
            b'-' => Some("-"),
            b'*' => Some("*"),
            b'/' => Some("/"),
            b'%' => Some("%"),
            b'.' => Some("."),
            b',' => Some(","),
            b':' => Some(":"),
            b'~' => Some("~"),
            b'|' => Some("|"),
            b'=' => Some("="),
            b'>' => Some(">"),
            b'<' => Some("<"),
            _ => None,
        };
        if let Some(op) = op1 {
            toks.push(tok(Kind::Op(op), p, p + 1));
            p += 1;
            continue;
        }
        match c {
            b'(' | b'[' | b'{' => {
                balance += 1;
                stack.push(toks.len());
                toks.push(tok(Kind::Open(c), p, p + 1));
                p += 1;
            }
            b')' | b']' | b'}' => {
                balance -= 1;
                let open = match c {
                    b')' => b'(',
                    b']' => b'[',
                    _ => b'{',
                };
                let here = toks.len();
                toks.push(tok(Kind::Close(c), p, p + 1));
                // A mismatched or unmatched closer is a syntax error to
                // minijinja, and it also moves minijinja's `paren_balance`
                // off zero, so where the region ends is no longer something
                // this pass can follow: leave the whole template alone.
                match stack.pop() {
                    Some(o) if toks[o].kind == Kind::Open(open) => toks[o].pair = here,
                    _ => return None,
                }
                p += 1;
            }
            b'\'' | b'"' => {
                // `eat_string`: up to the first unescaped delimiter.
                let mut k = p + 1;
                let mut escaped = false;
                while k < bytes.len() {
                    let b = bytes[k];
                    if escaped {
                        escaped = false;
                    } else if b == b'\\' {
                        escaped = true;
                    } else if b == c {
                        break;
                    }
                    k += 1;
                }
                if k >= bytes.len() {
                    return None;
                }
                toks.push(tok(Kind::Str, p, k + 1));
                p = k + 1;
            }
            b'0'..=b'9' => {
                let (len, int_like) = number_len(&src[p..]);
                toks.push(tok(Kind::Num { int_like }, p, p + len));
                p += len;
            }
            _ => {
                let len = ident_len(rest);
                if len == 0 {
                    return None;
                }
                toks.push(tok(Kind::Ident, p, p + len));
                p += len;
            }
        }
    }
}

/// minijinja's `skip_basic_tag` with `%}` as the block end: `name` after
/// optional whitespace (and, with `skip_ws_control`, an optional leading
/// `-`/`+`), then optional whitespace, an optional `-`/`+` and `%}`. The
/// length it spans, or `None`.
fn skip_basic_tag(s: &str, name: &str, skip_ws_control: bool) -> Option<usize> {
    let mut ptr = s;
    if skip_ws_control {
        if let Some(rest) = ptr.strip_prefix(['-', '+']) {
            ptr = rest;
        }
    }
    ptr = ptr.trim_start_matches(|c: char| c.is_ascii_whitespace());
    ptr = ptr.strip_prefix(name)?;
    ptr = ptr.trim_start_matches(|c: char| c.is_ascii_whitespace());
    if let Some(rest) = ptr.strip_prefix(['-', '+']) {
        ptr = rest;
    }
    ptr.strip_prefix("%}").map(|rest| s.len() - rest.len())
}

/// `handle_raw_tag`: from just past a raw tag, the end of the first
/// `{%` … `endraw` … `%}` tag, or `None` if there is none.
fn raw_block_end(src: &str, body: usize) -> Option<usize> {
    let mut ptr = body;
    while let Some(block) = src[ptr..].find("{%") {
        ptr += block + 2;
        if let Some(len) = skip_basic_tag(&src[ptr..], "endraw", true) {
            return Some(ptr + len);
        }
    }
    None
}

fn finish(
    toks: Vec<Tok>,
    stack: &[usize],
    content_end: usize,
    region_end: usize,
) -> Option<(Vec<Tok>, usize, usize)> {
    // An unclosed opener never lets `balance` reach 0, so the closing marker
    // is never seen and the lexer runs off the end (`None` above). Reaching
    // here with one open would need a negative excursion first, which
    // `lex_region` already refused.
    if !stack.is_empty() {
        return None;
    }
    Some((toks, content_end, region_end))
}

fn tok(kind: Kind, start: usize, end: usize) -> Tok {
    Tok {
        kind,
        start,
        end,
        pair: usize::MAX,
    }
}

/// `lex_identifier` without the `unicode` feature: `[A-Za-z_][A-Za-z0-9_]*`.
fn ident_len(s: &str) -> usize {
    s.bytes()
        .enumerate()
        .take_while(|&(idx, c)| {
            c == b'_'
                || if idx == 0 {
                    c.is_ascii_alphabetic()
                } else {
                    c.is_ascii_alphanumeric()
                }
        })
        .count()
}

/// `eat_number`'s length, and whether the token is an integer.
fn number_len(s: &str) -> (usize, bool) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        RadixInteger,
        Integer,
        Fraction,
        Exponent,
        ExponentSign,
    }
    let bytes = s.as_bytes();
    let radix = match bytes.get(..2) {
        Some(b"0b" | b"0B") => 2,
        Some(b"0o" | b"0O") => 8,
        Some(b"0x" | b"0X") => 16,
        _ => 10,
    };
    let (offset, mut state) = if radix == 10 {
        (0, State::Integer)
    } else {
        (2, State::RadixInteger)
    };
    let rest = &bytes[offset..];
    let mut num_len = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    for &c in &rest[num_len..] {
        state = match (c, state) {
            (b'.', State::Integer) => {
                let is_exp = matches!(rest.get(num_len + 1), Some(b'e' | b'E'))
                    && matches!(rest.get(num_len + 2), Some(b'+' | b'-' | b'0'..=b'9'));
                if !is_exp && ident_len(&s[offset + num_len + 1..]) > 0 {
                    break;
                }
                State::Fraction
            }
            (b'E' | b'e', State::Integer | State::Fraction) => State::Exponent,
            (b'+' | b'-', State::Exponent) => State::ExponentSign,
            (b'0'..=b'9', State::Exponent) => State::ExponentSign,
            (b'0'..=b'9', state) => state,
            (b'a'..=b'f' | b'A'..=b'F', State::RadixInteger) if radix == 16 => state,
            (b'_', state) => state,
            _ => break,
        };
        num_len += 1;
    }
    (
        offset + num_len,
        matches!(state, State::Integer | State::RadixInteger),
    )
}

/// One comparison (or one run of same-tier arithmetic operators)
/// minijinja's parser would build, as token indices.
#[derive(Debug)]
struct Site {
    /// The first token of the left operand.
    start: usize,
    /// The last token of the last operand.
    end: usize,
    /// Each operator: its first and last token (`not in` is two), and the
    /// function it becomes.
    ops: Vec<(usize, usize, &'static str)>,
}

struct Parser<'a> {
    src: &'a str,
    toks: &'a [Tok],
    pos: usize,
    end: usize,
    depth: usize,
    sites: Vec<Site>,
    /// The first assignment target named with [`RESERVED_PREFIX`].
    reserved: Option<usize>,
    /// Which operators become calls.
    pass: Pass,
}

type Parsed = Result<(), ()>;

impl Parser<'_> {
    fn kind(&self) -> Option<Kind> {
        (self.pos < self.end).then(|| self.toks[self.pos].kind)
    }

    fn kind_at(&self, at: usize) -> Option<Kind> {
        (at < self.end).then(|| self.toks[at].kind)
    }

    fn text_at(&self, at: usize) -> &str {
        let t = self.toks[at];
        &self.src[t.start..t.end]
    }

    fn is_ident(&self, word: &str) -> bool {
        self.kind() == Some(Kind::Ident) && self.text_at(self.pos) == word
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.kind(), Some(Kind::Op(o)) if o == op)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.end
    }

    /// Records the first variable in tokens `from..to` (an assignment
    /// target) that is named with [`RESERVED_PREFIX`]. An attribute after a
    /// `.` is not a variable.
    fn check_targets(&mut self, from: usize, to: usize) {
        for at in from..to.min(self.end) {
            if self.reserved.is_none()
                && self.toks[at].kind == Kind::Ident
                && self.text_at(at).starts_with(RESERVED_PREFIX)
                && !(at > from && self.toks[at - 1].kind == Kind::Op("."))
            {
                self.reserved = Some(at);
            }
        }
    }

    fn expect_end(&self) -> Parsed {
        if self.at_end() {
            Ok(())
        } else {
            Err(())
        }
    }

    // ----- statements (`parse_stmt_unprotected`) -----

    /// A `{{ }}` region: one expression and nothing after it.
    fn whole_expression(&mut self) -> Parsed {
        self.ifexpr()?;
        self.expect_end()
    }

    /// A `{% %}` region, for the statements the pinned feature set parses.
    fn block(&mut self) -> Parsed {
        if self.kind() != Some(Kind::Ident) {
            return Err(());
        }
        let keyword = self.text_at(self.pos).to_string();
        self.pos += 1;
        match keyword.as_str() {
            // `parse_if_cond`: `parse_expr_noif`, then the end of the block.
            "if" | "elif" => {
                self.or()?;
                self.expect_end()
            }
            // `parse_for_stmt`.
            "for" => {
                let target_end = self.find_top_level(|p, at| {
                    p.kind_at(at) == Some(Kind::Ident) && p.text_at(at) == "in"
                })?;
                self.check_targets(self.pos, target_end);
                self.pos = target_end + 1;
                self.or()?;
                if self.is_ident("if") {
                    self.pos += 1;
                    self.ifexpr()?;
                }
                if self.is_ident("recursive") {
                    self.pos += 1;
                }
                self.expect_end()
            }
            // `parse_set`.
            "set" => match self.find_top_level(|p, at| p.kind_at(at) == Some(Kind::Op("="))) {
                Ok(eq) => {
                    self.check_targets(self.pos, eq);
                    self.pos = eq + 1;
                    self.ifexpr()?;
                    while self.is_op(",") {
                        self.pos += 1;
                        if self.at_end() {
                            break;
                        }
                        self.ifexpr()?;
                    }
                    self.expect_end()
                }
                Err(()) => {
                    // A set block: the target, then an optional filter chain.
                    match self.find_top_level(|p, at| p.kind_at(at) == Some(Kind::Op("|"))) {
                        Ok(pipe) => {
                            self.check_targets(self.pos, pipe);
                            self.pos = pipe + 1;
                            self.filter_chain()
                        }
                        Err(()) => {
                            self.check_targets(self.pos, self.end);
                            Ok(())
                        }
                    }
                }
            },
            // `parse_with_block`.
            "with" => {
                let mut first = true;
                while !self.at_end() {
                    if !first {
                        if !self.is_op(",") {
                            return Err(());
                        }
                        self.pos += 1;
                    }
                    first = false;
                    let target = self.pos;
                    match self.kind() {
                        Some(Kind::Open(b'(')) => self.pos = self.toks[self.pos].pair + 1,
                        Some(Kind::Ident) => self.pos += 1,
                        _ => return Err(()),
                    }
                    self.check_targets(target, self.pos);
                    if !self.is_op("=") {
                        return Err(());
                    }
                    self.pos += 1;
                    self.ifexpr()?;
                }
                Ok(())
            }
            "autoescape" => self.whole_expression(),
            "filter" => self.filter_chain(),
            "else" | "endif" | "endfor" | "endwith" | "endset" | "endautoescape" | "endfilter" => {
                self.expect_end()
            }
            // `do` (see the module doc) and everything minijinja refuses.
            _ => Err(()),
        }
    }

    /// The first token at this bracket level matching `pred`, from `pos`.
    fn find_top_level(&self, pred: impl Fn(&Self, usize) -> bool) -> Result<usize, ()> {
        let mut at = self.pos;
        while at < self.end {
            if pred(self, at) {
                return Ok(at);
            }
            at = match self.toks[at].kind {
                Kind::Open(_) => self.toks[at].pair + 1,
                _ => at + 1,
            };
        }
        Err(())
    }

    /// `parse_filter_chain`, positioned after a leading `|` if there was one.
    fn filter_chain(&mut self) -> Parsed {
        let mut first = true;
        while !self.at_end() {
            if !first {
                if !self.is_op("|") {
                    return Err(());
                }
                self.pos += 1;
            }
            first = false;
            self.filter_test_name()?;
            if self.kind() == Some(Kind::Open(b'(')) {
                self.group()?;
            }
        }
        Ok(())
    }

    // ----- expressions (`parse_ifexpr` down to `parse_primary`) -----

    /// `parse_ifexpr`, iteratively: `a if b else c if d else e` recursed
    /// on its `else` branch, and that recursion consumed every following
    /// `if` -- so restarting the loop is the same parse.
    fn ifexpr(&mut self) -> Parsed {
        'expr: loop {
            self.or()?;
            while self.is_ident("if") {
                self.pos += 1;
                self.or()?;
                if self.is_ident("else") {
                    self.pos += 1;
                    continue 'expr;
                }
            }
            return Ok(());
        }
    }

    fn or(&mut self) -> Parsed {
        self.and()?;
        while self.is_ident("or") {
            self.pos += 1;
            self.and()?;
        }
        Ok(())
    }

    fn and(&mut self) -> Parsed {
        self.not()?;
        while self.is_ident("and") {
            self.pos += 1;
            self.not()?;
        }
        Ok(())
    }

    /// `unaryop!(parse_not, parse_compare, not)`, iteratively.
    fn not(&mut self) -> Parsed {
        while self.is_ident("not") {
            self.pos += 1;
        }
        self.compare()
    }

    /// `parse_compare`: a `math1`, then any number of operator + `math1`.
    fn compare(&mut self) -> Parsed {
        let start = self.pos;
        self.math1()?;
        let mut ops = Vec::new();
        loop {
            let op_start = self.pos;
            let op = match self.kind() {
                Some(Kind::Op("==")) => CmpOp::Eq,
                Some(Kind::Op("!=")) => CmpOp::Ne,
                Some(Kind::Op("<")) => CmpOp::Lt,
                Some(Kind::Op("<=")) => CmpOp::Le,
                Some(Kind::Op(">")) => CmpOp::Gt,
                Some(Kind::Op(">=")) => CmpOp::Ge,
                Some(Kind::Ident) if self.text_at(self.pos) == "in" => CmpOp::In,
                Some(Kind::Ident) if self.text_at(self.pos) == "not" => {
                    self.pos += 1;
                    if !self.is_ident("in") {
                        return Err(());
                    }
                    CmpOp::NotIn
                }
                _ => break,
            };
            let op_end = self.pos;
            self.pos += 1;
            ops.push((op_start, op_end, op.function()));
            self.math1()?;
        }
        if !ops.is_empty() && self.pass == Pass::Comparisons {
            self.sites.push(Site {
                start,
                end: self.pos - 1,
                ops,
            });
        }
        Ok(())
    }

    fn math1(&mut self) -> Parsed {
        let start = self.pos;
        self.concat()?;
        let mut ops = Vec::new();
        loop {
            let op = match self.kind() {
                Some(Kind::Op("+")) => ArithOp::Add,
                Some(Kind::Op("-")) => ArithOp::Sub,
                _ => break,
            };
            ops.push((self.pos, self.pos, op.function()));
            self.pos += 1;
            self.concat()?;
        }
        self.arithmetic_site(start, ops);
        Ok(())
    }

    /// Records a run of arithmetic operators of one tier, in the arithmetic
    /// pass only.
    fn arithmetic_site(&mut self, start: usize, ops: Vec<(usize, usize, &'static str)>) {
        if !ops.is_empty() && self.pass == Pass::Arithmetic {
            self.sites.push(Site {
                start,
                end: self.pos - 1,
                ops,
            });
        }
    }

    fn concat(&mut self) -> Parsed {
        self.math2()?;
        while self.is_op("~") {
            self.pos += 1;
            self.math2()?;
        }
        Ok(())
    }

    /// `parse_math2`. A `//` (minijinja's floor division, no Pebble
    /// operator) between two arithmetic operators ends the run:
    /// `a * b // c * d` is `((a * b) // c) * d`, so the run before the `//`
    /// becomes one call, that call is the left operand of the untouched
    /// `//`, and the whole of that is the left operand of the run after it.
    fn math2(&mut self) -> Parsed {
        let start = self.pos;
        self.pow()?;
        let mut ops = Vec::new();
        loop {
            let op = match self.kind() {
                Some(Kind::Op("*")) => ArithOp::Mul,
                Some(Kind::Op("/")) => ArithOp::Div,
                Some(Kind::Op("%")) => ArithOp::Mod,
                Some(Kind::Op("//")) => {
                    // Close the run so far (it ends on the token before
                    // the `//`); the `//` and everything left of it are the
                    // next run's left operand, which starts where this one
                    // did.
                    let done = std::mem::take(&mut ops);
                    self.arithmetic_site(start, done);
                    self.pos += 1;
                    self.pow()?;
                    continue;
                }
                _ => break,
            };
            ops.push((self.pos, self.pos, op.function()));
            self.pos += 1;
            self.pow()?;
        }
        self.arithmetic_site(start, ops);
        Ok(())
    }

    fn pow(&mut self) -> Parsed {
        self.unary()?;
        while self.is_op("**") {
            self.pos += 1;
            self.unary()?;
        }
        Ok(())
    }

    /// `parse_unary`: unary minus, postfix, then filters and tests.
    fn unary(&mut self) -> Parsed {
        let first = self.pos;
        self.unary_only()?;
        self.postfix()?;
        self.unary_minus_sites(first);
        self.filter_expr()
    }

    /// `unaryop!(parse_unary_only, parse_primary, -)`, iteratively.
    ///
    /// In the arithmetic pass a unary minus becomes `kestra_mul(-1, x)`
    /// ([`Self::unary_minus_sites`], recorded once the postfix steps are
    /// read).
    fn unary_only(&mut self) -> Parsed {
        while self.is_op("-") {
            self.pos += 1;
        }
        self.primary()
    }

    /// Records `kestra_mul(-1, x)` for each leading `-` of the operand that
    /// starts at `first` and ends where the parse is now, in the arithmetic
    /// pass. This is Pebble's `OperatorUtils.unaryMinus` (`multiply(-1, x)`,
    /// read with `javap -c`), so `-fromIon(ion='1.50')` is the BigDecimal
    /// `-1.50` and `-'5'` is `-5` (package #5a Task A8, 2026-10-29).
    ///
    /// The operand is Pebble's: the primary **with** its postfix steps, so
    /// `-a.b` is `-(a.b)` and `-f(x)` negates the call's result. minijinja
    /// reads them as `(-a).b` and `(-f)(x)`, which fail on every value
    /// Pebble's reading answers, so no such form that rendered before
    /// changes. One plain `-x` does: an integer past `i64` (a JSON
    /// `10000000000000000000`). minijinja negated it exactly; here the `-1`
    /// is a Long, so it takes the Long branch and wraps its low 64 bits
    /// (`8446744073709551616`); Kestra multiplies by an `Integer` -1, so two
    /// non-Longs take the `int` branch (`1981284352`, measured 2026-10-29).
    /// That is the Integer-vs-Long divergence (spec §6.1 item 1; fix wave
    /// #5a M2). A
    /// number literal with no postfix is left as it is: `-1` and
    /// `kestra_mul(-1, 1)` are the same value in both engines, and the
    /// compiled text stays readable. Kept out of the recursive frames
    /// ([`Self::unary`]), which every bracket level passes through:
    /// `deep_nesting_is_bounded` runs the pass on a 256 KiB stack.
    #[inline(never)]
    fn unary_minus_sites(&mut self, first: usize) {
        if self.pass != Pass::Arithmetic {
            return;
        }
        let mut primary = first;
        while primary < self.pos && matches!(self.toks[primary].kind, Kind::Op("-")) {
            primary += 1;
        }
        let literal =
            self.pos == primary + 1 && matches!(self.toks[primary].kind, Kind::Num { .. });
        if primary == first || literal {
            return;
        }
        // Innermost first, as every other site is recorded.
        for minus in (first..primary).rev() {
            self.sites.push(Site {
                start: minus,
                end: self.pos - 1,
                ops: vec![(minus, minus, ArithOp::Mul.function())],
            });
        }
    }

    fn primary(&mut self) -> Parsed {
        match self.kind() {
            Some(Kind::Ident) | Some(Kind::Num { .. }) => {
                self.pos += 1;
                Ok(())
            }
            // Adjacent string literals are one constant.
            Some(Kind::Str) => {
                while self.kind() == Some(Kind::Str) {
                    self.pos += 1;
                }
                Ok(())
            }
            Some(Kind::Open(_)) => self.group(),
            _ => Err(()),
        }
    }

    fn postfix(&mut self) -> Parsed {
        loop {
            match self.kind() {
                Some(Kind::Op(".")) => {
                    self.pos += 1;
                    match self.kind() {
                        Some(Kind::Ident) | Some(Kind::Num { int_like: true }) => self.pos += 1,
                        _ => return Err(()),
                    }
                }
                Some(Kind::Open(b'[')) | Some(Kind::Open(b'(')) => self.group()?,
                _ => return Ok(()),
            }
        }
    }

    /// `parse_filter_test_name`: `name(.name)*`.
    fn filter_test_name(&mut self) -> Parsed {
        if self.kind() != Some(Kind::Ident) {
            return Err(());
        }
        self.pos += 1;
        while self.is_op(".") {
            self.pos += 1;
            if self.kind() != Some(Kind::Ident) {
                return Err(());
            }
            self.pos += 1;
        }
        Ok(())
    }

    /// `parse_filter_expr`.
    fn filter_expr(&mut self) -> Parsed {
        loop {
            if self.is_op("|") {
                self.pos += 1;
                self.filter_test_name()?;
                if self.kind() == Some(Kind::Open(b'(')) {
                    self.group()?;
                }
            } else if self.is_ident("is") {
                self.pos += 1;
                if self.is_ident("not") {
                    self.pos += 1;
                }
                self.filter_test_name()?;
                match self.kind() {
                    Some(Kind::Open(b'(')) => self.group()?,
                    // The one-argument form: an identifier, a literal, a
                    // sign, a list or a map -- but not `and`/`or`/`else`/`is`.
                    Some(Kind::Ident)
                        if matches!(self.text_at(self.pos), "and" | "or" | "else" | "is") => {}
                    Some(
                        Kind::Ident
                        | Kind::Str
                        | Kind::Num { .. }
                        | Kind::Op("+")
                        | Kind::Op("-")
                        | Kind::Open(b'[')
                        | Kind::Open(b'{'),
                    ) => {
                        let first = self.pos;
                        self.unary_only()?;
                        self.postfix()?;
                        self.unary_minus_sites(first);
                    }
                    _ => {}
                }
            } else {
                return Ok(());
            }
        }
    }

    /// A bracketed group: a call's arguments, a list, a map, a tuple, a
    /// subscript or a slice. All of them are expressions separated by `,`
    /// and `:`, each optionally a keyword argument (`name=`) or a splat
    /// (`*`, `**`), so one reading serves all six.
    fn group(&mut self) -> Parsed {
        let open = self.pos;
        let close = self.toks[open].pair;
        if self.depth >= MAX_GROUP_DEPTH {
            return Err(());
        }
        let (saved_end, saved_depth) = (self.end, self.depth);
        self.depth += 1;
        self.pos = open + 1;
        self.end = close;
        let result = self.group_contents();
        self.end = saved_end;
        self.depth = saved_depth;
        self.pos = close + 1;
        result
    }

    fn group_contents(&mut self) -> Parsed {
        while !self.at_end() {
            if self.is_op("**") || self.is_op("*") {
                self.pos += 1;
            } else if self.kind() == Some(Kind::Ident)
                && self.kind_at(self.pos + 1) == Some(Kind::Op("="))
            {
                self.pos += 2;
            }
            loop {
                if self.at_end() || self.is_op(",") {
                    break;
                }
                if self.is_op(":") {
                    self.pos += 1;
                    continue;
                }
                self.ifexpr()?;
                if !(self.at_end() || self.is_op(",") || self.is_op(":")) {
                    return Err(());
                }
            }
            if self.is_op(",") {
                self.pos += 1;
            }
        }
        Ok(())
    }
}

/// Writes one region's content with its comparisons compiled.
fn emit(
    src: &str,
    toks: &[Tok],
    sites: &[Site],
    content_start: usize,
    content_end: usize,
    out: &mut String,
) {
    let n = toks.len();
    let mut prefix: Vec<Option<String>> = vec![None; n];
    let mut replacement: Vec<Option<String>> = vec![None; n];
    let mut suffixes: Vec<usize> = vec![0; n];
    for site in sites {
        // `a op1 b op2 c` becomes `op2(op1(a, b), c)`: the calls open
        // outermost first before the left operand, each operator becomes `,`
        // and every operator after the first also closes the call before it.
        // A leading space: the token before may be an identifier (`if`,
        // `and`, `not`), and gluing would make one name of the two.
        let mut open = String::new();
        for &(_, _, function) in site.ops.iter().rev() {
            open.push(' ');
            open.push_str(function);
            open.push('(');
        }
        // Sites are recorded innermost first (a parse finishes an operand
        // before the operator around it), so a later site that starts on the
        // same token encloses the earlier one and opens before it:
        // `a * b + c` is `kestra_add( kestra_mul(a, b), c)`.
        if let Some(inner) = prefix[site.start].take() {
            open.push_str(&inner);
        }
        prefix[site.start] = Some(open);
        for (index, &(first, last, _)) in site.ops.iter().enumerate() {
            // An operator on the site's own first token has no left operand:
            // it is a unary minus (`Parser::unary_minus_sites`), and `-x` becomes
            // `kestra_mul(-1, x)`.
            let text = if first == site.start {
                "-1,"
            } else if index == 0 {
                ","
            } else {
                "),"
            };
            replacement[first] = Some(text.to_string());
            if last != first {
                replacement[last] = Some(String::new());
            }
        }
        suffixes[site.end] += 1;
    }
    let mut cursor = content_start;
    for (i, t) in toks.iter().enumerate() {
        if let Some(p) = &prefix[i] {
            out.push_str(p);
        }
        out.push_str(&src[cursor..t.start]);
        match &replacement[i] {
            Some(r) => out.push_str(r),
            None => out.push_str(&src[t.start..t.end]),
        }
        for _ in 0..suffixes[i] {
            out.push(')');
        }
        cursor = t.end;
    }
    out.push_str(&src[cursor..content_end]);
}
