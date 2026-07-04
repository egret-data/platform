# Trang monitor Trino native (Phase 1.6b, 2026-07-04)

Bản EN (canonical): `docs/en/specs/2026-07-04-trino-monitor-design.md`.
Nối tiếp Phase 1.6a (#145).

**Mục tiêu:** UI monitor Trino native (style egret) — tab **Monitor** trên trang engine
detail: query list, detail + **live stage/split visualizer**, kill query, cluster/node
stats — do control-plane egret gọi REST API coordinator. egret = "1 cổng" xem engine;
KHÔNG proxy Web UI của Trino.

**Non-goals:** proxy Web UI Trino; autoscaling (1.5.4); observability export (1.5.5);
pod/scheduling (1.5.3); lưu lịch sử query (chỉ đọc query hiện tại/recent của coordinator).

## A. Reachability — `KubeClient::coordinator_request`
Thêm method `coordinator_request(ns, service, method, path, headers, body) -> (u16, Vec<u8>)`:
- **In-cluster**: HTTP thẳng `http://{engine}-trino.{ns}.svc:8080{path}`.
- **Dev (host+kubeconfig)**: qua **kube API-server proxy** `/api/v1/namespaces/{ns}/services/{svc}:8080/proxy{path}`.
- LiveKube tự chọn (biết in_cluster vs from_kubeconfig); FakeKube canned cho test.

## B. Monitor credential
- **Engine no-auth**: gọi với `X-Trino-User: egret-monitor` (không access-control ⇒ full quyền).
- **Engine có auth (1.6a)**: control-plane auto thêm user `egret-monitor`, password **derive
  từ engine.id** (`egret-monitor-{id}` — không cần lưu; provisioning bcrypt vào password.db,
  monitor re-derive). Gọi `Authorization: Basic egret-monitor:<derived>` + `X-Forwarded-Proto: https`.
  Chỉ thêm khi engine đã bật auth — KHÔNG ép auth lên engine no-auth.

## C. Backend routes (control-plane)
`GET /engines/{id}/queries` (→ `/v1/query`) · `GET .../queries/{qid}` (→ `/v1/query/{qid}`, có outputStage) ·
`DELETE .../queries/{qid}` (→ kill) · `GET .../cluster` (→ `/v1/node` + tally state từ `/v1/query`).
Map JSON Trino → DTO egret (QueryDto{id,user,state,query,elapsedMs,cpuMs,memoryBytes,progressPercent},
QueryDetailDto + errorMessage?/outputStage, ClusterDto{nodes,running,queued,blocked}). Non-2xx → 502 UI báo.

## D. Frontend — Monitor tab (EngineDetail)
- Query list (poll ~3s): id/user/state(badge)/SQL preview/elapsed/memory/CPU/progress; filter state+search.
- Query detail: full SQL + stats + error + **live stage/split visualizer** (cây stage→task từ outputStage,
  poll khi RUNNING) + nút **Kill**.
- Cluster panel: running/queued/blocked + node list. Style giống Trino Web UI.

## E. Test
Unit (map JSON fixtures → DTO; coordinator_request chọn đúng path qua FakeKube; derive monitor cred +
bcrypt vào password.db khi có auth) + live kind (chạy query chậm trên demo2 → list thấy RUNNING, detail có
stages, kill được, cluster liệt kê node).

Compat: không migration; monitor cred derive không lưu; chỉ đọc query live/recent (không persist egret-side).

Chi tiết đầy đủ xem bản EN.
