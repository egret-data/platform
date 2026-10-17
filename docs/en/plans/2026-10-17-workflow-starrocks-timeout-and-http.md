# StarRocks server-side timeout and refusing `http()`: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md`. Read it first; this plan refers to its sections.

**Goal:** a StarRocks statement egret has timed out stops on the engine at the same deadline, and `http()` is refused with a reason instead of "waiting for W9".

**Architecture:** one private helper in the StarRocks connector sets `query_timeout` (and `insert_timeout` if Task 1 finds it) on each fresh connection before the statement. A new `Refusal::Unsupported` variant in the expression layer carries `http`'s permanent refusal.

**Tech Stack:** Rust, `mysql_async`, `minijinja`, StarRocks 4.1.6 (`starrocks/allin1-ubuntu:4.1.6@sha256:5cbf09c4c788d2ca41a8e38599b650f04dbc6af0de51ebda0952770fb8333e5d`, already pulled locally).

**Branch / worktree:** `feat/workflow-starrocks-timeout-http` in `data-portal/egret-grpa` (from `origin/main` `4f6cafec`). The spec is commit `c0c0f598`.

## Global Constraints

- No migration, no frontend, no new config key (spec §3).
- `N = ceil(limits.timeout in seconds)`, minimum 1 (spec §1.3).
- A failed `SET` returns `EngineError::Engine` and the statement is not sent; never fail open (spec §1.3).
- `describe` / `describe_with_params` are not changed (spec §1.3).
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

- [ ] **Step 1: start the private engine.**

```bash
# No mysql client on the host: use the one in the image.
SR="docker exec -i egret-grpa-starrocks mysql -h127.0.0.1 -P9030 -uroot"
docker run -d --name egret-grpa-starrocks -p 9131:9030 \
  starrocks/allin1-ubuntu:4.1.6@sha256:5cbf09c4c788d2ca41a8e38599b650f04dbc6af0de51ebda0952770fb8333e5d
until $SR -e 'SELECT 1' >/dev/null 2>&1; do sleep 5; done
$SR -e "SHOW BACKENDS\G" | grep -E 'Alive'
```

Expected: `Alive: true`. Allow 30-90 s.

- [ ] **Step 2: check the two variables exist.**

```bash
$SR -e "SHOW VARIABLES LIKE '%_timeout'"
$SR -e "SET query_timeout = 2, insert_timeout = 2; SELECT @@query_timeout, @@insert_timeout"
```

Record whether `insert_timeout` is listed and whether the combined `SET` succeeds.

- [ ] **Step 3: find `LONG_SQL`.** Try in this order and keep the first that runs over 60 s with the default timeout and stops at about 2 s with `SET query_timeout = 2`:

```sql
SELECT count(*) AS n, 'egret-deadline-probe' AS marker FROM TABLE(generate_series(1, 100000000000));
SELECT sleep(120) AS s, 'egret-deadline-probe' AS marker;
```

```bash
time $SR -e "SET query_timeout = 2; <candidate>"   # with SR set as in Step 1
```

Record the elapsed time and the exact error line (for example `ERROR 1064 (HY000): ... timeout ...`).

- [ ] **Step 4: confirm the engine, not the client, ended it.** Start the candidate with `query_timeout = 2` in one shell, and in another run `$SR -e "SHOW FULL PROCESSLIST"` at 1 s and at 5 s. Expected: the marker is in `Info` at 1 s and absent at 5 s. Then confirm the control: run it with no `SET`, kill the client with Ctrl-C at 3 s, and check `SHOW FULL PROCESSLIST` at 5 s. Expected: the marker is still there (this is the bug).

- [ ] **Step 5: `insert_timeout`, only if Step 2 found it.** `CREATE DATABASE probe; CREATE TABLE probe.t (n BIGINT) DISTRIBUTED BY HASH(n) BUCKETS 1 PROPERTIES('replication_num'='1');` then `SET query_timeout = 600, insert_timeout = 2; INSERT INTO probe.t SELECT count(*) FROM TABLE(generate_series(1, 100000000000));`. Record whether it stops at about 2 s. Then run it again with `SET query_timeout = 2, insert_timeout = 600` and record which variable governs.

- [ ] **Step 6: write the results into both specs** under §1.3 as `**Measured 2026-10-17 on 4.1.6:**` bullets: the variables' existence, `LONG_SQL`, its elapsed time with and without the deadline, the error text, which variable governs `INSERT`. If `insert_timeout` does not exist, change the §1.3 bullet to say only `query_timeout` is set.

- [ ] **Step 7: commit.**

```bash
git add docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md docs/vi/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md
git commit -m "docs(workflow): measure StarRocks 4.1.6 query_timeout and insert_timeout

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

**Done when:** the spec carries the measured facts, and `LONG_SQL` is known.

---

### Task 2: the connector sets the deadline on every statement

**Files:**
- Modify: `crates/egret-query/src/execution/connector/starrocks.rs` (helper near `switch_identity` at `:226`; calls in `execute` `:251` and `execute_with_params` `:306`; doc at `:300-305`; unit tests in the file's `mod tests`)
- Modify: `crates/egret-query/tests/connector_starrocks.rs` (live test)
- Modify: `crates/egret-workflow/src/worker/handlers/sql.rs:553-583` (doc of `cancel()` and its error text)
- Modify: `crates/egret-workflow/src/routes.rs:2225-2231` (the quoted sentence)
- Modify: `crates/egret-dataapi/src/serving/router.rs:45-49` (module doc)

**Interfaces:**
- Consumes from Task 1: `LONG_SQL`, whether `insert_timeout` exists.
- Produces: `fn deadline_secs(timeout: std::time::Duration) -> u64` and `async fn bound_session(conn: &mut mysql_async::Conn, timeout: Duration) -> Result<(), EngineError>`, both private to `starrocks.rs`.

- [ ] **Step 1: write the failing unit test** in `starrocks.rs`'s `mod tests`:

```rust
#[test]
fn deadline_rounds_up_to_whole_seconds_and_never_below_one() {
    use std::time::Duration;
    assert_eq!(deadline_secs(Duration::from_millis(1200)), 2);
    assert_eq!(deadline_secs(Duration::from_millis(200)), 1);
    assert_eq!(deadline_secs(Duration::ZERO), 1);
    assert_eq!(deadline_secs(Duration::from_secs(30)), 30);
}
```

- [ ] **Step 2: run it, expect a compile failure** (`cannot find function deadline_secs`).

```bash
cargo test -p egret-query --lib deadline_rounds_up
```

- [ ] **Step 3: write the failing live test** in `crates/egret-query/tests/connector_starrocks.rs`, using the file's `parse()` / `spec()` helpers. Put Task 1's `LONG_SQL` in the constant.

```rust
/// The engine, not egret's outer wrapper, ends a statement at `limits.timeout`.
/// No `tokio::time::timeout` sits at the deadline here, so only the session
/// variable can stop it; the 30 s guard exists so a regression fails instead
/// of hanging.
#[tokio::test]
async fn starrocks_stops_a_statement_at_limits_timeout() {
    const LONG_SQL: &str = "<Task 1's LONG_SQL, containing 'egret-deadline-probe'>";
    let Some((h, p)) = parse() else {
        eprintln!("skip");
        return;
    };
    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(2),
    };
    let started = std::time::Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        c.execute(LONG_SQL, &limits, CancellationToken::new()),
    )
    .await
    .expect("the engine must end the statement long before 30 s");
    let err = out.expect_err("a statement over its deadline must fail");
    assert!(
        matches!(err, egret_query::execution::EngineError::Engine(_)),
        "got {err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());

    // The engine's view: the probe is no longer running.
    let url = format!("mysql://root@{h}:{p}");
    let mut probe = mysql_async::Conn::new(mysql_async::Opts::from_url(&url).unwrap())
        .await
        .unwrap();
    let infos: Vec<Option<String>> = mysql_async::prelude::Queryable::query_map(
        &mut probe,
        "SHOW FULL PROCESSLIST",
        |row: mysql_async::Row| row.get_opt::<Option<String>, _>("Info").and_then(|r| r.ok()).flatten(),
    )
    .await
    .unwrap();
    assert!(
        !infos.iter().flatten().any(|i| i.contains("egret-deadline-probe")
            && !i.contains("SHOW FULL PROCESSLIST")),
        "the probe is still running on the engine: {infos:?}"
    );
    eprintln!("ran: ended after {:?}", started.elapsed());
}
```

If the `query_map` closure does not compile against the pinned `mysql_async`, read `Info` with `row.get::<Option<String>, _>("Info")` inside a `query::<mysql_async::Row, _>` loop instead; the assertion stays the same.

- [ ] **Step 4: run it, expect FAIL** (the 30 s guard fires: `the engine must end the statement long before 30 s`).

```bash
EGRET_TEST_STARROCKS_URL=mysql://127.0.0.1:9131 \
  cargo test -p egret-query --test connector_starrocks starrocks_stops_a_statement -- --nocapture
```

Then `$SR -e "SHOW FULL PROCESSLIST"` and `KILL <id>` any leftover probe so the next run starts clean.

- [ ] **Step 5: implement** in `starrocks.rs`, beside `switch_identity`:

```rust
/// `limits.timeout` as StarRocks' whole-second session deadline: rounded UP,
/// so the engine never ends a statement before egret stops waiting for it,
/// and never 0, which StarRocks does not accept as "no limit".
fn deadline_secs(timeout: Duration) -> u64 {
    let whole = timeout.as_secs();
    let rounded = if timeout.subsec_nanos() > 0 { whole + 1 } else { whole };
    rounded.max(1)
}

/// Tell the engine the deadline egret already enforces by dropping the
/// future (spec 2026-10-17 §1). Dropping the MySQL connection tells
/// StarRocks nothing, so without this a timed-out statement runs to
/// completion. The connection is fresh per call and disconnected after
/// it, so the variable reaches no other statement. A failure here fails
/// the call: running without a deadline is the bug this closes.
async fn bound_session(conn: &mut mysql_async::Conn, timeout: Duration) -> Result<(), EngineError> {
    let n = deadline_secs(timeout);
    conn.query_drop(format!("SET query_timeout = {n}, insert_timeout = {n}"))
        .await
        .map_err(|e| EngineError::Engine(e.to_string()))
}
```

If Task 1 found no `insert_timeout`, the statement is `SET query_timeout = {n}` and the doc says why. In both `execute` and `execute_with_params`, after the `if let Some(u) = &self.execute_as { … }` block, add:

```rust
        bound_session(&mut conn, limits.timeout).await?;
```

Make sure `Duration` is imported (`use std::time::Duration;` is already used by `describe`; reuse it).

- [ ] **Step 6: run both tests, expect PASS**; the live output must contain `ran: ended after`.

```bash
cargo test -p egret-query --lib deadline_rounds_up
EGRET_TEST_STARROCKS_URL=mysql://127.0.0.1:9131 \
  cargo test -p egret-query --test connector_starrocks -- --nocapture
```

The whole `connector_starrocks` file runs, so the existing tests (identity switch, `COM_RESET_CONNECTION`) prove the extra `SET` broke nothing.

- [ ] **Step 7: mutation.** Comment out the `bound_session` call in `execute` only; rerun the live test; expect FAIL. Restore it; check `git diff` shows the call is back; rerun; expect PASS. Record both results for the report.

- [ ] **Step 8: correct the docs that state the old behaviour.**
  - `starrocks.rs:300-305`: replace the "**Not yet timeout-bound.**" paragraph with: the engine ends the statement at `limits.timeout` through `bound_session`; callers still wrap the future so egret stops waiting at the same instant.
  - `handlers/sql.rs` `cancel()` doc, the StarRocks bullet (`:557-565`): StarRocks is bounded by `query_timeout` set from the same `Limits::timeout`, like Trino's `query_max_run_time`; a real cancel before the deadline still needs `KILL QUERY` (group B). Delete the sentence about the Data API path failing queries.
  - `handlers/sql.rs:579-582` error text: `On Trino it is still bounded by the \`query_max_run_time\` this task sent with it, and on StarRocks by the \`query_timeout\` this task's connector set`. Keep `QueryExecPort` and `StarRocks` in the text: `cancel_refuses_rather_than_claiming_to_have_stopped_the_statement` (`:1428`) asserts both.
  - `routes.rs:2225-2231`: update the quote to the new text and change the last sentence to: a complete kill mechanism would still leave a StarRocks statement running until its `query_timeout`.
  - `serving/router.rs:45-49`: `neither … reads limits.timeout itself, and StarRocks sets no socket timeout at all` becomes: Trino sends `query_max_run_time` and StarRocks sets `query_timeout` from the same `limits.timeout`, so the engine also stops the statement; the wrapper is still what releases egret.

- [ ] **Step 9: format, lint, and run the crates whose text changed.**

```bash
cargo fmt --all
cargo clippy -p egret-query -p egret-workflow -p egret-dataapi --all-targets -- -D warnings
cargo test -p egret-workflow --lib handlers::sql
```

- [ ] **Step 10: commit.**

```bash
git add crates/egret-query crates/egret-workflow/src/worker/handlers/sql.rs crates/egret-workflow/src/routes.rs crates/egret-dataapi/src/serving/router.rs
git commit -m "fix(query): StarRocks stops a statement at limits.timeout

Each connection sets query_timeout (and insert_timeout) from the
deadline every caller already enforces by dropping the future, so a
statement egret has given up on no longer runs to completion on the
engine.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

**Done when:** the live test passes with `ran:` in its output, fails with the call removed, and no doc still says StarRocks ignores `limits.timeout` (`rg -n "not bounded by anything|unread here|sets no socket timeout" crates` finds nothing).

---

### Task 3: `http()` is refused for good

**Files:**
- Modify: `crates/egret-workflow/src/runners/functions.rs` (`enum Refusal` `:215-224`; the match at `:204-207`; the `REFUSED` row `:285`; the doc above `REFUSED` `:240-283`; the test `the_refused_list_is_six_and_kv_is_not_in_it` `:3561`)

**Interfaces:**
- Produces: `Refusal::Unsupported(&'static str)` and `pub(crate) const HTTP_REFUSAL: &str`.

- [ ] **Step 1: change the test first.** In `the_refused_list_is_six_and_kv_is_not_in_it`, remove the `("http", "an outbound HTTP client", "W9")` line from `expected`, change the count check to `expected.len() + 2` with the comment `// \`secret\` (Confined) and \`http\` (Unsupported) are checked by their sentences below.`, and change the chain to:

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

- [ ] **Step 2: run it, expect a compile failure** (`cannot find value HTTP_REFUSAL`).

```bash
cargo test -p egret-workflow --lib the_refused_list_is_six
```

- [ ] **Step 3: implement.** Beside `SECRET_REFUSAL`:

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

- [ ] **Step 4: run the test, expect PASS.** Then the whole functions module:

```bash
cargo test -p egret-workflow --lib the_refused_list_is_six
cargo test -p egret-workflow --lib runners::functions
```

- [ ] **Step 5: check the oracle still agrees.** The Pebble corpus compares refusals by error class (`oracle/error-class.txt:37`, paired with this test in `oracle/error-class-pairs.txt:71`), so a changed sentence must not move the ratchet:

```bash
rg -n "function-http" crates/egret-workflow/oracle/   # error-class.txt, error-class-pairs.txt
cargo test -p egret-workflow --test oracle
```

Expected: same pass/baseline counts as before the change (record them).

- [ ] **Step 6: format, lint, commit.**

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

- [ ] **Step 1: programme spec, EN.** At `:515` replace `and \`http\`, still owned by no row` with `and \`http\`, refused on purpose since 2026-10-17 because it would run in the control plane`. At the end of the parenthetical on `:541` append: `*(Decided 2026-10-17: \`http\` is refused permanently, \`Refusal::Unsupported\`; an expression renders in the egret server, so a request from it is a server-side request forgery. See \`docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md\` §2.)*`
- [ ] **Step 2: the same two edits in the VI file**, in Vietnamese, found with `rg -n "http" docs/vi/specs/2026-09-01-workflow-programme-design.md`.
- [ ] **Step 3: full local gate** (CI is out of quota; this is the merge gate). One command at a time:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p egret-query
cargo test -p egret-workflow
cargo test -p egret-dataapi
EGRET_TEST_STARROCKS_URL=mysql://127.0.0.1:9131 cargo test -p egret-query --test connector_starrocks -- --nocapture
```

Record pass/fail counts. Any red test is investigated before claiming it pre-existed: run it on `origin/main` in the same environment.

- [ ] **Step 4: tick this plan's boxes, commit the docs.**

```bash
git add docs/en/specs/2026-09-01-workflow-programme-design.md docs/vi/specs/2026-09-01-workflow-programme-design.md docs/en/plans/2026-10-17-workflow-starrocks-timeout-and-http.md
git commit -m "docs(workflow): record http() as refused on purpose

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 5: clean up the engine.** `docker rm -f egret-grpa-starrocks`.
- [ ] **Step 6: push and open the PR** (title `fix(workflow): StarRocks honours limits.timeout; refuse http()`), body listing the measurements from Task 1, the mutation result from Task 2, the gate counts, and ending with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`. Merge follows the autonomy rules (admin-merge when the local gate is green).

**Done when:** the PR is open with a green local gate recorded in its body.
