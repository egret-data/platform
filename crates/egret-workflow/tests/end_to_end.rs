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
//! # What this test found on its first run: the flow it can PUBLISH is not the
//! # flow it can RUN
//!
//! The brief for this file says *publish through the management API, then
//! start*. It does not work, and the reason is not this test's:
//! `POST .../flows/{id}/publish` answered **400 — `task 'greet' has unknown
//! type 'io.kestra.plugin.core.log.Log'`** for the only flow the shipped
//! worker can execute.
//!
//! `spec::registry::TASK_TYPES` — publish's table — spells egret's tasks
//! `core.log`, `egret.sql`, `core.http`; `Handlers::with_core_tasks()`, which
//! is what `egret-bin` hands to the runtime, registers Kestra's FQCNs, because
//! that is what `executor::build_plan` copies verbatim out of `Task::type`.
//! **The intersection of the two is empty for every core task.** So a flow
//! using `core.log` publishes and then finds no handler, and a flow using the
//! FQCN runs but cannot be published. `worker::registry`'s own doc records this
//! and declines to pick a winner, because *"deciding whether egret's public
//! task vocabulary is Kestra's FQCN, a short alias, or both is a spec-level
//! choice"*. Nothing had ever driven both halves in one process, so nothing had
//! ever had to care.
//!
//! This file does not paper over it and does not pick the winner either:
//! [`author_and_start`] drives the real publish route and **asserts the
//! refusal, naming it**, and
//! [`the_publish_table_and_the_shipped_registry_disagree_about_the_core_types`]
//! pins both directions from the two tables themselves. When the spec answers,
//! those are the two places that change — and the headline below is unaffected,
//! because **publishing is not what makes a flow startable**: `start_flow_execution`
//! runs the **draft's** source, measured and pinned in `tests/executions_routes.rs`.
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
        reaper_interval: Duration::from_secs(3600),
        working_timeout: Duration::from_secs(3600),
    }
}

// ── The setup, entirely over HTTP ───────────────────────────────────────────

/// Everything up to and including the start, through the management API and
/// nothing else: create the draft, drive the publish route, start an execution.
/// Returns the app, the flow id, the execution id, and the execution DTO the
/// start answered with.
///
/// **The publish route is driven and its REFUSAL is asserted** — see this
/// module's doc. It is here rather than dropped because the brief's path runs
/// through it and because a reader who assumes this flow was published would be
/// wrong about egret, not just about this test. It is not a property this file
/// endorses: it is the shape of the divergence, recorded at the one place that
/// exercises both halves.
///
/// **The start's own 201 body is returned rather than discarded** because one
/// assertion depends on it: `start_flow_execution` drives `advance()` to a
/// fixed point *inside its own transaction*, so a flow with nothing runnable is
/// already terminal by the time the route answers. Reading that body is how
/// each test below proves the execution was **not** terminal before any worker
/// existed — without it, "the execution is SUCCESS" would be satisfied by a
/// start that had finished the whole thing itself.
async fn author_and_start(
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

    // 2. Publish it — and be refused, by name.
    let published = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/publish"),
        user,
        serde_json::json!({ "note": "end-to-end" }),
    )
    .await;
    assert_eq!(
        published.status(),
        400,
        "egret cannot publish the flow its own worker can run; if this is now \
         200 the spec has answered the vocabulary question and this file's \
         module doc is the thing to update"
    );
    let refusal = common::body_text(published).await;
    assert!(
        refusal.contains("unknown type") && refusal.contains("io.kestra.plugin.core.log.Log"),
        "the refusal must be the TYPE TABLE's, not some other 400: {refusal}"
    );

    // The flow is therefore still a draft — and that is exactly what the start
    // route runs, so nothing below depends on the refusal above.
    let after = common::json(
        common::get(&app, &format!("/api/ws/{ws}/workflows/flows/{flow}"), user).await,
    )
    .await;
    assert_eq!(after["state"], "draft", "{after}");

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

/// **Author → start → SUCCESS, with no test driving the tick.**
///
/// (Author, not publish: [`author_and_start`] drives the publish route too, and
/// this module's doc records why it is refused and why the start does not need
/// it.)
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
async fn an_authored_flow_runs_to_success_with_nobody_driving_the_tick(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    // Granted BEFORE the app is built: `CedarPdp::build` loads the policy set
    // once, so a grant written afterwards is invisible to that PDP.
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;

    let (app, _flow, execution, started) = author_and_start(&db, ws, user, "e2e-live").await;

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
        Arc::new(Handlers::with_core_tasks()),
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
        "`core.debug.Return` from `Handlers::with_core_tasks()` wrote this, or \
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

    let (app, _flow, execution, started) = author_and_start(&db, ws, user, "e2e-off").await;
    assert!(
        !is_terminal(started["state"].as_str().expect("state")),
        "{started}"
    );

    let off = CancellationToken::new();
    spawn(
        pool.clone(),
        Arc::new(Handlers::with_core_tasks()),
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
    assert!(
        runs[0]["outputs"].is_null() || runs[0]["outputs"]["value"].is_null(),
        "no handler ran: {stalled}"
    );

    // ---- the positive control: the same rows, with a worker ----
    let on = CancellationToken::new();
    spawn(
        pool.clone(),
        Arc::new(Handlers::with_core_tasks()),
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

// ── The divergence this file was the first thing to drive ───────────────────

/// **Both directions of the type-vocabulary split, from the two tables
/// themselves.**
///
/// `author_and_start` asserts one half over HTTP — the FQCN flow the worker can
/// run is unpublishable. That alone would be satisfiable by a publish route
/// that refuses everything, so this test drives the other half: the SAME flow,
/// with the SAME task, spelled `core.log`, publishes cleanly. Two spellings of
/// one task, opposite answers; that is the split, not a broken route.
///
/// Then the registry is asked directly, because the HTTP half cannot say what
/// the worker would do with the flow that *did* publish:
/// `Handlers::with_core_tasks()` — the registry `egret-bin` hands to the
/// runtime — holds the FQCN and has never heard of `core.log`, so the
/// publishable flow is the one that would abort a tick with
/// `WorkerError::NoHandler`.
///
/// **This asserts the state of the tree, not a property to keep.** When the
/// spec decides the vocabulary, this test is where the decision lands: one line
/// in `spec::registry::TASK_TYPES` or one `Handlers::register` alias makes it
/// fail, and that failure is the reminder to publish a flow end to end above.
#[sqlx::test(migrations = "../../migrations")]
async fn the_publish_table_and_the_shipped_registry_disagree_about_the_core_types(
    pool: sqlx::PgPool,
) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app(&db).await;

    // One task, two spellings, one publish route.
    let mut status = Vec::new();
    for (name, type_id) in [
        ("short", "core.log"),
        ("fqcn", "io.kestra.plugin.core.log.Log"),
    ] {
        let source = format!(
            "id: split-{name}\nnamespace: a.e2e\ntasks:\n  \
             - id: greet\n    type: {type_id}\n    message: hi\n"
        );
        let created = common::json(
            common::post(
                &app,
                &format!("/api/ws/{ws}/workflows/flows"),
                user,
                serde_json::json!({ "source": source }),
            )
            .await,
        )
        .await;
        let flow = created["id"].as_str().expect("created id");
        let res = common::post(
            &app,
            &format!("/api/ws/{ws}/workflows/flows/{flow}/publish"),
            user,
            serde_json::json!({}),
        )
        .await;
        status.push((type_id, res.status().as_u16()));
    }
    assert_eq!(
        status,
        vec![
            ("core.log", 200u16),
            ("io.kestra.plugin.core.log.Log", 400u16)
        ],
        "publish accepts the short spelling and refuses the FQCN"
    );

    // And the shipped registry answers the other way round.
    let registered = Handlers::with_core_tasks();
    let type_ids = registered.type_ids();
    assert!(
        type_ids.contains(&"io.kestra.plugin.core.log.Log"),
        "the runtime's registry serves the spelling publish refuses: {type_ids:?}"
    );
    assert!(
        !type_ids.contains(&"core.log"),
        "and has never heard of the one publish accepts, so a published flow \
         aborts its tick with NoHandler: {type_ids:?}"
    );
}
