# AllowFailure block retry — thiết kế (gói 9b)

**Roadmap:** gói #9 tách thành **9a** (Pause flowable đầy đủ, đã merge #911) và **9b** (gói này: `retry:` trên block `AllowFailure`). 9b hoàn tất gói #9.

**Phụ thuộc:** flowable `AllowFailure` (#680), kế thừa retry (`executor/retry.rs`), và harness corpus/golden (`oracle-exec/`).

## 1. Mục tiêu

Gỡ từ chối `unsupported_allow_failure_retry` để một `retry:` khai báo trên block
`AllowFailure` được chấp nhận khi publish/start và chạy đúng như Kestra — và,
trong **cùng một commit**, hoà giải census corpus để corpus flow duy nhất mà từ
chối này đang chặn (`valids/retry-with-flowable-errors`) chuyển từ
*published-but-refused* sang *live và khớp golden*.

## 2. Kestra làm gì — phép đo đã có sẵn trong golden

Phép đo **không** phải bước chạy image trực tiếp. Trace Kestra ghim cho đúng
tương tác này đã nằm trong `oracle-exec/golden.json` (và `corpus.json`/`traces.json`)
dưới tên `valids/retry-with-flowable-errors`: block `AllowFailure` có block-level
`retry:` (constant, `RETRY_FAILED_TASK`, maxAttempts 3, warningOnRetry), một child
`run_script` (Log) fail khi counter < 2 và success khi ≥ 2, và một `errors:` block
tăng KV counter.

Trace golden (oracle mọi test 9b và driver phải tái hiện):

| run | parent | state | attempts |
|---|---|---|---|
| `set_kv` | — | CREATED→…→SUCCESS | 1 |
| `retry_block` (AllowFailure) | — | CREATED→RUNNING→**SUCCESS** | **1** (block không retry như một đơn vị) |
| `run_script` (child) | `retry_block` | …FAILED→RETRYING→…→**SUCCESS** | **2** (attempt 1 FAILED, attempt 2 SUCCESS) |

Execution: `CREATED → RUNNING → RETRYING → RUNNING → SUCCESS`.

Xác nhận 3 sự thật load-bearing, đều đã có sẵn trong engine cho MỌI flowable khác:

- **Flowable không bao giờ retry như một đơn vị** (`retry.rs` `is_retried_kind`
  chỉ liệt kê `Runnable`/`Executable`): block là một run duy nhất, **child** chạy
  các attempt.
- **Retry được kế thừa** (`retry.rs` `inherited_retry`, ~dòng 140-153): child
  không có `retry:` riêng dùng `retry:` của flowable tổ tiên gần nhất.
- **`AllowFailure` ánh xạ terminal** (`executor/allow_failure.rs` `block_state`):
  child fail → block `WARNING`; ở đây child cuối cùng **success** (attempt 2) nên
  block là `SUCCESS`.

`AllowFailure` là flowable DUY NHẤT mà egret đang *từ chối* block `retry:` thay vì
chạy — không phải vì hành vi khác, mà vì tương tác chưa được đo khi #680 ship.
Giờ đã có phép đo nên gỡ từ chối.

> **Ghi chú về errors task `incr_counter`.** Danh sách `runs` của golden KHÔNG chứa
> run `incr_counter` riêng, nhưng `run_script` vẫn success ở attempt 2. Task 0 phải
> đọc `oracle-exec/traces.json` cho flow này và ghi vào §6 cơ chế chính xác attempt
> 2 success (run của errors handler bị gộp/ẩn trong golden shape? hay KV increment
> xảy ra trên đường dẫn golden không surface thành run riêng?). Driver phải tái
> hiện golden **đúng shape đã vendored** — không bịa ra run `incr_counter` mà golden
> bỏ, và không bao giờ sửa golden để khớp egret.

## 3. Coupling corpus-ratchet — gỡ CUỐI, trong MỘT commit (bài học 9a)

Census toàn-suite của egret (`tests/executor.rs` tuple `(301, 279, 22)` = tổng,
matching, baselined; và bảng `PublishedButRefused` trong `src/executor/mod.rs`)
**ghép plan + validate + driver**. `valids/retry-with-flowable-errors` hiện là một
entry `PublishedButRefused` khoá theo `unsupported_allow_failure_retry`. Ngay khi
gỡ từ chối validate, flow đó publish và driver chạy nó — nếu driver chưa tái hiện
golden, suite đỏ giữa chừng.

**QUY TẮC (bê nguyên từ 9a):** gỡ từ chối **cuối cùng, trong cùng commit mà hành
vi đã đúng**. Trước commit đó, phát triển và test hành vi executor qua fixtures
trực tiếp / `plan_stored` với từ chối VẪN còn, để census corpus không đổi và xanh.

**Blast radius của lift** (mọi chỗ mã hoá từ chối cũ — grep, không tin vào liệt kê
của reviewer, theo bài học lift-refusal). Tất cả phải đổi trong commit lift:

- `src/spec/validate.rs` — arm từ chối (~2307) + unit test (`allow_failure_retry_body`, ~6193-6235) + header module (~58).
- `src/executor/mod.rs` — self-probe (~4346) + entry `PublishedButRefused { id: "valids/retry-with-flowable-errors", … }` (~4614) kèm `cause`/`codes`.
- `src/executor/plan.rs`, `src/executor/allow_failure.rs` — doc comment (~plan.rs:607, allow_failure.rs:13).
- `oracle-exec/driver.rs` — đếm LIVE/refused.
- `tests/executor.rs` — census tuple (`22 → 21` baselined, `279 → 280` matching, `21 + 280 = 301`) + danh sách `PublishedButRefused`/over-refusal.
- `tests/routes.rs` — `a_draft_with_an_allow_failure_saves_...` (~362) đang assert mã từ chối; phải lật sang assert block validate sạch (giữ control vẫn-bị-từ-chối, ví dụ một flow thực sự invalid).

Grep của Task 0 (`unsupported_allow_failure_retry` khắp `crates/egret-workflow/{src,tests}` VÀ `oracle-exec/`) là danh sách thẩm quyền; mục này là bản đồ khởi đầu, không thay thế grep.

## 4. Quyết định

- **D1 —** block `retry:` trên AllowFailure được child fail kế thừa, block không retry như một đơn vị, block ánh xạ state cuối (sau retry) qua `allow_failure::block_state` — giống hệt block retry của `Sequential` egret đã chạy. Parity Kestra, không phải luật mới.
- **D2 —** oracle là golden đã vendored, không phải chạy image mới. Task 0 đọc `golden.json`/`traces.json` cho `valids/retry-with-flowable-errors` và ghim test vào trace đó. Đo lại trên image là verify tuỳ chọn, không load-bearing.
- **D3 —** không migration, không API mới, không field model. `retry:` đã là property của `Task`; `AllowFailure` đã mang `tasks:`/`errors:`/`finally:`.
- **D4 —** planner phải giữ AllowFailure-có-`retry:` là flowable (`TaskKind::AllowFailure`), không phải `Runnable` opaque. Ghi chú `retry.rs:52-54` về planner cũ flatten có thể đã stale từ #680 — Task 1 kiểm, chỉ sửa nếu cần.
- **D5 —** gỡ từ chối cuối cùng, một commit, hoà giải toàn bộ blast radius §3. Không tin "xanh trên bar scoped mở rộng" cho tới khi grep sạch và các binary được nêu đã lên bar.

## 5. Phạm vi

**Trong:** gỡ từ chối; verify/tạo run khớp golden của AllowFailure block retry;
hoà giải census + golden coupling; test hành vi cho đường exhausts-retry (→WARNING)
và succeeds-late (→SUCCESS, ca corpus) và tương tác `warningOnRetry`; docs.

**Ngoài:** `retry:` trên flowable khác (đã chạy); `CREATE_NEW_EXECUTION` vs
`RETRY_FAILED_TASK` ngoài những gì policy kế thừa đã phân biệt; mọi thứ của 9a
(Pause) hay các gói roadmap sau.

## 6. Khác biệt / sự thật đo được

*(Task 0 điền từ `traces.json` — đặc biệt cơ chế `incr_counter` của §2, và liệu ca
exhausts-retry ánh xạ sang `WARNING` đúng như ca Sequential. Mọi khác biệt với
Kestra ghi ở đây TRƯỚC khi gỡ từ chối.)*

## 7. Kiểm thử

- **Gỡ-từ-chối:** AllowFailure có block `retry:` validate sạch và plan thành `TaskKind::AllowFailure` (không opaque).
- **Hành vi (ghim vào golden / terminal đo được):** corpus flow `valids/retry-with-flowable-errors` chạy live khớp golden; fixture child **luôn** fail → cạn retry → block `WARNING`; `warningOnRetry` log các attempt retry là warning mà không đổi terminal.
- **Corpus xanh:** census toàn-suite hoà giải về `(301, 280, 21)`; bảng `PublishedButRefused` không còn liệt kê flow này.
- **Scoped runs PHẢI gồm `--test routes --test execution_timestamps`** (+ `--test executor` cho census) nếu không CI `--workspace` đầy đủ là thứ đầu tiên bắt được file stale-refusal.

## 8. Rủi ro

Thấp–trung bình. Machinery executor đã có; rủi ro là coupling corpus (§3) — bài học
9a xử lý bằng gỡ-cuối-trong-một-commit-hoà-giải; và điểm tinh tế
`incr_counter`/attempt-2 (§2) — Task 0 giải từ trace đã vendored trước khi bất kỳ
test nào assert terminal.
