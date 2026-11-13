# Governance SP3c-2 — Clearance → rule `withhold` trong model quyền — Kế hoạch

> Bản tiếng Việt tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-13-gov-sp3c-2-clearance-withhold-rule.md) — mọi code/lệnh/signature chính xác ở bản tiếng Anh; khác nhau thì bản EN đúng. Thực thi theo SDD.

**Mục tiêu:** migrate mỗi `classification_policies` thành `perm.rules` kind mới `withhold`, exemption lưu ở `rule_exemptions`, để enforcement clearance đi qua IR như rule thường — và **fix luôn lỗ groups-only clearance biên dịch ra rỗng**.

**Kiến trúc:** thêm `perm.rule_kind='withhold'`. Withhold rule = row tag-selected (không payload mask/filter) + `rule_exemptions` là tập principal được cleared: mỗi `required_roles` → managed group `role:<name> holders` (SP3c-1), mỗi `required_groups` → group theo tên. Lowering đọc withhold rule + exemptions (expand qua principal directory chuẩn → role lẫn group đồng nhất). Migration đảo ngược, có provenance; equivalence gate byte-identical cho clearance role-based; groups-only (trước ra rỗng) nay được enforce (test riêng, không gate byte-identical).

## Ràng buộc chung (chi tiết + giá trị chính xác ở bản EN)
- **Dựa SP3c-1** (#858); branch off `feat/gov-sp3c`; tái dùng `role_holder_group_name`.
- `perm.rule_kind` thêm `'withhold'`; withhold row: selector_kind='tag', tag_key, mask/filter NULL (chỉnh CHECK).
- Exemption = cleared: required_roles→`kind='group' ref=role:<name> holders`; required_groups(uuid)→`kind='group' ref=<groups.name>`.
- **Fix groups-only**: `cleared` = union holder(required_roles) ∪ members(required_groups); bỏ nhánh skip groups-only ở `lower_withholds`.
- Equivalence gate **byte-identical CHỈ cho clearance có required_roles** (demo: 1 clearance role-based data_steward → byte-identical hoàn toàn); groups-only đổi behavior có chủ đích (test riêng).
- Đảo ngược + provenance `perm.clearance_migration` (không dùng migrated_from). Undo hash-guard 409.
- `classification_policies` rời enforcement input (Task 5 đọc withhold rule); vẫn giữ làm store authoring, write path sync sang rule (Task 6). KHÔNG xoá bảng.
- `explain` không đổi cho role-based. CI gate như thường. Migration 0222+ (max 0221). `CARGO_TARGET_DIR=target-sp3c2`. Co-author 5.5. StarRocks test seed ảnh fork egret.4.

## Tasks (code đầy đủ ở bản EN)
1. Migration `0222`: thêm `withhold` vào rule_kind + CHECK (withhold không payload). Lưu ý Postgres cấm dùng enum value mới trong cùng tx vừa ADD — có thể tách file.
2. Fix groups-only: `load_withhold_inputs` resolve required_groups vào `cleared`; bỏ `required_groups_empty` + nhánh skip ở `withhold.rs`. Test groups-only giờ enforce.
3. Migration `0223` provenance `perm.clearance_migration`.
4. `clearance_migrate.rs`: `migrate_clearances_in_tx`/`undo_clearances_in_tx` + equivalence gate (loại trừ groups-only) + undo hash-guard.
5. `load_withhold_inputs` đọc withhold rule thay vì classification_policies; chokepoint clearance arm theo exemption rule. classification_policies rời enforcement.
6. Write path clearance authoring sync sang withhold rule (`upsert_clearance_withhold_rule_in_tx`).
7. Endpoint `POST /perm/migrate/clearances` (+undo) + regen openapi/client + test explain byte-identical.

## Ngoài phạm vi
Implicit ws grant→explicit (SP3c-3); dời authoring khỏi classification_policies hẳn (gói sau); per-EP gate + switch + live (SP3d); Effect::WithholdMask vẫn chưa dùng.
