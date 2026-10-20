# Authenticating the Ranger surface (`/service`) and moving it under `/ranger`

Status: implemented on `feat/ranger-surface-auth-b` (plan Tasks 3-8, 6b, 7b; Tasks 1-2 in PR A), 2026-10-20. §3 and §4 were amended after implementation so they describe what was built; §8 lists the known limits. The live check on kind is a separate step and is not recorded here. Operator guide: [`docs/en/guides/ranger-surface.md`](../guides/ranger-surface.md). Rulings 2026-10-20 (user): 2.5.0 dropped, floor is 2.8.0; `required` is the default; nothing is deployed to real users yet, so **no transition machinery** (no `/service` alias, no deprecation window, no anonymous carve-out). Amended same day: **plugin 2.5.0 is dropped from support (user ruling); the supported floor is 2.8.0.** Origin: DG production-readiness review.
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
Source of the probe: committed in plan Task 1 as `tests/ranger/src/test/java/EgretPluginAuthTest.java`;
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

Measured afterwards by plan Task 1 (`../plans/2026-10-20-ranger-surface-auth-measurements.md`, 2.8.0 jar against a stub admin; 2.9.0 sends the same request):
- a running plugin does **not** re-read its security xml (Q1), so a credential change needs a pod restart;
- a 401 leaves a plugin with policies in memory enforcing them, a plugin that starts with no cache denies all, and one that starts with a cache file on disk behaves like the warm one; it retries every poll interval (Q2). Not measured: the same on real engine pods, and whether the cache directory (`/tmp/ranger/<svc>/policycache`, no volume mounted) survives a pod restart; the live check records both;
- ~~the roles / tags / userstore secure endpoints a plugin may call~~ **Measured 2026-10-20** (`../plans/2026-10-20-ranger-surface-auth-measurements.md`): in secure mode a 2.8.0 plugin also polls `GET /service/roles/secure/download/<svc>` every interval (no tags or userstore call seen). egret does not serve it and the plugin still enforces on the 404, so serving it stays out of scope; the only consequence is that the auth middleware must cover unmatched routes under `/ranger` (bad credentials ⇒ 401, not 404). Q1 (no XML re-read) and Q2 (warm cache survives a 401, cold cache denies all) agreed with what the draft of this spec assumed.

## 3. Decisions

| # | Decision |
|---|---|
| A1 | The Ranger surface moves to **`/ranger`**, keeping Ranger's own sub-path: `/ranger/service/plugins/...`. The plugin appends `/service/...` to `policy.rest.url` itself, so the plugin URL becomes `<egret>/ranger` and **no plugin change is needed**. |
| A2 | **`/service` is removed outright**, not aliased. No engine in the field depends on it, so there is no transition: the only URL is `<egret>/ranger`. |
| A3 | Engines **authenticate with HTTP Basic**, one credential per engine: username `engine-<engine_id>`, password a random stored secret (same pattern as `egret-internal-{id}` / `egret-monitor-{id}`, with a Rotate action). As built: 32 random bytes in URL-safe base64 (43 characters, re-drawn while it contains `--`), stored in the global scope of the secret store as `engine-ranger-{engine_id}-{random}`; `POST /api/control-plane/engines/{id}/rotate-ranger-credential` (`engine:update`) rotates it. |
| A4 | egret serves **both** `/ranger/service/plugins/policies/download/{svc}` and `/ranger/service/plugins/secure/policies/download/{svc}` with one handler. |
| A5 | **Authorization, not only authentication:** an engine credential may download **only the profile its engine is bound to** (and the tag service that profile names, which the handler already embeds). A compromised engine must not read another profile's policies. Mismatch ⇒ 403, not 404, and is logged; an engine bound to no profile gets 403. A profile name that does not exist is a 404 before the binding is checked, so an authenticated engine can tell whether a name exists (accepted). |
| A6 | Existing `ranger.auth_token` Bearer is **kept** for operator tooling and the harness (`tests/ranger/README.md` uses it). Accepted schemes: Bearer(token) **or** Basic(engine). An empty token accepts no bearer. The read-only operator routes (`/plugins/definitions…`, `/plugins/services`, `/plugins/policies/{id}`) refuse an engine credential (403). |
| A7 | New config `ranger.auth_mode` (env `EGRET_RANGER__AUTH_MODE`, Helm `ranger.authMode`): `required` (**default**) \| `anonymous`; any other value fails config loading. In `required` a request without a valid credential gets 401 with `WWW-Authenticate: Basic realm="egret-ranger"` and one body for every cause. `anonymous` is an explicit opt-out for throwaway local stacks only: a request with no credential or a wrong one is served as anonymous (every profile, and the operator routes), while a valid engine credential is still held to its profile. It logs one `warn!` at startup naming the exposure; it does not log each anonymous request. |
| A8 | **Decided (user): plugin 2.5.0 / Trino 470 is no longer supported; the floor is 2.8.0.** No internal listener, no anonymous carve-out. Details and the inventory of what changes are in §4.6. |
| A9 | The credential goes into a **Kubernetes Secret**, never a ConfigMap, and is mounted into the plugin config path. The security XML moved out of the ConfigMaps it lived in (Trino `{name}-config`, StarRocks `{name}-ranger-cm`) into one Secret per credential (A10); Trino mounts it by `subPath` at `/etc/trino/ranger-trino-security.xml` in the coordinator and workers, StarRocks through `starRocksFeSpec.secrets` in the FE conf directory. The StarRocks audit XML stays in `{name}-ranger-cm`. |
| A10 | **Rotation** follows the standing rule *never delete a secret value the cluster may still hold*. Each engine row keeps a **current** and one **previous** credential (`ranger_secret_id`, `ranger_secret_prev_id`); both verify. Rotating commits the new current and demotes the old one **before** the apply; for a **provisioned** engine the previous one is retired **only once the rollout of the current one is observed complete**, never when Kubernetes merely accepts the apply: every apply renders the plugin's security XML into a Secret **of that credential's own**, `{name}-rc-{tag}`, the tag being the first 6 alphanumerics of the credential's (non-secret) id and kept distinct between an engine's current and previous credential (a colliding mint is re-drawn), which the pod templates mount, and stamps the id (`egret/ranger-credential`) on those pod templates and on the Secret — so a pod built from a template that names a credential can only ever mount that credential's content (`subPath` mounts keep what a pod saw at start; with one shared Secret, interleaved applies could leave new-template pods holding the old content). The rollout is complete only when that credential's Secret exists and every such workload (Trino: the coordinator and worker Deployments; StarRocks: the FE StatefulSet, the CN runs no plugin) has that id on its pod template, `observedGeneration == generation`, all replicas updated and ready with none of an older template left (Deployment: `updated == replicas == ready == available == spec.replicas`, nothing unavailable or terminating; StatefulSet: `currentRevision == updateRevision`, `updated == replicas == ready == spec.replicas`), and every running pod of those roles carries the id, is Ready and is not terminating. Anything absent, unknown, partial, scaled to zero or failing to read is NOT complete and keeps the previous credential. Then, and only then, the previous one is deleted by exact id in one compare-and-set on both columns (current = the id the rollout was observed for AND previous = the id read), so a rollout observed for an older credential never retires the previous credential of a newer rotation; its Kubernetes Secret `{name}-rc-{tag of the previous id}` is deleted by exact name after that retire, together with the pre-per-credential `{name}-ranger-secret` (no pod mounts either once the rollout is complete). Engine delete and unbind delete every Ranger Secret of that engine, selected by its labels (`egret/engine={name}`, `egret/component=ranger-credential`), never by a name prefix. The tag is short because the StarRocks operator (`--volume-name-with-hash=true`) names each FE pod volume `<object name>-<4 chars>` and a volume name is a 63-character DNS label: a StarRocks engine name is therefore limited to 48 characters (400 above it, at create and on every apply of a bound StarRocks engine). The retired credential's Secret is deleted only while its annotation still names that credential, with a `resourceVersion` precondition. Right before every apply egret re-reads the row and refuses (409) a request whose rendered credential is not the CURRENT one any more (a build only ever renders the current one); this narrows, but without a per-engine lock does not close, the race between concurrent applies. A periodic sweep (every 30 s, on every replica) and every read of the engine make this check. For **external** engines (where no apply can prove the operator re-installed it), only when the operator explicitly confirms WHICH credential is installed: `POST /engines/{id}/retire-previous-ranger-credential` with `{ "installed": "<credentialId>" }`, the opaque id returned with the credential at registration or rotation. The retire is one compare-and-set on both columns (current = `installed` AND previous = the id read from the row), so a stale or duplicated confirmation sent before a later rotation is a 409 and deletes nothing; with a matching `installed` and nothing pending it is an idempotent 204. Never a prefix sweep except at engine delete. A rotation is refused (409) while the previous credential is still pending retirement, for both kinds of engine, so a rotation never deletes a credential; the rollout-observed retire (provisioned) and the operator-confirmed retire (external) are the only deletes outside engine delete. While a provisioned engine's previous credential is pending, `rotate-ranger-credential` answers 409 saying it is waiting for the rollout to complete; the 204 of a rotation means only that Kubernetes accepted the apply, and the previous credential remains valid until the rollout completes. Plugins read the XML only at start (measured, §2), so a rotation is a rolling restart of the engine pods. A plugin workload scaled to zero can never be observed rolled out, so it keeps the previous credential, and refuses rotation, until it scales up (§8). The rotate route answers 400 for a provisioned engine bound to no profile or when Kubernetes is not configured, and 502 when the cluster refuses the apply (both credentials then stay valid). |
| A11 | **External engines** (registered, operator-run, SP4d-4 Part C) get the same credential, displayed once at registration/rotation. Their operator pastes it into their own plugin config. The credential is minted before the registration transaction and written by the same INSERT, and discarded if that transaction fails. The registration 201 carries the engine plus `rangerCredential: { username, password, credentialId }`, and the external rotation 200 carries `{ username, password, credentialId }`, both with `Cache-Control: no-store`; no other response carries the password. `EngineDto` (every engine read) carries `rangerCredentialId` (the current credential's id, so anyone with `engine:read` sees it; it grants nothing) and `rangerCredentialPending`. The UI shows the password once in a dialog that cannot be dismissed until the operator confirms saving it, and keeps the rotation's `credentialId` in page state as the `installed` token for the retire. The plugin-config page renders the files with the engine's username and the placeholder `REPLACE_WITH_THE_ENGINE_PASSWORD`, and reports `authMode` as `engine-credential` or `anonymous` (the former `bearer-configured` value is gone: a breaking change for API clients); an engine registered before credentials existed is told to rotate to obtain one. A lost compare-and-set on an external rotation is a 409 (never the winner's password). Deleting the engine deletes its credentials (exact ids, then the `engine-ranger-{id}-` prefix). |
| A12 | Auth failures are **logged and counted** per engine in a separate table, `policy.ranger_auth_failures (engine_id, count, last_at, ok_at)` (migration 0201), whose upsert selects from `engine_mgmt.engines`, so only engines that exist get a row and a caller cannot grow the table by inventing ids; an engine-delete trigger removes the row. Only a refused Basic credential whose username parses as `engine-<uuid>` is counted (also in `anonymous` mode); no header, another username or a wrong bearer has no engine to count under. **Success is per engine, not a ledger join:** a download let through for an engine credential sets that engine's `ok_at` when it has a row. `ranger_plugin_downloads` cannot serve here, because it is keyed by profile and plugin id, so a healthy engine on the same profile would hide a failing one; nor can it record a failed poll, which has no verified service. **Coalescing:** recording is off the request path (never changes or delays the 401), per process, at most one failure upsert and one `warn!` per engine per 10 s (rejections held back when a burst stops are not written, and writes are skipped when 8 are in flight, so `count` is a lower bound), and at most one `ok_at` write per engine per 60 s; at most 4096 ids are tracked per window. Overview (Governance, critical, needs `engine:read`) shows `engine_auth_failing` for each engine whose `last_at` is newer than its `ok_at` (or has none) **and less than 24 h old**, so an engine that stopped presenting the bad credential drops off. An engine with no credential configured is not named by this item; a provisioned one shows in the existing drift items (`plugin_quiet` after 5 min, `plugin_silent` after 24 h) when no other plugin polls its profile, and an external one is not judged. |
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
`require_bearer` becomes `require_surface_auth`, producing a `Caller` extension:
`Operator` (Bearer matched), `Engine(EngineIdentity)` (Basic matched by the injected verifier against the engine's current
and previous stored secret, constant-time, both always compared), or `Anonymous` (anonymous mode only). The layer wraps every path
under `/ranger/service/`, including a catch-all route that answers 404 to an authenticated caller, so an unserved path
(the plugin's roles poll) is 401 without credentials. The download handlers resolve the requested `{service}` and check
that an `Engine` caller is bound to it or to the tag service its profile names (A5); `require_operator` refuses an engine on
the operator routes. `anonymous` mode serves callers without a valid credential, with a startup `warn!` naming the exposure.

### 4.3 Storage
Secrets live in the existing secret store, named `engine-ranger-{engine_id}-{generation}`. The engine row references them with two new
columns (`ranger_secret_id`, `ranger_secret_prev_id`). The existing `internal_secret_id`/`monitor_secret_id` pair is **Trino-only**
(`rotate_engine_secrets` refuses StarRocks), so it cannot be reused. External engines may carry the columns (migration 0184's CHECK does not name them).
Migrations: `0200_engine_ranger_credential.sql` (the two columns, plus a CHECK that a previous credential exists only next to a current one and differs from it) and `0201_ranger_auth_failures.sql` (A12).

### 4.4 Provisioner (`egret-engine-mgmt-api/src/ranger_config.rs`)
`security_xml` takes egret's URL and renders `policy.rest.url` as `<url>/ranger`. When the binding carries a credential it
adds `ranger.plugin.<trino|starrocks>.policy.rest.client.username/password`, unescaped; a credential can only be built through a
checked constructor that refuses `<`, `&` and `--`, and minted passwords are re-drawn until they contain no `--`. Every apply of a
bound provisioned engine carries one (it is ensured on every build, whatever `auth_mode` is), so the first apply after the upgrade
mints a credential and rolls the pods. The result goes into the per-credential Secret of A9/A10.

### 4.5 Rollout
One cut, no phases: routes, middleware, provisioner URL and credentials ship together with `auth_mode = required`.
Any engine provisioned before this change (developer machines only) is re-applied once (an update or a credential rotation; a
restart does not re-render), or deleted and recreated. A stale dev engine does not show up as `engine_auth_failing`: it presents no
credential (or polls the removed `/service` path, which is a plain 404), so nothing is counted for it (A12). It shows up in the drift items
(`plugin_quiet`, then `plugin_silent`) when no other plugin polls its profile.

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
- A pre-existing row stays readable and is reported unsupported with the upgrade path (`PUT /engines/{id}` to Trino 483, or delete it); it fails authentication
  (it sends no credential, so `engine_auth_failing` does not name it; the drift items do when no other plugin polls its profile, A12). No data migration, no compatibility path.

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
- **Live check** (pending at the time of the §3/§4 amendment; recorded in its own live-result document) on kind with a real Trino 483 and StarRocks 4.1.6: poll succeeds with credential, fails with a wrong one,
  cache survives an egret outage, restart after rotation picks up the new secret. Per the repo's own record, jsdom/unit
  green is not evidence here.
- Mutation: make the profile-binding check always true and confirm a test fails.

## 6. Risks
- A stale engine (pre-change, wrong URL or no credential) loses policy updates and, cold, denies all. Accepted: only developer machines have any. Overview names them only through the drift items (`plugin_quiet`, `plugin_silent`), and only when no other plugin polls the profile; `engine_auth_failing` names only an engine that presents a wrong credential (A12).
- Basic over plain HTTP exposes the credential on the wire; document that `required` expects TLS or a cluster-internal path.
- Moving the prefix breaks anything that curls `/service/...`: `tests/ranger/README.md`'s fixture command and any dev script must switch to `/ranger/service/...` in the same PR.

## 7. Open questions
Settled by the user on 2026-10-20: `required` default (A7); 2.5.0 dropped (A8); no alias and no deprecation window (A2, §4.5, §4.6).
Vietnamese mirror: `docs/vi/specs/2026-10-20-ranger-surface-auth-design.md` (with the operator guide `docs/vi/guides/ranger-surface.md`); the long plan is not mirrored.
Still open with the user: zero-replica rule (§8), a discard-pending route (§8), and whether `egret-data/docs` is updated in this PR.

## 8. Known limits / follow-ups
- **No per-engine apply lock.** The re-read right before an apply refuses (409) a request rendering a credential that is no longer current, but a rotation and its retire can still land between that re-read and the Kubernetes apply; a stale apply can then put a retired credential back on the cluster, and the engine gets 401 until its next apply. Closing it needs per-engine apply serialisation.
- **Zero-replica workloads.** A plugin workload with `replicas: 0` (a Trino engine with no workers, KEDA `minReplicaCount: 0`) is never observed rolled out, so the previous credential stays valid and rotation stays refused until it scales up. A provably safe vacuous-rollout rule was proposed and awaits a user decision; until then this is fail-closed.
- **No discard-pending route.** An external operator who loses a new password before installing it can only retire (which deletes the credential the engine still runs) and rotate again; the engine gets 401 in between.
- **No negative cache in the A12 coalescer.** A flood of random `engine-<uuid>` usernames can fill the 4096-entry map, and a real engine's first rejection is then not recorded until the window passes (10 s).
- **`credentialId` visible with `engine:read`.** The current credential's id is on `EngineDto`. It grants nothing by itself (retire needs `engine:update`, and the UI never sends it on its own), so the exposure is vacuous, but it is wider than needed.
- **`engine_auth_failing` can be made to flap** by a credential-less caller who knows an engine id: a wrong password for that id raises it, and the engine's next real download clears it.
- **Warm vs. cold plugin.** A plugin keeps enforcing cached policies across 401s, but a cold start without a cache denies all (§2); whether the cache directory survives a pod restart is for the live check.
- **Bad current + pending previous (development databases only).** A row whose current credential cannot be rendered (minted before the renderer's check) and whose previous one is pending refuses every apply and every rotation; recovery is manual (`ranger_credential.rs` module doc).
