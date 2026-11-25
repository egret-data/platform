# secret() ở các render surface — thiết kế (gói 10a)

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

## 6. Khác biệt / sự thật đo được (Task 0 — điền từ Kestra trước khi pin bất kỳ terminal nào)

*(Trống tới khi Task 0 chạy. Ghi đáp án M1–M4, quyết định seal inputs (D3), và mọi divergence quan sát
được ở đây, kèm `file:line` vào checkout Kestra `data-portal/kestra` @ `96893b589`.)*

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
