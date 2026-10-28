# Survey for roadmap package #4: HTTP and the unserved platform tasks

Worktree: `/Users/1pro-l4r8/Documents/github/data-portal/egret-http` (HEAD `1a72e827`, #3a merged).
Kestra: `/Users/1pro-l4r8/Documents/github/data-portal/kestra`.
Abbreviations: `KH` = `kestra/core/src/main/java/io/kestra/plugin/core/http/`, `KC` = `kestra/core/src/main/java/io/kestra/core/http/client/`, `EW` = `egret-http/crates/egret-workflow/`.
Read-only survey. Items marked **(unverified)** were inferred and not read in the code.

Roadmap anchors: row 4 at `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md:77`; task table rows at `:386-388`; the unserved rows at `:401`; "most used" at `:604`; `http()` stays refused at `:153`, `:544`, `:581`; `secret()` in http tasks is assigned to **#10** (`:83`, `:605`).

---

## 1. Kestra `http.Request`, `http.Download`, `http.SseRequest`

### 1.1 Shared properties (`AbstractHttp`, KH `AbstractHttp.java`)

All of these are `Property<>`, so all are rendered.

| Property | Type | Default | Notes |
|---|---|---|---|
| `uri` | String | required (`@NotNull`) `:45-46` | Rendered. Spaces become `%20` (`:75`). Nothing else is encoded. |
| `method` | String | `"GET"` `:48-49` | Free string, not an enum. |
| `body` | String | none `:51` | Used only when `formData` is empty (`:149`). Sent as a `StringRequestBody` with `contentType` and `options.defaultCharset` (`:150-156`). |
| `formData` | Map<String,Object> | none `:53` | When non-empty it wins over `body`. If `contentType == "multipart/form-data"`, it becomes multipart (`:104-141`): a string starting with `kestra://` is read from storage as a file part (`:113-120`); a `{name, content}` map is a file part with that filename (`:124-135`); any other value is JSON-serialised (`:137`). Otherwise it becomes `application/x-www-form-urlencoded` (`:143-147`). |
| `params` | Map<String,Object> | none `:55` | Added as query parameters. A List adds repeated keys. Any non-String or non-List value throws `"Unsupported param type: " + class` (`:79-96`). |
| `contentType` | String | `"application/json"` `:57-58` | With no body and no formData it is still sent as a `Content-Type` header (`:157-158`). |
| `headers` | Map<CharSequence,CharSequence> | none `:60` | Each value is rendered. Repeated keys are grouped (`:161-180`). |
| `options` | `HttpConfiguration` | `builder().build()` `:62-63` | See 1.2. |

### 1.2 `options` (`KC configurations/HttpConfiguration.java`)

| Property | Type | Default | Behaviour |
|---|---|---|---|
| `timeout.connectTimeout` | Duration | none (Apache default) | `TimeoutConfiguration.java:14-15`, applied at `HttpClient.java:135-141` |
| `timeout.readIdleTimeout` | Duration | **PT5M** `TimeoutConfiguration.java:18-19` | Socket timeout. There is **no total-request timeout**: only the task-level `timeout:` bounds it. |
| `proxy.type` | `java.net.Proxy.Type` | `DIRECT` `ProxyConfiguration.java:18-19` | |
| `proxy.address` / `port` / `username` / `password` | String/Integer | none | A missing port fails with `"A proxy port is required when a proxy address is set (options.proxy.port)."` (`HttpClient.java:148-149`). The proxy is **chosen by the flow author**. |
| `auth` | polymorphic on `type`: `BASIC` / `BEARER` / `DIGEST` | none | `AbstractAuthConfiguration.java:14-32`. BASIC sets `username`/`password` (`password` is `secret=true`) and adds a pre-emptive `Authorization: Basic …` header (`BasicAuthConfiguration.java:40-56`). BEARER sets `token` (`secret=true`) and adds `Authorization: Bearer …` (`BearerAuthConfiguration.java:33-48`). DIGEST is challenge-based and scoped to the request URI's host:port. Missing values fail with `"Digest authentication requires both \`username\` and \`password\`."` (`HttpClient.java:473-497`). **There is no API-key auth type**: an API key goes in `headers`. |
| `ssl.insecureTrustAllCertificates` | Boolean | none | **Kestra bug:** the check is `!= null` (`HttpClient.java:184-187`), so `false` also turns on trust-all with `NoopHostnameVerifier`. |
| `followRedirects` | Boolean | **true** `:35-37` | `disableRedirectHandling` when false (`HttpClient.java:196-198`). |
| `allowFailed` | Boolean | **false** `:39-42` | When true, a status of 400 or above does not throw. |
| `allowedResponseCodes` | List<Integer> | none `:44-46` | When set it **replaces** both rules: only the listed codes pass, **2xx included** (`HttpClient.java:514-524`). |
| `enabledTcpExtendedKeepAlive` | Boolean | true | Windows workaround. Not relevant to egret. |
| `defaultCharset` | Charset | UTF-8 | Used for the request body only. |
| `logs` | enum[] `REQUEST_HEADERS` / `REQUEST_BODY` / `RESPONSE_HEADERS` / `RESPONSE_BODY` | none | Logging interceptors (`HttpClient.java:113-127`). Logging headers would print `Authorization` (**unverified** whether it is masked). |

Client-wide behaviour (`KC HttpClient.java`):
- User-Agent is `Kestra` (`:94-95`). `Accept-Encoding: gzip, x-gzip, deflate` is forced, and gzip/deflate responses are decoded transparently (`:102`).
- **Status handling** is `throwIfResponseNotAllowed` (`:502-524`). The error text is `"Failed http request with response code '<code>'"`, plus `" and body:\n<whole body>"` when there is a body (`apache/HttpResponseFailure.java:18-24`). This is a `HttpClientResponseException`.
- Transport errors (`SocketException`, `SSLHandshakeException`) become `HttpClientRequestException(e.getMessage())` (`:534-539`). Other IO errors are wrapped in a `RuntimeException`.
- **Egress guard: only `kestra.tasks.http.allowed-list` and `kestra.tasks.http.denied-list`** (`validateUri`, `:549-566`). These are **string `startsWith` checks on the initial URI**: there is no parsing, no DNS resolution, no check on redirects, and no private-range default. The error texts are `"The URI <uri> is not in the configured allowed list (kestra.tasks.http.allowed-list)."` and `"... is in the configured denied list (kestra.tasks.http.denied-list)."`. **There is no SSRF protection by default.** A redirect, a DNS name that resolves privately, or `http://allowed.example@169.254.169.254/` all get past a prefix list. (Kestra's own doc says such a URI should "start with" an allowed prefix. The userinfo case was **not tested**.)
- **Retries:** none are configured explicitly. Apache HttpClient 5's builder default retry strategy applies (**unverified**: by default it retries once for idempotent requests on IO errors and on 429/503). Task-level `retry:` is the documented mechanism.
- The `RunContextResponseInterceptor` emits the metrics `response.length`, `request.count`, `request.bytes` and debug-logs the URI with the remote address (`apache/RunContextResponseInterceptor.java`).

### 1.3 `Request` (KH `Request.java`)

- Extra property: `encryptBody` (Boolean, default false, `:339-344`). When true, the output is `encryptedBody` (`EncryptedString`) and `body` is null (`:388-395`).
- Run (`:346-373`): reads the whole body as `Byte[]` **in memory**, then applies the cap `MAX_OUTPUT_BODY_BYTES = 19 MiB` (`:337`). Over the cap it fails with: `"Response body is too large to store in task outputs (<n> bytes > max <max> bytes). Use io.kestra.plugin.core.http.Download to fetch large payloads as files instead."` (`:377-382`). The cap is checked after the whole body has been read.
- The body is decoded as **UTF-8 always** (`:356`). If any code point is undefined it fails with `"Illegal unicode code point in request body: <cp>, the Request task only support valid Unicode strings as body.\nYou can try using the Download task instead."` (`:360-368`).
- **Doc drift:** the class description mentions `options.maxContentLength` (`:41`), but no such property exists in `HttpConfiguration`.
- **Outputs** (`:398-435`): `uri` (the **request** URI, not the URI after redirects, `:392`), `code` (Integer), `headers` (Map<String, List<String>>), `body` (String, **not parsed as JSON**), `encryptedBody`, and a `formData` output field that is declared but never set.
- Alias: `io.kestra.plugin.fs.http.Request` (`:333`).

### 1.4 `Download` (KH `Download.java`)

- Extra properties: `failOnEmptyResponse` (Boolean, default **true**, `:63-65`) and `saveAs` (String, `:67-73`).
- Streams the response to a temp file (`:79-120`). When the response is not content-encoded, it checks `Content-Length` against the bytes written and fails with `"Invalid size, got <n>, expected <len>"` (`:100-116`).
- An empty body with `failOnEmptyResponse` and without `options.allowFailed` fails with `HttpClientResponseException("No response from server")` (`:122-127`). Otherwise it logs a warning, `File '<uri>' is empty` (`:129`).
- **Filename:** `saveAs` first, else the `Content-Disposition` `filename`/`filename*` (`:160-189`, with the path cut at `File.separator` and spaces turned into `+`), else a random name keeping the URI's extension (`:191-204`). It is stored with `runContext.storage().putFile` (`:151`), which gives a `kestra:///…` URI.
- **There is no size cap on Download.**
- **Outputs** (`:206-232`): `uri` (internal storage URI), `code`, `length` (Long), `headers`. Metric: `response.length`. Alias: `io.kestra.plugin.fs.http.Download`.

### 1.5 `SseRequest` (KH `SseRequest.java`)

- Extra properties: `concatJqExpression` (String, `:114-120`, compiled as JQ 1.6) and `failedOnMissingJq` (Boolean, default true, `:122-129`).
- Reads the stream **until the server closes it** (`HttpClient.sseRequest`, `:320-335`, with a spec-faithful parser at `:337-419`). **No event cap and no total timeout:** only `readIdleTimeout` (5 min) and the task `timeout:` stop it. The example uses `maxEvents: 10` (`:66`), but **that property does not exist**.
- `sseRequest` **skips the status check entirely**: there is no `throwIfResponseNotAllowed` (`:320-335`), so a 500 with an SSE body "succeeds".
- Each event's `data` is parsed as JSON when possible, otherwise kept as a string (`:149-152`).
- JQ errors fail with `"Failed to resolve JQ expression '<q>' and value '<event>'"` (`:181`). A non-scalar result fails with `"Expected text value from JQ expression but got: <type>"` (`:177`).
- **Outputs:** `uri`, `events` (null when JQ was used), `size` (event count), `result` (the concatenated JQ results). Metric: `size`. No alias.

### 1.6 Not in scope but adjacent
`http.Trigger` (KH `Trigger.java`) belongs to roadmap #11. The `http()` Pebble function stays refused in egret (roadmap `:153`).

---

## 2. Oracle corpus coverage

- `EW oracle-exec/corpus/` has **zero** flows with an `http.Request`, `Download` or `SseRequest` task. A grep for `plugin.core.http` or `fs.http` in `corpus/` matches only `valids/http-listen.yaml` and `valids/http-listen-encrypted.yaml`. Both use **`io.kestra.plugin.fs.http.Trigger`** (to `https://api.chucknorris.io/jokes/random`) with a single `Log` task.
- Golden for both: `verdict: runs`, `execution: [CREATED, RUNNING, SUCCESS]`, run `hello` only. `corpus.json` has `disposition: runs`, `reason: SUCCESS`, `taskruns: 1`. The golden records the execution the harvester started. It does **not** depend on the trigger's network call: the task is a `Log` with no reference to `trigger.*`. Whether the harvest box had network access when the trigger polled is **unverified** and does not matter.
- Neither entry appears in `baseline.txt`, and no http class appears in `task-kinds.json`, `test-source-classes.json`, `probe.json`, `handler-surface.txt` or `w5b-plugins.json`. `runnable.txt:381-382` lists both as `SUCCESS`, and roadmap `:214`/`:234` says they "stay live".
- **Entries banked by #4: 0.** This matches the "0" in roadmap row 4. Any oracle evidence for #4 has to be **new**: either a recorded probe against the local Kestra image `28f59bf175e1` (present locally, verified with `docker image inspect`) talking to a local echo server, or unit tests with a local `axum` echo server and an injected resolver (the `egret-automation` pattern, §3.4).
- The tests that use `http.Request` as the example of an "unregistered type" must change (footprint step 7): `EW tests/worker.rs:1560` (`UNREGISTERED`) and `EW tests/end_to_end.rs:651` (`UNIMPLEMENTED`). Also affected: `core.http`/`core.slack` in `EW tests/not_served.rs:585-589`, `tests/flow_schema.rs:626-630`, `tests/executions_routes.rs:2934`, `src/spec/validate.rs:2228,4356,4367,4852`, and `src/models/flow.rs:473`.

---

## 3. The egret side

### 3.1 New-task-type footprint, checked against the code

The memory file lists 7 places. Each was checked against the code:

1. **`EW src/spec/registry.rs`**: `TASK_TYPES` at `:91` (40 rows today) and `EXECUTOR_LEVEL` at `:376`. Confirmed. The http types would go in as FQCN rows. Whether `io.kestra.plugin.fs.http.*` should be aliases is a decision (Kestra declares them as aliases, `Request.java:333`).
2. **Handler registration**: `EW src/worker/registry.rs` `with_core_tasks` `:126`, `add_publishable` `:198`, `register` `:234`; the pool-needing registrations are called from `runtime::handlers_for` (`EW src/runtime.rs:749`, which already calls the storage, namespace, kv and purge_logs `register`s at `:769-786`). Confirmed. The http handlers need **no pool**, but they do need the egress policy, the secret source (`render_with_secrets`), the `FileStore` (Download, multipart) and config. That means a `register(handlers, cfg, files, secrets)` called from `handlers_for` rather than `with_core_tasks`.
3. **`runtime::served_for`**: now at **`EW src/runtime.rs:976`** (the memory says ~1307, so it has drifted). The drift tests are at `:1126-1160`. Confirmed.
4. **Property-key arm**: the memory says `measure.rs`, but **the per-handler `wire_fields_of::<Spec>()` arms are in `EW src/spec/keys.rs:330-339`** (log, ret, fail, sleep, output_values, assert, purge_logs). `measure.rs` only builds the `property-treatment.json` sites (`fields_of` at `:586-594`, `:831`). So: add the arm in `keys.rs`, then regenerate `property-treatment.json` (generator switch `EGRET_WRITE_PROPERTY_TREATMENT`, `egret-config/src/lib.rs` `ENV_OUTSIDE_CONFIG`). **The memory needs a correction.**
5. **Kestra schema**: `KESTRA_TYPES` at `EW src/spec/schema/trim.py:46`, and the export recipe at `schema/README.md:18,46,62` (image `28f59bf175e1`, present locally). The trimmed `kestra-flow-schema.json` names `io.kestra.plugin.core.http.{Download,Request,SseRequest}` only in the type list (`:21137-21139`) and has **no definitions** for them. They must be exported together with the `HttpConfiguration`/auth/proxy/ssl/timeout defs. The hand-written stubs for `core.http`/`core.slack` are at `schema/egret-task-schemas.json:73-80`.
6. **Oracle files**: `oracle-exec/driver.rs` `fn registry` `:142`, `LIVE = 295` `:853`, `UNKNOWN_TYPE_REFUSED = 21` `:931`, `VALIDATE_REFUSED = 25` `:979`; `handler_surface.py` `W3B_HANDLERS` `:441` and `W3A_EXECUTOR_LEVEL` `:545`; `baseline.txt`; `PUBLISHED_BUT_REFUSED` at `EW src/executor/mod.rs:4400`; README coverage. Confirmed. With 0 corpus entries, none of the counts should move (**unverified** until the ratchet runs).
7. **Tests using the type as "unregistered"**: see §2. Confirmed. C1 already moved them *to* `core.http.Request`, so they must move again (to e.g. `io.kestra.plugin.core.flow.Template` or a made-up FQCN).

Extra places the memory does not list:
- The `egret-bin` comments that count handlers (`crates/egret-bin/src/main.rs:2219-2280`: "Six of twenty-three registered types have no handler", "13 core handlers") and `EW src/worker/claim.rs:955-975`. Both counts are already stale (`TASK_TYPES` has 40 rows).
- `egret-task-schemas.json` descriptions for the unserved rows still say "a flow using it publishes, and its run does not progress past this task" (`:39,51,63,75,78`). That has been false since no-silent-stall: publish refuses with `task_type_not_served`.
- The `secret()` validator scan (`EW src/spec/validate.rs:514-580`) exempts only `plugins::plugin_tasks`. It would need an http exemption if #4 lets `secret()` resolve on http tasks (§6).

### 3.2 Roadmap citations that drifted
- `registry.rs:161` is still the `core.http` row (`:160-164`). `core.slack` is at `:165-169`, `egret.sql` at `:92-99`, `egret.dataapi.call` at `:136-143`, `egret.dataset.refresh` at `:144-151`, and `egret.report.send` at `:152-159`.
- `egret-bin/src/main.rs:1835-1864` **no longer points at the unserved list.** It is now at **`main.rs:2236-2237`** ("`egret.sql` is not registered: it needs an `Arc<dyn QueryExecPort>` that nothing here provides") and **`:2262-2266`** (the six rows). `install_served(served_for(..))` is at `:2032`, and `handlers_for(..)` is called at `:2289` with the `WorkspaceSecretSource` at `:2292-2296`.

### 3.3 How a handler gets secrets
- `secret()` resolves **only on `kestra.*` plugin tasks**: see `EW src/runners/secrets.rs:1-50` (collect, fetch, resolve; one pass; values masked through `logs::mask`; a key computed from a secret value is refused unnamed). The refusal text is `SECRET_REFUSAL` (`EW src/runners/functions.rs:242-254`), and the save-time finding is `secret_outside_plugin_task` (`validate.rs:514-580`).
- The refusal's reason is **pod-spec exposure for script tasks**. That reason does not apply to an in-process http handler, which can call `render_with_secrets` exactly as the plugin path does. The roadmap, however, puts "`secret()` in … http tasks" in **#10** (`:83`). See §6, D1.
- The secret source is `egret_bin::workflow_secrets::WorkspaceSecretSource` over `workspace_secrets`, passed into `handlers_for` (`main.rs:2292-2296`).

### 3.4 Existing HTTP clients and SSRF guards
- **The only guard for user-supplied URLs is `crates/egret-automation/src/deliver/destination.rs`.** It parses with `url::Url` and judges `host_str()`, so userinfo, path and query cannot change the host (`:89-97`, test `userinfo_cannot_disguise_the_real_host` at `:331`). It resolves through an injected `Resolver` trait (`:43-57`). **Every** resolved address must be public (`:114-123`). `blocked_reason` (`:133-206`) covers v4 unspecified, loopback, RFC1918, link-local (169.254.169.254), broadcast, multicast, documentation, CGNAT 100.64/10, 192.0.0/24, benchmarking 198.18/15 and 240/4; on v6 it covers loopback, multicast, ULA fc00::/7 and link-local fe80::/10, with v4-mapped addresses judged as v4. Escape hatch: `SsrfPolicy { allow_hosts }`, an exact case-insensitive host match **before** resolution (`:59-76`).
- **Gaps in that list** (it was not written for an arbitrary-request feature): IPv6 NAT64 `64:ff9b::/96` and 6to4 `2002::/16`, which embed v4 addresses; v4-compatible `::a.b.c.d`; Teredo `2001::/32`; `0.0.0.0/8` beyond `0.0.0.0` itself (**unverified**: `is_unspecified` covers only 0.0.0.0); and the AWS IPv6 IMDS `fd00:ec2::254`, which ULA does cover. Also, because `allow_hosts` is matched before resolution, an allowed name is trusted whatever it resolves to.
- **Rebinding defence:** the vetted addresses are **pinned** with `reqwest::ClientBuilder::resolve(host, addr)`, and **redirects are disabled** (`webhook.rs:60-105`, `redirect::Policy::none()` at `:80`). This is the pattern to copy. It is wired at `crates/egret-bin/src/main.rs:2088-2091` from `config.automation.webhook_allow_hosts` (`crates/egret-config/src/lib.rs:1729-1734`).
- **Problem 1 with pinning:** with a proxy, DNS happens at the proxy, so pinning does nothing. reqwest also **reads `HTTP(S)_PROXY` from the environment by default**. `webhook.rs` does not call `.no_proxy()`; `egret-apps/src/proxy/mod.rs:120` and `subprocess_pod.rs:68` do. The http handlers must call `.no_proxy()`, or treat a configured proxy as the egress point (§5).
- **Problem 2, crate placement:** `egret-workflow` does not depend on `egret-automation`, and it should not (port rule). The guard should move to a small shared module (for example `egret-core` behind a feature, or a new `egret-egress` crate) used by both. `url` and `reqwest 0.12` (rustls, `json`, `cookies`) are already workspace deps (`Cargo.toml:58,209`). `egret-workflow` currently has **no** `reqwest` dependency (**unverified** for transitive use; `url` is a dev-dependency only).
- The other reqwest users have no user-chosen URLs: OIDC (`egret-auth/src/oidc.rs`), engine connectors (`egret-query/.../connector/{trino,starrocks,http_pool}.rs`), engine-mgmt, the renderer client, `egret-automation/src/deliver/slack.rs` (fixed Slack API, "No SSRF vetting here", `:10`), the apps proxy, and the provisioner.
- The "URL normaliser turns a path into a host" lesson (memory `url-normaliser-turns-path-into-host.md`, `destination.rs:15-19`): parse, never scan or prefix. Kestra's `startsWith` lists are this class of hole.

### 3.5 Where the worker runs (this sets how much SSRF matters)
`handlers_for` handlers run **in the egret-bin process** (`runtime::spawn_with_logs`, `main.rs:2307ff`). An in-process http task therefore sees egret's **own network position**: the metadata Postgres, the Kubernetes API with egret's service account, the internal engine endpoints (Trino/StarRocks admin), the renderer, the Polaris/catalog, and cloud metadata. Script task pods are fenced differently: `deploy/helm/egret/templates/networkpolicy-tasks.yaml` allows "anything EXCEPT egret's Postgres, the API server and `deniedCidrs` (metadata by default)". Private ranges are **allowed** for task pods. That NetworkPolicy does **not** apply to egret's own pod.

### 3.6 File storage for Download
- `Outcome.files: Vec<StoredFile>` (`EW src/worker/mod.rs:343-395`) is indexed in the outcome's transaction. The `StoredFile` struct is at `EW src/storage/mod.rs:370`, and `task_prefix` at `EW src/storage/uri.rs:393`.
- The template is `egret.sql` STORE (`EW src/worker/handlers/sql.rs:94-110,700-806`): stream through `tokio::io::duplex` into `fs.store.put_stream(&object, &mut rx, Some(size))` (`:763`), using a **fixed** filename (`STORE_FILE`) so that a re-claim overwrites its own object. `core.storage.Write` (`EW src/worker/handlers/storage.rs`) is the other template. Download diverges from Kestra here: Kestra uses a random name or `Content-Disposition`; egret should keep the name stable per run and sanitise it to one segment.
- The internal storage is optional (`crate::storage::current()` returns `Option`). Without `[storage]`, Download must fail and name the missing config, as `fetch: STORE` does.

### 3.7 Output size
Every render pass is capped at `MAX_OUTPUT_BYTES = 1 MiB` (`EW src/runners/env.rs:204`), and execution variables are capped too (`EW src/executor/update.rs:28`). Outputs persist in `task_runs.outputs` (JSONB) and go into every later render context. **Kestra's 19 MiB body would break every later render that touches it**, so egret's `Request` body cap should be ≤ 1 MiB, with an error that points at `Download`.

### 3.8 Config conventions
Figment `EGRET_` prefix with the `__` split (`crates/egret-config/src/lib.rs:2173-2190`); unknown keys only warn (`:2100-2112`). New keys therefore go on `WorkflowConfig` (`:750`), for example `workflow.http.allow_hosts`, which automatically becomes `EGRET_WORKFLOW__HTTP__ALLOW_HOSTS`. No `ENV_OUTSIDE_CONFIG` entry is needed. `webhook_allow_hosts` is **not documented** anywhere in this repo (`git grep` finds only the config and main.rs). The docs repo (`egret-data/docs`) was not checked (**unverified**).

---

## 4. The six unserved rows

| Row | What it is | What it needs | Code today | Recommendation and cost |
|---|---|---|---|---|
| `egret.sql` | Runs a bound-parameter statement on a workspace engine, with `fetch: NONE / FETCH_ONE / FETCH / STORE` | (a) An `Arc<dyn QueryExecPort>` in `handlers_for`. It **exists**: `query_port = Arc::new(engine_state)` at `main.rs:2981`, built from `engine_state` at `:1947`, but **after** `handlers_for` at `:2289`, so wiring means moving that line up. (b) **A `RefKind::Database` resolver.** `refs.rs:95` always returns `None` for Database, Dataset and Chart, so **a flow with `egret.sql` cannot be published even with a handler**. It needs a `query.databases` ownership port (`egret_query::DatabaseRepo` exists, `main.rs` `databases`). (c) A principal: today `flows.created_by` (`EW src/worker/mod.rs:292-307`); the open `run_as` question was left to W6, and whether W6 settled it is **unverified**. | **The handler is complete**: `EW src/worker/handlers/sql.rs` (91 KB), with `tests/sql_starrocks_live.rs`. 0 oracle entries (Kestra has no such type). | **Wire.** Cost **M**: port wiring, the Database ref port with its test (`refs.rs` has the test to update, `a_kind_with_no_port_refuses_even_a_well_formed_reference`), served_for, keys.rs, docs. Highest value. |
| `egret.dataapi.call` | Calls a Data API endpoint of this workspace (`endpoint` ref). There is no property for params yet (schema `:37-48`). | A new **invoke** port in `egret-dataapi-api`, since `DataApiEndpointPort` only has `workspace_of` (`refs.rs:9,61`), running the serving pipeline as the flow principal with params. The ref already resolves. | Nothing except the registry row and schema stub | **Defer or remove.** A real call is M (params schema, principal, rate-limit, audit). Once http.Request lands, a call through the public Data API URL is possible, but the SSRF guard would block an in-cluster URL. |
| `egret.dataset.refresh` | `POST /ws/{ws}/datasets/{id}/refresh` **reconciles the physical columns** of a BI dataset (`crates/egret-bi/src/routes.rs:694,1104-1140`). It is metadata, not data materialisation. | An `analytics.datasets` port (resolve plus refresh). `RefKind::Dataset` never resolves (`refs.rs:95`). | Nothing | **Remove.** Low value as a scheduled task: the name suggests materialisation, which it is not. Cost S to remove. |
| `egret.report.send` | Sends a chart (`chart` ref). Egret's reports are `egret-automation` alerts/reports with targets and recipients; there is `POST /ws/{ws}/alerts/{id}/run` (`crates/egret-automation/src/routes.rs:157`). | A `RefKind::Chart` port **and** a new automation "run now" port, or a redefinition as "run report `<id>`". The chart-only shape does not match the report model (recipients, format). | Nothing | **Remove now, or redefine later as `egret.report.run` (report id).** Defer cost M. |
| `core.http` | A reserved placeholder with no properties (schema `:73-76`) | Superseded by `io.kestra.plugin.core.http.Request` | Nothing | **Remove** (or make it an alias of `Request`, which the registry's no-invented-alias rule argues against, `registry.rs` comment at `:170-186`). Cost S, plus the test churn listed in §2. |
| `core.slack` | A reserved placeholder | Kestra's Slack is a **plugin** (`io.kestra.plugin.slack(.notifications).SlackIncomingWebhook`), not core. An incoming webhook is one POST, which `http.Request` covers. `egret-automation/deliver/slack.rs` uses a bot token and channel from workspace `slack_config`, which is a different design. | Nothing. The plugin catalog (`EW src/plugins/catalog.json`) holds only `io.kestra.plugin.jdbc*`. | **Remove.** Cost S. A future plugin-host catalog entry would cover Kestra's Slack plugin. |

Removing a row from `TASK_TYPES` makes stored flows that use it fail validation (`unknown type`) rather than `task_type_not_served`. They could not be published or started anyway, so the user-visible change is only the error code. The stored-execution path (`executor::plan_stored`) still fails such a run at the worker with `NoHandler`/unknown type.

---

## 5. Egress policy options

**Threat:** the flow author is any workspace editor. The URI, headers, proxy and redirects are author-controlled. The worker is egret itself (§3.5).

| Option | What it does | For | Against |
|---|---|---|---|
| **A. Default-deny non-public addresses** (reuse `destination::vet`, pin the addresses, re-vet every redirect hop, `.no_proxy()`) plus a per-deployment **host allowlist** (`workflow.http.allow_hosts`) | The same posture as alert webhooks | A proven pattern with tests already in the repo. Closes metadata, Postgres, the API server and rebinding. One rule for both features. | Blocks legitimate private APIs (an on-prem ERP at 10.x) until the operator lists them. Stricter than script pods, which may reach private ranges. |
| **B. Mirror the task-pod NetworkPolicy** (deny only egret's Postgres, the API server, `deniedCidrs`, loopback, link-local and egret's own Service) | Same reach as a script task | Consistent with what an author can already do by `curl` in a container. Fewer allowlist requests. | In-process, "egret's own services" is a long and drifting list (engines, renderer, Polaris, admin ports, the `egret_engine` endpoints). One missed entry is an authenticated-from-egret request. It is also hard to test exhaustively. |
| **C. Run http tasks in a task pod** (the container runner) | The NetworkPolicy does the fencing | Real network isolation, with no IP list in Rust. | Needs `container_runner` (off by default, `workflow_containers` feature). Seconds of pod start per request. Kestra runs these in-process. |
| **D. Kestra-compatible prefix lists** (`allowed-list` / `denied-list` on the URI string) | Kestra parity | Familiar to Kestra users | **Unsafe** (§1.2): no parsing, no DNS, no redirects. Could be *added on top of* A as an extra narrowing (parsed scheme+host+port+path-prefix match), never as the guard itself. |
| **E. Per-workspace allowlist** (in addition to A) | Workspace admins list their own internal hosts | Fits the multi-tenant model, so the operator is not a bottleneck | A workspace admin could allow egret's own internal hosts, so this needs an operator-level **never-allow** set (metadata, Postgres, API server, egret Services), a table or migration, UI and audit. **L.** Better as a follow-up. |

Implementation points for any option:
- Parse with `url::Url` and allow only `http`/`https`. Refuse userinfo, or at least judge `host_str()`. Port policy: allow any, or only 80/443 plus a list (**decision**).
- Resolve once and judge **every** address. Pin with `ClientBuilder::resolve`. Disable automatic redirects and follow them manually up to N hops (reqwest's own default is 10), **re-vetting each Location**. Drop `Authorization` on a cross-origin redirect: Kestra's Basic/Bearer interceptors add it to *every* request including redirects (`BasicAuthConfiguration.java:48-55`); behaviour after a host change is **unverified**.
- `options.proxy`: either refuse it, or vet the proxy host as a destination and accept that the final target is then unvetted. A **deployment-level** egress proxy (`workflow.http.proxy`) is the cleaner enterprise answer and makes the proxy the policy point.
- `ssl.insecureTrustAllCertificates`: decide whether to honour it. Kestra's `!= null` bug should not be copied.
- Caps: connect timeout, total timeout (default to the task `timeout:`), body cap ≤ 1 MiB for `Request` (§3.7), Download cap (a new `workflow.http.max_download_bytes`, **decision**), SSE max events / bytes / duration (Kestra has none).
- Logging: never log `Authorization`, `Proxy-Authorization`, `Cookie` or `Set-Cookie`. Mask the values of resolved secrets (`Resolved::values`). Truncate the failure body (Kestra puts the **whole** body in the error message, which ends up in the task-run message and logs).
- Audit: whether to emit an egress audit event per request (**decision**).

**Kestra's own behaviour, for comparison:** no default block at all, author-chosen proxy, trust-all available, redirects followed unvetted, only prefix lists for operators. Egret diverging to option A is a security divergence and belongs in roadmap §3.1.

---

## 6. Ambiguities, divergences, and decisions for the user

**Security decisions:**
- **D1. `secret()` on http tasks in #4 or in #10?** The roadmap puts it in #10 (`:83`). Without it, `options.auth.password/token` and `Authorization` headers can only come from inputs or KV (KV refuses `secret()` too, `validate.rs` `secret_in_kv_value`), so http.Request is barely usable for authenticated APIs. The pod-spec reason does not apply in-process. **Recommendation:** resolve `secret()` on the three http tasks through `render_with_secrets` in #4, extend the `validate.rs:514` exemption, and mask the values.
- **D2. Egress option:** A (recommended) vs B vs C, plus whether to add D-style narrowing and E later.
- **D3. Allowlist scope:** one shared allowlist with `automation.webhook_allow_hosts`, or a separate `workflow.http.allow_hosts`. Exact host match only, or also suffix/CIDR. Note that today's exact-match skips resolution entirely for a listed host.
- **D4. `options.proxy` from the flow:** refuse, vet, or replace with a deployment proxy.
- **D5. `ssl.insecureTrustAllCertificates`:** honour (Kestra parity), refuse, or allow only when the deployment opts in.
- **D6. Redirects:** follow with re-vet (parity) or refuse. Whether credentials survive a cross-host hop.
- **D7. Ports:** any port vs a list. Some internal admin ports sit on public IPs.
- **D8. Body in error messages:** cap and mask (diverges from Kestra's whole-body message).
- **D9. `encryptBody`:** implement with the existing SECRET-input cipher (`EW src/secret_inputs.rs`, installed at `main.rs:2015-2018`) or refuse.

**Behaviour or parity choices:**
- B1. The `Request` body cap: 1 MiB (egret render cap) vs Kestra's 19 MiB, with the error text adapted.
- B2. The Download filename: stable per run (re-claim safe) vs Kestra's random or `Content-Disposition` name. `saveAs` still honoured. Sanitise `/`, `\` and `..`.
- B3. `Download` has no cap in Kestra; egret needs one (config key).
- B4. `SseRequest`: fail on a status of 400 or above (Kestra does not check at all, which is arguably a Kestra bug); add an event or byte cap. JQ through `jaq` (already a dep, `EW Cargo.toml:225-227`), but jaq is not jackson-jq 1.6, so expect small differences (**unverified** which).
- B5. `DIGEST` auth: reqwest has no digest support, so it needs a crate or an implementation. Could be refused in #4 as a stated divergence.
- B6. `options.logs`: implement with redaction, or ignore.
- B7. Aliases `io.kestra.plugin.fs.http.Request/Download`: register them as aliases (Kestra does) or not.
- B8. Should `Request.body` be parsed as JSON? Kestra keeps a string, so `{{ outputs.x.body | fromJson }}` patterns rely on that. Keep it a string.
- B9. `uri` output: the request URI (Kestra) even after redirects.
- B10. `allowedResponseCodes` replaces the default rule (2xx not implied), as in Kestra.
- B11. Removing `core.http` / `core.slack` / `egret.dataset.refresh` / `egret.report.send` changes the error code for flows that already could not run. Confirm that no stored flow in production uses them (**unverified**: no data query was made).
- B12. `egret.sql` wiring brings in the `run_as` hole: `created_by` is `ON DELETE SET NULL`, and ad-hoc runs have no principal. Accept it for now, or wait for a `run_as` column.

**Stale items to fix along the way:**
- Memory footprint step 3 line (~1307 → `runtime.rs:976`) and step 4 location (`measure.rs` → `spec/keys.rs:330`).
- The roadmap citation `main.rs:1835-1864` → `main.rs:2236-2266`.
- `egret-task-schemas.json` texts saying the unserved types "publish".
- The handler and row counts in the comments at `main.rs:2219-2266` and `claim.rs:955-975`.
- Kestra doc drift to note in the spec (`maxContentLength`, `maxEvents`).
