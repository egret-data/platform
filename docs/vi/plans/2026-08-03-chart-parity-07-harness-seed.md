# Chart parity PR 07 — script seed harness — kế hoạch triển khai

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại
> `docs/en/plans/2026-08-03-chart-parity-07-harness-seed.md` là bản chuẩn; mã nguồn và lệnh nằm ở đó.

**Mục tiêu:** Commit một script dựng lại chín dataset và dashboard 19 chart **ZZ Chart Parity**, để
phép so sánh sống sót qua một lần reset database và ai cũng tái lập được.

**Kiến trúc:** Script TypeScript chạy bằng `vite-node`, sinh params **qua chính descriptor của app**
thay vì ghi tay. Đó là toàn bộ thiết kế, và lý do ở ngay dưới.

---

## Spec bảo "mã hoá các bẫy". Hãy làm cho phần lớn chúng không thể xảy ra.

Workstream 7 liệt kê bốn bẫy cần mã hoá. Từ khi spec được viết, harness đã nói dối **thêm bốn lần**, và
cả bốn cùng một gốc: **harness ghi params bằng tay, còn app ghi qua descriptor.**

| # | harness nói dối kiểu gì | phát hiện ở |
|---|---|---|
| 1 | sai khoá settings — `pie.dimension` số ít, `waterfall.dimension`, box plot bị đưa metric | lúc dựng tay |
| 2 | `temporal_units: {}` trên mọi chart, khiến cả một task của PR vô hiệu | PR 03 |
| 3 | `params.spec.rollup_groups: []` ở pivot — editor dựng lại spec từ settings nên đúng, dashboard đọc spec đã lưu nên vẽ một ô sai | PR 05b |
| 4 | params v2 với `settings: {}` và `spec` đầy đủ — ba trong bốn chart table/object mở ra "Set up your Table" | PR 06 |

Cả bốn là cùng một lỗi khoác áo khác nhau. Mã hoá từng cái thành một luật trong script Python để mặc
cho biến thể **tiếp theo** tự do xuất hiện.

**Đã kiểm chứng khả thi trước khi viết kế hoạch này.** `viz/registry.ts` xuất `getDescriptor(viz)`;
`buildSpec`, `toSettings`, `mergeVizSettings` đều là hàm thường, không phụ thuộc React ở chỗ gọi.
`vite-node` đã có sẵn qua vitest. Một phép thử:

```ts
const d = getDescriptor("pie")!;
d.toSettings({ ...d.defaultConfig, dimensions: ["category"], metric: "revenue" });
//  → {"pie.dimensions":["category"],"pie.metric":"revenue"}
```

`pie.dimensions` — đúng khoá số nhiều mà bẫy 1 nói tới — ra đúng mà không ai cần biết đó là bẫy.
`rollup_groups` cũng vậy, tức bẫy 3.

**Những bẫy ở mức API vẫn phải mã hoá**, vì không descriptor nào biết chúng:

- Metric của chart phải là **metric đã khai báo của dataset** (`POST .../datasets/{id}/metrics` với
  biểu thức kiểu `SUM(revenue)`), không phải tên cột thô, nếu không chart trả `unknown metric`.
- Tạo dataset dùng `source: {kind: "virtual", sql}` — `kind: "sql"` bị từ chối 422.
- PUT dashboard là **thay thế toàn bộ**, cần `name` cạnh `cards`, và mỗi card có
  `grid_col` / `grid_row` / `size_x` / `size_y`.
- Đăng nhập egret bị giới hạn **5 lần / 5 phút**.

## Phạm vi

**Chỉ phía egret.** 19 card của Metabase và H2 Sample Database của nó là kho riêng, reset Postgres của
egret không đụng tới, và dựng lại chúng cần API card + mô hình dashboard riêng của Metabase. Ngoài phạm
vi, ghi rõ trong header của script.

Thứ **có** trong phạm vi từ phía Metabase là **dữ liệu**: chín dataset là các literal `VALUES` của Trino,
vật chất hoá từ kết quả truy vấn kéo qua `/api/dataset` của Metabase. Các literal đó vào repo dưới dạng
dữ liệu, nên script không cần Metabase đang chạy.

**Một bẫy phía Metabase cần ghi vào docs dù ngoài phạm vi**: H2 viết hoa mọi định danh không trích dẫn,
nên `visualization_settings` của card ghi tên cột chữ thường sẽ **im lặng không render** — đó là lý do
`MB · Funnel` báo "Which fields do you want to use?" cho tới khi PR 05b sửa.

## Các task

| # | nội dung |
|---|---|
| 1 | Params sinh ra đúng cách editor sinh — `scripts/harness/params.ts`, đối chiếu byte với `paramsForSave()` |
| 2 | Chín dataset và các metric đã khai báo |
| 3 | 19 chart và dashboard |
| 4 | **Chứng minh bằng cách dựng lại thật**, không phải bằng đọc |
| 5 | Docs + mở PR (KHÔNG merge) |

### Bẫy duy nhất mà thiết kế này không xoá được

`temporal_units` **không** nằm trong descriptor. `ChartExplore` đặt mặc định `"month"` cho dimension
thời gian trong một `useEffect` — tức trong component, không phải trong `buildSpec`. Một caller headless
sẽ nhận `{}` nếu không tự áp luật đó. Đây là bẫy 2, và là chỗ duy nhất "mã hoá luật" vẫn là đáp án.
Phải có test khẳng định chart có dimension thời gian ra `temporal_units: {"<dim>": "month"}`, và có
comment nói rõ luật này bị nhân bản từ `ChartExplore` cùng lý do vì sao không dùng chung được.

### Lấy dữ liệu từ instance đang chạy, đừng gõ lại

Cả chín dataset lẫn 19 config chart phải được **đọc ngược từ API**, không phải gõ lại — để dữ liệu seed
giống hệt từng byte với dữ liệu mà mọi phép đo trong PR 01–06 đã dựa vào. Kiểm bằng một con số đã biết:
`2026-01 → 205 orders / 11.094,7`. Hình học card cũng vậy: svg 166×184 mà PR 02–05 đo được là hệ quả
của `size_x`/`size_y` hiện tại.

**Chú ý những chart mà harness từng làm sai và PR sau đã sửa** — `rollup_groups` của pivot,
`temporal_units` của pivot và `MB · Trend`, `settings` rỗng của ba chart table/object. Đọc trạng thái
**hiện tại** (đã được PR 05b và PR 06 sửa), không phải thứ seed ban đầu. Nếu còn chart nào mang hình
dạng mà descriptor không sinh ra được, **báo cáo** — đó là lời nói dối thứ năm vẫn còn nguyên.

### Tiêu chí nghiệm thu (Task 4)

Chạy script vào một **workspace mới** — không phải `demo`, để harness hiện tại còn nguyên nếu script sai
— rồi so hai bên trong trình duyệt. Kiểm riêng bốn thứ mà các PR trước phải sửa, vì đó chính là những
thứ một bản seed ghi tay hay làm sai:

1. Pivot render 14 hàng với bốn cột danh mục.
2. `MB · Table` mở ra với đủ mười cột.
3. Trục x của line/area/combo/waterfall đọc `January 2026`.
4. Chín nút sankey có chín màu phân biệt.

**Kiểm hash bundle trước** khi tin bất kỳ phép đo nào — trình duyệt cache `index.html`, và điều đó đã
gây một kết luận sai ở PR 05b.

**Chênh lệch là một phát hiện, không phải thất bại** — nó nghĩa là harness dựng tay và app bất đồng ở
đâu đó, đúng thứ PR này sinh ra để phơi bày.

### Điểm yếu đã biết

Task 3 yêu cầu đọc lại 19 config và nhận ra cái nào descriptor không sinh ra được. Nếu có quá hai cái
sai, đó là phát hiện đáng báo cáo riêng trước khi hoàn thiện script — nói ra thay vì lặng lẽ chuẩn hoá,
vì một harness đã bị chuẩn hoá sẽ âm thầm đổi thứ mà các phép đo sau này đem ra so.
