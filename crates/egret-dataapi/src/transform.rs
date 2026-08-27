//! C2: reshaping a Data API response with a small author-supplied script.
//!
//! Task 1 established the *floor*: the evaluator, its budgets, and the tests that
//! pin what a script may not do. Task 3 built the transform on it —
//! [`run_transform`] — and settled the budgets Task 1 left provisional by
//! measuring them.
//!
//! # Why Starlark, and what the sandbox actually rests on
//!
//! A transform is author-supplied code running inside egret's process, so the
//! question is not "which globals did we remember to strip" but "what can the
//! language express at all". Starlark has no `import` statement, no filesystem
//! builtins, no clock and no source of randomness. `db`, `http`, `tx` and
//! `cache` — the spec's stage-1 boundary — are unreachable for the same reason a
//! script cannot reach the clock: there is nothing in scope to reach them with,
//! and this crate injects nothing.
//!
//! That claim is only as good as the test behind it, and it took two attempts to
//! get a test that could actually break. Both failures were found by mutation,
//! not by reading:
//!
//! - **Probing a handful of names pins those names, not the boundary.** Swapping
//!   [`Globals::standard`] for `Globals::extended_internal` left every
//!   name-probe here GREEN while handing scripts `breakpoint`, `call_stack`,
//!   `debug`, `eval_type`, `print`, `pprint` and `json`. `print(__file__)` just
//!   moved on to dying at `__file__` and still looked like a passing probe.
//! - **A snapshot of `Globals::standard()` pins starlark, not us.** The first
//!   fix read that function directly, so it stayed GREEN under the same
//!   mutation. What a script can reach is decided by what [`eval_inner`]
//!   installs, and only a test that goes through [`eval_inner`] can see it.
//!
//! So the boundary is held by two tests together:
//! [`tests::the_global_environment_is_exactly_the_starlark_standard`] snapshots
//! [`transform_globals`] structurally, and
//! [`tests::the_evaluator_resolves_the_snapshot_and_nothing_beyond_it`] asserts
//! the same set through real evaluation — the one that still fails if a future
//! caller bypasses [`transform_globals`] entirely. The name-probes are kept
//! because they are legible, not because they are load-bearing.
//!
//! # What the language does NOT give us
//!
//! 1. **`load()` is part of `Dialect::Standard`.** It is the one construct in
//!    the grammar that names something outside the script. [`transform_dialect`]
//!    turns it off, so `load(...)` does not parse.
//! 2. **There is no built-in timeout, and the tick budget is not one.** A tick is
//!    a call or a loop backedge, so a script can burn seconds in a handful of
//!    ticks: `s = s + s` doubling a string 31 times was measured at **3.2s and
//!    ~2 GiB for about 31 ticks**. Task 3 wires the wall-clock bound
//!    `Evaluator::set_check_cancelled` asks for — see [`MAX_WALL_CLOCK`], which
//!    also records, measured, the case it still cannot stop.
//! 3. **The evaluator can panic on author input.** Not "should never happen" — a
//!    four-line script reaches it. [`eval_inner`] contains it; see there.
//! 4. **Serialising a value recurses, and nothing bounds its depth.** starlark
//!    catches a *cycle*, but `for i in range(200000): x = [x]` costs no budget
//!    this module has and overflows the native stack — an abort, which the
//!    `catch_unwind` in [`eval_inner`] cannot contain. [`MAX_JSON_DEPTH`] is
//!    the answer and [`check_output_shape`] is where it lives.
//!
//! # Where the transform itself is
//!
//! [`run_transform`] — one function, the only one that evaluates anything, the
//! one publish, preview and serving all reach.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use egret_dataapi_api::TransformSpec;
use egret_query_api::Param;
use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect, DialectTypes};
use starlark::values::dict::{AllocDict, DictRef};
use starlark::values::list::ListRef;
use starlark::values::tuple::TupleRef;
use starlark::values::Value;

/// Ceiling on evaluation steps, where a "tick" is one function call or one loop
/// backedge.
///
/// **This bounds work, not time.** Because a tick is a call or a backedge and
/// not an instruction, a script doing enormous work per tick is barely charged:
/// `s = s + s` repeated 31 times inside one loop ran for 3.2 seconds on ~31
/// ticks. [`MAX_WALL_CLOCK`] is the bound denominated in time; this one is the
/// *deterministic* backstop, and that is now its whole job.
///
/// # The determinism is conditional, and the condition is the build
///
/// "The same script gets the same verdict in preview, at publish and in
/// serving" holds only where this budget binds **before** the clock does, and
/// measured that depends on optimisation: 20M ticks is 0.2-0.36 s optimised —
/// inside [`MAX_WALL_CLOCK`], so ticks decide — and ~100 s unoptimised, where
/// the deadline preempts and the verdict becomes a property of the machine.
/// Same shape as the shadowing recorded under [`MAX_WALL_CLOCK`]. Shipped
/// binaries are optimised, so the claim holds where it matters; a debug-build
/// preview would not reproduce a serving refusal, and that is worth knowing
/// before someone debugs one.
///
/// # Measured, not chosen
///
/// Task 1 left this PROVISIONAL at 1 000 000. Task 3 measured, with
/// `Evaluator::get_total_tick_count` on this crate's own evaluator:
///
/// | script | rows | ticks |
/// |---|---|---|
/// | the spec §1 example (index and return) | 100 / 1 000 / 10 000 | **0** |
/// | a per-row reshape (`for r in rows: out.append({...})`) | 100 / 1 000 / 10 000 | **100 / 1 000 / 10 000** |
///
/// So a realistic transform costs **about one tick per row**, and a transform
/// that only re-labels the top level costs *nothing at all* — the tick budget
/// cannot see the spec's own example. Cost per tick, measured on an optimised
/// build (this crate's tests run unoptimised, where starlark is ~500x slower, so
/// the debug number is not the one to set a budget from): **~100M ticks/s** for
/// a tight loop, **~56M ticks/s** when each iteration calls a `def`.
///
/// # Why 20 000 000
///
/// The old 1 000 000 was *below the realistic maximum*, which is the failure the
/// brief warns about: a published endpoint may set `limits.max_rows` as high as
/// a million, and a plain `for r in statements["rows"]` over it costs a million
/// ticks before the script has done anything. A budget an honest script can hit
/// is not a backstop, it is a bug report.
///
/// 20 000 000 is ~2 000x the measured 10 000-row reshape, ~7x the largest honest
/// script the platform's own `max_rows` permits at a few ticks per row, and —
/// measured — 0.2 s to 0.36 s of optimised CPU, so it stays the same order as
/// [`MAX_WALL_CLOCK`] rather than becoming a limit in name only.
const MAX_TICKS: u64 = 20_000_000;

/// Ceiling on the Starlark heap, in bytes.
///
/// **Measured, so that nobody mistakes this for a bound.** starlark checks the
/// heap from `run_infrequent_instr_checks`, which runs once every 1000 ticks and
/// once at the end of an evaluation — never on allocation. Task 3 measured both
/// sides of that with `Heap::peak_allocated_bytes`, against this same 64 MiB
/// limit:
///
/// * **when ticks flow, it is exact.** A loop allocating a small list per
///   iteration was refused at a peak of **67 107 712 bytes** — 64 MiB is
///   67 108 864 — after 280 000 ticks.
/// * **when they do not, it is not a limit at all.** The string-doubling script
///   consumes ~31 ticks, so the check never runs until the loop is over: it
///   reached a peak of **8 589 932 992 bytes — 8.6 GB, a 128x overshoot**. Task 1
///   recorded 32x from a shallower probe; this is the same phenomenon measured
///   further along, and one more doubling panics the evaluator outright (see
///   [`eval_inner`]).
///
/// So: real protection against the runaway growth a loop produces, and no
/// protection whatever against a script that allocates between ticks. The
/// library's own documentation says to use OS-level limits in a subprocess if a
/// hard bound is wanted, and it is right.
///
/// The value is retained from Task 1 rather than re-derived: measured, a
/// 10 000-row per-row reshape peaks at **3 143 232 bytes**, so 64 MiB is ~20x the
/// realistic case, and the number that would actually need raising for a bigger
/// input is [`MAX_TRANSFORM_OUTPUT_BYTES`], not this.
const MAX_HEAP_BYTES: usize = 64 * 1024 * 1024;

/// Ceiling on Starlark call-stack depth.
///
/// starlark bounds recursion on its own — `Evaluator` falls back to a private
/// `DEFAULT_STACK_SIZE` when no limit is set — so the value here is only
/// meaningful if it is *tighter* than that fallback, and a test that merely
/// tries unbounded recursion cannot tell the difference. See
/// `tests::recursion_is_refused_at_our_depth_not_the_library_default`, which
/// establishes the relationship by measurement rather than by quoting a number
/// this crate cannot see.
///
/// 24 frames is generous for a response reshape. **Still unmeasured, and
/// deliberately so:** Task 3 was to settle this against a real transform and
/// found no evidence to settle it with — not one script in the benchmark
/// recursed at all, because a response reshape iterates rather than recurses.
/// A number moved without evidence is not less provisional than one left alone,
/// so it is left alone. The const-assert below is what keeps it honest.
const MAX_CALLSTACK: usize = 24;

/// Recursion depth used by the test that proves [`MAX_CALLSTACK`] is the limit
/// doing the work.
///
/// Deeper than [`MAX_CALLSTACK`] so our budget refuses it, and — verified at test
/// time by actually running it without our budget, not by comparing against a
/// copied constant — shallow enough that starlark's own fallback does not.
///
/// An independent literal, NOT `MAX_CALLSTACK + n`. Deriving it looks tidier and
/// silently kills the assertion below: `x < x + 11` holds for every `x`, so the
/// const assert became a tautology and raising `MAX_CALLSTACK` to 64 sailed
/// straight past it. Measured, not spotted by reading.
const RECURSION_PROBE_DEPTH: usize = 35;

/// Keep the two constants ordered so the guard test can discriminate at all.
///
/// This is NOT "the test would go vacuous" — measured, raising [`MAX_CALLSTACK`]
/// past the probe turns that test RED, which is a perfectly good outcome. What
/// it buys is a build error that *explains itself*, instead of a red test whose
/// message ("MAX_CALLSTACK is not being applied") would misdiagnose a merely
/// too-loose value as a wiring failure.
const _: () = assert!(
    MAX_CALLSTACK < RECURSION_PROBE_DEPTH,
    "RECURSION_PROBE_DEPTH must stay deeper than MAX_CALLSTACK, or the test that \
     proves our call-stack budget is the binding one cannot tell our limit from \
     starlark's own fallback"
);

/// Wall-clock ceiling on a single [`run_transform`] call.
///
/// **The bound [`MAX_TICKS`] is not.** A tick is a call or a loop backedge, so a
/// script that does enormous work per tick is barely charged — Task 1 measured a
/// string-doubling script running 3.24 s on ~31 ticks. This is the only limit in
/// this module denominated in the unit an operator actually cares about.
///
/// # What it bounds, and what it does not — measured, not reasoned
///
/// starlark consults the cancellation callback from
/// `run_infrequent_instr_checks`, which runs once every
/// `INFREQUENT_INSTRUCTION_CHECK_PERIOD` (1000) ticks and once more at the end
/// of each evaluation. So the deadline is enforced at **1000-tick granularity**,
/// and the honest statement of its guarantee is *"no more than 1000 further
/// ticks"*, not *"no more than 2 seconds"*. Three measurements make the
/// difference concrete, all against this 2 s deadline:
///
/// | script | ticks used | stopped after |
/// |---|---|---|
/// | `for i in range(1e9): n = n + 1` | 406 000 | **2.024 s** |
/// | `for i in range(2000): sorted(200k-element list)` | 1 000 | **28.8 s** |
/// | `s = s + s`, 31 times, no loop worth the name | 31 | **6.8 s** |
///
/// The first is the ordinary case and the deadline is tight there — 24 ms of
/// overshoot. The second is the honest limit of this mechanism: 1000 ticks of a
/// script whose every tick is expensive is 1000 sorts, and no callback is
/// consulted in between. The third is refused only because
/// `run_infrequent_instr_checks` also runs *once at the end* of the evaluation —
/// so a tickless script is refused rather than served, but it is not
/// interrupted, and its work is done before anyone asks.
///
/// This is a large improvement on Task 1, where **nothing** bounded time. It is
/// not a latency guarantee, and serving must not treat it as one — a request
/// deadline belongs in the request path. starlark's own documentation says the
/// same: use OS-level limits in a subprocess if a hard bound is required.
///
/// # It covers the whole call, not just the evaluation
///
/// The deadline is taken once in [`eval_inner`] and consulted from four places,
/// because only one of them is starlark's:
///
/// | phase | what polls it |
/// |---|---|
/// | converting the input | [`poll_input_budgets`], every [`INPUT_POLL_ROWS`] rows |
/// | evaluating the module and calling `transform` | starlark, every 1000 ticks |
/// | walking the returned shape | [`check_output_shape`], every [`OUTPUT_POLL_STEPS`] steps |
/// | serialising it | [`CappedSink`], every [`OUTPUT_POLL_STEPS`] writes |
///
/// The first, third and fourth are Rust code that earns **no ticks**, so without
/// those polls a script could spend the whole budget in a phase nothing was
/// watching — which is exactly what a million input rows did before
/// [`poll_input_budgets`] existed (335 MB, unwatched). A script does not get a
/// fresh deadline for its output.
///
/// # It shadows the heap refusal
///
/// `run_infrequent_instr_checks` tests cancellation *first*, then the heap, then
/// ticks. A script that blows [`MAX_HEAP_BYTES`] slowly therefore reports
/// `BudgetExceeded("wall-clock deadline")` rather than the heap. Both are
/// refusals and both name a budget, so this costs precision in the message and
/// nothing else — but it is why
/// `tests::the_heap_budget_refuses_a_script_that_outgrows_it` keeps its probe
/// small enough to finish in milliseconds.
pub(crate) const MAX_WALL_CLOCK: Duration = Duration::from_secs(2);

/// The function every transform script must define.
const ENTRY_POINT: &str = "transform";

/// The number of arguments [`ENTRY_POINT`] must take: `params` and `statements`.
const ENTRY_POINT_ARITY: usize = 2;

/// The function a script must define to build a statement's SQL template.
///
/// A name distinct from [`ENTRY_POINT`] on purpose: a script that builds SQL
/// and a script that reshapes a response are two different contracts called
/// through two different shapes, and [`eval_inner`] tells them apart by which
/// of these two names it is asked to look up — never by guessing from arity.
const BUILD_SQL_ENTRY_POINT: &str = "build_sql";

/// The number of arguments [`BUILD_SQL_ENTRY_POINT`] must take: `params`
/// alone.
///
/// No `statements` — a script building SQL runs *before* any statement has
/// executed, so there are no rows yet to hand it. That is the whole reason
/// this is a separate entry point rather than a `statements=None` case of
/// [`ENTRY_POINT`]: the absence is structural, not optional.
const BUILD_SQL_ENTRY_POINT_ARITY: usize = 1;

/// Which of the crate's two entry-point contracts a script is being held to.
///
/// # Why this is a value rather than three locals
///
/// [`eval_inner`] used to pick `(entry_name, arity, is_build_sql)` apart into
/// three parallel locals, and the two refusals an author actually reads —
/// [`TransformError::MissingEntryPoint`] and [`TransformError::WrongArity`] —
/// took none of them: they rendered `` `transform` `` and `(params,
/// statements)` from constants, so a script that failed to define `build_sql`
/// was told to define `transform`. That was invisible while [`build_sql`] had
/// no non-test caller and became visible the moment publish acquired one.
///
/// Carrying the contract *in the error* is what makes the message impossible
/// to get wrong: there is no second place holding the name, so the name cannot
/// disagree with the lookup that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPoint {
    /// [`ENTRY_POINT`] — an endpoint's response transform.
    Transform,
    /// [`BUILD_SQL_ENTRY_POINT`] — a statement's SQL template.
    BuildSql,
}

impl EntryPoint {
    /// The name [`eval_inner`] looks up in the evaluated module.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Transform => ENTRY_POINT,
            Self::BuildSql => BUILD_SQL_ENTRY_POINT,
        }
    }

    /// How many positional arguments the entry point is called with.
    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Transform => ENTRY_POINT_ARITY,
            Self::BuildSql => BUILD_SQL_ENTRY_POINT_ARITY,
        }
    }

    /// The parameter list an author should have written, spelled as they would
    /// write it — so the refusal is a line they can copy.
    pub(crate) const fn signature(self) -> &'static str {
        match self {
            Self::Transform => "(params, statements)",
            Self::BuildSql => "(params)",
        }
    }
}

/// Ceiling on the transform's response body, measured on the **serialised**
/// bytes rather than on any structure count.
///
/// Serialised, because that is the number that leaves the process, and because
/// no cheaper proxy is honest: a list of 10 elements and a list of 10 megabyte
/// strings have the same length.
///
/// 1 MiB, matching `render::MAX_SQL_BYTES` and `render::MAX_BIND_BYTES` — this
/// crate already draws its "one request may not carry more than this" line
/// there, and a third number would only invite the question of why.
///
/// **This can bite a legitimate endpoint.** A published `limits.max_rows` may be
/// as high as a million, and a transform that passes rows through will exceed a
/// megabyte long before that. That is a deliberate consequence: a transform
/// holds its whole input and its whole output in memory inside the request path,
/// so an endpoint that wants a million rows should not also want a script over
/// them. Publish (Task 5) is where an author should learn this, not serving.
///
/// `pub(crate)` since Task 5: [`crate::lifecycle::probe_transform`] projects a
/// probe's measured per-row growth to the endpoint's own `limits.max_rows` and
/// refuses a publish that cannot fit under this number, so the author meets it
/// while they can still change something. See
/// [`crate::lifecycle::ValidationError::TransformOutputExceedsRowLimit`] for
/// what that projection can and cannot prove.
pub(crate) const MAX_TRANSFORM_OUTPUT_BYTES: usize = 1 << 20;

/// Ceiling on the template a [`build_sql`] script emits, in bytes (spec §5).
///
/// # Measured to a BAND; 65536 is chosen inside it
///
/// Said plainly, because the alternative is a claim of measurement stronger
/// than the evidence. `tests::a_realistic_branching_statement_sizes_the_generated_sql_cap`
/// measures spec §1's shape scaled to an honest analytics query — four optional
/// dimensions, six metrics, two CTEs, three joins, a grain chosen from a dict
/// and two optional filters — at an **1862-byte script emitting 948 bytes**,
/// and holds the cap to `[40x, 100x]` of it: **[37.9 KiB, 94.8 KiB]**.
///
/// That band is what the measurement decides, and it is decided in **both**
/// directions, which is the part a floor alone cannot do. The bottom is C2's
/// ratio — a cap an honest script bumps into refuses work that should have been
/// allowed. The top is C2's own sentence turned around: "a cap an honest script
/// never approaches is not a cap at all", and past it the only thing bounding a
/// generated statement is `render::MAX_SQL_BYTES` again, which is where this
/// started.
///
/// 65536 is then a round number **chosen** inside that band, at ~69x. Nothing
/// measured distinguishes it from 48 KiB or 80 KiB, and this comment does not
/// pretend otherwise. What the test does guarantee is that raising it to
/// 512 KiB or 1 MiB — the realistic future edit, made to unblock one author —
/// goes red.
///
/// # Why not simply reuse 1 MiB, which is this crate's other answer
///
/// [`MAX_TRANSFORM_OUTPUT_BYTES`]'s doc says a third number "would only invite
/// the question of why", so here is why. 1 MiB is `render::MAX_SQL_BYTES`, and
/// that is a **backstop** — the point past which a string has stopped being SQL
/// — reached at render time, after the text has been built, stored and
/// described. It is not a budget an author should be able to sit inside: a
/// statement an engine is asked to *plan* is a few KB, and by the time a
/// generated one is a megabyte the author's mistake is several screens behind
/// them.
///
/// **`MAX_STATEMENTS x 64 KiB` also happens to be 1 MiB, and that is arithmetic
/// rather than a property.** It is recorded here only so the next reader does
/// not mistake it for one: the 16 templates are never rendered as one string,
/// nothing sums them against any limit, and a single 64 KiB template carrying a
/// `{% for %}` can render past 1 MiB on its own — which is precisely the case
/// `render::MAX_SQL_BYTES` exists for. This cap bounds the template; that one
/// bounds the render. Neither contains the other.
///
/// # Where it is enforced, and why there is only one place
///
/// Inside [`eval_inner`]'s [`EntryPoint::BuildSql`] arm, on the value the
/// script returned, before it is copied off the starlark heap. That arm is
/// reachable only through [`build_sql`], and [`build_sql`] is the only
/// generator in the crate — publish
/// ([`crate::lifecycle::GeneratedSql::build_after`], and its sentinel probes),
/// preview (both of those) and per-request serving
/// (`crate::serving::router::generate_one`) all pass through it. Three
/// call-site checks would be three chances to drift; this is one site none of
/// them can go around.
///
/// # What a refusal looks like on each path, including the case publish misses
///
/// On publish and preview it is an author-facing 400
/// ([`crate::lifecycle::ValidationError::StatementScriptFailed`], which carries
/// this rule's own sentence). On serving it is a 500, and that is the right
/// answer rather than an oversight: a script may emit its long template only on
/// a branch no sentinel probe reached, so an endpoint CAN publish green and
/// then fail per request. The caller's request was valid and the ENDPOINT
/// failed — the identical judgement `generate_one` already makes for a
/// generated `DELETE` on an unprobed branch. Nothing else could be true of a
/// text that does not exist until the request supplies its value.
///
/// **The serving case is only ever a branch publish never GENERATED.** A branch
/// publish did generate — every sentinel probe
/// ([`crate::lifecycle::GeneratedSql::probe`]) — is refused there, by name and
/// with this cap's sentence, even though that function otherwise reads a failed
/// evaluation as *inconclusive*. It is the single exception to that rule and it
/// is about evidence: a length refusal means the script ran to completion and
/// this process measured what it returned, so publish is holding the defect
/// rather than guessing at it. Swallowing it was a green publish and a 500 for
/// the first caller to take the branch.
pub(crate) const MAX_GENERATED_SQL_BYTES: usize = 64 * 1024;

/// Ceiling on how deeply JSON may nest, **in both directions**.
///
/// # Why this exists at all, and why it is not decoration
///
/// starlark's `Serialize for Value` recurses structurally. It **does** guard
/// against cycles — measured, not assumed: `x = []` / `x.append(x)` / `return x`
/// comes back as "Cycle detected when serializing value of type `list` to
/// JSON", from a thread-local stack of in-progress values. What it does **not**
/// guard is plain depth, and depth is trivially reachable:
///
/// ```text
/// for i in range(200000): x = [x]
/// ```
///
/// About 200 000 ticks, well inside [`MAX_TICKS`], and a few megabytes, well
/// inside [`MAX_HEAP_BYTES`] — so no budget in this module comes near it. It
/// ends in a **native stack overflow**, which is an abort and not an unwind:
/// [`eval_inner`]'s `catch_unwind` cannot see it and the whole process dies.
/// Measured, with this constant's check removed:
///
/// ```text
/// thread '...' has overflowed its stack
/// fatal runtime error: stack overflow, aborting
/// (signal: 6, SIGABRT: process abort signal)
/// ```
///
/// That is strictly worse than every other failure this module has, and tokio
/// worker threads run on a 2 MiB stack — smaller than the test thread that
/// produced the abort above.
///
/// # Why 64, measured on the stack this actually runs on
///
/// The obvious number is 128 — `serde_json`'s `Deserializer` starts at
/// `remaining_depth: 128`, so 128 is the deepest JSON that can reach this crate
/// at all, and matching it would mean never refusing anything that parsed.
/// **Measured, 128 aborts the process.** One `run_transform` per subprocess,
/// exit codes read directly, unoptimised:
///
/// | thread stack | 64 | 96 | 104 | 128 | 300 |
/// |---|---|---|---|---|---|
/// | **2 MiB** (a tokio worker) | ok | ok | **SIGABRT** | **SIGABRT** | SIGABRT |
/// | 8 MiB (a libtest thread) | ok | ok | ok | ok | **SIGABRT** |
///
/// About 20 KB of stack per level, and serving runs on the 2 MiB row. So the
/// safe ceiling on the stack that matters is ~96, and 64 is that with a 1.5x
/// margin — in the *fattest* build, which is the conservative direction.
///
/// # The cost, stated rather than buried
///
/// This constant is applied to the **input** in [`json_to_starlark`] as well as
/// to the output in [`check_output_shape`], because a value that arrives has to
/// be able to leave. The consequence is real: **engine JSON nested 65-128 deep
/// is refused by an endpoint that has a transform, and served by one that does
/// not.** That is a genuine narrowing, and it is the right way round — the
/// alternative measured above is not "serve it", it is "abort the worker". The
/// refusal is a clear `BudgetExceeded("input nesting depth")`, and a `jsonb`
/// column 65 levels deep is the only thing that can reach it.
///
/// `transform_tests::our_depth_limit_survives_a_tokio_sized_stack` is the guard:
/// raise this constant past what the stack can take and that test does not go
/// red, it takes the suite out with SIGABRT — which is exactly the signal, since
/// that is what production would do.
const MAX_JSON_DEPTH: usize = 64;

/// Every budget one evaluation runs under, in one value.
///
/// # Why this is a struct and not four arguments
///
/// Task 1 already needed one budget to be overridable — `max_callstack`, so that
/// `tests::recursion_is_refused_at_our_depth_not_the_library_default` could run
/// the same probe with and without it and prove which limit was binding. Task 3
/// needs the same of the tick budget, because installing a wall-clock deadline
/// makes the deadline fire first on any tick-dense script and the tick arm of
/// [`classify`] would otherwise have nothing left that could reach it.
///
/// # Non-test code cannot express a weakened budget
///
/// The fields are private and **every mutator is `#[cfg(test)]`**, so the only
/// `Budgets` a shipped binary can build is [`Budgets::default`] — the module
/// constants. That is deliberate: a relaxation that is unrepresentable is better
/// than one a reviewer has to remember to look for.
#[derive(Clone, Copy)]
struct Budgets {
    ticks: u64,
    /// `None` means "install nothing of ours and let starlark fall back to its
    /// own private `DEFAULT_STACK_SIZE`". Only tests pass `None`.
    callstack: Option<usize>,
    wall_clock: Duration,
    /// A SECOND deadline, taken fresh when the output phase begins.
    ///
    /// `None` — the only value a shipped binary can produce — means the output
    /// phase shares the one deadline taken at the top of [`eval_inner`], which
    /// is the production rule and the one `run_transform` argues for: a script
    /// does not get a fresh two seconds for its output.
    ///
    /// # Why the seam exists, measured
    ///
    /// One deadline across three phases means the EARLIER phase can spend it,
    /// and then the later phase's own guard is unreachable — not because it is
    /// missing, but because nothing gets that far. That is correct in
    /// production and fatal in a test that names a specific guard, and it is
    /// not hypothetical: with 8 spinner processes on 8 cores,
    /// `an_absurdly_deep_return_is_refused_rather_than_killing_the_process`,
    /// `an_oversized_output_is_refused_with_both_numbers` and
    /// `a_wide_shared_fan_out_is_stopped_by_the_output_deadline` failed **3 runs
    /// out of 3**, all three reporting a `"wall-clock deadline"` raised by the
    /// phase BEFORE the one under test. Idle, all three passed.
    ///
    /// The fix a reviewer will reach for first — relax the assertion to
    /// `matches!(err, BudgetExceeded(_))` — is the wrong one and was measured to
    /// be: see the comment in
    /// `a_wide_shared_fan_out_is_stopped_by_the_output_deadline`, where that
    /// version passed **with the poll it exists to pin deleted**. So the tests
    /// keep their exact assertions and get a budget in which only the phase
    /// under test can overrun.
    output_wall_clock: Option<Duration>,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets {
            ticks: MAX_TICKS,
            callstack: Some(MAX_CALLSTACK),
            wall_clock: MAX_WALL_CLOCK,
            // One deadline across every phase. See the field.
            output_wall_clock: None,
        }
    }
}

#[cfg(test)]
impl Budgets {
    fn with_ticks(mut self, ticks: u64) -> Self {
        self.ticks = ticks;
        self
    }

    fn without_callstack_limit(mut self) -> Self {
        self.callstack = None;
        self
    }

    /// Enlarge the deadline the EVALUATION runs under.
    ///
    /// For a test whose subject is a guard in the OUTPUT phase and whose script
    /// is slow to evaluate: without this the evaluation eats the budget on a
    /// contended machine and the guard under test never runs.
    fn with_wall_clock(mut self, wall_clock: Duration) -> Self {
        self.wall_clock = wall_clock;
        self
    }

    /// Give the OUTPUT phase a deadline of its own, taken when it begins.
    ///
    /// Always paired with [`Budgets::with_wall_clock`]: enlarging the evaluation
    /// deadline alone would enlarge the output phase's too, since they are the
    /// same instant.
    fn with_output_wall_clock(mut self, output_wall_clock: Duration) -> Self {
        self.output_wall_clock = Some(output_wall_clock);
        self
    }
}

/// The dialect a transform is parsed under.
///
/// Every field is written out rather than using `..Dialect::Standard`. `Dialect`
/// is effectively `#[non_exhaustive]` (it carries a `_non_exhaustive: ()` field
/// for exactly that purpose), so a struct-update would silently adopt whatever a
/// future starlark adds — including a future opt-out that defaults to permissive.
/// Spelling the fields out means a new field is a COMPILE ERROR here, and
/// someone has to decide.
///
/// The one departure from `Dialect::Standard` is `enable_load: false`. `load()`
/// is the only construct in standard Starlark that refers to something outside
/// the script's own text, and `Standard` leaves it ON; leaving it on would make
/// the sandbox depend on nobody ever calling `Evaluator::set_loader` — a promise
/// about future code rather than a property of the parse.
fn transform_dialect() -> Dialect {
    Dialect {
        enable_def: true,
        enable_lambda: true,
        // The one departure from `Dialect::Standard`. See above.
        enable_load: false,
        enable_keyword_only_arguments: false,
        enable_positional_only_arguments: false,
        enable_types: DialectTypes::Disable,
        // Moot while `enable_load` is false, and set to match.
        enable_load_reexport: false,
        enable_top_level_stmt: false,
        enable_f_strings: false,
        _non_exhaustive: (),
    }
}

/// The global environment a transform is evaluated in.
///
/// `Globals::standard()` and nothing else — no `LibraryExtension`, no injected
/// values. Extracted into a function so that the set a transform can name is a
/// thing with a NAME, and therefore a thing a test can read; inlined at the call
/// site it was only a expression buried in [`eval_inner`], and the test that
/// claimed to pin it was really pinning `Globals::standard()` itself.
///
/// `Globals::extended_internal()` is the tempting upgrade and must not be taken
/// casually: it adds 22 names including `print`, `pprint`, `debug`,
/// `breakpoint`, `call_stack`, `eval_type` and `starlark_rust_internal`.
fn transform_globals() -> Globals {
    Globals::standard()
}

/// What can go wrong when a transform runs.
///
/// Deliberately minimal: Task 3 elaborates this as the transform grows a
/// contract. The arms exist now because the distinctions they draw are already
/// real — a script that does not parse, a script that parses and then
/// misbehaves, a script cut off for exceeding a budget, and a script that broke
/// the evaluator are four different things to tell an endpoint author, and only
/// the last is a bug report rather than feedback.
// Scaffolding for Task 3, which gives this module its first non-test caller.
// Until then the whole chain — the budgets, the dialect, `classify` — is reached
// only from `#[cfg(test)]`, so the non-test build sees it as dead. Marking the
// two roots keeps that from becoming a standing warning the crate learns to
// ignore, and both attributes come off as soon as the transform has a caller.
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum TransformError {
    /// The script is not valid Starlark under [`transform_dialect`].
    #[error("the transform script did not parse: {0}")]
    Parse(String),
    /// The script parsed and ran, but never defined a callable named
    /// [`EntryPoint::name`] for the contract it was evaluated under.
    ///
    /// Also what a *non-callable* entry point produces — `transform = 1` leaves
    /// the endpoint with no transform function just as surely as omitting it,
    /// and the author's next move is the same line either way.
    ///
    /// **The contract is carried, not assumed.** Both entry points reach this
    /// arm, and until package S wired a caller onto [`build_sql`] the message
    /// named `` `transform` `` unconditionally — so an author whose statement
    /// script was missing `build_sql` was told to define a function this phase
    /// never looks up. See [`EntryPoint`].
    #[error("the script must define `{}{}`", .0.name(), .0.signature())]
    MissingEntryPoint(EntryPoint),
    /// The entry point exists but does not take the arguments it is called
    /// with.
    ///
    /// Separate from [`TransformError::MissingEntryPoint`] deliberately: "you
    /// named it wrong" and "you took the wrong arguments" send an author to
    /// different lines. Without the distinction both arrive as a generic
    /// call-failed message from starlark.
    ///
    /// Carries its [`EntryPoint`] for the same reason
    /// [`TransformError::MissingEntryPoint`] does — a wrong-arity `build_sql`
    /// must be told about `build_sql(params)`, not about
    /// `transform(params, statements)`.
    #[error(
        "`{}` must take exactly {} arguments {}, but takes {found}",
        .entry.name(), .entry.arity(), .entry.signature()
    )]
    WrongArity {
        /// Which contract's signature the script failed to match.
        entry: EntryPoint,
        /// How many parameters the script's entry point actually declares.
        found: usize,
    },
    /// [`BUILD_SQL_ENTRY_POINT`] ran to completion but returned something
    /// other than a string.
    ///
    /// A refusal rather than a coercion: the whole safety argument for a
    /// build-SQL script is that its template goes through the same
    /// `render()` a hand-written statement's SQL does, so stringifying a
    /// dict or a list here would hand the renderer text that was never meant
    /// to be a template, and the failure it caused would surface far from
    /// this, its real cause. Carries what came back, spelled the way
    /// [`starlark::values::Value::get_type`] spells it — `"dict"`, `"list"`,
    /// `"int"` — so an author reads the kind of their mistake rather than a
    /// dump of the value.
    #[error("build_sql must return a string template, but returned a {0}")]
    BuildSqlNotAString(String),
    /// [`BUILD_SQL_ENTRY_POINT`] returned a string longer than
    /// [`MAX_GENERATED_SQL_BYTES`].
    ///
    /// Carries **both** numbers, for the reason
    /// [`TransformError::OutputTooLarge`] does: "too large" without the size
    /// tells an author nothing about how much to cut. Exact rather than "at
    /// least" — the template is one string already in hand, so counting it
    /// costs nothing and promises nothing this cannot keep.
    ///
    /// **The message names no parameter and no value.** It reaches a caller
    /// only through `ServingError::Internal`, which does not surface its
    /// source, but the rule this crate holds itself to is that a serving
    /// refusal never spells out an endpoint's own shape — so this sentence is
    /// safe wherever it lands.
    #[error("build_sql returned a {bytes} byte template, over the {cap} byte cap")]
    GeneratedSqlTooLarge {
        /// The emitted template's length in bytes.
        bytes: usize,
        /// [`MAX_GENERATED_SQL_BYTES`], repeated here so the message is
        /// self-contained.
        cap: usize,
    },
    /// The script parsed but failed while running — an unbound name, a type
    /// error, an explicit `fail()`.
    #[error("the transform script failed while running: {0}")]
    Runtime(String),
    /// The returned value has no JSON counterpart — a function, a `range`, a
    /// dict keyed by something that is not a string, a NaN.
    ///
    /// An error rather than a lossy conversion: dropping the offending member
    /// would serve a response missing a field the script demonstrably produced,
    /// and the endpoint's declared `returns` would be a lie about it.
    #[error("the transform returned a value that is not JSON: {0}")]
    NotSerialisable(String),
    /// The serialised response exceeds [`MAX_TRANSFORM_OUTPUT_BYTES`].
    ///
    /// Carries **both** numbers. "Too large" without the size tells an author
    /// nothing about how much to cut.
    #[error("the transform returned at least {bytes} bytes, over the {cap} byte cap")]
    OutputTooLarge {
        /// The serialised size, counted even though only `cap` bytes of it were
        /// ever retained.
        ///
        /// Exact up to [`OUTPUT_COUNT_CEILING`] and a lower bound beyond it —
        /// hence "at least" in the message. A return value can serialise to
        /// exponentially more bytes than it occupies, so counting to the end is
        /// not something this is allowed to promise.
        bytes: usize,
        /// [`MAX_TRANSFORM_OUTPUT_BYTES`], repeated here so the message is
        /// self-contained.
        cap: usize,
    },
    /// The script was cut off for exceeding one of this module's budgets. The
    /// payload names which one.
    #[error("the transform script exceeded its {0}")]
    BudgetExceeded(&'static str),
    /// The evaluator itself panicked and the unwind was caught at this module's
    /// boundary. Distinct from [`TransformError::Runtime`] on purpose: this is a
    /// defect in starlark reached through author input, not a mistake in the
    /// author's script, and an operator needs to be able to tell them apart.
    #[error("the transform evaluator failed: {0}")]
    Evaluator(String),
}

/// Parse and evaluate `src` under the transform sandbox, discarding the value.
///
/// The evaluation surface the tests measure, and the one Task 3 builds the real
/// transform on top of. It installs every limit `starlark 0.14` offers and
/// injects **nothing** — no globals beyond [`Globals::standard`], no loader, no
/// extension values.
///
/// # Task 4's caller, and why this function and not a second parse
///
/// `pub(crate)` since Task 4: `lifecycle::check_transform` calls this at
/// publish to refuse a script that cannot be honoured, and it is the ONLY
/// evaluation this performs — it parses the script and runs its module top
/// level (defining `transform`, evaluating nothing else), and never calls
/// `transform(params, statements)`. That is deliberate: publish may refuse a
/// script, it may not run one against data — probing it against `returns` is a
/// later task's job. Calling through here rather than a second parse site is
/// what keeps `tests::there_is_exactly_one_place_that_parses_starlark`
/// meaningful: publish and serving agree about which scripts exist because
/// they go through the same parse.
pub(crate) fn eval_probe(src: &str) -> Result<(), TransformError> {
    eval_inner(src, Budgets::default(), None).map(|_| ())
}

/// The arguments [`ENTRY_POINT`] is called with.
///
/// A struct rather than two parameters on [`eval_inner`] so that "evaluate the
/// module and stop" and "evaluate the module and then call the entry point" are
/// one function with one argument that is present or absent, instead of two
/// functions that would each have to remember to install the budgets.
struct TransformCall<'a> {
    params: &'a BTreeMap<String, serde_json::Value>,
    statements: &'a BTreeMap<String, Vec<serde_json::Value>>,
}

/// What [`eval_inner`] does once the module's top level has run.
///
/// `None` — a probe — evaluates the module and stops. `Some` names which of
/// the crate's two entry-point contracts to call: [`ENTRY_POINT`] with a
/// row-shaped [`TransformCall`], or [`BUILD_SQL_ENTRY_POINT`] with `params`
/// alone. One enum rather than a `statements: Option<_>` on [`TransformCall`]
/// because a build-SQL script categorically cannot see rows — Starlark has no
/// way to express "the second argument does not exist", so the type doing the
/// refusing is the one honest way to keep a script from being handed
/// `statements` on the phase that runs before any statement has.
enum Entry<'a> {
    Transform(TransformCall<'a>),
    BuildSql { params: &'a GenerationParams },
}

/// The `params` dict a `build_sql` script is handed — on **every** path.
///
/// # Why this is a type and not a `BTreeMap`
///
/// Publish and serving must not be able to disagree about the type a generating
/// script sees. They used to: publish built the dict from
/// [`crate::lifecycle::representative_value`], one scalar per declared
/// parameter, while serving reused [`crate::serving::params::Resolved::json`],
/// whose shape follows the endpoint's frozen `iterated` set and the caller's
/// arity — so a parameter any statement loops over, or one a caller simply
/// repeated, reached the script as a one-element `Value::Array` where publish
/// had shown the author a string. `params["region"] == "ALL"` was then true for
/// the author and false for every caller, and the script silently took the
/// other branch: a 200 with the wrong rows, both texts legitimate `SELECT`s, so
/// every scan and both serve-time gates passed.
///
/// The fix is the constructor's signature rather than a rule two call sites
/// have to remember. [`GenerationParams::insert`] takes **at most one**
/// [`Param`], and `Param` is a scalar enum with no list case — so "hand a
/// generating script a list" is not something any caller can express, and
/// [`build_sql`] takes this type rather than a map so no caller can route
/// around it. `Resolved::json` no longer type-checks at either generation site.
///
/// # What that costs, stated rather than implied
///
/// A script cannot see how many values a caller supplied for one parameter. It
/// gets the first, which is exactly the shape publish probed it with, and the
/// RENDER still receives every value — `{% for r in region %}` over an iterated
/// parameter binds each repeat, unchanged (`serving::params::resolve`'s
/// `as_list`). A script that decides structure from the multiplicity is
/// therefore not expressible; a script that emits the loop and lets the
/// renderer bind the list — the only shape the design asks for (spec §4.3) —
/// is unaffected. The alternative, refusing the repeat outright, would take
/// `IN (?, ?, ?)` away from every scripted endpoint to close a case the
/// renderer already answers.
///
/// # And what it buys, in spec terms
///
/// §8's tripwire is "if a script's SQL can depend on anything other than
/// `params`, §3 collapses". Following the shape decision would have made a
/// script's SQL depend on the frozen `iterated` set — a set derived FROM the
/// statements' SQL, which for a scripted statement does not exist until
/// generation has already run. This shape depends on nothing but the declared
/// parameter list and the values supplied for it, which both sides hold before
/// generation, so the ordering problem does not arise at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GenerationParams(BTreeMap<String, serde_json::Value>);

impl GenerationParams {
    /// Declare `name`, carrying `value` — or Starlark `None` when there is none.
    ///
    /// `Option<&Param>` rather than a JSON value, and that is the whole point of
    /// the type: `Param` cannot be a list, and `None` is the absent case the
    /// renderer draws the same distinction for (`ParamValues::insert_absent`).
    /// A declared name is always present in the dict, so a script reading
    /// `params["region"]` gets an answer rather than a KeyError.
    pub(crate) fn insert(&mut self, name: String, value: Option<&Param>) {
        self.0
            .insert(name, value.map_or(serde_json::Value::Null, param_json));
    }

    /// This dict with `name` replaced — what
    /// [`crate::lifecycle::GeneratedSql::probe`] substitutes into.
    ///
    /// A copy rather than a mutation because the probes all vary ONE parameter
    /// from the same base, and a base that could be left mutated is a probe
    /// whose meaning depends on the order they ran in.
    pub(crate) fn with(&self, name: &str, value: Option<&Param>) -> Self {
        let mut out = self.clone();
        out.insert(name.to_owned(), value);
        out
    }
}

/// Convert `params` into a starlark dict on `heap`, polling the input budgets
/// once afterward.
///
/// The one conversion both [`Entry`] variants need — [`run_transform`]'s
/// `params` and [`build_sql`]'s are the same shape — factored out so there is
/// one place that remembers to poll [`poll_input_budgets`], not two.
fn alloc_params<'v>(
    params: &BTreeMap<String, serde_json::Value>,
    heap: starlark::values::Heap<'v>,
    deadline: Instant,
) -> Result<Value<'v>, TransformError> {
    let mut items = Vec::with_capacity(params.len());
    for (name, value) in params {
        items.push((heap.alloc(name.as_str()), json_to_starlark(value, heap, 0)?));
    }
    poll_input_budgets(heap, deadline)?;
    Ok(heap.alloc(AllocDict(items)))
}

/// Run an endpoint's response transform.
///
/// **This is the single evaluator.** Publish, preview and serving all call this
/// one function; there is no second path that evaluates a script, and the test
/// `tests::there_is_exactly_one_place_that_parses_starlark` is what keeps it
/// that way — a second evaluator would need a second parse, and that test fails
/// on the second parse site rather than on the second evaluator, which is the
/// cheaper thing to notice.
///
/// # The contract a script is held to
///
/// * it defines `transform(params, statements)` — [`ENTRY_POINT`], exactly
///   [`ENTRY_POINT_ARITY`] arguments;
/// * `params` is a dict of the endpoint's bound parameters, `statements` a dict
///   from statement name to its list of rows. **Keyed even at N = 1**: a script
///   that has to be rewritten when the author adds a second statement is a trap,
///   so the shape does not change with the count;
/// * whatever it returns must be JSON, must serialise to at most
///   [`MAX_TRANSFORM_OUTPUT_BYTES`], and must nest at most [`MAX_JSON_DEPTH`]
///   deep.
///
/// # What bounds it
///
/// Every limit this module has, all installed on one evaluator:
/// [`MAX_TICKS`], [`MAX_HEAP_BYTES`], [`MAX_CALLSTACK`] and — new in this task
/// and the only one denominated in time — [`MAX_WALL_CLOCK`]. The deadline is
/// taken **once, here**, and covers the module evaluation and the call to
/// `transform` together, because an author cannot be expected to care which half
/// of their script was slow.
///
/// Three phases, only one of which starlark watches, and what covers the other
/// two:
///
/// * **converting `params` and `statements` into starlark values** happens in
///   Rust, consumes no ticks, and so trips nothing starlark checks. Measured, a
///   million rows peaked at 335 MB against a 64 MiB limit before anything
///   noticed. [`poll_input_budgets`] covers it: the deadline and the heap, every
///   [`INPUT_POLL_ROWS`] rows.
/// * **evaluating** — starlark's own budgets, checked every 1000 ticks.
/// * **walking and serialising the result** likewise earns no ticks.
///   [`check_output_shape`] bounds the stack ([`MAX_JSON_DEPTH`]) and the
///   expanded value count before a byte is written, [`CappedSink`] bounds the
///   bytes, and **both poll the same deadline** every [`OUTPUT_POLL_STEPS`]
///   steps. Without that last part the output path was outside every budget:
///   `MAX_HEAP_BYTES` permits millions of nodes, so a walk over them is bounded
///   but not quick, and a script would have got a fresh unbounded phase after
///   its evaluation was cut off.
///
/// And one that is genuinely not bounded, stated rather than implied: **this is
/// not a latency guarantee.** [`MAX_WALL_CLOCK`] is checked at 1000-tick
/// granularity, so a script whose every tick is expensive overruns it — measured
/// at 28.8 s against a 2 s deadline. A request deadline belongs in the request
/// path; serving must not read this function as one.
pub(crate) fn run_transform(
    spec: &TransformSpec,
    params: &BTreeMap<String, serde_json::Value>,
    statements: &BTreeMap<String, Vec<serde_json::Value>>,
) -> Result<serde_json::Value, TransformError> {
    eval_inner(
        &spec.script,
        Budgets::default(),
        Some(Entry::Transform(TransformCall { params, statements })),
    )
}

/// Run a statement's SQL-building script and return the template it built.
///
/// **The third entry point onto [`eval_inner`]**, alongside [`eval_probe`]
/// (module top level only) and [`run_transform`] (`transform(params,
/// statements)`). Same evaluator, same budgets, same globals, same dialect,
/// same panic containment — only the call shape differs, via [`Entry`]. A
/// second evaluator here would need a second parse, and
/// `tests::there_is_exactly_one_place_that_parses_starlark` is what would
/// catch that.
///
/// # What comes back is a template, never finished SQL
///
/// The script defines `build_sql(params)` and returns a string; whatever it
/// returns is handed back **verbatim** — `{{ region }}` survives exactly as
/// written. That is the whole safety argument for a build-SQL statement: the
/// path a caller's bound value takes to the engine is unchanged, because the
/// text this returns still goes through the same `render()` that runs a
/// hand-written statement's SQL. This function never renders anything itself.
///
/// # The script sees `params` and nothing else
///
/// And it sees them as [`GenerationParams`], which is the type that stops
/// publish and serving handing the same script different types for the same
/// parameter — see there.
///
/// No `statements` — a build-SQL script runs *before* any statement has
/// executed, so there is nothing to hand it, and [`Entry::BuildSql`] has no
/// field to hand it even by mistake. No `db`, `http`, `tx`, `cache`, clock or
/// randomness either: the globals are [`transform_globals`], unchanged from
/// [`run_transform`]'s, and the dialect is [`transform_dialect`], likewise
/// unchanged.
///
/// # Why a non-string return is refused, not stringified
///
/// A script returning a dict or a list is an author mistake. Coercing it to
/// text would hand the renderer something that was never meant to be SQL and
/// let the failure surface far from its cause — see
/// [`TransformError::BuildSqlNotAString`].
///
/// # The one thing this checks about the template, and where every caller
/// inherits it
///
/// Its LENGTH, against [`MAX_GENERATED_SQL_BYTES`] — nothing else. This
/// function does not scan the text, and must not: a byte scanner guarding a
/// template language is the mistake P4.5 paid for five times over, which is why
/// the identifier rule is a behavioural probe
/// ([`crate::lifecycle::GeneratedSql::probe`]) and the head-word and
/// comparison rules are re-applied by their own owners
/// (`crate::serving::router::generate_one`).
///
/// The length is different in kind: it is a fact about the string rather than a
/// judgement about its content, so it is decided once, here, by the only
/// generator the crate has. Publish, preview and serving reach generation
/// through this function, so none of them has to remember the cap and none of
/// them can drift off it.
pub(crate) fn build_sql(script: &str, params: &GenerationParams) -> Result<String, TransformError> {
    match eval_inner(script, Budgets::default(), Some(Entry::BuildSql { params }))? {
        serde_json::Value::String(s) => Ok(s),
        // `eval_inner`'s `Entry::BuildSql` arm only ever returns `Ok` with a
        // `Value::String` — anything else it converts to
        // `TransformError::BuildSqlNotAString` before returning. Reachable
        // only if that invariant breaks.
        other => {
            unreachable!("eval_inner's BuildSql arm must return a JSON string on Ok, got {other:?}")
        }
    }
}

/// Does `value` satisfy the endpoint's declared `returns`?
///
/// **One rule, two callers.** Publish runs it against synthetic probe rows
/// ([`crate::lifecycle::probe_transform`]); serving runs it against the real
/// response before that response reaches a caller (Task 6). It lives here, next
/// to the single evaluator, for the same reason `eval_probe` does: a second
/// implementation of "does this match `returns`" could accept at publish what
/// serving refuses, and the endpoint would then publish green and 500 in
/// production — the exact failure the declaration exists to prevent.
///
/// # What is checked, and what deliberately is not
///
/// * the value is a JSON **object** — `returns` is a list of named fields, and
///   a list or a scalar has no field to match one;
/// * every `required` field is present — M1's flag, unchanged: `required` means
///   "present in every permutation", so a `required: false` field may be
///   absent;
/// * no field is present that `returns` does not declare — an undeclared field
///   is a response the published contract (and the generated OpenAPI document
///   built from `returns`) does not describe.
///
/// **Types are not checked.** `ColumnSpec.ty` is whatever the engine called the
/// column — `bigint`, `varchar(10)`, `timestamp(3)` — and a Starlark value has
/// no engine type to compare it against. `openapi_gen::json_type` maps such a
/// string onto one of four JSON types *conservatively*, mapping everything it
/// does not recognise to `string`; enforcing that mapping here would refuse a
/// script returning a number for a `decimal` column (JSON-typed `number`, but
/// the engine spelling is not in the list) while accepting anything at all for
/// the many types that fall through to `string`. That is a rule that refuses
/// honest scripts and passes dishonest ones, so this checks the question it can
/// answer exactly — which fields are there — and leaves the type question to
/// the author's own declaration.
///
/// The detail is a message an author reads, and it names **every** problem
/// rather than the first: fixing a missing field only to be told about an
/// undeclared one is two publish attempts for one edit.
pub(crate) fn check_returns(
    value: &serde_json::Value,
    returns: &[egret_dataapi_api::ColumnSpec],
) -> Result<(), String> {
    let serde_json::Value::Object(fields) = value else {
        return Err(format!(
            "the transform returned {}, but `returns` declares named fields, which only a \
             JSON object has",
            json_kind(value)
        ));
    };

    let mut problems: Vec<String> = Vec::new();

    let missing: Vec<&str> = returns
        .iter()
        .filter(|c| c.required && !fields.contains_key(&c.name))
        .map(|c| c.name.as_str())
        .collect();
    if !missing.is_empty() {
        problems.push(format!(
            "it does not return the declared field(s) {}",
            quoted_list(&missing)
        ));
    }

    // Through a SET, not `returns.iter().any(...)` per field. This function
    // runs against every response of every request, and the linear form was
    // O(fields x returns) — with `returns` unbounded until
    // `lifecycle::MAX_TRANSFORM_RETURNS` and `fields` bounded only by
    // `MAX_TRANSFORM_OUTPUT_BYTES`, that product was a request-path cost nobody
    // could name. One pass to build, one lookup per field.
    let declared: HashSet<&str> = returns.iter().map(|c| c.name.as_str()).collect();
    // Sorted, so the message does not depend on `serde_json::Map`'s ordering —
    // which is BTree today and insertion-ordered under `preserve_order`, i.e. a
    // feature flag away from making this text depend on the script's key order.
    let mut undeclared: Vec<&str> = fields
        .keys()
        .filter(|k| !declared.contains(k.as_str()))
        .map(String::as_str)
        .collect();
    undeclared.sort_unstable();
    if !undeclared.is_empty() {
        problems.push(format!(
            "it returns the undeclared field(s) {}",
            quoted_list(&undeclared)
        ));
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; and "))
    }
}

/// One [`egret_query_api::Param`] as JSON — what a bound parameter looks like
/// to a script.
///
/// **One conversion, two callers**, for the same reason [`check_returns`] is one
/// rule: `lifecycle::probe_params` builds publish's `params` dict from this, and
/// `serving::params::resolve_supplied_with` builds the serving one from it. A
/// second reading would let publish probe a script with a JSON number for a
/// `decimal` the serving path hands over as a string — a script that passes the
/// probe and fails on every real call, with nothing naming the difference.
///
/// Total over the enum rather than over the three variants
/// `representative_value` can produce today: a new `ParamType` mapping to a
/// new `Param` would otherwise reach a script as whatever the fallback arm
/// happened to be.
pub(crate) fn param_json(p: &egret_query_api::Param) -> serde_json::Value {
    use egret_query_api::Param;
    match p {
        Param::Null => serde_json::Value::Null,
        Param::Bool(b) => serde_json::Value::Bool(*b),
        Param::I64(i) => serde_json::Value::from(*i),
        // A non-finite float has no JSON counterpart; `representative_value`
        // never produces one, and `Null` is the honest answer if it ever does.
        Param::F64(f) => serde_json::Number::from_f64(*f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        // Decimal stays TEXT, like everywhere else in this crate: turning it
        // into a JSON number is exactly the float rounding the variant exists to
        // avoid.
        Param::Decimal(s) | Param::Str(s) | Param::Date(s) | Param::Timestamp(s) => {
            serde_json::Value::from(s.as_str())
        }
    }
}

/// What a JSON value is, in the words an author's error message uses.
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "`None`",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "a list",
        serde_json::Value::Object(_) => "an object",
    }
}

/// `` `a`, `b` `` — names an author can paste back into their script.
fn quoted_list(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A script source that makes [`eval_inner`] panic from *inside* the caught
/// region, so the containment can be tested without the 4 GiB it costs to reach
/// starlark's own panic.
///
/// `#[cfg(test)]`, so neither this nor the branch that reads it exists in a
/// shipped binary. It is a bare identifier, i.e. valid Starlark, so the parse
/// ahead of the injection point is the same parse any other script gets.
#[cfg(test)]
const PANIC_INJECTION_SRC: &str = "__egret_transform_panic_probe__";

/// The message [`PANIC_INJECTION_SRC`] panics with. The test asserts the caller
/// receives it **verbatim**, which is what makes a panic actionable instead of
/// merely reported.
#[cfg(test)]
const PANIC_INJECTION_MESSAGE: &str = "injected panic: transform containment probe";

/// The one evaluation: parse, install every budget, run the module, and — when
/// `call` is present — call [`ENTRY_POINT`] and convert what it returned.
///
/// Both [`eval_probe`] and [`run_transform`] come through here, which is the
/// point: a second entry point would be a second place to forget a budget.
/// `budgets` is [`Budgets::default`] everywhere outside `#[cfg(test)]` — see
/// [`Budgets`] for why that is enforced by construction rather than by review.
///
/// # Why the panic is caught here
///
/// starlark can panic on author input. `str_type.rs:148` asserts
/// `len as u32 as usize == len` before constructing a `StarlarkStr`, so any
/// script building a string of 4 GiB or more aborts the thread with
/// `len overflow`. Four lines of Starlark reach it, and no budget prevents it:
/// the heap limit is checked periodically and was measured overshooting 64 MiB
/// by 32x before firing.
///
/// Left uncaught, that unwind leaves this module as a panic. There is no net
/// below: `tower-http` is configured in the workspace manifest with
/// `features = ["trace", "cors"]`, `catch-panic` is not among them, and no
/// `CatchPanicLayer` exists anywhere under `crates/`. In the serving path the
/// unwind would take out the connection task — which is not a refusal, carries
/// no message an author could act on, and looks nothing like the 4xx this
/// deserves.
///
/// `catch_unwind` is the right instrument here for four checked reasons:
///
/// - **It works.** No profile in this workspace sets `panic = "abort"` — there
///   is no `[profile.release]` at all, so both profiles unwind.
/// - **The unwind is safe to catch.** The panicking `assert!` guards an
///   `unsafe fn` and fires *before* the truncating construction, so nothing
///   half-built survives it.
/// - **`AssertUnwindSafe` is honest rather than a silencer.** The `AstModule`,
///   the `Module`, its heap and the `Evaluator` are all created inside the
///   caught region and dropped by the unwind. Nothing observable outside this
///   function is mutated, so there is no broken invariant that could leak — the
///   exact condition the assertion exists to express.
/// - **It is not a swallow.** The panic hook still runs, so the panic is logged
///   with its location before this converts it into a value.
fn eval_inner(
    src: &str,
    budgets: Budgets,
    call: Option<Entry<'_>>,
) -> Result<serde_json::Value, TransformError> {
    let ast = AstModule::parse("transform.star", src.to_owned(), &transform_dialect())
        .map_err(|e| TransformError::Parse(e.to_string()))?;

    let globals = transform_globals();

    // Taken once, before anything runs, and consulted by the cancellation
    // callback below. One deadline for the module evaluation and the call
    // together — see `run_transform`.
    let deadline = Instant::now() + budgets.wall_clock;

    #[cfg(test)]
    let inject_panic = src == PANIC_INJECTION_SRC;

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        // `Module::new` does not exist in starlark 0.14 — a module is handed to
        // a closure so its heap is scoped. Everything the evaluator touches
        // therefore lives inside here, which is also what makes the unwind above
        // safe to catch.
        Module::with_temp_heap(|module| {
            let mut eval = Evaluator::new(&module);

            // Each of these returns `Err` only for a zero value or a second call
            // on the same evaluator, i.e. only for a bug in THIS function.
            // Treating that as a budget failure keeps a mis-wired limit from
            // silently evaluating unbounded.
            eval.set_max_tick_count(budgets.ticks)
                .map_err(|_| TransformError::BudgetExceeded("tick budget (failed to install)"))?;
            eval.set_max_heap_size(MAX_HEAP_BYTES)
                .map_err(|_| TransformError::BudgetExceeded("heap budget (failed to install)"))?;
            if let Some(frames) = budgets.callstack {
                eval.set_max_callstack_size(frames).map_err(|_| {
                    TransformError::BudgetExceeded("call-stack budget (failed to install)")
                })?;
            }

            // The wall-clock bound. Unlike the three above this one returns no
            // `Result`, so there is no mis-install to defend against — but there
            // is also nothing that would notice it missing except
            // `tests::the_deadline_stops_a_script_the_tick_budget_would_let_run`.
            eval.set_check_cancelled(Box::new(move || Instant::now() >= deadline));

            // Placed here, immediately before evaluation, so the injected
            // unwind crosses exactly the frames a real starlark panic crosses:
            // the `with_temp_heap` closure, the live `Evaluator`, and the
            // `AssertUnwindSafe` boundary. A panic raised anywhere shallower
            // would be testing a shorter unwind than the one being defended
            // against.
            #[cfg(test)]
            if inject_panic {
                panic!("{PANIC_INJECTION_MESSAGE}");
            }

            eval.eval_module(ast, &globals).map_err(classify)?;

            let Some(entry_call) = call else {
                // A sandbox probe: the module ran, and there is nothing to call.
                return Ok(serde_json::Value::Null);
            };

            // Which of the crate's two entry-point contracts this call is —
            // decided once, before the lookup, so a script missing EITHER
            // name reports `MissingEntryPoint` against the one it was
            // actually asked to define.
            let contract = match &entry_call {
                Entry::Transform(_) => EntryPoint::Transform,
                Entry::BuildSql { .. } => EntryPoint::BuildSql,
            };
            let entry = module
                .get(contract.name())
                .ok_or(TransformError::MissingEntryPoint(contract))?;
            check_arity(entry, contract)?;

            // Converting the inputs allocates on the SAME heap the script will
            // use, but it is Rust code: it consumes no ticks, so nothing starlark
            // would check ever fires while it runs. Measured, that is not
            // theoretical — a million rows peaked at **335 540 992 bytes** against
            // a 64 MiB limit and was refused only afterwards, by the deadline.
            // So the two budgets that CAN be polled from here are polled from
            // here, every `INPUT_POLL_ROWS` rows.
            let heap = module.heap();

            let returned = match entry_call {
                Entry::Transform(TransformCall { params, statements }) => {
                    let params_v = alloc_params(params, heap, deadline)?;

                    let mut statement_items = Vec::with_capacity(statements.len());
                    for (name, rows) in statements {
                        let mut converted = Vec::with_capacity(rows.len());
                        for (i, row) in rows.iter().enumerate() {
                            if i % INPUT_POLL_ROWS == 0 {
                                poll_input_budgets(heap, deadline)?;
                            }
                            converted.push(json_to_starlark(row, heap, 0)?);
                        }
                        statement_items.push((heap.alloc(name.as_str()), heap.alloc(converted)));
                    }
                    let statements_v = heap.alloc(AllocDict(statement_items));

                    eval.eval_function(entry, &[params_v, statements_v], &[])
                        .map_err(classify)?
                }
                // No `statements` — see `Entry::BuildSql`.
                Entry::BuildSql { params } => {
                    let params_v = alloc_params(&params.0, heap, deadline)?;
                    eval.eval_function(entry, &[params_v], &[])
                        .map_err(classify)?
                }
            };

            // The output phase's deadline. `None` — every shipped build — is
            // the same instant the evaluation ran under, so a script does not
            // get a fresh two seconds for its output. `Some` is the test-only
            // seam described on `Budgets::output_wall_clock`, and it is taken
            // HERE rather than at the top of this function precisely so that
            // what the evaluation spent cannot come out of it.
            let output_deadline = match budgets.output_wall_clock {
                None => deadline,
                Some(window) => Instant::now() + window,
            };

            if contract == EntryPoint::BuildSql {
                // A template, not JSON: `serialise_output`'s shape walk is for
                // a transform's structured return and does not apply here —
                // there is no tree to descend, no depth to bound and nothing to
                // serialise. What the deadline above still bounds is the one
                // thing that could be slow before this point, the evaluation
                // that produced `returned`, which is exactly what
                // `build_sql_is_bounded_by_the_same_budgets_as_a_transform`
                // exercises.
                //
                // The BYTE half of that walk does apply, and this is where it
                // is taken (package S Task 8.5, spec S5). Measured before the
                // copy off the starlark heap, so a script that emitted a
                // megabyte is refused without this process allocating a second
                // one. `MAX_GENERATED_SQL_BYTES` carries why the number is what
                // it is, why this is the only site, and what each caller does
                // with the refusal.
                match returned.unpack_str() {
                    Some(s) if s.len() > MAX_GENERATED_SQL_BYTES => {
                        Err(TransformError::GeneratedSqlTooLarge {
                            bytes: s.len(),
                            cap: MAX_GENERATED_SQL_BYTES,
                        })
                    }
                    Some(s) => Ok(serde_json::Value::String(s.to_owned())),
                    None => Err(TransformError::BuildSqlNotAString(
                        returned.get_type().to_owned(),
                    )),
                }
            } else {
                serialise_output(returned, output_deadline)
            }
        })
    }));

    match caught {
        Ok(result) => result,
        Err(payload) => Err(TransformError::Evaluator(panic_message(&*payload))),
    }
}

/// Recover the message from a caught panic payload.
///
/// `panic!` produces a `&'static str` for a literal and a `String` for a
/// formatted message; anything else is a custom payload we can only name.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "the evaluator panicked with a non-string payload".to_owned()
    }
}

/// Sort an evaluation error into [`TransformError`].
///
/// starlark reports a busted budget as `ErrorKind::Other` wrapping a private
/// enum, so there is no type to downcast to and the message is the only
/// discriminator available. That is a real fragility, and it is contained: if a
/// future version reworded the message the error still surfaces, just as
/// `Runtime` rather than `BudgetExceeded`.
/// `tests::the_budget_refusal_is_classified_as_a_budget` is what would go red.
fn classify(err: starlark::Error) -> TransformError {
    let text = err.to_string();
    match err.kind() {
        // Recursion past MAX_CALLSTACK.
        starlark::ErrorKind::StackOverflow(_) => TransformError::BudgetExceeded("call-stack depth"),
        starlark::ErrorKind::Parser(_) => TransformError::Parse(text),
        _ if text.contains("ticks has been exceeded") => {
            TransformError::BudgetExceeded("tick budget")
        }
        _ if text.contains("Heap memory limit") => TransformError::BudgetExceeded("heap budget"),
        _ if text.contains("Evaluation cancelled") => {
            TransformError::BudgetExceeded("wall-clock deadline")
        }
        _ => TransformError::Runtime(text),
    }
}

/// The entry point exists — does it take the contract's arguments?
///
/// `contract` is supplied by the caller in [`eval_inner`] rather than read from
/// a constant here, because which contract is being checked is a fact only the
/// caller has — this function only has a `Value`. It carries the arity AND the
/// name, so the refusal it builds cannot name one contract while the lookup
/// that produced `entry` used the other.
///
/// # Why the parameters are read out of the value's documentation
///
/// starlark 0.14 keeps a `def`'s `ParametersSpec` `pub(crate)`, so this crate
/// cannot ask a function how many arguments it takes. `Value::documentation()`
/// is the one public surface that exposes the parameter list, and it is derived
/// from that same spec rather than from a docstring, so it is a fact about the
/// function and not about how it was commented.
///
/// The alternative — call it and read the failure — was rejected: starlark's
/// message for a bad call names the argument it could not fill, which is a fine
/// message for a *caller* mistake and a confusing one here, where the caller is
/// egret and the mistake is the author's signature.
///
/// [`transform_dialect`] disables positional-only and keyword-only parameters,
/// so a well-formed `def` puts every parameter in `pos_or_named`; `*args` and
/// `**kwargs` still parse, and are counted so that `def transform(*args)`
/// reports what it is rather than reporting zero.
fn check_arity(entry: Value<'_>, contract: EntryPoint) -> Result<(), TransformError> {
    let params = match entry.documentation() {
        starlark::docs::DocItem::Member(starlark::docs::DocMember::Function(f)) => f.params,
        // Not a function at all — `transform = 1`. See `MissingEntryPoint`.
        _ => return Err(TransformError::MissingEntryPoint(contract)),
    };

    let positional = params.pos_only.len() + params.pos_or_named.len();
    let found = positional
        + params.named_only.len()
        + usize::from(params.args.is_some())
        + usize::from(params.kwargs.is_some());

    // `positional`, not `found`, decides: the entry point is called with
    // exactly `contract.arity()` positional arguments, so that many positional
    // parameters and nothing else is the only signature that works. `found` is
    // what the message reports, because it is what the author wrote.
    let arity = contract.arity();
    if positional == arity && found == arity {
        Ok(())
    } else {
        Err(TransformError::WrongArity {
            entry: contract,
            found,
        })
    }
}

/// How often the input conversion stops to check the budgets it is spending.
///
/// Rows, not bytes, because rows are what the loop counts and because the
/// endpoint's own `limits.max_rows` is denominated the same way. The check is
/// two loads and a compare, so the interval is set by how much overshoot is
/// tolerable rather than by its cost.
///
/// **A batch is 1024 rows, not a fixed number of bytes, and the difference is
/// large.** Measured: a million ordinary rows cost ~335 MB — 335 B/row, so a
/// batch is ~343 KB. But `transform_tests::the_heap_budget_applies_to_the_input_and_not_only_to_the_script`
/// deliberately uses 32 KiB rows, where the same 1024 rows is **32 MiB — half of
/// [`MAX_HEAP_BYTES`]**. So the overshoot this bounds is `1024 x row width`, and
/// a wide-row endpoint overshoots proportionally. Bytes would be the better
/// unit; rows are what the loop has without measuring each row, and the poll is
/// still the difference between 335 MB unwatched and one batch.
const INPUT_POLL_ROWS: usize = 1024;

/// How often the output path stops to check the clock, in walk steps or writes.
///
/// The output half of [`INPUT_POLL_ROWS`], and it exists for the same reason:
/// everything after `eval_function` returns is Rust code that earns no ticks, so
/// nothing starlark checks fires for any of it. Both [`check_output_shape`] and
/// [`CappedSink`] poll on this counter.
///
/// 4096 rather than 1024 because a step here is far smaller than a row there —
/// one node visit, or one `serde_json` token, which for a scalar is a single
/// byte. At the measured ~1.1 µs per visit unoptimised, 4096 steps is ~4.5 ms of
/// overshoot; reading `Instant::now()` per token instead would be a real cost on
/// the ordinary path, which is the one that must stay cheap.
const OUTPUT_POLL_STEPS: usize = 4096;

/// Consult the deadline once every [`OUTPUT_POLL_STEPS`] steps, resetting the
/// counter.
///
/// One function, because the output path polls from three places — the walk's
/// outer loop, the walk's child loops, and [`CappedSink::write`] — and three
/// hand-rolled counters is three chances to get the reset wrong.
fn poll_output_deadline(
    since_poll: &mut usize,
    deadline: Instant,
    refusal: &'static str,
) -> Result<(), TransformError> {
    if *since_poll < OUTPUT_POLL_STEPS {
        return Ok(());
    }
    *since_poll = 0;
    if Instant::now() >= deadline {
        return Err(TransformError::BudgetExceeded(refusal));
    }
    Ok(())
}

/// The refusal [`check_output_shape`] reports when the deadline passes under it.
const SHAPE_DEADLINE: &str = "wall-clock deadline, checking the shape of its output";

/// The refusal [`CappedSink`] reports when the deadline passes under it.
const SINK_DEADLINE: &str = "wall-clock deadline, serialising its output";

/// The budgets that Rust-side input conversion can enforce on itself.
///
/// # Why the labels differ from the script's own
///
/// `"heap budget, converting its input"` rather than `"heap budget"`, and it is
/// not only better diagnostics — it is the only thing that makes this function's
/// absence *observable*. starlark re-checks the heap once at the end of the
/// evaluation, so deleting the calls below still produces a heap refusal, just
/// after allocating the entire input first; a test asserting `"heap budget"`
/// passes either way. Measured: removing the poll left
/// `transform_tests::the_heap_budget_applies_to_the_input_and_not_only_to_the_script`
/// GREEN until the labels were separated.
///
/// [`MAX_TICKS`] is not one of them — conversion earns no ticks — and neither is
/// [`MAX_CALLSTACK`]. The heap check is spelled out rather than delegated to
/// `Evaluator::check_heap_size_limit` because that method returns a
/// `ResourceCheckResult` whose module is not public in starlark 0.14, so its
/// variants cannot be named from here; `Heap::peak_allocated_bytes` is the same
/// number the library compares.
fn poll_input_budgets(
    heap: starlark::values::Heap<'_>,
    deadline: Instant,
) -> Result<(), TransformError> {
    if Instant::now() >= deadline {
        return Err(TransformError::BudgetExceeded(
            "wall-clock deadline, converting its input",
        ));
    }
    if heap.peak_allocated_bytes() > MAX_HEAP_BYTES {
        return Err(TransformError::BudgetExceeded(
            "heap budget, converting its input",
        ));
    }
    Ok(())
}

/// Convert one `serde_json::Value` into a starlark value on `heap`.
///
/// Recursive, and bounded by [`MAX_JSON_DEPTH`] on the way in as well as on
/// the way out. Input nesting is already bounded in practice — everything here
/// arrives from `serde_json`, whose parser refuses past 128 levels — but "in
/// practice" is not a bound, and this recursion runs on whatever stack the
/// serving task happens to have.
fn json_to_starlark<'v>(
    value: &serde_json::Value,
    heap: starlark::values::Heap<'v>,
    depth: usize,
) -> Result<Value<'v>, TransformError> {
    if depth > MAX_JSON_DEPTH {
        return Err(TransformError::BudgetExceeded("input nesting depth"));
    }
    Ok(match value {
        serde_json::Value::Null => Value::new_none(),
        serde_json::Value::Bool(b) => Value::new_bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                heap.alloc(i)
            } else if let Some(u) = n.as_u64() {
                // Above `i64::MAX`. starlark promotes to a big int rather than
                // wrapping, so this is lossless — `as_f64` here would not be.
                heap.alloc(u)
            } else if let Some(f) = n.as_f64() {
                heap.alloc(f)
            } else {
                return Err(TransformError::NotSerialisable(format!(
                    "the number {n} has no starlark representation"
                )));
            }
        }
        serde_json::Value::String(s) => heap.alloc(s.as_str()),
        serde_json::Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                converted.push(json_to_starlark(item, heap, depth + 1)?);
            }
            heap.alloc(converted)
        }
        serde_json::Value::Object(fields) => {
            let mut converted = Vec::with_capacity(fields.len());
            for (k, v) in fields {
                converted.push((
                    heap.alloc(k.as_str()),
                    json_to_starlark(v, heap, depth + 1)?,
                ));
            }
            heap.alloc(AllocDict(converted))
        }
    })
}

/// Turn what `transform` returned into JSON, or say why it cannot be.
///
/// Refusals, in the order they can be detected:
///
/// 1. **too deep, cyclic, or too large once expanded** — [`check_output_shape`],
///    which must run *first* because everything below recurses over the same
///    structure and two of those three would not survive it;
/// 2. **not serialisable** — a function, a `range`, a dict with a non-string
///    key, a NaN;
/// 3. **too large in bytes** — [`MAX_TRANSFORM_OUTPUT_BYTES`]. Step 1 bounds the
///    number of values; this bounds their size, which a single long string can
///    blow on its own.
///
/// The value is serialised straight into a counting sink rather than through
/// `Value::to_json_value`. `to_json_value` would materialise the entire
/// `serde_json::Value` tree first — which for a return at the heap limit is
/// hundreds of megabytes of Rust heap, allocated only to be measured and thrown
/// away. The sink counts every byte but retains at most the cap, so the refusal
/// path costs a megabyte no matter how large the return was, and the accepted
/// path re-parses a buffer that is by definition small.
fn serialise_output(
    returned: Value<'_>,
    deadline: Instant,
) -> Result<serde_json::Value, TransformError> {
    check_output_shape(returned, deadline)?;

    let mut sink = CappedSink::new(MAX_TRANSFORM_OUTPUT_BYTES, deadline);
    let serialised = serde_json::to_writer(&mut sink, &returned);

    // Our own refusals before the value's, and deliberately: a sink that stopped
    // makes `to_writer` return an io error, and reporting THAT as "not JSON"
    // would send an author to fix a value that is perfectly fine.
    if sink.stopped == Some(Stop::Deadline) {
        return Err(TransformError::BudgetExceeded(SINK_DEADLINE));
    }
    if sink.stopped == Some(Stop::Ceiling) || sink.written > sink.cap {
        return Err(TransformError::OutputTooLarge {
            bytes: sink.written,
            cap: sink.cap,
        });
    }
    serialised.map_err(|e| TransformError::NotSerialisable(e.to_string()))?;

    serde_json::from_slice(&sink.buf).map_err(|e| TransformError::NotSerialisable(e.to_string()))
}

/// How far past the cap the counting is allowed to go before it gives up.
///
/// # Why counting cannot simply run to the end
///
/// [`check_output_shape`] already bounds the *number* of values, so the
/// exponential case never reaches here. What it cannot bound is their size: a
/// list of a thousand ten-megabyte strings is a thousand values and ten
/// gigabytes of output, and a sink that insisted on counting all of it would
/// copy ten gigabytes to report a number.
///
/// 16x the cap keeps the number in [`TransformError::OutputTooLarge`] exact for
/// every overshoot an author is likely to make honestly — someone who returns
/// three megabytes learns they returned three megabytes — while bounding the
/// work at 16 MiB of bulk writes, which is milliseconds.
const OUTPUT_COUNT_CEILING: usize = MAX_TRANSFORM_OUTPUT_BYTES * 16;

/// A writer that counts, keeps only the first `cap` bytes, and gives up at
/// [`OUTPUT_COUNT_CEILING`].
///
/// Counting past the cap is the point: [`TransformError::OutputTooLarge`]
/// carries the real size, and "too large" without a number tells an author
/// nothing about how much to cut. Retaining past the cap is what we refuse to
/// do, so an oversized return costs `cap` bytes of memory rather than its own
/// size — and stopping at the ceiling is what keeps it costing bounded *time*
/// as well.
/// Why a [`CappedSink`] gave up. Two reasons, and they are different refusals —
/// one is "your response is too big", the other is "your script ran too long".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Ceiling,
    Deadline,
}

struct CappedSink {
    buf: Vec<u8>,
    written: usize,
    cap: usize,
    /// Set once this sink refused further bytes. The resulting `to_writer` error
    /// is then ours, not the value's — see [`serialise_output`], which checks
    /// this before it believes the error.
    stopped: Option<Stop>,
    deadline: Instant,
    /// Writes since the deadline was last consulted. `Instant::now()` per write
    /// would be per *token* — serde_json writes `[` with its own call — so the
    /// clock is read once per [`OUTPUT_POLL_STEPS`] instead.
    since_poll: usize,
}

impl CappedSink {
    fn new(cap: usize, deadline: Instant) -> Self {
        CappedSink {
            buf: Vec::new(),
            written: 0,
            cap,
            stopped: None,
            deadline,
            since_poll: 0,
        }
    }
}

impl std::io::Write for CappedSink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.written >= OUTPUT_COUNT_CEILING {
            self.stopped = Some(Stop::Ceiling);
            return Err(std::io::Error::other(
                "the transform output passed the counting ceiling",
            ));
        }
        self.since_poll += 1;
        if poll_output_deadline(&mut self.since_poll, self.deadline, SINK_DEADLINE).is_err() {
            self.stopped = Some(Stop::Deadline);
            return Err(std::io::Error::other(SINK_DEADLINE));
        }
        self.written += data.len();
        if self.buf.len() < self.cap {
            let room = self.cap - self.buf.len();
            self.buf.extend_from_slice(&data[..data.len().min(room)]);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Refuse a return value whose *expanded* shape is unserviceable, before a single
/// byte of it is serialised.
///
/// Two refusals, from one memoised walk over the distinct values:
///
/// * **too deep** — [`MAX_JSON_DEPTH`], because serialisation recurses;
/// * **too large once expanded** — more values than [`MAX_TRANSFORM_OUTPUT_BYTES`]
///   allows bytes. Sound as a lower bound: every JSON value costs at least one
///   byte, and every array or object costs at least one more for its brackets,
///   so a structure with more values than the byte cap cannot fit under it.
///
/// The size half exists because starlark values are **shared, not copied**,
/// while serialisation **expands every reference**: `a = [1]` then `a = [a, a]`
/// sixty-two times is 63 allocations and a few dozen ticks, and serialises to
/// 2^62 values. Measured, letting the byte-counting sink discover that instead
/// took **25 seconds** to reach a 2 MiB ceiling in an unoptimised build; this
/// walk refuses the same value in **~300 µs**, because it saturates the count at
/// each node rather than emitting it.
///
/// See [`MAX_JSON_DEPTH`] for why the depth half is not decoration:
/// serialisation recurses, nothing else bounds its depth, and a stack overflow
/// is an abort that [`eval_inner`]'s `catch_unwind` cannot contain.
///
/// # The memoisation is load-bearing for termination. Do not delete it.
///
/// A walk that does not remember nodes visits *paths*, not nodes. On a pure
/// doubling structure that is survivable, and measuring only that shape is how
/// this comment came to say the opposite for one revision: deleting both memo
/// lookups left the doubling test GREEN, at 2.39 s instead of 0.03 s, because
/// sizes are computed bottom-up and the walk is refused at the first node whose
/// subtree passes [`MAX_TRANSFORM_OUTPUT_BYTES`] — level 21, ~2^21 visits.
///
/// **That inference was wrong, and one shape is why.** Keep every shared subtree
/// *under* every cap and then fan out, and the bottom-up refusal never gets a
/// chance to fire:
///
/// ```text
/// c = [1]
/// for i in range(18):
///     c = [c, c]
/// return [c] * 100000
/// ```
///
/// ~19 ticks and under a megabyte — beneath every budget in this module. `c`
/// expands to 786 431 values, which is *inside* the cap, so it is accepted; only
/// the root exceeds it, and the root cannot complete until all 100 000 copies of
/// `c` have been walked. Memo-less that is ~7.9e10 visits, which at the ~1.1 µs
/// per visit measured above is **hours**. With the memo it is ~100 058 and
/// returns in milliseconds.
///
/// So: the memo is the difference between a refusal and a hang, on a script that
/// costs its author nineteen ticks to write.
/// `transform_tests::a_fanned_out_shared_subtree_terminates_rather_than_hanging`
/// is that shape, and it is what makes deleting either memo lookup fail a test
/// rather than merely slow one down.
///
/// The `on_stack` set is what catches a genuine cycle (`x = []; x.append(x)`).
/// It is **not** there because starlark would miss one — measured, starlark
/// raises "Cycle detected when serializing value of type `list`" on exactly that
/// script. It is there because *this walk* would otherwise never reach starlark:
/// a memoised DFS on a cyclic graph never assigns a depth to any node on the
/// cycle and spins between them. A refusal that hangs is worse than the error it
/// was trying to improve on.
fn check_output_shape(root: Value<'_>, deadline: Instant) -> Result<(), TransformError> {
    /// The values a container holds. Anything not listed is a leaf for this
    /// purpose — a starlark `set` can only hold hashable values, so it cannot
    /// contain a list, a dict or a tuple and cannot deepen a structure.
    fn children<'v>(value: Value<'v>) -> Option<Vec<Value<'v>>> {
        if let Some(list) = ListRef::from_value(value) {
            return Some(list.content().to_vec());
        }
        if let Some(tuple) = TupleRef::from_value(value) {
            return Some(tuple.content().to_vec());
        }
        if let Some(dict) = DictRef::from_value(value) {
            return Some(dict.iter().flat_map(|(k, v)| [k, v]).collect());
        }
        None
    }

    // (value, whether its children have already been pushed)
    let mut stack: Vec<(Value<'_>, bool)> = vec![(root, false)];
    // id -> (nesting depth below it, expanded value count including it)
    let mut known: HashMap<_, (usize, usize)> = HashMap::new();
    let mut on_stack: HashSet<_> = HashSet::new();
    let mut since_poll = 0usize;

    while let Some((value, expanded)) = stack.pop() {
        // This walk runs after `eval_function` has returned, so it earns no
        // ticks and starlark checks nothing for it. Bounded by the heap it walks
        // — but `MAX_HEAP_BYTES` allows millions of nodes, so "bounded" is not
        // the same as "quick". The deadline is the same one the evaluation ran
        // under: a script does not get a fresh two seconds for its output.
        since_poll += 1;
        poll_output_deadline(&mut since_poll, deadline, SHAPE_DEADLINE)?;

        let id = value.identity();
        if known.contains_key(&id) {
            continue;
        }

        let Some(kids) = children(value) else {
            known.insert(id, (0, 1));
            continue;
        };

        if expanded {
            on_stack.remove(&id);
            let mut deepest = 0usize;
            let mut expanded_size = 1usize;
            for kid in &kids {
                // Polled INSIDE the child loop, not once per outer turn. A node
                // whose children are all already known costs one outer turn and
                // `kids.len()` work, so an outer-turn counter never reaches its
                // threshold — measured, removing an equivalent batch poll left
                // the fan-out test green because a *different* shape happened to
                // generate enough outer turns to trip it. Bounded overshoot: one
                // `OUTPUT_POLL_STEPS` of children.
                since_poll += 1;
                poll_output_deadline(&mut since_poll, deadline, SHAPE_DEADLINE)?;
                let (d, n) = known.get(&kid.identity()).copied().unwrap_or((0, 1));
                deepest = deepest.max(d);
                expanded_size = expanded_size.saturating_add(n);
            }
            let depth = deepest + 1;
            if depth > MAX_JSON_DEPTH {
                return Err(TransformError::BudgetExceeded("output nesting depth"));
            }
            if expanded_size > MAX_TRANSFORM_OUTPUT_BYTES {
                return Err(TransformError::OutputTooLarge {
                    bytes: expanded_size,
                    cap: MAX_TRANSFORM_OUTPUT_BYTES,
                });
            }
            known.insert(id, (depth, expanded_size));
            continue;
        }

        if !on_stack.insert(id) {
            // Reached a node that is still being expanded: a cycle.
            return Err(TransformError::NotSerialisable(
                "the transform returned a structure that contains itself".to_owned(),
            ));
        }
        stack.push((value, true));
        for kid in kids {
            since_poll += 1;
            poll_output_deadline(&mut since_poll, deadline, SHAPE_DEADLINE)?;
            if !known.contains_key(&kid.identity()) {
                stack.push((kid, false));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `check_returns` over the shapes that are not an object at all.
    ///
    /// The rule has two callers — publish's probe and, from Task 6, serving —
    /// so it is pinned here rather than only through whichever of them happens
    /// to exercise it. The message must name what DID come back: "does not
    /// match `returns`" over a script that returned a list sends an author
    /// looking for a missing field.
    #[test]
    fn only_a_json_object_can_satisfy_returns() {
        let declared = vec![egret_dataapi_api::ColumnSpec {
            name: "total".into(),
            ty: "bigint".into(),
            required: true,
        }];
        for (value, expected) in [
            (serde_json::json!([]), "a list"),
            (serde_json::json!(1), "a number"),
            (serde_json::json!("total"), "a string"),
            (serde_json::json!(true), "a boolean"),
            (serde_json::Value::Null, "`None`"),
        ] {
            let detail =
                check_returns(&value, &declared).expect_err("only an object has named fields");
            assert!(
                detail.contains(expected),
                "the refusal must name what came back ({expected}): {detail}"
            );
        }
        assert_eq!(
            check_returns(&serde_json::json!({ "total": 1 }), &declared),
            Ok(())
        );
    }

    /// Build a script that recurses exactly `depth` times and then returns.
    fn recursion_probe(depth: usize) -> String {
        format!("def f(n):\n  if n == 0:\n    return 0\n  return f(n - 1)\nf({depth})")
    }

    /// The names a transform is permitted to reach. Sorted.
    ///
    /// One list, read by both the structural snapshot and the behavioural one,
    /// so the two cannot disagree about what the boundary is.
    fn snapshot_of_permitted_globals() -> Vec<&'static str> {
        vec![
            "False",
            "None",
            "True",
            "abs",
            "all",
            "any",
            "bool",
            "bytes",
            "chr",
            "dict",
            "dir",
            "enumerate",
            "fail",
            "float",
            "getattr",
            "hasattr",
            "hash",
            "int",
            "len",
            "list",
            "max",
            "min",
            "ord",
            "range",
            "repr",
            "reversed",
            "sorted",
            "str",
            "tuple",
            "type",
            "zip",
        ]
    }

    /// Build a script that doubles a string `n` times, i.e. allocates 2^n bytes.
    fn doubling_probe(n: usize) -> String {
        format!("def g():\n  s = 'x'\n  for i in range({n}):\n    s = s + s\n  return len(s)\ng()")
    }

    // ---------------------------------------------------------------------
    // The global environment
    // ---------------------------------------------------------------------

    /// The assertion the rest of the sandbox tests lean on.
    ///
    /// Every other test here probes *names* — `open`, `time`, `db` — and pinning
    /// names does not pin the boundary. Measured: swapping
    /// `Globals::standard()` for `Globals::extended_internal()` in
    /// [`eval_inner`] left all of them green while `print('hi')`, `debug(1)` and
    /// `struct(a=1)` started returning `Ok(())` and `breakpoint`, `call_stack`,
    /// `pprint` and `eval_type` became reachable. `print(__file__)` simply moved
    /// on to dying at `__file__` and still looked like a passing probe.
    ///
    /// So this snapshots the whole set. It is deliberately brittle: any change
    /// to what a transform can name — ours or starlark's, in a patch release —
    /// stops the build's tests until someone looks at the diff. That is the
    /// point, and it is the module doc's "safety is a property of the language"
    /// claim reduced to something checkable.
    ///
    /// All 31 are pure: arithmetic, collections, conversion, reflection over
    /// values already in hand. None performs I/O, reads a clock, or produces
    /// randomness.
    #[test]
    fn the_global_environment_is_exactly_the_starlark_standard() {
        // The `Globals` MUST be held in a binding and the names MUST be copied
        // out. `FrozenStringValue::as_str` hands back a `&str` whose lifetime is
        // not tied to the `Globals` that owns the storage, so
        // `Globals::standard().names().map(|n| n.as_str()).collect()` compiles
        // and then reads freed memory. That is not theoretical: the first
        // version of this test did exactly that, passed when run alone, and
        // failed under the parallel suite with entries like "\0\0\0" once
        // another test reused the pages.
        let globals = transform_globals();
        let mut actual: Vec<String> = globals.names().map(|n| n.as_str().to_owned()).collect();
        actual.sort_unstable();

        let expected = snapshot_of_permitted_globals();

        let actual: Vec<&str> = actual.iter().map(String::as_str).collect();
        assert_eq!(
            actual, expected,
            "the set of names a transform can reach has changed. This is not a \
             test to update — it is the sandbox boundary. Anything added here is \
             something every endpoint author can now call; check what it does \
             before touching this list."
        );
    }

    /// The snapshot above, asserted through the EVALUATOR instead of through a
    /// `Globals` object.
    ///
    /// This exists because the structural version alone was not enough, and that
    /// was measured, not guessed: the first attempt read `Globals::standard()`
    /// directly, so swapping [`eval_inner`]'s environment for
    /// `Globals::extended_internal()` left every test in this module — the
    /// structural snapshot included — GREEN. A snapshot of a function nobody calls proves nothing about what a
    /// script can reach.
    ///
    /// So both directions are driven through the real evaluation path: every
    /// name in the snapshot must resolve, and every name starlark's extended
    /// library would have added must not. The negative half is computed from
    /// `Globals::extended_internal()` rather than listed, so it grows by itself
    /// when a starlark upgrade adds an extension.
    #[test]
    fn the_evaluator_resolves_the_snapshot_and_nothing_beyond_it() {
        let permitted = snapshot_of_permitted_globals();

        for name in &permitted {
            assert!(
                eval_inner(name, Budgets::default(), None).is_ok(),
                "`{name}` is in the snapshot but does not resolve in the \
                 evaluator — the snapshot and the evaluation path have drifted"
            );
        }

        let extended = Globals::extended_internal();
        let beyond: Vec<String> = extended
            .names()
            .map(|n| n.as_str().to_owned())
            .filter(|n| !permitted.contains(&n.as_str()))
            .collect();
        assert!(
            beyond.len() > 15,
            "the negative half must actually have something to check; \
             extended_internal added only {} names beyond the snapshot",
            beyond.len()
        );

        for name in &beyond {
            match eval_inner(name, Budgets::default(), None) {
                Err(TransformError::Runtime(msg)) => assert!(
                    msg.contains(&format!("Variable `{name}` not found")),
                    "`{name}` is outside the snapshot and must be unbound: {msg}"
                ),
                other => panic!(
                    "`{name}` is part of starlark's EXTENDED library and must not \
                     be reachable from a transform. Reaching it means the \
                     evaluator is not running on the snapshotted environment: \
                     {other:?}"
                ),
            }
        }
    }

    /// `db`, `http`, `tx` and `cache` are the spec's stage-1 boundary, and the
    /// brief's five probes do not mention them. They are unbound today, which is
    /// worth an assertion rather than an inference — stages 2 to 4 are where
    /// somebody proposes injecting one, and this is where that decision has to
    /// be made deliberately instead of by an `insert` going unnoticed.
    #[test]
    fn the_four_capability_names_are_unbound() {
        for name in ["db", "http", "tx", "cache"] {
            match eval_inner(&format!("{name}.anything()"), Budgets::default(), None) {
                Err(TransformError::Runtime(msg)) => assert!(
                    msg.contains(&format!("Variable `{name}` not found")),
                    "`{name}` must be unbound, and must fail by being unbound: {msg}"
                ),
                other => {
                    panic!("`{name}` is stage 1's boundary and must not be reachable: {other:?}")
                }
            }
        }
    }

    // ---------------------------------------------------------------------
    // The brief's probes
    // ---------------------------------------------------------------------

    #[test]
    fn a_script_cannot_reach_the_filesystem_or_the_clock() {
        // The whole language choice rests on this: safety is a property of the
        // LANGUAGE, not of a list of globals we remembered to strip. If any of
        // these evaluate, that premise is false and the choice must be reopened.
        //
        // Kept for legibility, but note it is NOT what holds the boundary — see
        // `the_global_environment_is_exactly_the_starlark_standard`, which is
        // the test that survives a widened global set.
        for src in [
            "open('/etc/passwd')",
            "print(__file__)",
            "import os",
            "time.time()",
            "random.random()",
        ] {
            assert!(
                eval_probe(src).is_err(),
                "a script reached something it must not: {src}"
            );
        }
    }

    /// The companion to the test above, and the reason it is worth anything.
    ///
    /// `is_err()` is satisfied by a *syntax* error, which would prove nothing
    /// about the sandbox — a probe that fails because we typo'd it looks exactly
    /// like a probe that failed because the sandbox held. So each probe is
    /// pinned to the reason it actually fails, and the split is stated out loud:
    ///
    /// - four of the brief's five probes reach evaluation and die on an
    ///   **unbound name**. That is the sandbox: `open`, `print`, `time` and
    ///   `random` are simply not in [`Globals::standard`], and nothing in this
    ///   crate puts them there.
    /// - measured, not assumed: `print(__file__)` dies on **`print`**, which
    ///   turns out to be absent from `Globals::standard` in starlark 0.14. It
    ///   therefore never evaluates `__file__`, so the identifier the probe is
    ///   named for was untested until `x = __file__` was added below.
    /// - `import os` never reaches evaluation. Starlark has no `import`
    ///   statement, so it is a **parse** error. It still belongs — "the grammar
    ///   cannot express an import" is a stronger guarantee than "the import
    ///   failed" — but it exercises the parser, not the global environment.
    #[test]
    fn each_sandbox_probe_fails_for_the_reason_we_think_it_does() {
        for src in [
            "open('/etc/passwd')",
            "print(__file__)",
            "time.time()",
            "random.random()",
            // NOT one of the brief's five. `print(__file__)` dies on `print`
            // being unbound and never evaluates `__file__` at all, so without
            // this line the identifier the probe is named for goes untested.
            "x = __file__",
        ] {
            match eval_probe(src) {
                Err(TransformError::Runtime(msg)) => assert!(
                    msg.contains("not found") || msg.contains("Variable"),
                    "{src} failed at run time, but not by failing to resolve a \
                     name — so it is not clear the sandbox is what stopped it: {msg}"
                ),
                other => panic!(
                    "{src} must reach evaluation and die on an unbound name; \
                     a parse error here would prove nothing about the sandbox: {other:?}"
                ),
            }
        }

        assert!(
            matches!(eval_probe("import os"), Err(TransformError::Parse(_))),
            "`import os` is expected to be a PARSE error — if it ever became a \
             runtime error, starlark grew an import statement and this module's \
             central premise needs re-examining"
        );
    }

    // ---------------------------------------------------------------------
    // The dialect
    // ---------------------------------------------------------------------

    /// `load()` is the one construct in standard Starlark that names something
    /// outside the script, and `Dialect::Standard` leaves it ON.
    ///
    /// Asserted on the dialect itself as well as through [`eval_probe`]. Driving
    /// it only through `eval_probe` would pin *this* parse site and nothing
    /// else, so a second parse site added later with `Dialect::Standard` would
    /// leave this test green while `load()` worked fine over there.
    #[test]
    fn load_is_not_part_of_the_transform_dialect() {
        assert!(
            !transform_dialect().enable_load,
            "the property is on the DIALECT, not on whoever happens to call it"
        );
        assert!(
            matches!(
                eval_probe("load('//other:defs.bzl', 'thing')"),
                Err(TransformError::Parse(_))
            ),
            "load() must not parse; if it does, the sandbox depends on no one \
             ever calling Evaluator::set_loader"
        );
    }

    /// The other half of the same worry, and the honest limit of what a test can
    /// do about it: [`transform_dialect`] only protects parses that *use* it.
    ///
    /// So this asserts there is exactly one place in the crate that parses
    /// Starlark at all. A second one — Task 3 is the likely author — fails here
    /// and has to say which dialect it means.
    ///
    /// # This is a convention-enforcer, not a proof
    ///
    /// Read it for what it is before relying on it:
    ///
    /// - It greps **text** — the literal `AstModule` immediately followed by
    ///   `::parse` — so it cannot see a parse reached through an alias, a
    ///   re-export, or a helper in another crate. (Written in two pieces here
    ///   for the same reason the needle is: the contiguous spelling in this doc
    ///   comment would be counted as a second parse site, which is exactly what
    ///   happened on the first attempt.)
    /// - It reads `$CARGO_MANIFEST_DIR/src` **at run time**, so it is checking
    ///   the sources on disk now, not the sources this binary was built from.
    ///   Those normally agree; they do not have to. A missing or unreadable
    ///   `src/` therefore makes this test FAIL LOUDLY rather than pass on an
    ///   empty scan — a green "exactly one parse site" from a directory that was
    ///   never read would be worse than no test.
    ///
    /// The needle is assembled at run time rather than written as a literal,
    /// because a literal would match this test's own source and make the count
    /// wrong. Do not "simplify" it.
    #[test]
    fn there_is_exactly_one_place_that_parses_starlark() {
        let needle = ["AstModule", "::", "parse"].concat();
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

        // Fail loudly rather than passing on an empty scan. Everything below
        // counts occurrences, so an unreadable tree would yield zero hits and a
        // confidently wrong verdict.
        assert!(
            src_dir.is_dir(),
            "{} is not a readable directory, so this test cannot see the crate's \
             sources and must not report a verdict about them",
            src_dir.display()
        );

        let mut files_scanned = 0usize;
        let mut hits: Vec<String> = Vec::new();
        let mut stack = vec![src_dir.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("the crate's src/ must be readable") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("a readable .rs file");
                    files_scanned += 1;
                    for _ in 0..text.matches(&needle).count() {
                        hits.push(path.display().to_string());
                    }
                }
            }
        }

        // This module alone guarantees a positive count, so a zero here means the
        // walk did not reach the sources it claims to have checked.
        assert!(
            files_scanned > 0,
            "no .rs files were scanned under {}, so the count below is \
             meaningless",
            src_dir.display()
        );

        assert_eq!(
            hits.len(),
            1,
            "there must be exactly one Starlark parse site in this crate, so that \
             `transform_dialect` is not merely a convention the next parse site \
             may forget. Found: {hits:?}"
        );
        assert!(
            hits[0].ends_with("transform.rs"),
            "the single parse site must be this module: {hits:?}"
        );
    }

    // ---------------------------------------------------------------------
    // The call-stack budget
    // ---------------------------------------------------------------------

    /// The third budget — and the one that nearly shipped untested.
    ///
    /// The obvious test is unbounded recursion, and it is worthless: starlark
    /// bounds recursion unaided via a private `DEFAULT_STACK_SIZE` fallback.
    /// Measured — deleting the `set_max_callstack_size` call left that test
    /// GREEN.
    ///
    /// This one runs the SAME probe twice, once with our budget and once
    /// without. The `None` run is what makes it honest: rather than comparing
    /// [`RECURSION_PROBE_DEPTH`] against a constant copied out of starlark
    /// (`DEFAULT_STACK_SIZE` is `pub(crate)`, so this crate cannot see it and a
    /// copy could silently stop tracking it), it demonstrates that the probe
    /// completes under starlark's own fallback. A refusal in the second run can
    /// then only be [`MAX_CALLSTACK`].
    #[test]
    fn recursion_is_refused_at_our_depth_not_the_library_default() {
        let probe = recursion_probe(RECURSION_PROBE_DEPTH);

        assert!(
            eval_inner(&probe, Budgets::default().without_callstack_limit(), None).is_ok(),
            "recursion {RECURSION_PROBE_DEPTH} deep must COMPLETE under \
             starlark's own fallback limit — if it does not, the refusal below \
             proves nothing about MAX_CALLSTACK, because the library would have \
             refused it anyway"
        );

        // A `match`, not `expect_err`: that takes a `&str`, so the depths could
        // not be formatted into the message that has to explain the failure.
        let err = match eval_inner(&probe, Budgets::default(), None) {
            Err(e) => e,
            Ok(_) => panic!(
                "recursion {RECURSION_PROBE_DEPTH} deep must be refused once \
                 MAX_CALLSTACK ({MAX_CALLSTACK}) is installed. The line above \
                 just showed the library does NOT refuse it, so completing here \
                 means our budget is either not applied or set too loose to bind"
            ),
        };
        assert!(
            matches!(err, TransformError::BudgetExceeded("call-stack depth")),
            "the refusal must be the call-stack budget, not the tick budget: {err:?}"
        );
    }

    /// Unbounded recursion terminates too.
    ///
    /// This asserts far less than the test above and the difference is worth
    /// stating: starlark's own fallback catches this unaided, so it stays green
    /// with `set_max_callstack_size` deleted, and it would stay green with
    /// [`MAX_CALLSTACK`] raised to a value that binds nothing. It is kept
    /// because it is the case an author will actually hit, and because it pins
    /// that the outcome is a refusal rather than a native stack overflow taking
    /// the process with it — but it is not evidence about our budget.
    #[test]
    fn unbounded_recursion_terminates_rather_than_blowing_the_native_stack() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(eval_probe("def f():\n  return f()\nf()"));
        });
        let err = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("unbounded recursion was not stopped within 10s")
            .expect_err("unbounded recursion must be refused");
        assert!(
            matches!(err, TransformError::BudgetExceeded("call-stack depth")),
            "unbounded recursion must surface as a call-stack refusal: {err:?}"
        );
    }

    // ---------------------------------------------------------------------
    // The heap budget
    // ---------------------------------------------------------------------

    /// The heap budget, two-sided.
    ///
    /// Task 1 originally shipped this untested on the stated grounds that
    /// exceeding 64 MiB would be slow. That was reasoning, and it was wrong:
    /// doubling a string is exponential, so 2^24 bytes trips the limit in about
    /// 25 ms. Both directions are asserted, because a test that only shows a
    /// refusal cannot distinguish a working budget from a broken evaluator that
    /// refuses everything.
    #[test]
    fn the_heap_budget_refuses_a_script_that_outgrows_it() {
        assert!(
            eval_probe(&doubling_probe(20)).is_ok(),
            "2^20 bytes is well inside the 64 MiB budget and must be allowed — \
             without this half, a refusal below would not distinguish a working \
             budget from an evaluator that refuses everything"
        );

        let err = eval_probe(&doubling_probe(24))
            .expect_err("2^24 bytes exceeds the 64 MiB budget and must be refused");
        assert!(
            matches!(err, TransformError::BudgetExceeded("heap budget")),
            "the refusal must classify as the heap budget — this is the only \
             test that exercises `classify`'s heap arm and its message match: {err:?}"
        );
    }

    // ---------------------------------------------------------------------
    // Containment
    // ---------------------------------------------------------------------

    /// starlark panics on author input, and this is the boundary that stops it.
    ///
    /// `str_type.rs:148` asserts `len as u32 as usize == len` before building a
    /// `StarlarkStr`, so a string of 4 GiB or more aborts the thread with
    /// `len overflow`. No budget prevents reaching it — the heap limit was
    /// measured overshooting 64 MiB by 32x at one doubling less than this.
    ///
    /// Without the `catch_unwind` in [`eval_inner`] this test does not fail, it
    /// *panics*, and in the serving path the unwind would take the connection
    /// task with it: there is no `CatchPanicLayer` under `crates/` and
    /// `tower-http` is built here without its `catch-panic` feature.
    ///
    /// Expect to see `panicked at ... len overflow` in the test output even when
    /// this passes. That is the panic hook doing its job before the unwind is
    /// converted into a value; catching a panic is not the same as hiding it.
    /// The containment itself, tested cheaply and run every time.
    ///
    /// The test below reaches a REAL starlark panic and is the more convincing
    /// demonstration, but it costs 4.31 GB and is `#[ignore]`d for that reason —
    /// which would leave the boundary this module actually owns with no
    /// unconditional cover at all. So this one injects the panic directly,
    /// through the same frames, and asserts the three things that make the
    /// containment worth having:
    ///
    /// 1. the call **returns** rather than unwinding into its caller;
    /// 2. it yields [`TransformError::Evaluator`] and **not**
    ///    [`TransformError::Runtime`] — the distinction that lets an operator
    ///    tell "the script was refused" from "the evaluator broke";
    /// 3. the panic message survives **verbatim**, so the report names the
    ///    defect instead of merely reporting that one occurred.
    ///
    /// No thread or channel here on purpose: with nothing to block on, a lost
    /// containment can only surface as this test panicking, never as a hang.
    ///
    /// It also covers the `String` arm of [`panic_message`] — `panic!` with a
    /// formatted argument produces a `String` payload, where starlark's
    /// `assert!` literal produces a `&'static str`.
    #[test]
    fn an_injected_panic_returns_as_an_error_instead_of_unwinding() {
        match eval_probe(PANIC_INJECTION_SRC) {
            Err(TransformError::Evaluator(msg)) => assert_eq!(
                msg, PANIC_INJECTION_MESSAGE,
                "the panic message must reach the caller verbatim — a caught \
                 panic that loses its message tells an operator that something \
                 broke but not what"
            ),
            Err(TransformError::Runtime(msg)) => panic!(
                "a panic must NOT be folded into Runtime: an operator has to be \
                 able to tell a refused script from a broken evaluator, and \
                 Runtime is what a refused script looks like. Got: {msg}"
            ),
            other => panic!(
                "an injected panic must be contained and returned as \
                 TransformError::Evaluator: {other:?}"
            ),
        }
    }

    // WHY THIS IS IGNORED, since re-enabling it looks harmless.
    //
    // Measured: 4,307,374,080 bytes max RSS, ~8 s. The cost is inherent — there
    // is no cheaper route to this panic. String *repetition* is guarded by
    // starlark's i32 integer limit (`'x' * 4294967296` fails with
    // "Integer value is too big to fit in i32" at 12 MB), so only repeated
    // *concatenation* reaches `StarlarkStr::new`, and that needs 4 GiB of live
    // strings by construction. Three variants were tried; all were guarded.
    //
    // The decisive part is the FAILURE mode, not the cost: under memory
    // pressure the 4 GiB allocation aborts, which kills the whole test binary
    // rather than producing one red test. A test that can take the suite down
    // is worse in CI than a test that is absent — and this repo has an OOM
    // history. So it stays as what it is: a reproduction of a starlark defect,
    // run deliberately.
    //
    // Nothing is left uncovered by ignoring it.
    // `an_injected_panic_returns_as_an_error_instead_of_unwinding` runs every
    // time and tests the boundary this module owns; this one exists to show
    // that the defect it defends against is real and reachable from four lines
    // of author Starlark.
    #[test]
    #[ignore = "allocates 4.31 GB; run with: cargo test -p egret-dataapi --lib \
                an_evaluator_panic_is_contained -- --ignored --exact"]
    fn an_evaluator_panic_is_contained_rather_than_unwinding_into_the_caller() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // 2^32 bytes — one doubling past the u32 length field.
            let _ = tx.send(eval_probe(&doubling_probe(32)));
        });
        let outcome = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the evaluator neither returned nor panicked within 60s");

        let err = outcome.expect_err("a 4 GiB string must not be built");
        match err {
            TransformError::Evaluator(msg) => assert!(
                msg.contains("len overflow"),
                "the caught panic must carry its message through to the caller, \
                 or an operator cannot tell which library defect fired: {msg}"
            ),
            other => panic!(
                "a panic must surface as TransformError::Evaluator and NOT be \
                 folded into Runtime — an operator has to be able to tell a \
                 refused script from a broken evaluator: {other:?}"
            ),
        }
    }

    // ---------------------------------------------------------------------
    // The tick budget
    // ---------------------------------------------------------------------

    #[test]
    fn an_infinite_loop_is_refused_rather_than_hanging() {
        // starlark has no built-in timeout, so this is what stands between an
        // author's `while True` and a wedged worker. Since Task 3 it is the
        // WALL-CLOCK DEADLINE that fires here, not the tick budget — measured, a
        // tight loop reaches ~406 000 ticks in the 2 s before the deadline, far
        // short of MAX_TICKS. The assertion is deliberately on "some budget
        // refused it", because which one binds is a function of the machine; the
        // two tests that discriminate are
        // `transform_tests::the_deadline_stops_a_script_the_tick_budget_would_let_run`
        // and `the_budget_refusal_is_classified_as_a_budget` below.
        //
        // THIS TEST MUST NOT BE ABLE TO HANG. Rust has no per-test timeout, so a
        // future regression in the budget would stall the whole suite rather than
        // fail one test, and a stalled suite does not look like a broken test.
        // Run the evaluation on its own thread and join with a deadline, so the
        // failure mode is a red test with a clear message.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(eval_probe(
                "def f():\n  for i in range(1, 1000000000):\n    pass\nf()",
            ));
        });
        let outcome = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("no budget stopped an unbounded loop within 10s");
        let err = outcome.expect_err("an unbounded loop must be refused");
        // Not a substring probe. `format!("{err:?}").contains("budget")` is
        // satisfied by the WORD `BudgetExceeded` no matter what payload it
        // carries, and `contains("tick")` by any message mentioning one, so the
        // old version of this assertion passed for every variant it was meant to
        // discriminate between. Match the variant and enumerate the two payloads
        // that are legitimate here, so a refusal that is really a Runtime error,
        // an evaluator panic or the heap goes red.
        assert!(
            matches!(
                err,
                TransformError::BudgetExceeded("tick budget" | "wall-clock deadline")
            ),
            "an unbounded loop must be refused by the tick budget or the \
             deadline, and by nothing else: {err:?}"
        );
    }

    /// starlark reports a busted budget as `ErrorKind::Other` around a private
    /// enum, so [`classify`] has to match on the message text. This is the test
    /// that notices if that text ever changes: without it, a reworded message
    /// would quietly demote every budget refusal to [`TransformError::Runtime`]
    /// and the test above would still pass, because the reworded message would
    /// almost certainly still contain the word "tick".
    ///
    /// # Why it drives a reduced budget rather than the shipped one
    ///
    /// Task 1 ran this on `eval_probe`, i.e. on [`MAX_TICKS`]. Task 3 installed a
    /// wall-clock deadline, and the deadline now fires first on any tick-dense
    /// script — measured, ~406 000 ticks of a 20 000 000 budget. Reaching the
    /// tick arm at the shipped budget would take ~100 s unoptimised, so this
    /// drives the budget down instead of driving the clock up.
    ///
    /// That is not a weaker test of the *installation*: 1000 ticks refuses a loop
    /// that runs 5000 times only if [`Budgets::ticks`] is actually installed —
    /// delete `set_max_tick_count` and the loop completes and this goes red. What
    /// it no longer sees is the shipped *value*, which
    /// `the_shipped_budgets_are_the_modules_constants` covers instead.
    #[test]
    fn the_budget_refusal_is_classified_as_a_budget() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(eval_inner(
                "def f():\n  for i in range(1, 5000):\n    pass\nf()",
                Budgets::default().with_ticks(1000),
                None,
            ));
        });
        let err = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the tick budget did not stop a 5000-iteration loop within 10s")
            .expect_err("a loop past the tick budget must be refused");
        assert!(
            matches!(err, TransformError::BudgetExceeded("tick budget")),
            "the tick refusal must classify as BudgetExceeded(\"tick budget\"); \
             getting Runtime here means starlark reworded the message that \
             `classify` matches on: {err:?}"
        );
    }

    /// The shipped budgets are the module's constants.
    ///
    /// **This is a weak test and is documented as one.** It reads the same
    /// constants the code reads, so it cannot notice a bad *value*; what it
    /// notices is a `Budgets::default` that has drifted away from them — which is
    /// exactly the mistake the `#[cfg(test)]` builders above make easy, since a
    /// test that forgets to reset a field would otherwise be indistinguishable
    /// from production.
    ///
    /// The behaviour of each budget is pinned elsewhere, by tests that watch a
    /// script get refused: `the_budget_refusal_is_classified_as_a_budget`,
    /// `recursion_is_refused_at_our_depth_not_the_library_default`,
    /// `the_heap_budget_refuses_a_script_that_outgrows_it`, and
    /// `transform_tests::the_deadline_stops_a_script_the_tick_budget_would_let_run`
    /// — the last of which asserts on the elapsed clock, so it does see the
    /// shipped value of [`MAX_WALL_CLOCK`].
    #[test]
    fn the_shipped_budgets_are_the_modules_constants() {
        let shipped = Budgets::default();
        assert_eq!(shipped.ticks, MAX_TICKS);
        assert_eq!(shipped.callstack, Some(MAX_CALLSTACK));
        assert_eq!(shipped.wall_clock, MAX_WALL_CLOCK);
        // The seam three output-phase tests use, pinned OFF in the shipped
        // budget. `Some(_)` here would mean a script gets a fresh deadline for
        // its output in production — the exact relaxation `run_transform`'s
        // doc comment argues against, and the one that would be invisible
        // because every other assertion in this module would still pass.
        assert_eq!(
            shipped.output_wall_clock, None,
            "a shipped evaluation must run its output phase on the SAME deadline as its evaluation; a second window here is a fresh unbounded phase for any script that overran the first"
        );
    }

    /// The output phase's deadline really is a SEPARATE one when asked for.
    ///
    /// Without this, `output_wall_clock` could be read and discarded and the
    /// only thing that would notice is the three tests it exists for — which
    /// would go green anyway, since they would then be running on one enlarged
    /// deadline. So this pins the property directly: an evaluation given a
    /// generous *evaluation* window and a zero-length *output* window must be
    /// refused by the output phase, and must name the output phase in the
    /// refusal.
    ///
    /// A zero window rather than a small one: the walk polls every
    /// `OUTPUT_POLL_STEPS`, so a deadline already in the past on arrival is the
    /// only value that makes the assertion independent of how fast this machine
    /// walks four values.
    #[test]
    fn the_output_phase_can_be_given_a_deadline_of_its_own() {
        let script = "def transform(params, statements):\n  x = []\n  for i in range(20000):\n    x.append(i)\n  return x";
        let params = std::collections::BTreeMap::new();
        let statements = std::collections::BTreeMap::new();
        let err = eval_inner(
            script,
            Budgets::default()
                .with_wall_clock(Duration::from_secs(300))
                .with_output_wall_clock(Duration::ZERO),
            Some(Entry::Transform(TransformCall {
                params: &params,
                statements: &statements,
            })),
        )
        .expect_err("an output phase whose deadline has already passed must be refused");
        assert!(
            matches!(err, TransformError::BudgetExceeded(SHAPE_DEADLINE)),
            "the OUTPUT phase must be what refuses, on its own deadline — a `wall-clock deadline` here would mean the evaluation's window was used for the output too: {err:?}"
        );
    }
}

// =========================================================================
// Task 3: the evaluator's own tests.
// =========================================================================

#[cfg(test)]
mod transform_tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    /// A [`GenerationParams`] from a list of scalars, for the `build_sql` tests
    /// below.
    ///
    /// Through the real constructor rather than a literal map, so a test cannot
    /// hand a generating script a shape production cannot produce — which is
    /// the property the type exists for.
    fn gp(entries: &[(&str, Param)]) -> GenerationParams {
        let mut out = GenerationParams::default();
        for (name, value) in entries {
            out.insert((*name).to_owned(), Some(value));
        }
        out
    }

    /// [`run_transform`] with the budgets spelled out, for the three tests
    /// whose subject is a guard in the OUTPUT phase.
    ///
    /// `run_transform` is [`Budgets::default`] and nothing else, which is the
    /// right shape for production and the wrong one for a test that names a
    /// specific output-phase refusal: one deadline covers evaluation and output
    /// together, so on a loaded machine the evaluation spends it and the guard
    /// under test never runs. See [`Budgets::output_wall_clock`] for the
    /// measurement — 3 failures out of 3 runs under contention, 0 out of 2
    /// idle.
    ///
    /// Every other test in this module still goes through `run_transform`, so
    /// this seam is used by exactly the three that need it and the shipped
    /// budget stays the thing under test everywhere else. What the enlarged
    /// numbers may NOT do is change which refusal is correct: each of the three
    /// keeps its exact assertion, and each was re-checked against a mutant of
    /// the guard it names.
    fn run_transform_under(
        spec: &TransformSpec,
        budgets: Budgets,
    ) -> Result<serde_json::Value, TransformError> {
        let params = BTreeMap::new();
        let statements = BTreeMap::new();
        eval_inner(
            &spec.script,
            budgets,
            Some(Entry::Transform(TransformCall {
                params: &params,
                statements: &statements,
            })),
        )
    }

    /// The evaluation deadline the three output-phase tests run under.
    ///
    /// Large enough that the SCRIPT finishing is not in question on any machine
    /// this suite runs on — these scripts build millions of values in an
    /// unoptimised build — and it bounds nothing those tests assert, because
    /// none of them asserts an evaluation-phase refusal.
    const SLOW_MACHINE_EVAL_WINDOW: Duration = Duration::from_secs(300);

    /// The OUTPUT deadline `a_wide_shared_fan_out_is_stopped_by_the_output_deadline`
    /// runs under, and it is small on purpose.
    ///
    /// # Why not `MAX_WALL_CLOCK`
    ///
    /// **Measured, and it is a coin flip.** That fan-out's walk is ~2 s in an
    /// unoptimised build on the machine this was written on, which is
    /// `MAX_WALL_CLOCK` to within the noise: run alone on an IDLE machine with a
    /// fresh 2 s window, the same test passed, failed and passed on three
    /// consecutive runs — the walk finishing first and reporting
    /// `OutputTooLarge`. A window a test's own subject finishes inside is not a
    /// deadline, it is a race, and it is the same class of defect as the
    /// contended failures this seam exists to fix, one machine-speed in the
    /// other direction.
    ///
    /// # Why 50 ms specifically, and what it costs
    ///
    /// The walk cannot be made much longer: it is ~10 million child iterations
    /// and it is already near `MAX_HEAP_BYTES` — `[c] * 5000000` is ~40 MB of
    /// pointers against a 64 MiB limit, so doubling it is refused by the heap
    /// rather than walked. So the window shrinks instead. 50 ms against a
    /// ~2000 ms walk is a **~40x margin** on this machine, and still ~4x on a
    /// hypothetical machine ten times faster; the deadline is polled every
    /// `OUTPUT_POLL_STEPS`, which is well under a millisecond of these steps, so
    /// nothing about the granularity fights it.
    ///
    /// **What this costs, stated rather than buried.** 50 ms lands inside the
    /// PUSH loop (a node's children are pushed before any of them are summed),
    /// so what this test now pins by wall-clock position is the child poll in
    /// the push loop. The sum loop's poll is the same `poll_output_deadline`
    /// call on the same `since_poll` counter, and deleting BOTH is what was
    /// measured against this test — it then reports
    /// `OutputTooLarge { bytes: 10000001 }`, the walk having run to completion
    /// outside every budget. Landing in the sum loop instead would require the
    /// window to exceed the push loop's duration, which is a number that varies
    /// with the machine — i.e. exactly the race above, wearing a different hat.
    const SHAPE_WALK_WINDOW: Duration = Duration::from_millis(50);

    #[test]
    fn a_transform_receives_params_and_statements_and_returns_json() {
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  \
                     return {\"r\": params[\"region\"], \"n\": len(statements[\"rows\"])}"
                .into(),
            returns: vec![],
        };
        let params = BTreeMap::from([("region".to_string(), json!("EU"))]);
        let statements =
            BTreeMap::from([("rows".to_string(), vec![json!({"a": 1}), json!({"a": 2})])]);
        let out = run_transform(&spec, &params, &statements).expect("runs");
        assert_eq!(out, json!({"r": "EU", "n": 2}));
    }

    #[test]
    fn statements_are_keyed_even_when_there_is_only_one() {
        // A script that stops working when the author adds a second statement is a
        // trap. At N=1 the key is still there.
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  return list(statements.keys())".into(),
            returns: vec![],
        };
        let statements = BTreeMap::from([("main".to_string(), vec![json!({"a": 1})])]);
        let out = run_transform(&spec, &BTreeMap::new(), &statements).expect("runs");
        assert_eq!(out, json!(["main"]));
    }

    #[test]
    fn a_script_without_the_entry_point_is_refused_by_name() {
        let spec = TransformSpec {
            script: "x = 1".into(),
            returns: vec![],
        };
        assert!(matches!(
            run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()),
            Err(TransformError::MissingEntryPoint(EntryPoint::Transform))
        ));
    }

    #[test]
    fn a_wrong_arity_entry_point_is_refused_by_name() {
        // Distinguished from MissingEntryPoint on purpose: "you named it wrong" and
        // "you took the wrong arguments" send an author to different lines.
        let spec = TransformSpec {
            script: "def transform(params):\n  return {}".into(),
            returns: vec![],
        };
        assert!(matches!(
            run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()),
            Err(TransformError::WrongArity {
                entry: EntryPoint::Transform,
                found: 1
            })
        ));
    }

    #[test]
    fn an_unserialisable_return_is_refused_rather_than_lost() {
        // Starlark can return values with no JSON counterpart. Silently dropping
        // one would serve a response missing a field the script produced.
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  return transform".into(),
            returns: vec![],
        };
        assert!(matches!(
            run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()),
            Err(TransformError::NotSerialisable(_))
        ));
    }

    #[test]
    fn an_oversized_output_is_refused_with_both_numbers() {
        let spec = TransformSpec {
            script: format!(
                "def transform(params, statements):\n  return [\"x\"] * {}",
                MAX_TRANSFORM_OUTPUT_BYTES
            ),
            returns: vec![],
        };
        // Both phases enlarged, and neither number is load-bearing: this test
        // asserts a SIZE refusal, not a time one. Building a million-element
        // list and then walking it is slow in an unoptimised build, and under
        // contention the shipped 2 s was spent before `check_output_shape`
        // reached the value count — measured, this test failed 3 runs out of 3
        // that way, with `BudgetExceeded(SHAPE_DEADLINE)`.
        match run_transform_under(
            &spec,
            Budgets::default()
                .with_wall_clock(SLOW_MACHINE_EVAL_WINDOW)
                .with_output_wall_clock(SLOW_MACHINE_EVAL_WINDOW),
        ) {
            Err(TransformError::OutputTooLarge { bytes, cap }) => {
                assert!(
                    bytes > cap,
                    "the error must carry both numbers: {bytes} vs {cap}"
                );
            }
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
    }

    /// A `transform` that is not a function at all.
    ///
    /// `MissingEntryPoint` rather than a new arm: the endpoint has no transform
    /// function either way, and the author's next move is the same line.
    #[test]
    fn a_non_callable_entry_point_reads_as_a_missing_one() {
        let spec = TransformSpec {
            script: "transform = 1".into(),
            returns: vec![],
        };
        assert!(matches!(
            run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()),
            Err(TransformError::MissingEntryPoint(EntryPoint::Transform))
        ));
    }

    /// `def transform(*args)` can be *called* with two arguments, so a
    /// call-and-see-what-happens arity check would accept it — and then the
    /// script's `params` would be a tuple, not a dict, and the failure would
    /// surface pages later as a type error inside the author's own code.
    #[test]
    fn a_varargs_entry_point_is_refused_rather_than_silently_accepted() {
        let spec = TransformSpec {
            script: "def transform(*args):\n  return len(args)".into(),
            returns: vec![],
        };
        match run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()) {
            Err(TransformError::WrongArity {
                entry: EntryPoint::Transform,
                found: 1,
            }) => {}
            other => panic!("expected WrongArity {{ found: 1 }}, got {other:?}"),
        }
    }

    /// The wall-clock deadline, which is the bound Task 1 did not have.
    ///
    /// The script loops forever. Measured, it burns ~406 000 ticks in the 2 s
    /// before the deadline fires — far short of `MAX_TICKS`, which is the whole
    /// point: **delete the `set_check_cancelled` line and this test does not
    /// fail, it runs 50x longer and then reports the tick budget instead.** So
    /// the assertion is on the classification AND on the clock.
    ///
    /// Threaded with a deadline for the reason
    /// `tests::an_infinite_loop_is_refused_rather_than_hanging` gives: Rust has
    /// no per-test timeout, and a stalled suite does not look like a broken
    /// test.
    #[test]
    fn the_deadline_stops_a_script_the_tick_budget_would_let_run() {
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  n = 0\n  \
                     for i in range(1000000000):\n    n = n + 1\n  return n"
                .into(),
            returns: vec![],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let out = run_transform(&spec, &BTreeMap::new(), &BTreeMap::new());
            let _ = tx.send((out, started.elapsed()));
        });
        // Generous: the assertion below is the one that discriminates. This
        // bound only keeps a regression from hanging the suite.
        let (out, elapsed) = rx
            .recv_timeout(MAX_WALL_CLOCK * 30)
            .expect("the deadline did not stop an unbounded loop");

        let err = out.expect_err("an unbounded loop must be refused");
        assert!(
            matches!(err, TransformError::BudgetExceeded("wall-clock deadline")),
            "the refusal must be the DEADLINE, not the tick budget — getting \
             \"tick budget\" here means `set_check_cancelled` is not installed, \
             or starlark reworded the message `classify` matches on: {err:?}"
        );
        assert!(
            elapsed >= MAX_WALL_CLOCK,
            "a refusal before the deadline is not the deadline: {elapsed:?}"
        );
    }

    /// A return value that contains itself.
    ///
    /// starlark would catch this on its own — measured, "Cycle detected when
    /// serializing value of type `list` to JSON". What would NOT survive it is
    /// [`check_output_shape`], which runs first: a memoised depth walk never
    /// assigns a depth to a node on a cycle, so without its `on_stack` set it
    /// spins between them forever and no error is ever reached.
    ///
    /// Hence the thread and the deadline. The failure this guards against is
    /// "never returns", and no assertion made on the same thread can observe it.
    #[test]
    fn a_self_referential_return_is_refused_rather_than_looping_the_depth_check() {
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  x = []\n  x.append(x)\n  return x"
                .into(),
            returns: vec![],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()));
        });
        let out = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("a self-referential return did not resolve in 20s — the depth walk is looping");
        match out {
            Err(TransformError::NotSerialisable(m)) => {
                assert!(
                    m.contains("contains itself"),
                    "the message must say what is wrong: {m}"
                );
            }
            other => panic!("expected NotSerialisable, got {other:?}"),
        }
    }

    /// The other half of the same hole: no cycle, just depth.
    ///
    /// 200 000 nested lists costs ~200 000 ticks and a few megabytes, so neither
    /// `MAX_TICKS` nor `MAX_HEAP_BYTES` comes near it, and serialising it
    /// overflows the stack. Tokio worker threads run on 2 MiB, which is smaller
    /// than the stack this test has.
    #[test]
    fn an_absurdly_deep_return_is_refused_rather_than_killing_the_process() {
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  x = []\n  \
                     for i in range(200000):\n    x = [x]\n  return x"
                .into(),
            returns: vec![],
        };
        // 200 000 loop iterations, unoptimised. Under contention that alone
        // outran the shipped 2 s deadline and the depth guard never ran —
        // measured, 3 failures out of 3, every one of them
        // `BudgetExceeded("wall-clock deadline")` from the EVALUATION. The
        // depth guard is a counter, not a clock, so neither window below can
        // decide what this asserts.
        match run_transform_under(
            &spec,
            Budgets::default()
                .with_wall_clock(SLOW_MACHINE_EVAL_WINDOW)
                .with_output_wall_clock(SLOW_MACHINE_EVAL_WINDOW),
        ) {
            Err(TransformError::BudgetExceeded("output nesting depth")) => {}
            other => panic!("expected BudgetExceeded(\"output nesting depth\"), got {other:?}"),
        }
    }

    /// A return exactly at the depth limit is allowed, so the test above is
    /// discriminating a limit rather than a blanket refusal.
    #[test]
    fn a_return_inside_the_depth_limit_is_allowed() {
        let spec = TransformSpec {
            script: format!(
                "def transform(params, statements):\n  x = 1\n  \
                 for i in range({}):\n    x = [x]\n  return x",
                MAX_JSON_DEPTH - 1
            ),
            returns: vec![],
        };
        run_transform(&spec, &BTreeMap::new(), &BTreeMap::new())
            .expect("a structure inside the depth limit must be served");
    }

    /// The exponential-expansion bomb, and the two places it could have hung.
    ///
    /// `a = [1]` then `a = [a, a]` sixty-two times is **63 allocations, a few
    /// dozen ticks and a few kilobytes** — no budget in this module is remotely
    /// near it — and it has 2^62 distinct root-to-leaf paths. Two things have to
    /// be right for this to come back at all:
    ///
    /// * [`check_output_shape`] must count the *expanded* size and refuse on it.
    ///   Depth alone waves this through — 63 is inside [`MAX_JSON_DEPTH`], and
    ///   correctly so.
    /// * it must reach that refusal without serialising anything. Measured,
    ///   letting the byte-counting sink discover the size instead took **25 s**
    ///   to reach a 2 MiB ceiling unoptimised; the walk refuses it in ~300 µs.
    ///
    /// **This test is the one a mutation caught, twice.** Its first version
    /// returned `len(a)` — an integer — so nothing deep was ever serialised and
    /// it passed with the memoisation deleted. Returning `a` made it bite, and
    /// what it then bit was the design: there was no expanded-size check at all,
    /// only a byte-counting sink that took 25 s to get there.
    ///
    /// Deleting the memoisation still leaves **this** test green, at 2.39 s
    /// instead of 0.03 s, because on a pure doubling structure the bottom-up
    /// size refusal fires at level 21 regardless. That is a fact about this
    /// shape and not about the memo — see
    /// `a_fanned_out_shared_subtree_terminates_rather_than_hanging`, which is
    /// the shape where the memo is the difference between a refusal and a hang,
    /// and [`check_output_shape`] for why generalising from this one was wrong.
    ///
    /// Threaded, because the failure mode being guarded against is "never
    /// returns", which no assertion can catch from the same thread.
    #[test]
    fn a_shared_structure_terminates_rather_than_expanding_exponentially() {
        let spec = TransformSpec {
            script: format!(
                "def transform(params, statements):\n  a = [1]\n  \
                 for i in range({}):\n    a = [a, a]\n  return a",
                MAX_JSON_DEPTH - 2
            ),
            returns: vec![],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()));
        });
        let out = rx.recv_timeout(std::time::Duration::from_secs(20)).expect(
            "63 shared nodes did not resolve in 20s — either the depth walk is \
             walking paths rather than nodes, or the output sink has no ceiling",
        );
        match out {
            Err(TransformError::OutputTooLarge { bytes, cap }) => {
                assert!(bytes > cap, "{bytes} vs {cap}");
            }
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
    }

    /// Cheap to build, and a hang without the memoisation.
    ///
    /// Each shared subtree stays **under** every cap, so the bottom-up size
    /// refusal that saves the doubling bomb above never gets to fire: `c`
    /// expands to 786 431 values, inside [`MAX_TRANSFORM_OUTPUT_BYTES`], and only
    /// the root exceeds it. A memo-less walk must therefore finish all 100 000
    /// copies of `c` before it can refuse anything — ~7.9e10 visits, hours at
    /// the measured ~1.1 µs each.
    ///
    /// The script costs **~19 ticks and under a megabyte**. No budget in this
    /// module is anywhere near it; the memo in [`check_output_shape`] is the
    /// only thing standing between it and a wedged worker.
    ///
    /// Threaded with a deadline, because the failure being guarded against is
    /// "never returns".
    #[test]
    fn a_fanned_out_shared_subtree_terminates_rather_than_hanging() {
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  c = [1]\n  \
                     for i in range(18):\n    c = [c, c]\n  return [c] * 100000"
                .into(),
            returns: vec![],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()));
        });
        let out = rx.recv_timeout(std::time::Duration::from_secs(20)).expect(
            "a fanned-out shared subtree did not resolve in 20s — the output walk \
             is visiting paths rather than nodes",
        );
        match out {
            Err(TransformError::OutputTooLarge { bytes, cap }) => {
                assert!(bytes > cap, "{bytes} vs {cap}");
            }
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
    }

    /// The output SHAPE WALK is inside the deadline.
    ///
    /// **Four distinct values and forty megabytes**, and ten million child
    /// iterations. `d = [c, c]` is there to make `c` *finish* before `e` is
    /// walked — `[e, d]` pops `d` first — so every one of `e`'s five million
    /// children is already known by the time `e` is reached. That is the point:
    /// `e` then costs **two outer turns** and five million child iterations in
    /// the push loop, and five million more in the sum loop.
    ///
    /// An earlier version of this test used `[[1]] * 5000000`, which does not
    /// discriminate: there the children are *not* yet known, so all five million
    /// get pushed and pop as five million outer turns, and a poll counting only
    /// outer turns catches it anyway. **Measured — that version stayed green
    /// with the child polls deleted.** This shape is the one where the poll has
    /// to be inside the child loops.
    ///
    /// The walk runs after `eval_function` has returned, so it earns no ticks
    /// and starlark checks nothing for it. Without the polls it eventually
    /// refuses with `OutputTooLarge` — correct, and arrived at outside every
    /// budget — so the assertion is on which refusal.
    #[test]
    fn a_wide_shared_fan_out_is_stopped_by_the_output_deadline() {
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  c = [1]\n  \
                     d = [c, c]\n  e = [c] * 5000000\n  return [e, d]"
                .into(),
            returns: vec![],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // The EVALUATION gets a window it cannot plausibly overrun, and the
            // OUTPUT phase gets a small one of its own, taken fresh when it
            // begins. That is the whole recalibration: this test's subject is
            // the walk being cut off by ITS deadline, and it still is — what it
            // no longer depends on is the machine. On one deadline shared with
            // the evaluation it failed 3 contended runs out of 3 with the SHAPE
            // poll perfectly intact; on a fresh `MAX_WALL_CLOCK` it failed 1
            // idle run out of 3, the walk finishing first. See
            // `SHAPE_WALK_WINDOW` for both measurements.
            let _ = tx.send(run_transform_under(
                &spec,
                Budgets::default()
                    .with_wall_clock(SLOW_MACHINE_EVAL_WINDOW)
                    .with_output_wall_clock(SHAPE_WALK_WINDOW),
            ));
        });
        // Long enough that the UNPOLLED walk finishes and fails the assertion
        // below, rather than timing out and blaming the harness. Sized against
        // the evaluation window above rather than against `MAX_WALL_CLOCK`,
        // which is no longer what bounds this thread.
        let out = rx
            .recv_timeout(SLOW_MACHINE_EVAL_WINDOW * 2)
            .expect("the output walk never returned at all");

        // `assert_eq!` on the payload, NOT `matches!(err,
        // BudgetExceeded(SHAPE_DEADLINE))`. Measured: that version passed with
        // the poll below it deleted, because the walk then overran into the
        // SINK's poll and the sink's different label satisfied the pattern
        // anyway. Comparing the string is what makes this name one phase.
        match out.expect_err("a walk past its deadline must be refused") {
            TransformError::BudgetExceeded(which) => assert_eq!(
                which, SHAPE_DEADLINE,
                "the SHAPE WALK must be what refuses; the sink's own deadline \
                 catching it later means the walk itself ran unpolled"
            ),
            other => panic!(
                "an OutputTooLarge or a success here means the walk ran to \
                 completion outside every budget: {other:?}"
            ),
        }
    }

    /// The SERIALISATION is inside the deadline too, and it is a separate poll.
    ///
    /// This shape is chosen so the walk above cannot be what refuses it: `c`
    /// has 37 distinct values and expands to 786 431, which is **inside**
    /// [`MAX_TRANSFORM_OUTPUT_BYTES`], so `check_output_shape` passes it in
    /// microseconds. Serialising it emits ~1.5 million tokens, and measured, a
    /// `serde_json` token through this sink costs ~12 µs unoptimised — about 18
    /// seconds.
    ///
    /// [`OUTPUT_COUNT_CEILING`] does not help: 1.5 million tokens is well under
    /// 16 MiB of bytes. Time and size are different axes and this is the one
    /// only the clock catches.
    #[test]
    fn a_slow_serialisation_is_stopped_by_the_output_deadline() {
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  c = [1]\n  \
                     for i in range(18):\n    c = [c, c]\n  return c"
                .into(),
            returns: vec![],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()));
        });
        let out = rx
            .recv_timeout(MAX_WALL_CLOCK * 60)
            .expect("the serialisation never returned at all");

        match out.expect_err("a serialisation past its deadline must be refused") {
            TransformError::BudgetExceeded(which) => assert_eq!(which, SINK_DEADLINE),
            other => panic!(
                "the sink must refuse on the deadline; an OutputTooLarge here \
                 means it counted all ~1.5M tokens first, outside every \
                 budget: {other:?}"
            ),
        }
    }

    /// [`MAX_JSON_DEPTH`] is applied on the way IN, not only on the way out.
    ///
    /// The input half has its own call site in [`json_to_starlark`] and its own
    /// refusal, and without a test that feeds deep *input* it is invisible:
    /// deleting the check leaves every other depth test green, because a script
    /// that builds its own deep value never goes through it. Measured — that
    /// mutation survived until this test existed.
    ///
    /// The two refusals are named differently for the same reason. Deleting the
    /// input check does not make this pass with a different message; it makes it
    /// report `"output nesting depth"`, which is a different line for the author
    /// to look at and the wrong one.
    #[test]
    fn deep_input_is_refused_on_the_way_in_and_says_so() {
        let deeper = MAX_JSON_DEPTH + 1;
        let mut row = json!(1);
        for _ in 0..deeper {
            row = serde_json::Value::Array(vec![row]);
        }
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  return statements[\"rows\"][0]".into(),
            returns: vec![],
        };
        let statements = BTreeMap::from([("rows".to_string(), vec![row])]);
        match run_transform(&spec, &BTreeMap::new(), &statements) {
            Err(TransformError::BudgetExceeded("input nesting depth")) => {}
            other => panic!(
                "input nested past the limit must be refused as INPUT — \
                 an \"output nesting depth\" here means json_to_starlark let it \
                 through and check_output_shape caught it on the way back: \
                 {other:?}"
            ),
        }
    }

    /// A single value that is small in count and large in bytes.
    ///
    /// This is the case `check_output_shape` cannot see: one string is one
    /// value, so the expanded-size half waves it through and only
    /// [`CappedSink`] can refuse it. Without this test the byte cap has no
    /// negative case left — the brief's `["x"] * 1048576` is now refused on the
    /// value count first.
    #[test]
    fn one_oversized_string_is_refused_by_the_byte_cap_not_the_value_count() {
        let spec = TransformSpec {
            script: format!(
                "def transform(params, statements):\n  return \"z\" * {}",
                MAX_TRANSFORM_OUTPUT_BYTES + 1
            ),
            returns: vec![],
        };
        match run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()) {
            Err(TransformError::OutputTooLarge { bytes, cap }) => {
                assert_eq!(cap, MAX_TRANSFORM_OUTPUT_BYTES);
                // The two quotes make it three over, so this is the SERIALISED
                // length and not the value count (which is 1).
                assert_eq!(bytes, MAX_TRANSFORM_OUTPUT_BYTES + 3);
            }
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
    }

    /// Few values, enormous bytes — the case only the sink's ceiling stops.
    ///
    /// `["z" * 1 MiB] * 32` is **34 values and one megabyte of heap**, because
    /// the list holds the same string thirty-two times. `check_output_shape`
    /// passes it: 34 is nothing. It serialises to ~33 MiB, and without
    /// [`OUTPUT_COUNT_CEILING`] the sink would copy every one of those bytes to
    /// report a number nobody needs — for a thousand ten-megabyte strings that is
    /// ten gigabytes.
    ///
    /// The assertion is that `bytes` is a **lower bound**, not the true size:
    /// that is what distinguishes a sink that stopped from one that ran to the
    /// end.
    #[test]
    fn a_few_enormous_strings_stop_at_the_counting_ceiling() {
        let spec = TransformSpec {
            script: format!(
                "def transform(params, statements):\n  return [\"z\" * {}] * 32",
                MAX_TRANSFORM_OUTPUT_BYTES
            ),
            returns: vec![],
        };
        let true_size = 32 * (MAX_TRANSFORM_OUTPUT_BYTES + 2) + 33;
        match run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()) {
            Err(TransformError::OutputTooLarge { bytes, cap }) => {
                assert_eq!(cap, MAX_TRANSFORM_OUTPUT_BYTES);
                assert!(bytes > cap, "{bytes} vs {cap}");
                assert!(
                    bytes < true_size,
                    "counting must STOP at the ceiling: got {bytes}, which is the \
                     whole {true_size} bytes, so the sink ran to the end"
                );
            }
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
    }

    /// [`MAX_JSON_DEPTH`] must survive the stack serving actually runs on.
    ///
    /// Not a style point. Measured, ~20 KB of stack goes per level of nesting,
    /// and a tokio worker gets 2 MiB — so the abort threshold is ~104 and the
    /// libtest thread this would otherwise run on (8 MiB) is four times more
    /// forgiving than production. Testing on the default thread would therefore
    /// pass at a depth that kills a worker.
    ///
    /// If someone raises [`MAX_JSON_DEPTH`] past what 2 MiB can take, this does
    /// not go red — it aborts the test binary with SIGABRT, because that is what
    /// a stack overflow is and what production would do with it.
    #[test]
    fn our_depth_limit_survives_a_tokio_sized_stack() {
        let spec = TransformSpec {
            script: format!(
                "def transform(params, statements):\n  x = 1\n  \
                 for i in range({MAX_JSON_DEPTH}):\n    x = [x]\n  return x"
            ),
            returns: vec![],
        };
        let done = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()))
            .expect("spawn")
            .join()
            .expect("the depth limit must not overflow a 2 MiB stack");
        done.expect("a return exactly at the depth limit must be served");
    }

    /// The gap between our limit and `serde_json`'s is deliberate, and this is
    /// where a future reader learns it did not happen by accident.
    ///
    /// `serde_json` accepts 128 levels; we accept [`MAX_JSON_DEPTH`]. Everything
    /// in between parses fine and is refused here — see that constant for the
    /// measurement that says why the alternative is an abort, not a wider limit.
    #[test]
    fn serde_json_accepts_deeper_json_than_this_module_will_carry() {
        let deeper = MAX_JSON_DEPTH + 1;
        let text = format!("{}1{}", "[".repeat(deeper), "]".repeat(deeper));
        serde_json::from_str::<serde_json::Value>(&text).expect(
            "serde_json must still accept nesting past our limit — if it stops, \
             the gap this documents has closed and MAX_JSON_DEPTH could be \
             revisited",
        );

        let spec = TransformSpec {
            script: format!(
                "def transform(params, statements):\n  x = 1\n  \
                 for i in range({deeper}):\n    x = [x]\n  return x"
            ),
            returns: vec![],
        };
        match run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()) {
            Err(TransformError::BudgetExceeded("output nesting depth")) => {}
            other => panic!("expected the depth refusal, got {other:?}"),
        }
    }

    /// Shared structure is not itself illegal — the refusal above is about size.
    ///
    /// Without this, a `check_output_shape` that refused every repeated node
    /// would pass the test above for the wrong reason.
    #[test]
    fn a_value_referenced_twice_is_served_twice() {
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  a = [1, 2]\n  return [a, a]".into(),
            returns: vec![],
        };
        let out = run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()).expect("runs");
        assert_eq!(out, json!([[1, 2], [1, 2]]));
    }

    /// The input conversion runs in Rust and earns no ticks, so nothing starlark
    /// checks fires while it allocates. Measured before the poll existed: a
    /// million rows reached a peak of 335 540 992 bytes against a 64 MiB limit.
    ///
    /// This uses few, fat rows rather than many thin ones so the test costs
    /// milliseconds rather than seconds while crossing the same line.
    #[test]
    fn the_heap_budget_applies_to_the_input_and_not_only_to_the_script() {
        // Sized so the heap is crossed several `INPUT_POLL_ROWS` batches in:
        // 2048 rows of 32 KiB is 64 MiB, and the poll before row 3072 is the one
        // that must see it. Fat rows rather than many thin ones so the test
        // costs milliseconds and ~96 MiB rather than the seconds a million
        // ordinary rows would take.
        let fat = "y".repeat(32 * 1024);
        let rows: Vec<serde_json::Value> = (0..3072).map(|_| json!({ "v": fat })).collect();
        let spec = TransformSpec {
            script: "def transform(params, statements):\n  return len(statements[\"rows\"])".into(),
            returns: vec![],
        };
        let statements = BTreeMap::from([("rows".to_string(), rows)]);
        match run_transform(&spec, &BTreeMap::new(), &statements) {
            Err(TransformError::BudgetExceeded("heap budget, converting its input")) => {}
            other => panic!(
                "96 MiB of input must be refused while it is being converted, \
                 not after the whole of it is on the heap — a plain \"heap \
                 budget\" here means the end-of-evaluation check caught it and \
                 `poll_input_budgets` did not: {other:?}"
            ),
        }
    }

    /// The cap is on the SERIALISED bytes, and the sink that counts them must
    /// still produce the value when it fits. A cap enforced by refusing
    /// everything would pass the oversized test above on its own.
    #[test]
    fn an_output_just_inside_the_cap_is_served_intact() {
        let spec = TransformSpec {
            script: format!(
                "def transform(params, statements):\n  return \"z\" * {}",
                MAX_TRANSFORM_OUTPUT_BYTES - 3
            ),
            returns: vec![],
        };
        let out = run_transform(&spec, &BTreeMap::new(), &BTreeMap::new())
            .expect("a payload inside the cap must be served");
        let text = out.as_str().expect("a string");
        assert_eq!(text.len(), MAX_TRANSFORM_OUTPUT_BYTES - 3);
        assert!(
            text.bytes().all(|b| b == b'z'),
            "the payload must be intact"
        );
    }

    // ---------------------------------------------------------------------
    // `build_sql`: the third entry point.
    // ---------------------------------------------------------------------

    /// The template a script builds must reach the caller unrendered.
    ///
    /// `{{ region }}` is the whole safety argument for this package: the
    /// braces have to survive `build_sql` verbatim so the bound value still
    /// goes through `render()` later, exactly as it would for a
    /// hand-written statement's SQL.
    #[test]
    fn build_sql_returns_the_template_the_script_built() {
        let script = "def build_sql(params):\n  return \"SELECT {{ region }} AS r\"";
        let out = build_sql(script, &GenerationParams::default()).expect("runs");
        assert_eq!(
            out, "SELECT {{ region }} AS r",
            "the braces must survive verbatim"
        );
    }

    /// `params` reaches the script, and the script can branch on it — the
    /// same dict shape `run_transform` hands `transform`, minus `statements`.
    #[test]
    fn build_sql_sees_params_and_can_branch_on_them() {
        let script = "def build_sql(params):\n  \
                      if params[\"grain\"] == \"daily\":\n    return \"SELECT 1 AS daily\"\n  \
                      return \"SELECT 1 AS hourly\"";
        let daily = gp(&[("grain", Param::Str("daily".into()))]);
        let hourly = gp(&[("grain", Param::Str("hourly".into()))]);
        assert_eq!(build_sql(script, &daily).unwrap(), "SELECT 1 AS daily");
        assert_eq!(build_sql(script, &hourly).unwrap(), "SELECT 1 AS hourly");
    }

    /// A script that never defines `build_sql` is refused by the same
    /// variant a missing `transform` is — see [`TransformError::MissingEntryPoint`]
    /// — carrying [`EntryPoint::BuildSql`] rather than the transform contract.
    ///
    /// Named `_for_build_sql` rather than reusing
    /// `a_script_without_the_entry_point_is_refused_by_name` verbatim: that
    /// name is already taken above, by the same assertion against
    /// `run_transform`. Same behaviour, different entry point, so both keep
    /// their own name rather than one shadowing the other.
    #[test]
    fn a_script_without_the_entry_point_is_refused_by_name_for_build_sql() {
        assert!(matches!(
            build_sql("x = 1", &GenerationParams::default()),
            Err(TransformError::MissingEntryPoint(EntryPoint::BuildSql))
        ));
    }

    /// **The message an author reads names the function they were asked to
    /// write.** Task 2 shipped `MissingEntryPoint` and `WrongArity` rendering
    /// `` `transform` `` and `(params, statements)` unconditionally, which was
    /// unobservable while `build_sql` had no non-test caller and became a
    /// wrong sentence the moment publish acquired one.
    ///
    /// Asserted on the rendered TEXT, not on the variant: the variant was
    /// already right, and it is the text that would have sent an author to
    /// define a function this phase never looks up. The transform half is
    /// asserted alongside so that "name the contract" cannot be satisfied by
    /// swapping which of the two is wrong.
    #[test]
    fn a_refusal_names_the_entry_point_the_author_was_asked_to_write() {
        let missing_build_sql = build_sql("x = 1", &GenerationParams::default())
            .expect_err("`x = 1` defines no `build_sql`")
            .to_string();
        assert_eq!(
            missing_build_sql,
            "the script must define `build_sql(params)`"
        );

        let wrong_arity_build_sql = build_sql(
            "def build_sql(params, statements):\n  return \"\"",
            &GenerationParams::default(),
        )
        .expect_err("`build_sql` takes one argument")
        .to_string();
        assert_eq!(
            wrong_arity_build_sql,
            "`build_sql` must take exactly 1 arguments (params), but takes 2"
        );

        let spec = TransformSpec {
            script: "x = 1".into(),
            returns: vec![],
        };
        assert_eq!(
            run_transform(&spec, &BTreeMap::new(), &BTreeMap::new())
                .expect_err("`x = 1` defines no `transform`")
                .to_string(),
            "the script must define `transform(params, statements)`",
            "the transform contract must keep its own wording"
        );

        let spec = TransformSpec {
            script: "def transform(params):\n  return {}".into(),
            returns: vec![],
        };
        assert_eq!(
            run_transform(&spec, &BTreeMap::new(), &BTreeMap::new())
                .expect_err("`transform` takes two arguments")
                .to_string(),
            "`transform` must take exactly 2 arguments (params, statements), but takes 1"
        );
    }

    // ---------------------------------------------------------------------
    // Package S Task 8.5: the emitted template's length (spec S5).
    // ---------------------------------------------------------------------

    /// **The measurement behind [`MAX_GENERATED_SQL_BYTES`], in the code.**
    ///
    /// Spec S5 says this cap needs measuring rather than choosing, so the
    /// measurement lives here where the suite re-takes it, not only in a
    /// report. The script is spec S1's shape scaled to a statement an analyst
    /// would actually publish: four optional dimensions, six metrics, two CTEs,
    /// three joins, a grain chosen from a dict, two optional filters, and every
    /// caller value left as `{{ }}` for the renderer.
    ///
    /// The band is narrow on purpose. This is a fixture, so its size is a
    /// constant — a test that only asserted "under the cap" would keep passing
    /// while the thing being measured drifted, and the ratio below would then
    /// be a ratio to nothing.
    /// The bottom of the band [`MAX_GENERATED_SQL_BYTES`] must sit in, as a
    /// multiple of a realistic generated statement. C2's ratio: a cap an honest
    /// script bumps into refuses work that should have been allowed.
    const HEADROOM_MIN: usize = 40;

    /// The top of it. `2.5x` C2's ratio, and the reason there is a top at all is
    /// in the assertion that uses it.
    const HEADROOM_MAX: usize = 100;

    #[test]
    fn a_realistic_branching_statement_sizes_the_generated_sql_cap() {
        let script = r#"
DIMENSIONS = {
    "region": "c.region",
    "segment": "c.segment",
    "channel": "b.channel",
    "category": "p.category",
}
METRICS = {
    "revenue": "sum(b.total)",
    "orders": "count(*)",
    "customers": "count(distinct b.customer_id)",
    "aov": "sum(b.total) / nullif(count(*), 0)",
    "refunds": "sum(b.refund_total)",
    "units": "sum(l.quantity)",
}
GRAIN = {
    "day": "date_trunc('day', o.ordered_at)",
    "week": "date_trunc('week', o.ordered_at)",
    "month": "date_trunc('month', o.ordered_at)",
}

def build_sql(params):
    dims = [DIMENSIONS[d] for d in sorted(DIMENSIONS) if params.get("by_" + d)]
    metrics = [METRICS[m] + " AS " + m for m in sorted(METRICS)]
    where = ["o.ordered_at >= {{ from_date }}", "o.ordered_at < {{ to_date }}"]
    if params.get("channel"):
        where.append("o.channel = {{ channel }}")
    if params.get("min_total"):
        where.append("o.total >= {{ min_total }}")
    select = ["b.bucket"] + dims + metrics
    group = ["b.bucket"] + dims
    return ("WITH base AS (\n" +
        "  SELECT o.id, o.customer_id, o.channel, o.total, o.refund_total,\n" +
        "         " + GRAIN[params["grain"]] + " AS bucket\n" +
        "  FROM sales.orders o\n" +
        "  WHERE " + "\n    AND ".join(where) + "\n" +
        "    AND o.status IN ('complete', 'shipped')\n" +
        "), enriched AS (\n" +
        "  SELECT b.*, l.quantity, c.region, c.segment, p.category\n" +
        "  FROM base b\n" +
        "  JOIN sales.order_lines l ON l.order_id = b.id\n" +
        "  JOIN sales.customers c ON c.id = b.customer_id\n" +
        "  JOIN sales.products p ON p.id = l.product_id\n" +
        ")\n" +
        "SELECT " + ", ".join(select) + "\n" +
        "FROM enriched b\n" +
        "GROUP BY " + ", ".join(group) + "\n" +
        "ORDER BY b.bucket\n" +
        "LIMIT {{ row_limit }}")
"#;
        let params = gp(&[
            ("grain", Param::Str("month".into())),
            ("by_region", Param::Bool(true)),
            ("by_segment", Param::Bool(true)),
            ("by_channel", Param::Bool(true)),
            ("by_category", Param::Bool(true)),
            ("channel", Param::Str("web".into())),
            ("min_total", Param::I64(10)),
        ]);
        let sql = build_sql(script, &params).expect("a realistic script runs");

        // The script is itself a quarter of `MAX_TRANSFORM_SCRIPT_BYTES`, which
        // is what makes it a fair sample of the upper end rather than a toy.
        assert!(
            (1800..1950).contains(&script.len()),
            "the fixture drifted: {} bytes",
            script.len()
        );
        assert!(
            (900..1000).contains(&sql.len()),
            "the emitted template drifted: {} bytes",
            sql.len()
        );
        assert!(
            sql.contains("{{ region }}") || sql.contains("{{ channel }}"),
            "the caller's values must still be unrendered placeholders: {sql}"
        );
        // BOTH directions. A floor alone is not a cap: it is satisfied by 1 MiB,
        // by 16 MiB and by `usize::MAX`, so a test carrying only the floor
        // cannot fail in the direction a cap exists for — someone raising the
        // number to unblock an author would keep every test green. The ceiling
        // is C2's own argument turned around: "a cap an honest script never
        // approaches is not a cap at all".
        //
        // The band is what the measurement actually decides. It does NOT decide
        // 65536 — see `MAX_GENERATED_SQL_BYTES`, which says so.
        assert!(
            MAX_GENERATED_SQL_BYTES >= HEADROOM_MIN * sql.len(),
            "the cap ({MAX_GENERATED_SQL_BYTES}) must leave a realistic \
             statement ({}) the same order of headroom \
             MAX_TRANSFORM_SCRIPT_BYTES leaves C2's example",
            sql.len()
        );
        assert!(
            MAX_GENERATED_SQL_BYTES <= HEADROOM_MAX * sql.len(),
            "the cap ({MAX_GENERATED_SQL_BYTES}) is more than {HEADROOM_MAX}x a \
             realistic statement ({}) — at that distance it stops being a cap \
             and the only thing bounding a generated statement is \
             render::MAX_SQL_BYTES again",
            sql.len()
        );

        // The SCRIPT's own cap, sized against the same fixture in the same
        // call, because the two numbers are decided by the same measurement and
        // splitting them across two tests is how a fixture drifts under one of
        // them. Two-sided for the reason above: a floor alone is satisfied by
        // axum's 2 MB, which is exactly the state this cap was added to leave.
        //
        // A DIFFERENT band from `MAX_GENERATED_SQL_BYTES`', and deliberately:
        // this cap is also the multiplier on publish's 1040 parses, so the
        // ceiling is what stops those becoming gigabytes. See
        // `egret_dataapi_api::MAX_STATEMENT_SCRIPT_BYTES`.
        const SCRIPT_HEADROOM_MIN: usize = 4;
        const SCRIPT_HEADROOM_MAX: usize = 16;
        assert!(
            egret_dataapi_api::MAX_STATEMENT_SCRIPT_BYTES >= SCRIPT_HEADROOM_MIN * script.len(),
            "the script cap ({}) leaves a realistic generating script ({}) less \
             than {SCRIPT_HEADROOM_MIN}x room for comments, a helper `def` and \
             a second branch",
            egret_dataapi_api::MAX_STATEMENT_SCRIPT_BYTES,
            script.len()
        );
        assert!(
            egret_dataapi_api::MAX_STATEMENT_SCRIPT_BYTES <= SCRIPT_HEADROOM_MAX * script.len(),
            "the script cap ({}) is more than {SCRIPT_HEADROOM_MAX}x a realistic \
             generating script ({}) — and this one multiplies: publish parses a \
             statement's script up to 1040 times",
            egret_dataapi_api::MAX_STATEMENT_SCRIPT_BYTES,
            script.len()
        );
        assert!(
            egret_dataapi_api::MAX_STATEMENT_SCRIPT_BYTES
                >= crate::lifecycle::MAX_TRANSFORM_SCRIPT_BYTES,
            "a generating script decides a statement's structure, so it may not \
             be capped tighter than a response transform"
        );
    }

    /// A template past [`MAX_GENERATED_SQL_BYTES`] is refused, with both
    /// numbers.
    ///
    /// The evaluator's existing budgets do not reach this: `"z" * 70000` is one
    /// instruction and a few tens of kilobytes against a 64 MiB heap, so a
    /// script can emit a megabyte well inside every limit C2 installed. The
    /// only backstop before this rule was `render::MAX_SQL_BYTES`, a downstream
    /// error at render time rather than the author-facing cap spec S5 asks for.
    #[test]
    fn a_template_over_the_cap_is_refused_by_the_cap_not_by_a_budget() {
        let over = MAX_GENERATED_SQL_BYTES + 1;
        let script = format!("def build_sql(params):\n  return \"z\" * {over}");
        let err = build_sql(&script, &GenerationParams::default()).expect_err("over the cap");
        match &err {
            TransformError::GeneratedSqlTooLarge { bytes, cap } => {
                assert_eq!(*bytes, over, "the exact size, not a lower bound");
                assert_eq!(*cap, MAX_GENERATED_SQL_BYTES);
            }
            other => panic!("expected a length refusal, not a budget one: {other:?}"),
        }
        assert_eq!(
            err.to_string(),
            format!(
                "build_sql returned a {over} byte template, over the \
                 {MAX_GENERATED_SQL_BYTES} byte cap"
            ),
            "both numbers, so an author knows how much to cut"
        );
    }

    /// The boundary is inclusive: exactly the cap is a template, one more is
    /// not.
    ///
    /// Two assertions rather than one because an off-by-one here is invisible
    /// to a test that only pushes something obviously huge through — and this
    /// cap is the sole reason the size of a generated statement is bounded at
    /// all, so its edge is worth naming.
    #[test]
    fn a_template_of_exactly_the_cap_is_accepted() {
        let script = format!("def build_sql(params):\n  return \"z\" * {MAX_GENERATED_SQL_BYTES}");
        let sql = build_sql(&script, &GenerationParams::default()).expect("exactly the cap fits");
        assert_eq!(sql.len(), MAX_GENERATED_SQL_BYTES);
    }

    /// The cap counts BYTES, not characters.
    ///
    /// `MAX_TRANSFORM_SCRIPT_BYTES` and `render::MAX_SQL_BYTES` are both byte
    /// counts, and a `.chars().count()` here would let a script emitting
    /// multi-byte text produce up to four times the memory those numbers
    /// bound. Starlark strings are UTF-8, so this is `str::len` and the test
    /// says which one it is.
    #[test]
    fn the_cap_counts_bytes_rather_than_characters() {
        // Two bytes each, so half the cap in characters is exactly the cap in
        // bytes, and one character more is over it.
        let chars = MAX_GENERATED_SQL_BYTES / 2 + 1;
        let script = format!("def build_sql(params):\n  return \"\\u00e9\" * {chars}");
        match build_sql(&script, &GenerationParams::default()).expect_err("over the cap in BYTES") {
            TransformError::GeneratedSqlTooLarge { bytes, .. } => {
                assert_eq!(bytes, chars * 2, "a character count would say {chars}");
            }
            other => panic!("expected a length refusal: {other:?}"),
        }
    }

    /// A script returning a dict or a list is an author mistake. Coercing it
    /// would hand the renderer something that is not SQL and let the failure
    /// surface far from its cause.
    #[test]
    fn a_script_returning_a_non_string_is_refused_rather_than_stringified() {
        let script = "def build_sql(params):\n  return {\"sql\": \"SELECT 1\"}";
        assert!(build_sql(script, &GenerationParams::default()).is_err());
    }

    /// The evaluator is C2's. An unbounded loop must be refused, not run —
    /// and this test must FAIL rather than hang if that stops being true, so
    /// it evaluates on its own thread with a deadline: Rust has no per-test
    /// timeout, and a budget regression here would otherwise stall the suite
    /// rather than fail one test.
    #[test]
    fn build_sql_is_bounded_by_the_same_budgets_as_a_transform() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(build_sql(
                "def build_sql(params):\n  for i in range(1, 1000000000):\n    pass\n  return \"x\"",
                &GenerationParams::default(),
            ));
        });
        let outcome = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the tick budget did not stop an unbounded loop within 30s");
        assert!(outcome.is_err(), "an unbounded loop must be refused");
    }
}
