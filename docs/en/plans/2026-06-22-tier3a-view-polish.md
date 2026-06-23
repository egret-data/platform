# Tier 3a — Dashboard View Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Metabase-style dashboard view polish to egret — duplicate, trash/soft-delete, bookmark, an info sidebar, and a dark-mode audit.

**Architecture:** Mixed backend (Rust `egret-bi`) + frontend (React). Two new migrations (`0019` soft-delete column, `0020` bookmarks table). Trash + bookmark are wired dashboard-only but the `bookmarks` table is polymorphic (`entity_type`/`entity_id`) for later reuse. Backend reuses `DashboardRepo` (`crates/egret-bi/src/dashboard.rs`), handlers + guards in `routes.rs`, DTOs in `model.rs`. Frontend reuses `api/dashboards.ts`, `routes/explore/Dashboards.tsx` (list), `routes/explore/DashboardView.tsx` (header).

**Tech Stack:** Rust + sqlx + axum (backend); React 18 + TS + Vite + Vitest + TanStack Query (frontend). Postgres.

**Conventions for every task:**
- TDD: failing test first → run-fail → implement → run-pass.
- **Backend sweep** before each backend commit (from repo root): `cargo test -p egret-bi && cargo fmt --all --check`. The repo tests are DB-gated and self-skip without `DATABASE_URL`; run with the dev DB URL `postgres://egret:egret@localhost:5432/egret` to actually exercise them.
- **Frontend sweep** before each frontend commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- **sqlx migration staleness:** after adding a file under `migrations/`, run `touch crates/egret-metastore/src/lib.rs` before `cargo build`/`test`/`serve` so the embedded migrator re-includes it.
- Stage explicit paths only (no `git add -A`). Configured git identity. End every commit message with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Branch: `feat/dashboards-tier3a` (spec already committed there).

**Backend facts (verified):** `Dashboard` struct fields `{id, workspace_id, name, description, parameters, created_by, updated_at, width}`; `const COLS` lists them; `DashboardRepo` has `create/get/list_by_workspace/replace_layout/owner/dashboard_workspace/delete`. Handlers `db_list/db_create/db_get/db_update/db_remove` + guards `require_dashboard_in_ws` (member/read), `require_dashboard_owner` (owner-or-admin). Current user = `ctx.principal.user_id.0`; workspace = `ctx.workspace_id.0`. `dashboard_router()` is in `routes.rs`. DTO builders `dashboard_summary_dto(d, n)` and `dashboard_detail_dto(d, tabs, cards)` in `model.rs`. Repo test fixture in `crates/egret-bi/tests/dashboard_repo.rs` (`setup() -> Option<Fixture>` with `repo/charts/ws/dataset/user`, self-skips without `DATABASE_URL`).

---

## Task ordering

A) **Task 1–2** Duplicate (BE → FE).
B) **Task 3–4** Migration 0019 + Trash (BE → FE).
C) **Task 5–6** Migration 0020 + Bookmark (BE → FE).
D) **Task 7** Info sidebar (FE).
E) **Task 8** Dark-mode audit (FE).
F) **Task 9** Live verify + PR.

---

### Task 1: Duplicate — backend repo + route

**Files:**
- Modify: `crates/egret-bi/src/dashboard.rs` (add `DashboardRepo::duplicate`)
- Modify: `crates/egret-bi/src/routes.rs` (`db_duplicate` handler + route)
- Test: `crates/egret-bi/tests/dashboard_repo.rs` (add `duplicate_copies_tabs_and_cards`)

- [ ] **Step 1: Write the failing test**

Append to `crates/egret-bi/tests/dashboard_repo.rs`:

```rust
#[tokio::test]
async fn duplicate_copies_tabs_and_cards_with_new_owner() {
    let Some(fx) = setup().await else { return };
    let src = fx.repo.create(fx.ws, "Orig", "desc", Some(fx.user)).await.unwrap();
    let tabs = vec![NewTab { name: "T1".into(), position: 0 }, NewTab { name: "T2".into(), position: 1 }];
    let cards = vec![
        NewCard { chart_id: None, card_type: "text".into(), grid_col: 0, grid_row: 0, size_x: 4, size_y: 2,
                  options: serde_json::json!({"text":"hi"}), tab_index: Some(0) },
        NewCard { chart_id: None, card_type: "heading".into(), grid_col: 0, grid_row: 2, size_x: 4, size_y: 1,
                  options: serde_json::json!({"text":"h"}), tab_index: Some(1) },
    ];
    fx.repo.replace_layout(src.id, &tabs, &cards).await.unwrap();

    let other = Uuid::new_v4(); // any non-null owner id is fine for the copy
    let copy = fx.repo.duplicate(src.id, Some(other)).await.unwrap().expect("copy");

    assert_ne!(copy.id, src.id);
    assert_eq!(copy.name, "Orig (copy)");
    assert_eq!(copy.created_by, Some(other));
    assert_eq!(fx.repo.tabs(copy.id).await.unwrap().len(), 2);
    let copy_cards = fx.repo.cards(copy.id).await.unwrap();
    assert_eq!(copy_cards.len(), 2);
    // tab_id remapped to the COPY's tabs, not the source's
    let copy_tab_ids: std::collections::HashSet<Uuid> =
        fx.repo.tabs(copy.id).await.unwrap().into_iter().map(|t| t.id).collect();
    for c in &copy_cards {
        assert!(c.tab_id.map(|t| copy_tab_ids.contains(&t)).unwrap_or(true));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi duplicate_copies_tabs_and_cards_with_new_owner`
Expected: FAIL — `no method named duplicate found for struct DashboardRepo`.

- [ ] **Step 3: Implement `DashboardRepo::duplicate`**

In `crates/egret-bi/src/dashboard.rs`, add this method inside `impl DashboardRepo` (after `owner`):

```rust
    /// Deep-copy a dashboard (row + tabs + cards) under a new owner, in one transaction.
    /// The copy's name is "<name> (copy)"; tab references on cards are remapped to the copy's tabs.
    /// Returns None if the source doesn't exist.
    pub async fn duplicate(
        &self,
        src_id: Uuid,
        new_owner: Option<Uuid>,
    ) -> Result<Option<Dashboard>, BiError> {
        let Some(src) = self.get(src_id).await? else { return Ok(None) };
        let src_tabs = self.tabs(src_id).await?;
        let src_cards = self.cards(src_id).await?;

        let mut tx = self.db.pool.begin().await?;
        let copy: Dashboard = sqlx::query_as::<_, Dashboard>(&format!(
            "INSERT INTO dashboards (workspace_id, name, description, parameters, created_by, width) \
             VALUES ($1,$2,$3,$4,$5,$6) RETURNING {COLS}"
        ))
        .bind(src.workspace_id)
        .bind(format!("{} (copy)", src.name))
        .bind(&src.description)
        .bind(&src.parameters)
        .bind(new_owner)
        .bind(&src.width)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_conflict_err)?;

        // old tab id -> new tab id
        let mut tab_map: std::collections::HashMap<Uuid, Uuid> = std::collections::HashMap::new();
        for t in &src_tabs {
            let nid: Uuid = sqlx::query_scalar(
                "INSERT INTO dashboard_tabs (dashboard_id, name, position) VALUES ($1,$2,$3) RETURNING id",
            )
            .bind(copy.id).bind(&t.name).bind(t.position)
            .fetch_one(&mut *tx).await?;
            tab_map.insert(t.id, nid);
        }
        for c in &src_cards {
            let new_tab = c.tab_id.and_then(|old| tab_map.get(&old).copied());
            sqlx::query(
                "INSERT INTO dashboard_cards \
                 (dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal, tab_id) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(copy.id).bind(c.chart_id).bind(&c.card_type)
            .bind(c.grid_col).bind(c.grid_row).bind(c.size_x).bind(c.size_y)
            .bind(&c.options).bind(c.ordinal).bind(new_tab)
            .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(Some(copy))
    }
```

- [ ] **Step 4: Add the route + handler**

In `crates/egret-bi/src/routes.rs`, add the handler (next to `db_remove`):

```rust
async fn db_duplicate(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    st.authz.require_workspace(&ctx, DB_READ).await.map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    let copy = st
        .dashboards
        .duplicate(id, Some(ctx.principal.user_id.0))
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dashboard".into()))?;
    Ok(Json(load_dashboard(&st, copy.id).await?))
}
```

Wire the route in `dashboard_router()`:

```rust
        .route("/ws/{workspace}/dashboards/{id}/duplicate", post(db_duplicate))
```

- [ ] **Step 5: Run test to verify it passes**

Run: `touch crates/egret-bi/src/../egret-metastore/src/lib.rs; DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi duplicate_copies_tabs_and_cards_with_new_owner`
Expected: PASS. (No migration was added in this task, so the `touch` is harmless.)

- [ ] **Step 6: Backend sweep + commit**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi && cargo fmt --all --check
git add crates/egret-bi/src/dashboard.rs crates/egret-bi/src/routes.rs crates/egret-bi/tests/dashboard_repo.rs
git commit -m "feat(dashboards): backend duplicate endpoint deep-copies tabs + cards

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Duplicate — frontend hook + UI

**Files:**
- Modify: `frontend/src/api/dashboards.ts` (add `duplicate` to `useDashboardMutations`)
- Modify: `frontend/src/routes/explore/Dashboards.tsx` (Duplicate action in the list)
- Test: `frontend/src/api/__tests__/dashboards.duplicate.test.tsx` (create)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/api/__tests__/dashboards.duplicate.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";

const apiJson = vi.fn().mockResolvedValue({ id: "new1", name: "X (copy)" });
vi.mock("../client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: vi.fn() }));

import { useDashboardMutations } from "../dashboards";

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, children);

describe("useDashboardMutations.duplicate", () => {
  beforeEach(() => apiJson.mockClear());
  it("POSTs to the duplicate endpoint and returns the copy", async () => {
    const { result } = renderHook(() => useDashboardMutations("eng"), { wrapper });
    let copy: { id: string } | undefined;
    await act(async () => { copy = await result.current.duplicate.mutateAsync("src1"); });
    expect(apiJson).toHaveBeenCalledWith("/ws/eng/dashboards/src1/duplicate", expect.objectContaining({ method: "POST" }));
    expect(copy?.id).toBe("new1");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run (from `frontend/`): `npx vitest run src/api/__tests__/dashboards.duplicate.test.tsx`
Expected: FAIL — `result.current.duplicate` is undefined.

- [ ] **Step 3: Add the `duplicate` mutation**

In `frontend/src/api/dashboards.ts`, inside `useDashboardMutations`, add before `return`:

```ts
  const duplicate = useMutation({
    mutationFn: (id: string) =>
      apiJson<DashboardDetail>(`/ws/${ws}/dashboards/${id}/duplicate`, { method: "POST" }),
    onSuccess: () => invalidate(),
  });
```

and extend the return: `return { create, update, remove, duplicate };`

- [ ] **Step 4: Run test to verify it passes**

Run: `npx vitest run src/api/__tests__/dashboards.duplicate.test.tsx`
Expected: PASS.

- [ ] **Step 5: Add the Duplicate action to the list**

In `frontend/src/routes/explore/Dashboards.tsx`: destructure `duplicate` from `useDashboardMutations`, and add a "Duplicate" button in the actions cell next to Delete (it navigates to the copy):

```tsx
  const { create, remove, duplicate } = useDashboardMutations(ws);
```

In the row's last `<td>`, before the Delete button:

```tsx
                  <Button size="sm" variant="ghost" aria-label={`duplicate ${d.name}`}
                    onClick={async () => {
                      try { const copy = await duplicate.mutateAsync(d.id); nav(`/explore/${ws}/dashboards/${copy.id}`); }
                      catch (e) { toast.error(String(e)); }
                    }}>Duplicate</Button>
```

- [ ] **Step 6: Frontend sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/api/dashboards.ts src/routes/explore/Dashboards.tsx src/api/__tests__/dashboards.duplicate.test.tsx
git commit -m "feat(dashboards): duplicate action in dashboard list

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Migration 0019 + Trash — backend

**Files:**
- Create: `migrations/0019_dashboard_soft_delete.sql`
- Modify: `crates/egret-bi/src/dashboard.rs` (`Dashboard.deleted_at` + `COLS`; `list_by_workspace(trashed)`; `set_trashed`)
- Modify: `crates/egret-bi/src/model.rs` (`DashboardDetailDto.deleted_at` + builder)
- Modify: `crates/egret-bi/src/routes.rs` (`db_list` reads `?trashed`; `db_trash`/`db_restore` + routes)
- Test: `crates/egret-bi/tests/dashboard_repo.rs` (`trash_hides_from_live_list_and_restore`)

- [ ] **Step 1: Write the migration**

Create `migrations/0019_dashboard_soft_delete.sql`:

```sql
ALTER TABLE dashboards ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX dashboards_deleted_idx ON dashboards (workspace_id, deleted_at);
```

Then run `touch crates/egret-metastore/src/lib.rs`.

- [ ] **Step 2: Write the failing test**

Append to `crates/egret-bi/tests/dashboard_repo.rs`:

```rust
#[tokio::test]
async fn trash_hides_from_live_list_and_restore_brings_back() {
    let Some(fx) = setup().await else { return };
    let d = fx.repo.create(fx.ws, "ToTrash", "", Some(fx.user)).await.unwrap();

    fx.repo.set_trashed(d.id, true).await.unwrap();
    let live: Vec<_> = fx.repo.list_by_workspace(fx.ws, false).await.unwrap();
    assert!(live.iter().all(|(x, _)| x.id != d.id), "trashed must be hidden from live list");
    let trashed: Vec<_> = fx.repo.list_by_workspace(fx.ws, true).await.unwrap();
    assert!(trashed.iter().any(|(x, _)| x.id == d.id), "trash list must include it");

    fx.repo.set_trashed(d.id, false).await.unwrap();
    let live2: Vec<_> = fx.repo.list_by_workspace(fx.ws, false).await.unwrap();
    assert!(live2.iter().any(|(x, _)| x.id == d.id), "restore returns it to live list");
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi trash_hides_from_live_list_and_restore_brings_back`
Expected: FAIL — `set_trashed` not found and `list_by_workspace` takes 1 arg.

- [ ] **Step 4: Repo — add `deleted_at`, `set_trashed`, list filter**

In `crates/egret-bi/src/dashboard.rs`:

1. Add the field to `Dashboard` (after `width`):
```rust
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
```
2. Extend `COLS`:
```rust
const COLS: &str = "id, workspace_id, name, description, parameters, created_by, updated_at, width, deleted_at";
```
3. Change `list_by_workspace` to take `trashed: bool`:
```rust
    pub async fn list_by_workspace(
        &self,
        workspace_id: Uuid,
        trashed: bool,
    ) -> Result<Vec<(Dashboard, i64)>, BiError> {
        let pred = if trashed { "deleted_at IS NOT NULL" } else { "deleted_at IS NULL" };
        let sql = format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM dashboard_cards c WHERE c.dashboard_id = dashboards.id) AS card_count \
             FROM dashboards WHERE workspace_id = $1 AND {pred} ORDER BY updated_at DESC"
        );
        let rows = sqlx::query_as::<_, DashboardWithCount>(&sql)
            .bind(workspace_id)
            .fetch_all(&self.db.pool)
            .await?;
        Ok(rows.into_iter().map(|r| (r.dash, r.card_count)).collect())
    }
```
4. Add `set_trashed`:
```rust
    /// Soft-delete (trashed=true → deleted_at=now()) or restore (false → NULL).
    pub async fn set_trashed(&self, id: Uuid, trashed: bool) -> Result<(), BiError> {
        let sql = if trashed {
            "UPDATE dashboards SET deleted_at = now() WHERE id = $1"
        } else {
            "UPDATE dashboards SET deleted_at = NULL WHERE id = $1"
        };
        sqlx::query(sql).bind(id).execute(&self.db.pool).await.map_err(map_conflict_err)?;
        Ok(())
    }
```

- [ ] **Step 5: DTO — expose `deleted_at` on detail**

In `crates/egret-bi/src/model.rs`: add `pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,` to `DashboardDetailDto`, and in `dashboard_detail_dto` set `deleted_at: d.deleted_at,`. (Summary DTO is unchanged — the list already filters.)

- [ ] **Step 6: Routes — `?trashed`, trash/restore handlers + wiring**

In `crates/egret-bi/src/routes.rs`:

`db_list` reads the query param and passes it through (add the `Query` extractor import `use axum::extract::Query;` if missing):

```rust
#[derive(serde::Deserialize)]
struct ListQuery { #[serde(default)] trashed: bool }

async fn db_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<DashboardSummaryDto>>, BiApiError> {
    st.authz.require_workspace(&ctx, DB_READ).await.map_err(BiApiError::from)?;
    let rows = st.dashboards.list_by_workspace(ctx.workspace_id.0, q.trashed).await.map_err(BiApiError::from)?;
    Ok(Json(rows.into_iter().map(|(d, n)| crate::model::dashboard_summary_dto(d, n)).collect()))
}
```

Add trash + restore handlers:

```rust
async fn db_trash(
    State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz.require_workspace(&ctx, DB_MANAGE).await.map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    require_dashboard_owner(&st, &ctx, id).await?;
    st.dashboards.set_trashed(id, true).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn db_restore(
    State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz.require_workspace(&ctx, DB_MANAGE).await.map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    require_dashboard_owner(&st, &ctx, id).await?;
    st.dashboards.set_trashed(id, false).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
```

Wire routes in `dashboard_router()`:

```rust
        .route("/ws/{workspace}/dashboards/{id}/trash", post(db_trash))
        .route("/ws/{workspace}/dashboards/{id}/restore", post(db_restore))
```

- [ ] **Step 7: Run test to verify it passes**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi trash_hides_from_live_list_and_restore_brings_back`
Expected: PASS.

- [ ] **Step 8: Backend sweep + commit**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi && cargo fmt --all --check
git add migrations/0019_dashboard_soft_delete.sql crates/egret-metastore/src/lib.rs crates/egret-bi/src/dashboard.rs crates/egret-bi/src/model.rs crates/egret-bi/src/routes.rs crates/egret-bi/tests/dashboard_repo.rs
git commit -m "feat(dashboards): soft-delete (trash) + restore backend

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Trash — frontend

**Files:**
- Modify: `frontend/src/api/dashboards.ts` (`deleted_at` on types; `useDashboards(ws, {trashed})`; `trash`/`restore` mutations)
- Modify: `frontend/src/routes/explore/Dashboards.tsx` (Move-to-trash + Trash section)
- Modify: `frontend/src/routes/explore/DashboardView.tsx` ("In Trash — Restore" banner)
- Test: `frontend/src/routes/explore/__tests__/Dashboards.trash.test.tsx` (create)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/routes/explore/__tests__/Dashboards.trash.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { createElement, type ReactNode } from "react";

const apiJson = vi.fn();
const apiFetch = vi.fn().mockResolvedValue({});
vi.mock("@/routes/explore/client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: (...a: unknown[]) => apiFetch(...a) }));
vi.mock("react-router-dom", async (orig) => ({ ...(await orig() as object), useParams: () => ({ workspace: "eng" }) }));

import Dashboards from "../Dashboards";

const wrap = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, createElement(MemoryRouter, null, children));

describe("Dashboards trash", () => {
  beforeEach(() => { apiJson.mockReset(); apiFetch.mockReset(); apiFetch.mockResolvedValue({}); });
  it("row Delete moves to trash (POST /trash), not hard delete", async () => {
    apiJson.mockResolvedValue([{ id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString() }]);
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    fireEvent.click(screen.getByRole("button", { name: "move One to trash" }));
    await waitFor(() => expect(apiJson).toHaveBeenCalledWith("/ws/eng/dashboards/d1/trash", expect.objectContaining({ method: "POST" })));
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/routes/explore/__tests__/Dashboards.trash.test.tsx`
Expected: FAIL — no "move One to trash" button (current label is "Delete").

- [ ] **Step 3: API — types, list arg, trash/restore mutations**

In `frontend/src/api/dashboards.ts`:

1. Add `deleted_at?: string | null;` to both `DashboardSummary` and `DashboardDetail` interfaces.
2. Make the list accept a trashed flag:
```ts
export function useDashboards(ws: string, opts?: { trashed?: boolean }) {
  const trashed = !!opts?.trashed;
  return useQuery<DashboardSummary[]>({
    queryKey: [...dashboardKeys.list(ws), { trashed }],
    queryFn: () => apiJson<DashboardSummary[]>(`/ws/${ws}/dashboards${trashed ? "?trashed=true" : ""}`),
  });
}
```
   (Adjust `invalidate` is unaffected — it invalidates the `dashboardKeys.list(ws)` prefix which still matches.)
3. Add `trash` + `restore` to `useDashboardMutations` before `return`:
```ts
  const trash = useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/dashboards/${id}/trash`, { method: "POST" }),
    onSuccess: () => invalidate(),
  });
  const restore = useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/dashboards/${id}/restore`, { method: "POST" }),
    onSuccess: () => invalidate(),
  });
```
   and return them: `return { create, update, remove, duplicate, trash, restore };`

- [ ] **Step 4: List — Move-to-trash + Trash section**

In `frontend/src/routes/explore/Dashboards.tsx`:

1. Add a `showTrash` toggle state: `const [showTrash, setShowTrash] = useState(false);` and read the matching list: `const { data, isLoading } = useDashboards(ws, { trashed: showTrash });`
2. Destructure: `const { create, remove, duplicate, trash, restore } = useDashboardMutations(ws);`
3. Add a toggle button next to "+ Dashboard":
```tsx
        <Button size="sm" variant={showTrash ? "default" : "outline"} onClick={() => setShowTrash((v) => !v)}>
          {showTrash ? "← Dashboards" : "Trash"}
        </Button>
```
4. In each row's actions cell, render different actions for live vs trash:
```tsx
                <td className="flex gap-1">
                  {showTrash ? (
                    <>
                      <Button size="sm" variant="ghost" aria-label={`restore ${d.name}`}
                        onClick={async () => { try { await restore.mutateAsync(d.id); } catch (e) { toast.error(String(e)); } }}>Restore</Button>
                      <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive" aria-label={`delete ${d.name} permanently`}
                        onClick={async () => { if (!confirm(`Permanently delete "${d.name}"?`)) return; try { await remove.mutateAsync(d.id); } catch (e) { toast.error(String(e)); } }}>Delete permanently</Button>
                    </>
                  ) : (
                    <>
                      <Button size="sm" variant="ghost" aria-label={`duplicate ${d.name}`}
                        onClick={async () => { try { const c = await duplicate.mutateAsync(d.id); nav(`/explore/${ws}/dashboards/${c.id}`); } catch (e) { toast.error(String(e)); } }}>Duplicate</Button>
                      <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive" aria-label={`move ${d.name} to trash`}
                        onClick={async () => { try { await trash.mutateAsync(d.id); } catch (e) { toast.error(String(e)); } }}>Delete</Button>
                    </>
                  )}
                </td>
```
   (`confirm` is the browser global; jsdom stubs it — in the test, `apiFetch`/`apiJson` are mocked and `confirm` defaults to true only when called, so the permanent-delete path isn't exercised by the Task-4 test.)

- [ ] **Step 5: Detail — "In Trash" banner**

In `frontend/src/routes/explore/DashboardView.tsx`, after the dashboard loads (where `dash` is available, above the grid), render a banner when trashed:

```tsx
      {dash.deleted_at && (
        <div className="mb-3 flex items-center justify-between rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm">
          <span>This dashboard is in the trash.</span>
          <Button size="sm" variant="outline" onClick={async () => { try { await mutations.restore.mutateAsync(dash.id); } catch (e) { toast.error(String(e)); } }}>Restore</Button>
        </div>
      )}
```
   Ensure `DashboardView` has access to `useDashboardMutations(ws)` (named `mutations`) and `toast`. If it already destructures specific mutations, add `restore` there.

- [ ] **Step 6: Run test + sweep + commit**

Run: `npx vitest run src/routes/explore/__tests__/Dashboards.trash.test.tsx` → PASS.

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/api/dashboards.ts src/routes/explore/Dashboards.tsx src/routes/explore/DashboardView.tsx src/routes/explore/__tests__/Dashboards.trash.test.tsx
git commit -m "feat(dashboards): move-to-trash, trash view with restore + permanent delete

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Migration 0020 + Bookmark — backend

**Files:**
- Create: `migrations/0020_bookmarks.sql`
- Modify: `crates/egret-bi/src/dashboard.rs` (`add_bookmark`/`remove_bookmark`; `list_by_workspace` returns `bookmarked`; `delete` purges bookmarks)
- Modify: `crates/egret-bi/src/model.rs` (`DashboardSummaryDto.bookmarked` + `DashboardDetailDto.bookmarked` + builders)
- Modify: `crates/egret-bi/src/routes.rs` (`db_list`/`db_get` pass user; `db_bookmark_add`/`db_bookmark_remove` + routes)
- Test: `crates/egret-bi/tests/dashboard_repo.rs` (`bookmark_toggles_and_flag_in_list`)

- [ ] **Step 1: Write the migration**

Create `migrations/0020_bookmarks.sql`:

```sql
CREATE TABLE bookmarks (
  user_id     uuid        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  entity_type text        NOT NULL,
  entity_id   uuid        NOT NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, entity_type, entity_id)
);
CREATE INDEX bookmarks_user_idx ON bookmarks (user_id, entity_type);
```

Then `touch crates/egret-metastore/src/lib.rs`.

- [ ] **Step 2: Write the failing test**

Append to `crates/egret-bi/tests/dashboard_repo.rs`:

```rust
#[tokio::test]
async fn bookmark_toggles_and_shows_in_list_flag() {
    let Some(fx) = setup().await else { return };
    let d = fx.repo.create(fx.ws, "Fav", "", Some(fx.user)).await.unwrap();

    fx.repo.add_bookmark(fx.user, "dashboard", d.id).await.unwrap();
    fx.repo.add_bookmark(fx.user, "dashboard", d.id).await.unwrap(); // idempotent
    let listed = fx.repo.list_by_workspace_for(fx.ws, fx.user, false).await.unwrap();
    let row = listed.iter().find(|(x, _, _)| x.id == d.id).expect("listed");
    assert!(row.2, "bookmarked flag should be true");

    fx.repo.remove_bookmark(fx.user, "dashboard", d.id).await.unwrap();
    let listed2 = fx.repo.list_by_workspace_for(fx.ws, fx.user, false).await.unwrap();
    let row2 = listed2.iter().find(|(x, _, _)| x.id == d.id).expect("listed");
    assert!(!row2.2, "bookmarked flag should be false after removal");
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi bookmark_toggles_and_shows_in_list_flag`
Expected: FAIL — `add_bookmark`/`list_by_workspace_for` not found.

- [ ] **Step 4: Repo — bookmark methods + bookmarked-aware list + delete purge**

In `crates/egret-bi/src/dashboard.rs`, add to `impl DashboardRepo`:

```rust
    pub async fn add_bookmark(&self, user: Uuid, entity_type: &str, entity_id: Uuid) -> Result<(), BiError> {
        sqlx::query(
            "INSERT INTO bookmarks (user_id, entity_type, entity_id) VALUES ($1,$2,$3) \
             ON CONFLICT (user_id, entity_type, entity_id) DO NOTHING",
        )
        .bind(user).bind(entity_type).bind(entity_id)
        .execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn remove_bookmark(&self, user: Uuid, entity_type: &str, entity_id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM bookmarks WHERE user_id=$1 AND entity_type=$2 AND entity_id=$3")
            .bind(user).bind(entity_type).bind(entity_id)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn is_bookmarked(&self, user: Uuid, id: Uuid) -> Result<bool, BiError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM bookmarks WHERE user_id=$1 AND entity_type='dashboard' AND entity_id=$2)",
        )
        .bind(user).bind(id)
        .fetch_one(&self.db.pool).await?)
    }

    /// Like `list_by_workspace` but also returns a per-user `bookmarked` flag.
    pub async fn list_by_workspace_for(
        &self,
        workspace_id: Uuid,
        user: Uuid,
        trashed: bool,
    ) -> Result<Vec<(Dashboard, i64, bool)>, BiError> {
        let pred = if trashed { "deleted_at IS NOT NULL" } else { "deleted_at IS NULL" };
        let sql = format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM dashboard_cards c WHERE c.dashboard_id = dashboards.id) AS card_count, \
             EXISTS (SELECT 1 FROM bookmarks b WHERE b.user_id = $2 AND b.entity_type='dashboard' AND b.entity_id = dashboards.id) AS bookmarked \
             FROM dashboards WHERE workspace_id = $1 AND {pred} ORDER BY updated_at DESC"
        );
        let rows = sqlx::query_as::<_, DashboardWithCountBm>(&sql)
            .bind(workspace_id).bind(user)
            .fetch_all(&self.db.pool).await?;
        Ok(rows.into_iter().map(|r| (r.dash, r.card_count, r.bookmarked)).collect())
    }
```

Add the private row struct near `DashboardWithCount`:

```rust
#[derive(sqlx::FromRow)]
struct DashboardWithCountBm {
    #[sqlx(flatten)]
    dash: Dashboard,
    card_count: i64,
    bookmarked: bool,
}
```

Extend `delete` to purge bookmarks (it currently only deletes the dashboard row; cards/tabs cascade):

```rust
    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM bookmarks WHERE entity_type='dashboard' AND entity_id=$1")
            .bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM dashboards WHERE id=$1")
            .bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
```

(Keep the existing single-arg `list_by_workspace(ws, trashed)` from Task 3 — Task 4's frontend used `db_list`, which we now switch to the `_for` variant in Step 6. `list_by_workspace` may stay for the trash repo test from Task 3, or you can repoint that test to `_for`. Keep both; `list_by_workspace` is still used by the Task-3 test.)

- [ ] **Step 5: DTO — `bookmarked` on summary + detail**

In `crates/egret-bi/src/model.rs`:
- Add `pub bookmarked: bool,` to `DashboardSummaryDto` and `DashboardDetailDto`.
- Change `dashboard_summary_dto` to take the flag:
```rust
pub fn dashboard_summary_dto(d: Dashboard, card_count: i64, bookmarked: bool) -> DashboardSummaryDto {
    DashboardSummaryDto { id: d.id.to_string(), name: d.name, description: d.description,
        created_by: d.created_by.map(|u| u.to_string()), card_count, updated_at: d.updated_at, bookmarked }
}
```
- Change `dashboard_detail_dto` to accept + set `bookmarked` (add a `bookmarked: bool` param, set the field). Update its only caller (`load_dashboard`) in Step 6.

- [ ] **Step 6: Routes — bookmarked-aware list/detail + bookmark endpoints**

In `crates/egret-bi/src/routes.rs`:

`db_list` uses the `_for` variant + the user:
```rust
    let rows = st.dashboards.list_by_workspace_for(ctx.workspace_id.0, ctx.principal.user_id.0, q.trashed)
        .await.map_err(BiApiError::from)?;
    Ok(Json(rows.into_iter().map(|(d, n, bm)| crate::model::dashboard_summary_dto(d, n, bm)).collect()))
```

`load_dashboard` needs the user for the detail `bookmarked` flag — change its signature to take `user: Uuid`:
```rust
async fn load_dashboard(st: &BiState, id: Uuid, user: Uuid) -> Result<DashboardDetailDto, BiApiError> {
    let d = st.dashboards.get(id).await.map_err(BiApiError::from)?.ok_or(Error::NotFound("dashboard".into()))?;
    let tabs = st.dashboards.tabs(id).await.map_err(BiApiError::from)?;
    let cards = st.dashboards.cards(id).await.map_err(BiApiError::from)?;
    let bookmarked = st.dashboards.is_bookmarked(user, id).await.map_err(BiApiError::from)?;
    Ok(crate::model::dashboard_detail_dto(d, tabs, cards, bookmarked))
}
```
Update every `load_dashboard(&st, X)` call (`db_create`, `db_get`, `db_update`, `db_duplicate`) to pass `ctx.principal.user_id.0`.

Add bookmark handlers + routes:
```rust
async fn db_bookmark_add(
    State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz.require_workspace(&ctx, DB_READ).await.map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    st.dashboards.add_bookmark(ctx.principal.user_id.0, "dashboard", id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn db_bookmark_remove(
    State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz.require_workspace(&ctx, DB_READ).await.map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    st.dashboards.remove_bookmark(ctx.principal.user_id.0, "dashboard", id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
```
Route (one path, two methods):
```rust
        .route("/ws/{workspace}/dashboards/{id}/bookmark", post(db_bookmark_add).delete(db_bookmark_remove))
```

- [ ] **Step 7: Run test to verify it passes**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi bookmark_toggles_and_shows_in_list_flag`
Expected: PASS. Also run the whole suite to catch the `dashboard_summary_dto`/`load_dashboard` signature changes:
`DATABASE_URL=... cargo test -p egret-bi` → all green.

- [ ] **Step 8: Backend sweep + commit**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi && cargo fmt --all --check
git add migrations/0020_bookmarks.sql crates/egret-metastore/src/lib.rs crates/egret-bi/src/dashboard.rs crates/egret-bi/src/model.rs crates/egret-bi/src/routes.rs crates/egret-bi/tests/dashboard_repo.rs
git commit -m "feat(dashboards): bookmark backend (generic table, dashboard-scoped, list flag)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Bookmark — frontend

**Files:**
- Modify: `frontend/src/api/dashboards.ts` (`bookmarked` on types; `addBookmark`/`removeBookmark` mutations)
- Modify: `frontend/src/routes/explore/Dashboards.tsx` (star toggle per row + Bookmarks filter)
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (star toggle in header)
- Test: `frontend/src/routes/explore/__tests__/Dashboards.bookmark.test.tsx` (create)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/routes/explore/__tests__/Dashboards.bookmark.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { createElement, type ReactNode } from "react";

const apiJson = vi.fn();
vi.mock("@/routes/explore/client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: vi.fn() }));
vi.mock("react-router-dom", async (orig) => ({ ...(await orig() as object), useParams: () => ({ workspace: "eng" }) }));

import Dashboards from "../Dashboards";

const wrap = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, createElement(MemoryRouter, null, children));

describe("Dashboards bookmark", () => {
  beforeEach(() => apiJson.mockReset());
  it("clicking the star bookmarks the dashboard (POST /bookmark)", async () => {
    apiJson.mockResolvedValue([{ id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString(), bookmarked: false }]);
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    fireEvent.click(screen.getByRole("button", { name: "bookmark One" }));
    await waitFor(() => expect(apiJson).toHaveBeenCalledWith("/ws/eng/dashboards/d1/bookmark", expect.objectContaining({ method: "POST" })));
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/routes/explore/__tests__/Dashboards.bookmark.test.tsx`
Expected: FAIL — no "bookmark One" button.

- [ ] **Step 3: API — `bookmarked` type + mutations**

In `frontend/src/api/dashboards.ts`: add `bookmarked?: boolean;` to `DashboardSummary` and `DashboardDetail`. Add to `useDashboardMutations`:

```ts
  const addBookmark = useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/dashboards/${id}/bookmark`, { method: "POST" }),
    onSuccess: () => invalidate(),
  });
  const removeBookmark = useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/dashboards/${id}/bookmark`, { method: "DELETE" }),
    onSuccess: () => invalidate(),
  });
```
Return them: `return { create, update, remove, duplicate, trash, restore, addBookmark, removeBookmark };`

- [ ] **Step 4: List — star toggle + Bookmarks filter**

In `frontend/src/routes/explore/Dashboards.tsx`:
- `import { Star } from "lucide-react";`
- Destructure `addBookmark, removeBookmark`.
- Add a Bookmarks filter state: `const [onlyBookmarked, setOnlyBookmarked] = useState(false);` and a toggle button near the others:
```tsx
        <Button size="sm" variant={onlyBookmarked ? "default" : "outline"} onClick={() => setOnlyBookmarked((v) => !v)}>★ Bookmarks</Button>
```
- Filter the rendered rows: `const rows = (data ?? []).filter((d) => !onlyBookmarked || d.bookmarked);` and map over `rows` instead of `data`.
- Prepend a star button to each row's name cell or actions:
```tsx
                  <Button size="sm" variant="ghost" aria-label={`${d.bookmarked ? "unbookmark" : "bookmark"} ${d.name}`}
                    onClick={async () => { try { await (d.bookmarked ? removeBookmark : addBookmark).mutateAsync(d.id); } catch (e) { toast.error(String(e)); } }}>
                    <Star className={`h-4 w-4 ${d.bookmarked ? "fill-amber-400 text-amber-400" : "text-muted-foreground"}`} />
                  </Button>
```

- [ ] **Step 5: Header — star toggle in DashboardView**

In `frontend/src/routes/explore/DashboardView.tsx` header (next to Fullscreen/Edit), add:

```tsx
        <Button size="sm" variant="ghost" aria-label={dash.bookmarked ? "remove bookmark" : "add bookmark"}
          onClick={async () => { try { await (dash.bookmarked ? mutations.removeBookmark : mutations.addBookmark).mutateAsync(dash.id); } catch (e) { toast.error(String(e)); } }}>
          <Star className={`h-4 w-4 ${dash.bookmarked ? "fill-amber-400 text-amber-400" : ""}`} />
        </Button>
```
   (`import { Star } from "lucide-react";` if not present; `mutations = useDashboardMutations(ws)`.)

- [ ] **Step 6: Run test + sweep + commit**

Run: `npx vitest run src/routes/explore/__tests__/Dashboards.bookmark.test.tsx` → PASS.

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/api/dashboards.ts src/routes/explore/Dashboards.tsx src/routes/explore/DashboardView.tsx src/routes/explore/__tests__/Dashboards.bookmark.test.tsx
git commit -m "feat(dashboards): bookmark star toggle + Bookmarks filter

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Info sidebar (frontend)

**Files:**
- Create: `frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (ⓘ button + render the sidebar)
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardInfoSidebar.test.tsx` (create)

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/DashboardInfoSidebar.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DashboardInfoSidebar } from "../DashboardInfoSidebar";

describe("DashboardInfoSidebar", () => {
  const base = { name: "Sales", description: "old", ownerLabel: "admin@egret.local", updatedAt: "2026-06-22T04:08:30Z" };
  it("shows owner + last edited and renders the description", () => {
    render(<DashboardInfoSidebar {...base} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText("admin@egret.local")).toBeTruthy();
    expect((screen.getByLabelText("description") as HTMLTextAreaElement).value).toBe("old");
  });
  it("saves an edited description on blur", () => {
    const onSave = vi.fn();
    render(<DashboardInfoSidebar {...base} onSaveDescription={onSave} onClose={vi.fn()} />);
    const ta = screen.getByLabelText("description");
    fireEvent.change(ta, { target: { value: "new desc" } });
    fireEvent.blur(ta);
    expect(onSave).toHaveBeenCalledWith("new desc");
  });
  it("does not save when the description is unchanged", () => {
    const onSave = vi.fn();
    render(<DashboardInfoSidebar {...base} onSaveDescription={onSave} onClose={vi.fn()} />);
    fireEvent.blur(screen.getByLabelText("description"));
    expect(onSave).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/DashboardInfoSidebar.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Create the component**

Create `frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx`:

```tsx
import { useState } from "react";

interface Props {
  name: string;
  description: string;
  ownerLabel: string | null;
  updatedAt: string;
  onSaveDescription: (next: string) => void;
  onClose: () => void;
}

export function DashboardInfoSidebar({ name, description, ownerLabel, updatedAt, onSaveDescription, onClose }: Props) {
  const [draft, setDraft] = useState(description);
  return (
    <aside className="sticky top-4 max-h-[calc(100vh-7rem)] w-[300px] shrink-0 self-start overflow-auto rounded-lg border bg-card p-3 text-sm">
      <div className="mb-2 flex items-center justify-between">
        <div className="font-medium">{name}</div>
        <button type="button" aria-label="close info" className="text-muted-foreground hover:text-foreground" onClick={onClose}>✕</button>
      </div>
      <label className="mb-3 flex flex-col gap-1 text-xs text-muted-foreground">Description
        <textarea aria-label="description" className="h-24 w-full rounded border bg-background px-2 py-1 text-sm"
          value={draft} onChange={(e) => setDraft(e.target.value)}
          onBlur={() => { if (draft !== description) onSaveDescription(draft); }} />
      </label>
      <dl className="flex flex-col gap-1 text-xs">
        <div className="flex justify-between"><dt className="text-muted-foreground">Owner</dt><dd>{ownerLabel ?? "—"}</dd></div>
        <div className="flex justify-between"><dt className="text-muted-foreground">Last edited</dt><dd>{new Date(updatedAt).toLocaleString()}</dd></div>
      </dl>
    </aside>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npx vitest run src/components/chart/dashboard/__tests__/DashboardInfoSidebar.test.tsx`
Expected: PASS.

- [ ] **Step 5: Wire into DashboardView**

In `frontend/src/routes/explore/DashboardView.tsx`:
1. `import { DashboardInfoSidebar } from "@/components/chart/dashboard/DashboardInfoSidebar";` and `import { Info } from "lucide-react";`
2. State: `const [showInfo, setShowInfo] = useState(false);` (place with the other top-level hooks, above the early return).
3. Header button (next to the bookmark/Fullscreen): `<Button size="sm" variant="ghost" aria-label="dashboard info" onClick={() => setShowInfo((v) => !v)}><Info className="h-4 w-4" /></Button>`
4. Resolve the owner label: charts/users aren't loaded here, so show `dash.created_by` directly (the id) OR, if an owner email is readily available via an existing hook, use it. Minimal: `ownerLabel={dash.created_by}` (the uuid). If a `useUser`/members hook exists in the codebase, map to email; otherwise the id is acceptable for this tier — note it in the PR.
5. Render the sidebar alongside the grid when `showInfo` (mirror how `EditSidebar` is rendered in a flex row; in view mode wrap the grid + sidebar in `<div className="flex gap-4">`):
```tsx
        {showInfo && (
          <DashboardInfoSidebar
            name={dash.name}
            description={dash.description}
            ownerLabel={dash.created_by}
            updatedAt={dash.updated_at}
            onClose={() => setShowInfo(false)}
            onSaveDescription={(next) => mutations.update.mutateAsync({
              id: dash.id, name: dash.name, description: next, parameters: dash.parameters,
              tabs: dash.tabs.map((t) => ({ name: t.name, position: t.position })),
              width: dash.width,
              cards: dash.cards.map((c) => ({ chart_id: c.chart_id, card_type: c.card_type, grid_col: c.grid_col, grid_row: c.grid_row, size_x: c.size_x, size_y: c.size_y, options: c.options, tab_index: tabIndexForCard(c) })),
            }).catch((e) => toast.error(String(e)))}
          />
        )}
```
   Use the SAME card→`tab_index` mapping the existing Save path uses (there is already a serialize/layout helper in `tabs.ts` / the Save handler — reuse it rather than re-deriving). If a `serializeLayout`-style helper exists, call it to build the update payload with only `description` changed. Keep this DRY: prefer calling the existing save serializer with an overridden description over hand-building `cards`.

- [ ] **Step 6: Sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/DashboardInfoSidebar.tsx src/components/chart/dashboard/__tests__/DashboardInfoSidebar.test.tsx src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboards): info sidebar with owner, last-edited, editable description

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Dark-mode audit (frontend)

**Files:**
- Modify: any dashboard component with a hardcoded color found during the audit (likely none).
- Test: none (visual audit; covered live in Task 9).

- [ ] **Step 1: Grep for hardcoded colors in dashboard UI**

Run (from `frontend/`):
```bash
grep -rnE "(bg|text|border)-(white|black|gray-[0-9]|slate-[0-9]|zinc-[0-9])|#[0-9a-fA-F]{3,6}" src/components/chart/dashboard src/routes/explore/Dashboards.tsx src/routes/explore/DashboardView.tsx
```
Expected: review each hit. Tailwind theme tokens (`bg-card`, `bg-background`, `text-muted-foreground`, `border`, `bg-primary`, `text-destructive`, `bg-amber-500/10` for the trash banner, `text-amber-400` for the star) are theme-aware and FINE. Only a literal `bg-white`/`#fff`-style color that ignores the theme is a defect.

- [ ] **Step 2: Fix any offender**

For each genuine offender, replace with the matching token (e.g. `bg-white` → `bg-card` or `bg-background`; a hardcoded hex → the nearest token). If the grep finds none (expected — the live audit already showed the dashboard renders correctly in dark), there is nothing to change; record that in the commit body.

- [ ] **Step 3: Sweep + commit (only if changes were made)**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add <only the files you changed>
git commit -m "fix(dashboards): dark-mode token fixes (audit)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
If no files changed, skip the commit and note "dark-mode audit clean — no hardcoded colors" in the Task 9 PR body.

---

### Task 9: Live verification + PR

**Files:** none (verification only).

- [ ] **Step 1: Final sweeps**

Backend (repo root): `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi && cargo fmt --all --check` → green.
Frontend (`frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run` → green.

- [ ] **Step 2: Run the stack & log in**

Bring up docker + `cargo run serve` (it will apply migrations 0019/0020) + `npm run dev`; log in `admin@egret.local` / `Admin12345!`, eng workspace. **Do NOT touch the user's "a1" dashboard.** Create a throwaway dashboard for any destructive checks and clean it up.

- [ ] **Step 3: Live-verify (Playwright MCP, side-by-side Metabase), 0 console errors**

1. **Duplicate**: on a dashboard, Duplicate → a "<name> (copy)" opens with the same cards/tabs.
2. **Trash**: list "Delete" → row leaves the live list; **Trash** toggle shows it → **Restore** returns it; **Delete permanently** (confirm) removes it for good.
3. **Bookmark**: star a dashboard (list + header) → ★ filled; **Bookmarks** filter shows only starred; unstar clears it.
4. **Info sidebar**: ⓘ opens the panel (owner, last edited, description); edit the description → persists on reload.
5. **Dark mode**: toggle the global theme → list, trash view, info sidebar, star, banners all render correctly.

- [ ] **Step 4: Clean up & open PR**

Delete throwaway dashboards. Then:
```bash
git push -u origin feat/dashboards-tier3a
```
Open PR `feat(dashboards): Tier 3a — view polish (duplicate, trash, bookmark, info sidebar)`; summarize the five features + the two migrations (0019 soft-delete, 0020 bookmarks); note dark-mode was already covered by the global theme. End the PR body with the `🤖 Generated with [Claude Code]` footer. Squash-merge when CI is green.

---

## Self-Review

**Spec coverage:**
- Dark-mode audit → Task 8. ✓
- Info sidebar (owner/last-edited/editable description, existing PUT) → Task 7. ✓
- Duplicate (repo deep-copy + endpoint + UI) → Tasks 1–2. ✓
- Trash (migration 0019, soft-delete list filter, trash/restore, permanent delete stays, detail `deleted_at` banner) → Tasks 3–4. ✓
- Bookmark (migration 0020 generic table, add/remove, `bookmarked` flag, endpoints, star UI + filter, permanent-delete purge) → Tasks 5–6. ✓
- Error handling: duplicate 404 (`require_dashboard_in_ws`), trash/restore 403 (`require_dashboard_owner`), bookmark idempotent (ON CONFLICT DO NOTHING), purge on permanent delete (Task 5 Step 4), restore name-collision → `map_conflict_err` returns Conflict (409) on the unique violation. ✓
- Testing: backend repo tests (Tasks 1/3/5), frontend Vitest (Tasks 2/4/6/7), live (Task 9). ✓
- Migration staleness touch (Tasks 3/5). ✓

**Type/signature consistency:**
- `list_by_workspace(ws, trashed)` introduced in Task 3; Task 5 ADDS `list_by_workspace_for(ws, user, trashed) -> Vec<(Dashboard, i64, bool)>` and repoints `db_list` to it (Task 3's repo test keeps using `list_by_workspace`, which still exists). Consistent.
- `dashboard_summary_dto(d, n, bookmarked)` and `dashboard_detail_dto(d, tabs, cards, bookmarked)` — both gain the `bookmarked` param in Task 5; all callers updated there.
- `load_dashboard(&st, id, user)` — signature change in Task 5; all four callers updated.
- Frontend `useDashboardMutations` returns grow monotonically: `{create, update, remove}` → `+duplicate` (T2) → `+trash, restore` (T4) → `+addBookmark, removeBookmark` (T6). Each task adds, never renames.
- `DashboardSummary`/`DashboardDetail` gain `deleted_at?` (T4) and `bookmarked?` (T6) — additive.

**Placeholder scan:** No TBD/TODO; every code step shows complete code. The one soft spot — Task 7 owner-label resolution — is explicitly bounded (use `created_by` id if no email hook exists) rather than left vague.
