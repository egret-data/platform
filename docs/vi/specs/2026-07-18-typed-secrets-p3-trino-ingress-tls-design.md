# Typed Secrets P3 — Trino ingress TLS từ typed secret — Thiết kế

**Ngày:** 2026-07-18 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/typed-secrets-p3-trino-ingress-tls` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-18-typed-secrets-p3-trino-ingress-tls-design.md`.

## Vấn đề

Một Trino engine đã provision có thể được expose ra ngoài qua Ingress (`trino_ingress`, `egret-orchestrator/src/trino.rs:982`). TLS của nó wire qua `ingress.tlsSecret` — một **chuỗi thô = tên k8s `Secret` có sẵn** mà admin phải tự tạo trong namespace engine. Không có gì nối nó với typed-secret store của egret: admin không thể chọn 1 cert công ty/host `kubernetes.io/tls` từ Secrets console để ingress dùng. Đây là consumer "cert everywhere" thứ hai (sau coordinator TLS của P2), và là cái ĐẦU TIÊN build primitive **"project typed-tls secret → k8s `kubernetes.io/tls` Secret thật"** mà P2 đã earmark (P2 build `keystore.pem` của Trino — shape khác).

## Mục tiêu — sub-project này (P3-ingress)

Cho Ingress của coordinator Trino tham chiếu 1 typed `kubernetes.io/tls` secret (chọn trên UI) cho TLS. egret **project** typed secret đó thành k8s `type: kubernetes.io/tls` Secret thật (`{name}-ingress-tls`) trong namespace engine lúc provision; Ingress render trỏ tới qua `spec.tls[].secretName`. Một cert công ty/host có thể back nhiều ingress của nhiều engine. Backward-compatible: `tlsSecret` thô (tên k8s Secret có sẵn) vẫn chạy.

## Ngoài phạm vi

- TLS cho ingress **của chính egret** (`egret.localtest.me`) — cấu hình qua Helm chart (deploy-time), operator lo riêng, không phải egret runtime.
- StarRocks FE ingress TLS (cùng primitive, slice sau).
- Reconciler projection đứng riêng / rotate cert không cần re-provision (projection xảy ra lúc apply, như `ensure_tls_cert` của P2).
- P1b (storage/DB → typed basic-auth).

## Bối cảnh — bề mặt đã grounding (verify)

- **`trino_ingress(spec) -> Option<Value>`** (`trino.rs:982`): render Ingress `networking.k8s.io/v1` cho coordinator Service `{name}-trino` (host, path `/`, backend port 8080), `ingressClassName` từ `class_name`, và — khi `ing.tls_secret` Some — `spec.tls = [{ hosts:[host], secretName: ts }]`. **TLS terminate tại ingress; Trino nội bộ vẫn HTTP.**
- **`Ingress` render struct** (`egret-engine-mgmt-api/src/lib.rs:253`): `{ enabled, class_name, host, tls_secret: Option<String>, annotations }`.
- **`parse_ingress`** (`engines.rs:1180`): dựng `IngressReq` từ wire object `ingress { enabled, className, host, tlsSecret, annotations }`.
- **Engine `config` là JSON blob** (`serde_json::Value`, `store.rs:40`) — ingress nằm trong đó. **KHÔNG cần migration** để thêm field.
- **Tái dùng primitive P2:** `resolve_tls_cert_secret(st, secret_ref) -> (cert_pem, key_pem)` (assert `type == kubernetes.io/tls`, `tls.crt`/`tls.key` non-empty); pattern picker coordinator-cert (`ControlPlaneEngines.tsx`).
- **UI:** `EngineSecurity.tsx` đã lộ `ingressEnabled/Host/ClassName/TlsSecret/Annotations`; `ingressTlsSecret` hiện là free-text.
- **Apply:** `apply_all` (`trino.rs:1152`) apply Ingress chỉ khi enabled; các object khác (vd keystore `{name}-tls`) apply cùng.

## Kiến trúc

### A. Projection primitive (mới — cái đã earmark)

Trong `egret-orchestrator`: từ `(cert_pem, key_pem)` render k8s Secret

```json
{ "apiVersion": "v1", "kind": "Secret", "type": "kubernetes.io/tls",
  "metadata": { "name": "<engine>-ingress-tls", "namespace": "<ns>",
                "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": "<engine>" } },
  "stringData": { "tls.crt": "<cert_pem>", "tls.key": "<key_pem>" } }
```

Đây là primitive tái dùng "project typed-tls secret → k8s `kubernetes.io/tls` Secret thật" (StarRocks FE ingress / consumer khác tái dùng). Cert+key lấy từ `resolve_tls_cert_secret` (P2).

### B. Config surface (không migration)

- `Ingress` render struct + `IngressReq` thêm `tls_cert_secret_ref: Option<Uuid>`; `parse_ingress` đọc `ingress.tlsCertSecretRef`.
- **Resolve lúc apply** (trong đường apply/provision, trước khi render Ingress): nếu `ingress.tls_cert_secret_ref` Some → `resolve_tls_cert_secret` → project Secret `{name}-ingress-tls` → set render `Ingress.tls_secret = "{name}-ingress-tls"`. Nếu chỉ có `tls_secret` thô → giữ nguyên (back-compat). Ưu tiên: `tls_cert_secret_ref` (typed) thắng `tls_secret` thô khi cả hai set.
- **Validate** ở create/update: ref set → `resolve_tls_cert_secret` phải OK (secret tồn tại + là TLS secret hợp lệ) — else 400 (như P2).

### C. Orchestrator apply + GC

- Apply projected `{name}-ingress-tls` Secret **trước** Ingress trong `apply_all` (để nginx thấy Secret khi reconcile). Update = re-project (rotation = update typed secret rồi re-provision — như config-checksum roll P2).
- **GC (đã chốt):** projected Secret gắn nhãn `app.kubernetes.io/managed-by: egret` + `egret/engine: {name}` và **best-effort delete khi xoá engine** (dọn luôn nợ keystore `{name}-tls` sót lại đã quan sát; áp cùng delete cho ingress-tls Secret, và — nếu rẻ — cả keystore Secret).

### D. UI (`EngineSecurity.tsx`)

- Thay free-text `ingressTlsSecret` bằng **picker typed-TLS-secret** (`Select` list `type === "kubernetes.io/tls"` từ `/secrets` + option "None" mặc định), theo pattern picker coordinator-cert P2 (Radix sentinel cho option rỗng). Submit include `ingress.tlsCertSecretRef` khi chọn. `tlsSecret` thô vẫn hỗ trợ ở API-level (back-compat) nhưng UI dẫn bằng picker.

## Luồng dữ liệu

1. Admin tạo `kubernetes.io/tls` secret cert công ty/host (console P1a). SAN phải khớp ingress host.
2. Engine → Security → Ingress: enable, set host (vd `trino-p3.localtest.me`), chọn TLS secret.
3. Provision/update: egret resolve typed secret → project `{name}-ingress-tls` (tls.crt/tls.key) → render Ingress coordinator với `spec.tls[].secretName = {name}-ingress-tls`.
4. nginx-ingress terminate TLS bằng company cert cho host engine; Trino nội bộ vẫn HTTP.

## Kiểm thử / acceptance

- **Unit (orchestrator):** projection render Secret `type: kubernetes.io/tls` đúng keys `tls.crt`/`tls.key`, name `{name}-ingress-tls`, nhãn managed-by; `trino_ingress` set `secretName` = tên projected khi ref set, fallback `tls_secret` thô khi không; ưu tiên typed-ref khi cả hai set.
- **DB-gated (engine-mgmt):** create/update validate ref (400 non-TLS/thiếu key); `parse_ingress` round-trip `tlsCertSecretRef`.
- **Live (opt-in, kind):** tạo `kubernetes.io/tls` secret (SAN = ingress host), provision Trino ingress-enabled + host + typed ref → verify Secret `{name}-ingress-tls` mang cert cung cấp, Ingress có `spec.tls[].secretName`, `curl https://<host>/v1/info` (qua nginx-ingress) serve cert cung cấp. Engine dùng `tlsSecret` thô vẫn chạy.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` xanh.

## Rủi ro

1. **SAN cert = ingress HOST** — SAN cert cung cấp phải khớp **ingress host** (vd `trino-p3.localtest.me`), KHÔNG phải coordinator svc host (`{name}-trino.{ns}.svc`) như P2. Yêu cầu SAN khác; ghi rõ. (nginx terminate + serve cert cho client ngoài, họ verify theo host.)
2. **Không bắt buộc CA:FALSE ở đây** — nginx (openssl) dễ dãi với `basicConstraints` của leaf, khác reqwest/rustls ở P2. Vẫn nên dùng leaf cert chuẩn, nhưng lỗi `CA:TRUE`-leaf của P2 không áp cho đường ingress-termination.
3. **GC** — dọn projected Secret là best-effort khi xoá engine; delete fail để lại orphan Secret vô hại (có nhãn managed-by egret để dọn tay).
4. **Thứ tự apply** — projected Secret phải apply trước Ingress để nginx resolve `secretName` ngay lần reconcile đầu.
5. **Rotation** — update typed secret cần re-provision engine để re-project k8s Secret (không có reconciler đứng riêng trong scope).

## Tiếp theo

StarRocks FE ingress TLS (cùng primitive) và các consumer P3 còn lại (OIDC/notebook/cron) — mỗi cái spec → plan riêng. Own-ingress Helm TLS của egret và P1b (storage/DB → basic-auth) vẫn ngoài slice này.
