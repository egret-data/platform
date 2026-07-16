# StarRocks Delegation / Bảo mật — Ngang hàng Trino — Thiết kế

**Ngày:** 2026-07-16 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/starrocks-delegation` (off `feat/starrocks-provisioning`)
**Chương trình:** StarRocks ngang-hàng-Trino, **lát 3/3 (cuối)**. Lát 1 (cưỡng chế Ranger) đã merge (#259); lát 2 (cấp phát) là PR #261 — nhánh này off nó. Spec đưa delegation per-user + bảo mật transport của StarRocks ngang với công việc delegation gần đây của Trino (#254–#257).

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-16-starrocks-delegation-design.md`.

## Mục tiêu

- **Chọn chế độ delegation** — `jwt` | `impersonate`, theo từng kết nối, y như Trino.
- **Chế độ impersonate** — egret kết nối bằng service user rồi chạy truy vấn **với tư cách người gọi** qua `EXECUTE AS <session_user> WITH NO REVERT` (tương đương `X-Trino-User` của Trino), Ranger ủy quyền theo danh tính đã chuyển.
- **TLS** — kết nối MySQL (`:9030`) chạy qua TLS, ghim cert FE, bịt lỗ JWT-cleartext-trên-TCP-thường (`mysql_clear_password` gửi token không mã hoá — lát 1 đã cảnh báo là không an toàn cho prod).
- **Cấp phát** — FE StarRocks được cấu hình security-integration JWT + FE TLS trên `:9030` + service user có quyền impersonate, gate theo cấu hình delegation của engine (không còn `security-integration.sql` thủ công).

Chứng minh live: truy vấn delegated ở mỗi chế độ qua TLS, Ranger cưỡng chế theo danh tính delegated.

## Ngoài phạm vi

- **Quyết định delegation + mint token DÙNG CHUNG, KHÔNG đổi.** `submit_query` (`execution/routes.rs`) đã phân giải `(session_user, bearer)` theo chế độ, **bất-khả-tri-engine** (delegated + `impersonate` → `session_user=Some, bearer=None`; delegated + jwt → mint engine JWT của người gọi). Bộ mint engine-JWT OIDC dùng chung với Trino. Lát này chỉ làm **connector** StarRocks tôn trọng đường impersonate + TLS, và cấp phát FE để chấp nhận chúng.
- Không thêm authz. Impersonate dựa StarRocks Ranger (lát 1); jwt dựa `authentication_jwt` của FE.

## Bối cảnh (đã kiểm chứng)

- **Quyết định delegation dùng chung + bất-khả-tri-engine** (`routes.rs::submit_query`): kết nối `db.delegated` sinh `(session_user, bearer)` — impersonate = `(Some(username), None)`; jwt = `(Some(username), Some(jwt))`; người gọi non-OIDC trên kết nối delegated bị từ chối (403), không fallback service creds.
- **`ConnSpec`** đã có `secret`, `session_user`, `bearer_token`; **`ConnOptions`** đã có `impersonate: Option<bool>`, `http_scheme`, `tls_cert`. Trino dùng hết; StarRocks chỉ dùng `bearer_token`.
- **Connector Trino** làm cả 2 chế độ + TLS. **Connector StarRocks** CHỈ jwt, KHÔNG TLS: bearer → `mysql_clear_password(JWT)`, login user = `session_user`. Đường impersonate (`bearer=None`, `session_user=Some`) hiện **bỏ qua `session_user`**, kết nối bằng service user — KHÔNG `EXECUTE AS`, nên KHÔNG impersonate. TCP thường trên `:9030`.
- **Cấp phát** (lát 2) template FE ConfigMap nhưng CHƯA cấu hình `authentication_jwt`, FE TLS, hay service user impersonate; `security-integration.sql` của lát 1 là bước thủ công.
- **Khả năng StarRocks:** `EXECUTE AS <user> WITH NO REVERT` chuyển danh tính session không-hoàn-lại cho kết nối; service user cần quyền `IMPERSONATE ON USER`. FE hỗ trợ SSL trên query port qua `ssl_*` `fe.conf` + keystore. `mysql_async` hỗ trợ TLS client qua `SslOpts` (ghim root cert).

## Kiến trúc

### A. Connector: impersonate + TLS (`connector/starrocks.rs`)
1. **Impersonate.** Tính danh tính chuyển giống Trino — impersonate khi `bearer.is_some() || options.impersonate==Some(true)` và có `session_user`. Lưu `execute_as: Option<String>`. Nếu `execute_as` có + đường non-bearer (impersonate), chạy `EXECUTE AS \`<escaped>\` WITH NO REVERT` làm câu đầu tiên trước truy vấn (helper escape backtick). jwt đã login AS user nên không cần EXECUTE AS.
2. **TLS.** Khi `options.tls_cert` có, dựng `mysql_async::SslOpts` ghim cert FE + set lên `OptsBuilder`. `mysql_clear_password` PHẢI qua TLS; nếu cleartext-plugin bật mà chưa có TLS → cảnh báo to (dev). Cấp phát prod luôn set cert.

Test unit: opts (SSL khi có cert) + phát/escape `EXECUTE AS`.

### B. Cấp phát: FE JWT + TLS + user impersonate (`egret-orchestrator/starrocks.rs` + engine-mgmt)
Mở rộng ConfigMap/bootstrap FE của lát 2, gate theo cấu hình delegation: (1) security-integration `authentication_jwt` (từ SQL lát 1, template hoá theo OIDC engine); (2) FE TLS `:9030` (`ssl_*` + keystore egret sinh + ghim qua `tls_cert`); (3) service user có `IMPERSONATE`. Ghi kết nối engine với `impersonate`/`tls_cert` đúng để connector tự chọn chế độ.

### C. Frontend
Selector chế độ delegation (#257) set `db.options.impersonate` bất-khả-tri-engine; đảm bảo nó (+ chỉ báo TLS) hiển thị cho kết nối/engine **StarRocks**, không gate-Trino. Chủ yếu tái dùng; rà các gate `engine==="trino"`.

## Luồng dữ liệu

Truy vấn delegated → `submit_query` phân giải `(session_user, bearer)` theo chế độ (không đổi) → `ConnSpec` → connector StarRocks: jwt → TLS + `mysql_clear_password(JWT)` login AS user; impersonate → TLS + login service user + `EXECUTE AS <session_user> WITH NO REVERT` → FE xác thực → Ranger ủy quyền danh tính hiệu lực → rows.

## Kiểm thử

- **Unit (CI):** opts connector (SSL khi có `tls_cert`; `EXECUTE AS` phát+escape ở đường impersonate, KHÔNG ở jwt); template provisioner (FE ConfigMap có JWT integration + `ssl_*` khi có delegation block).
- **CHỈ-LIVE:** FE TLS + JWT integration + user impersonate; chạy truy vấn delegated ở **jwt** và **impersonate** qua TLS, mỗi cái Ranger cưỡng chế theo danh tính delegated. Báo cáo đúng những gì đã chạy.

## Rủi ro chỉ-live
1. Ngữ nghĩa `EXECUTE AS … WITH NO REVERT` + quyền `IMPERSONATE` của service user — sửa 1 dòng SQL/grant.
2. FE TLS `:9030` — key `ssl_*` + định dạng keystore của image `fe-ubuntu` (chỉ-live như lát 2); `mysql_async` chấp nhận ghim cert.
3. Kích hoạt JWT integration — lát 1 thấy `CREATE SECURITY INTEGRATION` cần bước activate riêng.

## Phân rã (cho plan)
1. Connector impersonate (execute_as + EXECUTE AS + escape). 2. Connector TLS (SslOpts ghim cert). 3. Cấp phát (FE JWT + TLS keystore + user impersonate, gate delegation; ghi impersonate/tls_cert). 4. Frontend (bỏ gate-Trino). 5. Cổng + LIVE SMOKE (jwt + impersonate qua TLS, Ranger cưỡng chế).
