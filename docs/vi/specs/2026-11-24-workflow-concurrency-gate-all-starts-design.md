# Workflow concurrency — gate mọi kiểu khởi động (gói 8c)

**Trạng thái:** thiết kế, chờ duyệt
**Tác giả:** chương trình parity workflow egret
**Ngày:** 2026-11-24
**Phụ thuộc:** 8a (lõi concurrency — #908), 8b (behaviors + unqueue/force-run — #909)

## 1. Mục tiêu

Làm cho `concurrency: {limit, behavior}` của flow áp dụng cho **mọi** cách một
execution của flow đó khởi động — không chỉ fresh start. Hiện nay (8a/8b) chỉ
fresh start (API, schedule, webhook, Flow trigger) đi qua gate
(`concurrency::concurrency_admit`). Ba cách khởi động khác **bỏ qua** gate và
chạy bất kể limit:

- **replay** (execution mới copy từ một execution cũ, toàn bộ hoặc một phần);
- **restart-on-source** (biến thể replay chạy source đã đóng băng/chọn);
- **restart-in-place** một execution đã thất bại;
- **subflow con** do task `Subflow` sinh ra.

8c đóng khoảng trống đó: tất cả đi qua cùng một gate và tuân cùng behavior
(QUEUE / CANCEL / FAIL) như fresh start.

## 2. Tiền đề đã được chỉnh lại

Roadmap parity ghi follow-up này là *"tính vào limit nhưng chưa queue — soft
overshoot có giới hạn"*. **Tiền đề đó sai** khi đối chiếu Kestra, và 8c theo
Kestra.

Kestra áp gate concurrency trong `ExecutionEventMessageHandler` cho bất kỳ
execution nào có state là `CREATED` **hoặc** `failedThenRestarted()`:

```java
// executor/.../handler/ExecutionEventMessageHandler.java
if ((execution.getState().getCurrent() == State.Type.CREATED
     || execution.getState().failedThenRestarted())) {
    // ... resolve concurrency limits, rồi QUEUE / CANCEL / FAIL
}
```

- `CREATED` bao gồm fresh start, **replay** (execution mới born `CREATED`), và
  **subflow con** (cũng born `CREATED`).
- `failedThenRestarted()` = `current == RESTARTED && state trước đó isFailed()`
  — tức **restart-in-place một execution đã FAILED**. (Một `RESTARTED` theo sau
  `PAUSED` — tức resume — **không** phải `failedThenRestarted()`, nên resume
  **không bao giờ** bị re-gate.)

Vậy Kestra queue/cancel/fail cả bốn kiểu khởi động, **không soft overshoot**.
Không có ngoại lệ nào cho phép replay hay subflow con vượt limit.

Hai điều vốn đã đúng ở egret và không đổi bởi 8c:

- Live count của gate —
  `count(*) WHERE flow_id = $1 AND NOT state_is_terminal(state) AND state <> 'QUEUED'`
  — không phân biệt execution khởi động **bằng cách nào**, nên replay, restart
  và subflow con **đã được tính** vào limit (chúng chiếm slot). 8c chỉ thêm nửa
  còn thiếu: chính những start đó bị gate khi chúng đẩy flow vượt limit.
- Scope của gate là concurrency của chính flow. Subflow con bị gate theo
  `concurrency:` của **flow con**, không phải của cha.

## 3. Quyết định

**D1 — Full parity, một gói (user, 2026-11-24).** Gate cả replay,
restart-on-source, restart-in-place và subflow con trong 8c, khớp quy tắc
`CREATED | failedThenRestarted` của Kestra. Không tách restart sang gói sau trừ
khi hiện thực chứng tỏ bất khả thi (xem fallback §9).

**D2 — Deadlock footgun: khớp Kestra, không chặn (user, 2026-11-24).** Flow tự
khởi động chính nó bằng `Subflow` với `wait: true` dưới limit thấp có thể
deadlock: cha giữ slot khi `RUNNING`, con `QUEUED`, không bên nào tiến được.
Kestra cũng có footgun này và không chặn. 8c cũng không chặn; §7 tài liệu hoá
cho người dùng.

**D3 — Drain thống nhất bằng pop history.** Drain khôi phục execution `QUEUED`
về state mà nó sẽ có nếu không bị queue, rồi settle. `promote_queued` của 8a ép
state khôi phục là `CREATED`; 8c tổng quát hoá thành **pop `QUEUED` cuối khỏi
history đã lưu** và khôi phục state trước đó (`CREATED` cho fresh/replay/subflow,
`RESTARTED` cho restart). Tương thích ngược: history của fresh start là
`["CREATED","QUEUED"]`, pop ra `["CREATED"]` với state `CREATED` — y như hiện
tại.

**D4 — Ghi gate là append, không ghi đè.** Khi gate quyết `QUEUE`, state
`QUEUED` được **append** vào history hiện tại của row. Với execution mới
(fresh/replay/subflow) history hiện tại là `["CREATED"]` → `["CREATED","QUEUED"]`
(không đổi so với 8a). Với restart-in-place history hiện tại là
`[…,"FAILED","RESTARTED"]` → `[…,"FAILED","RESTARTED","QUEUED"]`, chính là thứ
pop của D3 đảo ngược.

**D5 — CANCEL / FAIL terminalize qua settle, không task run.** Như 8b, một start
vượt limit dưới `behavior: CANCEL`/`FAIL` thành `CANCELLED`/`FAILED` terminal
ngay, không task run và không chiếm slot, nhưng **đi qua** settle đầu tiên để
terminal hooks (outbox, Flow-trigger listeners, alerts) chạy. Với
replay/subflow con, execution mới terminalize trước khi copy bất kỳ run nào.
Với restart-in-place, execution — vốn đang `FAILED` — được set
`CANCELLED`/`FAILED` qua settle mà không drive.

**D6 — Không migration, không API mới, không đổi FE.** 8c dùng lại state
`QUEUED`, các route `unqueue`/`force-run` và FE của 8a/8b. Một replay / restart
/ subflow con bị queue hiện trong UI như một execution `QUEUED` bình thường và
có thể unqueue hoặc force-run y như một fresh start bị queue.

**D7 — Gộp các minor tồn đọng của 8b.** Xem §8.

## 4. Bốn điểm gate

Mỗi đường tới một điểm sắp chạy (drive) một execution mới hoặc mở lại. 8c chèn
gate tại điểm đó. Gate luôn là `concurrency_admit(conn, flow_id, limit,
behavior)` dưới advisory lock của flow, cùng transaction với lệnh ghi theo sau.

### 4.1 Fresh start (đã xong, 8a/8b)

`claim::start_execution_in_on` → `insert_execution_gated(…, gated=true)`. Không
đổi, ngoài việc giờ dùng chung helper `Admit → row/settle` mà 8c tách ra (§5) để
mọi đường xử lý bốn kết quả giống nhau.

### 4.2 Replay và restart-on-source (`worker/copy.rs`)

`copy::replay_locked` dựng execution **mới** bằng `claim::insert_execution`
(hiện `gated=false`) rồi settle — `settle_new` cho replay toàn bộ (không có run
copy) hoặc copy-rồi-settle cho replay một phần / restart-on-source.

Thay đổi 8c:

- Gate lúc insert (`gated=true`), bắt lấy `Admit`.
- `Admit::Run` → hành vi hiện tại (copy run nếu có, rồi settle).
- `Admit::Queue` → **vẫn copy run** (run copy của replay một phần phải tồn tại
  để drain resume), set row `QUEUED` (history `["CREATED","QUEUED"]`), và **bỏ
  qua settle**. Drain resume nó.
- `Admit::Cancel`/`Fail` → **không** copy run; terminalize execution mới qua
  settle (D5).

Vì replay là execution mới born `CREATED`, pop của drain (D3) khôi phục
`CREATED`, và `load` đọc bất kỳ run nào đã copy — replay toàn bộ không có (settle
từ rỗng), replay một phần có prefix đã copy (settle resume).

### 4.3 Restart-in-place (`worker/restart.rs`) — đường khó

`restart::restart_execution` khoá execution thất bại, `prepare` dựng lệnh reset
(xoá kết quả/lease của từng run thất bại) và chuyển `RESTARTED`, và settle walk
apply + persist chúng rồi drive.

Thay đổi 8c: sau khi lệnh reset và `RESTARTED` đã được **persist**, nhưng trước
khi walk drive, kiểm tra gate.

- `Admit::Run` → drive (hành vi hiện tại).
- `Admit::Queue` → append `QUEUED` (history `[…,"RESTARTED","QUEUED"]`), persist,
  và dừng trước khi drive. Run reset đã persist, nên drain (D3 pop → `RESTARTED`;
  `load` đọc run reset; settle drive) resume một restart giống hệt.
- `Admit::Cancel`/`Fail` → set `CANCELLED`/`FAILED` qua settle không drive (D5).

Đây là đường duy nhất gate rơi **vào trong** settle walk thay vì trước một
insert, nên là task rủi ro nhất. Implementer tìm seam sạch nhất để (a) persist
reset trước khi dừng `QUEUED` và (b) để lại row mà drain thống nhất resume được.
§9 cho fallback nếu seam bất khả thi.

### 4.4 Subflow con (`worker/subflow.rs`)

Task `Subflow` sinh con bằng INSERT riêng (born `CREATED`) rồi settle con trong
settle của cha (`wait: false`, đọc state con tại thời điểm này).

Thay đổi 8c:

- Gate theo `flow_id` + `concurrency:` của **flow con**
  (`concurrency_admit`), dưới advisory lock của flow con.
- `Admit::Run` → hành vi hiện tại.
- `Admit::Queue` → insert con `QUEUED`, **bỏ qua settle con**. Run `Subflow` của
  cha thấy con non-terminal (`QUEUED`): với `wait: true` nó tiếp tục chờ; drain
  chạy con khi slot trống, và settle terminal của con báo cho cha qua đường
  subflow-completion sẵn có.
- `Admit::Cancel`/`Fail` → insert con, terminalize qua settle (D5). Run
  `Subflow` của cha thấy con terminal `CANCELLED`/`FAILED` và phản ứng như với
  bất kỳ con terminal nào (`transmitFailed`, v.v.).

Subflow con born `CREATED` không có run, nên drain thống nhất khôi phục `CREATED`
và settle từ rỗng — đúng đường `promote_queued` sẵn có, không ngoại lệ.

## 5. Helper admit dùng chung

8c tách quyết định `Admit → (row state, row history, có settle không, terminal
nào)` thành một helper để cả bốn đường xử lý bốn kết quả giống nhau và không
lệch. Hình dạng (minh hoạ, implementer có thể chỉnh tên):

- đầu vào: history hiện tại của row sắp ghi/mở-lại và `Admit`;
- đầu ra: state + history cần persist, và caller nên settle bình thường,
  terminalize (với state nào), hay dừng (queue).

Fresh start, replay, subflow con ghi row mới (gốc `["CREATED"]`);
restart-in-place mở lại row cũ (gốc là history sống của nó). Logic
append/terminalize của helper giống nhau cho cả hai; chỉ khác history gốc.

## 6. Drain (`worker/concurrency.rs`)

`concurrency_drain`, `on_terminal` và `concurrency_sweep` **không đổi** hình
dạng: vẫn tìm các row `QUEUED` cũ nhất của một flow dưới limit và gọi
`promote_queued`. Thay đổi duy nhất nằm trong `claim::promote_queued` (D3): nó
pop `QUEUED` cuối khỏi history đã lưu và khôi phục state trước đó rồi `load` +
`settle`, thay vì ép `CREATED`.

Nghĩa là một replay, restart hay subflow con bị queue được drain bởi đúng máy móc
oldest-first như một fresh start bị queue, giữ thứ tự FIFO qua mọi kiểu khởi
động của một flow.

## 7. Deadlock footgun (tài liệu hoá, không chặn — D2)

Flow tự khởi động **chính nó** bằng `Subflow` với `wait: true` dưới một
concurrency limit thấp hơn độ sâu đệ quy của nó có thể deadlock: cha giữ slot khi
`RUNNING` và chờ, con `QUEUED` không thể được promote tới khi slot trống, và
không slot nào trống tới khi cha xong. Kestra cũng vậy. Docs
(`concurrency.md`) cảnh báo rằng một `Subflow` tự tham chiếu dưới limit thấp với
`wait: true` có thể treo, và `unqueue`/`force-run` (8b) là cửa thoát thủ công.

## 8. Các minor tồn đọng của 8b gộp vào

- **M2 — parse `?state=` của unqueue.** Một `?state=` không parse được trên route
  unqueue single/by-ids trả 400 không mã; cho nó một lỗi có mã, có tài liệu,
  nhất quán với các từ chối đầu vào khác.
- **Test deadlock-race.** Một test xác định cho ABBA mà deadlock-retry của 8b
  hấp thụ (force-run/unqueue advisory→row vs các mutator settle-drain
  row→advisory), thay vì chỉ dựa vào retry.
- **Test cap MAX_BULK.** Khẳng định cap `MAX_BULK` (1000) được áp cho route
  unqueue và force-run by-ids/by-query.
- **Test bulk-over-limit race.** Khẳng định một force-run hàng loạt vượt limit
  tôn trọng behavior (live count của drain đúng dưới promotion đồng thời).

## 9. Rủi ro và fallback

Gate restart-in-place (§4.3) là thay đổi duy nhất chạm vào trong settle walk và
là task rủi ro cao nhất. Nếu seam bất khả thi trong gói này, fallback là: giữ
restart-in-place soft-overshoot **chỉ** dưới `behavior: QUEUE` (vẫn terminalize
dưới CANCEL/FAIL, vốn không cần resume-drain), ship ba đường còn lại, và ghi
nhận gating QUEUE của restart-in-place là gói 8d. Mặc định là làm trọn cả bốn
trong 8c.

## 10. Kiểm thử

Theo từng đường, với `concurrency: {limit: 1, behavior: QUEUE|CANCEL|FAIL}`:

- **Replay** vượt limit: QUEUE → execution mới `QUEUED`, drain tới resume đúng
  (replay toàn bộ chạy từ rỗng; replay một phần resume prefix đã copy);
  CANCEL/FAIL → terminal, không run, hooks chạy.
- **Restart-on-source** vượt limit: ba kết quả như trên.
- **Restart-in-place** vượt limit: QUEUE → `QUEUED` rồi drain tới một restart
  giống hệt (run reset resume); CANCEL/FAIL → terminal.
- **Subflow con** vượt limit: QUEUE → con `QUEUED`, cha chờ, drain chạy con, cha
  được báo; CANCEL/FAIL → con terminal, cha phản ứng.
- **FIFO** qua các kiểu: một fresh start bị queue và một replay bị queue của
  cùng flow drain oldest-first.
- **Tương đương drain**: một fresh start bị queue được drain đúng byte-for-byte
  như 8a tạo ra (tương thích ngược D3).
- Các test của minor §8.

Scoped local runs cho `egret-workflow` **phải** gồm `--test routes --test
execution_timestamps` (cả hai guard nằm trong binary mà scoped run bỏ qua; chúng
làm vỡ CI ở 8a và 8b).

## 11. Ngoài phạm vi

- Concurrency limit scope namespace và tenant (Kestra EE; egret chỉ resolve
  `concurrency:` của chính flow).
- Bất kỳ thay đổi nào ở API `unqueue`/`force-run` hay FE ngoài mã lỗi M2.
- Chặn deadlock self-subflow (D2).
