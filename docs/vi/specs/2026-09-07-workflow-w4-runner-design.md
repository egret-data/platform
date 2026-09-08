# Workflow W4 — Runner, và ba cái lỗ W3b chuyển giao

Trạng thái: **spec, chờ duyệt**
Ngày: 2026-09-07
Bản chính (tiếng Anh): `docs/en/specs/2026-09-07-workflow-w4-runner-design.md`
Spec cha: `docs/vi/specs/2026-09-01-workflow-programme-design.md` (§6.4, §9 dòng W4)
Phụ thuộc: W3b (executor bền vững) — tiếp theo: W5 (plugin host), W9 (KV, storage)

## 1. Phép đo làm đổi hình dạng gói

§9 spec chương trình **đã** giao W4: *"hợp đồng `TaskCommands`;
`egret.container`, `egret.python`; stream log; file output; cancel/kill"* —
trích nguyên văn như nó đứng TRƯỚC khi gói này đo bất cứ thứ gì; dòng ấy nay đã
được sửa, vì **`egret.python` chưa bao giờ là việc của W4**: §6.4 giao tên đó cho
`KernelLauncher` của notebook, và `TASK_TYPES` đã giữ nó từ W1 kèm một tham chiếu
`notebook` BẮT BUỘC. Handler của W4 là **`egret.script.python`**. Đọc lên thì đây
là một gói về chạy container.

Đo trên corpus **trước khi viết bất cứ dòng nào**:

- **Không có một task script, container, shell hay command nào trong cả 345
  flow.** Loại duy nhất ngoài `core.*` là `io.kestra.plugin.fs.http.Trigger`,
  trên hai mục, và đó là việc của W6.
- Nên `egret.container` và `egret.script.python` chạm **KHÔNG** mục corpus nào và
  **KHÔNG** mục golden nào — y hệt `egret.sql`, thứ mà spec §3.1 của W3b đã ghi
  lại điều tương tự, nhưng chỉ sau khi đã làm xong.
- Trong khi đó, **ba cái lỗ W3b chuyển giao thẳng cho W4 chạm tới 20 mục
  baseline riêng biệt** (chồng nhau đúng 1):

| lỗ W3b để lại | số mục baseline |
|---|---|
| `timeout:` cấp task — thứ **worker** của Kestra cưỡng chế (`WorkerTaskCallable`, và validator của nó nói rõ) | 4 |
| `retry:` có `interval:` khác 0 — hôm nay **không có bộ đánh thức nào ngoài oracle** | 12 |
| kill / cancel — `TaskHandler::cancel` tồn tại và **không ai gọi** | 5 |

Cộng thêm bốn mục lệch **chỉ** ở quan hệ xảy-ra-trước, vì driver của W3b kết
thúc **một run mỗi tick** — một phán quyết đưa ra ở W3b và chuyển giao tường
minh về đây.

**Vậy sản phẩm mà oracle nhìn thấy được của W4 là mấy cái LỖ, không phải mấy cái
HANDLER.** Bộ chạy container là việc sản phẩm, có test riêng, đúng như
`egret.sql`. Spec này nói điều đó **ngay đầu** thay vì để một ratchet xanh ngụ ý
rằng bộ chạy đã được đo.

## 1.1 Con số "20 mục baseline" là hợp của ba lần grep khai báo, và nó là ~0

**Sửa §1 chứ không sửa vào §1.** Bảng trên đếm những flow *khai báo* một thứ.
Task 1 và Task 2 đo cái *chạm tới được*, và câu trả lời gần như là không có gì:

| lỗ | khai báo | chạm tới được | ai đo |
|---|---|---|---|
| `timeout:` | 4 | **0** | Task 2 — ba cái nằm trên một `Pause`, và worker của Kestra **bỏ qua `timeout:` trên một flowable** (đo trên artifact: một `Sequential` bọc một `Sleep` 4 giây chạy SUCCESS trong 4,2 s). Cái thứ tư xin **một giờ** trên một `core.log.Log`. Có đo đạc: ngân sách được áp **4** lần trên toàn oracle và hết hạn **0** lần. |
| `retry:` có `interval:` khác 0 | **14, không phải 12** | **10 cái có thể kẹt, 0 cái nhúc nhích** | Task 1 — 4 cái là `CREATE_NEW_EXECUTION` và không bao giờ ghi `not_before`. Và oracle **không thấy được cả lỗ lẫn thuốc**: `driver::pass_time` vừa lùi `created_at` vừa **tự gọi `worker::resume`**, nên một retry đến hạn bị tiêu thụ ngay bên trong lời gọi làm nó đến hạn. Có đo đạc: bộ đánh thức chạy **0** lần, với một positive control chạy được trên cùng bản build. |
| kill | 5 | **0** | Đọc lại từ chính YAML của năm flow ấy: một `Exit` mang `state: KILLED` (một trạng thái tác giả KHAI, không phải ai giết), một `Pause` trơn, hai flow khai `concurrency: {behavior: QUEUE, limit: 1}`, và một flow cha khai hai `Subflow` và được đặt tên theo giới hạn concurrency của **con** nó. Bốn cấu trúc của W8 và một output; **không cái nào khai một `timeout:`**, thứ duy nhất chạm tới đường kill. |

**Vậy W4 làm ratchet nhúc nhích đúng bằng không, và đó là tiêu đề trung thực.**
Bộ đánh thức `RETRYING` vẫn đóng lại một run treo vĩnh viễn; `timeout:` vẫn tới
được handler ở chỗ trước đây nó là một hằng số 300 s. Đó là những lỗi thật trong
đường chạy sản phẩm. Chúng chỉ đơn giản là những lỗi **oracle này không có cách
nào nhìn thấy** — đúng vị trí của `egret.sql` và của bộ chạy container, và đúng
vị trí W3c đã đứng khi nó đóng một over-acceptance mà không bank được gì.

Bài học chung, và nó là của tôi: **`grep` một từ khoá thì đếm được khai báo, còn
con số tiêu đề của một gói phải đếm cái mà một đường mã chạm tới được.** Cả ba
phần ba đều sai cùng một chiều, nên hợp của chúng sai ba lần.

> **W4 Task 6 thêm mục này**, vì bản tiếng Anh có §1.1 từ Task 2 còn bản tiếng
> Việt thì không: bản tham chiếu vẫn mang nguyên khẳng định chưa sửa, không một
> dấu hiệu nào. Một đính chính chỉ làm ở một ngôn ngữ là một cặp số đúng-ở-hai-
> thời-điểm-khác-nhau, đúng cái hình dạng mục *coverage statement* của
> `oracle-exec/README.md` tồn tại để thu hồi.

## 2. Ba cái lỗ ấy thực chất là gì

### 2.1 `RETRYING` không có bộ đánh thức, và câu nói ngược lại là SAI

`claim.rs` biện minh cho việc bỏ điều kiện `not_before` bằng câu *"độ trễ là việc
của `advance()` và nó đã làm rồi"*. Review toàn nhánh của W3b liệt kê đủ: claim
**loại** `RETRYING`; `reap` **loại** nó; `reopen_due` chỉ chạy bên trong một
`settle` có `now` **cố định cho cả transaction**; và người gọi `resume` duy nhất
trong cây là driver của oracle, đi qua `pass_time` — thứ **quay ngược
`executions.created_at`** để giả lập độ trễ.

Nên một retry trên một run **không có anh em nào còn sống** sẽ ngồi `RETRYING`
**vĩnh viễn**, im lặng. **Mười bốn** mục baseline khai `interval:` khác 0 —
xem §1.1; câu này viết "mười hai" cho tới khi Task 1 đo lại.

Đây là sản phẩm đầu tiên của gói, và nó là một bài toán **lập lịch**, không phải
bài toán container.

### 2.2 `timeout:` thuộc về worker, và worker là của W4

W3a Task 3 xác lập điều này trên artifact, và **sửa lại một brief trước đó của
tôi** vốn gán nó cho `advance()`: Kestra cưỡng chế `timeout` trong
`WorkerTaskCallable`, và chính validator của nó cảnh báo rằng thuộc tính này
*"chỉ liên quan tới runnable task"*. W3b sau đó đo được rằng `timeout:` cấp task
**bị tước trước khi handler nhìn thấy**, nên câu lệnh của `egret.sql` chạy dưới
một hằng số 300 giây **không liên quan gì** tới điều tác giả yêu cầu.

### 2.3 `cancel` từ chối, có chủ ý, để W4 hỏng ầm ĩ

W3b Task 6 **không** cài `TaskHandler::cancel` cho `egret.sql`. Nó cho hàm ấy
**từ chối**, kèm lý lẽ ghi lại: cài một phương thức **không ai gọi** thì sinh ra
một chốt **không ai thấy nổ**, và gói này đã sinh ra mười cái như thế. Lời từ
chối ấy là một **dây bẫy đặt đúng chỗ phải thay đổi** khi đường kill được nối.

**W4 được kỳ vọng sẽ giẫm phải nó. Nếu không giẫm, đó là một phát hiện.**

## 3. `TaskCommands`, và cái tên không được dùng lại

§6.4 spec chương trình đặt tên trait là `TaskHandler` và nói rõ **không** phải
`TaskRunner`, vì trong Kestra cái tên đó nghĩa là một bộ thi hành lệnh shell
(`TaskRunner.run(RunContext, TaskCommands, filesToDownload)`, javadoc ghi *"sẽ
được script plugin gọi"*). W4 đưa vào hợp đồng `TaskCommands` — chính thứ mà
`TaskRunner` của Kestra nhận — nên đây là gói mà hai cái tên **đứng cạnh nhau**
và sự phân biệt phải giữ được.

## 4. Cái gì phán xử W4

Ba dụng cụ, và chỉ một trong số đó là ratchet.

1. **Ratchet, cho mấy cái lỗ.** Hai mươi mục baseline chạm tới được bằng cách sửa
   lập lịch, timeout và hủy. Đó là con số phải báo **trước và sau**, với **mọi
   mục dịch chuyển đều được quy trách nhiệm** — một mục bank vì lý do chưa quy
   được là một **trùng hợp**, không phải một bank.
2. **Test riêng của bộ chạy container.** Như `egret.sql`, và kèm bài học W3b học
   theo cách đắt: bộ test của `egret.sql` in `7 passed` khi **không có engine
   nào**, giống hệt lúc có, cho tới khi bị bắt phải hỏng. **Một bộ test tự bỏ
   qua trong im lặng không phải là độ phủ.** Nếu bộ chạy của W4 cần Docker thì
   thiếu Docker phải **HỎNG**, và muốn bỏ qua thì phải **gõ ra**.
3. **Quyết định một-run-mỗi-tick.** W3b phán quyết giữ nguyên vì mở rộng nó sẽ
   đổi ranh giới transaction mà các tính chất độ bền tựa lên, và chuyển quyết
   định về đây với lý do W4 có worker pool thật. Giờ W4 sở hữu nó, và **cả hai
   câu trả lời đều chấp nhận được — miễn là giá của nó được đo**: bốn mục
   cặp-thứ-tự ở một bên, hình dạng transaction ở bên kia.

## 5. Luật trung thành

Giữ nguyên, và hai điều dưới đây chịu lực ở gói này:

- **Over-acceptance không được ship**, và review toàn nhánh W3b tìm ra những ca
  **lúc chạy** đầu tiên — mười một cái, giờ đã có sổ đăng ký. Một bộ chạy khởi
  động một tiến trình mà Kestra lẽ ra đã giết là cái tiếp theo.
- **Một chốt chưa ai thấy nổ là một chốt chưa ai thấy.** Mười cái qua W3a, W3c
  và W3b. Lưu ý W3b còn tìm ra mặt ngược lại: **hai lớp chốt là thay thế cho
  nhau**, nên mọi đột biến một-vết đều sống sót vì mỗi cái tự nó đã đủ. **Một
  chốt dư thừa và một chốt yếu trông giống hệt nhau trong bảng đột biến** —
  khác biệt phải được **đo** và **ghi ra**.

## 6. Rủi ro

1. **Không có gì trong corpus chạm tới bộ chạy**, nên tính đúng của nó dựa hoàn
   toàn vào test W4 tự viết. Cùng một đánh đổi `egret.sql` đã lấy, và cùng một
   rủi ro: một ratchet xanh bị đọc thành độ phủ nó không có.
2. **Docker trong đường test.** `egret.sql` cần StarRocks và W3b đã làm cho lỗi
   đó **ồn ào** thay vì im lặng. Một bộ chạy container cần một daemon, cùng quy
   tắc — nhưng giá cao hơn, vì **số máy thiếu Docker nhiều hơn số máy thiếu một
   cơ sở dữ liệu**.
3. **Đường kill cắt ngang cái lease.** Critical của W3b là một worker đã bị thu
   hồi vẫn commit đè lên kẻ thay thế nó, vá bằng một hàng rào trên cả hai bên
   ghi. Hủy là **cùng đường nối ấy tiếp cận từ phía kia**: một handler bị giết
   cũng **không được** commit, và hàng rào lẽ ra đã từ chối nó — **hãy kiểm
   chứng điều đó, đừng giả định**.

## 7. Sản phẩm bàn giao

- Bộ đánh thức `RETRYING`, và câu văn sai trong `claim.rs` được sửa.
- `timeout:` cấp task tới được handler, do worker cưỡng chế.
- Đường kill, giẫm lên lời từ chối có chủ ý của `egret.sql`.
- `TaskCommands`, `egret.container`, `egret.script.python`, với test riêng và một lỗi
  **ồn ào** khi thiếu daemon.
- Một phán quyết về một-run-mỗi-tick, kèm giá đã đo.
- Một tuyên bố độ phủ **tách bạch** hai mươi mục ratchet nói được với bộ chạy mà
  nó không nói được.
