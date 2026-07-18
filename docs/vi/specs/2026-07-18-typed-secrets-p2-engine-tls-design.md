# Typed Secrets P2 — Projection primitive + engine coordinator TLS — Thiết kế

**Ngày:** 2026-07-18 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/typed-secrets-p2-engine-tls` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-18-typed-secrets-p2-engine-tls-design.md`.

## Vấn đề

P0/P1a ([[egret-typed-secrets]], PR #277/#278) cho admin tạo typed `kubernetes.io/tls` secret (cert+key). Nhưng chưa gì DÙNG được: coordinator TLS của jwt-mode Trino engine luôn là **cert self-signed egret tự sinh per-engine** (`ensure_tls_cert` → `generate_self_signed`). Không có cách back HTTPS listener của coordinator bằng cert cung cấp — vd một cert công ty dùng chung nhiều engine ("cert mọi nơi").

## Mục tiêu — sub-project này (P2)

Cho engine optionally tham chiếu một P1a `kubernetes.io/tls` secret cho coordinator TLS: **có cert thì dùng, không thì tự sinh** (như cũ). Giới thiệu **projection primitive** tái dùng (resolve typed-tls secret → cert+key PEM) + consumer đầu tiên (Trino coordinator TLS). Một secret cert-công-ty có thể được nhiều engine tham chiếu. Hoàn toàn backward-compatible: engine hiện tại (không tham chiếu) giữ self-signed.

## Ngoài phạm vi

- StarRocks FE-TLS-từ-secret-cung-cấp (variant — cùng primitive, slice sau).
- Ingress TLS (public-facing egret.localtest.me HTTPS bằng cert công ty) — primitive thiết kế để tái dùng cho nó, nhưng wiring chart Ingress `tls:` là slice sau.
- P1b (storage/DB → typed basic-auth).

## Bối cảnh — bề mặt đã grounding (verify)

- **`ensure_tls_cert(st, engine) -> (keystore_pem, cert_pem)`** (`egret-engine-mgmt/src/engines.rs:1544`): nếu `engine.tls_secret_id` + `coordinator_cert` đã set → resolve + reuse; else `crate::tls_cert::generate_self_signed(&[engine.coordinator_host()])` → `(cert_pem, key_pem)`, `keystore = cert_pem + key_pem`, lưu keystore vào GLOBAL egret secret `engine-tls-{id}` (qua `secrets.create`), ghi `tls_secret_id` + `coordinator_cert`.
- keystore feed `EngineSpec.config.tls: Option<Tls { keystore_pem, port }>` (`egret-engine-mgmt-api/src/lib.rs`), render bởi `trino_tls_secret` (`egret-orchestrator/src/trino.rs:430`) vào `{name}-tls` k8s Secret (`stringData.keystore.pem`), mount tại `/etc/trino/tls/keystore.pem`. `coordinator_cert` (public cert) là cái query connector pin.
- Engine TLS chỉ jwt-mode (`0086_engine_tls.sql`: `engines.tls_secret_id`, `coordinator_cert`). Migration cao nhất `0088` → file mới `0089`.
- P1a: `SecretStore::get_typed(id) -> TypedSecret{r#type, data}`; `WorkspaceSecretStore` lộ named secret (`resolve_plaintext` trả 1 value; cần resolver TYPED). `secret_type::TLS = "kubernetes.io/tls"` (key `tls.crt`, `tls.key`).
- `CreateEngineBody` (`egret-engine-mgmt/src/engines.rs`): body tạo engine (name, warehouseId, kind, resources, trinoConfig…).

## Kiến trúc

### A. Schema — migration mới `migrations/0089_engine_tls_cert_ref.sql`

```sql
-- Optional: back jwt-mode coordinator TLS bằng typed `kubernetes.io/tls` secret
-- người dùng cung cấp (cert chung/công ty) thay cert auto self-signed. NULL giữ
-- hành vi self-signed per-engine hiện tại.
ALTER TABLE engine_mgmt.engines ADD COLUMN tls_cert_secret_ref uuid;
```

### B. Projection primitive (`egret-secrets` + resolver)

- `WorkspaceSecretStore::resolve_typed(workspace: Option<Uuid>, id) -> TypedSecret` — bản typed của `resolve_plaintext` (resolve `secret_ref`, `secrets.get_typed(ref)`).
- Helper (ở `engine-mgmt`, tái dùng): `resolve_tls_cert_secret(secrets, secret_ref) -> (cert_pem, key_pem)` — `resolve_typed` named secret, assert `type == "kubernetes.io/tls"` có `tls.crt` + `tls.key` non-empty, trả `(tls.crt, tls.key)`. (Đây là primitive "project typed-tls secret" mà slice ingress sau tái dùng.)

### C. `ensure_tls_cert` — nhánh cert-cung-cấp

Đầu `ensure_tls_cert`, trước đường self-signed (nhưng reuse `tls_secret_id`/`coordinator_cert` sẵn có vẫn thắng trước cho idempotency):

- Nếu `engine.tls_cert_secret_ref` là `Some`: `resolve_tls_cert_secret` → `(cert_pem, key_pem)`; `keystore = format!("{cert_pem}{key_pem}")`; `coordinator_cert = cert_pem`. Materialize y hệt đường self-signed hiện tại (lưu keystore vào secret `engine-tls-{id}` + set `tls_secret_id`/`coordinator_cert`), để downstream (`{name}-tls` Secret, coordinator mount, connector pin) không đổi.
- Else: đường `generate_self_signed` hiện tại.

`{name}-tls` k8s Secret + mount không đụng — chỉ mang keystore của cert cung cấp. Rotation: cập nhật secret cung cấp → re-provision engine để nhận keystore mới (như config-checksum roll; chi tiết chốt ở plan).

### D. API — engine create mang tham chiếu

- `CreateEngineBody` thêm `tls_cert_secret_ref: Option<Uuid>`; `create_engine` persist vào engine row. (Update theo sau nếu body update cho đổi — quyết ở plan; create-time là tối thiểu.)
- Validate: nếu set, `resolve_tls_cert_secret` phải thành công (secret tồn tại + là TLS secret hợp lệ) — reject create 400 nếu không.

### E. UI — picker TLS-cert optional

- Add-engine dialog (`ControlPlaneEngines.tsx`) thêm `Select` optional **"Coordinator TLS cert"** list các P1a `kubernetes.io/tls` named secret (fetch `/secrets`, filter `type === "kubernetes.io/tls"`) + option "Self-signed (default)". Submit include `tlsCertSecretRef` khi chọn secret. Chỉ ý nghĩa cho engine jwt/TLS-mode (hint đủ; backend bỏ qua với non-TLS).

## Kiểm thử / acceptance

- **Backend** (`egret-secrets` + `egret-engine-mgmt` DB-gated): `resolve_typed` trả data TLS secret; `resolve_tls_cert_secret` trả cert+key và reject non-TLS / thiếu-key; `ensure_tls_cert` dùng cert cung cấp (keystore = cert+key cung cấp, `coordinator_cert` = cert cung cấp) khi `tls_cert_secret_ref` set, và self-sign khi NULL.
- **Live (opt-in, tái dùng đường data-plane e2e):** tạo `kubernetes.io/tls` secret (cert+key test), provision jwt-mode Trino engine tham chiếu nó, bật JWT → coordinator serve HTTPS bằng cert CUNG CẤP (`{name}-tls` Secret mang nó) và egret connect (pin `coordinator_cert` = `tls.crt` cung cấp) → query chạy. Engine tham chiếu NULL vẫn self-sign.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` xanh.

## Rủi ro

1. **Data-plane TLS** — keystore/pin sai làm vỡ HTTPS coordinator + kết nối egret. Verify bằng đường data-plane live (provision + JWT + query). Giữ self-signed fallback làm mặc định.
2. **Cert hợp lệ** — cert cung cấp phải có SAN khớp coordinator host egret dial (`coordinator_host()` = `{name}-trino.{ns}.svc`), nếu không TLS verify fail. Ghi: cert chung/công ty cần SAN đúng (hoặc wildcard); pin `coordinator_cert` chấp nhận cert self-CA.
3. **Thứ tự PEM** — keystore self-signed là `cert + key`; keystore cung cấp phải cùng thứ tự (`tls.crt` rồi `tls.key`); `coordinator_cert` = chỉ `tls.crt`.
4. **Migration re-embed** — migration mới `0089` cần touch `metastore/src/lib.rs` ([[egret-sqlx-migration-staleness]]).

## Tiếp theo

StarRocks FE-TLS-từ-secret (cùng primitive) và Ingress TLS (chart `tls:` tham chiếu k8s TLS Secret đã project) — mỗi cái spec → plan riêng. P1b (storage/DB → basic-auth) vẫn gác lại.
