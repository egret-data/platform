# Make a Published Data API Endpoint Callable From Outside the Cluster

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** A consumer outside the cluster can fetch a token and call a published endpoint at the URLs egret itself advertises.

**Architecture:** Split one overloaded config value into its two real audiences — engines inside the cluster, and consumers outside it — and route the two consumer-facing paths at the edge.

**Tech Stack:** Rust (axum, figment config), Helm, nginx.

## The defect, as measured on kind (main `633954c4`, 2026-08-20)

A published endpoint is not callable from outside, for two independent reasons:

1. **Nothing routes it.** The serving router mounts at `/data-api/v1/{ws}/{*path}` (`crates/egret-server/src/lib.rs:369`) and the machine-IdP token endpoint at `/oauth/token` — but `deploy/helm/egret/templates/ingress.yaml` has only `/auth`→keycloak and `/`→frontend, and `frontend/nginx-prod.conf` proxies only `location /api/`. Measured through the ingress: `GET /data-api/v1/demo/kind/smoke` → **200 with `text/html`** (the SPA), `POST /oauth/token` → **405 text/html**. Straight to the backend via port-forward the same paths give **401 `{"code":"key_invalid"}`** and **415** — so the backend is fine and only the edge is missing.
2. **The advertised URLs are cluster-internal.** `openapi_gen.rs:318` builds `servers[0].url` as `{issuer}/data-api/v1/{slug}` and the `tokenUrl` as `{issuer}/oauth/token`; `issuer` is `egret_public_url` ← `config.catalog.public_url` (`crates/egret-bin/src/main.rs:1356`), which the chart deliberately sets to `http://egret.<ns>.svc.cluster.local:8080` so **engine pods** can fetch the catalog and JWKS. Live value served to a consumer: `http://egret.egret-system.svc.cluster.local:8080/data-api/v1/demo`.

**This pre-dates the trash work; it comes from P5.** It had never been measured because nothing had called a published endpoint from outside.

## The shape of the fix, and why

One value is being asked to name two different networks. Engines inside the cluster must reach egret by its in-cluster Service DNS; consumers outside must reach it by its ingress hostname. Both are legitimate and simultaneous, so they need two settings, not one changed setting.

**Do NOT simply repoint `catalog.public_url` at the ingress host.** It is the machine-IdP `iss`, it is byte-compared by StarRocks' `required_issuer`, and it is baked into every already-provisioned engine's catalog ConfigMap. Changing it is a live-cluster migration hazard, and it would send engine traffic out to the edge and back in.

**The token keeps being minted by the same backend**, so a consumer fetching a token from the external host still receives one whose `iss` is the internal issuer, and the serving path still validates it. The two audiences do not need the same origin — only a reachable one each.

## Global Constraints

- **`st.issuer` has FOUR call sites in `egret-dataapi` and they are NOT the same kind of thing.** Three are consumer-facing URLs and must move to the new value; one is a token claim and must NOT:
  - `routes.rs:788` — `token_endpoint` handed to the consumer at client issue → **move**
  - `openapi_gen.rs:221` — base for `servers[0].url` and `tokenUrl` → **move**
  - `try_it.rs:386` — the URL and `curl` line shown to the author → **move** (today it prints a hostname the author cannot resolve)
  - `try_it.rs:236` — the `"iss"` claim of a minted token → **LEAVE ALONE**
- **`servers[0].url` and `tokenUrl` must keep coming from the same base.** `openapi_gen.rs:312` states why: if they could disagree, one document would send a consumer to fetch a token from one host and call another. Preserve that property with the new value.
- **Default to `issuer` when the new setting is absent**, so every existing deployment and every test keeps its current behaviour. This must be a widening, not a change.
- `openapi.json` is a committed artifact gated by `egret-server`'s `committed_openapi_json_is_current` — a DIFFERENT crate. Regenerate and run that gate if any utoipa annotation changes.
- Backend tests are DB-gated: a missing `DATABASE_URL` makes them report `ok` while asserting nothing (measured: 0.00s unset vs ~20s set). Use `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test"`. NEVER point tests at the dev `egret` database.
- Run tests unpiped; NEVER pipe into `head`/`tail` — SIGPIPE kills a run mid-suite while exiting 0.
- `cargo fmt` and `cargo clippy -- -D warnings` clean. **Commit before cutting any mutation.**
- A kind cluster is live with the dev stack; do not mutate its data.

---

### Task 1: A separate consumer-facing base URL

**Files:**
- Modify: `crates/egret-config/src/lib.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-dataapi/src/routes.rs`, `crates/egret-dataapi/src/openapi_gen.rs`, `crates/egret-dataapi/src/try_it.rs`
- Test: `crates/egret-dataapi/tests/openapi_gen.rs`, `crates/egret-dataapi/tests/try_it.rs`

**Interfaces:**
- Produces: config `data_api.public_url: Option<String>` (env `EGRET_DATA_API__PUBLIC_URL`), and a new `DataApiState` field carrying the resolved consumer-facing origin alongside the existing `issuer`.

- [ ] **Step 1: Add the config field**

`crates/egret-config/src/lib.rs` already has this exact shape at `:243-250` — an `automation` section whose doc comment explains that its `public_url` is deliberately separate from another URL for a different audience. Follow it, and say in the doc comment which audience this one serves and why it is not `catalog.public_url`.

- [ ] **Step 2: Resolve it once, next to the value it falls back to**

`crates/egret-bin/src/main.rs:1356` derives `egret_public_url`. Resolve the new value immediately after, falling back to it, so both derivations sit together and neither can drift. Trim the trailing slash **at the source**, exactly as `egret_public_url` does — the comment there explains that a trailing slash silently breaks StarRocks JWT auth while Trino keeps working.

- [ ] **Step 3: Move the three consumer-facing call sites**

Name the new `DataApiState` field for the audience, not the mechanism — a reader at each call site has to be able to tell it apart from `issuer` at a glance. Add a doc comment on both fields stating which is which, since the whole defect was that one value served two audiences.

- [ ] **Step 4: Write the tests**

Two properties, and they must be able to fail:
1. With the setting present, `servers[0].url` AND `tokenUrl` both carry the new origin, and the minted token's `iss` still carries the OLD issuer. **A test that only checks `servers[0].url` would pass against a change that broke `tokenUrl`, which is the exact failure the code comment at `openapi_gen.rs:312` warns about.**
2. With the setting absent, every one of them is byte-identical to today's output.

Give the two URLs **different hosts** in the fixture. If the fixture sets them to the same value, a fix that ignores the new setting entirely still passes.

- [ ] **Step 5: Run, then mutation-check**

Mutations, each built to zero errors and confirmed before the result is read: point `openapi_gen` back at `issuer` (test 1 must fail); point the `iss` claim at the new value (test 1 must fail on the `iss` assertion); delete the fallback so the value is empty when unset (test 2 must fail).

- [ ] **Step 6: Commit**

---

### Task 2: Route the two consumer paths at the edge

**Files:**
- Modify: `deploy/helm/egret/templates/ingress.yaml`, `deploy/helm/egret/templates/configmap-env.yaml`, `frontend/nginx-prod.conf`

- [ ] **Step 1: Add ingress paths for `/data-api` and `/oauth` → the egret backend Service**

`/auth`→keycloak in the same file is the precedent for a path that bypasses the frontend. Send these to the backend directly rather than through the SPA's nginx: this is machine traffic, and the SPA server has no reason to be in its path.

**Order matters in an Ingress rule** — the `/` catch-all must not shadow the new prefixes. Check how `/auth` is ordered relative to `/` and match it.

- [ ] **Step 2: Set the consumer-facing URL when ingress is enabled**

In `configmap-env.yaml`, set `EGRET_DATA_API__PUBLIC_URL` to the external scheme+host when `ingress.enabled`, and leave it unset otherwise so the fallback applies. The neighbouring `EGRET_CATALOG__PUBLIC_URL` line shows the templating idiom; do not change that line.

- [ ] **Step 3: Add the nginx location too**

`frontend/nginx-prod.conf` proxies `location /api/`. Add `/data-api/` and `/oauth/` beside it. This is belt-and-braces for any deployment that fronts everything with the SPA server rather than the ingress, and it costs one block each. Say so in a comment so the duplication reads as deliberate.

- [ ] **Step 4: Verify on kind**

Deploy and measure, do not assume. **Check pod AGE, not the rollout message** — images are tagged `:dev`, so helm reports success while old pods keep running; `just _kind-refresh` is what actually rolls them.

Preserve the release's user-supplied values — run `helm get values egret -n egret-system` FIRST and pass exactly what is there (currently `oidc.enabled`, `ingress.enabled`, `kernel.launcher`).

Then, through the ingress:
- `POST http://egret.localtest.me/oauth/token` must return the backend's JSON, **not** 405 `text/html`.
- `GET http://egret.localtest.me/data-api/v1/demo/kind/smoke` must return `401 {"code":"key_invalid"}`, **not** 200 `text/html`.
- The generated document's `servers[0].url` must start with `http://egret.localtest.me`.

**Then do the thing none of this has ever done: actually call the endpoint.** Issue a client for a service account, fetch a token from the external `/oauth/token`, and call `servers[0].url` + the `paths` key with it. A 200 with rows is the only result that proves the defect is fixed; everything above is a proxy for it.

---

## Self-Review

**Spec coverage:** the two measured causes map to Tasks 1 and 2. **Placeholder scan:** Task 1's test bodies are described rather than written because the fixture idiom differs between the two test files; the falsifiability requirements are stated instead. **Type consistency:** the config key is `data_api.public_url` / `EGRET_DATA_API__PUBLIC_URL` throughout, and the existing field stays `issuer`.
