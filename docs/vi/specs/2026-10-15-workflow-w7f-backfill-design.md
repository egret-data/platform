# W7f — UI Workflows, phần 6: backfill của schedule trigger

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-15-workflow-w7f-backfill-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-15. Chưa xây gì. Mọi khẳng định về code của egret đã được kiểm tra trong worktree `egret-w7f` tại `f73c13647` (main, migration đến 0189) và được trích dẫn `path:line` từ `crates/egret-workflow/src/`, trừ khi ghi root khác. Kestra là checkout tại `data-portal/kestra`, `96893b589`; path của nó tính tương đối từ đó.

**Phụ thuộc:**
- W7e (#672, `f73c13647`): các route trigger, `TriggerEntryDto`, thứ tự lock, tab Triggers, trang Triggers toàn cục, `useTriggerSwitch`, dialog bật. Spec của nó là `docs/en/specs/2026-10-14-workflow-w7e-triggers-design.md`; §9 ở đó đã dành sẵn chỗ cho gói này.
- W12 (`workflow.triggers`, `scheduler::sync`, `fire_due`) và W6 Task 5 (mô hình backfill thuần trong `trigger/`, đã port nhưng chưa bao giờ nối dây).
- W7a (form Run, `frontend/src/components/workflows/run/`), W7b (trang tổng quan execution).

Bộ rule frontend `docs/en/frontend-rules.md` ràng buộc mọi dòng code frontend.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §8 (UI của Kestra làm tham chiếu) và §9 dòng W7.

**Đã chốt với người dùng, 2026-10-15:**
- **Một backfill trên khoảng thời gian đã chạy sẽ chạy lại các ngày đó, như Kestra.** Một tick backfill cho ngày đã có execution sẽ tạo execution thứ hai. Vì vậy unique index `workflow_executions_one_per_schedule_tick` (0156) thôi không bao phủ các execution backfill, và một execution backfill được nhận diện trong database (D2). Vẫn giữ một execution cho mỗi `(trigger, backfill, tick)`.
- **Input và label của backfill nằm trong phạm vi.** Input ghi đè `inputs:` của schedule theo từng key (`putAll` của Kestra), dưới cùng rule `refuse_templated`. Label cần một kho theo từng execution mà egret hiện chưa có (D3).

**Quyết định mặc định (2026-10-15),** theo rule của brief "theo Kestra trừ khi egret có lý do; giữ an toàn chặt". Mỗi quyết định được giải thích ở chỗ nó được dùng:
- D1. Một backfill nằm trong **một bảng riêng**, `workflow.trigger_backfills`, khoá `(flow_id, trigger_id)`, không nằm trong các cột của `workflow.triggers`. `sync` xoá mọi dòng `workflow.triggers` của một flow mà source không parse được; một backfill đang chạy phải sống sót qua đó, như các dòng công tắc của W7e (§3.1).
- D2. Một execution backfill mang **`workflow.executions.backfill_id`**, id của backfill đã bắn nó. Unique index theo tick được dựng lại với `AND backfill_id IS NULL`, và một unique index thứ hai trên `(backfill_id, scheduled_date)` giữ một execution cho mỗi tick của backfill. `execution_trigger` không đổi, nên phép kiểm tra bằng oracle vẫn đúng (§3.2).
- D3. **`workflow.executions.labels`** (jsonb, danh sách `{key, value}`) giữ các label mà một execution được khởi động với, **ngoài `labels:` của chính flow**. Bề mặt render và nguồn Flow-trigger gộp label của flow, rồi các label này, key sau thắng, như Kestra. Replay và vòng lặp sao chép chúng (§3.3).
- D4. Cửa sổ **bao gồm cả hai đầu**: `[start, end]`. Create của Kestra loại tick rơi đúng vào `start` (§2, §3.4).
- D5. Tạo backfill chuyển `next_evaluation_date` sang tick đầu tiên của cửa sổ và lưu vị trí của schedule trước đó, `previous_next_evaluation_date`, **một lần**. Pause và resume không bao giờ đụng vào nó (Kestra ghi đè nó ở mỗi lần pause và resume) (§3.4).
- D6. Khi một backfill kết thúc, do hoàn tất hay do xoá, schedule thường tiếp tục từ chỗ nó đã đứng, và các tick thường đến hạn trong lúc backfill được xử lý bằng `recoverMissedSchedules` của chính trigger (vắng = `ALL`). Kestra bỏ qua chúng khi hoàn tất và khôi phục ngày cũ khi xoá (§3.6).
- D7. Một backfill đang tạm dừng **không được claim**: claim loại nó ra, nên một trigger đang tạm dừng không tốn gì cho mỗi lượt. Pause và resume chỉ ghi dòng backfill (§3.5).
- D8. Server validate những gì Kestra chỉ kiểm ở UI của nó, và nhiều hơn: `start < end`, `end <= now()`, `start` lùi tối đa mười năm, ít nhất một và nhiều nhất 10.000 tick trong cửa sổ, chỉ `Schedule`, chỉ trigger đang bật và nằm trong hàng đợi, kiểm input và label. Create lần hai là 409 `backfill_exists` (Kestra thay thế âm thầm); pause, resume hoặc delete khi không có backfill là 404 `no_backfill` (Kestra trả 200 và không làm gì) (§3.4, §3.5).
- D9. Một trigger bị tắt bằng công tắc hoặc bị tắt trong YAML giữ backfill của nó đóng băng, như Kestra. Tạo một backfill trên trigger đang tắt bị từ chối, 409 `trigger_disabled` (§3.4).
- D10. `sync`: khi một schedule có backfill được khởi tạo lại (`spec_key` của nó đổi, nó từng rời hàng đợi, hoặc dòng của nó bị tạo lại), ngày kế tiếp của nó được **suy lại từ backfill**, mà không mất tick hiện tại. Một trigger id bị gỡ, đổi kind và xoá mềm sẽ xoá backfill; một source không parse được thì giữ nó (§3.7).
- D11. Bật lại một trigger đang tắt trong lúc có backfill giữ nguyên các ngày của nó, nên backfill tiếp tục ở tick nó đã dừng; lựa chọn của dialog áp dụng cho vị trí tiếp tục của schedule thường. Điều này thay câu của W7e §9, vốn hiểu sai Kestra (§2, §3.8).
- D12. Trong lúc backfill, `missed_ticks` đếm các tick **thường** đang chờ (từ `previous_next_evaluation_date` đến bây giờ), không phải của cửa sổ (§3.9).
- D13. Một tick của cửa sổ cho mỗi trigger cho mỗi lượt, như mọi schedule hiện nay (§3.6).
- D14. Các route nằm dưới route của W7e: create, pause/resume và delete theo từng trigger, `workflow:manage`; pause/resume và delete hàng loạt theo lựa chọn, tối đa 100, đồng bộ, có kết quả cho từng mục. Không có create hàng loạt (như Kestra). Một audit event cho mỗi thay đổi (§3.10).
- D15. `TriggerEntryDto` thêm `backfill`, `can_backfill` và `backfill_refusal`; tiến độ là công thức thời gian của Kestra, do server tính, cộng số tick chính xác (§3.9).
- D16. Form tạo là một chunk lazy; ngày dùng control `datetime-local` gốc với `max` đặt bằng bây giờ; input dùng lại mô hình và control của form Run (§4.3).
- D17. Migration là **0192** (§3.1). Quyết định mặc định (2026-10-15): 0190 và 0191 do worktree `egret-tagdef` (nhánh `fix/tag-policies-service-def`) giữ.
- D18 (P1). **Danh sách** execution mang `labels` riêng của execution cộng `backfill_id`; **chi tiết** (`ExecutionDto`) mang `effective_labels`, phép gộp (§3.3). Quyết định mặc định (2026-10-15).
- D19 (P2). Một thay đổi cron sẽ cho backfill hơn 10.000 tick thì **kết thúc nó**: `sync` cho schedule thường tiếp tục và ghi `last_error` (§3.7). Quyết định mặc định (2026-10-15).
- D20 (P3). Một lần snap lại không tìm được tick thì **giữ** `previous_next_evaluation_date` đã lưu (§3.7). Quyết định mặc định (2026-10-15).
- D21 (P4). `ticks_done` được tăng bằng `LEAST(ticks_done + 1, total_ticks)`, để một vi phạm CHECK không thể kết thúc một lượt scheduler (§3.6). Quyết định mặc định (2026-10-15).
- D22 (P5). Race tạm dừng giữa claim và lần đọc mới được dàn dựng bằng một hook chỉ dành cho test trong `scheduler::fire` (§6). Quyết định mặc định (2026-10-15).
- D23 (P6). Cờ bundle của dialog là cờ `backfill` mới, chặt hơn, không phải `editor` (§4.3). Quyết định mặc định (2026-10-15).
- D24 (P7). `deserialize_labels` được nới thành `pub(crate)` (§3.4). Quyết định mặc định (2026-10-15).
- D25. Một tick backfill bị từ chối khi start (`Started::Refused`) **kết thúc backfill** trong cùng transaction thay vì đưa trigger ra khỏi hàng đợi (§3.6). Quyết định mặc định (2026-10-15, review Task 4).

## 1. Vì sao

`Backfill` đã được port và thuần (`trigger/schedule.rs:1634-1652`, `trigger/mod.rs:270-310`), còn `fire_one` hard-code `backfill: None` (`scheduler/fire.rs:226-233`). Không route nào tạo ra nó: các route của W7e là hai route đọc và hai công tắc (`triggers/routes.rs:5-10`). Một người dùng Kestra thêm schedule vào một flow có sẵn, hoặc sửa một flow đã fail cả tuần, mong có "Backfill executions" trên trigger: chạy schedule cho một khoảng thời gian trong quá khứ, với input và label được chọn, rồi theo dõi, tạm dừng hoặc dừng nó.

## 2. Khác biệt so với Kestra

**Đọc từ mã nguồn** (checkout ở trên) trừ khi một ô nói khác; phép so sánh ở §7 gắn lại nhãn **quan sát** cho từng dòng mà nó đo.

| Kestra | egret | Vì sao |
|---|---|---|
| **Đọc từ mã nguồn.** Create, pause, resume và delete không validate gì trên server: không có `start < end`, không kiểm end ở tương lai, không có "đã có một cái" (create thay thế backfill đang chạy), không kiểm trigger có phải `Schedule` (`TriggerController.java:222-314`; `TriggerEventHandler.java:154-183`). UI chặn ngày tương lai và `start > end` (`ui/src/components/flows/FlowTriggers.vue:262-277`), input bắt buộc và label điền dở (`:509-540`) | Server từ chối từng trường hợp (D8, §3.4), kèm một code; create lần hai là 409 `backfill_exists` | Một caller API không được nhận 200 cho một request mà UI sẽ không bao giờ gửi, và việc thay thế âm thầm làm mất một cửa sổ đang chạy |
| **Đọc từ mã nguồn.** Pause, resume hoặc delete khi không có backfill không làm gì và trả 200 (`TriggerEventHandler.java:193, 214`) | 404 `no_backfill` | Nói rõ chuyện gì đã xảy ra |
| **Đọc từ mã nguồn.** Cửa sổ loại tick nằm đúng ở `start`: create tính lại ngày kế tiếp từ `currentDate = start` bằng `executionTime.nextExecution`, vốn là strictly-after (`TriggerEventHandler.java:177-179`, `Schedule.java:247-249, 273, 466-468`). Nó bao gồm tick ở `end` (`TriggerState.java:338-342`: chỉ xoá khi `next > end`) | `[start, end]`, bao gồm cả hai đầu (D4) | "Backfill 1 đến 7 tháng 10" trên cron nửa đêm phải chạy 7 lần, không phải 6 |
| **Đọc từ mã nguồn.** Create lưu `previousNextExecutionDate = nextEvaluationDate`, và mỗi pause và resume cũng vậy, vì cả hai dựng lại backfill qua cùng `TriggerState.backfill(...)` (`TriggerState.java:229-239`; `TriggerEventHandler.java:211-220`). Sau một lần pause, delete khôi phục một **ngày của cửa sổ** (`:190-203`), và scheduler sau đó phát lại mọi tick thường từ ngày đó đến bây giờ. Nếu không có ngày được lưu, delete sẽ throw (`TriggerState.java:204-206`) | Lưu một lần, lúc create (D5); delete tiếp tục schedule thường như §3.6 nói | Cái quirk này phát lại những tick không ai yêu cầu |
| **Đọc từ mã nguồn.** Khi hoàn tất tự nhiên, ngày kế tiếp là tick kế tiếp sau **bây giờ** (`Schedule.java:278-280`): các tick thường đến hạn trong lúc backfill bị bỏ qua. Khi delete, ngày kế tiếp cũ được khôi phục, nên chúng được phát lại. Hai đầu mâu thuẫn nhau | Cả hai đầu tiếp tục từ vị trí của chính schedule thường và áp dụng `recoverMissedSchedules` của trigger (vắng = `ALL`) cho các tick thường đến hạn trong lúc đó (D6) | Một backfill không được âm thầm làm mất các lần chạy thường, và cách nó kết thúc không được quyết định chúng có chạy hay không |
| **Đọc từ mã nguồn.** Một backfill đang tạm dừng được đánh giá ở mỗi lượt rồi bị bỏ qua (`Schedule.java:342-345`); ngày kế tiếp của nó nằm yên trong quá khứ | Một backfill đang tạm dừng không được claim (D7) | Không tốn việc và không ghi dòng nào cho mỗi trigger tạm dừng mỗi giây |
| **Đọc từ mã nguồn.** Bật lại một trigger đang tắt có backfill: `ALL` giữ state (`TriggerEventHandler.java:263-265`), `LAST` trả về nguyên vẹn (`:283-288`), `NONE` và "skip" mặc định tính lại ngày kế tiếp từ `currentDate` của backfill và bỏ ngày cũ (`:268-270`, `:314-317`). Phép tính lại là strictly-after `currentDate`, tức tick **chưa bắn**, nên skip-enable làm mất một tick của cửa sổ. **Spec W7e §9 nói "skip missed chỉ di chuyển `next_evaluation_date`", trích `:286-289`: đó là nhánh `LAST`, và câu đó sai với skip** | Các ngày được giữ cho mọi lựa chọn; backfill tiếp tục ở tick nó đã dừng; lựa chọn áp dụng cho vị trí tiếp tục của schedule thường (D11) | Không mất tick nào của cửa sổ, và lựa chọn của người dùng vẫn mang nghĩa như dialog nói |
| **Đọc từ mã nguồn.** Cập nhật định nghĩa trigger tính lại ngày kế tiếp từ context, gồm cả backfill (`TriggerEventHandler.java:500-513`), nên cùng rule strictly-after làm bỏ qua tick hiện tại của cửa sổ | `sync` suy lại từ backfill tại hoặc sau tick hiện tại của nó (D10) | Cùng lý do |
| **Đọc từ mã nguồn.** Một trigger bị tắt (runtime hoặc YAML) không được fetch, nên backfill của nó bị đóng băng (`DefaultSchedulableTriggerFetcher.java:69, 104`). Create được chấp nhận | Cũng đóng băng. Create trên trigger đang tắt: 409 `trigger_disabled` | Không nên khởi động một backfill không thể chạy |
| **Đọc từ mã nguồn.** Không có dấu trên execution backfill: bản ghi trigger là của bản chạy thật (`SchedulableExecutionFactory.java:35`), và không có system label nào gọi tên một backfill (`core/.../models/Label.java:21-51`) | `execution_trigger` cũng là của bản chạy thật (hình dạng oracle), và execution mang `backfill_id` trong một cột (D2). Trang tổng quan hiện "Backfill" | egret cần dấu cho các unique index của mình; hiển thị nó thì miễn phí |
| **Đọc từ mã nguồn.** Label: của flow (đã bỏ system label), rồi của trigger và của backfill (đã render), rồi `system.from` và `system.correlationId`; khử trùng theo key, giá trị **cuối** thắng ở vị trí của key đầu (`SchedulableExecutionFactory.java:37-44, 94-109`; `LabelService.java:29-51`; `Execution.java:209-217`; `Label.java:70-93`). Giá trị label của backfill được render (`SchedulableExecutionFactory.java:102`) | Label của flow, rồi của backfill, key sau thắng (D3). Một giá trị label backfill có template bị từ chối lúc create. `labels:` riêng của trigger và `system.from` vẫn chưa được áp dụng (§10) | egret không có render context ở seam này, như với `Schedule.inputs` (`scheduler/fire.rs:427-442`) |
| **Đọc từ mã nguồn.** Input: `inputs` đã render của trigger, phủ bởi `inputs` đã render của backfill (`SchedulableExecutionFactory.java:76-88`) | Cùng phép phủ, chưa render: một giá trị có template trong map đã gộp bị từ chối lúc create (§3.4) và, như hiện nay, tiêu thụ tick lúc bắn (`fire.rs:256-258`) | Cùng lý do |
| **Đọc từ mã nguồn.** Pause, resume và delete hàng loạt theo trigger hoặc theo query, bất đồng bộ, `202` kèm operation id, trigger không biết bị bỏ (`TriggerController.java:248-337`). Không có create hàng loạt | Chỉ theo lựa chọn, tối đa 100, đồng bộ, có kết quả cho từng mục (D14); không có create hàng loạt | Lý do của W7e D10 |
| **Đọc từ mã nguồn.** UI: cột Backfill với "Backfill executions" hoặc tag running/paused (`FlowTriggers.vue:120-140`); một sub-row luôn mở với "start → end · N%", một thanh sọc (hổ phách khi paused), pause/resume và stop (`BackfillBanner.vue`); tiến độ = `(currentDate − start) / (end − start)`, làm tròn, kẹp, 100 khi cửa sổ rỗng (`BackfillBanner.vue:75-82`); form nhúng form Run (input, label) | Cùng cột, sub-row và công thức; thanh không có animation (FE-A11Y-5) và trạng thái được nói bằng chữ (FE-A11Y-4); "N of M ticks" bên cạnh phần trăm; delete hỏi trước (FE-ACTION-1) | Bộ rule |
| **Đọc từ mã nguồn.** Không giới hạn kích thước cửa sổ | Tối đa 10.000 tick, và `start` lùi tối đa mười năm (D8) | Một cửa sổ cạn mỗi lượt một tick (khoảng 1 s mỗi tick), và một năm gõ nhầm (`0026`) nếu không sẽ là cửa sổ 2.000 năm |

## 3. Backend

### 3.1 Bảng backfill (D1, D17)

Migration `0192_workflow_trigger_backfills.sql` (quyết định mặc định, 2026-10-15). Main kết thúc ở `0189`; nhánh chưa merge `feat/view-governance` giữ `0186`-`0188` (kiểm bằng `git ls-tree` trên mọi ref cục bộ và remote, 2026-10-15, và trên đĩa của mọi worktree). `0190` và `0191` do worktree `egret-tagdef` (nhánh `fix/tag-policies-service-def`) giữ (kiểm lại 2026-10-15 khi làm Task 2), nên gói này dùng `0192`, không va với cái nào ở mọi thứ tự merge; kiểm lại mọi ref và worktree trước khi merge, vì không gian số là dùng chung.

```sql
CREATE TABLE workflow.trigger_backfills (
  flow_id                       uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  -- The `Schedule` trigger's `id:` in the YAML.
  trigger_id                    text        NOT NULL,
  -- Copied from the flow for filtering; not an authority.
  workspace_id                  uuid        NOT NULL,
  -- This backfill's identity, written on every execution it fires
  -- (workflow.executions.backfill_id). A new backfill over the same window
  -- has a new id, so it re-runs those dates (user decision, 2026-10-15).
  id                            uuid        NOT NULL DEFAULT gen_random_uuid() UNIQUE,
  window_start                  timestamptz NOT NULL,
  window_end                    timestamptz NOT NULL,
  -- The next window tick to fire: equal to workflow.triggers.next_evaluation_date
  -- while the trigger row is on the queue. Not `current_date`, a reserved word.
  current_tick                  timestamptz NOT NULL,
  paused                        boolean     NOT NULL DEFAULT false,
  paused_by                     uuid        REFERENCES users (id) ON DELETE SET NULL,
  paused_at                     timestamptz,
  -- Overlaid on the schedule's `inputs:` key by key.
  inputs                        jsonb       NOT NULL DEFAULT '{}'::jsonb,
  -- [{key, value}], no `system.` key, in the order given.
  labels                        jsonb       NOT NULL DEFAULT '[]'::jsonb,
  -- The regular schedule's next date when the backfill was created. Written
  -- at create and re-snapped by `sync` on a cron change; never by pause/resume.
  previous_next_evaluation_date timestamptz NOT NULL,
  total_ticks                   integer     NOT NULL,
  ticks_done                    integer     NOT NULL DEFAULT 0,
  created_by                    uuid        REFERENCES users (id) ON DELETE SET NULL,
  created_at                    timestamptz NOT NULL DEFAULT now(),
  updated_at                    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id),
  CHECK (window_start < window_end),
  CHECK (current_tick >= window_start),
  -- Dòng bị xoá khi tick kế tiếp vượt quá window_end (§3.6, §3.7),
  -- và lệnh tạo từ chối cửa sổ không có tick nào (§3.4).
  CONSTRAINT workflow_trigger_backfills_tick_within_window CHECK (current_tick <= window_end),
  CHECK (jsonb_typeof(inputs) = 'object'),
  CHECK (jsonb_typeof(labels) = 'array'),
  CHECK (paused = (paused_at IS NOT NULL)),
  CHECK (total_ticks BETWEEN 1 AND 10000 AND ticks_done BETWEEN 0 AND total_ticks)
);
```

- **Vì sao không dùng cột trên `workflow.triggers`** (đề xuất đầu tiên của brief): `sync` xoá mọi dòng `workflow.triggers` của một flow mà source không parse được (`scheduler/sync.rs:113-119`, `keep` rỗng khi `parsed` là `None`, `:85-89, 93-98`). API không thể lưu một source như vậy (`parse_draft`, W7e §3.4), nhưng một thay đổi parser có thể làm một source đã lưu bị fail, và `reconcile` chạy `sync` trên mọi flow lúc khởi động. W7e đặt công tắc trong bảng riêng đúng vì lý do này (`migrations/0189_workflow_disabled_triggers.sql:5-11`); một backfill đang chạy là cùng loại state của người dùng. Không có foreign key tới `workflow.triggers`, vì cùng lý do.
- **`end` và `current_date` là từ khoá dành riêng** trong SQL, vì vậy có `window_start`, `window_end` và `current_tick`.
- **`ON DELETE CASCADE`** chỉ chạy khi xoá cứng; xoá mềm là việc của `sync` (§3.7).

### 3.2 Nhận diện một execution backfill (D2)

```sql
ALTER TABLE workflow.executions ADD COLUMN backfill_id uuid;
-- A backfill execution is a schedule fire: 0156's CHECK ties scheduled_date to it.
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_backfill_is_a_schedule_fire
  CHECK (backfill_id IS NULL OR scheduled_date IS NOT NULL);
DROP INDEX workflow.workflow_executions_one_per_schedule_tick;
CREATE UNIQUE INDEX workflow_executions_one_per_schedule_tick
  ON workflow.executions (flow_id, (execution_trigger->>'id'), scheduled_date)
  WHERE scheduled_date IS NOT NULL AND backfill_id IS NULL;
CREATE UNIQUE INDEX workflow_executions_one_per_backfill_tick
  ON workflow.executions (backfill_id, scheduled_date)
  WHERE backfill_id IS NOT NULL;
```

- **Hiện nay một tick backfill cho ngày đã chạy sẽ bị từ chối**: index của 0156 là `(flow_id, execution_trigger->>'id', scheduled_date) WHERE scheduled_date IS NOT NULL` (`migrations/0156_workflow_triggers.sql:49-51`); vi phạm được phân loại `AlreadyFired` (`scheduler/start.rs:96-116`) và tick bị tiêu thụ kèm một cảnh báo (`scheduler/fire.rs:265-273`). Quyết định của người dùng cần đưa backfill ra ngoài nó.
- **Vì sao là một cột chứ không phải một key trong `execution_trigger`:** bản ghi phải giữ nguyên bản mà Kestra lưu, thứ `schedule_trigger_record_matches_the_captured_execution` ghim với một artifact đã chụp (`scheduler/fire.rs:475-510`). Bản thân Kestra không thêm dấu nào (§2).
- **Vì sao là id của backfill chứ không phải một boolean:** `(backfill_id, scheduled_date)` chính xác là "một execution cho mỗi (trigger, backfill, tick)". Một boolean sẽ làm backfill thứ hai trên cùng cửa sổ va với cái đầu.
- **Không có foreign key** tới `workflow.trigger_backfills`: dòng bị xoá khi backfill kết thúc, còn các execution của nó vẫn ở lại.
- **Tính idempotent của một tick backfill.** Nó là một lần bắn schedule như mọi lần khác, nên ba câu lệnh của W12 giữ nguyên (`scheduler/mod.rs` "## 1." đến "## 3."): claim khoá dòng trigger `FOR UPDATE SKIP LOCKED` (`fire.rs:145-166`), nên hai replica không bao giờ đánh giá một trigger cùng lúc; việc insert execution và việc advance commit trong một transaction (`fire.rs:289-302`), nên một lượt được thử lại hoặc thấy advance hoặc làm lại toàn bộ tick; và `workflow_executions_one_per_backfill_tick` là chốt chặn mà `classify` ánh xạ sang `AlreadyFired` như nó ánh xạ index theo tick (một hằng số mới bên cạnh `ONE_PER_TICK`, `start.rs:72`).
- **Migration khoá `workflow.executions` ở mức ACCESS EXCLUSIVE từ câu `ALTER TABLE` đầu tiên cho tới khi commit**, chặn cả đọc lẫn ghi (danh sách execution, worker claim, scheduler): `ADD COLUMN`, `ADD CONSTRAINT` và `DROP INDEX` đều lấy khoá đó, giữ tới hết transaction. Khoảng chặn là chừng bốn lượt quét toàn bảng: hai lần kiểm CHECK, và hai lần dựng partial index, mỗi lần đọc toàn bộ heap. Việc dựng lại không dùng `CONCURRENTLY`, vì sqlx chạy file trong một transaction, nên không có khoảnh khắc nào thiếu cả hai index. (Ghi chú của 0189, `0189:43-48`, tả một khoảng hẹp hơn cho index của chính nó; nó không áp dụng ở đây.)
- **Phần văn xuôi của 0156 không bị sửa** (văn xuôi migration là chỉ-thêm); 0192 nêu vị từ mới. Doc của câu lệnh 2 trong `scheduler/mod.rs` ("the unique index is the backstop") gọi tên cả hai index.

### 3.3 Label của execution (D3)

```sql
-- The labels this execution was started with beyond its flow's own `labels:`
-- ([{key, value}], Kestra's list shape). NULL: none. The flow's labels are
-- read from `source`, which is the execution's frozen copy.
ALTER TABLE workflow.executions ADD COLUMN labels jsonb
  CONSTRAINT workflow_executions_labels_are_a_list
  CHECK (labels IS NULL OR jsonb_typeof(labels) = 'array');
```

**Hiện nay** một execution không có cột labels (`migrations/0134_workflow_executions.sql` và mọi `ALTER` sau đó). `labels` mà một lần chạy render là `labels:` của chính flow cộng `system.correlationId` (`worker/render.rs:247-252`, `:504`); bộ xử lý Flow-trigger đọc label đã khai báo của **flow nguồn** cho các điều kiện label của `dependsOn` và `executionLabels` (`flow_trigger/process.rs:795-797, 1080-1089, 1348`); và không route hay trang nào hiện hoặc lọc label của execution (`ExecutionListItemDto`, `routes.rs:1417-1464`, không có; frontend không có bộ lọc label trên execution). `labels:` riêng của trigger được parse (`models/trigger.rs:51`) và không được áp dụng vào đâu cả.

- **Phép gộp, trong một hàm** (`labels::execution_labels(flow: &[Label], own: &[Label]) -> Vec<Label>`): label của flow, rồi của execution, khử trùng theo key, giữ giá trị **cuối** ở vị trí của key **đầu**, bỏ key hoặc value rỗng. Đó là `Execution.newExecution` + `Label.deduplicate` của Kestra (§2).
- **Render:** `Surface::load` (`worker/render.rs:168-178`) cũng select `labels`; map ở `:247-252` được dựng từ phép gộp. Key `system` vẫn được chèn sau cùng (`:504`), như hiện nay.
- **Nguồn Flow-trigger:** `load_source` (`flow_trigger/process.rs:778`) đọc cột và gộp tương tự, nên một điều kiện `dependsOn` ở hạ nguồn với `labels:` và `{{ trigger.executionLabels }}` thấy label của backfill, như trong Kestra. Divergence đã ghi ở W13 spec §5.3 bước 4 ("egret has no execution labels") thu hẹp lại thành các execution không có label riêng.
- **Được sao chép** ở những chỗ Kestra sao chép label của execution: route replay (`routes.rs:2941-2959`), một retry `CREATE_NEW_EXECUTION` (`worker/replay.rs:243`, `Execution.childExecution`, `Execution.java:416`) và một vòng lặp iteration (`worker/spawn.rs:393`, `Execution.loopExecution`, `Execution.java:448`). **Không** sao chép cho child subflow: `inheritLabels` hiện chưa được tôn trọng (`worker/subflow.rs:79-82`), không đổi. `backfill_id` không bao giờ được sao chép (một replay không có `scheduled_date`, và CHECK sẽ từ chối).
- **`NewExecution`** (`worker/claim.rs:2605-2654`) thêm `labels: Option<serde_json::Value>` và `backfill_id: Option<Uuid>`; mọi chỗ dựng khác truyền `None` trừ route replay, truyền `labels` của nguồn. "Mọi chỗ dựng khác" nghĩa là **mọi chỗ dựng, ở src lẫn tests, được liệt kê trong plan** (các chỗ nêu ở đây, `routes.rs:2668, 2944`, `worker/claim.rs:2572` và `flow_trigger/process.rs:996`, chỉ là các chỗ ở src).
- **Hiển thị (D18):** `ExecutionListItemDto` thêm `labels` (cột như đã lưu, label riêng của execution ngoài label của flow) và `backfill_id`. Phép chiếu của danh sách cố ý bỏ `source` (`repo.rs:955-968`, `execution_cols!`), mà label của flow chỉ nằm trong `source`, nên một danh sách đã gộp sẽ phải chọn và parse một tài liệu YAML cho mỗi dòng. `ExecutionDto` (chi tiết, có `source`) thêm `effective_labels`, phép gộp ở trên. Trang tổng quan execution hiện `effective_labels` dưới dạng chip `key: value` và, trong khối trigger của nó (`frontend/src/routes/workflows/ExecutionOverview.tsx:139-160`), "Backfill" bên cạnh trigger id khi `backfill_id` được đặt. Lọc execution theo label là việc tiếp theo (§10).

### 3.4 Tạo một backfill

`POST /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill`, `workflow:manage`, giới hạn body 64 KiB.

```json
{ "start": "2026-10-01T00:00:00Z", "end": "2026-10-07T00:00:00Z",
  "inputs": { "region": "eu" }, "labels": [{ "key": "reason", "value": "replay outage" }] }
```

`end` là tuỳ chọn (vắng = `now()` của transaction, mặc định của Kestra, `TriggerState.java:233`); `inputs` và `labels` là tuỳ chọn. `labels` chấp nhận dạng danh sách hoặc map của Kestra qua một bộ deserialize duy nhất, `deserialize_labels` ở `models/flow.rs:375`, được nới từ `pub(super)` thành `pub(crate)` cho việc này (D24).

**Trong một transaction, theo thứ tự lock của W7e** (`triggers/switch.rs:5-22`):
1. Dòng flow `FOR NO KEY UPDATE`, với vị từ workspace (`switch.rs:105-117`). Không tìm thấy hoặc ở thùng rác: 404 `not_found`.
2. Parse; trigger đầu tiên có id này (`switch.rs:120-126`). Không parse được: 400 `source_does_not_parse`. Không có id đó: 404 `not_found`. Không phải `Schedule`: 409 `backfill_not_supported`.
3. Bị tắt hiệu lực (YAML của flow, YAML của trigger, công tắc runtime, hoặc `state = 'disabled'`, như entry tính, `triggers/entry.rs:83-87`): 409 `trigger_disabled` (D9).
4. Dòng `workflow.triggers` `FOR UPDATE`. Vắng, hoặc `next_evaluation_date IS NULL` (rời hàng đợi; `ScheduleTrigger::resolve` từ chối nó, hoặc cron không còn tick tương lai): 409 `trigger_off_queue`, kèm `last_error`.
5. Validate (code ở §5), với `now()` của transaction và schedule đã resolve:
   - `start < end` (`backfill_window_invalid`); `end <= now` (`backfill_end_in_future`); `start >= now − 10 years` (`backfill_start_too_early`, quyết định mặc định: một năm hai chữ số gõ vào control ngày là nguyên nhân thực tế).
   - Các tick của cửa sổ, đếm bằng `ScheduleTrigger::ticks_through(start, end, 10_000)` (`trigger/schedule.rs:1429-1446`, bao gồm cả hai đầu): không có tick là `backfill_no_ticks`; hơn 10.000 là `backfill_too_many_ticks` (quyết định mặc định: ở lượt mặc định một giây, `crates/egret-config/src/lib.rs:691-696, 1396-1398`, 10.000 tick cạn trong dưới ba giờ; một cron mỗi phút phủ một tuần). Số đếm được lưu làm `total_ticks`.
   - **Input:** một object JSON (rule của `start_inputs`, `routes.rs:2306-2320`). Không key nào được gọi tên một input `FILE` (`backfill_inputs_invalid`: backfill không có upload, và một URI đã lưu thuộc về execution khác). Map **đã gộp** (`inputs:` của schedule phủ bởi các input này) phải qua `refuse_templated` (`fire.rs:430-442`), nên một input schedule có template phải được ghi đè. Mọi input đã khai báo có `required` không phải false và không có `defaults` phải có giá trị trong map đã gộp (`models/input.rs:54-62`), đúng như UI của Kestra kiểm (`FlowTriggers.vue:518-530`).
   - **Label:** không key nào bắt đầu bằng `system.` ở bất kỳ kiểu chữ hoa/thường nào (`@NoSystemLabelValidation` của Kestra), không key rỗng hoặc chỉ có khoảng trắng, không value rỗng, không key nào xuất hiện hai lần trong request, không cú pháp template trong value, tối đa 50 label, key tối đa 128 ký tự (Unicode scalar value, không phải byte) và value tối đa 1.024 (`backfill_labels_invalid`). Việc không phân biệt hoa/thường của tiền tố, key chỉ có khoảng trắng và key trùng là quyết định mặc định (2026-10-15): chúng giữ label lưu trữ không mơ hồ (key trùng sẽ bị phép gộp âm thầm gộp lại).
6. Insert dòng backfill với `ON CONFLICT (flow_id, trigger_id) DO NOTHING`; không dòng nào được insert: 409 `backfill_exists` (D8).
7. **Attach**, qua mô hình thuần: `TriggerState::attach_backfill` (`trigger/mod.rs:270-282`) với start bao gồm (D4): tick đầu tiên **tại hoặc sau** `start` (một `ScheduleTrigger::first_tick_at_or_after` mới, tức `next_after(at − 1 ns)` mà `ticks_through` đã dùng). `current_tick` và `next_evaluation_date` được đặt bằng nó; `previous_next_evaluation_date` là `next_evaluation_date` của dòng trước lần ghi này (D5); `evaluated_at` và `last_error` để yên (Kestra giữ `evaluatedAt`).
8. Đọc entry trong transaction, commit, audit (§3.10). 201 kèm entry.

**Vì sao claim hiện có sau đó chạy nó:** `next_evaluation_date` mới nằm trong quá khứ, nên dòng đến hạn ở lượt kế tiếp và được claim bởi cùng `WHERE t.next_evaluation_date <= now()` (`fire.rs:150`) qua cùng due index (`0156:35-36`). Không có vòng lặp thứ hai, ở Kestra hay ở đây: backfill **chính là** luồng ngày kế tiếp duy nhất của trigger (`TriggerEventHandler.java:177-179`).

**Các tick thường trong lúc backfill** không bắn: chỉ có một luồng, và nó đang đi qua cửa sổ. Điều xảy ra với chúng khi nó kết thúc là §3.6.

**`attach_backfill` đổi** sang start bao gồm; cửa sổ trong test của nó bắt đầu ở `00:00` trên cron `0 2 * * *` (`trigger/mod.rs:848-853, 898, 931-936, 1163-1168`), nên không test nào đổi. Một test thuần mới ghim việc một tick đúng ở `start` được bắn.

### 3.5 Pause, resume, delete

- `PUT …/flows/{id}/triggers/{trigger_id}/backfill/paused`, body `{"paused": bool}`; `DELETE …/flows/{id}/triggers/{trigger_id}/backfill`. Cả hai `workflow:manage`, cả hai trả entry.
- Cùng hình dạng transaction: flow `FOR NO KEY UPDATE`, parse, trigger, dòng `workflow.triggers` `FOR UPDATE` (vắng cũng được: source không parse được rồi đã được sửa, hoặc trigger rời hàng đợi), rồi dòng backfill `FOR UPDATE`. Không có: 404 `no_backfill`. Các từ chối ở bước 1-2 giống của create; một trigger đang tắt **không** bị từ chối (một backfill đóng băng có thể được pause hoặc delete).
- **Pause / resume** ghi `paused`, `paused_by`, `paused_at` và `updated_at` trên dòng backfill và không gì khác. Đã ở trạng thái được yêu cầu: 200, không ghi, không audit (tính idempotent của W7e, `switch.rs:190-195`).
  - Khi tạm dừng, claim bỏ qua nó (D7): claim thêm `AND NOT EXISTS (SELECT 1 FROM workflow.trigger_backfills b WHERE b.flow_id = t.flow_id AND b.trigger_id = t.trigger_id AND b.paused)`. Hiện nay một backfill tạm dừng sẽ đến hạn mãi mãi, bị claim mỗi lượt, bị bỏ qua với `BackfillPaused`, và dòng của nó bị ghi lại (`trigger/mod.rs:927-975` cho thấy điểm cố định; Kestra trả cùng chi phí đánh giá).
  - `next_evaluation_date` của trigger vẫn bằng `current_tick` khi tạm dừng, nên resume không cần ghi gì khác: lượt kế tiếp claim dòng và bắn `current_tick`. Đó là "re-queue".
  - Các tick thường cũng không bắn khi backfill đang tạm dừng; §3.6 áp dụng khi nó kết thúc.
- **Delete** xoá dòng backfill và tiếp tục schedule thường như §3.6 nói, với vị trí tiếp tục `max(previous_next_evaluation_date, current_tick)`: các tick chưa bắn của backfill bị bỏ. **Giới hạn đã biết:** khi schedule thường đã quá hạn lúc tạo (`previous_next_evaluation_date` trước đầu cửa sổ), phần backlog đó nằm trước đầu cửa sổ không được phục hồi, vì `max(...)` bắt đầu từ `current_tick`; backlog thường nằm trong khoảng của cửa sổ vẫn được phục hồi. Ghi ở §10.

### 3.6 Bắn, kết thúc và schedule thường (D6, D13)

**`fire_one`** (`scheduler/fire.rs:180-303`):
- Sau claim, nó đọc dòng backfill bằng một câu lệnh mới. Khoá dòng của claim trên `workflow.triggers` là thứ tuần tự hoá mọi writer của dòng backfill (§3.11), nên một lần đọc mới là hiện hành. (Một claim mà EvalPlanQual kiểm lại đọc `t` mới nhất nhưng các bảng khác theo snapshot của nó, `fire.rs:59-61`; lần đọc mới tránh phải dựa vào điều đó.) Một backfill bị phát hiện đang tạm dừng ở đây (một pause commit giữa snapshot của claim và khoá của nó) không ghi gì và trả `Frozen`.
- `TriggerState.backfill` được dựng từ dòng (`window_start`, `window_end`, `current_tick`, `paused: false`) thay cho `None` (`fire.rs:226-233`).
- `inputs` = `inputs:` của schedule (`fire.rs:224`) phủ bởi của backfill, rồi `refuse_templated` trên kết quả (`:225`). `tick` không đổi; `ScheduledExecution.from_backfill` của nó (`trigger/mod.rs:333-335, 473`), hiện chưa được đọc, quyết định `start` (`fire.rs:314-346`) có truyền `backfill_id` và `labels` của backfill vào `NewExecution` hay không.
- `UPDATE` cuối (`fire.rs:289-301`) được cộng, trong cùng transaction, thêm việc ghi dòng backfill: state của `tick` vẫn còn backfill → `UPDATE … SET current_tick = $next, ticks_done = LEAST(ticks_done + 1, total_ticks)` (D21: số `total_ticks` và lần đi dùng cùng dãy `Cron::next_after`, nên cái kẹp này lẽ ra không bao giờ cắn, nhưng một lần ghi không kẹp mà vi phạm CHECK của bảng sẽ làm hỏng transaction của trigger đã claim và kết thúc mọi lượt, dừng mọi schedule của cả deployment); không còn (tick kế tiếp vượt `end`, `trigger/mod.rs:290-310`) → `DELETE` dòng và thay ngày kế tiếp của `tick` (tick đầu tiên sau bây giờ, `trigger/schedule.rs:1542-1549`) bằng **vị trí tiếp tục** dưới đây.
- Các đường return sớm không đổi và để yên dòng backfill: `record_error` giữ các ngày và lượt kế tiếp thử lại cùng tick của cửa sổ (`fire.rs:282-285, 350-362`); `take_off_queue` đóng băng trigger, gồm cả backfill, đến lần save kế tiếp re-queue nó từ backfill (§3.7); `AlreadyFired` tiêu thụ tick (`:265-273`) và advance backfill.
- **Start bị từ chối thì kết thúc backfill (D25).** `Started::Refused` trên một tick backfill (một input ghi đè mà start từ chối, một loại task không được phục vụ, một source không plan được) là tất định, và một lần save sẽ suy lại đúng tick đó với đúng input đó (§3.7), nên đưa trigger ra khỏi hàng đợi sẽ làm nó kẹt đến khi người dùng xoá backfill. Thay vào đó, trong cùng transaction, dòng backfill bị xoá và schedule thường tiếp tục y như khi xoá (`from = max(previous_next_evaluation_date, current_tick)`, rồi áp policy), với `last_error = "backfill ended: <lý do từ chối>"`. Trigger vẫn ở trong hàng đợi trừ khi lần tiếp tục không tìm được tick. `Failed` tạm thời vẫn thử lại cùng tick. Từ chối trên một tick thường không đổi (`take_off_queue`).

**Tiếp tục (D6).** Một hàm thuần bên cạnh `tick`, `TriggerState::resume_after_backfill(trigger, from, policy, now) -> Option<DateTime<Utc>>`, dùng bởi hoàn tất và bởi delete:
- `from`: khi hoàn tất, `max(previous_next_evaluation_date, tick đầu tiên sau window_end)`; khi delete, `max(previous_next_evaluation_date, current_tick)`. Schedule thường tiếp tục từ chỗ nó đã đứng; một backlog thường mà backfill đã phủ rồi thì không chạy hai lần. **Giới hạn đã biết:** `max(...)` không bắt đầu sớm hơn cửa sổ (tick đầu tiên sau `window_end` khi hoàn tất, `current_tick` khi delete), nên khi schedule thường đã quá hạn lúc tạo, phần backlog của nó nằm trước đầu cửa sổ không được phục hồi (§10).
- `policy`: `recoverMissedSchedules` của trigger, đọc như W7e đọc (`RecoverMissed::from_model`, `trigger/schedule.rs:1271-1281`); giá trị nó không biết là `ALL`, hành vi của egret mỗi khi nó không áp dụng thuộc tính (W7e §10).
- `from > now`: `from` (không lỡ gì). Ngược lại `ALL`: `from` (mỗi tick thường bị lỡ chạy, một cái mỗi lượt); `LAST`: tick muộn nhất tại hoặc trước bây giờ (`last_tick_at_or_before`, `schedule.rs:1421-1425`); `NONE`: tick đầu tiên sau bây giờ. Không có tick nào: rời hàng đợi với `never_fires` (`scheduler/sync.rs:33-38`), như route công tắc làm (`switch.rs:282-300`). Khi kết quả của tick cuối đã đặt một `last_error`, cả hai được giữ, `never_fires` trước (`"<never_fires>; <kết quả>"`): đó là trạng thái dòng sẽ ở lại.
- **Vì sao không theo Kestra:** Kestra bỏ qua chúng khi hoàn tất và phát lại khi delete (§2). Cách thứ nhất làm mất các lần chạy người dùng không hề muốn mất (một backfill ba giờ trên schedule hằng giờ tốn ba lần chạy), và kết quả phụ thuộc vào việc người dùng có chờ hay không. egret phát lại chúng theo thiết lập của chính trigger, cũng là cái mà dialog bật lại đề nghị.

**Thông lượng (D13).** Một tick của cửa sổ cho mỗi trigger cho mỗi lượt, vì danh sách `seen` của `fire_due` làm "tối đa một lần mỗi lượt" thành thuộc tính của truy vấn (`fire.rs:79-136`). Một cửa sổ N tick mất khoảng N lượt, N giây ở khoảng mặc định: 10.000 tick, dưới ba giờ. Bắn nhiều tick mỗi lượt sẽ giữ dòng trigger và transaction cho cả một lô, và một render miss rollback toàn bộ lần thử (`fire.rs:91-104`), nên một lô sẽ làm lại việc. Kestra cũng bắn một tick mỗi lần đánh giá (W7e §2 quan sát `ALL` phát lại cách nhau khoảng 1,6 s). Form nói cửa sổ sẽ mất bao lâu.

**An toàn.** `tick` không bao giờ thấy một backfill tạm dừng trong vòng lặp, nên nhánh bỏ qua `BackfillPaused` không thể tới được ở đó; nó vẫn còn cho mô hình thuần. `NotDue`/`NotSchedulable` không thể đến từ một backfill (ngày của nó đã qua và trigger bị tắt thì không được claim), nên debug panic ở `fire.rs:241-254` không đổi.

### 3.7 `sync` và backfill (D10)

`sync` (`scheduler/sync.rs:78-200`) chạy bên trong mọi lần save, `reconcile` và `resync` của một lượt, mỗi cái giữ dòng flow (`fire.rs:382-425`).

- **Khởi tạo lại.** Hiện nay một dòng có `spec_key` đổi, hoặc rời hàng đợi, nhận tick kế tiếp sau bây giờ và `evaluated_at = NULL` (`sync.rs:150-165`), và một dòng mới cũng vậy. Với một dòng backfill cho trigger đó, ngày mới được **suy lại từ backfill**: tick đầu tiên của cron (mới) tại hoặc sau `current_tick`. Nếu nó sau `window_end`, backfill đã hoàn tất: dòng bị xoá và ngày là vị trí tiếp tục (§3.6). Ngược lại `current_tick` nhận ngày mới, `total_ticks` trở thành `ticks_done` cộng số tick còn lại (`ticks_through(current_tick, window_end)`), và `previous_next_evaluation_date` được snap lại về tick đầu tiên của cron mới tại hoặc sau nó. Hai trường hợp biên (quyết định mặc định, 2026-10-15):
  - **Vượt giới hạn (D19).** Nếu phép đếm lại vượt `CHECK (total_ticks BETWEEN 1 AND 10000)` của bảng (một cửa sổ hằng ngày dài một tuần thành 10.080 tick với `* * * * *`), CHECK sẽ làm lần save thất bại với 500, mà `sync` không bao giờ được làm hỏng một lần save vì backfill. Nên `sync` kết thúc backfill: xoá dòng, cho schedule thường tiếp tục như một lần delete (§3.5, §3.6), ghi `last_error = "the backfill ended: the new schedule gives more than 10000 ticks in its window"` và log mức `warn`.
  - **Không có tick để snap (D20).** Một cron mới không có tick tại hoặc sau `previous_next_evaluation_date` thì không cho gì, mà cột là `NOT NULL`: giá trị đã lưu được giữ, và lần tiếp tục (§3.6) sau đó không tìm được tick và đưa dòng ra khỏi hàng đợi với `never_fires`.

  Kestra cũng giữ backfill khi định nghĩa đổi (`TriggerEventHandler.java:500-513`); phép tính lại strictly-after của nó sẽ bỏ tick hiện tại (§2).
- Một schedule không còn resolve được (`lateMaximumDelay`, `when`, `trigger/schedule.rs:1298-1327`) rời hàng đợi như hiện nay; backfill của nó được giữ và đóng băng đến một lần save resolve được.
- **Quyết định chuyển sang Rust.** `CASE` của upsert (`sync.rs:157-165`) không thấy bảng backfill, nên `sync` đọc các dòng `workflow.triggers` của flow `FOR UPDATE` và các dòng backfill của nó trước upsert, quyết định từng ngày, và ghi nó. Thứ tự lock vẫn là flow, các dòng trigger, các dòng backfill (§3.11).
- **Vòng đời,** sau các dòng trigger, bên cạnh các dòng công tắc của W7e (`sync.rs:181-200`):
  - source parse được: xoá các backfill có `trigger_id` không còn là một **`Schedule`** được khai báo (id bị gỡ hoặc đổi kind: state của schedule mất, như Kestra xoá trigger state, `TriggerEventHandler.java:535-541`);
  - `gone` (xoá mềm): xoá hết (cascade không chạy);
  - source không parse được: giữ tất cả. Khi nó parse lại được, dòng trigger được tạo lại và suy lại từ backfill như trên.
- Đã xác nhận chấp nhận được (quyết định mặc định, 2026-10-15): một trigger id bị gỡ hoặc đổi kind kết thúc backfill của nó mà không tiếp tục, vì không còn schedule nào để tiếp tục. Các execution đã bắn vẫn ở lại.

### 3.8 Bật lại trong lúc có backfill (D11)

Bước 7 của W7e (`triggers/switch.rs:232-304`) tính "skip" bằng `TriggerState::initialise`, vốn không có backfill (`:250-255`), và ghi nó vào `next_evaluation_date` (`:269-281`). Với một backfill đã gắn, điều đó sẽ chỉ luồng vào tương lai trong khi `current_tick` vẫn ở quá khứ: cửa sổ sẽ không bao giờ được đi qua, và tick thường kế tiếp sẽ xoá nó như đã xong.

- Với một dòng backfill, bước 7 giữ `next_evaluation_date` (= `current_tick`) cho mọi lựa chọn, nên backfill tiếp tục ở tick nó đã dừng. Không mất tick nào của cửa sổ (skip của Kestra mất một, §2).
- Lựa chọn áp dụng cho vị trí tiếp tục của schedule thường thay vào đó: "skip" (hoặc `NONE`) chuyển `previous_next_evaluation_date` sang tick đầu tiên sau bây giờ; "recover" với `ALL` hoặc `LAST` để nguyên, và §3.6 áp dụng chính sách khi backfill kết thúc. `recover_missed_unknown` được kiểm như hiện nay.
- `moved_to` của câu trả lời (`switch.rs:42-43`) khi đó là `null`; audit event mang vị trí tiếp tục đã chuyển thay vào đó.

### 3.9 Đọc trigger (D12, D15)

`read_state` (`triggers/state.rs:119-164`) đọc `workflow.trigger_backfills` cho các flow trong phạm vi, với tên hiển thị của người tạo và người pause như nó đọc `disabled_by_name` (`:126-133`). Bộ dựng entry (`triggers/entry.rs:65-218`) vẫn thuần.

`TriggerEntryDto` (`triggers/routes.rs:158-192`) thêm:

| Trường | Ý nghĩa |
|---|---|
| `backfill` | `null`, hoặc `{id, start, end, current_tick, paused, paused_by_name, paused_at, inputs, labels, created_by_name, created_at, previous_next_evaluation_date, total_ticks, ticks_done, progress}` |
| `backfill.progress` | 0-100, công thức của Kestra: `(current_tick − start) / (end − start)`, làm tròn và kẹp, 100 cho khoảng rỗng (`BackfillBanner.vue:75-82`) |
| `can_backfill`, `backfill_refusal` | nút Backfill có dùng được không, và vì sao không, cái đầu tiên áp dụng: `flow_deleted`, `not_schedule`, `forbidden` (không có `workflow:manage`), `disabled`, `off_queue`, `running` (đã có một cái) |

- **`schedule.missed_ticks`** (`triggers/entry.rs:226-238`) hiện đếm từ `next_evaluation_date` đến bây giờ. Trong lúc backfill, ngày đó là một tick quá khứ của cửa sổ, nên nó sẽ đếm mọi tick từ đó đến bây giờ, gồm cả tick sau `end` sẽ không bao giờ chạy, và dialog bật (`frontend/src/components/workflows/triggers/triggersModel.ts:100-105`) sẽ đề nghị "recover" chúng. Với backfill, nó đếm từ `previous_next_evaluation_date`: các tick thường đang chờ backfill kết thúc, mà §3.6 và §3.8 quyết định.
- **Danh sách toàn cục** (`GET …/workflows/triggers`) mang cùng các trường cho mỗi dòng và thêm bộ lọc `backfill`: `running`, `paused` hoặc `any` (trong bộ nhớ, như `kind` và `state`, `triggers/routes.rs:380-386`).
- **Execution gần nhất** (`triggers/state.rs:32-40`) gồm cả execution backfill, như cột "Execution Id" của Kestra; trang tổng quan đánh dấu chúng.
- **Khả năng nhìn thấy:** input và label của backfill được hiện cho `workflow:read`, như input của mọi execution đã được hiện (`ExecutionListItemDto.inputs`, `routes.rs:1436`).

### 3.10 Route, hàng loạt, audit (D14)

| Route | Trả lời |
|---|---|
| `POST …/flows/{id}/triggers/{trigger_id}/backfill` | 201, entry |
| `PUT …/flows/{id}/triggers/{trigger_id}/backfill/paused` `{paused}` | 200, entry |
| `DELETE …/flows/{id}/triggers/{trigger_id}/backfill` | 200, entry |
| `PUT …/workflows/triggers/backfill/paused` `{triggers: [{flow_id, trigger_id}], paused}` | 200, `BulkResultsDto` |
| `POST …/workflows/triggers/backfill/delete` `{triggers}` | 200, `BulkResultsDto` |

- Đăng ký bên cạnh của W7e (`routes.rs:3330-3350`); tất cả `workflow:manage`. Hàng loạt: tối đa 100 (400 `too_many_triggers`), 64 KiB (`MAX_BULK_TRIGGERS_BODY_BYTES`, `triggers/routes.rs:640-643`), một transaction cho mỗi mục, một từ chối được ghi lại và mục kế tiếp vẫn chạy, một flow lạ là `not_found` (W7e §3.7). Delete hàng loạt là `POST` vì `DELETE` có body không portable.
- Lỗi đi qua `TriggerRouteError::Coded` (`triggers/routes.rs:258-290`); enum từ chối là một `BackfillRefusal` mới với `refusal_parts` riêng, như của W7e (`:740-776`).
- **Audit:** `workflow_trigger:backfill_create` (cửa sổ, số tick, key của input và key của label, không phải value), `:backfill_pause`, `:backfill_resume`, `:backfill_delete` (kèm ngày tiếp tục), resource `workflow_trigger`, id `{flow uuid}/{trigger_id}`, chỉ khi có thay đổi thật. Hoàn tất là việc của scheduler và không được audit.

### 3.11 Đồng thời

Mọi writer của một dòng backfill giữ, theo thứ tự: dòng flow (`FOR NO KEY UPDATE` cho các route và một lần save, `FOR SHARE` cho `reconcile`, `FOR SHARE SKIP LOCKED` cho `resync`), rồi dòng `workflow.triggers` của trigger, rồi dòng backfill. Một lượt giữ dòng trigger từ claim và ghi dòng backfill trong transaction của nó.
- **Route với một lượt:** route chờ dòng trigger; lượt chỉ lấy `KEY SHARE` trên flow (insert execution của nó), thứ `FOR NO KEY UPDATE` không chặn. Không có chu trình (W7e §3.2). Sau khi một pause hoặc delete trả lời, không tick backfill mới nào bắn.
- **Route với một lần save:** tuần tự hoá trên dòng flow, nên `sync` không bao giờ suy lại từ một backfill đang được tạo hoặc xoá.
- **Hai lần create:** tuần tự hoá trên dòng flow; lần thứ hai thấy dòng của lần đầu và trả 409.
- **Một dòng trigger không tồn tại** (source không parse được): không gì bắn nó, và chỉ `sync` (giữ flow) hoặc một route (giữ flow) mới đụng được backfill.

### 3.12 Các comment cần sửa

- `lib.rs:99-102`: "backfill has no route yet (W7f)" gọi tên các route thay vào đó.
- `scheduler/fire.rs:261-264` và câu lệnh 2 trong `scheduler/mod.rs`: chốt chặn là hai index.
- `trigger/mod.rs:229-241` (`TriggerState`): nhà database của backfill là `workflow.trigger_backfills`; `Backfill` thuần vẫn rút gọn thành cửa sổ (`schedule.rs:1628-1639`), input và label của nó sống bên cạnh.
- Gạch đầu dòng thứ hai của W7e spec §9: được sửa bởi dòng bật lại ở §2 (không sửa tại chỗ; spec này thay thế nó).

## 4. Frontend

Mọi thứ vẫn nằm trong khu vực Workflows lazy của W7a (FE-ROUTE-2). Client được sinh lại bằng `npm run gen:api` (FE-DATA-1); key trong `routes/workflows/queryKeys.ts` (FE-DATA-2). Module mới trong `components/workflows/triggers/`: `BackfillDialog.tsx`, `BackfillStrip.tsx`, `LabelsEditor.tsx`, `useBackfill.ts`; helper thuần trong `triggersModel.ts`.

### 4.1 Tab Triggers và trang toàn cục

- **Một cột Backfill** bên cạnh Enabled (W7e §9), trên cả hai bảng (`TriggersTable.tsx:105-152`):
  - một `Schedule` không có backfill: nút "Backfill" (`ReasonButton`, bị vô hiệu hoá kèm `backfill_refusal` của server bằng lời, FE-ACTION-4: "Only schedules can be backfilled", "You need workflow:manage", "Enable the trigger to backfill it", "Off the queue: {last_error}", "The flow is in the trash");
  - một backfill đang chạy: "Running" kèm icon; một cái đang tạm dừng: "Paused" kèm icon khác (FE-A11Y-4); các kind khác: trống.
- **Thanh tiến độ,** một `TableRow` thêm dưới dòng của trigger, trải qua các cột (sub-row luôn mở của Kestra): "{start} → {end}", "{progress}% · {ticks_done} of {total_ticks} ticks", một thanh `ui/progress` (không animation, FE-A11Y-5; thanh tạm dừng dùng token muted và chữ "Paused"), nút "Pause"/"Resume" và "Delete". Delete mở một confirm (FE-ACTION-1): "Delete this backfill? Its remaining {n} ticks will not run. Executions already started stay." Thanh của một trigger đang tắt nói "Frozen while the trigger is off".
- **Trạng thái** (`triggersModel.ts:43-62`): sau các nguyên nhân YAML, công tắc, flow và rời hàng đợi, "Backfilling ({progress}%)" hoặc "Backfill paused ({progress}%)".
- **Dialog bật** (`EnableTriggerDialog.tsx`): với một backfill, thêm một dòng: "The backfill resumes at {current_tick}. This choice is about the regular runs that fell due." `missed_ticks` đã có nghĩa là các tick thường (§3.9).
- **Hàng loạt ở trang toàn cục** (`routes/workflows/TriggersList.tsx:331-350`): "Pause backfills", "Resume backfills", "Delete backfills" (confirm) trên phần đã chọn; kết quả theo từng mục như của W7e (`no_backfill` đọc là "had no backfill"). Một facet `backfill` (Running, Paused, Any) trong URL (FE-PAGE-2).
- **Làm mới:** poll 30 s của tab (`FlowTriggersTab.tsx:27`) giữ nguyên; trong lúc một backfill của flow đang hiển thị chạy, tab poll mỗi 5 s để thanh chuyển động. Mỗi hành động invalidate cả hai key, như `useTriggerSwitch` làm (`useTriggerSwitch.ts:68-76`), và chỉ toast cho hàng loạt (FE-ACTION-2).

### 4.2 Drawer chi tiết

`TriggerDetailsDrawer.tsx` thêm một mục "Backfill" dưới Schedule: cửa sổ (theo múi giờ của trigger và của người xem), tick hiện tại, tiến độ, ai tạo và khi nào, ai pause và khi nào, input (bảng key/value) và label (chip), và "Regular runs resume from {previous_next_evaluation_date}" kèm điều `recoverMissedSchedules` sẽ làm với những tick đang chờ ("{n} regular runs are waiting; when the backfill ends, each will run", theo chính sách, dùng lại `recoverDescription`, `triggersModel.ts:107-113`).

### 4.3 Form tạo (D16)

`BackfillDialog`, được tải qua `lazy()` từ cả tab và trang toàn cục (nó kéo theo mô hình của form Run và thư viện `yaml`, `run/inputModel.ts:1`), và được liệt kê trong `scripts/assert-workflows-lazy.mjs` với một marker, đánh dấu bằng một cờ `backfill` mới, không phải `editor` (D23): `editor` chỉ kiểm marker nằm ngoài chunk của danh sách flow (`assert-workflows-lazy.mjs:383-392`) và vẫn qua nếu `FlowTriggersTab.tsx` import dialog tĩnh, còn `backfill` giữ marker khỏi cả những gì tải cùng các trang flow (`FLOW_PAGE_HOSTS`, như `files`, `:361-372`), đúng nghĩa "lazy chunk" của D16.
- **Tiêu đề** "Backfill {trigger id}"; một mô tả: "Runs this schedule for each of its ticks from start to end, both included, one about every second. Regular runs wait until it ends."
- **Start và End:** `datetime-local` gốc (như `KvValueControl.tsx:65` và các control của form Run, `run/inputControls.tsx:30`), `step="1"`, `max` = bây giờ (đọc khi dialog mở và ở mỗi thay đổi), mỗi cái có một `Label` gắn kèm (FE-FORM-2). End có thể để trống ("Leave empty to end now"). Giá trị là giờ cục bộ của trình duyệt, đổi sang UTC; dưới các trường: "In {trigger timezone}: {start} → {end}", và số tick của server sau một lỗi 400 gọi tên nó.
- **Input:** input của flow từ source đã lưu (tab có sẵn; trang toàn cục đọc `GET /flows/{id}`), qua `inputsFromSource`, `formFromInputs`, `validateForm` và `InputField` (`run/inputModel.ts:98, 207, 266`; `run/inputControls.tsx:33`), điền sẵn bằng giá trị `inputs:` thuần của schedule đè lên default của flow. Input `FILE` không được đưa ra; một cái bắt buộc làm submit bị vô hiệu hoá với "A FILE input cannot be backfilled".
- **Label:** `LabelsEditor`, các dòng key/value với Add và Remove; một key `system.` hoặc một dòng điền dở hiện lỗi của nó bên cạnh trường (FE-FORM-4); các dòng hoàn toàn trống bị bỏ (`cleanBackfill` của Kestra).
- **Submit** "Start backfill", bị vô hiệu hoá kèm lý do của nó cho đến khi: có start, `start < end` (hoặc không có end), không cái nào ở tương lai, input bắt buộc đã điền, không có label điền dở (`checkBackfill` của Kestra, cộng kiểm thứ tự). State form viết tay (FE-FORM-1).
- **Lỗi:** một 400 mà code gọi tên một trường thì hiện bên cạnh nó (`backfill_start_too_early` → Start; `backfill_end_in_future`, `backfill_window_invalid` → End; `backfill_inputs_invalid` → input được nêu tên qua `serverErrorInput`, `run/inputModel.ts:388`; `backfill_labels_invalid` → Labels); các lỗi khác hiện phía trên các nút kèm điều cần làm (FE-COPY-2). 409 `backfill_exists` refetch và đóng với câu của server. Không xoá gì người dùng đã gõ (FE-ACTION-3).

### 4.4 Execution

Các chip label của trang tổng quan và dấu "Backfill" (§3.3). Không có bộ lọc danh sách (§10).

## 5. Lỗi

| Status, code | Khi nào |
|---|---|
| 400 `invalid_body`, 413 `body_too_large` | Của W7e |
| 400 `source_does_not_parse` | Của W7e |
| 400 `backfill_window_invalid` | `start >= end` |
| 400 `backfill_end_in_future` | `end > now()` (thông báo cho biết giờ của server) |
| 400 `backfill_start_too_early` | `start` lùi quá mười năm |
| 400 `backfill_no_ticks` | cửa sổ không chứa tick nào của schedule này |
| 400 `backfill_too_many_ticks` | hơn 10.000 tick |
| 400 `backfill_inputs_invalid` | không phải object; một input `FILE`; cú pháp template trong input đã gộp; thiếu một input bắt buộc. Thông báo gọi tên input |
| 400 `backfill_labels_invalid` | một key `system.`, key hoặc value rỗng, cú pháp template, vượt giới hạn |
| 400 `too_many_triggers` | một danh sách hàng loạt hơn 100 |
| 404 `not_found` | flow, hoặc trigger id (câu của W7e) |
| 404 `no_backfill` | pause, resume hoặc delete khi không có |
| 409 `backfill_not_supported` | không phải `Schedule` |
| 409 `trigger_disabled` | create trên trigger bị tắt hiệu lực |
| 409 `trigger_off_queue` | create trên trigger không có ngày kế tiếp |
| 409 `backfill_exists` | create khi đã có một cái |

## 6. Test

**Rust, thuần** (`trigger/`): start bao gồm (một tick ở `start` được bắn); `first_tick_at_or_after`; `resume_after_backfill` cho `ALL`, `LAST`, `NONE`, không biết, `from > now`, không còn tick, và cả hai rule `from`; phép gộp label (thứ tự, cái cuối thắng, bỏ rỗng); phép phủ input với `refuse_templated` trên map đã gộp.

**Rust, database** (`tests/` của `egret-workflow`, mỗi test có database riêng):
- Create: mọi từ chối của §5 với code của nó; `next_evaluation_date` của dòng trigger = tick đầu tiên của cửa sổ và `previous_next_evaluation_date` = ngày cũ; flow của workspace khác 404; `workflow:read` 403.
- Drain: một cửa sổ `* * * * *` gồm 5 tick bắn 5 execution theo thứ tự qua 5 lượt, mỗi cái có `backfill_id`, input đã phủ và label; dòng sau đó bị xoá và ngày thường là vị trí tiếp tục.
- **Chạy lại (quyết định của người dùng):** một cửa sổ trên các tick đã có execution thường tạo execution thứ hai cho mỗi tick; backfill thứ hai trên cùng cửa sổ tạo cái thứ ba; một insert thứ hai bị ép cho một `(backfill_id, scheduled_date)` vi phạm `workflow_executions_one_per_backfill_tick` và bị tiêu thụ như `AlreadyFired`; index thường vẫn từ chối execution thường thứ hai (`the_unique_index_refuses_a_second_fire` không đổi).
- Test oracle `schedule_trigger_record_matches_the_captured_execution` không đổi, và `execution_trigger` của một execution backfill bằng của một lần chạy thật cho cùng tick.
- Tiếp tục: hoàn tất và delete cho `ALL` (các tick thường đang chờ chạy), `LAST`, `NONE`; một backlog thường cũ hơn `end` không chạy hai lần.
- Pause: một backfill tạm dừng không được claim và dòng của nó không bị ghi qua nhiều lượt (`updated_at` không đổi); resume bắn `current_tick` kế tiếp; pause sau snapshot của claim (lần đọc mới) không ghi gì, dàn dựng bằng một hook chỉ dành cho test (D22): khoảng đó nằm giữa `SELECT … FOR UPDATE OF t SKIP LOCKED` của claim và câu lệnh kế tiếp, nơi không có gì được ghi và claim không bao giờ chờ, nên không lock hay trigger bảng nào giữ được một lượt ở đó; `scheduler::fire` có thêm hook `#[cfg(any(test, feature = "test-util"))]` dừng lượt ngay sau claim (tiền lệ: các export test-util của `worker`, `src/worker/mod.rs:100-110`), không tốn gì ở bản release. Một test cũng xả cạn một cửa sổ và khẳng định `ticks_done == total_ticks` ngay trước khi dòng biến mất (D21); một test khác đổi cron giữa chừng sang cái vượt 10.000 tick và khẳng định backfill kết thúc với `last_error` (D19), và một test snap lại không có tick và giữ ngày đã lưu (D20).
- Bug delete của Kestra: pause, resume, delete: schedule thường tiếp tục từ vị trí đã lưu, không phải từ một ngày của cửa sổ.
- `sync`: đổi cron giữa chừng backfill suy lại mà không mất tick hiện tại và đếm lại; một source không parse được (ghi bằng SQL) giữ backfill và một lần save sửa lỗi tiếp tục nó; một id bị gỡ, đổi kind và xoá mềm xoá nó.
- Bật lại trong lúc backfill, mỗi lựa chọn: các ngày được giữ; skip chỉ chuyển vị trí tiếp tục.
- Label: render thấy chúng (`{{ labels.reason }}`), label backfill đè label flow cùng key, một `dependsOn` ở hạ nguồn với `labels:` khớp một execution backfill; replay, retry và vòng lặp iteration sao chép chúng; child subflow thì không.
- Đọc: `backfill`, `progress`, các số đếm, `can_backfill`/`backfill_refusal` theo từng nguyên nhân, `missed_ticks` từ vị trí tiếp tục, bộ lọc `backfill`.
- Đồng thời: một delete gặp lượt đang giữ dòng thì chờ, và không tick nào bắn sau đó; một save đổi cron và một pause trên một flow tuần tự hoá.
- Hàng loạt: 100 được chấp nhận, 101 bị từ chối, kết quả hỗn hợp với `no_backfill`.
- Mutation (mỗi cái phải giết một test): bỏ `AND backfill_id IS NULL` (test chạy lại chết); bỏ vế paused khỏi claim; ghi `previous_next_evaluation_date` khi pause; dùng strictly-after trong `attach_backfill`; bỏ phép gộp label trong `load_source`.

**Vitest:** nút và tag của cột, lý do vô hiệu hoá; chữ, phần trăm và nút của thanh tiến độ; confirm delete; lý do submit của dialog, body nó gửi (instant UTC, input, label), lỗi server ở mức trường; `LabelsEditor`; mục drawer; thanh hàng loạt toàn cục và kết quả; label và dấu của trang tổng quan.

**Playwright** (`frontend/tests/e2e/workflows-backfill.spec.ts`), trên stack riêng có scheduler và worker bật, không bao giờ trên stack dev dùng chung (harness của W7e §6):
1. Một flow có schedule `* * * * *`, label flow `team: data` và một input. Backfill năm phút gần nhất với một input và các label `team: ops`, `reason: test`: năm execution xuất hiện, thanh chạy đến 100 % rồi biến mất, trang tổng quan của mỗi execution hiện `team: ops` và "Backfill"; chế độ sáng và tối.
2. Chạy lại: backfill một cửa sổ mà schedule đã chạy: một execution thứ hai cho mỗi tick.
3. Một cron có giây (`withSeconds`) và cửa sổ một giờ: pause, chờ, không có execution mới; resume, chúng tiếp tục; delete kèm confirm: thanh biến mất, lần chạy kế tiếp là lần chạy thường.
4. Form: không chọn được ngày tương lai; Start sau End vô hiệu hoá submit kèm lý do; thiếu một input bắt buộc; create lần hai qua API trả 409.
5. Trang toàn cục: lọc `backfill=running`, pause hàng loạt hai cái, reload, cả hai đang tạm dừng; delete hàng loạt.
6. Một viewer không có `workflow:manage`: nút và các hành động của thanh tiến độ bị vô hiệu hoá kèm lý do.
7. Tắt trigger trong lúc backfill: "Frozen while the trigger is off"; bật lại: nó tiếp tục.

## 7. So sánh với Kestra

Task cuối của plan chạy trên image cục bộ `kestra/kestra` `28f59bf175e1` (`server local`, một container dùng một lần), với kịch bản 1-3 và 5 của §6 trên một flow `* * * * *`, và kiểm từng dòng **đọc từ mã nguồn** của §2: một cửa sổ bắt đầu đúng ở một tick (start loại trừ); một cửa sổ trên các ngày đã chạy (execution thứ hai); một label backfill trùng key với label flow (cái nào thắng); input ghi đè của schedule; pause, resume, delete (ngày nào được khôi phục, và các tick thường có phát lại không); hoàn tất tự nhiên với một tick thường nằm trong thời gian backfill (bị bỏ qua); API với `end` ở tương lai, `start > end`, create lần hai, pause khi không có backfill; hàng loạt theo trigger (202); công thức tiến độ trên màn hình. Mỗi dòng đã đo trở thành **quan sát**; khác biệt tìm thấy được thêm vào §2 (FE-REF-2).

## 8. Các rule gói này phải đáp ứng

- Bộ rule frontend, mọi rule.
- Không thêm dependency (control ngày là gốc).
- Chỉ sinh lại client bằng `npm run gen:api`.
- Văn xuôi migration là chỉ-thêm: 0156 và 0189 không bị sửa.

## 9. Những gì không xây

- **Create hàng loạt,** theo lựa chọn hoặc theo query (Kestra không có).
- **Pause/resume/delete theo query** (`by-query` của Kestra): chỉ theo lựa chọn, như W7e D10.
- **Nhiều tick mỗi lượt** (§3.6).

## 10. Việc tiếp theo

- Áp dụng `labels:` riêng của trigger và `system.from` cho execution của schedule, thứ egret parse rồi bỏ (`models/trigger.rs:51`); cần một render context cho biến của trigger.
- Lọc execution theo label và theo "backfill" trên danh sách execution.
- Phục hồi phần backlog thường nằm trước đầu cửa sổ khi schedule đã quá hạn lúc tạo (giới hạn đã biết ở §3.5, §3.6).
- Một backfill cho trigger đang bị tắt, nếu người dùng muốn xếp hàng một cái.
- Một log theo từng trigger (W7e §10) sẽ cho drawer hiện tick backfill nào bị bỏ qua và vì sao; hiện chỉ `last_error` giữ cái mới nhất.
