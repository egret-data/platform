//! **The assembled system, with nobody pushing it** — publish over HTTP, start
//! over HTTP, and a background runtime that finishes the work while this test
//! does nothing but read.
//!
//! # Why this file exists at all
//!
//! Until W10 nothing in this repository had ever run a workflow without a test
//! driving the tick. `oracle-exec/driver.rs` calls
//! [`worker::tick`](egret_workflow::worker::tick) itself with its own registry;
//! `tests/worker.rs` and `tests/executor.rs` call the executor; even
//! `tests/runtime.rs`, which does start the real tickers, seeds its executions
//! with a direct `start_execution` and answers them from a `Recorder` written
//! in that file. Every one of those instruments is blind to the same thing: the
//! **wiring** — a flow authored through the management API, a real handler
//! registry, a loop nobody calls, and a read surface that can see the result.
//!
//! **So the single property this file exists for is that `tick` appears nowhere
//! in it.** Grep for it: there is no call. The only thing that advances the
//! execution is [`runtime::spawn`](egret_workflow::runtime::spawn), and
//! [`with_the_worker_disabled_the_execution_stays_where_the_api_left_it`] is
//! what makes that claim falsifiable rather than merely asserted — with the one
//! config flag flipped, the identical fixture goes nowhere.
//!
//! # `#[sqlx::test]` — a scratch database per test, and it is not a preference
//!
//! [`worker::claim`](egret_workflow::worker::claim) and
//! [`worker::due_retries`](egret_workflow::worker::due_retries) select due work
//! **globally**: no workspace predicate, no execution predicate, because one
//! worker serves every workspace and that is the whole point of `SKIP LOCKED`.
//! On the shared `DATABASE_URL` database a ticker belonging to a *neighbouring
//! test binary* would happily claim this file's runs — and the damage is not to
//! the headline test, which would still pass, but to the **control**: an
//! execution that the disabled runtime left alone would be advanced by
//! somebody else's worker, and the control would fail; or, worse, the headline
//! would pass on a database where this test's own runtime never did anything.
//! A scratch database per test is the only arrangement in which "it terminated"
//! and "it did not" both mean what they say. `tests/runtime.rs` and
//! `tests/worker.rs` are on scratch databases for the same statement's sake,
//! and W10 Task 5 put its one worker-driven test there too.
//!
//! `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy **unconditionally**,
//! so an unexported variable silently falls back to the **dev** database
//! instead of skipping; every test here calls
//! `egret_test_db::assert_database_url_is_exported()` first, which is that
//! crate's guard for exactly this.
//!
//! # Every wait is on STATE, and every deadline names what was missing
//!
//! The runtime is a clock, and this branch has been bitten by wall-clock
//! assertions before. Nothing here asserts on elapsed time, in either
//! direction: [`wait_for_terminal`] polls the **read API** for the execution's
//! state and its deadline is a budget, not a measurement. When it runs out it
//! reports the state the execution was actually in and the state of every run —
//! because Step 4's mutation (delete the `spawn` from the setup) has to fail
//! *naming the non-terminal state*, and because in six months that message is
//! the only thing the next reader gets.
//!
//! # What this test found on its first run, and what was done about it
//!
//! The brief for this file says *publish through the management API, then
//! start*. **On the first run it did not work**, and the reason was not this
//! test's: `POST .../flows/{id}/publish` answered **400 — `task 'greet' has
//! unknown type 'io.kestra.plugin.core.log.Log'`** for the only flow the
//! shipped worker can execute.
//!
//! `spec::registry::TASK_TYPES` — publish's table — spelled egret's tasks
//! `core.log`, `egret.sql`, `core.http` (the spellings of that time; `core.log`
//! and `core.http` are gone since, the latter in package #4 Task C6);
//! `Handlers::with_core_tasks(egret_workflow::storage::current())`, which
//! is what `egret-bin` hands the runtime, registers Kestra's FQCNs, because
//! that is what `executor::build_plan` copies verbatim out of `Task::type`.
//! **The intersection was empty for every core task**, and worse: each of the
//! four handler `TYPE_ID`s appeared in that table exactly **zero** times, so
//! **no Kestra flow pasted into egret could be published at all**. Nothing had
//! ever driven both halves in one process, so nothing had ever had to care.
//!
//! **It was not an open question.** Programme spec §8 item 4 had already
//! decided it: *"`type:` accepts Kestra's full FQCN as the canonical form
//! (`io.kestra.plugin.core.log.Log`); short aliases are secondary. A Kestra flow
//! pasted into egret runs, and looking a task up in Kestra's docs lands on the
//! right page."* `TASK_TYPES` was the side that was wrong. Fix round 1 moved
//! it: the four FQCNs are canonical rows, `core.log` is an **alias** on the
//! `Log` row, and `Handlers::add_publishable` takes the alias list **from that
//! table** and panics on a handler whose `type_id()` has no row — so the two
//! vocabularies cannot drift without `egret-bin` refusing to start.
//!
//! So this file now goes **through** publish rather than around it, which is
//! the difference between *"the system works"* and *"the system works if you
//! skip the step that refuses it"*.
//! [`both_spellings_publish_and_both_dispatch`] keeps both directions pinned.
//!
//! **One thing to keep straight while reading the headline:** a published flow
//! is not what makes a start possible. `create_flow` answers 201 with a
//! validation report and **publish is the gate that used to refuse**, while
//! `start_flow_execution` runs the **draft's** source either way — measured and
//! pinned in `tests/executions_routes.rs`. Before the fix this file still
//! reached a terminal state, *from the draft*, with the publish step refused;
//! after it, the same bytes are also a live version. The assertions below say
//! which is which rather than leaving it to be inferred.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-workflow --test end_to_end

use std::sync::Arc;
use std::time::Duration;

use egret_metastore::Db;
use egret_workflow::runtime::{spawn, RuntimeConfig};
use egret_workflow::worker::Handlers;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod common;

// ── The flow, and why it is this flow ───────────────────────────────────────

/// **Two tasks, in order, and each one is load-bearing.**
///
/// * It **terminates with no external input**: `core.log.Log` writes a line and
///   `core.debug.Return` renders a string. Neither waits for anything — no
///   `Pause`, no `retry:`, no clock to run down, nothing for a human to resume.
///   The only reason this execution would sit still is that nothing is driving
///   it, which is precisely the difference the control has to be able to see.
/// * It needs **more than one tick**. The tasks are sequential, so the first
///   claim can only take `greet`; `answer` does not become runnable until that
///   settle commits, and only the *next* tick claims it. A loop that ticked
///   once and stopped would leave this execution non-terminal — which a
///   single-task flow could not tell apart from a healthy one.
/// * `answer` has an **output nothing else in this file could produce**.
///   `Return` is the only handler of the four in
///   [`Handlers::with_core_tasks`] that emits one, and the distinctive literal
///   below is asserted on: it is the evidence that the *real* registry ran the
///   task, not a double, not a fixture, and not a seeded row.
fn two_task_source(id: &str) -> String {
    format!(
        "id: {id}\n\
         namespace: a.e2e\n\
         tasks:\n\
         \x20 - id: greet\n\
         \x20   type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hello from a loop nobody called\n\
         \x20 - id: answer\n\
         \x20   type: io.kestra.plugin.core.debug.Return\n\
         \x20   format: {RETURN_VALUE}\n"
    )
}

/// The string only `core.debug.Return` can put in `outputs.value`.
const RETURN_VALUE: &str = "answered-by-the-real-registry";

/// The five states this engine calls terminal — `worker::advance` produces no
/// further commands from any of them. Same list `tests/runtime.rs` uses.
fn is_terminal(state: &str) -> bool {
    matches!(
        state,
        "SUCCESS" | "FAILED" | "WARNING" | "KILLED" | "CANCELLED"
    )
}

/// A runtime config that ticks fast, with the reaper parked an hour out so no
/// test here ever sees a lease reclaimed. `enabled` is the caller's, because it
/// is the one bit the control changes.
fn cfg(enabled: bool) -> RuntimeConfig {
    RuntimeConfig {
        enabled,
        tick_interval: Duration::from_millis(50),
        batch: 8,
        max_concurrent_runs: 8,
        reaper_interval: Duration::from_secs(3600),
        working_timeout: Duration::from_secs(3600),
        container_default_timeout: egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT,
        kill_poll: egret_workflow::worker::DEFAULT_KILL_POLL,
        scheduler_enabled: false,
        trigger_interval: Duration::from_secs(3600),
        log_limits: egret_workflow::logs::LogLimits::default(),
        log_retention_days: 30,
    }
}

// ── The setup, entirely over HTTP ───────────────────────────────────────────

/// Everything up to and including the start, through the management API and
/// nothing else: create the draft, publish it, start an execution. Returns the
/// app, the flow id, the execution id, and the execution DTO the start answered
/// with.
///
/// **The publish route is driven and its SUCCESS is asserted**, with the reason
/// spelled out at the call: it answered 400 until fix round 1, and a 400 here
/// again means the publish table and the worker registry have drifted apart
/// once more. This is the only place in the repository where a flow goes
/// through publish and then through the worker in one process.
///
/// **The start's own 201 body is returned rather than discarded** because one
/// assertion depends on it: `start_flow_execution` drives `advance()` to a
/// fixed point *inside its own transaction*, so a flow with nothing runnable is
/// already terminal by the time the route answers. Reading that body is how
/// each test below proves the execution was **not** terminal before any worker
/// existed — without it, "the execution is SUCCESS" would be satisfied by a
/// start that had finished the whole thing itself.
async fn publish_and_start(
    db: &Db,
    ws: Uuid,
    user: egret_core::UserId,
    flow_name: &str,
) -> (common::App, Uuid, Uuid, Value) {
    let app = common::app(db).await;

    // 1. Author the flow — the management API's create route, with the YAML in
    //    the body exactly as the editor sends it.
    let created = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": two_task_source(flow_name) }),
    )
    .await;
    assert_eq!(created.status(), 201, "create the draft");
    let created = common::json(created).await;
    let flow: Uuid = created["id"].as_str().unwrap().parse().unwrap();
    // **`create` validates and finds nothing wrong.** The type table publish
    // consults is not consulted here, which is why the divergence below is
    // invisible until the publish route is actually called.
    assert_eq!(
        created["validation"]["errors"],
        serde_json::json!([]),
        "the flow this suite runs must validate clean: {created}"
    );

    // 2. Publish it. **This answered 400 until fix round 1** — see this
    //    module's doc — so the status is asserted with the reason it matters,
    //    not as a formality.
    let published = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/publish"),
        user,
        serde_json::json!({ "note": "end-to-end" }),
    )
    .await;
    let status = published.status();
    let body = common::body_text(published).await;
    assert_eq!(
        status, 200,
        "a flow spelled the way the shipped worker dispatches it must publish; \
         a 400 naming an `unknown type` here is the TASK_TYPES/registry split \
         reopening: {body}"
    );
    let published: Value = serde_json::from_str(&body).expect("version dto");
    assert_eq!(published["version"], 1, "{published}");

    // The flow is live, and the version it points at is the one just cut.
    let after = common::json(
        common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{flow}"), user).await,
    )
    .await;
    assert_eq!(after["state"], "live", "{after}");
    assert_eq!(after["live_version_id"], published["id"], "{after}");

    // **The start route runs the DRAFT's source, not this version** — measured
    // and pinned in `tests/executions_routes.rs`. Here the two are the same
    // bytes because nothing edits the draft in between, and saying so is the
    // point: a reader who assumes the published version is what runs would be
    // wrong about this engine, just not about this test.

    // 3. Start an execution — `POST .../flows/{id}/executions`, no body, which
    //    is what "run this flow" is for a flow with no `inputs:`.
    let started = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
        user,
    )
    .await;
    assert_eq!(started.status(), 201, "start an execution");
    let dto = common::json(started).await;
    let execution: Uuid = dto["id"].as_str().unwrap().parse().unwrap();
    (app, flow, execution, dto)
}

/// `GET .../executions/{id}`, decoded — the read API, the same one a browser
/// polls.
async fn read(app: &common::App, ws: Uuid, user: egret_core::UserId, execution: Uuid) -> Value {
    common::json(
        common::get(
            app,
            &format!("/api/ws/{ws}/workflows/executions/{execution}"),
            user,
        )
        .await,
    )
    .await
}

/// `task_id=STATE` for every run in a DTO, for a failure message that says
/// where the execution stopped rather than merely that it did.
fn run_states(dto: &Value) -> String {
    match dto["runs"].as_array() {
        None => "(no runs array)".to_string(),
        Some(runs) if runs.is_empty() => "(no runs)".to_string(),
        Some(runs) => runs
            .iter()
            .map(|r| {
                format!(
                    "{}={}",
                    r["task_id"].as_str().unwrap_or("?"),
                    r["state"].as_str().unwrap_or("?")
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Poll the **read API** until the execution is terminal, or run out of budget.
///
/// `Ok(dto)` is the terminal execution. `Err(why)` is a sentence naming the
/// state it was actually in and the state of every run — which is what Step 4's
/// mutation must produce, and what the next person to break this gets.
///
/// The deadline is a **budget, not a measurement**: nothing here asserts that
/// the work took any particular time, only that the state did or did not
/// change. `Instant::elapsed` appears once, to decide when to give up.
async fn wait_for_terminal(
    app: &common::App,
    ws: Uuid,
    user: egret_core::UserId,
    execution: Uuid,
    budget: Duration,
) -> Result<Value, String> {
    let start = std::time::Instant::now();
    loop {
        let dto = read(app, ws, user, execution).await;
        let state = dto["state"].as_str().unwrap_or("(absent)").to_string();
        if is_terminal(&state) {
            return Ok(dto);
        }
        if start.elapsed() >= budget {
            return Err(format!(
                "execution {execution} is still in non-terminal state {state:?} after {budget:?}; \
                 runs: [{}]",
                run_states(&dto)
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ── The headline ────────────────────────────────────────────────────────────

/// **Publish → start → SUCCESS, with no test driving the tick.**
///
/// The runtime is started **after** the start deliberately, so that the 201's
/// own body is an uncontested reading: at that instant no ticker exists, and
/// the execution is demonstrably non-terminal. Everything that happens after is
/// the loop's doing. (Starting the tickers first would work too, but then the
/// "it was RUNNING when the API answered" assertion would race a tick and this
/// file's whole purpose is to not be flaky.)
///
/// Four assertions, because the terminal state alone is satisfiable by things
/// that are not "the system ran the flow":
///
/// 1. the execution was **non-terminal** when the start route answered, so the
///    terminal state below came from somewhere else;
/// 2. it reaches **SUCCESS** — polled on state, never on elapsed time;
/// 3. **both** runs are `SUCCESS`, so the second task really was claimed by a
///    *later* tick and not skipped;
/// 4. `answer`'s output carries [`RETURN_VALUE`], which only the real
///    `core.debug.Return` from [`Handlers::with_core_tasks`] can write.
#[sqlx::test(migrations = "../../migrations")]
async fn a_published_flow_runs_to_success_with_nobody_driving_the_tick(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    // Granted BEFORE the app is built: `CedarPdp::build` loads the policy set
    // once, so a grant written afterwards is invisible to that PDP.
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let (app, _flow, execution, started) = publish_and_start(&db, ws, user, "e2e-live").await;

    let at_start = started["state"].as_str().expect("the start's own state");
    assert!(
        !is_terminal(at_start),
        "the start route must not have finished the flow by itself, or nothing \
         below would be evidence of a worker: {started}"
    );

    // **The only thing in this file that can advance an execution.** There is
    // no call to `worker::tick` anywhere in this test — deleting this line is
    // Step 4's mutation, and it must fail naming the non-terminal state.
    let cancel = CancellationToken::new();
    spawn(
        pool.clone(),
        Arc::new(Handlers::with_core_tasks(egret_workflow::storage::current())),
        cfg(true),
        cancel.clone(),
    );

    // 60s is headroom, not an expectation: two ticks at 50 ms is the work.
    let dto = wait_for_terminal(&app, ws, user, execution, Duration::from_secs(60))
        .await
        .unwrap_or_else(|why| {
            panic!(
                "the runtime was started and nothing advanced the execution — {why}. \
                 That is what a missing `runtime::spawn`, a loop that never ticks, or a \
                 registry missing the flow's handlers looks like from outside."
            )
        });
    cancel.cancel();

    assert_eq!(
        dto["state"], "SUCCESS",
        "both tasks succeed, so the execution does: {dto}"
    );

    let runs = dto["runs"].as_array().expect("runs");
    assert_eq!(
        runs.len(),
        2,
        "one run per task, in the executor's creation order: {}",
        run_states(&dto)
    );
    assert_eq!(
        runs.iter()
            .map(|r| r["task_id"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>(),
        vec!["greet", "answer"],
        "{}",
        run_states(&dto)
    );
    // Both, not just the last: the second task is the one that proves the loop
    // ticked again after the first settle committed.
    for run in runs {
        assert_eq!(
            run["state"],
            "SUCCESS",
            "every run terminated successfully: [{}]",
            run_states(&dto)
        );
    }

    // The real registry ran the real handler. `Log` has no output at all and
    // `Return` is the only one of the four that has one, so this literal cannot
    // arrive by any other route.
    assert_eq!(
        runs[1]["outputs"]["value"].as_str(),
        Some(RETURN_VALUE),
        "`core.debug.Return` from `Handlers::with_core_tasks(egret_workflow::storage::current())` wrote this, or \
         nothing did: {dto}"
    );
    assert!(
        runs[0]["outputs"]["value"].is_null(),
        "`core.log.Log` has no `value` output; if it has one here, the output \
         above did not come from the handler this test thinks it did: {dto}"
    );
}

// ── The control, which is the harder half ───────────────────────────────────

/// **The same fixture, one flag flipped, and it goes nowhere.**
///
/// Without this test the headline passes on a harness that secretly drives the
/// tick — the `ADDED 0` failure in its purest form: a green result that would
/// be green whether or not the thing under test exists.
///
/// It is the identical setup: same YAML, same publish, same start route, same
/// `spawn` call. The **only** difference is `enabled: false`, which is
/// `egret_config::WorkflowConfig::worker_enabled`'s shipping default and
/// therefore what a deployment that says nothing gets.
///
/// **It carries its own positive control, and that is what makes the timeout
/// mean something.** "The execution never terminated" is also what a flow that
/// *cannot* terminate looks like, or a fixture that was never really started.
/// So after the budget expires, this test starts an **enabled** runtime on the
/// same database and the same execution, and watches it finish. The disabled
/// half is therefore not an absence of evidence: the same rows, seconds later,
/// run to SUCCESS the moment a worker exists. On a shared database this
/// argument would be worthless — a neighbour's ticker could supply the
/// advance — which is the second reason for `#[sqlx::test]`.
#[sqlx::test(migrations = "../../migrations")]
async fn with_the_worker_disabled_the_execution_stays_where_the_api_left_it(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let (app, _flow, execution, started) = publish_and_start(&db, ws, user, "e2e-off").await;
    assert!(
        !is_terminal(started["state"].as_str().expect("state")),
        "{started}"
    );

    let off = CancellationToken::new();
    spawn(
        pool.clone(),
        Arc::new(Handlers::with_core_tasks(egret_workflow::storage::current())),
        cfg(false),
        off.clone(),
    );

    // Five seconds is a hundred tick intervals of the enabled config. The
    // assertion is not about the duration: it is that at the end of it the
    // state has not moved, and the positive control below is what says the
    // duration was never the reason.
    let why = wait_for_terminal(&app, ws, user, execution, Duration::from_secs(5))
        .await
        .expect_err(
            "a disabled worker finished a flow: either `RuntimeConfig::enabled` \
             is not the gate, or something outside this test advanced the \
             execution — which on a scratch database should be impossible",
        );
    assert!(
        why.contains("still in non-terminal state"),
        "the refusal must name the state it stopped in: {why}"
    );
    off.cancel();

    // Asserted on three independent things, because each alone is satisfiable
    // by a different bug: the execution has not moved, the second task was
    // never created (so no claim ran for it), and the first task's lease is
    // still NULL — a claim that ran and then failed to dispatch would have
    // written one.
    let stalled = read(&app, ws, user, execution).await;
    assert!(
        !is_terminal(stalled["state"].as_str().expect("state")),
        "runs: [{}]",
        run_states(&stalled)
    );
    let runs = stalled["runs"].as_array().expect("runs");
    assert_eq!(
        runs.len(),
        1,
        "only the run the start's own settle created: [{}]",
        run_states(&stalled)
    );
    assert_eq!(runs[0]["task_id"], "greet", "{stalled}");
    assert!(
        runs[0]["claimed_at"].is_null(),
        "no claim was ever issued: the lease is what a claim writes, so a NULL \
         one is the only evidence no claim statement ran: {stalled}"
    );
    // **The control for the line above, and it is not optional.** `is_null()`
    // on a key nothing ever populates passes for free: a renamed column, a
    // `#[serde(rename)]`, a DTO that dropped the field — every one of those
    // makes the assertion vacuous while it still reads as a proof that no claim
    // ran. So write a lease by hand and read it back through the SAME route:
    // the field must go non-null. This pins the instrument, not the worker.
    sqlx::query("UPDATE workflow.task_runs SET claimed_at = now() WHERE execution_id = $1")
        .bind(execution)
        .execute(&pool)
        .await
        .expect("write a lease by hand");
    let leased = read(&app, ws, user, execution).await;
    assert!(
        !leased["runs"][0]["claimed_at"].is_null(),
        "`claimed_at` must be able to come back non-null, or the NULL above is \
         a fact about the DTO and not about the worker: {leased}"
    );
    // Put it back before the positive control below: a run that still holds a
    // lease is a run `claim` will not take, and the enabled runtime would then
    // stall for a reason that has nothing to do with the config gate.
    sqlx::query("UPDATE workflow.task_runs SET claimed_at = NULL WHERE execution_id = $1")
        .bind(execution)
        .execute(&pool)
        .await
        .expect("release the hand-written lease");
    assert!(
        runs[0]["outputs"].is_null() || runs[0]["outputs"]["value"].is_null(),
        "no handler ran: {stalled}"
    );

    // ---- the positive control: the same rows, with a worker ----
    let on = CancellationToken::new();
    spawn(
        pool.clone(),
        Arc::new(Handlers::with_core_tasks(egret_workflow::storage::current())),
        cfg(true),
        on.clone(),
    );
    let finished = wait_for_terminal(&app, ws, user, execution, Duration::from_secs(60))
        .await
        .unwrap_or_else(|why| {
            panic!(
                "the control is worthless unless this half finishes: {why}. The \
                 execution above stalled for a reason other than the config gate."
            )
        });
    on.cancel();
    assert_eq!(
        finished["state"], "SUCCESS",
        "flipping one flag is the whole difference between the two halves: {finished}"
    );
    assert_eq!(
        finished["runs"][1]["outputs"]["value"].as_str(),
        Some(RETURN_VALUE),
        "{finished}"
    );
}

// ── The vocabulary, pinned in both directions ───────────────────────────────

/// The canary only `Return` reads (as `format:`) and emits (as `{"value": …}`).
///
/// A `Log` run that comes back carrying an output is a run that reached the
/// wrong handler. (`Log` itself never carries `format:`: Kestra rejects it, and
/// so does publish, spec D1.)
const WRONG_HANDLER_CANARY: &str = "this-came-out-of-Return-not-Log";

/// **Both spellings publish, and both DISPATCH — dispatch measured by what the
/// handler produced, not by a key in a map.**
///
/// This test is the inverse of the one it replaces. Until fix round 1 it
/// measured the split: the same one-task flow spelled `core.log` published
/// **200** and spelled `io.kestra.plugin.core.log.Log` published **400**, while
/// the shipped registry served the FQCN and had never heard of `core.log`.
///
/// Now both halves are asserted for both spellings:
///
/// 1. **publish accepts each** — over HTTP, through the real route;
/// 2. **the shipped registry runs each, and the RIGHT handler runs it** — the
///    flow is started through the API, finished by `runtime::spawn`, and the
///    run's `outputs` read back through the read API.
///
/// # Leg 2 asserted `type_ids()` membership until fix round 2, and a swap walked through it
///
/// The doc said *dispatch* and the assertion said *the map has this key*. Those
/// are different claims, and the review's mutation is what separated them:
/// moving the `core.log` alias from the `Log` row to the `Return` row — so that
/// `core.log` **dispatches to the wrong handler** — killed exactly **one test in
/// 1090**, a unit test, and **this one passed**. A membership check cannot see
/// which handler a key points at; only the handler's own output can.
/// [`WRONG_HANDLER_CANARY`] is that output, and it is asserted **absent** here
/// and **present** by its own control in
/// [`the_canary_is_a_real_discriminator_and_not_a_field_nothing_reads`].
///
/// The **negative control** stays: a plausible neighbouring FQCN that nothing
/// implements must still be refused, or "both spellings publish" is satisfiable
/// by a table that stopped refusing anything.
#[sqlx::test(migrations = "../../migrations")]
async fn both_spellings_publish_and_both_dispatch(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;

    const FQCN: &str = "io.kestra.plugin.core.log.Log";
    const ALIAS: &str = "core.log";
    /// A neighbour in the same package that egret does not implement. Kestra
    /// really has it; egret has no handler and no row, and both ends must say
    /// so — this is the control that keeps "both spellings publish" from being
    /// satisfiable by a table that refuses nothing.
    /// (`core.output.OutputValues` until group C1 Task 2 implemented it, then
    /// `core.http.Request` until package #4 Task C3 did.)
    const UNIMPLEMENTED: &str = "io.kestra.plugin.core.execution.PurgeExecutions";

    // ---- leg 1: publish, over HTTP ----
    let measured = [
        (FQCN, publish_one(&app, ws, user, "fqcn", FQCN).await),
        (ALIAS, publish_one(&app, ws, user, "alias", ALIAS).await),
        (
            UNIMPLEMENTED,
            publish_one(&app, ws, user, "nope", UNIMPLEMENTED).await,
        ),
    ];
    assert_eq!(
        measured,
        [(FQCN, 200u16), (ALIAS, 200u16), (UNIMPLEMENTED, 400u16)],
        "publish accepts the canonical FQCN and its alias, and still refuses a \
         type nothing implements"
    );

    // ---- leg 2: run each spelling and read what the handler produced ----
    for spelling in [FQCN, ALIAS] {
        let run = run_one_task(&pool, &app, ws, user, spelling).await;
        assert_eq!(
            run["state"], "SUCCESS",
            "`{spelling}` must reach a handler and succeed: {run}"
        );
        // **The discrimination, and it is the whole of leg 2.** `Log` produces
        // no output; `Return` produces `{"value": …}` from the `format:` this
        // flow carries. A non-null `outputs` here means the spelling dispatched
        // to the wrong handler, and the value names which.
        assert!(
            run["outputs"].is_null(),
            "`{spelling}` reached a handler that produced an output; \
             `core.log.Log` produces none, so this ran the wrong one: {run}"
        );
        assert_ne!(
            run["outputs"]["value"].as_str(),
            Some(WRONG_HANDLER_CANARY),
            "`{spelling}` dispatched to `core.debug.Return`: {run}"
        );
    }

    // Membership, kept as a secondary line rather than the claim: it says the
    // alias is registered at all, which is necessary and — as the swap proved —
    // nowhere near sufficient.
    let shipped = Handlers::with_core_tasks(egret_workflow::storage::current());
    let type_ids = shipped.type_ids();
    assert!(type_ids.contains(&FQCN), "{type_ids:?}");
    assert!(type_ids.contains(&ALIAS), "{type_ids:?}");
    assert!(
        !type_ids.contains(&UNIMPLEMENTED),
        "a type nothing implements must not appear to be served: {type_ids:?}"
    );
}

/// **The canary's own control: `Return` DOES emit it.**
///
/// Without this, `outputs.is_null()` above is satisfiable by a `format:` field
/// nothing anywhere reads — the assertion would hold for a canary that could
/// never fire, which is a guard that cannot fail. Here the identical `format:`
/// value is put on a task whose type **is** `core.debug.Return`, and the run
/// comes back carrying it.
#[sqlx::test(migrations = "../../migrations")]
async fn the_canary_is_a_real_discriminator_and_not_a_field_nothing_reads(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;

    let run = run_one_task(&pool, &app, ws, user, "io.kestra.plugin.core.debug.Return").await;
    assert_eq!(run["state"], "SUCCESS", "{run}");
    assert_eq!(
        run["outputs"]["value"].as_str(),
        Some(WRONG_HANDLER_CANARY),
        "`core.debug.Return` reads `format:` and emits it; if it did not, the \
         canary in the test above would be a field nothing reads and its \
         absence would prove nothing: {run}"
    );
}

/// The one-task flow both vocabulary tests run: [`WRONG_HANDLER_CANARY`] as
/// `format:` on a `Return`, and a `message:` on every other type.
fn one_task_source(name: &str, type_id: &str) -> String {
    // Each type carries ONLY keys Kestra accepts on it (`Log`: `message`;
    // `Return`: `format`). Before save-time refusal of unknown keys (spec D1)
    // both carried both, which Kestra itself rejects (`Unrecognized field`).
    // The discrimination does not need the cross-key: a `Log` that dispatched
    // to `Return` has no `format` to render, so it fails or emits an output,
    // and either is caught.
    let body = if type_id == "io.kestra.plugin.core.debug.Return" {
        format!("format: {WRONG_HANDLER_CANARY}")
    } else {
        "message: hi".to_string()
    };
    // One flat literal, no `\`-continuations: `cargo fmt` reflowed an earlier
    // draft of this string and baked the source's own indentation into the
    // YAML, so `create` answered 400 from the PARSER -- a refusal that looks
    // exactly like the one these tests are about.
    format!(
        "id: vocab-{name}\nnamespace: a.e2e\ntasks:\n\x20 - id: greet\n\x20   type: {type_id}\n\x20   {body}\n"
    )
}

/// `POST /flows` then `POST /flows/{id}/publish`; returns publish's status.
async fn publish_one(
    app: &common::App,
    ws: Uuid,
    user: egret_core::UserId,
    name: &str,
    type_id: &str,
) -> u16 {
    let res = common::post(
        app,
        &format!("/api/ws/{ws}/workflows/flows"),
        user,
        serde_json::json!({ "source": one_task_source(name, type_id) }),
    )
    .await;
    let status = res.status();
    let body = common::body_text(res).await;
    // **`create` is not the gate and must not become one.** It answers 201 with
    // a validation report for every spelling here, including the type egret
    // does not implement -- publish is where a type is refused. If this panics,
    // the refusal has moved routes and the assertion above is measuring the
    // wrong door.
    assert_eq!(status, 201, "create refused `{type_id}`: {body}");
    let created: Value = serde_json::from_str(&body).expect("flow dto");
    let flow = created["id"].as_str().expect("created id");
    common::post(
        app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/publish"),
        user,
        serde_json::json!({}),
    )
    .await
    .status()
    .as_u16()
}

/// Publish a one-task flow of `type_id`, start it, let the **runtime** finish
/// it, and return the single task run's DTO.
///
/// The flow name is derived from `type_id` so two calls in one test never
/// collide on `(namespace, flowId)`.
async fn run_one_task(
    pool: &sqlx::PgPool,
    app: &common::App,
    ws: Uuid,
    user: egret_core::UserId,
    type_id: &str,
) -> Value {
    let name: String = type_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let status = publish_one(app, ws, user, &name, type_id).await;
    assert_eq!(status, 200, "`{type_id}` must publish before it can be run");

    let flows = common::json(
        common::get(
            app,
            &format!("/api/ws/{ws}/workflows/flows?namespace=a.e2e"),
            user,
        )
        .await,
    )
    .await;
    let flow = flows
        .as_array()
        .expect("flow list")
        .iter()
        .find(|f| f["flow_id"] == format!("vocab-{name}"))
        .unwrap_or_else(|| panic!("the flow just created is in the list: {flows}"))["id"]
        .as_str()
        .expect("flow id")
        .to_string();

    let started = common::post_no_body(
        app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
        user,
    )
    .await;
    assert_eq!(started.status(), 201, "start `{type_id}`");
    let execution: Uuid = common::json(started).await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let cancel = CancellationToken::new();
    spawn(
        pool.clone(),
        Arc::new(Handlers::with_core_tasks(egret_workflow::storage::current())),
        cfg(true),
        cancel.clone(),
    );
    let dto = wait_for_terminal(app, ws, user, execution, Duration::from_secs(60))
        .await
        .unwrap_or_else(|why| panic!("`{type_id}` never finished: {why}"));
    cancel.cancel();
    dto["runs"][0].clone()
}
