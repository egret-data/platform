# Typed Secrets P0 — Core model + backward-compat — Thiết kế

**Ngày:** 2026-07-18 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/typed-secrets-p0` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-18-typed-secrets-p0-design.md`.

## Vấn đề

Secret store của egret là single-value: một `secrets` row giữ MỘT chuỗi AES-256-GCM-sealed + nhãn `kind` lỏng (`db_password`, `workspace_secret`…). Mọi credential — storage key, DB password, catalog token, OIDC refresh — là một chuỗi opaque, tham chiếu qua `secret_ref` ở ~113 chỗ. Không có cách model **typed, multi-field** như Kubernetes (`Opaque` map key→value, `kubernetes.io/tls` = cert+key, `kubernetes.io/basic-auth` = user+password), chặn mục tiêu dùng một **TLS secret** back config-dùng-cert (cert toàn công ty) và cuối cùng unify mọi credential dưới một model typed.

## Mục tiêu — sub-project này (P0)

**Chỉ nền móng**: mở rộng crate `egret-secrets` + schema sang model secret **typed, multi-field** (`type` + `data: map key→sealed-value`, seal riêng mỗi key), có validate key theo type, đồng thời **mọi consumer hiện có chạy y nguyên** qua backward-compat shim. Không HTTP API, không UI, không k8s projection, không migrate consumer — đó là P1–P3.

Đây là P0 của chương trình 4 phase (xem "Bối cảnh chương trình"). Verify cô lập được bằng DB-gated tests của crate.

## Ngoài phạm vi (phase sau)

- **P1** — typed-secret HTTP API + control-plane UI (chọn type, editor TLS/basic-auth) + migrate `WorkspaceSecretStore` / storage / DB connection sang typed model.
- **P2** — project typed secret sang k8s `Secret` thật (`kubernetes.io/tls`) để engine pod / ingress mount (use-case "cert mọi nơi").
- **P3** — migrate consumer còn lại (OIDC, notebook, cron, catalog) + wiring cert toàn công ty; type mới `dockerconfigjson`, `ssh-auth`.

## Bối cảnh chương trình (tầm nhìn full-unify)

Mọi credential cuối cùng chảy qua một typed-Secret model (k8s-shaped: `type` + `data` map, project được sang k8s Secret, dùng mọi nơi). Thứ tự: **P0 core model** → P1 API/UI/consumer → P2 k8s projection → P3 consumer còn lại + global cert. Mỗi phase là spec → plan → impl riêng. P0 ship `Opaque` + `kubernetes.io/tls` + `kubernetes.io/basic-auth`; `dockerconfigjson` / `ssh-auth` cắm thêm rẻ sau (1 rule validate + 1 mapping projection — không đổi schema).

## Bối cảnh — bề mặt đã grounding (verify)

- **`secrets`** (migration `0008_database.sql`): `{ id uuid, kind text, nonce bytea, ciphertext bytea, key_id text default 'cfg', created_at }` — một sealed value.
- **`workspace_secrets`** (`0032`): `{ id, workspace_id, name, description, secret_ref → secrets(id), created_at, UNIQUE(workspace_id, name) }` — lớp đặt tên trên vault.
- Consumer tham chiếu `secrets(id)` trực tiếp (vd `databases.secret_id → secrets(id)`) — ~113 `secret_ref`; tất cả lấy một chuỗi.
- **`egret-secrets`** crate: `SecretStore` (`put(kind, plaintext) → Uuid`, `get(id) → String`, `update`, `delete`), `WorkspaceSecretStore` (lớp đặt tên, `NewSecret { name, value }`), `Crypto` (`seal(plaintext) → (nonce, ciphertext)`, `open(nonce, ciphertext) → String`, AES-256-GCM). Crate có DB-gated tests (`crates/egret-secrets/tests/`).
- Migration ở `./migrations`; migration mới không recompile `egret-metastore` tới khi touch `metastore/src/lib.rs` (re-embed) — xem [[egret-sqlx-migration-staleness]].

## Kiến trúc

Mở rộng vault sang lưu trữ typed multi-field; giữ API single-value làm shim.

### A. Schema — migration mới `migrations/00NN_typed_secrets.sql`

1. `ALTER TABLE secrets ADD COLUMN type text NOT NULL DEFAULT 'Opaque';`
2. `CREATE TABLE secret_data ( secret_id uuid NOT NULL REFERENCES secrets(id) ON DELETE CASCADE, key text NOT NULL, nonce bytea NOT NULL, ciphertext bytea NOT NULL, key_id text NOT NULL DEFAULT 'cfg', PRIMARY KEY (secret_id, key) );`
3. **Migrate value cũ:** `INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id) SELECT id, 'value', nonce, ciphertext, key_id FROM secrets;` (mỗi secret legacy thành `Opaque` với một key `value` — `DEFAULT 'Opaque'` đã set `type`).
4. `ALTER TABLE secrets DROP COLUMN nonce, DROP COLUMN ciphertext;` (forward-only; `secret_data` giờ là nguồn sự thật duy nhất cho value). `kind` và `key_id`-trên-secrets không ảnh hưởng; `kind` giữ làm nhãn category legacy song song `type` mới.

### B. Crate — `egret-secrets` typed model

- **Model:** `TypedSecret { type: String, data: BTreeMap<String, String> }` (map plaintext, chỉ trả về khi đọc). Mỗi value seal riêng vào một `secret_data` row (per-key `seal`).
- **`SecretStore` method mới:**
  - `put_typed(kind: &str, type_: &str, data: &BTreeMap<String,String>) -> Uuid` — validate required key theo type, insert `secrets` row (`kind`, `type`), seal + insert một `secret_data` row mỗi entry (transactional).
  - `get_typed(id) -> TypedSecret` — đọc `type` + tất cả `secret_data` row, `open` từng cái.
  - `get_key(id, key) -> String` — đọc + open một `secret_data` row.
  - `update_typed(id, data)` — re-validate + thay các `secret_data` row.
- **Backward-compat shim (chữ ký không đổi):**
  - `put(kind, plaintext)` = `put_typed(kind, "Opaque", { "value": plaintext })`.
  - `get(id)` = `get_key(id, "value")`.
  - `update(id, plaintext)` = thay key `value`.
  - ⇒ mọi consumer hiện có (~113 `secret_ref`, `WorkspaceSecretStore`, storage/DB connection) chạy y nguyên.
- **Type registry / validate** (`secret_type.rs`): bảng nhỏ `type → required keys`: `Opaque` (không), `kubernetes.io/tls` (`tls.crt`, `tls.key`), `kubernetes.io/basic-auth` (`username`, `password`). `put_typed`/`update_typed` từ chối data thiếu required key. Type lạ → từ chối (chỉ 3 type đăng ký ở P0).

### C. Tests (crate, DB-gated)

Mở rộng `crates/egret-secrets/tests/`:
- put_typed + get_typed round-trip một Opaque map nhiều key, một TLS (`tls.crt`+`tls.key`), và một basic-auth.
- Validate từ chối TLS thiếu `tls.key`, và type lạ.
- **Backward-compat shim:** `put`/`get` vẫn round-trip một value (lưu dưới `Opaque{value}`); `get_typed` trên secret shim-write hiện `type=Opaque, data={value}`.
- (Migration được exercise bởi suite hiện có chạy trên DB đã migrate — đường `get(id)` cũ giờ đọc `secret_data`.)

## Kiểm thử / acceptance

- `cargo test -p egret-secrets` (DB-gated) — test mới + cũ pass (test single-value cũ chứng minh shim + migration).
- `cargo fmt --all --check` + `cargo clippy -p egret-secrets` sạch. `cargo build` toàn workspace compile (migration re-embed sau khi touch `metastore/src/lib.rs`).
- Không đổi hành vi consumer hiện tại — phase này thuần additive + migration lưu-trữ trong suốt.

## Rủi ro

1. **Migration đúng** — mọi value `secrets` legacy phải vào `secret_data(key='value')` TRƯỚC khi drop cột cũ; migration copy trong cùng file trước DROP. Forward-only (không rollback), đúng style migration egret.
2. **sqlx staleness** — migration mới không hiệu lực trong `serve`/tests tới khi touch `metastore/src/lib.rs` re-embed; đã ghi.
3. **Transactional** — `put_typed`/`update_typed` phải ghi `secrets` row + tất cả `secret_data` row atomic (một transaction) để không tồn tại secret dở dang.
4. **key_id / rotation** — `key_id` per-key mang theo (dự phòng rotation), như hiện tại; không thêm logic rotation ở P0.

## Tiếp theo (P1)

Typed-secret HTTP API + control-plane UI (chọn type, editor TLS cert/key + basic-auth), và chuyển `WorkspaceSecretStore` + storage/DB connection sang `put_typed`/`get_typed`. Spec → plan riêng.
