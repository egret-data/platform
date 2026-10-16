// kestra: core/src/main/java/io/kestra/core/models/tasks/Task.java
// kestra: core/src/main/java/io/kestra/core/models/tasks/retrys/
// kestra: core/src/main/java/io/kestra/plugin/core/flow/

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::models::wire::{optional, required, with_default};
use crate::models::Property;

/// A task node in a flow. Control flow is typed (`TaskKind`'s non-`Leaf`
/// variants); everything else is an opaque `Leaf` -- a state machine that
/// knows what `egret.sql` is has stopped being a state machine.
///
/// `Serialize` is hand-written (see below), mirroring the hand-written
/// `Deserialize`: a derive + `#[serde(flatten)] kind: TaskKind` would put a
/// `Leaf`'s own fields under a `"spec"` key (`#[serde(untagged)]` serialises
/// a *struct* variant as its field map, not flattened into the parent), and
/// re-parsing that output would grow a `spec.spec.spec...` nest on every
/// round-trip. See the task report, Finding 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: String,
    pub r#type: String,
    pub description: Option<String>,
    pub retry: Option<Retry>,
    pub timeout: Option<Property<Duration>>,
    pub disabled: bool,
    /// `Task.java:89` — `@Builder.Default private String runIf = "true";`,
    /// rendered per run and compared by `TruthUtils.isTruthy`.
    ///
    /// **A raw expression, not a `Property<bool>`.** Java's field is a bare
    /// `String` with `dynamic = true`, and the value Kestra tests is the
    /// *rendered string*, not a parsed boolean: `"0"` is falsy and `"00"` is
    /// not. Holding the source here is what lets `advance()` stay pure — the
    /// caller renders it, exactly as it renders an `If`'s `condition`.
    ///
    /// **`None` rather than `Some("true")` for an absent key.** Kestra's
    /// default is the literal string `"true"`, which is truthy, so the two
    /// spellings decide the same way; `None` is what keeps a task nobody wrote
    /// a `runIf:` on out of the render tier's population altogether.
    ///
    /// An explicit `runIf: null` parses to `None` here, i.e. as an absent one.
    /// Java's `TruthUtils.isTruthy(null)` is `false`, so on the artifact that
    /// spelling *might* skip the task instead — but `@Builder.Default` versus
    /// Jackson's null handling decides it and nothing measured which, so the
    /// divergence is named rather than guessed at. No corpus flow writes it
    /// (`no_corpus_flow_writes_a_null_run_if` re-derives that from all 345 on
    /// every run), and `disabled: null` is already read the same way.
    pub run_if: Option<String>,
    /// `Task.java:92` — `@Builder.Default private boolean allowFailure = false;`
    ///
    /// **A plain `boolean`, not a `Property<Boolean>`**, so a pure builder can
    /// read it and no render is involved. Measured on the pinned artifact
    /// rather than read from the checkout: `javap -p
    /// io/kestra/core/models/tasks/Task.class` declares
    /// `private boolean allowFailure` and the accessor
    /// `public boolean isAllowFailure()`, which is the method
    /// `State$Type.fail(Task)` invokes.
    ///
    /// W8 Task 5. Until it, this field and [`Task::allow_warning`] were the two
    /// `Task.java` properties `models::Task` did not carry at all, which is
    /// what `worker::subflow::guess_state`'s doc records from the other side.
    pub allow_failure: bool,
    /// `Task.java:93` — `@Builder.Default private boolean allowWarning = false;`
    ///
    /// **It is read at two different kinds of site and they are not the same
    /// function** — see [`crate::executor::allow_failure`] for the table and
    /// for which corpus entry reaches which.
    pub allow_warning: bool,
    pub kind: TaskKind,
}

/// Control flow the executor must see into, versus the opaque leaf it must
/// not. Variant names and the types they carry are Kestra's own names for
/// the classes actually present in this checkout's
/// `core/src/main/java/io/kestra/plugin/core/flow/` -- verified by reading
/// that directory, not assumed from the W1 plan, which named this type
/// `ForEach` (no such class exists here; Kestra's own name is `Loop`) and
/// omitted `LoopUntil` entirely (see the task report for both corrections).
///
/// No `Serialize`/`Deserialize` derive here on purpose: `Task`'s own hand
/// -written impls dispatch into (or build) the right variant's fields
/// directly, at the top level of the JSON object, rather than through this
/// enum's own tag/flatten machinery.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskKind {
    Sequential(Sequential),
    Parallel(Parallel),
    If(If),
    Switch(Switch),
    Dag(Dag),
    Loop(Loop),
    LoopUntil(LoopUntil),
    Subflow(Subflow),
    AllowFailure(AllowFailure),
    WorkingDirectory(WorkingDirectory),
    // `LoopUntil` sits above, beside `Loop`; both spellings of it reach that
    // variant since W8 Task 4 -- see `kind_for`'s `waitfor` row.
    /// `io.kestra.plugin.core.flow.Pause` -- W8 Task 3.
    ///
    /// **It was a [`TaskKind::Leaf`] until W8 Task 3**, and
    /// `spec::validate`'s `KESTRA_FLOW_CONTROL` table anticipated this
    /// variant by name ("say, a future `TaskKind::Pause`"). The reason it is
    /// typed now is not tidiness: `executor::build_plan` has to read
    /// `pauseDuration`/`timeout` to bound the pause, and reading them out of
    /// an opaque `spec` map would be a second opinion about which keys a
    /// `Pause` has, beside this struct.
    Pause(Pause),
    Leaf {
        spec: serde_json::Value,
    },
}

impl Task {
    /// The tasks this node owns, in declaration order. Empty for a leaf.
    ///
    /// This is what W3's `advance()` walks, and it is the reason control flow is
    /// typed while leaves are opaque.
    pub fn children(&self) -> Vec<&Task> {
        match &self.kind {
            TaskKind::Sequential(s) => s.tasks.iter().collect(),
            TaskKind::Parallel(p) => p.tasks.iter().collect(),
            TaskKind::If(i) => i.then.iter().chain(i.r#else.iter()).collect(),
            TaskKind::Switch(s) => s
                .cases
                .values()
                .flatten()
                .chain(s.defaults.iter())
                .collect(),
            // `filter_map`, not `map`: `DagNode::task` is `Option<Task>` (Task
            // 4 fix round 3 -- `@NotNull` in Java, bean validation not
            // parsing), so a node with no task contributes no child here.
            // `spec::validate` reports the missing task as its own error
            // separately; this just has to not panic while walking a flow
            // that hasn't been validated yet.
            TaskKind::Dag(d) => d.tasks.iter().filter_map(|n| n.task.as_ref()).collect(),
            TaskKind::Loop(l) => l.tasks.iter().collect(),
            TaskKind::LoopUntil(l) => l.tasks.iter().collect(),
            // `finally` is a control-flow child like `tasks`: it runs, so the
            // walks that find children (validation, claiming) must see it.
            TaskKind::AllowFailure(a) => a.tasks.iter().chain(a.finally.iter()).collect(),
            TaskKind::WorkingDirectory(w) => w.tasks.iter().collect(),
            // `Pause.tasks` is Kestra's own field -- the list a pause runs
            // AFTER it is resumed -- so it is a control-flow child like any
            // other and this walk has to see it. `executor::build_plan` does
            // NOT descend into it; see `executor::plan::TaskKind::Pause` for
            // why a `Pause` that declares one is planned opaque instead.
            // The two answers differ on purpose: this walk is
            // `spec::validate`'s (duplicate ids, cycles, `is_runnable_child`),
            // and a child invisible to it is a child nothing checks.
            TaskKind::Pause(p) => p.tasks.iter().collect(),
            TaskKind::Subflow(_) | TaskKind::Leaf { .. } => Vec::new(),
        }
    }

    /// This task's own `errors:` list — the branch that runs when one of its
    /// [`children`](Task::children) fails.
    ///
    /// **Separate from `children()` on purpose, and W3b Task 5 is what proved
    /// it has to exist at all.** `children()` is the *control-flow* walk, and
    /// several callers depend on an `errors:` task not being in it. But a walk
    /// that never reaches one cannot FIND one either, and
    /// `worker::claim`'s `leaf_spec` — which looks a running task's own
    /// properties up by id — used `children()` alone: an error handler nested
    /// inside a flowable's `errors:` was invisible to it, the tick aborted with
    /// `WorkerError::Corrupt`, and the run stayed `RUNNING` for ever. Three
    /// corpus entries showed it (`valids/sequential-with-local-errors`,
    /// `valids/restart_local_errors`,
    /// `valids/replay-sequential-with-error-handler`), and it is a *product*
    /// defect rather than an oracle one: nothing but the oracle had ever run a
    /// flow with a nested `errors:` through a worker.
    ///
    /// Five kinds carry one — `Sequential`, `Parallel`, `If`, `Switch`, `Dag`
    /// — which is `AbstractBranch`'s `errors` in Kestra plus the two branching
    /// kinds that extend it.
    pub fn error_children(&self) -> Vec<&Task> {
        match &self.kind {
            TaskKind::Sequential(s) => s.errors.iter().collect(),
            TaskKind::Parallel(p) => p.errors.iter().collect(),
            TaskKind::If(i) => i.errors.iter().collect(),
            TaskKind::Switch(s) => s.errors.iter().collect(),
            TaskKind::Dag(d) => d.errors.iter().collect(),
            // Six kinds now. `Pause.errors` is real -- `valids/pause-errors-
            // finally-after-execution` declares one -- and it is read for the
            // same reason `children()` reads `Pause.tasks`: a handler nested
            // in it must be findable by `worker::claim::leaf_spec`.
            TaskKind::Pause(p) => p.errors.iter().collect(),
            TaskKind::AllowFailure(a) => a.errors.iter().collect(),
            _ => Vec::new(),
        }
    }
}

// kestra: Sequential.java -- `AbstractBranch`'s `tasks`, run one after
// another, plus its `errors`.
//
// **`errors` arrived at W3a Task 7, and `finally` deliberately did not.**
// W1 dropped both AbstractBranch fields ("W1 is the model, not the
// executor"), and `crates/egret-workflow/src/executor/plan.rs` recorded that
// as the reason `PlanTask::errors` could not be populated *at all*. Task 7
// implements `errors:`, so the field has to exist to be read; `finally:` has
// no owner in W3a and adding a field nothing reads would be a second dead
// field rather than a smaller version of the same fix.
//
// `children()` is NOT widened with it, and that is a decision rather than an
// oversight. Java's `AbstractBranch.allChildTasks()` *is* `tasks + errors +
// _finally`, so `children()` is narrower than Kestra's walk -- but
// `children()` is `spec::validate`'s walk (duplicate ids, `has_cycle`) and
// nothing else, and widening it would change which flows egret REFUSES at
// publish. That is W1's model to own; changing it here would move a refusal
// for a reason that has nothing to do with `errors:` working.
//
// **The argument as first written leaned on "`build_plan` never calls
// `validate`", and that has been false since `e0a22aa7`** -- Task 8 made
// `validate` `build_plan`'s third gate, so widening this walk now moves what
// the EXECUTOR refuses as well as what publish does. The conclusion is
// unchanged and the reason it is unchanged is the stronger half of the
// original: this is a rule about which flows are acceptable, and W1 owns
// those. The clause has been removed rather than left to be read as support.
//
// `tasks` is `@NotEmpty` on `AbstractBranch` in Java (Task 4 fix round 3
// owes the check), but that is bean validation, evaluated when Kestra
// *validates* a flow, not when Jackson *parses* one -- same
// `@NotNull`/`@NotEmpty`-is-bean-validation-not-parsing principle already
// applied to `Parallel::concurrent` below, just never audited against this
// field until fix round 3. `#[serde(default)]` on the plain `Vec<Task>`
// (not `Option<Vec<Task>>`) is deliberate: an absent key becomes an empty
// `Vec`, which `Task::children()`, the duplicate-id walk, and `has_cycle`
// all already iterate safely -- no `Option` unwrapping needed anywhere
// downstream, only the new emptiness check in `spec::validate`.
//
// **`#[serde(default)]` alone covered only the ABSENT key**, and that was a
// defect that reached the HTTP API. An explicit `tasks: null` is
// `invalid type: null, expected a sequence` to a bare `Vec<Task>`, while
// Jackson does not error on one -- it leaves the field null -- so egret's
// `POST /flows` answered 400 for a flow this Kestra publishes and RUNS.
// Every list field in this file therefore carries
// `deserialize_with = "wire::null_is_absent"` beside its `default`: the same
// bean-validation-is-not-parsing principle the paragraph above applies to
// `@NotEmpty` and `Parallel::concurrent` below applies to `@NotNull`.
// Kestra's own 422 for `invalids/invalid-parallel` is the evidence for that
// reading -- it reports `tasks[para4].tasks: The 'tasks' property cannot be
// empty` for the task written `tasks: null`, beside the same message for one
// written `tasks: []` and one with no `tasks` key at all. Parsed, then
// refused for being empty; two stages, and the null belongs to the second.
// See `oracle-exec/README.md`, `The null list Kestra accepts and egret
// refused`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sequential {
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub tasks: Vec<Task>,
    /// `AbstractBranch.errors` -- the list driven instead of `tasks` once a
    /// child of `tasks` has failed and will not be retried
    /// (`Execution.findTaskDependingFlowState`).
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub errors: Vec<Task>,
}

// kestra: Parallel.java -- `AbstractBranch` + `concurrent` (0 = unlimited).
// `concurrent` is `@NotNull` in Java (Task 4 owes a check for that), but
// `@NotNull` is bean validation, evaluated when Kestra *validates* a flow --
// not when Jackson *parses* one. So an explicit `concurrent: null` genuinely
// parses in Kestra (it just fails validation later), which means parsing
// here must accept it too: `Option<Property<i32>>`, not a bare `Property<i32>`.
//
// `tasks` carries the same `@NotEmpty`-on-`AbstractBranch` debt `Sequential`
// documents above (Task 4 fix round 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parallel {
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub tasks: Vec<Task>,
    /// `AbstractBranch.errors`, the same field `Sequential` carries and for
    /// the same reason -- see its doc for why `finally` is not beside it and
    /// why `children()` is not widened.
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub errors: Vec<Task>,
    #[serde(default = "default_concurrent")]
    pub concurrent: Option<Property<i32>>,
}

// kestra: If.java. `condition` is a plain `String`, NOT `Property<String>`,
// and Kestra's own source explains why: "we can't use Property<String> here
// because of the cache of the property evaluation which causes issue when
// using If in a ForEach with concurrencyLimit > 1"
// (https://github.com/kestra-io/kestra/issues/8697). The plan's brief typed
// this field `Property<String>`; the source wins.
//
// `condition` is `@NotNull` in Java with no default (Task 4 fix round 3 owes
// the check) -- same bean-validation-not-parsing principle as
// `Parallel::concurrent`, so `Option<String>` here, not a bare `String`.
// `then` is separately `@NotEmpty` in Java (`If.java`'s own `then` field,
// distinct from this crate's own `@NotEmpty`-on-`AbstractBranch` findings
// above) -- found in the same fix-round-3 sweep but NOT one of the 11 fields
// the round scoped, and not fixed here; see the task report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct If {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub then: Vec<Task>,
    #[serde(
        default,
        rename = "else",
        deserialize_with = "crate::models::wire::null_is_absent"
    )]
    pub r#else: Vec<Task>,
    /// `If.java:111`. `If` is not an `AbstractBranch` -- it extends `Task`
    /// directly -- but it declares its own `errors` all the same, and
    /// `If.resolveNexts` passes it to the same
    /// `FlowableUtils.resolveSequentialNexts`. See `Sequential::errors`.
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub errors: Vec<Task>,
}

// kestra: Switch.java. `value` is `@NotNull` in Java (Task 4 owes a check);
// bean validation runs at flow-validation time, not at parse time, so parse
// must still accept an explicit `value: null` -- `Option<Property<String>>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Switch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Property<String>>,
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub cases: BTreeMap<String, Vec<Task>>,
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub defaults: Vec<Task>,
    /// `Switch.java:112`, the same field `If` carries and for the same reason.
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub errors: Vec<Task>,
}

// kestra: Dag.java -- `tasks` is a list of `(task, dependsOn)` pairs, not a
// plain task list, and it carries the same `concurrent` field as `Parallel`
// (Kestra's field, not this task's invention -- `Dag.java` line 98), same
// `@NotNull`-is-bean-validation-not-parsing caveat as `Parallel::concurrent`.
//
// `tasks` is separately `@NotEmpty` in Java (`Dag.java`'s own field, not
// inherited -- `Dag` extends `Task` directly, not `AbstractBranch`; Task 4
// fix round 3 owes the check).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dag {
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub tasks: Vec<DagNode>,
    /// `Dag.java:106`, the same field the other three flowables carry.
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub errors: Vec<Task>,
    #[serde(default = "default_concurrent")]
    pub concurrent: Option<Property<i32>>,
}

// kestra: Dag.DagTask (inner class). Named `DagNode` here per this task's
// produced interface -- the Rust name isn't wire-visible (unlike `type`), so
// it does not need to match Kestra's inner-class name.
//
// `task` is `@NotNull` in `DagTask` (Task 4 fix round 3 owes the check) --
// the weakest of the 11 fields this round fixes (a dag node with no task is
// unusable either way, parse-required or not), but ported as `Option<Task>`
// for consistency with every other bean-validation-not-parsing field in
// this file rather than carving out an exception.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DagNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Task>,
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub depends_on: Vec<String>,
}

// kestra: Loop.java. THIS is Kestra's `ForEach` in this checkout -- there is
// no `io.kestra.plugin.core.flow.ForEach` class here (verified: only
// `Loop.java` and `LoopUntil.java` exist under `core/flow/`). The plan and
// design doc both assumed the name `ForEach`; Kestra's real name is `Loop`,
// so that is what this type and its `TaskKind` variant are called.
//
// `values` is `Object` in Java (string, array, or ION-file URI, rendered at
// use time) and is never wrapped in `Property<T>` there -- but it is
// unrendered source either way, so `Property<serde_json::Value>` reproduces
// the "any YAML shape becomes source text" behaviour every other dynamic
// field in this crate already gets from `Property`. `values` is `@NotNull`
// in Java (Task 4 owes a check); parsing must still accept an explicit
// `values: null`, same reasoning as `Parallel::concurrent` above.
//
// `transmitFailed` (Java: `Boolean = true`, no `@NotNull`) is ported too --
// omitted from the first pass of this task without being named as dropped,
// which was the actual defect flagged in review (silence, not the omission
// itself); `outputs`/`fetchType` remain deliberately out (execution-plane,
// unused before W3).
//
// `tasks` carries the same `@NotEmpty`-on-`AbstractBranch` debt `Sequential`
// documents above (Task 4 fix round 3). `concurrencyLimit` is `@NotNull
// @Builder.Default private Integer concurrencyLimit = 1;` in Java (verified
// in `Loop.java`) -- boxed `Integer`, not a primitive `int`, so the same
// bean-validation-not-parsing principle applies: `Option<i32>`, not a bare
// `i32`. Round 2's version of this field accepted absence (via
// `default_concurrency_limit`) but rejected an explicit `null` -- the exact
// asymmetry `Parallel::concurrent`/`Dag::concurrent` were already fixed
// against; Task 4 fix round 3 found and closed it here too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loop {
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub tasks: Vec<Task>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Property<serde_json::Value>>,
    #[serde(default = "default_concurrency_limit")]
    pub concurrency_limit: Option<i32>,
    #[serde(default = "default_true")]
    pub transmit_failed: bool,
}

// kestra: LoopUntil.java -- a second `AbstractBranch` sibling of `Loop` this
// checkout has that the plan's Step-1 reading did not list (it named only
// Sequential/Parallel/If/Switch/Dag/ForEach/Subflow/AllowFailure/
// WorkingDirectory). It runs `tasks` in a loop, evaluating `condition` after
// each iteration, so W3's `advance()` must walk its `tasks` exactly like
// `Loop`'s -- added here per this task's resolution: "if one exists that the
// brief omits, add it with a test." `condition` is `@NotNull` in Java (Task 4
// owes a check); parsing must still accept an explicit `condition: null`,
// same reasoning as `Parallel::concurrent` above.
//
// `tasks` carries the same `@NotEmpty`-on-`AbstractBranch` debt `Sequential`
// documents above (Task 4 fix round 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopUntil {
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub tasks: Vec<Task>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Property<String>>,
    // W8 Task 4. Both were silently DROPPED before this task -- serde ignores
    // an unknown key, so `checkFrequency:` and `failOnMaxReached:` round-tripped
    // out of every `LoopUntil` the model parsed -- and both decide how many
    // iterations the artifact runs, so neither can stay dropped now that
    // `executor::plan` has a `TaskKind::LoopUntil`.
    //
    // `Option` rather than Java's `@Builder.Default`: the default belongs in
    // `build_plan`, which is the one place that also knows the PLAN's answer
    // for an absent field. Writing Kestra's defaults back out here would put
    // a `checkFrequency:` into the serialised form of every flow that had
    // none -- the same rule `Task::serialize` already applies to `runIf:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_on_max_reached: Option<Property<bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_frequency: Option<CheckFrequency>,
}

// kestra: `LoopUntil.CheckFrequency`. Three fields, all `Property<…>` in Java
// and all optional here for the reason above. `interval` is `@NotNull` with
// `@Builder.Default Property.ofValue(Duration.ofMinutes(1))`, which is a bean
// default rather than a parse rule, so an absent one is `None` here and
// becomes `PT1M` in `build_plan`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckFrequency {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<Property<i32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<Property<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<Property<String>>,
}

// kestra: Subflow.java. Kept to the interface's field list
// (namespace/flowId/inputs/wait/transmitFailed); `revision`, `labels`,
// `inheritLabels`, `scheduleDate`, and `restartBehavior` are dropped for now,
// the same curation the brief already applied to this type.
//
// `namespace` is `@NotEmpty` and `flowId` is `@NotNull`, both in Java with no
// default (Task 4 fix round 3 owes both checks) -- same bean-validation-not-
// parsing principle as `Parallel::concurrent`, so both are `Option<String>`
// here, not bare `String`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subflow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<HashMap<String, serde_json::Value>>,
    #[serde(default = "default_true")]
    pub wait: bool,
    #[serde(default = "default_true")]
    pub transmit_failed: bool,

    // Task 6 fix round 1: `publish::resolve_task` fills this in once
    // `namespace`/`flow_id` have been checked through `WorkflowRefPort` and
    // confirmed to name a flow in the PUBLISHING workspace -- `None` on
    // every draft, `Some` only inside a frozen `FlowVersionRow::snapshot`.
    //
    // A `Leaf`'s own reference field (e.g. `egret.sql`'s `database`) is
    // overwritten IN PLACE with the resolved id once resolved, because that
    // field's only job was ever to name the target -- see
    // `publish::resolve_task`'s `TaskKind::Leaf` arm. `namespace`/`flow_id`
    // cannot be overwritten the same way: they are Kestra's own identity
    // pair, plausibly still needed by an error message or an editor after
    // publish, so this is a THIRD field rather than a destructive rewrite of
    // either existing one -- the resolved uuid `publish` already confirmed
    // is safe to trust, sitting beside the human-readable name rather than
    // replacing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_ref: Option<uuid::Uuid>,
}

// kestra: core/src/main/java/io/kestra/plugin/core/flow/Pause.java --
// `extends Sequential`, so `tasks` and `errors` are `AbstractBranch`'s, plus
// four of its own.
//
// **`finally` is dropped here, exactly as `Sequential` drops it**, and that
// is the same decision rather than a new loss: no `models` type carries the
// field, because nothing in this crate implements `finally:`. It was
// previously preserved by accident -- a `Pause` was a `Leaf`, so every key it
// wrote survived inside the opaque `spec` -- and `valids/pause-errors-finally-
// after-execution` is the one corpus flow that writes it.
//
// `pauseDuration` and `behavior` are `Property<..>`: Java declares both
// `dynamic = true` (`Pause.java`), and two corpus flows write an expression
// into them -- `valids/pause-duration-from-input` and `valids/pause-behavior`.
// Holding the SOURCE is what lets `executor::advance` stay pure.
//
// `onPause` is a single task, not a list: that is Kestra's shape and
// `valids/pause_on_pause` writes it as one mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pause {
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub tasks: Vec<Task>,
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub errors: Vec<Task>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_duration: Option<Property<Duration>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<Property<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_pause: Option<Box<Task>>,
}

// kestra: AllowFailure.java -- `extends Sequential` with no fields of its
// own. `tasks` and `errors` are `AbstractBranch`'s, as for `Sequential`, and
// `finally` is `AbstractBranch`'s `_finally`, which `Sequential` drops and this
// kind does not: a block whose failure is a warning runs it after the block
// has ended, whatever the block's state (spec §3.3).
//
// `tasks` carries the same `@NotEmpty`-on-`AbstractBranch` (via `Sequential`)
// debt `Sequential` documents above (Task 4 fix round 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowFailure {
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub tasks: Vec<Task>,
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub errors: Vec<Task>,
    #[serde(
        default,
        rename = "finally",
        deserialize_with = "crate::models::wire::null_is_absent"
    )]
    pub finally: Vec<Task>,
}

// kestra: WorkingDirectory.java -- also `extends Sequential`; `cache`,
// `namespaceFiles`, `inputFiles`, `outputFiles` are execution-plane concerns
// dropped here (the same curation the brief applied to `Subflow`).
// *(2026-10-05, W9b Task 5: `namespaceFiles` is KEPT, verbatim, and validated
// at publish by `NamespaceFilesSpec::parse` (`spec::validate`); it is not run,
// because egret does not run `WorkingDirectory` -- spec §7, the user's ruling.
// `cache`, `inputFiles` and `outputFiles` are still dropped.)*
//
// `tasks` carries the same `@NotEmpty`-on-`AbstractBranch` (via `Sequential`)
// debt `Sequential` documents above (Task 4 fix round 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingDirectory {
    #[serde(default, deserialize_with = "crate::models::wire::null_is_absent")]
    pub tasks: Vec<Task>,
    /// Kept verbatim (W9b Task 5); validated at publish by
    /// `storage::nsfiles::stage::NamespaceFilesSpec::parse`, never run.
    #[serde(
        default,
        rename = "namespaceFiles",
        skip_serializing_if = "Option::is_none"
    )]
    pub namespace_files: Option<serde_json::Value>,
}

// Used only for the `#[serde(default = "...")]` case: the key is *absent*,
// matching Java's `@Builder.Default` value (`Property.ofValue(0)`) taking
// effect when the field is never assigned. An explicit `null` in the input
// bypasses this function entirely -- `Option<Property<i32>>`'s own
// `Deserialize` sees the `null` token and yields `None` directly, exactly
// as Jackson would null out the field despite its default initializer.
fn default_concurrent() -> Option<Property<i32>> {
    Some(Property::from_source("0".to_string()))
}

// Same `#[serde(default = "...")]`-for-the-absent-case pattern as
// `default_concurrent` above: matches Java's `@Builder.Default` value (`1`)
// taking effect when the field is never assigned, while an explicit
// `concurrencyLimit: null` bypasses this function entirely and lands on
// `None` via `Option<i32>`'s own `Deserialize`.
fn default_concurrency_limit() -> Option<i32> {
    Some(1)
}

const fn default_true() -> bool {
    true
}

// kestra: retrys/AbstractRetry.java + Constant.java + Exponential.java +
// Random.java. Unlike `Task`, `AbstractRetry` really does use
// `@JsonSubTypes` (`@JsonTypeInfo(use = NAME, property = "type", ...)`) over
// a closed set of exactly three subtypes, so a plain internally-tagged serde
// enum reproduces Kestra's own dispatch instead of approximating it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryCommon {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<Property<Duration>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<i32>,
    #[serde(default)]
    pub warning_on_retry: bool,
    #[serde(default)]
    pub behavior: RetryBehavior,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RetryBehavior {
    #[default]
    RetryFailedTask,
    CreateNewExecution,
}

// `interval`/`maxInterval`/`minInterval` are `@NotNull` on all three Java
// subtypes (Task 4 owes a check on each). Same bean-validation-not-parsing
// reasoning as `Parallel::concurrent`: parsing must accept an explicit
// `null` for any of them, so all five are `Option<Property<Duration>>`, not
// bare.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Retry {
    Constant {
        #[serde(flatten)]
        common: RetryCommon,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval: Option<Property<Duration>>,
    },
    #[serde(rename_all = "camelCase")]
    Exponential {
        #[serde(flatten)]
        common: RetryCommon,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval: Option<Property<Duration>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_interval: Option<Property<Duration>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_factor: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Random {
        #[serde(flatten)]
        common: RetryCommon,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_interval: Option<Property<Duration>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_interval: Option<Property<Duration>>,
    },
}

impl<'de> Deserialize<'de> for Task {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        let mut map = serde_json::Map::deserialize(d)?;

        // `required`/`optional`/`with_default` (see `wire.rs`) always try to
        // deserialize a *present* value as the target type -- a caller-
        // supplied `disabled: "true"` (a quoted string) is a real type error
        // here, not a silent fall-through to `false`. Only a genuinely
        // *absent* key takes `missing_field`/`None`/the given default. See
        // the task report, Fix round 1 finding 1.
        let id: String = required(&mut map, "id")?;
        let r#type: String = required(&mut map, "type")?;
        let description: Option<String> = optional(&mut map, "description")?;
        let retry: Option<Retry> = optional(&mut map, "retry")?;
        let timeout: Option<Property<Duration>> = optional(&mut map, "timeout")?;
        let disabled: bool = with_default(&mut map, "disabled", false)?;
        // Kestra's key is `runIf`, and it is read here rather than left in
        // `rest` so that a `Leaf`'s opaque `spec` cannot be the only place it
        // survives -- which is where it was through W3a, and why the executor
        // could not see it.
        let run_if: Option<String> = optional(&mut map, "runIf")?;
        // Taken out of `map` for the same reason `runIf` is: these are
        // `Task.java`'s own fields, and leaving them in `rest` would make a
        // `Leaf`'s opaque `spec` the only place they survive -- invisible to
        // `build_plan`, which is the one caller that needs them.
        let allow_failure: bool = with_default(&mut map, "allowFailure", false)?;
        let allow_warning: bool = with_default(&mut map, "allowWarning", false)?;

        let rest = serde_json::Value::Object(map);
        let kind = kind_for(&r#type, rest).map_err(D::Error::custom)?;

        Ok(Task {
            id,
            r#type,
            description,
            retry,
            timeout,
            disabled,
            run_if,
            allow_failure,
            allow_warning,
            kind,
        })
    }
}

/// The inverse of the custom `Deserialize` above: emit the shared fields,
/// then flatten the control-flow struct's (or the leaf's `spec` map's)
/// fields into the *same* JSON object, at the top level -- not nested under
/// a `"kind"` or `"spec"` key. That is what makes `parse -> serialise` valid
/// Kestra YAML again and what makes `parse -> serialise -> parse ->
/// serialise` idempotent (see the round-trip tests below).
impl Serialize for Task {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;

        let kind_value = match &self.kind {
            TaskKind::Sequential(v) => serde_json::to_value(v),
            TaskKind::Parallel(v) => serde_json::to_value(v),
            TaskKind::If(v) => serde_json::to_value(v),
            TaskKind::Switch(v) => serde_json::to_value(v),
            TaskKind::Dag(v) => serde_json::to_value(v),
            TaskKind::Loop(v) => serde_json::to_value(v),
            TaskKind::LoopUntil(v) => serde_json::to_value(v),
            TaskKind::Subflow(v) => serde_json::to_value(v),
            TaskKind::AllowFailure(v) => serde_json::to_value(v),
            TaskKind::WorkingDirectory(v) => serde_json::to_value(v),
            TaskKind::Pause(v) => serde_json::to_value(v),
            // The leaf's own fields are already a flat map -- `rest` from
            // `Deserialize`, untouched -- so there is nothing to flatten
            // them out of, unlike a struct-variant's `#[serde(untagged)]`
            // serialisation.
            TaskKind::Leaf { spec } => Ok(spec.clone()),
        }
        .map_err(S::Error::custom)?;

        let mut map = match kind_value {
            serde_json::Value::Object(map) => map,
            other => {
                return Err(S::Error::custom(format!(
                    "task kind serialised to a non-object JSON value: {other}"
                )))
            }
        };

        // Task's own fields are inserted last so they always win: `id`/
        // `type`/etc. can never appear inside a leaf's `spec` map to begin
        // with (the Deserialize above strips them before building `rest`),
        // so there is no real collision today, but this ordering keeps it
        // that way even if a future control-flow struct ever grew a
        // same-named field.
        map.insert("id".to_string(), serde_json::Value::String(self.id.clone()));
        map.insert(
            "type".to_string(),
            serde_json::Value::String(self.r#type.clone()),
        );
        if let Some(description) = &self.description {
            map.insert(
                "description".to_string(),
                serde_json::Value::String(description.clone()),
            );
        }
        if let Some(retry) = &self.retry {
            map.insert(
                "retry".to_string(),
                serde_json::to_value(retry).map_err(S::Error::custom)?,
            );
        }
        if let Some(timeout) = &self.timeout {
            map.insert(
                "timeout".to_string(),
                serde_json::to_value(timeout).map_err(S::Error::custom)?,
            );
        }
        map.insert(
            "disabled".to_string(),
            serde_json::Value::Bool(self.disabled),
        );
        // Emitted only when the author wrote one, unlike `disabled` above:
        // `None` here means "no `runIf:` in the source", and writing Kestra's
        // default `"true"` back out would put a key into every serialised task
        // that no input had.
        if let Some(run_if) = &self.run_if {
            map.insert(
                "runIf".to_string(),
                serde_json::Value::String(run_if.clone()),
            );
        }
        // Emitted only when TRUE, following `runIf` above rather than
        // `disabled`: `false` is Kestra's default and both spellings
        // (`allowFailure: false` and no key at all) parse to the same `false`,
        // so round-tripping stays idempotent while a task nobody wrote the key
        // on does not grow one.
        if self.allow_failure {
            map.insert("allowFailure".to_string(), serde_json::Value::Bool(true));
        }
        if self.allow_warning {
            map.insert("allowWarning".to_string(), serde_json::Value::Bool(true));
        }

        serde_json::Value::Object(map).serialize(serializer)
    }
}

/// Dispatch a task's remaining fields on its `type`.
///
/// Both spellings are accepted and the FQCN is canonical, so a Kestra flow
/// pasted in parses and looking a task up in Kestra's docs lands on the right
/// page (programme spec section 7 rule 4). The short-alias spelling itself
/// (`core.` + the lower-cased class name, e.g. `core.allowfailure`) is the
/// W1 plan's own established scheme (used throughout
/// `docs/en/plans/2026-09-01-workflow-w1-model.md`), not invented for this
/// task; it is applied here to every real control-flow class name found by
/// reading `core/src/main/java/io/kestra/plugin/core/flow/`, including the
/// two names (`loop`, `loopuntil`) the plan's own reading missed.
/// **Class names -- plus the ONE alias Kestra registers, which W8 Task 4
/// added.** `io.kestra.plugin.core.flow.WaitFor` is an alias on `LoopUntil`
/// rather than a class of its own (`oracle-exec/task-kinds.json`: `alias_of`
/// `LoopUntil`; `LoopUntil.java`'s own `@Plugin(aliases = …)` is the
/// declaration), so reading the directory could not produce the name and
/// `kind_for` below had no `"waitfor"` row until Task 4. While it had none a
/// `WaitFor` fell to `TaskKind::Leaf` with its `tasks:` inside the opaque
/// `spec`, where `children()` cannot see them -- so `spec::validate`,
/// `publish::resolve_all` and `worker::claim`, which all walk `children()`,
/// walked none of them. Measured at W8 Task 1 fix round 4 and re-measured at
/// round 5, where the identical flow spelled `LoopUntil` validated its nested
/// task and the `WaitFor` one did not. **7 of the corpus's 10 flows of that
/// construct use the alias spelling**, so the gap covered the majority of the
/// population. Fixed in `kind_for` below, which is where the authority lives:
/// `spec::validate`'s `KESTRA_FLOW_CONTROL` is a claim held against it and
/// gained the same name in the same commit.
/// The canonical (lower-cased, prefix-stripped) short name for a task
/// `type` string, if it uses either spelling this crate accepts for a
/// Kestra `io.kestra.plugin.core.flow.*` class -- the FQCN itself, or the
/// `core.` + lower-cased-class-name alias. `None` for any other `type` (an
/// egret type like `egret.sql`, or a third-party plugin FQCN).
///
/// Shared by `kind_for` below (which `TaskKind` variant a canonical name
/// maps to) and `crate::spec::validate` (which canonical names are Kestra
/// flow-control classes this crate has NOT ported to a `TaskKind` variant --
/// **none today**: `Pause` gained one at W8 Task 3 and `WaitFor` gained one at
/// W8 Task 4, so the two tables are now equal and the constant is a
/// specification rather than a lookup -- and so must warn about rather than
/// silently falling
/// through to `Leaf`, indistinguishable from an ordinary task). `pub(crate)`
/// rather than re-exported: it's an implementation detail of dispatch, not
/// part of this module's public interface, but `spec` is a sibling of
/// `models`, not a descendant, so plain module-private visibility can't
/// reach it.
pub(crate) fn canonical_flow_type(r#type: &str) -> Option<String> {
    r#type
        .strip_prefix("io.kestra.plugin.core.flow.")
        .map(str::to_ascii_lowercase)
        .or_else(|| r#type.strip_prefix("core.").map(str::to_ascii_lowercase))
}

fn kind_for(r#type: &str, rest: serde_json::Value) -> Result<TaskKind, serde_json::Error> {
    let canonical = canonical_flow_type(r#type);

    Ok(match canonical.as_deref() {
        Some("sequential") => TaskKind::Sequential(serde_json::from_value(rest)?),
        Some("parallel") => TaskKind::Parallel(serde_json::from_value(rest)?),
        Some("if") => TaskKind::If(serde_json::from_value(rest)?),
        Some("switch") => TaskKind::Switch(serde_json::from_value(rest)?),
        Some("dag") => TaskKind::Dag(serde_json::from_value(rest)?),
        Some("loop") => TaskKind::Loop(serde_json::from_value(rest)?),
        // **`waitfor` and `loopuntil` share ONE row, because they are one
        // class.** `LoopUntil.java` carries `@Plugin(aliases =
        // "io.kestra.plugin.core.flow.WaitFor")` -- an alias registered ON
        // `LoopUntil`, not a second class -- so `io.kestra.plugin.core.flow.
        // WaitFor` and `…LoopUntil` are the same task type to the artifact and
        // must be the same `TaskKind` here. W8 Task 4 Step 0; before it
        // `"waitfor"` had no row anywhere in this crate and fell to
        // [`TaskKind::Leaf`], which cost two things, both measured at W8 Task 1
        // fix rounds 4 and 5: a `Leaf` has no `children()`, so `spec::validate`,
        // `publish::resolve_all` and `worker::claim` walked none of a
        // `WaitFor`'s nested tasks; and `spec::validate`'s `is_runnable_child`
        // is `matches!(kind, Leaf)`, so a `WaitFor` child of a
        // `WorkingDirectory` PUBLISHED where the identical `LoopUntil` child
        // was refused with `working_directory_child_not_runnable`. That
        // over-acceptance closes here rather than in the validator: this row is
        // the authority both of those read through.
        Some("loopuntil") | Some("waitfor") => TaskKind::LoopUntil(serde_json::from_value(rest)?),
        Some("subflow") => TaskKind::Subflow(serde_json::from_value(rest)?),
        Some("allowfailure") => TaskKind::AllowFailure(serde_json::from_value(rest)?),
        Some("workingdirectory") => TaskKind::WorkingDirectory(serde_json::from_value(rest)?),
        Some("pause") => TaskKind::Pause(serde_json::from_value(rest)?),
        _ => TaskKind::Leaf { spec: rest },
    })
}

#[cfg(test)]
mod tests {
    use super::{Retry, RetryBehavior, Task, TaskKind};

    /// A leaf task egret owns. The model must NOT understand what egret.sql is --
    /// a state machine that knows what SQL is has stopped being a state machine.
    #[test]
    fn an_unknown_type_becomes_an_opaque_leaf() {
        let yaml = r#"
id: stage
type: egret.sql
database: trino_prod
sql: SELECT 1
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        assert_eq!(t.id, "stage");
        assert_eq!(t.r#type, "egret.sql");
        match &t.kind {
            TaskKind::Leaf { spec } => {
                assert_eq!(spec["database"], "trino_prod");
                assert_eq!(spec["sql"], "SELECT 1");
                // `id` and `type` are Task's own fields and must not be
                // duplicated into the leaf spec, or a handler would see two
                // sources of truth for them.
                assert!(spec.get("id").is_none(), "id leaked into the leaf spec");
                assert!(spec.get("type").is_none(), "type leaked into the leaf spec");
            }
            other => panic!("expected Leaf, got {other:?}"),
        }
    }

    /// Control flow the state machine DOES have to see into. The FQCN is the
    /// canonical spelling; the short alias is the secondary path.
    #[test]
    fn control_flow_is_typed_under_both_spellings() {
        for spelling in ["io.kestra.plugin.core.flow.Sequential", "core.sequential"] {
            let yaml = format!(
                "id: fan\ntype: {spelling}\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n"
            );
            let t: Task = serde_norway::from_str(&yaml).unwrap();
            match &t.kind {
                TaskKind::Sequential(s) => assert_eq!(s.tasks.len(), 1, "{spelling}"),
                other => panic!("{spelling}: expected Sequential, got {other:?}"),
            }
        }
    }

    /// `children()` is what W3's advance() walks. A Leaf owning children would
    /// make the state machine descend into a task it must not understand.
    #[test]
    fn children_are_the_control_flow_nodes_only() {
        let yaml = r#"
id: outer
type: core.sequential
tasks:
  - id: a
    type: egret.sql
    sql: SELECT 1
  - id: b
    type: egret.sql
    sql: SELECT 2
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "children must be in declaration order");
        assert!(
            t.children()[0].children().is_empty(),
            "a leaf owns no children"
        );
    }

    #[test]
    fn disabled_defaults_to_false() {
        let t: Task = serde_norway::from_str("id: a\ntype: egret.sql\nsql: SELECT 1\n").unwrap();
        assert!(!t.disabled);
    }

    /// Kestra's `ForEach` is not named that in this checkout -- verified by
    /// reading `core/src/main/java/io/kestra/plugin/core/flow/`, which has
    /// `Loop.java`, not `ForEach.java`. Both the W1 plan and design doc
    /// assumed `ForEach`; this locks in the correction with the same
    /// both-spellings coverage `control_flow_is_typed_under_both_spellings`
    /// gives `Sequential`.
    #[test]
    fn foreach_is_really_named_loop_in_this_checkout() {
        for spelling in ["io.kestra.plugin.core.flow.Loop", "core.loop"] {
            let yaml = format!(
                "id: fan\ntype: {spelling}\nvalues: [1, 2, 3]\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n"
            );
            let t: Task = serde_norway::from_str(&yaml).unwrap();
            match &t.kind {
                TaskKind::Loop(l) => {
                    assert_eq!(l.tasks.len(), 1, "{spelling}");
                    assert_eq!(l.values.as_ref().unwrap().source(), "[1,2,3]", "{spelling}");
                }
                other => panic!("{spelling}: expected Loop, got {other:?}"),
            }
        }
    }

    /// `LoopUntil.java` is a second control-flow type this checkout has that
    /// the plan's Step-1 reading omitted entirely. It is a genuine sibling of
    /// `Loop` (both extend `AbstractBranch`), so it must be typed the same
    /// way, not fall through to `Leaf`.
    #[test]
    fn loop_until_is_a_control_flow_type_the_plan_missed() {
        let yaml = r#"
id: retry_until_ready
type: io.kestra.plugin.core.flow.LoopUntil
condition: "{{ outputs.check.value == 'ready' }}"
tasks:
  - id: check
    type: egret.sql
    sql: SELECT 1
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::LoopUntil(l) => {
                assert_eq!(
                    l.condition.as_ref().unwrap().source(),
                    "{{ outputs.check.value == 'ready' }}"
                );
                assert_eq!(t.children().len(), 1);
            }
            other => panic!("expected LoopUntil, got {other:?}"),
        }
    }

    // -- Review round 1, Finding 1: Serialize must be Deserialize's inverse --

    /// `Leaf { spec }` is a struct variant, and `#[serde(untagged)]`'s
    /// derived `Serialize` writes a struct variant as its *field map*
    /// (`{"spec": {...}}`), not flattened -- so re-parsing that and
    /// serialising again nested `spec.spec.spec...` one level deeper every
    /// round-trip, and even the FIRST serialisation wasn't valid Kestra YAML
    /// (a leaf's own fields belong at the top level, beside `id`/`type`).
    /// The fix is `Task`'s hand-written `Serialize`; this asserts the
    /// property directly rather than trusting the implementation.
    #[test]
    fn leaf_round_trips_without_growing_a_spec_wrapper() {
        let yaml = "id: t\ntype: egret.sql\ndatabase: d\nsql: SELECT 1\n";
        let t1: Task = serde_norway::from_str(yaml).unwrap();
        let once = serde_json::to_string(&t1).unwrap();

        let as_value: serde_json::Value = serde_json::from_str(&once).unwrap();
        assert_eq!(as_value["database"], "d");
        assert_eq!(as_value["sql"], "SELECT 1");
        assert!(
            as_value.get("spec").is_none(),
            "a leaf's fields must sit at the top level, not nested under spec: {once}"
        );

        let t2: Task = serde_json::from_str(&once).unwrap();
        let twice = serde_json::to_string(&t2).unwrap();
        assert_eq!(
            once, twice,
            "parse -> serialise -> parse -> serialise must be byte-identical"
        );
    }

    /// Same property, one level down: a `Sequential` whose only child is a
    /// leaf must not corrupt that child on re-serialisation either.
    #[test]
    fn nested_sequential_leaf_round_trips_without_growing_a_spec_wrapper() {
        let yaml = "id: outer\ntype: core.sequential\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t1: Task = serde_norway::from_str(yaml).unwrap();
        let once = serde_json::to_string(&t1).unwrap();

        let as_value: serde_json::Value = serde_json::from_str(&once).unwrap();
        let inner = &as_value["tasks"][0];
        assert_eq!(inner["sql"], "SELECT 1");
        assert!(inner.get("spec").is_none());

        let t2: Task = serde_json::from_str(&once).unwrap();
        let twice = serde_json::to_string(&t2).unwrap();
        assert_eq!(
            once, twice,
            "parse -> serialise -> parse -> serialise must be byte-identical"
        );
    }

    // -- Review round 1, Finding 2: one parsing test per remaining control-flow type --

    /// `Parallel.java`'s `concurrent`, plus `children()` still returning
    /// `tasks` in declaration order for this type too.
    #[test]
    fn parallel_carries_its_own_tasks_and_concurrent() {
        let yaml = r#"
id: fan
type: core.parallel
concurrent: 2
tasks:
  - id: a
    type: egret.sql
    sql: SELECT 1
  - id: b
    type: egret.sql
    sql: SELECT 2
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Parallel(p) => {
                assert_eq!(p.concurrent.as_ref().unwrap().source(), "2");
            }
            other => panic!("expected Parallel, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    /// `If.java`'s `_else` field is `@JsonProperty("else")` -- the wire name
    /// is the Rust keyword `else`, hence `r#else`. Exercises both branches
    /// and `children()`'s `then`-before-`else` chaining order.
    #[test]
    fn if_uses_else_not_r_else_on_the_wire() {
        let yaml = r#"
id: gate
type: io.kestra.plugin.core.flow.If
condition: "{{ 1 == 1 }}"
then:
  - id: a
    type: egret.sql
    sql: SELECT 1
else:
  - id: b
    type: egret.sql
    sql: SELECT 2
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::If(i) => {
                assert_eq!(i.condition.as_deref(), Some("{{ 1 == 1 }}"));
                assert_eq!(i.then.len(), 1);
                assert_eq!(i.r#else.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "children() must chain then before else");
    }

    /// `Switch.java`'s `cases` is a `Map<String, List<Task>>` keyed by the
    /// literal matched value, not a list of `(value, tasks)` pairs.
    #[test]
    fn switch_cases_are_keyed_by_the_matched_value() {
        let yaml = r#"
id: route
type: core.switch
value: "{{ inputs.env }}"
cases:
  prod:
    - id: p
      type: egret.sql
      sql: SELECT 1
  dev:
    - id: d
      type: egret.sql
      sql: SELECT 2
defaults:
  - id: fallback
    type: egret.sql
    sql: SELECT 3
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Switch(s) => {
                assert_eq!(s.value.as_ref().unwrap().source(), "{{ inputs.env }}");
                assert_eq!(s.cases["prod"][0].id, "p");
                assert_eq!(s.cases["dev"][0].id, "d");
                assert_eq!(s.defaults[0].id, "fallback");
            }
            other => panic!("expected Switch, got {other:?}"),
        }
        // BTreeMap iterates keys in sorted order ("dev" < "prod"), so
        // children() sees dev's tasks, then prod's, then defaults.
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["d", "p", "fallback"]);
    }

    /// `Dag.DagTask.dependsOn` is camelCase on the wire (`dependsOn`), and
    /// `Dag` carries the same `concurrent` field `Parallel` does. `children()`
    /// must stay in declaration order, not dependency order.
    #[test]
    fn dag_depends_on_is_camel_case_and_concurrent_is_ported() {
        let yaml = r#"
id: pipeline
type: core.dag
concurrent: 1
tasks:
  - task:
      id: extract
      type: egret.sql
      sql: SELECT 1
  - task:
      id: load
      type: egret.sql
      sql: SELECT 2
    dependsOn: ["extract"]
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Dag(d) => {
                assert_eq!(d.concurrent.as_ref().unwrap().source(), "1");
                assert_eq!(d.tasks[1].depends_on, vec!["extract".to_string()]);
                assert!(d.tasks[0].depends_on.is_empty());
            }
            other => panic!("expected Dag, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["extract", "load"]);
    }

    /// `Subflow.java`'s `flowId` is camelCase on the wire, and both booleans
    /// default to `true` in Kestra. A `Subflow` spawns a *different* flow's
    /// execution, so `children()` must stay empty -- it owns no local tasks.
    #[test]
    fn subflow_uses_flow_id_camel_case_and_defaults_wait_true() {
        let yaml = r#"
id: call
type: io.kestra.plugin.core.flow.Subflow
namespace: company.team
flowId: nightly_rollup
inputs:
  as_of: "{{ execution.startDate }}"
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Subflow(s) => {
                assert_eq!(s.namespace.as_deref(), Some("company.team"));
                assert_eq!(s.flow_id.as_deref(), Some("nightly_rollup"));
                assert!(s.wait);
                assert!(s.transmit_failed);
                assert_eq!(
                    s.inputs.as_ref().unwrap()["as_of"],
                    serde_json::json!("{{ execution.startDate }}")
                );
            }
            other => panic!("expected Subflow, got {other:?}"),
        }
        assert!(
            t.children().is_empty(),
            "Subflow spawns a different flow's execution, not local children"
        );
    }

    /// `AllowFailure.java` is `extends Sequential` with no fields of its
    /// own -- confirms it still gets `tasks` and isn't accidentally routed
    /// to `Leaf`.
    #[test]
    fn allow_failure_carries_tasks_like_sequential() {
        let yaml = "id: risky\ntype: core.allowfailure\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::AllowFailure(a) => assert_eq!(a.tasks.len(), 1),
            other => panic!("expected AllowFailure, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a"]);
    }

    /// `WorkingDirectory.java` is also `extends Sequential` -- same check as
    /// `AllowFailure`, under its own FQCN.
    #[test]
    fn working_directory_carries_tasks_like_sequential() {
        let yaml = "id: scoped\ntype: io.kestra.plugin.core.flow.WorkingDirectory\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::WorkingDirectory(w) => assert_eq!(w.tasks.len(), 1),
            other => panic!("expected WorkingDirectory, got {other:?}"),
        }
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a"]);
    }

    /// `AbstractRetry`'s three real `@JsonSubTypes` -- `constant`/
    /// `exponential`/`random` -- dispatched on `type`, plus the shared
    /// `RetryCommon` fields each variant flattens in.
    #[test]
    fn retry_dispatches_all_three_json_subtypes() {
        let constant = "id: a\ntype: egret.sql\nsql: SELECT 1\nretry:\n  type: constant\n  interval: PT10S\n  maxAttempts: 3\n";
        let t: Task = serde_norway::from_str(constant).unwrap();
        match t.retry.as_ref().unwrap() {
            Retry::Constant { common, interval } => {
                assert_eq!(common.max_attempts, Some(3));
                assert_eq!(interval.as_ref().unwrap().source(), "PT10S");
            }
            other => panic!("expected Retry::Constant, got {other:?}"),
        }

        let exponential = "id: a\ntype: egret.sql\nsql: SELECT 1\nretry:\n  type: exponential\n  interval: PT1S\n  maxInterval: PT30S\n  behavior: CREATE_NEW_EXECUTION\n";
        let t: Task = serde_norway::from_str(exponential).unwrap();
        match t.retry.as_ref().unwrap() {
            Retry::Exponential {
                common,
                max_interval,
                ..
            } => {
                assert_eq!(common.behavior, RetryBehavior::CreateNewExecution);
                assert_eq!(max_interval.as_ref().unwrap().source(), "PT30S");
            }
            other => panic!("expected Retry::Exponential, got {other:?}"),
        }

        let random = "id: a\ntype: egret.sql\nsql: SELECT 1\nretry:\n  type: random\n  minInterval: PT1S\n  maxInterval: PT5S\n";
        let t: Task = serde_norway::from_str(random).unwrap();
        match t.retry.as_ref().unwrap() {
            Retry::Random { min_interval, .. } => {
                assert_eq!(min_interval.as_ref().unwrap().source(), "PT1S");
            }
            other => panic!("expected Retry::Random, got {other:?}"),
        }
    }

    // -- Review round 1, Finding 4: @NotNull is bean validation, not parsing --

    /// Task 1's fix made `Property::deserialize` reject `null` for a bare
    /// `Property<T>` -- correct in isolation, but it silently broke every
    /// bare (non-`Option`) `Property<T>` field in this file, because
    /// Kestra's `@NotNull` is bean validation (checked when a flow is
    /// *validated*), not a Jackson parse-time constraint. An explicit
    /// `concurrent: null` (or `value: null`, `condition: null`, `values:
    /// null`, any retry interval `: null`, ...) must still parse; only
    /// Task 4's semantic validator may reject it.
    #[test]
    fn explicit_null_still_parses_on_notnull_property_fields() {
        let yaml = "id: fan\ntype: core.parallel\nconcurrent: null\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TaskKind::Parallel(p) => assert!(
                p.concurrent.is_none(),
                "explicit null must clear the field, not error"
            ),
            other => panic!("expected Parallel, got {other:?}"),
        }

        // Omitting the key entirely still gets Kestra's own default (0) --
        // `null` and "absent" are NOT the same thing when the Java field
        // has a `@Builder.Default`.
        let yaml_omitted = "id: fan\ntype: core.parallel\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t2: Task = serde_norway::from_str(yaml_omitted).unwrap();
        match &t2.kind {
            TaskKind::Parallel(p) => assert_eq!(p.concurrent.as_ref().unwrap().source(), "0"),
            other => panic!("expected Parallel, got {other:?}"),
        }

        // Switch::value has no Java default at all, so absent and explicit
        // null both correctly land on None -- Kestra doesn't distinguish
        // them either once a field has no initializer.
        let switch_null = "id: s\ntype: core.switch\nvalue: null\ndefaults:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n";
        let t3: Task = serde_norway::from_str(switch_null).unwrap();
        match &t3.kind {
            TaskKind::Switch(s) => assert!(s.value.is_none()),
            other => panic!("expected Switch, got {other:?}"),
        }
    }

    // -- Fix round 1, finding 1 (CRITICAL): a present-but-wrong-typed value
    // must be a parse error, never a silent fall-through to a type's
    // default. `disabled: "true"` (a quoted string) previously parsed
    // successfully with `disabled == false` -- a user who writes it believes
    // they disabled the task, and it runs anyway with no error and nothing
    // in a validation report.

    #[test]
    fn disabled_with_a_string_value_is_a_parse_error_not_a_silent_false() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\ndisabled: \"true\"\n";
        let err = serde_norway::from_str::<Task>(yaml).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("bool")
                || err.to_string().to_lowercase().contains("invalid type"),
            "expected a type error, got: {err}"
        );
    }

    #[test]
    fn disabled_with_a_number_value_is_a_parse_error() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\ndisabled: 1\n";
        assert!(serde_norway::from_str::<Task>(yaml).is_err());
    }

    #[test]
    fn description_with_a_non_string_value_is_a_parse_error() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\ndescription: [not, a, string]\n";
        assert!(serde_norway::from_str::<Task>(yaml).is_err());
    }

    /// The bug also affected `id`/`type` themselves, though less visibly:
    /// `.as_str()` swallowing a wrong-typed value meant the reported error
    /// was a misleading `missing field` rather than a real type mismatch.
    #[test]
    fn wrong_typed_id_is_a_type_error_not_a_misleading_missing_field() {
        let yaml = "id: 5\ntype: egret.sql\nsql: SELECT 1\n";
        let err = serde_norway::from_str::<Task>(yaml).unwrap_err();
        assert!(
            !err.to_string().to_lowercase().contains("missing field"),
            "a wrong-typed `id` must not report as a missing field: {err}"
        );
    }

    /// The default still applies when the key is genuinely absent -- the fix
    /// must not turn every optional field into a hard requirement.
    #[test]
    fn disabled_still_defaults_to_false_when_the_key_is_absent() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        assert!(!t.disabled);
    }

    // -- Fix round 2: an explicit `null` must parse exactly like an absent
    // key (Kestra's own Jackson accepts `null` for these fields --
    // `Task.java:48`'s `description` is a plain nullable reference,
    // `Task.java:63`'s `disabled` is a boxed `Boolean` with
    // `@Builder.Default`) -- while a WRONG-TYPED, non-null value must still
    // be rejected. Round 1 fixed the wrong-type case and broke the null
    // case as a side effect; both need their own test or one regresses the
    // other again.

    #[test]
    fn explicit_null_description_parses_like_an_absent_one() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\ndescription: null\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        assert!(t.description.is_none());
    }

    #[test]
    fn explicit_null_disabled_parses_like_an_absent_one() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\ndisabled: null\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        assert!(!t.disabled);
    }

    /// Companion to the two tests above: `null` must parse, but a
    /// wrong-typed non-null value must still be rejected -- the exact
    /// distinction this round exists to draw.
    #[test]
    fn wrong_typed_non_null_disabled_is_still_rejected() {
        assert!(serde_norway::from_str::<Task>(
            "id: t\ntype: egret.sql\nsql: SELECT 1\ndisabled: \"true\"\n"
        )
        .is_err());
        assert!(serde_norway::from_str::<Task>(
            "id: t\ntype: egret.sql\nsql: SELECT 1\ndisabled: 1\n"
        )
        .is_err());
    }

    // -- W3c: `runIf`, the second task-level property that stops a task
    // running. Kestra's default is the STRING "true", not a boolean, and the
    // value it tests is the rendered string -- so this field must survive
    // parsing as source text and must not be coerced.

    /// **`runIf:` reaches the struct and does not stay inside a leaf's opaque
    /// `spec`.** That is the whole W3c model change, and it is the failure
    /// `disabled:` had for eight tasks: a field that parses somewhere the
    /// executor cannot read is a field the executor does not have.
    #[test]
    fn run_if_is_read_off_the_task_and_not_left_in_the_leafs_spec() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\nrunIf: \"{{ inputs.go }}\"\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        assert_eq!(t.run_if.as_deref(), Some("{{ inputs.go }}"));
        let TaskKind::Leaf { spec } = &t.kind else {
            panic!("egret.sql is a leaf");
        };
        assert!(
            spec.get("runIf").is_none(),
            "a field read off the task must be stripped from `rest`, or the \
             round-trip below emits it twice"
        );
    }

    #[test]
    fn run_if_is_none_when_the_key_is_absent() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        assert!(t.run_if.is_none());
    }

    /// An explicit `runIf: null` parses like an absent one, the same reading
    /// `description:` and `disabled:` already take. The divergence this may
    /// hide from Kestra is named on the field and derived unreachable by
    /// `no_corpus_flow_writes_a_null_run_if`.
    #[test]
    fn explicit_null_run_if_parses_like_an_absent_one() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\nrunIf: null\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        assert!(t.run_if.is_none());
    }

    /// **`runIf: false` (a YAML boolean) is a parse error, not a silent
    /// skip.** Kestra's field is a `String`, so a bare `false` is a type error
    /// there too — and the failure mode of accepting it quietly is the worst
    /// one available: an author who wrote it believes the task is conditional,
    /// and it would run unconditionally with `run_if: None`.
    #[test]
    fn a_boolean_run_if_is_a_parse_error_not_a_silent_none() {
        assert!(serde_norway::from_str::<Task>(
            "id: t\ntype: egret.sql\nsql: SELECT 1\nrunIf: false\n"
        )
        .is_err());
        assert!(serde_norway::from_str::<Task>(
            "id: t\ntype: egret.sql\nsql: SELECT 1\nrunIf: 0\n"
        )
        .is_err());
    }

    /// The round trip, which is what says `Serialize` emits `runIf` under
    /// Kestra's own key and only when the author wrote one.
    #[test]
    fn run_if_round_trips_and_an_absent_one_emits_no_key() {
        let yaml = "id: t\ntype: egret.sql\nsql: SELECT 1\nrunIf: \"false\"\n";
        let t: Task = serde_norway::from_str(yaml).unwrap();
        let out = serde_json::to_value(&t).unwrap();
        assert_eq!(out.get("runIf").and_then(|v| v.as_str()), Some("false"));
        let again: Task = serde_json::from_value(out).unwrap();
        assert_eq!(again, t);

        let bare: Task = serde_norway::from_str("id: t\ntype: egret.sql\nsql: SELECT 1\n").unwrap();
        assert!(
            serde_json::to_value(&bare).unwrap().get("runIf").is_none(),
            "Kestra's default is the string \"true\"; writing it back into every \
             serialised task would put a key into output no input had"
        );
    }
}
