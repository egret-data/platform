# EB-5: UI warehouse + RBAC theo catalog (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-eb5-warehouse-rbac-ui.md`. Spec Component 5. Phụ thuộc EB-4. subagent-driven.

**Mục tiêu:** UI control-plane cho embedded catalog: danh sách Warehouses + trang Warehouse detail (roles theo consumer-type + grants + Catalog Clients). Bỏ trang thời-Polaris (connection card, catalogs Polaris, RBAC phẳng).

## Task
- **T1 (client):** thêm vào `controlPlane.ts`: warehouses (list/create/delete), roles (list/create), grants (list/add/revoke), clients (list/create/revoke). Bỏ fns Polaris (connection/catalogs/principals/assignments); giữ storage/secrets. typecheck.
- **T2 (Warehouses list):** `Warehouses.tsx` — `Table` Name/Workspace/Storage/Actions từ `listWarehouses`; "Add warehouse" (name + workspace Select + storage Select) → `createWarehouse`; row link `/warehouses/{id}` + Delete. Test list + empty.
- **T3 (Warehouse detail):** `WarehouseDetail.tsx` (useParams) Tabs: Roles&grants (`listRoles` Name/ConsumerType + "Add role" [consumerType Select] + per-role "Add grant" → `addGrant` + `listGrants` revoke); Clients (`listClients` + "Add client" [consumerType+workspace+role Select] → `createClient` → secret **1 lần** copyable + cảnh báo; Revoke). Test name+tabs + role row.
- **T4 (nav+route+bỏ Polaris pages):** thêm route `warehouses`+`warehouses/:id`; bỏ route `catalog`/`rbac`; `git rm ControlPlaneCatalog.tsx ControlPlaneRbac.tsx` (+test); Sidebar thay "Catalog"/"RBAC" → "Warehouses" (+sửa Sidebar.test). typecheck+lint+test.
- **T5 (verify):** typecheck/lint/test/build; manual: tạo warehouse→role+grant→client(secret 1 lần)→revoke.

## Ghi chú
- Client secret hiện 1 lần. Ngoài EB-5: e2e+engine wiring (EB-6).
