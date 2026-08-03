# Trục danh nghĩa vẽ theo thứ tự ổn định — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại `docs/en/plans/2026-08-03-nominal-axis-order.md`
> là bản chuẩn; mã nguồn và các bước chạy lệnh nằm ở đó.

**Mục tiêu:** Chấm dứt việc bar chart và row chart vẽ danh mục theo thứ tự mà engine tình cờ trả về.

**Kiến trúc:** Suy ra `orderby` bên trong `buildSpec` của cartesian — đúng cách smartscalar đã làm — để
**dữ liệu** xác định; và sắp xếp trục danh nghĩa trong `sortCategories` để những chart **đã lưu** với
`orderby: []` cũng được sửa. Hai lớp, mỗi lớp một lý do riêng.

---

## Lỗi

PR 07 đo trên `POST /charts/{id}/data`, mười lần gọi cùng một chart:

```
7 × ['Gizmo', 'Widget', 'Gadget', 'Doohickey']
3 × ['Widget', 'Gadget', 'Doohickey', 'Gizmo']
```

**Ba lần tải trang liên tiếp đều ra thứ tự chiếm 70%**, nên phép so hai lần tải đã kết luận là xác định
và lỗi sống sót qua PR 02–06. Mọi phép đo **vị trí** danh mục trên bar/row — mà chương trình này đã đo
vài lần — đều đo một giá trị đổi giữa các lần tải trang.

Spec của mọi chart có nhóm đều mang `orderby: []`. Chín trong mười hai được cứu bởi phép sắp xếp phía
client của riêng chúng; **bar và row thì không**, vì `cartesian.ts:526` cố ý để trục danh nghĩa theo
thứ tự gặp đầu tiên.

## Metabase thật sự để nguyên — nên chỉ sắp xếp phía client là chưa đủ

`model/dataset.ts:849` của Metabase:

```ts
export const sortDataset = (dataset, xAxisScale, showWarning) => {
  if (xAxisScale === "ordinal") {
    return dataset;          // ← trục danh nghĩa: giữ nguyên
  }
  …
};
```

Metabase không cần sắp xếp vì **truy vấn của nó mang sẵn thứ tự**: bước sort của câu hỏi MBQL, hoặc
`ORDER BY` do tác giả câu hỏi native viết. Chính câu hỏi `MB By Category` của harness dùng
`ORDER BY revenue DESC`.

`buildSpec` của egret hard-code `orderby: []` ở mọi nơi trừ smartscalar. **Đó mới là khoảng trống
thật**, và là lý do bộ render bị ôm một bài toán lẽ ra truy vấn phải giải.

## Hai lớp, hai lý do

**1. Suy `orderby` trong `buildSpec` của cartesian.** Đây là bản sửa trung thực: nó làm **dữ liệu** xác
định, nên tooltip, drill, export và mọi row limit đều nhất quán, chứ không chỉ trục được vẽ.
`simple.tsx:335` là tiền lệ sẵn có — smartscalar tự suy từ config của nó, có lý do ghi rõ, và không cần
thêm trường nào vào `SharedQueryState`.

**2. Sắp xếp trục danh nghĩa trong `sortCategories`.** Lớp 1 chỉ giúp những chart lưu **sau đó**: đường
dashboard đọc spec **đã lưu** nguyên văn (PR 05b xác lập điều này), nên mọi chart đã lưu vẫn giữ
`orderby: []` cho tới khi có người lưu lại. Không có lớp 2 thì bản sửa gần như không tới được ai.

`pivot/engine.ts:191-193` là tiền lệ, và lập luận của nó áp dụng nguyên văn: *"Non-deterministic output
that changes between runs is worse than any particular order, and Metabase sorts both axes too."*

**Lớp 2 là một sai lệch có chủ đích so với Metabase**, và PR phải nói rõ: Metabase để nguyên được vì
truy vấn của nó có thứ tự; spec đã lưu của egret thì không.

## Quyết định còn để mở: thứ tự nào?

**Tăng dần theo giá trị dimension** là thứ egret đã chọn hai lần — `sortTree` (`cmpValues` tăng dần) và
smartscalar (`desc: false`). Nó trung tính: không đoán người dùng muốn xếp hạng cái gì.

**Giảm dần theo metric** là thứ câu hỏi Metabase của harness dùng, là thứ `pie.ts` và `sankey.ts` làm,
và là thứ đọc hợp lý nhất trên bar chart.

**Phải xác lập query builder của Metabase mặc định sinh ra gì** cho một câu hỏi "summarise by category"
trước khi chọn — đó là bằng chứng quyết định mà cả kế hoạch này lẫn PR 07 đều chưa có.

- Nếu mặc định của Metabase không mang thứ tự, chọn **tăng dần theo dimension**: khớp hai tiền lệ của
  chính egret và là thứ ít gây bất ngờ nhất khi không có ý định nào từ người dùng.
- Nếu nó sắp theo aggregation giảm dần, chọn theo.

**Nói rõ bạn chọn gì và bằng chứng nào quyết định.** Không chọn theo sở thích.

## Ngoài phạm vi

- **Control sắp xếp trong studio.** Lý do truy vấn không có `orderby` là chẳng có gì cho người dùng yêu
  cầu nó. Đó là khoảng trống thật và là tính năng của Chart Studio, không phải PR này.
- **Đổi mười một chart có nhóm còn lại.** Phép sắp xếp phía client của chúng đang chạy tốt. Thêm
  `orderby` vào mọi `buildSpec` sẽ đúng hơn và đáng cân nhắc về sau, nhưng nó đổi việc dòng nào sống sót
  qua row limit trên mười một loại chart cùng lúc và cần đối chiếu riêng.

## Các task

| # | nội dung |
|---|---|
| 1 | Truy vấn mang sẵn thứ tự — `buildSpec` của cartesian |
| 2 | Chart đã lưu cũng được sửa — `sortCategories` |
| 3 | Đối chiếu live + mở PR (KHÔNG merge) |

Lưu ý hợp đồng với harness: `scripts/harness/params.ts` sinh params qua **chính** `buildSpec` này, và
`charts.test.ts` khẳng định 19 config round-trip. Thay đổi của bạn sẽ làm seed sinh ra khác đi — **đó là
đúng**, docs của PR 07 đã dự báo. Cập nhật kỳ vọng đó và nói rõ.

## Tiêu chí nghiệm thu (Task 3)

**Chứng minh tính xác định đúng cách PR 07 đã bác bỏ nó.** Hai lần tải trang là không đủ — đó chính là
thứ đã bỏ sót lỗi này suốt sáu PR.

- **Mười** lần gọi `POST /charts/{id}/data` cho `MB · Bar`: thứ tự danh mục phải giống nhau mọi lần.
  Báo cáo cả mười.
- **Sáu** lần tải dashboard, đọc thứ tự danh mục được vẽ từ DOM: giống nhau mọi lần. Báo cáo cả sáu.
- Làm tương tự cho `MB · Row` và box plot.
- Đối chiếu Metabase ở khổ card tương đương. Nếu bạn chọn thứ tự mà câu hỏi harness của Metabase không
  dùng thì hai bên sẽ khác — **đó là sai lệch có tài liệu, không phải thất bại**, và PR phải nêu kèm lý do.
- **Không thứ gì khác dịch chuyển** — kiểm lại các chart PR 02–06 đã sửa.

**Kiểm hash bundle** trước khi tin bất kỳ phép đo nào.

## Điểm yếu đã biết

Lớp 1 đổi thứ `scripts/harness/params.ts` sinh ra, nên seed của PR 07 sẽ tạo spec khác với các chart
đang lưu trong `demo`. Điều đó đúng và đã lường trước, nhưng nghĩa là harness và `demo` sẽ lệch nhau cho
tới khi chạy lại seed — phải nói trong PR thay vì để người sau tự phát hiện.
