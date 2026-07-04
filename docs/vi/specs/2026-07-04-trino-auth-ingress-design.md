# Trino authentication + Ingress expose (Phase 1.6a, 2026-07-04)

Bản EN (canonical): `docs/en/specs/2026-07-04-trino-auth-ingress-design.md`.
Nối tiếp Phase 1.5.2 (#142).

**Mục tiêu:** Expose Trino an toàn để dùng thật:
1. **Auth trên chính Trino** — PASSWORD (file-based, cho client ngoài: BI/JDBC/CLI
   user+pass) + JWT (cho egret query nội bộ qua `EngineToken` đã có, validate với JWKS IdP).
2. **Ingress** — k8s `Ingress` (nginx) cho coordinator Service, client ngoài tới Trino
   trực tiếp (TLS ở ingress). **egret KHÔNG proxy query** — ý tưởng reverse-proxy đã bỏ.

**Pivot:** bỏ egret query-proxy (thừa); ingress lo expose, Trino lo auth. Monitoring
(trang monitor native qua REST API Trino) = **Phase 1.6b**, spec riêng.

**Non-goals:** query proxy (bỏ), monitoring (1.6b), autoscaling (1.5.4), observability
(1.5.5), pod/scheduling (1.5.3), OAuth2/certificate auth.

## Chart-verified render
- `http-server.authentication.type={PASSWORD,JWT}`.
- PASSWORD → `password-authenticator.properties` (name=file, `file.password-file=.../auth/password/password.db`)
  + `password.db` (htpasswd: `username:bcrypt-hash`).
- JWT → `http-server.authentication.jwt.key-file=<JWKS>` (+ required-audience?).
- TLS ở ingress → `allow-insecure-over-http=true` + `process-forwarded=true`.
- Ingress `networking.k8s.io/v1` (className/host/tls/annotations).

## Data model (thêm vào TrinoConfig)
- `auth?{ passwordUsers:[{username,secretRef}], jwt?{jwksUri,requiredAudience?} }` (coordinator).
  authentication.type suy ra: PASSWORD nếu có passwordUsers, JWT nếu có jwt, cả hai nếu cả hai.
- `ingress?{ enabled, className?, host, tlsSecretRef?, annotations? }`.

## Orchestrator
- config.properties (coordinator): auth type + allow-insecure + process-forwarded + jwt key-file.
- `password-authenticator.properties` + `password.db` (`username:bcrypt`, orchestrator nhận hash SẴN
  → không cần bcrypt trong crate kube). Mount /etc/trino/auth/password/.
- `trino_ingress(spec)`: Ingress trỏ `{engine}-trino` Service, apply khi `ingress.enabled`, xoá khi tắt/delete.

## Control-plane
- Carrier + **bcrypt** password (resolve secret → bcrypt → `username:hash` gửi orchestrator; bcrypt ở
  control-plane, không ở kube layer). JWKS mặc định = Keycloak realm (nơi EngineToken phát ra) nếu bỏ trống.
  Provision Ingress qua provisioner.

## Frontend
Card **"Exposure & Auth"** (EngineDetail): password users (username + secret Select), JWT toggle (+ jwks),
Ingress (enable + host + className nginx + TLS secret + annotations KV). Save → roll coordinator + apply Ingress.

## Test
Unit (authentication.type + allow-insecure + jwt key-file + password.db bcrypt + Ingress resource) +
live kind (bật password user → curl coordinator `-u bob:pw` → 200, sai → 401; Ingress render verify,
routing thật cần nginx-ingress controller — opt-in).

Compat: field optional, engine cũ no-auth, không migration. allow-insecure an toàn (TLS ở ingress, coordinator nội bộ).

Chi tiết đầy đủ xem bản EN.
