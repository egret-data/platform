# Follow-up workflow, nhóm A: StarRocks tôn trọng timeout; `http()` bị từ chối vĩnh viễn

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế 2026-10-17, chưa xây. Mọi khẳng định về code của egret đã được kiểm tra trong worktree `egret-grpa` tại `4f6cafec` (main, sau #683) và được trích dẫn `path:line` từ gốc repo.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (ghi chú về các tên không gói nào sở hữu) và §11 câu 3.

**Quyết định cùng user, 2026-10-17:**
- **StarRocks có deadline phía server, cho mọi caller, bằng biến phiên.** Không làm `KILL QUERY` ở đây: cancel thật thuộc gói kill (nhóm B).
- **Jar plugin do operator quản lý.** Đây là phán quyết W5b ngày 2026-09-29, đã ghi ở §11 câu 3 của spec chương trình: jar chỉ vào qua PR được review sửa `plugin-host/plugins.lock` (Maven coordinate + sha256, kiểm lúc build image), platform admin thu hẹp bằng `allowed-types`, workspace bật bằng `workflow_kestra_plugins`. Workspace không bao giờ upload jar. Mục này không xây gì; ghi ra để quyết định không bị mở lại.
- **`http()` bị từ chối vĩnh viễn**, không còn "chờ W9".

## 1. StarRocks: engine dừng statement mà egret đã bỏ

### 1.1 Hiện trạng

- `crates/egret-query/src/execution/connector/starrocks.rs:251` (`execute`) và `:306` (`execute_with_params`) không đọc `limits.timeout`; doc ở `:300` nói vậy.
- Mọi caller đều đã bọc lời gọi bằng cùng deadline, nên egret ngừng chờ ở `limits.timeout`:
  - `crates/egret-query/src/execution/port_impl.rs:366` (`run_bounded`, đứng sau `execute_as` và `execute_as_with_params`: đường serving của Data API và `egret.sql`);
  - `crates/egret-query/src/execution/job.rs:222` (JobManager: SQL Lab, chart, alert);
  - `crates/egret-query/src/execution/connector/mod.rs:189` và `crates/egret-query/src/registry/tester.rs:47`.
- Drop future thì drop kết nối MySQL, và điều đó không báo gì cho StarRocks: statement chạy đến cùng trên engine (`crates/egret-workflow/src/worker/handlers/sql.rs:557-565`).
- Trino đã gửi `query_max_run_time` từ cùng `limits.timeout` (`crates/egret-query/src/execution/connector/trino.rs:490`).

### 1.2 Vì sao phản đối cũ không còn đúng

`handlers/sql.rs:562-565` nói không đặt biến phiên vì connector dùng chung với đường serving Data API "và sẽ bắt đầu làm hỏng query hôm nay chạy được". Trên main hiện tại điều đó sai: mọi caller ở trên đã báo timeout tại `limits.timeout`. Đồng hồ của egret bắt đầu trước bước connect, đồng hồ của engine bắt đầu khi statement tới, nên deadline của engine không bao giờ sớm hơn của egret. Biến này chỉ có thể dừng việc egret đã bỏ.

### 1.3 Thay đổi

- Một helper private trong `connector/starrocks.rs` gửi `SET query_timeout = N` (và `insert_timeout = N`, xem dưới) trên kết nối, với `N = ceil(limits.timeout tính bằng giây)`, tối thiểu 1.
- Chạy trong `execute` và `execute_with_params`, sau connect và sau `switch_identity`, trước statement. Mỗi lời gọi mở `Conn` riêng (`starrocks.rs:258`) và disconnect khi xong, nên biến không lan sang statement khác.
- Nếu `SET` lỗi, lời gọi trả `EngineError::Engine` và statement không được gửi. Chạy không deadline chính là thứ cần sửa, nên không chấp nhận fail-open.
- `insert_timeout`: các bản StarRocks gần đây dùng nó thay cho `query_timeout` với `INSERT`. Task 1 của plan đo trên StarRocks 4.1.6 đã ghim xem biến có tồn tại không và `INSERT … SELECT` có tuân theo không. Nếu không tồn tại, chỉ đặt `query_timeout` và ghi kết quả đo vào đây.
- `describe` và `describe_with_params` ngoài phạm vi: chúng tự bọc `tokio::time::timeout` (`starrocks.rs:389`, `:451`) và chỉ gửi query lấy metadata kết quả.
- Doc nêu hành vi cũ được sửa trong cùng commit: `handlers/sql.rs` (doc của `cancel()` và nội dung lỗi), `starrocks.rs:300-305`, và module doc của `crates/egret-dataapi/src/serving/router.rs:45-49`.

### 1.4 Test

- Unit: `N` làm tròn lên (1,2 s thành 2), không dưới 1 (200 ms thành 1), số giây chẵn giữ nguyên (30 s thành 30).
- Live, trong `crates/egret-query/tests/connector_starrocks.rs` (gate bằng `EGRET_TEST_STARROCKS_URL`): chạy một statement lâu hơn 2 s rất nhiều, gọi thẳng connector với `timeout = 2 s` (không bọc ngoài, nên chỉ engine kết thúc được nó), assert nó trả lỗi timeout của engine. Rồi trên kết nối thứ hai, tìm text của statement trong `SHOW PROCESSLIST` và assert nó biến mất trong deadline cộng một khoảng dư.
- Mutation: bỏ `SET`; test live phải đỏ. Một lần chạy xanh phải cho thấy test thực sự chạy (không phải dòng `skip`), vì test gate theo engine skip im lặng.

## 2. `http()`: từ chối vì nơi nó sẽ chạy

### 2.1 Hiện trạng

`crates/egret-workflow/src/runners/functions.rs:285` đăng ký `http` là `Refusal::NotYet { needs: "an outbound HTTP client", package: "W9" }`, nên tác giả đọc thấy "gói W9 của egret cung cấp". W9 đã ship mà không có nó, và spec chương trình ghi `http` "still owned by no row" (`docs/en/specs/2026-09-01-workflow-programme-design.md:515`, `:541`).

Expression render bên trong tiến trình egret server. Một request từ lúc render sẽ đi ra từ control plane của egret, nên tác giả flow có thể chạm tới mọi thứ mạng của egret chạm tới. Đó là SSRF, và một allow-list sẽ là bề mặt bảo mật mới cho một hàm mà không flow nào trong corpus dùng.

### 2.2 Thay đổi

- Variant mới `Refusal::Unsupported(&'static str)` cạnh `NotYet` và `Confined` (`functions.rs:215`), render giống `Confined` (`pebble_error`, `functions.rs:206`).
- `http` dùng nó với câu (giữ nguyên tiếng Anh trong code): *"The 'http' function is not supported: an expression renders inside egret's control plane, so a request made from it would leave from egret's own network. Make the request in a script task, which runs in the task's container."*
- Test `the_refused_list_is_six_and_kv_is_not_in_it` (`functions.rs:3561`) chuyển `http` từ danh sách not-yet sang một dòng `Unsupported` được kiểm, với cả ba dạng đối số. Số lượng vẫn là sáu.
- Spec chương trình (EN và VI) ghi quyết định tại hai câu trích ở trên: *"`http` bị từ chối có chủ đích (2026-10-17): nó sẽ chạy trong control plane."*

## 3. Phạm vi

Không migration, không frontend, không key config mới. Một PR, hai commit, mỗi mục một commit. Ngoài phạm vi: `KILL QUERY` và route kill (nhóm B), các task Kestra còn lại (nhóm C).
