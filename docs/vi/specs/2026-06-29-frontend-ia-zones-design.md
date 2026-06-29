# Frontend IA — zone workspace + khung Control Plane — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-29-frontend-ia-zones-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-29 · **Branch:** `feat/frontend-ia-zones` (off main). Dựng lại IA sidebar thành các zone theo mục đích + dựng **khung** (phần lớn placeholder) cho Data Engineering / Data Governance / Control Plane, theo mô hình 3-lớp ([[egret-three-layer-management-model]]).

## Mục tiêu
Tổ chức điều hướng thành zone rõ ràng + dựng **khung** cho đợt feature tới (chưa làm feature thật). Đổi "Admin" → **Control Plane**.

## Hiện trạng
`Sidebar.tsx`: 2 `NavGroup[]` — `EXPLORE_GROUPS` (workspace: Explore + Manage) và `ADMIN_GROUPS` ("Administration"). `isAdmin = pathname.startsWith("/admin")` chọn nhóm + base. `App.tsx`: `ExploreRoutes` + `AdminRoutes`; trang admin = `<Placeholder/>`. `routes/Placeholder.tsx` (frame "later phase"). `Header.tsx`: portal toggle, `<Link to="/admin">` nhãn "Admin"; test `PortalToggle.test.tsx` dùng `/admin`.

## Quyết định
1. **Workspace `/explore/:ws/*` — 4 zone:** **Explore** (SQL Lab·Datasets·Charts·Dashboards·Notebooks·**Alerts**⬚) · **Data Engineering**⬚ (**Data API·Workflows(n8n)·Pipelines(Airflow)·Code IDE(git)**) · **Data Governance**⬚ (**Policies·Data Lineage·Query Audit·Data Quality**) · **Manage** (Catalog·Storage·Databases·Secrets, giữ nguyên).
2. **Control Plane `/control-plane/*`** (đổi từ `/admin`), heading "Control Plane": **Workspaces**(có) · **Resources**⬚🔲 · **Monitoring**(có) · **Users**(có) · **Platform Policies**(đổi từ Policies) · **Catalog**(overview, có).
3. **Đổi `/admin`→`/control-plane`** mọi nơi: `App.tsx` (route, `AdminRoutes`→`ControlPlaneRoutes`), chuyển `routes/admin/`→`routes/control-plane/`, `Header.tsx` (isAdmin + link + nhãn), `Sidebar.tsx` (base+isAdmin+heading), cập nhật `PortalToggle.test.tsx`.
4. **Độ sâu:** item mới = `<Placeholder/>`; riêng **Resources** = khung có cấu trúc (no backend): Card + tabs **Trino/StarRocks/Spark Connect** + bảng `Name·Type·Endpoint·Status·Workspaces·Actions` (empty state) + nút **Add resource** (stub).
5. **Policies 2 lớp tách scope:** workspace Data Governance "Policies" (`/explore/:ws/policies`, placeholder) + Control Plane "Platform Policies" (`/control-plane/platform-policies`, đổi từ Policies cũ).

**Route mới:** workspace: `alerts, data-api, workflows, pipelines, code-ide, policies, data-lineage, query-audit, data-quality`. control-plane: `workspaces, resources(frame), monitoring, users, platform-policies, catalog`; index → `workspaces`.
**Icons:** Bell, Plug, Workflow, Waypoints, Code, ShieldCheck, Share2, ScrollText, BadgeCheck, Server.

## Components
`Sidebar.tsx` (NavGroups 4 zone + control-plane group, base/heading), `Header.tsx` (link /control-plane), `App.tsx` (route mới + ControlPlaneRoutes), `routes/control-plane/*` (chuyển từ admin) + trang placeholder mới + `ControlPlaneResources.tsx` (frame), `PortalToggle.test.tsx` + test sidebar cập nhật.

## Test (Vitest)
Sidebar render đủ 4 zone workspace + group Control Plane đúng link; vào từng route mới → đúng placeholder / Resources frame; `/control-plane` index → `workspaces`; Header toggle link `/control-plane` nhãn "Control Plane"; isAdmin dùng `/control-plane`; Resources frame render 3 tab + empty + nút Add. Test cũ vẫn xanh (cập nhật rename).

## Ngoài phạm vi
Feature thật sau mỗi placeholder (Alerts; Data API; Workflows; Pipelines; Code IDE; Policies/Platform Policies Ranger UI; Lineage; Query Audit; Data Quality; Control-Plane Resources CRUD + gán + monitor live). Không đụng backend trong đợt khung này.
