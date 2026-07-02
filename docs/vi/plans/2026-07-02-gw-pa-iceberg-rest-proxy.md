# Gateway P-A: proxy Iceberg-REST trong suốt (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-gw-pa-iceberg-rest-proxy.md`. Spec: `.../2026-07-02-polaris-gateway-machine-idp-design.md` (Trụ 1). Thực thi qua subagent-driven-development.

**Mục tiêu:** egret là ingress duy nhất tới Iceberg REST của Polaris — reverse-proxy **trong suốt** tại `/catalog/v1/*`, forward sang Polaris + rewrite URL nội bộ trong `GET /v1/config`. Proxy **không** tự auth (Polaris auth).

**Kiến trúc:** module nhỏ trong egret-server mount `/catalog/v1/*`, forward method + path-suffix + header `Authorization` + query + body sang `{polaris_base}/api/catalog/v1/*` qua `reqwest::Client`. Base URL Polaris lấy từ `polaris_connection` (P4). Chỉ soi response `GET /v1/config` để rewrite `overrides.uri`/`s3.signer.uri` về URL công khai egret.

## Bối cảnh
- `egret-server build_router` nest router theo `AppState` (control-plane ở `/api/control-plane`) — bắt chước cho nest `/catalog/v1`.
- `PolarisConnStore.get().base_url` (P4) = base Polaris; Iceberg REST base = `{base}/api/catalog`.
- URL công khai egret: config `EGRET_CATALOG__PUBLIC_URL` (default `http://localhost:8080/catalog`) — thêm nếu chưa có.
- Axum wildcard `/catalog/v1/{*rest}` + route riêng `/catalog/v1/config`; forward body streaming.

## Task
- **T1 (scaffold + rewrite):** crate `egret-catalog-proxy` (`ProxyState`, `catalog_proxy_router`); pure fn `rewrite_config_urls(body, internal_base, public_base)` = string-replace internal→public + validate JSON. Test rewrite `overrides.uri`+`s3.signer.uri`.
- **T2 (forward handler):** `proxy(State, Request)` — strip prefix `/catalog/v1/`, build `{base}/v1/{rest}{query}`, forward method+`Authorization`+`Content-Type`+body qua reqwest, trả status/body; nếu `GET config` success → `rewrite_config_urls`. `ProxyError`→502/400, **không log token**. Test forward method/path/header/body (wiremock upstream).
- **T3 (wire):** config `catalog.public_url`; `egret-bin` đọc base từ `PolarisConnStore.get()` lúc startup (chưa cấu hình → skip mount + log), dựng `ProxyState`, thread `Option<ProxyState>` vào `AppState`; `egret-server` nest `catalog_proxy_router` ở root **không CSRF/không auth**. build workspace.
- **T4 (verify live):** Polaris up + serve → `curl -H "Authorization: Bearer $TOK" localhost:8080/catalog/v1/config` → `overrides.uri`/`s3.signer.uri` = URL egret (không phải polaris:8181). fmt+clippy+test.

## Ghi chú
- **Không thêm auth** — forward `Authorization` nguyên xi, Polaris auth (P-D). Không log token.
- Ngoài P-A: egret mini-IdP cấp token (P-B), Polaris tin egret (P-D), RBAC UI (P-C).
