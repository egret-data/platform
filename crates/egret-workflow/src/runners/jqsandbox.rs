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
//! `Loader::load` is recursive descent with no depth limit, and a stack
//! overflow is `SIGABRT`, not a panic: `catch_unwind` does not see it and
//! `tests/oracle.rs`'s panic guard cannot report it.
//!
//! **An earlier cut bounded the source's BRACKET NESTING, and that was the
//! wrong invariant** — the parser recurses on productions, not on characters.
//! Six of the seven vectors contain no bracket at all. Measured here, on a
//! 2 MiB stack (a tokio worker's), by bisecting the shortest program that
//! aborts:
//!
//! | construct | shortest abort | stack per source byte |
//! |---|---:|---:|
//! | `-`×N + `1` | **205 B** | **9.99 KiB** |
//! | `[`×N … | 345 B | 5.94 KiB |
//! | `{a:`×N … | 513 B | 3.99 KiB |
//! | `try `×N + `.` | 817 B | 2.51 KiB |
//! | `.` + `\|.`×N | 5.1 kB | 0.40 KiB |
//! | `.` + `,.`×N | 43 kB | 0.05 KiB |
//! | `.` + ` // .`×N | 109 kB | 0.02 KiB |
//!
//! The cost per byte is **constant in the stack size** — re-measured at 8 MiB
//! and 64 MiB, `-`×N gives 9.93 and 9.91 KiB per byte — so it is a property
//! of the construct and can be used as a bound.
//!
//! So there are two mechanisms and no scanner:
//!
//! 1. **the parse and the run happen on a dedicated thread with an explicit
//!    stack** ([`JQ_STACK_BYTES`]), so the budget does not depend on which
//!    caller reached this filter;
//! 2. **the source is capped at [`MAX_SOURCE_BYTES`]**, which is that stack
//!    divided by the worst measured cost per byte and by a margin. The
//!    arithmetic is in the constants, so the next person can re-derive it
//!    with a different jaq.
//!
//! A 1 MiB cap — what the earlier cut allowed — is not compatible with any
//! stack that can be allocated: it would need 10 GiB. The honest cap is small.
//!
//! # 2. The evaluator has no budget of any kind
//!
//! There is no fuel, step or depth limit anywhere in `jaq-core` 3.1
//! (`grep fuel|budget|steps` finds nothing), and minijinja's own `FUEL` does
//! not help, because the whole jq program runs inside one filter call.
//!
//! Two cuts have now been wrong about this, in the same way. The first
//! counted bytes **as outputs were emitted**, which a program that emits
//! nothing walks past. The second counted **steps**, which bounds neither a
//! step that never happens (`range` with a step of `0` spins inside the
//! native, one call) nor a step whose *cost* grows (`[recurse(.+[1])]` stops
//! at 65 536 iterations and reaches 9.9 GiB, because each value is one
//! element longer than the last).
//!
//! Both of those are outside "count the steps", so the bound is not a
//! counter. jaq offers exactly two hooks — the definition list and the native
//! list — and every construct that can loop **without emitting** goes through
//! one of them:
//!
//! | construct | how it is bounded |
//! |---|---|
//! | `recurse`, `repeat`, `while`, `until` (and `..`, `paths`, `walk`, `leaf_paths`, which are defined on them) | replaced definitions that call [`TICK`] once per step |
//! | `range` — the only native generator | a guarded definition that refuses a step of `0` and a count past [`MAX_STEPS`] |
//! | a user `def` | refused; nothing else can bound a recursion egret did not write |
//!
//! **[`TICK`] is a wall-clock DEADLINE, not a counter**, and that is the
//! point: it does not need anyone to predict the shape of the next abuse. It
//! also carries a cheap shallow-length cap, which catches a single enormous
//! step that the deadline would let through.
//!
//! What it cuts: a legitimate jq program that runs longer than
//! [`MAX_RUN`]. Nothing in this package's 191-expression sweep is within two
//! orders of magnitude of it.
//!
//! **What it does not cover, stated rather than implied:** a program can
//! still compose *bounded* pieces into an expensive whole — nested `range`s
//! inside a collector, say — and reach the deadline only at the next tick.
//! The complete answer is abandoning the thread on a timeout, which leaks a
//! runaway thread into a long-lived process, and that is worse than a bound
//! that is late.
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

use jaq_core::box_iter::box_once;
use jaq_core::load::parse::Def;
use jaq_core::{Cv, DataT, Error, Exn, Native, ValXs};
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

/// The stack the jq thread gets, and the only stack the parser ever runs on.
///
/// Explicit rather than inherited: a 2 MiB tokio worker and an 8 MiB main
/// thread give the same source wildly different room, and a bound that
/// depends on the caller is not a bound.
///
/// **256 MiB, because the cap is derived from it and a smaller cap cut real
/// programs.** A thread stack is reserved address space committed lazily, so
/// the number costs nothing until it is touched, and what can actually be
/// touched is [`MAX_SOURCE_BYTES`] x [`WORST_STACK_PER_SOURCE_BYTE`] = about
/// 43 MiB, only ever by a deliberately pathological source. Raising the stack
/// is the right lever here; shaving [`PARSE_STACK_MARGIN`] is not.
pub(super) const JQ_STACK_BYTES: usize = 256 * 1024 * 1024;

/// The worst measured parser stack cost per byte of jq source, over every
/// recursive construct found: `-`×N, at **9.99 KiB per byte** on a 2 MiB
/// stack, 9.93 at 8 MiB and 9.91 at 64 MiB. Rounded up.
///
/// Re-measure this against a new `jaq` by bisecting the shortest program of
/// each shape that aborts; the module doc has the table.
const WORST_STACK_PER_SOURCE_BYTE: usize = 10 * 1024;

/// How much of [`JQ_STACK_BYTES`] the worst possible source is allowed to
/// need. Six, so the parse touches at most a sixth of the stack it has.
const PARSE_STACK_MARGIN: usize = 6;

/// The length [`check_source`] allows, **derived** rather than chosen:
/// 256 MiB / 10 KiB / 6 = 4369 bytes.
///
/// The arithmetic is here so it can be re-derived. An earlier cut derived
/// 1092 from a 64 MiB stack and that refused a realistic 1 352-byte jq
/// pipeline Kestra runs — a functional cut with no fidelity behind it. The
/// lever is the stack, not the margin.
pub(super) const MAX_SOURCE_BYTES: usize =
    JQ_STACK_BYTES / WORST_STACK_PER_SOURCE_BYTE / PARSE_STACK_MARGIN;

/// How long a jq program may run before [`TICK`] refuses it.
///
/// A deadline rather than a step count, because the two shapes that got past
/// the step count — a native that spins without stepping, and a step whose
/// cost grows — are both invisible to a counter and both obvious to a clock.
pub(super) const MAX_RUN: std::time::Duration = std::time::Duration::from_secs(1);

/// The shallow length (elements, entries or bytes) a value may have at a
/// step, enforced by `__egret_guard` in [`BOUNDED_DEFS`] rather than here —
/// see [`tick_native`] for why it could not be done in Rust.
///
/// Insurance against one enormous step that the deadline would let through;
/// the deadline is what actually closes the growth cases. Derived from
/// [`super::env::MAX_OUTPUT_BYTES`]: a value with more elements than the
/// output cap has bytes cannot be rendered.
#[allow(dead_code)]
pub(super) const MAX_VALUE_LENGTH: usize = super::env::MAX_OUTPUT_BYTES;

/// The name of the per-step guard [`BOUNDED_DEFS`] calls.
pub(super) const TICK: &str = "__egret_tick";

/// How deeply nested a value may be when it leaves the jq thread.
///
/// **This is a third bound, on a third axis, and the first two do not touch
/// it.** `MAX_SOURCE_BYTES` bounds the source; `MAX_STEPS` and `MAX_RUN`
/// bound the evaluation; neither bounds the DEPTH of the value jq builds.
/// `jq('reduce range(1400) as $i (.;[.])')` is a 40-byte source that
/// evaluates in 0.04 s and builds a 1 400-deep value, and every one of
/// egret's own value writers is recursive over it.
///
/// **128 is `serde_json`'s own recursion limit**, which is what already
/// bounds the *other* way a value enters this crate — a context deeper than
/// that fails to parse. Using the same number gives one invariant instead of
/// two: **no value anywhere in this crate is more than
/// `MAX_VALUE_DEPTH` deep.**
///
/// It is also far inside what the recursion can afford. Measured, by
/// bisecting the depth that overflows a thread of a given size, and linear in
/// that size:
///
/// | writer | KiB of stack per level |
/// |---|---:|
/// | `runners::env::kestra_to_string` | **1.346** |
/// | `runners::ionfmt::write_ion` | 1.049 |
/// | `runners::env::write_json` | 1.018 |
/// | `runners::yamlfmt::to_yaml` | 0.391 |
///
/// At the worst of those, 128 levels need 172 KiB — a **twelvefold** margin
/// on the 2 MiB stack a tokio worker gives the caller, which is the thread
/// that runs every one of them.
pub(super) const MAX_VALUE_DEPTH: usize = 128;

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
def __egret_guard: __egret_tick as $t | if . >= "" and length > 1048576 then error("jq: a value grew past egret's element limit") else . end;
def repeat(f): def rec($n): if $n <= 0 then error("jq: repeat exceeded egret's step bound") else __egret_guard as $t | (f, rec($n-1)) end; rec(65536);
def recurse(f): def rec($n): if $n <= 0 then error("jq: recurse exceeded egret's step bound") else __egret_guard as $t | (., (f | rec($n-1))) end; rec(65536);
def recurse: recurse(.[]?);
def recurse(f; cond): recurse(f | select(cond));
def while(cond; update): def rec($n): if $n <= 0 then error("jq: while exceeded egret's step bound") elif cond then __egret_guard as $t | (., (update | rec($n-1))) else empty end; rec(65536);
def until(cond; update): def rec($n): if $n <= 0 then error("jq: until exceeded egret's step bound") elif cond then . else __egret_guard as $t | (update | rec($n-1)) end; rec(65536);
def range($from; $to; $by): if $by == 0 then empty elif ($to - $from) / $by > 65536 then error("jq: range exceeded egret's step bound") else __egret_range($from; $to; $by) end;
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

/// **`BOUNDED_DEFS` may use only what `jaq-core`'s own `defs.jq` has defined
/// ABOVE the group it replaces** — `error`, `select`, `empty`, the operators,
/// and the natives. `type`, `arrays` and `strings` come from `jaq-std` and
/// `jaq-json`, which load later, and referencing one of them makes the WHOLE
/// standard library fail to compile: measured, `{{ x | jq('.a') }}` broke.
/// That is why `__egret_guard` tests `. >= ""` — jq's total order puts
/// numbers before strings, arrays and objects, so it is exactly "this value
/// has a length" — rather than asking `type`.
///
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
        .chain(core::iter::once(tick_native::<Data>()))
}

/// `name/arity`, which is how jaq keys a native and how this file lists them.
fn key(name: &str, arity: usize) -> String {
    format!("{name}/{arity}")
}

/// Refuses a jq program before the loader sees it.
///
/// **Two checks and no scanner.** The scanner this replaces had a defect of
/// exactly the kind a scanner has: it popped an interpolation frame on any
/// `)`, so `"\((1),` left it believing it was inside a string and it stopped
/// counting — `"\((1),` + `[`×171 + `]`×171 + `)"`, 352 bytes, aborted while
/// 150 levels returned normally. Depth-of-anything in the text was the wrong
/// invariant anyway; see the module doc.
///
/// The `def` check is deliberately **not** string-aware: a `def` inside a
/// string literal is refused too. That over-refuses `jq('"def"')` and it is
/// the price of not having a scanner to be wrong about, which is a trade this
/// module has now paid for twice.
pub(super) fn check_source(source: &str) -> Result<(), Refused> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Refused(format!(
            "jq: the query is {} bytes and egret's limit is {MAX_SOURCE_BYTES} -- jaq's parser \
             costs up to {} KiB of stack per byte of source, and that limit is the {} MiB \
             thread stack divided by it",
            source.len(),
            WORST_STACK_PER_SOURCE_BYTE / 1024,
            JQ_STACK_BYTES / 1024 / 1024,
        )));
    }
    if contains_word(source, "def") {
        return Err(Refused(
            "jq: 'def' (a user-defined function) is not available in egret -- a definition can \
             call itself, directly or through another definition, and it is the one recursion \
             egret does not write and so cannot bound"
                .to_string(),
        ));
    }
    if source.contains("__egret") {
        return Err(Refused(
            "jq: '__egret' is a name prefix egret reserves".to_string(),
        ));
    }
    Ok(())
}

/// `word` as a whole identifier somewhere in `source`.
fn contains_word(source: &str, word: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    source.match_indices(word).any(|(at, _)| {
        let before = source[..at].chars().next_back();
        let after = source[at + word.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

/// The depth of a jq value, **computed iteratively**, capped at
/// [`MAX_VALUE_DEPTH`] + 1.
///
/// Iteratively because the thing it is guarding against is recursion over
/// this value: a recursive depth check is the same overflow one function
/// earlier. The explicit stack holds `(value, depth)` pairs and never grows
/// past the number of nodes at the current frontier.
///
/// It stops as soon as the limit is exceeded, so a value 65 536 deep costs
/// 129 steps, not 65 536.
pub(super) fn too_deep(value: &Val) -> bool {
    let mut stack: Vec<(&Val, usize)> = vec![(value, 1)];
    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_VALUE_DEPTH {
            return true;
        }
        match value {
            Val::Arr(items) => stack.extend(items.iter().map(|item| (item, depth + 1))),
            Val::Obj(entries) => {
                stack.extend(entries.values().map(|item| (item, depth + 1)));
            }
            _ => {}
        }
    }
    false
}

/// The message [`too_deep`] earns.
pub(super) fn too_deep_message() -> String {
    format!(
        "jq: the result nests more than {MAX_VALUE_DEPTH} levels deep, which is what every \
         value writer in egret recurses over"
    )
}

/// The per-step deadline [`BOUNDED_DEFS`] calls, as a native.
///
/// Identity on the way through; it raises once the run has taken longer than
/// [`MAX_RUN`]. [`start_run`] sets the clock, on the jq thread, at the top of
/// each call.
///
/// **The length half of the guard is in jq, not here**, and not by choice: a
/// `RunPtr` is a higher-ranked `fn` over `<Data as DataT>::V<'a>`, and naming
/// that projection as `Val` — which is what inspecting the value needs — does
/// not typecheck. `__egret_guard` in [`BOUNDED_DEFS`] does it with `length`
/// instead, which is O(1) on the three types it applies to.
fn tick_native<D: DataT>() -> jaq_core::native::Filter<Native<D>>
where
    for<'a> D::V<'a>: jaq_core::ValT,
{
    // Generic over `D`, the way jaq writes its own natives: with the type
    // parameter in place the associated type stays unnormalised, and the fn
    // item coerces to the higher-ranked `RunPtr`. Written concretely it does
    // not, because the body normalises `D::V<'a>` to `Val`.
    fn tick<'a, D: DataT>(cv: Cv<'a, D>) -> ValXs<'a, D::V<'a>> {
        if past_deadline() {
            return box_once(Err(Exn::from(Error::str(format!(
                "jq: the query ran for longer than egret's {} ms limit",
                MAX_RUN.as_millis()
            )))));
        }
        box_once(Ok(cv.1))
    }
    jaq_core::native::run((
        TICK,
        jaq_core::native::v(0),
        tick::<D> as jaq_core::RunPtr<D>,
    ))
}

std::thread_local! {
    /// When the jq program on this thread started. Thread-local rather than
    /// shared because [`super::filters::run_jq`] gives every call its own
    /// thread, so there is nothing to synchronise and nothing to reset.
    static RUN_STARTED: std::cell::Cell<Option<std::time::Instant>> =
        const { std::cell::Cell::new(None) };
}

/// Starts the clock [`TICK`] reads. Called on the jq thread.
pub(super) fn start_run() {
    RUN_STARTED.with(|started| started.set(Some(std::time::Instant::now())));
}

fn past_deadline() -> bool {
    RUN_STARTED.with(|started| started.get().is_some_and(|at| at.elapsed() > MAX_RUN))
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
        // Every ROW comes from a probe; the corpus additionally carries the
        // argument-shape probes, whose `e` is not a string.
        let corpus: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../oracle/fixtures/jq-surface.probe.json"))
                .expect("the probe corpus must parse");
        let expressions = corpus
            .iter()
            .filter(|p| p["context"]["e"].is_string())
            .count();
        assert_eq!(
            rows, expressions,
            "the fixture and its probe corpus describe different expression sets"
        );
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

    /// `WITHHELD_NATIVES` and `ALLOWED_NATIVES` must be disjoint and must
    /// together be exactly what the three crates register. A `cargo update`
    /// that adds a native fails here rather than registering it silently,
    /// which is the whole reason this is an allow-list and not a deny-list.
    /// Every recursive construct measured, at the cap and past it.
    ///
    /// **Six of the seven shapes contain no bracket at all**, which is why
    /// the bound is a byte count and not a scanner. This is the shapes, not
    /// the examples a review happened to send.
    fn shapes(n: usize) -> Vec<(&'static str, String)> {
        vec![
            ("unary minus", "-".repeat(n) + "1"),
            ("bracket", "[".repeat(n) + "1" + &"]".repeat(n)),
            ("paren", "(".repeat(n) + "1" + &")".repeat(n)),
            ("object", "{a:".repeat(n) + "1" + &"}".repeat(n)),
            ("try", "try ".repeat(n) + "."),
            ("pipe", ".".to_string() + &"|.".repeat(n)),
            ("comma", ".".to_string() + &",.".repeat(n)),
            ("alternative", ".".to_string() + &" // .".repeat(n)),
            (
                "if",
                "if true then ".repeat(n) + "1" + &" else 1 end".repeat(n),
            ),
        ]
    }

    /// The same shapes, each at the largest size the cap admits — which is a
    /// different `n` per shape, because a `try ` costs four bytes and a `-`
    /// costs one.
    fn shapes_at_the_cap() -> Vec<(&'static str, String)> {
        let mut at_cap = Vec::new();
        for (name, _) in shapes(1) {
            let (mut low, mut high) = (1usize, MAX_SOURCE_BYTES);
            while low + 1 < high {
                let mid = (low + high) / 2;
                let len = shapes(mid)
                    .into_iter()
                    .find(|(n, _)| *n == name)
                    .expect("same list")
                    .1
                    .len();
                if len <= MAX_SOURCE_BYTES {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            let program = shapes(low)
                .into_iter()
                .find(|(n, _)| *n == name)
                .expect("same list")
                .1;
            at_cap.push((name, program));
        }
        at_cap
    }

    /// The cap refuses every shape that can abort, and it IS the arithmetic
    /// in the constants beside it.
    #[test]
    fn the_source_cap_is_the_stack_divided_by_the_worst_measured_cost() {
        assert_eq!(
            MAX_SOURCE_BYTES,
            JQ_STACK_BYTES / WORST_STACK_PER_SOURCE_BYTE / PARSE_STACK_MARGIN
        );
        const {
            assert!(
                MAX_SOURCE_BYTES * WORST_STACK_PER_SOURCE_BYTE * PARSE_STACK_MARGIN
                    <= JQ_STACK_BYTES
            )
        };
        // Derived from the cap, so the test stays right if the cap moves.
        for (name, program) in shapes(MAX_SOURCE_BYTES + 1) {
            let refused = check_source(&program).expect_err("a 4000-deep program is refused");
            assert!(
                refused.0.contains("egret's limit is"),
                "{name}: {refused:?}"
            );
        }
        check_source(".a.b | map(select(.x > 1)) | {y: [.[] | .z]}").expect("must allow");
    }

    /// A source AT the cap parses without aborting, for every shape.
    ///
    /// This is the other half of the derivation and the half a cap alone does
    /// not give: a bound that refuses everything is not evidence that what it
    /// admits is safe. Its observable claim is that these calls RETURN — a
    /// stack overflow is `SIGABRT` and would take the test binary with it.
    #[test]
    fn a_source_at_the_cap_parses_on_the_jq_thread() {
        for (name, program) in shapes_at_the_cap() {
            assert!(program.len() <= MAX_SOURCE_BYTES, "{name}");
            assert!(
                program.len() > MAX_SOURCE_BYTES - 20,
                "{name}: {} is not AT the cap",
                program.len()
            );
            let outcome = render_jq(&program, "[]");
            if let Err(message) = &outcome {
                assert!(
                    !message.contains("egret's limit is"),
                    "{name}: refused at the cap: {message}"
                );
            }
        }
    }

    /// `def` is refused **including inside a string literal**, on purpose.
    ///
    /// The scanner this replaces got its own string tracking wrong: it popped
    /// an interpolation frame on any `)`, so `"\((1),` left it believing it
    /// was inside a string and it stopped counting brackets entirely.
    /// Refusing a `def` that appears anywhere costs one over-refusal and
    /// removes the class.
    #[test]
    fn a_user_def_is_refused_wherever_it_appears() {
        for source in ["def f: f; f", "\"def\"", ". | def g: 1; g", "# def\n.a"] {
            let refused = check_source(source).expect_err("must refuse");
            assert!(refused.0.contains("'def'"), "{source:?}: {refused:?}");
        }
        check_source(".defined").expect("a longer word is not `def`");
        check_source(".undef").expect("a longer word is not `def`");
    }

    #[test]
    fn the_reserved_prefix_is_refused() {
        for source in ["__egret_range(0; 3; 1)", "__egret_tick", "__egret_guard"] {
            let refused = check_source(source).expect_err("must refuse");
            assert!(refused.0.contains("reserves"), "{source}: {refused:?}");
        }
    }

    /// **The bound has to FIRE, not merely be spelled.**
    ///
    /// The two tests this replaces asserted the definitions' NAMES and the
    /// presence of the literal `65536`. `def repeat(f): .;` passes the first
    /// with no bound anywhere, and `rec($n)` instead of `rec($n-1)` passes
    /// the second while the bound never fires. So this RUNS them: every
    /// program below is non-terminating without its bound, and each assertion
    /// names the bound that stopped it.
    #[test]
    fn every_bounded_definition_actually_stops() {
        for (program, needle) in [
            ("[repeat(1)]", "repeat exceeded"),
            // Emits nothing, so the byte counter never sees it; the step
            // bound inside `repeat` is what stops it.
            ("reduce repeat(1) as $x (0; .+1)", "repeat exceeded"),
            ("[while(true; .)]", "while exceeded"),
            ("until(false; .)", "until exceeded"),
            ("[recurse(.)]", "recurse exceeded"),
            ("[range(1e12)]", "range exceeded"),
            // The one shape a STEP COUNT cannot see: a step whose COST
            // grows. 65 536 iterations, each value one element longer than
            // the last, is O(n^2) retained -- 9.9 GiB, nothing emitted, the
            // byte cap never reached. Only a clock sees it.
            ("[recurse(.+[1])] | length", "ran for longer"),
        ] {
            let message =
                render_jq(program, "[]").expect_err(&format!("{program} must be refused"));
            assert!(message.contains(needle), "{program}: {message:?}");
        }
    }

    /// The bounded definitions still answer what a flow writes.
    #[test]
    fn the_bounded_definitions_still_answer_real_programs() {
        for (program, expected) in [
            ("[range(3)]", "[[0,1,2]]"),
            ("[range(1;4)]", "[[1,2,3]]"),
            // A step of 0 emits NOTHING and terminates, which is what
            // Kestra's `jackson-jq` does with it. `jaq`'s own `range` spins
            // forever on `range(0;1;0)`; the guarded definition is what makes
            // these two answers the same rather than a hang and a `[]`.
            ("[range(0;0;0)]", "[[]]"),
            ("[range(0;1;0)]", "[[]]"),
            ("reduce range(0;1;0) as $x (0;.+1)", "[0]"),
            ("[limit(3; repeat(1))]", "[[1,1,1]]"),
            ("[1 | while(. < 4; . + 1)]", "[[1,2,3]]"),
            ("3 | until(. > 5; . + 1)", "[6]"),
            ("[paths]", "[[]]"),
            ("[..] | length", "[1]"),
        ] {
            assert_eq!(
                render_jq(program, "[]").expect(program),
                expected,
                "{program}"
            );
        }
    }

    /// The definitions parse, they are the ones this module means to replace,
    /// and each recursion **decrements** and **calls the guard** — neither of
    /// which is visible in a list of names.
    #[test]
    fn the_bounded_definitions_parse() {
        let names: Vec<&str> = bounded_defs().iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            vec![
                "__egret_guard",
                "repeat",
                "recurse",
                "recurse",
                "recurse",
                "while",
                "until",
                "range",
                "range",
                "range"
            ]
        );
        assert_eq!(BOUNDED_DEFS.matches("rec($n-1)").count(), 4);
        assert_eq!(BOUNDED_DEFS.matches("__egret_guard as $t").count(), 4);
        assert_eq!(BOUNDED_DEFS.matches(TICK).count(), 1);
    }

    /// The literals in the jq source are the constants beside it.
    #[test]
    fn the_literals_in_the_jq_source_are_the_bounds() {
        const { assert!(MAX_STEPS * 2 <= crate::runners::env::MAX_OUTPUT_BYTES) };
        assert_eq!(BOUNDED_DEFS.matches(&MAX_STEPS.to_string()).count(), 5);
        assert_eq!(
            BOUNDED_DEFS.matches(&MAX_VALUE_LENGTH.to_string()).count(),
            1
        );
    }

    /// Renders `{{ v | jq(program) }}` against a JSON input.
    fn render_jq(program: &str, input: &str) -> Result<String, String> {
        let context = format!(
            "{{\"v\": {input}, \"e\": {}}}",
            serde_json::to_string(program).expect("a &str serialises")
        );
        let cx = crate::runners::env::RenderContext::from_json_str(&context)
            .unwrap_or_else(|e| panic!("{program}: context must parse: {e}"));
        crate::runners::env::render_str("{{ v | jq(e) }}", &cx).map_err(|e| e.to_string())
    }

    /// The **definition** list is guarded the way the native list is.
    ///
    /// `ALLOWED_NATIVES` makes a new native a test failure; nothing did the
    /// same for a new *definition*, and `splice` only asserts the names it
    /// replaces. A `cargo update` that adds a recursive definition — the one
    /// thing this module bounds by replacing definitions — would have been
    /// invisible.
    ///
    /// The count is deliberately the assertion: a change in either direction
    /// says "read the new defs.jq and decide whether one of them can loop".
    #[test]
    fn the_definition_list_is_the_one_this_module_was_written_against() {
        let stock: Vec<String> = jaq_core::defs()
            .chain(jaq_std::defs())
            .chain(jaq_json::defs())
            .map(|d| format!("{}/{}", d.name, d.args.len()))
            .collect();
        assert_eq!(
            stock.len(),
            128,
            "jaq's definition list changed size; read the new defs.jq and decide whether any \
             new definition can loop without emitting, then update this count"
        );

        // Every name this module replaces must still be there, at the arity
        // it replaces. `splice` asserts this too, at run time; here it is a
        // named failure rather than a panic inside a render.
        for (name, arity) in REPLACED_DEFS
            .iter()
            .chain(REFUSED_DEFS)
            .chain(KESTRA_DEFS_1)
            .chain(KESTRA_DEFS_2)
        {
            assert!(
                stock.contains(&format!("{name}/{arity}")),
                "jaq no longer defines {name}/{arity}"
            );
        }

        // And the set of definitions whose body names their own definition
        // is committed. Most are ARITY OVERLOADS (`range/2` calling
        // `range/3`), not recursion — telling the two apart needs a call
        // graph, so this asserts the set instead and a NEW member is a human
        // decision: is it an overload, or a loop this module must bound?
        let mut self_naming: Vec<&str> = jaq_core::defs()
            .chain(jaq_std::defs())
            .chain(jaq_json::defs())
            .filter(|d| format!("{:?}", d.body).contains(&format!("\"{}\"", d.name)))
            .map(|d| d.name)
            .collect();
        self_naming.sort_unstable();
        assert_eq!(
            self_naming,
            vec![
                "add",
                "all",
                "all",
                "any",
                "any",
                "capture",
                "combinations",
                "flatten",
                "halt",
                "halt_error",
                "match",
                "range",
                "range",
                "recurse",
                "recurse",
                "scan",
                "splits",
                "sub",
                "test",
            ],
            "a definition now names itself that did not before -- decide whether it is an arity \
             overload or a recursion this module has to bound"
        );
    }

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
