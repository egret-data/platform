# dbt Studio IDE: nghiên cứu tính năng

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-25-dbt-studio-ide-feature-research.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

> **Trạng thái:** chỉ là nghiên cứu. Chưa có quyết định nào về việc egret có làm hay làm thế nào. Tài liệu ghi lại sản phẩm làm được gì để spec sau này chọn phạm vi.
> **Ngày:** 2026-10-25.

Nguồn (đều được lấy ngày 2026-10-25; docs mô tả sản phẩm dbt platform, không phải dbt Core):

- <https://docs.getdbt.com/docs/platform/studio-ide/develop-in-studio?version=2>
- <https://docs.getdbt.com/docs/platform/studio-ide/ide-user-interface>
- <https://docs.getdbt.com/docs/platform/studio-ide/lint-format>
- <https://docs.getdbt.com/docs/platform/studio-ide/keyboard-shortcuts>
- <https://docs.getdbt.com/docs/platform/studio-ide/develop-studio-ai>
- <https://docs.getdbt.com/docs/platform/studio-ide/git-commit-signing>
- <https://docs.getdbt.com/docs/platform/git/merge-conflicts>
- <https://docs.getdbt.com/docs/platform/about-defer>

Chưa đọc: chi tiết tab Lineage, lệnh Semantic Layer / MetricFlow, "Build and view docs", repo `dbt-jsonschema`, trang biến môi trường, dbt AI FAQs, Manage dbt AI.

## 1. Nó là gì

Một IDE web để sửa, compile, chạy, test và quản lý phiên bản một dự án dbt. Nó compile code dbt thành SQL và thực thi trên warehouse của người dùng, bằng chính credential phát triển của người đó.

Điều kiện trên dbt platform: giấy phép Developer seat, một git repository có quyền ghi, dự án nối với một data platform, một môi trường phát triển có credential người dùng, dbt 1.0 trở lên.

## 2. Bố cục

| Vùng | Nội dung |
|---|---|
| Menu trái | Link repo (tên repo + nhánh), nút site tài liệu, menu Version Control, file explorer, nút chuyển cây file / tìm kiếm |
| Giữa | Editor nhiều tab, minimap, git diff view |
| Console (dưới editor) | Nút: Preview, Compile, Build, Lint, dbt Wizard. Tab: Commands, Problems, Results, Code quality, Compiled code, Lineage, cùng tab xem trước Markdown và CSV |
| Thanh dưới | Command bar, menu Defer, nút Status, phiên bản dbt, trạng thái dự án (số lỗi và cảnh báo), menu tuỳ chọn |
| Lớp phủ | Invocation History Drawer (mở bằng Ctrl+backtick) |

Có nút bật tắt dark mode ở thanh trạng thái.

## 3. File explorer

- Bấm để mở, kéo thả để chuyển giữa các thư mục, double-click để đổi tên.
- Ký hiệu: `•` chưa lưu, `M` đã sửa, `A` mới thêm, `D` đã xoá.
- Menu chuột phải: copy tên, copy đường dẫn tương đối, **copy dưới dạng `ref`**, tải xuống, nhân đôi, đổi tên, xoá.
- Breadcrumb ở đầu editor; chuột phải vào breadcrumb để mở các file cùng thư mục.

## 4. Editor

- Tab: sắp xếp lại được, có menu chuột phải (ví dụ nhân đôi), tab chưa lưu có chấm xanh, đóng nhiều tab chưa lưu thì lưu cùng lúc (batch save), nội dung chưa lưu nằm trong local storage của trình duyệt.
- Tô màu cú pháp SQL.
- Tìm và thay thế trong file (Cmd/Ctrl-F). Nhiều con trỏ. Nhảy tới dòng.
- Minimap.
- Phím tắt block comment chèn comment Jinja `{# #}` trong file SQL.
- Gõ `__` để liệt kê các hàm dbt.
- Tab xem trước Markdown (hiển thị `.md` trực tiếp) và tab xem trước CSV (hiển thị seed CSV dạng bảng trực tiếp).
- Git diff view: bản trước ở bên trái, thay đổi nội tuyến ở bên phải.

### Autocomplete hiểu dbt

| Kích hoạt | Hoàn thành |
|---|---|
| `ref` | tên model |
| `source` | tên source, rồi tên bảng |
| `macro` | tham số của macro |
| `env_var` | tên biến môi trường |
| `-` trong YAML | hoàn thành YAML nội tuyến |

### Kiểm tra YAML

Dùng JSON Schema của dự án mã nguồn mở `dbt-jsonschema` để hoàn thành, kiểm tra cấu trúc và chẩn đoán ngay trong editor. Schema theo đặc tả dbt v2 và áp dụng cho mọi release track. Docs nói rằng nếu một cảnh báo có vẻ sai thì hãy tin kết quả của lệnh dbt.

### CodeGenCodeLens

Link **Generate model** cạnh một bảng trong file YAML của source sẽ tạo một staging model cơ bản.

## 5. Tìm kiếm và command palette

- Tìm kiếm toàn dự án với phân biệt hoa thường và khớp nguyên từ, ô thay thế, thay tất cả, thay từng kết quả, xem trước nội dung sẽ thay.
- Command palette (Ctrl/Cmd+P): Go to File, Show and Run Commands (chỉ lệnh của IDE, không phải lệnh dbt), Search for Text, Go to Symbol in Editor, Go to Line/Column, Go to Symbol in Workspace, tìm trong các file đang mở. Cũng gọi được các thao tác git, compile và preview từ palette.

## 6. Chạy dbt

### Command bar

Nhận lệnh dbt kèm cú pháp node selection. Kết quả đi vào Invocation History Drawer.

### Các nút ở console

| Nút | Hành vi |
|---|---|
| **Preview** | Chạy SQL đang mở (đã lưu, chưa lưu, hoặc đoạn được bôi đen) và hiện các dòng ở Results. Không cần lưu. Mặc định 500 dòng; thêm `limit N` vào SQL để đổi; từ dbt 1.6 có điều khiển "Change row display" nâng mặc định tới 10.000 cho đến khi tải lại trang. Chấp nhận `SELECT TOP #`. Giới hạn phải viết thẳng, không lấy từ macro. |
| **Compile** | Compile SQL đã lưu hoặc chưa lưu vào tab Compiled code. Từ dbt 1.6, model đã lưu được compile cùng ngữ cảnh nên `{{ this }}` và `{{ is_incremental() }}` được giải. |
| **Build** | Dropdown cho `dbt build`, `dbt test` hoặc `dbt run` trên model đang mở, theo phạm vi: chỉ resource này, thêm upstream, thêm downstream, hoặc mọi phụ thuộc. Dùng được cho mọi node chạy được. Lệnh dbt yêu cầu file đã lưu. |
| **Lint** | Chạy linter trên file đang mở (xem mục 9). |
| **dbt Wizard** | Agent AI (xem mục 11). |

Tính năng chạy khác: chỉ build các model đã sửa, tiếp tục build từ lần fail gần nhất, lệnh MetricFlow, build và xem docs cho thay đổi đang phát triển, biến môi trường đặc biệt (ví dụ tên nhánh) để dựng tên schema động, IDE tự làm mới theo thời gian thực khi model, test, seed và operation chạy.

### Invocation History Drawer

- Danh sách các lần gọi trước đó: lệnh, nhánh, trạng thái, thời gian.
- Tóm tắt của lần được chọn; công tắc System Logs hiện toàn bộ stdout và debug log.
- Nút Command Control: huỷ hoặc chạy lại.
- Kết quả theo node, lọc theo trạng thái: Pass, Warn, Error, Skip, Queued; tóm tắt và debug log từng node.
- Model và test lỗi cho tải log chạy về.

### Tab Problems (chỉ dbt v2)

Các vấn đề của dự án sẽ chặn lần chạy, cập nhật khi người dùng sửa. Thanh dưới hiện số lượng.

## 7. Lineage

- DAG của model đang mở. Selector mặc định `2+model+2`; chuyển được sang `+model+` để xem toàn DAG.
- Double-click một node để mở file của nó. Cú pháp node selection mở rộng hoặc thu hẹp đồ thị.
- Không hỗ trợ `--exclude`.
- Lỗi đã biết: IDE có thể đơ khi mở từ hai model trở lên, tab Lineage đang bật và DAG lớn.

## 8. Defer

Chỉ build và test phần đã đổi; các `ref()` upstream không đổi sẽ trỏ tới object staging hoặc production sẵn có, dùng manifest của môi trường đó.

- Cần một môi trường được đánh dấu Production và một deploy job production đã chạy thành công trước đó. Job CI bị loại. Nếu có môi trường Staging thì dùng artifact của nó, và defer sẽ không chạy nếu Staging chưa từng có job.
- Thứ tự giải cho mỗi ref: relation ở development nếu có, nếu không thì staging, nếu không thì production. Mỗi lần gọi chỉ một môi trường. `--favor-state` bỏ qua bước development.
- Bật tắt ở menu Defer trên thanh dưới: Development environment (tắt defer), dbt default behavior (staging nếu có, không thì production, được khuyến nghị), Custom environment.
- `defer-env-id` trong `dbt_project.yml` chọn một môi trường cụ thể.
- Ở dbt Core, hiệu quả tương tự cần `--defer` cùng `--state` trỏ vào một `manifest.json` do người dùng tự quản. IDE tự lấy và cấp manifest. Cách dùng điển hình: nhánh mới, dev schema trống, sửa một model, `dbt build -s state:modified`.
- Cấu hình tự quản có thể dùng `defer_to_target` trong `profiles.yml` (dbt State) để selector `state:*` so từng node với lần chạy gần nhất của chính nó.

## 9. Lint và format

| Loại file | Lint | Format |
|---|---|---|
| `.sql` | SQLFluff (lint và fix) | sqlfmt, hoặc SQLFluff khi được chọn |
| YAML, Markdown, JSON | không | Prettier |
| `.py` | không | Black |

- Tab Code quality có hộp thoại Config để chọn SQLFluff (lint và format) hoặc sqlfmt (chỉ format). Có file `.sqlfluff` ở thư mục gốc dự án thì mặc định chuyển sang SQLFluff.
- **Lint file** hiện gạch lượn kèm chi tiết khi rê chuột và Quick Fix. Nó không chạy lại khi lưu. **Lint and Fix file** tự sửa. Mặc định lint chạy trên mọi file `.sql` đã sửa của nhánh.
- Cấu hình SQLFluff: `.sqlfluff` ở gốc dự án, có thể thêm `.sqlfluff` lồng trong thư mục con, và `.sqlfluffignore` (snapshot phải loại ra bằng tay). Phải khởi động lại IDE sau khi đổi cấu hình. Ví dụ của dbt Labs dùng `templater = dbt`, chậm hơn vì phải compile trước.
- sqlfmt: không cấu hình tuỳ chỉnh. Prettier: thứ tự ưu tiên cấu hình chuẩn; không hỗ trợ `.prettierrc.json5`, `.js` và `.toml`.
- Giới hạn: 60 giây trên backend, quá thì báo "Unable to complete linting this file"; fix khó hơn lint; không dùng SQLFluff từ terminal; model ephemeral không được hỗ trợ ở dbt 1.5 trở xuống; ở v2 stable track lint vẫn dùng engine v1.
- Format và lint dùng được trên nhánh được bảo vệ; commit ở đó bị chặn và IDE yêu cầu commit sang nhánh mới.

## 10. Quản lý phiên bản

- Nút Git actions có nhãn và hành động đổi theo trạng thái: pull, commit and sync, tạo merge/pull request, prune branches. Dropdown: revert changes, refresh git state, create merge/pull request, prune branches, change branch.
- Mục Changes liệt kê mọi thứ từ lần commit gần nhất; bấm một file để xem diff. Chuột phải vào file đã đổi để commit hoặc revert riêng file đó.
- Modal Commit Changes, modal Change Branch, modal Revert Uncommitted Changes.
- Nhánh chính được bảo vệ: vẫn sửa, format, lint và chạy dbt được, nhưng commit bị chặn và người dùng được nhắc commit sang nhánh mới.
- Prune Branches: xoá các nhánh cục bộ đã bị xoá ở remote; không bao giờ xoá nhánh hiện tại; không dùng được với repo do dbt quản lý.
- Link nhánh mở repo (chỉ GitHub hoặc GitLab trên tài khoản multi-tenant).

### Lưu giữ công việc

| Trạng thái | Nơi lưu | Ghi chú |
|---|---|---|
| Chưa lưu | local storage của trình duyệt | mất nếu đổi nhánh hoặc trình duyệt mà chưa lưu (có cảnh báo) |
| Đã lưu, chưa commit | lưu trữ bền của dbt | không nằm trong git; phải commit và sync hoặc revert trước khi Change branch |
| Đã commit | nhà cung cấp git | an toàn |

### Merge conflict

- Xảy ra khi pull hoặc khi commit mà người khác đã đổi cùng các dòng.
- Thanh **Commit and resolve** thay cho Commit. File xung đột có màu đỏ kèm biểu tượng cảnh báo ở file catalog và ở Changes.
- Editor hai khung: khung trái (thay đổi của bạn) chỉ đọc, khung phải (thay đổi đến) sửa được và có các dấu xung đột. Xoá dấu, chọn dòng, Save (vùng tô biến mất), lặp lại cho từng file, rồi Commit and resolve.
- Giới hạn: file xung đột mở từ Changes hoàn toàn chỉ đọc; phải giải quyết hết mọi xung đột trước khi commit.

### Commit có chữ ký

Chỉ gói Enterprise và Enterprise+. Platform tạo một cặp khoá GPG cho mỗi người dùng, ký các commit tạo trong IDE, và người dùng tải khoá công khai lên GitHub hoặc GitLab (không hỗ trợ Azure DevOps). Email dbt phải là email đã xác minh ở nhà cung cấp git. Commit từ công cụ khác không được ký.

## 11. Tính năng AI

Là dịch vụ của platform, tính phí theo mức dùng, bật mặc định, admin điều khiển được.

- **dbt Wizard** (preview, mọi tài khoản): agent dựa trên dbt Information Schema (lineage, test, sức khoẻ model, định nghĩa semantic). Các chế độ: Explore only, Ask for approval (mặc định, duyệt từng file khi nó soạn sửa), Edit files automatically. `@` giới hạn agent vào một model. Nó có thể xin chạy lệnh dbt (cho một lần, cho cả phiên, hoặc từ chối) và các lệnh này được đánh dấu ở tab Commands. Ngoài ra: gỡ lỗi job fail bằng một skill cấu hình sẵn, quy trình di chuyển sang dbt v2 (lặp `dbt compile` và sửa), skill tuỳ chỉnh cho quy ước của nhóm. Giới hạn: chỉ ở Studio IDE, chưa có plan mode, lịch sử chat giữ 90 ngày, tab mới bắt đầu trống, lệnh chạy quá 5 phút bị huỷ.
- **dbt Copilot** (số lượng hạn chế): một cú bấm tạo docs, test, semantic model và metric cho model đang chọn; sửa SQL nội tuyến bằng Cmd/Ctrl+B kèm accept/reject diff. Chưa hỗ trợ đặc tả YAML semantic model mới nhất.

## 12. Phím tắt

| Thao tác | macOS | Windows/Linux |
|---|---|---|
| Danh sách đầy đủ phím tắt editor | Fn-F1 | Fn-F1 |
| Mở file | Cmd-O | Ctrl-O |
| Đóng tab đang mở | Option-W | Alt-W |
| Preview | Cmd-Enter | Ctrl-Enter |
| Compile | Cmd-Shift-Enter | Ctrl-Shift-Enter |
| Danh sách hàm dbt | gõ `__` | gõ `__` |
| Command palette | Cmd-P / Cmd-Shift-P | Ctrl-P / Ctrl-Shift-P |
| Nhiều con trỏ | Option-Click, Shift-Option-Click | Alt-Click, Shift-Alt-Click |
| Invocation History Drawer | Ctrl-backtick | Ctrl-backtick |
| Block comment | Shift-Option-A | Shift-Alt-A |
| Lưu | Cmd/Ctrl-S | |

## 13. Thanh trạng thái và menu tuỳ chọn

Nút Status: kết nối warehouse và LSP (dbt v2) hoặc trạng thái engine server (v1), kèm lối tắt tới cài đặt môi trường và credential; modal Status hiện lỗi server hiện tại, debug log và tuỳ chọn khởi động lại. Hiển thị phiên bản dbt, có thể ghi đè theo cá nhân. Menu tuỳ chọn: xem chi tiết trạng thái, khởi động lại Studio (mất công việc chưa lưu), cài lại dependency, dọn dự án dbt (`dbt clean`), tự sửa cảnh báo deprecated (chuẩn bị cho dbt v2).

## 14. Phiên và giới hạn

- Khởi động: creation start (lần đầu, clone repo, chậm nhất), cold start (phiên mới trong vòng một giờ), hot start (tiếp tục trong vòng một giờ).
- Phiên tắt sau một giờ không hoạt động. Compile, preview và các lần gọi dbt tính là hoạt động; sửa và lưu thì không.
- Repo trên 6 GB cần support duyệt. Credential phải của cá nhân, không phải superuser hay production. Thay đổi đã lưu nhưng chưa commit chặn việc đổi nhánh. Trình chặn quảng cáo có thể làm hỏng các file như `google_adwords.sql`.

## 15. Những gì phụ thuộc dịch vụ của dbt platform

Chính IDE và môi trường phát triển được host; Developer seat và credential theo người dùng; artifact defer từ các job production/staging; dbt Wizard và Copilot; Semantic Layer và MetricFlow; build và xem docs; commit có chữ ký; scheduler và deployment.

## Phụ lục A: các công cụ khác đã có gì (chỉ để so sánh)

**dbt Studio của Obelisk** (bản clone cục bộ của repo Obelisk, đọc ngày 2026-10-25): cây file có New Model / New YAML, Monaco nhiều tab kèm khung SQL đã compile song song, chọn target, Save / Compile / Preview Data / dbt run / Run Tests, một dock gồm Data Preview, Model Lineage DAG một bước và Terminal & Logs. Gọi `dbt` bằng shell cho mỗi thao tác; lineage đọc từ `manifest.json` với phương án dự phòng bằng regex. Khoảng trống so với bảng trên: không autocomplete, không kiểm tra schema YAML, không lint hay format, không stream log trong Studio, không có lịch sử các lần gọi, không defer hay `state:modified`, không UI git, không command bar hay palette.

**Plugin dbt của Kestra** (<https://kestra.io/plugins/plugin-dbt>): `io.kestra.plugin.dbt.cli.DbtCLI` chạy lệnh dbt bất kỳ trong container qua task runner, parse kết quả chạy, nhận `profiles` inline kèm `secret()`, có `storeManifest` / `loadManifest` qua KV store, tuỳ chọn `assets.enableAuto` và `alertWebhook`. `dbt.cloud.TriggerRun` và `CheckStatus` điều khiển dbt Cloud. Các task theo từng lệnh đã bị deprecate. Đây chỉ là task thực thi; Kestra không có IDE cho dbt.

## Phụ lục B: câu hỏi mở cho quyết định (chưa trả lời ở đây)

1. Danh tính dbt chạy dưới đó trên engine có Ranger (người dùng, hay service account của workspace).
2. Git: dùng revision file của nền tảng, hay nối một git repository thật.
3. Engine đích đầu tiên: Trino, StarRocks hay cả hai.
4. Compile/Preview tương tác có dùng chung runner với việc chạy task workflow hay có runner riêng.
5. Những mục nào trong 4 đến 14 thuộc phạm vi bản phát hành đầu.
