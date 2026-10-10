# Rule frontend của egret

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/frontend-rules.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

Các rule mà mọi thay đổi dưới `frontend/` phải theo. File này là chuẩn; [`ui-conventions.md`](ui-conventions.md) giữ lý do đằng sau chúng và ghi lại các quyết định đã đảo. Các liên kết "Vì sao" bên dưới trỏ vào bản tiếng Anh của `ui-conventions.md`, vì bản tiếng Việt đã lệch (xem sổ nợ).

**Cách đọc.**
- Mỗi rule là **PHẢI** (MUST) hoặc **NÊN** (SHOULD). Vi phạm một rule NÊN cần một dòng lý do trong PR.
- Mã rule (`FE-<NHÓM>-<n>`) cố định mãi mãi và không bao giờ dùng lại khi đã bỏ.
- **Kiểm bằng** cho biết cái gì ép rule: rule `lint` làm `npm run lint` thất bại; rule `test` và `trình duyệt` được kiểm theo cách đó; rule `review` được trả lời trong [checklist cho reviewer](#checklist-cho-reviewer).
- Rule nào nêu tên một component thì nêu file chứa nó. Nếu component chuyển chỗ, file này đổi trong cùng PR.
- Code vi phạm một rule từ trước khi rule có mặt được liệt kê trong [sổ nợ](#sổ-nợ).

## 1. Khung trang (FE-PAGE)

#### FE-PAGE-1 — Trang danh sách dùng `ListPageShell` · PHẢI
Trang danh sách dùng `ListPageShell`, và shell này giữ thẻ `h1` duy nhất của trang. Thứ tự: tiêu đề, rồi toolbar (`ListToolbarButton`), rồi `SummaryStrip`, rồi bảng.
**Làm thế này:** `frontend/src/components/explore/ListPageShell.tsx`, `frontend/src/components/explore/ListToolbarButton.tsx`, `frontend/src/components/explore/SummaryStrip.tsx`. **Kiểm bằng:** review. **Vì sao:** [ui-conventions §1](../en/ui-conventions.md#1-the-list-page).

#### FE-PAGE-2 — Phần chi tiết là tab, trạng thái chia sẻ được nằm trong URL · PHẢI
Trang chi tiết có nhiều phần dùng `ui/tabs`. Tab đang chọn, cùng mọi filter hay lựa chọn mà người dùng muốn chia sẻ, nằm trong URL (`?tab=`, `?q=`). Nút Back đi lùi qua chúng.
**Làm thế này:** `frontend/src/components/ui/tabs.tsx`. **Kiểm bằng:** review.

#### FE-PAGE-3 — Tạo trên trang hay trong dialog tuỳ kích thước · PHẢI
- Tạo trên một trang (`…/new`, khai báo trước `:id`) khi form có nhiều phần, hoặc khi kết quả phải được đọc trước khi rời trang (secret chỉ hiện một lần).
- Tạo trong dialog khi chỉ có một đến ba trường.
- Lối vào là link, không phải lệnh điều hướng.

**Làm thế này:** trang: `frontend/src/routes/explore/dataapi/EndpointCreate.tsx` (khai báo là `data-api/endpoints/new` trong `frontend/src/App.tsx`); dialog: `frontend/src/components/explore/FolderNameDialog.tsx`. **Kiểm bằng:** review. **Vì sao:** [ui-conventions §3](../en/ui-conventions.md#3-create-flows-when-a-page-when-a-dialog).

#### FE-PAGE-4 — Danh sách dạng bảng dùng bảng dùng chung · PHẢI
Danh sách dạng bảng dùng `FilterableTable` hoặc `ui/table`. `<table>` tự viết chỉ được phép cho grid đặc thù (pivot, kết quả SQL), và kèm một comment giải thích.
**Làm thế này:** `frontend/src/components/explore/FilterableTable.tsx`, `frontend/src/components/ui/table.tsx`. **Kiểm bằng:** review.

#### FE-PAGE-5 — Cấu hình là bảng `Setting | Details | Value` · PHẢI
Phần cấu hình dùng `ConfigTable`/`ConfigTableRow`/`ConfigSection`, theo dạng `Setting | Details | Value`. Một tập thuộc tính phẳng là một danh sách key/value tìm kiếm được, không làm form riêng cho từng field.
**Làm thế này:** `frontend/src/components/config/ConfigTable.tsx` (`ConfigTable`, `ConfigTableRow`), `frontend/src/components/form/ConfigSection.tsx`. **Kiểm bằng:** review. **Vì sao:** [ui-conventions §4](../en/ui-conventions.md#4-setting--details--value-tables-and-when-a-card-is-still-right).

## 2. Trạng thái (FE-STATE)

#### FE-STATE-1 — Loading, rỗng và lỗi, kiểm lỗi trước · PHẢI
Mọi khung hiển thị dữ liệu đều vẽ trạng thái loading, rỗng và lỗi, và kiểm `isError` trước khi kiểm có dữ liệu. Lỗi không bao giờ hiển thị thành rỗng.
**Làm thế này:** `ListStates` (`frontend/src/components/explore/ListStates.tsx`), rẽ nhánh loading, lỗi, rỗng, dữ liệu theo đúng thứ tự đó. **Kiểm bằng:** review. **Vì sao:** [ui-conventions §2](../en/ui-conventions.md#2-loading-empty-error--three-states-never-two).

#### FE-STATE-2 — Nội dung đang tải là skeleton · PHẢI
Nội dung trang hay danh sách đang tải vẽ skeleton. Spinner (`Loader2`) chỉ dành cho nút hoặc thao tác tại chỗ. Chữ trơn "Loading…" không phải là trạng thái loading.
**Làm thế này:** `ListStates` (`frontend/src/components/explore/ListStates.tsx`), `ListSkeleton` (`frontend/src/components/explore/ListSkeleton.tsx`), `frontend/src/components/ui/skeleton.tsx`. **Kiểm bằng:** review.

#### FE-STATE-3 — Rỗng là `EmptyState`, filter không khớp thì nói vậy · PHẢI
Trạng thái rỗng vẽ `EmptyState`. "Rỗng" nghĩa là tập dữ liệu khi chưa lọc không có gì. Filter không khớp gì thì nói đúng như vậy và cho xoá filter.
**Làm thế này:** `frontend/src/components/explore/EmptyState.tsx`. **Kiểm bằng:** review. **Vì sao:** [ui-conventions §2](../en/ui-conventions.md#2-loading-empty-error--three-states-never-two).

#### FE-STATE-4 — Chỉ báo trạng thái so sánh nội dung · PHẢI
Chỉ báo trạng thái ("đã sửa", "khác bản đang chạy", "đã cũ") so sánh chính nội dung nó mô tả. Chỉ dùng timestamp khi không có nội dung, và khi đó liệt kê mọi nơi ghi timestamp ấy trong một comment.
**Làm thế này:** ví dụ: `frontend/src/routes/explore/dataapi/endpointState.tsx` (so bản nháp với định nghĩa đang phục vụ). **Kiểm bằng:** review.

## 3. Thao tác và phản hồi (FE-ACTION)

#### FE-ACTION-1 — Hành động phá huỷ dùng dialog xác nhận · PHẢI
Hành động phá huỷ được xác nhận bằng `ConfirmDeleteDialog` hoặc `ui/alert-dialog`. Không bao giờ dùng `window.confirm`.
**Làm thế này:** `frontend/src/components/ui/confirm-delete-dialog.tsx`, `frontend/src/components/ui/alert-dialog.tsx`. **Kiểm bằng:** lint (`egret/no-window-confirm`).

#### FE-ACTION-2 — Thành công thì invalidate; chỉ toast khi ở lại · PHẢI
Khi thành công, mutation invalidate những query nó làm thay đổi. Nếu điều hướng tới kết quả thì không toast. Nếu ở lại trang thì một toast xác nhận.
**Làm thế này:** ví dụ: `frontend/src/routes/explore/dataapi/EndpointCreate.tsx` (invalidate, điều hướng, không toast); `frontend/src/routes/explore/dataapi/detail/AccessSection.tsx` (invalidate, ở lại, có toast). **Kiểm bằng:** review. **Vì sao:** [ui-conventions §3.6](../en/ui-conventions.md#36-on-success-invalidate-and-no-toast--navigate-unless-the-response-is-the-point).

#### FE-ACTION-3 — Thất bại thì hiện lời server và không xoá gì · PHẢI
Khi thất bại, mutation hiện thông báo của server, bằng toast hoặc ngay tại chỗ, và không xoá gì người dùng đã nhập.
**Làm thế này:** ví dụ: `frontend/src/routes/explore/dataapi/EndpointCreate.tsx` (lời của server hiện ngay tại chỗ, mọi field giữ nguyên). **Kiểm bằng:** review. **Vì sao:** [ui-conventions §3.7](../en/ui-conventions.md#37-on-failure-clear-nothing).

#### FE-ACTION-4 — Control bị vô hiệu hoá nói vì sao · PHẢI
Control bị vô hiệu hoá phải nói vì sao, bằng tooltip hoặc chữ bên cạnh.
**Làm thế này:** `frontend/src/components/ui/tooltip.tsx`. **Kiểm bằng:** review.

#### FE-ACTION-5 — Chỉnh sửa chưa lưu thì cảnh báo · PHẢI
Form có chỉnh sửa chưa lưu dùng `useUnsavedChangesWarning`.
**Làm thế này:** `frontend/src/lib/useUnsavedChangesWarning.ts`. **Kiểm bằng:** review.

## 4. Màu, chữ, khoảng cách (FE-VISUAL)

#### FE-VISUAL-1 — Màu lấy từ token · PHẢI
Dùng các class token định nghĩa trong `frontend/src/index.css` (`bg-primary`, `text-muted-foreground`, `bg-highlight`, `text-success`, `text-destructive` và các token khác định nghĩa ở đó). Không dùng class màu Tailwind thô (`bg-green-600`). Không dùng màu tuỳ ý (`bg-[#0af]`, `text-[oklch(...)]`).
**Làm thế này:** `className="bg-primary text-primary-foreground"`, với token trong `frontend/src/index.css`. **Kiểm bằng:** lint (`egret/no-raw-palette`; không áp dụng dưới `frontend/src/components/chart/echarts/`, xem FE-VISUAL-6).

#### FE-VISUAL-2 — Style inline dùng `var(--token)` · PHẢI
Trong style inline, token viết là `var(--token)`, không bao giờ `hsl(var(--token))`. Token không đi xuyên vào SVG data URL được, nên phải đọc giá trị trước bằng `getComputedStyle`.
**Làm thế này:** `style={{ color: "var(--primary)" }}`, như trong `frontend/src/components/access/EffectBadge.tsx`; với SVG data URL, `frontend/src/components/chart/dashboard/grid-background.ts`. **Kiểm bằng:** review.

#### FE-VISUAL-3 — Cỡ chữ lấy từ thang có sẵn · PHẢI
Dùng `text-xs`, `text-sm`, `text-base`, `text-lg`, `text-xl` và `text-2xl`. Không dùng `text-[Npx]` tuỳ ý.
**Làm thế này:** `className="text-sm"`; định nghĩa của lint là `frontend/eslint-rules/rules/no-arbitrary-text-size.js`. **Kiểm bằng:** lint (`egret/no-arbitrary-text-size`).

#### FE-VISUAL-4 — Số dạng bảng; đúng ở cả sáng lẫn tối · PHẢI
Cột số, số đếm hay số đo dùng `tabular-nums`. Mọi màn hình đúng ở cả light lẫn dark; thay đổi màu được kiểm trên cả hai, và trên mọi bề mặt mà màu đó xuất hiện.
**Làm thế này:** ví dụ: `frontend/src/components/chart/pivot/PivotTable.tsx` (`tabular-nums` trên ô số). **Kiểm bằng:** trình duyệt.

#### FE-VISUAL-5 — Icon Lucide; một điểm nhấn mỗi trang · NÊN
Icon lấy từ `lucide-react`, và mỗi trang có tối đa một điểm nhấn `--highlight`. `--accent` là bề mặt hover/focus, không dùng làm màu nhấn.
**Làm thế này:** icon như trong `frontend/src/components/explore/RowActionsMenu.tsx`; `--highlight` và `--accent` định nghĩa trong `frontend/src/index.css`. **Kiểm bằng:** review.

#### FE-VISUAL-6 — Chart giữ palette Metabase và các dấu hiệu của nó · PHẢI
Chart giữ palette series của Metabase cùng các dấu hiệu không phụ thuộc màu: kiểu đường và hình marker theo từng series, và phần mô tả dữ liệu `aria`. Decal vẫn tắt, theo quyết định ở chart 05 (#360); `palette.ts` ghi lý do. Giá trị màu cụ thể bên trong code dựng option của chart được phép, vì ECharts cần giá trị cụ thể.
**Làm thế này:** `frontend/src/components/chart/echarts/palette.ts`. **Kiểm bằng:** review.

## 5. Form (FE-FORM)

#### FE-FORM-1 — State của form tự viết · PHẢI
Form giữ state bằng `useState` hoặc một hook riêng. Không thêm thư viện form.
**Làm thế này:** `frontend/src/routes/control-plane/useEngineConfigForm.ts`. **Kiểm bằng:** review.

#### FE-FORM-2 — Mọi input có `Label` gắn với nó · PHẢI
Mọi input có một `Label` gắn qua `htmlFor`/`id`.
**Làm thế này:** `<Label htmlFor="name">Name</Label><Input id="name" />` (`frontend/src/components/ui/label.tsx`). **Kiểm bằng:** lint (jsx-a11y, `jsx-a11y/label-has-associated-control`).

#### FE-FORM-3 — Giá trị đã biết thì dùng picker · PHẢI
Khi tập giá trị hợp lệ đã biết, control là picker hoặc combobox (chọn hoặc gõ) có trợ giúp, không bao giờ là ô nhập chữ trơn.
**Làm thế này:** `frontend/src/components/ui/select.tsx`, `frontend/src/components/ui/tag-combobox.tsx`; ví dụ: `frontend/src/routes/control-plane/config/PropertyPicker.tsx`. **Kiểm bằng:** review.

#### FE-FORM-4 — Lỗi nằm cạnh field của nó · PHẢI
Lỗi validate hiện cạnh field của nó, bằng chính lời của server nếu lỗi do server trả về.
**Làm thế này:** ví dụ: `frontend/src/routes/control-plane/config/QuantityInput.tsx` (lỗi nằm ngay dưới input của nó). **Kiểm bằng:** review.

## 6. Dữ liệu và API (FE-DATA)

#### FE-DATA-1 — Sinh lại bằng `npm run gen:api` · PHẢI
API client được sinh lại bằng `npm run gen:api`, không bao giờ bằng `orval` trơn, vì nó bỏ mất bước encode tham số đường dẫn.
**Làm thế này:** `npm run gen:api` (`frontend/package.json`), lệnh này chạy `orval` rồi `frontend/scripts/encode-path-params.mjs`. **Kiểm bằng:** review.

#### FE-DATA-2 — Hook của orval và một factory key cho mỗi domain · PHẢI
Endpoint mới được dùng qua hook do orval sinh ra. Query key tự viết lấy từ một factory key theo domain (`queryKeys.ts` trong thư mục của domain), có tiền tố là tên domain.
**Làm thế này:** `frontend/src/routes/control-plane/queryKeys.ts`. **Kiểm bằng:** review.

#### FE-DATA-3 — `localStorage` chỉ giữ tuỳ chọn · PHẢI
Dữ liệu từ server không bao giờ được cache trong `localStorage`. `localStorage` chỉ giữ tuỳ chọn của từng người xem (theme, một panel đang thu gọn), và mọi lần truy cập đều được bọc để trang vẫn chạy khi không có nó.
**Làm thế này:** ví dụ: `frontend/src/components/explore/folders.ts` (trạng thái thu gọn; mọi lần đọc nằm trong `try`). **Kiểm bằng:** review.

## 7. Routing (FE-ROUTE)

#### FE-ROUTE-1 — Route nằm trong `App.tsx`, sau guard của nó · PHẢI
Route được khai báo trong `frontend/src/App.tsx`. Mỗi route được bọc trong guard mà khu vực của nó cần (`RequireFeature`, `RequirePermission`, `RequireGlobalAdmin`). Redirect dùng đường dẫn tuyệt đối.
**Làm thế này:** `frontend/src/auth/guards.tsx`. **Kiểm bằng:** review.

#### FE-ROUTE-2 — Khu vực tính năng mới được lazy-load · PHẢI
Mọi khu vực tính năng mới, bắt đầu từ Workflow, được tách code bằng `React.lazy` và `Suspense`, với fallback là skeleton.
**Làm thế này:** ví dụ: `frontend/src/components/monaco/MonacoEditor.tsx` (`lazy` có fallback). **Kiểm bằng:** test: một bundle test kiểu `npm run test:bundle:monaco` (`frontend/scripts/assert-monaco-lazy.mjs`). Hiện chưa có test nào như vậy cho route: W7, khu vực lazy đầu tiên, thêm nó, theo mẫu `assert-monaco-lazy.mjs`.

#### FE-ROUTE-3 — URL kebab-case, thuộc workspace · PHẢI
URL viết kebab-case và thuộc workspace, dưới `/explore/:workspace/…`.
**Làm thế này:** `frontend/src/App.tsx` (ví dụ `/explore/:workspace/data-api/endpoints/new`). **Kiểm bằng:** review.

## 8. Truy cập (FE-A11Y)

#### FE-A11Y-1 — Qua bộ rule recommended của jsx-a11y · PHẢI
Code qua được bộ rule recommended của `eslint-plugin-jsx-a11y-x` (bản fork được bảo trì của `eslint-plugin-jsx-a11y`; mã rule viết là `jsx-a11y/…`). `jsx-a11y/no-autofocus` tắt; xem FE-A11Y-6.
**Làm thế này:** sửa những gì lint báo; cấu hình nằm ở `frontend/eslint-rules/plugins.js`. **Kiểm bằng:** lint (jsx-a11y).

#### FE-A11Y-2 — Ring focus dùng chung, đạt 3:1 · PHẢI
Focus dùng ring chung (`focus-visible:ring-2 focus-visible:ring-ring`). Chỉ báo focus đạt tương phản 3:1 trên mọi bề mặt nó xuất hiện.
**Làm thế này:** ring như trong `frontend/src/components/ui/button.tsx`. **Kiểm bằng:** test (`frontend/src/__tests__/focusIndicatorContrast.test.ts`).

#### FE-A11Y-3 — Mọi thứ bấm được đều dùng được bằng bàn phím · PHẢI
Mọi phần tử bấm được đều dùng được bằng bàn phím. Phần tử không phải nút mà được cho bấm thì có role, `tabIndex` và xử lý Enter/Space. Nút chỉ có icon thì có `aria-label`.
**Làm thế này:** `frontend/src/components/ui/icon-button.tsx` cho nút chỉ có icon. **Kiểm bằng:** review.

#### FE-A11Y-4 — Không bao giờ chỉ dùng màu · PHẢI
Thông tin không bao giờ chỉ truyền bằng màu; chart giữ kiểu đường, marker và mô tả `aria` của nó (FE-VISUAL-6).
**Làm thế này:** `frontend/src/components/chart/echarts/palette.ts` (hình marker và kiểu đường theo từng series, và mô tả `aria`). **Kiểm bằng:** review.

#### FE-A11Y-5 — Tôn trọng giảm chuyển động · PHẢI
Chuyển động tôn trọng `prefers-reduced-motion`.
**Làm thế này:** `frontend/src/index.css` (khối `prefers-reduced-motion: reduce` của nó). **Kiểm bằng:** review.

#### FE-A11Y-6 — `autoFocus` chỉ sau thao tác của chính người dùng · PHẢI
`autoFocus` chỉ dành cho ô xuất hiện do chính thao tác của người dùng (ô đầu tiên của dialog, sửa tại chỗ), không bao giờ lúc tải trang hay đổi route.
**Làm thế này:** ví dụ: `frontend/src/components/ui/confirm-delete-dialog.tsx` (focus trên dialog do người dùng mở). **Kiểm bằng:** review. **Vì sao:** `jsx-a11y/no-autofocus` tắt (phán quyết của người dùng, 2026-10-10), vì nó không phân biệt được hai trường hợp.

## 9. Chữ trên UI (FE-COPY)

#### FE-COPY-1 — Tiếng Anh, viết hoa kiểu câu, động từ, một tên cho mỗi thứ · PHẢI
Chữ trên UI bằng tiếng Anh. Tiêu đề, nút và mục menu viết hoa kiểu câu ("Add a chart"). Nút bắt đầu bằng động từ. Cùng một thứ thì gọi cùng một tên ở mọi nơi.
**Làm thế này:** ví dụ: `frontend/src/routes/explore/dataapi/EndpointCreate.tsx` ("New endpoint", "Create endpoint"). **Kiểm bằng:** review.

#### FE-COPY-2 — Lỗi nói chuyện gì xảy ra và làm gì tiếp · PHẢI
Thông báo lỗi nói chuyện gì đã xảy ra và người dùng có thể làm gì. Không bao giờ hiện exception thô hay stack trace.
**Làm thế này:** ví dụ: `frontend/src/components/monaco/MonacoEditor.tsx` ("The code editor could not be loaded. Reload the page to try again."). **Kiểm bằng:** review.

#### FE-COPY-3 — Chữ nhắc tên control thì có test · PHẢI
Chữ nhắc tên một control ("click Publish") là một khẳng định rằng control đó tồn tại đúng tên ấy, và có test kiểm.
**Làm thế này:** ví dụ: `frontend/src/components/layout/__tests__/Sidebar.test.tsx`. **Kiểm bằng:** test. **Vì sao:** [ui-conventions §3.9](../en/ui-conventions.md#39-copy-that-names-a-control-is-a-claim-about-the-tree-and-is-checked-like-one).

#### FE-COPY-4 — Trang tự viết số nhiều · PHẢI
Số nhiều do trang tự viết ra (`singular` của `SummaryStrip`), không ghép thêm "s" trong code dùng chung.
**Làm thế này:** `frontend/src/components/explore/SummaryStrip.tsx`. **Kiểm bằng:** review.

## 10. Sản phẩm tham chiếu (FE-REF)

#### FE-REF-1 — Visual của egret, luồng của sản phẩm gốc · PHẢI
Hệ visual là của egret. Cấu trúc và luồng thao tác chép sản phẩm gốc của khu vực đó:
- BI (chart, dashboard, bảng, panel cấu hình): Metabase, kể cả cách chia tab và dạng dòng của panel cấu hình;
- Workflow: Kestra.

**Làm thế này:** ví dụ: `frontend/src/components/chart/settings/` (panel cấu hình theo dạng Metabase). **Kiểm bằng:** review.

#### FE-REF-2 — So sánh với một bản đang chạy · PHẢI
Việc so sánh làm trên một bản đang chạy của sản phẩm gốc, không theo trí nhớ. Mọi khác biệt có chủ ý được ghi kèm lý do vào spec của package.
**Làm thế này:** `scripts/harness/` seed các dashboard đối chiếu với Metabase; ghi khác biệt vào spec của package dưới `docs/en/specs/`. **Kiểm bằng:** review.

## 11. Kiểm chứng (FE-TEST)

#### FE-TEST-1 — Test render thứ thật và có thể thất bại · PHẢI
- Test render component thật và kiểm qua DOM, ưu tiên `getByRole`.
- Test không chép lại biểu thức mà nó đang kiểm.
- Fixture phải phân biệt được hành vi đúng với một hành vi sai trông hợp lý; ví dụ, không sắp sẵn thứ tự khi thứ tự là điều cần kiểm.

**Làm thế này:** ví dụ: `frontend/src/components/explore/__tests__/ListStates.test.tsx`. **Kiểm bằng:** review. **Vì sao:** [ui-conventions §8](../en/ui-conventions.md#8-an-assertion-that-a-hardcoded-implementation-would-also-pass-pins-nothing).

#### FE-TEST-2 — Kết quả hiển thị được kiểm trên trình duyệt · PHẢI
Thay đổi về layout, chiều rộng, chiều cao, màu hay bất kỳ kết quả hiển thị nào được kiểm trên trình duyệt thật: Playwright, hoặc đo DOM bằng `getBoundingClientRect` và `getComputedStyle`. jsdom không có layout, và đọc code không kết luận được về layout.
**Làm thế này:** spec Playwright trong `frontend/tests/e2e/` (`npm run test:e2e`); ví dụ: `frontend/tests/e2e/data-api-detail-tab-widths.spec.ts`. **Kiểm bằng:** trình duyệt. **Vì sao:** [ui-conventions §10](../en/ui-conventions.md#10-reading-the-code-does-not-settle-layout).

#### FE-TEST-3 — Component dùng chung chỉ sửa theo kiểu thêm vào · PHẢI
Component dùng chung chỉ được sửa theo kiểu thêm vào. Phạm vi ảnh hưởng đo theo đường dẫn import, không theo tên symbol.
**Làm thế này:** không có một file riêng; theo `docs/en/ui-conventions.md` §7. **Kiểm bằng:** review. **Vì sao:** [ui-conventions §7](../en/ui-conventions.md#7-changing-a-shared-component-additively-and-measure-by-import-path).

#### FE-TEST-4 — Cổng kiểm · PHẢI
Mọi thay đổi frontend qua được, chạy từ `frontend/`:
- `npm run lint` (ESLint, rồi `npm run lint:legacy`, `frontend/scripts/lint-legacy.mjs`)
- `npx tsc -b`
- `npm run test`
- `npm run build`
- các bundle test mà thay đổi chạm tới (`npm run test:bundle`, `test:bundle:xlsx`, `test:bundle:csp`, `test:bundle:monaco`)

**Làm thế này:** các script trong `frontend/package.json`. **Kiểm bằng:** review: reviewer xác nhận cổng kiểm đã chạy.

## Checklist cho reviewer

Mỗi lần review một task frontend trả lời từng mục áp dụng được: **đạt**, **không đạt**, hoặc **không áp dụng**. Đây là những rule mà lint không kiểm được.

- [ ] FE-PAGE-1 — Trang danh sách dùng `ListPageShell`, shell giữ `h1` duy nhất; tiêu đề, toolbar, `SummaryStrip`, bảng.
- [ ] FE-PAGE-2 — Phần chi tiết dùng `ui/tabs`; tab, cùng mọi filter hay lựa chọn mà người dùng muốn chia sẻ, nằm trong URL.
- [ ] FE-PAGE-3 — Tạo trên trang khi có nhiều phần hoặc kết quả chỉ đọc một lần, trong dialog khi một đến ba trường; lối vào là link.
- [ ] FE-PAGE-4 — Danh sách dạng bảng dùng `FilterableTable` hoặc `ui/table`; `<table>` tự viết là grid đặc thù có comment.
- [ ] FE-PAGE-5 — Cấu hình dùng `ConfigTable`/`ConfigTableRow`/`ConfigSection` theo dạng `Setting | Details | Value`.
- [ ] FE-STATE-1 — Có đủ loading, rỗng và lỗi; kiểm `isError` trước; lỗi không hiển thị thành rỗng.
- [ ] FE-STATE-2 — Nội dung đang tải là skeleton; spinner chỉ cho nút và thao tác tại chỗ; không có "Loading…" trơn.
- [ ] FE-STATE-3 — Rỗng là `EmptyState` cho tập chưa lọc; filter không khớp thì nói vậy và cho xoá filter.
- [ ] FE-STATE-4 — Chỉ báo trạng thái so sánh nội dung; nếu phải dùng timestamp thì comment liệt kê nơi ghi.
- [ ] FE-ACTION-2 — Thành công thì invalidate các query bị đổi; không toast khi điều hướng, có toast khi ở lại.
- [ ] FE-ACTION-3 — Thất bại thì hiện lời server và không xoá gì người dùng đã nhập.
- [ ] FE-ACTION-4 — Control bị vô hiệu hoá nói vì sao.
- [ ] FE-ACTION-5 — Form có chỉnh sửa chưa lưu dùng `useUnsavedChangesWarning`.
- [ ] FE-VISUAL-2 — Style inline dùng `var(--token)`; SVG data URL nhận giá trị đã đọc ra.
- [ ] FE-VISUAL-4 — Số dùng `tabular-nums`; thay đổi màu đã kiểm ở light và dark trên mọi bề mặt.
- [ ] FE-VISUAL-5 — (NÊN) Icon từ `lucide-react`; tối đa một `--highlight` mỗi trang; `--accent` không dùng làm điểm nhấn.
- [ ] FE-VISUAL-6 — Chart giữ palette Metabase, kiểu đường, hình marker và mô tả `aria`; decal vẫn tắt.
- [ ] FE-FORM-1 — State của form bằng `useState` hoặc hook riêng; không thư viện form.
- [ ] FE-FORM-3 — Tập giá trị đã biết thì dùng picker hoặc combobox có trợ giúp.
- [ ] FE-FORM-4 — Lỗi validate nằm cạnh field, bằng lời của server nếu lỗi do server trả về.
- [ ] FE-DATA-1 — API client được sinh lại bằng `npm run gen:api`.
- [ ] FE-DATA-2 — Endpoint mới qua hook của orval; key tự viết lấy từ factory key của domain.
- [ ] FE-DATA-3 — Không có dữ liệu server trong `localStorage`; mọi lần truy cập đều được bọc.
- [ ] FE-ROUTE-1 — Route trong `App.tsx`, sau guard của khu vực; redirect dùng đường dẫn tuyệt đối.
- [ ] FE-ROUTE-3 — URL kebab-case dưới `/explore/:workspace/…`.
- [ ] FE-A11Y-3 — Mọi thứ bấm được đều dùng được bằng bàn phím; nút chỉ có icon có `aria-label`.
- [ ] FE-A11Y-4 — Không thông tin nào chỉ truyền bằng màu.
- [ ] FE-A11Y-5 — Chuyển động tôn trọng `prefers-reduced-motion`.
- [ ] FE-A11Y-6 — `autoFocus` chỉ trên ô do chính thao tác của người dùng làm hiện ra.
- [ ] FE-COPY-1 — Tiếng Anh, viết hoa kiểu câu, nút bắt đầu bằng động từ, một tên cho mỗi thứ.
- [ ] FE-COPY-2 — Lỗi nói chuyện gì xảy ra và làm gì tiếp; không exception thô hay stack trace.
- [ ] FE-COPY-4 — Trang tự viết số nhiều.
- [ ] FE-REF-1 — Hệ visual của egret; cấu trúc và luồng của sản phẩm gốc (Metabase cho BI, Kestra cho Workflow).
- [ ] FE-REF-2 — Đã so với một bản đang chạy; khác biệt có chủ ý được ghi vào spec.
- [ ] FE-TEST-1 — Test render component thật, kiểm qua DOM, không chép biểu thức, fixture có thể làm test thất bại.
- [ ] FE-TEST-2 — Kết quả hiển thị đã kiểm trên trình duyệt thật.
- [ ] FE-TEST-3 — Component dùng chung chỉ sửa theo kiểu thêm vào; phạm vi ảnh hưởng đo theo đường import.

## Sổ nợ

**Chính sách: sửa khi chạm tới, và danh sách chỉ co lại.**
- PR nào sửa một file có trong sổ thì đưa file đó về đúng rule, hoặc giải thích trong mô tả PR vì sao chưa làm được.
- Với rule lint, sửa một file nghĩa là xoá nó khỏi `frontend/eslint-rules/legacy.js` trong cùng commit. `npm run lint:legacy` thất bại khi một file đã sửa vẫn còn trong danh sách.
- Không bao giờ thêm file vào danh sách legacy. Comment `eslint-disable` không phải cách lách rule.

### Nợ kiểm bằng lint

Mỗi dòng là một danh sách legacy trong `frontend/eslint-rules/legacy.js`: các file vi phạm rule lúc rule được đưa vào. `eslint.config.js` tắt rule đó cho đúng những file ấy.

| Rule | Rule lint | Số file | Ghi chú |
|---|---|---|---|
| FE-VISUAL-1 | `egret/no-raw-palette` | 12 | gồm cả gradient thô của variant `ai` của Button (`frontend/src/components/ui/button.tsx`) |
| FE-VISUAL-3 | `egret/no-arbitrary-text-size` | 16 | |
| FE-ACTION-1 | `egret/no-window-confirm` | 2 | `DatasetDetail.tsx`, `DatabasesPage.tsx` |
| FE-A11Y-1 | `jsx-a11y/aria-role` | 3 | |
| FE-A11Y-1 | `jsx-a11y/click-events-have-key-events` | 4 | |
| FE-A11Y-1 | `jsx-a11y/heading-has-content` | 2 | |
| FE-A11Y-1, FE-FORM-2 | `jsx-a11y/label-has-associated-control` | 36 | |
| FE-A11Y-1 | `jsx-a11y/no-noninteractive-element-interactions` | 3 | |
| FE-A11Y-1 | `jsx-a11y/no-noninteractive-tabindex` | 2 | |
| FE-A11Y-1 | `jsx-a11y/no-static-element-interactions` | 1 | |
| FE-A11Y-1 | `jsx-a11y/role-has-required-aria-props` | 4 | |

### Nợ kiểm bằng review

Đếm ngày 2026-10-10. Đây là **số đếm bằng grep, không phải bản kiểm kê**: mỗi lệnh là một dụng cụ đo thô, có thể đếm thừa hoặc thiếu, và lệnh được ghi lại để chạy lại được. Chạy các lệnh từ `frontend/`; chúng chạy được trong cả bash lẫn zsh.

| Rule | Nợ | Số đếm grep |
|---|---|---|
| FE-PAGE-1 | trang danh sách của control-plane và governance tự viết khung | (1) 42 file route có `<h1>` riêng; (2) 42 file trong số đó không nhắc tới `ListPageShell` |
| FE-PAGE-4 | `<table>` tự viết ngoài các bảng dùng chung (một số là grid đặc thù được phép) | (3) 19 dòng |
| FE-STATE-2 | chữ "Loading…"; spinner dùng cho nội dung (chưa đếm) | (4) 28 dòng |
| FE-STATE-3 | chữ rỗng "No X" tự viết, và khối rỗng viền đứt | (5) 20 dòng dạng `"No <từ> yet\|found`; (6) 12 dòng `border-dashed` |
| FE-DATA-2 | hai tiền tố key `cp` và `control-plane` cho cùng một domain | (7) 30 dòng `["cp"`; (8) 52 dòng `["control-plane"` |
| FE-ROUTE-2 | không route nào trong `App.tsx` được lazy-load | (9) 0 `lazy(` trong `App.tsx` |
| tài liệu | `docs/vi/ui-conventions.md` đã lệch so với `docs/en/ui-conventions.md` | (10) 664 dòng so với 2.227 |

```bash
grep -rln "<h1" src/routes --include='*.tsx' | grep -v __tests__ | wc -l                                       # (1)
grep -rLn "ListPageShell" $(grep -rln "<h1" src/routes --include='*.tsx' | grep -v __tests__) | wc -l           # (2)
grep -rn "<table" src --include='*.tsx' | grep -v __tests__ | grep -v "components/ui/table" | wc -l             # (3)
grep -rn "Loading…\|Loading\.\.\." src --include='*.tsx' | grep -v __tests__ | wc -l                            # (4)
grep -rn "\"No [a-z]\+ \(yet\|found\)" src --include='*.tsx' | grep -v __tests__ | wc -l                        # (5)
grep -rn "border-dashed" src --include='*.tsx' | grep -v __tests__ | wc -l                                      # (6)
grep -rn "\[\"cp\"\|\['cp'" src --include='*.ts*' | grep -v __tests__ | wc -l                                  # (7)
grep -rn "\[\"control-plane\"" src --include='*.ts*' | grep -v __tests__ | wc -l                                # (8)
grep -c "lazy(" src/App.tsx                                                                                    # (9)
wc -l ../docs/vi/ui-conventions.md ../docs/en/ui-conventions.md                                               # (10)
```
