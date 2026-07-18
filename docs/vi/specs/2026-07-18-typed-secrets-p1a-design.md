# Typed Secrets P1a — Typed-secret console (API + UI) — Thiết kế

**Ngày:** 2026-07-18 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/typed-secrets-p1a` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-18-typed-secrets-p1a-design.md`.

## Vấn đề

P0 ([[egret-typed-secrets]], PR #277) cho `egret-secrets` model typed multi-field kiểu k8s (`type` + `data` map, `Opaque`/`kubernetes.io/tls`/`kubernetes.io/basic-auth`), nhưng phía trên crate chưa gì tạo/quản lý typed secret được: `/secrets` API và control-plane UI vẫn single-value (`{name, value}`), và `WorkspaceSecretStore` (lớp named) chỉ tạo single-value. Nên admin chưa lưu được **TLS cert (cert+key)** — enabler cho "cert mọi nơi" của P2.

## Mục tiêu — sub-project này (P1a)

Một **typed-secret console**: `/secrets` API + control-plane UI tạo / list / edit typed named secret (`Opaque` map key→value, `kubernetes.io/tls` cert+key, `kubernetes.io/basic-auth` user+pass), và `WorkspaceSecretStore` thêm typed create/update. Plaintext KHÔNG bao giờ lộ (list/get chỉ hiện `type` + tên KEY của data). Secret single-value cũ vẫn chạy và hiện `Opaque{value}`.

Đây là P1a; P1b (migrate storage/DB conn sang typed basic-auth) là follow-on riêng.

## Ngoài phạm vi

- **P1b** — migrate storage / DB connection (và giải credential) sang typed basic-auth. Chúng vẫn chạy qua P0 single-value shim ở đây.
- **P2** — project typed secret sang k8s `Secret` thật (mount cert).
- k8s projection, engine wiring, type mới (`dockerconfigjson`/`ssh-auth`).

## Bối cảnh — bề mặt đã grounding (verify)

- **Crate (P0):** `SecretStore::{put_typed(kind, type, data), get_typed(id)→TypedSecret{r#type,data}, get_key, update_typed}`; `secret_type::{OPAQUE, TLS, BASIC_AUTH, validate}`. `put`/`get`/`update` single-value là shim qua `Opaque{value}`.
- **Lớp named:** `WorkspaceSecretStore::create(workspace?, name, description?, plaintext)` = `secrets.put("workspace_secret", plaintext)` + INSERT `workspace_secrets(workspace_id, name, description, secret_ref)`; `WorkspaceSecretMeta { id, name, description, created_at, used_by }` (không lộ value). Global secret = `workspace_id` NULL.
- **API** (`egret-controlplane/src/secrets.rs`): `GET/POST /secrets`, `PUT/DELETE /secrets/{id}`. `CreateSecretInput { name, description?, value }`, `UpdateSecretInput { name?, description?, value? }`, `SecretDto { id, name, description, created_at, used_by }`.
- **UI** (`frontend/src/routes/control-plane/ControlPlaneSecrets.tsx`): dialog "Add secret" một input password; bảng secrets; import `type Secret, type SecretInput` từ api module FE.

## Kiến trúc

### A. `egret-secrets` — lớp named-secret typed

- `WorkspaceSecretStore::create_typed(workspace: Option<Uuid>, name, description?, r#type: &str, data: &BTreeMap<String,String>) -> Uuid` — `secrets.put_typed("workspace_secret", r#type, data)` (validate) + INSERT `workspace_secrets` (cùng rollback duplicate-name như `create`).
- `WorkspaceSecretStore::update_typed(id, r#type, data)` — resolve `secret_ref`, `secrets.update_typed(ref, ...)`.
- `WorkspaceSecretMeta` thêm `r#type: String` + `keys: Vec<String>` — query list/get LEFT JOIN `secrets` (`type`) và aggregate `secret_data.key` (chỉ tên). `create`/`update` cũ giữ (single-value shim).

### B. HTTP API (`egret-controlplane/src/secrets.rs`)

- `CreateSecretInput { name, description?, r#type: Option<String>, data: Option<BTreeMap<String,String>>, value: Option<String> }`:
  - đường typed: `r#type` + `data` (default `r#type = "Opaque"` khi có `data`).
  - alias legacy: có `value` → `data = { "value": value }`, `r#type = "Opaque"`.
  - reject nếu không có `data` lẫn `value`, hoặc có cả hai; `secret_type::validate` enforce required key.
- `UpdateSecretInput { name?, description?, r#type?, data?, value? }` — cùng one-of; rotate typed qua `update_typed`; alias `value` rotate key `value`.
- `SecretDto` thêm `r#type: String` (serialize `type`) + `keys: Vec<String>`. `GET /secrets` + `/secrets/{id}` lộ `type` + `keys` (không bao giờ value).

### C. Control-plane UI (`ControlPlaneSecrets.tsx` + api module)

- **Dialog add/edit:** một **type Select** (`Opaque` / `kubernetes.io/tls` / `kubernetes.io/basic-auth`) điều khiển field:
  - `Opaque` → editor rows key→value (thêm/xoá cặp; mặc định 1 row).
  - `kubernetes.io/tls` → textarea `tls.crt` + `tls.key` (paste PEM).
  - `kubernetes.io/basic-auth` → input `username` + `password`.
  - Submit post `{ name, description?, type, data }`.
- **List:** cột Badge `type` + tên keys (dòng nhỏ "keys: a, b"); không hiện value.
- **api module** (`frontend/src/api/secrets*` hoặc generated): `Secret` thêm `type` + `keys`; `SecretInput` thêm `type` + `data`.

## Kiểm thử / acceptance

- **Backend** (`egret-secrets` DB-gated + `egret-controlplane` unit/DB): `create_typed` + list hiện `type` + `keys` và KHÔNG BAO GIỜ value; typed create/update round-trip Opaque/tls/basic-auth qua API; validate reject TLS thiếu `tls.key`; alias `value` legacy vẫn tạo `Opaque{value}`.
- **Frontend** (vitest): dialog render field đúng theo type; `npm run lint`/`typecheck`/`test`/`build` xanh.
- **Live (opt-in):** tạo một TLS secret trong control-plane UI trên kind deploy; hiện trong list là `kubernetes.io/tls` với keys `tls.crt, tls.key`, không hiện value.

## Rủi ro

1. **API back-compat** — `/secrets` API dùng bởi UI (cập nhật ở đây) và có thể chỗ khác; giữ alias `value` để caller single-value còn chạy.
2. **Không lộ value** — DTO list/get chỉ lộ `type` + tên key; SQL select `secret_data.key`, không bao giờ `ciphertext`.
3. **Từ khoá `type`** — field Rust `r#type`; JSON serialize là `type` (serde rename). FE dùng `type`.
4. **Rollback duplicate-name** — `create_typed` phải giữ hành vi `create` (rollback vault row khi trùng tên workspace_secrets).

## Tiếp theo (P1b)

Migrate storage (access_key + secret_key) và DB (user + password) connection sang typed `kubernetes.io/basic-auth` secret, đọc credential theo key, kèm data migration cho connection hiện có. Spec → plan riêng.
