# Gateway P-A: transparent Iceberg-REST proxy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-polaris-gateway-machine-idp-design.md` (Pillar 1). **P-A of the gateway epic.** Independent of P-B/C/D to compile; the authenticated end-to-end path needs P-B (egret mini-IdP) + P-D (Polaris trusts egret) to be meaningful, but the forwarding itself stands alone. VI reference: `docs/vi/plans/2026-07-02-gw-pa-iceberg-rest-proxy.md`.

**Goal:** egret becomes the sole ingress to Polaris's Iceberg REST catalog: a **transparent reverse-proxy** at `/catalog/v1/*` that forwards to Polaris and rewrites internal URLs in `GET /v1/config` — no auth layer of its own (Polaris authenticates).

**Architecture:** A small module in `egret-server` mounts `/catalog/v1/*`, forwards method + path-suffix + `Authorization` header + query + body to `{polaris_base}/api/catalog/v1/*` via a shared `reqwest::Client`, and streams the response back. The Polaris base URL comes from the registered `polaris_connection` (P4). Only `GET /v1/config` responses are inspected — to rewrite `overrides.uri`/`s3.signer.uri` to egret's public URL.

**Tech Stack:** Rust (axum, reqwest, serde_json), Postgres (`polaris_connection`).

---

## Background the implementer needs

- `egret-server/src/lib.rs` `build_router` merges/nests feature routers behind `AppState`. The control-plane router is nested at `/api/control-plane`; mirror that mounting style for a new `/catalog/v1` nest.
- `egret-controlplane::PolarisConnStore` (from P4) has `get() -> Result<Option<PolarisConn>, CpError>` where `PolarisConn.base_url` is the Polaris base (e.g. `http://polaris:8181` or `http://localhost:8181`). The Iceberg REST base is `{base_url}/api/catalog`. The proxy reads this per request (or caches it).
- egret's public URL (what external clients call) is configured via `EGRET_SERVER__PUBLIC_URL` or derived; check `egret-config` for an existing public-base setting (`rg "public_url|PUBLIC_URL|external_url" crates/egret-config crates/egret-server`). If none exists, add one (`EGRET_CATALOG__PUBLIC_URL`, default `http://localhost:8080/catalog`).
- Axum wildcard capture: `.route("/catalog/v1/{*rest}", any(handler))` gives `Path(rest): Path<String>`; also handle the exact `/catalog/v1/config`. Forward with `axum::body::Body` streaming.

---

## Task 1: Proxy crate/module scaffold + a forwarding client

**Files:** Create `crates/egret-catalog-proxy/` (crate) with `src/lib.rs`; modify root nothing (glob workspace). Alternatively a module in `egret-server` — prefer a small crate for testability.

- [ ] **Step 1: Scaffold**

`Cargo.toml` deps: `axum`, `reqwest` (stream, rustls-tls), `serde_json`, `tokio`, `thiserror`, `bytes`, `http`. `src/lib.rs` exposes `ProxyState { http: reqwest::Client, iceberg_base: String /* resolver */, public_base: String }` and `pub fn catalog_proxy_router() -> Router<ProxyState>`.

- [ ] **Step 2: Failing test — the config URL rewrite (pure fn)**

```rust
// src/rewrite.rs
#[cfg(test)]
mod tests {
    use super::rewrite_config_urls;
    #[test]
    fn rewrites_internal_uris_to_public_base() {
        let body = br#"{"defaults":{},"overrides":{"uri":"http://polaris:8181/api/catalog"},"s3.signer.uri":"http://polaris:8181/api/catalog/v1/aws/s3/sign"}"#;
        let out = rewrite_config_urls(body, "http://polaris:8181/api/catalog", "http://localhost:8080/catalog").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["overrides"]["uri"], "http://localhost:8080/catalog");
        assert_eq!(v["s3.signer.uri"], "http://localhost:8080/catalog/v1/aws/s3/sign");
    }
}
```

- [ ] **Step 3: Implement `rewrite_config_urls`**

```rust
/// Rewrite any occurrence of the internal Iceberg base with the public base in a
/// /v1/config JSON body (string-replace on the internal base is sufficient and
/// covers overrides.uri, s3.signer.uri, and any other advertised URL).
pub fn rewrite_config_urls(body: &[u8], internal_base: &str, public_base: &str) -> Result<Vec<u8>, serde_json::Error> {
    let s = String::from_utf8_lossy(body);
    let replaced = s.replace(internal_base, public_base.trim_end_matches('/'));
    // validate it is still JSON (config responses are JSON); non-JSON passes through unchanged upstream.
    let _: serde_json::Value = serde_json::from_str(&replaced)?;
    Ok(replaced.into_bytes())
}
```

- [ ] **Step 4: Run + commit** — `cargo test -p egret-catalog-proxy rewrites_internal_uris` (PASS).
```bash
git add crates/egret-catalog-proxy && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(proxy): egret-catalog-proxy scaffold + /v1/config URL rewrite"
```
(End the body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.)

---

## Task 2: The forwarding handler

**Files:** Modify `crates/egret-catalog-proxy/src/lib.rs`

- [ ] **Step 1: Failing test — forwards method/path/headers/body (against a stub upstream)**

Use `axum::serve` on an ephemeral port as a fake Polaris that records the request + returns a canned body, OR a `wiremock` upstream. Assert: a `POST /catalog/v1/namespaces/ns/tables` with header `Authorization: Bearer XYZ` + JSON body arrives at the upstream as `POST /api/catalog/v1/namespaces/ns/tables` with the same header + body, and the upstream status/body flow back.

```rust
#[tokio::test]
async fn forwards_method_path_headers_body() {
    // start a wiremock upstream expecting POST /api/catalog/v1/namespaces/ns/tables
    //   with header authorization=Bearer XYZ, returning 200 {"ok":true}
    // build the proxy Router with iceberg_base = mock uri + "/api/catalog", send the request,
    // assert 200 + {"ok":true} + the mock recorded the forwarded request.
}
```

- [ ] **Step 2: Implement the handler**

```rust
async fn proxy(State(st): State<ProxyState>, req: Request) -> Result<Response, ProxyError> {
    let (parts, body) = req.into_parts();
    let rest = parts.uri.path().strip_prefix("/catalog/v1/").unwrap_or("");
    let q = parts.uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let base = st.iceberg_base(); // "{polaris_base}/api/catalog"
    let url = format!("{base}/v1/{rest}{q}");
    let bytes = axum::body::to_bytes(body, usize::MAX).await?;
    let mut rb = st.http.request(parts.method.clone(), &url).body(bytes.clone());
    // forward Authorization + Content-Type verbatim; DO NOT log the token.
    if let Some(a) = parts.headers.get(http::header::AUTHORIZATION) { rb = rb.header(http::header::AUTHORIZATION, a); }
    if let Some(c) = parts.headers.get(http::header::CONTENT_TYPE) { rb = rb.header(http::header::CONTENT_TYPE, c); }
    let up = rb.send().await?;
    let status = up.status();
    let ct = up.headers().get(http::header::CONTENT_TYPE).cloned();
    let mut out = up.bytes().await?.to_vec();
    // rewrite only the config response
    if parts.method == http::Method::GET && rest == "config" && status.is_success() {
        out = crate::rewrite::rewrite_config_urls(&out, base, &st.public_base).unwrap_or(out);
    }
    let mut resp = Response::builder().status(status);
    if let Some(c) = ct { resp = resp.header(http::header::CONTENT_TYPE, c); }
    Ok(resp.body(axum::body::Body::from(out))?)
}

pub fn catalog_proxy_router() -> Router<ProxyState> {
    Router::new()
        .route("/catalog/v1/config", any(proxy))
        .route("/catalog/v1/{*rest}", any(proxy))
}
```
`ProxyError`: `thiserror` over reqwest/axum/body errors → `IntoResponse` mapping to `502 Bad Gateway` (upstream) / `400` (bad body), with NO token in the message. `ProxyState.iceberg_base()` resolves `{polaris_base}/api/catalog` — inject the base at construction (P4 `PolarisConnStore.get().base_url`) or via a cached resolver; for v1 read it once at wiring time (Task 3) and pass the string in.

- [ ] **Step 3: Run + commit** — forwarding test PASS.
```bash
git add crates/egret-catalog-proxy && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(proxy): transparent forwarding handler for /catalog/v1/*"
```

---

## Task 3: Wire into egret-server

**Files:** Modify `crates/egret-server/src/lib.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-config/*` (public URL)

- [ ] **Step 1: Public-URL config**

Add (if absent) a config field `catalog.public_url` (env `EGRET_CATALOG__PUBLIC_URL`, default `http://localhost:8080/catalog`) in `egret-config`. This is the base external clients use (`{public_url}/v1/...`).

- [ ] **Step 2: Build `ProxyState` + nest**

In `egret-bin/src/main.rs`: read the Polaris base from the registered connection at startup — `PolarisConnStore::new(db).get().await?` → `base_url`; if unset, skip mounting the proxy (log "polaris connection not configured; catalog proxy disabled"). Build `ProxyState { http: reqwest::Client::new(), iceberg_base: format!("{base}/api/catalog"), public_base: config.catalog.public_url }`. Thread it through `AppState` as `Option<ProxyState>` (mirror the `Option<CpState>` pattern).
In `egret-server/src/lib.rs` `build_router`: when `Some`, nest `egret_catalog_proxy::catalog_proxy_router().with_state(proxy_state)` at the root (routes already carry `/catalog/v1/...`). **No CSRF, no auth layer** on this nest (it is a machine API authenticated by Polaris). Add `egret-catalog-proxy` to `egret-server` + `egret-bin` deps.

- [ ] **Step 3: Build**

`cargo build --workspace` → 0 errors.

- [ ] **Step 4: Commit**
```bash
git add crates/egret-server crates/egret-bin crates/egret-config Cargo.lock && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(proxy): mount /catalog/v1 transparent proxy in egret-server"
```

---

## Task 4: Live verification (with Polaris up)

- [ ] **Step 1: Manual proxy check**

With the dev stack (Polaris up) + serve running + a registered Polaris connection: mint a token (dev: Polaris root `client_credentials`, or later an egret Catalog Client token) and:
```bash
TOK=... ; curl -s -H "Authorization: Bearer $TOK" http://localhost:8080/catalog/v1/config | python3 -m json.tool | grep -i uri
```
Expected: `overrides.uri` / `s3.signer.uri` show **egret's** public URL (`http://localhost:8080/catalog`), not `polaris:8181` — proving the rewrite. And `curl .../catalog/v1/catalogs`-style calls proxy through.

- [ ] **Step 2: CI**

`cargo fmt --all --check` + `cargo clippy -p egret-catalog-proxy -p egret-server -p egret-bin --all-targets -- -D warnings` + `cargo test -p egret-catalog-proxy`. Expected: clean/pass.

- [ ] **Step 3: Commit any fixups.**

---

## Self-review notes (spec coverage for P-A)

- Spec **Pillar 1**: transparent forward of `/catalog/v1/*` (Task 2), `/v1/config` URL rewrite (Task 1), mounted with no egret auth layer (Task 3), Polaris reachable only via egret (Task 3 nests the only ingress).
- **No auth added** — `Authorization` is forwarded verbatim; Polaris authenticates (P-D). Token never logged (Task 2).
- **Out of P-A:** the egret mini-IdP that issues the tokens (P-B), Polaris trusting egret (P-D), RBAC UI (P-C).
