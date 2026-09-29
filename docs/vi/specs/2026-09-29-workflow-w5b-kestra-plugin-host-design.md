# W5b — Plugin host Kestra: plugin Kestra thật, chạy trong container của W5a

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-09-29-workflow-w5b-kestra-plugin-host-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, chờ duyệt
**Phụ thuộc:** W5a (task runner, `workflow_containers`, `plugin_configurations`, gia hạn lease), W11 (`RunLog`) — đều đã merge. Base `6acb1d68`.
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 (spike), §6.4 (dòng `kestra.<FQCN>`), §7 quy tắc 1, §9 (dòng W5), §11 câu hỏi mở 3
**Đã quyết với người dùng, 2026-09-29:**
- **Jar được dựng sẵn vào một image từ một lockfile trong repo.** `plugin-host/plugins.lock` ghi mỗi jar bằng toạ độ Maven và sha256; thêm plugin là một PR có review. Không ai thêm jar lúc chạy. Điều này trả lời nửa "jar" của §11 câu hỏi mở 3.
- **`type:` là `kestra.<FQCN>`**, cách viết duy nhất (như §6.4 đã viết). Một FQCN plugin viết trần bị từ chối lúc publish, kèm gợi ý cách viết có tiền tố. Điều này cố ý khác §7 quy tắc 4: một flow Kestra dán vào phải thêm tiền tố cho các dòng `type:` của plugin.
- **`secret()` đến cùng W5b**, phân giải lúc chạy từ `workspace_secrets` của workspace chứa execution, chỉ trên task `kestra.*`.
- **Lockfile đầu tiên có ba plugin:** `plugin-jdbc-postgres`, `plugin-jdbc-trino`, `plugin-jdbc-mysql` (cho StarRocks: Maven Central không có `plugin-jdbc-starrocks`, kiểm 2026-09-29).
- **Governance:** feature `workflow_kestra_plugins`, **tắt mặc định**, cần `workflow_containers`; platform admin thu hẹp catalog bằng glob `allowed-types`; không có entry thì cho phép cả catalog.
- **Mỗi task-run attempt một JVM**, trong một pod của W5a. Không JVM sống lâu, không JVM trong `egret-server`.
- **Timeout huỷ query trên engine** (thêm sau khi so với Kestra): host bị dừng bằng SIGTERM kèm thời gian grace và gọi `kill()` của plugin — với JDBC là `Statement.cancel()` — trước khi pod bị gỡ (§7.1).
- **`pluginDefaults:` bị từ chối lúc publish, trên mọi flow**, như Kestra 2.0 từ chối nó (§4.1). Hôm nay egret bỏ nó đi mà không báo.

**Bàn giao:** một PR (tài liệu + mã), các task ở §9.

## 1. Cái gì đã có, cái gì chưa

| phần | trạng thái trên `origin/main` (`6acb1d68`) |
|---|---|
| spike | đã chứng minh 2026-09-01 (§3 spec chương trình): một `ShimRunContext` ~120 dòng chạy `io.kestra.plugin.jdbc.postgresql.Query` nguyên bản, không Micronaut, trong 1,61–1,68 s. Mã nguồn của shim **không có trong repo** |
| Java trong repo | `crates/egret-workflow/oracle/` — dự án Maven link `io.kestra:core:2.0.0-rc13` (oracle Pebble); `tests/ranger/` |
| kiểu task | `spec::registry::TASK_TYPES` là bảng đóng mười lăm dòng; mọi `type:` khác bị từ chối lúc publish, nên không FQCN plugin nào publish được |
| runner | `TaskRunner` của W5a: Docker cục bộ, một pod Kubernetes mỗi task-run attempt trên production; input file upload bằng exec+tar; output qua dòng `::{json}::`; re-attach theo tên; xoá pod sau khi outcome commit |
| governance | feature `workflow_containers` (tắt mặc định); `plugin_configurations[type].values.allowed-images`; một cổng lúc publish và lúc chạy |
| `secret()` | bị từ chối bởi `runners::functions::REFUSED` (`"the secret backend", "W9"`); không gói nào sở hữu (spec chương trình §9, ghi chú sau W10) |
| secret của workspace | `egret_secrets::WorkspaceSecretStore`: có tên, có kiểu (`Opaque` `{value}`, `kubernetes.io/basic-auth` `{username,password}`, `kubernetes.io/tls`), mã hoá AES-256-GCM, có console |

## 2. Kestra làm gì, và egret khác ở đâu

Kestra (checkout `96893b589`, artifact `2.0.0-rc13`):
- Một task plugin là một lớp Java; worker gọi `run(RunContext)`. Thuộc tính là `Property<T>`, được render **bên trong** plugin qua `RunContext.render`, bằng Pebble và biến của flow, kể cả secret.
- Plugin là fat jar trên classpath của worker; `kestra plugins install` tải chúng từ Maven Central. OSS không có allowlist.
- `secret(key, namespace, subkey, full)` (`pebble/functions/SecretFunction.java`): `namespace` mặc định là của flow và có thể chỉ một namespace khác mà flow được phép đọc; `subkey` đọc một key của secret dạng JSON; `full` trả `{value, metadata}`.
- Mọi secret một run đã dùng được thay bằng `******` trong các dòng log của nó (`RunContextLogger.BaseAppender.replaceSecret`, thay chuỗi con). Output **không** được che.

| Kestra | egret W5b | vì sao |
|---|---|---|
| `type: io.kestra.plugin.jdbc.postgresql.Query` | `type: kestra.io.kestra.plugin.jdbc.postgresql.Query` | người dùng quyết: tiền tố nói "chạy trong JVM host" |
| cài plugin tuỳ ý | ba jar cố định bởi `plugins.lock`, kiểm sha256 lúc dựng image | governance (§11 câu 3) |
| thuộc tính render bên trong plugin | egret render (minijinja) trước khi dispatch; các `render*` của shim trả lại đầu vào | phát hiện 3 của spike; không có Pebble trong pod |
| thuộc tính lạ bị JSON deserializer từ chối lúc save | từ chối lúc publish theo `catalog.json`, sinh từ các jar lúc dựng image | publish chạy không cần JVM |
| backend `secret()` theo namespace | `workspace_secrets` của workspace chứa execution; `namespace` và `full` bị từ chối có tên | một flow không thể chỉ tới workspace khác |
| output và file vào internal storage | output inline (trần 1 MiB của W5a); mọi thứ cần storage thì FAILED có tên | internal storage là W9 |
| Kestra chạy plugin trong JVM worker của nó | một JVM mỗi task-run attempt, trong container của W5a | cách ly: plugin OOM hay `System.exit()` chỉ hạ một pod |
| `pluginDefaults` mức flow bị bỏ ở 2.0 (commit `ca6cd11fa`, có trong mọi tag từ `v2.0.0-rc6`): bị từ chối lúc save, bị bỏ qua khi đọc flow cũ | bị từ chối lúc publish trên mọi flow; egret bỏ đi không báo cho tới W5b | theo artifact đã pin; tính năng đã chuyển sang Policies bản Enterprise |
| khi kill hoặc timeout, worker gọi `kill()` của task (`WorkerJobLifecycle.kill`, `WorkerTaskCallable.java:48`) | cùng `kill()` đó, tới qua SIGTERM và thời gian grace (§7.1) | một query JDBC không được sống lâu hơn task của nó trên engine |

## 3. Host (`plugin-host/`)

Một dự án Maven ở gốc repo, cạnh `crates/egret-workflow/oracle/`:
- `plugins.lock`: mỗi jar một dòng, `groupId:artifactId:version sha256`. Nội dung đầu tiên: `io.kestra.plugin:plugin-jdbc-postgres:2.0.0`, `…plugin-jdbc-trino:2.0.0`, `…plugin-jdbc-mysql:2.0.0`. Cả ba dựng trên `io.kestra:platform:2.0.0-rc12` (theo POM của chúng, kiểm 2026-09-29), nên link được với artifact đã pin `io.kestra:core:2.0.0-rc13`; host không dời pin của §7 quy tắc 1. Các bản plugin sau (hiện là 2.0.4) dựng trên `platform 2.0.2` và chờ một lần nâng có chủ đích cả hai pin.
- `io.kestra.core.runners.ShimRunContext` kế thừa `DefaultRunContext`, trong chính package của Kestra (bẫy 2 và 3 của spike). `render*` trả lại đầu vào; `logger()` ghi ra stdout; `metric()` ghi một dòng DEBUG; `workingDir()` là `LocalWorkingDir` của Kestra trên thư mục làm việc của pod; `storage()`, `namespaceKv()`, `namespace…()`, `secret…()` và mọi collaborator egret không cung cấp đều ném `EgretUnsupported("… needs internal storage (W9)")`.
- `io.kestra.core.runners.EgretPluginHost` (main): đọc `task.json` (`{"type": FQCN, "properties": {…}}`), từ chối `type` không có trong `catalog.json` của chính nó (exit 2), deserialize bằng `JacksonMapper` của Kestra và đường fallback plugin registry (bẫy 1 của spike), chạy `validate`, rồi `run(shim)`, in `Output` thành một dòng `::{"outputs": …}::` và thoát.
- **Mã thoát**, mỗi mã là một lý do FAILED có tên phía egret: `0` thành công; `2` bị từ chối hoặc không hợp lệ (`ConstraintViolationException`, type lạ); `3` plugin ném exception (lớp và thông điệp trong lý do, stack trace trong log ở ERROR); `4` plugin đòi thứ thuộc W9. Mọi mã khác (JVM sập, bị OOM kill) là FAILED kèm mã.
- **Dòng log:** `LEVEL message` trên stdout, parse sang level của W11. Dòng không có tiền tố level (dòng tiếp theo của một stack trace) mang level của dòng trước nó, INFO nếu không có. stderr vẫn là ERROR, như mọi container.
- **Lý do thất bại:** khi thoát với mã khác 0, host in một dòng `::{"egretFailure": {"code": n, "message": "…"}}::` trước khi thoát; handler đặt message (tối đa 4 KiB) vào lý do FAILED. Runner chỉ thu khoá này cho task plugin, nên marker của script không đổi.
- **`catalog.json`** do `EgretPluginHost describe` sinh ra lúc dựng image: với mỗi lớp task trong ba jar, các thuộc tính kèm JSON schema của Kestra (kiểu, bắt buộc, mặc định, deprecated) và các thuộc tính output. Nó mang sha256 của `plugins.lock`. Được commit tại `crates/egret-workflow/src/plugins/catalog.json` và nhúng bằng `include_str!`. Một test Rust đỏ khi hash lockfile trong catalog đã commit khác `plugins.lock`.
- Image: `deploy/docker/plugin-host.Dockerfile`, Java 25 (class-file 69 của artifact), classpath Kestra core, ba jar đã kiểm theo `plugins.lock`, chạy bằng uid không phải root. Tham chiếu image lấy từ config, không bao giờ từ flow.

## 4. Mô hình task và publish

- Task có `type` bắt đầu bằng `kestra.` là task plugin. Phần còn lại phải là một FQCN trong `catalog.json`; các dòng của `TASK_TYPES` egret đã port native (`io.kestra.plugin.core.log.Log`, …) bị loại khỏi catalog lúc sinh và một test khẳng định hai tập rời nhau, nên một kiểu native không bao giờ chạy trong JVM.
- **Bị từ chối lúc publish, có tên:** FQCN `kestra.` không có trong catalog; FQCN trần có trong catalog (gợi ý cách viết có tiền tố); thuộc tính catalog không liệt kê cho lớp đó; thiếu thuộc tính bắt buộc; giá trị thuộc tính cần internal storage — đo theo từng plugin ở Task 1, ít nhất `fetchType: STORE`; `taskRunner` (task plugin chạy trên runner đã cấu hình); bảy thuộc tính gốc egret không đọc (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`), như trên task script.
- Giá trị thuộc tính dạng chữ được kiểm theo kiểu JSON trong catalog khi schema là `string`, `integer`, `number`, `boolean`, `array` hoặc `object` thuần; schema egret không đọc được (`anyOf`, `$ref`, …) và giá trị `{{ }}` được chấp nhận và host kiểm sau khi render (exit 2).
- `retry`, `timeout`, `disabled`, `runIf`, `allowFailure`, `allowWarning` đi qua mô hình `Task` của egret như mọi task. Task plugin không có `timeout:` chạy dưới `workflow.container_default_timeout_s`.

### 4.1 `pluginDefaults`

Kestra 2.0 đã bỏ `pluginDefaults` mức flow (commit `ca6cd11fa`, *"OSS enablers for EE Policies"*, 2026-07-17; có trong mọi tag từ `v2.0.0-rc6`, nên có trong `rc13` đã pin). `FlowService.validate` của nó trả một constraint violation nêu tên `pluginDefaults` (`FlowServiceTest.shouldReturnConstraintWhenFlowUsesRemovedPluginDefaults`); đường đọc lỏng của nó parse flow như vậy và không áp gì (`FlowParsingServiceTest.shouldIgnorePluginDefaultsWhenParsingLeniently`).

`Flow` của egret không có trường này và không có `deny_unknown_fields`, nên hôm nay một flow có `pluginDefaults:` publish được và các giá trị bị bỏ đi không báo. W5b làm cho `spec::validate` từ chối khoá `pluginDefaults` ở mức cao nhất, bất kể giá trị (kể cả danh sách rỗng), trên **mọi** flow, không chỉ flow có task plugin: *"`pluginDefaults` was removed in Kestra 2.0; set the properties on each task"*. Lời từ chối hiện trên bản nháp và chặn publish. Một bản nháp vẫn được chạy thì không áp gì, như đường đọc của Kestra. Task 1 đo câu chữ và phạm vi của lời từ chối trên artifact.

Các khoá mức cao nhất khác mà egret bỏ đi không báo không được rà ở đây (§11).

## 5. Governance

- **Feature `workflow_kestra_plugins`**, tắt mặc định (`default_enabled: false` của W5a), cần `workflow_containers`: một workspace có thể chạy script mà không chạy plugin, không bao giờ chạy plugin mà không chạy script.
- **`allowed-types`:** `plugin_configurations` thêm một entry `{type: kestra, values: {allowed-types: [glob]}}`, so với FQCN (bỏ `kestra.`) bằng `glob_match` của W5a. FQCN không chứa `/`, nên `*` vượt qua các đoạn package: `io.kestra.plugin.jdbc.*` cho phép mọi lớp JDBC. **Không có entry `kestra` thì cho phép mọi type trong catalog** (catalog đã được review như một PR); entry có danh sách rỗng thì không cho phép gì. `allowed-types` trên entry khác, và `allowed-images` trên entry `kestra`, bị từ chối lúc khởi động.
- **Image host** là `workflow.plugin_host.image` và không bị so với `allowed-images`: tác giả không chọn được nó.
- **Một hàm cổng, hai chỗ gọi** (W5a §4): lúc publish (feature, `allowed-types`, runner đã cấu hình) và lúc chạy sau khi render, trước khi tạo bất cứ thứ gì. Từ chối lúc chạy thì không tạo pod, và nhận nuôi pod đã có cho cùng (run, attempt) đúng như W5a.

## 6. `secret()`

- **Cú pháp của Kestra:** `{{ secret('NAME') }}` và `{{ secret('NAME', subkey='password') }}`. `namespace=` và `full=` bị từ chối có tên, lúc publish nếu là chữ, lúc chạy trong trường hợp còn lại.
- **Phân giải** lúc chạy, trong workspace của execution, qua `WorkspaceSecretStore`: không có `subkey` thì là key `value` của secret `Opaque` (secret không có key `value` là lỗi, nêu tên các key của nó); có `subkey` thì là key đó, với mọi kiểu. Secret hoặc key không tồn tại làm task FAILED, nêu tên secret và không bao giờ nêu giá trị.
- **Chỉ trên task `kestra.*`.** Trên task script `secret()` vẫn bị từ chối, kèm lý do: commands và env đã render của task script là trường của pod spec, ai có `get pods` trong namespace task đều đọc được. Ở mọi chỗ khác (inputs, `runIf`, `Log`, trigger) vẫn bị từ chối như hôm nay.
- **Giá trị được phép tồn tại ở đâu:** `task.json` đã render, ghi dưới dạng input file (exec+tar trên Kubernetes, `docker cp` trên Docker), và bộ nhớ của JVM. **Không ở đâu khác:** không trong `flow_versions`, không trong bất cứ thứ gì lưu trên task run (spec đã render, output egret ghi, thông báo lỗi), không trong pod spec hay env của nó.
- **Che:** mọi giá trị một run đã phân giải được thay bằng `******` trong các dòng log trước khi tới `RunLog` (thay chuỗi con như Kestra), trên holder đầu tiên và trên holder re-attach (nó render lại nên biết các giá trị). Output không được che, như Kestra: một query chọn ra mật khẩu sẽ trả nó về (§12).

## 7. Chạy một task plugin

Handler `kestra` (`worker::handlers::kestra`):
1. render thuộc tính của task bằng minijinja, phân giải `secret()`;
2. gọi cổng;
3. dựng một `TaskCommands`: image `workflow.plugin_host.image`, lệnh `java … EgretPluginHost /work/task.json`, input file `task.json`, không output file, timeout;
4. giao cho runner đã cấu hình (W5a) — cùng cách đặt tên pod, hardening, NetworkPolicy, gia hạn lease, re-attach và xoá-sau-commit;
5. ánh xạ mã thoát sang outcome; lấy output từ dòng `::{json}::` (bị trần 1 MiB của W5a chặn; vượt thì task FAILED có tên).

### 7.1 Timeout: huỷ trên engine, rồi mới gỡ

W5a huỷ container với grace 0 (`KubernetesRunner::cancel`, `DockerRunner::cancel` = `docker rm --force`). Cắt kết nối TCP không dừng được query: Postgres chỉ nhận ra khi nó ghi ra socket lần tới, còn Trino và StarRocks giữ query tới khi heartbeat của client hết hạn. Kestra thì gọi `kill()` của task, mà `AbstractJdbcQuery` hiện thực bằng `Statement.cancel()`.

- `TaskRunner::cancel` thêm đối số `grace: Duration`: `0` từ handler script (W5a không đổi) và `workflow.plugin_host.kill_grace_s` (mặc định **15 s**) từ handler plugin.
- **`cancel` chỉ trả về khi container đã dừng** (hoặc đã qua `grace` + 5 s): outcome commit sau `cancel`, rồi `release` gỡ pod với grace 0, nếu không sẽ cắt ngang `kill()` của plugin.
- Lệnh là `exec java …`, để JVM là PID 1 và tự nhận SIGTERM; một tiến trình cha `/bin/sh -c` sẽ không chuyển tiếp nó.
- **Kubernetes:** pod bị xoá với `gracePeriodSeconds = cancel_grace`; kubelet gửi SIGTERM tới `main`, chờ, rồi SIGKILL. **Docker:** `docker stop -t <grace>`, rồi `docker rm --force` của W5a.
- **Host** đăng ký một JVM shutdown hook: khi nhận SIGTERM nó gọi `kill()` của task đang chạy từ luồng của hook (đúng hợp đồng của Kestra: *"may be invoked from a different thread than the one running the job"*), chờ `run` trả về tối đa `cancel_grace − 2 s`, ghi log `killed: <lý do>` rồi thoát.
- Outcome là của egret, không phải của host: task FAILED với dòng `Timeout after …` của W11 bất kể host in gì. Việc kill chạy trước khi outcome commit và việc gỡ pod chạy sau, đúng thứ tự của W5a.
- **Đo ở Task 1, không giả định:** `kill()` của từng plugin trong ba plugin có thật sự kết thúc query trên Postgres (`pg_stat_activity`), Trino (trạng thái trong `system.runtime.queries`) và StarRocks (`SHOW PROCESSLIST`) trong thời gian grace không.
- egret chưa có kill cho người dùng (phát hiện của W10); khi có, nó đi cùng đường này.

`workflow.plugin_host`: `image` (bắt buộc khi `container_runner` khác `none`), `jvm_max_heap` (mặc định `384m`, truyền thành `-Xmx`, dưới giới hạn bộ nhớ mặc định của pod), `kill_grace_s` (mặc định 15). Với `container_runner: none` không task plugin nào chạy được và publish từ chối chúng, như task script.

## 8. Bảo mật

- Host chỉ deserialize lớp gốc do `type` nêu, và chỉ khi nó có trong catalog; classpath là Kestra core và ba jar đã khoá.
- Hardening pod, NetworkPolicy (Postgres của egret và Kubernetes API không với tới được) và trần tài nguyên là của W5a, không đổi.
- Một plugin JDBC tới Trino hoặc StarRocks bằng credential tác giả đưa; policy Ranger của engine áp cho user đó. egret không thêm hay bớt quyền truy cập nào qua đường này.
- Một test sống tìm giá trị của một secret trong database metadata (mọi bảng `workflow.*`) và trong `kubectl get pod -o yaml` sau một run, và kỳ vọng không khớp nào.

## 9. Các task

1. **Đo trên artifact** (Kestra `2.0.0-rc13` với ba plugin): `Query` của mỗi plugin tới Postgres, Trino và StarRocks — hình dạng output theo `fetchType`, `fetchType` mặc định, dòng log và level, việc che secret trong dòng log, các thuộc tính cần storage, lời từ chối flow có `pluginDefaults`, `kill()` có kết thúc query trên từng engine không. `plugin-jdbc-mysql` với StarRocks là đo, không giả định. Fixture `oracle-exec/w5b-plugins.json`.
2. **`plugin-host/`:** dự án Maven, shim, main, `describe`, `plugins.lock`, Dockerfile, `catalog.json`; test Java cho shim (lời gọi W9 ném `EgretUnsupported`, mã thoát); test Rust về hash lockfile.
3. **Mô hình, publish, governance:** kiểu `kestra.` từ catalog; từ chối lúc lưu; từ chối `pluginDefaults` trên mọi flow; `workflow_kestra_plugins`; `allowed-types`; cổng lúc publish.
4. **`secret()`:** phân giải, từ chối `namespace`/`full`, từ chối trên task script kèm lý do, che trong phần ingest của `RunLog`; test giá trị vắng mặt khỏi các hàng đã lưu.
5. **Handler và nối dây:** handler `kestra`; ánh xạ mã thoát; config `workflow.plugin_host`; registry trong `egret-bin`; `cancel_grace` trong cả hai runner và shutdown hook của host; cổng lúc chạy; Helm chart (giá trị image) và bước dựng image trong CI.
6. **End to end trên một cụm kind tạm:** `jdbc-postgres` tới một Postgres trong cụm; secret vắng mặt khỏi database và pod spec; feature tắt bị từ chối, không có pod; tuyên bố độ phủ; quét EN/VI; CI cục bộ đầy đủ.

## 10. Kiểm thử

| test | mutation nó giết |
|---|---|
| một flow `kestra.…postgresql.Query` chạy trong pod trên kind, các hàng nằm trong `outputs`, các dòng nằm trong API của W11 | handler không được đăng ký / output không được parse |
| cùng flow trên Docker tới Postgres, Trino và StarRocks (sống, có cổng như `container_docker_live`) | một plugin thiếu trong image |
| một secret dùng cho `password` không xuất hiện trong hàng `workflow.*` nào và không trong `kubectl get pod -o yaml` (sống, kind) | secret bị render vào spec đã lưu hoặc vào env |
| một dòng log chứa secret được lưu thành `******` | bỏ mất việc che |
| `secret()` trên task script, và `secret(namespace=…)`, bị từ chối có tên | bỏ mất việc từ chối |
| feature tắt → từ chối lúc publish và lúc chạy, không có pod | bỏ qua cổng lúc chạy |
| `allowed-types: []` từ chối mọi plugin; không entry thì cho phép catalog | đảo ngược mặc định |
| FQCN trần bị từ chối kèm gợi ý có tiền tố; FQCN native không bao giờ vào catalog | bỏ mất test tính rời nhau |
| thuộc tính lạ và `fetchType: STORE` bị từ chối lúc publish | bỏ mất kiểm catalog |
| plugin gọi `storage()` thất bại với exit 4 và lý do nêu W9 | lời gọi W9 không bị bẫy |
| hash catalog ≠ hash `plugins.lock` làm test Rust đỏ | đổi lockfile mà không sinh lại catalog |
| một flow có `pluginDefaults:` (kể cả `[]`, kể cả khi không có task plugin) bị từ chối lúc publish, có tên | khoá được nhận rồi bỏ đi |
| một task timeout trên `SELECT pg_sleep(600)` không để lại query nào trong `pg_stat_activity` sau grace (sống, Docker và kind); tương tự trên Trino (sống, Docker) | grace 0 / thiếu shutdown hook / không gọi `kill()` |

**Ratchet:** kỳ vọng **109 → 109**, đo; corpus không có task plugin.

## 11. Ngoài phạm vi

Internal storage, KV, namespace files, output dựa trên storage (W9); **trigger của plugin** (`io.kestra.plugin.jdbc.postgresql.Trigger` và mọi polling trigger); plugin flowable và dynamic (`dynamicWorkerResult`); plugin ngoài ba jar; thêm jar lúc chạy; allowlist theo workspace; `secret()` trên task script, vốn cần một Kubernetes Secret mỗi attempt và thêm RBAC; nâng pin của Kestra; rà các khoá mức cao nhất khác của flow mà egret bỏ đi không từ chối.

## 12. Giới hạn đã biết

- **Output không được che**, như Kestra. Một query trả về secret sẽ lưu nó vào output của task run, ai đọc được execution đều đọc được.
- **`fetchType: FETCH` bị chặn bởi trần output 1 MiB của W5a**; kết quả lớn hơn làm task FAILED có tên. `STORE`, thứ Kestra dùng cho kết quả lớn, chờ W9.
- **Render đồng nhất (identity):** một plugin cố ý hoãn render một thuộc tính theo từng hàng hoặc từng vòng lặp sẽ thấy giá trị egret đã render một lần (spec chương trình §3.2).
- **Chi phí:** khoảng 1,6–2 s khởi động JVM và scan plugin mỗi task-run attempt, đo trong spike, trước khi query chạy.
- **Che là thay chuỗi con**, như Kestra: một secret bị tách qua hai dòng log, hoặc bị plugin mã hoá lại (base64, URL-escape), sẽ không được che.
