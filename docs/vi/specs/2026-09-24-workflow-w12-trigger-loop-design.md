# W12 — Vòng trigger: một `cron:` bắn được

**Trạng thái:** thiết kế, chờ review
**Phụ thuộc:** W6 (`trigger::tick` thuần và bộ parse cron), W10 (các ticker của runtime và `start_execution_with_trigger`) — cả hai đã merge
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §6.1, §6.3, §9 (dòng W12)
**Phạm vi đã chốt với người dùng, 2026-09-24:** chỉ schedule trigger; lịch bị lỡ được bù theo mặc định `ALL` của Kestra.

## 1. Cái gì đã có, cái gì chưa

W6 đã ship phần khó làm đúng của một scheduler, W10 đã ship phần chạy mọi thứ.
Chưa có gì nối hai phần lại.

| thành phần | trạng thái trên `origin/main` (`6fb7520e`) |
|---|---|
| parse cron, timezone, `withSeconds`, nickname, số học backfill | **đã có** — `trigger/schedule.rs`, đo đối chiếu artifact đã ghim |
| `trigger::tick(trigger, state, inputs, clock)` | **đã có, thuần** — đúng thứ tự `TriggerScheduler.processSchedulableTrigger`, `TickOutcome::{NotDue, Skipped, NotSchedulable, Fired}` |
| `TriggerState::initialise` | **một người gọi, và nó là `#[cfg(test)]`** |
| `workflow.triggers` | **không migration nào tạo** (migration mới nhất là `0153`) |
| vòng claim trigger đến hạn | **không có** — `runtime::spawn` khởi động hai ticker, claim và reaper |
| tạo execution kèm `ExecutionTrigger` | **đã có** — `worker::start_execution_with_trigger`, webhook đang dùng |

Vậy W12 thêm một bảng, một cách giữ bảng đó khớp với YAML của flow, và một
ticker thứ ba; phần còn lại dùng lại. **W12 không đụng tới ngữ nghĩa của `tick`.**

Hai sự thật ở phía ghi, kiểm trong `repo.rs` chứ không giả định, định hình §3:

- `workflow.flows` được ghi bởi đúng **bốn** câu lệnh, đều trong `FlowRepo`:
  `create` (INSERT), `update` (UPDATE gồm `source`), `soft_delete` (UPDATE
  `deleted_at`) và `set_live` (UPDATE `live_version_id`, `state`). Cả bốn chạy
  thẳng trên `&self.db.pool`, **không trong transaction**.
- Không chỗ nào trong `crates/egret-workflow/src` ghi `state = 'disabled'`, và
  không đường nào xoá `deleted_at`. Cả hai được các đường start đọc; hôm nay không
  API nào ghi.

## 2. Cái gì chấm W12

**Không phải ratchet.** Execution do `Schedule` bắn không mang liên kết nào mà
bước settle của `harvest.py` lần theo được (tuyên bố độ phủ của W6 Task 8,
`oracle-exec/README.md`), nên `baseline.txt` dự kiến đọc **109 trước và 109 sau**,
và nó vẫn sẽ đọc 109 nếu mọi dòng của W12 đều sai. Ghi ra đây để con số không bị
đọc thành "không tiến triển" hay "đã kiểm".

W12 được chấm bằng test của chính nó (§6), và mỗi test phải được **thấy đỏ**
dưới một mutation có tên trước khi được tính — kỷ luật của
`assertions-that-survive-mutation`. Suite cần DB không được tự skip: thiếu
`DATABASE_URL` là lỗi, không phải `ok`.

## 3. Schema và sync

### 3.1 Bảng

Migration `NNNN_workflow_triggers.sql`, với `NNNN` là số trống kế tiếp **vào lúc làm task**: hôm nay trên `origin/main` là `0154`, nhưng nhánh SP4a chưa merge (`feat/sp4a-permission-model`) đã giữ `0154` và `0155`, nên nhánh nào merge sau thì đánh số lại:

```sql
CREATE TABLE workflow.triggers (
  flow_id              uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  trigger_id           text        NOT NULL,   -- `id:` của trigger trong YAML
  workspace_id         uuid        NOT NULL,   -- chép từ flow; xem dưới
  spec_key            text        NOT NULL,   -- chuỗi chuẩn "cron|withSeconds|timezone", không băm
  disabled             boolean     NOT NULL,   -- `disabled` của flow YAML HOẶC của trigger
  next_evaluation_date timestamptz,            -- NULL = không nằm trong hàng đợi (xem 3.3)
  evaluated_at         timestamptz,
  last_error           text,
  updated_at           timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id)
);
CREATE INDEX workflow_triggers_due
  ON workflow.triggers (next_evaluation_date)
  WHERE next_evaluation_date IS NOT NULL AND NOT disabled;
```

- **Một hàng cho mỗi trigger `Schedule`**, không phải cho mọi loại trigger.
  Webhook được đẩy qua HTTP nên không cần hàng; flow trigger nằm ngoài phạm vi (§7).
- `workspace_id` được chép để log và các view quản trị sau này lọc được mà không
  cần join. Nó không phải nguồn thẩm quyền: lúc bắn, hàng flow được đọc trong cùng
  câu lệnh.
- **`disabled` là một cột vì nó không thể là một vị từ lúc bắn.**
  `DefaultSchedulableTriggerFetcher` của Kestra loại một trigger mà flow *hoặc*
  chính khai báo của nó bị tắt **trước** khi đánh giá (bytecode được trích ở
  `TickOutcome::NotSchedulable`): state bị đóng băng, không bị tiêu. Một vòng lặp
  chỉ phát hiện điều đó sau khi claim sẽ để hàng vẫn đến hạn, claim lại ở pass
  sau, và — với `ORDER BY next_evaluation_date LIMIT 1` — claim nó **đầu tiên** ở
  mọi pass, bỏ đói mọi trigger phía sau. Cả hai cờ đều nằm trong YAML, nên `sync`
  ghi chúng và câu claim lọc theo chúng.
- **Không có cột lease.** §4 giữ row lock suốt quá trình bắn, nên không có gì để
  thu hồi.
- Các trường còn lại của `TriggerState` ánh xạ một-một; `backfill` không được lưu
  vì không đường nào của W12 tạo ra nó (§7). Chuỗi `namespace`/`flow_id` được đọc
  từ flow lúc bắn chứ không chép, nên đổi tên không làm chúng lỗi thời.

### 3.2 Một cột thứ hai trên executions, để bắn trùng không thể ghi được

Cùng migration:

```sql
ALTER TABLE workflow.executions ADD COLUMN scheduled_date timestamptz;
CREATE UNIQUE INDEX workflow_executions_one_per_schedule_tick
  ON workflow.executions (flow_id, (execution_trigger->>'id'), scheduled_date)
  WHERE scheduled_date IS NOT NULL;
```

kèm một CHECK: `scheduled_date IS NOT NULL` **khi và chỉ khi**
`execution_trigger->>'type'` là `io.kestra.plugin.core.trigger.Schedule`.

`SKIP LOCKED` của §4 là thứ ngăn bắn trùng; index này làm cho bắn trùng **không
biểu diễn được** nếu §4 có lúc sai (`unrepresentable-beats-unforgettable`). Nó là
một cột có kiểu thay vì index trên `execution_trigger->'variables'` vì dạng chữ của
các biến đó chưa được đo (§5), và một bảo đảm duy nhất không được phụ thuộc vào
một quyết định định dạng.

### 3.3 Sync: một hàm, gọi ở mọi lần ghi làm thay đổi cái được bắn

Mã chạm database nằm trong **module mới `src/scheduler/`**, không nằm trong
`src/trigger/`: chính doc của `trigger/mod.rs` hứa rằng không gì trong cây đó chạm
database, container hay mạng, và test của nó dựa vào lời hứa ấy. `trigger/` giữ
thuần; `scheduler/` là người gọi có I/O duy nhất của nó.

`scheduler::sync(conn, flow: &FlowRow, gone: bool)` (nó tự parse `flow.source` trừ khi `gone`), đọc `now()` từ chính
transaction đó — đồng hồ của vòng lặp (§4 điểm 4), để `next_evaluation_date` đầu
tiên không bao giờ được tính trên một đồng hồ rồi so trên đồng hồ khác:

1. Gom các trigger `Schedule` của flow (không có gì nếu `gone`, hoặc nếu
   source không parse được, hoặc flow đang bị xoá).
2. **Xoá** các hàng mà `trigger_id` không còn được khai báo.
3. Với mỗi schedule được khai báo, tính `spec_key` và resolve bằng
   `ScheduleTrigger::resolve`:
   - **hàng mới, hoặc `spec_key` đổi** → upsert với `TriggerState::initialise`
     (tick cron **kế tiếp** sau `now`, không bao giờ là `now`),
     `evaluated_at = NULL`, `last_error = NULL`;
   - trong mọi trường hợp, `disabled` = `disabled:` của flow HOẶC `disabled:` của
     trigger; bật/tắt nó **không** đụng tới các ngày, nên bật lại sẽ bù các tick bị
     lỡ theo `ALL`;
   - **`spec_key` không đổi** → để nguyên `next_evaluation_date` và
     `evaluated_at`. Sửa một task không được làm reset lịch;
   - **resolve từ chối** (cron hỏng, `lateMaximumDelay`, một `when` không render
     được) → upsert với `next_evaluation_date = NULL` và lý do ở `last_error`;
   - **resolve được, nhưng không có tick nào trong tương lai** (`0 0 30 2 *`: một
     cron không bao giờ bắn) → như trên: `next_evaluation_date = NULL`, và
     `last_error` nói rằng lịch không bao giờ bắn (final review I4).

   Quy tắc NULL-kèm-lý-do áp dụng khi hàng được khởi tạo (lại). Một lời từ chối có
   nguyên nhân không nằm trong `spec_key`, gặp một hàng đang nằm trong hàng đợi với
   key không đổi, sẽ giữ nguyên ngày của hàng đó; câu claim gặp lời từ chối lúc bắn
   và khi đó mới gỡ hàng khỏi hàng đợi (§4 điểm 6).

Bốn chỗ ghi trở thành có transaction và gọi nó:

| chỗ ghi | lời gọi sync |
|---|---|
| `create` | `sync(tx, row, false)` |
| `update` | `sync(tx, row, false)` — kể cả khi chỉ đổi `namespace`/`folder_id`; khi đó nó không làm gì |
| `soft_delete` | `sync(tx, row, true)` — xoá mọi hàng. Kestra xoá state trigger của flow bị xoá; giữ lại sẽ khiến một lần khôi phục sau này phát lại toàn bộ thời gian nằm trong thùng rác theo `ALL` |
| `set_live` | **miễn**, có nêu lý do: nó không đổi `source` lẫn `deleted_at`, còn `state` được đọc lúc bắn (§4) |

**Chốt chặn cho chỗ ghi thứ năm hay quên:** một test quét nửa không-test của
`crates/**/src` tìm các câu `INSERT INTO`, `UPDATE` hoặc `DELETE FROM`
`workflow.flows`, và khẳng định mỗi câu nằm trong một hàm có gọi `scheduler::sync`,
hoặc nằm trong danh sách miễn trừ mà mỗi mục mang một lý do. Nó có control dương
(hôm nay phải tìm thấy đúng bốn câu trên), nên không thể pass nhờ quét rỗng.

**Các flow có trước bảng (final review I1).** Migration tạo `workflow.triggers`
rỗng, và chỉ các chỗ ghi ở trên gọi `sync`, nên một flow lưu trước đó và không bao
giờ được lưu lại sẽ không bao giờ có hàng. Vì vậy ticker của scheduler chạy `sync`
cho mọi flow chưa bị xoá một lần khi khởi động, trước pass đầu tiên, mỗi flow một
transaction riêng. Nó idempotent — `spec_key` không đổi thì giữ nguyên ngày — và
một flow mà `sync` lỗi được ghi log rồi bỏ qua, không bao giờ làm hỏng việc khởi
động. Giống một lần lưu, nó đưa trở lại hàng đợi một hàng mà `fire_due` đã gỡ ra,
và xoá các hàng của một flow mà source không còn parse được.

Một flow có `state` chuyển thành `disabled` giữ nguyên hàng và
`next_evaluation_date`; §4 chỉ đơn giản là không bắn nó. Khi được bật lại, các tick
bị lỡ được bù theo `ALL`. Đó là mặc định `recoverMissedSchedules` của Kestra và là
quyết định của người dùng; hôm nay không đường nào ghi `disabled`, nên đây là quy
tắc cho đường sẽ ghi nó sau này.

## 4. Vòng lặp

Ticker thứ ba trong `runtime::spawn`, cách nhau `trigger_interval`. **Mỗi pass
đánh giá mọi trigger đến hạn đúng một lần** — chu kỳ của chính `TriggerScheduler`
— **một trigger một transaction**, và giữ các cặp `(flow_id, trigger_id)` đã xử lý
trong `seen`:

```text
BEGIN
  SELECT t.*, f.id, f.workspace_id, f.source, now() AS db_now
    FROM workflow.triggers t
    JOIN workflow.flows f ON f.id = t.flow_id
   WHERE t.next_evaluation_date <= now()
     AND NOT t.disabled
     AND f.deleted_at IS NULL
     AND f.state <> 'disabled'
     AND NOT EXISTS (SELECT 1 FROM workspace_features wf
                      WHERE wf.workspace_id = f.workspace_id
                        AND wf.feature = 'workflows' AND NOT wf.enabled)
     AND (t.flow_id, t.trigger_id) NOT IN (SELECT * FROM unnest($seen_flows, $seen_ids))
   ORDER BY t.next_evaluation_date
   LIMIT 1
   FOR UPDATE OF t SKIP LOCKED
  -- không có -> ROLLBACK, hết pass
  parse f.source; tìm Schedule có id = t.trigger_id; ScheduleTrigger::resolve
  -- lỗi bất kỳ -> UPDATE t SET next_evaluation_date = NULL, last_error = <lý do>; COMMIT
  tick(trigger, state_from(t), schedule.inputs, FixedClock(db_now))
  Fired(e) -> start_execution_in(&mut tx, ws, f.id, f.source, inputs, trigger_record(e),
                                scheduled_date = e.scheduled_date)
  UPDATE t SET next_evaluation_date, evaluated_at, last_error = NULL/<lý do skip>
COMMIT
```

**Vì sao từng dòng như vậy:**

1. **Một transaction bao cả bắn lẫn đẩy lịch.** Hôm nay
   `start_execution_with_trigger` tự mở transaction từ pool. W12 tách thân của nó
   thành `start_execution_in(conn, …)` và giữ hàm cũ làm lớp bọc, nên webhook, API
   start và mọi test giữ nguyên hành vi. Chết ở bất kỳ đâu trước `COMMIT` sẽ
   rollback cả execution lẫn thay đổi state, và pass sau bắn lại đúng tick đó.
   **Một tick được bắn đúng một lần hoặc không lần nào** — không bao giờ
   bắn-mà-không-đẩy-lịch, không bao giờ đẩy-lịch-mà-không-bắn.
2. **`SKIP LOCKED` là sự loại trừ, và nó chỉ phủ câu lệnh này.** N replica chia
   tập đến hạn y như câu claim của worker. Các câu còn lại trong transaction chạy
   dưới row lock đó. Quy tắc của `runtime.rs` — một tuyên bố an toàn cho mỗi câu
   SQL, không phải một cho cả vòng — có thêm một mục W12.
3. **Không có lease.** Row lock được giữ suốt quá trình bắn; việc bên trong là một
   lần parse, một hàm thuần và lần `settle` đầu tiên của execution mới — không
   handler, không mạng. Không có gì đủ chậm để cần lease, nên không có gì để reaper
   thu hồi.
4. **Một đồng hồ, và đó là của Postgres.** Vị từ đến hạn và `tick` cùng dùng
   `now()` mà câu claim trả về, đưa vào `tick` dưới dạng `FixedClock`. Các replica
   lệch giờ vì thế vẫn thống nhất, và quy tắc của W6 rằng `SystemClock::now` là chỗ
   đọc wall clock duy nhất dưới `src/trigger/` vẫn giữ — `FixedClock` nằm cạnh
   `TestClock` trong module đó.
5. **Cả sáu cách tắt một trigger đều được tôn trọng, và không cách nào tiêu một
   tick.** `deleted_at` và cột `state` được đọc từ flow trong `WHERE`; `disabled:
   true` trong YAML của flow và của chính trigger là cột `disabled` mà `sync` đã ghi
   (§3.1); feature `workflows` của workspace khi bị tắt là một `NOT EXISTS` trên
   `workspace_features` — đúng bảng, và đúng cách đọc `enabled = false`, mà cổng
   route trong `WsContext` dùng, nên hai bên không thể lệch nhau (final review I3;
   quyết định của người dùng ngày 2026-09-25); và chính `status` của workspace,
   một `EXISTS` trên `workspaces` đòi `status = 'active'` — đúng cột và đúng giá
   trị mà `WsContext` (`crates/egret-workspace/src/context.rs`) và route webhook
   (`webhook_routes.rs`) đã dùng để từ chối một workspace không active, nên một
   workspace bị archived cũng không thể lệch với hai chỗ đó (quyết định của
   người dùng ngày 2026-09-25). Cả sáu để state đóng băng, đúng hành
   vi đã đo của `NotSchedulable`, nên bật lại bất kỳ cái nào cũng bù các tick bị lỡ
   theo `ALL`. **YAML vẫn được kiểm lại sau `parse`**, vì source đọc dưới lock là
   nguồn thẩm quyền còn cột chỉ là bản sao: nếu hai bên lệch nhau — trigger bị tắt,
   không còn được khai báo, hoặc khai báo một cron có `spec_key` khác của hàng —
   vòng lặp không bắn, để nguyên các ngày, và chạy lại `sync` cho flow đó — cùng
   cánh cửa thứ ba mà `subflow::spawn_one` đóng (Critical của W10). Phép so
   `spec_key` không phải giả định (final review I2, đã đo): một câu claim phải chờ
   một lần lưu sẽ đọc lại hàng trigger mới nhất dưới `FOR UPDATE OF t` nhưng giữ
   hàng flow của snapshot, nên thiếu nó một pass có thể bắn cron cũ và ghi ngày của
   nó dưới key mới — vĩnh viễn, vì lần lưu cùng key giữ nguyên ngày. Câu lệnh riêng
   của `sync` đọc source đã commit.
6. **Trigger hỏng bị gỡ khỏi hàng đợi, không bị thử lại mỗi pass.** Đặt
   `next_evaluation_date = NULL` ngăn một flow có draft không còn parse được tốn một
   transaction lỗi mỗi giây mãi mãi. Lần `sync` kế tiếp — tức lần lưu kế tiếp của
   flow đó — đưa nó trở lại.

**Vì sao là `seen` chứ không phải giới hạn `batch`.** Một trigger tụt lại nhiều
ngày luôn giữ `next_evaluation_date` cũ nhất. Dưới `ORDER BY … LIMIT n` nó sẽ được
claim đầu tiên ở mọi vòng lặp con — ngày đã đẩy của nó *vẫn* cũ nhất — nên nó sẽ
chiếm trọn batch của mọi pass và các trigger đúng giờ phải chờ tới khi nó bắt kịp.
`seen` biến "một lần mỗi pass" thành tính chất của câu truy vấn chứ không phải của
may mắn, và xử lý toàn bộ tập đến hạn giới hạn một pass bằng số trigger chứ không
bằng một núm chỉnh. Vì thế vòng này không có `batch`.

**Bắt kịp theo `ALL`.** `tick` đẩy một tick mỗi lần gọi và một pass gọi nó một lần
cho mỗi trigger. Hệ quả, suy ra chứ không đo: cron `* * * * *` sau ba ngày outage
nợ ~4.320 execution và, với interval mặc định 1 giây, mất ~72 phút để bắt kịp;
trong lúc đó mọi trigger khác vẫn bắn một lần mỗi pass. Tác giả flow không muốn
phát lại tồn đọng sẽ dùng `lateMaximumDelay`, thứ W12 **vẫn từ chối** (§7); tới khi
nó có, đòn bẩy duy nhất là tắt rồi lưu lại.

**Cấu hình.** `WorkflowConfig` thêm `scheduler_enabled` (mặc định **false**, cùng
lý do với `worker_enabled`) và `trigger_interval_secs` (mặc định 1). Cổng nằm trong
`RuntimeConfig` như của W10, để test chạm tới được. Bật scheduler mà worker tắt sẽ
in cảnh báo lúc khởi động: execution sẽ được tạo và không có gì chạy chúng.

## 5. Execution mà một schedule tạo ra

- `execution_trigger = { id: <trigger id>, type: "io.kestra.plugin.core.trigger.Schedule", variables: { date, next, previous } }`.
  **Dạng chữ** của ba giá trị đó là `Schedule.Output.toMap(ZoneId)` qua Jackson
  mapper của Kestra, mà doc của `ScheduledExecution` ghi là **chưa đo**. Task 1 đo
  nó trên artifact đã ghim (`kestra/kestra@sha256:28f59bf1…`) bằng cách bắn một
  schedule thật rồi đọc execution được lưu. Nếu không đo được, spec được sửa để nói
  định dạng là do chọn, và chọn cái gì.
- `scheduled_date` = `ScheduledExecution.scheduled_date` (§3.2).
- `inputs` = `Schedule.inputs`, **nguyên văn**. Kestra render chúng qua một
  `RunContext`; egret không có render context ở chỗ nối này. Giá trị chứa cú pháp
  template (`{{` hoặc `{%`) bị **từ chối** — tick bị tiêu và lý do vào
  `last_error` — thay vì chạy với chuỗi nguyên văn, vì đó là chấp nhận quá mức theo
  đúng hướng oracle không thấy. `schedule_missing_inputs` của `spec::validate` đã
  từ chối, ngay lúc ghi, một schedule bỏ trống input bắt buộc.
- `trigger.*` được lưu và **không** đưa vào render context — cùng trạng thái
  migration `0140` để lại cho webhook. W12 không đổi điều đó.

## 6. Test

Mỗi dòng nêu mutation phải làm nó đỏ. Dòng nào mà mutation sống sót thì được báo
là chưa chứng minh, không phải đã phủ.

| tính chất | test | mutation phải giết |
|---|---|---|
| sync tạo, xoá và giữ | thêm/bỏ một `Schedule`; sửa một task không liên quan → `next_evaluation_date` không đổi; đổi cron → khởi tạo lại về tick kế tiếp | bỏ phép so `spec_key` |
| sync khi xoá | `soft_delete` xoá mọi hàng | bỏ lời gọi ở `soft_delete` |
| mọi chỗ ghi đều sync | bộ quét ở §3.3, control dương = 4 | xoá lời gọi `sync` trong `update` |
| bắn đúng một lần | `next_evaluation_date` lùi một tick về quá khứ, hai pass → một execution với `execution_trigger` đầy đủ và `scheduled_date` | bỏ câu `UPDATE` state |
| replica không bao giờ bắn trùng | hai ticker, một DB, N trigger đến hạn → mỗi `(trigger, date)` có đúng một execution; khẳng định trên execution, không trên số đếm | bỏ `SKIP LOCKED` |
| unique index là chốt chặn thật | tự insert execution thứ hai cho cùng `(flow, trigger, date)` → bị từ chối | bỏ index |
| bắn + đẩy lịch là nguyên tử | tiêm lỗi sau `start_execution_in`, trước `COMMIT` → không có execution, state không đổi | tách thành hai transaction |
| bắt kịp theo `ALL` | `next_evaluation_date` lùi năm tick về quá khứ → năm pass → năm execution, `scheduled_date` tăng dần, mỗi pass một cái | tick tới khi rảnh trong một pass |
| trigger tụt lại không bỏ đói trigger khác | trigger A tụt năm tick, trigger B đến hạn một tick → pass đầu bắn mỗi cái một lần | bỏ loại trừ `seen` |
| trigger hỏng không làm kẹt vòng | draft không parse được → có `last_error`, `next_evaluation_date` NULL; một trigger bên cạnh trong cùng pass vẫn bắn | bỏ nhánh gỡ khỏi hàng đợi |
| mọi công tắc tắt | `state='disabled'`, YAML `disabled: true`, trigger `disabled: true`, `deleted_at`, feature `workflows` của workspace bị tắt, `status='archived'` của workspace → không execution nào **và** `next_evaluation_date` không đổi; bật lại công tắc → các tick bị lỡ được bắn | bỏ lần lượt từng vế |
| trigger bị tắt không bỏ đói hàng đợi | hàng đến hạn cũ nhất bị tắt, một hàng trẻ hơn đang bật → hàng trẻ hơn bắn, và hàng bị tắt không bao giờ được claim | bỏ `NOT t.disabled` khỏi câu claim |
| bật lại thì bù theo `ALL` | tắt, đặt `next_evaluation_date` lùi ba tick, bật → ba pass, ba execution | reset các ngày khi `disabled` đổi |
| input có template bị từ chối | `inputs: {x: "{{ now() }}"}` → không execution, có ghi lý do | bỏ phép kiểm |
| mặc định tắt | `scheduler_enabled = false` → không câu lệnh nào chạy | đảo mặc định |
| đầu cuối | `tests/runtime.rs`: một flow với `*/1 * * * *`, bật worker và scheduler, `next_evaluation_date` của hàng được ghi lùi một phút → execution tới `SUCCESS` mà không có gì gọi `tick` bằng tay | — |

Đồng hồ trong test có DB: `now()` của câu claim là của Postgres, nên test cần một
trigger đến hạn sẽ ghi `next_evaluation_date` về quá khứ thay vì dịch một đồng hồ
Rust. `TestClock` vẫn dùng cho các test thuần của `tick`.

## 7. Cố ý không nằm trong W12

Ghi vào §9 của spec chương trình để mỗi mục có chủ hoặc một chữ "không ai" rõ ràng:

- **Flow trigger** (`trigger/flow.rs`, nhánh không cửa sổ) — theo sự kiện, thuộc
  về transaction settle của worker, không thuộc một bảng claim. Đề xuất **W13**.
- **API backfill** — `tick` hỗ trợ; không endpoint nào tạo backfill và corpus có
  0 khai báo sống.
- **`lateMaximumDelay`** — vẫn bị `ScheduleTrigger::resolve` từ chối, vì việc parse
  `java.time.Duration` chưa được đo.
- **`stopAfter`**, và `concurrency` / `allowConcurrent` giữa các lần cron liên tiếp.
- **Đưa `trigger.*` vào render context.**
- **UI cho trigger** — W7.

## 8. Cách giao

Một PR docs (spec này, en + vi, cùng các cập nhật §6.1/§6.3/§9 của spec chương
trình), rồi một PR code gồm sáu task:

1. Đo định dạng output của `Schedule` trên artifact (§5).
2. Migration `NNNN` (§3.1) + `scheduler::sync` + các chỗ ghi `FlowRepo` có transaction + bộ
   quét chỗ ghi.
3. Tách `start_execution_in`; hàm cũ thành lớp bọc của nó.
4. Transaction bắn (`scheduler::fire_due`), `FixedClock`, các công tắc tắt và các
   đường từ chối.
5. Ticker, `WorkflowConfig`, các mục an toàn trong `runtime.rs`, test đồng thời và
   test đầu cuối.
6. Whole-branch review, điền bảng mutation, chạy đủ cổng CI cục bộ.
