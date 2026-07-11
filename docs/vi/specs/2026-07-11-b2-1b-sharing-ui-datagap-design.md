# B2-1b — Sharing UI + đường data object-scoped

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-11-b2-1b-sharing-ui-datagap-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-11 · **Phiên bản:** 1.0
>
> **Tiền nhiệm:** [B2-1a object-scope backend](./2026-07-10-b2-1a-object-scope-design.md).
>
> Increment thứ hai của B2. Làm object-sharing **hoàn chỉnh chức năng + dùng được**: (1) đóng data-access gap của B2-1a để shared user thực sự thấy **DATA** của dashboard/chart được share, (2) thêm **sharing UI** kiểu Metabase.

---

## 1. Mục tiêu & scope

B2-1a cho shared user GET *định nghĩa* dashboard/chart nhưng chưa thấy *data* — `ch_data` (build SQL) + query `submit` (run SQL) là workspace-scoped → object-guest 403 khi lấy data. B2-1b đóng gap bằng **endpoint data object-scoped, chạy server-side** cho saved object (Option B), và ship **sharing modal** dùng share API của B2-1a.

**Trong scope:**
- Backend: `POST /ws/{ws}/charts/{id}/data` và `POST /ws/{ws}/dashboards/{id}/cards/{card_id}/data` — object-scoped, chạy query **của saved object** server-side, trả rows.
- Frontend: chuyển pipeline view/render (dashboard card + xem saved chart) sang các endpoint đó; editor giữ `ch_data`. **Sharing modal** kiểu Metabase (picker người/group, Viewer/Editor, inherited-vs-direct) trên dashboard/chart/dataset/folder.

**Ngoài scope:** dataset data-preview object-scope + object-scope cho notebook/saved-query/warehouse (B2-2); **per-user data-plane delegation** — data vẫn chạy trên workspace DB connection, không phải danh tính caller (B3); role hierarchy / explicit deny (B2-3); flip bảng legacy (B2-flip); nested collections.

## 2. Backend — endpoint data object-scoped cho saved object (Option B)

### 2.1 Vì sao không dùng đường arbitrary-spec
`ch_data` nhận `ChartDataBody { dataset_id, query_spec }` do client cung cấp, trả SQL cho client chạy. Authorize object-scope cái đó **không an toàn** — user được share 1 chart có thể POST spec/dataset KHÁC và chạy query tuỳ ý. Nên render cho shared user phải chạy **query đã lưu của object**, server-side, không bao giờ dùng client spec.

### 2.2 Điểm mấu chốt: `params.spec` CHÍNH LÀ `QuerySpec`
`params` JSONB của saved chart đã lưu query dưới key `spec` (`paramsForSave()` của FE return `{ spec: <QuerySpec>, …viz-config }` cho mọi viz). Nên backend **deserialize `params.spec` thẳng thành `QuerySpec` Rust** (`chartquery::QuerySpec`) — không cần port logic params→spec của FE. Giữ Option B là thay đổi vừa phải, tránh nợ C2 "params opacity" (chỉ đọc sub-object `spec` vốn đã structured).

### 2.3 `POST /ws/{ws}/charts/{id}/data`
1. Resolve chart `ObjectRef` (`shares::resolve`); `require_object(&*st.pdp, &ctx, "chart:read", chart_obj)`.
2. Load saved chart → `dataset_id`, `params`; extract `params.spec` → `QuerySpec` (lỗi nếu thiếu/invalid).
3. Load columns + metrics của dataset; `chartquery::build(&spec, …)` → SQL + `database_id`.
4. **Chạy server-side** trên workspace DB connection (build connector như `schema.rs`, hoặc delegate helper query-execution) với row-limit; trả `{ columns, rows }`.
5. Caller không cung cấp spec → chỉ chạy được query của chính chart. An toàn.

### 2.4 `POST /ws/{ws}/dashboards/{id}/cards/{card_id}/data`
Dashboard gồm card tham chiếu `chart_id`. User được share **dashboard** (không phải từng chart) phải thấy data card (Metabase: xem dashboard ⇒ xem data card, transitively).
1. `require_object(&*st.pdp, &ctx, "dashboard:read", dashboard_obj)`.
2. Verify card thuộc dashboard (`dashboard_cards.dashboard_id={id} AND id={card_id}`) → else `NotFound`.
3. Load chart của card (`chart_id`) + `params.spec` + dataset; build SQL; chạy server-side; trả rows. **Không cần grant từng chart** — dashboard grant là thẩm quyền (transitive). Áp filter/param dashboard nếu có (reuse logic interpolate param sẵn có server-side, hoặc nhận filter values đã resolve trong body và merge vào filters của spec, validate theo dataset columns).

### 2.5 Ghi chú execution
Data chạy trên **workspace** DB connection (như hiện tại — query chưa per-user; đó là B3). Nên render của shared user dùng connection workspace, chỉ gate bằng object grant. Đây là tenancy model hiện tại; B3 sau downscope per-user token.

## 3. Frontend — render pipeline + sharing modal

### 3.1 Chuyển render pipeline
- **Dashboard card** (`DashboardCardView`→`useChartData`): gọi `POST /dashboards/{id}/cards/{card_id}/data` (trả rows) thay vì `ch_data`+`submitQuery`. Đây là đường shared viewer đi.
- **Xem saved chart** (read-only, không phải editor): gọi `POST /charts/{id}/data`.
- **Chart editor** (`ChartExplore`, preview unsaved/editing): giữ `ch_data`+`submitQuery` (workspace-scoped) — editor thao tác client spec chưa lưu, user có `query:run` workspace.
- `useChartData` thêm selector mode/endpoint (saved-object vs editor). Giữ `ChartRenderer` là dispatch viz duy nhất (theo hướng M2/Md3 của BI review).

### 3.2 Sharing modal (kiểu Metabase)
Action "Share" trên dashboard/chart/dataset/folder mở permissions modal dùng share API B2-1a (`/ws/{ws}/objects/{kind}/{id}/grants`, đã có trong generated client):
- **List** grant trực tiếp (`GET …/grants`, manage-gated) — grantee (user/group) + role (Viewer/Editor).
- **Add**: picker người/group + dropdown Viewer/Editor → `PUT …/grants`.
- **Remove**: `DELETE …/grants/{grant_id}`.
- **Inherited vs direct**: show folder grants của item là inherited (read-only) cạnh direct grants, để user hiểu cascade (gọi `GET …/grants` của folder).
- Chỉ hiện cho manager (list endpoint manage-gated → non-manager không thấy/disable Share).
Theo convention FE (orval hooks, shadcn Dialog, OKLCH token); reuse picker người/group sẵn có nếu có.

## 4. Security

- Endpoint data chỉ chạy **`params.spec` đã lưu của object** — không bao giờ client spec — nên shared user không exfiltrate ngoài query của chart. Endpoint card authorize trên **dashboard** + verify card thuộc dashboard (không cross-dashboard). Filter param dashboard nhận từ client phải validate theo dataset columns (reuse validation sẵn) để không inject predicate.
- Write của share modal manage-gated ở API (B2-1a); UI chỉ phản ánh. List endpoint manage-gated (B2-1a F2) → không lộ ACL cho viewer.
- Regen FE client cho 2 data endpoint mới.

## 5. Testing

- **Backend:** object-guest có `ws_viewer` grant trên chart C → `POST /charts/{C}/data` trả rows (200); chart không grant → 403; endpoint bỏ qua client spec (chỉ chạy saved spec — assert SQL/rows khớp saved chart, không phải body spoof). Card: `ws_viewer` grant trên dashboard D → `POST /dashboards/{D}/cards/{card}/data` trả rows dù không grant chart của card; card_id không thuộc D → 404/403.
- **Frontend:** dashboard card + shared-chart view fetch qua endpoint mới (mock trả rows); editor vẫn dùng `ch_data`; sharing modal list/add/remove grant (mock client), show inherited vs direct, ẩn/disable cho non-manager. Vitest.
- Full gate: BE `cargo test --workspace` (pdp parity không đổi), FE `lint/typecheck/test/build`, OpenAPI + client current.

## 6. Câu hỏi mở (cho plan)

- Chỗ execution server-side: helper nhỏ trong `egret-bi` build connector + run SQL (như `schema.rs`), hay một entry execution reuse của `egret-query` mà egret-bi gọi. Ưu tiên reuse function execution của `egret-query` để BI không re-implement connector/limit/type-coercion.
- Filter/cross-filter dashboard: endpoint card nhận filter values đã resolve trong body (validate), hay B2-1b giữ card không filter (defer tương tác param cho shared viewer)? Mặc định: nhận filter values validate để shared dashboard interactive; validate theo dataset columns.
- Picker người/group: reuse component sẵn (UI workspace-members có) vs mới.
- `datasets` trong share modal: share dataset grant read entity dataset, nhưng chưa có endpoint "data view" dataset ở increment này — làm rõ dataset share truyền gì (định nghĩa/columns) hay bỏ datasets khỏi modal B2-1b. Mặc định: cho share dataset (read định nghĩa) nhưng ghi chú data-preview để sau.

---

*Design v1.0 (2026-07-11). Đóng data-access gap của object-share bằng endpoint data object-scoped chạy server-side (chạy `params.spec` của saved object), và ship sharing modal kiểu Metabase. Data vẫn chạy trên workspace connection (per-user delegation là B3). Một increment (backend + UI).*
