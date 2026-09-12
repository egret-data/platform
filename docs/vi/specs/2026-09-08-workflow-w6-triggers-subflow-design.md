# Workflow W6 — Subflow, replay, và bốn chữ oracle không nhìn thấy

Trạng thái: **spec, chờ duyệt**
Ngày: 2026-09-08
Bản chính (tiếng Anh): `docs/en/specs/2026-09-08-workflow-w6-triggers-subflow-design.md`
Spec cha: `docs/vi/specs/2026-09-01-workflow-programme-design.md` (§9 dòng W6)
Phụ thuộc: W3b (executor bền vững), W4 (runner) — tiếp theo: W7 (frontend), W8

## 1. Phép đo, trước tiên, vì nó chẻ gói này làm đôi

§9 spec chương trình giao W6: *"cron, webhook, flow trigger, `subflow`,
backfill"*. Đo trên corpus 345 flow và baseline 143 mục **trước khi viết bất cứ
dòng nào**:

| construct | khai báo trong corpus | khai báo trong baseline | **chạm tới được** |
|---|---|---|---|
| `Subflow` | 25 flow | 22 mục | **9 chặt / 11 cận trên** |
| flow trigger | 17 flow | 0 | **0** |
| cron / schedule trigger | 7 flow | 1 | **0** |
| webhook trigger | 15 flow | 3 | **0** |
| backfill | **0 dòng sống** (2 dòng bị comment) | 0 | **0** |
| replay `CREATE_NEW_EXECUTION` | 4 | 4 | **4** |

**Bốn trong năm chữ của dòng ấy đáng KHÔNG mục baseline nào, và một trong bốn
chữ đó không có lấy một dòng khai báo trong corpus.** Nên:

**Kết quả, do W6 Task 8 thêm vào để người đọc đến sau không cầm phải con số ước
lượng.** Mọi số dưới đây do
`the_sixteen_banked_entries_are_derived_and_the_four_triggers_reach_none`
trong `crates/egret-workflow/tests/executor.rs` dẫn ra, không phải trích:

| dòng | bảng trên dự đoán | đo được sau khi làm xong |
|---|---|---|
| `Subflow` | 9 chặt / 11 cận trên | **bank 12** — vượt cận trên một mục, vì tôi đọc *khai báo* `concurrency:` thành *bị chặn bởi* nó, mà `QUEUE`/`CANCEL` chỉ đổi THỜI ĐIỂM con chạy |
| replay `CREATE_NEW_EXECUTION` | 4 | **bank 4** |
| flow trigger / schedule / webhook / backfill | mỗi thứ 0 | **mỗi thứ 0**, và giờ ghi CƠ CHẾ của từng con 0 chứ không chỉ phán quyết — `oracle-exec/README.md`, *The coverage statement* → mục W6 |

`143 → 127`, tức **16 = 12 + 4**. Ước lượng 13–15 thiếu một ở cận trên, và bốn
con 0 vẫn đứng.

- **Nửa bank được là `Subflow` + replay** — 13 tới 15 mục, phán xử bằng ratchet
  đang có, và chúng **dùng chung một migration**.
- **Nửa không đo được là cron, webhook, flow trigger và backfill.** W6 vẫn xây,
  vì một tính năng Workflow không có lịch thì không phải một tính năng — nhưng
  **không gì trong ratchet nhúc nhích vì thứ W6 xây ở đó, và không gì trong
  ratchet nhận ra nếu nó hỏng.** Chúng được phán xử bằng test riêng, đúng vị trí
  `egret.sql` (W3b) và `egret.container` (W4) đang đứng.

Spec này nói điều đó **ngay trang đầu** thay vì để một ratchet xanh ngụ ý ngược
lại — kỷ luật mà §3.1 của W3b lập ra **sau khi đã làm xong**, và W4 áp dụng
**trước khi làm**.

### 1.1 Vì sao trigger vô hình — CƠ CHẾ, không phải phán quyết

"Oracle không thấy được" là **ba sự kiện khác nhau**, và đây là cái thứ ba.

**Harness CÓ kích hoạt trigger và KHÔNG nhìn thấy nổi một execution nào chúng
tạo ra.** Cả hai nửa đều đo trên artifact ghim. `docker compose` chạy
`server standalone`, tức là có scheduler; publish ba flow trigger rồi chạy một
cái sinh ra **13 execution, trong đó 10 cái đến từ thứ khác chứ không phải POST
của harness** — sáu từ một `Schedule` không ai yêu cầu, ba từ `Subflow`, một từ
`Flow` trigger.

Không cái nào tới được golden, vì ba lý do độc lập đã kiểm chứng:

1. `harvest.py` **không bao giờ** gọi `POST /executions/webhook/…` — chuỗi ấy
   xuất hiện đúng một lần trong file, với tư cách **lý do** loại
   `valids/webhook-inputs`. Webhook trigger chưa từng được gọi.
2. `run_flow` settle `{root} ∪ descendants(root)`, và `descendants()` đi theo
   **`parent_id`** — thứ Kestra để **NULL** cho mọi execution do trigger tạo (cái
   do `Flow` bắn mang `trigger_execution_id`; cái do `Schedule` bắn không mang cả
   hai).
3. `normalise.py` dựng `by_parent` từ `parentId` và `bail` với bất kỳ descendant
   nào không treo vào root — nên kể cả có tìm ra cũng không normalise nổi vào
   trace.

**Cơ chế, nói cho chính xác:** đây **không phải** một vị từ không bao giờ được
đánh giá. Đây là **một vị từ được đánh giá trên cả 268 flow đã chạy, ở mọi lượt
settle poll, mà về mặt cấu trúc KHÔNG THỂ chọn được một execution do trigger tạo
ra** — nó khoá theo một cột Kestra không bao giờ set cho loại ấy. Bên cạnh là một
cơ chế thứ hai yếu hơn: regex của nhánh quét log **không thể khớp trên artifact
này**, cả ở body JSON thô lẫn message đã giải mã — nên `descendants()` có **một**
nhánh chứ không phải hai như chính tài liệu của nó nói.

**Làm cho trigger đo được là một gói RIÊNG, và cố ý không phải gói này.** Nó cần
`descendants()` union thêm `trigger_execution_id`, `normalise.py` dựng cây theo
link ấy, một golden schema chứa được sub-execution **không có loop index**, một
lần **re-harvest**, và **viết lại `baseline.txt`** — cái mốc mà mọi con số đã
công bố từ W1 tới W4 đều phát biểu dựa vào. Giá ấy phải nằm **cạnh quyết định**,
không nằm **bên trong** gói này.

## 2. `Subflow` KHÔNG dùng lại được `SpawnExecution`, và lý do chịu lực

W3b đã dựng bộ thi hành `SpawnExecution`. Trỏ `Subflow` vào đó là cám dỗ tự
nhiên. **Làm thế sẽ phá đúng những mục nó định bank**, vì bốn khác biệt cấu trúc,
mỗi cái đều đo được:

1. `SpawnExecution` lấy nguồn của con **từ chính YAML của cha** và mang theo một
   `value` của vòng lặp. `Subflow` phân giải **một flow KHÁC** theo
   `(namespace, flowId)`.
2. CHECK được-ăn-cả-ngã-về-không của migration `0135` làm cho **một đứa con có
   cha nhưng không có loop index** trở nên **không biểu diễn được**.
3. **Quyết định:** `driver.rs` dựng trường `spawned` từ `parent_execution_id`.
   Nối một đứa con `Subflow` kiểu ấy sẽ đẩy nó vào trace của egret — trong khi
   **cả 41 mục golden có mang `spawned` đều có KHÔNG đứa con `Subflow` nào.** Mấy
   mục định bank sẽ biến thành lệch ở `spawned`.
4. Chính Kestra phân biệt: nó set `parentId` cho **một vòng lặp** và cho một
   **replay `CREATE_NEW_EXECUTION`**, và để NULL cho **con của subflow**, thứ
   được nối bằng `trigger_execution_id`.

**Nên W6 chép đúng sự phân biệt của Kestra thay vì nới rộng
`parent_execution_id`.** Bốn mục replay cần **đúng** thay đổi schema mà `Subflow`
cần — đó là lý do chúng nằm trong gói này: **một migration, không phải hai**.

## 3. Driver hôm nay không phân giải nổi một target `Subflow`

`oracle-exec/driver.rs` gọi `start_execution(..., None, source, inputs)` — nó
**không bao giờ publish corpus vào `workflow.flows`**. Nên không có bảng nào để
một `Subflow` phân giải `(namespace, flowId)`, và hôm nay egret **không chạy nổi
một `Subflow` nào**.

**W6 phải sửa driver trước khi `Subflow` đo được**, và bản thân thay đổi ấy là
một rủi ro: publish 345 flow vào đường của driver **nới rộng thứ mà mọi mục khác
chạm tới được**. Mọi mục dịch chuyển vì lý do ấy phải được **quy trách nhiệm**,
không được hấp thụ — đúng lỗi §4.4 của W3b đã gọi tên khi bề mặt render nở ra.

## 4. Vì sao 22 rơi xuống 9

Ba con số, mỗi cái đứng một mình đều sai — **22** (có khai báo `Subflow`), **21**
(có một run `Subflow` đã chạy), **15** (không có blocker nào khác). Con số chặt
là **9**, cận trên **11**, và cú rơi đến từ **hai điều kiện cần độc lập**:

- **Bảy mục có một blocker thứ hai thuộc về người khác** — `concurrency:` của W8
  ×2, tàn dư `Loop` ×2, `AllowFailure`/`WARNING` ×2, và `OutputValues` ×1. Cái
  cuối, `valids/task-runif`, chỉ có một run `Subflow` **SKIPPED** *và* một flow
  đích **không nằm trong corpus**.
- **Bốn mục nữa bị chặn vì chính target `Subflow` của chúng đang nằm trong
  baseline.**

Một mục khai báo `Subflow` nhưng lệch vì chuyện khác **không phải của W6**, và
plan phải báo cáo **mọi mục đã bank kèm lý do nó dịch chuyển**.

## 5. Cái gì phán xử W6

Ba dụng cụ, và chỉ một là ratchet.

1. **Ratchet, cho `Subflow` và replay** — 9–11 cộng 4, báo **trước và sau**, với
   **mọi mục dịch chuyển đều được quy trách nhiệm**. Một mục bank vì lý do chưa
   quy được là **trùng hợp**, không phải một bank.
2. **Test riêng của trigger**, không có oracle phủ, và nói rõ như vậy. Một cron
   bắn hai lần, một webhook xác thực, một flow trigger lọc — không cái nào corpus
   nhìn thấy, nên bộ test của chúng **là toàn bộ dụng cụ**. Luật W4 áp dụng:
   **một bộ test tự bỏ qua trong im lặng không phải là độ phủ**, và thiếu phụ
   thuộc thì phải **HỎNG** chứ không được bỏ qua.
3. **Bốn khẳng định SAI mà phép đo này tìm ra trong hồ sơ hiện có** (§7) phải
   được sửa — vì chính chúng là lý do sự im lặng của ratchet về trigger **trông
   như** không có hành vi, thay vì như một **defect của dụng cụ**.

## 6. Luật trung thành

Giữ nguyên, ba điều dưới đây chịu lực:

- **Tái tạo hành vi quan sát được, kể cả defect thượng nguồn.** Sự chia đôi
  `parentId`/`trigger_execution_id` của Kestra là thứ để **chép**, không phải để
  **dọn cho gọn**.
- **Over-acceptance không được ship.** Một trigger bắn ở chỗ Kestra không bắn là
  ca tiếp theo, và **oracle không bắt được**.
- **Một chốt chưa ai thấy nổ là một chốt chưa ai thấy** — mười cái qua W3a, W3c,
  W3b; W4 thêm mặt ngược lại: **một chốt DƯ THỪA và một chốt YẾU trông giống hệt
  nhau trong bảng đột biến.**
- **Một con số chỉ an toàn nếu có một chương trình đang chạy SO SÁNH nó.** Đợt
  quét của W4 mất năm lượt và 23 lần sửa mới tới được luật ấy; nó áp dụng cho mọi
  thứ W6 viết ra, **kể cả spec này**.

## 7. Những khẳng định sai trong hồ sơ, phải sửa

Mỗi cái đều đo được, không suy luận:

1. `oracle-exec/README.md` khẳng định *"Một sub-execution … mang `parentId`"* —
   **SAI** với con của `Subflow` trên artifact ghim, ở đó `parentId` là NULL và
   link là `trigger_execution_id`.
2. Cũng mục ấy mô tả `descendants()` như hợp của **hai** phát hiện độc lập. Regex
   của nhánh quét log không khớp cả body JSON thô (dấu nháy bị escape) lẫn message
   đã giải mã (markup mang `flowId=`/`namespace=` trước `]]`). **Đó là mã chết;
   chỉ có MỘT nhánh.**
3. Thông điệp khẳng định của `ENTRIES_LEFT_UNFINISHED = 84` nói mỗi mục là một
   loại task không có handler. Bốn mục `retry-new-execution-*` chỉ khai
   `core.execution.Fail`, thứ **CÓ** handler, và kết ở `RETRIED` — trạng thái mà
   `is_stalled` không liệt vào danh sách terminal, dù chính doc của
   `RunState::Retried` và `State.isTerminated()` của Kestra đều gọi nó là
   terminal. **4 trong 84 là hiện vật của bộ phân loại, không phải một run bị
   kẹt.**
4. Chính dòng W6 của spec chương trình. Nó được sửa **trong cùng commit với spec
   này, ở cả hai ngôn ngữ** — và bản sửa ấy chính là nội dung §1.

## 8. Rủi ro

1. **Publish corpus vào driver nới rộng bề mặt render VÀ bề mặt phân giải**, và
   sẽ có mục dịch chuyển vì lý do không liên quan tới `Subflow`. Phải quy trách
   nhiệm; đây là lỗi §4.4 của W3b với một nguyên nhân mới.
2. **Đổ `trigger.*` vào ngữ cảnh render sẽ làm `valids/webhook-plaintext` lệch
   thêm**, vì golden được thu với **không** ngữ cảnh trigger nào. Đo trước khi đổ.
3. **Trigger ship mà không được oracle phủ** — một đánh đổi có chủ ý, rủi ro là
   một ratchet xanh bị đọc thành độ phủ nó không có. Ba gói liên tiếp đã lấy đánh
   đổi này; **W6 là gói đầu tiên mà nửa không được phủ lại là nửa LỚN HƠN**.
4. **Scheduler là một cái đồng hồ**, và test của chương trình này đã bị đồng hồ
   tường cắn nhiều lần. Test trigger phải dùng **đồng hồ tiêm vào**.

## 9. Sản phẩm bàn giao

- `Subflow` với cột link **riêng**, không phải `parent_execution_id`, và replay
  `CREATE_NEW_EXECUTION` trên **cùng một migration**.
- Một driver có publish corpus để `(namespace, flowId)` phân giải được.
- Cron, webhook và flow trigger, với test riêng và **đồng hồ tiêm vào**.
- Backfill, với test riêng, và một câu viết rõ rằng corpus chứa **KHÔNG** dòng
  khai báo sống nào của nó.
- Bốn bản sửa ở §7.
- Một tuyên bố độ phủ **tách bạch** 13–15 mục ratchet nói được với đám trigger nó
  không nói được.
