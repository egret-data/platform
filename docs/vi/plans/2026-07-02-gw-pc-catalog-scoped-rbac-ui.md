# Gateway P-C: RBAC UI theo catalog (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-gw-pc-catalog-scoped-rbac-ui.md`. Spec Trụ 3. Phụ thuộc P-B (routes Catalog Client) + routes catalog-role/grant P4/P5. subagent-driven.

**Mục tiêu:** đưa RBAC vào đúng catalog. Trang **catalog detail** (`/control-plane/catalog/{name}`) gồm Roles + Grants + Clients; bỏ trang `/control-plane/rbac` phẳng.

**Kiến trúc:** React/shadcn. Route detail vào từ bảng catalogs. 3 section dùng endpoint có sẵn: catalog-roles (`GET/POST /polaris/catalogs/{c}/catalog-roles`), grants (`PUT .../catalog-roles/{cr}/grants`), Catalog Clients (`GET/POST /polaris/catalogs/{c}/clients`, `DELETE /polaris/clients/{id}` từ P-B). Bỏ assign dialog cũ + trang RBAC phẳng.

## Task
- **T1 (client fns + route detail):** thêm vào `controlPlane.ts`: `Client`/`ClientCreated` type + `listClients`/`createClient`/`revokeClient`. Thêm route `catalog/:name`→`CatalogDetail.tsx` (useParams, header + back, Tabs "Roles & grants"/"Clients"); bảng catalogs link Name→detail. Test heading + tabs render.
- **T2 (Roles & grants):** tab Roles = `Table` từ `listCatalogRoles(name)` + "Add role" (`createCatalogRole`); per role "Add grant" (kind/id/privilege → `addGrant`). Grants write-only (note). Test list roles.
- **T3 (Clients):** tab Clients = `Table` (Name/Consumer/Role/ClientID/LastUsed/Actions) từ `listClients(name)`; "Add client" (name + consumerType + workspace `useMyWorkspaces` + catalogRole `listCatalogRoles`) → `createClient` → hiện clientId/secret **1 lần** (copyable + cảnh báo); row "Revoke" (`revokeClient`, confirm). Test list + create-shows-secret-once.
- **T4 (bỏ RBAC phẳng + assign):** `git rm ControlPlaneRbac.tsx` + test; App.tsx bỏ route `rbac` + import; Sidebar bỏ nav "RBAC" (+ sửa Sidebar.test); `ControlPlaneCatalog.tsx` bỏ `AssignCatalogDialog` + nút "Assign…" (access = tạo Client trong catalog detail); giữ connection card + catalogs table. typecheck+lint+test.
- **T5 (verify):** typecheck/lint/test/build; manual: mở catalog → add role+grant → add Client (secret 1 lần) → revoke; không còn `/rbac`, không "Assign…".

## Ghi chú
- Credential Client hiện 1 lần (khớp P-B).
- Ngoài P-C: proxy (P-A), IdP (P-B), trust Polaris + e2e (P-D).
