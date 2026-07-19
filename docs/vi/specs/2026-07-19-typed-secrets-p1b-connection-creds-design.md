# Typed Secrets P1b — Creds storage + DB connection lên typed basic-auth — Thiết kế

**Ngày:** 2026-07-19 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/typed-secrets-p1b-connection-creds` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-19-typed-secrets-p1b-connection-creds-design.md`.

## Vấn đề

Storage và DB connection vẫn mang credential dạng cột identity plaintext + 1 secret single-value (shim Opaque P0), chưa theo model typed thống nhất:

| | Identity non-secret (plaintext) | Nửa bí mật (`secret_ref` single-value → shim Opaque P0) |
|---|---|---|
| **Storage** (`egret-controlplane/src/storage.rs`) | cột `access_key` | `secret_key` |
| **DB** (query registry `Database`) | `options.user` (JSON) | password |

Reads dùng `resolve_plaintext` (1 chuỗi). Không có gì nối cả pair credential với shape typed `kubernetes.io/basic-auth` mà P0 đã hỗ trợ, nên creds này không resolve-theo-key hay project ra k8s Secret một cách thống nhất được.

## Mục tiêu — sub-project này (P1b)

Back cả storage lẫn DB connection bằng typed secret `kubernetes.io/basic-auth` (keys `username` + `password`), đọc creds theo key. **Model hybrid:** basic-auth secret là source-of-truth cho pair credential; connection giữ cột plaintext `access_key`/`user` CHỈ để display/list (không phải bí mật). **Eager migration:** 1 Rust boot backfill idempotent convert secret của MỌI connection cũ sang basic-auth ngay (không lazy). Reads backward-compatible khi rollout (chịu được secret vẫn Opaque).

## Ngoài phạm vi

- Bỏ cột plaintext `access_key`/`user` (hybrid giữ để display).
- Project các basic-auth secret này ra k8s Secret thật để mount vào pod (consumer sau; primitive làm được).
- Consumer P3 còn lại (OIDC/notebook/cron), StarRocks FE ingress TLS.

## Bối cảnh — bề mặt đã grounding (verify)

- **`st.secrets` là `WorkspaceSecretStore`.** Storage/DB mint 1 NAMED secret GLOBAL qua `create(None, name, None, value)` (shim Opaque); `secret_ref` của conn → row `workspace_secrets` → vault `secrets`. Reads dùng `resolve_plaintext(None, ref_id)`.
- **Typed API đã có sẵn:** `WorkspaceSecretStore::create_typed(ws, name, desc, type, data)`, `resolve_typed(ws, ref_id) -> TypedSecret{r#type, data}`; `SecretStore::get_key(id, key)`, `put_typed`, `update_typed`; `secret_type::BASIC_AUTH = "kubernetes.io/basic-auth"` (keys `username` + `password`, có validate).
- **Storage write** (`storage.rs` `resolve_secret_ref`): `new_secret` mint `st.secrets.create(None, ns.name, None, ns.value)`; conn lưu `access_key` + `secret_ref`. **Storage read** (`open_store`, `storage.rs:156`): `resolve_plaintext(None, secret_ref)` → `secret_key`.
- **DB write** (query registry `DatabaseInput`): `new_secret` (mint named) hoặc `secret_id`; `options.user` mang username. **DB read:** registry resolve named secret → password → `ConnSpec.secret` (password basic-auth của connector).
- **Migrations chạy ở boot** (`db.migrate()` trong serve path, `egret-bin/src/main.rs:82`/`163`). Chưa có hook Rust backfill sau migration — sẽ thêm ngay sau `db.migrate()`.

## Kiến trúc

### A. Resolver primitive (`egret-secrets`)

`WorkspaceSecretStore::resolve_basic_auth(ws: Option<Uuid>, ref_id) -> Result<(String /*username*/, String /*password*/)>`:
- `resolve_typed(ws, ref_id)`; nếu `type == kubernetes.io/basic-auth` → trả `(data["username"], data["password"])`.
- **Fallback** (legacy / pre-backfill Opaque): trả `(String::new(), resolve_plaintext(ws, ref_id))` — chỉ password, username rỗng. Giữ reads chạy trước/nếu secret chưa convert.

Tái dùng cho cả storage + DB read.

### B. Write path

- **Storage** (`resolve_secret_ref`): khi mint secret mới, dùng `create_typed(None, name, None, BASIC_AUTH, {"username": access_key, "password": value})` thay `create` Opaque. Conn vẫn lưu `access_key` vào cột (hybrid display).
- **DB** (registry create/update): mint `create_typed(..., BASIC_AUTH, {"username": user, "password": value})` **khi `options.user` non-empty**; nếu không có user, mint Opaque như hiện tại (validate basic-auth đòi `username` non-empty — xem ghi chú dưới).
- **Luật identity rỗng:** `secret_type::validate` đòi `username` có mặt + **non-empty** cho basic-auth. `access_key` của storage là `NOT NULL` nên storage luôn mint basic-auth. DB connection không có `user` giữ secret Opaque (đọc trong suốt qua fallback `resolve_basic_auth`). Tránh đụng validation contract của P0; chỉ DB connection thiếu user (hiếm) mới ở lại Opaque.
- **Wire back-compat:** shape request không đổi — `new_secret.value` → `password`, `access_key`/`options.user` → `username`. Tham chiếu `secret_id`/`secret_ref` sẵn có vẫn chạy (resolve qua `resolve_basic_auth`, chịu Opaque).

### C. Read path

- **Storage `open_store`:** `let (_user, secret_key) = st.secrets.resolve_basic_auth(None, ref_id).await?;` (username hiện từ cột conn). Downstream (`StorageConfig.secret_key`) không đổi.
- **DB (resolve password → `ConnSpec.secret`):** resolve qua `resolve_basic_auth`, truyền password làm `secret`. Username vẫn từ `options.user` cho connector.
- Cả hai giữ fallback Opaque cho an toàn khi rollout.

### D. Eager migration — Rust boot backfill (không SQL migration)

`backfill_basic_auth_credentials(db, secrets)` chạy 1 lần, ngay sau `db.migrate()` trong serve path. Idempotent:
- Liệt kê storage connection + DB connection có `secret_ref`.
- Với mỗi cái, resolve `type` của vault secret; nếu đã `kubernetes.io/basic-auth` → skip.
- Nếu chưa, và identity của conn (`access_key` cho storage, `options.user` cho DB) **non-empty**, đọc value hiện tại (`resolve_plaintext`) rồi rewrite qua `update_typed(BASIC_AUTH, {"username": <identity>, "password": <value hiện tại>})`, transactional per-secret (như `update_typed` vốn có). Conn identity **rỗng** (DB thiếu user, hiếm) để lại Opaque — `resolve_basic_auth` đọc trong suốt.
- Chỉ đụng Opaque secret được storage/DB conn tham chiếu (không đụng `kubernetes.io/tls` hay secret khác). Log số converted (và số skipped-empty).
- **Guard secret dùng chung:** nếu cùng 1 `secret_ref` được nhiều conn với identity khác nhau tham chiếu → first-wins + warn (mỗi conn thường tự mint secret, nên hiếm).

Idempotency không cần bảng marker — chạy lại thấy đã basic-auth thì skip.

### E. UI

Gần như không đổi: form storage + DB đã thu identity (`access_key`/`user`) và secret value; backend map `value → password` và identity `→ username`. Có thể hiện nhãn "basic-auth" nhỏ ở chỗ show type secret — low priority, không bắt buộc.

## Kiểm thử / acceptance

- **Unit (`egret-secrets`):** `resolve_basic_auth` trả `(username, password)` cho basic-auth secret và `("", value)` cho Opaque.
- **DB-gated (`egret-controlplane` + registry):** tạo storage/DB connection mint secret `kubernetes.io/basic-auth` keys `username` + `password`; read resolve password đúng; backfill convert 1 conn Opaque legacy seed sẵn sang basic-auth (giữ password) và no-op ở lần chạy 2.
- **Live (opt-in, kind):** tạo storage connection → verify secret `kubernetes.io/basic-auth` (keys `username`,`password`) và `open_store` chạy (provision warehouse qua nó); tạo DB connection → query chạy; seed conn Opaque legacy, boot backfill, verify convert + vẫn chạy.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` xanh.

## Rủi ro

1. **Backfill atomic/idempotent** — đọc value cũ rồi ghi 2 keys phải atomic + reseal pair (`update_typed` re-validate + reseal); per-secret; skip khi đã basic-auth. Fail giữa chừng không được mất password.
2. **Secret dùng chung** — `secret_ref` chia sẻ nhiều conn identity khác nhau → username mơ hồ; first-wins + warn (hiếm).
3. **Back-compat rollout** — reads phải chịu cả Opaque (trước backfill) lẫn basic-auth; fallback Opaque của `resolve_basic_auth` lo việc này.
4. **Hai nguồn username** — storage `access_key` (cột) vs DB `options.user` (JSON); write + backfill xử lý theo từng loại conn.
5. **Username rỗng** — `secret_type::validate` đòi `username` non-empty cho basic-auth. Đã giải quyết (không đổi validation): conn identity rỗng giữ Opaque + đọc qua fallback; chỉ conn có identity mới thành basic-auth. Storage luôn có `access_key` (NOT NULL) nên luôn convert; chỉ DB conn thiếu user ở lại Opaque.

## Tiếp theo

Project các basic-auth secret này ra k8s Secret thật (DB creds mount pod / S3 creds cho engine) và consumer P3 còn lại (OIDC/notebook/cron), StarRocks FE ingress TLS — mỗi cái spec → plan riêng.
