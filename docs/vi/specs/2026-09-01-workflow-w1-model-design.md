# Workflow W1 — Mô hình & ngôn ngữ Flow

Trạng thái: **spec, chờ duyệt**
Ngày: 2026-09-01
Bản chính (tiếng Anh): `docs/en/specs/2026-09-01-workflow-w1-model-design.md`
Spec cha: `docs/vi/specs/2026-09-01-workflow-programme-design.md` (§9, dòng W1)
Phụ thuộc: W0 (`egret-workflow`, `egret-workflow-api`, `models::State`, entitlement `workflows`)

## 1. Phạm vi

W1 biến một flow thành **artifact hạng nhất, có version, publish được**. Nó giao:

- mô hình flow, port từ Kestra;
- parse + validate YAML;
- `workflow.flows` / `workflow.flow_versions`, phản chiếu cách tách draft/version của Data API;
- publish, gồm **giải tham chiếu** — đây là mối nối bảo mật;
- các route quản trị cần cho những việc trên.

Nó **không** giao executor, scheduler, task handler, hay đánh giá biểu thức. Trong W1 một flow viết
được, validate được, có version và publish được; không gì chạy nó cho tới W3.

## 2. Những gì đã chốt với người dùng

1. **Biểu diễn task theo lối lai.** Task control-flow là enum có kiểu vì máy trạng thái buộc phải hiểu
   cấu trúc của chúng; task lá thì mờ và do handler nhận.
2. **Port từ vựng kiểu `inputs:` của Kestra**, không dùng `ParamType` 6 biến thể của egret. Người dùng
   chọn trung thành thay vì tái dùng từ vựng, chấp nhận egret mang hai từ vựng tham số song song.
3. **Port toàn bộ mô hình flow của Kestra**, không lấy tập con — `labels`, `variables`, `outputs`,
   `retry` và `timeout` cấp task, `concurrency`, và phần còn lại.

Về (3): port *hình dạng dữ liệu* của `sla`, `checks`, `quotas`, `concurrency` là rẻ — chúng là các
value type 18–198 dòng, không bị gate Enterprise trong core — và khai báo ngay bây giờ nghĩa là định
dạng snapshot không phải đổi khi W3 và W8 cho chúng hành vi. *Hành vi* của chúng không thuộc W1. Một
flow khai báo `concurrency` sẽ parse, validate, có version và publish được; chưa gì thi hành nó, và
validator **nói rõ điều đó** thay vì nhận vào im lặng.

## 3. Mặt cần port

| Gói Kestra | LOC | File |
|---|---|---|
| `models/flows` (gồm `input/`, `sla/`, `check/`, `quota/`) | 3.454 | 53 |
| `models/tasks` (gồm `retrys/`, `common/`) | 3.078 | 51 |
| `models/executions` | 2.826 | 28 |
| `models/triggers` | 1.486 | 23 |
| `models/property` | 660 | 5 |
| **Tổng** | **11.504** | **160** |

Ước **3,5–4,5k dòng Rust**. Mã model Java nén rất mạnh: Lombok `@Builder`/`@Getter`, annotation
`@Schema` của OpenAPI, và wiring đa hình của Jackson đều thu về các `derive`.

`models/executions` được port ngay ở W1 dù chưa gì sinh ra `Execution` trước W3. Lý do đúng như đã áp
cho `State` ở W0: đó là các kiểu mà máy trạng thái thuần của W3 được viết dựa trên, và định nghĩa chúng
muộn nghĩa là viết máy đó trên một mục tiêu di động.

## 4. Mô hình task

### 4.1 Lai, và vì sao

```rust
pub struct Task {
    pub id: String,
    pub r#type: String,          // FQCN của Kestra là dạng chính tắc
    pub description: Option<String>,
    pub retry: Option<Retry>,
    pub timeout: Option<Property<Duration>>,
    pub disabled: bool,
    pub kind: TaskKind,
}

/// Control flow mà máy trạng thái buộc phải hiểu cấu trúc; còn lại thì mờ.
pub enum TaskKind {
    Sequential(Sequential),
    Parallel(Parallel),
    If(If),
    Switch(Switch),
    Dag(Dag),
    ForEach(ForEach),
    Subflow(Subflow),
    AllowFailure(AllowFailure),
    WorkingDirectory(WorkingDirectory),
    /// Mọi thứ khác: `egret.sql`, `core.http`, và sau này `io.kestra.plugin.jdbc.trino.Query`.
    Leaf { spec: serde_json::Value },
}
```

Cách tách này không phải cho tiện. `advance()` của W3 duyệt cây task để quyết định chạy gì tiếp, nên nó
**buộc phải** nhìn được vào `Sequential`/`Parallel`/`ForEach` — để `serde_json::Value` ở đó sẽ ép máy
trạng thái thuần parse lại cây ở mỗi lần gọi, mất đúng tính chất mà §6.3 của spec chương trình được
thiết kế quanh nó. Nó cũng **không được** nhìn vào `egret.sql`: một máy trạng thái biết SQL là gì thì
không còn là máy trạng thái.

`Leaf { spec }` cũng là biểu diễn duy nhất chứa được `io.kestra.plugin.*` khi plugin host của W5 tới,
mà không phải sửa enum.

### 4.2 Giải `type`

`type` được đối chiếu với một registry các FQCN control-flow đã biết
(`io.kestra.plugin.core.flow.Sequential` và anh em, cộng alias ngắn `core.sequential`, …). Không khớp
thì thành `Leaf`. **Một `type` lạ không phải lỗi parse** — parse không được phụ thuộc vào việc biết một
task làm gì, nếu không thì không sửa được flow trước khi handler của nó tồn tại.

Publish mới là nơi từ chối type lạ, đối chiếu với một **registry loại task do W1 giới thiệu**. Registry
đó khai báo và không mang hành vi: với mỗi `type` đã biết, nó ghi các trường được chấp nhận và trường
nào là tham chiếu tới object của egret (§8). Handler của W3 sau đó được viết dựa trên nó.

Registry buộc phải nằm ở W1 chứ không phải W3, nếu không gói này tự đánh bại chính nó: không có
registry thì mọi loại task egret đều lạ, nên không gì publish được và W1 sẽ giao một route publish luôn
từ chối. Đây cũng chính là mối nối §13.3 cảnh báo — hai nửa có thể lệch nhau — và đó là lý do nửa khai
báo đi trước, nửa hiện thực sau bắt buộc phải khớp nó.

## 5. `Property<T>`

`PropertyDeserializer` của Kestra lưu **mọi thứ thành chuỗi**: mảng và object bị serialize lại thành
JSON text, giá trị vô hướng thành dạng chuỗi của nó. Nên `Property<T>` bản chất là "nguồn thô, render và
ép kiểu lúc dùng", và bản port Rust là một newtype trên nguồn đó:

```rust
pub struct Property<T> {
    source: String,
    _target: PhantomData<T>,
}
```

W1 định nghĩa nó và hành vi serde. **W2 cho nó `render`.** Tách như vậy chính là thứ cho phép mô hình
tới trước engine biểu thức, và là lý do identity-render của plugin host W5 mạch lạc: một `Property` mà
nguồn không chứa `{{ }}` thì render ra chính nó.

## 6. Namespace, workspace và folder

`namespace` của Kestra là `@NotNull`, `^[a-z0-9][a-z0-9._-]*`, dài 1–150. Từ chối nó là phá luật trung
thành ở chỗ dễ thấy nhất — mọi flow Kestra trên đời đều có một cái.

- egret **bắt buộc** `namespace` với đúng pattern và độ dài của Kestra.
- Khoá duy nhất là `(workspace_id, namespace, id)` — chính là `(tenant, namespace, id)` của Kestra với
  workspace đứng vào chỗ tenant.
- `folder_id` vẫn là trục tổ chức của **giao diện**, đúng như `dataapi.endpoints` dùng, và cố ý **không**
  suy ra từ `namespace`: folder bị đổi tên và chuyển cha liên tục, còn namespace là một phần danh tính
  của flow.

Hai trục tổ chức là một cái giá thật, và đó là giá của việc dán flow Kestra vào chạy được.

## 7. Schema

```sql
CREATE TABLE workflow.flows (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id    uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  folder_id       uuid REFERENCES analytics.folders(id) ON DELETE SET NULL,
  namespace       text NOT NULL,
  flow_id         text NOT NULL,          -- `id` trong YAML; `id` đã dành cho khoá thay thế
  source          text NOT NULL,          -- YAML tác giả viết, nguyên từng byte
  plan            jsonb NOT NULL,         -- mô hình đã parse và chuẩn hoá
  state           text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft','live','disabled')),
  live_version_id uuid,
  created_by      uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at      timestamptz NOT NULL DEFAULT now(),
  updated_at      timestamptz NOT NULL DEFAULT now(),
  deleted_at      timestamptz,
  UNIQUE (workspace_id, namespace, flow_id)
);

CREATE TABLE workflow.flow_versions (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  flow_id     uuid NOT NULL REFERENCES workflow.flows(id) ON DELETE CASCADE,
  version     integer NOT NULL,
  snapshot    jsonb NOT NULL,   -- source + plan chuẩn hoá + tham chiếu ĐÃ GIẢI, tự chứa
  note        text NOT NULL DEFAULT '',
  created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (flow_id, version)
);

ALTER TABLE workflow.flows
  ADD CONSTRAINT flows_live_version_fk
  FOREIGN KEY (live_version_id) REFERENCES workflow.flow_versions(id) ON DELETE SET NULL;
```

Draft giữ **cả** `source` lẫn `plan`. `source` là thứ editor hiển thị và round-trip nguyên byte; `plan`
là thứ validate (và về sau, thi hành) đọc. Chỉ giữ `source` thì phải parse lại ở mỗi lần đọc; chỉ giữ
`plan` thì mất comment và định dạng của tác giả.

Cố ý **không** có trong W1: `workflow.executions`, `workflow.task_runs`, `workflow.triggers`,
`workflow.task_logs`. Chưa gì chạy, và một bảng không có người ghi là bề mặt chưa được kiểm.

**Đo ngày 2026-09-14, vì chữ "hoãn" dễ bị đọc thành "sau đó đã có", mà với hai
bảng trong số này thì không:** `workflow.executions` và `workflow.task_runs` đã
có ở migration 0134, còn **`workflow.task_logs` và `workflow.triggers` đến nay
vẫn không tồn tại trong bất kỳ migration nào.** Chúng là **W11** và **W12** ở §9
của spec chương trình, mỗi cái được ghi ở đó kèm phép đo.

## 8. Publish, và mối nối bảo mật

**Mọi tham chiếu từ flow tới object của egret đều được giải và kiểm workspace tại publish, rồi đóng băng
vào snapshot. Artifact đã publish không chứa id chưa giải nào.**

Đây là luật §6.2 của spec chương trình đặt ra, và W1 là nơi nó được hiện thực. Tham chiếu mà một flow
v1 có thể tạo:

| Trong YAML | Giải theo | Port |
|---|---|---|
| `database:` trong spec của task lá | `query.databases` | cạnh `QueryExecPort`; cần một `workspace_of` |
| `notebook:` | notebook trong `analytics` | `NotebookDocsPort` |
| `dataset:`, `chart:` | `analytics` | các port analytics sẵn có |
| `endpoint:` của Data API | `dataapi.endpoints` | `DataApiEndpointPort::workspace_of` |
| `subflow:` | `workflow.flows` | chính crate này |

Mỗi cái trả lời bằng workspace **của chính object**, và publish so nó với workspace của người gọi, từ
chối khi lệch bằng `NotFound` — không phân biệt được với "không tồn tại", nên publish không dùng để dò
xem object nào tồn tại ở workspace khác. Đó là quy ước mà `DataApiEndpointPort::workspace_of` đã ghi
sẵn, và là lý do nó tồn tại.

**Một flow trỏ sang workspace khác là thứ không publish nổi**, thay vì thứ mỗi handler phải nhớ từ chối
về sau. Migration 0128 là ca đã ghi lại của lối kia.

Vì spec của task lá là mờ trong W1, việc trích tham chiếu được lái bởi một **schema tham chiếu khai báo
theo từng loại task đã biết**, không phải bằng cách quét JSON bất kỳ tìm chuỗi hình dạng uuid. Một loại
task lạ không đóng góp tham chiếu nào và không publish được — xem §4.2.

## 9. Validate

Ba tầng, theo thứ tự, và thứ tự chính là thiết kế:

1. **Parse** — YAML → mô hình. Lỗi là lỗi cú pháp kèm dòng/cột.
2. **Cấu trúc** — id task duy nhất trong flow; không có chu trình trong `Dag`; handler `errors:`/
   `finally:` không bị tham chiếu như node kế tiếp thông thường; `subflow` không tự gọi chính nó; mọi
   `dependsOn` đều gọi tên một task có thật.
3. **Ngữ nghĩa** — pattern `namespace`; id input duy nhất; `Switch` vét cạn hoặc có nhánh mặc định; các
   tính năng đã khai báo nhưng chưa hiện thực (`concurrency`, `sla`, `checks`, `quotas`, `pause`) báo
   thành **cảnh báo có nêu tên gói sẽ hiện thực chúng**, không bao giờ nhận vào im lặng.

> **`sla` đã được THU HẸP ngày 2026-09-14 (W8 Task 6), còn `pause` thì đã xong từ W8 Task 3.**
> `EXECUTION_ASSERTION` của `SLA.Type` nay ĐƯỢC đánh giá (`egret-workflow/src/sla.rs`);
> `MAX_DURATION` thì chưa. Cảnh báo chỉ phát cho luồng có khai `MAX_DURATION`, không
> phát cho luồng mà `sla:` chỉ gồm assertion — cảnh báo về một tính năng ĐÃ được thi
> hành gây thiệt cho người dùng y hệt việc nhận vào im lặng, chỉ theo chiều ngược lại.

Kênh cảnh báo của tầng 3 quan trọng hơn vẻ ngoài của nó: không có nó, người dùng viết
`concurrency: {limit: 1}`, thấy publish xanh, và kết luận hợp lý rằng nó đang được thi hành.

## 10. Route

Phạm vi workspace, dưới `/api/ws/{ws}/workflows`, được entitlement `workflows` mà W0 đăng ký claim:

```
GET    /flows                     danh sách (lọc theo folder, namespace, state)
POST   /flows                     tạo draft từ YAML
GET    /flows/{id}                draft: source + plan + báo cáo validate
PUT    /flows/{id}                cập nhật draft
DELETE /flows/{id}                xoá mềm
POST   /flows/{id}/validate       parse + validate mà không lưu
POST   /flows/{id}/publish        giải tham chiếu, đóng băng một version, đặt live
GET    /flows/{id}/versions       danh sách version
GET    /flows/{id}/versions/{v}   một snapshot
```

`POST /validate` là route riêng vì editor gọi nó mỗi lần người dùng ngừng gõ, và nó không được đòi hỏi
một draft đã lưu.

## 11. Bố cục crate

```
egret-workflow/
  models/
    flow.rs          Flow, Label, Concurrency, Output, SLA, Check, Quota
    task.rs          Task, TaskKind, các kiểu control-flow
    input.rs         Input + từ vựng kiểu đã port
    property.rs      Property<T>
    execution.rs     Execution, TaskRun          (định nghĩa ở đây, sinh ra ở W3)
    trigger.rs       AbstractTrigger + các kiểu trigger
    state.rs         (W0)
  spec/
    parse.rs         YAML -> Flow
    validate.rs      ba tầng của §9
  repo.rs            flows + flow_versions
  publish.rs         giải tham chiếu + đóng băng snapshot
  routes.rs
```

`models/` và `spec/` không chạm database cũng không chạm mạng, theo §6.5 của spec chương trình.

## 12. Ngoài phạm vi

Không executor, scheduler, handler, hay đánh giá biểu thức. Không bảng `workflow.executions`/
`task_runs`. Không frontend — W7 sở hữu phần đó; route của W1 được kiểm bằng test và bằng `curl`.

## 13. Câu hỏi mở

1. `namespace` cho egret một trục tổ chức thứ hai bên cạnh folder. Đã xác nhận đây là giá của việc dán
   flow Kestra vào chạy được, nhưng giao diện ở W7 phải làm nó dễ hiểu chứ không gây rối.
2. Từ vựng `inputs:` của Kestra chồng lấn `ParamType` của egret mà không trùng khớp. W1 port bộ của
   Kestra. Hai bên có bao giờ hội tụ hay không là câu hỏi cho người tiếp theo đụng vào tham số Data API,
   và nó nên được trả lời có chủ đích chứ không phải bằng trôi dạt.
3. Trích tham chiếu cần một schema khai báo theo từng loại task (§8). Registry đó ra đời ở W1 với các
   mục cho loại task v1, và handler của W3 buộc phải giữ đồng bộ với nó — mối nối là chỗ hai nửa có thể
   lệch nhau, đúng dạng hỏng mà chương trình này đã gặp hai lần.
