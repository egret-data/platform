# Harness đối chiếu chart

Một dashboard tên **ZZ Chart Parity** mang 19 loại chart trên dữ liệu giống hệt Metabase, để mọi khác
biệt hình ảnh giữa hai sản phẩm chỉ có thể do bộ render chart gây ra. Đây là thứ mà PR 01–07 của
chương trình parity đã đo dựa vào.

`scripts/harness/` dựng lại phía egret của nó.

> **Bản tiếng Việt để tham chiếu.** Bản tiếng Anh tại `docs/en/guides/chart-parity-harness.md` là bản chuẩn.

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

**Phía Metabase.** 19 card và H2 Sample Database của nó là kho riêng, reset Postgres của egret không
đụng tới. Dựng lại chúng cần API card và mô hình dashboard riêng của Metabase.

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
trước và sau trong cùng một viewport, và nói rõ bạn dùng cái nào.

## Ghi chú môi trường

Stack là cụm **kind**, không phải docker-compose: backend ở `127.0.0.1:30080`, database của `demo` ở
`demo-trino-trino.egret-engines.svc:8443`.

Nếu bạn phục vụ bản build trên `:5174` để so trong trình duyệt, chú ý **`--strictPort` của Vite không
bảo vệ bạn**: nó bind IPv6 `::1` trong khi server Python giữ IPv4 `127.0.0.1`, nên cả hai cùng chạy và
`127.0.0.1:5174` âm thầm vẫn tới cái kia. **Kiểm hash bundle** với `ls -t frontend/dist/assets/index-*.js`
trước khi tin bất kỳ phép đo nào — trình duyệt cache `index.html`, và bundle cũ đã gây kết luận sai
hơn một lần.
