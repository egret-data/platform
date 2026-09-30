# W9a — Internal storage: file `kestra://` cho execution, task, plugin và các hàm file

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-01-workflow-w9a-internal-storage-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, chờ duyệt
**Phụ thuộc:** W4 (lease fence, `apply_outcome`), W5a (task runner, chuyển file input/output), W5b (plugin host, catalog, `task.json`), W6/W13 (`trigger_execution_id`), W10 (route execution), W11 (mẫu pruner) — đều đã merge. Base `ecdf4868`.
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 phát hiện 4 (`StorageInterface`), §9 (dòng W9), §11 câu hỏi mở 1
**Đã quyết với người dùng, 2026-10-01:**
- **W9 được tách thành ba gói:** W9a internal storage (spec này), rồi W9b namespace files, rồi W9c KV store.
- **egret chuyển file cho pod.** Một task pod không giữ credential storage nào và không mở kết nối nào tới object store. egret đưa mọi file task có thể đọc vào thư mục làm việc của nó và thu mọi file nó ghi, qua cơ chế chuyển file sẵn có của W5a (§5).
- **Streamed, với trần lớn cấu hình được.** `egret-storage` có thêm streaming; không file nào được giữ nguyên vẹn trong bộ nhớ của egret (§4.1).
- **Một flow đang chạy đọc file của execution của chính nó và của trigger execution của nó**, không bao giờ của workspace khác, và không bao giờ của một execution không liên quan trong cùng workspace (§4.4). Điều này cố ý hẹp hơn Kestra OSS (§2).
- **Retention riêng, mặc định 30 ngày**, bằng một pruner theo mẫu của W11 (§10).
- **`outputFiles` trở thành map tên → URI `kestra://`, như Kestra.** Điều này làm hỏng các flow egret đọc map tên → nội dung của W5a (§6.2).
- **Trong phạm vi:** FILE input và các hàm file (§7), API file của execution (§8), cầu storage cho plugin Kestra (§9.1), `egret.sql` với `fetchType: STORE` (§9.2), và các task storage lõi cùng `ForEach` trên một file (§9.3).

**Bàn giao:** một PR (tài liệu + mã), các task ở §12.

## 1. Cái gì đã có, cái gì chưa

| phần | trạng thái trên `origin/main` (`ecdf4868`) |
|---|---|
| object store | `egret-storage`: trait `ObjectStore { put, get, delete, copy, list }` trên buffer `Vec<u8>` nguyên vẹn, URI `s3://bucket/key`; một backend, `S3Store` (S3 và MinIO); cấu hình bởi `[storage]` (endpoint, region, bucket `warehouse`, keys, `path_style`). `egret-workflow` chỉ có nó như dev-dependency |
| hàm file | `runners::functions::REFUSED` từ chối `read`, `fileURI`, `fileSize`, `fileExists`, `isFileEmpty` với "…, W9" (`functions.rs:245-263`) |
| FILE input | `InputType::File` đã có (`models/input.rs`); route start chỉ nhận một JSON object; body webhook multipart bị từ chối |
| `outputFiles` | map tên → **nội dung** (UTF-8, ≤ 1 MiB mỗi file, `MAX_OUTPUT_FILE_BYTES`), được ghi nhận là một điểm khác Kestra (`handlers/container.rs:46-58`) |
| chuyển file | W5a: Docker `docker cp` một thư mục staging; Kubernetes exec `tar -x` vào và `tar -c` ra, với một bộ đóng gói ustar **trong bộ nhớ** (`worker/k8s/transfer.rs`) |
| plugin | catalog của W5b bỏ `postgresql.CopyIn`, `postgresql.CopyOut`, `postgresql.Batch`, `mysql.Batch` (`EgretPluginHost.NEEDS_STORAGE`), và từ chối `fetchType: STORE` lúc publish (`plugins/mod.rs`) |
| `egret.sql` | `Fetch` cố ý bỏ `STORE` (`worker/handlers/sql.rs:143`) |
| `ForEach` trên một file | một lỗ hổng có tên trong `advance()` (`executor/flowable/loop_.rs:171`): một `kestra:///….ion` đã render đến như một chuỗi thường |
| ION | `runners::ionfmt` ghi ION đúng như `FileSerde` của Kestra; `ion-rs` bị loại làm writer (`Cargo.toml:173`) |
| liên kết execution | `trigger_execution_id` (0138, FK ghép theo workspace): được đặt cho execution con của `Subflow` và execution do trigger `Flow` bắn. `parent_execution_id` (0135): sub-execution của một vòng lặp |
| retention | pruner của W11 (`logs/prune.rs`): mỗi giờ, theo batch, có ngân sách mỗi lượt, `log_retention_days` mặc định 30, 0 = mãi mãi |

## 2. Kestra làm gì, và egret khác ở đâu

Kestra (checkout `96893b589`, artifact `2.0.0-rc13`):
- `StorageInterface` (`core/…/storages/StorageInterface.java`): `get`, `getWithMetadata`, `allByPrefix`, `list`, `exists`, `getAttributes`, `put`, `delete`, `createDirectory`, `move`, `deleteByPrefix`, `purgeByLastModified`, cùng các biến thể `*InstanceResource`. `InternalStorage` là facade theo từng run (`getFile`, `putFile`, `isFileExist`, `deleteExecutionFiles`, …).
- URI (`StorageContext.java:41-57`): `kestra:///{ns-as-path}/{flow}/executions/{exec}/…` với `inputs/{input}/{file}`, `tasks/{taskId}/{taskRunId}/{file}`, `trigger/{triggerId}/{file}`; dấu chấm của namespace thành dấu gạch chéo; id của flow, task và trigger được slugify. Tenancy nằm trong đường dẫn backend, không nằm trong URI.
- Một file output được lưu dưới tên `IdUtils.from(path) + "-" + filename` với dấu cách thành `+` (`FilesService.java:94-97`): **tất định**, không ngẫu nhiên.
- Hàm file (`AbstractFileFunction.java:136-171`) cho phép: file của execution hiện tại; nếu không thì execution được biến `trigger` chỉ tới (một cấp: cha của subflow hoặc execution đã bắn một trigger `Flow`); nếu không thì namespace bất kỳ mà `isAllowedNamespace` chấp nhận — **mà trong OSS là mọi namespace** (`DefaultNamespaceService.java:38-41`).
- API file của execution (`ExecutionController.java:1173-1328, 2285-2338`): `GET /{id}/file`, `/file/metas`, `/file/preview` (100 dòng, tối đa 5000); URI của execution khác bị redirect.
- Vòng đời: không gì tự hết hạn. Xoá một execution xoá prefix của nó; `PurgeExecutions`, `PurgeCurrentExecutionFiles`, `PurgeStorage` là các task.

| Kestra | egret W9a | vì sao |
|---|---|---|
| một flow có thể đọc file task của mọi execution trong OSS (`isAllowedNamespace` luôn true) | chỉ execution của chính nó, loop root của nó, và trigger execution của nó | egret có ranh giới người đọc (`workflow:read`) mà Kestra OSS không có; một tác giả flow có `workflow:manage` không được đọc output của một flow họ không thấy. **Một flow Kestra chuyển một URI cho một flow không liên quan thì chạy trên Kestra và thất bại trên egret** |
| một plugin đọc mọi URI nó tính ra lúc chạy | một plugin đọc các URI có mặt trong thuộc tính đã render của nó | pod không giữ credential (§5). Plugin JDBC không bị ảnh hưởng; một plugin tương lai tính URI lúc chạy sẽ thất bại, có tên |
| file được giữ mãi mãi | được prune 30 ngày sau khi execution kết thúc (cấu hình được, 0 = mãi mãi) | lựa chọn của người dùng (§10). Một flow đọc file của execution cũ hơn cửa sổ này thì thất bại, nêu URI và cửa sổ |
| `read()` không có trần | `workflow.storage.max_read_bytes`, mặc định 10 MiB | kết quả được ghép vào một chuỗi đã render |
| API file redirect tới execution sở hữu | 404 | người xem hỏi execution sở hữu bằng id của chính nó |
| `file://`, body webhook multipart, `nsfile://` | bị từ chối | ngoài phạm vi; `nsfile://` và `fileURI` là W9b |

## 3. Bố cục storage và URI

- **Object key:** `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/{flow}/executions/{exec}/{tail}` trong bucket `[storage]`. `workflow.storage.prefix` mặc định là `workflow/`.
- **URI một flow thấy:** `kestra:///{ns-as-path}/{flow}/executions/{exec}/{tail}` — dạng của Kestra. **Nó không mang workspace**: egret ánh xạ một URI sang key bằng cách đặt tiền tố là workspace **của execution đang chạy**, không bao giờ là giá trị đọc từ URI, nên không gì trong một URI có thể chỉ tới workspace khác.
- `tail` là của Kestra: `inputs/{input}/{file}`, `tasks/{taskId}/{taskRunId}/{file}`, `trigger/{triggerId}/{file}`. Slugify và tên file output theo đúng Kestra; Task 1 đo cả hai trên Kestra và ghim vào một fixture (§11).
- **`{exec}` là loop root.** egret chạy một vòng lặp như sub-execution (`parent_execution_id`); Kestra chạy nó bên trong một execution duy nhất. Vì vậy một file ghi bên trong một vòng lặp dùng id ở đầu chuỗi `parent_execution_id`, để URI của nó là URI Kestra sẽ cho.
- Một URI bị từ chối nếu scheme của nó không phải `kestra`, nếu nó chứa `..`, hoặc nếu nó không parse được vào bố cục trên.

## 4. Lõi

### 4.1 Streaming trong `egret-storage`

`ObjectStore` giữ các method `Vec<u8>` cho caller hôm nay và có thêm `put_stream(uri, AsyncRead, size_hint)` (một PUT đơn dưới 8 MiB, nếu không thì multipart upload với part 8 MiB, huỷ khi lỗi), `get_stream(uri) -> AsyncRead`, `head(uri) -> Option<size>` và `delete_prefix(uri)`. Test trên MinIO thật với một file lớn hơn một part, đo rằng bộ nhớ resident của process không tăng theo file.

### 4.2 Index

`workflow.execution_files`: `workspace_id`, `execution_id` (loop root; FK ghép với `workspace_id`, `ON DELETE CASCADE`), `task_run_id` (nullable), `uri`, `size`, `created_at`; unique `(workspace_id, uri)`.

**Một file chỉ tồn tại với flow khi dòng index của nó đã tồn tại.** Mỗi lần ghi là: stream vào store, `head` lấy size thật, insert dòng index. `fileExists`, `fileSize` và `isFileEmpty` trả lời từ index; mọi lần đọc tra index trước.

### 4.3 Khi nào một file được commit

File của task được upload trước khi outcome commit, và các dòng index của chúng được insert **trong transaction commit outcome của task run**, sau lease fence của W4. Một worker bị fence hoặc sập chỉ để lại các object chưa được index (§10). Vì tên là tất định theo từng task run, một lần re-claim cùng task run ghi cùng key và ghi đè orphan của chính nó. File input commit cùng dòng execution (§7.1).

### 4.4 Ai được đọc một URI

Một hàm duy nhất, `storage::resolve_readable(workspace, current_execution, uri) -> Result<IndexedFile>`, được dùng bởi các hàm file, staging (§5) và cầu plugin (§9.1). Nó cho phép một URI mà dòng index của nó thuộc, trong workspace của execution đang chạy, về:
1. loop root của execution đang chạy; hoặc
2. `trigger_execution_id` của loop root (một cấp, quy tắc `trigger` của Kestra); hoặc
3. với một replay, execution mà nó được replay từ đó — Task 2 đo cách một replay của W6 ghi lại nguồn của nó và thêm liên kết nếu chưa có.

Mọi thứ khác là một lỗi nêu URI, không bao giờ là `false` hay rỗng.

### 4.5 Trần

`workflow.storage.max_file_bytes` (mặc định 1 GiB) mỗi file và `workflow.storage.max_task_bytes` (mặc định 5 GiB) mỗi task run. Vượt một trong hai thì task thất bại, nêu trần đó.

## 5. Chuyển file vào và ra khỏi pod

Cơ chế chuyển của W5a trở thành streamed: `get_stream` nuôi một streaming tar writer vào `docker cp` / exec `tar -x`; `tar -c` được đọc từng entry và mỗi entry stream sang `put_stream`. Bộ đóng gói trong bộ nhớ bị thay thế, không giữ song song.

**Vào.**
- **Task script:** một giá trị `inputFiles` là URI `kestra://` được staging dưới tên file đó, như Kestra; mọi giá trị khác vẫn là nội dung, như W5a.
- **Task plugin:** egret quét `task.json` đã render tìm các chuỗi `kestra://` và staging từng cái ở `.egret/storage/<n>`; `task.json` có thêm `storage: {uri → path}` và `baseUri` (tiền tố URI của task run).
- **Một URI mà `resolve_readable` từ chối làm task thất bại lúc render, trước khi có container nào**, nêu URI.

**Ra.**
- **Task script:** `outputFiles` nhận glob, như Kestra, và `outputDirectory: true` được hỗ trợ; các file khớp được stream tới `tasks/{taskId}/{taskRunId}/{Kestra name}` và `outputs.outputFiles.<relative path>` là URI.
- **Plugin:** `putFile` của host ghi dưới `.egret/out/`, tính URI từ `baseUri` và trả nó trong output của plugin; egret thu `.egret/out/` và index đúng các URI đó.
- Sau khi thu, **mọi URI dưới prefix của task run này xuất hiện trong outputs phải đã được thu**, nếu không task thất bại: không output nào trỏ vào hư không.

**Re-attach (W5a):** việc thu chạy như thường trên pod được nhận lại; không gì được index trước commit, nên không gì bị nhân đôi.

## 6. Task script

### 6.1 URI trong `inputFiles`

Như §5. Dạng nội dung của W5a không đổi.

### 6.2 `outputFiles` trở thành URI — breaking

`outputs.<task>.outputFiles` trở thành `{relative path → URI kestra://}`. Giới hạn 1 MiB và UTF-8 của W5a bị bỏ; nội dung được đọc bằng `{{ read(outputs.t.outputFiles['a.txt']) }}`. **Breaking:** một flow dùng dạng nội dung của W5a giờ nhận một URI. Phát hành sau `workflow_containers` (tắt mặc định); release note nêu rõ.

## 7. FILE input và các hàm file

### 7.1 FILE input

- Route start nhận `multipart/form-data`: mỗi part được đặt tên bằng một input id, một part file là giá trị của FILE input đó (dạng của Kestra). JSON vẫn giữ.
- egret sinh execution id, stream từng file tới `inputs/{input}/{file}`, và insert dòng execution cùng các dòng index trong một transaction.
- `allowedFileExtensions` được thực thi; giới hạn body của route theo `max_file_bytes`.
- Trong JSON, một FILE input chỉ có thể là một URI `kestra://` đọc được (§4.4) từ execution đang được tạo: các trường hợp `Subflow` và trigger `Flow`. Mọi chuỗi khác bị từ chối, nêu input.

### 7.2 Các hàm

| hàm | hành vi |
|---|---|
| `read(uri)` | nội dung dạng UTF-8; không phải UTF-8 là lỗi nêu URI; bị chặn bởi `max_read_bytes` |
| `fileSize(uri)` | từ index |
| `fileExists(uri)` | từ index; một URI không đọc được là lỗi (Task 1 đo câu trả lời của Kestra và spec theo nó) |
| `isFileEmpty(uri)` | từ index (`size == 0`) |

- Cả bốn đều đi qua `resolve_readable`. `fileURI` vẫn bị từ chối, giờ nêu W9b.
- Chúng tồn tại trong mọi lần render có execution (thuộc tính task, `runIf`, `Log`, outputs); một lần render không có (điều kiện của trigger, publish) từ chối chúng, có tên.
- Render là đồng bộ và đã chạy trên luồng riêng (`render_on_this_thread`): các tra cứu index được prefetch cho mọi literal `kestra://` trong văn bản và tra lại cho cái được ghép lúc render; `read` block trên runtime handle cho một lần đọc có trần.

## 8. API file của execution

| route | trả lời |
|---|---|
| `GET /api/ws/{ws}/workflows/executions/{id}/file?path=kestra:///…` | các byte thô, streamed |
| `GET …/executions/{id}/file/metas?path=…` | `{size}` từ index |
| `GET …/executions/{id}/file/preview?path=…&maxRows=100` | tối đa `maxRows` (max 5000): `.ion` thành các dòng JSON (đọc bằng `ion-rs`, reader của nó không bị ảnh hưởng bởi định dạng của writer), `.csv`/`.jsonl`/`.json`/text thành các dòng, nếu không thì `{previewable: false}`; ngừng đọc khi đã đầy |

- Quyền `workflow:read`; `path` phải là một dòng index **của execution `{id}`** trong workspace `{ws}` (loop root của nó), nếu không 404 — không redirect.
- Download là `application/octet-stream`, `Content-Disposition: attachment` với tên đã làm sạch, `X-Content-Type-Options: nosniff`.
- Có tài liệu utoipa; client frontend được sinh lại bằng `npm run gen:api`. UI là của W7.

## 9. Bên tiêu thụ

### 9.1 Cầu storage cho plugin

Trong host, `EgretStorage implements StorageInterface`, bọc trong `InternalStorage` công khai của Kestra. Nó chỉ đọc map `storage` và `baseUri` của `task.json`, không giữ credential và không mở kết nối.

| method | hành vi |
|---|---|
| `get`, `getWithMetadata`, `exists`, `getAttributes` | file đã staging, và file task này đã ghi; nếu không thì `FileNotFoundException` nêu URI |
| `put` | chỉ dưới `baseUri`; ghi `.egret/out/{relative}`; đếm byte theo `max_task_bytes` (truyền trong `task.json`) và thất bại trong pod |
| `list`, `allByPrefix` | file đã staging và đã ghi, lọc theo prefix |
| `delete`, `deleteByPrefix`, `move`, `createDirectory`, `purgeByLastModified` | chỉ file do chính task này ghi; mọi thứ khác ném `UnsupportedOperationException` kèm lý do |
| `*InstanceResource` | ném, kèm lý do |

Catalog nhận lại `postgresql.CopyIn`, `postgresql.CopyOut`, `postgresql.Batch`, `mysql.Batch`, và publish thôi từ chối `fetchType: STORE`. Mọi thuộc tính mà Task 1 của W5b đo là cần storage được đo lại và hết bị từ chối từng cái một. Mỗi test W5b khẳng định "bị từ chối, cần storage" đảo chiều, và mỗi lần đảo có một test thay thế chứng minh plugin chạy. File một plugin ghi không bị che, như outputs không bị che (W5b §12).

### 9.2 `egret.sql` với `fetchType: STORE`

`Fetch` có thêm `Store`. Handler chạy trong egret, nên nó stream các dòng qua `runners::ionfmt` vào `put_stream` tại `tasks/{taskId}/{taskRunId}/{name}.ion`, và outputs của nó là `{uri, size}` với `size` là số dòng — dạng của output STORE ở `AbstractJdbcQuery` của Kestra (Task 1 ghim nó). **Bộ nhớ:** STORE chỉ chặn được bộ nhớ nếu connector trả các dòng như một stream; Task 1 đo Trino và StarRocks và spec ghi lại cái nào làm được. Một connector buffer vẫn cho ra đúng dạng output, dưới giới hạn số dòng sẵn có của nó.

### 9.3 Task storage lõi và `ForEach` trên một file

Handler native cho `io.kestra.plugin.core.storage.{Write, Concat, Split, Size, Delete, Reverse, DeduplicateItems, FilterItems, PurgeCurrentExecutionFiles}`, mỗi cái theo thuộc tính và outputs của lớp Kestra tương ứng (Task 1 trích chúng). `Delete` và `PurgeCurrentExecutionFiles` chỉ xoá file của loop root của execution đang chạy, xoá dòng index trước. `PurgeStorage` ngoài phạm vi (một task quản trị xuyên các execution).

`ForEach` với `values` là một URI `kestra://`: **worker** phân giải và đọc file (ION, mỗi record một giá trị, như Kestra) **trước khi** nó gọi `advance()`, và đưa danh sách cho `advance()`; `advance()` vẫn thuần. Danh sách bị chặn bởi giới hạn độ rộng vòng lặp sẵn có. Các flow trong corpus `valids/loop-values-from-uri` và `valids/loop-outputs-auto` là mục tiêu.

## 10. Retention và orphan

- `workflow.storage_retention_days`, mặc định 30; 0 giữ mãi mãi.
- Một pruner mỗi giờ với ngân sách mỗi lượt, theo mẫu của W11, lấy các execution **đã kết thúc** hơn N ngày trước mà còn dòng index, gọi `delete_prefix`, rồi xoá các dòng index. Nếu xoá ở store thất bại, các dòng còn lại và lượt sau thử lại. **Một execution chưa kết thúc không bao giờ bị prune.**
- **Orphan** (đã upload, chưa từng commit): với mỗi execution bị prune và mỗi execution FAILED/KILLED đã kết thúc, liệt kê prefix của nó và xoá các object không có dòng index và cũ hơn 24 giờ.
- Một URI mà file của nó đã bị prune là một lỗi nêu URI và cửa sổ retention.

## 11. Lỗi

Mọi lỗi storage nêu URI và không bao giờ nêu giá trị secret. Một store không với tới được lúc chạy là lỗi hạ tầng: worker trả `Err`, lease hết hạn, run được re-claim, và không tốn `attempts` (W4). Vượt một trần làm task thất bại, nêu trần đó.

## 12. Các task

1. **Đo Kestra** (fixture dưới `oracle-exec/`): slugify; tên file output; `fileExists` trên URI không đọc được; output STORE; thuộc tính và outputs của các task storage lõi; `ForEach` trên một URI ION; streaming dòng của Trino và StarRocks trong connector của egret.
2. **`egret-storage` streaming; index; URI; `resolve_readable`** (migration: `workflow.execution_files`, và liên kết replay nếu thiếu).
3. **Chuyển pod streamed; URI trong `inputFiles`; `outputFiles` là URI.**
4. **FILE input; các hàm file.**
5. **API file của execution.**
6. **Cầu storage cho plugin và catalog.**
7. **`egret.sql` STORE.**
8. **Task storage lõi và `ForEach` trên một file.**
9. **Pruner.**
10. **End to end trên một cluster kind tạm; coverage; tài liệu; full CI cục bộ.**

## 13. Kiểm thử

- Unit: URI ↔ key, slugify, `resolve_readable` (của chính nó, loop root, trigger, replay, execution khác, workspace khác), bảng hàm.
- MinIO: streaming vượt một part, bộ nhớ đo được là phẳng.
- Docker và Kubernetes sống: glob `outputFiles`, URI `inputFiles`, tar streamed.
- Plugin sống: `Query` STORE → `CopyIn` của URI đó → `read()`.
- API: 404 cho file của execution khác và của workspace khác; header download; preview ION.
- Pruner: execution chưa kết thúc được giữ; orphan bị xoá; xoá store thất bại được thử lại.
- Oracle: ratchet được **đo** trước và sau và ghi trong `oracle-exec/README.md`; corpus có 5 flow FILE input, 1 `outputFiles`, 2 `STORE`, 2 `core.storage`.
- End to end trên một cluster kind tạm, như W5b.

## 14. Ngoài phạm vi

Namespace files, `nsfile://` và `fileURI` (W9b); KV store (W9c); `PurgeStorage` và `PurgeExecutions`; `file://`; body webhook multipart; đẩy log sang object storage (§11 câu hỏi mở 1); backend đĩa cục bộ.

## 15. Giới hạn đã biết

- Một flow chỉ đọc file của execution của chính nó, loop root của nó và trigger execution của nó (§2).
- Một plugin chỉ đọc các URI có mặt trong thuộc tính đã render của nó (§2).
- File được prune 30 ngày sau khi execution kết thúc theo mặc định; Kestra giữ chúng.
- `outputFiles` đổi từ nội dung sang URI (§6.2).
- STORE chỉ chặn bộ nhớ trên các connector stream dòng (§9.2).
- Mọi deployment chạy workflow container cần một bucket S3 hoặc MinIO với tới được.
