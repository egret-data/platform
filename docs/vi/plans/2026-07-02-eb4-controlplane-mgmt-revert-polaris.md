# EB-4: control-plane mgmt + revert egret-polaris (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-eb4-controlplane-mgmt-revert-polaris.md`. Spec Component 4. Phụ thuộc EB-3. subagent-driven.

**Mục tiêu:** routes control-plane (platform_admin) quản embedded catalog (warehouses, roles/grants, clients) trên store EB-3; **gỡ Polaris** của PR #115 (`egret-polaris` + routes Polaris + bảng Polaris).

## Task
- **T1 (routes):** `CpState` thêm `warehouses/roles/clients`; router (platform_admin) merge vào `control_plane_router`: `GET/POST /warehouses`,`GET/DELETE /warehouses/{id}`; `GET/POST /warehouses/{id}/roles`, `GET/POST /roles/{roleId}/grants`, `DELETE /grants/{grantId}`; `GET/POST /warehouses/{id}/clients` (POST→201 clientId/secret **1 lần**), `DELETE /clients/{id}`. Test 403 + create warehouse.
- **T2 (gỡ Polaris):** `git rm` `polaris_conn.rs/catalogs.rs/rbac.rs/assignment.rs` (+ test) + bỏ mod/merge/`CpState` fields + construction ở main.rs (giữ storage/secrets); `git rm -r crates/egret-polaris` + bỏ khỏi workspace/deps; build workspace (compiler-guided).
- **T3 (migration 0054):** `DROP workspace_catalog_assignments; DROP polaris_connection`. migrate+verify null.
- **T4 (verify):** fmt+clippy+test workspace; authz 403 non-admin mọi endpoint warehouses/roles/grants/clients.

## Ghi chú
- Giữ: crate control-plane + Storage/Secrets (P2). Ngoài EB-4: UI (EB-5), e2e (EB-6).
