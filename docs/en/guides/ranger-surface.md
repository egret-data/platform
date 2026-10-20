# The Ranger surface: URL, authentication and engine credentials

egret is the Ranger Admin for its engines. There is no separate Ranger server. Every Trino and StarRocks engine runs a stock Ranger plugin, and that plugin polls egret for the policies of the profile it is bound to. This guide covers the URL the plugins poll, who may call it, the per-engine credential each plugin presents, and how to rotate that credential, monitor it and upgrade to it.

Design: [`docs/en/specs/2026-10-20-ranger-surface-auth-design.md`](../specs/2026-10-20-ranger-surface-auth-design.md). Measurements of the real plugin jars: [`docs/en/plans/2026-10-20-ranger-surface-auth-measurements.md`](../plans/2026-10-20-ranger-surface-auth-measurements.md). Engine images and plugin versions: [engine-versions.md](engine-versions.md).

## URL layout

The surface is served under `/ranger`, on the same server and port as `/api`. A plugin's `policy.rest.url` is `<egret>/ranger`. The plugin appends Ranger's own `/service/...` paths itself, so the plugin needs no change.

| Path | Who calls it | Served |
|---|---|---|
| `GET /ranger/service/plugins/policies/download/{profile}` | a plugin configured without a username and password | yes |
| `GET /ranger/service/plugins/secure/policies/download/{profile}` | a 2.8.0+ plugin configured with a username and password (what egret's engines call) | yes, same handler and same body as the plain path |
| `GET /ranger/service/tags/download/{profile}` | Trino plugins, for the profile's tag service | yes |
| `GET /ranger/service/plugins/definitions`, `…/definitions/name/{name}`, `…/services`, `…/policies/{id}` | operator tooling only | yes, operator bearer only |
| `GET /ranger/service/roles/secure/download/{profile}` | a 2.8.0+ plugin in secure mode, every poll interval | **no**: 404 to an authenticated caller, 401 to anyone else |
| `/service/...` at the server root | the old URL | **removed**: 404, with no alias |

The plugin tolerates the 404 on the roles route and keeps enforcing (measured with the 2.8.0 jar, measurements doc, Q3). egret does not serve Ranger roles.

Nothing is served under `/ranger` when Ranger is off (`[ranger] enabled = false`, Helm `ranger.enabled`).

## Who may call it

Every request under `/ranger/service/` goes through one authentication step, whether egret serves the path or not.

| Caller | Header | May read |
|---|---|---|
| Engine | `Authorization: Basic base64(engine-<engine uuid>:<password>)` | the profile its engine is bound to, and the tag service that profile names. Anything else is 403 and is logged. An engine bound to no profile gets 403 everywhere. The operator routes are 403. |
| Operator | `Authorization: Bearer <ranger.auth_token>` | everything. An empty `auth_token` (the default) accepts no bearer. |
| Anonymous | none | only in `anonymous` mode, see below |

The operator token is the config key `[ranger] auth_token`, env `EGRET_RANGER__AUTH_TOKEN`. The Helm chart renders `EGRET_RANGER__AUTH_TOKEN` as an empty string, so a Helm install accepts no operator bearer unless you override that variable.

A download for a profile name that does not exist is a 404 for every caller that got past authentication.

## Authentication modes

| Setting | Where |
|---|---|
| `[ranger] auth_mode = "required"` or `"anonymous"` | egret config file |
| `EGRET_RANGER__AUTH_MODE=required` or `anonymous` | environment |
| `ranger.authMode: required` or `anonymous` | Helm values (renders `EGRET_RANGER__AUTH_MODE`) |

- **`required` (the default).** A request without a valid engine credential or operator bearer gets 401 with `WWW-Authenticate: Basic realm="egret-ranger"`. Every cause (no header, malformed header, unknown engine, wrong password, wrong bearer) gets the same body, so a refusal does not say which part was wrong.
- **`anonymous`.** A request without a credential, or with a wrong one, is served as anonymous and may read every profile and the operator routes. A valid engine credential is still held to its own profile, so an engine that presents its credential gets 403 on other profiles while a caller with no credential does not. egret logs one `warn!` at startup. It does not log each anonymous request.

Any other value is a configuration error, and egret does not start.

> **Warning.** In `anonymous` mode, anyone who can reach the URL can download every profile's policies: user, group and role names, resource names, masks and row filters. Use it only for a throwaway local stack, never for an egret that a network you do not control can reach.

The engine credential travels as HTTP Basic. Over plain HTTP anyone on the path can read it. `required` assumes TLS, or a path that stays inside the cluster.

## The engine credential

Each engine has its own credential:

- **Username:** `engine-<engine uuid>`.
- **Password:** 32 random bytes, URL-safe base64 (43 characters). A password never contains `<`, `&` or `--`, so it goes into the plugin XML unescaped.
- **Storage:** a secret in egret's secret store (global scope), named `engine-ranger-{engine uuid}-{random}`. The engine row names a **current** and at most one **previous** credential. Both authenticate.

### Provisioned engines

egret creates the credential on the first apply of an engine that is bound to a policy profile. Every apply checks that the engine has one. The password is never returned by the API.

The credential reaches the pods through a Kubernetes Secret, never a ConfigMap:

- One Secret per credential, named `{engine name}-rc-{tag}`. The tag is the first 6 letters and digits of the credential's id (the id is not secret). The Secret is labelled `egret/engine={engine name}` and `egret/component=ranger-credential`, and annotated `egret/ranger-credential=<credential id>`.
- It holds the plugin's security XML (`ranger-trino-security.xml` or `ranger-starrocks-security.xml`) with `policy.rest.url`, the username and the password.
- Trino mounts it in the coordinator and the workers at `/etc/trino/ranger-trino-security.xml`. StarRocks mounts it in the FE conf directory through the cluster's `starRocksFeSpec.secrets`. The StarRocks audit XML stays in the `{engine name}-ranger-cm` ConfigMap. The StarRocks CN runs no plugin.
- The pod templates carry the same `egret/ranger-credential` annotation. A new credential changes the Secret name and the pod templates, so the pods roll.

A StarRocks engine name is limited to 48 characters. The operator names each FE volume after the object it mounts plus a 4-character hash, and a volume name is at most 63 characters. A longer name is refused with 400 at create and on every apply of a bound StarRocks engine.

### External engines

An external (registered, operator-run) engine gets its credential at registration. egret never connects to its cluster, so you install the credential yourself.

- `POST /api/control-plane/engines/external` answers 201 with the engine and `rangerCredential: { username, password, credentialId }`, with `Cache-Control: no-store`. **This is the only time the password is shown.** The register page shows it once in a dialog with copy buttons, and the dialog cannot be closed until you confirm you saved the password and the credential id.
- `credentialId` names this credential. You send it back when you retire the previous credential after a rotation.
- No other response carries the password. `GET /api/control-plane/engines/{id}` shows `rangerCredentialId` (the current credential's id) and `rangerCredentialPending` (whether a previous credential still authenticates).
- The Plugin config tab (`GET /api/control-plane/engines/{id}/ranger-plugin-config`) renders the plugin files with the engine's username and the placeholder `REPLACE_WITH_THE_ENGINE_PASSWORD`. Its `authMode` is `engine-credential`, or `anonymous` when egret runs that mode.
- An engine registered before credentials existed has none. Its Plugin config tab says so. Rotate once to get its first credential (see below). There is nothing to retire afterwards.
- Deleting the engine deletes its credentials.

### Installing a credential by hand

Put the username and password in the plugin's security XML, next to `policy.rest.url`. For Trino the file is `ranger-trino-security.xml` and the prefix is `ranger.plugin.trino`. For StarRocks they are `ranger-starrocks-security.xml` and `ranger.plugin.starrocks`.

```xml
<property><name>ranger.plugin.trino.policy.rest.url</name><value>https://egret.example.com/ranger</value></property>
<property><name>ranger.plugin.trino.policy.rest.client.username</name><value>engine-0b8f6c1e-…</value></property>
<property><name>ranger.plugin.trino.policy.rest.client.password</name><value>the password egret showed once</value></property>
```

Then restart the engine. A running plugin reads this file only at start (measured, Q1). Use the rest of the files from the Plugin config tab as they are.

The plugin must be 2.8.0 or later. Plugin 2.5.0 ignores these two properties and sends no `Authorization` header at all (measured). egret refuses such an engine (see [the version floor](#the-version-floor)).

To check the credential before you restart, call the path the plugin will call, and look at the content type and the body as well as the status:

```bash
curl -s -o ranger-check.json -w '%{http_code} %{content_type}\n' \
  -u 'engine-<engine uuid>:<password>' \
  'https://egret.example.com/ranger/service/plugins/secure/policies/download/<profile>'
grep -o '"serviceName":"[^"]*"' ranger-check.json
```

`200 application/json` followed by `"serviceName":"<profile>"` means the credential and the binding are right. A `200` with `text/html` is not egret: a proxy or ingress in front of egret sends `/ranger` to the web console, and it answers 200 to any credential. Route `/ranger` to egret's backend. The Helm chart's ingress and the console's nginx (`frontend/nginx-prod.conf`) do this from this release on; a proxy of your own needs the same rule. See [troubleshooting](#troubleshooting) for the other codes. Delete `ranger-check.json` afterwards: it holds the profile's policies.

## Rotation and retirement

The standing rule: egret never deletes a credential the engine may still hold. So a rotation keeps the old credential valid as the previous one, and the previous one is deleted only once it is known to be out of use. Until then a second rotation is refused (409), for both kinds of engine, so a rotation never pushes a credential out.

### Provisioned engines

1. Rotate: `POST /api/control-plane/engines/{id}/rotate-ranger-credential` (needs `engine:update`), or **Rotate credential** in the Ranger credential section of the engine's Overview tab.
2. `204` means only that Kubernetes accepted the apply. The pods then roll onto the new credential's Secret.
3. Both credentials authenticate until egret observes the rollout complete. Complete means: the new credential's Secret exists and names that credential; every plugin workload (Trino: the coordinator and worker Deployments; StarRocks: the FE StatefulSet) names the new credential on its pod template, has observed its latest generation, and has all replicas updated and ready with nothing of an older template left; and every running pod of those roles names the new credential, is Ready and is not terminating.
4. egret checks this every 30 s (on every egret replica) and, in the background, on every read of the engine (so that read's answer can still say pending; the next one will not). Once the rollout is complete, the previous credential is deleted by its exact id, and its Kubernetes Secret is deleted by exact name, only while that Secret still names the retired credential. The old shared `{engine name}-ranger-secret` from before per-credential Secrets is deleted at the same time.
5. Until then the engine page shows **Previous credential pending**, `rangerCredentialPending` is `true`, and a further rotation is refused with 409.

Anything egret cannot read, or reads as partial, counts as not complete and keeps the previous credential. This is fail-closed, and these cases stay pending for as long as they last:

- a rollout that stalls (a pod that never becomes Ready);
- a plugin workload scaled to zero, for example a Trino engine with no workers, or one autoscaled to zero (a known limit, see below);
- an engine unbound from its profile while a rotation is pending;
- a probe that keeps failing.

While pending, the previous credential stays valid longer than you may want, and you cannot rotate again.

Other answers from the rotate route:

| Answer | Meaning |
|---|---|
| 409, "rotation is pending" | the previous credential's rollout has not been observed complete; retry once it has |
| 409, "the engine's ranger credential changed while the request was being built" | a concurrent change; retry |
| 400 | the engine is bound to no policy profile (it has no credential), Kubernetes is not configured, or a StarRocks engine name is over 48 characters |
| 502 | Kubernetes refused the apply. The row now names the new credential and keeps the previous one, and both authenticate until a later apply of the new one has rolled out |

### External engines

1. Rotate: `POST /api/control-plane/engines/{id}/rotate-ranger-credential`, or **Rotate credential** on the engine's Overview tab. The answer is 200 with `{ username, password, credentialId }` and `Cache-Control: no-store`, shown once, in the same dialog as at registration.
2. Install the new password on the engine (above) and restart it. The previous credential keeps authenticating meanwhile.
3. Confirm which credential is now installed: **Retire previous credential**, or

   ```bash
   curl -s -X POST \
     -H 'Content-Type: application/json' \
     -d '{"installed": "<credentialId of the new credential>"}' \
     'https://egret.example.com/api/control-plane/engines/<id>/retire-previous-ranger-credential'
   ```

   (with your egret session or API credentials, and `engine:update`). `204`: the previous credential is deleted and no longer authenticates. The same request again is also `204`.
4. Rotation is allowed again.

The retire is a compare-and-set: it deletes the previous credential only if `installed` is still the current credential. A stale or repeated confirmation sent before a later rotation gets 409 and deletes nothing. A provisioned engine's retire is refused with 409, because egret retires those itself.

The UI keeps the rotation's `credentialId` in page state (never in storage or the URL). After a reload it asks you to type the id you saved. Do not copy the current id from the engine page unless that is the credential the engine really runs.

**If you lose the new password before installing it, you are stuck in this release.** Rotation is refused while the previous credential is pending, and the only way out is to retire, which deletes the credential the engine still runs. To recover: retire with the new `credentialId`, rotate again, install that credential and retire again. From the first retire until the engine restarts with the newest credential, its plugin gets 401: it keeps enforcing its cached policies, receives no changes, and a restarted pod with no cache denies everything. There is no route to discard a pending credential yet.

## The version floor

egret supports Ranger plugin 2.8.0 and later. The supported (engine, plugin) pairs are Trino 2.8.0, StarRocks 2.8.0 and StarRocks 2.9.0. New engines default to Trino 483 (plugin 2.8.0) and StarRocks 4.1.6 (plugin 2.9.0).

Plugin 2.5.0, which `trinodb/trino:470` ships, cannot send a credential, so it is no longer supported:

- creating an engine on `trinodb/trino:470`, or updating one onto it, is refused with 400;
- registering an external engine that declares a plugin older than 2.8.0 is refused with 400 ("Ranger plugin 2.5.0 is not supported: egret requires 2.8.0 or later");
- an existing Trino 470 engine stays readable but fails authentication. Upgrade it to Trino 483 or delete it.

Details and the upgrade path: [engine-versions.md](engine-versions.md).

## Monitoring authentication failures

When an engine presents its username (`engine-<uuid>`) with a wrong password, egret counts the rejection for that engine in `policy.ranger_auth_failures` and logs `warn!` "ranger credential rejected" with the engine id and the number of rejections. Neither the password nor the header is logged.

The Governance **Overview** then shows the attention item `engine_auth_failing` (labelled "engine Ranger credentials", critical, visible with `engine:read`). It names each engine whose latest rejection is newer than its latest successful download and less than 24 hours old, with the number of rejections and the time of the last one, and links to the engine.

- **It clears by itself.** The next successful download by that engine records `ok_at`, and the item goes away. Successful downloads are recorded at most once a minute per engine, so the item can stay for about a minute plus one poll interval after the engine is fixed. A rejection older than 24 hours is no longer shown.
- **The counts are lower bounds.** Rejections are coalesced per engine per 10 s and per egret replica; those still held back when a burst stops are not written, and writes are skipped when 8 are already in flight.
- **Only existing engines are counted.** A username that names no engine adds nothing, so a caller cannot grow the table by inventing ids.
- **Not counted:** a request with no credential, a username that is not `engine-<uuid>`, and a wrong operator bearer. An engine whose plugin has no credential configured at all (a pre-change engine, or a hand-written XML without one) is therefore not named by this item. A provisioned one shows up in the "plugin liveness" items instead, `plugin_quiet` after 5 minutes and `plugin_silent` after 24 hours, but only when no other plugin polls the same profile. An external engine is not judged by those items.
- **In `anonymous` mode** a wrong engine credential is still counted, even though the request is then served anonymously, and that anonymous download does not clear the item.
- **The item can be forged.** Anyone who can reach egret and knows an engine's id (engine ids are visible to everyone with `engine:read`) can send a wrong password for it and raise the item. The engine's next real download clears it, so it flaps. It does not affect the engine.
- **A flood can hide a real failure briefly.** egret tracks at most 4096 engine ids per 10 s window in memory. A caller flooding with random ids can fill it, and a real engine's first rejection is then not recorded until the window passes.

## When egret cannot be reached, or refuses a plugin

What the 2.8.0 plugin does, measured against a stub admin with the real jar (measurements doc, Q1 and Q2):

- **A running plugin with policies in memory keeps enforcing them across 401s**, and retries every poll interval. It receives no changes until a download succeeds again.
- **A plugin that starts with no cached policies and gets 401 denies everything**, and keeps retrying.
- **A plugin that starts with a cache file on disk behaves like a running one**: it enforces the cached policies.
- A running plugin does not re-read its security XML. A credential change needs a restart.

Measured on kind with Trino 483 and StarRocks 4.1.6 pods against egret itself ([live result](../plans/2026-10-20-ranger-surface-auth-live-result.md), scenario 4):

- **A running engine keeps enforcing from memory while egret is down.** A StarRocks FE with egret scaled to 0 for more than one poll interval still answered queries by its policies.
- **The policy cache does not survive a pod restart.** The cache directory is `/tmp/ranger/<profile>/policycache` inside the container, on the container's own filesystem, and egret mounts no volume there. A new pod (a reschedule, a node drain, an OOM kill, a rotation's rollout, a deleted pod) starts with no cache.
- **A restarted StarRocks FE denies everything while egret is down**, including to the holders of the `egret-admin` policies: a query got `Access denied; you need (at least one of) the SELECT privilege(s)` and `SHOW DATABASES` returned no rows. The plugin logged `cache file does not exist or not readable`. Access came back 16 s after egret did. (With egret down, an Iceberg catalog that egret serves cannot be used at all either, whatever Ranger decides.)
- **A Trino engine with JWT authentication (the default) does not start at all while egret is down.** Its coordinator reads egret's JWKS at start (`Error reading JWK keys from http://egret…/.well-known/jwks.json`) and goes to `CrashLoopBackOff`; it starts once egret is back. Workers that were not restarted keep their cache.
- A rotation itself does not hit this: the new pods start with a credential that already authenticates, while egret is up.

So any pod restart while egret is unreachable takes that engine to deny-all (StarRocks) or down (JWT Trino) until egret returns. **Open, not decided:** mount an `emptyDir` at the cache directory (survives a container restart, not a pod deletion or a reschedule), mount a small PVC there (survives both, at the cost of a volume per pod), or accept the risk as documented here.
## Troubleshooting

| Code | On | Meaning and fix |
|---|---|---|
| 401 | `/ranger/service/...` | No credential, a malformed one, an unknown engine, a wrong or retired password, a deleted engine, or a wrong operator bearer. The body is the same for all of them. Check the plugin's `policy.rest.client.username` and `.password`, and that the plugin is 2.8.0 or later (2.5.0 sends no header). An engine provisioned before this change has no credential until its next apply (see [upgrade notes](#upgrade-notes)). |
| 403 | a download | The engine credential is valid, but the profile it asked for is not the one its engine is bound to (nor that profile's tag service), or the engine is bound to no profile. Fix the plugin's `service.name`, or bind the engine. egret logs the engine id and the requested name. |
| 403 | `/ranger/service/plugins/definitions`, `…/services`, `…/policies/{id}` | An engine credential on an operator route. Use the operator bearer. |
| 404 | `/service/...` at the root | The old URL. Set `policy.rest.url` to `<egret>/ranger`. |
| 404 | `/ranger/service/roles/secure/download/...` | Expected for an authenticated plugin; egret does not serve roles and the plugin carries on. |
| 404 | a download, after authentication | No profile has that name. |
| 404 | anything under `/ranger` | Ranger is off on this egret (`ranger.enabled`). |
| 200 `text/html` | anything under `/ranger` | Not egret: a proxy or ingress sends `/ranger` to the web console, which answers 200 to every request. Route the `/ranger` prefix to egret's backend (the Helm ingress and the console's nginx do from this release on). |
| 500 | `/ranger/service/...` | egret could not check the credential (database or secret store). The log says "ranger: engine credential check failed" with the engine id. |

Control-plane answers:

| Code | Route | Meaning |
|---|---|---|
| 409 | rotate | A previous credential is still pending: wait for the rollout (provisioned) or retire it (external). |
| 409 | retire | `installed` is not the current credential, or the engine is provisioned. |
| 409 | any apply | The engine's credential changed while the request was built; retry. Or the stored credential cannot be rendered (only credentials from development builds before the renderer's check); rotate it. |
| 400 | create or update | Trino 470 or a declared plugin below 2.8.0, or a StarRocks engine name over 48 characters. |

## Known limits

- **No per-engine apply lock.** egret re-reads the engine right before every apply and refuses (409) a request that renders a credential that is no longer current. Between that re-read and the Kubernetes apply, a concurrent rotation and its retire can still land, and a stale apply can then put a retired credential back on the cluster. The engine gets 401 until its next apply.
- **A plugin workload scaled to zero blocks retirement.** Its rollout can never be observed complete, so the previous credential stays valid and rotation stays refused until it scales up again.
- **An external operator who loses a new password before installing it is stuck** (see above). There is no route to discard a pending credential.
- **The failure coalescer has no negative cache** for unknown engine ids, so a flood of random ids can hide a real engine's first rejection for up to 10 s.
- **Any user with `engine:read` sees each engine's current `credentialId`.** It grants nothing on its own: retiring needs `engine:update`, and the UI never sends that id by itself.
- **A warm plugin keeps enforcing its cached policies across 401s, but a cold one denies everything,** and the cache does not survive a pod restart. A pod restarted while egret is down denies everything (StarRocks) or does not start (JWT Trino) until egret returns. See [above](#when-egret-cannot-be-reached-or-refuses-a-plugin).

## Upgrade notes

This is a breaking change, with no transition period.

**The URL moved.** `/service/...` at the server root is gone, with no alias; it answers 404. A plugin's `policy.rest.url` must be `<egret>/ranger`. egret re-renders it for provisioned engines on their next apply. For an external engine or a hand-written XML, change it yourself. Scripts that curl `/service/...` move to `/ranger/service/...`.

**Authentication is on by default.** `ranger.auth_mode` defaults to `required`, and nothing before this release presented a credential. On upgrade every plugin gets 401 until it has one. A running plugin keeps enforcing its cached policies and receives no changes; a pod that starts with no cache denies everything. So:

- **Provisioned engines** get their credential on their next apply, and that apply rolls their pods. Apply each bound engine once: save its settings (`PUT /api/control-plane/engines/{id}`), or call `POST /api/control-plane/engines/{id}/rotate-ranger-credential`. **Restart** does not re-render the configuration and is not enough.
- **Engines of the Docker provisioner** (`engines.provisioner: docker`, the demo stack, Trino only) are provisioned engines too: an apply removes the engine's containers and creates new ones whose security XML carries the credential, written `0600` inside the container. Each container is labelled `egret/ranger-credential=<credential id>` (the id, never the password). A rotation's rollout is observed complete when every container of the engine carries the new id and is running (and not unhealthy); a stopped or exited container keeps it pending. There is no Kubernetes Secret, so nothing else is deleted at retirement. A Docker update has no rolling replacement: the engine is briefly down while its containers are recreated.
- **A bound StarRocks engine whose name is over 48 characters** gets 400 on every apply from this release on (its operator-hashed volume names would not fit a DNS label), so it cannot get a credential. Delete it and add it again under a shorter name.
- **External engines** registered before this release have no credential. Rotate once to get one, install it and restart the engine. There is nothing to retire.
- **The dev-compose StarRocks** (`deploy/docker-compose.dev.yml`) mounts a hand-written XML with no credential and gets 401. Either register it as an external engine bound to `sr1` and add the two properties with the credential to `deploy/starrocks/ranger/ranger-starrocks-security.xml`, or run the host egret with `EGRET_RANGER__AUTH_MODE=anonymous` (local development only).
- `just deploy-kind-governance` no longer sets `ranger.authMode=anonymous`.
- Setting `ranger.authMode=anonymous` while you migrate restores the old behaviour, with the exposure described above.

**Trino engines logged their Ranger password before this fix.** The Trino Ranger plugin printed every plugin property at INFO on start, the password included (`AUDIT PROPERTY: ranger.plugin.trino.policy.rest.client.password=…`, logger `org.apache.ranger.audit.provider.AuditProviderFactory`). That line was in the stdout of every coordinator and worker, so anyone who could read the pod logs (`kubectl logs`, or egret's pod-logs view with `engine:read`) could read the credential. egret now renders that logger at `WARN` in every bound Trino engine's `log.properties`, after your own log lines, so a level you set for it is replaced. The change is in the pod-template checksum, so each bound Trino engine rolls on its next apply. egret's pod-logs view and its live stream also mask `…password=`, `…secret=` style values. **Rotate the credential of every bound Trino engine that ran before this fix**: the old password may also be in log storage outside egret. A rotation is itself an apply, so it renders the new `log.properties` too, and the pods that start with the new credential do not log it. StarRocks did not log it.

**The plugin-config response changed.** `GET /api/control-plane/engines/{id}/ranger-plugin-config` reports `authMode` as `engine-credential` or `anonymous`. The value `bearer-configured` is gone. A client outside egret that matched on it must change.

**Unbinding deletes the engine's Ranger Secrets at once.** An apply with no policy profile deletes every `{name}-rc-{tag}` Secret (and the legacy `{name}-ranger-secret`) immediately, while pods of the old, bound template may still be running until the rollout replaces them. Those pods keep running, but a `kubectl rollout undo` back to a bound template fails to mount its Secret: bind the engine again instead. The credential's value in egret's secret store is not deleted by the unbind.

**Trino 470 engines** must move to Trino 483 first. See [engine-versions.md](engine-versions.md).
