# Give egret's Machine-IdP an Externally Valid Issuer

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** A standard OIDC client, given only egret's public hostname, can discover and use the machine-IdP — without making engines dial the internet to fetch keys.

**Architecture:** Separate egret's *identity* (the `iss` claim, which must be externally valid) from the *addresses engines dial* (which must stay in-cluster). One more application of the address-vs-claim split #525 established for the Data API.

**Tech Stack:** Rust (figment config), Helm.

## Why this is now possible, and why it was not before

`/.well-known/openid-configuration` served from `egret.localtest.me` says:

```
issuer:         http://egret.egret-system.svc.cluster.local:8080
token_endpoint: http://egret.egret-system.svc.cluster.local:8080/oauth/token
```

Asked at one address, answers with another — and that other one does not resolve outside the cluster (`curl` → `000`). OIDC Discovery requires the document's `issuer` to equal both the URL it was fetched from and the `iss` of the tokens; a conforming client rejects this, and a non-conforming one chases an unreachable host.

**Two blockers were removed by #526, without that being its purpose.** Measured from inside the live Trino engine pod, `egret.localtest.me` resolves (CoreDNS rewrite → ingress) and both `/.well-known/jwks.json` (200) and `/catalog/v1/config` (401, the real CatalogGuard) are reachable. Before #526 an engine would have received SPA HTML from both.

## What actually enforces the issuer — measured, and it is not what the comments imply

| engine | enforces `iss`? |
|---|---|
| **Trino** | **No.** `required-issuer` appears nowhere in the codebase. The live coordinator config sets only `jwt.key-file` (a JWKS URL), `jwt.principal-field` and `jwt.required-audience=engine`. It validates signature and audience. |
| **StarRocks** | **Yes.** `crates/egret-engine-mgmt/src/engines.rs:3216` renders `CREATE SECURITY INTEGRATION egret_oidc PROPERTIES(… "required_issuer"="{issuer}" …)`. |

So the blast radius of changing the issuer is **StarRocks security integrations**, not every engine.

## Global Constraints

- **`engine_jwks_uri` must stay in-cluster** (`crates/egret-bin/src/main.rs:1859`). Engines fetching keys through the ingress would make the data plane depend on the ingress being healthy and hairpin traffic out and back. Keys are the same either way; only the address differs.
- **The catalog URI baked into engine ConfigMaps stays in-cluster** for the same reason. Do not touch `catalog.public_url`'s role there.
- **Default to `catalog.public_url` when the new key is absent**, so every existing deployment and test is byte-identical. A widening, not a change — the same discipline #525 used.
- **Trailing slash is trimmed at the source.** `main.rs:1353` explains why: StarRocks compares the issuer raw, so a trailing slash silently breaks StarRocks JWT auth while Trino keeps working — a confusing partial failure.
- Backend tests are DB-gated: a missing `DATABASE_URL` makes them report `ok` while asserting nothing. **Use `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test_t1"`** — the shared `egret_test` holds 3327 workspaces and `egret-auth`'s `pdp_hot_reload` fails there on timing. NEVER point tests at the dev `egret` database.
- `cargo test` without `--no-fail-fast` stops at the first failing binary.
- **A shell hook rewrites `... | grep` into `rtk grep`, which swallows piped stdin** — redirect to a file and grep the file.
- Run tests unpiped; NEVER pipe into `head`/`tail`. `cargo fmt` and `clippy --workspace --all-targets -- -D warnings` clean.
- **Commit before cutting any mutation.**

---

### Task 1: Split the issuer from the in-cluster address

**Files:**
- Modify: `crates/egret-config/src/lib.rs`, `crates/egret-bin/src/main.rs`
- Test: `crates/egret-config/`, plus wherever the discovery document is tested

**Interfaces:** produces config `idp.issuer` (env `EGRET_IDP__ISSUER`), defaulting to `catalog.public_url`.

- [ ] **Step 1: Add the config**

A new `IdpConfig` section — **not** a field on `OidcConfig`, which describes the *external* IdP (Keycloak) egret delegates to, a different thing entirely. `DataApiConfig` (`crates/egret-config/src/lib.rs:168`) is the shape to copy, including its resolver that treats blank **and slash-only** as unset and trims the trailing slash before the emptiness check.

- [ ] **Step 2: Move the five identity bindings, and only those**

Measured on `7dd576ae`. These are `iss` claims or things compared against one — they move together, or a token minted under one value is rejected by a check using another:

| line | what |
|---|---|
| `main.rs:1385` | `engine_minter` — the `iss` of engine tokens |
| `main.rs:1560` | notebook `idp_issuer` |
| `main.rs:1865` | `engine_oidc_issuer` — StarRocks' `required_issuer`, compared against that `iss` |
| `main.rs:1886` | `IdpState.issuer` — the machine-IdP's own identity and its discovery document |
| `main.rs:1928` | `DataApiState.issuer` — the `iss` of the token Try-it mints |

**These stay on `egret_public_url`:** `main.rs:1859` `engine_jwks_uri`, and the catalog URI. They are addresses an engine dials, not identity.

- [ ] **Step 3: Test the invariant this whole change is about**

The load-bearing property: **the `iss` of a minted token equals the `issuer` in the discovery document, and equals the host the document is served from.** Assert all three together. Fixtures must use a `idp.issuer` **different** from `catalog.public_url`, or a change that ignores the new key entirely still passes.

Also assert `jwks_uri` inside the *discovery document* follows the new issuer (it is what an external client dials) while `engine_jwks_uri` (what an engine dials) does not. Those two are now deliberately different, and a test that does not distinguish them would let either drift.

And: with the key absent, every value is byte-identical to today.

- [ ] **Step 4: Run, mutation-check, commit**

Mutations: point `IdpState.issuer` back at `egret_public_url` (the three-way invariant test must fail); move `engine_jwks_uri` onto the new issuer (the engine-address test must fail); delete the fallback (the absent-key test must fail).

---

### Task 2: Set it at the edge, and prove nothing in the data plane broke

**Files:** `deploy/helm/egret/templates/configmap-env.yaml`

- [ ] **Step 1: Set `EGRET_IDP__ISSUER` to the external URL when `ingress.enabled`**

The neighbouring `EGRET_DATA_API__PUBLIC_URL` line added by #525 is the idiom; scheme+host only. Leave it unset otherwise so the fallback applies.

- [ ] **Step 2: Deploy and prove discovery now validates**

Preserve the release's values (`helm get values egret -n egret-system`) and verify the roll by **pod AGE, not the rollout message** — `:dev` tags mean helm reports success while old pods keep running.

Then fetch `http://egret.localtest.me/.well-known/openid-configuration` and confirm `issuer`, `token_endpoint` and `jwks_uri` all name `egret.localtest.me` — i.e. the document is now self-consistent with where it was fetched from.

- [ ] **Step 3: Prove the data plane still works — this is the real risk**

Trino does not check the issuer, so it *should* be unaffected. **Do not assert that from the table above; measure it.** There is a live Trino engine `demo-trino` in workspace `demo`. Run a query through the SQL Lab backend API — `POST /api/ws/demo/queries` with `{database_id, sql}` (snake_case; `databaseId` gives 422), poll `/queries/{id}`, then `/results`. `SELECT current_user` returning the caller's name proves the whole chain still holds: egret mints a token with the NEW `iss`, Trino validates it against a JWKS URL that is still in-cluster, and impersonation still works.

If it fails, stop and report rather than adjusting anything — that result would mean the table above is wrong, which is worth knowing precisely.

- [ ] **Step 4: Record the StarRocks migration hazard where an operator will see it**

`starrocks_jwt_integration_sql` runs **only in the delegation bootstrap on engine creation** (`engines.rs:3381`), and the statement is `CREATE SECURITY INTEGRATION`, not a replace. So a StarRocks engine provisioned before the issuer changes keeps the old `required_issuer` and will reject new tokens, with no automatic repair.

There are **zero StarRocks engines on this cluster**, so nothing breaks here — but this must not ship as a silent landmine. Put the warning in the chart value's comment and in the config doc comment. Making the bootstrap idempotent is the real fix and is a follow-on, not this plan.

---

## Self-Review

**Spec coverage:** the identity/address split is Task 1; the deployment and its verification are Task 2. **Placeholder scan:** Task 1 Step 3's assertions are described rather than written because the discovery test's location has not been read; the falsifiability requirement (different values, or the test cannot fail) is stated instead. **Type consistency:** `idp.issuer` / `EGRET_IDP__ISSUER` throughout; `egret_public_url` keeps its name and its engine-facing role.
