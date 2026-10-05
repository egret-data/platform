# W9b — Namespace files: `nsfile://`, `fileURI`, `namespaceFiles`, các task `core.namespace` và API namespace file

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-05-workflow-w9b-namespace-files-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-05; chưa cài
**Phụ thuộc:** W9a (internal storage: streaming của `egret-storage`, `FileStore` toàn process, `storage::uri`, file scope trước render, chuyển file streamed, cầu storage cho plugin, parser multipart có giới hạn, header download an toàn của API file, pruner) — đã merge. W5a (script task, `inputFiles`), W5b (plugin host, `task.json`) — đã merge. Base `e539d8b3`.
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 phát hiện 4 (`StorageInterface`), §9 (dòng W9); spec W9a `docs/{en,vi}/specs/2026-10-01-workflow-w9a-internal-storage-design.md` §14 (namespace files dời sang W9b)
**Đã quyết với người dùng, 2026-10-05:**
- **Namespace file giữ revision, như Kestra.** Revision 1 nằm ở `path`, revision N > 1 ở `path.vN`; xoá là xoá mềm; `PurgeFiles` xoá cứng (§3, §4, §8.4).
- **Một flow đang chạy đọc namespace file của bất kỳ namespace nào trong workspace của nó, và chỉ ghi vào namespace của chính nó.** Ghi là `UploadFiles`, `DeleteFiles`, `PurgeFiles` (namespace của chính nó và các namespace con) và `putFile` của plugin. Workspace là ranh giới cứng, lấy từ execution đang chạy, không bao giờ từ một URI (§5).
- **Quyền API:** đọc (download, list, search, stats, revisions, export) cần `workflow:read`; ghi (upload, tạo thư mục, move, delete, purge) cần `workflow:manage`. Không có quyền mới (§9).
- **Trong phạm vi:** store và metadata của nó (`workflow.namespace_files`, object ở `{prefix}{ws}/{ns-as-path}/_files/{path}[.vN]`, URI `kestra:///{ns-as-path}/_files/…` qua một `Tail::NamespaceFile` mới, parse `nsfile://`) (§3, §4), các hàm biểu thức (§6), `namespaceFiles` trên script task, chỉ parse và kiểm tra trên `WorkingDirectory` (§7), bốn task `io.kestra.plugin.core.namespace` dưới dạng handler native (§8), REST API (§9) và cầu plugin (§10).
- **(Thêm ngày 2026-10-05, phán quyết của người dùng về phát hiện `WorkingDirectory` của spec.) `WorkingDirectory` nằm ngoài W9b.** W9b chỉ chạy `namespaceFiles` trên script task. Trên `WorkingDirectory`, khối này được parse và kiểm tra lúc publish: khối sai dạng bị từ chối, nêu tên thuộc tính; khối hợp lệ được nhận và giữ trong model cho gói sau sẽ chạy `WorkingDirectory`. Lúc chạy không gì thay đổi, vì egret vẫn không thực thi `WorkingDirectory` (§1, §7). Hai flow corpus vẫn nằm trong baseline, và không dự kiến ratchet thay đổi (§13).
- **(Thêm ngày 2026-10-05, phán quyết của người dùng về `inputFiles`.) Như Kestra: namespace file được nạp trước và `inputFiles` luôn ghi đè namespace file cùng tên, bất kể `ifExists`; `ifExists` chỉ quyết định khi các namespace đụng nhau** (§7). Task 1 vẫn đo điều này.
- **(Thêm ngày 2026-10-05, phán quyết của người dùng về namespace `_files`.) Namespace có segment `_files` bị từ chối** lúc publish và ở mọi thao tác namespace file (API, task, hàm), với lỗi nêu tên nó. Task 1 đếm các flow hiện có và các entry corpus có namespace như vậy, và báo cáo của nó liệt kê từng cái (§3).
- **(Thêm ngày 2026-10-05, phán quyết của người dùng về mục mở 9 của plan.) Giá trị `inputFiles` của script task là URI namespace file (`nsfile:///…`, `nsfile://other.ns/…`, hoặc `kestra:///…/_files/…`, ví dụ do `fileURI` tạo) được chấp nhận, như Kestra:** nó được stage vào thư mục làm việc của task qua cùng một hàm kiểm quyền đọc namespace file (mọi namespace của workspace), bên cạnh URI execution file của W9a (§7).
- **(Thêm ngày 2026-10-05, phán quyết của controller về mục mở 10 của plan.) `task.json` mang namespace file trong một mảng `namespaceFiles` riêng** (URI, namespace, path, revision, size, path đã stage); map `storage` của W9a (URI → path đã stage) không đổi (§10).
- **(Thêm ngày 2026-10-05, phán quyết của người dùng về các lựa chọn chi tiết.) Được duyệt làm lựa chọn của egret**, mỗi cái là một dòng ở §2: `PurgeFiles` mặc định là namespace của chính flow; namespace con khớp theo ranh giới dấu chấm; `DELETE` của API không xoá cha bị rỗng; export là zip streamed chỉ gồm file; `fileURI` không có `revision` là lỗi với path thiếu; đọc `kestra:///other/ns/_files/…` được phép trong workspace.

**Bàn giao:** một PR (tài liệu + mã), mười task ở §12, thực hiện theo một plan viết sau spec này.

## 1. Cái gì đã có, cái gì chưa

| phần | trạng thái tại `e539d8b3` |
|---|---|
| namespace | chỉ là một chuỗi trên flow, `Flow::namespace` (`crates/egret-workflow/src/models/flow.rs:41`); flow là duy nhất theo `(workspace_id, namespace, flow_id)` (`migrations/0132_workflow_flows.sql:30`). Không có bảng namespace và không có quyền theo namespace. Publish kiểm namespace theo `^[a-z0-9][a-z0-9._-]*`, 1-150 ký tự (`spec/validate.rs:625-636`, `is_valid_namespace` `:662`), nên một segment như `_files` trong `a._files` hiện được chấp nhận |
| flow id | `^[a-zA-Z0-9][a-zA-Z0-9._-]*`, 1-100 ký tự (`spec/validate.rs:651-660`, `is_valid_flow_id` `:692`): không flow nào tên `_files` được, nên trong một namespace `{ns}/_files/…` không đụng `{ns}/{flow}/executions/…` (nhưng xem §3 về namespace có segment `_files`) |
| URI internal storage | `storage::uri` parse `kestra:///{ns}/{flow}/executions/{exec}/{tail}` với `Tail::{Input, Task, Trigger, Execution}` (`storage/uri.rs:22-38`); URI không có `executions/{uuid}` bị từ chối là "not an execution file" (`:112-117`). Chưa có tail `_files`. `namespace_as_path` ở `uri.rs:186` |
| hàm file | `read`, `fileSize`, `fileExists`, `isFileEmpty` trả lời từ file scope trước render (W9a §4.4). `fileURI` bị từ chối nêu W9b (`runners/functions.rs:269`); path trần hay URI `nsfile://` bị từ chối nêu W9b (`functions.rs:356-361`) |
| đọc có giới hạn trên thread render | `off_the_runtime` (`runners/env.rs:2094-2100`, `block_in_place` trên runtime multi-thread, W9a Task 4 fix round 1 C1); cờ mất kết nối của file scope `infrastructure_failure` (`runners/files.rs:59`, `storage/scope.rs:250`, đọc bởi `worker/claim.rs:1258`) |
| `namespaceFiles` của script | bị từ chối lúc publish cho cả bốn kiểu script (`scripts/mod.rs:195-207`, `COMMON_REFUSED`); ghim bởi `tests/scripts_gate.rs:940-945`; doc module của worker nói nó cần W9b (`worker/commands.rs:105-107`) |
| `WorkingDirectory` | model chỉ giữ `tasks` và lặng lẽ bỏ `namespaceFiles`, `inputFiles`, `outputFiles`, `cache` (`models/task.rs:555-565`). **Executor không chạy nó**: `build_plan` lập kế hoạch nó thành một `Runnable` mờ (`executor/plan.rs:1230`, `executor/mod.rs:3319-3321`), và claim trả `None` cho nó (`worker/claim.rs:258`) |
| plugin host | `ShimRunContext.storage()` dựng `InternalStorage` với `NO_NAMESPACE_FILES`, mà `of(…)` ném `EgretUnsupported` nêu W9b (`plugin-host/…/ShimRunContext.java:274-280`); `EgretUnsupported` khiến host thoát mã 4 (`EgretUnsupported.java:7`) |
| pruner | pruner của W9a chỉ xoá theo execution dưới `FileStore::execution_prefix` (`storage/prune.rs:553`) |
| migration | cuối cùng là `0168_workflow_execution_files_pruned.sql`; W9b lấy **0169** |
| corpus | hai flow dùng `namespaceFiles`, cả hai trên `WorkingDirectory` (`oracle-exec/corpus/valids/working-directory-namespace-files.yaml`, `…-with-namespaces.yaml`, cái thứ hai với ba namespace, `folderPerNamespace: true` và một `exclude`); không flow nào dùng `nsfile://`, `fileURI` hay `core.namespace.*` |

## 2. Kestra làm gì, và egret khác ở đâu

Kestra (checkout `96893b589`, artifact `2.0.0-rc13`):
- **Bố cục.** Namespace file nằm dưới `/{ns-as-path}/_files` (`StorageContext.java:41`, `namespaceFilePrefix` `:324-326`). Revision 1 lưu ở `path`, revision N > 1 ở `path.vN`; thư mục luôn là revision 1 (`NamespaceFile.java:138-148`). Repository metadata cho một lần ghi `previous + 1` (thư mục giữ 1) (`AbstractJdbcNamespaceFileMetadataRepository.java:191-199`), đọc `previous` qua `findByPath`, vốn chạy transaction riêng (`:69-80`). Xoá mềm đánh dấu dòng mới nhất là deleted mà không tạo revision mới (`:203-205`).
- **`Namespace`** (`core/…/storages/Namespace.java`): `all`, `children`, `move`, `get`, `findAllFilesMatching(includes, excludes)`, `getFileContent(path, revision?)`, `getFileMetadata`, `exists`, `putFile(…, Conflicts)` với `Conflicts { OVERWRITE, ERROR, SKIP }` (`:186-189`), `createDirectory`, `delete`, `isDirectoryEmpty`.
- **`InternalNamespace`**: ghi object, rồi tạo các thư mục cha còn thiếu, rồi lưu metadata (`InternalNamespace.java:338-404`); `ERROR` ném lỗi, `SKIP` ghi log (`:391-399`); một path đã xoá mềm được ghi lại thành revision mới (`:372`); một entry còn sống khác loại (file vs thư mục) là `ConflictException`, một entry đã xoá thì bị purge trước (`:421-450`); cha được tự tạo (`mkDirs`, `:455-466`); `delete` đánh dấu path và mọi con là deleted (`:495-512`); `move` từ chối đích đã tồn tại, chép mọi revision, rồi purge nguồn, rollback các bản chép khi lỗi (`:130-215`) — *(sửa ngày 2026-10-05, Task 1 đo trên artifact: ba revision của một file chuyển thành 1, 2, 3, nhưng move một **thư mục** chỉ mang theo revision mới nhất của mỗi con, đánh số lại thành 1; move lên đích đã tồn tại trả 500)*; `purge` đánh dấu metadata deleted trước khi xoá object (`:218-224`); đọc đi qua metadata và **lùi về revision cũ hơn** khi object được index bị thiếu (`:257-289`).
- **Path.** `NamespaceFile.normalize` từ chối `..` (`NamespaceFile.java:167-178`). Controller từ chối `/_flows` (`NamespaceFileController.java:68-70`, `:379-386`) và mọi thành phần path dài quá 255 ký tự (`:52-53`, `:254-258`).
- **Hàm.** `nsfile:///path` là namespace của chính flow; `nsfile://other.ns/path` nêu namespace ở authority; tham số `namespace` cùng với authority là lỗi (`AbstractFileFunction.java:181-191`); path trần thành `nsfile:///path` trong namespace của tham số `namespace` hoặc của flow (`:92-95`). `fileURI(path, revision?)` trả một URI `kestra://`; có `revision` thì nó kiểm revision đó tồn tại và từ chối số không nguyên (`FileURIFunction.java:54-79`); không có thì nó không kiểm file tồn tại (`:76-78`, theo mã). `read` trả `String`, hoặc `byte[]` cho Ion nhị phân (`ReadFileFunction.java:69-76`), và nhận `revision` (`:78-89`). `isAllowedNamespace` của OSS luôn true (`NamespaceService.java:43-45`, gọi từ `DefaultNamespaceService.java:39`); không có kế thừa từ namespace cha.
- **`namespaceFiles` của script** (`core/…/models/tasks/NamespaceFiles.java`): `enabled` mặc định true khi khối có mặt (`:24-25`), `include`, `exclude`, `namespaces` mặc định `["{{flow.namespace}}"]` (`:42-44`), `ifExists` mặc định `OVERWRITE` (`:49-50`; `FileExistComportment { OVERWRITE, FAIL, WARN, IGNORE }`), `folderPerNamespace` mặc định false (`:55-56`). Bộ nạp (`NamespaceFilesUtils.java:44-98`) liệt kê file khớp theo từng namespace, rồi ghi chúng **song song** (`:69-85`), nên giữa hai namespace có cùng path thì thứ tự không được bảo đảm; `WARN` ghi log và giữ file đã có (`LocalWorkingDir.java:202-212`) — *(sửa ngày 2026-10-05, Task 1 đo: qua năm lần chạy mỗi chế độ, `OVERWRITE` giữ file của namespace sau, `WARN` và `IGNORE` giữ file đầu, `FAIL` làm task thất bại; dòng log của `WARN` không xuất hiện trong log của task)*. Script task và `WorkingDirectory` nạp namespace file **trước** `inputFiles` (`CommandsWrapper.java:155-178`, `WorkingDirectory.java:292-298`), rồi `inputFiles` được ghi bằng một `FileOutputStream` thường, thay namespace file bất kể `ifExists` (`FilesService.java:38-58`).
- **Glob** (`PathMatcherPredicate.java`): cú pháp mặc định là `glob:`; pattern bắt đầu bằng ký tự chữ được thêm `**/` phía trước (`:112-114`); hỗ trợ `regex:` (`:22-23`, `:132-134`); vị từ là includes AND NOT excludes (`:67-78`).
- **Task** (`core/…/plugin/core/namespace/`): `UploadFiles` — `namespace` (`@NotNull`, dù `run` lùi về namespace của flow, `:118-122`, `:162`; *sửa ngày 2026-10-05, Task 1 đo: publish từ chối khi thiếu `namespace` với 422 `tasks[up].namespace: must not be null`, nên không bao giờ tới được nhánh lùi*), `files` glob trên **working directory của chính nó** (`:192-201`), `filesMap` (tên → URI, chỉ đọc nếu tồn tại, `:204-222`), `destination` mặc định `/` (`:147-148`), `conflict` mặc định `OVERWRITE` (`:150-156`), trả về `Output` rỗng dù `Output.files` được khai báo (`:187`, `:224-228`). `DownloadFiles` — `namespace` bắt buộc, `files`, `destination` mặc định `""`; output path → URI `kestra://` (`DownloadFiles.java:81-145`). `DeleteFiles` — `namespace`, `files`, `deleteParentFolder` mặc định false; xoá mềm (`DeleteFiles.java:80-176`). `PurgeFiles` — `filePattern` (wildcard Commons-IO), `namespaces`, `namespacePattern`, `behavior` mặc định `Version { keepAmount: 1 }`, `includeChildNamespaces` mặc định true; xoá cứng; output `size` (`PurgeFiles.java:59-130`, `Version.java`, `PurgeTask.java`, `NamespaceFileService.java:95-115`).
- **API** (`NamespaceFileController.java`, `/api/v1/{tenant}/namespaces`): `GET {ns}/files/search?q` (`:73`), `GET {ns}/files?path&revision` (`:82`), `GET {ns}/files/stats` (`:101`), `GET {ns}/files/revisions` (`:126`), `GET`/`POST {ns}/files/directory` (`:154`, `:180`), `POST {ns}/files` multipart, `.zip` được bung (`:196-235`), `_flows/x` import một flow (`:236-246`), `GET {ns}/files/export` (zip trong bộ nhớ, `:269-308`; code của checkout thêm flow của namespace dưới dạng `_flows/*.yml`, nhưng *(sửa ngày 2026-10-05)* artifact mà Task 1 đo chỉ export file, không có entry `_flows`, từ một namespace đang chứa một flow), `PUT {ns}/files?from&to` (`:312`), `DELETE {ns}/files` (`:332`). OSS không có `@Secured`.
- **Vòng đời.** Entry xoá mềm ở lại tới khi `PurgeFiles`; `PurgeStorage` chỉ lo storage của execution; không tìm thấy việc dọn khi xoá namespace.

| Kestra | egret W9b | vì sao |
|---|---|---|
| một flow đọc file của mọi namespace trong OSS (`isAllowedNamespace` true) | mọi namespace **của workspace của nó** (§5) | workspace là tenant của egret; nó đến từ execution đang chạy, không bao giờ từ URI |
| một flow ghi vào mọi namespace (`UploadFiles`/`DeleteFiles`/`PurgeFiles` nhận bất kỳ `namespace`) | chỉ namespace của chính nó; `PurgeFiles` của chính nó và các namespace con (§5, §8) | quyết định của người dùng: tác giả có `workflow:manage` trên một flow không được viết lại file dùng chung của nhóm khác từ một task |
| `read` một URI `kestra:///other/ns/_files/…` bị `extractNamespace` từ chối (`AbstractFileFunction.java:195-201`, theo mã; Task 1 đo) | được nhận với mọi namespace của workspace (§6) | cùng quy tắc đọc như `nsfile://other.ns/…`; một hàm quyết định quyền đọc (§5) |
| revision = `previous + 1`, `previous` đọc trong transaction riêng không khoá (theo mã) | revision kế tiếp được lấy dưới khoá theo (workspace, namespace, path), trong transaction chèn nó (§4.2) | hai writer đồng thời không bao giờ dùng chung một revision |
| object bị thiếu cho một revision đã index thì lùi về revision cũ hơn (`InternalNamespace.java:267-289`) | lỗi của tác giả nêu path và revision; không lùi (§4.5) | lùi lặng lẽ phục vụ nội dung người gọi không hỏi; lệch là lỗi cần thấy, không phải để che |
| `fileURI(path)` không có `revision` không kiểm file tồn tại (theo mã; Task 1 đo) | path thiếu hoặc đã xoá mềm là lỗi nêu nó (§6) | một URI không chỉ tới gì sẽ hỏng về sau, xa nguyên nhân |
| `read` trả `byte[]` cho Ion nhị phân | không phải UTF-8 là lỗi nêu path, như `read` của W9a (§6) | kết quả được ghép vào một chuỗi đã render |
| `namespaceFiles` qua nhiều namespace ghi song song, thứ tự không bảo đảm | tất định: namespace đứng sau trong `namespaces` thắng (dưới `OVERWRITE`) (§7) | run tái lập được |
| `UploadFiles.files` glob trên working directory của chính task | glob trên **các file đọc được của execution** (handler native không có working directory); `files` cũng có thể là nguyên URI `kestra://` (§8.1) | egret chạy `UploadFiles` trong worker, không trong pod |
| `PurgeFiles` không có `namespaces` purge mọi namespace có flow; namespace con khớp theo tiền tố chuỗi (`ns.startsWith(…)`, `PurgeTask.java:56`; không có `namespaces` thì lấy mọi `findDistinctNamespace`, `:34`, `:82`), nên `a.b` khớp `a.bc` | mặc định là namespace của chính flow; con khớp theo ranh giới dấu chấm (§8.4) | quy tắc ghi; `a.bc` không phải con của `a.b` |
| `PurgeFiles` `keepAmount: 1` giữ revision mới nhất kể cả khi nó đã xoá mềm (theo mã; Task 1 đo) | path đã xoá mềm bị purge toàn bộ (§8.4) | quyết định của người dùng: file đã xoá không nên giữ storage mãi |
| `POST files` bung `.zip` và import flow từ `_flows/x` | không làm cả hai: `.zip` được lưu như một file, `/_flows` bị từ chối (§9) | flow được publish qua API flow của egret, với kiểm tra và quyền của nó |
| `GET files/export` dựng zip trong bộ nhớ (code của checkout thêm flow của namespace; artifact mà Task 1 đo không thêm flow nào, sửa ngày 2026-10-05) | zip streamed chỉ gồm file (§9) | bộ nhớ có giới hạn; flow được export qua API flow |
| `DELETE files` còn xoá các thư mục cha bị rỗng, với phép kiểm cha không giới hạn theo namespace (`NamespaceFileController.java:352-363`, theo mã) | chỉ xoá path được nêu và con của nó (§9) | việc leo lên là tác dụng phụ người gọi không nêu |
| namespace có segment `_files` (`a._files`) được nhận, nên key của flow của nó chung tiền tố với namespace file của `a` | bị từ chối lúc publish và bởi mọi thao tác namespace file (§3) | hai không gian key không được chồng nhau |

## 3. Bố cục storage và URI

- **Object key:** `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/_files/{path}` cho revision 1, `…/{path}.v{N}` cho N > 1; thư mục không có object (dòng của nó là thư mục), khác `createDirectory` trên store của Kestra. Cùng bucket và prefix với W9a.
- **URI flow thấy:** `kestra:///{ns-as-path}/_files/{path}[.vN]`, dạng của Kestra. Như của W9a nó không mang workspace: egret ánh xạ nó sang key bằng cách thêm workspace của execution đang chạy.
- **`Tail::NamespaceFile { path, revision }`** gia nhập `Tail` của W9a trong `storage/uri.rs`. `parse` nhận segment **đầu tiên** bằng `_files` ở chỉ số ≥ 1 làm dấu namespace file: các segment trước là namespace, sau là path; một đuôi `.v{N}` (N ≥ 2, chỉ chữ số) là revision. Phép kiểm này chạy trước khi tìm `executions/{uuid}`, nên `kestra:///a/_files/x/executions/<uuid>/f` là file `/x/executions/<uuid>/f` của namespace `a`, không bao giờ là URI execution của một flow `_files` (vốn không thể tồn tại, §1).
- **Namespace có segment `_files`** (`a._files`, hợp lệ theo `is_valid_namespace`) sẽ đặt key execution của flow của nó dưới file của namespace `a`. Namespace như vậy bị từ chối (phán quyết của người dùng, header): từ W9b publish từ chối nó, và mọi thao tác namespace file (API, task, hàm) từ chối nó làm đích hay authority, mỗi lần với lỗi nêu namespace và segment. Task 1 đếm các flow đã publish trong một database dev và các entry corpus có namespace chứa segment đó, và báo cáo của nó liệt kê từng cái (dự kiến không có).
- **`nsfile://`** được parse: không authority (`nsfile:///path`) → namespace của flow đang chạy; có authority (`nsfile://other.ns/path`) → namespace đó, kiểm bằng `is_valid_namespace`. Path trần (không có `://`) là `nsfile:///path`.
- **Quy tắc path**, một hàm cho mọi lối vào (hàm, task, API, cầu plugin): có `/` đầu (thêm nếu thiếu); không segment rỗng, `.` hay `..`; mỗi segment ≤ 255 ký tự; không phải `/_flows` hay nằm dưới nó; các ký tự W9a từ chối (`\`, `?`, `#`, ký tự điều khiển); segment cuối của một file không được kết thúc bằng `.v{chữ số}` (nó sẽ bị đọc như revision của path khác). Path vi phạm là lỗi của tác giả nêu path và quy tắc.

## 4. Lõi

### 4.1 Bảng metadata

`workflow.namespace_files` (migration 0169): `workspace_id` (FK tới workspace), `namespace` (text), `path` (text, đã chuẩn hoá, thư mục kết thúc bằng `/`), `revision` (int ≥ 1; luôn 1 với thư mục), `is_directory`, `size`, `deleted`, `created_at`, `created_by` (user hay execution đã ghi). Duy nhất `(workspace_id, namespace, path, revision)`; một index trên `(workspace_id, namespace, path)` cho dòng mới nhất. Một dòng cho mỗi revision; dòng mới nhất của một path là revision cao nhất. Không có FK tới namespace (không có bảng namespace): một namespace không có flow vẫn chứa được file.

**Một namespace file chỉ tồn tại khi dòng của nó tồn tại**, như index của W9a (W9a §4.2). Đọc, `exists`, stats và liệt kê đều đi qua bảng.

### 4.2 Ghi

1. Kiểm path (§3) và quy tắc ghi (§5).
2. Trong một transaction, lấy khoá theo `(workspace, namespace, path)` (`pg_advisory_xact_lock` trên hash của ba giá trị, hoặc `SELECT … FOR UPDATE` trên dòng mới nhất; plan chọn một và một test chạy hai writer đồng thời), đọc dòng mới nhất, áp quy tắc xung đột (`OVERWRITE` → revision kế; `ERROR` → lỗi của tác giả nêu path; `SKIP` → không ghi, báo là bỏ qua), và tính `revision = max + 1` (1 cho path mới).
3. Stream nội dung tới key của revision (`put_stream`), `head` lấy kích thước, chèn dòng, tạo dòng thư mục cha còn thiếu, commit. Khoá được giữ suốt lần upload để không writer thứ hai nào tính ra cùng revision; upload lỗi thì rollback transaction và chỉ để lại tối đa một object chưa index, mà lần ghi lại revision đó sẽ ghi đè.
4. **Đụng file/thư mục:** một dòng còn sống khác loại ở path là lỗi xung đột nêu nó; một dòng khác loại đã xoá mềm thì bị purge trước (quy tắc của Kestra).
5. **Ghi lại một path đã xoá mềm** tạo revision mới (`max + 1`, còn sống); các revision trước được giữ.

Trần: `workflow.storage.max_file_bytes` cho mỗi file, như W9a.

### 4.3 Xoá mềm

`delete(path)` đánh dấu deleted dòng mới nhất của path và của mọi con, trong một câu lệnh; object được giữ. Xoá thư mục là xoá các con của nó. Path đã xoá mềm vô hình với đọc, liệt kê, `exists` và glob, và thấy được với `revisions` và `PurgeFiles`.

### 4.4 Move

`move(from, to)` từ chối đích còn sống đã tồn tại; chép mọi revision của `from` (và của mọi con) sang `to` giữ nguyên số revision, chèn dòng dần; rồi purge nguồn (dòng trước, object sau). Lỗi trong lúc chép thì purge mọi bản chép đã làm, theo thứ tự ngược, và báo lỗi nêu cả hai path; không gì của nguồn bị đụng tới trước khi mọi bản chép thành công.

### 4.5 Đọc

Một lần đọc phân giải `(namespace, path, revision?)` thành một dòng (dòng còn sống mới nhất, hoặc revision được chỉ định, phải tồn tại; với đọc bản mới nhất thì path không được đang xoá mềm ở revision mới nhất), rồi stream key của revision đó. **Object bị thiếu sau một dòng là lỗi của tác giả nêu path và revision**; egret không lùi về revision cũ hơn (§2). Store không với tới được là lỗi hạ tầng (§11).

### 4.6 Một hàm quyết định đọc và ghi

`storage::nsfiles::authorize(workspace, running_flow_namespace, target_namespace, Access::{Read, Write, Purge})` quyết định mọi truy cập, cho hàm, staging, bốn task, cầu plugin và (với quyền của user thay cho flow) API. Không nơi gọi nào tự cài lại quy tắc. Mỗi lần từ chối có một test hỏng khi lần từ chối đó bị gỡ.

## 5. Ai được đọc và ghi

- **Workspace:** luôn là của execution đang chạy (hoặc, với API, `{ws}` của route sau khi kiểm quyền user). Không gì trong URI hay thuộc tính nêu một workspace.
- **Đọc:** mọi namespace của workspace đó (`nsfile://other.ns/…`, `kestra:///other/ns/_files/…`, `namespaceFiles.namespaces`, `DownloadFiles.namespace`).
- **Ghi** (`UploadFiles`, `DeleteFiles`, `putFile`/`delete`/`move` của plugin — dù sao cũng bị từ chối, §10): chỉ namespace của chính flow đang chạy, so khớp chính xác.
- **Purge** (`PurgeFiles`): namespace của chính flow và các namespace con (`ns == own` hoặc `ns` bắt đầu bằng `own + "."`).
- Lần từ chối là lỗi của tác giả nêu namespace và quy tắc; nó không bao giờ cho biết file có tồn tại ở đó hay không.

## 6. Các hàm biểu thức

| hàm | hành vi |
|---|---|
| `fileURI(path, revision?)` | `kestra:///{ns}/_files/{path}` cho revision còn sống mới nhất (`…{path}.vN` khi N > 1), hoặc cho revision được chỉ định, phải tồn tại. Tham số `kestra://` được trả nguyên sau khi parse được. `../`, `revision` không nguyên, path thiếu hay đã xoá mềm là lỗi nêu path |
| `read(path, revision?)` | nội dung UTF-8; không phải UTF-8 là lỗi nêu path (§2); bị chặn bởi `max_read_bytes` |
| `fileSize`, `fileExists`, `isFileEmpty` | từ dòng metadata; `fileExists` của một path đúng dạng không có dòng còn sống là `false`, như *missing* của W9a |

- Cả năm nhận path trần (→ `nsfile:///path`), `nsfile:///path`, `nsfile://other.ns/path`, và URI `kestra:///…/_files/…`; cộng tham số `namespace`, mà đi cùng authority là lỗi (`AbstractFileFunction.java:181-191`). URI `kestra://` của một execution giữ quy tắc snapshot của W9a.
- **Không snapshot cho namespace file.** Namespace file commit ngay, ngoài transaction của `settle`, nên pool thấy mọi dòng đã commit: render tra chúng thẳng qua pool, và một path ghép lúc render vẫn chạy. Lần tra và lần đọc chạy qua đường đọc có giới hạn không làm kẹt runtime của W9a Task 4 (`off_the_runtime`, `runners/env.rs:2094-2100`); store hay database mất kết nối thì bật cờ `infrastructure_failure` của file scope (§11), nên run được re-claim mà không tốn attempt.
- Chúng có trong mọi lần render có execution; lần render không có (điều kiện của trigger, publish) từ chối chúng theo tên, như của W9a.
- `fileURI` rời `REFUSED` (`functions.rs:269`), và lần từ chối path trần và `nsfile://` (`functions.rs:356-361`) được thay; các test ghim "nêu W9b" (`functions.rs:3993-4025`) lật lại, mỗi cái có một test thay thế chứng minh việc đọc.

## 7. `namespaceFiles` trên script task và `WorkingDirectory`

- **Thuộc tính như của Kestra:** `enabled` (mặc định true khi khối có mặt), `include`, `exclude`, `namespaces` (mặc định namespace của flow), `ifExists` (`OVERWRITE`/`FAIL`/`WARN`/`IGNORE`, mặc định `OVERWRITE`), `folderPerNamespace` (mặc định false).
- **Glob** theo `PathMatcherPredicate`: mặc định `glob:`, thêm `**/` trước pattern bắt đầu bằng ký tự chữ, hỗ trợ `regex:`, includes AND NOT excludes. Chuỗi path chính xác mà glob thấy (có `/` đầu hay không) là **phép đo của Task 1**, ghim bằng fixture; egret cài theo fixture, không theo cách đọc mã Java.
- **Lúc claim** egret liệt kê các file còn sống mới nhất khớp của mỗi namespace trong `namespaces` (mỗi cái được kiểm quyền đọc, §5), và stage chúng qua cơ chế chuyển streamed của W9a tại `path` (hoặc `{namespace}/{path}` với `folderPerNamespace`).
- **Thứ tự tất định:** các namespace được stage theo thứ tự danh sách, nên dưới `OVERWRITE` namespace sau thắng; `FAIL` làm task fail nêu cả hai nguồn; `WARN` giữ cái đầu và ghi một dòng log nêu path; `IGNORE` giữ cái đầu. **`inputFiles` được stage sau namespace file và luôn ghi đè cái cùng tên, bất kể `ifExists`**, như Kestra (phán quyết của người dùng, header); `ifExists` chỉ quyết định khi các namespace đụng nhau.
- **Giá trị `inputFiles` là URI namespace file** (`nsfile:///…`, `nsfile://other.ns/…`, `kestra:///…/_files/…`; phán quyết của người dùng, header) được stage như entry của `namespaceFiles`: kiểm quyền đọc bằng cùng một hàm (§4.6, §5), chuyển bằng transfer của W9a dưới tên của `inputFiles`. Namespace bị từ chối hoặc path thiếu làm task fail trước khi chạy, nêu tên path.
- **Giới hạn có tính cấu trúc:** việc liệt kê bị chặn theo số lượng (`workflow.storage.max_namespace_files`, mặc định 10 000), đếm trên các dòng egret thực sự liệt kê, với lần từ chối nêu trần; byte đã stage tính vào `max_task_bytes` khi chuyển streamed. Không giới hạn nào mô phỏng ngữ pháp glob.
- **Script task:** `namespaceFiles` rời `COMMON_REFUSED` (`scripts/mod.rs:202`); `tests/scripts_gate.rs:940-945` lật thành test cho thấy thuộc tính publish được, và vẫn còn một test từ chối cho khối sai dạng.
- **`WorkingDirectory` (ngoài W9b, phán quyết của người dùng, header):** **executor của egret hoàn toàn không chạy `WorkingDirectory`** (§1: lập kế hoạch mờ, claim trả `None`). Vì vậy W9b chỉ làm model của nó thôi lặng lẽ bỏ `namespaceFiles` (`models/task.rs:555-565`). **Lúc publish**, khối được parse và kiểm tra theo cùng quy tắc như trên script task: khối sai dạng bị từ chối, nêu tên thuộc tính; khối hợp lệ được nhận và giữ trong model, cho gói sau sẽ chạy `WorkingDirectory`. **Lúc chạy**, không gì thay đổi: `WorkingDirectory` hoạt động đúng như trước W9b và không namespace file nào được stage. Hàm staging là hàm script task dùng, nên gói đó chỉ cần gọi nó. Hai flow corpus vẫn nằm trong baseline; Task 1 ghi kết quả hiện tại của chúng và Task 10 đo ratchet trước và sau (không dự kiến thay đổi). *(Sửa ngày 2026-10-05, Task 1 đo: không flow nào trong hai flow có trong `baseline.txt` hay `golden.json`. Artifact từ chối cả hai lúc publish — `corpus.json`: 422 `Invalid type: io.kestra.core.tasks.test.Read`, `task-type-not-in-image` — nên chúng không có entry golden và nằm ngoài 294 entry của ratchet; "vẫn nằm trong baseline" nghĩa là "vẫn nằm ngoài ratchet". Ratchet ở base là `294 golden entries, 108 mismatching, 108 baselined, 0 broken`.)*

## 8. Các task `io.kestra.plugin.core.namespace`

Handler native trong worker, như các task `core.storage` của W9a. Mỗi cái theo thuộc tính và output của lớp Kestra tương ứng (Task 1 trích và đo chúng).

### 8.1 `UploadFiles`

- `namespace`: mặc định của flow; khác thì bị từ chối (§5).
- `files`: danh sách mà mỗi phần tử là nguyên một URI `kestra://` hoặc glob trên **các file đọc được của execution** (tập đọc được của W9a §4.4, khớp trên path file của URI); Kestra glob trên working directory của chính nó (§2).
- `filesMap`: tên → URI `kestra://` (được kiểm quyền đọc) hoặc nội dung literal.
- `destination` mặc định `/`; `conflict` mặc định `OVERWRITE` (`ERROR`, `SKIP`).
- Mỗi file được ghi theo §4.2. Output: rỗng, như `run` của Kestra trả về.

### 8.2 `DownloadFiles`

`namespace` bắt buộc (quy tắc đọc), `files` glob, `destination` mặc định `""`. Mỗi file khớp được chép (streamed) thành một file của execution dưới prefix của task run (`tasks/{taskId}/{taskRunId}/{destination}{path}`), index lúc commit như file task của W9a. Output `files`: path → URI `kestra://`.

### 8.3 `DeleteFiles`

`namespace` (chỉ của chính nó), `files` glob, `deleteParentFolder` mặc định false (xoá thư mục cha bị rỗng, sâu nhất trước). Xoá mềm (§4.3).

### 8.4 `PurgeFiles`

- `namespaces` / `namespacePattern` (loại trừ nhau, như Kestra), mặc định namespace của chính flow; `includeChildNamespaces` mặc định true (khớp theo ranh giới dấu chấm); mọi namespace phân giải ra phải qua quy tắc purge (§5), và pattern chỉ được khớp với các namespace có file trong workspace.
- `filePattern`: wildcard Commons-IO (`*`, `?`) trên path, như `filterItems` của Kestra. *(Sửa ngày 2026-10-05, Task 1 đo: chuỗi được so khớp là path **không có** `/` đầu — `filePattern: "/q.txt"` không purge gì, `"q.txt"` purge hai revision — nên egret so pattern với path đã bỏ `/` đầu.)*
- `behavior`: `Version { keepAmount, before }`, mặc định `keepAmount: 1`. `keepAmount` giữ N revision cao nhất của mỗi path; `before` purge các revision cập nhật trước thời điểm đó và không bao giờ purge revision mới nhất; path đã xoá mềm bị purge toàn bộ (§2).
- **Xoá cứng:** dòng metadata trước, object sau (thứ tự an toàn của W9a: crash để lại object mồ côi, không bao giờ để lại dòng chỉ vào hư không). Output `size`: số revision đã purge.

## 9. API namespace file

Dưới `/api/ws/{ws}/workflows/namespaces/{ns}/files`, phản chiếu `NamespaceFileController`:

| route | quyền | trả lời |
|---|---|---|
| `GET …/files?path&revision` | `workflow:read` | các byte, streamed, với header download của W9a (`application/octet-stream`, `Content-Disposition: attachment` đã làm sạch, `X-Content-Type-Options: nosniff`, `storage/routes.rs:262`) |
| `GET …/files/stats?path` | `workflow:read` | kích thước, loại, revision, thời điểm tạo |
| `GET …/files/revisions?path` | `workflow:read` | mọi revision, đánh dấu cái đã xoá |
| `GET …/files/search?q` | `workflow:read` | các path còn sống chứa `q`, chặn theo số lượng |
| `GET …/files/directory?path` | `workflow:read` | các con còn sống của thư mục |
| `GET …/files/export` | `workflow:read` | zip streamed của các file còn sống mới nhất (không có flow) |
| `POST …/files?path` (multipart) | `workflow:manage` | §4.2 với `OVERWRITE`; dùng lại parser multipart có giới hạn của W9a; `.zip` được lưu như file, không bung |
| `POST …/files/directory?path` | `workflow:manage` | tạo thư mục và các cha |
| `PUT …/files?from&to` | `workflow:manage` | §4.4 |
| `DELETE …/files?path` | `workflow:manage` | §4.3, chỉ path được nêu và con của nó |

- `{ns}` được kiểm bằng `is_valid_namespace` và quy tắc segment `_files` (§3); namespace không có flow vẫn chứa được file. Path không biết → 404; namespace của workspace khác không với tới được theo cấu trúc (`{ws}` giới hạn mọi truy vấn).
- `/_flows` bị từ chối (400) trên mọi route; egret không import flow nào từ đây (§2).
- Có tài liệu utoipa; client frontend được sinh lại bằng `npm run gen:api`. UI là của W7.

## 10. Cầu plugin

- `runContext.storage().namespace(ns)` thôi ném W9b: `NO_NAMESPACE_FILES` (`ShimRunContext.java:274-280`) được thay bằng một factory trên các namespace file đã stage.
- **Staging:** egret stage mọi giá trị thuộc tính đã render là **nguyên một** URI `nsfile://` hay `kestra:///…/_files/…` (hoặc do `fileURI` tạo ra), được kiểm quyền đọc (§5), tại `.egret/storage/<n>` của W9a; `task.json` có thêm một mảng `namespaceFiles` riêng, mỗi entry một file đã stage với URI, namespace, path, revision, size và path đã stage; map `storage` của W9a (URI → path đã stage) không đổi (phán quyết của controller, header).
- `Namespace.getFileContent`, `exists`, `getFileMetadata`, `all`, `findAllFilesMatching`, `get` trả lời trên tập đã stage của namespace đó.
- `putFile`, `delete`, `move`, `createDirectory` ném `UnsupportedOperationException` kèm lý do: pod không giữ credential và không plugin nào trong catalog dùng chúng (Task 1 kiểm catalog). Plugin tính path namespace lúc chạy sẽ không thấy nó (§15).

## 11. Vòng đời và lỗi

- **Không có retention cho namespace file:** chúng là nội dung do tác giả viết, như Kestra. Revision đã xoá mềm và object của chúng ở lại tới khi `PurgeFiles` (hoặc một lần delete qua API rồi purge).
- **Pruner của W9a không bao giờ đụng `_files`:** nó chỉ xoá dưới prefix execution; một test ghi namespace file, chạy một lượt prune, và khẳng định mọi object và dòng còn nguyên.
- **Không xoá theo namespace** (không có bảng namespace, §15).
- **Lỗi**, như của W9a: vi phạm quy tắc path, namespace bị từ chối, object thiếu sau một dòng, xung đột là lỗi của tác giả nêu path; store hay database không với tới được là lỗi hạ tầng (worker trả `Err`, run được re-claim, không tốn attempt); vượt trần làm task fail nêu trần. Không lỗi nào nêu giá trị secret.

## 12. Task

1. **Đo Kestra** (`oracle-exec/w9b-namespace-files.json`): đường dẫn object theo revision, thư mục, xoá mềm rồi ghi lại; ngữ nghĩa glob (chuỗi path mà glob thấy, thêm `**/`, `regex:`, excludes); `fileURI` có và không có `revision` trên path có, thiếu và đã xoá; `read` một `kestra:///…/_files/…` của namespace khác; `ifExists` giữa các namespace, và `inputFiles` ghi đè namespace file cùng tên; output của bốn task; `PurgeFiles` trên path đã xoá mềm; quy tắc path; cộng các baseline (test, ratchet, clippy), và số lượng cùng danh sách các flow đã publish và entry corpus có namespace chứa segment `_files`.
2. **Metadata, URI, quy tắc path, thao tác store** (migration 0169, `Tail::NamespaceFile`, parse `nsfile://`, quy tắc segment `_files` lúc publish, `authorize`, ghi/đọc).
3. **Revision, xung đột, move, xoá mềm** (khoá, writer đồng thời, rollback).
4. **Các hàm biểu thức** (`fileURI`, bốn hàm trên namespace file, tra qua pool trên thread render).
5. **Staging `namespaceFiles`** (script task; lật cổng publish; trên `WorkingDirectory` chỉ parse và kiểm tra khối).
6. **`UploadFiles`, `DownloadFiles`.**
7. **`DeleteFiles`, `PurgeFiles`.**
8. **API namespace file** (route, utoipa, `npm run gen:api`).
9. **Cầu plugin** (staging, factory `Namespace`, lật `EgretStorageTest`).
10. **End to end trên một cluster kind tạm; coverage; tài liệu; ratchet; CI đầy đủ chạy local.**

## 13. Kiểm thử

- Unit: quy tắc path (mỗi lần từ chối có test hỏng khi nó bị gỡ), `parse` URI `_files` (kể cả `kestra:///a/_files/x/executions/<uuid>/f`), `nsfile://` có và không có authority, `authorize` (của chính nó, đọc namespace khác, ghi namespace khác, purge namespace con, `a.bc` không phải con của `a.b`, workspace khác).
- Postgres: đánh số revision dưới writer đồng thời (hai task, một path, revision khác nhau, không chung key); xoá mềm rồi ghi lại; đụng file/thư mục; rollback move (store hỏng giữa lúc chép); purge `keepAmount`/`before`/đã xoá mềm; cô lập workspace.
- Test phá huỷ chỉ chạy trên `MemStore` hoặc dưới prefix dùng một lần trên MinIO; không bao giờ xoá ở gốc bucket dùng chung.
- MinIO: upload và download streamed vượt một part.
- API: phân quyền (`workflow:read` vs `workflow:manage`), 404 cho workspace khác và path không biết, header download, từ chối `/_flows`, giới hạn multipart.
- Docker và Kubernetes sống: `namespaceFiles` trên script task với nhiều namespace, `folderPerNamespace`, từng `ifExists`.
- Pruner: một lượt để nguyên `_files`.
- Oracle: ratchet được đo trước và sau và ghi trong `oracle-exec/README.md`; hai flow corpus là `WorkingDirectory` và vẫn nằm trong baseline, nên không dự kiến ratchet thay đổi (§7). Publish: `namespaceFiles` sai dạng trên `WorkingDirectory` bị từ chối theo tên và khối hợp lệ publish được; namespace có segment `_files` bị từ chối theo tên lúc publish, bởi API, bởi các task và bởi các hàm.
- Sống: một entry `inputFiles` ghi đè namespace file cùng tên dưới mọi `ifExists`.
- End to end trên một cluster kind tạm, như W9a.

## 14. Ngoài phạm vi

KV store (W9c); chạy chính `WorkingDirectory` (và vì thế `namespaceFiles`, `inputFiles`, `outputFiles`, `cache` của nó); bung `.zip` và import `_flows` khi upload; xoá namespace và quyền cấp namespace; UI (W7); plugin ghi namespace file.

## 15. Giới hạn đã biết

- Một flow chỉ ghi vào namespace của chính nó; một flow Kestra upload vào, xoá khỏi hay purge namespace khác sẽ fail trên egret, nêu namespace (§5).
- `PurgeFiles` không có `namespaces` purge namespace của chính flow (và các con), không phải mọi namespace (§8.4).
- `namespaceFiles` của `WorkingDirectory` được kiểm tra lúc publish và giữ lại, nhưng không chạy, vì egret không chạy `WorkingDirectory`; chạy nó là một gói sau (§7).
- `ifExists` không bảo vệ namespace file khỏi một entry `inputFiles` cùng tên, như Kestra (§7).
- Plugin chỉ đọc được namespace file được nêu bằng nguyên một giá trị thuộc tính; plugin tính path namespace lúc chạy, hoặc ghi namespace file, sẽ fail kèm lý do (§10).
- Object thiếu sau một dòng metadata là lỗi, trong khi Kestra phục vụ revision cũ hơn (§4.5).
- `read` một file nhị phân là lỗi, trong khi Kestra trả byte (§6).
- `UploadFiles.files` glob trên file của execution, không trên working directory (§8.1).
- Không có retention: namespace file và revision đã xoá mềm giữ storage tới khi bị purge (§11).
- Namespace có segment `_files` bị từ chối (§3).
- Không có bảng namespace, nên xoá mọi flow của một namespace vẫn để lại file của nó; chúng với tới được qua API (§9).
