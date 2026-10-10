# W7a — UI Workflows, phần 1: danh sách flow, editor flow nhiều panel, revisions và Run

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-10-workflow-w7a-flows-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-10; chưa cài.
**Phụ thuộc:**
- W1: flow, draft, version và validation;
- W10: start execution;
- bộ rule frontend `docs/en/frontend-rules.md` (#655), ràng buộc mọi rule của package này.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md`. §8 nói về UI của Kestra làm tham chiếu; §9 dòng W7 nói về phạm vi.

**Đã quyết với người dùng, 2026-10-10:**
- **W7 chia làm năm package,** theo thứ tự:
  1. **W7a, flow:** nhóm nav, danh sách flow, editor flow, Revisions và Run.
  2. **W7b, execution:** danh sách, chi tiết, gantt, log, outputs, resume và replay.
  3. **W7c, topology:** đồ thị flow, và đồ thị execution có màu theo state đang chạy.
  4. **W7d, namespace:** danh sách namespace, file và KV.
  5. **W7e, trigger.**

  Mỗi package được thêm các endpoint backend nhỏ mà UI của nó cần.
  - **Ngoài W7:** sửa kiểu no-code, blueprints, trang tài liệu plugin, AI, dashboards, kill và restart (chưa có backend), và SSE (API của egret chỉ có polling).
- **Danh sách flow nhóm theo cây namespace** (`company` → `company.team`). `folder_id` không hiện ở đâu trong UI workflow.
- **Editor flow là bố cục nhiều panel đầy đủ như `MultiPanelTabs` của Kestra.** Panel chứa tab; tab chuyển được giữa các panel và tách panel; bố cục được lưu theo từng flow.
- **Save và Publish là hai nút.** Save ghi draft; Publish tạo một version bất biến, là thứ execution chạy theo. Một chỉ báo suy từ nội dung cho biết khi draft khác version đang live.
- **Độ thông minh của editor:**
  - `monaco-yaml` dùng một JSON schema do một endpoint chỉ đọc mới cung cấp. Schema dựng từ chính registry mà backend dùng để validate, và lọc theo feature của workspace. Nhờ đó có gợi ý, hover và lỗi cấu trúc ngay khi gõ.
  - Một lần validate trên server (có debounce) đưa lỗi ngữ nghĩa vào editor và vào tab Problems.

## 1. Vì sao

Engine đã chạy flow trong production (từ W10), nhưng người dùng chỉ viết và chạy được flow bằng `curl`. W7a là package đầu tiên cho người dùng viết, publish và chạy flow trên trình duyệt. Nó cũng dựng khung để bốn package W7 còn lại gắn vào: nhóm nav, route lazy, registry tab và bộ từ vựng state.

## 2. Khác biệt với Kestra

| Kestra | egret | Vì sao |
|---|---|---|
| Save tạo một revision có hiệu lực ngay | Save ghi draft; Publish tạo version mà execution chạy theo | flow của egret có mô hình draft/version (W1); phán quyết của người dùng |
| Danh sách flow phẳng, có bộ lọc namespace | Danh sách nhóm theo cây namespace thu gọn được | Phán quyết của người dùng; namespace quyết định phạm vi ghi, file và KV, nên UI lấy nó làm trục tổ chức |
| Tab: overview, executions, edit, revisions, triggers, logs, metrics, dependencies, concurrency | W7a có Edit và Revisions. Executions đến cùng W7b, Triggers cùng W7e; các tab khác chưa có kế hoạch | Một tab chỉ xuất hiện cùng package làm nó |
| Panel editor: Code, No-code, Topology, Docs, Files, Blueprints | Code, Docs, Problems ở W7a; Topology cùng W7c; Files cùng W7d; không có No-code và Blueprints | Ngoài W7 theo phán quyết của người dùng. Problems là của egret: Kestra hiện lỗi trong editor và trên một banner |
| `/flows/validate` nhận YAML có thêm tiền tố `revision: N+1`, và phát hiện lần save `outdated` | `POST …/flows/validate` nhận YAML nguyên như vậy. Lần save trên bản cũ bị `PUT` từ chối nhờ `if_updated_at` (§5.3) | draft của egret không có bộ đếm revision |
| Form run theo revision mới nhất | Form run theo **version đang live**, và nói rõ khi draft khác | Execution chạy version live, không bao giờ chạy draft (W1) |

Mọi khác biệt khác tìm thấy khi đối chiếu với một Kestra đang chạy (§9) được thêm vào bảng này kèm lý do.

## 3. Route và điều hướng

**Nav.** Sidebar Explore có thêm nhóm **Workflows**, gate theo feature `workflows` giống các module khác (`RequireFeature`, `EXPLORE_MODULES`). Nhóm có hai mục **Flows** và **Executions**. Executions là trang placeholder cho tới W7b.

`components/layout/__tests__/Sidebar.test.tsx` hiện khẳng định không có link Workflows ("the real Workflows group lands in W7"). Khẳng định đó được thay bằng một khẳng định rằng nhóm có tồn tại và có gate.

**Route,** dưới `/explore/:workspace/workflows/`. Mọi trang được tách code bằng `React.lazy` (FE-ROUTE-2), với fallback là skeleton; guard là `RequireFeature feature="workflows"`.

| Route | Trang |
|---|---|
| `flows` | Danh sách flow (§4) |
| `flows/new` | Tạo flow (§5.4); khai báo trước `:id` |
| `flows/:id` | Chuyển hướng sang `flows/:id/edit` |
| `flows/:id/:tab` | Chi tiết flow; `tab` là `edit` hoặc `revisions` trong W7a. Tab lạ thì chuyển hướng sang `edit` |
| `executions` | Placeholder cho tới W7b |
| `executions/:id` | Trang execution tối thiểu (§6.3), W7b sẽ thay |

## 4. Danh sách flow

`ListPageShell` cho tiêu đề "Flows" và hành động chính "New flow" (link tới `flows/new`). Bên dưới là `SummaryStrip`, với tổng số và số lượng theo state, rồi tới bảng (FE-PAGE-1).

- **Nhóm.** Các dòng được nhóm dưới một **cây namespace** dựng phía client từ namespace của các flow, tách theo dấu `.`.
  - Mỗi nút cây hiện tên đoạn của nó và số flow bên dưới. Nút thu gọn và mở được, và trạng thái mở là tuỳ chọn của từng người xem.
  - Một namespace vừa có flow vừa có namespace con thì hiện flow của chính nó phía trên các nhánh con.
  - Cây là một `<table>` nhóm tự viết, cùng dạng với các danh sách nhóm của Explore (`ui-conventions.md` §1.5), kèm một comment giải thích (FE-PAGE-4).
- **Cột:**
  - flow id, là link tới editor của flow;
  - state, dưới dạng chip (draft, live, disabled);
  - thời điểm cập nhật, dạng thời gian tương đối;
  - người tạo.
- **Bộ lọc,** lưu trong URL (FE-PAGE-2):
  - tìm chữ theo flow id và namespace;
  - namespace, là một picker các namespace đã biết, tính cả các namespace con;
  - state.
- **Trạng thái** (FE-STATE):
  - loading dùng `ListStates`;
  - tập dữ liệu rỗng dùng `EmptyState`, với hành động chính là "New flow";
  - bộ lọc không khớp gì thì nói đúng như vậy và cho xoá bộ lọc;
  - lỗi dùng dòng lỗi của `ListStates`.

`GET …/flows` không phân trang; W7a hiện toàn bộ những gì nó trả về. Nếu có workspace nào đủ nhiều flow để chuyện này thành vấn đề, phân trang là một follow-up.

## 5. Editor flow

### 5.1 Bố cục nhiều panel

**Mô hình.**
- Tab Edit là một hàng ngang các **panel**, ngăn bằng thanh chia kéo được (`react-resizable-panels`). Mỗi panel có một dải tab và hiện một tab đang chọn.
- Bố cục là dữ liệu thuần: `{ panels: [{ id, size, tabs: TabId[], active: TabId }] }`. Mọi thao tác là một hàm thuần trên dữ liệu đó.
- **Các thao tác:**
  - chuyển một tab sang panel khác tại một vị trí;
  - tách, tức chuyển một tab vào một panel mới bên trái hoặc bên phải một panel;
  - đóng một tab;
  - mở lại một tab, qua menu "+" của panel, menu này liệt kê các tab đã đóng;
  - đổi kích thước.
  - Panel không còn tab nào thì bị xoá. Tab Code không đóng được.
- **Chuột:** tab kéo được bằng `@dnd-kit`. Thả lên một dải tab khác thì chuyển tab; thả lên mép trái hoặc phải của một panel thì tách panel.
- **Bàn phím (FE-A11Y-3):**
  - mọi tab có menu gồm "Move to panel…", "Split left", "Split right" và "Close";
  - bật bộ cảm biến bàn phím của dnd-kit;
  - dải tab là một `tablist`, điều hướng bằng phím mũi tên.
- **Lưu bố cục.**
  - Bố cục được lưu theo từng flow trong `localStorage`, với key `egret.wf.layout.<flow uuid>`. Đây là tuỳ chọn của từng người xem (FE-DATA-3).
  - Mọi lần đọc và ghi đều được bọc. Bố cục thiếu, không parse được hoặc không hợp lệ thì quay về mặc định. Một tab id lạ, ví dụ của một package sau đã bị rollback, bị bỏ qua.
  - Flow mới dùng bố cục mặc định.
- **Mặc định:** hai panel `[Code] | [Docs, Problems]`, tỷ lệ 60/40.

**Registry tab.** Mỗi loại tab được đăng ký một lần với `{ id, title, icon, closable, component }`. W7c đăng ký Topology và W7d đăng ký Files mà không sửa khung.

### 5.2 Tab Code, Docs và Problems

**Code.**
- Monaco cộng `monaco-yaml`. Schema lấy từ `GET …/workflows/schema` (§7.1), cache theo ETag. `monaco-yaml` và worker của nó được lazy-load cùng Monaco, và một bundle test khẳng định cả hai nằm ngoài chunk khởi đầu.
- Nhờ đó có gợi ý `type:`, thuộc tính và giá trị enum, hover tài liệu, và lỗi cấu trúc ngay khi gõ.
- **Validate trên server.**
  - Chạy 600 ms sau lần sửa cuối. Một request mới huỷ request cũ.
  - Gọi `POST …/flows/{id}/validate` với flow đã có, hoặc `POST …/flows/validate` (§7.2) ở trang tạo.
  - Kết quả thành marker của Monaco. `path` của mỗi kết quả (ví dụ `tasks[2].retry.maxAttempt`) được map sang một vùng bằng cách duyệt tài liệu YAML với package `yaml`. Path không map được thì đánh dấu dòng 1 và được liệt kê trong Problems kèm path của nó.
- **Lưu.**
  - `Ctrl/Cmd+S` để lưu.
  - Chỉnh sửa chưa lưu thì bật `useUnsavedChangesWarning`.
  - Người chỉ có quyền đọc (không có `workflow:manage`) thấy editor chỉ đọc, kèm banner giải thích vì sao.

**Docs.** Tab này đi theo con trỏ.
- Nó tìm mapping trong cùng chứa con trỏ có key `type:`, rồi hiện tài liệu của task hoặc trigger đó từ cùng schema: mô tả, bảng thuộc tính (tên, kiểu, bắt buộc, mặc định, mô tả) và outputs.
- Con trỏ nằm ngoài mọi mapping có `type:` thì hiện tài liệu cấp flow (`id`, `namespace`, `inputs`, `tasks`, `triggers`, `errors`, …).
- Cập nhật khi con trỏ di chuyển, debounce 150 ms.

**Problems.**
- Liệt kê lỗi và cảnh báo từ cả hai nguồn: marker của schema và kết quả của server. Mỗi mục hiện mức độ, thông báo và path.
- Chọn một mục thì con trỏ của editor nhảy tới vùng của mục đó và editor được focus.
- Tiêu đề tab mang số lỗi.
- Trạng thái (FE-STATE):
  - "Validating…" dùng một dòng skeleton;
  - "No problems" dùng `EmptyState`;
  - request validate thất bại thì hiện lỗi kèm nút thử lại. Không bao giờ hiện thành "không có lỗi".

### 5.3 Save, Publish và chỉ báo draft

Header chi tiết flow là flow id và namespace kèm chip state. Các hành động:

**Save** (`workflow:manage`).
- Gửi `PUT …/flows/{id}` với `{ source, if_updated_at }` (§7.3).
- Nếu nhận 409, một dialog giải thích draft đã bị đổi kể từ lúc mở. Dialog cho chọn "Reload" (bỏ thay đổi local, sau khi xác nhận) hoặc "Overwrite" (lưu lại mà không gửi `if_updated_at`).
- Save vẫn bật khi validate có lỗi; draft được phép không hợp lệ.

**Publish** (`workflow:manage`).
- Mở một dialog có ghi chú tuỳ chọn, rồi gửi `POST …/flows/{id}/publish`.
- Bị vô hiệu hoá khi còn chỉnh sửa chưa lưu ("Save first") hoặc có lỗi validate ("Fix N errors to publish"). Lý do hiện ngay trên control (FE-ACTION-4).

**Run** (`workflow:manage`).
- Mở form run (§6). Bị vô hiệu hoá khi chưa có version nào live ("Publish the flow before running it").

**Delete** (`workflow:manage`).
- Đi qua `ConfirmDeleteDialog` (FE-ACTION-1), rồi chuyển về danh sách.

**Chỉ báo draft** là một pill, "Draft differs from live v{n}".
- Nó được tính bằng cách so `source` của draft đã lưu với `source` trong snapshot của version live, từng byte (FE-STATE-4). Nếu draft được sửa về đúng nội dung live rồi lưu, pill tắt lại.
- Chỉnh sửa local chưa lưu là một chỉ báo riêng ("Unsaved changes").

Sau khi Save hoặc Publish thành công, query flow được invalidate; Publish còn invalidate thêm query versions. Save và Publish xác nhận bằng toast, vì người dùng ở lại trang (FE-ACTION-2).

### 5.4 Tạo flow

`flows/new` là một trang (FE-PAGE-3). Nó mở editor với mẫu sau:

```yaml
id: hello
namespace: company.team
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: Hello from egret
```

Hành vi:
- Validate dùng `POST …/flows/validate`.
- Lần Save đầu gửi `POST …/flows`, rồi thay URL bằng `flows/{id}/edit`, giữ nguyên chữ trong editor và bố cục.
- Lỗi 409, nghĩa là namespace và id đã tồn tại, hiện cạnh editor kèm thông báo của server, và chữ được giữ nguyên.

## 6. Run

### 6.1 Form

Run đọc `inputs:` trong snapshot của **version đang live**. Khi draft khác live, form ghi "Runs live v{n}; your draft is not published".

- **Khung chứa.** Flow không có input thì chạy ngay sau một bước xác nhận. Từ một đến ba input, tất cả đều là kiểu đơn, thì mở dialog. Còn lại mở một drawer bên phải.
- **Control theo kiểu input** (`models/input.rs`):

| Kiểu input | Control |
|---|---|
| STRING, EMAIL, URI | ô chữ |
| INT, FLOAT | ô số |
| BOOL | switch |
| DATETIME, DATE, TIME | picker ngày/giờ (giá trị ISO) |
| DURATION | ô chữ, placeholder ISO-8601 (`PT1H`) |
| SELECT | select; MULTISELECT là multi-select (cả hai lấy từ `values`) |
| ARRAY | danh sách các dòng, mỗi dòng theo `itemType` |
| JSON, YAML | một Monaco nhỏ |
| SECRET | ô mật khẩu |
| FILE | chọn file; request chuyển thành multipart |
| FORM, REUSABLE_INPUTS, ION | chưa hỗ trợ: form ghi tên input và kiểu của nó, và Run bị vô hiệu hoá |

- **Nhãn và giá trị.**
  - Mỗi control có một `Label` gắn qua `htmlFor` (FE-FORM-2), hiện `displayName` hoặc `id` của input, `description` của nó, và dấu bắt buộc.
  - Giá trị mặc định được điền sẵn. Ô bắt buộc mà để trống thì chặn submit, kèm thông báo ngay tại chỗ.
- **Submit.**
  - Gửi `POST …/flows/{id}/executions`.
  - Lỗi input từ server hiện cạnh đúng ô (FE-FORM-4). Không có gì đã nhập bị xoá (FE-ACTION-3).
  - Thành công thì chuyển sang `executions/{id}`.

### 6.2 Những gì form run không làm

`dependsOn` của Kestra (input được render lại theo input khác) chưa hỗ trợ trong W7a. Input có `dependsOn` hiện danh sách `values` tĩnh của nó, kèm một ghi chú. Backend có tính `dependsOn` hay không sẽ được ghi vào §15 sau khi kiểm.

### 6.3 Trang execution tối thiểu

`executions/:id` hiện:
- id execution, flow, namespace, chip state, thời điểm bắt đầu và thời lượng;
- link quay về flow;
- các task run, dạng danh sách đơn giản gồm id và state.

Khi execution chưa kết thúc, trang poll `GET …/executions/{id}` mỗi 2 giây. W7b sẽ thay trang này.

## 7. Bổ sung ở backend

### 7.1 `GET /api/ws/{ws}/workflows/schema`

- **Quyền:** `workflow:read`.
- **Phản hồi:** một JSON Schema (draft 2020-12) cho một tài liệu flow:
  - khung flow (`id`, `namespace`, `description`, `labels`, `inputs`, `variables`, `tasks`, `errors`, `finally`, `triggers`, `pluginDefaults`, `concurrency`, `disabled`, …);
  - một `oneOf` theo kiểu task, chọn theo `type`, cho `tasks`/`errors`/`finally` (đệ quy cho task con của task flowable), và tương tự theo kiểu trigger cho `triggers`.
- **Mục của mỗi kiểu** gồm mô tả, `properties` với kiểu, bắt buộc, mặc định và mô tả, và `outputs` (dưới dạng phần mở rộng `x-egret-outputs`, dùng cho tab Docs).
- **Nguồn.** Schema dựng từ registry (`spec/registry.rs`) và catalog plugin (`plugins/catalog.json`), đúng những thứ publish và validate đang dùng. Một test Rust khẳng định tập giá trị `type` trong schema bằng đúng tập registry chấp nhận với cùng tổ hợp feature. Nhờ vậy schema không thể liệt kê một kiểu mà publish từ chối, hay bỏ sót một kiểu publish chấp nhận.
- **Feature.**
  - Tắt `workflow_kestra_plugins` thì không có các kiểu plugin Kestra.
  - Tắt `workflow_containers` thì không có các kiểu container.
  - Phản hồi khác nhau theo workspace, nên nó mang một `ETag` mạnh và `Cache-Control: private, no-cache`.
- **Alias.** Các alias ngắn mà registry chấp nhận được đưa vào enum `type`, kèm tên FQCN chính thức trong phần mô tả.

### 7.2 `POST /api/ws/{ws}/workflows/flows/validate`

- **Quyền:** `workflow:manage`, giống route validate theo id hiện có.
- **Body:** `{ "source": string }`, cùng giới hạn kích thước body như route tạo.
- **Phản hồi:** cùng `ValidationReportDto` như `POST …/flows/{id}/validate`, tính bằng cùng một hàm. Không ghi gì.
- **Lỗi:** YAML không hợp lệ vẫn trả 200 kèm kết quả validate; 400 chỉ dành cho body request sai định dạng.

### 7.3 `if_updated_at` trong `PUT /flows/{id}`

`UpdateFlowRequest` có thêm trường tuỳ chọn `if_updated_at` (RFC 3339).
- Khi có trường này mà nó khác `updated_at` của hàng, lần cập nhật bị từ chối với 409, "the draft was changed since you opened it", và không có gì được ghi.
- So sánh và ghi nằm trong một câu lệnh (`UPDATE … WHERE id = $1 AND updated_at = $2`), nên hai lần lưu đồng thời không thể cùng qua.
- Không có trường này thì route giữ nguyên hành vi như hiện nay.
- Mọi nơi ghi `flows.updated_at` được liệt kê trong doc comment của route, vì ý nghĩa của điều kiện tiên quyết này dựa vào chúng (rule FE-STATE-4, áp cho một điều kiện tiên quyết).

`openapi.json` và client được sinh lại bằng `npm run gen:api` (FE-DATA-1).

## 8. Cấu trúc frontend

- `frontend/src/routes/workflows/` chứa các trang và `queryKeys.ts`: factory key có tiền tố `["workflows", …]` (FE-DATA-2).
- `frontend/src/components/workflows/` chứa:
  - chip state, ánh xạ 17 state của Kestra sang token của egret (§8.1);
  - khung nhiều panel và registry tab;
  - các tab của editor;
  - form run.
- Truy cập dữ liệu đi qua các hook do orval sinh cho mọi route ở §3–§7 (FE-DATA-2).

### 8.1 Màu state

Kestra tô màu state theo sắc màu. egret ánh xạ chúng sang token của mình (FE-VISUAL-1). Mỗi chip cũng mang tên state và một icon, nên state không bao giờ chỉ thể hiện bằng màu (FE-A11Y-4).

| Nhóm | State | Token |
|---|---|---|
| chờ | CREATED, SUBMITTED, QUEUED, RESTARTED, RESUBMITTED | `muted` |
| đang chạy | RUNNING, RETRYING, KILLING | `primary` |
| thành công | SUCCESS | `success` |
| cảnh báo | WARNING, PAUSED, BREAKPOINT | `highlight` |
| thất bại | FAILED, KILLED | `destructive` |
| trung tính | CANCELLED, SKIPPED, RETRIED | `muted-foreground` |

State của flow dùng cùng chip: draft là `muted`, live là `success`, disabled là `muted-foreground`.

## 9. Đối chiếu với Kestra

Trong lúc cài, một Kestra tạm chạy từ image `kestra/kestra` có sẵn trên máy (`server local`), ở một cổng còn trống. Nó bị xoá sau khi xong và không bao giờ đụng tới dev stack của egret. Editor (panel, chuyển và tách tab, panel docs, nhịp validate), Revisions và form run được so về cấu trúc và tương tác (FE-REF-2), kèm ảnh chụp hoặc ghi chép DOM trong báo cáo của task. Khác biệt được đưa vào §2.

## 10. Xử lý lỗi

- Mọi khung hiển thị dữ liệu có đủ trạng thái loading, rỗng và lỗi (FE-STATE-1).
- Mutation thất bại thì hiện thông báo của server (FE-ACTION-3).
- 403 do feature hoặc quyền thì hiện trang forbidden có sẵn.
- 404 với một flow thì hiện "Flow not found" kèm link về danh sách.
- Tải schema thất bại: editor vẫn dùng được nhưng không có gợi ý, và hiện "Autocomplete unavailable" kèm nút thử lại. Validate trên server vẫn chạy.
- Request validate thất bại: Problems hiện lỗi (§5.2). Publish vẫn bị vô hiệu hoá với lý do "Validation unavailable".

## 11. Kiểm thử

- **Rust:**
  - tập type của schema bằng tập của registry, với từng tổ hợp feature;
  - việc lọc theo feature;
  - alias;
  - ETag;
  - validate không cần id trả cùng báo cáo như validate theo id với cùng source;
  - `if_updated_at`: khớp, không khớp và không ghi gì, không có trường, và hai lần lưu đồng thời thì đúng một lần thành công;
  - quyền trên ba route.
- **Vitest,** render component thật (FE-TEST-1):
  - các hàm bố cục (chuyển, tách, đóng, mở lại, bố cục đã lưu không hợp lệ);
  - cây namespace (nhóm, namespace vừa có flow vừa là nhánh cha, số đếm, lọc theo nhánh);
  - ba trạng thái của danh sách, và sự phân biệt giữa rỗng thật với lọc không ra kết quả;
  - map path của kết quả validate sang vùng trong YAML;
  - tab Docs tìm `type:` bao quanh con trỏ;
  - Save, Publish và Run được bật hay bị vô hiệu hoá, kèm lý do;
  - dialog 409;
  - chỉ báo draft tắt khi nội dung khớp trở lại;
  - control của từng kiểu input và giá trị nó gửi đi;
  - lỗi theo ô từ server, và giá trị đã nhập được giữ;
  - restore trong Revisions kèm bước xác nhận.
- **Playwright,** trên một stack local, không bao giờ trên dev stack dùng chung (FE-TEST-2):
  - tạo flow → gợi ý xuất hiện → một lỗi được đánh dấu và liệt kê → Save → Publish → Run → trang execution;
  - kéo một tab để tách panel, rồi tải lại trang và thấy bố cục được khôi phục;
  - cách làm tương đương bằng bàn phím;
  - diff trong Revisions.
- **Bundle test (FE-ROUTE-2):** các route Workflows và `monaco-yaml` nằm ngoài chunk khởi đầu.
- **Cổng kiểm (FE-TEST-4):** `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`, các bundle test; và `cargo test -p egret-workflow`, clippy, fmt cho backend.

## 12. Hình dạng plan

1. Backend: §7.1, §7.2, §7.3, openapi và `gen:api`.
2. Khung: nhóm nav, route lazy, `queryKeys`, chip state, danh sách flow với cây namespace, và các trang placeholder.
3. Khung nhiều panel và registry tab.
4. Tab Code (monaco-yaml, schema, validate trên server, map path), Docs và Problems.
5. Header chi tiết flow: Save với `if_updated_at`, Publish, Delete, chỉ báo draft, và trang tạo flow.
6. Form run và trang execution tối thiểu.
7. Revisions và diff.
8. End to end, đối chiếu Kestra, và tài liệu (cập nhật §2 và §15 của spec này).

## 13. Ngoài phạm vi

- Mọi thứ của W7b–W7e. Sửa kiểu no-code, blueprints, trang tài liệu plugin và AI.
- Nhập/xuất flow, thao tác hàng loạt trên flow, và phân trang danh sách flow.
- Input có `dependsOn` (§6.2).
- Kill và restart.
- Cập nhật thời gian thực ngoài polling.

## 14. Rule package này phải đáp ứng

Toàn bộ `docs/en/frontend-rules.md`. Những rule quan trọng nhất ở đây:
- FE-PAGE-1/2/3/4;
- FE-STATE-1–4;
- FE-ACTION-1–5;
- FE-VISUAL-1/3/4;
- FE-FORM-2/3/4;
- FE-DATA-1/2/3;
- FE-ROUTE-1/2/3;
- FE-A11Y-3/6;
- FE-REF-1/2;
- FE-TEST-1/2/4.

Mỗi lần review task đều trả lời checklist cho reviewer của bộ rule.

## 15. Giới hạn đã biết

- Danh sách flow không phân trang (§4).
- Input có `dependsOn` là tĩnh (§6.2).
- Kết quả validate có `path` không map được thì hiện ở dòng 1 (§5.2).
