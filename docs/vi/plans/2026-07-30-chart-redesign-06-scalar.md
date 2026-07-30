# Thiết kế lại chart 06 — Scalar: kế hoạch triển khai

**Spec:** `docs/vi/specs/2026-07-30-chart-redesign-06-scalar.md`
**Quy tắc đối chiếu Metabase:** `docs/vi/plans/2026-07-30-chart-redesign-metabase-parity-rules.md`
**Ngày:** 2026-07-30
**Bản tiếng Anh (bản chuẩn):** `docs/en/plans/2026-07-30-chart-redesign-06-scalar.md`

Chín task. Mọi đường dẫn tệp và mọi khẳng định về code hiện có bên dưới đều được **đọc** ngày
2026-07-30, không dựa trí nhớ — đây là điều chỉnh thường trực của chương trình sau khi kế hoạch chart
01 sai ở 6/9 task.

**Backend: ngoài phạm vi, đã kiểm chứng.** `crates/egret-bi/src/chartparams.rs` chỉ kiểm tra *hình
dạng* (version, `viz_type` theo `VIZ_TYPES`, phần spec), và chính header của nó viết: *"Settings keys
are equally deliberately open — A2 chose flat settings so keys an older build doesn't know still
survive a round-trip."* Ba khoá mới không cần đổi gì phía server và không có migration, nên cũng
không có ràng buộc thứ tự deploy.

---

## Task 0 — Đo nền, trước khi viết code

Quy tắc 4: đo host box ở cả hai app **trước**. Vòng của chart 05 mỏng hơn 32% dù tỉ lệ đã đúng, chỉ
vì không ai làm bước này.

- Card 39 của Metabase ("Total orders this quarter") ở 1440×920; scalar tương đương của egret trên
  bộ Demo Sales, cùng viewport, cả dạng card dashboard lẫn trong studio.
- Ghi lại: host box, cỡ chữ render ra bằng px, màu giá trị, `getComputedStyle(...).fontFamily`.
- **Vẽ màu ra canvas 1×1 rồi đọc lại** — token của egret compute ra `oklch(...)` và regex rgb ngây
  thơ sẽ báo số vô nghĩa.

Đầu ra: một bảng trong PR. Nếu host box của egret nhỏ hơn thì đó là lỗi đầu tiên, và nó **không**
phải lỗi của scalar — chart 05 từng phát hiện `h-[60vh]` vứt đi 231px cho **mọi** loại chart. Kiểm
tra xem điều đó còn đúng không trước khi chỉnh cỡ chữ theo một container vốn đã sai.

## Task 1 — Kiểu dữ liệu và mối nối renderer

`frontend/src/api/charts.ts`:

```ts
export interface ScalarSegment { min: number | null; max: number | null; color: string; label?: string }
export interface ScalarConfig {
  metric?: string;
  segments?: ScalarSegment[];
  label?: string;
  sublabel?: string;
}
```

`ChartRenderer.tsx:353` — nhánh scalar đang liệt kê tay mỗi `metric`. **Chuyển sang spread**, theo
đúng comment đã nằm sẵn phía trên năm nhánh được sửa ở `01e91ab9`, với `columnSettings` và
`metricTypes` đặt **sau** phần spread để settings đã lưu không che được của shell.

Task này là chỗ cả đơn vị có thể hỏng trong im lặng: pie từng ship bốn setting đã code xong, đã hiện
trong panel mà app không với tới được, vì test gọi thẳng builder và đi vòng qua đúng dòng này. **Test
mối nối nằm ở task này, không để cuối** — render qua `ChartRenderer` với `vizType="scalar"` và khẳng
định màu `segments` đã lưu đi tới được DOM.

## Task 2 — Đo chữ

Tệp mới `frontend/src/components/chart/measure-text.ts`, port từ
`frontend/src/metabase/utils/measure-text.ts` của Metabase:

- `<canvas>` ở cấp module, **gắn vào `document.body`**, `display:none`, đặt `canvas.style.fontSize`
  theo `getComputedStyle(document.documentElement).fontSize`. Đây không phải chi tiết phụ —
  `context.font` với `rem` quy chiếu theo font-size của chính phần tử, nên canvas không gắn DOM sẽ đo
  theo 16px và mọi phép tính vừa-khung sai ở mọi nơi người dùng đổi cỡ chữ gốc.
- Chiều cao là `actualBoundingBoxAscent + actualBoundingBoxDescent` — chiều cao glyph, không phải
  line-height.
- `findSize({ text, targetHeight, targetWidth, unit, fontFamily, fontWeight, step, min, max })`, hạ
  dần từ `max` khi còn tràn một trong hai chiều. Chép nguyên.

**jsdom trả về số 0 từ `measureText`.** Test khẳng định một *cỡ chữ được chọn* dưới jsdom không chứng
minh gì — nó chỉ khẳng định chỗ vòng lặp chạm đáy. Hãy unit-test `findSize` với một bộ đo **tiêm
vào** có số liệu biết trước (kiểu `calcChordLength(10, π) === 20`: giá trị biết độc lập, không phải
tính lại chính công thức đang test), và để mọi khẳng định về số liệu thật cho phần kiểm tra trình
duyệt ở Task 9.

## Task 3 — Tự chỉnh cỡ trong `BigNumber`

- `useElementSize` (đã có sẵn, `components/chart/useElementSize.ts`) trên phần tử chứa giá trị.
- Cỡ tối đa theo quy tắc Metabase, hằng số chép nguyên:
  `(cardColWidth / GRID_COLS) × (MAX_SIZE_LARGE − MAX_SIZE_SMALL) + MAX_SIZE_SMALL`,
  `MAX_SIZE_SMALL = 2.2`, `MAX_SIZE_LARGE = 7`, và `NARROW_CARD_MAX_SIZE = 2.5` khi card hẹp hơn
  `NARROW_CARD_WIDTH = 240`. `GRID_COLS` của egret là 24, đúng bằng `GRID_WIDTH` của Metabase, nên tỉ
  lệ chuyển nguyên si.
- Min 1rem, bước 0.2rem, weight 700.
- **Font family truyền cho bộ đo phải đúng font DOM đang render.** Chart 05 vẽ mọi biểu đồ bằng
  `sans-serif` trong khi UI là Inter. Hãy đọc từ chính phần tử thay vì đặt hằng số, và kiểm chứng ở
  Task 9.
- Padding: Metabase trừ `PADDING = 32` hai lần ở chiều cao và một lần ở chiều rộng. Chép theo, và ghi
  trong PR rằng đó là lý do con số không bao giờ chạm mép card.

Giữ nguyên `tabular-nums`, `leading-none`, `role="img"` và `aria-label` đơn nhất y như cũ.

## Task 4 — Rút gọn theo bề rộng và tooltip giá trị đầy đủ

`compactifyValue(value, width, format)` mới, đặt cạnh `format.ts`, chép nguyên:
`COMPACT_MIN_LENGTH = 6`, `COMPACT_MAX_WIDTH = 250`, `COMPACT_WIDTH_PER_DIGIT = 25`;
rút gọn khi `len > 6 && (width < 250 || width < len × 25)`.

- `format.compact === true` đặt tay vẫn thắng — cơ chế này chỉ quyết trường hợp chưa ai quyết.
- Trả về cả hai giá trị; chỉ hiện giá trị đầy đủ trong tooltip **khi nó khác** cái đang vẽ.
- Tooltip phải tới được bằng bàn phím, không chỉ bằng rê chuột. Việc con số chính xác chỉ cách một
  lần rê chuột chính là thứ khiến rút gọn an toàn; nếu chỉ dùng được bằng chuột thì nó không an toàn.

Thứ tự quan trọng: format → rút gọn → đo → chỉnh cỡ. Chỉnh cỡ theo chuỗi chưa rút gọn là chọn cỡ chữ
cho một đoạn text không có trên màn hình.

## Task 5 — Editor dải dùng chung

Tách `frontend/src/components/chart/settings/SegmentRangesSection.tsx` ra từ
`GaugeSegmentsSection.tsx`, tham số hoá theo bốn khác biệt spec đã lập bảng: cận nullable, bật/tắt
đẩy-liền-kề, số dòng tối thiểu (1 với 0), và các dòng dự phòng khi chưa lưu gì.

`GaugeSegmentsSection` trở thành lớp bọc mỏng truyền bốn giá trị của gauge. **Kết quả render và các
test hiện có của gauge không được đổi** — đó là điều kiện nghiệm thu của task này, và là lý do dùng
lớp bọc thay vì viết lại một chart đang chạy thật.

Lớp bọc của scalar: không đẩy-liền-kề, không sắp xếp, `minRows: 0`, không có dòng dự phòng, và ô cận
để trống phải cho ra `null` chứ không phải `0`. `Number("")` bằng `0` — một dải sửa thành trống không
được lặng lẽ biến thành "từ 0".

Gắn nó thành control `display` trên descriptor scalar trong `viz/simple.tsx`, cạnh `pieDisplay` và
`gaugeDisplay`. `fromParams` chuyển tiếp mọi khoá dưới tiền tố nên không cần đổi codec.

## Task 6 — Màu theo dải và tooltip chú giải

`getSegmentColor(value, segments)` — chép nguyên ngữ nghĩa: `parseFloat`, `find` (**khớp đầu tiên
thắng**, nên thứ tự người dùng nhập là ngữ nghĩa và không có gì được sắp xếp), thiếu `min` là −∞,
thiếu `max` là +∞, không khớp hoặc không có dải nào thì lùi về token foreground. Dải có cả hai cận
null là không hợp lệ và bị loại, đúng theo `segmentIsValid(s, { allowOpenEnded: true })`.

Tooltip chú giải liệt kê từng dải theo ô màu / khoảng / nhãn, khoảng định dạng `min - max`, `≥ min`,
`≤ max` — chép từ `formatSegmentRange`.

**Không chép:** nền tooltip màu tối đóng cứng của Metabase. egret dùng token, nếu không nó biến mất ở
dark mode (quy tắc 2).

## Task 7 — Bấm để drill, và tín hiệu khi rê chuột

- Truyền `onValueClick` vào nhánh scalar — nó đã tới `ChartRenderer` và đã được chuyển cho pie.
- Scalar không có chiều nào, nên ngữ cảnh drill rỗng và `drillFilters` không sinh filter nào: "cho
  tôi xem các bản ghi đằng sau con số này".
- Đổi màu thương hiệu khi rê chuột **chỉ khi bấm được**, và về mặt ngữ nghĩa là một `<button>` thật để
  tới được bằng bàn phím — Metabase dùng `Stack` với `onClick`, đó là tín hiệu chỉ dành cho chuột và
  không đáng chép.
- Kiểm tra ở **mối nối**, không phải ở chart: chart 05 phát hiện `PreviewPane` chưa bao giờ truyền
  `onValueClick`, nên chưa từng có biểu đồ ECharts nào bấm được trong editor. Xác nhận scalar bấm
  được ở **cả** studio lẫn dashboard.

## Task 8 — Label, sublabel, và bỏ tên metric luôn-hiện

- Xoá `{metric && <div …>{metric}</div>}`.
- Render `label` (14px) và `sublabel` (12px, muted) khi được đặt, cả hai căn giữa dưới giá trị, và cả
  hai **phải được trừ vào** chiều cao đưa cho bộ tự-chỉnh-cỡ — nếu không, một con số có label sẽ tràn
  khỏi card.
- Thêm hai control text trên descriptor scalar.

Cái giá được ghi nhận, phải xuất hiện trong PR: scalar đã lưu sẽ mất phần tên cột in ra. Trên
dashboard không mất gì, vì `DashboardCardView.tsx:55` đã render header rồi.

## Task 9 — Quét tương phản, kiểm tra thật, bằng chứng

**Quét tương phản**, cả hai theme, ở cỡ chữ **nhỏ nhất** (1rem đậm — dưới ranh giới chữ-lớn 24px của
WCAG, nên chuẩn là 4.5:1 chứ không phải 3:1): mọi màu trong `SERIES_PALETTE` dùng làm nét chữ của dải
trên nền card. Báo cáo số liệu, gồm cả các ca không đạt; giới hạn palette của chart 05 đã được ghi
nhận thay vì giấu đi và đó là chuẩn mực. Ở đâu màu người dùng chọn không đạt thì editor phải nói ra —
không được lặng lẽ render một con số tiêu đề không đọc nổi.

**Kiểm tra thật trên kind**, trong một **trang mới** với hash bundle xác nhận từ `script[src]` — một
trang mở xuyên qua lần redeploy vẫn giữ module graph cũ và sinh ra kết quả ma. Chạy đủ mười lăm ca
đánh số trong spec.

Xác nhận menu xuất của scalar có CSV và **không** có PNG, và rằng lý do là container không chứa
`<svg>` — bộ nhớ ghi lại một lần xuất PNG tóm nhầm icon UI 16px, nên hãy kiểm chứng sự vắng mặt chứ
đừng mặc định.

**Mọi test mới đều kiểm bằng đột biến**: phá thứ nó canh, chạy lại, xác nhận fail, khôi phục, ghi
phép đột biến vào PR. Chart 05 từng ship một test hồi quy sống sót qua chính đột biến của nó vì
fixture tình cờ cho kết quả giống nhau ở cả hai nhánh code.

---

## Thứ tự và xếp chồng

1 → 2 → 3 → 4 tuần tự (mỗi cái dùng kết quả cái trước). 5 → 6 chỉ phụ thuộc 1. 7 và 8 chỉ phụ thuộc
1. 9 sau cùng.

Một PR duy nhất, theo đúng chương trình. Nhánh cắt từ `main` tại `01e91ab9`.

## Cổng kiểm chứng

Frontend lint, typecheck, unit test, build; deploy lên kind; kiểm tra thật; ảnh trước/sau ở cả hai cỡ
card và cả hai theme. Chạy toàn bộ suite ở máy local — CI trên GitHub đã hết quota. Không có migration
nên cổng backend nằm ngoài phạm vi.

Mở PR rồi **dừng lại** chờ review; không merge.
