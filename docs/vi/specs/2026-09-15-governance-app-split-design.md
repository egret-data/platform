# SP2 — Governance app split

**Trạng thái:** thiết kế, chờ review
**Chương trình:** `docs/{en,vi}/specs/2026-09-15-control-plane-dg-review-programme.md`
**Triển khai:** một PR chỉ ở frontend

> Bản tiếng Anh là bản chuẩn:
> `docs/en/specs/2026-09-15-governance-app-split-design.md`.

## 1. Vấn đề

Data Governance nằm bên trong Control Plane, nên một data steward phải vào console system-admin để quản lý policy, và nhóm DG này còn giữ cả audit log — vốn là log hệ thống (quyết định phân quyền ở tầng API và các mutation của admin), không phải log truy cập dữ liệu. Có hai bản sao của danh sách quyền Control Plane, và cả hai đều thiếu `tag:read`, nên một user chỉ có quyền tag thì không thể vào Classifications được luôn.

## 2. Quyết định

- Data Governance trở thành một app cấp cao nhất tại `/governance/*`, ngang hàng với Control Plane trên header.
- Audit log ở lại trong Control Plane, trong một nhóm **System** mới, chỉ dành cho platform_admin (không đổi).
- Đường dẫn API backend giữ nguyên (`/api/control-plane/policies*`, `/tags*`, `/audit`): không cần regenerate API client, không đổi Cedar hay quyền.

## 3. Thiết kế

### 3.1 Danh sách quyền — một module duy nhất

Module mới `frontend/src/auth/appAccess.ts`:

```ts
export const CONTROL_PLANE_PERMS = [
  "engine:read", "warehouse:read", "storage:read", "secret:read",
  "user:read", "group:read", "role:read", "workspace:read",
] as const;
export const GOVERNANCE_PERMS = ["policy:read", "tag:read"] as const;
export function canEnterControlPlane(me): boolean; // platform_admin || any CONTROL_PLANE_PERMS
export function canEnterGovernance(me): boolean;   // platform_admin || any GOVERNANCE_PERMS
```

`Header.tsx` và `guards.tsx` import từ đây; các bản sao cục bộ của chúng bị xoá.

### 3.2 Header

`Explorer | Governance | Control Plane`. Governance hiển thị khi `canEnterGovernance`, Control Plane hiển thị khi `canEnterControlPlane`. Trạng thái active theo tiền tố URL.

### 3.3 Sidebar

`Sidebar.tsx` có thêm chế độ thứ ba, được chọn bởi `pathname.startsWith("/governance")`, với `GOVERNANCE_GROUPS`:

| Group | Item | Route | perm |
|---|---|---|---|
| Policies | Policies | `/governance/policies` | `policy:read` |
| Classification | Classifications | `/governance/classifications` | `tag:read` |
| Lineage | Data lineage | `/governance/data-lineage` | platform_admin (placeholder, như hiện tại) |

Việc lọc theo rule hiện có (platform_admin, hoặc có `perm`). `CONTROL_PLANE_GROUPS` mất nhóm "Data governance" và có thêm:

| Group | Item | Route | perm |
|---|---|---|---|
| System | Audit log | `/control-plane/audit-log` | platform_admin (không cần perm, như hiện tại) |

Các item tương lai (chưa render trong SP2): Governance → *Data permissions* (SP4), *Data access log* (SP4/SP5).

### 3.4 Routes

`App.tsx`:

- `GovernanceRoutes` mới dưới `/governance/*`, bọc trong `RequireGovernanceAccess` (`canEnterGovernance`, nếu không thì trang forbidden hiện có):
  - `policies`, `policies/:service`, `policies/:service/:kind/new`, `policies/:service/:kind/:id` — `RequirePermission perm="policy:read"`
  - `classifications`, `classifications/:id` — `tag:read`
  - `data-lineage` — `RequireGlobalAdmin` (placeholder)
  - index → mục đầu tiên mà user có thể thấy (`policy:read` → policies, `tag:read` → classifications)
- Bị xoá khỏi `ControlPlaneRoutes`: bảy route tương tự. `audit-log` vẫn ở lại.
- **Redirect** (bên trong `ControlPlaneRoutes`): `policies/*`, `classifications/*`, `data-lineage` → đúng subpath tương ứng dưới `/governance`, giữ nguyên `search` và `hash` (vd: `?scope=tag`), với `replace`.
- `FIRST_SECTION_BY_PERM` (trang landing Control Plane) bỏ `policy:read`; một user chỉ có quyền governance mà mở `/control-plane` sẽ được redirect sang `/governance`.

### 3.5 Trang và link

- Di chuyển file: `routes/control-plane/{Policies,ProfileDetailPage,PolicyEditorPage}.tsx` và `routes/control-plane/governance/*` → `routes/governance/`, cùng với test của chúng.
- Cập nhật mọi link trong app sang path mới. Các call site đã biết: `Policies.tsx`, `ProfileDetailPage.tsx`, `PolicyEditorPage.tsx`, `governance/Classifications.tsx`, `governance/TagDetail.tsx`, `access/AccessOverview.tsx`, `components/layout/__tests__/Sidebar.test.tsx`, và bất kỳ link nào từ `WarehouseDetail` / `EngineDetail` tới một profile hoặc tag. Planner grep `"/control-plane/(policies|classifications|data-lineage)"` loại trừ `api/` và code sinh tự động (`frontend/src/api/policies.ts` dùng path backend không đổi và không được đụng vào).
- Docstring của `AuditLog.tsx`: "Control Plane → System → Audit log".

## 4. Testing

- Unit: `appAccess` (admin, chỉ steward, chỉ infra, chỉ tag); Header hiển thị đúng mục; lọc chế độ governance của Sidebar; mỗi redirect giữ nguyên subpath và query; allow/deny của `RequireGovernanceAccess`; trang landing Control Plane cho user chỉ có quyền governance.
- Test trang policy/classification hiện có vẫn pass ở vị trí mới.
- Frontend gate: `npm run lint`, typecheck, `npm test`, `npm run build`.
- **Live trên trình duyệt** với hai tài khoản: platform_admin thấy cả ba mục trên header và mục System → Audit log; một user có platform role duy nhất là `data_steward` vào được Governance (policies và classifications), không thấy Audit log, và trong Control Plane chỉ thấy các phần read-only mà role đó đã có sẵn. Một bookmark cũ `/control-plane/policies/<svc>/<kind>/new?scope=tag` sẽ đáp xuống trình soạn thảo governance.

## 5. Ngoài phạm vi

- Ghi lại các sự kiện audit còn thiếu (đăng nhập; thay đổi cấu hình engine/warehouse/storage/secret/policy) — khuyến nghị làm tiếp theo để audit log trở thành một log hệ thống hoàn chỉnh.
- Chuyển route backend sang `/api/governance/*`.
- Bất kỳ thay đổi nào về cách policy hoặc tag hoạt động (SP4).
