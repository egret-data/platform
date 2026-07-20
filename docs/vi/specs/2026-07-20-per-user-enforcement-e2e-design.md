# Enforcement per-user xuyên surfaces (Security e2e — Sub-project B) — Thiết kế

> Thuộc chuỗi "full security flow". Sub-project **B** trong ba phần:
> A (Ranger auto-admin policies — **đã merge** `16cca4d` #293) → **B = enforcement
> per-user xuyên SQL Lab → dataset → chart → dashboard → notebook (tài liệu này)**
> → C = notebook write isolation (sau).

## 1. Mục tiêu

Chứng minh enforcement Ranger per-user **nhất quán trên cả 5 surface** — SQL Lab,
dataset đã lưu, chart, dashboard tile, notebook — bằng cách drive **hai danh tính
trên CÙNG object** và assert chúng thấy **kết quả governed khác nhau**: user
full-access thấy mọi dòng + giá trị thật; member hạn chế chỉ thấy tập đã row-filter
với cột nhạy cảm bị MASK. Kết quả governed của member phải **giống nhau trên cả 5
surface**.

## 2. Hiện trạng (đã khảo sát)

**Enforcement đã nhất quán + per-user trên mọi surface** — B **không cần đổi
backend**; là e2e regression thuần.

- Mọi query trên mọi surface chạy dưới JWT mint của **caller**
  (`EngineTokenBroker::mint(user_id)`), không bao giờ dùng danh tính owner/service:
  - SQL Lab: `egret-query/src/execution/routes.rs:137` (jwt) / username caller `:127` (impersonate).
  - Dataset `ds_data`, chart `ch_run`, dashboard tile `db_card_run` — đều qua
    `resolve_delegation` → `egret-bi/routes.rs:1809`. Không có đường "chạy như creator".
  - Notebook: `egret-kernel/routes.rs:782 & 825` — kernel token gắn `(user_id, workspace_id)`.
- **Không có cache kết quả cross-user.** BI trả `{columns, rows}` inline. SQL Lab
  lưu kết quả nhưng đọc bị owner-gate (`routes.rs:198`); chỉ `platform_admin` đọc
  được kết quả SQL-Lab của người khác (admin-only, không phải member↔member).

## 3. Yêu cầu (chốt từ brainstorm)

| Quyết định | Chọn |
| --- | --- |
| Chứng minh gì | Cùng object → **user full thấy tất cả, member hạn chế thấy masked+filtered**, nhất quán mọi surface |
| Surfaces | **Cả 5**: SQL Lab → dataset → chart → dashboard → notebook (notebook cuối, best-effort) |
| Kịch bản | **admin vs member** trên cùng table/dataset/chart/dashboard/notebook |
| Governance | table `select` (+ `execute`/`impersonate`) cho cả hai; **MASK cột** + **row-filter `country='US'`** chỉ cho member |
| Đổi code | **Không** — e2e regression thuần |

## 4. Danh tính & mode engine (đã chốt)

Mint JWT per-user cần **refresh token OIDC** (`offline_access`); notebook mint JWT
**vô điều kiện** (kể cả engine impersonate, `kernel/routes.rs:782`). Break-glass
`admin@egret.local` **không** có refresh token → 401 ở notebook → không dùng được
cho test 5 surface.

- **Danh tính full: `admin-sso@egret.local`** (pw `Admin12345!`) — group
  `platform-admins` → `platform_admin` (qua `deploy/seed-sso-test-rbac.sql`), có
  `offline_access` → refresh token. Trino slug = **`admin-sso`**.
- **Danh tính hạn chế: `member-sso@egret.local`** (pw `Member12345!`) — group
  `data-team` → `ws_editor` ở `demo`/`eng`, có `offline_access`. Slug =
  **`member-sso`**. Helper `loginViaOidcMember` đã trỏ đúng.
- **Delegation engine: đặt `delegationMode: "jwt"` TƯỜNG MINH** (`enableEngineJwt`
  chỉ set `auth.jwt`, mode vẫn mặc định impersonate). jwt-mode cho MỘT danh tính
  đồng nhất (JWT `preferred_username`) tới Ranger trên cả 5 surface.

**Hệ quả then chốt từ giới hạn của A:** `platform_admin_user_ids()` của A chỉ liệt
kê grant **trực tiếp**; `admin-sso` được cấp qua **group** → A **không** auto-seed
nó. Nên B phải **tự tạo policy full-access cho `admin-sso`** (không nằm trong
auto-seed của A). Đây là phát hiện hữu ích — B phơi bày đúng gap group-grant của A
(follow-up: mở rộng `platform_admin_user_ids()` để expand group).

## 5. Kiến trúc (e2e)

Một spec opt-in `frontend/tests/e2e/per-user-enforcement-kind.spec.ts`
(`EGRET_E2E_KIND=1`, `test.setTimeout(900_000)`), deploy Ranger-on. `describe`
serial + `beforeAll` setup chung, rồi mỗi surface một block assertion.

### 5.1 Setup (control-plane = break-glass admin; data = OIDC users)

Control-plane dùng `loginLocalAdmin` + CSRF (không cần refresh token):
1. `provisionStorageAndWarehouse` → provision Trino **`delegationMode:"jwt"`** →
   `waitForEngineReady` → `assignEngineToWorkspace` (ws = `demo`).
2. Resolve Ranger service (`engineProfileName`).
3. Tạo policy Ranger (`createPolicy`, theo service):
   - **`admin-sso` full:** `access` cấp `execute` (queryid `*`) + `impersonate`
     (trinouser `*`) + `select`/`insert`/`create` trên catalog/schema/table đích —
     KHÔNG mask, KHÔNG row-filter.
   - **`member-sso` hạn chế:** `access` cấp `execute` + `impersonate` + `select`;
     `masking` (MASK) cột nhạy cảm; `rowfilter` `country='US'`.
4. Đảm bảo cả hai OIDC user tới được surface của ws `demo`: `member-sso` là
   `ws_editor`; xác nhận `admin-sso` tới được surface ws (platform_admin bypass —
   xem §7) hoặc cấp role ws. Đảm bảo engine **bật cho notebook** ở `demo`
   (`PUT /api/ws/demo/notebook-settings` với connection id — theo phát hiện A).

### 5.2 Seed data + BI objects (là `admin-sso`, đã có full access)

5. Login `admin-sso` (helper mới / `EGRET_E2E_OIDC_USER=admin-sso@egret.local`).
   Seed bảng governed, vd `egret.gov.sales(country varchar, item varchar, amount
   bigint)` với dòng đủ nước — vd 2×`US`, 1×`VN`, 1×`SG` — và cột nhạy cảm (`item`)
   để mask. Dùng SQL Lab `runQuery` DDL/DML (chạy được vì `admin-sso` có full policy).
6. Tạo BI objects một lần (dataset → chart → dashboard + card) trên bảng đó qua BI
   API. Dataset trỏ `egret.gov.sales`; chart select `country`, `item`, `amount`;
   dashboard 1 card = chart đó.

### 5.3 Assertion mỗi surface (drive CẢ HAI danh tính trên CÙNG object)

Mỗi surface, chạy "show country + item" là `admin-sso` và là `member-sso`, assert:
- **admin-sso**: đủ dòng (cả `VN`/`SG`) + `item` **thật**.
- **member-sso**: chỉ dòng `country='US'`, `item` **bị mask**.

| Surface | Endpoint | admin-sso | member-sso |
| --- | --- | --- | --- |
| SQL Lab | `runQuery` → `/queries` (`routes.rs:94`) | đủ, item thật | US, item mask |
| Dataset | `POST /datasets/{id}/data` (`routes.rs:1004`) | đủ | US, mask |
| Chart | `POST /charts/{id}/data` (`routes.rs:1829`) | đủ | US, mask |
| Dashboard | `POST /dashboards/{id}/cards/{card_id}/data` (`routes.rs:2080`) | đủ | US, mask |
| Notebook | `trino("SELECT country,item FROM egret.gov.sales")` | đủ | US, mask (**best-effort**) |

Assertion nhất quán cross-surface: `(rows, giá trị mask)` của member GIỐNG nhau cả
5, và là tập con/masked của admin.

Cleanup: `finally { deleteEngine }` (xóa engine + Ranger service + policies).

## 6. Helper (frontend/tests/e2e)

- Tái dùng: `loginLocalAdmin`, `loginViaOidcMember`, `csrf`, `firstWorkspace`,
  `provisionStorageAndWarehouse`, `waitForEngineReady`, `assignEngineToWorkspace`,
  `engineProfileName`, `createPolicy`, `runQuery`, `deleteEngine`.
- Thêm: helper `loginViaOidcAsAdminSso` (như `loginViaOidc` nhưng trỏ
  `admin-sso@egret.local`/`Admin12345!`), provisioner engine jwt-mode (hoặc param
  `delegationMode`), helper tạo BI object nếu chưa có.
- Notebook drive kernel như e2e notebook hiện có (set Monaco, Run, đọc output) dưới
  session mỗi user.

## 7. Edge case, rủi ro & câu hỏi mở (giải khi làm plan)

1. **admin-sso không được A auto-seed** (group-granted) → B tự tạo policy full cho
   nó (§4). KHÔNG dựa vào A cho phía admin.
2. **admin-sso truy cập surface ws:** `platform_admin` có bypass gate
   `dataset:read`/`chart:read`/`notebook:read` không, hay phải thêm `admin-sso` làm
   member ws? Xác nhận + nếu cần cấp role ws `demo` trong setup.
3. **Refresh token:** cả hai OIDC user có `offline_access` trong realm seed; xác
   nhận login lưu refresh token (đường OIDC đã lưu — chứng minh khi làm A). Nếu
   `seed-sso-test-rbac.sql` chưa apply sau DB reset, admin-sso có thể mất
   `platform_admin` — đảm bảo deploy apply nó.
4. **Kernel-token lifecycle notebook:** record kernel per-(user,ws) cũ có thể
   desync token; drive notebook trên kernel FRESH, coi notebook best-effort (tối
   thiểu assert member bị hạn chế; admin-full nếu kernel hợp tác).
5. **Assert masking:** assert `item` của member KHÁC giá trị seed thật (bị mask),
   thay vì hard-code output mask; và member thấy đúng số dòng US.
6. **Perm tạo BI object:** xác nhận `admin-sso` (hoặc danh tính setup) tạo được
   dataset/chart/dashboard ở `demo`.

## 8. Chiến lược test

B *chính là* test. Verify local: `npm run typecheck` + `eslint` spec mới. Live:
deploy Ranger-on + `npx playwright test per-user-enforcement-kind`. Thêm recipe
`just e2e-kind-per-user` như `e2e-kind-governance`. Nhớ bài học A:
`kubectl rollout restart deploy/egret` sau `deploy-kind-governance` trước khi chạy.

## 9. Ngoài phạm vi

- Đổi enforcement backend (đã nhất quán sẵn).
- Mở rộng `platform_admin_user_ids()` của A cho group-granted admin (follow-up; B
  workaround bằng policy admin-sso tường minh).
- **C** — notebook write isolation theo namespace mỗi workspace.
