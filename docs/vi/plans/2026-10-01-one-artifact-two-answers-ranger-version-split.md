# Một artifact, hai câu trả lời: Ranger 2.5.0 và 2.8.0 thi hành một tag deny khác nhau

Ngày: 2026-10-01
Bản tiếng Anh là bản chuẩn:
[`docs/en/plans/2026-10-01-one-artifact-two-answers-ranger-version-split.md`](../../en/plans/2026-10-01-one-artifact-two-answers-ranger-version-split.md).
Tài liệu này là bản tham chiếu; khi hai bản khác nhau, bản tiếng Anh đúng.

Đo bởi: differential gate của SP4d-2b
(`crates/egret-ranger-eval/tests/differential.rs` +
`tests/ranger/src/test/java/EgretCaseRunner.java`), chạy trên
`RangerPolicyEngineImpl` THẬT từ `ranger-plugins-common` 2.5.0 và 2.8.0. Review
Task 5 đã tái lập độc lập, đúng từng dòng, kể cả `policyId`.

## Phát hiện, một câu

**egret phát MỘT bản policy download cho cả hai engine, và với một tag DENY cộng
một resource ALLOW ở priority CAO HƠN HẲN, hai engine trả về câu trả lời NGƯỢC
NHAU** — Trino (Ranger plugin 2.5.0) DENY, StarRocks (2.8.0) ALLOW — và không có
gì trong egret nói cho steward biết điều đó.

Đây không phải lỗi của compiler egret, cũng không phải lỗi của
`egret-ranger-eval`: cả hai phía đều đúng và bản download là byte-identical. Đây là
khác biệt hành vi giữa hai phiên bản plugin Apache Ranger mà engine của egret đang
chạy.

## Đã đo gì

Một bản download tối thiểu, gửi byte-identical tới cả hai engine. Tag ACCESS policy
(id 900) deny `select` cho `bob` ở `policyPriority: 0`; một resource ACCESS policy
(id 1) allow `select` cho `bob`; request `hive.sales` của `bob`; tag `pii` gắn ở
`SELF`.

| resource allow | có deny items trên nó? | Ranger 2.5.0 (Trino 470) | Ranger 2.8.0 (StarRocks 4.1) |
|---|---|---|---|
| `policyPriority: 1` | không | **Denied**, `policyId: 900` (TAG policy) | **Allowed**, `policyId: 1` (resource policy) |
| `policyPriority: 2` | không | **Denied**, `policyId: 900` | **Allowed**, `policyId: 1` |
| `policyPriority: 1` | có, không khớp ai | Denied, `policyId: 1` | Denied, `policyId: 1` |
| `policyPriority: 0` | không | Denied, `policyId: 900` | Denied, `policyId: 900` |
| (không có khối `tagPolicies`) | không | Allowed, `policyId: 1` | Allowed, `policyId: 1` |

Dòng 3, 4 và 5 GIỐNG nhau giữa hai phiên bản. Chỉ riêng allower ở priority cao hơn
hẳn và không deny-capable là khác — chính điều đó làm đây là một khác biệt CỤ THỂ,
không phải nhận xét chung "hai phiên bản khác nhau".

## Chỗ tệ hơn cả một khác biệt phiên bản

`RangerPolicyEngineImpl.evaluatePoliciesNoAudit` ghi log, ở **offset 224 trong CẢ
HAI jar**:

> `access denied by a tag policy. Higher priority resource policies will be
> evaluated to check for allow`

Nghĩa là 2.5.0 *tự nói* hành vi của 2.8.0 trong log của chính nó và KHÔNG làm điều
đó. Ai đọc dòng log ấy — hoặc đọc tài liệu Ranger, hoặc đo trên StarRocks rồi khái
quát — sẽ kết luận rằng một override grant ở priority cao hơn mở lại được một tag
deny. Trên Trino thì không.

**Cơ chế thì CHƯA xác định được.** Ứng viên hiển nhiên,
`RangerDefaultPolicyEvaluator.evaluatePolicyItems` với map `accessTypeResults`, bị
loại: method đó gần như giống hệt nhau giữa hai jar ở vùng liên quan (khác biệt duy
nhất trong cả method là hai nhánh `ignoreDescendantDeny` và signature của
`getCompositeAccessResult`). Vậy còn một cơ chế thứ hai chưa được mô hình hoá trong
`RangerPolicyEngineImpl`, mà diff đã chuẩn hoá giữa hai jar dài ~210 dòng
(`RangerReadWriteLock`, inline policies, `RangerAccessRequest.ignoreDescendantDeny()`,
và `getServiceDefHelper().getAllAccessTypes()` thay cho
`getServiceDef().getAccessTypes()`).

## Ý nghĩa với sản phẩm

1. **Một clearance (tag deny) cộng một override grant KHÔNG portable giữa hai
   engine.** Đúng cái động tác quản trị mà steward hay dùng — "deny tag này ở mọi
   nơi, trừ bảng này thì grant lại ở priority cao hơn" — thi hành được trên
   StarRocks và KHÔNG thi hành được trên Trino. Cùng những dòng trong
   `policy.authz_policies`, quyền truy cập khác nhau.
2. **Đo trên một engine rồi khái quát là KHÔNG an toàn với hình dạng này.** Mọi
   phép đo live trong loạt SP4d tới giờ đều làm trên một engine mỗi lần. Đây là
   hình dạng đầu tiên tìm thấy mà như thế là chưa đủ.
3. **egret không có mặt UI hay tài liệu nào nói điều đó.** Thứ duy nhất nói hiện
   nay là `egret-ranger-eval`, nó TỪ CHỐI trả lời hình dạng này trên plugin 2.5.0
   (`UndecidableCause::TagDenyReopenedDiffersByPluginVersion`), nên trang `explain`
   hiện cho steward thấy "engine này chạy ranger 2.5.0, nó giữ tag deny ở đây trong
   khi 2.8.0 sẽ mở lại". Đó là một từ chối đúng đắn trong một công cụ, không phải
   một câu trả lời ở mức sản phẩm.

## Đã làm gì, và CHƯA làm gì

**Đã làm (SP4d-2b Task 5):**

* `egret-ranger-eval` tái lập nhánh 2.8.0 và TỪ CHỐI trên 2.5.0 cũng như khi không
  biết phiên bản plugin, thay vì báo câu trả lời của engine này cho engine kia. Từ
  chối đó mang theo cả hai policy id và phiên bản plugin.
* `explain` khai báo `EngineType::plugin_version()` để từ chối được giới hạn đúng
  vào engine thật sự khác biệt, và câu trên trang có nêu tên phiên bản.
* Corpus differential sinh ra hình dạng này, nên từ chối không thể im lặng biến
  mất.
* `tests/ranger/README.md` giữ phần disassembly, các offset và bảng trên.

**CHƯA làm, và cố ý ngoài phạm vi một task về test harness:**

* **Chưa ai quyết định egret NÊN LÀM GÌ.** Các lựa chọn, chưa chọn cái nào: từ chối
  compile (chặn override grant đặt trên một tag deny với profile Trino); cảnh báo
  lúc compile; compile khác nhau theo engine; hoặc chấp nhận khác biệt và ghi tài
  liệu cho steward. Việc này cần một quyết định sản phẩm, không phải một thay đổi
  crate.
* **Không có mặt nào cảnh báo steward lúc soạn policy.** Từ chối chỉ xuất hiện
  trong `explain`, nơi steward chỉ tới nếu họ chủ động đi tìm.
* **Nhánh 2.5.0 chưa được mô hình hoá**, nên `explain` trên Trino từ chối thay vì
  trả lời. Việc mô hình hoá là mục tiếp theo số **0** trong `tests/ranger/README.md`,
  và nó cần đúng phần audit nêu ở đó.

  ⚠ **Sửa 2026-10-01 bởi review toàn nhánh.** Bản tiếng Anh trước đây còn nói đường
  resource "mô hình một cơ chế khác cho cùng cái map và xanh trên 4000 case", kèm lưu
  ý đó là quan sát chứ không phải chứng minh. Quan sát ấy ĐÃ BỊ PHỦ ĐỊNH: cái xanh đó
  nằm trên một tập hợp KHÔNG chứa hình dạng gây lệch (corpus ép mọi resource ACCESS
  policy ngẫu nhiên về priority 0), và chỗ lệch — một tag ALLOW dưới một resource
  allow ở priority cao hơn hẳn — là một câu trả lời DENY SAI và chắc chắn, trên cả hai
  engine. Giờ nó TỪ CHỐI (`AccessPriorityCompositeNotModelled`), và phần audit được
  nâng từ mục 2 lên mục 0 vì điều đó.

## Cách tái lập

```bash
cd tests/ranger
# cases.json: bảng năm dòng ở trên, theo protocol của EgretCaseRunner
mvn -o test -Dranger.version=2.5.0 -Degret.cases=/tmp/cases.json -Degret.verdicts=/tmp/v250.json
mvn -o test -Dranger.version=2.8.0 -Degret.cases=/tmp/cases.json -Degret.verdicts=/tmp/v280.json
# verdict khác nhau ở dòng 1 và 2, khác cả isAllowed LẪN policyId
```

Tuyệt đối không truyền `-q`: nó chặn dòng tổng kết per-class của surefire, và output
rỗng không phải là pass.
