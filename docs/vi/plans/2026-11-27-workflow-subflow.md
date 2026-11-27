# Workflow Subflow parity (gói #12) — Kế hoạch triển khai

> **Dành cho agent:** BẮT BUỘC dùng sub-skill: superpowers:subagent-driven-development. Các bước dùng `- [ ]`.

**Mục tiêu:** Tôn trọng bốn trong năm key Subflow mà egret đang bỏ qua — `labels`, `inheritLabels`, `revision`, `restartBehavior` — cộng sửa "LoopUntil Subflow child matched by position only". (`scheduleDate` tách sang **#12b**: Task 0 thấy nó cần hệ thống deferred-execution-start mới + migration, lớn hơn nhiều so với nối một key.)

**Kiến trúc:** egret đã chạy `TaskKind::Subflow`. Mỗi key được tôn trọng tại seam spawn/restart của Subflow (`worker/handlers/subflow.rs`, `NewExecution` của con, `restart::open`). labels/inheritLabels đưa vào label của con; revision dùng lại flow-version 7b (`FlowRepo::get_version`); restartBehavior dùng lại seam restart 7a/7b. Hành vi ở Task 1–3; một commit reconcile (Task 4) dịch chuyển census.

**Tech Stack:** Rust, axum, sqlx/Postgres. Crate `crates/egret-workflow`.

## Ràng buộc chung

- **Đo trước:** Task 0 chốt ngữ nghĩa trên Kestra local `96893b589` trước khi tôn trọng bất kỳ key nào.
- **D1** bốn key (labels/inheritLabels/revision/restartBehavior), một gói; `scheduleDate` → #12b (Task 0: cần hệ thống trì hoãn + migration). **D2** revision dùng lại flow-version 7b `FlowRepo::get_version` (mặc định mới nhất). **D3** restartBehavior: RETRY_FAILED mặc định (7a) + NEW_EXECUTION qua seam restart 7a/7b (gate `restart_earlier_child`) + sửa LoopUntil child-by-identity. **D4** migration: labels/revision/restartBehavior không cần migration; sửa LoopUntil-identity CÓ THỂ cần một cột child-identity (0245) — Task 3 quyết định. **D5** census dịch chuyển một lần (Task 4); Task 1–3 không đụng keys.rs/measure.rs/property-treatment.json; `scheduleDate` vẫn ignored/warned, trỏ về #12b.
- **KỶ LUẬT LIFT-REFUSAL:** đây là các dịch chuyển census `ignored`→honoured (không có refusal khi publish để lift — key đã parse), nhưng vẫn grep các test assert cảnh báo trước Task 4. Các task hành vi giữ nguyên census.
- **Chạy scoped** gồm bộ Subflow/executor/restart/routes/execution_timestamps (+ những gì Task 0 nêu); test chạy tick/worker giữ mutex tuần tự hóa cục bộ file.
- Trailer commit `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; Conventional Commits; `Closes #N` chỉ trong body PR. `cargo clippy --all-targets -- -D warnings` (CI chạy; `cargo check` đơn lẻ bỏ sót large_enum_variant/clippy lint) và `cargo fmt --all --check` trước khi push; build foreground.

---

### Task 0: Đo — ngữ nghĩa Subflow của Kestra + seam của egret (có tính quyết định, chỉ báo cáo)

**Files:** không.

- [ ] **Bước 1 — labels/inheritLabels.** Cách Kestra ghép label con: `labels` riêng (đã render), `inheritLabels` gộp label execution cha, system label, và THỨ TỰ; context render cho giá trị label.
- [ ] **Bước 2 — revision.** Cách flow-version của 7b resolve flow tại revision N (hàm + bảng); lỗi khi thiếu revision; xác nhận egret lưu lịch sử revision dùng được từ đường spawn Subflow. Mặc định = mới nhất (hiện tại).
- [ ] **Bước 3 — scheduleDate.** Seam của egret gate việc start execution con theo `scheduled_date` (scheduler/worker); cha `wait:true` xử lý thế nào qua khoảng trì hoãn; cách Kestra xử lý `scheduleDate` quá khứ hoặc không hợp lệ.
- [ ] **Bước 4 — restartBehavior + LoopUntil-position.** Ngữ nghĩa NEW_EXECUTION của Kestra (số phận con đã start, liên kết cha↔con, cách outputs resolve) so với RETRY_FAILED; seam Subflow của `restart::open` cần mở rộng; vị trí chính xác của bug "LoopUntil Subflow child matched by position only" (ghi chú 7a) và cách con nên được khớp với nguồn theo identity.
- [ ] **Bước 5 — census + migration + lift map.** Baseline census sau #11; read-set `measure.rs` + vị trí lift `keys.rs` của các key; grep lift-refusal (test/fixture assert cảnh báo); kết luận migration (dự kiến không cần; số tiếp theo nếu cần). Ghi lại một BASE commit.

---

### Task 1: `labels` + `inheritLabels` trên execution con

**Files:** Sửa `crates/egret-workflow/src/worker/handlers/subflow.rs` (build `NewExecution` của con); dùng lại `src/labels.rs`. Test: `crates/egret-workflow/tests/subflow*.rs` (file Task 0 nêu).

**Interfaces:** Dùng Task 0 §1. Tạo ra cách ghép label con mà đường restart (Task 4) cũng tôn trọng.

- [ ] **Bước 1 — Test lỗi.** Flow cha có task Subflow khai báo `labels: {team: x}` và `inheritLabels: true`, execution cha mang label `env: prod` → con mang CẢ HAI (thứ tự theo Task 0 §1). Ca thứ hai `inheritLabels: false` → con chỉ mang label riêng.
- [ ] **Bước 2 — Chạy → fail.**
- [ ] **Bước 3 — Hiện thực.** Render `labels` của Subflow theo context task; khi `inheritLabels` render true, ghép với label execution cha theo thứ tự `visible_labels` (Task 0 §1); đưa vào `NewExecution.labels` của con. Label render lỗi thì bị bỏ (Kestra, như webhook 11c) trừ khi Task 0 nói khác.
- [ ] **Bước 4 — Chạy → pass.**
- [ ] **Bước 5 — Scoped + fmt. Bước 6 — Commit** `feat(workflow): Subflow labels + inheritLabels on the child execution`.

---

### Task 2: `revision` — spawn một revision cụ thể của subflow

**Files:** Sửa `worker/handlers/subflow.rs` (resolve flow) dùng lại tra cứu version của 7b. Test: test subflow.

**Interfaces:** Dùng Task 0 §2 (hàm resolve version). Không tạo gì cho downstream.

- [ ] **Bước 1 — Test lỗi.** Một subflow có hai revision; task Subflow của cha đặt `revision: 1` → con chạy source của revision 1; không có `revision` → chạy mới nhất. Revision không tồn tại → lỗi Task 0 §2 chốt.
- [ ] **Bước 2 — Chạy → fail.**
- [ ] **Bước 3 — Hiện thực.** Khi `revision` được đặt, resolve source subflow tại revision đó qua cơ chế 7b (Task 0 §2) thay vì bản mới nhất; mặc định không đổi.
- [ ] **Bước 4 — Chạy → pass. Bước 5 — Scoped + fmt. Bước 6 — Commit** `feat(workflow): Subflow revision (spawn a specific subflow revision)`.

---

### Task 3: `restartBehavior` NEW_EXECUTION + LoopUntil child-by-identity

**Files:** Sửa `src/restart*.rs` (seam Subflow của `restart::open`, 7a/7b) + `worker/handlers/subflow.rs`. Test: test restart/executor/subflow.

**Interfaces:** Dùng Task 0 §4. Task nặng nhất — cơ chế restart. LƯU Ý: sửa LoopUntil child-by-identity có thể cần migration child-identity (Task 0 §4 — egret không bao giờ đặt `TaskRun::value` trên run Subflow). Nếu vậy, thêm migration 0245 trong task này (đĩa đang ở 0244; migration scheduleDate của #12b lấy số kế tiếp). Xác định việc này trước và báo lên nếu thay đổi schema lớn hơn một cột.

- [ ] **Bước 1 — Test lỗi.** (a) Cha có Subflow `restartBehavior: NEW_EXECUTION` đã fail → restart cha spawn con MỚI (không restart tại chỗ con cũ); số phận con cũ + liên kết theo Task 0 §4. (b) Mặc định `RETRY_FAILED` → không đổi (restart tại chỗ, 7a). (c) LoopUntil có thân là Subflow: con được restart/lặp khớp nguồn theo identity, không theo vị trí list (bug hàng #12) — dựng ca mà Task 0 §4 xác định.
- [ ] **Bước 2 — Chạy → fail.**
- [ ] **Bước 3 — Hiện thực.** Đưa `restartBehavior` vào seam Subflow của `restart::open`: NEW_EXECUTION spawn con mới (sinh qua spawn Subflow thường, liên kết với task run cha) thay vì mở lại con hiện có; RETRY_FAILED giữ 7a. Sửa khớp con LoopUntil theo child identity (Task 0 §4).
- [ ] **Bước 4 — Chạy → pass hết. Bước 5 — Scoped + fmt (restart + executor + subflow + routes + execution_timestamps). Bước 6 — Commit** `feat(workflow): Subflow restartBehavior NEW_EXECUTION + match looped child by identity`.

---

### Task 4: Reconcile — census, cảnh báo, tài liệu

**Files:** Sửa `spec/schema/measure.rs` (read-set Subflow +4 key), `spec/keys.rs` (lift_of cho 4 key; giữ `scheduleDate` warned, trỏ về #12b), `spec/schema/property-treatment.json` (sinh lại), `spec/schema/README.md`. Tạo: bổ sung egret-data/docs (PR riêng, controller xử lý).

**Interfaces:** Dùng lift map Task 0 §5.

- [ ] **Bước 1 — Read-set.** Thêm `labels`, `inheritLabels`, `revision`, `restartBehavior` vào read-set Subflow trong `measure.rs`. KHÔNG thêm `scheduleDate` (hoãn sang #12b).
- [ ] **Bước 2 — Cảnh báo lift.** Bỏ `lift_of` Package(12) của BỐN key Subflow đó. Giữ `scheduleDate` warned nhưng trỏ lift/message về gói sau (#12b) — "một gói tương lai lift nó" (grep + cập nhật test assert cảnh báo trước).
- [ ] **Bước 3 — Sinh lại census.** `EGRET_WRITE_PROPERTY_TREATMENT=1`, rồi chạy lại không có. BỐN ô Subflow ignored→honoured (scheduleDate vẫn ignored/warned); ghi tổng mới (baseline Task 0 1257/3/367/120/1747 → honoured +4 = 1261/3/363/120/1747); cập nhật hàng README Subflow.
- [ ] **Bước 4 — Chạy** `cargo test -p egret-workflow spec::keys` + golden census + routes/execution_timestamps xanh; `cargo clippy --all-targets -- -D warnings`; `cargo fmt --all --check`.
- [ ] **Bước 5 — Tài liệu (egret-data/docs, PR riêng).** Ghi bốn key Subflow + các divergence Task 0 (NEW_EXECUTION, LoopUntil identity); ghi chú scheduleDate sẽ đến ở gói sau. Song ngữ.
- [ ] **Bước 6 — Commit** `feat(workflow): lift Subflow key warnings + reconcile census (package #12)`.

---

## Ghi chú tự rà

- **Phủ spec:** labels/inheritLabels → T1; revision → T2; restartBehavior + LoopUntil-identity → T3; census/cảnh báo/tài liệu → T4; đo đạc → T0. scheduleDate → #12b. D1–D5 là Ràng buộc chung.
- **Rủi ro:** T3 nặng nhất (cơ chế restart + bug loop-identity, có thể có migration). Nếu vượt, các ca (a)/(b)/(c) tách được nhưng giữ một task cho một cổng reviewer; báo lên nếu ngữ nghĩa restart chống lại kế hoạch.
- **Kỷ luật census:** Task 1–3 chỉ hành vi; Task 4 là lần dịch chuyển census duy nhất (bốn ô ignored→honoured; scheduleDate giữ nguyên).
- **Migration:** dự kiến không cần; nếu Task 0 thấy NEW_EXECUTION cần state lưu trữ, chèn một task migration trước Task 4.
