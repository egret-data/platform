# Workflow W3a — Máy trạng thái executor, và dụng cụ phán xử nó

Trạng thái: **spec, chờ duyệt**
Ngày: 2026-09-04
Bản chính (tiếng Anh): `docs/en/specs/2026-09-04-workflow-w3a-executor-design.md`
Spec cha: `docs/vi/specs/2026-09-01-workflow-programme-design.md` (§6.3, §9 dòng W3)
Phụ thuộc: W1 (mô hình), W2 (biểu thức) — tiếp theo: W3b (vòng claim, reaper, handler)

## 1. Mục tiêu

`advance()` — hàm thuần quyết định execution làm gì tiếp theo — cùng ngữ nghĩa
các flowable nó cần, **đo** với Kestra thật chứ không suy từ mã nguồn.

```rust
/// Không I/O, không DB, không đồng hồ. Vào một ảnh chụp, ra một dãy lệnh.
fn advance(plan: &Plan, state: &ExecutionState, now: Instant) -> Vec<Command>
```

W3 tách đôi vì cái khó về **ngữ nghĩa** và cái khó về **độ bền** là hai loại
việc khác nhau, cần hai loại review khác nhau. Nửa này không có I/O nào. Ranh
giới này spec chương trình đã vẽ sẵn ở §6.5: W3a là `executor/`, W3b là `worker/`.

## 2. Phạm vi

**Trong:** `advance()`; các kiểu `Plan`/`ExecutionState`/`Command`; các flowable
`sequential`, `parallel`, `if`, `switch`, `dag`, `loop`; `retry`, `timeout`,
`errors:`; và oracle vi sai phán xử toàn bộ.

**Ngoài:** vòng claim, reaper, task handler, và mọi thứ chạm Postgres hoặc mạng
(W3b). `subflow` (W6). `pause`/`resume`, `allowFailure`, giới hạn đồng thời,
`WorkingDirectory`, labels (W8).

### 2.1 `loopUntil` thuộc W8, không thuộc W3

Spec chương trình tự mâu thuẫn: bảng gói (§9) xếp `loopUntil` vào W3, còn §10
xếp vào W8 **kèm lý do** — đó là chỗ tập trung khối test của Kestra, và nó cộng
thêm được vào YAML nên hoãn không làm kẹt flow ai đã viết. §10 thắng. Bảng gói
sai và cần sửa khi spec này được duyệt.

`io.kestra.plugin.core.flow.WaitFor` là **alias** của `LoopUntil` (đo từ danh
sách plugin của artifact đang chạy). Bộ phân tích YAML của W1 phải giải được
alias; gói nào làm `loopUntil` thì sở hữu cả hai cách viết.

### 2.2 `loop` ở lại W3a, và `advance()` phát lệnh sinh execution

Đo được, không phải giả định: `Loop` của Kestra **không** lặp bên trong
execution. Mỗi giá trị sinh một **sub-execution** riêng. Qua các lần chạy spike,
mười hai execution gốc của flow `happy` sinh ra **hai mươi hai** sub-execution —
hai lần hỏng dừng ở vòng 0, cộng mười lần thành công trên hai giá trị — trong
khi `fail` và `order` sinh **không**. Task trong thân vòng lặp không bao giờ
xuất hiện trong `taskRunList` của cha.

Cơ chế sinh execution thuộc W6. Thay vì đẩy `loop` sang W6, `advance()` có thêm
biến thể `Command::SpawnExecution`: hàm thuần mô hình hoá trọn ngữ nghĩa và vẫn
thuần, còn **thực thi** lệnh đó là việc của W3b/W6. W3a so vết của execution
cha; phần so xuyên sub-execution đến khi có bộ sinh thật.

### 2.3 Trung thành phải phủ gì, và KHÔNG cần phủ gì

Hai mục tiêu tích hợp đã được xác nhận, và chúng đặt ngân sách trung thành ở hai
chỗ khác nhau:

| mục tiêu | phải trùng cái gì | gói nào trả |
|---|---|---|
| chạy được plugin JAR của Kestra | bề mặt plugin chạm tới — `DefaultRunContext`: render biểu thức, storage, logger, working dir | W2 (xong), W4, W5, W9 |
| nhận YAML Kestra / cho user Kestra chuyển sang | ngữ nghĩa flow: task nào chạy, thứ tự nào, trạng thái và cấu trúc attempt ra sao | W1, **W3** |

Một plugin **không bao giờ** quan sát `advance()`. Máy trạng thái quyết định
*khi nào* một task được gọi, không phải *nó thấy gì*. Nên độ trung thành của
executor không mua được gì cho tương thích plugin, và W3a không được tiêu ngân
sách vào đó.

**Chạy song song với một Kestra sống KHÔNG phải mục tiêu.** Đó là yêu cầu duy
nhất buộc bản ghi execution/log/state của egret phải trùng **ĐỊNH DẠNG** với
Kestra, và nó nằm ngoài. Vì vậy oracle so **vết đã chuẩn hoá về ngữ nghĩa** —
danh tính task, thứ tự, dãy trạng thái, số attempt — chứ không so bố cục JSON
của Kestra. Bản ghi execution của egret được tự do có hình dạng riêng.

Một hệ quả tức thì: mục `boom` lặp hai lần ở §4 phải được tái tạo như **ngữ
nghĩa retry** (hai attempt; execution đi qua `RETRYING`), không phải như một
phần tử mảng lặp lại. Nếu hoá ra đó là hiện vật tuần tự hoá trong bản ghi của
Kestra chứ không phải sự thật ngữ nghĩa, egret **không** sao chép.

## 3. Dụng cụ

Bài học W2 không phải "oracle vi sai thì hay". Nó là: **mọi lỗi đáng kể đều do
ĐO ra, không cái nào do suy luận** — sáu lần một test "hiển nhiên phủ" một ca đã
sống sót qua đúng biến thể lẽ ra phải giết nó. W3a chịu cùng cách đối xử: một
corpus flow, một golden thu từ Kestra thật, và một ratchet.

### 3.0 Corpus là của chính Kestra, không phải của tôi

W2 liệt kê được bề mặt của nó: 63 filter trên một ma trận hữu hạn, nên "chúng ta
phủ hết" là mệnh đề kiểm được. Các cách đan xen thực thi thì **không** hữu hạn,
và một corpus do tôi viết chỉ chứng minh rằng *những flow tôi nghĩ ra* thì khớp
— đúng luận điệu mà W2a sinh ra để phá.

Kestra đã liệt kê giúp rồi. Test resources của nó chứa **346 file flow YAML**,
trong đó `core/src/test/resources/flows` mang **261 `valids`** và **28
`invalids`**. Đó không phải phỏng đoán của tôi về cái gì quan trọng; đó là phát
biểu của chính dự án về việc executor của nó phải làm gì, tích luỹ suốt đời dự
án. Phân bố trên sáu flowable của W3a: `Loop` 40 flow, `If` 24, `Sequential` 21,
`Switch` 11, `Parallel` 10, `Dag` 3.

**28 file `invalids` là phần quý nhất.** Đó là những flow Kestra **từ chối**, nên
chúng kiểm đúng lớp lệch mà chương trình nói không được ship — egret trả lời chỗ
Kestra ném lỗi. Một corpus tự viết gần như không bao giờ sinh ra loại mục này, vì
người viết đang nghĩ theo hướng "cái gì phải chạy".

Đây là corpus nền. Mục tự viết chỉ thêm vào để bù những chỗ **đã đo được** là
hở, không bao giờ để độn độ phủ.

Hai giới hạn, nói trước thay vì phát hiện sau:

1. File YAML là **đầu vào**; phần khẳng định nằm trong mã Java — 401 lớp test,
   67 557 dòng, 2 645 `@Test` chỉ riêng `core`. Phần lớn khẳng định đó là kích
   thước `taskRunList` và trạng thái từng task, thứ mà một execution đã thu về
   chứa sẵn ở dạng rộng hơn. Hai thứ chúng mang mà một lần thu trần thì không:
   **53 trong 261 `valids` khai `inputs:`**, và GIÁ TRỊ nằm trong mã Java chứ
   không nằm trong YAML — `ParallelTest.errors()` lái `finally-parallel.yaml`
   bằng `Map.of("failed", true)`. Chạy mà không truyền input thì **không hỏng ồn
   ào**; nó chạy nhánh còn lại và sinh ra một golden trông ổn mà chẳng kiểm gì.
   Những giá trị đó phải được truy hồi theo từng flow, hoặc đánh dấu flow là
   không chạy độc lập được.
   Và flow nào được test Java lái bằng thiết lập đặc biệt — worker giả, lỗi tiêm
   vào — thì không tái hiện độc lập được.
   **Task đầu tiên của plan là đo xem bao nhiêu trong 261 flow chạy được độc
   lập**, và con số đó là kết quả, không phải giả định.
2. Khớp trên 261 flow vẫn không chặn được hành vi ở flow thứ 262. Ratchet chống
   hồi quy; nó không chứng minh độ phủ. README phải nói điều đó bằng đúng giọng
   `oracle/README.md` đang dùng.

### 3.1 Chạy với cái gì

`kestra/kestra:v2.0.0-rc13`, ghim cứng. Đây đúng artifact oracle W2 đang ghim
(`io.kestra:core:2.0.0-rc13`), và cùng một lý do: `latest` là thẻ động, mà một
golden động thì không phải golden. File compose Kestra ship cần sửa đúng hai
chỗ — ghim image, và bật basic-auth, vì 2.x từ chối mọi lời gọi API không xác thực.

**Artifact là thẩm quyền, checkout thì không.** Quy tắc này giữ nguyên từ W2.

### 3.2 Vết CHUẨN HOÁ ĐƯỢC — đo, không phải mong

Oracle chỉ đứng được nếu hai lần chạy cùng một flow cho ra vết so sánh được.
Chúng có. Chạy hai lần một flow phủ `sequential`, `parallel`, `if`, `switch`,
`dag`, `loop`, hai tài liệu execution khác nhau ở đúng **18 đường trường**, và
tất cả rơi vào hai lớp:

| lớp | trường |
|---|---|
| đồng hồ | `state.startDate`, `state.endDate`, `state.duration`, mọi `state.histories[].date`, `metadata.originalCreatedDate`, và các trường tương ứng bên trong `attempts[]` |
| id sinh ra | `id` (execution và task run), `parentTaskRunId`, `originalId`, `labels[].value` |

**Không** có khác biệt về độ dài, **không** có khác biệt về sự hiện diện. Hình
dạng ổn định; chỉ giá trị chạy.

### 3.3 Thứ tự là thứ tự TẠO, và điều đó được đo bằng thí nghiệm phân biệt

Mười lần chạy cùng một flow cho **một** thứ tự `taskRunList` duy nhất. Riêng
điều đó **không chứng minh gì**: mọi task trong đó xong trong vài mili giây, nên
một danh sách sắp theo *hoàn thành* cũng sẽ trông y hệt.

Nên dựng thêm một flow có ba nhánh song song ngủ 4s, 2s và 1s: thứ tự hoàn thành
là **đảo ngược** thứ tự tạo. Năm lần chạy, một thứ tự duy nhất, và nó là
`par a_slow b_mid c_fast` — **thứ tự tạo**. Sắp theo hoàn thành thì đã đảo.

Đây là giả định chịu lực của cả oracle, và đây là thí nghiệm có thể bác bỏ nó.
Mười lần chạy đầu thì không.

### 3.4 Luật chuẩn hoá

1. Thay mọi id sinh ra bằng thứ tự xuất hiện lần đầu.
2. Bỏ mọi trường đồng hồ — nhưng **suy ra quan hệ xảy-ra-trước** trước đã.
   Chính test của Kestra mang **39 khẳng định thứ tự trong 12 file** dạng
   `a2.startDate.isAfter(a1.endDate)`. Đó không phải khẳng định về tốc độ; đó là
   khẳng định `a2` **tuần tự sau** `a1`, tức đúng cái lý do tồn tại của một
   `sequential` lồng trong `parallel`. Một bộ chuẩn hoá bỏ sạch đồng hồ thì
   **không nhìn thấy** một flowable ngừng tuần tự hoá. Phát ra
   `before`/`after`/`overlapping` cho từng cặp có thứ tự, chỉ giữ những cặp
   **ổn định qua năm lần thu**, và công bố số cặp bị loại. Bản thân thời lượng
   thì bỏ, không chia khoảng: một khoảng thời lượng là khẳng định về tốc độ
   khoác áo khẳng định về tính đúng.
3. Giữ nguyên thứ tự `taskRunList` — §3.3 đã giành được quyền đó.
4. Giữ nguyên dãy `state.histories[].state`; bỏ ngày của chúng.
5. Đi theo `parentId` để thu sub-execution. Chúng **bị giấu khỏi
   `/executions/search`** (đo: một sub-execution lấy được trực tiếp bằng id,
   mang `parentId`, vắng mặt trong danh sách 200 dòng vốn trả về đủ 24 execution
   gốc). Một bộ thu đi liệt kê execution sẽ mất im lặng mọi vòng lặp.

### 3.5 Ratchet

Y hệt W2, cùng lý do: `baseline.txt` liệt kê các mục đang lệch; test hỏng cả khi
một mục ngoài baseline lệch (hồi quy) lẫn khi một mục trong baseline bắt đầu
khớp mà dòng của nó chưa bị xoá (tiến bộ chưa ghi sổ).

## 4. Những gì spike đã tìm ra mà suy luận thì không

Mỗi mục dưới đây là một phỏng đoán **sai của tôi**, bị artifact sửa trong vài
phút. Liệt kê ra vì chúng chính là lý lẽ cho dụng cụ.

| tôi viết | artifact nói |
|---|---|
| `retry.maxAttempt` | `maxAttempts` — và `Constant` mang sáu thuộc tính, gồm `behavior`, `warningOnRetry` |
| `{{ taskrun.value }}` trong thân vòng lặp | `{{ item.value }}` |
| `Sleep: PT0S` | *"duration: must be a positive duration"* |
| `Loop` lặp trong-execution | mỗi giá trị một sub-execution |

Thêm hai điều, ghi lại vì đó là hành vi W3a phải tái tạo và không cái nào hiển nhiên:

- **Một nhánh sai làm hỏng cả ba.** Lỗi kiểm tra ở đúng một nhánh song song
  (`c_fast`) khiến `a_slow` và `b_mid` cũng `FAILED`.
- **Task được retry xuất hiện HAI lần trong `taskRunList`.** Với
  `maxAttempts: 2`, `boom` hiện thành **hai** mục, mỗi mục giữ hai attempt,
  trong khi lịch sử của chính execution đọc là `CREATED RUNNING FAILED RETRYING
  RUNNING FAILED RETRYING RUNNING FAILED`. Điều này **đã đo và CHƯA giải thích**;
  giải thích nó là một task trong plan, không phải một chú thích ở đây.

## 5. Luật trung thành

Kế thừa nguyên vẹn từ spec chương trình và W2:

- Tái tạo **hành vi** quan sát được, kể cả defect của thượng nguồn; **không** tái
  tạo **cấu trúc** cài đặt (tên lớp Java, thứ tự duyệt map). Sub-execution là
  hành vi — chúng có id mà API trả về.
- Over-acceptance — egret trả lời chỗ Kestra từ chối — là lớp lệch không được
  ship, trừ các ngoại lệ README của W2 liệt kê **ngay cạnh** quy tắc.
- Một mục corpus phải **hỏng trước bản sửa** sinh ra nó, và bản sửa phải được
  chứng minh là giết được một biến thể. "Tôi không dựng được biến thể nào thoát"
  là phát biểu về những biến thể ai đó nghĩ ra được.
- Mọi phép đệ quy trên đầu vào của tác giả cần một cận **suy ra được**, viết
  thành biểu thức trên một chi phí đã đo.

## 6. Rủi ro

1. **Độ phủ bị chặn bởi test suite của Kestra, không phải bởi không gian trạng
   thái.** §3.0 thay một corpus lẽ ra do tôi chọn bằng 261+28 flow chính Kestra
   duy trì — nền mạnh hơn hẳn — nhưng vẫn là một mẫu của một không gian vô hạn.
   README phải nói corpus phủ gì và, quan trọng hơn, **không** phủ gì; và con số
   "chạy được độc lập" từ task đầu tiên của plan phải được công bố, không làm tròn lên.
2. **`SpawnExecution` không kiểm được đầu-cuối trong W3a.** Chưa có gì thực thi
   nó tới W3b/W6. W3a khẳng định lệnh **được phát ra**; đó là mức yếu hơn phần
   còn lại của oracle và phải được ghi nhãn như vậy, không được tính là đã phủ.
3. **Chuyện retry hai mục (§4) chưa được giải thích.** Nếu hoá ra đó là defect
   của Kestra chứ không phải mô hình, sao chép nó vẫn là luật — nhưng plan phải
   quyết định điều đó một cách **có ý thức**, không phải do vô tình.

## 7. Sản phẩm bàn giao

- `crates/egret-workflow/src/executor/` — `advance()`, `Plan`, `ExecutionState`,
  `Command`, sáu flowable, retry/timeout/`errors:`.
- `crates/egret-workflow/oracle-exec/` — compose đã ghim, corpus, bộ thu vết, bộ
  chuẩn hoá, `golden.json`, `baseline.txt`, và một README mang các luật theo
  cách `oracle/README.md` đang mang.
- Không đụng `worker/`, `repo.rs`, `routes.rs`. W3a không ship I/O nào.
