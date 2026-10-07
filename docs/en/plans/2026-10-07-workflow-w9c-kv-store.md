# W9c — The KV store, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A flow reads and writes Kestra's namespace KV store in egret. Each revision is an Ion object at `{prefix}{ws}/{ns-as-path}/_kv/{key}.ion[.vN]`, indexed in `workflow.kv_entries`. The plan covers the `kv()` function; the tasks `Set`, `Get`, `GetKeys`, `Delete`, `Put` and `PurgeKV`; an hourly expiry sweep; the KV API; `secret()` refused in a KV value at publish; and a plugin-host refusal that points at the `core.kv` tasks.

**Architecture:**
- `egret-workflow`'s `storage` module gains `kv`. It holds the key rule, the value types and their Ion encoding, the metadata store (the read side, then the locked write side), `Put`'s merge, the expiry sweep and the routes.
- KV reuses W9b wherever W9b already has the piece: **one** access function (`nsfiles::access::authorize`, which gains a `Resource` kind), the `Grant` every store function requires, `NsError` and its split, `CreatedBy`, the staging objects and their sweep, `LockedSession`, the render's one namespace connection (`storage/scope.rs`) and the route conventions (`nsfiles/routes.rs`).
- Every write takes the per-`(workspace, namespace, key)` lock `pg_advisory_xact_lock(hashtextextended('w9c:{ws}:{ns}:{key}', 0))`. Purges and the sweep take the same key as a **session** lock, held across "delete rows, commit, delete objects".
- KV is **not** in any execution's transaction and not in W9a's pre-render snapshot: `kv()` looks a key up at render, on the scope's connection.

**Tech Stack:** Rust (tokio, sqlx 0.9, minijinja, axum 0.8, `ion-rs` =1.0.1 as a **reader** only, egret's own Ion text writer `runners::ionfmt`), Java 25 + Maven (`plugin-host`, Kestra `io.kestra:core:2.0.0-rc13`), Postgres 16, MinIO, Valkey, Docker, Helm, kind.

**Spec:** `docs/{en,vi}/specs/2026-10-07-workflow-w9c-kv-store-design.md`. **§n refers to the spec.** The spec's §12 lists eight tasks; this plan has the same eight, in the same order (*Task map*).

**A warning about this document.** Code blocks show shapes to verify, not text to paste. Every existing name and `file:line` below was read at `e04e452d` (base `origin/main` `11782600` + the two spec commits). Every new name is defined in the task that introduces it and spelled the same way afterwards. A block marked **sketch** compiles only once the anchor it names has been read and the shape adjusted. Wherever the plan disagrees with the code, the plan is wrong, and the implementer says so in the report. W9a's plan was wrong in five places and W9b's in more (memory `plan-illustrative-code-is-a-guess`).

## Global Constraints

- **User decisions (spec header, 2026-10-07), in effect verbatim:**
  - **Scope.** W9c ships the `kv()` function; the tasks `Set`, `Get`, `GetKeys` and `Delete`; `PurgeKV`; the deep-merge `Put` task; and the KV API. The plugin host gets **no** KV bridge, because no catalog plugin calls `namespaceKv`. Its refusal message changes to point at the `core.kv.*` tasks (§10).
  - **The write scope is W9b's.** A flow reads the KV of any namespace in its workspace. It writes and deletes only in its own namespace. `PurgeKV` acts on the flow's own namespace and its child namespaces, on a dot boundary. One `authorize` serves both W9b and W9c, with a KV resource kind added.
  - **Storage follows Kestra.** Each revision's value is an Ion text object at `{prefix}{ws}/{ns-as-path}/_kv/{key}.ion`, or `….ion.vN` for revision N > 1. `workflow.kv_entries` holds one row per revision. egret **stores** the type. A write reuses W9b's staging: the body is staged while no connection or lock is held, then one short locked transaction locks `(ws, ns, key)`, checks `overwrite`, picks the revision, copies server-side, runs `head`, inserts the row and commits. Keys follow `[a-zA-Z0-9][a-zA-Z0-9._-]*`, at most 350 bytes.
  - **The value cap (the user's ruling on the value cap, 2026-10-07): a KV value's writes are capped at `max_read_bytes`, not `max_file_bytes`, so anything writable is also readable.** `Set`, `Put` (its merged result included) and the API's `PUT` refuse an encoded value over `max_read_bytes`, naming the cap. `max_file_bytes` does not apply to KV values.
  - **TTL: expired means missing.** A read of an expired key behaves exactly like a read of a missing key and honours `errorOnMissing`. **Reads never write.** An hourly sweep hard-deletes expired keys (rows and objects) by row id, across every workspace, and is safe on every replica.
  - **Inheritance follows Kestra.** `kv()` with no namespace, or a `Get` whose namespace is the flow's own, walks up the parents on a dot boundary. An explicit namespace does not walk. The API has a list-inherited route.
  - **`secret()` inside a `Set` or `Put` value is refused at publish**, with a message saying the secret's value would be exposed through the KV store.
  - **Approved design points:** versions and soft delete follow W9b's R1; `PurgeKV` purges expired keys only, or by version (`before`/`keepAmount`), with `keyPattern`, by row id with keyset paging, and a version purge needs `keepAmount` or `before`; `Set` takes `key`, `value`, `overwrite` (default true), `ttl`, `kvType`, `kvDescription`, with strict BOOLEAN; `Get` outputs `value`, `GetKeys` takes `prefix` and is bounded by `max_namespace_files`, `Delete` outputs `deleted`; `Put` deep-merges under the key's lock; `kv()`'s `errorOnMissing` defaults to true and the tasks' to false; the API lives under `/api/ws/{ws}/workflows/namespaces/{ns}/kv…`, reads need `workflow:read` and writes `workflow:manage`, and a lock wait over 5 s answers 409.
- **The user's rulings on this plan's gaps (2026-10-07), now in the spec's header:**
  - the API's bulk `DELETE` takes at most **1,000 keys** per request, in one all-or-nothing transaction, and more answers 413 naming the cap (item 6);
  - the server-paced store calls under a key's lock are **accepted** (item 7; the hold rule below);
  - `Put` on an expired key treats it as **missing**: it starts fresh, carrying over no description and no TTL (item 8);
  - a namespace of another workspace answers as an empty one: listings `200 []`, key reads 404 (item 9);
  - items 4, 5, 12, 13 and 14 are recorded in the spec as decisions.
- **The user's rulings on Task 1's findings (2026-10-07), now in the spec's header:**
  - `Put` stays a **real deep merge**, with the semantics of the fixture's `put_merge` cells (the artifact's `Put.mergeValues`); on the artifact a flow's value reaches `Put` as JSON text, so the merge is unreachable there (spec §2, §8.5; Tasks 3, 5);
  - the API's `PUT` body: **exactly one Ion value** is stored as that value's type, anything else as the raw text, typed STRING; a blob, clob or s-expression is 400 naming the type; a timestamp with a time is a DATETIME in UTC, a day-precision one a DATE (spec §2, §9; Task 6, `api.put_bodies`);
  - `PurgeKV`'s **`before` follows Kestra**: a revision is purged when the `created_at` of the key's next revision is at or before `before`; the latest is never purged by `before` (spec §2, §8.6; Tasks 3, 5, `purge.version_before`).
- **The user's rulings on Task 1's review (2026-10-07), now in the spec's header:**
  - **R-a:** `Set` stores a YAML map or list **typed**, as JSON, exactly as `Put` keeps it; one render rule serves both (Task 5, `typed_render.tr_yaml_*`);
  - **R-b:** an empty or whitespace-only `PUT` body answers **400 "empty body"** (Task 6, `api.put_bodies.empty`, `ws_only`);
  - **R-c:** a year- or month-precision Ion timestamp (`2025T`, `2025-01T`) is stored as the **raw text**, typed STRING (Task 6, `api.put_bodies.ion_ts_year`, `ion_ts_month`).
- **Dedicated containers for this branch:** Postgres `egret-w9c-postgres` on :5440 (postgres:16, egret/egret, db egret_test), Valkey `egret-w9c-valkey` :6400, MinIO `egret-w9c-minio` :9001 (egret/egretsecret, bucket warehouse). The env for every cargo test includes `EGRET_TEST_STARROCKS_URL=mysql://localhost:9030`.
- **Destructive tests run only on MemStore or under a throwaway prefix `w9c-tN-test/<uuid>/`. Never touch the shared dev DB or bucket. Never touch a kind cluster named `egret`.** On MinIO, a test builds a `FileStore` with that `prefix` and removes it with `delete_prefix` at the end. Never delete at the bucket root or under `workflow/`. Postgres means `egret-w9c-postgres` only. Task 1's `_kv` count runs **one read-only `SELECT`** on the dev database and touches nothing else there.
- **No DB connection or lock may be held across slow I/O (client bodies, S3 streaming, plugin runs, renders). Ask this for every write path.** Each task has a **hold audit** step. It names every place a pool connection or an advisory lock is held, and what I/O happens while it is. The server-paced windows the spec accepts are named where they occur, with their bound:
  - one server-side `copy` plus `head` (`Set`, the API's `PUT`);
  - `Put`'s bounded store read and store write, each at most `max_read_bytes`;
  - a purge's or the sweep's object deletes, under a session lock.

  **These three windows are an explicit allowance (the user's ruling, 2026-10-07; spec §4.3, §15)**, and they are the only ones. Client-paced I/O under a connection or a lock (a request body), a render and a plugin run stay forbidden; any of them is a review failure. API writers keep the 5 s lock wait, then 409 (`API_LOCK_WAIT`).
- **Wait while `pgrep -x cargo` shows a build. Run clippy as `rtk proxy cargo clippy`.** Never two cargo test processes at once (memory `no-concurrent-test-suites-and-tail-buffering`). Keep at least 10 G free on the disk holding the target dir (memory `egret-target-dir-fills-the-disk`). An `E0432` that looks like a missing export is retried first with a private `CARGO_TARGET_DIR` (memory `shared-cargo-target-across-checkouts`).
- **Migration number: the next free number on main. It is 0175 today; re-check at rebase.** Its comments are final at its first commit, because sqlx hashes comments (`VersionMismatch`). The file is not edited after Task 2's commit except by a fix-round commit on this branch before merge, and the report says so.
- **The ratchet must not regress (294/108/108/0).** The line is `294 golden entries, 108 mismatching, 108 baselined, 0 broken` (`oracle-exec/README.md:9531`). `valids/retry-with-flowable-errors` also needs the `AllowFailure` flowable, so W9c alone cannot bank it, and `valids/kv` has no golden entry. A change is a finding to explain, not to bank.
- **Commit trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never commit `.superpowers/`.** Use Conventional Commits and the worktree's configured identity, never `--author`. Check `git branch --show-current` == `feat/workflow-w9c-kv` before every commit (memory `a-detached-head-passes-every-check`). Never commit jars, `plugin-host/target/`, scratch files or a mutant. **No PR. No push.**
- **Known gate noise: worker-lease / flow_trigger_windows / task_logs wall-clock flakes. Re-run those alone.** The executor golden test has its own load flake: under host load, `executor_matches_golden` has reported 109 with `valids/retry-expo` stuck `RETRYING`. A gate that shows **that signature** re-runs the executor binary alone and reports both runs. Every other failure belongs to the task until measured otherwise (memory `pre-existing-is-a-hypothesis`).
- **Reading the fixture.** In `w9c-kv.json`'s `egret_expected`, egret's answer is a cell's **top-level** value. A cell with `"basis": "chosen"` carries Kestra's measured answer under `kestra`, the reason under `why`, and the premises it rests on under `premise`, and is listed in `egret_expected_basis.chosen_cells` (dotted `path` and RFC 6901 `pointer`). A cell's `compare` names what is compared exactly and what is masked (ids, timestamps, generated names). A test asserts the top level, never `kestra`, and compares as `compare` says.
- **Layout (§3):** object key `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/_kv/{key}.ion` for revision 1, `…/{key}.ion.v{N}` for N ≥ 2. Staging is W9b's: `{prefix}{ws}/_nsfiles_staging/{uuid}`, in `workflow.namespace_file_staging`. **No URI names a KV value**: `storage::uri` does not change.
- **Caps:** `max_read_bytes` (10 MiB) bounds every encoded value, written and read. `max_namespace_files` (10 000) bounds a listing, `GetKeys` and a purge page. The API's bulk delete body is bounded by `kv::store::MAX_BULK_DELETE_KEYS` (**1,000**, the user's ruling, 2026-10-07), a constant and not a config key. `PurgeKV` covers at most 64 namespaces (`nsfiles::stage::MAX_NAMESPACES`). No new config key.
- **One access function, one key rule.**
  - `nsfiles::access::authorize(…, Resource::Kv, access)` is the only code that decides whether a namespace's KV may be read, written or purged. Every `storage::kv::store` function takes the `Grant` and calls `require(grant, Resource::Kv, access)`, so a namespace-file grant cannot reach KV and a Read grant cannot write. A second predicate anywhere is a review failure.
  - `storage::kv::key::check_key` (behind `KvKey::parse`) is the one key rule for the function, the tasks and the API.
  - The `_kv` segment rule is `nsfiles::access::check_kv_segment`, called by publish and by `check_namespace`.
  - **The workspace is always the execution's** (or, for the API, the route's `{ws}` after the permission check). Nothing in a property or an argument names a workspace.
- **Every refusal has a test that fails when the check is removed**, and the task names the mutant that kills it. A refusal never says whether a key exists in a namespace.
- **Errors (§11).** These are the author's: a failed task or a 4xx naming the key, namespace or cap, through `NsError::{Author, NotFound, Conflict, TooLarge}`.
  - the key rule; a refused namespace; a write-scope refusal; a malformed or negative `ttl`; a failed coercion; a merge refusal;
  - `expiredOnly: false`; a version purge with neither property; a crossed cap; `overwrite: false` over a live key;
  - missing with `errorOnMissing: true`; a missing object behind a row.

  An unreachable store or database is infrastructure (`NsError::{Store, Db}`): a handler returns `Err`, a render sets `infrastructure_failure`, a route answers 503. No message names a secret, an `s3://` key or another workspace.
- **Mutation discipline (as W9a and W9b):**
  - Every mutation shows evidence it applied (`git diff --stat` plus a token count), a run count, an unmutated control and the assertion that killed it.
  - A mutant that fails to compile proves nothing. A SQL, Java or YAML mutant is proven by running that artefact (memory `mutants-outside-the-compiler`).
  - Never commit a mutant, and never mutate while a build or test runs. Restore with `git checkout -- <file>` and confirm `git diff --stat` is empty (memory `a-mutation-can-be-invalidated-by-the-restore`).
- **Per-task gate** (every task, before its commit):
  - `cargo test -p egret-workflow --no-fail-fast` **and** `cargo test -p egret-server --lib` (`committed_openapi_json_is_current`);
  - `cargo fmt --all --check`;
  - `rtk proxy cargo clippy -p egret-workflow --all-targets --keep-going`, with **zero** `warning:` locations. Main has 0 today; Task 1 records the baseline, and any location is new.
  - A route or schema change regenerates `openapi.json` (`cargo run -p egret-bin -- openapi > openapi.json`) and the frontend client with `npm run gen:api` in `frontend/`, **never bare `orval`** (memory `orval-regeneration-strips-url-encoding`), and reports `git diff --stat frontend/src/api/generated`.
- **sqlx 0.9:** `sqlx::query*` take `&'static str`. Dynamic SQL is built with `concat!` from literals, never `format!` into a query. A key prefix is matched with `left(key, length($n)) = $n`, **never `LIKE`** (`_` and `.` are legal in keys; memory `sql-like-underscore-dropped-a-database`). Key ranges in SQL use `COLLATE "C"`.
- **Render-time reads need the multi-thread runtime:** every test that renders `kv()` against a real database or store is `#[tokio::test(flavor = "multi_thread")]`.
- **A zero is reported beside a number that can disagree.** Sweeps are reported as N examined / M corrected per population, run with `/usr/bin/grep` and file arguments (never `rtk grep` piped; memory `rtk-grep-swallows-stdin`).
- **Environment:**
  - Worktree `/Users/1pro-l4r8/Documents/github/data-portal/egret-w9c-kv`, branch `feat/workflow-w9c-kv`.
  - Export, for **every** cargo test (export it; memory `sqlx-test-writes-into-the-dev-database`):

    ```bash
    export DATABASE_URL=postgres://egret:egret@localhost:5440/egret_test
    export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6400
    export EGRET_TEST_S3_ENDPOINT=http://localhost:9001 EGRET_TEST_S3_KEY=egret EGRET_TEST_S3_SECRET=egretsecret EGRET_TEST_S3_BUCKET=warehouse
    export EGRET_TEST_STARROCKS_URL=mysql://localhost:9030
    ```

  - `JAVA_HOME=/Users/1pro-l4r8/Library/Java/JavaVirtualMachines/amazon-corretto-25.jdk/Contents/Home`. The plugin host is `plugin-host/`. Kestra's checkout is `/Users/1pro-l4r8/Documents/github/data-portal/kestra` at `96893b589`.
  - Commit before any suite that may run over ~120 s (memory `long-commands-are-auto-backgrounded`).
  - Never `docker image prune -a`. The end to end uses a **temporary** kind cluster `egret-w9c-e2e`.
  - Live tests are gated by env vars and report IGNORED (never passed) without them, as `tests/container_docker_live.rs` does.
  - Scratch files go in the SDD workspace, each prefixed `w9c-` (memory `parallel-agents-share-the-scratchpad`).

## Task map

| plan task | spec §12 | note |
|---|---|---|
| 1 Measure Kestra, and the baselines | 1 | — |
| 2 Table, key rule, `_kv` segment, `Resource::Kv`, reads | 2 | also the value encoding (`KvType`, `encode`, `decode`): a read decodes, and tests seed objects with `encode` |
| 3 Writes, staging, `Put`'s merge, purge, the expiry sweep | 3 | also `delete_many`, so Task 6 only wires it |
| 4 `kv()` and the secret refusal | 4 | also creates `worker/handlers/kv.rs` with only the six FQCN constants, which publish's secret scan needs before Task 5 |
| 5 The six tasks | 5 | also `runners::env::render_typed` and `value::coerce` |
| 6 The KV API | 6 | — |
| 7 The plugin host's messages | 7 | — |
| 8 Rebase, end to end, docs, full CI | 8 | — |

**Order:** 1 → 8. Each task's *Consumes* names what it needs from earlier ones.

## Where the spec is silent or does not fit the code, and what this plan does

Each item was reported to the user with the plan. **Ruled 2026-10-07:** items 6, 7, 8 and 9 by the user. Items 4, 5, 10, 12, 13 and 14 are recorded in the spec (EN and VI) as decisions or corrections. An implementer who finds that the code says otherwise stops and reports.

1. **`authorize` gains a parameter, and that touches every W9b caller.** `git grep -n "authorize(" -- crates/egret-workflow` lists 66 call lines at `e04e452d` (comments and the definition excluded), 23 of them in `access.rs`. Each gains `Resource::NamespaceFiles`. `require` (`access.rs:180`; 14 lines in `storage/nsfiles` with the definition) gains a `Resource` too, so a KV grant cannot reach a namespace-file store function either (Task 2).
2. **Two private modules have to open.** `executor::retry` is `mod retry;` (`executor/mod.rs:48`), so its Java-`Duration.parse` (`retry.rs:518`) is unreachable from `storage`; it becomes `pub(crate) mod retry;`. `runners::ionfmt` is `mod ionfmt;` (`runners/mod.rs:44`) and its JSON-to-Ion writer `write_ion_json` (`ionfmt.rs:266`) is private; both become `pub(crate)`. `ion-rs` stays a reader only: `Cargo.toml:191-197` records that its writer cannot produce Kestra's layout (Tasks 2, 5).
3. **egret has no `renderTyped`.** Every render answers a `String` (`runners::render_str`). Task 5 adds `runners::env::render_typed`: a value that is exactly one `{{ … }}` expression is rendered as `{{ (…) | toJson }}` (`filters.rs:181`) and parsed back, so its type survives; anything else renders as a string. Task 1's `typed_render` cells decide which templates come back typed.
4. **The first sweep pass.** W9b's staging sweeper uses `tokio::time::interval` (`staging.rs:199`), whose first tick fires at once. §11 says the KV sweep's first pass comes "after an hour", so `spawn_kv_expiry_sweeper` uses `interval_at(Instant::now() + every, every)` (Task 3).
5. **A key named `inheritance` cannot be read by `GET …/kv/{key}`.** The key rule allows it, and the static route `…/kv/inheritance` takes it, as in Kestra (`KVController.java:73`, `:103`). The plan follows Kestra: the static route wins, a test pins that `PUT` and `DELETE` of that key work and that its `GET` answers the inheritance listing, and Task 8 records it as a known limit. Task 1 measures what Kestra answers for that key.
6. **The bulk `DELETE` takes one advisory lock per key in one transaction** (§9). Advisory locks use Postgres's shared lock table, which by default holds `max_locks_per_transaction` (64) × `max_connections` slots, about 6 400 on a stock server, so a body of `max_namespace_files` (10 000) keys could fail with *out of shared memory*. **Ruled 2026-10-07: the bulk delete takes at most 1,000 keys per request** (`MAX_BULK_DELETE_KEYS`). It stays one all-or-nothing transaction, and a body over the cap answers 413 naming it. Tasks 3 and 6 test 1,000 accepted and 1,001 refused.
7. **The global hold rule versus the spec's server-paced windows.** `Put` holds a pool connection and the key's transaction lock across one store read and one store write, each at most `max_read_bytes` (§8.5). `Set` and the API's `PUT` hold them across one server-side copy and a `head` (§4.3). `PurgeKV` and the sweep hold a session lock across object deletes (§8.6, §11). **Ruled 2026-10-07: accepted** (spec §4.3, §15), as the explicit allowance in the Global Constraints' hold rule. Each hold audit still names every window.
8. **`Put` over an expired latest revision.** **Ruled 2026-10-07: an expired key is missing to `Put`.** `Put` starts fresh and carries over no description and no expiry; `errorOnMissing` decides. Spec §8.5 now agrees with §4.5. Task 1's `put_merge.over_expired` cell records Kestra's answer beside it, as a chosen cell if they differ.
9. **Another workspace's namespace.** **Ruled 2026-10-07:** it answers exactly as an empty namespace. The listings answer `200 []`; a key read or `revisions` answers `404`. Spec §9 now agrees with §13, and a test compares the bytes.
10. **§10 says `ShimRunContext.java:52`'s class doc mentions W9c. It does not** (`:52` reads "KV, secrets, encryption, ACLs and the application context throw `EgretUnsupported`"). W9c is named at `ShimRunContext.java:341`, `EgretUnsupported.java:4`, `EgretPluginHost.java:58`, `EgretPluginHostTest.java:125-126` and `kestra.rs:79`, `:379`, `:1317-1321`. Task 7 changes exactly those.
11. **The render's connection and the object read.** W9b's `Snapshot::ns_lookup` (`scope.rs:343-396`) holds the scope's connection mutex across the object read. `kv()` releases the mutex before reading the object. The connection itself belongs to the render scope (it is not pooled) and stays open until `end_render`, as W9b's does; no advisory lock is taken on a read (Task 4).
12. **§4.1's index on `(workspace_id, namespace, key, revision DESC)`** duplicates the unique constraint's index, which leads with the same columns and is scanned backwards for the latest row (W9b's item 8). The plan adds no duplicate, records `EXPLAIN` on 10 000 rows, and adds the partial `expires_at` index the spec asks for (Task 2).
13. **The PurgeKV judgement is taken under the key's lock.** §8.6 judges each page's keys, then purges per key under its lock. A key rewritten between listing and lock would then lose the revisions the listing saw, though it is live again. The plan lists keys from the page, then **re-reads the key's rows under its session lock** and judges them there, as §11 does for the sweep (Task 3). This is stricter than the spec, never looser.
14. **The audit's spelling.** §9 says "one audit mutation `workflow_namespace_kv:{ns}/{key}`". W9b's form is action `workflow_namespace_file:{verb}`, resource type `workflow_namespace_file`, resource id `{ns}:{path}` (`nsfiles/routes.rs:349-366`). The plan uses action `workflow_namespace_kv:{put|delete|delete_many}`, resource type `workflow_namespace_kv` and resource id `{ns}/{key}` (`{ns}` for a bulk delete, with the keys in the detail) (Task 6).

## File Structure

```text
migrations/0175_workflow_kv_entries.sql                       NEW  T2: workflow.kv_entries (re-checked at T8 Step 0)
crates/egret-workflow/src/storage/mod.rs                       MOD  T2: pub mod kv; FileStore::namespace_kv_uri
crates/egret-workflow/src/storage/kv/mod.rs                    NEW  T2: module doc, LOCK_PREFIX, kv_lock_text
crates/egret-workflow/src/storage/kv/key.rs                    NEW  T2: KvKey, check_key, MAX_KEY_BYTES (the one key rule)
crates/egret-workflow/src/storage/kv/value.rs                  NEW  T2: KvType, KvValue, encode, decode, Ttl, parse_ttl;
                                                                    T5: coerce; T6: parse_api_body
crates/egret-workflow/src/storage/kv/store.rs                  NEW  T2 reads (resolve, resolve_first_live, list_live,
                                                                    list_inherited, revisions, read_value, namespaces_with_kv,
                                                                    rows_page, ancestors); T3 writes (set, put_merge, delete,
                                                                    delete_many, purge_key)
crates/egret-workflow/src/storage/kv/merge.rs                  NEW  T3: Put's merge, against the fixture
crates/egret-workflow/src/storage/kv/sweep.rs                  NEW  T3: sweep_expired, spawn_kv_expiry_sweeper, KvSweepConfig
crates/egret-workflow/src/storage/kv/routes.rs                 NEW  T6: seven routes, DTOs
crates/egret-workflow/src/storage/nsfiles/access.rs            MOD  T2: Resource, Actor::System, KV_SEGMENT, check_kv_segment;
                                                                    authorize/require gain a Resource
crates/egret-workflow/src/storage/nsfiles/{store,stage,routes}.rs MOD T2: Resource::NamespaceFiles at every authorize/require;
                                                                    T3: store::detached pub(crate); T6: routes' helpers pub(crate)
crates/egret-workflow/src/storage/lock.rs                      MOD  T3: LockedSession::try_acquire
crates/egret-workflow/src/storage/scope.rs                     MOD  T2: Resource at :1 call; T4: Snapshot::kv, Unconfigured::kv
crates/egret-workflow/src/executor/mod.rs                      MOD  T2: `pub(crate) mod retry;` (:48)
crates/egret-workflow/src/runners/mod.rs                       MOD  T2: `pub(crate) mod ionfmt;` (:44)
crates/egret-workflow/src/runners/ionfmt.rs                    MOD  T2: write_ion_json pub(crate) (:266)
crates/egret-workflow/src/runners/files.rs                     MOD  T4: KvArgs, KvAnswer, RenderFiles::kv
crates/egret-workflow/src/runners/functions.rs                 MOD  T4: kv(); REFUSED to six (:273-290); tests (:3325-3400)
crates/egret-workflow/src/runners/env.rs                       MOD  T5: render_typed
crates/egret-workflow/src/plugins/mod.rs                       MOD  T4: CODE_SECRET_IN_KV_VALUE (beside :173)
crates/egret-workflow/src/spec/validate.rs                     MOD  T2: `_kv` publish rule (validate_namespace :627);
                                                                    T4: secret_in_kv_value in secret_findings (:336-457)
crates/egret-workflow/src/worker/handlers/kv.rs                NEW  T4: the six FQCN constants; T5: the six handlers, DECLARED, register
crates/egret-workflow/src/worker/handlers/mod.rs               MOD  T4: pub mod kv (beside :46)
crates/egret-workflow/src/worker/handlers/{namespace,script}.rs MOD T2: Resource::NamespaceFiles at each authorize
crates/egret-workflow/src/worker/handlers/kestra.rs            MOD  T7: exit-4 message (:379), module doc (:79), test (:1317-1321)
crates/egret-workflow/src/spec/registry.rs                     MOD  T5: six TaskTypeSpec rows after W9b's (:255-271)
crates/egret-workflow/src/runtime.rs                           MOD  T5: kv::register beside namespace::register (:731)
crates/egret-workflow/src/routes.rs                            MOD  T6: router (management_router :2403) + OpenAPI (:2499)
crates/egret-bin/src/main.rs                                   MOD  T3: spawn_kv_expiry_sweeper beside the staging sweeper (:1871)
crates/egret-workflow/oracle-exec/{w9c-kv.json,docker-compose.w9c.yml}  NEW T1
crates/egret-workflow/oracle-exec/{driver.rs,README.md,handler-surface.txt}  MOD T5 (driver :139-170), T8 (README)
crates/egret-workflow/tests/{kv_store.rs,kv_render.rs,kv_tasks.rs,kv_api.rs,kv_s3_live.rs}  NEW
crates/egret-workflow/tests/{nsfiles_*.rs,storage_prune.rs,container_docker_live.rs,k8s_runner_live.rs,plugin_host_live.rs,publish.rs}  MOD T2 (Resource), T4 (publish)
plugin-host/src/main/java/io/kestra/core/runners/{ShimRunContext.java,EgretUnsupported.java,EgretPluginHost.java}  MOD T7
plugin-host/src/test/java/io/kestra/core/runners/EgretPluginHostTest.java  MOD T7 (:125-126)
openapi.json, frontend/src/api/generated/**                    REGEN T6 (`npm run gen:api`)
docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md     MOD  T8 (§9 W9 row :489, a W9c correction after W9b's :513-519)
docs/{en,vi}/specs/2026-10-07-workflow-w9c-kv-store-design.md  MOD  T1 (only where measured otherwise), T8 (dated corrections)
```

---

## Task 1: Measure Kestra, and the baselines

**Files:**
- Create `crates/egret-workflow/oracle-exec/w9c-kv.json` and `crates/egret-workflow/oracle-exec/docker-compose.w9c.yml`.
- Reuse `oracle-exec/docker-compose.yml` (the artifact `kestra/kestra@sha256:28f59bf1…`, pinned by digest) and `harvest.py`'s `TENANT`/`USER`/`PASSWORD` client.
- Amend spec §2, §3, §4.2, §4.6, §8, §9 (EN and VI) **only** where a measurement contradicts them, dated. Every measured difference from Kestra in a read result becomes its own §2 row (the spec header asks for this).

**Interfaces:**
- Produces `w9c-kv.json`, shaped like `w9b-namespace-files.json`: `artifact`, `artifact_note`, `captured_at`, `method`, `how_to_read_for_egret`, `findings` (quoted values), `egret_expected` (typed, below), `egret_expected_basis` (a rule per key, plus `rule` and `chosen_cells`), and `raw_records`.
- Later tasks read it with `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w9c-kv.json"))`.
- Every cell carries `compare` (what is exact, what is masked) and, where it depends on earlier state, `premise`.

```yaml
# oracle-exec/docker-compose.w9c.yml
# W9c Task 1: the pinned artifact on host port 18090, so it can run beside a W9b
# stack (18080) or an egret dev server (8080).
#
#   docker compose -p w9coracle -f docker-compose.yml -f docker-compose.w9c.yml up -d postgres kestra
#
# Compose concatenates `ports` across files, so the base mapping is replaced with
# `!override`. Nothing else changes: same image digest, configuration and volume.
services:
  kestra:
    ports: !override
      - "18090:8080"
```

| `egret_expected` key | shape | consumed by |
|---|---|---|
| `schema` | per class of `Set, Get, GetKeys, Delete, Put, PurgeKV`: properties (`required`, `default`) from `GET /api/v1/main/plugins/{cls}`, outputs | T5 |
| `keys` | per raw key (`""`, `a`, `A-1._x`, `_a`, `.a`, `-a`, `a/b`, `a b`, `é`, a 350-char key, a 351-char key, `inheritance`) → the API `PUT` status and message, and `Set`'s failure | T2, T6 |
| `types` | for STRING, NUMBER (integer, decimal, a 20-digit integer), BOOLEAN, DATETIME, DATE, DURATION (`P1D`, `PT36H`), JSON (object, array, nested): the object's **exact bytes** (`docker exec <kestra> cat /app/storage/main/<ns>/_kv/<key>.ion`), what `kv()` prints (`Return.format`), `Get`'s `outputs.value` (JSON) and the API's `KvDetail` | T2, T4, T5, T6 |
| `set_coercion` | each `kvType` × a string value: success and stored bytes, or the failure message; BOOLEAN `TRUE`, `yes`, `1`; a Number with `STRING`; a Boolean with `NUMBER`; a JSON object with `STRING`; DATE from an instant | T5 |
| `typed_render` | `value` of `{{ 1 + 1 }}`, `{{ 'a' }}`, `x {{ 1 }}`, `{{ [1,2] }}`, `{{ {'a':1} }}`, `{{ true }}`, `{{ null }}`, a YAML map with a templated leaf, a YAML list → the stored type and bytes | T5 |
| `ttl` | `ttl: PT0S`, `-PT1S`, `PT2S`, `P1D`, `xyz` → stored `expires_at` (via the API's detail) or the failure | T5 |
| `overwrite` | `overwrite: false` over a live key (message), over a deleted key, over an expired key | T3, T5 |
| `put_merge` | the cells of §8.5: map into map, nested maps, an array inside a map, map over scalar, scalar over map, `[{key, value}]` dotted patch (existing path, a missing level), a single `[{value}]`, a missing key under each `errorOnMissing`, `over_expired` (Put on an expired key) → the stored value, description and expiry kept | T3, T5 |
| `expiry` | a key with `ttl: PT2S`, read after 3 s by `kv()` and `Get` under each `errorOnMissing`, by `GetKeys` and by the API (`GET …/kv/{key}`, `DELETE`): each answer, then the row (`kv_metadata`, read as Step 2.6 says) and the storage tree afterwards | T3, T4, T5 |
| `inheritance` | namespaces `w9c.inh`, `w9c.inh.a`, `w9c.inh.a.b`, keys at different levels (one expired at the deepest): `kv(key)`, `kv(key, flow.namespace)`, `Get` with the flow's namespace, `Get` with a parent's; the missing-key messages of each; the API's `…/kv/inheritance` body | T2, T4, T5, T6 |
| `kv_function` | `kv()` without `key`; `kv(key, ns, 'x')` (a non-boolean `errorOnMissing`); `kv(namespace=…, key=…)` named; missing under each default | T4 |
| `get_keys` | `GetKeys` of a namespace holding `b`, `a`, `a.1`, `A`, `a_b` (one deleted, one expired), with and without `prefix`: the output **order** | T2, T5 |
| `delete` | `Delete` of a live, a missing, a deleted and an expired key under each `errorOnMissing`; the revisions afterwards | T3, T5 |
| `purge` | `PurgeKV` with each behaviour: `{type: key}` default, `expiredOnly: false`, `{type: version, keepAmount: 1}`, `before`, `keepAmount: 0`, **`{type: version}` with neither property**; `keyPattern`; `namespaces`, `namespacePattern`, `includeChildNamespaces` (`w9c.pp.a` vs `w9c.ppx`); the output `size` (revisions or keys) | T3, T5 |
| `api` | every `KVController` route: status, headers and body. Includes `PUT` with an Ion body (`{a:1}`, `1`, `1.5`, `"s"`, `2025-01-01T00:00:00Z`, `true`), a non-Ion body (`hello world`), the `description` and `ttl` headers; `GET` of a soft-deleted key; both deletes; the bulk body's answer for a missing key | T6 |
| `layout` | the storage tree after write, rewrite, rewrite, delete, rewrite, purge: every object key and revision | T2, T3 |
| `kv_segment_namespaces` | `{published_flows: [..], corpus_entries: [..], count_published, count_corpus, examined_published, examined_corpus}` | T2 |
| `function_kv_oracle` | the expression oracle's `function-kv` cell (`oracle/golden.json:2200-2206`, `compare: error-class`) and egret's answer at a render with no execution | T4 |

- [ ] **Step 0: Bring up this branch's own containers**, if absent, and record `docker ps` for each:

```bash
docker run -d --name egret-w9c-postgres -p 5440:5432 -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret -e POSTGRES_DB=egret_test postgres:16
docker run -d --name egret-w9c-valkey -p 6400:6379 valkey/valkey:8
docker run -d --name egret-w9c-minio -p 9001:9000 -e MINIO_ROOT_USER=egret -e MINIO_ROOT_PASSWORD=egretsecret minio/minio:latest server /data
docker exec egret-w9c-minio sh -c 'mc alias set local http://localhost:9000 egret egretsecret && mc mb -p local/warehouse'
```

- [ ] **Step 1: Bring up the artifact.** Run `docker compose -p w9coracle -f docker-compose.yml -f docker-compose.w9c.yml up -d postgres kestra` in `oracle-exec/`, then confirm that `GET http://localhost:18090/api/v1/main/plugins` lists `io.kestra.plugin.core.kv.Put` and `PurgeKV`. **Hazard (W13b):** never `docker exec psql` as a child of the artifact's postmaster. If the image refuses a request shape, **stop and report**; do not switch images.
- [ ] **Step 2: Measure** every row of the table, saving raw JSON, bytes and headers into `raw_records`:
  1. `schema`: the plugin API's JSON schema per class.
  2. `keys`, `types`, `api`, `layout`: through `KVController`; bytes by `docker exec <kestra> find /app/storage -path '*_kv*' -printf '%y %p %s\n'` and `cat` (hex-dumped into `raw_records` for non-ASCII).
  3. Functions and tasks: one flow per family, a `Return` task (`allowFailure: true`) per `kv()` cell, then `GET /api/v1/main/outputs/…`; a failure records the task's first ERROR log.
  4. `expiry`: `Set` with `ttl: PT2S`, `sleep 3`, then every reader, in a fixed order, each in its own execution. Record whether the **first** reader changed what the second saw (Kestra soft-deletes on read, §2).
  5. `purge`: seed through the API, run `PurgeKV`, then list the storage tree and `GET …/kv/{key}` per key.
  6. Kestra's rows (`expiry`, `delete`, `purge`): read **read-only** from a client container on the compose network, never as a child of the postmaster: `docker run --rm --network w9coracle_default postgres:18 psql postgresql://kestra:k3str4@postgres:5432/kestra -c "SELECT namespace, name, version, last, deleted, expiration_date FROM kv_metadata ORDER BY 1,2,3"`. Check the column names against `baseline-postgres.sql:383-409` first.
  7. `kv_segment_namespaces`, both counted beside the totals examined:
     - corpus: `/usr/bin/grep -rn "^namespace:" crates/egret-workflow/oracle-exec/corpus`, filtered for a `_kv` segment (a segment = between dots);
     - published flows: **one read-only** `SELECT workspace_id, namespace, flow_id FROM workflow.flows WHERE '_kv' = ANY(string_to_array(namespace, '.'))` on the dev database (`egret-dev-postgres-1`), and the same on `:5440`, each beside `SELECT count(*) FROM workflow.flows`. None is expected.
  8. `function_kv_oracle`: copy the golden cell verbatim, and note that egret's refusal without an execution is an error (the `error-class` compare).
- [ ] **Step 3: Save** the fixture with timestamps and the image digest. Where a question cannot be measured, say so in `findings` and mark egret's answer `"basis": "chosen"`. Mark as chosen every cell where §2 already decides egret differs: expiry, the write scope, `expiredOnly: false`, a version purge with neither property, the stored type, the bulk delete's all-or-nothing, the explicit-revision read of a soft-deleted key, and the `PurgeKV` default namespaces.
  - If `types` shows Kestra storing something other than Ion text (or raw ISO for DURATION) for any type, **stop and report**: §3 and §4.2 assume it.
  - If `expiry` shows `kv()` or `Get` answering a live value after expiry, **stop and report**: §4.5's premise is gone.
  - If Kestra's `GetKeys` order is not by key in byte order, record the order and follow it (spec §8.3).
- [ ] **Step 4:** `docker compose -p w9coracle … down -v`.
- [ ] **Step 5: Baseline gate** at the branch base, with the Global Constraints' env, into the SDD workspace:
  - `cargo test -p egret-workflow --no-fail-fast 2>&1`, its name/result list saved as `w9c-baseline-gate.txt`;
  - `cargo test -p egret-server --lib`, which must be green at the base. If `committed_openapi_json_is_current` is red there, **stop and report**;
  - the ratchet: `cargo test -p egret-workflow --test executor executor_matches_golden -- --nocapture`, its line saved as `w9c-baseline-ratchet.txt`. Expected: `294 golden entries, 108 mismatching, 108 baselined, 0 broken`. A different line is reported before anything else;
  - `valids/retry-with-flowable-errors` (`baseline.txt:614`) and `valids/kv`: their current rows, verbatim;
  - clippy, with rustc 1.99 (`rustc --version` recorded):
    - `rtk proxy cargo clippy --workspace --all-targets --keep-going 2>&1` → the sorted `warning:` locations as `w9c-baseline-clippy-warn.txt`. Expected **0**; the bar for every later task is 0 new.
    - `rtk proxy cargo clippy --workspace --all-targets -- -D warnings 2>&1` → its `error:` locations as `w9c-baseline-clippy-deny.txt`.
- [ ] **Step 6:** Commit `docs(workflow): W9c Task 1 — the KV store measured on the artifact` (the fixture, the compose file, any dated spec amendment).

---

## Task 2: Table, key rule, `_kv` segment, `Resource::Kv`, and reads

**Files:**
- Create `migrations/0175_workflow_kv_entries.sql`, `src/storage/kv/{mod.rs,key.rs,value.rs,store.rs}` and `tests/kv_store.rs`.
- Modify `src/storage/mod.rs` (`pub mod kv;` beside `:40`; `FileStore::namespace_kv_uri` after `namespace_object_uri` `:130-156`) and `src/storage/nsfiles/access.rs` (`:22-189`).
- Modify every `authorize(`/`require(` caller (item 1): `nsfiles/{routes.rs,stage.rs,store.rs}`, `storage/scope.rs`, `worker/handlers/{namespace.rs,script.rs}`, and the tests `nsfiles_api_s3_live.rs`, `nsfiles_render.rs`, `nsfiles_s3_live.rs`, `nsfiles_store_ddl.rs`, `nsfiles_store.rs`, `nsfiles_tasks.rs`, `container_docker_live.rs`, `k8s_runner_live.rs`, `plugin_host_live.rs` and `storage_prune.rs`.
- Modify `src/spec/validate.rs` (`validate_namespace` `:627-653`), `src/executor/mod.rs:48`, `src/runners/mod.rs:44` and `src/runners/ionfmt.rs:266`.

**Interfaces:**
- Consumes (W9b): `nsfiles::{NsError, CreatedBy}` (`nsfiles/mod.rs:41-74`), `access::{authorize, Actor, Access, Grant, check_namespace, check_files_segment, API_LOCK_WAIT}`, `store::Want` (`store.rs:51`), `FileStore`, `uri::namespace_as_path`, `storage::ion::read_values_capped` (`ion.rs:106`), `runners::ionfmt::write_ion_json`.
- Produces:

```sql
-- migrations/0175_workflow_kv_entries.sql (exact DDL; comments final at first commit)
-- W9c: the KV store (docs/en/specs/2026-10-07-workflow-w9c-kv-store-design.md §4.1).
-- One row per revision; a key's state is its highest revision's row. A KV value exists only
-- once its row exists. No FK to a namespace: there is no namespace table, and a namespace
-- with no flow can hold keys. The unique constraint's index is also the latest-row index.
CREATE TABLE workflow.kv_entries (
  id                   bigserial   PRIMARY KEY,
  workspace_id         uuid        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  namespace            text        NOT NULL CHECK (namespace <> ''),
  -- storage::kv::key::check_key: Kestra's regex (KVStore.java:137), at most 350 bytes
  key                  text        NOT NULL
    CHECK (octet_length(key) <= 350 AND key COLLATE "C" ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]*$'),
  revision             integer     NOT NULL CHECK (revision >= 1),
  value_type           text        NOT NULL
    CHECK (value_type IN ('STRING','NUMBER','BOOLEAN','DATETIME','DATE','DURATION','JSON')),
  size                 bigint      NOT NULL CHECK (size >= 0),
  description          text,
  -- now() + ttl at the commit, by the database's clock; NULL never expires
  expires_at           timestamptz,
  deleted              boolean     NOT NULL DEFAULT false,
  deleted_at           timestamptz,
  -- who soft-deleted it: a re-claimed Delete recognises its own earlier delete (§8.4, §11)
  deleted_by_user      uuid,
  deleted_by_execution uuid,
  deleted_by_task_run  uuid,
  created_at           timestamptz NOT NULL DEFAULT now(),
  created_by_user      uuid,
  created_by_execution uuid,
  created_by_task_run  uuid,
  CONSTRAINT workflow_kv_entries_revision_key UNIQUE (workspace_id, namespace, key, revision),
  CONSTRAINT workflow_kv_entries_one_writer
    CHECK (num_nonnulls(created_by_user, created_by_execution) <= 1),
  CONSTRAINT workflow_kv_entries_task_run_of_an_execution
    CHECK (created_by_task_run IS NULL OR created_by_execution IS NOT NULL),
  CONSTRAINT workflow_kv_entries_deleted_shape
    CHECK (deleted = (deleted_at IS NOT NULL)
           AND (deleted OR num_nonnulls(deleted_by_user, deleted_by_execution, deleted_by_task_run) = 0)),
  CONSTRAINT workflow_kv_entries_one_deleter
    CHECK (num_nonnulls(deleted_by_user, deleted_by_execution) <= 1
           AND (deleted_by_task_run IS NULL OR deleted_by_execution IS NOT NULL))
);
-- The expiry sweep's scan (§11).
CREATE INDEX workflow_kv_entries_expires_at ON workflow.kv_entries (expires_at)
  WHERE expires_at IS NOT NULL;
```

```rust
// src/storage/nsfiles/access.rs — ONE access decision, two resources (§4.7)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource { NamespaceFiles, Kv }
impl Resource {
    /// "namespace files" / "the KV store": the refusal's noun.
    pub fn noun(self) -> &'static str;
    /// "files" / "KV store": "a flow writes only its own namespace's {own_noun}".
    fn own_noun(self) -> &'static str;
}
pub enum Actor<'a> {
    Flow { namespace: &'a str },
    Api,
    /// NEW: egret's own background work (the KV expiry sweep, Task 3): any namespace,
    /// any access, no lock wait (it only try-locks). Never built from a request.
    System,
}
pub struct Grant { workspace: Uuid, namespace: String, resource: Resource, access: Access, lock_wait: Option<Duration> }
impl Grant { /* existing getters */ pub fn resource(&self) -> Resource; }
/// `_kv`, the KV marker segment (§3).
pub const KV_SEGMENT: &str = "_kv";
/// No `_kv` segment: publish (validate.rs) and, through check_namespace, every KV and
/// namespace-file operation call it. The message names the namespace and the segment.
pub fn check_kv_segment(namespace: &str) -> Result<(), String>;
/// is_valid_namespace, no empty segment, check_files_segment, THEN check_kv_segment.
pub fn check_namespace(namespace: &str) -> Result<(), String>;
/// The rule is unchanged and the same for both resources; the resource changes only the
/// refusal's words and is recorded in the Grant. Actor::System is granted like Actor::Api,
/// with lock_wait None.
pub fn authorize(workspace: Uuid, actor: Actor<'_>, target: &str, resource: Resource, access: Access)
    -> Result<Grant, String>;
/// Err(Internal) when the grant's resource is not `resource`, or a write is attempted with a
/// Read grant -- egret's bug, not the author's.
pub(crate) fn require(grant: &Grant, resource: Resource, access: Access) -> egret_core::Result<()>;

// src/storage/kv/key.rs — THE key rule (§3)
pub const MAX_KEY_BYTES: usize = 350;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KvKey(String);
impl KvKey {
    /// check_key, then the key. Never trims.
    pub fn parse(raw: &str) -> Result<Self, String>;
    pub fn as_str(&self) -> &str;
}
impl std::fmt::Display for KvKey { /* the key */ }
/// Kestra's messages (w9c `keys`) for an empty key and a key that breaks
/// ^[a-zA-Z0-9][a-zA-Z0-9._-]*$; over 350 bytes: "key '<first 40>…' is N bytes long; a KV key
/// is at most 350 bytes (Kestra's own column width)".
pub fn check_key(raw: &str) -> Result<(), String>;

// src/storage/kv/value.rs — types and the bytes Kestra writes (§4.2)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KvType { String, Number, Boolean, Datetime, Date, Duration, Json }
impl KvType {
    pub const ALL: [KvType; 7];
    /// "STRING" … "JSON": the column's spelling and Kestra's enum names.
    pub fn as_str(self) -> &'static str;
    /// A kvType property value, as Kestra's enum binding accepts it (case as w9c `set_coercion` measured).
    pub fn parse(s: &str) -> Option<Self>;
    /// The type of a value after coercion (§4.2): string -> STRING, number -> NUMBER, boolean ->
    /// BOOLEAN, object or array -> JSON, `null` -> STRING (Kestra stores a rendered `null` as Ion
    /// `null`, typed STRING: w9c `typed_render.tr_null`). Never DURATION, DATETIME or DATE: those
    /// come only from `coerce` of a String with that kvType (amended 2026-10-07, Task 1 and its review C1).
    pub fn of(v: &serde_json::Value) -> Self;
}
/// A value as egret stores and reads it back: DATETIME, DATE and DURATION are JSON strings in
/// the form w9c `types.<T>.kv_prints` measured (§4.2, §15).
#[derive(Clone, Debug, PartialEq)]
pub struct KvValue { pub kind: KvType, pub json: serde_json::Value }
/// The object's bytes, byte-equal to w9c `types.<T>.ion_bytes`: Ion text through
/// runners::ionfmt::write_ion_json for every type (a DURATION is an Ion string, `"P1D"` with its
/// quotes), except a DATETIME or DATE, written as the Ion timestamp literal the fixture shows (UTC,
/// millisecond precision; a DATE at day precision). Amended 2026-10-07 (Task 1).
pub fn encode(v: &KvValue) -> Vec<u8>;
/// The bytes back to the JSON value of `kind`: exactly one Ion value through
/// storage::ion::read_values_capped (bounded by `cap`), then normalised for DATETIME/DATE as
/// `encode` wrote them and for DURATION as w9c `types._rule_read` prints it. Err names what is
/// wrong, never a storage key. Amended 2026-10-07 (Task 1: a DURATION is one Ion string).
pub fn decode(kind: KvType, bytes: &[u8], cap: u64) -> Result<serde_json::Value, String>;
/// Where a ttl comes from: the two measured behaviours differ (Task 1 review I3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ttl {
    /// The tasks' `ttl` property (Set): zero and negative are both refused with
    /// `ttl: must be a positive duration` (w9c `ttl.PT0S`, `ttl.-PT1S`); a malformed one names the
    /// value and `ttl` (w9c `ttl.xyz`).
    Task,
    /// The API's `ttl` header: zero is ACCEPTED (the value is expired at once, w9c
    /// `api.headers.h_ttl_zero`); a negative one is `ttl cannot be negative` (`h_ttl_neg`, exact);
    /// a malformed one `Cannot convert value '<v>' to Duration` (`h_ttl_bad`, exact).
    Header,
}
/// An ISO-8601 duration as java.time.Duration.parse reads it (crate::executor::retry::parse_duration,
/// retry.rs:518), then zero and the sign judged by `mode`. Err is the message for that mode.
pub fn parse_ttl(raw: &str, mode: Ttl) -> Result<chrono::TimeDelta, String>;

// src/storage/kv/mod.rs
pub mod key; pub mod merge /* T3 */; pub mod routes /* T6 */; pub mod store; pub mod sweep /* T3 */; pub mod value;
/// The advisory-lock text of one key, bound as a value: "w9c:{ws-hyphenated}:{ns}:{key}" (§4.3).
pub(crate) fn kv_lock_text(grant: &crate::storage::nsfiles::access::Grant, key: &key::KvKey) -> String;

// src/storage/mod.rs
impl FileStore {
    /// s3://{bucket}/{prefix}{workspace}/{ns-as-path}/_kv/{key}.ion[.v{revision}] (§3).
    /// The workspace and namespace are the grant's; debug_assert!(grant.resource() == Resource::Kv).
    pub fn namespace_kv_uri(&self, grant: &nsfiles::access::Grant, key: &kv::key::KvKey, revision: u32) -> String;
}

// src/storage/kv/store.rs — the read side
#[derive(Clone, Debug, PartialEq)]
pub struct KvEntry {
    pub id: i64, pub workspace: Uuid, pub namespace: String, pub key: KvKey, pub revision: u32,
    pub value_type: KvType, pub size: u64, pub description: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>, pub deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// expires_at <= now() by THE DATABASE's clock, computed in the same statement (§4.5).
    pub expired: bool,
    /// The task run that wrote it, and the one that deleted it (re-claims, §11).
    pub created_by_task_run: Option<Uuid>, pub deleted_by_task_run: Option<Uuid>,
}
impl KvEntry { pub fn live(&self) -> bool { !self.deleted && !self.expired } }
/// Latest: the highest revision, which must be live (deleted or expired -> NotFound, the same
/// message as a key never written). Revision(n): that row, served when soft-deleted, NotFound
/// when expired or absent (§4.4, §4.5). The message names the key and namespace only.
pub async fn resolve(conn: &mut PgConnection, grant: &Grant, key: &KvKey, want: Want) -> Result<KvEntry, NsError>;
/// The inheritance walk (§4.6): `grants` deepest first, all of one workspace, all Read on Kv.
/// ONE statement reads every level's latest row; answers the first LIVE one, or None.
pub async fn resolve_first_live(conn: &mut PgConnection, grants: &[Grant], key: &KvKey) -> Result<Option<KvEntry>, NsError>;
/// `ns` and its parents, deepest first, cut at dots: "a.b.c" -> ["a.b.c", "a.b", "a"].
pub fn ancestors(ns: &str) -> Vec<&str>;
/// Live latest rows of the namespace, in the order w9c `get_keys` measured (byte order by
/// default; amended 2026-10-07, Task 1: the order the latest revisions were written, i.e. the latest
/// row's id ascending, NOT byte order), whose key starts with `prefix` (left(key, length($3)) = $3). At most cap + 1 are
/// fetched; the +1 is TooLarge naming workflow.storage.max_namespace_files and the count seen.
pub async fn list_live(conn: &mut PgConnection, grant: &Grant, prefix: Option<&str>, cap: u64) -> Result<Vec<KvEntry>, NsError>;
/// The API's inheritance listing: `grants` are the ANCESTORS (not the namespace itself), deepest
/// first; the live keys of each, the first occurrence of a key winning (KVController.java:73-100).
/// Bounded by `cap` over the union.
pub async fn list_inherited(conn: &mut PgConnection, grants: &[Grant], cap: u64) -> Result<Vec<KvEntry>, NsError>;
/// Every row of the key, ascending, deleted and expired flagged. No row: NotFound.
pub async fn revisions(conn: &mut PgConnection, grant: &Grant, key: &KvKey) -> Result<Vec<KvEntry>, NsError>;
/// Every row of the namespace by (key, revision), strictly after `after`; at most cap + 1 (T3's purge).
pub async fn rows_page(conn: &mut PgConnection, grant: &Grant, after: Option<(&KvKey, u32)>, cap: u64) -> Result<Vec<KvEntry>, NsError>;
/// The namespaces of `workspace` holding at least one row (PurgeKV's candidates, T5).
pub async fn namespaces_with_kv(conn: &mut PgConnection, workspace: Uuid) -> Result<Vec<String>, NsError>;
/// The entry's object, decoded. size > max_read_bytes: TooLarge naming the cap (no download).
/// A missing object: Author naming the key and the revision (no fallback, §4.5). Any other store
/// error: Store. Takes NO connection: call it after the row was read and the connection let go.
pub async fn read_value(fs: &FileStore, grant: &Grant, e: &KvEntry) -> Result<serde_json::Value, NsError>;
```

- [ ] **Step 1: Write the failing tests.**

```rust
// src/storage/kv/key.rs (unit) -- one test per refusal
#[test] fn the_keys_match_the_artifact() { /* every w9c `keys` row: accepted or the measured message */ }
#[test] fn a_key_of_350_bytes_is_accepted_and_351_is_refused_naming_the_bound() {}
#[test] fn an_empty_key_is_refused_with_kestras_message() {}
#[test] fn a_leading_dot_dash_or_underscore_is_refused() { /* "_a" can never be a marker's name */ }
#[test] fn a_slash_space_or_non_ascii_letter_is_refused() { /* "a/b", "a b", "é" */ }

// src/storage/kv/value.rs (unit)
#[test] fn encode_is_byte_equal_to_the_artifact_for_every_type() { /* w9c types.*.ion_bytes */ }
#[test] fn decode_reads_back_what_encode_wrote_for_every_type() { /* round trip, every types row */ }
#[test] fn decode_of_the_artifacts_bytes_answers_what_kestras_kv_printed() { /* types.*.kv_prints; a mismatch is a §2 row */ }
#[test] fn a_duration_is_stored_as_an_ion_string() { /* "P1D" -> b"\"P1D\"" (w9c types.dur_p1d) */ }
#[test] fn decode_refuses_two_values_and_none() {}
#[test] fn parse_ttl_task_refuses_zero_and_negative_as_not_positive() { /* w9c ttl.PT0S, ttl.-PT1S, ttl.xyz */ }
#[test] fn parse_ttl_header_accepts_zero_and_refuses_negative_with_its_own_message() { /* w9c api.headers.h_ttl_zero, h_ttl_neg, h_ttl_bad */ }

// src/storage/nsfiles/access.rs (unit) -- added beside W9b's
#[test] fn a_kv_segment_namespace_is_refused_for_every_resource_access_and_actor() { /* "a._kv", "a._kv.b"; "_kv" is invalid anyway */ }
#[test] fn a_flow_reads_any_kv_namespace_and_writes_only_its_own() {}
#[test] fn a_flow_purges_its_own_and_child_kv_namespaces_on_a_dot_boundary() { /* own "a.b": "a.b.c" yes, "a.bc" NO, "a" NO */ }
#[test] fn the_resource_changes_only_the_words() { /* the same pairs refused for both; messages differ by noun only */ }
#[test] fn require_refuses_a_grant_of_the_other_resource() { /* NamespaceFiles grant -> require(Kv, Read) is Err(Internal) */ }
#[test] fn a_kv_refusal_never_mentions_existence() { /* no "exist", "found", "missing" */ }

// src/spec/validate.rs (unit) + tests/publish.rs
#[test] fn publish_refuses_a_kv_segment_namespace_by_name() { /* code `namespace_kv_segment`, path `namespace` */ }
#[test] fn a_flow_id_of_kv_is_refused() { /* "_kv": invalid_flow_id, as `a_flow_id_of_files_is_refused` (:2372) */ }
#[test] fn valids_kv_still_publishes() { /* oracle-exec/corpus/valids/kv.yaml: no error finding */ }

// tests/kv_store.rs (Postgres :5440 + MemStore; rows inserted with plain SQL in this task)
#[tokio::test] async fn latest_is_the_highest_live_revision() {}
#[tokio::test] async fn a_deleted_latest_is_not_found_and_each_revision_is_served() { /* R1 */ }
#[tokio::test] async fn an_expired_latest_reads_exactly_as_a_key_never_written() {
    // expires_at = now() - 1s: resolve(Latest) NotFound with the same message as an absent key;
    // Revision(n) of the expired row NotFound; list_live leaves it out
}
#[tokio::test] async fn a_read_never_writes() {
    // row count, every column of every row (a hash) and the object list are identical before and
    // after resolve, resolve_first_live, list_live, list_inherited, revisions, read_value of an
    // expired, a deleted and a live key
}
#[tokio::test] async fn the_walk_answers_the_deepest_live_level_and_skips_an_expired_one() {
    // w9c inheritance cells; one statement (counted with a query counter or pg_stat_statements is
    // NOT available: count the round-trips through a wrapped connection in the test)
}
#[tokio::test] async fn the_same_key_in_two_workspaces_resolves_to_the_grants_row_only() {}
#[tokio::test] async fn list_live_refuses_one_past_the_cap_naming_it() { /* cap 3, 4 live -> TooLarge; 3 -> 3 */ }
#[tokio::test] async fn list_live_prefix_is_a_plain_starts_with() { /* prefix "a_" must not match "ab" (a LIKE would) */ }
#[tokio::test] async fn list_inherited_is_deepest_first_and_the_first_occurrence_wins() {}
#[tokio::test] async fn get_keys_order_matches_the_artifact() { /* w9c get_keys */ }
#[tokio::test] async fn a_row_whose_object_is_missing_is_an_author_error_naming_key_and_revision() {}
#[tokio::test] async fn a_stored_value_over_max_read_bytes_is_too_large_naming_the_cap_without_a_download() {}
#[tokio::test] async fn the_key_check_refuses_what_check_key_refuses() { /* CHECK by SQL: 351 bytes, "é", "_a", "" */ }
#[tokio::test] async fn the_one_writer_and_deleted_shape_checks_hold() { /* each CHECK by SQL */ }
#[tokio::test] async fn deleting_a_workspace_cascades_its_kv_entries() {}
#[tokio::test] async fn the_latest_row_query_uses_the_unique_index() { /* EXPLAIN on 10 000 rows: Index Scan Backward, recorded */ }
#[tokio::test] async fn the_kv_object_key_is_kestras_layout() { /* namespace_kv_uri vs w9c layout: .ion, .ion.v2 */ }
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **`access.rs`:** add `Resource` and `Actor::System`; `Grant` gains `resource`. The three refusal sentences of `authorize` (`:143-158`) take `resource.noun()` and `own_noun()`. For `Resource::NamespaceFiles` they must stay **byte-identical** to today's (W9b's tests assert them); run `nsfiles_*` before and after and report both counts. `require` checks the resource first, then the access rule as today.
  - **Every caller** gains `Resource::NamespaceFiles` (item 1). This is a mechanical change; `git diff --stat` and the 66-line count are in the report, and `nsfiles_*` pass unchanged.
  - **`_kv`:** `check_kv_segment` mirrors `check_files_segment` (`:114-122`) with `KV_SEGMENT`. `check_namespace` calls it after `check_files_segment`. `validate_namespace` (`validate.rs:627`) gains a third clause: when the pattern passes and `_files` passes, `check_kv_segment` → finding code `namespace_kv_segment`, path `namespace`. `validate_namespace` runs in `validate_for_run` (`:471`), so a stored flow with a `_kv` segment also stops at run, as `_files` does; Task 1's count says whom that affects.
  - **`store.rs` reads:** one `concat!` query each, with `WHERE workspace_id = $1 AND namespace = $2` in every statement. `expired` is computed in SQL as `(expires_at IS NOT NULL AND expires_at <= now())`.
    - The latest row: `ORDER BY revision DESC LIMIT 1`, then `live()` judged **after** taking it. Filtering first would resurrect an older revision.
    - `list_live`: `SELECT … FROM (SELECT DISTINCT ON (key) … ORDER BY key, revision DESC) latest WHERE NOT deleted AND NOT expired ORDER BY key COLLATE "C" LIMIT $n`, so the cap counts live keys. *(Amended 2026-10-07, Task 1: `ORDER BY latest.id`, not `key COLLATE "C"`: Kestra lists keys in the order their latest revision was written, w9c `get_keys`.)*
    - `resolve_first_live`: `namespace = ANY($2)` with `DISTINCT ON (namespace)`, then the deepest live level is picked in Rust in `grants` order. Assert that every grant has one workspace and `Resource::Kv`.
  - **`read_value`:** `e.size > cap` → `TooLarge` before any download; `get_stream(..).take(cap + 1)`; `NotFound` → `Author("revision {n} of key '{k}' in namespace '{ns}' is recorded but its value is gone from internal storage")`; other store errors are logged with the `s3://` key at `ERROR` and answered as `Store`, naming the key and namespace only. Then `value::decode(e.value_type, &bytes, cap)`, whose error is `Author` naming the key and revision.
  - **`value::encode`:** call `runners::ionfmt::write_ion_json` (make `ionfmt` and the function `pub(crate)`), and special-case only what `types` shows: a Double as Ion float text (`1.5e0`, `1e0`, `1.0e20`), the API's Ion decimals, and timestamps at millisecond precision in UTC. A DURATION needs no special case: it is an Ion string (amended 2026-10-07, Task 1).
  - **`parse_ttl`:** make `executor::retry` `pub(crate)` and call its `parse_duration`; then `Ttl::Task` refuses `<= 0` with `ttl: must be a positive duration`, and `Ttl::Header` refuses `< 0` with `ttl cannot be negative` and accepts zero (Task 1 review I3).
- [ ] **Step 4: Run the tests to see them pass**, then the per-task gate.
- [ ] **Step 5: Hold audit.** This task writes nothing. Confirm that `read_value` takes no connection and that every read function takes a `&mut PgConnection` it does not keep. State both in the report.
- [ ] **Step 6: Mutations:**
  - (a) `check_namespace` without `check_kv_segment` (the publish test and the access test both die; show both);
  - (b) `authorize` Purge with `starts_with(own)` and no `"."` (`a.bc` dies);
  - (c) `require` ignoring the resource (`require_refuses_a_grant_of_the_other_resource` dies);
  - (d) `check_key` without the length bound (351 dies), then without the leading-character class (`_a` dies);
  - (e) a read query without `workspace_id = $1` (the two-workspace test dies);
  - (f) `resolve` filtering `NOT deleted` in SQL before taking the latest (the deleted-latest test dies);
  - (g) `expired` computed in Rust from `Utc::now()` instead of SQL's `now()`: the test sets the row's `expires_at` from SQL `now() + 200ms` and sleeps 0 ms. If it does not die, say so; the claim is "the database's clock", and the report names the instrument that measures it;
  - (h) `list_live` filtering after `LIMIT` (cap test dies);
  - (i) `list_live`'s prefix with `LIKE $3 || '%'` (the `_` test dies);
  - (j) SQL mutant: drop `COLLATE "C"` from the CHECK, on a scratch database (`dropdb`/`createdb` on `:5440` only), proven by the CHECK test with `é` (if it survives, record that this server's collation makes the range ASCII anyway, and keep `COLLATE "C"` for other collations).
- [ ] **Step 7: Commit** `feat(workflow): W9c Task 2 — the KV table, the key rule, the _kv segment and reads under one access check`.

---

## Task 3: Writes, staging, `Put`'s merge, purge, and the expiry sweep

**Files:**
- Create `src/storage/kv/{merge.rs,sweep.rs}` and `tests/kv_s3_live.rs`.
- Modify `src/storage/kv/store.rs`, `src/storage/lock.rs` (`LockedSession::try_acquire` beside `acquire` `:51`), `src/storage/nsfiles/store.rs` (`detached` `:677` becomes `pub(crate)`), `crates/egret-bin/src/main.rs` (beside `:1871`) and `tests/kv_store.rs`.

**Interfaces:**
- Consumes: Task 2's `KvKey`, `KvValue`, `KvEntry`, `encode`, `decode`, `resolve`, `rows_page`, `read_value`, `kv_lock_text` and `namespace_kv_uri`; W9b's `staging::{record, forget}` (`staging.rs:63`, `:81`), `nsfiles::store::detached`, `storage::lock::{LockedSession, lock_timeout_value, is_lock_timeout, is_lock_timeout_error}` (`lock.rs:51`, `:129-148`), and `ObjectStore::{put, copy, head, get_stream, delete}`.
- Produces:

```rust
// src/storage/kv/store.rs — the write side (§4.3, §4.4, §8.5, §8.6)
#[derive(Clone, Debug, PartialEq)]
pub struct NewValue { pub value: KvValue, pub description: Option<String>,
                      pub ttl: Option<chrono::TimeDelta>, pub overwrite: bool }
#[derive(Clone, Debug, PartialEq)]
pub enum KvWritten {
    Written(KvEntry),
    /// A CreatedBy::TaskRun write whose task run already wrote the latest revision (a re-claim,
    /// judged under the key's lock): nothing is written (§11).
    Already(KvEntry),
}
/// §4.3. Step 1: require(Kv, Write); encode; encoded.len() > max_read_bytes -> TooLarge naming
/// workflow.storage.max_read_bytes, before anything is staged. Step 2: staging::record, then
/// fs.store.put(staged.uri, bytes) holding NO connection and NO lock. Step 3, on `detached`:
/// begin -> (lock_wait) SET LOCAL lock_timeout -> pg_advisory_xact_lock(hashtextextended($1, 0))
/// over kv_lock_text -> latest row -> re-claim skip -> overwrite rule (a LIVE latest with
/// overwrite false: Conflict with w9c `overwrite`'s message) -> revision = max + 1 -> copy staging
/// -> revision key -> head == size -> INSERT (expires_at = now() + make_interval(secs => $ttl))
/// -> commit. Step 4: staging::forget on every way out. A lock wait past the grant's lock_wait is
/// Conflict "the key is busy (another write holds it); try again", naming the key.
pub async fn set(pool: &PgPool, fs: &FileStore, grant: &Grant, key: &KvKey, new: NewValue, by: CreatedBy)
    -> Result<KvWritten, NsError>;
/// §8.5. Step 1 for the patch (require, the patch's encoded size); nothing is staged. On
/// `detached`, in ONE transaction holding the key's lock: latest row -> re-claim skip -> a live
/// latest: read_value (bounded) -> merge::merge -> KvType::of(merged) -> encode -> size check ->
/// fs.store.put(revision key) -> head -> INSERT keeping the latest's description and expires_at.
/// Missing (never written, deleted, expired, item 8): error_on_missing -> NotFound with w9c's
/// message; else merge::merge(None, patch).
pub async fn put_merge(pool: &PgPool, fs: &FileStore, grant: &Grant, key: &KvKey,
                       patch: serde_json::Value, error_on_missing: bool, by: CreatedBy)
    -> Result<KvWritten, NsError>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deleted { Deleted, Missing, Already }
/// §4.4/§8.4: under the key's lock, a LIVE latest row is flagged deleted (deleted_at = now(),
/// deleted_by_* from `by`); Missing for none/deleted/expired; Already when the latest row is
/// deleted by this task run (CreatedBy::TaskRun). Adds no revision.
pub async fn delete(pool: &PgPool, grant: &Grant, key: &KvKey, by: CreatedBy) -> Result<Deleted, NsError>;
/// §9's bulk delete: keys sorted and de-duplicated, ONE transaction taking each key's lock in
/// ascending order (each wait bounded by the grant's lock_wait), all or nothing. Answers the keys
/// that were live and are now deleted. More than MAX_BULK_DELETE_KEYS keys: TooLarge naming the cap,
/// nothing taken (the user's ruling, 2026-10-07).
pub const MAX_BULK_DELETE_KEYS: usize = 1_000;
pub async fn delete_many(pool: &PgPool, fs: &FileStore, grant: &Grant, keys: &[KvKey], by: CreatedBy)
    -> Result<Vec<KvKey>, NsError>;
/// §8.6, per key: require(Kv, Purge); on `detached`, take the key's SESSION lock (LockedSession,
/// the same hash as the transaction lock), re-read every row of the key (more than `cap`:
/// TooLarge naming the key and the cap), let `judge` pick the row ids to purge FROM THOSE FRESH
/// ROWS (item 13), DELETE … WHERE id = ANY($ids) RETURNING revision, commit, delete each removed
/// revision's object (a failure is a WARN orphan, not an error), unlock. Answers the rows removed.
/// (Amended 2026-10-07, the user's ruling: the re-read also computes each row's `superseded_at`,
/// the created_at of the key's NEXT revision, which `before` judges; see Step 3.)
pub async fn purge_key<F>(pool: &PgPool, fs: &FileStore, grant: &Grant, key: &KvKey, cap: u64, judge: F)
    -> Result<u64, NsError>
where F: FnOnce(&[PurgeRow]) -> Vec<i64> + Send + 'static;
/// One row of purge_key's re-read, in revision order. `superseded_at`: when this revision stopped
/// being the latest, i.e. the created_at of the key's next revision; None for the latest row.
#[derive(Clone, Debug, PartialEq)]
pub struct PurgeRow { pub entry: KvEntry, pub superseded_at: Option<chrono::DateTime<chrono::Utc>> }

// src/storage/kv/merge.rs
/// Kestra's Put (Put.java:72-121), against w9c `put_merge`: a map deep-merges into a map; a list of
/// {key, value} entries patches dotted paths; a single [{value}] replaces. A shape the fixture
/// refuses is Err with its message. `current` None: the key is missing.
pub fn merge(current: Option<&serde_json::Value>, patch: &serde_json::Value) -> Result<serde_json::Value, String>;

// src/storage/lock.rs
impl LockedSession {
    /// pg_try_advisory_lock on `key` on a pooled connection: Some(session) holding it, or None
    /// (the connection goes back to the pool, holding nothing).
    pub(crate) async fn try_acquire(pool: &PgPool, key: i64) -> egret_core::Result<Option<Self>>;
}

// src/storage/kv/sweep.rs (§11)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvSweepConfig { pub every: std::time::Duration /* 1 h */, pub batch: i64 /* 1000 */ }
impl Default for KvSweepConfig { /* hourly, 1000 */ }
/// One pass over EVERY workspace: keyset pages of `batch` by row id of the latest rows with
/// expires_at <= now(); per key: try-lock (busy: skipped, the next pass retries), re-read the latest
/// row under the lock, still expired -> DELETE every row of the key by the ids read, commit,
/// delete the objects, unlock. Grants come from authorize(ws, Actor::System, ns, Resource::Kv,
/// Access::Purge). Answers keys removed; Err is the database's.
pub async fn sweep_expired(pool: &PgPool, fs: &FileStore, cfg: &KvSweepConfig) -> Result<u64, NsError>;
/// Hourly, the FIRST pass after `every` (interval_at, item 4); stops on `cancel` between passes.
pub fn spawn_kv_expiry_sweeper(pool: PgPool, fs: Arc<FileStore>, cfg: KvSweepConfig, cancel: CancellationToken)
    -> tokio::task::JoinHandle<()>;
```

- [ ] **Step 1: Write the failing tests.**

```rust
// src/storage/kv/merge.rs (unit)
#[test] fn every_put_merge_cell_of_the_artifact() { /* every w9c put_merge cell, driven from its top level only: merge(existing, patch) == result, or Err(message); existing_state != live -> None */ }

// tests/kv_store.rs (Postgres :5440 + MemStore; flavor = "multi_thread")
#[tokio::test(flavor = "multi_thread")]
async fn two_concurrent_sets_with_overwrite_false_write_exactly_once() {
    // A store whose `copy` blocks on a Barrier for the first caller, so A holds the lock mid-copy
    // while B starts; release. Exactly one Written, one Conflict naming the key; one row. 20 rounds.
}
#[tokio::test(flavor = "multi_thread")]
async fn two_concurrent_puts_merging_different_fields_both_survive_in_two_revisions() {}
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_writers_get_distinct_revisions_and_keys() { /* 8 writers x 5 rounds: revisions 1..=40, 40 objects */ }
#[tokio::test] async fn revisions_are_laid_out_as_the_artifact_lays_them_out() { /* w9c layout: .ion, .ion.v2, .ion.v3 */ }
#[tokio::test] async fn overwrite_false_over_a_deleted_or_expired_key_writes() {}
#[tokio::test] async fn a_value_over_max_read_bytes_is_refused_naming_the_cap_and_nothing_is_staged() {
    // Caps { max_read_bytes: 64, .. }: TooLarge names workflow.storage.max_read_bytes; the staging
    // table and the store's staging prefix are empty; no row. And at exactly 64: written.
}
#[tokio::test] async fn a_merged_result_over_max_read_bytes_is_refused_and_nothing_is_written() {}
#[tokio::test] async fn expires_at_is_now_plus_ttl_by_the_databases_clock() { /* |expires_at - (created_at + ttl)| < 1 ms */ }
#[tokio::test] async fn put_keeps_the_latest_description_and_expiry() {}
#[tokio::test] async fn put_over_an_expired_key_is_a_missing_key() { /* item 8; w9c put_merge.over_expired */ }
#[tokio::test] async fn delete_flags_the_latest_and_a_rewrite_makes_the_next_revision_live() { /* R1 */ }
#[tokio::test] async fn a_reclaimed_set_put_or_delete_writes_nothing_more() {
    // CreatedBy::TaskRun { run }: Set twice -> Written then Already (one row); overwrite false twice
    // -> no Conflict against itself; Put twice -> one merge; Delete twice -> Deleted then Already
}
#[tokio::test] async fn delete_many_takes_1000_keys_and_refuses_1001_naming_the_cap() { /* 1,001: TooLarge, no lock taken, no row changed */ }
#[tokio::test] async fn delete_many_is_all_or_nothing_and_answers_the_live_keys() {
    // one key's lock held by another session past lock_wait: Conflict naming that key, NO key deleted
}
#[tokio::test] async fn an_api_grant_waits_at_most_its_lock_wait() { /* with_lock_wait(100ms): Conflict "busy", the connection released */ }
#[tokio::test] async fn a_failed_copy_rolls_back_and_forgets_the_staging_object() {}
#[tokio::test] async fn purge_key_hands_the_judge_each_rows_successors_created_at() {
    // three revisions: superseded_at = [Some(r2.created_at), Some(r3.created_at), None]; a soft-deleted latest: None
}
#[tokio::test] async fn purge_key_judges_under_the_lock() {
    // judge sees the rows read AFTER the lock: a rewrite between listing and purge keeps its new revision
}
#[tokio::test(flavor = "multi_thread")]
async fn a_purge_and_a_writer_never_leave_a_row_without_an_object() { /* every surviving row: head is Some */ }
#[tokio::test] async fn the_sweep_removes_an_expired_key_its_rows_and_objects() {}
#[tokio::test] async fn the_sweep_keeps_a_key_rewritten_between_listing_and_lock() { /* a hook between page and lock */ }
#[tokio::test] async fn the_sweep_skips_a_busy_key_and_the_next_pass_takes_it() {}
#[tokio::test(flavor = "multi_thread")] async fn two_sweeps_in_parallel_remove_each_key_once() {}
#[tokio::test] async fn the_sweep_covers_every_workspace_and_flowless_namespaces() {}
#[tokio::test] async fn the_sweep_pages_past_one_batch() { /* batch 3, 7 expired keys -> 7 */ }
#[tokio::test(start_paused = true)] async fn the_first_sweep_pass_comes_after_every() { /* advance 59 min: none; 61: one */ }
#[tokio::test] async fn a_kv_write_never_touches_namespace_file_rows_or_their_lock_keys() { /* "w9c:" vs "w9b:" texts */ }
// tests/storage_prune.rs
#[tokio::test] async fn a_prune_pass_leaves_kv_untouched() {}
// tests/kv_s3_live.rs (MinIO-gated; FileStore prefix "w9c-t3-test/<uuid>/"; delete_prefix at the end)
#[tokio::test(flavor = "multi_thread")] async fn set_get_put_delete_purge_and_sweep_on_minio() { /* a 10 MiB value at the cap */ }
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **Lock:** the text `kv_lock_text(grant, key)` is built in Rust and bound as `$1` (a value, never SQL). The **hash** is computed in SQL: `SELECT pg_advisory_xact_lock(hashtextextended($1, 0))`. The session form computes the key with `SELECT hashtextextended($1, 0)`, as `nsfiles::store::session_keys` (`store.rs:650`) does. A `lock_wait` sets `SELECT set_config('lock_timeout', $1, true)` first, as `xact_lock` (`store.rs:588-616`) does. A `55P03` (`is_lock_timeout`) becomes `Conflict` naming the key.
  - **Staging:** `set` uses `staging::record(pool, fs, ws)` and `fs.store.put(&staged.uri, bytes)`: a value is in memory and at most `max_read_bytes`, so it is one PUT. Then `detached` runs commit + `forget`, as `nsfiles::store::put` (`store.rs:1065-1106`) does.
  - **The `head` check:** `Ok(Some(n)) if n == size` or a `Store` error logged with the key (`store.rs:1192-1210`'s shape).
  - **`expires_at`:** `now() + make_interval(secs => $n)`, with `$n = ttl.num_milliseconds() as f64 / 1000.0`; `NULL` without a ttl.
  - **`PurgeKV`'s `before` (the user's ruling, 2026-10-07: it follows Kestra).** Kestra compares its `updated` column, the time a revision stopped being the latest. egret keeps no such column and derives it: a revision stopped being the latest when the key's **next revision** was created, so its instant is that next row's `created_at`. `purge_key`'s re-read under the lock computes it with a window function, in the same statement: `SELECT …, lead(created_at) OVER (ORDER BY revision) AS superseded_at FROM workflow.kv_entries WHERE workspace_id = $1 AND namespace = $2 AND key = $3 ORDER BY revision` (one `concat!`; the window runs over the one key's rows only). The `before` judge then purges each row with `superseded_at IS NOT NULL AND superseded_at <= before`, never by the row's own `created_at`. The latest row has no next row, so its `superseded_at` is None and `before` never purges it, a soft-deleted latest included (Kestra kept `kd`'s deleted revision 3). The rows a `before` or `keepAmount` purge removes are always the lowest revisions, so a gap left by an earlier purge never sits between a surviving row and its true successor. Fixture: `purge.version_before` (measured: `size` 3; `k3`, `kd`, `kx3` keep r2 and r3; `ke` keeps r1 because its r2 was written after `t_mid`).
  - **The sweep's page query** (one `concat!`): `SELECT e.id, e.workspace_id, e.namespace, e.key FROM workflow.kv_entries e WHERE e.expires_at <= now() AND e.id > $1 AND e.revision = (SELECT max(l.revision) FROM workflow.kv_entries l WHERE l.workspace_id = e.workspace_id AND l.namespace = e.namespace AND l.key = e.key) ORDER BY e.id LIMIT $2`. Under the lock: re-read the latest row; if it is still `expires_at <= now()`, `DELETE … WHERE id = ANY($ids_of_every_row_read) RETURNING revision`.
  - **`egret-bin`:** beside the staging sweeper (`main.rs:1871`), inside the same `if let Some(fs) = egret_workflow::storage::current()`, call `egret_workflow::storage::kv::sweep::spawn_kv_expiry_sweeper(db.pool.clone(), fs.clone(), Default::default(), reaper_cancel.clone())`, with a comment naming §11.
- [ ] **Step 4: Run the tests to see them pass**, the MinIO suite with the four `EGRET_TEST_S3_*` vars, then the per-task gate.
- [ ] **Step 5: Hold audit**, every write path, in the report. For each: what is held, from where to where, and the I/O inside.
  - `set`: `record` (one statement), then the staging PUT with nothing held, then the transaction holding the lock across one server-side `copy` + `head` (server-paced, ≤ `max_read_bytes`).
  - `put_merge`: the transaction holding the lock across one bounded GET and one PUT (each ≤ `max_read_bytes`; item 7).
  - `delete` / `delete_many`: no store I/O under the lock.
  - `purge_key` / the sweep: the session lock across the object deletes.

  A test pins each "nothing held" claim: during the staging PUT, a second session's `pg_try_advisory_xact_lock` on the key succeeds, and `pg_stat_activity` shows no connection of the writer.
- [ ] **Step 6: Mutations:**
  - (a) `set` without the advisory lock (the overwrite-false test dies; show the failing assertion; if 20 rounds do not kill it, widen the barrier window and say so);
  - (b) `put_merge` reading the latest **before** taking the lock (the two-`Put`s test dies);
  - (c) the overwrite rule treating an expired latest as live (the deleted/expired test dies);
  - (d) the size check after staging (the nothing-staged test dies);
  - (e) `purge_key` judging the listing's rows instead of the fresh ones (the under-the-lock test dies);
  - (f) `purge_key` with a transaction lock (the purge-vs-writer test dies);
  - (g) the sweep without its re-read under the lock (the rewritten-key test dies);
  - (h) the sweep with `interval` instead of `interval_at` (the first-pass test dies);
  - (i) `delete_many` committing per key (all-or-nothing dies);
  - (j) the re-claim skip removed from `set` (the re-claim test dies);
  - (k) `superseded_at` computed as the row's own `created_at` instead of `lead(created_at)` (the successor test dies, and Task 5's `purge.version_before` cell).
- [ ] **Step 7: Commit** `feat(workflow): W9c Task 3 — KV writes under a per-key lock, Put's merge, purge and the expiry sweep`.

---

## Task 4: `kv()` and the secret refusal

**Files:**
- Create `src/worker/handlers/kv.rs`, holding **only** the module doc and the six FQCN constants (Task 5 adds the rest), and `tests/kv_render.rs`.
- Modify `src/worker/handlers/mod.rs` (`pub mod kv;` beside `:46`) and `src/runners/files.rs` (beside `NsFileArgs` `:152-183` and the trait's defaults `:81-99`).
- Modify `src/runners/functions.rs`: register `kv` beside `fileURI` (`:184-187`); `REFUSED` (`:273-290`) loses `kv`; its doc (`:234-272`) gains a dated W9c line; the comment *"seven since W9b Task 4"* (`:192`) becomes six.
- Modify the test `kv_still_names_w9c_and_the_refused_list_shrinks_to_seven` (`:3358`, doc from `:3325`), `src/storage/scope.rs` (`impl RenderFiles for Snapshot` `:586-733`, `Unconfigured` `:746-771`), `src/plugins/mod.rs` (beside `:173`) and `src/spec/validate.rs` (`secret_findings` `:336-457`).

**Interfaces:**
- Consumes: Task 2's `KvKey::parse`, `ancestors`, `resolve_first_live`, `read_value`, `authorize(…, Resource::Kv, Access::Read)`; W9b's `Snapshot::{may_block, outage, ns_db, ns_conn}` (`scope.rs:71-86`, `:182-193`, `:214-236`) and the reconnect-once logic of `ns_lookup` (`:343-396`); `filters::{Args, is_java_null}` and `functions::{pebble_error, FILE_SCOPE}` as `file_function` uses them (`:360-390`).
- Produces:

```rust
// src/worker/handlers/kv.rs (this task: constants only)
pub const SET: &str = "io.kestra.plugin.core.kv.Set";
pub const GET: &str = "io.kestra.plugin.core.kv.Get";
pub const GET_KEYS: &str = "io.kestra.plugin.core.kv.GetKeys";
pub const DELETE: &str = "io.kestra.plugin.core.kv.Delete";
pub const PUT: &str = "io.kestra.plugin.core.kv.Put";
pub const PURGE_KV: &str = "io.kestra.plugin.core.kv.PurgeKV";

// src/runners/files.rs — no sqlx, no storage types
pub struct KvArgs<'a> {
    pub key: &'a str,
    /// None: the running flow's namespace WITH inheritance; Some: that namespace only, no walk,
    /// even when it is the flow's own (KvFunction.java:64-66).
    pub namespace: Option<&'a str>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum KvAnswer {
    Value(serde_json::Value),
    /// Never written, soft-deleted or expired at every level looked at: Kestra's sentence
    /// (w9c kv_function / inheritance), naming the namespace Kestra's names.
    Missing(String),
}
pub trait RenderFiles {
    /// W9c. Err: refused (the key rule, a refused namespace, no execution, an outage -- which
    /// ALSO sets infrastructure_failure). Default: refused, naming that kv() needs a running
    /// execution of a flow.
    fn kv(&self, args: &KvArgs<'_>) -> Result<KvAnswer, String> { /* refusal */ }
}

// src/plugins/mod.rs
pub const CODE_SECRET_IN_KV_VALUE: &str = "secret_in_kv_value";
/// §7's message, naming the task id and the property path.
pub fn secret_in_kv_value_message(task_id: &str, path: &str) -> String;
```

**The rule this task implements (§6), and the connection.**
- `Snapshot::kv`:
  1. `may_block`;
  2. `KvKey::parse(args.key)`, an `Err` naming the rule;
  3. the levels: `args.namespace` → `[ns]`; `None` → `ancestors(flow_namespace)` (no flow namespace → refused, naming that `kv()` needs a flow's execution);
  4. `authorize(self.workspace, Actor::Flow { namespace: own }, level, Resource::Kv, Access::Read)` per level, the first refusal returned;
  5. on the scope's one connection, opened lazily from `ns_db` and re-opened once on a reused broken connection as `ns_lookup` does, `resolve_first_live(conn, &grants, &key)`;
  6. **release the connection mutex**;
  7. then `read_value(&self.store, grant_of(entry), &entry)` on `handle.block_on`.
- `NotFound`/`None` → `KvAnswer::Missing(message)`. A `Db` or `Store` error → `self.outage(..)` and `Err`.
- **Factor** the connect-and-retry loop out of `ns_lookup` into one private helper that both call, rather than copying it. The block below is a **sketch**: adjust its signature to the lifetimes `ns_lookup`'s loop really needs.

```rust
// sketch — src/storage/scope.rs
fn on_ns_conn<T>(&self, what: &str,
                 mut work: impl FnMut(&mut PgConnection) -> futures_util::future::BoxFuture<'_, Result<T, NsError>>)
    -> Result<T, NsError>;
```

- [ ] **Step 1: Write the failing tests.**

```rust
// runners/functions.rs (unit, a fake RenderFiles answering from a map)
#[test] fn the_refused_list_is_six_and_kv_is_not_in_it() {
    // REPLACES kv_still_names_w9c_and_the_refused_list_shrinks_to_seven: six names, the same three
    // argument shapes each, `secret` still Confined(SECRET_REFUSAL); and {{ kv('k') }} on the fake
    // answers its value
}
#[test] fn kv_takes_key_namespace_and_error_on_missing_positionally_or_by_name() {}
#[test] fn kv_without_a_key_is_kestras_sentence() { /* "The 'kv' function expects an argument 'key'." */ }
#[test] fn kv_error_on_missing_defaults_to_true() { /* Missing -> error with the message; errorOnMissing=false -> null */ }
#[test] fn kv_returns_each_type_as_kestras_kv_printed_it() { /* w9c types.*.kv_prints through the fake */ }
#[test] fn a_non_boolean_error_on_missing_is_refused_as_measured() { /* w9c kv_function */ }
#[test] fn kv_without_a_scope_refuses_by_name() { /* "The 'kv' function needs a running execution …" */ }

// tests/kv_render.rs (Postgres :5440 + MemStore; flavor = "multi_thread"; install_db_for_tests)
#[tokio::test(flavor = "multi_thread")] async fn kv_matches_the_artifact() { /* every w9c kv_function, inheritance and types cell, through a real task run */ }
#[tokio::test(flavor = "multi_thread")] async fn kv_in_run_if_if_switch_and_a_task_property() {}
#[tokio::test(flavor = "multi_thread")] async fn kv_in_settle_reads_a_key_committed_after_the_execution_started() {}
#[tokio::test(flavor = "multi_thread")] async fn kv_with_no_namespace_walks_and_with_the_flows_own_does_not() { /* KvFunction.java:61-66 */ }
#[tokio::test(flavor = "multi_thread")] async fn an_expired_key_is_missing_and_honours_error_on_missing() { /* both values; rows and objects unchanged */ }
#[tokio::test(flavor = "multi_thread")] async fn another_workspaces_key_is_missing_and_never_its_value() {}
#[tokio::test(flavor = "multi_thread")] async fn a_kv_segment_namespace_argument_is_refused_by_name() {}
#[tokio::test(flavor = "multi_thread")] async fn a_database_outage_during_kv_re_claims_with_no_attempt_spent() {}
#[tokio::test(flavor = "multi_thread")] async fn a_store_outage_reading_the_value_re_claims_with_no_attempt_spent() {}
#[tokio::test(flavor = "multi_thread")] async fn the_scope_holds_no_lock_and_releases_its_connection_at_end_render() {}
#[tokio::test(flavor = "multi_thread")] async fn a_flow_trigger_condition_calling_kv_is_refused_by_name() {}
#[tokio::test(flavor = "multi_thread")] async fn retry_with_flowable_errors_reads_its_counter() {
    // the corpus flow's set_kv then {{ kv(namespace=flow.namespace, key='retry_counter_1') }} renders 1
    // (the AllowFailure part stays refused; the entry stays baselined)
}
// spec/validate.rs (unit) + tests/publish.rs
#[test] fn a_secret_in_a_set_or_put_value_is_refused_with_its_own_code_and_message() {
    // `value: "{{ secret('X') }}"`, a nested map leaf, a key -> secret_in_kv_value naming the task and
    // `tasks.<id>.value…`; NOT also secret_outside_plugin_task for that path
}
#[test] fn a_secret_elsewhere_on_a_set_task_keeps_the_generic_refusal() { /* `key: "{{ secret('X') }}"` */ }
#[test] fn valids_kv_and_retry_with_flowable_errors_still_publish_and_plan() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **`kv_function`** (`functions.rs`, beside `file_function`):
    - `Args::new(rest)`; `key` at `("key", 0)`, missing → Kestra's sentence; `namespace` at `("namespace", 1)` with `is_java_null` as absent; `errorOnMissing` at `("errorOnMissing", 2)`, default `true`, non-boolean as `kv_function` measured.
    - Look up the scope as `file_function` does (`:379-390`); no scope → refused by name.
    - Then `scope.0.kv(&KvArgs { … })`: `Value(v)` → `MjValue::from_serialize(&v)`; `Missing(m)` → `pebble_error(m)` when `errorOnMissing`, else `MjValue::from(())`; `Err(m)` → `pebble_error(m)`.
  - **`REFUSED`:** drop the `kv` row. The doc paragraph gains *"Updated 2026-10-07 (W9c Task 4): six names now. `kv` is implemented ([`kv_function`]) over the KV store."*
  - **`secret_findings`:** in `Walk::value`'s object arm (`:429-447`), when `ty` is `kv::SET` or `kv::PUT` and `k == "value"`, collect every string under `v` (keys too, with `strings`) and push `Finding { code: CODE_SECRET_IN_KV_VALUE, message: secret_in_kv_value_message(id, &p), path: p }` for each that `text_uses_secret`, **instead of** recursing into `self.value(v, here)`. Every other property of the task walks as today. The message is §7's sentence, verbatim.
  - **`Unconfigured::kv`** answers its `[storage]` message, naming `kv`.
- [ ] **Step 4: Run the tests to see them pass**, `cargo test -p egret-workflow --test oracle` (`function-kv` must still match: `compare: error-class`), the ratchet line (unchanged), then the per-task gate.
- [ ] **Step 5: Hold audit.** `kv()` writes nothing. Report the render scope's connection, opened once per render that calls `kv()` and closed at `end_render`, as W9b's; the mutex released before the object read; no advisory lock. Measure and state the cost: the median of 50 `kv()` renders on `:5440`.
- [ ] **Step 6: Mutations:**
  - (a) `authorize` skipped in `Snapshot::kv` (the `_kv`-namespace test dies);
  - (b) the walk taken for an explicit namespace equal to the flow's (the no-walk test dies);
  - (c) an expired level treated as found (the expired test dies);
  - (d) a DB error mapped to `Missing` (the outage test dies: `errorOnMissing: false` would answer `null`);
  - (e) `errorOnMissing` defaulting to false (the default test dies);
  - (f) the secret scan reporting `secret_outside_plugin_task` for `value` (the code test dies);
  - (g) the secret scan only on `Set` (the `Put` row dies).
- [ ] **Step 7: Commit** `feat(workflow): W9c Task 4 — kv() over the KV store, and secret() refused in a KV value`.

---

## Task 5: The six tasks

**Files:**
- Modify `src/worker/handlers/kv.rs` (the six handlers, `DECLARED`, `register`), `src/storage/kv/value.rs` (`coerce`), `src/runners/env.rs` (`render_typed`, beside `render_str` `:1915`), `src/spec/registry.rs` (six rows after `:271`), `src/runtime.rs` (after `:731-735`), `oracle-exec/driver.rs` (`registry()` `:139-170`) and `oracle-exec/handler-surface.txt` (run `python3 handler_surface.py`; commit its diff if it changes).
- Create `tests/kv_tasks.rs`.

**Interfaces:**
- Consumes: Tasks 2-3's store, `KvType::parse`, `parse_ttl`, `ancestors`, `namespaces_with_kv`, `rows_page`, `purge_key`, `put_merge`, `delete`, `set`; W9b's `handlers::storage::{spec, no_storage, outage}` (`storage.rs:345-370`), `nsfiles::stage::{ListProp, MAX_NAMESPACES}` (`stage.rs:91`, `:553`), `nsfiles::glob::wildcard_match` (`glob.rs:381`), `models::Property`, and namespace.rs's patterns (`flow_namespace` `:231`, `split` `:245`, `CreatedBy::TaskRun { execution: task.execution_id, task_run: task.run_id }` `:789`).
- Produces:

```rust
// src/runners/env.rs
/// Kestra's renderTyped (Set.java:111): a `template` that is exactly ONE `{{ … }}` with no other
/// text, not even surrounding spaces (w9c typed_render.tr_ws: ` {{ 1 }} ` is the string " 1 "), is
/// rendered as `{{ (…) | toJson }}` and parsed back with serde_json (arbitrary precision kept);
/// anything else is render_str's String. Against w9c typed_render.
pub fn render_typed(template: &str, cx: &RenderContext) -> Result<serde_json::Value, RenderError>;

// src/storage/kv/value.rs
/// §8.1: a typed value and an optional kvType -> KvValue, as the artifact coerces (w9c set_coercion):
/// NUMBER from a string by Jackson; BOOLEAN strict ("true"/"false", any case, else "Cannot parse '%s'
/// as a BOOLEAN value: expected 'true' or 'false'."); DATETIME an instant; DATE a local date falling
/// back to an instant's UTC date; DURATION validated, kept as written; JSON parsed; a Number with
/// STRING its toString(); the other mismatches as measured. No kvType: KvType::of. kvType applies to
/// a String only: any other value is returned as itself, typed KvType::of, whatever kvType says, so
/// DURATION is the stored type only for a String (w9c set_coercion.t_*_duration; review C1).
pub fn coerce(value: serde_json::Value, kv_type: Option<KvType>) -> Result<KvValue, String>;

// src/worker/handlers/kv.rs
pub struct SetKv { /* pool: PgPool, files: Option<Arc<FileStore>> */ }
pub struct GetKv; pub struct GetKeys; pub struct DeleteKv; pub struct PutKv; pub struct PurgeKv;   // same fields
impl SetKv { pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self; }  // and each of the others
/// add_publishable for all six; runtime::handlers_for calls it beside namespace::register.
pub fn register(handlers: &mut super::super::Handlers, pool: PgPool, files: Option<Arc<FileStore>>);
/// (class, [(property, required, default)], [outputs]) held against w9c `schema` by a test.
pub const DECLARED: &[super::namespace::DeclaredClass];
```

`DECLARED`, as the spec reads the artifact. The fixture's `schema` cells are the authority: where they differ, the test fails, and the fixture wins after a dated spec note.

| class | properties `(name, required, default)` | outputs |
|---|---|---|
| `Set` | `key` (true), `value` (true), `namespace` (false, "the flow's namespace"), `overwrite` (false, "true"), `ttl`, `kvType`, `kvDescription` | — |
| `Get` | `key` (true), `namespace` (false, "the flow's namespace"), `errorOnMissing` (false, "false") | `value` |
| `GetKeys` | `prefix`, `namespace` (false, "the flow's namespace") | `keys` |
| `Delete` | `key` (true), `namespace` (false, "the flow's namespace"), `errorOnMissing` (false, "false") | `deleted` |
| `Put` | `key` (true), `value` (true), `namespace` (false, "the flow's namespace"), `errorOnMissing` (false, "false") | — |
| `PurgeKV` | `behavior` (false, `{"type":"key","expiredOnly":true}`), `includeChildNamespaces` (false, "true"), `keyPattern`, `namespacePattern`, `namespaces` | `size` |

- [ ] **Step 1: Write the failing tests** (`tests/kv_tasks.rs`, Postgres `:5440` + MemStore, `flavor = "multi_thread"`, one `SERIAL` mutex as `nsfiles_tasks.rs:34`):

```rust
#[tokio::test] async fn declared_properties_and_outputs_match_the_artifact() { /* DECLARED vs w9c schema, top level */ }
#[test] fn render_typed_matches_the_artifact() { /* w9c typed_render, unit, no DB */ }
#[test] fn set_and_put_render_a_yaml_map_by_one_rule() { /* w9c typed_render.tr_yaml_* (chosen) and put_merge.templated_leaf: the same value */ }
#[test] fn every_set_coercion_matches_the_artifact() { /* w9c set_coercion, unit */ }
#[tokio::test(flavor = "multi_thread")] async fn set_then_get_round_trips_every_type() { /* w9c types: Get's outputs.value */ }
#[tokio::test(flavor = "multi_thread")] async fn set_with_overwrite_false_over_a_live_key_fails_naming_it() { /* w9c overwrite */ }
#[tokio::test(flavor = "multi_thread")] async fn set_ttl_zero_negative_and_malformed_as_measured() { /* w9c ttl */ }
#[tokio::test(flavor = "multi_thread")] async fn set_in_another_namespace_is_refused_naming_it_and_writes_nothing() {}
#[tokio::test(flavor = "multi_thread")] async fn set_of_a_value_over_max_read_bytes_fails_naming_the_cap() {}
#[tokio::test(flavor = "multi_thread")] async fn get_walks_only_when_its_namespace_is_the_flows() { /* w9c inheritance */ }
#[tokio::test(flavor = "multi_thread")] async fn get_missing_under_each_error_on_missing() { /* Get.java:91's message */ }
#[tokio::test(flavor = "multi_thread")] async fn get_keys_lists_live_keys_in_the_artifacts_order_with_prefix() {}
#[tokio::test(flavor = "multi_thread")] async fn get_keys_past_max_namespace_files_fails_naming_the_cap_and_count() {}
#[tokio::test(flavor = "multi_thread")] async fn delete_outputs_deleted_and_honours_error_on_missing() { /* w9c delete */ }
#[tokio::test(flavor = "multi_thread")] async fn delete_in_another_namespace_is_refused() {}
#[tokio::test(flavor = "multi_thread")] async fn put_matches_every_merge_cell() { /* w9c put_merge through the task */ }
#[tokio::test(flavor = "multi_thread")] async fn purge_kv_default_purges_expired_keys_of_own_and_children() { /* "a.b": a.b.c yes, a.bc no */ }
#[tokio::test(flavor = "multi_thread")] async fn purge_kv_expired_only_false_is_refused_naming_keep_amount_zero() {}
#[tokio::test(flavor = "multi_thread")] async fn purge_kv_version_with_neither_property_is_refused_naming_both() {}
#[tokio::test(flavor = "multi_thread")] async fn purge_kv_version_keep_amount_before_and_zero() { /* keepAmount: a deleted key entirely; before: superseded at or before it (w9c purge.version_before, measured), never the latest, a deleted latest kept */ }
#[tokio::test(flavor = "multi_thread")] async fn purge_kv_key_pattern_and_namespace_pattern_are_wildcards() { /* "w9c.pp.*"; "." literal */ }
#[tokio::test(flavor = "multi_thread")] async fn purge_kv_refuses_a_listed_namespace_outside_the_rule_before_purging_anything() {}
#[tokio::test(flavor = "multi_thread")] async fn purge_kv_pages_past_one_page() { /* max_namespace_files 3, 8 keys */ }
#[tokio::test(flavor = "multi_thread")] async fn purge_kv_size_counts_as_kestra_does() { /* w9c purge.size */ }
#[tokio::test(flavor = "multi_thread")] async fn a_reclaimed_set_put_delete_and_purge_are_idempotent() {
    // run each task, simulate the lease lapse (re-claim the same run_id), run again: one revision,
    // `deleted: true` again, PurgeKV size counts its own re-claim's purge only
}
#[tokio::test(flavor = "multi_thread")] async fn a_store_or_database_outage_in_any_task_is_a_worker_error() {}
#[tokio::test(flavor = "multi_thread")] async fn without_storage_each_task_fails_naming_storage() {}
#[tokio::test(flavor = "multi_thread")] async fn a_flow_that_sets_reads_puts_deletes_and_purges() { /* one flow, end to end in-process */ }
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.** Follow `namespace.rs`'s shape for each handler:
  - parse with `spec(T, task)`;
  - render each `Property` (`p.render(&task.render)`);
  - `outage(T, task)?`;
  - `flow_namespace(task)` (a `None` fails naming that this is not a flow's task run);
  - `authorize(task.workspace_id, Actor::Flow { namespace: &own }, &target, Resource::Kv, access)`, a refusal failing the task;
  - the store call;
  - `split(T, e)` for `NsError`;
  - `Ok(Outcome::succeeded_with(json))`.

  `cancel` holds nothing (each write commits on its own, on a detached task).
  - `Set`/`Put` share **one render rule** (the user's ruling R-a, 2026-10-07; spec §8.1), `render_value(raw, cx) -> serde_json::Value`: `value` is the raw spec JSON. A string goes through `render_typed`. An array or object stays an array or object, rendered leaf by leaf: each string leaf through `render_typed`, each key through `render_str`, every other leaf (number, boolean, null) kept as YAML gave it. A YAML number or boolean is kept as itself. So `{a: "{{ 1 + 1 }}", b: x}` is `{"a": 2, "b": "x"}` for both tasks (w9c `typed_render.tr_yaml_map`, chosen; `put_merge.templated_leaf`). This is NOT what the artifact does for `Set` (it stores a STRING of the JSON text, with string leaves; the `kestra` of each `tr_yaml_*` cell); do not reproduce it. `Set` then calls `coerce(rendered, kvType)`; `ttl` uses `parse_ttl(raw, Ttl::Task)`. `by` is `CreatedBy::TaskRun { … }`. `KvWritten::Already` succeeds with a log line saying the earlier attempt wrote it.
  - `Get`: the rendered namespace defaults to `own`. When it **equals** `own`, the levels are `ancestors(own)`; otherwise `[ns]`, with a grant per level. Read with `resolve_first_live` on a pool connection, then **let the connection go**, then `read_value`. Output `{"value": v}` or `{"value": null}`.
  - `GetKeys`: `list_live(conn, &grant, prefix, max_namespace_files)`. Output `{"keys": [...]}`.
  - `Delete`: `delete(pool, &grant, &key, by)`. `Deleted | Already` → `{deleted: true}`; `Missing` → `{deleted: false}`, or with `errorOnMissing` a failure carrying Kestra's message.
  - `PurgeKV`, on `detached`, mirroring `PurgeFiles`' `purge_namespaces`/`run_purge` (`namespace.rs:1384-1560`):
    - the namespace set: `namespaces` xor `namespacePattern` (default `[own]`). Children are added when `includeChildNamespaces` and only for `namespaces` (or the default): a `namespacePattern`'s matches are never expanded (Kestra's `PurgeTask.findNamespaces`; w9c `purge.namespaces.namespacePattern_exact`; Task 1 review I4). Candidates come from `namespaces_with_kv`, filtered to own and `own + "."` children; a listed one outside the rule is refused before anything is purged; at most `MAX_NAMESPACES`;
    - per namespace, `rows_page` pages of `max_namespace_files + 1`. Each **complete** key in a page goes to `purge_key` with a `judge` built from `behavior`: key/expired-only → every id when the fresh latest is expired; version → `keepAmount` as `revisions_to_purge` (`namespace.rs:1348`), a deleted latest → every id; `before` → every row whose `superseded_at` (Task 3's `PurgeRow`) is at or before `before`, never the latest, a deleted latest included (the user's ruling, 2026-10-07: as Kestra; w9c `purge.version_before`);
    - `keyPattern` with `wildcard_match` on the key;
    - `size` as `purge.size` measured.
  - **Registry:** six `TaskTypeSpec` rows, FQCN only, after W9b's block with a `// ── W9c Task 5 …` comment. Register in `runtime::handlers_for` and `oracle-exec/driver.rs::registry()` (`:170`, beside `namespace::register`).
  - Check whether `executor/mod.rs`'s `PUBLISHED_BUT_REFUSED` (`:3867`) names a KV type (it should not) and whether the plugin host's `describe` catalog could reach `core.kv` (it scans three JDBC jars only). State both in the report.
- [ ] **Step 4: Run the tests to see them pass**, the ratchet (`retry-with-flowable-errors` stays baselined: report its row before and after), `python3 oracle-exec/handler_surface.py --check`, then the per-task gate.
- [ ] **Step 5: Hold audit.** `Get` and `GetKeys` hold a pool connection only for their row reads, and `read_value` runs after it is released (a test asserts the pool's idle count during a stalled `get_stream`). `Set`, `Put`, `Delete` and `PurgeKV` inherit Task 3's audit. No render runs while a connection or lock is held: every property is rendered before the first store call.
- [ ] **Step 6: Mutations:**
  - (a) `Set` with `Access::Read` (the other-namespace test dies);
  - (b) `Get` walking for any namespace (the walk test dies);
  - (c) BOOLEAN coercion accepting `yes` (the coercion test dies);
  - (d) `render_typed` treating `x {{ 1 }}` as typed (the typed-render test dies);
  - (e) PurgeKV's child match without the dot (`a.bc` dies);
  - (f) the namespace refusal checked after purging earlier namespaces (the refusal test asserts nothing purged);
  - (g) `GetKeys` reading objects (a store that fails every `get_stream` makes it fail; the test asserts it succeeds);
  - (h) `Delete`'s re-claim skip removed (the re-claim test dies).
- [ ] **Step 7: Commit** `feat(workflow): W9c Task 5 — the io.kestra.plugin.core.kv tasks`.

---

## Task 6: The KV API

**Files:**
- Create `src/storage/kv/routes.rs` and `tests/kv_api.rs`.
- Modify `src/storage/nsfiles/routes.rs`: `ns_refusal` (`:202`), `bad_request` (`:197`), `core_refusal` (`:193`), `no_store` (`:245`), `upload_timed_out` (`:854`) and `upload_took_too_long` (`:866`) become `pub(crate)`. `grant` (`:225`) gains a `Resource` and becomes `pub(crate)`. `NsQuery` is already `pub(crate)` (`:173`).
- Modify `src/storage/kv/value.rs` (`parse_api_body`) and `src/routes.rs` (`management_router` `:2403`, after the namespace-file routes `:2457-2488`; `#[derive(utoipa::OpenApi)]` `:2499`, its `paths(...)` and `components(schemas(...))`).
- Regenerate `openapi.json` and the frontend client (`npm run gen:api`).

**Interfaces:**
- Consumes: everything in `storage::kv` (Tasks 2-3); `routes::{refuse, WorkflowState, WF_READ, WF_MANAGE}`; `storage::multipart::{IdleTimeout, TotalTimeout, idle_text}` (`multipart.rs:236-320`); `WorkflowState::{upload_idle_timeout, upload_total_timeout}` (`routes.rs:221`, `:226`); `st.audit.emit(egret_audit::mutation_for(…))` (`nsfiles/routes.rs:349-366`).
- Produces (paths relative to the router, mounted under `/api`):

```rust
// GET    /ws/{workspace}/workflows/namespaces/{namespace}/kv                  workflow:read   Vec<KvEntryDto> (live keys; another ws: [] as an empty ns, item 9)
// GET    /ws/{workspace}/workflows/namespaces/{namespace}/kv/inheritance      workflow:read   Vec<KvInheritedDto> (ancestors, deepest first, first wins)
// GET    /ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}?revision   workflow:read   KvDetailDto
// GET    /ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}/revisions  workflow:read   Vec<KvRevisionDto>
// PUT    /ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}            workflow:manage text/plain; headers description, ttl; 200, no body
// DELETE /ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}            workflow:manage bool (200 either way)
// DELETE /ws/{workspace}/workflows/namespaces/{namespace}/kv                  workflow:manage KvKeysBody -> KvKeysBody
#[derive(Serialize, utoipa::ToSchema)] #[serde(rename_all = "camelCase")]
pub struct KvEntryDto { pub key: String, pub r#type: String, pub size: u64, pub description: Option<String>,
                        pub expires_at: Option<DateTime<Utc>>, pub revision: u32, pub updated: DateTime<Utc> }
#[derive(Serialize, utoipa::ToSchema)] #[serde(rename_all = "camelCase")]
pub struct KvInheritedDto { pub namespace: String, #[serde(flatten)] pub entry: KvEntryDto }
#[derive(Serialize, utoipa::ToSchema)] #[serde(rename_all = "camelCase")]
pub struct KvDetailDto { pub r#type: String, pub value: serde_json::Value, pub revision: u32, pub updated: DateTime<Utc>,
                         pub description: Option<String>, pub expires_at: Option<DateTime<Utc>> }
#[derive(Serialize, utoipa::ToSchema)] #[serde(rename_all = "camelCase")]
pub struct KvRevisionDto { pub revision: u32, pub r#type: String, pub size: u64, pub description: Option<String>,
                           pub expires_at: Option<DateTime<Utc>>, pub created: DateTime<Utc>, pub deleted: bool, pub expired: bool }
#[derive(Deserialize, Serialize, utoipa::ToSchema)] pub struct KvKeysBody { pub keys: Vec<String> }
#[derive(Deserialize, utoipa::IntoParams)] pub struct KvRevisionQuery { pub revision: Option<u32> }

// src/storage/kv/value.rs
/// The PUT body (KVController.java:124-141): parsed as ONE Ion value (storage::ion), its type as
/// KVType.from infers it (w9c api); not Ion, or more than one value: the raw text, STRING.
/// (Amended 2026-10-07, the user's ruling, spec §9: a timestamp with a time is a DATETIME in UTC,
/// even at midnight; a day-precision one is a DATE; a blob, clob or sexp at any depth is Err naming
/// the type, answered 400; an annotated value at any depth makes the whole body the raw text.
/// Fix round 1, the user's rulings R-b and R-c: an empty or whitespace-only body is Err "empty body"
/// (400); a year- or month-precision timestamp is the raw text, STRING. The blob/clob/sexp refusal
/// comes before the annotation rule: `x::{{aGVsbG8=}}` is Err naming `blob` (re-review N3). w9c api.put_bodies.)
pub fn parse_api_body(body: &[u8]) -> Result<KvValue, String>;
```

- [ ] **Step 1: Write the failing tests** (`tests/kv_api.rs`, `tests/common` app, MemStore, `install_db_for_tests`):

```rust
#[tokio::test] async fn read_routes_need_workflow_read_and_write_routes_need_workflow_manage() {
    // workflow:read only: 4 GETs 200/404, 3 writes 403 and nothing written; with manage: 200
}
#[tokio::test] async fn every_row_of_the_status_table() { /* §9: 400 key rule, _kv/_files namespace, bad ttl, non-integer revision, bad bulk body; 404; 409; 413; 408; 503 */ }
#[tokio::test] async fn put_parses_ion_and_falls_back_to_the_raw_string() { /* w9c api PUT cells, the stored type of each; blob and sexp 400 naming the type */ }
#[tokio::test] async fn put_takes_description_and_ttl_headers() {}
#[tokio::test] async fn a_put_body_over_max_read_bytes_is_413_naming_the_cap_and_nothing_is_written() {}
#[tokio::test] async fn a_put_body_that_stalls_or_takes_too_long_is_408_and_nothing_is_written() { /* app_with_upload_idle / _total */ }
#[tokio::test] async fn a_busy_key_answers_409_after_the_lock_wait_holding_nothing() { /* another session holds the lock; pool idle count restored */ }
#[tokio::test] async fn get_answers_kestras_detail_with_the_stored_type() { /* w9c api get cells */ }
#[tokio::test] async fn a_soft_deleted_key_is_404_latest_and_200_by_revision() {}
#[tokio::test] async fn an_expired_key_or_revision_is_404_and_the_get_writes_nothing() { /* row hash and object list unchanged */ }
#[tokio::test] async fn revisions_lists_every_revision_flagged() {}
#[tokio::test] async fn inheritance_lists_ancestors_deepest_first_first_wins() {}
#[tokio::test] async fn a_key_named_inheritance_is_written_and_deleted_and_its_get_is_the_listing() { /* item 5 */ }
#[tokio::test] async fn delete_answers_true_then_false() {}
#[tokio::test] async fn bulk_delete_is_all_or_nothing_and_lists_the_live_keys() {}
#[tokio::test] async fn bulk_delete_of_1000_keys_is_accepted_and_1001_is_413_naming_the_cap() {
    // item 6, the user's ruling (2026-10-07): 1,000 live keys on :5440 with the server's default
    // max_locks_per_transaction -> 200, all 1,000 listed and deleted; 1,001 -> 413 naming 1,000,
    // and no key deleted
}
#[tokio::test] async fn another_workspaces_namespace_answers_byte_for_byte_as_an_empty_one() { /* item 9, every route */ }
#[tokio::test] async fn listings_past_max_namespace_files_are_413() {}
#[tokio::test] async fn no_get_route_writes() { /* the row hash and staging table before and after every GET */ }
#[tokio::test] async fn every_write_route_emits_one_audit_mutation() { /* tests/common audit_mutations; item 14 spelling */ }
#[tokio::test] async fn a_store_or_database_outage_is_503_and_the_database_error_is_never_sent() {}
#[tokio::test] async fn without_storage_every_route_is_503() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - Each handler runs `WsContext`, then `nsfiles::routes::grant(&st, &ctx, &ns, Resource::Kv, access)` (permission, then `authorize`; refusal → 400), then `KvKey::parse` (→ 400), then the store function. Errors go through `ns_refusal` (spec §9's table); a `Db` error is logged with a fixed message and never sent.
  - **`PUT`, holding nothing while the body arrives:**
    1. `grant` and the key;
    2. `st.storage` present (`no_store`);
    3. the body through `TotalTimeout::new(IdleTimeout::new(…))` and `http_body_util::Limited::new(body, max_read_bytes + 1)`, collected into memory (a 408 for either clock, a 413 at the limit);
    4. `parse_api_body` (Err: 400 naming the type, nothing staged);
    5. `ttl` header via `parse_ttl(raw, Ttl::Header)` (400; zero accepted);
    6. `store::set(pool, fs, &grant, &key, NewValue { overwrite: true, .. }, CreatedBy::User(ctx.principal.user_id.0))`.

    The route gets `DefaultBodyLimit::disable()`, as the namespace-file upload does (`routes.rs:2463-2464`).
  - `GET …/kv/{key}`: `resolve` on a pool connection, connection released, then `read_value`, then `KvDetailDto`.
  - `inheritance`: grants for `ancestors(ns)[1..]`, then `list_inherited`.
  - Bulk `DELETE`: `axum::Json<KvKeysBody>`, a malformed body answered through `bad_request`; each key parsed (400 naming it); more than `MAX_BULK_DELETE_KEYS` (1,000) → 413 naming the cap, before any lock; then `delete_many`.
  - Audit follows item 14.
  - utoipa documents every response, as `upload_namespace_file` does (`nsfiles/routes.rs:908-932`), including the 409 "busy" and the `inheritance` key collision (item 5).
- [ ] **Step 4: Run the tests to see them pass.** Then:
  - `cargo run -p egret-bin -- openapi > openapi.json`;
  - `cargo test -p egret-server --lib`;
  - `cd frontend && npm run gen:api && npm run lint && npx tsc --noEmit`;
  - the per-task gate.
- [ ] **Step 5: Hold audit.** The `PUT` body is read with no connection and no lock (a test stalls the body and asserts the pool's idle count and `pg_locks` show nothing of it). The `GET` value read runs after the row's connection is released. The bulk delete holds its locks for row updates only, with no store I/O. `set`'s window is Task 3's.
- [ ] **Step 6: Mutations:**
  - (a) a write route checking `WF_READ` (the permission test dies);
  - (b) `authorize` skipped (the `_kv`-namespace 400 dies);
  - (c) the body limit removed (413 dies);
  - (d) the body read after `grant` takes a pool connection it keeps (the stall test dies);
  - (e) `GET` of a deleted key served latest (404 dies);
  - (f) the bulk delete committing per key (all-or-nothing dies);
  - (f2) the bulk cap checked with `>=` (the 1,000-accepted test dies) or removed (the 1,001 test dies);
  - (g) the list route answering 404 for another workspace (byte-identity dies).
- [ ] **Step 7: Commit** `feat(workflow): W9c Task 6 — the KV API`.

---

## Task 7: The plugin host's messages

**Files:**
- Modify `plugin-host/src/main/java/io/kestra/core/runners/ShimRunContext.java:340-341`, `EgretUnsupported.java:4` and `EgretPluginHost.java:58`.
- Modify `plugin-host/src/test/java/io/kestra/core/runners/EgretPluginHostTest.java:125-126`.
- Modify `crates/egret-workflow/src/worker/handlers/kestra.rs:379` (the exit-4 message), `:79` (module doc) and `:1317-1321` (the pinning test).

**Interfaces:** none new. The two messages, exactly (§10):

```text
namespaceKv(): egret gives flows the KV store through the `io.kestra.plugin.core.kv` tasks and the `kv()` function, not through a plugin's RunContext; use a `core.kv` task before or after this one.
```

```text
{type}: the plugin needs a Kestra service egret does not provide to plugins: {the host's reason}
```

- [ ] **Step 1: Write the failing tests.**
  - Java: `EgretPluginHostTest` asserts the new `namespaceKv` message **exactly**, and that it contains neither `W9c` nor `does not provide yet`.
  - Rust (`kestra.rs` test at `:1305-1325`): exit 4 with the host's new reason produces the new handler message exactly. A second row with another `EgretUnsupported` reason (`decrypt`) shows the prefix is not KV-specific.
- [ ] **Step 2: Run them to watch them fail:** `mvn -f plugin-host/pom.xml test` with `JAVA_HOME`, and `cargo test -p egret-workflow --lib kestra`.
- [ ] **Step 3: Implement** the two messages. Rewrite the doc mentions so they are true now and dated (*"2026-10-07, W9c: the KV store is the `core.kv` tasks' and `kv()`'s; a plugin's `namespaceKv()` still throws, pointing at them"*). Nothing is staged for KV: `task.json` and `check_task_bytes` are unchanged, and a spy test asserts `task.json` has no new key.
- [ ] **Step 4: Run the tests to see them pass**; `mvn -f plugin-host/pom.xml verify`; rebuild `egret-plugin-host:dev` (`docker build -f deploy/docker/plugin-host.Dockerfile -t egret-plugin-host:dev plugin-host`) only if Task 8's end to end needs it; the per-task gate.
- [ ] **Step 5: Hold audit.** Not a write path; say so.
- [ ] **Step 6: Mutations:** (a) the old Java message restored (Java test dies, by running `mvn test`); (b) the handler's prefix still naming W9c (the Rust test dies).
- [ ] **Step 7: Commit** `feat(workflow): W9c Task 7 — the plugin host points namespaceKv() at the core.kv tasks`.

---

## Task 8: Rebase, end to end on a temporary kind cluster, docs, full CI

- [ ] **Step 0: Rebase if `origin/main` moved.**
  - `git fetch origin`; GitHub may be flaky, so retry up to three times with a pause and report each failure.
  - If `origin/main` is ahead of `11782600`, `git rebase origin/main` and resolve conflicts.
  - **Re-check that 0175 is still the next free migration number** (`ls migrations | tail -3`). If another branch took it, renumber before anything else, and say so: the old and new numbers, and the `_sqlx_migrations` recipe for a dev database that applied the branch's number.
  - Re-run the per-task gate, and record the old and new base shas.
- [ ] **Step 1: Set up the cluster.**
  - `kind create cluster --name egret-w9c-e2e`; build and `kind load` the backend image (and `egret-plugin-host:dev` if Task 7 rebuilt it).
  - Install the chart with `workflow.workerEnabled=true` and MinIO. If the chart's `deps.minio.image` pin answers 401 (as in W5b, W9a and W9b), `kind load` a local MinIO image and override `deps.minio.image`/`mcImage`, and report it.
  - **Never touch a kind cluster named `egret`.**
- [ ] **Step 2: Drive the scenario through the API**, with transcripts in the SDD workspace (`w9c-e2e-*`):
  1. `PUT` `cfg` (`{a:1}`), `n` (`1`), `s` (`hello world`) through the API; `GET` each (types JSON, NUMBER, STRING); `revisions` of `cfg` after a second `PUT`.
  2. A flow in that namespace that runs `Set` (`kvType: NUMBER`, `ttl: PT30S`), `Log` `{{ kv('n') }}` and `{{ kv('cfg').a }}`, `Get` from a child namespace's flow (inheritance), `Put` `{b: 2}` into `cfg`, `GetKeys`, `Delete`, and `PurgeKV` (`{type: version, keepAmount: 1}`). Check the API's `revisions` after each.
  3. **Expiry:** a key with `ttl: PT5S`; after 6 s `kv('k', errorOnMissing=false)` is `null`, and the API answers 404 with the row count unchanged. Run one sweep pass by restarting the server with a test-only short `KvSweepConfig`. If no config knob exists, the in-process sweep tests stand in; say which, and do not claim the hourly sweep was observed live.
  4. **Isolation:** a second workspace's flow `kv('cfg', '<first ns>')` fails naming the key as missing; its API answers as an empty namespace; a flow that `Set`s another namespace of its own workspace fails naming the namespace.
  5. **Publish:** a flow with `value: "{{ secret('X') }}"` on `Set` is refused with `secret_in_kv_value`.
  6. **The plugin host:** a `kestra.…postgresql.Query` stays green (no regression); `namespaceKv()` itself is not reachable from a catalog plugin (Task 1 of W5b's catalog), so the new message is pinned by Task 7's tests only. Say so.
- [ ] **Step 3:** `kind delete cluster --name egret-w9c-e2e`.
- [ ] **Step 4: Coverage and docs.**
  - **`oracle-exec/README.md`** gains a `### W9c — the KV store` section after W9b's (`:9524-9706`), before `### What this ratchet does **not** prove` (`:9707`). It records:
    - the ratchet **measured** before (Task 1's `w9c-baseline-ratchet.txt`) and after (`executor_matches_golden -- --nocapture` now);
    - `retry-with-flowable-errors` and `valids/kv` before and after;
    - every live test with its gate;
    - what Step 2 measured live and what it did not;
    - the `_kv`-namespace count from Task 1.
  - **The programme spec §9, EN and VI** (`docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md:489`): the W9 row says W9c shipped. Add a dated **W9c correction** after W9b's (`:513-519`), in W9b's form: what exists, the one access function with two resources, the user's rulings, and what is still not true (a plugin KV bridge, `AllowFailure`, the UI W7). Update §3 finding 4 (`:93-98`).
  - **The release note**, in that correction:
    - migration 0175 (`workflow.kv_entries`);
    - publish now refuses a namespace with a `_kv` segment (Task 1's count says whom it affects), and `secret()` in a `Set`/`Put` value;
    - seven API routes under `workflow:read`/`workflow:manage`, with no new permission;
    - a KV value is at most `max_read_bytes`;
    - the hourly expiry sweep on every process with a store;
    - the plugin host's new `namespaceKv` message;
    - the known limits of spec §15 (the key `inheritance`, the 1,000-key bulk cap and the accepted lock windows included).
  - **The W9c spec, EN and VI:** the dated corrections the task reports list. Items 4-10, 12, 13 and 14 are already in the spec (the user's rulings of 2026-10-07); add only what the implementation measured otherwise.
  - **Sweep** (N examined / M corrected per population, `/usr/bin/grep` with file arguments): (1) `\bW9c\b` in `crates/egret-workflow/src`, `plugin-host/src` and both programme specs; (2) `namespaceKv` in docs and doc comments; (3) `"the namespace KV store"`.
- [ ] **Step 5: Full local CI**, compared with Task 1's baselines:
  - `cargo fmt --all --check`;
  - `rtk proxy cargo clippy --workspace --all-targets --keep-going`: **0** `warning:` locations, against `w9c-baseline-clippy-warn.txt`'s 0. Report the `-D warnings` run beside it, against `w9c-baseline-clippy-deny.txt`;
  - `cargo test --workspace --no-fail-fast` with the Global Constraints' env (commit first: it runs long), plus the MinIO-gated suites and `just test-shipping-resolution`.
    - **Every failure absent from `w9c-baseline-gate.txt` is this branch's until measured otherwise.** Classify each by running that test at the base sha.
    - The known wall-clock flakes (worker-lease, `flow_trigger_windows`, `task_logs`) are re-run alone, and both runs are reported;
  - `cargo test -p egret-server --lib`. In `frontend/`: `npm run gen:api` (never bare `orval`), `git diff --exit-code src/api/generated`, `npm run lint`, `npx tsc --noEmit`, `npm test`;
  - `mvn -f plugin-host/pom.xml verify` with `JAVA_HOME`;
  - `helm lint deploy/helm/egret`;
  - the ratchet line, beside Task 1's: **294/108/108/0**.
- [ ] **Step 6: Commit** `docs(workflow): W9c Task 8 — end to end on kind, coverage and the programme spec`. **Stop:** no push and no PR until the user asks.

---

## Self-review

**1. Spec coverage**, section by section:

| spec | plan |
|---|---|
| §1 what exists | anchors re-read at `e04e452d`; item 10 corrects one |
| §2 Kestra vs egret, every row | Task 1 measures each and marks the chosen cells; each row's behaviour is a test in Tasks 2-7 (scope T2/T5, lock T3, row-after-object T3, expiry T2-T6, walk T2/T4, sweep scope T3, secret T4, cap T3/T6, key length T2, stored type T2, `_kv` T2, revisions route T6, `GET /kv` per namespace T6, PurgeKV default and dot boundary T5, `expiredOnly: false` T5, version-with-neither T5, bulk all-or-nothing T3/T6, plugin host T7) |
| §3 layout, `_kv`, key rule, key budget, staging | T2 (layout, rule, `_kv`), T3 (staging); the budget needs no new check (no task) |
| §4.1 table | T2 (item 12 for the index) |
| §4.2 values, types, size | T2 (encode/decode), T3 (cap on write), T5 (coercion), T6 (API inference) |
| §4.3 writing | T3, with the hold audit |
| §4.4 revisions, soft delete | T2 (reads), T3 (delete), T6 (routes) |
| §4.5 reading, expiry, reads never write | T2 (`a_read_never_writes`), T4, T6 (`no_get_route_writes`) |
| §4.6 inheritance | T2 (`resolve_first_live`), T4 (`kv`), T5 (`Get`), T6 (`inheritance`) |
| §4.7 one access function | T2 |
| §5 who may read and write | T2, with a refusal test per rule |
| §6 `kv()` | T4 |
| §7 secrets | T4 |
| §8.1-§8.6 tasks | T5 (T3 for the store side) |
| §9 API | T6 (items 5, 6, 9, 14) |
| §10 plugin host | T7 (item 10) |
| §11 sweep, re-claims, classification | T3 (sweep, re-claim at the store), T5 (re-claim through tasks, outages), T4 (render outage), T6 (503) |
| §12 constraints | Global Constraints |
| §13 testing | each bullet maps to a named test in T2-T8 |
| §14 out of scope | nothing planned for it |
| §15 known limits | T8's release note |

No requirement is left without a task.

**2. Placeholder scan.** No "TBD", "TODO", "fill in" or "similar to Task N". Where a value is the fixture's (bytes, messages, orders, `size`), the plan names the exact `w9c-kv.json` cell that supplies it, because the spec defers those values to Task 1 by design. One block is marked **sketch** (`on_ns_conn`): its lifetimes depend on `ns_lookup`'s loop, which the implementer reads first.

**3. Type consistency**, checked across tasks:
- `KvKey` (T2) is used in T3-T6.
- `KvType`/`KvValue` are defined in T2; `coerce` (T5) and `parse_api_body` (T6) return `KvValue`.
- `KvEntry` comes from T2. `KvWritten`, `NewValue` and `Deleted` come from T3 and are used by T5 and T6.
- `Resource::{NamespaceFiles, Kv}` and `Actor::System` come from T2; the sweep in T3 uses `Actor::System`.
- `kv_lock_text` (T2) is used by T3.
- `resolve_first_live` and `ancestors` (T2) are used by T4, T5 and T6.
- `KvArgs`/`KvAnswer` come from T4.
- The six FQCN constants are created in T4 and used by T4's secret scan and T5's registry.
- `DeclaredClass` is W9b's (`namespace.rs:155`), reused in T5.
- `spawn_kv_expiry_sweeper`/`KvSweepConfig` come from T3 and are wired in T3's `egret-bin` step.
- `purge_key`'s `judge` is `FnOnce(&[KvEntry]) -> Vec<i64>` in both T3 and T5.
- `nsfiles::routes::grant` gains `Resource` in T6, and T2's caller update does not touch it (it is called only from routes). T6 changes its one signature and its ten callers in `nsfiles/routes.rs`.
