# Notebook dataframe-first — follow-up: starrocks() helper + remove the SQL surface — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Bilingual: English canonical; VI reference at `docs/vi/plans/2026-07-01-notebook-dataframe-first-p3-starrocks-remove-sql.md`.

**Goal:** Complete the dataframe-first notebook: add the `starrocks(sql)` Daft-direct helper (transport = DIRECT per the P0 spike), verify all three engines live, then remove the in-notebook SQL surface (`%%egret_sql` magic, SQL cell type + target dropdown, the `query()`/`/kernel/query` materialize path).

**Architecture:** `starrocks(sql)` mirrors `trino(sql)` — `daft.read_sql` over a per-user JWT — but StarRocks speaks the MySQL wire protocol, so it uses a PyMySQL connection with a small custom `authentication_openid_connect_client` auth-plugin handler (proven in the P0 spike). Once all three helpers (`iceberg`/`trino`/`starrocks`) are verified working live, the now-dead SQL surface is removed across frontend, kernel image, and backend, and `notebook_cells.sql_target` is dropped.

**Tech Stack:** Python 3.11 (Daft, PyMySQL, sqlglot, pytest), Rust (axum, sqlx), React 18 + Vite + Vitest, Postgres.

**Spec:** `docs/en/specs/2026-07-01-notebook-dataframe-first-daft-design.md` (Phase 2 StarRocks + Phase 3 remove-SQL). **Spike:** `docs/en/specs/2026-07-01-notebook-starrocks-jwt-spike-findings.md` (§4 recipe, §5 DIRECT recommendation).

**Sequencing note:** the SQL-surface removal (Phase C) runs only AFTER the live verification (Phase B) confirms the three direct helpers work — never remove the old path before the replacement is proven live.

---

## File Structure

- `kernel-image/egret_kernel_helpers/engines.py` — add `starrocks(sql)` + `_openid_connect(jwt)` PyMySQL handler helper. *(A)*
- `kernel-image/Dockerfile` — add `pymysql` + `sqlglot` to the kernel deps. *(A)*
- `kernel-image/egret_kernel_helpers/__init__.py` + `startup/00-egret.py` — export/auto-load `starrocks`; remove `query`/magic exports. *(A, C)*
- `kernel-image/tests/test_engines.py` — starrocks tests. *(A)*
- `kernel-image/egret_kernel_helpers/{magic.py,query.py}` + `tests/{test_magic.py,test_query.py}` — DELETE. *(C)*
- `frontend/src/components/notebook/CellEditor.tsx` (+test) — remove SQL cell type + target dropdown. *(C)*
- `frontend/src/routes/explore/NotebookPage.tsx` (+test) — remove `%%egret_sql` wrapping + `sql_target`. *(C)*
- `frontend/src/api/notebooks.ts` (+test) — remove `sql_target`. *(C)*
- `crates/egret-kernel/src/routes.rs` + `src/query.rs` + `src/lib.rs` — remove `/kernel/query` + `NotebookQueryRunner` seam + `query_runner` field. *(C)*
- `crates/egret-bin/src/main.rs` — remove `EngineQueryRunner` + its wiring. *(C)*
- `crates/egret-notebook/src/{model.rs,store.rs}` + a new migration — drop `sql_target`. *(C)*

---

## Phase A — `starrocks(sql)` helper

### Task A1: kernel-image deps (pymysql + sqlglot)

**Files:**
- Modify: `kernel-image/Dockerfile`

Context: Daft `read_sql` needs `sqlglot` for schema inference; the StarRocks handler needs `pymysql`. Read the Dockerfile's existing pip/uv install line (it already installs `daft`, `pyiceberg[s3fs]`).

- [ ] **Step 1: Add the deps**

Add `pymysql` and `sqlglot` to the same install command that installs `daft`/`pyiceberg` in `kernel-image/Dockerfile` (match the existing pin/format — if versions are pinned elsewhere, pin these too; otherwise unpinned like the neighbors).

- [ ] **Step 2: Commit**

```bash
git add kernel-image/Dockerfile
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "build(kernel-image): add pymysql + sqlglot for starrocks() direct reads"
```
(The image is rebuilt in Phase B; no build here.)

### Task A2: `starrocks(sql)` helper (TDD)

**Files:**
- Modify: `kernel-image/egret_kernel_helpers/engines.py`
- Test: `kernel-image/tests/test_engines.py`

Context: mirror `trino(sql)` (already in `engines.py`): fetch a fresh JWT via `_engine_token(engine, env, cache)` inside the connection factory, return `daft.read_sql(sql, factory)`. StarRocks differs: the factory builds a **PyMySQL** connection with a custom auth-plugin handler for `authentication_openid_connect_client` (from the spike §4). Tests mock `daft` + `pymysql` + `sqlalchemy` via `sys.modules` (the factory is invoked in one test to assert wiring). The winning spike recipe:
```python
# [0x01][lenenc(JWT)][JWT] reply to the auth-switch to authentication_openid_connect_client
```

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_engines.py`:
```python
def test_starrocks_builds_read_sql_with_jwt(monkeypatch):
    import sys
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    cache = {"sr": {"jwt": "JWT", "expires_at": "2999-01-01T00:00:00Z",
                    "kind": "starrocks", "host": "sr", "port": 9030}}
    captured = {}
    fake_daft = type("D", (), {"read_sql": staticmethod(
        lambda sql, conn, **kw: captured.update(sql=sql, conn=conn) or "DF")})
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    out = E.starrocks("SELECT 1", engine="sr", env=env, cache=cache)
    assert out == "DF"
    assert captured["sql"] == "SELECT 1"
    assert callable(captured["conn"])


def test_starrocks_conn_factory_uses_pymysql_openid(monkeypatch):
    import sys
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    cache = {"sr": {"jwt": "JWT", "expires_at": "2999-01-01T00:00:00Z",
                    "kind": "starrocks", "host": "sr", "port": 9030}}
    seen = {}
    def fake_connect(**kw):
        seen["connect_kwargs"] = kw
        return "PYMYSQL_CONN"
    fake_pymysql = type("P", (), {"connect": staticmethod(fake_connect)})
    def fake_create_engine(url, **kw):
        seen["url"] = url; seen["creator"] = kw.get("creator")
        return type("E", (), {"connect": lambda self: seen.setdefault("connected", kw["creator"]())})()
    fake_sa = type("SA", (), {"create_engine": staticmethod(fake_create_engine)})
    fake_daft = type("D", (), {"read_sql": staticmethod(lambda sql, conn, **kw: conn)})
    monkeypatch.setitem(sys.modules, "pymysql", fake_pymysql)
    monkeypatch.setitem(sys.modules, "sqlalchemy", fake_sa)
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    factory = E.starrocks("SELECT 1", engine="sr", env=env, cache=cache)  # read_sql returns the factory
    conn = factory()                    # invoke the connection factory
    assert seen["url"] == "mysql+pymysql://"
    assert callable(seen["creator"])    # pymysql creator closure
    # invoking the creator connects via pymysql with the openid auth_plugin_map + user
    seen["creator"]()
    kw = seen["connect_kwargs"]
    assert kw["host"] == "sr" and kw["port"] == 9030 and kw["user"] == "egret"
    assert "authentication_openid_connect_client" in kw["auth_plugin_map"]
```

- [ ] **Step 2: Run, confirm FAIL**

Run: `.venv/bin/python -m pytest tests/test_engines.py::test_starrocks_builds_read_sql_with_jwt tests/test_engines.py::test_starrocks_conn_factory_uses_pymysql_openid -q`
Expected: FAIL (function missing).

- [ ] **Step 3: Implement `starrocks` + the OpenID handler**

Add to `engines.py`:
```python
import struct

_OPENID_PLUGIN = "authentication_openid_connect_client"


def _lenenc(n):
    if n < 0xFB:
        return bytes([n])
    if n < 0x1_0000:
        return b"\xfc" + struct.pack("<H", n)
    if n < 0x100_0000:
        return b"\xfd" + struct.pack("<I", n)[:3]
    return b"\xfe" + struct.pack("<Q", n)


def _openid_handler(jwt):
    """A PyMySQL custom auth-plugin class for StarRocks'
    `authentication_openid_connect_client`: replies to the auth-switch with
    [0x01][lenenc(JWT)][JWT] (no trailing NUL). See the P0 spike findings §4."""
    class _Handler:
        def __init__(self, con):
            self._con = con

        def authenticate(self, pkt):
            data = b"\x01" + _lenenc(len(jwt.encode())) + jwt.encode()
            self._con.write_packet(data)
            reply = self._con._read_packet()
            reply.check_error()
            return reply

    return _Handler


def starrocks(sql, *, engine="starrocks", env=None, cache=None):
    """Run SQL on the workspace StarRocks as you, returning a lazy Daft
    DataFrame (pushdown). Authenticated with a fresh per-user JWT over the MySQL
    protocol via a custom OpenID-Connect auth-plugin handler."""
    import daft

    def _conn():
        import pymysql
        from sqlalchemy import create_engine
        tok = _engine_token(engine, env=env, cache=cache)

        def _creator():
            # user is cosmetic (identity comes from the JWT); JWT carries auth.
            return pymysql.connect(
                host=tok["host"], port=tok["port"], user="egret", password="",
                auth_plugin_map={_OPENID_PLUGIN: _openid_handler(tok["jwt"])},
            )

        eng = create_engine("mysql+pymysql://", creator=_creator)
        return eng.connect()

    return daft.read_sql(sql, _conn)
```
Adjust to match the test's expectations (the test invokes `_conn` and its `creator`). Keep imports lazy inside `_conn`/`_creator`.

- [ ] **Step 4: Run, confirm PASS**

Run: `.venv/bin/python -m pytest tests/test_engines.py -q` then `.venv/bin/python -m pytest -q`.
Expected: all pass.

- [ ] **Step 5: Export + auto-load**

In `kernel-image/egret_kernel_helpers/__init__.py`: add `starrocks` to the `from .engines import ...` line and to `__all__`. In `kernel-image/startup/00-egret.py`: add `starrocks` to the `from egret_kernel_helpers import ...` auto-load line. In `tests/test_init.py`: add `"starrocks"` to the presence check. Run `.venv/bin/python -m pytest tests/test_init.py -q`.

- [ ] **Step 6: Commit**

```bash
git add kernel-image/egret_kernel_helpers/engines.py kernel-image/egret_kernel_helpers/__init__.py kernel-image/startup/00-egret.py kernel-image/tests/test_engines.py kernel-image/tests/test_init.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-helpers): starrocks(sql) -> lazy Daft DataFrame via PyMySQL OpenID JWT"
```

---

## Phase B — Live verification (manual checkpoint; gates Phase C)

### Task B1: Verify all three helpers live, as the user

**Files:** none (verification only).

- [ ] **Step 1: Rebuild the kernel image**

Rebuild the D1a kernel image so it includes `pymysql`/`sqlglot` and the new `engines.py` (follow the kernel-image build in `deploy/`/`kernel-image` README). Restart the backend so new kernels use it.

- [ ] **Step 2: Ensure a JWT user + enabled engines**

With the OIDC stack up: log in as a user who HAS a refresh token (so `EngineTokenMinter` can mint) — e.g. an OIDC SSO user; and ensure the workspace has Trino + StarRocks engines enabled for notebooks (`notebook_engine_access`) and a provisioned Iceberg warehouse.

- [ ] **Step 3: Run each helper in a notebook cell**

```python
iceberg("notebook_<you>.<table>").show()          # native
trino("SELECT 1 AS n").show()                     # read_sql + JWT
starrocks("SELECT current_user() AS u").show()    # pymysql openid + JWT — expect your identity
```
Expected: each renders a Daft DataFrame; `starrocks` shows your identity as `current_user()`. If `starrocks` fails on TLS/plugin, consult spike §4/§5 (the FE may need TLS in prod; the PyMySQL handler works plaintext + TLS).

- [ ] **Step 4: Record the outcome**

If all three pass → proceed to Phase C. If `starrocks` fails, STOP and report — do not remove the old SQL path until the replacement works. (Iceberg+Trino removal could still proceed, but keep the plan atomic: fix starrocks first.)

---

## Phase C — Remove the SQL surface

### Task C1: Frontend — drop the SQL cell type + target dropdown

**Files:**
- Modify: `frontend/src/components/notebook/CellEditor.tsx` (+ `__tests__/CellEditor.test.tsx`)
- Modify: `frontend/src/routes/explore/NotebookPage.tsx` (+ `__tests__/NotebookPage.test.tsx`)
- Modify: `frontend/src/api/notebooks.ts` (+ `__tests__/notebooks.test.tsx`)

Context (current code): `CellEditor.tsx` has `<SelectItem value="sql">SQL</SelectItem>` (line ~71) and a target `<Select ... aria-label="SQL target">` shown when `cell.cell_type === "sql"` (lines ~75-77), plus an `onChangeSqlTarget` prop and `monacoLanguage = cell.cell_type === "sql" ? "sql" : "python"`. `NotebookPage.tsx` wraps SQL sources as `` `%%egret_sql --target ${cell.sql_target||"daft"}\n${source}` `` in `handleRun` (line ~114), passes `onChangeSqlTarget` (line ~149), and references `cell.sql_target`. `api/notebooks.ts` has `sql_target` on the cell type (line 14) and in the update mutation (lines 146-149).

- [ ] **Step 1: Update the failing tests first (TDD-for-removal)**

In `CellEditor.test.tsx`: change/remove assertions that expect the "SQL" option or the "SQL target" control; add an assertion that the cell-type select offers only "Code" and "Markdown" and that no element with `aria-label="SQL target"` renders. In `NotebookPage.test.tsx`: remove/adjust any test asserting `%%egret_sql` wrapping or `sql_target`; assert a run sends the raw cell source (no `%%egret_sql` prefix). In `notebooks.test.tsx`: drop `sql_target` expectations. Run them — they should FAIL against the current code (which still has SQL).

Run: `cd frontend && npx vitest run src/components/notebook src/routes/explore src/api/__tests__/notebooks.test.tsx`
Expected: FAIL (current code still renders SQL / wraps egret_sql).

- [ ] **Step 2: Remove SQL from `CellEditor.tsx`**

Delete the `<SelectItem value="sql">SQL</SelectItem>`, the entire `{cell.cell_type === "sql" && onChangeSqlTarget && (…target Select…)}` block, the `onChangeSqlTarget` prop from the component's props type + signature, and simplify `monacoLanguage` to `"python"` (only code cells use Monaco now; markdown uses the textarea). Remove now-unused imports.

- [ ] **Step 3: Remove SQL from `NotebookPage.tsx`**

In `handleRun`, send `source` directly (delete the `cell.cell_type === "sql" ? \`%%egret_sql …\` : source` conditional — just use `source`). Remove the `onChangeSqlTarget={...}` prop passed to `CellEditor`, the `sql_target` references, and drop `cell.sql_target` from the `useMemo` deps. Remove `sql_target` from any local cell type.

- [ ] **Step 4: Remove `sql_target` from `api/notebooks.ts`**

Delete the `sql_target: string | null;` field from the cell interface and the `sql_target?` param + body key in the update mutation (lines 146-149).

- [ ] **Step 5: Run tests + typecheck + lint**

Run: `cd frontend && npx vitest run && npx tsc --noEmit && npm run lint`
Expected: green (the tests from Step 1 now pass; no dangling `sql_target`/`onChangeSqlTarget` references — tsc catches any).

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/notebook/CellEditor.tsx frontend/src/routes/explore/NotebookPage.tsx frontend/src/api/notebooks.ts frontend/src/components/notebook/__tests__/CellEditor.test.tsx frontend/src/routes/explore/__tests__/NotebookPage.test.tsx frontend/src/api/__tests__/notebooks.test.tsx
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(notebook-ui): remove SQL cell type + target dropdown (dataframe-first)"
```

### Task C2: Kernel image — delete the magic + query() helper

**Files:**
- Delete: `kernel-image/egret_kernel_helpers/magic.py`, `kernel-image/egret_kernel_helpers/query.py`
- Delete: `kernel-image/tests/test_magic.py`, `kernel-image/tests/test_query.py`
- Modify: `kernel-image/egret_kernel_helpers/__init__.py`, `kernel-image/startup/00-egret.py`

Context: `query()` → `POST /kernel/query` is the old materialize path, fully replaced by `iceberg`/`trino`/`starrocks`. `magic.py` (`%%egret_sql`) is removed with the SQL cell type. `__init__.py` currently does `from .magic import load_ipython_extension, run_egret_sql` and `from .query import query` and lists them in `__all__`. `startup/00-egret.py` imports `query` and calls `load_ipython_extension` for the magic.

- [ ] **Step 1: Delete the modules + their tests**

```bash
git rm kernel-image/egret_kernel_helpers/magic.py kernel-image/egret_kernel_helpers/query.py kernel-image/tests/test_magic.py kernel-image/tests/test_query.py
```

- [ ] **Step 2: Clean `__init__.py`**

Remove `from .magic import load_ipython_extension, run_egret_sql`, `from .query import query`, and remove `query`, `run_egret_sql`, `load_ipython_extension` from `__all__`. (Keep `iceberg`, `trino`, `starrocks`, `catalog`, `display`, `runtime`, `me`.)

- [ ] **Step 3: Clean `startup/00-egret.py`**

Remove `query` from the auto-load import line and delete the entire second `try:` block that loads the `%%egret_sql` magic (`load_ipython_extension` / "egret %%egret_sql magic not loaded").

- [ ] **Step 4: Run the suite**

Run: `cd kernel-image && .venv/bin/python -m pytest -q`
Expected: green (no references to the deleted modules; `test_init` no longer expects `query`/magic — update it if it did). Also `.venv/bin/python -c "import ast; ast.parse(open('startup/00-egret.py').read()); print('ok')"`.

- [ ] **Step 5: Commit**

```bash
git add -A kernel-image
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "refactor(kernel-helpers): remove %egret_sql magic + query() (replaced by direct helpers)"
```

### Task C3: Backend — remove `/kernel/query` + the `NotebookQueryRunner` seam

**Files:**
- Modify: `crates/egret-kernel/src/routes.rs` (remove route + `kernel_query` handler + `query_runner` field)
- Delete: `crates/egret-kernel/src/query.rs`
- Modify: `crates/egret-kernel/src/lib.rs` (drop `mod query;` + re-exports)
- Modify: `crates/egret-bin/src/main.rs` (remove `EngineQueryRunner` + wiring)
- Modify: `crates/egret-kernel/tests/http.rs` (remove `kernel_query` tests + `FakeQueryRunner` wiring)

Context: `KernelState.query_runner: Arc<dyn NotebookQueryRunner>` + the `/kernel/query` route + `kernel_query` handler are the old path. `query.rs` defines the seam + `FakeQueryRunner`. `egret-bin` has `EngineQueryRunner` implementing it. All dead once C2 lands. Keep `notebook_runtime_token` and `engine-token` (still used). Keep `notebook_engine_access` (gates engine-token).

- [ ] **Step 1: Remove from `routes.rs`**

Delete the `.route("/kernel/query", post(kernel_query))` registration, the `kernel_query` handler + its request/response types, and the `query_runner` field from `KernelState`.

- [ ] **Step 2: Delete the seam**

`git rm crates/egret-kernel/src/query.rs`. In `lib.rs`, remove `pub mod query;` and the `pub use query::{...}` re-export line.

- [ ] **Step 3: Remove from egret-bin**

In `crates/egret-bin/src/main.rs`, delete `struct EngineQueryRunner` + its `impl NotebookQueryRunner` + the `query_runner: Arc::new(EngineQueryRunner{...})` line in the `KernelState { ... }` construction.

- [ ] **Step 4: Fix tests**

In `crates/egret-kernel/tests/http.rs`, remove the `kernel_query` tests and the `query_runner`/`FakeQueryRunner` wiring in the test `KernelState` setup. Ensure `setup()`/state construction compiles without the field.

- [ ] **Step 5: Build + test + clippy**

Run:
```bash
cargo build --workspace
cargo clippy -p egret-kernel -p egret-bin --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-kernel -p egret-notebook -p egret-bin
```
Expected: green. (The compiler will flag every remaining reference to the removed symbols — chase them down.)

- [ ] **Step 6: Commit**

```bash
cargo fmt -p egret-kernel -p egret-bin
git add -A crates
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "refactor(kernel): remove /kernel/query + NotebookQueryRunner (dead after direct helpers)"
```

### Task C4: Drop the `sql_target` column

**Files:**
- Create: `migrations/00XX_drop_notebook_sql_target.sql` (next migration number)
- Modify: `crates/egret-notebook/src/model.rs` (remove `sql_target` from `Cell`/DTOs), `crates/egret-notebook/src/store.rs` (remove from SELECT/INSERT/UPDATE)

Context: `notebook_cells.sql_target` (migration 0046) is now unused (FE no longer writes it; magic is gone). `model.rs` `Cell` has `pub sql_target: Option<String>` and `UpdateCell`/`CreateCell` may reference it; `store.rs` reads/writes it.

- [ ] **Step 1: Write the migration**

Create `migrations/00XX_drop_notebook_sql_target.sql` (use the next sequential number after the highest existing):
```sql
ALTER TABLE notebook_cells DROP COLUMN sql_target;
```

- [ ] **Step 2: Remove from the Rust model + store**

Delete `sql_target` from `Cell` (model.rs) and any `UpdateCell`/`CreateCell` field, and remove it from every `store.rs` query (SELECT column list + row mapping + any INSERT/UPDATE that set it). Let the compiler guide you to all sites.

- [ ] **Step 3: Force migrator re-embed + build/test**

Touch the metastore lib so the embedded migrator picks up the new migration (per the repo's sqlx-migration-staleness note): `touch crates/egret-metastore/src/lib.rs`. Then:
```bash
cargo build --workspace
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo run -p egret-bin -- migrate
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-notebook
```
Expected: migration applies; tests green.

- [ ] **Step 4: fmt/clippy + commit**

```bash
cargo fmt -p egret-notebook
cargo clippy -p egret-notebook --all-targets -- -D warnings
git add migrations/ crates/egret-notebook/src/model.rs crates/egret-notebook/src/store.rs crates/egret-metastore/src/lib.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "refactor(notebook): drop unused notebook_cells.sql_target column"
```

---

## Task D1: Phase-close verification

**Files:** none.

- [ ] **Step 1: Full local CI**

```bash
cd egret
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test --workspace
cd frontend && npm run lint && npx tsc --noEmit && npx vitest run && npm run build
cd ../kernel-image && .venv/bin/python -m pytest -q
```
Expected: all green (modulo the known vendored `mysql_async` warning + the 2 pre-existing egret-engine NULL-workspace tests).

- [ ] **Step 2: Final review + finish**

Dispatch a final whole-branch review, then use `superpowers:finishing-a-development-branch`.

---

## Notes / risks

- **starrocks TLS.** The PyMySQL OpenID handler works over plaintext AND TLS (spike §5). If a prod StarRocks FE enforces TLS, the same helper works; no code change. The official `mysql-connector-python` OpenID plugin (rejected) needs TLS — we deliberately use PyMySQL.
- **Removal ordering.** Phase C runs only after Phase B confirms the three helpers live. Removing `/kernel/query`/`query()` before that would strand notebook engine access.
- **`notebook_runtime_token` + `notebook_engine_access` are KEPT** — the manifest token refresh and the engine enable-gate are still used by the direct path.
- **`sql_target` drop is destructive** — existing cells lose that column (it's unused; acceptable). The migration is one-way; no down-migration needed (repo convention).
- **Frontend removal relies on tsc** to catch every dangling `sql_target`/`onChangeSqlTarget` reference — run `npx tsc --noEmit` before committing C1.
