# Governance layer SP3 — enforcement points thay profile (thiết kế)

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-12-gov-sp3-enforcement-points.md) — mọi giá trị/tên bảng/chi tiết chính xác ở bản tiếng Anh; khi khác nhau thì bản tiếng Anh đúng.

Trạng thái: thiết kế, 2026-11-12. Sub-project 3 của chương trình governance-layer, nằm trong [concepts spec](2026-11-02-governance-layer-concepts.md) (D1–D10). Trước đó: SP1 neutral IR (#833), SP2 Trino OPA (#836/#838/#839/#844).

## Mục tiêu

Biến "profile" (hôm nay = Ranger authz service) thành **enforcement point (EP)** first-class, và loại bỏ input enforcement phi-model cuối cùng — **manual (operator) policies** — bằng cách type hóa cái chúng diễn đạt. Hợp nhất clearance, role principals và implicit workspace grant vào model chung để **groups-only clearance được thực thi trên mọi engine**. Không thêm switch runtime mới; đây là **data migration tại chỗ, reversible, per-EP**.

Acceptance (bảng concepts, dòng 3): mọi EP trên demo stack chuyển sang model mới; không manual policy; groups-only clearance chạy trên Trino + StarRocks.

## Ranh giới scope (đã chốt với user)

- SP3 **giữ nguyên vocab grant read/write** + `exclude→deny`. Vocab đầy đủ (`BROWSE/CREATE/MANAGE/OWNER`), gộp catalog grants và EP `iceberg-rest` là **SP5**. SP3 không thêm data privilege mới.
- SP3 lo **mặt EP (output)**: giết manual policy, unify clearance/role/workspace, dời catalog scope + engine privileges lên EP, thêm key image-patch-level cho matrix.
- Không switch "old/new model" runtime mới (switch per-EP duy nhất vẫn là `mechanism` ranger/opa của SP2). Migration là **tại chỗ**, reversible qua `perm.migrated_from`, gate per-EP = "không còn operator policy chưa chuyển được".

## Hiện trạng (inventory main 2026-11-12)

EP đã tồn tại một phần (giúp SP3 gọn): `authz_services` đã có `type`/`mechanism`/`origin`; `engine_grants` = **8 capability đã typed** (catalogue); catalog scope derive từ `engine_catalog_attachments`; `principal_kind` có `role`; clearance ở `classification_policies` riêng; version đang **per-engine** (`ranger_plugin_version`), **chưa có** image-level (egret.N); manual policy = `authz_policies managed_by='operator'` (JSON Ranger thô). Vậy SP3 là "hoàn thiện EP + giết manual + unify", không phải "xây EP từ đầu".

## Các gói

### SP3a — Khung EP + version per-EP + matrix key image
- Rename profile→EP **chỉ ở bề mặt** (UI/API/IR); **giữ bảng `authz_services`** (không migration đổi tên bảng lõi).
- Matrix key per-EP = `(engine, mechanism, plugin_version, egret_image_level)`. `egret_image_level` đọc từ image engine đang chạy.
- **Key này để làm gì:** diễn đạt capability phụ thuộc image fork StarRocks. Vd mask/row-filter mà image fork enforce qua mọi protocol (patch 0002/0003/0004) còn image official/cũ thì không → dưới mức bật thì matrix **lower to deny** (fail-closed), từ egret.4+ thì enforce. Corpus/demo đều trên image fork hiện tại nên output **byte-identical**; chỉ đổi với engine trên image không enforce được (vốn đã bị cấm bởi luật "chỉ image fork").
- **① Mixed-engine EP:** EP phục vụ nhiều engine khác version → lấy **capability thấp nhất trên toàn bộ engine** (fail-closed), EP hiện engine nào kìm. Không từ chối tập mixed.
- **Done:** matrix keyed theo image-level; EP dưới mức bật lower mask/row-filter to deny, egret.4+ enforce; UI/API nói "enforcement point"; Ranger JSON byte-identical trên corpus+demo.

### SP3b — Giết manual (operator) policies
- Trang **Migrate** phân loại mỗi `authz_policies` operator (đọc qua `RangerPolicy`): access allow/deny → grant; mask → mask rule; row-filter → row-filter rule; shape trùng engine-capability → typed engine privilege; **không chuyển được** → để nguyên, liệt kê, EP **không hoàn tất migration** khi còn.
- **② Không thêm typed privilege suy đoán.** Chỉ map shape đã biết; thêm typed privilege mới chỉ khi operator-policy thực tế trên demo stack có shape lặp lại (đo lúc viết plan). Mỗi chuyển đổi có provenance + undo.
- **Done:** policy chuyển được đã thành grant/rule/engine-privilege; cái không chuyển liệt kê per-EP; `explain` giữ nguyên answer per-EP.

### SP3c — Unify clearance / role / workspace
- **clearance → `withhold` rule:** migrate `classification_policies` → `perm.rules` (selector = classification, effect = withhold, exemptions = roles(→groups)+groups). Enforcement từ IR → **groups-only clearance chạy Trino + StarRocks**.
- **role → `role:<name> holders` group:** group managed mirror holders; migrate mọi ref `role` trong grant_principals/rule_exemptions/engine_grant_principals. Transitional: sau SP3c không grant mới nào target group này.
- **implicit workspace grant → explicit:** thành grant READ tường minh cho `ws:<slug> members` trên catalog bound, lưu một lần.
- **③** `role:<name> holders` và `ws:<slug> members` dùng **chung cơ chế managed-group** (tái dùng máy ws-members).
- **Done:** 3 input cũ biến mất khỏi enforcement input; groups-only clearance deny trên cả 2 engine (live); `explain` không đổi.

### SP3d — Gate per-EP + switch demo + live
- Gate: EP hoàn tất ⇔ không còn operator policy chưa chuyển (SP3b) + clearance/role/workspace đã migrate (SP3c); re-check mỗi lần compile. Switch mọi EP demo stack sang model mới + live-check từng cái (user thường, engine thật). Sau khi switch hết, xóa input cũ (có thể là follow-up ngắn).

## Lỗi (fail closed)
Capability matrix không biết với image-level → lower to deny "not measured". Operator policy không chuyển → EP không hoàn tất, giữ artifact tốt cuối. Role group không mirror được → group rỗng, member không ở allow nào, vẫn dính mọi deny.

## Verification
Unit test IR/matrix (image-level lowering, clearance-as-withhold group, role group, implicit-grant equivalence); byte-identical SP3a; `explain` equivalence trước/sau mỗi conversion; differential (Ranger 2.5/2.8, OPA vs Ranger) xanh; live check mỗi PR đổi hành vi (groups-only clearance trên cả Trino + StarRocks).

## Ngoài phạm vi SP3
Vocab đầy đủ + gộp catalog grants + EP iceberg-rest (SP5); service accounts / catalog clients (SP4); BI/object permissions, lineage, Spark, OPA cho StarRocks.

## Thứ tự: SP3a → SP3b → SP3c → SP3d (b,c phụ thuộc a; d phụ thuộc b,c).
