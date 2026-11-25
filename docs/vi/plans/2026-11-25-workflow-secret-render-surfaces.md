# secret() ở các render surface (gói 10a) — Kế hoạch triển khai

> **Cho worker agentic:** SUB-SKILL BẮT BUỘC: Dùng superpowers:subagent-driven-development để triển khai plan này theo từng task. Các bước dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Mục tiêu:** Resolve `secret()` trong Log `message:`, `runIf:` của task, inputs của flow, và triggers,
mask mọi giá trị nó trao ra toàn-execution — tái dùng resolver + masker đã có, không phát minh gì.

**Kiến trúc:** Confinement chỉ là một resolver được cài. `render_with_secrets(props, cx, workspace,
source) -> (Value, Resolved)` (`crates/egret-workflow/src/runners/secrets.rs`) hôm nay chỉ được handler
plugin `kestra.*` gọi (`src/worker/handlers/kestra.rs`). Mọi surface khác render đè lên `secret` stub
từ-chối. 10a gọi cùng resolver đó trên bốn đường render nữa và nạp mỗi `Resolved::values()` vào tập
redaction của execution qua `RenderContext::redact(values)` (`src/runners/env.rs`), cùng kênh SECRET
inputs đã dùng (`src/worker/render.rs`, `cx.redact(&self.secrets)`). Masker
`logs::mask(text, cx.redactions())` và seal AES `secret_inputs` đã tồn tại.

**Tech Stack:** Rust, crate `egret-workflow`; render minijinja; các module có sẵn `runners::secrets`,
`runners::env::RenderContext`, `logs::mask`, `secret_inputs`. Không migration, không API, không field
model.

**Spec:** `docs/vi/specs/2026-11-25-workflow-secret-render-surfaces-design.md` (EN bên cạnh). Vị trí seam
chính xác do Task 0 xác nhận trên worktree này (tên hàm ổn định; số dòng thì không — Task 0 định vị lại
mọi seam trước khi Task 1–4 đụng vào).

## Ràng buộc toàn cục

- Chat tiếng Việt; code/commit/docs tiếng Anh. Conventional Commits; `Closes #N` chỉ trong PR body.
- Trailer commit `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body kết thúc
  `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Stage theo path tường minh; không `git add -A`; không stage `target*/`. Không commit secret/scratch.
- MỘT heavy Rust build một lúc; build chạy FOREGROUND trong subagent. `cargo fmt --all --check` trước push.
- Scoped run `egret-workflow` PHẢI gồm `--test routes --test execution_timestamps`.
- Bank **0 corpus flow**: census tuple, `LIVE`/`VALIDATE_REFUSED` của driver, và `PublishedButRefused`
  KHÔNG được dịch chuyển. Nếu có cái nào đổi, nghĩa là đã lift một surface mà một corpus flow dùng — DỪNG
  và hoà giải (sẽ biến 10a thành corpus-coupled như 9a/9b, điều phép đo nói là không).
- **Kỷ luật lift-refusal (cắn 9a hai lần, 9b một lần):** trước khi gỡ bất kỳ từ-chối nào, grep **code**,
  **message text**, VÀ mọi **fixture YAML** assert nó, khắp `src/ tests/ oracle-exec/`.
- **Measurement-first:** không test nào assert terminal hành-vi-Kestra cho tới khi Task 0 ghi vào spec §6.
- DB: postgres + valkey cục bộ cho worktree (set cả `DATABASE_URL` và `EGRET_DATABASE__URL`). Chạy scoped
  `--test X` cục bộ; full `-p egret-workflow` treo — dùng CI cho toàn suite.

---

### Task 0: Đo Kestra, định vị seam, map blast radius của lift

**Files:**
- Modify (ghi findings): §6 của spec `docs/{en,vi}/specs/2026-11-25-workflow-secret-render-surfaces-design.md`
- Read-only: checkout Kestra `data-portal/kestra` @ `96893b589`; `src/runners/secrets.rs`,
  `src/runners/env.rs`, `src/worker/render.rs`, `src/worker/handlers/`, `src/spec/validate.rs`,
  `src/secret_inputs.rs`, Log handler, seam eval runIf, seam render inputs lúc start, seam eval trigger.

**Interfaces:**
- Produces: §6 spec đã điền (M1–M4 + quyết định D3); bốn render seam định vị theo hàm trên worktree này;
  grep blast-radius lift đầy đủ. Task 1–5 tiêu thụ.

**Deliverable:** §6 spec điền xong; bốn render seam định vị theo hàm; grep blast-radius lift đã chụp lại.
Không đổi code production. Task này gate mọi task sau.

- [ ] **Bước 1: M1 masking reach.** Trong checkout Kestra, xác nhận giá trị `secret()` được mask ở mọi
  dòng log của execution (không chỉ task resolve). Ghi class thực thi (`file:line`) vào §6.
- [ ] **Bước 2: M2 posture inputs (QUYẾT ĐỊNH mở).** Đo: khi input value/default render ra `secret()`,
  Kestra lưu plaintext vào inputs của execution hay giữ ra ngoài? Ghi đáp án và chốt **D3** trong §6:
  seal qua `secret_inputs::split` (nếu giữ ra ngoài) hay redaction-only (nếu lưu). Task 3 đọc quyết định này.
- [ ] **Bước 3: M3 thời điểm trigger.** Ghi lúc nào `secret()` của Schedule/Webhook resolve và giá trị có
  bị mask trong log của chính trigger không.
- [ ] **Bước 4: M4 runIf.** Xác nhận `secret()` trong `runIf` ra boolean và không lộ giá trị trừ qua error
  message.
- [ ] **Bước 5: Seam map.** Ghi hàm + file chính xác cho render của mỗi surface trên worktree NÀY: render
  message của Log handler; eval runIf; render inputs lúc start; render điều kiện/labels của trigger. Xác
  nhận chữ ký chính xác của `render_with_secrets` và tên accessor giá trị của `Resolved` ở đây.
- [ ] **Bước 6: Lift map.** `grep -rn` khắp `src/ tests/ oracle-exec/` cho code, message text, và fixture
  YAML của mỗi secret-refusal đụng Log/runIf/inputs/triggers. Chụp danh sách đầy đủ; đây là thẩm quyền cho
  lift của Task 1–4. Xác nhận `source_uses_secret` phủ mọi surface 10a (D5).
- [ ] **Bước 7: Commit** cập nhật §6 spec
  (`docs(workflow): record Kestra secret() render-surface measurements (10a Task 0)`).

---

### Task 1: secret() trong Log `message:`

**Files:**
- Modify: Log handler (`core.log.Log`; định vị trong `src/worker/handlers/` — seam map Task 0) để render
  `message:` bằng `render_with_secrets` và `cx.redact(resolved.values())` trước khi ghi dòng.
- Modify: `src/spec/validate.rs` — lift arm secret-refusal của Log (+ unit test, + ghi chú header).
- Modify: prose header `src/runners/secrets.rs` (bỏ `Log` khỏi danh sách "render against the refusing stub").
- Test: test mới chứng minh flow Log chạy và plaintext vắng mặt.

**Interfaces:**
- Consumes (xác nhận ở Task 0): `render_with_secrets(props, cx, workspace, source) -> (Value, Resolved)`;
  `Resolved::values()`; `RenderContext::redact(&[String])`.
- Produces: render message Log resolve `secret()` và góp giá trị vào redactions.

- [ ] **Bước 1: Viết test fail.** Flow có Log task với `message:` là `"pw={{ secret('DB_PASSWORD') }}"`,
  secret workspace `DB_PASSWORD=hunter2-SENTINEL`, caller giữ `WF_USE_SECRETS`. Drive nó; assert task thành
  công VÀ không dòng log lưu lại nào và không history lưu trữ nào chứa `hunter2-SENTINEL` (assert dạng mask
  `pw=******` thay vào đó).
- [ ] **Bước 2: Chạy — kỳ vọng FAIL** (hôm nay render Log từ chối `secret()`).
- [ ] **Bước 3: Nối resolver.** Trong Log handler, thay render thường của `message:` bằng
  `render_with_secrets`, rồi `cx.redact(resolved.values())` trước khi ghi dòng log (soi `handlers/kestra.rs`).
  Lift arm từ-chối Log trong `validate.rs` (dùng lift map Task 0 — code VÀ message VÀ fixture).
- [ ] **Bước 4: Chạy — kỳ vọng PASS.**
- [ ] **Bước 5: Mutation check.** Tạm bỏ lời gọi `cx.redact(...)`; test phải đỏ (plaintext giờ rò). Khôi
  phục. Ghi test là mutation-sensitive.
- [ ] **Bước 6: Chạy scoped bar** `cargo test -p egret-workflow --test <this> --test routes --test
  execution_timestamps --test executor` (và xác nhận census tuple KHÔNG đổi — banks 0).
- [ ] **Bước 7: Commit** (`feat(workflow): resolve secret() in Log message, masked execution-wide`).

---

### Task 2: secret() trong `runIf:` của task

**Files:**
- Modify: seam eval `runIf` (seam map Task 0) để render bằng `render_with_secrets` và
  `cx.redact(resolved.values())`.
- Modify: `src/spec/validate.rs` — lift arm secret-refusal của runIf (+ test + header).
- Modify: prose header `src/runners/secrets.rs` (bỏ `runIf`).
- Test: flow mà `runIf: "{{ secret('FLAG') == 'on-SENTINEL-7f' }}"` gate một task.

**Interfaces:**
- Consumes: cùng resolver + `redact` như Task 1.
- Produces: eval runIf resolve `secret()` ra boolean và góp giá trị vào redactions.

- [ ] **Bước 1: Test fail.** Secret `FLAG=on-SENTINEL-7f`; một task
  `runIf: "{{ secret('FLAG') == 'on-SENTINEL-7f' }}"` chạy, một sibling so sánh lệch bị SKIPPED; assert
  không log/message/history lưu lại nào chứa `on-SENTINEL-7f`.
- [ ] **Bước 2: Chạy — kỳ vọng FAIL** (runIf từ chối `secret()` hôm nay).
- [ ] **Bước 3: Nối resolver** ở seam runIf; lift từ-chối runIf (lift map Task 0).
- [ ] **Bước 4: Chạy — kỳ vọng PASS.**
- [ ] **Bước 5: Mutation check.** Bỏ redact; một giá trị secret tới error message phải làm test đỏ. Nếu
  runIf không log giá trị trên happy path, thêm ca render-fail để lộ nó qua error message cho mutation
  quan sát được.
- [ ] **Bước 6: Scoped bar** (gồm kiểm census-không-đổi qua `--test executor`).
- [ ] **Bước 7: Commit** (`feat(workflow): resolve secret() in runIf, masked execution-wide`).

---

### Task 3: secret() trong inputs của flow (posture theo D3 của Task 0)

**Files:**
- Modify: seam render-inputs-lúc-start (seam map Task 0 — đường start trong `src/routes.rs` /
  `src/secret_inputs.rs` / `src/worker/render.rs`).
- Modify: `src/spec/validate.rs` — lift arm secret-refusal của inputs (+ test + header).
- Modify: prose header `src/runners/secrets.rs` (bỏ `inputs`).
- Test: input flow có value/default là `{{ secret('TOKEN') }}`.

**Interfaces:**
- Consumes: resolver + `redact`; **nếu D3 = seal:** `secret_inputs::split(inputs, declaration) ->
  (masked, sealed)` và `reveal` ở `Surface::load`.
- Produces: input mà giá trị `secret()` render được mask khắp nơi (và, nếu D3 = seal, không bao giờ lưu
  plaintext vào `executions.inputs`).

- [ ] **Bước 1: Đọc D3 từ §6 spec** (Task 0 chốt). Assertion của task này phụ thuộc nó: seal → assert
  `executions.inputs` giữ `MASK`, không phải plaintext; redact-only → assert giá trị được mask trong log
  nhưng có thể lưu (khớp Kestra).
- [ ] **Bước 2: Test fail.** Secret `TOKEN=tok-SENTINEL-9a`; một input flow value `{{ secret('TOKEN') }}`
  được một `{{ inputs.x }}` downstream tiêu thụ; assert theo D3 và không dòng log nào rò plaintext.
- [ ] **Bước 3: Chạy — kỳ vọng FAIL.**
- [ ] **Bước 4: Nối** resolver ở seam inputs; nếu D3 = seal, route giá trị resolve qua
  `secret_inputs::split` để seal như một SECRET input; thêm giá trị vào redactions dù cách nào. Lift
  từ-chối inputs (lift map Task 0).
- [ ] **Bước 5: Chạy — kỳ vọng PASS.**
- [ ] **Bước 6: Mutation check** (bỏ seal/redact theo D3 → đỏ).
- [ ] **Bước 7: Scoped bar** (gồm census-không-đổi).
- [ ] **Bước 8: Commit** (`feat(workflow): resolve secret() in flow inputs, masked execution-wide`).

---

### Task 4: secret() trong triggers (Schedule / Webhook)

**Files:**
- Modify: seam render điều kiện/labels của trigger (seam map Task 0 — eval trigger; `webhook_routes.rs`
  cho Webhook).
- Modify: `src/spec/validate.rs` — lift arm secret-refusal của trigger (+ test + header).
- Modify: prose header `src/runners/secrets.rs` (bỏ `triggers`).
- Test: trigger Schedule/Webhook dùng `{{ secret('X') }}`.

**Interfaces:**
- Consumes: resolver + `redact`, áp tại lúc eval-trigger (timing M3).
- Produces: triggers resolve `secret()` với giá trị được mask trong log của trigger.

- [ ] **Bước 1: Test fail** ghim vào timing đo được M3: một trigger mà điều kiện/labels dùng `secret('X')`
  (giá trị `trg-SENTINEL-3c`) resolve, và giá trị vắng mặt trong log lưu lại của trigger.
- [ ] **Bước 2: Chạy — kỳ vọng FAIL.**
- [ ] **Bước 3: Nối** resolver ở seam trigger; lift từ-chối trigger (lift map Task 0).
- [ ] **Bước 4: Chạy — kỳ vọng PASS.**
- [ ] **Bước 5: Mutation check.**
- [ ] **Bước 6: Scoped bar** (gồm census-không-đổi + `--test routes`, vì trigger đụng routes).
- [ ] **Bước 7: Commit** (`feat(workflow): resolve secret() in triggers, masked execution-wide`).

---

### Task 5: Hoà giải, docs, grep cuối

**Files:**
- Modify: header module `src/runners/secrets.rs` (giờ mô tả bốn surface đã lift + hai surface giữ confined:
  KV value, output `value:`).
- Modify: docs-repo `egret-data/docs` `platform/workflows/flows/expressions.md` (và trang
  error-handling / flow-control nếu nhắc confinement secret()): `secret()` giờ chạy ở Log, runIf, inputs
  và triggers, có masking; vẫn từ chối ở KV/output value.
- Test: grep toàn-surface + các control vẫn-từ-chối.

- [ ] **Bước 1: Grep sạch.** Khắp `src/ tests/ oracle-exec/`, xác nhận không còn code/message/fixture
  từ-chối của surface đã lift, và hai surface CONFINED (KV value, output `value:`) VẪN từ chối (giữ/verify
  control — kỷ luật "file thứ 6/7" của 9a/9b).
- [ ] **Bước 2: Chứng minh census-không-đổi.** `cargo test -p egret-workflow --test executor` — tuple,
  `LIVE`, `VALIDATE_REFUSED`, `PublishedButRefused` đều không đổi so với pre-10a (banks 0).
- [ ] **Bước 3: Test control.** `secret()` trong một KV value và một output `value:` vẫn bị từ chối (mỗi
  cái một test, assert đúng code từ-chối còn sống).
- [ ] **Bước 4: Test gate.** Một flow gọi `secret()` ở mỗi surface đã lift vẫn bị từ chối với caller thiếu
  `workflow:use_secrets` (D5).
- [ ] **Bước 5: Docs** cập nhật (EN + VI) trong docs-repo trên nhánh `docs/`; gate `build` của nó xanh.
- [ ] **Bước 6: Full scoped bar** `cargo fmt --all --check` + `cargo clippy` + scoped tests + docs build.
  Commit (`feat(workflow): finish secret() render-surface reach (10a) + docs`).

---

## Tự review

- **Phủ spec:** §1 mục tiêu → Task 1–4 (mỗi surface một task). §3 M1–M4 → Task 0. §4 D1/D2 → wiring
  resolver+redact mỗi task. D3 → Task 0 + Task 3. D4 (KV/output giữ từ-chối) → control Task 5. D5 (gate)
  → Task 0 Bước 6 + Task 5 Bước 4. D6 (kỷ luật lift) → lift map Task 0 + bước lift mỗi task + grep Task 5.
  §8 kiểm thử (vắng-mặt-plaintext + mutation) → các bước mỗi task. §9 rủi ro (lift-trước-redact; sót
  từ-chối) → mutation check + grep Task 0/Task 5. Không gap.
- **Quét placeholder:** giá trị hoãn duy nhất là D3 (inputs seal vs redact), một phép đo có tên với người
  quyết (Task 0) và nhánh điều kiện trong Task 3 — kỷ luật measurement-first spec yêu cầu, không phải
  "TBD". Số dòng seam cố ý bỏ; Task 0 định vị trên worktree này.
- **Nhất quán type:** `render_with_secrets(...) -> (Value, Resolved)`, `Resolved::values()`,
  `RenderContext::redact(&[String])`, `secret_inputs::split -> (masked, sealed)` — dùng nhất quán khắp
  Task 1–4; Task 0 Bước 5 xác nhận accessor/chữ ký chính xác trên worktree này.
