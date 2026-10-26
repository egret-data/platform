// kestra: core/src/main/java/io/kestra/plugin/core/trigger/Flow.java
// kestra: executor/src/main/java/io/kestra/executor/FlowTriggerService.java
// kestra: executor/src/main/java/io/kestra/executor/DefaultExecutor.java
// kestra: core/src/main/java/io/kestra/core/services/ConditionService.java
// kestra: core/src/main/java/io/kestra/core/services/FlowService.java
// kestra: core/src/main/java/io/kestra/core/utils/TruthUtils.java

//! The **flow trigger's window-free arm** —
//! `io.kestra.plugin.core.trigger.Flow` with **no `dependsOn`**: a flow that
//! starts because *another* execution reached a state.
//!
//! # This is not "the flow trigger", and the name says so
//!
//! Kestra splits the plugin in two on `ListUtils.isEmpty(getDependsOn())`, and
//! the two arms are each other's negation — `computeExecutionsFromFlowTriggerConditions`
//! decides from **one execution**, `computeExecutionsFromFlowTriggerDependsOn`
//! accumulates a **`MultipleConditionWindow`** across many. This module is the
//! first arm only, which is why the type is called
//! [`WindowFreeFlowTrigger`] rather than anything that reads like the whole
//! plugin: the limitation is then repeated at every call site instead of
//! living in a doc nobody re-reads. **Twenty of the corpus's twenty-one
//! declarations take the arm this module does not implement**, and
//! `tests::the_corpus_flow_triggers_split_twenty_to_one` was written as the
//! acceptance criterion that fails the day the window lands. *(W13b Task 5,
//! 2026-09-26: the window landed and the test did not fail, because W13b built
//! the other arm BESIDE this type -- [`super::depends_on`], [`super::window`]
//! and `flow_trigger::process::listen_depends_on`, which the processor takes
//! before [`WindowFreeFlowTrigger::resolve`] is called. The twenty still take
//! that arm; `depends_on::tests::the_corpus_depends_on_declarations_resolve_eighteen_of_twenty`
//! is the count that moved.)*
//!
//! # The ratchet did not move, and could not have
//!
//! Seventeen of the 351 corpus flows declare a flow trigger and they carry
//! **twenty-one** declarations between them. **Zero baselined golden entries are
//! reachable through any of them**, and six of the 26 `not-standalone` corpus
//! flows exist only to be reached through one and have **no golden entry at
//! all**. The mechanism is spec §1.1's: `harvest.py`'s settle walks `parent_id`,
//! and a `Flow`-fired execution leaves that column **NULL** — it carries
//! `trigger_execution_id` instead. So `baseline.txt` reads **127 before this
//! module and 127 after**, and *it would read 127 if every line below were
//! wrong*. The tests in this file and the reviewer are the entire instrument,
//! which is where `egret.sql` (W3b), `egret.container` (W4) and
//! [`super::schedule`] (W6 Task 5) already stand.
//!
//! # The fact the whole package's link design rests on
//!
//! **A `Flow`-fired execution carries `trigger_execution_id` and `parentId`
//! NULL.** Measured on the pinned artifact
//! (`kestra/kestra@sha256:28f59bf1…`), not inherited from migration `0138`'s
//! comment:
//!
//! | round | executions | manual | trigger-fired | `parentId` |
//! |---|---|---|---|---|
//! | probe round 1 (18 listener flows) | 19 | 5 | 14 | `null` × 14 |
//! | probe round 2 (twin / self / disabled flow) | 9 | 2 | 7 | `null` × 7 |
//! | depth chains (three runs: →`d16`, →`d70`, →`d100`) | 189 | 3 | 186 | `null` × 186 |
//! | fix round 1 correlation chain (`c0`→`c3`) | 4 | 1 | 3 | `null` × 3 |
//! | **total** | **221** | **11** | **210** | **`null` × 210** |
//!
//! Every one of the 210 came back with `parentId` absent/`null` and with
//! `trigger.variables.executionId` naming the execution that caused it.
//!
//! The depth row's arithmetic closes against the per-flow counts rather than
//! being asserted: 17 + 71 + 101 = 189, and `d0` is the only flow that appears
//! in all three runs. **Fix round 1 corrected this from "211"** — two loose
//! figures that happened to nearly cancel: the depth chains' 189 counted the
//! three manual `d0` starts as trigger-fired, and round 2's 8 was eyeballed
//! where it should have been 7. Same rule as the corpus count below, caught the
//! same way and one round later.
//!
//! One example verbatim, `t6.listen/l-success` fired by `t6.probe/prod-ok`:
//!
//! ```text
//! "parentId": null,
//! "trigger": {
//!   "id": "t", "type": "io.kestra.plugin.core.trigger.Flow",
//!   "variables": { "executionId": "5cPAXnD34OjzRzJZ6iS1sO",
//!                  "namespace": "t6.probe", "flowId": "prod-ok",
//!                  "state": "SUCCESS", "flowRevision": 1, … } }
//! ```
//!
//! That is why [`FiredExecution`] names `trigger_execution_id` and has no
//! parent field at all: `oracle-exec/driver.rs` builds the oracle's `spawned`
//! from `parent_execution_id`, all 41 golden `spawned`-carrying entries have
//! zero `Subflow` children, and `workflow_executions_one_link_kind` (0138)
//! makes exactly one link kind representable per row. A flow trigger linked the
//! other way would turn Task 3's twelve banks into `spawned` mismatches.
//!
//! ## `trigger_execution_id` is the only link in the SCHEMA, not the only
//! ## reference on the artifact
//!
//! Fix round 1: an earlier draft of this file said `trigger_execution_id` was
//! *"the only link this execution has"*, and that is false of Kestra. The child
//! also carries a **`system.correlationId` label**, present on **14 of 14**
//! fired executions in probe round 1 and equal to the source's id in all
//! fourteen.
//!
//! **What that measurement cannot tell you, and this is the trap it walks
//! into:** every source in round 1 was top-level, so "the source's id" and "the
//! source's own `system.correlationId`" coincide on all fourteen rows. The
//! bytecode separates them — `lambda$evaluate$2` filters the *source's* label
//! list for `system.correlationId` and copies that **label** across verbatim.
//!
//! Settled live in fix round 1 with a four-deep chain, and the two really do
//! diverge:
//!
//! ```text
//! flow  execution id            trigger.…executionId          system.correlationId
//! c0    3v0vVBSXCK2kdCSP7zYW56  (started by hand)             3v0vVBSXCK2kdCSP7zYW56
//! c1    nQM2V2qD4oPnsVEIU0riz   3v0vVBSXCK2kdCSP7zYW56  (c0)  3v0vVBSXCK2kdCSP7zYW56
//! c2    1abGJbHjAdjdnbVDfyVPAO  nQM2V2qD4oPnsVEIU0riz   (c1)  3v0vVBSXCK2kdCSP7zYW56
//! c3    7VUqNzQzzGwXDsQhEdl9rz  1abGJbHjAdjdnbVDfyVPAO  (c2)  3v0vVBSXCK2kdCSP7zYW56
//! ```
//!
//! **The mechanism: the link is the immediate cause, and the correlationId is
//! whatever the source's own `system.correlationId` label held — copied
//! verbatim, never computed.** That sentence is exact. The convenient gloss
//! *"the correlationId is the root"* is **not**: it describes what the copying
//! produces in a chain Kestra started, and nothing more.
//!
//! Fix round 2, and the third time this one claim has been sharpened by a
//! deeper fixture: a root started with an explicit
//! `system.correlationId: ZZZTESTROOT` hands **`ZZZTESTROOT`** to all three
//! children — a value that is not an execution id at all. So "the root's id" is
//! true of machine-generated chains and false of the class, exactly as
//! "`system.correlationId` = the source execution's id" was true of top-level
//! sources and false of the class. Each correction came from someone building
//! the row the previous fixture did not contain; **keep the mechanism, distrust
//! every gloss over it.** See [`FiredExecution::correlation_id`].
//!
//! egret does not have this column, so [`FiredExecution::correlation_id`]
//! returns `None`. **Corrected, 2026-09-25 (W13a Task 5): this used to be a
//! forward-looking gap "owned by the wiring seam", and the seam now exists and
//! closes it.** `crate::flow_trigger::process::listen` sets
//! `trigger_execution_id` on the fired child, and
//! `worker::render::Surface::load`'s causal walk — corrected at W6 Task 8 to
//! follow WHICHEVER link a row carries rather than `parent_execution_id`
//! alone — reaches the source through it, the same way it already did for a
//! `Subflow` child. A flow-trigger child therefore renders
//! `labels.system.correlationId` as the chain's root id, not its own, closing
//! the gap this paragraph used to describe. What is still open is the SEEDED
//! case that walk was always going to miss: a root that declares an explicit
//! `system.correlationId` label hands that value, not an id, to every
//! descendant, and egret has no column to carry it — spec §8's "the
//! `system.correlationId` label", deliberately deferred, not this bug.
//!
//! # The filter, in the artifact's own order
//!
//! `DefaultExecutor.processFlowTriggers` is called **once per state transition
//! in the executor's batch, starting at index 1**, and hands each transition to
//! `FlowTriggerService.computeFlowTriggers(execution, flow)`, whose bytecode is:
//!
//! ```text
//!   0: flowService.removeUnwanted(flow, execution)   -- ifeq 59 -> emptyList
//!  12: ExecutionKind.isNormal(execution)             -- ifeq 59 -> emptyList
//!  19: flow.isDisabled()                             -- ifne 59 -> emptyList
//!  26: flow instanceof FlowWithException             -- ifne 59 -> emptyList
//!  33: flow.isDraft()                                -- ifne 59 -> emptyList
//!  40: flow.getTriggers() == null || isEmpty()       ->          emptyList
//!  84: executionDepthOrZero()+1 > maxDepth           -- warn  -> emptyList
//! 143: flowTriggers(flow)                            -- drops trigger.isDisabled()
//! 159: trigger.getStates().contains(state.current)
//! 170: conditionService.isValid(trigger, flow, runContext)   -- the `when`
//! ```
//!
//! [`evaluate`] is that list, in that order, with **two egret-own refusals in
//! front of it** — [`Refusal::DifferentWorkspace`] and [`Refusal::EntryState`],
//! both narrowings, both of which Kestra performs upstream rather than in this
//! filter — and two checks that cannot arise here dropped:
//! `FlowWithException` has no egret equivalent, and a trigger reaching
//! [`evaluate`] is by construction one the flow declares.
//!
//! # Over-acceptance is the hazard, so here is what is REFUSED
//!
//! The oracle cannot see a trigger that fires where Kestra would not. Three
//! things are therefore refused rather than guessed:
//!
//! 1. **`dependsOn` is refused at [`WindowFreeFlowTrigger::resolve`]**
//!    ([`FlowTriggerError::UnsupportedDependsOn`]). Kestra routes a trigger
//!    with a non-empty `dependsOn` down a *different* method
//!    (`computeExecutionsFromFlowTriggerDependsOn`) which evaluates a
//!    **`MultipleConditionWindow`** — persistent state, counted by
//!    `MultipleCondition.test` under `mode` ALL/ANY/AT_LEAST, and made
//!    fire-at-most-once by `window.fireOnce`. `valids/flow-trigger-fire-once-true-flow-listen`
//!    declares exactly that. *(W13b Task 1: that is the sibling checkout's
//!    `Window`; the pinned artifact has no `fireOnce` and refuses the flow with
//!    422, and since W13b Task 2 egret refuses it at parse.)* Firing once per matching execution, as a
//!    window-free implementation would, is over-acceptance in the direction
//!    nothing here can catch. **Twenty of the twenty-one corpus declarations
//!    carry a `dependsOn`**, so this refusal is most of the corpus — and
//!    refusing most of an unreachable corpus is the cheap half of the trade.
//!    *(Since W13b this refusal is routing, not a hole: the processor sends a
//!    `dependsOn` listener to its own arm before `resolve` runs, and there it
//!    fires inside a window. `resolve` still refuses it, so no caller can take
//!    a `dependsOn` trigger down THIS arm by mistake.)*
//! 2. **A `when` that the caller could not render refuses the trigger**
//!    ([`Refusal::WhenUnrenderable`]), because `ConditionService.isNotValid`
//!    catches `IllegalVariableEvaluationException` and returns `true`. Measured:
//!    a listener with `when: 'x{{ missingVar.someField }}'` — an expression that
//!    would render to the *truthy* `"x"` if a missing variable were tolerated —
//!    did **not** fire, while `when: 'no'` beside it did.
//! 3. **`when` is not rendered here.** It is a Jinja expression over the
//!    *source* execution's context (`runContextFactory.of(null, execution)` —
//!    measured: a `when` reading `labels.t6` saw the **producer's** labels), and
//!    this module has no render surface, exactly as [`super::ScheduledExecution`]
//!    carries unrendered `inputs`. The caller renders and passes a
//!    [`WhenVerdict`]; passing [`WhenVerdict::NotDeclared`] for a trigger that
//!    *does* declare a `when` is refused
//!    ([`Refusal::WhenNotRendered`]) rather than accepted.
//!
//! # `window:` without `dependsOn` — W13b refuses it at save, and why this section argued otherwise
//!
//! **Superseded by W13b Task 2:** `spec::validate` now refuses a `window` on a
//! trigger with no `dependsOn` (`window_without_depends_on`). The measurement
//! below still stands — on the artifact such a window is inert — and that is
//! exactly why it is refused: egret does not silently ignore a declaration.
//! It is a chosen over-refusal, and no corpus flow declares one. The analysis
//! is kept because it records what the artifact does.
//!
//! Fix round 1 considered refusing a declared `window:` by analogy with Task
//! 5's `lateMaximumDelay` — a modelled-away field that silently changed the
//! behaviour of the path that task implemented. **Measured, the analogy does
//! not hold, and the difference is the whole of it:**
//!
//! | | `lateMaximumDelay` (Task 5) | `window` (here) |
//! |---|---|---|
//! | consumed on the arm that ships? | **yes** — `Schedule.eval`'s own path | **no** |
//! | semantic call sites | `Schedule.nextEvaluationDate` / `eval` | **none** |
//!
//! All **four** `getWindow()` call sites inside `Flow` are Lombok-generated —
//! `toString` (offset 26), `equals` (187, 193) and `hashCode` (114) — plus the
//! getter's own declaration. Jar-wide, only `TimeWindow` and
//! `TimeWindowValidator` name it, and the value reaches `MultipleCondition`
//! solely through `dependsOnAsMultipleCondition()`, i.e. **only on the arm
//! [`WindowFreeFlowTrigger::resolve`] already refuses**. So a `window:` on a
//! trigger this module accepts is inert on the artifact too, and refusing it
//! would refuse a flow Kestra runs — over-*refusal* manufactured out of a
//! near-miss analogy. Recorded because this is the kind of suggestion that gets
//! re-proposed.
//!
//! # Measured, derived, or chosen
//!
//! Every general sentence in this file is one of the three, and the chosen ones
//! say so. **The chosen decisions:**
//!
//! 1. The name [`WindowFreeFlowTrigger`] — `models::FlowTrigger` already holds the wire
//!    name, and two `FlowTrigger`s in one crate is a footgun.
//! 2. [`evaluate`] being pure over `(listener, flow, source, when, max_depth)`,
//!    mirroring [`super::tick`] and `executor::advance()`. Kestra's shape is a
//!    service with six injected beans.
//! 3. The [`Refusal`] enum existing at all, and every refusal being **named**
//!    rather than folded into a `bool`. Kestra returns an empty list and, for
//!    the depth case only, logs.
//! 4. Refusing `dependsOn` at resolve time instead of modelling the window
//!    (item 1 above). **W13b implements the window outside this type**
//!    (`crate::flow_trigger::process`, with `super::depends_on` and
//!    `super::window` as its pure half); it reaches [`evaluate`] only as the
//!    listener filter both arms share, through
//!    [`WindowFreeFlowTrigger::depends_on_filter`].
//! 5. [`Refusal::EntryState`] — a defensive refusal of `CREATED` that Kestra
//!    achieves upstream, by starting its transition loop at index 1.
//! 6. [`FiredExecution`] carrying the source execution's *identity* rather than
//!    the rendered `trigger.*` variable map. `Flow.Output` has eleven fields and
//!    this package has not measured how Jackson formats all of them; the same
//!    call [`super::ScheduledExecution`] made.
//! 7. `max_depth` being a parameter with [`DEFAULT_MAX_EXECUTION_DEPTH`] beside
//!    it rather than a hard-coded constant.
//! 8. [`Refusal::DifferentWorkspace`] — **egret's, not Kestra's.** Kestra's
//!    candidate list is already tenant-scoped before `computeFlowTriggers` sees
//!    it (`withFlowTriggersOnly` reads one tenant's repository), so the artifact
//!    has no such check to copy. This repository has shipped the other answer
//!    once already — five id columns that named a row in another workspace, one
//!    of which (`chart_id`) allowed a cross-workspace read — and 0138's
//!    composite foreign keys exist because of it. A `(namespace, flowId)` pair
//!    with no workspace is that same shape, so the identity pair carries one
//!    and a mismatch refuses.
//! 9. [`FiredExecution::labels`] carrying the trigger's declared `labels:`
//!    **unrendered**, rather than dropping them as the first draft did. See
//!    that field.
//!
//! Two claims that read like measurements and are **derived**:
//! `Refusal::FlowDraft` and `Refusal::NotNormalKind` come from the bytecode
//! above and were never produced on the artifact — no probe published a draft
//! flow or started a `TEST`/`PLAYGROUND` execution. `Refusal::FlowDisabled`
//! beside them *was* measured (a listener flow with `disabled: true` produced
//! zero executions while its twin produced three).
//!
//! # What `dependsOn` would need, measured here so the next package need not
//!
//! `Flow$DependencyCondition.test(ctx)`, offset by offset, is a **per-execution**
//! predicate — and its namespace match is **exact equality, not a prefix**:
//!
//! ```text
//!   7: dependency.namespace != null && !execution.getNamespace().equals(ns) -> false
//!  39: dependency.flowId    != null && !execution.getFlowId().equals(id)    -> false
//!  64: execution.labels contains Label("system.simulatedExecution","true")  -> TRUE
//!  94: dependency.states    != null && !states.contains(state.current)      -> false
//! 131: any declared label NOT in execution.labels                           -> false
//! 179: TruthUtils.isTruthy(render(dependency.when).orElse("true"))
//! ```
//!
//! Live confirmation of the namespace rule, two-sided: a listener whose only
//! dependency was `namespace: t6.probe` fired on **all four** executions in
//! `t6.probe` and on **none** of the execution in `t6.probe.sub`. Note also that
//! the `simulatedExecution` short-circuit sits *after* namespace/flowId and
//! *before* states — an accept, not a reject, and the only early `true` in the
//! method. None of this is implemented here; it is the half a window package
//! will not have to re-measure.

use std::collections::BTreeMap;
use std::fmt;

use uuid::Uuid;

use crate::executor::is_truthy;
use crate::models::{ExecutionKind, Label, State, Trigger, TriggerKind};

/// `kestra.execution.depth.max-depth`.
///
/// **Measured twice and both ways.** The value is
/// `@Bindable(defaultValue = "100")` on `ExecutionDepthConfiguration`'s
/// `maxDepth` constructor argument, read out of the Micronaut bean definition's
/// constant pool (`#121 = Utf8 100`) in the pinned image. A chain of 105 flows
/// each triggered by the previous, fired once on that image, produced
/// executions up to **`d100` with `metadata.executionDepth: 100`** and
/// **`d101` was never created** — nor were `d102`…`d105`.
pub const DEFAULT_MAX_EXECUTION_DEPTH: u32 = 100;

/// Why a `Trigger` could not be resolved into a [`WindowFreeFlowTrigger`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowTriggerError {
    /// Not an `io.kestra.plugin.core.trigger.Flow` at all.
    NotAFlowTrigger,
    /// The trigger declares `dependsOn`, whose evaluation needs a
    /// `MultipleConditionWindow` -- the other arm, which this type does not
    /// model and [`super::depends_on`] / [`super::window`] do (W13b). See the
    /// module doc, *Over-acceptance is the hazard*, item 1.
    UnsupportedDependsOn(usize),
    /// The trigger has no `dependsOn` but carries a shape the save path
    /// refuses: the codes of [`super::depends_on_refusals`], the ONE list
    /// `spec::validate` reports about a flow trigger. On this arm that is
    /// either a `window` (inert on the artifact, which reads it only through
    /// `dependsOnAsMultipleCondition()`; `window_without_depends_on`, plus
    /// whatever the window itself breaks) or a `mode`/`minSatisfied` shape
    /// (`min_satisfied_required`, `min_satisfied_not_positive`,
    /// `min_satisfied_exceeds_conditions` -- Kestra's `FlowTriggerValidator`
    /// and `@Positive` do not look at `dependsOn` either).
    ///
    /// Refusing here as well is what keeps a DRAFT (which saves with
    /// validation errors, and whose listener rows `scheduler::sync` builds
    /// from whatever parses) from firing with a declaration save called
    /// unfireable. W13b Task 2 fix round 1 did this for `window` alone; the
    /// final-review fix wave (I1) widened it to the whole list, so the
    /// window-free arm cannot drift from the save path by construction.
    Refused(Vec<&'static str>),
}

impl fmt::Display for FlowTriggerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAFlowTrigger => write!(f, "trigger is not a core.trigger.Flow"),
            Self::UnsupportedDependsOn(n) => write!(
                f,
                "core.trigger.Flow `dependsOn` ({n} condition(s)) is not the \
                 window-free arm: it is evaluated over a window by the `dependsOn` arm"
            ),
            Self::Refused(codes) => write!(
                f,
                "core.trigger.Flow without `dependsOn` is refused at save ({}), so it \
                 is never evaluated",
                codes.join(", ")
            ),
        }
    }
}

impl std::error::Error for FlowTriggerError {}

/// A `io.kestra.plugin.core.trigger.Flow` resolved far enough to be evaluated
/// against another execution.
///
/// **Named `WindowFreeFlowTrigger` rather than `FlowTrigger`, and that is chosen:**
/// [`crate::models::FlowTrigger`] already carries the wire identity, and one
/// crate holding two types called `FlowTrigger` is a mistake waiting for its
/// first `use`.
// No `Eq`: this holds `models::Label`, which derives `PartialEq` only.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowFreeFlowTrigger {
    pub trigger_id: String,
    /// `Flow.states`, defaulted by `models::FlowTrigger`'s
    /// `default_flow_trigger_states()` to `terminatedTypes() + [PAUSED]`.
    ///
    /// **Membership is the whole rule; terminality is not part of it.** See
    /// [`Refusal::StateNotListed`].
    states: Vec<State>,
    /// `AbstractTrigger.when`, **unrendered**. `None` when the flow declares
    /// `when: null` explicitly; `Some("true")` when the key is absent, because
    /// `models::Trigger` supplies Kestra's own default.
    when_source: Option<String>,
    disabled: bool,
    /// `Flow.inputs`, **unrendered** — the same call [`super::ScheduledExecution`]
    /// makes. Kestra renders them through a `RunContext` seeded with
    /// `{trigger: {outputs: …}}` and, if that render throws, still creates the
    /// execution and immediately moves it to `FAILED`
    /// (`Flow.evaluate`'s catch block, offsets 435-488). Nothing here can do
    /// that without the render surface, so nothing here tries.
    inputs: BTreeMap<String, serde_json::Value>,
    /// `AbstractTrigger.labels`, **unrendered**.
    ///
    /// **Fix round 1: the first draft dropped this field on the floor**, which
    /// is the `lateMaximumDelay` failure with a new name — `models::Trigger`
    /// *models* `labels`, so a flow declaring them parsed, resolved and fired
    /// without them and nothing said so. `Flow.evaluate` calls
    /// `LabelService.fromTrigger(runContext, this, Map.of("trigger", vars))`,
    /// which iterates `trigger.getLabels()` minus the `system.` ones, **renders
    /// each value** through the run context, drops the ones that render to
    /// null, and attaches the rest to the execution it creates. The rendering
    /// is the caller's for the same reason `when` and `inputs` are; carrying
    /// them verbatim is what keeps the seam able to do it at all.
    ///
    /// **Measured, not merely read off the bytecode:** the fix-round-1 chain
    /// declared a label whose value was an expression over the trigger context
    /// on each of `c1`/`c2`/`c3`, and the fired executions came back carrying
    /// `declared: c1-c0`, `declared: c2-c1`, `declared: c3-c2`. So the values
    /// are real, they render against the **source's** context exactly as `when`
    /// does, and a dropped `labels:` loses them from the execution.
    labels: Vec<Label>,
}

impl WindowFreeFlowTrigger {
    /// Resolves a parsed `Trigger`, or says why it cannot be evaluated.
    pub fn resolve(trigger: &Trigger) -> Result<Self, FlowTriggerError> {
        let TriggerKind::Flow(spec) = &trigger.kind else {
            return Err(FlowTriggerError::NotAFlowTrigger);
        };

        // `FlowTriggerService` splits on exactly this: `dependsOn` empty goes to
        // `computeExecutionsFromFlowTriggerConditions`, non-empty goes to
        // `computeExecutionsFromFlowTriggerDependsOn` and a window. The two
        // lambdas are each other's negation (`ListUtils.isEmpty` and
        // `!ListUtils.isEmpty`), so there is no third path and no overlap.
        if !spec.depends_on.is_empty() {
            return Err(FlowTriggerError::UnsupportedDependsOn(
                spec.depends_on.len(),
            ));
        }
        // Whatever save refuses about this trigger -- a `window`, or a
        // `mode`/`minSatisfied` shape -- is refused here too, never ignored.
        // The SAME list save reports, not a second copy. See
        // [`FlowTriggerError::Refused`].
        let refusals = super::depends_on_refusals(spec);
        if !refusals.is_empty() {
            return Err(FlowTriggerError::Refused(
                refusals.iter().map(|r| r.code).collect(),
            ));
        }

        Ok(Self::filter_of(trigger, spec))
    }

    /// The **listener filter** of a trigger that DOES declare `dependsOn`
    /// (W13b), or `None` for any other trigger.
    ///
    /// `FlowTriggerService.computeFlowTriggers` (checkout `96893b589`, lines
    /// 216-243) runs BEFORE the split into the two arms, so both arms apply
    /// the same filter -- same flow, kind, flow disabled, draft, depth,
    /// trigger disabled, `states`, and the trigger's own `when` -- and only
    /// then does `computeExecutionsFromFlowTriggerDependsOn` touch a window.
    /// [`evaluate`] IS that filter, so the `dependsOn` arm calls it with this
    /// value rather than restating it; its [`Outcome::Fired`] is the fire's
    /// identity (inputs, labels, depth, source) once the window is satisfied.
    /// `evaluate`'s semantics are unchanged; what this adds is a way to reach
    /// it that [`Self::resolve`]'s refusal of `dependsOn` does not open.
    ///
    /// **Not a resolution of the `dependsOn` machinery**: `dependsOn`, `mode`,
    /// `minSatisfied` and `window` are the caller's
    /// (`crate::flow_trigger::process`), gated by
    /// [`super::depends_on_refusals`].
    pub fn depends_on_filter(trigger: &Trigger) -> Option<Self> {
        let TriggerKind::Flow(spec) = &trigger.kind else {
            return None;
        };
        if spec.depends_on.is_empty() {
            return None;
        }
        Some(Self::filter_of(trigger, spec))
    }

    fn filter_of(trigger: &Trigger, spec: &crate::models::FlowTrigger) -> Self {
        Self {
            trigger_id: trigger.id.clone(),
            states: spec.states.clone(),
            when_source: trigger.when.as_ref().map(|w| w.source().to_string()),
            disabled: trigger.disabled,
            inputs: spec
                .inputs
                .as_ref()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
            labels: trigger.labels.clone(),
        }
    }

    /// The states this trigger fires on.
    pub fn states(&self) -> &[State] {
        &self.states
    }

    /// The `when` expression the caller must render, or `None` when the trigger
    /// declares none. A caller that ignores this and passes
    /// [`WhenVerdict::NotDeclared`] anyway is refused, not obeyed.
    pub fn when_source(&self) -> Option<&str> {
        self.when_source.as_deref()
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// The flow that DECLARES the trigger — the one that would start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListeningFlow {
    /// **Not Kestra's**, and see [`Refusal::DifferentWorkspace`]: a
    /// `(namespace, flowId)` pair with no workspace is the shape this
    /// repository has already leaked through once.
    pub workspace_id: Uuid,
    pub namespace: String,
    pub flow_id: String,
    /// `Flow.isDisabled()`. **Measured:** a listener flow carrying
    /// `disabled: true` produced zero executions over two source executions its
    /// twin reacted to three times.
    pub disabled: bool,
    /// `Flow.isDraft()`. **Derived from bytecode, never produced on the
    /// artifact** — no probe published a draft flow.
    pub draft: bool,
}

/// The execution that just reached a state — Kestra's `Execution` argument,
/// reduced to what `computeFlowTriggers` reads off it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceExecution {
    pub id: Uuid,
    /// The workspace the source execution belongs to. See
    /// [`Refusal::DifferentWorkspace`].
    pub workspace_id: Uuid,
    pub namespace: String,
    pub flow_id: String,
    /// The transition being evaluated, i.e. `execution.getState().getCurrent()`
    /// for the `withState(...)` copy `DefaultExecutor` hands to
    /// `processFlowTriggers` — **not necessarily the execution's final state**.
    pub state: State,
    /// `Execution.kind`. `None` is `NORMAL` (`ExecutionKind.isNormal` treats a
    /// null kind as normal), matching `models::Execution::kind`.
    pub kind: Option<ExecutionKind>,
    /// `metadata.executionDepthOrZero()` — `None` for a top-level execution,
    /// which the artifact reads as `0` (measured: a manually started execution
    /// comes back with `metadata` carrying no `executionDepth` key at all, and
    /// the execution it triggers carries `executionDepth: 1`).
    pub execution_depth: Option<u32>,
}

/// What the caller did with [`WindowFreeFlowTrigger::when_source`].
///
/// Three variants because `ConditionService.isNotValid` has three outcomes and
/// they are not interchangeable: a rendered string is tested, a render failure
/// **refuses**, and no `when` at all accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhenVerdict {
    /// The trigger declares no `when`.
    NotDeclared,
    /// The caller rendered it to this.
    Rendered(String),
    /// The caller could not render it.
    ///
    /// `isNotValid` catches `IllegalVariableEvaluationException` and returns
    /// `true`, so `isValid` is `false` and the trigger does not fire —
    /// measured, see the module doc.
    Unrenderable,
}

/// Why a flow trigger did not fire. Every one of these is a place Kestra
/// returns an empty list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The listening flow and the source execution are in different
    /// workspaces.
    ///
    /// **Chosen, egret's own, and first — Kestra has no such check because it
    /// never needs one.** `DefaultExecutor` feeds `computeFlowTriggers` from
    /// `withFlowTriggersOnly(…)` over one tenant's flow repository, so a
    /// cross-tenant pair is already unrepresentable by the time the filter
    /// runs. **Corrected, 2026-09-25 (W13a Task 5): egret's caller now has that
    /// same guarantee, structurally, and this refusal is kept as the second
    /// line rather than the only one.** `crate::flow_trigger::process`'s
    /// listener query reads `WHERE l.workspace_id = $1` bound to the event's
    /// own `workspace_id`, so a cross-workspace pair is unrepresentable by the
    /// time this function sees it too — the same shape as `withFlowTriggersOnly`,
    /// not an argument for deleting the check. This repository has paid for
    /// the optimistic answer once already: five id columns that could name a
    /// row in another workspace, one of which allowed a cross-workspace read.
    /// Migration 0138's foreign keys are composite `(id, workspace_id)` for
    /// exactly that reason, so a writer that ignored this would be refused by
    /// the database — but only *after* this function had said `Fired`, which is
    /// the wrong place to find out.
    DifferentWorkspace,
    /// The transition offered is `CREATED`.
    ///
    /// **Chosen, and it is a narrowing.** Kestra never asks the filter about an
    /// execution's entry state: `DefaultExecutor` loops
    /// `for (int i = 1; i < stateTransitions.size(); i++)` and `CREATED` is
    /// always element 0 of an execution's history. Measured: a listener
    /// declaring `states: [CREATED]`, beside listeners on `RUNNING`, `SUCCESS`,
    /// `FAILED` and `WARNING` that all fired, produced **zero** executions.
    /// Enforcing it here rather than trusting every future caller refuses a
    /// case the artifact refuses, which is the safe direction.
    EntryState,
    /// `FlowService.removeUnwanted` — the listening flow IS the source's flow.
    ///
    /// The comparison is `flow.uidWithoutRevision().equals(FlowId.uidWithoutRevision(execution))`,
    /// i.e. the **pair** `(namespace, flowId)`. **Measured with all three
    /// discriminating rows**, because a fixture that differs in both halves
    /// cannot tell "same pair" from "same namespace" or "same id":
    ///
    /// | listener | source | fired |
    /// |---|---|---|
    /// | `r2.c/selftwin` | `r2.c/selftwin` | **no**, twice over |
    /// | `r2.a/other` | `r2.a/twin` | yes, 3× |
    /// | `r2.b/twin` | `r2.a/twin` | yes, 3× |
    SameFlow,
    /// `ExecutionKind.isNormal` — a `TEST`, `PLAYGROUND` or `LOOP` execution
    /// fires nothing. **Derived from bytecode; not produced on the artifact.**
    NotNormalKind(ExecutionKind),
    /// The listening flow is disabled. **Measured.**
    FlowDisabled,
    /// The listening flow is a draft. **Derived from bytecode; not produced on
    /// the artifact.**
    FlowDraft,
    /// `executionDepthOrZero() + 1 > maxDepth`. See
    /// [`DEFAULT_MAX_EXECUTION_DEPTH`] for the two-sided measurement.
    DepthExceeded { depth: u32, max: u32 },
    /// The trigger itself is disabled. **Measured:** a listener whose trigger
    /// carried `disabled: true` produced zero executions where its twin fired.
    ///
    /// Note this is NOT [`super::TickOutcome::NotSchedulable`]'s situation:
    /// a flow trigger holds no scheduling state, so there is no slot to
    /// consume and nothing to freeze.
    TriggerDisabled,
    /// `trigger.getStates().contains(execution.getState().getCurrent())` was
    /// false.
    ///
    /// **`states` is a membership test and nothing else — terminality is not
    /// part of it, and saying "the flow trigger fires on terminal states" is
    /// false.** Measured: a listener declaring `states: [RUNNING]` fired
    /// **twice**, once for each source execution that passed through `RUNNING`.
    /// `trigger-flow-listener-with-concurrency-limit` in the corpus declares
    /// `states: [QUEUED, RUNNING, SUCCESS]` for the same reason. What is true is
    /// narrower: the *default* list is `terminatedTypes() + [PAUSED]`.
    StateNotListed(State),
    /// The rendered `when` was one of Kestra's four falsy strings.
    ///
    /// `TruthUtils.FALSE_VALUES` is `["false", "0", "-0", ""]`, compared after
    /// trimming. Measured on the artifact, all four plus a padded one plus a
    /// non-member: `"false"`, `"0"`, `"-0"`, `""` and `"  false  "` each
    /// produced zero executions; `"no"`, `"true"` and an absent `when` each
    /// produced one.
    WhenFalsy(String),
    /// The caller could not render the `when`. See [`WhenVerdict::Unrenderable`].
    WhenUnrenderable,
    /// The trigger declares a `when` and the caller passed
    /// [`WhenVerdict::NotDeclared`].
    ///
    /// **Chosen.** It is a caller bug, not an artifact behaviour, and the only
    /// two answers available are "refuse" and "fire without evaluating the
    /// condition". The second is over-acceptance, so this is the first.
    WhenNotRendered,
}

/// The outcome of one `(trigger, execution)` pair.
// No `Eq`: `FiredExecution` holds `models::Label`. See there.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Fired(Box<FiredExecution>),
    Refused(Refusal),
}

/// The execution a fired flow trigger asks for.
// No `Eq`: `models::Label` derives `PartialEq` only, and a `FiredExecution`
// carries the trigger's declared labels.
#[derive(Debug, Clone, PartialEq)]
pub struct FiredExecution {
    /// The LISTENING flow — the one that runs.
    pub namespace: String,
    pub flow_id: String,
    pub trigger_id: String,
    /// **`workflow.executions.trigger_execution_id` (migration 0138) — the
    /// only link in the SCHEMA, and not the only reference on the artifact.**
    /// Kestra's `Execution.trigger.variables.executionId`, which names the
    /// **immediate** cause.
    ///
    /// The unqualified form of this sentence — *"the only link this execution
    /// has"* — shipped in the first draft and is false: the child also carries
    /// a `system.correlationId` label, which egret does not model. See this
    /// module's doc and [`Self::correlation_id`]. Fix round 2 removed this
    /// copy; fix round 1 had corrected only the one in the module doc, which is
    /// how a corrected claim survives in a second place.
    pub trigger_execution_id: Uuid,
    /// `metadata.executionDepth` for the new row: the source's depth plus one.
    pub execution_depth: u32,
    /// `Flow.inputs`, **unrendered** — see [`WindowFreeFlowTrigger::inputs`].
    pub inputs: BTreeMap<String, serde_json::Value>,
    /// The trigger's declared `labels:`, **unrendered** — see
    /// [`WindowFreeFlowTrigger::labels`].
    pub labels: Vec<Label>,
}

impl FiredExecution {
    /// **Always `None`, and the whole package depends on it.**
    ///
    /// `driver.rs` builds the oracle's `spawned` field from
    /// `parent_execution_id`; a flow-trigger child reachable that way would
    /// appear in egret's trace where the golden has nothing.
    /// `workflow_executions_one_link_kind` refuses the row that sets both.
    pub const fn parent_execution_id(&self) -> Option<Uuid> {
        None
    }

    /// The `system.correlationId` label Kestra attaches to the child.
    ///
    /// **This is NOT `trigger_execution_id`, and the difference only shows in a
    /// chain.** `Flow.evaluate` copies the *source's own* `system.correlationId`
    /// label across verbatim (`lambda$evaluate$2`), so the child inherits
    /// **whatever the source carried** rather than its immediate cause's id.
    /// Probe round 1 could not tell the two apart — all fourteen of its sources
    /// were top-level, where they coincide. A four-deep chain (fix round 1)
    /// does: `c1`/`c2`/`c3` each carry `trigger.variables.executionId` = their
    /// immediate cause and `system.correlationId` = `c0`.
    ///
    /// **And "= the root's id" is itself a gloss** (fix round 2): a root
    /// started with an explicit `system.correlationId: ZZZTESTROOT` gives all
    /// three children `ZZZTESTROOT`, which is not an execution id. The label is
    /// copied, not derived — that is the whole rule.
    ///
    /// It returns `None` because **egret has no such column**; the source's
    /// label is not this struct's to keep. **Corrected, 2026-09-25 (W13a Task
    /// 5): the wiring seam this doc used to say did not exist yet now does.**
    /// `crate::flow_trigger::process::listen` sets `trigger_execution_id` on
    /// the fired child, and `worker::render::Surface::load`'s causal walk
    /// (corrected at W6 Task 8 to follow whichever link a row carries) reaches
    /// through it and renders `labels.system.correlationId` as the chain's
    /// root id — the module doc's derivation, not the child's own id. The
    /// SEEDED case is the one this still cannot answer, deliberately (spec
    /// §8): see the module doc.
    pub const fn correlation_id(&self) -> Option<Uuid> {
        None
    }

    /// **Always `None`.** `trigger_run_ix` addresses *which taskrun* of
    /// `trigger_execution_id` caused this execution, which is a question a
    /// `Subflow` has an answer to and a flow trigger does not: it is caused by
    /// an execution reaching a state and by no particular run. 0138's column
    /// comment predicted exactly this, and
    /// `workflow_executions_trigger_run_needs_its_execution` is the constraint
    /// that keeps the ordinal from existing without its execution.
    pub const fn trigger_run_ix(&self) -> Option<i32> {
        None
    }
}

/// One `(trigger, execution)` decision — `FlowTriggerService.computeFlowTriggers`
/// plus `Flow.evaluate`, pure over its arguments.
///
/// The order of the checks is the artifact's (see the module doc), which
/// matters because the [`Refusal`] returned is the *first* reason, not an
/// arbitrary one.
pub fn evaluate(
    listener: &WindowFreeFlowTrigger,
    flow: &ListeningFlow,
    source: &SourceExecution,
    when: &WhenVerdict,
    max_depth: u32,
) -> Outcome {
    // egret's own, and in front of the artifact's list because Kestra achieves
    // it upstream: its candidate flows come from one tenant's repository.
    if flow.workspace_id != source.workspace_id {
        return Outcome::Refused(Refusal::DifferentWorkspace);
    }

    // Moved to the front, and it is a REFUSAL Kestra performs by never asking.
    // See `Refusal::EntryState`.
    if source.state == State::Created {
        return Outcome::Refused(Refusal::EntryState);
    }

    // `FlowService.removeUnwanted`: the pair, not either half.
    if flow.namespace == source.namespace && flow.flow_id == source.flow_id {
        return Outcome::Refused(Refusal::SameFlow);
    }

    // `ExecutionKind.isNormal`: a null kind IS normal.
    if let Some(kind) = source.kind {
        if kind != ExecutionKind::Normal {
            return Outcome::Refused(Refusal::NotNormalKind(kind));
        }
    }

    if flow.disabled {
        return Outcome::Refused(Refusal::FlowDisabled);
    }
    if flow.draft {
        return Outcome::Refused(Refusal::FlowDraft);
    }

    // `executionDepthOrZero() + 1 > maxDepth` -- strictly greater, so a chain
    // that reaches exactly `max_depth` runs and the next one does not.
    let depth = source.execution_depth.unwrap_or(0) + 1;
    if depth > max_depth {
        return Outcome::Refused(Refusal::DepthExceeded {
            depth,
            max: max_depth,
        });
    }

    // `flowTriggers(flow)` drops disabled triggers before the state filter.
    if listener.disabled {
        return Outcome::Refused(Refusal::TriggerDisabled);
    }

    // # This is the one place `PAUSED` is live in production today
    //
    // **W8 Task 2 fix round 2, and the sweeps that found it took five methods.**
    // `models::FlowTrigger`'s `default_flow_trigger_states()` is
    // `terminatedTypes() + [PAUSED]`, so a flow trigger that declares no
    // `states:` fires on a source execution in `PAUSED` — and this line is
    // what honours that. Neither of W8 Task 2's first four sweeps reached this
    // file: they were gated on a state NAME or on a judgement FORM, and this
    // line carries neither an `is_terminal` nor a `State::` literal.
    //
    // **No code change was needed here and that is measured, not assumed.**
    // This plane is `models::State`, which has held `Paused` since W1; adding
    // `executor::RunState::Paused` does not reach it. **Corrected, 2026-09-25
    // (W13a Task 5): the bridge this paragraph said did not exist is built.**
    // `SourceExecution` now has a production constructor,
    // `crate::flow_trigger::process::load_source`, and `settle`'s outbox hook
    // (spec §4.3) writes an event for a `PAUSED` transition same as any other
    // -- it is not gated on terminality, because a Flow trigger's default
    // `states` includes `PAUSED`. `process_one` converts the event's stored
    // `RunState::wire()` string back into `models::State` before calling this
    // predicate (`run_state_wire_is_the_model_states_serde_spelling` pins the
    // spelling both sides trust), so a paused EXECUTION fires a listener
    // before it resumes -- Kestra's answer, which is why `PAUSED` was in the
    // default list at all, is the one egret gives too.
    if !listener.states.contains(&source.state) {
        return Outcome::Refused(Refusal::StateNotListed(source.state));
    }

    // `ConditionService.isValid(trigger, flow, runContext)` ==
    // `!isNotValid(...)` == `!TruthUtils.isFalsy(render(when))`, with a render
    // exception meaning "not valid".
    //
    // `isFalsy` and `isTruthy` are NOT each other's negation on the artifact --
    // both answer `false` for a Java `null` -- so the two differ at exactly one
    // input, an absent `when`, and there the artifact FIRES. That is
    // `WhenVerdict::NotDeclared` below, and it is why this is a match on three
    // variants rather than a call to `is_truthy` over an `Option`.
    match when {
        WhenVerdict::Unrenderable => return Outcome::Refused(Refusal::WhenUnrenderable),
        WhenVerdict::NotDeclared if listener.when_source.is_some() => {
            return Outcome::Refused(Refusal::WhenNotRendered);
        }
        WhenVerdict::NotDeclared => {}
        WhenVerdict::Rendered(rendered) => {
            if !is_truthy(rendered) {
                return Outcome::Refused(Refusal::WhenFalsy(rendered.clone()));
            }
        }
    }

    Outcome::Fired(Box::new(FiredExecution {
        namespace: flow.namespace.clone(),
        flow_id: flow.flow_id.clone(),
        trigger_id: listener.trigger_id.clone(),
        trigger_execution_id: source.id,
        execution_depth: depth,
        inputs: listener.inputs.clone(),
        labels: listener.labels.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        evaluate, FiredExecution, FlowTriggerError, ListeningFlow, Outcome, Refusal,
        SourceExecution, WhenVerdict, WindowFreeFlowTrigger, DEFAULT_MAX_EXECUTION_DEPTH,
    };
    use crate::models::{ExecutionKind, State, Trigger};
    use uuid::Uuid;

    fn ws() -> Uuid {
        Uuid::parse_str("99999999-8888-7777-6666-555555555555").unwrap()
    }

    fn other_ws() -> Uuid {
        Uuid::parse_str("00000000-8888-7777-6666-555555555555").unwrap()
    }

    fn trigger(yaml: &str) -> Trigger {
        serde_norway::from_str(yaml).unwrap_or_else(|e| panic!("bad fixture trigger: {e}\n{yaml}"))
    }

    /// A flow trigger with no `states`, no `when`, no `dependsOn`.
    fn plain() -> WindowFreeFlowTrigger {
        WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n",
        ))
        .expect("plain flow trigger resolves")
    }

    fn listening() -> ListeningFlow {
        ListeningFlow {
            workspace_id: ws(),
            namespace: "t6.listen".into(),
            flow_id: "listener".into(),
            disabled: false,
            draft: false,
        }
    }

    fn src_id() -> Uuid {
        Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap()
    }

    fn source(state: State) -> SourceExecution {
        SourceExecution {
            id: src_id(),
            workspace_id: ws(),
            namespace: "t6.probe".into(),
            flow_id: "producer".into(),
            state,
            kind: None,
            execution_depth: None,
        }
    }

    /// What a caller does: render the `when` if there is one, then evaluate.
    /// The fixtures below that declare no `when:` key still carry Kestra's
    /// default of `"true"`, so this renders it -- passing `NotDeclared` for
    /// them would exercise `Refusal::WhenNotRendered` and nothing else.
    fn run(l: &WindowFreeFlowTrigger, f: &ListeningFlow, s: &SourceExecution) -> Outcome {
        let when = match l.when_source() {
            None => WhenVerdict::NotDeclared,
            Some(src) => WhenVerdict::Rendered(src.to_string()),
        };
        evaluate(l, f, s, &when, DEFAULT_MAX_EXECUTION_DEPTH)
    }

    fn refusal(o: Outcome) -> Refusal {
        match o {
            Outcome::Refused(r) => r,
            Outcome::Fired(e) => panic!("expected a refusal, fired {e:?}"),
        }
    }

    fn fired(o: Outcome) -> FiredExecution {
        match o {
            Outcome::Fired(e) => *e,
            Outcome::Refused(r) => panic!("expected a fire, refused {r:?}"),
        }
    }

    // ---------------------------------------------------------------------
    // A SECOND fixture, disjoint in every field.
    //
    // Fix round 2. Everything above this comment was built from one listener,
    // one flow and one source, so every value `evaluate` carries from an input
    // to an output had exactly one witness -- and a field whose fixture
    // supplies one value cannot distinguish "computed" from "constant".
    // Measured before the fix: replacing `trigger_execution_id: source.id`,
    // `namespace`, `flow_id`, `trigger_id`, `inputs`, `when_source`,
    // `resolve`'s `trigger_id`, and the `source.namespace`/`source.flow_id`
    // comparison operands with the literal these fixtures happen to supply left
    // **all 693 lib tests passing** -- nine survivors out of thirteen probes,
    // with four deliberate controls dying, so the sweep discriminated.
    //
    // These three share NO string, uuid, state or id with the first set.
    // ---------------------------------------------------------------------

    fn other_src_id() -> Uuid {
        Uuid::parse_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap()
    }

    /// The second listener: different id, states, `when`, inputs and labels.
    fn other_listener() -> WindowFreeFlowTrigger {
        WindowFreeFlowTrigger::resolve(&trigger(
            "id: relay\ntype: io.kestra.plugin.core.trigger.Flow\n\
             states: [WARNING]\nwhen: \"{{ alpha != beta }}\"\n\
             inputs:\n  alpha: \"A\"\n\
             labels:\n  tier: gold\n",
        ))
        .expect("second listener resolves")
    }

    fn other_listening() -> ListeningFlow {
        ListeningFlow {
            workspace_id: other_ws(),
            namespace: "z9.listen".into(),
            flow_id: "relay-flow".into(),
            disabled: false,
            draft: false,
        }
    }

    fn other_source() -> SourceExecution {
        SourceExecution {
            id: other_src_id(),
            workspace_id: other_ws(),
            namespace: "z9.probe".into(),
            flow_id: "emitter".into(),
            state: State::Warning,
            kind: None,
            execution_depth: Some(4),
        }
    }

    /// **Every field of the fired execution tracks its input, and no field is a
    /// constant that the fixtures happened to agree with.**
    ///
    /// Fix round 2, and the general form of the `max_depth` lesson: *a
    /// two-sided pair on a comparison says nothing about where its operand came
    /// from* — and neither does a single-valued fixture say anything about
    /// where a **carried** value came from. `the_fired_execution_carries_the_trigger_link_and_no_parent`
    /// was written to pin "the fact the whole package's link design rests on"
    /// and it **passed with the implementation of every field it checked
    /// replaced by a literal**, because `src_id()`, `"t6.listen"`, `"listener"`
    /// and `"listen"` were the only values any test ever supplied.
    ///
    /// Two disjoint triples, asserted **pairwise different** first — a row that
    /// silently agreed with the other would re-open the hole this closes — and
    /// then each field asserted against its own triple.
    #[test]
    fn every_carried_field_tracks_its_input_rather_than_a_fixture_constant() {
        let (l1, f1, s1) = (plain(), listening(), source(State::Success));
        let (l2, f2, s2) = (other_listener(), other_listening(), other_source());

        // The fixtures really are disjoint. Without this, a later edit that
        // made them coincide would turn the assertions below back into
        // tautologies without failing anything.
        assert_ne!(f1.namespace, f2.namespace);
        assert_ne!(f1.flow_id, f2.flow_id);
        assert_ne!(l1.trigger_id, l2.trigger_id);
        assert_ne!(s1.id, s2.id);
        assert_ne!(s1.workspace_id, s2.workspace_id);
        assert_ne!(f1.workspace_id, f2.workspace_id);
        assert_ne!(s1.namespace, s2.namespace);
        assert_ne!(s1.flow_id, s2.flow_id);
        assert_ne!(s1.state, s2.state);
        assert_ne!(s1.execution_depth, s2.execution_depth);
        assert_ne!(l1.inputs, l2.inputs);
        assert_ne!(l1.labels, l2.labels);
        assert_ne!(l1.when_source(), l2.when_source());

        let e1 = fired(run(&l1, &f1, &s1));
        let e2 = fired(run(&l2, &f2, &s2));

        // The LISTENING flow's identity, not the source's and not a constant.
        assert_eq!(
            (e1.namespace.as_str(), e1.flow_id.as_str()),
            ("t6.listen", "listener")
        );
        assert_eq!(
            (e2.namespace.as_str(), e2.flow_id.as_str()),
            ("z9.listen", "relay-flow")
        );

        // The TRIGGER's id.
        assert_eq!(e1.trigger_id, "listen");
        assert_eq!(e2.trigger_id, "relay");

        // **The link.** Two sources, two ids, each carried to its own child.
        assert_eq!(e1.trigger_execution_id, s1.id);
        assert_eq!(e2.trigger_execution_id, s2.id);
        assert_ne!(e1.trigger_execution_id, e2.trigger_execution_id);
        assert_eq!(e1.parent_execution_id(), None);
        assert_eq!(e2.parent_execution_id(), None);
        assert_eq!(e2.trigger_run_ix(), None);

        // Depth follows its own source.
        assert_eq!(e1.execution_depth, 1);
        assert_eq!(e2.execution_depth, 5);

        // Inputs and labels, including the EMPTY declaration -- the row that
        // was missing, and the one a "carry the declaring fixture's map"
        // implementation survives without.
        assert!(e1.inputs.is_empty());
        assert!(e1.labels.is_empty());
        assert_eq!(e2.inputs.get("alpha").and_then(|v| v.as_str()), Some("A"));
        assert_eq!(e2.inputs.len(), 1);
        assert_eq!(e2.labels.len(), 1);
        assert_eq!(e2.labels[0].key, "tier");
        assert_eq!(e2.labels[0].value, "gold");
    }

    /// `when_source()` returns the expression the flow declared, verbatim.
    ///
    /// Fix round 2. Round 1 only ever asserted `Some("true")` and `None`, so
    /// `resolve` could have mapped **every** declared `when` to `"true"` and no
    /// test would have noticed — measured: that mutation survived all 693. It
    /// matters because the caller renders exactly this string: collapsing it to
    /// `"true"` would evaluate a condition the author did not write, and fire
    /// where Kestra would not.
    #[test]
    fn the_when_source_is_the_declared_expression_and_not_the_default() {
        for (yaml, expected) in [
            (
                "when: \"{{ labels.source == 'a' }}\"",
                Some("{{ labels.source == 'a' }}"),
            ),
            ("when: \"false\"", Some("false")),
            ("when: \"true\"", Some("true")),
            ("when: null", None),
        ] {
            let l = WindowFreeFlowTrigger::resolve(&trigger(&format!(
                "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n{yaml}\n"
            )))
            .unwrap();
            assert_eq!(l.when_source(), expected, "{yaml}");
        }
        // The absent key takes Kestra's default, which is the one value that
        // could hide a collapse-to-"true" -- asserted last, and beside the
        // rows that make it non-vacuous.
        assert_eq!(plain().when_source(), Some("true"));
    }

    // ---------------------------------------------------------------------
    // The negatives, first, because over-acceptance is the hazard the oracle
    // cannot see.
    // ---------------------------------------------------------------------

    /// **A terminal state that must not fire it.**
    ///
    /// `SUCCESS` and `FAILED` are both terminal; a trigger declaring only
    /// `SUCCESS` must refuse `FAILED`. Measured on the artifact: `l-success`
    /// (`states: [SUCCESS]`) fired **once**, for the SUCCESS producer, and not
    /// for the FAILED one, while `l-failed` beside it did the mirror image.
    ///
    /// Two-sided in one test: the same listener, the same flow, the same
    /// source, one field different.
    #[test]
    fn a_terminal_state_that_is_not_listed_does_not_fire() {
        let l = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\nstates: [SUCCESS]\n",
        ))
        .unwrap();
        let f = listening();

        assert_eq!(
            refusal(run(&l, &f, &source(State::Failed))),
            Refusal::StateNotListed(State::Failed),
            "FAILED is terminal but is not in `states: [SUCCESS]`"
        );
        assert!(
            matches!(run(&l, &f, &source(State::Success)), Outcome::Fired(_)),
            "SUCCESS is the one state listed and must fire"
        );
    }

    /// **The `states` list is a membership test; terminality is not part of
    /// it.**
    ///
    /// This is the general sentence the obvious fixture cannot support.
    /// `SUCCESS`/`FAILED`/`WARNING`/`KILLED` are all terminal, so a fixture
    /// built only from them can satisfy "fires on terminal states" and "fires
    /// on listed states" equally. The discriminating rows are the
    /// **non-terminal** ones, and they have to go both ways or they only prove
    /// that everything fires:
    ///
    /// | `states` | source state | terminal? | rule A "terminal only" | rule B "listed" | artifact |
    /// |---|---|---|---|---|---|
    /// | `[RUNNING]` | `RUNNING` | no | refuse | fire | **fire** |
    /// | `[QUEUED]` | `QUEUED` | no | refuse | fire | fire |
    /// | `[KILLING]` | `KILLING` | no | refuse | fire | fire |
    /// | default | `RUNNING` | no | refuse | refuse | refuse |
    /// | default | `PAUSED` | no | refuse | **fire** | fire |
    ///
    /// `RUNNING` is the measured one: a listener declaring `states: [RUNNING]`
    /// fired twice on the artifact, once per source execution that passed
    /// through it. The last two rows are the pair that stops this from being a
    /// claim about the default list — `PAUSED` is non-terminal and IS in it,
    /// and the first draft of this test asserted the opposite and failed.
    #[test]
    fn a_non_terminal_state_fires_when_it_is_listed_and_not_when_it_is_not() {
        let f = listening();

        // Every non-terminal state, listed explicitly, fires.
        for state in [
            State::Submitted,
            State::Running,
            State::Paused,
            State::Restarted,
            State::Killing,
            State::Queued,
            State::Retrying,
            State::Breakpoint,
        ] {
            assert!(
                !state.is_terminated(),
                "{state:?} was chosen for this table because it is NOT terminal"
            );
            let wire = format!("{state:?}").to_uppercase();
            let l = WindowFreeFlowTrigger::resolve(&trigger(&format!(
                "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\nstates: [{wire}]\n"
            )))
            .unwrap();
            assert!(
                matches!(run(&l, &f, &source(state)), Outcome::Fired(_)),
                "{state:?} is not terminal and IS listed, so it must fire"
            );
        }

        // Against the DEFAULT list, the same non-terminal states split:
        // `PAUSED` is in it and the rest are not.
        assert!(matches!(
            run(&plain(), &f, &source(State::Paused)),
            Outcome::Fired(_)
        ));
        for state in [
            State::Submitted,
            State::Running,
            State::Restarted,
            State::Killing,
            State::Queued,
            State::Retrying,
            State::Breakpoint,
        ] {
            assert_eq!(
                refusal(run(&plain(), &f, &source(state))),
                Refusal::StateNotListed(state),
                "{state:?} is not in `terminatedTypes() + [PAUSED]`"
            );
        }
    }

    /// The default `states` list is `terminatedTypes() + [PAUSED]`, whole.
    ///
    /// Enumerated rather than sampled: every one of the 17 `State` variants is
    /// asserted in one direction or the other, so a variant added to
    /// `State::is_terminated` cannot quietly change what a default flow trigger
    /// accepts without failing here.
    #[test]
    fn the_default_state_list_is_every_terminal_state_plus_paused() {
        let l = plain();
        let f = listening();
        let all = [
            State::Created,
            State::Submitted,
            State::Running,
            State::Paused,
            State::Restarted,
            State::Killing,
            State::Success,
            State::Warning,
            State::Failed,
            State::Killed,
            State::Cancelled,
            State::Queued,
            State::Retrying,
            State::Retried,
            State::Skipped,
            State::Breakpoint,
            State::Resubmitted,
        ];
        let mut accepted = Vec::new();
        for state in all {
            // CREATED is refused by an earlier rule, so it cannot report on
            // the state list at all -- see `Refusal::EntryState`.
            if state == State::Created {
                assert_eq!(refusal(run(&l, &f, &source(state))), Refusal::EntryState);
                continue;
            }
            match run(&l, &f, &source(state)) {
                Outcome::Fired(_) => accepted.push(state),
                Outcome::Refused(Refusal::StateNotListed(s)) => assert_eq!(s, state),
                other => panic!("{state:?}: unexpected {other:?}"),
            }
        }
        assert_eq!(
            accepted,
            vec![
                State::Paused,
                State::Success,
                State::Warning,
                State::Failed,
                State::Killed,
                State::Cancelled,
                State::Retried,
                State::Skipped,
                State::Resubmitted,
            ],
            "the default list is terminatedTypes() + PAUSED"
        );
        assert_eq!(
            accepted.len(),
            all.iter().filter(|s| s.is_terminated()).count() + 1,
            "one more than the terminal states: PAUSED"
        );
    }

    /// **A condition that must not match.**
    ///
    /// All four of `TruthUtils.FALSE_VALUES`, the trimmed form, and two members
    /// of the complement. Enumerating the falsy set matters because it is
    /// closed: any string outside it is truthy, including ones a reader would
    /// call false.
    #[test]
    fn the_when_condition_refuses_exactly_kestras_four_falsy_strings() {
        let l = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\nstates: [SUCCESS]\nwhen: \"{{ x }}\"\n",
        ))
        .unwrap();
        let f = listening();
        let s = source(State::Success);

        for falsy in ["false", "0", "-0", "", "  false  ", "\tfalse\n", " 0 "] {
            assert_eq!(
                refusal(evaluate(
                    &l,
                    &f,
                    &s,
                    &WhenVerdict::Rendered(falsy.to_string()),
                    DEFAULT_MAX_EXECUTION_DEPTH
                )),
                Refusal::WhenFalsy(falsy.to_string()),
                "{falsy:?} is falsy on the artifact"
            );
        }
        // The complement, including the two a reader is most likely to expect
        // to be falsy and which are NOT.
        for truthy in ["true", "no", "null", "False", "0.0", "-1", " x "] {
            assert!(
                matches!(
                    evaluate(
                        &l,
                        &f,
                        &s,
                        &WhenVerdict::Rendered(truthy.to_string()),
                        DEFAULT_MAX_EXECUTION_DEPTH
                    ),
                    Outcome::Fired(_)
                ),
                "{truthy:?} is not in FALSE_VALUES, so it fires"
            );
        }
    }

    /// An unrenderable `when` refuses, and it is not the same thing as a falsy
    /// one.
    ///
    /// Measured: `when: 'x{{ missingVar.someField }}'` produced **zero**
    /// executions. The expression was chosen so that the two hypotheses give
    /// different answers — if a missing variable rendered to the empty string
    /// the result would be `"x"`, which is **truthy**, and the trigger would
    /// have fired. It did not.
    #[test]
    fn a_when_the_caller_could_not_render_refuses_rather_than_firing() {
        let l = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\nstates: [SUCCESS]\nwhen: \"x{{ missing.field }}\"\n",
        ))
        .unwrap();
        let f = listening();
        let s = source(State::Success);

        assert_eq!(
            refusal(evaluate(
                &l,
                &f,
                &s,
                &WhenVerdict::Unrenderable,
                DEFAULT_MAX_EXECUTION_DEPTH
            )),
            Refusal::WhenUnrenderable
        );
        // The other side: had it rendered to what it would render to with the
        // variable merely empty, it would have fired.
        assert!(matches!(
            evaluate(
                &l,
                &f,
                &s,
                &WhenVerdict::Rendered("x".into()),
                DEFAULT_MAX_EXECUTION_DEPTH
            ),
            Outcome::Fired(_)
        ));
    }

    /// An absent `when` fires; a declared-but-unrendered one refuses.
    ///
    /// The first half is the single input at which `isTruthy` and `!isFalsy`
    /// disagree — Java's `null` — and the artifact's answer is **fire**
    /// (`l-w-absent` produced one execution). The second half is egret's own
    /// refusal of a caller that skipped the render.
    #[test]
    fn an_absent_when_fires_and_an_unrendered_declared_one_does_not() {
        let f = listening();
        let s = source(State::Success);

        // `when: null` -> `models::Trigger` parses to `None`.
        let absent = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\nstates: [SUCCESS]\nwhen: null\n",
        ))
        .unwrap();
        assert_eq!(absent.when_source(), None);
        assert!(matches!(
            evaluate(
                &absent,
                &f,
                &s,
                &WhenVerdict::NotDeclared,
                DEFAULT_MAX_EXECUTION_DEPTH
            ),
            Outcome::Fired(_)
        ));

        // A trigger with NO `when:` key at all takes Kestra's default of
        // `"true"`, which is a declared condition as far as this seam is
        // concerned -- so `NotDeclared` is a caller bug.
        let defaulted = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\nstates: [SUCCESS]\n",
        ))
        .unwrap();
        assert_eq!(defaulted.when_source(), Some("true"));
        assert_eq!(
            refusal(evaluate(
                &defaulted,
                &f,
                &s,
                &WhenVerdict::NotDeclared,
                DEFAULT_MAX_EXECUTION_DEPTH
            )),
            Refusal::WhenNotRendered
        );
        assert!(matches!(
            evaluate(
                &defaulted,
                &f,
                &s,
                &WhenVerdict::Rendered("true".into()),
                DEFAULT_MAX_EXECUTION_DEPTH
            ),
            Outcome::Fired(_)
        ));
    }

    /// A flow trigger never fires on its own flow, and "its own flow" is the
    /// **pair** `(namespace, flowId)`.
    ///
    /// Three discriminating rows, because a fixture where the listener and the
    /// source differ in both halves proves nothing about either:
    ///
    /// | row | namespace | flowId | measured on the artifact |
    /// |---|---|---|---|
    /// | same pair | same | same | `r2.c/selftwin` refused **two** of its own executions |
    /// | same namespace only | same | different | `r2.a/other` fired 3× off `r2.a/twin` |
    /// | same flowId only | different | same | `r2.b/twin` fired 3× off `r2.a/twin` |
    #[test]
    fn a_flow_trigger_does_not_fire_on_its_own_flows_execution() {
        // Fix round 2: run the three rows against TWO disjoint sources. With
        // one source, `source.namespace` and `source.flow_id` could each be
        // replaced by the fixture's literal and every row still passed --
        // measured, both mutations survived all 693 lib tests. The comparison
        // was two-sided; its operands were not.
        for (l, s) in [
            (plain(), source(State::Success)),
            (other_listener(), other_source()),
        ] {
            let ctx = format!("{}/{}", s.namespace, s.flow_id);

            let same_pair = ListeningFlow {
                workspace_id: s.workspace_id,
                namespace: s.namespace.clone(),
                flow_id: s.flow_id.clone(),
                disabled: false,
                draft: false,
            };
            assert_eq!(
                refusal(run(&l, &same_pair, &s)),
                Refusal::SameFlow,
                "{ctx}: a flow trigger must not fire on its own flow"
            );

            let same_namespace = ListeningFlow {
                namespace: s.namespace.clone(),
                flow_id: "a-different-flow".into(),
                ..same_pair.clone()
            };
            assert!(
                matches!(run(&l, &same_namespace, &s), Outcome::Fired(_)),
                "{ctx}: same namespace, different id: Kestra compares the pair"
            );

            let same_flow_id = ListeningFlow {
                namespace: "a.different.namespace".into(),
                flow_id: s.flow_id.clone(),
                ..same_pair.clone()
            };
            assert!(
                matches!(run(&l, &same_flow_id, &s), Outcome::Fired(_)),
                "{ctx}: same id, different namespace: Kestra compares the pair"
            );
        }
    }

    /// A disabled trigger and a disabled flow each refuse, and each has its own
    /// live twin.
    #[test]
    fn disabled_refuses_on_both_the_trigger_and_the_flow() {
        let s = source(State::Success);

        let off = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\ndisabled: true\n",
        ))
        .unwrap();
        assert_eq!(
            refusal(run(&off, &listening(), &s)),
            Refusal::TriggerDisabled
        );
        assert!(matches!(run(&plain(), &listening(), &s), Outcome::Fired(_)));

        let flow_off = ListeningFlow {
            disabled: true,
            ..listening()
        };
        assert_eq!(refusal(run(&plain(), &flow_off, &s)), Refusal::FlowDisabled);

        let draft = ListeningFlow {
            draft: true,
            ..listening()
        };
        assert_eq!(refusal(run(&plain(), &draft, &s)), Refusal::FlowDraft);
    }

    /// Only a `NORMAL` execution fires a flow trigger, and `None` is `NORMAL`.
    #[test]
    fn only_a_normal_execution_kind_fires_a_flow_trigger() {
        let l = plain();
        let f = listening();

        for kind in [None, Some(ExecutionKind::Normal)] {
            let s = SourceExecution {
                kind,
                ..source(State::Success)
            };
            assert!(
                matches!(run(&l, &f, &s), Outcome::Fired(_)),
                "{kind:?} is normal"
            );
        }
        for kind in [
            ExecutionKind::Test,
            ExecutionKind::Playground,
            ExecutionKind::Loop,
        ] {
            let s = SourceExecution {
                kind: Some(kind),
                ..source(State::Success)
            };
            assert_eq!(refusal(run(&l, &f, &s)), Refusal::NotNormalKind(kind));
        }
    }

    /// The depth guard is `newDepth > maxDepth`, so `maxDepth` itself runs.
    ///
    /// Both sides of the boundary, and both sides measured on the artifact: a
    /// 105-flow trigger chain produced `d100` with `executionDepth: 100` and
    /// never created `d101`.
    #[test]
    fn the_depth_guard_admits_the_boundary_and_refuses_the_step_past_it() {
        let l = plain();
        let f = listening();

        let at = SourceExecution {
            execution_depth: Some(DEFAULT_MAX_EXECUTION_DEPTH - 1),
            ..source(State::Success)
        };
        assert_eq!(
            fired(run(&l, &f, &at)).execution_depth,
            DEFAULT_MAX_EXECUTION_DEPTH,
            "a source at 99 produces the execution at depth 100"
        );

        let past = SourceExecution {
            execution_depth: Some(DEFAULT_MAX_EXECUTION_DEPTH),
            ..source(State::Success)
        };
        assert_eq!(
            refusal(run(&l, &f, &past)),
            Refusal::DepthExceeded {
                depth: DEFAULT_MAX_EXECUTION_DEPTH + 1,
                max: DEFAULT_MAX_EXECUTION_DEPTH,
            }
        );

        // A top-level source has no depth at all and reads as zero.
        assert_eq!(
            fired(run(&l, &f, &source(State::Success))).execution_depth,
            1
        );
    }

    /// `CREATED` is never offered to the filter, and this seam refuses it
    /// rather than trusting the caller.
    #[test]
    fn the_entry_state_is_refused_even_when_it_is_listed() {
        let l = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\nstates: [CREATED, SUCCESS]\n",
        ))
        .unwrap();
        let f = listening();
        assert!(
            l.states().contains(&State::Created),
            "the fixture must actually list CREATED, or this proves nothing"
        );
        assert_eq!(
            refusal(run(&l, &f, &source(State::Created))),
            Refusal::EntryState
        );
        // The same listener on the other listed state fires, so the refusal is
        // about CREATED and not about the listener.
        assert!(matches!(
            run(&l, &f, &source(State::Success)),
            Outcome::Fired(_)
        ));
    }

    // ---------------------------------------------------------------------
    // Resolution
    // ---------------------------------------------------------------------

    /// `dependsOn` is refused, one condition is enough, and an empty list is
    /// not a `dependsOn`.
    #[test]
    fn depends_on_is_refused_and_an_absent_or_empty_one_is_not() {
        let one = trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n\
             dependsOn:\n  - namespace: t6.probe\n",
        );
        assert_eq!(
            WindowFreeFlowTrigger::resolve(&one),
            Err(FlowTriggerError::UnsupportedDependsOn(1))
        );

        let two = trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n\
             dependsOn:\n  - namespace: t6.probe\n  - flowId: other\n",
        );
        assert_eq!(
            WindowFreeFlowTrigger::resolve(&two),
            Err(FlowTriggerError::UnsupportedDependsOn(2))
        );

        let empty =
            trigger("id: listen\ntype: io.kestra.plugin.core.trigger.Flow\ndependsOn: []\n");
        assert!(
            WindowFreeFlowTrigger::resolve(&empty).is_ok(),
            "`ListUtils.isEmpty` is what Kestra branches on, not `!= null`"
        );
        assert!(WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n"
        ))
        .is_ok());
    }

    /// The twenty corpus declarations that carry a `dependsOn` are refused BY
    /// THIS ARM (they take the `dependsOn` arm instead: see
    /// `depends_on::tests::the_corpus_depends_on_declarations_resolve_eighteen_of_twenty`),
    /// and the one that does not is resolved. Since W13b Task 2 nineteen are
    /// refused at [`WindowFreeFlowTrigger::resolve`] and one -- the `fireOnce`
    /// flow -- at parse, because `models::Window` refuses a field the
    /// artifact's `Window` does not have.
    ///
    /// This is the count the module doc states, compared by a running program
    /// rather than asserted in prose — and the prose said **nineteen** until
    /// this test ran, because the hand count was taken off a truncated dump.
    #[test]
    fn the_corpus_flow_triggers_split_twenty_to_one() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle-exec/corpus");
        assert!(dir.is_dir(), "corpus not found at {}", dir.display());

        let mut files = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("read corpus dir") {
                let p = e.expect("dir entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "yaml" || x == "yml") {
                    files.push(p);
                }
            }
        }
        files.sort();
        assert!(
            files.len() > 300,
            "expected the 351-flow corpus, found {}",
            files.len()
        );

        let mut declaring_flows = 0usize;
        let mut resolved = Vec::new();
        let mut refused = Vec::new();
        // W13b Task 2: a flow that no longer PARSES is not skipped silently --
        // that is how this count would have dropped to sixteen without anyone
        // being told. `models::Window` refuses unknown fields, so
        // `fireOnce` (which the artifact refuses with 422 too) is a parse
        // error now; it is counted here, by name and by reason.
        let mut unparseable_declaring = Vec::new();
        for path in &files {
            let text = std::fs::read_to_string(path).expect("read flow");
            let flow = match serde_norway::from_str::<crate::models::Flow>(&text) {
                Ok(flow) => flow,
                Err(e) => {
                    if text.contains("io.kestra.plugin.core.trigger.Flow") {
                        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                        unparseable_declaring.push((name, e.to_string()));
                    }
                    continue;
                }
            };
            let mut declares = false;
            for t in &flow.triggers {
                if t.r#type != "io.kestra.plugin.core.trigger.Flow" {
                    continue;
                }
                declares = true;
                let name = format!("{}/{}", flow.id, t.id);
                match WindowFreeFlowTrigger::resolve(t) {
                    Ok(_) => resolved.push(name),
                    Err(FlowTriggerError::UnsupportedDependsOn(_)) => refused.push(name),
                    Err(e) => panic!("{name}: unexpected {e}"),
                }
            }
            if declares {
                declaring_flows += 1;
            }
        }

        assert_eq!(
            unparseable_declaring.len(),
            1,
            "only the fireOnce flow fails to parse: {unparseable_declaring:?}"
        );
        let (name, why) = &unparseable_declaring[0];
        assert_eq!(name, "flow-trigger-fire-once-true-flow-listen");
        assert!(why.contains("unknown field `fireOnce`"), "{why}");

        assert_eq!(
            declaring_flows + unparseable_declaring.len(),
            17,
            "corpus flows declaring a flow trigger"
        );
        assert_eq!(
            resolved,
            vec!["flow-trigger-when-condition-flow-listen/on_completion"],
            "the one declaration this module can evaluate"
        );
        // Its one declaration carries a `dependsOn` too, so it is the
        // twentieth: refused at parse now rather than at `resolve`.
        assert_eq!(refused.len(), 19, "declarations refused for `dependsOn`");
        assert_eq!(
            resolved.len() + refused.len() + unparseable_declaring.len(),
            21
        );
    }

    /// W13b Task 2 fix round 1: a `window` with no `dependsOn` is refused
    /// by name, whatever the window says -- including a value the save-time
    /// validator would also refuse (`P1M`), because `resolve` is what stands
    /// between a draft that saved with validation errors and a fire.
    #[test]
    fn a_window_without_depends_on_is_refused_not_ignored() {
        for window in [
            "window:\n  every: PT1H\n",
            "window:\n  every: P1M\n",
            "window: {}\n",
        ] {
            let t = trigger(&format!(
                "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n{window}"
            ));
            match WindowFreeFlowTrigger::resolve(&t) {
                Err(FlowTriggerError::Refused(codes)) => assert!(
                    codes.contains(&"window_without_depends_on"),
                    "{window}: {codes:?}"
                ),
                other => panic!("{window}: {other:?}"),
            }
        }
        let empty = trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\ndependsOn: []\n\
             window:\n  every: PT1H\n",
        );
        assert_eq!(
            WindowFreeFlowTrigger::resolve(&empty),
            Err(FlowTriggerError::Refused(vec!["window_without_depends_on"])),
            "`dependsOn: []` is the window-free arm too"
        );
        // Control: the same trigger without the window resolves.
        assert!(WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n"
        ))
        .is_ok());
    }

    /// Final-review fix wave, I1: a `mode`/`minSatisfied` shape save refuses
    /// is refused on the window-free arm too, by the same code, so a draft
    /// carrying one gets no listener row and never fires. Before the fix
    /// `resolve` read only `dependsOn` and `window`, and all three resolved.
    #[test]
    fn a_mode_shape_save_refuses_is_refused_without_depends_on() {
        for (body, code) in [
            ("mode: AT_LEAST\n", "min_satisfied_required"),
            (
                "mode: AT_LEAST\nminSatisfied: 1\n",
                "min_satisfied_exceeds_conditions",
            ),
            ("minSatisfied: 0\n", "min_satisfied_not_positive"),
        ] {
            let t = trigger(&format!(
                "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n{body}"
            ));
            assert_eq!(
                WindowFreeFlowTrigger::resolve(&t),
                Err(FlowTriggerError::Refused(vec![code])),
                "{body}"
            );
        }
        // Controls: the modes save accepts without `dependsOn` still resolve.
        for body in ["mode: ANY\n", "mode: ALL\n", "minSatisfied: 1\n"] {
            assert!(
                WindowFreeFlowTrigger::resolve(&trigger(&format!(
                    "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n{body}"
                )))
                .is_ok(),
                "{body}"
            );
        }
    }

    /// A `core.schedule` is not a flow trigger and does not resolve as one.
    #[test]
    fn a_schedule_is_not_a_flow_trigger() {
        assert_eq!(
            WindowFreeFlowTrigger::resolve(&trigger(
                "id: t\ntype: io.kestra.plugin.core.trigger.Schedule\ncron: \"0 2 * * *\"\n"
            )),
            Err(FlowTriggerError::NotAFlowTrigger)
        );
        // ... and the inverse, so this is not measuring the fixture loader.
        assert!(WindowFreeFlowTrigger::resolve(&trigger(
            "id: t\ntype: io.kestra.plugin.core.trigger.Flow\n"
        ))
        .is_ok());
    }

    // ---------------------------------------------------------------------
    // The link
    // ---------------------------------------------------------------------

    /// **The fired execution is linked by `trigger_execution_id` and by
    /// nothing else.**
    ///
    /// Measured across **210** trigger-fired executions on the artifact, in
    /// four independent rounds (the table in this module's doc): `parentId` was
    /// `null` on every one and `trigger.variables.executionId` named the
    /// immediate source. Migration 0138's
    /// `workflow_executions_one_link_kind` is what makes the wrong combination
    /// unrepresentable in the database; this is what makes it unrepresentable
    /// in the value a writer binds from.
    #[test]
    fn the_fired_execution_carries_the_trigger_link_and_no_parent() {
        let e = fired(run(&plain(), &listening(), &source(State::Success)));

        assert_eq!(e.trigger_execution_id, src_id());
        assert_eq!(e.parent_execution_id(), None);
        assert_eq!(e.trigger_run_ix(), None);
        // The execution that runs is the LISTENER's, not the source's.
        assert_eq!(e.namespace, "t6.listen");
        assert_eq!(e.flow_id, "listener");
        assert_eq!(e.trigger_id, "listen");
    }

    /// `Flow.inputs` are carried unrendered, exactly as declared.
    #[test]
    fn the_triggers_inputs_are_carried_unrendered() {
        let l = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n\
             inputs:\n  from-parent: \"{{ outputs.only.value }}\"\n  n: 3\n",
        ))
        .unwrap();
        let e = fired(run(&l, &listening(), &source(State::Success)));
        assert_eq!(e.inputs.len(), 2);
        assert_eq!(
            e.inputs.get("from-parent").and_then(|v| v.as_str()),
            Some("{{ outputs.only.value }}"),
            "unrendered -- the render surface is the caller's"
        );
        assert_eq!(e.inputs.get("n").and_then(|v| v.as_i64()), Some(3));
    }

    /// The refusal reported is the FIRST reason in the artifact's order, not an
    /// arbitrary one.
    ///
    /// A ladder, not a list: every refusal is violated at once, then **exactly
    /// one field changes per step**, and each step must report the next reason
    /// down. That is what pins the *order* — a step that changed two fields
    /// could be explained by either, and fix round 1 found two such steps in
    /// the first draft (`TriggerDisabled -> StateNotListed` changed the
    /// listener *and* the state; `DepthExceeded -> TriggerDisabled` swapped the
    /// whole flow and source at once). Every adjacency below is now
    /// single-field, and at each rung every later check is still violated, so
    /// the answer discriminates against all of them and not merely against its
    /// neighbour.
    #[test]
    fn the_refusals_are_reported_in_the_artifacts_order() {
        let disabled_listener = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n\
             states: [SUCCESS]\ndisabled: true\nwhen: \"{{ x }}\"\n",
        ))
        .unwrap();
        let enabled_listener = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n\
             states: [SUCCESS]\nwhen: \"{{ x }}\"\n",
        ))
        .unwrap();

        // Everything violated at once.
        let mut listener = &disabled_listener;
        let mut flow = ListeningFlow {
            workspace_id: other_ws(),
            namespace: "t6.probe".into(),
            flow_id: "producer".into(),
            disabled: true,
            draft: true,
        };
        let mut src = SourceExecution {
            id: src_id(),
            workspace_id: ws(),
            namespace: "t6.probe".into(),
            flow_id: "producer".into(),
            state: State::Created,
            kind: Some(ExecutionKind::Test),
            execution_depth: Some(DEFAULT_MAX_EXECUTION_DEPTH),
        };
        let mut when = WhenVerdict::Rendered("0".into());

        macro_rules! step {
            ($expected:expr) => {
                assert_eq!(
                    refusal(evaluate(
                        listener,
                        &flow,
                        &src,
                        &when,
                        DEFAULT_MAX_EXECUTION_DEPTH
                    )),
                    $expected,
                    "flow={:?} source={:?}",
                    flow,
                    src
                );
            };
        }

        step!(Refusal::DifferentWorkspace);
        flow.workspace_id = ws();
        step!(Refusal::EntryState);
        // WARNING, not SUCCESS: the state stays unlisted so `StateNotListed`
        // is still pending and every rung below still discriminates.
        src.state = State::Warning;
        step!(Refusal::SameFlow);
        flow.flow_id = "listener".into();
        step!(Refusal::NotNormalKind(ExecutionKind::Test));
        src.kind = None;
        step!(Refusal::FlowDisabled);
        flow.disabled = false;
        step!(Refusal::FlowDraft);
        flow.draft = false;
        step!(Refusal::DepthExceeded {
            depth: DEFAULT_MAX_EXECUTION_DEPTH + 1,
            max: DEFAULT_MAX_EXECUTION_DEPTH,
        });
        src.execution_depth = None;
        step!(Refusal::TriggerDisabled);
        listener = &enabled_listener;
        step!(Refusal::StateNotListed(State::Warning));
        src.state = State::Success;
        step!(Refusal::WhenFalsy("0".into()));
        when = WhenVerdict::Rendered("true".into());

        // ... and with the last one lifted it fires, so the ladder was a ladder
        // and not a wall.
        assert!(matches!(
            evaluate(listener, &flow, &src, &when, DEFAULT_MAX_EXECUTION_DEPTH),
            Outcome::Fired(_)
        ));
    }

    /// **`max_depth` is the argument, and nothing before fix round 1 said so.**
    ///
    /// Every other depth assertion in this file passes
    /// [`DEFAULT_MAX_EXECUTION_DEPTH`], so an implementation that ignored the
    /// parameter and compared against the literal `100` was indistinguishable
    /// from this one — the reviewer's mutation `depth > max_depth` →
    /// `depth > 100` **survived all seventeen tests**. The lesson generalises
    /// past the fix: *a two-sided pair on a comparison says nothing about where
    /// its operand came from.* The boundary was pinned; the boundary's **input**
    /// was not.
    ///
    /// So this drives the boundary with values the default can never produce,
    /// two-sided at each, and then asserts the verdict **moves** when only
    /// `max_depth` changes.
    #[test]
    fn the_depth_boundary_follows_the_argument_and_not_a_constant() {
        let l = plain();
        let f = listening();
        let when = WhenVerdict::Rendered("true".into());

        for max in [1u32, 3, 7, DEFAULT_MAX_EXECUTION_DEPTH + 41] {
            assert_ne!(
                max, DEFAULT_MAX_EXECUTION_DEPTH,
                "a row equal to the default cannot tell the two apart"
            );

            // The rung at `max` runs ...
            let at = SourceExecution {
                execution_depth: Some(max - 1),
                ..source(State::Success)
            };
            assert_eq!(
                fired(evaluate(&l, &f, &at, &when, max)).execution_depth,
                max,
                "max_depth={max}: the boundary itself must fire"
            );

            // ... and the next one does not.
            let past = SourceExecution {
                execution_depth: Some(max),
                ..source(State::Success)
            };
            assert_eq!(
                refusal(evaluate(&l, &f, &past, &when, max)),
                Refusal::DepthExceeded {
                    depth: max + 1,
                    max
                },
                "max_depth={max}: one past the boundary must refuse"
            );

            // The SAME source judged under the default gives the OTHER answer
            // whenever the two bounds disagree. This is the assertion the old
            // tests could not make, and the one the surviving mutation dies on.
            let under_default = evaluate(&l, &f, &past, &when, DEFAULT_MAX_EXECUTION_DEPTH);
            if max < DEFAULT_MAX_EXECUTION_DEPTH {
                assert!(
                    matches!(under_default, Outcome::Fired(_)),
                    "max_depth={max} refused a depth the default admits"
                );
            } else {
                assert_eq!(
                    refusal(under_default),
                    Refusal::DepthExceeded {
                        depth: max + 1,
                        max: DEFAULT_MAX_EXECUTION_DEPTH,
                    },
                    "max_depth={max} admitted a depth the default refuses"
                );
            }
        }
    }

    /// The listening flow and the source must share a workspace.
    ///
    /// **egret's own refusal, not Kestra's** — see
    /// [`Refusal::DifferentWorkspace`]. Three rows, because "they differ" has
    /// two ways to be produced and a fixture that only moves the listener would
    /// not tell them apart from "the listener is unusual".
    #[test]
    fn a_source_in_another_workspace_does_not_fire_a_flow_trigger() {
        let l = plain();
        let s = source(State::Success);

        assert!(matches!(run(&l, &listening(), &s), Outcome::Fired(_)));

        let elsewhere = ListeningFlow {
            workspace_id: other_ws(),
            ..listening()
        };
        assert_ne!(elsewhere.workspace_id, s.workspace_id);
        assert_eq!(
            refusal(run(&l, &elsewhere, &s)),
            Refusal::DifferentWorkspace
        );

        // Move the SOURCE instead: same refusal.
        let source_elsewhere = SourceExecution {
            workspace_id: other_ws(),
            ..source(State::Success)
        };
        assert_eq!(
            refusal(run(&l, &listening(), &source_elsewhere)),
            Refusal::DifferentWorkspace
        );
        // Move BOTH: not a mismatch, so this is about the PAIR and not about
        // either value being special.
        assert!(matches!(
            run(&l, &elsewhere, &source_elsewhere),
            Outcome::Fired(_)
        ));
    }

    /// The trigger's declared `labels:` are carried unrendered.
    ///
    /// **Fix round 1.** `models::Trigger` models `labels`, and the first draft
    /// of `resolve` dropped them on the floor — a flow declaring them parsed,
    /// resolved and fired without them, and nothing said so. That is the
    /// `lateMaximumDelay` failure under a new name.
    /// `LabelService.fromTrigger` iterates `trigger.getLabels()` minus the
    /// `system.` ones, renders each value, and attaches the survivors to the
    /// execution Kestra creates, so dropping them is a behaviour difference.
    #[test]
    fn the_triggers_declared_labels_are_carried_unrendered() {
        let l = WindowFreeFlowTrigger::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n\
             labels:\n  team: data\n  from: \"{{ trigger.flowId }}\"\n",
        ))
        .unwrap();
        let e = fired(run(&l, &listening(), &source(State::Success)));

        let mut got: Vec<(&str, &str)> = e
            .labels
            .iter()
            .map(|x| (x.key.as_str(), x.value.as_str()))
            .collect();
        got.sort_unstable();
        assert_eq!(
            got,
            vec![("from", "{{ trigger.flowId }}"), ("team", "data")],
            "unrendered, and every declared label present"
        );

        // The other side: a trigger with no `labels:` carries none, so the
        // assertion above is about the declaration and not about a default.
        assert!(fired(run(&plain(), &listening(), &source(State::Success)))
            .labels
            .is_empty());
    }

    /// `system.correlationId` is Kestra's, and egret does not have it.
    ///
    /// Asserted rather than described, so the gap becomes a failing test the
    /// day someone adds the column. See [`FiredExecution::correlation_id`].
    #[test]
    fn the_correlation_id_kestra_attaches_is_not_modelled_here() {
        let e = fired(run(&plain(), &listening(), &source(State::Success)));
        assert_eq!(e.correlation_id(), None);
        // And it is NOT a spelling of the link: on the artifact the two
        // coincide only when the source is top-level.
        assert_eq!(e.trigger_execution_id, src_id());
    }
}
