# Text filter "does-not-contain" (`not_like`) — thiết kế

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-23-does-not-contain-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-23 · **Branch:** `feat/dashboards-does-not-contain` (off main)

## Mục tiêu

Thêm operator text **does-not-contain** — operator filter duy nhất bị defer suốt Tier 2d/3b vì cần op backend. Nó là phủ
định của `contains`: `contains` → `like '%v%'`, nên `does-not-contain` → `not_like '%v%'` (op mới). Đây là feature filter
dashboard đầu tiên đụng SQL builder backend.

## Phạm vi

**Có:** (1) variant `NotLike` ở enum `FilterOp` backend + render SQL (`NOT LIKE`); (2) `"not_like"` ở union `FilterOp`
frontend; (3) `paramToFilters` text `does-not-contain` → `not_like '%v%'`; (4) `OPERATORS.text` thêm `does-not-contain`.

**Không cần:** branching theo engine (NOT LIKE là SQL chuẩn, Trino + StarRocks giống nhau; `engine` chỉ ảnh hưởng quoting
identifier). Chart-builder **không có** chỗ chọn op cho text filter (đã verify — filter ở đó qua field zone), nên không có gì
để mở rộng; `does-not-contain` thuần là operator của dashboard text-parameter. Không migration.

## Code hiện có (đã verify)

- Backend `crates/egret-bi/src/chartquery.rs`: enum `FilterOp` (`#[serde(rename_all="snake_case")]`). `render_filter` nhánh
  scalar build `format!("{lhs} {op} {lit}")`, `op` từ `match f.op` (`Like => "LIKE"`, `_ => unreachable!`). In/NotIn,
  IsNull/IsNotNull xử ở nhánh trước. `sql_literal` quote/escape string (primitive bảo mật). Op từ enum → không inject op.
- Frontend `api/charts.ts`: `FilterOp` union; `ValueFilterOp = Exclude<FilterOp, NullFilterOp>` (thêm `not_like` tự thành
  ValueFilterOp). `params.ts` `paramToFilters` text: is→eq, is-not→ne, contains→like '%v%', starts-with→like 'v%',
  ends-with→like '%v'. `ParameterEditor.tsx` `OPERATORS.text`.

## Thay đổi backend (chartquery.rs)
1. Thêm `NotLike,` vào enum `FilterOp` (sau `Like`) → deserialize từ `"not_like"`.
2. Thêm `FilterOp::NotLike => "NOT LIKE",` vào `match f.op` nhánh scalar.
→ filter `{op:"not_like", value:"%v%"}` render `col NOT LIKE '%v%'` cho cả 2 engine; value escape qua `sql_literal` như `Like`.

## Thay đổi frontend
1. `api/charts.ts`: thêm `"not_like"` vào union `FilterOp`.
2. `params.ts` text case: `case "does-not-contain": return [mkf("not_like", \`%${v}%\`)];`.
3. `ParameterEditor.tsx`: `OPERATORS.text` += `"does-not-contain"`.
4. `api/dashboards.ts` `ParamOperator`: đảm bảo `"does-not-contain"` có trong nhóm text-operator.

## Data flow
```
text param, op "does-not-contain", value "foo"
  → paramToFilters → { col, op:"not_like", value:"%foo%" }
  → applyParamsToChart → QuerySpec.filters → backend render_filter → `col NOT LIKE '%foo%'`
```

## Xử lý lỗi
- `not_like` value rỗng → `paramToFilters` trả `[]` (guard `isBlank` đầu text case). Value không phải string → `sql_literal`
  stringify/escape như `Like`. Op lạ → backend reject lúc deserialize (enum) → không inject SQL.

## Kiểm thử
- **Backend** (chartquery tests): test filter `not_like` render `... NOT LIKE '%v%'` (mirror test `Like`, assert value quote/escape).
- **Frontend** (Vitest): `paramToFilters` does-not-contain → `{op:"not_like", value:"%v%"}`; rỗng → `[]`; `OPERATORS.text` có does-not-contain.
- **Live** (Playwright song song Metabase, 0 lỗi console): text filter does-not-contain thu hẹp card (dòng chứa term biến mất).
  Chỉ dashboard test, **không đụng a1**.
- **Full sweep** trước mỗi commit — BE: `cargo test -p egret-bi && cargo fmt --all --check`; FE (từ `frontend/`):
  `npm run typecheck && lint && build && npx vitest run`.

## Quy ước
Metabase là chuẩn · clean-room · TDD · full sweep · identity git + footer Co-Authored-By · docs song ngữ · squash-merge ·
branch `feat/dashboards-does-not-contain`.
