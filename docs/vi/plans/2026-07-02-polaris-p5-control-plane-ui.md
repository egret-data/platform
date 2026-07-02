# Control-Plane Polaris — Plan 5: UI control-plane (catalog + RBAC + assignment) (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-polaris-p5-control-plane-ui.md` (code/step đầy đủ).
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P5/6.** Dùng routes P4 + trang Storage/Secrets P2. Thực thi qua subagent-driven-development.

**Mục tiêu:** UI global-admin dưới `/control-plane`: đăng ký connection Polaris, quản catalog (tạo trên storage/list/delete), quản RBAC (principals/roles/grants), gán catalog cho workspace — theo khung `ControlPlaneEngines` + shadcn.

**Kiến trúc:** React 18 + shadcn (forwardRef) + TanStack Query. Trang mới trong `routes/control-plane/`, wire vào `ControlPlaneRoutes` (App.tsx, RequireGlobalAdmin) + section "Control Plane" trong Sidebar. Data qua `controlPlane.ts` (mở rộng từ P2). Verify Vitest + typecheck/build.

## Bối cảnh
- `ControlPlaneRoutes` có workspaces/engines/users → thêm catalog/rbac/storage/secrets. Sidebar section "Control Plane" (heading ~L65). Frame: `ControlPlaneEngines.tsx` + `__tests__`. shadcn: Table/Card/Tabs/Button/Dialog/Select/Input/Badge. `apiJson`/`apiFetch` (`@/api/client`).
- Endpoint P4: `/control-plane/polaris/connection`(+health), `/polaris/catalogs`, `/polaris/principals`(create lộ secret 1 lần), `/polaris/principal-roles`, `/polaris/catalogs/{c}/catalog-roles`, assign PUT, grants PUT, `/control-plane/workspaces/{ws}/catalog-assignments`.

## Task
- **T1 (client):** thêm vào `controlPlane.ts`: connection (get/put/health), catalogs (list/create/delete), rbac (principals/roles/catalog-roles/grants), assignment (list/assign/unassign). typecheck.
- **T2 (connection card):** `ControlPlaneCatalog.tsx` — nếu connection null → form "Connect Polaris" (baseUrl, kcTokenUrl, kcClientId, kcClientSecret, kcScope)→put; nếu có → hiện + badge health (`polarisHealth`) + Edit. Test "prompt connect khi null".
- **T3 (catalogs table):** bảng Name/Type/Storage/Workspaces/Actions từ `listCatalogs`; dialog "Add catalog" (Name + Select storage từ P2 client)→`createCatalog`; row Delete + "Assign…". Test list.
- **T4 (assign dialog):** `AssignCatalogDialog` — list workspace + toggle assign/unassign cho catalog → `assignCatalog`/`unassignCatalog`; hiện ws đã gán; invalidate. Test assign gọi đúng.
- **T5 (RBAC page):** `ControlPlaneRbac.tsx` Tabs: Principals (create → hiện clientId/secret **1 lần** copyable + cảnh báo; delete), Principal Roles (create + assign-to-principal), Catalog Roles (chọn catalog → list/create + assign principal-role), Grants (chọn catalog+catalog-role → add grant: securable type + id + privilege Select). Test list principals.
- **T6 (nav+route+verify):** thêm route catalog/rbac/storage/secrets vào `ControlPlaneRoutes`; thêm nav Sidebar; `typecheck+lint+test+build`; manual: connect Polaris → tạo catalog trên `minio` → gán `eng` → thêm principal + table grant; non-admin không vào được.

## Ghi chú
- clientSecret của principal hiện **1 lần** (khớp P4: route trả 1 lần, không lưu).
- Ngoài P5: Polaris compose + e2e + wiring connector engine (P6); future epic K8s-provisioning.
