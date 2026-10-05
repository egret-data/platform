# W9b — Namespace files, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A flow reads and writes Kestra's namespace files in egret: revisioned objects under `{prefix}{ws}/{ns-as-path}/_files/{path}[.vN]` indexed in `workflow.namespace_files`, reached through `nsfile://`, bare paths and `kestra:///…/_files/…` URIs; the expression functions (`fileURI` and the four W9a file functions); `namespaceFiles` staged into script tasks; the four `io.kestra.plugin.core.namespace` tasks; a namespace file REST API; and read-only namespace files for Kestra plugins.

**Architecture:**
- `egret-workflow`'s `storage` module gains `nsfiles`: path rules, **one** access function (`nsfiles::access::authorize`, returning a `Grant` that every store operation requires), the metadata store (read side, then the locked write side), globs, staging, the routes and a streamed zip writer.
- `storage::uri::parse_any` recognises the `_files` marker **before** the `executions/{uuid}` search and answers `StorageUri::{Execution, NamespaceFile}`; W9a's `parse` keeps answering execution URIs only.
- Namespace files are **not** in W9a's pre-render snapshot. A render looks them up at render time on a connection the `Snapshot` opens lazily from the connect options `egret-bin` installs; handlers and routes use their pool. Every lookup goes through `authorize` + `nsfiles::store::resolve`.
- Every write takes a per-`(workspace, namespace, path)` Postgres advisory lock (transaction-scoped for writes and soft deletes, session-scoped for hard purges and moves, so that the "rows first, then objects" order holds without letting a writer reuse a purged key).

**Tech Stack:** Rust (tokio, sqlx 0.9, minijinja, axum 0.8 `multipart`, `regex`, `crc32fast` 1.5.0, aws-sdk-s3 via `egret-storage`), Java 25 + Maven (Kestra `io.kestra:core:2.0.0-rc13`), MinIO, Docker, Helm, kind.

**Spec:** `docs/{en,vi}/specs/2026-10-05-workflow-w9b-namespace-files-design.md`. **§n refers to the spec.** The spec's §12 lists ten tasks; this plan has eleven. The mapping and the reason for the one split are in *Task map* below.

**A warning about this document.** Code blocks show shapes to verify, not text to paste. Every existing name below was read at `d7bd1b3c` (base `e539d8b3` + the spec commits); every new name is defined in the task that introduces it and used with the same spelling afterwards. A block marked **sketch** compiles only once the anchor it names has been read and the shape adjusted. Wherever the plan disagrees with the code, the plan is wrong, and the implementer says so in the report. W9a's plan was wrong in five places; see memory `plan-illustrative-code-is-a-guess`.

## Global Constraints

- **User decisions (spec header), verbatim in effect:**
  - **Namespace files keep revisions, as Kestra does.** Revision 1 lives at `path`, revision N > 1 at `path.vN`; a delete is soft; `PurgeFiles` hard-purges.
  - **A running flow reads namespace files of any namespace in its workspace, and writes only to its own namespace.** Writes are `UploadFiles`, `DeleteFiles`, `PurgeFiles` (own namespace and its child namespaces) and a plugin's `putFile`. The workspace is the hard boundary, taken from the running execution and never from a URI.
  - **API permissions:** reading (download, list, search, stats, revisions, export) needs `workflow:read`; writing (upload, create directory, move, delete, purge) needs `workflow:manage`. No new permission.
  - **`WorkingDirectory` stays out of W9b.** `namespaceFiles` runs on script tasks only; on `WorkingDirectory` the block is parsed and validated at publish (a malformed block refused naming the property, a valid one kept in the model). Nothing changes at run. The two corpus flows stay baselined; no ratchet change is expected.
  - **Namespace files load first and `inputFiles` always overwrite a same-named namespace file, whatever `ifExists` says; `ifExists` governs only collisions between namespaces.**
  - **A namespace with a `_files` segment is refused** at publish and in every namespace-file operation (API, tasks, functions), with an error naming it.
  - **Approved as egret's choices:** `PurgeFiles` defaults to the flow's own namespace; child namespaces match on a dot boundary; the API's `DELETE` does not remove parents left empty; export is a streamed, files-only zip; `fileURI` without `revision` is an error on a missing path; reading `kestra:///other/ns/_files/…` is allowed within the workspace.
- **Layout (§3):** object key `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/_files{path}` for revision 1 and `…{path}.v{N}` for N ≥ 2 (`path` always starts with `/`); a directory has a row and **no object**. URI a flow sees: `kestra:///{ns-as-path}/_files{path}[.vN]`, no workspace in it.
- **Defaults:** `workflow.storage.max_namespace_files` **10 000** (new, §7); `max_file_bytes` 1 GiB, `max_task_bytes` 5 GiB, `max_read_bytes` 10 MiB (W9a's, unchanged). Search and listing answers are capped by the same count.
- **One access function, one readability path.** `storage::nsfiles::access::authorize` is the only code that decides whether a namespace may be read, written or purged; every store function takes the `Grant` it returns, so an unauthorised call does not type-check. Functions, staging, the four tasks, the API and the plugin bridge all reach `authorize` and then `nsfiles::store::resolve`/`list_live`. A second predicate anywhere is a review failure. **The workspace is always the execution's (or the API route's `{ws}` after the permission check), never parsed out of a URI or a property.**
- **One path rule function** (`nsfiles::path::NsPath::parse_file`/`parse_dir`) for every entry point, and **one namespace rule** (`nsfiles::access::check_namespace`: `is_valid_namespace` + the `_files` segment) used by publish and by `authorize` — the save-time refusal and its run-time twin are the same function, and each task's report says which test pins which.
- **Every refusal has a test that fails when the check is removed**; the task names the mutant that kills it. A refusal never says whether a file exists in a namespace the caller may not read.
- **Revisions are taken under a lock.** A write holds `pg_advisory_xact_lock` on the path (and on each parent directory path, in ascending path order) across the upload and the row insert; a hard purge or move holds the same keys with `pg_advisory_lock` (session) across "delete rows, commit, delete objects". A concurrency test fails without the lock (Task 3).
- **Bounds are structural:** counts are counted on what egret actually fetched (`LIMIT cap + 1` rows, the `+1` is the refusal), bytes as they stream. Nothing bounds by inspecting a glob or a path's grammar.
- **Errors (§11):** a path-rule violation, a refused namespace, a missing object behind a row, a conflict are the author's (a failed task, a 4xx), naming the path. An unreachable store or database is an **infrastructure** error: a handler returns `Err` (lease lapses, re-claim, no attempt spent); a render sets the scope's `infrastructure_failure` (read at `worker/claim.rs:1258`, `worker/render.rs:567`); a route answers 503. A crossed cap fails the task naming the cap. No message names a secret or an `s3://` key (the store's error goes to the server log only, as W9a's `Delete` does).
- **Destructive tests:** delete, purge and move run only on `MemStore` (`storage::install_mem_for_tests`) or, on MinIO, under a throwaway prefix `w9b-tN-test/<uuid>/` (a `FileStore` with that `prefix`), deleted at the end of the test by `delete_prefix` of that prefix. **Never** a delete on the bucket root or on `workflow/`. Postgres: only the dedicated `egret-w9b-postgres`; never the shared dev database (Task 1's `_files` count is a read-only `SELECT` there, and nothing else touches it).
- **Mutation discipline (as W9a):** every mutation shows evidence it applied (`git diff --stat` plus a token count), a run count, an unmutated control and the assertion that killed it. A mutant that fails to compile proves nothing; a SQL, Java or YAML mutant is proven by running that artefact (memory `mutants-outside-the-compiler`). Never commit a mutant; never mutate while a build or test runs; restore with `git checkout -- <file>` and confirm `git diff --stat` is empty (memory `a-mutation-can-be-invalidated-by-the-restore`).
- **Per-task gate** (every task, before its commit): `cargo test -p egret-workflow --no-fail-fast` **and** `cargo test -p egret-server --lib` (`committed_openapi_json_is_current`, `crates/egret-server/src/lib.rs:779`; W9a's per-task gates missed a stale `openapi.json`), plus `cargo fmt --all --check` and `cargo clippy -p egret-workflow --all-targets` with no new warning location against Task 1's list. Any route or schema change regenerates `openapi.json` (`cargo run -p egret-bin -- openapi > openapi.json`) and the frontend client with `npm run gen:api` in `frontend/` — **never bare `orval`** (memory `orval-regeneration-strips-url-encoding`) — then `git diff --stat frontend/src/api/generated` is reported.
- **sqlx 0.9:** `sqlx::query*` take `&'static str` (memory `egret-sqlx-09-forced-sql-audit`). Dynamic SQL is built with `concat!` from literals (W9a's `readable_sql!` in `storage/index.rs:114` is the pattern); never `format!` into a query. Child paths are matched with `left(path, length($dir)) = $dir`, **never `LIKE`** (memory `sql-like-underscore-dropped-a-database`; `_` is common in file names).
- **Migration 0169** (`migrations/0168_workflow_execution_files_pruned.sql` is the last at HEAD). Its comments are final at its first commit: sqlx hashes comments (`VersionMismatch`, W9a's release note), so the file is not edited after Task 2's commit except by a fix-round commit on this branch, before merge, and the report says so.
- **Render-time reads need the multi-thread runtime** (W9a constraint, unchanged): every test that renders a namespace function against a real database or store is `#[tokio::test(flavor = "multi_thread")]`.
- **The ratchet** is measured before (Task 1) and after (Task 11): at the base it reads **`294 golden entries, 108 mismatching, 108 baselined, 0 broken`** (`oracle-exec/README.md:9258`). No change is expected (§7, §13); a change is a finding to explain, not to bank.
- **A zero is reported beside a number that can disagree.** Sweeps are N examined / M corrected per population, with `/usr/bin/grep` and file arguments (never `rtk grep` piped; memory `rtk-grep-swallows-stdin`).
- **Commits:** Conventional Commits; trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; the worktree's configured identity, never `--author`. Check `git branch --show-current` == `feat/workflow-w9b-namespace-files` before every commit (memory `a-detached-head-passes-every-check`). Never commit `.superpowers/`, jars, `plugin-host/target/`, scratch files or a mutant. **No PR.**
- **Environment:**
  - Worktree `/Users/1pro-l4r8/Documents/github/data-portal/egret-w9b-ns`. Dedicated Postgres `egret-w9b-postgres` (postgres:16) on **:5439**, `egret`/`egret`, database `egret_test`.
  - Export, for every cargo test: `DATABASE_URL=postgres://egret:egret@localhost:5439/egret_test` (export it; memory `sqlx-test-writes-into-the-dev-database`), `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`.
  - MinIO (`egret-dev-minio-1`, `:9000`): set **all four** `EGRET_TEST_S3_ENDPOINT=http://localhost:9000 EGRET_TEST_S3_KEY=egret EGRET_TEST_S3_SECRET=egretsecret EGRET_TEST_S3_BUCKET=warehouse`.
  - `JAVA_HOME=/Users/1pro-l4r8/Library/Java/JavaVirtualMachines/amazon-corretto-25.jdk/Contents/Home`; the plugin host is `plugin-host/`; Kestra's checkout is `/Users/1pro-l4r8/Documents/github/data-portal/kestra` at `96893b589`.
  - `pgrep -lx cargo` first; never two cargo test processes at once (memory `no-concurrent-test-suites-and-tail-buffering`); keep ≥ 10 G free on the disk holding the target dir (memory `egret-target-dir-fills-the-disk`). An `E0432` that looks like a missing export is first retried with a private `CARGO_TARGET_DIR` (memory `shared-cargo-target-across-checkouts`).
  - Commit before any suite that may run over ~120 s (memory `long-commands-are-auto-backgrounded`).
  - Never `docker image prune -a`. The kind cluster `egret` is shared; the end to end uses a **temporary** cluster `egret-w9b-e2e`.
  - Live tests are gated by env vars and report IGNORED (never passed) without them, following `tests/container_docker_live.rs`'s hand-rolled main.

## Task map

| plan task | spec §12 | why |
|---|---|---|
| 1 Measure Kestra, and the baselines | 1 | — |
| 2 URIs, path rules, `authorize`, the table, reads | 2 | the **write** half of §12.2 moves to Task 3: computing a revision is the lock, and the lock is §12.3's; Task 2 inserts rows with test SQL |
| 3 Writes: lock, revisions, conflicts, soft delete, move, purge | 3 (+ §12.2's write) | — |
| 4 The expression functions | 4 | — |
| 5 Globs and `namespaceFiles` staging; `WorkingDirectory` validated | 5 | — |
| 6 `UploadFiles`, `DownloadFiles` | 6 | — |
| 7 `DeleteFiles`, `PurgeFiles` | 7 | — |
| 8 The namespace file API (nine routes) | 8 (first part) | — |
| 9 `GET …/files/export`: the streamed zip | 8 (second part) | **split:** a hand-rolled ZIP writer (stored entries, data descriptors, ZIP64) is a format a reviewer can reject while approving the nine routes; its instrument is external (`unzip -t`, Python `zipfile`), not the route tests |
| 10 The plugin bridge | 9 | — |
| 11 End to end on kind, coverage, docs, full CI | 10 | — |

**Order:** 1 → 11. Each task's *Consumes* names what it needs from earlier ones.

## Where the spec is silent or does not fit the code, and what this plan does

Each is reported to the user with the plan; an implementer who finds the code says otherwise stops and reports.

1. **`Tail::NamespaceFile` cannot join W9a's `Tail`** (§3): `ExecUri` (`uri.rs:44`) carries a `flow_slug` and an `execution: Uuid` that a namespace file has not. The plan adds `StorageUri::{Execution, NamespaceFile}` from one `parse_any`, whose `_files` check runs first, as §3 requires; W9a's `parse` and its 17 call sites keep their shape (Task 2).
2. **`authorize(workspace, running_flow_namespace, …)` "with the user's permission instead of a flow" for the API** (§4.6): the plan's `Actor::{Flow, Api}`; `authorize` returns a `Grant` every store function requires (Task 2).
3. **"The render looks them up directly through the pool"** (§6): `settle` and all eight of its callers hold only a connection. The plan opens one connection lazily per render scope from connect options `egret-bin` installs (Task 4).
4. **A revision read of a soft-deleted path** (§4.5's sentence *"the given revision, which must exist and whose path must not be soft-deleted at its latest revision for a latest-read"*): the plan follows §4.3 — a soft-deleted path is invisible to every read, whatever the revision; only `revisions` and `PurgeFiles` see it.
5. **A `kestra:///…/_files/p` URI with no `.vN` suffix** names revision 1's object (it is that object's key), not "the latest": the plan parses it as revision 1, subject to Task 1's measurement (Task 1 Step 3 stops if Kestra reads the latest).
6. **`UploadFiles.files` globs "matched on the URI's file path"** (§8.1): the plan matches the URI's `Tail` file component with a leading `/` (Task 6).
7. **"Rows first, then objects" for a hard purge** (§4.2, §8.4) races a writer that recomputes the purged revision number and reuses its key once a transaction lock is released; the plan holds a **session** advisory lock across "delete rows, commit, delete objects" (Task 3).
8. **§4.1's index on `(workspace_id, namespace, path)`** is the leading prefix of the unique constraint's index; the plan adds no duplicate (Task 2).
9. **An `inputFiles` value that is a namespace file URI** (`{{ fileURI('x') }}`) is in no section. The plan leaves W9a's behaviour: refused by name (now naming `namespaceFiles` and the plugin bridge as the ways a namespace file reaches a task) — Task 5 adds that message and its test. Supporting it is a one-function change if the user rules so.
10. **§10's "task.json's `storage` map gains the namespace file entries with namespace, path and revision"**: the existing map is `URI → path` strings read by `EgretPluginHost.storageSpec` (`:343-353`); the plan adds a separate `namespaceFiles` array instead of changing that map's value type (Task 10).
11. **§8.2's `tasks/{taskId}/{taskRunId}/{destination}{path}`** joins `""` and `/a.txt` into `…//a.txt`; the plan normalises to exactly one `/` (Task 6).
12. **§12 has ten tasks; the plan has eleven** (*Task map*).

## File Structure

```text
migrations/0169_workflow_namespace_files.sql                  NEW  T2: workflow.namespace_files
crates/egret-config/src/lib.rs                                MOD  T2: WorkflowStorageConfig.max_namespace_files (10 000)
crates/egret-bin/src/main.rs                                  MOD  T2: storage::nsfiles::install_db (beside storage::install, :1586)
crates/egret-workflow/Cargo.toml                              MOD  T9: crc32fast = "=1.5.0" (already in Cargo.lock)
crates/egret-workflow/src/storage/mod.rs                      MOD  T2: pub mod nsfiles; Caps.max_namespace_files;
                                                                   FileStore::namespace_object_uri; T3: pub mod lock
crates/egret-workflow/src/storage/uri.rs                      MOD  T2: StorageUri, NsFileUri, parse_any, namespace_file_uri
crates/egret-workflow/src/storage/lock.rs                     NEW  T3: LockedSession (moved out of prune.rs, generalised)
crates/egret-workflow/src/storage/prune.rs                    MOD  T3: uses storage::lock::LockedSession
crates/egret-workflow/src/storage/nsfiles/mod.rs              NEW  T2: module doc, install_db/db_options, NsError, CreatedBy
crates/egret-workflow/src/storage/nsfiles/path.rs             NEW  T2: NsPath (the one path rule)
crates/egret-workflow/src/storage/nsfiles/access.rs           NEW  T2: Access, Actor, Grant, authorize, check_namespace
crates/egret-workflow/src/storage/nsfiles/store.rs            NEW  T2 reads (resolve, list_live, revisions, search, open);
                                                                   T3 writes (put, create_directory, soft_delete, move_path, purge_revisions)
crates/egret-workflow/src/storage/nsfiles/glob.rs             NEW  T5: PathMatcherPredicate (glob:/regex:), Commons-IO wildcard (T7)
crates/egret-workflow/src/storage/nsfiles/stage.rs            NEW  T5: NamespaceFilesSpec, IfExists, stage
crates/egret-workflow/src/storage/nsfiles/routes.rs           NEW  T8: nine routes; T9: export
crates/egret-workflow/src/storage/nsfiles/zip.rs              NEW  T9: streamed STORED zip writer
crates/egret-workflow/src/storage/multipart.rs                NEW  T8: MultipartGate, HeaderBudget, multipart_refusal moved out of routes.rs
crates/egret-workflow/src/storage/scope.rs                    MOD  T4: Snapshot's namespace-file lookups; scope_for(flow_namespace)
crates/egret-workflow/src/runners/files.rs                    MOD  T4: FileTarget, classify, NsFileArgs, NsFileMeta, two trait methods
crates/egret-workflow/src/runners/functions.rs                MOD  T4: fileURI; the four functions dispatch on classify
crates/egret-workflow/src/spec/validate.rs                    MOD  T2: the `_files` publish rule; T5: WorkingDirectory.namespaceFiles
crates/egret-workflow/src/models/task.rs                      MOD  T5: WorkingDirectory keeps namespace_files
crates/egret-workflow/src/scripts/mod.rs                      MOD  T5: namespaceFiles leaves COMMON_REFUSED; ScriptTask.namespace_files
crates/egret-workflow/src/worker/handlers/script.rs           MOD  T5: ScriptRuntime.pool; staging before inputFiles
crates/egret-workflow/src/worker/commands.rs                  MOD  T5: module doc (:105-107)
crates/egret-workflow/src/worker/claim.rs                     MOD  T4: scope_for(..., flow_namespace) at :1522 and :1740
crates/egret-workflow/src/worker/handlers/namespace.rs        NEW  T6-T7: UploadFiles, DownloadFiles, DeleteFiles, PurgeFiles
crates/egret-workflow/src/worker/handlers/mod.rs              MOD  T6: pub mod namespace
crates/egret-workflow/src/spec/registry.rs                    MOD  T6-T7: four TASK_TYPES rows
crates/egret-workflow/src/runtime.rs                          MOD  T5 (ScriptRuntime.pool), T6 (namespace::register), T10 (KestraRuntime.pool)
crates/egret-workflow/src/worker/handlers/kestra.rs           MOD  T10: namespace-file leaves staged
crates/egret-workflow/src/routes.rs                           MOD  T8: router + OpenAPI; multipart code moves to storage/multipart.rs
crates/egret-workflow/oracle-exec/{w9b-namespace-files.json,driver.rs,README.md,handler-surface.txt}  T1, T6, T7, T11
crates/egret-workflow/tests/{nsfiles_store.rs,nsfiles_render.rs,nsfiles_tasks.rs,nsfiles_api.rs,nsfiles_s3_live.rs}  NEW
crates/egret-workflow/tests/{scripts_gate.rs,publish.rs,storage_prune.rs,container_docker_live.rs,k8s_runner_live.rs,plugin_host_live.rs}  MOD
plugin-host/src/main/java/io/kestra/core/runners/{ShimRunContext.java,EgretPluginHost.java,EgretNamespace.java(NEW)}  T10
plugin-host/src/test/java/io/kestra/core/runners/{EgretStorageTest.java,EgretNamespaceTest.java(NEW)}             T10
openapi.json, frontend/src/api/generated/**                   REGEN T8, T9 (`npm run gen:api`)
docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md    MOD  T11 (§9 W9 row + W9b correction + release note)
docs/{en,vi}/specs/2026-10-05-workflow-w9b-namespace-files-design.md  MOD  T1 (only where measured otherwise), T11 (dated corrections)
```

---

## Task 1: Measure Kestra, and the baselines

**Files:**
- Create `crates/egret-workflow/oracle-exec/w9b-namespace-files.json`.
- Reuse `oracle-exec/docker-compose.yml` (the artifact `kestra/kestra:v2.0.0-rc13`) and `harvest.py`'s `BASE`/`TENANT`/`multipart()` client. Add nothing unless a measurement needs it; if it does, `docker-compose.w9b.yml`, and say why.
- Amend spec §2, §3, §6, §7, §8 (EN and VI) **only** where a measurement contradicts them, dated.

**Interfaces:**
- Produces `w9b-namespace-files.json`, shaped like `w9a-storage.json`: `artifact`, `artifact_note`, `captured_at`, `method`, `how_to_read_for_egret`, `findings` (quoted values), `egret_expected` (typed, below), `raw_records`. Later tasks read it with `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w9b-namespace-files.json"))`. A cell whose egret answer is the spec's choice and not Kestra's carries `"basis": "chosen"` and Kestra's answer under `"kestra"`, as W9a's `egret_expected_basis.rule`.

| `egret_expected` key | shape | consumed by |
|---|---|---|
| `layout` | for one path written three times, deleted, then rewritten: every object key in the store per step (listed from the artifact's storage volume), the revision numbers, and whether `createDirectory` puts an object | T2, T3 |
| `path_rules` | per raw path (`a/b.txt`, `/a/b.txt`, `a/../b`, `./a`, `a//b`, a 256-char segment, a 255-char segment, `/_flows/x`, `/_flowsx`, `x.v2`, `x.v`, `x.v02`, `a\b`, `a?b`, `a#b`, a control char, `a b.txt`, `é.txt`) → the API's status and message on upload | T2 |
| `uri` | `fileURI` and `namespaceFile.uri()` for revisions 1, 2, 3 of `/a/b.txt` in `io.kestra.tests` and of a directory | T2, T4 |
| `globs` | `[{pattern, path, matches}]`, ≥ 30 rows: what string the predicate sees (leading `/` or not), a pattern starting with a word character (`*.txt`, `a/*.txt`), `/**`, `**/x`, `{a,b}`, `[ab]`, `?`, `regex:^/a/.*$`, an exclude, a directory row | T5, T6, T7 |
| `file_uri` | `{present_rev1, present_rev3, with_revision_2, with_revision_missing, revision_non_integer, missing, soft_deleted, dotdot, kestra_arg, other_namespace_authority}` → `{"value"}` or `{"error"}` | T4 |
| `read` | `read` of: a bare path, `nsfile:///p`, `nsfile://other.ns/p`, `nsfile://other.ns/p` with `namespace=`, a `kestra:///{own}/_files/p` (latest = v3: **which revision's bytes**), `kestra:///{other}/_files/p`, `read(p, revision=2)`, a binary file | T4 |
| `four_functions_ns` | `fileSize`, `fileExists`, `isFileEmpty` on present, missing, soft-deleted, directory | T4 |
| `staging` | a `shell.Commands` with `namespaceFiles` over two namespaces holding the same `/x.txt` under each `ifExists`; `folderPerNamespace: true` (the directory name: namespace with dots or slashes); an `inputFiles` entry `x.txt` under each `ifExists` (whose bytes the command sees); `include`/`exclude` effects; `enabled: false` | T5 |
| `tasks` | per class of `UploadFiles, DownloadFiles, DeleteFiles, PurgeFiles`: properties (`required`, `default`) from the plugin schema, outputs, and one run each with fixed inputs (outputs, then the namespace's rows via the API) | T6, T7 |
| `purge` | `PurgeFiles` `keepAmount: 1` on a soft-deleted path (does the latest survive?), `before`, `namespacePattern`, `includeChildNamespaces` with `a.b` vs `a.bc` | T7 |
| `api` | for each route of `NamespaceFileController`: status, headers and body shape; the multipart part name; `revisions` and `stats` bodies; `search` matching rule (substring? case?); `directory` listing of a nested tree | T8, T9 |
| `plugin_namespace_use` | for each class in `crates/egret-workflow/src/plugins/catalog.json`: whether its bytecode calls `Namespace`/`storage().namespace(` (`javap -c` over the three jars in `plugin-host/plugins.lock`) | T10 |
| `files_segment_namespaces` | `{published_flows: [..], corpus_entries: [..], count_published, count_corpus}` | T2 |

- [ ] **Step 1: Bring up** the artifact: `docker compose -f docker-compose.yml up -d` in `oracle-exec/`; confirm `GET /api/v1/main/plugins` lists `io.kestra.plugin.core.namespace`. **Hazard (W13b):** never `docker exec psql` as a child of the artifact's postmaster; read Kestra's metadata through its API. If the image refuses a request shape, **stop and report**; do not switch images.
- [ ] **Step 2: Measure** every row of the table, saving raw JSON, bytes and headers into `raw_records`:
  1. `layout`/`uri`/`path_rules`/`api`: through `NamespaceFileController` (`webserver/…/NamespaceFileController.java`), object keys by listing the artifact's storage directory (`docker exec <kestra> find /app/storage -path '*_files*'`).
  2. `globs`: a `shell.Commands` task with `namespaceFiles.include: [<pattern>]` that runs `find . -type f`, one run per pattern over a fixed tree; and `PathMatcherPredicate` directly in `jshell` with the classpath `plugin-host/target/lib/*` (build it with `mvn -f plugin-host/pom.xml -q dependency:copy-dependencies` if absent) to record the exact string it is given. **egret implements against this table, not against a reading of the Java** (§7).
  3. `file_uri`/`read`/`four_functions_ns`: a `Log` task per cell; an error cell records the execution's failed message.
  4. `staging`, `tasks`, `purge`: as the table says; read the namespace's state after each run through `GET …/files/revisions`.
  5. `plugin_namespace_use`: `javap -c -p` over every class named in `catalog.json`, grep for `io/kestra/core/storages/Namespace` and `storage()`'s `namespace(`. Record each class with `true`/`false`, the count, and the jar sha256s.
  6. `files_segment_namespaces`: corpus — `/usr/bin/grep -rn "^namespace:" oracle-exec/corpus` filtered for a `_files` segment (a segment = between dots); published flows — **one read-only** `SELECT workspace_id, namespace, flow_id FROM workflow.flows WHERE '_files' = ANY(string_to_array(namespace, '.'))` on the dev database (`egret-dev-postgres-1`), and the same on `:5439`. Report each row and the counts; none is expected. Report beside it the total rows examined (`SELECT count(*) FROM workflow.flows`) so the zero has a number that could disagree.
- [ ] **Step 3: Save** the fixture with timestamps and the image digest. Where a question cannot be measured, say so in `findings` and mark egret's answer `"basis": "chosen"`.
  - If a `kestra:///…/_files/p` URI with no `.vN` reads **revision 1's** bytes when the latest is 3 (the object at the key `p`), record it: Task 2 then parses such a URI as revision 1 exactly (this plan's choice; see Task 2). If it reads the latest, **stop and report** before Task 2: the spec's URI rule would then need the user.
  - If `plugin_namespace_use` finds a catalog class that writes namespace files, **stop and report**: §10 assumes none does.
- [ ] **Step 4:** `docker compose … down -v`.
- [ ] **Step 5: Baseline gate** at the branch base, with the Global Constraints' env, into the SDD workspace (prefix every scratch file `w9b-` — memory `parallel-agents-share-the-scratchpad`):
  - `cargo test -p egret-workflow --no-fail-fast 2>&1`, the name/result list as `w9b-baseline-gate.txt`;
  - `cargo test -p egret-server --lib` (must be green at the base; if `committed_openapi_json_is_current` is red there, **stop and report**);
  - the ratchet: `cargo test -p egret-workflow --test executor executor_matches_golden -- --nocapture`, its line as `w9b-baseline-ratchet.txt`; expected `294 golden entries, 108 mismatching, 108 baselined, 0 broken` — a different line is reported before anything else;
  - the two corpus flows' current outcome: their entries in `oracle-exec/baseline.txt` (both baselined) and their `executor_matches_golden` rows, recorded verbatim;
  - clippy, **both lists**: `cargo clippy --workspace --all-targets --keep-going 2>&1` (no `-D warnings`) → the sorted `warning:` locations as `w9b-baseline-clippy-warn.txt`; `cargo clippy --workspace --all-targets -- -D warnings 2>&1` → its `error:` locations as `w9b-baseline-clippy-deny.txt`. Task 11 gates on **no new location** in the first and reports the second beside it (W9a: under `-D` `egret-workspace` fails to compile and hides every crate depending on it).
- [ ] **Step 6:** Commit `docs(workflow): W9b Task 1 — namespace files measured on the artifact`.

---

## Task 2: URIs, path rules, `authorize`, the table, and reads

**Files:**
- Create `migrations/0169_workflow_namespace_files.sql`, `src/storage/nsfiles/{mod.rs,path.rs,access.rs,store.rs}`, `tests/nsfiles_store.rs`.
- Modify `src/storage/mod.rs` (`pub mod nsfiles;`, `Caps` gains `max_namespace_files`, `FileStore::namespace_object_uri`), `src/storage/uri.rs`, `src/runners/files.rs` (only `NAMESPACE_FILES_SEGMENT` and `namespace_marker`, so the marker has one definition from the start; Task 4 adds the rest), `src/spec/validate.rs` (`validate_namespace`, `:625`), `crates/egret-config/src/lib.rs` (`WorkflowStorageConfig`, `:805`, its `validate` `:829` and `Default` `:850`), `crates/egret-bin/src/main.rs` (`:1586`).
- Every `Caps { … }` literal gains the field (`git grep -n "Caps {" -- crates` → 22 lines at `d7bd1b3c`, including the declaration); prefer `..Caps::default()` where a test spells only some caps.

**Interfaces:**
- Consumes (W9a): `storage::{FileStore, Caps, current, install_mem_for_tests}`, `uri::{SCHEME_PREFIX, namespace_as_path, parse, ExecUri}`, `spec::validate::is_valid_namespace` (`validate.rs:662`, `pub(crate)`).
- Produces:

```sql
-- migrations/0169_workflow_namespace_files.sql (exact DDL; comments final at first commit)
-- W9b: namespace files (docs/en/specs/2026-10-05-workflow-w9b-namespace-files-design.md §4.1).
-- One row per revision; a path's state is its highest revision's row. A namespace file
-- exists only while its row does. No FK to a namespace: there is no namespace table, and
-- a namespace with no flow can hold files.
CREATE TABLE workflow.namespace_files (
  id                   bigserial   PRIMARY KEY,
  workspace_id         uuid        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  namespace            text        NOT NULL CHECK (namespace <> ''),
  -- normalised by nsfiles::path::NsPath: a leading '/', a directory ends with '/'
  path                 text        NOT NULL CHECK (left(path, 1) = '/' AND path <> '/'),
  revision             integer     NOT NULL CHECK (revision >= 1),
  is_directory         boolean     NOT NULL,
  size                 bigint      NOT NULL DEFAULT 0 CHECK (size >= 0),
  deleted              boolean     NOT NULL DEFAULT false,
  created_at           timestamptz NOT NULL DEFAULT now(),
  -- who wrote it: a user (the API) or an execution (a task); no FK, the row outlives its writer
  created_by_user      uuid,
  created_by_execution uuid,
  CONSTRAINT workflow_namespace_files_revision_key UNIQUE (workspace_id, namespace, path, revision),
  CONSTRAINT workflow_namespace_files_directory_shape
    CHECK (is_directory = (right(path, 1) = '/') AND (NOT is_directory OR (revision = 1 AND size = 0))),
  CONSTRAINT workflow_namespace_files_one_writer
    CHECK (num_nonnulls(created_by_user, created_by_execution) <= 1)
);
```

The unique constraint's index leads with `(workspace_id, namespace, path)`, so it **is** §4.1's index for the latest row (`ORDER BY revision DESC LIMIT 1` scans it backwards); a second index would duplicate it. Say so in the report; `EXPLAIN` of the latest-row query on 10 000 rows shows an index scan, recorded.

```rust
// src/storage/uri.rs — added; W9a's parse/ExecUri/Tail unchanged in shape
pub const NAMESPACE_FILES_SEGMENT: &str = crate::runners::files::NAMESPACE_FILES_SEGMENT; // "_files", one definition (T4 puts it in runners)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NsFileUri {
    pub namespace: String,   // ns_path with '/' -> '.', checked by access::check_namespace
    pub path: String,        // NsPath form: leading '/', no ".vN" suffix
    pub revision: u32,       // 1 when the URI has no ".vN" suffix (Task 1, `read`): the URI names an OBJECT
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageUri { Execution(ExecUri), NamespaceFile(NsFileUri) }
/// The FIRST segment equal to `_files` at index >= 1 marks a namespace file (§3), and this
/// check runs before the `executions/{uuid}` search: `kestra:///a/_files/x/executions/<uuid>/f`
/// is namespace `a`'s file `/x/executions/<uuid>/f`.
pub fn parse_any(uri: &str) -> Result<StorageUri, String>;
/// W9a's parse, now: parse_any, and a NamespaceFile answer becomes
/// Err("it is a namespace file URI (…/_files/…), not an execution file").
pub fn parse(uri: &str) -> Result<ExecUri, String>;
impl NsFileUri { pub fn to_uri(&self) -> String; }  // parse_any(u)? == NamespaceFile(n) => n.to_uri() == u
/// `kestra:///{ns-as-path}/_files{path}[.v{revision}]`, read back through parse_any (as W9a's `checked`).
pub fn namespace_file_uri(namespace: &str, path: &crate::storage::nsfiles::path::NsPath, revision: u32) -> Result<String, String>;

// src/storage/mod.rs
pub struct Caps { pub max_file_bytes: u64, pub max_task_bytes: u64, pub max_read_bytes: u64, pub max_namespace_files: u64 }
impl FileStore {
    /// `s3://{bucket}/{prefix}{workspace}/{ns-as-path}/_files{path}[.v{revision}]`. The workspace is the grant's.
    pub fn namespace_object_uri(&self, grant: &nsfiles::access::Grant, path: &nsfiles::path::NsPath, revision: u32) -> String;
}

// src/storage/nsfiles/path.rs — THE path rule (§3)
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NsPath(String);
impl NsPath {
    /// A file: leading '/' added when missing; refuses an empty, '.' or '..' segment, a segment over 255
    /// chars, `/_flows` or under it, '\\' '?' '#' or a control char, a last segment ending in `.v{digits}`,
    /// a trailing '/'. The error names the path and the rule.
    pub fn parse_file(raw: &str) -> Result<Self, String>;
    /// A directory: the same rules, the stored form ends with '/'; `/` itself is refused (the root has no row).
    pub fn parse_dir(raw: &str) -> Result<Self, String>;
    pub fn as_str(&self) -> &str;
    pub fn is_dir(&self) -> bool;
    /// The lock key's path part: the path without its trailing '/', so a file `/a` and a directory `/a/` share it.
    pub fn lock_path(&self) -> &str;
    /// Every parent directory, outermost first: `/a/b/c.txt` -> [`/a/`, `/a/b/`].
    pub fn parents(&self) -> Vec<NsPath>;
}

// src/storage/nsfiles/access.rs — THE access decision (§4.6, §5)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access { Read, Write, Purge }
#[derive(Clone, Copy, Debug)]
pub enum Actor<'a> {
    /// A running execution's flow; `namespace` comes from the execution (the flow row), never from a URI.
    Flow { namespace: &'a str },
    /// An API route that has ALREADY checked the caller's permission: `workflow:read` for Read,
    /// `workflow:manage` for Write/Purge (route tests pin both, Task 8).
    Api,
}
/// Proof that `authorize` said yes. Fields private: only `authorize` builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant { workspace: Uuid, namespace: String, access: Access }
impl Grant { pub fn workspace(&self) -> Uuid; pub fn namespace(&self) -> &str; pub fn access(&self) -> Access; }
/// `is_valid_namespace` + no `_files` segment. Publish (validate.rs) and authorize both call it.
pub fn check_namespace(namespace: &str) -> Result<(), String>;
/// Read: any namespace of `workspace`. Write: `target == own` exactly. Purge: `target == own` or
/// `target.starts_with(own + ".")`. Api: any access (the permission was checked by the route).
/// A refusal names the namespace and the rule, never whether a file exists there.
pub fn authorize(workspace: Uuid, actor: Actor<'_>, target: &str, access: Access) -> Result<Grant, String>;
/// For store functions: `Err(Error::Internal)` when a write is attempted with a Read grant -- egret's bug, not the author's.
pub(crate) fn require(grant: &Grant, access: Access) -> egret_core::Result<()>;

// src/storage/nsfiles/mod.rs
#[derive(Debug, thiserror::Error)]
pub enum NsError {
    #[error("{0}")] Author(String),     // a rule, a refused namespace, a missing object behind a row
    #[error("{0}")] NotFound(String),   // no live row; the message names the path, never another workspace
    #[error("{0}")] Conflict(String),   // ERROR conflict, file/directory clash, move onto a live target
    #[error("{0}")] TooLarge(String),   // a cap, named
    #[error("{0}")] Store(String),      // the object store failed: infrastructure
    #[error(transparent)] Db(#[from] egret_core::Error), // the database failed: infrastructure
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreatedBy { User(Uuid), Execution(Uuid) }
/// Connect options for render-time lookups (Task 4). Set once by egret-bin from its pool.
pub fn install_db(opts: sqlx::postgres::PgConnectOptions) -> Result<(), sqlx::postgres::PgConnectOptions>;
pub fn db_options() -> Option<std::sync::Arc<sqlx::postgres::PgConnectOptions>>;
#[cfg(any(test, feature = "test-util"))]
pub fn install_db_for_tests(pool: &sqlx::PgPool);   // idempotent: pool.connect_options() (one DB per test process)

// src/storage/nsfiles/store.rs — the read side
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NsEntry { pub path: NsPath, pub revision: u32, pub is_directory: bool, pub size: u64,
                     pub deleted: bool, pub created_at: chrono::DateTime<chrono::Utc> }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want { Latest, Revision(u32) }
/// The live row: Latest = the highest revision, which must not be deleted; Revision(n) = that row,
/// and the path's highest revision must not be deleted (a soft-deleted path is invisible, §4.3).
pub async fn resolve(conn: &mut PgConnection, grant: &Grant, path: &NsPath, want: Want) -> Result<NsEntry, NsError>;
/// Live latest rows under `dir` (None = the namespace root); recursive or one level.
/// Fetches at most `cap + 1` rows; the `+1` is `NsError::TooLarge` naming workflow.storage.max_namespace_files.
pub async fn list_live(conn: &mut PgConnection, grant: &Grant, dir: Option<&NsPath>, recursive: bool, cap: u64) -> Result<Vec<NsEntry>, NsError>;
pub async fn revisions(conn: &mut PgConnection, grant: &Grant, path: &NsPath) -> Result<Vec<NsEntry>, NsError>; // deleted flagged
pub async fn search(conn: &mut PgConnection, grant: &Grant, needle: &str, cap: u64) -> Result<Vec<NsEntry>, NsError>; // per Task 1 `api.search`
/// The namespaces of `workspace` that hold at least one row (PurgeFiles' namespacePattern, T7).
pub async fn namespaces_with_files(conn: &mut PgConnection, workspace: Uuid) -> Result<Vec<String>, NsError>;
/// Stream the entry's object. A missing object is NsError::Author naming the path and revision (§4.5,
/// no fallback); any other store error is NsError::Store.
pub async fn open(fs: &FileStore, grant: &Grant, e: &NsEntry)
    -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, NsError>;
```

- [ ] **Step 1: Write the failing tests.**

```rust
// src/storage/uri.rs (unit)
#[test] fn the_files_marker_is_recognised_before_the_executions_search() {
    // "kestra:///a/_files/x/executions/<uuid>/f" -> NamespaceFile { namespace "a", path "/x/executions/<uuid>/f", revision 1 }
    // "kestra:///io/kestra/tests/_files/a/b.txt.v3" -> { "io.kestra.tests", "/a/b.txt", 3 }
    // "kestra:///_files/x" -> Err (no namespace before the marker)
    // "kestra:///a/_files" and "kestra:///a/_files/" -> Err (no path)
    // parse("kestra:///a/_files/x") -> Err naming "namespace file"
}
#[test] fn namespace_file_uris_match_the_artifact() { /* every w9b-namespace-files.json `uri` row, both directions */ }
#[test] fn a_v_suffix_is_a_revision_only_when_it_is_two_or_more_digits_only() {
    // ".v2" -> 2; ".v1", ".v0", ".v02", ".v", ".vx" -> refused by parse_any (a file name may not end so: NsPath rule)
}
#[test] fn the_first_files_segment_is_the_marker() {
    // "kestra:///a/_files/_files/x" -> namespace "a", path "/_files/x" (a `_files` PATH segment is legal; only a
    // namespace segment is refused); "kestra:///a/b/_files/x" -> namespace "a.b";
    // "kestra:///a/_files.x/y" -> not a marker (segment is "_files.x") -> W9a's execution search -> Err
}

// src/storage/nsfiles/path.rs (unit) -- one test per refusal, each named for the mutant that removes it
#[test] fn the_paths_match_the_artifact() { /* path_rules: egret's accepted/refused set; "chosen" rows say so */ }
#[test] fn dotdot_and_dot_and_empty_segments_are_refused() {}
#[test] fn a_segment_of_256_characters_is_refused_and_255_is_not() {}
#[test] fn flows_and_under_it_are_refused_and_flowsx_is_not() {}
#[test] fn a_file_ending_in_v_digits_is_refused() {}
#[test] fn backslash_query_fragment_and_control_characters_are_refused() {}
#[test] fn a_missing_leading_slash_is_added() {}
#[test] fn parents_are_outermost_first_and_exclude_the_root() {}

// src/storage/nsfiles/access.rs (unit)
#[test] fn a_flow_reads_any_namespace_of_its_workspace() {}
#[test] fn a_flow_writes_only_its_own_namespace_compared_exactly() { /* "a" vs "a.b", "a.b" vs "a", "A" n/a (lowercase rule) */ }
#[test] fn a_flow_purges_its_own_and_child_namespaces_on_a_dot_boundary() { /* own "a.b": "a.b.c" yes, "a.bc" NO, "a" NO */ }
#[test] fn a_files_segment_namespace_is_refused_for_every_access_and_actor() { /* "a._files", "_files" (also invalid), "a._files.b" */ }
#[test] fn an_invalid_namespace_is_refused_naming_it() {}
#[test] fn a_refusal_never_mentions_existence() { /* no "exist", "found", "missing" in any authorize message */ }
#[test] fn the_grant_carries_the_callers_workspace() {}

// tests/nsfiles_store.rs (Postgres :5439; rows inserted with plain SQL in this task)
#[tokio::test] async fn latest_is_the_highest_live_revision() {}
#[tokio::test] async fn a_soft_deleted_latest_hides_every_revision() { /* Want::Latest and Want::Revision(1) both NotFound */ }
#[tokio::test] async fn the_same_path_in_two_workspaces_resolves_to_the_grants_row_only() {
    // identical (ns, path) in ws A (size 1) and B (size 2); grant for A -> 1; grant for C -> NotFound, never 2
}
#[tokio::test] async fn list_live_refuses_one_past_the_cap_naming_it() { /* cap 3, 4 rows -> TooLarge; 3 rows -> 3 */ }
#[tokio::test] async fn list_live_does_not_match_a_sibling_prefix() { /* dir "/a/" never lists "/ab/x" ; "_" in names */ }
#[tokio::test] async fn a_row_whose_object_is_missing_is_an_author_error_naming_path_and_revision() { /* MemStore, no object */ }
#[tokio::test] async fn the_directory_shape_check_refuses_a_directory_with_revision_two() { /* the CHECK, by SQL */ }
#[tokio::test] async fn deleting_a_workspace_cascades_its_namespace_files() {}
// spec/validate.rs (unit) + tests/publish.rs
#[test] fn publish_refuses_a_files_segment_namespace_by_name() { /* code `namespace_files_segment`, path `namespace` */ }
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - `parse_any`: split as W9a's `parse` does (`uri.rs:70-157`): scheme, empty/`.`/`..` segments and refused characters first (the same checks, shared — extract them into a private `fn segments(uri) -> Result<Vec<&str>, String>` that both arms use); then `runners::files::namespace_marker(&segments)` (defined in `runners/files.rs` in this task: `segments.iter().skip(1).position(|s| *s == NAMESPACE_FILES_SEGMENT).map(|i| i + 1)`) — the first index ≥ 1 equal to `_files`; then W9a's `executions/{uuid}` search unchanged. Namespace = `segments[..i].join(".")`, checked with `access::check_namespace`; path = `/` + rest joined; a trailing `.v{N}` with N ≥ 2 and digits only, no leading zero, splits off as the revision.
  - **The `_files` publish rule:** `validate_namespace` (`validate.rs:625`) pushes a second finding, code `namespace_files_segment`, message from `check_namespace`'s error, path `namespace`. `check_namespace` itself calls `is_valid_namespace` first; publish keeps its existing `invalid_namespace` finding and adds the segment one only when the pattern passes, so a message is never doubled. Name in the report which test pins the save-time refusal and which the run-time twin (`a_files_segment_namespace_is_refused_for_every_access_and_actor`).
  - `NsPath`: the rules in the order the interface lists; each `Err` is `format!("namespace file path {raw:?}: {rule}")`.
  - `authorize`: `check_namespace(target)?`, then by actor and access. `Actor::Flow { namespace }` with an invalid own namespace is `Err` too (a flow published before the `_files` rule; Task 1 lists any).
  - `store.rs` reads: one `concat!` query each, `WHERE workspace_id = $1 AND namespace = $2` in every statement. Latest row per path: `SELECT DISTINCT ON (path) … ORDER BY path, revision DESC`, then filter `NOT deleted` in Rust **after** taking the latest (filtering in SQL first would resurrect an older live revision of a deleted path — a test pins it: `a_soft_deleted_latest_hides_every_revision`). Children: `left(path, length($3)) = $3`; one level: no further `/` after the prefix except a directory's trailing one.
  - `namespace_object_uri`: `format!("s3://{}/{}{}/{}/{}{}{}", bucket, prefix, ws, namespace_as_path(ns), "_files", path, suffix)`, `suffix` `""` or `.v{N}`. A directory never gets one (the caller never asks; `debug_assert!`).
  - `egret-config`: `max_namespace_files: u64`, serde default 10 000, `EGRET_WORKFLOW__STORAGE__MAX_NAMESPACE_FILES`, ≥ 1 in `validate`.
  - `egret-bin` (`main.rs:1586`): after `storage::install`, `egret_workflow::storage::nsfiles::install_db((*db.pool.connect_options()).clone())` — read how `db` (the `egret_metastore::Db`) exposes its pool first; `sqlx::Pool::connect_options` exists in sqlx-core 0.9.0 (`pool/mod.rs:545`). A second install is a boot error, as for the store.
- [ ] **Step 4: Run the tests to see them pass**, then the per-task gate.
- [ ] **Step 5: Mutations:** (a) `parse_any` searching `executions` before `_files` (the marker test dies); (b) `authorize` Purge with `starts_with(own)` and no `"."` (`a.bc` dies); (c) `authorize` Write accepting a child namespace; (d) `check_namespace` without the `_files` segment check (both the publish test and the authorize test die — show both); (e) each `NsPath` rule removed in turn (five runs, five deaths); (f) a read query without `workspace_id = $1` (the two-workspace test dies); (g) `resolve` filtering `NOT deleted` before taking the latest; (h) `list_live`'s `LIMIT cap + 1` replaced by `LIMIT cap` (the cap test dies: it can no longer tell "exactly cap" from "over"); (i) SQL mutant: drop `workflow_namespace_files_directory_shape` from the migration on a scratch database (`dropdb`/`createdb` on `:5439` only), proven by the SQL test.
- [ ] **Step 6: Commit** `feat(workflow): W9b Task 2 — namespace file URIs, path rules, one access check and the metadata table`.

---

## Task 3: Writes — the lock, revisions, conflicts, soft delete, move and purge

**Files:**
- Create `src/storage/lock.rs`. Modify `src/storage/prune.rs` (`LockedSession` `:275-294` moves to `lock.rs`; `prune_pass_with` `:242` uses it unchanged in behaviour), `src/storage/nsfiles/store.rs`, `tests/nsfiles_store.rs`, `tests/storage_prune.rs`; create `tests/nsfiles_s3_live.rs`.

**Interfaces:**
- Consumes: Task 2's `Grant`, `NsPath`, `NsEntry`, `NsError`, `CreatedBy`, `resolve`, `FileStore::namespace_object_uri`; W9a's `ObjectStore::{put_stream, head, get_stream, delete}` (`crates/egret-storage/src/lib.rs:46-91`).
- Produces:

```rust
// src/storage/lock.rs
/// A pooled connection holding session-level advisory locks. Dropped without `unlock`, it is
/// detached from the pool and closed (closing a session releases its locks) -- prune.rs's rule.
pub(crate) struct LockedSession { /* conn: Option<PoolConnection<Postgres>>, unlocked: bool */ }
impl LockedSession {
    /// `pg_advisory_lock` on every key, in the order given (callers sort).
    pub(crate) async fn acquire(pool: &PgPool, keys: &[i64]) -> egret_core::Result<Self>;
    pub(crate) fn conn(&mut self) -> &mut PgConnection;
    /// `pg_advisory_unlock` on every key; hands the connection back only if every unlock answered true.
    pub(crate) async fn unlock(self) -> egret_core::Result<()>;
}
// prune.rs keeps PRUNE_LOCK_KEY and its own try-lock; only the struct moves.

// src/storage/nsfiles/store.rs — the write side
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict { Overwrite, Error, Skip }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Written { Written(NsEntry), Skipped(NsPath) }
/// The lock key of one path: hashtextextended($1, 0) in SQL over the bound text "w9b:{ws}:{ns}:{lock_path}".
/// Every writer locks the path's parents and the path, ascending by `lock_path` (a parent sorts first).
/// §4.2: lock -> read latest -> conflict rule -> revision = max + 1 (1 when none) -> put_stream to the
/// revision's key under max_file_bytes -> head -> insert row -> insert missing parent directory rows
/// (ON CONFLICT DO NOTHING) -> commit. A live row of the other kind is Conflict; a deleted one is purged
/// first (rows and objects, inside the same lock). Commits immediately: namespace files are not in any
/// execution's transaction (§6).
pub async fn put(pool: &PgPool, fs: &FileStore, grant: &Grant, path: &NsPath,
                 body: &mut (dyn tokio::io::AsyncRead + Send + Unpin), conflict: Conflict, by: CreatedBy)
    -> Result<Written, NsError>;
/// The directory and its missing parents; an existing live directory is Ok (idempotent); a live file there is Conflict.
pub async fn create_directory(pool: &PgPool, grant: &Grant, path: &NsPath, by: CreatedBy) -> Result<NsEntry, NsError>;
/// §4.3: the latest row of `path` and of every child marked deleted, in ONE statement; objects kept.
/// NotFound when nothing live is there. Returns what it marked.
pub async fn soft_delete(pool: &PgPool, grant: &Grant, path: &NsPath) -> Result<Vec<NsEntry>, NsError>;
/// §4.4: refuses a live target; copies every revision of `from` and of each child to `to` keeping revision
/// numbers (get_stream -> put_stream, never ObjectStore::copy: its copy source is not URL-encoded,
/// crates/egret-storage/src/s3.rs:237-252, and is capped at 5 GiB per S3 op), inserting rows as it goes;
/// on any failure purges every copy made, in reverse order, and reports both paths; then purges the source.
pub async fn move_path(pool: &PgPool, fs: &FileStore, grant: &Grant, from: &NsPath, to: &NsPath, by: CreatedBy)
    -> Result<Vec<(NsEntry, NsEntry)>, NsError>;
/// Hard delete of the given revisions of one path (and, when `path` is a directory, nothing else: callers
/// pass files). Session-locked: delete rows RETURNING, commit, delete objects, unlock. A crash leaves orphan
/// objects, never a row naming nothing (§8.4). Requires Access::Purge or Access::Write (the API's DELETE
/// never calls it; move does). Returns how many revisions were purged.
pub async fn purge_revisions(pool: &PgPool, fs: &FileStore, grant: &Grant, path: &NsPath, revisions: &[u32])
    -> Result<u64, NsError>;
```

**Why session locks for purge and move.** §4.2 holds the lock across the upload so that no two writers compute the same revision. A purge that deletes the rows of `p` (revision 1) and commits **releases a transaction lock**; a writer can then take revision 1 again and upload to the key `p`, which the purge then deletes — a row naming nothing, the one outcome §8.4's order exists to prevent. Holding a **session** lock on the same key across "delete rows, commit, delete objects" closes that window, and keeps §8.4's order (a crash between the commit and the object deletes leaves orphans). Session and transaction advisory locks on one key conflict with each other (PostgreSQL docs, "Advisory Locks"); a test proves it on `:5439` rather than citing it.

- [ ] **Step 1: Write the failing tests.**

```rust
// tests/nsfiles_store.rs (Postgres :5439 + MemStore via install_mem_for_tests; flavor = "multi_thread")
#[tokio::test(flavor = "multi_thread")]
async fn two_concurrent_writers_of_one_path_get_distinct_revisions_and_keys() {
    // THE lock test. A body that blocks on a tokio::sync::Barrier after its first byte, so writer A holds
    // the lock mid-upload while writer B starts; then release. Revisions {2,3} (after an initial rev 1),
    // two distinct object keys, both objects present with their own bytes. 20 rounds.
    // Without the lock: the same revision is computed twice -> a unique violation or a shared key.
}
#[tokio::test(flavor = "multi_thread")]
async fn a_writer_of_a_child_and_a_writer_of_a_file_named_like_its_parent_never_both_win() {
    // put "/a/x" and put "/a" concurrently: exactly one succeeds, the other is Conflict naming the path
}
#[tokio::test] async fn revisions_number_one_then_v2_v3_as_the_artifact_lays_them_out() { /* w9b layout */ }
#[tokio::test] async fn conflict_error_and_skip() { /* ERROR -> Conflict naming the path, nothing written; SKIP -> Skipped, no object */ }
#[tokio::test] async fn rewriting_a_soft_deleted_path_creates_the_next_revision_live() {}
#[tokio::test] async fn a_live_directory_where_a_file_is_written_is_a_conflict_and_a_deleted_one_is_purged_first() {}
#[tokio::test] async fn parents_are_created_and_an_existing_parent_is_left_alone() {}
#[tokio::test] async fn a_failed_upload_rolls_back_and_the_next_write_reuses_the_revision() {
    // body errors after 3 bytes: no row; the next put gets the same revision and overwrites the orphan key
}
#[tokio::test] async fn a_file_over_max_file_bytes_is_refused_naming_the_cap_and_leaves_no_row() {}
#[tokio::test] async fn soft_delete_marks_the_path_and_every_child_in_one_statement() {}
#[tokio::test] async fn soft_delete_does_not_touch_a_sibling_with_a_shared_prefix() { /* "/a/" vs "/ab/", "/a_b" */ }
#[tokio::test] async fn move_refuses_a_live_target() {}
#[tokio::test] async fn move_copies_every_revision_and_child_then_purges_the_source() {}
#[tokio::test] async fn move_that_fails_mid_copy_purges_its_copies_in_reverse_and_leaves_the_source() {
    // a FailingStore wrapper over MemStore that fails the Nth put_stream; source rows/objects identical before/after
}
#[tokio::test(flavor = "multi_thread")] async fn a_purge_and_a_writer_never_leave_a_row_without_an_object() {
    // purge_revisions(p, [1]) concurrent with put(p): after both, every row's object exists (head is Some)
}
#[tokio::test] async fn a_session_lock_blocks_a_transaction_lock_on_the_same_key() { /* the PostgreSQL fact, measured */ }
#[tokio::test] async fn a_write_with_a_read_grant_is_an_internal_error() {}
// tests/storage_prune.rs
#[tokio::test] async fn a_prune_pass_leaves_namespace_files_untouched() {
    // §11: write ns files (rows + objects), age everything, run prune_pass; every row and object survives
}
// tests/nsfiles_s3_live.rs (MinIO-gated; FileStore with prefix "w9b-t3-test/<uuid>/"; delete_prefix of it at the end)
#[tokio::test(flavor = "multi_thread")] async fn a_namespace_file_larger_than_one_part_round_trips() { /* 20 MiB + 3 */ }
#[tokio::test(flavor = "multi_thread")] async fn move_and_purge_on_minio_with_a_name_holding_spaces_and_unicode() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **Lock keys:** the key **text** `format!("w9b:{ws}:{ns}:{lock_path}")` is built in Rust and bound as `$1` (a value, never SQL); the **hash** is computed in SQL, `SELECT pg_advisory_xact_lock(hashtextextended($1, 0))`, one statement per key, in ascending order of `lock_path` (a set-returning form over `unnest` does not guarantee the order). `put` locks `parents()` then `path`; `move_path` locks the union of both chains, sorted; `soft_delete` locks `path` (a writer under it locks it as a parent).
  - `put`'s capped reader: a small `AsyncRead` wrapper counting bytes and failing past `caps.max_file_bytes` with an error the caller maps to `NsError::TooLarge` naming `workflow.storage.max_file_bytes` (W9a's `collect_one`, `collect.rs:82`, counts the same way; reuse its counting code if it can be extracted without changing it, else write the wrapper here and say why).
  - Store errors map to `NsError::Store`, logged with the `s3://` key at `tracing::error!`; the returned message names the namespace and path only.
  - `purge_revisions`: `LockedSession::acquire(pool, &[key])` — compute the key with one `SELECT hashtextextended($1, 0)` first; `DELETE … RETURNING revision` in a transaction on `session.conn()`; commit; `delete` each object; `session.unlock()`. An object delete that fails after the commit is logged as an orphan at `WARN` and counted, not an error (§11: the row is gone, so the file is gone for every reader).
  - `prune.rs`: only the struct moves; its tests (`tests/storage_prune.rs`, `storage_prune_s3_live.rs`) run unchanged and pass — the report says so with their counts.
- [ ] **Step 4: Run the tests to see them pass**, the MinIO suite with the four env vars, then the per-task gate.
- [ ] **Step 5: Mutations:** (a) `put` without the advisory lock (the concurrency test must die — show the failing assertion; if it does not die in 20 rounds, the test is wrong: widen the barrier window and say so); (b) the lock keyed on `as_str()` instead of `lock_path()` (the file-vs-parent race dies); (c) `purge_revisions` with a transaction lock instead of the session lock (the purge-vs-writer test dies); (d) objects deleted before rows; (e) move without the rollback (the mid-copy test dies); (f) soft delete with `LIKE` instead of `left(...)` (the `_` sibling test dies); (g) the conflict check reading the oldest row instead of the latest.
- [ ] **Step 6: Commit** `feat(workflow): W9b Task 3 — namespace file writes under a per-path lock, soft delete, move and purge`.

---

## Task 4: The expression functions

**Files:**
- Modify `src/runners/files.rs`, `src/runners/functions.rs` (`REFUSED` `:265-282`, `file_function` `:316-377`, the tests `the_eight_refusals_name_themselves_and_their_owner` `:3251`, `file_uri_names_w9b_and_kv_names_w9c` `:3987`, `a_path_with_no_scheme_names_namespace_files_and_w9b` `:4006`, `an_nsfile_uri_names_namespace_files_and_w9b` `:4019`), `src/storage/scope.rs` (`Snapshot` `:47`, `scope_for` `:296`, the `Unconfigured` scope `:270`), `src/worker/claim.rs` (`input_for` `:1500`, `settle` `:1723`: both `scope_for` calls, `:1522` and `:1740`).
- Create `tests/nsfiles_render.rs`.

**Interfaces:**
- Consumes: Task 2 `uri::parse_any`, `NsPath`, `authorize`, `Actor::Flow`, `Access::Read`, `resolve`, `open`, `nsfiles::db_options`; Task 3 `put` (to seed tests).
- Produces:

```rust
// src/runners/files.rs — no sqlx, no storage types (runners stays below storage)
// (Task 2 already added NAMESPACE_FILES_SEGMENT and namespace_marker here.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileTarget { Execution, Namespace }
/// A string with no "://" or starting "nsfile://" -> Namespace; "kestra:///" whose path has the marker ->
/// Namespace; anything else -> Execution (W9a's rules then apply, unchanged).
pub fn classify(path: &str) -> FileTarget;
/// What a function was asked for, unparsed: storage parses it (path rule, nsfile authority, revision).
pub struct NsFileArgs<'a> { pub path: &'a str, pub namespace: Option<&'a str>, pub revision: Option<String> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NsFileMeta { pub size: u64, pub uri: String /* kestra:///…/_files/…[.vN] of the resolved revision */ }
pub trait RenderFiles {  // existing methods unchanged; two added, with defaults that refuse
    /// Missing: well-formed, readable, no live row (fileExists answers false). Refused: everything else.
    fn namespace_file(&self, args: &NsFileArgs<'_>) -> Result<NsFileMeta, FileUnavailable> { /* Refused: needs a running execution with storage */ }
    /// UTF-8 contents, capped at max_read_bytes; non-UTF-8 is an error naming the path.
    fn read_namespace_file(&self, args: &NsFileArgs<'_>) -> Result<String, String> { /* the same refusal */ }
    /// The running flow's namespace (from the execution's flow, never a URI); None outside an execution.
    /// Tasks 5, 6, 7 and 10 read it here and nowhere else.
    fn flow_namespace(&self) -> Option<&str> { None }
}

// src/storage/scope.rs
pub async fn scope_for(conn: &mut PgConnection, store: Option<Arc<FileStore>>, workspace: Uuid, execution: Uuid,
                       texts: &[&str], json: &[&serde_json::Value], task_prefix: Option<String>,
                       flow_namespace: Option<String>)   // NEW: surface.flow().map(|f| f.namespace.clone())
    -> Result<FileScope>;
// Snapshot gains: flow_namespace: Option<String>, ns_db: Option<Arc<PgConnectOptions>>,
//                 ns_conn: Mutex<Option<PgConnection>>  -- opened on first namespace call, on `handle`
```

**The rule this task implements (§6), and how it reaches a database from the render thread.** Namespace files commit outside any execution transaction, so a render may look them up on any connection. `settle` (`claim.rs:1723`) holds only its transaction's `&mut PgConnection` (its callers — `spawn.rs:445,601`, `subflow.rs:575,946`, `replay.rs:339`, `claim.rs:1656,2169,2190` — have no pool either), and that connection cannot be lent to the render thread. So the `Snapshot` opens **one** connection lazily, on its runtime `handle`, from `nsfiles::db_options()`, the first time a template calls a namespace function, and keeps it for the rest of that render scope. A `PgPool` is not used for this: a pool created in one `#[tokio::test]` runtime breaks in the next, and the process-wide connect options are plain data. The cost is one connect per render scope that uses a namespace function; the task measures it (median of 50 on `:5439`) and the report states it. A failed connect or query sets the scope's outage (`Snapshot::outage`, `scope.rs:139`), so the run is re-claimed with no attempt spent (§11).

- [ ] **Step 1: Write the failing tests.**

```rust
// runners/functions.rs (unit, fake RenderFiles over a map) -- the W9b tests REPLACE the three that pinned "names W9b"
#[test] fn file_uri_returns_the_resolved_revisions_uri() { /* fake answers NsFileMeta { uri } */ }
#[test] fn file_uri_of_a_kestra_uri_returns_it_unchanged_after_it_parses() {}
#[test] fn a_namespace_argument_together_with_an_authority_is_an_error() { /* AbstractFileFunction.java:181-191 */ }
#[test] fn a_bare_path_and_nsfile_reach_the_namespace_route_and_an_execution_uri_does_not() { /* spy on which trait method ran */ }
#[test] fn file_exists_of_a_missing_namespace_file_is_false() {}
#[test] fn kv_still_names_w9c_and_the_refused_list_shrinks_to_seven() { /* renamed from the_eight_refusals… */ }
// tests/nsfiles_render.rs (Postgres + MemStore; flavor = "multi_thread"; install_db_for_tests)
#[tokio::test(flavor = "multi_thread")] async fn the_functions_match_the_artifact() {
    // every w9b file_uri / read / four_functions_ns cell, through a real task run's render
}
#[tokio::test(flavor = "multi_thread")] async fn a_run_if_in_settle_reads_a_namespace_file() {
    // the settle path, which has no pool: runIf "{{ fileExists('cfg/on.txt') }}" -> the task runs
}
#[tokio::test(flavor = "multi_thread")] async fn a_path_assembled_at_render_is_read() { /* "{{ read('a/' ~ inputs.x) }}" */ }
#[tokio::test(flavor = "multi_thread")] async fn another_namespace_of_the_workspace_is_read_by_authority_and_by_kestra_uri() {}
#[tokio::test(flavor = "multi_thread")] async fn another_workspaces_namespace_file_is_not_found_and_never_its_bytes() {
    // same ns + path in ws B; the render in ws A fails naming the path, the message says nothing about B
}
#[tokio::test(flavor = "multi_thread")] async fn a_files_segment_namespace_authority_is_refused_by_name() {}
#[tokio::test(flavor = "multi_thread")] async fn read_over_max_read_bytes_fails_naming_the_cap() {}
#[tokio::test(flavor = "multi_thread")] async fn read_of_a_binary_file_fails_naming_the_path() {}
#[tokio::test(flavor = "multi_thread")] async fn a_database_outage_during_a_namespace_read_re_claims_with_no_attempt_spent() {
    // install options pointing at a closed port for this scope; the worker returns Err; attempts unchanged
}
#[tokio::test(flavor = "multi_thread")] async fn a_flow_trigger_condition_calling_file_uri_is_refused_by_name() {
    // execution_render_context (render.rs:808) carries no scope, as W9a's
}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - `file_function` (`functions.rs:316`): after the scope lookup, replace the W9b refusal (`:357-362`) with `match classify(path)`. `Execution` → W9a's path unchanged, except that a `namespace` or `revision` argument is an error naming the argument (Kestra ignores them there; record what Task 1 saw, and if Kestra ignores them, follow Kestra and say so). `Namespace` → `scope.0.namespace_file(&args)` / `read_namespace_file`, with `FileUnavailable::Missing` → `false` for `fileExists` exactly as the execution arm does.
  - `fileURI`: leaves `REFUSED`; registered beside the four (`functions.rs:175-185`, `env.add_function("read", …)` and its siblings) the same way; a `kestra://` argument parses through `parse_any` (via the scope, which owns storage types — add nothing to runners) and returns unchanged; else `namespace_file(&args)?.uri`. The `REFUSED` doc paragraph *"Updated 2026-10-01 (W9a Task 4): eight names now"* gains a dated W9b line; the count becomes seven.
  - `Snapshot::namespace_file`: the same thread guards as `read_bytes` (`scope.rs:162-180`: not on a runtime worker, multi-thread only) — extract them into one private fn both call; then on `handle.block_on`: parse (`NsFileArgs` → namespace from the authority, the `namespace` argument or `flow_namespace`; `NsPath::parse_file`; revision from the URI suffix or `revision.parse::<u32>()`, a non-integer an error naming the path), `authorize(self.workspace, Actor::Flow { namespace: flow_namespace }, &ns, Access::Read)`, `resolve(conn, &grant, &path, want)`, map `NotFound` → `FileUnavailable::Missing`, `Db`/connect errors → `self.outage(..)` + `Refused`. `read_namespace_file` then `open` + read at most `max_read_bytes + 1` + `decode_utf8` (W9a's, `storage/mod.rs:226`, its message names the URI: pass the `kestra:///…/_files/…` URI).
  - `Snapshot::flow_namespace` answers its field; the `Unconfigured` scope answers `None`.
  - `scope_for` gains `flow_namespace`; both call sites pass `surface.flow().map(|f| f.namespace.clone())` (`render.rs:632`). The `Unconfigured` scope's two new methods answer its existing `[storage]` message.
  - The module docs of `runners/files.rs` (*"Why a snapshot and not a lookup at render"*) and `storage/scope.rs` gain a dated W9b paragraph: execution files are snapshot-only; namespace files are looked up at render, and why.
- [ ] **Step 4: Run the tests to see them pass**, `cargo test -p egret-workflow --test oracle` (the render oracle's file-function entries must still match), then the per-task gate.
- [ ] **Step 5: Mutations:** (a) `authorize` skipped in `Snapshot::namespace_file` (the `_files`-authority test dies; the two-workspace test still passes, because `resolve` is scoped by the grant's workspace — say so); (b) `resolve` without `workspace_id = $1` (the two-workspace render test dies, independently of Task 2's store test); (c) a DB error mapped to `Missing` (the outage test dies: `fileExists` would answer `false`); (d) `classify` treating a bare path as an execution URI (the W9a refusal returns; the artifact test dies).
- [ ] **Step 6: Commit** `feat(workflow): W9b Task 4 — fileURI, and the file functions over namespace files`.

---

## Task 5: Globs and `namespaceFiles` staging; `WorkingDirectory` validated

**Files:**
- Create `src/storage/nsfiles/{glob.rs,stage.rs}`.
- Modify `src/scripts/mod.rs` (`COMMON_REFUSED` `:195-206` loses `namespaceFiles`, `COMMON_IMPLEMENTED` `:163-175` gains it, `ScriptTask` `:341` gains `namespace_files`, the parse loop `:445`), `src/worker/handlers/script.rs` (`ScriptRuntime` `:191` gains `pool`, `Script::render` `:235`, `run` `:458`), `src/worker/commands.rs` (module doc `:105-107`), `src/runtime.rs` (`ScriptRuntime { … }` `:765`), `src/models/task.rs` (`WorkingDirectory` `:563-567` and its comment `:552-556`), `src/spec/validate.rs` (`TaskKind::WorkingDirectory` arm `:1505`).
- Every `ScriptRuntime {` literal gains `pool: None` or the pool (`git grep -n "ScriptRuntime {" -- crates`: `runtime.rs:765`, `script.rs:634,782,1060,1516,1585`, `tests/container_docker_live.rs:98,1467` at `d7bd1b3c`).
- Tests beside each; `tests/scripts_gate.rs` (`:932-945`), `tests/publish.rs`, `tests/container_docker_live.rs`, `tests/k8s_runner_live.rs`.

**Interfaces:**
- Consumes: Task 2 `authorize`, `list_live`, `NsPath`, `FileStore::namespace_object_uri`; W9a `worker::commands::StoredInput` (`commands.rs:345`), `storage::transfer::TarSource::Stored`.
- Produces:

```rust
// src/storage/nsfiles/glob.rs -- Kestra's PathMatcherPredicate, implemented against w9b globs fixture
pub struct PathMatcher { /* includes: Vec<regex::Regex>, excludes: Vec<regex::Regex> */ }
impl PathMatcher {
    /// "glob:" default, "regex:" supported, "**/" prepended to a pattern starting with a word character
    /// (PathMatcherPredicate.java:112-114), Java glob syntax translated to a regex (each pattern compiled
    /// with regex::RegexBuilder::size_limit, an author error naming the pattern when it is over).
    /// Empty includes match everything. A refusal names the pattern.
    pub fn new(includes: &[String], excludes: &[String]) -> Result<Self, String>;
    /// includes AND NOT excludes, on the string the fixture says the predicate sees.
    pub fn matches(&self, path: &NsPath) -> bool;
}
/// Commons-IO FilenameUtils.wildcardMatch: '*' and '?' only (PurgeFiles.filePattern, Task 7).
pub fn wildcard_matches(pattern: &str, path: &NsPath) -> bool;

// src/storage/nsfiles/stage.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfExists { Overwrite, Fail, Warn, Ignore }
/// The block as written: strings may hold templates; parse checks shape, not values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceFilesSpec {
    pub enabled: Option<String>,             // template or "true"/"false"; absent = true when the block is present
    pub include: Vec<String>, pub exclude: Vec<String>,
    pub namespaces: Option<Vec<String>>,     // None = the flow's namespace
    pub if_exists: IfExists, pub folder_per_namespace: Option<String>,
}
impl NamespaceFilesSpec {
    /// The ONE parse, at publish (script tasks AND WorkingDirectory) and at run. Err = (property path relative to
    /// the block, message), e.g. ("namespaces", "must be a list of strings"); an unknown key is refused by name.
    pub fn parse(v: &serde_json::Value) -> Result<Self, (String, String)>;
}
/// The block after rendering.
pub struct RenderedNamespaceFiles { pub enabled: bool, pub include: Vec<String>, pub exclude: Vec<String>,
                                    pub namespaces: Vec<String>, pub if_exists: IfExists, pub folder_per_namespace: bool }
pub struct Staged { pub files: BTreeMap<String /*rel path in the working dir*/, crate::worker::commands::StoredInput>,
                    pub warnings: Vec<String> }
/// §7: for each namespace IN LIST ORDER: authorize(Read) -> list_live(recursive, cap = remaining of
/// max_namespace_files across all namespaces) -> PathMatcher -> rel = path without its leading '/'
/// (or "{namespace}/{path}" with folder_per_namespace, per the fixture). A rel already staged from an earlier
/// namespace: Overwrite replaces; Fail -> Err naming both namespaces and the path; Warn keeps the first and
/// pushes one line naming the path; Ignore keeps the first. Returns Err(String) for the author's failures,
/// NsError::{Store,Db} for infrastructure.
pub async fn stage(conn: &mut PgConnection, fs: &FileStore, workspace: Uuid, flow_namespace: &str,
                   spec: &RenderedNamespaceFiles) -> Result<Result<Staged, String>, NsError>;

// scripts/mod.rs
pub struct ScriptTask { /* … */ pub namespace_files: Option<crate::storage::nsfiles::stage::NamespaceFilesSpec> }
// handlers/script.rs
pub struct ScriptRuntime { /* … */ pub pool: Option<sqlx::PgPool> }   // None: a task declaring namespaceFiles fails naming [storage]
// models/task.rs
pub struct WorkingDirectory { pub tasks: Vec<Task>,
    #[serde(default, rename = "namespaceFiles", skip_serializing_if = "Option::is_none")]
    pub namespace_files: Option<serde_json::Value> }   // kept verbatim; validated by NamespaceFilesSpec::parse at publish
```

- [ ] **Step 1: Write the failing tests.**

```rust
// glob.rs (unit)
#[test] fn every_glob_row_of_the_artifact_matches_as_kestra_does() { /* w9b globs: ≥ 30 rows */ }
#[test] fn a_pattern_starting_with_a_word_character_gets_double_star_prefix() {}
#[test] fn regex_patterns_are_supported_and_an_invalid_one_is_refused_naming_it() {}
#[test] fn an_oversized_pattern_is_refused_naming_it_not_a_panic() { /* "a{1000}{1000}" style -> size_limit */ }
#[test] fn excludes_win_over_includes() {}
#[test] fn wildcard_star_and_question_mark_only() {}
// stage.rs (unit for parse; Postgres for stage)
#[test] fn parse_accepts_the_two_corpus_blocks_and_templates() { /* oracle-exec/corpus/valids/working-directory-namespace-files*.yaml */ }
#[test] fn parse_refuses_each_malformed_property_by_name() { /* namespaces: "x"; ifExists: NOPE; include: [1]; unknown: 1 */ }
#[tokio::test] async fn a_later_namespace_wins_under_overwrite_and_each_if_exists_does_what_the_artifact_does() {}
#[tokio::test] async fn folder_per_namespace_lays_out_as_the_artifact() {}
#[tokio::test] async fn one_file_past_max_namespace_files_across_namespaces_is_refused_naming_the_cap() {}
#[tokio::test] async fn a_files_segment_namespace_in_namespaces_is_refused_by_name() {}
#[tokio::test] async fn another_workspaces_namespace_stages_nothing() {}
// handlers/script.rs (spy runner)
#[tokio::test] async fn namespace_files_are_staged_as_stored_inputs_and_input_files_overwrite_them_under_every_if_exists() {
    // §7, the user's ruling: four runs (OVERWRITE, FAIL, WARN, IGNORE), inputFiles "x.txt" always wins,
    // and FAIL does NOT fail for an inputFiles collision
}
#[tokio::test] async fn a_store_outage_while_staging_is_a_worker_error() {}
#[tokio::test] async fn enabled_false_stages_nothing_and_calls_the_runner() {}
#[tokio::test] async fn an_input_files_value_that_is_a_namespace_file_uri_is_refused_naming_namespace_files() {
    // ambiguity 9: "{{ fileURI('x') }}" as an inputFiles value fails before the runner, naming `namespaceFiles`
}
// tests/scripts_gate.rs: the `namespaceFiles` refusal at :932-945 becomes `targetOS` (still refused, by name) and a
// new test publishes a script task with a valid namespaceFiles and refuses a malformed one naming
// `namespaceFiles.namespaces`.
// tests/publish.rs
#[tokio::test] async fn a_working_directory_with_a_malformed_namespace_files_is_refused_by_name_and_a_valid_one_publishes() {}
#[tokio::test] async fn a_working_directory_namespace_files_round_trips_through_the_model() { /* serialize -> parse -> equal */ }
// live (gated): container_docker_live and k8s_runner_live -- two namespaces, folderPerNamespace, each ifExists,
// and an inputFiles entry overwriting a namespace file under EVERY ifExists (spec §13); `cat` in the container
// prints the expected bytes
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - `scripts::parse` (`:445`): the `"namespaceFiles"` key goes through `NamespaceFilesSpec::parse`; an `Err((p, m))` is one `ScriptRefusal { code: CODE_INVALID, path: format!("namespaceFiles.{p}"), message }`. Update `COMMON_REFUSED`'s doc (*"the other three (`namespaceFiles`, `runner`, `targetOS`)"*), dated.
  - `validate.rs:1505`: when `w.namespace_files` is `Some(v)`, `NamespaceFilesSpec::parse(v)`; an error is a finding `working_directory_namespace_files_invalid` at `{path}.namespaceFiles.{p}`. Nothing else about `WorkingDirectory` changes; `executor/plan.rs:1230` and `claim.rs:258` are untouched — the report confirms with `git diff --stat` on both.
  - `models/task.rs`: the comment *"`cache`, `namespaceFiles`, `inputFiles`, `outputFiles` are execution-plane concerns dropped here"* gains a dated W9b line: `namespaceFiles` is kept (validated, not run).
  - **Handler order** (`script.rs` `run` `:458`): `render` stays synchronous (`:235`) and renders the block's strings (`include`, `exclude`, `namespaces`, `enabled`, `folderPerNamespace`) with the same `cx`; then, in `run`, with `rt.pool` and `rt.files`, `stage` on a pool connection with `flow_namespace` from `task.render.files().and_then(|f| f.0.flow_namespace())` (Task 4's method; never `render_str("{{ flow.namespace }}")`, the way `kestra.rs:406` reads it today); then **merge**: start from `Staged.files`, remove every name `inputFiles` declares (inline or stored), then add `input_files`/`stored_inputs` as today. One merge function, unit-tested over the four `ifExists` values. `Staged.warnings` go to `task.log.line(LogLevel::Warn, …)` (`logs/run_log.rs:163`).
  - `stage_inputs` (`script.rs:383`): a value that `classify`s as `FileTarget::Namespace` and starts with `kestra:///` or `nsfile://` fails before the runner with a message naming `namespaceFiles` (ambiguity 9); the existing execution-URI path is unchanged.
  - `commands.rs` module doc (`:105-107`): *"No `namespaceFiles`, no `targetOS`"* becomes a dated line: `namespaceFiles` arrive as `stored_inputs`, staged by the handler.
- [ ] **Step 4: Run the tests to see them pass**, both live suites with their gates, the ratchet (must read the same line as Task 1: the two corpus flows are `WorkingDirectory`), then the per-task gate.
- [ ] **Step 5: Mutations:** (a) staging after `inputFiles` (the overwrite test dies under OVERWRITE... and must also die under FAIL: show it); (b) `Fail` triggered by an `inputFiles` collision; (c) namespaces staged in reverse order; (d) the cap counted per namespace instead of across all; (e) `authorize` dropped from `stage` (the `_files` test dies); (f) `namespaceFiles` re-added to `COMMON_REFUSED` (the publish test dies); (g) `validate.rs` not calling `parse` for `WorkingDirectory` (the malformed-WD test dies).
- [ ] **Step 6: Commit** `feat(workflow): W9b Task 5 — namespaceFiles staged into script tasks; WorkingDirectory's validated`.

---

## Task 6: `UploadFiles` and `DownloadFiles`

**Files:**
- Create `src/worker/handlers/namespace.rs`, `tests/nsfiles_tasks.rs`.
- Modify `src/worker/handlers/mod.rs` (`pub mod namespace;`), `src/spec/registry.rs` (two `TaskTypeSpec` rows after the W9a block `:195-250`), `src/runtime.rs` (`handlers_for` `:704`: `crate::worker::handlers::namespace::register(&mut handlers, pool.clone(), crate::storage::current())` beside `storage::register` `:724`), `oracle-exec/driver.rs` (`registry()` `:137`, with `install_db_for_tests`), `oracle-exec/handler-surface.txt` (run `handler_surface.py` and commit its diff if it lists implemented types).

**Interfaces:**
- Consumes: Task 2 `authorize`, `resolve`, `list_live`, `open`; Task 3 `put`, `Conflict`, `CreatedBy::Execution`; Task 5 `PathMatcher`; W9a `index::{readable_executions}`, `uri::{parse, verbatim_file_uri}` (`uri.rs:339`), `storage::StoredFile`, `Outcome.files`, `RenderFiles::{task_prefix, flow_namespace}`.
- Produces:

```rust
// handlers/namespace.rs
pub const UPLOAD_FILES: &str = "io.kestra.plugin.core.namespace.UploadFiles";
pub const DOWNLOAD_FILES: &str = "io.kestra.plugin.core.namespace.DownloadFiles";
pub const DELETE_FILES: &str = "io.kestra.plugin.core.namespace.DeleteFiles";   // T7
pub const PURGE_FILES: &str = "io.kestra.plugin.core.namespace.PurgeFiles";     // T7
pub struct UploadFiles { /* pool: PgPool, files: Option<Arc<FileStore>> */ }
pub struct DownloadFiles { /* same */ }
impl UploadFiles { pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self; }
impl DownloadFiles { pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self; }
/// Registers the four (T7 adds two) with add_publishable.
pub fn register(handlers: &mut super::super::Handlers, pool: PgPool, files: Option<Arc<FileStore>>);
/// Properties and outputs per class, held against w9b-namespace-files.json `tasks` by a test (as W9a's DECLARED).
pub(crate) const DECLARED: &[(&str, &[(&str, bool /*required*/, Option<&str> /*default*/)], &[&str] /*outputs*/)];
```

**Every list that enumerates task types gains these:** `spec::registry::TASK_TYPES` (FQCN, no alias — as W9a's storage rows); `namespace::register` via `runtime::handlers_for`; `oracle-exec/driver.rs::registry()`; `handler-surface.txt` if the script lists them; `executor/mod.rs`'s `PUBLISHED_BUT_REFUSED` (`:3867`) if a type appears there (it should not; confirm). The plugin host's `NATIVE_TYPES` excludes ported types from the catalog; `describe` scans only the three JDBC jars, so `core.namespace` never reaches it — confirm with `describe` and state it.

- [ ] **Step 1: Write the failing tests** (`tests/nsfiles_tasks.rs`, Postgres + MemStore, `flavor = "multi_thread"`):

```rust
#[tokio::test] async fn declared_properties_and_outputs_match_the_artifact() { /* UploadFiles, DownloadFiles */ }
#[tokio::test(flavor = "multi_thread")] async fn upload_files_from_a_write_uri_and_a_glob_lands_as_the_artifact_lays_it_out() {
    // Write -> UploadFiles files: ["{{ outputs.w.uri }}", "**.txt"], destination "/in/" -> rows, revisions, bytes;
    // output is empty (Kestra's run returns an empty Output)
}
#[tokio::test(flavor = "multi_thread")] async fn upload_files_map_takes_a_uri_or_literal_contents() {}
#[tokio::test(flavor = "multi_thread")] async fn upload_files_to_another_namespace_is_refused_naming_it_and_writes_nothing() {}
#[tokio::test(flavor = "multi_thread")] async fn upload_files_conflict_error_and_skip() {}
#[tokio::test(flavor = "multi_thread")] async fn upload_files_glob_never_reaches_an_unreadable_execution() {
    // an unrelated execution in the same workspace holds "x.txt"; "**.txt" does not upload it (the readable set)
}
#[tokio::test(flavor = "multi_thread")] async fn upload_files_glob_past_the_cap_is_refused_naming_it() {}
#[tokio::test(flavor = "multi_thread")] async fn download_files_copies_into_the_task_prefix_and_indexes_at_commit() {
    // outputs.files: path -> kestra:// URI; rows in execution_files only after apply_outcome commits
}
#[tokio::test(flavor = "multi_thread")] async fn download_files_reads_another_namespace_of_the_workspace() {}
#[tokio::test(flavor = "multi_thread")] async fn download_files_of_a_files_segment_namespace_is_refused_by_name() {}
#[tokio::test(flavor = "multi_thread")] async fn a_store_outage_in_either_task_is_a_worker_error_not_a_failed_task() {}
#[tokio::test(flavor = "multi_thread")] async fn without_storage_both_fail_naming_storage() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - Properties through `serde` structs with W9a's `spec()` helper pattern (`storage.rs:353`), rendered with `render_str` on `task.render` as W9a's handlers do (read `handlers/storage.rs:388-460`, `Write`, first).
  - **The running namespace** is `task.render.files().and_then(|f| f.0.flow_namespace())` (Task 5's method) and nothing else; `None` fails the task naming that it is not a task run of a flow's execution.
  - `UploadFiles`: `namespace` rendered, default the flow's; `authorize(task.workspace_id, Actor::Flow{…}, ns, Access::Write)`. `files` items: a whole `kestra:///` string is resolved through the **render snapshot** (`scope.0.lookup`, W9a's one readability decision); any other string is a glob matched with `PathMatcher` over the readable execution files, listed on a pool connection with `readable_executions(conn, ws, task.execution_id)` + `SELECT uri, size FROM workflow.execution_files WHERE workspace_id = $1 AND execution_id = ANY($2) LIMIT $3` (cap + 1 = `max_namespace_files` + 1), each matched on its `Tail`'s `file` with a leading `/` (the spec's *"the URI's file path"*; see the report's ambiguity 6). Destination path = `destination` joined with that file path, through `NsPath::parse_file`. Each file: `get_stream` of the execution object → `put` with `CreatedBy::Execution(task.execution_id)`.
  - `DownloadFiles`: `namespace` required, `authorize(Read)`; `list_live` + `PathMatcher`; each match `open` → `put_stream` at `uri::verbatim_file_uri(task_prefix, &format!("{destination}{path_without_leading_slash}"))` (normalise exactly one `/`); `head` for the size; returned in `Outcome.files` (indexed by `apply_outcome`, W9a). Output `files`: `{path → uri}`.
  - Errors: `NsError::{Store, Db}` → `Err` (infrastructure); everything else a failed `Outcome` naming the path or cap.
- [ ] **Step 4: Run the tests to see them pass**, the ratchet (unchanged line), then the per-task gate.
- [ ] **Step 5: Mutations:** (a) `UploadFiles` with `Access::Read` (the other-namespace test dies); (b) the execution-file listing without `execution_id = ANY(readable)` (the unrelated-execution test dies); (c) `DownloadFiles` files not in `Outcome.files` (the index test dies); (d) the cap `LIMIT` dropped.
- [ ] **Step 6: Commit** `feat(workflow): W9b Task 6 — UploadFiles and DownloadFiles`.

---

## Task 7: `DeleteFiles` and `PurgeFiles`

**Files:** `src/worker/handlers/namespace.rs`, `src/spec/registry.rs`, the same lists as Task 6, `tests/nsfiles_tasks.rs`.

**Interfaces:**
- Consumes: Task 3 `soft_delete`, `purge_revisions`; Task 2 `list_live`, `revisions`, `namespaces_with_files`; Task 5 `PathMatcher`, `wildcard_matches`.
- Produces: `pub struct DeleteFiles`, `pub struct PurgeFiles`, each `new(pool: PgPool, files: Option<Arc<FileStore>>)`, registered by `namespace::register`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[tokio::test] async fn declared_properties_and_outputs_match_the_artifact() { /* DeleteFiles, PurgeFiles */ }
#[tokio::test(flavor = "multi_thread")] async fn delete_files_soft_deletes_matches_and_keeps_objects() {}
#[tokio::test(flavor = "multi_thread")] async fn delete_parent_folder_removes_parents_left_empty_deepest_first() {}
#[tokio::test(flavor = "multi_thread")] async fn delete_files_in_another_namespace_is_refused_and_deletes_nothing() {}
#[tokio::test(flavor = "multi_thread")] async fn purge_files_defaults_to_own_namespace_and_its_children_on_a_dot_boundary() {
    // own "a.b": rows of "a.b", "a.b.c" purged; "a.bc" and "a" untouched
}
#[tokio::test(flavor = "multi_thread")] async fn purge_files_refuses_a_listed_namespace_outside_the_purge_rule() {}
#[tokio::test(flavor = "multi_thread")] async fn namespaces_and_namespace_pattern_are_exclusive() {}
#[tokio::test(flavor = "multi_thread")] async fn namespace_pattern_matches_only_namespaces_holding_files_and_each_passes_the_rule() {}
#[tokio::test(flavor = "multi_thread")] async fn keep_amount_keeps_the_n_highest_revisions_and_size_counts_revisions_purged() {}
#[tokio::test(flavor = "multi_thread")] async fn before_purges_older_revisions_and_never_the_latest() {}
#[tokio::test(flavor = "multi_thread")] async fn a_soft_deleted_path_is_purged_entirely() { /* §2: chosen; Kestra's answer recorded beside */ }
#[tokio::test(flavor = "multi_thread")] async fn file_pattern_is_a_commons_io_wildcard() {}
#[tokio::test(flavor = "multi_thread")] async fn a_purge_leaves_no_row_without_its_object() { /* every surviving row: head is Some */ }
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - `DeleteFiles`: `namespace` default the flow's, `authorize(Write)`; matches by `PathMatcher` over `list_live`; each `soft_delete`; `deleteParentFolder`: after the deletes, for each distinct parent of a deleted path, deepest first, `soft_delete` it only if `list_live(dir, recursive=false)` is empty. Output per Task 1.
  - `PurgeFiles`: resolve the namespace set — `namespaces` (rendered list) or `namespacePattern` (regex over `namespaces_with_files`), default `[own]`; with `includeChildNamespaces` add every `namespaces_with_files` entry that is `ns + "."`-prefixed; **every** resolved namespace through `authorize(Purge)`, the first refusal fails the task naming it before anything is purged. Per namespace, per path (`revisions`): the set to purge by `behavior` — `keepAmount` keeps the N highest; `before` takes revisions with `created_at < before` except the highest; a path whose highest revision is deleted: every revision. Then `purge_revisions`. Output `size` = revisions purged.
  - Properties `behavior.type` per Kestra's `FilesPurgeBehavior`/`Version` (read `Version.java` in the checkout for the type discriminator spelling; the fixture decides).
- [ ] **Step 4: Run the tests to see them pass**, the ratchet, the per-task gate.
- [ ] **Step 5: Mutations:** (a) child match without the dot (`a.bc` dies); (b) the namespace check per-namespace after purging earlier ones (the refusal test asserts nothing was purged: it dies); (c) `before` purging the latest; (d) soft-deleted latest kept by `keepAmount` (Kestra's behaviour; the chosen test dies); (e) `DeleteFiles` with `Access::Read`.
- [ ] **Step 6: Commit** `feat(workflow): W9b Task 7 — DeleteFiles and PurgeFiles`.

---

## Task 8: The namespace file API (nine routes)

**Files:**
- Create `src/storage/multipart.rs`, `src/storage/nsfiles/routes.rs`, `tests/nsfiles_api.rs`.
- Modify `src/routes.rs`: move `MultipartGate` (`:1774`), `HeaderBudget` (`:1818`), `HEADER_BUDGET_PIECE_BYTES`, its `Stream` impl (`:1835`), `multipart_refusal` (`:1882`), `body_too_large`, `is_length_limit` (`:1760`), `MULTIPART_OVERHEAD_BYTES`, `MAX_MULTIPART_HEADER_BYTES`, `multipart_body_limit` (`:1942`), `is_multipart` (`:1947`) into `storage/multipart.rs` as `pub(crate)` (pure move; `start_multipart` `:2112` imports them; W9a's FILE-input tests pass unchanged — the report gives their count); add the routes to `management_router` (`:2564`) and the `paths(...)`/`components(schemas(...))` of the `#[derive(utoipa::OpenApi)]` (`:2628`).
- Regenerate `openapi.json` and the frontend client (`npm run gen:api`).

**Interfaces:**
- Consumes: everything in `nsfiles` (Tasks 2-3); W9a `storage::routes::content_disposition` (`routes.rs:193`, `pub(crate)`), its download header code (`:270-310`) — extract the header-setting into a `pub(crate) fn download_headers(res: &mut Response, name: &str) -> Result<(), Response>` both routes call, rather than copying it; `routes::{WF_READ, WF_MANAGE}` (`routes.rs:91,94`); `st.audit.emit(egret_audit::mutation_for(...))` (`routes.rs:575` pattern).
- Produces (paths relative to the router, which is mounted under `/api`):

```rust
// GET    /ws/{workspace}/workflows/namespaces/{namespace}/files?path&revision     workflow:read   bytes, W9a download headers
// GET    /ws/{workspace}/workflows/namespaces/{namespace}/files/stats?path        workflow:read   NsFileStatsDto
// GET    /ws/{workspace}/workflows/namespaces/{namespace}/files/revisions?path    workflow:read   Vec<NsFileRevisionDto>
// GET    /ws/{workspace}/workflows/namespaces/{namespace}/files/search?q          workflow:read   Vec<String> (live paths), capped
// GET    /ws/{workspace}/workflows/namespaces/{namespace}/files/directory?path    workflow:read   Vec<NsFileStatsDto> (live children)
// POST   /ws/{workspace}/workflows/namespaces/{namespace}/files?path  (multipart) workflow:manage put(OVERWRITE); .zip stored as a file
// POST   /ws/{workspace}/workflows/namespaces/{namespace}/files/directory?path    workflow:manage create_directory
// PUT    /ws/{workspace}/workflows/namespaces/{namespace}/files?from&to           workflow:manage move_path
// DELETE /ws/{workspace}/workflows/namespaces/{namespace}/files?path              workflow:manage soft_delete (path + children only)
#[derive(Serialize, utoipa::ToSchema)] pub struct NsFileStatsDto { pub path: String, pub r#type: String /* "File"|"Directory" per Task 1 */, pub size: u64, pub revision: u32, pub created: chrono::DateTime<chrono::Utc> }
#[derive(Serialize, utoipa::ToSchema)] pub struct NsFileRevisionDto { pub revision: u32, pub deleted: bool, pub size: u64, pub created: chrono::DateTime<chrono::Utc> }
#[derive(Deserialize, utoipa::IntoParams)] pub struct NsPathQuery { pub path: String, pub revision: Option<u32> }
#[derive(Deserialize, utoipa::IntoParams)] pub struct NsMoveQuery { pub from: String, pub to: String }
#[derive(Deserialize, utoipa::IntoParams)] pub struct NsSearchQuery { pub q: String }
```

- [ ] **Step 1: Write the failing tests** (`tests/nsfiles_api.rs`, `tests/common` app, MemStore, `install_db_for_tests`):

```rust
#[tokio::test] async fn read_routes_need_workflow_read_and_write_routes_need_workflow_manage() {
    // a user with workflow:read only: 5 GETs 200, 4 writes 403 (and nothing written); with manage: 200/201
}
#[tokio::test] async fn a_download_has_the_safe_headers() { /* octet-stream, sanitised attachment, nosniff; name with ", ;, CRLF */ }
#[tokio::test] async fn another_workspaces_namespace_file_is_404_and_an_unknown_path_is_404() {}
#[tokio::test] async fn flows_paths_are_400_on_every_route() {}
#[tokio::test] async fn a_files_segment_namespace_is_400_on_every_route_naming_it() {}
#[tokio::test] async fn each_path_rule_is_400_naming_the_rule() {}
#[tokio::test] async fn a_zip_upload_is_stored_as_one_file() {}
#[tokio::test] async fn an_upload_over_the_body_limit_is_413_and_an_unterminated_header_is_400() { /* W9a's multipart bounds, reused */ }
#[tokio::test] async fn delete_removes_the_path_and_its_children_and_not_an_emptied_parent() {}
#[tokio::test] async fn move_onto_a_live_target_is_409() {}
#[tokio::test] async fn stats_revisions_search_and_directory_have_the_artifacts_shapes() {}
#[tokio::test] async fn search_and_directory_past_the_cap_are_refused_naming_it() {}
#[tokio::test] async fn a_store_outage_is_503() {}
#[tokio::test] async fn every_write_route_emits_one_audit_mutation() { /* tests/common audit_mutations */ }
#[tokio::test] async fn a_namespace_with_no_flow_can_hold_files() {}
#[tokio::test(flavor = "multi_thread")] async fn a_download_of_64_mib_streams_from_minio() { /* MinIO-gated, throwaway prefix */ }
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - Each handler: `WsContext`, `egret_core::authz::require(&*st.pdp, &ctx, WF_READ | WF_MANAGE)`, then `authorize(ctx.workspace_id.0, Actor::Api, &ns, access)` (the `_files` and pattern refusal → 400), `NsPath::parse_*` (→ 400 naming the rule), then the store function. `NsError` → status: `Author` 400, `NotFound` 404, `Conflict` 409, `TooLarge` 413, `Store`/`Db` 503 (`Db` also logged).
  - Upload: the multipart body through `storage::multipart`'s `HeaderBudget` + `http_body_util::Limited` exactly as `start_multipart` (`routes.rs:2112-2150`) builds it; the route gets `DefaultBodyLimit::disable()` as the start route does (`routes.rs:2587`). The file part's name per Task 1 (`api`); its body streams into `put` (`CreatedBy::User(ctx.principal.user_id.0)`).
  - Audit: `workflow_namespace_file:{upload,create_directory,move,delete}`, resource type `workflow_namespace_file`, detail `{namespace, path}` (+ `to` for move).
  - utoipa: every response documented, as W9a's `download_execution_file` (`storage/routes.rs:256-269`).
- [ ] **Step 4: Run the tests to see them pass**; `cargo run -p egret-bin -- openapi > openapi.json`; `cargo test -p egret-server --lib`; `cd frontend && npm run gen:api && npm run lint && npx tsc --noEmit`; the per-task gate.
- [ ] **Step 5: Mutations:** (a) a write route checking `WF_READ` (the permission test dies); (b) `Actor::Api` replaced by skipping `authorize` (the `_files` test dies); (c) `Content-Disposition` unsanitised; (d) the body limit removed (413 test dies); (e) `DELETE` walking up to empty parents.
- [ ] **Step 6: Commit** `feat(workflow): W9b Task 8 — the namespace file API`.

---

## Task 9: `GET …/files/export`, a streamed zip

**Files:**
- Create `src/storage/nsfiles/zip.rs`. Modify `src/storage/nsfiles/routes.rs`, `src/routes.rs` (router, OpenAPI), `crates/egret-workflow/Cargo.toml` (`crc32fast = "=1.5.0"`, the version already in `Cargo.lock`; a new **direct** dependency — say so in the report), `tests/nsfiles_api.rs`. Regenerate `openapi.json` and the frontend client.

**Interfaces:**
- Consumes: Task 2 `list_live` (recursive, cap), `open`.
- Produces:

```rust
// zip.rs -- STORED entries (no compression), general-purpose flag bit 3 (sizes and CRC in a data descriptor
// after each entry, so nothing is buffered), ZIP64 extra fields and end records when any size, offset or the
// entry count exceeds the 32/16-bit fields, UTF-8 names (flag bit 11). At most one copy buffer in memory.
pub struct ZipWriter<W> { /* out, offset, central: Vec<CentralEntry> -- one small record per entry */ }
impl<W: tokio::io::AsyncWrite + Send + Unpin> ZipWriter<W> {
    pub fn new(out: W) -> Self;
    /// `name` is relative (no leading '/'); the reader is copied through a CRC32 + byte counter.
    pub async fn add(&mut self, name: &str, body: &mut (dyn tokio::io::AsyncRead + Send + Unpin), mtime: chrono::DateTime<chrono::Utc>) -> std::io::Result<u64>;
    pub async fn finish(self) -> std::io::Result<W>;
}
// GET /ws/{workspace}/workflows/namespaces/{namespace}/files/export   workflow:read
//   application/zip, Content-Disposition attachment "{namespace}_files.zip" (via content_disposition),
//   the latest live file of every path (no directories, no flows), body streamed through a tokio::io::duplex.
```

- [ ] **Step 1: Write the failing tests:**

```rust
// zip.rs (unit; the instruments are external readers)
#[tokio::test] async fn python_zipfile_and_unzip_read_every_entry_byte_for_byte() {
    // write 3 entries (empty, 1 byte, 3 MiB; names with space, é, sub/dir) to a temp file;
    // `python3 -c "import zipfile; z=zipfile.ZipFile(p); assert z.testzip() is None; ..."` and `unzip -t` both pass
    // (skip with a printed reason when either binary is absent; the report counts skips)
}
#[tokio::test] async fn a_zip64_archive_is_written_when_the_entry_count_passes_65535() { /* 65 536 empty entries; python reads them */ }
#[tokio::test] async fn the_writer_holds_no_entry_body_in_memory() { /* a 20 MiB generated body; peak buffered < 1 MiB (W9a's counting sink) */ }
// tests/nsfiles_api.rs
#[tokio::test] async fn export_streams_the_latest_live_files_only() { /* deleted and older revisions absent; directories absent */ }
#[tokio::test] async fn export_needs_workflow_read_and_is_404_for_another_workspace() {}
#[tokio::test] async fn export_past_max_namespace_files_is_refused_before_a_byte_is_sent() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.** Read the APPNOTE (PKWARE `APPNOTE.TXT` 6.3.x, sections 4.3.7-4.3.9, 4.3.12-4.3.16, 4.5.3) before writing; W9a's hand-rolled ustar (`storage/transfer.rs`) is the in-repo precedent for a byte-level format with external readers as the instrument. The route lists first (so the cap is refused before the 200 line), then spawns the writer into one end of a `tokio::io::duplex` and returns `Body::from_stream(ReaderStream::new(other_end))`; a store error mid-stream is logged naming the namespace and path and ends the body (the status line has gone), as W9a's `NamedErrors` (`storage/routes.rs:233`).
- [ ] **Step 4: Run the tests to see them pass**; openapi + `npm run gen:api` + lint + tsc; the per-task gate.
- [ ] **Step 5: Mutations:** (a) the data descriptor's CRC computed over the wrong bytes (python's `testzip` dies); (b) ZIP64 end records omitted (the 65 536-entry test dies); (c) the writer buffering each body (the memory test dies).
- [ ] **Step 6: Commit** `feat(workflow): W9b Task 9 — namespace export as a streamed zip`.

---

## Task 10: The plugin bridge

**Files:**
- Create `plugin-host/src/main/java/io/kestra/core/runners/EgretNamespace.java`, `plugin-host/src/test/java/io/kestra/core/runners/EgretNamespaceTest.java`.
- Modify `ShimRunContext.java` (`NO_NAMESPACE_FILES` `:274-288`, `storage()` `:255-271`, `StorageSpec` `:77`), `EgretPluginHost.java` (`storageSpec` `:343`, the doc `:49-55`), `EgretUnsupported.java` (doc `:5`), `EgretStorageTest.java` (the W9b assertion `:261` flips).
- Modify `src/worker/handlers/kestra.rs` (`KestraRuntime` `:160` gains `pool: Option<PgPool>`; the staging walk before `:395`; the "namespace files — W9b" messages `:342`, `:1165`), `src/runtime.rs` (`:775`), every `KestraRuntime {` literal (`kestra.rs:721`, `tests/k8s_runner_live.rs:2129`, `tests/plugin_host_live.rs:290`), `tests/plugin_host_live.rs`.

**Interfaces:**

```text
task.json gains (beside W9a's "storage", "baseUri", "maxTaskBytes", "context"):
  "namespaceFiles": [{"uri": "<the literal property value: nsfile://… or kestra:///…/_files/…>",
                      "namespace": "<ns>", "path": "/<path>", "revision": <n>, "size": <bytes>,
                      "staged": ".egret/storage/<n>"}]
```

```java
// EgretNamespace.java -- io.kestra.core.storages.Namespace over the staged entries of ONE namespace
final class EgretNamespace implements Namespace {
    EgretNamespace(String tenantId, String namespace, Path workingDir, List<Staged> entries);
    // getFileContent(path[, revision]), exists, getFileMetadata, all(..), findAllFilesMatching, get: over `entries`;
    // an absent path -> FileNotFoundException naming the path and saying that a plugin reads only namespace files
    // named whole in its properties (spec §10, §15).
    // putFile, delete, move, createDirectory, children: UnsupportedOperationException with that reason.
}
// ShimRunContext: NO_NAMESPACE_FILES is replaced by a NamespaceFactory whose of(...) returns an EgretNamespace
// over storageSpec's namespaceFiles entries for that namespace (empty list for another namespace).
```

- [ ] **Step 1: Write the failing tests.**
  - Java (`EgretNamespaceTest`, JUnit 5): `getFileContent` of a staged path returns its bytes; of another path `FileNotFoundException` naming it; `exists`; `findAllFilesMatching` over the staged set; `putFile`/`delete`/`move`/`createDirectory` throw `UnsupportedOperationException` with the reason; `storage().namespace("other.ns")` gives an empty namespace, not an exit 4. `EgretStorageTest.java:261` (the W9b message) flips to a test that `storage().namespace(ns)` no longer throws `EgretUnsupported`. `EgretStorage.get` of a staged `kestra:///…/_files/…` URI returns its bytes (the existing staged map).
  - Rust (`kestra.rs` spy tests): every **whole** string leaf of the rendered properties that is `nsfile://…` or a `kestra:///…/_files/…` URI is resolved through `authorize(Read)` + `resolve` on `rt.pool`, staged at `.egret/storage/<n>`, and listed under `namespaceFiles` in `task.json`; a substring inside SQL is not staged; a refused namespace fails the task at render, before any runner; a missing path fails naming it; a store outage is `Err`.
  - Live (`tests/plugin_host_live.rs`, `EGRET_PLUGIN_HOST_LIVE=1`, Docker, MinIO-gated): `postgresql.Query` with `sql: "{{ read('q.sql') }}"` (a function, no bridge) and a `postgresql.CopyIn` whose `from` is `"{{ fileURI('data.csv') }}"` (the bridge) both run against a namespace file.
- [ ] **Step 2: Run them to watch them fail** (`mvn -f plugin-host/pom.xml test` with `JAVA_HOME`; `cargo test -p egret-workflow --lib kestra`).
- [ ] **Step 3: Implement.**
  - **Read `InternalStorage.java:68-86` and `NamespaceFactory.java:15-29` in the rc13 jar** (not only the checkout) before writing the factory: `of(logger, tenant, ns, storage)` is what `InternalStorage.namespace(..)` calls. If rc13's `Namespace` interface differs from the checkout's (`Namespace.java:34-178`), the jar wins; list the methods implemented.
  - The handler classifies leaves with `runners::files::classify` and `uri::parse_any`; execution URIs keep W9a's path through the snapshot unchanged. The running namespace for `authorize` is `task.render.files().and_then(|f| f.0.flow_namespace())` (Task 4); the existing `render_str("{{ flow.namespace }}")` at `kestra.rs:406` stays only for task.json's `context`.
  - Messages that still say namespace files need W9b (`kestra.rs:342`, `:1165`, `EgretPluginHost.java:55`, `EgretUnsupported.java:5`) are rewritten to what is now true, dated.
- [ ] **Step 4: Run the tests to see them pass**; `mvn -f plugin-host/pom.xml verify`; rebuild `egret-plugin-host:dev` (`docker build -f deploy/docker/plugin-host.Dockerfile -t egret-plugin-host:dev plugin-host`) and run the live suite; the per-task gate.
- [ ] **Step 5: Mutations:** (a) `putFile` delegating instead of throwing (Java test dies, by running `mvn test`); (b) the handler staging substrings (the SQL spy test dies); (c) `authorize` skipped for a bridge leaf (the refused-namespace test dies); (d) the factory returning another namespace's entries (Java test dies).
- [ ] **Step 6: Commit** `feat(workflow): W9b Task 10 — Kestra plugins read namespace files through egret`.

---

## Task 11: End to end on a temporary kind cluster, coverage, docs, full CI

- [ ] **Step 0: Rebase if `origin/main` moved.** `git fetch origin` (GitHub may be flaky: retry up to three times with a pause; report each failure). If `origin/main` is ahead of `e539d8b3`, `git rebase origin/main`; resolve conflicts; **re-check that 0169 is still the next free migration number** (another branch may have taken it: renumber before anything else and say so); re-run the per-task gate before going on. Record the old and new base shas.
- [ ] **Step 1: Set up the cluster.**
  - `kind create cluster --name egret-w9b-e2e`; build and `kind load` `egret-plugin-host:dev`, `egret-task-helper:dev` and the backend image.
  - Install the chart with `workflow.containerRunner=kubernetes`, `workflow.workerEnabled=true`, `workflow.pluginHost.image=egret-plugin-host:dev`, and Postgres in a namespace the task NetworkPolicy lets through (W5b's `e2e-data`).
  - **MinIO:** if the chart's `deps.minio.image` pin answers 401 (as in W5b/W9a), `kind load` a locally present MinIO image and override `deps.minio.image`/`mcImage`; report it.
  - **Do not touch the shared `egret` cluster.**
- [ ] **Step 2: Drive the scenario through the API**, transcripts into the SDD workspace (`w9b-e2e-*`):
  1. Upload `cfg/a.txt` twice and `data.csv` through the API; `revisions` shows 1 and 2; download revision 1 and the latest (bytes differ as uploaded).
  2. A flow in that namespace: `Log` `{{ read('cfg/a.txt') }}` and `{{ fileURI('cfg/a.txt') }}` (ends `.v2`); a script task with `namespaceFiles` over two namespaces, `folderPerNamespace: true`, and an `inputFiles` entry overwriting one namespace file; its `cat` output checked.
  3. `UploadFiles` from a `Write` URI → `DownloadFiles` → `DeleteFiles` → `PurgeFiles` (`keepAmount: 1`); the API's `revisions` after each.
  4. A `kestra.…postgresql.CopyIn` whose `from` is `{{ fileURI('data.csv') }}`; row count in Postgres.
  5. **The pod holds no storage credential:** `kubectl get pod -o yaml` during a run (hold it with `sleep 20`) contains none of `[storage]`'s access key, secret key or endpoint; control: the image name appears.
  6. **Isolation:** a second workspace's flow reading `nsfile://<first ns>/cfg/a.txt` fails naming the path; its API answers 404 for the first workspace's namespace; a flow uploading to another namespace of its own workspace fails naming the namespace.
  7. Export: the zip downloads and `unzip -t` passes.
- [ ] **Step 3:** `kind delete cluster --name egret-w9b-e2e`.
- [ ] **Step 4: Coverage and docs.**
  - `oracle-exec/README.md` gains a `### W9b — namespace files` section after W9a's (`README.md:9251`, before `:9525`): the ratchet **measured** before (Task 1's `w9b-baseline-ratchet.txt`) and after (`executor_matches_golden -- --nocapture` now); the two `WorkingDirectory` corpus flows' outcome before and after (unchanged, both baselined — or the measured difference explained); every live test with its gate; what was measured live (Step 2) and what was not; the `_files`-namespace count from Task 1.
  - Programme spec §9 EN+VI (`docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md:484`): the W9 row says W9b shipped; a dated **W9b correction** paragraph after W9a's (`:502-506`), as W9a's: what exists, the one access function, the user's rulings, what is still not true (`WorkingDirectory`, plugin writes, KV W9c). §3 finding 4 (`:93`) updated.
  - **The release note** (in that correction, as W9a's `:504`): new config key `workflow.storage.max_namespace_files` (10 000); migration 0169; publish now refuses a namespace with a `_files` segment (Task 1's count says whom it affects); `namespaceFiles` publishes on script tasks; `WorkingDirectory.namespaceFiles` is validated at publish (a previously-accepted malformed block now fails to publish); nine API routes + export under `workflow:read`/`workflow:manage`; the plugin host no longer exits 4 on `storage().namespace(..)`; the known limits of spec §15.
  - The W9b spec EN+VI: the corrections this plan's reports list (each dated), including the ambiguities ruled in Tasks 2-10.
  - **Sweep** (N examined / M corrected per population, `/usr/bin/grep` with file arguments): (1) `\bW9b\b` in `crates/egret-workflow/src`, `plugin-host/src`, both programme specs — a message saying namespace files "need W9b" is now false; (2) `namespaceFiles` in docs and doc comments saying it is refused; (3) `NO_NAMESPACE_FILES`.
- [ ] **Step 5: Full local CI**, compared with Task 1's baselines:
  - `cargo fmt --all --check`;
  - clippy: `cargo clippy --workspace --all-targets --keep-going` — **no new `warning:` location** against `w9b-baseline-clippy-warn.txt`, and the baseline's locations listed as still present or gone; the `-D warnings` run reported beside it against `w9b-baseline-clippy-deny.txt`;
  - `cargo test --workspace --no-fail-fast` with the Global Constraints' env (commit first: it runs long), plus the MinIO-gated suites with the four `EGRET_TEST_S3_*` vars and `just test-shipping-resolution`; **every failure absent from `w9b-baseline-gate.txt` is this branch's until measured otherwise** (memory `pre-existing-is-a-hypothesis`): classify each against the base by running that test at the base sha;
  - `cargo test -p egret-server --lib` (`committed_openapi_json_is_current`); in `frontend/`: `npm run gen:api` (never bare `orval`), `git diff --exit-code src/api/generated`, `npm run lint`, `npx tsc --noEmit`, `npm test`;
  - `mvn -f plugin-host/pom.xml verify` with `JAVA_HOME` (the plugin host changed in Task 10);
  - `helm lint deploy/helm/egret`;
  - the ratchet line, beside Task 1's.
- [ ] **Step 6: Commit** `docs(workflow): W9b Task 11 — end to end on kind, coverage and the programme spec`. **Stop:** no PR until the user asks.
