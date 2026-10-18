# WorkingDirectory: các task con dùng chung một thư mục làm việc

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-16-workflow-working-directory-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-16; đã xây dựng trên `feat/workflow-working-directory` (chỉ Docker, Kubernetes để sau), với một lần chạy trực tiếp được ghi ở §7 ngày 2026-10-17. Đọc từ source tại `origin/main` ở `8043c4ac`, và từ checkout Kestra `data-portal/kestra` ở `96893b589`. Image đã ghim `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd` là artifact mà oracle đã ghi lại. Mọi sự thật về Kestra bên dưới không đọc từ source đều được đánh dấu **verify on the image** và đo trong task đầu tiên của plan.

**Thay thế:** phán quyết của người dùng ngày 2026-10-05 trong `docs/en/specs/2026-10-05-workflow-w9b-namespace-files-design.md` rằng *"`WorkingDirectory` stays out of W9b"*. Phán quyết đó giữ block ở mức parse và validate, không bao giờ chạy nó. Spec này cho nó chạy. Phán quyết của W9b không thay đổi gì khác.

**Phụ thuộc:**
- W5a (script task, `egret-workflow/src/worker/commands.rs`, các runner copy file vào và ra khỏi container hoặc pod của task).
- W9a (lưu trữ nội bộ, streaming của `egret-storage`, `storage::uri`, pruner). Snapshot ở §5.2 và cache ở §5.4 nằm ở đó.
- W9b (`namespaceFiles` trên script task, staging `FilesService.inputFiles` và `outputFiles`). `WorkingDirectory` dùng lại staging đó thay vì tự viết.
- W8 (cờ `allowFailure:` trên task con, `executor/allow_failure.rs`), quyết định một task con lỗi có dừng block hay không.
- Gói no-silent-stall, gói từ chối một run `WorkingDirectory` đã lưu bằng `NotALeaf` (`worker/mod.rs`) và làm validate từ chối các task con không chạy được.
- Oracle (`crates/egret-workflow/oracle-exec/README.md`) và ratchet của nó (`oracle-exec/baseline.txt`).

## 1. Vì sao

`io.kestra.plugin.core.flow.WorkingDirectory` chạy các task con **lần lượt trong cùng một thư mục làm việc**, nên file mà task con trước ghi ra vẫn còn đó cho task con sau, không cần nối output. Nó cũng đưa `namespaceFiles` và `inputFiles` vào thư mục đó trước khi các task con chạy, thu `outputFiles` sau khi chạy xong, và có thể cache một tập file giữa các lần chạy (`cache:`).

Egret từng lập kế hoạch cho nó như một `Runnable` mờ (`executor/plan.rs`, `executor/mod.rs:3546`: `K::WorkingDirectory(_) => PlannedAs::Opaque`). Từ no-silent-stall, `spec::validate` từ chối mọi `WorkingDirectory` lúc publish (`unsupported_working_directory`, `validate.rs:1759`), nên không flow mới nào chạy được nó. Model chỉ giữ `tasks` và `namespaceFiles` (`models/task.rs:568-600`); `inputFiles`, `outputFiles` và `cache` bị bỏ ngay lúc parse. Plan Task 2 giữ chúng lại lúc parse; Task 3 gỡ phần từ chối.

Corpus có 15 flow nhắc tới `WorkingDirectory`: 13 flow hợp lệ và 2 flow không hợp lệ. Bốn flow hợp lệ trong số đó nằm trong golden (`valids/task-runif-workingdirectory`, `valids/working-directory-invalid-runif`, `valids/working-directory-taskrun`, `valids/working-directory-taskrun-nested`), và không ai trong số đó dùng `inputFiles`, `outputFiles`, `cache` hay `namespaceFiles`. 9 flow hợp lệ còn lại bị artifact từ chối lúc publish, vì nó không biết các test class mà chúng dùng (`corpus.json`):

| Test class mà flow dùng | Flow |
|---|---|
| `io.kestra.plugin.core.flow.WorkingDirectoryTest$LocalFiles` (422 lúc publish) | `working-directory-inputs`, `working-directory-outputs`, `working-directory-cache`, `working-directory-loop`, `working-directory` |
| `io.kestra.core.tasks.test.Read` (422) | `working-directory-namespace-files`, `working-directory-namespace-files-with-namespaces` |
| `io.kestra.core.tasks.test.Encrypted` (422) | `working-directory-secret-output`, `working-directory-taskrun-encrypted` |

Vì vậy **oracle hiện tại không đo được `inputFiles`, `outputFiles`, `cache`, `namespaceFiles`, hay việc chia sẻ thư mục giữa các task con.** §6 nói cái gì thay thế nó.

## 2. Kestra làm gì

Đọc từ `worker/src/main/java/io/kestra/worker/processors/WorkerTaskProcessor.java` (`runWorkingDirectory`, dòng 90-172) và `core/src/main/java/io/kestra/plugin/core/flow/WorkingDirectory.java`.

**Worker** chạy toàn bộ block, và executor thấy mỗi task con như một task run riêng:

1. `preExecuteTasks`: khôi phục archive cache nếu có và chưa hết hạn (§2.3); nạp `namespaceFiles` (`NamespaceFilesUtils.loadNamespaceFiles`); ghi `inputFiles` (`FilesService.inputFiles`). **Nếu bước này ném lỗi, task run của block là `FAILED` và không task con nào chạy.**
2. Với mỗi task con trong `tasks`, theo thứ tự, bỏ qua `disabled: true`:
   - tính `runIf` của nó; nếu falsy thì ghi nhận `SKIPPED` và vòng lặp đi tiếp;
   - nếu không thì chạy bằng `runTask(..., cleanUp=false)` trong **cùng** thư mục làm việc (`forWorkingDirectorySubtask(..., runContext.workingDir())`);
   - nếu trạng thái của task con là `FAILED` **và nó không có `allowFailure: true`**, `break`: các task con còn lại không chạy.
3. `postExecuteTasks`, **vô điều kiện** (sau vòng lặp, bất kể task con làm gì): thu `outputFiles` và ghi thành `outputs.ion` (lộ ra là `outputs.<wdir>.outputFiles`, đã đo, §2.1); rồi lưu archive cache nếu có file khớp đã đổi kể từ lúc khôi phục.
4. Mọi lỗi ở bước 3 làm block lỗi.

Trạng thái cuối của block sau đó do executor tính từ các task con, như với `Sequential` (`resolveNexts` chỉ chạy task kế tiếp khi một task con lỗi, `WorkingDirectory.java`).

### 2.1 Các sự thật mà spec này phụ thuộc, cần verify trên image

Đo trên image đã ghim, 2026-10-16, trên một compose project riêng (`wd-probe`, đã xoá sau đó), với script task thật trên Docker runner (`alpine:3.20`). Các flow probe và JSON của từng lần chạy nằm trong scratchpad của phiên, không nằm trong repo.

- **V1 xác nhận.** `wd-fail`: task con lỗi mà không có `allowFailure` (`exit 1`) làm block `FAILED`, và task con đứng sau nó **không có task run nào** cả.
- **`allowFailure` trên task con, đã đo.** `wd-allowfail`: cùng task con lỗi đó với `allowFailure: true` cho `WARNING`, task con sau nó chạy và `SUCCESS`, và block là `WARNING`. `allowFailure` của task con cho vòng lặp đi tiếp và đánh dấu block `WARNING`.
- **`runIf` false, đã đo.** `wd-runif`: task con có `runIf: "{{ false }}"` **vẫn** có một task run, `SKIPPED` với một attempt. Nó không vắng mặt. Task con sau nó chạy và đọc file mà task con đầu ghi.
- **Chia sẻ, đã đo.** `wd-files`: task con 1 đọc một entry `inputFiles` và ghi `a.txt`; task con 2 đọc `a.txt` (`SECOND_READ_A=hello`). Thư mục được chia sẻ giữa các task con, như §1 nói.
- **V2 xác nhận.** `wd-finish`: task con 1 ghi `out.txt` rồi lỗi. Block là `FAILED`, và một task `errors:` vẫn đọc được `outputs.wdir.outputFiles` chứa `out.txt`. Vậy `outputFiles` được thu sau khi một task con lỗi.
- **V3 xác nhận.** `wd-prepare-fail`: một URI `inputFiles` không tồn tại làm block `FAILED` với `attempts = 0` và không có task con nào chạy (`Failed preExecuteTasks on WorkingDirectory: File not found for URI …`).
- **Cache, đã đo.** `wd-cache` chạy hai lần: lần thực thi đầu ghi `written`, lần thứ hai `restored` (archive được đọc lại).
- **Khoá output, đã đo.** `outputs.wdir.outputFiles` là một map từ mỗi kết quả khớp glob của `outputFiles` sang một URI `kestra:///…/tasks/wdir/{run id}/…-out.txt`. Khoá **không** phải `uris`: `outputs.wdir.uris` không tồn tại trên image này, và ví dụ Java trong `WorkingDirectory.java` nói `uris` là lỗi thời.
- **V4 chưa đo.** Output của một task con là của chính nó là điều chuẩn cho mọi task; plan không dành probe cho nó.

### 2.2 `outputFiles`, `inputFiles`, `namespaceFiles`

- `inputFiles` và `namespaceFiles` được nạp **trước** task con đầu tiên, theo thứ tự này: namespace file trước, rồi `inputFiles`, nên `inputFiles` ghi đè một namespace file cùng tên (phán quyết của W9b, vẫn đúng ở đây).
- `outputFiles` là một danh sách glob, khớp không đệ quy với thư mục sau task con cuối, như `outputFiles` của script task (`worker/handlers/script.rs:68`). File thiếu thì bị bỏ qua.

### 2.3 `cache`

- `cache.patterns` là glob trên thư mục làm việc; `cache.ttl` là tuỳ chọn (vắng mặt nghĩa là archive không bao giờ hết hạn).
- Khoá lưu trữ của archive là `(namespace, flow, cache id = id của block, object id = giá trị của task run)`, băm cho object id (`StorageContext.getCacheURI`, `StorageContext.java:204`).
- Khôi phục diễn ra trước các task con; archive được ghi sau chúng **chỉ khi** có file khớp nào đó bị sửa sau lúc khôi phục. Vì vậy lần chạy đầu (chưa có archive) lưu bất cứ thứ gì khớp.
- Archive đã hết hạn (lần sửa cuối cũ hơn `ttl`) bị xoá khi đọc và coi như không có (`InternalStorage.getCacheFile`).

## 3. Egret hiện làm gì

- `models::WorkingDirectory` giữ `tasks`, `namespace_files`, và (từ plan Task 2) `input_files`, `output_files` và `cache`, giữ nguyên lúc parse (`models/task.rs`). Publish từ chối mọi `WorkingDirectory` bằng `unsupported_working_directory` (`validate.rs:1759`), nên flow có nó không được lưu và không chạy được; chính phần từ chối đó là lý do không run nào tới được claim. Trước Task 2, ba property này bị bỏ lúc parse, và điều đó chỉ bị che bởi phần từ chối ấy.
- `spec::validate` từ chối task con không chạy được (`working_directory_child_not_runnable`) và block rỗng (`working_directory_tasks_required`), và validate `namespaceFiles` lúc publish (`spec/validate.rs:1663`).
- `executor::plan` lập kế hoạch block như một `Runnable` mờ, nên executor không bao giờ điều khiển các task con của nó, và claim của worker trả `None` cho nó (`worker/claim.rs`; `NotALeaf` trong `worker/mod.rs`).
- Các leaf của egret mỗi cái chạy trong runner **riêng** của nó, và thư mục làm việc của leaf là `/egret/working-dir` bên trong một container hoặc pod được tạo cho task đó (`worker/commands.rs:133`, `WORKING_DIR`). Không có gì mang thư mục từ leaf này sang leaf kế tiếp.
- File vốn đã đi **vào và ra khỏi** container của leaf: `docker cp` cho input (`worker/commands.rs`, staging input), và helper `files-out` cho output trong pod Kubernetes (`worker/commands.rs:321`). §5.2 dùng lại các đường đó.

Vậy khác biệt trung tâm với Kestra không phải là thiếu một tính năng mà là một **ranh giới runtime**: các task con của Kestra chia sẻ thư mục vì một tiến trình worker chạy hết chúng trong một thư mục tạm; các task con của egret chạy trong các container hoặc pod riêng, mỗi cái có thư mục mới.

## 4. Lựa chọn thiết kế

### 4.1 Ba cách chia sẻ một thư mục

| Phương án | Cách làm | Kết luận |
|---|---|---|
| A. Chạy cả block trong một runner | một claim chạy mọi task con trong một container hoặc pod | **Bị loại.** Các task con có image, runner, timeout và retry khác nhau; một pod không chạy được một bước `python` và một bước `shell` với image khác nhau, và retry của một task con sẽ chạy lại các anh em của nó. |
| B. Volume dùng chung | một Docker named volume, hoặc PVC `ReadWriteMany` cho Kubernetes, gắn vào mọi task con | **Không phải mặc định.** Nó cần hạ tầng mà cluster kind và triển khai mặc định không có, và một claim RWX là lựa chọn riêng cho từng cluster. Nó vẫn là một tối ưu có thể làm sau, đứng sau một setting của runner. |
| **C. Snapshot giữa các task con** | sau mỗi task con, thư mục được copy ra khỏi container hoặc pod của nó vào lưu trữ nội bộ; trước task con kế tiếp, nó được copy vào | **Được chọn.** Chạy được trên mọi runner egret có, tái dùng đường copy của §3, và giữ mỗi task con là một leaf bình thường. Tốn một lần copy ra và một lần copy vào cho mỗi task con, bị chặn bởi `max_task_bytes`. |

### 4.2 Executor điều khiển block

Block trở thành một flowable, `TaskKind::WorkingDirectory { tasks }`, được executor điều khiển như `Sequential` (như `AllowFailure` được điều khiển, `2026-10-16-workflow-allow-failure-flowable-design.md` §5), chứ không do một worker chạy. Mỗi task con được claim như một leaf thông thường, và executor đưa cho nó tham chiếu snapshot của block. Cách này giữ mỗi task con là một task run với id, timeout, retry và log riêng, và giữ executor là bên duy nhất ghi trạng thái run (task con của Kestra cũng là task run, nên hình dạng trace khớp).

Hệ quả: việc chuẩn bị của block (`namespaceFiles`, `inputFiles`, khôi phục cache) và việc kết thúc của nó (`outputFiles`, lưu cache) là **các claim riêng** của block, nên bản thân block có thêm công việc tương đương hai leaf. Plan quyết định chúng là một claim của block hay một bước bên trong task con đầu và cuối; §5.3 chọn cái đầu.

### 4.3 Snapshot thuộc về mỗi lần chạy của block

Một snapshot là một archive của thư mục tại một thời điểm trong lần chạy của block, được lưu trong bucket của W9a theo `{workflow.storage.prefix}{workspace}/executions/{exec}/_wd/{block run id}/prepare.tar` và `.../_wd/{block run id}/{child run}.tar`, và row của block trỏ tới object hiện tại (§5.2). Nó được ghi bởi task con vừa xong và đọc bởi task con sắp chạy. Nó bị xoá khi block tới trạng thái kết thúc; một lần crash để lại object, và `_wd` sweep là lớp chặn cuối (§7, open point 2).

*Đính chính, 2026-10-19 (gói nợ, PR-1):* đoạn này ban đầu đặt tên object là `{seq}.tar` (khoá ở §5.2 mới là khoá đã xây: `prepare.tar` và `{child run}.tar`) và gọi pruner của W9a là lớp chặn cuối. Pruner không với tới `_wd`; `_wd` sweep (migration 0196, `storage::wd_sweep`) thì có.

### 4.4 Các quyết định được đưa ra khi triển khai (2026-10-16)

Ghi lại khi snapshot, các claim của block và đường copy Docker được xây dựng (plan Task 6). Mỗi mục là một lựa chọn mà spec để ngỏ, hoặc một thay đổi so với §5 như đã viết.

- **Stream, không đóng gói.** Thư mục nằm trong container, nên worker không bao giờ giữ nó. Copy ra là `docker cp c:<wd>/. -` đọc vào `snapshot::admit`, `admit` kiểm tra từng entry (symbolic và hard link, device, `..`, tên tuyệt đối đều bị từ chối; giới hạn được kiểm tra theo kích thước của từng entry trước khi byte của nó được copy) và ghi archive đã được chấp nhận thẳng vào `ObjectStore::put_stream` qua một pipe 256 KiB trong bộ nhớ. Copy vào là `get_stream` vào `admit` vào `docker cp - c:<wd>`. Không archive nào được giữ nguyên khối, và không file tạm nào được ghi trên worker. `pack` và `unpack` trên một `Vec` đã bị bỏ. Mode bit được giữ (`TarEntry::mode`), nên file thực thi vẫn thực thi được.
- **Claim của block là các row không có `type_id`.** Claim chuẩn bị và claim kết thúc là các row `task_runs` ở `RUNNING` có `working_dir` là `PREPARING` hoặc `FINISHING`. `claim` lấy chúng dưới cùng lease như một leaf, và `run_block_claim` hoàn tất chúng qua cùng fence và `settle`, với `CompleteWorkingDirectoryClaim`. Một claim lỗi đặt `SetRunState { Failed }` lên block và ghi lý do vào `message`, nên block kết thúc `FAILED` trước mọi task con (V3). Một claim không bao giờ để lại việc dở dang: mọi lỗi trong công việc của claim đều thành `BlockDone::Failed`.
- **Con trỏ là một cột.** Migration 0194 thêm `task_runs.working_dir_snapshot`, URI object của snapshot hiện tại của block. Các object là `{prefix}{ws}/executions/{exec}/_wd/{block run}/prepare.tar` (được claim chuẩn bị ghi, một archive rỗng) và `.../{child run}.tar` (bản copy ra của một task con). Khoá được đặt tên theo run tạo ra nó, không theo `{seq}.tar`: lần retry của một run ghi đè object của chính nó, và không cần lưu sequence nào. Object của một task con trở thành con trỏ khi commit của task con thấy nó qua `head`; một lần copy ra bị từ chối không để lại object và không di chuyển con trỏ. Object bị xoá trước khi task con chạy, nên một object của lần thử trước không thể đóng giả là của lần này.
- **Task con không có con trỏ là dữ liệu hỏng.** `working_dir::carry_of` từ chối một task con mà block của nó không có snapshot, và gọi tên lỗi (`corrupt: ...`); task con lỗi mà không có run của riêng nó. Nó không bao giờ được chạy trên thư mục rỗng (§5.2).
- **Claim kết thúc xoá các snapshot của block sau khi commit**, bằng một `delete_prefix`. Lỗi khi xoá chỉ là cảnh báo; pruner quét phần còn lại (open point 2 bên dưới vẫn mở). *Đính chính, 2026-10-19 (gói nợ, PR-1):* pruner không với tới `_wd`; `_wd` sweep thì có (migration 0196). Một lần xoá lỗi để `working_dir_swept_at` là NULL, và sweep xoá object rồi đóng dấu cột; một lần xoá thành công đóng dấu ngay.
- **Chỉ Docker, và từ chối ở nơi khác.** Một task con mà runner không phải Docker lỗi kèm tên trước khi gate chạy (`script.rs`). Đây là quy tắc không im lặng bỏ (§8): một task con Kubernetes nếu không sẽ chạy trên thư mục rỗng và thành công.
- **Snapshot quá giới hạn hoặc vấn đề về link làm task con lỗi.** `TooLarge` và `Refused` từ một lần copy ra làm task con lỗi với vấn đề là lý do của nó và xoá object. Block khi đó dừng (§5.3). Output của một claim kết thúc sau một task con như vậy được đọc từ snapshot đã được chấp nhận gần nhất (Task 7; chưa được xây dựng).
- **`namespaceFiles` chạy trong claim chuẩn bị (Task 7).** Claim chuẩn bị render `namespaceFiles` của block với render context của chính lần chạy block (`claim::render_context`, cùng context mà một leaf của cùng flow render, nên không có renderer thứ hai), stage nó bằng `stage` của W9b cho namespace của chính flow, và ghi thư mục thành snapshot đầu tiên bằng `write_tar` trên các input đã lưu, stream qua một pipe. Lỗi render, staging hoặc giới hạn kích thước làm block lỗi trước mọi task con (V3). Mã `working_directory_property_not_yet_run` vẫn nằm trong `REFUSED_BY_NAME` trong khi `inputFiles`, `outputFiles` và `cache` còn bị nó từ chối. Được ghi trong `property-treatment.json` là `honoured`.
- **`inputFiles` chạy trong claim chuẩn bị, sau `namespaceFiles` (Task 7).** `inputFiles` của block được render với cùng context và được tách như `inputFiles` của script task (nội dung inline; file `kestra://` được tra qua render scope; URI namespace file được giải qua `resolve_ns_file`). `merge` cho phép mọi tên input thắng một namespace file cùng tên, và `write_archive` ghi nội dung cùng các input đã lưu thành một archive. `inputFiles` của chính task con không nằm trong claim này: task con script stage chúng sau khi copy snapshot vào, như vẫn làm (`run_created`), nên hành vi ở cấp task con là của script task, không phải một đường mới.
- **`outputFiles` do claim kết thúc thu, và được kiểm tra sau mỗi task con (Task 7).** Claim kết thúc đọc con trỏ của block (snapshot cuối) và thu mọi file mà một tên khai báo khớp (một tên literal, hoặc một mẫu `*`/`?` trong tên file, như `output_problem` chấp nhận) qua `collect_one` dưới tiền tố task của chính run block. `outputs` của nó là `{"outputFiles": {name: kestra:// URI}}`, được ghi bằng cập nhật row có fence của block, và các file của nó được index dưới run block. Một tên khai báo là thư mục hoặc không phải file thường làm finish lỗi, kèm tên (test của plan Task 7 Step 1 là một block khai báo `sub`). Bản copy ra của mỗi task con được kiểm tra với cùng tên trước khi kết quả của nó được ghi (`check_child_outputs`), nên một task con để lại thư mục dưới một tên khai báo sẽ lỗi kèm tên và block dừng (spec §5.3). Kiểm tra chạy bất kể exit code, nên task con lỗi cũng được kiểm tra (V2). Được ghi là đã báo cáo, không phải đã đo: §5.5 của spec nói "output được ghi lúc claim kết thúc", và cách này giữ điều đó, cộng thêm kiểm tra theo task con mà quy tắc "task con lỗi kèm tên" của plan đòi hỏi.
- **`cache` được giữ bởi claim chuẩn bị và claim kết thúc (Task 7).** Khoá theo hình dạng §5.4 của spec với hai thay đổi đã nêu: object là `.tar` (archive của egret; `.zip` trong spec là của Kestra), và id là id của task cộng với id của flow, không phải của execution, nên các run của một flow dùng chung nó. Giá trị task run được băm bằng SHA-256 (chuỗi rỗng khi không có). Archive bắt đầu bằng một entry đánh dấu chứa thời điểm lưu tính bằng nano giây, và TTL được xét theo đó, nên TTL không phụ thuộc vào việc store có báo thời gian sửa đổi hay không (store trong bộ nhớ không báo). Cache cũ hơn `ttl` (hoặc không có đánh dấu) bị xoá khi đọc và coi như không có; `ttl` vắng mặt thì không bao giờ hết hạn. Khôi phục ghi các file của cache, rồi `namespaceFiles` và `inputFiles` đè lên. Lưu diễn ra ở claim kết thúc: nó lưu khi cache không có hoặc đã hết hạn, hoặc khi các file khớp của snapshot cuối khác với archive theo tên hoặc nội dung (một file khớp đã biến mất được tính là thay đổi). Cache không đổi thì không bị ghi lại. File khớp bị chặn bởi `max_task_bytes`; kiểm tra giới hạn có trong code nhưng không được test ở đây. Hai đơn giản hoá, đều được ghi lại: cache chỉ giữ file thường (thư mục rỗng không được giữ), và mẫu được khớp với đường dẫn tương đối với thư mục làm việc.
- **Tên entry thư mục trong archive có dấu `/` ở cuối.** Tên khai báo được khớp với tên entry đã bỏ dấu đó, nên `sub` khớp entry thư mục `sub/`.

## 5. Thiết kế

### 5.1 Phạm vi

- Model: `WorkingDirectory` giữ `tasks`, `namespaceFiles`, `inputFiles`, `outputFiles` và `cache`, mỗi cái ở kiểu của Kestra. Không gì bị bỏ lúc parse.
- Lập kế hoạch: `TaskKind::WorkingDirectory { tasks }`, được điều khiển bởi `executor/flowable/working_directory.rs`, một nhánh dispatch trong `executor/flowable/mod.rs`.
- Các claim riêng của block: claim **chuẩn bị** (khôi phục cache, `namespaceFiles`, `inputFiles`, và ghi một snapshot rỗng) và claim **kết thúc** (`outputFiles`, lưu cache, xoá snapshot). Mỗi cái tái dùng các hàm W9a và W9b mà một script task đã dùng (`FilesService.inputFiles`, bộ thu `outputFiles`).
- Mỗi task con: một leaf bình thường mà claim mang snapshot của block. Trước khi handler chạy, worker copy snapshot vào `/egret/working-dir` của task con; sau handler, nó copy thư mục ra lại thành snapshot kế tiếp.
- Template: `{{ workingDir }}` trong một task con là `/egret/working-dir`, như trong mọi script task.

### 5.2 Copy snapshot

- Ra: worker chạy cùng helper archive mà `outputFiles` của script task đã dùng, trên toàn thư mục, với giới hạn kích thước W9a `max_task_bytes`; vượt giới hạn thì task con **lỗi** và block dừng, kèm tên giới hạn. Cắt cụt âm thầm là không được phép.
- Vào: snapshot được stream vào container hoặc pod của task con kế tiếp bằng cùng cách copy mà staging input dùng. Thiếu snapshot cho một block đã chạy một task con là dữ liệu hỏng (`Corrupt`), không phải thư mục rỗng.
- Task con bị bỏ qua (`runIf` false, hoặc `disabled`) không copy gì; task con kế tiếp đọc cùng snapshot đó.

### 5.3 Task con lỗi

- Task con lỗi mà không có `allowFailure` dừng các task con còn lại của block (V1). Snapshot của nó vẫn được lấy, và claim kết thúc vẫn chạy `outputFiles` và lưu cache (V2), nên output và cache của block phản ánh công việc đã làm.
- Task con có `allowFailure: true` mà lỗi không dừng block; task con kế tiếp đọc snapshot được lấy sau nó.
- Claim chuẩn bị lỗi làm block lỗi mà không có task con nào chạy (V3).

### 5.4 Cache

- Khôi phục là một phần của claim chuẩn bị: archive cho `(namespace, flow, block id, giá trị task run)` được tra theo quy tắc TTL của §2.3 và được ghi vào thư mục trước task con đầu tiên.
- Lưu là một phần của claim kết thúc, với cùng quy tắc "đã đổi kể từ lúc khôi phục", dùng trạng thái thư mục của snapshot.
- Archive dùng cùng bucket W9a và hình dạng khoá của nó, `{prefix}{ws}/{ns-as-path}/_cache/{flow}/{block id}/{hash of value}.zip` (đoạn namespace là của W9b). Một lần ghi cache bị chặn bởi `max_task_bytes`, và một lần ghi cache quá giới hạn **làm claim kết thúc lỗi**, như một snapshot quá giới hạn.

### 5.5 Output

- `outputs` của block mang `outputFiles`, một map từ mỗi kết quả khớp của `outputFiles` sang URI `kestra://` của nó, như image làm (`outputs.wdir.outputFiles['out.txt']`, §2.1). Map được ghi ở claim kết thúc và được đọc như mọi output task khác.

### 5.6 Không thay đổi gì

- Cờ `allowFailure:` (W8) và ngữ nghĩa `allowFailure` của một leaf.
- Các handler của script: một task con vẫn là leaf không đổi; nó chỉ thấy một thư mục bắt đầu khác.
- Task con là flowable: validate tiếp tục từ chối chúng (`working_directory_child_not_runnable`). `Sleep` và `PurgeFlows` của Kestra vẫn được chấp nhận, như hiện nay.
- Plugin storage và cầu KV: một task con plugin đọc và ghi thư mục như khi nó là leaf. Không có gì mới được bắc cầu.

## 6. Nghiệm thu

Oracle hiện tại không thể là bài kiểm tra nghiệm thu cho phần lớn nội dung này (§1), nên plan thêm các flow còn thiếu, và đây là tiêu chuẩn.

1. **Các flow corpus mới chỉ dùng type thật**, không dùng test class: ít nhất một flow cho mỗi thứ `inputFiles`, `outputFiles`, `cache` (được khôi phục ở lần chạy thứ hai), một task con lỗi có và không có `allowFailure`, một task con có `runIf` false, `namespaceFiles`, và một task con chia sẻ file với task con kế tiếp (lý do block tồn tại). Mỗi cái được đo trên image đã ghim qua harvest của oracle, nên entry golden của nó là trace của Kestra, không phải output của egret. Golden không bao giờ được sửa tay.
2. **Bốn WorkingDirectory valid đang chạy vẫn khớp**, và các dòng baseline của chúng chỉ rời `baseline.txt` khi trace khớp.
3. **Mỗi dòng của §5.3 có một test executor**: task con lỗi làm block dừng; task con `allowFailure` tiếp tục; task con bị bỏ qua không copy gì; claim chuẩn bị lỗi làm block lỗi mà không có task con nào chạy; claim kết thúc lỗi làm block lỗi.
4. **Mutation**: bỏ copy vào của snapshot (task con thứ hai thấy thư mục rỗng) phải làm một test lỗi; bỏ copy ra phải làm một test lỗi; bỏ claim kết thúc khi task con lỗi phải làm một test lỗi (V2).
5. **Kích thước**: snapshot vượt `max_task_bytes` làm task con lỗi và kèm tên giới hạn (test với giới hạn nhỏ).
6. **Gate**: `cargo fmt --check`, `cargo clippy -p egret-workflow --all-targets -- -D warnings`, `cargo test -p egret-workflow`, và lần chạy oracle trong `oracle-exec/README.md`, trên một database riêng. Một lần chạy trực tiếp một flow có snapshot thật qua runner Docker được ghi lại, vì đường copy phụ thuộc runner.
7. **Ratchet chỉ di chuyển** theo các entry được banking; không entry nào được thêm vào baseline.

## 7. Câu hỏi còn mở

1. **Đường copy của Kubernetes.** §3 nói có helper `files-out` cho output. Copy một thư mục **vào** một pod, để copy snapshot vào, chưa được xây dựng; plan kiểm tra những gì runner k8s có và đo nó trên cluster kind trước khi §5.2 dựa vào nó.
2. **Rác do crash.** Một snapshot bị bỏ lại bởi một block đã crash được pruner xoá theo retention (`workflow.storage_retention_days`). Plan xác nhận pruner tới được prefix `_wd`, hoặc thêm một lần quét giống staging sweep của W9b.

   *Đã giải quyết, 2026-10-19 (gói nợ, PR-1):* pruner không với tới `_wd`, nên đã thêm một sweep: migration 0196 thêm `task_runs.working_dir_swept_at` (kèm CHECK rằng nó chỉ được đặt trên run của block) và một partial index, còn `storage::wd_sweep` chạy mỗi giờ trên mọi replica có store, sau một advisory lock riêng. Nó lấy các run của block có `working_dir IS NOT NULL AND working_dir_swept_at IS NULL` mà execution đã terminal và `COALESCE(ended_at, updated_at)` cũ hơn 24 giờ (100 mỗi batch), gọi `delete_prefix` trên prefix `_wd` của block rồi đóng dấu cột. Nó chạy bất kể `storage_retention_days` ghi gì: đây là object tạm. Câu chọn không bao giờ đọc state của run, nên nó cũng quét snapshot của block mà force-close của kill đã đóng thành `KILLED` (spec nợ §2.2); test `the_wd_sweep_removes_a_force_closed_blocks_snapshots` khoá điều đó. Các test khác: `swept_at_is_only_for_a_block_run`, `the_wd_sweep_removes_a_crashed_blocks_snapshots_once`, `a_normal_finish_stamps_swept_at` và `a_failed_delete_leaves_swept_at_null_for_the_sweep`.
3. **Chi phí snapshot (đo trên Kestra, là baseline).** Một file 100 MB do task con 1 ghi và được task con 2 và 3 đọc mất Kestra 13,6 s cho bên ghi, 43,9 s và 57,4 s cho hai bên đọc (`wd-cost`), so với khoảng 1 s mỗi task con cho file 1 KB (`wd-cost-baseline`). Vậy việc copy vào và ra khỏi container của chính Kestra đã tốn hàng chục giây ở kích thước này. Số của egret, từ cùng probe trên Docker, được đo trong plan Task 6 Step 4 và phải ít nhất tương đương trước khi phương án C được chấp nhận.

**Số của egret (đo 2026-10-16, Docker trên macOS, `working_dir_docker_live.rs`, mỗi cái một lần chạy).** Một task con ghi file 1 MB mất khoảng 1,5 s (bên ghi) và 1,3 s (bên đọc). Một task con ghi 100 MB mất khoảng 4,3 s (bên ghi) và 7,3 s (bên đọc). Con số là khoảng thời gian của mỗi task con từ `created_at` đến `updated_at` cuối cùng, nên gồm cả việc tạo và khởi động container. Chi phí copy là hiệu với lần chạy 1 MB: khoảng 2,8 s ra cho 100 MB, khoảng 6 s vào. Con số đó thấp hơn nhiều so với 13,6 s, 43,9 s và 57,4 s của Kestra, vốn đo cùng số byte trên một máy khác và một harness khác, nên nó là dấu hiệu chứ không phải chứng minh ngang bằng. Phương án C vẫn được chấp nhận trên bằng chứng này. Chi phí của egret cho mỗi task con là một lần copy ra và một lần copy vào, như §4.1 nói.
4. **`taskRun.value` trong một loop.** Một block nằm trong `Loop` nhận giá trị của vòng lặp làm object id của cache (§2.3). Plan xác nhận giá trị đó tới được claim của block.
5. **Trace của Kestra cho một task con bị bỏ qua** được đo (§2.1): task con có `runIf` false có một task run ở trạng thái `SKIPPED` với một attempt. Trace của egret phải khớp hình dạng đó, và plan kiểm tra điều này với entry golden khi entry đó tồn tại.

**Ghi chú chạy trực tiếp, 2026-10-17 (WorkingDirectory Task 9, §6 mục 6).** Flow corpus `working-directory-real-shared-file` (`writer` chạy `echo shared-content > shared.txt` và `echo WRITER_DONE`; `reader` chạy `echo READER_SAW=$(cat shared.txt)`, đều trên `alpine:3.20` qua runner Docker) được chạy một lần qua đường thực thi thật: `start_execution`, rồi `tick_with` tới khi block kết thúc, với copy ra và copy vào thật của runner Docker và bộ ghi log của worker được gắn vào. Object store là store trong bộ nhớ của tiến trình này (`install_mem_for_tests`), trên một database Postgres riêng (`egret_wd_test`, cổng 5449). Nó không đụng dev stack dùng chung, MinIO hay RustFS dùng chung, hay cluster kind. Đọc lại từ database: block `wdir` `SUCCESS`, `writer` `SUCCESS`, `reader` `SUCCESS`; `writer` ghi log `WRITER_DONE`, và `reader` ghi log `READER_SAW=shared-content`. Vậy reader đã đọc file mà writer tạo ra, qua snapshot. Probe là một test tạm, không được commit. Hình dạng là cùng hình dạng mà `tests/working_dir_docker_live.rs` khẳng định. Lần chạy này không đo lại chi phí copy, và không bao phủ Kubernetes.

## 8. Quy tắc mà gói này phải đáp ứng

- Oracle là bài kiểm tra nghiệm thu. Một trace khác với image không phải là "gần đủ"; sự khác biệt đi vào `baseline.txt` kèm nguyên nhân, hoặc code phải đổi.
- Không sửa `oracle-exec/golden.json` bằng tay. Các entry golden mới đến từ harvest của các flow mới.
- Tài liệu là song ngữ: spec này và plan có một bản tham khảo tiếng Việt trong `docs/vi/`, viết sau khi bản tiếng Anh đã chốt (quy ước `docs-bilingual-convention`).
- Không im lặng bỏ: một property egret không chạy được thì bị từ chối lúc publish, hoặc lỗi lúc chạy, không bao giờ bị bỏ qua.

## 9. Không xây ở đây

- Plugin ghi vào namespace file và cầu KV cho plugin (các mục "still not true" còn lại của chương trình, không đổi).
- Che giá trị KV trong log.
- Volume dùng chung (§4.1, phương án B).
- Task con là flowable của `WorkingDirectory`.
