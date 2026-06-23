# does-not-contain (`not_like`) — Kế hoạch triển khai

> Bản tiếng Việt tham chiếu. Bản chuẩn (đủ code từng task) là tiếng Anh:
> `docs/en/plans/2026-06-23-does-not-contain.md`. Spec: `docs/en/specs/2026-06-23-does-not-contain-design.md`.

**Mục tiêu:** Thêm operator text **does-not-contain** (phủ định của `contains`) — cần op backend `not_like`.
Branch `feat/dashboards-does-not-contain` (off main, spec đã commit).

**Quy ước mọi task:** TDD (test đỏ trước). BE sweep: `cargo test -p egret-bi && cargo fmt --all --check`
(set `DATABASE_URL=postgres://egret:egret@localhost:5432/egret`). FE sweep (từ `frontend/`):
`npm run typecheck && lint && build && npx vitest run`. Stage path tường minh; identity git + footer Co-Authored-By.

## Task 1 — Backend not_like (`crates/egret-bi/src/chartquery.rs`)
- Test đỏ: thêm `not_like_filter_renders_not_like` trong `mod tests` (build QuerySpec với `Filter{op:FilterOp::NotLike, value:"%spam%"}`, assert `sql.contains("\"region\" NOT LIKE '%spam%'")`).
- Thêm `NotLike,` vào enum `FilterOp` (sau `Like`; serde snake_case → "not_like").
- Thêm `FilterOp::NotLike => "NOT LIKE",` vào `match f.op` nhánh scalar (sau `Like => "LIKE"`). KHÔNG thêm vào nhánh In/NotIn hay IsNull.
- `NOT LIKE` là SQL chuẩn → không branching theo engine; value escape qua `sql_literal` như `Like`.

## Task 2 — Frontend types + paramToFilters + OPERATORS
- Test đỏ: `params.notcontain.test.ts` (does-not-contain → `{op:"not_like", value:"%v%"}`, rỗng → `[]`); thêm vào `operators.test.ts`: `OPERATORS.text` chứa `does-not-contain`.
- `api/charts.ts`: thêm `"not_like"` vào union `FilterOp` (ValueFilterOp/ChartFilter tự theo).
- `api/dashboards.ts`: thêm `"does-not-contain"` vào nhóm text của `ParamOperator`.
- `params.ts` text case: thêm `case "does-not-contain": return [mkf("not_like", \`%${v}%\`)];` cạnh `contains` (rỗng đã có guard `isBlank`).
- `ParameterEditor.tsx`: `OPERATORS.text` += `"does-not-contain"`.

## Task 3 — Verify live + PR
- Full sweep BE+FE. Backend đang chạy là bản cũ (chưa có `not_like`) → **rebuild + restart** `egret serve` (`.env`: DATABASE_URL +
  EGRET_ADMIN_EMAIL=admin@egret.local + EGRET_ADMIN_PASSWORD=Admin12345!). `npm run dev`. Login admin.
- Playwright song song Metabase, **0 lỗi console, không đụng a1**, dùng dashboard test: text filter does-not-contain → dòng chứa substring biến mất; các op text khác vẫn chạy. Xóa dashboard test.
- push + PR + squash-merge khi CI xanh (flaky `egret-ranger` có thể cần rerun, không liên quan).

## Thứ tự
Task 1 (backend) → Task 2 (frontend) → Task 3 (live + PR).
