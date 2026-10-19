// kestra: webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java

//! The **unauthenticated** webhook surface — W6 Task 7.
//!
//! ```text
//! POST|PUT|GET  /workflows/v1/{ws}/executions/webhook/{namespace}/{flow}/{key}
//! ```
//!
//! Kestra's is
//! `@AnonymousAccess @Post("/api/v1/{tenant}/executions/webhook/{namespace}/{id}/{key}{/path}")`
//! — measured off `ExecutionController`'s bytecode and confirmed against a
//! running server. This is that shape with Kestra's `{tenant}`, which it
//! resolves from the request through `tenantService.resolveTenant()`, replaced
//! by egret's `{ws}` **slug in the path**.
//!
//! **The parameter is spelled `{ws}` and not `{workspace}`**, following
//! `egret-dataapi`'s serving router, whose own doc records `{workspace}` as the
//! marker of a route the `WsContext` feature gate is expected to cover. This
//! surface is deliberately outside that gate — it has no session for the gate
//! to resolve.
//!
//! **That spelling is a signal to a reader here, not a mechanism**, and fix
//! round 1's sweep is what made the difference matter: `egret-server`'s
//! `tests/feature_route_coverage.rs` suite reads the **OpenAPI document**, and this route
//! contributes nothing to it (see "no OpenAPI document" below), so neither
//! spelling would be seen by that suite either way. For `egret-dataapi`'s
//! serving router — which *does* publish a document — the spelling really is
//! load-bearing. Saying so here without the qualification claimed a guard this
//! route does not have.
//!
//! What *is* enforced is written down rather than mechanised:
//! `egret-workspace`'s `features.rs` records the exemption beside the
//! `workflows` entry, as it already did beside `data_api`. **Turning the
//! `workflows` module off does not stop webhook traffic**; `state = 'disabled'`
//! on the flow does — and so does `disabled: true` in its YAML, and so does
//! the trigger's runtime switch (a `workflow.disabled_triggers` row) — which is
//! the same answer the Data API gives for its own serving path.
//!
//! # This surface is OFF by default, and here is the switch
//!
//! **`EGRET_WORKFLOW__WEBHOOKS_ENABLED`** — `[workflow] webhooks_enabled` in
//! `egret.toml`, read through `egret_config::Config` like every other
//! deployment switch and set into
//! `egret_server::AppState::workflow_webhooks_enabled` by `egret-bin`. It is in
//! the chart (`deploy/helm/egret/values.yaml`'s `workflow.webhooksEnabled`,
//! wired through `configmap-env.yaml`) and it follows the
//! `EGRET_<SECTION>__<KEY>` convention `EGRET_CATALOG__ENABLED` already sets.
//!
//! Fix round 2 first shipped it as a flat `EGRET_WORKFLOW_WEBHOOKS_ENABLED`
//! read with `std::env::var` in `main.rs` — outside the config type, outside
//! the chart and outside the convention (fix round 3, Minors 2 and 3).
//!
//! **Default `false`.** figment does the string→bool step, so a value it cannot
//! parse is a **startup error** rather than a silent `false`: an operator who
//! meant to turn this on and mistyped finds out at boot, and a typo can never
//! turn it on. With it off the routes below are **not mounted at all**, so
//! their URLs answer exactly as a URL nobody ever wrote does — measured, and
//! asserted against an unrouted control in
//! `egret-server/tests/workflow_webhook_mount.rs`.
//!
//! It exists because the alternative was worse than a gap. This surface's own
//! security review returned *not approved for exposure*, and until fix round 2
//! the router was mounted unconditionally — so an operator who had read that
//! verdict and agreed with it **had nothing to do about it**. A documented
//! limitation nobody can act on is not a control. The lever is the difference.
//!
//! It gates only this group. Workflow management
//! (`/api/ws/{workspace}/workflows/...`, session-authenticated) is unaffected,
//! which is the same split `flows.state = 'disabled'` makes per flow.
//!
//! # Authorization is the whole of this module
//!
//! A webhook is an unauthenticated public surface by construction: **the key in
//! the URL is the only credential there is.** Everything below follows from
//! that.
//!
//! ## The scoping, and the two places it is load-bearing
//!
//! 1. **The flow lookup is `workspace_id = $1 AND namespace = $2 AND flow_id =
//!    $3`, and `workspace_id` is the ONLY thing standing between a caller and
//!    another tenant's flow.** `workflow.flows` carries
//!    `UNIQUE (workspace_id, namespace, flow_id)`, so two workspaces may both
//!    hold `io.kestra.tests/webhook` — that is not an edge case, it is what the
//!    corpus looks like when two people import it.
//!
//!    **`FlowRepo::find_by_name` was never used here** (removed 2026-10-19,
//!    debt spec §5.1): it was `WHERE namespace = $1 AND flow_id = $2 ORDER BY
//!    id LIMIT 1`, with no workspace predicate at all, because
//!    `WorkflowRefPort` has no caller-workspace parameter. `worker::subflow::resolve` already refused it
//!    for the same reason and recorded that **nothing in the schema is a second
//!    line of defence** — that module's own mutation showed a cross-workspace
//!    resolve producing a child execution with no constraint anywhere objecting.
//!    This module has the workspace, so it queries with it.
//!
//! 2. **The key is compared only against the triggers of the flow that lookup
//!    returned.** A key is not a workspace-wide capability and not a
//!    namespace-wide one: it authorizes exactly one trigger of exactly one
//!    flow of exactly one workspace. `tests/webhook_route.rs` stands two
//!    workspaces up holding byte-identical flows and asserts that each key
//!    fires its own and that **no execution row appears in the other
//!    workspace** — a count, not a status code, because a 404 with a
//!    side-effect is the failure that matters.
//!
//! An archived workspace answers 404: `get_by_slug(...).filter(|w| w.status ==
//! "active")`, the same predicate `egret-dataapi`'s serving path applies for
//! the same reason.
//!
//! ## What every refusal looks like, and why two of them diverge from Kestra
//!
//! | condition | Kestra | here |
//! |---|---|---|
//! | workspace absent or archived | n/a | **404** `not found: webhook` |
//! | flow absent, deleted, or unparseable | **404** `Flow not found` | **404** `not found: webhook` |
//! | no trigger whose key matches | **404** `Webhook not found` | **404** `not found: webhook` |
//! | flow `disabled` | **409**, *before* the key check | **409**, *after* it |
//! | matched trigger `disabled: true` | **409** `…the trigger 'x' is disabled.` | **409**, same sentence |
//! | matched trigger switched off at runtime (`workflow.disabled_triggers`) | n/a | **409**, same sentence |
//! | trigger declares something egret cannot do | n/a | **409**, naming it |
//! | trigger `inputs:` will not render | **422** | **400** (`InvalidInput`) |
//!
//! **The first three rows are one byte-identical response**, and that is the
//! divergence. Kestra distinguishes "no such flow" from "no such webhook" on a
//! surface where the caller has presented no credential at all, which makes the
//! endpoint an oracle for *does workspace W, namespace N hold a flow called F*.
//! Anyone can enumerate it. Collapsing them costs an operator one diagnostic
//! and removes the enumeration; `the_four_absences_are_indistinguishable` is
//! the running check that they stay collapsed, and it compares **bodies**, not
//! just statuses.
//!
//! **The 409s were moved behind the key comparison** for the same reason, and
//! that reordering is safe in the direction that matters: a caller who has not
//! produced the key learns nothing, and a caller who has produced it already
//! knows the flow exists, so telling it *why* the request will not run discloses
//! nothing new. Neither divergence ever fires where Kestra would not — the
//! programme's actual constraint — because both only ever turn a 200 or a 409
//! into a 404.
//!
//! The `inputs:` row is a status difference and nothing more: egret has no
//! `Error` variant that maps to 422 and inventing one for this seam would be a
//! cross-crate change for one endpoint.
//!
//! # Which source a webhook runs, and the deferral that has now arrived
//!
//! **The DRAFT row's `workflow.flows.source`**, not a `flow_versions` snapshot.
//! `worker::subflow::resolve`'s doc explicitly reserved that decision for
//! "whoever adds a product route that starts an execution"; this is that route,
//! and the decision plus its three reasons and its one real cost are written
//! down **in both files**, because a parent and its child must not disagree
//! about which version of the world they are in. **Reason 2 — "a
//! version-snapshot resolver would reach almost nothing", measured at 16 of 345
//! corpus flows reaching a live version — was RETIRED at W10 Task 6 fix round
//! 1**, which made egret able to publish a pasted Kestra flow at all and took
//! that figure to **248 of 345**. The decision is unchanged; see that
//! function's doc for why reasons 1 and 3 are what it rests on.
//!
//! The cost, restated here so a reader of this file does not have to find it:
//! **an author editing a draft changes what a live webhook URL runs, with no
//! publish step in between.**
//!
//! # What is deliberately not here
//!
//! * **No `wait:`.** See [`crate::trigger::webhook`]'s "knowingly narrower"
//!   list. A flow declaring it — or `returnOutputs`, `responseContentType` or
//!   `responseCode` — is refused **by name**, one `if` per field, rather than
//!   answered with a response shape it did not ask for. That is fix round 1's
//!   Important 3: the first version refused only `wait` and silently ignored the
//!   other three.
//! * **No OpenAPI document.** The management router's `openapi()` covers the
//!   session-authenticated seam; this one is machine-facing and anonymous, like
//!   `catalog_engine`'s mount, which also carries none.
//!
//! # The bounds, and what they are NOT
//!
//! Fix round 1, Important 6. The first version had one bound — the body cap —
//! and justified the absence of the rest as "parity with Kestra". That does not
//! transfer: Kestra's webhook lives behind a single tenant's own deployment,
//! and this one is a path-scoped surface shared by every workspace in a
//! multi-tenant install, where one caller's flood is another tenant's outage.
//! So there are three, and each is stated with what it does not do:
//!
//! 1. **The body cap** ([`MAX_WEBHOOK_BODY_BYTES`]), enforced by a layer before
//!    the handler.
//! 2. **A bulkhead** ([`WEBHOOK_MAX_CONCURRENCY`]) — a semaphore built ONCE per
//!    router, so at most that many webhook requests are in flight at a time and
//!    the rest are refused rather than queued. It bounds the damage a flood does
//!    to the *rest of the process*; it does not bound the flood.
//! 3. **A per-target attempt limiter** ([`WEBHOOK_ATTEMPTS_PER_WINDOW`] in
//!    [`WEBHOOK_ATTEMPT_WINDOW`]), keyed on `(workspace, namespace, flow)` and
//!    counting **every** attempt, matched or not. This is what bounds key
//!    guessing: a key is the whole credential, and without it an attacker gets
//!    as many guesses per second as egret can serve 404s.
//!
//! **What they are not.** Both limiters are **in-process**, so an install with
//! N replicas allows N times the stated rate, and both reset when the process
//! does. A shared-store limiter (the token bucket `egret-dataapi` uses) is the
//! honest version and is not built here. The keying is on the *target*, not the
//! source address, so one flow's attackers and its legitimate callers share a
//! budget — deliberate, because an attacker chooses its source address and does
//! not choose the target. Neither of these is a claim that the surface is safe
//! to expose to the open internet; the security review's verdict stands, and
//! these bound the blast radius rather than remove it.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use egret_core::{Error, Result as EgretResult};
use serde::Serialize;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::routes::WorkflowState;
use crate::trigger::webhook::{
    build_output, declared_unsupported, find, render_inputs, ExecutionTrigger, InputContext,
    KeyContext, WebhookRequest,
};

/// The largest webhook body this surface accepts, in bytes.
///
/// **Chosen, and the number is pinned by
/// `tests::the_body_cap_is_this_number_and_not_another`** -- a test that only
/// sends `cap + 1` and `cap` passes at any value, which is what the first
/// version of it did (fix round 1, Important 2: raised 64x, still green).
///
/// # The comparison this doc used to make was wrong
///
/// It said "1 MiB is `egret-dataapi`'s serving cap and there is no reason for
/// this seam to be more generous than the one that requires a token".
/// `SERVING_MAX_BODY_BYTES` is **64 KiB**, so that sentence was false twice
/// over: the figure was wrong by 16x, and the conclusion it drew was the
/// opposite of what its own premise implied -- the anonymous seam was the more
/// generous one.
///
/// # Why 256 KiB rather than 64 KiB
///
/// The two surfaces differ in what legitimate traffic looks like.
/// `SERVING_MAX_BODY_BYTES`'s own doc says its bound is generous "since
/// legitimate is zero" -- a Data API request body carries parameters, not
/// payload. A webhook body **is** the payload: it is the message the caller
/// came to deliver, it becomes `trigger.body`, and a GitHub push event alone is
/// routinely tens of kilobytes. 64 KiB would refuse real traffic.
///
/// 256 KiB is four times that and still small enough that the per-request cost
/// egret bears -- hold the bytes, parse them as JSON, write them into a `jsonb`
/// column -- is bounded well under a megabyte. It is a **choice**, not a
/// measurement, and it is the largest single input an unauthenticated caller
/// can make this process allocate.
pub const MAX_WEBHOOK_BODY_BYTES: usize = 256 * 1024;

/// How many webhook requests may be in flight at once, per process.
///
/// `egret-dataapi`'s `SERVING_MAX_CONCURRENCY` is 64 for a surface that runs a
/// query; this is smaller because a webhook request does strictly more work
/// before it can be refused — it renders every candidate key with Pebble — and
/// because it is reachable without a credential. Chosen, not measured.
///
/// The semaphore is built **once per router**: [`WebhookGuard::new`] is the one
/// place it is constructed, and [`webhook_router`] calls it once. A caller that
/// builds a router per request gets a fresh semaphore per request, which is a
/// limit nobody can reach — the mistake `egret-dataapi`'s `serving_bulkhead`
/// doc warns about, repeated here because this module has its own constructor.
/// [`webhook_router_with_guard`] exists so a caller whose router outlives its
/// requests can supply one.
pub const WEBHOOK_MAX_CONCURRENCY: usize = 16;

/// How many attempts one `(workspace, namespace, flow)` may make per
/// [`WEBHOOK_ATTEMPT_WINDOW`].
///
/// It counts **every** attempt, matched or refused, because the thing being
/// bounded is key *guessing* and a guess is a refusal. Chosen: 60 per minute is
/// far above any real webhook's rate and far below anything useful for brute
/// force — a 16-character key drawn from 62 symbols is ~10^28 guesses, and at
/// this rate the search takes longer than the universe has existed either way,
/// so the number is about bounding COST, not about making the search
/// infeasible.
pub const WEBHOOK_ATTEMPTS_PER_WINDOW: u32 = 60;

/// The attempt limiter's fixed window.
pub const WEBHOOK_ATTEMPT_WINDOW: Duration = Duration::from_secs(60);

/// What the attempt limiter counts against: the three path segments that name a
/// target, and deliberately **not** the key — keying on the key would give an
/// attacker a fresh budget per guess.
pub type AttemptKey = (String, String, String);

/// One fixed window: when it started, and how many attempts it has seen.
///
/// `count == 0` means the slot has never been used; the instant is then
/// whatever the guard was built at, which is exactly what the reset arm wants.
pub type AttemptWindow = (Instant, u32);

/// How many windows the attempt limiter keeps, for ever.
///
/// **This is a CAP, not a sweep trigger, and the difference is fix round 2's
/// Important 2.** The previous version kept a `HashMap<AttemptKey, _>` and
/// swept expired entries when the map reached 4096 — which bounds nothing,
/// because every one of the three segments is attacker-chosen and none has to
/// name anything real. 65,536 distinct URLs inside one window produce 65,536
/// live entries, and each admission then walks the whole map under the mutex.
/// A rate limiter built to stop a flood was itself the cheapest flood in the
/// process.
///
/// The replacement allocates **once, at construction**, and never again: a
/// fixed array of windows, indexed by a hash of the key. Admission is O(1) —
/// one hash, one index, one compare — and the mutex is held for that and
/// nothing else. Memory is `WEBHOOK_ATTEMPT_SLOTS * size_of::<AttemptWindow>()`
/// whatever anyone sends, which at 4096 slots is under 100 KiB.
pub const WEBHOOK_ATTEMPT_SLOTS: usize = 4096;

/// The two in-process bounds, built once per router.
///
/// **Fixed window, not a token bucket**, and the difference is admitted rather
/// than hidden: a fixed window lets 2x the rate through across a boundary. A
/// bucket needs either a shared store (which this crate does not depend on) or
/// per-key state with eviction, and per-key state is what Important 2 was
/// about.
///
/// # Collisions are real, bounded, and unchooseable
///
/// [`WEBHOOK_ATTEMPT_SLOTS`] windows serve every target, so two targets whose
/// keys hash to one slot **share a budget**. That is the price of a constant
/// allocation and it is the right trade on an anonymous surface: the
/// alternative — exact per-target accounting — is an allocation the attacker
/// sizes.
///
/// The damage is bounded and it cannot be aimed. The hasher is a
/// [`std::collections::hash_map::RandomState`] built **per guard**, i.e. per
/// process, so the slot a given `(workspace, namespace, flow)` lands in is not
/// derivable from outside and is different in every replica. An attacker
/// spending a whole window on one slot degrades `1/4096` of targets in one
/// process, chosen by neither of you, and the degradation is a 429 that clears
/// within [`WEBHOOK_ATTEMPT_WINDOW`].
#[derive(Debug)]
pub struct WebhookGuard {
    bulkhead: Semaphore,
    /// Fixed length [`WEBHOOK_ATTEMPT_SLOTS`], allocated once in [`Self::new`]
    /// and never resized. `Box<[_]>` rather than `Vec<_>` so that "never
    /// resized" is a property of the type rather than of the code below.
    attempts: Mutex<Box<[AttemptWindow]>>,
    /// Fixed per guard, so the key→slot mapping is stable within a process and
    /// unpredictable between them.
    hasher: RandomState,
}

impl WebhookGuard {
    #[must_use]
    pub fn new(permits: usize) -> Self {
        let now = Instant::now();
        Self {
            bulkhead: Semaphore::new(permits),
            attempts: Mutex::new(vec![(now, 0u32); WEBHOOK_ATTEMPT_SLOTS].into_boxed_slice()),
            hasher: RandomState::new(),
        }
    }

    /// Which window serves `key`.
    fn slot(&self, key: &AttemptKey) -> usize {
        // `WEBHOOK_ATTEMPT_SLOTS` is a power of two, so the mask is exact. It
        // is asserted rather than assumed -- see
        // `tests::the_slot_count_is_a_power_of_two`.
        (self.hasher.hash_one(key) as usize) & (WEBHOOK_ATTEMPT_SLOTS - 1)
    }

    /// Record one attempt against `key`. `false` means the window is exhausted.
    ///
    /// `now` is a parameter so the test drives it rather than sleeping — this
    /// workspace has lost whole gate runs to wall-clock assertions.
    ///
    /// **The work under the lock is O(1) and independent of what has been sent
    /// before**, which is the whole of Important 2's fix. There is no sweep, no
    /// iteration and no insertion.
    /// # The `count == 0` arm is not redundant, and here is what it changes
    ///
    /// Fix round 3, Minor 1. A slot that has never been used holds
    /// `(built_at, 0)`. Without the first arm, the **first** admission against
    /// a fresh slot within a window of construction would not reset it, so the
    /// window it starts is measured from **the guard's construction** rather
    /// than from that admission — and the target gets whatever is left of a
    /// window it never used.
    ///
    /// It is observable, and `tests::an_unused_slot_starts_its_window_when_it_is_first_used`
    /// is the discriminator: build at `t0`, spend the quota at `t0 + 59s`, ask
    /// again at `t0 + 61s`. With the arm the window began at `t0 + 59s` and is
    /// still live, so the caller is refused; without it the window began at
    /// `t0` and has expired, so the caller is admitted with a fresh budget.
    pub fn admit_attempt(&self, key: AttemptKey, now: Instant) -> bool {
        let slot = self.slot(&key);
        let mut windows = self.attempts.lock().expect("webhook attempt map poisoned");
        let window = &mut windows[slot];
        if window.1 == 0 || now.saturating_duration_since(window.0) >= WEBHOOK_ATTEMPT_WINDOW {
            *window = (now, 0);
        }
        window.1 += 1;
        window.1 <= WEBHOOK_ATTEMPTS_PER_WINDOW
    }

    /// How many bytes of window state this guard holds. Constant, and the test
    /// that says so sends a million distinct keys at it.
    #[must_use]
    pub fn window_bytes(&self) -> usize {
        self.attempts.lock().expect("poisoned").len() * std::mem::size_of::<AttemptWindow>()
    }
}

/// The single "no" this surface gives to anyone who has not produced the key.
///
/// A constant rather than three call sites, because the property the tests
/// assert is that the three absences are **indistinguishable**, and three
/// hand-written strings drift.
fn not_found() -> Error {
    Error::NotFound("webhook".into())
}

/// `io.kestra.plugin.core.trigger.WebhookResponse`, reduced to the fields
/// egret can fill.
///
/// Kestra's carries `tenantId`, `id`, `namespace`, `flowId`, `flowRevision`,
/// `trigger`, `outputs`, `labels`, `state` and `url` (measured on the wire).
/// Dropped here, each with a reason: `flowRevision` (egret has no revision),
/// `outputs` (the `returnOutputs` streaming path, not implemented), `labels`
/// (there is no `labels` COLUMN on an egret execution — the flow's own
/// `labels:` block plus the one synthesised `system.correlationId` are built
/// per render by `worker::render::Surface::load`, which this response cannot
/// reach; `system.from` is not synthesised anywhere. *This sentence said
/// "Kestra's execution-label scheme, which egret does not have", and
/// `{{ labels.system.correlationId }}` renders in egret — W6 Task 8*) and
/// `url` (needs the configured public base URL, which this crate does not
/// read).
#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    /// egret's workspace id — Kestra's `tenantId`.
    pub workspace_id: Uuid,
    /// The execution that was created.
    pub id: Uuid,
    pub namespace: String,
    pub flow_id: String,
    /// `{id, type, variables}` — exactly what was stored on the execution row.
    pub trigger: ExecutionTrigger,
    /// Always `CREATED`: this surface answers asynchronously.
    pub state: &'static str,
}

/// The anonymous webhook router. Mount it **outside** `/api` and outside the
/// CSRF layer.
///
/// Outside `/api` because `WsContext`'s feature-entitlement gate recognises a
/// path beginning `/api/ws/` or `/ws/` and would demand a session this caller
/// does not have. Outside CSRF for the reason `egret-dataapi`'s serving mount
/// records: double-submit CSRF defends requests a browser authenticates
/// *ambiently*, and nothing here is ambient — the credential is a path segment
/// the caller types.
///
/// All three methods share one handler, as Kestra's five do.
pub fn webhook_router() -> Router<WorkflowState> {
    webhook_router_with_guard(Arc::new(WebhookGuard::new(WEBHOOK_MAX_CONCURRENCY)))
}

/// [`webhook_router`] with its guard **supplied**, so a test can drive a bound
/// it genuinely exceeds instead of asserting that a layer was installed.
///
/// The guard is the one of the bounds whose meaning depends on the router
/// outliving the request; see [`WEBHOOK_MAX_CONCURRENCY`].
pub fn webhook_router_with_guard(guard: Arc<WebhookGuard>) -> Router<WorkflowState> {
    Router::new()
        .route(
            "/{ws}/executions/webhook/{namespace}/{flow}/{key}",
            post(handle).put(handle).get(handle),
        )
        .layer(axum::Extension(guard))
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
}

/// Every method lands here. `GET` carries no body, which is not a special case:
/// an empty body is already "no body" (see
/// [`crate::trigger::webhook::build_output`]).
async fn handle(
    State(st): State<WorkflowState>,
    axum::Extension(guard): axum::Extension<Arc<WebhookGuard>>,
    Path((workspace, namespace, flow, key)): Path<(String, String, String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> EgretResult<Json<WebhookResponse>> {
    // --- 0. the two bounds, before anything that costs -----------------------
    //
    // The attempt limiter is keyed on the THREE path segments that name a
    // target, and NOT on the key -- keying on the key would give an attacker a
    // fresh budget per guess, which is the opposite of a limiter.
    //
    // It runs before the bulkhead permit is taken, so a flood is refused
    // without occupying one; and both run before the first database call, so a
    // refusal costs no query. A refused attempt is still COUNTED, which is the
    // whole point.
    if !guard.admit_attempt(
        (workspace.clone(), namespace.clone(), flow.clone()),
        Instant::now(),
    ) {
        return Err(Error::TooManyRequests);
    }
    let _permit = guard
        .bulkhead
        .try_acquire()
        .map_err(|_| Error::TooManyRequests)?;

    // --- 1. the workspace, from the path and from nowhere else ---------------
    //
    // `status == "active"` is checked here rather than left to a later query:
    // an archived workspace must not be startable by a caller holding an old
    // key, and this is the one place that fact is in hand.
    let ws = st
        .workspaces
        .get_by_slug(&workspace)
        .await?
        .filter(|w| w.status == "active")
        .ok_or_else(not_found)?;
    let ws_id = ws.id.0;

    // --- 2. the flow, scoped to THAT workspace -------------------------------
    //
    // Scoped by workspace -- see the module doc. `deleted_at IS NULL`
    // for the same reason `subflow::resolve` has it; `state` is read rather
    // than filtered, because a disabled flow still has to be told apart from an
    // absent one AFTER the key matches.
    let row: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, source, state FROM workflow.flows \
          WHERE workspace_id = $1 AND namespace = $2 AND flow_id = $3 \
            AND deleted_at IS NULL",
    )
    .bind(ws_id)
    .bind(&namespace)
    .bind(&flow)
    .fetch_optional(&st.db.pool)
    .await
    .map_err(|e| Error::Internal(anyhow::anyhow!("webhook flow lookup: {e}")))?;
    let Some((flow_uuid, source, state)) = row else {
        return Err(not_found());
    };

    // An unparseable draft has no triggers, so no key can match it. Answering
    // the same 404 rather than a 409 keeps it in the indistinguishable set.
    let Ok(parsed) = crate::spec::parse(&source) else {
        return Err(not_found());
    };

    // --- 3. the key ----------------------------------------------------------
    let kcx = KeyContext {
        flow_id: parsed.id.clone(),
        namespace: parsed.namespace.clone(),
        vars: parsed.variables.clone().into_iter().collect(),
    };
    let Some((trigger, webhook)) = find(&parsed.triggers, &key, &kcx) else {
        return Err(not_found());
    };

    // --- 4. from here the caller HAS the key, so refusals may explain ---------
    if trigger.disabled {
        // Kestra's own sentence, measured on the wire.
        return Err(Error::Conflict(format!(
            "Cannot execute webhook: the trigger '{}' is disabled.",
            trigger.id
        )));
    }
    // The runtime switch (W7e, spec §3.2). A webhook has no `workflow.triggers`
    // row, so this reads `workflow.disabled_triggers` directly -- and only now,
    // after the key matched, so a wrong key or a missing flow still gets the
    // one shared 404. Same sentence as the YAML switch above.
    let switched_off: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM workflow.disabled_triggers \
          WHERE flow_id = $1 AND trigger_id = $2)",
    )
    .bind(flow_uuid)
    .bind(&trigger.id)
    .fetch_one(&st.db.pool)
    .await
    .map_err(|e| Error::Internal(anyhow::anyhow!("webhook switch lookup: {e}")))?;
    if switched_off {
        return Err(Error::Conflict(format!(
            "Cannot execute webhook: the trigger '{}' is disabled.",
            trigger.id
        )));
    }
    // **Both ways a flow can be switched off, and the YAML one was missing.**
    //
    // (The runtime switch is per trigger and was checked above.)
    //
    // Fix round 1, Critical 1. egret has two: the `state` column, which is
    // honoured here though nothing writes it yet, and the YAML's own
    // `disabled: true`, which is
    // Kestra's `Flow.isDisabled()` and therefore the one the artifact answers
    // 409 for. The first version of this handler read only the column, so a
    // flow whose source said `disabled: true` FIRED -- over-acceptance, the one
    // direction the oracle cannot catch. `trigger::flow::evaluate` was already
    // checking `flow.disabled` for the same field; this seam was not.
    //
    // `parsed.draft` is deliberately NOT checked: measured on the artifact, a
    // `draft: true` flow answers **200** through the webhook route.
    if parsed.disabled || state == "disabled" {
        return Err(Error::Conflict(
            "Cannot execute flow: flow is disabled.".into(),
        ));
    }
    // Declarations this port parses and cannot honour. **Refused by name**, one
    // `if` per field, rather than accepted and ignored.
    //
    // Fix round 1, Important 3: `returnOutputs`, `responseContentType` and
    // `responseCode` were parsed into `models::Webhook` and then silently
    // dropped, which is the `lateMaximumDelay` shape -- a field an author can
    // write, that validates, and that changes nothing. Only `wait` was refused.
    // The list lives in `trigger::webhook::declared_unsupported`, which the
    // flow schema's measurement (`spec::schema`) also calls, so a property
    // refused here is never offered by the editor's completion.
    if let Some(unsupported) = declared_unsupported(webhook) {
        return Err(Error::Conflict(unsupported.message(&trigger.id)));
    }

    // --- 5. what the flow will see -------------------------------------------
    // The verb is not passed on: `WebhookRequest` has no `method` field, for
    // the reason its doc gives. It is read above only to reject nothing -- all
    // three verbs reach this handler, as Kestra's five do.
    let req = WebhookRequest {
        query: parse_query(query.as_deref()),
        headers: collect_headers(&headers),
        content_type: headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        body: body.to_vec(),
    };
    let output = build_output(&req, webhook.fetch_type, None)
        .map_err(|u| Error::Conflict(u.message(&trigger.id)))?;

    // `inputs:` render against a context that binds **no** `trigger` at all, so
    // the key comparison's private object is unreachable rather than merely
    // un-asked-for. See `trigger::webhook::InputContext` for why the previous
    // substring scan was not a guard.
    //
    // Two refusals, two statuses: a capability gap egret owes (409, named) and
    // an author's broken template (400).
    let inputs = render_inputs(webhook, &InputContext::of(&kcx)).map_err(|r| match r {
        crate::trigger::webhook::InputRefusal::NeedsTriggerContext { .. } => {
            Error::Conflict(r.message(&trigger.id))
        }
        crate::trigger::webhook::InputRefusal::Unrenderable { .. } => {
            Error::InvalidInput(r.message(&trigger.id))
        }
    })?;

    // --- 6. start it ---------------------------------------------------------
    let record = ExecutionTrigger {
        id: trigger.id.clone(),
        r#type: trigger.r#type.clone(),
        variables: output,
    };
    let trigger_json = serde_json::to_value(&record)
        .map_err(|e| Error::Internal(anyhow::anyhow!("serialising the trigger record: {e}")))?;

    let id = crate::worker::start_execution_with_trigger(
        &st.db.pool,
        ws_id,
        Some(flow_uuid),
        &source,
        inputs,
        Some(trigger_json),
    )
    .await?;

    Ok(Json(WebhookResponse {
        workspace_id: ws_id,
        id,
        namespace: parsed.namespace,
        flow_id: parsed.id,
        trigger: record,
        state: "CREATED",
    }))
}

/// `URIBuilder(uri).getQueryParams()` — split on `&`, then on the **first**
/// `=`, percent-decoding both halves and treating `+` as a space.
///
/// Written out rather than taken from `serde_urlencoded`, because that crate
/// deserialises into a map and a map cannot hold `?a=1&a=2`, which is the one
/// shape this function exists to preserve.
pub(crate) fn parse_query(raw: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            // `?flag` with no `=` is a name with an empty value, which is what
            // `URIBuilder` produces for it too.
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

/// Percent-decoding with `+` as a space, lossy on invalid UTF-8.
///
/// Lossy rather than failing: a webhook must not 400 on a query string it can
/// still route, and the key itself never comes through here -- it is a path
/// segment axum has already decoded.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            // **Byte arithmetic, never string slicing.**
            //
            // Fix round 1, Important 4: this read `&s[i + 1..i + 3]`, which
            // PANICS when those offsets are not char boundaries -- `?a=%1é` was
            // enough, and it unwound the connection task on an anonymous public
            // endpoint. The doc immediately above promises "lossy rather than
            // failing"; slicing a `str` by byte offset cannot keep that promise,
            // so the two hex digits are decoded from the bytes directly.
            b'%' if i + 2 < bytes.len() => {
                match (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi << 4 | lo);
                        i += 3;
                    }
                    // A malformed escape is a literal `%`, not an error.
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One hex digit, or `None`.
///
/// Written out rather than reached for through `u8::from_str_radix`, because
/// that function takes a `&str` and getting one out of a `&[u8]` at an
/// arbitrary offset is exactly the slicing that panicked.
#[must_use]
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Request headers as `(lowercase-name, value)`.
///
/// Kestra's are `java.net.http.HttpHeaders.map()`, which is already a
/// multimap; the grouping into lists happens in
/// [`crate::trigger::webhook::build_output`] so that headers and query
/// parameters go through one code path.
///
/// **The iteration order is `HeaderMap`'s, which is unspecified.** This doc used
/// to say "in wire order" (fix round 1's sweep); `HeaderMap::iter` makes no such
/// guarantee, and nothing here can restore it. It matters only for a header sent
/// twice with different values, whose two values may therefore be grouped in
/// either order. Recorded rather than claimed away -- `parse_query` DOES
/// preserve order, because a query string is one string this function reads left
/// to right, and that difference between the two is real.
///
/// A header whose value is not valid UTF-8 is **dropped**, not lossily decoded:
/// it would otherwise reach a flow as a string the caller never sent.
/// `tests::a_non_utf8_header_is_dropped_rather_than_mangled` is the test, which
/// the first version of this claim did not have.
fn collect_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|s| (k.as_str().to_string(), s.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `?a=1&a=2` is the shape a map cannot hold, which is why this function
    /// exists at all.
    #[test]
    fn repeated_query_parameters_survive_parsing() {
        assert_eq!(
            parse_query(Some("validationToken=abc&validationToken=def&x=1")),
            vec![
                ("validationToken".to_string(), "abc".to_string()),
                ("validationToken".to_string(), "def".to_string()),
                ("x".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn query_parsing_handles_the_awkward_shapes() {
        assert_eq!(parse_query(None), vec![]);
        assert_eq!(parse_query(Some("")), vec![]);
        assert_eq!(
            parse_query(Some("flag")),
            vec![("flag".to_string(), String::new())],
            "a bare name is a name with an empty value"
        );
        assert_eq!(
            parse_query(Some("a=")),
            vec![("a".to_string(), String::new())]
        );
        assert_eq!(
            parse_query(Some("a=b=c")),
            vec![("a".to_string(), "b=c".to_string())],
            "split on the FIRST `=` only"
        );
        assert_eq!(
            parse_query(Some("a=1&&b=2")),
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string())
            ],
            "an empty pair is skipped, not turned into an empty name"
        );
    }

    /// **The cap is pinned to a number, not to itself.**
    ///
    /// Fix round 1, Important 2: the only test of the cap sent `cap + 1` and
    /// `cap`, so raising the constant 64x left it green. A bound nothing
    /// compares to a literal is a bound that can move silently.
    ///
    /// Fix round 2, Important 3 / claim 13: the *second* version of this test
    /// had **M24's own shape inside the round that found M24**. It compared
    /// `MAX_WEBHOOK_BODY_BYTES` to a local function returning the literal
    /// `64 * 1024` and claimed "if that constant moves, this is what says so" —
    /// which is false, and the review proved it by raising the real
    /// `SERVING_MAX_BODY_BYTES` 16x and watching this pass. A copy of a number
    /// cannot detect the original moving.
    ///
    /// So it now READS the original. `egret-workflow` must not depend on
    /// `egret-dataapi` (the port-trait convention), and a test may still open a
    /// file — which the policy tests in this crate already do. The parse is
    /// deliberately strict: a `const` whose spelling changes fails loudly here
    /// rather than silently reverting to a copied number.
    #[test]
    fn the_body_cap_is_this_number_and_not_another() {
        assert_eq!(MAX_WEBHOOK_BODY_BYTES, 256 * 1024);
        assert_eq!(
            MAX_WEBHOOK_BODY_BYTES as u64,
            4 * serving_max_body_bytes(),
            "this seam is documented as four times `SERVING_MAX_BODY_BYTES`; one of the two \
             moved"
        );
    }

    /// `egret_dataapi::serving::guard::SERVING_MAX_BODY_BYTES`, read from its
    /// source.
    ///
    /// Not a dependency edge: this crate must not depend on `egret-dataapi`, so
    /// the value is parsed out of the file. Not a copy either — that was claim
    /// 13. If the constant is renamed, moved to another file, or written in a
    /// form this does not parse, the test **fails** rather than falling back to
    /// a literal, because a silent fallback is exactly the copy this replaced.
    fn serving_max_body_bytes() -> u64 {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../egret-dataapi/src/serving/guard.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let needle = "pub const SERVING_MAX_BODY_BYTES: u64 = ";
        let at = src
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not found in {}", path.display()));
        let expr: String = src[at + needle.len()..]
            .chars()
            .take_while(|c| *c != ';')
            .collect();
        // The two forms this has ever been written in: `N` and `N * M`.
        let value: u64 = expr
            .split('*')
            .map(|part| {
                part.trim()
                    .replace('_', "")
                    .parse::<u64>()
                    .unwrap_or_else(|e| panic!("cannot parse {expr:?} from guard.rs: {e}"))
            })
            .product();
        assert!(value > 0, "parsed {expr:?} as zero");
        value
    }

    /// The reader above must be able to FAIL — otherwise it is a copy with
    /// extra steps, which is what claim 13 was.
    ///
    /// It is driven against a fixture whose constant is a different number, so
    /// the parse is shown to track the file rather than to return a constant.
    #[test]
    fn the_serving_cap_reader_tracks_the_file_it_reads() {
        assert_eq!(serving_max_body_bytes(), 64 * 1024, "today's value");

        // A stand-in file with a different value, parsed by the same two lines.
        fn parse(src: &str) -> u64 {
            let needle = "pub const SERVING_MAX_BODY_BYTES: u64 = ";
            let at = src.find(needle).expect("needle");
            let expr: String = src[at + needle.len()..]
                .chars()
                .take_while(|c| *c != ';')
                .collect();
            expr.split('*')
                .map(|p| p.trim().replace('_', "").parse::<u64>().expect("number"))
                .product()
        }
        assert_eq!(
            parse("pub const SERVING_MAX_BODY_BYTES: u64 = 1024 * 1024;"),
            1024 * 1024
        );
        assert_eq!(parse("pub const SERVING_MAX_BODY_BYTES: u64 = 4096;"), 4096);
    }

    /// A header whose value is not valid UTF-8 is dropped, not mangled.    /// A header whose value is not valid UTF-8 is dropped, not mangled.
    ///
    /// The claim existed in `collect_headers`' doc from the first commit and
    /// **had no test** — fix round 1's sweep. It would survive the
    /// implementation being `filter_map(|(k, v)| Some((k, lossy(v))))`.
    #[test]
    fn a_non_utf8_header_is_dropped_rather_than_mangled() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "x-binary",
            axum::http::HeaderValue::from_bytes(&[0xff, 0xfe]).expect("a header value of bytes"),
        );
        let got = collect_headers(&headers);
        assert!(
            got.iter()
                .any(|(k, v)| k == "content-type" && v == "application/json"),
            "the control: a valid header IS kept"
        );
        assert!(
            !got.iter().any(|(k, _)| k == "x-binary"),
            "a non-UTF-8 value must be dropped, not lossily decoded: {got:?}"
        );
    }

    /// **Important 4's regression test.** `%` followed by bytes that are not a
    /// char boundary must not panic.
    ///
    /// The old body sliced `&s[i + 1..i + 3]`, which panics whenever those
    /// offsets fall inside a multi-byte character. On an anonymous endpoint
    /// that unwinds the connection task, so it is a denial of service reachable
    /// with one query string.
    #[test]
    fn a_percent_escape_across_a_char_boundary_does_not_panic() {
        // The exact shape the review reported.
        assert_eq!(parse_query(Some("a=%1é")).len(), 1);
        // And a sweep of the neighbourhood, every one of which used to be a
        // slice across a boundary.
        for raw in [
            "a=%1é",
            "a=%éé",
            "a=é%1",
            "a=%1\u{1F600}",
            "a=%\u{1F600}1",
            "a=%",
            "a=%1",
            "a=%%",
            "a=%zz",
            "k%é=v",
        ] {
            let out = parse_query(Some(raw));
            assert!(!out.is_empty(), "{raw} produced nothing");
        }
        // The control: decoding still works, so the assertions above are not
        // satisfied by a function that gave up on `%`.
        assert_eq!(parse_query(Some("a=%20b")), vec![("a".into(), " b".into())]);
    }

    #[test]
    fn percent_and_plus_decoding() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("100%25"), "100%");
        assert_eq!(percent_decode("%E2%9C%93"), "\u{2713}");
        assert_eq!(percent_decode("a%"), "a%", "a trailing stray % is literal");
        assert_eq!(percent_decode("a%zz"), "a%zz", "a bad escape is literal");
        // The control: decoding must actually be happening, or every assertion
        // above is satisfied by returning the input.
        assert_ne!(percent_decode("a%20b"), "a%20b");
    }

    /// The attempt window admits exactly [`WEBHOOK_ATTEMPTS_PER_WINDOW`] and
    /// then refuses, per key, and the clock is a parameter rather than a sleep.
    #[test]
    fn the_attempt_window_admits_its_quota_and_then_refuses() {
        let g = WebhookGuard::new(1);
        let key = || ("ws".to_string(), "ns".to_string(), "flow".to_string());
        let t0 = Instant::now();

        for i in 1..=WEBHOOK_ATTEMPTS_PER_WINDOW {
            assert!(g.admit_attempt(key(), t0), "attempt {i} of the quota");
        }
        assert!(
            !g.admit_attempt(key(), t0),
            "the one past the quota must be refused"
        );

        // A DIFFERENT target has its own budget -- without this row the
        // assertion above is satisfied by a limiter that refuses everything
        // after N globally.
        assert!(g.admit_attempt(("ws".into(), "ns".into(), "other-flow".into()), t0));
        assert!(g.admit_attempt(("other-ws".into(), "ns".into(), "flow".into()), t0));

        // The window rolls.
        assert!(
            g.admit_attempt(key(), t0 + WEBHOOK_ATTEMPT_WINDOW),
            "a new window starts fresh"
        );
        // And not a moment early: at the last instant INSIDE the window the
        // key is still exhausted, which is what makes the row above about the
        // window rather than about any elapsed time at all.
        let g2 = WebhookGuard::new(1);
        for _ in 0..WEBHOOK_ATTEMPTS_PER_WINDOW {
            assert!(g2.admit_attempt(key(), t0));
        }
        assert!(!g2.admit_attempt(
            key(),
            t0 + WEBHOOK_ATTEMPT_WINDOW - Duration::from_millis(1)
        ));
    }

    /// **Minor 1's discriminator.** The `count == 0` arm of the reset decides
    /// *when a fresh slot's window starts*, and that is observable.
    ///
    /// Build at `t0`; first use at `t0 + 59s`; ask again at `t0 + 61s`.
    ///
    /// * with the arm — the window began at `t0 + 59s`, two seconds ago, and
    ///   the caller is **refused**;
    /// * without it — the first use did not reset, so the window began at the
    ///   guard's construction, has expired, and the caller is **admitted**.
    ///
    /// Mutation `R-I` (delete `window.1 == 0 ||`) survived 741 green tests
    /// before this existed.
    #[test]
    fn an_unused_slot_starts_its_window_when_it_is_first_used() {
        let g = WebhookGuard::new(1);
        let key = || ("ws".to_string(), "ns".to_string(), "flow".to_string());
        // `new()` stamps the slots with its own `Instant::now()`, so `t0` has
        // to be that instant rather than a later one -- read it back off an
        // untouched slot so the fixture cannot drift from the constructor.
        let t0 = g.attempts.lock().unwrap()[0].0;

        let first_use = t0 + Duration::from_secs(59);
        for i in 1..=WEBHOOK_ATTEMPTS_PER_WINDOW {
            assert!(g.admit_attempt(key(), first_use), "attempt {i}");
        }
        assert!(
            !g.admit_attempt(key(), first_use),
            "the quota is spent at the instant it was spent"
        );

        // t0 + 61s: 61 seconds after CONSTRUCTION, 2 seconds after first use.
        assert!(
            !g.admit_attempt(key(), t0 + Duration::from_secs(61)),
            "the window must be measured from FIRST USE, not from construction -- \
             admitting here means a target gets a fresh budget 2 seconds into its window"
        );
        // The control: 60 seconds after first use it really does reset, so the
        // assertion above is not satisfied by a window that never expires.
        assert!(
            g.admit_attempt(key(), first_use + WEBHOOK_ATTEMPT_WINDOW),
            "and it does expire a full window after first use"
        );
    }

    /// A refused attempt is still COUNTED. That is the whole point: a key guess
    /// is a refusal, so a limiter that only counted successes would bound
    /// nothing.
    ///
    /// Asserted at the guard rather than through the route, because the route
    /// cannot distinguish "counted" from "not counted" until the quota is
    /// reached, and reaching it through HTTP costs 60 requests.
    #[test]
    fn the_limiter_counts_the_attempt_not_the_outcome() {
        let g = WebhookGuard::new(1);
        let key = || ("ws".to_string(), "ns".to_string(), "flow".to_string());
        let t0 = Instant::now();
        // `admit_attempt` has no outcome parameter at all, which is the
        // strongest form of this: there is nothing it could branch on.
        for _ in 0..WEBHOOK_ATTEMPTS_PER_WINDOW {
            g.admit_attempt(key(), t0);
        }
        assert!(!g.admit_attempt(key(), t0));
    }

    /// **Important 2's regression test.** The window state is a CAP, so an
    /// anonymous caller cannot make this process allocate by naming targets
    /// that do not exist.
    ///
    /// The previous version swept a `HashMap` at 4096 entries, which is a
    /// trigger and not a bound: 65,536 attacker-chosen URLs inside one window
    /// held 65,536 entries and made every admission walk all of them under the
    /// mutex. This drives **a quarter of a million distinct keys** — sixty
    /// times the old sweep point — and asserts the allocation does not move.
    #[test]
    fn the_window_state_is_capped_not_swept() {
        let g = WebhookGuard::new(1);
        let before = g.window_bytes();
        assert_eq!(
            before,
            WEBHOOK_ATTEMPT_SLOTS * std::mem::size_of::<AttemptWindow>()
        );
        let t0 = Instant::now();
        for i in 0..250_000 {
            g.admit_attempt(
                (format!("ws-{i}"), format!("ns-{i}"), format!("flow-{i}")),
                t0,
            );
        }
        assert_eq!(
            g.window_bytes(),
            before,
            "a quarter of a million attacker-chosen keys moved the allocation"
        );
    }

    /// The slot mask is exact only if the count is a power of two.
    #[test]
    fn the_slot_count_is_a_power_of_two() {
        assert!(WEBHOOK_ATTEMPT_SLOTS.is_power_of_two());
        // And every slot is reachable, so the mask is not silently folding the
        // whole key space into a corner of the array.
        let g = WebhookGuard::new(1);
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..(WEBHOOK_ATTEMPT_SLOTS * 20) {
            seen.insert(g.slot(&(format!("w{i}"), "n".into(), "f".into())));
        }
        assert!(
            seen.len() > WEBHOOK_ATTEMPT_SLOTS * 9 / 10,
            "only {} of {WEBHOOK_ATTEMPT_SLOTS} slots were reachable",
            seen.len()
        );
    }

    /// The key→slot mapping differs between guards, so an attacker cannot
    /// choose which legitimate target it collides with.
    ///
    /// Two guards, the same thousand keys: if the mapping were derivable the
    /// two would agree on every one of them.
    #[test]
    fn the_slot_mapping_is_not_derivable_from_outside() {
        let a = WebhookGuard::new(1);
        let b = WebhookGuard::new(1);
        let keys: Vec<AttemptKey> = (0..1000)
            .map(|i| (format!("ws{i}"), "ns".to_string(), "flow".to_string()))
            .collect();
        let agree = keys.iter().filter(|k| a.slot(k) == b.slot(k)).count();
        // Chance agreement is ~1/4096 per key, so ~0 of 1000. Anything near
        // 1000 means the hasher is deterministic across guards.
        assert!(
            agree < 50,
            "{agree} of 1000 keys landed in the same slot in two guards -- the mapping is \
             derivable, so a collision can be aimed"
        );
    }

    /// The bulkhead holds its permit for the span of a request and releases it    /// The bulkhead holds its permit for the span of a request and releases it
    /// on drop.
    #[test]
    fn the_bulkhead_refuses_past_its_permits() {
        let g = WebhookGuard::new(2);
        let a = g.bulkhead.try_acquire().expect("first permit");
        let _b = g.bulkhead.try_acquire().expect("second permit");
        assert!(
            g.bulkhead.try_acquire().is_err(),
            "a third must be refused, not queued"
        );
        drop(a);
        assert!(
            g.bulkhead.try_acquire().is_ok(),
            "and the permit comes back when the request ends"
        );
    }

    /// The three absences share one message, and this is the assertion that
    /// they cannot drift apart -- there is only one constructor.
    #[test]
    fn there_is_exactly_one_not_found_message() {
        assert_eq!(not_found().to_string(), "not found: webhook");
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webhook_routes.rs"),
        )
        .expect("read own source");
        let shipping = &source[..source
            .find("\n#[cfg(test)]")
            .expect("this file has a test module")];
        assert_eq!(
            shipping.matches("Error::NotFound(").count(),
            1,
            "every 404 on this surface must go through not_found(), so that the three \
             absences stay byte-identical"
        );
        // Counted rather than guessed. The needle is the bare identifier,
        // because one of the four call sites passes the function rather than
        // calling it (`.ok_or_else(not_found)`) and a `not_found()` needle
        // silently misses it -- the same spelling trap that made this file's
        // sibling assertion in `trigger/webhook.rs` say "three" for two.
        assert_eq!(
            shipping.matches("not_found").count(),
            5,
            "the definition plus its FOUR call sites: no workspace, no flow, unparseable \
             flow, no matching key"
        );
    }
}
