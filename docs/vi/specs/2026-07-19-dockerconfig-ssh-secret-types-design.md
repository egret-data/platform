# Typed secrets — type dockerconfigjson + ssh-auth (+ dockerconfigjson → imagePullSecrets) — Thiết kế

**Ngày:** 2026-07-19 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/dockerconfig-ssh-secret-types` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-19-dockerconfig-ssh-secret-types-design.md`.

## Vấn đề

Registry typed-secret có `Opaque` + `kubernetes.io/tls` + `kubernetes.io/basic-auth`. Thiếu 2 type k8s chuẩn: `kubernetes.io/dockerconfigjson` (credential kéo image từ registry private) và `kubernetes.io/ssh-auth` (SSH private key). Cái đầu có consumer egret thật — engine kéo image từ registry private cần pod `imagePullSecrets` — hiện chỉ nhận **tên k8s Secret thô có sẵn** (`config.image_pull_secrets`), không phải typed secret từ store egret.

## Mục tiêu — slice này

1. Thêm 2 type vào registry (validate + toàn bộ typed CRUD/console có sẵn miễn phí).
2. Console editor cho cả hai.
3. Wire `kubernetes.io/dockerconfigjson` vào `imagePullSecrets` của engine **Trino**: engine tham chiếu typed dockerconfigjson secret; egret project thành k8s `type: kubernetes.io/dockerconfigjson` Secret `{name}-pull` lúc provision + thêm vào pod `imagePullSecrets` — cùng pattern ref→resolve→project→wire của P3.

## Ngoài phạm vi

- Wiring imagePullSecrets cho **StarRocks** — pull secrets của SR nằm dưới `starrocksConfig.pod.image_pull_secrets` (pod config JSON thô lồng), path khác Trino; follow-up sau.
- Consumer `ssh-auth` — chưa có consumer egret; slice này chỉ thêm type + console (dùng git/ssh tương lai).

## Bối cảnh — bề mặt đã grounding (verify)

- **`secret_type.rs`** (`egret-secrets`): match `required_keys` — `OPAQUE`/`TLS`/`BASIC_AUTH`. `validate` đòi mỗi key liệt kê có mặt + non-empty. Thêm type = 1 arm (không đổi schema).
- **`config.image_pull_secrets: Vec<String>`** (`egret-engine-mgmt-api/src/lib.rs:349`): tên k8s Secret thô. Trino render vào `pod_spec["imagePullSecrets"]` (`trino.rs:852`). Parse từ config wire `v["imagePullSecrets"]` (`engines.rs:1165`).
- **StarRocks** set `imagePullSecrets` từ `pod.image_pull_secrets` (`starrocks.rs:458`, dưới `starrocksConfig.pod`) — path ngoài phạm vi.
- **Typed secret CRUD + console** (`egret-controlplane/src/secrets.rs`, `ControlPlaneSecrets.tsx`) generic theo `type` + `keys` (P1a) — type mới chỉ cần console editor; API `create_typed`/list/`SecretDto{type,keys}` đã xử lý.
- **Pattern P3 để mirror** cho engine wiring: `tls_cert_secret_ref` trên ingress config → resolve lúc provision (`resolve_tls_cert_secret`) → project `{name}-ingress-tls` → tham chiếu. `resolve_tls_cert_secret` là TLS-specific; cần resolver sibling cho dockerconfigjson.

## Kiến trúc

### A. Type registry (`secret_type.rs`)

```rust
pub const DOCKER_CONFIG_JSON: &str = "kubernetes.io/dockerconfigjson";
pub const SSH_AUTH: &str = "kubernetes.io/ssh-auth";
// required_keys:
DOCKER_CONFIG_JSON => Some(&[".dockerconfigjson"]),
SSH_AUTH => Some(&["ssh-privatekey"]),
```
`validate` đã enforce có mặt + non-empty. KHÔNG cần thêm biến thể `SecretsError` (tái dùng `InvalidType`/`MissingKey`).

### B. Console (`ControlPlaneSecrets.tsx`)

- Thêm cả hai vào type `Select`.
- **Editor dockerconfigjson**: input registry / username / password / email → build `.dockerconfigjson` JSON client-side:
  `{"auths":{"<registry>":{"username","password","email","auth":<base64("user:pass")>}}}` → lưu dưới key `.dockerconfigjson`.
- **Editor ssh-auth**: `Textarea` private-key → key `ssh-privatekey`.
- List vẫn hiện type + keys (không bao giờ value).

### C. dockerconfigjson → Trino imagePullSecrets (mirror P3)

- **Resolver** (`egret-engine-mgmt`): `resolve_dockerconfigjson_secret(st, ref) -> String` (value `.dockerconfigjson`) — `resolve_typed`, assert `type == kubernetes.io/dockerconfigjson`, trả key. (Sibling của `resolve_tls_cert_secret`.)
- **Config field**: `image_pull_secret_ref: Option<Uuid>` trong config (nằm trong config JSON, không migration; parse ở `build_trino_config`/`parse`).
- **Provision** (`build_provision_req`): nếu set, `resolve_dockerconfigjson_secret` → set runtime field mang value `.dockerconfigjson` đã project + append `{name}-pull` vào `config.image_pull_secrets`.
- **Orchestrator**: render `trino_pull_secret(spec)` (Secret `type: kubernetes.io/dockerconfigjson` `{name}-pull` từ value resolved) apply trong `apply_all` trước Deployment; GC trong `delete` (`{name}-pull`).
- **Validate**: create/update resolve ref (400 nếu thiếu / không phải dockerconfigjson).

## Kiểm thử / acceptance

- **Unit (`egret-secrets`):** `validate` chấp nhận `dockerconfigjson`{`.dockerconfigjson`} + `ssh-auth`{`ssh-privatekey`}, reject khi thiếu/rỗng key.
- **Unit (orchestrator):** `trino_pull_secret` render Secret `type: kubernetes.io/dockerconfigjson` `{name}-pull` với data `.dockerconfigjson`; `trino_deployment` gồm `{name}-pull` trong `imagePullSecrets` khi set.
- **DB-gated (engine-mgmt):** create validate `image_pull_secret_ref` sai (400).
- **Live (kind):** tạo dockerconfigjson secret (API) → provision Trino engine tham chiếu → verify Secret `{name}-pull` (`type: kubernetes.io/dockerconfigjson`) + pod coordinator/worker `imagePullSecrets` gồm `{name}-pull` → GC khi delete. Tạo ssh-auth secret qua console → list `kubernetes.io/ssh-auth`, key `ssh-privatekey`.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` xanh.

## Rủi ro

1. **`.dockerconfigjson` đúng dạng** — console build JSON; value sai làm kubelet fail pull image. Giữ console là source-of-truth; backend lưu value opaque.
2. **imagePullSecrets là list** — append `{name}-pull` (không replace) để giữ `image_pull_secrets` thô đã cấu hình; de-dup nếu đã có.
3. **Không migration / không OpenAPI regen** — config field là JSON opaque; console tái dùng API `/secrets` sẵn có.

## Tiếp theo

Wiring dockerconfigjson cho StarRocks `pod.image_pull_secrets`; consumer `ssh-auth` (notebook/pipeline nguồn git) — mỗi cái slice riêng. OIDC/notebook/cron → typed là item typed-secrets còn lại.
