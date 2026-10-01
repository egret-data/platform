# W9a — Internal storage: `kestra://` files for executions, tasks, plugins and the file functions

**Status:** design, revised 2026-10-01 to match the plan (`docs/en/plans/2026-10-01-workflow-w9a-internal-storage.md`)
**Depends on:** W4 (lease fence, `apply_outcome`), W5a (task runners, input/output file transfer), W5b (plugin host, catalog, `task.json`), W6/W13 (`trigger_execution_id`), W10 (execution routes), W11 (the pruner pattern) — all merged. Base `ecdf4868`.
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 finding 4 (`StorageInterface`), §9 (row W9), §11 open question 1
**Decided with the user, 2026-10-01:**
- **(Added 2026-10-01, the user's ruling on the plan's item 1.) Files are looked up in a snapshot taken before the render; there is no second lookup at render.** A URI assembled at render (string concatenation) that is not in the snapshot is an error naming it (§4.4, §7.2).
- **(Added 2026-10-01, the user's ruling on Task 1's concern 1.) A file written inside a `Loop` iteration carries the iteration's execution id, as Kestra does; there is no loop-root mapping for URIs.** The readable set therefore covers every iteration under the same loop root, and the file API serves the whole loop family (§3, §4.2, §4.4, §8).
- **(Added 2026-10-01, the user's ruling on Task 7's concern 1.) The readable set also holds the loop family of each trigger execution** — its loop root and every iteration under that root — still one trigger level, never the trigger's trigger. A `Subflow` child started from inside a `Loop` iteration therefore reads the loop root's FILE inputs and its sibling iterations' files, as Kestra OSS allows (§2, §4.4, §15).
- **W9 is split into three packages:** W9a internal storage (this spec), then W9b namespace files, then W9c the KV store.
- **egret moves files for the pod.** A task pod holds no storage credential and opens no connection to the object store. egret stages every file a task may read into its working directory and collects every file it writes, through W5a's existing transfer (§5).
- **Streamed, with large configurable caps.** `egret-storage` gains streaming; no file is ever held whole in egret's memory (§4.1).
- **A running flow reads its own execution's files and its trigger execution's** (and a replay its source's, §4.4), never another workspace's, and never an unrelated execution's in the same workspace (§4.4). This is narrower than Kestra OSS on purpose (§2).
- **Retention of its own, 30 days by default**, by a pruner modelled on W11's (§10).
- **`outputFiles` becomes a map of name → `kestra://` URI, as in Kestra.** This breaks egret flows that read W5a's name → contents map (§6.2).
- **In scope:** FILE inputs and the file functions (§7), the execution file API (§8), the storage bridge for Kestra plugins (§9.1), `egret.sql` with `fetch: STORE` (§9.2), and the core storage tasks with `Loop` over a file (§9.3; the programme's name `ForEach` is Kestra's `io.kestra.plugin.core.flow.Loop` in this checkout).

**Delivery:** one PR (docs and code), fourteen tasks in §12, implemented by the plan `docs/en/plans/2026-10-01-workflow-w9a-internal-storage.md`.

## 1. What exists and what does not

| piece | state on `origin/main` (`ecdf4868`) |
|---|---|
| object store | `egret-storage`: trait `ObjectStore { put, get, delete, copy, list }` on whole `Vec<u8>` buffers, `s3://bucket/key` URIs; one backend, `S3Store` (S3 and MinIO); configured by `[storage]` (endpoint, region, bucket `warehouse`, keys, `path_style`). `egret-workflow` has it as a dev-dependency only |
| file functions | `runners::functions::REFUSED` refuses `read`, `fileURI`, `fileSize`, `fileExists`, `isFileEmpty` with "…, W9" (`functions.rs:245-263`) |
| FILE inputs | `InputType::File` exists (`models/input.rs`); the start route accepts only a JSON object; multipart webhook bodies are refused |
| `outputFiles` | a map of name → **contents** (UTF-8, ≤ 1 MiB each, `MAX_OUTPUT_FILE_BYTES`), documented as a departure from Kestra (`handlers/container.rs:46-58`) |
| file transfer | W5a: Docker `docker cp` of a staging directory; Kubernetes exec `tar -x` in and `tar -c` out, with an **in-memory** ustar packer (`worker/k8s/transfer.rs`) |
| plugins | W5b's catalog leaves out `postgresql.CopyIn`, `postgresql.CopyOut`, `postgresql.Batch`, `mysql.Batch` (`EgretPluginHost.NEEDS_STORAGE`), and refuses `fetchType: STORE` at publish (`plugins/mod.rs`) |
| `egret.sql` | `Fetch` leaves `STORE` out on purpose (`worker/handlers/sql.rs:143`). `QueryExecPort::execute_as_with_params` returns a whole `TabularResult`, capped by `Sql::max_rows` (`MAX_FETCH_ROWS`, 10 000). **`egret.sql` is registered in no production registry** (`Handlers::with_core_tasks` and `runtime::handlers_for` do not add it; `worker/registry.rs:90-120` says so) |
| `Loop` over a file | a named hole in `advance()` (`executor/flowable/loop_.rs:171`): a rendered `kestra:///….ion` arrives as a plain string. There is no loop-width limit in the code |
| ION | `runners::ionfmt` writes Ion **text** exactly as Kestra's `toIon` filter (`JacksonMapper.ofIon()`) does; Kestra's `FileSerde`, which writes plugin STORE and item-task files, writes **binary** Ion 1.0 (Task 1, `w9a-storage.json` finding `store_binary_ion`), so `ionfmt` does not reproduce those files; `ion-rs` is excluded as a writer (`Cargo.toml:173`) and is not in `Cargo.lock`. `ionfmt::read_ion` (behind `fromIon`) refuses timestamps and blobs |
| execution links | `trigger_execution_id` (0138, workspace-scoped composite FK): set for a `Subflow` child and a `Flow`-trigger-fired execution. `parent_execution_id` (0135): a loop iteration's sub-execution, **and** a retry (`CREATE_NEW_EXECUTION`) replay's source, told apart by `loop_index` (NULL for the replay, 0138). A **manual** replay (`POST …/executions/{id}/replay`) records no link (`routes.rs` `replay_execution`'s own doc) |
| retention | W11's pruner (`logs/prune.rs`): hourly, batches, a per-pass budget, `log_retention_days` default 30, 0 = forever |

## 2. What Kestra does, and where egret differs

Kestra (checkout `96893b589`, artifact `2.0.0-rc13`):
- `StorageInterface` (`core/…/storages/StorageInterface.java`): `get`, `getWithMetadata`, `allByPrefix`, `list`, `exists`, `getAttributes`, `put`, `delete`, `createDirectory`, `move`, `deleteByPrefix`, `purgeByLastModified`, plus `*InstanceResource` variants. `InternalStorage` is the per-run facade (`getFile`, `putFile`, `isFileExist`, `deleteExecutionFiles`, …).
- URIs (`StorageContext.java:41-57`): `kestra:///{ns-as-path}/{flow}/executions/{exec}/…` with `inputs/{input}/{file}`, `tasks/{taskId}/{taskRunId}/{file}`, `trigger/{triggerId}/{file}`; the namespace's dots become slashes; flow, task and trigger ids are slugified. Tenancy is in the backend path, not the URI.
- An output file is stored as `IdUtils.from(path) + "-" + filename` with spaces as `+` (`FilesService.java:94-97`): **deterministic**, not random.
- File functions (`AbstractFileFunction.java:136-171`) allow: the current execution's files; else the execution named by the `trigger` variable (one level: a subflow's parent or the execution that fired a `Flow` trigger); else any namespace `isAllowedNamespace` accepts — **which in OSS is every namespace** (`DefaultNamespaceService.java:38-41`).
- Execution file API (`ExecutionController.java:1173-1328, 2285-2338`): `GET /{id}/file`, `/file/metas`, `/file/preview` (100 rows, max 5000); a URI of another execution is redirected.
- Lifecycle: nothing expires on its own. Deleting an execution deletes its prefix; `PurgeExecutions`, `PurgeCurrentExecutionFiles`, `PurgeStorage` are tasks.

| Kestra | egret W9a | why |
|---|---|---|
| a flow may read any execution's task file in OSS (`isAllowedNamespace` is always true) | its own execution, its loop root and every loop iteration under that root, its trigger execution **and that trigger's loop family** (its loop root and every iteration under it; one trigger level), and its replay chain only (§4.4) | egret has a reader boundary (`workflow:read`) Kestra OSS does not; a flow author with `workflow:manage` must not read outputs of a flow they cannot see. **A Kestra flow that hands a URI to an unrelated flow runs on Kestra and fails on egret** |
| a plugin reads any URI it computes at run | a plugin reads the URIs present in its rendered properties | the pod holds no credential (§5). JDBC plugins are unaffected; a future plugin that computes URIs at run would fail, by name |
| files are kept forever | pruned 30 days after the execution ends (configurable, 0 = forever) | the user's choice (§10). A flow that reads a file of an execution older than the window fails, naming the URI and the window |
| `read()` has no cap | `workflow.storage.max_read_bytes`, default 10 MiB | the result is spliced into a rendered string |
| the file API redirects (301) to the owning execution (measured, Task 1) | 200 for a file of `{id}`'s loop family (its loop root and every iteration under it), else 404 with no redirect (§8) | inside the loop family the readable set already joins the executions (§4.4); anything else a viewer asks by the owning execution's own id |
| a file function reads any URI a template computes | a URI must be in the snapshot taken before the render: in the flow source, or in a JSON value of the render context (inputs, outputs, execution, loop item). One assembled by string concatenation is an error naming it | the user's ruling (header): inside `settle` the render thread cannot use the transaction's connection, and a pool would not see the rows that transaction just wrote (§4.4) |
| `Loop` reads an ION file in batches of `concurrencyLimit`, with no cap | the worker reads the whole file before `advance()`, capped by `workflow.storage.max_read_bytes` | `advance()` stays pure; no loop-width limit exists to reuse (§9.3) |
| an output file is `IdUtils.from(<absolute path in a random temp dir>) + "-" + name` | the same scheme over egret's fixed `/egret/working-dir/<rel>` | the name scheme matches; the hash cannot, because Kestra's directory is random |
| a replay reads what its `trigger` variable names | a replay also reads its source's files, transitively along the replay chain | a replay copies its source's `inputs` verbatim, so a FILE input's URI names the original execution (§4.4) |
| `allowedFileExtensions` is **not enforced** by the artifact (Task 1: `bad.csv` accepted at start, and `POST …/validate` returned no error) | enforced at start (§7.1) | chosen: the property says what it means. A flow that relied on Kestra ignoring it fails on egret |
| JDBC STORE files and the item tasks' output are **binary** Ion 1.0 (Task 1) | `egret.sql` STORE writes Ion **text** through `ionfmt` (§9.2); every egret reader (preview, `Loop`, the item tasks) accepts both | Kestra's readers accept Ion text: measured for `Loop`, the file preview and `DeduplicateItems`/`FilterItems` reading `Write` output (Task 1). `ionfmt`'s own bytes were **not** fed to a Kestra reader (unmeasured) |
| `file://`, multipart webhook bodies, `nsfile://` | refused | out of scope; `nsfile://` and `fileURI` are W9b |

## 3. Storage layout and URIs

- **Object key:** `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/{flow}/executions/{exec}/{tail}` in the `[storage]` bucket. `workflow.storage.prefix` defaults to `workflow/`.
- **URI a flow sees:** `kestra:///{ns-as-path}/{flow}/executions/{exec}/{tail}` — Kestra's form. **It carries no workspace**: egret maps a URI to a key by prefixing the workspace **of the running execution**, never a value read from the URI, so nothing in a URI can name another workspace.
- `tail` is Kestra's: `inputs/{input}/{file}`, `tasks/{taskId}/{taskRunId}/{file}`, `trigger/{triggerId}/{file}`, or a bare `{file}` directly under the execution — the form Kestra's `Loop` writes for `outputs` in STORE mode, `executions/{iteration}/{n}.ion`, measured by Task 1 on `valids/loop-outputs-auto` (`w9a-storage.json`: `egret_expected.loop_over_ion["valids/loop-outputs-auto"].loop_outputs`, finding `loop_outputs_store_uri`). Slugify and the output file name **scheme** follow Kestra exactly; Task 1 measures both on Kestra and pins them in a fixture (`oracle-exec/w9a-storage.json`). The output name hashes the file's absolute path in the pod, `/egret/working-dir/<rel>`; Kestra's working directory is random, so the hash cannot match Kestra's, only the scheme.
- **`{exec}` is the writing execution — the user's ruling (header).** egret runs a loop iteration as a sub-execution (`parent_execution_id` with a `loop_index`), and so does Kestra 2.0: Task 1 measured each iteration as an execution of kind `LOOP`, and a file written inside an iteration carries the **iteration's** id although `execution.id` renders the root's id there (`w9a-storage.json` finding `loop_iteration_uri_is_the_iteration`). egret does the same: no URI is rewritten to the loop root. The loop root is still computed, for the readable set (§4.4) and the file API (§8). The walk follows `parent_execution_id` only while the row has a `loop_index`: a retry replay also sets `parent_execution_id` (with `loop_index` NULL) and the walk stops there.
- A URI is refused if its scheme is not `kestra`, if it holds `..`, or if it does not parse into the layout above.

## 4. Core

### 4.1 Streaming in `egret-storage`

`ObjectStore` keeps its `Vec<u8>` methods for today's callers and gains `put_stream(uri, AsyncRead, size_hint)` (a single PUT under 8 MiB, else a multipart upload in 8 MiB parts, aborted on error), `get_stream(uri) -> AsyncRead`, `head(uri) -> Option<size>` and `delete_prefix(uri)`. Tested against a real MinIO with a file larger than one part, measuring that the process's resident memory does not grow with the file.

### 4.2 The index

`workflow.execution_files` (migration 0161): `workspace_id`, `execution_id` (the writing execution — the iteration for a file written inside a `Loop` iteration, the `{exec}` of its URI, §3; composite FK with `workspace_id`, `ON DELETE CASCADE`), `task_run_id` (nullable, no FK, as `task_logs.task_run_id`), `uri`, `size`, `created_at`; unique `(workspace_id, uri)`. The same migration adds `workflow.executions.replayed_from_execution_id` (composite FK with `workspace_id`, no `ON DELETE` action as 0138's `trigger_execution_id`, never the row itself, never beside `parent_execution_id`), written by the manual replay route.

**A file exists for a flow only once its index row exists.** Every write is: stream to the store, `head` for the real size, insert the index row. `fileExists`, `fileSize` and `isFileEmpty` answer from the index; every read looks the index up first.

### 4.3 When a file is committed

Task files are uploaded before the outcome commits, and their index rows are inserted **in the transaction that commits the task run's outcome**, behind W4's lease fence. A fenced or crashed worker leaves only unindexed objects (§10). Because names are deterministic per task run, a re-claim of the same task run writes the same keys and overwrites its own orphans. Input files commit with the execution row (§7.1).

### 4.4 Who may read a URI

One function, `storage::resolve_readable_many(conn, workspace, readable, uris)`, decides; `resolve_readable(workspace, current_execution, uri)` is its one-element form. The file functions, staging (§5), the plugin bridge (§9.1), FILE inputs given as JSON (§7.1) and the file API (§8) all reach it. The **readable set** of an execution, in its own workspace, is:
1. the running execution `E` and its loop root `R` (the walk of §3);
2. every loop iteration under `R`, at any depth (every row whose walk of §3 reaches `R`) — so a task after the loop, a sibling iteration and a nested iteration read each other's files (the user's ruling, header). Items 1 and 2 are `E`'s **loop family**;
3. the replay chain from `R`: the source of each replay, transitively — a retry replay's `parent_execution_id` (`loop_index` NULL) or a manual replay's `replayed_from_execution_id`; and
4. the `trigger_execution_id` of `E`, of `R` and of every execution in 3 (one level each, Kestra's `trigger` rule; a replay copies its source's inputs and, in Kestra, its trigger); and
5. the **loop family of each execution in 4** — its loop root and every iteration under that root (the user's ruling of 2026-10-01 on Task 7's concern 1). A `Subflow` inside a `Loop` iteration names the iteration as its child's trigger, so without this the child could not read `{{ inputs.f }}` (a FILE input of the loop root) or a sibling iteration's file, which Kestra OSS allows. Still one trigger level: the trigger's own trigger, and the loop families of the replay chain's links, are not in the set.

What Kestra OSS allows here (Task 1): an iteration reading its root's file and an iteration reading its own file were **measured** readable (`file_functions.loop_root_from_iteration`, `file_functions_extra.iteration_own_write`). A task after the loop, or a sibling, reading an iteration's file was **not measured**; by the code it is readable in OSS through the namespace fallback (`AbstractFileFunction.java:137-171`, `isAllowedNamespace` always true in OSS), so item 2 matches OSS there. The replay chain's own loop iterations are not in the set.

A URI whose `{exec}` is outside the set is refused **without reading any row of that execution**, so the answer never says whether the file exists. A URI in the set with no row is *missing* (what `fileExists` answers is Task 1's measurement). Anything else unreadable is an error naming the URI, never `false` or empty.

**The snapshot (the user's ruling, header).** A render runs on its own thread and cannot query the database: inside `settle` the only connection is the transaction's, and a pool would not see the index rows that transaction has just inserted for the task that finished. So before every render with an execution, egret collects every `kestra://` string in the flow source and in every JSON value of the render context (inputs, outputs of every terminated task, execution, loop item) and resolves them at once on the connection that renders. The four functions, staging and the plugin bridge answer from that snapshot. **A URI assembled at render that is not in the snapshot is an error naming it; there is no second lookup.**

### 4.5 Caps

`workflow.storage.max_file_bytes` (default 1 GiB) per file and `workflow.storage.max_task_bytes` (default 5 GiB) per task run. Crossing either fails the task, naming the cap.

### 4.6 Where the object store lives

The object store is **process-wide**: `egret-bin` installs one `FileStore` (the `S3Store` it already builds for query results, the `[storage]` bucket, `workflow.storage.prefix`, the caps) once at start, and `storage::current()` returns it. `read()` runs inside `settle`, which is reached from about eight entry points (the worker, a start, `resume`, the scheduler, the Flow-trigger processor, the webhook, every spawn), none of which holds a store handle. **Index lookups never go through it**: they run on the caller's connection (§4.4). Every function that decides anything takes the store as a parameter; only the wiring reads `current()`. With no store installed, the four functions, FILE inputs and `outputFiles` fail naming `[storage]`, and `container_runner` other than `none` requires `[storage]` at boot.

## 5. Moving files in and out of a pod

W5a's transfer becomes streamed: `get_stream` feeds a streaming tar writer into `docker cp` / exec `tar -x`; `tar -c` is read entry by entry and each entry streams to `put_stream`. The in-memory packer is replaced, not kept beside.

**In.**
- **Script tasks:** an `inputFiles` value that is a `kestra://` URI is staged under that file name, as Kestra does; any other value stays contents, as in W5a.
- **Plugin tasks:** egret stages every rendered property value that is **a whole `kestra://` URI** (a string leaf, not a substring: a URI quoted inside SQL text is not a file read) at `.egret/storage/<n>`; `task.json` gains `storage: {uri → path}`, `baseUri` (the task run's URI prefix), `maxTaskBytes` and the storage context (namespace, flow, the writing execution, task, task run).
- **A URI the snapshot refuses fails the task at render, before any container exists**, naming the URI.

**Out.**
- **Script tasks:** `outputFiles` keeps W5a's rule — literal names and non-recursive globs (`*`, `?` in the file name, within one directory); `**`, `[...]` and a wildcard directory are refused by name — and `outputDirectory: true` is supported; matches are streamed to `tasks/{taskId}/{taskRunId}/{Kestra name}` and `outputs.outputFiles.<relative path>` is the URI.
- **Plugins:** the host's `putFile` writes under `.egret/out/`, computes the URI from `baseUri` and returns it in the plugin's outputs; egret collects `.egret/out/` and indexes exactly those URIs.
- After collection, **every URI under this task run's prefix that appears in the outputs must have been collected**, or the task fails: no output points at nothing.

**Re-attach (W5a):** collection runs as usual on the adopted pod; nothing is indexed before the commit, so nothing is duplicated.

## 6. Script tasks

### 6.1 `inputFiles` URIs

As §5. The W5a contents form is unchanged.

### 6.2 `outputFiles` becomes URIs — breaking

`outputs.<task>.outputFiles` becomes `{relative path → kestra:// URI}`. The 1 MiB and UTF-8 limits of W5a are gone; content is read with `{{ read(outputs.t.outputFiles['a.txt']) }}`. **Breaking:** a flow that used the W5a contents form now receives a URI. Released behind `workflow_containers` (off by default); the release note names it.

## 7. FILE inputs and the file functions

### 7.1 FILE inputs

- The start route accepts `multipart/form-data`: each part is named by an input id, a file part is that FILE input's value (Kestra's shape). JSON stays.
- egret generates the execution id, streams each file to `inputs/{input}/{file}`, and inserts the execution row and the index rows in one transaction.
- `allowedFileExtensions` is enforced; the route's body limit follows `max_file_bytes`.
- In JSON, a FILE input may only be a `kestra://` URI readable (§4.4) from the execution being created: the `Subflow` and `Flow`-trigger cases (and a replay's or loop iteration's copied inputs). Any other string is refused, naming the input. The check runs at every place an execution is created (the start, a `Subflow` child, a loop iteration, a replay), after the row exists and in its transaction.

### 7.2 The functions

| function | behaviour |
|---|---|
| `read(uri)` | the contents as UTF-8; non-UTF-8 is an error naming the URI (chosen: Kestra replaces invalid bytes with U+FFFD, measured by Task 1); capped by `max_read_bytes` |
| `fileSize(uri)` | from the index |
| `fileExists(uri)` | from the index. Measured on Kestra (Task 1): a well-formed URI of the running execution that names no file answers `false`; a URI that does not parse, holds `..`, or has another scheme is an error. egret follows: *missing* (in the readable set, no row) is `false`; a URI outside the readable set is an error (chosen: Kestra OSS reads it, §2) |
| `isFileEmpty(uri)` | from the index (`size == 0`) |

- All four answer from the snapshot (§4.4). `fileURI` stays refused, now naming W9b; `kv` names W9c.
- They exist in every render with an execution (task properties, `runIf`, `Log`, outputs); a render with none (a trigger's conditions, publish) refuses them by name.
- Rendering is synchronous and already on its own thread (`render_on_this_thread`, and `bind_on_this_thread` for `egret.sql`): the functions answer from the snapshot taken before the render; **a URI assembled at render is an error naming it** (the user's ruling); `read` blocks on the runtime handle for one capped read, so a render that reads a file needs the multi-thread runtime (production's).

## 8. The execution file API

| route | answer |
|---|---|
| `GET /api/ws/{ws}/workflows/executions/{id}/file?path=kestra:///…` | the raw bytes, streamed |
| `GET …/executions/{id}/file/metas?path=…` | `{size}` from the index |
| `GET …/executions/{id}/file/preview?path=…&maxRows=100` | up to `maxRows` (max 5000): `.ion` as JSON rows (read with `ion-rs` 1.0.1 as a **reader**, one top-level value at a time, text or binary — Kestra's STORE files are binary Ion with no lines (Task 1); its writer's format is irrelevant here, and `ionfmt::read_ion` refuses the timestamps and blobs that plugin STORE files hold), `.csv`/`.jsonl`/`.json`/text as lines, else `{previewable: false}`; stops reading once full |

- Permission `workflow:read`; `path` must be an index row whose execution is in the **loop family of `{id}`** — `loop_root({id})` and every loop iteration under it (§4.4 items 1–2) — in workspace `{ws}`, else 404, no redirect. Of the two candidates, `{id}` itself only or its whole loop family, this is the one consistent with the readable set: every task of the family reads every file of the family, so any of its ids serves them (Kestra reaches the same files by a 301 to the owning iteration, Task 1). Trigger and replay links stay out: a viewer asks those executions by their own id. The check is `resolve_readable_many` with the readable set narrowed to that family.
- Downloads are `application/octet-stream`, `Content-Disposition: attachment` with a sanitised name, `X-Content-Type-Options: nosniff`.
- The preview answers `{previewable, rows, truncated}` (controller ruling on Task 8). `truncated` is Kestra's flag: more rows follow than `maxRows`, or `workflow.storage.max_read_bytes` ended the read. A row is any JSON value (a line's string; an Ion value as a number, string, null, array or object), so its schema is any value.
- The Ion reader is depth-safe (Task 8 fix round 1). Every byte passes a depth scan, of text or binary Ion, before the reader sees it. The text scan's safety is structural (fix round 3). It skips only regions it can prove are data: quoted strings, quoted symbols, long strings and lobs `{{…}}` (a lob is not a level; its base64 is data, and its clob strings keep their string rules). Everything else counts its opening brackets, **the inside of a `//` or `/*…*/` comment included**. A closing bracket inside a comment does not count. So however `ion-rs` reads a comment, the count is never behind it: `ion-rs` 1.0.1, for example, reads a `/*` with no `*/` as an s-expression operator and parses what follows. A shallow file with many brackets in comments can therefore be refused; Kestra writes no comments. A comment ends where `ion-rs` ends one: a line comment at `\n` or `\r`, a block comment at its first `*/`. A `/` directly after an operator character is part of an s-expression operator, not a comment start. The binary scan refuses a length that runs past its container. A container nested deeper than **100** levels is never handed to the reader, and the file is `previewable: false`. The reader recurses once per level, and before this fix one file nested 1000 deep aborted the process. The reader runs on its own thread with a 16 MiB stack. Each fill of its buffer is filled whole, so a value of `n` bytes is re-parsed about log₂(n) times, not once per store chunk. `storage::ion::read_values` is the only way into it.
- utoipa-documented; the frontend client is regenerated with `npm run gen:api`. The UI is W7's.

## 9. Consumers

### 9.1 The plugin storage bridge

In the host, `EgretStorage implements StorageInterface`, wrapped in Kestra's public `InternalStorage`. It reads only `task.json`'s `storage` map and `baseUri`, holds no credential and opens no connection.

| methods | behaviour |
|---|---|
| `get`, `getWithMetadata`, `exists`, `getAttributes` | staged files, and files this task wrote; else `FileNotFoundException` naming the URI |
| `put` | only under `baseUri`; writes `.egret/out/{relative}`; counts bytes against `max_task_bytes` (passed in `task.json`) and fails in the pod |
| `list`, `allByPrefix` | staged and written files, filtered by prefix |
| `delete`, `deleteByPrefix`, `move`, `createDirectory`, `purgeByLastModified` | only this task's own written files; anything else throws `UnsupportedOperationException` with a reason |
| `*InstanceResource` | throw, with a reason |

The catalog re-admits `postgresql.CopyIn`, `postgresql.CopyOut`, `postgresql.Batch`, `mysql.Batch`, and publish stops refusing `fetchType: STORE`. Every property Task 1 of W5b measured as needing storage is re-measured and un-refused one by one. Each W5b test asserting "refused, needs storage" flips, and each flip has a replacement test proving the plugin runs. Files a plugin writes are not masked, as outputs are not (W5b §12).

### 9.2 `egret.sql` with `fetch: STORE`

`Fetch` gains `Store` (`egret.sql`'s own key is `fetch:`). The handler runs in egret, so it writes rows through `runners::ionfmt` into `put_stream` at `tasks/{taskId}/{taskRunId}/{name}.ion`, and its outputs are `{uri, size}` with `size` the row count — the shape of Kestra's `AbstractJdbcQuery` STORE outputs (Task 1 pins it). Task 1 measured Kestra's name as random per run (`{random unsigned decimal}.ion`), so `{name}` is a fixed `result.ion` (§4.3 needs a re-claim to overwrite its own object). Kestra's STORE file is **binary** Ion 1.0, each row with its own version marker and symbol table, a timestamp as an annotated string (`LocalDateTime::"…"`); `ionfmt` writes Ion text, so the bytes cannot match (the fixture's `ion_rows` records Kestra's value per SQL type). **Memory:** `QueryExecPort::execute_as_with_params` returns a whole `TabularResult`, so nothing streams through the port whatever the connector does (Task 1 records each connector's internal behaviour); STORE is capped by `Sql::max_rows` (`MAX_FETCH_ROWS`, 10 000) and fails at the cap exactly as `FETCH` does. **`egret.sql` is in no production registry** (§1): W9a tests the handler directly and on live StarRocks, and does not wire it.

### 9.3 Core storage tasks and `Loop` over a file

Native handlers for `io.kestra.plugin.core.storage.{Write, Concat, Split, Size, Delete, Reverse, DeduplicateItems, FilterItems, PurgeCurrentExecutionFiles}`, each following its Kestra class's properties and outputs (Task 1 extracts them). `Delete` and `PurgeCurrentExecutionFiles` delete only files of the running execution's loop family (its loop root and every iteration under it, §4.4), index row first; a trigger's or replay source's file is readable, never deletable (Kestra OSS `Delete` removed another execution's file, Task 1). Which prefix Kestra's `PurgeCurrentExecutionFiles` empties when run inside an iteration was not measured. `PurgeStorage` is out of scope (an administrative task across executions).

`io.kestra.plugin.core.flow.Loop` (the programme's `ForEach`) with `values` a `kestra://` URI: **the worker** resolves the URI through the snapshot and reads the file (ION with `ion-rs`, one top-level value at a time, text or binary Ion — a plugin's STORE file is binary, Task 1 — each coalesced as Kestra's `FlowableUtils.ionValueToString`: a string as itself, a number's text, `null` an error, anything else JSON) **before** it calls `advance()`, and puts the JSON array in the run's `rendered`; `advance()` stays pure. **There is no loop-width limit in the code** to reuse, so the file is capped by `workflow.storage.max_read_bytes`, the same cap as `read()`. The corpus flows `valids/loop-values-from-uri` and `valids/loop-outputs-auto` are the target.

## 10. Retention and orphans

- `workflow.storage_retention_days`, default 30; 0 keeps forever.
- An hourly pruner with a per-pass budget, modelled on W11's, takes executions **ended** more than N days ago that still have index rows, calls `delete_prefix`, then deletes the index rows. **A loop family is pruned as one, on its loop root's end** (the user's ruling of 2026-10-01 puts an iteration's files under the iteration's own id, §3): an iteration that ended while its root still runs keeps its files, so a task after the loop can still read them; when the root has ended more than N days ago, the root and every iteration under it are pruned together. If the store delete fails, the rows stay and the next pass retries. **An execution that has not ended is never pruned.**
- **Orphans** (uploaded, never committed): for each pruned execution and each ended FAILED/KILLED execution, list its prefix and delete objects that have no index row and are older than 24 hours.
- A URI whose file was pruned is an error naming the URI and the retention window.

## 11. Errors

Every storage error names the URI and never a secret value. An unreachable store at run is an infrastructure error: the worker returns `Err`, the lease lapses, the run is re-claimed, and no `attempts` is spent (W4). A crossed cap fails the task, naming the cap.

## 12. Tasks

Fourteen tasks, in this order; the plan (`docs/en/plans/2026-10-01-workflow-w9a-internal-storage.md`) gives each one's files, interfaces, tests and mutations, and the reason for each split of the first draft's ten.

1. **Measure Kestra** (`oracle-exec/w9a-storage.json`): slugify; `IdUtils.from`; output file names; `outputDirectory`; the file functions on own, trigger, unrelated, missing and malformed URIs; FILE inputs; the file API; plugin storage; STORE outputs and ION per type; the core storage tasks; `Loop` over an ION URI; egret's connectors' row streaming. Plus the baselines (tests, ratchet, clippy).
2. **`egret-storage` streaming** (`put_stream`, `get_stream`, `head`, `delete_prefix`; a `MemStore` for tests).
3. **The index, URIs, `resolve_readable_many`, the process-wide `FileStore`** (migration 0161: `workflow.execution_files`, `executions.replayed_from_execution_id`).
4. **The file functions on the pre-render snapshot.**
5. **Streamed transfer into a container; `inputFiles` URIs.**
6. **Collection out; `outputFiles` as URIs; indexing at commit.**
7. **FILE inputs.**
8. **The execution file API.**
9. **The plugin storage bridge and catalog.**
10. **`egret.sql` STORE.**
11. **`Write`, `Size`, `Delete`, `PurgeCurrentExecutionFiles`; `Loop` over a file.**
12. **`Concat`, `Split`, `Reverse`, `DeduplicateItems`, `FilterItems`.**
13. **The pruner** (migration 0162: `files_pruned_at`, `files_swept_at`).
14. **End to end on a temporary kind cluster; coverage; docs; full local CI.**

## 13. Testing

- Unit: URI ↔ key, slugify, `IdUtils.from`, `resolve_readable_many` (own, loop root, loop iteration and sibling iteration, trigger, retry replay, manual replay chain, other execution, other workspace), the snapshot (a URI assembled at render is an error), the function table.
- MinIO: streaming beyond one part, memory measured flat.
- Live Docker and Kubernetes: `outputFiles` globs, `inputFiles` URIs, streamed tar.
- Live plugins: `Query` STORE → `CopyIn` of that URI → `read()`.
- API: 404 for another execution's and another workspace's file; the download headers; ION preview.
- Pruner: an execution that has not ended is kept; orphans deleted; a failed store delete retried.
- Oracle: the ratchet is **measured** before and after and recorded in `oracle-exec/README.md`; the corpus holds 5 FILE-input flows, 1 `outputFiles`, 2 `STORE`, 2 `core.storage`.
- End to end on a temporary kind cluster, as W5b.

## 14. Out of scope

Namespace files, `nsfile://` and `fileURI` (W9b); the KV store (W9c); `PurgeStorage` and `PurgeExecutions`; `file://`; multipart webhook bodies; log offload to object storage (§11 open question 1); a local-disk backend.

## 15. Known limits

- A flow reads only its own execution's, its loop family's (its loop root and every iteration under it) and its trigger execution's files and that trigger's loop family's, and a replay its source chain's (§2, §4.4). One trigger level only: a file of the trigger's own trigger is refused (2026-10-01 ruling).
- A `Subflow` or `Flow` trigger fired from a **manual replay** that passes the copied `{{ inputs.f }}` is refused: that URI names the replay's source, and the child's readable set follows its trigger's loop family but not its trigger's replay chain (the 2026-10-01 ruling covers the loop family only, §4.4 item 5).
- A file written inside a `Loop` iteration carries the iteration's id, as in Kestra (§3, the user's ruling); the file API serves it through any id of the loop family, with no redirect (§8).
- `allowedFileExtensions` is enforced; the Kestra artifact does not enforce it (Task 1), so such a flow passes on Kestra and fails on egret (§2).
- Kestra's plugins write binary Ion; every egret reader must accept it. `egret.sql` STORE writes Ion text, which Kestra's readers accept as measured for `Write` output; `ionfmt`'s own bytes were not measured on Kestra (§2, §9.2).
- A URI assembled at render by string concatenation is an error; a URI must appear in the flow source or reach the render through inputs or outputs (§4.4, the user's ruling).
- A plugin reads only URIs that are a whole property value in its rendered properties (§2, §5).
- `Loop` over a file reads it whole, capped by `max_read_bytes` (§9.3).
- An output file's name follows Kestra's scheme; its hash differs from Kestra's (§3).
- Files are pruned 30 days after the execution ends by default — for a `Loop` iteration, after its loop root ends (§10); Kestra keeps them.
- `outputFiles` changed from contents to URIs (§6.2).
- `egret.sql` STORE holds the whole result in memory, capped at 10 000 rows, because the port returns a whole `TabularResult`; and `egret.sql` is not wired into any production registry (§9.2).
- Every deployment that runs workflow containers needs a reachable S3 or MinIO bucket.
