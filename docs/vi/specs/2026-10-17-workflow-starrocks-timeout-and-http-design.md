# Follow-up workflow, nhóm A: statement StarRocks bị timeout thật ra có dừng; `http()` bị từ chối vĩnh viễn

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** đã xây và kiểm chứng 2026-10-17. Mọi khẳng định về code của egret đã được kiểm tra trong worktree `egret-grpa` tại `4f6cafec` (main, sau #683) và được trích dẫn `path:line` từ gốc repo.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (ghi chú về các tên không gói nào sở hữu) và §11 câu 3.

**Quyết định cùng user, 2026-10-17:**
- **StarRocks: không đặt biến phiên; sửa doc và khoá hành vi đã đo** (§1.3, quyết định sau phép đo ở §1.2). Không làm `KILL QUERY` ở đây: cancel sớm thuộc gói kill (nhóm B).
- **Jar plugin do operator quản lý.** Đây là phán quyết W5b ngày 2026-09-29, đã ghi ở §11 câu 3 của spec chương trình: jar chỉ vào qua PR được review sửa `plugin-host/plugins.lock` (Maven coordinate + sha256, kiểm lúc build image), platform admin thu hẹp bằng `allowed-types`, workspace bật bằng `workflow_kestra_plugins`. Workspace không bao giờ upload jar. Mục này không xây gì; ghi ra để quyết định không bị mở lại.
- **`http()` bị từ chối vĩnh viễn**, không còn "chờ W9".

## 1. StarRocks: doc nói statement bị timeout vẫn chạy tiếp, nhưng không phải vậy

### 1.1 Code và doc đang khẳng định gì

- `crates/egret-query/src/execution/connector/starrocks.rs:251` (`execute`) và `:306` (`execute_with_params`) không đọc `limits.timeout`. Mọi caller bọc lời gọi bằng `tokio::time::timeout` ở cùng deadline và drop future khi nó hết hạn: `crates/egret-query/src/execution/port_impl.rs:366` (`run_bounded`, đường serving Data API và `egret.sql`), `crates/egret-query/src/execution/job.rs:222` (JobManager), `crates/egret-query/src/execution/connector/mod.rs:189`, `crates/egret-query/src/registry/tester.rs:47`.
- Bốn chỗ nói rằng sau khi drop, statement **chạy tới cùng trên engine**: `crates/egret-workflow/src/worker/handlers/sql.rs:557-565` (doc của `cancel()`) và `:579-582` (nội dung lỗi: *"on StarRocks … it is not bounded by anything"*), `crates/egret-workflow/src/routes.rs:2225-2231` (phát hiện về kill, trích câu đó), `starrocks.rs:300-305`, và `crates/egret-dataapi/src/serving/router.rs:45-49`.

### 1.2 Kết quả đo

Trên `starrocks/allin1-ubuntu:4.1.6@sha256:5cbf09c4…` đã ghim, container riêng, 2026-10-17 (Task 1 của plan; output thô trong báo cáo task):

- **Đường thật của egret, connector chưa sửa:** `tokio::time::timeout(3 s, c.execute(LONG_SQL, …))` hết hạn, future bị drop, và một kết nối mới **không** thấy dòng nào của statement trong `SHOW FULL PROCESSLIST` ở +1 s, +5 s hay +20 s. Tương tự với `INSERT INTO probe.t SELECT …` qua `execute`, và với `execute_with_params` (`WHERE ? = 1`, `Param::I64(1)`). Sau khi drop, statement biến mất khỏi processlist, khớp với việc kết nối bị đóng (connector tự mở và ngắt `Conn` riêng cho mỗi lời gọi, `starrocks.rs:258`). Kill client CLI `mysql` cũng cho kết quả như vậy.
- `LONG_SQL` = `SELECT count(*) AS n, 'egret-deadline-probe' AS marker FROM TABLE(generate_series(1, 100000000000))`: chạy lâu hơn 70 s nếu không có gì kết thúc nó.
- Để tham khảo, không áp dụng (§1.3): `query_timeout` (mặc định 300) và `insert_timeout` (mặc định 14400) đều tồn tại; `SET query_timeout = 2` kết thúc `LONG_SQL` ở 2 s với `ERROR 5024 (53400): Query reached its timeout of 2 seconds, …`; `INSERT … SELECT` chỉ tuân theo `insert_timeout` (chỉ đặt `query_timeout = 2` thì nó chạy 159 s).
- Đối chứng dương: cả ba test đều assert đã thấy statement đang chạy trước khi drop, nên probe không nhìn thấy statement thì không thể xanh. Needle: `egret-deadline-probe` (SELECT), `INSERT INTO egret_drop_probe.t` (INSERT), `egret-params-deadline-probe` (SELECT có tham số, binary protocol).
- Lưu ý khi đo: StarRocks bỏ comment `/* */` khỏi `Info`, nên marker phải nằm trong chính text SQL; kết nối `mysql_async` thô để dò cần `prefer_socket(false)`.
- Phạm vi: các khẳng định về StarRocks của đường plugin-host (`docs/en/specs/2026-09-29-workflow-w5b-kestra-plugin-host-design.md:107,116,172`, `crates/egret-workflow/oracle-exec/README.md:9265`, spec chương trình EN `:509` "a StarRocks query is not") nói về `kill()` của Kestra qua Connector/J trên plugin host; chúng được đo trước khi nâng StarRocks 4.1.6 (#666), không đo lại ở đây, và để cho nhóm B.

Vậy khẳng định ở §1.1 sai trên 4.1.6, với mọi đường egret dùng. Lý do từng giữ biến phiên ở ngoài (*"sẽ làm hỏng query hôm nay chạy được"*, `handlers/sql.rs:562-565`) cũng không còn ý nghĩa.

### 1.3 Quyết định và thay đổi

**Quyết định cùng user, 2026-10-17: không đặt biến phiên.** Nó tốn thêm một roundtrip cho mọi statement StarRocks, kể cả đường serving Data API, để chặn một rò rỉ không xảy ra. Phần còn hở là kết nối không bao giờ được đóng sạch (mất mạng, mất node), khi đó default của engine giới hạn statement: 300 s cho query, 14400 s cho `INSERT`. Phần dư này được ghi ra, không sửa.

- **Sửa mọi đoạn ở §1.1** theo hành vi đã đo: khi egret ngừng chờ, nó drop kết nối và StarRocks 4.1.6 tự huỷ statement. `cancel()` trong `handlers/sql.rs` vẫn trả `Err` (nó không dừng được statement *trước* deadline, việc đó vẫn thuộc nhóm B), nhưng thông điệp không còn nói statement không bị giới hạn. Nội dung lỗi phải giữ chữ `QueryExecPort` và `StarRocks`: `cancel_refuses_rather_than_claiming_to_have_stopped_the_statement` (`handlers/sql.rs:1428`) assert cả hai. Trong `routes.rs`, điểm 7 của phát hiện về kill được viết lại: statement bị bỏ không còn chạy trên StarRocks; handler vẫn không dừng sớm được.
- **Khoá hành vi bằng live test** để một lần nâng StarRocks hoặc `mysql_async` làm đổi hành vi này sẽ đỏ, thay vì âm thầm làm doc sai trở lại.

### 1.4 Test

- Live, trong `crates/egret-query/tests/connector_starrocks.rs` (gate bằng `EGRET_TEST_STARROCKS_URL`), ba test: `LONG_SQL` qua `execute`, một `INSERT … SELECT` từ `generate_series` vào bảng nháp qua `execute`, và một SELECT `WHERE ? = 1` qua `execute_with_params`. Mỗi test bọc lời gọi trong `tokio::time::timeout(3 s, …)`, assert timeout ngoài đã nổ, rồi dò `SHOW FULL PROCESSLIST` trên kết nối mới tối đa 10 s và assert text của statement đã biến mất. Mỗi test cũng assert nó **đã thấy** statement trong processlist khi đang chạy (dò trước deadline, trên kết nối mới), để một probe không nhìn thấy statement thì không thể xanh.
- Test này đo một tính chất hôm nay đang đúng, nên lần chạy đỏ được chứng minh bằng **mutation của probe, không phải của code**: trỏ phép so processlist vào một chuỗi không bao giờ có trong `Info` thì assertion "đã thấy đang chạy" phải fail. Một lần chạy xanh phải cho thấy test thực sự chạy (không phải dòng `skip`), vì test gate theo engine skip im lặng.

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
