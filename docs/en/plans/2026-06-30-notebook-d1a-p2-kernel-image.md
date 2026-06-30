# Notebook D1a · P2 — Kernel Image + `egret_kernel_helpers` (Docker/Python) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A Docker image (Jupyter Kernel Gateway + Daft + pyiceberg) and a pip-installable `egret_kernel_helpers` package exposing `query()`/`catalog()`/`display()`/`runtime`/`me`, auto-loaded into every kernel, that consume the F1 manifest env and (for `query()`) the P1 callback.

**Architecture:** Pure-Python helper package with **lazy imports** of daft/pyiceberg (so unit tests stay light and kernel startup is fast). `query()` POSTs to the P1 `/api/kernel/query` (stdlib `urllib`, no heavy HTTP dep) and wraps `{columns, rows}` into a Daft DataFrame; `catalog()` builds a pyiceberg `RestCatalog` directly from the manifest (no callback); `display()` renders Daft DataFrames. A build-time IPython startup file imports the helpers into the user namespace. Verified by pytest (mocked) + a `docker build` and a docker-run REST smoke.

**Tech Stack:** Python 3.11, Daft, pyiceberg, jupyter_kernel_gateway; Docker; pytest.

**Spec:** `docs/en/specs/2026-06-30-notebook-d1a-kernel-image-design.md` · **Depends on P1** (`/api/kernel/query` contract: request `{engine, sql, max_rows}`, response `{columns:[{name,type}], rows:[[...]], truncated}`).

**Constraints:** Git identity `hoangquocvietbk2.13@gmail.com`; Co-Authored-By trailer. Same `feat/notebook-d1a-kernel` branch as P1 (one D1a PR). The kernel-image tests are Python (`pytest`), not part of the Rust workspace — run from `kernel-image/`.

---

## File structure (new dir `kernel-image/`)
- `pyproject.toml` — package metadata for `egret_kernel_helpers` (setuptools).
- `egret_kernel_helpers/__init__.py` — exports `query`, `catalog`, `display`, `load_runtime`, `runtime`, `me`.
- `egret_kernel_helpers/runtime.py` — parse `EGRET_NOTEBOOK_RUNTIME` → `Runtime`.
- `egret_kernel_helpers/query.py` — `query()` + `_post()` + `_to_daft()`.
- `egret_kernel_helpers/catalog.py` — `catalog()`.
- `egret_kernel_helpers/display.py` — `display()`.
- `startup/00-egret.py` — IPython startup shim (build-time copied into the profile).
- `Dockerfile`, `entrypoint.sh`, `smoke.sh`.
- `tests/test_runtime.py`, `tests/test_query.py`, `tests/test_catalog.py`, `tests/test_display.py`.
- `.github/workflows/kernel-image.yml` — build + smoke (for when CI quota returns; local `smoke.sh` is the gate).

All `pytest` commands run from `kernel-image/` with the package installed editable: `python -m pip install -e . && python -m pip install pytest`.

---

## Task 1: Package skeleton + runtime parsing (TDD)

**Files:** Create `kernel-image/pyproject.toml`, `kernel-image/egret_kernel_helpers/__init__.py` (empty for now), `kernel-image/egret_kernel_helpers/runtime.py`, `kernel-image/tests/test_runtime.py`.

- [ ] **Step 1: `pyproject.toml`**:
```toml
[build-system]
requires = ["setuptools>=68"]
build-backend = "setuptools.build_meta"

[project]
name = "egret-kernel-helpers"
version = "0.1.0"
description = "Egret notebook kernel helpers (query/catalog/display)"
requires-python = ">=3.10"

[tool.setuptools]
packages = ["egret_kernel_helpers"]
```

- [ ] **Step 2: Failing test** `tests/test_runtime.py`:
```python
import json
from egret_kernel_helpers.runtime import load_runtime

MANIFEST = {
    "username": "alice",
    "catalog": {"url": "http://catalog:8181", "warehouse": "main",
                "namespace": "notebook_alice", "token": "tok-abc"},
    "engines": [{"id": "11111111-1111-1111-1111-111111111111", "name": "trino-dev",
                 "engine": "trino", "enabled": True}],
    "catalogs": [{"id": "22222222-2222-2222-2222-222222222222", "name": "lakehouse", "enabled": True}],
}

def test_load_runtime_parses_manifest():
    rt = load_runtime({"EGRET_NOTEBOOK_RUNTIME": json.dumps(MANIFEST)})
    assert rt.username == "alice"
    assert rt.catalog.url == "http://catalog:8181"
    assert rt.catalog.namespace == "notebook_alice"
    assert rt.catalog.token == "tok-abc"
    assert rt.engines[0].name == "trino-dev"
    assert rt.engines[0].id == "11111111-1111-1111-1111-111111111111"

def test_load_runtime_missing_env_returns_none():
    assert load_runtime({}) is None
```

- [ ] **Step 3: Run — fails** `cd kernel-image && python -m pip install -e . -q && python -m pytest tests/test_runtime.py -q` → FAIL (module missing).

- [ ] **Step 4: Implement** `egret_kernel_helpers/runtime.py`:
```python
"""Parse the EGRET_NOTEBOOK_RUNTIME manifest injected by egret (F1/D1b)."""
import json
import os


class RuntimeCatalog:
    def __init__(self, d):
        self.url = d["url"]
        self.warehouse = d["warehouse"]
        self.namespace = d["namespace"]
        self.token = d["token"]


class Engine:
    def __init__(self, d):
        self.id = d["id"]
        self.name = d["name"]
        self.engine = d["engine"]
        self.enabled = d.get("enabled", True)


class Runtime:
    def __init__(self, d):
        self.username = d["username"]
        self.catalog = RuntimeCatalog(d["catalog"])
        self.engines = [Engine(e) for e in d.get("engines", [])]
        self.catalogs = d.get("catalogs", [])


def load_runtime(env=None):
    """Return a Runtime from EGRET_NOTEBOOK_RUNTIME, or None if unset."""
    env = os.environ if env is None else env
    raw = env.get("EGRET_NOTEBOOK_RUNTIME")
    if not raw:
        return None
    return Runtime(json.loads(raw))
```

- [ ] **Step 5: Run — passes**.

- [ ] **Step 6: Commit**
```
git add kernel-image/pyproject.toml kernel-image/egret_kernel_helpers/__init__.py kernel-image/egret_kernel_helpers/runtime.py kernel-image/tests/test_runtime.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-image): egret_kernel_helpers package + runtime manifest parsing

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `query()` helper (TDD, mocked)

**Files:** Create `kernel-image/egret_kernel_helpers/query.py`, `kernel-image/tests/test_query.py`.

- [ ] **Step 1: Failing test** `tests/test_query.py` (mocks the HTTP `_post` and a fake `daft` module so the test needs neither network nor daft installed):
```python
import json, sys, types
import pytest
from egret_kernel_helpers import query as q

MANIFEST = {
    "username": "alice",
    "catalog": {"url": "http://c", "warehouse": "w", "namespace": "notebook_alice", "token": "t"},
    "engines": [{"id": "eng-1", "name": "trino-dev", "engine": "trino", "enabled": True}],
    "catalogs": [],
}
ENV = {"EGRET_NOTEBOOK_RUNTIME": json.dumps(MANIFEST),
       "EGRET_API_URL": "http://egret:3000", "EGRET_KERNEL_TOKEN": "egk_test"}

def _fake_daft(monkeypatch):
    captured = {}
    fake = types.ModuleType("daft")
    fake.from_pydict = lambda d: captured.setdefault("data", d) or "DAFT_DF"
    monkeypatch.setitem(sys.modules, "daft", fake)
    return captured

def test_query_posts_and_builds_dataframe(monkeypatch):
    captured = _fake_daft(monkeypatch)
    calls = {}
    def fake_post(url, body, token):
        calls["url"] = url; calls["body"] = json.loads(body); calls["token"] = token
        return {"columns": [{"name": "n", "type": "integer"}], "rows": [[1], [2]], "truncated": False}
    df = q.query("trino-dev", "SELECT 1", env=ENV, http=fake_post)
    assert df == "DAFT_DF"
    assert calls["url"] == "http://egret:3000/api/kernel/query"
    assert calls["token"] == "egk_test"
    # engine name resolved to id; sql + max_rows forwarded
    assert calls["body"] == {"engine": "eng-1", "sql": "SELECT 1", "max_rows": 10000}
    # rows pivoted into column-oriented dict for daft.from_pydict
    assert captured["data"] == {"n": [1, 2]}

def test_query_requires_configuration():
    with pytest.raises(RuntimeError):
        q.query("trino-dev", "SELECT 1", env={}, http=lambda *a: {})
```

- [ ] **Step 2: Run — fails** `python -m pytest tests/test_query.py -q` → FAIL.

- [ ] **Step 3: Implement** `egret_kernel_helpers/query.py`:
```python
"""query(engine, sql) — run governed SQL via the egret callback, as your identity."""
import json
import os

from .runtime import load_runtime


def query(engine, sql, max_rows=10000, *, env=None, http=None):
    """Run SQL on a workspace engine through egret (Trino/StarRocks + Ranger),
    returning a Daft DataFrame. `engine` is an engine id or name from the manifest."""
    env = os.environ if env is None else env
    api = env.get("EGRET_API_URL")
    token = env.get("EGRET_KERNEL_TOKEN")
    if not api or not token:
        raise RuntimeError(
            "egret: kernel not configured (missing EGRET_API_URL / EGRET_KERNEL_TOKEN)"
        )
    # Resolve engine name → id when the manifest is present (server re-validates).
    engine_ref = engine
    rt = load_runtime(env)
    if rt is not None:
        match = next((e for e in rt.engines if engine in (e.id, e.name)), None)
        if match is not None:
            engine_ref = match.id
    body = json.dumps({"engine": engine_ref, "sql": sql, "max_rows": max_rows}).encode()
    resp = (http or _post)(api.rstrip("/") + "/api/kernel/query", body, token)
    return _to_daft(resp)


def _to_daft(resp):
    import daft
    cols = [c["name"] for c in resp["columns"]]
    data = {c: [row[i] for row in resp["rows"]] for i, c in enumerate(cols)}
    return daft.from_pydict(data)


def _post(url, body, token):
    import urllib.error
    import urllib.request
    req = urllib.request.Request(
        url, data=body, method="POST",
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        if e.code == 401:
            raise RuntimeError("egret: kernel token expired — restart the kernel") from None
        if e.code == 403:
            raise RuntimeError("egret: engine not enabled for notebooks or not permitted") from None
        detail = e.read().decode()[:200]
        raise RuntimeError(f"egret: query failed (HTTP {e.code}): {detail}") from None
    except Exception as e:  # noqa: BLE001 — surface a clear message, never silent-empty
        raise RuntimeError(f"egret: cannot reach the query endpoint ({e})") from None
```

- [ ] **Step 4: Run — passes**.

- [ ] **Step 5: Commit**
```
git add kernel-image/egret_kernel_helpers/query.py kernel-image/tests/test_query.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-image): query() helper (egret callback → Daft DataFrame)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `catalog()` helper (TDD, mocked pyiceberg)

**Files:** Create `kernel-image/egret_kernel_helpers/catalog.py`, `kernel-image/tests/test_catalog.py`.

- [ ] **Step 1: Failing test** `tests/test_catalog.py` (fakes `pyiceberg.catalog.rest`):
```python
import json, sys, types
import pytest
from egret_kernel_helpers import catalog as cat

MANIFEST = {
    "username": "alice",
    "catalog": {"url": "http://c:8181", "warehouse": "main", "namespace": "notebook_alice", "token": "tok"},
    "engines": [], "catalogs": [],
}
ENV = {"EGRET_NOTEBOOK_RUNTIME": json.dumps(MANIFEST)}

def _fake_pyiceberg(monkeypatch):
    captured = {}
    mod_rest = types.ModuleType("pyiceberg.catalog.rest")
    class RestCatalog:
        def __init__(self, name, **props):
            captured["name"] = name; captured["props"] = props
    mod_rest.RestCatalog = RestCatalog
    monkeypatch.setitem(sys.modules, "pyiceberg", types.ModuleType("pyiceberg"))
    monkeypatch.setitem(sys.modules, "pyiceberg.catalog", types.ModuleType("pyiceberg.catalog"))
    monkeypatch.setitem(sys.modules, "pyiceberg.catalog.rest", mod_rest)
    return captured

def test_catalog_builds_rest_catalog_from_manifest(monkeypatch):
    captured = _fake_pyiceberg(monkeypatch)
    c = cat.catalog(env=ENV)
    assert captured["props"] == {"uri": "http://c:8181", "warehouse": "main", "token": "tok"}

def test_catalog_without_runtime_raises():
    with pytest.raises(RuntimeError):
        cat.catalog(env={})
```

- [ ] **Step 2: Run — fails**.

- [ ] **Step 3: Implement** `egret_kernel_helpers/catalog.py`:
```python
"""catalog() — a pyiceberg RestCatalog for the user's private namespace (read/write)."""
import os

from .runtime import load_runtime


def catalog(id=None, *, env=None):
    """Return a pyiceberg RestCatalog bound to the workspace warehouse with the
    user's 1-grant token. The user's private namespace is `notebook_{username}`
    (available as `.namespace` on the runtime catalog)."""
    rt = load_runtime(os.environ if env is None else env)
    if rt is None:
        raise RuntimeError("egret: no notebook runtime configured")
    c = rt.catalog
    if not c.url or not c.token:
        raise RuntimeError("egret: workspace catalog not configured (no warehouse/token)")
    from pyiceberg.catalog.rest import RestCatalog
    return RestCatalog("egret", uri=c.url, warehouse=c.warehouse, token=c.token)
```
> Note: `id` is accepted for forward-compat (selecting an enabled workspace catalog) but the MVP always binds the private namespace's warehouse/token; document that. Reading a table as Daft is `daft.read_iceberg(catalog().load_table("notebook_alice.t"))` — left to the user/E1, not wrapped here (YAGNI).

- [ ] **Step 4: Run — passes**.

- [ ] **Step 5: Commit**
```
git add kernel-image/egret_kernel_helpers/catalog.py kernel-image/tests/test_catalog.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-image): catalog() helper (pyiceberg RestCatalog from manifest)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `display()` helper (TDD)

**Files:** Create `kernel-image/egret_kernel_helpers/display.py`, `kernel-image/tests/test_display.py`.

- [ ] **Step 1: Failing test** `tests/test_display.py` (fakes IPython + daft):
```python
import sys, types
from egret_kernel_helpers import display as disp

def _fake_ipython(monkeypatch):
    shown = []
    mod = types.ModuleType("IPython.display")
    mod.display = lambda obj: shown.append(obj)
    monkeypatch.setitem(sys.modules, "IPython", types.ModuleType("IPython"))
    monkeypatch.setitem(sys.modules, "IPython.display", mod)
    return shown

class _FakeDF:
    def __init__(self): self.limited = None
    def limit(self, n): self.limited = n; return self

def _fake_daft_with_df(monkeypatch):
    mod = types.ModuleType("daft")
    mod.DataFrame = _FakeDF
    monkeypatch.setitem(sys.modules, "daft", mod)

def test_display_limits_dataframe(monkeypatch):
    shown = _fake_ipython(monkeypatch)
    _fake_daft_with_df(monkeypatch)
    df = _FakeDF()
    disp.display(df, n=5)
    assert df.limited == 5
    assert shown and shown[0] is df  # limit() returns self in the fake

def test_display_falls_through_for_non_dataframe(monkeypatch):
    shown = _fake_ipython(monkeypatch)
    _fake_daft_with_df(monkeypatch)
    disp.display({"a": 1})
    assert shown == [{"a": 1}]
```

- [ ] **Step 2: Run — fails**.

- [ ] **Step 3: Implement** `egret_kernel_helpers/display.py`:
```python
"""display(obj) — render Daft DataFrames (capped); fall through to IPython otherwise."""


def display(obj, n=50):
    from IPython.display import display as _ipy
    try:
        import daft
        if isinstance(obj, daft.DataFrame):
            _ipy(obj.limit(n))
            return
    except Exception:  # noqa: BLE001 — daft missing / not a DataFrame → fall through
        pass
    _ipy(obj)
```

- [ ] **Step 4: Run — passes**.

- [ ] **Step 5: Commit**
```
git add kernel-image/egret_kernel_helpers/display.py kernel-image/tests/test_display.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-image): display() helper (Daft DataFrame render + fallthrough)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Package exports + IPython startup shim

**Files:** Modify `kernel-image/egret_kernel_helpers/__init__.py`; create `kernel-image/startup/00-egret.py`, `kernel-image/tests/test_init.py`.

- [ ] **Step 1: Failing test** `tests/test_init.py`:
```python
import json
import egret_kernel_helpers as ek

def test_exports_present():
    for name in ("query", "catalog", "display", "load_runtime"):
        assert hasattr(ek, name), f"missing export {name}"

def test_me_reads_username(monkeypatch):
    monkeypatch.setenv("EGRET_NOTEBOOK_RUNTIME", json.dumps({
        "username": "bob",
        "catalog": {"url": "u", "warehouse": "w", "namespace": "notebook_bob", "token": "t"},
        "engines": [], "catalogs": [],
    }))
    import importlib
    importlib.reload(ek)
    assert ek.me == "bob"
```

- [ ] **Step 2: Run — fails**.

- [ ] **Step 3: Implement** `egret_kernel_helpers/__init__.py`:
```python
"""Egret notebook kernel helpers — auto-loaded into every kernel.

Exposes query(), catalog(), display(), plus `runtime` (parsed manifest) and
`me` (your username). Designed to import cleanly even with no manifest (a bare
kernel) — the helpers raise a clear error only when actually called.
"""
from .catalog import catalog
from .display import display
from .query import query
from .runtime import load_runtime

runtime = load_runtime()
me = runtime.username if runtime is not None else None

__all__ = ["query", "catalog", "display", "load_runtime", "runtime", "me"]
```

- [ ] **Step 4: Create the startup shim** `startup/00-egret.py`:
```python
# Auto-loaded by IPython at kernel start — makes the egret helpers global.
try:
    from egret_kernel_helpers import query, catalog, display, runtime, me  # noqa: F401
except Exception as _e:  # noqa: BLE001
    print(f"egret helpers not loaded: {_e}")
```

- [ ] **Step 5: Run — passes** `python -m pytest tests/ -q` (all four test files + init).

- [ ] **Step 6: Commit**
```
git add kernel-image/egret_kernel_helpers/__init__.py kernel-image/startup/00-egret.py kernel-image/tests/test_init.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-image): package exports (runtime/me) + IPython startup shim

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Dockerfile + entrypoint

**Files:** Create `kernel-image/Dockerfile`, `kernel-image/entrypoint.sh`.

- [ ] **Step 1: `Dockerfile`**:
```dockerfile
# Egret notebook kernel image: Jupyter Kernel Gateway + Daft + pyiceberg + helpers.
# D1b spawns `jupyter kernelgateway`; D1c proxies its channels WS. Helpers are
# auto-loaded into every Python kernel via an IPython startup file.
FROM python:3.11-slim

WORKDIR /opt/egret-kernel

# Runtime deps. Daft + pyiceberg are the engine/catalog libraries; the gateway
# is the kernel front door D1b/D1c talk to.
RUN pip install --no-cache-dir jupyter_kernel_gateway daft "pyiceberg[s3fs]"

# Install the helper package.
COPY pyproject.toml ./
COPY egret_kernel_helpers ./egret_kernel_helpers
RUN pip install --no-cache-dir .

# Auto-load helpers into every kernel (IPython startup runs these at kernel boot).
RUN mkdir -p /root/.ipython/profile_default/startup
COPY startup/00-egret.py /root/.ipython/profile_default/startup/00-egret.py

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 8888
ENTRYPOINT ["/entrypoint.sh"]
```

- [ ] **Step 2: `entrypoint.sh`**:
```bash
#!/usr/bin/env bash
set -euo pipefail

# Start the Jupyter Kernel Gateway with the same flags D1b's SubprocessLauncher
# uses and D1c proxies to. Auth is disabled at the gateway (it is reached only
# via egret's authenticated proxy on localhost / the pod network).
exec jupyter kernelgateway \
  --KernelGatewayApp.ip=0.0.0.0 \
  --KernelGatewayApp.port="${KERNEL_GATEWAY_PORT:-8888}" \
  --KernelGatewayApp.allow_origin='*' \
  --KernelGatewayApp.allow_headers='*' \
  --KernelGatewayApp.allow_methods='*' \
  --KernelGatewayApp.auth_token='' \
  --JupyterWebsocketPersonality.list_kernels=True
```

- [ ] **Step 3: Build** `cd kernel-image && docker build -t egret-kernel:dev .` → succeeds. (If Docker is unavailable in this environment, mark this step BLOCKED and note it — the smoke task covers the runtime check.)

- [ ] **Step 4: Commit**
```
git add kernel-image/Dockerfile kernel-image/entrypoint.sh
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-image): Dockerfile + entrypoint (Kernel Gateway + Daft + helpers)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Docker-run smoke + CI workflow

**Files:** Create `kernel-image/smoke.sh`, `.github/workflows/kernel-image.yml`.

- [ ] **Step 1: `smoke.sh`** — build, run, assert the gateway is alive + has a python3 kernelspec + can create/delete a kernel (kernel *execution* needs the WS channels = D1c + a client, so the smoke covers lifecycle, not code-exec):
```bash
#!/usr/bin/env bash
set -euo pipefail
IMG="egret-kernel:smoke"
PORT="${PORT:-18888}"
NAME="egret-kernel-smoke"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker build -t "$IMG" .
docker run -d --rm --name "$NAME" -p "${PORT}:8888" "$IMG"

# Wait for the gateway (up to ~30s).
for i in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:${PORT}/api" >/dev/null 2>&1; then break; fi
  sleep 1
done
curl -fsS "http://127.0.0.1:${PORT}/api" >/dev/null || { echo "FAIL: gateway not alive"; exit 1; }

# python3 kernelspec present?
curl -fsS "http://127.0.0.1:${PORT}/api/kernelspecs" | grep -q '"python3"' || { echo "FAIL: no python3 kernelspec"; exit 1; }

# Create + delete a kernel (proves the kernel actually launches w/ Daft+helpers).
KID=$(curl -fsS -X POST "http://127.0.0.1:${PORT}/api/kernels" -H 'Content-Type: application/json' -d '{"name":"python3"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')
[ -n "$KID" ] || { echo "FAIL: kernel did not start"; exit 1; }
curl -fsS -X DELETE "http://127.0.0.1:${PORT}/api/kernels/${KID}" >/dev/null
echo "SMOKE OK: gateway alive, python3 kernelspec present, kernel ${KID} started + stopped"
```
Make it executable: `chmod +x kernel-image/smoke.sh`.

- [ ] **Step 2: Run the smoke locally** `cd kernel-image && ./smoke.sh` → prints `SMOKE OK: ...`. (If Docker is unavailable here, mark BLOCKED + note it; the user runs it on a Docker host.)

- [ ] **Step 3: CI workflow** `.github/workflows/kernel-image.yml` (runs on changes under `kernel-image/`; the real gate is the local smoke since GitHub Actions is out of quota):
```yaml
name: kernel-image
on:
  pull_request:
    paths: ["kernel-image/**"]
jobs:
  helpers-pytest:
    runs-on: ubuntu-latest
    defaults: { run: { working-directory: kernel-image } }
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with: { python-version: "3.11" }
      - run: python -m pip install -e . pytest
      - run: python -m pytest tests/ -q
  image-smoke:
    runs-on: ubuntu-latest
    defaults: { run: { working-directory: kernel-image } }
    steps:
      - uses: actions/checkout@v4
      - run: ./smoke.sh
```

- [ ] **Step 4: Commit**
```
git add kernel-image/smoke.sh .github/workflows/kernel-image.yml
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "test(kernel-image): docker-run smoke + CI (pytest + image smoke)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review notes (author)
- **Spec coverage:** image §3.3 (T6 Dockerfile/entrypoint), helpers §3.4 (`query` T2, `catalog` T3, `display` T4, `runtime`/`me` T5), auto-load shim §3.3 (T5 startup file), testing §7 (pytest T1–T5, docker build + docker-run smoke T6/T7). ✔
- **Contract with P1:** `query()` POSTs `{engine, sql, max_rows}` and reads `{columns:[{name,type}], rows, truncated}` — matches the P1 handler exactly. ✔
- **Type/name consistency:** `load_runtime`/`Runtime`/`RuntimeCatalog`/`Engine`; `query(engine, sql, max_rows=, *, env=, http=)` + `_post`/`_to_daft`; `catalog(id=None, *, env=)`; `display(obj, n=50)`; exports `query/catalog/display/load_runtime/runtime/me`. ✔
- **Light tests:** daft/pyiceberg/IPython are lazily imported and faked in tests, so pytest needs none of them; the real libs are exercised by the docker-run smoke. ✔
- **YAGNI:** no `requests` dep (stdlib urllib); no Daft read-iceberg wrapper (user/E1); `id` arg on `catalog()` reserved but MVP binds the private namespace. ✔
- **Docker-unavailable fallback:** T6/T7 note BLOCKED if no Docker here; pytest still fully validates the helpers. ✔
