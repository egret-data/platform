# AllowFailure flowable: plan triển khai

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/plans/2026-10-16-workflow-allow-failure-flowable.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Spec:** `docs/en/specs/2026-10-16-workflow-allow-failure-flowable-design.md`. Đọc spec trước; plan này tham chiếu các mục của nó (§3 hành vi Kestra, §5 thiết kế, §6 nghiệm thu).

**Mục tiêu:** banked `valids/allow-failure`, `valids/allow-failure-with-retry` và `valids/finally-allowfailure` theo image, mỗi dòng §5.1 có unit test, và không thêm entry nào vào ratchet.

**Nhánh:** `feat/workflow-allow-failure-flowable` từ `origin/main`. Một worktree mới, không dùng `egret-w7f` vì nó đang giữ công việc khác.

**Quy tắc áp dụng:** oracle là bài kiểm tra nghiệm thu (spec §8). Không bao giờ đổi `golden.json` để khớp egret. Chỉ chạy một suite cargo hoặc test tại một thời điểm. Không bao giờ dùng dev stack dùng chung (`:5432`, `deploy/docker-compose.dev.yml`, bucket dùng chung). Test cần DB chạy trên DB riêng.

## Task 1: đo bảng trên image (§5.1, §3.2, §7)

Chưa viết code nào trước khi xong task này. Cách đọc Java của spec lấy từ source; image đã ghim là chuẩn tham chiếu.

- [ ] **Bước 1: liệt kê các flow corpus cần đo.** Ba mục tiêu cộng hai nhóm đối chứng: một `Sequential` có task con lỗi, và một task lỗi có flag `allowFailure: true` đứng một mình.
- [ ] **Bước 2: chạy từng flow trên image** (`kestra/kestra@sha256:28f59bf175e1…`, một container dùng một lần, `server local`) qua cùng phương pháp probe mà oracle dùng. Ghi mọi chuyển trạng thái cùng task run id. Không dùng lại jar của checkout.
- [ ] **Bước 3: đối chiếu golden.** So trace của image với `oracle-exec/golden.json` cho ba mục tiêu. Nếu khác, golden sai với phiên bản này và sự khác biệt là phát hiện đầu tiên của task, không được sửa âm thầm.
- [ ] **Bước 4: đo tổ hợp ở §3.2.** Một block có `allowWarning: true` mà task con là `WARNING` (từ một `allowFailure: true` lồng bên trong), và cùng block không có nó. Ghi trạng thái cuối của block.
- [ ] **Bước 5: ghi bảng đo** vào §5.1 của `docs/en/specs/…` thành một cột "measured" cạnh các dòng, rồi commit. Nếu một dòng khác §5.1, sửa §5.1 trước, rồi mới viết code.

**Xong khi:** mọi dòng của §5.1 có trạng thái đã đo, và §5.1 khớp với nó.

## Task 2: bảng thành code thuần, có test (§5.1, §5.2)

- [ ] **Bước 1: test thất bại trước.** Trong `executor/flowable/allow_failure.rs`, một test dạng bảng trên bốn dòng của §5.1 cộng tổ hợp `allowWarning` từ Task 1. Cho trạng thái terminal của các task con và các flag; khẳng định trạng thái block. Test thất bại vì hàm chưa tồn tại.
- [ ] **Bước 2: cài ánh xạ thành hàm thuần** `fn block_state(children: RunState, errors_ran: bool, allow_warning: bool) -> RunState`, cạnh `soften` trong `executor/allow_failure.rs` hoặc trong module mới. Nó không đụng tới plan hay state machine.
- [ ] **Bước 3: kiểm tra mutation.** Đổi từng nhánh một lần (`FAILED` → `SUCCESS`; bỏ `allow_warning`; để `errors_ran` quyết định) và xác nhận có test thất bại cho mỗi cái. Khôi phục sau mỗi lần.

**Xong khi:** test bảng qua và mọi mutation bị giết.

## Task 3: lập plan cho cấu trúc (§5.2)

- [ ] **Bước 1: test plan thất bại trước.** Một flow có một `AllowFailure` chứa hai task con được lập plan thành `TaskKind::AllowFailure { tasks }` với hai task con; các danh sách `errors:` và `finally:` được mang theo.
- [ ] **Bước 2: thêm biến thể** vào `TaskKind` trong `executor/plan.rs`, ánh xạ từ node trong `flatten_flow` cạnh `Sequential`. Bỏ đường refuse cho node này trong `spec/validate.rs` (nhánh `AllowFailure` của `walk_task`), giữ bước duyệt `errors:` lồng nhau.
- [ ] **Bước 3: giữ refuse cho `retry:`** trên block (§5.3), với một mã được nêu trong commit message của task này.
- [ ] **Bước 4: cập nhật các test đã đếm nó là opaque**: `executor/mod.rs` quanh dòng 3390-3410, 3960-3970, 4140 và 4235. Số lượng corpus (6) không đổi. Mỗi khẳng định được di chuyển đều ghi lý do trong comment.

**Xong khi:** `cargo test -p egret-workflow` qua các test plan và validate, và số lượng corpus không đổi.

## Task 4: chạy thật (§5.1, §5.2)

- [ ] **Bước 1: test executor thất bại trước** cho hình dạng của ba mục tiêu: một task con lỗi, anh em của nó bị bỏ qua, `errors:` chạy, `finally:` chạy, block là `WARNING`, flow sau nó chạy.
- [ ] **Bước 2: handler** `executor/flowable/allow_failure.rs`: gọi `sequential::drive` cho các task con, errors và finally, rồi áp `block_state` của Task 2 lên trạng thái terminal. Thêm một nhánh dispatch trong `flowable/mod.rs`.
- [ ] **Bước 3: chạy các test executor** trong `crates/egret-workflow/tests/executor.rs` và các unit test `executor::`, tất cả xanh.

**Xong khi:** test executor qua và anh em không được chạy.

## Task 5: oracle (§6.1, §6.5)

- [ ] **Bước 1: chạy oracle** với lệnh trong `oracle-exec/README.md`, trên DB riêng.
- [ ] **Bước 2: kiểm ba mục tiêu khớp golden.** Mọi sai khác quay lại Task 1 Bước 5 và spec; golden không được sửa.
- [ ] **Bước 3: xóa các dòng đã banked** khỏi `oracle-exec/baseline.txt` (`valids/allow-failure`, `valids/allow-failure-with-retry`, `valids/finally-allowfailure`), và cập nhật mọi số đếm trong comment đầu file nếu có nêu chúng.
- [ ] **Bước 4: xác nhận ratchet giảm đúng ba** và không thêm gì.
- [ ] **Bước 5: `valids/retry-with-flowable-errors`** vẫn bị baseline; comment của nó trong `baseline.txt` nêu ngoài phạm vi retry ở §2.

**Xong khi:** ratchet thấp hơn đúng ba và ba entry khớp với image.

## Task 6: gate và commit (§6.6)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] `cargo test -p egret-workflow` (một suite tại một thời điểm; test cần DB chạy trên DB riêng)
- [ ] Lần chạy oracle của Task 5, ghi lại kèm số đếm.
- [ ] Commit theo đường dẫn cụ thể, message theo Conventional Commits, `Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>`. Không bao giờ `git add -A`.

## Task 7: tài liệu

- [ ] Cập nhật `docs/en/specs/2026-09-01-workflow-programme-design.md` §9: dòng flowable `AllowFailure`, và danh sách những việc vẫn chưa đúng, kèm ngày.
- [ ] Bản tham khảo tiếng Việt của spec và plan này trong `docs/vi/` (quy ước; sau khi bản tiếng Anh đã xong).

## Ngoài phạm vi

- Retry trên block (`valids/retry-with-flowable-errors`, spec §2).
- Flag `allowFailure:` (W8, `executor/allow_failure.rs`).
- Ba mục còn lại trong danh sách "vẫn chưa đúng" của chương trình (WorkingDirectory, ghi file namespace từ plugin, KV bridge, che giá trị KV trong log).
