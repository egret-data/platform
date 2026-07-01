# Notebook E-P1 — SQL magic + sql_target + interrupt/restart Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** The execution foundations: a `%%egret_sql --target daft|<engine>` kernel cell-magic, a per-cell `sql_target` column, backend interrupt/restart passthroughs to the Kernel Gateway, and the `last_output` JSON contract + size cap (consumed by the E-P2 recorder).

**Architecture:** Python magic in the D1a kernel image routes SQL to `daft.sql()` (Daft target) or the D1a `query()` helper (governed engine target, per-user via F2). Rust: extend the C1 cell PATCH with `sql_target`; add gateway interrupt/restart HTTP calls + kernel routes; define a shared `last_output` output-item type + 256 KB cap.

**Tech Stack:** Python (IPython magics, Daft), Rust (axum, sqlx, reqwest), the D1a kernel image + D1c gateway client.

---

## File structure
- `kernel-image/egret_kernel_helpers/magic.py` (new) — the `%%egret_sql` cell magic.
- `kernel-image/egret_kernel_helpers/__init__.py` + `startup/00-egret.py` — register the magic.
- `kernel-image/tests/test_magic.py` (new).
- `migrations/0046_notebook_cell_sql_target.sql` (new).
- `crates/egret-notebook/src/model.rs` + `store.rs` + `routes.rs` — `sql_target` on `UpdateCell` + persistence.
- `crates/egret-kernel/src/gateway.rs` — `interrupt_kernel` / `restart_kernel`.
- `crates/egret-kernel/src/routes.rs` — `POST …/notebooks/{nb}/kernel/interrupt` + `…/restart`.
- `crates/egret-notebook/src/output.rs` (new) — `CellOutput` item type + `MAX_OUTPUT_BYTES` + `cap_outputs`.

---

## Task 1: `%%egret_sql` cell magic (TDD, kernel image)

**Files:** Create `kernel-image/egret_kernel_helpers/magic.py`, `kernel-image/tests/test_magic.py`; modify `__init__.py`, `startup/00-egret.py`.

- [ ] **Step 1: Failing test** `kernel-image/tests/test_magic.py` (fakes `daft` + the `query` helper — no daft/network needed, mirrors the D1a `test_query.py` style):
```python
import sys, types
from egret_kernel_helpers import magic as m

def test_daft_target_calls_daft_sql(monkeypatch):
    calls = {}
    fake_daft = types.SimpleNamespace(sql=lambda s: calls.setdefault("sql", s) or "DF")
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    out = m.run_egret_sql("--target daft", "SELECT 1")
    assert calls["sql"] == "SELECT 1"
    assert out == "DF"

def test_engine_target_calls_query(monkeypatch):
    calls = {}
    monkeypatch.setattr(m, "query", lambda engine, sql: calls.update(engine=engine, sql=sql) or "QDF")
    out = m.run_egret_sql("--target trino", "SELECT 2")
    assert calls == {"engine": "trino", "sql": "SELECT 2"}
    assert out == "QDF"

def test_default_target_is_daft(monkeypatch):
    fake_daft = types.SimpleNamespace(sql=lambda s: "DF")
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    assert m.run_egret_sql("", "SELECT 3") == "DF"
```

- [ ] **Step 2: Run — fails** `cd kernel-image && python -m pytest tests/test_magic.py -q` → FAIL (module missing).

- [ ] **Step 3: Implement** `kernel-image/egret_kernel_helpers/magic.py`:
```python
"""`%%egret_sql --target daft|<engine>` cell magic.

daft   → daft.sql(cell) over the attached Iceberg catalog.
<engine> → the governed query() helper (egret callback → your identity via F2).
The SQL body is passed as a Python value, never string-built, so no escaping issues.
"""
import argparse
import shlex

from .query import query


def _parse_target(line: str) -> str:
    p = argparse.ArgumentParser(prog="%%egret_sql", add_help=False)
    p.add_argument("--target", default="daft")
    args, _ = p.parse_known_args(shlex.split(line))
    return args.target


def run_egret_sql(line: str, cell: str):
    """Core (unit-testable): route the cell SQL to Daft or a governed engine."""
    target = _parse_target(line)
    if target == "daft":
        import daft
        return daft.sql(cell)
    return query(target, cell)


def load_ipython_extension(ipython):
    """Register the %%egret_sql cell magic on the running IPython kernel."""
    from IPython.core.magic import register_cell_magic

    @register_cell_magic("egret_sql")
    def _egret_sql(line, cell):
        return run_egret_sql(line, cell)
```

- [ ] **Step 4: Register the magic.** In `kernel-image/egret_kernel_helpers/__init__.py`, add `from .magic import load_ipython_extension, run_egret_sql` and append `"load_ipython_extension"` to `__all__`. In `kernel-image/startup/00-egret.py`, after the helper import, add:
```python
try:
    from egret_kernel_helpers import load_ipython_extension as _load_egret
    _load_egret(get_ipython())  # noqa: F821 — get_ipython is global in an IPython kernel
except Exception as _e:  # noqa: BLE001
    print(f"egret %%egret_sql magic not loaded: {_e}")
```

- [ ] **Step 5: Run — passes** `cd kernel-image && python -m pytest tests/test_magic.py -q`.

- [ ] **Step 6: Commit**
```bash
git add kernel-image/egret_kernel_helpers/magic.py kernel-image/egret_kernel_helpers/__init__.py kernel-image/startup/00-egret.py kernel-image/tests/test_magic.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-image): %%egret_sql cell magic (daft | governed engine)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
(NOTE: the kernel image itself is rebuilt + smoke-tested in E-P4/CI; the magic exec is validated by pytest here since it needs no live kernel.)

---

## Task 2: `sql_target` on cells (TDD)

**Files:** `migrations/0046_notebook_cell_sql_target.sql`, `crates/egret-notebook/src/model.rs`, `store.rs`, `routes.rs`; touch `crates/egret-metastore/src/lib.rs` (re-embed).

- [ ] **Step 1: Migration** `migrations/0046_notebook_cell_sql_target.sql`:
```sql
-- Notebook E-P1: per-cell SQL execution target (Daft in-kernel, or a governed
-- engine id/name). NULL/'daft' = run via Daft; else the query() engine target.
ALTER TABLE notebook_cells ADD COLUMN sql_target text;
```

- [ ] **Step 2: Extend `UpdateCell`** (`model.rs:92`):
```rust
pub struct UpdateCell {
    pub cell_type: Option<String>,
    pub source: Option<String>,
    pub sql_target: Option<String>,
}
```
(Confirm `#[derive(Debug, Deserialize)]` stays; `serde` will default missing `sql_target` to `None`.)

- [ ] **Step 3: Failing test** — extend `crates/egret-notebook/src/store.rs` tests (mirror `update_cell_partial` at ~line 926):
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn update_cell_sets_sql_target(pool: sqlx::PgPool) {
    let db = Db { pool };
    let store = NotebookStore::new(db.clone());
    let (ws, user) = seed(&db).await;
    let nb = store.create(ws, user, "nb").await.unwrap();
    let cid = store.add_cell(nb, "sql", "SELECT 1", 0).await.unwrap(); // match the real add_cell signature
    store.update_cell(nb, cid, &UpdateCell {
        cell_type: None, source: None, sql_target: Some("trino".into()),
    }).await.unwrap();
    let target: Option<String> = sqlx::query_scalar("SELECT sql_target FROM notebook_cells WHERE id=$1")
        .bind(cid).fetch_one(&db.pool).await.unwrap();
    assert_eq!(target.as_deref(), Some("trino"));
}
```
(Read the real `add_cell`/`seed` signatures in store.rs + its test module and adapt the call.)

- [ ] **Step 4: Run — fails** `cargo test -p egret-notebook update_cell_sets_sql_target` (postgres up + `DATABASE_URL`) → FAIL.

- [ ] **Step 5: Implement** — in `store.rs::update_cell` (line 357) add the column to the UPDATE:
```rust
            "UPDATE notebook_cells SET \
             cell_type  = COALESCE($3, cell_type), \
             source     = COALESCE($4, source), \
             sql_target = COALESCE($5, sql_target), \
             updated_at = now() \
             WHERE id=$1 AND notebook_id=$2",
```
add `.bind(p.sql_target.as_deref())` after the `source` bind. In `routes.rs::update_cell`, no change needed if it forwards the whole `UpdateCell` to `store.update_cell` — verify it does; if it maps fields explicitly, add `sql_target`. Touch `crates/egret-metastore/src/lib.rs` (add `; 0046 adds notebook_cells.sql_target` to the re-embed comment) so `serve` picks up the migration.

- [ ] **Step 6: Run — passes**; also confirm the cell read path (the DTO the list/get returns) includes `sql_target` if the frontend needs it — add it to the cell row struct + SELECTs if the C1 cell DTO doesn't already `SELECT *`.

- [ ] **Step 7: Commit**
```bash
git add migrations/0046_notebook_cell_sql_target.sql crates/egret-notebook/src/model.rs crates/egret-notebook/src/store.rs crates/egret-notebook/src/routes.rs crates/egret-metastore/src/lib.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(notebook): per-cell sql_target column + PATCH support

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Gateway interrupt/restart + kernel routes (TDD)

**Files:** `crates/egret-kernel/src/gateway.rs`, `crates/egret-kernel/src/routes.rs`, test in `crates/egret-kernel/tests/http.rs` (+ the fake gateway in gateway.rs tests).

- [ ] **Step 1: Failing test** — extend the fake-gateway test in `gateway.rs` (it already spawns a fake gateway with `/api/kernels` routes ~line 64-84). Add fake routes `POST /api/kernels/{id}/interrupt` + `/restart` (return 204) and a test:
```rust
#[tokio::test]
async fn interrupt_and_restart_hit_the_gateway() {
    let url = spawn_fake_gateway().await;
    interrupt_kernel(&url, "k1").await.unwrap();
    let new_id = restart_kernel(&url, "k1").await.unwrap(); // restart returns the (same) kernel id
    assert_eq!(new_id, "k1");
}
```

- [ ] **Step 2: Run — fails** `cargo test -p egret-kernel --lib interrupt_and_restart` → FAIL (fns missing).

- [ ] **Step 3: Implement** in `gateway.rs` (next to `delete_kernel`):
```rust
/// Interrupt a running kernel via the Jupyter Kernel Gateway REST API.
pub async fn interrupt_kernel(gateway_url: &str, kernel_id: &str) -> Result<(), KernelError> {
    let resp = reqwest::Client::new()
        .post(format!("{gateway_url}/api/kernels/{kernel_id}/interrupt"))
        .send().await.map_err(|e| KernelError::Gateway(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(KernelError::Gateway(format!("interrupt HTTP {}", resp.status())));
    }
    Ok(())
}

/// Restart a running kernel; returns the kernel id (unchanged for an in-place restart).
pub async fn restart_kernel(gateway_url: &str, kernel_id: &str) -> Result<String, KernelError> {
    let resp = reqwest::Client::new()
        .post(format!("{gateway_url}/api/kernels/{kernel_id}/restart"))
        .send().await.map_err(|e| KernelError::Gateway(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(KernelError::Gateway(format!("restart HTTP {}", resp.status())));
    }
    Ok(kernel_id.to_string())
}
```
(Confirm `KernelError::Gateway` exists — D1c added it; match the exact variant. Add the fake gateway routes in Step 1's harness.)

- [ ] **Step 4: Run — passes** `cargo test -p egret-kernel --lib interrupt_and_restart`.

- [ ] **Step 5: Add the HTTP routes** in `routes.rs`. Register under the session (CSRF-protected) group (NOT the bearer callback group — these are browser-initiated):
```rust
.route("/ws/{workspace}/notebooks/{nb}/kernel/interrupt", post(kernel_interrupt))
.route("/ws/{workspace}/notebooks/{nb}/kernel/restart", post(kernel_restart))
```
Handlers: authenticate via `WsContext` + the notebook-belongs-to-ws/readable check (mirror the existing `…/kernel/connect` handler exactly — copy its auth + kernel/gateway resolution), then resolve `(kernel_id, gateway_url)` the SAME way `connect`/`channels` resolve it (via `st.notebook_kernels` + `st.manager`), and call `gateway::interrupt_kernel` / `restart_kernel`. Return `204`/`{ "kernel_id": ... }`. READ the `connect` handler first and reuse its resolution — do NOT invent a new one.

- [ ] **Step 6: HTTP test** — in `crates/egret-kernel/tests/http.rs`, add a test that POSTs `…/kernel/interrupt` with a valid session and asserts it reaches the (fake) gateway path, mirroring the existing connect test's setup. Run `cargo test -p egret-kernel --test http kernel_interrupt`.

- [ ] **Step 7: Commit**
```bash
git add crates/egret-kernel/src/gateway.rs crates/egret-kernel/src/routes.rs crates/egret-kernel/tests/http.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): interrupt/restart endpoints (gateway REST passthrough)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `last_output` contract + size cap (TDD)

**Files:** Create `crates/egret-notebook/src/output.rs`; add `pub mod output;` to `crates/egret-notebook/src/lib.rs`.

The E-P2 recorder writes `notebook_cells.last_output` (jsonb). Define the shape + cap here so both the recorder and the frontend share one contract.

- [ ] **Step 1: Failing test** `crates/egret-notebook/src/output.rs` (in a `#[cfg(test)] mod`):
```rust
#[test]
fn cap_truncates_oversized_and_marks() {
    let big = CellOutput::Stream { name: "stdout".into(), text: "x".repeat(MAX_OUTPUT_BYTES + 10) };
    let capped = cap_outputs(vec![big]);
    let bytes: usize = serde_json::to_vec(&capped).unwrap().len();
    assert!(bytes <= MAX_OUTPUT_BYTES + 256); // within the cap + a small marker allowance
    assert!(matches!(capped.last(), Some(CellOutput::Truncated { .. })));
}

#[test]
fn cap_keeps_small_outputs_verbatim() {
    let small = vec![CellOutput::Stream { name: "stdout".into(), text: "hi".into() }];
    assert_eq!(cap_outputs(small.clone()).len(), 1);
}
```

- [ ] **Step 2: Run — fails** `cargo test -p egret-notebook cap_truncates` → FAIL (module missing).

- [ ] **Step 3: Implement** `crates/egret-notebook/src/output.rs`:
```rust
//! The `notebook_cells.last_output` contract: a trimmed nbformat-style output
//! list, shared by the E-P2 recorder (writer) and the frontend renderer (reader).
use serde::{Deserialize, Serialize};

/// Server-side persistence cap for a cell's outputs (before the JSONB write).
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CellOutput {
    Stream { name: String, text: String },
    ExecuteResult { data: serde_json::Value }, // MIME bundle {mime: value}
    DisplayData { data: serde_json::Value },
    Error { ename: String, evalue: String, traceback: Vec<String> },
    Truncated { dropped: usize },
}

/// Keep outputs whose cumulative serialized size is within `MAX_OUTPUT_BYTES`;
/// append a `Truncated` marker for the rest.
pub fn cap_outputs(outputs: Vec<CellOutput>) -> Vec<CellOutput> {
    let mut kept: Vec<CellOutput> = Vec::new();
    let mut used = 0usize;
    let mut dropped = 0usize;
    for o in outputs {
        let sz = serde_json::to_vec(&o).map(|v| v.len()).unwrap_or(0);
        if used + sz <= MAX_OUTPUT_BYTES {
            used += sz;
            kept.push(o);
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        kept.push(CellOutput::Truncated { dropped });
    }
    kept
}
```
Add `pub mod output;` to `crates/egret-notebook/src/lib.rs`.

- [ ] **Step 4: Run — passes** `cargo test -p egret-notebook cap_truncates cap_keeps`.

- [ ] **Step 5: Commit**
```bash
git add crates/egret-notebook/src/output.rs crates/egret-notebook/src/lib.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(notebook): last_output CellOutput contract + 256KB cap

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Full local CI + kernel-image pytest

- [ ] **Step 1:** `cd kernel-image && python -m pytest tests/ -q` (via its `.venv`) → all pass (incl. test_magic).
- [ ] **Step 2:** `cargo fmt --all --check` → clean.
- [ ] **Step 3:** `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- [ ] **Step 4:** `cargo test --workspace` (postgres+minio up, `warehouse` bucket, `DATABASE_URL`) → green except the known infra-flaky items; any failure in `egret-notebook`/`egret-kernel` (non-infra) = BLOCKER.
- [ ] **Step 5:** Note in the PR: the kernel image `%%egret_sql` magic is exercised by pytest; a real kernel rebuild + WS exec is validated in E-P3/E-P4 (needs the FE client). No live-kernel test here.

---

## Self-review
- **Spec coverage (E §3.2 magic, §3.3 sql_target + last_output contract/cap, §3.4 interrupt/restart):** Task 1 magic ✔; Task 2 sql_target ✔; Task 3 interrupt/restart ✔; Task 4 last_output contract + cap ✔. Recorder itself = E-P2; FE client/renderer = E-P3/P4.
- **Placeholder scan:** Task 2/3 say "read the real `add_cell`/`seed`/`connect` signatures and adapt" — those helpers exist in-repo; the SQL, magic, gateway calls, and output contract are fully specified. No logic placeholders.
- **Type consistency:** `UpdateCell{cell_type,source,sql_target}`, `run_egret_sql(line,cell)`, `interrupt_kernel(url,id)->()` / `restart_kernel(url,id)->String`, `CellOutput` enum + `cap_outputs(Vec<CellOutput>)->Vec<CellOutput>` + `MAX_OUTPUT_BYTES` — used consistently.
- **Scope:** foundations only; the recorder tap (E-P2) consumes the Task-4 contract; the FE (E-P3/P4) consumes the magic + sql_target + interrupt/restart. Each task independently testable.
