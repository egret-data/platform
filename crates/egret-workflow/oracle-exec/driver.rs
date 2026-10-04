//! **The real driver behind the ratchet.** W3b Task 5.
//!
//! Until this file, `tests/executor.rs` drove `advance()` through a harness of
//! test code: four tiers that fed terminal states out of `golden.json`,
//! rendered expressions themselves from `corpus.json` plus the flow source, and
//! never spawned anything. `oracle-exec/README.md` published what that cost —
//! 39 entries differing on nothing but `spawned`, 227 of 1104 kept ordered
//! pairs never asserted, and one field the oracle was circular for.
//!
//! This module drives the **shipping** worker instead: `worker::start_execution`
//! writes an execution to Postgres, `worker::tick` claims its runs and hands
//! each to the registered handler, `worker::render` fills the render slots
//! inside the same transaction, and `worker::spawn` runs the loop iterations.
//! Nothing here reads `golden.json`.
//!
//! # What this driver still supplies, in full
//!
//! Three things, and they are the whole list. Each is a property of the
//! *environment* a worker runs in, not of the execution's semantics, and none
//! of them is read from the golden.
//!
//! 1. **The inputs.** `corpus.json` records what `harvest.py` POSTed when it
//!    captured each golden entry, and they are posted here for the same flow.
//!    A golden captured with `param: true` is a trace of the `then` branch, so
//!    a driver that could not post them would be comparing a different
//!    execution. This is the same provenance the harness used and is unchanged
//!    by Task 5.
//! 2. **The passage of time.** A `ScheduleRetry` writes a `not_before` and the
//!    worker will not reopen the run until the execution's clock passes it.
//!    Rather than sleep, [`pass_time`] moves `executions.created_at` back — the
//!    origin the `Tick` is measured from — by exactly the shortfall. That is
//!    the harness's tier 4 ("time never moves while anything else can happen")
//!    against a real column, and it is a *smaller* intervention than the
//!    harness's: it changes one timestamp and calls the same `resume` any other
//!    caller would.
//! 3. **The wall-clock cost of `core.flow.Sleep`.** The registry is built with
//!    `Sleep::capped(ZERO)`, which validates the duration exactly as the
//!    product handler does and then does not wait. Four corpus flows sleep for
//!    30 s or more and one for `PT10M`. Nothing the oracle compares can see
//!    this: `normalise.py` drops every clock from the golden, and egret's
//!    happens-before endpoints are the logical `executions.seq`, not instants.
//!
//! **What is no longer supplied is the thing that mattered:** a run's terminal
//! state. It comes from a handler that ran.
//!
//! # A stall is a wrong answer here, not a broken instrument
//!
//! The harness completed every `Runnable` from the golden, so an execution that
//! could not finish meant the state machine was spinning, and it was reported
//! as `Broken` — an unbaselineable failure. With real handlers that is no
//! longer true: a flow whose task type has no handler *stalls*, because
//! `Registry::run` refuses an unknown type rather than inventing a state for
//! it, and stalling is what the shipping worker really does. So this driver
//! returns the **partial trace** when nothing can move, and only a run that
//! exceeds [`STEP_BOUND`] or panics is `Broken`.
//! `every_mismatching_entry_is_attributed_to_the_field_it_differs_on` in
//! `tests/executor.rs` counts and names them, which keeps
//! that population from growing silently.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::worker::{self, Handlers};
use egret_workflow::{CreateFlowInput, FlowRepo, WorkflowRefs};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::{step_relation, Run, Trace};

/// How many driver steps one entry may take before the run is declared broken.
///
/// A step is one `tick` that ran something, or one `pass_time` that moved the
/// clock. Each ran-tick finishes exactly one task run, and each clock move
/// strictly increases the execution's own `now` to a distinct `not_before` that
/// a `ScheduleRetry` wrote. So the bound is "task runs the trace will hold,
/// plus retries", and the widest golden entry holds 765 history entries across
/// its execution, its runs, their attempts and its sub-executions
/// (`valids/loop-nested`, measured over the whole golden by
/// `the_step_bound_dominates_the_widest_golden_entry`). Four times that is the
/// same headroom the harness's bound carried, for the same reason: the parts
/// are loose rather than tight.
pub const STEP_BOUND: usize = 4 * 765;

/// **One run per tick — but the interleaving the golden was normalised against
/// is the WORKER's discipline, not this constant.**
///
/// The harness's tier 3 completed exactly one task per step — the
/// earliest-created still in flight — so exactly one interleaving was
/// reachable.
///
/// # This constant is not the lever, and W4 Task 4 measured that
///
/// This doc used to say that a batch of `n` "would let two siblings of one
/// `Parallel` finish inside one round and would make which of them created its
/// successor first depend on the batch size". **Both halves are false**, and
/// they are corrected here rather than left standing with a note beneath them.
/// `worker::claim` orders by `created_at, execution_id, run_ix` (W3b Task 7's
/// fix), which makes a batch of `n` indistinguishable from `n` successive
/// claims of 1 — a successor's `created_at` is strictly later than every
/// sibling already claimable, so the order is breadth-first either way — and
/// the happens-before endpoints are `executions.seq`, bumped once per `settle`,
/// so two siblings finished in one batch still end at different steps.
/// Measured: **`BATCH = 32` with the shipping worker leaves
/// `executor_matches_golden` at 143 mismatching and `executor.rs` at 34/34** —
/// the ratchet figure is the claim, and the `34/34` is the binary's size on the
/// day W4 Task 4 ran it. It is **35** since W4 Task 6 added a derivation test,
/// so a reader re-running this gets `35/35`; the row is about the 143.
///
/// The lever is the worker applying one handler result per **transaction**.
/// `BATCH = 32` with a prototype that applies a whole batch's results before
/// calling `advance()` banks `valids/dag`, `valids/full` and
/// `valids/parallel-disabled-tasks` (143 → 140, no regression); the raw output
/// is `oracle-exec/w4t4-widened-tick.txt`. W4 Task 4 ruled against it —
/// `worker::tick`'s doc carries the ruling and its costs, and
/// `tests/worker.rs::one_result_per_transaction_is_the_ruling_and_this_is_the_step_it_costs`
/// is the instrument. **Neither half moves the ratchet alone**, so raising this
/// constant without re-opening that ruling changes nothing.
///
/// What DOES still argue for 1 here, and is unaffected by the above: it bounds
/// the blast radius of a worker error. `Registry::run` refusing an unknown type
/// aborts the tick, and with a batch the runs claimed beside it keep a lease
/// until `reap` — which for a driver that must attribute every entry to the
/// flow rather than to the harness is noise it does not need.
const BATCH: usize = 1;

/// The registry the ratchet judges: the four handlers W3b ships, with `Sleep`
/// capped — see the module doc, item 3.
pub fn registry() -> Handlers {
    let mut r = Handlers::new();
    r.add(std::sync::Arc::new(
        egret_workflow::worker::handlers::log::Log::new(),
    ));
    r.add(std::sync::Arc::new(
        egret_workflow::worker::handlers::ret::Return,
    ));
    r.add(std::sync::Arc::new(
        egret_workflow::worker::handlers::fail::Fail::new(),
    ));
    r.add(std::sync::Arc::new(
        egret_workflow::worker::handlers::sleep::Sleep::capped(Duration::ZERO),
    ));
    r
}

// ---------------------------------------------------------------------------
// the corpus, in `workflow.flows` — W6 Task 1
// ---------------------------------------------------------------------------

/// What happened to one corpus flow on its way into `workflow.flows`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filed {
    /// A draft row exists AND a version of it is live. The product's whole
    /// path ran.
    Live,
    /// A draft row exists; `publish` refused it. The message is the refusal.
    DraftOnly(String),
    /// No row at all: the flow does not parse, or `FlowRepo::create` refused
    /// it. The message is the refusal.
    Absent(String),
}

/// **Put the whole corpus into `workflow.flows`, so a `(namespace, flowId)`
/// pair has a table to resolve against.** W6 Task 1.
///
/// # Why this exists at all
///
/// [`run`] hands `start_execution` a flow *source* and `None` for the flow id.
/// Nothing about that ever wrote a row to `workflow.flows`, so
/// `FlowRepo::find_by_name` — the one lookup a `core.subflow` task's
/// `(namespace, flowId)` pair can go through — had nothing to find, and egret
/// could not execute a `Subflow` at all. 25 corpus flows declare one.
///
/// # Two passes, and the order is `harvest.py`'s, not the alphabet's
///
/// Every draft is created before anything is published, for the reason
/// `harvest.py`'s own capture loop records: **ten corpus flows name another
/// corpus flow that sorts later**, and a publish-then-next loop would resolve
/// against a table that is still being filled. `harvest.py` measured that on
/// `valids/loop-with-subflow` — harvested flow-by-flow it failed with one loop
/// iteration, harvested after everything was published it succeeds with two,
/// "which is the whole of the 141-vs-142 gap".
///
/// The iteration order is `corpus.json`'s own array order — the order
/// `harvest.py` POSTed in — and **not** the sorted order [`super::corpus_json`]
/// hands back. That is load-bearing rather than tidy: three
/// `(namespace, flowId)` pairs are claimed by more than one corpus file
/// (`io.kestra.tests/switch-impossible` by 2, `/duplicate` by 3, `/empty` by
/// 4), `UNIQUE (workspace_id, namespace, flow_id)` (migration 0132) lets only
/// the first of each win, and sorted order would hand
/// `io.kestra.tests/switch-impossible` to `invalids/switch-invalid` instead of
/// to `valids/switch-impossible` — the one member of the three groups that
/// Kestra published and executed. [`NAME_CONTESTED`] is the guard.
///
/// # The path is the product's, and it is the product's that refuses
///
/// Pass 1 is `routes::create_flow` without the HTTP: `spec::parse`, the plan as
/// `serde_json::to_value(&flow)`, `FlowRepo::create`. Pass 2 is
/// `routes::publish_flow` without the HTTP: `publish::publish` through the real
/// [`WorkflowRefs`]. Nothing here inserts a row itself.
///
/// **`publish` refuses almost the whole corpus, and that is this task's
/// finding rather than its bug** — see this module's `PUBLISH_REFUSED_*`
/// constants and `oracle-exec/README.md`'s *The corpus in `workflow.flows`*
/// section for the mechanism and what it costs the rest of W6.
///
/// # What this deliberately does NOT do
///
/// It does not hand the flow's id to [`run`]'s `start_execution`. That would
/// set `executions.flow_id`, which `worker::claim` LEFT JOINs to read
/// `flows.created_by` as the run's `principal_id` — a second widening, with a
/// second population to attribute, bought for a field no corpus flow can
/// reach (`handlers::sql` is its only reader and
/// `no_corpus_entry_can_exercise_the_egret_sql_handler` measures that no
/// corpus entry reaches that handler). W6's `Subflow` links its child by its
/// own column, not by `parent_execution_id`, and can take the target's id from
/// this table directly.
pub async fn publish_corpus(pool: &PgPool, ws: Uuid, by: UserId) -> BTreeMap<String, Filed> {
    let repo = FlowRepo::new(Db { pool: pool.clone() });

    // The census below counts rows this function wrote. A table that was not
    // empty when it started would make every one of those figures a count of
    // somebody else's rows as well, and the process's own clone is created
    // from the migrated template, so this is a fact and not a hope.
    let already: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.flows")
        .fetch_one(pool)
        .await
        .expect("count workflow.flows");
    assert_eq!(
        already, 0,
        "workflow.flows must be empty before the corpus is filed into it, or \
         the census this function asserts counts rows it did not write"
    );

    let entries = super::corpus();
    let mut filed: BTreeMap<String, Filed> = BTreeMap::new();
    let mut drafts: Vec<(String, Uuid)> = Vec::new();
    // `"{namespace}/{flowId}"` for every `core.subflow` in the corpus, and the
    // same key for every corpus flow that HOLDS one of those names. The two
    // together are what [`target_census`] joins — see its doc for why the
    // population that matters is the targets and not the declarers.
    let mut targets: BTreeSet<String> = BTreeSet::new();
    let mut holder: BTreeMap<String, String> = BTreeMap::new();

    // Pass 1 — the draft. `routes::create_flow`'s three steps, minus the HTTP.
    for entry in &entries {
        let source = super::corpus_source(&entry.id);
        let flow = match egret_workflow::spec::parse(&source) {
            Ok(f) => f,
            Err(e) => {
                filed.insert(
                    entry.id.clone(),
                    Filed::Absent(format!("parse: {}", e.message)),
                );
                continue;
            }
        };
        let plan = serde_json::to_value(&flow).expect("a parsed flow serialises");
        let input = CreateFlowInput {
            namespace: flow.namespace.clone(),
            flow_id: flow.id.clone(),
            source,
            plan,
            folder_id: None,
        };
        // Collected from every corpus flow that PARSES, before the create can
        // refuse it: a `Subflow`'s target is a name its author wrote, and the
        // population is not narrowed by whether the flow naming it won a
        // `(namespace, flowId)` contest.
        subflow_targets(&flow, &mut targets);
        match repo.create(ws, by, &input).await {
            Ok(row) => {
                holder.insert(format!("{}/{}", flow.namespace, flow.id), entry.id.clone());
                drafts.push((entry.id.clone(), row.id));
            }
            Err(e) => {
                filed.insert(entry.id.clone(), Filed::Absent(format!("create: {e}")));
            }
        }
    }

    name_contest_won_by_the_right_file(&repo, &drafts).await;

    // Pass 2 — the version. `routes::publish_flow`, minus the HTTP, through the
    // real `WorkflowRefs`: `RefKind::Subflow` resolves through the same
    // `FlowRepo::find_by_name` the product uses, over the table pass 1 has just
    // finished filling.
    let refs = CountingRefs {
        inner: WorkflowRefs {
            flows: FlowRepo::new(Db { pool: pool.clone() }),
            dataapi_endpoints: Arc::new(NoDataApi),
        },
        subflow_lookups: std::sync::atomic::AtomicUsize::new(0),
    };
    let (mut on_published, mut on_refused) = (0usize, 0usize);
    for (corpus_id, flow) in &drafts {
        let before = refs.count();
        let outcome =
            match egret_workflow::publish::publish(&repo, &refs, ws, by, *flow, "oracle").await {
                Ok(_) => Filed::Live,
                Err(e) => Filed::DraftOnly(format!("{e}")),
            };
        // Attributed to the publish that made them, because a lookup on a flow
        // that goes on to be refused proves nothing about a flow that ships.
        let made = refs.count() - before;
        match outcome {
            Filed::Live => on_published += made,
            _ => on_refused += made,
        }
        filed.insert(corpus_id.clone(), outcome);
    }
    lookup_census(on_published, on_refused);

    census(&filed);
    target_census(&filed, &targets, &holder);
    filed
}

/// Every `(namespace, flowId)` a `core.subflow` in `flow` names, as
/// `"{namespace}/{flowId}"` — the exact key `publish::subflow_ref_name` builds
/// and `FlowRepo::find_by_name` splits back apart.
///
/// # This walk is deliberately WIDER than the product's, and the corpus cannot
/// tell
///
/// All four root lists, then `Task::children()` **and**
/// `Task::error_children()` at every depth. Neither `spec::validate`
/// (`validate.rs:1283`) nor `publish::resolve_task` recurses through
/// `error_children()` — both walk `children()` only — so **a `Subflow` nested
/// inside a flowable's `errors:` would never have its reference resolved at
/// publish.** That is the same blind spot W3b Task 5 found in
/// `worker::claim`'s `leaf_spec`, one seam over; it is latent rather than a
/// live defect, and it belongs to whoever owns `Subflow` resolution.
///
/// This census asks a different question from `publish` — *what would a
/// resolver have to reach* — so it takes the wider walk on purpose. **Measured:
/// the extra arm is redundant on today's corpus.** Deleting it (fix round 1
/// mutation 4) changes no figure and fails no test, because no corpus flow puts
/// a `core.subflow` inside an `errors:`. Recorded rather than removed, per
/// W4's rule that a redundant guard and a weak one look identical in a mutation
/// table unless somebody measures the difference and writes it down.
fn subflow_targets(flow: &egret_workflow::models::Flow, out: &mut BTreeSet<String>) {
    fn walk(t: &egret_workflow::models::Task, out: &mut BTreeSet<String>) {
        if let egret_workflow::models::TaskKind::Subflow(s) = &t.kind {
            if let (Some(namespace), Some(flow_id)) = (&s.namespace, &s.flow_id) {
                out.insert(format!("{namespace}/{flow_id}"));
            }
        }
        for child in t.children() {
            walk(child, out);
        }
        for child in t.error_children() {
            walk(child, out);
        }
    }
    for list in [
        &flow.tasks,
        &flow.errors,
        &flow.finally_,
        &flow.after_execution,
    ] {
        for task in list {
            walk(task, out);
        }
    }
}

/// **`RefKind::Subflow` resolutions `publish` made inside a publish that
/// SUCCEEDED** — the size of the surface steps 4-7 were actually exercised on.
///
/// Two sources, both through the `RefKind::Subflow` seam, in the [`LIVE`]
/// flows that reach a live version: one per `core.subflow` task, **and** one
/// per `dependsOn` entry of a `core.trigger.Flow` trigger, which
/// `resolve_trigger_dependencies` (there since W1, `228173f9`) resolves the
/// same way. No corpus flow declares an `egret.*` type, so no other `RefKind`
/// is reachable at all. The split between the two sources is not measured
/// here; the constant has counted both since it was set at W10 (`48f91c03`).
/// (Corrected in the W13b final-review fix wave: this paragraph used to say
/// no corpus flow declares a flow trigger, which was false when written.)
///
/// **18 through W8 Task 2, 19 through W10 Task 6, 47 now**, and the pair must
/// still be read as one measurement — but the arithmetic that held them
/// together has changed, and pretending otherwise would be the error this file
/// keeps warning about.
///
/// Through W8 Task 3 the total was **conserved** at 22: a flow that stopped
/// being refused moved its lookup from [`SUBFLOW_LOOKUPS_ON_REFUSED`] to here.
/// W10 Task 6 fix round 1 is different: the total went **22 → 49**, because a
/// publish that used to abort at the first unported `Leaf` now walks the whole
/// flow and resolves every `Subflow` behind it. The lookups did not move — they
/// had never been made at all. `resolve_all` reaching further is the surface
/// this constant measures, and it is now more than twice the size it was.
///
/// **47 → 45 at W13b Task 2**, and this time nothing about `Subflow` moved:
/// the two were the `dependsOn` pair of
/// `valids/flow-trigger-fire-once-true-flow-listen`, resolved through the
/// same `RefKind::Subflow` seam by `resolve_trigger_dependencies`. That flow
/// no longer parses -- `models::Window` refuses `fireOnce`,
/// as the pinned artifact does with 422 -- so it never reaches publish. See
/// [`PARSE_REFUSED`] and [`LIVE`], which moved with it.
const SUBFLOW_LOOKUPS_ON_PUBLISHED: usize = 45;

/// `RefKind::Subflow` resolutions made inside a publish that then FAILED — a
/// `Subflow` that `resolve_all` reached before it met the unported `Leaf` that
/// refused the flow. Counted apart because a lookup on a flow nobody can
/// publish is not evidence the reference resolution shipped.
///
/// **4 through W8 Task 2, 3 through W10 Task 6, 2 now** — see
/// [`SUBFLOW_LOOKUPS_ON_PUBLISHED`], which is where the flows that stopped
/// being refused went, and why this one falling by one while that one rose by
/// 28 is not a contradiction.
const SUBFLOW_LOOKUPS_ON_REFUSED: usize = 2;

/// Count and assert the reference lookups, so the claim about how big this
/// task's genuinely-new surface is has a running program behind it rather than
/// a sentence.
fn lookup_census(on_published: usize, on_refused: usize) {
    eprintln!(
        "[publish_corpus] {on_published} `RefKind::Subflow` lookup(s) inside a publish that \
         succeeded, {on_refused} inside one that failed"
    );
    assert_eq!(
        on_published, SUBFLOW_LOOKUPS_ON_PUBLISHED,
        "Subflow lookups on a flow that reached a live version"
    );
    assert_eq!(
        on_refused, SUBFLOW_LOOKUPS_ON_REFUSED,
        "Subflow lookups on a flow publish then refused"
    );
}

/// The real [`WorkflowRefs`], wrapped so the lookups it is asked for can be
/// counted. It delegates every call unchanged — the port under test is still
/// the product's.
struct CountingRefs {
    inner: WorkflowRefs,
    subflow_lookups: std::sync::atomic::AtomicUsize,
}

impl CountingRefs {
    fn count(&self) -> usize {
        self.subflow_lookups
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl egret_workflow::WorkflowRefPort for CountingRefs {
    async fn workspace_of(
        &self,
        kind: egret_workflow::RefKind,
        name_or_id: &str,
    ) -> Result<Option<(Uuid, Uuid)>, egret_core::Error> {
        if matches!(kind, egret_workflow::RefKind::Subflow) {
            self.subflow_lookups
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.inner.workspace_of(kind, name_or_id).await
    }
}

/// Every distinct string a corpus `core.subflow` names as its target, including
/// the one that is not a name at all — see [`SUBFLOW_TARGETS_UNRENDERED`].
const SUBFLOW_TARGETS: usize = 21;

/// **Targets whose corpus flow has a live version — what a resolver that went
/// through `flow_versions` would reach.**
///
/// **2 of the 20 literal names through W10 Task 6; 18 of 20 now.** This is the
/// figure `worker::subflow::resolve`'s reason 2 turned on, and fix round 1
/// turned it over: with the four core FQCNs in `spec::registry::TASK_TYPES`
/// (see [`LIVE`]) a snapshot resolver would reach almost every target rather
/// than almost none. **The resolution source did not change** — a parent and
/// its child must not disagree about which version of the world they are in,
/// which was always the binding reason — and that doc now says so, with this
/// argument marked retired rather than quietly dropped.
const SUBFLOW_TARGETS_LIVE: usize = 18;

/// Targets whose corpus flow has a draft row and no version — reachable only by
/// a resolver that reads `workflow.flows`, which is what
/// `FlowRepo::find_by_name` does. **17 through W10 Task 6, 1 now**; the 16 that
/// left went to [`SUBFLOW_TARGETS_LIVE`], not out of the population.
const SUBFLOW_TARGETS_DRAFT_ONLY: usize = 1;

/// A literal `(namespace, flowId)` no corpus flow holds: `dummy_namespace/
/// dummy_flow`, named by `valids/task-runif`. Task 3's decision, not this
/// task's — and explicitly not to be "fixed" by inventing a target.
const SUBFLOW_TARGETS_ABSENT: usize = 1;

/// **Not a name: `{{inputs.namespace}}/{{inputs.flowId}}`, counted apart so the
/// literal-name population is exactly 20.**
///
/// `valids/task-flow-dynamic`'s dynamic target, read out of the UNRENDERED
/// source because that is where `publish` reads it too. No corpus flow can hold
/// it, and it is not "a target that is missing" — it is the recorded
/// `executor::mod::PUBLISHED_BUT_REFUSED` row, the thing egret structurally
/// cannot express. Counted in the total rather than dropped, so a reader sees
/// the exclusion instead of inheriting it.
const SUBFLOW_TARGETS_UNRENDERED: usize = 1;

/// **What a `Subflow` resolver would actually reach, by target rather than by
/// declarer — and this is the figure Task 3's choice of resolution source turns
/// on.**
///
/// Fix round 1. The first version of this section published a figure that no
/// running program compared, and it was wrong twice over: it said "seven of the
/// nine remaining `Subflow` declarers" would be missed by a `flow_versions`
/// resolver. All nine of those declarers have a draft row, so nothing about
/// them is missed — and **declarers is the wrong population entirely.** What a
/// resolver has to reach is the set of names `Subflow` tasks POINT AT, and a
/// declarer that publishes says nothing about whether its target did.
///
/// So this is asserted here, beside the rest, rather than written into a
/// document nothing executes.
fn target_census(
    filed: &BTreeMap<String, Filed>,
    targets: &BTreeSet<String>,
    holder: &BTreeMap<String, String>,
) {
    let (mut live, mut draft_only, mut absent) = (Vec::new(), Vec::new(), Vec::new());
    let mut unrendered = Vec::new();
    for target in targets {
        // Split BEFORE the lookup, not after: a template is not a name that
        // happens to be missing, and folding it into `absent` is how the
        // literal-name denominator would silently become 21.
        if target.contains("{{") {
            unrendered.push(target.clone());
            continue;
        }
        match holder
            .get(target)
            .and_then(|corpus_id| filed.get(corpus_id))
        {
            Some(Filed::Live) => live.push(target.clone()),
            Some(_) => draft_only.push(target.clone()),
            None => absent.push(target.clone()),
        }
    }

    eprintln!(
        "[publish_corpus] {} distinct `Subflow` target(s) = {} literal name(s) \
         ({} live, {} draft-only, {} held by no corpus flow) + {} unrendered",
        targets.len(),
        live.len() + draft_only.len() + absent.len(),
        live.len(),
        draft_only.len(),
        absent.len(),
        unrendered.len(),
    );
    eprintln!("[publish_corpus]   targets live: {live:?}");
    eprintln!("[publish_corpus]   targets held by no corpus flow: {absent:?}");
    eprintln!("[publish_corpus]   targets unrendered: {unrendered:?}");

    assert_eq!(
        targets.len(),
        SUBFLOW_TARGETS,
        "distinct `Subflow` targets: {targets:?}"
    );
    assert_eq!(
        live.len(),
        SUBFLOW_TARGETS_LIVE,
        "targets a `flow_versions` resolver would reach: {live:?}"
    );
    assert_eq!(
        draft_only.len(),
        SUBFLOW_TARGETS_DRAFT_ONLY,
        "targets only a DRAFT-row resolver reaches: {draft_only:?}"
    );
    assert_eq!(
        absent.len(),
        SUBFLOW_TARGETS_ABSENT,
        "targets no corpus flow holds -- Task 3's decision, not this task's: {absent:?}"
    );
    assert_eq!(
        unrendered.len(),
        SUBFLOW_TARGETS_UNRENDERED,
        "targets that are not names at all: {unrendered:?}"
    );
    assert_eq!(
        live.len() + draft_only.len() + absent.len() + unrendered.len(),
        targets.len(),
        "every target is classified exactly once"
    );
}

/// The three `(namespace, flowId)` pairs more than one corpus file claims, and
/// **which file must win each** — the first of the group in `corpus.json`'s
/// array order.
///
/// Only the first line of each group matters to anything downstream, and only
/// the first is a flow Kestra published (`publish_status` 200) and executed:
/// `valids/switch-impossible` runs and has a golden entry; every other member
/// of every group is an `invalids/` flow Kestra itself refused with a 422 for
/// its own invalidity. A `Subflow` naming `io.kestra.tests/switch-impossible`
/// must therefore find the flow that runs, not the one that does not exist on
/// Kestra's side either.
///
/// This is a guard whose failure mode is nameable: swap the iteration in
/// [`publish_corpus`] to [`super::corpus_json`]'s sorted keys and
/// `invalids/switch-invalid` takes the name, silently, because it parses
/// perfectly well — egret's refusal of it is `validate`'s, at publish, and by
/// then the row is already in the table.
const NAME_CONTESTED: &[(&str, &str)] = &[
    (
        "io.kestra.tests/switch-impossible",
        "valids/switch-impossible",
    ),
    ("io.kestra.tests/duplicate", "invalids/duplicate"),
    ("io.kestra.tests/empty", "invalids/empty"),
];

/// Every contested `(namespace, flowId)` resolves — through the product's own
/// `FlowRepo::find_by_name`, the lookup a `core.subflow` actually goes through —
/// to the file [`NAME_CONTESTED`] names, and to no other.
///
/// Asserted against the table rather than against the loop that filled it: the
/// question is what a `Subflow` will find, and only the table can answer that.
async fn name_contest_won_by_the_right_file(repo: &FlowRepo, drafts: &[(String, Uuid)]) {
    for (name, winner) in NAME_CONTESTED {
        let (namespace, flow_id) = name.split_once('/').expect("a contested name is ns/id");
        let found = repo
            .find_by_name(namespace, flow_id)
            .await
            .expect("resolve a contested name")
            .unwrap_or_else(|| panic!("{name} resolves to nothing at all"));
        let drafted = drafts
            .iter()
            .find(|(_, id)| *id == found.0)
            .map(|(corpus_id, _)| corpus_id.as_str())
            .unwrap_or("a row this function did not write");
        assert_eq!(
            drafted, *winner,
            "{name} must resolve to {winner}; it resolves to {drafted}. The iteration \
             order in publish_corpus is corpus.json's array order for exactly this \
             reason — see NAME_CONTESTED."
        );
    }
}

/// A corpus flow whose YAML `spec::parse` refuses outright, so no draft row can
/// exist for it at all: `namespace`/`flow_id` are `NOT NULL` columns and there
/// is nothing to put in them. Kestra refused all of these too.
///
/// **5 → 6 at W13b Task 2**: `valids/flow-trigger-fire-once-true-flow-listen`,
/// whose `window: {fireOnce: true}` the pinned artifact refuses with 422
/// `Unrecognized field "fireOnce"` (`corpus.json`, `checkout-drift`) and
/// `models::Window`'s `deny_unknown_fields` now refuses too.
const PARSE_REFUSED: usize = 6;

/// A corpus flow that parses but loses its `(namespace, flowId)` to an earlier
/// file — the six losers of [`NAME_CONTESTED`]'s three groups.
const CREATE_REFUSED: usize = 4;

/// How many corpus flows reach a **live version**.
///
/// **16 through W8 Task 2, 21 now, and the five are named rather than
/// counted**: `valids/flow-concurrency-cancel-pause`,
/// `valids/flow-concurrency-queue-pause`,
/// `valids/follow-dependencies-partial-parent`, `valids/loop-pause` and
/// `valids/pause_on_pause`. Every one of them is a flow whose only non-control
/// task was a `Pause`, which W8 Task 3 made a typed `models::TaskKind` — so it
/// is dispatched before any `Leaf` arm exists to consult
/// `registry::known_type`, exactly as a `Subflow` is, and publish step 4 no
/// longer refuses it.
///
/// **That is an over-refusal closing, not a new acceptance.** The pinned
/// artifact published all five with a 200 (`corpus.json`'s `publish_status`),
/// so egret refusing them was a divergence this programme forbids in the
/// direction the ratchet cannot see: a flow nobody can publish never reaches
/// `golden.json` at all.
///
/// # 21 → **248** at W10 Task 6 fix round 1, and the same sentence explains it
///
/// The five above were an over-refusal closing one class at a time. This is the
/// same class closing wholesale: `spec::registry::TASK_TYPES` held egret's short
/// spellings and **none** of the four FQCNs `worker::Handlers::with_core_tasks`
/// implements, so publish refused every flow whose leaves were
/// `io.kestra.plugin.core.log.Log`, `…debug.Return`, `…execution.Fail` or
/// `…flow.Sleep` — which is most of a Kestra corpus. Programme spec §8 rule 4
/// had already decided that `type:` accepts the FQCN as canonical (*"a Kestra
/// flow pasted into egret runs"*); the table did not implement the rule.
///
/// It was found by W10 Task 6's end-to-end test, the first thing in the
/// repository to drive publish and the worker in one process, and the size of
/// what it was hiding is this constant: **227 corpus flows egret refused and
/// the pinned artifact published.** [`UNKNOWN_TYPE_REFUSED`] fell 296 → 68 in
/// the same change, and the two must be read together.
///
/// # 248 → **247** at W13b Task 2
///
/// The one that left is `valids/flow-trigger-fire-once-true-flow-listen`, now
/// refused at parse ([`PARSE_REFUSED`]). It is an over-acceptance closing: the
/// pinned artifact refused it (422, `checkout-drift`), so it has no golden
/// entry and the ratchet cannot see it move.
///
/// # 247 → **245** at W5b final fix round 1
///
/// `valids/secrets` (`secret()` in a `Return`'s `format` and a `Log`) and
/// `valids/webhook-secret-key` (`secret()` in a `Webhook` trigger's `key`)
/// are now refused at publish by W5b spec §6: `secret()` resolves only in a
/// `kestra.*` task's properties, and anywhere else is a save-only finding,
/// `secret_outside_plugin_task`. **This is a deliberate divergence, not an
/// over-refusal to close**: the pinned artifact published both, and egret
/// refused the call at run before W5b and still does (the render's
/// `SECRET_REFUSAL`), so publishing them only postponed the same refusal to
/// run time. They moved to [`VALIDATE_REFUSED`].
const LIVE: usize = 245;

/// How many corpus flows have a draft row that `publish` refused **because a
/// task names a type `spec::registry::TASK_TYPES` does not hold** — the
/// finding, and the reason [`LIVE`] is not 345.
///
/// (It read *"the reason `LIVE` is 16 and not 295"* until W8 Task 3 fix round
/// 1, and *"is 21 and not 295"* until W10 Task 6 fix round 2 — **three lines
/// below `const LIVE`, inside the doc block the previous round had just
/// rewritten, two lines from the parenthesis warning about exactly this.** A
/// fact corrected where it was found and left standing three lines away is this
/// branch's named failure mode, and it has now happened *twice in the same
/// paragraph*: the `[LIVE]` link was supposed to make the number unrestatable
/// and the sentence restated it anyway, because "21" is not the token a reader
/// sweeping for "16" is looking for. **The figure is gone entirely now** —
/// there is no numeral in this sentence to go stale.)
///
/// `registry::known_type` is an allow-list of fifteen rows — **five**
/// `egret.*` types (this sentence said "eight" from W6 until W10 Task 6 fix
/// round 2, and "seven" until W5a Task 2 turned `egret.container` and
/// `egret.script.python` into aliases; counted, not quoted, at `TASK_TYPES`),
/// `core.http`/`core.slack`, — **since W10 Task 6 fix round 1** — the four
/// Kestra FQCNs `worker::Handlers::with_core_tasks` implements
/// (`…core.log.Log`, `…core.debug.Return`, `…core.execution.Fail`,
/// `…core.flow.Sleep`) plus the `core.log` alias, and — **since W5a Task 2**
/// — the four `io.kestra.plugin.scripts.{shell,python}.{Commands,Script}`
/// FQCNs with the two `egret.*` aliases. The corpus writes none of the script
/// types, so the last addition moves no count in this file.
///
/// **That last clause is what makes [`LIVE`] 248.** This paragraph used to end
/// *"the flows that DO reach a version are exactly the corpus flows whose every
/// task is control flow, a `core.subflow` or a `core.pause` … so none meets the
/// allow-list at all"*, which was true when only 21 flows published and is
/// **false for 227 of the 248 now**: the overwhelming majority of live corpus
/// flows are live precisely *because* their leaves meet the allow-list. The old
/// sentence survives here only as the description of the **21**, which is the
/// population it was written for.
///
/// **This is not a hole in the product** *for the 68 that remain*: egret's
/// publish refusing a genuinely unported Kestra plugin —
/// `core.output.OutputValues`, `core.execution.Labels`, the `io.kestra.core.*`
/// test-only classes — is exactly the behaviour W1 built the table for.
///
/// **301 through W8 Task 2, 296 through W10 Task 6, 68 now.** The 228 that left
/// were NOT unported: `worker::handlers` has implemented all four of their types
/// since W3b, and only `TASK_TYPES` did not say so. See [`LIVE`] for the
/// mechanism and for why the figure was hiding behind a table rather than
/// behind a missing handler; that is the difference between "egret has not
/// ported this plugin" and "egret refuses a plugin it runs", and only the first
/// is a statement about the oracle rather than a defect.
const UNKNOWN_TYPE_REFUSED: usize = 68;

/// How many corpus flows have a draft row that `publish` refused for a reason
/// of **egret's own validate** — a real disagreement about the flow, not an
/// unported plugin. [`census`] prints all of them by name.
///
/// **19 → 20 at W10 Task 6 fix round 1**, and the direction is the informative
/// part: a flow that used to die at step 4 on an unported type now gets far
/// enough to be judged on its own merits, so this population can only grow when
/// [`UNKNOWN_TYPE_REFUSED`] shrinks. One flow moved between the two buckets;
/// nothing became newly wrong.
///
/// **20 → 22 at W5b final fix round 1**: the two `secret()` flows [`LIVE`]
/// names, refused by W5b spec §6's publish-time rule.
const VALIDATE_REFUSED: usize = 22;

/// Every corpus flow that has a draft row and no live version — the sum of the
/// two above, derived rather than written down a third time.
const DRAFT_ONLY: usize = UNKNOWN_TYPE_REFUSED + VALIDATE_REFUSED;

/// Assert the census, print it, and name the refusal classes.
///
/// A figure nothing compares is stale the moment the code moves, so every
/// number this task publishes is a constant above and an `assert_eq!` here.
fn census(filed: &BTreeMap<String, Filed>) {
    let mut live = Vec::new();
    let mut unknown_type = Vec::new();
    let mut validate_refused: Vec<(String, String)> = Vec::new();
    let mut parse_refused = Vec::new();
    let mut create_refused = Vec::new();
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for (id, what) in filed {
        match what {
            Filed::Live => live.push(id.clone()),
            Filed::DraftOnly(why) => {
                let class = publish_class(why);
                if class.starts_with("unknown task type ") {
                    unknown_type.push(id.clone());
                } else {
                    // The whole message, not the class: these nineteen are
                    // egret disagreeing with the flow rather than lacking a
                    // plugin, and three of them are flows Kestra PUBLISHED.
                    validate_refused.push((id.clone(), why.clone()));
                }
                *reasons.entry(class).or_default() += 1;
            }
            Filed::Absent(why) if why.starts_with("parse: ") => parse_refused.push(id.clone()),
            Filed::Absent(_) => create_refused.push(id.clone()),
        }
    }

    eprintln!(
        "[publish_corpus] {} corpus flows: {} live, {} draft-only ({} unported task type, \
         {} egret's own validate), {} unparseable, {} name-contested",
        filed.len(),
        live.len(),
        unknown_type.len() + validate_refused.len(),
        unknown_type.len(),
        validate_refused.len(),
        parse_refused.len(),
        create_refused.len(),
    );
    for (class, n) in &reasons {
        eprintln!("[publish_corpus]   publish refused {n:>3} × {class}");
    }
    for (id, why) in &validate_refused {
        eprintln!("[publish_corpus]   validate refused {id}: {why}");
    }
    eprintln!("[publish_corpus]   unparseable: {parse_refused:?}");
    eprintln!("[publish_corpus]   name-contested losers: {create_refused:?}");
    eprintln!("[publish_corpus]   live: {live:?}");

    assert_eq!(filed.len(), 345, "every corpus flow is accounted for");
    assert_eq!(live.len(), LIVE, "live versions: {live:?}");
    assert_eq!(
        unknown_type.len() + validate_refused.len(),
        DRAFT_ONLY,
        "drafts publish refused"
    );
    assert_eq!(
        unknown_type.len(),
        UNKNOWN_TYPE_REFUSED,
        "drafts refused for an unported task type"
    );
    assert_eq!(
        validate_refused.len(),
        VALIDATE_REFUSED,
        "drafts refused by egret's own validate: {validate_refused:?}"
    );
    assert_eq!(
        parse_refused.len(),
        PARSE_REFUSED,
        "corpus flows that do not parse: {parse_refused:?}"
    );
    assert_eq!(
        create_refused.len(),
        CREATE_REFUSED,
        "corpus flows that lost their name: {create_refused:?}"
    );
}

/// Group a publish refusal by its cause, so the census names classes rather
/// than 300 individual messages.
fn publish_class(message: &str) -> String {
    match message.split_once("unknown type '") {
        Some((_, rest)) => format!(
            "unknown task type '{}'",
            rest.split_once('\'').map(|(t, _)| t).unwrap_or(rest)
        ),
        None => message.chars().take(60).collect(),
    }
}

/// `RefKind::DataApiEndpoint` is structurally unreachable from this corpus,
/// and this double is how that is asserted rather than assumed.
///
/// It is declared by an `egret.*` task type (`spec::registry::TASK_TYPES`),
/// `publish` only reads a `RefField` after `registry::known_type` has
/// already matched, and `no_corpus_entry_can_exercise_the_egret_sql_handler`
/// measures that no corpus flow names an `egret.*` type at all. A double
/// that answered `Ok(None)` instead would turn a corpus that had grown one
/// into a silent `NotFound` indistinguishable from the 300-odd refusals
/// beside it. Same shape, and the same reason, as `refs.rs`'s own
/// `UnreachableDataApi`.
struct NoDataApi;

#[async_trait::async_trait]
impl egret_dataapi_api::DataApiEndpointPort for NoDataApi {
    async fn workspace_of(&self, _id: Uuid) -> Result<Option<Uuid>, egret_core::Error> {
        unreachable!("no corpus flow names an `egret.dataapi.call`")
    }
}

/// What one entry's run produced.
pub enum Ran {
    /// The trace the driver read back out of Postgres.
    Trace(Box<Trace>),
    /// `build_plan` refused the flow. The one thing a `refused` golden entry
    /// compares.
    Refused(String),
    /// The instrument failed: the step bound was exceeded, or a database error
    /// nothing about the flow explains.
    Broken(String),
}

/// Start `source` as a real execution and drive it to a standstill.
///
/// `inputs` is what `harvest.py` posted for this flow — see the module doc.
pub async fn run(
    pool: &PgPool,
    workspace: Uuid,
    reg: &Handlers,
    source: &str,
    inputs: serde_json::Value,
) -> Ran {
    // `build_plan` first, and separately, so that a flow egret refuses is
    // reported as a refusal rather than as a driver failure. It is the same
    // call `start_execution` makes; making it here costs one plan build and
    // buys the distinction the `refused` verdict rests on.
    if let Err(e) = egret_workflow::executor::build_plan(source) {
        return Ran::Refused(e.message);
    }
    let id = match worker::start_execution(pool, workspace, None, source, inputs).await {
        Ok(id) => id,
        Err(e) => return Ran::Broken(format!("start_execution: {e}")),
    };

    for step in 0..=STEP_BOUND {
        if step == STEP_BOUND {
            return Ran::Broken(format!(
                "did not settle within {STEP_BOUND} steps; execution {id}"
            ));
        }
        // A worker error is NOT a driver error. `Registry::run` refuses a type
        // it has no handler for, which leaves that one run leased and unmoved —
        // exactly what the shipping worker does — and the driver carries on so
        // that the rest of the execution still produces the trace it produces.
        // With `BATCH == 1` the refusal can only ever concern the run it names.
        //
        // **`WorkerError::Corrupt` is the exception, and it used to be swallowed
        // with everything else.** It means a stored row cannot be read back into
        // what `advance()` reads — a `task_ix` the plan does not hold, an
        // `attempts` entry with no `started_at` — which is the DRIVER's own
        // storage being wrong rather than the flow diverging. That is the class
        // of the one product defect this package found, and under `Err(_) =>
        // continue` it could only ever surface as a stalled execution, i.e. as
        // an ordinary mismatch, and never as `Broken`. `Broken` is the verdict
        // `every_mismatching_entry_is_attributed_to_the_field_it_differs_on`
        // asserts at 0.
        match worker::tick(pool, BATCH, reg).await {
            Ok(0) => {}
            Ok(_) => continue,
            Err(e) => match corrupt(&e) {
                true => return Ran::Broken(format!("tick: {e}")),
                false => continue,
            },
        }
        match pass_time(pool, id).await {
            Ok(true) => continue,
            Ok(false) => break,
            Err(e) => return Ran::Broken(format!("pass_time: {e}")),
        }
    }

    match read_trace(pool, id).await {
        Ok(t) => Ran::Trace(Box::new(t)),
        Err(e) => Ran::Broken(format!("read_trace: {e}")),
    }
}

/// Whether a worker error is the storage being wrong rather than the flow
/// diverging.
///
/// Downcast rather than matched on the message: `WorkerError` reaches here
/// inside `egret_core::Error::Internal(anyhow::Error)`, and `anyhow!` over a
/// `std::error::Error` preserves the concrete type, so the classification
/// cannot drift when somebody rewords the `#[error(...)]` string.
fn corrupt(e: &egret_core::Error) -> bool {
    let egret_core::Error::Internal(a) = e else {
        return false;
    };
    matches!(
        a.downcast_ref::<egret_workflow::worker::WorkerError>(),
        Some(egret_workflow::worker::WorkerError::Corrupt(..))
    )
}

/// The harness's tier 4, against a real clock.
///
/// Nothing is claimable and nothing rendered, so the only thing that can still
/// move the execution is a deadline: a run whose `not_before_ms` is ahead of
/// the execution's own `now`. `Tick` is milliseconds since
/// `executions.created_at`, so moving that column **back** by the shortfall is
/// exactly "time passed", and it is the whole intervention — the state, the
/// histories and the retry policy are untouched, and `worker::resume` then does
/// the same work it would have done had the driver waited.
///
/// Time moves to the **earliest** deadline still ahead, over the whole tree,
/// and never further: a `timeout` that fired early would be a different
/// execution.
///
/// Returns whether anything moved.
async fn pass_time(pool: &PgPool, root: Uuid) -> Result<bool, sqlx::Error> {
    let due: Option<(Uuid, i64)> = sqlx::query_as(
        "WITH RECURSIVE tree AS ( \
             SELECT id, created_at FROM workflow.executions WHERE id = $1 \
             UNION ALL \
             SELECT e.id, e.created_at FROM workflow.executions e \
               JOIN tree t ON e.parent_execution_id = t.id \
         ) \
         SELECT t.id, MIN(r.not_before_ms) - \
                MIN((EXTRACT(EPOCH FROM (now() - t.created_at)) * 1000)::bigint) AS shortfall \
           FROM tree t JOIN workflow.task_runs r ON r.execution_id = t.id \
          WHERE r.not_before_ms IS NOT NULL \
            AND r.not_before_ms > (EXTRACT(EPOCH FROM (now() - t.created_at)) * 1000)::bigint \
          GROUP BY t.id \
          ORDER BY shortfall \
          LIMIT 1",
    )
    .bind(root)
    .fetch_optional(pool)
    .await?;

    let Some((execution, shortfall)) = due else {
        return Ok(false);
    };
    sqlx::query(
        "UPDATE workflow.executions \
            SET created_at = created_at - make_interval(secs => $1::double precision / 1000.0) \
          WHERE id = $2",
    )
    // One millisecond past the deadline rather than exactly on it: `Tick` is
    // integral milliseconds and `reopen_due`'s predicate is `not_before <= now`,
    // so landing exactly on the boundary would depend on a rounding this driver
    // does not control.
    .bind(shortfall + 1)
    .bind(execution)
    .execute(pool)
    .await?;
    worker::resume(pool, execution)
        .await
        .map_err(|e| sqlx::Error::Protocol(format!("resume: {e}")))?;
    Ok(true)
}

/// Read one execution and everything under it back out of Postgres, in the
/// shape the golden is compared in.
///
/// # `spawned` is ordered the way `normalise.py` orders it
///
/// `(parent run ordinal, loop index)` — Rule 5's "by the parent run", with the
/// loop index as the tiebreak, because the creation instant carries no
/// information at all: every sub-execution of one batch is written in one
/// transaction and shares `now()`, and on Kestra's side all 670 loop
/// sub-executions carry their parent execution's `startDate`.
async fn read_trace(pool: &PgPool, id: Uuid) -> Result<Trace, sqlx::Error> {
    Box::pin(read_trace_inner(pool, id, None, None)).await
}

async fn read_trace_inner(
    pool: &PgPool,
    id: Uuid,
    parent_run: Option<usize>,
    value: Option<String>,
) -> Result<Trace, sqlx::Error> {
    let history: serde_json::Value =
        sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?;

    let rows = sqlx::query(
        "SELECT task_id, value, parent_ix, history, attempts, created_seq, ended_seq \
           FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let mut runs = Vec::with_capacity(rows.len());
    let mut created: Vec<Option<usize>> = Vec::with_capacity(rows.len());
    let mut ended: Vec<Option<usize>> = Vec::with_capacity(rows.len());
    for r in &rows {
        runs.push(Run {
            task: r.get("task_id"),
            value: r.get("value"),
            parent: r.get::<Option<i32>, _>("parent_ix").map(|p| p as usize),
            state: strings(&r.get::<serde_json::Value, _>("history")),
            attempts: r
                .get::<serde_json::Value, _>("attempts")
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|x| strings(x.get("history").unwrap_or(&serde_json::Value::Null)))
                        .collect()
                })
                .unwrap_or_default(),
        });
        created.push(r.get::<Option<i64>, _>("created_seq").map(|v| v as usize));
        ended.push(r.get::<Option<i64>, _>("ended_seq").map(|v| v as usize));
    }

    let mut order = Vec::new();
    for i in 0..runs.len() {
        for j in (i + 1)..runs.len() {
            if let Some(rel) = step_relation(created[i], ended[i], created[j], ended[j]) {
                order.push((i, j, rel.to_string()));
            }
        }
    }

    // **This query is why `Subflow` gets a column of its own** (W6 Task 2). It
    // keys on `parent_execution_id`, and all 41 golden `spawned`-carrying
    // entries have ZERO `Subflow` children -- so a subflow child linked this
    // way would turn the entries W6 exists to bank into `spawned` mismatches.
    // Migration 0138 makes that unrepresentable rather than merely
    // discouraged: `workflow_executions_one_link_kind` forbids a row carrying
    // both links, and `a_subflow_child_is_invisible_to_the_query_that_builds_spawned`
    // below drives this function over one and asserts the empty `spawned`.
    //
    // **Widened by W6 Task 4, which is the commit that starts writing the row
    // it could not decode.** The tuple was `(Uuid, i32, String)`, which was
    // exactly right while 0135's CHECK made a child with a parent and no loop
    // index unrepresentable. 0138 made that child representable and
    // `worker::replay` now creates it: the `CREATE_NEW_EXECUTION` replay, whose
    // `parent_run_ix`, `loop_index` and `loop_value` are all NULL and whose
    // four golden entries want `{"parent_run": null, "value": null}` in
    // `spawned`. Decoding a NULL into `i32` is a `ColumnDecode` error, so the
    // narrow tuple would have turned all four into `Broken` -- the verdict
    // `every_mismatching_entry_is_attributed_to_the_field_it_differs_on`
    // asserts at 0 -- rather than into a mismatch.
    //
    // **`ORDER BY parent_run_ix, loop_index` is unchanged, and the NULLs are
    // measured rather than sorted by accident.** Postgres puts NULLs LAST in an
    // ascending order, so a replay would sort after every loop child of the
    // same parent. Nothing can observe that: over the 710 descendants in
    // `traces.json`, 40 carry a NULL `loopIndex`, **no parent carries more than
    // one of them, and no parent mixes one with a loop child** -- and on
    // egret's side the same holds by construction, because
    // `worker::replay::spawn_due` refuses to replay a `Loop` sub-execution at
    // all (the artifact cannot do it either; see that function's doc) and
    // migration 0138's `NULLS NOT DISTINCT` unique index admits one replay per
    // parent.
    let children: Vec<(Uuid, Option<i32>, Option<String>)> = sqlx::query_as(
        "SELECT id, parent_run_ix, loop_value FROM workflow.executions \
          WHERE parent_execution_id = $1 ORDER BY parent_run_ix, loop_index",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let mut spawned = Vec::with_capacity(children.len());
    for (child, run_ix, loop_value) in children {
        spawned.push(
            Box::pin(read_trace_inner(
                pool,
                child,
                run_ix.map(|ix| ix.max(0) as usize),
                loop_value,
            ))
            .await?,
        );
    }

    Ok(Trace {
        parent_run,
        value,
        execution: strings(&history),
        runs,
        order,
        spawned,
    })
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// --------------------------------------------------------------------------
// W6 Task 2: the constraint the whole package's banks depend on
// --------------------------------------------------------------------------

#[cfg(test)]
mod w6_subflow_link {
    use super::*;

    /// Insert one execution, optionally linked to `parent` the LOOP way or to
    /// `trigger` the SUBFLOW way.
    ///
    /// Written here rather than driven through `worker::start_execution`
    /// because nothing spawns a subflow child yet -- W6 Task 3 does -- and the
    /// property under test is a property of the QUERY, which reads rows and
    /// does not care who wrote them.
    async fn child(
        pool: &PgPool,
        ws: Uuid,
        parent: Option<(Uuid, i32, i32, &str)>,
        trigger: Option<(Uuid, i32)>,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO workflow.executions \
               (workspace_id, namespace, flow_key, source, state, history, \
                parent_execution_id, parent_run_ix, loop_index, loop_value, \
                trigger_execution_id, trigger_run_ix) \
             VALUES ($1, 't', 'f', 'src', 'SUCCESS', \
                     '[\"CREATED\",\"RUNNING\",\"SUCCESS\"]'::jsonb, \
                     $2, $3, $4, $5, $6, $7) \
             RETURNING id",
        )
        .bind(ws)
        .bind(parent.map(|p| p.0))
        .bind(parent.map(|p| p.1))
        .bind(parent.map(|p| p.2))
        .bind(parent.map(|p| p.3))
        .bind(trigger.map(|t| t.0))
        .bind(trigger.map(|t| t.1))
        .fetch_one(pool)
        .await
        .expect("insert an execution")
    }

    /// **A subflow child must not appear in `spawned`, and a loop child must.**
    ///
    /// This is the single assertion the rest of W6 rests on. [`read_trace`]
    /// builds `spawned` from `parent_execution_id`; measured over the golden,
    /// all **41** `spawned`-carrying entries have **zero** `Subflow` children,
    /// and Kestra's own artifact agrees -- `valids/subflow-parent` ran its
    /// `Subflow` to SUCCESS in all five rounds and `traces.json` records
    /// `"descendants": []` for it, because `harvest.py` follows `parentId` and
    /// Kestra leaves `parentId` NULL for a subflow child.
    ///
    /// **The loop child is the positive control and it is not optional.**
    /// Without it, deleting the whole `spawned` query would leave this test
    /// green: an empty `spawned` is what a subflow child is supposed to
    /// produce, so "no children found" cannot be the only thing asserted.
    #[sqlx::test(migrations = "../../migrations")]
    async fn a_subflow_child_is_invisible_to_the_query_that_builds_spawned(pool: PgPool) {
        // # This call cannot fire once any `#[sqlx::test]` in this binary has run
        //
        // **Pre-existing, not W8's, and it corrupts mutation runs** — recorded
        // by W8 Task 2 fix round 2 because this is where the plant happens.
        //
        // `assert_database_url_is_exported()` returns early when `DATABASE_URL`
        // is present in the PROCESS environment, and `#[sqlx::test]` calls
        // `dotenvy::dotenv()` unconditionally, which puts `.env`'s value there.
        // This test is itself a `#[sqlx::test]`. So in a run where nobody
        // exported the variable, whichever DB-gated test wins the race sees a
        // planted value and **passes**, while the ones that lose fail — and
        // because test order inside a binary is not deterministic, **which one
        // fails rotates between runs**.
        //
        // The consequence worth writing down: a mutation run made without
        // exporting `DATABASE_URL` collects **exactly one rotating red for
        // free**, and a free red is a red that can be mistaken for a kill. Every
        // mutation verdict in W8 Task 2 was taken with the variable exported and
        // against a 0-failing control for that reason.
        //
        // **Owner: `egret-test-db`**, not this driver and not W8 — the fix is
        // for `assert_database_url_is_exported` to distinguish a value the
        // operator exported from one `dotenvy` planted, which it already knows
        // how to do (`dotenv_file_setting` reads the FILE for exactly this
        // reason) but does not do on the early-return path.
        egret_test_db::assert_database_url_is_exported();
        let db = Db { pool: pool.clone() };
        let (ws, _user) = crate::common::seed_workspace(&db).await;

        let root = child(&pool, ws, None, None).await;
        child(&pool, ws, None, Some((root, 0))).await;

        let trace = read_trace(&pool, root)
            .await
            .expect("read the root's trace");
        assert!(
            trace.spawned.is_empty(),
            "a subflow child reached the oracle's `spawned`. It is linked by \
             `trigger_execution_id`; this query keys on `parent_execution_id`; \
             the two are kept apart by migration 0138's \
             `workflow_executions_one_link_kind`. All 41 golden \
             `spawned`-carrying entries have zero `Subflow` children, so this \
             turns W6's intended banks into `spawned` mismatches. Got: {:?}",
            trace.spawned
        );

        let loop_child = child(&pool, ws, Some((root, 0, 0, "a")), None).await;
        let trace = read_trace(&pool, root)
            .await
            .expect("read the root's trace again");
        assert_eq!(
            trace.spawned.len(),
            1,
            "the positive control: a LOOP child of the same parent must still \
             be found by the same query, or the assertion above passes because \
             the query is broken rather than because the link is right"
        );
        assert_eq!(trace.spawned[0].parent_run, Some(0));
        assert_eq!(trace.spawned[0].value.as_deref(), Some("a"));
        assert_ne!(loop_child, root, "sanity: the child is a row of its own");
    }
}
