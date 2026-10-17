# StarRocks timeout docs corrected and `http()` refused: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md`. Read it first; this plan refers to its sections.

**Goal:** every text that says a timed-out StarRocks statement keeps running on the engine states the measured behaviour instead (it is cancelled when egret drops the connection), a live test pins that behaviour, and `http()` is refused with a reason instead of "waiting for W9".

**Architecture:** no connector code changes (spec §1.3, decided with the user after Task 1's measurement): doc and message corrections plus three engine-gated tests in `crates/egret-query/tests/connector_starrocks.rs`. A new `Refusal::Unsupported` variant in the expression layer carries `http`'s permanent refusal.

**Tech Stack:** Rust, `mysql_async`, `minijinja`, StarRocks 4.1.6 (`starrocks/allin1-ubuntu:4.1.6@sha256:5cbf09c4c788d2ca41a8e38599b650f04dbc6af0de51ebda0952770fb8333e5d`, already pulled locally).

**Branch / worktree:** `feat/workflow-starrocks-timeout-http` in `data-portal/egret-grpa` (from `origin/main` `4f6cafec`). The spec is commit `c0c0f598`.

## Global Constraints

- No migration, no frontend, no new config key (spec §3).
- No change to `crates/egret-query/src/execution/connector/starrocks.rs`'s behaviour: no `SET`, no session variable (spec §1.3). Only its doc comment changes.
- The `http` sentence, verbatim: `The 'http' function is not supported: an expression renders inside egret's control plane, so a request made from it would leave from egret's own network. Make the request in a script task, which runs in the task's container.`
- Never use the shared dev stack (`deploy/docker-compose.dev.yml`, `:9030`, `:5432`). StarRocks for this plan is a private container `egret-grpa-starrocks` on host port **9131**.
- Run one cargo command at a time. Engine-gated tests skip silently without `EGRET_TEST_STARROCKS_URL`: every live run must show the test's own output, not `skip`.
- Commits follow Conventional Commits and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

### Task 1: measure StarRocks 4.1.6 before coding

**Files:**
- Modify: `docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md` §1.3 (add "Measured 2026-10-17" bullets)
- Modify: `docs/vi/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md` §1.3 (same, in Vietnamese)

**Interfaces:**
- Produces for Task 2: (a) whether `insert_timeout` exists; (b) `LONG_SQL`, a single `SELECT` that runs for more than 60 s on an empty all-in-one and that `query_timeout = 2` ends within 10 s; (c) the exact error text StarRocks returns when it ends it.

- [x] **Step 1: start the private engine.**

```bash
# No mysql client on the host: use the one in the image.
SR="docker exec -i egret-grpa-starrocks mysql -h127.0.0.1 -P9030 -uroot"
docker run -d --name egret-grpa-starrocks -p 9131:9030 \
  starrocks/allin1-ubuntu:4.1.6@sha256:5cbf09c4c788d2ca41a8e38599b650f04dbc6af0de51ebda0952770fb8333e5d
until $SR -e 'SELECT 1' >/dev/null 2>&1; do sleep 5; done
$SR -e "SHOW BACKENDS\G" | grep -E 'Alive'
```

Expected: `Alive: true`. Allow 30-90 s.

- [x] **Step 2: check the two variables exist.**

```bash
$SR -e "SHOW VARIABLES LIKE '%_timeout'"
$SR -e "SET query_timeout = 2, insert_timeout = 2; SELECT @@query_timeout, @@insert_timeout"
```

Record whether `insert_timeout` is listed and whether the combined `SET` succeeds.

- [x] **Step 3: find `LONG_SQL`.** Try in this order and keep the first that runs over 60 s with the default timeout and stops at about 2 s with `SET query_timeout = 2`:

```sql
SELECT count(*) AS n, 'egret-deadline-probe' AS marker FROM TABLE(generate_series(1, 100000000000));
SELECT sleep(120) AS s, 'egret-deadline-probe' AS marker;
```

```bash
time $SR -e "SET query_timeout = 2; <candidate>"   # with SR set as in Step 1
```

Record the elapsed time and the exact error line (for example `ERROR 1064 (HY000): ... timeout ...`).

- [x] **Step 4: confirm the engine, not the client, ended it.** Start the candidate with `query_timeout = 2` in one shell, and in another run `$SR -e "SHOW FULL PROCESSLIST"` at 1 s and at 5 s. Expected: the marker is in `Info` at 1 s and absent at 5 s. Then confirm the control: run it with no `SET`, kill the client with Ctrl-C at 3 s, and check `SHOW FULL PROCESSLIST` at 5 s. Expected: the marker is still there (this is the bug).

- [x] **Step 5: `insert_timeout`, only if Step 2 found it.** `CREATE DATABASE probe; CREATE TABLE probe.t (n BIGINT) DISTRIBUTED BY HASH(n) BUCKETS 1 PROPERTIES('replication_num'='1');` then `SET query_timeout = 600, insert_timeout = 2; INSERT INTO probe.t SELECT count(*) FROM TABLE(generate_series(1, 100000000000));`. Record whether it stops at about 2 s. Then run it again with `SET query_timeout = 2, insert_timeout = 600` and record which variable governs.

- [x] **Step 6: write the results into both specs** under §1.3 as `**Measured 2026-10-17 on 4.1.6:**` bullets: the variables' existence, `LONG_SQL`, its elapsed time with and without the deadline, the error text, which variable governs `INSERT`. If `insert_timeout` does not exist, change the §1.3 bullet to say only `query_timeout` is set.

- [x] **Step 7: commit.**

```bash
git add docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md docs/vi/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md
git commit -m "docs(workflow): measure StarRocks 4.1.6 query_timeout and insert_timeout

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

**Done when:** the spec carries the measured facts, and `LONG_SQL` is known.

---

### Task 2: correct the StarRocks docs and pin the measured behaviour

**Files:**
- Modify: `crates/egret-query/tests/connector_starrocks.rs` (three live tests)
- Modify: `crates/egret-query/src/execution/connector/starrocks.rs:300-305` (doc only)
- Modify: `crates/egret-workflow/src/worker/handlers/sql.rs:553-583` (doc of `cancel()` and its error text)
- Modify: `crates/egret-workflow/src/routes.rs:2225-2231` (the kill finding's point 7)
- Modify: `crates/egret-dataapi/src/serving/router.rs:45-49` (module doc)

**Interfaces:**
- Consumes from Task 1 (spec §1.2): `LONG_SQL` = `SELECT count(*) AS n, 'egret-deadline-probe' AS marker FROM TABLE(generate_series(1, 100000000000))`; StarRocks strips `/* */` comments from `Info`, so match on SQL text; a raw `mysql_async` probe connection needs `prefer_socket(false)`; the private engine runs at `mysql://127.0.0.1:9131`.
- Produces: nothing later tasks call.

- [x] **Step 1: write the three live tests** in `connector_starrocks.rs`, beside the existing ones, using the file's `parse()` / `spec()` helpers. Shared helpers in the same file:

```rust
/// A fresh raw connection to read the engine's own view of what is running.
async fn probe_conn(h: &str, p: i32) -> mysql_async::Conn {
    let opts = mysql_async::OptsBuilder::default()
        .ip_or_hostname(h.to_string())
        .tcp_port(p as u16)
        .user(Some("root"))
        .prefer_socket(false);
    mysql_async::Conn::new(opts).await.expect("probe connection")
}

/// Whether a statement whose text contains `needle` is running, ignoring the
/// `SHOW FULL PROCESSLIST` that asks.
async fn running(probe: &mut mysql_async::Conn, needle: &str) -> bool {
    use mysql_async::prelude::Queryable;
    let rows: Vec<mysql_async::Row> = probe.query("SHOW FULL PROCESSLIST").await.unwrap();
    rows.iter().any(|r| {
        let info: Option<String> = r.get_opt("Info").and_then(|v| v.ok()).flatten();
        info.is_some_and(|i| i.contains(needle) && !i.contains("SHOW FULL PROCESSLIST"))
    })
}

/// Drop `fut` after 3 s, as `port_impl::run_bounded` does at `limits.timeout`,
/// having seen the statement running; then assert the engine ended it.
async fn assert_dropped_statement_stops<F>(h: &str, p: i32, needle: &str, fut: F)
where
    F: std::future::Future,
{
    let mut probe = probe_conn(h, p).await;
    let watch = async {
        let mut seen = false;
        for _ in 0..25 {
            if running(&mut probe, needle).await {
                seen = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        seen
    };
    let (outer, seen) = tokio::join!(tokio::time::timeout(Duration::from_secs(3), fut), watch);
    assert!(outer.is_err(), "the statement must outlive the 3 s outer timeout");
    assert!(seen, "the probe never saw `{needle}` running, so it cannot see it stop");

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while running(&mut probe, needle).await {
        assert!(
            std::time::Instant::now() < deadline,
            "`{needle}` still runs on StarRocks 10 s after egret dropped it"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    eprintln!("ran: `{needle}` stopped after the drop");
}
```

The tests (spec §1.4):

```rust
/// Spec 2026-10-17 §1.2: dropping the future at the deadline closes the
/// connection, and StarRocks 4.1.6 cancels the statement on its own. This pins
/// that, so an upgrade that changes it goes red instead of making the docs in
/// `handlers/sql.rs` and `routes.rs` false again.
#[tokio::test]
async fn starrocks_cancels_a_select_when_egret_drops_it() {
    let Some((h, p)) = parse() else {
        eprintln!("skip");
        return;
    };
    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(3),
    };
    let sql = "SELECT count(*) AS n, 'egret-deadline-probe' AS marker \
               FROM TABLE(generate_series(1, 100000000000))";
    assert_dropped_statement_stops(
        &h,
        p,
        "egret-deadline-probe",
        c.execute(sql, &limits, CancellationToken::new()),
    )
    .await;
}

/// The same for an `INSERT … SELECT`, which StarRocks bounds by
/// `insert_timeout` (default 14400 s), not `query_timeout` (spec §1.2).
#[tokio::test]
async fn starrocks_cancels_an_insert_when_egret_drops_it() {
    let Some((h, p)) = parse() else {
        eprintln!("skip");
        return;
    };
    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(30),
    };
    for ddl in [
        "CREATE DATABASE IF NOT EXISTS egret_drop_probe",
        "CREATE TABLE IF NOT EXISTS egret_drop_probe.t (n BIGINT) \
         DISTRIBUTED BY HASH(n) BUCKETS 1 PROPERTIES('replication_num'='1')",
    ] {
        c.execute(ddl, &limits, CancellationToken::new()).await.unwrap();
    }
    let sql = "INSERT INTO egret_drop_probe.t \
               SELECT count(*) FROM TABLE(generate_series(1, 100000000000))";
    assert_dropped_statement_stops(
        &h,
        p,
        "INSERT INTO egret_drop_probe.t",
        c.execute(sql, &limits, CancellationToken::new()),
    )
    .await;
}
```

If `OptsBuilder`, `get_opt` or `query` do not compile against the pinned `mysql_async`, adapt the call shape in the helpers only; the assertions stay the same.

- [x] **Step 2: run them, expect PASS** (the behaviour holds today), with `ran:` lines in the output for all three.

```bash
EGRET_TEST_STARROCKS_URL=mysql://127.0.0.1:9131 \
  cargo test -p egret-query --test connector_starrocks -- --nocapture
```

The whole file runs, so the existing tests still pass too.

- [x] **Step 3: show the test can fail (probe mutation, spec §1.4).** In the SELECT test only, change the `needle` argument to `"egret-never-in-info"`; rerun; expect FAIL with `the probe never saw`. Restore it; check `git diff` on the test shows the original needle; rerun; expect PASS. Record both outputs for the report.

- [x] **Step 4: correct the texts** (spec §1.1 lists them; §1.3 says what they become). Each states what was measured, with the date and engine version, and points at the three tests by name:
  - `starrocks.rs:300-305`: replace the "**Not yet timeout-bound.**" paragraph: `limits.timeout` is still unread here; the caller's outer `tokio::time::timeout` drops this future, which closes the connection, and StarRocks 4.1.6 cancels the statement on its own (measured 2026-10-17, pinned by `starrocks_cancels_a_select_when_egret_drops_it`). Keep the obligation that a caller MUST supply the outer wrapper.
  - `handlers/sql.rs` `cancel()` doc, the StarRocks bullet (`:557-565`): StarRocks, like Trino, does not leave an abandoned statement running: the worker's drop closes the connection and the engine cancels it (measured 2026-10-17). What this handler still cannot do is stop a statement **before** its budget, which needs a port verb and `KILL QUERY` (group B). Delete the sentence that the session variable would fail Data API queries, and the claim that a timed-out statement runs to completion.
  - `handlers/sql.rs:579-582` error text: replace `on StarRocks that connector does not read \`Limits::timeout\` at all, so it is not bounded by anything` with `on StarRocks it ends when the connection is dropped at that budget`. Keep `QueryExecPort` and `StarRocks` in the text: `cancel_refuses_rather_than_claiming_to_have_stopped_the_statement` (`:1428`) asserts both.
  - `routes.rs:2225-2231`: update the quote to the new error text, and replace the last sentence (*"even a complete kill mechanism would leave a statement running on StarRocks with nothing to stop it"*) with: a kill still cannot reach `egret.sql` before its budget, because the handler has no way to stop its statement early; an abandoned statement does not keep running on StarRocks 4.1.6 (measured 2026-10-17).
  - `serving/router.rs:45-49`: keep `neither … reads limits.timeout itself`, and replace `and StarRocks sets no socket timeout at all` with a sentence that dropping the future closes the connection, which StarRocks 4.1.6 answers by cancelling the statement (measured 2026-10-17); Trino additionally bounds the coordinator with `query_max_run_time` (the next bullet already says so).

Then check nothing still states the old behaviour:

```bash
rg -n "not bounded by anything|runs to completion on the engine|run the full hour here on StarRocks|sets no socket timeout|with nothing to stop it" crates
```

Expected: no output. *(2026-10-17: `rg` is not installed on this host, so this sweep printed nothing and proved nothing; it was redone with `grep -rn` in the final-review fix wave, which found claim.rs, guard.rs, oracle-exec/README.md and sql.rs:541.)* If a hit remains elsewhere (for example `handlers/sql.rs:503-504`, *"runs the full hour here on StarRocks"*), correct it the same way and list it in the report.

- [x] **Step 5: format, lint, run the crates whose text changed.**

```bash
cargo fmt --all
cargo clippy -p egret-query -p egret-workflow -p egret-dataapi --all-targets -- -D warnings
cargo test -p egret-workflow --lib handlers::sql
```

- [x] **Step 6: commit.**

```bash
git add crates/egret-query crates/egret-workflow/src/worker/handlers/sql.rs crates/egret-workflow/src/routes.rs crates/egret-dataapi/src/serving/router.rs
git commit -m "fix(workflow): a timed-out StarRocks statement is cancelled, say so

Measured on StarRocks 4.1.6: dropping the connector future at the
deadline closes the connection and the engine cancels the statement.
Correct the docs and the cancel() message that said it runs to
completion, and pin the behaviour with three engine-gated tests.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

**Done when:** all three live tests pass with `ran:` in the output, the probe mutation fails as expected, and the `rg` above finds nothing.

---

### Task 3: `http()` is refused for good

**Files:**
- Modify: `crates/egret-workflow/src/runners/functions.rs` (`enum Refusal` `:215-224`; the match at `:204-207`; the `REFUSED` row `:285`; the doc above `REFUSED` `:240-283`; the test `the_refused_list_is_six_and_kv_is_not_in_it` `:3561`)

**Interfaces:**
- Produces: `Refusal::Unsupported(&'static str)` and `pub(crate) const HTTP_REFUSAL: &str`.

- [x] **Step 1: change the test first.** In `the_refused_list_is_six_and_kv_is_not_in_it`, remove the `("http", "an outbound HTTP client", "W9")` line from `expected`, change the count check to `expected.len() + 2` with the comment `// \`secret\` (Confined) and \`http\` (Unsupported) are checked by their sentences below.`, and change the chain to:

```rust
        for (name, want) in not_yet.chain([
            ("secret", SECRET_REFUSAL.to_string()),
            ("http", HTTP_REFUSAL.to_string()),
        ]) {
```

After the loop, add:

```rust
        let message = err("{{ http('https://example.com') }}");
        assert!(
            !message.contains("not implemented yet") && !message.contains("W9"),
            "http names no package that would provide it: {message}"
        );
```

- [x] **Step 2: run it, expect a compile failure** (`cannot find value HTTP_REFUSAL`).

```bash
cargo test -p egret-workflow --lib the_refused_list_is_six
```

- [x] **Step 3: implement.** Beside `SECRET_REFUSAL`:

```rust
/// `http()`'s refusal (2026-10-17, spec `2026-10-17-workflow-starrocks-timeout-and-http`
/// §2). Not "not yet": an expression renders inside the egret server, so a
/// request from it would leave from the control plane's network, which is a
/// server-side request forgery for any flow author. A script task makes the
/// same request from its own container.
pub(crate) const HTTP_REFUSAL: &str = "The 'http' function is not supported: an expression \
     renders inside egret's control plane, so a request made from it would leave from \
     egret's own network. Make the request in a script task, which runs in the task's \
     container.";
```

Add the variant to `enum Refusal`:

```rust
    /// egret does not provide it and will not: the reason is where it would
    /// run, not a missing package. The message is the whole sentence.
    Unsupported(&'static str),
```

Extend the match: `Refusal::Confined(reason) | Refusal::Unsupported(reason) => pebble_error(*reason),`. Change the row to `("http", Refusal::Unsupported(HTTP_REFUSAL)),`. In the doc above `REFUSED`, add `**Updated 2026-10-17: \`http\` is \`Unsupported\`, not waiting for a package.**` and remove the sentence that `http` is left naming W9.

- [x] **Step 4: run the test, expect PASS.** Then the whole functions module:

```bash
cargo test -p egret-workflow --lib the_refused_list_is_six
cargo test -p egret-workflow --lib runners::functions
```

- [x] **Step 5: check the oracle still agrees.** The Pebble corpus compares refusals by error class (`oracle/error-class.txt:37`, paired with this test in `oracle/error-class-pairs.txt:71`), so a changed sentence must not move the ratchet:

```bash
rg -n "function-http" crates/egret-workflow/oracle/   # error-class.txt, error-class-pairs.txt
cargo test -p egret-workflow --test oracle
```

Expected: same pass/baseline counts as before the change (record them).

- [x] **Step 6: format, lint, commit.**

```bash
cargo fmt --all
cargo clippy -p egret-workflow --all-targets -- -D warnings
git add crates/egret-workflow/src/runners/functions.rs
git commit -m "fix(workflow): refuse http() because it would run in the control plane

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

**Done when:** `{{ http(...) }}` fails with `HTTP_REFUSAL`, no message names W9 for it, and the oracle counts are unchanged.

---

### Task 4: record the decisions, gate, PR

**Files:**
- Modify: `docs/en/specs/2026-09-01-workflow-programme-design.md` (`:515`, `:541`)
- Modify: `docs/vi/specs/2026-09-01-workflow-programme-design.md` (the same two sentences)
- Modify: this plan (tick the boxes)

- [x] **Step 1: programme spec, EN.** At `:515` replace `and \`http\`, still owned by no row` with `and \`http\`, refused on purpose since 2026-10-17 because it would run in the control plane`. At the end of the parenthetical on `:541` append: `*(Decided 2026-10-17: \`http\` is refused permanently, \`Refusal::Unsupported\`; an expression renders in the egret server, so a request from it is a server-side request forgery. See \`docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md\` §2.)*`
- [x] **Step 2: the same two edits in the VI file**, in Vietnamese, found with `rg -n "http" docs/vi/specs/2026-09-01-workflow-programme-design.md`.
- [x] **Step 3: full local gate** (CI is out of quota; this is the merge gate). One command at a time:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p egret-query
cargo test -p egret-workflow
cargo test -p egret-dataapi
EGRET_TEST_STARROCKS_URL=mysql://127.0.0.1:9131 cargo test -p egret-query --test connector_starrocks -- --nocapture
```

Record pass/fail counts. Any red test is investigated before claiming it pre-existed: run it on `origin/main` in the same environment.

- [x] **Step 4: tick this plan's boxes, commit the docs.**

```bash
git add docs/en/specs/2026-09-01-workflow-programme-design.md docs/vi/specs/2026-09-01-workflow-programme-design.md docs/en/plans/2026-10-17-workflow-starrocks-timeout-and-http.md
git commit -m "docs(workflow): record http() as refused on purpose

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 5: clean up the engine.** `docker rm -f egret-grpa-starrocks`.
- [ ] **Step 6: push and open the PR** (title `fix(workflow): StarRocks cancels a dropped statement (docs + test); refuse http()`), body listing the measurements from Task 1, the probe-mutation result from Task 2, the gate counts, and ending with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`. Merge follows the autonomy rules (admin-merge when the local gate is green).

**Done when:** the PR is open with a green local gate recorded in its body.
