# Engine JWT Delegation Default-On — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provisioning a Trino/StarRocks engine with no `delegationMode` defaults to per-user JWT delegation (trust egret's machine-IdP), degrading to `impersonate` when JWT can't be guaranteed, with an explicit `"none"` opt-out.

**Architecture:** A pure resolver (`resolve_delegation`) maps `(explicit mode, has-JWKS, SR image tag) → {Jwt|Impersonate|Open}`. The two create paths call it and **persist the resolved concrete mode** into the stored spec, so the provision-req builder and update-preserve paths are unchanged. The frontend adds an "Auto" selection that sends an omitted mode.

**Tech Stack:** Rust (axum/sqlx control-plane in `crates/egret-engine-mgmt`), React/TS frontend, kind e2e (Playwright).

**Spec:** `docs/en/specs/2026-07-21-engine-jwt-delegation-default-design.md` (+ vi).

**Key files:**
- `crates/egret-engine-mgmt/src/delegation_default.rs` — NEW pure resolver + image-tag parse (Task 1).
- `crates/egret-engine-mgmt/src/lib.rs` — declare the new module (Task 1).
- `crates/egret-engine-mgmt/src/engines.rs` — StarRocks create `create_starrocks_engine` (~2714), `validate_sr_delegation_mode` (~2478), StarRocks-body `fe_image`/`delegation_mode` (~183/198); Trino create path in `create_engine` (~1902–2060), the Trino `delegationMode` create-side validation (grep — mirrored by the update-side note at ~3471) (Tasks 2–3).
- `frontend/src/routes/control-plane/useEngineConfigForm.ts` + the Security tab in `EngineDetail.tsx` — delegation selector (Task 4).
- `openapi.json` — regenerate (Task 4).
- `justfile` + `frontend/tests/e2e/delegation-default-kind.spec.ts` — live e2e (Task 5).

**Preflight for every task:** the reviewer/implementer reads the referenced spans in `engines.rs` before editing (line numbers drift). Backend gate before each commit: `cargo fmt -p egret-engine-mgmt` + `cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings` + the task's tests.

---

### Task 1: Pure delegation resolver + StarRocks image-tag parse

**Files:**
- Create: `crates/egret-engine-mgmt/src/delegation_default.rs`
- Modify: `crates/egret-engine-mgmt/src/lib.rs` (add `mod delegation_default;` + re-export)
- Test: inline `#[cfg(test)]` in `delegation_default.rs`

- [ ] **Step 1: Write the failing tests** — create `delegation_default.rs` with ONLY the tests first (types referenced won't exist yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_pre_3_5_parse() {
        assert!(starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:3.4")));
        assert!(starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:3.4.10")));
        assert!(starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:2.5")));
        // >= 3.5 and unparseable/absent → NOT clearly pre-3.5 (assume capable)
        assert!(!starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:3.5")));
        assert!(!starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:4.1")));
        assert!(!starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:latest")));
        assert!(!starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu")));
        assert!(!starrocks_image_is_pre_3_5(Some("")));
        assert!(!starrocks_image_is_pre_3_5(None));
    }

    #[test]
    fn explicit_modes_are_honored() {
        assert_eq!(resolve_delegation(Some("none"), true, None), Ok(ResolvedDelegation::Open));
        assert_eq!(resolve_delegation(Some("impersonate"), false, None), Ok(ResolvedDelegation::Impersonate));
        assert_eq!(resolve_delegation(Some("jwt"), true, None), Ok(ResolvedDelegation::Jwt));
    }

    #[test]
    fn explicit_jwt_without_jwks_is_rejected() {
        let err = resolve_delegation(Some("jwt"), false, None).unwrap_err();
        assert!(err.contains("public_url"), "message names the fix: {err}");
    }

    #[test]
    fn unknown_mode_is_rejected() {
        assert!(resolve_delegation(Some("bogus"), true, None).is_err());
    }

    #[test]
    fn default_prefers_jwt_then_degrades() {
        // omitted + JWKS + capable image → jwt
        assert_eq!(resolve_delegation(None, true, Some("starrocks/fe-ubuntu:4.1")), Ok(ResolvedDelegation::Jwt));
        assert_eq!(resolve_delegation(None, true, None), Ok(ResolvedDelegation::Jwt)); // Trino: no image
        // omitted + JWKS + pre-3.5 image → impersonate
        assert_eq!(resolve_delegation(None, true, Some("starrocks/fe-ubuntu:3.4")), Ok(ResolvedDelegation::Impersonate));
        // omitted + no JWKS → impersonate
        assert_eq!(resolve_delegation(None, false, None), Ok(ResolvedDelegation::Impersonate));
        // blank string behaves like omitted
        assert_eq!(resolve_delegation(Some(""), false, None), Ok(ResolvedDelegation::Impersonate));
    }

    #[test]
    fn has_platform_jwks_needs_both() {
        assert!(has_platform_jwks("https://e/jwks", "https://e"));
        assert!(!has_platform_jwks("", "https://e"));
        assert!(!has_platform_jwks("https://e/jwks", ""));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test -p egret-engine-mgmt --lib delegation_default`
Expected: FAIL — `cannot find function/type` (resolver not defined yet).

- [ ] **Step 3: Write the implementation** — prepend to `delegation_default.rs`:

```rust
//! Default-on delegation resolution: an omitted engine `delegationMode` resolves
//! to per-user JWT (trust egret's machine-IdP), degrading to `impersonate` when
//! JWT can't be guaranteed. See docs/en/specs/2026-07-21-engine-jwt-delegation-default-design.md.

/// The concrete delegation outcome after applying the default-resolution rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedDelegation {
    /// Per-user JWT delegation (engine trusts egret's machine-IdP).
    Jwt,
    /// Service-user impersonation (`EXECUTE AS` / `X-Trino-User`).
    Impersonate,
    /// No delegation — an "open" service-user engine (the explicit `"none"` opt-out).
    Open,
}

/// True iff the platform can mint/validate engine JWTs — egret's machine-IdP JWKS
/// URI and issuer are both set (⟺ `catalog.public_url` is configured).
pub fn has_platform_jwks(engine_jwks_uri: &str, engine_oidc_issuer: &str) -> bool {
    !engine_jwks_uri.is_empty() && !engine_oidc_issuer.is_empty()
}

/// Best-effort: does a StarRocks FE image tag CLEARLY predate `authentication_jwt`
/// (added in 3.5)? Parses the tag after the last `:` as `major.minor…`. Returns
/// `true` ONLY when it clearly parses to `< 3.5`; an absent/unparseable tag
/// (`latest`, a digest, empty, no tag) returns `false` — assume capable, the
/// default image is 4.1. This only steers the DEFAULT toward `impersonate`; it is
/// never a hard-fail.
pub fn starrocks_image_is_pre_3_5(image: Option<&str>) -> bool {
    let Some(image) = image else { return false };
    let tag = match image.rsplit_once(':') {
        Some((_, t)) => t,
        None => return false, // no tag at all
    };
    let mut parts = tag.trim_start_matches('v').split('.');
    let (Some(maj), Some(min)) = (parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(maj), Ok(min)) = (maj.parse::<u32>(), min.parse::<u32>()) else {
        return false;
    };
    maj < 3 || (maj == 3 && min < 5)
}

/// Resolve the effective delegation mode.
///
/// - `explicit`: the raw wire `delegationMode` (`"jwt"|"impersonate"|"none"`), or
///   `None`/`""` when omitted.
/// - `has_jwks`: [`has_platform_jwks`].
/// - `sr_image`: the StarRocks FE image tag; `None` for Trino (no version floor).
///
/// Returns `Err` (→ a 400 message) ONLY for an explicit `"jwt"` the platform can't
/// satisfy, or an unrecognized value. The DEFAULT path (omitted) never errors — it
/// degrades to `Impersonate`.
pub fn resolve_delegation(
    explicit: Option<&str>,
    has_jwks: bool,
    sr_image: Option<&str>,
) -> Result<ResolvedDelegation, String> {
    match explicit.map(str::trim).filter(|s| !s.is_empty()) {
        Some("none") => Ok(ResolvedDelegation::Open),
        Some("impersonate") => Ok(ResolvedDelegation::Impersonate),
        Some("jwt") if has_jwks => Ok(ResolvedDelegation::Jwt),
        Some("jwt") => Err(
            "JWT delegation was requested but this platform has no machine-IdP JWKS. \
             Set `catalog.public_url`, or pass delegationMode:\"none\"/\"impersonate\"."
                .to_string(),
        ),
        Some(other) => Err(format!(
            "invalid delegationMode: {other} (expected \"jwt\", \"impersonate\", or \"none\")"
        )),
        None => Ok(if has_jwks && !starrocks_image_is_pre_3_5(sr_image) {
            ResolvedDelegation::Jwt
        } else {
            ResolvedDelegation::Impersonate
        }),
    }
}
```

- [ ] **Step 4: Register the module** — in `crates/egret-engine-mgmt/src/lib.rs`, add near the other `mod` lines:

```rust
mod delegation_default;
pub use delegation_default::{
    has_platform_jwks, resolve_delegation, starrocks_image_is_pre_3_5, ResolvedDelegation,
};
```

(If `engines` is a private `mod`, keep the same visibility style; the re-export lets `engines.rs` call `crate::delegation_default::…` — either the `pub use` or a `use crate::delegation_default::*;` inside `engines.rs` works. Match the crate's existing intra-module `use` style.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p egret-engine-mgmt --lib delegation_default`
Expected: PASS (6 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-engine-mgmt/src/delegation_default.rs crates/egret-engine-mgmt/src/lib.rs
git commit -m "feat(engines): pure delegation-default resolver + SR image-tag parse"
```

---

### Task 2: StarRocks create applies the default (+ persist + auto-TLS + accept "none")

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — `validate_sr_delegation_mode` (~2478), `create_starrocks_engine` (~2714, the block around `validate_sr_delegation_mode(&sr_body)?` at ~2750 and `build_starrocks_spec(&sr_body, None)` at ~2783).
- Test: `#[cfg(test)]` in `engines.rs` (the module already has StarRocks create tests near ~5973; add there).

**Context to read first:** `create_starrocks_engine` (2714–2800), `validate_sr_delegation_mode` (2475–2487), the `StarRocksConfigBody` fields `fe_image`/`delegation_mode`/`tls_enabled` (183–203), and `build_starrocks_spec` (grep it — it reads `sr_body.delegation_mode` + defaults `tls_enabled`). `st.engine_jwks_uri` / `st.engine_oidc_issuer` are `CpState` fields (see `engines.rs` test builder ~5484).

- [ ] **Step 1: Write the failing tests** — add to the StarRocks create test module in `engines.rs`. Use the existing test harness (`CpState` builder used at ~5484/5973; reuse its helper to set `engine_jwks_uri`/`engine_oidc_issuer`). Assert on the PERSISTED spec's delegation block via the same `store::Engine` accessors the existing tests use (`delegation_mode()` / `supports_jwt_delegation()`), mirroring the Trino test at ~5134-5142.

```rust
// (names follow the existing SR create tests; reuse their `cp_state_with(...)`
// / `sr_create_body(...)` helpers — read the ~5973 test for the exact helpers.)

#[tokio::test]
async fn sr_create_defaults_to_jwt_with_tls_when_capable() {
    let st = cp_state_with_jwks("https://egret/.well-known/jwks.json", "https://egret");
    let body = sr_create_body(/* delegation_mode: */ None, /* fe_image: */ Some("starrocks/fe-ubuntu:4.1"));
    let (code, _dto) = create_starrocks_engine(st.clone(), body, wh()).await.unwrap();
    assert_eq!(code, StatusCode::CREATED);
    let eng = st.engine_mgmt.get_by_name("sr-x").await.unwrap();
    assert_eq!(eng.delegation_mode().as_deref(), Some("jwt"));
    assert!(eng.supports_jwt_delegation());
    // tls_enabled persisted true (read the stored delegation block’s tlsEnabled)
    assert_eq!(eng.spec["starRocksConfig"]["delegation"]["tlsEnabled"], serde_json::json!(true));
}

#[tokio::test]
async fn sr_create_omitted_degrades_to_impersonate_without_jwks() {
    let st = cp_state_with_jwks("", ""); // no public_url
    let body = sr_create_body(None, Some("starrocks/fe-ubuntu:4.1"));
    let (_c, _d) = create_starrocks_engine(st.clone(), body, wh()).await.unwrap();
    let eng = st.engine_mgmt.get_by_name("sr-x").await.unwrap();
    assert_eq!(eng.delegation_mode().as_deref(), Some("impersonate"));
}

#[tokio::test]
async fn sr_create_omitted_degrades_to_impersonate_on_pre_3_5_image() {
    let st = cp_state_with_jwks("https://egret/jwks", "https://egret");
    let body = sr_create_body(None, Some("starrocks/fe-ubuntu:3.4"));
    let (_c, _d) = create_starrocks_engine(st.clone(), body, wh()).await.unwrap();
    let eng = st.engine_mgmt.get_by_name("sr-x").await.unwrap();
    assert_eq!(eng.delegation_mode().as_deref(), Some("impersonate"));
}

#[tokio::test]
async fn sr_create_explicit_jwt_without_jwks_400s() {
    let st = cp_state_with_jwks("", "");
    let body = sr_create_body(Some("jwt"), Some("starrocks/fe-ubuntu:4.1"));
    let err = create_starrocks_engine(st, body, wh()).await.unwrap_err();
    assert!(matches!(err, CpError::BadRequest(m) if m.contains("public_url")));
}

#[tokio::test]
async fn sr_create_explicit_none_is_open() {
    let st = cp_state_with_jwks("https://egret/jwks", "https://egret");
    let body = sr_create_body(Some("none"), Some("starrocks/fe-ubuntu:4.1"));
    let (_c, _d) = create_starrocks_engine(st.clone(), body, wh()).await.unwrap();
    let eng = st.engine_mgmt.get_by_name("sr-x").await.unwrap();
    assert_eq!(eng.delegation_mode(), None); // no delegation block ⇒ open engine
}

#[tokio::test]
async fn sr_update_with_omitted_mode_preserves_stored() {
    // §6 regression: the default resolver is CREATE-only. Create an open ("none")
    // engine, then update an UNRELATED field with an omitted delegationMode — the
    // stored mode must stay open, NOT flip to jwt.
    let st = cp_state_with_jwks("https://egret/jwks", "https://egret");
    let created = sr_create_body(Some("none"), Some("starrocks/fe-ubuntu:4.1"));
    create_starrocks_engine(st.clone(), created, wh()).await.unwrap();
    let upd = sr_update_body_no_delegation(); // e.g. change feReplicas only
    update_starrocks_engine(st.clone(), "sr-x", upd).await.unwrap(); // use the real update fn name
    let eng = st.engine_mgmt.get_by_name("sr-x").await.unwrap();
    assert_eq!(eng.delegation_mode(), None); // still open — not resolved to jwt
}
```

(Use the actual StarRocks update handler name from `engines.rs` — grep `async fn update_starrocks` / the update dispatch near ~3442. If the existing update tests already cover "omitted preserves stored", extend one of those instead of adding a duplicate.)

If the existing SR create tests are `#[sqlx::test]` (need a DB) rather than pure, follow their exact attribute + fixtures; do NOT invent a lighter harness. If no in-module create test exists to copy, assert the resolver wiring at the `sr_body` level instead (call the small extraction from Step 3 directly) — but prefer the create-level test to match the existing ~5973 pattern.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p egret-engine-mgmt sr_create_ 2>&1 | tail`
Expected: FAIL — omitted currently yields `delegation_mode() == None` (open), not `"impersonate"`/`"jwt"`; `"none"` currently 400s in `validate_sr_delegation_mode`.

- [ ] **Step 3: Extend the validator to accept `"none"`** — in `validate_sr_delegation_mode` (~2480), change the allowed set:

```rust
        if m != "jwt" && m != "impersonate" && m != "none" {
            return Err(CpError::BadRequest(format!(
                "invalid starrocks delegationMode: {m} (expected \"jwt\", \"impersonate\", or \"none\")"
            )));
        }
```

- [ ] **Step 4: Apply the default in `create_starrocks_engine`** — make `sr_body` mutable and, right AFTER `validate_sr_delegation_mode(&sr_body)?` (~2750), resolve and rewrite the mode + force TLS for jwt:

```rust
    // Default-on delegation (spec 2026-07-21): an omitted mode resolves to jwt
    // (trust egret's machine-IdP), degrading to impersonate when JWT can't be
    // guaranteed. Persist the RESOLVED concrete mode so the provision-req builder
    // and update-preserve path see an explicit value. Only create resolves the
    // default; update preserves the stored mode.
    let resolved = crate::delegation_default::resolve_delegation(
        sr_body.delegation_mode.as_deref(),
        crate::delegation_default::has_platform_jwks(&st.engine_jwks_uri, &st.engine_oidc_issuer),
        sr_body.fe_image.as_deref(),
    )
    .map_err(CpError::BadRequest)?;
    match resolved {
        crate::delegation_default::ResolvedDelegation::Jwt => {
            sr_body.delegation_mode = Some("jwt".into());
            // JWT is sent as a cleartext MySQL password → TLS is mandatory (the
            // connector fail-closes without it). Force it on regardless of the body.
            sr_body.tls_enabled = Some(true);
        }
        crate::delegation_default::ResolvedDelegation::Impersonate => {
            sr_body.delegation_mode = Some("impersonate".into());
        }
        crate::delegation_default::ResolvedDelegation::Open => {
            sr_body.delegation_mode = None; // no delegation block ⇒ open engine
        }
    }
```

Change the `let sr_body = b.starrocks…` binding (~2740) to `let mut sr_body = …`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p egret-engine-mgmt sr_create_ 2>&1 | tail`
Expected: PASS (5 new tests). Then run the full crate delegation tests: `cargo test -p egret-engine-mgmt delegation` and the SR suite `cargo test -p egret-engine-mgmt starrocks` — Expected: existing tests still green (an existing test that provisioned an explicit `"jwt"`/`"impersonate"` SR engine is unaffected; a test that relied on "omitted SR ⇒ open" must be updated to send `delegationMode:"none"` — fix it in this step and note it in the commit).

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt -p egret-engine-mgmt && cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings
git add crates/egret-engine-mgmt/src/engines.rs
git commit -m "feat(engines): StarRocks create defaults to jwt delegation (degrade→impersonate, none opt-out)"
```

---

### Task 3: Trino create applies the default (synthesize jwt vs impersonate, accept "none")

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — the Trino branch of `create_engine` (~1902–2060, where `b.trino_config` is read and the spec is assembled/persisted BEFORE `build_provision_req` at ~2061), and the Trino create-side `delegationMode` validation (grep for the create-side counterpart of the update note at ~3471; extend it to accept `"none"`).
- Test: `#[cfg(test)]` in `engines.rs` near the Trino delegation tests (~5120–5150).

**Context to read first:** `create_engine` (1902–2077); how the Trino `trinoConfig` JSON is placed on the persisted spec; `resolve_auth` (1341–1421, already defaults an empty `jwksUri` to `st.engine_jwks_uri`, aud → `"engine"`, and 400s on jwt-without-JWKS — so the explicit-jwt hard-fail already surfaces at create via the inline `provision` at ~2077); `is_jwt_delegation()`/`is_impersonate_delegation()` (read `trinoConfig.delegationMode`); the Trino delegation test at 5120–5146.

**Design:** at create, resolve the default from `trinoConfig.delegationMode` and rewrite the persisted `trinoConfig` JSON:
- `Jwt` → set `trinoConfig.delegationMode = "jwt"` AND ensure `trinoConfig.auth.jwt` is an object (empty `{}` is enough — `resolve_auth` fills `jwksUri` from the platform default and aud `"engine"`).
- `Impersonate` → set `trinoConfig.delegationMode = "impersonate"` (provision's `inject_delegator_user` + delegator policies handle the rest).
- `Open` → leave `delegationMode` unset and no `auth.jwt` (open engine).
- `sr_image` arg is `None` (Trino has no version floor).

- [ ] **Step 1: Write the failing tests** — add near the Trino delegation tests (~5120). Mirror their style (build a spec JSON, run create or the create-time resolution, assert via `is_jwt_delegation()`/`is_impersonate_delegation()` + the presence of `trinoConfig.auth.jwt`).

```rust
#[tokio::test]
async fn trino_create_defaults_to_jwt_when_capable() {
    let st = cp_state_with_jwks("https://egret/.well-known/jwks.json", "https://egret");
    let body = trino_create_body(/* delegationMode: */ None); // omitted
    let (code, _dto) = create_engine(st.clone(), body).await.unwrap();
    assert_eq!(code, StatusCode::CREATED);
    let eng = st.engine_mgmt.get_by_name("trino-x").await.unwrap();
    assert!(eng.is_jwt_delegation());
    assert!(eng.spec["trinoConfig"]["auth"]["jwt"].is_object());
}

#[tokio::test]
async fn trino_create_omitted_degrades_to_impersonate_without_jwks() {
    let st = cp_state_with_jwks("", "");
    let body = trino_create_body(None);
    let (_c, _d) = create_engine(st.clone(), body).await.unwrap();
    let eng = st.engine_mgmt.get_by_name("trino-x").await.unwrap();
    assert!(eng.is_impersonate_delegation());
    assert!(!eng.is_jwt_delegation());
}

#[tokio::test]
async fn trino_create_explicit_none_is_open() {
    let st = cp_state_with_jwks("https://egret/jwks", "https://egret");
    let body = trino_create_body_mode("none");
    let (_c, _d) = create_engine(st.clone(), body).await.unwrap();
    let eng = st.engine_mgmt.get_by_name("trino-x").await.unwrap();
    assert!(!eng.is_jwt_delegation() && !eng.is_impersonate_delegation());
}
```

If `create_engine` provisions inline (it calls `prov.provision` at ~2077) and the test harness has no real orchestrator, use the SAME provisioning stub/fake the existing Trino create tests use (read ~5989/6229 for how create is exercised in tests). If the existing tests assert on the persisted spec WITHOUT provisioning, factor the create-time resolution into a tiny pure step (e.g. `apply_trino_delegation_default(&mut trino_config, resolved)`) and unit-test THAT directly, plus one integration test that omitted ⇒ jwt end-to-end.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p egret-engine-mgmt trino_create_ 2>&1 | tail`
Expected: FAIL — omitted currently yields no delegation (open), not jwt/impersonate; `"none"` currently rejected by the Trino delegationMode validator.

- [ ] **Step 3: Accept `"none"` in the Trino delegationMode validator** — find the create-side validator (the counterpart to the update-side note at ~3471, "Validate delegationMode exactly like create (jwt|impersonate|blank)") and add `"none"` to the accepted set, mirroring Task 2 Step 3.

- [ ] **Step 4: Apply the default in the Trino branch of `create_engine`** — after `b.trino_config` is read and BEFORE the spec is persisted / `build_provision_req` (~2061), resolve and rewrite the `trinoConfig` JSON:

```rust
    // Default-on delegation (spec 2026-07-21) — mirror the StarRocks path. Resolve
    // from trinoConfig.delegationMode; persist the resolved concrete mode so the
    // stored spec is explicit (build_provision_req/resolve_auth read it back).
    {
        let mode = trino_config_json
            .get("delegationMode")
            .and_then(|v| v.as_str());
        let resolved = crate::delegation_default::resolve_delegation(
            mode,
            crate::delegation_default::has_platform_jwks(&st.engine_jwks_uri, &st.engine_oidc_issuer),
            None, // Trino has no engine-version floor for JWT
        )
        .map_err(CpError::BadRequest)?;
        let obj = trino_config_json
            .as_object_mut()
            .expect("trinoConfig is a JSON object");
        match resolved {
            crate::delegation_default::ResolvedDelegation::Jwt => {
                obj.insert("delegationMode".into(), serde_json::json!("jwt"));
                // Ensure an auth.jwt object exists so resolve_auth synthesizes the
                // platform JWKS + aud "engine". Don't clobber an admin-provided block.
                let auth = obj.entry("auth").or_insert_with(|| serde_json::json!({}));
                if let Some(a) = auth.as_object_mut() {
                    a.entry("jwt").or_insert_with(|| serde_json::json!({}));
                }
            }
            crate::delegation_default::ResolvedDelegation::Impersonate => {
                obj.insert("delegationMode".into(), serde_json::json!("impersonate"));
            }
            crate::delegation_default::ResolvedDelegation::Open => {
                obj.remove("delegationMode");
            }
        }
    }
```

(`trino_config_json` = whatever local holds the mutable `trinoConfig` value that gets persisted onto the spec — read the create branch for its exact name; if it's currently read as an immutable `Option<&Value>`, clone it to an owned `serde_json::Value::Object` before this block and write the owned value onto the spec.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p egret-engine-mgmt trino_create_ 2>&1 | tail`
Expected: PASS (3 new). Then `cargo test -p egret-engine-mgmt` (whole crate) — Expected: green; update any existing test that assumed "omitted Trino ⇒ open" to send `delegationMode:"none"`, noting it in the commit.

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt -p egret-engine-mgmt && cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings
git add crates/egret-engine-mgmt/src/engines.rs
git commit -m "feat(engines): Trino create defaults to jwt delegation (degrade→impersonate, none opt-out)"
```

---

### Task 4: Frontend delegation selector (Auto default → omitted) + OpenAPI regen

**Files:**
- Modify: `frontend/src/routes/control-plane/useEngineConfigForm.ts` (the `DelegationMode` type + form default + serialize).
- Modify: the Security tab in `frontend/src/routes/control-plane/EngineDetail.tsx` (the delegation `<Select>` options).
- Regenerate: `openapi.json` (backend endpoints unchanged shape-wise but the docstrings/enum for `delegationMode` change — regen keeps the drift test green).
- Test: `frontend` typecheck + lint (no new unit test required; the wizard has no isolated test harness for this control — matching existing coverage).

**Context to read first:** `useEngineConfigForm.ts` `DelegationMode` (~362) + where the form seeds/serializes it; the Security tab `<Select>` in `EngineDetail.tsx` (~648).

- [ ] **Step 1: Add the "Auto" + "None" options and default to Auto** — in `useEngineConfigForm.ts`, widen the type and default:

```ts
// "" (Auto) is the DEFAULT: sends an OMITTED delegationMode so the backend
// resolves jwt→impersonate gracefully. "none" is the explicit open-engine opt-out.
export type DelegationMode = "" | "jwt" | "impersonate" | "none";
```

Set the form's initial `delegationMode` for a NEW engine to `""` (Auto). In the serializer that builds the create body, **omit** `delegationMode` when the value is `""` (send nothing), and send the literal string otherwise (`"jwt"`/`"impersonate"`/`"none"`).

- [ ] **Step 2: Update the Security-tab selector** — in `EngineDetail.tsx`, make the delegation `<Select>` options:

```tsx
<option value="">Auto — JWT, else impersonate (recommended)</option>
<option value="jwt">JWT (per-user)</option>
<option value="impersonate">Impersonate (service user)</option>
<option value="none">None (open engine)</option>
```

Ensure the create wizard's default selected value is `""` (Auto). On the EDIT screen of an existing engine, seed the select from the stored mode (an already-resolved `"jwt"`/`"impersonate"`, or `""`/absent → show Auto), so an unrelated edit re-submits the stored mode (backend update preserves it).

- [ ] **Step 3: Typecheck + lint**

Run: `cd frontend && npx tsc --noEmit && npm run lint`
Expected: clean.

- [ ] **Step 4: Regenerate OpenAPI**

Run: `cargo run -p egret-bin -- openapi > openapi.json`
Then verify the drift test: `cargo test -p egret-server committed_openapi_json_is_current`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/useEngineConfigForm.ts frontend/src/routes/control-plane/EngineDetail.tsx openapi.json
git commit -m "feat(fe): delegation selector Auto/JWT/Impersonate/None (Auto default → omitted)"
```

---

### Task 5: Live e2e — default provision comes up jwt-delegated

**Files:**
- Create: `frontend/tests/e2e/delegation-default-kind.spec.ts`
- Modify: `justfile` (add `e2e-kind-delegation-default`, mirror `e2e-kind-sts`).
- Test: opt-in (`EGRET_E2E_KIND=1`), not in `npm test`/CI.

**Context to read first:** `frontend/tests/e2e/sts-broker-kind.spec.ts` (provision + per-user query harness) and its helpers; the `e2e-kind-sts` recipe in `justfile`.

- [ ] **Step 1: Write the spec** — `EGRET_E2E_KIND`-gated. Provision a Trino AND a StarRocks engine via the backend create API sending **no** `delegationMode`, wait ready, then assert each came up delegated + a per-user query runs:

```ts
import { test, expect } from "@playwright/test";
// reuse the sts-broker helpers: login, provision-engine, wait-ready, run-query.
const ENABLED = process.env.EGRET_E2E_KIND === "1";

test.describe("default provision ⇒ jwt delegation", () => {
  test.skip(!ENABLED, "opt-in: EGRET_E2E_KIND=1 against a kind deploy with public_url");
  test.setTimeout(15 * 60_000);

  test("Trino + StarRocks default to jwt-delegated and serve a per-user query", async ({ request, page }) => {
    // 1. create both engines with NO delegationMode in the body.
    // 2. poll GET engine detail → spec.trinoConfig.delegationMode === "jwt"
    //    and spec.starRocksConfig.delegation.mode === "jwt".
    // 3. wait engines ready, then run a governed SELECT as an OIDC user through
    //    each (reuse sts-broker's query path); assert rows return (admin) and
    //    masking applies (member), proving the per-user JWT reached the engine.
  });
});
```

Fill the body concretely from `sts-broker-kind.spec.ts` (it already provisions engines + runs admin-full vs member-masked queries). Assert the delegation mode surfaced on the engine-detail DTO is `"jwt"` for BOTH on the `public_url` kind deploy.

- [ ] **Step 2: Add the recipe** — in `justfile`, after `e2e-kind-sts`:

```make
# Opt-in: default provision (no delegationMode) ⇒ both engines come up jwt-delegated.
e2e-kind-delegation-default: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test delegation-default-kind
```

- [ ] **Step 3: Discover + typecheck (no live run here — the coordinator runs it live)**

Run: `cd frontend && npx tsc --noEmit && npx playwright test delegation-default-kind --list`
Expected: typecheck clean; `--list` shows the test (skipped without `EGRET_E2E_KIND`).

- [ ] **Step 4: Commit**

```bash
git add frontend/tests/e2e/delegation-default-kind.spec.ts justfile
git commit -m "test(e2e): default provision ⇒ jwt delegation on kind"
```

---

## After all tasks

- Final holistic review of the branch (cross-cutting: resolver ↔ both create paths ↔ persisted-spec ↔ provision, no regression to explicit modes / update-preserve).
- Live run: rebuild `egret-backend:dev` → clear stale `:dev` on the node → `kind load` → `kubectl rollout restart deploy/egret -n egret-system` → `just e2e-kind-delegation-default` (green).
- Full local gate: `cargo fmt --all --check` + `cargo clippy --workspace -- -D warnings` + `cargo test -p egret-engine-mgmt` + FE `tsc`/lint + the openapi drift test.
- Finish the branch (`superpowers:finishing-a-development-branch`) → PR → squash-merge.
