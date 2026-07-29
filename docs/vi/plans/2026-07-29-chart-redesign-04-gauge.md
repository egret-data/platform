# Kế hoạch triển khai — Thiết kế lại chart 04: Gauge

> **Cho agent thực thi:** BẮT BUỘC DÙNG SUB-SKILL superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để làm từng task một. Các bước dùng cú pháp checkbox (`- [ ]`).

**Bản tiếng Anh (bản chuẩn):** `docs/en/plans/2026-07-29-chart-redesign-04-gauge.md` — khi hai bản lệch nhau, theo bản tiếng Anh. Code và test đầy đủ nằm ở bản đó.

**Mục tiêu:** Cho người dùng tự định nghĩa các dải màu của gauge, để một gauge đo chỉ số *thấp mới là tốt* thôi nói ngược sự thật với người đọc.

**Kiến trúc:** `gauge.segments` — mảng `{min, max, color, label?}` — thay `gauge.min`/`gauge.max`. Thang đo suy ra từ các dải. Gauge chưa cấu hình tự suy ra ba dải bằng nhau **lúc render** nên trông không đổi. Migration 0102 chuyển những chart đã lưu min hoặc max. Ô sửa là một control `display` trên `fieldDescriptor` sẵn có.

**Spec:** `docs/vi/specs/2026-07-29-chart-redesign-04-gauge.md`

---

## Sự thật rút ra từ đọc code — kiểm lại, đừng suy lại

Đối chiếu với cây mã tại `2638db51` cộng các commit docs của nhánh này.

1. `echarts/options/gauge.ts` vẽ dải bằng colour stop của axisLine, mỗi stop là `[phân_số, màu]` với phân số là vị trí dọc `[min, max]`. `ZONE_LOW/MID/HIGH` là `SERIES_PALETTE[3] / [4] / [1]` — đỏ, vàng, xanh.
2. Thang hôm nay: `min = cfg.min ?? 0`, `max = cfg.max ?? (value > min ? value * 2 : min + 100)`.
3. Gauge khai báo bằng `fieldDescriptor` trong `viz/simple.tsx` với ba trường `data`. **`FieldDescriptorSpec` đã hỗ trợ `display?: VizControl<Cfg>[]`** và pie đang dùng — **đừng** thêm kind mới vào `viz/fields.tsx`.
4. `fromParams` đọc **mọi key dưới prefix**, nên `gauge.segments` tự round-trip, không cần đụng codec.
5. Khuôn ô chọn màu là ô per-slice của pie trong `settings/PieDisplaySection.tsx` — đọc và theo nó.
6. Migration mới nhất là `0101`, nên bản của bạn là `0102`.

---

## Các task

### Task 1: Hình dạng setting
Thêm `GaugeSegment` và `GaugeConfig.segments`; **bỏ `min` và `max`** ở cả `GaugeConfig` lẫn `GaugeBuildConfig`; bỏ hai trường đó khỏi `fields` của gauge trong `simple.tsx`. Để `gauge.ts` biên dịch được bằng một shim giữ nguyên hành vi — task 3 viết lại tử tế.

### Task 2: Migration 0102
Quy tắc: **chỉ chart đã lưu `gauge.min` hoặc `gauge.max` mới được chuyển**, thành đúng ba mốc một phần ba đang vẽ hôm nay với đúng màu hôm nay. Chart chưa lưu gì thì **không** nhận segment nào và rơi vào mặc định lúc render — ghi segment cho chúng là đóng băng một thang tự động đang bám theo dữ liệu.

Đọc `0101` trước: nó có hai chi tiết phải dùng lại — tách sub-object trong `LATERAL` trước khi dùng `-` (vì `-` bind chặt hơn `->`), và chuẩn hoá giá trị không phải object thành `'{}'::jsonb` **bên trong** `LATERAL` thay vì guard bằng `jsonb_typeof(...) AND EXISTS(...)`, vì Postgres không hứa đánh giá `AND` từ trái sang.

Hai nơi lưu như 0101, và nhớ `vizOverride` là ghi đè **một phần** — chỉ ghi `gauge.segments` vào đó khi chính override ấy có `gauge.min`/`gauge.max`.

Màu phải **đọc giá trị hex thật từ `echarts/palette.ts` rồi viết thẳng vào SQL**, kèm chú thích nguồn: migration không import được palette, và màu đổi về sau không được phép lặng lẽ đổi lại chart đã migrate.

Ca **có min nhưng không có max**: tự quyết định, làm, và nói rõ chọn gì và vì sao (gợi ý: hôm nay chart đó render bằng thang tự động từ giá trị, nên đóng băng bất kỳ max nào cũng là đổi nó).

Chạy thử trên database tạm với đủ ca, rồi `touch crates/egret-metastore/src/lib.rs`.

### Task 3: Vẽ từ segments
Không có segment → **render y hệt hôm nay** (ba stop tại `1/3`, `2/3`, `1`, đỏ→vàng→xanh, trên `[0, value*2]`); có test ghim đúng các con số đó. Có segment → thang suy ra từ min nhỏ nhất và max lớn nhất, stop đặt đúng phân số. **Ca "thấp mới là tốt" — đỏ ở trên, xanh ở dưới — phải có test riêng**, vì đó là lỗi đơn vị này sinh ra để sửa. Sắp xếp theo `max` trước khi thành stop, và chặn ca `max === min` thay vì chia cho 0.

### Task 4: Ô sửa dải
**Không đụng `viz/fields.tsx`.** Gauge nhận một control `display`, y như pie.

Khi chưa có segment nào, ô sửa phải hiện **ba dải mặc định sắp được render**, không phải danh sách rỗng — nếu không người dùng không sửa được thứ họ đang nhìn. Xoá tên thì ghi `undefined`, không phải `""` (bài học từ chart 03). Xoá xuống còn một dải thì dừng — gauge không có dải nào thì không có cung.

Bẫy: **đừng sắp xếp trong ô sửa.** Sắp xếp mỗi lần gõ sẽ làm các hàng nhảy chỗ ngay dưới con trỏ. Việc sắp xếp thuộc về builder lúc render (task 3 đã làm).

### Task 5: Verify đầy đủ, deploy, kiểm tra thật, mở PR
Cả hai cổng — có migration nên backend nằm trong phạm vi. Test workspace cần `DATABASE_URL`: cấp database tạm chứ đừng bỏ qua, và xác nhận `_sqlx_migrations` có `102`, đó cũng là cách chứng minh bước `touch` ở task 2 có tác dụng.

**Roll backend trước, xác nhận migration chạy, rồi mới roll frontend** — chart 02 đã cho thấy vì sao. Mở app kèm tham số cache-busting.

Bước 1 của kiểm tra thật là bước quan trọng nhất: gauge chưa cấu hình phải **giống hệt** `main`. Bước 4 là mục đích của cả đơn vị: đỏ ở trên, xanh ở dưới.

**Nhánh này xếp chồng trên `docs/chart-comparison-audit` (PR #357), không phải trên `main`.** Kiểm tra #357 đã merge chưa: rồi thì rebase lên `main` trước khi push; chưa thì nói rõ trong PR và chọn base cho đúng, đừng lặng lẽ gộp commit của bản kiểm vào một PR chart.

**Mở PR rồi DỪNG — không merge, chờ duyệt.**
