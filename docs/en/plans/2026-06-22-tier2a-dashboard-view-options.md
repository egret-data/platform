# Tier 2a — Dashboard View Options (fixed/full width + dark-mode) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a persisted fixed/full-width dashboard setting (Metabase-style) and verify/fix the dashboard's dark-mode appearance.

**Architecture:** A new `dashboards.width` column ('fixed'|'full') flows through the repo + DTO + whole-dashboard PUT; the frontend adds an edit-mode Width toggle and a centered container in view mode when fixed. Dark mode is a verify-and-fix pass (the dashboard chrome already uses theme tokens).

**Tech Stack:** Rust (axum, sqlx, Postgres) for `egret-bi`; React 18 + TS + Vite + Vitest for the frontend.

**Spec:** `docs/en/specs/2026-06-22-tier2a-dashboard-view-options-design.md`

**Branch:** `feat/dashboards-tier2a` (already checked out off main, which includes Tier 1).

## Conventions (every task)

- TDD: failing test → run (fail) → implement → run (pass) → commit.
- **Backend full sweep before commit:** `cargo fmt --all --check && cargo build -p egret-bin && cargo test -p egret-bi` (a local Postgres is at `postgres://egret:egret@localhost:5432/egret` — export `DATABASE_URL` so DB-gated tests run). `cargo fmt` is a CI step — run it or CI goes red.
- **Frontend full sweep before commit (from `frontend/`):** `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Configured git identity (never `--author`); commit body ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Stage explicit paths.
- **sqlx migration-staleness:** after adding a migration, `touch crates/egret-metastore/src/lib.rs` before any `cargo build`/`cargo test`.

## File structure

- `migrations/0018_dashboard_width.sql` (Create) — the `width` column.
- `crates/egret-bi/src/dashboard.rs` (Modify) — `Dashboard.width`, `COLS`, `update(…, width)`.
- `crates/egret-bi/src/model.rs` (Modify) — `DashboardDetailDto.width`, `UpdateDashboard.width`, `dashboard_detail_dto`.
- `crates/egret-bi/src/routes.rs` (Modify) — `db_update` validates + passes `width`.
- `crates/egret-bi/tests/dashboard_repo.rs` (Modify) — width round-trip test.
- `frontend/src/api/dashboards.ts` (Modify) — `DashboardDetail.width`, update body `width`.
- `frontend/src/routes/explore/DashboardView.tsx` (Modify) — Width toggle + fixed wrapper.
- `frontend/src/routes/explore/__tests__/DashboardView.width.test.tsx` (Create) — width tests.

---

## Task 1: Backend `width` column + repo/DTO/router

**Files:**
- Create: `migrations/0018_dashboard_width.sql`
- Modify: `crates/egret-bi/src/dashboard.rs`, `crates/egret-bi/src/model.rs`, `crates/egret-bi/src/routes.rs`
- Test: `crates/egret-bi/tests/dashboard_repo.rs`

- [ ] **Step 1: Create the migration**

`migrations/0018_dashboard_width.sql`:
```sql
ALTER TABLE dashboards
  ADD COLUMN width text NOT NULL DEFAULT 'full' CHECK (width IN ('fixed', 'full'));
```
Then: `touch crates/egret-metastore/src/lib.rs`.

- [ ] **Step 2: Write the failing repo test**

Append to `crates/egret-bi/tests/dashboard_repo.rs`:
```rust
#[tokio::test]
async fn dashboard_width_defaults_full_and_round_trips() {
    let Some(f) = setup().await else { eprintln!("skip"); return; };
    let d = f.repo.create(f.ws, "Wide", "", Some(f.user)).await.unwrap();
    assert_eq!(f.repo.get(d.id).await.unwrap().unwrap().width, "full");

    f.repo.update(d.id, "Wide", "", &serde_json::json!([]), "fixed").await.unwrap();
    assert_eq!(f.repo.get(d.id).await.unwrap().unwrap().width, "fixed");
}
```

- [ ] **Step 3: Run it — RED (compile error)**

Run: `touch crates/egret-metastore/src/lib.rs && cargo test -p egret-bi dashboard_width_defaults_full_and_round_trips`
Expected: compile error — `Dashboard` has no `width`, and `update` takes 4 args not 5.

- [ ] **Step 4: Add `width` to the repo (`dashboard.rs`)**

Add the field to `Dashboard` (last field):
```rust
    pub width: String,
```
Extend `COLS`:
```rust
const COLS: &str = "id, workspace_id, name, description, parameters, created_by, updated_at, width";
```
Change `update` to take + set `width`:
```rust
    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        parameters: &serde_json::Value,
        width: &str,
    ) -> Result<(), BiError> {
        sqlx::query(
            "UPDATE dashboards SET name=$2, description=$3, parameters=$4, width=$5, updated_at=now() WHERE id=$1",
        )
        .bind(id).bind(name).bind(description).bind(parameters).bind(width)
        .execute(&self.db.pool).await.map_err(map_conflict_err)?;
        Ok(())
    }
```
(`create` uses `RETURNING {COLS}` so it already returns `width` — no other repo change. `list_by_workspace` selects `{COLS}` + a count; it still maps into `Dashboard`, now with `width` — fine.)

- [ ] **Step 5: DTO + router (`model.rs`, `routes.rs`)**

`model.rs` — `DashboardDetailDto` gains (before `tabs`):
```rust
    pub width: String,
```
`UpdateDashboard` gains:
```rust
    #[serde(default = "default_width")]
    pub width: String,
```
Add the default fn near the other serde defaults:
```rust
fn default_width() -> String { "full".into() }
```
`dashboard_detail_dto` sets it:
```rust
        width: d.width,
```
(place it right before `tabs: …`).

`routes.rs` `db_update` — validate + pass width. After the `validate_layout(...)` line, add:
```rust
    if b.width != "fixed" && b.width != "full" {
        return Err(egret_core::Error::InvalidInput("width must be 'fixed' or 'full'".into()).into());
    }
```
and change the `update` call to pass width:
```rust
    st.dashboards.update(id, &b.name, &b.description, &b.parameters, &b.width).await.map_err(BiApiError::from)?;
```

- [ ] **Step 6: Fix existing `update` call sites**

`grep -rn "\.update(" crates/egret-bi/` — the repo test `dashboard_crud_cards_and_unique_conflict` calls `f.repo.update(d.id, "Ops v2", "", &dash_params)` (and one with `&serde_json::json!([])`). Add `, "full"` as the 5th arg to each. (The router call site is already fixed in Step 5.)

- [ ] **Step 7: Run the test — GREEN**

Run: `touch crates/egret-metastore/src/lib.rs && DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi dashboard_width_defaults_full_and_round_trips`
Expected: PASS (or "skip" if no DB — but the local DB is up, so it runs).

- [ ] **Step 8: Backend sweep + commit**

Run: `cargo fmt --all --check && cargo build -p egret-bin && DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi`
(If `cargo fmt --all --check` reports diffs, run `cargo fmt --all` and re-check.)
```bash
git add migrations/0018_dashboard_width.sql crates/egret-bi/src/dashboard.rs crates/egret-bi/src/model.rs crates/egret-bi/src/routes.rs crates/egret-bi/tests/dashboard_repo.rs crates/egret-metastore/src/lib.rs
git commit -m "feat(dashboards): persist fixed/full width on the dashboard

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Frontend width toggle + fixed container

**Files:**
- Modify: `frontend/src/api/dashboards.ts`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/routes/explore/__tests__/DashboardView.width.test.tsx`

- [ ] **Step 1: API types**

In `frontend/src/api/dashboards.ts`, add `width` to `DashboardDetail` (after `tabs` or `parameters`):
```ts
  width: "fixed" | "full";
```
And to the `update` mutation body type (the `mutationFn` arg), add `width: "fixed" | "full";` alongside `tabs`/`cards`.

- [ ] **Step 2: Failing test `frontend/src/routes/explore/__tests__/DashboardView.width.test.tsx`**

(Mirror the mock setup from `DashboardView.tabs.test.tsx` — mock `@/api/dashboards`, `@/api/charts`, `DashboardCardView`, router.)
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", () => ({ useCharts: () => ({ data: [] }) }));

const update = { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false };
const base = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [],
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }],
  width: "full",
};
let detail = { ...base };
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update }) };
});

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

describe("DashboardView width", () => {
  it("view mode 'fixed' wraps content in a centered max-width container", () => {
    detail = { ...base, width: "fixed" };
    const { container } = renderView();
    expect(container.querySelector(".max-w-\\[1200px\\]")).toBeTruthy();
  });
  it("view mode 'full' has no fixed wrapper", () => {
    detail = { ...base, width: "full" };
    const { container } = renderView();
    expect(container.querySelector(".max-w-\\[1200px\\]")).toBeNull();
  });
  it("edit mode: choosing Fixed and saving sends width in the payload", () => {
    detail = { ...base, width: "full" };
    update.mutateAsync.mockClear();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "width fixed" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(update.mutateAsync).toHaveBeenCalledWith(expect.objectContaining({ width: "fixed" }));
  });
});
```

- [ ] **Step 3: Run it — RED.** Run (from `frontend/`): `npx vitest run src/routes/explore/__tests__/DashboardView.width.test.tsx`. Expected FAIL (no wrapper, no Width toggle, payload lacks width).

- [ ] **Step 4: Implement in `DashboardView.tsx`**

Add a `draftWidth` state **above** the `if (isLoading || !dash) return …` early return (Rules of Hooks), next to the other `useState`s:
```ts
  const [draftWidth, setDraftWidth] = useState<"fixed" | "full">("full");
```
Seed it in `startEdit`:
```ts
    setDraftWidth(dashboard.width);
```
After the early return, derive the effective width:
```ts
  const effectiveWidth = editing ? draftWidth : dashboard.width;
```
Add `width` to the `onSave` PUT payload (the `update.mutateAsync({ … })` call):
```ts
      width: draftWidth,
```
Add the Width toggle to the **edit** header (next to Cancel/Save). Find the `{editing ? ( … Cancel/Save … )` block and add before/after the Cancel button:
```tsx
              <div className="flex items-center gap-1 rounded border p-0.5">
                <button type="button" aria-label="width full" className={`rounded px-2 py-0.5 text-xs ${draftWidth === "full" ? "bg-accent" : ""}`} onClick={() => setDraftWidth("full")}>Full</button>
                <button type="button" aria-label="width fixed" className={`rounded px-2 py-0.5 text-xs ${draftWidth === "fixed" ? "bg-accent" : ""}`} onClick={() => setDraftWidth("fixed")}>Fixed</button>
              </div>
```
Wrap the dashboard content in a centered container when fixed. The root canvas `<div className="… bg-muted/30 p-4">` currently holds the header, filter bar, tabs, and grid directly. Wrap **all of that inner content** in:
```tsx
      <div className={effectiveWidth === "fixed" ? "mx-auto w-full max-w-[1200px]" : ""}>
        {/* existing header + DashboardFilterBar + DashboardTabBar + grid + dialogs */}
      </div>
```
(The outer canvas keeps `bg-muted/30 p-4` full-bleed; only the inner column is constrained.)

- [ ] **Step 5: Run it — GREEN.** Run (from `frontend/`): `npx vitest run src/routes/explore/__tests__/DashboardView.width.test.tsx`. Expected PASS (3). Also run the other `DashboardView.*` suites — still green.

- [ ] **Step 6: Frontend sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/api/dashboards.ts frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.width.test.tsx
git commit -m "feat(dashboards): edit-mode Width toggle + fixed-width view container

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Dark-mode verify + fix (live, conditional)

**Files:** depends on findings (likely none, or small token swaps in dashboard components).

This is a verify-and-fix pass, not pure TDD — the dashboard chrome already uses theme tokens, so this confirms it and patches any gap.

- [ ] **Step 1: Start the stack** — frontend `npm run dev`, backend `target/debug/egret serve` (after the migration re-embed). Log in `admin@egret.local` / `Admin12345!`, ws `eng`.

- [ ] **Step 2: Drive the seeded PARITY dashboard in dark mode** (Playwright MCP). Open `/explore/eng/dashboards/56f83b64-b7a8-488f-8bf4-cec47af7b703`, click the header theme toggle (moon icon) to enter dark mode, and inspect: canvas, cards (incl. inline titles), filter pills (default + selected `bg-primary/5` state + open popover with search/checkbox list), tabs (active brand underline), edit-mode controls (Add chart, On-click, Width toggle), drill menu + records dialog, the "Set the required filters" / "No cards on this tab" placeholders. Watch for **0 console errors**.

- [ ] **Step 3: Fix any gaps** — for each element that's wrong in dark mode (invisible border, low-contrast text, a hardcoded colour), swap the offending class to a theme token (`bg-card`, `bg-popover`, `text-foreground`, `text-muted-foreground`, `border`, `bg-muted`). Keep each fix minimal. Likely files: `FilterPill.tsx`, `DashboardFilterBar.tsx`, `DashboardTabBar.tsx`, `DashboardCardView.tsx`, `DashboardView.tsx`. If a fix changes a component with unit tests, run them.

- [ ] **Step 4: If any fixes were made — sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add <the files you changed>
git commit -m "style(dashboards): dark-mode polish for dashboard chrome

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
If **no** gaps were found, record that in the Task 4 report (no commit needed).

---

## Task 4: Final live verification + PR

**Files:** none (manual verification).

- [ ] **Step 1: Width round-trip (Playwright MCP)** — on the seeded PARITY dashboard: Edit → set Width = Fixed → Save → reload → the content is centered at ~1200px (like Metabase). Edit → Full → Save → reload → full-bleed. **0 console errors.**

- [ ] **Step 2: Dark-mode side-by-side** — compare the dashboard in dark mode against Metabase `localhost:3001/dashboard/1` (toggle Metabase's theme if available); confirm egret reads correctly.

- [ ] **Step 3: Full sweeps** — frontend `npm run typecheck && npm run lint && npm run build && npx vitest run`; backend `cargo fmt --all --check && cargo test -p egret-bi`. All green.

- [ ] **Step 4: Push + PR**
```bash
git push -u origin feat/dashboards-tier2a
gh pr create --base main --title "feat(dashboards): Tier 2a — fixed/full width + dark-mode polish" --body "<summary, changes, testing, 🤖 footer>"
```
Then await the user to merge.

---

## Notes for the implementer

- **Rules of Hooks:** `draftWidth` `useState` goes ABOVE the `if (isLoading || !dash) return …` early return in `DashboardView`.
- **Existing dashboards** default to `width = 'full'` (migration default) — no behaviour change for them, and tabless/empty dashboards are unaffected by the fixed wrapper.
- **`cargo fmt`** is a CI Format step — run `cargo fmt --all --check` before the backend commit or CI fails (lesson from PR #29).
