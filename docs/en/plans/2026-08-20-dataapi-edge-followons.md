# Data API Edge Follow-Ons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Close the three follow-ons PR #525 left open: an exposed token endpoint with no limiter, a catalog URI the UI advertises but the edge does not route, and two discovery paths that answer a lie.

**Architecture:** Lift the existing rate limiter into a shared crate and apply it to the machine-IdP token endpoint; extend the edge routing PR #525 established to the remaining machine paths.

**Tech Stack:** Rust (axum, figment), Helm, nginx.

## Context: what #525 already did

`main` `cfa46a3f` added `data_api.public_url` (`EGRET_DATA_API__PUBLIC_URL`, falling back to `catalog.public_url`), moved three consumer-facing URL sites onto it while leaving the token `iss` claim alone, and routed `/data-api` and `/oauth` to the backend at the edge — proven with a real external call returning `200 {"data":[{"one":1,"who":"svc-kind-smoke-bot"}]}`.

## Global Constraints

- **Do not repoint `catalog.public_url`.** It is the machine-IdP `iss`, StarRocks byte-compares it (`engine_oidc_issuer`), engines fetch JWKS from `{iss}/.well-known/jwks.json`, and it is baked into every provisioned engine's ConfigMap.
- Backend tests are DB-gated: **a missing `DATABASE_URL` makes them report `ok` while asserting nothing** — measured 0.00s unset vs ~20s set. Use `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test"` and report durations. NEVER point tests at the dev `egret` database.
- Run tests unpiped. NEVER pipe into `head`/`tail` — SIGPIPE kills a run mid-suite while exiting 0.
- `cargo fmt` and `cargo clippy -- -D warnings` clean. `openapi.json` is gated by `egret-server`'s `committed_openapi_json_is_current`, a DIFFERENT crate — regenerate and run it if any utoipa annotation changes.
- **Commit before cutting any mutation.** A mutation that fails to COMPILE, or a patch that never lands, proves nothing — build to zero errors, confirm, then read.
- A kind cluster is live with the dev stack. Leave `GET /kind/smoke` in workspace `demo` `live` and non-trashed, and do not delete dev data.

---

### Task 1: Rate-limit the now-public token endpoint

**Files:**
- Move: `crates/egret-auth/src/ratelimit.rs` → `crates/egret-core`
- Modify: `crates/egret-auth/src/routes.rs`, `crates/egret-machine-idp/src/lib.rs`, `crates/egret-config/src/lib.rs`, `crates/egret-bin/src/main.rs`, the two `Cargo.toml`s
- Test: `crates/egret-machine-idp/` tests

**Why:** #525 made `/oauth/token` publicly reachable. `token_endpoint` runs an **Argon2 verify per request** (`crates/egret-dataapi/src/clients.rs:164`) with no limiter anywhere in the path. Unknown `client_id`s short-circuit before the hash (`clients.rs:142`) and secrets are 256-bit random, so this is a CPU-cost knob for anyone holding a non-secret `client_id`, not a credential-guessing risk — but it is now reachable from the internet.

- [ ] **Step 1: Lift the limiter into a shared crate**

`crates/egret-auth/src/ratelimit.rs` is ~50 lines of std-only code with a generic `check(key) -> bool` (plus `check_at` for tests). `egret-machine-idp` depends on `egret-core`, not on `egret-auth`, so the type has to move to `egret-core` — **do not add an `egret-auth` dependency to the IdP; that is the wrong direction.**

Keep `egret-auth`'s existing call site working. Its tests move with it.

- [ ] **Step 2: Add the config keys**

`RateLimitConfig` (`crates/egret-config/src/lib.rs:12`) already holds `login_max_attempts` / `login_window_secs`. Add the token-endpoint pair beside them, with defaults, following how the login defaults are set at `:377-378`.

- [ ] **Step 3: Apply it, keyed the way login is keyed**

Login keys on `ip:email` (`egret-auth/src/routes.rs:172`). Key the token endpoint on **`ip:client_id`**, for the reason login does: keying on `client_id` alone would let anyone lock out a legitimate client by spamming its id, and the `client_id` is not a secret. An attacker then only locks themselves out.

Return **429**, with a body shaped like the endpoint's existing OAuth2 errors — read what `invalid_client` returns and match it rather than inventing a shape.

- [ ] **Step 4: Write the tests**

1. Under the limit, a request with a bad secret still reaches the verifier and returns the normal `invalid_client`.
2. Over the limit, it returns 429 **before** the verifier is consulted.
3. **A different IP with the same `client_id` is unaffected**, and a different `client_id` from the same IP is too.

Use `check_at` with an injected instant rather than sleeping; the limiter already supports it and the existing tests at `ratelimit.rs:62-75` show the idiom.

**Test 3 is the one that carries weight.** A limiter keyed on the whole request, or on `client_id` alone, still passes tests 1 and 2 — those two only prove *something* is limiting. Only test 3 distinguishes the key that avoids the lockout footgun from one that creates it.

- [ ] **Step 5: Run, mutation-check, commit**

Mutations: remove the limiter check (test 2 fails); key on `client_id` alone (test 3 fails); key on `ip` alone (test 3's second half fails).

---

### Task 2: Route the remaining machine paths at the edge

**Files:**
- Modify: `deploy/helm/egret/templates/ingress.yaml`, `frontend/nginx-prod.conf`

**Interfaces:** consumes the pattern #525 added for `/data-api` and `/oauth`; add `/catalog` and `/.well-known`.

- [ ] **Step 1: `/catalog`**

`crates/egret-server/src/lib.rs:375` nests the embedded lakekeeper Iceberg REST catalog at `/catalog/v1`, and `frontend/src/routes/control-plane/WarehouseDetail.tsx:2199` shows the user `${window.location.origin}/catalog` as the catalog URI to paste into Spark and Trino. Measured today: that URL returns the SPA. **The page tells the user a URI that does not work** — and #525 made the token URI on that same page work, so it is now the odd one out.

This is machine traffic with its own bearer-token auth and is already CSRF-exempt (`lib.rs:284`).

- [ ] **Step 2: `/.well-known/openid-configuration` and `/.well-known/jwks.json`**

Both live on the machine-IdP (`crates/egret-machine-idp/src/lib.rs:84-85`). Measured today: both return **`200 text/html`** — a success status with an HTML body, which most OAuth libraries surface as a parse error rather than "not found".

**Route the two exact paths, not the whole `/.well-known/` prefix**, so ACME/`security.txt` and anything else under it stay with the SPA.

**State the limitation in a comment rather than implying this makes external discovery work.** It does not: `discovery()` (`lib.rs:117-124`) builds `issuer`, `token_endpoint` and `jwks_uri` from the in-cluster `st.issuer`, and OIDC requires the document's `issuer` to equal both the `iss` of the tokens and the URL the document was fetched from. A client discovering against the external host will now get JSON it correctly rejects, instead of HTML it cannot parse. That is an improvement in honesty, not a working discovery flow — closing that needs the issuer itself to become externally reachable, which is deliberately out of scope.

`/.well-known/jwks.json` is the one that is genuinely correct served externally: a key set carries no issuer semantics.

- [ ] **Step 3: nginx too**

`frontend/nginx-prod.conf` gained `/data-api/` and `/oauth/` in #525 beside `/api/`. Add the new ones the same way, with the same deliberate-duplication comment.

**Note the ordering trap already present in that file:** the regex location `~* \.(js|css|png|…)$` is evaluated ahead of prefix locations, so a path ending in one of those extensions is served from the static root rather than proxied. `/.well-known/jwks.json` does not end in one of them — check the list yourself rather than trusting this sentence.

- [ ] **Step 4: Verify on kind, then commit**

**Preserve the release's user-supplied values** — run `helm get values egret -n egret-system` FIRST and pass exactly those (`oidc.enabled`, `ingress.enabled`, `kernel.launcher`).

**Images are tagged `:dev`, so `helm upgrade` reports success while the OLD pods keep running.** `just _kind-refresh` does the rollout restart. **Verify by pod AGE, never by the rollout message.** Only the frontend image needs rebuilding if you touch nginx; the backend needs it only if Task 1's Rust is in the same deploy.

Then measure through `http://egret.localtest.me`:
- `/catalog/v1/config` returns the catalog's own JSON, not `text/html`.
- `/.well-known/jwks.json` returns a JSON key set.
- `/.well-known/openid-configuration` returns JSON.
- `/.well-known/acme-challenge/x` still reaches the SPA — proving you routed the two paths and not the prefix.
- `/explore` and `/api/...` are unregressed.

Report the actual status **and content-type** for each; a 200 alone is what made this defect invisible in the first place.

---

## Self-Review

**Spec coverage:** the three follow-ons map to Task 1 (limiter) and Task 2 steps 1-2 (catalog, discovery). **Placeholder scan:** Task 1's test bodies are described rather than written because the IdP's test idiom has not been read yet; the falsifiability requirement is stated instead. **Type consistency:** the limiter keeps its `check`/`check_at` API across the move, and the config keys sit in the existing `RateLimitConfig`.
