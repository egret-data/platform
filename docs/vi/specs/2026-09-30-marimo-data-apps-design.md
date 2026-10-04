# Data App trên marimo — thay thế notebook

> Bản dịch tham khảo. Bản gốc: [`docs/en/specs/2026-09-30-marimo-data-apps-design.md`](../../en/specs/2026-09-30-marimo-data-apps-design.md).
> Khi hai bản mâu thuẫn, bản tiếng Anh thắng.

**Trạng thái:** đã thiết kế; spike S0 và S0b đã xong (§9)
**Ngày:** 2026-09-30
**Thay thế:** notebook Jupyter (`egret-notebook`, nửa Jupyter của `egret-kernel`, `kernel-image/`,
`frontend/src/components/notebook/`)
**Triển khai thành:** hai spike (S0, S0b) và sáu PR (P1–P6), §9.

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
| D7 | **Chế độ run: mỗi phiên bản app một pod, mỗi session người xem một tiến trình.** Không bao giờ dùng thread. S0 cho thấy `--sandbox` của marimo không cho điều này (nó cô lập theo server); thay vào đó entrypoint của egret ép nhánh kernel IPC của marimo (§2, §6.3). S0b: kernel của mỗi người xem có thể lấy từ một tiến trình khởi động sẵn (warm pool hoặc fork server) nằm sau interpreter mà marimo được cấu hình để khởi chạy; chọn cái nào được quyết ở P5 trên Linux (§6.3). Nếu bản patch ngừng chạy sau một lần nâng cấp: mỗi (app × người xem) một pod. | Các session không chung bộ nhớ. Entrypoint patch API nội bộ của marimo: ghim chính xác phiên bản + test hợp đồng. Đầu kernel của một launcher khởi động sẵn chạy `marimo._ipc.launch_kernel`, module mà marimo ghi rõ đường dẫn là public; đầu server vẫn cần các patch nội bộ. |
| D8 | **Chế độ edit: mỗi (tác giả × workspace) một pod, concurrency lạc quan** (revision + 409). Không khoá, không cộng tác thời gian thực. | Mỗi kernel chạy bằng quyền của đúng một người. |
| D9 | **Image Python cố định, thư viện chọn sẵn.** Không cài package lúc chạy. | Egress đóng được, chỉ còn tới egret. |
| D10 | **"Notebooks" đổi thành "Apps"** trong menu Explore; quyền `app:read`, `app:write`, `app:manage`. | Bỏ `notebook:*`. |
| D11 | **Dùng UI của chính marimo trong iframe**, phục vụ qua reverse proxy của egret. Không fork frontend, không tự viết UI trên giao thức của nó. | Iframe không theo design OKLCH của egret; theme sáng/tối truyền qua query param. |
| D12 | **Origin apps là một subdomain anh em** (`apps.<host egret>`), same-site với egret (2026-10-01). | P4 phải có cookie `__Host-` (kiểm trên HTTPS), GET không có tác dụng phụ, CSRF dựa trên header, không cho phép CORS cho origin apps (§6.1). |
| D13 | **SQL cell: cài DuckDB, engine egret là mặc định** (2026-10-01). | DuckDB để viết SQL trên DataFrame đã lấy về, chỉ trong pod; SQL cell mới nhắm vào gateway (§4.4). |
| D14 | **Bước kiểm `has_marimo_installed` mỗi lần khởi chạy của marimo do shim của launcher trả lời** (2026-10-01). | Một mẹo dựa vào hành vi nội bộ của marimo: ghim phiên bản marimo, test hợp đồng trong CI của image sẽ fail khi lệnh đổi; hiện chưa theo hướng sửa upstream (§6.3). |
| D15 | **Nghiệm thu thời gian khởi động của P5 tính tương đối so với chế độ thread** (≤ 0,5 giây trung vị, ≤ 1 giây cho 3 đồng thời, Linux, ≥ 5 lần chạy); "< 2 giây" là mục tiêu trải nghiệm riêng, không chặn (2026-10-01). **Đã bị D24 thay thế (2026-10-03):** tiêu chí này so sánh một pool kernel khởi động sẵn với chế độ thread trong MỘT pod dùng chung; D24 bỏ pod dùng chung, nên không còn pool nào để so sánh. | Mức sàn tải trang ≈ 1,75 giây nằm ngoài tầm của launcher kernel (S0b). Xem `docs/en/plans/2026-10-03-data-apps-p5-startup-result.md` cho số đo thay thế (chi phí khởi động nguội của một run pod — chưa đáng tin, đo dưới tải nặng không liên quan). |

### 1.2 D24 — run mode là một pod cho mỗi (viewer, version đã publish), không phải mỗi version (2026-10-03)

| # | Quyết định | Hệ quả |
|---|---|---|
| D24 | Kênh IPC của marimo (`marimo/_ipc/connection.py`) bind socket ZeroMQ không xác thực lên `tcp://127.0.0.1:<cổng ngẫu nhiên>`. Thiết kế run mode ban đầu của spec này — nhiều kernel của nhiều viewer, cô lập bằng patch IPC của P3, dùng chung một pod cho mỗi **version** đã publish — sẽ cho phép code của app (chạy trong kernel của viewer B) chạm tới kênh điều khiển của viewer A trong cùng pod (đọc được `x-egret-app-token` của A, hoặc giả mạo output vào phiên của A): một lỗ rò giữa các viewer, nặng hơn D3 ("code tác giả không đáng tin với viewer của chính nó"). **Đã chốt với người dùng: mỗi run pod chỉ phục vụ một (version, viewer)** — đúng phương án dự phòng D7 đã nêu cho trường hợp patch IPC hỏng, nay dùng làm mặc định thay vì pod dùng chung. | §5.2, §6.3 và D7 bị thay thế ở chỗ mô tả một pod cho mỗi version; giờ một run pod chỉ phục vụ đúng một cặp `(version_id, user_id)`. Patch IPC ép buộc của P3 (`apps-image/egret_marimo/patches.py::install_run_mode_ipc_patch`) vẫn giữ nhưng **tắt mặc định** (`apps.pod.run_kernel_mode = "thread"`): vì mỗi pod chỉ có một viewer, các tab trình duyệt của chính người đó dùng chung kernel chế độ thread của marimo không còn là vấn đề cô lập giữa người khác nhau (đã đo trong `docs/en/plans/2026-10-03-data-apps-p5-startup-result.md` — hai tab, cùng một viewer, chế độ thread, token không bao giờ lẫn). Phải trả giá: mỗi viewer đang xem một pod riêng thay vì nhiều viewer dùng chung. Mục tiêu số của D15 không còn áp dụng (xem dòng đó). Plan: `docs/en/plans/2026-10-03-data-apps-p5-run-mode.md`. |
| D16 | **Việc pool bị treo khi tắt được phủ bằng một test nghiệm thu trong P5**, không làm spike riêng (2026-10-01). | Áp dụng cho launcher mà P5 chọn; trên k8s thứ tự tắt này xảy ra thường xuyên. |

### 1.1 Các quyết định đề xuất cho P4 (2026-10-02) — **chưa thống nhất với người dùng**

Phát sinh khi viết plan P4 (`docs/en/plans/2026-10-02-data-apps-p4-origin-proxy-edit.md`) vì spec chưa nói. D19 và D23 là câu hỏi cho người dùng; phần còn lại là mặc định của plan, đảo lại nếu người dùng không đồng ý.

| # | Đề xuất | Lý do |
|---|---|---|
| D17 | Apps host chạy bằng **listener riêng** (`apps.listen`), chỉ gắn route ticket và proxy. | Một router phân theo Host header khiến một lỗi lộ cả hai bề mặt; hai cổng thì tách bạch về cấu trúc. |
| D18 | **Grant là một ticket đã redeem**: một dòng trong `analytics.app_grants`; cookie của apps origin giữ plaintext có hash là `cookie_hash`. `login_sid` cascade từ `sessions`. | Dùng một lần nguyên tử, logout huỷ grant theo cấu trúc, đúng với nhiều replica egret. |
| D19 | **Token truy vấn cấp theo grant**, do proxy cấp lười; thiếu cache thì cấp mới. Token của grant bị thu hồi khi WebSocket cuối cùng trên replica đó đóng. **Câu hỏi mở:** số WebSocket đang mở đếm theo từng replica, nên với nhiều replica chỉ còn hết hạn idle (30 phút) làm lưới đỡ — chấp nhận, hay đếm chung trong DB? | Token theo cookie của S0 hỏng khi reload iframe (WebSocket cũ đóng đã thu hồi nó). |
| D20 | Listener của egret từ chối mọi request có `Sec-Fetch-Site: same-site`. **Đã đo ở P4e** (xem `docs/en/plans/2026-10-03-data-apps-p4e-live-result.md`): Chromium gửi header này cho JavaScript ở apps origin và egret từ chối mọi request, kể cả `GET` thường mang cookie của người xem; nằm ngoài danh sách của spec. | S0 đo thấy cookie Lax của người xem đi kèm request từ apps origin; kiểm `GET` là cần nhưng chưa đủ. |
| D21 | Cookie là `__Host-egret_session` / `__Host-egret_csrf` khi `session.secure` bật, giữ tên hiện tại nếu không; frontend đọc cả hai. | `__Host-` cần `Secure`; Chromium từ chối qua http (S0). |
| D22 | Edit pod dùng **trait `AppPodLauncher` mới** trong `egret-apps`; không mở rộng `KernelLauncher` của `egret-kernel` (thay cho chữ "generalised" ở §6.3). | P6 xoá nửa Jupyter của `egret-kernel`. |
| D23 | **Chặn grid layout ở chế độ edit** thay vì lưu cùng draft. **Câu hỏi mở:** mất grid layout ở v1 có chấp nhận được không? | Lưu nó cần đổi schema và một hook của marimo chưa tồn tại. |

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
  mỗi session một tiến trình. **Chế độ run của marimo không có sẵn đường nào dẫn tới đó** (câu hỏi 2 của
  S0): với `marimo run` một file, `--sandbox` chạy lại (re-exec) *cả server* bên trong một env uv
  (`marimo/_cli/cli.py:1249-1253`, `_cli/sandbox.py:461`), và server bên trong lại là chế độ thread — 4
  session, 1 pid. Kernel manager subprocess qua ZeroMQ (`managers/ipc.py`) chỉ tới được bằng cách để
  entrypoint của egret patch hai hàm nội bộ: `SessionImpl.create` (`marimo/_session/session.py:91,99`)
  bị ép vào nhánh IPC (`session.py:160`), và `ipc._get_venv_config` (`ipc.py:65`) trỏ tới interpreter
  của image, vì nếu không, IPC manager dựng một env uv cho mỗi session (`ipc.py:331-431`). Với cả hai,
  S0 đo được 4 session → 4 tiến trình kernel. (`isolate_apps` không phải: nó cô lập theo *file*, không
  theo session.)
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
  `POST /api/kernel/save`, và frontend marimo **có** báo lỗi đó: câu hỏi 3 của S0 thấy một toast đỏ "Failed
  to save" mang nội dung `detail` của lỗi, nút lưu vẫn ở trạng thái chưa lưu và buffer đã sửa được giữ.
  (Một lần đọc `frontend/src/core/saving/save-component.tsx:111` trước đó kết luận frontend im lặng; phép
  đo bác bỏ điều đó.)
- **Cấu hình marimo không tắt được** terminal, cài package, file browser hay secrets; chỉ AI có công tắc
  (`[ai] enabled`). Không có biến `MARIMO_CONFIG` — cấu hình được tìm qua `XDG_CONFIG_HOME` hoặc thư mục
  làm việc (`marimo/_config/utils.py`).
- **marimo không kiểm Origin, Host hay CSRF** và không đặt `X-Frame-Options`/CSP cho trang app; nó đòi
  header `Marimo-Server-Token` trên POST (skew protection), proxy phải cho đi qua.
- **CSRF của egret là double-submit trên cookie host-only** `egret_session` / `egret_csrf`, SameSite=Lax,
  không có tiền tố `__Host-` (`crates/egret-auth/src/cookie.rs`, `csrf.rs`), và egret không có lớp CORS.
  Một subdomain anh em có thể *tung* (toss) cookie có `Domain=` vào egret, và request nó gửi tới egret
  mang theo cookie Lax của egret — S0 đã đo cả hai (§6.1).
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
  token một cell thấy là token trên request đã kích hoạt nó. Câu hỏi 1 của S0 đã xác nhận điều này qua
  proxy, kể cả với cell chạy lại theo reactive do một UI element, với session chạy trong thread và với
  session chạy trong các tiến trình kernel IPC riêng (hai người xem, 14 và 12 query qua gateway, không
  query nào lẫn người).
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
   chứa `/ws/{ws}` nên cổng tính năng không tự suy ra được — handler phải tự kiểm), `app:read`
   trên app của token ở chế độ run; `app:write` **và** `query:run` ở chế độ edit. Chế độ run không đòi
   `query:run`: `ws_viewer` không có quyền này, người xem không tự viết được SQL (token không bao giờ tới
   trình duyệt; chỉ code của tác giả gửi truy vấn), và Ranger vẫn quyết định theo từng người xem.
3. Tra `database` **theo tên** trong workspace của token (404 nếu không có — giống như bị từ chối).
4. Kiểm chỉ đọc (§4.3); từ chối với 400 kèm lý do.
5. `QueryExecPort::execute_as` (hoặc `execute_as_with_params`) bằng người dùng của token, với
   `max_rows = min(yêu cầu, trần cấu hình)` và timeout cấu hình.
6. Audit: app, phiên bản (hoặc draft), người dùng, database, hash câu lệnh, số dòng, thời gian, kết quả.

Phản hồi: `TabularResult` dạng JSON (cột, dòng, `truncated`). Arrow là tối ưu sau.

Mã lỗi giữ các phân biệt của port: `Forbidden` → 403 kèm thông báo của engine (người xem thiếu grant —
điều được chờ đợi theo D3); `NotFound` → 404; `InvalidInput` → 400; `Internal` → 500 không lộ địa chỉ
engine; token hết hạn → 401, helper đổi thành "phiên hết hạn — tải lại app". Mỗi session có rate limit
(cấu hình được; như P1 đã làm: 20 lời gọi mỗi cửa sổ 2 giây, và một lời gọi vượt giới hạn giữ token thêm
một cửa sổ).

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
  engine (câu hỏi 5 của S0: `mo.sql(…, engine=db)` chạy qua gateway bằng danh tính người xem). Dropdown
  engine của SQL cell được dựng từ các biến đang sống trong kernel, nên kết nối chỉ hiện ở đó **sau khi
  cell định nghĩa nó đã chạy**; trước đó dropdown chỉ có "DuckDB (In-Memory)".
- **Engine SQL mặc định của marimo là DuckDB**, và một SQL cell không chọn engine sẽ lỗi nếu chưa cài
  `duckdb` và `sqlglot` (`marimo/_sql/sql.py:112-118`). **Đã quyết với người dùng (2026-10-01, D13): cả hai.** Image
  cài `duckdb` + `sqlglot`, để tác giả viết SQL trên các DataFrame đã lấy về (chỉ trong pod; egress đã
  đóng), **và** `egret_marimo` đặt engine egret làm mặc định cho SQL cell mới, để nó nhắm vào gateway bằng
  danh tính người xem. Chưa đo: marimo có cho một thư viện đặt engine SQL mặc định không; P3 tìm hiểu, và
  nếu không được thì mẫu app mới tạo sẵn cell kết nối để engine egret có trong dropdown và được chọn ngay.
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
- **Việc lấy lại khi load không được đặt lại revision mà một editor đang mở giữ.** `NotebookWorkspace.load`
  chạy hai lần mỗi lần mở editor (trang index và session, `marimo/_server/api/endpoints/assets.py:387`,
  `session_manager.py:191,353`) và chạy lại khi tải lại trang, mở tab thứ hai hoặc fetch index. Patch của
  S0 lấy lại draft và gắn lại revision ở mỗi lần gọi; một editor đang giữ revision *n* khi đó sẽ lặng lẽ
  nhận *n + 1* và lần lưu cũ kế tiếp của nó sẽ thành công, ghi đè tác giả kia. P3 gắn revision với session
  đang sửa (hoặc từ chối gắn lại khi một session còn giữ revision cũ hơn) và test đúng chuỗi này.
- `write(path, source)` → `PUT …/draft` kèm `If-Match`, rồi ghi bản cục bộ. Lỗi 409 được raise với thông
  báo nằm trong `detail` của lỗi.
- `exists`, `rename` → ánh xạ tới app; rename bị từ chối (đổi tên ở egret, không ở marimo).
- **409 hiện ngay trong marimo** (câu hỏi 3 của S0): tác giả thấy một toast đỏ "Failed to save" với nội
  dung của egret ("This draft was saved by someone else — reload."), nút lưu vẫn ở trạng thái chưa lưu,
  và buffer đã sửa được giữ. Không cần phương án lùi hiển thị xung đột ở header egret. Việc header poll
  revision của draft chỉ giữ lại nếu "Publish bản nháp rev N" cần.
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

### 5.3 Hợp đồng API phía pod (định nghĩa ở P3, egret phục vụ ở P4)

Đây là các endpoint egret duy nhất mà pod marimo gọi, ngoài query gateway (§4.2). Mỗi endpoint nhận một
**token pod** (`Authorization: Bearer <token pod>`) do egret tạo khi khởi chạy pod. P4 tạo token và
phục vụ các route này; P3 xây client và test nó với một egret giả.

| Pod | Endpoint | Hành vi |
|---|---|---|
| edit (tác giả × workspace) | `GET /api/apps/pod/apps/{app_id}/draft` | `{source, revision}`. egret kiểm lại `app:write` của tác giả trong token ở mỗi lần gọi. |
| edit | `PUT /api/apps/pod/apps/{app_id}/draft` kèm `If-Match: "<rev>"`, body `{source}` | 200 `{revision}` · 409 `{error, current_revision}` · 413 · 428, cùng hợp đồng với API cookie (§5) |
| run (một phiên bản) | `GET /api/apps/pod/apps/{app_id}/versions/{version_id}` | `{source, version_no}`. Token gắn với đúng phiên bản đó. |

Quy tắc phía pod, rút ra từ S0:
- **Revision gốc theo từng session editor, không theo app.** marimo tạo một `AppFileManager` mới ở mỗi
  lần `NotebookWorkspace.load`, và mỗi session editor giữ manager từ lần nạp của chính nó. Vì vậy mỗi lần
  nạp đều tải draft mới nhất, và `EgretStorage` của nó giữ revision của nội dung mà session đó nhận được.
  Session nào có draft đã thay đổi bên dưới sẽ nhận 409 ở lần lưu kế tiếp, và marimo hiện toast. Ở chế độ
  edit marimo giữ một session cho mỗi file, và tải lại trình duyệt thường *tiếp tục* session mồ côi (TTL
  120 giây). Vì vậy pod đánh dấu session là xung đột khi gặp 409 và từ chối tiếp tục nó, và lần tải lại
  tạo một session mới ở revision mới nhất. Tab thứ hai dùng chung session (kiosk hoặc RTC) chứ không tách
  ra. Cách này bắt được editor cũ và tác giả thứ hai. Quy tắc cũ (một revision gốc cho mỗi app suốt vòng đời pod) đã bị bỏ ở P3: revision dùng chung
  không phân biệt được các session, nên một session cũ có thể lưu đè bản mới hơn sau khi một lần nạp khác
  đã tải lại.
- **Pod từ chối khởi động** nếu có `.marimo.toml` trong thư mục làm việc, bất kỳ thư mục cha nào của nó,
  hoặc `$HOME`. marimo ưu tiên các file đó hơn cấu hình đã khoá trong `XDG_CONFIG_HOME`.
- **marimo 0.25.0 không có API đặt engine mặc định.** SQL cell mới dùng kết nối được định nghĩa gần nhất
  mà người dùng chưa chọn khác, nên chính cell `db = egret.connect(...)` của mẫu app mới (P2) là thứ
  khiến engine egret trở thành mặc định (D13).

## 6. Runtime, proxy và ranh giới bảo mật

### 6.1 marimo không được chung origin với egret

Ở chế độ run, JavaScript của tác giả chạy trong trình duyệt của người xem. Câu hỏi 4 của S0 đã đo đường
chạy: một module `_esm` của **anywidget** chạy JS tuỳ ý (theo thiết kế — widget mang theo JS), còn `mo.Html`
được làm sạch (một thẻ `<script>` và một `<img onerror>` đều không chạy). Nếu iframe chung origin với egret,
script đó đọc được document cha và gọi mọi API egret bằng cookie của người xem — kể cả API admin. Đó là XSS
vào egret, và D3 khiến tác giả là không tin cậy đối với người xem. Vì vậy:

- marimo được phục vụ từ **một origin riêng** (vd. `apps.egret.example`; ở local là `apps.egret.localhost`
  cạnh `egret.localhost` — **không phải** một cổng khác, vì cookie không tách theo cổng) trong
  `<iframe sandbox="allow-scripts allow-same-origin allow-forms allow-downloads">`. `allow-same-origin`
  là so với origin *apps*, không phải của egret.
- Cookie của egret là host-only, nên theo thiết kế origin apps không bao giờ giữ chúng. S0 chỉ xác nhận
  một phần: `document.cookie` trên origin apps không có cookie nào của egret, một phép kiểm không thấy
  được cookie HttpOnly.
- Để mở iframe, egret phát **ticket dùng một lần** (TTL 30 giây, gắn với người dùng, app, mode, và phiên
  bản hoặc draft). Origin apps đổi nó tại `/_ticket` lấy cookie của origin apps (HttpOnly, Secure,
  SameSite=Lax, cùng phạm vi gắn kết). Token session ở §4.1 được tạo từ lần đổi đó. S0 đo được ticket chỉ
  dùng một lần (dùng lại → 403), request không có cookie apps → 401, và cookie của session run → 401 ở
  upstream edit. (TTL 30 giây chưa được đo.)
- **S0 đo được JS của tác giả làm được gì và không làm được gì** (Playwright tự động, HeadlessChrome 136,
  một lần chạy, http, với một egret giả mô phỏng cookie và CSRF double-submit của egret): nó không đọc được
  document cha của egret (`SecurityError`), cookie của egret, hay bất kỳ response nào của egret (không có
  cho phép CORS → `TypeError`); POST mang `X-Egret-CSRF` chết ở preflight CORS; session run không tới được
  editor. **Nhưng có hai thứ đã tới egret:**
  - **cookie SameSite=Lax của egret đi theo request từ origin apps.** Một `fetch` từ widget tới `whoami`
    của egret tới nơi kèm cookie phiên của người xem và egret giả thực thi nó bằng danh tính người đó; CORS
    chỉ ngăn script đọc response. Request same-site mang cookie Lax, nên đây là hành vi của trình duyệt,
    không phải đặc thù của bản giả.
  - **một cookie bị tung vào đã tới egret.** Widget đặt `egret_csrf=evil; domain=<domain cha>`, và request
    kế tiếp của egret mang hai giá trị `egret_csrf`, giá trị thật và `evil`. Không thay đổi trạng thái nào
    thành công chỉ vì POST no-cors không thêm được header CSRF.
- **Hệ quả, và yêu cầu cho P4.** Mọi `GET` của egret (hoặc request nào không cần header tuỳ biến) có tác
  dụng phụ, và mọi kiểm CSRF chỉ so *giá trị* cookie mà không cần một header kẻ tấn công không đặt được,
  đều bị khai thác được từ bên trong một app. Vì vậy P4:
  - kiểm toán rằng các route `GET` của egret không có tác dụng phụ, và giữ như vậy (một test fail khi một
    route thay đổi trạng thái nhận `GET`);
  - giữ CSRF **dựa trên header** (header `X-Egret-CSRF` phải có và khớp), không bao giờ chỉ so giá trị;
  - đổi tên cookie egret thành `__Host-egret_session` / `__Host-egret_csrf` (tiền tố `__Host-` cấm thuộc
    tính `Domain`, nên cookie bị tung vào lẽ ra không dùng được tên đó) — **lớp phòng thủ này chưa được
    đo**: qua http, Chromium từ chối lưu hẳn cookie `__Host-` có `Secure`, nên không hình thành được phiên
    của nạn nhân. P4 kiểm nó qua HTTPS trên kind (kỳ vọng: `__Host-egret_csrf` bị tung vào không bao giờ
    tới egret);
  - giữ egret không có bất kỳ cho phép CORS nào cho origin apps (request cross-origin mang `X-Egret-CSRF`
    cần preflight, và preflight sẽ thất bại — S0 đã đo).
- **Origin apps là một subdomain anh em, tức same-site với egret.** Lý do đưa ra là cookie trong iframe
  cross-site là cookie bên thứ ba, bị trình duyệt chặn. S0 cho thấy cái giá của same-site (ở trên). **Câu
  hỏi này đã được quyết với người dùng (2026-10-01, D12): giữ subdomain anh em** và trả giá bằng các việc
  hardening của P4 ở trên (cookie `__Host-` kiểm trên HTTPS, GET không có tác dụng phụ, CSRF dựa trên
  header, không cho phép CORS). Đã loại: một registrable site khác (vd. `egret-apps.example`) — chặn được
  cookie Lax đi theo và cookie tossing, nhưng cookie của origin apps thành cookie bên thứ ba bị chặn và
  ticket → cookie cần cơ chế thay thế; S0 chưa đo. Chưa thử: phục vụ iframe từ chính host egret với
  `sandbox` không có `allow-same-origin` (origin rỗng) — marimo sẽ mất cookie và storage; cần một spike.
- Việc này thêm một host vào ingress, Helm, kind và cấu hình OIDC. Câu hỏi 4 của S0 cho thấy marimo edit và
  run chạy trọn vẹn sau proxy trên một origin riêng với `--base-url`, WebSocket và ticket → cookie, nên
  kiến trúc được giữ.

### 6.2 Proxy

Một service axum trong egret, lắng nghe trên host apps:

- Chuyển tiếp HTTP và WebSocket; xoá header hop-by-hop và mọi header `X-Egret-*` từ client; cho
  `Marimo-Server-Token` đi qua (skew protection).
- **Chặn theo route** những gì marimo không tắt được (§2). Câu hỏi 6 của S0 kiểm kê 107 route của marimo
  0.25.0 và chặn 22 prefix sau, dưới cả `/e` lẫn `/r`, mà edit, lưu, 409 và chế độ run không đổi:
  `/terminal`, `/api/packages`, `/api/secrets`, `/api/files`, `/api/ai`, `/api/storage`, `/api/home`,
  `/api/kernel/copy`, `/api/kernel/rename`, `/api/kernel/install_missing_packages`,
  `/api/kernel/save_user_config`, `/api/export/auto_export`, `/api/export/requirements`, `/api/lsp`,
  `/api/datasources`, `/api/cache`, `/api/environment`, `/api/usage`, `/api/sessions`, `/api/status`,
  `/auth`, `/og`. Lý do cho từng prefix nằm trong kết quả S0. Chính editor gọi `/api/packages/sandbox`,
  `/api/lsp/health`, `/api/usage` và `/api/datasources/discover` và nhận 403 mà không ảnh hưởng việc
  sửa; panel settings không còn lưu được thay đổi.
- **Kiểm và chuyển tiếp cùng một đường dẫn.** Proxy đầu tiên của S0 so khớp một đường dẫn đã chuẩn hoá rồi
  chuyển tiếp một đường dẫn mà httpx lại chuẩn hoá lần nữa. Trong 40 biến thể qua raw socket: 3 `GET` HTTP
  (`/e/x/../api/packages/list` và các dạng `%2e%2e` / `..%2f`) trả 200 kèm danh sách package; 16 dạng
  WebSocket của `/terminal/ws` nhận 101 từ proxy nhưng marimo từ chối URL upstream chưa gộp, nên không tới
  shell; 2 `POST …/x/../api/files/list_files` lọt bước kiểm và tới marimo (401 từ marimo); các 401 của
  `GET` dạng chấm trên `/r` và các 404 của dạng `//` / `API` chưa xác định được nguồn. Proxy lấy
  đường dẫn thô mà client gửi, **từ chối** nếu nó có segment `.` hoặc `..` (kể cả sau một lần giải mã
  phần trăm), `//`, `.` `/` `\` `%` hoặc NUL ở dạng mã hoá, hoặc dấu gạch ngược, còn không thì so khớp
  không phân biệt hoa thường (sau khi giải mã phần trăm) và chuyển tiếp đúng chuỗi đó (đã sửa: 0 trên 40
  lọt — 28 × 403, 12 × 400).
- **Deny-list thì mong manh**: mỗi lần nâng cấp marimo có thể thêm route. P4 nên ưu tiên **allow-list**
  các route mà editor và app run cần (kết quả S0 phân loại mọi route), và chạy lại việc kiểm kê route ở
  mỗi lần nâng cấp marimo.
- Những gì danh sách không đóng được: `/api/kernel/save` là cần thiết và nhận một `filename` (save-as),
  chỉ bị giới hạn bởi việc marimo kiểm nằm-trong-workspace (`marimo/_server/api/endpoints/files.py:152-180`),
  nên thư mục workspace của pod không được chứa gì ngoài draft; `/public/{path}` chọn thư mục notebook theo
  header `X-Notebook-Id` do client gửi (`endpoints/assets.py:712-750`) — proxy xoá nó hoặc ghim nó vào app
  của session; `/api/kernel/execute` và scratchpad chạy code với quyền như một cell.
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
| Kernel | mỗi notebook đang mở một tiến trình | mỗi session người xem một tiến trình (D7), qua patch IPC của egret; lấy từ một launcher khởi động sẵn (pool hoặc fork server, chọn ở P5) |
| Vòng đời | reaper khi rảnh, như hiện nay | dừng sau 15 phút không có session; egret hiện màn hình chờ lúc cold start và ở mọi session mới cho tới khi app render lần đầu |
| Giới hạn | như hiện nay (250m/512Mi → 1000m/2Gi), cấu hình được | cấu hình được; tối đa số session mỗi pod, vượt thì 503 "app đang bận" — mức trần (20 là ước đoán trước S0) và giới hạn bộ nhớ được đặt lại ở P5 từ ngân sách dưới đây, đo trên kind |

- **Mỗi session run một tiến trình là patch của egret, không phải tính năng của marimo** (§2). Entrypoint
  ép `SessionImpl.create` vào nhánh IPC và trỏ `ipc._get_venv_config` tới interpreter của image; các
  kernel dùng chung site-packages của image. Cả hai là **API nội bộ của marimo**, nên, giống
  `EgretStorage`: image ghim chính xác phiên bản marimo và CI của image có test hợp đồng (N session → N pid
  kernel khác nhau, và header của mỗi session tới đúng kernel của nó). Nếu một lần nâng cấp làm vỡ nó và
  không patch lại được, phương án lùi là mỗi (app × người xem) một pod.
- **Chi phí đã đo** (S0 và S0b; Playwright tự động, mỗi trường hợp một lần chạy, laptop dev macOS đang
  chịu tải nặng không liên quan (load 1 phút 4,5 tới 26), không phải pod; kết quả S0b:
  [`2026-10-01-marimo-s0b-result.md`](../../en/plans/2026-10-01-marimo-s0b-result.md)):
  - **Bộ nhớ.** ≈ 80 MB RSS mỗi kernel IPC đang chạy (S0 79,9–81,3 MB; S0b worker pool đã nhận việc
    ~81 MB) cộng server 77 MB (S0). Khởi động sẵn thêm chi phí lúc rảnh: một worker pool ấm là
    64,5–70,3 MB và pool cỡ N giữ N+1 worker; tiến trình cha của fork server là 48,5 MB, còn bộ nhớ riêng
    của mỗi tiến trình con chưa đo (RSS trên macOS tính cả trang copy-on-write dùng chung). Vậy ngân sách
    mỗi pod là server + launcher lúc rảnh + số session × ~81 MB; ở 20 session riêng kernel đã ≈ 1,6 GB,
    chưa kể dữ liệu của app.
  - **Thời gian.** 13,2 giây của S0 cho lần render IPC đầu tiên **không tái hiện được** (S0b: 4,82 giây
    khi ấm, 7,35 giây với cache `.pyc` rỗng; phần chênh còn lại chưa giải thích). Ở một session ấm
    ≈ 1,75 giây là trình duyệt tải trang egret và frontend marimo, ở mọi chế độ — một mức sàn không
    launcher nào thay đổi. Trên đó, khởi chạy IPC trực tiếp thêm ~2,3 giây (T1, load ~4,5); ở vòng 4 của
    S0b (load 5,4 → 22,4) p50 tuần tự là thread 2,12 giây, direct 6,09, pool 3,54, fork server 3,03. Không
    chế độ nào, kể cả thread, đạt < 2 giây tuần tự hay < 3 giây cho ba người xem đồng thời trên laptop
    này; số đồng thời ở đó bị việc tải trang chi phối (5,7–11,8 giây từ ticket → WebSocket ở mọi chế độ)
    và không xếp hạng được gì.
  - **Chi phí lớn nhất có thể bỏ** là bước kiểm `has_marimo_installed` của marimo: trước **mỗi** lần khởi
    chạy kernel từ một interpreter đã cấu hình, không ghi được (patch `_get_venv_config` của egret làm nó
    thành như vậy), marimo chạy `<python> -c "import marimo, msgspec, zmq; …"`
    (`marimo/_session/_venv.py:128`), một interpreter mới import marimo. Ở lần chạy pool, spawn → worker
    nhận việc mất 1,0 tới 1,8 giây, phần lớn là bước kiểm này (khoảng đó gồm cả lúc shim khởi động). Bỏ
    qua nó làm spawn → ready của pool giảm từ 1,03–1,27 giây xuống 0,11–0,23 giây (một giá trị ngoại lai
    1,48 giây). **Đã quyết với người dùng (2026-10-01, D14): phương án (a)** — shim của launcher trả lời
    bước kiểm, được bảo vệ bằng test hợp đồng trong CI của image. Các phương án đã cân nhắc: (a) shim của launcher trả lời đúng lệnh đó (thứ S0b đã đo —
    một mẹo dựa vào hành vi nội bộ, cần test hợp đồng sẽ fail khi lệnh đổi); (b) một thay đổi upstream ở
    marimo (vd. cache kết quả theo interpreter và phiên bản); hoặc (c) chạy bước kiểm một lần lúc build
    image hay lúc pod khởi động và để patch của entrypoint trả về kết quả đó (thêm một patch nội bộ, cùng
    quy tắc test hợp đồng).
- **Launcher.** S0b dựng hai launcher khởi động sẵn sau một shim ở đường dẫn interpreter đã cấu hình:
  một **warm pool** (N worker dùng một lần đã import sẵn marimo, polars, httpx và `egret_marimo`; các
  worker thừa kế socket đang listen và mỗi lúc chỉ một worker được "armed", vì truyền socket đã accept
  bằng `send_fds` làm mất session trên macOS với nguyên nhân chưa chứng minh) và một **fork server** (một
  tiến trình cha đơn luồng có cùng các import, fork một tiến trình con cho mỗi kernel). Cả hai chạy chính
  `marimo._ipc.launch_kernel.main()` của marimo, module mà docstring của marimo khai báo đường dẫn là
  "a public API used by external consumers … Changing this path is a BREAKING CHANGE"
  (`marimo/_ipc/launch_kernel.py`); payload `KernelArgs` qua stdin và các dòng `KERNEL_READY` /
  `KERNEL_INFO` không nằm trong câu đó, nên test hợp đồng ở P3 cũng phủ chúng. Cả hai cắt phần p50 tuần
  tự vượt thread của direct đi hơn một nửa (pool ở vòng 3 và 4; fork server ở vòng 4, vòng duy nhất của
  nó) và giữ được cô lập (pid khác nhau, dùng một lần, kernel bị
  giết khi client của nó ngắt, không còn sống sau session TTL, danh tính đúng). **S0b không chọn được
  giữa hai cái**: ở vòng 4 chênh lệch p50 tuần tự của chúng là ~0,5 giây và đổi chiều khi bỏ bước kiểm
  (pool 3,54 so với fork server 3,03 giây; khi bỏ bước kiểm 2,43 so với 2,95 giây), các lần chạy đơn ở
  load 5–22. Chúng khác nhau ở chỗ khác:
  bộ nhớ rảnh của pool là (N+1) × ~70 MB và nó treo khi tắt nếu kernel bị giết trước (còn mở, phía
  marimo chưa điều tra); fork server chỉ an toàn khi tiến trình cha không bao giờ *dùng* polars (chỉ
  import), điều không gì ép buộc. **P5 chọn** sau khi đo trên một node Linux yên tĩnh trên kind: mọi
  biến thể xen kẽ, ≥ 5 lần chạy mỗi biến thể, mốc theo từng pha, PSS/USS cho mỗi worker rảnh và mỗi
  tiến trình con được fork, hành vi tắt của cả hai, với cách xử lý `has_marimo_installed` mà P3/P5 chọn.
- **UX lúc mở session.** Mọi session người xem mới đều chờ một kernel, không chỉ pod lạnh, và **tải lại
  không bao giờ dùng lại kernel** (S0b: tải lại trang host được pid mới; tải lại riêng iframe qua proxy
  của spike để trang trắng vì lúc WebSocket cũ đóng đã thu hồi token mà cookie của nó vẫn trỏ tới — P4
  cấp ticket lại hoặc từ chối rõ ràng trường hợp đó). Màn hình chờ che toàn bộ khoảng này; P5 đặt cỡ pool
  theo đợt người xem mới dự kiến (S0b không có số cho người xem tới lúc không còn worker ấm: mọi lần nhận
  việc đã đo đều gặp một worker ấm), và S0b không cho lý do để đổi thời gian chờ rảnh của pod (15 phút).

- `KernelLauncher` (`crates/egret-kernel/src/launcher.rs`) cùng các cài đặt k8s và subprocess của nó được
  tổng quát hoá để khởi chạy cả hai loại pod; reaper và hardening pod hiện có (non-root, drop ALL,
  seccomp RuntimeDefault) giữ nguyên.
- **NetworkPolicy:** ingress chỉ từ egret; egress chỉ tới gateway của egret và DNS.
- **Cấu hình marimo** (qua `XDG_CONFIG_HOME` trong image) đặt `[ai] enabled = false`; mọi thứ khác bị chặn
  ở proxy (§6.2). Cài package đằng nào cũng thất bại vì egress đã đóng. `XDG_CONFIG_HOME` là nơi marimo
  tìm *cuối cùng* — một `.marimo.toml` ở thư mục làm việc, thư mục cha bất kỳ, hoặc `~` thắng trước
  (`marimo/_config/utils.py:69-120`) — nên image bảo đảm không có file nào như vậy.
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
  (quản lý share). Share dùng hệ object-shares chung với loại object `app`. **P1 chỉ có `app:read` và
  `app:write`**; `app:manage` đến cùng share ở P2, vì test catalog-drift từ chối một quyền được khai báo mà
  không có chỗ nào thực thi.
- Module tính năng workspace `notebooks` được thay bằng `apps`, phủ `apps`, `apps-runtime`.
- `notebook:read` / `notebook:write` bị gỡ ở P6.

## 9. Triển khai

### S0 — spike (code bỏ đi, sản phẩm là một báo cáo viết) — **đã xong**

Kết quả: [`docs/en/plans/2026-09-30-marimo-s0-result.md`](../../en/plans/2026-09-30-marimo-s0-result.md)
(2026-09-30, marimo 0.25.0, HeadlessChrome 136, nhánh `spike/marimo-s0` @ `03f7a9fd`, không bao giờ merge).
Không phương án lùi nào bị kích hoạt; kiến trúc được giữ. Hai câu hỏi mở cho người dùng được liệt kê ở đó; câu về origin apps đã được quyết (D12, subdomain).

| # | Câu hỏi | Nếu không được | Điều đã xảy ra |
|---|---|---|---|
| 1 | Header do proxy gắn có tới kernel của đúng session qua `mo.app_meta().request` không, kể cả khi chạy lại theo reactive? | Truyền token qua query param khi tạo session | **Có**, ở chế độ thread và IPC. Không kích hoạt (§4.1). |
| 2 | Chế độ run có chạy mỗi session một tiến trình được không — `--sandbox` offline, hoặc entrypoint ép nhánh IPC — và tốn bao nhiêu thời gian khởi động? | Mỗi (app × người xem) một pod | `--sandbox`: **không** (1 pid cho 4 session). Ép IPC: **có** (4 pid), ≈ 80 MB mỗi kernel, 13,2 giây cho session đầu tiên và 32,9 giây cho ba session đồng thời, trên laptop. Không kích hoạt; D7 dựa vào patch của egret (§2, §6.3). |
| 3 | Load đã patch + `EgretStorage` có đi trọn vòng một draft không, và tác giả thấy gì khi 409? | Agent đồng bộ theo dõi file; header egret hiển thị 409 | **Có**; marimo hiện toast "Failed to save" với nội dung của egret. Không cái nào kích hoạt (§5.1). Yêu cầu mới: lấy lại khi load không được đặt lại revision của editor đang mở. |
| 4 | marimo có chạy được sau proxy với base URL và WebSocket, trên origin riêng, với ticket → cookie không? | Không có — xem lại kiến trúc | **Có**. Không kích hoạt. JS của tác giả chạy được (anywidget); cookie Lax của egret đi theo request của nó và cookie bị tung vào tới egret; `__Host-` chưa đo được trên http → yêu cầu cho P4 và một câu hỏi mở (§6.1). |
| 5 | SQL cell có nhận kết nối DB-API của `egret_marimo` làm engine không? | Một lớp engine marimo riêng | **Có**; chỉ hiện trong dropdown sau khi cell của nó đã chạy. Không kích hoạt. Đã quyết: cài DuckDB, engine egret mặc định (D13, §4.4). |
| 6 | Proxy phải chặn những route nào (terminal, package, secrets, file browser) mà không làm hỏng edit và lưu? | — (đây là phép đo; proxy chặn dù thế nào) | 22 prefix; phát hiện và sửa một lỗ vượt qua bằng dot-segment; ưu tiên allow-list (§6.2). |

### S0b — spike độ trễ khởi động kernel — **đã xong**

Kết quả: [`docs/en/plans/2026-10-01-marimo-s0b-result.md`](../../en/plans/2026-10-01-marimo-s0b-result.md)
(2026-09-30 tới 2026-10-01, marimo 0.25.0, HeadlessChrome 136, nhánh `spike/marimo-s0` @ `14cc33f1`,
không bao giờ merge; Playwright tự động, mỗi trường hợp một lần chạy, laptop dev macOS chịu tải nặng).

| # | Câu hỏi | Điều đã xảy ra |
|---|---|---|
| S0b | Thời gian khởi động của một người xem mới đi đâu ở chế độ ép IPC, và kernel khởi động sẵn (warm pool, fork server) có giúp không? | ≈ 1,75 giây là tải trang + frontend ở mọi chế độ (mức sàn); bước kiểm `has_marimo_installed` mỗi lần khởi chạy của marimo chiếm phần lớn khoảng spawn → nhận việc 1,0–1,8 giây và có thể bỏ; 13,2 giây của S0 không tái hiện được (4,82 giây). p50 tuần tự vòng 4: thread 2,12, direct 6,09, pool 3,54, fork server 3,03 giây; < 2 giây / < 3 giây đồng thời không chế độ nào đạt, kể cả thread. Pool và fork server chưa tách được; P5 chọn trên Linux (§6.3). Cô lập giữ được ở mọi biến thể. |

### Các PR, theo thứ tự

1. **P1 — Query gateway.** `app_sessions` (`login_sid` cascade từ bảng `sessions` của egret, nên logout
   thu hồi nó theo cấu trúc), bộ kiểm chỉ đọc, `POST /api/apps/query`, audit, rate limit — cộng những thứ
   gateway cần để phân quyền: bảng định danh `analytics.apps`, quyền `app:*` và module tính năng `apps`.
   Chỉ backend; test được bằng token seed sẵn, không cần marimo. Plan:
   `docs/en/plans/2026-10-01-data-apps-p1-query-gateway.md`.
2. **P2 — Lưu trữ và vòng đời.** `app_drafts`, `app_versions` (+ trigger, và FK từ
   `app_sessions.version_id`, phức hợp với `app_id` để phiên bản thuộc đúng app), API draft với
   revision/409, publish, khôi phục, CRUD app và share với `app:manage`; gateway kiểm phiên bản của token
   run vẫn tồn tại và đang publish; FK `apps.folder_id` (dùng cho tổ tiên PDP); `ALL_FEATURE_KEYS` ở
   frontend thêm `apps`.
   Plan: `docs/en/plans/2026-10-01-data-apps-p2-storage-lifecycle.md`.
3. **P3 — Image `egret-apps` và `egret_marimo`.** Entrypoint, `EgretStorage`, DB-API, `egret.sql`, cấu hình
   marimo đã khoá, patch IPC cho chế độ run và cách xử lý bước kiểm `has_marimo_installed` mỗi lần khởi
   chạy của marimo (D14: shim của launcher trả lời, với test hợp đồng sẽ fail khi marimo đổi lệnh); pytest
   với một egret giả, kể
   cả test hợp đồng `StorageInterface` (có ca tải-lại-không-gắn-lại, §5.1) và test hợp đồng IPC (§6.3).
   Client API phía pod theo §5.3. Plan: `docs/en/plans/2026-10-01-data-apps-p3-image-and-client.md`.
4. **P4 — Origin apps, proxy, chế độ edit.** Ticket → cookie, proxy (allow-list route hoặc block list của
   S0, kiểm và chuyển tiếp một đường dẫn thô, xoá hoặc ghim `X-Notebook-Id`), đổi tên cookie sang `__Host-`
   và kiểm qua HTTPS trên kind, kiểm toán phía egret rằng route `GET` không có tác dụng phụ và CSRF vẫn dựa
   trên header (§6.1), launcher edit, trang danh sách Apps và trang Edit. Từ review cuối của P1: thu hồi
   token theo id / phiên đăng nhập (không chỉ theo plaintext) và dọn các dòng hết hạn; quyết định phiên
   đăng nhập egret đã hết hạn nhưng chưa xoá có phải chặn token app không; cột `impersonator` trên
   `app_sessions` trước khi mint token khi đang impersonate; kiểm `session_idle_secs <=
   session_absolute_secs` lúc khởi động; chỉ audit lời gọi bắt đầu khoá rate limit, không audit mọi 429.
   Từ review cuối của P3: các endpoint phía pod ở §5.3 và việc mint token pod; proxy còn chặn
   `/api/ai/*`, `/terminal`, `/api/kernel/copy` và `/api/kernel/rename` (image cũng từ chối copy/rename vào
   draft, nhưng proxy mới là ranh giới); token tới pod qua `secretKeyRef`; tini làm PID 1; deployment xử
   lý mã thoát của launcher (64 = env sai, không khởi động lại; 65 = có file config bị từ chối, cấp
   `emptyDir` mới; 69 = lấy phiên bản thất bại, thử lại); kernel chạy dưới uid khác với server (hoặc
   `PR_SET_DUMPABLE=0` tạm thời) để cell không đọc được token của server; config mới chỉ khoá `ai` và
   `rtc_v2` — các khoá PEP 723 / venv / runtime vẫn mở; tải lại khi WebSocket cũ còn kết nối cần tải lại
   lần hai; endpoint `egret.databases()`; chạy lại test hợp đồng marimo, e2e và kiểm kê route mỗi lần
   nâng marimo; layout dạng grid (`layouts/*.grid.json`) chỉ ghi xuống đĩa của pod, không bao giờ về egret —
   lưu cùng draft hoặc chặn layout grid ở chế độ edit.
5. **P5 — Chế độ run.** Launcher theo phiên bản, UI publish/versions/khôi phục, trang xem app; launcher
   kernel (warm pool hoặc fork server), mức trần session mỗi pod, giới hạn bộ nhớ và UX lúc mở session
   đặt theo số đo trên kind (§6.3). **Mục tiêu khởi động (D15):** tiêu chí nghiệm thu = người xem mới trên
   pod đã ấm chậm hơn chế độ thread đo cùng lượt tối đa **0,5 giây** ở trung vị, và ba người xem mới đồng
   thời chậm hơn tối đa **1 giây** ở giá trị lớn nhất, đo trên node Linux ở kind với ≥ 5 lần chạy xen kẽ
   mỗi biến thể. "< 2 giây tới lần render đầu" được theo dõi như mục tiêu trải nghiệm riêng (asset
   frontend, cache) và không chặn P5. **Tắt pod (D16):** một test nghiệm thu cho launcher mà P5 chọn — pod
   tắt được trong thời gian grace khi các kernel đã chết trước (S0b thấy pool bị treo đúng ở thứ tự này;
   fork server chưa được thử).
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
- **JavaScript trong app không đọc được cookie của egret hay response của egret** — browser test với một
  app cố thử. (Request của nó *có* tới egret bằng danh tính người xem, §6.1; vì thế mới có hai mục sau.)
- **JavaScript trong app không hoàn tất được bất kỳ thay đổi trạng thái nào trên egret** — kiểm bằng việc
  kiểm toán rằng không `GET` nào của egret có tác dụng phụ và mọi route thay đổi trạng thái đều đòi header
  `X-Egret-CSRF` (CSRF dựa trên header), cộng một browser test thử POST no-cors và POST mang header.
- **Cookie `__Host-` bị tung vào không bao giờ tới egret** — app đặt `__Host-egret_csrf` có `Domain=` rồi
  POST; kiểm qua HTTPS trên kind để cookie `__Host-` tồn tại (S0 không làm được, §6.1).
- **Hai người xem có grant Ranger khác nhau thấy các dòng khác nhau từ cùng một app**, trên Trino thật và
  StarRocks thật trên kind.
- Admin mở một app chạy `DELETE` nhận 400 và bảng không đổi.
- `UPDATE` và `DELETE` trên `app_versions` thất bại.
- Hai người sửa cùng một draft: người thứ hai nhận 409, và marimo hiển thị nó — kể cả khi người thứ hai đã
  tải lại trang hoặc mở tab thứ hai ở giữa (§5.1).
- Proxy từ chối các dạng dot-segment, `//` và mã hoá phần trăm của một route bị chặn (40 biến thể raw
  socket của S0), trên HTTP và WebSocket.
- **Hai người xem khác nhau của cùng một app đã publish rơi vào hai pod khác nhau, không bao giờ
  chung một pod** (D24) — đã kiểm chứng sống trên kind: [kết quả P5e](plans/2026-10-04-data-apps-p5-live-result.md),
  cũng tìm ra và sửa hai lỗi thật (cổng container của run pod, và danh sách cổng ingress của
  NetworkPolicy apps) mà chỉ cluster sống mới lộ ra được.
- Kiểm thử sống bằng trình duyệt trên kind cho P4 và P5.

## 11. Hoàn thành khi

- Tác giả xây một app có SQL cell và widget UI trên Trino và trên StarRocks, rồi publish.
- Hai người xem thấy kết quả theo quyền của chính họ — bao gồm cả run mode, kể từ P5e (2026-10-04):
  [kết quả sống](plans/2026-10-04-data-apps-p5-live-result.md).
- Admin mở một app độc hại không ghi được dữ liệu, không đọc được cookie hay response của egret, và không
  hoàn tất được bất kỳ thay đổi trạng thái nào trên egret.
- Notebook Jupyter đã được gỡ.

## 12. Ngoài phạm vi

Cộng tác thời gian thực; dependency theo app (PEP 723 với mirror nội bộ — bước tiếp theo tự nhiên, và
không đổi kiến trúc này); nhúng ra ngoài egret; link công khai; chạy hoặc export app theo lịch; truyền
dữ liệu dạng Arrow; autoscale; chuyển đổi notebook cũ.
