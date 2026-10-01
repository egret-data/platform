# W9a — Internal storage: file `kestra://` cho execution, task, plugin và các hàm file

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-01-workflow-w9a-internal-storage-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, sửa ngày 2026-10-01 cho khớp với plan (`docs/en/plans/2026-10-01-workflow-w9a-internal-storage.md`)
**Phụ thuộc:** W4 (lease fence, `apply_outcome`), W5a (task runner, chuyển file input/output), W5b (plugin host, catalog, `task.json`), W6/W13 (`trigger_execution_id`), W10 (route execution), W11 (mẫu pruner) — đều đã merge. Base `ecdf4868`.
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 phát hiện 4 (`StorageInterface`), §9 (dòng W9), §11 câu hỏi mở 1
**Đã quyết với người dùng, 2026-10-01:**
- **(Thêm ngày 2026-10-01, phán quyết của người dùng về mục 1 của plan.) File được tra trong một snapshot chụp trước khi render; không có lần tra thứ hai lúc render.** Một URI được ghép lúc render (nối chuỗi) mà không có trong snapshot là một lỗi nêu URI đó (§4.4, §7.2).
- **(Thêm ngày 2026-10-01, phán quyết của người dùng về mối lo 1 của Task 1.) Một file ghi bên trong một vòng lặp `Loop` mang execution id của chính vòng lặp đó, như Kestra; không có ánh xạ URI về loop root.** Vì vậy tập đọc được bao gồm mọi vòng lặp dưới cùng loop root, và API file phục vụ cả họ vòng lặp (§3, §4.2, §4.4, §8).
- **W9 được tách thành ba gói:** W9a internal storage (spec này), rồi W9b namespace files, rồi W9c KV store.
- **egret chuyển file cho pod.** Một task pod không giữ credential storage nào và không mở kết nối nào tới object store. egret đưa mọi file task có thể đọc vào thư mục làm việc của nó và thu mọi file nó ghi, qua cơ chế chuyển file sẵn có của W5a (§5).
- **Streamed, với trần lớn cấu hình được.** `egret-storage` có thêm streaming; không file nào được giữ nguyên vẹn trong bộ nhớ của egret (§4.1).
- **Một flow đang chạy đọc file của execution của chính nó và của trigger execution của nó** (và một replay đọc file của nguồn của nó, §4.4), không bao giờ của workspace khác, và không bao giờ của một execution không liên quan trong cùng workspace (§4.4). Điều này cố ý hẹp hơn Kestra OSS (§2).
- **Retention riêng, mặc định 30 ngày**, bằng một pruner theo mẫu của W11 (§10).
- **`outputFiles` trở thành map tên → URI `kestra://`, như Kestra.** Điều này làm hỏng các flow egret đọc map tên → nội dung của W5a (§6.2).
- **Trong phạm vi:** FILE input và các hàm file (§7), API file của execution (§8), cầu storage cho plugin Kestra (§9.1), `egret.sql` với `fetch: STORE` (§9.2), và các task storage lõi cùng `Loop` trên một file (§9.3; cái tên `ForEach` của chương trình là `io.kestra.plugin.core.flow.Loop` của Kestra trong checkout này).

**Bàn giao:** một PR (tài liệu + mã), mười bốn task ở §12, thực hiện theo plan `docs/en/plans/2026-10-01-workflow-w9a-internal-storage.md`.

## 1. Cái gì đã có, cái gì chưa

| phần | trạng thái trên `origin/main` (`ecdf4868`) |
|---|---|
| object store | `egret-storage`: trait `ObjectStore { put, get, delete, copy, list }` trên buffer `Vec<u8>` nguyên vẹn, URI `s3://bucket/key`; một backend, `S3Store` (S3 và MinIO); cấu hình bởi `[storage]` (endpoint, region, bucket `warehouse`, keys, `path_style`). `egret-workflow` chỉ có nó như dev-dependency |
| hàm file | `runners::functions::REFUSED` từ chối `read`, `fileURI`, `fileSize`, `fileExists`, `isFileEmpty` với "…, W9" (`functions.rs:245-263`) |
| FILE input | `InputType::File` đã có (`models/input.rs`); route start chỉ nhận một JSON object; body webhook multipart bị từ chối |
| `outputFiles` | map tên → **nội dung** (UTF-8, ≤ 1 MiB mỗi file, `MAX_OUTPUT_FILE_BYTES`), được ghi nhận là một điểm khác Kestra (`handlers/container.rs:46-58`) |
| chuyển file | W5a: Docker `docker cp` một thư mục staging; Kubernetes exec `tar -x` vào và `tar -c` ra, với một bộ đóng gói ustar **trong bộ nhớ** (`worker/k8s/transfer.rs`) |
| plugin | catalog của W5b bỏ `postgresql.CopyIn`, `postgresql.CopyOut`, `postgresql.Batch`, `mysql.Batch` (`EgretPluginHost.NEEDS_STORAGE`), và từ chối `fetchType: STORE` lúc publish (`plugins/mod.rs`) |
| `egret.sql` | `Fetch` cố ý bỏ `STORE` (`worker/handlers/sql.rs:143`). `QueryExecPort::execute_as_with_params` trả về nguyên một `TabularResult`, bị chặn bởi `Sql::max_rows` (`MAX_FETCH_ROWS`, 10 000). **`egret.sql` không nằm trong registry production nào** (`Handlers::with_core_tasks` và `runtime::handlers_for` không thêm nó; `worker/registry.rs:90-120` nói vậy) |
| `Loop` trên một file | một lỗ hổng có tên trong `advance()` (`executor/flowable/loop_.rs:171`): một `kestra:///….ion` đã render đến như một chuỗi thường. Trong mã không có giới hạn độ rộng vòng lặp nào |
| ION | `runners::ionfmt` ghi Ion **text** đúng như filter `toIon` của Kestra (`JacksonMapper.ofIon()`); `FileSerde` của Kestra, thứ ghi file STORE của plugin và file của các task item, ghi Ion 1.0 **nhị phân** (Task 1, finding `store_binary_ion` trong `w9a-storage.json`), nên `ionfmt` không tái tạo các file đó; `ion-rs` bị loại làm writer (`Cargo.toml:173`) và không có trong `Cargo.lock`. `ionfmt::read_ion` (đứng sau `fromIon`) từ chối timestamp và blob |
| liên kết execution | `trigger_execution_id` (0138, FK ghép theo workspace): được đặt cho execution con của `Subflow` và execution do trigger `Flow` bắn. `parent_execution_id` (0135): sub-execution của một vòng lặp, **và** nguồn của một replay do retry (`CREATE_NEW_EXECUTION`), phân biệt bằng `loop_index` (NULL với replay, 0138). Một replay **thủ công** (`POST …/executions/{id}/replay`) không ghi liên kết nào (chính doc của `replay_execution` trong `routes.rs`) |
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
| một flow có thể đọc file task của mọi execution trong OSS (`isAllowedNamespace` luôn true) | chỉ execution của chính nó, loop root của nó và mọi vòng lặp dưới root đó, trigger execution của nó, và chuỗi replay của nó (§4.4) | egret có ranh giới người đọc (`workflow:read`) mà Kestra OSS không có; một tác giả flow có `workflow:manage` không được đọc output của một flow họ không thấy. **Một flow Kestra chuyển một URI cho một flow không liên quan thì chạy trên Kestra và thất bại trên egret** |
| một plugin đọc mọi URI nó tính ra lúc chạy | một plugin đọc các URI có mặt trong thuộc tính đã render của nó | pod không giữ credential (§5). Plugin JDBC không bị ảnh hưởng; một plugin tương lai tính URI lúc chạy sẽ thất bại, có tên |
| file được giữ mãi mãi | được prune 30 ngày sau khi execution kết thúc (cấu hình được, 0 = mãi mãi) | lựa chọn của người dùng (§10). Một flow đọc file của execution cũ hơn cửa sổ này thì thất bại, nêu URI và cửa sổ |
| `read()` không có trần | `workflow.storage.max_read_bytes`, mặc định 10 MiB | kết quả được ghép vào một chuỗi đã render |
| API file redirect (301) tới execution sở hữu (Task 1 đo) | 200 cho một file thuộc họ vòng lặp của `{id}` (loop root của nó và mọi vòng lặp dưới nó), nếu không 404, không redirect (§8) | trong họ vòng lặp, tập đọc được đã nối các execution với nhau (§4.4); ngoài đó người xem hỏi execution sở hữu bằng id của chính nó |
| một hàm file đọc mọi URI mà template tính ra | một URI phải có trong snapshot chụp trước khi render: trong source của flow, hoặc trong một giá trị JSON của render context (inputs, outputs, execution, item của vòng lặp). URI ghép bằng nối chuỗi là một lỗi nêu URI đó | phán quyết của người dùng (phần đầu): trong `settle` luồng render không dùng được connection của transaction, và một pool sẽ không thấy các dòng transaction đó vừa ghi (§4.4) |
| `Loop` đọc file ION theo từng batch `concurrencyLimit`, không có trần | worker đọc cả file trước `advance()`, bị chặn bởi `workflow.storage.max_read_bytes` | `advance()` vẫn thuần; không có giới hạn độ rộng vòng lặp nào để dùng lại (§9.3) |
| file output là `IdUtils.from(<đường dẫn tuyệt đối trong một thư mục tạm ngẫu nhiên>) + "-" + name` | cùng scheme trên `/egret/working-dir/<rel>` cố định của egret | scheme của tên khớp; hash thì không thể, vì thư mục của Kestra là ngẫu nhiên |
| một replay đọc thứ mà biến `trigger` của nó chỉ tới | một replay còn đọc file của nguồn của nó, bắc cầu dọc chuỗi replay | một replay sao chép nguyên `inputs` của nguồn, nên URI của một FILE input chỉ tới execution gốc (§4.4) |
| artifact **không thực thi** `allowedFileExtensions` (Task 1: `bad.csv` được nhận khi start, và `POST …/validate` không báo lỗi) | thực thi khi start (§7.1) | lựa chọn: thuộc tính nói đúng điều nó nghĩa. Một flow dựa vào việc Kestra bỏ qua nó sẽ thất bại trên egret |
| file STORE của JDBC và output của các task item là Ion 1.0 **nhị phân** (Task 1) | STORE của `egret.sql` ghi Ion **text** qua `ionfmt` (§9.2); mọi bộ đọc của egret (preview, `Loop`, các task item) nhận cả hai | bộ đọc của Kestra nhận Ion text: đã đo với `Loop`, preview file và `DeduplicateItems`/`FilterItems` đọc output của `Write` (Task 1). Byte do chính `ionfmt` ghi **chưa** được đưa cho bộ đọc nào của Kestra (chưa đo) |
| `file://`, body webhook multipart, `nsfile://` | bị từ chối | ngoài phạm vi; `nsfile://` và `fileURI` là W9b |

## 3. Bố cục storage và URI

- **Object key:** `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/{flow}/executions/{exec}/{tail}` trong bucket `[storage]`. `workflow.storage.prefix` mặc định là `workflow/`.
- **URI một flow thấy:** `kestra:///{ns-as-path}/{flow}/executions/{exec}/{tail}` — dạng của Kestra. **Nó không mang workspace**: egret ánh xạ một URI sang key bằng cách đặt tiền tố là workspace **của execution đang chạy**, không bao giờ là giá trị đọc từ URI, nên không gì trong một URI có thể chỉ tới workspace khác.
- `tail` là của Kestra: `inputs/{input}/{file}`, `tasks/{taskId}/{taskRunId}/{file}`, `trigger/{triggerId}/{file}`, hoặc một `{file}` trần ngay dưới execution — dạng mà `Loop` của Kestra ghi cho `outputs` ở chế độ STORE, `executions/{iteration}/{n}.ion`, Task 1 đo trên `valids/loop-outputs-auto` (`w9a-storage.json`: `egret_expected.loop_over_ion["valids/loop-outputs-auto"].loop_outputs`, finding `loop_outputs_store_uri`). Slugify và **scheme** tên file output theo đúng Kestra; Task 1 đo cả hai trên Kestra và ghim vào một fixture (`oracle-exec/w9a-storage.json`). Tên output hash đường dẫn tuyệt đối của file trong pod, `/egret/working-dir/<rel>`; thư mục làm việc của Kestra là ngẫu nhiên, nên hash không thể khớp với của Kestra, chỉ scheme khớp.
- **`{exec}` là execution ghi file — phán quyết của người dùng (đầu tài liệu).** egret chạy một vòng lặp như sub-execution (`parent_execution_id` có `loop_index`), và Kestra 2.0 cũng vậy: Task 1 đo được mỗi vòng lặp là một execution loại `LOOP`, và file ghi bên trong một vòng lặp mang id của **chính vòng lặp** dù `execution.id` ở đó render ra id của root (finding `loop_iteration_uri_is_the_iteration` trong `w9a-storage.json`). egret làm giống vậy: không URI nào bị viết lại về loop root. Loop root vẫn được tính, cho tập đọc được (§4.4) và API file (§8). Việc đi lên chỉ theo `parent_execution_id` khi dòng có `loop_index`: một replay do retry cũng đặt `parent_execution_id` (với `loop_index` NULL) và việc đi lên dừng ở đó.
- Một URI bị từ chối nếu scheme của nó không phải `kestra`, nếu nó chứa `..`, hoặc nếu nó không parse được vào bố cục trên.

## 4. Lõi

### 4.1 Streaming trong `egret-storage`

`ObjectStore` giữ các method `Vec<u8>` cho caller hôm nay và có thêm `put_stream(uri, AsyncRead, size_hint)` (một PUT đơn dưới 8 MiB, nếu không thì multipart upload với part 8 MiB, huỷ khi lỗi), `get_stream(uri) -> AsyncRead`, `head(uri) -> Option<size>` và `delete_prefix(uri)`. Test trên MinIO thật với một file lớn hơn một part, đo rằng bộ nhớ resident của process không tăng theo file.

### 4.2 Index

`workflow.execution_files` (migration 0161): `workspace_id`, `execution_id` (execution ghi file — chính vòng lặp với một file ghi bên trong một vòng lặp `Loop`, tức `{exec}` trong URI của nó, §3; FK ghép với `workspace_id`, `ON DELETE CASCADE`), `task_run_id` (nullable, không FK, như `task_logs.task_run_id`), `uri`, `size`, `created_at`; unique `(workspace_id, uri)`. Cùng migration đó thêm `workflow.executions.replayed_from_execution_id` (FK ghép với `workspace_id`, không có hành động `ON DELETE` như `trigger_execution_id` của 0138, không bao giờ trỏ vào chính dòng đó, không bao giờ đi cùng `parent_execution_id`), do route replay thủ công ghi.

**Một file chỉ tồn tại với flow khi dòng index của nó đã tồn tại.** Mỗi lần ghi là: stream vào store, `head` lấy size thật, insert dòng index. `fileExists`, `fileSize` và `isFileEmpty` trả lời từ index; mọi lần đọc tra index trước.

### 4.3 Khi nào một file được commit

File của task được upload trước khi outcome commit, và các dòng index của chúng được insert **trong transaction commit outcome của task run**, sau lease fence của W4. Một worker bị fence hoặc sập chỉ để lại các object chưa được index (§10). Vì tên là tất định theo từng task run, một lần re-claim cùng task run ghi cùng key và ghi đè orphan của chính nó. File input commit cùng dòng execution (§7.1).

### 4.4 Ai được đọc một URI

Một hàm duy nhất, `storage::resolve_readable_many(conn, workspace, readable, uris)`, quyết định; `resolve_readable(workspace, current_execution, uri)` là dạng một phần tử của nó. Các hàm file, staging (§5), cầu plugin (§9.1), FILE input đưa dưới dạng JSON (§7.1) và API file (§8) đều đi tới nó. **Tập đọc được** của một execution, trong workspace của chính nó, là:
1. execution đang chạy `E` và loop root `R` của nó (cách đi lên ở §3);
2. mọi vòng lặp dưới `R`, ở mọi độ sâu (mọi dòng mà cách đi lên ở §3 tới được `R`) — nên một task sau vòng lặp, một vòng lặp anh em và một vòng lặp lồng đọc được file của nhau (phán quyết của người dùng, đầu tài liệu). Mục 1 và 2 là **họ vòng lặp** của `E`;
3. chuỗi replay từ `R`: nguồn của mỗi replay, bắc cầu — `parent_execution_id` của một replay do retry (`loop_index` NULL) hoặc `replayed_from_execution_id` của một replay thủ công; và
4. `trigger_execution_id` của `E`, của `R` và của mọi execution ở 3 (mỗi cái một cấp, quy tắc `trigger` của Kestra; một replay sao chép inputs của nguồn và, trong Kestra, cả trigger của nó).

Kestra OSS cho phép gì ở đây (Task 1): một vòng lặp đọc file của root và một vòng lặp đọc file của chính nó đã được **đo** là đọc được (`file_functions.loop_root_from_iteration`, `file_functions_extra.iteration_own_write`). Một task sau vòng lặp, hay một vòng lặp anh em, đọc file của một vòng lặp thì **chưa đo**; theo mã nguồn, OSS cho đọc qua nhánh namespace (`AbstractFileFunction.java:137-171`, `isAllowedNamespace` luôn true trong OSS), nên mục 2 khớp OSS ở đó. Các vòng lặp của chính chuỗi replay không nằm trong tập.

Một URI có `{exec}` nằm ngoài tập bị từ chối **mà không đọc dòng nào của execution đó**, nên câu trả lời không bao giờ tiết lộ file có tồn tại hay không. Một URI trong tập mà không có dòng nào là *missing* (`fileExists` trả lời gì là phép đo của Task 1). Mọi thứ không đọc được khác là một lỗi nêu URI, không bao giờ là `false` hay rỗng.

**Snapshot (phán quyết của người dùng, phần đầu).** Một lần render chạy trên luồng riêng và không truy vấn được database: trong `settle` connection duy nhất là của transaction, và một pool sẽ không thấy các dòng index mà transaction đó vừa insert cho task vừa xong. Vì vậy trước mỗi lần render có execution, egret gom mọi chuỗi `kestra://` trong source của flow và trong mọi giá trị JSON của render context (inputs, outputs của mọi task đã kết thúc, execution, item của vòng lặp) và phân giải tất cả một lần trên connection đang render. Bốn hàm, staging và cầu plugin trả lời từ snapshot đó. **Một URI được ghép lúc render mà không có trong snapshot là một lỗi nêu URI đó; không có lần tra thứ hai.**

### 4.5 Trần

`workflow.storage.max_file_bytes` (mặc định 1 GiB) mỗi file và `workflow.storage.max_task_bytes` (mặc định 5 GiB) mỗi task run. Vượt một trong hai thì task thất bại, nêu trần đó.

### 4.6 Object store nằm ở đâu

Object store là **chung cho cả process**: `egret-bin` cài một `FileStore` (chính `S3Store` nó đã dựng cho kết quả query, bucket `[storage]`, `workflow.storage.prefix`, các trần) một lần lúc khởi động, và `storage::current()` trả về nó. `read()` chạy bên trong `settle`, thứ được gọi tới từ khoảng tám điểm vào (worker, một lần start, `resume`, scheduler, bộ xử lý trigger Flow, webhook, mọi spawn), không điểm nào giữ handle của store. **Tra cứu index không bao giờ đi qua nó**: chúng chạy trên connection của caller (§4.4). Mọi hàm quyết định điều gì đều nhận store qua tham số; chỉ phần nối dây đọc `current()`. Khi không có store nào được cài, bốn hàm, FILE input và `outputFiles` thất bại nêu `[storage]`, và `container_runner` khác `none` đòi `[storage]` lúc boot.

## 5. Chuyển file vào và ra khỏi pod

Cơ chế chuyển của W5a trở thành streamed: `get_stream` nuôi một streaming tar writer vào `docker cp` / exec `tar -x`; `tar -c` được đọc từng entry và mỗi entry stream sang `put_stream`. Bộ đóng gói trong bộ nhớ bị thay thế, không giữ song song.

**Vào.**
- **Task script:** một giá trị `inputFiles` là URI `kestra://` được staging dưới tên file đó, như Kestra; mọi giá trị khác vẫn là nội dung, như W5a.
- **Task plugin:** egret staging mọi giá trị thuộc tính đã render mà **là nguyên một URI `kestra://`** (một chuỗi lá, không phải chuỗi con: một URI nằm trong câu SQL không phải là một lần đọc file) ở `.egret/storage/<n>`; `task.json` có thêm `storage: {uri → path}`, `baseUri` (tiền tố URI của task run), `maxTaskBytes` và storage context (namespace, flow, execution ghi file, task, task run).
- **Một URI mà snapshot từ chối làm task thất bại lúc render, trước khi có container nào**, nêu URI.

**Ra.**
- **Task script:** `outputFiles` giữ quy tắc của W5a — tên literal và glob không đệ quy (`*`, `?` trong tên file, trong một thư mục); `**`, `[...]` và thư mục wildcard bị từ chối có tên — và `outputDirectory: true` được hỗ trợ; các file khớp được stream tới `tasks/{taskId}/{taskRunId}/{Kestra name}` và `outputs.outputFiles.<relative path>` là URI.
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
- Trong JSON, một FILE input chỉ có thể là một URI `kestra://` đọc được (§4.4) từ execution đang được tạo: các trường hợp `Subflow` và trigger `Flow` (và inputs được sao chép của một replay hay một vòng lặp). Mọi chuỗi khác bị từ chối, nêu input. Việc kiểm chạy ở mọi chỗ tạo execution (start, execution con của `Subflow`, một vòng lặp, một replay), sau khi dòng đã tồn tại và trong transaction của nó.

### 7.2 Các hàm

| hàm | hành vi |
|---|---|
| `read(uri)` | nội dung dạng UTF-8; không phải UTF-8 là lỗi nêu URI (lựa chọn: Kestra thay byte không hợp lệ bằng U+FFFD, Task 1 đo); bị chặn bởi `max_read_bytes` |
| `fileSize(uri)` | từ index |
| `fileExists(uri)` | từ index. Đo trên Kestra (Task 1): một URI đúng dạng của execution đang chạy mà không chỉ tới file nào trả `false`; một URI không parse được, chứa `..`, hoặc có scheme khác là lỗi. egret theo đó: *thiếu* (trong tập đọc được, không có dòng) là `false`; một URI ngoài tập đọc được là lỗi (lựa chọn: Kestra OSS đọc được nó, §2) |
| `isFileEmpty(uri)` | từ index (`size == 0`) |

- Cả bốn trả lời từ snapshot (§4.4). `fileURI` vẫn bị từ chối, giờ nêu W9b; `kv` nêu W9c.
- Chúng tồn tại trong mọi lần render có execution (thuộc tính task, `runIf`, `Log`, outputs); một lần render không có (điều kiện của trigger, publish) từ chối chúng, có tên.
- Render là đồng bộ và đã chạy trên luồng riêng (`render_on_this_thread`, và `bind_on_this_thread` cho `egret.sql`): các hàm trả lời từ snapshot chụp trước khi render; **một URI được ghép lúc render là một lỗi nêu URI đó** (phán quyết của người dùng); `read` block trên runtime handle cho một lần đọc có trần, nên một lần render đọc file cần runtime đa luồng (của production).

## 8. API file của execution

| route | trả lời |
|---|---|
| `GET /api/ws/{ws}/workflows/executions/{id}/file?path=kestra:///…` | các byte thô, streamed |
| `GET …/executions/{id}/file/metas?path=…` | `{size}` từ index |
| `GET …/executions/{id}/file/preview?path=…&maxRows=100` | tối đa `maxRows` (max 5000): `.ion` thành các dòng JSON (đọc bằng `ion-rs` 1.0.1 làm **reader**, từng giá trị cấp cao nhất một, text hay nhị phân — file STORE của Kestra là Ion nhị phân, không có dòng (Task 1); định dạng writer của nó không liên quan ở đây, và `ionfmt::read_ion` từ chối timestamp và blob mà file STORE của plugin chứa), `.csv`/`.jsonl`/`.json`/text thành các dòng, nếu không thì `{previewable: false}`; ngừng đọc khi đã đầy |

- Quyền `workflow:read`; `path` phải là một dòng index mà execution của nó thuộc **họ vòng lặp của `{id}`** — `loop_root({id})` và mọi vòng lặp dưới nó (§4.4 mục 1–2) — trong workspace `{ws}`, nếu không 404, không redirect. Trong hai lựa chọn, chỉ chính `{id}` hoặc cả họ vòng lặp của nó, đây là lựa chọn nhất quán với tập đọc được: mọi task trong họ đọc được mọi file của họ, nên id nào của họ cũng phục vụ chúng (Kestra tới cùng các file đó bằng một 301 sang vòng lặp sở hữu, Task 1). Liên kết trigger và replay đứng ngoài: người xem hỏi các execution đó bằng id của chính chúng. Việc kiểm là `resolve_readable_many` với tập đọc được thu hẹp về họ đó.
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

### 9.2 `egret.sql` với `fetch: STORE`

`Fetch` có thêm `Store` (key riêng của `egret.sql` là `fetch:`). Handler chạy trong egret, nên nó ghi các dòng qua `runners::ionfmt` vào `put_stream` tại `tasks/{taskId}/{taskRunId}/{name}.ion`, và outputs của nó là `{uri, size}` với `size` là số dòng — dạng của output STORE ở `AbstractJdbcQuery` của Kestra (Task 1 ghim nó). Task 1 đo được tên của Kestra là ngẫu nhiên mỗi lần chạy (`{số thập phân không dấu ngẫu nhiên}.ion`), nên `{name}` là `result.ion` cố định (§4.3 cần một lần re-claim ghi đè được object của chính nó). File STORE của Kestra là Ion 1.0 **nhị phân**, mỗi dòng có version marker và bảng symbol riêng, timestamp là chuỗi có annotation (`LocalDateTime::"…"`); `ionfmt` ghi Ion dạng text, nên các byte không thể khớp (`ion_rows` trong fixture ghi giá trị của Kestra cho từng kiểu SQL). **Bộ nhớ:** `QueryExecPort::execute_as_with_params` trả về nguyên một `TabularResult`, nên không gì stream qua port dù connector làm gì (Task 1 ghi lại hành vi bên trong của từng connector); STORE bị chặn bởi `Sql::max_rows` (`MAX_FETCH_ROWS`, 10 000) và thất bại ở trần đúng như `FETCH`. **`egret.sql` không nằm trong registry production nào** (§1): W9a test handler trực tiếp và trên StarRocks sống, và không nối dây nó.

### 9.3 Task storage lõi và `Loop` trên một file

Handler native cho `io.kestra.plugin.core.storage.{Write, Concat, Split, Size, Delete, Reverse, DeduplicateItems, FilterItems, PurgeCurrentExecutionFiles}`, mỗi cái theo thuộc tính và outputs của lớp Kestra tương ứng (Task 1 trích chúng). `Delete` và `PurgeCurrentExecutionFiles` chỉ xoá file thuộc họ vòng lặp của execution đang chạy (loop root của nó và mọi vòng lặp dưới nó, §4.4), xoá dòng index trước; file của trigger hay của nguồn replay đọc được nhưng không bao giờ xoá được (`Delete` của Kestra OSS đã xoá file của một execution khác, Task 1). `PurgeCurrentExecutionFiles` của Kestra chạy bên trong một vòng lặp thì xoá tiền tố nào: chưa đo. `PurgeStorage` ngoài phạm vi (một task quản trị xuyên các execution).

`io.kestra.plugin.core.flow.Loop` (`ForEach` của chương trình) với `values` là một URI `kestra://`: **worker** phân giải URI qua snapshot và đọc file (ION bằng `ion-rs`, từng giá trị cấp cao nhất một, Ion text hay nhị phân — file STORE của plugin là nhị phân, Task 1 — mỗi cái được quy về chuỗi như `FlowableUtils.ionValueToString` của Kestra: chuỗi giữ nguyên, số thành văn bản của nó, `null` là lỗi, còn lại thành JSON) **trước khi** nó gọi `advance()`, và đặt mảng JSON vào `rendered` của run; `advance()` vẫn thuần. **Trong mã không có giới hạn độ rộng vòng lặp nào** để dùng lại, nên file bị chặn bởi `workflow.storage.max_read_bytes`, cùng trần với `read()`. Các flow trong corpus `valids/loop-values-from-uri` và `valids/loop-outputs-auto` là mục tiêu.

## 10. Retention và orphan

- `workflow.storage_retention_days`, mặc định 30; 0 giữ mãi mãi.
- Một pruner mỗi giờ với ngân sách mỗi lượt, theo mẫu của W11, lấy các execution **đã kết thúc** hơn N ngày trước mà còn dòng index, gọi `delete_prefix`, rồi xoá các dòng index. **Một họ vòng lặp được prune cùng lúc, khi loop root của nó kết thúc** (phán quyết của người dùng ngày 2026-10-01 đặt file của một vòng lặp dưới id của chính vòng lặp, §3): một vòng lặp đã kết thúc trong khi root vẫn chạy thì giữ file của nó, để một task sau vòng lặp vẫn đọc được; khi root đã kết thúc hơn N ngày, root và mọi vòng lặp dưới nó được prune cùng nhau. Nếu xoá ở store thất bại, các dòng còn lại và lượt sau thử lại. **Một execution chưa kết thúc không bao giờ bị prune.**
- **Orphan** (đã upload, chưa từng commit): với mỗi execution bị prune và mỗi execution FAILED/KILLED đã kết thúc, liệt kê prefix của nó và xoá các object không có dòng index và cũ hơn 24 giờ.
- Một URI mà file của nó đã bị prune là một lỗi nêu URI và cửa sổ retention.

## 11. Lỗi

Mọi lỗi storage nêu URI và không bao giờ nêu giá trị secret. Một store không với tới được lúc chạy là lỗi hạ tầng: worker trả `Err`, lease hết hạn, run được re-claim, và không tốn `attempts` (W4). Vượt một trần làm task thất bại, nêu trần đó.

## 12. Các task

Mười bốn task, theo thứ tự này; plan (`docs/en/plans/2026-10-01-workflow-w9a-internal-storage.md`) cho từng task file, interface, test và mutation của nó, và lý do của mỗi lần tách từ mười task của bản nháp đầu.

1. **Đo Kestra** (`oracle-exec/w9a-storage.json`): slugify; `IdUtils.from`; tên file output; `outputDirectory`; các hàm file trên URI của chính nó, trigger, không liên quan, missing và sai dạng; FILE input; API file; storage của plugin; output STORE và ION theo từng kiểu; các task storage lõi; `Loop` trên một URI ION; streaming dòng trong connector của egret. Cùng các baseline (test, ratchet, clippy).
2. **`egret-storage` streaming** (`put_stream`, `get_stream`, `head`, `delete_prefix`; một `MemStore` cho test).
3. **Index, URI, `resolve_readable_many`, `FileStore` chung cho process** (migration 0161: `workflow.execution_files`, `executions.replayed_from_execution_id`).
4. **Các hàm file trên snapshot trước render.**
5. **Chuyển streamed vào container; URI trong `inputFiles`.**
6. **Thu ra; `outputFiles` là URI; index lúc commit.**
7. **FILE input.**
8. **API file của execution.**
9. **Cầu storage cho plugin và catalog.**
10. **`egret.sql` STORE.**
11. **`Write`, `Size`, `Delete`, `PurgeCurrentExecutionFiles`; `Loop` trên một file.**
12. **`Concat`, `Split`, `Reverse`, `DeduplicateItems`, `FilterItems`.**
13. **Pruner** (migration 0162: `files_pruned_at`, `files_swept_at`).
14. **End to end trên một cluster kind tạm; coverage; tài liệu; full CI cục bộ.**

## 13. Kiểm thử

- Unit: URI ↔ key, slugify, `IdUtils.from`, `resolve_readable_many` (của chính nó, loop root, vòng lặp và vòng lặp anh em, trigger, replay do retry, chuỗi replay thủ công, execution khác, workspace khác), snapshot (một URI ghép lúc render là lỗi), bảng hàm.
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

- Một flow chỉ đọc file của execution của chính nó, của họ vòng lặp của nó (loop root và mọi vòng lặp dưới nó) và của trigger execution của nó, và một replay đọc file của chuỗi nguồn của nó (§2, §4.4).
- Một file ghi bên trong một vòng lặp `Loop` mang id của chính vòng lặp, như trong Kestra (§3, phán quyết của người dùng); API file phục vụ nó qua bất kỳ id nào của họ vòng lặp, không redirect (§8).
- `allowedFileExtensions` được thực thi; artifact Kestra không thực thi nó (Task 1), nên một flow như vậy chạy được trên Kestra và thất bại trên egret (§2).
- Plugin của Kestra ghi Ion nhị phân; mọi bộ đọc của egret phải nhận nó. STORE của `egret.sql` ghi Ion text, thứ mà bộ đọc của Kestra nhận như đã đo với output của `Write`; byte do chính `ionfmt` ghi chưa được đo trên Kestra (§2, §9.2).
- Một URI được ghép lúc render bằng nối chuỗi là một lỗi; một URI phải có trong source của flow hoặc đến lần render qua inputs hay outputs (§4.4, phán quyết của người dùng).
- Một plugin chỉ đọc các URI là nguyên một giá trị thuộc tính trong thuộc tính đã render của nó (§2, §5).
- `Loop` trên một file đọc nguyên file, bị chặn bởi `max_read_bytes` (§9.3).
- Tên một file output theo scheme của Kestra; hash của nó khác của Kestra (§3).
- File được prune 30 ngày sau khi execution kết thúc theo mặc định — với một vòng lặp `Loop`, sau khi loop root của nó kết thúc (§10); Kestra giữ chúng.
- `outputFiles` đổi từ nội dung sang URI (§6.2).
- STORE của `egret.sql` giữ nguyên kết quả trong bộ nhớ, bị chặn ở 10 000 dòng, vì port trả về nguyên một `TabularResult`; và `egret.sql` không được nối vào registry production nào (§9.2).
- Mọi deployment chạy workflow container cần một bucket S3 hoặc MinIO với tới được.
