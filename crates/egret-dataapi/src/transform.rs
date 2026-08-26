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
//! builtins, no clock and no source of randomness — those are absent from the
//! grammar and from [`Globals::standard`], not removed by us afterwards. `db`,
//! `http`, `tx` and `cache` are unreachable for the same reason a script cannot
//! reach the clock: there is nothing in scope to reach them with, and this crate
//! injects nothing. [`tests::a_script_cannot_reach_the_filesystem_or_the_clock`]
//! is what holds that claim to account.
//!
//! Two things the language does NOT give us for free, both handled here:
//!
//! 1. `load()` IS part of `Dialect::Standard`, and it is the one construct in
//!    the grammar that names an external module. [`transform_dialect`] turns it
//!    off, so `load(...)` does not parse.
//! 2. There is **no built-in timeout**. Termination is bought with
//!    [`Evaluator::set_max_tick_count`] and nothing else, which is why
//!    [`tests::an_infinite_loop_is_refused_rather_than_hanging`] exists and why
//!    it is written so it cannot hang.

use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};

/// Ceiling on evaluation steps, where a "tick" is one function call or one loop
/// backedge. Starlark has no timeout, so this is the *only* thing that ends a
/// runaway script.
///
/// PROVISIONAL. Task 3 measures a representative transform with
/// `Evaluator::get_total_tick_count` and sets this from evidence; until then it
/// is a guess sized to leave headroom over a few-thousand-row reshape while
/// still failing an unbounded loop in well under a second.
const MAX_TICKS: u64 = 1_000_000;

/// Ceiling on the Starlark heap, in bytes. Best-effort by the library's own
/// admission — it is checked periodically rather than on allocation — so it is a
/// guard against ordinary runaway growth, not against a determined adversary.
///
/// PROVISIONAL, same as [`MAX_TICKS`].
const MAX_HEAP_BYTES: usize = 64 * 1024 * 1024;

/// Ceiling on Starlark call-stack depth.
///
/// Deliberately BELOW starlark's own default of 50 (`DEFAULT_STACK_SIZE`, which
/// `Evaluator` falls back to when no limit is set). That relationship is the
/// whole point and is easy to get backwards: a value above 50 would *loosen*
/// what starlark already enforces while still reading like a safety measure, and
/// a test that only tries unbounded recursion passes either way, because the
/// default catches that on its own. See
/// `tests::recursion_is_refused_at_our_depth_not_the_library_default`.
///
/// 24 frames is generous for a response reshape; PROVISIONAL like [`MAX_TICKS`],
/// and Task 3 revisits it against a real transform.
const MAX_CALLSTACK: usize = 24;

/// Enforce the relationship [`MAX_CALLSTACK`] documents, at COMPILE time.
///
/// Raising the constant above starlark's default would not break a test, it
/// would silently make the test that guards it vacuous — the script it runs
/// would start hitting the library's limit instead of ours and still be
/// refused. A build error is the only way this cannot be missed.
const _: () = assert!(
    MAX_CALLSTACK < 50,
    "MAX_CALLSTACK must stay below starlark's DEFAULT_STACK_SIZE of 50; above it \
     this constant loosens the library's own limit instead of tightening it"
);

/// The dialect a transform is parsed under.
///
/// `Dialect::Standard` with `enable_load` turned OFF. That single change is
/// load-bearing: `load()` is the only construct in standard Starlark that refers
/// to something outside the script's own text, and leaving it enabled would mean
/// the sandbox depended on us never installing a `FileLoader` — a promise about
/// future code rather than a property of the parse.
fn transform_dialect() -> Dialect {
    Dialect {
        enable_load: false,
        ..Dialect::Standard
    }
}

/// What can go wrong when a transform runs.
///
/// Deliberately minimal: Task 3 elaborates this as the transform grows a
/// contract. The three arms exist now because the distinction they draw is
/// already real — a script that does not parse, a script that parses and then
/// misbehaves, and a script that was cut off for exceeding a budget are three
/// different things to tell an endpoint author.
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
}

/// Parse and evaluate `src` under the transform sandbox, discarding the value.
///
/// The evaluation surface the tests measure, and the one Task 3 builds the real
/// transform on top of. It installs every limit
/// `starlark 0.14` offers and injects **nothing** — no globals beyond
/// [`Globals::standard`], no loader, no extension values.
#[allow(dead_code)]
fn eval_probe(src: &str) -> Result<(), TransformError> {
    let ast = AstModule::parse("transform.star", src.to_owned(), &transform_dialect())
        .map_err(|e| TransformError::Parse(e.to_string()))?;

    let globals = Globals::standard();

    // `Module::new` does not exist in starlark 0.14 — a module is handed to a
    // closure so its heap is scoped. Everything the evaluator touches therefore
    // lives inside here.
    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);

        // Each of these returns `Err` only for a zero value or a second call on
        // the same evaluator, i.e. only for a bug in THIS function. Treating
        // that as a budget failure keeps a mis-wired limit from silently
        // evaluating unbounded.
        eval.set_max_tick_count(MAX_TICKS)
            .map_err(|_| TransformError::BudgetExceeded("tick budget (failed to install)"))?;
        eval.set_max_heap_size(MAX_HEAP_BYTES)
            .map_err(|_| TransformError::BudgetExceeded("heap budget (failed to install)"))?;
        eval.set_max_callstack_size(MAX_CALLSTACK)
            .map_err(|_| TransformError::BudgetExceeded("call-stack budget (failed to install)"))?;

        eval.eval_module(ast, &globals).map_err(classify)?;
        Ok(())
    })
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

    #[test]
    fn a_script_cannot_reach_the_filesystem_or_the_clock() {
        // The whole language choice rests on this: safety is a property of the
        // LANGUAGE, not of a list of globals we remembered to strip. If any of
        // these evaluate, that premise is false and the choice must be reopened.
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
    ///   failed" — but it exercises the parser, not the global environment, and
    ///   the report must not count it as one of the four.
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

    /// `load()` is the one construct in standard Starlark that names something
    /// outside the script, and `Dialect::Standard` leaves it ON. The brief's
    /// five probes do not cover it, which would have left the sandbox resting on
    /// "we never installed a `FileLoader`" — a promise about future code.
    /// [`transform_dialect`] turns it off in the grammar instead.
    #[test]
    fn load_is_not_part_of_the_transform_dialect() {
        assert!(
            matches!(
                eval_probe("load('//other:defs.bzl', 'thing')"),
                Err(TransformError::Parse(_))
            ),
            "load() must not parse; if it does, the sandbox depends on no one \
             ever calling Evaluator::set_loader"
        );
    }

    /// The third budget — and the one that nearly shipped untested.
    ///
    /// The obvious test is unbounded recursion. It is worthless: starlark
    /// already refuses that unaided, because `Evaluator` falls back to
    /// `DEFAULT_STACK_SIZE` (50) when no limit is set. Measured, not reasoned —
    /// deleting the `set_max_callstack_size` call left that test GREEN.
    ///
    /// So this one recurses to a depth between the two: deeper than
    /// [`MAX_CALLSTACK`] (24) and shallower than starlark's default (50). It can
    /// only pass if OUR constant is the limit doing the work; drop the call and
    /// the script runs to completion and returns 0.
    #[test]
    fn recursion_is_refused_at_our_depth_not_the_library_default() {
        // The `MAX_CALLSTACK < 50` premise this test rests on is enforced by a
        // `const _: () = assert!(…)` next to the constant, so it is a build
        // error rather than something this test has to remember to check.

        // ~37 frames: comfortably past 24, comfortably short of 50.
        let probe = "def f(n):\n  if n == 0:\n    return 0\n  return f(n - 1)\nf(35)";

        let err = eval_probe(probe).expect_err(
            "recursion 35 deep must be refused by MAX_CALLSTACK; if it succeeded, \
             set_max_callstack_size is not being applied and the only thing \
             bounding recursion is starlark's default",
        );
        assert!(
            matches!(err, TransformError::BudgetExceeded("call-stack depth")),
            "the refusal must be the call-stack budget, not the tick budget: {err:?}"
        );
    }

    /// Unbounded recursion terminates too. This asserts far less than the test
    /// above — starlark's default would catch it unaided — but it is the case an
    /// author will actually hit, and it pins that the outcome is a refusal
    /// rather than a native stack overflow taking the process with it.
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
