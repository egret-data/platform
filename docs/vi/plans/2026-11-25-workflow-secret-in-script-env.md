# secret() trong env của script task (gói 10b) — Kế hoạch triển khai

> **Cho worker agent:** SUB-SKILL BẮT BUỘC: Dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để triển khai từng task. Các bước dùng checkbox (`- [ ]`).

**Mục tiêu:** Cho `secret('NAME')` resolve trong map `env:` của script task, inject an toàn ở nơi runtime cho phép (k8s Secret + secretKeyRef; docker `--env-file`), mask giá trị trong stdout/stderr của script, và giữ mọi field script task khác refuse `secret()`.

**Kiến trúc:** Một seam resolution async trong Script handler (`render()` là đồng bộ và refuse `secret()` trong `env`, nên render author-env chuyển khỏi `render()` vào `run()` async, dùng đường `render_with_secrets` của 10a). Các env entry có secret được tách vào `TaskCommands::secret_env` mới và được mỗi runner inject qua kênh an toàn; một log handle đã mask (`RunLog::masked` của 10a) được luồn vào cả hai runner để output script bị redact.

**Tech Stack:** Rust, `crates/egret-workflow`; axum + sqlx/Postgres; docker CLI runner; k8s runner (`k8s-openapi` `EnvVar`/`Secret`). Tái dùng machinery 10a: `runners::secrets::{render_with_secrets, current, text_uses_secret, Resolved}`, `logs::RunLog::masked`.

## Ràng buộc toàn cục

- Chat tiếng Việt; code/commit/docs tiếng Anh.
- Conventional Commits; `Closes #N` chỉ trong body PR (không trong commit message).
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; body PR kết bằng `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Không bao giờ commit secret hay file scratch. Stage theo path rõ; không `git add -A`; không stage `target*/`.
- MỘT build Rust nặng tại một thời điểm; build chạy FOREGROUND trong subagent.
- Chạy test scoped `egret-workflow` PHẢI kèm `--test routes --test execution_timestamps` (parity CI).
- Merge khi CI xanh, không `--admin`, `cargo fmt --all --check` trước.
- secret() giữ **chỉ-env** trên script task: `commands:`, `script:`, `beforeCommands:`, `interpreter:`, `inputFiles:`, `outputFiles:`, `namespaceFiles:`, field image vẫn refuse.
- DB chạy local: set cả `DATABASE_URL` và `EGRET_DATABASE__URL` về Postgres test của worktree.

## Cấu trúc file

- `crates/egret-workflow/src/worker/commands.rs` — `TaskCommands` thêm `secret_env` + `secret_masks`; `Debug` thủ công redact chúng; `stream_line`/`stream_replayed_line` nhận `log: &RunLog` thay vì `task`; `DockerRunner` build log đã mask + kênh `--env-file`.
- `crates/egret-workflow/src/worker/handlers/script.rs` — `render()` thôi render author `env` và trả về render context; `run()` thêm seam `resolve_env` async sinh `env`/`secret_env`/`secret_masks`.
- `crates/egret-workflow/src/worker/k8s/pod_spec.rs` — env có secret phát `value_from` secretKeyRef.
- `crates/egret-workflow/src/worker/k8s/runner.rs` — tạo Secret per-attempt với ownerReference tới Pod; build log đã mask.
- `crates/egret-workflow/src/spec/validate.rs` — allow-arm cho `env` script task + tập membership `script`.
- `crates/egret-workflow/src/runners/functions.rs` — text message `SECRET_REFUSAL`.
- Tests: `crates/egret-workflow/tests/env_secret.rs` (mới); bổ sung `tests/plugins_gate.rs`; unit test trong `pod_spec.rs` và `commands.rs`.
- Docs (repo riêng `egret-data/docs`): `platform/workflows/flows/expressions.md`, doc RBAC deployment, `known-issues.md`.

> **Lưu ý:** mã nguồn, chữ ký hàm, và các code block bên dưới giữ nguyên như bản EN (`docs/en/plans/2026-11-25-workflow-secret-in-script-env.md`). Bản VI này dịch phần diễn giải; khi triển khai, đọc kèm bản EN để lấy code chính xác.

---

### Task 0: Đo census / Banks (đo trước, không có production code)

**File:** chỉ đọc `tests/executor.rs`, `oracle-exec/`, corpus/fixture mang flow. Sản phẩm: một finding viết trong task report.

**Vì sao:** 10a giữ census ổn định bằng cách để một bề mặt (`triggers.*.key`) refused. 10b gỡ `env` script task. Nếu có flow corpus/oracle nào đang đặt `secret()` trong `env` script task và nằm ở bucket refused, gỡ sẽ flip sang valid và dịch tuple census/oracle. Phải biết **trước khi** viết code và chốt: giữ census ổn định (không flow nào phụ thuộc refusal này) hay ghi nhận ratchet có chủ đích.

- [ ] **Bước 1: Grep corpus** (từ `crates/egret-workflow`):
```bash
grep -rn "secret(" tests/ oracle-exec/ | grep -iE "env|script|shell|container"
grep -rln "secret(" tests/ oracle-exec/
grep -rn "VALIDATE_REFUSED\|PublishedButRefused\|census\|REFUSED" tests/ oracle-exec/ src/ | head -40
```
- [ ] **Bước 2: Chốt và ghi** — nêu kèm file:line một trong: **(A) census không đổi** (không flow nào có secret() trong env script — kỳ vọng, như 10a) → tiếp; **(B) census dịch** → liệt kê từng flow sẽ flip + quyết định (chấp nhận ratchet có ghi chú, hoặc thêm guard hẹp); nếu (B), báo controller trước Task 1.
- [ ] **Bước 3:** Không đổi source trong Task 0; ghi finding vào ledger SDD / task report.

---

### Task 1: Field secret của `TaskCommands` + plumbing log đã mask (không đổi hành vi)

**File:** sửa `commands.rs` (`TaskCommands` ~212, `Debug` thủ công ~358, `stream_line` ~719, `stream_replayed_line` ~748, vòng streaming `DockerRunner`, fake runner trong `#[cfg(test)]`); sửa `k8s/runner.rs` (vòng streaming ~355-380).

**Interfaces — Produces:**
- `TaskCommands { … , pub secret_env: BTreeMap<String,String>, pub secret_masks: Vec<String> }` — `secret_env` = entry env có secret (key→giá trị resolved) cho kênh an toàn; `secret_masks` = mọi giá trị secret cần redact trong log (từ `Resolved.values`). Cả hai default rỗng.
- `stream_line(log: &RunLog, …)` và `stream_replayed_line(log: &RunLog, …)` — tham số đầu giờ là log handle, không phải `&TaskInput` (chúng chỉ dùng `task.log`).

- [ ] **Bước 1: Thêm field + Debug redact.** Thêm vào `TaskCommands` sau `env`:
```rust
    /// Secret-bearing env entries (key -> resolved value), injected through a
    /// SECURE channel by the runner (k8s Secret+secretKeyRef; docker --env-file),
    /// never inline in the pod spec / docker CLI. Package 10b.
    pub secret_env: BTreeMap<String, String>,
    /// Every secret value to redact from this task's stdout/stderr (from
    /// `runners::secrets::Resolved.values`). Package 10b.
    pub secret_masks: Vec<String>,
```
Cập nhật `Debug` thủ công (~358): in `secret_env` chỉ tên key, không bao giờ in giá trị `secret_masks`:
```rust
            .field("env", &self.env)
            .field("secret_env", &self.secret_env.keys().collect::<Vec<_>>())
            .field("secret_masks", &format_args!("<{} values>", self.secret_masks.len()))
```
- [ ] **Bước 2: Refactor stream fns nhận `log: &RunLog`.** Trong `stream_line`/`stream_replayed_line` thay tham số `task: &TaskInput` bằng `log: &crate::logs::RunLog`, thay mọi `task.log` bên trong bằng `log`.
- [ ] **Bước 3: Hai runner build log đã mask và truyền vào.** Ngay trước vòng streaming trong `DockerRunner::run` và `KubernetesRunner::run`:
```rust
let log = task.log.masked(&cmds.secret_masks); // empty masks == identity
```
truyền `&log` cho mọi `stream_line`/`stream_replayed_line`; mọi `task.log.line(...)` trong đường streaming (vd WARN stderr-not-separable của k8s ~369) cũng ghi qua `log`. (`masked(&[])` trả clone cùng tập mask → no-op khi không có secret.)
- [ ] **Bước 4: Sửa call site trong test fake** để truyền `&task.log` (hoặc handle đã mask); test mask ở commands.rs ~4002 vẫn pass.
- [ ] **Bước 5: Build + test (foreground):**
```bash
cargo test -p egret-workflow --test routes --test execution_timestamps
cargo test -p egret-workflow --lib worker::commands
```
Kỳ vọng PASS — task thuần plumbing, chưa flow nào populate được field mới.
- [ ] **Bước 6: Commit**
```bash
git add crates/egret-workflow/src/worker/commands.rs crates/egret-workflow/src/worker/k8s/runner.rs
git commit -m "feat(workflow): TaskCommands secret_env/secret_masks + masked-log plumbing"
```

---

### Task 2: Seam resolution của handler (`render()` sync → resolve async trong `run()`) + masking

**File:** sửa `script.rs` (`render()` ~247, struct `Rendered`, `run()` ~621); test `tests/env_secret.rs` (mới).

**Interfaces — Consumes:** `TaskCommands::{secret_env, secret_masks}`; `runners::secrets::{render_with_secrets, current, text_uses_secret, Resolved}` (`render_with_secrets(props: &Map<String,Value>, cx: &RenderContext, workspace: Uuid, source: &dyn SecretSource) -> Result<(Map<String,Value>, Resolved), String>`; `current() -> Option<Arc<dyn SecretSource>>`; `text_uses_secret(&str) -> bool`; `Resolved.values: Vec<String>`). **Produces:** `cmds.env` (author env không-secret + python defaults, inline), `cmds.secret_env`, `cmds.secret_masks`.

**Ghi chú thiết kế:** `render()` **đồng bộ** và render author `env` qua `render_values`→`render_str` (engine confined refuse `secret()`). Nên render author-env phải rời `render()` sang `run()` async. `render()` vẫn build context `cx` (với `WORKING_DIR_VAR`/`OUTPUT_DIR_VAR`) và mọi non-env prop; thôi nạp author env vào `cmds.env` (python `ENV_DEFAULTS` giữ lại). `run()` resolve `parsed.env` với `cx` được trả về.

- [ ] **Bước 1: Viết integration test fail.** Tạo `tests/env_secret.rs`, theo mẫu test secret 10a (vd `tests/log_secret.rs`) để cài fake `SecretSource` + fake runner echo env. Assert: env `{TOK: "{{ secret('API') }}", PLAIN: "hi"}` → TOK resolve vào secret_env; PLAIN ở cmds.env (inline); giá trị secret bị MASK trong stdout khi script echo `$TOK`; secret_masks chứa giá trị resolved. (Tái dùng harness fake-SecretSource + capture-log của 10a nếu có.)
- [ ] **Bước 2: Chạy — kỳ vọng FAIL** (hiện env secret() bị refuse):
```bash
cargo test -p egret-workflow --test env_secret --test routes --test execution_timestamps
```
- [ ] **Bước 3: `render()` — thôi render author env; trả `cx`.** Xoá `let author_env = render_values(type_id, "env", &p.env, &cx)?;` (~271) và `env.extend(author_env);` (~334). Giữ build `env` từ `ENV_DEFAULTS` cho python. Thêm `cx` vào struct `Rendered`:
```rust
struct Rendered { cmds: TaskCommands, ns_inputs: …, namespace_files: …, cx: RenderContext }
```
- [ ] **Bước 4: `run()` — resolve_env async sau render(), trước gate.** Ngay sau match bind `cmds`, `cx`:
```rust
// Package 10b: resolve the author env with secret() support, splitting the
// secret-bearing entries into the secure channel and collecting the mask set.
match resolve_env(task, &parsed.env, &cx).await {
    Ok((plain, secret_env, masks)) => {
        cmds.env.extend(plain);            // non-secret entries inline, as before
        cmds.secret_env = secret_env;
        cmds.secret_masks = masks;
    }
    Err(why) => return Ok(Outcome::failed_with(why)),
}
```
Thêm helper (cùng file) — xem code đầy đủ ở bản EN (`resolve_env`): source `Some` → `render_with_secrets` + phân loại per-key qua `text_uses_secret(raw)` → secret_env vs plain, trả `resolved.values`; source `None` → `render_values` confined (đường hôm nay; secret() lỗi ở đây nhưng đã refuse lúc save).
- [ ] **Bước 5: Chạy test — kỳ vọng PASS.**
- [ ] **Bước 6: Commit**
```bash
git add crates/egret-workflow/src/worker/handlers/script.rs crates/egret-workflow/tests/env_secret.rs
git commit -m "feat(workflow): resolve secret() in script env (handler seam + masking)"
```

---

### Task 3: Kênh an toàn k8s — Secret object + secretKeyRef + ownerReference GC

**File:** sửa `k8s/pod_spec.rs` (env block ~317-333); sửa `k8s/runner.rs` (tạo Secret trước/cùng Pod; lỗi RBAC); test unit trong `pod_spec.rs`. **Consumes:** `cmds.secret_env`.

- [ ] **Bước 1: Viết unit test fail** `secret_env_emits_secret_key_ref_not_inline` — xem code EN: PLAIN giữ `value` inline, `value_from` None; TOK có `secret_key_ref` (key="TOK", optional Some(false)), `value` None; chuỗi "shh" KHÔNG xuất hiện trong danh sách EnvVar.
- [ ] **Bước 2: Chạy — FAIL.** `cargo test -p egret-workflow --lib worker::k8s::pod_spec`
- [ ] **Bước 3: Phát secretKeyRef cho key secret_env** (pod_spec ~317-333) — xem code EN (`EnvVar { value: None, value_from: Some(EnvVarSource { secret_key_ref: Some(SecretKeySelector{ name, key, optional: Some(false) }) }) }`). Luồn `secret_name` per-attempt (deterministic theo (run, attempt) để re-attach tính lại cùng tên).
- [ ] **Bước 4: Tạo Secret với ownerReference tới Pod.** Trong `KubernetesRunner::run`, khi `!cmds.secret_env.is_empty()` và không re-attach: tạo `Secret` trong namespace pod từ `cmds.secret_env`, `ownerReferences` trỏ Pod (`controller: Some(false)`, `block_owner_deletion: Some(false)`) — tạo Pod trước lấy `uid` rồi tạo Secret với owner đó (hoặc tạo Secret, tạo Pod, patch ownerReference). Re-attach: tên deterministic nên Secret đã tồn tại, không tạo lại. Lỗi create/RBAC → fail task với message rõ nêu namespace + verb thiếu.
- [ ] **Bước 5: Chạy unit test — PASS**; chạy suite k8s nếu có (có thể skip khi không có cluster — ghi skip vào report).
- [ ] **Bước 6: Commit**
```bash
git add crates/egret-workflow/src/worker/k8s/pod_spec.rs crates/egret-workflow/src/worker/k8s/runner.rs
git commit -m "feat(workflow): k8s secret() env via Secret object + secretKeyRef (owner GC)"
```

---

### Task 4: Kênh docker — `--env-file` + unlink

**File:** sửa `commands.rs` (`DockerRunner` build env của `docker create` ~3015-3024); test unit trong `commands.rs`. **Consumes:** `cmds.secret_env`.

- [ ] **Bước 1: Viết unit test fail** — tách phần build env-arg thành helper test được nếu chưa; assert: env không-secret → `--env KEY=VALUE`; env secret → `--env-file <path>` (không `--env TOK=…`); nội dung env-file `TOK=shh\n` mode `0600`; giá trị secret KHÔNG xuất hiện trong argv `docker create`.
- [ ] **Bước 2: Chạy — FAIL.**
- [ ] **Bước 3: Implement kênh env-file** (~3015): giữ `cmds.env` không-secret là `--env KEY=VALUE`. Khi `!cmds.secret_env.is_empty()`: ghi file tạm (scratch dir hoặc `temp_dir()`), mode `0600` (`OpenOptionsExt::mode(0o600)`), dòng `KEY=VALUE\n`; thêm `--env-file <path>`; sau `docker create` trả về `remove_file(path)` (debug log nếu lỗi, không fail task). Bảo đảm xoá file cả trên đường lỗi (RAII guard hoặc xoá trước mọi early return).
- [ ] **Bước 4: Chạy unit test — PASS.**
- [ ] **Bước 5: Commit**
```bash
git add crates/egret-workflow/src/worker/commands.rs
git commit -m "feat(workflow): docker secret() env via --env-file (0600, unlinked after create)"
```

---

### Task 5: Gỡ validation lúc save + message refusal + test gate

**File:** sửa `validate.rs` (`Walk` ~607, block `is_log` ~683, chuỗi allow-arm ~702, constructor walk ~790); sửa `functions.rs` (`SECRET_REFUSAL` ~325); test `tests/plugins_gate.rs`.

- [ ] **Bước 1: Viết gate test fail** trong `plugins_gate.rs`: (a) env secret() giờ SAVE được; (b) commands secret() vẫn refuse; (c) script secret() vẫn refuse; (d) inputFiles secret() vẫn refuse; (e) env lifted vẫn cần `workflows.use_secrets`.
- [ ] **Bước 2: Chạy — FAIL** (env đang refuse).
- [ ] **Bước 3: Thêm tập `script` + `is_script`.** Trong `Walk` (~607) thêm `script: HashSet<(&'a str, &'a str)>`. Constructor (~790) populate như `log` (filter task mà `known_type(&t.r#type)` resolve về Script handler; nếu registry có `TYPE_ID` canonical của Script thì filter theo đó như filter `log::TYPE_ID` ~786; nếu không, match `t.r#type` với 6 FQCN: `egret.container`, `egret.script.python`, `io.kestra.plugin.scripts.python.{Commands,Script}`, `io.kestra.plugin.scripts.shell.{Commands,Script}`). Cạnh `is_log` (~683):
```rust
let is_script = matches!((id, ty), (Some(i), Some(t)) if self.script.contains(&(i, t)));
```
- [ ] **Bước 4: Thêm allow-arm env** (sau arm `is_log && k == "message"` ~709):
```rust
} else if is_script && k == "env" {
    // Package 10b: a script task's `env:` values resolve `secret()`
    // (resolved async in the handler, injected via a secure channel).
    // A `namespace=` / `full=` in the call is still refused, as elsewhere.
    let mut found = Vec::new();
    strings(v, here, &mut found);
    for (p, text) in found {
        self.property(p, text);
    }
}
```
- [ ] **Bước 5: Cập nhật `SECRET_REFUSAL`** (functions.rs ~325) — xem text EN: nêu thêm `env:` script task resolve được, và hướng dẫn author đặt secret vào `env:` rồi tham chiếu `$NAME`. Cập nhật doc comment ~320 cho khớp.
- [ ] **Bước 6: Chạy gate + validate test — PASS**
```bash
cargo test -p egret-workflow --test plugins_gate --test routes --test execution_timestamps
cargo test -p egret-workflow --lib spec::validate
```
- [ ] **Bước 7: Commit**
```bash
git add crates/egret-workflow/src/spec/validate.rs crates/egret-workflow/src/runners/functions.rs crates/egret-workflow/tests/plugins_gate.rs
git commit -m "feat(workflow): lift secret() in script env at save; keep other fields refused"
```

---

### Task 6: Docs (repo `egret-data/docs`)

**File** (checkout `/Users/1pro-l4r8/Documents/github/data-portal/docs`, branch mới `docs/workflow-secret-in-script-env`):
- `platform/workflows/flows/expressions.md` — "Where secret() works": thêm `env:` script task; nêu pattern `$NAME`; `:::caution` docker `inspect` còn thấy giá trị.
- Doc deployment/RBAC — engine k8s SA cần `create` Secret trong namespace task (thêm verb vào snippet RBAC, giải thích dùng cho inject secret() env).
- `known-issues.md` — phơi nhiễm dư docker `inspect` (nếu caution chưa phủ).

- [ ] **Bước 1:** Sửa như trên (song ngữ nếu docs repo song ngữ — cập nhật cả hai cây ngôn ngữ).
- [ ] **Bước 2:** Verify build docs local nếu có; nếu không dựa vào check `build` của docs PR.
- [ ] **Bước 3: Commit**
```bash
git add platform/workflows/flows/expressions.md <deployment-doc> <known-issues>
git commit -m "docs(workflow): secret() in script env — usage, docker caution, RBAC"
```

---

## Self-Review

- **Phủ spec:** D1 → Task 3/4; D2 env-only → Task 2 phân loại + Task 5 arm; D3 k8s → Task 3; D4 docker → Task 4; D5 masking → Task 1 + Task 2; D6 RBAC → Task 3 + Task 6; §4 outage fail-safe → nhánh `Err` Task 2; §4 replay → tên deterministic Task 3; §5 census → Task 0; tests → mọi task; docs → Task 6. Không gap.
- **Placeholder:** mọi bước code có code cụ thể; hai con trỏ research (tập membership script; tái dùng harness test 10a) nêu rõ file + chỗ mirror + fallback.
- **Nhất quán type:** `secret_env: BTreeMap<String,String>`, `secret_masks: Vec<String>` dùng y hệt Task 1–4; chữ ký `render_with_secrets`/`current`/`text_uses_secret`/`Resolved.values` khớp secrets.rs; `stream_line(log: &RunLog, …)` nhất quán.
