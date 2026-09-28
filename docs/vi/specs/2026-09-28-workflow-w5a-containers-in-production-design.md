# W5a — Task kịch bản (script) và task runner: container chạy trên production

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-09-28-workflow-w5a-containers-in-production-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, chờ duyệt
**Phụ thuộc:** W4 (container handler, `TaskRunner`/`DockerRunner`), W10 (runtime), W11 (`RunLog`, logs API) — đều đã merge. Base `8f35f85a`.
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3.1, §6.4, §9 (dòng W5), §11 câu hỏi mở 3
**Đã quyết với người dùng, 2026-09-28:**
- **W5 được chia làm hai.** W5a (spec này) làm cho task container chạy được trên production. W5b (image plugin host kiểu Kestra, lớp Java shim, cơ chế dispatch `kestra.<FQCN>` và allowlist jar plugin) đến sau và dựa trên W5a. Các plugin dựa vào storage và secret chờ W9.
- **Theo đúng hình dạng của Kestra.** Task kịch bản (script) của Kestra mang một thuộc tính `taskRunner:`; W5a port các kiểu task đó và thuộc tính đó thay vì phát triển thêm kiểu `egret.container` riêng của egret.
- **Governance:** một feature theo từng workspace `workflow_containers`, **tắt mặc định**, cộng một allowlist image toàn cục do platform admin đặt. Cả hai đều chặt hơn Kestra OSS, vốn không có cái nào trong hai thứ đó; đây là chủ đích và được ghi rõ.
- **Container chạy ở đâu:** một pod Kubernetes cho mỗi task trên production; Docker vẫn dùng cho phát triển cục bộ và test.
- **Thời gian:** worker gia hạn lease của nó trong khi handler đang chạy (heartbeat kiểu Kestra), và một task container không có `timeout:` nhận một mặc định theo cấu hình (1 giờ).
- **Cách ly:** hardening Kubernetes tiêu chuẩn, egress mở, trừ tới Postgres metadata của egret và Kubernetes API.

**Bàn giao:** một PR (tài liệu + mã), các task ở §8.

## 1. Cái gì đã có, cái gì chưa

| phần | trạng thái trên `origin/main` (`8f35f85a`) |
|---|---|
| container handler | `egret.container` và `egret.script.python` (W4). Hợp đồng: `image`, `commands`, `beforeCommands`, `env`, `interpreter` (mặc định `/bin/sh -c`), `outputFiles`, `failFast`; output qua các dòng `::{json}::`; file output nội tuyến (inline), giới hạn 1 MiB |
| runner | trait `TaskRunner` (`worker/commands.rs:286`); **một** bản cài đặt duy nhất, `DockerRunner`, điều khiển Docker CLI (`create`, `cp`, `start --attach`, `cp`, `rm`), container đặt tên `egret-run-<run_id>-<claimed_at_micros>` |
| registry trên production | `Handlers::with_core_tasks()` = `Log`, `Return`, `Fail`, `Sleep`. **Container và python chưa được đăng ký**; chính comment của `egret-bin` nói 9 trên 13 kiểu có thể publish bị treo trên production |
| governance | chưa có. Các khoá feature (`egret-workspace/src/features.rs`) có ngữ nghĩa **"không có hàng = bật"**, nên không khoá nào hiện có có thể tắt mặc định |
| thời gian | chưa có ngân sách thời gian mặc định cho container/python; reaper thu hồi một run sau `working_timeout_s` (mặc định **3600 s**), nên một task container chạy quá một giờ bị chạy lại trong khi lần chạy đầu vẫn đang diễn ra (mối nguy đã ghi lại của W4) |
| tiền lệ Kubernetes | kernel launcher của notebook (`egret-bin/src/kernel_launcher_k8s.rs`, crate `kube`) với `namespace-kernels.yaml`, `rbac-kernels.yaml` (`pods` get/list/watch/create/delete, `pods/log` get), `networkpolicy-kernels.yaml` |

## 2. Kestra làm gì, và egret khác ở đâu

Kestra (checkout `96893b589`, image `kestra/kestra:v2.0.0-rc13`):
- **Task kịch bản (script)** (`io.kestra.plugin.scripts.shell.Commands`, `shell.Script`, `python.Commands`, `python.Script`) kế thừa `AbstractExecScript`. Chúng tách *cái gì* chạy (task) khỏi *chạy ở đâu* (thuộc tính `taskRunner:`). Runner mặc định là **Docker** (`AbstractExecScript.java:52-58`, `pullPolicy: IF_NOT_PRESENT`).
- **`TaskRunner`** (`core/.../tasks/runners/TaskRunner.java`) chạy một `TaskCommands` (image, interpreter, before-commands, commands, env, thư mục làm việc/output, timeout); `{{ workingDir }}` và biến môi trường `WORKING_DIR` trỏ tới thư mục làm việc.
- **Runner:** `Process` (một subprocess cục bộ, nằm trong core); `io.kestra.plugin.scripts.runner.docker.Docker` (label `kestra.io/*`, đóng/mở gói tar vào/ra, `resume: true` gắn lại (re-attach) theo label sau khi worker restart, `delete: true`); **Kubernetes làm task runner chỉ có ở bản Enterprise** (`io.kestra.plugin.ee.kubernetes.runner.Kubernetes`). Bản OSS đi kèm `io.kestra.plugin.kubernetes.core.PodCreate`, chuyển file bằng một **init container và một sidecar files**, upload và download bằng exec + tar với marker ready/ended, và gắn lại theo label.
- **Chống chạy đôi:** worker heartbeat (`kestra.server.liveness`, 3 s, timeout 45 s); một task chỉ được nộp lại khi worker của nó được xác nhận đã chết, và runner container/pod gắn lại (re-attach) thay vì tạo mới.
- **Governance:** bản OSS **không có** allowlist plugin, image hay runner nào. `kestra.plugins.configurations` (`{type, values}` cho mỗi kiểu plugin, đọc bởi `RunContext.pluginConfiguration`) là núm chỉnh duy nhất; `volume-enabled` của Docker mặc định `false` qua đó.

| Kestra | egret W5a | lý do |
|---|---|---|
| `taskRunner` mặc định là Docker | mặc định là `container_runner` đã cấu hình (`kubernetes` trên production, `docker` cục bộ) | egret chạy trên Kubernetes; một Docker socket trong server là root trên node |
| runner `Process` | **bị từ chối lúc lưu** | nó sẽ chạy mã tuỳ ý bên trong `egret-server` |
| runner Kubernetes là Enterprise | egret cài đặt nó dưới cùng một FQCN | một flow viết cho Kestra EE chạy không đổi |
| không có allowlist image, không có công tắc theo từng tenant | feature `workflow_containers` (mặc định tắt) + `allowed-images` trong `plugin_configurations` | quyết định của người dùng: chặt hơn Kestra OSS |
| container chỉ nhận diện qua label | pod còn có thêm một **tên xác định (deterministic)** | huỷ và gắn lại từ bất kỳ worker nào, lý do runner Docker của egret vốn đã đặt tên container |
| heartbeat + coordinator liveness | worker gia hạn lease của nó (`claimed_at`) mỗi `working_timeout / 3`; reaper vốn đã chỉ thu hồi lease đã hết hạn | cùng một bảo đảm trên mô hình claim sẵn có của egret |
| file output → URI internal-storage | nội dung nội tuyến, có giới hạn (như W4) | internal storage là W9 |

## 3. Các kiểu task và thuộc tính

Được port, dùng đúng FQCN của Kestra: `io.kestra.plugin.scripts.shell.Commands`, `io.kestra.plugin.scripts.shell.Script`, `io.kestra.plugin.scripts.python.Commands`, `io.kestra.plugin.scripts.python.Script`. `egret.container` và `egret.script.python` vẫn giữ vai trò **alias** của `shell.Commands` và `python.Script` (các flow hiện có tiếp tục chạy được).

Các thuộc tính được cài đặt: `containerImage`, `commands` (Commands) / `script` (Script), `beforeCommands`, `interpreter`, `env`, `inputFiles` (chỉ nội dung nội tuyến), `outputFiles`, `failFast`, `warningOnStdErr`, `timeout`, `taskRunner`.

- **Image mặc định** theo từng kiểu task là image của Kestra, **được đo ở Task 1** (`oracle-exec/w5a-scripts.json`): `ubuntu` cho `shell.*` và `python:3.13-slim` cho `python.*`. Chúng phải qua được allowlist như mọi image khác.
- **Bị từ chối lúc lưu**, kèm lỗi có tên rõ ràng, không bao giờ bị bỏ qua âm thầm: `namespaceFiles`, `outputDirectory`, `inputFiles` được đưa dưới dạng storage URI, `targetOS`, và bất kỳ thuộc tính nào khác mà Task 1 phát hiện image chấp nhận nhưng W5a không cài đặt (đã đo: `runner` đã deprecated, và trên `python.*` là `dependencies`, `dependencyCacheEnabled`, `packageManager`, `pythonVersion`, `uvAutoInstallEnabled`, `uvInstallerSha256`, `uvInstallerVersion`; danh sách theo từng kiểu là `egret_expected.task_types` trong fixture). Bản thân Kestra từ chối thuộc tính lạ lúc lưu (`Unrecognized field …`, đã đo).
- **Thuộc tính task cơ sở trên các kiểu script.** Model `Task` của egret đọc `id`, `type`, `description`, `retry`, `timeout`, `disabled`, `runIf`, `allowFailure` và `allowWarning`. Bảy thuộc tính còn lại mà Kestra nhận trên mọi task (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`) bị egret bỏ đi mà không có tác dụng gì, nên trên bốn kiểu script (và các alias của chúng) chúng **bị từ chối lúc lưu** như các thuộc tính ở trên. `version` cũng bị từ chối trên runner Docker.
- **Bắt buộc lúc lưu:** `commands` (Commands) hoặc `script` (Script), đã đo. `taskRunner` thì không: schema đánh dấu nó `$required`, nhưng một flow không có nó (hoặc có `taskRunner: null`) vẫn lưu được và chạy trên runner mặc định.
- `warningOnStdErr` được chấp nhận và **không có tác dụng gì**, đúng như trên artifact. *Sửa theo phép đo của Task 1:* với `true`, `false` hay không đặt, một script ghi ra stderr rồi thoát 0 kết thúc **SUCCESS**, và dòng stderr được log ở mức ERROR (như W11 đã đo); field của Kestra ghi `"Not used anymore, will be removed soon"`. Bản nháp trước nói nó đặt WARNING; điều đó chưa từng được đo.

`taskRunner:` chấp nhận:
- `io.kestra.plugin.scripts.runner.docker.Docker`: `pullPolicy`, `cpu` (chỉ `cpu.cpus`), `memory` (chỉ `memory.memory`; `kernelMemory`, `memoryReservation`, `memorySwap`, `memorySwappiness` và `oomKillDisable` bị từ chối), `delete` (mặc định `true`). Các thuộc tính Docker khác của Kestra (volumes, privileged, networkMode, …, và `image` bị ẩn khỏi schema nhưng Kestra vẫn nhận) bị từ chối lúc lưu.
- `io.kestra.plugin.ee.kubernetes.runner.Kubernetes`: `pullPolicy`, `resources` (`request`/`limit` cho cpu và memory, giới hạn trần bởi config), `delete` (mặc định `true`). `resources` nhận đúng `request.cpu`, `request.memory`, `limit.cpu` và `limit.memory`; mọi khoá khác, ở bất kỳ cấp nào, bị từ chối. Namespace, service account, node selector và tương tự **không** cho tác giả flow đặt; chúng đến từ config.
- `io.kestra.plugin.core.runner.Process`: **bị từ chối lúc lưu**.
- Vắng mặt: dùng runner mặc định đã cấu hình.

## 4. Governance

- **Feature `workflow_containers`**, yêu cầu `workflows`. Đây là feature đầu tiên **tắt mặc định**: catalog feature có thêm `default_enabled: bool`, và với khoá này, không có hàng nghĩa là *tắt*. Các khoá hiện có vẫn giữ `true`, nên hành vi của chúng không đổi.
- **Allowlist image:** `workflow.plugin_configurations`, một danh sách `{type, values}` đánh khoá theo FQCN của runner (đúng hình dạng `kestra.plugins.configurations` của Kestra). `values.allowed-images` là một danh sách glob pattern được so khớp với image reference đúng như viết, sau khi mặc định tag là `latest` (đã đo ở Task 1: `containerImage: python` khiến Docker pull `python:latest`). **Không có mục nào, hoặc danh sách rỗng, thì không image nào được phép.** Chuẩn hoá trước khi so khớp, chính xác là:
  - không thêm và không bỏ gì: không `docker.io/`, không `library/` (`docker.io/library/python:3.13-slim` không khớp `python:*`);
  - một reference dạng digest (`name@sha256:…`) được so khớp đúng như viết, không thêm tag (đã đo: artifact chạy `python@sha256:…` đúng như viết);
  - ngoài ra, tag là phần sau một dấu `:` nằm sau dấu `/` cuối cùng, nên port trong `host:5000/img` không phải tag; reference không có tag được thêm `:latest` (`host:5000/img` → `host:5000/img:latest`);
  - các image mặc định được so khớp theo cùng cách: `ubuntu` → `ubuntu:latest`, `python:3.13-slim` giữ nguyên.
  - *Ngữ nghĩa glob (thêm ở vòng sửa 1 của Task 2):* `*` khớp một dãy ký tự bất kỳ **trừ `/`**, `?` khớp một ký tự khác `/`, và không có `**`. Vì vậy một dấu sao không thể vươn tới một registry host khác: `python:3*` không khớp `python:3000/x/img` (host `python`, cổng 3000), và `registry.company.com/*` không khớp `registry.company.com.evil.io/x`. Muốn cho phép đường dẫn sâu hơn dưới một registry thì viết một `*` cho mỗi cấp (`registry.company.com/*/*`).
- **Một hàm gate, hai điểm gọi** (bài học của W13b: bản nháp vẫn lưu được dù có lỗi validate và vẫn chạy được):
  - **lúc publish**, một image dạng literal nằm ngoài allowlist, một feature đang tắt hoặc một runner/thuộc tính bị từ chối là một lỗi 400 có tên;
  - **lúc chạy**, sau khi render (image có thể là một biểu thức `{{ }}`), handler gọi đúng hàm đó. Khi bị từ chối, task chuyển FAILED kèm lý do, và **không pod hay container nào được tạo ra**.
- Tắt feature chặn các run mới ngay tại gate lúc chạy; các run đã ở trong pod thì chạy cho xong.

## 5. Runner Kubernetes

**Pod, một cho mỗi lần thử của task run:**
- tên `egret-run-<run_id>-<attempt>`; label `egret.io/workspace-id`, `egret.io/execution-id`, `egret.io/taskrun-id`, `egret.io/attempt`;
- namespace `workflow.kubernetes.namespace` (pod security `restricted`);
- `automountServiceAccountToken: false`, `runAsNonRoot: true`, `runAsUser: workflow.kubernetes.run_as_user`, capability bị drop, `seccompProfile: RuntimeDefault`;
- resources: mặc định theo config, request của tác giả bị giới hạn trần bởi `max_resources`;
- `imagePullPolicy` lấy từ `pullPolicy`; `imagePullSecrets` lấy từ config;
- một `emptyDir` dùng chung làm thư mục làm việc; biến môi trường `WORKING_DIR` trỏ vào đó.

**Container**, theo đúng `PodCreate` của Kestra:
1. init container `files-in` (helper image, chỉ khi có input files): chờ egret upload file bằng exec + tar, xác nhận bằng một marker, rồi thoát;
2. `main`: image của flow, chạy `interpreter [beforeCommands + commands]` trong thư mục làm việc;
3. sidecar `files-out` (helper image): giữ pod sống sau khi `main` thoát để egret có thể download `outputFiles` bằng exec + tar.

**Lifecycle:** tạo; upload input files; theo dõi log của `main` (`pods/log?follow`) đưa vào `ingest_line` → `RunLog` (W11); chờ `main` kết thúc rồi đọc exit code của nó từ pod status; download output files; **xoá pod** (trừ khi `delete: false`), kể cả khi thất bại.

**Timeout và kill:** xoá pod với grace 0; khi timeout, task chuyển FAILED kèm dòng `Timeout after …` của W11.

**Gắn lại (re-attach):** lúc claim, tra pod theo tên. Đang chạy: gắn lại và theo dõi log từ `sinceTime` = dòng cuối cùng đã lưu của lần thử run đó (W11), nên chỉ vài dòng lặp lại. Đã kết thúc: download output rồi xoá. Không tồn tại: tạo mới.

**Pod mồ côi:** reaper, trên tick sẵn có của nó, xoá các pod mang label của egret mà run của chúng đã ở trạng thái kết thúc (egret chết giữa lúc download output và lúc xoá).

**RBAC:** một Role trong namespace của task: `pods` get/list/watch/create/delete, `pods/log` get, `pods/exec` create; gắn (bound) vào ServiceAccount của egret.

**NetworkPolicy:** pod của task không được tới Postgres metadata của egret hay Kubernetes API server; mọi egress khác đều mở.

Runner Docker cũng có cùng hành vi gắn lại theo tên và xoá khi xong; nó vốn đã đặt tên container.

## 6. Gia hạn lease và timeout mặc định

- Trong khi handler đang chạy, worker gia hạn lease của nó mỗi `working_timeout_s / 3`: `UPDATE … SET claimed_at = now() WHERE id = $run AND claimed_at = $held` (một fence). Reaper, không đổi, chỉ thu hồi lease cũ hơn `working_timeout_s`, nên run của một worker còn sống không bao giờ bị thu hồi, và run của một worker đã chết bị thu hồi trong vòng `working_timeout_s`.
- Nếu một lần gia hạn thấy fence đã dịch chuyển (một worker khác đang giữ run), worker ngừng theo dõi task và không ghi outcome nào; pod được để lại cho chủ mới gắn lại.
- Áp dụng cho mọi handler, không riêng container, nên một `egret.sql` chạy lâu cũng an toàn.
- Một task container hay script không có `timeout:` chạy dưới `workflow.container_default_timeout` (mặc định **1 giờ**).

## 7. Cấu hình và triển khai

`egret_config::WorkflowConfig` (env `EGRET_WORKFLOW__…`):
- `container_runner`: `none` | `docker` | `kubernetes`, mặc định `none`. Với `none`, registry trên production không đổi; nếu không, bốn kiểu script và hai alias được đăng ký.
- `container_default_timeout_s` (3600).
- `plugin_configurations`: `[{type, values}]`.
- `kubernetes`: `namespace`, `default_resources`, `max_resources`, `image_pull_secrets`, `run_as_user`, `helper_image`.

Helm chart: `workflow.containerRunner`; một namespace cho task, Role, RoleBinding và NetworkPolicy, mô phỏng theo `namespace-kernels.yaml`, `rbac-kernels.yaml` và `networkpolicy-kernels.yaml`. Helper image được build từ một Dockerfile trong repo.

## 8. Các task

1. **Đo trên artifact**: image mặc định của bốn kiểu script task; thuộc tính được chấp nhận và bị từ chối; hình dạng kết quả `outputFiles`; `inputFiles` nội tuyến; `warningOnStdErr`; `failFast`; một image không có tag. Fixture `oracle-exec/w5a-scripts.json`.
2. **Model và governance:** bốn kiểu cộng alias; `taskRunner`; các trường hợp bị từ chối lúc lưu; `default_enabled` trong catalog feature và khoá `workflow_containers`; `plugin_configurations`; hàm gate duy nhất, được gọi lúc publish.
3. **Gia hạn lease và timeout mặc định** trong worker.
4. **`KubernetesRunner`:** bộ dựng pod-spec thuần; lifecycle với truyền file bằng exec+tar, theo dõi log, gắn lại, xoá; quét pod mồ côi; helper image.
5. **Nối dây:** config; registry trong `egret-bin`; runner Docker với vai trò `taskRunner` Docker; gate lúc chạy; Helm chart.
6. **Đầu-cuối trên kind**; tuyên bố độ phủ; rà EN/VI; CI cục bộ đầy đủ.

## 9. Kiểm thử

| test | mutation nó giết |
|---|---|
| một flow `shell.Commands` chạy trong một pod trên kind, các dòng log đọc được qua API của W11, `outputFiles` được trả về, pod bị xoá | pod không bị xoá / log không được theo dõi |
| một worker bị kill giữa chừng: lần claim kế tiếp gắn lại đúng pod đó; chỉ từng tồn tại một pod, chỉ một outcome được ghi | tạo mới thay vì gắn lại |
| một task lâu hơn `working_timeout` không bị reap trong khi worker của nó còn sống; bị reap sau khi worker chết | không gia hạn lease |
| timeout xoá pod, FAILED kèm `Timeout after …` | timeout mà không xoá pod |
| feature tắt → bị từ chối lúc publish và lúc chạy, không pod nào được tạo | gate lúc chạy bị bỏ qua |
| image ngoài allowlist bị từ chối; allowlist rỗng từ chối tất cả | bỏ kiểm tra allowlist |
| runner `Process` bị từ chối lúc lưu | Process được chấp nhận |
| từ một pod của task, Postgres của egret không thể tới được | thiếu NetworkPolicy |
| pod spec: các trường `restricted`, không có SA token, resources bị giới hạn trần | một trường bị bỏ khỏi builder |

Test Kubernetes sống chỉ chạy khi có sẵn một cluster kind, gate bằng một biến môi trường, giống cách `container_docker_live` gate theo Docker. **Ratchet:** kỳ vọng **109 → 109**, đo chứ không khẳng định; corpus không có task script nào.

## 10. Ngoài phạm vi

W5b (image plugin host, shim, `kestra.<FQCN>`, allowlist jar); internal storage, `namespaceFiles`, URI `inputFiles`/`outputFiles` dựa vào storage, secret (W9); allowlist image theo từng workspace; `volumes`, `privileged`, service account tuỳ chỉnh; `targetOS` của Windows; các ngôn ngữ script khác của Kestra (node, R, …), có thể thêm sau theo cùng hình dạng.

## 11. Giới hạn đã biết

- Một pod sống lâu hơn egret hoàn toàn (egret bị gỡ cài đặt) không được egret dọn dẹp.
- Gắn lại có thể lặp lại vài dòng log quanh thời điểm thu hồi.
- `pods/exec` trong namespace của task cho phép egret exec vào pod của task; namespace đó không chứa gì khác.
