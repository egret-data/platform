# secret() ở các render surface — thiết kế (gói 10a)

> **Cập nhật sau Task 0 (2026-11-25):** phép đo Kestra đã sửa vài tiền đề: masking là per-attempt (không execution-wide), D3 = SEAL (nghiêm hơn Kestra, user chốt), lift = allow-list trên walker, triggers chỉ gồm Webhook `inputs` + Flow-trigger `when`/`inputs` (giữ `key` refused → "banks 0" phụ thuộc điều này), runIf/inputs cần pre-fetch. Chi tiết trong §6 (đã điền) và bản EN; §6 là authority.

**Roadmap:** gói **#10 "secret() everywhere"** tách thành **10a** (gói này: `secret()` ở các render
surface in-process — Log, `runIf`, inputs, triggers — resolve và mask) và **10b** (`secret()` trong
script env, inject an toàn vào container — spec riêng). #10 hoàn tất khi cả hai merge. Ba task http đã
resolve `secret()` (kéo lên từ gói #4).

**Phụ thuộc:** máy móc secret đã có — `runners::secrets::render_with_secrets` (collect→fetch→resolve,
3-pass, ghi `Resolved::values`), `logs::mask`, `redactions` toàn-execution của
`runners::env::RenderContext`, và `secret_inputs` (seal cho input kiểu SECRET). 10a tái dùng tất cả;
không phát minh primitive masking mới.

## 1. Mục tiêu

Gỡ confinement đang chỉ resolve `secret()` ở property của task `kestra.*` và ba task http, để một lời
gọi `secret()` trong **Log** `message:`, trong **`runIf:`** của task, trong **input** value/default
của flow, và trong **trigger** (điều kiện/`labels` của Schedule/Webhook) resolve ra giá trị thật tại
lúc render và chạy như Kestra — đồng thời mọi giá trị nó trao ra được mask ở MỌI dòng log lưu lại và
trong message của một run fail, đúng như giá trị của một SECRET input đã được mask hôm nay. Đây là nửa
"thay confinement của Appendix B §8 item 3 bằng reach của Kestra" của roadmap #10 chạy in-process; nửa
script-env là 10b.

## 2. Hiện trạng — confinement chỉ là một resolver được cài

`runners::env::environment` cài một `secret` stub **từ-chối** (row `REFUSED` trong `functions`, với lý
do pod-spec). Mọi surface render đè lên nó, nên `{{ secret('X') }}` là lỗi ở Log, `runIf`, inputs và
triggers. Chỉ `runners::secrets::render_with_secrets` cài một `secret` **hoạt động**, theo từng render,
đè lên một bản copy của environment (`env::render_once_with`), và chỉ đường property của task `kestra.*`
(`worker::handlers::kestra`) cùng các http handler gọi nó. `Resolved::values` ghi mọi giá trị trao ra;
caller mask log của attempt đó bằng `logs::mask`.

Kênh masking toàn-execution đã tồn tại và load-bearing cho SECRET inputs: `RenderContext::redact(values)`
nạp tập redaction được áp vào **mọi dòng log một run ghi ra** và vào message của run fail (qua
`RenderContext::redactions()` + `logs::mask`). `secret_inputs::split` thay giá trị input `type: SECRET`
bằng `MASK`, seal plaintext (AES-256-GCM dưới `secrets.master_key`) vào `executions.secret_inputs`, và
`reveal` (ở `worker::render::Surface::load`) mở nó ra để `{{ inputs.x }}` render giá trị thật — rồi giá
trị đó được thêm vào redactions (`worker::render` gọi `cx.redact(&self.secrets)`).

`secret_inputs.rs` ghi rõ posture bao quanh 10a: **giá trị của hàm `secret()` KHÔNG được mask trong
`outputs` của task** (W5b §12, như Kestra) — tác giả chủ ý publish nó. Nên surface KV-value và
output-`value:` giữ nguyên dạng từ-chối / không-mask như một security divergence (§3.1); 10a không đụng.

Vậy confinement không phải một subsystem riêng phải xây — nó là `render_with_secrets` được nối vào bốn
đường render nữa, mỗi đường rồi nạp `Resolved::values` của nó vào `redactions` của execution.

## 3. Đo Kestra — Task 0, load-bearing

Gói này bank **0 corpus flow**, nên **không có coupling corpus-ratchet** (khác gói 9a và 9b): census
tuple, các hằng `LIVE`/`VALIDATE_REFUSED` của driver, và bảng `PublishedButRefused` không bị 10a dịch
chuyển. Phép đo trên source Kestra (pin `96893b589`, 2.0.0-SNAPSHOT) và feature surface, không phải
golden. Task 0 đo và ghi vào §6, và không test nào assert terminal cho tới khi egret tái hiện cái đo
được:

- **M1 — masking reach.** Kestra có mask giá trị `secret()` ở MỌI dòng log của execution, hay chỉ ở task
  resolve nó? (redactions của egret là toàn-execution; xác nhận khớp.)
- **M2 — posture inputs (quyết định mở).** Khi một input value/default của flow render ra một `secret()`,
  Kestra có lưu plaintext vào inputs của execution không, hay giữ nó ra ngoài (mask/seal)? Điều này
  quyết 10a có route giá trị đó qua `secret_inputs::split` (seal, lựa chọn parity an toàn) hay chỉ thêm
  vào `redactions`. **Quyết định hoãn tới phép đo này** (user, 2026-11-25).
- **M3 — thời điểm render trigger.** `secret()` của trigger Schedule/Webhook resolve lúc nào (eval
  trigger vs. start execution), và giá trị có bị mask trong log của chính trigger không?
- **M4 — runIf.** Xác nhận `secret()` trong `runIf` resolve ra boolean và không bao giờ lộ giá trị (chỉ
  có thể tới log qua một error message, điều `redactions` phủ).

## 4. Quyết định

- **D1 — tái dùng, không phát minh.** 10a cài `render_with_secrets` (hoặc cùng seam resolver đó) trên
  đường render của Log, `runIf`, input và trigger; thêm mỗi `Resolved::values` vào redactions của
  `RenderContext` của execution. Không primitive masking mới, không migration, không API, không field
  model.
- **D2 — masking là toàn-execution và bắt buộc.** Giá trị một `secret()` trao ra ở bất kỳ surface 10a
  nào được mask ở mọi dòng log lưu lại và trong message của run fail, qua redactions — cùng kênh SECRET
  inputs dùng. Một surface không được lift cho tới khi giá trị của nó vào redactions.
- **D3 — posture seal của inputs quyết bởi M2.** Nếu Kestra giữ plaintext `secret()` của input ra khỏi
  execution lưu trữ, 10a seal nó qua `secret_inputs` (tái dùng `split`/`reveal`); nếu không, chỉ thêm
  giá trị vào redactions. Ghi vào §6 sau Task 0.
- **D4 — KV value và output `value:` giữ từ-chối/không-mask (§3.1).** Ngoài phạm vi; từ-chối của chúng
  KHÔNG bị lift. Đây là security divergence chủ ý mà `secret_inputs.rs` đã ghi.
- **D5 — gate `WF_USE_SECRETS` không đổi.** `routes::source_uses_secret` đã từ chối publish/start bởi
  caller thiếu `workflow:use_secrets` mỗi khi source có thể gọi `secret()`. Mở rộng NƠI `secret()`
  *resolve* không mở rộng NƠI nó được *phát hiện*; gate đã phủ mọi surface vì nó quét text của source.
  Xác nhận không surface 10a nào thoát `source_uses_secret`.
- **D6 — lift cuối cùng, sau khi hành vi đã đúng, grep mọi cách mã hoá.** Bài học lift-refusal cắn gói
  9a hai lần và 9b một lần (lần 9b là một fixture YAML hardcode trong danh sách "still refused" của một
  test mà cả grep theo code lẫn grep theo message đều không bắt): trước khi gỡ một từ-chối, grep **code**
  của nó, **message text** của nó, VÀ mọi **fixture YAML** assert nó, khắp `src/ tests/ oracle-exec/`.
  Grep là thẩm quyền; §5 là bản đồ khởi đầu, không thay thế.

## 5. Blast radius của lift (grep là thẩm quyền)

Mọi chỗ mã hoá "secret() bị từ chối ở đây" cho một surface 10a phải đổi khi surface đó được lift. Các
điểm khởi đầu đã biết (grep của Task 0 xác nhận tập đầy đủ trên worktree):

- `src/spec/validate.rs` — các arm secret-refused cho Log/runIf/inputs/triggers + unit test của chúng +
  ghi chú header module.
- `src/runners/secrets.rs` — prose header ("script tasks, inputs, `runIf`, `Log` and triggers all render
  against [stub từ-chối]") và wiring row `REFUSED` cho các surface này.
- `src/plugins/mod.rs` — từ-chối placement của `secret()` (giữ phần vẫn áp cho surface ngoài-10a; chỉ
  lift phần 10a).
- `src/eval.rs` — endpoint eval giữ `secret()` sealed (nó là read surface, không phải execution); xác
  nhận 10a không đổi seal của eval.
- Mọi test key-theo-fixture (routes/publish/worker) dùng một flow `secret()` Log/runIf/input/trigger làm
  ca "must be refused" — analog file thứ 6/7 của 9a/9b. Grep theo fixture, không chỉ code.

## 6. Khác biệt / sự thật đo được (Task 0, đo ngày 2026-11-25)

Nguồn: checkout Kestra `data-portal/kestra` @ `96893b589`, đọc (không chạy). Đường dẫn nằm dưới
`core/src/main/java/io/kestra/core/` trừ khi có tiền tố khác.

### 6.1 M1 — phạm vi masking: theo từng logger, KHÔNG phải toàn execution

`secret()` gọi `SecretFunction.consumeSecret` (`runners/pebble/functions/SecretFunction.java:121-128`),
tức biến `addSecretConsumer` (`RunVariables.SECRET_CONSUMER_VARIABLE_NAME`, `runners/RunVariables.java:30`).
Consumer đó là `RunContextLogger::usedSecret` (`runners/RunContextLogger.java:187-192`; nối tại
`RunVariables.java:567-569` và `RunContextInitializer.java:144,238,265,280-281`). `usedSecret` thêm
plaintext **và base64 của nó** vào danh sách `useSecrets` riêng của logger; mọi appender thay bằng
`******` trên các dòng log của chính logger đó (`BaseAppender.replaceSecret`,
`RunContextLogger.java:376-385`).

Vậy masking là **theo từng `RunContextLogger`** (mỗi task run / trigger một cái), không theo execution.
Secret được resolve ở task A bị mask trong log của A (task con của dynamic taskrun kế thừa danh sách cha,
`RunContextLogger.java:296`), nhưng task B sau đó không resolve nó thì không mask.
**Khác biệt của egret (chặt hơn, chấp nhận):** tập mask của egret áp lên mọi dòng của Surface mang nó,
nhưng giá trị một render resolve lúc chạy không được lưu bền ở đâu cả — nguồn bền duy nhất là
`executions.secret_inputs` / `secret_outputs`. "Toàn execution" của D2 vì thế nghĩa là "mọi dòng của
attempt/render đã resolve nó"; bền vững toàn execution chỉ có với input đã seal (D3). Mỗi surface được
lift phải mask log của attempt của nó VÀ message lỗi của nó bằng các giá trị nó đã resolve.

### 6.2 M2 — posture của inputs: Kestra LƯU plaintext (kiểu không phải SECRET)

`FlowInputOutput.resolveInputValue` (`runners/FlowInputOutput.java:305-408`) render biểu thức `defaults`
bằng run context của execution (`resolveDefaultValue`, `:372-381`, `:413-427`; comment ở đó nhắc
`secret()`). Giá trị đã render đi qua `parseType` (`:538-552`): `SELECT/STRING/EMAIL -> current.toString()`
được lưu **plaintext** trong inputs của execution; chỉ `type: SECRET` được mã hóa
(`EncryptionService.encrypt` -> `EncryptedString`, `:542-550`). Consumer chỉ mask trong logger của run
context đã render nó (`RunContextFactory.of(flow, execution, ...)`, `runners/RunContextFactory.java:113-139`).

**QUYẾT ĐỊNH D3: SEAL, qua `secret_inputs::split`** — một khác biệt có chủ đích, chặt hơn Kestra. Lý do:
(a) hành vi của Kestra là "plaintext trong execution đã lưu", ở egret nghĩa là secret nằm rõ trong
`workflow.executions.inputs` và trong body API — đúng loại rò rỉ `secret_inputs` sinh ra để ngăn;
(b) seal không đổi gì với render (`Surface::load` reveal và redact, `worker/render.rs:230`);
(c) phương án "chỉ redact" sửa log nhưng không sửa dòng đã lưu. **Hai điều Task 3 phải mang theo:**
1. egret không bao giờ render default dạng template lúc start: `inputs_check` không lưu gì cho nó
   (`inputs_check.rs:359-371`) và `worker::render::with_declared_defaults` (`render.rs:1036-1082`) chèn
   **nguồn template thô**, hiện chỉ resolve được vì `render_str` render lại output của chính nó (nhiều
   pass). `render_with_secrets` theo phán quyết 1A là **một pass**, nên `{{ inputs.x }}` có default
   `{{ secret('X') }}` sẽ KHÔNG resolve qua nó. Default dạng template phải được render bằng resolver
   **lúc start** rồi lưu (đã seal) — như Kestra.
2. `secret_inputs::split` chọn id chỉ theo `type: SECRET` đã khai báo (`secret_inputs.rs:68-80`, qua
   `secret_ids`). Để seal một input STRING đã resolve secret, `split` (hoặc hàm anh em) phải nhận tập id.

### 6.3 M3 — thời điểm của trigger

Schedule: run context của trigger là `runContextFactory.of(flow, trigger)` trong scheduler
(`scheduler/.../TriggerScheduler.java:179,203`), có `RunContextLogger` riêng (`RunContextFactory.java:176-193`;
consumer nối bởi `RunVariables.build` `:567`). `when` render theo từng tick ứng viên lúc đánh giá trigger
(`plugin/core/trigger/Schedule.java:372-381, 548-559`); `inputs` và `labels` render khi dựng execution, vẫn
lúc đánh giá, trước khi execution tồn tại (`plugin/core/trigger/SchedulableExecutionFactory.java:60-87,
91-108`). Webhook: **lúc có request**, `WebhookService.newExecution` (`services/WebhookService.java:147-178`)
render labels (`LabelService.fromTrigger`), `conditionService.isValid`, rồi
`runContext.render(trigger.getInputs())`; `key` render mỗi request trong `ExecutionController.findWebhook`
(`webserver/.../ExecutionController.java:889-890`). Giá trị chỉ được mask trong logger **của trigger**
(execution được tạo có logger riêng, không kế thừa danh sách). Input của trigger sau đó đi qua `readInputs`,
nên input đích kiểu SECRET được mã hóa còn STRING lưu plaintext (như M2).

### 6.4 M4 — runIf

`runIf` render ra chuỗi rồi `TruthUtils.isTruthy` (`utils/TruthUtils.java:5-14`: false với `false`, `0`,
`-0`, rỗng) — một boolean; giá trị không bao giờ lộ ra (`executor/.../ExecutorService.java:1298,1377`;
`executor/.../handler/ExecutionEventMessageHandler.java:269`; `worker/.../WorkerTaskProcessor.java:138`).
Đường duy nhất ra text là message lỗi, ghi trên logger của task (`ExecutionEventMessageHandler.java:317-318`,
`WorkerTaskProcessor.java:153`), logger đó mask. runIf phía executor dùng
`RunContextFactory.of(flow, task, execution, taskRun)` có logger nên consumer đã được nối.

### 6.5 egret hiện có gì ở từng surface (seam; worktree `05f737230`)

- **Resolver:** `runners::secrets::render_with_secrets(props: &Map<String, Value>, cx: &RenderContext,
  workspace: Uuid, source: &dyn SecretSource) -> Result<(Map<String, Value>, Resolved), String>`
  (`secrets.rs:141`, async, một pass, lỗi đã được mask). **`Resolved.values: Vec<String>` là field
  public** (`:123-125`) — không có hàm accessor. Kênh redact: `RenderContext::redact(&mut self, &[String])` /
  `redactions()` (`runners/env.rs:413-420`).
- **Log:** `worker::handlers::log::Log::run` (`worker/handlers/log.rs:91`, `render_str(text, &task.render)`);
  `Log` không giữ `SecretSource` (handler kestra/http giữ `rt.secrets`, `kestra.rs:204`, `http.rs:200`),
  log đã mask dựng từ `task.render.redactions()` (`worker/claim.rs:272`).
- **runIf:** `worker::render::render_run_if` (`worker/render.rs:1763`), `crate::render_str` **đồng bộ**,
  gọi từ `claim.rs:5605` trong một pass settle — render đồng bộ không thể await việc lấy secret, nên surface
  cần pre-fetch (tách collect/fetch/resolve) trước pass. Runif lỗi thì im lặng (không message) trừ khi vượt
  giới hạn đọc (`failed_on_a_limit`).
- **Inputs lúc start:** `worker::claim::checked_inputs` / `checked_inputs_for` / `input_renderer`
  (`claim.rs:4279-4330`) -> `inputs_check::resolve` (trait `InputRenderer`, `inputs_check.rs:137`): render
  `dependsOn.condition`, `expression` của SELECT/MULTISELECT, và default dạng template chỉ khi có input
  khác phụ thuộc. Đồng bộ, không có nguồn secret; `start_execution_in_on` (`claim.rs:4347`). Dạng lưu:
  `secret_inputs::split` lúc start (`secret_inputs.rs:80`).
- **Triggers:** egret KHÔNG có `when` của Schedule (bị từ chối: `ScheduleError::UnsupportedWhenCondition`,
  `trigger/schedule.rs:1277-1300`, roadmap #11) và từ chối giá trị `inputs` dạng template của Schedule lúc
  fire (`scheduler/fire.rs:595-606` `refuse_templated`; cảnh báo lúc save KI-122). Các seam render thật hiện
  nay: `inputs` của Webhook (`trigger::webhook::render_inputs`, `webhook.rs:1007`, gọi ở
  `webhook_routes.rs:645`; đồng bộ, không nguồn) và `key` của Webhook (`render_key`, `webhook.rs:540`),
  `when`/`inputs` của Flow-trigger (`flow_trigger/process.rs:955 render_when`, `start_fire` `:996`).
  Không có render `conditions` của Webhook; `labels` của trigger là field model (`models/trigger.rs:51`),
  không tìm thấy seam render. **Surface "trigger" của 10a hẹp hơn "condition và labels của
  Schedule/Webhook" trong spec: xem §6.7.**

### 6.6 Bản đồ lift (grep là thẩm quyền; code + message + fixture YAML)

CODE từ chối `secret_outside_plugin_task` (`plugins/mod.rs:177`), phát ra bởi `spec/validate.rs:614-621`
(`Walk::outside`, từ chối `secret()` trong MỌI chuỗi không phải property của task `kestra.*`/http — nên mỗi
lần lift phải là allow-list theo đường dẫn, không phải xóa bỏ; `Return.format`, `value:` của outputs,
`value` của Set/Put, `key`, commands của script phải vẫn bị từ chối). MESSAGE từ chối:
`functions::SECRET_REFUSAL` (`runners/functions.rs:323-326`, chữ "resolves only on `kestra.*` ... no other
render (inputs, runIf, Log, triggers) resolves secrets" — sẽ thành sai và phải viết lại; mọi assertion bên
dưới trên chuỗi con đó đi theo). Đấu nối: `functions.rs:388`, re-export `runners/mod.rs:95`.

Chỗ mang code/message (src): `src/plugins/mod.rs:174-193`, `src/spec/validate.rs:541-707` (walker + header)
và test `:5881-6031` (**khẳng định `runIf` và `Log` cạnh task http bị từ chối: `:6013-6015`, `:6026-6027`**),
`:6816-6830` (outputs `value:` — giữ từ chối), `src/runners/secrets.rs:1-10` (header) và test
`:997, :1026, :1085, :1102`, `src/runners/functions.rs:3795` (theo hằng), `src/runners/nested.rs:55-67,925`
(giữ), `src/outputs.rs:516-535` (giữ), `src/worker/handlers/storage/items.rs:42,2059-2095` (giữ),
`src/storage/prefetch.rs:73` (giữ), `src/eval.rs:13-22,403-422` (seal của eval — giữ),
`src/secret_inputs.rs:37` (văn xuôi).

Test (tests/): `tests/plugins_gate.rs:1027-1110` (**`secret_outside_a_plugin_tasks_properties_is_refused_by_name`
— các case đúng nghĩa đen là "runIf của plugin task" (`tasks.q.runIf`), "defaults của input"
(`inputs.i.defaults`), "Log dưới errors của flowable" (`tasks.s.errors.e.message`), "một trigger"
(`triggers.w.key`), cộng case `commands` của script; khẳng định message "resolves only on"**),
`:1133-1135,1158-1170` (giữ), `:813-1000, 1192` (gate WF_USE_SECRETS — giữ, D5), `tests/publish.rs:917-960`
(giá trị KV; assertion phủ định trên chuỗi con message), `tests/executions_routes.rs:4090-4302` (seal của
eval — giữ).

Fixture YAML (oracle-exec): `oracle-exec/corpus/valids/secrets.yaml` (Return `format:` x4 + một Log
`message:`; vẫn bị từ chối sau 10a vì `Return.format`) và
`oracle-exec/corpus/valids/webhook-secret-key.yaml` (`key:` của Webhook). Cả hai được nêu trong
`oracle-exec/driver.rs:743-751` (ghi chú census "245") và nằm trong `VALIDATE_REFUSED`
(`driver.rs:957-960`, "22"); còn `w5b-plugins.json`, `w9a-storage.json` (chỉ `secret()` ở plugin task;
không đụng). **CẢNH BÁO DÍNH CORPUS:** "banks 0 flow corpus" của spec chỉ đúng nếu lift trigger vẫn để
`key` của Webhook bị từ chối. Lift cho phép `secret()` ở bất kỳ đâu dưới `triggers:` sẽ đưa
`valids/webhook-secret-key` ra khỏi `VALIDATE_REFUSED` và làm census đỏ. Quyết định: giữ `triggers.*.key`
bị từ chối ở 10a (xem §6.7).

`source_uses_secret` (`runners/secrets.rs:333-348`) duyệt **mọi** chuỗi lá YAML và key mapping của cả
source (alias được giải, escape trong dấu nháy kép được decode), nên phủ Log, runIf, inputs và triggers:
D5 ĐƯỢC XÁC NHẬN, không surface nào thoát. Nó chặn create/update/publish (`routes.rs:697,824,1159`); start
không bị chặn riêng (source đã bị chặn lúc lưu).

### 6.7 Khác biệt và lo ngại ghi lại cho các task sau

1. Kestra mask theo logger; egret mask theo render/attempt đã resolve (+ input đã seal).
2. Inputs: seal (chặt hơn plaintext-cho-kiểu-không-SECRET của Kestra); cần render-lúc-start cho default
   dạng template và `split` nhận tập id (§6.2).
3. Triggers: chỉ `inputs` của Webhook và `when`/`inputs` của Flow-trigger có seam render; `when` của
   Schedule (#11) và `inputs` dạng template của Schedule bị từ chối hẳn và KHÔNG thuộc 10a. `key` của
   Webhook giữ bị từ chối (dính corpus, và là credential so sánh mỗi request).
4. runIf đồng bộ trong pass settle: Task 2 cần pre-fetch trước pass.

## 7. Phạm vi

**Trong:** resolve `secret()` ở Log `message:`, `runIf:` của task, input value/default của flow, và
trigger Schedule/Webhook; mask mọi giá trị trao ra toàn-execution qua redactions; lift các từ-chối tương
ứng; posture seal inputs theo M2; test; docs.

**Ngoài:** `secret()` trong **env** của script/container (→ 10b); runner Kubernetes (→ #17); `secret()`
trong **KV value** và **output `value:`** của task (§3.1 security divergence, không đổi); seal của
endpoint eval (không đổi); mọi *source* hay *store* secret mới (store secret của workspace không đổi).

## 8. Kiểm thử

- **Theo từng surface** (Log, runIf, inputs, triggers): một flow mà `secret('X')` resolve ở đó chạy
  được, và giá trị secret **không** xuất hiện ở bất kỳ dòng log lưu lại nào hay trong execution lưu trữ
  (history/inputs). **Mutation:** vô hiệu kênh redaction cho surface đó phải làm test đỏ (assertion là
  vắng-mặt-plaintext, điều một masker bị tắt sẽ fail).
- **Control giữ từ-chối:** `secret()` trong một KV value và trong một output `value:` vẫn
  từ-chối/không-mask (D4).
- **Gate:** một flow gọi `secret()` ở bất kỳ surface 10a nào vẫn bị từ chối với caller thiếu
  `workflow:use_secrets` (D5).
- **Scoped run phải gồm `--test routes --test execution_timestamps`** (luật dự án) và `--test executor`
  để xác nhận census tuple KHÔNG đổi (banks 0).

## 9. Rủi ro

Thấp–trung bình. Máy móc (resolver + masker toàn-execution + seal input) đã tồn tại và được chứng minh
cho SECRET inputs và `secret()` của plugin task; 10a nối nó vào bốn đường nữa. Rủi ro thật là (a) một
surface bị lift trước khi giá trị của nó vào redactions (D2 chặn điều này — test trên vắng-mặt-plaintext
kèm mutation), và (b) một cách mã hoá từ-chối bị sót lúc lift (D6 — grep mọi cách mã hoá, kể cả fixture,
bài học 9a/9b). Không có coupling corpus-ratchet (banks 0), nên census không thể đỏ giữa suite — điểm
đơn giản hoá duy nhất 10a có so với 9a/9b.
