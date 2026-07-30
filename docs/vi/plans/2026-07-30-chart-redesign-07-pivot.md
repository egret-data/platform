# Thiết kế lại chart 07 — Pivot: kế hoạch triển khai

**Spec:** `docs/vi/specs/2026-07-30-chart-redesign-07-pivot.md`
**Quy tắc đối chiếu Metabase:** `docs/vi/plans/2026-07-30-chart-redesign-metabase-parity-rules.md`
**Ngày:** 2026-07-30
**Bản tiếng Anh (bản chuẩn):** `docs/en/plans/2026-07-30-chart-redesign-07-pivot.md`

Mọi đường dẫn tệp và số dòng bên dưới đều được đọc ngày 2026-07-30. Lỗi chặn được cô lập bằng cách
chạy năm spec thật trên backend, không phải bằng đọc suông.

**Backend NẰM TRONG phạm vi** (khác chart 06). Không có migration, nhưng backend phải roll trước
frontend — một frontend gửi pivot có bucket tới backend chưa sửa sẽ nhận 502.

---

## T1 — Lỗi chặn: `GROUPING()` nhận biểu thức (backend)

**Tệp:** `crates/egret-bi/src/chartquery.rs`, nhánh `has_rollup` (~dòng 399).

Hiện mọi biểu thức chiều đều bị nhúng thẳng vào cả `GROUPING()` lẫn `GROUP BY GROUPING SETS`. Khi
một chiều có đơn vị thời gian, biểu thức của nó là lời gọi hàm, và
`GROUPING(date_trunc('month', "order_date"))` là lỗi **parse** trong Trino.

**Thay đổi:** khi `rollup_groups` khác rỗng, bọc nguồn vào một bảng dẫn xuất trong đó mọi biểu thức
chiều được vật chất hoá dưới tên riêng, rồi chỉ tham chiếu các tên đó:

```sql
FROM (SELECT <dim_expr> AS <dim_name>, … , <cột nguồn mà metric cần> FROM <source>) AS __b
```

rồi sinh `GROUPING("dim_name")` và `GROUPING SETS (("a","b"), …)`.

Ba chi tiết quyết định đúng/sai:

- **Biểu thức metric vẫn phải phân giải được.** Một metric như `sum(totalprice)` tham chiếu cột thô,
  nên `SELECT` bên trong phải chiếu ra thứ metric cần, không chỉ các chiều. Dạng đơn giản mà đúng:
  chiếu `*` cộng các biểu thức chiều đã đặt alias — nhưng khi đó alias chiều có thể **đụng** cột thô
  cùng tên (`date_trunc(...) AS "order_date"` bên cạnh `order_date`). Hãy đặt alias bucket theo tên
  dành riêng, người dùng không chạm tới được (`__dim_0`, theo đúng quy ước `__grp_i` sẵn có) và chọn
  cột tường minh thay vì dựa vào `*`.
- **`WHERE` vẫn nằm trên cột thô.** `render_filter` đã ghi rõ filter so sánh timestamp thật, không
  bao giờ so với khoá đã truncate (`chartquery.rs:284-286`). Vậy filter phải được áp ở truy vấn
  **bên trong**, nơi cột thô còn tồn tại.
- **Chỉ bọc khi có rollup.** Nhánh không rollup đang chạy tốt và được dùng khắp nơi; giữ nguyên từng
  byte để việc này không thể làm hồi quy mọi loại chart khác.

**Đừng "sửa" bằng cách bỏ các cột marker.** `__grp_i` chính là cách `indexResult`
(`pivot/engine.ts:35-49`) phân biệt hàng subtotal với hàng lá. Bỏ đi là hỏng pivot.

### Ma trận test (đây mới là trọng tâm của task)

Lỗi sống sót vì rollup và đơn vị thời gian đều được test riêng còn phần giao thì không — bốn test
rollup đều truyền `temporal_units: HashMap::new()`. Hãy bổ sung theo ma trận:

|  | không đơn vị | truncate (`month`) | extract (`day_of_week`) |
|---|---|---|---|
| không rollup | đã có | đã có | **thêm** |
| một nhóm rollup | đã có | **thêm** | **thêm** |
| hai nhóm rollup (pivot thật) | đã có | **thêm** | **thêm** |

Khẳng định SQL sinh ra chứa `GROUPING("…")` trên một **định danh trần**, và không có `GROUPING(` nào
theo sau bởi `(`. Chạy cho cả hai engine. Kiểm bằng đột biến: gỡ phần bọc, xác nhận test mới fail,
khôi phục, ghi lại phép đột biến.

## T2 — Subtotal theo từng chiều

Metabase: `pivot_table.column_show_totals`, theo từng chiều hàng, mặc định bật cho mọi chiều trừ
chiều cuối (`settings.ts:280-322`) — subtotal dưới chiều trong cùng chỉ lặp lại hàng lá, cũng chính
là việc mà `condense` sinh ra để xử lý.

- `PivotConfig.showColTotals` có thêm dạng theo chiều: giữ boolean làm lối tắt toàn cục, thêm
  `dimTotals?: Record<string, boolean>` phân giải lúc render, mặc định theo quy tắc Metabase. Mặc
  định phân giải ở builder, không bao giờ ở settings đã lưu — quy tắc của chương trình từ chart 01.
- `engine.ts` đọc nó ở chỗ tính `hasSubtotal` (`engine.ts:226`).
- Editor: một công tắc cho mỗi chiều hàng trong mục Totals của `viz/pivot.tsx`.

## T3 — Kéo giãn cột, có lưu

`pivot.columnWidths: { left?: number[]; value?: Record<number, number> }`, phản chiếu cách Metabase
tách `leftHeaderWidths` / `valueHeaderWidths` — card 10 ship giá trị thật cho cả hai, nên đây là
setting mà chính dashboard mẫu của nó phụ thuộc.

- Một tay cầm kéo ở mép phải mỗi ô header; `dnd-kit` đã là dependency (chart 03 dùng cho việc đổi
  thứ tự cột — **kiểm tra trước khi thêm bất cứ gì**), và chart 03 cũng đã xác lập rằng thao tác chỉ
  dùng chuột là một khiếm khuyết: hãy cho tay cầm kéo giãn được bằng bàn phím.
- Độ rộng áp qua `<colgroup>` để ô thân không phải tự biết bề rộng của mình.
- Lưu theo đúng đường của `collapsedRows` — trạng thái tương tác lưu cùng chart, và **cố ý không**
  thuộc `buildSpec`, nếu không kéo giãn một cột sẽ chạy lại truy vấn (`viz/pivot.tsx:116-118` giải
  thích điều đó cho collapse và sort).

## T4 — Ảo hoá, có điều kiện phải đo trước

**Đừng bắt đầu cho tới khi T1 vào và đã có số liệu.** Khi pivot có bucket cuối cùng cũng chạy được,
hãy dựng một pivot thật lớn và ghi lại: số hàng kết quả, số node DOM, thời gian vẽ lần đầu, độ mượt
khi cuộn — trên cả hai app cùng viewport. Metabase render 111 ô cho kết quả 48 hàng nhờ `Grid` + hai
`Collection` + `ScrollSync`; egret render mọi ô.

Nếu egret vẫn thoải mái ở kích thước thực tế, **ghi số liệu lại và bỏ qua task này.** Nếu không, chỉ
dựng cửa sổ cho hàng thân (header vốn nhỏ) và giữ nguyên các pane sticky. Quy tắc parity tồn tại vì
chart 05 đã viết lại một biểu đồ mà nó chưa đo.

## Thứ tự

T1 làm một mình trước — đó là lỗi truy vấn hỏng, mọi thứ khác chỉ là bề mặt bên cạnh nó. Rồi T2 và
T3 song song (độc lập nhau). T4 sau cùng, có điều kiện.

## Cổng kiểm chứng

Rust: `cargo test -p egret-bi`, fmt, clippy. Frontend: typecheck, lint, test, build. Deploy lên kind,
**backend trước**, rồi frontend; kiểm tra thật trong trang mới với hash bundle đã xác nhận. Chạy toàn
bộ CI ở local — GitHub Actions đã hết quota thanh toán, mọi job fail ở giây thứ 3 mà không chạy step
nào.

Chạy đủ chín ca kiểm tra thật trong spec. Mọi test mới đều kiểm bằng đột biến và ghi lại phép đột
biến.

Mở PR rồi **dừng** chờ review.
