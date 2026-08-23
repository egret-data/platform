# Một schedule chỉ được trỏ tới object trong workspace của chính nó

*(Bản tiếng Anh tại `docs/en/specs/2026-08-23-report-target-workspace-scoping-design.md` là bản chuẩn; bản này để tham chiếu.)*

Một report hoặc alert ở workspace này có thể trỏ tới chart hoặc dashboard ở workspace
khác, và dữ liệu được giao đi. Đây không phải giả thuyết: nó đã được chạy trên StarRocks
thật với SMTP sink thật, và file đính kèm tới nơi mang theo dòng dữ liệu của workspace kia.

Mọi điều dưới đây được **đo trước khi viết**. Probe nằm ở
`.superpowers/sdd/chart-target-scoping-probe.md`; tài liệu này trích dẫn nó chứ không kể
lại, và đánh dấu rõ những gì **chưa xác lập được**.

## 1. Điều đã chứng minh

Một `ws_admin` của workspace 1, **không có bất kỳ grant nào** trên workspace 2:

- lưu được `chart_id` và `dashboard_id` của workspace 2 vào một schedule — **HTTP 200**,
  ở cả `create` lẫn `update`;
- lưu được object của workspace 2 qua `automation.schedule_targets`, **và một UUID không
  tồn tại ở bất cứ đâu**, vì cột đó không có khoá ngoại;
- nhận được, tại SMTP sink thật, một file đính kèm 39 byte nội dung `"secret_col"` /
  `"WS2-CONFIDENTIAL-ROW-42"` — dữ liệu của workspace 2, ra khỏi workspace 2;
- và khi replay đúng render token mà chính runner đã mint, nhận lại
  `"title":"ws2 secret dashboard"` cùng `"rows":[["WS2-CONFIDENTIAL-ROW-42"]]` từ
  `/render/payload` và `/render/data` — hai endpoint chỉ xác thực bằng token.

Riêng biệt: các FK đều là `ON DELETE CASCADE`, nên workspace 2 xoá chart **của chính họ**
đã âm thầm đưa schedule của workspace 1 từ một hàng về không.

### Chưa xác lập được

Rằng Chromium sau đó vẽ những dòng ấy vào file PNG được giao. Tại điểm đo được, dữ liệu
đã qua biên giới API rồi, nên điều này không đổi mức nghiêm trọng — nhưng nó chưa được
lái thật, và không được khẳng định.

Rằng engine của một connection **delegated** có từ chối hay không. Cái *đã* đo được là
egret không từ chối: nhánh delegated chỉ đổi đúng một thứ so với không-delegated — một
bearer cho owner của workspace 1 — rồi vẫn gửi SQL của workspace khác đi. Không có nhánh
nào trong `build_chart_report`, `run_card` hay `owner_identity` có thể trả về "denied"
theo tenancy. **Nên mọi bảo vệ ở đó là theo chính sách, không theo cấu trúc**, và bản cài
mặc định ship `ranger.enabled: false` với Trino ở `access-control.name=allow-all`.

## 2. Vì sao không có gì chặn lại

| | |
|---|---|
| `migrations/0110:34-35` | cả hai FK chỉ kiểm tồn tại, `ON DELETE CASCADE`, không có vị từ workspace |
| `migrations/0115:14-24` | `schedule_targets.object_id` **không có FK và không có cột workspace**; `kind` là đa hình |
| `egret-automation/src/routes.rs` | **0** lần xuất hiện `chart_id` hay `dashboard_id`. Không có gate object, cả create lẫn update |
| `egret-bi/src/chart.rs:82` | `ChartRepo::get(id)` là `SELECT … WHERE id = $1` |
| `egret-bi/src/dashboard.rs:177` | `DashboardRepo::get(id)` cũng vậy |
| `egret-automation/src/runner.rs:583-603` | `build_chart_report` truyền `chart.workspace_id` — workspace của **chính cái chart** — vào `compile_chart_sql` |

Connector không-delegated vứt bỏ owner và gửi service user của connection làm
`X-Trino-User`, nên `owner_identity` không cứu được.

## 3. Ba entry point, không phải một

Phát hiện quan trọng nhất của probe, vì nó quyết định một bản vá đúng phải phủ những gì:

- **`build_chart_report`** — chỉ đọc `s.chart_id`, không bao giờ đọc targets. Đây là
  đường `csv` và `text`.
- **`render_image_report`** — đọc `targets_for` trước, chỉ rơi về `chart_id` khi danh
  sách rỗng. Đây là `png` và `pdf`, và nó **ghi đè** `chart_id`.
- **`render_alert_attachment`** (`runner.rs:767-800`) — mint render token từ
  `report_target`, trên một **alert**. `model.rs:176-179` chỉ cấm nêu *cả hai* chart và
  dashboard trên một alert; nó không bao giờ bắt buộc phải có, và không bao giờ kiểm
  quyền sở hữu.

Nên bản vá khoanh theo `chart_id` sẽ trượt `schedule_targets`, còn bản vá khoanh theo
"report" sẽ trượt alert.

## 4. Bản vá

### 4.1 Làm cho phép đọc không-scope trở thành không biểu đạt được

`ChartRepo::get(id)` thành `get(id, workspace_id)`, `DashboardRepo::get` tương tự. Một
caller không thể quên một tham số mà trình biên dịch đòi, và tính chất đó áp dụng cho cả
caller thứ mười hai chưa ai viết — đó chính là khác biệt giữa cách này và một cái gate mà
mỗi chỗ gọi phải tự nhớ. Đây là nguyên tắc repo đã tự ghi — *unrepresentable beats
unforgettable* — áp vào phép đọc thay vì vào một CHECK.

Bán kính đã đo: **11 chỗ gọi**, 8 cho chart và 3 cho dashboard, nằm ở `egret-automation`
và `egret-bi`. Bốn trong số đó ở `render/payload.rs` — chính đường token đang rò — nên
thay đổi rơi đúng vào mã đang rò chứ không phải bên cạnh nó.

Mỗi caller truyền workspace **của schedule**, không bao giờ của object. Object ngoại lai
khi đó không resolve được, và đường `NotRunnable("report's chart no longer exists")` sẵn
có tiếp quản.

**Hệ quả, nói rõ vì đây là thay đổi hành vi thật.** Một schedule đã đang giữ target ngoại
lai sẽ bắt đầu hỏng **lúc chạy** với "no longer exists" thay vì bị từ chối lúc lưu. Đó là
chiều đúng — nó fail-closed — nhưng vẫn là hỏng lúc chạy cho một hàng từng được chấp nhận,
nên §4.2 tồn tại để chặn hàng mới, còn run log là nơi người vận hành gặp hàng cũ.

### 4.2 Validate target ở đường ghi

`schedule_targets.object_id` không thể nhận khoá ngoại: `kind` là đa hình, nên cột đó trỏ
tới hai bảng khác nhau. Vì vậy nó nhận validation ở tầng ứng dụng — lúc create và update,
mọi `(kind, object_id)` phải **tồn tại** và thuộc workspace của schedule — cộng một lần
kiểm lại trước khi render, làm phòng thủ chiều sâu cho những hàng đã ghi trước khi bản vá
này lên.

Cùng validation đó phủ luôn `chart_id` và `dashboard_id`, để từ chối-lúc-lưu là trải
nghiệm thường gặp còn hỏng-lúc-chạy ở §4.1 là lưới cuối chứ không phải cửa chính.

### 4.3 Những gì bản vá cố ý không làm

**Không thêm cột workspace vào `schedule_targets`.** Làm vậy là nhân bản một sự thật vốn
đã nằm trên object, mà ứng dụng thì vẫn phải điền cho đúng — tức là dời lỗi đi chỗ khác
chứ không bỏ được nó.

**Không tách `schedule_targets` thành các bảng theo kind với FK thật.** Đó là hình dạng
sạch nhất về mặt tham chiếu và là câu trả lời đúng về dài hạn, nhưng là một migration lớn
nằm dưới một lỗ rò xuyên tenant đang mở, và lỗ rò không nên chờ nó.

**Không đổi `ON DELETE CASCADE`.** Một khi mọi target đều cùng workspace thì cascade là
đúng: chủ sở hữu object xoá nó thì kéo theo schedule đang render nó. Ràng buộc xuyên
tenant biến mất cùng với target xuyên tenant.

**Không sửa nhánh impersonate của `owner_identity`.** Probe thấy nó mint một JWT mà
connection không được cấu hình để dùng, nên `EXECUTE AS … WITH NO REVERT` bị bỏ qua im
lặng và lượt chạy hỏng ở engine. Đó là lỗi độ tin cậy và nó **fail-closed** — nó thu hẹp
phơi nhiễm này chứ không mở rộng, gói gọn vào connection delegated **jwt-mode**. Nó có
issue riêng.

## 5. Chứng minh thế nào, chứ không phải khẳng định

Probe chính là kế hoạch kiểm thử. Mỗi mục dưới đây phải **đỏ trước** bản vá và **xanh sau**:

- workspace 1 nêu `chart_id` của workspace 2 — bị từ chối ở create, và ở update;
- tương tự qua `schedule_targets`, cho cả kind `chart` lẫn `dashboard`;
- một `schedule_targets.object_id` nêu UUID không tồn tại ở đâu — bị từ chối;
- một **alert** mang `chart_id` ngoại lai — bị từ chối, vì `render_alert_attachment` là
  entry point mà một bản vá hình-dạng-report sẽ bỏ sót;
- một hàng ngoại lai đã tồn tại từ trước — render fail-closed thay vì giao đi;
- và đối chứng âm: một target **cùng workspace** vẫn render và vẫn giao được, vì một bản
  vá làm hỏng ca đang chạy đúng thì không phải bản vá.

Mọi test phân quyền phải chạy dưới một principal **không phải admin**. Admin qua được mọi
lớp kiểm tra, nên phép đo bằng admin sẽ vẫn xanh kể cả khi bản vá bị xoá.
