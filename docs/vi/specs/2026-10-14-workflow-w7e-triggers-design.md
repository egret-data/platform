# W7e — UI Workflows, phần 5: trigger (tab Triggers của flow, trang Triggers toàn cục, công tắc runtime)

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-14-workflow-w7e-triggers-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** đã hiện thực trên `feat/workflow-w7e-triggers`, 2026-10-15. Các kịch bản Playwright 1-7 của §6 đã chạy trên một stack riêng (`frontend/tests/e2e/workflows-triggers.spec.ts`), và phép so sánh của §7 đã chạy với image Kestra cục bộ; §2 nói dòng nào được quan sát. Chỗ nào văn bản này và code khác nhau thì code đã được đo và văn bản được sửa (2026-10-15).

**Phụ thuộc:**
- W7a (#659): khu vực Workflows, trang chi tiết flow và danh sách tab của nó (`FLOW_TABS`, `frontend/src/routes/workflows/FlowDetail.tsx:52-53`, có comment đã dành sẵn một chỗ cho W7e).
- W7b (#662): danh sách execution và các chip trạng thái của nó.
- W7d (#669): việc parse ngoài runtime với một flow đã gắn (`namespaces/dependencies.rs:204-221`, `routes.rs:3210-3233`), được dùng lại ở đây.
- W12 (`workflow.triggers`, `scheduler::sync`, `fire_due`), W13a/W13b (`workflow.flow_listeners`, `flow_trigger_windows`), W6 Task 7 (route webhook).

Bộ rule frontend `docs/en/frontend-rules.md` ràng buộc mọi dòng code frontend.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §8 (UI của Kestra làm tham chiếu) và §9 dòng W7.

**Đã chốt với người dùng, 2026-10-14:**
- **Backfill là một gói riêng, W7f** (migration, route, nối dây `fire_one`, form và banner tiến độ của nó). W7e không xây phần nào trong đó và không được làm nó bất khả thi (§9).
- **Một công tắc bật/tắt runtime, lưu trong database như Kestra lưu,** tách khỏi `disabled:` trong YAML. Bật lại một `Schedule` thì hỏi người dùng có phục hồi các tick đã lỡ hay không (`recoverMissedSchedules` của Kestra).

**Quyết định mặc định (2026-10-14),** theo rule của brief "theo Kestra trừ khi egret có lý do; giữ an toàn chặt". Mỗi quyết định được giải thích ở chỗ nó được dùng:
- D1. Công tắc là một bảng các trigger **bị tắt**, `workflow.disabled_triggers`: có dòng nghĩa là "tắt ở runtime", không có dòng nghĩa là bật. Không có dòng "enabled" được lưu (§3.1).
- D2. Công tắc là nguồn quyền lực; `workflow.triggers.disabled` và `workflow.flow_listeners.disabled` trở thành **bản sao hiệu lực** của nó (flow YAML HOẶC trigger YAML HOẶC runtime), do `sync` và route công tắc ghi. Nhờ đó mọi vị từ `NOT disabled` hiện có đều tuân theo nó mà không phải sửa (§3.2).
- D3. Công tắc phủ các trigger `Schedule`, `Webhook` và `Flow`. Của Kestra chỉ phủ các trigger do scheduler chạy (§2). Một trigger `Leaf` (mọi kiểu khác) không bao giờ được egret bắn và không có công tắc.
- D4. Một trigger bị tắt trong YAML không thể bật từ UI hay API: 409 `trigger_disabled_in_source` (server của Kestra chấp nhận và UI của nó ẩn công tắc).
- D5. Bật lại một `Schedule` với "skip missed" chuyển `next_evaluation_date` sang tick đầu tiên sau `now()` trong transaction của công tắc. "Recover missed" theo `recoverMissedSchedules` của chính trigger (vắng = `ALL`), như Kestra; vì vậy W7e bắt đầu đọc thuộc tính đó, thứ mà egret hiện đang bỏ (§3.3).
- D6. Một lần lưu hợp lệ không còn khai báo một trigger id thì xoá dòng công tắc của nó; một source không parse được thì không bao giờ xoá dòng nào; xoá mềm xoá hết (§3.4).
- D7. Window đang mở của một listener `dependsOn` bị tắt được để tự hết hạn, như với listener bị tắt trong YAML hiện nay (§3.2).
- D8. Một index mới phục vụ "execution gần nhất theo từng trigger" (§3.6), chỉ giữ nếu `EXPLAIN` cho thấy truy vấn dùng nó (rule của 0183).
- D9. Danh sách toàn cục parse draft ngoài các worker async với giới hạn 2.000 flow **sau** khi lọc namespace và flow, 422 `triggers_too_large` nếu vượt (§3.5). Trang chỉ chuyển focus vào bộ lọc namespace khi gặp 422 đó sau một thay đổi do người dùng làm bằng một picker, bộ phân trang hoặc Clear filters: không bao giờ ở lần tải đầu của trang, và không bao giờ khi đang gõ trong ô tìm kiếm (quyết định mặc định, 2026-10-15; FE-A11Y-6; §4.4).
- D10. Bật/tắt hàng loạt chỉ theo lựa chọn, tối đa 100 trigger, một transaction cho mỗi trigger, đồng bộ, có kết quả cho từng trigger. Không có "chọn tất cả kết quả khớp" (§3.7).
- D11. Trùng trigger id trở thành lỗi validation, `duplicate_trigger_id`, như Kestra, nhưng là finding **chỉ-khi-lưu**: nó hiện trên draft và chặn publish, và không nằm trong `validate_for_run`, nên các flow đã lưu mà đang khai báo trùng vẫn tiếp tục chạy (§3.8).
- D12. Không có hành động unlock, restart hay delete-state (egret không có lock được lưu bền; §3.9). Không cảnh báo rằng một draft không bắn: draft có bắn (§4.4).
- D13. Migration được đánh số **0189**: `0186`-`0188` đã bị nhánh chưa merge `feat/view-governance` giữ (§3.1).
- D14. Phục hồi `LAST` dùng `ScheduleTrigger::last_tick_at_or_before` mới; `cron.last_before` hiện có là strictly-before và private (§3.3).
- D15. `off_runtime` của W7d (`dependencies.rs:213`) trở thành `pub(crate)` với thông báo lỗi trung tính (§3.5).
- D16. Phản hồi mang `disabled_by_name` bên cạnh `disabled_by`, vì trình duyệt không phân giải được user id (danh sách user chỉ dành cho admin) (§3.5, §4.1).
- D17. W7e **không** thêm routing ingress, nginx hay Vite cho webhook và không dựng URL webhook từ `window.location.origin`: UI hiện đường dẫn webhook kèm nút copy và một ghi chú (§4.2).
- D18 (quyết định mặc định, 2026-10-15). Trong các route đọc, một flow có cột `state` là `disabled` làm mọi trigger của nó `effective_disabled`, bên cạnh YAML, công tắc runtime và thùng rác: flow như vậy không kích hoạt gì (claim của schedule và câu select listener đọc `state`, `scheduler/start.rs`, `flow_trigger/mod.rs`; webhook trả 409). `yaml_disabled` và `runtime_disabled` vẫn là false và công tắc vẫn dùng được. Bản sao `disabled` lưu trong các dòng trigger (§3.2) không đổi (§3.5).

## 1. Vì sao

Mọi trigger một flow khai báo hiện chỉ nhìn thấy được dưới dạng YAML. Không route nào liệt kê trigger hay trạng thái của chúng (`management_router`, `crates/egret-workflow/src/routes.rs:3270-3430`, có flow, version, execution, namespace, file, KV, resume và replay). Người dùng không thể thấy một schedule bắn lần tới lúc nào, vì sao nó rời hàng đợi (`workflow.triggers.last_error`), một window `dependsOn` đã đi đến đâu, hay URL của một webhook. Cách duy nhất để dừng một trigger là sửa YAML của flow, việc này còn tạo thêm một revision mới. Người dùng Kestra mong đợi một tab Triggers trên flow, một trang Triggers toàn cục và một công tắc.

## 2. Khác biệt so với Kestra

**Đo trên một Kestra đang chạy, 2026-10-14/15 (Task 8, §7).** Image cục bộ `kestra/kestra` `28f59bf175e1` (Kestra 2.0 nightly, `server local`, trong một container dùng một lần) chạy các kịch bản 1-3 và 5 của §6 với các flow của end to end: `kcmp.team.up1`, `kcmp.team.up2`, và `kcmp.team.main` với một `Schedule` `* * * * *`, một `Webhook`, một trigger `Flow` có `dependsOn` trên cả hai upstream (nightly này chấp nhận `dependsOn`) và một `Schedule` có `disabled: true`. Cùng các kịch bản đó chạy trên egret (`frontend/tests/e2e/workflows-triggers.spec.ts`). Mỗi ô Kestra nói nó được **quan sát** trên image đang chạy (API của nó ở `/api/v1/main/...` hoặc UI của nó, đọc trong DOM) hay **đọc trong mã nguồn** (checkout ở `data-portal/kestra`, `96893b589`; path tính từ đó).

| Kestra | egret | Vì sao |
|---|---|---|
| **Quan sát.** Chỉ các trigger do scheduler chạy mới có trigger state, và `disabled` ở runtime nằm ở đó. `PUT /api/v1/main/triggers/set-disabled` trên `Webhook` hoặc trigger `Flow` trả 404 "Trigger disappeared after set-disabled" (scheduler ghi log "Trigger state not found"): không cái nào tắt được ở runtime. Sau đó POST tới webhook vẫn trả 200 và khởi động một execution, và chạy cả hai upstream vẫn khởi động downstream. (Trong mã nguồn: đường webhook chỉ kiểm tra `disabled` của flow, `webserver/.../ExecutionController.java:844-858`, và trigger Flow chỉ kiểm tra YAML, `executor/.../FlowTriggerService.java:50, 61`.) | Công tắc dừng cả ba loại: việc claim schedule, việc chọn listener Flow-trigger cùng insert outbox, và route webhook. **Quan sát:** một webhook đã tắt trả 409 "Cannot execute webhook: the trigger 'hook' is disabled." và không khởi động gì; bật lại, 200 và một execution | Một công tắc để webhook vẫn bắn là một điều khiển giả |
| **Quan sát.** Công tắc ở tab flow vắng mặt khi trigger không có dòng state: các dòng `Webhook` và `Flow` không có (dòng webhook có "Send test event") | Mọi trigger `Schedule`, `Webhook` và `Flow` đều có công tắc; một `Leaf` hiện "Not run by egret" và không có công tắc | egret không có dòng state cho webhook, mà công tắc phủ chúng (D3) |
| **Quan sát.** Bật qua API một trigger có YAML ghi `disabled: true` trả 200 và đặt `disabled` của state thành false; công tắc của UI vẫn tắt và bị vô hiệu hoá (nó hiện `!(disabled \|\| sourceDisabled)`). Trong mã nguồn, trigger vẫn không bao giờ bắn: fetcher bỏ qua trigger bị tắt trong YAML (`DefaultSchedulableTriggerFetcher.java:104`) | 409 `trigger_disabled_in_source`, "This trigger is disabled in the flow's YAML; remove `disabled: true` there" (cũng quan sát trên egret); công tắc bị vô hiệu hoá kèm "Disabled in the flow's YAML" | Người gọi API không nên nhận 200 cho một thay đổi không có tác dụng |
| **Quan sát** cho skip và `ALL`. Tắt qua hai tick: không gì chạy. Bật với lựa chọn mặc định (body không mang `recoverMissedSchedules`): không lần chạy lỡ nào, lần chạy kế là tick sau đó, và `evaluatedAt` thành thời điểm bật. Bật với "Follow…" khi thuộc tính vắng (`ALL`): cả hai tick lỡ đều chạy, mỗi lượt scheduler một tick, cách nhau khoảng 1,6 s. **Đọc trong mã nguồn** cho `LAST` và `NONE` (`core/.../Schedulable.java:47-51`, `TriggerEventHandler.java:259-306`) | Các kết quả giống vậy (§3.3), end to end quan sát cho skip và `ALL` (`LAST` và `NONE` do test Rust). `evaluated_at` được để nguyên | egret không có phục hồi khi khởi động đọc `evaluated_at`; nó vẫn là "tick cuối cùng đã xử lý" |
| **Quan sát.** Dialog, tiêu đề "Enable trigger", đưa ra "Do not recover missed schedules" (được chọn) và "Follow the trigger's recoverMissedSchedules configuration", với Cancel và Enable. Nó không hiện số tick lỡ và không hiện thời điểm kế, và nó mở ở **mọi** lần bật một `Schedule`, có tick lỡ hay không (`FlowTriggers.vue:612-617`) | Hai lựa chọn giống vậy, thêm số tick đã lỡ và việc `recoverMissedSchedules` của trigger sẽ làm gì với chúng. Nó chỉ mở cho một `Schedule` đã tắt có tick kế đã qua; mọi lần bật khác gửi ngay | Người dùng quyết định khi đã thấy cái giá trước mặt, và không bị hỏi khi chẳng có gì để quyết |
| **Đọc trong mã nguồn.** Các hành động unlock, restart và delete-state (`TriggerController.java:161-217, 339`) | Không có | egret không giữ lock được lưu bền (§3.9) |
| **Quan sát.** Hàng loạt theo trigger (`POST /triggers/set-disabled/by-triggers`) trả 202 `{operationId, totalItems}` và chạy bất đồng bộ, không có kết quả cho từng trigger: trigger của một flow không tồn tại bị bỏ không một lời, và một webhook được tính là chấp nhận rồi bị scheduler bỏ. "Theo query" đọc trong mã nguồn (`TriggersManage.vue:790-880`) | Đồng bộ, theo lựa chọn, tối đa 100, có kết quả cho từng trigger (quan sát: `{results: [{…, ok}]}`) | Một transaction cho mỗi trigger đủ nhanh; kết quả theo từng dòng nói rõ điều đã xảy ra |
| **Quan sát.** Trang toàn cục ở `admin/triggers` chuyển hướng tới tab "Add" (catalog) bên cạnh "Manage", nơi có "Export as CSV" | "Triggers" trong nav Workflows; không catalog, không CSV | Không có gì để cài; xuất file là việc theo sau |
| **Quan sát.** Trang toàn cục của Kestra liệt kê các trigger **state**, nên một `Webhook` hay một trigger `Flow` không bao giờ hiện trên đó | Mọi trigger đã khai báo, của mọi kind, parse từ source của các flow (§3.5) | Trang này là nơi người dùng tìm "cái gì có thể khởi động flow của tôi" |
| **Đọc trong mã nguồn.** Bộ lọc: namespace, flow, khoảng ngày, scope, trigger id, worker, enabled/disabled; không có bộ lọc theo kiểu (`ui/src/components/filter/configurations/triggerFilter.ts`). Trang đang chạy hiện "Add filters" | Namespace (prefix), flow, kind, enabled/disabled, một ô tìm văn bản, tất cả trên URL | egret không có worker id; kind là thứ người dùng quét tìm |
| **Quan sát.** Các cột ở tab flow: Id, Type, Execution Id, Last triggered date, Next evaluation date, Last evaluation date, Backfill, Enabled; ở trang toàn cục: Id, Flow, Namespace, Execution Id, Last triggered date, Backfill, Enabled. Mặc định không có cột worker (`workerId` của API là đọc trong mã nguồn) | "Last execution" (id, state và thời gian của nó); không có worker | egret không lưu ngày bắn gần nhất; execution chính là bản ghi |
| **Đọc trong mã nguồn.** Dòng mở rộng nhúng log của trigger | Không có log trigger | egret không ghi: lỗi schedule là `last_error`, lỗi Flow-trigger nằm trong log server (§10) |
| **Quan sát.** Tab flow không có URL webhook; dòng có nút "Send test event". Kestra phục vụ webhook trên chính origin API của nó (`/api/v1/main/executions/webhook/{namespace}/{flow}/{key}`, 200 trên cổng phục vụ UI) | Drawer chi tiết hiện **đường dẫn** của webhook (`/workflows/v1/{ws}/executions/webhook/…`), che cho tới khi Reveal, có Copy và một ghi chú. Chỉ backend phục vụ nó: end to end POST tới cổng riêng của backend; cùng đường dẫn ở origin của UI không được route (quan sát: dev server trả 404) | D17: không routing và không origin; mở webhook ra ngoài là việc theo sau với review bảo mật riêng |
| **Quan sát** ở dòng (không hiện tiến độ); dòng mở rộng chưa được mở | Drawer hiện window đang mở và các điều kiện đã thoả (quan sát: chạy một upstream, window "duration", điều kiện đầu "Met", điều kiện thứ hai "Not met") | egret có dữ liệu (`flow_trigger_windows.results`) |
| **Quan sát.** Trùng trigger id từ chối việc lưu: `POST /flows` trả 422 "Invalid Flow: Duplicate trigger id with name [t]", và flow không được lưu (`core/.../FlowValidator.java:65-68`) | `duplicate_trigger_id` là finding chỉ-khi-lưu: draft vẫn lưu, chỗ trùng được đánh dấu và chặn publish, và các flow đã lưu vẫn chạy (D11) | Ở egret draft lưu được dù có lỗi validation; một lỗi lúc chạy sẽ dừng các flow đã lưu |
| **Quan sát.** Một dòng state mới chép `disabled` của YAML vào `disabled` runtime; từ đó hai giá trị là hai trường riêng (`sourceDisabled`, `disabled`) | Công tắc runtime chỉ nằm trong `workflow.disabled_triggers`; YAML được đọc từ source; các dòng trigger giữ bản sao hiệu lực (D2) | Một lần sửa YAML không nên bị che bởi một giá trị runtime mà nó từng gieo |
| **Quan sát.** Một cột Backfill và các nút | W7f | Quyết định của người dùng |

## 3. Backend

### 3.1 Bảng công tắc

Migration `0189_workflow_disabled_triggers.sql` (quyết định mặc định, 2026-10-14). `0185` là số cuối trên main, nhưng nhánh chưa merge `feat/view-governance` giữ `0186`-`0188`; `0189` không va với bên nào dù merge theo thứ tự nào. Kiểm tra lại mọi ref trước khi merge, vì không gian số là dùng chung.

```sql
CREATE TABLE workflow.disabled_triggers (
  flow_id      uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  -- The trigger's `id:` in the YAML, of any kind.
  trigger_id   text        NOT NULL,
  -- Copied from the flow for filtering; not an authority.
  workspace_id uuid        NOT NULL,
  disabled_by  uuid        REFERENCES users (id) ON DELETE SET NULL,
  disabled_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id)
);

COMMENT ON COLUMN workflow.triggers.disabled IS
  'Effective: flow YAML disabled OR trigger YAML disabled OR a row in workflow.disabled_triggers (W7e).';
COMMENT ON COLUMN workflow.flow_listeners.disabled IS
  'Effective: flow YAML disabled OR trigger YAML disabled OR a row in workflow.disabled_triggers (W7e).';
```

- **Một dòng nghĩa là tắt (D1).** Brief đề xuất một cột `disabled boolean`. Một dòng "enabled" sẽ có nghĩa giống không có dòng, nên bật là xoá dòng và cột bị bỏ. Ai tắt và lúc nào được giữ lại cho UI.
- **Vì sao một bảng riêng:** `sync` xoá mọi dòng `workflow.triggers` và `flow_listeners` của một flow có source không parse được (`crates/egret-workflow/src/scheduler/sync.rs:55-59, 76-82, 215-223`), và một `Webhook` không có dòng nào cả (`sync.rs:1-4`). Công tắc phải sống sót qua cả hai.
- **Ý nghĩa của các cột cũ thay đổi,** nên migration nói điều đó bằng `COMMENT ON` thay vì sửa văn xuôi của 0156 (`migrations/0156_workflow_triggers.sql:22-24`) và 0157 (`0157_workflow_flow_triggers.sql:32`), những chỗ này giữ nguyên như đã viết.

### 3.2 Thực thi: bản sao hiệu lực (D2)

`effective_disabled = flow YAML disabled || trigger YAML disabled || row in disabled_triggers`.

- **`sync`** (`sync.rs:48`) đọc các id `disabled_triggers` của flow một lần, trong transaction của nó, và ghi giá trị hiệu lực ở chỗ hiện nay nó ghi `disabled`: `workflow.triggers` (`sync.rs:89, 119-134`) và `workflow.flow_listeners` (`sync.rs:184, 225-245`).
- **Route công tắc** ghi cùng giá trị đó lên dòng hiện có, trong transaction ghi `disabled_triggers` (§3.3).
- **Những gì tuân theo nó mà không phải sửa:**
  - việc claim schedule, `AND NOT t.disabled` (`scheduler/fire.rs:144-158`), nên một schedule tắt không bao giờ được claim và không tiêu tốn một tick; index due vẫn là partial trên `NOT disabled` (`0156:35-36`);
  - việc chọn listener Flow-trigger, `AND NOT l.disabled` (`flow_trigger/process.rs:572-589`);
  - insert outbox, chỉ ghi một event nếu có listener với `NOT l.disabled` nghe nó (`worker/claim.rs:2477-2491`).
- **Vì sao là bản sao chứ không phải join:** ba câu lệnh nóng giữ nguyên hình dạng, các partial index `workflow_triggers_due` và `workflow_flow_listeners_ws` (`0157:35`) vẫn đúng, và chỉ có một hàm tính giá trị.
- **Route webhook** không có dòng nào để đọc. Sau khi key khớp và sau khi kiểm tra trigger YAML (`webhook_routes.rs:559-566`), nó đọc `disabled_triggers` cho `(flow, trigger.id)` và trả cùng 409 như một trigger bị tắt trong YAML: "Cannot execute webhook: the trigger 'x' is disabled." Việc tra cứu chỉ chạy sau khi key đã khớp, nên bốn trường hợp vắng mặt vẫn là một 404 không phân biệt được (`webhook_routes.rs:107-125`).
- **`fire_one`** vẫn so source với dòng đã claim (`fire.rs:204-213`); một dòng tắt không được claim. `resync` của nó (nhánh Frozen, gọi `sync`) giờ đọc flow `FOR SHARE SKIP LOCKED` (xem **Đồng thời** bên dưới).
- **Window (D7).** Window đang mở trong `flow_trigger_windows` của một listener `dependsOn` bị tắt được để yên. Nó không được nạp khi đang tắt, hết hạn ở `window_end` và `sweep_expired_windows` xoá nó (`process.rs:280-292`), đúng như window của một listener bị tắt trong YAML hiện nay (dòng listener vẫn còn, `sync.rs:184`, nên cascade từ `0158` không chạy). Bật lại trong window thì giữ các điều kiện đã thoả trước khi tắt.

**Đồng thời.** Route công tắc lấy lần lượt: dòng flow `FOR NO KEY UPDATE`, rồi dòng `workflow.triggers` hoặc `flow_listeners` của trigger `FOR UPDATE`. Đây là thứ tự của một lần lưu (flow, rồi các dòng trigger; câu lệnh 4 của `scheduler/mod.rs`).
- Với một lần lưu hoặc `reconcile` (`sync.rs:319-334`, flow `FOR SHARE`): được tuần tự hoá trên dòng flow, nên `sync` không bao giờ tính giá trị hiệu lực từ một công tắc sắp đổi. Không có flow lock thì một lần lưu có thể đọc công tắc cũ, chờ trên dòng mà route đang giữ, rồi ghi đè giá trị của route: mất cập nhật.
- Với một pass: pass giữ dòng trigger và muốn `KEY SHARE` trên flow, thứ `FOR NO KEY UPDATE` không chặn, nên không có chu trình. Route chờ một lần bắn đang chạy commit. **Sau khi route trả lời, không có lần bắn mới nào của schedule đó bắt đầu.**
- Với `resync` của một pass (`fire.rs`, nhánh Frozen: dòng đã claim không khớp source và `sync` chạy lại trong transaction của pass): `resync` đọc flow `FOR SHARE SKIP LOCKED`. Không có khoá flow, nó có thể đọc các dòng công tắc trước khi route commit, chờ dòng trigger mà route giữ, rồi ghi giá trị cũ đè lên giá trị của route (review I1: mất cập nhật, dòng công tắc tắt mà bản sao bật, nên claim vẫn bắn). Một `FOR SHARE` có chờ sẽ deadlock khi route bật/tắt chính trigger đã claim (route giữ flow và muốn dòng trigger; pass giữ dòng trigger và muốn flow), còn `NOWAIT` sẽ huỷ transaction của pass. Khi flow đang bị giữ, `resync` không làm gì: trigger đã claim vẫn đóng băng (không bắn, giữ nguyên ngày), lần ghi của bên giữ khoá tự sửa bản sao, và một pass sau resync dòng nào vẫn còn lệch. `FOR SHARE` không xung đột với `KEY SHARE` của lần insert execution khi bắn, cũng không với `FOR SHARE` của `reconcile`. Test: `disabled_triggers.rs` `a_resync_does_not_undo_a_concurrent_switch_on_another_trigger`, `a_resync_of_the_trigger_being_switched_does_not_deadlock`.
- Một pass Flow-trigger đọc listener không có lock (`process.rs:570-571`), và một request webhook có thể đã đọc công tắc trước khi nó đổi. Cả hai có thể bắn thêm một lần sau khi route trả lời. Điều này được nêu trong mô tả API.

### 3.3 Bật và tắt

`PUT /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/disabled` yêu cầu `workflow:manage` (`routes.rs:108`), như các thao tác ghi flow khác.

```json
{ "disabled": false, "recover_missed": false }
```

- **Từ chối:**
  - 404 khi flow không thuộc workspace này hoặc nằm trong thùng rác (`FlowRepo::get` mang vị từ workspace; thùng rác được kiểm tra như `startable_flow` làm, `routes.rs:2262-2270`), hoặc source không khai báo trigger nào có id đó.
  - 400 khi source không parse được (công tắc không thể biết kind của trigger).
  - 409 `trigger_disabled_in_source` với `disabled: false` khi flow hoặc trigger là `disabled: true` trong YAML (D4). Việc **tắt** một trigger như vậy được chấp nhận, để công tắc đã đặt sẵn khi YAML được sửa.
  - 409 `trigger_not_supported` cho một trigger `Leaf`.
- **Tắt:** insert dòng (idempotent), cập nhật bản sao hiệu lực.
- **Bật:** xoá dòng, cập nhật bản sao hiệu lực. Với một `Schedule` **đã bị tắt** (lệnh xoá đã xoá một dòng) có dòng đang ở trong hàng đợi (`next_evaluation_date IS NOT NULL`) và ngày tới đã qua, áp dụng `recover_missed`:
  - `false` hoặc vắng (`SKIP` mặc định của Kestra): `next_evaluation_date` = tick đầu tiên sau `now()` của transaction, tính như `sync` tính một ngày đầu tiên (`TriggerState::initialise`, `trigger/mod.rs:245-260`, với `FixedClock(now)`, `sync.rs:93-95`).
  - `true`: theo `recoverMissedSchedules` của trigger:
    - vắng hoặc `ALL`: giữ các ngày; mọi tick đã lỡ được chạy lại, mỗi pass một tick (`trigger/schedule.rs:226-229`).
    - `NONE`: như `false`.
    - `LAST`: `next_evaluation_date` = tick mới nhất tại hoặc trước `now()` (`ScheduleTrigger::last_tick_at_or_before` mới, vì `cron.last_before` hiện có, `schedule.rs:1391`, là strictly-before và private, nên sẽ bỏ sót tick đúng tại `now()`) khi nó sau `evaluated_at`; nếu không thì giữ nguyên. Khi không có `evaluated_at` (chưa từng bắn), như `NONE`. Đây là `updateForLastMissedSchedule` của Kestra (`TriggerEventHandler.java:283-306`).
  - `evaluated_at` không bị đụng tới, `last_error` cũng vậy, trừ khi một lần skip mà cron không còn tick nào sau `now()` đưa dòng ra khỏi hàng đợi với lý do của `sync`, đúng như `sync` lưu. Một dòng đã rời hàng đợi vẫn ở ngoài; lần lưu kế tiếp đưa nó vào lại (`sync.rs:111-118`). `recover_missed` bị bỏ qua với các kind khác.
  - **Chỉ với trigger đã bị tắt** (quyết định mặc định, 2026-10-15). Bật một trigger đang bật thì không xoá gì và không đổi gì: các ngày được giữ, `recover_missed_unknown` không được kiểm tra, và không ghi sự kiện audit nào (tắt một trigger đang tắt cũng vậy). Một schedule đang bật mà quá hạn là đang bắt kịp sau thời gian ngừng hoặc đang chạy lại backlog `ALL` mỗi pass một tick; một lần bật với `recover_missed: false` mặc định, kể cả Enable hàng loạt trên một lựa chọn lẫn lộn, không được vứt bỏ backlog đó.
- **Một lần lưu sau đó không huỷ một lần skip.** `sync` giữ các ngày của dòng đang trong hàng đợi khi `spec_key` (`cron|withSeconds|timezone`, `sync.rs:338-345`) không đổi (`sync.rs:126-134`), nên ngày đã chuyển vẫn đứng vững. Một `spec_key` đổi thì khởi tạo lại về tick kế tiếp sau now, cũng là một lần skip.
- **`recoverMissedSchedules` được đọc lần đầu tiên.** `models::Schedule` hiện bỏ nó (`models/trigger.rs:93-96`) và `serde` không có `deny_unknown_fields`, nên nó được parse rồi bị bỏ qua. W7e bắt nó như `lateMaximumDelay` được bắt (`models/trigger.rs:130-150`): một giá trị thô, được giữ khi serialise lại. Chỉ route này đọc nó. Giá trị khác `ALL`, `NONE` hoặc `LAST` trả 409 `recover_missed_unknown` khi `recover_missed: true`; các đường khác của scheduler không đổi (§10).
- **Câu trả lời:** mục của trigger, như `GET …/triggers` trả về (§3.5).
- **Audit:** `workflow_trigger:disable` / `workflow_trigger:enable`, resource `workflow_trigger`, id `{flow uuid}/{trigger_id}`, kèm `recover_missed` và ngày đã chuyển, qua `st.audit.emit` như `update_flow` làm (`routes.rs:787-797`).

### 3.4 Vòng đời của công tắc (D6)

Bên trong `sync`, sau các dòng:
- **Source parse được:** xoá các dòng `disabled_triggers` của flow có `trigger_id` mà source không còn khai báo, ở mọi kind. Một trigger id được đổi tên là một trigger mới và bắt đầu ở trạng thái bật, như Kestra.
- **Source không parse được:** giữ mọi dòng. Không công tắc nào của người dùng bị mất vì một draft hỏng.
- **`gone` (xoá mềm):** xoá hết. `ON DELETE CASCADE` không chạy khi xoá mềm, và việc xoá đã bỏ mọi dòng trigger để một flow trong thùng rác không giữ state trigger nào (`repo.rs:655-679`). Không có route khôi phục.
- **Đổi kind dưới cùng một id** giữ dòng: công tắc là theo id, như state của Kestra.

Tiền đề của brief "một lần lưu không hợp lệ hay không parse được không được xoá các override" đúng như một rule, với một điều chỉnh: API không thể lưu một source không parse được (`update_flow` và `create_flow` gọi `parse_draft`, `routes.rs:519-529`, trả 400). Một source đã lưu vẫn có thể không parse được sau một thay đổi parser, và `reconcile` chạy `sync` trên mọi flow ở mỗi lần khởi động (`sync.rs:300-317`). Đó là trường hợp rule bảo vệ. Một source parse được nhưng không qua validation vẫn lưu bình thường (`parse_draft` không validate), và các trigger id của nó được đọc như đã khai báo.

### 3.5 Đọc trigger

Cả hai route yêu cầu `workflow:read` (`routes.rs:105`).

**`GET /api/ws/{workspace}/workflows/flows/{id}/triggers`**: một mục cho mỗi trigger đã khai báo, theo thứ tự khai báo.

| Trường | Nguồn |
|---|---|
| `trigger_id`, `type`, `kind` (`schedule` / `webhook` / `flow` / `other`) | YAML |
| `yaml_disabled` (flow hoặc trigger), `runtime_disabled`, `disabled_by`, `disabled_by_name`, `disabled_at`, `effective_disabled` | YAML, `disabled_triggers` (tên được đọc trong cùng câu lệnh; trình duyệt không phân giải được user id vì danh sách user chỉ dành cho admin) |
| `can_toggle`, và `toggle_refusal` khi false | D3, D4, `workflow:manage` của người gọi (`caller_may_manage`, `routes.rs:1651-1660`) |
| `duplicate_id` | D11 |
| `schedule`: `cron`, `with_seconds`, `timezone` (vắng = UTC), `next_evaluation_date`, `evaluated_at`, `last_error`, `off_queue`, `missed_ticks` (khi bị tắt hoặc quá hạn; đếm tới 1.000, rồi "1000+"), `recover_missed_schedules` | YAML, `workflow.triggers` |
| `flow`: `states`, `listening` (có một dòng `flow_listeners`) và `not_listening_reason` (một trigger không window mà `resolve` thất bại thì không có dòng, `sync.rs:194-211`), `depends_on`: `mode`, `min_satisfied`, `conditions` [`{namespace, flow_id, states, labels, when, met}`], `window` `{kind, start, end}` hoặc không có | YAML, `flow_listeners`, `flow_trigger_windows` |
| `webhook`: `url_path` (`/workflows/v1/{ws}/executions/webhook/{namespace}/{flow}/{key}`, `webhook_routes.rs:469-472`), `key_error` khi key không render được, `unsupported` (`declared_unsupported`) | YAML, key đã render |
| `last_execution`: `{id, state, created_at}` | §3.6 |

Và ở trên cùng: `flow_disabled` (YAML hoặc `state = 'disabled'`), `flow_deleted`, `scheduler_enabled`, `worker_enabled`, `webhooks_enabled`.

- **Các điều kiện** được gán nhãn bằng cách tính lại `condition_key` (`trigger/depends_on.rs:92`) cho từng mục `dependsOn`; `met` là `results[key] == true`. Một window có `window_end <= now()` được báo là không có (sweep hoặc event kế tiếp sẽ xoá nó).
- **Key webhook.** Nó render dựa trên id và namespace của flow, `variables` của flow, và chỉ id cùng kiểu của trigger (`KeyContext::render_context_for`, `trigger/webhook.rs:436-452`), tất cả đọc từ source mà `GET /flows/{id}` đã trả cho `workflow:read` (`FlowDto.source`, `routes.rs:314-321`). Vì vậy việc trả URL đã render **không mở rộng** ai có thể biết key. UI hiện nó dạng che cho tới khi được bấm xem.
- **Cờ server.** `WorkflowState` (`routes.rs:200-251`) có thêm `trigger_flags { scheduler_enabled, worker_enabled, webhooks_enabled }`, đặt từ `config.workflow` (`egret-config/src/lib.rs:596, 619, 689`) ở nơi state được dựng (`egret-bin/src/main.rs:2302`). Deployment đặt chúng một lần cho mọi replica (`deploy/helm/egret/values.yaml:86-98`).
- **Một flow có `state` là `disabled`** (D18) trả mọi trigger với `effective_disabled: true`, và `state=disabled` của danh sách gồm chúng; `yaml_disabled`, `runtime_disabled` và `can_toggle` không bị ảnh hưởng.
- **Một flow trong thùng rác** trả trigger của nó với `flow_deleted: true`, mọi trigger đều effective-disabled và `can_toggle: false` (`get_flow` phục vụ draft trong thùng rác, `routes.rs:695-700`).

**`GET /api/ws/{workspace}/workflows/triggers`**: mọi trigger của các flow trong workspace không nằm trong thùng rác.

- **Query:** `namespace_prefix` (neo theo dấu chấm, như của W7b), `flow_id` (uuid), `kind` (phân tách bằng dấu phẩy), `state` (`enabled` / `disabled`, trên `effective_disabled`), `q` (chuỗi con không phân biệt hoa thường của trigger id, flow id hoặc namespace), `limit` (mặc định 50, tối đa 200, bị kẹp), `offset`.
- **Dòng:** mục theo từng flow được làm phẳng, cộng `flow_uuid` (uuid của flow; đặt tên như vậy vì `flow` của chính mục là thông tin của một trigger `Flow`, và một object không mang được hai khoá cùng tên), `namespace`, `flow_id` (id trong YAML) và `flow_disabled` (cờ cấp trên của route theo từng flow, theo từng dòng: YAML của flow hoặc `state = 'disabled'`). Một dòng không mang đường dẫn webhook (`webhook` là null) và không mang điều kiện (`depends_on.conditions` là null): drawer đọc route theo từng flow.
- **Trang:** `{rows, total, limit, offset, unparsable, scheduler_enabled, worker_enabled, webhooks_enabled, can_manage}`; `unparsable` đếm các flow trong phạm vi có source không parse được (trigger của chúng không biết); `can_manage` là quyền `workflow:manage` của người gọi trong workspace, theo người gọi chứ không theo dòng, nên một trang mà mọi dòng đều bị từ chối vì lý do khác vẫn cho biết công tắc và route hàng loạt có thuộc về người gọi hay không. Thứ tự: namespace, flow id, thứ tự khai báo.
- **Cách làm (D9):** các flow trong phạm vi được đọc với bộ lọc namespace và flow trong SQL, tối đa 2.001 (hình dạng của `dependency_drafts`, `repo.rs:381-397`). Hơn 2.000 trả 422 `triggers_too_large`, "more than 2000 flows match; narrow by namespace". Các source được parse trên blocking pool (`off_runtime` của W7d, `dependencies.rs:213`, nâng lên `pub(crate)` với thông báo lỗi trung tính, vì hiện nó private và thông báo của nó nêu tên đồ thị phụ thuộc). Các bảng state được đọc, mỗi bảng một câu lệnh, cho các flow id trong phạm vi. Bộ lọc kind, state và tìm kiếm lọc trong bộ nhớ, rồi trang được cắt, rồi `last_execution` chỉ được đọc cho trang đó.
- **Vì sao phải parse:** webhook và trigger `Leaf` không có dòng ở đâu cả, và chỉ source biết chúng.

### 3.6 Execution gần nhất theo từng trigger (D8)

```sql
SELECT k.flow_id, k.trigger_id, e.id, e.state, e.created_at
  FROM unnest($1::uuid[], $2::text[]) AS k(flow_id, trigger_id)
  CROSS JOIN LATERAL (
    SELECT id, state, created_at FROM workflow.executions e
     WHERE e.flow_id = k.flow_id
       AND e.execution_trigger IS NOT NULL
       AND e.execution_trigger->>'id' = k.trigger_id
     ORDER BY e.created_at DESC LIMIT 1) e;
```

- **Hiện chưa index nào phủ được nó.** `workflow_executions_flow_idx` chỉ trên `flow_id` (`0134:93`), nên mỗi lần dò đọc mọi execution của flow; `workflow_executions_one_per_schedule_tick` (`0156:49-51`) chỉ phủ schedule và sắp theo `scheduled_date`.
- **Migration thêm** `CREATE INDEX workflow_executions_last_by_trigger ON workflow.executions (flow_id, (execution_trigger->>'id'), created_at DESC) WHERE execution_trigger IS NOT NULL`: một lần đi xuống index cho mỗi lần dò (Index Scan, không phải index-only: `id` và `state` được đọc từ heap), tối đa 200 lần dò mỗi trang.
- **Nó được đo, như 0183 đã đo** (`0183:74-85`): một test ghim `EXPLAIN` dùng index. Nếu planner không chọn nó, index bị bỏ khỏi migration và số đo được ghi vào đó thay thế.
- **Chi phí:** thêm một index cho mỗi lần insert execution do trigger sinh ra; execution khởi động từ API hay bằng replay không có `execution_trigger` (`routes.rs:2656, 2932`: `trigger: None`) và nằm ngoài partial index.

### 3.7 Bật/tắt hàng loạt (D10)

`PUT /api/ws/{workspace}/workflows/triggers/disabled` yêu cầu `workflow:manage`.

```json
{ "triggers": [{ "flow_id": "…uuid…", "trigger_id": "daily" }], "disabled": true, "recover_missed": false }
```

- Tối đa 100 mục (400 nếu vượt); giới hạn body 64 KiB.
- Mỗi mục chạy §3.3 trong transaction riêng, theo thứ tự đã cho. Các mục độc lập: một lần từ chối được ghi lại và mục kế tiếp vẫn chạy.
- 200 với `{results: [{flow_id, trigger_id, ok, code?, message?, trigger?}]}`. Một flow ngoài workspace là `not_found`, không bao giờ là 403.
- Một audit event cho mỗi trigger được bật/tắt.

### 3.8 Trùng trigger id (D11)

`spec::validate` có thêm `duplicate_trigger_id` (lỗi). Kestra từ chối y như vậy (`FlowValidator.java:65-68`). Khác với `duplicate_task_id` (`spec/validate.rs:593`, phát ra trong `validate_for_run`), nó nằm trong `save_only_findings`: hiện trên draft và chặn publish, và **không** nằm trong `validate_for_run`. `executor::build_plan` gọi `validate_for_run` lúc chạy (`validate.rs:335-338`), nên một lỗi lúc chạy ở đó sẽ chặn mọi execution của flow đã lưu mà đang khai báo một trigger id hai lần, một thay đổi hành vi mà gói này không đòi hỏi. Draft vẫn lưu được với lỗi validation, nên các route đọc đánh dấu bản trùng (`duplicate_id: true`) và công tắc áp dụng cho id, thứ duy nhất các bảng trigger hiện có thể khoá theo: khoá chính của chúng là `(flow_id, trigger_id)`, nên `sync` đã gộp hai schedule cùng một id thành một dòng (`sync.rs:119-145`).

### 3.9 Những gì không xây, và vì sao

- **Unlock.** egret không lưu lock bền: lock của claim là một row lock giữ trong một transaction (`fire.rs:157`; câu lệnh 1 của `scheduler/mod.rs`). Không có gì để unlock.
- **Restart và delete-state.** "Xoá state" của một schedule sẽ khởi tạo lại nó về tick kế tiếp, việc bật lại với "skip" đã làm. Ghi ở §10.
- **Backfill.** W7f.

### 3.10 Các comment cần sửa

Mỗi chỗ đã được kiểm tra trong worktree này:
- `crates/egret-workflow/src/lib.rs:92-96` nói các route "tạo backfill" nằm ngoài module. Không có route nào. Văn bản mới: vòng lặp điều khiển đồng hồ nằm trong `scheduler`; backfill chưa có route (W7f).
- `crates/egret-workflow/src/routes.rs:2271-2274` và `webhook_routes.rs:569-571` nói cột `state` là "cột mà UI quản trị ghi". Không code nào ghi `state = 'disabled'`: các câu lệnh `UPDATE workflow.flows` duy nhất là `repo.rs:552`, `:664` và `:739` (`state = 'live'`). Văn bản mới: cột này được tôn trọng nhưng chưa có gì ghi nó; công tắc runtime là theo từng trigger (`workflow.disabled_triggers`).
- `crates/egret-workspace/src/features.rs:312-315` bảo operator dừng traffic webhook bằng `state = 'disabled'` trên flow, một công tắc không ai đặt được. Văn bản mới: công tắc runtime của trigger hoặc `disabled: true` trong YAML. `webhook_routes.rs:33-36` thêm công tắc trigger bên cạnh YAML.

## 4. Frontend

Mọi thứ nằm trong khu vực Workflows tải lười của W7a (FE-ROUTE-2). Module mới: `components/workflows/triggers/` (`TriggersTable`, `TriggerDetailsDrawer`, `EnableTriggerDialog`, `triggersModel.ts`) và `routes/workflows/TriggersList.tsx`. Client được sinh lại bằng `npm run gen:api` (FE-DATA-1); các key ở `routes/workflows/queryKeys.ts` (FE-DATA-2).

### 4.1 Tab Triggers của flow

`FLOW_TABS` trở thành `["edit", "revisions", "executions", "triggers"]` (`FlowDetail.tsx:52-53`); URL là `flows/:id/triggers` (FE-PAGE-2).

- **Cột** (`ui/table`, FE-PAGE-4): chọn, ID, Type (icon kind và tên kiểu ngắn), Next run, Last tick (`evaluated_at`, nhãn "Last evaluated" với tooltip nói đó là tick đã xử lý, không phải lần bắn theo đồng hồ), Last execution (chip state, thời gian tương đối, liên kết), Status, Enabled (công tắc), Details.
- **Status** nói bằng lời vì sao một trigger không bắn: "Disabled in YAML", "Switched off by {disabled_by_name} {time}", "Off the queue: {last_error}", "Not listening: {reason}", "Not run by egret" (Leaf), "Key does not render", và cả "The flow is in the trash", "On, but the flow is disabled, so it does not fire" (D18), "Not served by egret: {reason}" (một tuỳ chọn webhook đã khai báo mà egret từ chối), và "Active" khi không có gì chặn. Nguyên nhân đầu tiên thắng, theo thứ tự: YAML, công tắc runtime, flow, rồi trạng thái riêng của kind. Không bao giờ chỉ bằng màu (FE-A11Y).
- **Công tắc.**
  - Bị vô hiệu hoá kèm lý do (FE-ACTION-4): "Disabled in the flow's YAML", "Not run by egret", "You need workflow:manage", "The flow is in the trash".
  - Tắt thì gửi PUT ngay.
  - Bật một `Schedule` có ngày tới đã qua thì mở `EnableTriggerDialog` (§4.3); mọi trigger khác bật ngay.
  - Công tắc chỉ di chuyển khi server trả lời (FE-ACTION-3: một lần từ chối hiện thông điệp của server và để công tắc nguyên chỗ cũ).
- **Hàng loạt:** "Enable" và "Disable" tác động lên phần đã chọn qua route hàng loạt; "Enable" khi phần chọn có một `Schedule` mở dialog ở dạng hàng loạt; kết quả nêu tên các trigger bị từ chối và lý do.
- **Trạng thái rỗng** (FE-STATE-3): "This flow has no triggers" kèm liên kết tới editor.
- **Banner** phía trên bảng, mỗi cái nói phải làm gì:
  - `scheduler_enabled` false và flow có trigger `Schedule` hoặc `Flow`: "The scheduler is off on this server, so schedules and flow triggers do not fire. An administrator sets `workflow.schedulerEnabled`."
  - `worker_enabled` false: "The worker is off, so triggered executions are created but do not run."
  - `webhooks_enabled` false và flow có một `Webhook`: "Webhooks are off on this server; their URLs answer 404."
  - Flow bị tắt (YAML hoặc state): "This flow is disabled, so none of its triggers fire."
  - Không có banner cho draft: draft có bắn (`process.rs:908-911`; `flow_is_on`, `scheduler/start.rs:51-67`, chỉ loại `state = 'disabled'`).
- **Làm mới:** tab refetch mỗi 30 giây khi đang hiển thị và sau mỗi lần bật/tắt (FE-ACTION-2).

### 4.2 Drawer chi tiết

Mở từ Details hoặc ID của một dòng; `?trigger=<id>` trên URL để chia sẻ được (FE-PAGE-2) (trên trang toàn cục là `?trigger=<flow uuid>/<trigger id>`, §4.4).

- **Định nghĩa:** khối YAML của trigger, chỉ đọc, kèm liên kết "Edit in the editor" mở Code tại khối đó. `key:` của một webhook cũng bị ẩn trong khối này cho tới khi Reveal, và cả khối bị ẩn khi không tách được key khỏi phần còn lại (thất bại theo hướng đóng).
- **Schedule:** cron, timezone, lần chạy tới theo timezone của trigger và của người xem, tick cuối, lỗi cuối, số tick đã lỡ khi bị tắt.
- **Flow trigger:** các state được nghe; với `dependsOn`, mode, window (kind, bắt đầu và kết thúc, thời gian còn lại), và từng điều kiện với đã thoả hoặc chưa thoả. "No window is open" khi không có.
- **Webhook** (quyết định mặc định, 2026-10-14): **đường dẫn** `url_path` (`/workflows/v1/{ws}/executions/webhook/{namespace}/{flow}/{key}`), được che với Reveal và Copy (nút copy báo "Copied" trong một vùng `aria-live`), và một ghi chú rằng nó do backend egret phục vụ tại host nào operator mở cho webhook. Nếu `webhooks_enabled` là false, vẫn hiện cảnh báo "Webhooks are off on this server". W7e **không** dựng URL từ `window.location.origin` và **không** thêm routing: không có gì ở origin của trình duyệt chuyển `/workflows/` tới backend (Helm ingress, `nginx-prod.conf`, Vite proxy), nên URL như vậy sẽ nhận `index.html` của SPA với 200 và không bao giờ bắn, và thêm routing sẽ mở ra một bề mặt ẩn danh mà review của nó ghi "not approved for exposure" (`webhook_routes.rs:60-64`). Mở webhook qua ingress là việc tiếp theo, cần review bảo mật riêng.
- **Last execution:** liên kết tới nó.

### 4.3 Dialog bật

Cho một trigger hoặc cho các `Schedule` trong phần chọn hàng loạt.

- **Tiêu đề:** "Enable trigger" / "Enable {n} triggers".
- **Lựa chọn** (radio group, FE-FORM-2):
  - "Skip missed runs; start at the next tick" (mặc định, `SKIP` của Kestra). Nó không nêu thời điểm: trình duyệt không có thư viện cron và §8 không cho thêm dependency mới, nên tick kế tiếp là của server, hiện trong tab khi công tắc trả lời (sửa 2026-10-15).
  - "Recover missed runs ({n} missed)". Mô tả của nó theo `recoverMissedSchedules` của trigger: "runs every missed tick, one per scheduler pass" (`ALL`), "runs only the latest missed tick" (`LAST`), "skips them: this trigger's `recoverMissedSchedules` is NONE" (`NONE`).
- **Nút:** Cancel, Enable.

### 4.4 Trang Triggers toàn cục

`/explore/:workspace/workflows/triggers` (FE-ROUTE-3), một `ListPageShell` (FE-PAGE-1). Sidebar: "Triggers" sau "Namespaces" trong nhóm Workflows (`components/layout/Sidebar.tsx:55-59`), feature `workflows`, và route nằm cạnh các route khác (`routes/workflows/WorkflowsRoutes.tsx:64-82`).

- **Bộ lọc trên URL** (FE-PAGE-2): namespace prefix (một picker trên danh sách namespace của W7d, FE-FORM-3), flow, kind, Enabled / Disabled, tìm kiếm, trang.
- **Cột:** của tab flow, thêm Namespace và Flow (liên kết), bớt tên dài của Type.
- **Hàng loạt** enable và disable như ở tab flow.
- **Trạng thái** (FE-STATE-1/2/3): skeleton, lỗi trước, "No triggers match these filters" kèm Clear filters. Một 422 `triggers_too_large` hiện thông điệp của nó; nó chỉ chuyển focus vào bộ lọc namespace sau một thay đổi do người dùng làm bằng một picker, bộ phân trang hoặc Clear filters, không ở lần tải đầu của trang và không khi người dùng đang gõ trong ô tìm kiếm (D9, FE-A11Y-6). `unparsable > 0` hiện "{n} flows could not be read; their triggers are not listed".
- **Drawer** ở đây là `?trigger=<flow uuid>/<trigger id>`, vì hai flow có thể khai báo cùng một trigger id; nó đọc route theo từng flow của flow đó và chính flow cho phần Định nghĩa, và nêu tên flow trong tiêu đề.
- **Quyền** lấy từ `can_manage` của trang (§3.5), không từ các dòng.
- **Cùng các banner** cho các cờ server.

### 4.5 Quyền

Người xem có `workflow:read` mà không có `workflow:manage` thấy mọi thứ; các công tắc, ô chọn và nút hàng loạt bị vô hiệu hoá kèm "You need workflow:manage" (FE-ACTION-4). `toggle_refusal` của server nêu lý do đầu tiên áp dụng, theo thứ tự thùng rác, `Leaf`, quyền của người gọi, YAML, nên người xem như vậy cũng đọc "You need workflow:manage" trên một trigger bị tắt trong YAML (quan sát 2026-10-15).

### 4.6 Chunk

Các module mới chỉ tải bên trong chunk Workflows; `test:bundle:workflows` (`frontend/package.json:14`) có thêm marker cho `TriggersList` và `FlowTriggersTab`.

## 5. Lỗi

- 404: một flow hoặc trigger id không biết ("This trigger no longer exists in the flow; reload").
- 409 `trigger_disabled_in_source`, `trigger_not_supported`, `recover_missed_unknown`: câu của server.
- 400: một source không parse được, một danh sách hàng loạt quá 100.
- 422 `triggers_too_large`: §4.4.
- Lỗi mạng nói điều đã xảy ra và phải làm gì (FE-COPY-2).

## 6. Test

**Rust** (`tests/` của `egret-workflow`, mỗi test có database riêng):
- Bảng công tắc và `sync`:
  - bản sao hiệu lực trên cả hai bảng, với từng trong ba nguyên nhân;
  - một lần lưu hợp lệ bỏ một id thì xoá dòng công tắc của nó; một source không còn parse được (ghi bằng SQL, như một thay đổi parser sẽ để lại) giữ nó, và `reconcile` giữ nó; một xoá mềm xoá nó; dòng của flow khác không bị đụng tới.
- Thực thi:
  - một schedule bị tắt không được claim và giữ nguyên các ngày của nó;
  - một listener bị tắt không được chọn và không có outbox event nào được ghi chỉ cho nó;
  - webhook trả 409 với câu của YAML; một key sai vẫn trả body 404 dùng chung (`the_four_absences_are_indistinguishable` được mở rộng với một trigger bị tắt).
- Bật lại: `recover_missed` false chuyển ngày sang tick kế tiếp; true với vắng, `ALL`, `NONE`, `LAST` mỗi cái cho kết quả của nó; một dòng off-queue vẫn tắt; một lần lưu sau đó với cùng cron giữ ngày đã chuyển; một lần với cron mới khởi tạo lại.
- Từ chối: bật YAML-disabled (409), Leaf (409), id không biết (404), flow của workspace khác (404, không phải 403), flow trong thùng rác (404), chỉ có `workflow:read` (403).
- Đồng thời: một lần tắt gặp một pass đang giữ dòng thì chờ, và lần bắn của pass là lần cuối (hình dạng của `a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance`); một lần lưu và một lần bật/tắt trên cùng một flow kết thúc với giá trị của công tắc.
- Đọc: mọi trường của §3.5 theo từng kind; điều kiện có nhãn và `met`; window hết hạn được báo là không có; path webhook đã render; `missed_ticks` bị chặn trần; bộ lọc, phân trang của danh sách toàn cục, 422 ở 2.000 flow và `unparsable`; plan của last-execution dùng index.
- Hàng loạt: 100 được chấp nhận, 101 bị từ chối, kết quả lẫn lộn.
- Validation: `duplicate_trigger_id`.
- Mutation: bỏ số hạng công tắc khỏi giá trị hiệu lực của `sync`; bỏ việc tra cứu webhook; bỏ flow lock trong route (test đồng thời phải chết).

**Vitest:** các cột của tab và chữ status theo từng state; lý do vô hiệu hoá của công tắc; các lựa chọn của dialog và body mỗi lựa chọn gửi; kết quả hàng loạt; drawer theo từng kind và nút copy; bộ lọc URL của trang toàn cục; người xem chỉ đọc.

**Playwright** (`frontend/tests/e2e/workflows-triggers.spec.ts`), trên một stack riêng với scheduler, worker và webhook bật, không bao giờ trên stack dev dùng chung:
1. Một flow có schedule `* * * * *`, một webhook và một trigger Flow `dependsOn` trên hai flow upstream. Tab Triggers hiện từng cái với lần chạy tới và URL webhook; chế độ sáng và tối.
2. Tắt schedule; chờ qua hai tick; không execution nào được tạo. Bật với "Skip": lần chạy tới ở tương lai và không execution lỡ nào xuất hiện. Lặp lại với "Recover": các tick đã lỡ chạy.
3. Tắt webhook: một `curl` tới URL của nó trả 409; bật lại: 200. Request đi tới cổng riêng của backend với `url_path`, vì W7e không thêm routing nào cho webhook (D17).
4. Chạy một flow upstream: drawer hiện window và một điều kiện đã thoả; chạy flow còn lại: flow downstream chạy.
5. Trang toàn cục: lọc theo namespace và kind trên URL, tắt hàng loạt hai trigger, tải lại, thấy chúng tắt.
6. Một người xem không có `workflow:manage` thấy các công tắc bị vô hiệu hoá kèm lý do.
7. Một trigger có `disabled: true` trong YAML: công tắc bị vô hiệu hoá với "Disabled in the flow's YAML".

## 7. So sánh với Kestra

Task cuối của kế hoạch chạy kịch bản 1-3 và 5 của §6 trên image Kestra cục bộ (`kestra/kestra` `28f59bf175e1`, `server local`, một container dùng một lần), và kiểm tra cụ thể: công tắc runtime với một webhook và với một trigger Flow, bật một trigger YAML-disabled qua API, và cả hai lựa chọn của dialog. Mỗi dòng của §2 mà nó đo được sẽ được gán lại nhãn **quan sát**; khác biệt nó tìm ra được thêm vào đó (FE-REF-2).

## 8. Các rule gói này phải đáp ứng

- Bộ rule frontend, mọi rule.
- Không thêm dependency mới.
- Chỉ sinh lại client bằng `npm run gen:api`.
- Văn xuôi migration là chỉ-thêm: 0156 và 0157 không được sửa (§3.1).

## 9. W7f (backfill) không bị làm bất khả thi

- Mục trigger và dòng danh sách là object; W7f thêm một trường `backfill` vào cả hai.
- Công tắc không đụng tới state backfill. W7f phải giữ rule của Kestra rằng bật lại để yên một backfill đang chạy (`TriggerEventHandler.java:286-289`): "skip missed" chỉ chuyển `next_evaluation_date`, và tiến độ backfill của W7f sẽ nằm trong các cột riêng của nó.
- Tab flow không dành cột nào cho backfill; W7f thêm một cột cạnh Enabled.

## 10. Việc theo sau

- `recoverMissedSchedules` chỉ được tôn trọng khi bật lại. Một trigger bị tắt trong YAML rồi bật lại vẫn chạy lại mọi tick đã lỡ, và egret không có phục hồi khi khởi động để áp dụng nó.
- Log trigger: lỗi schedule chỉ tồn tại dưới dạng `last_error` mới nhất, lỗi bắn Flow-trigger chỉ trong log server (`process.rs:604-683`). Một log theo từng trigger sẽ cho phép drawer hiện lịch sử.
- Một hành động "reset schedule", nếu người dùng muốn ngoài việc bật lại với "skip".
- Hàng loạt "chọn tất cả kết quả khớp" và xuất CSV trên trang toàn cục.
- `stopAfter` được parse và không có gì đọc nó (`models/trigger.rs:51-60`); Kestra tắt trigger qua cùng state runtime mà W7e thêm, nên bảng của W7e là nơi nó sẽ ghi.
- Danh sách toàn cục parse tới 2.000 draft cho mỗi request; một chỉ mục trigger được lưu sẽ bỏ giới hạn đó.
