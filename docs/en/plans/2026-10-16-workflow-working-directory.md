# WorkingDirectory: implementation plan

**Spec:** `docs/en/specs/2026-10-16-workflow-working-directory-design.md`. Read it first; this plan refers to its sections (§2 Kestra, §3 egret today, §4 the design choice, §5 the design, §6 acceptance, §7 open points).

**Goal:** `WorkingDirectory` runs its children in one working directory, with `namespaceFiles`, `inputFiles`, `outputFiles` and `cache`, as Kestra does; each behaviour is measured on the pinned image before it is coded.

**Branch:** `docs/workflow-working-directory-spec` carries the spec (PR #682). Implementation goes on `feat/workflow-working-directory`, cut from `origin/main` after the spec is merged. A new worktree, not one that holds other work.

**Rules that apply:** the oracle is the acceptance test (spec §8). Never change `golden.json` by hand. One cargo or test suite at a time. Never use the shared dev stack (`:5432`, `deploy/docker-compose.dev.yml`, shared buckets, the shared kind cluster "egret"). DB-backed tests and any live run use a private database and a private stack, with a name that is not "egret". Commit by named paths; never `git add -A`, never bare `git stash`.

## Task 0: decide the open points (spec §7)

Three questions are the user's to answer before Task 4, because each changes cost or scope:

- [ ] **Option C or option B (spec §4.1).** This plan implements C, the snapshot. B (a shared volume) stays out unless Task 1 Step 4 shows C is too slow.
- [ ] **Kubernetes copy-in (spec §7.1).** If the k8s runner cannot copy a directory into a pod, the snapshot is Docker-only at first and the Kubernetes path is a separate task. Task 1 Step 5 measures it.
- [ ] **The size bound for a snapshot.** Default: `max_task_bytes` (5 GiB, W9a). Task 1 Step 4 may argue for a lower default for snapshots.

**Done when:** each question has an answer written into spec §7, or the plan's default is kept and the answer says so.

## Task 1: measure on the image (spec §2.1, §5.2, §7)

Nothing is coded until this is done. The corpus cannot do it (spec §1): its WorkingDirectory flows use test classes the artifact refuses. So the measurement uses **new flows with real types**, run the way the oracle runs a flow.

- [x] **Step 1: write the probe flows** (done 2026-10-16, in the session scratchpad, not the repo; they move into `corpus/` only in Task 8). Real types only: `io.kestra.plugin.scripts.shell.Commands` on the **Docker** runner (`io.kestra.plugin.scripts.runner.docker.Docker`, `alpine:3.20`). The Process runner is not used: egret refuses `io.kestra.plugin.core.runner.Process` at save (`scripts/mod.rs`), so a probe on it would measure a flow egret cannot run. Flows: (a) `inputFiles` and `outputFiles`; (b) `cache` with a `ttl`, run twice; (c) a child that fails without `allowFailure`, followed by a sibling; (d) the same with `allowFailure: true`; (e) a child with `runIf` false between two children; (f) a `namespaceFiles` block on a `WorkingDirectory`; (g) a file written by child 1 and read by child 2.
- [x] **Step 2: run each probe** (done: the private compose project `wd-probe`, removed after; the image and compose file are the oracle's) on the pinned image (`kestra/kestra@sha256:28f59bf175e1…`) in a throwaway compose project named `wd-probe`, with the oracle's `docker-compose.yml` settings, using the same normalisation as `normalise.py`. Record every task run, its state and parent id, and the outputs. Do not reuse the checkout's jars. Tear the project down with its volumes.
- [x] **Step 3: record V1–V4 and the skipped trace (spec §2.1, §7.5).** Done: V1, V2, V3 confirmed; V4 not measured (standard behaviour); the skipped child has a `SKIPPED` task run. No fact was contradicted, but the output key was: `outputs.wdir.outputFiles`, not `uris` (the spec is corrected).
- [x] **Step 4: measure the snapshot cost (spec §7.3).** Done for Kestra's baseline (100 MB: 43.9 s and 57.4 s per reader child; 1 KB: about 1 s). Egret's number is still to be taken in Task 6 Step 4. One probe with a directory of about 100 MB written by child 1, read by child 2 and child 3, run on Docker with the Process runner, timed. Egret's cost is measured the same way in Task 6. Write the number into spec §7.3.
- [ ] **Step 5: measure the Kubernetes copy-in (spec §7.1).** Not started; needs a private kind cluster `egret-wd`, which is a separate step after the Docker path is coded. On a **private** kind cluster (name `egret-wd`, not `egret`), check whether the k8s runner's file path can copy a directory into a running pod, the way `files-in` copies inputs. Write the answer into spec §7.1. If it cannot, the Kubernetes path is a separate task and this plan's acceptance is Docker-only.
- [x] **Step 6: write the measured results into the spec** (done in spec §2.1, §5.5, §7.3, §7.5); commit by named path pending.

**Done when:** every fact in spec §2.1 is verified or the spec is changed; §7.1 and §7.3 have numbers or a decision.

## Task 2: keep every property at parse (spec §3, §5.1)

- [ ] **Step 1: failing test.** In `models/task.rs` tests: a `WorkingDirectory` YAML with `inputFiles`, `outputFiles` and `cache` parses and round-trips with all four fields present, not dropped.
- [ ] **Step 2: add the fields** to `models::WorkingDirectory` (`models/task.rs:580`): `input_files`, `output_files`, `cache`, each with Kestra's shape (`Object`, list of globs, `Cache { patterns, ttl }`). Keep `namespace_files` as it is (W9b).
- [ ] **Step 3: property treatment.** Update `spec/schema/property-treatment.json` for the three properties: they move from "dropped" to "kept". The diff is only those three entries.

**Done when:** the parse test passes and the treatment file says "kept" for the three properties.

## Task 3: plan the block as a flowable (spec §4.2, §5.1)

- [ ] **Step 1: failing plan test.** A flow with a `WorkingDirectory` of two children plans as `TaskKind::WorkingDirectory { tasks }` with the two children, not as an opaque `Runnable`.
- [ ] **Step 2: add the variant** to `TaskKind` in `executor/plan.rs`, mapped in `flatten_flow` beside `Sequential` and `AllowFailure`.
- [ ] **Step 3: validate.** `spec/validate.rs` (`walk_task`, the `WorkingDirectory` arm at ~1663) keeps its refusals. Remove any refusal that only existed because the block was opaque; the refusal of a non-runnable child stays.
- [ ] **Step 4: update the census.** Tests that count the block as opaque move with it, each with a comment: `executor/mod.rs` around 3390-3392, 3546 (`K::WorkingDirectory(_) => PlannedAs::Opaque`), 3589, 3645-3646, 3730, 3975, and 4084 (the "13 WorkingDirectory" note). The derived census numbers in `tests/executor.rs` are re-derived from the planning, not relaxed; each change says why.

**Done when:** `cargo test -p egret-workflow` passes the plan and validate tests, and every census change has its derivation in its assertion message.

## Task 4: the snapshot as pure code (spec §4.3, §5.2)

- [ ] **Step 1: failing test** for `snapshot::pack(dir) -> Vec<u8>` and `snapshot::unpack(bytes, dir)`: a directory with nested files, an empty directory and a symlink (refused, not followed) round-trips; a directory over the cap returns `SnapshotTooLarge { cap, size }`, naming the cap.
- [ ] **Step 2: implement** `worker/snapshot.rs` with a tar writer over `storage::transfer::write_tar` (`storage/transfer.rs:90`), a size check before any copy, and no path escape (`..` and absolute names refused on unpack).
- [ ] **Step 3: mutation check.** Remove the cap check; remove the symlink refusal; allow `..` on unpack. Each must fail a test. Restore after each.

**Done when:** the round-trip, cap and escape tests pass and each mutation is killed.

## Task 5: the block's driver (spec §4.2, §5.1, §5.3)

- [ ] **Step 1: failing executor test** for the shape of spec §5.3: child 1 fails without `allowFailure`; child 2 has no run; the block is `FAILED`; the finish claim ran (`outputs` written). A second test: child 1 with `allowFailure: true` fails; child 2 runs; the block is the child-2 result.
- [ ] **Step 2: the driver** `executor/flowable/working_directory.rs`, built on `sequential::drive` for the children, with a prepare step before the first child and a finish step after the last one, whether or not a child failed (spec V2). Add one dispatch arm in `executor/flowable/mod.rs`.
- [ ] **Step 3: the prepare and finish claims** are the block's own claims (spec §5.1). They carry no child and no handler; the worker runs them with the functions of Task 6 and Task 7.
- [ ] **Step 4: mutation check.** Skip the finish claim on child failure (must fail); let a failed child without `allowFailure` continue (must fail); run a child with `runIf` false (must fail, since it takes no copy and no run).

**Done when:** both executor tests pass and each mutation is killed.

## Task 6: the child's starting directory (spec §5.2)

- [ ] **Step 1: failing worker test** on the Process runner: child 1 writes `a.txt`; child 2 reads it. Without the copy the test fails.
- [ ] **Step 2: copy in and out** through the runner seam (`worker/commands.rs`, `TaskRunner`). Before a child, the snapshot is written into the child's `/egret/working-dir`; after it, the directory is packed into the next snapshot. Docker uses the input staging path (`docker cp`), Kubernetes the `files-in`/`files-out` path only if Task 1 Step 5 says it works.
- [ ] **Step 3: a missing snapshot** for a block that has run a child is `WorkerError::Corrupt`, not an empty directory (spec §5.2).
- [ ] **Step 4: cost check.** Repeat the Task 1 Step 4 probe with egret, and record the number beside Kestra's.

**Done when:** the Docker test passes, the missing-snapshot test passes, and the cost number is in the spec.

## Task 7: `namespaceFiles`, `inputFiles`, `outputFiles` and `cache` (spec §2.2, §2.3, §5.4, §5.5)

- [ ] **Step 1: failing tests** for each: `inputFiles` writes its entries before child 1 (reusing `FilesService.inputFiles`'s egret counterpart); `namespaceFiles` loads before `inputFiles` and `inputFiles` wins a collision; `outputFiles` globs the directory after the last child and writes `outputs.<block>.uris`; `cache` restores on the second run and saves only when a matched file changed.
- [ ] **Step 2: implement** on the prepare and finish claims, using the W9a/W9b functions a script task already uses. The cache key is `(namespace, flow, block id, hash of the task run value)` in the W9a bucket (spec §5.4). An over-cap write fails the finish claim.
- [ ] **Step 3: mutation check.** Load `namespaceFiles` after `inputFiles` (the collision test must fail); save the cache unconditionally (the "unchanged" test must fail); ignore the TTL (the expiry test must fail).

**Done when:** each test passes and each mutation is killed.

## Task 8: the oracle (spec §6)

- [ ] **Step 1: move the probes into the corpus.** The flows of Task 1 Step 1 move from `corpus/probes/` to `corpus/valids/`, one per behaviour, with names `working-directory-real-*`.
- [ ] **Step 2: harvest.** Run `harvest.py` as the README documents (about 35 minutes; it rewrites committed artifacts, so it runs on the private stack only). The new flows get golden entries from Kestra's trace.
- [ ] **Step 3: update every census number that counts the corpus.** The corpus grows from 345 flows; `tests/executor.rs` holds that count in several places (lines 653, 925, 1331 and the derivations near them) and `harvest.py` has guards that name counts. Each change is re-derived from the harvest, not relaxed, and each assertion's message says the derivation.
- [ ] **Step 4: check the golden for the new flows against the probes of Task 1.** A mismatch goes back to Task 1 Step 3 and the spec; the golden is not edited.
- [ ] **Step 5: the ratchet.** Remove from `baseline.txt` only the entries whose traces now match. The 4 golden WorkingDirectory valids are checked the same way. Add nothing.

**Done when:** the ratchet is at or below its starting value, every new flow matches the image, and no census number is unexplained.

## Task 9: gates and the live run (spec §6.6, §6.7)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] `cargo test -p egret-workflow --no-fail-fast`, one suite at a time, on a private database. Expect the 7 `sql_starrocks_live` failures without `EGRET_TEST_STARROCKS_URL`, and report them as environmental, as before.
- [ ] The oracle run of Task 8, recorded with its counts.
- [ ] A live run of one flow with a real snapshot through the Docker runner on the private stack, recorded.
- [ ] Commit by named paths, conventional message, `Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>`.

## Task 10: docs

- [ ] Correct `docs/en/specs/2026-09-01-workflow-programme-design.md` §9: a dated correction that WorkingDirectory runs, and the "still not true" list loses its WorkingDirectory item. The correction names what is still not true (the Kubernetes path if Task 1 Step 5 failed; any open point left).
- [ ] Vietnamese references of the spec and this plan in `docs/vi/`, after the English is final.

## Out of scope

- Plugin writes to namespace files and the KV bridge for plugins.
- Masking KV values in logs.
- A shared volume (spec §4.1, option B), unless Task 0 decides otherwise.
- Flowable children of a `WorkingDirectory`.
- Retry on the block itself (the AllowFailure non-goal, for the same reason).
