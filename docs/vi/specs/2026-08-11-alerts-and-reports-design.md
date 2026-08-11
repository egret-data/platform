# Alerts & Reports — thiết kế

**Trạng thái:** đề xuất
**Sản phẩm tham chiếu:** Apache Superset, *Alerts & Reports* (`superset/reports/models.py`,
`superset/commands/report/execute.py`), đã checkout tại `data-portal/superset`.

> Bản tiếng Anh là bản gốc: `docs/en/specs/2026-08-11-alerts-and-reports-design.md`.

---

## 1. Hiện trạng

Không có gì. `Alerts` là một mục sidebar trong nhóm **Automate** trỏ tới `<Placeholder>`
(`App.tsx:106`). Không bảng, không route, không crate, không API. Toàn bộ tính năng là làm mới.

Ba sự thật về egret quyết định hình dạng công việc, cả ba đều đo được chứ không phỏng đoán:

1. **Không có kênh gửi thông báo nào.** Không SMTP (`lettre` không phải dependency), không Slack,
   không webhook sender ở bất kỳ đâu trong `crates/`.
2. **Không có scheduler dùng chung.** Việc chạy định kỳ duy nhất là vòng `tokio::interval` dựng
   tay — `egret-kernel/src/reaper.rs` là tiền lệ và là mẫu để sao chép.
3. **Ảnh chart được tạo Ở TRÌNH DUYỆT.** `downloadPng` đọc `<svg>` đã render từ DOM
   (`components/chart/export-chart.ts`). Không có render chart phía server, nên "gửi email
   dashboard này dạng PNG" cần một trình duyệt headless — đây không phải phần phụ của alert engine
   mà là hạng mục lớn nhất trong thiết kế này.

## 2. Các quyết định đã chốt

| Câu hỏi | Quyết định |
|---|---|
| Phạm vi | **Cả Alerts và Reports**, như Superset |
| Điều kiện alert | **SQL + validator** (mô hình Superset), không phải ngưỡng theo chart |
| Kênh gửi | **Email (SMTP) + Webhook**; Slack để sau |
| Cấu hình SMTP | **Theo từng workspace**, không phải toàn cục |
| Ảnh report | Dịch vụ trình duyệt headless (đã chấp nhận nằm trong phạm vi) |

Cố ý để sau: Slack app, chia sẻ schedule ở mức đối tượng, định dạng `CSV`/`TEXT` cho report ngoài
phần alert vốn đã đính kèm.

## 3. Kiến trúc thông tin

`Alerts` rời khỏi **Automate** và trở thành một module **Explore**, đặt tên
**"Alerts & Reports"** — tên của Superset, vì trang này chứa cả hai loại và gọi là "Alerts" thì
đặt sai tên cho một nửa.

Các mục Explore không phải danh sách viết tay: chúng đến từ `EXPLORE_MODULES`
(`frontend/src/lib/exploreModules.ts`), nguồn sự thật duy nhất dùng chung với
`useDefaultExplorePath`, và mỗi mục được kiểm soát bằng **feature entitlement** của workspace với
key trùng key module. Nên việc chuyển là: thêm `{ key: "alerts", label: "Alerts & Reports",
icon: Bell, path: "alerts" }` vào danh sách đó và xoá mục khỏi nhóm `Automate`.

Hai hệ quả, đều là mong muốn:

- Module tự động được kiểm soát bằng entitlement (`workspace_features`, migration 0094 — vắng mặt
  nghĩa là bật, nên các workspace hiện có sẽ có sẵn).
- `useDefaultExplorePath` bắt đầu tính đến nó, nên người dùng chỉ bật mỗi Alerts sẽ vào thẳng đó
  thay vì rơi vào route chết.

`Automate` còn lại mỗi `Workflows`. Giữ nguyên: `Workflows` là placeholder riêng có tương lai
riêng, gộp heading bây giờ thì sau lại phải tách ra.

## 4. Mô hình dữ liệu

Schema mới `automation`. Một bảng chứa cả hai loại, như Superset — chúng dùng chung ~80% cột và
toàn bộ phần lập lịch; tách ra là nhân đôi scheduler.

```
automation.schedules
  id, workspace_id, type ('alert'|'report'), name, description, active
  crontab, timezone                       -- khi nào
  -- chỉ alert
  database_id, sql, validator_type ('not_null'|'operator'), validator_config jsonb
  last_eval_at, last_state, last_value, last_value_row jsonb
  -- chỉ report
  chart_id, dashboard_id, report_format ('png'|'pdf'|'csv'|'text')
  custom_width, custom_height
  -- dùng chung
  grace_period_s, working_timeout_s, log_retention_days, email_subject
  created_by, created_at, updated_by, updated_at
  UNIQUE (workspace_id, type, name)

automation.schedule_owners      (schedule_id, user_id)
automation.schedule_recipients  (id, schedule_id, kind ('email'|'webhook'), config jsonb)
automation.schedule_runs        (id, schedule_id, state, started_at, ended_at,
                                 value, error, notification_sent_at)
automation.smtp_config          (workspace_id PK, host, port, username, from_address,
                                 tls_mode, password_secret_id, updated_by, updated_at)
```

`last_state` / `schedule_runs.state` dùng nguyên năm trạng thái của Superset, vì đó là bộ từ vựng
làm trang danh sách đọc được: `success`, `working`, `error`, `noop` ("chưa kích hoạt"),
`grace` ("đang trong thời gian chờ").

**Vì sao mật khẩu SMTP không phải một cột.** Nó đi qua `egret-secrets` dưới dạng typed secret và
bảng chỉ lưu id. Mật khẩu nằm trong một blob JSONB chính là hình dạng sẽ rò ra qua một `SELECT *`
trong lúc hỗ trợ khách hàng.

## 5. Thực thi

### 5.1 Scheduler

Một ticker `tokio::interval` cho mỗi replica trong crate mới `egret-automation`, khởi động từ
`egret-bin` cạnh kernel reaper. Mỗi nhịp giành các schedule đến hạn bằng

```sql
SELECT ... FROM automation.schedules
 WHERE active AND next_run_at <= now()
 FOR UPDATE SKIP LOCKED
```

`SKIP LOCKED` là thứ làm cho N replica an toàn mà không cần bầu leader hay hàng đợi ngoài. Cố ý
**không** dùng k8s CronJob cho mỗi schedule: schedule là dữ liệu người dùng thay đổi lúc chạy, và
một CronJob cho mỗi hàng biến số lượng object của cluster thành hàm của hành vi người dùng.

`working_timeout_s` tồn tại cho trường hợp thiết kế này không ngăn được: một replica chết giữa
chừng để lại hàng ở trạng thái `working`, và không ai nhặt lên nữa. Một lần chạy quá hạn sẽ bị
giải phóng ở nhịp kế tiếp.

`grace_period_s` là luật chống spam: sau khi alert bắn, nó sẽ không bắn lại cho tới hết thời gian
chờ, kể cả điều kiện vẫn đúng. Không có nó, một alert 1 phút trên điều kiện kéo dài sẽ gửi 60 email
mỗi giờ.

### 5.2 Đánh giá alert

Chạy `sql` trên `database_id`, lấy **cột đầu của hàng đầu**, rồi áp validator:

- `not_null` — bắn khi giá trị khác null và khác 0 (luật của Superset).
- `operator` — bắn khi `value <op> threshold` với `<`, `<=`, `>`, `>=`, `==`, `!=`, thêm
  `between` / `not between` với hai cận.

**QUYỀN CỦA AI ĐƯỢC DÙNG ĐỂ CHẠY TRUY VẤN — quyết định bảo mật của thiết kế này.** Một schedule
chạy khi không ai đăng nhập, và cách làm ngây thơ là chạy bằng service account, biến mọi alert
thành đường leo thang đặc quyền: viết SQL trên bảng bạn không được đọc, rồi nhận dữ liệu qua email.
Vì vậy:

- Truy vấn chạy dưới danh nghĩa **chủ sở hữu** schedule, dùng engine token theo người dùng do
  machine IdP cấp (chính token broker đang cấp engine token theo người dùng), không bao giờ dùng
  danh tính dịch vụ dùng chung.
- Phân quyền được đánh giá **lúc chạy, không phải lúc lưu**. Nếu chủ sở hữu mất quyền với bảng đó,
  lần chạy thất bại với `error` và schedule tiếp tục hỏng một cách thấy được, thay vì tiếp tục gửi
  dữ liệu mà chủ sở hữu có thể đã không còn được xem.
- Chủ sở hữu bị vô hiệu hoá thì schedule của họ tắt theo.

### 5.3 Render report

Một Deployment riêng `egret-renderer`: Chromium headless điều khiển bằng Playwright, mở
`POST /render {url, width, height, format}` chỉ trong mạng cluster (không Ingress).

Backend gọi nó kèm URL tới một route in riêng và một **token ngắn hạn, một-đối-tượng, dùng một
lần** phát cho từng lần render: giới hạn đúng một dashboard hoặc chart id, sống ~60s, gắn với chủ
sở hữu schedule. Renderer tuyệt đối không được giữ credential thường trực đọc được mọi dashboard
trong hệ thống — đó là ranh giới giữa một dịch vụ chụp ảnh và một dịch vụ rút dữ liệu.

Hệ thống con này đủ lớn để có **spec riêng** trước khi làm (P4 trong plan); mục này chỉ chốt hợp
đồng và ranh giới bảo mật của nó.

### 5.4 Gửi thông báo

- **Email** — `lettre`, SMTP theo workspace, STARTTLS/TLS ngầm/không. Alert đính kèm các hàng kích
  hoạt dạng CSV; report đính kèm PNG/PDF đã render.
- **Webhook** — `POST` với body JSON `{schedule, type, state, value, triggered_at, rows}`. Có thể
  kèm secret dùng chung, gửi ở header `X-Egret-Signature` dạng HMAC của body để bên nhận xác minh
  nguồn.

**SSRF là rủi ro ở đây và là cái bẫy đã có tiền án trong repo này.** URL webhook do người dùng chọn
theo đúng thiết kế: bất kỳ editor nào cũng có thể trỏ vào `http://169.254.169.254/…` hoặc vào một
service nội bộ trong cluster. Đích đến phải được kiểm tra **sau khi phân giải DNS** đối chiếu với
danh sách chặn gồm dải private, loopback, link-local và metadata, kèm một allow-list rõ ràng cho
vận hành viên thật sự muốn gọi endpoint nội bộ. `url-normaliser-turns-path-into-host` trong lịch sử
repo này chính là kiểu hỏng cần tránh: một kiểm tra đọc path thành host và cho qua một URL đáng lẽ
phải chặn.

## 6. Phân quyền

`alert:manage` — tạo, sửa, xoá, chạy ngay. Gán cho `ws_editor` và `ws_admin`, theo mẫu
`chart:manage` (migration 0014). Quyền đọc suy ra từ việc là thành viên workspace cộng entitlement
của module.

Chia sẻ schedule ở mức đối tượng **ngoài phạm vi v1**: schedule có chủ sở hữu, và editor thấy các
schedule của workspace. Thêm grant theo từng schedule sau này khớp với bộ máy `shares` sẵn có mà
không cần migrate dữ liệu.

## 7. Giao diện

**Trang danh sách** — một trang, hai tab (`Alerts` | `Reports`), mỗi dòng hiển thị tên, badge trạng
thái, lịch (crontab dạng người đọc được), lần chạy cuối, chủ sở hữu, và công tắc bật/tắt. Hình dạng
của Superset; badge trạng thái là thứ khiến trang này đáng mở.

**Trình soạn** — modal với đúng các mục Superset dùng, vì chúng ánh xạ thẳng vào mô hình và người
dùng từ Superset sang sẽ nhận ra ngay: General · Điều kiện alert (SQL + validator) ·
Nội dung (chart/dashboard + định dạng) · Lịch (crontab + múi giờ) · Thông báo (người nhận) ·
Nâng cao (grace period, working timeout, log retention).

**Nhật ký chạy** — drawer cho từng schedule liệt kê các lần chạy gần đây kèm trạng thái, thời lượng,
giá trị và lỗi. Đây là thứ đầu tiên người ta mở khi alert "không chạy", nên nó ra cùng engine chứ
không phải sau đó.

**Workspace settings → Notifications** — host/port/thông tin đăng nhập/địa chỉ gửi của SMTP, kèm nút
**Gửi email thử**. Cấu hình SMTP chưa thử không phân biệt được với cấu hình SMTP hỏng, cho tới lúc
một alert im lặng thất bại lúc 3 giờ sáng.

## 8. Thiết kế này KHÔNG làm gì

- Không Slack app, không PagerDuty, không SMS.
- Không alert theo giá trị của một chart đã lưu — chỉ SQL, như đã chọn.
- Không chia sẻ schedule theo đối tượng.
- Không chạy bù các lần lỡ: schedule bị tắt trong khoảng thời gian của nó sẽ không chạy đuổi.
  Superset cũng vậy.
- Không gửi report dạng `CSV`/`TEXT` cho dashboard (chỉ chart mới có dạng bảng tự nhiên).

## 9. Rủi ro

| Rủi ro | Cách xử lý |
|---|---|
| Alert thành đường leo thang đặc quyền | Chạy dưới danh nghĩa chủ sở hữu, phân quyền lúc chạy (§5.2) |
| Webhook bị lợi dụng làm SSRF | Chặn theo IP sau phân giải DNS + allow-list cho vận hành (§5.4) |
| Renderer giữ credential vạn năng | Token theo từng lần render, một đối tượng, ~60s (§5.3) |
| Replica chết làm kẹt hàng ở `working` | `working_timeout_s` giải phóng cưỡng bức (§5.1) |
| Spam alert khi điều kiện kéo dài | `grace_period_s` (§5.1) |
| SMTP sai cấu hình, lỗi vô hình | Nút gửi thử + nhật ký chạy + trạng thái `error` trên danh sách |
| Chromium headless tốn kém/bất ổn trên kind | Deployment riêng, spec riêng, và là pha cuối — alert ra mắt mà không cần nó |
