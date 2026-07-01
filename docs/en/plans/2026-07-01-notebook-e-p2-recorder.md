# Notebook E-P2 — Output recorder (backend tap) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A backend recorder that taps the kernel↔browser Jupyter WS (the D1c proxy), parses iopub messages, accumulates per-cell outputs, and persists `notebook_cells.last_output`/`execution_count`/`last_execution_time_ms` server-side — durable even if the browser tab closes mid-run. Plus a `GET …/active-executions` endpoint to restore in-flight cells on reopen.

**Architecture:** The proxy seam changes from an arg-less `on_activity: Arc<dyn Fn()>` to `Arc<dyn ChannelTap>` (frame bytes + direction). A `Recorder` (per WS connection) correlates `execute_request` (browser→kernel, carrying `metadata.egret_cell_id` + `header.msg_id`) with iopub replies (`parent_header.msg_id`), accumulates `CellOutput`s (the E-P1 contract), and on `status: idle` persists via a store writer (spawned off the pump). An in-memory `ActiveExecutions` registry tracks running cells for the restore endpoint.

**Tech Stack:** Rust (axum WS, serde_json, tokio), the D1c `BackendWsProxy`, the E-P1 `CellOutput` contract + `notebook_cells` output columns.

**Scope:** backend only. The FE Jupyter client that SETS `metadata.egret_cell_id` + reads `last_output`/active-executions is E-P3. The recorder is testable here by feeding synthetic Jupyter frames — no live kernel/FE needed.

---

## File structure
- `crates/egret-notebook/src/store.rs` — add `record_output(...)` writer.
- `crates/egret-kernel/src/jmsg.rs` (new) — minimal Jupyter message serde structs.
- `crates/egret-kernel/src/recorder.rs` (new) — `ChannelTap` trait, `FrameDir`, `Recorder`, `ActiveExecutions`.
- `crates/egret-kernel/src/proxy.rs` — swap `on_activity: Arc<dyn Fn()>` → `tap: Arc<dyn ChannelTap>`.
- `crates/egret-kernel/src/routes.rs` — build the composite tap (recorder + throttled touch) in `channels`; add `GET …/active-executions`; add `active` to `KernelState`.
- `crates/egret-kernel/src/lib.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-kernel/tests/http.rs` — module exports + wiring.

---

## Task 1: `record_output` store writer (TDD)

**Files:** `crates/egret-notebook/src/store.rs`.

- [ ] **Step 1: Failing test** (mirror `update_cell_sets_sql_target`; uses `output::CellOutput`):
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn record_output_persists_capped_outputs(pool: sqlx::PgPool) {
    use crate::output::CellOutput;
    let db = Db { pool };
    let store = NotebookStore::new(db.clone());
    let (ws, user) = seed(&db).await;
    let nb = store.create(ws, user, "nb").await.unwrap();
    let cid = /* add a cell — real add_cell signature */;
    let outs = vec![CellOutput::Stream { name: "stdout".into(), text: "hi".into() }];
    store.record_output(nb, cid, &outs, Some(3), Some(42)).await.unwrap();
    let (lo, ec, ms): (Option<serde_json::Value>, Option<i32>, Option<i32>) = sqlx::query_as(
        "SELECT last_output, execution_count, last_execution_time_ms FROM notebook_cells WHERE id=$1")
        .bind(cid).fetch_one(&db.pool).await.unwrap();
    assert_eq!(ec, Some(3));
    assert_eq!(ms, Some(42));
    assert_eq!(lo.unwrap()[0]["type"], "stream");
}
```

- [ ] **Step 2: Run — fails** `cargo test -p egret-notebook record_output_persists` (postgres up + DATABASE_URL) → FAIL (method missing).

- [ ] **Step 3: Implement** in `store.rs` (next to `update_cell`), using `cap_outputs` + the IDOR guard:
```rust
    /// Persist a cell's captured outputs (capped), execution count, and elapsed ms.
    /// Called by the E-P2 recorder; `AND notebook_id=$2` guards cross-notebook writes.
    pub async fn record_output(
        &self,
        nb: Uuid,
        cid: Uuid,
        outputs: &[crate::output::CellOutput],
        execution_count: Option<i32>,
        elapsed_ms: Option<i32>,
    ) -> Result<(), NotebookApiError> {
        let capped = crate::output::cap_outputs(outputs.to_vec());
        let json = serde_json::to_value(&capped)
            .map_err(|e| NotebookApiError::from(egret_core::Error::Internal(anyhow::anyhow!(e))))?;
        sqlx::query(
            "UPDATE notebook_cells SET \
             last_output = $3, execution_count = $4, last_execution_time_ms = $5, updated_at = now() \
             WHERE id=$1 AND notebook_id=$2",
        )
        .bind(cid).bind(nb).bind(json).bind(execution_count).bind(elapsed_ms)
        .execute(&self.db.pool).await
        .map_err(|e| NotebookApiError::from(egret_core::Error::Internal(anyhow::anyhow!(e))))?;
        Ok(())
    }
```
(Match the crate's exact error-mapping idiom from `update_cell` — read it and mirror. `NotebookApiError` + `egret_core::Error::Internal(anyhow!)` is the existing pattern.)

- [ ] **Step 4: Run — passes** `cargo test -p egret-notebook record_output_persists` + `cargo build -p egret-notebook`.
- [ ] **Step 5: Commit**
```bash
git add crates/egret-notebook/src/store.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(notebook): NotebookStore::record_output (capped last_output + count + elapsed)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Jupyter message parsing + the Recorder state machine (TDD)

**Files:** Create `crates/egret-kernel/src/jmsg.rs`, `crates/egret-kernel/src/recorder.rs`; add `pub mod jmsg; pub mod recorder;` to `crates/egret-kernel/src/lib.rs`. Add `egret-notebook` dep to egret-kernel if not present (it is — used by KernelState).

- [ ] **Step 1: Jupyter structs** `crates/egret-kernel/src/jmsg.rs`:
```rust
//! Minimal Jupyter Messaging Protocol structs for the recorder tap. The Kernel
//! Gateway `/channels` WS multiplexes each message as a JSON object with a
//! `channel` field. We only deserialize the fields the recorder needs; unknown
//! fields are ignored (serde default). Forwarding stays byte-verbatim in the proxy.
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct JHeader {
    #[serde(default)]
    pub msg_id: String,
    #[serde(default)]
    pub msg_type: String,
}

#[derive(Debug, Deserialize)]
pub struct JMsg {
    #[serde(default)]
    pub header: JHeader,
    #[serde(default)]
    pub parent_header: JHeader,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub content: serde_json::Value,
}

impl Default for JHeader {
    fn default() -> Self { Self { msg_id: String::new(), msg_type: String::new() } }
}

impl JMsg {
    /// Parse a channels-WS text frame; None if not JSON we recognize.
    pub fn parse(bytes: &[u8]) -> Option<JMsg> {
        serde_json::from_slice(bytes).ok()
    }
}
```

- [ ] **Step 2: Failing test** `crates/egret-kernel/src/recorder.rs` (`#[cfg(test)] mod`) — a fake store captures the persisted call:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use egret_notebook::output::CellOutput;
    use std::sync::Mutex as StdMutex;
    use uuid::Uuid;

    #[derive(Default)]
    struct FakeSink { calls: StdMutex<Vec<(Uuid, Vec<CellOutput>, Option<i32>)>> }
    #[async_trait::async_trait]
    impl OutputSink for FakeSink {
        async fn record(&self, _nb: Uuid, cid: Uuid, outs: Vec<CellOutput>, ec: Option<i32>, _ms: Option<i32>) {
            self.calls.lock().unwrap().push((cid, outs, ec));
        }
    }

    fn exec_request(msg_id: &str, cell_id: Uuid) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "header": {"msg_id": msg_id, "msg_type": "execute_request"},
            "metadata": {"egret_cell_id": cell_id},
            "content": {"code": "1"}
        })).unwrap()
    }
    fn iopub(parent: &str, msg_type: &str, content: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "header": {"msg_id": "x", "msg_type": msg_type},
            "parent_header": {"msg_id": parent, "msg_type": "execute_request"},
            "content": content
        })).unwrap()
    }

    #[tokio::test]
    async fn accumulates_and_persists_on_idle() {
        let sink = std::sync::Arc::new(FakeSink::default());
        let cid = Uuid::now_v7();
        let rec = Recorder::new(Uuid::now_v7(), Uuid::now_v7(), sink.clone(), Default::default());
        rec.on_frame(FrameDir::ClientToKernel, &exec_request("m1", cid));
        rec.on_frame(FrameDir::KernelToClient, &iopub("m1", "execute_input", serde_json::json!({"execution_count": 7})));
        rec.on_frame(FrameDir::KernelToClient, &iopub("m1", "stream", serde_json::json!({"name":"stdout","text":"hi"})));
        rec.on_frame(FrameDir::KernelToClient, &iopub("m1", "status", serde_json::json!({"execution_state":"idle"})));
        // persistence is spawned; give it a tick
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, cid);
        assert_eq!(calls[0].2, Some(7));
        assert!(matches!(calls[0].1[0], CellOutput::Stream { .. }));
    }

    #[tokio::test]
    async fn active_executions_tracks_running_then_clears() {
        let reg: ActiveExecutions = Default::default();
        let (nb, user) = (Uuid::now_v7(), Uuid::now_v7());
        let rec = Recorder::new(nb, user, std::sync::Arc::new(FakeSink::default()), reg.clone());
        let cid = Uuid::now_v7();
        rec.on_frame(FrameDir::ClientToKernel, &exec_request("m1", cid));
        assert_eq!(reg.running(nb, user), vec![cid]);
        rec.on_frame(FrameDir::KernelToClient, &iopub("m1", "status", serde_json::json!({"execution_state":"idle"})));
        assert!(reg.running(nb, user).is_empty());
    }
}
```

- [ ] **Step 3: Run — fails** `cargo test -p egret-kernel --lib recorder` → FAIL (module missing).

- [ ] **Step 4: Implement** `crates/egret-kernel/src/recorder.rs`:
```rust
//! Output recorder: taps the Jupyter channels WS, correlates execute_request →
//! iopub by msg_id, accumulates per-cell outputs, and persists them on `idle`.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use uuid::Uuid;

use egret_notebook::output::CellOutput;
use crate::jmsg::JMsg;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FrameDir { ClientToKernel, KernelToClient }

/// Persistence seam (real impl wraps NotebookStore::record_output).
#[async_trait::async_trait]
pub trait OutputSink: Send + Sync {
    async fn record(&self, nb: Uuid, cid: Uuid, outputs: Vec<CellOutput>, execution_count: Option<i32>, elapsed_ms: Option<i32>);
}

/// In-memory registry of currently-running cells per (notebook, user), for the
/// `active-executions` restore endpoint. Cheap clone (Arc).
#[derive(Clone, Default)]
pub struct ActiveExecutions(Arc<Mutex<HashMap<(Uuid, Uuid), Vec<Uuid>>>>);
impl ActiveExecutions {
    pub fn running(&self, nb: Uuid, user: Uuid) -> Vec<Uuid> {
        self.0.lock().unwrap().get(&(nb, user)).cloned().unwrap_or_default()
    }
    fn add(&self, nb: Uuid, user: Uuid, cid: Uuid) {
        self.0.lock().unwrap().entry((nb, user)).or_default().push(cid);
    }
    fn remove(&self, nb: Uuid, user: Uuid, cid: Uuid) {
        if let Some(v) = self.0.lock().unwrap().get_mut(&(nb, user)) { v.retain(|c| *c != cid); }
    }
}

struct Pending { cell_id: Uuid, started: Instant, outputs: Vec<CellOutput>, execution_count: Option<i32> }

pub struct Recorder {
    nb: Uuid,
    user: Uuid,
    sink: Arc<dyn OutputSink>,
    active: ActiveExecutions,
    pending: Mutex<HashMap<String, Pending>>, // execute_request msg_id → accumulator
}

impl Recorder {
    pub fn new(nb: Uuid, user: Uuid, sink: Arc<dyn OutputSink>, active: ActiveExecutions) -> Self {
        Self { nb, user, sink, active, pending: Mutex::new(HashMap::new()) }
    }

    pub fn on_frame(&self, dir: FrameDir, bytes: &[u8]) {
        let Some(m) = JMsg::parse(bytes) else { return };
        match dir {
            FrameDir::ClientToKernel => self.on_client(m),
            FrameDir::KernelToClient => self.on_kernel(m),
        }
    }

    fn on_client(&self, m: JMsg) {
        if m.header.msg_type != "execute_request" { return; }
        let Some(cell_id) = m.metadata.get("egret_cell_id").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok()) else { return };
        self.active.add(self.nb, self.user, cell_id);
        self.pending.lock().unwrap().insert(m.header.msg_id, Pending {
            cell_id, started: Instant::now(), outputs: Vec::new(), execution_count: None,
        });
    }

    fn on_kernel(&self, m: JMsg) {
        let parent = m.parent_header.msg_id;
        if parent.is_empty() { return; }
        // finalize on idle (persist + clear); otherwise accumulate.
        let mut guard = self.pending.lock().unwrap();
        let Some(p) = guard.get_mut(&parent) else { return };
        match m.header.msg_type.as_str() {
            "execute_input" => { p.execution_count = m.content.get("execution_count").and_then(|v| v.as_i64()).map(|n| n as i32); }
            "stream" => {
                let name = m.content.get("name").and_then(|v| v.as_str()).unwrap_or("stdout").to_string();
                let text = m.content.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                p.outputs.push(CellOutput::Stream { name, text });
            }
            "execute_result" => { p.outputs.push(CellOutput::ExecuteResult { data: m.content.get("data").cloned().unwrap_or_default() }); }
            "display_data" => { p.outputs.push(CellOutput::DisplayData { data: m.content.get("data").cloned().unwrap_or_default() }); }
            "error" => { p.outputs.push(CellOutput::Error {
                ename: m.content.get("ename").and_then(|v| v.as_str()).unwrap_or("").into(),
                evalue: m.content.get("evalue").and_then(|v| v.as_str()).unwrap_or("").into(),
                traceback: m.content.get("traceback").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            }); }
            "status" if m.content.get("execution_state").and_then(|v| v.as_str()) == Some("idle") => {
                let p = guard.remove(&parent).unwrap();
                self.active.remove(self.nb, self.user, p.cell_id);
                let (nb, user, cid) = (self.nb, self.user, p.cell_id);
                let ec = p.execution_count;
                let ms = i32::try_from(p.started.elapsed().as_millis()).ok();
                let outs = p.outputs;
                let sink = self.sink.clone();
                let _ = user; // captured for potential per-user store scoping
                tokio::spawn(async move { sink.record(nb, cid, outs, ec, ms).await; });
            }
            _ => {}
        }
    }
}
```
(NOTE: `Instant`/`tokio::time` are unavailable in some deterministic contexts — the plan uses real `Instant` for elapsed, fine for a WS pump. `Uuid::now_v7()` is used in TESTS only, which is allowed; production ids come from the parsed metadata.)

- [ ] **Step 5: Run — passes** `cargo test -p egret-kernel --lib recorder` + `cargo build -p egret-kernel` + `cargo clippy -p egret-kernel --lib -- -D warnings`.
- [ ] **Step 6: Commit**
```bash
git add crates/egret-kernel/src/jmsg.rs crates/egret-kernel/src/recorder.rs crates/egret-kernel/src/lib.rs crates/egret-kernel/Cargo.toml
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): Jupyter message parse + output Recorder state machine

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Change the proxy seam to `ChannelTap` + wire the recorder

**Files:** `crates/egret-kernel/src/recorder.rs` (add the trait), `proxy.rs`, `routes.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-kernel/tests/http.rs`.

- [ ] **Step 1: Define `ChannelTap`** in `recorder.rs` and impl it for `Recorder`:
```rust
/// Seam the proxy calls on every forwarded frame (both directions), with bytes.
pub trait ChannelTap: Send + Sync {
    fn on_frame(&self, dir: FrameDir, bytes: &[u8]);
}
impl ChannelTap for Recorder {
    fn on_frame(&self, dir: FrameDir, bytes: &[u8]) { Recorder::on_frame(self, dir, bytes) }
}
```
(Rename the inherent `Recorder::on_frame` if it collides — keep the inherent method + have the trait delegate, or move the body into the trait impl. Pick one and be consistent.)

- [ ] **Step 2: Swap the proxy seam.** In `proxy.rs`: change the `KernelProxy::proxy_channels` param + `run_proxy` param from `on_activity: Arc<dyn Fn() + Send + Sync>` to `tap: Arc<dyn crate::recorder::ChannelTap>`. In the pump's two `Some(Ok(msg))` arms, replace `on_activity();` with a byte-extracting call BEFORE the verbatim mapper:
```rust
        // browser → kernel arm:
        if let Some(b) = frame_bytes(&msg) { tap.on_frame(FrameDir::ClientToKernel, b); }
        // kernel → browser arm:
        if let Some(b) = frame_bytes_tg(&msg) { tap.on_frame(FrameDir::KernelToClient, b); }
```
Add small helpers `fn frame_bytes(m: &AxMessage) -> Option<&[u8]>` (Text→as_bytes, Binary→slice, else None) and `fn frame_bytes_tg(m: &TgMessage) -> Option<&[u8]>`. Keep `ax_to_tung`/`tung_to_ax` verbatim.

- [ ] **Step 3: Update the proxy unit test.** The existing `proxy_forwards_both_ways_and_counts_activity` uses a counting `on_activity` closure — replace with a `CountTap { n: AtomicUsize }` implementing `ChannelTap` (increment on each `on_frame`); assert the count as before. Keep the forwarding assertions unchanged.

- [ ] **Step 4: Build the composite tap in `channels`** (`routes.rs`). Build a tap that does BOTH the throttled `notebook_kernels`/manager touch (existing behavior) AND recording:
```rust
struct ChannelsTap { rec: Recorder, touch: Arc<dyn Fn() + Send + Sync> }
impl ChannelTap for ChannelsTap {
    fn on_frame(&self, dir: FrameDir, bytes: &[u8]) {
        (self.touch)();
        self.rec.on_frame(dir, bytes);
    }
}
```
Construct it in `channels` with the existing throttled closure as `touch`, and `Recorder::new(nb, ctx.principal.user_id.0, Arc::new(StoreSink(st.notebook.store.clone())), st.active.clone())`. Where `StoreSink` wraps `NotebookStore`:
```rust
struct StoreSink(egret_notebook::NotebookStore);
#[async_trait::async_trait]
impl OutputSink for StoreSink {
    async fn record(&self, nb: Uuid, cid: Uuid, outs: Vec<CellOutput>, ec: Option<i32>, ms: Option<i32>) {
        let _ = self.0.record_output(nb, cid, &outs, ec, ms).await; // best-effort; log on error
    }
}
```
Pass `Arc::new(the_tap)` to `proxy.proxy_channels(...)`.

- [ ] **Step 5: Add `active: ActiveExecutions` to `KernelState`** (routes.rs) and construct one in `crates/egret-bin/src/main.rs` (`ActiveExecutions::default()`), and in `crates/egret-kernel/tests/http.rs` `setup()`.

- [ ] **Step 6: Verify** `cargo build --workspace`, `cargo test -p egret-kernel` (proxy + http still pass), `cargo clippy -p egret-kernel --all-targets -- -D warnings`, `cargo fmt --all`.
- [ ] **Step 7: Commit**
```bash
git add crates/egret-kernel/src/recorder.rs crates/egret-kernel/src/proxy.rs crates/egret-kernel/src/routes.rs crates/egret-bin/src/main.rs crates/egret-kernel/tests/http.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): proxy ChannelTap seam + wire the output recorder into channels

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `GET …/active-executions` endpoint (TDD)

**Files:** `crates/egret-kernel/src/routes.rs`, test `crates/egret-kernel/tests/http.rs`.

- [ ] **Step 1: Failing http test** — mirror the interrupt test's auth setup; seed the `ActiveExecutions` registry with a running cell for (nb, user), GET `…/notebooks/{nb}/kernel/active-executions`, assert the JSON `{ "running": [cid] }`; a non-member → 403/404.
- [ ] **Step 2: Run — fails** (route missing → 404).
- [ ] **Step 3: Implement** the handler (session-authed, same `WsContext` + notebook-readable check as interrupt): resolve `(nb, user)`, return `Json(json!({ "running": st.active.running(nb, ctx.principal.user_id.0) }))`. Register in the session (CSRF) router:
```rust
.route("/ws/{workspace}/notebooks/{nb}/kernel/active-executions", get(active_executions))
```
- [ ] **Step 4: Run — passes**; `cargo build --workspace` + clippy + fmt.
- [ ] **Step 5: Commit**
```bash
git add crates/egret-kernel/src/routes.rs crates/egret-kernel/tests/http.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): GET active-executions (restore in-flight cells)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Full local CI

- [ ] **Step 1:** `cargo fmt --all --check` → clean (third_party/mysql_async excluded — not a workspace member).
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- [ ] **Step 3:** `cargo test --workspace` (postgres+minio up, `warehouse` bucket, DATABASE_URL) → green except known infra-flaky; any failure in `egret-notebook`/`egret-kernel` = BLOCKER. Confirm `recorder` unit tests + `record_output_persists` + `active-executions` http tests pass.
- [ ] **Step 4:** Note in the PR: the recorder is unit-tested with synthetic Jupyter frames; a live-kernel end-to-end (real iopub → DB) is validated with the FE client in E-P3/P4.

---

## Self-review
- **Spec coverage (E §3.4 recorder + active-executions):** Task 1 output writer ✔; Task 2 message parse + recorder accumulate/correlate/persist-on-idle ✔; Task 3 proxy tap seam + wiring (recorder owns persistence; FE does NOT PATCH) ✔; Task 4 active-executions ✔.
- **Placeholder scan:** Task 1/4 say "mirror update_cell / the interrupt test" — those exist in-repo; the recorder state machine, message structs, store writer, and seam change are fully specified. No logic placeholders.
- **Type consistency:** `CellOutput` (E-P1) reused; `Recorder::new(nb,user,Arc<dyn OutputSink>,ActiveExecutions)`; `ChannelTap::on_frame(FrameDir,&[u8])`; `record_output(nb,cid,&[CellOutput],Option<i32>,Option<i32>)`; `ActiveExecutions::running(nb,user)->Vec<Uuid>` — used consistently across tasks.
- **Security:** the recorder persists via the same `AND notebook_id=$2` IDOR guard; the active-executions endpoint reuses the interrupt handler's auth + notebook-readable check; the recorder is scoped to the connection's `(nb, user)` (from the authed channels handler), so it can only write that user's own notebook cells.
- **Correctness:** best-effort persistence (spawned, errors logged) — a DB hiccup never breaks the live WS forward; the FE still renders live (E-P3), the recorder is the durable backstop.
