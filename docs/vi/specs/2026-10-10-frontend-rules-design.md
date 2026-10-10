# Rule frontend: một bộ rule cho UI của egret, ép buộc ở chỗ máy kiểm được

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-10-frontend-rules-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-10; chưa cài.
**Áp dụng cho:** mọi thứ dưới `frontend/`, bắt đầu từ W7 (UI workflow). Code cũ theo rule khi được chạm tới (§6).
**Liên quan:** `docs/{en,vi}/ui-conventions.md` (lý do của các quyết định trước; file được giữ và trở thành phần giải thích lý do mà bộ rule trỏ tới).

**Đã quyết với người dùng, 2026-10-10:**
- **Ép buộc: bộ rule cộng lint.** Rule nào máy kiểm chắc được thì thành lỗi ESLint. Code đang vi phạm được liệt kê thành nợ có tên, và code mới không được làm nợ tăng. Không migrate hàng loạt trước W7.
- **Cấu trúc: một bộ rule ngắn mới, giữ tài liệu cũ.** `docs/en/frontend-rules.md` là nguồn chuẩn. `docs/en/ui-conventions.md` là phần giải thích lý do và ghi các quyết định đã đảo, không còn là nguồn rule.
- **Chữ trên UI bằng tiếng Anh, không i18n.** Một văn phong chung (§4.9). Dữ liệu bản đồ và khu vực bằng tiếng Việt giữ nguyên.
- **Sản phẩm tham chiếu: visual của egret, UX của sản phẩm gốc.** Mọi khu vực dùng token, component và khung trang của egret. Cấu trúc và luồng thao tác của một khu vực chép sản phẩm gốc của khu vực đó, đối chiếu trên bản đang chạy: BI theo Metabase, Workflow theo Kestra. Chart giữ palette series của Metabase.
- **Danh mục rule (§4) như đã trình bày lúc brainstorm,** chấp nhận các lựa chọn có dấu sao:
  - tạo mới chia giữa trang và dialog theo §3 của `ui-conventions.md`;
  - skeleton cho nội dung đang tải;
  - form vẫn tự viết;
  - hook của orval cộng một factory query key cho mỗi domain;
  - `React.lazy` cho mọi khu vực tính năng mới.

- **(2026-10-10, phán quyết của người dùng ở Task 2) `jsx-a11y/no-autofocus` tắt trong lint và trở thành FE-A11Y-6, do review kiểm.** Rule này báo trên 41 file, gần như toàn bộ là `autoFocus` có chủ ý trên ô đầu tiên của dialog hoặc ô đổi tên tại chỗ, tức focus chuyển theo chính thao tác của người dùng; rule lint không phân biệt được điều đó với focus bị giật lúc tải trang.

## 1. Vì sao

Khảo sát `frontend/src` ngày 2026-10-10 cho thấy nền tốt nhưng chưa có một chuẩn chung:
- **Token và primitive tốt:** token OKLCH, primitive shadcn/Radix, các phần dùng chung cho trang danh sách.
- **Hai khung trang.** `ListPageShell` phục vụ Explore và Data API. Control plane và governance tự viết khung, với khoảng 49 thẻ `<h1>` ở cấp route.
- **Ba kiểu bảng:** `FilterableTable`/`ui/table`, TanStack Table trong `ChartTable`, và khoảng 9 `<table>` tự viết.
- **Ba kiểu loading:** skeleton, chữ "Loading…" và spinner.
- **Hai kiểu trạng thái rỗng:** `EmptyState` ở khoảng 21 chỗ, và khoảng 53 dòng chữ "No X" tự viết.
- **Màu thô:** khoảng 35 class màu Tailwind thô và vài giá trị hex tuỳ ý đi vòng qua token.
- **Xác nhận phá huỷ:** `window.confirm` song song với `ConfirmDeleteDialog`.
- **Query key:** hai tiền tố (`cp`, `control-plane`) cho cùng một domain.
- **Không tách code theo route.**
- **Không có lint a11y.**

`ui-conventions.md` ghi vì sao nhiều quyết định trước được đưa ra, nhưng đó là 2.225 dòng lịch sử xoay quanh Data API. Agent không dùng nó làm checklist được, và bản tiếng Việt đã lệch (662 dòng). W7 sắp thêm cả một khu vực tính năng, và nó nên được xây theo một chuẩn viết thành văn.

## 2. Sản phẩm bàn giao

1. **`docs/en/frontend-rules.md` (bản chính) và `docs/vi/frontend-rules.md` (bản tham khảo).** Hai file này gồm danh mục rule ở §4 theo định dạng §3, checklist cho reviewer ở §5 và sổ nợ ở §6. Bản tiếng Anh dưới khoảng 400 dòng để agent đọc trọn.
2. **File trỏ đường**, mỗi file vài dòng, bảo agent đọc bộ rule trước khi sửa bất cứ thứ gì dưới `frontend/`:
   - `frontend/CLAUDE.md`, mà Claude Code tự nạp khi làm việc trong thư mục đó;
   - `frontend/AGENTS.md`, cho các agent khác.
3. **`docs/{en,vi}/ui-conventions.md`:** đầu mỗi file thêm một dòng nói bộ rule là chuẩn và file này là phần lý do. Không đổi gì khác trong file. Độ lệch của bản tiếng Việt được ghi thành nợ.
4. **Lint** (§7):
   - một plugin ESLint cục bộ chứa các rule riêng của egret;
   - `eslint-plugin-jsx-a11y-x` (bản fork còn bảo trì của `eslint-plugin-jsx-a11y`);
   - một danh sách legacy cho mỗi rule;
   - một bước kiểm để mọi danh sách legacy luôn đúng.
5. **Xoá `frontend/src/theme/egret-theme.ts`.** Đó là một palette hex màu cam đã cũ, không ai import; các chỗ khớp `egret-theme` duy nhất là key localStorage của theme provider.

## 3. Định dạng một rule

Mỗi rule có:
- **mã**: `FE-<NHÓM>-<n>`, cố định mãi, và không bao giờ dùng lại sau khi bị bỏ;
- **mức**: **PHẢI** (MUST) hoặc **NÊN** (SHOULD). Vi phạm một rule NÊN cần một dòng lý do trong PR;
- **rule**, trong một hoặc hai câu;
- **làm thế này**: component, hook hay file có thật cần dùng, theo đường dẫn;
- **kiểm bằng**: `lint (egret/<rule>)`, `lint (jsx-a11y)`, `test`, `browser` hoặc `review`;
- **vì sao** (tuỳ chọn): link tới mục của `ui-conventions.md` hoặc sự cố đứng sau rule.

Rule nhắc tên một component thì ghi file chứa nó. Nếu component chuyển chỗ, bộ rule đổi trong cùng PR.

## 4. Danh mục rule

**Mức.** Mọi rule dưới đây là PHẢI, trừ khi ghi NÊN.

**Cách kiểm.** Rule chỉ ghi cách kiểm khi đó là lint, test hay trình duyệt; mọi rule khác do review kiểm.

### 4.1 Khung trang (FE-PAGE)
1. Trang danh sách dùng `ListPageShell` (`src/components/explore/ListPageShell.tsx`), và shell này giữ thẻ `h1` duy nhất của trang.
   - Thứ tự: tiêu đề, rồi toolbar (`ListToolbarButton`), rồi `SummaryStrip`, rồi bảng (`ui-conventions.md` §1).
2. Trang chi tiết có nhiều phần dùng `ui/tabs`. Tab đang chọn, cùng mọi filter hay lựa chọn mà người dùng muốn chia sẻ, nằm trong URL (`?tab=`, `?q=`). Nút Back đi lùi qua chúng.
3. **Tạo mới:**
   - Tạo trên một trang (`…/new`, khai báo trước `:id`) khi form có nhiều phần, hoặc khi kết quả phải được đọc trước khi rời trang (secret chỉ hiện một lần).
   - Tạo trong dialog khi chỉ có một đến ba trường.
   - Lối vào là link, không phải lệnh điều hướng (`ui-conventions.md` §3).
4. Danh sách dạng bảng dùng `FilterableTable` (`src/components/explore/FilterableTable.tsx`) hoặc `ui/table`.
   - `<table>` tự viết chỉ được phép cho grid đặc thù (pivot, kết quả SQL), và kèm một comment giải thích.
5. Phần cấu hình dùng `ConfigTable`/`ConfigTableRow`/`ConfigSection` (`src/components/config/`), theo dạng `Setting | Details | Value` (`ui-conventions.md` §4).
   - Một tập thuộc tính phẳng là một danh sách key/value tìm kiếm được, không làm form riêng cho từng field.

### 4.2 Trạng thái (FE-STATE)
1. Mọi khung hiển thị dữ liệu đều vẽ trạng thái loading, rỗng và lỗi, và kiểm `isError` trước khi kiểm có dữ liệu. Lỗi không bao giờ hiển thị thành rỗng (`ui-conventions.md` §2).
2. Nội dung trang hay danh sách đang tải vẽ skeleton (`ListStates`, `ListSkeleton`, `ui/skeleton`).
   - Spinner (`Loader2`) chỉ dành cho nút hoặc thao tác tại chỗ.
   - Chữ trơn "Loading…" không phải là trạng thái loading.
3. Trạng thái rỗng vẽ `EmptyState` (`src/components/explore/EmptyState.tsx`).
   - "Rỗng" nghĩa là tập dữ liệu khi chưa lọc không có gì.
   - Filter không khớp gì thì nói đúng như vậy và cho xoá filter.
4. Chỉ báo trạng thái ("đã sửa", "khác bản đang chạy", "đã cũ") so sánh chính nội dung nó mô tả. Chỉ dùng timestamp khi không có nội dung, và khi đó liệt kê mọi nơi ghi timestamp ấy trong một comment.

### 4.3 Thao tác và phản hồi (FE-ACTION)
1. Hành động phá huỷ được xác nhận bằng `ConfirmDeleteDialog` (`src/components/ui/confirm-delete-dialog.tsx`) hoặc `ui/alert-dialog`. Kiểm bằng lint (`egret/no-window-confirm`).
2. Khi thành công, mutation invalidate những query nó làm thay đổi.
   - Nếu điều hướng tới kết quả thì không toast. Nếu ở lại trang thì một toast xác nhận (`ui-conventions.md` §3.6).
3. Khi thất bại, mutation hiện thông báo của server, bằng toast hoặc ngay tại chỗ, và không xoá gì người dùng đã nhập (`ui-conventions.md` §3.7).
4. Control bị vô hiệu hoá phải nói vì sao, bằng tooltip hoặc chữ bên cạnh.
5. Form có chỉnh sửa chưa lưu dùng `useUnsavedChangesWarning` (`src/lib/useUnsavedChangesWarning.ts`).

### 4.4 Màu, chữ, khoảng cách (FE-VISUAL)
1. **Màu lấy từ token** (`src/index.css`): `bg-primary`, `text-muted-foreground`, `bg-highlight`, `text-success`, `text-destructive` và các token khác định nghĩa ở đó.
   - Không dùng class màu Tailwind thô (`bg-green-600`).
   - Không dùng màu tuỳ ý (`bg-[#0af]`, `text-[oklch(...)]`).
   - Kiểm bằng lint (`egret/no-raw-palette`).
2. Trong style inline, token viết là `var(--token)`, không bao giờ `hsl(var(--token))`. Token không đi xuyên vào SVG data URL được, nên phải đọc giá trị trước bằng `getComputedStyle`.
3. **Cỡ chữ lấy từ thang có sẵn:** `text-xs`, `text-sm`, `text-base`, `text-lg`, `text-xl` và `text-2xl`. Không dùng `text-[Npx]` tuỳ ý. Kiểm bằng lint (`egret/no-arbitrary-text-size`).
4. **Số và giao diện sáng/tối:**
   - Cột số, số đếm hay số đo dùng `tabular-nums`.
   - Mọi màn hình đúng ở cả light lẫn dark; thay đổi màu được kiểm trên cả hai, và trên mọi bề mặt mà màu đó xuất hiện.
   - Kiểm trên trình duyệt.
5. **NÊN:** icon lấy từ `lucide-react`, và mỗi trang có tối đa một điểm nhấn `--highlight`. `--accent` là bề mặt hover/focus, không dùng làm màu nhấn.
6. Chart giữ palette series của Metabase (`src/components/chart/echarts/palette.ts`) cùng các dấu hiệu không phụ thuộc màu: kiểu đường, marker và decal. Giá trị màu cụ thể bên trong code dựng option của chart được phép, vì ECharts cần giá trị cụ thể.

### 4.5 Form (FE-FORM)
1. Form giữ state bằng `useState` hoặc một hook riêng (như `useEngineConfigForm`). Không thêm thư viện form.
2. Mọi input có một `Label` gắn qua `htmlFor`/`id`. Kiểm bằng lint (`jsx-a11y`).
3. Khi tập giá trị hợp lệ đã biết, control là picker hoặc combobox (chọn hoặc gõ) có trợ giúp, không bao giờ là ô nhập chữ trơn.
4. Lỗi validate hiện cạnh field của nó, bằng chính lời của server nếu lỗi do server trả về.

### 4.6 Dữ liệu và API (FE-DATA)
1. API client được sinh lại bằng `npm run gen:api`, không bao giờ bằng `orval` trơn, vì nó bỏ mất bước encode tham số đường dẫn.
2. **Endpoint và query key:**
   - Endpoint mới được dùng qua hook do orval sinh ra.
   - Query key tự viết lấy từ một factory key theo domain (`queryKeys.ts` trong thư mục của domain, như `src/routes/control-plane/queryKeys.ts`), có tiền tố là tên domain.
3. Dữ liệu từ server không bao giờ được cache trong `localStorage`. `localStorage` chỉ giữ tuỳ chọn của từng người xem (theme, một panel đang thu gọn), và mọi lần truy cập đều được bọc để trang vẫn chạy khi không có nó.

### 4.7 Routing (FE-ROUTE)
1. Route được khai báo trong `src/App.tsx`. Mỗi route được bọc trong guard mà khu vực của nó cần (`RequireFeature`, `RequirePermission`, `RequireGlobalAdmin`, trong `src/auth/guards.tsx`). Redirect dùng đường dẫn tuyệt đối.
2. Mọi khu vực tính năng mới, bắt đầu từ Workflow, được tách code bằng `React.lazy` và `Suspense`, với fallback là skeleton. Kiểm bằng một bundle test kiểu `test:bundle:monaco`.
3. URL viết kebab-case và thuộc workspace, dưới `/explore/:workspace/…`.

### 4.8 Truy cập (FE-A11Y)
1. Code qua được bộ rule recommended của `eslint-plugin-jsx-a11y`. Kiểm bằng lint.
2. Focus dùng ring chung (`focus-visible:ring-2 focus-visible:ring-ring`). Chỉ báo focus đạt tương phản 3:1 trên mọi bề mặt nó xuất hiện; `src/__tests__/focusIndicatorContrast.test.ts` giữ điều này.
3. Mọi phần tử bấm được đều dùng được bằng bàn phím. Phần tử không phải nút mà được cho bấm thì có role, `tabIndex` và xử lý Enter/Space. Nút chỉ có icon thì có `aria-label`.
4. Thông tin không bao giờ chỉ truyền bằng màu; chart giữ decal và marker của nó (FE-VISUAL-6).
5. Chuyển động tôn trọng `prefers-reduced-motion`.
6. `autoFocus` chỉ dành cho ô xuất hiện do chính thao tác của người dùng (ô đầu tiên của dialog, sửa tại chỗ), không bao giờ lúc tải trang hay đổi route. *(2026-10-10, phán quyết của người dùng: `jsx-a11y/no-autofocus` tắt, vì nó không phân biệt được hai trường hợp.)*

### 4.9 Chữ trên UI (FE-COPY)
1. Chữ trên UI bằng tiếng Anh.
   - Tiêu đề, nút và mục menu viết hoa kiểu câu ("Add a chart").
   - Nút bắt đầu bằng động từ.
   - Cùng một thứ thì gọi cùng một tên ở mọi nơi.
2. Thông báo lỗi nói chuyện gì đã xảy ra và người dùng có thể làm gì. Không bao giờ hiện exception thô hay stack trace.
3. Chữ nhắc tên một control ("click Publish") là một khẳng định rằng control đó tồn tại đúng tên ấy, và có test kiểm (`ui-conventions.md` §3.9).
4. Số nhiều do trang tự viết ra (`singular` của `SummaryStrip`), không ghép thêm "s" trong code dùng chung.

### 4.10 Sản phẩm tham chiếu (FE-REF)
1. **Hệ visual là của egret.** Cấu trúc và luồng thao tác chép sản phẩm gốc của khu vực đó:
   - BI (chart, dashboard, bảng, panel cấu hình): Metabase, kể cả cách chia tab và dạng dòng của panel cấu hình;
   - Workflow: Kestra.
2. Việc so sánh làm trên một bản đang chạy của sản phẩm gốc, không theo trí nhớ. Mọi khác biệt có chủ ý được ghi kèm lý do vào spec của package.

### 4.11 Kiểm chứng (FE-TEST)
1. **Test:**
   - Test render component thật và kiểm qua DOM, ưu tiên `getByRole`.
   - Test không chép lại biểu thức mà nó đang kiểm.
   - Fixture phải phân biệt được hành vi đúng với một hành vi sai trông hợp lý; ví dụ, không sắp sẵn thứ tự khi thứ tự là điều cần kiểm.
2. Thay đổi về layout, chiều rộng, chiều cao, màu hay bất kỳ kết quả hiển thị nào được kiểm trên trình duyệt thật: Playwright, hoặc đo DOM bằng `getBoundingClientRect` và `getComputedStyle`. jsdom không có layout, và đọc code không kết luận được về layout (`ui-conventions.md` §10).
3. Component dùng chung chỉ được sửa theo kiểu thêm vào. Phạm vi ảnh hưởng đo theo đường dẫn import, không theo tên symbol (`ui-conventions.md` §7).
4. Cổng kiểm cho mọi thay đổi frontend:
   - `npm run lint`
   - `npx tsc -b`
   - `npm run test`
   - `npm run build`
   - các bundle test mà thay đổi chạm tới

## 5. Checklist cho reviewer

Bộ rule kết thúc bằng một checklist gồm những rule mà lint không kiểm được:
- FE-PAGE-1 đến 5
- FE-STATE-1 đến 4
- FE-ACTION-2 đến 5
- FE-VISUAL-2 và 4 đến 6
- FE-FORM-1, 3 và 4
- FE-DATA-1 đến 3
- FE-ROUTE-1 và 3
- FE-A11Y-3 đến 6
- FE-COPY-1, 2 và 4
- FE-REF-1 và 2
- FE-TEST-1 đến 3

Mỗi lần review một task frontend trả lời từng mục áp dụng được: đạt, không đạt, hoặc không áp dụng.

## 6. Sổ nợ

Mục cuối của bộ rule liệt kê mọi chỗ đã biết đang vi phạm rule. Mỗi mục ghi mã rule, các file và một con số đếm lúc viết bộ rule. Chính sách cho mọi mục như nhau: **sửa khi chạm tới**. PR nào sửa một file có trong sổ thì đưa file đó về đúng rule, hoặc giải thích trong mô tả PR vì sao chưa làm được. Các mục đã biết:

| Rule | Nợ (khảo sát 2026-10-10; đếm lại ở task đầu của plan) |
|---|---|
| FE-PAGE-1 | trang danh sách của control-plane và governance tự viết khung (~49 thẻ `<h1>` cấp route) |
| FE-PAGE-4 | ~9 `<table>` tự viết ngoài các grid đặc thù |
| FE-STATE-2 | ~28 dòng chữ "Loading…"; spinner dùng cho nội dung |
| FE-STATE-3 | ~53 dòng chữ rỗng "No X" tự viết, ~12 khối viền đứt |
| FE-ACTION-1 | `window.confirm` trong `DatasetDetail.tsx` và `DatabasesPage.tsx` |
| FE-VISUAL-1 | ~35 class màu thô trong ~12 file; gradient thô của variant `ai` của Button |
| FE-VISUAL-3 | ~33 cỡ chữ tuỳ ý `text-[Npx]` |
| FE-DATA-2 | hai tiền tố key `cp` và `control-plane` cho cùng một domain |
| FE-ROUTE-2 | không route nào trong `App.tsx` được lazy-load |
| FE-A11Y-1 | những gì `jsx-a11y` báo ở lần chạy đầu (đếm trong plan) |
| tài liệu | `docs/vi/ui-conventions.md` đã lệch so với bản tiếng Anh |

Mục kiểm bằng lint chính là danh sách legacy của rule đó (§7). Mục kiểm bằng review là một dòng trong bảng này.

## 7. Lint

1. **Một plugin cục bộ, `frontend/eslint-rules/`**, đăng ký với tên `egret` trong `eslint.config.js`. Mỗi rule có test `RuleTester` chạy trong vitest:
   - **`egret/no-raw-palette`** báo một chuỗi hoặc template literal có chứa:
     - một utility màu Tailwind dùng màu trong palette, `(bg|text|border|ring|outline|fill|stroke|from|via|to|divide|placeholder|caret|accent|decoration|shadow)-(slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-(50|[1-9]00|950)`, có thể kèm tiền tố variant và hậu tố độ mờ;
     - hoặc một giá trị màu tuỳ ý, `-[#…]`, `-[rgb(…)]`, `-[hsl(…)]` hay `-[oklch(…)]`.
     Rule không áp dụng dưới `src/components/chart/echarts/` (FE-VISUAL-6).
   - **`egret/no-arbitrary-text-size`** báo `text-[<số><đơn vị>]`.
   - **`egret/no-window-confirm`** báo `window.confirm(…)` và `confirm(…)` trơn khi nó trỏ tới hàm global.
2. **`eslint-plugin-jsx-a11y-x`**, với cấu hình `recommended`, ở mức error. *(Sửa 2026-10-10, theo plan: đây là bản fork còn được bảo trì của `eslint-plugin-jsx-a11y`, cùng bộ rule. Bản gốc 6.10.2, phát hành lần cuối năm 2024, chỉ khai báo hỗ trợ tới ESLint 9, còn egret chạy ESLint 10. Phần settings `jsx-a11y` của fork ánh xạ các primitive của egret (`Button`, `Input`, `Label`, `Textarea`) sang thẻ HTML tương ứng.)*
3. **Danh sách legacy.**
   - Mỗi rule có danh sách riêng gồm các file vi phạm nó lúc rule được đưa vào: `frontend/eslint-rules/legacy.js`, mỗi mã rule một mục, cho cả `egret/*` lẫn `jsx-a11y/*`.
   - `eslint.config.js` tắt đúng rule đó cho đúng các file ấy.
   - Một file không bao giờ được thêm vào danh sách sau khi danh sách đã được tạo.
4. **Danh sách luôn đúng.** Một script, `npm run lint:legacy`, chạy mỗi rule không kèm danh sách legacy trên các file được liệt kê. Script thất bại khi một file trong danh sách không còn vi phạm rule của nó, nên file đã sửa phải ra khỏi danh sách. Script chạy trong cổng kiểm: `npm run lint` chạy ESLint rồi chạy script này (FE-TEST-4).
5. Mọi rule này đều ở mức error. Các rule React Compiler mà `eslint.config.js` đặt ở mức warn vẫn giữ nguyên; chúng nằm ngoài package này.

## 8. Kiểm thử

- **Các ca `RuleTester` cho mỗi rule egret:**
  - hợp lệ: class token, giá trị màu trong code dựng chart ở đường dẫn được miễn, `text-sm`, `confirm` là một biến cục bộ;
  - không hợp lệ: từng mẫu ở §7.1, gồm cả tiền tố variant (`dark:bg-red-500`), hậu tố độ mờ (`bg-sky-500/20`) và template literal.
- **`lint:legacy`:** một test cho chính script đó, với một file fixture vi phạm và một file không vi phạm.
- **`npm run lint` xanh trên toàn bộ cây** khi có các danh sách legacy, và đỏ khi thêm một vi phạm vào file không nằm trong danh sách. Plan kiểm điều này bằng cách tạm thêm một vi phạm vào một file ngoài danh sách.
- **Các file trỏ đường:** người đọc trong `frontend/` tới được bộ rule sau một bước; plan kiểm từng đường dẫn link có tồn tại.
- **Việc xoá `egret-theme.ts`:** `npm run build` và `tsc` xanh.

## 9. Ngoài phạm vi

- Migrate code cũ cho đúng rule, ngoài việc xoá `egret-theme.ts`. Nợ được trả khi file được chạm tới (§6).
- i18n, prettier, và đống warning tồn đọng của React Compiler.
- Chuyển sang data router (cần cho việc chặn điều hướng trong app).
- Viết lại `ui-conventions.md` hay đồng bộ lại bản tiếng Việt của nó.
- Công cụ so sánh ảnh chụp (visual regression).
