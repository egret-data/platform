# Governance SP3d — Gate "migration hoàn tất" theo từng EP — Kế hoạch

> Bản tiếng Việt tham khảo. Bản gốc [tiếng Anh](../../en/plans/2026-11-14-gov-sp3d-ep-migration-gate.md) — code/lệnh/signature chính xác ở bản EN. Thực thi theo SDD.

**Mục tiêu:** mỗi enforcement point (EP = 1 dòng `policy.authz_services`) có verdict *migration hoàn tất* — một phép đọc thuần trên chính các dòng của EP, chỉ `true` khi không còn operator policy bất khả chuyển (SP3b) **và** input role/workspace/clearance đã migrate (SP3c) — phơi qua HTTP và lưu lại ở mỗi lần compile.

**Kiến trúc:** soi gương `opa_guard::stored_state_refusals_via` ("rỗng = sẵn sàng"). Hàm thuần `ep_gate(...) -> GateVerdict` nhận 4 input đã tính; scoper async `ep_gate_via[_tx]` đọc chúng từ dòng của EP, scope qua `catalogs_for_service_via(ep)`. Verdict phơi read-only ở `GET /perm/policies/services/{id}/migration-gate`, cờ `complete` lưu vào `perm.compile_runs` tại seam mọi compile đi qua (`recompile_locked_service_in_tx`). Gate **không bao giờ fail compile** — model cũ vẫn enforce đến khi migrate xong (không fail-open), verdict chỉ là tín hiệu trạng thái.

## Ràng buộc chung (chi tiết ở bản EN)
- Branch `feat/gov-sp3d` chồng trên `feat/gov-sp3c-3`; **rebase lên `main` sau khi #861 merge** trước final review. `CARGO_TARGET_DIR=target-sp3d`. KHÔNG `git add -A` (`target-sp3d/` chưa ignore) — stage theo đường dẫn tường minh.
- Co-author 5.5; Conventional Commits; `Closes #N` chỉ ở PR body; `git commit` thường; không secret.
- Migration mới nhất trên stack là `0227` → SP3d dùng `0228+`; số migration là namespace chung, reconcile khi merge.
- **Mọi DB-gated test: `static SERIAL` + scope theo EP/catalog/workspace riêng (KHÔNG global count)** (bài học flake SP3c).
- Gate CHỈ đọc trạng thái migration khả nghịch (`perm.migrated_from`, `perm.clearance_migration`, `perm.ws_assignment_migration`); không đóng/xoá/ghi.
- **Ngoài phạm vi** (follow-up, cần demo stack): switch `mechanism`, reconfigure engine, live check Trino/StarRocks, screenshot badge, xoá legacy input. Không thêm cột "old/new model" hay field model/version per-EP.
- Docs endpoint mới vào `egret-data/docs` (Platform), song ngữ, tick ô Docs.

## Tasks (code đầy đủ ở bản EN)
1. **Kiểu verdict + `ep_gate` thuần** (`ep_gate.rs`): `GateRefusal` (OperatorPolicies/RolePrincipals/WorkspaceGrants/ClearanceTags) + `.message()`, `GateVerdict{complete,refusals}`, `ep_gate(mig, role_refs, ws_grants, clearance_tags)`. Unit test mọi nhánh.
2. **Scoper async** `ep_gate_via[_tx]`: role_refs (per-EP: grant/engine-grant/rule-exemption kind='role' trên catalog của EP), ws_grants (per-EP: origin='workspace_assignment' hoặc principal workspace_members trên catalog EP), clearance (installation-wide: gated tag thiếu rule withhold — query `perm.rules` trực tiếp vì compiler loại withhold), operator (ep_migration_status). DB test incomplete→migrate→complete.
3. **Endpoint** `GET services/{id}/migration-gate` (mirror `service_migration_status`, require `policy:read`) + openapi + `npm run gen:api` + drift gate. Route test.
4. **Lưu verdict ở compile seam**: migration 0228 thêm `perm.compile_runs.migration_complete boolean` (nullable); tính `ep_gate_via_tx` trong `recompile_locked_service_in_tx`, truyền `complete` vào `record_compile_ok_in_tx` (thêm arg `Option<bool>`, quét mọi caller). Gate lỗi → log + lưu NULL, KHÔNG fail compile. DB test.
5. **Overview + docs**: `migration_incomplete_count_via` + field `migration_incomplete` vào governance overview (chỗ đã gọi `failed_compile_runs_via`) + openapi/client; docs song ngữ (ý nghĩa, 4 điều kiện, endpoint, GateVerdict). Badge FE + switch + live check + screenshot = follow-up.

## Nợ xác minh (implementer phải đóng, không phải placeholder)
Constructor `CompileError` chính xác, tên field `catalogs_for_service_via`, tên cột `tags`/`classification_policies`, format `withhold_rule_name`, lệnh/đường dẫn regen openapi, vị trí assembler overview. Mỗi task ghi "verify against live schema" + lệnh `rg`.
