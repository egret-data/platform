# Task 14 report: rebase, end to end on kind, coverage, docs, full local CI

**Status: DONE_WITH_CONCERNS.** Branch `feat/workflow-w9a-internal-storage`, now 75 commits on `86233067` (origin/main at fetch time). HEAD **`1ea7f140`**. No push, no PR.

Commits added by this task (on top of the rebased 71):

| commit | what |
|---|---|
| `8bf24d59` | refactor: migrations renumbered 0161→0166, 0169→0167, 0170→0168, with every reference; `openapi.json` regenerated from the binary; frontend client regenerated with `npm run gen:api` |
| `fb7b6d07` | fix: two `outputFiles` refusal messages said "egret has no internal storage … W9 owns it" (`worker/commands.rs`) |
| `58d92e7e` | test: `k8s_runner_live`'s plugin cases run under egret's default 512Mi limit (they were OOM-killed under the suite's 128Mi) |
| `1ea7f140` | docs: README W9a section, programme spec §3/§9/§11, W5a/W5b/W9a spec corrections, EN+VI |

## 1. Rebase (Step 0)

- Before: HEAD `df234418`, tree `3efffc6b`, merge-base with origin/main `ecdf4868`. origin/main was `86233067`, 19 commits ahead. Backup ref kept locally: `w9a-t14-pre-rebase` → `df234418` (delete when no longer wanted).
- `git rebase origin/main`: 71/71 commits, **no conflicts** (git auto-merged the 6 overlapping files: `Cargo.lock`, `Cargo.toml`, `egret-bin/src/main.rs`, `egret-config/src/lib.rs`, the generated `model/index.ts`, `openapi.json`).
- Evidence the rebase reverted nothing of Data Apps and changed nothing of the branch:
  - The set of files in `git diff origin/main HEAD` equals the set in `git diff ecdf4868 df234418`: 117 files each (`w9a-t14-post-files.txt`, `w9a-t14-branch-files.txt`).
  - For each of the 117 files, the `+`/`-` lines of the branch's patch hash identically before and after: **117 identical, 0 differ**.
  - Of the 238 files origin/main changed since `ecdf4868`, every one the branch does not touch is identical to origin/main at the rebased tip: **0 differ**.
- Migrations: origin/main's highest is `0165_data_apps_origin.sql`. Renamed with `git mv`, order kept: `0166_workflow_execution_files`, `0167_workflow_loop_file_values`, `0168_workflow_execution_files_pruned`. Each header now says it was renumbered and why. References updated: 8 Rust comments (`loop_.rs`, `routes.rs`, `prune.rs`, `uri.rs`, `claim.rs`×2, `spawn.rs`, two test files), the plan's file map (which also gained the missing 0167 row) and Task 3/13 paths, the W9a spec EN+VI (§4.2, §10 ×2, §12 ×2). No test asserts a migration file name (checked: only `egret-metastore`'s `migrate!` reads the directory).
- **Concern:** the local, unpushed branch `feat/sp4d-debt-and-guardrails` also holds `0166`-`0168`. Whichever merges second must renumber.
- `openapi.json`: regenerated from `cargo run -p egret-bin -- openapi`. Diff: 5 lines. One is the renumbered migration in a route description. **The other is a pre-existing defect: the committed file was already stale at `df234418`** — W9a Task 11 fix round 1 added a `TaskRunDto.rendered` doc comment and never re-emitted. `committed_openapi_json_is_current` passes now. `npm run gen:api` changed `model/taskRunDto.ts` and `workflow/workflow.ts`.
- Test template: `egret_test_template` on :5438 was dropped and rebuilt by the first test run.
- **Note:** origin/main moved again during the task, to `28cebc41` (#629, Data Apps P6a: no migrations, no `openapi.json`). I did not rebase a second time, because that would have invalidated the gate. A final rebase before merge is needed.

## 2. End to end on a temporary kind cluster (Steps 1–3)

Cluster `egret-w9a-e2e` (kind v0.29, k8s v1.33.1, kindnet), created with **its own kubeconfig file** (`--kubeconfig` in the scratchpad), so the global context was never switched. It was deleted at the end (`w9a-t14-e2e-kind-delete.txt`). `kind get clusters` then listed only `egret`, and the current context is `kind-egret`.

- Images: `egret-backend:w9a-e2e`, built from the rebased tree with the renumbered migrations; it does not contain the `fb7b6d07` message change. Also `egret-plugin-host:dev` and `egret-task-helper:dev`, rebuilt. `kind load` failed for multi-arch pulled images (`postgres:16`, `alpine:3.20`, `python:3.13-slim`, MinIO) with a containerd digest error, so those went in with `docker save --platform linux/amd64 | ctr import`.
- **MinIO**: `quay.io/minio/minio` and `mc` answered **401** again (`w9a-t14-e2e-minio-pull.txt`). The local `minio/minio:latest` is the same release, `RELEASE.2025-09-07T16-13-09Z`, and has `mc` `RELEASE.2025-08-13` inside. I retagged it `egret-w9a-minio:2025-09-07` and used it as both `deps.minio.image` and `mcImage`.
- Chart from the branch: `containerRunner=kubernetes`, `workerEnabled=true`, `pluginHost.image=egret-plugin-host:dev`, allowed images `alpine:3.20` and `python:3.13-slim`, the kind CIDRs, mailpit off. Namespace `egret-engines` had to be created by hand (the chart expects it). Postgres 16 ran in `e2e-data`. Frontend and renderer pods stayed ImagePullBackOff, which does not matter for the API.

Check by check (transcripts `w9a-t14-e2e-*.txt` and `w9a-t14-e2e-4-pod.yaml`):

1. **FILE input → script → outputFiles → read() → later task → file API** (`-1-files.txt`):
   - The multipart start returned 201. Input URI: `kestra:///e2e/w9a/files/executions/febcc375…/inputs/f/in.csv`.
   - Task `s` succeeded. `outputFiles` = {`in.csv`, `out.csv`} as URIs; the glob also caught the staged input.
   - The `Log` line read `READ=ID,NAME\n1,ALPHA\n2,BETA\n3,GAMMA\n`.
   - **A later task staged `outputs.s.outputFiles['out.csv']` through the real snapshot path** (Task 5's open check): `LATER 31 653fb20d…caff5`. That equals the sha256 of the expected file.
   - File API: download HTTP 200, 31 bytes, `cmp` equal to the expected bytes, `attachment` disposition; the input download equals the upload; metas `{"size":31}`; preview `maxRows=2` → `["ID,NAME","1,ALPHA"]`, truncated.
   - The bucket keys are `workflow/{ws id}/e2e/w9a/files/executions/{id}/…`.
2. **Plugin STORE chain** (`-2-plugins.txt`):
   - Query STORE: `size: 500`, `.ion`. Then Batch from the URI (`rowCount 500`), CopyOut (CSV), and CopyIn from the CopyOut URI (`rowCount 500`).
   - In Postgres, `src`, `dst` and `dst2` each hold 500 rows with sum 125250, and `src EXCEPT dst2` is empty.
   - **Deviation from the plan:** "Query STORE → CopyIn of that URI" cannot work, because the STORE file is binary Ion and CopyIn reads CSV (Task 9 measured the same). The chain goes through Batch and CopyOut instead.
3. **core.storage.Write → Loop → Size** (`-3-loop.txt`):
   - 3 iterations with `loop_value` `{"n":1,…}` / `{"n":2,…}` / `{"n":3,…}` and `Return` `0 - …`, `1 - …`, `2 - …`.
   - The loop run's `rendered` is URI + `\n3`.
   - `Size` = 55, which equals the content's bytes.
4. **No storage credential in the pod** (`-4-pod.yaml`, taken while the pod was Running during `sleep 20`; 8091 bytes):
   - **0** occurrences each of `egretsecret`, `minio`, `9000`, `STORAGE`, `ACCESS_KEY`, `SECRET_KEY`, `s3://` and `warehouse`.
   - No value equals the access key `egret`. The only env is `WORKING_DIR`.
   - Controls in the same dump: `alpine:3.20` 2, `egret-task-helper:dev` 4.
5. **Second workspace** (`-5-cross-ws.txt`): `w9a2`, with the same namespace and flow id, ran `read('<w9a1 URI>')`.
   - The task FAILED with "… belongs to an execution this flow may not read …", naming the URI.
   - File API: 404 `not found: execution` for w9a1's execution id; 404 `no such file of execution …` for the URI under w9a2's own execution.
   - Control: the same request in w9a1 returned 200.
6. **Retention** (`-6-retention.txt`): `febcc375`'s `updated_at` was set to now − 2 days, then `EGRET_WORKFLOW__STORAGE_RETENTION_DAYS=1` (which restarts the pod; the first tick is immediate).
   - Log: `workflow storage pruned executions=1 orphans=0 failed=0`.
   - Index rows 3 → 0, and `files_pruned_at` is set.
   - Bucket objects 6 → 3, with 0 left for that execution; the other executions' 3 objects stayed.
   - File API download and metas: 404 "… was deleted: files are kept 1 days after their execution ends". An unpruned file: 200.
   - A manual replay was refused with 400, naming the pruned FILE input.

Found live:
- **`k8s_runner_live` `a_plugin_stores_and_reads_storage_in_a_pod` failed: 21/22 in the suite, and 2 of 3 runs alone.** The kernel log shows a memory-cgroup OOM at `usage 131072kB, limit 131072kB` with java anon-rss 129 MB. The suite's pod limit is 128Mi, and the plugin JVM runs with `-Xmx384m`. This is a test-config defect from Task 9, not a product defect: the chart default is 512Mi, and all 5 plugin pods in the e2e succeeded. Fixed in `58d92e7e`. After the fix: 5/5 alone, then the whole suite **22 passed, 0 failed** (`w9a-t14-k8s-plugin-rerun.log`, `w9a-t14-k8s-live2.log`).
- Wording (not fixed, minor):
  - The readable-set refusal lists own / loop family / trigger / replay source, and omits the trigger's loop family.
  - The pruned message says "1 days".

## 3. Coverage and docs (Step 4)

- `oracle-exec/README.md` has a new section, "W9a — internal storage, and the one entry it banked", before "What this ratchet does not prove". It covers:
  - the ratchet, **109 → 108, measured**, with a per-task table (figures taken from each task's ratchet log);
  - the 10 corpus flows (5 FILE input, 1 outputFiles, 2 STORE, 2 core.storage), classified against `harvest-run.txt`, golden and baseline. Only `valids/loop-values-from-uri` moved;
  - what the driver sees (`storage::` 5 times in `driver.rs`, against `worker::` 30);
  - the release note;
  - the 6 e2e checks;
  - the gated live tests table;
  - what was not measured (NetworkPolicy, since kindnet does not enforce it; a live S3 outage; Trino STORE; large files through a pod on kind);
  - the sweep table.
- **Release note** (outputFiles contents → URIs, behind `workflow_containers` which is off by default, read with `read()`; a container runner requires `[storage]`; the helper image needs `cat`; the W5a/W5b behaviours that changed). It appears in both the README section and the programme spec's new W9a correction, EN and VI.
- Programme spec EN+VI:
  - §3 finding 4 now says the bridge exists (dated);
  - the W5 row has a dated note;
  - the W9 row is split into W9a/W9b/W9c (user decision 2026-10-01);
  - the W5b correction's "waits for W9" is annotated as retired for storage;
  - a new dated **W9a correction** paragraph;
  - §11 Q1 says object storage for log lines is still not built (W9a spec §14), dated.
- W5b spec EN+VI: 8 dated "Superseded 2026-10-04, W9a" notes (§1 row, shim, exit 4, publish refusal, catalog note, test-table row, §14, §15).
- W5a spec EN+VI: dated notes on the divergence row and the properties line.
- W9a spec EN+VI:
  - **Task 13's carry**: §10 now says the pre-delete re-check sees **direct** links only (a replay of a replay is not seen); §15 says the residual window spans the family's store deletes (one `delete_prefix` per member) plus the row statement, "not one statement". Both dated.
  - Status line updated.
  - The stale header reads-bullet (Task 1 deferred M3) is marked superseded by the rulings, dated.
  - The other corrections the plan lists (render-time lookup, ForEach = Loop, loop cap, Task 1's contradictions) were already in the spec from Tasks 1/11. Checked; nothing left.

Sweep (`/usr/bin/grep` with file arguments, at the pre-edit rebased tip `d3cce769`, extracted with `git archive`):

| # | population | examined | corrected |
|---|---|---:|---:|
| 1 | `\bW9\b` in `crates/egret-workflow/src`, `plugin-host/src`, both programme specs | 20 lines (src 8 = functions.rs 6 + commands.rs 2; plugin-host 0; spec EN 6, VI 6) | 2 messages (commands.rs); per spec 4 lines + W5b-correction annotation + new correction; left: functions.rs 6 (deliberate: `http`→W9, `fileURI`→W9b, `kv`→W9c), history lines |
| 2 | "1 MiB"/"contents" about outputFiles | 49 `1 MiB` lines + 13 `outputFiles`+content lines; 15 state what outputFiles holds | 4 (W5a EN+VI ×2); 11 left (historical tables, W9a spec text that describes the change, container.rs already correct, plan) |
| 3 | "no internal storage" | 21 (code 14, plugin-host 5, plan 2) | 1 (commands.rs, also in pop. 1); 20 are conditional "… configured ([storage])" and true |

## 4. Full local CI (Step 5)

| check | result | vs baseline |
|---|---|---|
| `cargo fmt --all --check` | exit 0, empty | = |
| clippy, no `-D` (`w9a-t14-clippy-warn.log`) | exit 0, **16** warning locations | **0 new.** All 16 baseline locations are still present. Two moved by line shifts only, with the same lint: `egret-bin/src/main.rs:275→276` (auto-deref, `&mut **tx`) and `tests/executor.rs:5280→5349` (too many args 9/7). Gone: none |
| clippy `-D warnings` (`w9a-t14-clippy-D.log`) | exit 101, 2 error locations: `egret-workspace/src/members.rs:191:73`, `:196:75` | = baseline (dependents unlinted, as known) |
| `cargo test --workspace --no-fail-fast`, :5438, all 4 S3 vars, StarRocks, shared store, Docker (`w9a-t14-gate-raw.log`, HEAD `58d92e7e`, 59 min) | **5402 passed, 3 failed, 31 ignored** (354 result lines) | the 3 failures are **pre-existing on origin/main, measured** (below) |
| egret-workflow in that run | **1804 passed, 0 failed, 25 ignored** (+1 doctest ignored = 26) | = Task 13's 1804/0/26 |
| live suites inside it | container_docker_live 23/0, storage_render_s3_live 6/0, storage_files_api_s3_live 1/0, storage_prune_s3_live 1/0, egret-storage s3_stream 11/0, sql_starrocks_live 9/0, `committed_openapi_json_is_current` ok | |
| ratchet `executor_matches_golden --nocapture` | **294 / 108 mismatching / 108 baselined / 0 broken** | 109 at base → 108 |
| `just test-shipping-resolution` equivalent (`-p egret-bin -p egret-workflow --lib --test oracle`) | 3 + 1182 + 10 passed, 0 failed | |
| `k8s_runner_live` on `egret-w9a-e2e` (`EGRET_TEST_KIND_CONTEXT`) | before fix 21/1; after **22/0** | |
| `plugin_host_live --ignored` (EGRET_PLUGIN_HOST_LIVE, S3, StarRocks) | **3 passed, 0 failed**, "ENGINES SKIPPED: 0 of 2" twice | |
| `mvn -f plugin-host/pom.xml verify` (Java 25) | BUILD SUCCESS, **40 tests, 0 failures** (EgretPluginHostTest 19, EgretStorageTest 21) | |
| `helm lint deploy/helm/egret` | 1 chart linted, 0 failed (INFO: icon) | |
| frontend `npm run gen:api` | 2 generated files changed, committed in `8bf24d59` | |
| `npm run lint` | 0 errors, 177 warnings | = Task 8's 177 |
| `tsc --noEmit` | exit 0 | |
| `npm test` (vitest) | 5599 passed, 5 failed (all `Test timed out in 5000ms`; load average 32–43 from other sessions) | Re-run of the 4 files alone: 3 pass; `ChartExplore.dirty` failed 2 alone, then 7/7 **twice**. The branch's frontend diff is generated API files only. These are the known ChartExplore mount timeout flake class, not this branch's |

**The 3 workspace failures, measured on the rebase base `86233067`** (a detached worktree, private target dir, throwaway Postgres :5439; `w9a-t14-baseline-main-3.log`). All 3 fail there identically:
- `egret-perm-compile --test routes` `explain_reports_the_tag_set_the_evaluator_was_handed` (52/1 there as here);
- `egret-perm-compile --test wire_dtos` `cause_access_priority_composite_rides_the_arbitration_wire_shape` (60/1);
- `egret-test-db --test every_test_recipe_refuses_the_skip` (`justfile:95-96` Ranger recipes lack `{{ no_db_skip }}`).

The branch's diff touches none of `justfile`, `egret-perm-compile`, `egret-test-db` or `egret-policy`. These are origin/main's defects; report them to the Data Apps / SP4 owners.

S3 safety: every MinIO-gated test writes under its own random throwaway prefix. `s3_stream`'s empty-key refusal targets the non-existent bucket `w9a-t2-no-such-bucket`. Nothing touched the `warehouse` root. The e2e used the cluster's own MinIO.

## 5. Progress lines

**Carry to Task 14, addressed:** Task 13 m2 §10/§15 wording (done, EN+VI). Task 5's "verify a real Snapshot::take staging `{{ outputs.w.uri }}`" (e2e check 1, `later` task). Task 9's "W5b spec still describes storage refusals" (done). Task 1's "Task 14 clippy gate against the non-`-D` list" (done). Task 1 deferred M3, the header reads-bullet (done).

**Deferred minors, listed and not addressed:**
- T1 M1: the "clamped 5000" file-API finding is labelled measured but was not measured.
- T2: `MemStore::list` ignores bucket and delimiter; the abort test has a redundant `put_stream`; no single-PUT vs multipart ETag test, and no empty-body or exactly-8 MiB boundary test; the `raw_cfg_client_args` helper is superfluous.
- T4 M6: unpinned rules. Also: the walk's I3 arm has no `collect()` for "uri + trailing text"; M5 `taskrun.value` has no discriminating case.
- T5: a whitespace-padded `kestra://` value gives an unexplained "not looked up"; the stage order puts readability before the feature gate; the helper needs `cat` (now in the release note).
- T6:
  - same-key collision between `a.txt` and `$OUTPUT_DIR/a.txt` (twice: once that one is silently dropped, once that the directory file silently replaces the declared mapping);
  - hard links fail the task, where Kestra collects them;
  - the `.egret` inputFiles key message;
  - the fence test's rival panic shows as a timeout;
  - the outage test relaxes `reap >= 1`.
- T7: no test for the multipart 5xx→400 arm; 8 KiB pacing.
- T8:
  - no Content-Length on download;
  - text detection by bytes;
  - CSV/JSONL rows come back as strings;
  - about 0.4% of valid text Ion with brackets in comments is falsely refused (known limit);
  - ion-rs pinned at =1.0.1.
- T9: a whole-leaf `kestra:///` value the plugin never reads still fails (doc line); a stale "decoded path" javadoc in `EgretStorage.java`; mixed IAE vs IOException refusals.
- T10: the ionfmt non-finite branch; the store_target fallback is test-only; the linear-time ratio test should interleave its reads.
- T11: the values file is read while the settle transaction is open; a null record fails before any iteration.
- T12 known limits: `(` in a comment over-counts; the non-append LST is charged by its tail; worst case about 8× cap per value.
- T13: during long outages each pass retries the known-failed families first.
- New from T14: the readable-set message omits the trigger's loop family; "1 days".

**FOR FINAL REVIEW, listed:**
- T4: permanent store errors (AccessDenied, missing bucket) are classed as an outage, so the run is re-claimed forever with no backoff.
- T6/T12: the worker lease timing tests are flaky under load. Not hit in this gate: egret-workflow passed 1804/0.
- T7 M4: a replay refusal leaves the execution RETRIED, so a Flow trigger on FAILED never fires.
- T8: add `cargo deny`/`audit` to the gate; ion-rs downgrades generic-array.
- T10: clippy `result_large_err` (resolved in T10; still 0 in this run).
- T11: no record-count cap on Loop over a file; no FK from `(execution_id, run_ix)` to `task_runs`; a stale `runtime.rs` doc; the boxing of the third settle frame.
- T12: the stream write path has no timeout on a stalled store; a cancelled S3 `put_stream` may leave a multipart upload behind (the abort guard should cover it — unverified).

## 6. Open items / concerns

1. **The migration number clash with the local branch `feat/sp4d-debt-and-guardrails`** (0166–0168).
2. **origin/main moved to `28cebc41` after the rebase** (1 commit, no migrations or openapi). Rebase again before merging.
3. **`openapi.json` was stale before this task** (Task 11 fix round 1). It is fixed now, but a task gate let it through: those gates ran `-p egret-workflow`, not `egret-server`'s currency test.
4. **3 origin/main test failures** (`egret-perm-compile` ×2, `egret-test-db` recipe check), reproduced on `86233067`.
5. **The e2e image was built before `fb7b6d07`**, which only changes message text. No later code change touches the e2e paths.
6. **The vitest flakes are timeouts under load** (load average 32–43). The one file that failed alone passed 7/7 twice.
7. **The local backup branch `w9a-t14-pre-rebase` remains.**

## Fix round 1

- **I1** down-compat: `migrations/0166-0168` headers (comments only, no SQL changed) and the README release note now give the recovery for a dev/kind DB that ran the old numbers 161/169/170: `just reset-testdata`, or drop `workflow.loop_file_values`, `executions.files_pruned_at`/`files_swept_at` (takes `workflow_executions_unswept_idx`), `workflow.execution_files`, `executions.replayed_from_execution_id` (takes its FK, CHECKs, index) and delete the `_sqlx_migrations` rows matched on (version, description). The `feat/sp4d-debt-and-guardrails` collision (0166-0168) is stated in each header and the README.
- **I2** release note: README W9a section and programme spec (EN `:504`, VI `:493`, item 6) now carry retention (`workflow.storage_retention_days`, 30, 0 = forever, 404 / function errors), the file API routes (`file`, `file/metas`, `file/preview`, `workflow:read`, no new permission, 5000-row clamp), `[workflow.storage]` `prefix`/`max_file_bytes`/`max_task_bytes`/`max_read_bytes` with their defaults and boot validation, the 400 on replay of a pruned FILE input, the 512Mi limit vs `-Xmx384m` (128Mi OOMs), and the migration renumbering. The W9a spec has no release-note section of its own, so nothing to mirror there.
- **M2** `Unreadable::Pruned` pluralises ("1 day"); `Unreadable::NotReadable` now names the trigger's loop family. `tests/storage_files_api.rs` expected text updated; two unit tests added (`storage::message_tests`). README "found live" paragraph says both are fixed.
- **M3** README `EgretPluginHostTest (15 tests)` -> 19 (19 `@Test` in the file).
- **M5** `git range-diff ecdf4868..w9a-t14-pre-rebase 86233067..d3cce769`: 69 of 71 commits `=`; the two `!` are Task 8's `451095d0 -> 8377dbe3` and `7b674a93 -> ae7981af`. Both differ only in the context lines of hunks of `frontend/src/api/generated/model/index.ts` (the neighbouring `export * from` lines are origin/main's new models, e.g. `explainTagDecline`, `wireTagResourceProblem`); the added/removed lines are identical. Expected index overlap, nothing else.
- **Tests** (after dropping the stale `egret_test_template` on :5438, which the edited 0166 header invalidated: `VersionMismatch(166)`, 356 failures on the first run, all from that): `cargo test -p egret-workflow --no-fail-fast` **1806 passed, 0 failed, 26 ignored** (1804 + the 2 new message tests); `cargo test -p egret-server --lib` 6 passed (incl. the openapi currency test); `cargo fmt --all --check` clean.
- Note: any other checkout sharing a template DB with the pre-fix 0166 checksum must drop `egret_test_template` once.
