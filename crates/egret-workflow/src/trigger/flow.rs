// kestra: core/src/main/java/io/kestra/plugin/core/trigger/Flow.java
// kestra: executor/src/main/java/io/kestra/executor/FlowTriggerService.java
// kestra: executor/src/main/java/io/kestra/executor/DefaultExecutor.java
// kestra: core/src/main/java/io/kestra/core/services/ConditionService.java
// kestra: core/src/main/java/io/kestra/core/services/FlowService.java
// kestra: core/src/main/java/io/kestra/core/utils/TruthUtils.java

//! The **flow trigger** — `io.kestra.plugin.core.trigger.Flow`: a flow that
//! starts because *another* execution reached a state.
//!
//! # The ratchet did not move, and could not have
//!
//! Seventeen of the 345 corpus flows declare a flow trigger and they carry
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
//! comment: across **three independent probe rounds** — 14 trigger-fired
//! executions in round 1, 189 in the depth chain, 8 in round 2, **211 in
//! total** — every single one came back with `parentId` absent/`null` and with
//! `trigger.variables.executionId` naming the execution that caused it. One
//! example verbatim, `t6.listen/l-success` fired by `t6.probe/prod-ok`:
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
//! [`evaluate`] is that list, in that order, with one check moved to the front
//! (see [`Refusal::EntryState`]) and two that cannot arise here dropped:
//! `FlowWithException` has no egret equivalent, and a trigger reaching
//! [`evaluate`] is by construction one the flow declares.
//!
//! # Over-acceptance is the hazard, so here is what is REFUSED
//!
//! The oracle cannot see a trigger that fires where Kestra would not. Three
//! things are therefore refused rather than guessed:
//!
//! 1. **`dependsOn` is refused at [`FlowListener::resolve`]**
//!    ([`FlowTriggerError::UnsupportedDependsOn`]). Kestra routes a trigger
//!    with a non-empty `dependsOn` down a *different* method
//!    (`computeExecutionsFromFlowTriggerDependsOn`) which evaluates a
//!    **`MultipleConditionWindow`** — persistent state, counted by
//!    `MultipleCondition.test` under `mode` ALL/ANY/AT_LEAST, and made
//!    fire-at-most-once by `window.fireOnce`. `valids/flow-trigger-fire-once-true-flow-listen`
//!    declares exactly that. Firing once per matching execution, as a
//!    window-free implementation would, is over-acceptance in the direction
//!    nothing here can catch. **Twenty of the twenty-one corpus declarations
//!    carry a `dependsOn`**, so this refusal is most of the corpus — and
//!    refusing most of an unreachable corpus is the cheap half of the trade.
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
//! # Measured, derived, or chosen
//!
//! Every general sentence in this file is one of the three, and the chosen ones
//! say so. **The chosen decisions:**
//!
//! 1. The name [`FlowListener`] — `models::FlowTrigger` already holds the wire
//!    name, and two `FlowTrigger`s in one crate is a footgun.
//! 2. [`evaluate`] being pure over `(listener, flow, source, when, max_depth)`,
//!    mirroring [`super::tick`] and `executor::advance()`. Kestra's shape is a
//!    service with six injected beans.
//! 3. The [`Refusal`] enum existing at all, and every refusal being **named**
//!    rather than folded into a `bool`. Kestra returns an empty list and, for
//!    the depth case only, logs.
//! 4. Refusing `dependsOn` at resolve time instead of modelling the window
//!    (item 1 above). What the window would have to do is measured and written
//!    down in this file's `dependsOn` section below; none of it is implemented.
//! 5. [`Refusal::EntryState`] — a defensive refusal of `CREATED` that Kestra
//!    achieves upstream, by starting its transition loop at index 1.
//! 6. [`FiredExecution`] carrying the source execution's *identity* rather than
//!    the rendered `trigger.*` variable map. `Flow.Output` has eleven fields and
//!    this package has not measured how Jackson formats all of them; the same
//!    call [`super::ScheduledExecution`] made.
//! 7. `max_depth` being a parameter with [`DEFAULT_MAX_EXECUTION_DEPTH`] beside
//!    it rather than a hard-coded constant.
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
use crate::models::{ExecutionKind, State, Trigger, TriggerKind};

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

/// Why a `Trigger` could not be resolved into a [`FlowListener`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowTriggerError {
    /// Not an `io.kestra.plugin.core.trigger.Flow` at all.
    NotAFlowTrigger,
    /// The trigger declares `dependsOn`, whose evaluation needs a
    /// `MultipleConditionWindow` this package does not model. See the module
    /// doc, *Over-acceptance is the hazard*, item 1.
    UnsupportedDependsOn(usize),
}

impl fmt::Display for FlowTriggerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAFlowTrigger => write!(f, "trigger is not a core.trigger.Flow"),
            Self::UnsupportedDependsOn(n) => write!(
                f,
                "core.trigger.Flow `dependsOn` is not supported ({n} \
                 condition(s)): it needs a multiple-condition window"
            ),
        }
    }
}

impl std::error::Error for FlowTriggerError {}

/// A `io.kestra.plugin.core.trigger.Flow` resolved far enough to be evaluated
/// against another execution.
///
/// **Named `FlowListener` rather than `FlowTrigger`, and that is chosen:**
/// [`crate::models::FlowTrigger`] already carries the wire identity, and one
/// crate holding two types called `FlowTrigger` is a mistake waiting for its
/// first `use`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowListener {
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
}

impl FlowListener {
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

        Ok(Self {
            trigger_id: trigger.id.clone(),
            states: spec.states.clone(),
            when_source: trigger.when.as_ref().map(|w| w.source().to_string()),
            disabled: trigger.disabled,
            inputs: spec
                .inputs
                .as_ref()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
        })
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

/// What the caller did with [`FlowListener::when_source`].
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Fired(Box<FiredExecution>),
    Refused(Refusal),
}

/// The execution a fired flow trigger asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiredExecution {
    /// The LISTENING flow — the one that runs.
    pub namespace: String,
    pub flow_id: String,
    pub trigger_id: String,
    /// **`workflow.executions.trigger_execution_id` (migration 0138), and the
    /// only link this execution has.** Kestra's
    /// `Execution.trigger.variables.executionId`.
    pub trigger_execution_id: Uuid,
    /// `metadata.executionDepth` for the new row: the source's depth plus one.
    pub execution_depth: u32,
    /// `Flow.inputs`, **unrendered** — see [`FlowListener::inputs`].
    pub inputs: BTreeMap<String, serde_json::Value>,
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
    listener: &FlowListener,
    flow: &ListeningFlow,
    source: &SourceExecution,
    when: &WhenVerdict,
    max_depth: u32,
) -> Outcome {
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
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        evaluate, FiredExecution, FlowListener, FlowTriggerError, ListeningFlow, Outcome, Refusal,
        SourceExecution, WhenVerdict, DEFAULT_MAX_EXECUTION_DEPTH,
    };
    use crate::models::{ExecutionKind, State, Trigger};
    use uuid::Uuid;

    fn trigger(yaml: &str) -> Trigger {
        serde_norway::from_str(yaml).unwrap_or_else(|e| panic!("bad fixture trigger: {e}\n{yaml}"))
    }

    /// A flow trigger with no `states`, no `when`, no `dependsOn`.
    fn plain() -> FlowListener {
        FlowListener::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n",
        ))
        .expect("plain flow trigger resolves")
    }

    fn listening() -> ListeningFlow {
        ListeningFlow {
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
    fn run(l: &FlowListener, f: &ListeningFlow, s: &SourceExecution) -> Outcome {
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
        let l = FlowListener::resolve(&trigger(
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
            let l = FlowListener::resolve(&trigger(&format!(
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
        let l = FlowListener::resolve(&trigger(
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
        let l = FlowListener::resolve(&trigger(
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
        let absent = FlowListener::resolve(&trigger(
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
        let defaulted = FlowListener::resolve(&trigger(
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
        let l = plain();
        let s = source(State::Success);

        let same_pair = ListeningFlow {
            namespace: s.namespace.clone(),
            flow_id: s.flow_id.clone(),
            disabled: false,
            draft: false,
        };
        assert_eq!(refusal(run(&l, &same_pair, &s)), Refusal::SameFlow);

        let same_namespace = ListeningFlow {
            namespace: s.namespace.clone(),
            flow_id: "a-different-flow".into(),
            ..same_pair.clone()
        };
        assert!(
            matches!(run(&l, &same_namespace, &s), Outcome::Fired(_)),
            "same namespace, different id: Kestra compares the pair"
        );

        let same_flow_id = ListeningFlow {
            namespace: "a.different.namespace".into(),
            flow_id: s.flow_id.clone(),
            ..same_pair.clone()
        };
        assert!(
            matches!(run(&l, &same_flow_id, &s), Outcome::Fired(_)),
            "same id, different namespace: Kestra compares the pair"
        );
    }

    /// A disabled trigger and a disabled flow each refuse, and each has its own
    /// live twin.
    #[test]
    fn disabled_refuses_on_both_the_trigger_and_the_flow() {
        let s = source(State::Success);

        let off = FlowListener::resolve(&trigger(
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
        let l = FlowListener::resolve(&trigger(
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
            FlowListener::resolve(&one),
            Err(FlowTriggerError::UnsupportedDependsOn(1))
        );

        let two = trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n\
             dependsOn:\n  - namespace: t6.probe\n  - flowId: other\n",
        );
        assert_eq!(
            FlowListener::resolve(&two),
            Err(FlowTriggerError::UnsupportedDependsOn(2))
        );

        let empty =
            trigger("id: listen\ntype: io.kestra.plugin.core.trigger.Flow\ndependsOn: []\n");
        assert!(
            FlowListener::resolve(&empty).is_ok(),
            "`ListUtils.isEmpty` is what Kestra branches on, not `!= null`"
        );
        assert!(FlowListener::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n"
        ))
        .is_ok());
    }

    /// The twenty corpus declarations that carry a `dependsOn` are refused,
    /// and the one that does not is resolved.
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
            "expected the 345-flow corpus, found {}",
            files.len()
        );

        let mut declaring_flows = 0usize;
        let mut resolved = Vec::new();
        let mut refused = Vec::new();
        for path in &files {
            let text = std::fs::read_to_string(path).expect("read flow");
            let Ok(flow) = serde_norway::from_str::<crate::models::Flow>(&text) else {
                continue;
            };
            let mut declares = false;
            for t in &flow.triggers {
                if t.r#type != "io.kestra.plugin.core.trigger.Flow" {
                    continue;
                }
                declares = true;
                let name = format!("{}/{}", flow.id, t.id);
                match FlowListener::resolve(t) {
                    Ok(_) => resolved.push(name),
                    Err(FlowTriggerError::UnsupportedDependsOn(_)) => refused.push(name),
                    Err(e) => panic!("{name}: unexpected {e}"),
                }
            }
            if declares {
                declaring_flows += 1;
            }
        }

        assert_eq!(declaring_flows, 17, "corpus flows declaring a flow trigger");
        assert_eq!(
            resolved,
            vec!["flow-trigger-when-condition-flow-listen/on_completion"],
            "the one declaration this module can evaluate"
        );
        assert_eq!(refused.len(), 20, "declarations refused for `dependsOn`");
        assert_eq!(resolved.len() + refused.len(), 21);
    }

    /// A `core.schedule` is not a flow trigger and does not resolve as one.
    #[test]
    fn a_schedule_is_not_a_flow_trigger() {
        assert_eq!(
            FlowListener::resolve(&trigger(
                "id: t\ntype: io.kestra.plugin.core.trigger.Schedule\ncron: \"0 2 * * *\"\n"
            )),
            Err(FlowTriggerError::NotAFlowTrigger)
        );
        // ... and the inverse, so this is not measuring the fixture loader.
        assert!(FlowListener::resolve(&trigger(
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
    /// Measured across 211 trigger-fired executions on the artifact, in three
    /// independent rounds: `parentId` was `null` on every one and
    /// `trigger.variables.executionId` named the source. Migration 0138's
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
        let l = FlowListener::resolve(&trigger(
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

    /// The refusal reported is the FIRST reason in Kestra's order, not an
    /// arbitrary one.
    ///
    /// Built by stacking every refusal onto one input and peeling them off one
    /// at a time: each step must report exactly the next reason, which is a
    /// stronger statement than any single-refusal test and is the only thing
    /// that pins the *order*.
    #[test]
    fn the_refusals_are_reported_in_the_artifacts_order() {
        let disabled_listener = FlowListener::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n\
             states: [SUCCESS]\ndisabled: true\nwhen: \"{{ x }}\"\n",
        ))
        .unwrap();
        let enabled_listener = FlowListener::resolve(&trigger(
            "id: listen\ntype: io.kestra.plugin.core.trigger.Flow\n\
             states: [SUCCESS]\nwhen: \"{{ x }}\"\n",
        ))
        .unwrap();

        let worst_flow = ListeningFlow {
            namespace: "t6.probe".into(),
            flow_id: "producer".into(),
            disabled: true,
            draft: true,
        };
        let worst_source = SourceExecution {
            state: State::Created,
            kind: Some(ExecutionKind::Test),
            execution_depth: Some(DEFAULT_MAX_EXECUTION_DEPTH),
            ..source(State::Created)
        };

        // Peel one layer per step. The expected reason at each step is the
        // NEXT one in the bytecode's order.
        let steps: Vec<(Refusal, ListeningFlow, SourceExecution, &FlowListener)> = vec![
            (
                Refusal::EntryState,
                worst_flow.clone(),
                worst_source.clone(),
                &disabled_listener,
            ),
            (
                Refusal::SameFlow,
                worst_flow.clone(),
                SourceExecution {
                    state: State::Success,
                    ..worst_source.clone()
                },
                &disabled_listener,
            ),
            (
                Refusal::NotNormalKind(ExecutionKind::Test),
                ListeningFlow {
                    flow_id: "listener".into(),
                    ..worst_flow.clone()
                },
                SourceExecution {
                    state: State::Success,
                    ..worst_source.clone()
                },
                &disabled_listener,
            ),
            (
                Refusal::FlowDisabled,
                ListeningFlow {
                    flow_id: "listener".into(),
                    ..worst_flow.clone()
                },
                SourceExecution {
                    state: State::Success,
                    kind: None,
                    ..worst_source.clone()
                },
                &disabled_listener,
            ),
            (
                Refusal::FlowDraft,
                ListeningFlow {
                    flow_id: "listener".into(),
                    disabled: false,
                    ..worst_flow.clone()
                },
                SourceExecution {
                    state: State::Success,
                    kind: None,
                    ..worst_source.clone()
                },
                &disabled_listener,
            ),
            (
                Refusal::DepthExceeded {
                    depth: DEFAULT_MAX_EXECUTION_DEPTH + 1,
                    max: DEFAULT_MAX_EXECUTION_DEPTH,
                },
                ListeningFlow {
                    flow_id: "listener".into(),
                    disabled: false,
                    draft: false,
                    ..worst_flow.clone()
                },
                SourceExecution {
                    state: State::Success,
                    kind: None,
                    ..worst_source.clone()
                },
                &disabled_listener,
            ),
            (
                Refusal::TriggerDisabled,
                listening(),
                source(State::Success),
                &disabled_listener,
            ),
            (
                Refusal::StateNotListed(State::Warning),
                listening(),
                source(State::Warning),
                &enabled_listener,
            ),
            (
                Refusal::WhenFalsy("0".into()),
                listening(),
                source(State::Success),
                &enabled_listener,
            ),
        ];

        for (expected, flow, src, listener) in steps {
            let got = evaluate(
                listener,
                &flow,
                &src,
                &WhenVerdict::Rendered("0".into()),
                DEFAULT_MAX_EXECUTION_DEPTH,
            );
            assert_eq!(refusal(got), expected, "flow={flow:?} source={src:?}");
        }
    }
}
