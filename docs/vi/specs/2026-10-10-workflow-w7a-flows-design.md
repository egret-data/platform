# W7a — UI Workflows, phần 1: danh sách flow, editor flow nhiều panel, revisions và Run

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-10-workflow-w7a-flows-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** đã cài, 2026-10-10 (W7a Task 1–8). Lần chạy end-to-end của Task 8 và phần đối chiếu với một Kestra đang chạy (2.0.0-rc13, image local đã ghim) được gộp vào §2, §5, §6 và §15.
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
- **Save và Create version là hai nút.** Save ghi draft, và đó là thứ mọi execution chạy; Create version tạo một snapshot bất biến có đánh số. Một chỉ báo suy từ nội dung cho biết khi draft đã lưu khác version mới nhất. *(Viết lại 2026-10-10 theo phán quyết bên dưới; bản đầu gọi nút thứ hai là Publish và nói execution chạy version.)*
- **Độ thông minh của editor:**
  - `monaco-yaml` dùng một JSON schema do một endpoint chỉ đọc mới cung cấp. Schema dựng từ chính registry mà backend dùng để validate, và lọc theo feature của workspace. Nhờ đó có gợi ý, hover và lỗi cấu trúc ngay khi gõ.
  - Một lần validate trên server (có debounce) đưa lỗi ngữ nghĩa vào editor và vào tab Problems.

- **(2026-10-10, phán quyết của người dùng khi lập plan) Dữ liệu thuộc tính của schema lấy từ JSON schema của chính Kestra, lọc theo egret.** Registry của egret chỉ có tên type và alias, còn `catalog.json` chỉ có thuộc tính của chín class JDBC, nên không có dữ liệu thuộc tính nào để trả về. Flow schema của Kestra cho bản đã ghim được xuất một lần từ image Kestra trên máy và commit thành một artifact, giống các fixture oracle. Schema được lọc chỉ giữ các type egret chấp nhận, và bỏ các thuộc tính egret bỏ qua. Các type riêng của egret (`egret.*`, `core.http`, `core.slack`, các type script) có schema viết bằng tay (§7.1).

- **(2026-10-10, phán quyết của người dùng về review Task 2) Backend trả tên hiển thị của người tạo và người publish.** `FlowListItemDto`, `FlowDto` và `FlowVersionDto` có thêm `created_by_name: Option<String>`, join phía server. UI không cần lấy danh sách thành viên (route đó cần `workspace:manage_members`), nên mọi người xem đều thấy tên (§4, §7.4).

- **(2026-10-10, phán quyết của người dùng về phát hiện của Task 6) UI nói đúng thực tế: mọi execution chạy bản draft đã lưu.** Start qua API, schedule, webhook và subflow đều đọc `workflow.flows.source`. Không đường nào đọc `flow_versions`; đây là đính chính W10 của spec chương trình ("executions run the DRAFT"). `state` không chặn gì ngoài `disabled`. Engine giữ nguyên, và W7a sửa UI cho khớp:
  - **Save làm thay đổi thứ đang chạy.** Đây là mô hình của Kestra; thông báo xác nhận Save và editor nói rõ điều này.
  - **Publish đổi thành "Create version".** Đó là một snapshot bất biến có số và ghi chú (một mốc phát hành), dùng để so sánh và khôi phục trong Revisions. Nó không làm thay đổi thứ đang chạy.
  - **Chỉ báo so nội dung.** Nó ghi "Changes since v{n}": draft đã lưu khác version mới nhất, so từng byte (FE-STATE-4). Chỉ báo này tách riêng khỏi "Unsaved changes".
  - **Run** chạy bản draft đã lưu và đọc `inputs:` của nó. Run không cần version nào, và chỉ bị vô hiệu hoá khi draft còn chỉnh sửa chưa lưu ("Save first: Run uses the saved draft"), khi validate có lỗi, hoặc khi flow đang `disabled`.

- **(2026-10-10, phán quyết của người dùng về review Task 6, M5) Chip state của flow chỉ cho biết flow có chạy hay không.** Nó ghi **Active** khi state backend là `draft` hoặc `live`, và **Disabled** khi là `disabled`; "Draft" và "Live" đọc như "version nào đang chạy", điều không còn ý nghĩa. Số version mới nhất hiện tách khỏi chip: cột "Version" trong danh sách flow và một nhãn trong header chi tiết flow, cả hai ghi "v{n}", hoặc "—" khi chưa có version. `FlowListItemDto` và `FlowDto` có thêm `latest_version: Option<i32>`, số version lớn nhất của flow, đọc trong cùng câu truy vấn với hàng đó (như `created_by_name`).

## 1. Vì sao

Engine đã chạy flow trong production (từ W10), nhưng người dùng chỉ viết và chạy được flow bằng `curl`. W7a là package đầu tiên cho người dùng viết, tạo version và chạy flow trên trình duyệt. Nó cũng dựng khung để bốn package W7 còn lại gắn vào: nhóm nav, route lazy, registry tab và bộ từ vựng state.

## 2. Khác biệt với Kestra

| Kestra | egret | Vì sao |
|---|---|---|
| Save tạo một revision có hiệu lực ngay | Save cũng có hiệu lực ngay, nhưng không tạo version. "Create version" tạo một snapshot có số và ghi chú khi người dùng muốn | Mọi execution chạy bản draft đã lưu (spec chương trình, đính chính W10); phán quyết của người dùng |
| Danh sách flow phẳng, có bộ lọc namespace | Danh sách nhóm theo cây namespace thu gọn được | Phán quyết của người dùng; namespace quyết định phạm vi ghi, file và KV, nên UI lấy nó làm trục tổ chức |
| Tab: overview, executions, edit, revisions, triggers, logs, metrics, dependencies, concurrency | W7a có Edit và Revisions. Executions đến cùng W7b, Triggers cùng W7e; các tab khác chưa có kế hoạch | Một tab chỉ xuất hiện cùng package làm nó |
| Panel editor: Code, No-code, Topology, Docs, Files, Blueprints | Code, Docs, Problems ở W7a; Topology cùng W7c; Files cùng W7d; không có No-code và Blueprints | Ngoài W7 theo phán quyết của người dùng. Problems là của egret: Kestra hiện lỗi trong editor và trên một banner |
| `/flows/validate` nhận YAML có thêm tiền tố `revision: N+1`, và phát hiện lần save `outdated` | `POST …/flows/validate` nhận YAML nguyên như vậy. Lần save trên bản cũ bị `PUT` từ chối nhờ `if_updated_at` (§5.3) | draft của egret không có bộ đếm revision |
| Form run theo revision mới nhất | Form run theo **bản draft đã lưu**, và Run bị vô hiệu hoá khi còn chỉnh sửa chưa lưu | Execution chạy bản draft đã lưu (phán quyết của người dùng, 2026-10-10) |
| Input JSON và YAML là editor code nội tuyến | Một ô văn bản font monospace; JSON được kiểm trước khi Run | Ô văn bản nhận `id` mà `Label` trỏ tới (FE-FORM-2) và để phím Tab di chuyển giữa các ô; `tabFocusMode` và `aria-labelledby` của Monaco làm được cả hai, nhưng form run không được lợi gì từ một editor (W7a Task 6, được chấp nhận khi review) |
| MULTISELECT là một select nhiều lựa chọn có tìm kiếm và chọn tất cả | Một danh sách checkbox, mỗi giá trị một ô | Mỗi checkbox có `Label` gắn riêng (FE-FORM-2) và tập giá trị vẫn là một picker (FE-FORM-3); `ui/tag-combobox` không nhận `id`. Danh sách `values` dài làm drawer cao lên, và drawer cuộn được (W7a Task 6, được chấp nhận khi review) |
| Form run ("Execute the flow") là một modal giữa màn hình, rộng khoảng 750px, có ba tab (Inputs, Other properties, cURL command); phần thân cuộn được và nút Execute nằm yên ở footer | Tối đa ba input scalar thì mở dialog (576px, giữa màn hình); ngoài ra là chính primitive dialog ghim vào mép phải, cao hết màn hình (512px; trên điện thoại thì rộng hết). Cả hai chỉ có một khung input, và Run nằm trong footer dính đáy trong khi các ô cuộn | `components/ui` không có sheet hay drawer, và phần thân của primitive dialog vốn đã cuộn được. Label, thuộc tính execution và tab cURL không thuộc W7a. Footer dính đáy giống Kestra: theo phép đo ở Task 8, Run của drawer nằm dưới mép màn hình 175px ở 1440×900 cho tới khi được làm dính đáy |
| Input BOOL không được chạm tới được gửi là `false` (`utils/inputs.ts:169-175`, được `updateDefaults` của `InputsForm.vue` áp cho mọi input chưa đặt) | Giống vậy: switch bắt đầu từ giá trị mặc định hoặc `false`, và luôn được gửi | Đúng hành vi của chính Kestra; switch không có trạng thái thứ ba |
| Panel của editor: mỗi panel có một menu chấm (ở 2.0.0-rc13: Move right, Move left, Close all panels, Keyboard shortcuts), một tay nắm để kéo và nút phóng to; tab kéo bằng HTML5 drag; thả tab lên một tab khác thì vào panel đó, đứng trước tab ấy, thả lên thân một panel thì vào dải tab của panel đó, và thả lên mép trái hoặc phải ngoài cùng của khung thì tạo panel mới ở đó; `KeepAlive` giữ các tab ẩn của panel luôn mount, và panel được key theo chỉ số | Mỗi panel có một menu "Tab actions: <title>", tác động lên tab đang chọn (Move to panel…, Split left, Split right, Close); thả tab lên mép trái hoặc phải của **bất kỳ panel nào** thì tách cạnh panel đó; mỗi panel chỉ mount tab đang chọn, và khi tập panel đổi thì các panel được mount lại (§5.1) | Một `tablist` chỉ được chứa tab, nên đặt nút menu trong từng tab sẽ phá ngữ nghĩa đó; một menu mỗi panel giữ được ngữ nghĩa mà mọi tab vẫn tới được menu của mình trong hai phím. Tách cạnh bất kỳ panel nào là mô hình của spec. React không có `KeepAlive`: trạng thái của tab nằm ngoài tab, và Task 4 giữ model và view state của Monaco qua lần mount lại (`path`, `keepCurrentModel`, `saveViewState`). Nếu một tab khác cần luôn mount thì phương án dự phòng là portal đổi cha |
| Một thanh công cụ trên các panel bật/tắt từng loại tab (Flow Code, No-code, Topology, Docs, Files, Blueprints); mở một loại thì thêm một panel mới bên phải. Một dấu chấm trên tab Code báo có sửa đổi chưa lưu | Menu "+" của mỗi panel mở lại một tab đã đóng vào chính panel đó. Sửa đổi chưa lưu là một badge ở header ("Unsaved changes") | Menu "+" để người xem chọn panel, và người dùng bàn phím tới được nó bằng Tab (FE-A11Y-3). Badge ở header nằm cạnh Save và các chỉ báo khác, điều mà một dấu chấm trên tab không làm được khi Code ở panel khác |
| Validate chạy khoảng 1 giây sau phím cuối (đo được: một `POST /flows/validate` 980 ms sau phím thứ ba trong ba phím cách nhau 150 ms). Thanh công cụ hiện "Valid" hoặc "N Error(s)" | 600 ms sau lần sửa cuối (§5.2). Số lỗi nằm trên tab Problems, và danh sách của nó đưa con trỏ tới từng lỗi | Problems là của egret (dòng ở trên); 600 ms là lựa chọn của spec, và request mới huỷ request cũ |
| Id task lặp lại được đánh dấu ở lần xuất hiện thứ hai | Được đánh dấu ở lần đầu (`tasks.say` map vào phần tử đầu tiên khớp, §15) | Kết quả của server gọi task theo id chứ không theo vị trí; bộ validate của Kestra tự báo chỗ trùng. Một path mang vị trí là thay đổi ở backend, để sau |
| Docs: ngoài task là một trình duyệt tài liệu tham khảo (tìm kiếm; Overview, Flow, Tasks, Inputs, Pebble, Examples). Trong một task là trang của plugin với breadcrumb và các tab (Overview, Properties, Examples). Nó đi theo con trỏ | Ngoài task là tài liệu cấp flow; trong task là một trang cuộn: mô tả, bảng thuộc tính và outputs. Nó cũng đi theo con trỏ (150 ms) | Cả hai đọc cùng một schema được phục vụ (§7.1); trang tài liệu plugin và ví dụ nằm ngoài W7 |
| Revisions: hai ô chọn revision cạnh nhau, mỗi ô có Restore riêng, và một ô chọn side-by-side hoặc inline phía trên một diff rộng hết trang. Restore hỏi "Are you sure you want to restore the revision 1?" và tạo một revision mới | Một danh sách version (số, ghi chú, tác giả, thời gian) cạnh khung xem từ `xl`, phía trên nó khi hẹp hơn; "Compare with" chọn bên còn lại; diff hai cột từ 600px và inline khi hẹp hơn, kèm chú thích nói rõ là kiểu nào (§5.5). Restore lưu chữ của version vào draft, và hộp xác nhận nói thứ sẽ chạy tiếp và thứ bị mất | Version là snapshot còn draft là thứ chạy (phán quyết của người dùng), nên restore là một lần lưu draft chứ không phải version mới. Danh sách mang ghi chú và tác giả mà ô chọn của Kestra giấu đi |
| STRING (và JSON) là một editor code một dòng (`ks-editor`, Monaco) | Một ô chữ (JSON: ô văn bản ở trên) | Ô native nhận `id` mà `Label` trỏ tới (FE-FORM-2), và một STRING không cần editor (đo trên 2.0.0-rc13 ở vòng sửa 1 của Task 8) |
| DATETIME là một ô chữ mở bộ chọn ngày giờ (lịch, cột giờ, Now và OK) | Ô `datetime-local` của trình duyệt, có giây | Control native nhận `id` của `Label`, dùng được bằng bàn phím sẵn, và `components/ui` không có bộ chọn ngày giờ. SELECT ở cả hai đều là dropdown (của Kestra lọc khi gõ; `ui/select` của egret thì không), và FILE ở cả hai là ô chọn file của trình duyệt |
| INT là ô số có nút − và +. Một key YAML 1.1 như `on` được ghi nhãn là `true` | Một ô số. Id của input đúng như được viết (`on`) | Ô số của chính trình duyệt; YAML của egret là 1.2, nên `on` là chuỗi |
| `monaco-yaml` 5.5.1 và `monaco-worker-manager` 2.0.1 được vá trong `node_modules` (`kestra/ui/patches/`): worker được khởi động bằng `createWebWorker({ worker })` của monaco 0.56, và bộ kiểm tra chọn thành viên `oneOf` theo `type` để gợi ý `type:` liệt kê mọi kiểu | Không vá `node_modules`. `monacoYaml.ts` đưa cho `monaco-yaml` một Monaco có `createWebWorker` khởi động worker theo cách monaco 0.56 làm; schema được phục vụ chỉ bắt buộc `id` và `type` trực tiếp, phần còn lại sau `if: type là chuỗi` (§7.1), nhờ đó mọi cách viết `type:` đều được gợi ý | Đo bằng schema của egret trong Chromium và bằng chính worker của `monaco-yaml` trong Node (W7a Task 4): nếu để nguyên, worker rơi về bản giả trong trang không có phương thức YAML nào, và gợi ý `type:` chỉ đưa 10 trên 54 cách viết của task |

Mọi khác biệt khác tìm thấy khi đối chiếu với một Kestra đang chạy (§9) được thêm vào bảng này kèm lý do.

## 3. Route và điều hướng

**Nav.** **Workflows là một nhóm Sidebar riêng**, giống Data API và vì cùng lý do: một module có nhiều phần. Nó **không** thuộc `EXPLORE_MODULES`, danh sách còn quyết định trang mở đầu của workspace. Mọi mục đều mang `feature: "workflows"`, nên cả nhóm ẩn đi khi workspace không có quyền dùng, và các route được canh bởi `RequireFeature feature="workflows"`. Nhóm có hai mục **Flows** và **Executions**. Executions là trang placeholder cho tới W7b.

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

`ListPageShell` cho tiêu đề "Flows" và hành động chính "New flow" (link tới `flows/new`). Bên dưới là `SummaryStrip`, với tổng số và số flow Active, Disabled, rồi tới bảng (FE-PAGE-1).

- **Nhóm.** Các dòng được nhóm dưới một **cây namespace** dựng phía client từ namespace của các flow, tách theo dấu `.`.
  - Mỗi nút cây hiện tên đoạn của nó và số flow bên dưới. Nút thu gọn và mở được, và trạng thái mở là tuỳ chọn của từng người xem.
  - Một namespace vừa có flow vừa có namespace con thì hiện flow của chính nó phía trên các nhánh con.
  - Cây là một `<table>` nhóm tự viết, cùng dạng với các danh sách nhóm của Explore (`ui-conventions.md` §1.5), kèm một comment giải thích (FE-PAGE-4).
- **Cột:**
  - flow id, là link tới editor của flow;
  - state, dưới dạng chip: **Active** (backend `draft` hoặc `live`) hoặc **Disabled** (phán quyết M5 ở phần đầu);
  - version: version mới nhất dạng "v{n}", hoặc "—" khi chưa có (`latest_version`);
  - thời điểm cập nhật, dạng thời gian tương đối;
  - người tạo.
- **Bộ lọc,** lưu trong URL (FE-PAGE-2):
  - tìm chữ theo flow id và namespace;
  - namespace, là một picker các namespace đã biết, tính cả các namespace con;
  - state: Active hoặc Disabled (`?state=active|disabled`).
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
- **Chuột:** tab kéo được bằng `@dnd-kit`. Thả lên một tab thì chèn trước tab đó; thả lên dải tab, sau các tab, thì thêm vào cuối; thả lên vùng mép trái hoặc phải trong thân của **bất kỳ panel nào** thì tách cạnh panel đó. Thả ở chỗ không thuộc vùng nào thì không có gì thay đổi.
- **Bàn phím (FE-A11Y-3):**
  - mọi tab có menu gồm "Move to panel…", "Split left", "Split right" và "Close". Đó là một menu "Tab actions: <title>" cho mỗi panel, tác động lên tab đang chọn của panel: một `tablist` chỉ được chứa tab, nên nút menu trong từng tab sẽ phá ngữ nghĩa tab. Chọn tab bằng phím mũi tên, rồi Tab tới menu. Phím Delete cũng đóng một tab đóng được;
  - bật bộ cảm biến bàn phím của dnd-kit, và phím mũi tên nhảy từ đích thả này sang đích thả kế tiếp;
  - dải tab là một `tablist`, điều hướng bằng phím mũi tên.
- **Lưu bố cục.**
  - Bố cục được lưu theo từng flow trong `localStorage`, với key `egret.wf.layout.<flow uuid>`. Đây là tuỳ chọn của từng người xem (FE-DATA-3).
  - Mọi lần đọc và ghi đều được bọc. Bố cục thiếu, không parse được hoặc không hợp lệ thì quay về mặc định. Một tab id lạ, ví dụ của một package sau đã bị rollback, bị bỏ qua.
  - Flow mới dùng bố cục mặc định.
- **Mặc định:** hai panel `[Code] | [Docs, Problems]`, tỷ lệ 60/40.
- **Vùng tách** được tô khi đang kéo tab: viền đứt 2px màu `primary` (3.59:1 sáng, 6.93:1 tối) trên nền `primary/10`, với "Split left" hoặc "Split right" ở gần đỉnh, tránh khỏi lớp phủ của tab đang kéo vốn đi theo con trỏ.
- **Gutter** rộng 8px với một vạch 4px, và vùng nhận con trỏ đúng bằng 8px đó (`resizeTargetMinimumSize`). Vùng nhận mặc định của thư viện (10px với chuột, 20px với cảm ứng) lấn 3px vào tab đầu của panel kế bên, và một lần kéo bắt đầu ở đó đã đổi kích thước panel (W7a Task 8).
- **Mount.** Mỗi panel chỉ mount tab đang chọn, key theo tab id. Khi tập panel đổi (tách, hoặc đóng hay chuyển làm một panel trống) thì các panel được mount lại, để kích thước luôn khớp bố cục. Vì vậy tab giữ trạng thái bên ngoài chính nó: tab Code giữ model và view state của Monaco qua lần mount lại (`path`, `keepCurrentModel`, `saveViewState`; Task 4), và trang dispose model đó khi rời flow. §2 ghi khác biệt với `KeepAlive` của Kestra.

**Registry tab.** Mỗi loại tab được đăng ký một lần bằng `registerTab({ id, title, icon, closable, render })`; `render(context)` nhận `context` mà trang đưa cho khung. W7c đăng ký Topology và W7d đăng ký Files mà không sửa khung. Tab đăng ký sau khi editor đã mount (một chunk lazy) vẫn được nhận: khung render lại, và bố cục đã lưu có nhắc tới tab đó thì giữ nó, trừ khi người xem đã đổi bố cục trước khi tab được đăng ký.

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
- Nó tìm task hoặc trigger trong cùng chứa con trỏ, rồi hiện tài liệu của task hoặc trigger đó từ cùng schema: mô tả, bảng thuộc tính (tên, kiểu, bắt buộc, mặc định, mô tả) và outputs.
  - Task hoặc trigger là một mapping có `type:` dạng chuỗi và có `id:` hoặc là một phần tử của danh sách. Vì thế `type: constant` của chính một retry (mapping nằm dưới một key, không có `id`) bị bỏ qua, như `isTaskLike` của Kestra, và một task gõ `type:` trước `id:` vẫn được nhận ra.
  - Khi văn bản chưa parse được (key thiếu `:`, dấu nháy chưa đóng), cây chịu lỗi được đọc tại chỗ đang sửa, còn ở mọi chỗ khác thì đọc bản nguồn gần nhất đã parse được.
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

### 5.3 Save, Create version và chỉ báo thay đổi

*(Viết lại 2026-10-10 theo phán quyết của người dùng về phát hiện của Task 6: execution chạy bản draft đã lưu.)*

Header chi tiết flow là flow id và namespace, chip state (Active hoặc Disabled), nhãn "Version v{n}" (hoặc "Version —"), và các chỉ báo bên dưới. Các hành động:

**Save** (`workflow:manage`).
- Gửi `PUT …/flows/{id}` với `{ source, if_updated_at }` (§7.3).
- Nếu nhận 409, một dialog giải thích draft đã bị đổi kể từ lúc mở. Dialog cho chọn "Reload" (bỏ thay đổi local, sau khi xác nhận) hoặc "Overwrite" (lưu lại mà không gửi `if_updated_at`).
- Save vẫn bật khi validate có lỗi; draft được phép không hợp lệ.
- Lưu là thay đổi thứ đang chạy: lần start qua API, lần bắn schedule, webhook hay lời gọi subflow kế tiếp sẽ dùng bản draft đã lưu. Toast của Save ghi "Saved. New runs use this version of the flow.".

**Create version** (`workflow:manage`; route backend là `POST …/flows/{id}/publish`).
- Mở một dialog có ghi chú tuỳ chọn. Dialog giải thích version là một snapshot để so sánh và khôi phục, và không làm thay đổi thứ đang chạy.
- Bị vô hiệu hoá khi còn chỉnh sửa chưa lưu ("Save first"), có lỗi validate ("Fix N errors to create a version"), request validate thất bại ("Validation unavailable", §10), hoặc validate còn đang chạy ("Validating the flow…"). Lý do hiện ngay trên control (FE-ACTION-4).

**Run** (`workflow:manage`).
- Mở form run (§6), chạy bản draft đã lưu.
- Bị vô hiệu hoá, kèm lý do hiện ra (FE-ACTION-4), khi:
  - flow đang tắt, do `state` hoặc do `disabled: true` trong YAML đã lưu ("The flow is disabled");
  - còn chỉnh sửa chưa lưu ("Save first: Run uses the saved draft");
  - bản draft đã lưu trên server không phải văn bản editor này đang giữ, vì đã đọc lại một lần lưu của người khác ("The flow changed on the server; reload to run it");
  - validate có lỗi ("Fix N errors to run"), request validate thất bại ("Validation unavailable"), hoặc validate còn đang chạy ("Validating the flow…"), để Run không bao giờ start một draft mà lỗi của nó chưa được biết.

**Delete** (`workflow:manage`).
- Đi qua `ConfirmDeleteDialog` (FE-ACTION-1), rồi chuyển về danh sách.

**Chỉ báo thay đổi** là một pill, "Changes since v{n}", trong đó v{n} là version mới nhất.
- Nó được tính bằng cách so `source` của draft đã lưu với `source` trong snapshot của version đó, từng byte (FE-STATE-4). Nếu draft được sửa về đúng nội dung của version rồi lưu, pill tắt lại. Flow chưa có version nào thì không hiện pill.
- Chỉnh sửa local chưa lưu là một chỉ báo riêng ("Unsaved changes").

Sau khi Save hoặc Create version thành công, query flow được invalidate; Create version còn invalidate thêm query versions. Cả hai xác nhận bằng toast, vì người dùng ở lại trang (FE-ACTION-2).

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

### 5.5 Revisions

*(Thêm ở vòng sửa 1 của W7a Task 7.)*

`flows/:id/revisions` liệt kê các version của flow, mới nhất trước: số, ghi chú, tác giả (`created_by_name`, hoặc "—") và thời điểm. Nó đánh dấu version **Latest**, và **Matches draft** ở version nào có chữ trùng từng byte với draft đã lưu (FE-STATE-4). Chọn một version thì thấy YAML của nó. "Compare with" so nó với một version khác hoặc với draft hiện tại (đã lưu) trong diff editor của Monaco, chữ cũ hơn bên trái. Lựa chọn và phép so nằm trong URL (`?v=`, `?compare=`; FE-PAGE-2).

**Restore to draft** là một lần Save chữ của version: `PUT` kèm `if_updated_at`, cùng hộp thoại save-cũ khi gặp 409, và không bao giờ publish. Editor chỉ nhận chữ sau khi đã lưu xong. Restore đổi thứ được chạy, nên luôn hỏi trước, và hộp xác nhận nói rõ điều đó.

**Dòng báo mất: một cách đọc có chủ ý quy tắc của người dùng.** Quy tắc yêu cầu hộp xác nhận nêu thứ bị mất "khi draft có thay đổi so với version mới nhất". Hộp xác nhận chỉ cảnh báo draft đã lưu bị bỏ khi **không version nào giữ chữ của draft đã lưu**. Khi draft trùng một version cũ hơn thì không mất gì: version đó vẫn giữ nó, và phép kiểm hẹp hơn giữ cho hộp xác nhận không báo một mất mát không có thật. Hễ dòng này hiện thì draft cũng khác version mới nhất, nên câu chữ của nó ("changes since v{latest} that no version keeps") đúng. Các sửa đổi chưa lưu trong editor luôn được nêu khi có.

**Bố cục.** Danh sách nằm cạnh khung xem từ `xl`, và phía trên nó khi hẹp hơn: với `lg`, màn hình 1024px chỉ còn cho diff 222px. Diff hiện hai cột khi khung của nó rộng ít nhất 600px, và inline khi hẹp hơn; khi đó chú thích ghi "v1 → Current draft" kèm nghĩa của − và +, thay vì gọi tên cột trái và cột phải. Để Monaco tự quyết thì nó chuyển inline dưới 900px, đúng bằng độ rộng khung trên màn hình 1440px (W7a Task 8).

Khi một lần save của người khác đã được đọc về (draft đã lưu trên server không phải chữ editor này đang giữ), tab không so với thứ nó không bảo đảm được: nó báo flow đã đổi trên server, không đánh dấu "Matches draft", không diff với draft, và tắt Restore cho tới khi tải lại trang.

## 6. Run

### 6.1 Form

Run đọc `inputs:` của **bản draft đã lưu** (`FlowDto.source`), cũng là thứ server khởi chạy (phán quyết của người dùng, 2026-10-10). Run không bao giờ được mở khi còn chỉnh sửa chưa lưu (§5.3), nên form và server luôn đọc cùng một nội dung.

- **Khung chứa.** Flow không có input thì chạy ngay sau một bước xác nhận. Từ một đến ba input, tất cả đều là kiểu đơn, thì mở dialog. Còn lại mở một drawer bên phải. Ở cả hai, Cancel và Run nằm trong footer luôn hiện trong khi các ô cuộn, và phần đệm cuộn của thân dài hơn footer đó (45px khi rộng, 85px khi xếp chồng dưới `sm`), nên ô nhận focus được cuộn ra khỏi chỗ footer che (WCAG 2.4.11).
- **Request.** Một lần start bằng JSON đi ra với `Content-Type: application/json`, do `customInstance` của client sinh ra (`api/mutator.ts`) đặt cho mọi body là chuỗi mà không ghi kiểu. Route nhận JSON hoặc multipart, nên client sinh ra không ghi kiểu nào, và `fetch` của trình duyệt gắn nhãn `text/plain` cho body là chuỗi, thứ route từ chối bằng 415; cho tới lần chạy end-to-end của Task 8, mọi Run không có input FILE đều hỏng theo cách đó. Stub fetch của vitest chưa từng nhìn header.
- **Control theo kiểu input** (`models/input.rs`):

| Kiểu input | Control |
|---|---|
| STRING, EMAIL, URI | ô chữ; EMAIL và URI được kiểm trong form |
| INT, FLOAT | ô số |
| BOOL | switch |
| DATETIME, DATE, TIME | picker ngày/giờ (giá trị ISO) |
| DURATION | ô chữ, placeholder ISO-8601 (`PT1H`), kiểm theo cú pháp `Duration` của Java |
| SELECT | select; MULTISELECT là danh sách checkbox (cả hai lấy từ `values`; §2) |
| ARRAY | danh sách các dòng, mỗi dòng theo `itemType` |
| JSON, YAML | một ô văn bản font monospace (§2) |
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

`dependsOn` của Kestra (input được render lại theo input khác) chưa hỗ trợ trong W7a. Input có `dependsOn` hiện danh sách `values` tĩnh của nó, kèm một ghi chú. Backend cũng không tính `dependsOn` (§15).

### 6.3 Trang execution tối thiểu

`executions/:id` hiện:
- id execution, flow, namespace, chip state, thời điểm bắt đầu và thời lượng;
- link quay về flow;
- các task run, dạng danh sách đơn giản gồm id và state.

Khi execution chưa kết thúc, trang poll `GET …/executions/{id}` mỗi 2 giây. Một lần poll thất bại thì trang giữ câu trả lời gần nhất, báo việc làm mới thất bại kèm nút Retry, và vẫn tiếp tục poll với backoff (thời gian chờ tăng theo khoảng thời gian kể từ câu trả lời tốt gần nhất, tối đa 30 giây) cho tới khi execution kết thúc. W7b sẽ thay trang này.

## 7. Bổ sung ở backend

### 7.1 `GET /api/ws/{ws}/workflows/schema`

- **Quyền:** `workflow:read`.
- **Phản hồi:** một JSON Schema (draft 2020-12) cho một tài liệu flow:
  - khung flow (`id`, `namespace`, `description`, `labels`, `inputs`, `variables`, `tasks`, `errors`, `finally`, `triggers`, `pluginDefaults`, `concurrency`, `disabled`, …);
  - một `oneOf` theo kiểu task, chọn theo `type`, cho `tasks`/`errors`/`finally` (đệ quy cho task con của task flowable), và tương tự theo kiểu trigger cho `triggers`.
- **Mục của mỗi kiểu** gồm mô tả, `properties` với kiểu, bắt buộc, mặc định và mô tả, và `outputs` (dưới dạng phần mở rộng `x-egret-outputs`, dùng cho tab Docs).
  - `required` chỉ giữ `id` và `type`; các thuộc tính bắt buộc còn lại của kiểu nằm ở `then.required`, sau `if: { required: [type], properties: { type: { type: string } } }`. Tập tài liệu được chấp nhận không đổi, và nhờ vậy `monaco-yaml` gợi ý đủ mọi cách viết `type:` (`spec/schema/README.md` §5, W7a Task 4).
- **Nguồn** *(phán quyết của người dùng, 2026-10-10)*. Schema được ghép lúc build từ ba đầu vào đã commit:
  1. **`kestra-flow-schema.json`.** Flow JSON schema của Kestra, xuất một lần từ image `kestra/kestra` trên máy (`GET /api/v1/plugins/schemas/flow`). Phiên bản Kestra được ghi trong file, và lệnh xuất nằm trong comment đầu file hoặc README.
  2. **`egret-task-schemas.json`.** Schema viết bằng tay cho các type Kestra không có: `egret.*`, `core.http`, `core.slack`, và các type script nếu định nghĩa của Kestra không khớp với egret.
  3. **Registry** (`spec/registry.rs`), cùng tập flowable trong `models/task.rs`.
- **Lọc.**
  - Chỉ giữ các type egret chấp nhận, theo FQCN chính thức, kèm alias của chúng.
  - Bỏ các thuộc tính egret parse nhưng bỏ qua, để gợi ý không bao giờ đề xuất thứ egret lặng lẽ bỏ đi. Ví dụ là các thuộc tính gốc của task mà egret bỏ qua trên task không phải script (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`).
  - Task đầu tiên của plan đo, với từng type, cách egret xử lý từng thuộc tính Kestra mô tả (tôn trọng, bỏ qua hay từ chối) trước khi chốt bộ lọc.
- **Test.**
  - Một test Rust khẳng định tập giá trị `type` trong schema trả về bằng đúng tập registry chấp nhận với cùng tổ hợp feature.
  - Một test thứ hai khẳng định không thuộc tính nào được trả về là thuộc tính egret đã biết là bỏ qua.
- **Thứ tự ưu tiên.** Với chín class JDBC, thuộc tính trong `catalog.json` được giữ ở những chỗ khác với bản xuất của Kestra.
- **Feature.**
  - Tắt `workflow_kestra_plugins` thì không có các kiểu plugin Kestra.
  - Tắt `workflow_containers` thì không có các kiểu container.
  - Phản hồi khác nhau theo workspace, nên nó mang một `ETag` mạnh và `Cache-Control: private, no-cache`.
- **Alias.** Các alias ngắn mà registry chấp nhận được đưa vào enum `type`, kèm tên FQCN chính thức trong phần mô tả.

### 7.2 `POST /api/ws/{ws}/workflows/flows/validate`

- **Quyền:** `workflow:manage`, giống route validate theo id hiện có.
- **Body:** `{ "source": string }`, cùng giới hạn kích thước body như route tạo.
- **Phản hồi:** cùng `ValidationReportDto` như `POST …/flows/{id}/validate`, tính bằng cùng một hàm. Không ghi gì.
- **Lỗi:** giống route hiện có, YAML không parse được trả 400, với thông báo của parser có dòng và cột. Editor biến thông báo đó thành một marker tại vị trí ấy và một mục trong Problems. Tài liệu parse được nhưng không validate được thì trả 200 kèm kết quả.

### 7.3 `if_updated_at` trong `PUT /flows/{id}`

`UpdateFlowRequest` có thêm trường tuỳ chọn `if_updated_at` (RFC 3339).
- Khi có trường này mà nó khác `updated_at` của hàng, lần cập nhật bị từ chối với 409, "the draft was changed since you opened it", và không có gì được ghi. `code` trong body là `draft_changed` *(W7a Task 5, vòng sửa 1)*; 409 còn lại của route này, khi namespace và id đã thuộc flow khác, không có code. Client rẽ nhánh theo code, không bao giờ theo câu chữ.
- So sánh và ghi nằm trong một câu lệnh (`UPDATE … WHERE id = $1 AND updated_at = $2`), nên hai lần lưu đồng thời không thể cùng qua.
- Không có trường này thì route giữ nguyên hành vi như hiện nay.
- Mọi nơi ghi `flows.updated_at` được liệt kê trong doc comment của route, vì ý nghĩa của điều kiện tiên quyết này dựa vào chúng (rule FE-STATE-4, áp cho một điều kiện tiên quyết).

### 7.4 Tên người tạo

`FlowListItemDto`, `FlowDto` và `FlowVersionDto` có thêm `created_by_name: Option<String>`. Đó là tên hiển thị của người đã tạo flow hoặc version, join ngay trong câu truy vấn đọc hàng đó. Nó là null khi `created_by` null hoặc người dùng không còn tồn tại. UI hiện tên, hoặc "—" khi null, và không bao giờ lấy danh sách thành viên workspace cho việc này.

`openapi.json` và client được sinh lại bằng `npm run gen:api` (FE-DATA-1).

### 7.5 `can_manage` trên `FlowDto`

*(Thêm ở W7a Task 5.)* `GET …/flows/{id}` trả `can_manage: bool`: người gọi có `workflow:manage` trong workspace này hay không, do PDP quyết định bằng đúng phép kiểm tra các route ghi dùng. Khi nó là false, editor mở ở chế độ chỉ đọc, không có Save, Create version, Run hay Delete (§5.2). `workflow:manage` là quyền theo workspace, còn `/auth/me` chỉ liệt kê quyền platform, nên frontend không có cách nào khác để biết; cách này theo `can_manage` mà các đối tượng workspace khác đã mang (dashboard, schedule, app). Create và update trả `true`, vì người gọi vừa qua phép kiểm tra. PDP lỗi là lỗi, không phải `false`. Đây chỉ là gợi ý cho UI: mọi route ghi vẫn tự kiểm tra.

Trang tạo flow chưa có flow để hỏi. Lời gọi validate không-id của chính nó làm đúng phép kiểm tra `workflow:manage`, nên một 403 từ lời gọi đó, hoặc từ Save, chuyển trang sang chỉ đọc, với banner "You need permission to manage flows" và không có Save. Nút "New flow" ở danh sách vẫn không bị chặn: danh sách không mang tín hiệu quyền nào mà không phải đoán.

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

State của flow dùng cùng chip, chỉ cho biết flow có chạy hay không (phán quyết M5 ở phần đầu): `draft` và `live` đều là **Active** trên `success`, còn `disabled` là **Disabled** trên `muted-foreground`. Số version không bao giờ nằm trong chip.

## 9. Đối chiếu với Kestra

Trong lúc cài, một Kestra tạm chạy từ image `kestra/kestra` có sẵn trên máy (`server local`), ở một cổng còn trống. Nó bị xoá sau khi xong và không bao giờ đụng tới dev stack của egret. Editor (panel, chuyển và tách tab, panel docs, nhịp validate), Revisions và form run được so về cấu trúc và tương tác (FE-REF-2), kèm ảnh chụp hoặc ghi chép DOM trong báo cáo của task. Khác biệt được đưa vào §2.

## 10. Xử lý lỗi

- Mọi khung hiển thị dữ liệu có đủ trạng thái loading, rỗng và lỗi (FE-STATE-1).
- Mutation thất bại thì hiện thông báo của server (FE-ACTION-3).
- 403 do feature hoặc quyền thì hiện trang forbidden có sẵn.
- 404 với một flow thì hiện "Flow not found" kèm link về danh sách.
- Tải schema thất bại: editor vẫn dùng được nhưng không có gợi ý, và hiện "Autocomplete unavailable" kèm nút thử lại. Validate trên server vẫn chạy.
- Request validate thất bại: Problems hiện lỗi (§5.2). Create version và Run vẫn bị vô hiệu hoá với lý do "Validation unavailable".

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
  - Save, Create version và Run được bật hay bị vô hiệu hoá, kèm lý do;
  - dialog 409;
  - chỉ báo thay đổi tắt khi nội dung khớp trở lại;
  - control của từng kiểu input và giá trị nó gửi đi;
  - lỗi theo ô từ server, và giá trị đã nhập được giữ;
  - restore trong Revisions kèm bước xác nhận.
- **Playwright,** trên một stack local, không bao giờ trên dev stack dùng chung (FE-TEST-2), trong `frontend/tests/e2e/workflows-flows.spec.ts` (bật bằng `EGRET_E2E_WORKFLOWS=1`; các lệnh dựng stack nằm ở đầu file):
  - tạo flow → gợi ý xuất hiện → một lỗi được đánh dấu và liệt kê → Save → Run trên draft đã lưu → trang execution tới SUCCESS → Create version kèm ghi chú;
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
5. Header chi tiết flow: Save với `if_updated_at`, Create version, Delete, chỉ báo thay đổi, và trang tạo flow.
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
- Input có `dependsOn` là tĩnh (§6.2). **Backend cũng không tính `dependsOn` của input** (kiểm ở W7a Task 6): `models/input.rs` parse và giữ nó, nhưng không đoạn code nào đọc `Input::depends_on`. Mọi `depends_on` khác trong `egret-workflow` thuộc Flow trigger hoặc task DAG, và không có endpoint render lại input như `…/executions/validate` của Kestra.
- **Backend không kiểm giá trị input nào** (kiểm ở W7a Task 6). `start_flow_execution` nhận object `inputs` nguyên như được gửi; `with_declared_defaults` (`worker/render.rs`) parse một chuỗi đúng một lần thành JSON cho INT, FLOAT, BOOL, JSON, ARRAY và MULTISELECT, giữ nguyên chuỗi nếu parse lỗi, và điền `defaults:` cho input bị bỏ trống. Nó không kiểm `required`, không kiểm kiểu, không kiểm giá trị SELECT có thuộc danh sách hay không; `Input` đã parse của egret không có trường `values`. Chỉ input FILE bị từ chối theo từng input (đuôi file, kích thước, upload). Vì vậy form run là nơi duy nhất kiểm `required`, số, JSON, DATETIME, EMAIL, URI và DURATION; start qua API với giá trị sai vẫn chạy.
- Kết quả validate có `path` không map được thì hiện ở dòng 1 (§5.2).
  - Path của server gọi phần tử danh sách theo id (`tasks.<id>.…`, `inputs.<id>`). Id lặp lại thì map vào phần tử đầu tiên khớp được phần còn lại của path; khi mọi phần tử đều khớp (`tasks.dup.type`) thì là phần tử đầu tiên. Task con có id trùng tên một thuộc tính của task cha, và id input có dấu chấm, được giải bằng cách thử từng cách đọc.
  - Lỗi parse mà server không định vị được thì hiện ở dòng 1, kèm thông điệp và không có path.
  - Path trỏ vào phần văn bản chưa từng parse được (chưa có bản nguồn nào trước đó parse được) thì map về dòng 1 cho tới khi tài liệu parse được.
- **Một số lỗi chỉ là cảnh báo, hoặc không là gì, ở server** (kiểm ở W7a Task 8). Kiểu task lạ là cảnh báo `unknown_task_type`, nên Create version và Run vẫn bật; một `core.log.Log` thiếu `message` vẫn validate sạch ở server, chỉ schema đánh dấu nó. Vì vậy lỗi trong test e2e là một id task bị trùng.
- **Độ tương phản đo ở vòng sửa 1 của Task 8, sáng và tối,** trên editor, Revisions và form run: mọi chữ đạt 4.5:1, trừ ba thứ không phải token của package này.
  - Chữ của nút primary (trắng trên `--primary`, "Create version", "Run") là 3.5:1 ở chế độ sáng. Đó là Button của cả app, dùng ở mọi trang.
  - Màu nền dòng đổi của diff trong theme `vs` và `vs-dark` gốc của Monaco hạ chữ token trên dòng đổi xuống 3.35–4.27:1 (4.77 và 6.31 trên nền thường). Một theme Monaco riêng sẽ sửa được; app chưa có theme nào như vậy.
  - Khi đứng yên, vạch gutter và viền trên của footer là `--border` của app (1.25–1.32:1), như mọi đường chia. Gutter chuyển sang `primary` khi hover và focus.
  - Badge Problems ở chế độ tối từng là 2.83:1 (gần trắng trên `--destructive` đã sáng lên); giờ chữ số ở đó màu tối (6.85:1).
- Tab Problems được đặt tên "Problems, N errors" bằng `aria-label` (`TabDefinition.badgeLabel`): Chromium từng đặt tên là "Problems , N errors", nối số lỗi ẩn bằng một dấu cách.
- **Một thay đổi bố cục được ghi 300 ms sau thay đổi cuối,** và cả khi có `pagehide`; trình duyệt bị giết (chứ không đóng) trong 300 ms đó thì mất thay đổi ấy.
