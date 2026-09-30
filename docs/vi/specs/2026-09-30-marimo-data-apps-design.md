# Data App trên marimo — thay thế notebook

> Bản dịch tham khảo. Bản gốc: [`docs/en/specs/2026-09-30-marimo-data-apps-design.md`](../../en/specs/2026-09-30-marimo-data-apps-design.md).
> Khi hai bản mâu thuẫn, bản tiếng Anh thắng.

**Trạng thái:** đã thiết kế
**Ngày:** 2026-09-30
**Thay thế:** notebook Jupyter (`egret-notebook`, nửa Jupyter của `egret-kernel`, `kernel-image/`,
`frontend/src/components/notebook/`)
**Triển khai thành:** một spike (S0) và sáu PR (P1–P6), §9.

Notebook đổi mục đích. Nó không còn là nơi phân tích ad-hoc — việc đó thuộc về SQL Lab. Nó trở thành
nơi tác giả xây **dashboard tuỳ biến cao bằng Python** (một *Data App*), theo kiểu marimo hay Streamlit,
và publish cho người khác dùng. Dashboard kiểu Metabase vẫn là lựa chọn no-code; Data App là lựa chọn
viết bằng code.

Runtime là [marimo](https://github.com/marimo-team/marimo) (Apache-2.0, bản 0.25.0 lúc viết): chế độ
edit là nơi tác giả viết, chế độ run là app đã publish.

## 1. Các quyết định đã chốt với người dùng

| # | Quyết định | Hệ quả |
|---|---|---|
| D1 | **Thay hẳn, không chuyển đổi.** Notebook hiện có bị bỏ. | Không có công cụ import; P6 gỡ tính năng cũ. |
| D2 | **Chỉ truy cập dữ liệu qua SQL** trên engine của workspace (Trino, StarRocks). Không đọc/ghi Iceberg, không catalog token, không Daft. | Image kernel bỏ `pyiceberg`, `daft`, `write_table`, JWT `aud:catalog`. |
| D3 | **App chạy bằng quyền của người xem**, không bao giờ bằng quyền tác giả. | Code của app là không tin cậy đối với người xem; xem §3, §6. |
| D4 | **Người xem là người dùng egret đã đăng nhập** — thành viên workspace và guest được share. Không nhúng ra ngoài, không link công khai. | Người xem nào cũng có danh tính để uỷ quyền. |
| D5 | **Draft + các phiên bản published bất biến.** Bản mới nhất đang chạy; khôi phục = publish lại source cũ. | `app_versions` chỉ ghi thêm. |
| D6 | **SQL từ app chỉ được đọc.** | Bịt lỗ confused deputy (`DELETE` của tác giả chạy bằng quyền admin đang xem). |
| D7 | **Chế độ run: mỗi phiên bản app một pod, mỗi session người xem một tiến trình.** Nếu S0 thất bại: mỗi (app × người xem) một pod. Không bao giờ dùng thread. | Các session không chung bộ nhớ. |
| D8 | **Chế độ edit: mỗi (tác giả × workspace) một pod, concurrency lạc quan** (revision + 409). Không khoá, không cộng tác thời gian thực. | Mỗi kernel chạy bằng quyền của đúng một người. |
| D9 | **Image Python cố định, thư viện chọn sẵn.** Không cài package lúc chạy. | Egress đóng được, chỉ còn tới egret. |
| D10 | **"Notebooks" đổi thành "Apps"** trong menu Explore; quyền `app:read`, `app:write`, `app:manage`. | Bỏ `notebook:*`. |
| D11 | **Dùng UI của chính marimo trong iframe**, phục vụ qua reverse proxy của egret. Không fork frontend, không tự viết UI trên giao thức của nó. | Iframe không theo design OKLCH của egret; theme sáng/tối truyền qua query param. |

## 2. Những gì code nói

Đọc trước khi thiết kế; mỗi điều dưới đây đã định hình thiết kế.

- **Chạy SQL dưới danh tính một người dùng đã có sẵn.** `QueryExecPort::execute_as(ws, database_id,
  principal, sql, limits)` (`crates/egret-query-api/src/port.rs:158`) chạy SQL bằng danh tính uỷ quyền
  của principal để Ranger đánh giá đúng người đó, với `Limits { max_rows, timeout }` và cờ `truncated`.
  Data API đã phục vụ qua nó. Gateway (§4) là một caller mới của port này, không phải đường thực thi mới.
- **Port fail-closed về phạm vi workspace** (`database_id` không thuộc `ws` → `NotFound`) nhưng để việc
  kiểm *quyền* cho caller. Gateway phải tự đánh giá `query:run`.
- **Kernel hiện tại giữ JWT của engine.** `EGRET_NOTEBOOK_RUNTIME` mang `engine_jwt` và `catalog_jwt`
  vào pod (`crates/egret-notebook/src/runtime.rs:77`). Chấp nhận được khi code duy nhất trong kernel là
  của chính người dùng; không chấp nhận được khi code của tác giả chạy cho người xem (D3). Pod mới
  **không giữ credential engine nào**.
- **Workspace chưa có SQL parser.** `sqlparser` là dependency mới (§4.3).
- **`create_asgi_app` của marimo chỉ dùng cho chế độ run** (`marimo/_server/asgi.py:354`). Chế độ edit chỉ
  được phục vụ bởi server `marimo edit`, nên không mount được trong egret; nó phải chạy trong pod.
- **Chế độ run của marimo đặt các session vào thread** của cùng một tiến trình
  (`marimo/_session/managers/kernel.py`; lựa chọn nằm ở `marimo/_session/session.py:136-195`). Code của
  tác giả khi đó có thể chuyển dữ liệu của người xem này sang người xem khác qua state của module. D7 đòi
  mỗi session một tiến trình. Đường có sẵn duy nhất là `--sandbox`, chọn kernel manager subprocess qua
  ZeroMQ (`managers/ipc.py`) nhưng cũng dựng env uv cho mỗi session; cách còn lại là entrypoint của egret
  ép nhánh IPC. Câu hỏi 2 của S0 đo cả hai. (`isolate_apps` không phải: nó cô lập theo *file*, không theo
  session.)
- **Xác thực của marimo là một token dùng chung** (`marimo/_server/api/auth.py`), nhưng nó giữ
  `scope["user"]` từ bên ngoài, và code notebook thấy request qua `mo.app_meta().request` (headers,
  cookies, `user`, `meta`). **Request được làm mới theo từng lệnh kernel**, không chụp một lần: nó là một
  contextvar đặt quanh mỗi lệnh (`marimo/_runtime/kernel_request_handlers.py`), lấy từ handshake
  WebSocket cho lần chạy đầu (`ws_session_connector.py:262`) và từ mỗi lần gọi `/run`,
  `/set_ui_element_value`, `/instantiate` sau đó (`endpoints/execution.py`). Header bắt đầu bằng `marimo`
  / `x-marimo` bị bỏ; `X-Egret-*` đi qua. Đó là kênh theo-session vào kernel.
- **marimo có lớp lưu trữ nhưng là nội bộ và chỉ phủ việc ghi.** `StorageInterface`
  (`marimo/_session/notebook/storage.py:15`) được `AppFileManager(storage=…)` nhận (`file_manager.py:69`),
  mà server chỉ tạo ở một chỗ (`NotebookWorkspace.load`, `marimo/_server/workspace/_base.py:87`). Nhưng
  lúc tải, marimo đọc file thẳng từ đĩa (`load.load_app(path)`, `file_manager.py:285`), và chỉ bước ghi
  cuối của một lần lưu đi qua `storage.write` (`file_manager.py:266`). Lưu mà raise thì thành lỗi HTTP từ
  `POST /api/kernel/save`, và frontend marimo không xử lý gì ngoài việc để notebook ở trạng thái "chưa
  lưu" (`frontend/src/core/saving/save-component.tsx:111`).
- **Cấu hình marimo không tắt được** terminal, cài package, file browser hay secrets; chỉ AI có công tắc
  (`[ai] enabled`). Không có biến `MARIMO_CONFIG` — cấu hình được tìm qua `XDG_CONFIG_HOME` hoặc thư mục
  làm việc (`marimo/_config/utils.py`).
- **marimo không kiểm Origin, Host hay CSRF** và không đặt `X-Frame-Options`/CSP cho trang app; nó đòi
  header `Marimo-Server-Token` trên POST (skew protection), proxy phải cho đi qua.
- **CSRF của egret là double-submit trên cookie host-only** `egret_session` / `egret_csrf`, SameSite=Lax,
  không có tiền tố `__Host-` (`crates/egret-auth/src/cookie.rs`, `csrf.rs`), và egret không có lớp CORS.
  Một subdomain anh em có thể *tung* (toss) cookie có `Domain=` vào egret (§6.1).
- **SQL cell của marimo tìm engine từ các biến trong notebook** (`marimo/_sql/get_engines.py:35`), kể cả
  mọi kết nối DB-API 2.0. Một object do package helper tạo ra là đủ để SQL cell hiển thị nó.
- **Cổng tính năng và quyền cần gỡ:** module tính năng `notebooks`
  (`crates/egret-workspace/src/features.rs:227`, prefix `notebooks`, `notebook-runtime`,
  `notebook-settings`); `notebook:read` / `notebook:write` (`crates/egret-core/src/rbac.rs:394`). Không
  có `notebook:manage`; việc quản lý share hiện mượn `notebook:write`.
- **Task workflow `egret.python` tham chiếu một notebook** (`crates/egret-workflow/src/spec/registry.rs:90`)
  nhưng không có handler. P6 phải quyết số phận của nó (§9).

## 3. Kiến trúc

```text
Trình duyệt — origin egret (vd. egret.example)
  └─ /explore/apps/{id}        header egret  +  <iframe src="https://apps.egret.example/r/{version}/?ticket=…">
Trình duyệt — origin apps (apps.egret.example), bên trong iframe sandbox
  └─ UI marimo (edit hoặc run)
         │  HTTP + WebSocket
egret server
  ├─ App store (Postgres)   apps · app_drafts(revision) · app_versions(chỉ ghi thêm) · app_sessions
  ├─ App proxy (host apps)  ticket→cookie · authz · tạo token session · gắn header · chuyển tiếp HTTP/WS
  ├─ App launcher (k8s)     pod edit theo (tác giả × ws) · pod run theo phiên bản app · reaper
  └─ Query gateway          POST /api/apps/query — token session → execute_as(người xem), chỉ đọc
         ▲
Pod marimo — image `egret-apps`, egress CHỈ tới egret
  ├─ edit: entrypoint egret → server marimo edit, EgretStorage, mỗi notebook một tiến trình kernel
  ├─ run:  entrypoint egret → server marimo run, mỗi session người xem một tiến trình kernel
  └─ egret_marimo (Python): kết nối DB-API + egret.sql() → gateway bằng token session
```

Hai bất biến luôn đúng:

1. **Không pod nào giữ credential tới engine.** Mọi truy cập dữ liệu đi qua gateway bằng token gắn với
   (người dùng, app, phiên bản-hoặc-draft, mode), chỉ đọc, sống ngắn.
2. **Source `.py` chỉ nằm trong Postgres.** Đĩa của pod là bản sao bỏ đi được.

## 4. Query gateway và token session

### 4.1 Token session

Token ngẫu nhiên dạng opaque; chỉ lưu hash, trong `analytics.app_sessions`:

| Cột | Ý nghĩa |
|---|---|
| `token_hash` | PK |
| `user_id`, `workspace_id`, `app_id` | ai, ở đâu, app nào |
| `version_id` nullable | phiên bản published được ghim (run); `NULL` = draft (edit) |
| `mode` | `edit` \| `run` |
| `login_sid` | phiên đăng nhập egret; back-channel logout thu hồi theo nó |
| `created_at`, `last_used_at`, `expires_at` | hết hạn khi rảnh 30 phút, tuyệt đối 8 giờ |
| `revoked_at` | đặt khi WebSocket đóng, logout, hoặc thu hồi thủ công |

- **Proxy tạo token** khi một session marimo được mở (§6.2), và gắn nó vào header `X-Egret-App-Token`
  trên mọi request chuyển tới pod. Mọi header `X-Egret-*` từ client bị xoá trước. Trình duyệt không bao
  giờ thấy token.
- Kernel đọc token qua `mo.app_meta().request.headers`, mà marimo làm mới ở mỗi lệnh kernel (§2), nên
  token một cell thấy là token trên request đã kích hoạt nó. Câu hỏi 1 của S0 xác nhận điều này qua
  proxy, kể cả với cell chạy lại theo reactive do một UI element.
- Token không phải là quyền. **Mỗi lần gọi gateway đều đánh giá lại quyền** (§4.2); gỡ `app:read` của một
  người xem chặn ngay query kế tiếp của họ, không phải session kế tiếp.

### 4.2 `POST /api/apps/query`

Xác thực bằng `Authorization: Bearer <token session>`; miễn CSRF (không có cookie nào tham gia).

```json
{ "database": "trino_prod", "sql": "SELECT …", "params": [], "max_rows": 10000 }
```

Các bước, theo thứ tự, mỗi bước fail-closed:

1. Tra token theo hash; từ chối nếu không có, đã thu hồi hoặc hết hạn (401). Cập nhật `last_used_at`.
2. Kiểm quyền của người dùng trong workspace của token: workspace bật tính năng `apps` (đường dẫn không
   chứa `/ws/{ws}` nên cổng tính năng không tự suy ra được — handler phải tự kiểm), `app:read` (run) hoặc
   `app:write` (edit) trên app của token, và `query:run` trên database.
3. Tra `database` **theo tên** trong workspace của token (404 nếu không có — giống như bị từ chối).
4. Kiểm chỉ đọc (§4.3); từ chối với 400 kèm lý do.
5. `QueryExecPort::execute_as` (hoặc `execute_as_with_params`) bằng người dùng của token, với
   `max_rows = min(yêu cầu, trần cấu hình)` và timeout cấu hình.
6. Audit: app, phiên bản (hoặc draft), người dùng, database, hash câu lệnh, số dòng, thời gian, kết quả.

Phản hồi: `TabularResult` dạng JSON (cột, dòng, `truncated`). Arrow là tối ưu sau.

Mã lỗi giữ các phân biệt của port: `Forbidden` → 403 kèm thông báo của engine (người xem thiếu grant —
điều được chờ đợi theo D3); `NotFound` → 404; `InvalidInput` → 400; `Internal` → 500 không lộ địa chỉ
engine; token hết hạn → 401, helper đổi thành "phiên hết hạn — tải lại app". Mỗi session có rate limit
(cấu hình được, mặc định 10 query/giây, burst 20).

### 4.3 Kiểm tra chỉ đọc

Parse bằng `sqlparser` với dialect gần engine nhất (Trino → `GenericDialect`, StarRocks →
`MySqlDialect`). Chỉ chấp nhận khi đầu vào là **đúng một** câu lệnh loại `Query`, `Explain`,
`ShowTables`/`ShowColumns`/`ShowSchemas`/`ShowCatalogs`/`ShowCreate`, hoặc `Describe`, và `Query` không
chứa CTE sửa dữ liệu. **Mọi thứ parser không parse được đều bị từ chối** — một câu parser không hiểu thì
không thể biết là chỉ đọc. Cú pháp riêng của engine mà parse thất bại là cái giá đã biết; thông báo lỗi
nói rõ để tác giả viết lại.

Đây là lớp phòng thủ bổ sung cho Ranger, không thay thế nó. Nơi engine có chế độ session hoặc transaction
chỉ đọc, P1 đo xem kết nối uỷ quyền có dùng được không và ghi lại kết quả.

### 4.4 Phía Python — `egret_marimo`

Cài trong image `egret-apps`:

- `egret.connect("trino_prod")` → kết nối **DB-API 2.0** mà cursor gọi gateway. marimo nhận nó là
  engine, nên nó xuất hiện trong dropdown engine của SQL cell (câu hỏi 5 của S0).
- `egret.sql("SELECT …", database="trino_prod")` → polars DataFrame, cho code Python.
- `egret.databases()` → các tên người xem được phép truy vấn, dùng cho picker.
- Database được tham chiếu **theo tên**, nên app không mang UUID nội bộ nào.

Hệ quả của D3, ghi ra để không ai báo nhầm là lỗi: **cùng một app cho người xem có grant khác nhau thấy
kết quả khác nhau**, và một cell có thể lỗi 403 với người này mà chạy được với người kia.

## 5. Lưu trữ và vòng đời

P2 thêm các bảng dưới đây; P6 xoá các bảng của notebook (từ migration 0037–0043, 0090, 0091) mà không
chuyển dữ liệu (D1):

| Bảng | Cột |
|---|---|
| `analytics.apps` | `id`, `workspace_id`, `name`, `description`, `folder_id`, `owner_user_id`, `created_at`, `updated_at`, `deleted_at` |
| `analytics.app_drafts` | `app_id` PK/FK, `source text` (≤ 1 MiB, CHECK), `revision bigint`, `updated_by`, `updated_at` |
| `analytics.app_versions` | `id`, `app_id`, `version_no` (UNIQUE theo app), `source`, `from_revision`, `published_by`, `published_at` |
| `analytics.app_sessions` | §4.1 |

- **`app_versions` chỉ ghi thêm theo cấu trúc**: trigger báo lỗi khi `UPDATE` và `DELETE`. Xoá app là xoá
  mềm trên `apps`; các phiên bản vẫn còn.
- **API draft.** `GET /api/ws/{ws}/apps/{id}/draft` trả `source` và `revision` (ETag). `PUT` bắt buộc
  `If-Match: <revision>`; lệch → **409** kèm revision hiện tại; thành công thì revision tăng trong cùng
  câu lệnh (`UPDATE … WHERE revision = $n RETURNING`).
- **Publish** chép draft ở revision hiện tại thành `version_no + 1`. Header egret hiển thị "Publish bản
  nháp rev N" để tác giả biết chính xác mình publish gì. Bản đầu không kiểm tra hợp lệ lúc publish; bản
  hỏng thì khôi phục.
- **Khôi phục** không di chuyển con trỏ: nó publish source của phiên bản cũ thành một phiên bản mới. Lịch
  sử thẳng một đường, và "bản mới nhất là bản đang chạy" (D5) vẫn đúng.
- Folder, thùng rác và bookmark dùng lại các cài đặt chung của notebook.

### 5.1 Chế độ edit: `EgretStorage`

Pod không chạy `marimo edit` trực tiếp. Nó chạy một entrypoint của egret, khởi động server marimo edit
với `EgretStorage` cài đặt `StorageInterface` của marimo:

- **Khi mở**, `NotebookWorkspace.load` đã được entrypoint patch sẽ lấy draft (`GET …/draft`), ghi ra đĩa
  của pod, nhớ revision, rồi mới tạo `AppFileManager` — vì marimo tải từ đĩa, không qua storage.
- `write(path, source)` → `PUT …/draft` kèm `If-Match`, rồi ghi bản cục bộ. Lỗi 409 được raise.
- `exists`, `rename` → ánh xạ tới app; rename bị từ chối (đổi tên ở egret, không ở marimo).
- **409 phải hiện ra được.** Frontend marimo không báo lưu thất bại ngoài việc để notebook "chưa lưu"
  (§2). Câu hỏi 3 của S0 đo tác giả thực sự thấy gì; nếu không thấy gì, header egret (ngoài iframe) hiện
  "draft này đã được người khác lưu — tải lại", dựa trên việc poll revision của draft mà header vốn cần
  cho "Publish bản nháp rev N".
- Lớp lưu trữ xác thực bằng danh tính tác giả của pod (kernel token theo pod hiện có) và egret kiểm
  `app:write` ở mỗi lần gọi.

`StorageInterface` là **nội bộ** của marimo, nên: image ghim chính xác phiên bản marimo; CI của image có
test hợp đồng (lưu, đọc lại, 409) với nó; nếu một lần nâng cấp làm vỡ nó, phương án lùi đã ghi là một agent
theo dõi file trong pod, đồng bộ lên API draft. Phương án lùi được ghi ra, không làm trước.

Đã loại: chặn `/api/kernel/save` tại proxy. Request đó mang danh sách cell, không phải file `.py`, nên
egret sẽ phải tự sinh lại định dạng file của marimo — một bản sao lệch dần sau mỗi lần nâng cấp.

### 5.2 Chế độ run

Mỗi pod run phục vụ đúng một phiên bản đã publish. Khi khởi động nó lấy source của phiên bản đó một lần,
bằng token theo pod chỉ đọc được đúng phiên bản đó, ghi ra đĩa cục bộ và không bao giờ ghi ngược. Phiên
bản mới có pod mới; session mở trước khi publish tiếp tục chạy trên phiên bản cũ cho tới khi đóng, và pod
cũ dừng khi rảnh.

## 6. Runtime, proxy và ranh giới bảo mật

### 6.1 marimo không được chung origin với egret

Ở chế độ run, tác giả có thể xuất HTML và JavaScript tuỳ ý (`mo.Html`, anywidget). Nếu iframe chung origin
với egret, script đó đọc được document cha và gọi mọi API egret bằng cookie của người xem — kể cả API
admin. Đó là XSS vào egret, và D3 khiến tác giả là không tin cậy đối với người xem. Vì vậy:

- marimo được phục vụ từ **một origin riêng** (vd. `apps.egret.example`; ở local là `apps.egret.localhost`
  cạnh `egret.localhost` — **không phải** một cổng khác, vì cookie không tách theo cổng) trong
  `<iframe sandbox="allow-scripts allow-same-origin allow-forms allow-downloads">`. `allow-same-origin`
  là so với origin *apps*, không phải của egret.
- Cookie phiên của egret không bao giờ được gửi tới origin apps.
- Để mở iframe, egret phát **ticket dùng một lần** (TTL 30 giây, gắn với người dùng, app, mode, và phiên
  bản hoặc draft). Origin apps đổi nó tại `/_ticket` lấy cookie của origin apps (HttpOnly, Secure,
  SameSite=Lax, cùng phạm vi gắn kết). Token session ở §4.1 được tạo từ lần đổi đó.
- **Origin apps là một subdomain anh em, tức same-site với egret.** Buộc phải thế: cookie trong iframe
  cross-site là cookie bên thứ ba, bị trình duyệt chặn. Same-site có một cái giá — script trên origin apps
  có thể đặt cookie với `Domain=<domain cha của egret>` mà egret sẽ nhận (*cookie tossing*), có thể che
  `egret_csrf` và phá CSRF double-submit. Vì vậy P4 còn:
  - đổi tên cookie egret thành `__Host-egret_session` / `__Host-egret_csrf` (tiền tố `__Host-` cấm thuộc
    tính `Domain`, nên cookie bị tung vào không dùng được tên đó);
  - giữ egret không có bất kỳ cho phép CORS nào cho origin apps (request cross-origin mang `X-Egret-CSRF`
    cần preflight, và preflight sẽ thất bại).
- Việc này thêm một host vào ingress, Helm, kind và cấu hình OIDC. Câu hỏi 4 của S0 phải chứng minh nó
  chạy trọn vẹn; nếu không được thì không có phương án lùi nào ngoài xem lại kiến trúc.

### 6.2 Proxy

Một service axum trong egret, lắng nghe trên host apps:

- Chuyển tiếp HTTP và WebSocket; xoá header hop-by-hop và mọi header `X-Egret-*` từ client; cho
  `Marimo-Server-Token` đi qua (skew protection).
- **Chặn theo route** những gì marimo không tắt được (§2): WebSocket terminal, endpoint cài package,
  endpoint secrets, và endpoint file browser ngoài những cái editor cần để lưu. Câu hỏi 6 của S0 cho ra
  danh sách chính xác.
- Gắn token marimo của pod (`--token` của marimo; không bao giờ hiện cho người dùng) và token session.
- **Edit:** `/e/{pod}/…` chỉ tới pod edit của chính người gọi; app nêu trong `?file=` yêu cầu `app:write`
  (`EgretStorage` kiểm lại lần nữa).
- **Run:** `/r/{version}/…` yêu cầu `app:read` trên app của phiên bản.
- Thu hồi token session khi WebSocket của session đóng.

### 6.3 Pod

| | Edit | Run |
|---|---|---|
| Khoá | (tác giả, workspace) | phiên bản app |
| Server | entrypoint egret → `marimo edit`, `EgretStorage` | entrypoint egret → `marimo run`, `include_code=False` |
| Kernel | mỗi notebook đang mở một tiến trình | mỗi session người xem một tiến trình (D7) |
| Vòng đời | reaper khi rảnh, như hiện nay | dừng sau 15 phút không có session; egret hiện màn hình chờ lúc cold start |
| Giới hạn | như hiện nay (250m/512Mi → 1000m/2Gi), cấu hình được | cấu hình được; tối đa 20 session mỗi pod, vượt thì 503 "app đang bận" |

- `KernelLauncher` (`crates/egret-kernel/src/launcher.rs`) cùng các cài đặt k8s và subprocess của nó được
  tổng quát hoá để khởi chạy cả hai loại pod; reaper và hardening pod hiện có (non-root, drop ALL,
  seccomp RuntimeDefault) giữ nguyên.
- **NetworkPolicy:** ingress chỉ từ egret; egress chỉ tới gateway của egret và DNS.
- **Cấu hình marimo** (qua `XDG_CONFIG_HOME` trong image) đặt `[ai] enabled = false`; mọi thứ khác bị chặn
  ở proxy (§6.2). Cài package đằng nào cũng thất bại vì egress đã đóng.
- **Người xem không thấy code** (`include_code=False`); người có `app:write` xem code ở chế độ edit.
- Không autoscale ở bản đầu.

## 7. Frontend

- Mục "Notebooks" trong menu Explore thành **"Apps"**. Danh sách hiển thị trạng thái mỗi app (chỉ có draft
  / published vN) và hai hành động **Open** (run) và **Edit** (edit, cần `app:write`).
- Cả hai là route toàn trang của egret: header egret (tên, trạng thái, Publish, Versions, Share) phía trên
  iframe. Panel Versions liệt kê các phiên bản và cho khôi phục.
- Theme truyền cho marimo qua query param.
- Frontend notebook (`NotebookPage.tsx`, `components/notebook/`, `lib/kernel/`, ≈ 4.9k LOC cộng test) bị
  xoá ở P6.

## 8. Quyền và cổng tính năng

- Quyền mới: `app:read` (mở app đã publish), `app:write` (sửa draft, publish, khôi phục), `app:manage`
  (quản lý share). Share dùng hệ object-shares chung với loại object `app`.
- Module tính năng workspace `notebooks` được thay bằng `apps`, phủ `apps`, `apps-runtime`.
- `notebook:read` / `notebook:write` bị gỡ ở P6.

## 9. Triển khai

### S0 — spike (code bỏ đi, sản phẩm là một báo cáo viết)

| # | Câu hỏi | Nếu không được |
|---|---|---|
| 1 | Header do proxy gắn có tới kernel của đúng session qua `mo.app_meta().request` không, kể cả khi chạy lại theo reactive? | Truyền token qua query param khi tạo session |
| 2 | Chế độ run có chạy mỗi session một tiến trình được không — `--sandbox` offline, hoặc entrypoint ép nhánh IPC — và tốn bao nhiêu thời gian khởi động? | Mỗi (app × người xem) một pod |
| 3 | Load đã patch + `EgretStorage` có đi trọn vòng một draft không, và tác giả thấy gì khi 409? | Agent đồng bộ theo dõi file; header egret hiển thị 409 |
| 4 | marimo có chạy được sau proxy với base URL và WebSocket, trên origin riêng, với ticket → cookie không? | Không có — xem lại kiến trúc |
| 5 | SQL cell có nhận kết nối DB-API của `egret_marimo` làm engine không? | Một lớp engine marimo riêng |
| 6 | Proxy phải chặn những route nào (terminal, package, secrets, file browser) mà không làm hỏng edit và lưu? | — (đây là phép đo; proxy chặn dù thế nào) |

### Các PR, theo thứ tự

1. **P1 — Query gateway.** `app_sessions`, bộ kiểm chỉ đọc, `POST /api/apps/query`, audit, rate limit.
   Chỉ backend; test được bằng token seed sẵn, không cần marimo.
2. **P2 — Lưu trữ và vòng đời.** `apps`, `app_drafts`, `app_versions` (+ trigger), API draft với
   revision/409, publish, khôi phục, quyền `app:*`, module tính năng `apps`.
3. **P3 — Image `egret-apps` và `egret_marimo`.** Entrypoint, `EgretStorage`, DB-API, `egret.sql`, cấu hình
   marimo đã khoá; pytest với một egret giả, kể cả test hợp đồng `StorageInterface`.
4. **P4 — Origin apps, proxy, chế độ edit.** Ticket → cookie, proxy (có chặn route), đổi tên cookie sang
   `__Host-`, launcher edit, trang danh sách Apps và trang Edit.
5. **P5 — Chế độ run.** Launcher theo phiên bản, UI publish/versions/khôi phục, trang xem app.
6. **P6 — Gỡ notebook.** `egret-notebook`, nửa Jupyter của `egret-kernel`, `kernel-image/`, frontend
   notebook, các bảng notebook và `notebook:*`. Task workflow `egret.python` — đang tham chiếu notebook và
   chưa có handler — được trỏ sang một phiên bản app hoặc đánh dấu chưa hỗ trợ, quyết định trong plan P6.

## 10. Kiểm thử

Các thuộc tính bảo mật là tiêu chí nghiệm thu. Mỗi thuộc tính có một test, và mỗi test được chứng minh là
fail khi code nó bảo vệ bị cố ý làm hỏng.

- `X-Egret-App-Token` (và mọi `X-Egret-*`) do client gửi không bao giờ tới pod.
- Gateway từ chối `DELETE`, `INSERT`, `GRANT`, `SELECT 1; DROP TABLE t`, CTE sửa dữ liệu, câu lệnh giấu sau
  comment, và mọi thứ không parse được.
- Token mất hiệu lực sau logout, sau khi gỡ `app:read`, và sau khi WebSocket của nó đóng.
- **JavaScript trong app không đọc được cookie của egret và không gọi được API egret** — browser test với
  một app cố thử, kể cả tung một `egret_csrf` có `Domain=` rồi POST tới egret.
- **Hai người xem có grant Ranger khác nhau thấy các dòng khác nhau từ cùng một app**, trên Trino thật và
  StarRocks thật trên kind.
- Admin mở một app chạy `DELETE` nhận 400 và bảng không đổi.
- `UPDATE` và `DELETE` trên `app_versions` thất bại.
- Hai người sửa cùng một draft: người thứ hai nhận 409, và marimo hiển thị nó.
- Kiểm thử sống bằng trình duyệt trên kind cho P4 và P5.

## 11. Hoàn thành khi

- Tác giả xây một app có SQL cell và widget UI trên Trino và trên StarRocks, rồi publish.
- Hai người xem thấy kết quả theo quyền của chính họ.
- Admin mở một app độc hại không ghi được dữ liệu và không chạm được API egret.
- Notebook Jupyter đã được gỡ.

## 12. Ngoài phạm vi

Cộng tác thời gian thực; dependency theo app (PEP 723 với mirror nội bộ — bước tiếp theo tự nhiên, và
không đổi kiến trúc này); nhúng ra ngoài egret; link công khai; chạy hoặc export app theo lịch; truyền
dữ liệu dạng Arrow; autoscale; chuyển đổi notebook cũ.
