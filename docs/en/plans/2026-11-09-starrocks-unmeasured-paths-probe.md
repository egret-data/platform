# StarRocks unmeasured paths probe (package C): implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** measure whether StarRocks 4.1.7 enforces egret's security model on four unmeasured
paths:
1. Java and Python UDFs;
2. the Hive, JDBC and Paimon catalogs;
3. stream load and routine load;
4. the CN/BE ports from inside the engine namespace.

Each check runs against the official and the egret allin1 image. Every finding is recorded as
enforced, leak or out of model, and every leak gets a follow-up id. No patch and no product code is
written.

**Spec:** `docs/en/specs/2026-11-09-starrocks-unmeasured-paths-probe-design.md` (and its `docs/vi`
mirror). Its open questions must be answered before Task 8 publishes anything. Questions 1, 2 and 6
also shape Tasks 1, 2 and 6.

**Repositories and branches:**
- **Fork:** `egret-data/egret-starrocks`, checkout `egret-starrocks/`, branch `test/probe-unmeasured-paths`
  from `main` d77446e. All harness work goes here.
- **Platform:** this repository, branch `docs/starrocks-probe-report`, for the report (Task 8).
- **Docs:** `egret-data/docs`, branch `docs/starrocks-probe-known-issues`, for the out-of-model
  known issues (Task 8).

**Architecture.**
- **Shared start-up.** `tests/e2e/stack.sh` holds the start-up that `run.sh` and the new
  `probe.sh` share.
- **Host-side probes.** Probes 1 to 3 are Python files run from the harness venv on the host, as
  `checks.py` is. They use the path helpers moved into `tests/e2e/sqlpaths.py`, and the verdict
  model in `tests/e2e/probe_lib.py`.
- **In-namespace probe.** Probe 4 is `tests/e2e/inns_checks.py`. It runs in a throwaway
  `python:3.12-slim` container on the probe network, with no credentials and only the standard
  library.
- **Test fixtures.**
  - `tests/e2e/canary.py`, on the host, serves fixture files and logs egress;
  - `tests/e2e/fakeranger.py` re-reads its policy file, so a run can revoke a grant.
- **Raw output** goes to the measuring session's scratchpad. The report cites it.

## Global Constraints

- **No product code.** Nothing under `crates/`, `frontend/` or `deploy/` changes. In the fork,
  nothing under `patches/`, `docker/` or `scripts/` changes.
- **The existing gate stays as it is.**
  - `tests/e2e/run.sh` and `checks.py` keep every check, with the same names, tags and expected
    values.
  - The refactor in Task 1 is proven by identical check lines before and after, on the egret
    image.
  - Probe checks never go into `checks.py` in this package.
- **Docker.**
  - Start only containers named `egs-probe-*`, on the network `egs-probe-net`.
  - Never prune, and never touch containers you did not start.
  - Never run `probe.sh` and `run.sh` at the same time: both bind the fake Ranger port 18777 that
    `ranger-starrocks-security.xml` names.
  - Never run two test suites on the machine at once. Ask the controller before any Docker run.
- **Images:**
  - every image tag is pinned exactly in `probe.sh`, and its digest is recorded in the report;
  - StarRocks runs only as `starrocks/allin1-ubuntu:4.1.7` (the official comparison) or
    `ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4`.
- **Destructive checks** (N4, N5, N11) run only inside the throwaway allin1 container of a probe
  run, and N11 runs last. On kind, N11 never runs.
- **Claims:**
  - every verdict in the report is printed by a command the report names;
  - a refusal counts as `ENFORCED` only when the check's sight control succeeded;
  - "untestable" is written down as a chosen boundary, with its reason.
- **Typed opt-out only.** A probe is skipped only through `PROBE_SKIP`, parsed into the `Probe`
  enum; an unknown name exits 2. A missing service is an `ERROR`, never a skip.
- **Disclosure.** Until the user answers spec open question 1, commit everything on the local fork
  branch and push nothing.
- **Python style:** the style of `checks.py`: stdlib and `requirements.txt` pins only, scripts run
  with `python -I`, and a broken check reports instead of stopping the run.
- **Commits:** Conventional Commits, each message ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

### Task 1: Harness: shared start-up, the probe runner, the verdict model and the fixtures

**Files (fork):**
- `tests/e2e/stack.sh` (new)
- `tests/e2e/run.sh` (sources `stack.sh`)
- `tests/e2e/sqlpaths.py` (new)
- `tests/e2e/checks.py` (imports `sqlpaths`)
- `tests/e2e/probe.sh` (new)
- `tests/e2e/probe_lib.py` (new)
- `tests/e2e/canary.py` (new)
- `tests/e2e/fakeranger.py`
- `tests/e2e/policies.py`

**Produces:**

```python
# probe_lib.py
class Verdict(enum.Enum):
    ENFORCED = "ENFORCED"; LEAK = "LEAK"; BLIND = "BLIND"; NA = "N/A"; SKIP = "SKIP"; ERROR = "ERROR"

class Probe(enum.Enum):
    UDF = "udf"; PYTHON_UDF = "python-udf"; HIVE = "hive"; JDBC = "jdbc"; PAIMON = "paimon"
    STREAM_LOAD = "stream-load"; ROUTINE_LOAD = "routine-load"; IN_NAMESPACE = "in-namespace"

def skipped_probes(env_value: str) -> set[Probe]: ...   # raises SystemExit(2) on an unknown name

@dataclass
class ProbeCheck:
    id: str              # "U5", "C14-paimon", ...
    probe: Probe
    name: str
    sight: Callable[[], bool]          # True when the harness can see the leak content
    actual: Callable[[], object]
    leaked: Callable[[object], bool]   # True when `actual` carries what policies refuse
    na_reason: Callable[[], str | None] = lambda: None

def run_checks(checks: list[ProbeCheck], skipped: set[Probe]) -> int: ...  # prints lines + summary
```

**Requirements:**
- **`stack.sh`.** Move every start-up step of `run.sh` into named functions, with names, ports and
  the image as variables:
  - the network;
  - RustFS;
  - iceberg-rest;
  - the fake Ranger;
  - the conf extraction and edits;
  - the StarRocks container;
  - the wait loops;
  - the seed SQL;
  - the AuditLoader install;
  - the policy-load and tag-load waits.

  `run.sh` keeps its variables and its order and calls the functions. Its checks and audit-checks
  sections are unchanged.
- **`sqlpaths.py`.** Move `connect`, `norm`, `run`, `text`, `sql_prepare`, `binary`, `http`,
  `flight`, `PASSWORDS` and `DENIED` out of `checks.py`:
  - `PORT`, `HTTP_PORT` and `FLIGHT_PORT` come from a `configure(port, http_port, flight_port)`
    call;
  - `checks.py` calls `configure` from `sys.argv` and keeps its `CHECKS` list byte-for-byte;
  - `run`'s DENIED error codes stay 5203, 5204 and 5210 to 5214.
- **`probe.sh <allin1-image>`.**
  - **Names and ports:** `NAME=egs-probe` and `egs-probe-net`; host ports 19230 (MySQL), 19231
    (FE HTTP) and 19240 (BE HTTP 8040); Flight 19508 and 19519; canary 18778.
  - **Start-up:** source `stack.sh`, then call `policies.py --probe`, so `udfdev`, `loader` and
    `nat` exist with their policies.
  - **Seeding:** start the services for the probes that are not skipped, run each probe's seed
    function, then run the probe files in the order udf, catalogs, loads, in-namespace.
  - **Output:** print the combined summary and exit 0 unless some check is `BLIND` or `ERROR`.
  - **Variants:** `BE_HTTP_AUTH=1` appends `enable_http_auth = true` to `be.conf`.
    `UDF_JAVA_POLICY=1` adds `-Djava.security.policy=/opt/egret-probe/deny-all.policy` to
    `be.conf`'s `JAVA_OPTS` and mounts `tests/e2e/udf/deny-all.policy` there.
  - `KEEP=1` behaves as in `run.sh`.
- **Output line format:** `<VERDICT>  [probe:<name>]  <id> <check name>  <shown result>`, with the
  result cut at 110 characters as in `checks.py`. The summary gives a count per verdict.
- **`run_checks`:**
  - **skipped probe:** `SKIP`;
  - **`na_reason()` not `None`:** `N/A` with that reason;
  - **`sight()` false:** `BLIND`;
  - **an exception:** `ERROR`, with its class and message;
  - **`leaked(actual)`:** `LEAK`;
  - **otherwise:** `ENFORCED`.
- **`canary.py <dir> <port> <log>`.** It:
  - serves `GET /files/<name>` from `<dir>`;
  - answers 200 and logs `<path> <peer>` for every other path;
  - offers a helper `canary_hits(log, marker) -> list[str]` in `probe_lib.py`.
- **`fakeranger.py`** reads both JSON files on every request. Its arguments and output are
  unchanged.
- **`policies.py`.**
  - **`--probe` adds:**
    - `udfdev`: bob's grant, mask and filter on `demo.customers`, plus `create function` on
      `default_catalog.demo`, plus `usage` on `function` `demo.*`;
    - `loader`: `insert` on `demo.s_target`, `demo.r_target` and `demo.pk_target`, and the row
      filter `id = 1` on `demo.pk_target`;
    - the catalog policies of Task 3 (alice, bob, mallory on `hive`, `jdbc` and `paimon`
      `sales.orders`).
  - **`--version 2 --revoke loader`** writes the same document with `policyVersion` 2 and without
    loader's insert items.
  - **No flags:** the output is identical to today's (compare the JSON).
- **Pins.** Run `docker manifest inspect` on each service image (Postgres, a Hive Metastore,
  Kafka in KRaft mode, a JDK 17 image, `python:3.12-slim`). Pin the tags that resolve in
  `probe.sh`, and record their digests in the task report.

**Checks:**
- **Unit** (`python -I -m unittest tests/e2e/test_probe_lib.py`, new):
  - `skipped_probes`: empty, two names, an unknown name (exit 2), and whitespace;
  - `run_checks`: each verdict path, including a `sight()` false that must give `BLIND` even when
    `actual` is a refusal.
- **Policies:** `policies.py` without flags writes byte-identical JSON to `main`'s; the task
  report shows the `diff`.

**Run (controller, Docker):**

```bash
cd egret-starrocks
tests/e2e/run.sh ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4 > "$SCRATCH/run-before.txt" 2>&1   # on main
tests/e2e/run.sh ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4 > "$SCRATCH/run-after.txt" 2>&1    # on the branch
diff <(grep -E '^(PASS|FAIL)' "$SCRATCH/run-before.txt") <(grep -E '^(PASS|FAIL)' "$SCRATCH/run-after.txt")
```

The diff is empty. `$SCRATCH` is the session scratchpad.

**Mutant:** in `run_checks`, treat a failed sight as `ENFORCED`. The `BLIND` unit test fails.

### Task 2: Probe 1, Java and Python UDFs

**Files (fork):**
- `tests/e2e/probe_udf.py` (new)
- `tests/e2e/udf/Echo.java`, `ReadFile.java` and `Fetch.java` (new; package `egret.probe`)
- `tests/e2e/udf/deny-all.policy` (new; a `grant {}` with no permission)
- `tests/e2e/udf/build.sh` (new; builds `probe-udf.jar` in the pinned JDK 17 container into the
  canary directory)
- the `seed_udf` and `udf_phase` functions in `probe.sh`

**Requirements:**
- **U0** runs in the main phase, before `enable_udf` is on. The UDF phase then:
  - appends `enable_udf = true` to `fe.conf`;
  - `docker restart egs-probe-sr`;
  - reruns the BE and policy-version-1 waits from `stack.sh`.
- **Sentinel.** The harness writes the sentinel, `/opt/egret-probe/sentinel.txt` containing
  `egret-probe-sentinel-<RUN_ID>`, with `docker exec`.
- **The UDF classes:**
  - `ReadFile.evaluate(String) -> String`: `Files.readAllBytes`, the first 200 bytes as UTF-8, or
    `"ERR:" + e.getClass().getName()`;
  - `Fetch.evaluate(String) -> String`: an `HttpURLConnection` GET, its status code, or
    `"ERR:" + class`;
  - `Echo.evaluate(String) -> String`: its argument.
- **Functions are created** with `"type" = "StarrocksJar"`, `"symbol" = "egret.probe.<Class>"`,
  and `"file" = "http://host.docker.internal:18778/files/probe-udf.jar"`.
- **Checks U0 to U10** follow the spec's table. The leak predicates:
  - **U5:** the result contains the sentinel;
  - **U6:** the canary logged `/udf-egress-<RUN_ID>`;
  - **U7:** the result is `200`;
  - **U8:** the result is not DENIED, and not an `ERROR 52xx`. This is the `not_refused` rule of
    `checks.py`: the privilege check let it through;
  - **U9:** the canary logged the FE's fetch. This is recorded, and it is a `LEAK` only for eve.
    For udfdev it is classified in Task 8.
- **Python.**
  - **P0:** `docker exec egs-probe-sr sh -c 'command -v python3 && python3 -c "import pyarrow"'`,
    plus `be.conf`'s `python_envs`. Its output is the `na_reason` of P2 to P4 when either part
    fails.
  - **P1:** an inline Python function, with `"type" = "Python"`, `"symbol" = "echo"`,
    `"file" = "inline"` and `"input" = "scalar"`, and the body after `AS $$ ... $$`. Check the
    exact grammar against `CreateFunctionAnalyzer.analyzePython` and the 4.1.7 parser before
    writing it.
- **Variant `UDF_JAVA_POLICY=1`** reruns U5 to U7. The task report records whether each flips to
  `ENFORCED`, or whether the JVM refuses `System.setSecurityManager`. That would be JDK 17's
  deprecation path, and it is recorded verbatim.

**Run (controller, Docker), each image, then the variant on the egret image:**

```bash
tests/e2e/udf/build.sh
PROBE_SKIP=hive,jdbc,paimon,stream-load,routine-load,in-namespace tests/e2e/probe.sh starrocks/allin1-ubuntu:4.1.7 > "$SCRATCH/udf-official.txt" 2>&1
PROBE_SKIP=hive,jdbc,paimon,stream-load,routine-load,in-namespace tests/e2e/probe.sh ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4 > "$SCRATCH/udf-egret.txt" 2>&1
UDF_JAVA_POLICY=1 PROBE_SKIP=hive,jdbc,paimon,stream-load,routine-load,in-namespace tests/e2e/probe.sh ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4 > "$SCRATCH/udf-egret-policy.txt" 2>&1
```

**Mutant:** point U5 at a path that does not exist. The check must show `ERR:java.nio.file.NoSuchFileException`
and be judged `BLIND` or `ERROR`, never `ENFORCED`. To get that, the U5 sight also checks the
sentinel with `docker exec`.

### Task 3: Probe 2, JDBC and Hive catalogs

**Files (fork):**
- `tests/e2e/probe_catalogs.py` (new)
- the `start_postgres`, `start_hive`, `seed_jdbc` and `seed_hive` functions in `probe.sh`
- `tests/e2e/images/hive-metastore/Dockerfile` (only if needed, see below)

**Requirements:**
- **Postgres.** It is pinned with the password `pgsecret`, a database `sales` and the table
  `orders`, seeded by `psql` with `ORDERS_ALL`.
- **The JDBC driver.** It comes from Maven Central, pinned by version and sha256 in `probe.sh`,
  and is served by the canary.
- **The JDBC catalog.** `jdbc` is created with `jdbc_uri`, `user`, `password`, `driver_url` and
  `driver_class`. These are the keys the egret renderer uses.
- **The Hive catalog.** The Hive Metastore is pinned, and `hive` is created with
  `hive.metastore.uris` and the RustFS `aws.s3.*` keys. The database and table are created
  through the catalog as root, partitioned by `region`, and the rows are inserted.
  - If the metastore cannot create an `s3://` location, add the S3A jars in
    `tests/e2e/images/hive-metastore/Dockerfile`, built by `probe.sh`.
  - Record which way it went.
- **When catalogs are created.** Both are created after the AuditLoader is loading (the
  `stack.sh` wait), so that C15 has something to check.
- **The checks.** C1 to C16 follow the spec, as one `ProbeCheck` list per catalog:
  - `ORDERS_ALL` and `ORDERS_BOB` come from `sqlpaths`, or are copied as literals;
  - **C10's predicate:** the output contains the full secret (`pgsecret`, or `e2esecret`);
  - **C11's predicate:** any row names `sales` or `orders`;
  - **C14 for Hive** first tries `SHOW PARTITIONS FROM hive.sales.orders`, then
    `information_schema.partitions_meta`, as alice. The first that alice can read is the probe
    for bob. When neither works for alice, C14 is `N/A`, with both errors;
  - **C15:** `starrocks_audit_db__.starrocks_audit_tbl__` `stmt LIKE '%<secret>%'`, read as root,
    plus `grep -c <secret>` on `fe.audit.log` through `docker exec`;
  - **C16:** eve's `CREATE EXTERNAL CATALOG` with `driver_url` =
    `http://host.docker.internal:18778/c16-<RUN_ID>.jar`. It leaks if it is not DENIED, or if the
    canary got a hit.

**Run (controller, Docker), on each image:**

```bash
PROBE_SKIP=udf,python-udf,paimon,stream-load,routine-load,in-namespace tests/e2e/probe.sh <image> > "$SCRATCH/cat-<official|egret>.txt" 2>&1
```

**Mutant:** in `policies.py --probe`, drop bob's `jdbc` mask. C2 must become `LEAK` for `jdbc`.
This shows the comparison sees a clear email. Revert it afterwards.

### Task 4: Probe 2, the Paimon catalog

**Files (fork):**
- `tests/e2e/probe_catalogs.py` (the Paimon list)
- the `seed_paimon` function in `probe.sh`
- `tests/e2e/requirements.txt` (only if `pypaimon` is used, pinned)

**Requirements:**
- **The catalog.** `paimon` is created with `paimon.catalog.type = filesystem`,
  `paimon.catalog.warehouse = s3://warehouse/paimon/`, and the RustFS keys.
- **Seeding.** Write `sales.orders`, with `ORDERS_ALL` in one commit, through `pypaimon` from the
  harness venv.
  - If `pypaimon` cannot write an S3 filesystem warehouse, stop. Report the error verbatim, and
    apply the user's answer to spec open question 4.
  - Do not add Spark or Flink without that answer.
- **The checks.** C1 to C16 for `paimon`. C14 reads `orders$files`, `orders$snapshots` and
  `orders$partitions` (H7):
  - **sight:** alice reads a non-empty `orders$files`;
  - **leak:** bob reads any row of any of the three, or eve does.

**Run (controller, Docker):** as Task 3, with `PROBE_SKIP=udf,python-udf,hive,jdbc,stream-load,routine-load,in-namespace`.

**Mutant:** grant bob nothing on `paimon`. C14 must then show `ENFORCED` (DENIED), which proves
the check can produce both verdicts.

### Task 5: Probe 3, stream load and routine load

**Files (fork):**
- `tests/e2e/probe_loads.py` (new)
- the `start_kafka`, `seed_loads` and `revoke_loader` functions in `probe.sh`

**Requirements:**
- **Tables and users.**
  - `demo.s_target (id BIGINT, v VARCHAR(64))` is a duplicate-key table.
  - `demo.r_target` has the same columns.
  - `demo.pk_target` is a primary-key table, seeded with ids 1 to 3.
  - `nat` is created, and root runs `GRANT INSERT ON demo.s_target TO nat`. The outcome, whether
    accepted or refused under `access_control = ranger`, is printed and drives S6's `na_reason`.
- **Stream load client.** `PUT /api/demo/<table>/_stream_load`, with Basic auth, the header
  `label: <check>-<RUN_ID>`, and CSV columns `id,v`.
  - **Via the FE:** send it to port 19231 and follow one 307 by hand. If the `Location` host is
    `127.0.0.1` or does not resolve from the host, rewrite it to `127.0.0.1:19240` and print
    `rewritten`.
  - **Directly:** send it to 19240.
  - **Denied** means: the JSON `Message` contains `Access denied`, or HTTP 401 or 403.
- **S6 to S11** follow the spec.
  - **S8:** loader loads `x,egret-errlog-<RUN_ID>`. A non-numeric id makes the row rejected, with
    `max_filter_ratio` 1. The `ErrorURL` from the answer is then fetched by eve, with her Basic
    auth, and by an unauthenticated request:
    - **leak:** the marker is in the body;
    - **sight:** root's fetch shows the marker.
  - **S10:** compare the outcome of `INSERT INTO demo.pk_target VALUES (2, 'over')` by SQL with
    the stream load of the same row, as loader. Read `id = 2` back as root after each.
- **Kafka.** One broker with a PLAINTEXT and a SASL_PLAINTEXT (PLAIN) listener, and the user
  `probe` with password `kafkasecret`. The topic is `probe-r`, and messages are produced with
  `docker exec egs-probe-kafka <kafka-console-producer.sh path in the pinned image> ...`.
- **R1 to R8** follow the spec.
  - **R7:** `revoke_loader` runs `policies.py --version 2 --revoke loader` over the served file,
    then waits for `lastKnownVersion=2` in `ranger.log` (timeout 60 s, else `ERROR`). It then
    produces 3 messages and waits 60 s:
    - **leak:** the row count grew;
    - also recorded: the job state from root's `SHOW ROUTINE LOAD`.
- **Routine load runs last** in this file. The revoke changes loader's grants for anything after
  it.

**Run (controller, Docker), on each image:**

```bash
PROBE_SKIP=udf,python-udf,hive,jdbc,paimon,in-namespace tests/e2e/probe.sh <image> > "$SCRATCH/loads-<official|egret>.txt" 2>&1
```

**Mutant:** skip the revoke step but keep R7's predicate. R7 must show `LEAK` (the rows arrive).
That proves that a real revoke result of `ENFORCED` is meaningful.

### Task 6: Probe 4, the CN/BE ports from inside the network (allin1)

**Files (fork):**
- `tests/e2e/inns_checks.py` (new; standard library only)
- the `inns_phase` function in `probe.sh`

**Requirements:**
- **Run.**
  `docker run --rm --network egs-probe-net -v "$HERE:/e2e:ro" python:<pinned> python -I /e2e/inns_checks.py egs-probe-sr <run_id> <segment path> <segment sha256> <S4 label>`.
  - `probe.sh` finds the segment path and sha256 beforehand, as root:
    `SHOW TABLET FROM demo.customers`, then `find` under the BE `storage_root_path` through
    `docker exec`.
  - The script prints the same line format as `probe_lib`, and `probe.sh` folds its counts into
    the summary.
- **Thrift.** `inns_checks.py` carries a minimal TBinaryProtocol writer and reader (strict
  message header, buffered transport), enough for these calls:
  - `getDbNames(TGetDbsParams)`: field 4 `current_user_ident` (`TUserIdentity` 1 `username`,
    2 `host`, 3 `is_domain`), and field 5 `catalog_name`;
  - `forward(TMasterOpRequest)`: its required fields, taken from `FrontendService.thrift`.

  Before writing them, check every field id against `work/starrocks/gensrc/thrift/` and cite the
  lines in a comment.
- **N1 to N11** follow the spec.
  - **N4** is followed at once by its restore. N5 runs before and after N4.
  - **N11** runs last, then the script exits. `probe.sh` then skips any later phase and says so.
- **`BE_HTTP_AUTH=1` variant.** It reruns N1 to N5 and N11 with no credentials and with eve's
  Basic auth, and reruns S8. Pass `--auth eve:evepw` to the script for the second pass.

**Run (controller, Docker), each image, then the variant on the egret image:**

```bash
PROBE_SKIP=udf,python-udf,hive,jdbc,paimon,routine-load tests/e2e/probe.sh <image> > "$SCRATCH/inns-<official|egret>.txt" 2>&1
BE_HTTP_AUTH=1 PROBE_SKIP=udf,python-udf,hive,jdbc,paimon,routine-load tests/e2e/probe.sh ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4 > "$SCRATCH/inns-egret-auth.txt" 2>&1
```

`stream-load` stays on: N3 and the S8 rerun need its label and its error log.

**Mutant:** send N8's `eve` call with `root`. The control must then show `demo`, which proves the
predicate tells the two apart.

### Task 7: Probe 4 on kind (controller, optional per spec open question 5)

Run this only on a kind cluster that egret provisions into, never one you did not create. Follow
memory `egret-deploy-kind` for the recipe.

1. Create a StarRocks engine through the API, used only for this probe, on the default images
   (FE `4.1.7-egret.4`, the official CN).
2. Start a pod in the engine namespace:
   `kubectl run -n egret-engines egs-probe-inns --image=python:<pinned> --restart=Never --command -- sleep 3600`.
   Then `kubectl cp tests/e2e/inns_checks.py egret-engines/egs-probe-inns:/tmp/`.
3. Run `inns_checks.py --no-destructive` against the engine's CN and FE Services. Get their names
   from `kubectl get svc -n egret-engines`. The flag skips N11, and skips N4 and N5, unless the
   engine is deleted right after.
4. **NetworkPolicy control.** Run the same pod in `default`. N1 must time out there, which shows
   the policy is the boundary and the in-namespace result is not a reachability artefact.
5. Record which shared-data ports answer, and `GET :8060/flags` on the CN.
6. **With `cn_config` `enable_http_auth = true`**, set through the engine's settings and re-applied,
   rerun step 3, then check that:
   - the CN stays `Alive` in `SHOW COMPUTE NODES`;
   - an egret query on the engine runs;
   - the operator reports no probe failure (`kubectl get pods` shows no restarts).

Delete both pods and the engine afterwards. The outputs go to `$SCRATCH/kind-*.txt`.

### Task 8: Full runs, the findings report, the fork README section and the known issues

**Files:**
- Platform: `docs/en/reports/2026-11-09-starrocks-unmeasured-paths-probe.md` (new).
- Fork: the "Probe" section of `README.md`.
- Docs (`egret-data/docs`): the Known issues page, for out-of-model findings only.

**Steps:**
1. **Controller, Docker: full runs**, with no `PROBE_SKIP` except the user-approved ones, on both
   images, plus the two variants on the egret image:

   ```bash
   tests/e2e/probe.sh starrocks/allin1-ubuntu:4.1.7 > "$SCRATCH/probe-official.txt" 2>&1
   tests/e2e/probe.sh ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4 > "$SCRATCH/probe-egret.txt" 2>&1
   BE_HTTP_AUTH=1 UDF_JAVA_POLICY=1 tests/e2e/probe.sh ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4 > "$SCRATCH/probe-egret-mitigated.txt" 2>&1
   ```

   Each run's summary must have zero `BLIND` and zero `ERROR`. Otherwise go back to the
   probe's task.
2. **The report** follows the shape of `docs/en/reports/2026-11-06-gov-sp2b-opa-latency.md`:
   - **raw files:** the scratchpad path of the raw files, and each table citing its file and
     command;
   - **pins:** the image digests and service tags;
   - **hypotheses:** H1 to H8, each confirmed or refuted, citing the checks;
   - **checks:** one row per check, with official verdict, egret verdict, mitigated verdict,
     class (enforced, leak, out of model) and follow-up id;
   - **boundaries:** every `SKIP`, `N/A` and chosen boundary, with its reason;
   - **follow-ups:** a table with id, class, candidate (fork patch `0010` on, egret change, or
     known issue), the checks that become its gate, and severity in one line.

   Respect spec open question 1 for leak rows.
3. **The fork README** "Probe" section: the date, the images, the verdict counts per run, and a
   link to the report if it is public.
4. **The known issues** (docs repo): one entry per out-of-model finding, under the next free KI
   numbers. Each entry gives what happens, who is affected, and the operator's mitigation. Every
   code fence has a language, and `npm run build` in the docs repo passes.
5. **No push** until the user has read the report and answered open question 1.

**Done when** every check of the spec has a row with two verdicts, every leak has a follow-up id,
and the user has the report.
