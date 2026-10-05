# W9b — Namespace files: `nsfile://`, `fileURI`, `namespaceFiles`, the `core.namespace` tasks and the namespace file API

**Status:** design, 2026-10-05; not yet implemented
**Depends on:** W9a (internal storage: `egret-storage` streaming, the process-wide `FileStore`, `storage::uri`, the pre-render file scope, streamed transfer, the plugin storage bridge, the bounded multipart parser, the file API's safe download headers, the pruner) — merged. W5a (script tasks, `inputFiles`), W5b (plugin host, `task.json`) — merged. Base `e539d8b3`.
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 finding 4 (`StorageInterface`), §9 (row W9); W9a spec `docs/{en,vi}/specs/2026-10-01-workflow-w9a-internal-storage-design.md` §14 (namespace files deferred to W9b)
**Decided with the user, 2026-10-05:**
- **Namespace files keep revisions, as Kestra does.** Revision 1 lives at `path`, revision N > 1 at `path.vN`; a delete is soft; `PurgeFiles` hard-purges (§3, §4, §8.4).
- **A running flow reads namespace files of any namespace in its workspace, and writes only to its own namespace.** Writes are `UploadFiles`, `DeleteFiles`, `PurgeFiles` (own namespace and its child namespaces) and a plugin's `putFile`. The workspace is the hard boundary, taken from the running execution and never from a URI (§5).
- **API permissions:** reading (download, list, search, stats, revisions, export) needs `workflow:read`; writing (upload, create directory, move, delete, purge) needs `workflow:manage`. No new permission (§9).
- **In scope:** the store and its metadata (`workflow.namespace_files`, objects at `{prefix}{ws}/{ns-as-path}/_files/{path}[.vN]`, `kestra:///{ns-as-path}/_files/…` URIs through a new `Tail::NamespaceFile`, `nsfile://` parsing) (§3, §4), the expression functions (§6), `namespaceFiles` on script tasks, parsed and validated only on `WorkingDirectory` (§7), the four `io.kestra.plugin.core.namespace` tasks as native handlers (§8), the REST API (§9) and the plugin bridge (§10).
- **(Added 2026-10-05, the user's ruling on the spec's `WorkingDirectory` finding.) `WorkingDirectory` stays out of W9b.** W9b runs `namespaceFiles` on script tasks only. On `WorkingDirectory` the block is parsed and validated at publish: a malformed block is refused, naming the property; a valid one is accepted and kept in the model for the later package that runs `WorkingDirectory`. Nothing changes at run, because egret still does not execute `WorkingDirectory` (§1, §7). The two corpus flows stay baselined, and no ratchet change is expected (§13). *(Note 2026-10-05, Task 1 measured; the ruling's text above is unchanged: the two corpus flows are not baselined, they stand **outside the ratchet entirely** — the artifact refused both at publish (422 `Invalid type: io.kestra.core.tasks.test.Read`), so they have no golden entry and no line in `baseline.txt`. The ruling's substance holds: no ratchet change can come from them.)*
- **(Added 2026-10-05, the user's ruling on `inputFiles`.) As in Kestra, namespace files load first and `inputFiles` always overwrite a same-named namespace file, whatever `ifExists` says; `ifExists` governs only collisions between namespaces** (§7). Task 1 still measures it.
- **(Added 2026-10-05, the user's ruling on `_files` namespaces.) A namespace with a `_files` segment is refused** at publish and in every namespace-file operation (API, tasks, functions), with an error naming it. Task 1 counts the existing flows and corpus entries with such a namespace, and its report lists each one (§3).
- **(Added 2026-10-05, the user's ruling on the plan's open item 9.) A script task's `inputFiles` value that is a namespace-file URI (`nsfile:///…`, `nsfile://other.ns/…`, or `kestra:///…/_files/…`, e.g. from `fileURI`) is accepted, as Kestra does:** it is staged into the task's working directory through the same one namespace-file read check (any namespace of the workspace), alongside W9a's execution-file URIs (§7).
- **(Added 2026-10-05, the controller's ruling on the plan's open item 10.) `task.json` carries namespace files in a separate `namespaceFiles` array** (URI, namespace, path, revision, size, staged path); W9a's `storage` map (URI → staged path) is unchanged (§10).
- **(Added 2026-10-05, the user's ruling on the detailed choices.) Approved as egret's choices**, each a row in §2: `PurgeFiles` defaults to the flow's own namespace; child namespaces match on a dot boundary; the API's `DELETE` does not remove parents left empty; export is a streamed, files-only zip; `fileURI` without `revision` is an error on a missing path; reading `kestra:///other/ns/_files/…` is allowed within the workspace.
- **(Added 2026-10-05, the user's ruling R1 in Task 1's fix round 1.) Soft-deleted history stays visible**, unlike Kestra's 404s: the `revisions` API lists every revision of a soft-deleted path, the deleted ones flagged; an explicit-revision read (`GET …/files?path&revision=N`, `read(path, revision)`, `fileURI(path, revision)`) of a revision of a soft-deleted path succeeds; a **latest** read (no revision) of a soft-deleted path is not-found. Rewriting it creates a new revision, as before (§2, §4.3, §4.5, §6, §9).
- **(Added 2026-10-05, the user's ruling R2.) `PurgeFiles.namespacePattern` uses Kestra's wildcard grammar** (`FilenameUtils.wildcardMatch`: `*`, `?`; `w9b.pp.*` does not match `w9b.ppx.c`), restricted to the flow's own namespace and its children (the purge rule, §5) (§2, §8.4).
- **(Added 2026-10-05, the user's ruling R3.) `fileURI` of a directory is an error naming the path; search is as Kestra's (case-insensitive substring, files only); `GET …/files/directory` on a file's path is 400; GET routes never create anything (stats with no path answers a virtual root directory and creates nothing)** (§2, §6, §9).

**Delivery:** one PR (docs and code), ten tasks in §12, implemented by a plan written after this spec.

## 1. What exists and what does not

| piece | state at `e539d8b3` |
|---|---|
| namespace | only a string on a flow, `Flow::namespace` (`crates/egret-workflow/src/models/flow.rs:41`); flows are unique on `(workspace_id, namespace, flow_id)` (`migrations/0132_workflow_flows.sql:30`). There is no namespace table and no per-namespace permission. Publish checks the namespace against `^[a-z0-9][a-z0-9._-]*`, 1-150 chars (`spec/validate.rs:625-636`, `is_valid_namespace` `:662`), so a segment such as `_files` in `a._files` is accepted today |
| flow ids | `^[a-zA-Z0-9][a-zA-Z0-9._-]*`, 1-100 chars (`spec/validate.rs:651-660`, `is_valid_flow_id` `:692`): a flow cannot be named `_files`, so under one namespace `{ns}/_files/…` cannot collide with `{ns}/{flow}/executions/…` (but see §3 for a namespace whose segment is `_files`) |
| internal storage URIs | `storage::uri` parses `kestra:///{ns}/{flow}/executions/{exec}/{tail}` with `Tail::{Input, Task, Trigger, Execution}` (`storage/uri.rs:22-38`); a URI with no `executions/{uuid}` is refused as "not an execution file" (`:112-117`). There is no `_files` tail. `namespace_as_path` is `uri.rs:186` |
| file functions | `read`, `fileSize`, `fileExists`, `isFileEmpty` answer from the pre-render file scope (W9a §4.4). `fileURI` is refused naming W9b (`runners/functions.rs:269`); a bare path or an `nsfile://` URI is refused naming W9b (`functions.rs:356-361`) |
| bounded read on the render thread | `off_the_runtime` (`runners/env.rs:2094-2100`, `block_in_place` on the multi-thread runtime, W9a Task 4 fix round 1 C1); the file scope's outage flag `infrastructure_failure` (`runners/files.rs:59`, `storage/scope.rs:250`, read by `worker/claim.rs:1258`) |
| script `namespaceFiles` | refused at publish for all four script types (`scripts/mod.rs:195-207`, `COMMON_REFUSED`); pinned by `tests/scripts_gate.rs:940-945`; the worker's module doc says it needs W9b (`worker/commands.rs:105-107`) |
| `WorkingDirectory` | its model keeps only `tasks` and silently drops `namespaceFiles`, `inputFiles`, `outputFiles` and `cache` (`models/task.rs:555-565`). **The executor does not run it**: `build_plan` plans it as an opaque `Runnable` (`executor/plan.rs:1230`, `executor/mod.rs:3319-3321`), and the claim answers `None` for it (`worker/claim.rs:258`) |
| plugin host | `ShimRunContext.storage()` builds `InternalStorage` with `NO_NAMESPACE_FILES`, whose `of(…)` throws `EgretUnsupported` naming W9b (`plugin-host/…/ShimRunContext.java:274-280`); `EgretUnsupported` makes the host exit 4 (`EgretUnsupported.java:7`) |
| pruner | W9a's pruner deletes per execution under `FileStore::execution_prefix` only (`storage/prune.rs:553`) |
| migrations | the last is `0168_workflow_execution_files_pruned.sql`; W9b takes **0169** |
| corpus | two flows use `namespaceFiles`, both on `WorkingDirectory` (`oracle-exec/corpus/valids/working-directory-namespace-files.yaml`, `…-with-namespaces.yaml`, the second with three namespaces, `folderPerNamespace: true` and an `exclude`); none uses `nsfile://`, `fileURI` or `core.namespace.*` |

## 2. What Kestra does, and where egret differs

Kestra (checkout `96893b589`, artifact `2.0.0-rc13`):
- **Layout.** Namespace files live under `/{ns-as-path}/_files` (`StorageContext.java:41`, `namespaceFilePrefix` `:324-326`). Revision 1 is stored at `path`, revision N > 1 at `path.vN`; a directory is always revision 1 (`NamespaceFile.java:138-148`). The metadata repository gives a write `previous + 1` (a directory stays 1) (`AbstractJdbcNamespaceFileMetadataRepository.java:191-199`), reading `previous` through `findByPath`, which runs its own transaction (`:69-80`). A soft delete marks the latest row deleted without a new revision (`:203-205`).
- **`Namespace`** (`core/…/storages/Namespace.java`): `all`, `children`, `move`, `get`, `findAllFilesMatching(includes, excludes)`, `getFileContent(path, revision?)`, `getFileMetadata`, `exists`, `putFile(…, Conflicts)` with `Conflicts { OVERWRITE, ERROR, SKIP }` (`:186-189`), `createDirectory`, `delete`, `isDirectoryEmpty`.
- **`InternalNamespace`**: writes the object, then creates missing parent directories, then saves metadata (`InternalNamespace.java:338-404`); `ERROR` throws, `SKIP` logs (`:391-399`); a soft-deleted path is rewritten as a new revision (`:372`); a live entry of the other kind (file vs directory) is a `ConflictException`, a deleted one is purged first (`:421-450`); parents are auto-created (`mkDirs`, `:455-466`); `delete` marks the path and all its children deleted (`:495-512`); `move` refuses an existing target, copies every revision, then purges the source, rolling back the copies on failure (`:130-215`) — *(amended 2026-10-05, Task 1 measured on the artifact: a file's three revisions move as 1, 2, 3, but a **directory** move carries each child's latest revision only, renumbered 1; a move onto an existing target answers 500)*; `purge` marks metadata deleted before removing the object (`:218-224`); a read goes through metadata and **falls back to an older revision** when the indexed object is missing (`:257-289`).
- **Paths.** `NamespaceFile.normalize` refuses `..` (`NamespaceFile.java:167-178`). The controller refuses `/_flows` (`NamespaceFileController.java:68-70`, `:379-386`) and any path component over 255 characters (`:52-53`, `:254-258`).
- **Functions.** `nsfile:///path` is the flow's own namespace; `nsfile://other.ns/path` names a namespace in the authority; a `namespace` argument together with an authority is an error (`AbstractFileFunction.java:181-191`); a bare path becomes `nsfile:///path` in the `namespace` argument's namespace or the flow's (`:92-95`). `fileURI(path, revision?)` returns a `kestra://` URI; with `revision` it checks that revision exists and refuses a non-integer (`FileURIFunction.java:54-79`); without one it does not check that the file exists (`:76-78`, by the code). `read` returns a `String`, or `byte[]` for binary Ion (`ReadFileFunction.java:69-76`), and takes `revision` (`:78-89`). OSS `isAllowedNamespace` is always true (`NamespaceService.java:43-45`, called from `DefaultNamespaceService.java:39`); there is no parent-namespace inheritance.
- **Script `namespaceFiles`** (`core/…/models/tasks/NamespaceFiles.java`): `enabled` default true when the block is present (`:24-25`), `include`, `exclude`, `namespaces` default `["{{flow.namespace}}"]` (`:42-44`), `ifExists` default `OVERWRITE` (`:49-50`; `FileExistComportment { OVERWRITE, FAIL, WARN, IGNORE }`), `folderPerNamespace` default false (`:55-56`). The loader (`NamespaceFilesUtils.java:44-98`) lists matches per namespace, then writes them **in parallel** (`:69-85`), so between two namespaces holding the same path the order is not enforced; `WARN` logs and keeps the existing file (`LocalWorkingDir.java:202-212`) — *(amended 2026-10-05, Task 1 measured: over five runs per mode `OVERWRITE` kept the later namespace's file, `WARN` and `IGNORE` the first, `FAIL` failed the task; `WARN`'s line did not reach the task's logs)*. Script tasks and `WorkingDirectory` load namespace files **before** `inputFiles` (`CommandsWrapper.java:155-178`, `WorkingDirectory.java:292-298`), and `inputFiles` are then written with a plain `FileOutputStream`, replacing a namespace file whatever `ifExists` says (`FilesService.java:38-58`).
- **Globs** (`PathMatcherPredicate.java`): `glob:` is the default syntax; a pattern starting with a word character gets `**/` prepended (`:112-114`); `regex:` is supported (`:22-23`, `:132-134`); the predicate is includes AND NOT excludes (`:67-78`).
- **Tasks** (`core/…/plugin/core/namespace/`): `UploadFiles` — `namespace` (`@NotNull`, though `run` falls back to the flow's, `:118-122`, `:162`; *amended 2026-10-05, Task 1 measured: publish refuses a missing `namespace` with 422 `tasks[up].namespace: must not be null`, so the fallback is unreachable*), `files` globs over **its own working directory** (`:192-201`), `filesMap` (name → URI, read only if it exists, `:204-222`), `destination` default `/` (`:147-148`), `conflict` default `OVERWRITE` (`:150-156`), returns an empty `Output` though `Output.files` is declared (`:187`, `:224-228`). `DownloadFiles` — `namespace` required, `files`, `destination` default `""`; output path → `kestra://` URI (`DownloadFiles.java:81-145`). `DeleteFiles` — `namespace`, `files`, `deleteParentFolder` default false; soft (`DeleteFiles.java:80-176`). `PurgeFiles` — `filePattern` (Commons-IO wildcard), `namespaces`, `namespacePattern`, `behavior` default `Version { keepAmount: 1 }`, `includeChildNamespaces` default true; hard purge; output `size` (`PurgeFiles.java:59-130`, `Version.java`, `PurgeTask.java`, `NamespaceFileService.java:95-115`).
- **API** (`NamespaceFileController.java`, `/api/v1/{tenant}/namespaces`): `GET {ns}/files/search?q` (`:73`), `GET {ns}/files?path&revision` (`:82`), `GET {ns}/files/stats` (`:101`), `GET {ns}/files/revisions` (`:126`), `GET`/`POST {ns}/files/directory` (`:154`, `:180`), `POST {ns}/files` multipart, `.zip` expanded (`:196-235`), `_flows/x` imports a flow (`:236-246`), `GET {ns}/files/export` (zip in memory, `:269-308`; the checkout adds the namespace's flows as `_flows/*.yml`, but *(amended 2026-10-05)* the artifact measured by Task 1 exported the files only, with no `_flows` entry, from a namespace holding a flow), `PUT {ns}/files?from&to` (`:312`), `DELETE {ns}/files` (`:332`). No `@Secured` in OSS.
- **Lifecycle.** Soft-deleted entries stay until `PurgeFiles`; `PurgeStorage` handles execution storage only; no cleanup on namespace deletion was found.

| Kestra | egret W9b | why |
|---|---|---|
| a flow reads any namespace's files in OSS (`isAllowedNamespace` true) | any namespace **of its workspace** (§5) | the workspace is egret's tenant; it comes from the running execution, never from the URI |
| a flow writes to any namespace (`UploadFiles`/`DeleteFiles`/`PurgeFiles` take any `namespace`) | its own namespace only; `PurgeFiles` its own and its child namespaces (§5, §8) | the user's decision: an author with `workflow:manage` on one flow must not rewrite another team's shared files from a task |
| `read` of a `kestra:///other/ns/_files/…` URI is refused by `extractNamespace` (`AbstractFileFunction.java:195-201`, by the code; Task 1 measures) | accepted for any namespace of the workspace (§6) | the same read rule as `nsfile://other.ns/…`; one readability function (§5) |
| revision = `previous + 1`, `previous` read in its own transaction with no lock (by the code) | the next revision is taken under a per-(workspace, namespace, path) lock, in the transaction that inserts it (§4.2) | two concurrent writers must never share a revision |
| a missing object for an indexed revision falls back to an older revision (`InternalNamespace.java:267-289`) | an author error naming the path and revision; no fallback (§4.5) | a silent fallback serves content the caller did not ask for; drift is a fault to see, not to hide |
| `fileURI(path)` without `revision` does not check the file exists (by the code; Task 1 measures) | a missing or soft-deleted path is an error naming it (§6) | a URI that names nothing would fail later, far from its cause |
| `read` returns `byte[]` for binary Ion | non-UTF-8 is an error naming the path, as W9a's `read` (§6) | the result is spliced into a rendered string |
| `namespaceFiles` across namespaces written in parallel, order not enforced | deterministic: a later namespace in `namespaces` wins (under `OVERWRITE`) (§7) | reproducible runs |
| `UploadFiles.files` globs the task's own working directory | globs over the **execution's readable files** (a native handler has no working directory); `files` may also be whole `kestra://` URIs (§8.1) | egret runs `UploadFiles` in the worker, not in a pod |
| `PurgeFiles` with no `namespaces` purges every namespace with a flow; a child namespace is matched by string prefix (`ns.startsWith(…)`, `PurgeTask.java:56`; no `namespaces` takes every `findDistinctNamespace`, `:34`, `:82`), so `a.b` matches `a.bc` | default is the flow's own namespace; children are matched on a dot boundary (§8.4) | the write rule; `a.bc` is not a child of `a.b` |
| `PurgeFiles` `keepAmount: 1` keeps the latest revision even when it is soft-deleted (by the code; Task 1 measures) | a soft-deleted path is purged entirely (§8.4) | the user's decision: a deleted file should not hold storage forever |
| `POST files` expands a `.zip` and imports a flow from `_flows/x` | neither: a `.zip` is stored as a file, `/_flows` is refused (§9) | flows are published through egret's flow API, with its validation and permission |
| `GET files/export` builds the zip in memory (the checkout's code adds the namespace's flows; the artifact measured by Task 1 adds none, amended 2026-10-05) | streamed zip of the files only (§9) | memory is bounded; flows are exported by the flow API |
| `DELETE files` also deletes parent directories left empty, with a parent check not scoped to the namespace (`NamespaceFileController.java:352-363`, by the code) | deletes the named path and its children only (§9) | the walk-up is a side effect the caller did not name |
| a namespace whose segment is `_files` (`a._files`) is accepted, so its flows' keys share a prefix with `a`'s namespace files | refused at publish and by every namespace file operation (§3) | the two key spaces must not overlap |
| a soft-deleted path answers 404 to `revisions` and to every revision read, and after a rewrite its deleted revision stays unreadable (`?revision=3` of `[1, 2, 3, 4]` is 404) (Task 1 measured) | `revisions` lists every revision, deleted ones flagged; an explicit-revision read succeeds; only a latest read is not-found (§4.3, §4.5, §6, §9) | the user's ruling R1 (2026-10-05): history stays visible until `PurgeFiles` removes it |
| `PurgeFiles.namespacePattern` is a wildcard matched against the namespaces that hold **flows**, any namespace (Task 1 measured: `w9b.pp.*` left `w9b.pp.noflow`, which held files and no flow) | the same wildcard grammar, matched against the namespaces that hold **files** and are the flow's own or its children (§8.4) | the user's ruling R2 (2026-10-05); the purge rule (§5) |
| `fileURI` of a directory answers a URI with no object behind it (Task 1 measured) | an error naming the path (§6) | the user's ruling R3 (2026-10-05): a URI that names no file would fail later, far from its cause |
| `GET files/directory` on a file's path answers 200 `[]` (Task 1 measured) | 400 naming the path (§9) | the user's ruling R3 (2026-10-05) |
| `GET files/stats` with no path creates the namespace's root directory on the fly (Task 1 measured) | answers a virtual root directory and creates nothing; no GET route creates anything (§9) | the user's ruling R3 (2026-10-05): a read does not write |
| `UploadFiles` skips a `filesMap` URI that does not exist, silently, and concatenates `destination` with each key (`up2` + `m.txt` → `/up2m.txt`) (Task 1 measured) | a URI that does not resolve fails the task naming it, before anything is written; `destination` is a directory joined with exactly one `/` (`/up2/m.txt`) (§8.1) | fix round 1 of Task 1 (2026-10-05): §8.1's read check, and a path the author can predict |

## 3. Storage layout and URIs

- **Object key:** `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/_files/{path}` for revision 1, `…/{path}.v{N}` for N > 1; a directory has no object (its row is the directory), unlike Kestra's `createDirectory` on the store. Same bucket and prefix as W9a.
- **URI a flow sees:** `kestra:///{ns-as-path}/_files/{path}[.vN]`, Kestra's form. Like W9a's it carries no workspace: egret maps it to a key by prefixing the running execution's workspace.
- **`Tail::NamespaceFile { path, revision }`** joins W9a's `Tail` in `storage/uri.rs`. `parse` recognises the **first** segment equal to `_files` at index ≥ 1 as the namespace file marker: segments before it are the namespace, after it the path; a trailing `.v{N}` (N ≥ 2, digits only) is the revision. This check runs before the `executions/{uuid}` search, so `kestra:///a/_files/x/executions/<uuid>/f` is namespace `a`'s file `/x/executions/<uuid>/f`, never an execution URI of a flow `_files` (which cannot exist, §1).
- **A namespace with a segment `_files`** (`a._files`, legal by `is_valid_namespace`) would put its flows' execution keys under namespace `a`'s files. Such a namespace is refused (the user's ruling, header): publish refuses it from W9b on, and every namespace-file operation (API, tasks, functions) refuses it as a target or an authority, each with an error naming the namespace and the segment. Task 1 counts the published flows in a dev database and the corpus entries whose namespace has one, and its report lists each (none is expected).
- **`nsfile://`** parses as: no authority (`nsfile:///path`) → the running flow's namespace; an authority (`nsfile://other.ns/path`) → that namespace, checked with `is_valid_namespace`. A bare path (no `://`) is `nsfile:///path`.
- **Path rules**, one function for every entry point (functions, tasks, API, plugin bridge): a leading `/` (added when missing); no empty, `.` or `..` segment; each segment ≤ 255 characters; not `/_flows` or under it; W9a's refused characters (`\`, `?`, `#`, control characters); a file's last segment must not end in `.v{digits}` (it would read as a revision of another path). A path that breaks a rule is an author error naming the path and the rule.

## 4. Core

### 4.1 The metadata table

`workflow.namespace_files` (migration 0169): `workspace_id` (FK to the workspace), `namespace` (text), `path` (text, normalised, a directory ending in `/`), `revision` (int ≥ 1; always 1 for a directory), `is_directory`, `size`, `deleted`, `created_at`, `created_by` (the user or the execution that wrote it). Unique `(workspace_id, namespace, path, revision)`; an index on `(workspace_id, namespace, path)` for the latest row. One row per revision; the latest row of a path is its highest revision. No FK to a namespace (there is no namespace table): a namespace with no flows can hold files.

**A namespace file exists only once its row exists**, as W9a's index (W9a §4.2). Reads, `exists`, stats and listings go through the table.

### 4.2 Writing

1. Check the path (§3) and the write rule (§5).
2. In one transaction, take a per-`(workspace, namespace, path)` lock (`pg_advisory_xact_lock` on a hash of the three, or `SELECT … FOR UPDATE` on the latest row; the plan picks one and a test runs two writers concurrently), read the latest row, apply the conflict rule (`OVERWRITE` → next revision; `ERROR` → author error naming the path; `SKIP` → no write, reported as skipped), and compute `revision = max + 1` (1 for a new path).
3. Stream the content to the revision's key (`put_stream`), `head` for the size, insert the row, create missing parent directory rows, commit. The lock is held across the upload so that no second writer can compute the same revision; a failed upload rolls the transaction back and leaves at most an unindexed object, which a re-write of that revision overwrites.
4. **File/directory clash:** a live row of the other kind at the path is a conflict error naming it; a soft-deleted one of the other kind is purged first (Kestra's rule).
5. **Rewriting a soft-deleted path** creates a new revision (`max + 1`, live); earlier revisions are kept.

Caps: `workflow.storage.max_file_bytes` per file, as W9a.

### 4.3 Soft delete

`delete(path)` marks the latest row of the path and of every child deleted, in one statement; objects are kept. Deleting a directory deletes its children. A soft-deleted path is invisible to reads, listings, `exists` and globs, and visible to `revisions` and to `PurgeFiles`. *(Amended 2026-10-05, the user's ruling R1: "reads" here are **latest** reads; an explicit-revision read of a soft-deleted path — `?revision=N`, `read(path, revision)`, `fileURI(path, revision)`, or a `kestra:///…/_files/…` URI, which names one revision — succeeds, and `revisions` lists every revision with the deleted ones flagged.)*

### 4.4 Move

`move(from, to)` refuses an existing live target; copies every revision of `from` (and of every child) to `to` keeping revision numbers, inserting rows as it goes; then purges the source (rows first, then objects). A failure during the copy purges every copy already made, in reverse order, and reports the failure naming both paths; nothing of the source is touched before every copy has succeeded.

### 4.5 Reading

A read resolves `(namespace, path, revision?)` to a row (the latest live one, or the given revision, which must exist and whose path must not be soft-deleted at its latest revision for a latest-read), then streams that revision's key. **A missing object behind a row is an author error naming the path and revision**; egret does not fall back to an older revision (§2). An unreachable store is an infrastructure error (§11). *(Amended 2026-10-05, the user's ruling R1: a given revision is served even when the path is soft-deleted; only a latest read of a soft-deleted path is not-found.)*

### 4.6 One readability and writability function

`storage::nsfiles::authorize(workspace, running_flow_namespace, target_namespace, Access::{Read, Write, Purge})` decides every access, for the functions, staging, the four tasks, the plugin bridge and (with the user's permission instead of a flow) the API. No caller re-implements the rule. Each refusal has a test that fails when the refusal is removed.

## 5. Who may read and write

- **Workspace:** always the running execution's (or, for the API, the route's `{ws}` after the user's permission check). Nothing in a URI or a property names a workspace.
- **Read:** any namespace of that workspace (`nsfile://other.ns/…`, `kestra:///other/ns/_files/…`, `namespaceFiles.namespaces`, `DownloadFiles.namespace`).
- **Write** (`UploadFiles`, `DeleteFiles`, a plugin's `putFile`/`delete`/`move` — refused anyway, §10): the running flow's own namespace only, compared exactly.
- **Purge** (`PurgeFiles`): the flow's own namespace and its child namespaces (`ns == own` or `ns` starts with `own + "."`).
- A refusal is an author error naming the namespace and the rule; it never says whether a file exists there.

## 6. The expression functions

| function | behaviour |
|---|---|
| `fileURI(path, revision?)` | `kestra:///{ns}/_files/{path}` for the latest live revision (`…{path}.vN` when N > 1), or for the given revision, which must exist. A `kestra://` argument is returned unchanged after it parses. `../`, a non-integer `revision`, a missing or soft-deleted path are errors naming the path. *(Amended 2026-10-05: "soft-deleted" applies without `revision`; with one, a revision of a soft-deleted path is returned (R1). A directory is an error naming the path (R3).)* |
| `read(path, revision?)` | UTF-8 contents; non-UTF-8 is an error naming the path (§2); capped by `max_read_bytes` |
| `fileSize`, `fileExists`, `isFileEmpty` | from the metadata row; `fileExists` of a well-formed path with no live row is `false`, as W9a's *missing* |

- All five accept a bare path (→ `nsfile:///path`), `nsfile:///path`, `nsfile://other.ns/path`, and a `kestra:///…/_files/…` URI; plus a `namespace` argument, which together with an authority is an error (`AbstractFileFunction.java:181-191`). A `kestra://` URI of an execution keeps W9a's snapshot rule.
- **No snapshot for namespace files.** Namespace files commit immediately, outside `settle`'s transaction, so the pool sees every committed row: the render looks them up directly through the pool, and a path assembled at render works. The lookup and the read run through W9a Task 4's non-wedging bounded read (`off_the_runtime`, `runners/env.rs:2094-2100`); a store or database outage sets the file scope's `infrastructure_failure` flag (§11), so the run is re-claimed with no attempt spent.
- They exist in every render with an execution; a render with none (a trigger's conditions, publish) refuses them by name, as W9a's.
- `fileURI` leaves `REFUSED` (`functions.rs:269`), and the bare-path and `nsfile://` refusal (`functions.rs:356-361`) is replaced; the tests that pin "names W9b" (`functions.rs:3993-4025`) flip, each with a replacement test proving the read.

## 7. `namespaceFiles` on script tasks and `WorkingDirectory`

- **Properties as Kestra's:** `enabled` (default true when the block is present), `include`, `exclude`, `namespaces` (default the flow's namespace), `ifExists` (`OVERWRITE`/`FAIL`/`WARN`/`IGNORE`, default `OVERWRITE`), `folderPerNamespace` (default false).
- **Globs** follow `PathMatcherPredicate`: `glob:` default, `**/` prepended to a pattern that starts with a word character, `regex:` supported, includes AND NOT excludes. The exact path string a glob sees (leading `/` or not) is **Task 1's measurement**, pinned by a fixture; egret implements against the fixture, not against a reading of the Java.
- **At claim time** egret lists the latest live files of each namespace in `namespaces` (each read-checked, §5) that match, and stages them through W9a's streamed transfer at `path` (or `{namespace}/{path}` with `folderPerNamespace`).
- **Deterministic order:** namespaces are staged in list order, so under `OVERWRITE` a later namespace wins; `FAIL` fails the task naming both sources; `WARN` keeps the first and logs one line naming the path; `IGNORE` keeps the first. **`inputFiles` are staged after namespace files and always overwrite a same-named one, whatever `ifExists` says**, as in Kestra (the user's ruling, header); `ifExists` governs collisions between namespaces only.
- **`inputFiles` values that are namespace-file URIs** (`nsfile:///…`, `nsfile://other.ns/…`, `kestra:///…/_files/…`; the user's ruling, header) are staged like `namespaceFiles` entries: read-checked by the same function (§4.6, §5), streamed by W9a's transfer under the `inputFiles` name. A refused namespace or a missing path fails the task before it starts, naming the path.
- **Bounds are structural:** the listing is capped by a count (`workflow.storage.max_namespace_files`, default 10 000), counted on the rows egret actually listed, with a refusal naming the cap; staged bytes count against `max_task_bytes` as the transfer streams them. No bound mirrors a glob grammar.
- **Script tasks:** `namespaceFiles` leaves `COMMON_REFUSED` (`scripts/mod.rs:202`); `tests/scripts_gate.rs:940-945` flips to a test that the property publishes, and a refusal test remains for a malformed block.
- **`WorkingDirectory` (out of W9b, the user's ruling, header):** **egret's executor does not run `WorkingDirectory` at all** (§1: planned opaque, the claim answers `None`). W9b therefore only stops its model from silently dropping `namespaceFiles` (`models/task.rs:555-565`). **At publish**, the block is parsed and validated with the same rules as on a script task: a malformed block is refused, naming the property; a valid one is accepted and kept in the model, for the later package that runs `WorkingDirectory`. **At run**, nothing changes: `WorkingDirectory` behaves exactly as before W9b and no namespace file is staged. The staging function is the one script tasks use, so that package only calls it. The two corpus flows stand outside the ratchet; Task 1 records their current outcome and plan Task 11 (spec §12 item 10) measures the ratchet before and after (no change is expected). *(Amended 2026-10-05, Task 1 measured: neither flow is in `baseline.txt` or `golden.json`. The artifact refused both at publish — `corpus.json`: 422 `Invalid type: io.kestra.core.tasks.test.Read`, `task-type-not-in-image` — so they have no golden entry and stand outside the ratchet's 294; before Task 1's fix round 1 this sentence said they "stay baselined". The ratchet at the base is `294 golden entries, 108 mismatching, 108 baselined, 0 broken`.)*

## 8. The `io.kestra.plugin.core.namespace` tasks

Native handlers in the worker, like W9a's `core.storage` tasks. Each follows its Kestra class's properties and outputs (Task 1 extracts and measures them).

### 8.1 `UploadFiles`

- `namespace`: default the flow's; anything else is refused (§5).
- `files`: a list whose items are whole `kestra://` URIs or globs over the **execution's readable files** (W9a §4.4's readable set, matched on the URI's file path); Kestra globs its own working directory (§2).
- `filesMap`: name → `kestra://` URI (read-checked) or literal contents. *(Amended 2026-10-05, Task 1 fix round 1: every URI of `files` and `filesMap` is resolved before the first write; one that does not resolve fails the task naming it, and nothing is written. Kestra skips it silently.)*
- `destination` default `/`; `conflict` default `OVERWRITE` (`ERROR`, `SKIP`). *(Amended 2026-10-05, Task 1 fix round 1: `destination` is a directory; a missing leading `/` is added and each file's name is joined to it with exactly one `/` — `destination: up2` and key `m.txt` write `/up2/m.txt`, where Kestra wrote `/up2m.txt`.)*
- Each file is written by §4.2. Output: empty, as Kestra's run returns.

### 8.2 `DownloadFiles`

`namespace` required (read rule), `files` globs, `destination` default `""`. Each match is copied (streamed) to an execution file under the task run's prefix (`tasks/{taskId}/{taskRunId}/{destination}{path}`), indexed at commit as W9a's task files. Output `files`: path → `kestra://` URI.

### 8.3 `DeleteFiles`

`namespace` (own only), `files` globs, `deleteParentFolder` default false (deletes a parent directory left empty, deepest first). Soft (§4.3).

### 8.4 `PurgeFiles`

- `namespaces` / `namespacePattern` (exclusive, as Kestra), default the flow's own; `includeChildNamespaces` default true (dot-boundary match); every resolved namespace must pass the purge rule (§5), and a pattern is matched only against the namespaces that hold files in the workspace. *(Amended 2026-10-05, the user's ruling R2: `namespacePattern` uses Kestra's wildcard grammar, `FilenameUtils.wildcardMatch` — `*` any run of characters, dots included, `?` one character, case-sensitive; `w9b.pp.*` does not match `w9b.ppx.c` — and its candidates are restricted to the namespaces holding files that are the flow's own or its children, so a pattern never reaches another namespace and nothing is refused for one it could not match. A listed `namespaces` entry outside the purge rule is still refused by name.)*
- `filePattern`: Commons-IO wildcard (`*`, `?`) on the path, as Kestra's `filterItems`. *(Amended 2026-10-05, Task 1 measured: the string matched is the path **without** its leading `/` — `filePattern: "/q.txt"` purged nothing, `"q.txt"` purged two revisions — so egret matches the pattern against the path with the leading `/` removed.)*
- `behavior`: `Version { keepAmount, before }`, default `keepAmount: 1`. `keepAmount` keeps the N highest revisions of each path; `before` purges revisions updated before the instant and never the latest; a soft-deleted path is purged entirely (§2).
- **Hard delete:** metadata rows first, then objects (W9a's safe order: a crash leaves an orphan object, never a row pointing at nothing). Output `size`: the number of revisions purged.

## 9. The namespace file API

Under `/api/ws/{ws}/workflows/namespaces/{ns}/files`, mirroring `NamespaceFileController`:

| route | permission | answer |
|---|---|---|
| `GET …/files?path&revision` | `workflow:read` | the bytes, streamed, with W9a's download headers (`application/octet-stream`, sanitised `Content-Disposition: attachment`, `X-Content-Type-Options: nosniff`, `storage/routes.rs:262`); a given `revision` of a soft-deleted path is served, a latest read of one is 404 (R1, 2026-10-05) |
| `GET …/files/stats?path` | `workflow:read` | size, type, revision, created; no `path` answers a virtual root directory and creates nothing (R3, 2026-10-05) |
| `GET …/files/revisions?path` | `workflow:read` | every revision, deleted ones flagged, a soft-deleted path's included (R1, 2026-10-05) |
| `GET …/files/search?q` | `workflow:read` | live paths containing `q`, capped by count; as Kestra's: a case-insensitive substring of the path, files only (R3, 2026-10-05) |
| `GET …/files/directory?path` | `workflow:read` | live children of the directory; a file's path is 400 naming it (R3, 2026-10-05) |
| `GET …/files/export` | `workflow:read` | a streamed zip of the latest live files (no flows) |
| `POST …/files?path` (multipart) | `workflow:manage` | §4.2 with `OVERWRITE`; reuses W9a's bounded multipart parser; a `.zip` is stored as a file, not expanded |
| `POST …/files/directory?path` | `workflow:manage` | creates the directory and its parents |
| `PUT …/files?from&to` | `workflow:manage` | §4.4 |
| `DELETE …/files?path` | `workflow:manage` | §4.3, the named path and its children only |

- `{ns}` is checked with `is_valid_namespace` and the `_files` segment rule (§3); a namespace with no flows can still hold files. Unknown path → 404; another workspace's namespace is unreachable by construction (`{ws}` scopes every query).
- `/_flows` is refused (400) on every route; egret imports no flow from here (§2).
- **No GET route creates anything** (the user's ruling R3, 2026-10-05), where Kestra's `stats` creates the namespace's root (§2).
- utoipa-documented; the frontend client is regenerated with `npm run gen:api`. The UI is W7's.

## 10. The plugin bridge

- `runContext.storage().namespace(ns)` stops throwing W9b: `NO_NAMESPACE_FILES` (`ShimRunContext.java:274-280`) is replaced by a factory over staged namespace files.
- **Staging:** egret stages every rendered property value that is a **whole** `nsfile://` or `kestra:///…/_files/…` URI (or that `fileURI` produced), read-checked (§5), at W9a's `.egret/storage/<n>`; `task.json` gains a separate `namespaceFiles` array, one entry per staged file with its URI, namespace, path, revision, size and staged path; W9a's `storage` map (URI → staged path) is unchanged (the controller's ruling, header).
- `Namespace.getFileContent`, `exists`, `getFileMetadata`, `all`, `findAllFilesMatching`, `get` answer over the staged set of that namespace.
- `putFile`, `delete`, `move`, `createDirectory` throw `UnsupportedOperationException` with a reason: the pod holds no credential and no catalog plugin uses them (Task 1 checks the catalog). A plugin that computes a namespace path at run does not see it (§15).

## 11. Lifecycle and errors

- **No retention for namespace files:** they are authored content, as in Kestra. Soft-deleted revisions and their objects stay until `PurgeFiles` (or an API delete followed by a purge).
- **W9a's pruner never touches `_files`:** it deletes under execution prefixes only; a test writes namespace files, runs a prune pass, and asserts every object and row survives.
- **No per-namespace deletion** (no namespace table, §15).
- **Errors**, as W9a's: a path-rule violation, a refused namespace, a missing object behind a row, a conflict are author errors naming the path; an unreachable store or database is an infrastructure error (the worker returns `Err`, the run is re-claimed, no attempt spent); a crossed cap fails the task naming the cap. No error names a secret.

## 12. Tasks

1. **Measure Kestra** (`oracle-exec/w9b-namespace-files.json`): object paths per revision, directories, soft delete then rewrite; glob semantics (the path string a glob sees, `**/` prefixing, `regex:`, excludes); `fileURI` with and without `revision` on present, missing and deleted paths; `read` of another namespace's `kestra:///…/_files/…`; `ifExists` across namespaces, and `inputFiles` overwriting a same-named namespace file; the four tasks' outputs; `PurgeFiles` on a soft-deleted path; path rules; plus the baselines (tests, ratchet, clippy), and the count and list of published flows and corpus entries whose namespace has a `_files` segment.
2. **Metadata, URIs, path rules, store operations** (migration 0169, `Tail::NamespaceFile`, `nsfile://` parsing, the `_files` segment rule at publish, `authorize`, write/read).
3. **Revisions, conflicts, move, soft delete** (the lock, concurrent writers, rollback).
4. **The expression functions** (`fileURI`, the four over namespace files, the pool lookup on the render thread).
5. **`namespaceFiles` staging** (script tasks; the publish gate flipped; on `WorkingDirectory` the block parsed and validated only).
6. **`UploadFiles`, `DownloadFiles`.**
7. **`DeleteFiles`, `PurgeFiles`.**
8. **The namespace file API** (routes, utoipa, `npm run gen:api`).
9. **The plugin bridge** (staging, the `Namespace` factory, `EgretStorageTest` flips).
10. **End to end on a temporary kind cluster; coverage; docs; ratchet; full local CI.**

## 13. Testing

- Unit: path rules (each refusal with a test that fails when it is removed), `parse` of `_files` URIs (including `kestra:///a/_files/x/executions/<uuid>/f`), `nsfile://` with and without authority, `authorize` (own, other namespace read, other namespace write, child purge, `a.bc` not a child of `a.b`, other workspace).
- Postgres: revision numbering under concurrent writers (two tasks, one path, distinct revisions, no shared key); soft delete then rewrite; file/directory clash; move rollback (a store that fails mid-copy); purge `keepAmount`/`before`/soft-deleted; workspace isolation.
- Destructive tests run only on `MemStore` or under throwaway prefixes on MinIO; never a delete on the shared bucket root.
- MinIO: streamed upload and download beyond one part.
- API: authz (`workflow:read` vs `workflow:manage`), 404 for another workspace's and an unknown path, download headers, `/_flows` refused, multipart bounds.
- Live Docker and Kubernetes: `namespaceFiles` on a script task with several namespaces, `folderPerNamespace`, each `ifExists`.
- Pruner: a pass leaves `_files` untouched.
- Oracle: the ratchet is measured before and after and recorded in `oracle-exec/README.md`; the two corpus flows are `WorkingDirectory` and stand outside the ratchet (the artifact refused both at publish, so they have no golden entry; corrected 2026-10-05), so no ratchet change is expected (§7). Publish: a malformed `namespaceFiles` on `WorkingDirectory` is refused by name and a valid one publishes; a namespace with a `_files` segment is refused at publish, by the API, by the tasks and by the functions, by name.
- Live: an `inputFiles` entry overwrites a same-named namespace file under every `ifExists`.
- End to end on a temporary kind cluster, as W9a.

## 14. Out of scope

The KV store (W9c); running `WorkingDirectory` itself (and so its `namespaceFiles`, `inputFiles`, `outputFiles`, `cache`); `.zip` expansion and `_flows` import on upload; namespace deletion and namespace-level permissions; the UI (W7); plugin writes to namespace files.

## 15. Known limits

- A flow writes only to its own namespace; a Kestra flow that uploads to, deletes from or purges another namespace fails on egret, naming the namespace (§5).
- `PurgeFiles` with no `namespaces` purges the flow's own namespace (and children), not every namespace (§8.4).
- `WorkingDirectory`'s `namespaceFiles` is validated at publish and kept, but not run, because egret does not run `WorkingDirectory`; running it is a later package (§7).
- `ifExists` does not protect a namespace file from an `inputFiles` entry of the same name, as in Kestra (§7).
- A plugin reads only namespace files named by a whole property value; one that computes a namespace path at run, or writes a namespace file, fails with a reason (§10).
- A missing object behind a metadata row is an error, where Kestra serves an older revision (§4.5).
- `read` of a binary file is an error, where Kestra returns bytes (§6).
- `UploadFiles.files` globs the execution's files, not a working directory (§8.1).
- No retention: namespace files and soft-deleted revisions keep storage until purged (§11).
- A namespace with a segment `_files` is refused (§3).
- `fileURI` of a directory is an error, where Kestra returns a URI with no object behind it (§6; R3, 2026-10-05).
- `UploadFiles` fails on a `filesMap` or `files` URI that does not resolve, where Kestra skips it; `destination` is joined to each name with `/` (§8.1; 2026-10-05).
- There is no namespace table, so deleting every flow of a namespace leaves its files; they are reachable through the API (§9).
