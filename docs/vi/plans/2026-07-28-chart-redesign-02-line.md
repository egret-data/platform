# Kế hoạch triển khai — Thiết kế lại chart 02: Line

> **Cho agent thực thi:** BẮT BUỘC DÙNG SUB-SKILL superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để làm từng task một. Các bước dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Bản tiếng Anh (bản chuẩn):** `docs/en/plans/2026-07-28-chart-redesign-02-line.md` — khi hai bản lệch nhau, theo bản tiếng Anh.

**Mục tiêu:** Cho line chart một bộ setting trung thực về hình dạng, giá trị thiếu và điểm đánh dấu — mỗi thứ là một lựa chọn nhiều trạng thái thật sự chứ không phải boolean — kèm ghi đè theo từng series, và migrate chart đang có mà không đổi hình dạng của bất kỳ chart nào.

**Kiến trúc:** Ba setting chung cả chart (`lineShape`, `missing`, `markers`) thay bốn boolean trong namespace `cartesian.*` dùng chung, cộng map `seriesLine` chứa ghi đè per-series theo đúng khuôn `seriesColors`/`seriesLabels` sẵn có. Migration 0101 ghi lại cả hai nơi lưu settings. Việc vẽ vẫn nằm trong `decorateSeries`; chỉ `zero` cần biến đổi dữ liệu, vì ECharts không có chế độ null-thành-0.

**Spec:** `docs/vi/specs/2026-07-28-chart-redesign-02-line.md`

---

## Sự thật rút ra từ đọc code — đừng suy lại, và đừng làm trái

Đã đối chiếu với cây mã tại `1fe27573`. Nếu đoạn code trong task lệch với thứ bạn thấy, **DỪNG và báo lại** thay vì làm cho bằng được.

1. `decorateSeries` trong `frontend/src/components/chart/echarts/options/cartesian.ts` **đã** đặt sẵn cho mọi series line:
   ```ts
   out.lineStyle = { width: CHART_THEME.lineWidth, type: lineType(colorIdx) };
   out.symbol = symbolShape(colorIdx);
   out.showSymbol = cfg.showMarkers !== false;
   out.connectNulls = cfg.connectNulls ?? true;
   if (cfg.step) out.step = "middle";
   out.smooth = !!cfg.smooth;
   ```
   **`lineType(colorIdx)` đang chủ ý xoay vòng liền/đứt/chấm theo từng series** — đây là tính năng trợ năng để người mù màu phân biệt được các đường. Vậy `style` per-series là **ghi đè một vòng xoay tự động đang có**, không phải thêm năng lực mới. `symbolShape(colorIdx)` cũng vậy.
2. Chế độ bậc thang của egret là `"middle"`. Của Metabase là `step-after`. **Giữ `"middle"`** — đổi đi sẽ làm mọi chart bậc thang đang có trông khác, đúng thứ đơn vị này sinh ra để tránh.
3. Migration nằm ở `./migrations` tại gốc repo; mới nhất là `0100_dashboard_viz_override_v2.sql`.
4. Settings nằm ở **hai** nơi: `analytics.charts.params -> 'settings'` và `analytics.dashboard_cards.options -> 'vizOverride'`. Cả hai đều là object phẳng khoá theo `cartesian.<field>`. Migration 0101 phải xử lý cả hai.
5. `CartesianConfig` ở `frontend/src/api/charts.ts`; `KEYS` trong `frontend/src/components/chart/viz/cartesian-codec.ts` là `as const satisfies readonly (keyof CartesianConfig)[]`, nên type và KEYS phải đổi cùng nhau.
6. `ChartRenderer` giờ spread cả config cartesian vào builder, nên không phải sửa danh sách chuyển trường nào. **Đừng dựng lại danh sách đó.**

---

## Cấu trúc tệp

- `migrations/0101_line_shape_settings.sql` — mới; ghi lại cả hai nơi lưu settings.
- `frontend/src/api/charts.ts` — `CartesianConfig`: thêm ba trường + `seriesLine`, bỏ bốn trường.
- `frontend/src/components/chart/viz/cartesian-codec.ts` — `KEYS` theo sau.
- `frontend/src/components/chart/echarts/options/cartesian.ts` — `CartesianBuildConfig` theo sau; `decorateSeries` đọc setting mới; thêm phép biến đổi cho `zero`.
- `frontend/src/components/chart/settings/CartesianDisplaySection.tsx` — điều khiển chung cả chart trong `SeriesOptions`; ghi đè per-series trong các hàng per-series sẵn có.
- `frontend/src/components/chart/echarts/__tests__/cartesian.line.test.ts` — mới.
- `frontend/src/components/chart/settings/__tests__/CartesianDisplaySection.test.tsx` — mở rộng.

---

## Các task

Nội dung chi tiết từng bước, kèm code và test đầy đủ, nằm ở bản tiếng Anh. Tóm tắt để theo dõi:

### Task 1: Hình dạng setting
Bỏ `smooth`/`step`/`connectNulls`/`showMarkers` khỏi `CartesianConfig` và `CartesianBuildConfig`, thêm `lineShape`/`missing`/`markers`/`seriesLine` + type `LineSeriesStyle`; `KEYS` theo sau. Test codec: round-trip setting mới, và **không** còn key cũ nào. Việc gỡ bốn trường sẽ làm hỏng mọi nơi đang đọc — `tsc` sẽ chỉ ra; **đừng thêm lại trường**, cứ để Task 3 và Task 7 viết lại.

### Task 2: Migration 0101
Quy tắc: **với mọi chart cartesian và mọi ghi đè của dashboard card, ghi cả ba key mới sao cho chart trông y hệt hôm nay, rồi xoá bốn key cũ.**
- `lineShape`: `step` thắng `smooth`, rồi tới `smooth`, còn lại `linear`.
- `missing`: `connectNulls = false` → `none`, còn lại `interpolate`.
- `markers`: `showMarkers = false` → `off`, **còn lại `on`** — không bao giờ `auto`. Builder hôm nay hiện marker trừ khi bị tắt, nên `auto` sẽ làm chart cũ đổi hình. Chart mới nhận `auto` từ mặc định của builder.

Chạy thử trên database thật qua port-forward, với các hàng phủ: cả `step` và `smooth` cùng true; `connectNulls: false`; không có key marker; một ghi đè dashboard card; và một chart **không** có key `cartesian.` nào (phải để nguyên). Rồi `touch crates/egret-metastore/src/lib.rs` để migrator nhúng nhận tệp mới.

### Task 3: Vẽ hình dạng đường
`decorateSeries` giải `ln = cfg.seriesLine?.[s.name]` trước, rồi `shape = ln.shape ?? cfg.lineShape ?? "linear"`.

### Task 4: Giá trị thiếu, kèm phép biến đổi zero
`missing` **chỉ có ở cấp chart** — `LineSeriesStyle` chủ ý không có `missing`, vì cách đọc một khoảng trống là phát biểu về dữ liệu, hai series trong cùng chart mà bất đồng thì chart nói hai điều cùng lúc. `zero` phải đổi **dữ liệu** vì ECharts không có chế độ đó. Có test riêng: bar **không** được đổi null thành 0 — cột thiếu và cột bằng 0 nói hai chuyện khác nhau.

### Task 5: Marker với chế độ auto
`MARKER_AUTO_MAX = 30` là **TẠM**, chốt lại bằng chart thật ở Task 8.

### Task 6: Nét và độ dày theo series
Đọc kỹ sự thật số 1 ở trên trước khi làm. `style` per-series ghi đè vòng xoay trợ năng cho **riêng series đó**; đừng gỡ vòng xoay. Kiểm giá trị thật của `CHART_THEME.lineWidth` rồi chọn S/M/L sao cho **M đúng bằng độ dày hiện tại**.

### Task 7: Panel
Task 1 đã xoá bốn công tắc cũ; task này đặt lại mỗi setting một điều khiển đúng hình dạng. Dùng lại `AutoToggle` sẵn có cho markers. Ghi đè per-series phải **gộp**, không được ghi đè cả map:
```tsx
onChange({ seriesLine: { ...config.seriesLine, [name]: { ...config.seriesLine?.[name], style: v } } })
```
Có test ghim đúng lỗi này — đặt cho một series không được xoá mất series khác.

### Task 8: Verify đầy đủ, deploy, kiểm tra thật, mở PR
Khác chart 01: đơn vị này **có migration**, nên cổng backend cũng nằm trong phạm vi — `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`. Đừng chạy hai `cargo test --workspace` cùng lúc. Backend cũng phải build và roll lại, rồi **kiểm tra trong database xem migration đã chạy chưa**, đừng đoán.

**Roll backend TRƯỚC frontend.** Task 1 đã gỡ bốn key cũ khỏi codec, nên chart còn lưu `cartesian.smooth` sẽ đọc ra `lineShape: undefined` và vẽ mất độ mượt. Migration 0101 mới là thứ bịt khoảng đó. Khi squash-merge thì hai thứ là một commit nên không có vấn đề, nhưng quy trình deploy tay hai bước này có thể đặt frontend mới trước database chưa migrate — và triệu chứng (mọi line chart lặng lẽ mất hình dạng) trông **y hệt** lỗi render của task 3. Roll `deploy/egret`, xác nhận migration đã chạy, rồi mới roll frontend.

Sau khi deploy, mở app kèm tham số cache-busting — trình duyệt cache `index.html`, chart 01 đã mất một vòng vì chụp nhầm bundle cũ trông y hệt.

Ca số 5 của kiểm tra thật — mở **Revenue over time (line)** đã lưu từ trước — chính là bài kiểm tra thật sự của migration.

Chụp ảnh trước/sau ở cả hai kích thước. Muốn có ảnh "trước" thật thì deploy `main` trước, chụp, rồi deploy lại nhánh.

**Mở PR rồi DỪNG — không merge, chờ duyệt.**
