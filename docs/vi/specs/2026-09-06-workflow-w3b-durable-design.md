# Workflow W3b — Executor bền vững, và lần đầu oracle phán xử một hệ thống đang chạy

Trạng thái: **spec, chờ duyệt**
Ngày: 2026-09-06
Bản chính (tiếng Anh): `docs/en/specs/2026-09-06-workflow-w3b-durable-design.md`
Spec cha: `docs/vi/specs/2026-09-01-workflow-programme-design.md` (§6.3, §9 dòng W3b)
Phụ thuộc: W3a (máy trạng thái thuần), W1 (mô hình), W2 (biểu thức)
Tiếp theo: W4 (runner & container), W6 (trigger & subflow), W7 (frontend)

## 1. Mục tiêu

Làm cho `advance()` **chạy**. W3a dựng một hàm thuần và một oracle vi sai lái nó
qua một harness test; W3b dựng **driver thật** — vòng claim trên Postgres, một
reaper, và các handler thực sự thi hành task — rồi **nối driver đó vào chính
oracle ấy**, để thứ bị đem ra so sánh thôi là một harness và trở thành hệ thống
sẽ ship.

## 2. Vì sao oracle đổi, và điều đó mua được gì

Harness của W3a là bốn tầng mã test: nó **feed** trạng thái kết thúc từ golden,
nó tự render biểu thức, và nó **không bao giờ sinh** sub-execution. Ba hệ quả đã
được đo và công bố trong `oracle-exec/README.md`:

- **39 mục lệch ở đúng một thứ: `spawned`.** `Command::SpawnExecution` được phát
  ra và không ai thi hành.
- **227 trong 1104 cặp thứ tự chưa bao giờ được khẳng định**, vì `spawned` của
  egret luôn rỗng — chỉ 877 cặp thật sự được khẳng định.
- Oracle **vòng tròn ở đúng một trường** — trạng thái cuối của một run
  `Runnable` và của mỗi attempt — vì harness cấp sẵn giá trị đó.

Một driver thật đóng cả ba **cùng lúc**. Nó sinh sub-execution, nên `spawned`
so sánh được và 227 cặp kia vào tầm; và nó sinh trạng thái kết thúc từ một
handler **đã chạy thật**, nên trường bị feed thôi bị feed.

**Đây mới là sản phẩm thật của gói.** Vòng claim là phương tiện.

## 3. Phép đo khiến việc này khả thi

Nối executor thật vào oracle chỉ đáng làm nếu executor **chạy được** corpus. Đo
trên **266** mục `runs` so sánh được:

| loại task không-flowable | số mục cần |
|---|---|
| `core.debug.Return` | 115 |
| `core.log.Log` | 82 |
| `core.execution.Fail` | 55 |
| `core.output.OutputValues` | 23 |
| `core.flow.Sleep` | 20 |

**194 trong 266 mục chỉ dùng `Log`, `Return`, `Fail` và `Sleep`.** Bốn handler
tầm thường chạm tới ba phần tư corpus so sánh được, từ đầu đến cuối.

### 3.1 Danh sách handler trong spec chương trình có TRƯỚC phép đo này

§9 của spec chương trình giao W3b *"task `core.log`, `egret.sql`"*. `egret.sql`
chạm **không** mục corpus nào; `core.debug.Return` chạm **115** và
`core.execution.Fail` chạm **55**, và **không cái nào được nêu tên**. Danh sách
ấy được viết trước khi corpus tồn tại.

W3b làm bốn cái oracle cần — `core.log.Log`, `core.debug.Return`,
`core.execution.Fail`, `core.flow.Sleep` — **và cả `egret.sql`**, vì đó là thứ
biến tính năng này thành sản phẩm chứ không phải bản trình diễn. Nhưng
`egret.sql` **ship mà KHÔNG có oracle nào phủ**, và spec này nói thẳng điều đó
thay vì để một ratchet xanh ngụ ý ngược lại. Tính đúng của nó dựa trên test
riêng chạy với engine thật, như `egret-dataapi` vẫn làm.

`Sleep` là `RunnableTask` trong Kestra dù nằm dưới `core.flow`
(`Sleep.java:47`), nên ở đây nó là **handler**, không phải flowable.

## 4. Kiến trúc

### 4.1 Vòng lặp là cái `egret-automation` đã chứng minh

`crates/egret-automation/src/scheduler.rs` claim bằng
`UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED LIMIT n) RETURNING …`,
và chú thích của chính nó giải thích vì sao. W3b **dùng lại** câu lệnh đó chứ
không phát minh câu mới.

- **vòng worker** — claim `task_runs` có `state = 'pending' AND not_before <= now()`,
  chạy handler, rồi **trong cùng một transaction** ghi kết quả, gọi `advance()`,
  và chèn các task kế tiếp. Không tồn tại cửa sổ nào mà một task đã xong còn
  execution thì chưa nhúc nhích.
- **reaper** — `working_timeout_s`, như automation đang làm.

### 4.2 `advance()` KHÔNG đổi

Hàm của W3a vốn đã thuần trên `(plan, state, now)` và hợp đồng của nó đã viết
xong. W3b là **người gọi**, và plan phải coi mọi thay đổi trong `advance()` là
một **phát hiện** chứ không phải một bước: nếu driver thật cần máy trạng thái
khác đi so với thứ oracle đã phán xử, đó là một phân kỳ cần đo, không phải một
lần refactor cần làm.

### 4.3 Driver trao cho `advance()` những trạng thái harness chưa từng trao

Task 4 của W3a đã đo điều này và để lại như một cảnh báo. Harness áp **một** kết
quả worker mỗi vòng, nên một `CreateRun` không bao giờ được phát ra trong lúc
một anh em còn ở `CREATED`; một runner điều khiển bằng thông điệp thì **có** —
trên `valids/dag`, `valids/if` và `valids/parallel-disabled-tasks`. Kestra có
một guard cho đúng tình huống đó mà W3a đã xoá vì "bất khả đạt", kèm tên của thứ
sẽ làm nó đạt được: *"một người gọi áp một lô kết quả từng phần — worker của
W4."* Người gọi đó bây giờ là W3b. **Hãy chờ nó quay lại.**

### 4.4 Render là việc của người gọi, và người gọi đã đổi

`advance()` không render được. Harness render từ `corpus.json` cộng mã nguồn
flow, và với tới `inputs` và `vars`. `RunContext` của Kestra với tới `outputs`,
`taskrun` và `tasksWithState()`. Khoảng cách đó là lý do bản sửa `runIf:` của
W3c **không bank được gì**, và là lý do `loop-outputs-auto` không phân giải nổi
`values:` của nó. Driver thật **có** outputs, nên **bề mặt render nở ra và
những biểu thức trước đây bất khả đạt trở thành sống** — một số rơi vào các mục
đang baseline **vì lý do khác**. Mọi dịch chuyển như thế phải được **quy trách
nhiệm**, không được hấp thụ.

## 5. `SpawnExecution` thuộc W3b, không thuộc W6

Plan giao nó cho W6 cùng `Subflow`. Sai hai lần: W3a phát lệnh đó cho một **vòng
lặp**, không phải subflow; và sinh execution là một **thao tác ghi bền vững** —
đúng loại việc gói này đang xây. W6 giữ ngữ nghĩa riêng của `Subflow`; W3b cho
cái lệnh ấy một người thi hành.

Đóng nó là thứ khiến 39 mục và 227 cặp trở nên so sánh được, nên plan phải **báo
cả hai con số trước và sau** thay vì tuyên bố đã đóng lỗ.

## 6. Cái gì phán xử W3b

Ba dụng cụ, trả lời ba câu hỏi khác nhau.

1. **Chính ratchet cũ, nhưng phía sau là driver thật.** Đây là phần chính. Mỗi
   mục nó bank là một mục **hệ thống sẽ ship** tái hiện được, không phải một
   harness tái hiện được.
2. **Các tính chất về độ bền, thứ ratchet không nhìn thấy.** Hai worker không
   bao giờ claim cùng một run; một tiến trình bị giết giữa handler và commit
   không để lại execution nửa vời; reaper thu hồi **đúng** những gì
   `working_timeout_s` nói và không gì khác. Những thứ này cần Postgres thật và
   những lần giết tiến trình có chủ đích, không cần corpus.
3. **`egret.sql` với engine thật**, không có oracle phủ, và nói rõ như vậy.

## 7. Luật trung thành

Giữ nguyên từ W3a, và đáng nhắc lại vì một hệ thống đang chạy làm hai điều trong
số đó dễ vi phạm hơn:

- Tái tạo **hành vi** quan sát được, kể cả defect thượng nguồn; không tái tạo
  cấu trúc cài đặt.
- **Over-acceptance không được ship.** W3a đóng hai (`disabled:`, danh sách task
  null) và W3c đóng cái thứ ba (`runIf:`) — rồi chính bản sửa của W3c đẻ ra cái
  thứ tư hẹp hơn, và nó tự đóng nốt. Một driver bền vững có **nhiều chỗ hơn** để
  chạy một thứ Kestra không chạy.
- Một chốt chưa ai thấy nổ là một chốt chưa ai thấy. Tám cái đã ship hoặc suýt
  ship qua W3a và W3c.
- Một con số **viết ra** cạnh một phép suy dẫn là lỗi gói này đã sinh ra **tám
  lần**. Hãy suy dẫn; và khẳng định tổng ngay cạnh các thành phần.

## 8. Rủi ro

1. **Corpus được thu từ một Kestra có chạy task thật. Handler của egret không
   phải handler của Kestra.** `Return` render một chuỗi định dạng; `Log` ghi một
   thông điệp; `Fail` ném lỗi. Ở đâu trạng thái kết thúc trong golden phụ thuộc
   vào việc task **đã làm gì** chứ không chỉ việc nó đã chạy, egret phải tái tạo
   cái việc đó. Task đầu tiên của plan phải đo xem bao nhiêu trong 194 mục phụ
   thuộc vào một **giá trị output**.
2. **Nở bề mặt render sẽ làm các mục dịch chuyển vì những lý do không liên quan
   đến độ bền**, và các dịch chuyển ấy sẽ **trông như** thành quả của W3b. Phải
   quy trách nhiệm.
3. **`egret.sql` ship mà không được phủ.** Đó là một đánh đổi có chủ ý, và rủi ro
   là một ratchet xanh bị đọc thành độ phủ nó không có.

## 9. Sản phẩm bàn giao

- `crates/egret-workflow/src/worker/` — vòng claim, reaper, sổ đăng ký handler,
  và năm handler.
- Bộ thi hành `SpawnExecution`.
- `oracle-exec/` — vẫn corpus và golden ấy, nhưng phán xử qua driver thật.
- Test độ bền với Postgres thật.
- Một tuyên bố độ phủ **tách bạch** thứ ratchet chứng minh với thứ test riêng
  của `egret.sql` chứng minh.
