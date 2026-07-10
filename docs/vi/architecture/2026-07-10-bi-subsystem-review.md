# Review kiến trúc subsystem BI ("Explore")

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/architecture/2026-07-10-bi-subsystem-review.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Review / đánh giá · **Ngày:** 2026-07-10 · **Loại:** nội bộ
>
> Đánh giá 3 mặt subsystem BI/Explore — backend (`crates/egret-bi`), data model (migrations + stores), frontend (`frontend/src` chart/dashboard/explore). Sinh từ audit read-only song song. Mục đích: thiết kế BI đã ổn chưa, và cần lên lịch trả nợ gì.

---

## Kết luận

**Ổn, nợ có giới hạn.** Cả ba tầng nền tảng tốt; nợ nằm ở **tổ chức code** + vài **lỗ hổng correctness**, không phải rot cấu trúc. Không có gì khiến gọi thiết kế là có vấn đề. Doc này xếp hạng findings để lên lịch.

## Phần thực sự tốt (giữ hướng)

- **Backend** — `chartquery.rs` là compiler `QuerySpec → SQL` thuần, hardened chống injection (enum `FilterOp`; `sql_literal` type-validated tách riêng `chartquery.rs:135-187`; ~24 test inline gồm test injection). `typegen.rs` single-source-of-truth phân loại type engine. Repo per-entity mỏng, nhất quán. Split **"backend compile SQL, frontend render"** có chủ đích: `ch_data` trả `{ sql, database_id }` (`routes.rs:1627-1666`), client execute + shape + render. Dialect thật (`GROUPING SETS` portable vì StarRocks misparse `ROLLUP(a),ROLLUP(b)` — `chartquery.rs:408-457`).
- **Data model** — semantic layer (`dataset_columns`/`dataset_metrics`, `0012`/`0013`) + dashboard layout (grid geometry là cột thật, `dashboard_tabs` là bảng thật) **chuẩn hoá đàng hoàng — tốt hơn Metabase vốn nhét JSON**. `workspace_id NOT NULL … ON DELETE CASCADE` nhất quán, không đường cross-tenant (**tenant-safe**). Migration hygiene kỷ luật (split seam `0062–0067` có chủ đích, không churn). `access_grants` (`0068`) đã reserve `scope_type='object'` + `object_type` + `scope_id`.
- **Frontend** — kỷ luật orval client cực tốt: **0 fetch tay trong feature code**, một mutator chokepoint, không drift. Gần 0 `any`. Logic nặng (pivot `engine.ts`, dashboard `params.ts`, echarts `options/*`) **thuần + unit-test** (74 test dưới `components/chart`, 159 FE test file tổng). Metabase-fidelity thật — palette/style lifted có attribution; 19 viz type; dashboard model đầy đủ (grid, tabs, cross-filter, click-behavior, per-card override). `ChartRenderer` reuse cả editor lẫn dashboard card.

## Findings, xếp hạng

### Correctness (fix sớm; rẻ)

**C1 — `UNIQUE(workspace_id, name)` xung đột soft-delete.** charts/dashboards/datasets có `UNIQUE(workspace_id, name)` (`0014`/`0015`/`0012`) **và** sau đó `deleted_at` (`0019`/`0022`). Item trong trash vẫn giữ tên → không tạo mới trùng tên được, restore có thể conflict. **Latent bug.** Fix: **partial unique index** `… WHERE deleted_at IS NULL`. (Cũng nên xét lại uniqueness toàn-workspace cho tool nhiều người — hai analyst không cùng tạo "Revenue".)

**C2 — Chart `params`/`viz_type` là JSONB opaque, không validate.** `analytics.charts.params jsonb` (`0014`) lưu verbatim (`chart.rs:20-21`, `routes.rs:1481-1482`), không enum/schema/validate. Quan trọng: builder `QuerySpec` (đã validate) là **type tách rời, không liên kết** — config chart đã lưu không bao giờ được kiểm với builder sẽ dùng nó, nên không đảm bảo chart lưu ra được SQL, và config không query được ("chart nào dùng metric X?"). Đây là hiện thân BI của nợ "JSONB-then-normalize". Tối thiểu: **envelope có version + validate** cho `params`.

### Maintainability (refactor giá trị cao)

**M1 — `routes.rs` 2621 dòng monolith.** Chứa `BiState`, 5 router, ~40 handler, DTO helper, 5 struct `#[derive(OpenApi)]`; comment section thay ranh giới module. Cộng thêm dòng authz+error copy-paste ~40 lần. Tách `routes/{sqllab,dataset,chart,dashboard,folder}.rs` + guard combinator/extractor.

**M2 — `ChartExplore.tsx` 986 dòng god-component** (37 `useState`, ~11 config object per-viz). Branching per-viz nhân bản **6 chỗ** (hydration effect, `specForRun`, validate trong `run`, `paramsForSave`, `resetControls`, JSX panel). **Không có viz-registry phía editor** — thêm viz type = sửa cả 6 + `ChartRenderer` + `chartTypes.ts` + API types. Refactor FE giá trị cao nhất: rút **per-viz descriptor/registry**.

### Medium

**Md1 — `bookmarks` polymorphic (không FK) rò orphan.** `bookmarks(entity_type, entity_id)` (`0020`) không FK. `dashboard.rs:203` purge khi hard-delete; chart/dataset **không** → tích orphan. Centralize cleanup (trigger / shared purge).

**Md2 — Thiếu index.** Không index `charts/dashboards/datasets.folder_id`, `dashboard_cards.chart_id/tab_id`. `chart_id CASCADE` seq-scan `dashboard_cards` mỗi lần xoá chart; list theo folder seq-scan. List cũng tính count qua correlated subquery per-row. Chịu được ở cardinality hiện tại; thêm trước khi scale.

**Md3 — Editor bypass renderer dispatcher.** `ChartExplore` render `ChartTable`/`PivotTable` trực tiếp trong khi dùng `ChartRenderer` cho viz khác → prop-wiring table/pivot ở hai nơi dễ drift. `ChartRenderer` (251 dòng) là if-chain lặp `<div>{empty?…:…}</div>` 7 lần. Gom preview vào `ChartRenderer`.

**Md4 — SQL builder + dialect có trần mở rộng.** `build()` (`chartquery.rs:355-485`) một hàm ~130 dòng dựa `format!`, không AST; metric/`HAVING` filter + join deferred (`chartquery.rs:280`). `match engine` rải `chartquery.rs`/`schema.rs`/`expr.rs`/`typegen.rs`, không `Dialect` trait (match exhaustive là safety net). Ổn hôm nay; sẽ kháng cự khi cần join/HAVING/engine thứ 3.

### Product ceiling (roadmap, không phải defect)

**P1 — Folder phẳng, per-type vs Metabase nested collections.** `folders` (`0021`) không `parent_id`, phân theo `entity_type`. Hai giới hạn: không nesting cho workspace lớn; folder không chứa mixed type. **Liên quan trực tiếp B2 object-scope đang làm:** object grant sẽ per-object; folder-inheritance chạy được nhưng bị giới hạn folder phẳng single-type. Nếu cần parity collection thật → nested (polymorphic) collections.

**P2 — `saved_queries` công dân hạng hai.** Không `folder_id`/`deleted_at`/bookmark (`0010`), khác mọi Explore entity — abstraction "Explore resource" chưa đồng nhất.

**P3 — FE gaps vs Metabase.** Chưa GUI query-builder/"notebook" join-and-aggregate (metric pre-defined trên dataset, không compose trong editor); alerts/subscriptions/pulses là `Placeholder`; editor nặng không lazy-load. Scope gap, không phải design flaw.

## Một ranh giới kiến trúc đáng xét lại chủ động

Decoupling **"backend compile SQL; frontend làm hết result-shaping + render"** khiến thêm chart type = 0 công backend — nhưng cũng khiến backend **không guardrail nào** trên chart config (gốc của C2). Ranh giới này defensible, nhưng là chỗ duy nhất đáng quyết định lại chủ động: vd validate `params` phía server (structure/refs) **mà không** kéo render về server.

## Lịch đề xuất

- **Tuần này (rẻ, correctness):** C1 partial unique index · Md2 index folder/card · Md1 centralize bookmark cleanup.
- **Refactor giá trị cao:** M1 tách `routes.rs` + authz guard combinator · M2 viz-registry `ChartExplore` (+ Md3 gom table/pivot preview vào `ChartRenderer`).
- **Quyết định sâu:** C2 version + validate chart `params` (+ xem lại ranh giới guardrail backend).
- **Roadmap:** P1 nested collections nếu cần Metabase-parity ở scale · P2 kéo `saved_queries` vào abstraction Explore · Md4 query AST / `Dialect` trait khi join/HAVING/engine thứ 3.

---

*Đánh giá 2026-07-10. Thiết kế BI về cơ bản ổn cả backend, data model, frontend; nợ là tổ chức (M1/M2) + vài lỗ hổng correctness (C1/C2) + trần roadmap (P1–P3). Không có gì structural. Liên quan Effort B / B2 object-scope đang làm: finding folder-phẳng (P1) xác nhận object grant sẽ per-object với folder-inheritance giới hạn.*
