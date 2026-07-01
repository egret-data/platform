# REST Catalog Redesign — Plan 2: vended STS credentials + remove signer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Spec: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (Workstream **D**). Plan 2 of 6 (A → B → **D** → E → C → F). Depends on nothing in Plan 1 to compile, but is sequenced after B. Self-contained and testable.
> VI reference: `docs/vi/plans/2026-07-02-rest-catalog-redesign-p2-vended-sts.md`.

**Goal:** Have the catalog vend short-lived, table-scoped STS credentials for the notebook warehouse (so the kernel does direct S3 IO without ever holding root credentials) and remove the remote-signing path egret currently proxies.

**Architecture:** Enable STS credential vending on the lakekeeper S3 storage profile (`sts_enabled=true` + `sts_role_arn` + `sts_endpoint`, `remote_signing_enabled=false`) when a storage config carries an STS role; delete the `/signer` handling in `front_auth`; stop leaking root `AWS_*` credentials into the kernel subprocess so it relies on vended creds.

**Tech Stack:** Rust (`egret-catalog-engine`, `egret-config`, `egret-kernel`), the pinned `lakekeeper` `S3Profile`, MinIO STS (dev), pyiceberg/Daft (kernel helpers).

---

## Background the implementer needs

- Warehouse creation: `crates/egret-catalog-engine/src/provision_ws.rs`. `create_warehouse_on(...)` builds a `CreateWarehouseRequest` whose storage profile comes from `s3_profile_at(cfg, key_prefix)` (~line 204). Today both arms end with `.flavor(S3Flavor::S3Compat|Aws).sts_enabled(false)`. `cfg: &StorageConfig` carries `endpoint/region/bucket/access_key/secret_key/path_style` (resolved from the workspace storage connection in `browse.rs::create_warehouse`).
- lakekeeper `S3Profile` (pinned crate `crates/lakekeeper/src/service/storage/s3.rs`) fields: `sts_enabled: bool`, `sts_role_arn: Option<String>` (validation: if `sts_enabled` then `sts_role_arn` **or** `assume_role_arn` must be set), `sts_endpoint: Option<url::Url>`, `sts_token_validity_seconds: u64`, `remote_signing_enabled: bool`. When a client requests `vended_credentials` and STS is enabled with a role, lakekeeper returns temporary S3 creds in the loadTable/config `config`.
- `front_auth.rs` (after Plan 1) has `warehouse_prefix(path)` with a `"signer" => segs.next()` arm and `required_permission()` with a `trimmed.ends_with("/aws/s3/sign")` read-only case, plus tests `warehouse_prefix_extracts_the_warehouse_uuid` (has a `/signer/...` assertion) and the sign classification in `required_permission_classifier`.
- Kernel subprocess env: `egret-kernel` builds the kernel launch environment (SubprocessLauncher / kernel manager). Today serve's `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_ENDPOINT_URL` are inherited by the kernel, and pyiceberg used them for direct S3 IO. With vended STS, pyiceberg reads S3 settings from the table `config`; the kernel must NOT inherit root `AWS_*`.

---

## Task 1: Enable STS credential vending on the S3 storage profile

**Files:**
- Modify: `crates/egret-catalog-engine/src/provision_ws.rs`
- Modify: the `StorageConfig` struct (find its definition — `egret-storage` or `egret-config`; `rg "struct StorageConfig" crates`) to add STS fields
- Test: `crates/egret-catalog-engine/src/provision_ws.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Add STS fields to `StorageConfig`**

Add two optional fields (STS is opt-in; when `sts_role_arn` is absent, behavior is unchanged):

```rust
/// STS AssumeRole ARN for credential vending. When set, warehouses on this
/// storage vend short-lived table-scoped creds instead of remote-signing.
pub sts_role_arn: Option<String>,
/// STS endpoint (MinIO: usually the same as the S3 endpoint). Falls back to
/// the S3 `endpoint` when unset.
pub sts_endpoint: Option<String>,
```
Populate them where `StorageConfig` is built from a storage connection (`browse.rs::create_warehouse` resolver) — from the connection row, or a dev default from egret config `EGRET_STORAGE__STS_ROLE_ARN`. Existing construction sites default both to `None`.

- [ ] **Step 2: Write the failing test for STS-enabled profile**

In `provision_ws.rs` tests:

```rust
#[test]
fn s3_profile_enables_sts_when_role_present() {
    let cfg = StorageConfig {
        endpoint: Some("http://localhost:9000".into()),
        region: "us-east-1".into(),
        bucket: "warehouse".into(),
        access_key: "egret".into(),
        secret_key: "egretsecret".into(),
        path_style: true,
        sts_role_arn: Some("arn:aws:iam::000000000000:role/egret-notebook".into()),
        sts_endpoint: None,
    };
    let StorageProfile::S3(p) = s3_profile_at(&cfg, "warehouses/notebook").unwrap() else {
        panic!("expected S3 profile");
    };
    assert!(p.sts_enabled);
    assert_eq!(p.sts_role_arn.as_deref(), Some("arn:aws:iam::000000000000:role/egret-notebook"));
    assert!(!p.remote_signing_enabled);
    // STS endpoint falls back to the S3 endpoint for MinIO.
    assert_eq!(p.sts_endpoint.as_ref().map(|u| u.as_str()), Some("http://localhost:9000/"));
}

#[test]
fn s3_profile_keeps_sts_off_without_role() {
    let cfg = StorageConfig { sts_role_arn: None, ..sample_cfg() };
    let StorageProfile::S3(p) = s3_profile_at(&cfg, "warehouses/x").unwrap() else { panic!() };
    assert!(!p.sts_enabled);
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p egret-catalog-engine --lib s3_profile_enables_sts_when_role_present`
Expected: FAIL (fields/behavior missing).

- [ ] **Step 4: Set the STS builder flags in `s3_profile_at`**

For each arm of the `match cfg.endpoint` in `s3_profile_at`, replace the `.sts_enabled(false)` tail with a conditional:

```rust
// after the shared builder produces `builder` with bucket/region/flavor/prefix:
let profile = if let Some(role) = cfg.sts_role_arn.as_deref() {
    let sts_endpoint = cfg
        .sts_endpoint
        .as_deref()
        .or(cfg.endpoint.as_deref())
        .filter(|e| !e.is_empty())
        .map(|e| e.parse::<url::Url>())
        .transpose()
        .context("parsing STS endpoint URL")?;
    builder
        .sts_enabled(true)
        .sts_role_arn(role.to_string())
        .sts_endpoint(sts_endpoint)     // Option<url::Url>; None for real AWS
        .remote_signing_enabled(false)
        .build()
} else {
    builder.sts_enabled(false).build()
};
```
(Adapt to the two typed-builder arms; the S3Compat arm sets `sts_endpoint`, the AWS arm may leave it `None`. Confirm exact builder method names against the pinned `S3Profile` builder.)

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p egret-catalog-engine --lib s3_profile`
Expected: PASS (both new tests).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-catalog-engine/src/provision_ws.rs  # + the StorageConfig struct file
git commit -m "feat(catalog): vend STS credentials on the storage profile when an STS role is set"
```

---

## Task 2: Remove the `/signer` handling from front_auth

**Files:**
- Modify: `crates/egret-catalog-engine/src/front_auth.rs`

- [ ] **Step 1: Update the `warehouse_prefix` test to drop the signer case and add a no-signer expectation**

In the `warehouse_prefix_extracts_the_warehouse_uuid` test, remove the two `/signer/...` assertions and assert signer is treated as a normal (non-special) first segment:

```rust
// signer path is no longer special — the first segment is taken verbatim
// (there is no signer request anymore; vended STS replaced it).
assert_eq!(warehouse_prefix("/signer/0190abcd/x"), Some("signer"));
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p egret-catalog-engine --lib warehouse_prefix`
Expected: FAIL (current code returns `Some("0190abcd")` for the signer path).

- [ ] **Step 3: Remove the signer arm in `warehouse_prefix`**

```rust
fn warehouse_prefix(path: &str) -> Option<&str> {
    let mut segs = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty());
    match segs.next()? {
        "config" => None,
        wh => Some(wh),
    }
}
```
Update the doc comment to drop the signer-path paragraph.

- [ ] **Step 4: Remove the `/aws/s3/sign` special-case in `required_permission`**

In `required_permission`, delete the `|| trimmed.ends_with("/aws/s3/sign")` clause so the read-only POST set is just `config` + `/metrics`. Update the `required_permission_classifier` test to drop the sign assertion.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p egret-catalog-engine --lib 'front_auth::tests'`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-catalog-engine/src/front_auth.rs
git commit -m "refactor(catalog): drop /signer handling — vended STS replaces remote signing"
```

> Note: if a deployment cannot use STS for a warehouse (store without AssumeRole), that warehouse would need remote-signing and thus the signer branch. This plan assumes STS vending for the notebook warehouse (the only Daft-accessed catalog); the fallback is documented in the spec risks, not implemented here.

---

## Task 3: Stop leaking root AWS creds into the kernel; rely on vended STS

**Files:**
- Modify: `crates/egret-kernel` kernel launch env construction (find it: `rg "AWS_ACCESS_KEY_ID|env(|envs(|Command::new" crates/egret-kernel/src`)
- Modify (verify only): `kernel-image/egret_kernel_helpers/catalog.py`

- [ ] **Step 1: Scrub `AWS_*` from the kernel subprocess environment**

Where the kernel subprocess env is assembled, remove/skip any `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_ENDPOINT_URL` inherited from serve, e.g. build the child env by filtering:

```rust
let env: Vec<(String, String)> = std::env::vars()
    .filter(|(k, _)| !k.starts_with("AWS_"))
    .chain(extra_env)   // the egret-specific vars the kernel needs
    .collect();
```
Rationale: the kernel must obtain S3 access only via the catalog's vended STS creds (in the table `config`), proving the credential isolation the redesign is for.

- [ ] **Step 2: Verify `catalog()` relies on the vended creds (no static AWS passthrough)**

Confirm `kernel-image/egret_kernel_helpers/catalog.py` builds `RestCatalog(name, uri=..., warehouse=..., token=...)` and does NOT inject `s3.access-key-id`/`s3.secret-access-key` from env. pyiceberg reads the vended `s3.*` settings from the loadTable/config `config` automatically. If any static S3 property is set from env, remove it.

- [ ] **Step 3: Commit**

```bash
git add crates/egret-kernel  # + kernel-image/egret_kernel_helpers/catalog.py if changed
git commit -m "feat(kernel): kernel uses vended STS creds only — scrub root AWS_* from its env"
```

---

## Task 4: MinIO STS dev setup + live verification (documented, run once)

**Files:**
- Modify: `frontend/tests/e2e/README.md` (or the dev-stack docs) — add the MinIO STS setup

- [ ] **Step 1: Configure MinIO AssumeRole for the storage access key**

MinIO exposes STS `AssumeRole` at the S3 endpoint. Attach a policy to the `egret` access key that allows the warehouse bucket/prefix, so `AssumeRole` returns scoped creds. Document the `mc` commands, e.g.:

```bash
mc admin policy create local egret-notebook /path/to/egret-notebook-policy.json
mc admin user info local egret   # ensure the key can AssumeRole
```
Set the dev STS role ARN egret passes (`EGRET_STORAGE__STS_ROLE_ARN`, e.g. `arn:aws:iam::000000000000:role/egret-notebook`) — MinIO accepts an arbitrary RoleArn for AssumeRole but requires it present.

- [ ] **Step 2: Live check — vended creds, no signer, no AWS_* in kernel**

Boot the stack, run a notebook `write_iceberg` + `iceberg` roundtrip (as in `notebook-dataframe.spec.ts`) and assert: the loadTable/config response contains vended `s3.session-token`; no request hits `/aws/s3/sign`; the kernel has no `AWS_*` env. Confirm the roundtrip succeeds.

```bash
# vended creds present in config?
curl -s -H "Authorization: Bearer $TOK" \
  "http://localhost:8080/catalog/v1/config?warehouse=notebook" | python3 -m json.tool | grep -i "session-token\|access-key" || echo "check loadTable config"
```

- [ ] **Step 3: Commit the docs**

```bash
git add frontend/tests/e2e/README.md
git commit -m "docs(catalog): dev MinIO STS setup for vended credentials"
```

---

## Task 5: Workspace verification

- [ ] **Step 1: fmt + clippy + unit + integration**

Run:
```bash
cargo fmt --all --check
cargo clippy -p egret-catalog-engine -p egret-kernel --all-targets -- -D warnings
cargo test -p egret-catalog-engine --lib
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-catalog-engine --test front_auth --test gc_e2e
```
Expected: all pass. `front_auth` integration still passes (signer path removal does not affect the non-signer flows it exercises).

---

## Self-review notes (spec coverage for Plan 2)

- Spec **D** (vended STS): Task 1 (STS on storage profile), Task 4 (MinIO STS + live verify), Task 3 (kernel relies on vended creds).
- Spec **D** (remove signer): Task 2.
- Known softness for the implementer to resolve by reading code: exact `StorageConfig` struct location + its construction sites (Task 1 Step 1), and the kernel launch-env assembly site (Task 3 Step 1). Both are `rg`-locatable and noted inline.
