# Data Apps P3 — the `egret-apps` image and `egret_marimo` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the container that runs a data app, and the Python package inside it. P3 must deliver:
- marimo 0.25.0, locked down;
- an `egret_marimo` client that sends SQL to egret's gateway under the viewer's identity;
- `EgretStorage`, which keeps drafts in egret rather than on the pod's disk;
- one kernel process per viewer session in run mode;
- the shim that answers marimo's per-launch install check.

Everything is tested against a fake egret. The real pod endpoints and the launcher arrive in P4.

**Architecture:**
- **Layout and image.** A new top-level directory, `apps-image/`, mirrors `kernel-image/`: `Dockerfile`, `pyproject.toml`, the package, `tests/`, `smoke.sh`. The image is `egret-apps:dev`, built `FROM python:3.12-slim` with **exact pins** in `requirements.txt` and run as uid 1000.
- **The `egret_marimo` package:**
  - `client.py` (DB-API connection, `sql()`);
  - `storage.py` (`EgretStorage` plus the patched `NotebookWorkspace.load`, with one base revision per app);
  - `patches.py` (forced IPC in run mode);
  - `launcher.py` (console script `egret-apps edit|run`).
- **The shim.** `venv-shim/bin/python` answers `has_marimo_installed` (D14) and passes every other invocation through.
- **Origin.** All of this is productionised from the S0/S0b spike: branch `spike/marimo-s0`, never merged. Every behaviour taken from it is re-tested here.

**Tech Stack:** Python 3.12, marimo 0.25.0, httpx, polars, duckdb + sqlglot (D13), pytest, Playwright (Chromium, for the end-to-end contract tests), Docker.

**Spec:** `docs/en/specs/2026-09-30-marimo-data-apps-design.md`:
- §4.4 (the Python side);
- §5.1–§5.3 (`EgretStorage`, run mode, **the pod API contract**);
- §6.3 (pods, locked configuration);
- D7 (a process per session), D13 (DuckDB shipped; the template makes egret the default), D14 (the shim answers the check).

## Global Constraints

- **Branch:** `feat/data-apps-p3-image`, off `origin/main`, in its own worktree.
- **Git:** use the configured identity. Write Conventional Commits that end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Pin `marimo==0.25.0`** and every other runtime dependency exactly, in `apps-image/requirements.txt`. The patches target private marimo API, so an upgrade must fail a contract test before it can ship.
- **Pod API contract (spec §5.3).** These are the exact paths the client calls, all with `Authorization: Bearer <pod token>`:

  | Method | Path | Result |
  |---|---|---|
  | `GET` | `/api/apps/pod/apps/{app_id}/draft` | `{source, revision}` |
  | `PUT` | `/api/apps/pod/apps/{app_id}/draft` with `If-Match: "<rev>"` and body `{"source": …}` | `{revision}` · 409 `{error, current_revision}` · 413 · 428 |
  | `GET` | `/api/apps/pod/apps/{app_id}/versions/{version_id}` | `{source, version_no}` |

  The gateway is `POST /api/apps/query` with `Authorization: Bearer <session token>`. The session token is read from the `x-egret-app-token` header of the request that triggered the kernel command.
- **Environment variables the pod reads.** All are set by egret's launcher in P4. A missing required variable stops the launcher with exit 64 and a clear message.

  | Variable | Required for | Meaning |
  |---|---|---|
  | `EGRET_URL` | both modes | egret's base URL, without a trailing slash |
  | `EGRET_POD_TOKEN` | both modes | the pod token |
  | `EGRET_MARIMO_TOKEN` | both modes | the marimo `--token-password` |
  | `EGRET_BASE_URL` | both modes | `/e/<pod>` or `/r/<version>` |
  | `EGRET_APP_ID` | run | the app |
  | `EGRET_VERSION_ID` | run | the pinned version |
  | `APPS_WORKDIR` | edit | the working directory |

- **Locked configuration:**
  - `XDG_CONFIG_HOME` points at the image's config, which contains `[ai] enabled = false`.
  - The launcher **refuses to start** (exit 65) if a `.marimo.toml` exists in `APPS_WORKDIR` or any parent of it, or in `$HOME`.
  - Route blocking happens at egret's proxy (P4), not here.
- **Process per run session (D7):**
  - Run mode forces marimo's IPC kernel manager: `SessionImpl.create(sandbox=True)` for `SessionMode.RUN`, with `ipc._get_venv_config` pointed at `/opt/egret-apps/venv-shim`.
  - The shim answers **exactly** `python -c "import marimo, msgspec, zmq; print(marimo.__version__)"` with the pinned version, and execs the real interpreter for everything else.
  - Edit mode uses marimo's own process-per-notebook and is not patched.
- **Python tests:**
  - Unit and contract tests run under `pytest` from `apps-image/` in a local venv (`python3.12 -m venv .venv && .venv/bin/pip install -r requirements.txt -e . -r requirements-dev.txt`).
  - End-to-end tests are marked `@pytest.mark.e2e`. They need Chromium (`playwright install chromium`) and run in `smoke.sh` and in the workflow, not by default.
- **No change to egret's Rust code or Helm in P3.** The launcher, the pod token and the pod endpoints are P4. The only repo-wide changes are the `justfile` recipes and a new CI workflow.
- **Disk.** Python and Docker only; no cargo build is needed. Check `df -h` before `docker build`, because the image is about 1 GB.
- **Illustrative code is a guess.** It comes from the S0 spike against marimo 0.25.0. Read the installed `site-packages/marimo` before patching, follow the real symbols, and record each deviation.

---

## File structure (all new, under `apps-image/`)

| File | Responsibility |
|---|---|
| `Dockerfile` | `python:3.12-slim`, pinned deps, package install, shim, config, uid 1000, `ENTRYPOINT ["egret-apps"]` |
| `requirements.txt` | Exact runtime pins: marimo, httpx, polars, duckdb, sqlglot, pyarrow, plotly, altair, anywidget |
| `requirements-dev.txt` | pytest, uvicorn, starlette, playwright |
| `pyproject.toml` | Package `egret-marimo`; console script `egret-apps = egret_marimo.launcher:main`; `[tool.pytest.ini_options]` with the `e2e` marker |
| `config/marimo/marimo.toml` | `[ai] enabled = false` |
| `venv-shim/bin/python`, `venv-shim/pyvenv.cfg` | The D14 shim |
| `egret_marimo/__init__.py` | Public API: `connect`, `sql`, `Connection`, `GatewayError`, `SessionExpired` |
| `egret_marimo/client.py` | DB-API connection to the gateway; `sql()` → polars |
| `egret_marimo/podapi.py` | Typed HTTP client for the pod API (§5.3) |
| `egret_marimo/storage.py` | `EgretStorage`, `install_storage_patch()`, the per-app base-revision registry |
| `egret_marimo/patches.py` | `install_run_mode_ipc_patch()` |
| `egret_marimo/launcher.py` | `main()`: env validation, config guard, patch install, marimo argv, launch |
| `tests/fake_egret.py` | Starlette fake of the gateway and the pod API, with an inspectable state |
| `tests/test_client.py`, `tests/test_podapi.py`, `tests/test_storage.py`, `tests/test_launcher.py`, `tests/test_marimo_contract.py` | Unit and contract tests |
| `tests/e2e/test_edit_and_run.py` | Playwright end-to-end: edit save and 409, run per-session processes and tokens |
| `smoke.sh` | Build the image, run it against the fake egret, check that it serves |
| `../justfile` (modify) | Add `egret-apps:dev` to `images-build` and the `kind load` lines; recipe `apps-image-test` |
| `../.github/workflows/apps-image.yml` | `apps-pytest` and `apps-image-smoke` jobs, path-filtered, as in `kernel-image.yml` |

---

### Task 1: Package skeleton, the gateway client, and the pod API client

**Files:**
- Create: `apps-image/pyproject.toml`, `apps-image/requirements.txt`, `apps-image/requirements-dev.txt`, `apps-image/.gitignore`, `apps-image/egret_marimo/{__init__,client,podapi}.py`, `apps-image/tests/fake_egret.py`, `apps-image/tests/test_client.py`, `apps-image/tests/test_podapi.py`

**Interfaces:**
- Produces:

```python
# client.py
class GatewayError(RuntimeError): status: int; detail: str
class SessionExpired(GatewayError): ...          # 401
def connect(database: str, dialect: str = "sql") -> Connection
class Connection:   # DB-API 2.0 surface marimo's DBAPIEngine requires
    dialect: str
    def cursor(self) -> Cursor; def commit(self) -> None; def rollback(self) -> None; def close(self) -> None
class Cursor:
    description: list[tuple] | None; rowcount: int
    def execute(self, sql: str, params=()) -> "Cursor"; def fetchall(self) -> list[tuple]
    def fetchone(self) -> tuple | None; def close(self) -> None
def sql(query: str, database: str, params=()) -> "polars.DataFrame"
def _session_token() -> str   # x-egret-app-token of the triggering request; EGRET_TEST_TOKEN only under pytest

# podapi.py
@dataclass class Draft: source: str; revision: int
class DraftConflict(Exception): current_revision: int
class PodApi:
    def __init__(self, base_url: str, pod_token: str, client: httpx.Client | None = None)
    def get_draft(self, app_id: str) -> Draft
    def put_draft(self, app_id: str, expected_revision: int, source: str) -> int   # new revision; raises DraftConflict
    def get_version(self, app_id: str, version_id: str) -> str
```

- [ ] **Step 1: Write `requirements.txt` with exact pins**

Pin `marimo==0.25.0`. For httpx, polars, duckdb, sqlglot, pyarrow, plotly, altair and anywidget, take the versions the spike's `uv.lock` resolved (`git -C ../egret-wt-marimo-s0 show spike/marimo-s0:spikes/marimo-s0/uv.lock`, or the spike README). For packages the spike did not use, take the current release. Write each as `name==x.y.z` and record its source in a comment at the top.

`requirements-dev.txt`: pytest, uvicorn, starlette, playwright, pinned the same way.

- [ ] **Step 2: Write `tests/fake_egret.py`**

It is a Starlette app holding an in-memory state:

```python
STATE.drafts[app_id]    = {"source", "revision"}
STATE.versions[(app_id, version_id)] = {"source", "version_no"}
STATE.pod_tokens        = {token: {"mode", "app_id"?, "version_id"?}}
STATE.session_tokens    = {token: {"user"}}
STATE.queries           = [{"token", "user", "database", "sql", "params", "pid"}]
STATE.events            = []
```

It implements the exact §5.3 contract (401 for a wrong pod token, 428 without `If-Match`, 409 `{error, current_revision}` when stale, 413 over 1 MiB). It also implements `POST /api/apps/query`: 401 for an unknown session token, otherwise `{columns, rows, truncated, row_count}` with the rows `[[user, sql]]`. Add a `serve(port)` helper that runs it in a uvicorn thread for tests.

- [ ] **Step 3: Write the failing tests**

`tests/test_podapi.py`:
- get then put round-trip;
- put with a stale revision raises `DraftConflict(current_revision=…)`;
- `If-Match` is sent quoted;
- a wrong pod token raises `GatewayError(status=401)`;
- `get_version` returns the source.

`tests/test_client.py`:
- `connect().cursor().execute()` sets `description` and the rows;
- `sql()` returns polars with the gateway's column names;
- a 401 raises `SessionExpired`;
- a 400 raises `GatewayError` with the gateway's message;
- `DBAPIEngine.is_compatible(connect("db"))` is true;
- `_session_token()` reads `x-egret-app-token` from a stubbed `mo.app_meta().request` (monkeypatch), and raises without one;
- `EGRET_TEST_TOKEN` is honoured **only** when `PYTEST_CURRENT_TEST` is set. A production pod must never accept a token from its own environment.

- [ ] **Step 4: Run them to see them fail**

```bash
cd apps-image && python3.12 -m venv .venv && .venv/bin/pip install -r requirements.txt -r requirements-dev.txt -e .
.venv/bin/pytest -q
```

Expected: import errors.

- [ ] **Step 5: Implement `client.py`, `podapi.py` and `__init__.py`**

Start from the spike's `egret_marimo/__init__.py`, with these changes:
- the gateway URL comes from `EGRET_URL`, and requests are `httpx.post(f"{base}/api/apps/query", json={...}, headers={"authorization": f"Bearer {token}"}, timeout=70)` (above the gateway's 60 s);
- send `pid` only in tests (`EGRET_TEST_INCLUDE_PID`). The production body is `{database, sql, params}`;
- map statuses to errors: 401 → `SessionExpired("session expired — reload the app")`, and any other non-2xx → `GatewayError(status, body.error)`;
- `Connection.dialect` is passed in by the caller. Its default is `"sql"`, because the pod does not know the engine.

`podapi.py` puts the same 401/409/413/428 mapping in one place.

- [ ] **Step 6: Run the tests until they pass, then commit**

```bash
.venv/bin/pytest -q
git add apps-image && git commit -m "feat(apps-image): egret_marimo gateway and pod-API clients"
```

---

### Task 2: `EgretStorage`, with one base revision per app

**Files:**
- Create: `apps-image/egret_marimo/storage.py`, `apps-image/tests/test_storage.py`, `apps-image/tests/test_marimo_contract.py`

**Interfaces:**
- Consumes: `PodApi`, `Draft`, `DraftConflict` from Task 1.
- Produces:

```python
class BaseRevisions:   # process-wide registry, one per pod
    def get(self, app_id: str) -> int | None
    def set(self, app_id: str, revision: int) -> None
    def mark_stale(self, app_id: str) -> None
    def is_stale(self, app_id: str) -> bool
def install_storage_patch(workdir: Path, api: PodApi, revisions: BaseRevisions) -> None
class EgretStorage(FilesystemStorage):   # bound to (app_id, api, revisions)
    def write(self, path: Path, content: str) -> None   # PUT with If-Match = revisions.get(app_id); 409 → mark_stale + HTTPException(409, …)
```

The patched `NotebookWorkspace.load(key, defaults)`, for a file directly under `workdir` named `<app_id>.py`, does this:
1. If `revisions.get(app_id)` is None **or** `revisions.is_stale(app_id)`: fetch the draft, write it to disk, and call `revisions.set(app_id, draft.revision)`, which also clears the stale mark.
2. Otherwise leave both the disk file and the base revision untouched. Do not fetch.
3. Build the manager with the original `load`, then set `manager.storage = EgretStorage(app_id, api, revisions)`.

On a successful write, `EgretStorage.write` calls `revisions.set(app_id, new)` and writes the local file. On a conflict it calls `mark_stale(app_id)` and raises `marimo._utils.http.HTTPException(409, "This draft was saved by someone else — reload to get the latest version.")`.

- [ ] **Step 1: Write the failing tests**

`tests/test_storage.py` drives marimo's real classes (`marimo._server.workspace` / `marimo._session.notebook.file_manager`) against `fake_egret` running in a thread:

| # | Case |
|---|---|
| 1 | First load fetches the draft, writes it to disk and records base revision 1. `manager.storage` is an `EgretStorage`. |
| 2 | **The S0 hazard.** Load app A, giving manager M1. Another author saves through the fake's PUT, so the server is at rev 2. Load A again (M2), as marimo does on a second request. Then a save through **M1** must raise HTTPException 409, and the server source must still be the other author's. Before the fix, rebinding on load let this save succeed and overwrite. |
| 3 | After that 409, the next load re-fetches (disk = the server's rev-2 source, base = 2), and a save then succeeds as rev 3. |
| 4 | A normal save advances the base, so two consecutive saves from one manager both succeed (rev 2, rev 3). |
| 5 | A file outside `workdir`, or not named `<uuid>.py`, is loaded by the original code, untouched. |
| 6 | 413 from the server is raised as HTTPException 413 with the server's message. |

`tests/test_marimo_contract.py` checks the private API the patches rely on, so that an upgrade fails here first:
- `marimo.__version__ == "0.25.0"`;
- `NotebookWorkspace.load` exists with the parameters `(self, key, defaults=None)`;
- `AppFileManager.storage` is assignable, and `save` calls `self.storage.write(path, contents)` (assert it with a recording storage on a temp file);
- `FilesystemStorage` and `HTTPException(status_code, detail)` keep their shape.

- [ ] **Step 2: Run them to see them fail**

Run: `.venv/bin/pytest tests/test_storage.py tests/test_marimo_contract.py -q`. Expected: the storage tests fail, and the contract tests pass.

- [ ] **Step 3: Implement `storage.py`**

Start from the spike's `install_storage_patch`/`EgretStorage` (`entrypoint.py:29-66`). Replace the per-load rebinding with the `BaseRevisions` rules above, and guard the registry with a `threading.Lock`.

- [ ] **Step 4: Run the tests until they pass**

Run: `.venv/bin/pytest -q`.

- [ ] **Step 5: Mutation checks, then commit**

Do these one at a time, confirm the named case fails, and restore with `git checkout -- <file>`:
1. Always re-fetch on load → case 2 fails.
2. Do not `mark_stale` on 409 → case 3 fails.
3. Do not advance the base on a successful write → case 4 fails.

Then commit:

```bash
git add apps-image && git commit -m "feat(apps-image): EgretStorage — drafts in egret, one base revision per app"
```

---

### Task 3: Run-mode IPC patch, the D14 shim, and the launcher

**Files:**
- Create: `apps-image/egret_marimo/patches.py`, `apps-image/egret_marimo/launcher.py`, `apps-image/venv-shim/bin/python`, `apps-image/venv-shim/pyvenv.cfg`, `apps-image/config/marimo/marimo.toml`, `apps-image/tests/test_launcher.py`, `apps-image/tests/e2e/test_edit_and_run.py`
- Modify: `apps-image/tests/test_marimo_contract.py`

**Interfaces:**
- Produces:
  - `install_run_mode_ipc_patch(venv_shim: Path) -> None`;
  - `launcher.main(argv: list[str] | None = None) -> int`.
- Console usage: `egret-apps edit` or `egret-apps run`. All other input comes from the environment table in Global Constraints.

- [ ] **Step 1: Write `venv-shim/bin/python`**

It must be executable. Set `EGRET_REAL_PYTHON` and `EGRET_MARIMO_VERSION` in the image environment.

```sh
#!/bin/sh
# D14: answer marimo's per-launch install probe without starting an interpreter
# (it costs 1.0–1.8 s per kernel launch — S0b). Everything else runs the real Python.
if [ "$1" = "-c" ] && [ "$2" = "import marimo, msgspec, zmq; print(marimo.__version__)" ]; then
  echo "$EGRET_MARIMO_VERSION"; exit 0
fi
exec "$EGRET_REAL_PYTHON" "$@"
```

`pyvenv.cfg` contains `home = /usr/local/bin`, `include-system-site-packages = true` and `version = 3.12`.

- [ ] **Step 2: Write the failing launcher tests**

`tests/test_launcher.py`:
- A missing required variable exits 64 and names the variable.
- A `.marimo.toml` in `APPS_WORKDIR`, in a parent of it, or in `$HOME` exits 65 and names the path.
- Edit mode builds this argv, with the storage patch installed and the IPC patch **not** installed:

  ```
  ["edit", <workdir>, "--host", "0.0.0.0", "--port", "2718", "--headless", "--base-url", <EGRET_BASE_URL>, "--token-password", <EGRET_MARIMO_TOKEN>, "--skip-update-check"]
  ```

- Run mode fetches the version source once into `<workdir>/<app_id>.py` and builds `["run", <file>, … "--port", "2719", …]`, with the IPC patch installed and the storage patch not installed.
- The shim answers the exact probe with the pinned version and passes `-c "print(1)"` through. Run the script with `subprocess`.
- `XDG_CONFIG_HOME` is set to the image config, and `MARIMO_SKIP_UPDATE_CHECK=1`.

`test_marimo_contract.py`: add assertions that `SessionImpl.create` is a classmethod that takes `mode` and `sandbox` **as keywords**, and that `marimo._session.managers.ipc._get_venv_config` exists. The S0b spike found both.

- [ ] **Step 3: Implement `patches.py` and `launcher.py`**

Start from the spike's `entrypoint.py`:
- the forced-IPC block (`:69-99`);
- XDG setup (`:169-172`);
- `sys.argv = ["marimo", *argv]` before `marimo._cli.cli.main.main(args=argv, standalone_mode=True)` (`:201-205`).

Production differences:
- No pool or forkserver: that is P5's choice.
- The venv path is `/opt/egret-apps/venv-shim`, overridable for tests with `EGRET_VENV_SHIM`.
- The marimo token comes from the environment, never from argv defaults.

- [ ] **Step 4: Write `tests/e2e/test_edit_and_run.py`**

Mark it `@pytest.mark.e2e`. These tests are automated Playwright. Each test starts `fake_egret` and a launcher subprocess (`.venv/bin/egret-apps edit|run` with the environment set), then puts a minimal header-injecting proxy in front. That proxy can be a few lines with Starlette and httpx, or `fake_egret` can also serve as the proxy; either way it injects `x-egret-app-token`, choosing the token by a test cookie. Chromium then drives it:

1. **Run mode, D7 and §4.1:**
   - two browser contexts (viewers alice and bob) open the app;
   - each page shows `os.getpid()` and the viewer the gateway returned;
   - assert the pids differ, the pids are not the server's pid, each page shows its own viewer, and `fake_egret.STATE.queries` records the right token per query, including after a reactive re-run (a slider).
2. **Edit mode, §5.1/§5.3:**
   - open the editor, type into a cell, and save with Ctrl+S → the fake's draft becomes rev 2;
   - an external PUT makes the server rev 3;
   - save again → the page shows marimo's "Failed to save" toast containing "reload", and the fake still holds the external source;
   - reload the page → the editor shows rev 3's source, and the next save succeeds as rev 4.

Use the test app source from P2's `template.rs` (copy it into `tests/e2e/apps/`), plus a cell `mo.md(f"pid={os.getpid()} viewer={egret.sql('SELECT 1', database='db')['viewer'][0]}")`.

- [ ] **Step 5: Run everything**

```bash
.venv/bin/pytest -q                               # unit + contract
.venv/bin/playwright install chromium && .venv/bin/pytest -q -m e2e
```

Both pass. Record the timings and the pids observed.

- [ ] **Step 6: Mutation checks, then commit**

Do these one at a time, confirm the named test fails, and restore:
1. Remove the run-mode IPC patch → e2e 1 fails (one pid).
2. Make the shim always exec the real Python → the launcher shim test fails. e2e still passes, which is fine: the shim only saves time.
3. Remove the `.marimo.toml` guard → its test fails.

Then commit:

```bash
git add apps-image && git commit -m "feat(apps-image): launcher, run-mode IPC patch, install-probe shim, locked config"
```

---

### Task 4: Image, smoke test, recipes, CI workflow

**Files:**
- Create: `apps-image/Dockerfile`, `apps-image/smoke.sh`, `.github/workflows/apps-image.yml`
- Modify: `justfile` (the `images-build`, `deploy-kind*` `kind load` lines; a new `apps-image-test` recipe)

- [ ] **Step 1: Write the `Dockerfile`**

```dockerfile
FROM python:3.12-slim
WORKDIR /opt/egret-apps
COPY requirements.txt ./
RUN pip install --no-cache-dir -r requirements.txt
COPY pyproject.toml ./
COPY egret_marimo ./egret_marimo
RUN pip install --no-cache-dir --no-deps .
COPY venv-shim ./venv-shim
COPY config ./config
RUN chmod +x venv-shim/bin/python \
 && useradd --uid 1000 --create-home --home-dir /home/egret egret \
 && mkdir -p /work && chown -R 1000:1000 /home/egret /work
ENV HOME=/home/egret \
    XDG_CONFIG_HOME=/opt/egret-apps/config \
    MARIMO_SKIP_UPDATE_CHECK=1 \
    EGRET_REAL_PYTHON=/usr/local/bin/python \
    EGRET_MARIMO_VERSION=0.25.0 \
    EGRET_VENV_SHIM=/opt/egret-apps/venv-shim \
    APPS_WORKDIR=/work
USER 1000
EXPOSE 2718 2719
ENTRYPOINT ["egret-apps"]
```

Then check that `EGRET_MARIMO_VERSION` matches the pin: add a build-time `RUN python -c "import marimo,sys; sys.exit(marimo.__version__ != '0.25.0')"` before `USER`.

- [ ] **Step 2: Write `smoke.sh`, modelled on `kernel-image/smoke.sh`**

1. `docker build -t egret-apps:smoke .`
2. Start `fake_egret` on the host. If the venv is absent, create it with `requirements-dev.txt`.
3. `docker run --rm -d --add-host=host.docker.internal:host-gateway -e EGRET_URL=http://host.docker.internal:<port> -e … egret-apps:smoke run` with a seeded version.
4. Poll `GET /r/<v>/` until 200 (bounded at 120 s), then assert the HTML is marimo's.
5. Repeat with `edit` and `GET /e/<pod>/`.
6. Check `docker exec … id -u` → 1000.
7. Check that the image refuses to start with a `.marimo.toml` mounted at `/work` (exit 65).
8. Clean up the containers.

Print `SMOKE OK` at the end.

- [ ] **Step 3: Update the `justfile`**

- Add `docker build -t egret-apps:dev apps-image` to `images-build`.
- Add `egret-apps:dev` to every `kind load docker-image … --name egret` line (`deploy-kind`, `deploy-kind-oidc`, `deploy-kind-governance`).
- Add a recipe:

```make
apps-image-test:
    cd apps-image && python3.12 -m venv .venv && .venv/bin/pip install -q -r requirements.txt -r requirements-dev.txt -e . && .venv/bin/pytest -q
```

- [ ] **Step 4: Write `.github/workflows/apps-image.yml`**

Copy the shape of `kernel-image.yml`: `on: pull_request: paths: ["apps-image/**"]`, with two jobs.
- `apps-pytest`: setup-python 3.12, install the requirements, `pytest -q`, `playwright install --with-deps chromium`, `pytest -q -m e2e`.
- `apps-image-smoke`: `./smoke.sh`.

- [ ] **Step 5: Run it all locally**

```bash
df -h .
just apps-image-test
.venv/bin/pytest -q -m e2e        # from apps-image/
./apps-image/smoke.sh
docker image ls egret-apps:smoke --format '{{.Size}}'
```

Expected: everything is green and `SMOKE OK` prints. Record the image size, and remove the smoke image afterwards (`docker rmi egret-apps:smoke`).

- [ ] **Step 6: Commit**

```bash
git add apps-image justfile .github/workflows/apps-image.yml
git commit -m "feat(apps-image): egret-apps image, smoke test, recipes and CI workflow"
```

---

## Self-review against the spec

| Spec requirement | Task |
|---|---|
| §4.4 DB-API `connect`, `sql()` → polars, database by name, token from `mo.app_meta().request` | 1 |
| §4.2 401 → "session expired — reload the app" | 1 |
| §5.1 drafts in egret: patched `load`, `EgretStorage`, 409 visible in marimo's UI | 2, 3 (e2e 2) |
| §5.3 pod API contract (client side), one base revision per app, stale-after-409 | 1, 2 |
| §5.2 run mode fetches the version once and never writes back | 3 |
| §5.3 refusal to start on `.marimo.toml`; `[ai] enabled = false` | 3, 4 |
| D7 process per run session (forced IPC), token reaches the right kernel | 3 (e2e 1) |
| D13 DuckDB and sqlglot shipped; default engine via the template's connection cell | 1 (pins), §5.3 note |
| D14 shim answers `has_marimo_installed` | 3 |
| Contract tests that fail on a marimo upgrade | 2, 3 (`test_marimo_contract.py`) |

Out of P3, by design:
- egret's pod endpoints, pod-token minting and the launcher (P4);
- proxy route blocking (P4);
- warm pool or fork server (P5);
- Helm and `egret-config` keys for the apps image (P4, with the launcher).
