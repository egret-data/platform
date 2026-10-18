# WorkingDirectory: plan triển khai

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/plans/2026-10-16-workflow-working-directory.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Spec:** `docs/en/specs/2026-10-16-workflow-working-directory-design.md`. Đọc spec trước; plan này tham chiếu các mục của nó (§2 Kestra, §3 egret hiện tại, §4 lựa chọn thiết kế, §5 thiết kế, §6 nghiệm thu, §7 câu hỏi còn mở).

**Mục tiêu:** `WorkingDirectory` chạy các task con trong một thư mục làm việc chung, với `namespaceFiles`, `inputFiles`, `outputFiles` và `cache`, như Kestra làm; mỗi hành vi được đo trên image đã ghim trước khi được viết thành code.

**Nhánh:** `docs/workflow-working-directory-spec` mang spec (PR #682). Phần triển khai đi trên `feat/workflow-working-directory`, tách từ `origin/main` sau khi spec được merge. Một worktree mới, không phải worktree đang chứa việc khác.

**Quy tắc áp dụng:** oracle là bài kiểm tra nghiệm thu (spec §8). Không bao giờ sửa `golden.json` bằng tay. Một cargo hoặc một test suite tại một thời điểm. Không bao giờ dùng dev stack dùng chung (`:5432`, `deploy/docker-compose.dev.yml`, bucket dùng chung, cluster kind dùng chung "egret"). Các test có DB và mọi lần chạy trực tiếp dùng một database riêng và một stack riêng, với tên không phải "egret". Commit theo đường dẫn cụ thể; không bao giờ `git add -A`, không bao giờ `git stash` trần.

## Task 0: quyết định các câu hỏi mở (spec §7)

Ba câu hỏi là của người dùng trả lời trước Task 4, vì mỗi câu đổi chi phí hoặc phạm vi:

- [x] **Phương án C hay phương án B (spec §4.1).** Plan này triển khai C, snapshot. B (volume dùng chung) đứng ngoài trừ khi Task 1 Step 4 cho thấy C quá chậm.
- [ ] **Copy vào Kubernetes (spec §7.1).** Nếu runner k8s không copy được một thư mục vào pod, snapshot ban đầu chỉ có Docker và đường Kubernetes là một task riêng. Task 1 Step 5 đo nó. *(Hoãn: Kubernetes. Ghi ngày 2026-10-19: chưa bắt đầu; đường Docker đã ra và WorkingDirectory bị từ chối trên các runner khác, spec §3 và §7.1.)*
- [ ] **Giới hạn kích thước của một snapshot.** Mặc định: `max_task_bytes` (5 GiB, W9a). Task 1 Step 4 có thể đề nghị một mặc định thấp hơn cho snapshot. *(Hoãn, ghi ngày 2026-10-19: chưa có câu trả lời nào được viết vào spec §7; code dùng mặc định `max_task_bytes`.)*

**Xong khi:** mỗi câu hỏi có câu trả lời được ghi vào spec §7, hoặc giữ mặc định của plan và câu trả lời nói như vậy.

## Task 1: đo trên image (spec §2.1, §5.2, §7)

Chưa viết code gì cho tới khi xong việc này. Corpus không làm được (spec §1): các flow WorkingDirectory của nó dùng test class mà artifact từ chối. Vậy phép đo dùng **flow mới với type thật**, chạy theo cách oracle chạy một flow.

- [x] **Step 1: viết các flow probe** (xong 2026-10-16, trong scratchpad của phiên, không trong repo; chúng chuyển vào `corpus/` chỉ ở Task 8). Chỉ type thật: `io.kestra.plugin.scripts.shell.Commands` trên runner **Docker** (`io.kestra.plugin.scripts.runner.docker.Docker`, `alpine:3.20`). Không dùng runner Process: egret từ chối `io.kestra.plugin.core.runner.Process` lúc save (`scripts/mod.rs`), nên probe trên nó sẽ đo một flow egret không chạy được. Các flow: (a) `inputFiles` và `outputFiles`; (b) `cache` có `ttl`, chạy hai lần; (c) một task con lỗi mà không có `allowFailure`, theo sau bởi một anh em; (d) cùng với `allowFailure: true`; (e) một task con có `runIf` false giữa hai task con; (f) một block `WorkingDirectory` có `namespaceFiles`; (g) một file do task con 1 ghi và task con 2 đọc.
- [x] **Step 2: chạy từng probe** (xong: compose project riêng `wd-probe`, đã xoá sau đó; image và compose file là của oracle) trên image đã ghim (`kestra/kestra@sha256:28f59bf175e1…`) trong một compose project tên `wd-probe`, với các setting `docker-compose.yml` của oracle, dùng cùng chuẩn hoá như `normalise.py`. Ghi mọi task run, trạng thái và parent id của nó, và các output. Không dùng lại jar của checkout. Gỡ project cùng volume của nó.
- [x] **Step 3: ghi V1–V4 và trace của task con bị bỏ qua (spec §2.1, §7.5).** Xong: V1, V2, V3 được xác nhận; V4 chưa đo (hành vi chuẩn); task con bị bỏ qua có một task run `SKIPPED`. Không sự thật nào bị bác bỏ, nhưng khoá output thì khác: `outputs.wdir.outputFiles`, không phải `uris` (spec đã được sửa).
- [x] **Step 4: đo chi phí snapshot (spec §7.3).** Xong cho baseline của Kestra (100 MB: 43,9 s và 57,4 s cho mỗi task con đọc; 1 KB: khoảng 1 s). Số của egret vẫn phải lấy ở Task 6 Step 4. Một probe với một thư mục khoảng 100 MB do task con 1 ghi, task con 2 và 3 đọc, chạy trên Docker với runner Process, được đo thời gian. Chi phí của egret được đo theo cách tương tự ở Task 6. Ghi con số vào spec §7.3.
- [ ] **Step 5: đo copy vào Kubernetes (spec §7.1).** Chưa bắt đầu; cần một cluster kind riêng `egret-wd`, là một bước riêng sau khi đường Docker đã được viết code. Trên một cluster kind **riêng** (tên `egret-wd`, không phải `egret`), kiểm tra xem đường file của runner k8s có copy được một thư mục vào một pod đang chạy hay không, giống cách `files-in` copy input. Ghi câu trả lời vào spec §7.1. Nếu không làm được, đường Kubernetes là một task riêng và nghiệm thu của plan này chỉ có Docker. *(Hoãn: Kubernetes. Ghi ngày 2026-10-19: chưa bắt đầu; đường Docker đã ra và WorkingDirectory bị từ chối trên các runner khác, spec §3 và §7.1.)*
- [x] **Step 6: ghi kết quả đo vào spec** (xong trong spec §2.1, §5.5, §7.3, §7.5); commit theo đường dẫn còn chờ.

**Xong khi:** mọi sự thật trong spec §2.1 đã được xác nhận hoặc spec đã đổi; §7.1 và §7.3 có số hoặc một quyết định.

## Task 2: giữ mọi property lúc parse (spec §3, §5.1)

- [x] **Step 1: test thất bại trước.** (xong) Trong test của `models/task.rs`: một YAML `WorkingDirectory` có `inputFiles`, `outputFiles` và `cache` được parse và round-trip với đủ bốn trường, không bị bỏ.
- [x] **Step 2: thêm các trường** (xong: `input_files`, `output_files`, `cache` là `serde_json::Value` nguyên văn, test `working_directory_keeps_inputs_outputs_and_cache_at_parse` pass) vào `models::WorkingDirectory` (`models/task.rs:580`): `input_files`, `output_files`, `cache`, mỗi cái có hình dạng của Kestra (`Object`, danh sách glob, `Cache { patterns, ttl }`). Giữ `namespace_files` như cũ (W9b).
- [x] **Step 3: xử lý property.** Xong 2026-10-16: đã tạo lại; ba property đọc là `honoured` ở mức model, như `namespaceFiles`. Đó là một phép đo đọc, không phải một lần chạy; block vẫn bị từ chối lúc publish cho tới Task 3. Cập nhật `spec/schema/property-treatment.json` cho ba property: chúng chuyển từ "dropped" sang "kept". Diff chỉ gồm ba mục đó.

**Xong khi:** test parse pass và file treatment ghi "kept" cho ba property.

## Task 3: lập kế hoạch block như một flowable (spec §4.2, §5.1)

- [x] **Step 1: test lập kế hoạch thất bại trước.** Một flow có `WorkingDirectory` với hai task con được lập kế hoạch thành `TaskKind::WorkingDirectory { tasks }` với hai task con, không phải một `Runnable` mờ.
- [x] **Step 2: thêm biến thể** vào `TaskKind` trong `executor/plan.rs`, ánh xạ trong `flatten_flow` cạnh `Sequential` và `AllowFailure`.
- [x] **Step 3: validate.** `spec/validate.rs` (nhánh `WorkingDirectory`, khoảng 1663; phần từ chối `unsupported_working_directory` khoảng 1759). Gỡ `unsupported_working_directory`, vì block nay đã chạy được. Giữ phần từ chối task con không chạy được và kiểm tra `working_directory_tasks_required`. Thêm lại, cho ba property chưa chạy, một từ chối theo tên `working_directory_property_not_yet_run` (nêu tên property), được gỡ bởi Task 7, và thêm mã của nó vào `REFUSED_BY_NAME` trong `spec/schema/measure.rs` để file treatment ghi `refused`. (Task 2 đã thử điều này sớm và thấy nó thừa khi cả block còn bị từ chối; nó thuộc về đây.)
- [x] **Step 4: cập nhật census.** Các test đếm block là mờ sẽ đổi theo, mỗi cái có comment: `executor/mod.rs` quanh 3390-3392, 3546 (`K::WorkingDirectory(_) => PlannedAs::Opaque`), 3589, 3645-3646, 3730, 3975, và 4084 (ghi chú "13 WorkingDirectory"). Các số census dẫn xuất trong `tests/executor.rs` được tính lại từ lập kế hoạch, không bị nới; mỗi thay đổi nói vì sao.

**Xong khi:** `cargo test -p egret-workflow` pass các test lập kế hoạch và validate, và mỗi thay đổi census có phép dẫn xuất trong thông điệp assert của nó.

## Task 4: snapshot như mã thuần (spec §4.3, §5.2)

- [x] **Step 1: test thất bại trước** cho `snapshot::pack(dir) -> Vec<u8>` và `snapshot::unpack(bytes, dir)`: một thư mục có file lồng nhau, một thư mục rỗng và một symlink (bị từ chối, không theo) round-trip được; một thư mục vượt giới hạn trả `SnapshotTooLarge { cap, size }`, nêu tên giới hạn.
- [x] **Step 2: triển khai** `worker/snapshot.rs` với một bộ ghi tar trên `storage::transfer::write_tar` (`storage/transfer.rs:90`), kiểm tra kích thước trước mọi copy, và không thoát đường dẫn (tên `..` và tên tuyệt đối bị từ chối khi unpack).
- [x] **Step 3: kiểm tra mutation.** Bỏ kiểm tra giới hạn; bỏ việc từ chối symlink; cho phép `..` khi unpack. Mỗi cái phải làm một test thất bại. Khôi phục sau mỗi lần.

**Xong khi:** các test round-trip, giới hạn và thoát pass và mỗi mutation bị diệt.

## Task 5: bộ điều khiển của block (spec §4.2, §5.1, §5.3)

- [x] **Step 1: test executor thất bại trước** cho hình dạng của spec §5.3: task con 1 lỗi mà không có `allowFailure`; task con 2 không có run; block là `FAILED`; claim kết thúc đã chạy (`outputs` được ghi). Test thứ hai: task con 1 với `allowFailure: true` lỗi; task con 2 chạy; block là kết quả của task con 2.
- [x] **Step 2: bộ điều khiển** `executor/flowable/working_directory.rs`, xây trên `sequential::drive` cho các task con, với một bước chuẩn bị trước task con đầu và một bước kết thúc sau task con cuối, dù một task con có lỗi hay không (spec V2). Thêm một nhánh dispatch trong `executor/flowable/mod.rs`.
- [x] **Step 3: claim chuẩn bị và claim kết thúc** là các claim riêng của block (spec §5.1). Chúng không mang task con và không có handler; worker chạy chúng bằng các hàm của Task 6 và Task 7.
- [x] **Step 4: kiểm tra mutation.** Bỏ claim kết thúc khi task con lỗi (phải thất bại); để một task con lỗi mà không có `allowFailure` đi tiếp (phải thất bại); chạy một task con có `runIf` false (phải thất bại, vì nó không copy gì và không có run).

**Xong khi:** cả hai test executor pass và mỗi mutation bị diệt.

## Task 6: thư mục bắt đầu của task con (spec §5.2)

- [x] **Step 1: test worker thất bại trước** trên runner Process: task con 1 ghi `a.txt`; task con 2 đọc nó. Không có copy thì test thất bại.
- [x] **Step 2: copy vào và ra** qua seam của runner (`worker/commands.rs`, `TaskRunner`). Trước một task con, snapshot được ghi vào `/egret/working-dir` của task con; sau nó, thư mục được đóng gói thành snapshot kế tiếp. Docker dùng đường staging input (`docker cp`), Kubernetes dùng đường `files-in`/`files-out` chỉ khi Task 1 Step 5 nói nó hoạt động.
- [x] **Step 3: snapshot thiếu** cho một block đã chạy một task con là `WorkerError::Corrupt`, không phải thư mục rỗng (spec §5.2).
- [x] **Step 4: kiểm tra chi phí.** Lặp lại probe của Task 1 Step 4 với egret, và ghi con số cạnh số của Kestra.

**Xong khi:** test Docker pass, test snapshot thiếu pass, và con số chi phí có trong spec.

## Task 7: `namespaceFiles`, `inputFiles`, `outputFiles` và `cache` (spec §2.2, §2.3, §5.4, §5.5)

- [x] **Step 1: test thất bại trước** cho từng cái: `inputFiles` ghi các entry trước task con 1 (tái dùng phần tương đương của egret cho `FilesService.inputFiles`); `namespaceFiles` nạp trước `inputFiles` và `inputFiles` thắng khi trùng tên; `outputFiles` glob thư mục sau task con cuối và ghi `outputs.<block>.uris`; `cache` khôi phục ở lần chạy thứ hai và chỉ lưu khi có file khớp đã đổi.
- [x] **Step 2: triển khai** trên claim chuẩn bị và claim kết thúc, dùng các hàm W9a/W9b mà một script task đã dùng. Khoá cache là `(namespace, flow, block id, hash của giá trị task run)` trong bucket W9a (spec §5.4). Một lần ghi quá giới hạn làm claim kết thúc lỗi.
- [x] **Step 3: kiểm tra mutation.** Nạp `namespaceFiles` sau `inputFiles` (test trùng tên phải thất bại); luôn lưu cache (test "không đổi" phải thất bại); bỏ qua TTL (test hết hạn phải thất bại).

**Xong khi:** mỗi test pass và mỗi mutation bị diệt.

## Task 8: oracle (spec §6)

- [x] **Step 1: đưa các probe vào corpus.** Các flow của Task 1 Step 1 chuyển từ `corpus/probes/` sang `corpus/valids/`, mỗi hành vi một flow, tên `working-directory-real-*`.
- [x] **Step 2: harvest.** Chạy `harvest.py` như README ghi (khoảng 35 phút; nó ghi lại các artifact đã commit, nên chỉ chạy trên stack riêng). Các flow mới có entry golden từ trace của Kestra.
- [x] **Step 3: cập nhật mọi số census đếm corpus.** Corpus tăng từ 345 flow; `tests/executor.rs` giữ con số đó ở nhiều chỗ (các dòng 653, 925, 1331 và các phép dẫn xuất gần đó) và `harvest.py` có guard nêu tên số đếm. Mỗi thay đổi được tính lại từ harvest, không bị nới, và thông điệp assert của mỗi cái nói phép dẫn xuất.
- [x] **Step 4: kiểm tra golden cho các flow mới với các probe của Task 1.** Một chỗ lệch quay lại Task 1 Step 3 và spec; golden không bị sửa.
- [x] **Step 5: ratchet.** Chỉ gỡ khỏi `baseline.txt` các entry có trace nay đã khớp. Bốn WorkingDirectory valid trong golden được kiểm tra theo cùng cách. Không thêm gì.

**Xong khi:** ratchet ở mức bằng hoặc thấp hơn giá trị ban đầu, mọi flow mới khớp với image, và không có số census nào không được giải thích.
