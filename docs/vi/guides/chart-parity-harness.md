# Harness đối chiếu chart

Một dashboard tên **ZZ Chart Parity** mang 19 loại chart — và, từ `#437`, thêm ba card có breakout —
trên dữ liệu giống hệt Metabase, để mọi khác biệt hình ảnh giữa hai sản phẩm chỉ có thể do bộ render
chart gây ra. Đây là thứ mà PR 01–07 của chương trình parity đã đo dựa vào, và là thứ ba mươi mốt PR
sau đó đã so sánh với.

`scripts/harness/` dựng lại phía egret của nó.

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại `docs/en/guides/chart-parity-harness.md` là bản chuẩn.

## Mốc hiện tại

Bất cứ thứ gì không nằm trong bảng này đều là trôi lệch; hãy đối chiếu bảng này trước khi tin một báo
cáo cũ.

| | |
|---|---|
| dashboard egret | `8a196343-ad93-4968-8040-67598d29c3cb` trong `demo`, `width: "full"` |
| số card | **22** (19 loại viz + 3 card breakout), tất cả `4×5` trên lưới 12 cột |
| `options` của card | **`vizOverride` = 0** — mọi card vẽ theo settings đã lưu của chính chart |
| `updated_at` | **2026-08-08T01:03:49.111686Z** (trước đó là `2026-08-04T08:48:00.790962Z` với 19 card) |
| dashboard Metabase | id `2`, 22 dashcard, tất cả `8×6` trên lưới 24 cột |
| card Metabase | 40–58 là 19 card cũ; **59 `MB · Bar Breakout`, 60 `MB · Area Breakout`, 61 `MB · Bar Breakout Desc`** |

**Đừng lưu hay sửa những chart này hoặc hai dashboard này.** Chúng là mốc mà mọi so sánh về sau đo dựa
vào. Ba card breakout được **THÊM**, không card cũ nào bị đụng tới: 18 trong 19 chart egret có `params`
lưu trữ giống hệt từng byte sau khi seed lại, cả 19 dashcard Metabase giữ nguyên hình học và settings.
Card thứ mười chín được ghi ở mục "Trôi lệch đã biết" bên dưới.

## Ba card breakout, và điểm mù chúng khép lại

Suốt ba mươi mốt PR, mọi card đều để trống `cartesian.series`, nên **không có chart nào có breakout
trong toàn bộ công cụ đo**. Đó là lý do lỗi của `#436` — cùng một card vẽ
`Doohickey / Gadget / Gizmo / Widget` ở lần tải này và `Gizmo / Widget / Gadget / Doohickey` ở lần sau
— sống sót qua ba mươi PR đối chiếu cạnh nhau. Không ai nhìn mà bỏ sót; đơn giản là không có gì để nhìn.

Cả ba đều breakout theo **`category` của `MB Month x Category`**: bốn band, giống hệt nhau ở hai phía
(literal `VALUES` của egret được vật chất hoá từ chính kết quả Metabase của truy vấn mà `MB · Pivot`
đang chạy), và bốn chuỗi ASCII ngắn sắp xếp như nhau trong Trino, H2 và JavaScript — nên khác biệt
trên màn hình là do renderer chứ không bao giờ do collation. `state` (hơn 40 giá trị) và `vendor` bị
loại vì không đọc nổi; `order_month` thì đã là trục x.

| card | thứ chỉ nó bao phủ |
|---|---|
| `MB · Bar Breakout` | bar nhóm, không có sort của tác giả — đúng hình dạng `#436` đã đo |
| `MB · Area Breakout` | **stacked**, nên thứ tự band là vị trí dọc chứ không chỉ là thứ tự legend |
| `MB · Bar Breakout Desc` | `ORDER BY category DESC`, điều kiện duy nhất mà `preserveSeriesOrder` chấp nhận |

**Card thứ ba không phải bản sao của card thứ nhất, và lý do chính là toàn bộ thiết kế.** Metabase
KHÔNG sắp xếp band của nó: `getBreakoutDistinctValues` là `Array.from(new Set(rows.map(...)))` — thứ
tự đến thuần tuý (`echarts/cartesian/model/series.ts:123`), được làm tất định bởi truy vấn chứ không
bởi renderer. Vậy nên với card tăng dần, một renderer LUÔN sắp xếp cũng sẽ trùng khớp Metabase một
cách tình cờ, và harness không phân biệt được "egret sắp xếp" với "egret tôn trọng truy vấn". Chỉ card
nào có thứ tự engine khác thứ tự đã sắp mới phân biệt được, và đó là việc của card Desc. Cả hai đều
đáng có; chỉ một cái là nửa công cụ đo.

**Cố ý bỏ ra ngoài.** COMBO có breakout: mặc định hiển thị theo từng series của egret dưới breakout là
theo vị trí (`seriesDisplayFor`: `index === 0 ? "line" : "bar"`) còn Metabase thì không, nên một card
combo không ghim sẵn sẽ so hai mặc định khác nhau và dạy một bài học sai — mà ghim hiển thị cho từng
band ở cả hai phía lại làm card mù trước thứ tự band, tức là mất luôn lý do tồn tại. SPLIT PANELS
(`#435`): layout của nó dùng chỉ số band nên đúng là một nơi đọc thứ tự này, nhưng bốn panel trong một
card `4×5` cho mỗi band khoảng một phần năm của ~250px, và phép so sẽ là về độ dễ đọc chứ không phải
thứ tự — mà card không thể đổi kích thước, vì hình học `4×5` đồng nhất chính là thứ mọi phép đo của PR
01–06 đã dựa vào.

### Đã kiểm chứng thế nào

egret ở **1600×1000**, bundle `dist` mới build phục vụ qua proxy tại `127.0.0.1:5175` (bundle của cụm
kind đã cũ — xem "Ghi chú môi trường"), **sáu lần tải trang** dashboard: 6/6
`Doohickey / Gadget / Gizmo / Widget` ở bar và area, 6/6 `Widget / Gizmo / Gadget / Doohickey` ở card
Desc. Ba question view của Metabase khớp từng band.

Việc card này **sẽ BẮT ĐƯỢC** `#436` được chứng minh chứ không khẳng định suông, theo hai nửa:

- **Nửa dữ liệu, chạy thật.** Spec trước `#436` của card này (chỉ `ORDER BY order_month`, vì
  `withImplicitBreakoutOrder` khi đó chỉ lấy trục x) trả về **ba thứ tự band khác nhau trong mười lần
  chạy** trên Trino. Spec đã seed, có cả hai breakout, trả về một thứ tự 10/10.
- **Nửa render, chạy thật, trên đúng mã trước `#436`.** Cụm kind vẫn phục vụ bundle trước `#433`, và
  STUDIO dựng lại spec từ config chứ không đọc spec đã lưu — nên mở `MB · Bar Breakout` ở đó chính là
  trải nghiệm trước `#436`. Sáu lần tải vẽ `Doohickey / Gizmo / Widget / Gadget` ba lần và
  `Gadget / Gizmo / Doohickey / Widget` ba lần. Không cái nào là thứ tự đã sắp.

`scripts/harness/charts.breakout.test.ts` giữ chứng minh đó chạy lại được mà không cần stack sống: nó
đưa chính config của card hai thứ tự đến khác nhau và khẳng định hôm nay chúng vẽ giống nhau, còn dưới
`preserveSeriesOrder: true` thì khác nhau — đây không phải mô phỏng gần đúng, vì `sortBands` bắt đầu
bằng `if (preserve) return bands`, đúng nguyên văn biểu thức trước `#436`.

### Khoá settings mà ba card này phụ thuộc

Cả hai phía, vì một card lưu khoá hơi khác sẽ dạy một bài học sai:

| | egret | Metabase |
|---|---|---|
| breakout | `cartesian.series: "category"` | `graph.dimensions: ["ORDER_MONTH", "CATEGORY"]` (phần tử thứ hai) |
| metric | `cartesian.yMetrics: ["revenue"]` | `graph.metrics: ["REVENUE"]` |
| stacking | `cartesian.stacked: true` | `stackable.stack_type: "stacked"` |
| thứ tự band | `orderby` trong spec | `ORDER BY` trong SQL của chính card |

**VIẾT HOA ở phía Metabase**, vì H2 viết hoa mọi định danh không trích dẫn và cột kết quả thật sự là
`ORDER_MONTH` / `CATEGORY` / `REVENUE`. Đây chính là cái bẫy từng khiến `MB · Funnel` hiện "Which
fields do you want to use?".

## Chạy

```bash
cd frontend
npx vite-node --config vitest.config.ts ../scripts/harness/seed.ts -- --workspace=<slug>
```

Cờ: `--workspace` (bắt buộc), `--database`, `--recreate` (xoá rồi tạo lại thay vì upsert; **mặc định
tắt**), `--base`, `--email`, `--password`.

Script idempotent: dataset được dùng lại, chart được cập nhật tại chỗ, PUT dashboard là thay thế toàn
bộ — chạy hai lần không tạo ra 38 chart và không làm đổi ID chart. Nó **từ chối** thay vì đoán khi SQL
của một dataset đã trôi hoặc một chart đã chuyển sang dataset khác.

Việc cuối cùng nó làm là chạy lại một con số đã biết trên instance thật — `2026-01 → 205 orders /
11.094,7` — và fail nếu không khớp.

## Workspace mới cần gì trước đã

Hai bước script không tự làm, và thiếu chúng thì lỗi sẽ rất khó hiểu:

1. **Bật feature.** Workspace mới được tạo với `features: []`; cần `datasets`, `charts`, `dashboards`.
2. **Gắn engine.** `PUT /control-plane/engines/{id}/workspaces` là **thay thế toàn bộ tập** — phải gửi
   kèm các workspace đang có, nếu không những cái bị bỏ sót sẽ **âm thầm mất engine**.

Lưu ý thêm: `DELETE /api/admin/workspaces/{id}` là **xoá mềm** (`status = 'archived'`). Không có
endpoint purge; dòng archived vẫn nằm trong danh sách admin.

## Vì sao params được SINH RA chứ không viết tay

Suốt thời gian được bảo trì bằng tay, harness đã nói dối **năm lần**, mỗi lần đều làm mất công thật của
một PR:

| # | kiểu gì | phát hiện ở |
|---|---|---|
| 1 | sai khoá settings — `pie.dimension` số ít, `waterfall.dimension`, box plot bị đưa metric | lúc dựng tay |
| 2 | `temporal_units: {}` trên mọi chart, làm cả một task của PR vô hiệu | PR 03 |
| 3 | `params.spec.rollup_groups: []` ở pivot — editor dựng lại spec từ settings nên đúng, dashboard đọc spec đã lưu nên vẽ một ô sai | PR 05b |
| 4 | params v2 với `settings: {}` và `spec` đầy đủ — ba trong bốn chart table/object mở ra "Set up your Table" | PR 06 |
| 5 | `MB · Scalar` / `Gauge` / `Progress` lưu `dimensions: ["category"], row_limit: 1000` trong khi descriptor dựng `oneRow` — nên chúng đọc `rows[0]` của một kết quả bốn dòng **không có thứ tự** và hiển thị **danh mục khác nhau giữa các lần tải trang** | PR 07 |

Cả năm là cùng một lỗi: **harness ghi params bằng tay, app ghi qua descriptor.**
`scripts/harness/params.ts` khép lại chuyện đó bằng cách gọi chính `getDescriptor()` / `buildSpec` /
`toSettings` / `mergeVizSettings` của app, nên `harnessParams(viz, config)` giống từng byte với thứ
`paramsForSave()` của editor sẽ lưu. Bẫy 1, 3, 4, 5 trở thành **không thể xảy ra** thay vì được ghi
chú; generator **ném lỗi** nếu một config không sinh ra settings nào trong namespace `<prefix>.*` —
tức bẫy 4 dưới dạng bất biến chạy được.

Bẫy 2 là cái kháng lại: mặc định `month` cho dimension thời gian nằm trong `useEffect` của
`ChartExplore`, không nằm trong descriptor nào. Giờ nó được dùng chung dưới dạng
`withDefaultTemporalUnits()` trong `frontend/src/components/chart/temporal-units.ts`, gọi từ cả hai nơi.

## Script KHÔNG bao gồm gì

**Phía Metabase.** 22 card và H2 Sample Database của nó là kho riêng, reset Postgres của egret không
đụng tới. Dựng lại chúng cần API card và mô hình dashboard riêng của Metabase.

Vì vậy ba card breakout được **dựng bằng tay** qua API đó, giống hệt cách 19 card kia ra đời. Ghi lại
hình dạng vì trong repo này không có gì mã hoá nó:

- `POST /api/card` với `dataset_query` theo dạng MBQL5 mà các card cũ đang dùng —
  `{"lib/type": "mbql/query", "database": 1, "stages": [{"lib/type": "mbql.stage/native", "native": "<SQL>"}]}`.
- `PUT /api/dashboard/2` với **toàn bộ** danh sách `dashcards`. Các mục cũ được gửi lại nguyên vẹn với
  `id`, hình học và `visualization_settings` của chúng; mục mới mang **`id` ÂM**, đó là cách báo cho
  Metabase tạo mới. Lưới 24 cột nhận card `8×6`, ba card mỗi hàng.
- `updated_at` của dashboard **KHÔNG đổi** khi chỉ dashcard thay đổi, nên không thể dùng nó để biết đã
  thêm card hay chưa. Hãy đếm số dashcard.

Thứ **có** bao gồm là **dữ liệu**: chín dataset là literal `VALUES` của Trino, vật chất hoá từ kết quả
truy vấn kéo qua `/api/dataset` của Metabase, commit trong `datasets.ts` dưới dạng dữ liệu — nên script
không cần Metabase đang chạy.

Các bẫy phía Metabase, ghi lại vì đã tốn thời gian:

- **H2 viết hoa mọi định danh không trích dẫn.** `visualization_settings` ghi tên cột chữ thường sẽ im
  lặng không render — đó là lý do `MB · Funnel` báo "Which fields do you want to use?".
- **H2 đòi cả biểu thức select trong `GROUP BY`**, không phải lời gọi hàm bên trong.
- Gauge cần `gauge.segments` chứ không chỉ `gauge.range`; chỉ đặt range sẽ để lại nhãn segment tự suy.
- Gửi `result_metadata: null` khi PUT một card có đổi SQL, nếu không Metabase giữ metadata cột cũ.

## Các bẫy mức API mà script mã hoá

- Metric của chart phải là **metric đã khai báo của dataset**, không phải tên cột thô, nếu không chart
  trả `unknown metric`. `charts.ts` suy ra danh sách metric từ **chính** `buildSpec` đã ghi spec, nên
  bất biến này không thể kiểm một danh sách viết tay đã cũ.
- Tạo dataset dùng `source: {kind: "virtual", sql}` — `kind: "sql"` bị từ chối 422.
- PUT dashboard là thay thế toàn bộ, cần `name` cạnh `cards`, mỗi card có
  `grid_col` / `grid_row` / `size_x` / `size_y`.
- Đăng nhập egret giới hạn **5 lần / 5 phút**. Script đăng nhập một lần.

## Đã được giải quyết kể từ khi viết tài liệu này

**Thứ tự danh mục — đã sửa, và sửa KHÁC cách tài liệu này đề xuất.** Trước đây mục này nói bar và row
vẽ theo thứ tự engine (đo được: bảy trong mười lần một kiểu, ba lần kiểu kia), rằng không descriptor
nào diễn đạt được `orderby`, và rằng bản sửa thuộc về `sortCategories` dưới dạng sắp **giảm dần theo
giá trị series**.

Cả ba đều đã lạc hậu:

- `#394` sắp trục danh nghĩa **tăng dần theo dimension**, và suy một `orderby` trong `buildSpec` để
  **dữ liệu** cũng có thứ tự — đúng thứ tự mà query processor của Metabase tự thêm cho mọi cột breakout
  (`add_implicit_clauses.clj:71`), không phải giảm dần theo giá trị.
- `#395` thêm control sắp xếp vào studio, nên `SharedQueryState` **giờ có** `orderby`.
- `#399` đóng nốt waterfall, họ thứ tư.

Đo lại sau lần seed gần nhất, mười lần gọi mỗi chart: **10/10 `Doohickey, Gadget, Gizmo, Widget`** trên
cả bar lẫn row. Đừng đi tìm lỗi này nữa.

**Nhóm chart số đơn — đã sửa bằng chính việc chạy script này.** `MB · Scalar`, `Gauge`, `Progress`
từng lưu `dimensions: ["category"], row_limit: 1000` rồi đọc `rows[0]` của một `GROUP BY` không thứ tự.
Triệu chứng sống: trên **một** lần tải trang, Scalar hiện 423,7K của Widget còn Gauge hiện 383.061,95
của Gizmo. Giờ cả ba đọc tổng toàn tập **1.510.621,69** với ngưỡng 2.000.000, 15/15 lần gọi giống nhau.

**Nếu bạn dựng lại một bảng mốc, nhớ ghi viewport.** Con số svg `166×184` trong các PR parity được đo ở
một viewport không ai ghi lại; ở 1600×1000 cùng những card đó đo ra `193×214`. Cố định viewport, đo
trước và sau trong cùng một viewport, và nói rõ bạn dùng cái nào. (Đã xác nhận lại `193×214` trên các
card breakout mới ở 1600×1000.)

## Trôi lệch đã biết, thấy nhưng không sửa

- **`MB · Map` có thêm `map.type: "region"`** ở lần seed lại ngày 2026-08-08. Không phải ai đó sửa
  card: `#434` thêm một khoá phân biệt pin/region vào descriptor map, và `fromParams` của nó mặc định
  khoá vắng mặt thành `"region"` — nên seed giờ **GHI RA** đúng cái mặc định mà renderer vốn đã ngầm
  hiểu. Trung tính về hành vi, và không tránh được nếu không bỏ hẳn card đó — toàn bộ thiết kế của
  harness là nó ghi đúng thứ app ghi. Bất kỳ lần seed lại nào sau `#434` cũng sẽ như vậy.
- **Metabase không vẽ legend series trên dashcard `8×6`.** Cả ba card breakout vẽ band trên dashboard
  mà không có legend nào, trong khi egret vẫn vẽ legend trong card `4×5`. Vì thế phép so THỨ TỰ band
  phải làm ở question view (`/question/59`), hoặc trên dashboard bằng màu và vị trí stack. Để nguyên:
  đổi kích thước card Metabase sẽ phá hình học mà các phép đo trước đã dựa vào.
- **`MB · Pivot` hoàn toàn không có bản vẽ Metabase nào, và chưa từng có.** Card 41 hiện
  *"Pivot tables are only supported for questions built in the query builder"* — cả trên dashboard lẫn
  trên `/question/41` — vì các card của harness là SQL thuần. Việc viết hoa settings (xem bên dưới)
  không đổi được điều này và cũng không thể: card không bao giờ chạy đến chỗ đọc chúng. Điều này đã
  được ghi ở `2026-08-02-chart-parity-05-label-room.md`; nhắc lại ở đây vì nó rất dễ bị nhầm với lỗi
  chữ hoa/thường, mà nó không phải. **Mục tiêu của pivot vẫn phải lập luận trên cơ sở riêng của nó.**
  *Dữ liệu* của nó vẫn đáng tin — literal `VALUES` của egret được vật chất hoá từ chính kết quả card
  này chạy, và truy vấn chạy tốt; Metabase chỉ từ chối phần *render pivot*.

## Lỗi settings bị vô hiệu, và cái giá của nó (2026-08-08)

Tài liệu này từng nói tên cột chữ thường trên các card Metabase gốc là vô hại — *"Chúng vẫn vẽ đúng …
settings tình cờ trùng với mặc định cho các hình dạng kết quả này."* **Điều đó SAI với bốn card**, và
bốn card đó không thể nhìn ra chỉ từ settings. Nay đã sửa; mục này là biên bản, vì cái bẫy này đã cắn
harness hai lần rồi (funnel, rồi đến nhóm này).

### Cơ chế, chính xác

H2 viết hoa các định danh không trích dẫn, nên mọi cột kết quả thật sự là `CATEGORY` / `ORDER_MONTH` /
`REVENUE`. Metabase phân giải một setting tên cột bằng `columnsAreValid`
(`visualizations/lib/utils.ts:35`), vốn là **tra cứu khoá object phân biệt hoa thường** —
`colsByName[name]`, cộng với `col.name === columnName` nghiêm ngặt lúc render
(`lib/graph/columns.ts:16`). Tên không phân giải được sẽ trượt `isValid`, và `getComputedSetting`
(`lib/settings.ts:118`) rơi xuống `getDefault`. **Card vẽ theo mặc định của Metabase và setting đã lưu
không bao giờ được đọc.** Nó không cảnh báo, cũng không vẽ trắng — chính vì thế lỗi này sống sót.

Hai hệ quả đáng biết:

- `graph.dimensions` và `graph.metrics` được kiểm **cùng nhau** bởi `getAreDimensionsAndMetricsValid`,
  nên một tên sai làm hỏng cả hai.
- `pivot_table.column_split` là ngoại lệ: nó dùng `getValue` chứ không `isValid`/`getDefault`, nên giá
  trị lưu luôn được xử lý và tên sai hoa thường bị loại khỏi phân vùng của nó thay vì cả setting rơi
  về mặc định.

### Con số thật: 12, không phải 19

"Mười chín" là đếm gộp toàn bộ card gốc. Xét từng card, chỉ **12 card thực sự có tham chiếu cột chữ
thường**, và trong đó chỉ **4 card vẽ khác với điều settings nói**:

| | card | |
|---|---|---|
| chữ thường **và sai nhìn thấy được** | **46, 47, 56, 58** | scatter, combo, sankey, map |
| chữ thường nhưng mặc định trùng khớp | 42, 43, 44, 45, 48, 53, 55 | bar, row, line, area, pie, waterfall, box plot — đều là dạng kết quả `[dimension, metric]`, nơi mặc định *chính là* ý định đã lưu |
| chữ thường, nhưng vô hiệu vì lý do khác | 41 | pivot — không bao giờ render (xem "Trôi dạt đã biết") |
| không có setting nào đặt tên cột | 40, 49, 54, 57 | `visualization_settings: {}` |
| settings không đặt tên cột nào | 50, 51 | `progress.goal`, `gauge.range`/`segments` — là số, luôn có hiệu lực |
| vốn đã chữ hoa | 52, 59, 60, 61 | funnel (đã sửa trước đó) + ba card breakout |

### Bốn card đó thực sự đang vẽ gì

Đo ở 1600×1000 trên `/dashboard/2` và các question view, trước và sau:

| card | settings nói | đang vẽ | nay |
|---|---|---|---|
| 47 `MB · Combo` | 2 metric; `REVENUE` là **line** ở trục phải | **3** series (`ORDERS`, `REVENUE`, `AVG_TOTAL`), `REVENUE` là **bar** — `getDefaultMetrics` lấy mọi cột số | 2 series, `REVENUE` là line |
| 58 `MB · Map` | tô theo `REVENUE` | tô theo **`ORDERS`** — mặc định của `map.metric` là `cols.find(isMetric)`, tức cột số *đầu tiên*. Legend đọc `21 – 206 … 1,342 +`, đúng min/max của ORDERS; REVENUE trải `1.358,69 – 108.466,6` | legend `1.4k – 12.4k … 108.5k +` |
| 46 `MB · Scatter` | bubble theo `ORDERS` | bubble **đồng đều** — `getDefaultScatterColumns` trả `bubble: null` | bubble có kích thước |
| 56 `MB · Sankey` | `SOURCE → CATEGORY` | **`CATEGORY → SOURCE`**, tức ngược chiều — `findSensibleSankeyColumns` tự chọn | `SOURCE → CATEGORY` |

Tám card còn lại giống hệt từng điểm ảnh trước và sau; chỉ `visualization_settings` thay đổi trên mọi
card, và dashboard 2 vẫn đủ 22 dashcard ở `8×6`, không có override ở cấp dashcard.

### Điều này có làm lay chuyển kết luận nào phía egret không? Không — và đó mới là điểm chính

**Không có bài viết nào trong `docs/`, `scripts/harness/` hay các commit #407–#438 từng nói egret lệch
về số series của combo, bubble của scatter, chiều của sankey hay metric của map.** Bảng so sánh từng
chart duy nhất của harness (`2026-08-02-chart-parity-04-colour-assignment.md`) không hề bao gồm combo
hay map, và chỉ bao gồm scatter và sankey ở khía cạnh màu và hình học nhãn. Nên không kết luận đã ghi
nào phải rút lại.

Đó là may mắn chứ không phải kết quả tốt. Cả bốn trường hợp, **egret vốn đã lưu đúng điều settings của
Metabase định nói** (`scripts/harness/charts.ts:99-122`), nên ai *đã* so chúng thì sẽ thấy egret "sai"
trong khi thực chất đang so với một card chưa được cấu hình.

Một phép đo cũ đáng đánh dấu là chưa giải thích được. `2026-08-02-chart-parity-05-label-room.md:45`
ghi nhãn phải dài nhất của sankey Metabase là **`Doohickey`** — một giá trị CATEGORY, tức
`SOURCE → CATEGORY`, đúng chiều settings định nói và ngược với điều card 56 đang vẽ ngày 2026-08-08.
Hoặc mặc định đã đổi dưới chân nó (một lần làm mới `result_metadata` cũ đủ gây ra điều đó), hoặc nhãn
bị gán nhầm. Không đáng truy bây giờ, vì bản sửa chữ hoa loại bỏ hoàn toàn sự phụ thuộc vào cơ chế dò
mặc định của Metabase — **và đó mới là lý do thật để sửa một setting vô hiệu ngay cả khi mặc định tình
cờ trùng: mặc định có thể trôi, setting đã lưu thì không.**

### Tái lập

Phía Metabase vẫn chưa có script. Settings đã sửa được liệt kê ở đây để có thể áp lại bằng tay hoặc
bằng bất kỳ công cụ nào sau này, mỗi cái qua `PUT /api/card/{id}` chỉ với
`{"visualization_settings": …}` — xem bảng JSON trong bản tiếng Anh (bản chuẩn).

`series_settings` được đánh khoá theo **tên cột metric** với chart một card
(`getSeriesVizSettingsKey`), nên cũng phải viết hoa — khoá chữ thường lặng lẽ không khớp gì cả và
series giữ nguyên màu lẫn kiểu hiển thị mặc định.

**Làm sao biết một setting có hiệu lực, mà không cần đọc mã nguồn Metabase.** Đừng kiểm tra rằng card
"trông đúng" — đó chính là phép kiểm đã thất bại ở đây suốt cả loạt PR. Hãy đổi setting thành một giá
trị *sai* rồi tải lại: nếu hình không đổi, setting đang vô hiệu.

## Ghi chú môi trường

Stack là cụm **kind**, không phải docker-compose: backend ở `127.0.0.1:30080`, database của `demo` ở
`demo-trino-trino.egret-engines.svc:8443`.

Nếu bạn phục vụ bản build trên `:5174` để so trong trình duyệt, chú ý **`--strictPort` của Vite không
bảo vệ bạn**: nó bind IPv6 `::1` trong khi server Python giữ IPv4 `127.0.0.1`, nên cả hai cùng chạy và
`127.0.0.1:5174` âm thầm vẫn tới cái kia. **Kiểm hash bundle** với `ls -t frontend/dist/assets/index-*.js`
trước khi tin bất kỳ phép đo nào — trình duyệt cache `index.html`, và bundle cũ đã gây kết luận sai
hơn một lần.

**Bundle mà kind phục vụ không phải bundle bạn vừa build, và nó cũ hơn nhiều so với vẻ ngoài.** Đo
ngày 2026-08-08: `:30080` phục vụ `index-BWjx3fdn.js`, không chứa `unpinFromZero`, `tooltipMetrics`
hay `splitPanels` — tức là cũ hơn `#433`, thua `main` mười một PR đã merge. Mọi "kiểm tra thật" bằng
cách mở `:30080` đều là kiểm tra bản build đó. Xác định niên đại bằng cách grep JS được phục vụ với
các mục trong `KEYS` của `cartesian-codec.ts`: chúng là **chuỗi ký tự** nên sống sót qua minify (tên
thuộc tính như `preserveSeriesOrder` cũng vậy, nhưng một khoá settings như `cartesian.yMetrics` thì
KHÔNG — codec ghép chuỗi để tạo ra nó, nên grep luôn trả về 0 và chẳng chứng minh điều gì).

Một static server thuần không thay thế được, vì app cùng origin với API của nó, còn proxy của dev
server trỏ vào `:8080` của stack docker chứ không phải `:30080` của kind. Cách chạy được gói trong
khoảng 60 dòng: phục vụ `frontend/dist` và reverse-proxy `/api` sang `127.0.0.1:30080` trên một cổng
riêng, bind rõ ràng vào `127.0.0.1`.

Hai điều nữa từng tốn thời gian trong trình duyệt:

- **Cookie phiên của egret không phân biệt theo cổng**, nên một lần đăng nhập ở `127.0.0.1:5175` cũng
  là đăng nhập ở `127.0.0.1:30080`. Rất hữu ích với giới hạn 5 lần / 5 phút.
- Mở một chart trong STUDIO sẽ bật hộp thoại `beforeunload` ở lần điều hướng kế tiếp, vì effect mặc
  định gộp theo tháng đánh dấu nó "bẩn" ngay khi mở. Nó chặn công cụ snapshot và evaluate của
  Playwright và không phải lúc nào cũng tự tắt; mở **tab mới** cho mỗi lần tải đáng tin hơn là reload,
  và dù sao cũng không có gì bị lưu.
