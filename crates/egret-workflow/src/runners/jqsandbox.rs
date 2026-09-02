// kestra: core/src/main/java/io/kestra/core/runners/pebble/filters/JqFilter.java
// jaq: jaq-core 3.1.1, jaq-std 3.0.3, jaq-json 2.0.3

//! What `jq` is allowed to be, before `runners::filters::jq` runs it.
//!
//! Kestra hands the author's query straight to `JsonQuery.compile` and lets
//! `jackson-jq` do whatever it does. egret cannot: a rendered `Property` is
//! author-supplied source on a request path, and the jq language can express,
//! in a handful of characters, three things that are not "a wrong answer" but
//! "the process is gone".
//!
//! # 1. The parser overflows the stack, and that is an ABORT
//!
//! `Loader::load` is recursive descent with no depth limit. Measured:
//! **692** nested `[` on the 8 MiB main stack and **171 on a 2 MiB stack — a
//! tokio worker's — i.e. a template about 360 bytes long.** `[`, `(` and
//! `{a:` all reach it.
//!
//! A stack overflow is `SIGABRT`, not a panic: `catch_unwind` does not see it
//! and neither does `tests/oracle.rs`'s panic guard, which is why nothing in
//! this package would have reported it. [`check_source`] bounds the nesting
//! **before** the loader is called, the way `rewrite::MAX_NESTING_DEPTH`
//! bounds the expression rewriter for the same reason.
//!
//! # 2. The evaluator has no budget of any kind
//!
//! There is no fuel, step or depth limit anywhere in `jaq-core` 3.1
//! (`grep fuel|budget|steps` finds nothing), and minijinja's own `FUEL` does
//! not help, because the whole jq program runs inside one filter call.
//!
//! An earlier cut of this filter counted bytes **as outputs were emitted**,
//! which bounds a generator that emits and nothing else: `[repeat(1)]`,
//! `reduce repeat(1) as $x (0; .+1)`, `until(false; .)` and `def f: f; f`
//! all produce their first output never, and walk straight past it.
//!
//! jaq exposes exactly two places a caller can stand: the **definition list**
//! and the **native function list**, both arguments to the loader and the
//! compiler. Everything below is built on those two.
//!
//! - [`BOUNDED_DEFS`] replaces jaq's own `repeat`, `recurse`, `while` and
//!   `until` — which are jq-source definitions, not natives — with
//!   depth-counting versions that `error` past [`MAX_STEPS`]. That closes the
//!   whole family, `..`, `paths`, `walk` and `leaf_paths` included, since
//!   every one of them is defined on `recurse`.
//! - `range/3` is a native, so it is **renamed** and shadowed by a definition
//!   that refuses a count past [`MAX_STEPS`] before calling it. Renaming is
//!   possible because a native is a `(name, args, ptr)` tuple; the name is
//!   just the first field.
//! - a **user `def` is refused outright**. `def f: f; f` is two words and no
//!   bound above touches it, mutual recursion cannot be detected by reading
//!   the text, and a `def` inside a one-line `Property` expression is not a
//!   thing flows do. Refused by name, like the regex constructs in
//!   `super::javare`, and recorded as a divergence rather than hidden.
//!
//! # 3. Some natives read the host, and the guard has to be an ALLOW-list
//!
//! `env` returns the whole process environment. `debug`/`stderr` write
//! arbitrary author text into the service log (`egret-bin` installs
//! `LogTracer`, so `eprintln`-shaped output becomes log records).
//! `localtime`/`strflocaltime` read `jiff::tz::TimeZone::system()`. `halt`
//! reaches `std::process::exit`. **Kestra's `jackson-jq` at `Versions.JQ_1_6`
//! has none of the six** — measured through the harness — so withholding them
//! is fidelity as well as the obvious call.
//!
//! A deny-list would be fail-OPEN: `jaq-std = "3.0"` is a caret range, so a
//! `cargo update` to 3.1 can register a native that no deny-list mentions.
//! [`ALLOWED_NATIVES`] is the whole set egret runs, committed, and
//! `the_allow_list_covers_every_native_jaq_registers` fails if the crates
//! ever register a name that is on neither list.

use jaq_core::load::parse::Def;
use jaq_core::Native;
use jaq_json::Val;

/// The one value type this crate runs jq over.
///
/// **`natives` cannot be generic**: `jaq_json::funs()` is written for
/// `jaq_json::Val` specifically, not for any `D: DataT`, so the whole native
/// list is pinned to it. That is also why a budget-carrying newtype around
/// `Val` was not an option for bounding the evaluator -- it would have cost
/// `jaq_json`'s entire half of the standard library.
pub(super) type Data = jaq_core::data::JustLut<Val>;

/// The recursion and generator bound: how many times a `repeat`, `recurse`,
/// `while` or `until` may go round, and how many values a `range` may name.
///
/// **Two ceilings meet here and the lower one wins**, which is worth stating
/// rather than dressing one of them up as the derivation:
///
/// - a render that survives is at most [`super::env::MAX_OUTPUT_BYTES`], and
///   an emitted value costs at least two characters (itself and a separator),
///   so more than `MAX_OUTPUT_BYTES / 2` steps cannot produce a result anyone
///   can see. That is the *usefulness* ceiling, and it is 524 288.
/// - a step costs time whether or not it emits. Measured on this machine:
///   `[repeat(1)]` spends **3.3 s** at 1 048 576 steps and **0.2 s** at
///   65 536. That is the *tolerable* ceiling on a request path, and it is the
///   lower of the two.
///
/// 65 536 is therefore the bound, and it is far above anything real: the
/// deepest recursion in this package's own 132-expression jq sweep is in
/// single digits. A legitimate program that needs more is a recorded
/// divergence, like every other refusal in this module.
// Read by the tests and by a human; the runtime path uses the LITERAL in
// `BOUNDED_DEFS`, because a `Def<&'static str>` borrows its source.
// `the_literal_in_the_jq_source_is_the_bound` is what ties the two together.
#[allow(dead_code)]
pub(super) const MAX_STEPS: usize = 65536;

/// The nesting depth [`check_source`] allows in a jq program.
///
/// jaq's parser overflows at 171 on a 2 MiB stack in a debug build. 64 leaves
/// a factor of 2.6 there and far more in a release build, where frames are
/// smaller, and no jq program a flow author writes is anywhere near it — the
/// deepest in this package's own 132-expression sweep is 4.
pub(super) const MAX_SOURCE_DEPTH: usize = 64;

/// The length [`check_source`] allows in a jq program, in bytes.
///
/// Bounds the parse itself rather than what it produces. Derived from the same
/// output cap: a query longer than the largest output a render can return is
/// not a query anyone can use the result of.
pub(super) const MAX_SOURCE_BYTES: usize = super::env::MAX_OUTPUT_BYTES;

/// Why a jq program was refused before it ran.
///
/// Kept separate from "the program failed", because `JqFilter` reports every
/// runtime failure with one sentence about the *input* and these are about the
/// *query*.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Refused(pub(super) String);

/// The definitions that replace jaq's own.
///
/// Read `jaq-core/src/defs.jq` beside this: the originals are
///
/// ```text
/// def repeat(f): def rec: f, rec; rec;
/// def recurse(f): def rec: ., (f | rec); rec;
/// def while(cond; update): def rec: if cond then ., (update | rec) else empty end; rec;
/// def until(cond; update): def rec: if cond then . else update | rec end; rec;
/// def range(from; to): range(from; to; 1);
/// def range(to): range(0; to);
/// ```
///
/// — four unbounded recursions and two wrappers around the native. Each one
/// below is the same definition with a counter threaded through it, and
/// `range` additionally refuses before the native rather than after.
///
/// `recurse/0` and `recurse/2` are re-stated verbatim because they are defined
/// in terms of `recurse/1` and would otherwise still resolve to jaq's
/// unbounded one.
const BOUNDED_DEFS: &str = r#"
def repeat(f): def rec($n): if $n <= 0 then error("jq: repeat exceeded egret's step bound") else f, rec($n-1) end; rec(65536);
def recurse(f): def rec($n): if $n <= 0 then error("jq: recurse exceeded egret's step bound") else ., (f | rec($n-1)) end; rec(65536);
def recurse: recurse(.[]?);
def recurse(f; cond): recurse(f | select(cond));
def while(cond; update): def rec($n): if $n <= 0 then error("jq: while exceeded egret's step bound") elif cond then ., (update | rec($n-1)) else empty end; rec(65536);
def until(cond; update): def rec($n): if $n <= 0 then error("jq: until exceeded egret's step bound") elif cond then . else update | rec($n-1) end; rec(65536);
def range($from; $to; $by): if $by != 0 and ($to - $from) / $by > 65536 then error("jq: range exceeded egret's step bound") else __egret_range($from; $to; $by) end;
def range(from; to): range(from; to; 1);
def range(to): range(0; to);
"#;

/// The name the native `range/3` is moved to, so a bounded definition can
/// take its place. Chosen to be unwritable by an author: a jq identifier
/// cannot begin with `_` followed by `_egret`… it can, so the source check
/// refuses the prefix instead. See [`check_source`].
pub(super) const RENAMED_RANGE: &str = "__egret_range";

/// The natives egret withholds, each with the reason, and each **absent from
/// Kestra's own jq** so withholding it is fidelity too.
///
/// Named by `name/arity`, which is how jaq keys them.
// Read by `the_allow_list_covers_every_native_jaq_registers`, which is where
// it does its work: the runtime path filters by `ALLOWED_NATIVES` alone, and
// this is what makes a native that is on NEITHER list a test failure.
#[allow(dead_code)]
pub(super) const WITHHELD_NATIVES: &[(&str, &str)] = &[
    // `std::env::vars()`: the whole process environment, `DATABASE_URL` and
    // every secret in it.
    ("env/0", "reads the process environment"),
    // `jaq-std`'s `log()` feature. `egret-bin` installs `LogTracer`, so this
    // is an author writing arbitrary text into the service log, in unbounded
    // quantity.
    ("debug_empty/0", "writes to the service log"),
    ("stderr_empty/0", "writes to the service log"),
    // `jiff::tz::TimeZone::system()`: the host's timezone.
    ("localtime/0", "reads the host timezone"),
    ("strflocaltime/1", "reads the host timezone"),
    // `jaq_core::unwrap_valr` turns this into `std::process::exit`. That call
    // is not reached (see `runners::filters::run_jq`), but a native whose
    // whole purpose is to end the process does not need to be registered.
    ("halt/1", "ends the process"),
];

/// Every native egret DOES register, committed so that a dependency bump
/// adding one is a test failure rather than a silent new capability.
///
/// 108 names: the 114 `jaq-core`, `jaq-std` and `jaq-json` register at the
/// versions in the lock file, minus [`WITHHELD_NATIVES`]' six.
pub(super) const ALLOWED_NATIVES: &[&str] = &[
    "acos/0",
    "acosh/0",
    "ascii_downcase/0",
    "ascii_upcase/0",
    "asin/0",
    "asinh/0",
    "atan/0",
    "atan2/2",
    "atanh/0",
    "bsearch/1",
    "cbrt/0",
    "ceil/0",
    "contains/1",
    "copysign/2",
    "cos/0",
    "cosh/0",
    "decode_base64/0",
    "decode_uri/0",
    "encode_base64/0",
    "encode_uri/0",
    "endswith/1",
    "erf/0",
    "erfc/0",
    "error_empty/0",
    "escape_html/0",
    "escape_sh/0",
    "exp/0",
    "exp10/0",
    "exp2/0",
    "explode/0",
    "expm1/0",
    "fabs/0",
    "fdim/2",
    "first/1",
    "floor/0",
    "fma/3",
    "fmax/2",
    "fmin/2",
    "fmod/2",
    "frexp/0",
    "fromdateiso8601/0",
    "fromjson/0",
    "gmtime/0",
    "group_by/1",
    "has/1",
    "hypot/2",
    "ilogb/0",
    "implode/0",
    "indices/1",
    "j0/0",
    "j1/0",
    "jn/2",
    "key_values/0",
    "keys_unsorted/0",
    "last/1",
    "ldexp/2",
    "length/0",
    "lgamma/0",
    "limit/2",
    "log/0",
    "log10/0",
    "log1p/0",
    "log2/0",
    "ltrim/0",
    "ltrimstr/1",
    "matches/2",
    "max_by_or_empty/1",
    "min_by_or_empty/1",
    "mktime/0",
    "modf/0",
    "nearbyint/0",
    "nextafter/2",
    // Non-deterministic, and kept: `jackson-jq` has `NowFunction`, so this
    // one IS fidelity. It has no golden for the same reason `function-now`
    // has none.
    "now/0",
    "path/1",
    "path_value/1",
    "pow/2",
    "range/3",
    "remainder/2",
    "reverse/0",
    "rint/0",
    "round/0",
    "rtrim/0",
    "rtrimstr/1",
    "scalbln/2",
    "sin/0",
    "sinh/0",
    "skip/2",
    "sort/0",
    "sort_by/1",
    "split_/2",
    "split_matches/2",
    "sqrt/0",
    "startswith/1",
    "strftime/1",
    "strptime/1",
    "tan/0",
    "tanh/0",
    "tgamma/0",
    "tobytes/0",
    "todateiso8601/0",
    "tojson/0",
    "trim/0",
    "trunc/0",
    "unescape_html/0",
    "utf8bytelength/0",
    "y0/0",
    "y1/0",
    "yn/2",
];

/// The eight definitions [`BOUNDED_DEFS`] replaces, by `name/arity`.
///
/// They are **contiguous** in `jaq-core/src/defs.jq` (lines 16-23), which is
/// what makes replacing them in place safe: everything above them is what
/// they need (`error`, `select`, `empty`) and everything below them needs
/// them (`paths` is `skip(1; path(..))`, i.e. `recurse/0`). Appending instead
/// would leave `paths` resolving jaq's unbounded `recurse`, and prepending
/// would leave `error` undefined.
///
/// [`replace_unbounded_defs`] asserts the contiguity rather than assuming it,
/// so a `jaq-core` bump that moves one of them fails loudly.
const REPLACED_DEFS: &[(&str, usize)] = &[
    ("range", 2),
    ("range", 1),
    ("repeat", 1),
    ("recurse", 1),
    ("recurse", 0),
    ("recurse", 2),
    ("while", 2),
    ("until", 2),
];

/// The six definitions that **reference** a withheld native, by
/// `name/arity`.
///
/// Removing a native is not enough on its own. `jaq-std/src/defs.jq` lines
/// 3-9 say `def stderr: (stderr_empty as $x | .), .;` and
/// `def halt: halt(0);`, so dropping `stderr_empty/0` or `halt/1` makes the
/// **whole standard library** fail to compile and every jq call fails,
/// whatever the author wrote. Measured: the first cut of this module did
/// exactly that, and `{{ x | jq('.a') }}` broke along with `env`.
///
/// So the six user-visible names stay, and their bodies become refusals that
/// say why. `env`, `localtime` and `strflocaltime` need no entry here: no
/// definition references them, so dropping the natives is enough and the
/// author gets jq's own "undefined filter".
const REFUSED_DEFS: &[(&str, usize)] = &[
    ("stderr", 0),
    ("debug", 0),
    ("debug", 1),
    ("halt", 0),
    ("halt_error", 1),
    ("halt_error", 0),
];

/// The bodies that replace [`REFUSED_DEFS`].
const REFUSED_DEFS_SOURCE: &str = r#"
def stderr: error("jq: 'stderr' is not available in egret -- it writes to the service log, and Kestra's jq does not have it");
def debug: error("jq: 'debug' is not available in egret -- it writes to the service log, and Kestra's jq does not have it");
def debug(msgs): error("jq: 'debug' is not available in egret -- it writes to the service log, and Kestra's jq does not have it");
def halt: error("jq: 'halt' is not available in egret -- it ends the process, and Kestra's jq does not have it");
def halt_error($exit_code): error("jq: 'halt_error' is not available in egret -- it ends the process, and Kestra's jq does not have it");
def halt_error: error("jq: 'halt_error' is not available in egret -- it ends the process, and Kestra's jq does not have it");
"#;

/// Definitions where `jaq`'s jq and Kestra's `jackson-jq` disagree and the
/// disagreement is fixable in jq source.
///
/// **`scan` is the one that would have reached a flow.** `jaq-std/defs.jq`
/// says `def scan(re): scan(re; "");` — no `g` flag — so
/// `"a1b2" | [scan("[0-9]")]` is `["1"]` there and `["1","2"]` in Kestra
/// (and in jq 1.6, whose `scan` is global). Both engines answer without
/// error, which puts it in the silent class; measured through the harness.
/// jq 1.6's `scan` with an explicit flag set, and its capture behaviour:
/// with capture groups it yields the captures, not the whole match. jaq's
/// yields `.[0].string` either way, so `"a1b2" | [scan("([0-9])")]` is
/// `[["1"],["2"]]` in Kestra and was `["1"]` here.
///
/// Two groups because the two arities are **not adjacent** in
/// `jaq-std/defs.jq` (lines 92 and 109) and [`splice`] insists on
/// contiguity — which is the assertion doing its job, not an obstacle.
const KESTRA_DEFS_2: &[(&str, usize)] = &[("scan", 2)];
const KESTRA_DEFS_2_SOURCE: &str = r#"
def scan(re; flags): matches(re; flags)[] | if length > 1 then [.[1:][].string] else .[0].string end;
"#;
const KESTRA_DEFS_1: &[(&str, usize)] = &[("scan", 1)];
const KESTRA_DEFS_1_SOURCE: &str = r#"
def scan(re): scan(re; "g");
"#;

/// Parses [`BOUNDED_DEFS`]. Panics only if this file is wrong, which
/// [`tests::the_bounded_definitions_parse`] would have caught.
///
/// The bound is written into the jq source as a **literal**, because a
/// `Def<&'static str>` borrows from its source and the `Loader` outlives no
/// `String` built per call.
/// [`tests::the_literal_in_the_jq_source_is_the_bound`] is what keeps the
/// literal and [`MAX_STEPS`] from drifting apart.
pub(super) fn bounded_defs() -> Vec<Def<&'static str>> {
    jaq_core::load::parse(BOUNDED_DEFS, |p| p.defs())
        .expect("runners::jqsandbox::BOUNDED_DEFS must be valid jq")
}

/// jaq's own definitions, with the unbounded ones and the host-touching ones
/// swapped out **in place**.
pub(super) fn definitions() -> Vec<Def<&'static str>> {
    let mut defs: Vec<Def<&'static str>> = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs())
        .collect();
    splice(&mut defs, REPLACED_DEFS, bounded_defs());
    splice(&mut defs, REFUSED_DEFS, refused_defs());
    splice(&mut defs, KESTRA_DEFS_2, parse_defs(KESTRA_DEFS_2_SOURCE));
    splice(&mut defs, KESTRA_DEFS_1, parse_defs(KESTRA_DEFS_1_SOURCE));
    defs
}

/// Replaces the definitions named by `keys` with `replacements`, at the
/// position of the first of them.
///
/// Position matters both ways: everything **above** the group is what it needs
/// (`error`, `select`, `empty`) and everything **below** it needs the group
/// (`paths` is `skip(1; path(..))`, i.e. `recurse/0`). Appending would leave
/// `paths` on jaq's unbounded `recurse`; prepending would leave `error`
/// undefined.
///
/// Both assertions are the point of the function: a `jaq` bump that renames,
/// re-arities or separates one of these fails here rather than silently
/// leaving the unbounded original in place.
fn splice(
    defs: &mut Vec<Def<&'static str>>,
    keys: &[(&str, usize)],
    replacements: Vec<Def<&'static str>>,
) {
    let matches: Vec<usize> = defs
        .iter()
        .enumerate()
        .filter(|(_, d)| keys.contains(&(d.name, d.args.len())))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        matches.len(),
        keys.len(),
        "jaq no longer defines exactly the filters this file replaces: {keys:?}"
    );
    let first = matches[0];
    assert!(
        matches.iter().enumerate().all(|(n, i)| *i == first + n),
        "the definitions this file replaces are no longer contiguous, so replacing them in \
         place would move a filter past something that needs it: {keys:?}"
    );
    defs.splice(first..first + matches.len(), replacements);
}

/// Parses [`REFUSED_DEFS_SOURCE`].
fn refused_defs() -> Vec<Def<&'static str>> {
    jaq_core::load::parse(REFUSED_DEFS_SOURCE, |p| p.defs())
        .expect("runners::jqsandbox::REFUSED_DEFS_SOURCE must be valid jq")
}

/// Parses one of this module's jq sources. Panics only if this file is
/// wrong, which a test would have caught.
fn parse_defs(source: &'static str) -> Vec<Def<&'static str>> {
    jaq_core::load::parse(source, |p| p.defs())
        .unwrap_or_else(|| panic!("runners::jqsandbox: {source:?} must be valid jq"))
}

/// jaq's natives, minus [`WITHHELD_NATIVES`] and with the real `range/3`
/// moved aside so [`BOUNDED_DEFS`]' guarded `range` can take its name.
pub(super) fn natives() -> impl Iterator<Item = jaq_core::native::Filter<Native<Data>>> {
    jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs())
        // An ALLOW-list, not a deny-list: `jaq-std = "3.0"` is a caret range,
        // so a `cargo update` can register a native no deny-list mentions.
        .filter(|fun| ALLOWED_NATIVES.contains(&key(fun.0, fun.1.len()).as_str()))
        // `range/3` is a native, so `BOUNDED_DEFS`' guarded `range` can only
        // reach it under another name.
        .map(|(name, args, native)| {
            if name == "range" {
                (RENAMED_RANGE, args, native)
            } else {
                (name, args, native)
            }
        })
}

/// `name/arity`, which is how jaq keys a native and how this file lists them.
fn key(name: &str, arity: usize) -> String {
    format!("{name}/{arity}")
}

/// Refuses a jq program before the loader sees it.
///
/// Three checks, in the order they can be answered: length, nesting depth,
/// and the two identifiers egret reserves.
///
/// The scanner tracks jq string literals so that a bracket inside `"…"` is not
/// counted as nesting and a `def` inside `"…"` is not read as a definition.
/// `\(…)` interpolation is code again, and is counted.
pub(super) fn check_source(source: &str) -> Result<(), Refused> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Refused(format!(
            "jq: the query is longer than egret's {MAX_SOURCE_BYTES}-byte limit"
        )));
    }

    // `in_string` is a stack rather than a flag, because `"\( "x" )"` nests.
    let mut in_string: Vec<bool> = vec![false];
    let mut depth = 0usize;
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut word = String::new();

    while i < chars.len() {
        let c = chars[i];
        let inside = *in_string.last().expect("stack is never empty");

        if inside {
            match c {
                '\\' => {
                    if chars.get(i + 1) == Some(&'(') {
                        // Interpolation: the body is code, and it is nested.
                        in_string.push(false);
                        depth += 1;
                        i += 2;
                        continue;
                    }
                    // Any other escape consumes its second character, so a
                    // `\"` does not end the string.
                    i += 2;
                    continue;
                }
                '"' => {
                    in_string.pop();
                    i += 1;
                    continue;
                }
                _ => {
                    i += 1;
                    continue;
                }
            }
        }

        // Outside a string: a `#` comment runs to the end of the line.
        if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        if c.is_alphanumeric() || c == '_' {
            word.push(c);
            i += 1;
            continue;
        }
        if !word.is_empty() {
            check_word(&word)?;
            word.clear();
        }

        match c {
            '"' => in_string.push(true),
            '[' | '(' | '{' => {
                depth += 1;
                if depth > MAX_SOURCE_DEPTH {
                    return Err(Refused(format!(
                        "jq: the query nests more than {MAX_SOURCE_DEPTH} levels deep, which \
                         overflows the parser's stack"
                    )));
                }
            }
            ']' | '}' => depth = depth.saturating_sub(1),
            ')' => {
                depth = depth.saturating_sub(1);
                // The `)` that closes a `\(` returns to the string it was in.
                if in_string.len() > 1 && !*in_string.last().expect("checked len") {
                    in_string.pop();
                }
            }
            _ => {}
        }
        i += 1;
    }
    if !word.is_empty() {
        check_word(&word)?;
    }
    Ok(())
}

/// The two identifiers a program may not use.
fn check_word(word: &str) -> Result<(), Refused> {
    if word == "def" {
        return Err(Refused(
            "jq: 'def' (a user-defined function) is not available in egret -- a definition can \
             call itself, directly or through another definition, and nothing egret can read \
             from the source bounds that"
                .to_string(),
        ));
    }
    if word.starts_with("__egret") {
        return Err(Refused(format!("jq: '{word}' is a name egret reserves")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The jq sweep, as a fixture.** `oracle/jq-surface.txt` records what
    /// Kestra's `jackson-jq` answers for 191 expressions and what egret's
    /// `jaq` answers, and this re-runs every one of them.
    ///
    /// It exists because an earlier round recorded FIVE divergences and wrote
    /// "five entries record what is left", which reads as *five is what is
    /// left*. There were 21. Two of those — `scan` at both arities — are now
    /// fixed in `KESTRA_DEFS_*`; the rest are recorded here, each with a
    /// reason, where a prose count cannot hide them.
    ///
    /// The comparison mirrors the corpus's own error-class rule: a row where
    /// Kestra fails and egret fails is not a divergence whatever the two
    /// messages say, and a row where Kestra SUCCEEDS and egret refuses is.
    #[test]
    fn the_jq_surface_matches_the_engine() {
        let fixture = include_str!("../../oracle/jq-surface.txt");
        let mut rows = 0;
        let mut divergent = 0;
        for line in fixture.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(cols.len(), 5, "five tab-separated columns: {line}");
            let (expression, input, kestra, declared, reason) = (
                unescape(cols[0]),
                unescape(cols[1]),
                unescape(cols[2]),
                unescape(cols[3]),
                cols[4],
            );
            rows += 1;

            let agree = kestra == declared
                || (kestra == "error" && (declared == "error" || declared == "refused"));
            if agree {
                assert_eq!(
                    reason, "-",
                    "`{expression}`: the engines agree, so there is nothing to explain"
                );
            } else {
                divergent += 1;
                assert!(
                    reason.len() > 20,
                    "`{expression}`: Kestra answers {kestra:?} and egret answers {declared:?}; a \
                     divergence must say why, in the file"
                );
            }

            let template = "{{ v | jq(e) }}";
            let context = format!(
                "{{\"v\": {input}, \"e\": {}}}",
                serde_json::to_string(&expression).expect("a &str serialises")
            );
            let cx = crate::runners::env::RenderContext::from_json_str(&context)
                .unwrap_or_else(|e| panic!("`{expression}`: context must parse: {e}"));
            let actual = match crate::runners::env::render_str(template, &cx) {
                Ok(value) => value,
                Err(e) => {
                    let message = e.to_string();
                    if message.contains("invalid operation: jq: ") {
                        "refused".to_string()
                    } else {
                        "error".to_string()
                    }
                }
            };
            assert_eq!(
                actual, declared,
                "`{expression}`: the file says egret answers {declared:?} and it answers \
                 {actual:?}"
            );
        }
        assert_eq!(rows, 191, "the fixture lost rows");
        assert_eq!(
            divergent, 19,
            "the divergence count moved without the file saying so"
        );
    }

    /// JSON's escapes, which is what the fixture uses so a tab-separated file
    /// can carry a value containing a tab.
    fn unescape(field: &str) -> String {
        serde_json::from_str::<String>(&format!("\"{field}\"")).unwrap_or_else(|e| {
            panic!("{field:?} is not a JSON string body: {e}");
        })
    }

    #[test]
    fn the_source_check_bounds_nesting_before_the_parser_can_overflow() {
        // 171 is where jaq's parser overflows a 2 MiB stack. The bound is 64,
        // so the refusal happens with a wide margin -- and it happens for all
        // three bracket shapes.
        for open in ['[', '(', '{'] {
            let close = match open {
                '[' => ']',
                '(' => ')',
                _ => '}',
            };
            let deep: String = std::iter::repeat_n(open, 200)
                .chain(std::iter::repeat_n(close, 200))
                .collect();
            let refused = check_source(&deep).expect_err("must refuse");
            assert!(refused.0.contains("nests more than 64"), "{refused:?}");
        }
        // And it does NOT refuse the depth a real query has.
        check_source(".a.b | map(select(.x > 1)) | {y: [.[] | .z]}").expect("must allow");
    }

    /// A bracket inside a string literal is not nesting, and an escaped quote
    /// does not end the string. Both are the ways a character-counting
    /// scanner refuses a valid query.
    #[test]
    fn the_source_check_reads_string_literals() {
        let brackets_in_a_string = format!("\"{}\"", "[".repeat(200));
        check_source(&brackets_in_a_string).expect("a string is not nesting");
        check_source("\"a\\\"[[[b\"").expect("an escaped quote does not end the string");
        // Interpolation IS code again.
        check_source("\"v=\\(.a)\"").expect("interpolation is allowed");
        let deep_interpolation = format!("\"\\({}1{})\"", "[".repeat(100), "]".repeat(100));
        assert!(check_source(&deep_interpolation).is_err());
    }

    #[test]
    fn a_user_def_is_refused_by_name() {
        let refused = check_source("def f: f; f").expect_err("must refuse");
        assert!(refused.0.contains("'def'"), "{refused:?}");
        // `def` inside a string is not a definition, and neither is a word
        // that merely contains it.
        check_source("\"def\"").expect("a string is not a definition");
        check_source(".defined").expect("a longer word is not `def`");
        check_source("# def\n.a").expect("a comment is not a definition");
    }

    #[test]
    fn the_reserved_prefix_is_refused() {
        let refused = check_source("__egret_range(0; 3; 1)").expect_err("must refuse");
        assert!(refused.0.contains("reserves"), "{refused:?}");
    }

    #[test]
    fn a_very_long_query_is_refused_before_it_is_parsed() {
        let long = ".".repeat(MAX_SOURCE_BYTES + 1);
        assert!(check_source(&long).is_err());
    }

    /// The replacement definitions have to parse, or every jq call fails.
    #[test]
    fn the_bounded_definitions_parse() {
        let names: Vec<&str> = bounded_defs().iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            vec![
                "repeat", "recurse", "recurse", "recurse", "while", "until", "range", "range",
                "range"
            ]
        );
    }

    /// The jq source carries the bound as a literal because it has to; this
    /// is what stops the literal and the constant drifting apart, and what
    /// keeps the derivation from `MAX_OUTPUT_BYTES` true rather than
    /// merely written down.
    #[test]
    fn the_literal_in_the_jq_source_is_the_bound() {
        const { assert!(MAX_STEPS * 2 <= crate::runners::env::MAX_OUTPUT_BYTES) };
        let literal = MAX_STEPS.to_string();
        // Four recursion definitions plus the `range` guard.
        assert_eq!(BOUNDED_DEFS.matches(&literal).count(), 5);
    }

    /// `WITHHELD_NATIVES` and `ALLOWED_NATIVES` must be disjoint and must
    /// together be exactly what the three crates register. A `cargo update`
    /// that adds a native fails here rather than registering it silently,
    /// which is the whole reason this is an allow-list and not a deny-list.
    #[test]
    fn the_allow_list_covers_every_native_jaq_registers() {
        let registered: std::collections::BTreeSet<String> = jaq_core::funs::<Data>()
            .chain(jaq_std::funs())
            .chain(jaq_json::funs())
            .map(|f| format!("{}/{}", f.0, f.1.len()))
            .collect();

        let allowed: std::collections::BTreeSet<String> =
            ALLOWED_NATIVES.iter().map(|s| s.to_string()).collect();
        let withheld: std::collections::BTreeSet<String> = WITHHELD_NATIVES
            .iter()
            .map(|(s, _)| s.to_string())
            .collect();

        assert!(
            allowed.is_disjoint(&withheld),
            "a native cannot be both allowed and withheld: {:?}",
            allowed.intersection(&withheld).collect::<Vec<_>>()
        );
        let covered: std::collections::BTreeSet<String> =
            allowed.union(&withheld).cloned().collect();
        assert_eq!(
            registered, covered,
            "jaq registers natives this file does not classify (or classifies names it does not \
             register) -- a dependency bump has changed the surface"
        );
        assert_eq!(ALLOWED_NATIVES.len(), 108);
        assert_eq!(WITHHELD_NATIVES.len(), 6);
    }
}
