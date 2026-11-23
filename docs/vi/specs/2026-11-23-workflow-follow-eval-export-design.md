# Workflow gói 7e — follow, eval, thống kê & export (thiết kế)

**Trạng thái:** thiết kế, chờ duyệt (brainstorm 2026-11-23).
**Gói:** #7 execution operations, sub-package **7e** (sau 7a #860, 7b #866, 7c #868, 7d #872).
**Branch:** `feat/workflow-follow-eval-export` (worktree `egret-execops`, đã sync lên origin/main `7fd2e5e59`).
**Migration:** không — mọi endpoint đều read-only.

## Mục tiêu

Port phần read-side còn lại của `ExecutionController` và `LogController` Kestra vào
egret: **follow** trực tiếp một execution và log của nó qua Server-Sent Events,
**eval** tùy biến một biểu thức Pebble trong render context của execution (hoặc task
run), và các endpoint **thống kê/liệt kê** nhỏ mà UI dựa vào — distinct
namespaces/flows, thời lượng trung bình của một flow, execution mới nhất mỗi flow,
và xuất danh sách execution ra CSV.

Một endpoint Kestra trong vùng này, `GET /{id}/follow-dependencies`, được **hoãn**
(chính SDK Kestra đánh dấu `x-skipped`, và nó cần duyệt cây subflow cộng một stream
thứ ba); ghi lại làm follow-up, không ship ở đây.

## Phạm vi

Ship (đường dẫn egret — egret không dùng segment `/actions/` của Kestra):

| # | egret path | Method | Nguồn Kestra |
|---|------------|--------|--------------|
| 1a | `POST /api/ws/{ws}/workflows/executions/{id}/eval` | POST `text/plain` | `POST /{id}/actions/eval` |
| 1b | `POST /api/ws/{ws}/workflows/executions/{id}/task-runs/{taskRunId}/eval` | POST `text/plain` | `POST /{id}/actions/eval/{taskRunId}` |
| 2 | `GET /api/ws/{ws}/workflows/executions/{id}/follow` | GET SSE | `GET /{id}/follow` |
| 3 | `GET /api/ws/{ws}/workflows/executions/{id}/logs/follow` | GET SSE | `GET /logs/{id}/follow` |
| 5a | `GET /api/ws/{ws}/workflows/executions/namespaces` | GET | `GET /namespaces` |
| 5b | `GET /api/ws/{ws}/workflows/executions/namespaces/{ns}/flows` | GET | `GET /namespaces/{ns}/flows` |
| 6 | `GET /api/ws/{ws}/workflows/executions/namespaces/{ns}/flows/{flowId}/average-duration` | GET | `GET /namespaces/{ns}/flows/{flowId}/average-duration` |
| 7 | `POST /api/ws/{ws}/workflows/executions/latest` | POST | `POST /latest` |
| 8 | `GET /api/ws/{ws}/workflows/executions/export.csv` | GET `text/csv` | `GET /export/by-query/csv` |

Hoãn (follow-up): `#4` follow-dependencies.

## Kiến trúc

Tất cả handler nằm trong `crates/egret-workflow/src/routes.rs` cạnh các route
execution hiện có, đăng ký trong cùng khối router, và document bằng `utoipa::path`
hệt như các route lân cận. Phần hỗ trợ mới:

- **Helper SSE producer** (mới, nhỏ): một response `axum::response::sse::Sse` dựng
  từ `tokio_stream` chạy vòng poll. egret hiện **chưa có** SSE *producer* phía
  server — `http/sse.rs` là *parser* phía client từ gói #4, không liên quan. Log
  follow của engine-pod dùng WebSocket và là subsystem riêng; không tái dùng.
- **Eval** tái dùng bộ render có sẵn (`RenderContext` / analog `VariableRenderer`
  mà outputs/SLA dùng). Lớp HTTP chỉ load execution (và task run), dựng render
  context, render một chuỗi, và ánh xạ lỗi render vào trường `error`.
- **Repo methods** (`crates/egret-workflow/src/repo.rs`) cho distinct, latest,
  average-duration và cursor export — đều là `SELECT` thuần.

### Poll→SSE (quyết định Q1)

egret không có event bus cho execution (frontend hiện poll
`GET …/executions/{id}/logs?after=` qua `useLogFollow`). Thay vì dựng Postgres
`LISTEN/NOTIFY` push vào executor và log writer, các handler follow sẽ **poll DB
theo interval cố định và chỉ phát event khi trạng thái quan sát được thay đổi**,
giữ đúng *shape* endpoint Kestra (`text/event-stream`, browser `EventSource` dùng
được) mà không cần hạ tầng mới.

Cơ chế (cả hai stream follow):
- Khi connect, resolve execution dưới `workflow:read`; execution không tồn tại hoặc
  thuộc workspace khác → **404 TRƯỚC khi mở stream** (không phải event lỗi).
- Phát event đầu với snapshot hiện tại, rồi poll mỗi **1s** (hằng số đặt tên; chỉnh
  được). Chỉ phát event mới khi snapshot khác lần phát trước.
- **Follow execution (#2):** snapshot là execution DTO (cùng shape
  `GET /executions/{id}`). Khi execution tới trạng thái terminal, phát một event
  cuối rồi **đóng stream**. `followExecution` của Kestra cũng đóng khi terminal.
- **Follow logs (#3):** tái dùng watermark phía server của tail `?after=`. Mỗi lần
  poll lấy các dòng sau offset đã phát và phát chúng; stream đóng khi execution
  terminal **và** tail log đã rút cạn (đúng quy tắc "settle watermark" gói 7d dùng
  cho log download).
- Gửi comment keep-alive SSE (axum `KeepAlive`) để connection nhàn rỗi sống qua
  proxy.
- Query poll là một `SELECT` có index mỗi interval mỗi stream; chi phí tuyến tính
  theo số stream mở, chấp nhận được với mẫu một-người-xem-mỗi-execution của UI.

### Eval (#1, quyết định Q2)

- Body là biểu thức Pebble thô (`text/plain`), khớp Kestra.
- Response: `{ "result": string | null, "error": string | null }` (`EvalResult`
  của Kestra). Render thành công điền `result`; lỗi tương đương
  `IllegalVariableEvaluationException` điền `error` và vẫn trả **200** (Kestra trả
  lỗi in-band, không phải lỗi HTTP).
- **Secret bị seal ở CẢ 2 path** — cấp execution và cấp task-run — chặt hơn Kestra
  (Kestra chỉ seal path task-run). Ngăn caller `workflow:read` exfil
  `{{ secret('X') }}`. Ghi lại làm divergence có chủ đích.
- Gate: `workflow:read` (eval là inspect, không chạy task). Gate creator `egret.sql`
  không áp dụng (không có task run).
- `1b` resolve task run trong execution; `taskRunId` không rõ → **404**.

### Thống kê & export

- **distinct (#5):** `SELECT DISTINCT namespace …` và `SELECT DISTINCT flow_id
  WHERE namespace = $1 …`, scope theo workspace, có order, cho dropdown filter.
- **average-duration (#6):** SQL trực tiếp trên các execution đã tới trạng thái
  terminal và có cả mốc start lẫn end trong **30 ngày** gần nhất
  (`AVERAGE_DURATION_LOOKBACK = Duration.ofDays(30)` của Kestra):
  `SELECT AVG(<end> - <start>), COUNT(*)` (tên cột timestamp chính xác xác nhận
  với schema trong plan). Response
  `{ "avgDurationMs": number | null, "count": number }`; `avgDurationMs` là `null`
  khi `count = 0`. Thay cho subsystem daily-statistics của Kestra (egret không có);
  một `AVG` phẳng chính là trung bình có trọng số theo count mà Kestra tính bằng
  cách cộng bucket, nên con số khớp.
- **latest (#7):** body là `[{ "namespace": string, "id": string }]` (`FlowFilter`
  của Kestra, `id` là flow id). Response là danh sách
  `{ id, flowId, namespace, startDate, status }` (`LastExecutionResponse`):
  execution mới nhất mỗi flow qua `DISTINCT ON (flow_id) … ORDER BY flow_id,
  created_at DESC`. Flow không có execution thì bỏ qua.
- **export (#8):** `GET …/executions/export.csv` tái dùng `ExecutionFilter` (cùng
  query params như `list_executions`), stream dạng `text/csv` với
  `Content-Disposition: attachment; filename=executions.csv`. Một hàng header cộng
  một hàng mỗi execution — cột phẳng phản chiếu các trường `ExecutionListItemDto`
  (id, namespace, flow_id, flow_key, state, created_at, started_at, ended_at,
  duration_ms, labels; tập cột chính xác chốt trong plan). Stream qua cursor để
  export lớn không buffer trong bộ nhớ.

## Bảo mật

- Mọi endpoint gate `workflow:read`; không endpoint nào chạy task, nên không có
  gate creator `egret.sql` và không cần `workflow:manage`.
- Eval seal secret cả hai path (ở trên).
- Export áp dụng đúng workspace scoping và filter authz như `list_executions`.
- SSE resolve và authorize execution **trước** khi mở stream; cross-workspace hoặc
  không tồn tại → 404.

## Frontend

1. **Eval panel** — ô nhập biểu thức trong execution view và task-run view; post
   tới `1a`/`1b`, hiện `result` hoặc `error`.
2. **Nút export CSV** trên execution list, mang filter hiện hành vào `export.csv`.
3. **Progress average-duration** — ước lượng tiến độ cho execution đang chạy từ
   `#6`.
4. **Filter dựa distinct** — dropdown namespace/flow lấy từ `#5`.
5. **follow** — execution view và log view chuyển sang `EventSource` tới `#2`/`#3`;
   polling `useLogFollow` hiện có giữ làm fallback nếu SSE lỗi.

## Kiểm thử

`crates/egret-workflow/tests/executions_routes.rs` (không migration, không golden):

- **eval:** biểu thức literal trả `result`; biểu thức hỏng trả `error` với 200;
  `{{ secret('X') }}` bị **seal** ở cả path execution và task-run; `taskRunId`
  không rõ → 404; execution không rõ → 404.
- **follow execution:** phát snapshot đầu; chỉ phát lại khi đổi; phát event cuối và
  đóng khi terminal; execution không tồn tại/cross-workspace → 404 trước khi mở
  stream.
- **follow logs:** phát các dòng nối thêm sau watermark; đóng khi terminal và tail
  đã rút cạn.
- **distinct:** namespaces và flows theo namespace là distinct, scope workspace, có
  order.
- **average-duration:** `null`/`count=0` khi không có execution terminated trong
  cửa sổ; giá trị có trọng số theo count khi có; tôn trọng cửa sổ 30 ngày.
- **latest:** execution mới nhất mỗi flow; flow không có thì bỏ; body rỗng → danh
  sách rỗng.
- **export:** header CSV + một hàng mỗi execution khớp; `Content-Disposition`
  attachment; filter thu hẹp các hàng.

## Divergences so với Kestra (ghi trong code + §6 của ghi chú khi merge)

1. Đường dẫn bỏ `/actions/` (quy ước egret).
2. Eval seal secret ở **cả hai** path (Kestra chỉ seal path task-run).
3. average-duration là `AVG` trực tiếp trên cửa sổ 30 ngày, không phải subsystem
   daily-statistics (egret không có); con số tương đương.
4. follow là **poll→SSE** (interval 1s), không phải event bus reactive; shape
   endpoint y hệt.
5. `#4` follow-dependencies được hoãn.

## Quyết định mở (đã chọn mặc định; xác nhận khi review)

- **Q1 SSE transport:** poll→SSE. ✅ (user)
- **Q2 eval security:** seal cả hai path, gate `workflow:read`. ✅ (user)
- **Q3 phạm vi:** hoãn `#4`; ship `#1,2,3,5,6,7,8`. ✅ (user)
- **Q4 poll interval:** 1s cho cả hai stream follow (hằng số đặt tên). *(mặc định)*
- **Q5 cột export:** cột phẳng của execution-list (ở trên). *(mặc định)*
- **Q6 phạm vi FE:** cả năm mục. ✅ (user, review thiết kế)
