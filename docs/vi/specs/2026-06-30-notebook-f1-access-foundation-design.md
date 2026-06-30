# Notebook F1 — Nền tảng truy cập (thiết kế)

**Trạng thái:** nháp · **Ngày:** 2026-06-30 · **Epic:** Notebooks · **Trước đó:** C1 (đã merge, squash `b57a418`, PR #94)
> Bản tiếng Anh là canonical: `docs/en/specs/2026-06-30-notebook-f1-access-foundation-design.md`. Bản này là tham chiếu.

## 1. Bối cảnh & phạm vi
Mô hình cô lập data của notebook (memory `egret-notebook-access-model`) là **2 plane**:
- **Đọc governed** qua Trino, Ranger phân quyền **per end-user** (`X-Trino-User` impersonation). Governed = read-only với notebook.
- **Đọc/ghi riêng** ở namespace `notebook_{user}` trên REST catalog, gate bằng **catalog token 1-grant** per (user×workspace).

F1 xây **nền tảng** cho cả 2 plane để D1 (kernel) + E1 (execution) dùng. F1 **không** xây kernel/execution — chỉ giao (a) Trino impersonation per-user và (b) namespace+token riêng + 1 runtime endpoint đưa kernel mọi thứ cần.

### Trong phạm vi
- **F1a — Trino impersonation:** `username` cố định per-user; cờ `impersonate` opt-in per engine connection; `TrinoConnector` gửi `X-Trino-User = username` (basic-auth vẫn service principal) khi connection bật cờ.
- **F1b — Catalog access riêng:** provision idempotent namespace `notebook_{username}` + token 1-grant; endpoint `GET /api/ws/{ws}/notebook-runtime` trả manifest cho kernel.

### Ngoài phạm vi (sau)
Kernel + helper + execution + output (D1/E1); StarRocks per-user; cấu hình Trino impersonation + Ranger policy (ops, không phải code egret — nhưng cần để enforce, xem §3.4); per-user MinIO IAM/STS.

## 2. Quyết định (từ brainstorm)
- **Phạm vi impersonation = cờ per-connection, default OFF.** Không đổi hành vi SQL Lab/charts cho tới khi admin bật cho 1 Trino connection đã cấu hình. Khi ON, **mọi** query qua connection đó (SQL Lab + notebook) chạy as end-user — governance nhất quán.
- **`X-Trino-User` = `username` = local-part email**, lưu **cố định** per-user (derive 1 lần; suffix khi trùng), KHÔNG derive lại mỗi query.

## 3. F1a — Trino impersonation
- **3.1 Username cố định:** thêm `username citext UNIQUE` vào `users` (migration). Derive `slug(local-part(email))`, trùng thì thêm suffix (kiểu sparklabx); backfill. Immutable; là danh tính Ranger policy viết theo.
- **3.2 Cờ `impersonate`:** thêm `impersonate: Option<bool>` (default false) vào `ConnOptions` (lưu trong `databases.options` JSONB — không cần migration schema). Toggle trên form engine connection (DatabasesPage), chỉ Trino, kèm note.
- **3.3 Luồng connector:** job biết `user: Option<Uuid>` → resolve `username` → truyền `session_user: Option<String>` qua `ConnectorFactory::build`/`ConnSpec`. `TrinoConnector` hiện set CẢ `X-Trino-User` lẫn basic-auth = service user → **tách**: basic-auth = `spec.options.user` (service principal được phép impersonate, giữ nguyên); `X-Trino-User` = `session_user` **chỉ khi** `impersonate==true` + có session_user, ngược lại = service user (hành vi cũ). StarRocks để sau.
- **3.4 Config Trino+Ranger (ops, không build ở đây):** cần cho service principal được impersonate + trỏ Ranger plugin của Trino tới egret-ranger + viết policy. F1 ship code + note; manual Trino test (§7) verify end-to-end.
- **3.5 An toàn:** cờ OFF (default) = hành vi y nguyên, không rủi ro. ON nhưng Trino chưa cấu hình → Trino từ chối query (lỗi ồn ào, gọn trong 1 connection), không bypass ngầm.

## 4. F1b — Catalog access riêng
- **4.1 Provisioning (idempotent)** mỗi lần kernel start: đảm bảo username; namespace `notebook_{username}` tồn tại (qua `NamespaceService::create_namespace`, already-exists = ok); token 1-grant `{notebook_{username}: read+write}` (qua `CatalogStore::create_token`). Vì token lưu hashed (không lấy lại raw), endpoint **mint token mới + revoke token cũ** của (user×workspace) mỗi lần gọi (`revoke_token_for_workspace` + `create_token`) → kernel luôn có raw token dùng được, tối đa 1 token active, không lưu raw at-rest.
- **4.2 Runtime endpoint** `GET /api/ws/{ws}/notebook-runtime` (gate `notebook:read`): provision (idempotent) rồi trả **manifest**: `{ username, catalog{url,warehouse,namespace,token}, engines[], catalogs[] }`. `engines`/`catalogs` lấy từ **Notebook settings** (RS-2) — chỉ những cái workspace đã enable cho notebook. Đây là **cầu nối F1→D1**: kernel gọi lúc boot để lấy token, namespace riêng, username (cho X-Trino-User), engines/catalogs. Helper (`query()`/`catalog()`/handle) build ở D1/E1 trên manifest này.
- **4.3 Vị trí:** endpoint + provisioning ở `egret-notebook`; thêm dep `egret-catalog` (mint/revoke token) + gọi `NamespaceService` để tạo namespace (chốt call path sạch ở plan).

## 5. Luồng dữ liệu
- **Đọc governed:** client → engine job (biết user_id) → resolve username → TrinoConnector (impersonate on) → Trino `X-Trino-User=alice` → Ranger duyệt alice → rows.
- **Đọc/ghi riêng:** kernel boot → `GET /notebook-runtime` → {token, namespace…} → kernel dùng token 1-grant với REST catalog (scope `notebook_alice`); data IO qua remote signer sẵn có.

## 6. Bảo mật / authz
Impersonation default OFF; ON là chủ ý admin per-connection. `X-Trino-User`=username cố định; basic-auth vẫn service principal (chỉ nó được impersonate, enforce phía Trino). Token runtime **1-grant** (chỉ `notebook_{username}` rw) → kernel bị chiếm cũng không vượt namespace/người khác. Endpoint gate `notebook:read`, chỉ provision namespace/token **của chính caller**. engines/catalogs = tập notebook-allowed của workspace (RS-2, đã ws-scoped). Mint-and-revoke giữ tối đa 1 token active.

## 7. Kiểm thử
- **Unit/integration (`#[sqlx::test]`):** derive username + suffix + ổn định; provisioning idempotent (tạo 1 lần, gọi lại mint mới + revoke cũ); runtime trả manifest chỉ gồm engines/catalogs đã enable; gate `notebook:read`; grant token đúng `{notebook_{username}: rw}`; cross-user (runtime của B không trả namespace/token của A).
- **Connector test:** `impersonate=true` + session_user → request Trino mang `X-Trino-User=<username>`, basic-auth=service; `false` → y nguyên.
- **Manual real-Trino test** (memory `egret-manual-trino-integration-test`): bật cờ trên Trino dev, query 2 user, verify Ranger chặn đúng (1 cho, 1 chặn) — impersonation thực sự có hiệu lực. Chạy trước merge.
- **CI local đầy đủ** trước merge (memory `egret-run-full-ci-local-before-merge`). **Migration append-only** (bài học C1 — KHÔNG sửa migration đã apply).

## 8. Phân rã build (1 spec, 2 nhóm)
- **F1a:** username column+derive+backfill · cờ `impersonate` ở ConnOptions + toggle DatabasesPage · luồng connector (job→username→ConnSpec→TrinoConnector) · tests.
- **F1b:** provisioning (namespace + token mint/revoke) · endpoint `/notebook-runtime` (đọc settings RS-2) · tests.

## 9. Câu hỏi mở (chốt khi plan)
1. Backfill username: trong-migration vs lazy-on-first-use. *Lean: trong-migration cho xác định, lazy cho user tạo sau (OIDC).*
2. Call path tạo namespace từ egret-notebook (gọi thẳng `NamespaceService` vs route nội bộ) — chọn cái không rối ranh giới crate.
3. Toggle `impersonate` cần perm mới? — tái dùng perm quản lý database (`workspace:manage_members`, theo RS-2).
