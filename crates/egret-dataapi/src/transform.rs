//! C2: reshaping a Data API response with a small author-supplied script.
//!
//! Task 1 establishes nothing but the *floor*: the evaluator, its budgets, and
//! the tests that pin what a script may not do. The transform itself is Task 3.
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
//!    ~2 GiB for about 31 ticks**. Bounding wall-clock needs
//!    `Evaluator::set_check_cancelled`, which is available and deliberately NOT
//!    wired here — see [`MAX_TICKS`].
//! 3. **The evaluator can panic on author input.** Not "should never happen" — a
//!    four-line script reaches it. [`eval_inner`] contains it; see there.

use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect, DialectTypes};

/// Ceiling on evaluation steps, where a "tick" is one function call or one loop
/// backedge.
///
/// **This bounds work, not time.** Because a tick is a call or a backedge and
/// not an instruction, a script doing enormous work per tick is barely charged:
/// `s = s + s` repeated 31 times inside one loop ran for 3.2 seconds on ~31
/// ticks. A wall-clock bound needs `Evaluator::set_check_cancelled`, which takes
/// a `Box<dyn Fn() -> bool>` and therefore needs a deadline or cancellation
/// token to consult. Task 1 has nothing to connect one to, so it is left for
/// Task 3 — which should treat it as the missing bound, not a refinement.
///
/// PROVISIONAL. Task 3 measures a representative transform with
/// `Evaluator::get_total_tick_count` and sets this from evidence.
const MAX_TICKS: u64 = 1_000_000;

/// Ceiling on the Starlark heap, in bytes.
///
/// **Measured, so that nobody mistakes this for a bound.** starlark checks the
/// heap periodically rather than on allocation, and the overshoot is not small:
/// against this 64 MiB limit, a script doubling a string 31 times was refused
/// only after allocating roughly **2 GiB** — a 32x overshoot — and one more
/// doubling panics the evaluator outright (see [`eval_inner`]). So this catches
/// ordinary runaway growth and nothing more; it does not make the process's
/// memory use bounded, and the library's own documentation says as much.
///
/// PROVISIONAL, same as [`MAX_TICKS`].
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
/// 24 frames is generous for a response reshape; PROVISIONAL like [`MAX_TICKS`],
/// and Task 3 revisits it against a real transform.
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
    /// The script parsed but failed while running — an unbound name, a type
    /// error, an explicit `fail()`.
    #[error("the transform script failed while running: {0}")]
    Runtime(String),
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
#[allow(dead_code)]
fn eval_probe(src: &str) -> Result<(), TransformError> {
    eval_inner(src, Some(MAX_CALLSTACK))
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

/// The body of [`eval_probe`], with the call-stack budget made optional.
///
/// `max_callstack: None` means "install no limit of ours and let starlark fall
/// back to its own". Nothing outside `#[cfg(test)]` passes `None`; it exists so
/// the guard test can establish that our budget — and not starlark's private
/// fallback — is what refuses the recursion probe, by *running* the probe both
/// ways instead of comparing against a number copied out of the library.
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
fn eval_inner(src: &str, max_callstack: Option<usize>) -> Result<(), TransformError> {
    let ast = AstModule::parse("transform.star", src.to_owned(), &transform_dialect())
        .map_err(|e| TransformError::Parse(e.to_string()))?;

    let globals = transform_globals();

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
            eval.set_max_tick_count(MAX_TICKS)
                .map_err(|_| TransformError::BudgetExceeded("tick budget (failed to install)"))?;
            eval.set_max_heap_size(MAX_HEAP_BYTES)
                .map_err(|_| TransformError::BudgetExceeded("heap budget (failed to install)"))?;
            if let Some(frames) = max_callstack {
                eval.set_max_callstack_size(frames).map_err(|_| {
                    TransformError::BudgetExceeded("call-stack budget (failed to install)")
                })?;
            }

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
            Ok(())
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
        _ => TransformError::Runtime(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                eval_inner(name, Some(MAX_CALLSTACK)).is_ok(),
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
            match eval_inner(name, Some(MAX_CALLSTACK)) {
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
            match eval_inner(&format!("{name}.anything()"), Some(MAX_CALLSTACK)) {
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
            eval_inner(&probe, None).is_ok(),
            "recursion {RECURSION_PROBE_DEPTH} deep must COMPLETE under \
             starlark's own fallback limit — if it does not, the refusal below \
             proves nothing about MAX_CALLSTACK, because the library would have \
             refused it anyway"
        );

        // A `match`, not `expect_err`: that takes a `&str`, so the depths could
        // not be formatted into the message that has to explain the failure.
        let err = match eval_inner(&probe, Some(MAX_CALLSTACK)) {
            Err(e) => e,
            Ok(()) => panic!(
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
        // The tick budget IS the defence — starlark has no built-in timeout.
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
            .expect("the tick budget did not stop an unbounded loop within 10s");
        let err = outcome.expect_err("an unbounded loop must be refused");
        assert!(
            format!("{err:?}").to_lowercase().contains("tick")
                || format!("{err:?}").to_lowercase().contains("budget"),
            "the refusal must name the budget it hit: {err:?}"
        );
    }

    /// starlark reports a busted budget as `ErrorKind::Other` around a private
    /// enum, so [`classify`] has to match on the message text. This is the test
    /// that notices if that text ever changes: without it, a reworded message
    /// would quietly demote every budget refusal to [`TransformError::Runtime`]
    /// and the test above would still pass, because the reworded message would
    /// almost certainly still contain the word "tick".
    #[test]
    fn the_budget_refusal_is_classified_as_a_budget() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(eval_probe(
                "def f():\n  for i in range(1, 1000000000):\n    pass\nf()",
            ));
        });
        let err = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the tick budget did not stop an unbounded loop within 10s")
            .expect_err("an unbounded loop must be refused");
        assert!(
            matches!(err, TransformError::BudgetExceeded("tick budget")),
            "the tick refusal must classify as BudgetExceeded(\"tick budget\"); \
             getting Runtime here means starlark reworded the message that \
             `classify` matches on: {err:?}"
        );
    }
}
