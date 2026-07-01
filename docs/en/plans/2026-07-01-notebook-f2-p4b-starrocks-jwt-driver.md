# Notebook F2-P4b — StarRocks JWT via patched mysql_async Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make egret's Rust MySQL driver (`mysql_async`) present the user's OIDC engine JWT to StarRocks via StarRocks' `authentication_openid_connect_client` handshake, so StarRocks authenticates the notebook user per-JWT (the F2-P4 `enable_cleartext_plugin` approach doesn't work — StarRocks demands its own plugin, and mysql_async returns `None` for unknown plugins).

**Protocol (verified against StarRocks FE source + MySQL 9.6 docs):** When StarRocks requests the `authentication_openid_connect_client` plugin (an AuthSwitchRequest), the client must reply with **`[1 arbitrary byte][length-encoded string = the JWT]`**, single-round, over a secure connection. StarRocks `JWTAuthenticationProvider.authenticate` reads `readInt1()` (ignored) + `readLenEncodedString()` = the token.

**Approach:** Vendor `mysql_async` 0.34.2 into `third_party/` and patch its auth-switch to handle that plugin (mysql_common needs NO change — the patched mysql_async constructs the framed bytes itself). Wire it via `[patch.crates-io]`. `.pass(jwt)` (already set by the F2-P4 connector) supplies the token.

**Tech Stack:** Rust, `mysql_async` 0.34.2 (vendored+patched), StarRocks 4.1, TLS.

**Scope:** Client-side driver + connector. Live StarRocks enforcement is validated at the end (needs the dev stack + a security integration, both already set up in the P5 session). Keep the change additive — native/cleartext/caching-sha2 auth paths unchanged (zero regression to existing mysql_async usage: egret-bi charts, other engine queries).

---

## Task 1: Vendor mysql_async + wire `[patch.crates-io]` (mechanism validation)

**Files:** `third_party/mysql_async/**` (new, copied), root `Cargo.toml`.

- [ ] **Step 1: Copy the crate** from the cargo cache into the repo:
```bash
SRC=$(ls -d ~/.cargo/registry/src/*/mysql_async-0.34.2 | head -1)
mkdir -p third_party
cp -R "$SRC" third_party/mysql_async
# strip the cache checksum file that breaks a path dep
rm -f third_party/mysql_async/.cargo-ok third_party/mysql_async/.cargo_vcs_info.json
```
- [ ] **Step 2: Add `[patch.crates-io]`** to the ROOT `Cargo.toml` (workspace root):
```toml
[patch.crates-io]
mysql_async = { path = "third_party/mysql_async" }
```
(If a `[patch.crates-io]` section already exists, add the line to it.)
- [ ] **Step 3: Build the workspace UNCHANGED** — this validates the vendoring/patch mechanism before any code change:
```
cargo build --workspace 2>&1 | tail -5
```
Expected: builds successfully, and `cargo tree -p mysql_async` (or the build output) shows mysql_async resolved to the local path. If the vendored crate fails to build (missing `Cargo.toml` fields, a `[lib]`/publish quirk, or a checksum issue), fix minimally (e.g. remove `[badges]`/`publish` metadata) and note it. Do NOT proceed to Task 2 until the workspace builds with the vendored-but-unmodified crate.
- [ ] **Step 4: Commit**
```bash
git add third_party/mysql_async Cargo.toml Cargo.lock
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "chore(deps): vendor mysql_async 0.34.2 for a StarRocks OIDC-auth patch"
```

---

## Task 2: Patch mysql_async auth-switch for the OpenID plugin (TDD)

**Files:** `third_party/mysql_async/src/conn/mod.rs`.

Two hardcoded match arms currently mishandle unknown plugins:
- `perform_auth_switch` (~line 594-642): `AuthPlugin::Other(_) => x.gen_data(pass, nonce)` → mysql_common returns `None` (no token sent).
- `continue_auth` (~line 650-676): `AuthPlugin::Other(ref name) => Err(UnknownAuthPlugin{..})`.

Patch BOTH to special-case the OpenID plugin name.

- [ ] **Step 1: Add a constant + a framing helper** near the top of the auth impl (or a small `fn`):
```rust
// StarRocks / MySQL 9.2+ OpenID Connect client plugin. The server (StarRocks FE
// JWTAuthenticationProvider) reads: readInt1() (ignored) + readLenEncodedString() = JWT.
const OPENID_CONNECT_CLIENT_PLUGIN: &[u8] = b"authentication_openid_connect_client";

/// Frame a JWT as the OpenID client auth response: `[0x01][lenenc(token)]`.
fn openid_auth_data(token: &str) -> Vec<u8> {
    let t = token.as_bytes();
    let mut out = vec![0x01u8];
    // length-encoded integer (MySQL protocol)
    let n = t.len() as u64;
    if n < 0xfb {
        out.push(n as u8);
    } else if n < 0x1_0000 {
        out.push(0xfc);
        out.extend_from_slice(&(n as u16).to_le_bytes());
    } else {
        out.push(0xfd);
        out.extend_from_slice(&(n as u32).to_le_bytes()[..3]);
    }
    out.extend_from_slice(t);
    out
}
```

- [ ] **Step 2: Failing unit test** (in `third_party/mysql_async/src/conn/mod.rs` under a `#[cfg(test)] mod` — or wherever the crate keeps unit tests; if the crate has no unit-test module here, add a minimal one):
```rust
#[test]
fn openid_auth_data_frames_lenenc() {
    // short token: [0x01][len as 1 byte][bytes]
    assert_eq!(openid_auth_data("ab"), vec![0x01, 0x02, b'a', b'b']);
    // 300-byte token uses 0xfc + u16 LE
    let big = "x".repeat(300);
    let d = openid_auth_data(&big);
    assert_eq!(&d[..4], &[0x01, 0xfc, 44, 1]); // 300 = 0x012c → LE 2c 01
    assert_eq!(d.len(), 1 + 3 + 300);
}
```
Run: `cargo test -p mysql_async openid_auth_data` → FAIL (fn missing).

- [ ] **Step 3: Implement the two arms.** In `perform_auth_switch`, replace `AuthPlugin::Other(_) => x.gen_data(...)` with:
```rust
                AuthPlugin::Other(ref name) if name.as_ref() == OPENID_CONNECT_CLIENT_PLUGIN => {
                    self.inner.opts.pass().map(|p| {
                        mysql_common::packets::AuthPluginData::Clear(
                            std::borrow::Cow::Owned(openid_auth_data(p))
                        )
                    })
                }
                x @ AuthPlugin::Other(_) => x.gen_data(self.inner.opts.pass(), &self.inner.nonce),
```
(VERIFY the exact `AuthPluginData` variant/API in the vendored `mysql_common` — the earlier read showed `AuthPluginData::Clear(Cow<[u8]>)` is what cleartext uses and is written verbatim on the wire. Use whatever variant writes the bytes unframed; `openid_auth_data` already includes the full `[int1][lenenc]` framing, so it must be sent as-is with NO extra length prefix. If `Clear` adds its own framing, use the raw variant instead — inspect how each `AuthPluginData` variant is serialized in `write_auth_response`/the packet writer.)
In `continue_auth`, replace the `AuthPlugin::Other(ref name) => Err(UnknownAuthPlugin..)` arm with: for `name == OPENID_CONNECT_CLIENT_PLUGIN`, do what the `MysqlClearPassword` arm does (read the server's response packet / expect OK); for any OTHER name, keep the `UnknownAuthPlugin` error.

- [ ] **Step 4: Run** `cargo test -p mysql_async openid_auth_data` → PASS; `cargo build --workspace` → clean; `cargo clippy -p mysql_async -- -D warnings` (the vendored crate may have pre-existing clippy lint config — only ensure YOUR additions are clean).

- [ ] **Step 5: Commit**
```bash
git add third_party/mysql_async/src/conn/mod.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(mysql_async): send JWT for the StarRocks authentication_openid_connect_client plugin"
```

---

## Task 3: StarRocks TLS + connector confirmation

**Files:** `deploy/docker-compose.dev.yml`, `deploy/starrocks/**`, possibly `crates/egret-engine/src/connector/starrocks.rs`.

The OpenID client protocol requires a secure connection. StarRocks may reject the handshake over plain TCP.

- [ ] **Step 1: Try over plain TCP first** — the connector already sets `.pass(jwt)` (F2-P4). With the Task-2 patch, `mysql_async` will now answer the openid plugin. Test live (StarRocks + the `egret_oidc` security integration from the P5 session are already up):
```
# mint a JWT (host):
TOK=$(curl -s -d 'grant_type=password&client_id=egret&client_secret=egret-secret&username=tester&password=pw&scope=openid offline_access' http://localhost:8081/realms/egret/protocol/openid-connect/token | python3 -c 'import sys,json;print(json.load(sys.stdin)["access_token"])')
```
Write a tiny throwaway Rust example OR extend `crates/egret-engine/tests/connector_starrocks.rs` (env-gated) to build a `ConnSpec{ engine: StarRocks, host, port:9030, bearer_token: Some(TOK), .. }` and run `SELECT current_user()`. Expected: authenticates as `tester`. If StarRocks rejects with an "insecure connection" / SSL-required error → Step 2.
- [ ] **Step 2 (if needed): enable StarRocks TLS.** StarRocks FE supports SSL on the MySQL port via `ssl_keystore`/`enable_ssl` FE config or a keystore. Add a keystore (reuse a self-signed PKCS12/JKS via a `deploy/starrocks/gen-certs.sh`, gitignored) + the FE config, mount it, restart. The connector's `mysql_async` connection then needs `ssl_opts` — add an `ssl` flag to `ConnOptions` and set `OptsBuilder::ssl_opts` in `StarRocksConnector::new` when TLS is on (accept the self-signed cert in dev via `SslOpts::with_danger_accept_invalid_certs(true)`; real cert in prod). Document as dev-only.
- [ ] **Step 3: Commit** any deploy/connector changes with a clear message; if plain-TCP worked, note that TLS is a prod hardening item in `deploy/starrocks/README.md`.

---

## Task 4: Live e2e + full CI

- [ ] **Step 1: Live proof** — with the patched driver, `egret_oidc` integration, and a fresh JWT: the env-gated `connector_starrocks` JWT test (or the throwaway example) runs `SELECT current_user()` and returns `tester`. Capture the output. Also a negative: a bad/empty token → auth error (not a silent pass).
- [ ] **Step 2: Full CI** — postgres+minio up, `DATABASE_URL` set: `cargo fmt --all --check` (note: the vendored crate is third-party — exclude it from egret's fmt scope if the repo's fmt config would reformat it; only egret crates must be fmt-clean), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Classify failures (known infra/flaky vs regression). The vendored mysql_async patch must not break any existing mysql_async-using test (egret-bi, other engine tests).
- [ ] **Step 3: Document** in the PR + `deploy/starrocks/README.md`: StarRocks JWT now works via the patched vendored `mysql_async` (`third_party/`), the exact protocol, and the maintenance note (re-apply the patch when bumping mysql_async).

---

## Self-review
- **Spec coverage:** StarRocks per-user JWT via the real `authentication_openid_connect_client` handshake (T2), wired end-to-end (T1 vendoring, T3 TLS, T4 live). The F2-P4 connector `.pass(jwt)` supplies the token — no connector-auth change needed for the openid path.
- **Placeholder scan:** the `AuthPluginData` variant + `continue_auth` read-arm are marked "verify against the vendored mysql_common serialization" because they're external-crate internals — the framing (`[int1][lenenc]`) and the two patch locations are exact. T3 branches on empirical StarRocks behavior (plain-TCP vs TLS-required).
- **Type/name consistency:** `OPENID_CONNECT_CLIENT_PLUGIN` constant, `openid_auth_data(token)->Vec<u8>` framing helper, used in both `perform_auth_switch` and the test.
- **Regression safety:** the patch only adds an arm for the exact openid plugin name; all existing auth plugins (native/cleartext/caching-sha2/old) are untouched → zero impact on other mysql_async usage.
- **Maintenance:** vendored `third_party/mysql_async` is pinned to 0.34.2; bumping requires re-applying the ~20-line patch (documented).
