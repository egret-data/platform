# Governance SP3c-1 — Role principal → managed group `role:<name> holders` — Kế hoạch

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-13-gov-sp3c-1-role-holder-groups.md) — mọi code, lệnh, signature, tên hàm chính xác ở bản tiếng Anh; khi khác nhau thì bản tiếng Anh đúng. Thực thi theo SDD.

**Mục tiêu:** thay mọi principal `kind='role'` trong model quyền trung lập bằng managed group `role:<name> holders` có membership soi gương (mirror) holder của role, để output biên dịch cho engine **byte-identical** và role principal biến mất khỏi enforcement input.

**Kiến trúc:** thêm lớp group `source='managed'`. Mỗi role được tham chiếu bởi grant/rule-exemption/engine-grant giữ một managed group `role:<name> holders` đồng bộ với holder của role (tái dùng query holder sẵn có). Một migration **đảo ngược được, có equivalence-gate** viết lại các ref `role` thành ref `group`. Chokepoint recompile role-membership được mở rộng để managed group luôn tươi khi holder đổi.

## Ràng buộc chung (tóm tắt — chi tiết + giá trị chính xác ở bản EN)
- **Output biên dịch byte-identical** (role và group đều expand về cùng username). Golden/equivalence khác 0 = sai.
- **`explain` không đổi** cho mọi principal liên quan — chứng minh bằng equivalence probe.
- Tên group đúng literal **`role:<name> holders`**; `source='managed'` (giá trị CHECK mới).
- **Transitional:** sau khi ship, KHÔNG grant/rule/engine-grant mới nào được nhắm thẳng group `role:* holders`, và không ghi `kind='role'` mới — chặn ở write seam.
- **Đảo ngược được + provenance** (bảng `perm.role_principal_migration` riêng; KHÔNG dùng `perm.migrated_from` vì bảng đó khóa theo `policy_id`, loại trừ clearance/role). Undo hash-guard, 409 nếu row đã đổi.
- Chỉ migrate role **được tham chiếu** (đo demo: chỉ `data_steward`); code tổng quát nhưng không xử lý 9 role không tham chiếu.
- Mọi ghi `user_groups` đi qua `PermGroupRecompiler`.
- Migration number dùng chung namespace: max hiện tại `0219` → mới `0220+`, renumber trên main + PR đang mở lúc thực thi.
- CI gate: fmt, clippy, `committed_openapi_json_is_current`, generated-client digest, DB-gated test. `CARGO_TARGET_DIR=target-sp3c`. Co-author Claude Opus 5.5. Không push.
- Package này KHÔNG tạo StarRocks engine → không dính fork-image gotcha (3c-2 mới dính).

## Tasks (xem code đầy đủ ở bản EN)
1. Migration `0220`: cho phép `groups.source='managed'`.
2. Module `role_holder_group.rs`: `role_holder_group_name` + `sync_role_holder_group_in_tx` (upsert group managed + reconcile `user_groups` đúng bằng holder của role, idempotent).
3. Mở rộng chokepoint `recompile_role_referencing_profiles`: re-sync holder group khi holder đổi, trong cùng transaction.
4. Migration `0221` (provenance) + `role_principal_migrate.rs`: `migrate_role_principals_in_tx` (viết lại role→group, ghi provenance, equivalence-gate byte-identical, refuse nếu lệch) + `undo_role_principals_in_tx` (hash-guard, 409 nếu lệch). `principal_refs.rs` chấp nhận ref managed-group.
5. Endpoint admin `POST /perm/migrate/role-principals` (+ `/undo`) + guard write-seam (từ chối `kind='role'` mới và ref mới tới group `role:* holders`). Regen openapi + client.
6. Test `explain` byte-identical qua migration cho member của role.

## Ngoài phạm vi
Clearance→withhold + schema `rule_kind='withhold'` + provenance clearance (SP3c-2); implicit ws grant→explicit `ws:<slug> members` (SP3c-3, tái dùng máy managed-group ở task 1–3); dọn đường holder-resolution cũ; thay đổi frontend (kỳ vọng không có — task 6 chứng minh explain ổn định).
