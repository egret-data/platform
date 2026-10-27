//! `If` and `Switch` — the two flowables that pick a task list by a value the
//! **caller** rendered.
//!
//! One file, because they are one shape: choose a list, then run that list the
//! way [`sequential`](super::sequential) runs any list. Kestra builds them the
//! same way — `If.resolveNexts` and `Switch.resolveNexts` both delegate to
//! `FlowableUtils.resolveSequentialNexts` over whatever `childTasks()` chose
//! (`core/src/main/java/io/kestra/plugin/core/flow/{If,Switch}.java`) — so the
//! only thing either module owns is the choice.
//!
//! # `advance()` cannot render, and that is why `rendered` exists
//!
//! Kestra evaluates the expression through a `RunContext`, which reaches
//! inputs, variables, task outputs and the KV store. `advance()` is pure, so
//! the caller renders and writes the answer into
//! [`TaskRun::rendered`](crate::executor::TaskRun::rendered) — the field Task 3
//! added for exactly this and that nothing wrote until now.
//!
//! `rendered: None` therefore means "the caller has not answered yet", and the
//! only correct response to it is to emit nothing. It is **not** an error and
//! it is **not** a falsy value: reading it as falsy would silently take the
//! `else` branch of every `If` the caller had not got to.

use crate::executor::plan::{Plan, TaskIx};
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

use super::{sequential, Step};

/// Kestra's `TruthUtils.isTruthy`, over the value the caller rendered.
///
/// `core/src/main/java/io/kestra/core/utils/TruthUtils.java` is
/// `condition != null && !List.of("false", "0", "-0", "").contains(condition.trim())`,
/// and this is **not** what a Rust reader would write: it is neither
/// `parse::<bool>()`, nor "non-empty", nor a case-insensitive match, and `"0"`
/// is falsy while `"00"` is not.
///
/// # `condition.trim()` is not `str::trim()`, and the difference is silent
///
/// The line above transliterates to `rendered.trim()` and that is **wrong in
/// both directions**. Java's `String.trim()` strips leading and trailing code
/// units `<= U+0020` and nothing else; Rust's [`str::trim`] strips Unicode
/// `White_Space`, which is a different set that neither contains nor is
/// contained by the other. Measured live on `kestra/kestra:v2.0.0-rc13`, 18
/// flows with a literal condition, and **11 of them disagreed with
/// `str::trim()`**:
///
/// | rendered | Kestra | `str::trim()` would give |
/// |---|---|---|
/// | `"\u{1}false\u{1}"`, `"\u{E}false\u{E}"`, `"\u{1F}false\u{1F}"`, `"\u{1}0\u{1}"` | `else` | `then` — Rust does not strip C0 controls |
/// | `"\u{A0}false\u{A0}"` (NBSP), `"\u{85}…"`, `"\u{3000}…"`, `"\u{2007}…"`, `"\u{2028}…"`, `"\u{A0}0\u{A0}"`, `"\u{A0}"` | `then` | `else` — Rust strips these, Java does not |
///
/// So the predicate trims **exactly the codepoints `<= U+0020`**, which is
/// Java's rule stated as a rule rather than as a method name. `U+000B` and
/// `U+000C` are in both sets and agree, which is why a smaller probe would have
/// missed this entirely — and so did the original 20-row measurement, because
/// **every one of its padded rows padded with an ASCII space.**
///
/// This is exactly the silent-wrong-branch class this module's doc warns about:
/// an NBSP in a rendered data value is not exotic. Nothing in the corpus reaches
/// it, so no baselined entry moves; the committed rows are what hold it.
///
/// # Measured, not read
///
/// The rule was measured against the running artifact before this line was
/// written: 20 flows, each an `If` with a **literal** condition — so the
/// rendered value is exactly the literal and the measurement isolates this
/// predicate from the renderer — published to `kestra/kestra:v2.0.0-rc13` and
/// executed, recording which branch got a taskrun.
///
/// | rendered | branch |
/// |---|---|
/// | `"true"`, `"1"`, `"00"`, `"0.0"`, `"abc"`, `"FALSE"`, `"False"`, `"null"`, `"no"`, `"[]"`, `"{}"`, `"true\n"`, `"  true  "` | `then` |
/// | `"false"`, `"0"`, `"-0"`, `""`, `"  false  "`, `" 0 "`, `"   "` | `else` |
///
/// The three padded rows are what pin *that* there is a trim, and
/// `"FALSE"`/`"False"`/`"no"`/`"null"` are what pin "four exact strings"
/// against a looser reading. **They do not pin WHICH trim** — every one of them
/// pads with an ASCII space, where every candidate agrees. The 18 rows of the
/// section above are what pin that, and they were measured only after a review
/// asked the question.
/// `kestra_truthiness_is_four_exact_strings_compared_after_trimming` in
/// `executor/mod.rs` carries all 38 rows as assertions.
///
/// **Kestra's own Javadoc on `If.condition` is wrong about this**, which is a
/// third reason not to take the rule from a reading: it says "Boolean coercion
/// allows 0, -0, null and '' to evaluate to false", and a rendered `"null"`
/// takes `then` — measured. There is no `null` in `FALSE_VALUES`.
///
/// The `!= null` half of the Java is not here: a Java `null` condition is
/// [`build_plan`](crate::executor::build_plan)'s refusal (an `If` with no
/// `condition`), and a *missing render* is [`Option::None`] at the call site
/// above, not a falsy string.
pub fn is_truthy(rendered: &str) -> bool {
    // `trim_matches(|c| c <= '\u{20}')`, NOT `trim()`: see the section above for
    // the 11 measured rows where the two differ, in both directions.
    !matches!(
        rendered.trim_matches(|c: char| c <= '\u{20}'),
        "false" | "0" | "-0" | ""
    )
}

/// Drive one `If`.
///
/// # The empty branch SUCCEEDS
///
/// Kestra's `If.resolveState` wraps `childTasks()` in `ListUtils.emptyOnNull`
/// and short-circuits an empty list into
/// `execution.guessFinalState(null, parentTaskRun, …)`, which over no taskruns
/// is `SUCCESS`. The golden says the same thing: `valids/if`, driven with
/// `if2: false`, carries `if-2` as `CREATED RUNNING SUCCESS` with **no child
/// run at all**, and `valids/if-without-else`'s measured alternative
/// (`param: false`) is one taskrun and `SUCCESS`.
///
/// That is not a property of `else` being *absent* rather than empty: an
/// explicit `else: []` with a falsy condition also ended `SUCCESS` with one
/// taskrun on the running artifact. Java reaches both through the same
/// `emptyOnNull`.
///
/// **There is no special case here for it, and that is deliberate.** An early
/// `if chosen.is_empty() { return Step::Done(Success) }` was written first and
/// then deleted: [`sequential::drive`](super::sequential::drive) over an empty
/// list already falls out of its loop with `Done(Success)`, so a mutation that
/// removed the early return killed nothing at all. A guard nobody can see fail
/// is the pattern this package has already shipped three times, so the branch
/// went rather than the mutation.
///
/// **A `Switch` in the same position FAILS** — see [`drive_switch`]. The two
/// are not one rule, and writing them as one is the mistake this pair of doc
/// comments exists to prevent.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_if(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    then_: &[TaskIx],
    else_: &[TaskIx],
    errors: &[TaskIx],
    finally: &[TaskIx],
    now: Tick,
) -> Step {
    let Some(rendered) = state.runs[run].rendered.as_deref() else {
        return Step::Waiting;
    };
    let chosen = if is_truthy(rendered) { then_ } else { else_ };
    // An empty `chosen` ends the `If` without its `finally:`
    // (`If.java:188-198`): `current_list`'s first arm.
    sequential::drive(plan, state, chosen, errors, finally, Some(run), now)
}

/// Drive one `Switch`.
///
/// The case whose key **equals** the rendered value, else `default`. Kestra's
/// `Switch.childTasks` is
/// `cases.entrySet().stream().filter(entry -> entry.getKey().equals(value))
/// .findFirst().orElse(resolveTasks(defaults))` — an equality on the key and
/// nothing else: no prefix, no regex, no case folding, and no ordering.
///
/// # The three answers, and why "empty" is two different things
///
/// - **A case matched.** Its list is driven sequentially. If that list is
///   *empty* this returns [`Step::Waiting`] **for ever**, which is Kestra's
///   behaviour and not an omission: an empty matched list is still a
///   `findFirst()` hit, so `defaults` is not reached, and `resolveState` on an
///   empty-but-non-null list falls to its last branch and answers
///   `Optional.empty()`. Measured live — a flow with `cases: {MATCH: []}` and
///   a non-empty `defaults`, run with `value: MATCH`, was still `RUNNING` after
///   45 s with only its own taskrun.
/// - **No case matched, and there is a `default`.** It is driven sequentially.
/// - **No case matched and no `default`: `FAILED`.** `resolveTasks(null)` is
///   `null`, and `FlowableUtils.resolveState` logs "No task found on flow" and
///   returns `FAILED` for a null list. Measured in the golden twice —
///   `valids/fail-on-switch` (`param: fail`) and `valids/switch-impossible`
///   (`string: impossible`) are each one run, `CREATED RUNNING FAILED`.
///
/// **This is where a `Switch` differs from an [`If`](drive_if)**, which
/// succeeds in the same position, and the cause is exactly that `If` overrides
/// `resolveState` to wrap the list in `emptyOnNull` first and `Switch` does
/// not.
///
/// # What this cannot represent
///
/// An empty `default` here means Kestra's `defaults: null` OR an absent
/// `defaults:` — and since the null-list fix (`wire::null_is_absent`) it means
/// both, because they now parse identically. On the artifact those two agree
/// too: Jackson leaves the field null either way, and with no matching case the
/// `Switch` ends `FAILED`. `valids/switch-impossible` is that flow, and it is
/// banked — egret reproduces its `FAILED` exactly.
///
/// What still cannot be represented is the third spelling, `defaults: []`: on
/// the artifact an empty-but-non-null list leaves the `Switch` `RUNNING` for
/// ever (measured live, same probe as above), and an empty `Vec` here cannot
/// hold that apart from the other two. **Nothing in the corpus writes
/// `defaults: []`** (measured over all 351 flows), so nothing reaches it.
/// Closing it needs an `Option<Vec<Task>>` in W1's model and a third arm here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_switch(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    cases: &[(String, Vec<TaskIx>)],
    default: &[TaskIx],
    errors: &[TaskIx],
    finally: &[TaskIx],
    now: Tick,
) -> Step {
    let Some(rendered) = state.runs[run].rendered.as_deref() else {
        return Step::Waiting;
    };
    match cases.iter().find(|(key, _)| key == rendered) {
        Some((_, chosen)) if chosen.is_empty() => Step::Waiting,
        Some((_, chosen)) => {
            sequential::drive(plan, state, chosen, errors, finally, Some(run), now)
        }
        None if default.is_empty() => Step::Done(RunState::Failed),
        None => sequential::drive(plan, state, default, errors, finally, Some(run), now),
    }
}
