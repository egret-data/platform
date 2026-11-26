# Các loại trigger tùy chọn của workflow (gói 11e) — Kế hoạch triển khai

> **Cho agentic worker:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development để triển khai kế hoạch này từng task. Các step dùng cú pháp checkbox (`- [ ]`).

**Mục tiêu:** Gỡ hai loại trigger mà egret từ chối kèm "package #11 lifts ScheduleOnDates and http.Trigger" — `ScheduleOnDates` (fire theo danh sách ngày tường minh) và `http.Trigger` (poll một HTTP endpoint theo interval và fire khi điều kiện đúng).

**Kiến trúc:** Hai `TriggerKind` variant mới. ScheduleOnDates là Schedulable → tái dùng scheduler hiện có (`workflow.triggers` + `fire.rs`), tính ngày kế tiếp từ danh sách ngày. http.Trigger là polling trigger → một đường fire mới: khi tick interval đến hạn, claim row, commit (nhả lock), thực hiện HTTP request NGOÀI mọi transaction qua `egret-egress`, render `responseCondition`, rồi fire (bind response) hoặc advance, trong transaction mới. Hành vi được xây khi lời từ chối lúc publish vẫn GIỮ cho cả hai loại (kiểm thử qua fixture parse trực tiếp); task reconcile gỡ từ chối + dời census trong MỘT commit.

**Tech Stack:** Rust, axum, sqlx/Postgres, `egret-egress`. Crate `crates/egret-workflow`.

## Ràng buộc toàn cục

- **Đo trước:** không loại nào được honour cho tới khi Task 0 chốt ngữ nghĩa Kestra của nó trên source pin local `/Users/1pro-l4r8/Documents/github/data-portal/kestra` @ `96893b589`.
- **D1** 11e = cả hai loại, một gói. **D2** poll của http.Trigger = claim→commit→poll-ngoài-lock→fire, tái dùng scheduler + `workflow.triggers`; KHÔNG có polling worker riêng. **D3** honour `failOnTriggerError` + `allowConcurrent` cho http.Trigger (nó là polling trigger). **D4** ScheduleOnDates cần **migration 0244** (Task 0): nó fire theo tick ngày idempotent và phải ghi `workflow.executions.scheduled_date` cho guard chống double-fire một-lần-mỗi-tick, nhưng CHECK `workflow_executions_scheduled_date_iff_schedule` (0156) ghim type vào đúng `...Schedule` — 0244 mở rộng CHECK đó để cho phép cả `...ScheduleOnDates` (unique index một-lần-mỗi-tick vốn đã không phụ thuộc type). http.Trigger (polling, `scheduled_date=NULL`) không cần migration. ScheduleOnDates "hết ngày" → egret giữ `next_evaluation_date=NULL` (ra khỏi queue), một khác biệt có ghi tài liệu so với `now+1yr` của Kestra. **D5** egress của http.Trigger tái dùng `egret-egress` + `workflow.http.allow_hosts`; `secret()` resolve trên các field request (#10a). **D6** trigger realtime/fs/plugin vẫn bị từ chối (#18).
- **KỶ LUẬT LIFT-REFUSAL (load-bearing — đã dính 9a×2/9b×1/và ở 11d):** lời từ chối publish/validate của hai loại mới giữ nguyên cho tới task reconcile (Task 4), được gỡ trong CÙNG commit mà hành vi fire đã đúng và census đã sinh lại. Task 1–3 xây hành vi kiểm thử qua fixture `Flow` parse trực tiếp (serde), KHÔNG qua publish. Trước khi gỡ, grep code + message + YAML fixture + mọi test khẳng định các loại này bị từ chối.
- **Census:** `property-treatment.json` do probe sinh + golden-test; nó KHÔNG được dời ở Task 1–3; nó dời một lần, ở Task 4. Baseline sau 11d: **1228 honoured / 3 warned / 354 ignored / 117 refused / 1702 tổng**.
- **Lượt chạy scoped** gồm `--test routes --test execution_timestamps --test scheduler_fire --test scheduler_sync`; test điều khiển tick giữ mutex tuần tự hóa cục bộ theo file (worker tick claim toàn cục).
- Trailer commit `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; Conventional Commits; `Closes #N` chỉ trong PR body. `cargo fmt --all --check` trước khi push; build foreground, từng cái một; env var mới → egret-config `ENV_OUTSIDE_CONFIG`.

---

### Task 0: Đo — ngữ nghĩa Kestra + các seam egret + cơ chế gỡ (load-bearing, chỉ báo cáo)

**File:** không (báo cáo).

- [ ] **Step 1 — ScheduleOnDates.** Trên source pin (`core/.../trigger/ScheduleOnDates.java`): ngữ nghĩa chính xác của `nextDate` — danh sách có được sort không? `lastEvaluation` có inclusive không (code dùng `date.isEqual(lastEvaluation) || date.isAfter(lastEvaluation)`)? `timezone` được áp thế nào? `nextEvaluationDate` trả gì khi danh sách cạn? tương tác recoverMissedSchedules; nó hỗ trợ inputs/labels/when nào.
- [ ] **Step 2 — http.Trigger.** (`core/.../http/Trigger.java` + `HttpInterface` + `Request`): render context chính xác của `responseCondition` (`response.*` lộ ra gì — statusCode, body, headers — và tên biến, `response`); shape `Request.Output` bind làm trigger output; LỖI poll khác điều kiện falsy ra sao (không fire vs failOnTriggerError); interval mặc định (60s) + khả năng override; sub-key nào egret sẽ KHÔNG model (`options`/HttpConfiguration, `encryptBody`, `formData`?) → mỗi cái vẫn bị từ chối kèm gói gỡ nó. Xác nhận failOnTriggerError + allowConcurrent áp dụng (polling trigger, `WorkerTriggerProcessor`).
- [ ] **Step 3 — Cơ chế trigger-kind + từ chối của egret.** Đọc `models/trigger.rs` `TriggerKind` (:75) + `kind_for` (:480): xác nhận cả hai loại hiện route vào `Leaf`. Tìm CHÍNH XÁC cách validate từ chối chúng (`spec/validate.rs:189`/`:3921`) — từ chối keyed theo `TriggerKind::Leaf`, theo type KHÔNG thuộc {schedule,webhook,flow}, hay theo allowlist? Điều này quyết định cách GIỮ từ chối sau khi thêm variant (kế hoạch thời điểm gỡ). Ghi lại gate chính xác + cách giữ hai loại mới bị từ chối lúc publish khi chúng đã là variant có kiểu.
- [ ] **Step 4 — Các điểm dispatch của scheduler.** Liệt kê mọi điểm `TriggerKind::Schedule(_)` trong `scheduler/sync.rs` (:115, :160, :607, :611, :661) và `scheduler/fire.rs` (:244, :257, :926) phải nhận thêm các kind mới, và cách `spec_key`/`next_evaluation_date` được tính, để Task 1–3 mở rộng nhất quán.
- [ ] **Step 5 — Entry egress.** Hàm chính xác mà http.Request của #4 gọi trong `crates/egret-egress` (client.rs/guard.rs) + cách allowlist `workflow.http.allow_hosts` được thực thi, để tái dùng cho poll. Và cách `secret()` resolve trên field của http.Request (#10a) để bắt chước trên field của trigger.
- [ ] **Step 6 — Census + migration.** Baseline 1228/3/354/117/1702; các điểm gỡ từ chối + grep lift-refusal (code+message+fixture+test) cho lời từ chối loại trigger và mọi khẳng định từ chối ScheduleOnDates/http.Trigger. Kết luận migration (dự kiến không cần; số kế tiếp còn trống nếu cần). Ghi lại một commit BASE.

---

### Task 1: `ScheduleOnDates` — trigger Schedulable trên danh sách ngày tường minh

**File:**
- Sửa: `crates/egret-workflow/src/models/trigger.rs` (enum `TriggerKind` :75, `kind_for` :480, nhánh re-serialise :411)
- Tạo: `crates/egret-workflow/src/trigger/schedule_on_dates.rs` (tính ngày kế tiếp)
- Sửa: `crates/egret-workflow/src/scheduler/sync.rs` + `fire.rs` (nhận kind mới; ngày kế tiếp từ dates)
- Test: `crates/egret-workflow/tests/scheduler_fire.rs`

**Interface:**
- Dùng: Task 0 §1 (ngữ nghĩa nextDate) + §3 (giữ bị từ chối lúc publish) + §4 (các điểm dispatch).
- Sinh ra: `TriggerKind::ScheduleOnDates(ScheduleOnDates)`; `schedule_on_dates::next_after(dates, tz, after) -> Option<DateTime<Utc>>`.

- [ ] **Step 1 — Test fail (ngày kế tiếp).** Trong unit test của `trigger/schedule_on_dates.rs`: với `dates = [d1, d2, d3]` (không sort) + một timezone, `next_after(.., after=d1) == Some(d2)` (sau nghiêm ngặt, đã sort), và `next_after(.., after=d3) == None` (cạn). Ghim tính inclusive theo Task 0 §1.

```rust
#[test]
fn next_after_returns_the_first_date_strictly_after() {
    let dates = parse_dates(&["2026-02-01T00:00:00Z", "2026-01-01T00:00:00Z", "2026-03-01T00:00:00Z"]);
    assert_eq!(next_after(&dates, Tz::UTC, dt("2026-01-01T00:00:00Z")), Some(dt("2026-02-01T00:00:00Z")));
    assert_eq!(next_after(&dates, Tz::UTC, dt("2026-03-01T00:00:00Z")), None);
}
```

- [ ] **Step 2 — Chạy → fail** (`next_after` chưa định nghĩa). `cargo test -p egret-workflow schedule_on_dates`.
- [ ] **Step 3 — Model + parse.** Thêm `ScheduleOnDates { dates: Vec<String>, timezone: Option<String>, inputs: Option<Map>, recover_missed_schedules: Option<...> }` theo cách xử lý field của `Schedule` (giá trị giữ nguyên chuỗi tác giả viết; dates được parse lúc fire). Thêm `TriggerKind::ScheduleOnDates(..)` + route nó trong `kind_for` (canonical `io.kestra.plugin.core.trigger.ScheduleOnDates` và short `scheduleOnDates`) + nhánh re-serialise. Cài `next_after`.
- [ ] **Step 4 — Chạy → pass** unit test.
- [ ] **Step 5 — Scheduler nhận + fire.** Test fail trong `scheduler_fire.rs`: một flow (parse trực tiếp, KHÔNG publish) với trigger ScheduleOnDates trên hai ngày quá khứ fire một lần mỗi ngày theo thứ tự, rồi ra khỏi queue. Mở rộng `sync.rs` (cho `ScheduleOnDates` vào bảng triggers; `spec_key` = digest của dates|timezone) và `fire.rs` (tính `next_evaluation_date` qua `next_after`; fire qua đường Schedule — tái dùng inputs/when/labels/stopAfter/allowConcurrent). Giữ lời từ chối publish (Task 0 §3).
- [ ] **Step 6 — Chạy → pass.** Scoped: `cargo test -p egret-workflow --test scheduler_fire --test scheduler_sync --test routes --test execution_timestamps`. `cargo fmt --all --check`.
- [ ] **Step 7 — Commit.** `feat(workflow): ScheduleOnDates trigger (fire on an explicit date list)`

---

### Task 2: `http.Trigger` — model, parse và lên lịch theo interval (chưa poll)

**File:**
- Sửa: `models/trigger.rs` (`TriggerKind` + `kind_for` + re-serialise)
- Tạo: `crates/egret-workflow/src/trigger/http_trigger.rs` (helper model + interval)
- Sửa: `scheduler/sync.rs` + `fire.rs` (nhận kind; `next_evaluation_date = last + interval`)
- Test: `tests/scheduler_fire.rs`

**Interface:**
- Dùng: Task 0 §2 (field, interval mặc định) + §4 (dispatch).
- Sinh ra: `TriggerKind::HttpTrigger(HttpTrigger)` với `uri/method/body/params/headers/content_type/response_condition/interval` (chuỗi tác giả viết/Property); helper `interval()` (mặc định 60s). Poll+fire thực sự là Task 3; ở đây một tick http.Trigger đến hạn được claim và interval được advance, với một seam (`poll_and_fire`) mà Task 3 lấp — cho tới lúc đó một tick đến hạn chỉ advance (không fire), sau lời từ chối publish.

- [ ] **Step 1 — Test fail.** Một flow parse trực tiếp có http.Trigger (`interval: PT30S`) được nhận vào `workflow.triggers` và `next_evaluation_date` của nó advance theo interval ở một lượt đến hạn (chưa có execution). Assert row tồn tại và ngày advance 30s.
- [ ] **Step 2 — Chạy → fail.**
- [ ] **Step 3 — Model + parse + lên lịch.** Thêm struct `HttpTrigger` + `TriggerKind::HttpTrigger` + route `kind_for` (`io.kestra.plugin.core.http.Trigger`). Từ chối các sub-key mà Task 0 §2 nói egret không model (`options`/`encryptBody`/… ) ở parse hoặc validate, mỗi cái nêu gói gỡ nó. Mở rộng `sync.rs` để nhận nó; `fire.rs` coi một http.Trigger đến hạn là "interval tick" (advance `next_evaluation_date += interval`) và gọi seam `poll_and_fire` là stub no-op trong task này.
- [ ] **Step 4 — Chạy → pass.**
- [ ] **Step 5 — Scoped + fmt.** Như Task 1 Step 6.
- [ ] **Step 6 — Commit.** `feat(workflow): http.Trigger model + interval scheduling (poll in next task)`

---

### Task 3: `http.Trigger` — poll, điều kiện, fire và lifecycle

**File:**
- Sửa: `crates/egret-workflow/src/trigger/http_trigger.rs` (poll) + `scheduler/fire.rs` (đường claim→commit→poll→fire)
- Sửa: tái dùng `crates/egret-egress` (dự kiến không đổi) + các bề mặt render/secret
- Test: `tests/scheduler_fire.rs` (hoặc `tests/http_trigger.rs` mới) với một stub HTTP server local

**Interface:**
- Dùng: seam `poll_and_fire` + model `HttpTrigger` của Task 2; Task 0 §2 (ctx responseCondition, shape Request.Output, lỗi-vs-falsy) + §5 (entry egret-egress, allowlist, secret()).
- Sinh ra: không có gì cho downstream.

- [ ] **Step 1 — Test fail (stub server).** Dựng một HTTP stub local. (a) endpoint trả 200 → `responseCondition` (`{{ response.statusCode < 400 }}`) truthy → một execution fire với response được bind dưới `{{ trigger.* }}`. (b) endpoint trả 500 → điều kiện falsy → KHÔNG có execution, interval advance. (c) `failOnTriggerError: true` + request lỗi (connection refused / host không được phép) → một execution born-failed. (d) `allowConcurrent: false` + một execution trước còn sống → poll bị giữ (hold 11d). (e) host không nằm trong allowlist → egress guard chặn, không fire (failOnTriggerError quyết định việc hiển thị lỗi).
- [ ] **Step 2 — Chạy → fail** (poll_and_fire là stub).
- [ ] **Step 3 — Cài poll.** Trong đường fire, với http.Trigger đến hạn: claim row, COMMIT (nhả lock), render các field request (secret() qua #10a), gọi `egret-egress` với guard `workflow.http.allow_hosts`, render `responseCondition` theo response context (Task 0 §2), rồi trong transaction MỚI hoặc fire (born-failed=false; bind `Request.Output` làm trigger record để binding `{{ trigger.* }}` của 11a lộ nó ra) + advance, hoặc chỉ advance. Khi lỗi poll/eval: `failOnTriggerError` true → execution born-failed (cơ chế 11d Task-2), ngược lại `last_error`. Áp hold `allowConcurrent` của 11d trước khi poll.
- [ ] **Step 4 — Chạy → pass** cả năm.
- [ ] **Step 5 — Scoped + fmt.** Thêm `--test http_trigger` nếu là file mới; cộng bộ scheduler/routes/timestamps.
- [ ] **Step 6 — Commit.** `feat(workflow): http.Trigger poll + responseCondition + fire + lifecycle`

---

### Task 4: Reconcile — gỡ từ chối, census, tài liệu

**File:**
- Sửa: `spec/validate.rs` (:189, :3921 từ chối), `spec/keys.rs` (lift_of cho key của các loại mới + failOnTriggerError honoured-trên-http.Trigger), `spec/schema/measure.rs` (read-set cho hai loại), `spec/schema/property-treatment.json` (sinh lại), `spec/schema/README.md`
- Tạo: tài liệu trong egret-data/docs (PR riêng, do controller xử lý)

**Interface:**
- Dùng: bản đồ gỡ Task 0 §6. Cả hai loại trở thành honoured; `failOnTriggerError` chuyển từ Inert phẳng sang honoured-trên-http.Trigger (vẫn Inert/warned trên Schedule/Webhook/Flow); allowConcurrent cũng honoured trên http.Trigger.

- [ ] **Step 1 — Gỡ từ chối (một commit, hành vi đúng).** Bỏ ScheduleOnDates + http.Trigger khỏi lời từ chối unsupported-trigger; cập nhật thông báo để chỉ liệt kê những gì còn bị từ chối (realtime/fs/plugin → #18). Grep + cập nhật mọi test/fixture từng khẳng định hai loại này bị từ chối (kỷ luật lift-refusal).
- [ ] **Step 2 — Read-set census + lift_of.** `measure.rs`: thêm các key honoured của hai loại vào read-set. `keys.rs`: `failOnTriggerError` honoured trên http.Trigger (không Inert ở đó); allowConcurrent honoured trên http.Trigger; giữ treatment Schedule/Webhook/Flow từ 11d.
- [ ] **Step 3 — Sinh lại census golden.** `EGRET_WRITE_PROPERTY_TREATMENT=1`, rồi chạy lại. Ghi tổng hợp mới (Task 0 ghim delta kỳ vọng: hai row trigger mới honoured + failOnTriggerError/allowConcurrent trên http.Trigger; tổng tăng theo số key của hai loại mới). Cập nhật các row README + dòng Result.
- [ ] **Step 4 — Chạy** `cargo test -p egret-workflow spec::keys` + census golden + `--test routes --test execution_timestamps` xanh; `cargo fmt --all --check`.
- [ ] **Step 5 — Tài liệu (egret-data/docs, PR riêng).** Ghi tài liệu cho cả hai trigger + các khác biệt (poll trong scheduler thay vì worker; allowlist egress; sub-key không model). Song ngữ.
- [ ] **Step 6 — Commit.** `feat(workflow): lift ScheduleOnDates + http.Trigger refusal + reconcile census (package 11e)`

---

## Ghi chú tự review

- **Phủ spec:** ScheduleOnDates → Task 1; http.Trigger model/lên lịch → Task 2, poll/fire/lifecycle → Task 3; gỡ từ chối + census + failOnTriggerError-trên-http.Trigger → Task 4; đo/bản đồ gỡ/migration → Task 0. D1–D6 là Ràng buộc toàn cục.
- **Thời điểm gỡ:** từ chối giữ qua Task 1–3 (hành vi kiểm thử qua fixture parse trực tiếp), chỉ gỡ ở Task 4 — kỷ luật 9a; census dời một lần.
- **Migration:** dự kiến không cần (tái dùng `workflow.triggers`); nếu Task 0 thấy polling cần state lưu trữ, chèn một task migration trước Task 2.
- **Rủi ro:** Task 3 nặng nhất (đường poll mới + egress + lifecycle); nếu vượt, năm test case (a–e) đủ độc lập để land từng phần, nhưng giữ một task cho một reviewer gate.
