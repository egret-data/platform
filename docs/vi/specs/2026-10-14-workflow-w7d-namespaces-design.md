# W7d — UI Workflows, phần 4: namespace (danh sách, chi tiết, file, KV, dependencies)

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-14-workflow-w7d-namespaces-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-14.

**Phụ thuộc:**
- W7a (#659): khu vực Workflows, editor nhiều panel và sổ đăng ký tab của nó, mẫu dialog lưu bị cũ (`if_updated_at`, 409 `draft_changed`).
- W7b (#662): danh sách execution với `namespace_prefix`.
- W7c (#667): các phần bố cục và vẽ của `TopologyView` (xyflow + dagre), tải lười.
- W9b: file của namespace và các route của chúng.
- W9c: kho KV và các route của nó.

Bộ rule frontend `docs/en/frontend-rules.md` ràng buộc mọi dòng code frontend.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §8 (UI của Kestra làm tham chiếu) và §9 dòng W7. **Spec W7a** §5.1 đã dành tab Files của editor cho gói này.

**Đã chốt với người dùng, 2026-10-14:**
- **Chi tiết namespace có các tab của Kestra OSS, gồm cả Dependencies:** Overview, Flows, Executions, Dependencies, Files, KV. Các tab EE của Kestra (Secrets, Variables, Policies, History, Audit logs, Plugin defaults) được bỏ ra.
- **Namespace đến từ flow, file namespace và key KV.** Một namespace được liệt kê khi nó có một flow, một file namespace còn sống hoặc một key KV còn sống, và mọi namespace cha theo dấu chấm cũng được liệt kê. Không có bảng namespace và không có migration cho nó. Kestra OSS chỉ liệt kê namespace có flow; egret còn liệt kê cả namespace chỉ chứa file hoặc key, thứ mà một task như `UploadFiles` có thể tạo ra.
- **Một gói,** không tách.
- **Trong editor của flow, mỗi file namespace mở thành một tab riêng** trong khung nhiều panel của W7a, cạnh Code, như ở Kestra. Lưu flow và lưu file là hai việc riêng. Có một thao tác "Save all files", và các tab file sống sót qua một layout đã lưu. Một lần lưu file gửi kèm revision mà nó đã đọc và bị từ chối bằng 409 nếu file đã đổi trong lúc đó. Lưu của Kestra là last-write-wins.

## 1. Vì sao

W9b và W9c đã cho egret file namespace và kho KV, nhưng chỉ chạm tới được qua API và qua task. Người dùng không thể thấy namespace nào tồn tại hay duyệt file của chúng. Không ai sửa được một script mà `namespaceFiles` của flow nạp, hay quản lý một key mà `{{ kv('…') }}` đọc, nếu không dùng `curl`. Người dùng Kestra kỳ vọng một trang Namespaces và một panel Files cạnh YAML của flow.

## 2. Khác biệt so với Kestra

| Kestra (OSS) | egret | Vì sao |
|---|---|---|
| Namespace chỉ suy ra từ flow (`GET /namespaces/search`) | Từ flow, file namespace còn sống và key KV còn sống, cộng các namespace cha | Một namespace có thể chứa file hoặc key mà không có flow (phán quyết của người dùng) |
| Overview là dashboard execution, hoặc màn hình "no executions" | Số đếm (flow, file, key), execution gần đây, namespace con | egret không có biểu đồ dashboard |
| Lưu file namespace là last-write-wins | So sánh rồi ghi theo revision của file; 409 mở Reload hoặc Overwrite | Hai người cùng sửa một file không được ghi đè nhau trong im lặng (rule của W7a cho flow) |
| Đồ thị dependencies qua `GET /namespaces/{ns}/dependencies` | Cùng ý tưởng, dựng từ draft (§3.4) | egret không có chỉ mục dependency được lưu |
| Tạo namespace (EE) | Không: một namespace tồn tại khi có thứ gì đó nằm trong nó | Ngang bằng OSS |

Phần so sánh ở §9 bổ sung các khác biệt đo được.

## 3. Backend

### 3.1 Liệt kê namespace

`GET /api/ws/{workspace}/workflows/namespaces?prefix=` yêu cầu `workflow:read`.

- **Trả về gì.** Hợp của ba nguồn, mỗi nguồn giới hạn trong workspace:
  - namespace của các flow chưa bị xoá (`deleted_at IS NULL`);
  - namespace có ít nhất một file namespace còn sống (revision cao nhất của một path không phải `deleted`);
  - namespace có ít nhất một key KV còn sống.

  Mọi namespace cha theo dấu chấm được thêm vào tập đó.
- **Mỗi dòng:** `{namespace, flows, files, kv_keys, own}`. `own` là false với một namespace chỉ hiện diện vì là cha.
- **Thứ tự và bộ lọc:** các dòng được sắp theo namespace. `prefix` lọc bằng rule neo theo dấu chấm mà W7b dùng cho `namespace_prefix`.
- **Truy vấn:** một câu SQL trên các bảng hiện có; không có migration.

`GET /api/ws/{workspace}/workflows/namespaces/{ns}` trả về cùng dòng đó cho một namespace, hoặc 404 khi không nguồn nào biết nó và nó không phải cha của namespace nào. Overview dùng route này.

### 3.2 Ghi file có kiểm tra revision

`POST …/namespaces/{ns}/files` (W9b) có thêm tham số query tuỳ chọn `if_revision`.

- **Khi có nó:** lần ghi chỉ commit nếu revision hiện tại của path bằng giá trị đó. Nếu không, nó trả 409 với `code: "file_changed"`, như `draft_changed` của W7a. `if_revision=0` nghĩa là "path không được tồn tại".
- **Khi không có:** hành vi không đổi, nên các caller và task hiện có không bị ảnh hưởng.
- **Tính nguyên tử:** việc kiểm tra và việc ghi là một câu lệnh dưới khoá theo path sẵn có của W9b.

### 3.3 Không đổi

Mọi route file của W9b và route KV của W9c giữ nguyên quyền của chúng: đọc cần `workflow:read`, còn ghi, xoá và purge cần `workflow:manage`.

### 3.4 Dependencies

`GET /api/ws/{workspace}/workflows/namespaces/{ns}/dependencies` yêu cầu `workflow:read`.

- **Node:** các flow của namespace và của các namespace con (neo theo dấu chấm), cộng mọi flow ở nơi khác trong workspace có cạnh trực tiếp tới hoặc từ một trong số đó. Mỗi node là `{id, namespace, flow_id, disabled, in_namespace}`.
- **Cạnh:** `{source, target, kind}`, trong đó `kind` là:
  - `subflow`: một task `Subflow` ở bất kỳ đâu trong cây task của flow, kể cả `errors:`, có `namespace` và `flowId` là literal gọi tên một flow có thật;
  - `flow_trigger`: một trigger `Flow` có điều kiện gọi tên flow nguồn, từ `workflow.flow_listeners` và các điều kiện của trigger.
- **Đích động:** một `Subflow` có đích là biểu thức không vẽ cạnh, và node của nó mang `dynamic_subflows: n`.
- **Nguồn sự thật:** dựng từ các draft đã lưu của flow, thứ mà mọi execution chạy (phán quyết W7a). Không có chỉ mục được lưu.
- **Giới hạn:** đọc hơn 2.000 flow thì trả 422 kèm một thông báo.

## 4. Frontend

Mọi thứ nằm trong khu vực Workflows tải lười của W7a. Thêm một mục nav Workflows "Namespaces".

### 4.1 Danh sách namespace

Trang là `/explore/:workspace/workflows/namespaces`.

- **Cây:** namespace được vẽ thành cây theo dấu chấm, kèm số flow, file và key của mỗi node. Node chỉ là cha thì bị làm mờ.
- **Bộ lọc:** bộ lọc prefix, giữ trong URL (FE-PAGE-2).
- **Trạng thái:** skeleton khi đang tải (FE-STATE-2) và một trạng thái rỗng.
- **Điều hướng:** chọn một node mở chi tiết của nó.

### 4.2 Chi tiết namespace

Trang là `/explore/:workspace/workflows/namespaces/:namespace/:tab`, với các tab dọc theo thứ tự của Kestra: Overview, Flows, Executions, Dependencies, Files, KV. Tab là một phần của URL, và tab không biết thì chuyển hướng về Overview. Breadcrumb hiện từng đoạn của namespace.

- **Overview:** số đếm, 10 execution gần nhất (danh sách của W7b với `namespace_prefix`), và các namespace con trực tiếp.
- **Flows** và **Executions:** danh sách flow của W7a và danh sách execution của W7b, với bộ lọc namespace cố định là namespace này và các con của nó.
- **Dependencies:** một đồ thị các flow vẽ bằng các phần xyflow và dagre của W7c.
  - Cạnh `subflow` là nét liền và cạnh `flow_trigger` là nét đứt.
  - Flow ngoài namespace bị làm mờ.
  - Chọn một node mở flow đó.
  - Có điều hướng bàn phím và một chế độ xem danh sách, như `TopologyView` của W7c.
- **Files:** `NamespaceFiles`, §4.3.
- **KV:** §4.4.

### 4.3 Files

`NamespaceFiles` là một component dùng ở hai nơi: tab Files của namespace và tab Files của editor flow.

**Cây file**
- Tạo một file hoặc một thư mục.
- Import một file, một thư mục hoặc một `.zip` (server giải nén, như W9b làm). Export namespace thành zip.
- Đổi tên, và di chuyển bằng kéo thả. Mọi thao tác kéo đều có tương đương bàn phím ("Move to…").
- Xoá, kèm xác nhận nêu rõ bao nhiêu file sẽ mất.
- Tìm kiếm, qua route tìm kiếm của W9b.
- Xem các revision của một file và khôi phục một revision. Khôi phục ghi nội dung của revision đó thành một revision mới.

**Mở một file**
- File văn bản mở trong Monaco, với ngôn ngữ lấy từ phần mở rộng.
- Ảnh hiện bản xem trước.
- File nhị phân khác chỉ cho tải xuống.

**Lưu**
- Một lần lưu gửi `if_revision`.
- Một 409 `file_changed` mở dialog lưu bị cũ của W7a: Reload hoặc Overwrite. Overwrite gửi lại không kèm `if_revision`.
- File chưa lưu hiện một chấm trên tab, và đóng nó thì hỏi trước.

**Trên trang namespace**
- Cây ở bên trái và các file đang mở là tab ở bên phải.
- Các tab đang mở được nhớ theo từng namespace trong `localStorage`.

**Trong editor flow**
- Một tab Files được đăng ký qua `registerTab`, cùng cách W7c đã đăng ký Topology: một stub có thân tải lười. Nó hiện cây của namespace của flow.
- Mở một file thêm một tab `file:<path>` vào khung nhiều panel, trong panel đang chứa Code.
- Một tab file lưu bằng Ctrl/Cmd+S. "Save all files" lưu mọi tab file chưa lưu.
- Các tab file được khôi phục từ layout đã lưu.
- Nút Save của chính flow không lưu file, và lưu một file không lưu flow.

### 4.4 KV

**Bảng**
- Cột: key, type, description, last modified, expiration. Tất cả đều sắp xếp được.
- Một bộ lọc key, giữ trong URL.

**Hành động trên dòng**
- Xem giá trị.
- Sửa.
- Sao chép `{{ kv('key') }}`.
- Xoá. Các dòng được chọn có thể xoá cùng nhau bằng bulk delete của W9c.

**Drawer tạo và sửa**
- Trường: key (khoá khi sửa), type, value, description, và expiration dưới dạng TTL.
- Kiểu: STRING, NUMBER, BOOLEAN, DATETIME, DATE, DURATION, JSON. Mỗi kiểu có control riêng; JSON dùng Monaco.
- Giá trị được kiểm trong form bằng cùng các rule mà server áp dụng.
- Tạo một key đã tồn tại thì hỏi trước khi ghi đè.

**Key kế thừa**
- Một drawer "Inherited KV" liệt kê các key kế thừa từ namespace cha, qua route kế thừa của W9c. Cha gần nhất thắng.

### 4.5 Quyền

Người xem có `workflow:read` mà không có `workflow:manage` thấy mọi thứ. Mọi thao tác ghi (tạo, import, đổi tên, di chuyển, xoá, lưu, ghi KV) bị vô hiệu kèm lý do hiển thị (FE-ACTION). Danh sách flow và execution giữ rule riêng của chúng từ W7a và W7b.

### 4.6 Chunk

- Monaco chỉ tải trong chunk của file editor.
- xyflow và dagre chỉ tải trong chunk Dependencies, dùng lại chunk Topology của W7c khi có thể.
- `test:bundle:workflows` có thêm marker cho các module mới.

## 5. Lỗi

**Namespace**
- Một namespace không biết hiện "not found" kèm liên kết quay lại danh sách.

**Files**
- Một đoạn `_files` bị từ chối bằng thông báo của server (rule của W9b).
- Một file vượt giới hạn kích thước hiện thông báo của 413.
- Xung đột revision mở dialog của §4.3.
- Lỗi mạng hiện điều gì đã xảy ra và cần làm gì (FE-COPY-2).

**KV**
- Giá trị sai kiểu bị từ chối ngay trong form trước khi gửi request.

**Dependencies**
- Quá 2.000 flow thì có 422 kèm thông báo của nó.

## 6. Test

**Rust**
- Liệt kê:
  - từng nguồn trong ba nguồn, và các cha được thêm theo dấu chấm;
  - namespace của workspace khác vắng mặt;
  - flow đã xoá, file đã xoá và key đã xoá hoặc hết hạn không được đếm;
  - số đếm chính xác;
  - bộ lọc prefix neo theo dấu chấm;
  - route chi tiết trả 404.
- `if_revision`:
  - khớp thì commit;
  - lệch thì trả 409 `file_changed` và không ghi gì;
  - `0` trên path đã tồn tại bị từ chối;
  - không có nó thì hành vi không đổi;
  - hai lần ghi đồng thời cùng revision: đúng một lần commit.
- Dependencies:
  - một `Subflow` literal, kể cả một cái dưới `errors:`;
  - một `Subflow` động được đếm, không được vẽ;
  - một trigger `Flow`;
  - cạnh vào và ra khỏi namespace;
  - các namespace con của namespace được tính;
  - giới hạn flow;
  - giới hạn theo workspace.
- Mutation trên phần giới hạn và trên phần kiểm tra revision.

**Vitest**
- Cây namespace từ một danh sách phẳng.
- Tab và URL.
- Files:
  - thao tác cây, import và export;
  - dialog 409;
  - một tab file trong editor: mở, chấm dirty, Ctrl/Cmd+S, lưu tất cả, khôi phục từ layout đã lưu;
  - xử lý nhị phân và ảnh.
- Form KV cho mọi kiểu, bulk delete và key kế thừa.
- Trình xem chỉ đọc.

**Playwright,** trên một stack riêng, không bao giờ trên stack dev dùng chung:
1. Tạo một flow trong `company.team`, và một flow trong `company.other` gọi nó qua một `Subflow`.
2. Trên trang namespace, tạo `scripts/a.py`, sửa và lưu nó, đổi tên nó, export zip.
3. Tạo một key JSON và một key DATE, và thấy các key kế thừa từ `company`.
4. Trong editor flow, mở `a.py` ở tab Files cạnh Code, sửa nó, Ctrl/Cmd+S.
5. Thực hiện lần lưu thứ hai từ một phiên khác, và thấy dialog 409.
6. Mở Dependencies và thấy cạnh `subflow`.

Các phép đo DOM bao gồm chế độ sáng và tối.

## 7. W7d không làm gì

- Tạo và xoá namespace, và mô tả namespace (EE).
- Secrets, variables, plugin defaults, policies, history và audit logs (EE).
- Dashboard execution có biểu đồ.
- Sửa flow từ tab Files: import `_flows/` không được hỗ trợ, và W9b từ chối chúng.

## 8. Các rule gói này phải đáp ứng

- Bộ rule frontend, mọi rule.
- Không thêm dependency mới trừ khi plan nêu tên; Monaco, xyflow và dagre hiện có được dùng lại.
- Chỉ sinh lại client bằng `npm run gen:api`.

## 9. So sánh với Kestra

Kịch bản ở §6 chạy trên image Kestra cục bộ. Mọi khác biệt có chủ ý được ghi ở §2.

## 10. Giới hạn đã biết

- Dependencies đọc draft của mọi flow trong mỗi request, chặn ở 2.000 flow. Workspace có nhiều flow hơn sẽ nhận 422.
- Một `Subflow` có đích là biểu thức không có cạnh.
