# Chart parity PR 05 — chỗ cho nhãn — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-02-chart-parity-05-label-room.md` là bản chuẩn; mã nguồn và các bước chạy
> lệnh nằm ở đó.

**Mục tiêu:** Chấm dứt việc bốn chart vẽ nhãn không vừa chỗ — sankey có nhãn cột phải chạy ra ngoài
khung, row chart có các mốc giá trị chồng thành một vệt, gauge đặt mười một con số quanh một cung
nhỏ, và pie có nhãn lát đè lên nhau.

**Kiến trúc:** Cả bốn đều là quyết định theo kích thước, nên đều gắn vào seam builder-biết-kích-thước
mà PR 01 tạo ra. Ba trong bốn dùng lại máy móc đã có (`axisLabelFit`, `measureText`, `sliceLabel`);
riêng sankey cần phần dành chỗ mới trong builder của nó.

---

## Phạm vi: kế hoạch này làm BỐN trong sáu mục của workstream 5

Hai mục còn lại **hoãn sang PR 05b** vì không phải chuyện fitting nhãn ECharts:

- **Funnel** là component React (`FunnelChart.tsx`), không phải ECharts builder — bài toán flex layout.
- **Pivot** là bảng bị co lại thay vì cuộn. Cũng React, cũng không phải builder.

Chúng không dùng chung mã với bốn mục ở đây và sẽ nhân đôi bán kính ảnh hưởng của PR mà không được
lợi gì. Cả hai vẫn được **đo trong tài liệu này** để PR 05b khởi hành từ bằng chứng chứ không từ lời
văn của spec.

## Số đo trước khi sửa

Card egret là svg **166×184**; của Metabase là **311×223**. Đọc từ DOM.

### 1. Sankey — nhãn cột phải ra khỏi khung

| | egret | Metabase |
|---|---|---|
| x của nhãn cột phải | **158** trên svg 166px | 273 trên svg 343px |
| bề rộng nhãn | 39–63px | 34–56px |
| mép phải chạm tới | **197–221px** | 307–329px |

Nhãn của egret bắt đầu cách mép phải 8px rồi chạy quá mép 30–55px. Người dùng chỉ thấy một mẩu — đó
chính là thứ spec mô tả là "cắt còn một hai ký tự"; chúng **không bị cắt**, chúng **được vẽ ra ngoài
khung**. Metabase dành sẵn chỗ nên nhãn dài nhất (`Doohickey`, 56px) kết thúc cách mép 14px.

`options/sankey.ts` đã ghi sẵn chẩn đoán ở dòng ~129–142: `label.position` mặc định `"right"` và
không hề bị ghi đè, nên cả cột cuối cũng vẽ sang phải vào khoảng chừa `right: "8%"`; không chỗ nào đặt
`label.width` hay `label.overflow`.

### 2. Row chart — trục giá trị chưa từng được fit

Trục dưới của egret, cả bốn mốc trên cùng một hàng `y=136`:

| mốc | x | rộng | khoảng |
|---|---|---|---|
| `100K` | 101 | 31 | 101–132 |
| `200K` | 109 | 32 | 109–141 |
| `300K` | 117 | 33 | 117–150 |
| `400K` | 125 | 33 | 125–158 |

Bước 8px cho nhãn rộng 31px: mỗi nhãn đè hàng xóm 23px. Row chart của Metabase ở 311px vẽ **một** mốc.

**Nguyên nhân, tìm thấy trong mã:** `options/cartesian.ts:1027` là
`const ordinalX = !horizontal && !usesXScale(cfg);` và ladder ở `:1076` chỉ chạy khi `ordinalX`. Row
chart đặt `horizontal`, nên `ordinalX` sai và **ladder của PR 02 không bao giờ chạy cho nó**. Vả lại
trục dưới của nó là trục *giá trị*, vốn cần cách chữa khác: ít mốc hơn, không phải xoay nhãn.

Lỗi thứ hai cùng chart: tiêu đề trục xoay `category` ở `x=66, y=46, h=54`, còn nhãn `Gadget` chiếm
`x=56–100, y=51`. Chúng đè nhau. Metabase đặt `CATEGORY` ở `x=1`, ngoài vùng nhãn.

### 3. Gauge — mười một con số quanh cung nhỏ

egret vẽ `0`, `76,612.39`, `153,224.78` … `766,123.9` — **mười một nhãn**, vài cái đè nhau:
`229,837.17` chiếm x 65–122 và `306,449.56` chiếm x 73–135, cùng `y=69`.

`options/gauge.ts:97-98` đã nêu sẵn nguyên nhân và cần gạt: builder không đặt `splitNumber`, nên mặc
định 10 khoảng của ECharts cho mười một con số ở mọi kích thước.

### 4. Pie — nhãn lát đè nhau

| nhãn | khoảng x | khoảng y |
|---|---|---|
| `Doohickey` | 34–106 | 38–55 |
| `Widget` | 76–126 | 42–59 |

Đè nhau cả hai chiều và cả hai đều được vẽ. Đây là chỗ bất ngờ: `options/pie.ts` **đã** chạy
`fitSliceLabel` (bản port thuật toán dây cung của chính Metabase) **và** đặt
`labelLayout: { hideOverlap: true }` làm chốt chặn ở `:1544`. Không cái nào loại hai nhãn này.
**Phải xác định vì sao trước khi sửa bất cứ thứ gì.**

Metabase ở 208×214 không vẽ nhãn lát nào — chỉ `1.5M` / `Total` ở giữa — nên "ẩn đi" là đáp án hợp
lệ ở kích thước này.

### Hoãn sang PR 05b, nhưng đã đo

**Funnel** (card 184×234), các hộp lá có `overflow:hidden; text-overflow:ellipsis`:

| phần tử | hộp |
|---|---|
| `Visited` | 104×33 |
| `Added to cart` | **19×33** |
| `42.86 %` | **4×20** |
| `56,280` | **4×20** |

Bước đầu chiếm 104px, ba bước sau mỗi bước 19px cho tên và 4px cho số. Card funnel của Metabase trên
dashboard này ghi *"Which fields do you want to use?"* — chưa cấu hình, nên **không có bản tham chiếu**.

**Pivot** (card 184×234). Bảng chỉ có **2 hàng**: header (`order_month` / `Total`) và
`Grand totals 7,763.7 7,763.7`. `scrollableDescendants: 0` — không gì trong card cuộn được. Card pivot
của Metabase ghi *"Pivot tables are only supported for questions built in the query builder"*, mà card
của harness là SQL thuần, nên **không thể lấy tham chiếu Metabase cho pivot**. Đích của nó phải tự
lập luận: cuộn trong card thay vì rụng phần thân.

## Ngoài phạm vi

- **Tỉ lệ vùng vẽ.** Bar và box plot của egret dành 29% chiều cao card cho vùng vẽ trong khi line
  chart dành 58%, vì nhãn xoay 90° chiếm trọn chiều dài. Của Metabase là 59% trên card cao hơn. Việc
  này đáng có điều tra riêng và không nằm trong PR này — sửa nó là đụng lại ladder xoay của PR 02.

## Các task

| # | nội dung |
|---|---|
| 1 | Sankey dành chỗ cho nhãn nó vẽ |
| 2 | Trục giá trị của row chart được fit; tiêu đề trục không đè nhãn |
| 3 | Số vạch của gauge suy ra từ bán kính đo được |
| 4 | Nhãn pie đè nhau — **chẩn đoán trước, sửa sau** |
| 5 | Đối chiếu live + mở PR (KHÔNG merge) |

**Task 4 cố ý để mở.** Đây là mục duy nhất chưa xác định được nguyên nhân, vì hai cơ chế lẽ ra đã ngăn
được nó đều đang có mặt. Viết sẵn bản sửa vào kế hoạch khi chưa biết cơ chế nào hỏng là đoán mò, và
chương trình này đã trả giá cho việc đó nhiều lần.

## Tiêu chí nghiệm thu (Task 5)

1. **Không nhãn sankey nào vượt mép phải svg.** Hôm nay chúng chạm 197–221px trên svg 166px.
2. **Không hai mốc giá trị nào của row chart đè nhau.** Hôm nay cả bốn đè, mỗi cặp 23px.
3. **Tiêu đề trục row chart không đè nhãn phân loại.**
4. **Gauge vẽ tối đa bốn vạch ở 166×184**, không cái nào đè nhau. Hôm nay mười một.
5. **Không hai nhãn lát pie nào đè nhau.**
6. **Không thứ gì đang vừa lại thành không vừa** — đối chiếu bar, line, area, combo, scatter,
   waterfall, box plot với bảng số của PR 02 và PR 03.

Đây là bài kiểm định thật. Mọi PR trong chương trình này đều từng ship một lỗi vượt qua suite của
chính nó và chỉ bị bắt ở bước này. Mô phỏng không phải bằng chứng.
