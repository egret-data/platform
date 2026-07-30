# Thiết kế lại chart 07 — Pivot Table

**Trạng thái:** thiết kế đã duyệt, chờ kế hoạch triển khai
**Ngày:** 2026-07-30
**Spec chương trình:** `docs/vi/specs/2026-07-28-chart-redesign-program-design.md`
**Quy tắc đối chiếu Metabase:** `docs/vi/plans/2026-07-30-chart-redesign-metabase-parity-rules.md`
**Đơn vị trước:** scalar `2ae3f6b1`
**Bản tiếng Anh (bản chuẩn):** `docs/en/specs/2026-07-30-chart-redesign-07-pivot.md`

## Bảng so sánh của chương trình KHÔNG có dòng nào cho pivot

Chart 03 sửa dòng table sai, chart 04 sửa dòng gauge sai, chart 05 sửa dòng pie sai. Pivot thì
không có dòng nào — bảng so sánh chỉ phủ 4/19 loại. Nên spec này được viết bằng cách **đọc hết cả
hai** bản cài đặt và **chạy cả hai app**, không phải đọc Metabase rồi nhớ lại egret. Mọi điều bên
dưới đều dẫn tới một tệp cụ thể hoặc một phép đo ngày 2026-07-30.

---

## Lỗi chặn: pivot có bucket thời gian thì KHÔNG chạy được

Dựng đúng cái pivot bình thường nhất — region × category ở cạnh, ngày ở trên, gom theo tháng:

```
Error: engine: line 2:164: mismatched input '('. Expecting: ')', ',', '.'
```

**Mọi pivot có đơn vị thời gian đều hỏng.** Không phải lỗi render: truy vấn không bao giờ chạy. Mà
studio **mặc định đặt đơn vị `Month`** ngay khi kéo một cột ngày vào Columns, nên người dùng gặp lỗi
này ngay lần đầu.

### Đã cô lập, không phỏng đoán

Năm spec chạy thật trên backend, mỗi lần đổi một thứ:

| # | spec | kết quả |
|---|---|---|
| A | `GROUP BY` thường + `date_trunc('month', …)` | **200 OK** |
| B | `rollup_groups`, không đơn vị thời gian | **200 OK** |
| C | `rollup_groups` + tháng trên chiều **có** rollup | **502** |
| D | `rollup_groups` + tháng trên chiều **KHÔNG** nằm trong rollup nào | **502** |
| E | `rollup_groups` + `day_of_week` (extract, không phải truncate) | **502** |

**Chính ca D chỉ ra thủ phạm.** Cột bị bucket không nằm trong grouping set nào mà vẫn hỏng — nên lỗi
KHÔNG ở `GROUPING SETS`. Nó nằm ở cột marker sinh cho **mọi** chiều
(`crates/egret-bi/src/chartquery.rs:401-404`):

```rust
for (i, e) in dim_exprs.iter().enumerate() {
    select.push(format!("GROUPING({e}) AS __grp_{i}"));
}
```

Khi có đơn vị, `e` là `date_trunc('month', "order_date")`, nên sinh ra:

```sql
GROUPING(date_trunc('month', "order_date"))
```

**`GROUPING()` nhận tham chiếu cột, không nhận lời gọi hàm.** Parser Trino đọc `date_trunc` như phần
đầu của một tên định danh rồi gặp `(` — đúng như thông báo `Expecting: ')', ',', '.'`. Ca E xác nhận
lỗi không riêng `date_trunc`: mọi dạng biểu thức đều hỏng, nên vấn đề là **biểu thức trong
`GROUPING()`**, không phải phép truncate.

### Cách sửa: bucket trong subquery, group theo alias

`GROUPING()` cần một cột trần, thì đưa cho nó một cột trần. Tính mọi biểu thức chiều ở `SELECT` bên
trong, rồi group và đánh dấu trên alias:

```sql
SELECT "region", "order_date", sum(...) AS "total_revenue",
       GROUPING("region") AS __grp_0, GROUPING("order_date") AS __grp_1
FROM (SELECT "region", date_trunc('month', "order_date") AS "order_date", ... FROM t) AS __b
GROUP BY GROUPING SETS (("region", "order_date"), ("region"), ("order_date"), ())
```

Cách này portable — nó bỏ biểu thức khỏi cả `GROUPING()` **lẫn** `GROUP BY GROUPING SETS`, nên bảo
vệ luôn StarRocks, vốn khó tính hơn Trino ở chỗ này ([[egret-starrocks-vs-trino-sql]]). Đây là thay
đổi backend, nên khác chart 06: **cổng backend nằm trong phạm vi và backend phải roll trước
frontend**.

### Vì sao không test nào bắt được

`chartquery.rs` có test cho cả hai tính năng — bốn test có `rollup_groups` khác rỗng, một test có
đơn vị thời gian thật — và **không test nào có cả hai**. Mọi test rollup đều truyền
`temporal_units: HashMap::new()`. Lỗi nằm đúng ô trống của ma trận 2×2 đó.

Đó là phát hiện quy trình của đơn vị này: *các tính năng được test riêng lẻ, phần giao thì không.*
Task test trong kế hoạch là một **ma trận**, không phải một danh sách.

---

## Thực chất mỗi bên có gì

Đọc từ `pivot/engine.ts` + `pivot/PivotTable.tsx` + `viz/pivot.tsx` (547 dòng) và
`visualizations/PivotTable/*` của Metabase (3.032 dòng) cùng `lib/data_grid.js`.

### Đã ngang bằng — đừng làm lại

Pivot của egret tốt hơn nhiều so với cách gọi "chưa trau chuốt". Tất cả những thứ sau đã có và chạy:

| | |
|---|---|
| vùng rows/columns/values | `FieldZone`, kéo đổi thứ tự |
| nhóm hàng lồng nhau + subtotal | `Totals for X`, `emitChildren` |
| hàng tổng cuối | `kind: "grand"` |
| cột tổng bên phải | `showRowTotals` → nhóm cột `Total` |
| gộp tổng trùng lặp | `condense`, mặc định `true` giống Metabase |
| thu/mở nhóm hàng | nút `+`/`–`, lưu vào `collapsedRows` |
| sắp xếp theo chiều hàng | bấm header, xoay asc/desc |
| định dạng có điều kiện | `buildConditional`, **cả single lẫn range** |
| tiêu đề + định dạng số theo cột | `column_settings`, áp cho header, ô và CSV |
| drill từ ô giá trị | `pivotCellDrill` → chiều hàng + cột |
| header và cột đầu dính (sticky) | `sticky` + phân tầng z-index |

Đo thật: 2 tầng header, sub-header metric, 21 hàng thân, 4 nút thu/mở, dải subtotal, 12px — đúng hết.

### Khoảng trống thật, đều đã đối chiếu code

| Khoảng trống | Metabase | egret | Bằng chứng |
|---|---|---|---|
| **Bucket thời gian** | chạy (card 10 hiện `Q2 2025 … Q4 2026`) | **truy vấn hỏng** | lỗi chặn ở trên |
| **Ảo hoá (virtualisation)** | `react-virtualized` `Grid` + 2 `Collection` + `ScrollSync`; 111 ô trong DOM cho kết quả 48 hàng | mọi ô đều nằm trong DOM | `PivotTable.tsx:15`; đo thật |
| **Kéo giãn cột** | `pivot_table.column_widths`, dnd-kit, có lưu — **card 10 ship kèm độ rộng đã chỉnh** | không có | `settings.ts:188`, settings card 10 |
| **Subtotal theo từng chiều** | `pivot_table.column_show_totals`, theo từng chiều hàng, mặc định bật trừ chiều cuối | một công tắc `showColTotals` toàn cục | `settings.ts:280-322` |
| **Chia cột mặc định thông minh** | phân hoạch theo `distinct-count`: ≤3 chiều → 1 cột + còn lại là hàng; ≥4 → 2 cột | người dùng tự kéo từng trường | `settings.ts:113-134` |

### Chỗ egret đang hơn, và nên giữ

- **Định dạng có điều kiện phong phú hơn.** `isValid` của pivot Metabase đòi `type === "single"` và
  cấm `highlight_row` (`settings.ts:218-240`), thế mà chính card 10 của nó lưu một luật `range` —
  code và dữ liệu mẫu của nó tự mâu thuẫn. egret hỗ trợ cả single lẫn range trên pivot từ trước.
- **Sắp xếp bằng một cú bấm, không phải qua panel.** Metabase giấu chiều sắp xếp trong một widget
  setting theo cột; egret xoay trực tiếp từ header. Giữ cách của egret.

### Cố ý không chép

- **Ngữ nghĩa `GROUPING()` không phải bài toán của Metabase.** Metabase pivot ở phía client từ một
  kết quả phẳng; egret đẩy rollup xuống SQL. Kiến trúc khác nhau, và cách của egret chính là lý do
  pivot của nó chịu được kết quả lớn — bản sửa thuộc về SQL của egret, không phải một bản port.
- **Định dạng field-ref cũ trong `column_split`** (`settings.ts:294-309` mang cả tên cột lẫn field
  ref, kèm migration). egret xưa nay chỉ lưu tên. Không có gì để chép.

---

## Phạm vi

**T1 — lỗi chặn (backend).** Bọc biểu thức chiều vào subquery mỗi khi `rollup_groups` khác rỗng, để
`GROUPING()` và `GROUPING SETS` chỉ nhìn thấy cột trần. Kèm ma trận test còn thiếu: {không rollup,
có rollup} × {không đơn vị, đơn vị truncate, đơn vị extract} × {Trino, StarRocks}.

**T2 — subtotal theo từng chiều.** `pivot.showColTotals` có thêm dạng theo từng chiều hàng, mặc định
bật cho mọi chiều trừ chiều cuối (quy tắc Metabase). Công tắc toàn cục vẫn giữ làm lối tắt
"tất cả/không gì" để không hồi quy.

**T3 — kéo giãn cột, có lưu.** `pivot.columnWidths`, áp cho cột header trái và cột giá trị, kéo từ
một tay cầm. Đây là setting mà chính dashboard mẫu của Metabase đang dùng, nên không phải giả định.

**T4 — ảo hoá, CHỈ khi đo thấy cần.** Chỉ dựng cửa sổ hàng cho phần thân khi pivot vượt ngưỡng.
**Có điều kiện là phải đo trước, không mặc định**: dựng một pivot đủ lớn để thấy đau, ghi lại số
liệu, rồi mới quyết. Chart 05 đã tốn 40 commit viết lại một thứ mà một phép đo có thể khoanh vùng
trước.

**Không nằm trong đơn vị này:** chia cột mặc định thông minh (nó đổi diện mạo pivot đầu tiên của
người dùng, xứng đáng bàn riêng), và mọi thay đổi về sắp xếp hay định dạng có điều kiện.

---

## Kiểm tra thật

1. Region × category × tháng — **đúng ca đang hỏng hôm nay** — trả về và render được.
2. Mọi đơn vị thời gian, gồm cả một đơn vị dạng extract (`day_of_week`).
3. Pivot không có đơn vị giống hệt `main` từng byte.
4. Tắt subtotal cho một chiều, bật cho chiều khác.
5. Kéo giãn một cột header trái và một cột giá trị; tải lại; độ rộng còn nguyên.
6. Thu/mở nhóm vẫn chạy sau khi kéo giãn.
7. Drill từ một ô mang theo cả chiều hàng lẫn chiều cột.
8. Một pivot lớn: ghi số hàng, số node DOM, thời gian vẽ lần đầu — trước và sau T4.
9. Cả hai theme; cả card dashboard lẫn studio.

## Kiểm chứng

Cổng đầy đủ. **Có thay đổi backend nên cổng Rust nằm trong phạm vi và backend phải roll trước
frontend** — chart 02 đã xác lập lý do. Không migration: `pivot.columnWidths` và dạng subtotal theo
chiều đều là bổ sung, và `chartparams.rs` để mở các khoá settings.
