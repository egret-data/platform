# SP4a — mô hình quyền dữ liệu của egret, và trình biên dịch của nó

**Trạng thái:** đã thiết kế
**Ngày:** 2026-09-24
**Mở rộng:** `2026-09-15-control-plane-dg-review-programme.md` §2.3 và §3 (SP4), trên nền SP3 (#587–#593)
**Triển khai thành ba PR:** SP4a mô hình và compiler, SP4b UI, SP4c di trú.

SP3 biến `registry.catalogs` thành nơi duy nhất định nghĩa một catalog. SP4 tạo ra nơi duy nhất định
nghĩa *quyền truy cập vào nó*. Spec này là SP4a: mô hình, store, và trình biên dịch biến nó thành
Ranger JSON mà các engine vốn đã thực thi.

## 1. Những gì code nói mà spec chương trình thì không

Đọc trước khi thiết kế; mỗi điều dưới đây đều làm đổi thiết kế.

- **Không có mô hình permission nào để thay thế cả.** `policy.authz_policies` lưu **JSON Ranger
  nguyên văn** trong một cột jsonb (`egret-policy/src/store.rs:286`), và "profile" chỉ là một dòng
  `policy.authz_services` — không có bảng riêng, không có artefact nào được render. Editor hiện nay
  (`PolicyEditorPage.tsx`, 954 dòng) là một cái form trên Ranger JSON. SP4a không phải là di trú giữa
  hai mô hình; nó là mô hình **đầu tiên**, với Ranger JSON bị giáng xuống thành đầu ra.
- **Tag truyền tới engine chỉ mang cái tên.** `ServiceTags::from_assignments`
  (`egret-policy-api/src/dto.rs:142-205`) phát ra `{"type": <TÊN tag>}`; cái mask thực thi nó là một
  policy *riêng* viết theo tên đó. Đó là lý do đổi tên tag làm gãy enforcement một cách âm thầm, và
  là lý do mô hình phải trỏ bằng `tags.key`.
- **Tag chỉ gắn được vào table và column.** `catalog_tag_assignments` khoá theo
  `(tag_id, warehouse_id, schema_name, table_name, column_name)` (`migrations/0081`), với
  `column_name = ''` nghĩa là mức table. Không có tag mức catalog hay schema, nên một selector theo
  tag không thể mang nghĩa "mọi thứ trong catalog này".
- **StarRocks mask và filter được; nó chỉ thiếu tag service.** Service-def của nó khai báo
  `dataMaskDef` (6 kiểu mask) và `rowFilterDef`
  (`egret-policy/src/service-defs/starrocks.json`), trong khi `service_def.rs:22-24` ghi rằng plugin
  không hỗ trợ tag (upstream `starrocks#67458`) và `tag_service` chỉ được tính cho Trino
  (`admin/mod.rs:543-553`). Khoảng trống hẹp hơn "StarRocks không được cưỡng chế" và rộng hơn
  "StarRocks ổn rồi" — xem D5.
- **Cấu trúc bốn tầng allow/deny/exception của Ranger tồn tại vì Ranger không có phép trừ tập hợp
  trên principal.** Principal của một policy item là một tập user/group/role không có toán tử trừ,
  nên "nhóm finance trừ contractor" không diễn đạt được cách nào khác
  ([Ranger deny-conditions and excludes](https://cwiki.apache.org/confluence/display/RANGER/Deny-conditions+and+excludes+in+Ranger+policies)).
  Cho mô hình một phép trừ là hai trong bốn tầng đó thôi làm khái niệm mà người dùng phải học.
- **Mask và row-filter policy của Ranger không nhận wildcard và không nhận nhiều giá trị**: một cột
  một policy item, item có thứ tự, cái đầu khớp thì thắng, không có deny
  ([row-level filtering and column masking](https://cwiki.apache.org/confluence/display/RANGER/Row-level+filtering+and+column-masking+using+Apache+Ranger+policies+in+Apache+Hive)).
  Có người báo đúng điều này với Trino trên dev@ranger: "chúng tôi chỉ chọn được một catalog, một
  schema, một bảng và một cột mỗi lần"
  ([thread](https://www.mail-archive.com/dev@ranger.apache.org/msg37731.html)). Đây là nguyên nhân
  bùng nổ số policy, và là thứ mà rule-có-tên của D1 sinh ra để tránh.
- **Audit của Ranger đứt đúng tại chỗ bị từ chối.** Tab Access liên kết một quyết định tới policy
  tạo ra nó, nhưng khi không có gì khớp và request rơi xuống default-deny thì cột Policy ID hiện `-`.
  Kiểu từ chối phổ biến nhất lại là kiểu không có gì để bấm vào.
- **Biểu thức mask tuỳ biến là một chuỗi Ranger không bao giờ parse**; nó chết lúc chạy query, trong
  engine ([Cloudera thread](https://community.cloudera.com/t5/Support-Questions/Custom-masking-expression-using-UDF-in-Apache-Ranger/m-p/170283)).

Không dùng làm bằng chứng: con số "Ranger cần nhiều hơn 75 lần số thay đổi policy" được trích dẫn
rộng rãi là do bên bán hàng tài trợ và đã có phản bác công khai có cơ sở (benchmark đó viết mỗi user
một policy thay vì thêm user vào policy sẵn có, và bỏ qua `denyAllElse`). Phần còn đứng vững chỉ là:
lối soạn thảo ngây thơ của Ranger dẫn tới mỗi cột một policy, và UI không hề lái người dùng ra khỏi
lối đó.

## 2. Các quyết định

**D1 — Hai object và một toán tử.** Một **grant** nói principal nào được làm hành động nào trên phạm
vi nào. Một **rule** là mask hoặc row filter *có tên, dùng lại được*, chọn dữ liệu theo tag hoặc theo
đường dẫn, và nêu ai được miễn trừ. Cả hai nhận một **principal set** có phép trừ tường minh:

```text
PrincipalSet { include: [user|group|role|workspace-members …],
               exclude: [user|group|role …] }
```

Phép trừ chính là toàn bộ lý do egret bỏ được allow-exception và deny-exception khỏi bề mặt soạn
thảo. Rule có tên là hình dạng của Snowflake chứ không phải của Ranger: một rule `PII → hash` phủ mọi
cột mang tag đó, thay vì mỗi cột một policy
([Snowflake POLICY_REFERENCES](https://docs.snowflake.com/en/sql-reference/account-usage/policy_references)).

**Hành động** của grant là một bộ từ vựng nhỏ của riêng egret, không phải của engine nào:
`read`, `write`, `create`, `drop`. Compiler ánh xạ từng cái sang access type của engine đích —
`read` thành `select` trên cả hai, `write` thành `insert, update, delete` — bởi hai service-def
không trùng nhau (Trino có `use`, `impersonate`, `read_sysinfo`; StarRocks có `node`, `plugin`,
`usage`). Một steward không nên phải biết bên dưới là engine nào, và một hành động mà engine không
diễn đạt được phải là lỗi biên dịch nêu đích danh engine, chứ không phải im lặng bỏ qua.

**D2 — Deny thắng, allow hợp, và tag không cấp quyền truy cập.** Một câu, đúng cho mọi engine:
*thấy được nếu có grant nào cho phép, trừ khi có grant nào cấm; mask và row filter luôn áp dụng, bất
kể quyền đến từ đâu.* Tập `exempt` của rule là cách một cá nhân thoát khỏi mask — không phải bằng một
grant đối chọi.

Tag chỉ chọn dữ liệu cho rule, không làm gì khác. Đây không phải đơn giản hoá cho vui: Ranger xét tag
policy **trước và đoản mạch** — một tag ALLOW là kết thúc đánh giá, resource policy không bao giờ
được xét tới
([Cloudera, tags and policy evaluation](https://docs.cloudera.com/runtime/7.2.17/security-ranger-authorization/topics/security-ranger-tags-and-policy-evaluation.html)).
Nếu egret không bao giờ phát ra tag *access* policy thì không có đoản mạch, và câu của D2 chính xác
là thứ engine làm. Ngữ nghĩa của mô hình và ngữ nghĩa của bên thực thi khớp nhau **do cấu trúc**, chứ
không phải do tài liệu dặn dò.

**D3 — Gán catalog cho workspace là cấp quyền đọc cho workspace đó.** Hôm nay một lần gán chỉ đổi thứ
egret hiển thị; engine vẫn từ chối query
(`2026-09-19-catalog-registry-design.md:158`). SP4a biến lần gán đó thành một grant đọc ngầm, để một
hành động có một ý nghĩa. **Đây là thay đổi hành vi và phải được phát hành như một thay đổi hành
vi**: sau SP4a, gán catalog là cấp quyền thật. Grant tường minh vẫn còn, để siết lại (một deny) hoặc
nới ra (quyền ghi, hoặc một principal set khác). Grant ngầm hiện ra trong UI như một grant người dùng
không tự viết, có nêu nguồn gốc — không bao giờ vô hình.

**D4 — Profile là generated hoặc manual; generated là chỉ đọc.** Engine do egret provision bind vào
profile generated, biên dịch từ mô hình. Profile viết tay ở lại cho engine ngoài và giữ editor JSON
hôm nay. Spec chương trình đã chốt vị trí này (`2026-09-15-…-programme.md:83`); SP4a biến
`generated | manual` thành một cột tường minh thay vì một quy ước, vì một profile generated mà ai đó
sửa tay sẽ bị ghi đè âm thầm ở lần biên dịch kế tiếp và phải được cảnh báo **trước khi** điều đó xảy ra.

**D5 — egret biên dịch lại StarRocks khi tag đổi; trên Trino thì plugin lo.** Một rule chọn theo tag
biên dịch thành một tag-masking policy trên Trino, và plugin áp nó lên bất cứ thứ gì đang mang tag.
StarRocks không có tag service, nên cùng rule đó biên dịch thành **một resource-masking policy cho
mỗi cột khớp**, liệt kê tại thời điểm biên dịch — và danh sách đó lỗi thời ngay khi có cột mới được
gắn tag. egret vốn đã bump service bị ràng buộc ở mỗi lần ghi tag
(`egret-catalog-registry/src/tag_version.rs:54`), nên egret biên dịch lại profile StarRocks theo đúng
trigger đó. Việc `ranger-tagsync` làm cho Trino, egret làm cho StarRocks.

Cái giá là có thật và phải chặn: một rule trên một tag được gắn rộng sẽ thành N policy trên StarRocks.
SP4a áp một **trần tại thời điểm biên dịch** cho số policy bung ra trên mỗi rule, **từ chối** biên
dịch khi vượt trần kèm thông báo nêu tên rule và con số, và **không bao giờ cắt bớt âm thầm**. Giá trị
của trần được đặt từ một phép đo trên kind, không chọn trước.

**D6 — Rule trỏ `tags.key`, grant trỏ `registry.catalogs.id`.** Cả hai đều vá chỗ đã được ghi nhận là
gãy: tag policy hôm nay lưu `tags.name` vốn thay đổi được và không duy nhất (`programme spec:43`), và
danh tính catalog giờ nằm ở registry. Compiler phân giải key → name và id → name lúc biên dịch, nên
một lần đổi tên dẫn tới biên dịch lại chứ không dẫn tới gãy.

**D7 — Một dạng principal chuẩn duy nhất, và hỗ trợ nhóm phải được kiểm chứng chứ không giả định.**
Principal hôm nay chỗ thì viết bằng email chỗ thì username (`programme spec:44`), và chính spec đó
*suy đoán* — không kiểm chứng — rằng không có group provider hay usersync nào, nên rule theo nhóm có
thể chưa bao giờ khớp bên trong engine. SP4a chọn một dạng, và **chứng minh bằng một query sống trên
kind** xem grant theo nhóm có được cưỡng chế không. Nếu không, UI phải nói ra ngay chỗ chọn nhóm; một
điều khiển im lặng không làm gì còn tệ hơn một điều khiển không tồn tại.

Điều này quyết định luôn cách grant ngầm của D3 biên dịch, và hai thứ phải được chốt cùng nhau. Nếu
nhóm phân giải được bên trong engine, `workspace-members` biên dịch thành một group và đổi thành viên
không cần biên dịch lại. Nếu không, nó biên dịch thành **danh sách thành viên liệt kê lúc biên
dịch**, và thêm một người vào workspace bắt buộc phải biên dịch lại profile — đúng hình dạng lỗi thời
của D5, với cùng một lời giải: egret sở hữu phép biên dịch, nên egret biên dịch lại khi có lần ghi
thành viên. Spec không thể chọn giữa hai phương án trước khi kiểm chứng, và plan của SP4a phải coi
phép kiểm chứng đó là **việc mở khoá phần còn lại**, không phải một ô tick ở cuối.

**D8 — Evaluator được xây trong SP4a, để giải thích, chưa để thực thi.** Câu "vì sao user này xem
được?" của SP4b cần một quyết định *kèm xuất xứ*, và nguồn trung thực duy nhất cho điều đó là egret
tự đánh giá mô hình của chính nó. Cũng chính evaluator đó là thứ SP5 cần để cưỡng chế trên engine
không có plugin Ranger. Xây một lần ở đây còn cho compiler một oracle: một profile đã biên dịch có
thể được đối chiếu với câu trả lời của chính mô hình, thay vì đối chiếu với một snapshot JSON.

Đầu ra của nó nêu **lớp lý do**, không chỉ một policy id: `allowed_by_grant`, `explicit_deny`,
`no_grant_matched`, `masked_by_rule`, `filtered_by_rule`. Đây là câu trả lời trực tiếp cho khoảng
trống audit của Ranger, nơi một lần từ chối do rơi-xuyên-qua không có gì để liên kết tới.

**D9 — Những gì cố ý không mô hình hoá.** Mỗi thứ kèm lý do, vì "Ranger có" không phải là một lý do:

| Bỏ | Vì sao |
|---|---|
| Policy condition (biểu thức JavaScript) | Service-def của Trino không khai báo `policyConditions` nào cả, nên chúng không tồn tại trên engine chính của egret. Mở lại nếu có ca thật. |
| Delegated admin (ô tick trên từng policy item) | egret đã có hệ thống permission riêng (Cedar) và mô hình quản trị ba tầng. Một mặt phẳng admin thứ hai, theo từng policy, không được hoà giải với cái thứ nhất — đó đúng là cách Ranger có tới ba mặt phẳng. |
| Security zone | Chúng tồn tại để chia một bản cài Ranger giữa các nhóm. egret đã chia theo workspace. |
| Lịch hiệu lực và điều kiện IP | Chưa có ca nào. Quyền có hạn thời gian nên được diễn đạt bằng một grant mà ai đó gỡ đi, hơn là một policy hết hạn một cách vô hình. |
| Allow-exception / deny-exception như khái niệm **soạn thảo** | Đã được thay bằng toán tử trừ (D1). Chúng vẫn tồn tại như **đích biên dịch** — vô hình trong UI. |

## 3. Lưu trữ

Một schema `perm` mới, đặt cạnh `policy` (nơi tiếp tục phục vụ Ranger JSON thô cho profile manual).

| Bảng | Chứa |
|---|---|
| `perm.grants` | `effect`, `actions[]`, phạm vi: `catalog_id` → `schema`, `table`, `column` (cho phép null và wildcard), `origin` (`explicit` \| `workspace_assignment`) |
| `perm.grant_principals` | `(grant_id, kind, ref, mode)` với `mode ∈ {include, exclude}` |
| `perm.rules` | `name` UNIQUE, `kind` (`mask` \| `row_filter`), `body`, `selector_kind` (`tag` \| `path`), `tag_key` hoặc các cột đường dẫn |
| `perm.rule_exemptions` | principal set miễn trừ của rule, cùng hình dạng với `grant_principals` |
| `perm.compiled` | chỉ mục ngược: object egret nào sinh ra policy nào trong profile nào |

`perm.compiled` là mảnh Ranger không có thứ tương đương. Nó trả lời "rule này thực sự đang áp ở đâu",
và từ đầu kia, "cái gì sinh ra policy này" — câu một người vận hành hỏi khi một profile generated làm
họ bất ngờ. Nó cũng là cách một lần biên dịch biết phải xoá gì: biên dịch lại thì **diff** với bảng
này thay vì xoá sạch rồi tạo lại profile, nên một policy id mà engine đang cache không bị xáo trộn.

## 4. Biên dịch

Một chiều duy nhất: mô hình → Ranger JSON. Không có gì đọc ngược Ranger JSON về mô hình.

- **Grant** → resource access policy. Phần `exclude` của principal set biên dịch thành
  `allowExceptions` của Ranger (hoặc `denyExceptions` với grant deny) — cấu trúc bốn tầng sống sót
  như một chi tiết cài đặt và không xuất hiện ở bất kỳ đâu trong API hay UI của egret.
- **Rule `kind = mask`** → trên Trino, một tag-masking policy cho mỗi rule khi selector là tag; một
  resource-masking policy cho mỗi cột khi selector là đường dẫn. Trên StarRocks, luôn là theo cột (D5).
- **Rule `kind = row_filter`** → row-filter policy, chia theo cùng cách.
- **Thân mask** chọn từ `maskTypes` mà engine khai báo, không phải chuỗi tự do, ở mọi chỗ mà ý định
  diễn đạt được như vậy. Biểu thức tuỳ biến vẫn được phép nhưng **được parse và bị từ chối ngay lúc
  soạn thảo nếu không parse được** — điểm mấu chốt là Ranger không bao giờ parse nó và tác giả chỉ
  phát hiện lúc chạy query.

**Compiler là ranh giới an toàn.** Đầu ra của nó là thứ engine cưỡng chế, nên SP4a kiểm nó trên một
engine sống chứ không chỉ trên JSON kỳ vọng: một mask mà Ranger chấp nhận vẫn có thể hỏng trong
Trino, và một snapshot test sẽ báo xanh trong khi dữ liệu rò ra ngoài.

## 5. SP4b và SP4c thừa kế những gì

SP4a cố ý không kèm UI mới. Nó để lại:

- mô hình, store và API của nó;
- compiler, và profile generated bind vào engine do egret provision;
- evaluator, và một endpoint `explain` trả về quyết định + xuất xứ + lớp lý do;
- `perm.compiled`, chỉ mục ngược mà cả hai pha sau đều đọc.

**SP4b** xây UI hai tầng: mặc định là ngôn ngữ ý định (steward nói "email là PII; analyst thấy bản
hash"), kèm một lối xem cấu trúc cho người vận hành cần nhìn grant và rule như object. Nó thêm hai
động từ — *giải thích một quyết định* và *liệt kê toàn bộ* (ai thấy được cái này / user này thấy được
gì) — vốn là hai câu hỏi khác nhau và là hai màn hình khác nhau
([GCP Policy Intelligence](https://cloud.google.com/bigquery/docs/troubleshoot-access-control)).

**SP4c** di trú: policy viết tay đang có trên engine do egret provision được đọc lên và **đề xuất**
thành object của mô hình (không bao giờ tự chuyển đổi âm thầm); Cedar classification clearance — hôm
nay là hệ thống thứ ba, chỉ chặn được object BI và không mask nổi một cột catalog
(`programme spec §2.3`) — gộp vào rule; và notebook role cùng namespace theo workspace mà SP3 hoãn lại
(`2026-09-19-catalog-registry-design.md:158`) hạ cánh ở đây.

## 6. Kiểm thử

- Một golden test cho compiler theo từng loại rule và từng engine — cần, nhưng chưa đủ.
- **Sống trên kind, cả hai engine**: một grant, một deny, một tag mask và một row filter, mỗi cái
  được kiểm bằng cách chạy query với một principal thật và xem thứ trả về. Một mask sinh ra JSON đúng
  mà không mask chính là lỗi mà phép kiểm này bắt được.
- **Evaluator và engine phải đồng ý với nhau.** Với mỗi ca sống, `explain` phải trả về đúng quyết
  định mà engine đã đưa ra. Bất đồng là lỗi ở compiler hoặc ở evaluator, và muốn tìm ra thì cần cả
  hai — đó là lý do thứ hai khiến D8 đặt evaluator vào SP4a.
- Một test rằng trần bung policy của StarRocks **từ chối** chứ không cắt bớt, và assert rằng thông
  báo nêu tên rule cùng con số.
- Một test rằng đổi tên tag không làm đổi enforcement (D6), và đổi tên catalog thì biên dịch lại.
- Phép kiểm chứng hỗ trợ nhóm của D7, ghi lại như một kết quả sống kèm ngày — không phải như một lời
  khẳng định.

## 7. Ngoài phạm vi

UI (SP4b). Di trú policy đang có, gộp Cedar clearance, và notebook role (SP4c). Cưỡng chế trên engine
không có plugin Ranger (SP5) — evaluator xây ở đây là nền của việc đó, nhưng không có gì trong SP4a
viết lại SQL. Nạp audit của Ranger; log truy cập dữ liệu của riêng egret được nêu tên cho SP4/SP5
trong spec tách app governance và không được thử ở đây.
