# StarRocks FE ingress TLS từ typed secret — Thiết kế

**Ngày:** 2026-07-19 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/starrocks-fe-ingress-tls` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-19-starrocks-fe-ingress-tls-design.md`.

## Vấn đề

P3 (`[[egret-typed-secrets]]`) cho Ingress coordinator **Trino** tham chiếu typed `kubernetes.io/tls` secret, egret project thành k8s `type: kubernetes.io/tls` Secret thật. StarRocks **chưa có ingress nào** trong orchestrator — FE StarRocks không expose ra ngoài được, càng không kèm typed cert. Slice này đưa StarRocks lên parity dùng CHÍNH primitive projection của P3.

## Mục tiêu — slice này

Cho **FE HTTP endpoint (`:8030`, web console / REST API)** của StarRocks expose qua Ingress mà TLS back bằng typed `kubernetes.io/tls` secret, project thành `{name}-ingress-tls` lúc provision — cùng cơ chế P3 làm cho Trino. Backward-compatible (không ingress trừ khi cấu hình); tái dùng struct `Ingress` chung (đã mang `tls_cert_secret_ref` từ P3) + helper projection.

## Ngoài phạm vi — quan trọng

- **Query path StarRocks (`:9030`, MySQL protocol) là raw TCP, KHÔNG thể front bằng HTTP/nginx Ingress.** Slice này chỉ expose FE HTTP (`:8030`). TLS cho query path đã xử lý in-band trên `:9030` bởi delegation (`[[egret-starrocks-delegation]]`), không đổi.
- Bỏ/đổi tên bất kỳ Service StarRocks hiện có.

## Bối cảnh — bề mặt đã grounding (verify)

- **StarRocks không render ingress** (`egret-orchestrator/src/starrocks.rs` — không có `Ingress`/`networking.k8s.io`). Của Trino là `trino_ingress` / `trino_ingress_tls_secret` (`trino.rs`).
- **`Ingress` config chung** (`egret-engine-mgmt-api/src/lib.rs:254`): `{ enabled, class_name, host, tls_secret, tls_cert_secret_ref: Option<Uuid>, tls_cert_pem, tls_key_pem, annotations }`. `EngineSpec.config.ingress` chung mọi kind.
- **FE Service** = `{name}-fe-service` (`starrocks.rs:340`); FE HTTP `http_port = 8030`, query `query_port = 9030` (`starrocks.rs:100`/`102`).
- **Orchestrator StarRocks**: `apply_all` (`starrocks.rs:714`), `update` (`793`), `delete(namespace, name)` (`893`).
- **`build_starrocks_provision_req`** (`egret-engine-mgmt/src/engines.rs:2109`) dựng `EngineSpec` StarRocks; `ensure_tls_cert` gọi ở `2235`. Block resolve-ingress của P3 chỉ thêm vào builder **Trino** (`build_provision_req`) — StarRocks chưa wire.
- **Validate create/update** ingress `tls_cert_secret_ref` (qua `parse_ingress`) của P3 là kind-agnostic — dự kiến đã cover StarRocks (cần verify).
- **UI**: picker ingress `EngineSecurity.tsx` chung — dự kiến đã tới được engine StarRocks (cần verify).

## Kiến trúc (mirror P3)

### A. Helper projection chung (refactor)

Tách body `trino_ingress_tls_secret` (P3) thành free fn tái dùng (module orchestrator chung, hoặc duplicate tối thiểu nếu không có module hợp):

```rust
pub fn ingress_tls_secret(name: &str, namespace: &str, ing: &Ingress) -> Option<Value> {
    if !ing.enabled { return None; }
    let (crt, key) = (ing.tls_cert_pem.as_ref()?, ing.tls_key_pem.as_ref()?);
    Some(json!({
        "apiVersion": "v1", "kind": "Secret", "type": "kubernetes.io/tls",
        "metadata": { "name": format!("{name}-ingress-tls"), "namespace": namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": name } },
        "stringData": { "tls.crt": crt, "tls.key": key }
    }))
}
```
`trino_ingress_tls_secret` thành caller mỏng của fn này (Trino không đổi hành vi).

### B. `starrocks_ingress` render (mới)

Mirror `trino_ingress`, nhưng target FE HTTP Service/port:

```rust
pub fn starrocks_ingress(spec: &EngineSpec) -> Option<Value> {
    let ing = spec.config.ingress.as_ref()?;
    if !ing.enabled { return None; }
    // rules → backend Service `{name}-fe-service` port 8030 (FE HTTP);
    // ingressClassName từ class_name; annotations; và khi `tls_secret` set:
    // spec.tls = [{ hosts:[host], secretName: tls_secret }].
}
```

### C. `apply_all` StarRocks + GC

- Trong `apply_all` (`starrocks.rs:714`): apply `ingress_tls_secret(name, ns, ing)` **trước** `starrocks_ingress(spec)` (cả hai chỉ render khi enabled + có PEM).
- Trong `update` (`793`): khi ingress disabled/absent, best-effort delete Secret `{name}-ingress-tls` cũ + Ingress (mirror P3 Trino `update`).
- Trong `delete` (`893`): best-effort delete `{name}-ingress-tls` + object Ingress.

### D. `build_starrocks_provision_req` — resolve ingress

Thêm cùng block P3 thêm vào builder Trino, chỉnh theo binding `config` local của StarRocks (guard `config.ingress.as_mut()`; resolve `tls_cert_secret_ref` qua `resolve_tls_cert_secret`; inject `tls_cert_pem`/`tls_key_pem`; set `tls_secret = "{name}-ingress-tls"`).

### E. Validate + UI (verify, khả năng no code)

- Xác nhận create/update đã validate ingress `tls_cert_secret_ref` cho StarRocks (đường `parse_ingress` kind-agnostic từ P3). Chỉ thêm check vào path StarRocks nếu phát hiện gap.
- Xác nhận picker ingress `EngineSecurity.tsx` tới được engine StarRocks. Chỉ thêm wiring nếu phát hiện gap.

## Kiểm thử / acceptance

- **Unit (orchestrator):** `starrocks_ingress` render Ingress backend `{name}-fe-service`:8030 + `spec.tls[].secretName` khi `tls_secret` set, `None` khi disabled; helper chung `ingress_tls_secret` render Secret `type: kubernetes.io/tls` `{name}-ingress-tls`; test Trino không đổi sau refactor.
- **Live (opt-in, kind):** tạo `kubernetes.io/tls` secret (SAN = FE ingress host), provision StarRocks engine ingress-enabled + host + typed ref → verify Secret `{name}-ingress-tls` mang cert cung cấp, Ingress có `spec.tls[].secretName`, `curl https://<host>/api/bootstrap` (hoặc path FE HTTP) qua nginx serve cert cung cấp → GC khi delete.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` xanh.

## Rủi ro

1. **SAN = ingress host** (như P3); nginx dễ dãi leaf (không cần CA:FALSE).
2. **Tên/port FE Service** — xác nhận đúng FE HTTP Service (`{name}-fe-service`) + port (8030) render target; backend sai → 502 qua nginx.
3. **Refactor helper chung** — tách `ingress_tls_secret` không được đổi Secret render của Trino (byte-identical); test unit Trino canh việc này.
4. **Builder provision StarRocks** — block resolve-ingress phải vào `build_starrocks_provision_req` (path SR), không chỉ builder Trino.

## Tiếp theo

Own-ingress Helm TLS của egret, global company-cert wiring, consumer P3 còn lại (OIDC/notebook/cron), và các type `dockerconfigjson`/`ssh-auth` — mỗi cái spec → plan riêng.
