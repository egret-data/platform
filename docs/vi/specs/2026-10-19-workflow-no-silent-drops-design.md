# Gói workflow #1 "Không bỏ im lặng": mọi khoá egret đọc rồi bỏ qua đều được gọi tên

> **Bản dịch tham khảo.** Bản tiếng Anh `docs/en/specs/2026-10-19-workflow-no-silent-drops-design.md` là bản chuẩn; khi hai bản khác nhau, bản tiếng Anh thắng. Bản này đi theo bản tiếng Anh từng mục một. Mã, tên khoá, mã lỗi và câu thông báo người dùng thấy được giữ nguyên tiếng Anh.

**Trạng thái:** thiết kế ngày 2026-10-19. Dòng #1 của lộ trình `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. Kế hoạch: `docs/en/plans/2026-10-19-workflow-no-silent-drops.md`. Một PR, nhánh `feat/workflow-no-silent-drops`.

**Các khẳng định lấy từ đâu:** mọi khẳng định về mã egret được kiểm trong worktree `egret-nodrops` tại main `309405b1`. Mã được dẫn `path:line`, tương đối với `crates/egret-workflow/src/` trừ khi ghi gốc khác. Kestra là bản checkout `data-portal/kestra`, ghim tại `96893b589` (2.0.0-SNAPSHOT, 2026-08-17); đường dẫn tương đối với `core/src/main/java/io/kestra/`, viết tắt `k/`, trừ khi ghi gốc khác. Bảng kiểm kê khoá đứng sau §3 được đo trên đúng hai checkout đó (quét cả 351 tệp corpus bằng Ruby Psych; không build, không chạy gì). Khẳng định nào không kiểm tĩnh được thì ghi "đo ở Task N" và nêu task của kế hoạch sẽ đo nó.

**Mục tiêu (người dùng, 2026-10-19):** egret là engine của Kestra OSS, trừ chỗ egret khác đi vì bảo mật. Gói #1 biến mọi khoá egret đọc rồi bỏ qua thành hoặc một lời từ chối có tên, hoặc một cảnh báo có tên, và mỗi cái nêu gói lộ trình sẽ gỡ nó (Quy tắc 1 của lộ trình). `finally:`/`afterExecution:` cấp flow và mọi `finally:` của flowable **không** bị từ chối hay cảnh báo ở đây: chúng thuộc #2, gói kế tiếp.

**Kestra làm gì, và #1 chép theo** (kiểm kê §0):
- `k/core/serializers/YamlParser.java:29-31`: bộ map nghiêm đặt `FAIL_ON_UNKNOWN_PROPERTIES`. Một khoá lạ thành `ConstraintViolationException` với thông báo của Jackson (`Unrecognized field "invalid" (class …), not marked as ignorable`) ở `YamlParser.java:144-157`.
- Đây là Jackson thuần, nên áp cho mọi bean lồng: flow, từng lớp task, trigger, từng kiểu con của input, retry, `Concurrency`, `SLA`, `Check`, `CheckFrequency`, nút `Dag`. Không lớp mô hình nào mang `@JsonIgnoreProperties(ignoreUnknown = true)`.
- Phân tích nghiêm chỉ chạy khi lưu **không phải bản nháp**: `k/core/services/FlowService.java:134-141` (tạo) và `:167-174` (cập nhật) ghi rằng bản nháp được bỏ qua phân tích nghiêm và kiểm ràng buộc, được lưu khi còn sai và sẽ hỏng lúc chạy. Bản nháp của egret là bản nháp của Kestra, còn publish của egret là lần lưu không-nháp của Kestra, nên #1 từ chối ở publish và báo trên bản nháp, đúng như `pluginDefaults:` hiện nay.
- Kestra còn có cảnh báo lúc lưu (`FlowService.warnings`, `FlowService.java:546-596`). egret hiện chỉ port cảnh báo thiếu subflow (`subflow_not_found`, spec nợ §5.1).

## Quyết định

D1–D10 do controller quyết ngày 2026-10-19; hướng đã cố định, lời văn là của spec này. D11–D16 là mặc định của spec này, theo quy tắc "theo Kestra trừ khi egret có lý do khác, và giữ an toàn chặt".

- **D1. Thuộc tính lạ bị từ chối như Kestra, chỉ lúc lưu và publish.** Các vị trí: task lá lõi, khoá cấp cao nhất của flow, flowable, trigger có kiểu (`Schedule`, `Webhook`, `Flow`), input (theo kiểu), output của flow, và các bean lồng ở §3.7. Phát hiện là lỗi `unknown_property`. Nó chỉ phát ra từ điểm vào lúc lưu (D11), không bao giờ từ `validate_for_run`, nên `executor::build_plan` (`executor/plan.rs:1093-1105`) không đổi và phiên bản đã publish trước #1 vẫn chạy. Chỗ serde đã bỏ khoá, phép kiểm đọc nguồn thô. Bảng kiểu → struct đang `#[cfg(test)]` trong `spec/schema/measure.rs:105-176` chuyển sang mã sản phẩm (`spec::keys`, §2.3). Khoá có giá trị `null` cũng bị từ chối: Jackson từ chối một tên lạ bất kể giá trị.
- **D2. Khoá Kestra nhận mà egret không thực thi là cảnh báo, không phải từ chối.** Flow vẫn publish được, và cảnh báo nêu gói sẽ gỡ nó (§3). `onResume` của Pause là cảnh báo (giữ 3 flow `onResume` của corpus publish được). Điều này theo kết luận của bảng kiểm kê cho từng khoá: không mất mục golden-MATCH nào và không mất flow LIVE nào, trừ `invalids/invalid-property`, flow mà chính Kestra từ chối (§6).
- **D3. Mã cảnh báo là `not_honoured`, khác `not_yet_enforced`.** `spec/schema/measure.rs` chỉ xếp một thuộc tính là `warned` khi xuất hiện cảnh báo `not_yet_enforced` mới (`measure.rs:708-712`), và `warned` được phục vụ trong schema của trình soạn (`spec/schema/mod.rs:131-150`). Vì vậy cảnh báo `not_honoured` giữ khoá ở `ignored` (hoặc `honoured`) trong `property-treatment.json`, và trình soạn không bắt đầu gợi ý `logLevel` trên mọi task lõi. Năm dòng `not_yet_enforced` hiện có giữ mã cũ, nên vẫn là `warned`. Một test mới ghim điều này (`a_not_honoured_warning_is_not_served`, Task 2 của kế hoạch); `the_committed_property_treatment_is_what_egret_does_today` (`measure.rs:860`) và `no_served_property_is_one_egret_ignores_or_refuses` của `tests/flow_schema.rs` vẫn xanh vì không dòng nào thành được phục vụ, và các dòng duy nhất thay đổi đi từ `ignored` sang `refused` (§6.3).
- **D4. Mọi lời từ chối hay cảnh báo hiện nêu một gói đã cũ, hoặc không nêu gói nào, được viết lại để nêu gói lộ trình.** Đó là các dòng `not_yet_enforced` ("W8", "a later package"), các lời từ chối của script và plugin ("W5a", "W5b"), và các lời từ chối flowable không nêu gì. Danh sách cũ → mới đầy đủ ở §5. Các lời từ chối vì bảo mật (task runner `Process`, `http()`, `secret()` ngoài plugin task) giữ nguyên chữ (D10).
- **D5. Port các cảnh báo lúc lưu của chính Kestra** ở chỗ egret có cấu trúc tương ứng (§4): `timeout`/`taskCache`/`workerSelector` trên task không-runnable (`FlowService.java:569-583`) và Flow trigger không có `dependsOn` (`:553-566`). Cảnh báo bí mật dạng chữ thường (`:585-593`) không có chỗ gắn trong egret: ở bản ghim, trường `@PluginProperty(secret = true)` duy nhất nằm trong cấu hình http client, và egret chưa có http task cho tới #4, gói sẽ port nó.
- **D6. `lateMaximumDelay` của Schedule thành cảnh báo lúc publish, nêu #11.** Hiện nó được nhận lúc publish và chỉ bị từ chối khi scheduler đồng bộ trigger (`trigger/schedule.rs:1368-1371`, hiện ra ở `last_error` của dòng trigger), nên trigger không bao giờ chạy. Kestra nhận nó lúc lưu và tôn trọng nó. Bảng kiểm kê đo được 0 flow trong corpus. Cảnh báo giữ flow publish được như Kestra, và nói trước publish điều mà lần đồng bộ nói sau publish: trigger này sẽ không chạy. Cùng lập luận và cùng cảnh báo áp cho các lời từ chối khác chỉ hiện sau publish: `when` của Schedule (`schedule.rs:1354`) và `wait`/`returnOutputs`/`responseContentType`/`responseCode` của Webhook (`trigger/webhook.rs:345-360`, 409 ở mọi lần gọi). Bản thân các lời từ chối lúc đồng bộ và lúc gọi không đổi.
- **D7. Con số "345" ở thì hiện tại, đã cũ, đổi thành 351** (§8). Các delta lịch sử ("345 -> 351 at WorkingDirectory Task 8") giữ nguyên. Câu nào ghép 345 với một số đếm khác đo cùng lúc ("16 of 345") là lịch sử và giữ nguyên.
- **D8. #1 không giữ lại `finally:` của flowable.** Phiên bản publish trước #2 chỉ còn nó trong `snapshot.source` (`publish.rs`, `"source": draft.source` của snapshot), vì `plan` của snapshot là `Flow` được serialize lại, vốn đã bỏ nó. §10 dặn #2 dựng lại từ nguồn.
- **D9. Các sửa lộ trình đi trong PR của gói này**, EN và VI, dưới dạng ghi chú có ngày, chỉ thêm (§9).
- **D10. Bảo mật.** Các phép kiểm mới chỉ đọc flow đang được lưu: không hỏi repository, port, workspace hay flow nào khác, nên một phát hiện không thể phụ thuộc vào, hay để lộ, thứ gì ngoài bản nháp của người gọi. Không biểu thức nào được render lúc lưu: giá trị không bao giờ được tính, và thông báo chỉ chứa tên khoá và chuỗi `type` do chính tác giả viết. Cổng script và plugin, lời từ chối runner `Process`, lời từ chối `http()` và giới hạn `secret()` không đổi (lộ trình §3.1).
- **D11. Một điểm vào lúc lưu duy nhất, `spec::validate_source(source, &flow)`.** Nó là `validate(&flow)` cộng `spec::keys::findings(source, &flow, &mut report)`. Bốn chỗ báo cáo về một flow đang lưu hoặc đang publish gọi nó: `publish::publish_gated` (`publish.rs:311`), `routes::draft_dto` (`routes.rs:513`), `routes::validate_flow_source` (`routes.rs:899`) và `run` của `measure.rs` (`measure.rs:227-260`). `validate(&Flow)` và `validate_for_run(&Flow)` giữ chữ ký và phát hiện cũ. Một test quét mã nguồn ghim rằng không có lời gọi sản phẩm nào ngoài `spec/` gọi thẳng `validate(`.
- **D12. Một bảng nêu gói gỡ cho mọi khoá**: `spec::keys::lift_of`. Cổng script, cổng plugin và bộ duyệt mới đều đọc nó, nên hai thông báo không thể nêu hai gói khác nhau cho cùng một khoá. Một test đối chiếu nửa "Kestra nhận khoá này" của bảng với bản export schema rc13 đã commit (`spec/schema/kestra-flow-schema.json`), các khác biệt được liệt kê theo tên. **Bản ghim quyết định** (D9): `onChildFailure` của rc13 trên `Parallel`/`Dag` mới hơn bản ghim (`git log -S onChildFailure` rỗng tại `96893b589`), nên nó là thuộc tính lạ và bị từ chối.
- **D13. Lời từ chối của script và plugin vẫn là từ chối.** Các khoá Task.java của chúng (`logLevel`, `logToFile`, `taskCache`, `version`, `assets`, `workerSelector`, `policyRefs`) bị từ chối ở đó và chỉ bị cảnh báo trên task lõi. Hạ chúng xuống là thay đổi hành vi ngoài phạm vi #1; #13 gỡ chúng trên mọi loại task cùng lúc. Chỉ thông báo của chúng đổi (D4).
- **D14. `tasks:` của `Pause` vẫn bị `executor::pause_gap` từ chối, cùng mã, với thông báo nói rằng đó không phải thuộc tính của Kestra.** `Pause` của Kestra không có trường `tasks` ở bản ghim (`k/plugin/core/flow/Pause.java:158`, `extends Task implements FlowableTask`; các trường là `pauseDuration`, `behavior`, `onPause`, `onResume`, `errors`, `finally`). Lời từ chối là vĩnh viễn (thuộc tính lạ), và vẫn nằm trong `validate_for_run`, vì một phiên bản đã lưu có `tasks:` trên `Pause` sẽ treo (`executor/plan.rs:860-863`). Mã `unsupported_pause_property` được giữ, vì `REFUSED_BY_NAME` ở `measure.rs:77` và các nguyên nhân `PUBLISHED_BUT_REFUSED` đọc nó.
- **D15. Khoá flow do máy chủ quản lý được nhận mà không có phát hiện nào**, như Kestra: `revision` (Kestra tự gán, `FlowService.java:506-508`), `updated`, `tenantId` (`@Hidden`) và `deleted` (`k/core/models/flows/AbstractFlow.java:41,44,65,76`). Kestra bỏ qua giá trị tác giả viết cho từng khoá; `deleted: true` sẽ đo ở Task 4 từ đường tạo của `FlowRepository` trong Kestra, và thành cảnh báo `not_honoured` nếu Kestra có hành động theo nó.
- **D16. Một giá trị mặc định khác Kestra mà không cần viết khoá nào là một dòng lộ trình, không phải cảnh báo.** `allowConcurrent` của Schedule mặc định `false` trong Kestra và khoá trigger cho tới khi execution của nó kết thúc (`scheduler/src/main/java/io/kestra/scheduler/TriggerScheduler.java:345-349`); egret không bao giờ khoá. #1 chỉ cảnh báo khi viết rõ `allowConcurrent: false`, còn giá trị mặc định đi vào #11 (§9). `required` của input (mặc định `true`, `k/core/models/flows/Input.java:82`) cùng dạng; #3 đã liệt kê nó.

## 1. #1 bao những gì

| vị trí | khoá lạ | khoá Kestra mà egret bỏ qua | khoá egret từ chối về sau | kiểm ở đâu |
|---|---|---|---|---|
| flow | `unknown_property` | `not_honoured`: `workerSelector`, `policyRefs`, `draft` | – | nguồn (§3.1) |
| task lá lõi | `unknown_property` | `not_honoured`: 7 khoá Task.java, `retry.warningOnRetry` | – | mô hình: lá giữ spec của nó (§3.2) |
| flowable | `unknown_property` | `not_honoured`: bảng §3.3 | các lời từ chối hiện có của `Pause`, viết lại | nguồn (§3.3) |
| trigger có kiểu | `unknown_property` | `not_honoured`: bảng §3.4 | `not_honoured` lúc publish cho các lời từ chối lúc đồng bộ/lúc gọi (D6) | nguồn (§3.4) |
| input, output | `unknown_property`, kể cả khoá của kiểu input khác | `not_honoured`: bảng §3.5 | – | nguồn (§3.5) |
| bean lồng | `unknown_property` | `not_honoured`: §3.7 | – | nguồn (§3.7) |
| script, plugin task | không đổi (`script_property_unknown`, `plugin_property_unknown`) | từ chối như cũ, viết lại (D13) | – | không đổi |
| trigger `Leaf`, kiểu lá chưa đăng ký hoặc chưa được phục vụ | không kiểm: không có danh sách thuộc tính để đối chiếu | – | – | không đổi (`not_yet_enforced`, `unknown_task_type`, `task_type_not_served`) |

**Không thuộc #1, theo Quy tắc 1:** `finally:`/`afterExecution:` của flow, `finally:` của mọi loại flowable, và `errors:` trên `Loop`, `LoopUntil`, `WorkingDirectory`. #2 gỡ tất cả (§9 thêm `errors:` của `LoopUntil`/`WorkingDirectory` và `finally:` của `WorkingDirectory` vào dòng #2). Bộ duyệt bỏ qua các khoá này theo tên; nó không từ chối, không cảnh báo, và không đi vào trong chúng (task trong một danh sách bị bỏ không có trong mô hình).

## 2. Cơ chế

### 2.1 Phát hiện và mã

`Finding {code, message, path}` và `ValidationReport {errors, warnings}` (`spec/validate.rs:85-103`) không đổi. Lỗi chặn publish; cảnh báo đi vào `warnings` của snapshot và DTO bản nháp.

Mã mới, mỗi mã là của egret (Kestra không có mã nào cho chúng):
- `unknown_property` (lỗi): khoá mà cả Kestra (ở bản ghim) lẫn egret đều không nhận trên đối tượng đó.
- `not_honoured` (cảnh báo): khoá Kestra nhận mà egret không thực thi.
- `flow_trigger_without_depends_on` (cảnh báo): cảnh báo Flow-trigger của Kestra (§4).
- `runnable_property_on_non_runnable` (cảnh báo): cảnh báo `timeout`/`taskCache`/`workerSelector` của Kestra (§4).

### 2.2 Chạy ở đâu

- `spec::validate_source(source: &str, flow: &Flow) -> ValidationReport` (D11) là `validate(flow)` rồi `spec::keys::findings(source, flow, &mut report)`.
- `keys::findings` phân tích `source` thêm một lần, thành `serde_json::Value`, bằng `serde_norway::from_str` (cùng bộ deserialize `spec::parse` dùng, `spec/parse.rs:30-33`). Nguồn vừa phân tích thành `Flow` được, nên với một bản nháp đã lưu việc này không thể hỏng; nếu hỏng, `findings` không thêm gì (dụng cụ đo hỏng thì mở, không bao giờ từ chối sai) và một `debug_assert!` nêu tên nó.
- Nó không bao giờ chạy lúc chạy. `build_plan` gọi `validate_for_run` (`plan.rs:1105`), hàm này không gọi gì trong số trên.

### 2.3 Các bảng khoá (`src/spec/keys.rs`, mới, mã sản phẩm)

- `TASK_MODEL_KEYS`: những gì bộ deserialize của `models::Task` tự đọc: `id type description retry timeout disabled runIf allowFailure allowWarning` (`models/task.rs:764-781`).
- `TASK_JAVA_UNREAD`: các khoá `Task.java` của Kestra mà egret không đọc: `version workerSelector policyRefs logLevel logToFile taskCache assets` (`k/core/models/tasks/Task.java:45,68,74,77,85,97,102`).
- `leaf_keys(canonical_type) -> Option<Vec<&'static str>>`: chuyển từ `measure.rs:105-150` (`leaf_handler_fields`), nội dung không đổi. `None` nghĩa là "không có danh sách thuộc tính" và lá không được kiểm (`egret.*`, `core.http`, `core.slack`, script, plugin và kiểu chưa đăng ký).
- `flowable_keys(kind)`, `trigger_keys(kind)`: chuyển từ `measure.rs:152-176`, dựng từ `spec::schema::wire_fields_of` (`spec/schema/mod.rs:820`, vốn là mã sản phẩm).
- `input_keys(type)`: các khoá chung (`id type description required defaults prefill displayName dependsOn`, `Input.java:62-98`) cộng khoá riêng của kiểu con, theo §3.5.
- `lift_of(site, key) -> Option<Lift>`: các khoá Kestra nhận mà egret bỏ qua, cùng gói gỡ từng khoá (§3). `None` cho khoá egret tôn trọng hoặc Kestra không có.
- `enum Lift { Package(u8), Enterprise, KeptDesign(&'static str) }`. `Package(n)` hiện "roadmap package #n lifts it"; `Enterprise` hiện "it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap §3.3)"; `KeptDesign` hiện lý do của nó và "(roadmap §3.2)".

`measure.rs` nhập các bảng này thay vì giữ bản sao riêng, nên dụng cụ đo và phép kiểm đọc chung một bảng.

### 2.4 Duyệt nguồn song song với mô hình

Task được ghép với map thô của nó theo cấu trúc, không bao giờ bằng cách tìm trong cây thô những đối tượng trông giống task (một thuộc tính plugin có thể chứa `{id, type}`):
- Duyệt bắt đầu từ bốn danh sách gốc của cả hai cây (`root_lists` trong `spec/path.rs`) rồi đi xuống, theo từng loại flowable, vào đúng những danh sách mà `Task::children()` và `Task::error_children()` duyệt (`models/task.rs:122-201`), cùng thứ tự: `tasks`, `then`/`else`, từng case của `Switch` và `defaults`, `task` của từng nút `Dag`, `finally` của `AllowFailure`, `tasks`/`errors`/`onPause` của `Pause`, và `errors` của các loại có nó.
- Một cặp có `id` thô khác `id` của mô hình thì dừng duyệt bên dưới nó, không phát hiện gì (dụng cụ đo hỏng thì mở), và test corpus ở Task 4 của kế hoạch chứng minh không flow corpus nào đi vào nhánh đó.
- Đường dẫn là của `spec::path`, nên một phát hiện nằm đúng chỗ các phát hiện khác của `validate` cho cùng task (`tasks.t.invalid`).

## 3. Các khoá, theo từng vị trí

Mỗi dòng bảng là một khoá và việc #1 làm với nó. "warn #n" là cảnh báo `not_honoured` nêu gói n. Dòng Kestra là ở bản ghim.

### 3.1 Flow (`models/flow.rs:39-134`; Kestra `AbstractFlow.java:28-100`, `Flow.java:49-142`)

| khoá | Kestra | #1 |
|---|---|---|
| các khoá `models::Flow` đọc (`wire_fields_of::<Flow>()`) | – | không gì (không đổi; `pluginDefaults` giữ `removed_plugin_defaults`) |
| `workerSelector` | `AbstractFlow:99`; OSS không định tuyến gì (`WorkerQueueService.java:85-87`) | warn, `Enterprise` |
| `policyRefs` | `Flow:93-98`, "parsed but ignored in the open-source edition" | warn, `Enterprise` |
| `draft` | `AbstractFlow:72` | warn, `KeptDesign("egret's draft and publish split replaces it: a published version runs whatever `draft:` says")`. Không từ chối: `valids/webhook-draft` là golden MATCH |
| `revision`, `updated`, `tenantId`, `deleted` | máy chủ quản lý | không gì (D15) |
| `finally`, `afterExecution` | `Flow:79-88` | không gì (#2, Quy tắc 1) |
| mọi khoá khác | nghiêm | `unknown_property`. Corpus: 0 flow |

### 3.2 Task lá lõi

`spec` của lá được mô hình giữ nguyên văn (`models/task.rs:949-975`), nên phép kiểm này không cần nguồn: nó chạy trên mô hình bên trong `keys::findings`. Với mỗi lá đã đăng ký có danh sách thuộc tính (`leaf_keys`, 30 kiểu cộng bí danh `core.log`; kiểm kê §3b), mỗi khoá của `spec`:
- nằm trong `leaf_keys(type)` hoặc `TASK_MODEL_KEYS`: không gì;
- nằm trong `TASK_JAVA_UNREAD`: warn, theo `lift_of`: `logLevel`, `logToFile`, `taskCache`, `version`, `assets` → #13 (lộ trình §3.3 để spec của #13 quyết `version` và `assets`); `workerSelector`, `policyRefs` → `Enterprise`;
- mọi khoá khác: `unknown_property`, `task '<id>': unknown property '<key>' on <type>: neither Kestra nor egret accepts it`.

`retry.warningOnRetry: true` (task hoặc flow) được đọc (`models/task.rs:697-705`) mà không gì dùng (`executor/plan.rs:687-693`): warn #13 trên mọi loại task và trên flow. Corpus: 4 flow, 2 golden MATCH, nên không từ chối.

Corpus: một flow, `invalids/invalid-property` (`Return` có `invalid:`); Kestra từ chối nó với `Unrecognized field "invalid"`. Không flow hợp lệ nào bị chạm.

### 3.3 Flowable (`models/task.rs`; Kestra `AbstractBranch.java:27-41` cho các loại nhánh `tasks`, `errors`, `finally`)

Khoá được phép là `flowable_keys(kind) ∪ TASK_MODEL_KEYS`. Sau đó:

| loại | khoá | Kestra | #1 |
|---|---|---|---|
| mọi loại | `finally` (trừ `AllowFailure`, vốn đọc nó) | `AbstractBranch:32`, `If:114`, `Switch:115`, `Dag:109`, `Pause:199` | không gì (#2) |
| `Loop`, `LoopUntil`, `WorkingDirectory` | `errors` | `AbstractBranch` | không gì (#2; §9 mở rộng #2 cho `LoopUntil` và `WorkingDirectory`) |
| mọi loại | `logLevel`, `logToFile` | `Task.java:77,85` | warn #13 |
| mọi loại | `version`, `assets` | `Task.java:45,102` | warn #13 |
| mọi loại | `policyRefs` | `Task.java:74` | warn, `Enterprise` |
| mọi loại | `timeout`, `taskCache`, `workerSelector` | `Task.java:59,97,68` | cảnh báo của chính Kestra (§4); `timeout` trên `Pause` được đọc (`plan.rs:1436-1440`) và không nhận gì |
| `Loop` | `outputs`, `fetchType` | `Loop.java:248,261` | warn #6. Corpus: 6 flow, 5 trong baseline, 1 bị loại lúc thu |
| `Subflow` | `revision`, `labels`, `inheritLabels`, `scheduleDate`, `restartBehavior` | `Subflow.java:102,117,138-139,144,155` | warn #12. `labels`/`inheritLabels`: 3 golden MATCH. `scheduleDate`: xem Q2 |
| `Pause` | `onResume` | `Pause.java:193` | warn #9 (D2). Corpus: 3 flow not-standalone |
| `Pause` | `tasks` | không phải thuộc tính Kestra | `unsupported_pause_property` từ `pause_gap`, viết lại (D14); bộ duyệt không báo lần hai |
| `Parallel`, `Dag` | `onChildFailure` | không có ở bản ghim (chỉ rc13) | `unknown_property` (D12) |
| mọi loại | mọi khoá khác | nghiêm | `unknown_property`. Corpus: 0 |

### 3.4 Trigger có kiểu (`models/trigger.rs`; Kestra `AbstractTrigger.java:38-108`)

Các khoá gốc mà bộ deserialize của `models::Trigger` đọc (`trigger.rs:363-400`) được liệt kê thành hằng `TRIGGER_BASE_KEYS`. Khoá được phép là chúng cộng `trigger_keys(kind)`. Trigger `Leaf` không được kiểm (không có danh sách thuộc tính; dòng `not_yet_enforced` của nó giữ nguyên).

| khoá | Kestra | #1 |
|---|---|---|
| `stopAfter` | `AbstractTrigger:90` (đọc bởi `TriggerState.java:249,268`) | warn #11 (được đọc, không gì dùng, `trigger.rs:52-60`) |
| `failOnTriggerError` | `:98` | warn #11 |
| viết `allowConcurrent: false` | `:105` | warn #11: "egret never locks a trigger while its execution runs". Không viết hoặc `true`: không gì (D16) |
| `logToFile`, `logLevel` | `:94,74` | warn #13 |
| `version`, `assets` | `:43,108` | warn #13 |
| `workerSelector`, `policyRefs` | `:65,71` | warn, `Enterprise` |
| `labels` trên `Schedule`/`Webhook` | `:84` | warn #11 (chỉ Flow trigger đọc). Corpus: `valids/webhook-plugin-labels`, golden MATCH |
| `when` trên `Webhook` | `:55` | warn #11 (bị bỏ qua) |
| `when`, `lateMaximumDelay` trên `Schedule` | `:55`, `Schedule.java:230` | warn #11, lấy từ chính lời từ chối của `ScheduleTrigger::resolve`: "this Schedule trigger will not fire" (D6) |
| `wait`, `returnOutputs`, `responseContentType`, `responseCode` trên `Webhook` | `Webhook.java:188,196,208,218` | warn #11, lấy từ `webhook::declared_unsupported`: "every call to this webhook answers 409" (D6) |
| `interval` trên `Schedule` | `Schedule.java:221`, `final … = null`, không tác dụng | không gì: khoá nằm trong `trigger_keys(Schedule)` nếu mô hình đọc nó; nếu không, nó được thêm vào tập được phép kèm trích dẫn này, vì Kestra nhận nó và cũng không làm gì với nó |
| mọi khoá khác | nghiêm | `unknown_property`. Corpus: 0 |

Cảnh báo lúc publish cho một lời từ chối lúc đồng bộ hay lúc gọi gọi đúng vị từ mà lần đồng bộ hay route gọi, nên hai bên không thể vênh nhau. Giai đoạn 2 của `measure.rs` đã xếp các khoá này là `refused` (`measure.rs:722-727`); điều đó không đổi.

### 3.5 Input và output (`models/input.rs:54-96`; Kestra `Input.java:62-98` cộng từng kiểu con)

Một struct của egret phục vụ cả 20 kiểu input; Kestra có một lớp cho mỗi kiểu, nên khoá hợp lệ trên kiểu này là khoá lạ trên kiểu khác.

| kiểu | khoá riêng của Kestra | egret | #1 |
|---|---|---|---|
| STRING, SECRET | `validator` (`StringInput.java:22`, `SecretInput.java:23`) | bị bỏ | warn #3 |
| INT, FLOAT, DURATION | `min`, `max` (`IntInput.java:17,20`, `FloatInput.java:17,20`, `DurationInput.java:19,22`) | bị bỏ | warn #3 |
| DATE, DATETIME, TIME | `after`, `before` (`DateInput`/`DateTimeInput`/`TimeInput.java:19,22`) | bị bỏ | warn #3 |
| SELECT | `values`, `expression`, `allowCustomValue` (`SelectInput.java:32-58`) | bị bỏ | warn #3 (5 flow golden-MATCH viết `values`) |
| SELECT, MULTISELECT | `isRadio`, `autoSelectFirst` (gợi ý giao diện) | bị bỏ | warn #3 (§9 thêm chúng vào dòng #3 như gợi ý của form khởi chạy) |
| MULTISELECT | `values`, `expression`, `allowCustomValue` (`MultiselectInput.java:33-59`); `itemType` | giữ `itemType` | warn #3 cho ba khoá bị bỏ |
| ARRAY | `itemType` (`ArrayInput.java:26`) | giữ | không gì |
| JSON | `jsonSchema` (`JsonInput.java:33`) | bị bỏ | warn #3 (1 golden MATCH) |
| FILE | `allowedFileExtensions` (`FileInput.java:27`) | giữ, có áp | không gì |
| FORM | `inputs` (`FormInput.java:28`) | bị bỏ | warn, `Enterprise` (lộ trình §3.3 liệt kê FORM) |
| REUSABLE_INPUTS | `ref`, `namespace`, `revision` (`ReusableInputsInput.java:35-48`) | bị bỏ | warn, `Enterprise` |
| mọi kiểu | khoá chỉ thuộc kiểu con khác (`validator` trên INT, `itemType` trên STRING) | hiện được nhận | `unknown_property` |
| mọi kiểu | mọi khoá khác | bị bỏ | `unknown_property` |

`input_keys` chỉ giữ `itemType` và `allowedFileExtensions` trên đúng các kiểu Kestra có chúng: `itemType` trên STRING là `unknown_property`. Bảng kiểm kê không quét khoá chéo kiểu theo từng kiểu; Task 5 đo corpus.

Output của flow: `FlowOutput` có đủ sáu trường của Kestra (`models/flow.rs:196-214`, `Output.java`); mọi khoá khác là `unknown_property`.

### 3.6 Script task và plugin task

Hành vi không đổi (D13). Thông báo của chúng nêu gói lộ trình (§5).

### 3.7 Bean lồng

Mỗi bean được đối chiếu với các trường wire của struct egret tương ứng và, chỗ Kestra có khoá mà egret bỏ, với `lift_of`:
- `retry` (task và flow), theo kiểu: các khoá chung (`RetryCommon`, `models/task.rs:697-705`) cộng khoá riêng của kiểu (`constant`: `interval`; `exponential`: `interval`, `maxInterval`, `delayFactor`; `random`: `minInterval`, `maxInterval`; `k/core/models/tasks/retrys/{Constant,Exponential,Random}.java`). `Retry` là enum có tag (`task.rs:723`), nên tập theo kiểu là một hằng viết từ các lớp Kestra, kèm unit test rằng mỗi khoá liệt kê deserialize được vào mô hình (đo ở Task 6).
- `concurrency` (`Concurrency`, `flow.rs:164`), `sla` (theo kiểu), `checks` (`Check`, `flow.rs:299`), `checkFrequency` (`CheckFrequency`, `task.rs:514`), nút `Dag` (`DagNode`, `task.rs:422`), `dependsOn` của input (`DependsOn` trong `input.rs`), và các mục `dependsOn` của Flow trigger.
- `quotas` được bỏ qua: cả khoá đã là cảnh báo Enterprise, và Kestra OSS không đọc nó.
- `Window` vốn đã `deny_unknown_fields` (`trigger.rs:266`) và không đổi.

Bean nào có mô hình không phải struct derive thường thì có một hằng viết tay kèm file:line của Kestra; Task 6 liệt kê là những bean nào.

## 4. Cảnh báo lúc lưu của chính Kestra (D5)

**Thuộc tính runnable trên task không-runnable** (`FlowService.java:569-583`). Kestra duyệt `flow.allTasksWithChilds()` và, với mọi task không phải `RunnableTask`, cảnh báo khi `timeout` khác null (trừ trên `Pause`), `taskCache` và `workerSelector`. Ở bản ghim, các kiểu lõi không-runnable là 11 flowable cộng `Exit`, `SetVariables`, `UnsetVariables` và `Labels` (mỗi cái `extends Task` mà không có `RunnableTask`). Thông báo, nguyên văn:
- `The task '<id>' cannot use the 'timeout' property as it's only relevant for runnable tasks.`
- `The task '<id>' cannot use the 'taskCache' property as it's only relevant for runnable tasks.`
- `The task '<id>' cannot use the 'workerSelector' property as it's only relevant for runnable tasks.`

Mã `runnable_property_on_non_runnable`, đường dẫn `<đường dẫn task>.<khoá>`. Trên các task này không thêm cảnh báo #13/`Enterprise` của §3 nữa: Kestra cũng không tôn trọng khoá ở đó, nên không gói nào gỡ nó. Để cảnh báo đúng, egret không được tôn trọng `timeout` trên bất kỳ kiểu nào trong số này: `WorkingDirectory` và bốn kiểu cấp executor sẽ đo ở Task 7 (`worker/timeout.rs`); kiểu nào egret có tôn trọng thì bị loại khỏi cảnh báo, kèm một dòng doc.

**Flow trigger không có `dependsOn`** (`FlowService.java:553-566`): khi `dependsOn` rỗng và `when` vắng hoặc là chuỗi `"true"`. Chữ của Kestra là `This flow will be triggered for EVERY execution of EVERY flow on your instance. We recommend adding the dependsOn property to the Flow trigger '<id>'.` Listener của egret bị giới hạn trong workspace (`flow_trigger/process.rs:575-585`, `WHERE l.workspace_id = $1`), nên chữ của egret thay "on your instance" bằng "in this workspace" và giữ nguyên phần còn lại. Mã `flow_trigger_without_depends_on`, đường dẫn `triggers.<id>`.

## 5. Các thông báo người dùng thấy được thay đổi

Mọi đoạn chữ thay đổi, cũ → mới. `<…>` là chỗ điền như hiện nay. Chữ thông báo giữ tiếng Anh như trong mã.

| ở đâu | cũ | mới |
|---|---|---|
| `not_yet_enforced`, mọi dòng (`validate.rs:2108-2114`) | ``"`<name>` is declared in this flow but is not yet enforced by any package in this branch; enforcement lands in <package>"`` | ``"`<name>` is declared in this flow but egret does not enforce it yet; <lift>"`` |
| dòng `concurrency` | `W8` | `roadmap package #8 lifts it` |
| dòng `sla` (MAX_DURATION) | `W8` | `roadmap package #16 lifts it` |
| dòng `checks` | `W8` | `roadmap package #16 lifts it` |
| dòng `quotas` | `W8` | `it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap §3.3)` |
| dòng `triggers` (trigger `Leaf`) | `a later package; egret fires only Schedule, Webhook and Flow triggers` | `egret fires only Schedule, Webhook and Flow triggers; roadmap package #11 lifts ScheduleOnDates and http.Trigger, #18 lifts plugin triggers, and McpToolTrigger is not engine scope (roadmap §3.3)` |
| `unsupported_pause_property`, `errors`/`behavior`/`onPause` (`validate.rs:1667-1675`) | `… Remove '<p>', or use a plain Pause (optionally bounded by pauseDuration or timeout)` | như cũ, thêm `. Roadmap package #9 lifts it` |
| `unsupported_pause_property`, `tasks` (D14) | cùng câu như trên | ``task '<id>' is a pause with 'tasks', which Kestra's Pause does not have: it is an unknown property, refused on egret as Kestra refuses it. Put the tasks after the Pause instead`` |
| `unsupported_allow_failure_retry` (`validate.rs:1641-1648`) | `… put `retry:` on the child task that should be retried` | như cũ, thêm `. Roadmap package #9 lifts it` |
| `unsupported_loop_until_property` (`validate.rs:1528-1535`) | `… '<field>' must be a literal value` | như cũ, thêm `. Roadmap package #9 lifts it` (Q3) |
| script `script_property_not_implemented` (`scripts/mod.rs:507-517`) | `property '<k>' is accepted by Kestra on <fqcn> but is not implemented by egret (W5a); remove it` | `property '<k>' is accepted by Kestra on <fqcn> but is not implemented by egret (<lift>); remove it`, `<lift>` lấy từ `lift_of`: `logLevel`/`logToFile`/`taskCache`/`version`/`assets` → `roadmap package #13`; `runner`/`targetOS` và bảy khoá python → `roadmap package #17`; `workerSelector`/`policyRefs` → `Kestra Enterprise only, ignored by Kestra OSS too, roadmap §3.3` |
| thuộc tính runner của script (`scripts/mod.rs:706-717,798,832-835,863-866`) | `(W5a)` | `(roadmap package #17)` |
| mẫu `outputFiles` của script (`worker/commands.rs:3108-3124`) | `(W5a)` | `(roadmap package #17)` (§9 thêm chúng vào #17) |
| plugin `plugin_property_not_implemented` (`plugins/mod.rs:370-378`) | `… not implemented by egret (W5b); remove it` | `… not implemented by egret (<lift>); remove it`, như với script |
| plugin `taskRunner` (`plugins/mod.rs:358-366`) | `… (workflow.container_runner); remove it` | như cũ, thêm ` Roadmap package #18 lifts it` |
| Display của `ScheduleError::UnsupportedLateMaximumDelay`, `UnsupportedWhenCondition` (`schedule.rs:1292-1299`) | `core.schedule `lateMaximumDelay` is not supported: <d>` / `core.schedule `when` conditions are not supported: <w>` | như cũ, thêm `; roadmap package #11 lifts it` |
| Webhook `Wait`/`ReturnOutputs`/`ResponseContentType`/`ResponseCode` (`webhook.rs:377-393`) | `… which egret does not implement yet -- …` | như cũ, thêm ` (roadmap package #11)` |

Cố ý không đổi: lời từ chối runner `Process`, `http()`, `secret()` (D10); `StoreBody`/`Multipart` của Webhook (`webhook.rs:367-376`), chỉ xảy ra trên triển khai không có internal storage, tức là cấu hình chứ không phải khoảng cách tương đương; `unsupported_foreach` và `unsupported_flow_task` (Kestra 2.0 đã bỏ cả hai task); `removed_plugin_defaults` (đúng với Kestra 2.0).

Các test ghim chữ cũ được cập nhật trong cùng task: `the_not_yet_enforced_list_is_pinned_in_both_directions` (`validate.rs:2906`), `tests/plugins_gate.rs:266` (ghim một tiền tố không đổi), `tests/publish.rs:770` (chuỗi con không đổi), và mọi unit test của `scripts` khẳng định "(W5a)". Lệnh quét của kế hoạch là `grep -rn "(W5a)\|(W5b)\|lands in\|a later package" src tests`.

## 6. Corpus và ratchet

### 6.1 Oracle nhìn vào đâu

- Thống kê (`LIVE`, `UNKNOWN_TYPE_REFUSED`, `VALIDATE_REFUSED` trong `oracle-exec/driver.rs`) lấy từ `publish_corpus`, vốn chạy `publish::publish_gated` thật (`driver.rs:334-460`). Một lời từ chối lúc lưu làm nó dịch chuyển.
- Phán quyết `refused` của ratchet lấy từ `executor::build_plan` (`driver.rs:1168-1174`, `Ran::Refused`), vốn chỉ chạy `validate_for_run`. **Lời từ chối chỉ-lúc-lưu không làm ratchet dịch chuyển.** Câu "banks `invalids/invalid-property`, baseline 73 → 72" của bảng kiểm kê giả định một lời từ chối mà ratchet nhìn thấy. Với D1 thì ratchet không thấy (Q1).

### 6.2 Delta dự kiến (phải đo, không được giả định)

Trên main: 301 golden, 73 lệch, 73 trong baseline, 0 hỏng; `LIVE` 295, `VALIDATE_REFUSED` 25, `UNKNOWN_TYPE_REFUSED` 21; `PUBLISHED_BUT_REFUSED` 6.

| con số | dự kiến sau #1 | vì sao |
|---|---|---|
| ratchet (golden / lệch / baseline / hỏng) | 301 / 73 / 73 / 0 | cảnh báo không bao giờ làm nó dịch; lỗi mới duy nhất là chỉ-lúc-lưu |
| `LIVE` | 294 | `invalids/invalid-property` bị từ chối lúc publish (`unknown_property`) |
| `VALIDATE_REFUSED` | 26 | cùng flow đó |
| `UNKNOWN_TYPE_REFUSED` | 21 | không kiểu nào đổi |
| `PUBLISHED_BUT_REFUSED` | 6 | `invalid-property` là 422 của Kestra, không phải flow Kestra đã publish |
| `STILL_OVER_ACCEPTED` / `UNATTRIBUTED` (`tests/executor.rs:6650,7093`) | `invalids/invalid-property` ở lại, nguyên nhân viết lại: "refused at publish since #1 (`unknown_property`); the executor oracle compares `build_plan`, which plans it" | ratchet vẫn thấy nó bị nhận quá |

Nếu controller trả lời Q1 bằng (b), Task 8b tuỳ chọn của kế hoạch đổi dụng cụ đo và ratchet đọc 72 / 72 trong baseline, `invalid-property` chuyển từ `STILL_OVER_ACCEPTED` sang một mục `REFUSAL_GROUPS` có thông báo `unknown_property`, và các con số thống kê ở trên vẫn đúng.

Mọi dịch chuyển thống kê khác là một phát hiện: một khoá input chéo kiểu (§3.5) hoặc một khoá bean lồng (§3.7) mà bảng kiểm kê chưa quét. Kế hoạch dừng lại ở đó và báo cáo, thay vì chỉnh hằng số.

### 6.3 `property-treatment.json`

Sinh lại bằng `EGRET_WRITE_PROPERTY_TREATMENT=1` khi `measure.rs` gọi `validate_source`. Diff dự kiến: `io.kestra.plugin.core.flow.Parallel.onChildFailure` và `io.kestra.plugin.core.flow.Dag.onChildFailure` đi từ `ignored` sang `refused` (D12). Không dòng nào được thành `honoured` hay `warned` (D3). Dòng nào khác dịch chuyển thì phải giải thích trong báo cáo task, nếu không thì thay đổi sai.

## 7. Bảo mật (D10)

- `keys::findings` nhận `(&str, &Flow)` và trả phát hiện. Nó không có tham số repository, port, pool hay workspace, nên không thể tra cứu gì; chính kiểu của hàm là phép thử chữ ký.
- Nó không bao giờ gọi bộ render. Cảnh báo `when` hay `lateMaximumDelay` đến từ `ScheduleTrigger::resolve` và `webhook::declared_unsupported`, vốn đọc mô hình và không render (`schedule.rs:1343-1372`; `webhook.rs:345-360`). Task 5 đọc lại cả hai trước khi dựa vào điều đó.
- Thông báo mang tên khoá, id task và chuỗi `type` của chính tác giả, không bao giờ mang giá trị. Tên khoá được in dạng `'<key>'` như cổng script; khoá là YAML do tác giả viết trong bản nháp này.
- Lỗi là chỉ-lúc-lưu, nên không thể dùng một lời từ chối để chặn phiên bản đã publish của người khác.

## 8. "345" đã cũ (D7)

Corpus có 351 flow (`driver.rs:1042` khẳng định). Các con số ở thì hiện tại cần sửa:
- assert và thông báo: `trigger/depends_on.rs:1188`, `trigger/flow.rs:1705`;
- doc comment: `oracle-exec/driver.rs:857`; `models/task.rs:50`; `executor/plan.rs:408,767,1227,1434,1567`; `executor/mod.rs:1271,2696,2781,4352,5334,5550,9545`; `executor/flowable/branch.rs:189`, `pause.rs:192`, `dag.rs:74`; `worker/spawn.rs:800`, `render.rs:89`, `replay.rs:148`, `subflow.rs:279`; `trigger/mod.rs:28`, `flow.rs:34`, `schedule.rs:266`; `worker/handlers/python.rs:47`.
- Các câu nêu một số đếm khác đo trên 345 (`executor/mod.rs:2764` "52 `Loop` tasks across the 345", `worker/spawn.rs:1333` "43 of its 345 files", `worker/subflow.rs:990-1036` và `webhook_routes.rs:150,153` "16 of 345", "248 of 345") là lịch sử hoặc cần đếm lại. Kế hoạch tính lại số đếm ở chỗ có test tính nó, còn lại bỏ chữ số ("across the corpus"), như `driver.rs:859-869` đã làm cho `LIVE`.
- Delta lịch sử giữ nguyên: `executor/mod.rs:2990,4387`, `driver.rs:1037`, và chú thích ở `tests/executor.rs:1474`.

## 9. Sửa lộ trình (D9)

Trong `docs/{en,vi}/plans/2026-10-18-workflow-kestra-parity-roadmap.md`, dưới dạng ghi chú có ngày, không xoá gì:
1. **#9:** `tasks:` của `Pause` không phải thuộc tính của Kestra (`Pause.java:158`); nó bị bỏ khỏi phạm vi #9 và bị từ chối vĩnh viễn (D14). #9 thêm `checkFrequency`/`failOnMaxReached` không phải literal của `LoopUntil` (`unsupported_loop_until_property`; Q3).
2. **#11:** thêm `allowConcurrent` của Schedule (mặc định `false` trong Kestra, `TriggerScheduler.java:345-349`; egret không bao giờ khoá; D16), `failOnTriggerError`, và `when` của Webhook.
3. **#2:** thêm `errors:` của `LoopUntil`, `errors:` của `WorkingDirectory` và `finally:` của `WorkingDirectory` (đều từ `AbstractBranch`; kiểm kê §4).
4. **#3:** thêm `isRadio`/`autoSelectFirst` của SELECT/MULTISELECT như gợi ý của form khởi chạy, và `expression` của SELECT/MULTISELECT.
5. **#17:** thêm mẫu `**` và `[...]` của `outputFiles` trong script (`worker/commands.rs:3108-3124`).
6. **Phụ lục B §4:** "Schedule.lateMaximumDelay | supported" là sai: nó bị từ chối khi scheduler đồng bộ trigger (`trigger/schedule.rs:1368-1371`), và từ #1 thì bị cảnh báo lúc publish. Một ghi chú sửa có ngày dưới bảng.
7. **Bản ghim quyết định:** bản export schema (rc13, dựng ngày 2026-09-01) mới hơn bản ghim mã nguồn (2026-08-17); chỗ hai bên khác nhau (`onChildFailure`), bản ghim quyết. Một ghi chú ở §4 "Keeping this current".
8. **Mục 15 của Phụ lục B §8** ("unknown and unsupported properties … ignored on core tasks") được #1 đóng lại: một ghi chú có ngày.

## 10. Cho các gói sau

- **#2:** phiên bản publish trước #2 chỉ giữ `finally:` của flowable trong `snapshot.source`; `plan` của nó không có (`publish.rs`, `"plan"` là `Flow` serialize lại). #2 phải dựng lại plan từ `snapshot.source` khi chạy một phiên bản như vậy, nếu không phiên bản đó sẽ tiếp tục lặng lẽ bỏ qua `finally:`. Điều tương tự đúng cho `errors:` của `Loop`/`LoopUntil`/`WorkingDirectory` và `finally:` của `WorkingDirectory`. `finally:`/`afterExecution:` cấp flow đã có trong plan (`models/flow.rs:81,92`).
- **Mọi gói gỡ:** xoá dòng `lift_of` của khoá và test `not_honoured` của nó trong cùng thay đổi khiến egret thực thi khoá đó; khi ấy test đối chiếu (D12) đòi khoá phải có trong mô hình của egret.
- **#13:** quyết `version` và `assets` (lộ trình §3.3); nếu chúng chỉ có ở Enterprise, dòng `lift_of` của chúng thành `Enterprise`.
- **#4:** port cảnh báo bí mật dạng chữ thường của Kestra (`FlowService.java:585-593`) cùng với các http task.

## 11. Đóng gói và test

Một PR. TDD theo từng task; unit test trong `spec::keys` cho mọi loại dòng bảng; test có DB chỉ cho đường publish và route. Ratchet của oracle giữ xanh sau mọi task, và các hằng thống kê chỉ đổi trong task làm chúng dịch chuyển, kèm một dòng doc có ngày. Không migration, không khoá cấu hình, không thay đổi frontend (cảnh báo đã hiển thị theo thông báo).

## 12. Câu hỏi cho controller

- **Q1. D1 (chỉ-lúc-lưu) và câu "`invalids/invalid-property` được bank, baseline 73 → 72" của D2 không thể cùng đúng.** Phán quyết refused của ratchet là của `build_plan` (`driver.rs:1168-1174`), vốn không bao giờ chạy phát hiện chỉ-lúc-lưu. Các lựa chọn: (a) giữ D1; ratchet giữ 73/73, `invalid-property` ở lại `STILL_OVER_ACCEPTED` với nguyên nhân mới, và chỉ thống kê dịch chuyển (LIVE 295 → 294). (b) Giữ D1 và đổi dụng cụ đo: một mục golden `refused` cũng khớp khi `publish_corpus` xếp nó `DraftOnly` với thông báo thuộc một nhóm `REFUSAL_GROUPS` có tên; cách này bank nó (73 → 72) và là Task 8b của kế hoạch. (c) Phát `unknown_property` trong `validate_for_run`; cách này bank nó nhưng chặn các phiên bản publish trước #1 chạy, trái với D1. Kế hoạch dựng (a) và mang (b) như task tuỳ chọn. Lưu ý câu "không mất flow LIVE nào" của bảng kiểm kê cũng cần đọc là "không mất flow LIVE nào mà Kestra coi là hợp lệ": `invalid-property` hiện LIVE và rời LIVE trong mọi lựa chọn.
  - **Đã trả lời 2026-10-19 (người dùng): (b).** Task 8b nằm trong phạm vi, không còn tuỳ chọn: oracle tính một lời từ chối lúc lưu thuộc một nhóm có tên trong `REFUSAL_GROUPS` là khớp với entry `refused` của golden, và ratchet dự kiến ở 72 mismatching / 72 baselined. Phải đo.
- **Q2. `scheduleDate` của Subflow: bảng kiểm kê nói từ chối, D2 nói khoá Kestra-hợp-lệ là cảnh báo.** egret sẽ khởi chạy flow con ngay thay vì vào ngày đó. Corpus: 0 flow. Spec này cảnh báo (D2). Từ chối nó sẽ là một `unsupported_subflow_property` chỉ-lúc-lưu nêu #12.
- **Q3. `unsupported_loop_until_property` không có dòng lộ trình nào.** Spec này nêu #9 và thêm nó vào dòng #9 (§9 mục 1). Lựa chọn khác là một dòng mới.
- **Q4. D5 đòi cảnh báo của Kestra nguyên văn; cảnh báo Flow-trigger nói "on your instance".** Listener của egret giới hạn trong workspace, nên chữ nguyên văn sẽ sai. Spec này đổi cụm đó thành "in this workspace" (§4).
- **Q5. D1 liệt kê flowable, trigger và input là các vị trí lồng.** Tính nghiêm của Kestra còn phủ retry, concurrency, SLA, checks, `checkFrequency`, nút `Dag` và `dependsOn`, và một lỗi gõ ở bất kỳ chỗ nào trong số đó hiện bị bỏ im lặng. Spec này đưa chúng vào (§3.7, Task 6 của kế hoạch). Bỏ Task 6 thì các khoá đó vẫn im lặng và mục tiêu không đạt với chúng.
