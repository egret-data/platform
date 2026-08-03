# Chart parity PR 05b — funnel và pivot — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-03-chart-parity-05b-funnel-pivot.md` là bản chuẩn; mã nguồn và các bước chạy
> lệnh nằm ở đó.

**Mục tiêu:** Làm funnel đọc được trên card hẹp, và tìm ra vì sao pivot chỉ hiện hai hàng trên
dashboard trong khi ở editor nó hiện mười bốn hàng.

**Kiến trúc:** Hai component React không liên quan nhau. Funnel là bài toán port typography — Metabase
tự thu nhỏ chữ trên card nhỏ còn egret hard-code. Pivot **không phải** lỗi layout, và kế hoạch này
không giả định nó là gì.

---

## Đính chính spec

Workstream 5 nói pivot *"render đúng trong editor nhưng co lại còn header + dòng tổng trên card
dashboard. Cần cuộn trong card thay vì rụng phần thân."*

**Đo thì thấy sai cả hai vế.** Nó không rụng phần thân nó đang có, và cho nó cuộn cũng chẳng sửa được gì.

| | editor | card dashboard |
|---|---|---|
| card | — | 182×232 |
| kích thước bảng | **846×406** | **269×58** |
| số hàng | **14** (header + 12 tháng + tổng) | **2** |
| hàng header | `order_month \| Doohickey \| Gadget \| Gizmo \| Widget \| Total` | `order_month \| ⟨rỗng⟩ \| Total` |
| hàng thân đầu | `2026-01-01 \| 1,939.99 \| 2,717.2 \| 3,439.68 \| 2,997.83 \| 11,094.7` | `Grand totals \| 7,763.7 \| 7,763.7` |
| scroller | `max-h-[60vh] overflow-auto` | `min-h-0 flex-1 overflow-auto px-2 pb-2` |
| scrollHeight / clientHeight | 406 / 406 | **192 / 192** |

Ba kết luận:

1. **Không có gì bị cắt.** `scrollHeight === clientHeight` trên dashboard, và bảng chỉ cao 58px trong
   scroller 192px. Không có phần thân nào bị giấu để mà cuộn tới.
2. **Chiều cột biến mất.** Header trên dashboard có một ô rỗng ở chỗ editor có bốn cột danh mục.
3. **Con số khác nhau.** `7,763.7` so với tổng mười hai tháng khoảng 140.000 ở editor. Đây không phải
   cùng một tập kết quả.

Vậy đường dashboard đang đưa cho pivot **dữ liệu khác**, và cái "co lại" nhìn thấy chính là thứ
`PivotTable` render đúng cho dữ liệu đó. Lỗi nằm phía trên bảng.

## Số đo phía funnel

Card funnel của egret là 184×234. Các hộp lá, đều có `overflow:hidden; text-overflow:ellipsis`:

| phần tử | hộp |
|---|---|
| `Visited` (tên bước 1) | 104×33 |
| `131,320` (giá trị bước 1) | 93×28 |
| `sessions` (tên metric) | 93×19 |
| `Added to cart` | **19×33** |
| `42.86 %` | **4×20** |
| `56,280` | **4×20** |

Bước đầu chiếm 104px trong hộp nội dung 152px, ba bước còn lại chia phần thừa.

**Cấu trúc của egret vốn đã khớp Metabase.** `FunnelNormal.styled.tsx` cho bước đầu
`min-width: unset; width: unset` và mọi bước sau `width: 100%; min-width: 20px` — đúng bằng `shrink-0`
và `min-w-[20px] flex-1` của egret. Cột đầu không bị chặn **không phải** là lỗi.

**Thứ egret thiếu là typography co giãn của Metabase.** `FunnelNormal.tsx:106-108`:

```ts
const isNarrow = Boolean(gridSize && gridSize.width < 7);
const isShort  = Boolean(gridSize && gridSize.height <= 5);
const isSmall  = isShort || isNarrow;
```

| phần tử | thường | hẹp |
|---|---|---|
| `Head` (tên bước) | `1em` | `0.85em` |
| `FunnelStart > Title` (số lớn) | `1.72em` | `1.72em × 0.75` = **1.29em** |
| `FunnelStart > Subtitle` (tên metric) | 1.18em | `1.72em × 0.5` = **0.86em** |
| `Info` (tỉ lệ + giá trị mỗi bước) | `1.15em` | `0.85em` |
| `Info > Subtitle` | `0.6875em` | `0.875em` |
| padding gốc (`isSmall`) | `1rem` | `0.5rem` |

egret hard-code đúng cột bên trái của bảng đó: `p-4`, `text-[1.72em]`, `text-[0.6875em]`, `text-[1.15em]`.

**Ngưỡng của Metabase tính bằng ô LƯỚI dashboard, không phải pixel**, mà funnel của egret hiện không
nhận kích thước nào cả. Quy đổi ngưỡng là việc của Task 2 và phải **đo**, không được đoán.

## Ngoài phạm vi

- Port hành vi `funnel.type: "bar"` — `StepBars` đã có sẵn.
- Làm `PivotTable` nhỏ hoặc dày hơn — chưa có chẩn đoán thì chưa có cơ sở đổi cách render của bảng.

## Các task

| # | nội dung |
|---|---|
| 1 | **Lấy bản tham chiếu Metabase cho funnel** — cấu hình card (`funnel.dimension`/`funnel.metric`) rồi đo, kể cả đo xem `isNarrow` lật ở bề rộng pixel nào |
| 2 | Funnel thu nhỏ chữ trên card nhỏ; cấp kích thước cho component qua `ResizeObserver` |
| 3 | **CHỈ CHẨN ĐOÁN**: vì sao pivot nhận dữ liệu khác trên dashboard. Báo cáo trước khi đổi bất cứ thứ gì |
| 4 | Sửa thứ Task 3 tìm ra |
| 5 | Đối chiếu live + mở PR (KHÔNG merge) |

**Cảnh giác với harness.** Đã ba lần chart trên dashboard này trống vì tôi đặt sai khoá settings, và
PR 03 từng vô hiệu suốt một task vì harness lưu `temporal_units: {}`. **Đối chiếu params lưu ở card
dashboard với params của chính chart trước khi kết luận egret có lỗi** — nếu card mang settings khác,
đó là lỗi harness, và Task 3 kết thúc bằng sửa harness rồi đo lại, không phải bằng sửa mã.

Nếu Task 3 kết luận lỗi nằm ở harness chứ không ở egret, **Task 4 trở thành: sửa harness, đo lại, và
ghi vào PR rằng mục pivot của spec là hiện tượng do harness gây ra.** Đó là kết quả hợp lệ và phải
được báo cáo thẳng, không được tô vẽ thành một bản sửa mã.

## Tiêu chí nghiệm thu (Task 5)

1. **Mọi hộp tên bước của funnel rộng hơn 40px** ở card 184px. Hôm nay bước 2–4 được 19px.
2. **Mọi hộp giá trị và phần trăm rộng hơn 40px.** Hôm nay được 4px.
3. **Bước đầu vẫn hiện giá trị và tên metric** — thu nhỏ không được biến chúng thành dấu ba chấm.
4. **Pivot hiện chiều cột và các hàng thân** ở card 182×232, hoặc — nếu Task 3 kết luận là do harness —
   pivot render giống hệt editor sau khi sửa harness.
5. **Không thứ gì khác trên dashboard đổi** — kiểm lại bốn chart PR 05 đã sửa.

## Điểm yếu đã biết

Test của Task 2 không dùng được `getBoundingClientRect()` dưới jsdom (jsdom không layout), và kế hoạch
**không** chốt cách đo. Nêu ra thay vì lấp liếm: nếu không tìm được assertion trung thực ở mức unit,
hãy nói vậy và để bước đối chiếu trong trình duyệt ở Task 5 gánh tiêu chí đó, chứ đừng viết một test
xanh vì lý do sai. PR 05 đã bắt được đúng lỗi này trong `sizeAware.test.ts`, nơi một ca chỉ xanh vì
measurer của jsdom trả về 0.
