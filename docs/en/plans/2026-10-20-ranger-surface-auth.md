# Ranger surface authentication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Serve the Ranger plugin surface only under `/ranger`, require per-engine HTTP Basic credentials by default, restrict each credential to its own engine's profile, and drop plugin 2.5.0 / Trino 470 from support.

**Architecture:** One axum middleware authenticates `Bearer` (operator) or `Basic engine-<uuid>:<secret>` (engine) and stores a `Caller` in request extensions; the download handler authorizes the caller against the requested profile. Credentials are random secrets in the existing secret store, referenced by two new `engine_mgmt.engines` columns (current + previous, so a rotation never locks a running pod out). The plugin config that carries the password moves from a ConfigMap to a Kubernetes Secret.

**Tech Stack:** Rust (axum 0.8, sqlx, tower), Postgres migrations, Java 11+/Maven (`tests/ranger`), Kubernetes manifests rendered in `egret-provisioner`.

Spec: `docs/en/specs/2026-10-20-ranger-surface-auth-design.md` (commit `f4ef32e4`). Read §2 (measurements) first.

## Global Constraints

- Supported plugin floor is **2.8.0**; supported (type, plugin) pairs are **Trino 2.8.0, StarRocks 2.8.0, StarRocks 2.9.0**. 2.5.0 / Trino 470 is refused, not migrated.
- The only plugin URL is `<egret>/ranger`; the plugin appends `/service/...`. `/service` at the root is **removed** (404). No alias, no deprecation window.
- `ranger.auth_mode` is `required` (default) or `anonymous`. `anonymous` is an explicit opt-out for local stacks and logs a `warn!`.
- Engine credential: username `engine-<engine_id>`, password a random secret (32 random bytes, URL-safe base64 no padding — `random_internal_secret()` in `crates/egret-engine-mgmt/src/engines.rs`). Compared in constant time.
- An engine credential may download **only** its engine's profile (`engine_mgmt.engines.authz_service_id`) and the tag service that profile names; any other service ⇒ **403**. Engine credentials may not call any other `/ranger/service` route.
- Standing rule (user, 2026-09-16): **never delete a secret value the cluster may still hold.** Commit a minted secret even when the apply fails; delete the superseded one only after a successful apply; never prefix-sweep except at engine delete.
- The plugin-config XML must contain **no XML comment** (StarRocks FE refuses `--`). Passwords never appear in logs or `Debug` output.
- Every migration number is taken at the moment the file is created: run `git ls-tree origin/main migrations/ | tail -3` first (main tops out at 0199 on 2026-10-20).
- Conventional Commits; no `Closes #N` in commits. Never commit a secret. Scratch files go to the scratchpad.
- Repo docs rule (`CLAUDE.md`): user-visible/API/config/permission changes need docs in `egret-data/docs`; this plan covers `docs/en` and flags the rest for the user (Task 9).

## PR layout

- **PR A** = Tasks 1–2 (measure, drop 2.5.0). Green on 2.8.0+ before any auth work.
- **PR B** = Tasks 3–8 (prefix, auth, credentials, provisioner, external, signal).
- **Task 9** = live check and docs, on PR B's branch.

## File structure

| File | Responsibility |
|---|---|
| `tests/ranger/src/test/java/EgretPluginAuthTest.java` (new) | Real-jar proof of what the plugin sends; listed in `pom.xml` |
| `crates/egret-engine-mgmt-api/src/credential.rs` (new) | `EngineCredentialVerifier` trait, `EngineIdentity`, `plugin_username()` — the seam between the policy crate and engine-mgmt |
| `crates/egret-policy/src/ranger/auth.rs` | `Caller`, `require_surface_auth`, `require_operator` |
| `crates/egret-policy/src/ranger/routes.rs` | `RangerState` + `ranger_router` + new `ranger_surface` |
| `crates/egret-policy/src/ranger/api/download.rs` | profile-binding check in both download handlers |
| `crates/egret-engine-mgmt/src/ranger_credential.rs` (new) | mint / commit / rotate / verify of the engine credential |
| `crates/egret-engine-mgmt-api/src/ranger_config.rs` | renders `…/ranger` URL and the credential into the plugin XML |
| `crates/egret-provisioner/src/{trino,starrocks}.rs` | security XML moves to a Secret |
| `migrations/0200_engine_ranger_credential.sql` (new; renumber at creation) | two columns on `engine_mgmt.engines` |
| `migrations/0201_ranger_auth_failures.sql` (new; renumber) | per-engine auth-failure counter |

---

## Task 1: Measure the three open questions and commit the probe

**Files:**
- Create: `tests/ranger/src/test/java/EgretPluginAuthTest.java`
- Modify: `tests/ranger/pom.xml` (surefire `<includes>`), `tests/ranger/README.md`
- Create: `docs/en/plans/2026-10-20-ranger-surface-auth-measurements.md`

**Interfaces:**
- Produces: a written answer to Q1 (does a running plugin re-read its security XML?), Q2 (what does a 401 do to a warm vs. cold cache?), Q3 (which URLs does a plugin call in secure mode?). Tasks 4, 5 and 6 assume the answers given in the spec (Q1 no, Q2 warm keeps serving the cache / cold denies all, Q3 only the policies download); if a measurement disagrees, **stop and amend the spec** before continuing.

- [ ] **Step 1: Write the probe as a JUnit test**

`EgretSurefireIncludesTest` fails when a `*.java` in `src/test/java` is not named in `pom.xml`, so add the include first (Step 2). The test starts a `com.sun.net.httpserver.HttpServer` on `127.0.0.1:0`, builds a `RangerPluginConfig(type, "svc", type, "cl", type, null)`, sets `ranger.plugin.<type>.policy.rest.url`, `.service.name`, `.policy.cache.dir` (a fresh temp dir) and optionally `.policy.rest.client.username/password`, then calls `new RangerAdminRESTClient().init("svc","cl","ranger.plugin."+type,cfg)` and `getServicePoliciesIfUpdated(-1,0)`. The server records `METHOD path auth=<Authorization header>`.

```java
import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.nio.file.Files;
import java.util.*;
import java.util.concurrent.CopyOnWriteArrayList;
import org.apache.ranger.admin.client.RangerAdminRESTClient;
import org.apache.ranger.authorization.hadoop.config.RangerPluginConfig;
import org.apache.ranger.plugin.util.ServicePolicies;
import org.junit.Test;
import static org.junit.Assert.*;

/**
 * What a stock Ranger plugin SENDS to its admin. Measured, not assumed: egret's
 * auth design (docs/en/specs/2026-10-20-ranger-surface-auth-design.md) rests on it.
 * Run per version: mvn -q test -Dranger.version=2.8.0 -Dtest=EgretPluginAuthTest
 */
public class EgretPluginAuthTest {
  private static final String BODY =
      "{\"serviceName\":\"svc\",\"serviceId\":1,\"policyVersion\":5,\"policies\":[]}";

  private static List<String> poll(String type, int status, Map<String, String> extra) throws Exception {
    List<String> seen = new CopyOnWriteArrayList<>();
    HttpServer s = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
    s.createContext("/", ex -> {
      seen.add(ex.getRequestMethod() + " " + ex.getRequestURI().getPath()
          + " auth=" + ex.getRequestHeaders().getFirst("Authorization"));
      byte[] b = status == 200 ? BODY.getBytes() : new byte[0];
      ex.getResponseHeaders().add("Content-Type", "application/json");
      ex.sendResponseHeaders(status, status == 200 ? b.length : -1);
      if (status == 200) ex.getResponseBody().write(b);
      ex.close();
    });
    s.start();
    try {
      String prefix = "ranger.plugin." + type;
      RangerPluginConfig cfg = new RangerPluginConfig(type, "svc", type, "cl", type, null);
      cfg.set(prefix + ".policy.rest.url", "http://127.0.0.1:" + s.getAddress().getPort() + "/ranger");
      cfg.set(prefix + ".service.name", "svc");
      cfg.set(prefix + ".policy.cache.dir", Files.createTempDirectory("rcache").toString());
      extra.forEach((k, v) -> cfg.set(prefix + "." + k, v));
      RangerAdminRESTClient c = new RangerAdminRESTClient();
      c.init("svc", "cl", prefix, cfg);
      try { ServicePolicies sp = c.getServicePoliciesIfUpdated(-1, 0); seen.add("RESULT " + (sp == null ? "null" : "v" + sp.getPolicyVersion())); }
      catch (Exception e) { seen.add("RESULT exception " + e.getClass().getSimpleName()); }
      return seen;
    } finally { s.stop(0); }
  }

  private static final Map<String, String> CREDS = Map.of(
      "policy.rest.client.username", "engine-1", "policy.rest.client.password", "s3cret");

  @Test public void withCredentialsThePluginSendsBasicOnTheSecurePath() throws Exception {
    for (String type : List.of("trino", "starrocks")) {
      List<String> seen = poll(type, 200, CREDS);
      assertTrue(type + ": " + seen, seen.contains(
          "GET /ranger/service/plugins/secure/policies/download/svc auth=Basic ZW5naW5lLTE6czNjcmV0"));
      assertTrue(type + ": " + seen, seen.contains("RESULT v5"));
    }
  }

  @Test public void withoutCredentialsThePluginSendsNoHeaderOnTheLegacyPath() throws Exception {
    List<String> seen = poll("trino", 200, Map.of());
    assertTrue(seen.toString(), seen.contains("GET /ranger/service/plugins/policies/download/svc auth=null"));
  }

  @Test public void a401IsSurvivedWithoutAnException() throws Exception {
    // Q2 (cold cache): the result of a 401 on a first poll. Record it, whatever it is.
    List<String> seen = poll("trino", 401, CREDS);
    System.out.println("Q2-cold " + seen);
    assertTrue(seen.toString(), seen.stream().anyMatch(x -> x.startsWith("RESULT")));
  }
}
```

- [ ] **Step 2: List the test in `pom.xml`**

Add inside `<includes>` (after `EgretCaseFoldAgreementTest.java`), with a comment line in the file's existing style:

```xml
<!-- Ranger surface auth: what a stock plugin sends (Basic, secure path) at >= 2.8.0. -->
<include>EgretPluginAuthTest.java</include>
```

- [ ] **Step 3: Run on 2.8.0 and 2.9.0, expect PASS**

Run (from `tests/ranger`): `mvn -q test -Dranger.version=2.8.0 -Dtest='EgretPluginAuthTest,EgretSurefireIncludesTest'` then the same with `-Dranger.version=2.9.0`.
Expected: `Tests run: …, Failures: 0`. Also run once with `-Dranger.version=2.5.0` and confirm `withCredentialsThePluginSendsBasicOnTheSecurePath` **fails** (the header is null): that is the evidence for the floor. Paste that failure line into the measurements doc.

- [ ] **Step 4: Measure Q1 and Q3 and the warm-cache half of Q2**

Q1: extend the scratch probe (not the committed test): start the stub server, construct `RangerBasePlugin("trino","trino")` with a config file on disk (`ranger-trino-security.xml` rewritten via `Files.writeString` between two polls with a different `policy.rest.client.password`), poll interval 1000 ms, and report which `Authorization` value the server sees on poll 2. Q2 warm: first poll 200 (fills the cache dir), then the server answers 401; call `getServicePoliciesIfUpdated` again and report whether the plugin keeps its policies. Q3: with the stub answering 404 to everything except the policies path, run a full `RangerBasePlugin.init()` with credentials and list **every** path the server saw.
Record each result in `docs/en/plans/2026-10-20-ranger-surface-auth-measurements.md` as: question, method, exact output, and "agrees / disagrees with the spec". Scratch code stays in the scratchpad.

- [ ] **Step 5: Update the README and commit**

In `tests/ranger/README.md`: change the curl in "Optional — regenerate fixtures" to `…/ranger/service/plugins/policies/download/trino_prod`, and add one paragraph pointing at `EgretPluginAuthTest`.

```bash
git add tests/ranger docs/en/plans/2026-10-20-ranger-surface-auth-measurements.md
git commit -m "test(ranger): pin what a stock plugin sends to its admin, per version"
```

---

## Task 2: Drop plugin 2.5.0 / Trino 470 from support

**Files (verify every line number with grep before editing — they are from 2026-10-20 `origin/main`):**
- Modify: `crates/egret-perm/src/actions.rs` (~84, ~107), `crates/egret-perm/src/engine_images.rs` (~22, ~198, ~233, ~309–311)
- Modify: `crates/egret-engine-mgmt/src/external.rs` (~55 `KNOWN_RANGER_PLUGIN_VERSIONS`), `src/engines.rs` (~7279), `tests/no_floating_images.rs` (~180–188), `tests/routes.rs` (~25 sites), `tests/registry_schema_constraints.rs` (~654, ~743)
- Modify: `tests/ranger/pom.xml` (`<ranger.version>`), `tests/ranger/README.md`, `justfile` (147–210), `crates/egret-policy/tests/ranger_engine_gate.rs` (~37–40), `crates/egret-perm-compile/tests/gate_timing.rs` (~16), the `egret-ranger-eval` differential test version list
- Modify: `docs/en/guides/engine-versions.md`

**Interfaces:**
- Produces: `MODELLED_RANGER_PLUGINS` = `[("trino","2.8.0"), ("starrocks","2.8.0"), ("starrocks","2.9.0")]`; `egret_perm::ranger_plugin_is_modelled("trino","2.5.0") == false`; creating/updating an engine to a `< 2.8.0` plugin and registering an external engine declaring `< 2.8.0` both return **400** with a message naming the floor.

- [ ] **Step 1: Write the failing tests**

In `crates/egret-perm/src/actions.rs` tests module add:

```rust
#[test]
fn plugin_2_5_0_is_not_modelled_and_the_floor_is_2_8_0() {
    assert!(!ranger_plugin_is_modelled("trino", "2.5.0"));
    assert!(ranger_plugin_is_modelled("trino", "2.8.0"));
    assert!(ranger_plugin_is_modelled("starrocks", "2.8.0"));
    assert!(ranger_plugin_is_modelled("starrocks", "2.9.0"));
}
```

In `crates/egret-engine-mgmt/tests/routes.rs`, rewrite the test near line 11300 (the table of `(type, version, modelled)`) so the `("trino","2.5.0", …)` row expects a **400 at registration** instead of `true`; add one new test registering an external Trino with `"2.7.0"` and asserting 400 and that the body names `2.8.0`. Use the file's existing `register_external` / `register_body` helpers (see their use at ~12531).

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p egret-perm plugin_2_5_0_is_not_modelled -- --nocapture` and `cargo test -p egret-engine-mgmt --test routes registers_external -- --nocapture`
Expected: the first FAILS (`2.5.0` still modelled); the second FAILS (registration accepted).

- [ ] **Step 3: Implement**

1. `actions.rs`: delete `("trino", "2.5.0")` from the list and fix the doc sentence at ~107 to `Trino@2.8.0, StarRocks@2.8.0 and StarRocks@2.9.0`.
2. `engine_images.rs`: remove the `("trino","trinodb/trino","470","2.5.0")` row and the `plugin_version: "2.5.0"` default at ~22 (the default image is already 483; make the struct/test data agree). Tests at ~233/~311 use `trinodb/trino:483`.
3. `external.rs`: add next to `KNOWN_RANGER_PLUGIN_VERSIONS`

```rust
/// The lowest plugin release egret supports. Below it a plugin cannot send the
/// per-engine credential the Ranger surface requires (measured, spec §2).
pub const MIN_SUPPORTED_RANGER_PLUGIN: &str = "2.8.0";

/// `true` when `version` (`major.minor.patch`) is at least [`MIN_SUPPORTED_RANGER_PLUGIN`].
pub fn plugin_version_is_supported(version: &str) -> bool {
    fn parts(v: &str) -> Option<(u32, u32, u32)> {
        let mut it = v.split('.').map(|p| p.parse::<u32>().ok());
        Some((it.next()??, it.next()??, it.next()??))
    }
    matches!((parts(version), parts(MIN_SUPPORTED_RANGER_PLUGIN)), (Some(v), Some(min)) if v >= min)
}
```

   and remove `"2.0.0".."2.7.0"` from `KNOWN_RANGER_PLUGIN_VERSIONS` (keep the const's type honest: `[&str; 2]` = `["2.8.0","2.9.0"]`). Registration's existing 400 path for an unknown version then fires; make its message `"Ranger plugin {v} is not supported: egret requires {MIN_SUPPORTED_RANGER_PLUGIN} or later"`.
4. The create/update path for provisioned engines: the plugin version is derived from the image (`engine_images.rs`), so an image that maps to `< 2.8.0`, or a `trinodb/trino:470` tag, must now resolve to *unknown* and be refused by the existing "tag egret has not measured" rule. Confirm with a test: `PUT /engines/{id}` with `imageVersion: "470"` ⇒ 400.
5. Fixtures: replace `"2.5.0"` with `"2.8.0"` at every site in `routes.rs`, `registry_schema_constraints.rs`, `engines.rs:~8984` and `lineage_download_db.rs:79`. Replace `trinodb/trino:470` with `trinodb/trino:483` in `no_floating_images.rs` and `engines.rs:~7279`.
6. Harness: `pom.xml` `<ranger.version>` → `2.8.0`; `justfile`/`ranger_engine_gate.rs`/`gate_timing.rs`/differential version lists → `["2.8.0","2.9.0"]`; README: replace "Why 2.5.0 is the default" with a short "Why 2.8.0 is the floor" citing Task 1's measurement.
7. `engine-versions.md`: delete the Trino 470 row, state the floor and the upgrade path.
8. **Do not edit** migrations, old plans/specs, or `ambiguous_arbitration.rs:118` (history).

- [ ] **Step 4: Verify**

Run:
```bash
cargo test -p egret-perm -p egret-engine-mgmt --no-fail-fast
just test-ranger-gates
grep -rIn --exclude-dir=target --exclude-dir=node_modules --exclude-dir=.git -E '2\.5\.0|trino:470' crates tests justfile docs/en/guides | grep -v '^crates/.*migrations'
```
Expected: tests pass; the gate reports 0 disagreements on 2.8.0 and 2.9.0; the grep returns only comments that describe history (review each hit — a hit in live code or a fixture is a miss).

- [ ] **Step 5: Commit (PR A ends here)**

```bash
git add -A crates tests justfile docs/en/guides
git commit -m "feat(engines)!: drop Ranger plugin 2.5.0 / Trino 470; the floor is 2.8.0"
```

---

## Task 3: Move the surface to `/ranger`, add the secure path, remove `/service`

**Files:**
- Modify: `crates/egret-engine-mgmt-api/src/ranger_config.rs`, `crates/egret-policy/src/ranger/routes.rs`, `crates/egret-policy/src/lib.rs` (re-export), `crates/egret-server/src/lib.rs:~289`
- Modify: `crates/egret-engine-mgmt/src/external.rs:~790` (`download_url`), golden `crates/egret-engine-mgmt-api/tests/golden/*.xml`
- Modify: `crates/egret-policy/tests/http_ranger.rs`, `tests/ranger/README.md`, `justfile`/`scripts` that curl `/service` (find with grep)
- Test: `crates/egret-policy/tests/http_ranger.rs`

**Interfaces:**
- Produces: `pub const RANGER_SURFACE_PREFIX: &str = "/ranger";` in `egret-engine-mgmt-api::ranger_config`; `pub fn ranger_surface(state: RangerState) -> Router` in `egret-policy` (the router nested under the prefix, with `/service` inside it); `ranger_router` additionally serves `/plugins/secure/policies/download/{service}`.

- [ ] **Step 1: Write the failing tests**

In `http_ranger.rs`, following the file's existing request helpers, add:

```rust
#[tokio::test]
async fn the_secure_path_serves_the_same_body_as_the_plain_path() {
    let app = test_surface().await; // the helper the other tests in this file use, but built with ranger_surface
    let plain = get(&app, "/ranger/service/plugins/policies/download/trino_prod").await;
    let secure = get(&app, "/ranger/service/plugins/secure/policies/download/trino_prod").await;
    assert_eq!(plain.status(), 200);
    assert_eq!(secure.status(), 200);
    assert_eq!(body_json(plain).await, body_json(secure).await);
}

#[tokio::test]
async fn the_old_root_service_prefix_is_gone() {
    let app = test_surface().await;
    let resp = get(&app, "/service/plugins/policies/download/trino_prod").await;
    assert_eq!(resp.status(), 404);
}
```

(If the helpers are named differently, use the file's own; the assertions are the point. Run the surface in `anonymous` mode here — Task 4 introduces the mode; until then the router is anonymous by default.)

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p egret-policy --test http_ranger the_secure_path the_old_root -- --nocapture`
Expected: FAIL (`ranger_surface` undefined / 404 on the secure path).

- [ ] **Step 3: Implement**

`ranger_config.rs`:
```rust
/// Where egret serves the Ranger plugin surface. The plugin appends
/// `/service/plugins/...` to its configured URL, so the URL it is given is
/// `<egret>` + this prefix. One spelling, used by the server and the renderer.
pub const RANGER_SURFACE_PREFIX: &str = "/ranger";

/// The URL an engine's plugin is configured with.
pub fn plugin_base_url(egret_url: &str) -> String {
    format!("{}{RANGER_SURFACE_PREFIX}", egret_url.trim_end_matches('/'))
}
```
`security_xml` uses `plugin_base_url(url)` for `policy.rest.url`. Update the doc comment of `ranger_plugin_files` ("the plugin appends `/service/...`" stays true).

`routes.rs`: add the route and the nesting function:
```rust
.route(
    "/plugins/secure/policies/download/{service}",
    get(api::download::download_policies),
)
```
```rust
/// The whole Ranger surface as the server mounts it: `/ranger/service/...`.
pub fn ranger_surface(state: RangerState) -> Router {
    Router::new().nest(
        egret_engine_mgmt_api::ranger_config::RANGER_SURFACE_PREFIX,
        Router::new().nest("/service", ranger_router(state)),
    )
}
```
Export `ranger_surface` next to `ranger_router` in `egret-policy/src/lib.rs`. In `egret-server/src/lib.rs` replace `.nest("/service", egret_policy::ranger_router(ranger))` with `.merge(egret_policy::ranger_surface(ranger))`.

`external.rs`: `download_url` becomes `format!("{}/service/plugins/policies/download/{service_name}", plugin_base_url(&st.engine_egret_url))`; replace the warning text that says "egret requires a bearer token on /service" with `/ranger` (full rewrite of these judgements happens in Task 7).

Regenerate the golden XML (`tests/golden/trino.ranger-trino-security.xml`, the StarRocks one) with the test's own regeneration switch (see the golden test's header for the env var); review the diff: only the `policy.rest.url` value changes. Update the README curl and any script found by `grep -rn "/service/plugins" justfile scripts deploy docs/en/guides`.

- [ ] **Step 4: Run, expect PASS; regenerate the OpenAPI**

Run: `cargo test -p egret-policy -p egret-engine-mgmt-api -p egret-server --no-fail-fast`, then `npm run gen:api` in `frontend` (CI fails on drift; the utoipa `path = "/service/plugins/…"` annotations in `download.rs` and `crud.rs` must become `/ranger/service/plugins/…` — change them and commit what `gen:api` produces, check `encodeURIComponent` survived).

- [ ] **Step 5: Commit**

```bash
git add -A crates tests docs frontend justfile scripts
git commit -m "feat(ranger)!: serve the plugin surface under /ranger and add the secure download path"
```

---

## Task 4: Authenticate the surface: config, middleware, profile binding

**Files:**
- Create: `crates/egret-engine-mgmt-api/src/credential.rs` (+ `pub mod credential;` and re-exports in `lib.rs`)
- Modify: `crates/egret-config/src/lib.rs` (`RangerSettings`, validation), `crates/egret-policy/src/ranger/{auth.rs,routes.rs,api/download.rs}`, `crates/egret-bin/src/main.rs:~1426`, `deploy/helm/egret/values.yaml` (+ template), `egret.toml`
- Test: `crates/egret-policy/tests/ranger_surface_auth.rs` (new)

**Interfaces:**
- Consumes: `RANGER_SURFACE_PREFIX`, `ranger_surface` (Task 3).
- Produces (in `egret-engine-mgmt-api::credential`):

```rust
use uuid::Uuid;

/// What a verified engine credential entitles its holder to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineIdentity {
    pub engine_id: Uuid,
    /// The policy profile (`policy.authz_services.id`) the engine is bound to.
    pub authz_service_id: Option<i64>,
}

/// The HTTP Basic username an engine's plugin presents.
pub fn plugin_username(engine_id: Uuid) -> String {
    format!("engine-{engine_id}")
}

/// The engine id inside a plugin username, if it is one.
pub fn engine_id_from_username(username: &str) -> Option<Uuid> {
    username.strip_prefix("engine-").and_then(|s| Uuid::parse_str(s).ok())
}

#[async_trait::async_trait]
pub trait EngineCredentialVerifier: Send + Sync {
    /// `Ok(Some(_))` when `password` is a currently valid credential of `engine_id`;
    /// `Ok(None)` for any mismatch or an unknown engine (callers must not distinguish);
    /// `Err` only for an infrastructure failure.
    async fn verify(&self, engine_id: Uuid, password: &str) -> Result<Option<EngineIdentity>, String>;
}
```
- Produces (in `egret-policy::ranger::auth`): `pub enum Caller { Operator, Engine(EngineIdentity), Anonymous }`, `pub async fn require_surface_auth`, `pub async fn require_operator`; `RangerState` gains `pub mode: AuthMode` and `pub verifier: Option<Arc<dyn EngineCredentialVerifier>>`.

- [ ] **Step 1: Write the failing tests** (`ranger_surface_auth.rs`)

Use a stub verifier so no DB engine table is needed for the middleware logic; the DB-backed binding test follows the DB-gated pattern of `plugin_downloads_db.rs` (read its header for how it obtains a pool and seeds a service).

```rust
use std::sync::Arc;

struct Stub { good: (uuid::Uuid, &'static str), bound_to: Option<i64> }

#[async_trait::async_trait]
impl egret_engine_mgmt_api::credential::EngineCredentialVerifier for Stub {
    async fn verify(&self, id: uuid::Uuid, pw: &str)
        -> Result<Option<egret_engine_mgmt_api::credential::EngineIdentity>, String> {
        Ok((id == self.good.0 && pw == self.good.1).then(|| egret_engine_mgmt_api::credential::EngineIdentity {
            engine_id: id, authz_service_id: self.bound_to }))
    }
}

fn basic(user: &str, pw: &str) -> String {
    use base64::Engine as _;
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pw}")))
}

#[tokio::test] async fn required_mode_refuses_anonymous() { /* GET download, no header ⇒ 401 */ }
#[tokio::test] async fn required_mode_accepts_a_valid_engine_basic_credential() { /* 200 */ }
#[tokio::test] async fn a_wrong_password_is_401() { /* 401, body has no hint which part was wrong */ }
#[tokio::test] async fn an_engine_credential_for_another_profile_is_403() { /* bound_to = Some(other) ⇒ 403 */ }
#[tokio::test] async fn an_engine_credential_cannot_call_the_crud_routes() { /* GET /plugins/services ⇒ 403 */ }
#[tokio::test] async fn the_operator_bearer_still_works_everywhere() { /* 200 on download and on /plugins/services */ }
#[tokio::test] async fn anonymous_mode_serves_without_credentials() { /* mode = Anonymous ⇒ 200 */ }
#[tokio::test] async fn an_unserved_route_under_the_surface_is_401_not_404_without_credentials() { /* GET /ranger/service/roles/secure/download/trino_prod with no/bad credentials in Required mode ⇒ 401 (measured: a 2.8.0 plugin polls this route; egret does not serve it, so valid credentials ⇒ 404) */ }
#[tokio::test] async fn the_tag_service_of_the_bound_profile_is_allowed() { /* DB-gated: profile P with tagService T ⇒ engine bound to P may download T, not an unrelated tag service */ }
```
Fill each body using the request helper pattern from `http_ranger.rs` (build the app with `ranger_surface(RangerState { store, auth_token, mode, verifier })`, call `tower::ServiceExt::oneshot`). Assert on status codes only plus, for 401, that the response carries `WWW-Authenticate: Basic realm="egret-ranger"`.

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p egret-policy --test ranger_surface_auth` — Expected: compile error (`credential`, `AuthMode` undefined).

- [ ] **Step 3: Implement**

`egret-config`: in `RangerSettings` add `#[serde(default)] pub auth_mode: RangerAuthMode` with

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RangerAuthMode {
    #[default]
    Required,
    Anonymous,
}
```
Update the construction sites in `lib.rs` (~1673/1687 defaults and the tests at ~2993), replace the comment at ~1763 ("ranger.auth_token may be empty…") with: *an empty token is fine; engines authenticate with their own credentials; `anonymous` must be chosen explicitly*. Add a `tracing::warn!` in `egret-bin` when `Anonymous`.

`auth.rs`: replace `require_bearer`:

```rust
#[derive(Clone, Debug)]
pub enum Caller { Operator, Engine(EngineIdentity), Anonymous }

pub async fn require_surface_auth(
    State(state): State<RangerState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, Response> {
    let header = req.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    let caller = if let Some(t) = header.and_then(|v| v.strip_prefix("Bearer ")) {
        (!state.auth_token.is_empty() && ct_eq(t.as_bytes(), state.auth_token.as_bytes()))
            .then_some(Caller::Operator)
    } else if let Some((user, pw)) = header.and_then(|v| v.strip_prefix("Basic ")).and_then(decode_basic) {
        match (engine_id_from_username(&user), state.verifier.as_ref()) {
            (Some(id), Some(v)) => match v.verify(id, &pw).await {
                Ok(found) => found.map(Caller::Engine),
                Err(e) => { tracing::error!(error = %e, "engine credential check failed"); return Err(StatusCode::INTERNAL_SERVER_ERROR.into_response()); }
            },
            _ => None,
        }
    } else { None };
    let caller = match (caller, state.mode) {
        (Some(c), _) => c,
        (None, AuthMode::Anonymous) => Caller::Anonymous,
        (None, AuthMode::Required) => return Err(unauthorized()),
    };
    req.extensions_mut().insert(caller);
    Ok(next.run(req).await)
}

fn unauthorized() -> Response {
    let mut r = StatusCode::UNAUTHORIZED.into_response();
    r.headers_mut().insert(WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"egret-ranger\""));
    r
}
```
`decode_basic` base64-decodes and splits at the first `:`. A bad Basic header ⇒ `None` ⇒ 401 (never a 400 that reveals parsing). Keep `ct_eq`. In `Anonymous` mode a *bad* credential is also let through as `Anonymous` — document that in the doc comment ("anonymous mode serves everyone").

`require_operator`: reads `Caller` from extensions; `Engine(_)` ⇒ 403; `Operator`/`Anonymous` ⇒ pass. In `routes.rs` layer `require_surface_auth` over the whole router **including its fallback** (Task 1 measured that a 2.8.0 plugin also polls `/service/roles/secure/download/<svc>`, which egret does not serve: bad credentials there must be 401, valid ones 404 — attach the layer so unmatched routes pass through it, e.g. `.fallback(not_found).layer(...)`, and prove it with the test above) and `require_operator` over `api::crud_routes()` only (`.merge(api::crud_routes().layer(from_fn(require_operator)))` — the layer must wrap before the merge).

`download.rs`: add

```rust
/// 403 unless `caller` may read `svc`: an engine credential reads only the profile its engine is bound to,
/// plus the tag service that profile names. Operators and anonymous-mode callers read anything.
async fn authorize_service(store: &PolicyStore, caller: &Caller, svc: &crate::ranger::dto::Service) -> Result<(), RangerError> {
    let Caller::Engine(id) = caller else { return Ok(()) };
    let Some(bound) = id.authz_service_id else { return Err(RangerError::forbidden()) };
    if bound == svc.id { return Ok(()) }
    if store.tag_service_of(bound).await?.is_some_and(|t| t == svc.id) { return Ok(()) }
    tracing::warn!(engine_id = %id.engine_id, requested = %svc.name, "engine credential asked for a service it is not bound to");
    Err(RangerError::forbidden())
}
```
`Service` type and `RangerError::forbidden()` are placeholders for the file's real names: read `store.get_service` return type and `ranger/error.rs`, and add a `Forbidden` variant mapping to 403 with `RangerErrorBody` if none exists. `tag_service_of(profile_id) -> Result<Option<i64>>` reads `config_jsonb->>'tagService'` (the `egret_policy_api::TAG_SERVICE_CONFIG_KEY` constant used in `download_policies`) and resolves the named service id. Call `authorize_service` right after `get_service` in **both** `download_policies` and `download_tags` (extract `Extension(caller): Extension<Caller>`).

`main.rs`: build `RangerState { store, auth_token, mode: config.ranger.auth_mode.into(), verifier }` — `verifier` is `None` until Task 5 provides the implementation; pass `None` for now (engine Basic then never authenticates, which is correct until Task 5 lands, and the tests use the stub).

Helm: add `ranger.authMode: required` to `values.yaml` and thread it to `EGRET_RANGER__AUTH_MODE` in the deployment template (find the existing `ranger.authToken` plumbing with `grep -rn authToken deploy/helm`); dev compose and `egret.toml` set `auth_mode = "anonymous"` **only** where the dev stack has no engine credential yet — Task 6 removes that exception.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p egret-policy --test ranger_surface_auth --test http_ranger` and `cargo clippy -p egret-policy -p egret-config -p egret-engine-mgmt-api -- -D warnings`.

- [ ] **Step 5: Mutation check, then commit**

Temporarily make `authorize_service` return `Ok(())` for every caller; confirm `an_engine_credential_for_another_profile_is_403` fails; revert (diff must be empty).

```bash
git add -A crates deploy egret.toml
git commit -m "feat(ranger): authenticate the plugin surface; an engine reads only its own profile"
```

---

## Task 5: Engine credential: storage, minting, rotation, verification

**Files:**
- Create: `migrations/0200_engine_ranger_credential.sql` (renumber at creation), `crates/egret-engine-mgmt/src/ranger_credential.rs`
- Modify: `crates/egret-engine-mgmt/src/{lib.rs,engines.rs}` (module, build/apply hooks, route), the `Engine` struct and its row mapping in the engine store, `crates/egret-bin/src/main.rs` (pass the verifier), `crates/egret-engine-mgmt/src/engines.rs` engine-delete path (secret cleanup)
- Test: `crates/egret-engine-mgmt/tests/ranger_credential_db.rs` (new, DB-gated like `registry_schema_constraints.rs`)

**Interfaces:**
- Consumes: `EngineCredentialVerifier`, `plugin_username` (Task 4); `random_internal_secret()` and `st.secrets` (`create`, `resolve_plaintext`, `delete`) from `engines.rs` — Trino's existing internal-secret pair is **Trino-only** (see `rotate_engine_secrets`), so this credential is separate and works for StarRocks too.
- Produces:

```rust
pub struct PluginCredential { pub username: String, pub password: String }   // Debug redacts password

/// The credential to render into the engine's plugin config: reuse the committed one, or mint and commit one.
pub(crate) async fn ensure(st: &CpState, engine: &Engine) -> Result<PluginCredential, CpError>;
/// Mint a new current credential, demote the old to "previous", return the new one.
pub(crate) async fn rotate(st: &CpState, engine: &Engine) -> Result<PluginCredential, CpError>;
/// (Superseded by Task 6b.) Retire the previous credential only once the rollout of
/// the current one is OBSERVED complete: exact id, compare-and-set on both columns;
/// the unconditional `retire_previous` exists only behind the `test-util` feature.
pub async fn retire_previous_if_rolled_out(&self, engine: &Engine, port: &dyn EngineProvisioningPort) -> RolloutRetire;
pub struct DbVerifier { /* engine store + secrets */ }
impl EngineCredentialVerifier for DbVerifier { /* current and previous, constant time */ }
```

**Design (why two columns):** the verifier accepts `ranger_secret_id` (current) **and** `ranger_secret_prev_id` (previous). A rotation commits the new current and demotes the old to previous *before* the apply; both authenticate while the pods roll; the previous one is deleted (by exact id, compare-and-set on both columns) only once the rollout of the current one is OBSERVED complete (Task 6b: pod-template/Secret annotation `egret/ranger-credential` = the current credential's id, workload status fully rolled, no pod of an older template left; each credential rendered into its own Secret `{name}-rc-{tag}`, deleted by exact name after its retire), never when Kubernetes merely accepts the apply — never a prefix sweep (standing rule).

- [ ] **Step 1: Migration**

```sql
-- Per-engine Ranger plugin credential (spec 2026-10-20-ranger-surface-auth).
-- `ranger_secret_id` is the password the engine's plugin presents now;
-- `ranger_secret_prev_id` is the one it presented before the last rotation, kept
-- valid until the rollout is observed complete (never delete a secret
-- the cluster may still hold). Both name rows in the secret store; neither is
-- a foreign key because the secret store is a separate seam.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN ranger_secret_id      uuid,
    ADD COLUMN ranger_secret_prev_id uuid;
```
Check `0184_engine_external.sql`'s CHECK: it constrains named columns to NULL for external engines and does not name these, so external engines may carry a credential. Re-run `git ls-tree origin/main migrations/ | tail -3` and renumber if main moved.

- [ ] **Step 2: Write the failing DB tests** (`ranger_credential_db.rs`)

```rust
#[tokio::test] async fn ensure_mints_once_and_reuses() { /* ensure twice ⇒ same password; row has ranger_secret_id */ }
#[tokio::test] async fn rotate_demotes_the_old_credential_and_both_verify() {
    /* p1 = ensure; p2 = rotate; verify(p1) and verify(p2) both Some; p1 != p2 */ }
#[tokio::test] async fn retire_previous_revokes_only_the_old_one() {
    /* after retire_previous: verify(p1) None, verify(p2) Some; prev column NULL */ }
#[tokio::test] async fn a_second_rotation_before_retiring_deletes_the_older_previous() {
    /* p1, p2 = rotate, p3 = rotate ⇒ p1 secret deleted (it is no longer "previous"), p2 and p3 verify */ }
#[tokio::test] async fn verify_is_none_for_an_unknown_engine_and_for_a_wrong_password() {}
#[tokio::test] async fn verify_returns_the_engines_bound_profile() { /* identity.authz_service_id == the engine's authz_service_id */ }
#[tokio::test] async fn concurrent_ensure_converges_on_one_credential() {
    /* two ensure calls via tokio::join!; both return the same password; exactly one secret row survives for the engine */ }
```
Scope every query to the test's own engine id: the test database is shared with concurrently running tests (never an unscoped `SELECT … FROM secrets`).

- [ ] **Step 3: Run to confirm failure** — `cargo test -p egret-engine-mgmt --test ranger_credential_db` ⇒ compile error.

- [ ] **Step 4: Implement `ranger_credential.rs`**

Key pieces (the module also needs the repo's usual `CpState`/`CpError` imports — copy them from `external.rs`):

```rust
fn secret_name(engine_id: Uuid) -> String {
    format!("engine-ranger-{engine_id}-{}", Uuid::new_v4().simple())
}

pub(crate) async fn ensure(st: &CpState, engine: &Engine) -> Result<PluginCredential, CpError> {
    if let Some(id) = engine.ranger_secret_id {
        match st.secrets.resolve_plaintext(None, id).await {
            Ok(password) => return Ok(PluginCredential { username: plugin_username(engine.id), password }),
            Err(egret_secrets::SecretsError::NotFound) => {} // deleted out-of-band: mint a fresh one
            Err(e) => return Err(CpError::BadRequest(format!("resolve ranger credential: {e}"))),
        }
    }
    mint_and_commit(st, engine, /*demote*/ false).await
}

pub(crate) async fn rotate(st: &CpState, engine: &Engine) -> Result<PluginCredential, CpError> {
    mint_and_commit(st, engine, /*demote*/ true).await
}

async fn mint_and_commit(st: &CpState, engine: &Engine, demote: bool) -> Result<PluginCredential, CpError> {
    let password = random_internal_secret(); // make it pub(crate) in engines.rs
    let new_id = st.secrets
        .create(None, &secret_name(engine.id), Some("Ranger plugin credential"), &password)
        .await
        .map_err(|e| CpError::BadRequest(format!("store ranger credential: {e}")))?;
    // CAS on the id this call READ, like commit_internal_secrets: a concurrent writer wins cleanly.
    let new_prev = if demote { engine.ranger_secret_id } else { engine.ranger_secret_prev_id };
    let committed = st.engine_mgmt
        .set_ranger_secret_ids(engine.id, engine.ranger_secret_id, new_id, new_prev)
        .await?;
    if !committed {
        // Nothing has been handed to a cluster yet, so discarding our own secret is safe.
        let _ = st.secrets.delete(None, new_id).await;
        let fresh = st.engine_mgmt.get_engine(engine.id).await?.ok_or(CpError::NotFound)?;
        return ensure_committed_only(st, &fresh).await; // resolve the winner's credential; never mint again
    }
    // A demoted-away older previous is no longer reachable by any pod: it was already replaced by `engine.ranger_secret_id`.
    if demote {
        if let Some(older) = engine.ranger_secret_prev_id.filter(|o| Some(*o) != engine.ranger_secret_id) {
            if let Err(e) = st.secrets.delete(None, older).await {
                tracing::warn!(engine_id = %engine.id, error = %e, "could not delete a superseded ranger credential");
            }
        }
    }
    Ok(PluginCredential { username: plugin_username(engine.id), password })
}
```
`set_ranger_secret_ids(id, expected_current, new_current, new_prev) -> Result<bool>` goes in the engine store next to `set_internal_secret_ids` (copy its SQL shape: `UPDATE engine_mgmt.engines SET ranger_secret_id=$3, ranger_secret_prev_id=$4 WHERE id=$1 AND ranger_secret_id IS NOT DISTINCT FROM $2`, return `rows_affected() == 1`). `ensure_committed_only` is `ensure` without the mint branch (returns a 500-class `CpError` if the winner's secret is missing). Add `ranger_secret_id` / `ranger_secret_prev_id: Option<Uuid>` to the `Engine` struct and every `SELECT` list/`FromRow` that builds it (grep `monitor_secret_id` to find them all).

`retire_previous`:
```rust
pub(crate) async fn retire_previous(st: &CpState, engine_id: Uuid) {
    let Ok(Some(e)) = st.engine_mgmt.get_engine(engine_id).await else { return };
    let Some(prev) = e.ranger_secret_prev_id else { return };
    // Clear the column first (CAS on `prev`): if the clear loses, a newer rotation owns the column — do not delete.
    match st.engine_mgmt.clear_ranger_prev(engine_id, prev).await {
        Ok(true) => if let Err(err) = st.secrets.delete(None, prev).await {
            tracing::warn!(engine_id = %engine_id, error = %err, "could not delete a retired ranger credential");
        },
        Ok(false) => {}
        Err(err) => tracing::warn!(engine_id = %engine_id, error = %err, "could not clear the previous ranger credential"),
    }
}
```

`DbVerifier::verify(engine_id, password)`: load the engine (`None` ⇒ `Ok(None)`), collect `[ranger_secret_id, ranger_secret_prev_id]` flatten, `resolve_plaintext` each (`NotFound` ⇒ skip), compare with a constant-time equality (reuse the `ct_eq` shape from `auth.rs`; a 6-line copy is acceptable to avoid a cross-crate dependency), and return `EngineIdentity { engine_id, authz_service_id: engine.authz_service_id }`. Always evaluate every candidate (no early return on first match) so timing does not reveal which slot matched.

Wire it: in `main.rs` build the engine-mgmt state before the Ranger state if it is not already (read the order; if engine-mgmt state is constructed after `RangerState`, build `DbVerifier` from the same `Db` + `WorkspaceSecretStore` handles it needs — it only needs the engine store and secrets, not the whole `CpState`) and pass `verifier: Some(Arc::new(DbVerifier::new(..)))`.

Engine delete: where the delete path removes the internal secrets (`Task 15` comment in `commit_internal_secrets`), also delete `ranger_secret_id` and `ranger_secret_prev_id`, and prefix-sweep `engine-ranger-{id}-*` using the same method that path uses for `engine-internal-{id}-*` (escape-safe: compare with `left(name, length($1)) = $1`, not `LIKE` — an unescaped `_` in `LIKE` once dropped a database).

- [ ] **Step 5: Run, expect PASS; mutation; commit**

Run: `cargo test -p egret-engine-mgmt --test ranger_credential_db`. Mutation: make `retire_previous` delete without the CAS; the "second rotation" test or a dedicated race test must fail; revert.

```bash
git add -A migrations crates
git commit -m "feat(engines): a per-engine Ranger plugin credential with a safe rotation window"
```

---

## Task 6: Provisioner: render the credential into a Kubernetes Secret

**Files:**
- Modify: `crates/egret-engine-mgmt-api/src/ranger_config.rs` (`ranger_plugin_files` signature, `security_xml`), `crates/egret-provisioner/src/trino.rs` (config map builder ~356, volume mount ~669, volumes), `crates/egret-provisioner/src/starrocks.rs` (~185–215, mounts ~625, cleanup list ~1146), the `ranger:` spec struct (find with `grep -rn "poll_interval_ms" crates/egret-provisioner/src`), `crates/egret-engine-mgmt/src/engines.rs` (build + apply hooks), `crates/egret-policy/src/admin/mod.rs:~1022`, golden files
- Test: provisioner unit tests in the same files; `crates/egret-engine-mgmt/tests/routes.rs` apply test

**Interfaces:**
- Consumes: `ensure`, `rotate`, `retire_previous`, `PluginCredential` (Task 5), `plugin_base_url` (Task 3).
- Produces:

```rust
// egret-engine-mgmt-api::ranger_config
pub struct PluginCredentialRef<'a> { pub username: &'a str, pub password: &'a str }
pub fn ranger_plugin_files(kind: EngineKind, service_name: &str, egret_url: &str,
    poll_interval_ms: u64, credential: Option<PluginCredentialRef<'_>>) -> RangerPluginFiles;
/// Shown in the files an operator installs by hand when the password is not repeated.
pub const PASSWORD_PLACEHOLDER: &str = "REPLACE_WITH_THE_ENGINE_PASSWORD";
```
Rendered when `credential` is `Some`: two extra properties in the security XML,
`ranger.plugin.<p>.policy.rest.client.username` and `.password`. No other file changes.
Provisioner: new Secret `{name}-ranger-secret` holding the security XML; `{name}-config` (Trino) and `{name}-ranger-cm` (StarRocks) no longer carry it.

- [ ] **Step 1: Write the failing tests**

```rust
// ranger_config.rs tests
#[test]
fn the_security_xml_carries_the_credential_only_when_given() {
    let none = ranger_plugin_files(EngineKind::Trino, "svc", "http://e", 30_000, None);
    assert!(!none.get(TRINO_SECURITY_XML).unwrap().contains("rest.client.password"));
    let with = ranger_plugin_files(EngineKind::Trino, "svc", "http://e", 30_000,
        Some(PluginCredentialRef { username: "engine-1", password: "pw" }));
    let xml = with.get(TRINO_SECURITY_XML).unwrap();
    assert!(xml.contains("<name>ranger.plugin.trino.policy.rest.client.username</name><value>engine-1</value>"));
    assert!(xml.contains("<name>ranger.plugin.trino.policy.rest.client.password</name><value>pw</value>"));
    assert!(xml.contains("<value>http://e/ranger</value>"));
}
#[test] fn no_starrocks_file_contains_a_double_hyphen_with_a_credential() { /* extend the existing test to pass a credential whose password contains '-' and '_' */ }
```
In `trino.rs`/`starrocks.rs` tests assert: the ConfigMap's `data` has **no** key `ranger-trino-security.xml` / `ranger-starrocks-security.xml`; a Secret named `{name}-ranger-secret` exists and holds it; the StatefulSet/Pod template has a volume sourcing that Secret and the XML mount's `name` is that volume (Trino: `volume_mounts` entry for `/etc/trino/ranger-trino-security.xml` references the secret volume; StarRocks: the `ranger-cm` mount loop at ~625 mounts the audit key from the ConfigMap and the security key from the Secret); the StarRocks delete list (~1146) contains `("v1","Secret", "{name}-ranger-secret")`.

- [ ] **Step 2: Run to confirm failure** — `cargo test -p egret-engine-mgmt-api -p egret-provisioner` ⇒ compile errors / assertion failures.

- [ ] **Step 3: Implement**

`security_xml(plugin, svc, url, poll, credential)` appends, before `</configuration>`:
```rust
let creds = credential.map(|c| format!(
  "  <property><name>ranger.plugin.{plugin}.policy.rest.client.username</name><value>{u}</value></property>\n\
     <property><name>ranger.plugin.{plugin}.policy.rest.client.password</name><value>{p}</value></property>\n",
  u = c.username, p = c.password)).unwrap_or_default();
```
(Escape nothing: username is `engine-<uuid>` and the password is URL-safe base64; add a debug assertion that neither contains `<`, `&` or `--`.) Update every caller of `ranger_plugin_files` (`trino.rs:~356`, `starrocks.rs:~193`, `external.rs:~765`, `admin/mod.rs:~1022`, `routes.rs:~13290`, the starrocks test at ~1366) with `None` where no credential applies yet.

Spec struct: add `pub credential: Option<(String, String)>` to the provisioner's `ranger` binding struct; both builders pass `credential.as_ref().map(|(u,p)| PluginCredentialRef{..})`.

Trino: split the security XML out of the `{name}-config` ConfigMap into a new builder `trino_ranger_secret(spec) -> Option<Value>` returning

```rust
json!({ "apiVersion": "v1", "kind": "Secret", "type": "Opaque",
        "metadata": { "name": format!("{}-ranger-secret", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": { "ranger-trino-security.xml": xml } })
```
add a pod volume `{ "name": "ranger-secret", "secret": { "secretName": "{name}-ranger-secret" } }` and point the existing `volume_mounts.push(... "ranger-trino-security.xml" ...)` at `"name": "ranger-secret"`. Apply it wherever `trino_catalogs_config_map` / the config map are applied and add it to the engine's teardown list (copy how the TLS Secret is applied and removed; `starrocks.rs:~1146` shows the delete list shape). Do the equivalent for StarRocks (`starrocks_ranger_secret`), keeping the audit XML in `-ranger-cm`.

Pods must roll when the Secret changes: add the Secret's content hash to the pod template annotation the way the ConfigMap's hash is added (find it with `grep -n "checksum\|config-hash" crates/egret-provisioner/src/*.rs`; if the ConfigMap has no such annotation, add `egret/ranger-secret-hash: <sha256 hex of the XML>` to both pod templates — without it a rotation would not restart the pods).

Engine build/apply (`engines.rs`): in `build_provision_req_with`, for an engine bound to a profile, `let cred = ranger_credential::ensure(st, engine).await?` (or `rotate` when `BuildOpts.rotate_ranger_credential`), and put it in the spec. ~~After a **successful** `apply_built`, call `ranger_credential::retire_previous(st, engine.id).await`.~~ Superseded by Task 6b (user decision 2026-10-20): nothing is retired after the apply; `ranger_credential::retire_previous_if_rolled_out` retires the previous credential once the provisioner observes the rollout of the current one complete (periodic sweep + engine reads). Add `rotate_ranger_credential: bool` to `BuildOpts` (default false).

Rotation route (all provisioned engines, both types): `POST /engines/{id}/rotate-ranger-credential`, `engine:update`, 409 for external (Task 7 adds the external variant), 204 on success; mirrors `rotate_engine_secrets` (including the `deleting` guard and the `apply_conflict()` mapping). Register it in the router and the OpenAPI `paths(...)` list; then `npm run gen:api`.

Remove the dev exception: `egret.toml`, dev compose and the kind helm values no longer need `auth_mode = "anonymous"` once provisioned engines carry credentials; delete those overrides.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p egret-engine-mgmt-api -p egret-provisioner -p egret-engine-mgmt --no-fail-fast` (the `routes.rs` apply tests use a fake provisioner: assert it received the Secret and that the apply's request contains a credential whose password verifies through `DbVerifier`), then `just test-ranger-gates`.

- [ ] **Step 5: Commit**

```bash
git add -A crates deploy egret.toml frontend/src/api
git commit -m "feat(provisioner): give each engine its Ranger credential through a Kubernetes Secret"
```

---

## Task 7: External engines: credential at registration and on rotation

**Files:**
- Modify: `crates/egret-engine-mgmt/src/external.rs` (registration ~765 region, `plugin_config_judgements`, DTOs), `crates/egret-engine-mgmt/src/engines.rs` (route registration)
- Test: `crates/egret-engine-mgmt/tests/routes.rs`

**Interfaces:**
- Consumes: `ranger_credential::{ensure, rotate}`, `PASSWORD_PLACEHOLDER`.
- Produces: registration response gains `rangerCredential: { username, password }` **exactly once**; `POST /engines/{id}/rotate-ranger-credential` on an external engine returns `200 { username, password }` (and demotes the old one; it stays valid until the operator confirms with `POST /engines/{id}/retire-previous-ranger-credential` `{ "installed": "<credentialId>" }` (the opaque id returned with the credential; a confirmation naming anything but the current credential is a 409 and retires nothing), since no apply can prove the operator re-installed it; a further rotation is refused with 409 while it is pending — the standing rule, see spec A10); `GET …/ranger-plugin-config` renders the files with `PASSWORD_PLACEHOLDER` and a warning that the real password was shown once.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn registering_an_external_engine_returns_its_credential_once() { /* body has rangerCredential.username == "engine-<id>"; a second GET of the engine DTO does not contain "password" */ }
#[tokio::test] async fn the_returned_credential_authenticates_against_the_verifier() { /* DbVerifier.verify(id, password) is Some */ }
#[tokio::test] async fn rotating_an_external_engines_credential_returns_a_new_one_and_keeps_the_old_valid() {}
#[tokio::test] async fn the_plugin_config_route_never_contains_the_password() {
    /* body does not contain the password; contains PASSWORD_PLACEHOLDER and "/ranger/service/plugins/policies/download/" */ }
```

- [ ] **Step 2: Run to confirm failure** — `cargo test -p egret-engine-mgmt --test routes external_credential` ⇒ FAIL.

- [ ] **Step 3: Implement**

Registration: after the row is inserted, `let cred = ranger_credential::ensure(&st, &engine).await?;` and add `ranger_credential: Option<PluginCredentialDto>` (`#[serde(skip_serializing_if = "Option::is_none")]`, `ToSchema`) to the registration response DTO only — **not** to `EngineDto`, so no list/get ever carries it. Rotation: extend the Task 6 handler: for an external engine call `ranger_credential::rotate` and return `Json(PluginCredentialDto)` with 200 (no apply, no `retire_previous`). Retirement of an external engine's previous credential is a separate, operator-confirmed route, `POST /engines/{id}/retire-previous-ranger-credential` (`engine:update`, external only, body `{ "installed": "<credentialId>" }`, idempotent 204; provisioned engines 409), which retires by the exact previous id read from the row with one compare-and-set on both columns (current = `installed`, previous = that id); a mismatch is a 409 and deletes nothing. Both password-carrying responses send `Cache-Control: no-store`.

`plugin_config_judgements`: rewrite the `auth_mode` block — the surface is now always authenticated unless egret runs `anonymous`:

```rust
let auth_mode = match f.surface_mode {
    RangerAuthMode::Required => {
        must_supply.push("The engine's Ranger credential (shown once at registration or rotation) in the security XML below, in place of the placeholder.".to_string());
        "engine-credential"
    }
    RangerAuthMode::Anonymous => {
        warnings.push("egret runs the Ranger surface in anonymous mode: anyone who can reach the URL can download every profile's policies.".to_string());
        "anonymous"
    }
};
```
Add `surface_mode` to `PluginConfigFacts` (fed from config via `CpState`, replacing `bearer_configured`), delete the "stock plugin cannot send a bearer" warning and the `bearer-configured` value, and update the DTO doc/OpenAPI enum text. Render the files with `Some(PluginCredentialRef { username, password: PASSWORD_PLACEHOLDER })`.

- [ ] **Step 4: Run, expect PASS; `npm run gen:api`**

- [ ] **Step 5: Commit**

```bash
git add -A crates frontend/src/api
git commit -m "feat(engines): external engines get a Ranger credential, shown once"
```

---

## Task 8: Surface authentication failures on Overview

**Files:**
- Create: `migrations/0201_ranger_auth_failures.sql` (renumber at creation)
- Modify: `crates/egret-policy/src/ranger/auth.rs` (record), `crates/egret-policy/src/plugin_downloads.rs` (store methods), the governance attention builder (find with `grep -rn "fn judge_plugins" crates` — Overview's plugin-drift signal), `frontend` Overview component only if the attention item needs a new label (find with `grep -rn "plugin" frontend/src/routes/governance`)
- Test: `crates/egret-policy/tests/ranger_auth_failures_db.rs` (new, DB-gated like `plugin_downloads_db.rs`)

**Interfaces:**
- Produces: `PolicyStore::record_auth_failure(engine_id: Uuid)`, `PolicyStore::auth_failures() -> Vec<AuthFailure { engine_id, count, last_at }>`; Overview's attention list includes an item `engine_auth_failing` per engine whose last failure is newer than its last successful download.

- [ ] **Step 1: Migration**

```sql
-- How often an engine presented a bad Ranger credential, and when last. One row
-- per engine whose username parsed as an engine id; an unparseable username is
-- logged only (no key to count under, and counting it would let anyone grow the table).
CREATE TABLE policy.ranger_auth_failures (
    engine_id uuid        PRIMARY KEY,
    count     bigint      NOT NULL DEFAULT 1,
    last_at   timestamptz NOT NULL DEFAULT now()
);
```

- [ ] **Step 2: Write the failing tests**

```rust
#[tokio::test] async fn a_failure_upserts_and_counts() { /* record twice ⇒ count 2, last_at advanced */ }
#[tokio::test] async fn an_unknown_engine_id_is_not_counted() {
    /* record_auth_failure only for engines that EXIST: the INSERT selects from engine_mgmt.engines, so a random uuid adds no row */ }
#[tokio::test] async fn overview_flags_an_engine_whose_failure_is_newer_than_its_last_download() {}
```
The second test is the guard against unbounded growth: an attacker can send any `engine-<uuid>`.

- [ ] **Step 3: Run to confirm failure.**

- [ ] **Step 4: Implement**

Store method — insert only for a real engine:
```rust
sqlx::query(
    "INSERT INTO policy.ranger_auth_failures (engine_id) \
     SELECT id FROM engine_mgmt.engines WHERE id = $1 \
     ON CONFLICT (engine_id) DO UPDATE SET count = policy.ranger_auth_failures.count + 1, last_at = now()",
).bind(engine_id).execute(&self.pool).await?;
```
In `require_surface_auth`, on the 401 path with a parseable `engine-<uuid>` username, call `record_auth_failure` best-effort (log and swallow errors — the 401 must still be returned). Also emit `tracing::warn!(engine_id = %id, "ranger credential rejected")` (never the password). Overview: add the `engine_auth_failing` item beside the plugin-drift item, built from `auth_failures()` joined against `ranger_plugin_downloads.at` for that engine's service; copy the item's wire shape and its frontend label from the drift item.

- [ ] **Step 5: Run, expect PASS; commit**

```bash
git add -A migrations crates frontend/src
git commit -m "feat(governance): show engines whose Ranger credential is being rejected"
```

---

## Task 9: Live check and documentation

**Files:**
- Modify: `docs/en/guides/engine-versions.md`, a new `docs/en/guides/ranger-surface.md`, `README`/helm docs for `ranger.authMode`
- Evidence: `docs/en/plans/2026-10-20-ranger-surface-auth-live-result.md`

- [ ] **Step 1: Live check on kind** (per `memory`: reset on a throwaway DB, `just reset-testdata` / `deploy-kind-governance`; do not use the shared dev stack). With a real Trino 483 and StarRocks 4.1.6 provisioned by egret, record observed output for each:
  1. both engines poll `/ranger/service/plugins/secure/policies/download/<profile>` with Basic and a policy change reaches them;
  2. `curl` the download URL without credentials ⇒ 401, with a wrong password ⇒ 401, with engine A's credential for engine B's profile ⇒ 403, `/service/...` at the root ⇒ 404;
  3. `kubectl get cm,secret` shows the password only in `{name}-ranger-secret`, nowhere in a ConfigMap;
  4. stop egret, restart an engine pod ⇒ it serves from the policy cache dir (record whether `/tmp/ranger/...` survives a pod restart — if it does not, **write that finding down; it is the open availability risk from the review**);
  5. `POST …/rotate-ranger-credential` ⇒ pods roll, polls continue throughout (no 401 in egret's log between old and new), afterwards the old password ⇒ 401;
  6. Overview shows `engine_auth_failing` after you deliberately break an external engine's password.
- [ ] **Step 2: Docs.** Write `ranger-surface.md` (URL, modes, credential lifecycle, external-engine installation, rotation, the floor), update `engine-versions.md`, and document `ranger.authMode` / `RANGER_AUTH_MODE`. **Ask the user** whether `egret-data/docs` (Platform section) is to be updated in this PR — the previous DG PRs deferred it by explicit decision; do not assume either way.
- [ ] **Step 3: Full local gate** (CI is out of quota; see `egret-run-full-ci-local-before-merge`): `cargo fmt --check`, clippy with `-D warnings`, `cargo test --workspace` (run alone, commit first; it can take the disk to a few GiB free), `just test-ranger-gates`, frontend `npm run lint && npx tsc --noEmit && npx vitest run && npm run build`, `npm run gen:api` with a clean `git diff`.
- [ ] **Step 4: Commit and report** each Step-1 observation (including the failures and surprises) in the live-result doc before opening the PR.

```bash
git add -A docs
git commit -m "docs(ranger): the authenticated surface, its lifecycle and the live result"
```

---

## Self-review

**Spec coverage:** A1 prefix → T3; A2 `/service` removed → T3 (test `the_old_root_service_prefix_is_gone`); A3 Basic per engine → T4/T5; A4 both paths → T3; A5 binding + tag service + crud 403 → T4; A6 Bearer kept → T4; A7 `required` default, `anonymous` opt-out → T4; A8 2.5.0 dropped → T2; A9 Secret not ConfigMap → T6; A10 rotation window → T5/T6; A11 external credential → T7; A12 failures logged/counted/Overview → T8 (the ledger-row half of A12 — recording a failed poll in `ranger_plugin_downloads` — is **not** implemented: that table's key is `(service_id, plugin_id, kind)` and a rejected request has no verified service, so a separate counter table is used instead; amend spec A12 accordingly when merging. **Done in Task 9:** spec A12 now describes `policy.ranger_auth_failures` with a per-engine `ok_at` instead of any join against the downloads ledger (which is keyed by profile, so a healthy engine on the same profile would hide a failing one), the per-engine coalescing (10 s failures, 60 s successes, lower-bound counts) and the 24 h window; spec §8 lists the known limits); A13 out of scope → nothing built. §5 verification → T1, T9; measurements → T1.

**Spec amendments this plan forces (already applied to the spec):** (1) the engine credential uses its own two columns because the existing internal-secret pair is Trino-only; (2) rotation keeps exactly one previous credential (not "old and new until the engine reports"); (3) A12 as above (applied in Task 9, with the implementation's per-engine `ok_at`, coalescing and 24 h window).

**Type consistency:** `EngineIdentity`, `EngineCredentialVerifier`, `plugin_username`, `engine_id_from_username` (T4) are the names used in T5/T7; `PluginCredential` (T5) vs `PluginCredentialRef` (T6, borrowed view) vs `PluginCredentialDto` (T7, wire); `ensure`/`rotate`/`retire_previous` (T5) are the names called in T6/T7; `ranger_surface`, `RANGER_SURFACE_PREFIX`, `plugin_base_url` (T3) used in T4/T6/T7.

**Placeholders to resolve at execution (named, not hidden):** T4 `Service`/`RangerError::forbidden()` are stand-ins for the file's real type/constructor (Step 3 says to read `get_service` and `ranger/error.rs`); T6's pod-template hash annotation depends on whether one exists today (Step 3 gives both branches); T8's Overview item copies the drift item's wire shape, whose location is given by grep.
