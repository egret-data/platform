# dbt Studio và quy trình phát triển dựa trên git, nhiều môi trường: ghi chú thiết kế

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-25-dbt-studio-and-git-environments-design-notes.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

> **Trạng thái:** biên bản thảo luận. **Chưa có gì ở đây được quyết định.** Tài liệu giữ lại các quan điểm do chủ sản phẩm nêu, các đề xuất đưa ra để đáp lại, và các câu hỏi còn mở, để spec sau này bắt đầu từ đây thay vì từ trí nhớ.
> **Ngày:** 2026-10-25.
> **Đầu vào:** `docs/en/specs/2026-10-25-dbt-studio-ide-feature-research.md` (dbt platform Studio IDE làm được gì); việc đọc repo Obelisk và dagron; trang plugin dbt của Kestra; schema workflow của egret (`migrations/0132_workflow_flows.sql`).

## 1. Vì sao có tài liệu này

Mong muốn ban đầu là một dbt Studio trong egret. Cuộc thảo luận mở rộng thành một quy trình phát triển chung cho project dbt và workflow: sửa trên UI, giữ mọi thứ trong git, đưa qua các môi trường. Tài liệu này ghi lại suy nghĩ đó theo thứ tự, đánh dấu ai nói gì, và liệt kê những gì phải kiểm tra trước khi viết spec.

Nhãn dùng bên dưới: **[Đã nêu]** là quan điểm chủ sản phẩm đưa ra. **[Đề xuất]** là gợi ý đưa ra trong thảo luận và chưa được chấp nhận. **[Mở]** cần quyết định. **[Chưa kiểm chứng]** là nhận định chưa đối chiếu với code.

## 2. Quan điểm do chủ sản phẩm nêu

- **[Đã nêu]** Git là nguồn sự thật chính, như GitOps. Việc đổi môi trường nên đơn giản.
- **[Đã nêu]** Một workspace là một dự án hoặc một team. Các môi trường nằm **trong** một workspace, không phải các workspace riêng (ba workspace cho mỗi dự án sẽ nhân ba thành viên, vai trò và cấu hình secret).
- **[Đã nêu]** Có **môi trường cá nhân** cho mỗi user. Nó chạy bằng quyền của chính user đó, và cho user đổi môi trường, ví dụ ghi vào schema cá nhân. Chỉ prod mới ghi vào schema prod.
- **[Đã nêu]** **prod và uat chạy bằng service account.**
- **[Đã nêu]** Phát triển trên **dữ liệu thật** là điểm mấu chốt: phát triển data trên dữ liệu giả che mất lỗi. Đọc dữ liệu thật; ghi vào schema cá nhân.
- **[Đã nêu]** Quy trình phát triển dbt nên là khuôn mẫu cho việc phát triển workflow: mọi thứ phát triển trên UI và tích hợp với git.

## 3. Những gì đã có (đã kiểm tra)

- Flow gồm hai bảng: `workflow.flows` (bản nháp, duy nhất theo `(workspace_id, namespace, flow_id)`, `state` thuộc `draft|live|disabled`) và `workflow.flow_versions` (snapshot bất biến, tham chiếu đã resolve lúc publish). Schema workflow không có khái niệm môi trường.
- Egret cấp token engine theo từng user từ machine IdP của chính nó (claims `sub`, `preferred_username`, `groups`, `aud: engine`; TTL 15 phút). Quyết định của Ranger và clearance phân loại đi theo user đó.
- Namespace files (W9b), KV (W9c) và file của execution (W9a) có phạm vi theo workspace và namespace. Công tắc runtime của trigger nằm ở `workflow.disabled_triggers`.
- Egret chưa có code dbt. Chỗ duy nhất nhắc tới là guide view-governance (một lần chạy dbt trên 8 thread qua catalog có cột phân loại sẽ bị từ chối giữ chỗ).
- **[Chưa kiểm chứng]** Ghi chú dự án nói mọi execution chạy bản draft đã lưu, còn comment trong migration 0132 mô tả ý định ngược lại. Phải xác nhận trong code trước khi thiết kế phụ thuộc vào việc cái nào đúng.

## 4. Mô hình đề xuất

### 4.1 Môi trường trong một workspace [Đề xuất]

| | personal (mỗi user một cái) | uat | prod |
|---|---|---|---|
| Chạy bằng | chính user (token engine được ủy quyền) | service account | service account |
| Git ref | nhánh của user (bản làm việc) | `main` | một tag hoặc SHA, có phê duyệt |
| Ghi vào | schema cá nhân | schema uat | schema prod |
| Trigger theo lịch | tắt | tuỳ chọn | bật |
| Đọc | dữ liệu thật, trong quyền của user | dữ liệu thật | dữ liệu thật |

Một flow trở thành một định nghĩa (định danh `(workspace, namespace, flow_id)`) nhân với môi trường, trỏ tới một version đã triển khai. Hai người sửa cùng một flow không còn đụng nhau: mỗi người có một bản triển khai cá nhân. Mỗi execution mang `environment_id`.

### 4.2 Mô hình git [Đề xuất]

- Trunk-based. `main` là trạng thái chuẩn; nhánh tính năng ngắn hạn; thay đổi vào `main` qua pull request. Không đề xuất GitFlow với các nhánh `develop` và `release` sống lâu.
- Môi trường không phải là nhánh. Mỗi môi trường có một **binding**: `{repository, đường dẫn, ref, tự đồng bộ hay không}`, trong đó `ref` là nhánh, tag hoặc SHA (kiểu Argo CD).
- **Deploy** dời ref. **Rollback** dời ngược lại. Bộ reconcile so cây file ở ref với trạng thái workspace và publish các `flow_versions` mới, mỗi version gắn SHA của commit.
- Chỉ **định nghĩa** nằm trong git: flow, namespace files, project dbt. Trạng thái vận hành thì không: execution, giá trị KV, secret, giá trị biến, công tắc runtime của trigger.
- Kiểm tra PR dùng lại các phần có sẵn: validate lúc publish (hàm thuần), một `plan` ngữ nghĩa cho thấy việc merge đổi gì ở từng môi trường (ý tưởng từ `dagron plan`), và `dbt compile` cho dbt.

### 4.3 Workflow thay đổi thế nào [Đề xuất]

- Ở môi trường do git quản lý (uat, prod), API và UI **từ chối** ghi trực tiếp; chỉ bộ reconcile mới ghi. Từ chối chứ không bỏ qua, khớp hướng no-silent-drops.
- Ở personal, "bản nháp" là bản làm việc của nhánh user. Save ghi bản làm việc, Commit đẩy nhánh, Run chạy bản đã lưu như hôm nay. Ngữ nghĩa hiện tại giữ nguyên ở personal và chỉ siết lại ở nơi git quản lý.
- Nút Publish trở thành Commit / Open PR.
- Sửa khẩn cấp đi qua nhánh hotfix và pull request, hoặc một công tắc break-glass vẫn tạo ra commit.

### 4.4 Dữ liệu và danh tính [Đề xuất, một phần Chưa kiểm chứng]

- Personal đọc dữ liệu thật theo quyền và clearance của user. Điều này khả thi vì token engine theo từng user.
- **Masking có thể khác nhau giữa personal và prod**, vì prod chạy với clearance của service account. uat chạy bằng cùng service account với prod là nơi sự khác biệt đó lộ ra.
- Việc chuyển hướng ghi vào schema cá nhân phải được bảo đảm **theo cấu trúc**. Viết lại SQL thì mong manh (lịch sử dự án ghi nhiều lần lexer engine không khớp). Đề xuất: (a) flow và dbt đặt tên đích qua biến môi trường (dbt có `target` và `generate_schema_name`; workflow sẽ dùng thứ như `{{ env.schema }}`), và (b) một lớp bảo vệ phía sau trong đó run ở personal xác thực bằng một **principal dẫn xuất** có policy, do egret biên dịch với vai trò Ranger Admin, đọc như user và chỉ ghi vào schema cá nhân, nên lệnh ghi prod viết cứng sẽ bị engine từ chối.
- **[Chưa kiểm chứng]** Mô hình quyền SP4a có biểu diễn được "chỉ ghi vào một schema" cho một principal dẫn xuất hay không.
- Secret: ưu tiên danh tính (token engine của user) hơn secret. Secret prod không được đọc được từ personal.
- Các task có tác dụng phụ ra ngoài (webhook, HTTP, chat, email) cần chính sách theo môi trường: cho phép, từ chối hoặc chạy khô.

## 5. Chi phí của chiều môi trường

Mọi thứ đang khoá theo `(workspace, namespace)` đều phải có thêm chiều môi trường, nếu không personal có thể ghi đè prod:

- KV, namespace files và file của execution: quy tắc "flow chỉ ghi namespace của mình" trở thành "ghi namespace của mình **trong môi trường của mình**". Một lựa chọn: lớp phủ copy-on-write, personal đọc xuyên xuống uat/prod và ghi vào lớp riêng.
- Trạng thái trigger: cửa sổ depends-on, công tắc bật tắt runtime, concurrency, backfill.
- Secret và biến theo từng môi trường.
- Subflow resolve trong cùng môi trường; danh sách execution, metric và log lọc theo môi trường.
- Cấu hình plugin và allowlist container: quyết định có thay đổi theo môi trường không.

## 6. Điều kiện tiên quyết và rủi ro

- **Service account làm principal** là phụ thuộc cứng của uat và prod. Ghi chú dự án xếp nó sau nợ data-governance; hôm nay nó chưa là principal đầy đủ **[Chưa kiểm chứng: đọc lại trạng thái hiện tại]**.
- **Promote là một đường leo thang đặc quyền.** Người không có quyền ghi prod vẫn có thể deploy một flow ghi prod qua service account. Quyền sửa flow, quyền đổi binding của môi trường và quyền phê duyệt deploy phải tách riêng, và việc review pull request là một phần của kiểm soát.
- Credential git cần chỗ cất trong secret store của workspace, và đổi binding là một quyền nhạy cảm.
- Lệch hai chiều: người ta có thể commit ngoài UI. Cần webhook hoặc polling và quy tắc bên nào thắng.
- Flow đã tạo trên UI cần một lần export vào repository.
- Trước khi deploy, kiểm tra môi trường đích có đủ connection và bảng mà flow tham chiếu.

## 7. Các phương án đã xem xét

| Phương án | Kết luận hiện tại |
|---|---|
| Mỗi môi trường một workspace (dev, staging, prod) | Chủ sản phẩm loại: nhân số workspace, thành viên, vai trò và secret. |
| CSDL là nguồn sự thật, git là đích đồng bộ (kiểu Kestra) | Đơn giản hơn và giữ Draft/Publish, nhưng git chỉ là bản sao lưu và hỗ trợ review, không phải điểm kiểm soát. Không chọn. |
| GitFlow (`develop`, `release` sống lâu) | Không đề xuất: thêm merge và cherry-pick mà không có lợi. |
| Môi trường chỉ là tiền tố namespace hoặc biến `{{ env.X }}` | Không cô lập được state và danh tính; không đề xuất. |

## 8. Câu hỏi về module cho dbt [Đề xuất]

- Một module dbt riêng (khu điều hướng riêng, như Data API và Workflow) cho Studio và kho project, cộng với một task `dbt` mỏng trong engine workflow cho các lần chạy production theo lịch.
- Một lớp git dùng chung bên dưới cả hai (kết nối repository, branch, commit, diff, revert, conflict), làm việc trên một cây file và không biết gì về dbt hay flow.
- Module dbt nên dùng lại runner container của engine workflow (W5a), không dùng execution của workflow, để Compile và Preview không mỗi lần tạo một execution. **[Chưa kiểm chứng]** Runner W5a có gọi được ngoài một execution hay không. Nếu không, hoặc tách một primitive dùng chung, hoặc giấu một execution cho mỗi lần gọi tương tác.
- Kestra có `io.kestra.plugin.dbt.cli.DbtCLI`. Nó có thể chạy qua plugin host W5b nếu thêm plugin dbt vào `plugin-host/plugins.lock`, nhưng host hiện hỗ trợ plugin không cần storage hay secret, và `profiles` dùng `secret()` cùng `storeManifest` (KV) cần được kiểm tra. **[Chưa kiểm chứng]** Một spike là cách để biết. Phương án thay thế là `egret.container` với image dbt.
- Kestra không có IDE cho dbt; Studio là việc mới trong mọi trường hợp.

## 9. Các quyết định còn mở

1. **Phạm vi bản phát hành đầu.** Đủ chiều môi trường cho KV, file, trigger và secret, hay bản đầu giới hạn ở dbt và các flow không ghi KV hay file, chỉ có personal và prod. Nghiêng về: phương án hẹp hơn.
2. **Service account.** Làm thành gói riêng trước, hay dùng một user kỹ thuật thay thế cho prod lúc đầu.
3. **Secret ở personal.** Secret của chính user, một bộ dev dùng chung, hay không có (chỉ truy cập engine bằng token).
4. **Promote lên prod.** Phê duyệt một tag hoặc SHA, hay tự động từ `main`.
5. **Engine đầu tiên.** Trino, StarRocks hay cả hai.
6. **Độ sâu của git.** Chỉ revision của namespace files, hay một repository thật có nhánh và xử lý conflict.
7. **Trạng thái flow ở môi trường do git quản lý.** Bỏ draft hiện có ở đó, hay giữ như một bản gương chỉ đọc.

## 10. Cần kiểm tra trước khi viết spec

- Execution đọc draft hay bản đã publish (mục 3).
- Mô hình quyền SP4a: principal dẫn xuất đọc như user và ghi vào một schema.
- Trạng thái công việc service account principal.
- Runner container W5a có gọi được ngoài một execution không, và thời gian khởi động của `dbt compile`.
- Các adapter dbt cho Trino và StarRocks và phiên bản dbt-core chúng ghim (`dbt-starrocks` cục bộ yêu cầu `dbt-core>=1.9`).
- Các task git của Kestra hoạt động thế nào và có truy cập được qua plugin host không.
- Chiều môi trường phải chạm tới đâu (khảo sát mọi bảng khoá theo workspace và namespace).
