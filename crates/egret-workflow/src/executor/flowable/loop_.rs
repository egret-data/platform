//! `Loop` — Kestra's `ForEach` under its real class name — and
//! [`Command::SpawnExecution`], the command that runs one iteration.
//!
//! # A `Loop` does not iterate inside its execution
//!
//! This is the whole shape of the flowable and it is measured rather than
//! read. Each value produces a **separate sub-execution** carrying `parentId`,
//! and the body's tasks never get a run in the parent. Two derivations, one per
//! half: **41** golden entries carry **142** sub-executions between them
//! (`the_sub_executions_are_compared_and_this_is_what_they_cost` — the name was
//! `the_spawned_field_is_all_that_stands_between_these_entries_and_a_bank`, and
//! the sentence it made stopped being true when W3b Task 4 built the spawner;
//! corrected here by W6 Task 3, the first package to open this directory),
//! and across the **39** comparable entries that hold a `Loop` not one of the
//! **82** tasks inside a loop body has a run in its parent — on either side
//! (`no_run_is_ever_created_for_a_task_inside_a_loop`, which checks Kestra's own
//! golden as the control). So this module emits no [`Command::CreateRun`] at all — that is what
//! [`Plan::is_loop_body`](crate::executor::Plan::is_loop_body) is for — and
//! everything under `tasks:` is planned, walked and then left alone.
//!
//! # Why only the FIRST batch is spawned, and why that is a derivation
//!
//! [`advance`](crate::executor::advance) is a pure function of `(plan, state,
//! now)`, and [`ExecutionState`] holds **nothing** about a sub-execution: no
//! id, no state, no count. Two consequences follow, and neither is a
//! preference:
//!
//! - **The spawns must all be emitted in one round.** A round that emitted
//!   some and left the rest would be asked again against a state that records
//!   neither, and would emit the same ones for ever. So the emission is tied to
//!   the one transition that happens exactly once — `CREATED` to `RUNNING` —
//!   and [`open`] is the only place it can live.
//! - **The batch is the first one and there is no second.** Kestra spawns
//!   `min(limit, size)` iterations, then one more each time an iteration
//!   terminates (`Loop.java:405-424` and `LoopExecutionEventMessageHandler`).
//!   The "each time one terminates" half needs an iteration outcome, which is
//!   the thing this crate cannot represent, so W3b owns it.
//!
//! What that costs is exact and is in `oracle-exec/README.md`: the loop's own
//! terminal state is supplied by the caller rather than decided here, and every
//! iteration after the first batch is neither spawned nor compared.
//!
//! # The third shape of `values:`, an ION file, is read by the WORKER
//!
//! Kestra documents three shapes and [`iteration_values`] implements two. The
//! third, an ION file URI, was a named gap here until W9a Task 11
//! (2026-10-03); it is closed in `worker::render`, before `advance()` is asked,
//! so this module still performs no I/O -- see [`iteration_values`].

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx, TaskKind};
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

use super::{start, Step};

/// What a `CREATED` `Loop` run needs.
///
/// Three answers, all three measured on the artifact, and the first two are why
/// this is reached at `CREATED` rather than at `RUNNING`:
///
/// - **No values: `SUCCESS`, without ever running.** `valids/loop-empty`
///   (`values: '[]'`) carries `1_each` as `CREATED SUCCESS` with **no
///   `RUNNING` and no attempt**, and the flow goes on to its next task. A
///   `Loop` that started first and then noticed it had nothing to do would
///   produce `CREATED RUNNING SUCCESS` and one attempt, which is a different
///   trace.
/// - **Unresolvable values: `FAILED`, the same way.** `valids/loop-null`
///   (`values: '[1, null, {…}]'`) carries `1_each` as `CREATED FAILED`, no
///   attempt, and the execution `FAILED`. Kestra's own cause is
///   `FlowableUtils.resolveValues` throwing "Found a null value inside the
///   iteration values" before any iteration exists.
/// - **Values: start, and spawn the first batch.** `CREATED RUNNING …` with one
///   attempt, which is what every other golden `Loop` run carries.
///
/// `rendered` is [`None`] until the caller has rendered `values`, and the only
/// correct answer to that is [`Step::Waiting`] — the same rule, and the same
/// reason, as an unrendered `If`.
pub(crate) fn open(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    now: Tick,
) -> Step {
    let TaskKind::Loop {
        concurrency_limit, ..
    } = plan.tasks[task].kind
    else {
        unreachable!("flowable::open dispatches here only for a TaskKind::Loop");
    };

    let Some(rendered) = state.runs[run].rendered.as_deref() else {
        return Step::Waiting;
    };
    let Ok(values) = resolve_loop_values(rendered) else {
        return Step::Commands(vec![Command::SetRunState {
            run,
            state: RunState::Failed,
        }]);
    };
    if values.is_empty() {
        return Step::Commands(vec![Command::SetRunState {
            run,
            state: RunState::Success,
        }]);
    }

    let limit = first_batch(concurrency_limit, values.len());
    let spawn = |value| Command::SpawnExecution {
        task,
        value,
        parent_run: run,
    };
    let mut cmds = start(plan, run, task, now);
    match values {
        LoopValues::Listed(values) => cmds.extend(values.into_iter().take(limit).map(spawn)),
        // A file's values are not in the state (see [`rendered_from_file`]):
        // each spawn carries an EMPTY value, and the worker's spawner reads
        // the iteration's value by its index (`worker::spawn`).
        LoopValues::FromFile { .. } => cmds.extend((0..limit).map(|_| spawn(String::new()))),
    }
    Step::Commands(cmds)
}

/// How many iterations Kestra starts at once, given the limit and the number of
/// values.
///
/// `Loop.java:422`, verbatim:
/// `int limit = this.concurrencyLimit == 0 ? size : Math.min(this.concurrencyLimit, size);`
///
/// **`0` is unlimited and `1` is full serialisation**, which is the opposite
/// way round from what a reader who has just met `Parallel::concurrent`
/// expects — that field defaults to `0`, this one defaults to `1`
/// (`Loop.java:233`, `@Builder.Default private Integer concurrencyLimit = 1;`).
/// Treating `0` as a cap of zero would spawn nothing and hang every unlimited
/// loop in the corpus; treating the default as `0` would spawn every value of
/// every loop at once, which
/// `the_default_concurrency_limit_is_one_and_the_corpus_can_tell` shows Kestra
/// does not do.
fn first_batch(concurrency_limit: u32, size: usize) -> usize {
    if concurrency_limit == 0 {
        size
    } else {
        (concurrency_limit as usize).min(size)
    }
}

/// The iteration values, as the strings Kestra puts on `loopRun.value`.
///
/// This is `FlowableUtils.resolveValues`'s **String** arm
/// (`core/src/main/java/io/kestra/core/runners/FlowableUtils.java:418-455`),
/// because a string is the only shape that reaches here: Kestra's
/// `Loop.values` is a bare `Object`, so a YAML sequence arrives as a `List` and
/// a quoted JSON string arrives as a `String`, while W1's
/// `Property<serde_json::Value>` flattens **both** spellings to the same JSON
/// text before this crate ever sees them.
///
/// # The three answers
///
/// - **A JSON array** — one value per element, in order. A string element is
///   itself; anything else is its compact JSON; a `null` element is an error,
///   which is exactly what fails `valids/loop-null`.
/// - **A JSON object** — one value per entry, in key order, each entry's
///   *value* (not its key, and not the pair). Measured on `valids/loop-map`,
///   whose `values:` is `{key1: value 1, key2: value 2, key3: value 3}` and
///   whose three sub-executions carry `"value 1"`, `"value 2"`, `"value 3"`.
/// - **Anything else** — an error. Kestra's own message is "Unknown value type:
///   NUMBER" for a scalar, and unparseable text throws before that.
///
/// # The THIRD shape, an ION file URI, is resolved before this is called
///
/// `Loop.java:210-214` documents three shapes; the third is "an ION file URI,
/// each line will be deserialized as an ION object then coalesced to a
/// string". Kestra does not reach `resolveValues` for it:
/// `FlowableUtils.resolveLoopValuesUri` (`FlowableUtils.java:519`) renders
/// `values`, asks `URIFetcher.supports`, and `Loop.initFromUri`
/// (`Loop.java:404`) reads the file.
///
/// **Until W9a Task 11 this was a NAMED GAP here**: a rendered `kestra:///….ion`
/// arrived as a plain string, was not JSON, and took the error arm, which
/// failed `valids/loop-outputs-auto` and `valids/loop-values-from-uri` (both
/// `values: "{{ outputs.write.uri }}"`) inside the executor whatever the
/// render context held.
///
/// **Since W9a Task 11 (2026-10-03) the worker reads the file** --
/// `worker::render::render_expressions` and `Surface::loop_values_from_file`,
/// in `settle`, BEFORE `advance()` is asked: the URI is resolved through the
/// render snapshot (the one readability decision), the file is read whole as
/// bytes, capped by `workflow.storage.max_read_bytes` (no loop-width limit
/// exists to reuse), parsed one top-level Ion value at a time by
/// `storage::ion` (text or binary Ion), each value coalesced as Kestra's
/// `FlowableUtils.ionValueToString` (`storage::ion::loop_values`, Kestra's
/// decimals, doubles and `java.time` forms), and what lands in
/// [`TaskRun::rendered`](crate::executor::TaskRun::rendered) -- the caller's
/// answer, as for an `If` -- is [`rendered_from_file`]'s form: the URI and
/// how many records the file holds. The records themselves go to
/// `workflow.loop_file_values` (fix round 1: `rendered` is read by every load
/// of the execution, and a values file is up to 10 MiB). This function
/// refuses that form; [`resolve_loop_values`] counts it, and [`open`] stays
/// pure: it spawns the first batch with empty values, which the worker's
/// spawner fills by index, every record an iteration.
///
/// **No `distinct()` for a file** (the user's ruling of 2026-10-03; until
/// then the array went down the array arm and a record the file held twice
/// iterated once): Kestra's URI path, `readLoopValuesFromUri`, has none.
/// One consequence remains, registered by W9a Task 11: the file is read whole
/// before the first iteration, so a `null` record fails the loop before any
/// iteration where Kestra fails it at the batch that reaches it.
/// `valids/loop-values-from-uri` banks; `valids/loop-outputs-auto` iterates as
/// Kestra does and is still baselined on the `Loop`'s own `outputs:` (STORE
/// mode, `executions/{iteration}/{n}.ion`), which egret does not produce --
/// out of W9a by the user's ruling of 2026-10-03 (spec §15).
///
/// # `distinct()` is on the array arm and NOT on the object arm
///
/// Kestra deduplicates the array's resolved values
/// (`.stream().distinct().toList()`, twice — once in each of its two array
/// arms) and does not deduplicate the object's -- nor a file's (above). All
/// three are reproduced. No corpus
/// flow has a duplicate value, so
/// `kestra_deduplicates_a_list_of_values_and_not_a_map_of_them` is what holds
/// it rather than an entry.
///
/// # Where this is knowingly WIDER than Kestra, and why it cannot be narrowed
///
/// Kestra's `List` arm resolves each element through `valueAsString`, which
/// accepts `String`, `Number` and `Map` and **throws for a boolean or a nested
/// list**; its `String` arm serialises any non-null element and accepts both.
/// So on the artifact `values: [true]` fails and `values: '[true]'` succeeds.
/// Egret takes the `String` arm for both, because W1's `Property` has already
/// flattened them into one text and the distinction is gone before this
/// function is called. It is the same class of model gap as `defaults: []`
/// versus an absent `defaults:` that [`drive_switch`](super::branch::drive_switch)
/// records, in a different field, and closing it means widening W1's model
/// rather than anything here. `every_corpus_loop_value_is_a_shape_both_kestra_
/// arms_accept` measures that no corpus flow is either shape.
///
/// # The bound
///
/// There is no recursion here. `serde_json`'s own parser bounds nesting depth
/// at 128 and returns `Err` past it, which lands on the `FAILED` arm of
/// [`open`] rather than on the stack.
pub fn iteration_values(rendered: &str) -> Result<Vec<String>, String> {
    if values_name_a_file(rendered) {
        return Err(format!(
            "values were read from a file ({}); they are read by index, not listed",
            rendered.lines().next().unwrap_or(rendered)
        ));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(rendered).map_err(|e| format!("values is not JSON: {e}"))?;
    match parsed {
        serde_json::Value::Array(items) => {
            let mut out: Vec<String> = Vec::with_capacity(items.len());
            for item in items {
                let value = match item {
                    serde_json::Value::Null => {
                        // Kestra: "Found a null value inside the iteration
                        // values=…". `valids/loop-null` is the corpus entry.
                        return Err("found a null value inside the iteration values".to_string());
                    }
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                // Kestra's `.distinct()`, which keeps the FIRST occurrence.
                if !out.contains(&value) {
                    out.push(value);
                }
            }
            Ok(out)
        }
        serde_json::Value::Object(entries) => entries
            .into_iter()
            .map(|(_, value)| match &value {
                serde_json::Value::Null => {
                    Err("found a null value inside the iteration values".to_string())
                }
                serde_json::Value::String(s) => Ok(s.clone()),
                serde_json::Value::Number(_) | serde_json::Value::Object(_) => {
                    Ok(value.to_string())
                }
                // Kestra's `valueAsString` has no arm for a boolean or a nested
                // list: "Unknown value element type". The object arm is the one
                // place both Kestra spellings go through `valueAsString`, so
                // this one CAN be reproduced exactly.
                _ => Err("unknown value element type in the iteration values".to_string()),
            })
            .collect(),
        other => Err(format!(
            "unknown values type: {}",
            match other {
                serde_json::Value::Null => "NULL",
                serde_json::Value::Bool(_) => "BOOLEAN",
                serde_json::Value::Number(_) => "NUMBER",
                serde_json::Value::String(_) => "STRING",
                _ => unreachable!("arrays and objects are handled above"),
            }
        )),
    }
}

/// Kestra's `URIFetcher.supports(String)`, verbatim: a `Loop`'s rendered
/// `values:` names a FILE when it STARTS with `kestra://`, `file://` or
/// `nsfile://` (`URIFetcher.java:19,65-67`) -- nothing is trimmed, and
/// anything else is one of the two JSON shapes.
///
/// The worker routes on it (`worker::render::render_expressions` reads the
/// file), and [`resolve_loop_values`] on it again to know that `rendered` is
/// [`rendered_from_file`]'s form: a rendered text that names a file never
/// reaches `rendered` as itself, so the two cannot be confused.
pub fn values_name_a_file(rendered: &str) -> bool {
    ["kestra://", "file://", "nsfile://"]
        .iter()
        .any(|scheme| rendered.starts_with(scheme))
}

/// A `Loop` run's values, as its `rendered` holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopValues {
    /// An inline `values:`: [`iteration_values`]'s list.
    Listed(Vec<String>),
    /// A file's (W9a Task 11, fix round 1): `count` records, NOT in the
    /// state. The worker wrote them to `workflow.loop_file_values`, one row
    /// per record in file order, when it rendered the run, and reads an
    /// iteration's value from there by its index when it spawns it.
    FromFile { count: usize },
}

impl LoopValues {
    /// How many iterations the loop has.
    pub fn len(&self) -> usize {
        match self {
            LoopValues::Listed(values) => values.len(),
            LoopValues::FromFile { count } => *count,
        }
    }

    /// Whether the loop has no iteration.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A `Loop` run's `rendered`, resolved: [`iteration_values`] for an inline
/// `values:`, the count for [`rendered_from_file`]'s form.
pub fn resolve_loop_values(rendered: &str) -> Result<LoopValues, String> {
    if !values_name_a_file(rendered) {
        return iteration_values(rendered).map(LoopValues::Listed);
    }
    let Some((uri, count)) = rendered.rsplit_once('\n') else {
        return Err(format!(
            "values names the file {rendered}, which the worker has not read"
        ));
    };
    count
        .parse()
        .map(|count| LoopValues::FromFile { count })
        .map_err(|e| format!("the count of the values read from {uri} is not a number: {e}"))
}

/// What the worker stores in a `Loop` run's `rendered` once it has read the
/// file `uri` names: the URI, a newline, and how many records it holds --
/// **not the records** (W9a Task 11, fix round 1). `rendered` is read by
/// every load of the execution and returned by the task-run API, and a
/// values file is up to `workflow.storage.max_read_bytes`; the records go to
/// `workflow.loop_file_values` (migration 0169) in the same transaction, and
/// stay there unchanged for the run's life.
///
/// It starts with the URI, so [`values_name_a_file`] tells it apart from
/// every inline `values:`; the count holds no newline, so the LAST newline
/// is the separator, whatever the URI holds. Every record is an iteration,
/// duplicates included: Kestra's URI path has no `distinct()` (the user's
/// ruling of 2026-10-03).
pub fn rendered_from_file(uri: &str, count: usize) -> String {
    format!("{uri}\n{count}")
}
