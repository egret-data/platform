# W7d — UI Workflows, phần 4: namespace (danh sách, chi tiết, file, KV, dependencies)

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-14-workflow-w7d-namespaces-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** đã hiện thực, 2026-10-14. Đã đo sống trên một stack riêng (database mới, RustFS riêng, `serve` và Vite trên cổng riêng): danh sách và chi tiết namespace, tab Files với kéo thả thật và import thư mục thật qua `webkitdirectory`, Ctrl/Cmd+S theo từng editor với Code và hai tab file đặt cạnh nhau (chứng minh bằng các request được gửi), dialog 409 `file_changed` từ một phiên thứ hai, câu hỏi khi đổi namespace, KV (JSON, DATE, một NUMBER có số mũ đọc lại đủ mọi chữ số, key kế thừa) và cạnh `subflow`, với các editor được đo ở chế độ sáng và tối (`frontend/tests/e2e/workflows-namespaces.spec.ts`). Chế độ tối từng có một lỗi, do chính lần chạy đó tìm ra và được sửa ở vòng review cuối: theme của Monaco là toàn cục và một editor không nêu theme đặt nó về sáng, nên một tab file làm Code bên cạnh thành sáng. Nay mọi editor theo chế độ của app (`MonacoEditor` đặt mặc định), và lần chạy khẳng định nền editor tối trên trang namespace, cho Code cạnh hai tab file, và sau khi một revision mở trong drawer. Các khác biệt so với một Kestra đang chạy nằm ở §2.

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

**Đã đo trên một Kestra đang chạy, 2026-10-14 (Task 9, §9).** Image cục bộ `kestra/kestra` `28f59bf175e1` (Kestra 2.0 nightly, `server local`, trong một container dùng một lần) chạy kịch bản của §6: `company.team.child`, `company.other.caller` gọi nó qua một `Subflow`, một trigger `Flow` trên `child`, một file namespace, các key KV của từng kiểu. Cùng các request đó được gửi tới egret. Mỗi dòng nói rõ nó được **quan sát** trên image đang chạy (API hoặc UI của nó) hay được **đọc** trong mã nguồn Kestra.

| Kestra | egret | Vì sao |
|---|---|---|
| Quan sát: `GET /namespaces/search` liệt kê namespace có flow và các cha của chúng (`company`, `company.other`, `company.team`); một namespace chỉ chứa một file (`w7d.filesonly`) hoặc chỉ một key KV (`w7d.kvonly`) không được liệt kê | Được liệt kê, với `own: true` và số đếm của nó; một namespace chỉ chứa một thư mục còn sống cũng được liệt kê (`own: true`, `files: 0`) | Một namespace có thể chứa file hoặc key mà không có flow (phán quyết của người dùng) |
| Quan sát: `GET /namespaces/nope.nothing` trả 200 `{"id":"nope.nothing"}` | 404, và trang nói "Namespace not found" kèm liên kết quay lại | Một liên kết tới thứ không có thì phải nói vậy |
| Quan sát: các tab của trang namespace là Overview, Edit (EE), Flows, Executions, Dependencies, Secrets, Assets, Variables, Policies (EE), **KV Store, Namespace Files**, Revisions, Audit logs (EE); đọc: `ORDER` trong `ui/src/components/namespaces/utils/useHelpers.ts` đặt `kv` trước `files` | Overview, Flows, Executions, Dependencies, **Files, KV** | Thứ tự người dùng đã chốt (2026-10-14); image nightly này đặt KV trước |
| Đọc: tab Overview là `Dashboard` execution, hoặc `NoExecutions` khi namespace không có execution nào (`ui/src/components/namespaces/components/NamespaceOverview.vue`) | Số đếm (flow, file, key), execution gần đây, namespace con | egret không có biểu đồ dashboard |
| Quan sát: hai lần ghi một file trả 200 và 200, lần thứ hai thắng; tham số `if_revision` bị bỏ qua (200); `files/stats` không mang revision | So sánh rồi ghi: `if_revision` (`0` = không được tồn tại); một revision cũ trả 409 `file_changed` nêu tên revision ("it is at revision 2, not 1; nothing was written"); editor mở Reload hoặc Overwrite | Hai người cùng sửa một file không được ghi đè nhau trong im lặng (rule của W7a cho flow) |
| Quan sát: tải `bundle.zip` lên `?path=/bundle.zip` thì nó được giải nén, ở gốc namespace (path bị bỏ qua): `/inner/x.txt`, `/y.sql` | Lưu thành một file, `/bundle.zip`. Import là file và thư mục (`webkitdirectory`, mỗi file một lần tải lên) | PHÁN QUYẾT CỦA NGƯỜI DÙNG 2026-10-14: không giải nén zip |
| Quan sát: một file dưới `/_flows/` được import thành một flow (`viafile` được tạo trong namespace của file) | 400: "'/_flows' and everything under it is reserved for flow sources" | Rule của W9b; flow được sửa trong editor flow |
| Quan sát: `GET /namespaces/company.team/dependencies` không có cạnh `caller` → `child` (một `Subflow` đi vào namespace), trong khi đồ thị của `company.other` có; đồ thị của `company.other` không có cạnh cho trigger `Flow` đi vào `listener`, trong khi đồ thị của `company.team` có | Mọi cạnh vào hoặc ra khỏi namespace, trong cả hai đồ thị (`company.other.caller → company.team.child`, `subflow`; `company.team.child → company.other.listener`, `flow_trigger`) | Đồ thị của một namespace phải giống nhau khi nhìn từ đầu nào |
| Quan sát: một trigger `Flow` có `dependsOn` chỉ nêu `namespace: company.team` vẽ một cạnh từ mọi flow của `company.team` | Không có cạnh: chỉ một mục `dependsOn` nêu cả `namespace` lẫn `flowId` mới vẽ cạnh (§3.4) | Một cạnh nói "flow này khởi động flow kia"; một điều kiện cho cả namespace không phải vậy |
| Đọc: `GET /namespaces/{ns}/dependencies` trả về `flowTopologyService.namespaceGraph` (`NamespaceController.java`), một đồ thị giữ trong topology flow được lưu (`FlowTopologyRepository`) | Dựng từ draft trên mỗi request (§3.4) | egret không có chỉ mục dependency được lưu |
| Quan sát: `PUT` KV giá trị `1.2345678901234567890123e5` đọc lại `123456.78901234567` (một float Ion); `…d5` đọc lại đủ mọi chữ số. Đọc: trường NUMBER của UI là `type="number"` và gửi `String(value)` (`KVTable.vue`) | API giống hệt (ngang bằng W9c); UI gửi số mũ dưới dạng decimal Ion (`d`), nên `1.2345678901234567890123e5` đọc lại `123456.78901234567890123`, đo trong trình duyệt | Số của một key không bị form làm tròn |
| Đọc: UI gửi JSON dưới dạng văn bản đã gõ (`KVTable.vue`) | Như vậy: JSON được gửi như đã gõ, các số giữ nguyên như đã viết | Ngang bằng |
| Quan sát: `ttl: PT0S` được nhận (200) và key đọc lại 410 ngay lập tức | API giống hệt (200); form từ chối `PT0S` trước khi gửi request | Một key hết hạn ngay khi được ghi là một lỗi gõ |
| Đọc: hành động Create của danh sách namespace chỉ hiện ngoài OSS (`v-if="!isOSS && canCreate"`, `ui/src/override/components/namespaces/Namespaces.vue`) | Không: một namespace tồn tại khi có thứ gì đó nằm trong nó | Ngang bằng OSS |

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
- **Thư mục:** một namespace chỉ chứa một thư mục còn sống (không có file nào bên dưới) được liệt kê, `own: true` và `files: 0`. Thư mục rỗng không được đếm là file.

`GET /api/ws/{workspace}/workflows/namespaces/{ns}` trả về một `WorkflowNamespaceDetailDto` cho một namespace: cùng dòng đó cộng `can_manage`, cho biết caller có `workflow:manage` ở đó hay không (trang dựa vào nó để vô hiệu các thao tác ghi). Nó trả 404 khi không nguồn nào biết namespace và nó không phải cha của namespace nào. Overview dùng route này.

### 3.2 Ghi file có kiểm tra revision

`POST …/namespaces/{ns}/files` (W9b) có thêm tham số query tuỳ chọn `if_revision`.

- **Khi có nó:** lần ghi chỉ commit nếu revision hiện tại của path bằng giá trị đó. Nếu không, nó trả 409 với `code: "file_changed"`, như `draft_changed` của W7a. `if_revision=0` nghĩa là "path không được tồn tại".
- **Khi không có:** hành vi không đổi, nên các caller và task hiện có không bị ảnh hưởng.
- **Tính nguyên tử:** việc kiểm tra và việc ghi là một transaction (`commit_staged` trong `storage/nsfiles/store.rs`), transaction này lấy khoá theo path sẵn có của W9b (`xact_lock`) trước khi đọc revision hiện tại, nên không lần ghi nào commit được giữa lúc kiểm tra và lúc insert.
- **Path đã xoá:** revision hiện tại là revision còn sống mới nhất. Một path đã bị xoá mềm, như một path chưa từng tồn tại, ở revision 0: `if_revision=0` ghi lại nó, và revision mà một editor đã đọc trước lần xoá trả 409 `file_changed` ("was deleted").
- **Các 409 khác:** một 409 không có `code: "file_changed"` (ví dụ một thư mục ở path đó) là một lỗi thường, không phải dialog lưu bị cũ.
- **Zip:** ở đây một `.zip` là một file như mọi file khác (W9b lưu nó thành một file), nên `if_revision` áp dụng cho nó. Ý tưởng từ chối `if_revision` trên một `.zip` bị bỏ: tiền đề của nó, rằng server giải nén zip, là sai.

### 3.3 Không đổi

Mọi route file của W9b và route KV của W9c giữ nguyên quyền của chúng: đọc cần `workflow:read`, còn ghi, xoá và purge cần `workflow:manage`.

### 3.4 Dependencies

`GET /api/ws/{workspace}/workflows/namespaces/{ns}/dependencies` yêu cầu `workflow:read`.

- **Node:** các flow của namespace và của các namespace con (neo theo dấu chấm), cộng mọi flow ở nơi khác trong workspace có cạnh trực tiếp tới hoặc từ một trong số đó. Mỗi node là `{id, namespace, flow_id, disabled, in_namespace}`.
- **Cạnh:** `{source, target, kind}`, trong đó `kind` là:
  - `subflow`: một task `Subflow` ở bất kỳ đâu trong cây task của flow, kể cả `errors:`, có `namespace` và `flowId` là literal gọi tên một flow có thật;
  - `flow_trigger`: một trigger `Flow` có `dependsOn` chứa một mục nêu cả `namespace` lẫn `flowId` của flow nguồn. Nó được suy ra từ draft, không từ `workflow.flow_listeners`, vốn không có danh sách upstream. Một mục chỉ nêu namespace không vẽ cạnh (§2).
- **Đích động:** một `Subflow` có đích là biểu thức không vẽ cạnh, và node của nó mang `dynamic_subflows: n`.
- **Draft hỏng:** một draft không parse được vẫn là một node, gắn cờ `unparsable: true`, khi nó nằm trong namespace hoặc có cạnh tới nó; các cạnh đi vào nó được giữ và nó không có cạnh đi ra. `unparsable` ở cấp trên cùng đếm mọi draft hỏng trong workspace, vì một draft hỏng ngoài namespace có thể che một cạnh đi vào nó.
- **Nguồn sự thật:** dựng từ các draft đã lưu của flow, thứ mà mọi execution chạy (phán quyết W7a). Không có chỉ mục được lưu.
- **Chạy ở đâu:** các draft được parse ngoài các worker async (`spawn_blocking`).
- **Giới hạn:** giới hạn đếm mọi flow của workspace, không chỉ của namespace. Hơn 2.000 thì trả 422 với `code: "dependencies_too_large"` kèm một thông báo.

## 4. Frontend

Mọi thứ nằm trong khu vực Workflows tải lười của W7a. Thêm một mục nav Workflows "Namespaces".

### 4.1 Danh sách namespace

Trang là `/explore/:workspace/workflows/namespaces`.

- **Cây:** namespace được vẽ thành cây theo dấu chấm, kèm số flow, file và key của mỗi node. Node chỉ là cha thì bị làm mờ. Caption của bảng nói rõ số đếm là của riêng từng namespace, như Overview ("Counted in this namespace only; each child namespace counts its own.").
- **Bộ lọc:** bộ lọc prefix, giữ trong URL (FE-PAGE-2).
- **Trạng thái:** skeleton khi đang tải (FE-STATE-2) và một trạng thái rỗng.
- **Điều hướng:** chọn một node mở chi tiết của nó.

### 4.2 Chi tiết namespace

Trang là `/explore/:workspace/workflows/namespaces/:namespace/:tab`, với các tab dọc theo thứ tự của Kestra: Overview, Flows, Executions, Dependencies, Files, KV. Tab là một phần của URL, và tab không biết thì chuyển hướng về Overview. Breadcrumb hiện từng đoạn của namespace.

- **Overview:** số đếm, 10 execution gần nhất (danh sách của W7b với `namespace_prefix`), và các namespace con trực tiếp.
- **Flows** và **Executions:** danh sách flow của W7a và danh sách execution của W7b, với bộ lọc namespace cố định là namespace này và các con của nó. Cây của tab Flows gập/mở trong bộ nhớ, bắt đầu ở trạng thái mở: trạng thái thu gọn đã lưu của danh sách workspace không được đọc ở đó, nên một namespace tổ tiên bị thu gọn không thể che cả tab.
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
- Import file, hoặc một thư mục qua bộ chọn thư mục của trình duyệt (`webkitdirectory`): mỗi file một lần tải lên, mỗi file với path của nó bên dưới thư mục, mỗi file với `if_revision=0`, và các path đã tồn tại được liệt kê trong một câu hỏi Overwrite. **PHÁN QUYẾT CỦA NGƯỜI DÙNG 2026-10-14: không giải nén zip.** Một `.zip` được import thành một file, như W9b lưu nó. Export namespace thành zip.
- Đổi tên, và di chuyển bằng kéo thả. Mọi thao tác kéo đều có tương đương bàn phím ("Move to…").
- Xoá, kèm xác nhận nêu rõ bao nhiêu file sẽ mất.
- Tìm kiếm, qua route tìm kiếm của W9b.
- Xem các revision của một file và khôi phục một revision. Khôi phục ghi nội dung của revision đó thành một revision mới, kèm `if_revision`; một 409 hỏi có khôi phục đè lên revision mới hơn hay không.

- Import hơn 100 file thì hỏi trước. Trong lúc một lần import chạy, mọi thao tác ghi (file hay thư mục mới, import khác, đổi tên, di chuyển, xoá, kéo thả) bị tắt kèm "Wait for the import to finish."
- Phần trợ giúp bàn phím của cây chỉ nêu F2 và Delete khi được phép ghi.

**Mở một file**
- File văn bản mở trong Monaco, với ngôn ngữ lấy từ phần mở rộng, theo theme sáng hoặc tối của app.
- Ảnh hiện bản xem trước.
- File nhị phân khác, và mọi file lớn hơn 5 MB, chỉ cho tải xuống. Một revision lớn hơn 5 MB, xem từ Revisions, cũng chỉ cho tải xuống.

**Lưu**
- Một lần lưu gửi `if_revision`.
- Một 409 `file_changed` mở dialog lưu bị cũ của W7a: Reload hoặc Overwrite. Overwrite gửi lại không kèm `if_revision`.
- Ctrl/Cmd+S thuộc về editor đang có focus: mỗi editor file đăng ký một action Monaco của riêng nó (`addAction`, dispose cùng editor), nên phím này chỉ lưu file đó, không bao giờ lưu flow hay file khác.
- File chưa lưu hiện một chấm trên tab, và đóng nó thì hỏi trước.

**Trên trang namespace**
- Cây ở bên trái và các file đang mở là tab ở bên phải.
- Các tab đang mở được nhớ theo từng namespace trong `localStorage`.
- Tab Files vẫn được mount, ở trạng thái ẩn, kể từ lần đầu được hiện, nên ghé một tab khác của namespace không làm mất sửa đổi chưa lưu. Các tab dùng kích hoạt thủ công: phím mũi tên chuyển focus, Enter hoặc Space mới chọn.
- **Lệch rule (FE-PAGE-2):** file đang mở và chuỗi tìm kiếm không nằm trong URL ở đây. `NamespaceFiles` là một component dùng chung cho trang này và editor flow, nơi các file đang mở là tab của layout; chúng được giữ trong state của component (và các tab đang mở trong `localStorage`).

**Trong editor flow**
- Một tab Files được đăng ký qua `registerTab`, cùng cách W7c đã đăng ký Topology: một stub có thân tải lười. Nó hiện cây của namespace của flow.
- Mở một file thêm một tab `file:<path>` vào khung nhiều panel, trong panel đang chứa Code.
- Một tab file lưu bằng Ctrl/Cmd+S. "Save all files" lưu mọi tab file chưa lưu.
- Các tab file được khôi phục từ layout đã lưu.
- Nút Save của chính flow không lưu file, và lưu một file không lưu flow.
- Một lần lưu flow chuyển flow sang namespace khác trong khi một tab file có sửa đổi chưa lưu thì hỏi trước: **Save files first** (vào namespace cũ, rồi mới lưu flow), **Discard file changes**, hoặc **Cancel**. Các tab file của namespace cũ đóng lại khi flow chuyển đi.
- Khôi phục từ Revisions của cây Files thì một tab file đang mở không có sửa đổi chưa lưu đọc lại file; tab có sửa đổi thì giữ chúng, và lần lưu của nó gặp câu hỏi lưu bị cũ.
- Khi lần lưu của người khác chuyển flow sang namespace khác, các tab file đóng lại và sửa đổi chưa lưu của chúng mất, kèm một thông báo nói rõ điều đó.

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
- JSON được gửi như đã gõ, nên các số của nó giữ đủ mọi chữ số. Một NUMBER có số mũ được gửi dưới dạng decimal Ion (`6.02e23` thành `6.02d23`), nên mọi chữ số được giữ; một float Ion sẽ làm tròn nó.
- Kiểm tra TTL theo chính bộ parse duration của egret (chấp nhận phần thập phân). Form từ chối `PT0S`: key sẽ hết hạn ngay khi được ghi.
- Sửa một key giữ TTL còn lại của nó trừ khi expiration được đổi. Thời gian còn lại được đọc lúc bấm Save; nếu key đã hết hạn trong lúc drawer mở, form từ chối ("This key expired while you were editing; choose an expiration") và không gửi gì, thay vì một PUT không có `ttl`, thứ sẽ làm key thành vĩnh viễn.
- Một lần sửa gửi một DATETIME có văn bản không đổi đúng như server đã gửi, nên micro- hay nano-giây của giá trị đã lưu còn nguyên khi chỉ sửa description (văn bản `datetime-local` của form chỉ giữ tới mili-giây).
- Tạo một key đã tồn tại thì hỏi trước khi ghi đè.

**Key kế thừa**
- Một drawer "Inherited KV" liệt kê các key kế thừa từ namespace cha, qua route kế thừa của W9c. Cha gần nhất thắng.
- **Lệch rule (FE-DATA-2):** một key tên `inheritance` không đọc được ở path mà fetcher sinh ra dựng, vì `…/kv/inheritance` là route liệt kê kế thừa. Riêng key đó được đọc qua cùng mutator (`customInstance`) tại URL sinh ra với chữ cuối được percent-encode (`…/kv/inheritanc%65`), thứ server decode lại thành key (`kvApi.ts`).

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

**Playwright** (`frontend/tests/e2e/workflows-namespaces.spec.ts`), trên một stack riêng, không bao giờ trên stack dev dùng chung:
1. Tạo một flow trong `company.team`, một flow trong `company.other` gọi nó qua một `Subflow`, và một key trong `company`; danh sách namespace hiện chúng dưới dạng cây.
2. Trên trang namespace: tạo file với `if_revision=0`; sửa một file, sang tab KV rồi quay lại (văn bản và chấm chưa lưu vẫn còn), lưu bằng Ctrl/Cmd+S (một POST kèm `if_revision`); đổi tên; di chuyển bằng kéo thả thật; import một thư mục qua `webkitdirectory`; export zip. Cây và editor được đo ở chế độ sáng và tối, nền editor được khẳng định là tối ở chế độ tối.
3. KV: tạo một key JSON và một key DATE, đọc lại một NUMBER có số mũ đủ mọi chữ số, và thấy các key kế thừa từ `company`.
4. Trong editor flow: Code cạnh HAI tab file, đặt cạnh nhau; Ctrl/Cmd+S trong mỗi file chỉ lưu file đó, và trong Code chỉ lưu flow. Ở chế độ tối hai editor file và Code có nền tối, và vẫn tối sau khi một revision mở, ở nền tối, trong drawer Revisions.
5. Một phiên thứ hai lưu một file: lần lưu của phiên này nhận 409 `file_changed`, dialog lưu bị cũ mở ra, và Overwrite gửi lại không kèm `if_revision`.
6. Một flow được lưu sang namespace khác trong khi một file có sửa đổi chưa lưu thì hỏi trước; "Save files first" ghi vào namespace cũ.
7. Dependencies vẽ cạnh `subflow`.

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

Kịch bản ở §6 đã chạy trên image Kestra cục bộ ngày 2026-10-14 (Task 9). Mọi khác biệt tìm thấy được ghi ở §2, mỗi dòng ghi rõ là quan sát hay đọc trong mã nguồn.

## 10. Giới hạn đã biết

- Dependencies đọc draft của mọi flow trong mỗi request, chặn ở 2.000 flow. Workspace có nhiều flow hơn sẽ nhận 422.
- Một `Subflow` có đích là biểu thức không có cạnh.
- Mọi draft của workspace được nạp cho mỗi request, và kích thước của chúng không bị giới hạn, chỉ số lượng.
