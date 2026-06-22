# Phase 3d-C — Dashboard filters / parameters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Metabase-style dashboard parameters/filters — a filter widget bar whose values are mapped to each card's columns and injected into every card's query, plus `{{slug}}` interpolation in text/heading cards.

**Architecture:** Parameters persist on the dashboard (migration `0016`, jsonb). Pure helpers turn a parameter value + a per-card column mapping into extra `ChartFilter`s and inject them into the card's `QuerySpec`; since `useChartData` keys on the spec JSON, cards auto-refetch when a value changes. Values URL-sync by slug. No engine change.

**Tech Stack:** Rust (egret-bi, sqlx) + React 18 / TS / Vite / Vitest, react-router `useSearchParams`, existing `useChartData` / `DashboardCardView` / `ChartFilter`/`QuerySpec`.

**Spec:** `docs/en/specs/2026-06-22-phase-3d-c-dashboard-filters-design.md`

---

## Conventions

- Frontend commands from `frontend/`. Full sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Configured git identity, never `--author`. Footer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Stage explicit paths. Branch `feat/dashboards-phase-c` (checked out).
- **sqlx-migration-staleness gotcha:** after adding migration `0016`, a running `cargo run serve` keeps the stale embedded migrator — `touch crates/egret-metastore/src/lib.rs` before rebuild so `sqlx::migrate!` re-reads the dir.

## Existing types you build on (verified)

- `@/api/charts`: `ChartFilter = { col: string; op: FilterOp; value?: unknown }` with `FilterOp = "eq"|"ne"|"gt"|"ge"|"lt"|"le"|"like"|"in"|"not_in"|"is_null"|"is_not_null"`. `QuerySpec.filters: ChartFilter[]`. `Chart = { id, dataset_id, name, viz_type, params: ChartParams }`, `ChartParams.spec: QuerySpec`. Backend renders `like` as `col LIKE <value>` verbatim (so add `%` yourself) and `in` as `col IN (...)`.
- `@/api/dashboards`: `DashboardDetail`, `DashboardCard { …, options: Record<string,unknown> }`, `useDashboard`, `useDashboardMutations`.
- `DashboardView.tsx` (Phase A/B): `draftCards` state, `updateCardOptions(cardId, patch)`, whole-layout PUT in `onSave` (currently sends `{ id, name, description, cards }`), `useDashboardViewControls`, renders `<DashboardCardView ws card chart refreshNonce />` per grid child.

## File structure

```
migrations/0016_dashboard_parameters.sql                       NEW
crates/egret-bi/src/{dashboard.rs,model.rs,routes.rs}          MODIFY (thread parameters)
crates/egret-bi/tests/dashboard_repo.rs                        MODIFY (params round-trip)
frontend/src/api/dashboards.ts                                 MODIFY (DashboardParameter + parameters)
frontend/src/components/chart/dashboard/
  params.ts                                                    NEW (paramToFilters, applyParamsToChart, interpolateParams, slugify)
  __tests__/params.test.ts                                     NEW
  DashboardFilterBar.tsx                                       NEW
  ParameterEditor.tsx                                          NEW (add/edit one parameter)
  __tests__/DashboardFilterBar.test.tsx                        NEW
frontend/src/api/
  useDashboardParams.ts                                        NEW (URL-synced values)
  useColumnValues.ts                                           NEW (distinct values)
  __tests__/useDashboardParams.test.tsx                        NEW
frontend/src/routes/explore/
  DashboardView.tsx                                            MODIFY (params state, filter bar, mapping, injection)
  __tests__/DashboardView.test.tsx                             MODIFY
frontend/src/components/chart/dashboard/DashboardCardView.tsx  MODIFY ({{slug}} interpolation)
```

---

## Task 1: Backend — persist `parameters` on the dashboard

**Files:** `migrations/0016_dashboard_parameters.sql` (new); `crates/egret-bi/src/{dashboard.rs,model.rs,routes.rs}`; `crates/egret-bi/tests/dashboard_repo.rs`.

- [ ] **Step 1: Migration**

Create `migrations/0016_dashboard_parameters.sql`:
```sql
ALTER TABLE dashboards ADD COLUMN parameters jsonb NOT NULL DEFAULT '[]';
```

- [ ] **Step 2: Thread through `dashboard.rs`**

Add to the `Dashboard` struct: `pub parameters: serde_json::Value,`. Update `COLS`:
```rust
const COLS: &str = "id, workspace_id, name, description, parameters, created_by, updated_at";
```
Rename `update_meta` → `update` taking parameters:
```rust
pub async fn update(
    &self,
    id: Uuid,
    name: &str,
    description: &str,
    parameters: &serde_json::Value,
) -> Result<(), BiError> {
    sqlx::query("UPDATE dashboards SET name=$2, description=$3, parameters=$4, updated_at=now() WHERE id=$1")
        .bind(id).bind(name).bind(description).bind(parameters)
        .execute(&self.db.pool).await.map_err(map_conflict_err)?;
    Ok(())
}
```
`create` and `get` need no body change (COLS now includes `parameters`; new rows default to `'[]'`). The `DashboardWithCount` flatten in `list_by_workspace` still works (it selects `COLS`); the list path doesn't expose parameters (summary DTO unchanged).

- [ ] **Step 3: Thread through `model.rs`**

Add `pub parameters: serde_json::Value,` to `DashboardDetailDto`. Add to `UpdateDashboard`:
```rust
#[serde(default = "empty_arr")]
pub parameters: serde_json::Value,
```
and a helper near `empty_obj`:
```rust
fn empty_arr() -> serde_json::Value { serde_json::json!([]) }
```
Update `dashboard_detail_dto` to pass it through:
```rust
pub fn dashboard_detail_dto(d: Dashboard, cards: Vec<DashboardCard>) -> DashboardDetailDto {
    DashboardDetailDto {
        id: d.id.to_string(),
        name: d.name,
        description: d.description,
        parameters: d.parameters,
        created_by: d.created_by.map(|u| u.to_string()),
        updated_at: d.updated_at,
        cards: cards.into_iter().map(dashboard_card_dto).collect(),
    }
}
```

- [ ] **Step 4: Thread through the `db_update` route** (`routes.rs`)

Replace the `update_meta` call:
```rust
st.dashboards
    .update(id, &b.name, &b.description, &b.parameters)
    .await
    .map_err(BiApiError::from)?;
```

- [ ] **Step 5: Repo test** — add to `crates/egret-bi/tests/dashboard_repo.rs`, inside the existing DB-gated test (after the metadata-update assertions):
```rust
    // parameters round-trip
    let params = serde_json::json!([{ "id": "p1", "name": "Region", "slug": "region", "type": "text", "operator": "is" }]);
    f.repo.update(d.id, "Ops v2", "", &params).await.unwrap();
    let got = f.repo.get(d.id).await.unwrap().unwrap();
    assert_eq!(got.parameters, params);
```
(If the test uses `update_meta` anywhere, switch it to `update(.., &serde_json::json!([]))`.)

- [ ] **Step 6: Build + test + fmt + clippy**

Run: `cargo build -p egret-bin && cargo test -p egret-bi && cargo clippy -p egret-bi --all-targets && cargo fmt -p egret-bi -- --check`
Expected: all clean. (DB-gated test self-skips without `DATABASE_URL`.)

- [ ] **Step 7: Commit**
```bash
git add migrations/0016_dashboard_parameters.sql crates/egret-bi/src/dashboard.rs crates/egret-bi/src/model.rs crates/egret-bi/src/routes.rs crates/egret-bi/tests/dashboard_repo.rs
git commit -m "feat(dashboard): persist parameters on the dashboard (migration 0016)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `api/dashboards.ts` — parameter types + payload

**Files:** `frontend/src/api/dashboards.ts`

- [ ] **Step 1: Add types + thread `parameters`**

Add the parameter types and extend `DashboardDetail` + the update payload:
```ts
export type ParamType = "text" | "number" | "date" | "id" | "category";
export type ParamOperator =
  | "is" | "is-not" | "contains" | "starts-with" | "ends-with"   // text
  | "=" | "!=" | "between" | ">=" | "<="                         // number
  | "range" | "relative" | "single";                            // date
export interface DashboardParameter {
  id: string;
  name: string;
  slug: string;
  type: ParamType;
  operator: ParamOperator;
  default?: unknown;
  required?: boolean;
}
```
Add `parameters: DashboardParameter[]` to `DashboardDetail`. Change the `update` mutation's payload type to include it:
```ts
const update = useMutation({
  mutationFn: ({ id, ...b }: { id: string; name: string; description?: string; parameters: DashboardParameter[]; cards: CardInput[] }) =>
    apiJson<DashboardDetail>(`/ws/${ws}/dashboards/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
  onSuccess: (d) => invalidate(d.id),
});
```

- [ ] **Step 2: Typecheck + commit**

Run `npm run typecheck` (note: `DashboardView.onSave` will now type-error for a missing `parameters` — that's fixed in Task 8; for THIS commit, temporarily pass `parameters: []` in the existing `onSave` call so typecheck stays green, with a `// TODO(Task 8)` note). Then:
```bash
git add frontend/src/api/dashboards.ts frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(api): dashboard parameter types + update payload

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Pure helpers — `paramToFilters`, `applyParamsToChart`, `interpolateParams`, `slugify`

**Files:** Create `frontend/src/components/chart/dashboard/params.ts` + `__tests__/params.test.ts`.

- [ ] **Step 1: Write the failing test (verbatim)**

```ts
// frontend/src/components/chart/dashboard/__tests__/params.test.ts
import { describe, it, expect } from "vitest";
import { paramToFilters, applyParamsToChart, interpolateParams, slugify } from "../params";
import type { DashboardParameter } from "@/api/dashboards";
import type { Chart } from "@/api/charts";

const p = (over: Partial<DashboardParameter>): DashboardParameter => ({
  id: "p1", name: "P", slug: "p", type: "text", operator: "is", ...over,
});

describe("slugify", () => {
  it("kebabs and uniquifies", () => {
    expect(slugify("Order Region", [])).toBe("order_region");
    expect(slugify("Order Region", ["order_region"])).toBe("order_region_2");
  });
});

describe("paramToFilters", () => {
  it("text operators", () => {
    expect(paramToFilters(p({ operator: "is" }), "EU", "region")).toEqual([{ col: "region", op: "eq", value: "EU" }]);
    expect(paramToFilters(p({ operator: "is-not" }), "EU", "region")).toEqual([{ col: "region", op: "ne", value: "EU" }]);
    expect(paramToFilters(p({ operator: "contains" }), "EU", "region")).toEqual([{ col: "region", op: "like", value: "%EU%" }]);
    expect(paramToFilters(p({ operator: "starts-with" }), "EU", "region")).toEqual([{ col: "region", op: "like", value: "EU%" }]);
    expect(paramToFilters(p({ operator: "ends-with" }), "EU", "region")).toEqual([{ col: "region", op: "like", value: "%EU" }]);
  });
  it("number operators incl. between → two filters", () => {
    expect(paramToFilters(p({ type: "number", operator: "=" }), 5, "n")).toEqual([{ col: "n", op: "eq", value: 5 }]);
    expect(paramToFilters(p({ type: "number", operator: ">=" }), 5, "n")).toEqual([{ col: "n", op: "ge", value: 5 }]);
    expect(paramToFilters(p({ type: "number", operator: "between" }), [2, 8], "n")).toEqual([{ col: "n", op: "ge", value: 2 }, { col: "n", op: "le", value: 8 }]);
  });
  it("date range and single", () => {
    expect(paramToFilters(p({ type: "date", operator: "range" }), ["2026-01-01", "2026-02-01"], "d")).toEqual([{ col: "d", op: "ge", value: "2026-01-01" }, { col: "d", op: "le", value: "2026-02-01" }]);
    expect(paramToFilters(p({ type: "date", operator: "single" }), "2026-01-01", "d")).toEqual([{ col: "d", op: "eq", value: "2026-01-01" }]);
  });
  it("date relative computes a [start,end] range from a fixed now", () => {
    const now = new Date("2026-01-31T00:00:00Z");
    const out = paramToFilters(p({ type: "date", operator: "relative" }), "last7", "d", now);
    expect(out).toHaveLength(2);
    expect(out[0]).toMatchObject({ col: "d", op: "ge" });
    expect(out[1]).toMatchObject({ col: "d", op: "le", value: "2026-01-31" });
    expect(out[0].value).toBe("2026-01-24");
  });
  it("category multi-select → in", () => {
    expect(paramToFilters(p({ type: "category", operator: "is" }), ["EU", "US"], "region")).toEqual([{ col: "region", op: "in", value: ["EU", "US"] }]);
  });
  it("id → eq", () => {
    expect(paramToFilters(p({ type: "id", operator: "is" }), 42, "id")).toEqual([{ col: "id", op: "eq", value: 42 }]);
  });
  it("blank / empty values produce no filters", () => {
    expect(paramToFilters(p({}), "", "region")).toEqual([]);
    expect(paramToFilters(p({ type: "category" }), [], "region")).toEqual([]);
    expect(paramToFilters(p({ type: "number", operator: "between" }), [null, null], "n")).toEqual([]);
  });
});

const chart = (): Chart => ({
  id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "",
  params: { spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [{ col: "region", op: "is_not_null" }], orderby: [], row_limit: 1000, rollup_groups: [] } },
  created_by: null, updated_at: "",
});

describe("applyParamsToChart", () => {
  it("injects mapped param filters and keeps existing ones", () => {
    const params: DashboardParameter[] = [p({ id: "pr", slug: "region", operator: "is" })];
    const out = applyParamsToChart(chart(), params, { region: "EU" }, { pr: "region" });
    expect(out.params.spec.filters).toEqual([
      { col: "region", op: "is_not_null" },
      { col: "region", op: "eq", value: "EU" },
    ]);
  });
  it("ignores params with no mapping for this card", () => {
    const params: DashboardParameter[] = [p({ id: "pr", slug: "region" })];
    const out = applyParamsToChart(chart(), params, { region: "EU" }, {}); // no mapping
    expect(out.params.spec.filters).toEqual([{ col: "region", op: "is_not_null" }]);
    expect(out).toBe(out); // returns a chart object
  });
});

describe("interpolateParams", () => {
  it("substitutes {{slug}} with the value, blanks when unset", () => {
    const params: DashboardParameter[] = [p({ slug: "region" })];
    expect(interpolateParams("Region: {{region}}", params, { region: "EU" })).toBe("Region: EU");
    expect(interpolateParams("Region: {{region}}", params, {})).toBe("Region: ");
    expect(interpolateParams("no tokens", params, {})).toBe("no tokens");
  });
});
```

- [ ] **Step 2: Run → FAIL** (`npx vitest run src/components/chart/dashboard/__tests__/params.test.ts` → module not found)

- [ ] **Step 3: Implement `params.ts`**

```ts
// frontend/src/components/chart/dashboard/params.ts
import type { ChartFilter, Chart } from "@/api/charts";
import type { DashboardParameter } from "@/api/dashboards";

export function slugify(name: string, existing: string[]): string {
  const base = name.trim().toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "") || "param";
  if (!existing.includes(base)) return base;
  let i = 2;
  while (existing.includes(`${base}_${i}`)) i++;
  return `${base}_${i}`;
}

const isBlank = (v: unknown) => v === undefined || v === null || v === "";

/** Days for a relative preset key (last7/last30/last90/last365). */
const RELATIVE_DAYS: Record<string, number> = { last7: 7, last30: 30, last90: 90, last365: 365 };
const iso = (d: Date) => d.toISOString().slice(0, 10);

/** Turn a parameter value (for one card's mapped column) into 0..2 ChartFilters. */
export function paramToFilters(
  param: DashboardParameter,
  value: unknown,
  column: string,
  now: Date = new Date(),
): ChartFilter[] {
  const f = (op: ChartFilter["op"], v: unknown): ChartFilter => ({ col: column, op, value: v });
  switch (param.type) {
    case "text": {
      if (isBlank(value)) return [];
      const v = String(value);
      switch (param.operator) {
        case "is": return [f("eq", v)];
        case "is-not": return [f("ne", v)];
        case "contains": return [f("like", `%${v}%`)];
        case "starts-with": return [f("like", `${v}%`)];
        case "ends-with": return [f("like", `%${v}`)];
        default: return [f("eq", v)];
      }
    }
    case "number": {
      if (param.operator === "between") {
        const [min, max] = Array.isArray(value) ? value : [undefined, undefined];
        const out: ChartFilter[] = [];
        if (!isBlank(min)) out.push(f("ge", Number(min)));
        if (!isBlank(max)) out.push(f("le", Number(max)));
        return out;
      }
      if (isBlank(value)) return [];
      const n = Number(value);
      const op = ({ "=": "eq", "!=": "ne", ">=": "ge", "<=": "le" } as const)[param.operator as "=" | "!=" | ">=" | "<="] ?? "eq";
      return [f(op, n)];
    }
    case "date": {
      if (param.operator === "single") return isBlank(value) ? [] : [f("eq", String(value))];
      if (param.operator === "range") {
        const [start, end] = Array.isArray(value) ? value : [undefined, undefined];
        const out: ChartFilter[] = [];
        if (!isBlank(start)) out.push(f("ge", String(start)));
        if (!isBlank(end)) out.push(f("le", String(end)));
        return out;
      }
      // relative
      const days = RELATIVE_DAYS[String(value)];
      if (!days) return [];
      const end = new Date(now);
      const start = new Date(now);
      start.setUTCDate(start.getUTCDate() - days);
      return [f("ge", iso(start)), f("le", iso(end))];
    }
    case "id":
      return isBlank(value) ? [] : [f("eq", value)];
    case "category": {
      const arr = Array.isArray(value) ? value.filter((v) => !isBlank(v)) : isBlank(value) ? [] : [value];
      return arr.length ? [f("in", arr)] : [];
    }
    default:
      return [];
  }
}

/** Return a chart whose spec.filters include filters from each mapped parameter. */
export function applyParamsToChart(
  chart: Chart,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
  mappings: Record<string, string>,
  now: Date = new Date(),
): Chart {
  const extra: ChartFilter[] = [];
  for (const param of parameters) {
    const column = mappings[param.id];
    if (!column) continue;
    extra.push(...paramToFilters(param, values[param.slug], column, now));
  }
  if (extra.length === 0) return chart;
  return {
    ...chart,
    params: { ...chart.params, spec: { ...chart.params.spec, filters: [...chart.params.spec.filters, ...extra] } },
  };
}

/** Replace {{slug}} tokens in text with the current parameter value (blank when unset). */
export function interpolateParams(
  text: string,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
): string {
  const bySlug = new Map(parameters.map((p) => [p.slug, p]));
  return text.replace(/\{\{\s*([\w-]+)\s*\}\}/g, (_m, slug) => {
    if (!bySlug.has(slug)) return _m;
    const v = values[slug];
    return v == null ? "" : Array.isArray(v) ? v.join(", ") : String(v);
  });
}
```

- [ ] **Step 4: Run → PASS**, then `npm run typecheck`.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/chart/dashboard/params.ts frontend/src/components/chart/dashboard/__tests__/params.test.ts
git commit -m "feat(dashboard): param→ChartFilter mapping + chart injection + {{slug}} helpers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `useDashboardParams` — URL-synced parameter values

**Files:** Create `frontend/src/api/useDashboardParams.ts` + `__tests__/useDashboardParams.test.tsx`.

- [ ] **Step 1: Failing test**

```tsx
// frontend/src/api/__tests__/useDashboardParams.test.tsx
import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type { ReactNode } from "react";
import { useDashboardParams } from "../useDashboardParams";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "text", operator: "is", default: "EU" },
  { id: "p2", name: "Tags", slug: "tags", type: "category", operator: "is" },
];
const wrap = (initial: string) => ({ children }: { children: ReactNode }) =>
  <MemoryRouter initialEntries={[initial]}>{children}</MemoryRouter>;

describe("useDashboardParams", () => {
  it("seeds from default when the URL has no value", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    expect(result.current.values.region).toBe("EU");
  });
  it("reads a scalar value from the URL", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d?region=US") });
    expect(result.current.values.region).toBe("US");
  });
  it("reads a JSON array value (category) from the URL", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap(`/d?tags=${encodeURIComponent('["a","b"]')}`) });
    expect(result.current.values.tags).toEqual(["a", "b"]);
  });
  it("setValue writes to the URL", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    act(() => result.current.setValue("region", "APAC"));
    expect(result.current.values.region).toBe("APAC");
  });
});
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement**

```ts
// frontend/src/api/useDashboardParams.ts
import { useSearchParams } from "react-router-dom";
import type { DashboardParameter } from "@/api/dashboards";

export interface DashboardParamsState {
  values: Record<string, unknown>;
  setValue: (slug: string, value: unknown) => void;
}

const ARRAY_TYPES = new Set(["category"]);

/** Read/write each parameter's value in the URL by slug. Arrays/ranges are JSON-encoded. */
export function useDashboardParams(parameters: DashboardParameter[]): DashboardParamsState {
  const [params, setParams] = useSearchParams();
  const values: Record<string, unknown> = {};
  for (const p of parameters) {
    const raw = params.get(p.slug);
    if (raw == null) {
      if (p.default !== undefined) values[p.slug] = p.default;
      continue;
    }
    if (ARRAY_TYPES.has(p.type) || p.operator === "between" || p.operator === "range") {
      try { values[p.slug] = JSON.parse(raw); } catch { values[p.slug] = raw; }
    } else {
      values[p.slug] = raw;
    }
  }
  const setValue = (slug: string, value: unknown) =>
    setParams((prev) => {
      const n = new URLSearchParams(prev);
      const blank = value === undefined || value === null || value === "" || (Array.isArray(value) && value.length === 0);
      if (blank) n.delete(slug);
      else n.set(slug, typeof value === "object" ? JSON.stringify(value) : String(value));
      return n;
    }, { replace: true });
  return { values, setValue };
}
```

- [ ] **Step 4: Run → PASS**, typecheck.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/api/useDashboardParams.ts frontend/src/api/__tests__/useDashboardParams.test.tsx
git commit -m "feat(api): useDashboardParams — URL-synced filter values

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `useColumnValues` — distinct values for category/id dropdowns

**Files:** Create `frontend/src/api/useColumnValues.ts` + `__tests__/useColumnValues.test.tsx`.

This reuses the build→submit→poll pipeline with a dimension-only spec. Model it on `useChartData` (read `frontend/src/api/useChartData.ts` first).

- [ ] **Step 1: Failing test**

```tsx
// frontend/src/api/__tests__/useColumnValues.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

const apiJson = vi.fn();
vi.mock("@/api/client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: vi.fn() }));

import { useColumnValues } from "../useColumnValues";

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  apiJson.mockReset();
  apiJson.mockImplementation((url: string, opts?: { method?: string }) => {
    if (url.endsWith("/charts/data")) return Promise.resolve({ sql: "SELECT 1", database_id: "db1" });
    if (url.endsWith("/queries") && opts?.method === "POST") return Promise.resolve({ id: "q1", status: "finished" });
    if (url.includes("/queries/q1/results")) return Promise.resolve({ columns: [{ name: "region", type: "varchar" }], rows: [["EU"], ["US"], ["EU"]], page: 0, page_size: 1000, row_count: 3, truncated: false });
    if (url.includes("/queries/q1")) return Promise.resolve({ id: "q1", database_id: "db1", status: "finished", columns: null, preview: null, row_count: 3, truncated: false, error: null });
    return Promise.reject(new Error("unexpected " + url));
  });
});

describe("useColumnValues", () => {
  it("returns distinct column values", async () => {
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "region", true), { wrapper });
    await waitFor(() => expect(result.current.values.length).toBe(2));
    expect(result.current.values).toEqual(["EU", "US"]);
  });
  it("is inert when disabled", () => {
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "region", false), { wrapper });
    expect(result.current.values).toEqual([]);
    expect(apiJson).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** (mirror `useChartData`'s build→submit→poll; build a dimension-only spec)

```ts
// frontend/src/api/useColumnValues.ts
import { useEffect, useState } from "react";
import { useBuildChartData } from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";

export interface ColumnValues { values: string[]; isLoading: boolean; error: string | null }

/** Distinct values of one column of a dataset, for filter dropdowns. */
export function useColumnValues(ws: string, datasetId: string | null, column: string | null, enabled: boolean): ColumnValues {
  const build = useBuildChartData(ws);
  const submit = useSubmitQuery(ws);
  const [queryId, setQueryId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const status = useQueryStatus(ws, queryId);
  const finished = status.data?.status === "finished";
  const results = useResults(ws, queryId, 0, finished);

  const key = enabled && datasetId && column ? `${datasetId}:${column}` : "";
  useEffect(() => {
    let cancelled = false;
    if (!key || !datasetId || !column) return;
    setQueryId(null); setError(null);
    (async () => {
      try {
        const spec = { metrics: [], dimensions: [column], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] };
        const { sql, database_id } = await build.mutateAsync({ dataset_id: datasetId, query_spec: spec });
        if (cancelled) return;
        const q = await submit.mutateAsync({ database_id, sql, limit: 1000 });
        if (!cancelled) setQueryId(q.id);
      } catch (e) { if (!cancelled) setError(String(e)); }
    })();
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ws, key]);

  const raw = results.data?.rows ?? [];
  const values = Array.from(new Set(raw.map((r) => r[0]).filter((v) => v != null).map(String)));
  return { values, isLoading: enabled && !error && !(finished && results.data), error };
}
```

- [ ] **Step 4: Run → PASS**, typecheck/lint (the `exhaustive-deps` disable mirrors useChartData; if lint flags it as unused, remove it).

- [ ] **Step 5: Commit**
```bash
git add frontend/src/api/useColumnValues.ts frontend/src/api/__tests__/useColumnValues.test.tsx
git commit -m "feat(api): useColumnValues — distinct values for filter dropdowns

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `DashboardFilterBar` — widgets per parameter type

**Files:** Create `frontend/src/components/chart/dashboard/DashboardFilterBar.tsx` + `__tests__/DashboardFilterBar.test.tsx`.

A presentational bar: one widget per parameter, value + onChange passed in. Distinct-value dropdowns (category/id) are fed `options` from the parent (so the bar stays pure/testable); the parent uses `useColumnValues`.

- [ ] **Step 1: Failing test**

```tsx
// frontend/src/components/chart/dashboard/__tests__/DashboardFilterBar.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DashboardFilterBar } from "../DashboardFilterBar";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "text", operator: "contains" },
  { id: "p2", name: "Amount", slug: "amount", type: "number", operator: "between" },
  { id: "p3", name: "Status", slug: "status", type: "category", operator: "is" },
];

describe("DashboardFilterBar", () => {
  it("renders a labelled widget per parameter and reports text changes", () => {
    const onChange = vi.fn();
    render(<DashboardFilterBar parameters={params} values={{}} onChange={onChange} optionsBySlug={{ status: ["new", "paid"] }} />);
    expect(screen.getByText("Region")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Region"), { target: { value: "EU" } });
    expect(onChange).toHaveBeenCalledWith("region", "EU");
  });
  it("number between renders two inputs", () => {
    render(<DashboardFilterBar parameters={params} values={{}} onChange={vi.fn()} optionsBySlug={{}} />);
    expect(screen.getByLabelText("Amount min")).toBeInTheDocument();
    expect(screen.getByLabelText("Amount max")).toBeInTheDocument();
  });
  it("category renders a multi-select listbox with the provided options", () => {
    render(<DashboardFilterBar parameters={params} values={{}} onChange={vi.fn()} optionsBySlug={{ status: ["new", "paid"] }} />);
    expect(screen.getByLabelText("Status")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "new" })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** — a widget switch per parameter. Use plain accessible controls (label + input/select); keep it dependency-light. Key requirements the test pins: each widget's control has `aria-label={param.name}` (text/number-single/date-single/category/id) or `"{name} min"`/`"{name} max"` (between/range), category is a multi `<select aria-label={name}>` with `<option>` per value.

```tsx
// frontend/src/components/chart/dashboard/DashboardFilterBar.tsx
import type { DashboardParameter } from "@/api/dashboards";

interface Props {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  onChange: (slug: string, value: unknown) => void;
  optionsBySlug: Record<string, string[]>;   // distinct values for category/id dropdowns
}

const RELATIVE_OPTS = [
  { v: "last7", label: "Last 7 days" }, { v: "last30", label: "Last 30 days" },
  { v: "last90", label: "Last 90 days" }, { v: "last365", label: "Last 365 days" },
];

export function DashboardFilterBar({ parameters, values, onChange, optionsBySlug }: Props) {
  if (parameters.length === 0) return null;
  return (
    <div className="mb-3 flex flex-wrap items-end gap-3 rounded-md border p-2">
      {parameters.map((p) => (
        <label key={p.id} className="flex flex-col gap-0.5 text-xs text-muted-foreground">
          <span>{p.name}{p.required ? " *" : ""}</span>
          <Widget p={p} value={values[p.slug]} onChange={(v) => onChange(p.slug, v)} options={optionsBySlug[p.slug] ?? []} />
        </label>
      ))}
    </div>
  );
}

function Widget({ p, value, onChange, options }: { p: DashboardParameter; value: unknown; onChange: (v: unknown) => void; options: string[] }) {
  const cls = "h-8 rounded border bg-background px-1 text-sm";
  if (p.type === "category") {
    const sel = Array.isArray(value) ? (value as string[]) : [];
    return (
      <select multiple aria-label={p.name} className={cls} value={sel}
        onChange={(e) => onChange(Array.from(e.target.selectedOptions).map((o) => o.value))}>
        {options.map((o) => <option key={o} value={o}>{o}</option>)}
      </select>
    );
  }
  if (p.type === "id") {
    return options.length
      ? <select aria-label={p.name} className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)}>
          <option value="">—</option>{options.map((o) => <option key={o} value={o}>{o}</option>)}
        </select>
      : <input aria-label={p.name} className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }
  if (p.type === "number" && p.operator === "between") {
    const [min, max] = Array.isArray(value) ? value : ["", ""];
    return (
      <span className="flex gap-1">
        <input aria-label={`${p.name} min`} type="number" className={cls + " w-20"} value={min ?? ""} onChange={(e) => onChange([e.target.value === "" ? null : Number(e.target.value), max ?? null])} />
        <input aria-label={`${p.name} max`} type="number" className={cls + " w-20"} value={max ?? ""} onChange={(e) => onChange([min ?? null, e.target.value === "" ? null : Number(e.target.value)])} />
      </span>
    );
  }
  if (p.type === "number") {
    return <input aria-label={p.name} type="number" className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value === "" ? null : Number(e.target.value))} />;
  }
  if (p.type === "date" && p.operator === "range") {
    const [start, end] = Array.isArray(value) ? value : ["", ""];
    return (
      <span className="flex gap-1">
        <input aria-label={`${p.name} min`} type="date" className={cls} value={start ?? ""} onChange={(e) => onChange([e.target.value, end ?? ""])} />
        <input aria-label={`${p.name} max`} type="date" className={cls} value={end ?? ""} onChange={(e) => onChange([start ?? "", e.target.value])} />
      </span>
    );
  }
  if (p.type === "date" && p.operator === "relative") {
    return <select aria-label={p.name} className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)}>
      <option value="">—</option>{RELATIVE_OPTS.map((o) => <option key={o.v} value={o.v}>{o.label}</option>)}
    </select>;
  }
  if (p.type === "date") { // single
    return <input aria-label={p.name} type="date" className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }
  // text
  return <input aria-label={p.name} className={cls} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
}
```

- [ ] **Step 4: Run → PASS**, typecheck/lint.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/chart/dashboard/DashboardFilterBar.tsx frontend/src/components/chart/dashboard/__tests__/DashboardFilterBar.test.tsx
git commit -m "feat(dashboard): filter widget bar (text/number/date/id/category)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: `ParameterEditor` — add/edit one parameter (edit mode)

**Files:** Create `frontend/src/components/chart/dashboard/ParameterEditor.tsx` + a test in `DashboardFilterBar.test.tsx` (or a new `ParameterEditor.test.tsx`).

A small editor row: name input, type select, operator select (shown for text/number/date), required checkbox, delete button. Emits a patched `DashboardParameter`.

- [ ] **Step 1: Failing test** (`__tests__/ParameterEditor.test.tsx`)

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ParameterEditor } from "../ParameterEditor";
import type { DashboardParameter } from "@/api/dashboards";

const param: DashboardParameter = { id: "p1", name: "Region", slug: "region", type: "text", operator: "is" };

describe("ParameterEditor", () => {
  it("edits the name", () => {
    const onChange = vi.fn();
    render(<ParameterEditor param={param} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.change(screen.getByLabelText(/parameter name/i), { target: { value: "Area" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ name: "Area" }));
  });
  it("changing type resets to that type's first operator", () => {
    const onChange = vi.fn();
    render(<ParameterEditor param={param} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.change(screen.getByLabelText(/parameter type/i), { target: { value: "number" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ type: "number", operator: "=" }));
  });
  it("deletes", () => {
    const onDelete = vi.fn();
    render(<ParameterEditor param={param} onChange={vi.fn()} onDelete={onDelete} />);
    fireEvent.click(screen.getByRole("button", { name: /delete parameter/i }));
    expect(onDelete).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** — define the operator lists per type and the editor.

```tsx
// frontend/src/components/chart/dashboard/ParameterEditor.tsx
import type { DashboardParameter, ParamType, ParamOperator } from "@/api/dashboards";

export const OPERATORS: Record<ParamType, ParamOperator[]> = {
  text: ["is", "is-not", "contains", "starts-with", "ends-with"],
  number: ["=", "!=", "between", ">=", "<="],
  date: ["range", "relative", "single"],
  id: ["is"],
  category: ["is"],
};

export function ParameterEditor({ param, onChange, onDelete }: {
  param: DashboardParameter;
  onChange: (next: DashboardParameter) => void;
  onDelete: () => void;
}) {
  const cls = "h-7 rounded border bg-background px-1 text-xs";
  const showOp = param.type === "text" || param.type === "number" || param.type === "date";
  return (
    <div className="flex items-center gap-1">
      <input aria-label="parameter name" className={cls} value={param.name}
        onChange={(e) => onChange({ ...param, name: e.target.value })} />
      <select aria-label="parameter type" className={cls} value={param.type}
        onChange={(e) => { const t = e.target.value as ParamType; onChange({ ...param, type: t, operator: OPERATORS[t][0] }); }}>
        {(Object.keys(OPERATORS) as ParamType[]).map((t) => <option key={t} value={t}>{t}</option>)}
      </select>
      {showOp && (
        <select aria-label="parameter operator" className={cls} value={param.operator}
          onChange={(e) => onChange({ ...param, operator: e.target.value as ParamOperator })}>
          {OPERATORS[param.type].map((o) => <option key={o} value={o}>{o}</option>)}
        </select>
      )}
      <label className="flex items-center gap-1 text-xs"><input type="checkbox" checked={!!param.required}
        onChange={(e) => onChange({ ...param, required: e.target.checked })} aria-label="parameter required" /> req</label>
      <button type="button" aria-label="delete parameter" className="text-xs text-destructive" onClick={onDelete}>✕</button>
    </div>
  );
}
```

- [ ] **Step 4: Run → PASS**, typecheck/lint.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/chart/dashboard/ParameterEditor.tsx frontend/src/components/chart/dashboard/__tests__/ParameterEditor.test.tsx
git commit -m "feat(dashboard): parameter editor row

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Wire parameters into `DashboardView` (state, manager, filter bar, save)

**Files:** `frontend/src/routes/explore/DashboardView.tsx`, `__tests__/DashboardView.test.tsx`.

Read the current file first. Add a `draftParameters` state (mirrors `draftCards`), a parameter manager in edit mode (uses `ParameterEditor` + a "+ Filter" button using `slugify`), the `<DashboardFilterBar>` (view + edit) wired to `useDashboardParams`, and include `parameters` in `onSave`.

- [ ] **Step 1: Failing tests (append)**

```tsx
describe("DashboardView (Phase C — parameters)", () => {
  it("adds a filter parameter in edit mode", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /\+ filter/i }));
    expect(screen.getByLabelText(/parameter name/i)).toBeInTheDocument();
  });
  it("renders the filter bar in view mode when the dashboard has parameters", () => {
    // dashState.loaded has no params by default; this test sets one
    dashState.current = { data: { ...(dashState.loaded.data as object), parameters: [{ id: "p1", name: "Region", slug: "region", type: "text", operator: "is" }] }, isLoading: false };
    renderAt();
    expect(screen.getByLabelText("Region")).toBeInTheDocument();
  });
});
```
(Requires the Phase B test mock to carry `parameters`. Update the hoisted `dashState.loaded.data` to include `parameters: []`, and the `useDashboard` mock already returns `dashState.current`.)

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement**

- Import: `useDashboardParams`, `DashboardFilterBar`, `ParameterEditor`, `slugify`, types `DashboardParameter`.
- Add state next to `draftCards`: `const [draftParameters, setDraftParameters] = useState<DashboardParameter[] | null>(null);` and `const parameters = editing && draftParameters ? draftParameters : (dashboard.parameters ?? []);`
- In `startEdit`: also `setDraftParameters((dashboard.parameters ?? []).map((p) => ({ ...p })));`. In `cancelEdit`: `setDraftParameters(null);`. In the id-change reset effect: `setDraftParameters(null);`.
- `const view = useDashboardViewControls();` already exists; add `const paramsState = useDashboardParams(parameters);` near it (BEFORE the early return — Rules of Hooks).
- Parameter manager (edit mode), e.g. inside the edit `<aside>` or a new block:
```tsx
{editing && (
  <div className="mb-3 rounded-md border p-2">
    <div className="mb-1 flex items-center justify-between">
      <span className="text-xs font-medium text-muted-foreground">Filters</span>
      <Button size="sm" variant="outline" onClick={() => {
        const slug = slugify("filter", parameters.map((p) => p.slug));
        setDraftParameters([...parameters, { id: `new-${slug}-${cardSeqRef.current++}`, name: "Filter", slug, type: "text", operator: "is" }]);
      }}>+ Filter</Button>
    </div>
    <div className="flex flex-col gap-1">
      {parameters.map((p) => (
        <ParameterEditor key={p.id} param={p}
          onChange={(next) => setDraftParameters(parameters.map((x) => x.id === p.id ? { ...next, slug: x.slug } : x))}
          onDelete={() => setDraftParameters(parameters.filter((x) => x.id !== p.id))} />
      ))}
    </div>
  </div>
)}
```
- Filter bar (both modes), above the grid: compute `optionsBySlug` for category/id params from the first mapped card (Task 9 wires `useColumnValues`; for THIS task pass `optionsBySlug={{}}` and a follow-up note — or implement the simple single-source lookup if straightforward). Render:
```tsx
<DashboardFilterBar parameters={parameters} values={paramsState.values} onChange={paramsState.setValue} optionsBySlug={optionsBySlug} />
```
- `onSave`: include parameters:
```tsx
await update.mutateAsync({ id: dashboard.id, name: dashboard.name, description: dashboard.description, parameters, cards: payload });
```
  (Remove the temporary `parameters: []` placeholder added in Task 2.)

- [ ] **Step 4: Run tests + sweep** (`npx vitest run`, then typecheck/lint/build). All Phase A/B/C tests green.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.test.tsx
git commit -m "feat(dashboard): parameter manager + filter bar wired into DashboardView

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Per-card mapping + filter injection + `{{slug}}` interpolation

**Files:** `frontend/src/routes/explore/DashboardView.tsx`, `frontend/src/components/chart/dashboard/DashboardCardView.tsx`, `__tests__/DashboardView.test.tsx`.

- [ ] **Step 1: Failing tests (append to DashboardView.test.tsx)**

```tsx
describe("DashboardView (Phase C — mapping + injection)", () => {
  it("shows a per-card mapping dropdown for each parameter in edit mode", () => {
    dashState.current = { data: { ...(dashState.loaded.data as object), parameters: [{ id: "p1", name: "Region", slug: "region", type: "text", operator: "is" }] }, isLoading: false };
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    expect(screen.getByLabelText(/map Region/i)).toBeInTheDocument();
  });
});
```
(The mock chart "Sales" has `params.spec.dimensions` — ensure the mock chart in this file includes a dimension, e.g. `dimensions: ["region"]`, so the mapping dropdown has a column option. Update the `useCharts` mock chart's spec accordingly.)

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement**

In `DashboardView`:
- For each card in edit mode, render a mapping control per parameter (mappable columns = `chart.params.spec.dimensions ∪ chart.params.spec.columns`):
```tsx
{editing && parameters.length > 0 && c.card_type === "chart" && (() => {
  const ch = chartById.get(c.chart_id ?? "");
  const cols = ch ? [...ch.params.spec.dimensions, ...ch.params.spec.columns] : [];
  const mappings = (c.options.paramMappings as Record<string, string>) ?? {};
  return (
    <div className="m-1 flex flex-col gap-0.5 border-t pt-1">
      {parameters.map((p) => (
        <label key={p.id} className="flex items-center justify-between gap-1 text-[10px] text-muted-foreground">
          <span>{p.name}</span>
          <select aria-label={`map ${p.name}`} className="h-6 rounded border bg-background text-[10px]"
            value={mappings[p.id] ?? ""}
            onChange={(e) => updateCardOptions(c.id, { paramMappings: { ...mappings, [p.id]: e.target.value } })}>
            <option value="">none</option>
            {cols.map((col) => <option key={col} value={col}>{col}</option>)}
          </select>
        </label>
      ))}
    </div>
  );
})()}
```
- Compute the injected chart per card and pass it to `DashboardCardView`. Replace the chart passed in:
```tsx
const baseChart = chartById.get(c.chart_id ?? "");
const mappings = (c.options.paramMappings as Record<string, string>) ?? {};
const injected = baseChart ? applyParamsToChart(baseChart, parameters, paramsState.values, mappings) : undefined;
// ...
<DashboardCardView ws={ws} card={c} chart={injected} refreshNonce={view.refreshNonce} parameters={parameters} paramValues={paramsState.values} />
```
  (Import `applyParamsToChart` from `@/components/chart/dashboard/params`.)
- Build `optionsBySlug` for the filter bar (Task 8 left it `{}`): for each category/id parameter, find the first card whose `options.paramMappings[param.id]` is set, and call `useColumnValues(ws, thatCard.chart.dataset_id, mappedColumn, true)`. Since hooks can't be called in a loop conditionally, implement a small `<ParamOptionsLoader>` helper component per category/id param that calls `useColumnValues` and reports values up via `onLoaded`, OR (simpler) compute the sources array (stable length per parameters) and render one hidden loader component per category/id param. Keep it out of the render loop's hook rules by using a dedicated child component. If this proves complex, ship category/id with a free-text fallback (optionsBySlug `{}`) and note the dropdown-population as a follow-up — do NOT block the phase on it.

In `DashboardCardView.tsx`:
- Add props `parameters?: DashboardParameter[]` and `paramValues?: Record<string, unknown>`. For `text`/`heading` cards, interpolate before rendering:
```tsx
import { interpolateParams } from "./params";
// text:
const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {});
return <TextCard text={text} />;
// heading similarly
```

- [ ] **Step 4: Run tests + full sweep.** All green.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/components/chart/dashboard/DashboardCardView.tsx frontend/src/routes/explore/__tests__/DashboardView.test.tsx
git commit -m "feat(dashboard): per-card param mapping, filter injection, {{slug}} interpolation

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Live verification (Playwright MCP) + PR

- [ ] **Step 1: Restart the backend** (migration 0016 — apply the staleness fix): `touch crates/egret-metastore/src/lib.rs && cargo build -p egret-bin`, then restart `cargo run -p egret-bin -- serve`. Confirm `to_regclass('public.dashboards')` has a `parameters` column. Frontend dev already running.

- [ ] **Step 2: Manual live flow (Playwright MCP browser, real Chrome):** open a dashboard → Edit → **+ Filter** → make a `text/contains` param "Region" and a `category` param → map them to a chart card's column → add a text card with `{{region}}` → Save → in view mode set Region = a value → confirm the mapped card **refetches filtered**, the `{{region}}` text updates, and the value is in the URL (`?region=…`) and survives reload. Check `mcp__playwright__browser_console_messages` = **0 errors**. (This is manual MCP browser driving, NOT an automated Playwright test — egret's automated suite is Vitest.)

- [ ] **Step 3: Backend hygiene:** `cargo test -p egret-bi && cargo clippy -p egret-bi --all-targets && cargo fmt -p egret-bi -- --check`. Clean up any test dashboards created.

- [ ] **Step 4: Open the PR**
```bash
git push -u origin feat/dashboards-phase-c
gh pr create --base main --title "feat(dashboards): Phase C — filters/parameters + {{slug}} interpolation" --body "<summary; reference docs/en/specs/2026-06-22-phase-3d-c-dashboard-filters-design.md; note manual Playwright-MCP verification (not an automated test); 🤖 Generated with [Claude Code](https://claude.com/claude-code)>"
```
Wait for CI; squash-merge.

---

## Acceptance (Phase C)

- A dashboard can define text/number/date/id/category parameters (with chosen operators / category multi-select); each is mapped per-card to a column and persists.
- Setting a filter value injects the right `ChartFilter`(s) into each mapped card's query (cards refetch filtered); unmapped cards are untouched.
- `{{slug}}` in text/heading cards reflects the current value; values URL-sync by slug and survive reload.
- All Phase A/B behaviour intact; full sweep green; live MCP check has 0 console errors.
