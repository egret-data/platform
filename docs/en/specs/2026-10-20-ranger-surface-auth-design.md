# Authenticating the Ranger surface (`/service`) and moving it under `/ranger`

Status: draft for review, 2026-10-20. Rulings 2026-10-20 (user): 2.5.0 dropped, floor is 2.8.0; `required` is the default; nothing is deployed to real users yet, so **no transition machinery** (no `/service` alias, no deprecation window, no anonymous carve-out). Amended same day: **plugin 2.5.0 is dropped from support (user ruling); the supported floor is 2.8.0.** Origin: DG production-readiness review.
Programme: Control Plane & DG (follows #4b Classification #702, independent of #5 Lineage).

## 1. Problem

The route `/service/plugins/policies/download/{service}` is what every engine's Ranger plugin polls.
Today it is served **anonymously by default**:

- `ranger.auth_token` empty ⇒ `require_bearer` lets everyone through
  (`crates/egret-policy/src/ranger/auth.rs:23`); config validation allows empty
  (`crates/egret-config/src/lib.rs:1763`).
- The plugin files egret generates carry **no credential**
  (`crates/egret-engine-mgmt-api/src/ranger_config.rs`, `security_xml`).
- `/service` is nested on the same server and port as `/api`
  (`crates/egret-server/src/lib.rs:289`), so anything that can reach the API reaches it.

A download returns every enabled policy of a profile, so an unauthenticated caller learns user names,
group names, role names, resource names, masks and row filters. Reads only: the `/service` write routes
were removed earlier, so nothing can be changed through it.

The name `/service` is also the Ranger Admin's own prefix and is easy to confuse with Kubernetes
services and with `/api`.

## 2. Measurements (2026-10-20)

Probe: a stub HTTP server that records the request line and `Authorization` header, driven by the real
`org.apache.ranger.admin.client.RangerAdminRESTClient` from each `ranger-plugins-common` jar, configured
the way `security_xml` configures a plugin, plus `ranger.plugin.<p>.policy.rest.client.username/.password`.
Source of the probe: to be committed as `tests/ranger/src/test/java/EgretPluginAuthTest.java` (plan Task 1);
the scratch copy is not part of the repo.

| `ranger-plugins-common` | Ships in | Without credentials | With `…rest.client.username/password` |
|---|---|---|---|
| 2.5.0 | Trino 470 | `GET /service/plugins/policies/download/<svc>`, no header | **identical request, no header — the properties are ignored** |
| 2.8.0 | Trino 483, StarRocks 4.1.4 | same, no header | `GET /service/plugins/secure/policies/download/<svc>` with `Authorization: Basic base64(user:pass)` |
| 2.9.0 | StarRocks 4.1.6 | not run | same as 2.8.0 (the `starrocks` prefix was measured on 2.8.0; 2.9.0 only with the `trino` prefix) |

Consequences, each of which contradicts a naive "just add a token":

1. **The plugin speaks Basic, not Bearer.** egret's current Bearer check can never be satisfied by a stock plugin.
2. **With credentials the plugin changes the PATH** to `/service/plugins/secure/policies/download/…`.
   egret serves only the non-`secure` path (`routes.rs:18`), so switching a 2.8+ engine to credentials without a
   new route would 404 and the engine would fall back to its cache, then deny-all.
3. **Plugin 2.5.0 (Trino 470) cannot authenticate at all** through this channel. Its client has `setBasicAuthInfo`
   but nothing wires it from config; the other mechanisms in 2.5.0 are Kerberos, client TLS certificates, or a custom
   `policy.source.impl` class. None is proportionate here.
4. Defaults today are Trino 483 and StarRocks 4.1.6 (`docs/en/guides/engine-versions.md`), so **only an engine that an
   operator pinned to Trino 470 is affected** by point 3. **Ruling (user, 2026-10-20): 2.5.0 / Trino 470 leaves the
   supported set; the floor is 2.8.0.** That removes the only engine that could not authenticate, so every supported
   engine can use Basic and no anonymous exception is needed (§3 A8, §4.6).

Not measured (plan Task 1 must measure before relying on them):
- whether a running plugin re-reads its security xml (assumed **no**, so a credential change needs a pod restart);
- what a 401 does to a plugin with a warm cache vs. a cold one (assumed: warm keeps serving the cache, cold denies all);
- ~~the roles / tags / userstore secure endpoints a plugin may call~~ **Measured 2026-10-20** (`../plans/2026-10-20-ranger-surface-auth-measurements.md`): in secure mode a 2.8.0 plugin also polls `GET /service/roles/secure/download/<svc>` every interval (no tags or userstore call seen). egret does not serve it and the plugin still enforces on the 404, so serving it stays out of scope; the only consequence is that the auth middleware must cover unmatched routes under `/ranger` (bad credentials ⇒ 401, not 404). Q1 (no XML re-read) and Q2 (warm cache survives a 401, cold cache denies all) agreed with the assumptions above.

## 3. Decisions

| # | Decision |
|---|---|
| A1 | The Ranger surface moves to **`/ranger`**, keeping Ranger's own sub-path: `/ranger/service/plugins/...`. The plugin appends `/service/...` to `policy.rest.url` itself, so the plugin URL becomes `<egret>/ranger` and **no plugin change is needed**. |
| A2 | **`/service` is removed outright**, not aliased. No engine in the field depends on it, so there is no transition: the only URL is `<egret>/ranger`. |
| A3 | Engines **authenticate with HTTP Basic**, one credential per engine: username `engine-<engine_id>`, password a random stored secret (same pattern as `egret-internal-{id}` / `egret-monitor-{id}`, with a Rotate action). |
| A4 | egret serves **both** `/ranger/service/plugins/policies/download/{svc}` and `/ranger/service/plugins/secure/policies/download/{svc}` with one handler. |
| A5 | **Authorization, not only authentication:** an engine credential may download **only the profile its engine is bound to** (and the tag service that profile names, which the handler already embeds). A compromised engine must not read another profile's policies. Mismatch ⇒ 403, not 404, and is logged. |
| A6 | Existing `ranger.auth_token` Bearer is **kept** for operator tooling and the harness (`tests/ranger/README.md` uses it). Accepted schemes: Bearer(token) **or** Basic(engine). |
| A7 | New config `ranger.auth_mode`: `required` (**default**) \| `anonymous`. In `required` an unauthenticated request gets 401. `anonymous` is an explicit opt-out for throwaway local stacks only; it logs a `warn!` at startup and on each anonymous download names the exposure. |
| A8 | **Decided (user): plugin 2.5.0 / Trino 470 is no longer supported; the floor is 2.8.0.** No internal listener, no anonymous carve-out. Details and the inventory of what changes are in §4.6. |
| A9 | The credential goes into a **Kubernetes Secret**, never a ConfigMap, and is mounted into the plugin config path. ❓ Task 1 must read how the provisioner mounts the plugin xml today; if it is a ConfigMap that is a prerequisite change (small, but it comes first). |
| A10 | **Rotation** follows the standing rule *never delete a secret value the cluster may still hold*. Each engine row keeps a **current** and one **previous** credential (`ranger_secret_id`, `ranger_secret_prev_id`); both verify. Rotating commits the new current and demotes the old one **before** the apply; for a **provisioned** engine the previous one is retired **only once the rollout of the current one is observed complete**, never when Kubernetes merely accepts the apply: every apply renders the plugin's security XML into a Secret **of that credential's own**, `{name}-ranger-{credential id}` (the id is not secret), which the pod templates mount, and stamps the id (`egret/ranger-credential`) on those pod templates and on the Secret — so a pod built from a template that names a credential can only ever mount that credential's content (`subPath` mounts keep what a pod saw at start; with one shared Secret, interleaved applies could leave new-template pods holding the old content). The rollout is complete only when that credential's Secret exists and every such workload (Trino: the coordinator and worker Deployments; StarRocks: the FE StatefulSet, the CN runs no plugin) has that id on its pod template, `observedGeneration == generation`, all replicas updated and ready with none of an older template left (Deployment: `updated == replicas == ready == available == spec.replicas`, nothing unavailable or terminating; StatefulSet: `currentRevision == updateRevision`, `updated == replicas == ready == spec.replicas`), and every running pod of those roles carries the id, is Ready and is not terminating. Anything absent, unknown, partial, scaled to zero or failing to read is NOT complete and keeps the previous credential. Then, and only then, the previous one is deleted by exact id in one compare-and-set on both columns (current = the id the rollout was observed for AND previous = the id read), so a rollout observed for an older credential never retires the previous credential of a newer rotation; its Kubernetes Secret `{name}-ranger-{previous id}` is deleted by exact name after that retire, together with the pre-per-credential `{name}-ranger-secret` (no pod mounts either once the rollout is complete). Engine delete and unbind delete every Ranger Secret of that engine, selected by its labels (`egret/engine={name}`, `egret/component=ranger-credential`), never by a name prefix. A periodic sweep (every 30 s, on every replica) and every read of the engine make this check. For **external** engines (where no apply can prove the operator re-installed it), only when the operator explicitly confirms WHICH credential is installed: `POST /engines/{id}/retire-previous-ranger-credential` with `{ "installed": "<credentialId>" }`, the opaque id returned with the credential at registration or rotation. The retire is one compare-and-set on both columns (current = `installed` AND previous = the id read from the row), so a stale or duplicated confirmation sent before a later rotation is a 409 and deletes nothing; with a matching `installed` and nothing pending it is an idempotent 204. Never a prefix sweep except at engine delete. A rotation is refused (409) while the previous credential is still pending retirement, for both kinds of engine, so a rotation never deletes a credential; the rollout-observed retire (provisioned) and the operator-confirmed retire (external) are the only deletes outside engine delete. While a provisioned engine's previous credential is pending, `rotate-ranger-credential` answers 409 saying it is waiting for the rollout to complete; the 204 of a rotation means only that Kubernetes accepted the apply, and the previous credential remains valid until the rollout completes. Plugins read the XML only at start (Task 1 re-measures this), so a rotation is a rolling restart of the engine pods. |
| A11 | **External engines** (registered, operator-run, SP4d-4 Part C) get the same credential, displayed once at registration/rotation. Their operator pastes it into their own plugin config. The registration (201) and rotation (200) bodies carry `{ username, password, credentialId }` with `Cache-Control: no-store`; no other response carries the password. The plugin-config page renders the files with the engine's username and a placeholder password; an engine registered before credentials existed is told to rotate to obtain one. Deleting the engine deletes its credentials. |
| A12 | Auth failures are **logged and counted** per engine (`policy.ranger_auth_failures`, only for engines that exist, so a caller cannot grow the table by inventing ids), and Overview shows an `engine_auth_failing` item when an engine's latest failure is newer than its latest successful download. (The earlier idea of recording the failed poll in `ranger_plugin_downloads` is dropped: that table is keyed by a verified service and a rejected request has none.) |
| A13 | Out of scope: mTLS, Kerberos, JWT (present in master `RangerRESTClient` but not measured in the shipped jars), per-profile credentials shared by several engines, serving roles/tags/userstore endpoints. |

## 4. Design

### 4.1 Routing (`egret-server`)
```text
/api/...                      unchanged
/ranger/service/...           ranger_router(state)         ← new canonical
/service/...                  removed (404)
```
`ranger_router` gains `/plugins/secure/policies/download/{service}` next to the existing route, same handler.

### 4.2 Auth middleware (`egret-policy/src/ranger/auth.rs`)
`require_bearer` becomes `require_surface_auth`, producing an `Authenticated` extension:
`Operator` (Bearer matched) or `Engine(engine_id)` (Basic matched against the stored per-engine secret, constant-time,
also compare against the previous secret during A10 overlap). The download handler resolves the requested
`{service}` to its profile and checks that an `Engine` principal is bound to it (A5).
`anonymous` mode keeps today's behaviour, with a startup `warn!` naming the exposure.

### 4.3 Storage
Secrets live in the existing secret store, named `engine-ranger-{engine_id}-{generation}`. The engine row references them with two new
columns (`ranger_secret_id`, `ranger_secret_prev_id`). The existing `internal_secret_id`/`monitor_secret_id` pair is **Trino-only**
(`rotate_engine_secrets` refuses StarRocks), so it cannot be reused. External engines may carry the columns (migration 0184's CHECK does not name them).
Migration number to be reserved at implementation time (`git ls-tree origin/main migrations/`; main tops out at 0199 today).

### 4.4 Provisioner (`egret-engine-mgmt-api/src/ranger_config.rs`)
`security_xml` takes a `url` that ends in `/ranger` and, when engine auth is on and the plugin version is ≥ 2.8.0,
adds `policy.rest.client.username/password` (from the Secret, A9) for every supported engine (all are ≥ 2.8.0 after §4.6).

### 4.5 Rollout
One cut, no phases: routes, middleware, provisioner URL and credentials ship together with `auth_mode = required`.
Any engine provisioned before this change (developer machines only) is re-applied once, or deleted and recreated; Overview
flags an engine whose plugin keeps polling the removed `/service` path or fails authentication (A12), which is how a stale
dev engine shows up.

### 4.6 Dropping plugin 2.5.0 (Trino 470)
Supported (type, plugin) pairs become **Trino 2.8.0, StarRocks 2.8.0, StarRocks 2.9.0**. Concretely:

| Where | Change |
|---|---|
| `crates/egret-perm/src/actions.rs:84` (`("trino","2.5.0")`) and the doc at `:107` | remove the pair from the modelled list |
| `crates/egret-perm/src/engine_images.rs:22,198` | drop the `trinodb/trino:470` → 2.5.0 mapping; tests at `:233`, `:311` move to 483 |
| `crates/egret-engine-mgmt/src/external.rs:55` | an external engine declaring a version below 2.8.0 is **refused at registration** (400), not merely "unmodelled", because it can never hold a credential |
| `crates/egret-engine-mgmt/tests/no_floating_images.rs`, `src/engines.rs:7279` | fixtures use 483 |
| `crates/egret-engine-mgmt/tests/routes.rs` (about 25 sites register an external Trino as `"2.5.0"`) and `registry_schema_constraints.rs` | fixture version becomes `"2.8.0"`; the test near line 11300 that asserts "2.5.0 modelled for Trino only" is rewritten, not deleted: 2.5.0 must now be **refused** |
| `tests/ranger/pom.xml` `<ranger.version>` default | `2.5.0` → `2.8.0`; the README section "Why 2.5.0 is the default" is replaced |
| `crates/egret-policy/tests/ranger_engine_gate.rs:40`, `justfile` (lines 147–201), the `egret-ranger-eval` differential, `crates/egret-perm-compile/tests/gate_timing.rs` | version lists become `2.8.0` and `2.9.0`. This **removes the 2.5.0 half of the differential corpus** (155,880 cases ran on both): CI gets shorter, and "0 disagreements" afterwards speaks for 2.8.0 and 2.9.0 only |
| `docs/en/guides/engine-versions.md` | the Trino 470 row goes; add a "dropped in <release>" note and the upgrade path |
| Historical text (migration 0179/0190 comments, old plans and specs, `ambiguous_arbitration.rs:118`) | **left as is**: append-only history, and several describe measurements of that jar that remain true |

Engines on Trino 470 / 2.5.0 (developer machines only, per the user): **disabled, not migrated.**
- Creating or updating an engine to an image whose plugin is below 2.8.0 is refused (400), and an external registration declaring such a version is refused too.
- A pre-existing row stays readable and is reported unsupported with the upgrade path (`PUT /engines/{id}` to Trino 483, or delete it); it will fail authentication
  and Overview will say so. No data migration, no compatibility path.

Not done here (optional follow-up): deleting the 2.5.0-specific branches inside `egret-ranger-eval` and the compiler that only
mirror that jar. They are dead for supported engines but harmless; removing them is a separate, riskier change and should not ride
with an auth change.

## 5. Verification plan
- **Task 1 (measure):** commit the probe as a surefire-listed test (the include list is an allow-list, see
  `pom.xml` and `EgretSurefireIncludesTest`), run on 2.8.0 and 2.9.0 with BOTH plugin prefixes (`trino`, `starrocks`);
  add the three unmeasured items from §2. Keep the 2.5.0 result in §2 as the reason for the floor, not as a CI case.
- **Task 2 (drop 2.5.0)** lands **before** the auth work, as its own PR, so that "all green on 2.8.0+" is established first.
- Rust tests: Basic ok / wrong password / wrong profile (403) / Bearer ok / anonymous in each mode / old-and-new secret
  during rotation / secure and non-secure paths serve an identical body / `/service` is 404 / `anonymous` mode warns / a 2.5.0 image is refused at create, update and registration.
- **Live check** on kind with a real Trino 483 and StarRocks 4.1.6: poll succeeds with credential, fails with a wrong one,
  cache survives an egret outage, restart after rotation picks up the new secret. Per the repo's own record, jsdom/unit
  green is not evidence here.
- Mutation: make the profile-binding check always true and confirm a test fails.

## 6. Risks
- A stale engine (pre-change, wrong URL or no credential) loses policy updates and, cold, denies all. Accepted: only developer machines have any, and Overview names them (A12).
- Basic over plain HTTP exposes the credential on the wire; document that `required` expects TLS or a cluster-internal path.
- Moving the prefix breaks anything that curls `/service/...`: `tests/ranger/README.md`'s fixture command and any dev script must switch to `/ranger/service/...` in the same PR.

## 7. Open questions
Settled by the user on 2026-10-20: `required` default (A7); 2.5.0 dropped (A8); no alias and no deprecation window (A2, §4.5, §4.6).
Remaining: Vietnamese mirror — written with the plan unless you say otherwise (previous DG specs were mirrored late).
