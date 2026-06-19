# Phase 3c-3.3 — Định dạng giá trị + Conditional Formatting + Settings Sidebar

> Trạng thái: thiết kế đã duyệt. Bản tham chiếu (Tiếng Việt). Bản gốc chuẩn (Tiếng Anh):
> `docs/en/specs/2026-06-19-phase-3c-3-3-formatting-conditional-design.md`.
> Mở rộng tính năng chart (Phase 3c-3 / 3c-3.2, đã merge `643ea75`). Branch
> `feat/phase-3c-3-3-formatting`.

## Mục tiêu

Đưa **Table** và **Pivot Table** của egret ngang Metabase ở phần *trình bày*: **định dạng giá
trị** theo từng cột (số / tiền tệ / phần trăm / ngày) và **conditional formatting** (luật một
màu + heatmap dải màu), cấu hình qua **settings sidebar** kiểu Metabase. Đây là bản port trung
thực mô hình Metabase (chỉ thị rõ của người dùng: chép sát Metabase — xem
[[metabase-fidelity-preference]]).

Đây là sub-phase đầu trong bốn phần parity (3c-3.3 formatting/conditional → 3c-3.4 tương tác
table → 3c-3.5 pivot collapse/sticky/sort → 3c-3.6 drill-through).

## Phạm vi

**Trong phạm vi:**

- **Engine định dạng** thuần (`formatValue`) — `Intl.NumberFormat` cho số, **dayjs** cho ngày —
  phủ bộ tùy chọn số + ngày của Metabase.
- **Engine conditional-formatting** thuần (`makeCellBackgroundGetter`) — luật một màu theo toán
  tử + gradient dải màu, first-match-wins, alpha blend.
- **Settings sidebar** trong trang Explore với hai mục: **Format** theo từng cột và
  **Conditional Formatting** cấp viz (danh sách luật sắp xếp được + trình sửa luật).
- Gắn vào render ô của **Table** & **Pivot**; lưu trong chart `params`; hydrate khi mở.
- Thêm dependency: **dayjs**.

**Ngoài phạm vi (hoãn):**

- Kéo-thả sắp luật (dùng nút lên/xuống; kéo → 3c-3.4+).
- Khóa cột type-aware field-ref kiểu Metabase (ta khóa theo **tên** cột, duy nhất trong 1 chart).
- Tương tác table (resize/reorder/visibility/sticky/totals/pagination) → 3c-3.4.
- Pivot collapse/sticky/sort → 3c-3.5; drill-through → 3c-3.6.
- Render ô markdown/link/ảnh/email, mini-bar, định dạng tọa độ.
- Default formatting toàn cục/admin (`custom-formatting` của Metabase) — chỉ theo từng chart.

**Không đổi backend** — `params` là JSONB mờ; mọi thiết lập nằm trong đó.

## Bối cảnh — mô hình Metabase ta mô phỏng

**Formatting** (`metabase/.../lib/settings/column.ts`, `utils/formatting/numbers.tsx`,
`lib/formatting/date.tsx`): thiết lập theo cột dưới `column_settings[key]`, áp bởi
`formatValue(value, options)` định tuyến tới `formatNumber` (`Intl.NumberFormat`) hoặc
`formatDateTimeWithUnit` (dayjs). Khóa số: `number_style` (decimal/percent/scientific/currency),
`currency`, `currency_style`, `decimals`, `scale`, `prefix`, `suffix`, `number_separators`,
compact, negative-in-parentheses. Khóa ngày: `date_style`, `date_separator`, `date_abbreviate`,
`time_enabled`, `time_style`.

**Conditional formatting** (`lib/table_format.ts`, `components/settings/
ChartSettingsTableFormatting/`): một mảng cấp viz `table.column_formatting`. Một màu:
`{ columns, type:"single", operator, value, color, highlight_row }`. Dải màu:
`{ columns, type:"range", colors, min_type, max_type, min_value, max_value }`.
`makeCellBackgroundGetter` biên dịch luật → `(value, rowIndex, colName) => color|null` (luật
khớp đầu tiên thắng; alpha ô 0.65, dòng 0.2; gradient bằng d3-scale). Pivot tắt `highlight_row`
và giới hạn ô số.

## 1. Lưu trữ — bổ sung vào chart `params`

`ChartParams` (`frontend/src/api/charts.ts`) thêm hai trường tùy chọn, lưu nguyên trong JSONB
`params`:

```ts
export interface ChartParams {
  spec: QuerySpec;
  rows?: string[]; cols?: string[]; metrics?: string[];
  showRowTotals?: boolean; showColTotals?: boolean; condense?: boolean;
  // MỚI:
  columnSettings?: Record<string, ColumnFormat>; // khóa theo tên cột kết quả
  conditionalFormatting?: Rule[];                 // cấp viz, có thứ tự (khớp đầu thắng)
}
```

## 2. Mô hình dữ liệu (`frontend/src/components/chart/format-types.ts`)

```ts
export type NumberStyle = "decimal" | "percent" | "currency" | "scientific";

export interface ColumnFormat {
  // số
  number_style?: NumberStyle;
  currency?: string;            // ISO 4217, mặc định "USD"
  currency_style?: "symbol" | "code" | "name" | "narrowSymbol";
  decimals?: number;            // số chữ số thập phân chính xác (min=max)
  scale?: number;               // nhân trước khi format
  prefix?: string;
  suffix?: string;
  number_separators?: string;   // 2 ký tự: "[thập phân][nghìn]", mặc định ".,"
  compact?: boolean;            // 1.2K / 3.4M
  negative_in_parentheses?: boolean;
  // ngày
  date_style?: string;          // format dayjs, vd "MMMM D, YYYY"
  date_separator?: "/" | "-" | ".";
  date_abbreviate?: boolean;
  time_enabled?: "minutes" | "seconds" | "milliseconds" | null;
  time_style?: "h:mm A" | "HH:mm";
  // chung
  column_title?: string;        // ghi đè tiêu đề hiển thị
}

export type FormattingOperator =
  | "=" | "!=" | "<" | ">" | "<=" | ">="
  | "contains" | "does-not-contain" | "starts-with" | "ends-with"
  | "is-true" | "is-false" | "is-null" | "not-null";

export interface SingleColorRule {
  columns: string[];
  type: "single";
  operator: FormattingOperator;
  value?: string | number;      // bỏ với is-null/not-null/is-true/is-false
  color: string;                // hex
  highlight_row?: boolean;      // bỏ qua trong pivot
}
export interface ColorRangeRule {
  columns: string[];
  type: "range";
  colors: string[];             // 2 hoặc 3 màu hex
  min_type: "custom" | "all" | null; // null = min theo từng cột
  max_type: "custom" | "all" | null;
  min_value?: number;           // khi min_type === "custom"
  max_value?: number;
}
export type Rule = SingleColorRule | ColorRangeRule;
```

## 3. Engine định dạng (`frontend/src/components/chart/format.ts`, thuần)

`formatValue(value, columnType, fmt?) -> string`:

- **Null/rỗng** → `""`.
- Cột **số** (hoặc có `number_style`): áp `scale`; `scientific` → `toExponential(decimals ?? 2)`;
  `compact` → `Intl.NumberFormat("en",{notation:"compact",maximumFractionDigits: decimals ?? 2})`;
  ngược lại `Intl.NumberFormat("en", { style, currency, currencyDisplay: currency_style,
  minimumFractionDigits: decimals ?? 0, maximumFractionDigits: decimals ??
  (style==="currency"?2:3), useGrouping:true })`. Rồi áp `number_separators` (thay sau),
  `negative_in_parentheses`, `prefix`/`suffix`. Số đến dạng chuỗi được ép kiểu (ca StarRocks
  text-protocol).
- Cột **thời gian**: `dayjs(value)`; nếu sai → `String(value)`. Giải format dayjs từ `date_style`
  (mặc định `"MMMM D, YYYY"`) với thay `date_separator` và `date_abbreviate` (`MMMM`→`MMM`,
  `dddd`→`ddd`); nối format giờ từ `time_style` + độ chi tiết `time_enabled` khi giá trị có giờ.
- **Còn lại** → `String(value)` (kèm `prefix`/`suffix` nếu có).

`columnType` là chuỗi kiểu engine của cột kết quả (trường `type` từ kết quả) → phân loại số /
thời gian / khác (tái dùng regex `NUMERIC` đã có trong `ChartTable`, cộng regex thời gian
`date|time|timestamp`). Mặc định: khi không có `ColumnFormat`, số dùng locale grouping (hành vi
hiện tại), ngày dùng `MMMM D, YYYY`.

## 4. Engine conditional (`frontend/src/components/chart/conditional.ts`, thuần)

`makeCellBackgroundGetter(rows, colIndexByName, rules, opts?: { isPivot?: boolean }) ->
(value, colName) => string | null`:

- Tính trước **extent** số theo từng cột (min/max bỏ qua phi-số) một lần, dùng cho luật range
  với `min_type/max_type === null` hoặc `"all"`.
- Mỗi ô, duyệt luật **theo thứ tự**; luật đầu tiên có `columns` chứa `colName` và cho ra màu sẽ
  thắng.
- **Một màu:** đánh giá `operator` với giá trị (đã ép kiểu); khớp → `alpha(color, isRowFormatter
  ? 0.2 : 0.65)`. Toán tử: so sánh số ép về number; toán tử chuỗi dùng `String(value)`;
  `is-null`/`not-null`/`is-true`/`is-false` không cần `value`. Luật `highlight_row` cho màu cấp
  dòng **chỉ khi `!isPivot`** (xử lý bằng row-getter riêng renderer có thể gọi; pivot bỏ qua).
- **Dải màu:** giải `[min,max]` từ extent/custom; dựng gradient trên 2–3 màu bằng **nội suy RGB
  tuyến tính** của các đầu mút hex (helper nội bộ nhỏ — không cần d3), clamp về `[min,max]`,
  alpha 0.75; giá trị phi-số → `null`.
- `alpha(hex, a)` = helper nhỏ hex→`rgba()`.

## 5. Settings sidebar (`frontend/src/components/chart/settings/`)

Một panel bên phải trong trang Explore, bật bằng nút **Settings** (bánh răng). Hai mục (bộ chuyển
mục đơn giản, mô phỏng sidebar Metabase):

### 5.1 `FormatSection.tsx` — định dạng theo cột
- Một bộ chọn cột (cột kết quả / cột của dataset cho chart). Chọn một cột hiện điều khiển
  theo-kiểu ghi vào `columnSettings[colName]`:
  - **Số:** Style (Normal/Percent/Currency/Scientific) · Currency + Currency-style (khi
    currency) · Decimals · Multiply-by (scale) · Separators · Compact · Negative-in-parentheses ·
    Prefix · Suffix.
  - **Thời gian:** Date style (danh sách preset render kèm ngày mẫu) · Separator (/ - .) ·
    Abbreviate · Time (Off / HH:MM / HH:MM:SS) · 12/24 giờ.
  - **Tất cả:** Ghi đè tiêu đề cột.
- Điều khiển ẩn theo phụ thuộc (tùy chọn currency chỉ khi `number_style==="currency"`, …) — mô
  phỏng `getHidden` của Metabase.

### 5.2 `ConditionalFormattingSection.tsx` — luật cấp viz
- Một **danh sách luật**: mỗi luật là một card (cột + mô tả 1 dòng + ô màu/gradient) với
  **lên/xuống** sắp xếp + xóa; nút **Add rule**.
- Một **`RuleEditor`** (inline, thay danh sách khi sửa):
  1. **Columns** — multi-select cột format được (số/chuỗi/boolean). Chọn loại cột sẽ vô hiệu cột
     không tương thích (toán tử phải đồng nhất).
  2. **Type** — radio Single color / Color range (range chỉ khi tập cột đều là số).
  3. **Single color:** Operator (theo kiểu cột đã chọn), Value (ẩn với null/true/false), Color,
     toggle "Highlight whole row" (ẩn trong pivot).
  4. **Color range:** bộ chọn dải 2–3 màu; radio Min/Max extent (theo từng cột / tất cả cột (đa)
     / custom) với ô số khi custom.
  5. Add/Update/Delete/Cancel.

Cung cấp sẵn bảng màu nhỏ + vài dải màu preset làm hằng số.

## 6. Gắn vào renderer

- **`ChartTable`** thêm prop tùy chọn `columnSettings?` + `conditionalFormatting?`. Text ô →
  `formatValue(value, col.type, columnSettings[col.name])`; `style.backgroundColor` ô → getter
  `(value, col.name)`. Getter `highlight_row` tô cả `<tr>` khi không phải pivot.
- **`PivotTable`** ô giá trị → `formatValue(value, metricType, columnSettings[metricName])`; nền
  qua getter khóa theo cột metric/leaf; `isPivot:true` (chỉ số, không row highlight). Engine hiện
  trả chuỗi đã format — đổi để format giá trị thô qua `formatValue` (engine pivot trả giá trị thô
  từ `cell()`; áp `formatValue` ở renderer hoặc truyền formatter vào engine). **Quyết định:** đổi
  `buildPivot` để `BodyRow.values` giữ **giá trị thô `CellValue[]`** (bỏ `fmt` nội bộ); **renderer**
  áp `formatValue` (hiển thị) và background getter (cần số thô). Cập nhật assertion test
  `values`-dạng-chuỗi của engine sang giá trị thô.

## 7. Gắn vào Explore

`ChartExplore` giữ state `columnSettings` + `conditionalFormatting`, render nút **Settings** +
sidebar, truyền cả hai vào renderer đang dùng, đưa vào `paramsForSave`, và khôi phục ở effect
hydrate. Cột kết quả cho UI settings lấy từ `results.data.columns` thành công gần nhất (phân loại
số/thời gian từ `type` mỗi cột).

## 8. Xử lý lỗi

- Cột không xác định trong luật/settings (vd sau khi đổi shape truy vấn) → luật/setting đó trơ
  (không khớp), không bao giờ throw.
- Tùy chọn format sai (vd decimals âm) → clamp/bỏ qua; format không bao giờ throw.
- Luật trỏ cột đã bỏ vẫn nằm trong `params` nhưng không cho màu.

## 9. Kiểm thử

- **Unit test `format.ts`:** decimal/percent/currency/scientific, decimals, scale, separators,
  compact, negative-in-parentheses, prefix/suffix; date style/separator/abbreviate/time;
  null/sai; số dạng chuỗi.
- **Unit test `conditional.ts`:** mỗi toán tử (khớp + không khớp), is-null/not-null, toán tử
  chuỗi, thứ tự first-match-wins, đầu mút + điểm giữa + clamp gradient, extent theo cột / tất cả /
  custom, đầu ra alpha, phi-số → null.
- **Test component:** `RuleEditor` dựng đúng object luật cho single + range; mục Format ghi đúng
  `columnSettings`.
- **Test renderer:** `ChartTable` áp format tiền + luật một màu (kiểm tra text + style nền);
  `PivotTable` áp format cho ô giá trị.
- **Smoke Playwright thủ công:** mở pivot đã seed, thêm format tiền + luật heatmap qua sidebar,
  xác nhận render trực tiếp; save → mở lại → settings hydrate.

## 10. Cấu trúc tệp

- Tạo `frontend/src/components/chart/format-types.ts` (kiểu dùng chung).
- Tạo `frontend/src/components/chart/format.ts` (+ tests) — `formatValue`.
- Tạo `frontend/src/components/chart/conditional.ts` (+ tests) — getter + helper màu.
- Tạo `frontend/src/components/chart/settings/SettingsSidebar.tsx`, `FormatSection.tsx`,
  `ConditionalFormattingSection.tsx`, `RuleEditor.tsx` (+ tests).
- Sửa `frontend/src/components/chart/ChartTable.tsx` và
  `frontend/src/components/chart/pivot/PivotTable.tsx` (+ `engine.ts` lộ giá trị thô).
- Sửa `frontend/src/api/charts.ts` (`ChartParams`) và
  `frontend/src/routes/explore/ChartExplore.tsx` (host sidebar + persist + hydrate).
- Sửa `frontend/package.json` (thêm `dayjs`).
