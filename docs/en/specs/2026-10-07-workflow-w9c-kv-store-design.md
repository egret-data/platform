# W9c — The KV store: `kv()`, the `core.kv` tasks, the expiry sweep and the KV API

**Status:** design, 2026-10-07; not yet implemented.
**Depends on:** W9b (namespace files: `nsfiles::access::authorize` and `Grant`, the staging objects and their sweep, the advisory-lock helpers, the render-time namespace connection in `storage/scope.rs`, the route conventions of `nsfiles/routes.rs`) — merged as #653. W9a (the process-wide `FileStore`, `egret-storage`'s `ObjectStore`), W5b (the plugin host) — merged. Base `origin/main` `11782600`.
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 finding 4, §9 (row W9: "split on 2026-10-01 … W9c *the KV store*"); W9b spec `docs/{en,vi}/specs/2026-10-05-workflow-w9b-namespace-files-design.md` §14 (the KV store left to W9c).
**Decided with the user, 2026-10-07:**
- **(2026-10-07) Scope.** W9c ships the `kv()` function; the tasks `Set`, `Get`, `GetKeys` and `Delete`; `PurgeKV`; the deep-merge `Put` task; and the KV API. The plugin host gets **no** KV bridge, because no catalog plugin calls `namespaceKv`. Its refusal message changes to point at the `core.kv.*` tasks (§10).
- **(2026-10-07) The write scope is W9b's.** A flow reads the KV of any namespace in its workspace. It writes and deletes only in its own namespace. `PurgeKV` acts on the flow's own namespace and its child namespaces, on a dot boundary. One `authorize` serves both W9b and W9c, with a KV resource kind added. Kestra lets a flow write any existing namespace; §2 records the divergence (§5).
- **(2026-10-07) Storage follows Kestra.** Each revision's value is an Ion text object at `{prefix}{ws}/{ns-as-path}/_kv/{key}.ion`, or `….ion.vN` for revision N > 1. A Postgres table, `workflow.kv_entries`, holds one row per revision: workspace, namespace, key, revision, type, size, description, `expires_at`, deleted, `created_at` and `created_by` (user, execution or task run). egret **stores** the type, where Kestra re-infers it on read; the Task 1 fixture decides whether read results still match Kestra, and every measured difference is a row in §2. A write reuses W9b's staging. The body is staged while no connection or lock is held. Then one short locked transaction locks `(ws, ns, key)`, checks `overwrite`, picks the revision, copies server-side, runs `head`, inserts the row and commits. So `overwrite: false` and `Put`'s read-merge-write are race-free. A value is capped by `max_file_bytes`; Kestra has no cap (§2). *(Amended 2026-10-07 by the user's ruling on the value cap, below: the cap is `max_read_bytes`.)* Keys follow Kestra's regex `[a-zA-Z0-9][a-zA-Z0-9._-]*`, and their length is bounded (§3). The migration takes the next free number on main: 0175 today, re-checked by the plan's rebase step (§3, §4).
- **(2026-10-07) TTL: expired means missing.** A read of an expired key behaves exactly like a read of a missing key, and it honours `errorOnMissing`. Reads never write. An hourly sweep hard-deletes expired keys, rows and objects, by row id, across every workspace, and it is safe to run on every replica. Kestra soft-deletes on read, errors regardless of `errorOnMissing`, and sweeps only the `main` tenant and namespaces that have flows (§2, §4.5, §11).
- **(2026-10-07) Inheritance follows Kestra.** A `kv()` with no namespace, or a `Get` whose namespace is the flow's own, walks up the parent namespaces on a dot boundary. An explicit namespace does not walk. The API has a list-inherited route (§4.6, §9).
- **(2026-10-07) `secret()` inside a `Set` or `Put` value is refused at publish.** The error says the secret's value would be exposed through the KV store. This diverges from Kestra (§2, §7).
- **(2026-10-07, the user's ruling on the value cap) A KV value's writes are capped at `max_read_bytes`, not `max_file_bytes`, so anything writable is also readable.** `Set`, `Put` (its merged result included) and the API's `PUT` refuse an encoded value over `max_read_bytes`, naming the cap (§2, §4.2, §4.3, §8.1, §8.5, §9).
- **(2026-10-07) Approved design points (sections 2–4 of the brainstorm).**
  - Versions and soft delete follow W9b's R1. Every write makes a new revision. `Delete` flags the latest revision. History is visible through the API, with deleted revisions flagged. A rewrite after a delete makes a new revision.
  - `PurgeKV` purges expired keys only, or by version (`before` / `keepAmount`). It filters with `keyPattern` and deletes by row id with keyset paging. W9b's rule applies: a version purge needs `keepAmount` or `before`.
  - `Set` takes `key`, `value`, `overwrite` (default true), `ttl`, `kvType` and `kvDescription`. BOOLEAN is strict, as in the artifact.
  - `Get` outputs `value`. `GetKeys` takes `prefix`, lists live keys only, and is bounded by `max_namespace_files`. `Delete` outputs `deleted`. `Put` deep-merges under the key's lock, with merge semantics measured in Task 1.
  - `kv()` uses W9b's per-render connection and `authorize(Read)`. Its `errorOnMissing` defaults to true; the tasks' defaults to false.
  - The API lives under `/api/ws/{ws}/workflows/namespaces/{ns}/kv…`. It has list, list inherited, get (with type), revisions, put (through staging, with a body bound and timeouts), delete and bulk delete. Reads need `workflow:read` and writes `workflow:manage`. A lock wait over 5 s answers 409. Statuses follow W9b's conventions.
  - The plan's shape (§12) and the testing approach (§13), with W9b's lessons as global constraints.
- **(2026-10-07, the user's rulings on the plan's gaps)**
  - **Bulk delete.** The API's bulk `DELETE` takes at most **1,000 keys per request**. It stays one all-or-nothing transaction. A body over the cap answers 413 naming it (§9). One advisory lock per key, in one transaction, at `max_namespace_files` keys could exhaust Postgres's shared lock table (`max_locks_per_transaction` × `max_connections`, about 6 400 slots on a stock server).
  - **Server-paced store calls under the key's lock are accepted.** These are `Put`'s bounded read and write, the copy and `head` of `Set` and the API's `PUT`, and the object deletes of `PurgeKV` and the expiry sweep. Client-paced I/O under a lock (a request body), a render and a plugin run stay forbidden. API writers keep the 5 s lock wait, then 409 (§4.3, §15).
  - **`Put` on an expired key treats it as missing.** It starts fresh and carries over no description and no expiry (§4.5, §8.5).
  - **A namespace of another workspace** answers exactly as an empty namespace. The listings answer `200 []`; a key read and `revisions` answer 404 (§9, §13).
  - **Decisions recorded from the plan:**
    - a key named `inheritance` cannot be read by `GET …/kv/{key}`, as in Kestra (§9, §15);
    - the expiry sweep's first pass comes one interval after start (`interval_at`; §11);
    - `PurgeKV` re-reads and judges a key's rows under the key's session lock (§8.6);
    - no second latest-row index (§4.1);
    - the audit's spelling (§9).

**Delivery:** one PR (docs and code), eight tasks in §12, implemented by a plan written after this spec.

## 1. What exists and what does not

| piece | state at `11782600` |
|---|---|
| `kv()` | refused by name: `("kv", not_yet("the namespace KV store", "W9c"))` in `REFUSED` (`runners/functions.rs:276`, seven entries); pinned by `kv_still_names_w9c_and_the_refused_list_shrinks_to_seven` (`:3325-3370`) |
| `io.kestra.plugin.core.kv.*` | no row in `spec/registry.rs`, so publish refuses each as an unknown type (`publish.rs:449-459`; `spec/validate.rs:1654` warns `unknown_task_type`) |
| `secret()` | resolves only on `kestra.*` plugin tasks; everywhere else it is `Refusal::Confined(SECRET_REFUSAL)` (`runners/functions.rs:229-232`, `:275`), and publish reports `CODE_SECRET_OUTSIDE_PLUGIN_TASK` with that generic reason (`spec/validate.rs:395-403`) |
| plugin host | `ShimRunContext.namespaceKv` throws `EgretUnsupported` naming W9c (`plugin-host/…/ShimRunContext.java:340-341`, class doc `:52`; pinned by `EgretPluginHostTest.java:125-126`; doc mentions `EgretUnsupported.java:4`, `EgretPluginHost.java:58`). The handler's exit-4 message names "the KV store — W9c" (`worker/handlers/kestra.rs:379`, module doc `:79`, pinned at `:1317-1321`). The three locked plugin jars (postgres, mysql, trino 2.0.0) hold **0** references to `namespaceKv` |
| W9b reusables | `nsfiles::access::{authorize, Actor, Access, Grant, check_namespace, check_files_segment, API_LOCK_WAIT}` (`access.rs:23-176`); `nsfiles::staging::{record, forget, sweep, spawn_staging_sweeper}` and the table `workflow.namespace_file_staging` (`staging.rs:47-192`); the write path `store::put` → `stage_body` → `commit_staged` (`store.rs:1065`, `:1108`, `:1146`) and its transaction lock `pg_advisory_xact_lock(hashtextextended(…, 0))` (`store.rs:567-605`); `storage::lock::LockedSession` (session locks, `lock_timeout`, `is_lock_timeout`); `NsError` and `CreatedBy` (`nsfiles/mod.rs:37-73`); `nsfiles::db_options()` and the render's one namespace connection (`storage/scope.rs:26-42`); the routes' `ns_refusal`, `no_store`, `audit`, the 408 body timeouts (`nsfiles/routes.rs:202`, `:245`, `:349`, `:821-887`) |
| caps | `Caps { max_file_bytes (1 GiB), max_task_bytes (5 GiB), max_read_bytes (10 MiB), max_namespace_files (10 000) }` (`storage/mod.rs:61-67`, defaults `egret-config/src/lib.rs:898-957`) |
| migrations | the last is `0174_workflow_namespace_files.sql`; W9c takes **0175**, re-checked at rebase |
| corpus | 2 of 345 `oracle-exec` flows use KV. `valids/kv.yaml` (`kv('my-key').field ?? null`, `kv('my-key', inputs.namespace, inputs.errorOnMissing)`) is `not-standalone`, has no golden entry, and is an over-refusal guard (`oracle-exec/README.md:3627-3637`). `valids/retry-with-flowable-errors.yaml` (`kv.Set` with `kvType: NUMBER`, then `AllowFailure` incrementing it through `kv(namespace=…, key=…) + 1`) is baselined (`baseline.txt:614`) and held by both `core.kv.Set` and the opaque `AllowFailure` flowable (`README.md:6420`). The expression oracle's `function-kv` (`oracle/corpus.json:302`) pins only "Function or Macro [kv] does not exist" (`oracle/golden.json:2201-2205`) |

## 2. What Kestra does, and where egret differs

Kestra (source `96893b589`; artifact `2.0.0-rc13`, nightly `169ca7f2`, the image W9a and W9b measured). Where the two differ, the artifact governs. Paths are relative to `core/src/main/java/io/kestra/plugin/core/kv/` unless named.
- **Tasks.** `Set` (`Set.java:63-147`) renders `value` with `renderTyped` (`:111`), defaults `namespace` to `{{ flow.namespace }}` (`:86`) and `overwrite` to true (`:93`), and coerces a String by `kvType`: NUMBER by Jackson; BOOLEAN strictly in the artifact (`equalsIgnoreCase("true"|"false")`, else `Cannot parse '%s' as a BOOLEAN value: expected 'true' or 'false'.`; the source's `TypeConverter.toBoolean` at `:120` is older); DATETIME by `toInstant`; DATE by `LocalDate.parse` falling back to an `Instant`'s UTC date (`:156-162`); DURATION validated then stored as the raw string; JSON by `JacksonMapper.toObject`. A Number with `kvType: STRING` becomes its `toString()` (`:131-133`). `Get` (`Get.java:55-122`) has `errorOnMissing` false (`:74`), walks parents when the namespace equals the flow's (`:82-83`, `:99-111`), else checks the ACL (`:85`); missing with `errorOnMissing` throws `No value found for key '…' in namespace '…' and \`errorOnMissing\` is set to true` (`:91`); an expired key fails the task whatever `errorOnMissing` says (`:77-97`, inferred). `GetKeys` (`GetKeys.java:49-88`) lists live keys by `prefix`, with the ACL check (`:65`). `Delete` (`Delete.java:47-90`) soft-deletes, `errorOnMissing` false (`:66`), output `deleted`. `Put` (`Put.java:72-121`) read-modify-writes: a Map deep-merges into a Map, a list of `{key, value}` patches dotted paths, a single `[{value}]` replaces; it keeps the description and expiry and always overwrites (`:100`, `:121`). `PurgeKV` (`PurgeKV.java:57-132`) takes `keyPattern`, `namespaces` xor `namespacePattern` (neither: all namespaces), `includeChildNamespaces` true (`:90`), `behavior` default `{type: key, expiredOnly: true}` (`:83`, `Key.java:33`) or `{type: version, before, keepAmount}` (`Version.java:30-69`), and hard-purges.
- **`kv()`** (`core/runners/pebble/functions/KvFunction.java:23-104`): arguments `key`, `namespace`, `errorOnMissing` (`:34-36`); `errorOnMissing` defaults to **true** (`:53`, confirmed in the artifact's bytecode); no namespace means the flow's with parent inheritance (`:61-63`, `:84-96`), an explicit one, even the flow's own, none (`:64-66`); missing: `The key '…' does not exist in the namespace '…'.` (`:78`); no key: `The 'kv' function expects an argument 'key'.` (`:100`); an expired key is swallowed to `null` when `!errorOnMissing` (`:68-72`).
- **Storage.** `kvFileName` is `key + ".ion"` plus `".v"+rev` for rev > 1 (`KVStore.java:41-43`), under `/{ns-as-path}/_kv` (`StorageContext.java:43`, `:328-329`). A `Duration` is written as its raw ISO string, every other value as Ion text (`InternalKVStore.java:80`, `JacksonMapper.java:138-141`); reading turns any String that looks like an ISO duration into a `Duration` (`:134-142`). The type is not stored: `KVType.from` re-infers it for the API. Description and expiry are object metadata (`KVMetadata.java:43-53`) and DB columns; expiry is `now + ttl` at write (`KVMetadata.java:25`). The table `kv_metadata` (`jdbc-postgres/…/baseline-postgres.sql:383-409`) keeps revisions: a write is `previous + 1` and marks the previous row not-last; a delete marks the latest row deleted without a new revision (`AbstractJdbcKvMetadataRepository.java:160-180`). `put` saves the row before it writes the object (`InternalKVStore.java:82-96`). A read of an expired latest row soft-deletes it and throws `The requested value has expired` (`:150-160`). *(Measured 2026-10-07, Task 1: no task or API path hands the store a `java.time.Duration`. `Set` with `kvType: DURATION` stores the validated String, so every DURATION is an Ion string, `"P1D"` with its quotes; the raw-ISO branch is unreachable.)*
- **Access.** `KVStoreService.checkAccessNamespaceIsAllowed` (artifact): the flow's own namespace or an ancestor on a dot boundary is "self"; any other must pass `checkAllowedNamespace` (always true in OSS, `NamespaceService.java:43-45`) and must **exist** (a flow or KV in it), else `Cannot access the KV store. The namespace '%s' does not exist.` The source (`KVStoreService.java:165-196`) has no dot boundary; the artifact governs. `RunContext.namespaceKv(ns)` passes the flow's namespace as `from` (`DefaultRunContext.java:626-628`).
- **Sweep.** `KVPurgeCleaner` runs hourly after a 1 h delay, off workers, in batches of 1000 (`KVPurgeCleaner.java:44`), over the `main` tenant only and over namespaces with flows only (`:89-93`).
- **API** (`webserver/…/KVController.java`, `/api/v1/{tenant}`): `GET /kv` paged across namespaces (`:56-70`); `GET /namespaces/{ns}/kv/inheritance` (ancestors only, deepest first, first wins, `:73-100`); `GET /namespaces/{ns}/kv/{key}` → `KvDetail{type, value, revision, updated}` (`:103-121`); `PUT /namespaces/{ns}/kv/{key}` (`text/plain` parsed as Ion, falling back to the raw string; headers `description`, `ttl`; always overwrites, `:124-141`); `DELETE …/kv/{key}` → boolean (`:144-150`); `DELETE /namespaces/{ns}/kv` with `{keys: […]}` → `{keys: deleted}`, per-key IO errors ignored (`:153-182`).

| Kestra | egret W9c | why |
|---|---|---|
| a flow reads the KV of any existing namespace (OSS) | any namespace **of its workspace**, existing or not; a namespace with no KV has no keys (§5) | the workspace is egret's tenant. egret has no namespace table, so Kestra's existence check is dropped. A read cannot reveal anything in a namespace that holds nothing |
| a flow `Set`s or `Delete`s the KV of any existing namespace, and of its ancestors (`checkAccessNamespaceIsAllowed`) | its own namespace only; `PurgeKV` its own and its children on a dot boundary (§5) | the user's decision (2026-10-07), W9b's rule: an author with `workflow:manage` on one flow must not rewrite another team's shared state from a task |
| `overwrite: false` is check-then-act, and `revision = previous + 1` is computed with no lock; `Put` reads and writes with no lock | the overwrite check, the revision and `Put`'s merge run under a per-`(ws, ns, key)` advisory lock, in the transaction that inserts the row (§4.3, §8.5) | the user's decision (2026-10-07): two writers never share a revision, and `overwrite: false` and `Put` are race-free |
| the row is saved before the object is written | the object is copied into place, then the row is inserted, in one transaction (§4.3) | W9b's order: a reader never sees a revision with no object behind it |
| a read of an expired key soft-deletes it; `Get` and the API then error regardless of `errorOnMissing`; `kv()` swallows it only when `errorOnMissing` is false | an expired key is a missing key everywhere: `errorOnMissing` decides, and no read writes (§4.5) | the user's decision (2026-10-07): expiry is an absence, not a fault, and a read that writes makes outcomes depend on who looked first |
| `Get`'s parent walk stops with an error at an expired key | the walk treats it as missing and goes on to the parent (§4.6) | the same rule: expired means missing |
| the expiry sweep covers the `main` tenant and namespaces with flows | every workspace and every namespace holding KV, hourly, on every replica (§11) | the user's decision (2026-10-07): an expired value in a flowless namespace would otherwise keep storage for ever |
| `secret()` in a `Set` value stores the secret in plain text, unmasked | refused at publish, naming the task and saying the secret would be exposed through KV (§7) | the user's decision (2026-10-07): anyone with `workflow:read` could read it back |
| no cap on a value | the stored Ion object is at most `max_read_bytes` (10 MiB by default), on every write path (§4.2) | memory and storage are bounded; `kv()` splices the value into a render. The user's ruling (2026-10-07): the write cap equals the read bound, so anything writable is also readable |
| no key length cap except the column `VARCHAR(350)` | at most 350 bytes; a longer key is an author error naming the rule (§3) | any key Kestra can store, egret can store; a long key fails naming it, not as a DB error |
| the type is re-inferred on read (`KVType.from`), so a STRING `"P1D"` reads back as a DURATION | the type is stored with each revision and read back as written (§4.2) | the user's decision (2026-10-07). Which read results differ from Kestra is **measured in Task 1**; each measured difference is added to this table as its own row |
| a namespace segment `_kv` (`a._kv`) is accepted | refused at publish and by every KV and namespace-file operation (§3) | its flows' and files' keys would sit under namespace `a`'s `_kv` prefix; the two key spaces must not overlap (W9b's `_files` rule) |
| no revisions route; a deleted key answers 404 | `GET …/kv/{key}/revisions` lists every revision, deleted ones flagged, and an explicit-revision read of a soft-deleted key succeeds; a latest read is 404 (§9) | W9b's R1: history stays visible until a purge removes it |
| `GET /kv` lists every namespace, paged | one namespace's live keys, bounded by `max_namespace_files` (§9) | W9b's listing rule; a cross-namespace listing is out of scope (§14) |
| `PurgeKV` with no `namespaces` purges every namespace; children match by string prefix in the source | default is the flow's own namespace (and its children); children match on a dot boundary (§8.6) | the purge rule (§5) |
| `PurgeKV` `behavior: {type: key, expiredOnly: false}` hard-purges every matched key | refused, naming `behavior: {type: version, keepAmount: 0}` as the form that purges every revision (§8.6) | the user's ruling that `PurgeKV` purges "expired only, or by version"; a total purge has one explicit spelling, as in W9b |
| `PurgeKV` `behavior: {type: version}` with neither `keepAmount` nor `before`: **measured in Task 1** *(2026-10-07: it purges every revision)* | refused, naming both properties (§8.6) | W9b's rule (2026-10-06, Task 7 review C2), applied here by the user's decision |
| bulk `DELETE` ignores per-key IO errors | one transaction, all or nothing, at most 1,000 keys; the answer lists the keys that were live (§9) | an answer that says "deleted" means deleted |
| `RunContext.namespaceKv` serves plugins | the plugin host refuses it with a reason pointing at the `core.kv.*` tasks (§10) | the user's decision (2026-10-07): no catalog plugin calls it |
| `Get`'s output serialises a DATETIME as an Instant with microseconds (`2025-01-02T03:04:05.000000Z`) and a DURATION as seconds (`86400.0`) | `Get` returns them as the strings `kv()` prints (`2025-01-02T03:04:05Z`, `PT24H`) (§4.2) | measured 2026-10-07 (Task 1); §4.2's reading-back rule gives both readers one printed form |
| a DURATION whose text Kestra's read pattern does not match (`p1d`, `-PT1S`, `PT1.5S`) reads back as a STRING | it reads back as the DURATION it was stored as (§4.2) | measured 2026-10-07 (Task 1); the type is stored (the user's decision) |
| `overwrite: false` over an expired key fails (`Key already exists`); `Delete` of an expired key answers `deleted: true` and soft-deletes it; the API answers 410 for an expired key | the key is missing: the write succeeds, `Delete` answers `deleted: false` (or fails under `errorOnMissing`), the API answers 404 (§4.5, §8.4, §9) | measured 2026-10-07 (Task 1); expired means missing (the user's decision) |
| `PurgeKV` `{type: key}` deletes the revision rows whose own expiry passed, so an older revision without a ttl survives as a row with no latest; `size` counts revisions | every revision of a key whose latest revision expired (§8.6); `size` counts revisions, as Kestra's does | measured 2026-10-07 (Task 1); a purged key leaves nothing behind |
| `PurgeKV` `{type: version, before}` compares the row's `updated` column, the time a revision stopped being the latest | compares the revision's `created_at` (§8.6) | measured 2026-10-07 (Task 1); the spec's rule is kept and the Task 1 report flags it |
| `PurgeKV`'s candidate namespaces are the namespaces that have **flows**: a child namespace with KV but no flow is never purged | the namespaces holding KV that are the flow's own or its children (§8.6) | measured 2026-10-07 (Task 1) |
| a flow's `Put` value (a map, an entry list, even `{{ {'b': 2} }}`) reaches the store as its JSON **text** and is stored as a STRING: the artifact's `Put` never merges from a flow | `Put` merges the typed value as the artifact's own `Put.mergeValues` does (§8.5) | measured 2026-10-07 (Task 1); **awaits the user's ruling** (Task 1 report) |
| `PUT …/kv/{key}` parses an Ion stream and keeps its first value: `hello world` is stored as `"hello"` | a body that is not one Ion value is stored as the raw string (§9) | measured 2026-10-07 (Task 1); the spec's fallback is kept; **awaits the user's ruling** (Task 1 report) |
| the API answers 422 for a refused key, ttl or body, and 500 for a malformed bulk body | 400 (§9) | W9b's status conventions; measured 2026-10-07 (Task 1) |

## 3. Storage layout and keys

- **Object key:** `{workflow.storage.prefix}{workspace_id}/{ns-as-path}/_kv/{key}.ion` for revision 1, `…/{key}.ion.v{N}` for N > 1 (Kestra's `kvFileName`). Same bucket and prefix as W9a and W9b. The workspace always comes from the grant (§5), never from input. `FileStore` gains `namespace_kv_uri(grant, key, revision)` beside `namespace_object_uri`. **No URI names a KV value:** a flow reaches a value only through `kv()` and the tasks, so `storage::uri` is unchanged.
- **The `_kv` segment rule.** A namespace whose segment is `_kv` (`a._kv`, legal by `is_valid_namespace`) would put its flows' execution keys and its namespace files under namespace `a`'s KV prefix. `access::check_namespace` gains the clause beside `_files` (a `check_kv_segment`), so `authorize` refuses such a namespace for every KV and every namespace-file operation, and publish refuses it as it refuses `_files`, each with an error naming the namespace and the segment. Task 1 counts the published flows in a dev database and the corpus entries whose namespace has a `_kv` segment, and lists each; none is expected.
- **The key rule**, one function (`kv::key::check_key`) for every entry point (function, tasks, API): Kestra's regex `^[a-zA-Z0-9][a-zA-Z0-9._-]*$` (`KVStore.java:137`) and at most **350 bytes**. Kestra's messages for an empty key and a key that breaks the regex are reproduced as Task 1 measures them; the length refusal names the bound. **Why 350:** it is Kestra's own column width (`kv_metadata.name VARCHAR(350)`), so every key Kestra can hold, egret can hold. The key is ASCII by the regex, so bytes equal characters. A key never holds `/`, so it is one object-key segment, and it never starts with `_`, so no key can be named like a marker.
- **The key budget.** After the prefix, a KV object key holds at most 36 (workspace) + 1 + 150 (namespace) + 1 + 3 (`_kv`) + 1 + 350 (key) + 4 (`.ion`) + 12 (`.v` and a revision) = 558 bytes. W9b's boot rule (a prefix of at most 200 bytes) therefore keeps every KV key under S3's 1024 bytes too; no new check is needed.
- **Staging** reuses W9b's: a value's encoded bytes go to `{prefix}{ws}/_nsfiles_staging/{uuid}`, recorded in `workflow.namespace_file_staging` (`staging::record`, `forget`), and swept by the existing hourly sweep (`spawn_staging_sweeper`). The table and the segment keep their W9b names.

## 4. Core

### 4.1 The metadata table

`workflow.kv_entries` (migration 0175, re-checked at rebase):
- `id bigserial` primary key; `workspace_id uuid` FK to the workspace, `ON DELETE CASCADE`; `namespace text`; `key text` (the key rule as a CHECK: length ≤ 350 and the regex); `revision int` (≥ 1).
- `value_type text`, CHECKed to one of `STRING`, `NUMBER`, `BOOLEAN`, `DATETIME`, `DATE`, `DURATION`, `JSON`; `size bigint` (the object's bytes).
- `description text` (nullable); `expires_at timestamptz` (nullable; null never expires).
- `deleted bool` default false; `deleted_at timestamptz` and `deleted_by_user`, `deleted_by_execution`, `deleted_by_task_run` (nullable, set together with `deleted`).
- `created_at timestamptz` default `now()`; `created_by_user`, `created_by_execution`, `created_by_task_run` with W9b's one-writer CHECK (`CreatedBy::{User, Execution, TaskRun}`).
- `UNIQUE (workspace_id, namespace, key, revision)`, whose index also serves the latest row; a partial index on `expires_at` where `expires_at IS NOT NULL` for the sweep. *(2026-10-07, the user's ruling: the unique constraint's index leads with `(workspace_id, namespace, key)` and is scanned backwards for the latest row, as W9b's is. A separate index on `(workspace_id, namespace, key, revision DESC)` would duplicate it, so none is created.)*

The `deleted_by_*` columns are beyond the column list the user named: they let a `Delete` re-claimed after an outage recognise its own earlier delete (§8.4, §11). **A KV value exists only once its row exists.** The latest row of a key is its highest revision; that row is the key's state.

### 4.2 Values and types

- **Encoding.** egret writes each value as Kestra does: a DURATION as its raw ISO-8601 string bytes, every other type as Ion **text**. The exact bytes per type are **measured in Task 1** (STRING, NUMBER (integer, decimal), BOOLEAN, DATETIME, DATE, DURATION, JSON object and array), and egret's encoder is tested against those bytes, so a bucket written by egret reads like Kestra's. The plan picks the Ion crate. *(Measured 2026-10-07, Task 1: every DURATION is an Ion string, `"P1D"`. A Double is Ion float text (`1.5e0`, `1e0`, `1.0e20`), an integer Ion int text, an API decimal an Ion decimal (`1.5`). DATETIME is an Ion timestamp in UTC at millisecond precision, DATE a day-precision timestamp, JSON compact Ion text with field names unquoted when they are identifiers (`{b:1,a:"x"}`, `{'a.b':1}`). The fixture's `types` cells hold the bytes.)*
- **Type.** The type is stored in `value_type`: the `kvType` given, or, without one, the type of the rendered typed value (string → STRING, number → NUMBER, boolean → BOOLEAN, object or array → JSON). The API's `PUT` infers it from the parsed Ion value as `KVType.from` would (§9). A read returns the value typed by `value_type`, never by re-inference (§2). *(Amended 2026-10-07, measured in Task 1: `kvType` coerces only a String; a number, boolean, map or list is stored as itself whatever `kvType` says. So the stored type is DURATION when `kvType` is DURATION, and otherwise the type of the value after coercion, as `KVType.from` would name it (a JSON `null` is STRING).)*
- **Reading back.** `kv()` and `Get` return STRING, NUMBER, BOOLEAN and JSON as the matching JSON value. DATETIME, DATE and DURATION have no JSON type in egret's render; they come back as the strings Kestra's `kv()` prints for them, which are **measured in Task 1** (for example, whether a DURATION written as `P1D` prints as `P1D` or `PT24H`). A difference the fixture shows is a §2 row. *(Measured 2026-10-07, Task 1: DATETIME prints as `Instant.toString` (`2025-01-02T03:04:05Z`) and DATE as `2025-01-02`. A DURATION whose text matches Kestra's duration pattern prints as `java.time.Duration` prints it (`P1D` → `PT24H`); any other prints as written. A null inside a JSON map is dropped by every Kestra read, and egret reproduces that. §2 lists the differences.)*
- **Size** *(the user's ruling, 2026-10-07)*. The encoded object is at most `max_read_bytes` (10 MiB by default), on every write path: `Set`, `Put` (its merged result) and the API's `PUT`. A larger value is an author error naming `max_read_bytes` (413 on the API), and nothing is written. A read that decodes the value into memory (`kv()`, `Get`, `Put`'s merge, the API's `GET`) is bounded by the same cap, so every stored value is readable. `max_file_bytes` does not apply to KV values. A read still checks the bound, and a stored object over it is an author error naming the cap; only an object written before the cap was lowered can trigger it.

### 4.3 Writing

Every write (`Set`, `Put`, the API's `PUT`) has this shape. W9b's lesson holds: **no database connection and no lock is held across a caller-paced body.**

1. **Check** the key (§3), the namespace and the write rule (§5) through `authorize(…, Resource::Kv, Access::Write)`, then render and type the value (a task, §8.1) or parse it (the API, §9). Encode it (§4.2) and check its size against `max_read_bytes`, before anything is staged.
2. **Stage.** Record a staging row (`staging::record`) and put the encoded bytes to its staging object, holding no connection and no lock.
3. **Commit**, on a detached task as W9b's `commit_staged`, so a dropped request or a cancelled future still finishes. In one transaction:
   - take `pg_advisory_xact_lock(hashtextextended('w9c:{ws}:{ns}:{key}', 0))`, under `SET LOCAL lock_timeout` when the grant has a `lock_wait` (the API's 5 s; a flow waits as long as it takes);
   - read the latest row;
   - apply the re-claim skip: when this task run already wrote the latest revision (`created_by_task_run`), answer "already written" and write nothing (§11);
   - apply `overwrite: false`: a live, unexpired latest row is a conflict naming the key; a missing, soft-deleted or expired one is not;
   - set `revision = max + 1` (1 for a new key);
   - copy the staging object onto the revision's key server-side (`ObjectStore::copy`);
   - `head` it, whose size must be the staged size;
   - insert the row;
   - commit.
4. **Forget** the staging object and its row on every way out (written, skipped, refused, failed).

`Put` skips step 2 and differs in step 3: it reads, merges and writes under the lock (§8.5). A `Delete` takes the same lock and updates the latest row (§4.4). The lock serialises every writer of one key, and the window it covers is server-paced: one server-side copy, or `Put`'s bounded read and write. *(2026-10-07, the user's ruling.)* These server-paced store calls under the key's lock are **accepted**:
- `Put`'s read and write, each at most `max_read_bytes`;
- the copy and `head` of `Set` and the API's `PUT`;
- the object deletes of `PurgeKV` and the sweep, under the session lock.

Client-paced I/O under a lock stays forbidden: a request body is read and staged before any lock (§9). An API writer still waits at most 5 s for the lock, then answers 409.

### 4.4 Revisions and soft delete

Following W9b's R1:
- Every write makes a new revision; earlier revisions and their objects stay.
- `Delete` flags the latest revision `deleted` (with `deleted_at` and `deleted_by_*`), under the key's lock. No revision is added, as in Kestra.
- A latest read of a soft-deleted key is missing. An explicit-revision read (`GET …/kv/{key}?revision=N`) of a soft-deleted key's revision succeeds, unless that revision has expired (§4.5).
- `GET …/kv/{key}/revisions` lists every revision, deleted ones flagged.
- A write after a delete makes revision `max + 1`, live.
- Revisions and their objects stay until `PurgeKV` or the expiry sweep removes them.

### 4.5 Reading and expiry

- **Live** means the latest row is not deleted and `expires_at` is null or later than the database's `now()`. Every latest read (`kv()`, `Get`, `GetKeys`, the API's list, get and inheritance) answers only from live rows.
- **Expired means missing.** An expired key reads exactly as a key that was never written: `kv()` and `Get` honour `errorOnMissing`, `GetKeys` and the listings leave it out, `Set` with `overwrite: false` may write over it. An explicit-revision read of an expired revision is not-found too.
- **Reads never write.** Nothing on a read path inserts, updates or deletes; the sweep (§11) and `PurgeKV` remove expired keys.
- **Expiry is fixed at write:** `expires_at = now() + ttl`, by the database's clock in the commit's transaction. `Put` keeps the latest revision's `expires_at` and description (Kestra, `Put.java:121`) when that revision is live. An expired key is missing to `Put` as to every reader: `Put` starts fresh, with no description and no expiry (§8.5; the user's ruling, 2026-10-07).
- A read resolves the latest live row, then reads that revision's object (bounded by `max_read_bytes`, §4.2). **A missing object behind a row is an author error naming the key and the revision**, with no fallback (W9b's rule). An unreachable store or database is an infrastructure error (§11).

### 4.6 Inheritance

- `kv(key)` with no `namespace`, and `Get` whose rendered `namespace` equals the running flow's own, look the key up in the flow's namespace and then in each parent, cutting at dots (`a.b.c` → `a.b` → `a`), and answer from the first namespace where the key is live. An expired or soft-deleted key at a level is missing there, and the walk goes on.
- An explicit `kv(key, namespace)` does not walk, even when the namespace is the flow's own (`KvFunction.java:64-66`). `Get` with any other namespace does not walk. `GetKeys`, `Delete`, `Set`, `Put` never walk.
- The walk reads each level through `authorize(Read)`, which any namespace of the workspace passes. One query reads every level's latest row (at most the namespace's segment count, ≤ 75); one object is read.
- When no level has the key, the missing-key message names the namespace Kestra's message names, which Task 1 measures; egret reproduces it.

### 4.7 One access function

W9b's `nsfiles::access::authorize` gains a resource kind: `authorize(workspace, actor, target, resource: Resource::{NamespaceFiles, Kv}, access)`. The rule is the same for both kinds (§5); the resource changes only the refusal's words ("namespace files" or "the KV store"), and the `Grant` records it. Every KV store function takes a `Grant` and asserts its resource and access (`require`), so a namespace-file grant cannot reach KV and a Read grant cannot write. The access rule stays in one module, `nsfiles::access`, for both packages.

## 5. Who may read and write

- **Workspace:** always the running execution's (for the API, the route's `{ws}` after the permission check). Nothing in a property or argument names a workspace.
- **Read** (`kv()`, `Get`, `GetKeys`, the inheritance walk): any namespace of the workspace. A namespace with no KV simply has no keys.
- **Write** (`Set`, `Put`, `Delete`): the running flow's own namespace only, compared exactly.
- **Purge** (`PurgeKV`): the flow's own namespace and its child namespaces (`ns == own`, or `ns` starts with `own + "."`).
- **API:** any namespace of the route's workspace, after `workflow:read` (reads) or `workflow:manage` (writes) (§9).
- A refusal is an author error naming the namespace and the rule; it never says whether a key exists there. Each refusal has a test that fails when it is removed.

## 6. The `kv()` function

- **Signature as Kestra's:** `kv(key, namespace?, errorOnMissing?)`, positional or named (`KvFunction.java:34-36`). `errorOnMissing` defaults to **true** (`:53`). A missing `key` is the error `The 'kv' function expects an argument 'key'.`
- **Lookup:** without `namespace`, the flow's namespace with inheritance; with one, that namespace only (§4.6). The value is returned typed (§4.2). Missing (never written, soft-deleted, expired) with `errorOnMissing: true` is an error carrying Kestra's message as Task 1 measures it; with `false`, `null`.
- **Connection:** the render's one namespace connection (`storage/scope.rs`: opened on the first namespace lookup from `nsfiles::db_options()`, closed when the render ends), shared with W9b's file functions. A failed connect or query sets the scope's `infrastructure_failure` flag, so the run is re-claimed with no attempt spent (§11). The lookup and the object read go through W9a's non-wedging bounded read (`off_the_runtime`).
- **No snapshot.** KV commits outside `settle`, like namespace files, so a render sees every committed row, and a key assembled at render works.
- **Where:** every render with an execution, including `runIf`, `If`/`Switch` conditions, `Log` and task properties. A render with none (a trigger's conditions, publish) refuses `kv()` by name, as W9b's functions. Whether that matches the expression oracle's `function-kv` cell is **measured in Task 1**.
- `kv` leaves `REFUSED` (`functions.rs:276`), which shrinks to six; `kv_still_names_w9c_and_the_refused_list_shrinks_to_seven` is replaced by a test that pins six names and a test that proves a read.

## 7. Secrets in KV values

- **At publish**, a `Set` or `Put` whose `value` calls `secret(…)` anywhere in its templates is refused with a new code, `secret_in_kv_value`. The message names the task and the property and says: *the secret's value would be stored in the KV store, where anyone who can read this namespace's KV (`workflow:read`) could read it, unmasked; pass the secret to the task that needs it instead.* It is reported **instead of** the generic `CODE_SECRET_OUTSIDE_PLUGIN_TASK` for that property, so the author reads the reason that applies.
- The scan is publish's existing `secret()` scan (`spec/validate.rs:395-403`), pointed at the two tasks' `value`. The refusal stands on its own: it holds even if `secret()` is later allowed outside `kestra.*` tasks.
- The API's `PUT` is unaffected: its body is a literal value the caller already holds.
- A value that reaches a secret indirectly (a variable or an output that holds one) is not detectable at publish; at run, `secret()` is still confined (`SECRET_REFUSAL`), so no rendered secret reaches a KV value through egret's renders.

## 8. The `io.kestra.plugin.core.kv` tasks

Native handlers in the worker, like W9b's `core.namespace` tasks; each gets a row in `spec/registry.rs`. Properties and outputs follow the artifact's classes; Task 1 extracts each schema and measures each behaviour. `namespace` defaults to the flow's own on every task.

### 8.1 `Set`

- `key` (required, the key rule); `value` (required); `namespace` (write rule, §5); `overwrite` (default true); `ttl` (ISO-8601 duration; a negative one is an author error, and zero's behaviour is **measured in Task 1**); `kvType` (one of the seven types); `kvDescription`.
- **Typed render**, as Kestra's `renderTyped` (`Set.java:111`): a value that is a single expression keeps its rendered type (`{{ 1 + 1 }}` is a NUMBER); any other value is a string. Which templates come back typed is **measured in Task 1**.
- **Coercion** of a String by `kvType`, as the artifact: NUMBER as a JSON number; BOOLEAN strict, `true`/`false` case-insensitively, anything else the artifact's message; DATETIME as an instant; DATE as a local date, falling back to an instant's UTC date; DURATION validated and kept as written; JSON parsed. A Number with `kvType: STRING` becomes its string. The other mismatches (a Boolean with `kvType: NUMBER`, a JSON value with `STRING`) are **measured in Task 1** and reproduced. A failed coercion is an author error carrying the measured message.
- *(Measured 2026-10-07, Task 1: a `ttl` of zero or a negative one fails with `ttl: must be a positive duration`. `kvType` coerces only a String (§4.2). `value` is a string property, so a YAML map, list, number or boolean reaches `renderTyped` as text and is stored as a STRING (`{"a":"2","b":"x"}`). The messages are in the fixture's `set_coercion` and `ttl` cells.)*
- The encoded value is at most `max_read_bytes`; a larger one fails the task, naming the cap, before anything is staged (§4.2).
- Written by §4.3; `overwrite: false` over a live key fails the task, naming the key (the message is Kestra's, **measured in Task 1**). Output: none, as Kestra's `VoidOutput`.

### 8.2 `Get`

`key`, `namespace` (read rule; inheritance when it equals the flow's, §4.6), `errorOnMissing` (default false). Output `{value}`: the typed value, or `null` when missing and `errorOnMissing` is false; missing with `true` fails the task with Kestra's message (`Get.java:91`).

### 8.3 `GetKeys`

`prefix` (optional; a plain `startsWith` on the key), `namespace` (read rule; no inheritance). Output `{keys}`: the live keys, ordered as Task 1 measures Kestra's order. At most `max_namespace_files` keys; more fails the task naming the cap and the count, before any output. The listing reads rows only, never an object. *(Measured 2026-10-07, Task 1: Kestra's order is not by key. It lists keys in the order their latest revision was written: its query has no ORDER BY, and a rewrite moves a key to the end. egret orders by the latest revision's row id, ascending, which reproduces it.)*

### 8.4 `Delete`

`key`, `namespace` (write rule), `errorOnMissing` (default false). Under the key's lock, a live latest row is flagged deleted (§4.4) and the output is `{deleted: true}`; a missing or expired key outputs `{deleted: false}`, or fails with Kestra's message when `errorOnMissing` is true. **Re-claim:** when the latest row is already deleted by this task run (`deleted_by_task_run`), the output is `{deleted: true}` and nothing is written (§11).

### 8.5 `Put`

- `key`, `value` (any type), `namespace` (write rule), `errorOnMissing` (default false).
- **The merge rules** follow the artifact (`Put.java:72-121`): a map deep-merges into a map; a list of `{key, value}` entries patches dotted paths; a single `[{value}]` entry replaces the whole value. The exact semantics are **measured in Task 1** and pinned by fixture cells: nested maps, arrays inside maps, a type clash between a map and a scalar, a missing key with each `errorOnMissing`, and a dotted path through a missing level. egret implements against the fixture.
- *(Measured 2026-10-07, Task 1: on the artifact a flow's `value` reaches `Put` as JSON text, so `Put` stores a STRING and never merges (§2). The fixture's merge semantics come from the artifact's `Put.mergeValues`, run on typed values. Maps merge recursively. A list inside a map is appended to. A null in the patch leaves the field as it was. A scalar replaces a map, and a map a scalar. A dotted entry builds missing levels and replaces a scalar on its path. A keyed entry over a non-map value fails with ``Cannot apply keyed `value` updates to a non-JSON existing KV value.`` Whether egret merges or follows the artifact awaits the user's ruling.)*
- **Under the key's lock.** `Put` runs §4.3 step 1 for its patch and stages nothing, since the merged bytes exist only under the lock. Then, in the commit transaction, holding the lock:
  - read the latest live row, and its object (bounded by `max_read_bytes`). A key that was never written, is soft-deleted or has **expired** is missing: `errorOnMissing` decides, and a write starts fresh (the user's ruling, 2026-10-07; §4.5);
  - merge;
  - check the merged result's encoded size against `max_read_bytes`: a larger result fails the task, naming the cap, and nothing is written;
  - write the merged object directly to the revision's key;
  - `head` it;
  - insert the row, keeping the latest live revision's description and `expires_at` (a missing key carries over neither), with the type of the merged value;
  - commit.

  The lock covers one bounded, server-paced store read and one store write, never a caller's body.
- **Why the merge runs under the lock** rather than optimistically outside it: with a retry loop, a busy key would fail a flow for another writer's timing, which W9b rules out for flows. The re-claim skip applies as for `Set`. Output: none.

### 8.6 `PurgeKV`

- **Namespaces:** `namespaces` xor `namespacePattern` (Kestra's wildcard grammar, W9b's `glob` Commons-IO matcher); the default is the flow's own namespace. `includeChildNamespaces` defaults to true and matches on a dot boundary. The candidates are only the namespaces holding KV that are the flow's own or its children, so a pattern never reaches another namespace. A listed namespace outside the purge rule is refused by name before anything is purged. At most 64 namespaces are listed or covered (W9b's `MAX_NAMESPACES`).
- **`keyPattern`:** the same wildcard grammar, on the key.
- **`behavior`:**
  - `{type: key, expiredOnly: true}` (the default) purges every revision of each matched key whose latest revision has expired.
  - `{type: key, expiredOnly: false}` is refused. The error names `behavior: {type: version, keepAmount: 0}` as the form that purges every revision.
  - `{type: version, keepAmount, before}` keeps the `keepAmount` highest revisions of each matched key, and purges revisions created at or before `before`, never the latest. A soft-deleted key is purged entirely (W9b's rule). `keepAmount: 0` purges every revision. A `version` behaviour with neither property is refused, naming both and saying that `keepAmount: 0` purges everything.
  - *(Measured 2026-10-07, Task 1: Kestra refuses `before` together with `keepAmount` (`behavior: Cannot set both 'before' and 'keepAmount' properties`), and egret refuses it too. A version purge with neither property purges every revision on Kestra.)*
- **Execution, as W9b's `PurgeFiles`:**
  - Rows are read in keyset pages of `max_namespace_files + 1` by `(key, revision)`.
  - Each page's complete keys are purged per key, under the key's session lock (`LockedSession`). *(2026-10-07, the user's ruling.)* Under that lock the key's rows are **re-read and judged as they are then**, as the sweep does (§11), so a key rewritten since the listing is judged on its new rows. Each purge deletes the rows by id, commits, then deletes the objects, then unlocks. The lock is still held when the objects are deleted, so a writer cannot reuse a freed revision number's object key in between.
  - A key holding more revisions than the cap fails the task, naming it, with the keys before it purged.
  - The purge runs to completion on a detached task.
- **Output** `{size}`: the count Kestra reports, revisions or keys, **measured in Task 1**. *(Measured 2026-10-07, Task 1: revisions, not keys.)*

## 9. The KV API

Under `/api/ws/{ws}/workflows/namespaces/{ns}/kv`, mirroring `KVController`, with W9b's route conventions (`WsContext`, then `egret_core::authz::require` with `WF_READ` or `WF_MANAGE`, then `authorize(…, Actor::Api, …)`; `no_store()` answers 503 when `[storage]` is absent; each write emits one audit mutation; no new permission). *(2026-10-07, the user's ruling on the audit's spelling, W9b's form.)* The action is `workflow_namespace_kv:{put|delete|delete_many}` and the resource type `workflow_namespace_kv`. The resource id is `{ns}/{key}`, or `{ns}` for a bulk delete, which lists the keys in its detail.

| route | permission | answer |
|---|---|---|
| `GET …/kv` | `workflow:read` | the namespace's live keys, each with `key`, `type`, `size`, `description`, `expiresAt`, `revision`, `updated`; at most `max_namespace_files`, else 413 |
| `GET …/kv/inheritance` | `workflow:read` | the ancestors' live keys only, deepest first, the first occurrence of a key winning (Kestra, `:73-100`), each with its namespace; the same bound |
| `GET …/kv/{key}?revision` | `workflow:read` | `{type, value, revision, updated, description, expiresAt}`, the value typed (§4.2); a latest read of a missing, soft-deleted or expired key is 404; a given `revision` of a soft-deleted key is served, an expired one is 404 (§4.4, §4.5) |
| `GET …/kv/{key}/revisions` | `workflow:read` | every revision: `revision`, `type`, `size`, `description`, `expiresAt`, `created`, `deleted`, `expired`; 404 when the key has no row |
| `PUT …/kv/{key}` | `workflow:manage` | body `text/plain`, parsed as Ion and falling back to the raw string (Kestra, `:124-141`); headers `description` and `ttl` (ISO-8601); always overwrites; §4.3; 200 with no body |
| `DELETE …/kv/{key}` | `workflow:manage` | §4.4; `true` when a live key was deleted, `false` when there was none (200 either way) |
| `DELETE …/kv` | `workflow:manage` | body `{keys: […]}`, at most **1,000** keys (the user's ruling, 2026-10-07; more is 413 naming the cap); one transaction taking each key's lock in ascending order, all or nothing; `{keys: […]}` lists the keys that were live and are now deleted |

- **The `PUT` body** is bounded by `max_read_bytes` (it is parsed in memory to type it, §4.2), and so is the encoded value (the user's ruling, 2026-10-07). It is read with W9b's body timeouts: no byte for 30 seconds, or more than 12 hours in all, answers 408 and writes nothing. The body is read and staged before any lock is taken.
- *(Measured 2026-10-07, Task 1: Kestra keeps the first value of an Ion stream, so `hello world` is stored as `"hello"`. This spec keeps the raw-string fallback until the user rules (§2). A timestamp at exactly midnight UTC is stored as a DATE. The fixture's `api` cells record the rest.)*
- **Lock wait:** an API write waits at most 5 s (`API_LOCK_WAIT`) for the key's lock, then answers 409 "the key is busy (another write holds it); try again", naming the key, holding nothing.
- **Errors → status** (one mapping for every route, W9b's `ns_refusal` with KV's errors):

| error | status |
|---|---|
| the key rule, a refused namespace (invalid, empty segment, `_files` or `_kv` segment), a malformed or negative `ttl`, a non-integer `revision`, a malformed bulk body | 400 |
| no live row: an unknown key, a latest read of a soft-deleted key, an expired key or revision, an unknown revision, a key or `revisions` read in another workspace's namespace (as in an empty one) | 404 |
| the key's lock held longer than 5 s | 409 |
| a `PUT` body or its encoded value over `max_read_bytes`, a stored value over `max_read_bytes` on `GET`, a listing over `max_namespace_files`, a bulk body over 1,000 keys | 413 |
| a body that stalls for 30 s or takes over 12 h | 408 |
| the store or the database unreachable | 503, logged with a fixed message; the database's error is never sent |

No message names a storage key or another workspace. A namespace another workspace holds answers exactly as a namespace that never held anything. *(2026-10-07, the user's ruling.)* The listings (`GET …/kv`, `…/kv/inheritance`) answer `200 []`, and a key read or `revisions` answers 404, byte for byte as for an empty namespace (§13).

**A key named `inheritance`** is legal, but `GET …/kv/inheritance` is the inheritance route, so its value cannot be read by `GET …/kv/{key}`. This is Kestra's collision too (`KVController.java:73`, `:103`). Its `PUT`, `DELETE` and `…/revisions` work, and the tasks and `kv()` read it (§15; the user's ruling, 2026-10-07). **No GET route writes** (§4.5). utoipa documents every route; the frontend client is regenerated with `npm run gen:api`. The UI is W7's.

## 10. The plugin host

- No KV bridge: `ShimRunContext.namespaceKv` keeps throwing `EgretUnsupported`, with a new message: *namespaceKv(): egret gives flows the KV store through the `io.kestra.plugin.core.kv` tasks and the `kv()` function, not through a plugin's RunContext; use a `core.kv` task before or after this one.* `EgretPluginHostTest.java:125-126` pins the new message, and the message at `ShimRunContext.java:341` and the doc mentions (`EgretUnsupported.java:4`, `EgretPluginHost.java:58`) drop "W9c". *(Corrected 2026-10-07: this sentence first cited `ShimRunContext.java:52`, whose class doc names no package and stays unchanged.)*
- The handler's exit-4 message (`worker/handlers/kestra.rs:379`) drops "the KV store — W9c": *the plugin needs a Kestra service egret does not provide to plugins*, followed by the host's reason, which names `namespaceKv` and the `core.kv` tasks. The module doc (`:79`) and the pinning test (`:1317-1321`) follow.
- Nothing is staged for KV, so `task.json` and the task-bytes check (`check_task_bytes`) are unchanged.

## 11. Lifecycle and failure classification

- **The expiry sweep.** Every egret process with a store starts `spawn_kv_expiry_sweeper` beside W9b's staging sweeper. It runs hourly, the first pass after an hour, over every workspace. *(2026-10-07, the user's ruling.)* It uses `interval_at(now + every, every)`, not `interval`, whose first tick is immediate (W9b's staging sweeper uses `interval`):
  - It lists, in keyset pages of 1000 by row id, the keys whose latest row has `expires_at <= now()` (the partial index).
  - For each key, it takes the key's session lock with `pg_try_advisory_lock` and skips the key when it is busy; the next pass retries.
  - Under the lock it re-reads the latest row. If that row is still expired, it deletes the key's rows by the ids it read, commits, deletes the objects, then unlocks.
  - A key rewritten in between keeps its new revision.
  - Two replicas sweeping at once are harmless: a busy key is skipped, and a delete by id of a row already gone is a no-op.
  - A failed object delete is logged; its row is already gone, so the object is orphaned (§15).
  - Age is the database's clock.
- **Re-claims are idempotent.** After an infrastructure failure the same task run is re-claimed.
  - `Set` and `Put` skip when this task run wrote the latest revision (`created_by_task_run`, judged under the key's lock, §4.3), so a retry adds no revision and `overwrite: false` never conflicts with its own write.
  - `Delete` answers `deleted: true` when this task run deleted the latest revision (§8.4).
  - `PurgeKV` recomputes from the rows as they are; its `size` counts what the re-claim purged itself.
  - The skip keys on the task run, so it also covers a `RETRY_FAILED_TASK` retry, as W9b's does.
- **A flow's write waits** for a busy key as long as it takes (W9b's rule): what it waits behind is server-paced, either one copy or one `Put`'s bounded read and write. An API write waits 5 s (§9).
- **Failure classification** (KV reuses W9b's `NsError` and its split):

| failure | class | effect |
|---|---|---|
| the key rule, a refused namespace, a write-scope refusal, a malformed `ttl`, a failed `kvType` coercion, a `Put` shape the merge refuses, `expiredOnly: false`, a `version` purge with neither property, a cap crossed (`max_read_bytes`, `max_namespace_files`, 64 namespaces), `overwrite: false` over a live key, missing with `errorOnMissing: true`, a missing object behind a row | author (`Author`, `NotFound`, `Conflict`, `TooLarge`) | the task fails with a message naming the key, the namespace or the cap; the API answers 4xx |
| `secret()` in a `Set`/`Put` value | author, at publish | the flow is not published (§7) |
| the store or the database unreachable, at run or at render | infrastructure (`Store`, `Db`) | the handler returns `Err`, or the render sets `infrastructure_failure`; the run is re-claimed with no attempt spent; the API answers 503 |

No error names a secret, a storage key or another workspace.

## 12. Tasks

1. **Measure Kestra** into `oracle-exec/w9c-kv.json`, with W9b's contract format: egret's answer at the top level and Kestra's under `kestra`, with `chosen_cells` where egret deliberately differs and the premises each cell rests on. Cells:
   - each task's schema (properties, defaults, required, outputs);
   - value-type round-trips through `Set` → `kv()`/`Get`/API for every type, DATE, DATETIME, DURATION and JSON included, and the Ion bytes stored;
   - `Set` coercions and their messages (strict BOOLEAN, the mismatches of §8.1), the typed render, `ttl` zero and negative;
   - `Put`'s merge (§8.5);
   - TTL: reads of an expired key by each reader with each `errorOnMissing`, and what the row and object look like afterwards;
   - inheritance: `kv()` with and without `namespace`, `Get` with the flow's namespace, an expired level, and the missing-key messages;
   - `PurgeKV`'s behaviours, `size`, and a `version` purge with neither property;
   - the API's routes and bodies (`KvDetail`, the PUT's Ion parsing and fallback, both deletes, `inheritance`);
   - the storage tree after writes, a delete, a rewrite and a purge;
   - the count and list of flows and corpus entries with a `_kv` namespace segment;
   - `function-kv` in the expression oracle;
   - the baselines (tests, clippy, the ratchet at 108).
2. **Table, access, reads:** migration 0175, `check_key`, the `_kv` segment rule (publish and `check_namespace`), `Resource::Kv` in `authorize`, the store's latest-live read, the inheritance walk, listings, revisions.
3. **Writes, staging, purge, sweep:** the §4.3 write over W9b's staging, soft delete, `Put`'s locked merge, the per-key purge by row id with keyset paging, `spawn_kv_expiry_sweeper`; the concurrency tests.
4. **`kv()` and the secret refusal:** the function over the render's connection, `REFUSED` to six, `secret_in_kv_value` at publish.
5. **The six tasks:** `Set`, `Get`, `GetKeys`, `Delete`, `Put`, `PurgeKV`, with their registry rows, against the fixture.
6. **The KV API:** routes, the error mapping, utoipa, `npm run gen:api`.
7. **The plugin host's messages** (§10).
8. **Rebase, end to end, docs, CI:** rebase onto `origin/main` and re-check the migration number; end to end on a temporary kind cluster; the ratchet before and after; the programme spec's W9 row and a W9c correction; `oracle-exec/README.md`; full local CI.

**Global constraints for every task** (W9b's lessons):
- For every write path, the review checks, and the task report states, that no database connection and no lock is held across a caller-paced body. The server-paced windows (one copy, `Put`'s bounded read and write, a purge's object deletes) are named where they occur.
- Destructive tests (delete, purge, sweep) run only on `MemStore` or under a throwaway prefix on MinIO, never against the shared bucket's root.
- Each branch uses its own Postgres, Valkey and MinIO.
- The ratchet must not regress: 108 mismatching before and after. `retry-with-flowable-errors` also needs the `AllowFailure` flowable, so W9c alone does not bank it, and `valids/kv` has no golden entry.

## 13. Testing

- **Unit:** the key rule (each refusal with a test that fails when it is removed, 350 accepted and 351 refused); the `_kv` and `_files` segment rules; `authorize` with `Resource::Kv` (own write, other-namespace read, other-namespace write refused, child purge, `a.bc` not a child of `a.b`, a namespace-file grant refused by a KV store function); Ion encoding against the fixture's bytes per type; `Set` coercions; `Put` merges against the fixture's cells; the inheritance walk's order.
- **Postgres:**
  - Two concurrent `Set`s with `overwrite: false` on one key: exactly one writes.
  - Two concurrent `Put`s merging different fields: both fields survive, in two revisions.
  - Revision numbering under concurrent writers.
  - Soft delete then rewrite; explicit-revision reads.
  - Expiry: a read past `expires_at` is missing and writes nothing (the row count and object list are unchanged).
  - The sweep removes an expired key and keeps a key rewritten between listing and lock.
  - Two sweeps in parallel.
  - `PurgeKV` per behaviour, the refusals, and paging past one page.
  - Re-claim skips for `Set`, `Put`, `Delete`.
  - Workspace isolation.
- **Destructive tests** on `MemStore` or a throwaway MinIO prefix only (§12).
- **Render:** `kv()` in `runIf`, `If`, `Switch` and a task property; an outage sets `infrastructure_failure` and re-claims; a render with no execution refuses it by name.
- **Publish:** `secret()` in `Set` and `Put` values refused with `secret_in_kv_value` and its message; a `_kv` namespace refused; `valids/kv.yaml` still publishes.
- **API:** `workflow:read` versus `workflow:manage`; every row of the status table; the 5 s busy 409; the body bounds and the 408s; a bulk delete of 1,000 keys accepted and 1,001 refused with 413; another workspace's namespace byte-identical to an empty namespace's (listings `200 []`, key reads 404).
- **Plugin host:** the new `namespaceKv` and exit-4 messages, pinned.
- **Oracle:** the fixture's egret cells re-run against the build; the ratchet measured before and after and recorded in `oracle-exec/README.md`.
- **End to end** on a temporary kind cluster: a flow that `Set`s, reads with `kv()` and `Get`, `Put`s, `Delete`s and purges; a TTL that expires and is swept; the API from outside.

## 14. Out of scope

A KV bridge for plugins (§10); a cross-namespace paged listing (`GET /kv` across namespaces); namespace deletion and namespace-level permissions; the UI (W7); `AllowFailure` (and so banking `retry-with-flowable-errors`); masking KV values in logs or outputs.

## 15. Known limits

- A flow writes and deletes only its own namespace's KV; a Kestra flow that `Set`s another namespace's key fails on egret, naming the namespace (§5).
- `PurgeKV` with no `namespaces` purges the flow's own namespace and its children, not every namespace; `expiredOnly: false` is refused in favour of `version` with `keepAmount: 0` (§8.6).
- `secret()` in a `Set` or `Put` value is refused at publish (§7).
- A KV value is at most `max_read_bytes` (10 MiB by default), where Kestra has no cap; a `Set`, `Put` or API `PUT` of a larger value fails, naming the cap (§4.2; the user's ruling, 2026-10-07).
- DATETIME, DATE and DURATION values come back as strings in egret's renders, so a template comparing them compares strings (§4.2).
- The read-increment-`Set` pattern (`retry-with-flowable-errors`) is racy, as in Kestra; `Put` is the race-free form for a map, and there is no atomic increment.
- An expired key keeps its rows and objects until the next sweep, up to about an hour; reads already treat it as missing (§4.5).
- The sweep's failed object delete leaves an orphaned object, which nothing lists afterwards, as W9a's and W9b's purges do.
- Deleting a workspace cascades its `workflow.kv_entries` rows away and leaves its `_kv` objects in storage, as W9b's namespace files are left.
- At render, every database error of a `kv()` lookup is an outage, re-claimed with no attempt spent, and a lookup is bounded only by the OS's TCP timeout; this is W9b's render limit, shared (W9b §15).
- A namespace with a `_kv` segment is refused (§3).
- A key named `inheritance` cannot be read by the API's `GET …/kv/{key}`: that path is the inheritance listing, as in Kestra. Its write, delete and revisions routes work, and so do the tasks and `kv()` (§9; the user's ruling, 2026-10-07).
- The API's bulk delete takes at most 1,000 keys per request (413 past it); a larger set takes several requests, each all or nothing (§9; the user's ruling, 2026-10-07).
- Server-paced store calls run under a key's lock, and a writer of that key waits behind them:
  - `Put`'s bounded read and write;
  - the copy and `head` of `Set` and the API's `PUT`;
  - `PurgeKV`'s and the sweep's object deletes.

  A flow waits as long as it takes; an API writer waits 5 s, then gets 409. Accepted by the user's ruling, 2026-10-07 (§4.3).
