# Đưa Dashboard về ngang Metabase — Thiết kế

> Bản tiếng Việt tham chiếu. Bản tiếng Anh tại
> `docs/en/specs/2026-08-09-dashboard-parity-design.md` là bản chuẩn — khi hai bản lệch nhau,
> lấy bản tiếng Anh.

**Trạng thái:** đã duyệt 2026-08-09
**Phạm vi:** bề mặt dashboard của egret (chế độ xem, chế độ sửa, card, filter, tab, thông tin)
**Tham chiếu:** Metabase OSS tại `data-portal/metabase`, bản chạy `localhost:3033`
**Đối tượng:** egret tại `data-portal/egret`, bản chạy `localhost:30080`

Đây là phần tiếp nối của `2026-08-01-chart-parity-design.md`. Đợt đó đã đưa mười chín bộ vẽ
chart và bảng thiết lập của chúng về ngang Metabase. Nên chart *bên trong* một card dashboard
đã đúng rồi; tài liệu này nói về mọi thứ bao quanh nó.

---

## 1. Cách tìm ra các điểm lệch

Hai bản kiểm kê code độc lập (mỗi bên một bản) cộng với một lượt đo trực tiếp trên cả hai app
đang chạy, ở các khổ 1440×900, 1800×900 và 390×844. Mọi khẳng định bên dưới đều kèm `path:line`
hoặc một con số đo được. Những phát hiện tan biến khi soi kỹ được ghi ở §9 để không ai suy lại
lần nữa.

Dashboard harness là "ZZ Chart Parity", 22 card mỗi bên trên dữ liệu giống nhau từng byte
(`docs/en/guides/chart-parity-harness.md`). Nó được dựng để so *bộ vẽ*, nên là một dụng cụ yếu
với phần khung dashboard — hầu hết phát hiện ở đây đến từ việc lái tay giao diện, không phải từ
harness.

---

## 2. Các lỗi đo được

### 2.1 Dashboard hỏng hoàn toàn dưới ngưỡng mobile

Đo ở khổ 390×844 trên dashboard harness:

| | rộng lưới | kích thước card | cuộn ngang |
|---|---|---|---|
| egret | 86px | **9 × 224 px** | có — `scrollWidth 441 > 390` |
| Metabase | 362px | 362 × 290 px | không — `scrollWidth 390 === 390` |

Hai nguyên nhân độc lập:

1. `DashboardGridLayout.tsx:57-60` dựng `RGL` trần với `cols={GRID_COLS}` cứng ở 24. Không có
   breakpoint, không có wrapper responsive, không có số cột cho mobile. Hai mươi tư cột trong
   một khung 86px cho ra cột rộng 3,6px.
2. `AppShell.tsx:9-19` chỉ thu sidebar theo `localStorage`. Không có truy vấn viewport nào
   trong `components/layout/` (lệnh `matchMedia` duy nhất ở `Header.tsx:58` là
   `prefers-color-scheme`). Ở 390px sidebar vẫn chiếm 264px.

Luật của Metabase, trích nguyên văn từ `utils/dashboard_grid.ts:11-26` và
`dashboard/components/grid/utils.ts:21-36`:

- `GRID_BREAKPOINTS = { desktop: 753, mobile: 752 }`
- `GRID_COLUMNS = { desktop: 24, mobile: 1 }`
- margin `{ desktop: [6, 6], mobile: [6, 10] }`
- ở mobile mọi card bị ép về `x: 0, w: 1` và xếp chồng theo thứ tự layout
- chiều cao mobile theo loại hiển thị — `action` 1, `link` 1, `heading` 2, `scalar` 4,
  `text` `max(2, chiềuCaoDesktop)`, còn lại `MOBILE_DEFAULT_CARD_HEIGHT = 6`
  (`visualizations/shared/utils/sizes.ts:36-58`)
- tắt kéo và đổi kích thước ở ngưỡng mobile (`grid/GridLayout.tsx:234-235`), và **bỏ qua** mọi
  thay đổi layout trừ khi đang sửa *và* đang ở desktop (`DashboardGrid.tsx:299-301`) — thiếu
  cái chốt thứ hai này thì một lần mở bằng điện thoại sẽ âm thầm ghi đè layout desktop đã lưu
  thành một cột

### 2.2 Huỷ chỉnh sửa làm mất việc mà không hỏi

`DashboardView.tsx:249` vứt toàn bộ trạng thái nháp ngay lập tức. Đã chứng minh trực tiếp: một
filter thêm trong chế độ sửa biến mất khi bấm Cancel, không một lời cảnh báo.

Metabase đưa Cancel qua `DashboardLeaveConfirmationModal` khi dashboard đang bẩn — "Discard your
changes?" với Cancel / Discard changes (`DashboardLeaveConfirmationModal.tsx:29-46`, gọi ở
`DashboardHeader.tsx:69-87`). Nó chặn cả điều hướng route chứ không chỉ nút Cancel.

egret có chốt `beforeunload` (`lib/useUnsavedChangesWarning.ts:14-24`) nhưng cố ý không chặn
điều hướng SPA. Nên hôm nay lối ra duy nhất được bảo vệ là đóng tab — cách **ít** có khả năng
làm mất việc nhất.

### 2.3 Ô chọn cột để nối filter không đủ chỗ hiện tên cột

Đo với sidebar filter đang mở ở 1440px: ô `<select>` trong lớp phủ của mỗi card rộng **97px**
(`clientWidth === scrollWidth === 97`), do `CardFilterMapOverlay.tsx:8-19` vẽ, nằm trong một
card lúc đó chỉ rộng 125px.

Metabase cho ô này rộng bằng card, có nhãn ("Column to filter on", "Connect to", "Variable to
map to" tuỳ loại card — `DashCardCardParameterMapperContent.tsx:106-120`), và khi một card
*không* nối được thì nó nói rõ vì sao: "The selected filter is on another tab." `:188-195`,
"This filter can only connect to its own card." `:197-204`, cộng gợi ý riêng cho card
text/link/iframe `:134-144`.

egret vẽ một ô select cụt và không gì thêm.

### 2.4 Lưới dẫn hướng lúc sửa đậm gần gấp đôi Metabase

Cả hai bên dùng chung cơ chế — một lưới ô SVG lặp làm `background-image`
(egret `grid-background.ts:15-25`, port từ `generateGridBackground` của Metabase). Chỉ khác màu
nét. Trộn lên nền của chính mỗi bên rồi đo:

| | nét | nền | tương phản |
|---|---|---|---|
| egret (tối) | `oklch(0.992857 0 0 / 0.28)` → `#4d4d4d` | `#090909` | **2,36 : 1** |
| Metabase | `hsla(195, 6%, 87%)` → `#dcdfe0` | `#f9f9fa` | **1,27 : 1** |

`DashboardGridLayout.tsx:28` ghi rõ lý do: `--border` bị đánh giá là "gần như vô hình" nên nét
được pha về phía `--foreground` bằng `color-mix`. Nhưng lưới của Metabase *vốn dĩ* gần vô hình —
nó là một gợi ý nằm dưới các card, không phải một bảng ô vuông. Ở 2,36:1 trong theme tối, lưới
trống trở thành thứ nổi nhất trên khung.

### 2.5 Filter `location` nhiều giá trị không sống sót qua URL

`useDashboardParams.ts:9` liệt kê `ARRAY_TYPES` dùng để quyết định có mã hoá JSON hay không.
`location` là loại chọn nhiều (`FilterPill.tsx:87`) nhưng vắng mặt trong danh sách đó, nên ở
`:32` một giá trị location nhiều phần tử bị ghi bằng `String(value)` — một chuỗi nối dấu phẩy
không thể tách ngược thành mảng. Tải lại trang là mất hết trừ giá trị đầu, hoặc hỏng những giá
trị có chứa dấu phẩy.

---

## 3. Những gì backend đã có mà không nút nào chạm tới

### 3.1 Chiều rộng cố định / tràn

Cột đã có (`migrations/0018_dashboard_width.sql`), DTO mang nó
(`egret-analytics-api/src/bi.rs:369`), PUT kiểm tra nó (`routes.rs:2702-2706`), và frontend đã
*vẽ* nó — `DashboardView.tsx:178,462` áp `max-w-[1200px]` khi `width === "fixed"`. Nhưng
**không control nào set nó**, và cả hai đường lưu đều ghi lại giá trị cũ nguyên vẹn
(`DashboardView.tsx:390,404`). Mọi dashboard kẹt ở mặc định server `"full"` (`bi.rs:411-413`).

Metabase: khung mặc định **1048px, căn giữa** — đo trực tiếp ở khổ 1800px, lưới vẫn rộng 1048px
tại x=376. Công tắc là một switch "Full width" duy nhất trong menu "…" lúc sửa
(`ExtraEditButtonsMenu.tsx:17-46`).

**Quyết định.** Lấy 1048px của Metabase làm số đo cố định (thay 1200px của egret) và cho
dashboard **tạo mới** mặc định `"fixed"`. Dashboard cũ giữ nguyên giá trị đã lưu, nên không có
gì đang dùng bị đổi hình.

### 3.2 Hành động cấp dashboard chỉ với tới được từ trang danh sách

Các mutation `duplicate`, `trash`, `delete`, `folder` đều có trong `api/dashboards.ts:121-136`
và đã hiện thực đầy đủ phía server (`crates/egret-bi/src/dashboard.rs`), nhưng `DashboardView`
không import cái nào (`DashboardView.tsx:61` chỉ lấy `update, restore, addBookmark,
removeBookmark`). Muốn nhân bản chính dashboard đang xem thì phải rời trang về danh sách.

Menu "…" của Metabase ngay trên trang dashboard: Reset all filters, Subscriptions, Enter
fullscreen, Edit settings, Move, Duplicate, Move to trash (`DashboardActionMenu.tsx:88-163`) —
đã kiểm trực tiếp.

### 3.3 `description` khi tạo

`CreateDashboard.description` đã có (`bi.rs:376-382`); hộp thoại tạo chỉ gửi `{name}`
(`Dashboards.tsx:73`).

---

## 4. Các workstream

Xếp theo phụ thuộc. W1 phải trước W2 — cả hai đều sửa `DashboardGridLayout.tsx`.

### W1 — Lưới responsive

Port nguyên mô hình breakpoint của Metabase (hằng số ở §2.1). Cụ thể:

- mở rộng `dashboard-grid.ts` với `GRID_BREAKPOINTS`, `GRID_COLUMNS`,
  `MOBILE_HEIGHT_BY_DISPLAY_TYPE`, `MOBILE_DEFAULT_CARD_HEIGHT`, và margin theo breakpoint
- `DashboardGridLayout.tsx` chuyển sang layout responsive của RGL, suy layout mobile từ layout
  desktop chứ không lưu layout thứ hai
- **chỉ lưu thay đổi layout khi đang sửa VÀ đang ở breakpoint desktop** — cái chốt này là toàn
  bộ vấn đề; thiếu nó, mở dashboard trên điện thoại là ghi đè layout của nó
- tắt kéo và đổi kích thước ở breakpoint mobile
- `AppShell.tsx` thêm một luật viewport: dưới 768px thanh điều hướng mặc định thu lại và không
  chiếm chỗ layout. Việc này chạm mọi route; lựa chọn trong localStorage vẫn quyết định ở
  desktop.

Đường auto-height bằng `ResizeObserver` (`DashboardView.tsx:354-366`, `AutoHeightMeasure.tsx`)
cũng không được ghi `size_y` khi đang ở breakpoint mobile, cùng lý do với chốt layout.

### W2 — An toàn và độ trung thực khi sửa

- **Hộp xác nhận huỷ.** Cancel khi nháp đã bẩn thì mở hộp xác nhận ("Discard your changes?" /
  Cancel / Discard changes). Nháp sạch thì thoát ngay, như Metabase. Độ bẩn so nháp với ảnh
  chụp mà `startEdit()` lấy (`DashboardView.tsx:240-248`).
- **Toast hoàn tác** cho ba hành động phá huỷ mà Metabase cho hoàn tác: xoá card
  (`cards-typed.ts:472-523`), xoá tab (`use-dashboard-tabs.ts:43-61`), thay card
  (`DashboardGrid.tsx:399-406`). Mỗi cái khôi phục trạng thái nháp trước đó; đều ở mức nháp nên
  không cần gọi server.
- **Màu nét lưới.** Đưa tương phản lưới dẫn hướng về dải của Metabase. Mục tiêu ≤ 1,5:1 so với
  nền dashboard ở cả hai theme, kiểm bằng cách tính giá trị đã trộn — không kiểm bằng mắt.
- **Ô nối cột.** Ô select trong lớp phủ rộng bằng card và mang đúng dòng tiêu đề của Metabase
  theo loại card. Card không nhận được filter hiện tại thì hiện lý do thay vì một ô select rỗng
  — ít nhất là "The selected filter is on another tab." và các gợi ý cho card text/heading/link.

### W3 — Công tắc chiều rộng cố định / tràn

- Một switch "Full width" trong menu "…" mới lúc sửa, ghi `width` qua đường lưu sẵn có.
- Số đo cố định thành 1048px.
- Dashboard mới mặc định `"fixed"` (đổi mặc định phía server cộng đường tạo).

### W4 — Header chế độ xem

- Thay `<select>` auto-refresh bằng menu popover giống Metabase — tiêu đề "Auto Refresh", dấu
  tích ở dòng đang chọn, cùng bảy giá trị (vốn đã trùng: Off/1/5/10/15/30/60 phút,
  `RefreshWidget.tsx:12-55` so với `DashboardView.tsx:485-495`).
- **Refresh thủ công**, hôm nay chưa có — chỉ bộ hẹn giờ mới bump `refreshNonce`
  (`useDashboardViewControls.ts:27`).
- **Vòng đếm ngược**: nút hiện thời gian tới lần refresh kế khi đã đặt chu kỳ
  (`RefreshWidgetTarget.tsx:33-54`).
- **Menu "…"** trên trang dashboard: Duplicate, Move to folder, Move to trash, Edit settings,
  Reset all filters. Mọi mutation đã có sẵn (§3.2).
- **Phím tắt.** Port phần ánh xạ được sang tính năng egret có: `e` sửa / huỷ sửa, `s` lưu,
  `o` bookmark, `]` sidebar thông tin, `f` thêm filter, `a` thêm card, `1`–`9` chọn tab. Không
  port: `q`/`n` (tạo câu hỏi mới từ dashboard — egret không có luồng này),
  `$mod+backspace` (xoá vào thùng rác — một phím tắt phá huỷ mà không có hoàn tác thì tệ hơn là
  không có).

### W5 — Mức card

- **Định dạng tải về.** Hôm nay chỉ CSV và PNG (`ChartExportMenu.tsx:40-56`). Thêm `.xlsx` và
  `.json`, cộng công tắc "Keep the data formatted" của Metabase (mặc định bật, khớp hành vi vô
  điều kiện hiện tại) và dòng ghi chú giới hạn dòng. PNG vẫn phụ thuộc việc card có vẽ `<svg>`.
  Xuất của egret hoàn toàn ở client (`export-chart.ts`), nên `.json` là miễn phí còn `.xlsx`
  cần một bộ ghi. Dùng một thư viện nhỏ (nhóm `xlsx`, import lười để không vào bundle chính)
  thay vì tự viết định dạng hoặc thêm endpoint backend.
- **"Edit question"** trong menu, và tiêu đề card thành link tới chart, khớp `getDashcardHref`
  (`dashboard/selectors.ts:228-248`). Ẩn khi title override là `""` (đã ẩn header) và khi đang
  sửa.
- **Mô tả từng card** (`card.description` bên Metabase,
  `visualizations/lib/settings/visualization.ts:36-44`) hiện thành một dấu thông tin trên header
  card.
- **Công tắc `dashcard.background`** — một card có thể bỏ khung, như card heading của Metabase
  mặc định (`Heading/index.ts:11-40`). Card heading của egret vốn đã không khung
  (`HeadingCard.tsx`); phần này tổng quát hoá nó cho mọi card.

### W6 — Filter và tham số

- **"People can pick": nhiều giá trị / một giá trị.** egret cứng hoá theo loại
  (`FilterPill.tsx:87`); Metabase mở ra bất cứ khi nào cặp loại/toán tử nằm trong
  `SINGLE_OR_MULTI_SELECTABLE_TYPES` (`metabase-lib/v1/parameters/constants.ts:243-259`).
- **Kéo sắp xếp chip filter** (`ParametersList.tsx:56-64,90-104`; ngưỡng kích hoạt 15px). Hôm
  nay chúng hiện theo thứ tự tạo và không có cách đổi (`DashboardFilterBar.tsx:22-25`).
- **Auto-connect.** Khi nối filter vào một card, đề nghị nối nó vào mọi card có cột cùng tên —
  đúng lời văn và các mốc 12s / 8s của Metabase (`auto-wire-parameters/toasts.ts:26-91`,
  `constants.ts:1-2`). Đây là thứ tiết kiệm thời gian lớn nhất mà luồng filter của egret đang
  thiếu: hôm nay từng card phải nối tay.
- **Công tắc auto-apply + toast Apply.** Một thiết lập cấp dashboard; khi tắt, thay đổi filter
  ở dạng nháp cho tới khi bấm Apply (`FilterApplyToast.tsx:44-71`). egret hiện chỉ có auto-apply
  (`DashboardView.tsx:564`).
- **Panel linked filters** thêm phần khung của Metabase — tiêu đề "Limit this filter's choices",
  câu giải thích, và ba trạng thái bị chặn (không có tham số dùng được, nguồn giá trị tuỳ
  chỉnh, widget input box) mà hôm nay chỉ là một danh sách checkbox rỗng câm lặng
  (`ParameterLinkedFilters.tsx:52,93-103`).
- **Widget giá trị mặc định theo kiểu** thay ô text trần (`FilterSettingsPanel.tsx:100-105`), để
  mặc định kiểu ngày được chọn bằng bộ chọn ngày và mặc định kiểu category chọn từ danh sách.
- **`location` qua URL** — thêm `location` vào `ARRAY_TYPES` (§2.5).

### W7 — Tham số nội tuyến

Metabase có thể chuyển một filter khỏi thanh trên cùng vào trong một card heading hoặc question
(`MoveParameterMenu.tsx:67-134`, `supportsInlineParameters`), kèm hành vi thu gọn thành
"Show filters" khi card quá hẹp (`CollapsibleDashboardParameterList.tsx:40-86`).

Đây là phần đuôi của chương trình. Nó phụ thuộc mô hình sắp xếp của W6 và là workstream duy nhất
cắt được mà không để lại thứ gì làm dở.

### W8 — Sidebar thông tin và lịch sử phiên bản

- **Chủ sở hữu.** `DashboardView.tsx:691` truyền `dash.created_by` thô. Phân giải nó thành tên
  hiển thị và hiện cả ngày tạo lẫn ngày sửa cuối, như Metabase ("August 1, 2026 by Ref User").
- **Link "Saved in"** tới folder.
- **Lịch sử phiên bản.** Món duy nhất trong chương trình cần việc backend: một bảng revisions,
  chụp lại khi PUT dashboard, cộng endpoint list và revert, rồi tab History của Metabase với
  timeline và nút hoàn nguyên (`DashboardInfoSidebar.tsx:225-265`, `actions/revisions.ts:11-35`).

Xếp cuối để cắt được mà không bỏ rơi W1–W7.

### W9 — Card iframe

`createVirtualCard("iframe")` với danh sách URL cho phép (`IFrameViz.tsx:13-16`), tối thiểu 4×3
/ mặc định 12×8. Cột loại card của egret là text tự do không có CHECK
(`migrations/0015_dashboard.sql:21`), nên phần này chỉ ở frontend.

---

## 5. Những thứ cố ý không làm

**Card action button — bỏ khỏi phạm vi.** Card action của Metabase là giao diện cho hệ thống
model-actions/writeback của nó. egret không có: grep toàn repo cho `writeback`, `ModelAction` và
`implicit_action` trên `.rs`, `.ts`, `.tsx` không trả về gì. Làm cái card mà không có hệ thống
phía dưới thì được một cái nút không hành động được.

**Subscriptions, public link, embedding.** Vắng mặt ở cả backend lẫn frontend của egret. Chia sẻ
của egret dựa trên grant (`ShareDialog`, principal + role + hạn), và mô hình auth là uỷ quyền
token theo từng người, không phải link ẩn danh có chữ ký. Đây là quyết định sản phẩm, không phải
lỗ hổng.

**Visualizer / ghép nhiều nguồn trên một card.** Đã tranh luận và từ chối trong đợt chart:
đường lối của egret là một card dashboard được ghi đè *cách trình bày*, không bao giờ ghi đè
*dữ liệu*.

**Collections so với folders, moderation, caching chi tiết, Metabot/Insights.** Bị khoá sau bản
enterprise của Metabase, hoặc khác kiến trúc trong egret.

---

## 6. Chiến lược kiểm thử

Mỗi workstream đều có unit test, nhưng lịch sử của chính chương trình này rất rõ: **mọi lỗi có
ý nghĩa đều được tìm ra bằng cách đo app đang chạy, không phải bằng review và không phải bằng
bộ test.** Nên mỗi workstream còn kèm một lượt kiểm trực tiếp có ghi lại con số.

- **W1** — khẳng định chiều rộng lưới, chiều rộng card và `document.scrollWidth` ở 390px và
  1440px trên dashboard harness. Khẳng định ở mobile là `scrollWidth === innerWidth` và chiều
  rộng card sai lệch vài px so với khung; khẳng định ở desktop là layout đã lưu **không đổi**
  sau một lần ghé bằng mobile.
- **W2** — hộp thoại huỷ được kiểm bằng cách sửa nháp, bấm Cancel, rồi xác nhận thay đổi vẫn
  còn sau khi huỷ-cái-huỷ. Tương phản lưới kiểm bằng cách tính giá trị nét đã trộn so với nền ở
  cả hai theme, như §2.4, không bằng ảnh chụp.
- **W4/W5** — nội dung menu kiểm theo role và tên.
- **W6** — bản sửa `location` có test khứ hồi (ghi nhiều giá trị → đọc lại), vì lỗi chính là
  giá trị không sống sót qua khâu tuần tự hoá.

Test phải sống sót qua mutation. Luật nhà vẫn áp dụng: cắt cụt phần hiện thực và nhìn test đỏ
trước khi dám nói nó kiểm được cái gì. Trong đợt chart đã có sáu lần một khẳng định vẫn xanh khi
hiện thực bị cắt cụt; ba trong số đó là test đọc chuỗi *hiển thị* trong khi code khoá theo giá
trị *thô*.

---

## 7. Rủi ro

**W1 chạm vào app shell.** Breakpoint sidebar đổi mọi route, không riêng dashboard. Giảm thiểu:
thay đổi mang tính cộng thêm (một luật viewport ép thu dưới 768px) và lựa chọn localStorage vẫn
quyết định ở desktop, nên không hành vi desktop nào dịch chuyển.

**W6 là workstream lớn nhất và lộ ra nhiều nhất.** Riêng auto-connect ghi mapping lên nhiều card
cùng lúc. Nó bắt buộc phải hoàn tác được — Metabase ghép toast auto-connect với một toast hoàn
tác đi sau đúng vì lý do này (`toasts.ts:26-91`).

**W3 đổi một giá trị mặc định.** Dashboard mới thành `"fixed"`. Bản ghi cũ không bị đụng, nhưng
migration phải đọc kỹ: nó đổi *mặc định của cột*, không đổi các giá trị đang có.

---

## 8. "Xong" trông như thế nào

Dashboard harness, mở cạnh nhau ở 1440px và ở 390px, chỉ còn khác Metabase ở những mục liệt kê
tại §5. Cụ thể: nó xếp thành một cột trên điện thoại mà không cuộn ngang; Cancel không thể âm
thầm phá việc; một filter nối được vào hai mươi card mà không cần hai mươi thao tác chọn tay; và
mọi hành động cấp dashboard mà backend đã hiện thực đều có lối với tới ngay từ dashboard.

---

## 9. Những phát hiện không sống sót qua khâu soi kỹ

Ghi lại để không ai suy lại.

- **Hình học lưới không phải điểm lệch.** `dashboard-grid.ts:3-8` khớp Metabase chính xác — 24
  cột, tỉ lệ 10/9, margin 6px, container padding `[0,0]`, sàn chiều cao hàng 40px, và bảng kích
  thước mặc định/tối thiểu theo từng viz khớp `dashboards/constants.cljc:11-37` từng dòng. Card
  184px đo được trong khung 1136px quy ra đúng `size_x: 4`. Bản port là thật.
- **Thanh công cụ trên card lúc sửa không phải luôn hiện.** Nó trông như được vẽ thường trực
  trong DOM, nhưng khung chứa mang `opacity-0 transition-opacity group-hover:opacity-100` — cả
  hai bên đều hiện khi hover. egret đặt nó ở giữa trên, cưỡi lên mép card; Metabase đặt góc trên
  phải bên trong. Khác vị trí, không phải lỗi.
- **"Replace chart" xoá `options` không phải điểm lệch.** egret reset `options` về `{}`
  (`DashboardView.tsx:274-277`); Metabase cũng xoá series, `parameter_mappings` và viz settings
  (`cards-typed.ts:245-284`). Cả hai đều vứt. Cái egret thiếu là toast hoàn tác, và đó là lý do
  hành vi này cảm thấy tệ hơn — nằm ở W2, không phải ở việc đổi cách reset.
- **Các mức auto-refresh vốn đã trùng.** Off/1/5/10/15/30/60 phút ở cả hai bên. Chỉ khác cái
  control (`<select>` gốc so với popover), và đó là W4.
- **Các layout section vốn đã trùng.** Cả hai đều có ba layout dựng sẵn với cùng hình dạng
  (`dashboard-sections.ts:36-77` so với `dashboard/sections.ts:202-224`).
