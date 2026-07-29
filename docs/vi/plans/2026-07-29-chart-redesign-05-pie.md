# Kế hoạch triển khai — Thiết kế lại chart 05: Pie

> **Cho agent thực thi:** BẮT BUỘC DÙNG SUB-SKILL superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để làm từng task một. Các bước dùng cú pháp checkbox (`- [ ]`).

**Bản tiếng Anh (bản chuẩn):** `docs/en/plans/2026-07-29-chart-redesign-05-pie.md` — khi hai bản lệch nhau, theo bản tiếng Anh. Code và test đầy đủ nằm ở bản đó.

**Mục tiêu:** Cho pie chia theo tối đa ba chiều dưới dạng vòng lồng nhau, và cho người dùng chăm chút các lát — đúng những khoảng cách mà dòng pie trong bảng so sánh bảo là không tồn tại.

**Kiến trúc:** `pie.dimensions: string[]` thay `pie.dimension`, nên số vòng chính là độ dài mảng. Mỗi vòng là một series pie riêng của ECharts ở bán kính khác nhau, dựng bằng chính `buildSlices` áp cho từng tầng. Vòng ngoài kế thừa màu lát cha, làm nhạt trong OKLCH. Tên/thứ tự/ẩn lát áp cho chiều thứ nhất. Migration 0103 nâng khoá cũ.

**Spec:** `docs/vi/specs/2026-07-29-chart-redesign-05-pie.md`

---

## Sự thật rút ra từ đọc code — kiểm lại, đừng suy lại

Đối chiếu với `9a8e810b`.

1. `buildSlices` làm việc thật: bỏ số 0, xử lý toàn âm (lấy trị tuyệt đối) so với âm-dương lẫn lộn (bỏ số âm), sắp xếp giảm dần trừ khi `sortRows === false`, rồi gộp các lát dưới ngưỡng thành một lát `"Other"` **chỉ khi gộp được từ 2 lát trở lên**. Tất cả phải tiếp tục đúng, cho **từng vòng**.
2. `dimensionUnit` không được lưu — `ChartRenderer` suy ra từ `params.spec.temporal_units?.[pie.dimension ?? ""]`. Nhiều chiều thì thành tra theo từng vòng, không cần setting mới.
3. Pie là `fieldDescriptor` với `fields: [dimension, metric]`.
4. `FieldSpec.kind` chỉ có bốn kind vô hướng, **và `display` không giúp được** — chiều là truy vấn nên thuộc section Data, nơi chỉ `fields` render.
5. `palette.ts` **không có** hàm làm nhạt, và `zrender` **không phải** dependency trực tiếp (chỉ `echarts`), nên `zrender/lib/tool/color` là thò tay vào ruột gói bắc cầu — không dùng.
6. Migration mới nhất là `0102`.
7. SVG renderer **không giải** `var()` — chart 04 đã xác lập. Màu vòng phải là chuỗi cụ thể khi tới option.

---

## Các task

### Task 1: Hình dạng setting
`dimensions?: string[]` thay `dimension`, thêm `sliceNames`/`hiddenSlices`/`sliceOrder`/`decimals`. `buildSpec` dùng `c.dimensions ?? []`. Để builder tạm chỉ vẽ **một** vòng từ `dimensions[0]`; task 4–6 làm phần còn lại.

### Task 2: Kind `dimensions` cho `fieldDescriptor`
Multi-select có thứ tự kèm `max`. Cột đã chọn không được mời lại; hàng "Add" biến mất khi đủ `max`. Kiểm xem `temporal: true` hành xử ra sao với trường nhiều giá trị — quyết định hàng chọn đơn vị ngày hiện theo **từng** chiều hay chỉ chiều đầu, làm, và nói rõ chọn gì và vì sao.

### Task 3: Migration 0103
`pie.dimension` (chuỗi) thành `pie.dimensions` (mảng một phần tử); xoá khoá cũ. Hai nơi lưu, `vizOverride` chỉ ghi ở nơi thật sự mang khoá cũ.

Đọc `0102` trước — ba điều phải dùng lại, hai trong số đó do review tìm ra chứ không phải viết đúng ngay từ đầu: tách sub-object trong `LATERAL` trước khi dùng `-`; chuẩn hoá phi-object thành `'{}'::jsonb` **bên trong** `LATERAL`; và **gate bằng `jsonb_typeof(...) = 'string'`, không phải bằng sự tồn tại khoá** — bản nháp đầu của 0102 ghi ra rác đúng vì chỗ này. Ở đây không có cast, nhưng vẫn phải **báo cáo là đã kiểm** xem bài học cast có áp dụng không.

Chạy thử trên database tạm với đủ ca (chuỗi, `null`, không có khoá, giá trị không phải chuỗi, override có/không, settings là chuỗi/mảng/`null`, chạy hai lần), rồi `touch crates/egret-metastore/src/lib.rs`.

### Task 4: Vòng lồng nhau
Mỗi chiều một series pie ở bán kính tăng dần. Vòng *n* gom theo `dimensions[0..n]`, và các lát con phải nằm **liền nhau** theo đúng thứ tự lát cha. Giữ `buildSlices` làm hàm cho từng tầng thay vì nhân bản luật của nó. Có test cho: một chiều render **y hệt** hôm nay; gộp `"Other"` áp **theo từng vòng**; giá trị null ở vòng ngoài không làm hỏng lát cha.

### Task 5: Màu các vòng
Viết `lightenOklch(hex, amount)` trong `palette.ts` — sRGB → tuyến tính → OKLab → tăng L → quay lại. Khoảng 35 dòng, ghi công thức trong chú thích để người đọc kiểm được. Vòng *n* dùng màu lát cha làm nhạt *n* bậc. `sliceColors` do người dùng đặt vẫn thắng, và **con của lát bị ghi đè phải làm nhạt từ màu ghi đè**, không phải từ palette.

### Task 6: Chăm chút lát và số thập phân
Đổi tên, ẩn, sắp xếp lát của **chiều thứ nhất**, cộng số thập phân của phần trăm. Nói rõ trong panel là chúng áp cho chiều thứ nhất.

Ca quan trọng nhất: **ẩn một lát thì phần trăm còn lại phải tính lại theo cơ sở mới** — hãy assert một giá trị phần trăm, không chỉ assert lát đã biến mất. Một lát bị ẩn mà vẫn tính vào tổng là một chart nói dối về tỉ lệ.

### Task 7: Verify đầy đủ, deploy, kiểm tra thật, mở PR
Cả hai cổng; cấp database tạm cho test cần DB và xác nhận `_sqlx_migrations` có `103`.

**Migration cần fixture có thể chưa tồn tại** — kiểm xem database kind có pie nào lưu `pie.dimension` không. Gauge thì không có, và một migration chỉ kiểm trên bảng rỗng là chưa được kiểm. Nếu không có, lưu một cái trên `main` trước.

Roll backend trước, xác nhận migration chạy, rồi mới frontend; mở app kèm tham số cache-busting.

Bước 1 của kiểm tra thật là nền của mọi thứ: pie một chiều **không đổi**. Bước 3 — vòng ngoài phải nhận ra được là thành viên nhạt hơn cùng họ màu — là phán đoán chỉ mắt người làm được, nên **hãy nhìn** chứ đừng khẳng định.

**Mở PR rồi DỪNG — không merge, chờ duyệt.**
