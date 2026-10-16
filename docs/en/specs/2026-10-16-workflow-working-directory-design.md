# WorkingDirectory: children that share one working directory

**Status:** design, 2026-10-16. Nothing is built. Read in source from `origin/main` at `8043c4ac`, and from the Kestra checkout `data-portal/kestra` at `96893b589`. The pinned image `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd` is the artifact the oracle was recorded against. Every Kestra fact below that is not read from source is marked **verify on the image** and measured in the plan's first task.

**Supersedes:** the user's ruling of 2026-10-05 in `docs/en/specs/2026-10-05-workflow-w9b-namespace-files-design.md` that *"`WorkingDirectory` stays out of W9b"*. That ruling kept the block parsed and validated and never run. This spec runs it. The W9b ruling is not otherwise changed.

**Depends on:**
- W5a (script tasks, `egret-workflow/src/worker/commands.rs`, the runners that copy files in and out of a task's container or pod).
- W9a (internal storage, `egret-storage` streaming, `storage::uri`, the pruner). The snapshot in §5.2 and the cache in §5.4 live there.
- W9b (`namespaceFiles` on script tasks, `FilesService.inputFiles` and `outputFiles` staging). `WorkingDirectory` reuses that staging rather than writing its own.
- W8 (the `allowFailure:` flag on a child, `executor/allow_failure.rs`), which decides whether a failed child stops the block.
- The no-silent-stall package, which refuses a stored `WorkingDirectory` run with `NotALeaf` (`worker/mod.rs`) and makes validate refuse non-runnable children.
- The oracle (`crates/egret-workflow/oracle-exec/README.md`) and its ratchet (`oracle-exec/baseline.txt`).

## 1. Why

`io.kestra.plugin.core.flow.WorkingDirectory` runs its child tasks **one after another in the same working directory**, so a file written by one child is there for the next, with no output wiring. It also stages `namespaceFiles` and `inputFiles` into that directory before the children run, collects `outputFiles` after them, and can cache a set of files between runs (`cache:`).

Egret plans it as an opaque `Runnable` (`executor/plan.rs`, `executor/mod.rs:3546`: `K::WorkingDirectory(_) => PlannedAs::Opaque`). Since no-silent-stall, `spec::validate` refuses every `WorkingDirectory` at publish (`unsupported_working_directory`, `validate.rs:1759`), so no new flow can run one. The model kept only `tasks` and `namespaceFiles` (`models/task.rs:568-600`); `inputFiles`, `outputFiles` and `cache` were dropped at parse. Plan Task 2 keeps them at parse; Task 3 lifts the refusal.

The corpus has 15 flows that mention `WorkingDirectory`: 13 valid and 2 invalid. Four of the valid ones are in the golden (`valids/task-runif-workingdirectory`, `valids/working-directory-invalid-runif`, `valids/working-directory-taskrun`, `valids/working-directory-taskrun-nested`), and none of them uses `inputFiles`, `outputFiles`, `cache` or `namespaceFiles`. The other 9 valid flows are refused at publish by the artifact, which does not know the test classes they use (`corpus.json`):

| Test class the flow uses | Flows |
|---|---|
| `io.kestra.plugin.core.flow.WorkingDirectoryTest$LocalFiles` (422 at publish) | `working-directory-inputs`, `working-directory-outputs`, `working-directory-cache`, `working-directory-loop`, `working-directory` |
| `io.kestra.core.tasks.test.Read` (422) | `working-directory-namespace-files`, `working-directory-namespace-files-with-namespaces` |
| `io.kestra.core.tasks.test.Encrypted` (422) | `working-directory-secret-output`, `working-directory-taskrun-encrypted` |

So **the existing oracle cannot measure `inputFiles`, `outputFiles`, `cache`, `namespaceFiles` or the sharing of a directory between children.** §6 says what replaces it.

## 2. What Kestra does

Read from `worker/src/main/java/io/kestra/worker/processors/WorkerTaskProcessor.java` (`runWorkingDirectory`, lines 90-172) and `core/src/main/java/io/kestra/plugin/core/flow/WorkingDirectory.java`.

The **worker** runs the whole block, and the executor sees each child as its own task run:

1. `preExecuteTasks`: restore the cache archive if one exists and has not expired (§2.3); load `namespaceFiles` (`NamespaceFilesUtils.loadNamespaceFiles`); write `inputFiles` (`FilesService.inputFiles`). **If this throws, the block's task run is `FAILED` and no child runs.**
2. For each child in `tasks`, in order, skipping `disabled: true`:
   - evaluate its `runIf`; a falsy one is recorded `SKIPPED` and the loop goes on;
   - otherwise run it with `runTask(..., cleanUp=false)` in the **same** working directory (`forWorkingDirectorySubtask(..., runContext.workingDir())`);
   - if the child's state is `FAILED` **and it does not have `allowFailure: true`**, `break`: the remaining children are not run.
3. `postExecuteTasks`, **unconditionally** (after the loop, whatever a child did): capture `outputFiles` and write them as `outputs.ion` (exposed as `outputs.<wdir>.outputFiles`, measured, §2.1); then save the cache archive if a matched file changed since the restore.
4. Any failure in step 3 fails the block.

The block's final state is then computed by the executor from its children, as for `Sequential` (`resolveNexts` runs the next tasks only when a child failed, `WorkingDirectory.java`).

### 2.1 Facts this spec depends on, to verify on the image

Measured on the pinned image, 2026-10-16, on a private compose project (`wd-probe`, removed after), with real script tasks on the Docker runner (`alpine:3.20`). The probe flows and per-run JSON are in the session scratchpad, not the repo.

- **V1 confirmed.** `wd-fail`: a child that fails without `allowFailure` (`exit 1`) ends the block `FAILED`, and the sibling after it has **no task run** at all.
- **`allowFailure` on a child, measured.** `wd-allowfail`: the same failing child with `allowFailure: true` is `WARNING`, the sibling runs and is `SUCCESS`, and the block is `WARNING`. The child's `allowFailure` lets the loop go on and marks the block `WARNING`.
- **`runIf` false, measured.** `wd-runif`: the child with `runIf: "{{ false }}"` **does** get a task run, `SKIPPED` with one attempt. It is not absent. The sibling after it runs and reads the file the first child wrote.
- **Sharing, measured.** `wd-files`: child 1 reads an `inputFiles` entry and writes `a.txt`; child 2 reads `a.txt` (`SECOND_READ_A=hello`). The directory is shared between children, as §1 says.
- **V2 confirmed.** `wd-finish`: child 1 writes `out.txt` and then fails. The block is `FAILED`, and an `errors:` task still reads `outputs.wdir.outputFiles` containing `out.txt`. So `outputFiles` are collected after a failed child.
- **V3 confirmed.** `wd-prepare-fail`: an `inputFiles` URI that does not exist makes the block `FAILED` with `attempts = 0` and no child run (`Failed preExecuteTasks on WorkingDirectory: File not found for URI …`).
- **Cache, measured.** `wd-cache` run twice: the first execution logs `written`, the second `restored` (the archive was read back).
- **Output key, measured.** `outputs.wdir.outputFiles` is a map from each `outputFiles` glob match to a `kestra:///…/tasks/wdir/{run id}/…-out.txt` URI. The key is **not** `uris`: `outputs.wdir.uris` does not exist on this image, and the Java example in `WorkingDirectory.java` that says `uris` is stale.
- **V4 not measured.** A child's own outputs being its own is standard for every task; the plan does not spend a probe on it.

### 2.2 `outputFiles`, `inputFiles`, `namespaceFiles`

- `inputFiles` and `namespaceFiles` load **before** the first child, in this order: namespace files, then `inputFiles`, so `inputFiles` overwrite a same-named namespace file (the W9b ruling, which holds here).
- `outputFiles` is a list of globs, matched non-recursively against the directory after the last child, as a script task's `outputFiles` is (`worker/handlers/script.rs:68`). A missing file is ignored.

### 2.3 `cache`

- `cache.patterns` are globs over the working directory; `cache.ttl` is optional (absent means the archive never expires).
- The archive's storage key is `(namespace, flow, cache id = the block's id, object id = the task run's value)`, hashed for the object id (`StorageContext.getCacheURI`, `StorageContext.java:204`).
- Restore happens before the children; the archive is written after them **only if some matched file was modified after the restore**. A first run (no archive) therefore saves whatever matched.
- An expired archive (last modified older than `ttl`) is deleted on read and treated as absent (`InternalStorage.getCacheFile`).

## 3. What egret does now

- `models::WorkingDirectory` holds `tasks`, `namespace_files`, and (since plan Task 2) `input_files`, `output_files` and `cache`, kept verbatim at parse (`models/task.rs`). Publish refuses every `WorkingDirectory` with `unsupported_working_directory` (`validate.rs:1759`), so a flow with one is not stored and cannot run; that refusal is the reason no run reaches the claim. Before Task 2 the three properties were dropped at parse, which was hidden only by that refusal.
- `spec::validate` refuses a non-runnable child (`working_directory_child_not_runnable`) and an empty block (`working_directory_tasks_required`), and validates `namespaceFiles` at publish (`spec/validate.rs:1663`).
- `executor::plan` plans the block as an opaque `Runnable`, so the executor never drives its children, and the worker's claim answers `None` for it (`worker/claim.rs`; `NotALeaf` in `worker/mod.rs`).
- Egret's leaves each run in **their own** runner, and a leaf's working directory is `/egret/working-dir` inside a container or pod that is created for that task (`worker/commands.rs:133`, `WORKING_DIR`). Nothing carries a directory from one leaf to the next.
- Files already move **into and out of** a leaf's container: `docker cp` for inputs (`worker/commands.rs`, the input staging), and a `files-out` helper for outputs in a Kubernetes pod (`worker/commands.rs:321`). §5.2 reuses those paths.

So the central difference from Kestra is not a missing feature but a **runtime boundary**: Kestra's children share a directory because one worker process runs them all in one temp dir; egret's children run in separate containers or pods, each with a fresh directory.

## 4. Design choices

### 4.1 Three ways to share a directory

| Option | How | Verdict |
|---|---|---|
| A. Run the whole block in one runner | one claim runs every child in one container or pod | **Rejected.** Children have different images, runners, timeouts and retries; one pod cannot run a `python` step and a `shell` step with different images, and a child's retry would re-run its siblings. |
| B. Shared volume | a Docker named volume, or a `ReadWriteMany` PVC for Kubernetes, mounted into every child | **Not the default.** It needs infrastructure the kind cluster and the default deployment do not have, and an RWX claim is a per-cluster choice. It stays a possible later optimisation behind a runner setting. |
| **C. Snapshot between children** | after each child, the directory is copied out of its container or pod into internal storage; before the next child, it is copied in | **Chosen.** Works on every runner egret has, reuses the copy paths of §3, and keeps every child a normal leaf. Costs one copy out and one copy in per child, bounded by `max_task_bytes`. |

### 4.2 The block is driven by the executor

The block becomes a flowable, `TaskKind::WorkingDirectory { tasks }`, driven like `Sequential` by the executor (as `AllowFailure` is, `2026-10-16-workflow-allow-failure-flowable-design.md` §5), not run by one worker. Each child is claimed as an ordinary leaf, and the executor passes it the block's snapshot reference. This keeps every child a task run with its own id, timeout, retry and log, and keeps the executor the only writer of run state (Kestra's children are also task runs, so the trace shape matches).

Consequences: the block's preparation (`namespaceFiles`, `inputFiles`, cache restore) and its finish (`outputFiles`, cache save) are **separate claims** of the block, so the block itself has two extra leaves' worth of work. The plan decides whether they are a claim of the block or a step inside its first and last child; §5.3 picks the first.

### 4.3 The snapshot is per block run

A snapshot is an archive of the directory at a point in the block's run, stored in W9a's bucket under `{workflow.storage.prefix}{workspace}/executions/{exec}/_wd/{block run id}/{seq}.tar`, indexed in a row so the pruner and the sweeper can find it. It is written by the child that finished and read by the child that starts next. It is deleted when the block reaches a terminal state; a crash leaves objects behind, and the W9a pruner's retention is the backstop (§7, open point 2).

### 4.4 Decisions taken in implementation (2026-10-16)

Recorded when the snapshot, the block's claims and the Docker copy were built (plan Task 6). Each is a choice the spec left open, or a change from §5 as written.

- **Streamed, not packed.** The directory lives in the container, so the worker never holds it. Copy-out is `docker cp c:<wd>/. -` read into `snapshot::admit`, which validates each entry (symbolic and hard links, devices, `..`, absolute names are refused; the cap is checked against each entry's size before its bytes are copied) and writes the admitted archive straight into `ObjectStore::put_stream` through a 256 KiB in-memory pipe. Copy-in is `get_stream` into `admit` into `docker cp - c:<wd>`. No archive is held whole, and no temporary file is written on the worker. `pack` and `unpack` over a `Vec` are gone. Mode bits are kept (`TarEntry::mode`), so an executable stays executable.
- **Block claims are rows with no `type_id`.** The prepare and finish claims are `task_runs` rows in `RUNNING` whose `working_dir` is `PREPARING` or `FINISHING`. `claim` takes them under the same lease as a leaf, and `run_block_claim` completes them through the same fence and `settle`, with `CompleteWorkingDirectoryClaim`. A failed claim sets `SetRunState { Failed }` on the block and writes its reason to `message`, so the block ends `FAILED` before any child (V3). A claim never stays unfinished: every error in the claim's work is a `BlockDone::Failed`.
- **The pointer is a column.** Migration 0194 adds `task_runs.working_dir_snapshot`, the object URI of the block's current snapshot. Objects are `{prefix}{ws}/executions/{exec}/_wd/{block run}/prepare.tar` (written by the prepare claim, an empty archive) and `.../{child run}.tar` (a child's copy-out). The key is named by the producing run, not `{seq}.tar`: a retry of one run overwrites its own object, and no sequence has to be stored. A child's object becomes the pointer when the child's commit sees it with `head`; a copy-out that was refused leaves no object and does not move the pointer. The object is removed before the child runs, so an object from an earlier attempt cannot pass for this one.
- **A child with no pointer is corrupt.** `working_dir::carry_of` refuses a child whose block has no snapshot, by name (`corrupt: ...`), and the child fails without a run of its own. It is never started on an empty directory (§5.2).
- **The finish claim discards the block's snapshots after it commits**, with one `delete_prefix`. A failure to delete is a warning; the pruner sweeps what is left (open point 2 below stays open).
- **Docker only, and refused elsewhere.** A child whose runner is not Docker fails by name before the gate runs (`script.rs`). This is the no-silent-drop rule (§8): a Kubernetes child would otherwise run on an empty directory and succeed.
- **Over-cap and link problems fail the child.** `TooLarge` and `Refused` from a copy-out fail the child with the problem as its reason and remove the object. The block then stops (§5.3). Outputs of a finish claim after such a child are read from the last admitted snapshot (Task 7; not yet built).
- **`namespaceFiles` runs in the prepare claim (Task 7).** The prepare claim renders the block's `namespaceFiles` with the block run's own render context (`claim::render_context`, the context a leaf of the same flow renders with, so no second renderer exists), stages it with W9b's `stage` for the flow's own namespace, and writes the directory as the first snapshot with `write_tar` over the stored inputs, streamed through a pipe. A render, staging or cap failure fails the block before any child (V3). The `working_directory_property_not_yet_run` code stays in `REFUSED_BY_NAME` while `inputFiles`, `outputFiles` and `cache` are refused by it. Recorded in `property-treatment.json` as `honoured`.
- **`inputFiles` runs in the prepare claim, after `namespaceFiles` (Task 7).** The block's `inputFiles` are rendered with the same context and split as a script task's are (inline contents; `kestra://` files looked up through the render scope; namespace-file URIs resolved by `resolve_ns_file`). `merge` lets every input name win over a namespace file of its name, and `write_archive` writes the contents and the stored inputs as one archive. A child's own `inputFiles` are not in this claim: a script child stages them after its snapshot copy-in, as it always has (`run_created`), so the child-level behaviour is the script task's, not a new path.
- **`outputFiles` is collected by the finish claim, and checked after each child (Task 7).** The finish claim reads the block's pointer (the last snapshot) and collects every file that a declared name matches (a literal name, or a `*`/`?` pattern in the file name, as `output_problem` accepts) through `collect_one` under the block run's own task prefix. Its `outputs` are `{"outputFiles": {name: kestra:// URI}}`, written with the block's fenced row update, and its files are indexed under the block run. A declared name that is a directory or a non-file fails the finish, naming it (plan Task 7 Step 1's test is a block that declares `sub`). Each child's copy-out is checked against the same names before its outcome is recorded (`check_child_outputs`), so a child that leaves a directory under a declared name fails by name and the block stops (spec §5.3). The check runs whatever the exit code, so a failed child is checked too (V2). Recorded, not measured: the spec's §5.5 says "outputs are written at the finish claim", and this keeps that, with the child check added as the plan's "child fails by name" rule requires.
- **Directory entries are named with a trailing `/` in an archive.** A declared name is matched against the entry name without it, so `sub` matches the directory entry `sub/`.

## 5. Design

### 5.1 Scope

- Model: `WorkingDirectory` keeps `tasks`, `namespaceFiles`, `inputFiles`, `outputFiles` and `cache`, each as Kestra's type. Nothing is dropped at parse.
- Planning: `TaskKind::WorkingDirectory { tasks }`, driven by `executor/flowable/working_directory.rs`, one dispatch arm in `executor/flowable/mod.rs`.
- The block's own claims: a **prepare** claim (cache restore, `namespaceFiles`, `inputFiles`, and writing an empty snapshot) and a **finish** claim (`outputFiles`, cache save, snapshot delete). Each reuses the W9a and W9b functions that a script task already uses (`FilesService.inputFiles`, the `outputFiles` collector).
- Each child: a normal leaf whose claim carries the block's snapshot. Before the handler runs, the worker copies the snapshot into the child's `/egret/working-dir`; after the handler, it copies the directory back out as the next snapshot.
- Templates: `{{ workingDir }}` in a child is `/egret/working-dir`, as in any script task.

### 5.2 Snapshot copies

- Out: the worker runs the same archive helper a script task's `outputFiles` already uses, over the whole directory, with the W9a size cap `max_task_bytes`; over the cap, the child **fails** and the block stops, naming the cap. A silent truncation is not allowed.
- In: the snapshot is streamed into the next child's container or pod with the same copy the input staging uses. A missing snapshot for a block that has run a child is a corruption (`Corrupt`), not an empty directory.
- A child that is skipped (`runIf` false, or `disabled`) takes no copy: the next child reads the same snapshot.

### 5.3 Child failure

- A child that fails without `allowFailure` stops the block's remaining children (V1). Its snapshot is still taken, and the finish claim still runs `outputFiles` and the cache save (V2), so the block's outputs and cache reflect the work done.
- A child with `allowFailure: true` that fails does not stop the block; the next child reads the snapshot taken after it.
- A failing prepare claim fails the block with no child run (V3).

### 5.4 Cache

- Restore is part of the prepare claim: the archive for `(namespace, flow, block id, task run value)` is looked up with the TTL rule of §2.3 and written into the directory before the first child.
- Save is part of the finish claim, with the same "changed since restore" rule, using the snapshot's directory state.
- The archive uses the same W9a bucket and its key shape, `{prefix}{ws}/{ns-as-path}/_cache/{flow}/{block id}/{hash of value}.zip` (the namespace segment is W9b's). A cache write is bounded by `max_task_bytes`, and an over-cap cache write **fails the finish claim**, as an over-cap snapshot does.

### 5.5 Outputs

- The block's `outputs` carries `outputFiles`, a map from each `outputFiles` match to its `kestra://` URI, as the image does (`outputs.wdir.outputFiles['out.txt']`, §2.1). The map is written at the finish claim and is read like any other task output.

### 5.6 What is not changed

- The `allowFailure:` flag (W8) and the `allowFailure` semantics of a leaf.
- The script handlers: a child is an unchanged leaf; it only sees a different starting directory.
- Flowable children: validate keeps refusing them (`working_directory_child_not_runnable`). Kestra's `Sleep` and `PurgeFlows` stay accepted, as now.
- Plugin storage and the KV bridge: a plugin child reads and writes the directory as it would as a leaf. Nothing new is bridged.

## 6. Acceptance

The current oracle cannot be the acceptance test for most of this (§1), so the plan adds the missing flows, and this is the bar.

1. **New corpus flows that use real types only**, not the test classes: at least one each for `inputFiles`, `outputFiles`, `cache` (restored on a second run), a child that fails with and without `allowFailure`, a child with `runIf` false, `namespaceFiles`, and a child that shares a file with the next child (the reason the block exists). Each is measured on the pinned image through the oracle's harvest, so its golden entry is Kestra's trace, not egret's output. The golden is never edited by hand.
2. **The existing four running WorkingDirectory valids stay matching**, and their baseline lines leave `baseline.txt` only if the trace matches.
3. **Every row of §5.3 has an executor test**: child fails and stops the block; `allowFailure` child continues; a skipped child takes no copy; a failing prepare fails the block with no child run; a failing finish fails the block.
4. **Mutations**: dropping the snapshot copy-in (the second child sees an empty directory) must fail a test; dropping the copy-out must fail a test; skipping the finish claim on child failure must fail a test (V2).
5. **Size**: a snapshot over `max_task_bytes` fails the child and names the cap (test with a small cap).
6. **Gates**: `cargo fmt --check`, `cargo clippy -p egret-workflow --all-targets -- -D warnings`, `cargo test -p egret-workflow`, and the oracle run in `oracle-exec/README.md`, on a private database. A live run of one flow with a real snapshot through a Docker runner is recorded, since the copy paths are runner-specific.
7. **The ratchet moves** only by the banked entries; no entry is added to the baseline.

## 7. Open points

1. **Kubernetes copy path.** §3 says a `files-out` helper exists for outputs. Copying a directory **in** a pod, for the snapshot's copy-in, is not built; the plan checks what the k8s runner has and measures it on a kind cluster before §5.2 relies on it.
2. **Crash leftovers.** A snapshot left by a crashed block is removed by the pruner's retention (`workflow.storage_retention_days`). The plan confirms the pruner reaches the `_wd` prefix, or adds a sweep like the staging sweep of W9b.
3. **Snapshot cost (measured on Kestra, the baseline).** A 100 MB file written by child 1 and read by children 2 and 3 takes Kestra 13.6 s for the writer, 43.9 s and 57.4 s for the readers (`wd-cost`), against about 1 s per child for a 1 KB file (`wd-cost-baseline`). So Kestra's own per-child copy in and out of the container already costs tens of seconds at this size. Egret's number, from the same probe on Docker, is measured in plan Task 6 Step 4 and must be at least comparable before option C is accepted.

**Egret's number (measured 2026-10-16, Docker on macOS, `working_dir_docker_live.rs`, one run each).** A child that writes a 1 MB file takes about 1.5 s (writer) and 1.3 s (reader). A child that writes 100 MB takes about 4.3 s (writer) and 7.3 s (reader). The figure is each child's span from its `created_at` to its last `updated_at`, so it includes the container's create and start. The copy cost is the difference from the 1 MB run: about 2.8 s out for 100 MB, about 6 s in. That is well below Kestra's 13.6 s, 43.9 s and 57.4 s, which measure the same bytes on a different host and a different harness, so it is a sign, not a proof of parity. Option C stays accepted on this evidence. Egret's per-child cost is one copy out and one copy in, as §4.1 says.
4. **`taskRun.value` in a loop.** A block inside a `Loop` takes the iteration's value as its cache object id (§2.3). The plan confirms the value reaches the block's claim.
5. **Kestra's trace of a skipped child** is measured (§2.1): a `runIf`-false child has a task run in state `SKIPPED` with one attempt. Egret's trace must match that shape, and the plan checks it against the golden entry once one exists.

## 8. Rules this package must meet

- The oracle is the acceptance test. A trace that differs from the image is not "close enough"; the mismatch goes into `baseline.txt` with its cause, or the code changes.
- No change to `oracle-exec/golden.json` by hand. New golden entries come from the harvest of new flows.
- Docs are bilingual: this spec and the plan get a Vietnamese reference in `docs/vi/`, written after the English is final (convention `docs-bilingual-convention`).
- No silent drop: a property egret does not run is refused at publish or fails at run, never ignored.

## 9. Not built here

- Plugin writes to namespace files and the KV bridge for plugins (the programme's remaining "still not true" items, unchanged).
- Masking KV values in logs.
- A shared volume (§4.1, option B).
- Flowable children of a `WorkingDirectory`.
