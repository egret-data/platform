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

## 3. Dụng cụ

Bài học W2 không phải "oracle vi sai thì hay". Nó là: **mọi lỗi đáng kể đều do
ĐO ra, không cái nào do suy luận** — sáu lần một test "hiển nhiên phủ" một ca đã
sống sót qua đúng biến thể lẽ ra phải giết nó. W3a chịu cùng cách đối xử: một
corpus flow, một golden thu từ Kestra thật, và một ratchet.

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
2. Bỏ mọi trường đồng hồ. Thời lượng **bỏ**, không chia khoảng: một khoảng thời
   lượng là một khẳng định về tốc độ khoác áo khẳng định về tính đúng.
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

1. **Corpus là chọn, không phải liệt kê.** W2 liệt kê được bề mặt của nó — 63
   filter, một ma trận hữu hạn. Các cách đan xen thực thi thì không hữu hạn.
   Plan phải nói corpus phủ gì, và quan trọng hơn, **không** phủ gì.
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
