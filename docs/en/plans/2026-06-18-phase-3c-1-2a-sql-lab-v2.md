# Phase 3c-1.2a — SQL Lab v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn SQL Lab into a multi-tab tool with cascading data selectors, schema-aware autocomplete, a SQL formatter, a per-query row-limit, and a proper save dialog — fixing the stale-results bug along the way.

**Architecture:** Mostly frontend. Multiple SQL tabs persisted to **localStorage** (no backend tab table). Two small backend changes: a per-query `limit` on the engine submit endpoint, and a `description` column on `saved_queries`. Results are fetched via a react-query key that includes `queryId` so stale results can never render.

**Tech Stack:** Rust (axum, sqlx runtime queries); React 18 + TypeScript + `@tanstack/react-query` + `react-resizable-panels` + `@monaco-editor/react` + **`sql-formatter`** (new); Vitest.

---

## Conventions

**Backend:** runtime sqlx only; DB-gated tests self-skip without `DATABASE_URL` (`postgres://egret:egret@localhost:5432/egret`); engine-gated tests self-skip without `EGRET_TEST_TRINO_URL` (`http://localhost:8090`, up); plain `git commit`; `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` clean. New migration → `touch crates/egret-metastore/src/lib.rs` to force the embedded migrator to recompile.

**Frontend:** all commands from `frontend/`: `npm test` (= `vitest run`), `npm run typecheck`, `npm run lint`, `npm run build`. API via `apiJson`/`apiFetch` from `@/api/client`. Path alias `@/` → `src/`. Tests: vitest + `@testing-library/react` + `QueryClientProvider` + `MemoryRouter`, `fetch` mocked.

## Reference facts (verified)

- `egret_engine::model::SubmitQuery { database_id: Uuid, sql: String }` (`crates/egret-engine/src/model.rs:68`). `Limits { max_rows: usize, timeout: Duration }`.
- `JobManager::submit(ws, database_id, user: Option<Uuid>, sql: &str) -> Result<Uuid,_>` spawns `run` → `execute`, which uses `self.limits` (a fixed `Limits`). To support a per-query cap, thread an effective `Limits` from `submit` → `run` → `execute`. The submit HTTP handler is in `crates/egret-engine/src/routes.rs` (`async fn submit`, calls `st.jobs.submit(...)`).
- `egret_bi::SavedQueryRepo` (`crates/egret-bi/src/saved.rs`): `struct SavedQuery { id, database_id, name, sql, created_by, updated_at }`, `const COLS = "id, database_id, name, sql, created_by, updated_at"`, methods `create(ws, db_id, name, sql, user)`, `update(id, name, sql, db_id)`. DTOs in `model.rs`: `SaveQuery { database_id, name, sql }`, `SavedQueryDto { id, database_id, name, sql, created_by, updated_at }`. The save route is in `routes.rs` (`async fn save`, `dto(...)`).
- Frontend SQL Lab today (3c-1): `frontend/src/api/sqllab.ts` (hooks incl `useSubmitQuery`, `fetchResults`, `useSavedQueries`, `useSaveQuery`, `useDeleteSavedQuery`, schema hooks, `csvUrl`); `frontend/src/components/sqllab/{SqlEditor,ResultsGrid,SchemaTree}.tsx`; `frontend/src/routes/explore/SqlLab.tsx`. `SchemaTree.tsx` is REPLACED by `DataPanel.tsx`.
- shadcn `Dialog` exists: `@/components/ui/dialog` exports `Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogTrigger` (verify the exact export names in the file). `@/components/ui/{select,button,table,tabs,input,label,textarea,scroll-area}` exist.

## File structure

```text
migrations/0011_sqllab_v2.sql                 # CREATE
crates/egret-bi/src/{model,saved,routes}.rs    # MODIFY: description
crates/egret-engine/src/{model,job,routes}.rs  # MODIFY: per-query limit
frontend/package.json                          # MODIFY: + sql-formatter
frontend/src/api/sqllab.ts                     # MODIFY: limit, description, useResults
frontend/src/hooks/useTabs.ts                  # CREATE
frontend/src/components/sqllab/DataPanel.tsx   # CREATE (replaces SchemaTree)
frontend/src/components/sqllab/SqlTab.tsx      # CREATE
frontend/src/components/sqllab/SaveQueryDialog.tsx  # CREATE
frontend/src/components/sqllab/SqlEditor.tsx   # MODIFY: keywords + schema words
frontend/src/routes/explore/SqlLab.tsx         # REWRITE: tab bar + active SqlTab
frontend/src/components/sqllab/SchemaTree.tsx  # DELETE (+ its test if any)
```

---

## Task 1: Migration `0011` + `saved_queries.description`

**Files:** Create `migrations/0011_sqllab_v2.sql`; Modify `crates/egret-bi/src/{saved.rs,model.rs,routes.rs}`; Modify `crates/egret-bi/tests/saved.rs`.

- [ ] **Step 1: `migrations/0011_sqllab_v2.sql`**
```sql
ALTER TABLE saved_queries ADD COLUMN description text NOT NULL DEFAULT '';
```

- [ ] **Step 2: extend the DB-gated test** — in `crates/egret-bi/tests/saved.rs`, add a test asserting description round-trips:
```rust
#[tokio::test]
async fn description_roundtrips() {
    let Some((r, wid, uid)) = setup().await else { eprintln!("skip"); return; };
    let q = r.create(wid, None, "q", "SELECT 1", "my desc", Some(egret_core::UserId(uid))).await.unwrap();
    assert_eq!(q.description, "my desc");
    let got = r.get(q.id).await.unwrap().unwrap();
    assert_eq!(got.description, "my desc");
    r.delete(q.id).await.unwrap();
}
```
Update the EXISTING `crud_roundtrip`/`owner_returns_creator` tests' `create(...)` calls to pass the new `description` arg (e.g. `""`). Run `DATABASE_URL=… cargo test -p egret-bi --test saved` → compile FAIL.

- [ ] **Step 3: `saved.rs`** — add `description` to the struct, `COLS`, and the `create`/`update` signatures:
```rust
// struct SavedQuery: add field
    pub description: String,
// const:
const COLS: &str = "id, database_id, name, sql, description, created_by, updated_at";
// create: new signature + bind
    pub async fn create(&self, ws: Uuid, db_id: Option<Uuid>, name: &str, sql: &str, description: &str, user: Option<UserId>) -> Result<SavedQuery, BiError> {
        let row = sqlx::query_as::<_, SavedQuery>(&format!(
            "INSERT INTO saved_queries (workspace_id, database_id, name, sql, description, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6) RETURNING {COLS}"))
            .bind(ws).bind(db_id).bind(name).bind(sql).bind(description).bind(user.map(|u| u.0))
            .fetch_one(&self.db.pool).await?;
        Ok(row)
    }
// update: add description
    pub async fn update(&self, id: Uuid, name: &str, sql: &str, description: &str, db_id: Option<Uuid>) -> Result<SavedQuery, BiError> {
        let row = sqlx::query_as::<_, SavedQuery>(&format!(
            "UPDATE saved_queries SET name=$2, sql=$3, description=$4, database_id=$5, updated_at=now() WHERE id=$1 RETURNING {COLS}"))
            .bind(id).bind(name).bind(sql).bind(description).bind(db_id)
            .fetch_optional(&self.db.pool).await?
            .ok_or_else(|| BiError::NotFound(id.to_string()))?;
        Ok(row)
    }
```

- [ ] **Step 4: `model.rs`** — `SaveQuery` gains `#[serde(default)] pub description: String,`; `SavedQueryDto` gains `pub description: String,`.

- [ ] **Step 5: `routes.rs`** — in `dto(...)` add `description: q.description,`; in the `save` handler pass `&b.description` to `create`; in the `update` handler (if present) pass `&b.description`. (Find the existing `save`/`update` calls and thread the new arg.)

- [ ] **Step 6: Run** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi --test saved` → all pass. `cargo build -p egret-bi` + `cargo clippy -p egret-bi --all-targets -- -D warnings` clean.

- [ ] **Step 7: Commit**
```bash
cargo fmt --all
git add migrations/0011_sqllab_v2.sql crates/egret-bi/src crates/egret-bi/tests/saved.rs
git commit -m "feat(bi): saved query description"
```

---

## Task 2: Engine per-query row limit

**Files:** Modify `crates/egret-engine/src/{model.rs,job.rs,routes.rs}`; Create `crates/egret-engine/tests/limit.rs`.

- [ ] **Step 1: failing engine-gated test** `crates/egret-engine/tests/limit.rs`
```rust
//! Engine-gated test that a per-query limit caps the result. Self-skip without env.
//! Uses the JobManager directly with a real Trino connector.
use std::sync::Arc;
use std::time::Duration;
use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::{Connector, ConnectorFactory};
use egret_engine::model::{Limits, QueryStatus};
use egret_engine::{JobManager, QueryStore};
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use uuid::Uuid;

// Real Trino factory (ignores the registry db, builds from EGRET_TEST_TRINO_URL).
struct TrinoFactory { host: String, port: i32 }
#[async_trait::async_trait]
impl ConnectorFactory for TrinoFactory {
    async fn build(&self, _db: &egret_database::model::Database, _s: Option<String>) -> Result<Box<dyn Connector>, egret_engine::EngineError> {
        Ok(egret_engine::connector::build_connector(&egret_engine::connector::ConnSpec {
            engine: Engine::Trino, host: self.host.clone(), port: self.port,
            options: ConnOptions { user: Some("admin".into()), http_scheme: Some("http".into()), ..Default::default() },
            secret: None,
        }))
    }
}
#[derive(Clone, Default)]
struct MemStore(Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>);
#[async_trait::async_trait]
impl ObjectStore for MemStore {
    async fn put(&self, u: &str, b: Vec<u8>) -> Result<(), StorageError> { self.0.lock().unwrap().insert(u.into(), b); Ok(()) }
    async fn get(&self, u: &str) -> Result<Vec<u8>, StorageError> { self.0.lock().unwrap().get(u).cloned().ok_or_else(|| StorageError::NotFound(u.into())) }
    async fn delete(&self, u: &str) -> Result<(), StorageError> { self.0.lock().unwrap().remove(u); Ok(()) }
}

#[tokio::test]
async fn per_query_limit_caps_rows() {
    let (Some(url), Some(trino)) = (std::env::var("DATABASE_URL").ok(), std::env::var("EGRET_TEST_TRINO_URL").ok()) else { eprintln!("skip"); return; };
    let u = url::Url::parse(&trino).unwrap();
    let db = Db::connect(&url, 5).await.expect("connect"); db.migrate().await.unwrap();
    let wid: Uuid = sqlx::query_scalar("INSERT INTO workspaces (slug,name) VALUES ($1,'t') RETURNING id").bind(format!("w-{}", Uuid::new_v4().simple())).fetch_one(&db.pool).await.unwrap();
    let did: Uuid = sqlx::query_scalar("INSERT INTO databases (name,engine,host,port) VALUES ($1,'trino','h',8080) RETURNING id").bind(format!("db_{}", Uuid::new_v4().simple())).fetch_one(&db.pool).await.unwrap();
    let store = QueryStore::new(db.clone());
    let storage: Arc<dyn ObjectStore> = Arc::new(MemStore::default());
    let jobs = JobManager::new(store.clone(),
        egret_database::DatabaseRepo::new(db.clone(), egret_secrets::SecretStore::new(db.clone(), None)),
        Arc::new(TrinoFactory { host: u.host_str().unwrap().into(), port: u.port().unwrap_or(8080) as i32 }),
        storage, "s3://warehouse".into(), "query-results".into(),
        Limits { max_rows: 100_000, timeout: Duration::from_secs(30) }, 100);
    // generate 100 rows but ask for limit=5
    let id = jobs.submit(wid, did, None, "SELECT * FROM UNNEST(SEQUENCE(1,100)) AS t(n)", Some(5)).await.unwrap();
    for _ in 0..40 { if store.get(id).await.unwrap().unwrap().status == QueryStatus::Finished { break; } tokio::time::sleep(Duration::from_millis(50)).await; }
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.status, QueryStatus::Finished);
    assert_eq!(q.row_count, Some(5));
    assert_eq!(q.truncated, true);
}
```
Run `DATABASE_URL=… EGRET_TEST_TRINO_URL=http://localhost:8090 cargo test -p egret-engine --test limit` → compile FAIL (`submit` arity).

- [ ] **Step 2: `model.rs`** — `SubmitQuery` gains the limit field:
```rust
pub struct SubmitQuery {
    pub database_id: Uuid,
    pub sql: String,
    #[serde(default)]
    pub limit: Option<usize>,
}
```

- [ ] **Step 3: `job.rs`** — thread a per-query `Limits` through `submit → run → execute`:
```rust
    pub async fn submit(&self, ws: Uuid, database_id: Uuid, user: Option<Uuid>, sql: &str, limit: Option<usize>) -> Result<Uuid, EngineError> {
        let id = self.store.insert(ws, database_id, user, sql).await?;
        let max_rows = limit.map(|l| l.clamp(1, self.limits.max_rows)).unwrap_or(self.limits.max_rows);
        let limits = Limits { max_rows, timeout: self.limits.timeout };
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id, token.clone());
        let this = self.clone();
        let sql = sql.to_string();
        tokio::spawn(async move {
            this.run(id, database_id, sql, limits, token).await;
            this.cancels.lock().unwrap().remove(&id);
        });
        Ok(id)
    }
```
Change `run` and `execute` to take `limits: Limits` and use it instead of `self.limits` in `connector.execute(sql, &limits, …)` and `tokio::time::timeout(limits.timeout, …)`:
```rust
    async fn run(&self, id: Uuid, database_id: Uuid, sql: String, limits: Limits, token: CancellationToken) { /* … pass limits to execute … */ }
    async fn execute(&self, id: Uuid, database_id: Uuid, sql: &str, limits: &Limits, token: &CancellationToken) -> Result<(), EngineError> {
        // … connector.execute(sql, limits, token.clone()) … tokio::time::timeout(limits.timeout, fut) …
    }
```
Add `use crate::model::Limits;` if not present.

- [ ] **Step 4: `routes.rs`** — the `submit` handler passes `body.limit`:
```rust
    let id = st.jobs.submit(ctx.workspace_id.0, body.database_id, Some(ctx.principal.user_id.0), &body.sql, body.limit)
        .await.map_err(EngineApiError::from)?;
```

- [ ] **Step 5: Run** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret EGRET_TEST_TRINO_URL=http://localhost:8090 cargo test -p egret-engine --test limit` → 1 passed. Full crate still green: `DATABASE_URL=… cargo test -p egret-engine`. `cargo clippy -p egret-engine --all-targets -- -D warnings` clean. (The `job.rs` DB-gated test calls `submit(...)` — update it to pass `None` for the new arg.)

- [ ] **Step 6: Commit**
```bash
cargo fmt --all
git add crates/egret-engine/src crates/egret-engine/tests/limit.rs
git commit -m "feat(engine): per-query row limit"
```

---

## Task 3: Frontend API updates + `useTabs`

**Files:** Modify `frontend/package.json` (add `sql-formatter`), `frontend/src/api/sqllab.ts`; Create `frontend/src/hooks/useTabs.ts`, `frontend/src/hooks/__tests__/useTabs.test.ts`.

- [ ] **Step 1: add `sql-formatter`** — `cd frontend && npm install sql-formatter@^15.4.0`.

- [ ] **Step 2: failing test** `frontend/src/hooks/__tests__/useTabs.test.ts`
```ts
import { renderHook, act } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { useTabs } from "@/hooks/useTabs";

afterEach(() => localStorage.clear());

test("seeds one tab, adds, renames, switches, closes, persists", () => {
  const { result, rerender } = renderHook(() => useTabs("eng"));
  expect(result.current.tabs).toHaveLength(1);
  const first = result.current.tabs[0].id;
  act(() => result.current.addTab());
  expect(result.current.tabs).toHaveLength(2);
  const second = result.current.activeId;
  act(() => result.current.renameTab(second!, "My Tab"));
  expect(result.current.tabs.find((t) => t.id === second)!.title).toBe("My Tab");
  act(() => result.current.patchTab(second!, { sql: "SELECT 2", dbId: "db1", limit: 100 }));
  // persisted to localStorage
  const stored = JSON.parse(localStorage.getItem("sqllab:tabs:eng")!);
  expect(stored.find((t: { id: string }) => t.id === second).sql).toBe("SELECT 2");
  act(() => result.current.closeTab(second!));
  expect(result.current.tabs).toHaveLength(1);
  expect(result.current.activeId).toBe(first);
  // never below one tab
  act(() => result.current.closeTab(first));
  expect(result.current.tabs).toHaveLength(1);
  rerender();
});
```
Run `cd frontend && npx vitest run src/hooks/__tests__/useTabs.test.ts` → FAIL.

- [ ] **Step 3: `frontend/src/hooks/useTabs.ts`**
```ts
import { useCallback, useEffect, useRef, useState } from "react";

export interface SqlTabState { id: string; title: string; sql: string; dbId: string | null; limit: number }

const DEFAULT_LIMIT = 1000;
const newId = () => Math.random().toString(36).slice(2, 10);
const freshTab = (n: number): SqlTabState => ({ id: newId(), title: `Query ${n}`, sql: "SELECT 1", dbId: null, limit: DEFAULT_LIMIT });

function load(ws: string): { tabs: SqlTabState[]; activeId: string } {
  try {
    const tabs = JSON.parse(localStorage.getItem(`sqllab:tabs:${ws}`) || "[]") as SqlTabState[];
    if (Array.isArray(tabs) && tabs.length) {
      const active = localStorage.getItem(`sqllab:active:${ws}`) || tabs[0].id;
      return { tabs, activeId: tabs.some((t) => t.id === active) ? active : tabs[0].id };
    }
  } catch { /* fall through */ }
  const t = freshTab(1);
  return { tabs: [t], activeId: t.id };
}

export function useTabs(ws: string) {
  const [tabs, setTabs] = useState<SqlTabState[]>(() => load(ws).tabs);
  const [activeId, setActiveId] = useState<string>(() => load(ws).activeId);
  const counter = useRef(tabs.length);

  useEffect(() => { localStorage.setItem(`sqllab:tabs:${ws}`, JSON.stringify(tabs)); }, [ws, tabs]);
  useEffect(() => { localStorage.setItem(`sqllab:active:${ws}`, activeId); }, [ws, activeId]);

  const addTab = useCallback(() => {
    counter.current += 1;
    const t = freshTab(counter.current);
    setTabs((cur) => [...cur, t]);
    setActiveId(t.id);
  }, []);
  const closeTab = useCallback((id: string) => {
    setTabs((cur) => {
      if (cur.length <= 1) return cur; // never below one
      const next = cur.filter((t) => t.id !== id);
      setActiveId((a) => (a === id ? next[0].id : a));
      return next;
    });
  }, []);
  const renameTab = useCallback((id: string, title: string) => setTabs((cur) => cur.map((t) => (t.id === id ? { ...t, title } : t))), []);
  const patchTab = useCallback((id: string, partial: Partial<SqlTabState>) => setTabs((cur) => cur.map((t) => (t.id === id ? { ...t, ...partial } : t))), []);
  const openTab = useCallback((init: Partial<SqlTabState> & { title: string }) => {
    const t: SqlTabState = { ...freshTab(0), ...init, id: newId() };
    setTabs((cur) => [...cur, t]);
    setActiveId(t.id);
  }, []);

  return { tabs, activeId, setActive: setActiveId, addTab, closeTab, renameTab, patchTab, openTab };
}
```

- [ ] **Step 4: `frontend/src/api/sqllab.ts`** — three changes:
  1. `SavedQuery` interface gains `description: string`.
  2. `useSubmitQuery` body type gains `limit?: number`:
     ```ts
     export function useSubmitQuery(ws: string) {
       return useMutation({
         mutationFn: (body: { database_id: string; sql: string; limit?: number }) =>
           apiJson<{ id: string; status: QueryStatus }>(`/ws/${ws}/queries`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
       });
     }
     ```
  3. `useSaveQuery` body gains `description`:
     ```ts
     mutationFn: (body: { database_id: string | null; name: string; sql: string; description: string }) =>
       apiJson<SavedQuery>(`/ws/${ws}/sqllab/queries`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
     ```
  4. Add a `useResults` hook (react-query keyed by `queryId`):
     ```ts
     import { useQuery } from "@tanstack/react-query"; // already imported
     export function useResults(ws: string, queryId: string | null, page: number, enabled: boolean) {
       return useQuery<ResultsPage>({
         queryKey: ["sqllab", ws, "results", queryId, page],
         queryFn: () => fetchResults(ws, queryId!, page, 100),
         enabled: !!queryId && enabled,
       });
     }
     ```

- [ ] **Step 5: Run** `cd frontend && npx vitest run src/hooks/__tests__/useTabs.test.ts` → pass. `npm run typecheck` clean.

- [ ] **Step 6: Commit**
```bash
git add frontend/package.json frontend/package-lock.json frontend/src/api/sqllab.ts frontend/src/hooks/useTabs.ts frontend/src/hooks/__tests__/useTabs.test.ts
git commit -m "feat(sqllab): tabs hook + api limit/description/useResults + sql-formatter dep"
```

---

## Task 4: DataPanel — cascading selects + columns + collapse

**Files:** Create `frontend/src/components/sqllab/DataPanel.tsx`.

- [ ] **Step 1: implement `DataPanel.tsx`**
```tsx
import { useState } from "react";
import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useCatalogs, useColumns, useDatabases, useSchemas, useTables } from "@/api/sqllab";

export interface DataPanelProps {
  ws: string;
  dbId: string | null;
  onDb: (id: string) => void;
  onInsert: (name: string) => void;
  /** Report all loaded object names (catalogs/schemas/tables/columns) for autocomplete. */
  onWords: (words: string[]) => void;
}

export function DataPanel({ ws, dbId, onDb, onInsert, onWords }: DataPanelProps) {
  const { data: databases } = useDatabases(ws);
  const engine = databases?.find((d) => d.id === dbId)?.engine ?? null;
  const isTrino = engine === "trino";
  const [collapsed, setCollapsed] = useState(false);
  const [catalog, setCatalog] = useState("");
  const [schema, setSchema] = useState("");
  const [table, setTable] = useState("");

  const cats = useCatalogs(ws, isTrino ? dbId : null);
  const schemas = useSchemas(ws, dbId, isTrino ? catalog : "");
  const tables = useTables(ws, dbId, catalog, schema);
  const columns = useColumns(ws, dbId, catalog, schema, table, !!table);

  // feed autocomplete
  const words = [
    ...(cats.data ?? []), ...(schemas.data ?? []), ...(tables.data ?? []),
    ...((columns.data ?? []).map((c) => c.name)),
  ];
  // report once per change (cheap; provider reads latest)
  const wkey = words.join("|");
  if (wkey !== (DataPanel as unknown as { _w?: string })._w) {
    (DataPanel as unknown as { _w?: string })._w = wkey;
    onWords(words);
  }

  if (collapsed) {
    return <div className="border-r p-1"><Button variant="ghost" size="sm" onClick={() => setCollapsed(false)} title="Show data panel"><PanelLeftOpen className="h-4 w-4" /></Button></div>;
  }

  function pick<T>(set: (v: string) => void, resets: ((v: string) => void)[]) {
    return (v: string) => { set(v); resets.forEach((r) => r("")); };
  }
  const qualified = (t: string) => (catalog ? `${catalog}.${schema}.${t}` : `${schema}.${t}`);

  return (
    <div className="flex h-full w-full flex-col border-r">
      <div className="flex items-center justify-between border-b p-2">
        <span className="text-xs font-medium">Data</span>
        <Button variant="ghost" size="sm" onClick={() => setCollapsed(true)} title="Collapse"><PanelLeftClose className="h-4 w-4" /></Button>
      </div>
      <div className="space-y-2 p-2 text-sm">
        <Select value={dbId ?? ""} onValueChange={(v) => { onDb(v); setCatalog(""); setSchema(""); setTable(""); }}>
          <SelectTrigger><SelectValue placeholder="Database" /></SelectTrigger>
          <SelectContent>{(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}</SelectContent>
        </Select>
        {isTrino && (
          <Select value={catalog} onValueChange={pick(setCatalog, [setSchema, setTable])}>
            <SelectTrigger><SelectValue placeholder="Catalog" /></SelectTrigger>
            <SelectContent>{(cats.data ?? []).map((c) => <SelectItem key={c} value={c}>{c}</SelectItem>)}</SelectContent>
          </Select>
        )}
        <Select value={schema} onValueChange={pick(setSchema, [setTable])} disabled={isTrino && !catalog}>
          <SelectTrigger><SelectValue placeholder="Schema" /></SelectTrigger>
          <SelectContent>{(schemas.data ?? []).map((s) => <SelectItem key={s} value={s}>{s}</SelectItem>)}</SelectContent>
        </Select>
        <Select value={table} onValueChange={setTable} disabled={!schema}>
          <SelectTrigger><SelectValue placeholder="Table" /></SelectTrigger>
          <SelectContent>{(tables.data ?? []).map((t) => <SelectItem key={t} value={t}>{t}</SelectItem>)}</SelectContent>
        </Select>
        {table && (
          <button className="text-xs text-muted-foreground underline" onClick={() => onInsert(qualified(table))}>insert {table}</button>
        )}
      </div>
      <div className="flex-1 overflow-auto border-t p-2 text-sm">
        {table ? (columns.data ?? []).map((c) => (
          <button key={c.name} className="flex w-full justify-between rounded px-2 py-0.5 text-left hover:bg-accent" onClick={() => onInsert(c.name)}>
            <span className="truncate">{c.name}</span><span className="ml-2 text-muted-foreground">{c.type}</span>
          </button>
        )) : <span className="text-muted-foreground">Pick a table to see columns.</span>}
      </div>
    </div>
  );
}
```
> **Implementer note:** the `(DataPanel as …)._w` memo-via-static is a hack to avoid re-calling `onWords` every render. If lint/types complain, replace with a `useEffect(() => onWords(words), [wkey])` (compute `wkey` and depend on it). Prefer the `useEffect` form — cleaner. Verify `lucide-react` exports `PanelLeftClose`/`PanelLeftOpen` (it does); if not, use `ChevronLeft`/`ChevronRight`.

- [ ] **Step 2: Verify** `cd frontend && npm run typecheck` (DataPanel not yet imported — it'll be in Task 6; typecheck still passes as an unused module). `npm run lint` clean.
- [ ] **Step 3: Commit**
```bash
git add frontend/src/components/sqllab/DataPanel.tsx
git commit -m "feat(sqllab): DataPanel cascading selects + columns + collapse"
```

---

## Task 5: SqlEditor autocomplete (keywords + schema words)

**Files:** Modify `frontend/src/components/sqllab/SqlEditor.tsx`.

- [ ] **Step 1: rewrite `SqlEditor.tsx`** to take `schemaWords: string[]` and merge with a keyword list:
```tsx
import Editor, { type Monaco } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";
import { useEffect, useRef } from "react";

const KEYWORDS = [
  "SELECT","FROM","WHERE","GROUP BY","ORDER BY","HAVING","LIMIT","JOIN","LEFT JOIN","RIGHT JOIN",
  "INNER JOIN","ON","AS","AND","OR","NOT","IN","IS NULL","IS NOT NULL","DISTINCT","COUNT","SUM",
  "AVG","MIN","MAX","CASE","WHEN","THEN","ELSE","END","UNION","UNION ALL","WITH","INSERT","UPDATE","DELETE","CREATE","TABLE",
];

export function SqlEditor({ value, onChange, schemaWords }: { value: string; onChange: (v: string) => void; schemaWords: string[] }) {
  const wordsRef = useRef(schemaWords);
  wordsRef.current = schemaWords;
  const disposableRef = useRef<{ dispose: () => void } | null>(null);
  useEffect(() => () => disposableRef.current?.dispose(), []);

  function handleMount(_editor: unknown, monaco: Monaco) {
    disposableRef.current = monaco.languages.registerCompletionItemProvider("sql", {
      provideCompletionItems: (model: MonacoNS.editor.ITextModel, position: MonacoNS.Position) => {
        const word = model.getWordUntilPosition(position);
        const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
        const kw = KEYWORDS.map((k) => ({ label: k, kind: monaco.languages.CompletionItemKind.Keyword, insertText: k, range }));
        const objs = Array.from(new Set(wordsRef.current)).map((w) => ({ label: w, kind: monaco.languages.CompletionItemKind.Field, insertText: w, range }));
        return { suggestions: [...objs, ...kw] };
      },
    });
  }

  return <Editor height="100%" defaultLanguage="sql" value={value} onChange={(v) => onChange(v ?? "")} onMount={handleMount}
    options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }} />;
}
```
> **Implementer note:** the old `SchemaHint` prop is gone — `schemaWords: string[]` replaces it. Task 6 supplies it.

- [ ] **Step 2: Verify** `cd frontend && npm run typecheck` will FAIL where `SqlEditor` is still used with the old `hint` prop — that usage is rewritten in Task 6. To keep this task green in isolation, do Step 1 and proceed to Task 6 in the same working session (they're coupled). If you need this task to typecheck alone, temporarily leave the old `SqlLab.tsx` importing the old prop — but the recommended flow is to do Tasks 5 + 6 back-to-back and commit Task 6's verification. **Mark this task's commit together with Task 6** (no standalone commit), OR stub the SqlLab usage. Simplest: fold Tasks 5 + 6 into one commit at the end of Task 6.

(No separate commit — committed with Task 6.)

---

## Task 6: SqlTab + tab bar + SaveQueryDialog + formatter

**Files:** Create `frontend/src/components/sqllab/{SqlTab.tsx,SaveQueryDialog.tsx}`; Rewrite `frontend/src/routes/explore/SqlLab.tsx`; Delete `frontend/src/components/sqllab/SchemaTree.tsx` (+ any test referencing it).

- [ ] **Step 1: `SaveQueryDialog.tsx`**
```tsx
import { useState } from "react";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

export function SaveQueryDialog({ open, onOpenChange, onSave }: {
  open: boolean; onOpenChange: (o: boolean) => void; onSave: (name: string, description: string) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit() {
    if (!name.trim()) return;
    setBusy(true);
    try { await onSave(name.trim(), description.trim()); onOpenChange(false); setName(""); setDescription(""); }
    finally { setBusy(false); }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>Save query</DialogTitle></DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1"><Label htmlFor="sq-name">Name</Label><Input id="sq-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Monthly revenue" /></div>
          <div className="space-y-1"><Label htmlFor="sq-desc">Description</Label><Textarea id="sq-desc" value={description} onChange={(e) => setDescription(e.target.value)} rows={3} /></div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={submit} disabled={busy || !name.trim()}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
```
> **Implementer note:** confirm `@/components/ui/dialog` exports `DialogContent/DialogHeader/DialogTitle/DialogFooter` and that `@/components/ui/{input,label,textarea}` exist (input + label do; verify `textarea` — if missing, use a plain `<textarea className="…">`).

- [ ] **Step 2: `SqlTab.tsx`** (one tab's content: DataPanel + editor + toolbar + results, bound to `queryId`)
```tsx
import { useState } from "react";
import { format as formatSql } from "sql-formatter";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  csvUrl, useCancelQuery, useDatabases, useDeleteSavedQuery, useHistory, useQueryStatus, useResults,
  useSaveQuery, useSavedQueries, useSubmitQuery,
} from "@/api/sqllab";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";
import { DataPanel } from "@/components/sqllab/DataPanel";
import { SaveQueryDialog } from "@/components/sqllab/SaveQueryDialog";
import type { SqlTabState } from "@/hooks/useTabs";

const LIMITS = [10, 100, 1000, 10000, 100000];

export function SqlTab({ ws, tab, patch, onOpenSaved }: {
  ws: string;
  tab: SqlTabState;
  patch: (p: Partial<SqlTabState>) => void;
  onOpenSaved: (sql: string, dbId: string | null) => void;
}) {
  const { data: databases } = useDatabases(ws);
  const engine = databases?.find((d) => d.id === tab.dbId)?.engine ?? null;
  const [queryId, setQueryId] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [words, setWords] = useState<string[]>([]);
  const [saveOpen, setSaveOpen] = useState(false);

  const submit = useSubmitQuery(ws);
  const cancel = useCancelQuery(ws);
  const status = useQueryStatus(ws, queryId);
  const save = useSaveQuery(ws);
  const finished = status.data?.status === "finished";
  const running = status.data?.status === "queued" || status.data?.status === "running";
  const results = useResults(ws, queryId, page, finished);

  async function run() {
    if (!tab.dbId) { toast.error("Pick a database"); return; }
    setQueryId(null); setPage(0);
    try { const { id } = await submit.mutateAsync({ database_id: tab.dbId, sql: tab.sql, limit: tab.limit }); setQueryId(id); }
    catch (e) { toast.error(String(e)); }
  }
  function doFormat() {
    const language = engine === "starrocks" ? "mysql" : "trino";
    try { patch({ sql: formatSql(tab.sql, { language }) }); } catch { /* keep as-is */ }
  }
  function insert(name: string) { patch({ sql: tab.sql + name }); }

  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={22} minSize={0} collapsible>
        <DataPanel ws={ws} dbId={tab.dbId} onDb={(id) => patch({ dbId: id })} onInsert={insert} onWords={setWords} />
      </Panel>
      <PanelResizeHandle className="w-1 bg-border" />
      <Panel defaultSize={78}>
        <PanelGroup direction="vertical">
          <Panel defaultSize={50} minSize={20}>
            <div className="flex h-full flex-col">
              <div className="flex items-center gap-2 border-b p-2">
                <Button size="sm" onClick={run} disabled={running}>Run</Button>
                <Button size="sm" variant="outline" disabled={!running} onClick={() => queryId && cancel.mutate(queryId)}>Cancel</Button>
                <Button size="sm" variant="outline" onClick={doFormat}>Format</Button>
                <Select value={String(tab.limit)} onValueChange={(v) => patch({ limit: Number(v) })}>
                  <SelectTrigger className="h-8 w-28"><SelectValue /></SelectTrigger>
                  <SelectContent>{LIMITS.map((l) => <SelectItem key={l} value={String(l)}>{l.toLocaleString()} rows</SelectItem>)}</SelectContent>
                </Select>
                <Button size="sm" variant="outline" onClick={() => setSaveOpen(true)}>Save</Button>
                {running && <span className="text-sm text-muted-foreground">running…</span>}
                {status.data?.status === "failed" && <span className="text-sm text-red-600">{status.data.error}</span>}
              </div>
              <div className="flex-1"><SqlEditor value={tab.sql} onChange={(v) => patch({ sql: v })} schemaWords={words} /></div>
            </div>
          </Panel>
          <PanelResizeHandle className="h-1 bg-border" />
          <Panel defaultSize={50} minSize={20}>
            <Tabs defaultValue="results" className="flex h-full flex-col">
              <TabsList className="m-2 w-fit"><TabsTrigger value="results">Results</TabsTrigger><TabsTrigger value="history">History</TabsTrigger><TabsTrigger value="saved">Saved</TabsTrigger></TabsList>
              <TabsContent value="results" className="flex-1 overflow-hidden">
                <ResultsGrid page={results.data ?? null} pageIndex={page} onPage={setPage} truncated={status.data?.truncated ?? false}
                  csvHref={finished && queryId ? csvUrl(ws, queryId) : null} />
              </TabsContent>
              <TabsContent value="history" className="flex-1 overflow-auto"><HistoryTab ws={ws} onLoad={(s) => patch({ sql: s })} /></TabsContent>
              <TabsContent value="saved" className="flex-1 overflow-auto"><SavedTab ws={ws} onOpen={onOpenSaved} /></TabsContent>
            </Tabs>
          </Panel>
        </PanelGroup>
      </Panel>
      <SaveQueryDialog open={saveOpen} onOpenChange={setSaveOpen}
        onSave={async (name, description) => { await save.mutateAsync({ database_id: tab.dbId, name, sql: tab.sql, description }); toast.success("Saved"); }} />
    </PanelGroup>
  );
}

function HistoryTab({ ws, onLoad }: { ws: string; onLoad: (sql: string) => void }) {
  const { data } = useHistory(ws);
  return <div className="p-2 text-sm">{(data ?? []).map((h) => (
    <button key={h.id} className="block w-full truncate rounded px-2 py-1 text-left hover:bg-accent" onClick={() => onLoad(h.sql)} title={h.sql}>
      <span className="text-muted-foreground">[{h.status}]</span> {h.sql}
    </button>
  ))}</div>;
}
function SavedTab({ ws, onOpen }: { ws: string; onOpen: (sql: string, dbId: string | null) => void }) {
  const { data } = useSavedQueries(ws);
  const del = useDeleteSavedQuery(ws);
  return <div className="p-2 text-sm">{(data ?? []).map((q) => (
    <div key={q.id} className="flex items-center gap-2 rounded px-2 py-1 hover:bg-accent">
      <button className="flex-1 truncate text-left" onClick={() => onOpen(q.sql, q.database_id)} title={q.description || q.sql}>{q.name}</button>
      <button className="text-xs text-red-600" onClick={() => del.mutate(q.id)}>delete</button>
    </div>
  ))}</div>;
}
```

- [ ] **Step 2b: Rewrite `frontend/src/routes/explore/SqlLab.tsx`** (tab bar + active SqlTab)
```tsx
import { useState } from "react";
import { useParams } from "react-router-dom";
import { Plus, X } from "lucide-react";
import { useTabs } from "@/hooks/useTabs";
import { SqlTab } from "@/components/sqllab/SqlTab";

export default function SqlLab() {
  const ws = useParams().workspace!;
  const { tabs, activeId, setActive, addTab, closeTab, renameTab, patchTab, openTab } = useTabs(ws);
  const [editing, setEditing] = useState<string | null>(null);
  const active = tabs.find((t) => t.id === activeId) ?? tabs[0];

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-1 border-b bg-muted/30 px-1">
        {tabs.map((t) => (
          <div key={t.id} className={`flex items-center gap-1 rounded-t px-2 py-1 text-sm ${t.id === activeId ? "bg-background" : "hover:bg-accent"}`}>
            {editing === t.id ? (
              <input autoFocus defaultValue={t.title} className="w-24 bg-transparent outline-none"
                onBlur={(e) => { renameTab(t.id, e.target.value || t.title); setEditing(null); }}
                onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} />
            ) : (
              <button onClick={() => setActive(t.id)} onDoubleClick={() => setEditing(t.id)} className="max-w-[12rem] truncate">{t.title}</button>
            )}
            <button onClick={() => closeTab(t.id)} className="text-muted-foreground hover:text-foreground"><X className="h-3 w-3" /></button>
          </div>
        ))}
        <button onClick={addTab} className="px-2 py-1 text-muted-foreground hover:text-foreground" title="New tab"><Plus className="h-4 w-4" /></button>
      </div>
      <div className="flex-1 overflow-hidden">
        {active && (
          <SqlTab key={active.id} ws={ws} tab={active}
            patch={(p) => patchTab(active.id, p)}
            onOpenSaved={(sql, dbId) => openTab({ title: "Saved", sql, dbId, limit: active.limit })} />
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Delete `SchemaTree.tsx`** — `git rm frontend/src/components/sqllab/SchemaTree.tsx`. If a test imports it, delete that too.

- [ ] **Step 4: Verify** `cd frontend && npm run typecheck` clean; `npm run lint` clean; `npm run build` succeeds.
> **Implementer note:** the `SqlTab key={active.id}` remounts on tab switch — so the runtime `queryId`/results reset when switching tabs (a running query in a backgrounded tab stops being polled). That's the accepted MVP behavior per the spec (per-tab runtime state isn't preserved across switches in this increment; the *editor* state IS, via localStorage). If you want to preserve a running query across switches, lift `queryId` into the parent keyed by tab id — but keep it simple unless tests require otherwise.

- [ ] **Step 5: Commit** (Tasks 5 + 6 together)
```bash
git add frontend/src/components/sqllab frontend/src/routes/explore/SqlLab.tsx
git commit -m "feat(sqllab): multi-tab + DataPanel + formatter + LIMIT + save dialog"
```

---

## Task 7: Component tests + final verification

**Files:** Create `frontend/src/components/sqllab/__tests__/SqlLabTabs.test.tsx`; update any stale test.

- [ ] **Step 1: tab + run-flow test** `SqlLabTabs.test.tsx`
```tsx
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

vi.mock("@monaco-editor/react", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (v: string) => void }) =>
    <textarea data-testid="sql" value={value} onChange={(e) => onChange?.(e.target.value)} />,
}));
import SqlLab from "@/routes/explore/SqlLab";

beforeEach(() => localStorage.clear());
afterEach(() => vi.restoreAllMocks());

function mockApi() {
  const calls: Record<string, number> = {};
  vi.stubGlobal("fetch", vi.fn(async (url: string | URL, init?: RequestInit) => {
    const u = String(url); const m = (init?.method ?? "GET").toUpperCase();
    const json = (b: unknown) => new Response(JSON.stringify(b), { status: 200, headers: { "content-type": "application/json" } });
    if (u.includes("/me/databases")) return json([{ id: "db1", name: "trino", engine: "trino" }]);
    if (u.includes("/sqllab/databases/")) return json([]); // schema selects
    if (u.endsWith("/sqllab/queries")) return json([]);
    if (u.endsWith("/queries") && m === "GET") return json([]);
    if (u.endsWith("/queries") && m === "POST") { calls.submit = (calls.submit ?? 0) + 1; const id = `q${calls.submit}`; return json({ id, status: "queued" }); }
    if (u.match(/\/queries\/q\d+\/results/)) { const id = u.match(/q(\d+)/)![0]; return json({ columns: [{ name: id, type: "bigint" }], rows: [[1]], page: 0, page_size: 100, row_count: 1, truncated: false }); }
    if (u.match(/\/queries\/q\d+/)) { const id = u.match(/q\d+/)![0]; return json({ id, database_id: "db1", status: "finished", columns: [{ name: id, type: "bigint" }], preview: [[1]], row_count: 1, truncated: false, error: null }); }
    return json({});
  }));
}

function renderApp() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}><MemoryRouter initialEntries={["/explore/eng/sql-lab"]}>
    <Routes><Route path="/explore/:workspace/sql-lab" element={<SqlLab />} /></Routes>
  </MemoryRouter></QueryClientProvider>);
}

test("opens one tab, can add a second", async () => {
  mockApi(); renderApp();
  expect(await screen.findByText("Query 1")).toBeInTheDocument();
  await userEvent.click(screen.getByTitle("New tab"));
  expect(await screen.findByText("Query 2")).toBeInTheDocument();
});

test("run query A then B shows B's results, never A's (no stale)", async () => {
  mockApi(); renderApp();
  // pick db (auto via DataPanel select) — set sql + run; the DataPanel DB select:
  await userEvent.click(await screen.findByText("Database"));
  await userEvent.click(await screen.findByText(/trino \(trino\)/));
  await userEvent.click(screen.getByText("Run")); // q1
  await waitFor(() => expect(screen.getByText("q1")).toBeInTheDocument());
  await userEvent.click(screen.getByText("Run")); // q2
  await waitFor(() => expect(screen.getByText("q2")).toBeInTheDocument());
  expect(screen.queryByText("q1")).not.toBeInTheDocument(); // the old result is gone
});
```
> **Implementer note:** the Radix `Select` under jsdom can be finicky (pointer events / scrollIntoView). If picking the DB is flaky, apply the same pragmatic seam used in 3c-1: auto-select when exactly one DB is granted (add a `useEffect` in `DataPanel` that calls `onDb(databases[0].id)` when `!dbId && databases?.length === 1`) — this also improves real UX. Then the test just clicks Run. Keep the core assertion: after running q2, `q1`'s column header is gone and `q2`'s is shown (the stale-results regression).

- [ ] **Step 2: Run frontend** `cd frontend && npm test` → all pass; `npm run typecheck` + `npm run lint` + `npm run build` clean.

- [ ] **Step 3: Backend final verification**
```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret EGRET_TEST_TRINO_URL=http://localhost:8090 EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030 cargo test --workspace
```
Expected: 0 failures.

- [ ] **Step 4: Commit**
```bash
git add frontend/src/components/sqllab/__tests__
git commit -m "test(sqllab): multi-tab + stale-results regression"
```

- [ ] **Step 5: Final whole-feature review**, then **superpowers:finishing-a-development-branch**.

---

## Self-review notes (author)

- **Spec coverage:** §4 migration → T1; §5.1 description → T1; §5.2 limit → T2; §6.1 tabs/localStorage → T3 (`useTabs`); §6.2 tab bar → T6 (SqlLab rewrite); §6.3 SqlTab + results-by-queryId → T3 (`useResults`) + T6 (SqlTab); §6.4 DataPanel cascading + columns + collapse → T4; §6.5 toolbar format + limit → T6; §6.6 autocomplete → T5; §6.7 save dialog + saved/history → T6; §9 testing → T1/T2 backend + T7 frontend.
- **No placeholders:** every step has complete code; the four implementer notes flag spots to verify against live APIs/jsdom (the `onWords` effect form, the dialog/textarea exports, lucide icon names, the Radix-Select test seam) — each with a concrete fallback.
- **Type consistency:** `SqlTabState`, `useTabs` API (`addTab/closeTab/renameTab/patchTab/openTab/setActive`), `useResults(ws,queryId,page,enabled)`, `useSubmitQuery({…,limit})`, `useSaveQuery({…,description})`, `SavedQuery.description`, `SqlEditor({value,onChange,schemaWords})`, `DataPanel({ws,dbId,onDb,onInsert,onWords})`, `SqlTab({ws,tab,patch,onOpenSaved})` are used identically across tasks. Engine `submit(ws,db,user,sql,limit)` matches its test + the routes call.
- **Deferred (per spec §1/§12):** Explorer Workbench (backend tabs, Home, generic shell), streaming export (3c-1.2b). Not in any task.
