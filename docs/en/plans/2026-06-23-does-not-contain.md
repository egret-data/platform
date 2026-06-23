# does-not-contain (`not_like`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the text-filter **does-not-contain** operator (the negation of `contains`), which requires a new backend `not_like` SQL op.

**Architecture:** Backend adds a `NotLike` variant to the `FilterOp` enum + a `NOT LIKE` arm in the SQL renderer (`crates/egret-bi/src/chartquery.rs`); `NOT LIKE` is standard SQL so no per-engine branching. Frontend adds `not_like` to the `FilterOp` union, maps the dashboard text operator `does-not-contain → not_like '%v%'` in `paramToFilters`, and lists it in `OPERATORS.text`.

**Tech Stack:** Rust + sqlx (backend); React 18 + TS + Vitest (frontend).

**Conventions for every task:** TDD (failing test first). Backend sweep before commit (from repo root): `cargo test -p egret-bi && cargo fmt --all --check` (run with `DATABASE_URL=postgres://egret:egret@localhost:5432/egret` so DB-gated tests run; the `chartquery` tests are pure unit tests and run without it, but pass the URL anyway for the full suite). Frontend sweep (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage explicit paths. Configured git identity. End every commit message with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Branch `feat/dashboards-does-not-contain` (spec already committed).

**Backend facts (verified):** `crates/egret-bi/src/chartquery.rs` — `FilterOp` enum (`#[serde(rename_all = "snake_case")]`, variants `Eq/Ne/Gt/Ge/Lt/Le/Like/In/NotIn/IsNull/IsNotNull`). `render_filter`'s scalar arm: `let op = match f.op { FilterOp::Eq => "=", … FilterOp::Like => "LIKE", _ => unreachable!("handled above") }; format!("{lhs} {op} {lit}")`. `sql_literal` single-quotes/escapes strings. Existing filter test `build_filters_use_where_and_escape` (line 502) builds a `QuerySpec` with `Filter { col, op, value }` and asserts `sql.contains("…")`; helpers `col(name, tg, expr)`, `phys()`, `metric()` are in the test module.

**Frontend facts (verified):** `api/charts.ts` — `FilterOp = "eq"|…|"like"|"in"|"not_in"|"is_null"|"is_not_null"`; `ValueFilterOp = Exclude<FilterOp, NullFilterOp>` (so `not_like` auto-becomes a ValueFilterOp). `params.ts` `paramToFilters` text case uses `mkf(op, v)` and an `isBlank(value)` guard, mapping `contains → mkf("like", \`%${v}%\`)`. `ParameterEditor.tsx` `OPERATORS.text = ["is","is-not","contains","starts-with","ends-with"]`. `api/dashboards.ts` `ParamOperator` text group lists `"is"|"is-not"|"contains"|"starts-with"|"ends-with"`.

---

### Task 1: Backend `not_like`

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs` (`FilterOp` enum + `render_filter`)
- Test: `crates/egret-bi/src/chartquery.rs` (add a `#[test]` in the existing `mod tests`)

- [ ] **Step 1: Write the failing test** — add to `mod tests` in `crates/egret-bi/src/chartquery.rs` (after `build_filters_use_where_and_escape`):

```rust
    #[test]
    fn not_like_filter_renders_not_like() {
        let cols = vec![col("region", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into()],
            columns: vec![],
            filters: vec![Filter {
                col: "region".into(),
                op: FilterOp::NotLike,
                value: serde_json::json!("%spam%"),
            }],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
        };
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(sql.contains("\"region\" NOT LIKE '%spam%'"), "{sql}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi not_like_filter_renders_not_like 2>&1 | tail -20`
Expected: COMPILE error — `no variant named NotLike found for enum FilterOp`.

- [ ] **Step 3: Add the `NotLike` enum variant**

In the `FilterOp` enum, add `NotLike,` immediately after `Like,`:

```rust
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Like,
    NotLike,
    In,
    NotIn,
    IsNull,
    IsNotNull,
}
```

- [ ] **Step 4: Add the `NOT LIKE` render arm**

In `render_filter`'s scalar op `match f.op`, add the `NotLike` arm right after `Like`:

```rust
                FilterOp::Like => "LIKE",
                FilterOp::NotLike => "NOT LIKE",
```

(The scalar arm already does `format!("{lhs} {op} {lit}")`, so `not_like` renders `"region" NOT LIKE '%spam%'`, with the value escaped by `sql_literal` exactly like `Like`. `NotLike` is a scalar op — it must NOT be added to the `In | NotIn` or `IsNull | IsNotNull` arms.)

- [ ] **Step 5: Run test to verify it passes**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi not_like_filter_renders_not_like 2>&1 | tail -10`
Expected: `test ... ok` (1 passed). If it reports 0 run/filtered, the test name was mistyped.

- [ ] **Step 6: Backend sweep + commit**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi && cargo fmt --all --check
git add crates/egret-bi/src/chartquery.rs
git commit -m "feat(charts): add not_like filter op (renders NOT LIKE)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Frontend types + `paramToFilters` + `OPERATORS`

**Files:**
- Modify: `frontend/src/api/charts.ts` (`FilterOp` union)
- Modify: `frontend/src/api/dashboards.ts` (`ParamOperator` text group)
- Modify: `frontend/src/components/chart/dashboard/params.ts` (`paramToFilters` text case)
- Modify: `frontend/src/components/chart/dashboard/ParameterEditor.tsx` (`OPERATORS.text`)
- Test: `frontend/src/components/chart/dashboard/__tests__/params.notcontain.test.ts` (create)
- Test: `frontend/src/components/chart/dashboard/__tests__/operators.test.ts` (extend)

- [ ] **Step 1: Write the failing tests**

Create `frontend/src/components/chart/dashboard/__tests__/params.notcontain.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { paramToFilters } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "F", slug: "f", type: "text", operator: "does-not-contain", ...over });

describe("paramToFilters does-not-contain", () => {
  it("maps to not_like with %v% wildcards", () => {
    expect(paramToFilters(p({}), "spam", "title")).toEqual([{ col: "title", op: "not_like", value: "%spam%" }]);
  });
  it("blank → []", () => {
    expect(paramToFilters(p({}), "", "title")).toEqual([]);
  });
});
```

Add to the existing `frontend/src/components/chart/dashboard/__tests__/operators.test.ts` (inside the `describe("OPERATORS", ...)` block):

```ts
  it("text includes does-not-contain", () => {
    expect(OPERATORS.text).toContain("does-not-contain");
  });
```

- [ ] **Step 2: Run tests to verify they fail**

Run (from `frontend/`):
`npx vitest run src/components/chart/dashboard/__tests__/params.notcontain.test.ts src/components/chart/dashboard/__tests__/operators.test.ts`
Expected: FAIL — `does-not-contain` not in `OPERATORS.text`; `paramToFilters` falls through to the default `eq` (not `not_like`); and `operator: "does-not-contain"` won't typecheck against `ParamOperator` until Step 4.

- [ ] **Step 3: Add `not_like` to the frontend `FilterOp` union**

In `frontend/src/api/charts.ts`, change the `FilterOp` union (line 14) to include `"not_like"`:

```ts
export type FilterOp =
  | "eq" | "ne" | "gt" | "ge" | "lt" | "le" | "like" | "not_like" | "in" | "not_in" | "is_null" | "is_not_null";
```

(`ValueFilterOp = Exclude<FilterOp, NullFilterOp>` and `ChartFilter` follow automatically.)

- [ ] **Step 4: Add `does-not-contain` to `ParamOperator`**

In `frontend/src/api/dashboards.ts`, add `"does-not-contain"` to the text group of the `ParamOperator` union (the line listing `"is" | "is-not" | "contains" | "starts-with" | "ends-with"`):

```ts
  | "is" | "is-not" | "contains" | "does-not-contain" | "starts-with" | "ends-with"   // text / category / location
```

(Keep the rest of the union and any trailing comment unchanged.)

- [ ] **Step 5: Map `does-not-contain` in `paramToFilters`**

In `frontend/src/components/chart/dashboard/params.ts`, in the `case "text":` operator switch, add a `does-not-contain` arm next to `contains`:

```ts
        case "contains":
          return [mkf("like", `%${v}%`)];
        case "does-not-contain":
          return [mkf("not_like", `%${v}%`)];
```

(The text case already guards `if (isBlank(value)) return [];` at the top, so blank → `[]` needs no extra code.)

- [ ] **Step 6: Add `does-not-contain` to `OPERATORS.text`**

In `frontend/src/components/chart/dashboard/ParameterEditor.tsx`, change `OPERATORS.text`:

```ts
  text: ["is", "is-not", "contains", "does-not-contain", "starts-with", "ends-with"],
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `npx vitest run src/components/chart/dashboard/__tests__/params.notcontain.test.ts src/components/chart/dashboard/__tests__/operators.test.ts`
Expected: PASS. Also run the whole dashboard `__tests__` dir to confirm no regression: `npx vitest run src/components/chart/dashboard/__tests__`.

- [ ] **Step 8: Frontend sweep + commit**

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/api/charts.ts src/api/dashboards.ts src/components/chart/dashboard/params.ts src/components/chart/dashboard/ParameterEditor.tsx src/components/chart/dashboard/__tests__/params.notcontain.test.ts src/components/chart/dashboard/__tests__/operators.test.ts
git commit -m "feat(dashboards): text does-not-contain filter operator (not_like)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Live verification + PR

**Files:** none (verification only).

- [ ] **Step 1: Final sweeps**

Backend (repo root): `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi && cargo fmt --all --check` → green.
Frontend (`frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run` → green.

- [ ] **Step 2: Run the stack & log in**

The dev backend on :8080 predates this branch (it has no `not_like` op), so for the live test **rebuild + restart it**: `cargo build --bin egret`, then restart `egret serve` (a `.env` with `DATABASE_URL`, `EGRET_ADMIN_EMAIL=admin@egret.local`, `EGRET_ADMIN_PASSWORD=Admin12345!` reproduces the dev config). `npm run dev` for the frontend. Log in `admin@egret.local` / `Admin12345!`, eng workspace. **Do NOT touch "a1".** Use a throwaway dashboard and delete it after.

- [ ] **Step 3: Live-verify (Playwright MCP, side-by-side Metabase), 0 console errors**

On a throwaway dashboard with a chart that has a text dimension: add a text filter, set its operator to **does-not-contain**, enter a substring that some rows contain → those rows disappear from the card (the SQL ran `col NOT LIKE '%substring%'`). Confirm against Metabase's does-not-contain behaviour. Confirm existing text operators (is/is-not/contains/starts-with/ends-with) still work.

- [ ] **Step 4: Clean up & open PR**

Delete the throwaway dashboard. Then:
```bash
git push -u origin feat/dashboards-does-not-contain
```
Open PR `feat(dashboards): text does-not-contain filter operator (not_like)`; summarise the backend `NOT LIKE` op + the dashboard text operator; note no migration and no per-engine branching. End the PR body with the `🤖 Generated with [Claude Code]` footer. Squash-merge when CI is green (the flaky `egret-ranger` test may need a rerun — it is unrelated).

---

## Self-Review

**Spec coverage:**
- Backend `NotLike` enum + `NOT LIKE` render → Task 1. ✓
- Frontend `not_like` on `FilterOp` → Task 2 Step 3. ✓
- `paramToFilters` `does-not-contain → not_like '%v%'` → Task 2 Step 5. ✓
- `OPERATORS.text` += does-not-contain → Task 2 Step 6. ✓
- `ParamOperator` += does-not-contain → Task 2 Step 4 (needed so the operator typechecks). ✓
- No per-engine branching, no chart-builder change → respected (Task 1 adds only the scalar arm). ✓
- Testing (backend render test, frontend paramToFilters + operators, live) → Tasks 1–3. ✓

**Type/signature consistency:**
- Backend `FilterOp::NotLike` (Task 1) ↔ frontend `"not_like"` (Task 2) — the serde `snake_case` rename makes `NotLike` deserialize from `"not_like"`, so the wire value the frontend emits matches the backend variant.
- `does-not-contain` is used identically in `ParamOperator` (Task 2.4), `paramToFilters` (2.5), and `OPERATORS.text` (2.6).
- `mkf("not_like", …)` (2.5) requires `"not_like"` to be a `ValueFilterOp` — satisfied by 2.3.

**Placeholder scan:** No TBD/TODO; every step shows complete code.
