# REST Catalog Redesign — Plan 1: base_url root-fix + config split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Spec: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (Workstreams **A** and **B**). This is the first of six sequenced plans (A → B → D → E → C → F); it is self-contained and testable on its own.
> VI reference: `docs/vi/plans/2026-07-01-rest-catalog-redesign-p1-base-url-config.md`.

**Goal:** Make the embedded lakekeeper advertise the correct public catalog URL at the source (removing the PR #114 response-rewrite band-aid), and split the overloaded `catalog.public_url` config so setting the catalog URL never breaks the kernel callback `api_url`.

**Architecture:** (A) Fork lakekeeper to add a public `RequestMetadata::with_base_url` setter, apply it via a Cargo `[patch]`, and have `front_auth` stamp the configured public catalog URL onto every injected `RequestMetadata`; delete the rewrite middleware. (B) Add `server.public_url` to config; derive the kernel `api_url` from it instead of from `catalog.public_url`.

**Tech Stack:** Rust (axum, sqlx), the git-pinned `lakekeeper` crate, `egret-config`, `egret-catalog-engine`, `egret-bin`.

---

## Background the implementer needs

- `crates/egret-catalog-engine/src/front_auth.rs` currently contains (added in PR #114) a `rewrite_lakekeeper_base(body, public)` function, a `LAKEKEEPER_INTERNAL_CATALOG_BASE` const, a response-rewrite block at the end of `front_auth()` that buffers JSON bodies and swaps `http://localhost:8181/catalog` → the public URL, and a `FrontAuthState.catalog_public_url: Option<String>` field set via `FrontAuthState::new(store, jwt, catalog_public_url)`. Unit tests `rewrites_config_overrides_uri`, `rewrites_s3_signer_uri_in_loadtable_response`, `rewrite_returns_none_when_no_internal_base_present` cover it.
- The three places egret builds a lakekeeper `RequestMetadata`: `token_request_metadata()` (opaque + JWT token path), the system-principal metadata (in `crate::bootstrap` / where `new_lakekeeper_internal` or the system actor is constructed), and the browse metadata (in `browse.rs`). Each uses a `RequestMetadata` whose `base_url` defaults to `http://localhost:8181`.
- lakekeeper is pinned in `crates/egret-catalog-engine/Cargo.toml` to `git = "https://github.com/lakekeeper/lakekeeper", rev = "791650571da769679a2037102a9737eb4fc84cfa"` (also `lakekeeper-storage-postgres`). `RequestMetadata` lives in that crate at `crates/lakekeeper/src/request_metadata.rs` (private `base_url: String` field ~line 97, getter `base_url()` ~line 501, no setter).
- `crates/egret-config/src/lib.rs`: `Config.server: ServerConfig { host: String, port: u16 }`, `Config.catalog: CatalogSettings` (has `public_url: Option<String>`).
- `crates/egret-bin/src/main.rs`: `catalog_base_url` is computed as `config.catalog.public_url.clone().unwrap_or_else(|| "http://localhost:8080".into())` (hoisted above the catalog-engine block by PR #114) and passed to `mount_catalog_engine(.., Some(catalog_base_url.clone()))`; `api_url` is computed (~line 749) as `config.catalog.public_url.clone().unwrap_or_else(|| format!("http://127.0.0.1:{}", config.server.port))`.

---

## Task 1: Fork lakekeeper and add a `with_base_url` setter

**Files:**
- Fork repo: `github.com/egretdb/lakekeeper` (new fork of `lakekeeper/lakekeeper` at the pinned rev)
- Modify (in the fork): `crates/lakekeeper/src/request_metadata.rs`
- Modify: `Cargo.toml` (workspace root) — add `[patch]` for the lakekeeper git source
- Test (in the fork): `crates/lakekeeper/src/request_metadata.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Create the fork and a branch off the pinned rev**

```bash
# One-time: fork upstream into the egret org, then branch at the exact pinned rev.
gh repo fork lakekeeper/lakekeeper --org egretdb --fork-name lakekeeper --clone
cd lakekeeper
git checkout -b egret/base-url-setter 791650571da769679a2037102a9737eb4fc84cfa
```

- [ ] **Step 2: Write the failing test for the setter**

In `crates/lakekeeper/src/request_metadata.rs`, inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn with_base_url_overrides_the_default() {
    let md = RequestMetadata::new_lakekeeper_internal(uuid::Uuid::now_v7())
        .with_base_url("http://localhost:8080/catalog".to_string());
    assert_eq!(md.base_url(), "http://localhost:8080/catalog");
    assert_eq!(md.base_uri_catalog(), "http://localhost:8080/catalog/catalog");
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p lakekeeper with_base_url_overrides_the_default`
Expected: FAIL — `no method named with_base_url`.

- [ ] **Step 4: Add the setter**

In `crates/lakekeeper/src/request_metadata.rs`, next to the `base_url()` getter (~line 501):

```rust
/// Override the base URL reported in self-referential responses (`/config`
/// `overrides.uri`, per-table `s3.signer.uri`). Embedders that mount this
/// catalog behind a reverse proxy set the externally-reachable URL here so
/// clients follow the proxy rather than the internal bind address.
#[must_use]
pub fn with_base_url(mut self, base_url: String) -> Self {
    self.base_url = base_url;
    self
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p lakekeeper with_base_url_overrides_the_default`
Expected: PASS.

- [ ] **Step 6: Commit and push the fork branch**

```bash
git add crates/lakekeeper/src/request_metadata.rs
git commit -m "feat: add RequestMetadata::with_base_url for reverse-proxy embedders"
git push -u origin egret/base-url-setter
# Note the resulting commit sha for the [patch] rev below:
git rev-parse HEAD
```

- [ ] **Step 7: Point egret at the fork via `[patch]`**

In the egret workspace-root `Cargo.toml`, add (using the sha from Step 6 as `<FORK_SHA>`):

```toml
[patch."https://github.com/lakekeeper/lakekeeper"]
lakekeeper = { git = "https://github.com/egretdb/lakekeeper", rev = "<FORK_SHA>" }
lakekeeper-storage-postgres = { git = "https://github.com/egretdb/lakekeeper", rev = "<FORK_SHA>" }
```

- [ ] **Step 8: Verify egret builds against the patched crate**

Run: `cargo build -p egret-catalog-engine`
Expected: builds; `cargo tree -p lakekeeper` shows the `egretdb/lakekeeper` source.

- [ ] **Step 9: Commit the patch wiring**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build(catalog): patch lakekeeper to egret fork with base_url setter"
```

---

## Task 2: Thread the public catalog URL onto every injected RequestMetadata

**Files:**
- Modify: `crates/egret-catalog-engine/src/front_auth.rs`
- Modify: `crates/egret-catalog-engine/src/browse.rs` (system/browse metadata construction — the second and third construction sites)
- Test: `crates/egret-catalog-engine/src/front_auth.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing test for base_url stamping**

In `front_auth.rs` tests, assert the token metadata carries the configured base_url. Add a small helper on `FrontAuthState` first is not needed — test `token_request_metadata` directly by making it take the base_url (see Step 3). Write:

```rust
#[test]
fn token_metadata_carries_configured_base_url() {
    let auth = TokenAuth::test_fixture(); // existing test helper; if absent, construct a minimal TokenAuth
    let md = token_request_metadata(&auth, "http://localhost:8080/catalog");
    assert_eq!(md.base_url(), "http://localhost:8080/catalog");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p egret-catalog-engine --lib token_metadata_carries_configured_base_url`
Expected: FAIL — `token_request_metadata` takes one argument.

- [ ] **Step 3: Give `token_request_metadata` the base_url and stamp it**

In `front_auth.rs`, change the signature and the construction:

```rust
fn token_request_metadata(auth: &TokenAuth, base_url: &str) -> RequestMetadata {
    // ... unchanged: build `uid`, `grants`, `authentication` ...
    let mut md = RequestMetadata::new_lakekeeper_internal(Uuid::now_v7())
        .with_base_url(base_url.to_string());
    md.set_authentication(Actor::Principal(uid), authentication);
    md
}
```

Update its single caller in `front_auth()`:

```rust
let base_url = state
    .catalog_public_url
    .as_deref()
    .unwrap_or("http://localhost:8080/catalog");
req.extensions_mut()
    .insert(token_request_metadata(&auth, base_url));
```

- [ ] **Step 4: Stamp base_url on the system + browse metadata sites**

In `browse.rs` (and any `bootstrap.rs` site) wherever a `RequestMetadata` is built for internal/system or browse calls, chain `.with_base_url(base_url)` from the same configured value. The browse router already carries `BrowseState`; add a `catalog_public_url: String` to it (set in `egret-bin` from `catalog_base_url`) and use it. Where a site has no state handle, thread the value through the same constructor argument you added.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p egret-catalog-engine --lib token_metadata_carries_configured_base_url`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-catalog-engine/src/front_auth.rs crates/egret-catalog-engine/src/browse.rs
git commit -m "feat(catalog): stamp public catalog URL onto injected RequestMetadata"
```

---

## Task 3: Remove the PR #114 response-rewrite band-aid

**Files:**
- Modify: `crates/egret-catalog-engine/src/front_auth.rs`

- [ ] **Step 1: Delete the rewrite function, const, and tests**

Remove from `front_auth.rs`: the `LAKEKEEPER_INTERNAL_CATALOG_BASE` const, the `rewrite_lakekeeper_base` function, and the three tests (`rewrites_config_overrides_uri`, `rewrites_s3_signer_uri_in_loadtable_response`, `rewrite_returns_none_when_no_internal_base_present`).

- [ ] **Step 2: Delete the response-rewrite block in `front_auth()`**

Replace the tail of `front_auth()` (the `let resp = next.run(req).await;` … JSON-buffering + `rewrite_lakekeeper_base` block) with the original passthrough:

```rust
    req.extensions_mut()
        .insert(token_request_metadata(&auth, base_url));

    Ok(next.run(req).await)
}
```

Keep `FrontAuthState.catalog_public_url` — it is now consumed by Task 2 (metadata base_url), not for rewriting.

- [ ] **Step 3: Verify it compiles and the rewrite is gone**

Run: `cargo build -p egret-catalog-engine && ! rg "rewrite_lakekeeper_base|LAKEKEEPER_INTERNAL_CATALOG_BASE" crates/egret-catalog-engine/src`
Expected: builds; `rg` finds nothing (exit non-zero → the `!` makes the line succeed).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-catalog-engine/src/front_auth.rs
git commit -m "refactor(catalog): remove #114 response-rewrite; base_url is correct at source"
```

---

## Task 4: Set `LAKEKEEPER__BASE_URI` at boot and verify the source is correct

**Files:**
- Modify: `crates/egret-bin/src/main.rs`

- [ ] **Step 1: Export `LAKEKEEPER__BASE_URI` from the catalog public URL before the engine boots**

In `main.rs`, before `egret_catalog_engine::build_state(...)` runs, set lakekeeper's own config env from the single source of truth (belt-and-braces with Task 2, and correct for any lakekeeper code path that reads `CONFIG.base_uri`):

```rust
// SAFETY: set before the lakekeeper engine (and its lazy CONFIG) is built,
// during single-threaded startup.
std::env::set_var("LAKEKEEPER__BASE_URI", &catalog_base_url);
```

- [ ] **Step 2: Manual integration check (documented, run once)**

Boot the dev stack with `EGRET_CATALOG__ENABLED=true EGRET_CATALOG__AUTH_TOKEN=dev EGRET_CATALOG__PUBLIC_URL=http://localhost:8080/catalog`, mint a catalog token, and assert the config no longer leaks `:8181`:

```bash
curl -s -H "Authorization: Bearer $TOK" \
  "http://localhost:8080/catalog/v1/config?warehouse=notebook" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['overrides']['uri'])"
# Expected: http://localhost:8080/catalog   (NOT http://localhost:8181/catalog)
```

- [ ] **Step 3: Commit**

```bash
git add crates/egret-bin/src/main.rs
git commit -m "feat(catalog): set LAKEKEEPER__BASE_URI from the configured public URL"
```

---

## Task 5: Split `catalog.public_url` from the kernel `api_url` (Workstream B)

**Files:**
- Modify: `crates/egret-config/src/lib.rs`
- Modify: `crates/egret-bin/src/main.rs`
- Test: `crates/egret-config/src/lib.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing test for the new field's default**

In `egret-config` tests:

```rust
#[test]
fn server_public_url_defaults_to_none() {
    let c = Config::default();
    assert!(c.server.public_url.is_none());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p egret-config server_public_url_defaults_to_none`
Expected: FAIL — no field `public_url` on `ServerConfig`.

- [ ] **Step 3: Add `public_url` to `ServerConfig`**

In `lib.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// Externally-reachable base URL the kernel uses to call egret back
    /// (engine-token / notebook-runtime). Defaults to `http://127.0.0.1:{port}`
    /// when unset. Distinct from `catalog.public_url` (the catalog URL).
    #[serde(default)]
    pub public_url: Option<String>,
}
```

And in `Config::default()` set `public_url: None` on the `server: ServerConfig { .. }` literal.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p egret-config server_public_url_defaults_to_none`
Expected: PASS.

- [ ] **Step 5: Derive `api_url` from `server.public_url` (not `catalog.public_url`)**

In `main.rs`, change the `api_url` computation:

```rust
let api_url = config
    .server
    .public_url
    .clone()
    .unwrap_or_else(|| format!("http://127.0.0.1:{}", config.server.port));
```

`catalog_base_url` keeps deriving from `config.catalog.public_url`. The two fields are now independent.

- [ ] **Step 6: Verify the split — build + a boot smoke check**

Run: `cargo build -p egret-bin`
Then boot with `EGRET_CATALOG__PUBLIC_URL=http://localhost:8080/catalog` (no `server.public_url` set) and confirm the kernel engine-token callback base is `http://127.0.0.1:8080` (not `.../catalog`) — check the notebook-runtime manifest or the serve log.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-config/src/lib.rs crates/egret-bin/src/main.rs
git commit -m "feat(config): split server.public_url (kernel api_url) from catalog.public_url"
```

---

## Task 6: Workspace verification

**Files:** none (verification only)

- [ ] **Step 1: fmt + clippy + unit tests on the touched crates**

Run:
```bash
cargo fmt --all --check
cargo clippy -p egret-catalog-engine -p egret-config -p egret-bin --all-targets -- -D warnings
cargo test -p egret-catalog-engine --lib && cargo test -p egret-config
```
Expected: all clean/pass. (The removed rewrite tests are gone; the two new unit tests pass.)

- [ ] **Step 2: DB-gated integration tests for the catalog engine**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-catalog-engine --test front_auth --test gc_e2e`
Expected: pass (the `mount_catalog_engine` signature is unchanged in this plan).

---

## Self-review notes (spec coverage for Plan 1)

- Spec **A** (base_url at source): Tasks 1–4 (fork setter, stamp metadata, remove rewrite, set `LAKEKEEPER__BASE_URI`, verify no `:8181`). Fallback (config-driven rewrite) is intentionally NOT taken — the fork is the chosen root fix.
- Spec **B** (config split): Task 5.
- Out of this plan (later): C (notebook→JWT), D (vended STS + signer removal), E (warehouse purpose), F (legacy cleanup).
