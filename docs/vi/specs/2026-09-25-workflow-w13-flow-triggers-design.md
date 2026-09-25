# W13 — Flow trigger: một flow chạy vì một flow khác vừa kết thúc

**Trạng thái:** thiết kế, chờ review
**Phụ thuộc:** W6 (`trigger::flow::evaluate`, hàm thuần), W10 (runtime), W12 (`scheduler::sync`, `reconcile`, ticker của scheduler, `start_execution_in`). Cả ba đã merge.
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (dòng W13)
**Đã chốt với người dùng, 2026-09-25:**
- **Làm cả hai nhánh:** nhánh không cửa sổ (không có `dependsOn`), và nhánh `dependsOn` với cửa sổ bền.
- **Flow nghe đang ở bản nháp vẫn bắn**, giống cron, webhook và API start vốn đã chạy bản draft.
- **Xử lý bất đồng bộ qua outbox**, không lồng trong `settle`. §3 giải thích vì sao thiết kế đầu bị rút.

**Cách giao:** một PR docs, sau đó **W13a** (hạ tầng + nhánh không cửa sổ) và **W13b** (`dependsOn` + cửa sổ), nối tiếp nhau.

## 1. Cái gì đã có, cái gì chưa

| thành phần | trạng thái trên `origin/main` (`006563ee`) |
|---|---|
| `trigger::flow::evaluate(listener, flow, source, when, max_depth)` | **đã có, thuần** (W6). Lọc theo đúng thứ tự của artifact: workspace, trạng thái vào, cùng flow, kind, flow disabled, draft, độ sâu, trigger disabled, `states`, `when` |
| `WindowFreeFlowTrigger::resolve` | **đã có**. Từ chối `dependsOn` bằng `UnsupportedDependsOn` |
| `models::FlowTrigger` | Có `inputs`, `states` (mặc định: các trạng thái kết thúc + `PAUSED`), `dependsOn`, `mode`, `minSatisfied`. **`window` bị bỏ**: khai báo `window:` vẫn parse được nhưng bị mất lặng lẽ |
| `executions.trigger_execution_id` | **đã có** (migration 0138). `workflow_executions_one_link_kind` chỉ cho mỗi hàng một loại liên kết |
| độ sâu execution được lưu lại | **chưa có**. `ExecutionCtx::depth` đếm *số `settle` lồng nhau trên stack*, không phải `executionDepth` của Kestra |
| nơi gọi `evaluate` | **chưa có** |

## 2. Cái gì chấm W13

**Không phải ratchet: dự kiến 109 trước và 109 sau, cho cả hai PR.** W6 đã đo lý do:
- bước settle của `harvest.py` lần theo `parent_id`;
- execution do flow trigger bắn có `parentId = null` ở 210 trên 210 lần đo, và chỉ mang liên kết trigger.

Corpus có 17 flow nghe (21 khai báo, 20 cái dùng `dependsOn`) nhưng **không** entry baseline nào với tới được. Muốn oracle đi theo `trigger_execution_id` thì phải harvest lại. Việc đó nằm ngoài phạm vi, ghi ở §9.

W13 được chấm bằng test của chính nó (§7). Mỗi test phải được thấy đỏ dưới một mutation có tên, như W12.

## 3. Vì sao đánh giá bất đồng bộ

Thiết kế đầu đánh giá listener ngay trong `settle` của execution nguồn và bắn lồng tại chỗ. Code đã bác thiết kế này:

- `settle` vốn đã lồng execution con cho `Subflow`, và giới hạn ở **`subflow::MAX_SPAWN_CHAIN = 8`** vì mỗi tầng tốn stack của worker (tokio worker chỉ có 2 MiB). Một chuỗi flow trigger lồng A→B→C… sẽ bị cắt ở khoảng 8 tầng, trong khi Kestra cho **100** (`DEFAULT_MAX_EXECUTION_DEPTH`).
- Một nguồn có N listener sẽ kéo N execution, cộng mọi thứ mà `settle` đầu tiên của *chúng* bắn tiếp, vào chung transaction của worker.
- Chính Kestra cũng bất đồng bộ ở đây: `DefaultExecutor.processFlowTriggers` lấy execution từ một hàng đợi.

Vì vậy `settle` chỉ **ghi lại** lần đổi trạng thái, còn việc đánh giá do một vòng riêng làm.

## 4. W13a: hạ tầng và nhánh không cửa sổ

### 4.1 Schema (migration `0157`; SP4a đang giữ 0154/0155)

```sql
-- Mỗi Flow trigger mà một flow khai báo có một hàng, do scheduler::sync giữ.
CREATE TABLE workflow.flow_listeners (
  flow_id      uuid    NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  trigger_id   text    NOT NULL,
  workspace_id uuid    NOT NULL,
  states       text[]  NOT NULL,  -- Flow.states sau khi áp mặc định
  depends_on   boolean NOT NULL,  -- thuộc nhánh nào
  disabled     boolean NOT NULL,  -- disabled của flow YAML HOẶC của trigger
  PRIMARY KEY (flow_id, trigger_id)
);
CREATE INDEX workflow_flow_listeners_ws ON workflow.flow_listeners (workspace_id) WHERE NOT disabled;

-- Outbox: mỗi lần nguồn đổi trạng thái mà có listener nào đó nghe được thì có một hàng.
CREATE TABLE workflow.flow_trigger_events (
  id                  bigserial   PRIMARY KEY,
  source_execution_id uuid        NOT NULL REFERENCES workflow.executions (id) ON DELETE CASCADE,
  workspace_id        uuid        NOT NULL,
  state               text        NOT NULL,
  done                text[]      NOT NULL DEFAULT '{}', -- các listener đã xử lý xong
  attempts            integer     NOT NULL DEFAULT 0,
  created_at          timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE workflow.executions ADD COLUMN execution_depth integer NOT NULL DEFAULT 0;
-- Sự kiện outbox mà một lần bắn đã dùng; có giá trị khi và chỉ khi trigger là Flow.
ALTER TABLE workflow.executions ADD COLUMN flow_trigger_event_id bigint;
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_flow_trigger_event_iff_flow
  CHECK ((flow_trigger_event_id IS NOT NULL)
         = (execution_trigger IS NOT NULL
            AND execution_trigger->>'type' = 'io.kestra.plugin.core.trigger.Flow'));
-- Chốt chặn, như index một-tick-một-lần của W12: mỗi listener bắn nhiều nhất
-- một lần cho mỗi sự kiện outbox.
CREATE UNIQUE INDEX workflow_executions_one_per_flow_trigger_event
  ON workflow.executions (flow_trigger_event_id, flow_id, (execution_trigger->>'id'))
  WHERE flow_trigger_event_id IS NOT NULL;
```

Unique index lấy khoá theo sự kiện outbox, không theo trạng thái của nguồn, vì một nguồn quay lại một trạng thái (`PAUSED`, `RUNNING`, `PAUSED`) là một lần chuyển mới, tức một sự kiện mới, và Kestra bắn lại cho nó. Khoá theo `(nguồn, trạng thái)` sẽ chỉ cho listener nghe được lần đầu. Id sự kiện là `bigserial` và không bao giờ bị dùng lại. `flow_trigger_event_id` không phải khoá ngoại, vì processor xoá sự kiện ngay trong transaction bắn (§4.4).

### 4.2 Giữ `flow_listeners` đồng bộ

- **`scheduler::sync` ghi các hàng này** trong cùng lượt với hàng `Schedule`. Nhờ vậy mọi hàm ghi của `FlowRepo`, `reconcile` lúc khởi động và bộ quét chỗ ghi đều phủ luôn, không cần cơ chế mới.
- Khai báo không resolve được thì không có hàng, và lý do được ghi log.
- Khai báo `dependsOn` có hàng với `depends_on = true` ngay từ W13a, nhưng processor của W13a bỏ qua nó. Thông báo từ chối nói rõ W13b là chủ. W13b sẽ bỏ lệnh bỏ qua đó.

### 4.3 Điểm nối trong `settle`

Ở cuối `settle`, một câu lệnh ghi một sự kiện cho mỗi trạng thái mà settle đã **nối thêm** vào history của execution (`state.history[before.history.len()..]`), trừ `CREATED`, theo đúng thứ tự:

```sql
INSERT INTO workflow.flow_trigger_events (source_execution_id, workspace_id, state)
SELECT $1, $2, t.s
  FROM unnest($3::text[]) WITH ORDINALITY AS t(s, n)   -- các trạng thái được nối thêm
 WHERE EXISTS (SELECT 1 FROM workflow.flow_listeners l
                WHERE l.workspace_id = $2 AND NOT l.disabled AND t.s = ANY (l.states))
 ORDER BY t.n
```

- **Mọi lần đổi trạng thái, không chỉ lúc kết thúc.** `PAUSED` nằm trong `states` mặc định mà không phải trạng thái kết thúc. Hook báo cho execution cha hiện chỉ chạy ở trạng thái kết thúc, nên không dùng lại được.
- **Mọi trạng thái được nối thêm, không chỉ trạng thái cuối.** Một settle có thể nối nhiều trạng thái: một lần start có `Pause` giới hạn đi `CREATED → RUNNING → PAUSED`, còn settle kết thúc một pause và vào pause kế tiếp đi `PAUSED → RESTARTED → RUNNING → PAUSED`, trạng thái đầu và cuối trùng nhau. Kestra chạy `processFlowTriggers` một lần cho mỗi lần chuyển, nên mỗi trạng thái nối thêm là một sự kiện. Lát cắt này đúng vì history của execution chỉ được nối thêm trong một settle.
- **`EXISTS` giữ outbox rỗng** với workspace không có listener. Chi phí trên đường nóng chỉ là một lần dò index.
- **Nguyên tử với việc đổi trạng thái.** Lần đổi trạng thái nào commit thì có sự kiện, lần nào rollback thì không.

### 4.4 Processor

`flow_trigger::process_events(&PgPool) -> Result<EventReport>` chạy từ **ticker scheduler của W12**, ngay sau `fire_due`, dưới cùng công tắc `scheduler_enabled`. Mỗi pass rút cạn outbox, **mỗi sự kiện một transaction**:

```text
BEGIN
  DELETE FROM workflow.flow_trigger_events
   WHERE id = (SELECT id FROM workflow.flow_trigger_events ORDER BY id
               LIMIT 1 FOR UPDATE SKIP LOCKED)
  RETURNING *                                    -- không có -> hết pass
  nạp execution nguồn (namespace, flow key, kind, execution_depth, render surface)
  SELECT listener: cùng workspace, state thuộc states, khác flow nguồn, không disabled,
         flow nghe chưa xoá, state <> 'disabled', feature workflows bật,
         workspace active                        -- sáu công tắc tắt của W12
  với mỗi listener, trong một SAVEPOINT:
    parse source của listener; WindowFreeFlowTrigger::resolve
    render `when` và `inputs` trên context của NGUỒN
    trigger::flow::evaluate(.., max_depth = 100)
    Fired -> start_execution_in(tx, NewExecution { trigger_execution_id: nguồn,
             execution_depth: nguồn + 1, flow_trigger_event_id: sự_kiện.id,
             trigger: {id, type: Flow, variables}, .. })
    lỗi -> phân loại như W12; lỗi của listener này không bao giờ làm hỏng lần bắn của listener khác
COMMIT
```

**Vì sao từng dòng như vậy:**

1. **Sự kiện bị xoá trong chính transaction bắn**, nên mỗi sự kiện được xử lý đúng một lần hoặc không lần nào. Nếu process chết, cả lần bắn lẫn lệnh xoá cùng rollback và pass sau thử lại. Unique index là chốt chặn.
   - **Thử lại theo từng listener, không theo cả sự kiện.** Listener gặp lỗi tạm thời chỉ rollback savepoint của nó, còn lần bắn của các listener khác vẫn commit.
   - Sự kiện được giữ lại, ghi các listener đã xong vào `done` và tăng `attempts`. Pass sau chỉ đánh giá những listener chưa có trong `done`.
   - Sau 5 lần thử, sự kiện bị bỏ và các listener vẫn lỗi được log ở mức `warn`. Nhờ vậy một listener lỗi không thể làm hỏng cũng không thể chặn các listener khác.
2. **`SKIP LOCKED` chia sự kiện giữa các replica**, như câu claim của W12. Hai sự kiện cho cùng một listener có thể được xử lý song song. Chỉ nhánh cửa sổ cần chúng xếp hàng, và W13b làm việc đó bằng row lock của cửa sổ.
3. **Context của nguồn được đọc lúc xử lý, không phải lúc đổi trạng thái.** `endDate` và `lastTaskId` của bản ghi trigger cũng vậy.
   - Với trạng thái kết thúc, hai thời điểm này thường như nhau. Chúng khác nhau khi nguồn đã đi tiếp trước khi sự kiện được xử lý: một nguồn đã kết thúc nhưng bị replay hoặc restart trong lúc đó sẽ render context muộn hơn, với `endDate`/`lastTaskId` muộn hơn.
   - Với `PAUSED`, nếu nguồn đã resume trước khi sự kiện được xử lý thì `when` sẽ render trên context muộn hơn. Đây là độ lệch có ghi rõ so với Kestra, vốn render trên execution nó được giao.
   - Cách còn lại là chụp nguyên render context vào mỗi hàng sự kiện. Làm vậy không đáng về dung lượng cho một độ lệch chỉ xảy ra khi nguồn đi tiếp trước khi sự kiện được xử lý.
4. **Độ sâu là dữ liệu, không phải stack.** `evaluate` đọc `execution_depth` đã lưu của nguồn, và execution được bắn ghi depth + 1. Mỗi lần bắn là một lời gọi `start_execution_in` riêng trong vòng lặp của processor, không lồng nhau, nên không stack nào phình ra, và trần là 100 như Kestra. Một chuỗi tiến **ít nhất** một bước mỗi pass: sự kiện do settle ĐẦU TIÊN của execution vừa bắn ghi ra (listener nghe `RUNNING`, execution sinh ra đã FAILED, flow kết thúc ngay trong settle đầu) được commit cùng lần bắn và được claim ngay trong CÙNG pass, nên các bước như vậy nối nhau trong một pass. Độ sâu vẫn chặn nó.
5. **Độ trễ:** một bước mà listener có task phải chờ worker rồi chờ tick kế của scheduler, mặc định 1 giây, nên chuỗi 10 bước như vậy mất khoảng 10 giây trở lên. Các bước kết thúc hoặc phát sự kiện ngay trong settle đầu có thể nhanh hơn, như điểm 4. Chấp nhận và nói rõ.
6. **Scheduler tắt thì outbox phình.**
   - `settle` ghi sự kiện hễ có listener, bất kể `scheduler_enabled`, vì `settle` không đọc config.
   - Khi bật scheduler lên, phần tồn đọng sẽ được xử lý. Đây là bản tương ứng của việc cron bù lịch theo `ALL`.
   - Operator nào để scheduler tắt trong khi có flow khai báo flow trigger sẽ tích luỹ mỗi lần đổi trạng thái được nghe một hàng. Điều này được ghi trong comment của chart, cạnh `schedulerEnabled`.

`NewExecution` (W12 Task 3) thêm bốn trường: `trigger_execution_id: Option<Uuid>`, `execution_depth: i32`, `flow_trigger_event_id: Option<i64>` và `born_failed: bool` (§4.5).

### 4.5 Execution được bắn

- `execution_trigger = { id, type: "io.kestra.plugin.core.trigger.Flow", variables }`. W6 mới đo được một phần `variables` (`executionId`, `namespace`, `flowId`, `state`, `flowRevision`, …). **Task 1 của W13a đo toàn bộ object** trên artifact đã ghim, như Task 1 của W12, và code tái tạo đúng theo file đo đó.
- `inputs` được render trên context của nguồn. **Render lỗi KHÔNG làm lần bắn bị từ chối.** Kestra, đã đo (W13a Task 1, `listener_run1_FAILED_bad_inputs`), vẫn tạo execution của listener, `CREATED → FAILED`, `taskRunList: []`, không có `inputs`, đủ bản ghi trigger và depth + 1. egret làm y như vậy: execution được khởi tạo với `born_failed`, áp `SetExecutionState { FAILED }` trước settle đầu tiên, nên không run nào được tạo và hook outbox phát sự kiện `FAILED` như mọi trường hợp khác. `inputs` lưu là `{}` (cột `NOT NULL`). `workflow.executions` không có cột message cấp execution, nên lỗi render được log chứ không lưu. Chỉ `when` không render được mới là từ chối thuần, không tạo execution (W6 đã đo).
- **Chưa làm, và nói rõ:** nhãn `system.correlationId` (egret chưa có cột labels), và việc đưa `trigger.*` vào render context. Cron cũng ở tình trạng tương tự.

## 5. W13b: `dependsOn` và cửa sổ

Phần này bám theo `FlowTriggerService.processMultipleConditionWindow` và `AbstractJdbcMultipleConditionStateStore.process` (checkout Kestra `96893b589`).

### 5.1 Model

- `FlowTrigger` thêm trường `window`. Kestra 2.x khai báo nó là `Window { deadline | from+to | every+offset | lookback }` rồi quy về một `TimeWindow`:
  - có `deadline` → `DAILY_TIME_DEADLINE`;
  - có `from` + `to` → `DAILY_TIME_WINDOW`;
  - có `lookback` → `SLIDING_WINDOW`;
  - còn lại → `DURATION_WINDOW`, lấy `every` (mặc định 1 ngày) làm độ dài cửa sổ và `offset` làm độ lệch.
- Khai báo trộn các nhóm loại trừ nhau bị từ chối khi lưu.
- Giá trị `Duration` được parse theo tập con ISO-8601 mà corpus dùng. Ngoài tập con đó thì từ chối, không đoán.

### 5.2 Schema (migration `0158`)

```sql
CREATE TABLE workflow.flow_trigger_windows (
  flow_id      uuid        NOT NULL,
  trigger_id   text        NOT NULL,
  window_start timestamptz NOT NULL,
  window_end   timestamptz NOT NULL,
  results      jsonb       NOT NULL,   -- {điều kiện -> true}; chỉ lưu true
  outputs      jsonb,
  PRIMARY KEY (flow_id, trigger_id),
  FOREIGN KEY (flow_id, trigger_id) REFERENCES workflow.flow_listeners ON DELETE CASCADE
);
```

### 5.3 Xử lý một listener `dependsOn`

Chạy bên trong savepoint của từng listener ở §4.4:

1. `INSERT … ON CONFLICT DO NOTHING` một cửa sổ mới, tính tại `now()` của Postgres, rồi `SELECT … FOR UPDATE`. Cửa sổ đã hết hạn thì bị thay bằng cửa sổ mới.
2. Nếu `now` nằm ngoài cửa sổ (ví dụ `DAILY_TIME_WINDOW` nhận sự kiện ngoài khung giờ), sự kiện bị bỏ qua đối với listener này.
3. Đánh giá từng `Dependency`: namespace, flowId, `states`, `labels`, và `when` render trên nguồn. Các kết quả true được cộng dồn; điều kiện đã đạt thì giữ nguyên là đạt.
4. Nếu `when` của trigger đạt và `mode` thoả (`ALL`, `ANY`, hoặc `AT_LEAST` ≥ `minSatisfied`), trigger bắn qua đúng đường của §4.4. Sau đó cửa sổ bị xoá nếu đã thoả (`resetOnSuccess`, mặc định bật).
5. Ticker của scheduler cũng xoá các cửa sổ hết hạn, mỗi pass một lần.

**Đồng thời:**
- Hai nguồn thoả hai điều kiện của cùng một listener, được xử lý trên hai replica, sẽ xếp hàng trên row của cửa sổ. Cái sau đọc thấy kết quả của cái trước và bắn đúng một lần.
- Cặp `ON CONFLICT DO NOTHING` + `FOR UPDATE` bảo đảm một hàng chưa tồn tại không thể bị tạo hai lần.

### 5.4 Đo trước khi viết code

Task 1 của W13b bắn flow thật trên artifact và lưu kết quả nguyên văn. Mã Java ở trên chưa trả lời được các câu sau:
- biên của `DURATION_WINDOW`: tính nửa đêm theo timezone nào, và `offset` ảnh hưởng ra sao;
- mốc bắt đầu của `SLIDING_WINDOW` được đặt thế nào;
- một điều kiện đã đạt mà trigger chưa bắn thì có được giữ sang cửa sổ sau không;
- `resetOnSuccess` hoạt động thế nào với `ANY` và `AT_LEAST`;
- sự kiện rơi đúng vào biên `end` thì được xử lý ra sao.

## 6. Các công tắc tắt

Một listener không nghe (nhánh không cửa sổ) hoặc không cộng dồn (nhánh cửa sổ) khi rơi vào một trong các trường hợp sau:
- flow của nó đã bị xoá;
- `state` của flow là `'disabled'`;
- YAML của flow có `disabled`;
- trigger có `disabled`;
- feature `workflows` của workspace đang tắt;
- workspace không `active`.

Đây đúng là sáu công tắc của W12. Sự kiện đến trong lúc listener đang tắt thì **bị bỏ đối với listener đó, không xếp hàng lại**. Điều này khớp với Kestra, vì ở đó flow bị tắt không bao giờ được xét làm ứng viên. Đây là chỗ duy nhất flow trigger cố ý khác cách cron đóng băng rồi bù lịch: một sự kiện không phát lại được như một chiếc đồng hồ.

## 7. Test

Mỗi dòng nêu mutation phải làm nó đỏ.

### 7.1 W13a

| tính chất | mutation phải giết |
|---|---|
| `sync` ghi, cập nhật và xoá hàng listener; `reconcile` điền hàng cho flow đã lưu từ trước | bỏ nhánh flow trigger trong `sync` |
| nguồn đạt SUCCESS sinh đúng một sự kiện, và một pass bắn đúng một execution với liên kết, depth, bản ghi trigger và inputs đúng | bỏ hook |
| PAUSED làm bắn listener dùng `states` mặc định | chỉ phát sự kiện ở trạng thái kết thúc |
| workspace không có listener thì không ghi sự kiện nào | bỏ `EXISTS` |
| bị từ chối: cùng flow, khác workspace, kind không phải normal, `when` sai, `when` không render được | bỏ lần lượt từng lọc |
| sáu công tắc tắt | bỏ lần lượt từng vế |
| chuỗi A→B→C bắn qua ba pass; vòng A→B→A dừng ở depth 100 | bỏ kiểm độ sâu; ghi depth 0 |
| listener hỏng không chặn các listener khác của cùng sự kiện, và không làm mất sự kiện đối với chúng | bỏ savepoint |
| một sự kiện được xử lý đúng một lần dù có hai processor (đếm số lần insert như W12) | bỏ `SKIP LOCKED` và index |
| lỗi tiêm vào sau khi bắn thì lần bắn bị rollback và sự kiện vẫn còn | xoá sự kiện trong transaction riêng |
| flow nghe là bản nháp vẫn bắn | ánh xạ draft sang `isDraft` của Kestra |

### 7.2 W13b

| tính chất | mutation phải giết |
|---|---|
| `ALL` với hai dependency: chỉ A thì không bắn; A rồi B thì bắn một lần và reset cửa sổ | không cộng dồn kết quả |
| `ANY`; `AT_LEAST` với `minSatisfied` | đảo phép so sánh mode |
| hai nguồn trên hai processor cùng thoả `ALL` thì bắn đúng một lần | bỏ `FOR UPDATE` của cửa sổ |
| kết quả của cửa sổ đã hết hạn không được tính | bỏ kiểm `end` |
| sự kiện ngoài khung giờ bị bỏ qua với `DAILY_TIME_WINDOW` | bỏ kiểm còn trong cửa sổ |
| biên của bốn kiểu `TimeWindow` khớp số đo của Task 1 (test thuần trên `(window, now)`, dữ liệu lấy từ file đo) | — |
| `window:` khai báo mâu thuẫn bị từ chối khi lưu | bỏ validate |

## 8. Cố ý không nằm trong W13

- Cho oracle đi theo `trigger_execution_id` (cần harvest lại).
- Nhãn `system.correlationId`.
- Đưa `trigger.*` vào render context.
- Backfill.
- `stopAfter` (W8 phát hiện trường này được parse nhưng không ai đọc).
- UI.

## 9. Cách giao

- **PR docs:** spec này (EN + VI), plan, và các dòng W13a/W13b trong §9 của spec chương trình.
- **W13a:**
  1. Đo `execution_trigger` của Flow trigger trên artifact.
  2. Migration 0157, và `sync` cho listener.
  3. Liên kết + depth trong `NewExecution`, và hook trong `settle`.
  4. Processor và các test của nó.
  5. Tuyên bố độ phủ, rồi full gate.
- **W13b:**
  1. Đo biên cửa sổ trên artifact.
  2. Model `window`, `Duration`, validate.
  3. Port thuần `TimeWindow`.
  4. Migration 0158 và luồng xử lý `dependsOn`.
  5. Test đồng thời, tuyên bố độ phủ, full gate.

Mỗi task đều qua review. Cuối mỗi PR có một review toàn nhánh. Test chạy trên Postgres riêng như W12.
