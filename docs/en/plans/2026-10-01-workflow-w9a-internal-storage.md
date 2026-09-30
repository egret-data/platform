# W9a — Internal storage, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A flow's files live in egret's object store under `kestra://` URIs, as in Kestra: FILE inputs, script `inputFiles`/`outputFiles`, the four file functions, the execution file API, Kestra plugins that read and write storage, `egret.sql` with `fetch: STORE`, the core storage tasks and `Loop` over a file, with a 30-day retention pruner. A task pod holds no storage credential: egret moves every byte.

**Architecture:**
- `egret-storage` gains four streaming methods on `ObjectStore` (8 MiB parts, never a whole file in memory).
- `egret-workflow` gains a `storage` module: URI ↔ object key, the index `workflow.execution_files`, **one** readability function (`resolve_readable_many`), the process-wide `FileStore`, and the pruner.
- Every render with an execution gets a **snapshot**: before rendering, egret looks up every `kestra://` string in the flow source and in the render context, through `resolve_readable_many`, on the connection that renders. The file functions, script staging and the plugin bridge all answer from that snapshot, so there is one readability decision.
- Task files are uploaded during the run and indexed in `apply_outcome`'s transaction, behind W4's lease fence.
- W5a's two runners stream tar in and out; the in-memory packer is replaced.

**Tech Stack:** Rust (tokio, sqlx, minijinja, axum 0.8 `multipart`, aws-sdk-s3 1.141, kube, `ion-rs` 1.0.1 reader), Java 25 + Maven (Kestra `io.kestra:core:2.0.0-rc13`), MinIO, Docker, Helm, kind.

**Spec:** `docs/{en,vi}/specs/2026-10-01-workflow-w9a-internal-storage-design.md`. **§n refers to the spec.** The spec's §12 lists ten tasks; this plan has fourteen. The mapping and the reason for each split are in *Task map* below.

**A warning about this document.** Code blocks show shapes to verify, not text to paste. Every existing name below was read at `72d35839`; every new name is defined in the task that introduces it and used with the same spelling afterwards. Wherever the plan disagrees with the code, the plan is wrong, and the implementer says so in the report. Past plans in this programme were wrong in 4–6 places each; see memory `plan-illustrative-code-is-a-guess`.

## Global Constraints

- **User decisions (spec header), verbatim in effect:**
  - W9 is split: **W9a** internal storage (this plan), **W9b** namespace files, **W9c** the KV store.
  - **egret moves files for the pod.** A task pod holds no storage credential and opens no connection to the object store.
  - Streamed, with large configurable caps; **no file is ever held whole in egret's memory.**
  - A running flow reads its own execution's (its loop root's) files and its trigger execution's, and a replay reads its source's (§4.4). Never another workspace's; never an unrelated execution's in the same workspace.
  - Retention of its own, **30 days** by default, `0` keeps forever (§10).
  - `outputs.<task>.outputFiles` becomes `{relative path → kestra:// URI}` — **breaking** (§6.2).
  - In scope: FILE inputs + file functions (§7), the execution file API (§8), the plugin storage bridge (§9.1), `egret.sql` STORE (§9.2), core storage tasks + `Loop` over a file (§9.3).
- **Defaults, verbatim:**
  - `workflow.storage.prefix` `workflow/`; `workflow.storage.max_file_bytes` 1 GiB; `workflow.storage.max_task_bytes` 5 GiB; `workflow.storage.max_read_bytes` 10 MiB; `workflow.storage_retention_days` 30 (0 = forever).
  - Multipart part size 8 MiB (a single PUT under 8 MiB).
  - Preview `maxRows` default 100, max 5000.
  - Orphans older than 24 hours.
- **URIs (§3):** `kestra:///{ns-as-path}/{flow-slug}/executions/{exec}/{tail}`, tail `inputs/{input}/{file}` | `tasks/{task-slug}/{taskRunId}/{file}` | `trigger/{trigger-slug}/{file}`. **The URI carries no workspace**; the key is `{prefix}{workspace_id}{uri path}`, the workspace always being the running execution's. `{exec}` is the **loop root**. A URI is refused if the scheme is not `kestra`, it holds `..`, or it does not parse.
- **Errors (§11).** Every storage error names the URI and never a secret value. An unreachable store at run is an **infrastructure** error: the handler returns `Err` (a worker failure — the lease lapses, the run is re-claimed, no `attempts` is spent, W4), never `Ok(Outcome FAILED)`. A crossed cap, an unreadable URI or a bad file is the author's: a failed task naming the cap or the URI. This is the split `handlers::container::mapped` already makes (`container.rs:120-128`).
- **One readability function.** `storage::resolve_readable_many` is the only code that decides whether a URI is readable. The file functions, staging, the plugin bridge, FILE inputs given as JSON and the file API all reach it (through the render snapshot or directly). A second predicate anywhere is a review failure.
- **Every refusal has a test that fails when the check is removed** (mutation-minded). Where a refusal exists at save or start time and again at run, both go through one shared function, and the test says which.
- **Render-time reads need the multi-thread runtime.** `read()` blocks the render thread on a runtime handle while the calling tokio worker waits in `join`. Production (`#[tokio::main]`) is multi-thread. Every test that renders a file function against a real store uses `#[tokio::test(flavor = "multi_thread")]`; a `current_thread` test deadlocks on S3 (MinIO) and passes on `MemStore`, which proves nothing about S3.
- **The ratchet** is measured before (Task 1) and after (Task 14); both numbers go in `oracle-exec/README.md`. The spec expects `valids/loop-values-from-uri` and `valids/loop-outputs-auto` as targets (§9.3); banking either is progress only if measured.
- **Mutation discipline** (as W5b):
  - Every mutation shows evidence that it applied (`git diff --stat` plus a token count), a run count, an unmutated control, and the assertion that killed it.
  - A mutant that fails to compile proves nothing. A Java, SQL or YAML mutant is proven by running that artefact.
  - Never commit a mutant. Never mutate while a build or test runs.
- **A zero is reported beside a number that can disagree.** Sweeps are N examined / M corrected per population, using `/usr/bin/grep` with file arguments (never `rtk grep` piped; memory `rtk-grep-swallows-stdin`).
- **Commits:** Conventional Commits; trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; the worktree's configured identity (`hoangquocvietbk2.13@gmail.com`), never `--author`. Check `git branch --show-current` == `feat/workflow-w9a-internal-storage` before every commit (memory `a-detached-head-passes-every-check`). Never commit `.superpowers/`, jars, `plugin-host/target/`, or a mutant. **No PR.**
- **Environment:**
  - Worktree `/Users/1pro-l4r8/Documents/github/data-portal/egret-w9a`. Dedicated Postgres `egret-w9a-postgres` on **:5438**.
  - `DATABASE_URL=postgres://egret:egret@localhost:5438/egret_test` (export it; memory `sqlx-test-writes-into-the-dev-database`), `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`, `EGRET_TEST_STARROCKS_URL=mysql://localhost:9030`.
  - **MinIO:** the dev container `egret-dev-minio-1` on `:9000`, root `egret`/`egretsecret`, bucket `warehouse` exists. Storage tests reach it through the existing gate `EGRET_TEST_S3_ENDPOINT=http://localhost:9000` plus **`EGRET_TEST_S3_KEY=egret EGRET_TEST_S3_SECRET=egretsecret EGRET_TEST_S3_BUCKET=warehouse`** — set all four: `crates/egret-storage/tests/s3_minio.rs` defaults the key to `minioadmin`, which this MinIO refuses.
  - `JAVA_HOME=/Users/1pro-l4r8/Library/Java/JavaVirtualMachines/amazon-corretto-25.jdk/Contents/Home`; the plugin host is `plugin-host/`.
  - `CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target` is shared: `pgrep -lx cargo` first, never two cargo test processes at once (memory `no-concurrent-test-suites-and-tail-buffering`), keep ≥ 10 G free. An `E0432` that looks like a missing export is first retried with a private target dir (memory `shared-cargo-target-across-checkouts`).
  - Commit before any suite that may run over ~120 s (memory `long-commands-are-auto-backgrounded`).
  - Never `docker image prune -a`. The kind cluster `egret` is shared; the e2e uses a **temporary** cluster `egret-w9a-e2e`.
  - Live tests are gated by env vars and report IGNORED (never passed) without them, following `tests/container_docker_live.rs`'s hand-rolled main.

## Task map

| plan task | spec §12 | why split |
|---|---|---|
| 1 Measure Kestra | 1 | — |
| 2 `egret-storage` streaming | 2 (first half) | a different crate with a different instrument (MinIO + resident memory); a reviewer can reject the multipart upload and accept the index |
| 3 The index, URIs, `resolve_readable_many`, `FileStore` | 2 (second half) | Postgres-only; the security core of the package deserves its own gate |
| 4 The file functions and the render snapshot | 4 (first half) | **moved before §12.3**: staging (Task 5) and the plugin bridge (Task 9) resolve URIs through the snapshot this task builds; building it first is what keeps one readability path |
| 5 Streamed transfer in; `inputFiles` URIs | 3 (first half) | both runners change their input path; the output path is a separate rejection surface |
| 6 Collection out; `outputFiles` as URIs; commit-time indexing | 3 (second half) | the breaking change and the lease-fenced index write |
| 7 FILE inputs | 4 (second half) | an HTTP route + the four execution-creation sites; nothing shared with Task 4's render path |
| 8 Execution file API | 5 | — |
| 9 Plugin storage bridge and catalog | 6 | — |
| 10 `egret.sql` STORE | 7 | — |
| 11 `Write`, `Size`, `Delete`, `PurgeCurrentExecutionFiles`, `Loop` over a file | 8 (first half) | nine handlers + a `Loop` change is two reviewer gates; this half is what the two target corpus flows need |
| 12 `Concat`, `Split`, `Reverse`, `DeduplicateItems`, `FilterItems` | 8 (second half) | file transformers, two of them rendering an expression per record |
| 13 The pruner | 9 | — |
| 14 End to end on kind, coverage, docs, full CI | 10 | — |

**Order:** 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14. Each task's *Consumes* names what it needs from earlier ones.

## File Structure

```text
migrations/0161_workflow_execution_files.sql                 NEW  T3: the index; executions.replayed_from_execution_id
migrations/0162_workflow_execution_files_pruned.sql          NEW  T13: executions.files_pruned_at, files_swept_at
crates/egret-storage/Cargo.toml                              MOD  T2: tokio (normal dep), `test-util` feature
crates/egret-storage/src/lib.rs                              MOD  T2: put_stream, get_stream, head, delete_prefix; MemStore re-export
crates/egret-storage/src/s3.rs                               MOD  T2: the four methods on S3Store
crates/egret-storage/src/error.rs                            MOD  T2: StorageError::Unsupported
crates/egret-storage/src/mem.rs                              NEW  T2: MemStore (feature `test-util`)
crates/egret-storage/tests/s3_stream.rs                      NEW  T2: MinIO, > one part, resident memory
crates/egret-config/src/lib.rs                               MOD  T3: WorkflowConfig.storage, storage_retention_days
crates/egret-bin/src/main.rs                                 MOD  T3: storage::install; T13: the pruner's retention
crates/egret-workflow/Cargo.toml                             MOD  T3 egret-storage normal dep; T7 axum `multipart`,
                                                                  tokio-util `io`; T8 ion-rs; self dev-dep `test-util`
crates/egret-workflow/src/storage/mod.rs                     NEW  T3: FileStore, Caps, install/current, IndexedFile,
                                                                  Unreadable, StoredFile, index writers
crates/egret-workflow/src/storage/uri.rs                     NEW  T3: ExecUri, Tail, parse, slugify_str, id_utils_from,
                                                                  task_prefix, output_file_uri
crates/egret-workflow/src/storage/index.rs                   NEW  T3: loop_root, readable_executions, resolve_readable_many
crates/egret-workflow/src/storage/scope.rs                   NEW  T4: Snapshot (implements runners::files::RenderFiles)
crates/egret-workflow/src/storage/transfer.rs                NEW  T5: streaming ustar writer/reader (replaces worker/k8s/transfer.rs)
crates/egret-workflow/src/storage/collect.rs                 NEW  T6: one collected entry -> put_stream -> StoredFile; caps
crates/egret-workflow/src/storage/inputs.rs                  NEW  T7: FILE inputs (multipart parts, JSON URI check)
crates/egret-workflow/src/storage/routes.rs                  NEW  T8: the three file routes
crates/egret-workflow/src/storage/ion.rs                     NEW  T8: ion-rs line -> JSON (preview, Loop values)
crates/egret-workflow/src/storage/prune.rs                   NEW  T13
crates/egret-workflow/src/runners/files.rs                   NEW  T4: RenderFiles, FileScope, FILE_SCOPE
crates/egret-workflow/src/runners/{env.rs,functions.rs,filters.rs,mod.rs}   MOD T4
crates/egret-workflow/src/worker/{mod.rs,claim.rs,render.rs,commands.rs}    MOD T4-T7
crates/egret-workflow/src/worker/k8s/{runner.rs,transfer.rs}                MOD/DEL T5-T6
crates/egret-workflow/src/worker/handlers/{script.rs,container.rs,kestra.rs,sql.rs}  MOD T5,T6,T9,T10
crates/egret-workflow/src/worker/handlers/storage.rs         NEW  T11-T12: the nine core.storage handlers
crates/egret-workflow/src/worker/{registry.rs,subflow.rs,spawn.rs,replay.rs}          MOD T7, T11
crates/egret-workflow/src/scripts/mod.rs                     MOD  T5 (inputFiles), T6 (outputDirectory)
crates/egret-workflow/src/plugins/{mod.rs,catalog.json}      MOD  T9
crates/egret-workflow/src/spec/registry.rs                   MOD  T11-T12: nine TASK_TYPES rows
crates/egret-workflow/src/models/input.rs                    MOD  T7: allowed_file_extensions
crates/egret-workflow/src/routes.rs                          MOD  T3 (replay link), T7 (multipart start), T8 (router, OpenAPI)
crates/egret-workflow/src/runtime.rs                         MOD  T13: the storage pruner beside the log pruner
crates/egret-workflow/src/executor/flowable/loop_.rs         MOD  T11: the named gap's doc
crates/egret-workflow/oracle-exec/{w9a-storage.json,driver.rs,README.md,baseline.txt}   T1, T11, T14
crates/egret-workflow/tests/{storage_index.rs,storage_render.rs,storage_files_api.rs,storage_tasks.rs,storage_prune.rs}  NEW
plugin-host/src/main/java/io/kestra/core/runners/{EgretStorage.java,ShimRunContext.java,EgretPluginHost.java}  T9
plugin-host/src/test/java/io/kestra/core/runners/EgretStorageTest.java     NEW T9
openapi.json, frontend/src/api/generated/**                  REGEN T7, T8 (`npm run gen:api`, never bare orval)
docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md   MOD  T14
```

---

## Task 1: Measure Kestra, and the baseline

**Files:**
- Create `crates/egret-workflow/oracle-exec/w9a-storage.json`.
- Reuse `oracle-exec/docker-compose.yml` + `docker-compose.w5b.yml` (the JDBC jars and the three engines). Add nothing unless a measurement needs it; if it does, `docker-compose.w9a.yml`, and say why.
- Amend spec §3, §7.2, §9.2, §9.3 (EN and VI) **only** where a measurement contradicts them.

**Interfaces:**
- Produces `w9a-storage.json`, shaped like `w5b-plugins.json`: `artifact`, `artifact_note`, `captured_at`, `method`, `how_to_read_for_egret`, `findings` (quoted values), `egret_expected` (typed, below), `raw_records`. Later tasks read `egret_expected` with `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w9a-storage.json"))`.

| `egret_expected` key | shape | consumed by |
|---|---|---|
| `slugify` | `[{input, output}]`, ≥ 12 cases: spaces, `_`, accents, uppercase, `中文`, leading/trailing `-` | T3 |
| `id_utils_from` | `[{input, output}]`, ≥ 8 strings, incl. an absolute path with a space and a non-ASCII name | T3 |
| `output_file_uri` | `[{working_dir, rel_path, uri}]` from a `shell.Commands` with `outputFiles` `a b.txt`, `sub/x.csv`, `*.log`, and `outputDirectory: true` | T3, T6 |
| `output_directory` | how `outputDirectory: true` is exposed (the render variable, the env var, the directory name) and how its files are keyed in `outputFiles` | T6 |
| `script_output_files_map` | one run's full `outputs` of a script task | T6 |
| `input_files_uri` | a script task whose `inputFiles` value is a `kestra://` URI: where the file lands, and the task's view of it | T5 |
| `file_functions` | `{own, loop_root_from_iteration, trigger_subflow_parent, trigger_flow_source, unrelated_same_namespace, missing_in_own, malformed, not_utf8}` × `{read, fileSize, fileExists, isFileEmpty}` → `{"value": …}` or `{"error": "…"}` | T4 |
| `file_input` | the multipart request, the resulting `inputs.<id>` URI tail (`inputs/<id>/<filename>`), a text part's value, the `allowedFileExtensions` refusal (status, message), and a FILE input given as a JSON string (accepted or not, message) | T7 |
| `file_api` | for `.ion`, `.csv`, `.jsonl`, `.txt` and a binary file: `/file` status + `Content-Type` + `Content-Disposition`; `/file/metas` body; `/file/preview` body (the row shape of an ION struct holding a timestamp, a decimal, a null and a nested list); a URI of another execution (status, `Location`) | T8 |
| `plugin_storage` | per FQCN of the three jars: `postgresql.CopyIn`, `CopyOut`, `Batch`; `mysql.Batch`; `mysql.Query`/`Queries` `inputFile`; every `Query`'s `fetchType: STORE` and `store: true` — run on Kestra, outputs recorded; plus `still_refused_values` in `plugins::refused_values_json()`'s shape (`{}` if everything runs) | T9 |
| `store_outputs` | `postgresql.Query` `fetchType: STORE`: output keys, what `size` counts, the file name pattern (and whether it is random), the extension | T10 |
| `ion_rows` | `[{sql_type, sql_literal, ion_text}]`: int, bigint, decimal, float, bool, text with `"`, timestamp, timestamptz, date, null, bytea | T10 |
| `core_storage` | per class of `Write, Concat, Split, Size, Delete, Reverse, DeduplicateItems, FilterItems, PurgeCurrentExecutionFiles`: properties (`required`, `default`), outputs, and **one run** with fixed inputs: outputs and the bytes of every file written | T11, T12 |
| `loop_over_ion` | `valids/loop-values-from-uri` and `valids/loop-outputs-auto` run fresh: every `loopRun.value`; plus a file holding a number, a nested struct, a list and a `null` record (Kestra: `Found a null value in the ION file`) | T11 |
| `connector_streaming` | `{trino: bool, starrocks: bool, evidence: "file:line"}` for **egret's** connectors | T10 |

- [ ] **Step 1: Bring up** the pinned artifact: `docker compose -f docker-compose.yml -f docker-compose.w5b.yml up -d` in `oracle-exec/`. Confirm `GET /api/v1/main/plugins` lists `io.kestra.plugin.core.storage` and the three JDBC groups (`harvest.py`'s `BASE`/`TENANT` and `multipart()` are the client to reuse).
  - **Hazard (W13b):** never `docker exec psql` as a child of the artifact's postmaster; use the API or a separate container.
  - If the image refuses the jars, **stop and report**; do not switch images.
- [ ] **Step 2: Measure** every row of the table, saving raw execution JSON, file bytes (via `GET …/executions/{id}/file`) and HTTP headers:
  1. `slugify`/`output_file_uri`: flows whose `id` and task ids carry the cases, writing through `shell.Commands` `outputFiles` and `core.storage.Write`. Record the Kestra working directory's absolute path per run: the file name is `IdUtils.from(path.toString()) + "-" + name` with spaces → `+` (`FilesService.java:94-97`), so the **absolute** path decides it.
  2. `id_utils_from`: run `io.kestra.core.utils.IdUtils.from(s)` in `jshell` with the classpath `plugin-host/target/lib/*` (built by `mvn -f plugin-host/pom.xml -q dependency:copy-dependencies` if absent). `IdUtils.from` is md5 → `UUID.nameUUIDFromBytes` → `FriendlyId.toFriendlyId` (`IdUtils.java:13,20-26`).
  3. `file_functions`: an unrelated execution is a second flow in the same namespace whose file URI is passed in as a STRING input (Kestra OSS allows it, §2). `missing_in_own` is a well-formed URI of the running execution that names no file.
  4. `file_api`, `file_input`, `plugin_storage`, `store_outputs`, `ion_rows`, `core_storage`, `loop_over_ion`, `output_directory`, `input_files_uri`: as the table says.
  5. `connector_streaming`: **egret's** side, no Kestra. Read `crates/egret-query/src/execution/connector/trino.rs` (`execute`, `:450`), `connector/starrocks.rs` (`execute` `:251`, `drain_rows` `:464`) and `execution/port_impl.rs` (`run_bounded` `:352`). `QueryExecPort::execute_as_with_params` (`crates/egret-query-api/src/port.rs`) returns a whole `TabularResult`, so nothing streams **through the port**; record that, and whether each connector streams **internally**, with file:line.
- [ ] **Step 3: Save** the fixture with timestamps and the image digest. Where a question cannot be measured, say so in `findings` and mark the spec's answer "chosen".
  - `fileExists` on an unreadable URI (§7.2): the spec follows Kestra's answer. Record it for both `missing_in_own` and `unrelated_same_namespace` (the second is readable on Kestra OSS and **not** on egret, so egret's answer there is the spec's error, whatever Kestra says).
  - If `store_outputs` shows Kestra's STORE file name is random (a temp file), record it: egret uses a deterministic name instead (§4.3 needs re-claims to overwrite), and Task 10 names it.
- [ ] **Step 4:** `docker compose … down -v`.
- [ ] **Step 5: Baseline gate** at the branch base, after the artifact is down, into the SDD workspace:
  - `cargo test -p egret-workflow` on `:5438`, the name/result list as `baseline-gate.txt`;
  - the ratchet: `cargo test -p egret-workflow --test executor executor_matches_golden -- --nocapture`, its `N golden entries, M mismatching, B baselined, X broken` line as `baseline-ratchet.txt`;
  - `cargo clippy --workspace --all-targets -- -D warnings 2>&1`, the sorted list of `error:` locations as `baseline-clippy.txt`. It is red on `origin/main` from newer toolchain lints (egret-perm, egret-perm-compile, egret-workspace `members.rs`, egret-engine-mgmt, egret-workflow `spec/validate.rs:159` and `tests/executor.rs:5280`); Task 14 gates on **no new** location.
- [ ] **Step 6:** Commit `docs(workflow): W9a Task 1 — internal storage measured on the artifact`.

---

## Task 2: `egret-storage` streams

**Files:**
- Modify `crates/egret-storage/Cargo.toml`: `tokio.workspace = true` under `[dependencies]` (today a dev-dependency only); `[features] test-util = []`; a self dev-dependency `egret-storage = { path = ".", features = ["test-util"] }` (the `egret-kernel` pattern, `crates/egret-kernel/Cargo.toml:8,51`).
- Modify `src/lib.rs`, `src/s3.rs`, `src/error.rs`. Create `src/mem.rs`, `tests/s3_stream.rs`.

**Interfaces:**

```rust
// crates/egret-storage/src/lib.rs — added to `trait ObjectStore` (existing methods unchanged)
/// A single PUT when the body is under PART_BYTES, else a multipart upload in
/// PART_BYTES parts, aborted on any error. Returns the bytes written.
async fn put_stream(
    &self,
    uri: &str,
    body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
    size_hint: Option<u64>,
) -> Result<u64, StorageError> {
    let _ = (uri, body, size_hint);
    Err(StorageError::Unsupported("put_stream"))
}
async fn get_stream(&self, uri: &str)
    -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
    let _ = uri;
    Err(StorageError::Unsupported("get_stream"))
}
/// The object's size, `None` when absent.
async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
    let _ = uri;
    Err(StorageError::Unsupported("head"))
}
/// Deletes every object whose key starts with `uri`'s key. Returns how many.
async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
    let _ = uri;
    Err(StorageError::Unsupported("delete_prefix"))
}

pub const PART_BYTES: usize = 8 * 1024 * 1024;

// src/error.rs
#[error("this object store does not implement {0}")]
Unsupported(&'static str),

// src/mem.rs, #[cfg(feature = "test-util")], re-exported as `egret_storage::MemStore`
#[derive(Default, Clone)]
pub struct MemStore { /* Arc<Mutex<BTreeMap<String /*s3 uri*/, Vec<u8>>>> */ }
impl MemStore { pub fn new() -> Self; pub fn keys(&self) -> Vec<String>; }
```

The defaults exist because six test fakes implement `ObjectStore` today (`crates/egret-dataapi/tests/serving_starrocks_live.rs:97`, `crates/egret-query/tests/{execution_http.rs:18,execution_job.rs:90,execution_limit.rs:45,port_describe.rs:48}`, `crates/egret-workflow/tests/sql_starrocks_live.rs:107`) and none needs streaming. A default that **errors** is the only safe one (a default `Ok` would be the silent-success shape). `S3Store` and `MemStore` override all four, and a test says so.

- [ ] **Step 1: Write the failing tests.**

```rust
// src/mem.rs (unit, feature test-util)
#[tokio::test] async fn mem_store_round_trips_a_stream_and_reports_its_size() {}
#[tokio::test] async fn mem_store_delete_prefix_removes_only_that_prefix() {} // a sibling prefix `a/b2/` survives `a/b/`
#[tokio::test] async fn head_of_an_absent_object_is_none_not_an_error() {}

// tests/s3_stream.rs — gated on EGRET_TEST_S3_ENDPOINT (+ KEY/SECRET/BUCKET, see Global Constraints)
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_larger_than_one_part_is_uploaded_in_parts_and_reads_back_byte_for_byte() {
    // 20 MiB + 3 bytes from a generated AsyncRead (a counter pattern, never a Vec);
    // put_stream returns 20 MiB + 3; head == Some(20 MiB + 3);
    // get_stream's bytes compared to the generator chunk by chunk.
}
#[tokio::test(flavor = "multi_thread")]
async fn resident_memory_does_not_grow_with_the_file() {
    // libc::getrusage(RUSAGE_SELF).ru_maxrss before and after streaming 256 MiB up and back
    // (ru_maxrss is BYTES on macOS, KiB on Linux -- normalise). Assert growth < 64 MiB.
    // Control, in the same test: put(uri, vec![0; 128 MiB]) -- the Vec path -- grows >= 128 MiB,
    // so the instrument can see growth at all.
}
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_body_aborts_the_multipart_upload() {
    // an AsyncRead that errors after 9 MiB: put_stream is Err, and
    // ListMultipartUploads for the key is empty afterwards (no billed leftovers).
}
#[tokio::test(flavor = "multi_thread")]
async fn delete_prefix_deletes_more_than_one_list_page() { /* 1001 small objects */ }
#[tokio::test(flavor = "multi_thread")]
async fn a_small_body_is_one_put() { /* 3 bytes; head == Some(3) */ }
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_credential_error_names_the_uri_and_not_the_secret() {
    // S3Store with secret_key "w9a-secret-probe"; get_stream -> Err; its Display contains the uri
    // and not "w9a-secret-probe" (control: the Debug of the config redacts it too, config.rs:15-26)
}
#[test] fn s3_and_mem_stores_override_all_four_streaming_methods() {
    // compile-level: call each method on both types in an async block that is only type-checked
}
```

- [ ] **Step 2: Run them to watch them fail** (`cargo test -p egret-storage --features test-util`; the MinIO ones with the four env vars).
- [ ] **Step 3: Implement.**
  - `put_stream`: read up to `PART_BYTES` into **one** reused buffer. If the body ends first, `put_object` (the existing `put`'s call shape, `s3.rs:52-61`). Else `create_multipart_upload`, then `upload_part` per buffer, `complete_multipart_upload` with the part list; on any error `abort_multipart_upload`, then return the error. `size_hint` is advisory only.
  - `get_stream`: `get_object` as `get` does (keep its `NotFound` mapping, `s3.rs:63-82`), then `out.body.into_async_read()` (aws-smithy-types 1.6.2 `byte_stream.rs:434`, feature `rt-tokio`, which the workspace enables) boxed.
  - `head`: `head_object`; a `NotFound` service error → `Ok(None)`.
  - `delete_prefix`: `list_objects_v2` flat with pagination (as `list` does, `s3.rs:115-160`), `delete_objects` in batches of 1000.
  - Keys: the four take `s3://bucket/key` URIs through `S3Store::split` (`s3.rs:39-47`), like `put`/`get`/`delete`.
- [ ] **Step 4: Run the tests to see them pass**, including every other crate that implements `ObjectStore` (`cargo test -p egret-query -p egret-dataapi --no-run` at least).
- [ ] **Step 5: Mutations:** (a) `put_stream` buffers the whole body before one PUT (the memory test must die, not only the part test); (b) no `abort_multipart_upload` on error; (c) `delete_prefix` without pagination; (d) `head` maps absence to `Err`.
- [ ] **Step 6: Commit** `feat(storage): W9a Task 2 — streamed put and get, head and delete_prefix`.

---

## Task 3: The index, URIs, `resolve_readable_many` and the `FileStore`

**Files:**
- Create `migrations/0161_workflow_execution_files.sql`, `crates/egret-workflow/src/storage/{mod.rs,uri.rs,index.rs}`, `crates/egret-workflow/tests/storage_index.rs`.
- Modify `crates/egret-workflow/src/lib.rs` (`pub mod storage;`), `Cargo.toml` (`egret-storage.workspace = true` moves to `[dependencies]`; a self dev-dependency with `features = ["test-util"]`, and `[features] test-util = ["egret-storage/test-util"]`).
- Modify `src/runners/filters.rs` (extract `slugify_str`), `src/runners/functions.rs` (`base62` → `pub(crate)`).
- Modify `crates/egret-config/src/lib.rs`, `crates/egret-bin/src/main.rs`.
- Modify `src/routes.rs` `replay_execution` (`:1902`) and `src/worker/claim.rs` `NewExecution` (`:1900`) + `start_execution_in` (`:1943`).

**Interfaces:**

```sql
-- migrations/0161_workflow_execution_files.sql (exact DDL; comments as 0138's)
CREATE TABLE workflow.execution_files (
  id            bigserial   PRIMARY KEY,
  workspace_id  uuid        NOT NULL,
  execution_id  uuid        NOT NULL,        -- the LOOP ROOT (spec §3)
  task_run_id   uuid,                        -- NULL for an input file; no FK, as task_logs.task_run_id (0159)
  uri           text        NOT NULL CHECK (left(uri, 10) = 'kestra:///'),
  size          bigint      NOT NULL CHECK (size >= 0),
  created_at    timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT workflow_execution_files_uri_key UNIQUE (workspace_id, uri),
  CONSTRAINT workflow_execution_files_execution_same_workspace
    FOREIGN KEY (execution_id, workspace_id)
    REFERENCES workflow.executions (id, workspace_id) ON DELETE CASCADE
);
CREATE INDEX workflow_execution_files_execution_idx
  ON workflow.execution_files (execution_id);

-- The manual replay's link (spec §4.4 item 3). routes.rs `replay_execution`'s own doc
-- says "no column carries it" and names this column as the fix. NO ACTION on delete,
-- for 0138's reason on trigger_execution_id.
ALTER TABLE workflow.executions
  ADD COLUMN replayed_from_execution_id uuid;
ALTER TABLE workflow.executions
  ADD CONSTRAINT workflow_executions_replayed_from_same_workspace
    FOREIGN KEY (replayed_from_execution_id, workspace_id)
    REFERENCES workflow.executions (id, workspace_id),
  ADD CONSTRAINT workflow_executions_replayed_from_not_self
    CHECK (replayed_from_execution_id IS DISTINCT FROM id),
  ADD CONSTRAINT workflow_executions_replayed_from_is_not_a_child
    CHECK (replayed_from_execution_id IS NULL OR parent_execution_id IS NULL);
```

`(id, workspace_id)` is unique on `workflow.executions` since 0138 (`workflow_executions_id_workspace_key`), which the composite FKs need. `left(…)` rather than `LIKE`: memory `sql-like-underscore-dropped-a-database`.

```rust
// src/storage/uri.rs
pub const SCHEME_PREFIX: &str = "kestra:///";
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tail {
    Input { input: String, file: String },
    Task { task_slug: String, task_run: String, file: String },
    Trigger { trigger_slug: String, file: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecUri { pub ns_path: String, pub flow_slug: String, pub execution: Uuid, pub tail: Tail }
pub fn parse(uri: &str) -> Result<ExecUri, String>;          // refuses a non-kestra scheme, `..`, an empty segment, a non-UUID {exec}
impl ExecUri { pub fn to_uri(&self) -> String; }             // parse(u)?.to_uri() == u for every accepted u
pub fn namespace_as_path(namespace: &str) -> String;         // dots -> slashes (StorageContext.namespaceAsPath)
pub fn slugify_str(input: &str) -> String;                   // Slugify.of; re-exported from runners::filters' body
pub fn id_utils_from(s: &str) -> String;                     // IdUtils.from: md5 -> nameUUIDFromBytes -> base62
pub fn task_prefix(namespace: &str, flow_id: &str, root: Uuid, task_id: &str, task_run: Uuid) -> String; // ends with '/'
pub fn input_uri(namespace: &str, flow_id: &str, root: Uuid, input: &str, file: &str) -> String;
/// `{task_prefix}{id_utils_from(abs_path)}-{basename with ' ' -> '+'}` (FilesService.java:94-97).
pub fn output_file_uri(task_prefix: &str, abs_path: &str) -> String;

// src/storage/mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps { pub max_file_bytes: u64, pub max_task_bytes: u64, pub max_read_bytes: u64 }
pub struct FileStore {
    pub store: std::sync::Arc<dyn egret_storage::ObjectStore>,
    pub bucket: String,
    pub prefix: String,
    pub caps: Caps,
    pub retention_days: u32,
}
impl FileStore {
    /// `s3://{bucket}/{prefix}{workspace}{path of uri}` — the workspace is the caller's, never the URI's.
    pub fn object_uri(&self, workspace: Uuid, uri: &str) -> Result<String, String>;
    pub fn execution_prefix(&self, workspace: Uuid, namespace: &str, flow_id: &str, root: Uuid) -> String;
}
/// Process-wide, set once by egret-bin. See "Why a process-wide store" below.
pub fn install(fs: std::sync::Arc<FileStore>) -> Result<(), std::sync::Arc<FileStore>>;
pub fn current() -> Option<std::sync::Arc<FileStore>>;
#[cfg(any(test, feature = "test-util"))]
pub fn install_mem_for_tests() -> std::sync::Arc<FileStore>;   // idempotent: MemStore, default caps

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFile { pub uri: String, pub execution_id: Uuid, pub task_run_id: Option<Uuid>, pub size: u64 }
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unreadable {
    #[error("{uri} is not a storage URI egret can read: {reason}")]
    Malformed { uri: String, reason: String },
    /// Its execution is not this execution's loop root, trigger execution or replay source.
    /// Deliberately one variant: the message never says whether the file exists.
    #[error("{uri} belongs to an execution this flow may not read: a flow reads only its own \
             execution's files, its trigger execution's and, for a replay, its source's")]
    NotReadable { uri: String },
    /// Its execution is readable and no such file was ever committed.
    #[error("{uri} does not exist")]
    Missing { uri: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredFile { pub uri: String, pub size: u64 }
pub async fn index_task_files(conn: &mut PgConnection, workspace: Uuid, task_run: Uuid, files: &[StoredFile]) -> egret_core::Result<()>;
pub async fn index_input_files(conn: &mut PgConnection, workspace: Uuid, execution: Uuid, files: &[StoredFile]) -> egret_core::Result<()>;

// src/storage/index.rs
/// Walk parent_execution_id while the row is a LOOP ITERATION (loop_index IS NOT NULL).
/// A retry replay also has parent_execution_id set, with loop_index NULL (0138's table;
/// worker/replay.rs's module doc), and must NOT be walked through.
pub async fn loop_root(conn: &mut PgConnection, workspace: Uuid, execution: Uuid) -> egret_core::Result<Uuid>;
/// {R} ∪ {trigger_execution_id of R} for R in the replay chain from loop_root(execution):
/// the root, then (parent_execution_id WHERE loop_index IS NULL) or replayed_from_execution_id,
/// transitively (a replay copies its source's inputs verbatim, replay.rs:254-257, routes.rs:1943).
pub async fn readable_executions(conn: &mut PgConnection, workspace: Uuid, execution: Uuid) -> egret_core::Result<Vec<Uuid>>;
/// THE readability decision. Never reads a row of an execution outside `readable`.
pub async fn resolve_readable_many(
    conn: &mut PgConnection, workspace: Uuid, readable: &[Uuid], uris: &[String],
) -> egret_core::Result<std::collections::BTreeMap<String, Result<IndexedFile, Unreadable>>>;
pub async fn resolve_readable(conn: &mut PgConnection, workspace: Uuid, current_execution: Uuid, uri: &str)
    -> egret_core::Result<Result<IndexedFile, Unreadable>>;   // readable_executions + resolve_readable_many, one element

// egret-config
pub struct WorkflowStorageConfig { pub prefix: String, pub max_file_bytes: u64, pub max_task_bytes: u64, pub max_read_bytes: u64 }
// WorkflowConfig gains `#[serde(default)] pub storage: WorkflowStorageConfig` and
// `#[serde(default = "default_storage_retention_days")] pub storage_retention_days: u32` (30),
// beside log_retention_days (crates/egret-config/src/lib.rs:584). Env: EGRET_WORKFLOW__STORAGE__MAX_FILE_BYTES, …

// claim.rs NewExecution gains (Task 7 adds `id` and `input_files`):
pub replayed_from_execution_id: Option<Uuid>,  // the manual replay's link
```

**Why a process-wide store.** `read()` runs inside `settle` (`runIf`, `Loop` values) which is reached from `apply_outcome`, `start_execution_in`, `resume`, the scheduler, the Flow-trigger processor, the webhook and every spawn — none has a store handle, and threading one through all of them is a larger change than this package. So the **object store** is installed once (`storage::install`, from `egret-bin` beside the `S3Store` it already builds at `main.rs:1573-1583`). **Index lookups never use the global**: they run on the connection the caller holds (Task 4's snapshot), so no pool outlives a test's runtime. Every function that decides anything takes the store as a parameter; only the wiring points call `current()`. List every `current()` call site in the task report.

- [ ] **Step 1: Write the failing tests.**

```rust
// src/storage/uri.rs (unit)
#[test] fn slugify_and_id_utils_match_the_artifact() { /* every w9a-storage.json slugify + id_utils_from row */ }
#[test] fn output_file_uri_matches_the_artifact() { /* every output_file_uri row */ }
#[test] fn parse_refuses_by_name() {
    // "s3://b/k", "file:///etc/passwd", "nsfile:///a", "kestra:///a/../b/executions/<uuid>/inputs/f/x",
    // "kestra:///ns/f/executions/not-a-uuid/inputs/f/x", "kestra:///ns/f/executions/<uuid>/other/x",
    // "kestra:///ns//f/executions/<uuid>/inputs/f/x" -> each Err naming the reason
}
#[test] fn parse_to_uri_round_trips() {}
#[test] fn the_object_key_takes_the_workspace_from_the_caller_and_nothing_from_the_uri() {
    // a URI whose ns-path is a UUID string equal to another workspace id: the key still starts with prefix+caller ws
}

// tests/storage_index.rs (Postgres :5438)
#[tokio::test] async fn a_loop_iteration_resolves_to_its_root_and_a_retry_replay_does_not() {}
#[tokio::test] async fn readable_covers_own_root_trigger_retry_replay_and_manual_replay_chain() {
    // root R (trigger T), retry-replay R1 of R, manual replay R2 of R1; a loop iteration I of R2:
    // readable_executions(I) == {R2, R1, R, T}
}
#[tokio::test] async fn a_file_of_an_unrelated_execution_in_the_same_workspace_is_not_readable() {
    // THE probe: the other execution's row EXISTS. Removing the `readable` filter makes it Ok.
}
#[tokio::test] async fn the_same_uri_in_two_workspaces_resolves_to_the_callers_row_only() {
    // identical uri strings indexed in ws A (size 1) and ws B (size 2); from A -> 1; from C -> NotReadable/Missing, never 2
}
#[tokio::test] async fn a_readable_execution_with_no_such_row_is_missing_not_unreadable() {}
#[tokio::test] async fn the_manual_replay_route_records_its_source() {
    // POST .../executions/{id}/replay -> the new row's replayed_from_execution_id == id (route test via tests/common app)
}
#[tokio::test] async fn the_composite_fk_refuses_a_file_row_naming_another_workspaces_execution() {}
#[tokio::test] async fn deleting_a_workspace_cascades_its_file_rows() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - `slugify_str`: move the body of `runners::filters::slugify` (`filters.rs:2242-2275`) into a `pub(crate) fn slugify_str(&str) -> String`, and have the filter call it (the filter keeps its Java-null arm).
  - `id_utils_from`: `md5(s)`, then `nameUUIDFromBytes` (md5 of those 16 bytes, version nibble 3, variant `10`), then `runners::functions::base62(u128::from_be_bytes(..), 0)` (`functions.rs:1403`, made `pub(crate)`). The fixture decides; do not argue from the Java.
  - `loop_root`/`readable_executions`: one recursive CTE each, scoped by `workspace_id` in every step.
  - `resolve_readable_many`: parse each URI; `Malformed` on failure; `NotReadable` when `{exec}` ∉ `readable`; else one `SELECT uri, execution_id, task_run_id, size FROM workflow.execution_files WHERE workspace_id = $1 AND uri = ANY($2)` over the readable ones; absent → `Missing`.
  - `index_*_files`: `INSERT … ON CONFLICT (workspace_id, uri) DO UPDATE SET size = EXCLUDED.size, task_run_id = EXCLUDED.task_run_id, created_at = now()` — a retried task run writes the same deterministic URIs (§4.3). Each file's `execution_id` is parsed from its URI and must be the loop root the caller passes; a mismatch is `Error::Internal` (egret built a wrong URI).
  - `replay_execution` (`routes.rs:1902`): switch from `worker::start_execution` to `start_execution_in` in a transaction with `replayed_from_execution_id: Some(id)`; update the handler doc's *"no column carries it"* paragraph (it names this column as the fix). `start_execution_in` binds the two new columns; every `NewExecution { … }` literal gets the new field (`flow_trigger/process.rs:917`, `scheduler/fire.rs:260`, `worker/claim.rs:1872`, `tests/worker.rs:7103,7130,7166,7245,7367,7566`).
  - Config: `WorkflowStorageConfig` with serde defaults; a boot check that `container_runner != none` requires `[storage]` `bucket`, `access_key`, `secret_key` non-empty (spec §15).
  - `egret-bin`: build the `FileStore` from the `storage` Arc already created at `main.rs:1582-1583` (reuse the Arc, not a second client) and `storage::install` it before the router and the runtime.
- [ ] **Step 4: Run the tests to see them pass**, then `cargo test -p egret-workflow --test worker --test executions_routes`.
- [ ] **Step 5: Mutations:** (a) `resolve_readable_many` without the `readable` filter; (b) the SELECT without `workspace_id`; (c) `loop_root` walking through a row with `loop_index IS NULL`; (d) `readable_executions` without the trigger step; (e) without the replay chain; (f) `object_uri` taking the workspace from the URI's first segment.
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 3 — the file index, kestra:// URIs and the one readability check`.

---

## Task 4: The file functions, on a snapshot taken before every render

**Files:**
- Create `src/runners/files.rs`, `src/storage/scope.rs`, `tests/storage_render.rs`.
- Modify `src/runners/env.rs` (`RenderContext` `:328`, `render_on_this_thread` `:2086`, `bind_on_this_thread` `:2289`), `src/runners/functions.rs` (`REFUSED` `:243-274`, its test `the_twelve_refusals_name_themselves_and_their_owner` `:3151`), `src/runners/mod.rs`.
- Modify `src/worker/render.rs` (`Surface` `:82`, `Surface::context` `:547`), `src/worker/claim.rs` (`input_for` `:1475`, `settle` `:1658`).

**Interfaces:**
- Consumes (Task 3): `storage::{current, FileStore, IndexedFile, Unreadable, readable_executions, resolve_readable_many, loop_root, uri::task_prefix}`.
- Produces:

```rust
// src/runners/files.rs — no sqlx, no storage types: runners stays below storage
pub trait RenderFiles: Send + Sync {
    /// The indexed file, or why it is not readable here. Never touches the database.
    fn lookup(&self, uri: &str) -> Result<FileMeta, String>;
    /// UTF-8 contents, capped at the store's max_read_bytes. Blocks on the runtime handle.
    fn read_utf8(&self, uri: &str) -> Result<String, String>;
    /// This run's baseUri (`…/tasks/{task}/{taskRunId}/`), None outside a task run.
    fn task_prefix(&self) -> Option<&str>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMeta { pub size: u64 }
/// Identity-compared (Arc::ptr_eq), so RenderContext keeps PartialEq + Eq.
#[derive(Clone)]
pub struct FileScope(pub std::sync::Arc<dyn RenderFiles>);
/// The render-time global the four functions look up. A space in it, like
/// CONTEXT_SCOPE_NAMES (env.rs:1669), so no template can name it.
pub(crate) const FILE_SCOPE: &str = "egret file scope";

// RenderContext (env.rs:328) becomes a two-field struct; its bindings API is unchanged:
impl RenderContext {
    pub fn with_files(self, files: FileScope) -> Self;
    pub fn files(&self) -> Option<&FileScope>;
}

// src/storage/scope.rs
pub struct Snapshot { /* files: BTreeMap<String, Result<IndexedFile, Unreadable>>, store: Arc<FileStore>,
                         workspace: Uuid, handle: tokio::runtime::Handle, task_prefix: Option<String> */ }
impl Snapshot {
    /// Every `kestra:///…` string in `texts` (regex, whole run of non-space, non-quote chars)
    /// resolved at once through resolve_readable_many on `conn`.
    pub async fn take(conn: &mut PgConnection, store: Arc<FileStore>, workspace: Uuid,
                      execution: Uuid, texts: &[&str], json: &[&serde_json::Value],
                      task_prefix: Option<String>) -> egret_core::Result<Self>;
    pub fn resolve(&self, uri: &str) -> Result<&IndexedFile, String>;   // what staging (T5) and the bridge (T9) call
    pub fn into_scope(self) -> crate::runners::files::FileScope;
}
```

**The rule this task implements, and where it narrows §7.2.** The spec says index lookups are *"prefetched for every `kestra://` literal in the text and looked up again for one assembled at render"*. The second half needs a database handle on the render thread; inside `settle` the only connection is the transaction's, which the render thread cannot use (and a pool would not see that transaction's uncommitted index rows — the just-finished task's files). So the snapshot is taken from **the flow source plus every JSON value of the render context** (`inputs`, `outputs` of every terminated task, `execution`, `item`): that covers every URI a template can reach without building it from pieces. A URI assembled at render that is not in the snapshot is an error naming the URI and saying why. **This is a spec ambiguity reported to the controller** (see the plan's report); if the user rules for a render-time lookup, it is a change inside `Snapshot::resolve` only.

- [ ] **Step 1: Write the failing tests.**

```rust
// runners/functions.rs (unit): a fake RenderFiles over a map
#[test] fn the_four_functions_answer_from_the_scope() {
    // read -> contents; fileSize -> size; isFileEmpty -> size == 0; fileExists -> true
}
#[test] fn file_exists_on_a_missing_file_is_the_artifacts_answer() { /* w9a-storage.json file_functions.missing_in_own */ }
#[test] fn every_other_unreadable_uri_is_an_error_naming_it() { /* NotReadable, Malformed: all four functions */ }
#[test] fn without_a_scope_the_four_are_refused_by_name_saying_they_need_an_execution() {}
#[test] fn file_uri_names_w9b_and_kv_names_w9c() {}
#[test] fn read_over_the_cap_is_an_error_naming_the_cap_and_at_the_cap_is_not() {}
#[test] fn read_of_non_utf8_is_an_error_naming_the_uri() {}
#[test] fn the_file_scope_global_is_unreachable_from_a_template() { /* every probe the_context_scope_global_is_unreachable_from_a_template uses, aimed at FILE_SCOPE */ }

// tests/storage_render.rs (Postgres; MemStore via install_mem_for_tests; flavor = "multi_thread")
#[tokio::test(flavor = "multi_thread")] async fn a_run_if_reads_a_file_the_previous_task_committed_in_the_same_transaction() {
    // task a writes (indexed in apply_outcome's tx), task b runIf: "{{ fileSize(outputs.a.uri) > 0 }}"
    // -> b runs. This is the case a pool lookup would get WRONG.
}
#[tokio::test(flavor = "multi_thread")] async fn a_log_task_reads_its_trigger_executions_file() {}
#[tokio::test(flavor = "multi_thread")] async fn a_loop_iteration_reads_a_file_its_root_wrote() {}
#[tokio::test(flavor = "multi_thread")] async fn a_file_of_an_unrelated_execution_fails_the_render_naming_the_uri() {}
#[tokio::test(flavor = "multi_thread")] async fn a_flow_trigger_condition_calling_read_is_refused_by_name() {
    // execution_render_context (render.rs:808) carries no scope
}
#[tokio::test(flavor = "multi_thread")] async fn an_assembled_uri_not_in_the_snapshot_is_an_error_saying_why() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **Functions:** remove `read`, `fileSize`, `fileExists`, `isFileEmpty` from `REFUSED` and register them in `environment()` (`env.rs:1717`) as real functions taking `path` (named or positional, `Args::new(rest).get("path", 0)` as `from_json` does). Each looks up `FILE_SCOPE` with `state.lookup(..).and_then(|v| v.downcast_object_ref::<FileScopeObject>())` — the pattern `fetch_context` uses (`functions.rs:1719-1726`) — and without one returns `pebble_error("The '<name>' function needs a running execution …")`.
  - `fileURI` → `not_yet("namespace files", "W9b")`; `kv` → `not_yet("the namespace KV store", "W9c")`. `http` is left as it is and reported (it is owned by no package; the programme spec §9 note).
  - The `REFUSED` doc (*"The twelve names…"*) and the test `the_twelve_refusals_name_themselves_and_their_owner` shrink to eight names; rename the test for the new count.
  - **Both render threads** add the `FILE_SCOPE` global when `cx.files()` is `Some`: `render_on_this_thread` after `extend` (so an `extend` cannot unbind it) and `bind_on_this_thread` (`egret.sql`). `environment()` itself never holds it, so `the name-surface test` (`env.rs:2842-2851`) is unchanged; confirm.
  - `read_utf8` in `Snapshot`: `handle.block_on(store.get_stream(object_uri))`, read at most `max_read_bytes + 1` bytes; one more is the cap error.
  - **Where a scope is attached**, and nowhere else:
    - `input_for` (`claim.rs:1475`): after `Surface::load`, take a `Snapshot` on `conn` with `task_prefix = Some(uri::task_prefix(flow.namespace, flow.id, root, &c.task_id, c.id))`, attach to the `RenderContext` passed to `c.input(…)`.
    - `settle` (`claim.rs:1658`): after `Surface::load`, take a `Snapshot` with `task_prefix = None` and store it on the `Surface`; `Surface::context` attaches it. That reaches `render_run_if` (`render.rs:981`), `render_expressions` (`:1031`) and the SLA render (`claim.rs:2546`).
    - `execution_render_context` (`render.rs:808`) does **not** take one: a Flow trigger's `conditions`/`inputs` render refuses the four (§7.2).
    - With `storage::current()` `None`, no snapshot is taken and the four refuse, naming `[storage]`.
  - Texts for the snapshot: `ctx.source`, plus `Surface`'s `inputs`, `inherited`, `own`, `execution`, `item` JSON (`render.rs:82-94`); the handler's `TaskInput.spec` is inside `ctx.source`.
- [ ] **Step 4: Run the tests to see them pass**, then `cargo test -p egret-workflow --test oracle` (the render oracle holds `function-fileexists` and `function-isfileempty` with `compare: error-class`; both must still match) and the whole crate.
- [ ] **Step 5: Mutations:** (a) `settle` without the snapshot (the same-transaction test must die); (b) `Snapshot::take` without the render context's JSON (only the source); (c) the global installed in `render_on_this_thread` only (an `egret.sql` `sql:` using `fileSize` must die — add that unit test with a fake scope through `render_bound`); (d) `fileExists` answering `false` for `NotReadable`.
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 4 — read, fileSize, fileExists and isFileEmpty on a pre-render snapshot`.

---

## Task 5: Streamed transfer into a container; `inputFiles` URIs

**Files:**
- Create `src/storage/transfer.rs` (streaming ustar; the header, octal and long-name code moves from `src/worker/k8s/transfer.rs`, which is deleted at the end of Task 6).
- Modify `src/worker/commands.rs` (`TaskCommands` `:215`, `DockerRunner::run_created` `:1921`, `stage_input` `:2385`), `src/worker/k8s/runner.rs` (the upload at `:618`), `src/scripts/mod.rs` (`inputFiles` `:525-535`, `CODE_INPUT_FILES_URI` `:414`), `src/worker/handlers/script.rs` (`:235-300`).
- Tests beside each, plus `tests/container_docker_live.rs` and `tests/k8s_runner_live.rs`.

**Interfaces:**
- Consumes: Task 2's `get_stream`; Task 4's `Snapshot::resolve` via `task.render.files()`; Task 3's `FileStore::object_uri`.
- Produces:

```rust
// commands.rs — TaskCommands gains (Default stays derivable):
/// Files staged from internal storage: name relative to WORKING_DIR -> where egret reads them.
/// Never contents; the Debug impl prints names and sizes only.
pub stored_inputs: BTreeMap<String, StoredInput>,
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredInput { pub object_uri: String, pub size: u64 }

// storage/transfer.rs
pub enum TarSource<'a> { Contents(&'a str), Stored(&'a StoredInput) }
/// A ustar stream of every file (parents first), written as it is read: at most one
/// 512-byte block plus the store's read buffer in memory.
pub async fn write_tar(out: &mut (dyn tokio::io::AsyncWrite + Send + Unpin),
                       files: &BTreeMap<String, TarSource<'_>>, store: Option<&FileStore>) -> Result<(), String>;
```

- [ ] **Step 1: Write the failing tests.**

```rust
// storage/transfer.rs (unit)
#[tokio::test] async fn write_tar_is_byte_identical_to_the_old_packer_for_contents() { /* vs worker::k8s::transfer::pack */ }
#[tokio::test] async fn a_stored_input_is_copied_from_the_store_without_being_collected() {
    // 20 MiB MemStore object; a counting AsyncWrite; the peak bytes buffered by write_tar < 1 MiB
}
#[tokio::test] async fn a_stored_input_whose_object_is_shorter_than_its_size_is_an_error() {}
// scripts/mod.rs
#[test] fn an_input_files_value_that_is_a_uri_is_accepted_at_publish_and_a_string_map_still_is() {}
#[test] fn input_files_as_a_whole_string_is_still_refused_with_the_code() { /* CODE_INPUT_FILES_URI; message no longer says W5a only */ }
// handlers/script.rs (spy runner, as script.rs's tests do)
#[tokio::test] async fn a_uri_input_is_staged_under_its_name_and_never_as_contents() { /* per w9a-storage.json input_files_uri */ }
#[tokio::test] async fn an_unreachable_store_while_staging_is_a_worker_error_not_a_failed_task() {}
#[tokio::test] async fn an_unreadable_uri_fails_the_task_at_render_and_calls_no_runner() {}
// live
// container_docker_live: stage a 12 MiB stored input; `wc -c` in the container prints 12582912
// k8s_runner_live (EGRET_TEST_KIND=1): the same through exec `tar -x`
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **Publish** (`scripts/mod.rs:525`): a map **value** that is a `kestra://` string (or a template) is accepted; `inputFiles` given as one whole string keeps `CODE_INPUT_FILES_URI` and its message loses "(W5a)". That is the one save-time rule; run time re-checks the rendered value through `Snapshot::resolve` — the same readability decision, and the test above says so.
  - **Handler** (`script.rs`): after rendering `inputFiles`, a value that is a `kestra://` URI goes to `stored_inputs` (`object_uri` from `FileStore::object_uri(task.workspace_id, uri)`, `size` from the snapshot); any other value stays contents (§6.1). An unreadable URI fails the task **before** the runner is called.
  - **Docker** (`run_created`): replace the staging directory + `docker cp src dst` with `docker cp - <name>:<WORKING_DIR>` fed by `write_tar` on the child's stdin (`Stdio::piped()`, `kill_on_drop(true)` as `cli` does at `commands.rs:1433-1454`). Remove `stage_input` if nothing else calls it.
  - **Kubernetes** (`runner.rs:618`): replace `transfer::pack` + one write with `write_tar` into the exec's stdin writer.
  - `TaskCommands`' `Debug` (`commands.rs:277-298`) prints `stored_inputs` as names and sizes.
- [ ] **Step 4: Run the tests to see them pass**, both live suites with their gates, and the crate.
- [ ] **Step 5: Mutations:** (a) a URI value passed as contents (the spy test must die); (b) the render-time `resolve` skipped (the unreadable test must die, and the runner must be called — show it); (c) `write_tar` buffering the stored object (the peak-buffer test must die).
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 5 — files stream into a task container, and inputFiles take kestra:// URIs`.

---

## Task 6: Collecting out; `outputFiles` as URIs; indexing at commit

**Files:**
- Create `src/storage/collect.rs`.
- Modify `src/worker/commands.rs` (`CommandsResult` `:334`, `collect_output_files` `:2061`, `ingest_output_file` `:919`, `MAX_OUTPUT_FILE_BYTES` `:158` and its module doc `:95-105`), `src/worker/k8s/runner.rs` (`:774-790`), delete `src/worker/k8s/transfer.rs`.
- Modify `src/worker/handlers/container.rs` (`outputs_for` `:54-66` and its doc `:44-53`), `src/scripts/mod.rs` (`COMMON_REFUSED` `:195-207` loses `outputDirectory`, `COMMON_IMPLEMENTED` `:164-175` gains it).
- Modify `src/worker/mod.rs` (`Outcome` `:257`), `src/worker/claim.rs` (`apply_outcome` `:1511`).

**Interfaces:**
- Consumes: Task 2 `put_stream`, `head`; Task 3 `uri::output_file_uri`, `index_task_files`, `StoredFile`; Task 4 `RenderFiles::task_prefix`; Task 5 `storage::transfer`.
- Produces:

```rust
// commands.rs — TaskCommands gains
pub output_directory: bool,
/// Where collected files go. `None` with any output declared fails the task naming
/// `[storage]`: W5a's contents-in-outputs path no longer exists.
pub collect_to: Option<CollectTo>,
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectTo {
    pub workspace: Uuid,
    pub task_prefix: String,   // kestra:///…/tasks/{task}/{taskRunId}/
    pub naming: Naming,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Naming { KestraOutputName, UnderPrefixVerbatim }   // T9 uses the second for `.egret/out/`

// CommandsResult.output_files: BTreeMap<String /*relative path*/, crate::storage::StoredFile>

// storage/collect.rs
/// One file out of a container: stream to put_stream, count against both caps, head for the real size.
pub async fn collect_one(fs: &FileStore, to: &CollectTo, rel: &str,
                         body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
                         declared_size: u64, task_bytes_so_far: &mut u64) -> Result<StoredFile, String>;
/// §5: every URI under `task_prefix` that appears anywhere in `outputs` was collected.
pub fn check_outputs_collected(outputs: &serde_json::Value, task_prefix: &str,
                               collected: &[StoredFile]) -> Result<(), String>;

// worker/mod.rs — Outcome gains (every literal site updated; see Step 3)
/// Files this run wrote, indexed by apply_outcome in the fenced transaction (§4.3).
pub files: Vec<crate::storage::StoredFile>,
```

- [ ] **Step 1: Write the failing tests.**

```rust
// container.rs
#[test] fn output_files_are_uris_keyed_by_relative_path() { /* shape = w9a-storage.json script_output_files_map */ }
// collect.rs
#[tokio::test] async fn a_file_over_max_file_bytes_fails_naming_the_cap() {}
#[tokio::test] async fn files_over_max_task_bytes_together_fail_naming_the_cap() {}
#[test] fn an_output_pointing_under_the_prefix_at_nothing_collected_fails() {}
#[test] fn an_output_pointing_at_another_prefix_is_not_this_checks_business() {}
// scripts/mod.rs
#[test] fn output_directory_is_implemented_and_no_longer_refused() {}
// claim.rs / tests/worker.rs (Postgres)
#[tokio::test] async fn files_are_indexed_in_the_outcomes_transaction_and_not_before() {
    // a registry whose handler returns files: before commit no row; after, rows with task_run_id
}
#[tokio::test] async fn a_fenced_outcome_indexes_nothing() {
    // move claimed_at, apply_outcome -> LeaseLost, and 0 rows in execution_files for the run
}
#[tokio::test] async fn a_retry_attempt_overwrites_its_own_uris_without_a_unique_violation() {}
#[tokio::test] async fn an_unreachable_store_while_collecting_is_a_worker_error_not_a_failed_task() {}
// live (both runners): a re-attached container (W5a's adopt path, same (run, attempt)) is collected
// and its files indexed exactly once (§5 Re-attach)
// live: container_docker_live and k8s_runner_live
// `*.csv` glob, `sub/x.csv`, a 12 MiB file, outputDirectory: true -> URIs; read back from the store byte-identical;
// a symlink declared as an output is still refused by name (the W5a message), and never followed
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **Docker** (`collect_output_files`): replace `docker cp c:<path> <staging>` with `docker cp c:<path> -` read by `transfer`'s reader entry by entry; only regular-file entries are taken. This removes the host staging directory and with it the host-side symlink hazard the W5a docs describe (`commands.rs:1082-1160`): update those docs to say what is now true, dated. The W5a rules stay: literal names and non-recursive globs, a missing declared file ignored, `**`/`[...]`/wildcard directory refused by name (`output_problem` `:2629`).
  - **Kubernetes** (`runner.rs:774`): `tar -c` read as a stream through the same reader, replacing `unpack_one` + `judge`. `judge`'s messages about "no internal storage (W9 owns it)" go; the symlink/directory/other refusals keep their wording.
  - `outputDirectory: true`: per `w9a-storage.json` `output_directory` (the variable, the env var, the keying). Collect everything under that directory as if each file were declared.
  - Names: `Naming::KestraOutputName` → `uri::output_file_uri(task_prefix, &format!("{WORKING_DIR}/{rel}"))` (`WORKING_DIR` = `/egret/working-dir`, `commands.rs:128`). The path handed to `IdUtils.from` differs from Kestra's (its working directory is a random temp dir): record in the report that the **scheme** matches Kestra and the hash cannot.
  - `MAX_OUTPUT_FILE_BYTES` and the UTF-8 rule go (§6.2); `max_file_bytes`/`max_task_bytes` replace them. Keep `MAX_VARS_BYTES`.
  - The script handler sets `collect_to` from `task.render.files().task_prefix()`; with no store, a task declaring `outputFiles` fails naming `[storage]`, before the runner.
  - `outcome_for` (`container.rs:77`) carries `files` from `result.output_files`, then runs `check_outputs_collected` over the final outputs.
  - `Outcome.files`: add the field; every `Outcome { … }` literal gets `files: Vec::new()` or the real list — `trigger/flow.rs`, `trigger/mod.rs`, `worker/claim.rs`, `handlers/container.rs`, `handlers/kestra.rs`, `handlers/sql.rs`, `worker/mod.rs` (the four constructors), `worker/timeout.rs`, `tests/{executor.rs,oracle.rs,worker.rs}` (from `git grep -n "Outcome {"`; 32 sites at `72d35839`).
  - `apply_outcome`: after the fenced `UPDATE … WHERE id = $2 AND claimed_at = $3` hits (`claim.rs:1529-1558`) and before `settle`, `storage::index_task_files(conn, ctx.workspace_id, claimed.id, &outcome.files)`. After, not before: a lost lease returns before indexing.
- [ ] **Step 4: Run the tests to see them pass**, both live suites, and the crate.
- [ ] **Step 5: Mutations:** (a) indexing before the fence check; (b) `check_outputs_collected` removed; (c) the task cap not accumulated across files; (d) the Docker reader taking a symlink entry's target bytes; (e) `outputDirectory` still in `COMMON_REFUSED` (the publish test dies).
- [ ] **Step 6: Commit** `feat(workflow)!: W9a Task 6 — outputFiles are kestra:// URIs, collected by stream and indexed at commit` with a `BREAKING CHANGE:` footer naming §6.2.

---

## Task 7: FILE inputs

**Files:**
- Create `src/storage/inputs.rs`.
- Modify `src/models/input.rs` (`Input` gains `allowed_file_extensions`), `src/routes.rs` (`start_flow_execution`, `StartExecutionRequest`), `src/worker/claim.rs` (`start_execution_in`), `src/worker/subflow.rs` (the INSERT at `:506`), `src/worker/spawn.rs` (`:309`), `src/worker/replay.rs` (`:235`).
- Modify `Cargo.toml`: `axum = { workspace = true, features = ["multipart"] }` (adds `multer`, a new crate — say so in the report), `tokio-util = { workspace = true, features = ["io"] }` (the `egret-provisioner` precedent, `crates/egret-provisioner/Cargo.toml:25`).
- Regenerate `openapi.json` (`cargo run -p egret-bin -- openapi > openapi.json`) and `frontend/` with `npm run gen:api`.

**Interfaces:**
- Consumes: Task 3 `index_input_files`, `input_uri`, `FileStore::execution_prefix`, `resolve_readable_many`, `readable_executions`; Task 2 `put_stream`, `delete_prefix`.
- Produces:

```rust
// claim.rs NewExecution gains (every literal site updated again, as in Task 3):
pub id: Option<Uuid>,                             // None = the DB's gen_random_uuid(), as today
pub input_files: Vec<crate::storage::StoredFile>, // indexed by start_execution_in, in its transaction

// routes.rs — the multipart body's schema, for OpenAPI only (the handler reads parts, not this type)
#[derive(utoipa::ToSchema)]
pub struct StartExecutionMultipart {
    /// One part per input id; a part with a file name is that FILE input's file.
    #[schema(value_type = Object)]
    pub parts: std::collections::BTreeMap<String, String>,
}

// models/input.rs — Input gains
#[serde(default, skip_serializing_if = "Option::is_none")]
pub allowed_file_extensions: Option<Vec<String>>,   // FileInput.allowedFileExtensions; lower-cased, with the dot

// storage/inputs.rs
/// Stream one uploaded part to `inputs/{input}/{file}` of `execution`; the extension and the
/// per-file cap are checked while streaming.
pub async fn upload_input(fs: &FileStore, workspace: Uuid, namespace: &str, flow_id: &str,
                          execution: Uuid, input: &crate::models::Input, file_name: &str,
                          body: &mut (dyn tokio::io::AsyncRead + Send + Unpin)) -> Result<StoredFile, String>;
/// Every FILE input's JSON value must be a kestra:// URI readable from `execution`
/// (resolve_readable_many). Called at every execution-creating INSERT, after the row exists.
pub async fn check_file_inputs(conn: &mut PgConnection, workspace: Uuid, execution: Uuid,
                               flow: &crate::models::Flow, inputs: &serde_json::Value,
                               uploaded: &[StoredFile]) -> egret_core::Result<Result<(), String>>;
pub fn extension_allowed(input: &crate::models::Input, file_name: &str) -> Result<(), String>;
```

- [ ] **Step 1: Write the failing tests** (`tests/executions_routes.rs` pattern, Postgres, `install_mem_for_tests`):

```rust
#[tokio::test(flavor = "multi_thread")] async fn a_multipart_start_stores_the_file_and_the_input_is_its_uri() {
    // tail == w9a-storage.json file_input's; the index row exists; a text part is the text input's value
}
#[tokio::test(flavor = "multi_thread")] async fn the_execution_row_and_its_input_rows_commit_together() {
    // a flow whose first settle fails (e.g. unplannable after upload is impossible -> use a deferred trigger
    // as tests/scripts_gate.rs does) leaves neither row, and the uploaded object is deleted
}
#[tokio::test] async fn a_disallowed_extension_is_refused_with_the_artifacts_message() {}
#[tokio::test] async fn a_file_over_max_file_bytes_is_refused_naming_the_cap() {}
#[tokio::test] async fn a_json_file_input_that_is_not_a_readable_uri_is_refused_naming_the_input() {
    // a plain string; an unrelated execution's real URI (the probe: the row exists)
}
#[tokio::test(flavor = "multi_thread")] async fn a_subflow_child_receives_its_parents_file_uri_and_reads_it() {}
#[tokio::test(flavor = "multi_thread")] async fn a_flow_trigger_passes_its_sources_file_uri_and_it_is_readable() {}
#[tokio::test(flavor = "multi_thread")] async fn a_subflow_passing_an_unreadable_uri_fails_the_parents_run_not_the_worker() {}
#[tokio::test] async fn a_json_start_with_no_file_input_is_unchanged() { /* the two-sided guard start_inputs has today */ }
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **The route** (`start_flow_execution`, `routes.rs:1729`): accept `multipart/form-data` and JSON on the same path. Branch on `Content-Type` with an `axum::extract::Request` and the two extractors (`Multipart::from_request`, `Option<Json<StartExecutionRequest>>`), keeping `start_inputs`' refusals for JSON. Multipart: each part's name is an input id; a part with a file name is that FILE input's value; any other part is a text value; an unknown id is refused naming it.
  - **Order:** check the gate (`startable_flow`), generate `Uuid::new_v4()`, stream each file part through `upload_input` (`StreamReader` over the field), then one transaction: `start_execution_in(NewExecution { id: Some(id), input_files, … })` inserts the row, `check_file_inputs`, `index_input_files`, then `settle`. On any failure after an upload, best-effort `delete_prefix` of the execution's prefix (a start that never committed has no execution for the pruner to find).
  - `DefaultBodyLimit::max(max_file_bytes + 1 MiB)` on this route only (`webhook_routes.rs:474` is the precedent). Multipart webhook bodies stay refused (`trigger/webhook.rs:631`).
  - `check_file_inputs` is called at **all four** creating INSERTs: `claim.rs:1963` (`start_execution_in`), `subflow.rs:506`, `spawn.rs:309`, `replay.rs:235`. For a Subflow, map the refusal the way `subflow::spawn_one` already maps *"a target that does not resolve"* (it completes the parent's run FAILED); read that code first and follow it. Loop iterations and replays copy inputs that were already checked; the call still runs and must pass (the readable set covers them), and a test says so.
  - OpenAPI: `request_body(content((StartExecutionRequest = "application/json"), (StartExecutionMultipart = "multipart/form-data")))` — verify the utoipa 5 syntax against its docs (Context7), and describe the part-naming rule.
- [ ] **Step 4: Run the tests to see them pass**; `cargo test -p egret-server committed_openapi_json_is_current`; frontend `npm run gen:api && npm run lint && npm test` in `frontend/`.
- [ ] **Step 5: Mutations:** (a) `check_file_inputs` removed from `subflow.rs` only (the subflow test must die); (b) the extension check skipped; (c) the index insert moved outside the transaction; (d) the JSON branch accepting any string for a FILE input.
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 7 — FILE inputs, uploaded as multipart or passed as a readable URI`.

---

## Task 8: The execution file API

**Files:**
- Create `src/storage/routes.rs`, `src/storage/ion.rs`, `tests/storage_files_api.rs`.
- Modify `src/routes.rs` (`management_router` `:1968`, the `#[derive(utoipa::OpenApi)] paths(...)` at `:2017`), `Cargo.toml` (`ion-rs = "=1.0.1"`, default features; reader only — the `Cargo.toml:173` comment about the writer stays true and gains one line saying the reader is now used, and why).
- Regenerate `openapi.json` and the frontend client (`npm run gen:api`).

**Interfaces:**

```rust
// storage/ion.rs — ion-rs 1.0.1 `Element::read_one` (src/element/mod.rs:913), one value per line
pub fn line_to_json(line: &str) -> Result<serde_json::Value, String>;   // the value shapes = w9a-storage.json file_api.preview

// storage/routes.rs
#[derive(Deserialize, utoipa::IntoParams)] pub struct FileQuery { pub path: String }
#[derive(Deserialize, utoipa::IntoParams)] pub struct PreviewQuery { pub path: String, #[serde(rename = "maxRows")] pub max_rows: Option<u32> }
#[derive(Serialize, utoipa::ToSchema)] pub struct FileMetasDto { pub size: u64 }
#[derive(Serialize, utoipa::ToSchema)] pub struct FilePreviewDto { pub previewable: bool, pub rows: Vec<serde_json::Value> }
// GET /api/ws/{workspace}/workflows/executions/{id}/file          -> application/octet-stream, streamed
// GET /api/ws/{workspace}/workflows/executions/{id}/file/metas    -> FileMetasDto
// GET /api/ws/{workspace}/workflows/executions/{id}/file/preview  -> FilePreviewDto
```

- [ ] **Step 1: Write the failing tests:**

```rust
#[tokio::test] async fn workflow_read_downloads_and_the_headers_are_safe() {
    // 200; Content-Type application/octet-stream; Content-Disposition attachment; filename sanitised
    // (a name with `"`, `;`, CR/LF, `/` yields none of them); X-Content-Type-Options: nosniff
}
#[tokio::test] async fn another_executions_file_is_404_with_no_redirect() { /* same ws, a real row of another execution */ }
#[tokio::test] async fn another_workspaces_execution_is_404() {}
#[tokio::test] async fn without_workflow_read_it_is_403() {}
#[tokio::test] async fn a_loop_iteration_id_serves_its_roots_files() {}   // "(its loop root)", §8
#[tokio::test] async fn metas_is_the_index_size() {}
#[tokio::test] async fn ion_preview_rows_match_the_artifact_and_max_rows_caps_at_5000() {}
#[tokio::test] async fn csv_jsonl_and_text_preview_as_lines_and_binary_is_not_previewable() {}
#[tokio::test] async fn preview_stops_reading_once_full() { /* a MemStore reader counting bytes read < file size */ }
#[tokio::test(flavor = "multi_thread")] async fn a_download_larger_than_memory_budget_streams() { /* MinIO-gated, 64 MiB */ }
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - Authorisation exactly as `GET …/executions/{id}/logs` (`routes.rs:1398-1422`): `WsContext`, `egret_core::authz::require(&*st.pdp, &ctx, WF_READ)`, `st.executions.get(ws, id)` for existence (a foreign id is 404).
  - The file: `resolve_readable_many(&mut conn, ws, &[loop_root(&mut conn, ws, id).await?], &[path])` — the **one** function, with the readable set narrowed to that execution's root; anything but `Ok` is 404 (§8: no redirect).
  - Download: `Body::from_stream(tokio_util::io::ReaderStream::new(get_stream(..)))`.
  - Preview: read line by line up to `maxRows` and at most `max_read_bytes`; `.ion` through `ion::line_to_json`; `.csv`/`.jsonl`/`.json`/text as strings; else `{previewable: false, rows: []}`.
- [ ] **Step 4: Run the tests to see them pass**; the OpenAPI test; `npm run gen:api && npm run lint && npm test`.
- [ ] **Step 5: Mutations:** (a) the readable set widened to `readable_executions(id)` (the other-execution test must die only if the file is the trigger's — add that case); (b) `maxRows` unclamped; (c) `Content-Disposition` unsanitised.
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 8 — download, metas and preview of an execution's files`.

---

## Task 9: The plugin storage bridge and the catalog

**Files:**
- Create `plugin-host/src/main/java/io/kestra/core/runners/EgretStorage.java`, `plugin-host/src/test/java/io/kestra/core/runners/EgretStorageTest.java`.
- Modify `ShimRunContext.java` (`storage()` `:205`, `inputAndOutput()` `:220`), `EgretPluginHost.java` (`NEEDS_STORAGE` `:99-104`, `task.json` reading `:250-270`), regenerate `crates/egret-workflow/src/plugins/catalog.json` with `describe`.
- Modify `src/plugins/mod.rs` (`REFUSED_VALUES` `:267-338`, `needs_storage` `:357`, `CODE_PROPERTY_NEEDS_STORAGE` `:171`), `src/worker/handlers/kestra.rs` (`KestraRuntime::commands` `:165-189`), `tests/plugins_gate.rs`, `tests/plugin_host_live.rs`.

**Interfaces:**

```text
task.json = {"type": FQCN, "properties": {…rendered…}, "mask": [...],
             "storage": {"<kestra:// uri>": ".egret/storage/<n>", …},
             "baseUri": "kestra:///…/tasks/{task}/{taskRunId}/",
             "maxTaskBytes": <u64>,
             "context": {"namespace": …, "flowId": …, "executionId": <loop root>, "taskId": …, "taskRunId": …}}
host writes    .egret/out/<path relative to baseUri>
host outputs   the plugin's own outputs, URIs = baseUri + relative path
```

```java
// EgretStorage.java — io.kestra.core.storages.StorageInterface (StorageInterface.java:57-305)
final class EgretStorage implements StorageInterface {
    EgretStorage(Path workingDir, Map<String, String> staged, URI baseUri, long maxTaskBytes);
    // get/getWithMetadata/exists/getAttributes: staged or written; else FileNotFoundException naming the URI
    // put: only under baseUri -> .egret/out/<relative>; counts bytes; over maxTaskBytes -> IOException naming the cap
    // list/allByPrefix: staged + written, filtered by prefix
    // delete/deleteByPrefix/move/createDirectory/purgeByLastModified: this task's written files only;
    //   anything else -> UnsupportedOperationException with the reason
    // *InstanceResource / getInstanceAttributes / createInstanceDirectory: UnsupportedOperationException
}
// ShimRunContext.storage(): new InternalStorage(StorageContext.forTask(null, ns, flowId, execId, taskId, taskRunId, null),
//                                               egretStorage, <NamespaceFactory, see Step 3>)
```

- [ ] **Step 1: Write the failing tests.**
  - Java (`EgretStorageTest`, JUnit 5, as `EgretPluginHostTest`): `get` of a staged URI returns its bytes; of anything else `FileNotFoundException` whose message holds the URI; `put` outside `baseUri` refused; `put` past `maxTaskBytes` refused naming the cap; `delete` of a staged file refused; `StorageContext.forTask(…).getContextStorageURI()` **equals** the `baseUri` egret computes for the same five values (a fixture pair written by a Rust test into `plugin-host/src/test/resources/base-uri-cases.json` and read here).
  - Rust (`kestra.rs` spy tests): the staged set is exactly the whole-string `kestra://` leaves of the rendered properties; `task.json` holds `storage`, `baseUri`, `maxTaskBytes`, `context`; an unreadable URI fails at render and calls no runner; the runner is given `collect_to` with `Naming::UnderPrefixVerbatim` over `.egret/out`.
  - `tests/plugins_gate.rs`: the W5b assertion that `REFUSED_VALUES` equals `w5b-plugins.json`'s `refused_values` becomes: it equals `w9a-storage.json`'s `plugin_storage.still_refused_values`. **Every** W5b test that asserted "refused, needs storage" flips — find them with `git grep -n "CODE_PROPERTY_NEEDS_STORAGE\|needs_storage\|fetchType.*STORE\|W9" -- crates/egret-workflow/tests crates/egret-workflow/src/plugins crates/egret-workflow/src/worker/handlers/kestra.rs` — and each flip has a replacement test that the plugin **runs**.
  - Live (`tests/plugin_host_live.rs`, `EGRET_PLUGIN_HOST_LIVE=1`, Docker, MinIO-gated): `postgresql.Query` `fetchType: STORE` → `postgresql.CopyIn` from that URI → a `Log` with `read()` of a `CopyOut` URI; `mysql.Batch` into StarRocks from a `Write`'s ION (skipped with a printed reason without `EGRET_TEST_STARROCKS_URL`, and the report counts skips).
- [ ] **Step 2: Run them to watch them fail** (`mvn -f plugin-host/pom.xml test` with `JAVA_HOME` set; `cargo test -p egret-workflow --test plugins_gate`).
- [ ] **Step 3: Implement.**
  - **Read `InternalStorage.java:45-60` in the Kestra checkout before choosing the third constructor argument.** If the constructor dereferences its `NamespaceFactory`, or `putFile`/`getFile` reach it, **stop and report**: the shim would need a namespace stand-in, which is W9b's territory.
  - `inputAndOutput()` stays unsupported unless Task 1's `plugin_storage` runs show a re-admitted class calling it; then implement only what those classes call, and say so.
  - `NEEDS_STORAGE` empties (or keeps what `still_refused_values` keeps); regenerate `catalog.json` with `describe`; `plugins.lock` is unchanged, so `the_committed_catalog_was_generated_from_the_committed_lockfile` still holds.
  - If `still_refused_values` is `{}`, delete `REFUSED_VALUES`, `NeedsStorage`, `Literal`, `needs_storage`, `refused_values_json` and `CODE_PROPERTY_NEEDS_STORAGE` (dead code), and the fixture test asserts the fixture's set is empty.
  - The handler: after `render_with_secrets`, walk the rendered properties; every **whole string leaf** that starts with `kestra:///` is resolved through `task.render.files()`'s snapshot (Task 4) and staged as `stored_inputs[".egret/storage/<n>"]`. Whole leaves, not substrings: a URI quoted inside SQL text is not a file read (a spec ambiguity, reported).
  - Files a plugin writes are **not** masked, as outputs are not (§9.1, W5b §12); the live test records one file holding the secret's value and says so rather than asserting it away.
  - `EgretUnsupported` messages that say "(W9)" for storage (`ShimRunContext.java:205-221`) go; `namespaceKv` names W9c.
- [ ] **Step 4: Run the tests to see them pass**; `mvn -f plugin-host/pom.xml verify`; rebuild `egret-plugin-host:dev` (`docker build -f deploy/docker/plugin-host.Dockerfile -t egret-plugin-host:dev plugin-host`) and run the live suite.
- [ ] **Step 5: Mutations:** (a) `put` accepted outside `baseUri` (Java test must die, by running `mvn test`); (b) the handler staging substrings too (a spy test with a URI inside `sql` must die); (c) `check_outputs_collected` bypassed for plugins (a host fake returning a URI it did not write must fail the task).
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 9 — Kestra plugins read and write storage through egret`.

---

## Task 10: `egret.sql` with `fetch: STORE`

**Files:** `src/worker/handlers/sql.rs` (`Fetch` `:143-156`, `outputs_for` `:523`, `run` `:290-360`), `src/runners/ionfmt.rs` (a JSON-row writer beside `write_ion` `:53`), `tests/sql_starrocks_live.rs`.

**Interfaces:**

```rust
// sql.rs
pub enum Fetch { None, One, All, #[serde(rename = "STORE")] Store }
// run(): Fetch::Store takes the async arm: rows -> ionfmt, one row per line, through a tokio::io::duplex
// into put_stream at `{task_prefix}{name}.ion`; outputs {"uri": …, "size": <row count>} (w9a-storage.json store_outputs)

// ionfmt.rs
pub(crate) fn write_ion_row(out: &mut String, columns: &[String], row: &[serde_json::Value]);
```

**Every predicate over `Fetch`** (a new variant): `outputs_for`'s `match fetch` (`sql.rs:539`) and its early `Fetch::None` return; the `Deserialize` spellings; the module doc (`:140-145`, *"`STORE` is deliberately absent … W9 owns"*). `git grep -n "Fetch::" -- crates/egret-workflow` lists them; the report names each.

- [ ] **Step 1: Write the failing tests:** `store_writes_every_row_as_ion_and_outputs_uri_and_row_count`; `store_ion_matches_the_artifact_for_every_type_egret_can_carry` (`ion_rows`; a type egret's `TabularResult` cannot carry — `bytea`, `timestamptz` as text — is recorded as a divergence, not faked); `store_over_the_row_cap_fails_like_fetch_does` (`result.truncated`, the same message shape as `sql.rs:552-557`); `store_without_a_store_fails_naming_storage_before_the_statement_runs` (spy port: not called); the file is indexed through `Outcome.files`; live StarRocks: 50 000 rows STORE then `read()` of the first line.
- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.** The port returns a whole `TabularResult` (`egret-query-api/src/port.rs`), so STORE is bounded by `Sql::max_rows` (`MAX_FETCH_ROWS`, 10 000) exactly as FETCH is — "under its existing row limit" (§9.2). Task 1's `connector_streaming` goes into this handler's doc and into §15's known limit. The file name is `w9a-storage.json`'s `store_outputs` pattern if deterministic, else the fixed `result.ion` (§4.3 needs a re-claim to overwrite its own object), stated in the doc.
- [ ] **Step 4: Run the tests to see them pass**, the live suite with `EGRET_TEST_STARROCKS_URL`.
- [ ] **Step 5: Mutations:** (a) STORE truncating instead of failing at the cap; (b) `size` = bytes instead of rows; (c) the file not added to `Outcome.files` (the index assertion must die).
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 10 — egret.sql stores its rows as ION`.

---

## Task 11: `Write`, `Size`, `Delete`, `PurgeCurrentExecutionFiles`, and `Loop` over a file

**Files:**
- Create `src/worker/handlers/storage.rs`, `tests/storage_tasks.rs`.
- Modify `src/worker/handlers/mod.rs`, `src/worker/registry.rs` (`with_core_tasks` `:123`), `src/spec/registry.rs` (`TASK_TYPES` `:77`), `oracle-exec/driver.rs` (`registry()` `:131`), `src/worker/render.rs` (`render_expressions` `:1031`), `src/executor/flowable/loop_.rs` (the named-gap doc `:167-205`).

**Interfaces:**

```rust
// handlers/storage.rs — one TaskHandler per FQCN, each type_id the io.kestra.plugin.core.storage.* FQCN
pub struct Write; pub struct Size; pub struct Delete; pub struct PurgeCurrentExecutionFiles;
// TaskHandler::cancel: Ok(()) with the reason "holds nothing past run" (each states it; no default body exists)
```

**Every list that enumerates task types and must gain these four (and Task 12's five):** `spec::registry::TASK_TYPES` (`add_publishable` requires the row); `Handlers::with_core_tasks` (`registry.rs:123`); `oracle-exec/driver.rs::registry()` (`:131`; with a `MemStore`-backed `install_mem_for_tests`, so the ratchet can run them); `oracle-exec/handler-surface.txt` if `handler_surface.py` lists implemented types (run it and commit the diff); `executor/mod.rs`'s `PUBLISHED_BUT_REFUSED` (`:3804`) if a type appears there. `plugin-host`'s `NATIVE_TYPES` (`EgretPluginHost.java:84-93`) excludes natively ported types from the catalog; `describe` scans only the three JDBC jars, so `core.storage` classes never reach it — **verify with `describe` and state it**, rather than adding them.

- [ ] **Step 1: Write the failing tests:** per class, the properties and outputs equal `w9a-storage.json`'s `core_storage` and one run reproduces the recorded outputs and bytes; `Delete` of a file of another (readable) execution — a trigger's — is refused (§9.3: only the loop root's), **index row first** (a store failure after the row delete leaves an orphan, never a dangling row); `PurgeCurrentExecutionFiles` deletes only the loop root's files; `Loop` with `values: "{{ outputs.write.uri }}"` iterates the file's records as `loop_over_ion` records, a `null` record fails the run with Kestra's message, a file over `max_read_bytes` fails naming the cap.
- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - Inputs through `task.render.files()` (the snapshot); outputs written with `put_stream` under `task_prefix`, returned in `Outcome.files`.
  - **`Loop` over a file** (§9.3): in `render_expressions`, for a `TaskKind::Loop` whose rendered value is a `kestra:///` URI, resolve it through the `Surface`'s snapshot, read it (`read_utf8`'s path, capped by `max_read_bytes`), parse each line with `storage::ion::line_to_json`, coalesce each record as `FlowableUtils.ionValueToString` does (a string as itself, a number's `toString`, `null` an error, anything else JSON), and put the **JSON array** in `rendered`. `advance()` is untouched and still pure. `loop_.rs`'s *"A NAMED GAP"* doc is rewritten to say where the file is read, dated.
  - **The cap.** §9.3 says *"capped by the existing loop-width limit"*; there is no such limit in the code (`git grep` of `executor/flowable/loop_.rs` and `worker/spawn.rs` finds none). This task caps by `max_read_bytes` on the file, the same cap as `read()`, and reports it as a spec correction.
- [ ] **Step 4: Run the tests to see them pass**, then the ratchet: `cargo test -p egret-workflow --test executor executor_matches_golden -- --nocapture`. Delete from `baseline.txt` exactly the entries that now match (the ratchet fails otherwise), and record the number.
- [ ] **Step 5: Mutations:** (a) `Delete` allowing any readable execution; (b) the store delete before the index delete; (c) the `null` record coalesced to `"null"`; (d) the file read without the cap.
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 11 — core storage tasks, and Loop over an ION file`.

---

## Task 12: `Concat`, `Split`, `Reverse`, `DeduplicateItems`, `FilterItems`

**Files:** `src/worker/handlers/storage.rs`, the same lists as Task 11, `tests/storage_tasks.rs`.

- [ ] **Step 1: Write the failing tests:** per class, `core_storage`'s properties, outputs and one run reproduced byte for byte; `Split` by `bytes`, `rows` and `partitions` never holds the input whole (a counting reader: peak buffered < 1 MiB on a 20 MiB input); `DeduplicateItems.expr` and `FilterItems.filterCondition` render **per record** with the record bound as Kestra binds it (read `DeduplicateItems.java:87-190`, `FilterItems.java:81-210` for the binding name), `errorOrNullBehavior` each value; an expression that fails on one record follows `errorOrNullBehavior`, never a worker error.
- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement** as Task 11. `Concat.files` is `Object` in Kestra (a list or a JSON string of a list, `Concat.java:161`); follow the fixture for both.
- [ ] **Step 4: Run the tests to see them pass**, and the ratchet again.
- [ ] **Step 5: Mutations:** (a) `Split` collecting before writing; (b) `FilterItems` inverting `INCLUDE`/`EXCLUDE`; (c) `DeduplicateItems` keeping the first instead of the last (whichever the fixture shows is Kestra's).
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 12 — Concat, Split, Reverse, DeduplicateItems and FilterItems`.

---

## Task 13: The pruner

**Files:**
- Create `migrations/0162_workflow_execution_files_pruned.sql`, `src/storage/prune.rs`, `tests/storage_prune.rs`.
- Modify `src/storage/mod.rs` (`Unreadable::Pruned`), `src/storage/index.rs`, `src/runtime.rs` (`spawn_with_logs` `:344`, `RuntimeConfig` `:247`), `crates/egret-bin/src/main.rs` (`:1926` area).

**Interfaces:**

```sql
-- migrations/0162_workflow_execution_files_pruned.sql (exact DDL)
ALTER TABLE workflow.executions
  ADD COLUMN files_pruned_at timestamptz,   -- set when the pruner deleted this root's files
  ADD COLUMN files_swept_at  timestamptz;   -- set when the orphan sweep of a FAILED/KILLED root ran
```

```rust
// storage/mod.rs — a new Unreadable variant
#[error("{uri} was deleted: files are kept {retention_days} days after their execution ends")]
Pruned { uri: String, retention_days: u32 },

// storage/prune.rs — modelled on logs/prune.rs (PruneConfig::new, spawn_pruner, pass)
pub struct StoragePruneConfig { pub retention_days: u32, pub every: Duration /*1 h*/, pub batch: i64, pub pass_budget: Duration, pub orphan_age: Duration /*24 h*/ }
pub fn spawn_storage_pruner(pool: PgPool, fs: Arc<FileStore>, cfg: StoragePruneConfig, cancel: CancellationToken) -> Option<JoinHandle<()>>;
pub async fn prune_pass(pool: &PgPool, fs: &FileStore, cfg: &StoragePruneConfig) -> egret_core::Result<PruneReport>;
#[derive(Debug, Default, PartialEq, Eq)] pub struct PruneReport { pub executions: u64, pub orphans: u64, pub failed: u64 }

// RuntimeConfig gains `storage_retention_days: u32`
```

**Every predicate over `Unreadable`** (a new variant): `Display`; `resolve_readable_many`'s classification (a URI whose `{exec}` is readable, has no row, and `files_pruned_at IS NOT NULL` → `Pruned`); the four functions (`fileExists` on `Pruned` follows the error rows, not `Missing`'s); the file API (404); staging and the bridge (fail the task with the message). The report lists each match site from `git grep -n "Unreadable::"`.

- [ ] **Step 1: Write the failing tests** (Postgres + `MemStore`, a store fake that fails `delete_prefix` on demand):

```rust
#[tokio::test] async fn an_execution_that_has_not_ended_is_never_pruned() {
    // RUNNING, updated_at forced 400 days back, with rows: kept
}
#[tokio::test] async fn an_ended_root_older_than_the_window_loses_objects_then_rows() {}
#[tokio::test] async fn a_failed_store_delete_keeps_the_rows_and_the_next_pass_retries() {}
#[tokio::test] async fn orphans_older_than_a_day_are_deleted_and_younger_ones_and_indexed_ones_are_kept() {}
#[tokio::test] async fn zero_retention_starts_no_pruner() {}
#[tokio::test] async fn a_pruned_uri_is_an_error_naming_it_and_the_window() {}
#[tokio::test] async fn a_pass_stops_at_its_budget() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.** "Ended" = state terminal (`RunState::ALL` filtered by `is_terminal`, the idiom of `ExecutionRepo::log_status`, `repo.rs:1085-1090`) and `updated_at < now() - make_interval(days => $n)`, over **roots with index rows**. Per root: `delete_prefix(execution_prefix)`, then `DELETE` its rows and set `files_pruned_at` in one statement; a failed `delete_prefix` leaves both. Orphans: for each root pruned in this pass and each FAILED/KILLED root ended more than 24 h ago with `files_swept_at IS NULL`, `list` its prefix, delete objects with no row whose `last_modified` is older than 24 h, set `files_swept_at`. Started from `spawn_with_logs` beside `spawn_pruner` (`runtime.rs:356-360`) when `storage::current()` is `Some` and retention > 0.
- [ ] **Step 4: Run the tests to see them pass.**
- [ ] **Step 5: Mutations:** (a) the terminal predicate dropped; (b) rows deleted before the store; (c) the orphan age check removed; (d) `Pruned` reported as `Missing` (`fileExists` would then answer `false` silently).
- [ ] **Step 6: Commit** `feat(workflow): W9a Task 13 — execution files are pruned 30 days after the execution ends`.

---

## Task 14: End to end on a temporary kind cluster, coverage, docs, full CI

- [ ] **Step 1: Set up the cluster.**
  - `kind create cluster --name egret-w9a-e2e`; build and `kind load` `egret-plugin-host:dev` and `egret-task-helper:dev` and the backend image.
  - Install the chart with `workflow.containerRunner=kubernetes`, `workflow.workerEnabled=true`, `workflow.pluginHost.image=egret-plugin-host:dev`, and a Postgres in a namespace the task NetworkPolicy lets through (W5b's `e2e-data`).
  - **MinIO:** the chart's `deps.minio.image` is a quay.io pin that answered 401 in W5b's run. If it does again, `kind load` a locally present MinIO image and override `deps.minio.image`/`mcImage`; report it.
  - **Do not touch the shared `egret` cluster.**
- [ ] **Step 2: Drive the scenario through the API**, saving transcripts to the SDD workspace:
  1. A multipart start with a FILE input → a script task `inputFiles: {in.csv: "{{ inputs.f }}"}` → `outputFiles: ["*.csv"]` → a `Log` with `{{ read(outputs.s.outputFiles['out.csv']) }}` → the file API downloads the same bytes.
  2. `kestra.…postgresql.Query` `fetchType: STORE` → `postgresql.CopyIn` of that URI → row count checked in Postgres.
  3. `core.storage.Write` → `Loop` over its URI (three iterations) → `Size`.
  4. **The pod holds no storage credential:** `kubectl get pod -o yaml` **during** a run (hold it with `sleep 20`) contains none of `[storage]`'s access key, secret key or endpoint; controls: the image name appears.
  5. A second workspace's execution cannot read the first's URI (render error naming it; file API 404).
  6. Retention: set `storage_retention_days=1`, force one ended root's `updated_at` back two days in Postgres, wait for one pass (or restart to trigger), and see its objects gone and the URI answering the pruned message.
- [ ] **Step 3:** `kind delete cluster --name egret-w9a-e2e`.
- [ ] **Step 4: Coverage and docs.**
  - `oracle-exec/README.md` gains a W9a section, as W5b's (`README.md:9112-9245`): the ratchet **measured** before (Task 1's `baseline-ratchet.txt`) and after, per task in a table; which of the corpus's 5 FILE-input, 1 `outputFiles`, 2 STORE (both `Loop` STORE and webhook STORE, which W9a does not implement) and 2 `core.storage` flows moved and why; every live test with its gate; what was measured live; what was not.
  - Programme spec §9 EN+VI: the W9 row split into W9a/W9b/W9c (user decision, 2026-10-01) and a dated W9a correction paragraph like W5b's (`programme-design.md:495`); §11 question 1: object storage for **log lines** is still not built (§14 of the W9a spec), dated; §3 finding 4 (`StorageInterface`) says the bridge exists.
  - **The breaking change's release note** (§6.2): the W9a section of `oracle-exec/README.md` and the programme spec's W9a correction both state that `outputs.<task>.outputFiles` changed from contents to URIs, behind `workflow_containers` (off by default), and how to read a file (`read(...)`).
  - The W9a spec EN+VI: the corrections this plan's report lists (the render-time lookup, the loop cap, `ForEach` = `io.kestra.plugin.core.flow.Loop`, anything Task 1 contradicted), each dated.
  - **Sweep** (N examined / M corrected per population, `/usr/bin/grep` with file arguments): (1) `\bW9\b` in `crates/egret-workflow/src`, `plugin-host/src` and both programme specs — a message that says storage "needs W9" is now false; KV says W9c, namespace files W9b; (2) *"1 MiB"*/*"contents"* about `outputFiles` in docs and doc comments; (3) *"no internal storage"*.
- [ ] **Step 5: Full local CI**, compared with Task 1's baselines:
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --all-targets -- -D warnings`: the `error:` locations compared with `baseline-clippy.txt`; **no new location** passes, and the known-red members are listed as still red;
  - `cargo test --workspace` on `:5438` with the Global Constraints' env, plus the MinIO-gated suites with the four `EGRET_TEST_S3_*` vars; `just test-shipping-resolution` (the `-p egret-bin` resolution, `.github/workflows/ci.yml`);
  - `cargo test -p egret-server committed_openapi_json_is_current`; in `frontend/`: `npm run gen:api` (never bare `orval`; memory `orval-regeneration-strips-url-encoding`), `git diff --exit-code src/api/generated`, `npm run lint`, `npm test`;
  - `mvn -f plugin-host/pom.xml verify` with `JAVA_HOME`;
  - `helm lint deploy/helm/egret`.
  Every failure absent from `baseline-gate.txt` is this branch's until measured otherwise (memory `pre-existing-is-a-hypothesis`).
- [ ] **Step 6: Commit** `docs(workflow): W9a Task 14 — end to end on kind, coverage and the programme spec`. **Stop:** no PR until the user asks.
