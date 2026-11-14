# Governance SP3c-3 — Implicit workspace grant → explicit `ws:<slug> members` — Kế hoạch

> Bản tiếng Việt tham khảo. Bản gốc [tiếng Anh](../../en/plans/2026-11-14-gov-sp3c-3-ws-members-grant.md) — code/lệnh/signature chính xác ở bản EN. Thực thi theo SDD.

**Mục tiêu:** thay implicit workspace grant (grant `origin='workspace_assignment'` + principal `kind='workspace_members'`) bằng **explicit** READ grant tới managed group `ws:<slug> members`, để enforcement input không còn principal kind `workspace_members`.

**Kiến trúc:** y hệt pattern SP3c-1 (role→`role:<name> holders`) áp cho workspace. Managed group `ws:<slug> members` (source='managed' từ SP3c-1) soi gương member của workspace; principal `workspace_members` được viết lại thành `kind='group'` ref `ws:<slug> members`, origin→'explicit'. Vì `workspace_members` kind VÀ group ref đều expand về cùng username → output byte-identical, **không fail-open window** (grant chưa migrate vẫn expand & enforce vì kind không bị bỏ). Decision ③: `ws:<slug> members` và `role:<name> holders` cùng máy ws-members.

## Ràng buộc chung (chi tiết ở bản EN)
- Dựa SP3c-1 (đã merged); branch off main; độc lập SP3c-2. Template `role_holder_group.rs` → `ws_members_group.rs`.
- Shape cố định bởi `assignment.rs::sync_assignment_grants_in_tx` (grant workspace_assignment + principal workspace_members ref=ws_id).
- Tên group literal `ws:<slug> members` (slug = `workspaces.slug`). Membership từ `members::workspace_members_via`.
- Byte-identical; **không fail-open** (workspace_members kind vẫn expand); **không cần read-switch** (khác SP3c-2).
- Transitional: cấm authoring `kind='workspace_members'` mới + ref `ws:* members` mới (guard SP3c-1 refused_for_authoring).
- Đảo ngược + provenance `perm.ws_assignment_migration` (không dùng migrated_from). Undo hash-guard 409.
- explain không đổi. Mọi ghi user_groups qua PermGroupRecompiler; ws-membership change re-sync group.
- CI gate thường. **Mọi DB-gated test lấy SERIAL + scope own catalog/workspace (KHÔNG global count), pass default parallelism** (bài học flake SP3c-1/SP3c-2).
- Migration: max main 0221, nhưng SP3c-2 (#859) dùng 0222-0224 → chọn số tự do trên main + PR đang mở, reconcile ở merge.
- `CARGO_TARGET_DIR=target-sp3c3`. Co-author 5.5. Không push. (Docker local có thể down → CI chạy DB-gated.)

## Tasks (code đầy đủ ở bản EN)
1. `ws_members_group.rs`: `ws_members_group_name(slug)` + `sync_ws_members_group_in_tx`/`_if_present_in_tx` (mirror role_holder_group.rs; adoption guard như SP3c-1).
2. Chokepoint: ws-membership change re-sync group. **Sweep MỌI live path ghi `access_grants scope='workspace'`+recompile** (bài học missed-sibling SP3c-1).
3. Migration provenance `perm.ws_assignment_migration`.
4. `ws_assignment_migrate.rs`: `migrate_/undo_ws_assignments_in_tx` + equivalence gate byte-identical + undo 409; principal_refs chấp nhận `ws:* members` (existence-checked, không blanket).
5. `sync_assignment_grants_in_tx` tạo explicit ws:<slug> members group grant thay workspace_members (giữ idempotent bind/unbind; chốt dedup key trong report).
6. Write guard (cấm workspace_members mới + ws:* members ref mới) + endpoint `POST /perm/migrate/ws-assignments` (+undo) + regen openapi/client.
7. Test explain byte-identical qua migration.

## Ngoài phạm vi
SP3d (per-EP gate + switch + live); bỏ hẳn kind `workspace_members` khỏi enum (cleanup sau); docs (bind giờ grant ws:<slug> members group + endpoint migration).
