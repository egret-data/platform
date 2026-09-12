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
//! **The parameter is spelled `{ws}` and not `{workspace}`, and that is a
//! convention rather than a shortening.** `egret-server`'s
//! `feature_route_coverage` suite reads `{workspace}` as the marker of a route
//! the `WsContext` feature gate is expected to cover, and this surface is
//! deliberately outside that gate — it has no session for the gate to resolve.
//! `egret-dataapi`'s serving router carries the identical spelling for the
//! identical reason, and `egret-workspace`'s `features.rs` records both
//! exemptions beside the module each belongs to. **Turning the `workflows`
//! module off does not stop webhook traffic**; `state = 'disabled'` on the flow
//! does, which is the same answer the Data API gives for its own serving path.
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
//!    **`FlowRepo::find_by_name` must not be used here**, and its own doc says
//!    why: it is `WHERE namespace = $1 AND flow_id = $2 ORDER BY id LIMIT 1`,
//!    with no workspace predicate at all, because `WorkflowRefPort` has no
//!    caller-workspace parameter. `worker::subflow::resolve` already refused it
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
//! | trigger declares something egret cannot do | n/a | **409**, naming it |
//! | trigger `inputs:` will not render | **422** | **400** (`InvalidInput`) |
//!
//! **The first three rows are one byte-identical response**, and that is the
//! divergence. Kestra distinguishes "no such flow" from "no such webhook" on a
//! surface where the caller has presented no credential at all, which makes the
//! endpoint an oracle for *does workspace W, namespace N hold a flow called F*.
//! Anyone can enumerate it. Collapsing them costs an operator one diagnostic
//! and removes the enumeration; `the_three_absences_are_indistinguishable` is
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
//! about which version of the world they are in. See that function's doc for
//! the measured figure behind reason 2 (16 of 345 corpus flows reach a live
//! version).
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

use axum::body::Bytes;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    build_output, find, render_inputs, ExecutionTrigger, InputContext, KeyContext, Unsupported,
    WebhookRequest,
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
/// The semaphore is built **once per router**, in [`webhook_router`]. A caller
/// that builds a router per request gets a fresh semaphore per request, which
/// is a limit nobody can reach — the mistake `egret-dataapi`'s
/// `serving_bulkhead` doc warns about, repeated here because this module has
/// its own constructor.
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

/// The two in-process bounds, built once per router.
///
/// **Fixed window, not a token bucket**, and the difference is admitted rather
/// than hidden: a fixed window lets 2x the rate through across a boundary. A
/// bucket needs either a shared store (which this crate does not depend on) or
/// per-key state with eviction; the window's state is one entry per
/// `(workspace, namespace, flow)` that has been called, swept whenever the map
/// grows past [`Self::SWEEP_AT`].
#[derive(Debug)]
pub struct WebhookGuard {
    bulkhead: Semaphore,
    attempts: Mutex<HashMap<(String, String, String), (Instant, u32)>>,
}

impl WebhookGuard {
    /// Sweep expired windows when the map reaches this size.
    ///
    /// Without it the map is an unbounded allocation an anonymous caller
    /// controls the key of: every distinct `(ws, namespace, flow)` in a URL
    /// makes an entry, and none of the three has to name anything real.
    const SWEEP_AT: usize = 4096;

    #[must_use]
    pub fn new(permits: usize) -> Self {
        Self {
            bulkhead: Semaphore::new(permits),
            attempts: Mutex::new(HashMap::new()),
        }
    }

    /// Record one attempt against `key`. `false` means the window is exhausted.
    ///
    /// `now` is a parameter so the test drives it rather than sleeping — this
    /// workspace has lost whole gate runs to wall-clock assertions.
    pub fn admit_attempt(&self, key: (String, String, String), now: Instant) -> bool {
        let mut map = self.attempts.lock().expect("webhook attempt map poisoned");
        if map.len() >= Self::SWEEP_AT {
            map.retain(|_, (started, _)| now.duration_since(*started) < WEBHOOK_ATTEMPT_WINDOW);
        }
        let entry = map.entry(key).or_insert((now, 0));
        if now.duration_since(entry.0) >= WEBHOOK_ATTEMPT_WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= WEBHOOK_ATTEMPTS_PER_WINDOW
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
/// (`system.from`/`system.correlationId` are Kestra's execution-label scheme,
/// which egret does not have) and `url` (needs the configured public base URL,
/// which this crate does not read).
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
    // Not `FlowRepo::find_by_name` -- see the module doc. `deleted_at IS NULL`
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
    // **Both ways a flow can be switched off, and the YAML one was missing.**
    //
    // Fix round 1, Critical 1. egret has two: the `state` column, which the
    // management UI writes, and the YAML's own `disabled: true`, which is
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
    for unsupported in [
        webhook.wait.then_some(Unsupported::Wait),
        webhook.return_outputs.then_some(Unsupported::ReturnOutputs),
        webhook
            .response_content_type
            .is_some()
            .then_some(Unsupported::ResponseContentType),
        webhook
            .response_code
            .is_some()
            .then_some(Unsupported::ResponseCode),
    ]
    .into_iter()
    .flatten()
    {
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
fn parse_query(raw: Option<&str>) -> Vec<(String, String)> {
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
    #[test]
    fn the_body_cap_is_this_number_and_not_another() {
        assert_eq!(MAX_WEBHOOK_BODY_BYTES, 256 * 1024);
        // And the comparison the doc makes, asserted rather than remembered --
        // the first version of that doc got this figure wrong by 16x.
        assert_eq!(
            MAX_WEBHOOK_BODY_BYTES as u64,
            4 * egret_dataapi_guard_cap(),
            "the doc says this seam is four times `SERVING_MAX_BODY_BYTES`; if that constant \
             moves, this is what says so"
        );
    }

    /// `egret-dataapi`'s `SERVING_MAX_BODY_BYTES`, restated so the assertion
    /// above compares to something real.
    ///
    /// It is a literal rather than a dependency edge on purpose: this crate must
    /// not depend on `egret-dataapi` (the whole point of the port-trait
    /// convention), so the figure is copied and the copy is dated. Read off
    /// `crates/egret-dataapi/src/serving/guard.rs:214` on 2026-09-12:
    /// `pub const SERVING_MAX_BODY_BYTES: u64 = 64 * 1024;`
    fn egret_dataapi_guard_cap() -> u64 {
        64 * 1024
    }

    /// A header whose value is not valid UTF-8 is dropped, not mangled.
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

    /// The map is swept, so an anonymous caller cannot grow it without bound by
    /// naming targets that do not exist.
    #[test]
    fn the_attempt_map_is_swept_rather_than_grown_for_ever() {
        let g = WebhookGuard::new(1);
        let t0 = Instant::now();
        for i in 0..WebhookGuard::SWEEP_AT {
            g.admit_attempt(("ws".into(), "ns".into(), format!("flow-{i}")), t0);
        }
        assert_eq!(
            g.attempts.lock().unwrap().len(),
            WebhookGuard::SWEEP_AT,
            "nothing expires while the clock stands still"
        );
        // One more, a window later: the sweep fires and drops the expired ones.
        g.admit_attempt(
            ("ws".into(), "ns".into(), "late".into()),
            t0 + WEBHOOK_ATTEMPT_WINDOW,
        );
        assert_eq!(
            g.attempts.lock().unwrap().len(),
            1,
            "the sweep must drop every expired window, keeping only the new one"
        );
    }

    /// The bulkhead holds its permit for the span of a request and releases it
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
