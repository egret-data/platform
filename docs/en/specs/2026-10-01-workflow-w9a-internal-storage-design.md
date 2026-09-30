# W9a — Internal storage: `kestra://` files for executions, tasks, plugins and the file functions

**Status:** design, awaiting review
**Depends on:** W4 (lease fence, `apply_outcome`), W5a (task runners, input/output file transfer), W5b (plugin host, catalog, `task.json`), W6/W13 (`trigger_execution_id`), W10 (execution routes), W11 (the pruner pattern) — all merged. Base `ecdf4868`.
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 finding 4 (`StorageInterface`), §9 (row W9), §11 open question 1
**Decided with the user, 2026-10-01:**
- **W9 is split into three packages:** W9a internal storage (this spec), then W9b namespace files, then W9c the KV store.
- **egret moves files for the pod.** A task pod holds no storage credential and opens no connection to the object store. egret stages every file a task may read into its working directory and collects every file it writes, through W5a's existing transfer (§5).
- **Streamed, with large configurable caps.** `egret-storage` gains streaming; no file is ever held whole in egret's memory (§4.1).
- **A running flow reads its own execution's files and its trigger execution's**, never another workspace's, and never an unrelated execution's in the same workspace (§4.4). This is narrower than Kestra OSS on purpose (§2).
- **Retention of its own, 30 days by default**, by a pruner modelled on W11's (§10).
- **`outputFiles` becomes a map of name → `kestra://` URI, as in Kestra.** This breaks egret flows that read W5a's name → contents map (§6.2).
- **In scope:** FILE inputs and the file functions (§7), the execution file API (§8), the storage bridge for Kestra plugins (§9.1), `egret.sql` with `fetchType: STORE` (§9.2), and the core storage tasks with `ForEach` over a file (§9.3).

**Delivery:** one PR (docs and code), tasks in §12.

## 1. What exists and what does not

| piece | state on `origin/main` (`ecdf4868`) |
|---|---|
| object store | `egret-storage`: trait `ObjectStore { put, get, delete, copy, list }` on whole `Vec<u8>` buffers, `s3://bucket/key` URIs; one backend, `S3Store` (S3 and MinIO); configured by `[storage]` (endpoint, region, bucket `warehouse`, keys, `path_style`). `egret-workflow` has it as a dev-dependency only |
| file functions | `runners::functions::REFUSED` refuses `read`, `fileURI`, `fileSize`, `fileExists`, `isFileEmpty` with "…, W9" (`functions.rs:245-263`) |
| FILE inputs | `InputType::File` exists (`models/input.rs`); the start route accepts only a JSON object; multipart webhook bodies are refused |
| `outputFiles` | a map of name → **contents** (UTF-8, ≤ 1 MiB each, `MAX_OUTPUT_FILE_BYTES`), documented as a departure from Kestra (`handlers/container.rs:46-58`) |
| file transfer | W5a: Docker `docker cp` of a staging directory; Kubernetes exec `tar -x` in and `tar -c` out, with an **in-memory** ustar packer (`worker/k8s/transfer.rs`) |
| plugins | W5b's catalog leaves out `postgresql.CopyIn`, `postgresql.CopyOut`, `postgresql.Batch`, `mysql.Batch` (`EgretPluginHost.NEEDS_STORAGE`), and refuses `fetchType: STORE` at publish (`plugins/mod.rs`) |
| `egret.sql` | `Fetch` leaves `STORE` out on purpose (`worker/handlers/sql.rs:143`) |
| `ForEach` over a file | a named hole in `advance()` (`executor/flowable/loop_.rs:171`): a rendered `kestra:///….ion` arrives as a plain string |
| ION | `runners::ionfmt` writes ION exactly as Kestra's `FileSerde` does; `ion-rs` is excluded as a writer (`Cargo.toml:173`) |
| execution links | `trigger_execution_id` (0138, workspace-scoped composite FK): set for a `Subflow` child and a `Flow`-trigger-fired execution. `parent_execution_id` (0135): a loop iteration's sub-execution |
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
| a flow may read any execution's task file in OSS (`isAllowedNamespace` is always true) | its own execution, its loop root, and its trigger execution only | egret has a reader boundary (`workflow:read`) Kestra OSS does not; a flow author with `workflow:manage` must not read outputs of a flow they cannot see. **A Kestra flow that hands a URI to an unrelated flow runs on Kestra and fails on egret** |
| a plugin reads any URI it computes at run | a plugin reads the URIs present in its rendered properties | the pod holds no credential (§5). JDBC plugins are unaffected; a future plugin that computes URIs at run would fail, by name |
| files are kept forever | pruned 30 days after the execution ends (configurable, 0 = forever) | the user's choice (§10). A flow that reads a file of an execution older than the window fails, naming the URI and the window |
| `read()` has no cap | `workflow.storage.max_read_bytes`, default 10 MiB | the result is spliced into a rendered string |
| the file API redirects to the owning execution | 404 | a viewer asks the owning execution by its own id |
| `file://`, multipart webhook bodies, `nsfile://` | refused | out of scope; `nsfile://` and `fileURI` are W9b |

## 3. Storage layout and URIs

- **Object key:** `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/{flow}/executions/{exec}/{tail}` in the `[storage]` bucket. `workflow.storage.prefix` defaults to `workflow/`.
- **URI a flow sees:** `kestra:///{ns-as-path}/{flow}/executions/{exec}/{tail}` — Kestra's form. **It carries no workspace**: egret maps a URI to a key by prefixing the workspace **of the running execution**, never a value read from the URI, so nothing in a URI can name another workspace.
- `tail` is Kestra's: `inputs/{input}/{file}`, `tasks/{taskId}/{taskRunId}/{file}`, `trigger/{triggerId}/{file}`. Slugify and the output file name follow Kestra exactly; Task 1 measures both on Kestra and pins them in a fixture (§11).
- **`{exec}` is the loop root.** egret runs a loop iteration as a sub-execution (`parent_execution_id`); Kestra runs it inside the one execution. A file written inside an iteration therefore uses the id at the top of the `parent_execution_id` chain, so its URI is the one Kestra would give.
- A URI is refused if its scheme is not `kestra`, if it holds `..`, or if it does not parse into the layout above.

## 4. Core

### 4.1 Streaming in `egret-storage`

`ObjectStore` keeps its `Vec<u8>` methods for today's callers and gains `put_stream(uri, AsyncRead, size_hint)` (a single PUT under 8 MiB, else a multipart upload in 8 MiB parts, aborted on error), `get_stream(uri) -> AsyncRead`, `head(uri) -> Option<size>` and `delete_prefix(uri)`. Tested against a real MinIO with a file larger than one part, measuring that the process's resident memory does not grow with the file.

### 4.2 The index

`workflow.execution_files`: `workspace_id`, `execution_id` (the loop root; composite FK with `workspace_id`, `ON DELETE CASCADE`), `task_run_id` (nullable), `uri`, `size`, `created_at`; unique `(workspace_id, uri)`.

**A file exists for a flow only once its index row exists.** Every write is: stream to the store, `head` for the real size, insert the index row. `fileExists`, `fileSize` and `isFileEmpty` answer from the index; every read looks the index up first.

### 4.3 When a file is committed

Task files are uploaded before the outcome commits, and their index rows are inserted **in the transaction that commits the task run's outcome**, behind W4's lease fence. A fenced or crashed worker leaves only unindexed objects (§10). Because names are deterministic per task run, a re-claim of the same task run writes the same keys and overwrites its own orphans. Input files commit with the execution row (§7.1).

### 4.4 Who may read a URI

One function, `storage::resolve_readable(workspace, current_execution, uri) -> Result<IndexedFile>`, used by the file functions, staging (§5) and the plugin bridge (§9.1). It allows a URI whose index row belongs, in the running execution's workspace, to:
1. the running execution's loop root; or
2. the loop root's `trigger_execution_id` (one level, Kestra's `trigger` rule); or
3. for a replay, the execution it was replayed from — Task 2 measures how a W6 replay records its source and adds the link if none exists.

Anything else is an error naming the URI, never `false` or empty.

### 4.5 Caps

`workflow.storage.max_file_bytes` (default 1 GiB) per file and `workflow.storage.max_task_bytes` (default 5 GiB) per task run. Crossing either fails the task, naming the cap.

## 5. Moving files in and out of a pod

W5a's transfer becomes streamed: `get_stream` feeds a streaming tar writer into `docker cp` / exec `tar -x`; `tar -c` is read entry by entry and each entry streams to `put_stream`. The in-memory packer is replaced, not kept beside.

**In.**
- **Script tasks:** an `inputFiles` value that is a `kestra://` URI is staged under that file name, as Kestra does; any other value stays contents, as in W5a.
- **Plugin tasks:** egret scans the rendered `task.json` for `kestra://` strings and stages each at `.egret/storage/<n>`; `task.json` gains `storage: {uri → path}` and `baseUri` (the task run's URI prefix).
- **A URI that `resolve_readable` refuses fails the task at render, before any container exists**, naming the URI.

**Out.**
- **Script tasks:** `outputFiles` accepts globs, as Kestra does, and `outputDirectory: true` is supported; matches are streamed to `tasks/{taskId}/{taskRunId}/{Kestra name}` and `outputs.outputFiles.<relative path>` is the URI.
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
- In JSON, a FILE input may only be a `kestra://` URI readable (§4.4) from the execution being created: the `Subflow` and `Flow`-trigger cases. Any other string is refused, naming the input.

### 7.2 The functions

| function | behaviour |
|---|---|
| `read(uri)` | the contents as UTF-8; non-UTF-8 is an error naming the URI; capped by `max_read_bytes` |
| `fileSize(uri)` | from the index |
| `fileExists(uri)` | from the index; an unreadable URI is an error (Task 1 measures Kestra's answer and the spec follows it) |
| `isFileEmpty(uri)` | from the index (`size == 0`) |

- All four go through `resolve_readable`. `fileURI` stays refused, now naming W9b.
- They exist in every render with an execution (task properties, `runIf`, `Log`, outputs); a render with none (a trigger's conditions, publish) refuses them by name.
- Rendering is synchronous and already on its own thread (`render_on_this_thread`): index lookups are prefetched for every `kestra://` literal in the text and looked up again for one assembled at render; `read` blocks on the runtime handle for one capped read.

## 8. The execution file API

| route | answer |
|---|---|
| `GET /api/ws/{ws}/workflows/executions/{id}/file?path=kestra:///…` | the raw bytes, streamed |
| `GET …/executions/{id}/file/metas?path=…` | `{size}` from the index |
| `GET …/executions/{id}/file/preview?path=…&maxRows=100` | up to `maxRows` (max 5000): `.ion` as JSON rows (read with `ion-rs`, whose reader is not affected by its writer's format), `.csv`/`.jsonl`/`.json`/text as lines, else `{previewable: false}`; stops reading once full |

- Permission `workflow:read`; `path` must be an index row **of execution `{id}`** in workspace `{ws}` (its loop root), else 404 — no redirect.
- Downloads are `application/octet-stream`, `Content-Disposition: attachment` with a sanitised name, `X-Content-Type-Options: nosniff`.
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

### 9.2 `egret.sql` with `fetchType: STORE`

`Fetch` gains `Store`. The handler runs in egret, so it streams rows through `runners::ionfmt` into `put_stream` at `tasks/{taskId}/{taskRunId}/{name}.ion`, and its outputs are `{uri, size}` with `size` the row count — the shape of Kestra's `AbstractJdbcQuery` STORE outputs (Task 1 pins it). **Memory:** STORE only bounds memory if the connector yields rows as a stream; Task 1 measures Trino and StarRocks and the spec records which do. A connector that buffers still produces the right output shape, under its existing row limit.

### 9.3 Core storage tasks and `ForEach` over a file

Native handlers for `io.kestra.plugin.core.storage.{Write, Concat, Split, Size, Delete, Reverse, DeduplicateItems, FilterItems, PurgeCurrentExecutionFiles}`, each following its Kestra class's properties and outputs (Task 1 extracts them). `Delete` and `PurgeCurrentExecutionFiles` delete only files of the running execution's loop root, index row first. `PurgeStorage` is out of scope (an administrative task across executions).

`ForEach` with `values` a `kestra://` URI: **the worker** resolves and reads the file (ION, one value per record, as Kestra) **before** it calls `advance()`, and hands `advance()` the list; `advance()` stays pure. The list is capped by the existing loop-width limit. The corpus flows `valids/loop-values-from-uri` and `valids/loop-outputs-auto` are the target.

## 10. Retention and orphans

- `workflow.storage_retention_days`, default 30; 0 keeps forever.
- An hourly pruner with a per-pass budget, modelled on W11's, takes executions **ended** more than N days ago that still have index rows, calls `delete_prefix`, then deletes the index rows. If the store delete fails, the rows stay and the next pass retries. **An execution that has not ended is never pruned.**
- **Orphans** (uploaded, never committed): for each pruned execution and each ended FAILED/KILLED execution, list its prefix and delete objects that have no index row and are older than 24 hours.
- A URI whose file was pruned is an error naming the URI and the retention window.

## 11. Errors

Every storage error names the URI and never a secret value. An unreachable store at run is an infrastructure error: the worker returns `Err`, the lease lapses, the run is re-claimed, and no `attempts` is spent (W4). A crossed cap fails the task, naming the cap.

## 12. Tasks

1. **Measure Kestra** (fixture under `oracle-exec/`): slugify; output file names; `fileExists` on an unreadable URI; STORE outputs; the core storage tasks' properties and outputs; `ForEach` over an ION URI; Trino and StarRocks row streaming in egret's connectors.
2. **`egret-storage` streaming; the index; URIs; `resolve_readable`** (migration: `workflow.execution_files`, and the replay link if missing).
3. **Streamed pod transfer; `inputFiles` URIs; `outputFiles` as URIs.**
4. **FILE inputs; the file functions.**
5. **The execution file API.**
6. **The plugin storage bridge and catalog.**
7. **`egret.sql` STORE.**
8. **Core storage tasks and `ForEach` over a file.**
9. **The pruner.**
10. **End to end on a temporary kind cluster; coverage; docs; full local CI.**

## 13. Testing

- Unit: URI ↔ key, slugify, `resolve_readable` (own, loop root, trigger, replay, other execution, other workspace), the function table.
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

- A flow reads only its own execution's, its loop root's and its trigger execution's files (§2).
- A plugin reads only URIs present in its rendered properties (§2).
- Files are pruned 30 days after the execution ends by default; Kestra keeps them.
- `outputFiles` changed from contents to URIs (§6.2).
- STORE bounds memory only on connectors that stream rows (§9.2).
- Every deployment that runs workflow containers needs a reachable S3 or MinIO bucket.
