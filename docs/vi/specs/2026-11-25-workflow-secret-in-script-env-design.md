# Gói 10b — `secret()` trong env của script task — Thiết kế

**Trạng thái:** thiết kế đã duyệt, trước khi lập plan
**Ngày:** 2026-11-25
**Roadmap:** egret-workflow parity với Kestra, gói #10 ("secret() ở mọi nơi"), phần **10b**. Phần 10a (`secret()` ở các bề mặt render in-process: Log, runIf, inputs, triggers) đã merge PR #913 ngày 2026-11-25.

## 1. Mục tiêu

Cho `secret('NAME')` resolve được trong map `env:` của script task, để truyền secret
cho code user chạy trong container/pod — **inject an toàn ở nơi runtime cho phép** —
trong khi mọi field script task khác vẫn refuse `secret()`. Lần đầu tiên stdout/stderr
của script được mask giá trị secret.

Trong phạm vi (field duy nhất được gỡ): **giá trị** trong `tasks.*.env` của script task
(`io.kestra.plugin.scripts.{python,shell}.{Script,Commands}`, `egret.container`,
`egret.script.python`).

Ngoài phạm vi rõ ràng (vẫn refuse, cả save lẫn run):
`commands:`, `script:`, `beforeCommands:`, `interpreter:`, `inputFiles:`,
`outputFiles:`, `namespaceFiles:`, các field image/container. User tham chiếu secret
trong script qua biến môi trường (`$NAME`), không bao giờ inline `secret()` vào command.
Gói 10b là **env-only**.

## 2. Vì sao env-only + secure-where-possible

Sự thật đã đo (Kestra @ 96893b589; worktree egret):

- **Kestra không có kênh secret riêng.** `{{ secret('X') }}` trong `env:` chỉ là render
  Pebble bình thường thành env var; runner truyền plaintext vào container
  (`docker --env KEY=VALUE`) hoặc pod spec. Bảo mật **chỉ** dựa vào log masking (một tập
  `RunContextLogger.useSecrets` thống nhất, raw + base64, thay thế trong log appender).
  `env` không ghi ra disk; `inputFiles` thì có.
- **egret hiện cố ý refuse.** `secret()` là `Refusal::Confined` trong engine render confined;
  message refuse nêu lý do: trên script task env/commands đã render là field của pod spec,
  ai có `get pods` trong namespace task đều đọc được.

Cơ chế an toàn loại bỏ phơi nhiễm đó — một k8s Secret object tham chiếu qua `secretKeyRef`
— **chỉ dùng được cho env var**. Secret inline vào `commands:`/`script:` bị thế vào argv
và lọt vào `command`/`args` của pod spec + execution history; `inputFiles` lọt ra disk.
Cả hai không thể bảo mật. Do đó posture an toàn **ép env-only**: gỡ `env:`, giữ refuse
phần còn lại, bảo user tham chiếu `$NAME`.

**Quyết định (user đã duyệt):**

- **D1 — Posture:** secure-where-possible, nghiêm hơn Kestra ở k8s.
- **D2 — Phạm vi:** chỉ giá trị `env:`.
- **D3 — k8s:** inject các env entry chứa secret qua một k8s **Secret object** per-attempt
  + `secretKeyRef`; giá trị không có trong pod spec. Secret mang **ownerReference tới Pod**
  để k8s GC tự xoá khi pod bị xoá (không delete thủ công, không cần quyền delete).
- **D4 — Docker:** không có cơ chế an toàn tương đương; truyền env chứa secret qua
  **`--env-file`** (mode 0600, `unlink` ngay sau `docker create`) để giá trị không hiện
  trong `ps` của host. Giá trị vẫn còn trong `docker inspect` — phơi nhiễm dư có ghi chú
  (parity với Kestra ở docker).
- **D5 — Masking:** lần đầu nối `task.log.masked(&resolved.values)` vào script handler;
  mask stdout/stderr và dòng command echo.
- **D6 — RBAC:** service account k8s của engine phải có quyền `create` Secret trong
  namespace task. Chi phí vận hành đã chấp nhận của D3.

## 3. Kiến trúc

Một seam trong script handler, cộng một helper mới mỗi runner.

### 3.1 Resolution (dùng chung, `worker/handlers/script.rs`)

Hiện `Script::render` build env bằng `render_values(type_id, "env", &p.env, &cx)`
(container.rs → `render_str`, engine **confined** refuse `secret()`).

Thay render env bằng đường secret-aware của 10a `runners::secrets::render_with_secrets`
(đúng call mà `log.rs`/`http.rs` dùng). Nó:

- resolve `secret()` bằng nguồn secret hoạt động,
- trả về map đã render **và** `Resolved.values` — danh sách giá trị secret đã thấy, để mask.

Render env xảy ra **async trong handler run** (không trong txn Postgres ở settle-pass như
runIf/inputs của 10a). Nên resolution chỉ cần `await` fetch secret; fetch lỗi thì fail
attempt sạch sẽ. **Không cần machinery Prefetch** (cái đó chỉ cho các bề mặt sync-in-txn của 10a).

Phân loại per-entry: một env entry là **có secret** nếu template dùng `secret()`.
`render_with_secrets` áp dụng per value (hoặc giao tập resolved-values với từng value đã
render) để handler biết, theo từng key, có đưa vào kênh an toàn hay inline như cũ. Env
không-secret giữ nguyên đường inline cũ.

Handler truyền cho runner, trong `TaskCommands`:
- `env`: các entry không-secret (inline, như hiện tại), và
- một field mới `secret_env: BTreeMap<String,String>`: các entry có secret
  (key → giá trị đã resolve), để runner inject an toàn.

`resolved.values` đưa cho `task.log.masked(...)` (D5).

### 3.2 Runner k8s (`worker/k8s/`)

Với mỗi attempt mà `secret_env` khác rỗng:

1. **Tạo Secret** `egret-task-<exec8>-<task>-<attempt>` (tên suy ra, unique per attempt)
   trong namespace của pod, `data` = base64 từng giá trị `secret_env`, với
   `ownerReferences: [<Pod>]` (`controller: false`, `blockOwnerDeletion: false`).
2. **Pod spec** (`pod_spec.rs`): với mỗi key `secret_env` phát
   ```
   EnvVar { name: KEY, value: None,
            value_from: Some(EnvVarSource { secret_key_ref:
              Some(SecretKeySelector { name: <secret>, key: KEY, optional: Some(false) }) }) }
   ```
   thay cho `value` inline. Env không-secret giữ inline.
3. **Vòng đời:** tạo Secret, rồi tạo Pod với ownerReference trỏ về nó — chi tiết thứ tự
   chốt trong plan (tạo Pod trước để lấy UID rồi tạo Secret với UID đó làm owner; hoặc tạo
   Secret, tạo Pod, rồi patch ownerReference của Secret về UID Pod). Dù cách nào, khi pod
   bị xoá, GC gỡ Secret. Không có đường delete tường minh.
4. **RBAC lỗi** (không quyền create) → fail attempt với message rõ, nêu namespace và verb
   còn thiếu.

### 3.3 Runner docker (`worker/commands.rs`)

Với mỗi attempt mà `secret_env` khác rỗng:

1. Ghi **env-file** tạm (dòng `KEY=VALUE\n`) vào path riêng, mode `0600`.
2. Thêm `--env-file <path>` vào `docker create` (chỉ entry có secret; env không-secret
   tiếp tục dùng `--env KEY=VALUE`).
3. `unlink` env-file ngay sau khi `docker create` trả về (daemon đã đọc xong).
4. Giá trị vẫn lấy được qua `docker inspect` — phơi nhiễm dư có ghi chú.

### 3.4 Validation lúc save (`spec/validate.rs`)

Thêm một allow-arm trong walker `secret_outside_plugin_task`: với **script task**, **giá trị**
`env` resolve `secret()` (xử như arm `message` của Log). Mọi field script task khác vẫn vào
`Walk::outside` → `CODE_SECRET_OUTSIDE_PLUGIN_TASK`.

Sửa `SECRET_REFUSAL` (functions.rs): message hiện nói `secret()` bị refuse thẳng trên script
task. Đổi thành: `secret()` resolve trong `env:` của script task; để dùng secret trong command,
đặt nó vào `env:` và tham chiếu `$NAME` trong script. Vẫn nêu các field còn refuse.

## 4. Xử lý lỗi & ca biên

- **Secret-store outage khi render env:** resolve async của handler fail → **fail attempt**
  (fail-safe; không inject một phần/plaintext, không leak). Đơn giản hơn 10a vì không có txn
  mở để rollback.
- **Replay / retry:** re-resolve mỗi attempt; tên Secret k8s per-attempt; GC theo pod. Không
  tái dùng Secret cũ giữa các attempt.
- **env key đụng framework var** (`WORKING_DIR`, `OUTPUT_DIR`, …): hành vi không đổi — author
  env extend sau defaults; key author có secret override như key author vẫn thế.
- **env hỗn hợp** (vài secret, vài không): entry không-secret inline, entry secret qua kênh
  an toàn, trong cùng pod/container.
- **docker + không có secret_env / k8s + không có secret_env:** không đổi (không Secret
  object, không env-file).
- **Chỉ có hai runner** (docker, k8s); cả hai đều được xử.

## 5. Kiểm thử

- **Task 0 (đo trước, như 10a):** kiểm corpus test / census xem có flow nào đặt `secret()`
  trong `env` của script task và đang ở `VALIDATE_REFUSED` không. Gỡ sẽ flip sang valid và
  dịch tuple census/oracle. Chốt như 10a đã làm (giữ một refusal hẹp để census ổn định, hoặc
  chấp nhận ratchet có ghi chú) **trước khi** viết code.
- **`tests/env_secret.rs`** (integration): script task với `env: {TOK: "{{ secret('X') }}"}`
  resolve; script đọc được `$TOK`; giá trị bị mask trong log đã bắt kể cả khi script echo nó.
- **Unit k8s** (`pod_spec.rs`): key env có secret phát `value_from` secretKeyRef (không `value`
  inline); key không-secret giữ inline.
- **Unit docker** (`commands.rs`): env có secret đi qua `--env-file`; file bị `unlink` sau
  create; env không-secret giữ `--env`.
- **`tests/plugins_gate.rs`:** `env:` script với `secret()` giờ **save được**; `secret()` trong
  `commands:`/`script:`/`inputFiles:` vẫn **refuse lúc save**; vẫn cần `workflows.use_secrets`
  để save.
- **Flip save-validation:** ca script-env từng bị refuse giờ validate; các ca còn refuse giữ finding.
- **Chạy test scoped PHẢI kèm** `--test routes --test execution_timestamps` (luật parity CI).
- **Live verify trên kind** (live check beats review): chạy script task có `secret()` trong `env`;
  xác nhận `kubectl get pod <p> -o yaml` thấy `secretKeyRef` (không phải value), Secret tồn tại
  và do pod sở hữu, pod/Secret bị GC cùng nhau, và log bị mask.

## 6. Tài liệu

- `platform/workflows/flows/expressions.md` — "Where secret() works": thêm `env:` script; nêu
  pattern `$NAME`; `:::caution` phơi nhiễm dư `docker inspect`.
- Docs deployment — service account k8s của engine cần quyền `create` Secret trong namespace
  task (đoạn RBAC).
- Known-issues — giới hạn dư còn lại (phơi nhiễm `docker inspect`) nếu caution chưa phủ hết.

## 7. Ràng buộc toàn cục

- Chat tiếng Việt; code/commit/docs tiếng Anh.
- Conventional Commits; `Closes #N` chỉ trong body PR.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`;
  body PR kết bằng dòng Claude Code generation.
- Không bao giờ commit secret hay file scratch. Stage theo path rõ; không `git add -A`;
  không stage `target*/`.
- MỘT build Rust nặng tại một thời điểm; build chạy foreground trong subagent.
- Merge khi CI xanh, không `--admin`, `cargo fmt --all --check` trước.
- Chạy test scoped `egret-workflow` kèm `--test routes --test execution_timestamps`.
