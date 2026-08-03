# Control sắp xếp trong chart studio — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại `docs/en/plans/2026-08-03-studio-sort-control.md`
> là bản chuẩn; mã nguồn và các bước chạy lệnh nằm ở đó.

**Mục tiêu:** Cho người dùng nói được chart lấy dữ liệu theo thứ tự nào. Hôm nay chẳng có gì làm được
việc đó — và đó là lý do mọi truy vấn mang `orderby: []` và ba loại chart vẽ danh mục theo thứ tự engine.

**Kiến trúc:** `orderby` gia nhập `filters` và `rowLimit` trong `SharedQueryState` — sắp xếp là chuyện
của truy vấn, đúng cách Metabase mô hình hoá nó (một bước trong notebook, không phải một setting của
viz). Sắp xếp do người dùng đặt sẽ ghi đè mặc định tăng dần mà PR #394 suy ra, theo đúng luật ưu tiên
của chính Metabase.

---

## Vì sao có PR này

PR #394 đã chặn được việc bar, row và box plot vẽ danh mục theo thứ tự engine trả về, bằng hai lớp.
**Nhưng nó không sửa được nguyên nhân**: `SharedQueryState` là `{filters, rowLimit, temporalUnits}`, nên
không descriptor nào diễn đạt được một phép sắp xếp và không có gì trong studio yêu cầu được.

Hệ quả nhìn thấy trên dashboard đối chiếu:

| chart | egret | Metabase |
|---|---|---|
| bar | `Doohickey, Gadget, Gizmo, Widget` | `Widget, Gadget, Gizmo, Doohickey` |

Metabase khác vì câu hỏi của nó được **viết** kèm `ORDER BY revenue DESC`. egret khớp mặc định *ngầm*
của Metabase và không có cách nào diễn đạt cái tường minh.

## Luật ưu tiên — port của Metabase, đừng tự nghĩ ra

`add_implicit_clauses.clj:71`:

> Trường trong `breakout` sẽ thêm một `order-by` tăng dần ngầm định **trừ khi trường đó đã được tham
> chiếu tường minh trong `order-by`**.

Tức là: **sắp xếp tường minh thắng; nếu không thì tăng dần theo breakout.** PR #394 đã cài nửa sau
trong `buildSpec` của `viz/cartesian.tsx`. PR này thêm nửa đầu và **không được phá nửa sau** — chart
không có sắp xếp do người dùng đặt phải hành xử y hệt hôm nay.

## CÁI BẪY MÀ KẾ HOẠCH NÀY SINH RA ĐỂ TRÁNH

**`sortCategories` sẽ âm thầm ghi đè phép sắp xếp người dùng đặt.** Lớp 2 của PR #394 sắp xếp trục danh
nghĩa phía client **bất kể truy vấn yêu cầu gì**, chính là để những chart đã lưu với `orderby: []` có
thứ tự ổn định. Người dùng yêu cầu giảm dần theo doanh thu sẽ được truy vấn honour, dữ liệu trả về đúng
thứ tự đó, rồi bị sắp lại theo bảng chữ cái trước khi vẽ — control trông như không làm gì cả.

Seam đã có sẵn. `cartesian.ts:59` có `preserveCategoryOrder`, do #394 thêm cho bar mode của funnel, và
comment của nó nêu đúng luật mà PR này hoàn thiện:

> Everything that reaches this from a stored `orderby: []` spec must NOT set it — see `sortCategories`.

Vế ngược lại chính là thứ phải nối: **chart có spec lưu mang `orderby` KHÁC RỖNG phải đặt
`preserveCategoryOrder`**, vì engine đã xếp dữ liệu đúng thứ tự người dùng yêu cầu rồi.
`options/boxplot.ts` dùng chung `sortCategories` nên cũng cần xử lý như vậy.

**Đây là trọng tâm của bài kiểm định.** Một control sắp xếp vượt qua unit test nhưng không làm gì trên
màn hình chính là kiểu hỏng mà chương trình này đã ship hai lần.

## Phạm vi

**Trong:** nhiều phép sắp xếp có thứ tự, mỗi cái gồm một trường và một chiều; lưu trữ (miễn phí — `spec.orderby`
vốn đã được lưu và round-trip qua `harnessParams`); phần nối `preserveCategoryOrder`.

**Ngoài, và phải nói rõ trong PR:**
- **Kéo thả để đổi thứ tự danh sách sắp xếp.** `@dnd-kit` đã là dependency và `SeriesListSection` đang
  dùng, nên về sau rẻ; nhưng nút mũi tên là đủ để ship và giữ diff dễ review.
- **Trình dựng filter.** Studio cũng chưa có — filter chỉ đến từ thao tác drill (`ChartExplore.tsx:254`).
  Ngoài phạm vi, nhưng đáng ghi trong PR rằng sắp xếp là mệnh đề truy vấn **do người dùng soạn** đầu
  tiên mà studio có, nên nó đặt ra khuôn mẫu.
- **Sắp xếp theo từng series hay từng cột.** Metabase có một danh sách sort cho mỗi stage truy vấn; PR
  này cũng vậy.

## Những trường nào sắp xếp được

`chartquery.rs:364` (`orderby_sql`) chấp nhận, theo thứ tự: một **metric** (giải ra biểu thức của nó),
một **dimension** trong spec, hoặc một **cột thô**. Ngoài ra là
`unknown order-by field: {field}` — lỗi 400.

Vậy các lựa chọn của picker đúng bằng: `dimensions` + `metrics` của spec + (với viz kiểu raw-records như
table hay box plot) `columns` của nó. **Suy ra từ chính spec mà descriptor dựng**, không phải từ danh
sách cột đầy đủ của dataset, nếu không người dùng có thể chọn một trường mà truy vấn sẽ từ chối.

## Các task

| # | nội dung |
|---|---|
| 1 | `orderby` thành shared query state; mọi `buildSpec` phát nó ra |
| 2 | **Sắp xếp do người dùng đặt không bị renderer ghi đè** — nối `preserveCategoryOrder` |
| 3 | Bản thân control, trong tab Data |
| 4 | Đối chiếu live + mở PR (KHÔNG merge) |

Smartscalar là ngoại lệ: `simple.tsx:335` tự suy orderby của nó có lý do ("Trend so sánh hai kỳ cuối,
nên dữ liệu phải về theo thứ tự dimension"). Phải quyết định sắp xếp do người dùng đặt có ghi đè nó
không, và nói rõ vì sao, dù chọn đằng nào.

**Harness sẽ dịch chuyển**: `scripts/harness/params.ts` chạy qua đúng các `buildSpec` này và
`charts.test.ts` ghim thứ seed phát ra. Cập nhật và nói rõ đã đổi gì.

## Tiêu chí nghiệm thu (Task 4)

Trên `MB · Bar`:

1. Thêm sắp xếp theo `revenue`, giảm dần. **Chart phải vẽ lại thành
   `Widget, Gadget, Gizmo, Doohickey`** — đúng thứ tự card Metabase hiện, vì đó là thứ
   `ORDER BY revenue DESC` của tác giả nó tạo ra.
2. Lưu, tải lại trang, xác nhận đã lưu.
3. Bỏ sắp xếp. Nó phải quay về tăng dần theo dimension, **không** phải về thứ tự engine.
4. **Tải dashboard sáu lần** và xác nhận thứ tự người dùng đặt ổn định cả sáu. Sáu, không phải hai —
   ba lần tải liên tiếp đều ra cùng thứ tự engine chính là thứ đã che lỗi gốc suốt sáu PR.
5. **Không thứ gì khác dịch chuyển** — kiểm lại các chart PR 02–07 đã sửa.

**Kiểm hash bundle** trước khi tin bất kỳ phép đo nào — `index.html` cũ đã gây kết luận sai hai lần
trong chương trình này.

## Điểm yếu đã biết

Danh sách trường sắp xếp được suy từ spec mà descriptor dựng, nên một viz có `buildSpec` đặt trường ở
chỗ bất thường có thể chào ra trường mà backend từ chối bằng 400. Test của Task 3 phải phủ ít nhất một
viz raw-records (table hoặc box plot) và một viz tổng hợp, vì hai loại đi qua hai nhánh khác nhau của
`orderby_sql`.
