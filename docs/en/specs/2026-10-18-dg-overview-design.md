# Data Governance frontend redesign #3: Governance Overview — design

Date: 2026-10-18. Status: draft on the programme frame (decisions F1–F12 of
`docs/en/specs/2026-10-16-dg-frontend-redesign-design.md`, "programme spec" below). Base: main `4a67321b` (#2 People &
groups merged as #690, the group-name/role-mint package as #692). Branch `feat/dg-overview`, worktree `egret-dgov`.

Part 1 of the programme spec gives this sub-project one row: **Overview — what needs attention — new.** This document
specifies it. Every claim about existing code carries a `file:line` from `4a67321b`.

The scope was set by the user on 2026-10-18: the signals egret already computes, plus engine drift (plugins that do not
hold what egret serves), plus a governance audit trail. Recent changes are served by a new route that returns
governance events only, gated `policy:read` OR `tag:read`; the full audit log stays `audit:read`
(`crates/egret-controlplane/src/audit.rs:80`).

## Premises in the brief that the code contradicts

The brief was checked against the code. These statements in it were wrong or incomplete, and the design follows the
code:

1. **"Emit inside the same transaction as the write where the existing store allows."** The existing store never
   allows it. Every mutation event today goes through `AuditSink::emit`, a bounded `mpsc` channel drained by a batch
   writer (`crates/egret-audit/src/lib.rs:53-71`, `:217-256`). `emit` is called **after** the write commits
   (`crates/egret-controlplane/src/access/mod.rs:466`, `crates/egret-bi/src/catalog_tags.rs:683`), drops the event when
   the channel is full (`lib.rs:64-69`), and drops every event when `audit.enabled = false`, because the receiver is
   then never drained (`crates/egret-bin/src/main.rs:1201-1210`). A transactional write needs a new helper (§3.1).
2. **`egret-auth/src/pdp/audit.rs` is not where mutations are recorded.** It is the PDP decorator that writes one
   `decision` row per authorisation (`crates/egret-auth/src/pdp/audit.rs:27-37`). Mutations share the table,
   `audit.audit_log` (`migrations/0076_audit_log.sql:5-17`), as `kind = 'mutation'`. Decisions therefore dominate the
   table, which matters for the feed's query (§3.4).
3. **"Rows deleted after 24 h" (`policy.ranger_plugin_downloads`).** Silent rows are pruned only opportunistically:
   one upsert in 64 *for the same service* deletes that service's rows older than 24 h
   (`crates/egret-policy/src/plugin_downloads.rs:96-108`, `:113-128`). A service whose plugins all stopped polling
   gets no upsert, so its stale rows stay. The drift rules (§4.3) therefore read `at`, never row presence.
4. **`ranger_plugin_version_modelled` is not stored.** It is computed per request in Rust from the engine's type,
   origin, declared version or pinned image (`plugin_report`, `crates/egret-engine-mgmt/src/engines.rs:335-371`). It
   cannot be read with SQL; §4.4 moves the function to `egret-perm` so the Overview can call it.
5. **The tag access-policy PUT is not at `tags.rs:243`.** It is `set_access_policy`
   (`crates/egret-controlplane/src/tags.rs:285`), and it has a sibling, `DELETE` `clear_access_policy` (`:334`); both
   are audited here. The write itself is `ClassificationPolicyRepo::set_policy`/`clear_policy`, which open their own
   transaction (`crates/egret-bi/src/tags.rs:514-563`, `:572-586`).
6. **Smaller citations.** `/perm/unresolved-principals` and `/perm/overlaps` are routes in `routes.rs` (`:1687`,
   `:1656`); `principal_refs.rs:377` and `overlap.rs:432` are the DTOs, and the store functions are
   `unresolved_principals_via` (`principal_refs.rs:447`) and `stored_overlaps_via` (`overlap.rs:478`). The Overview
   reservation comment is `frontend/src/components/layout/Sidebar.tsx:83-85`.
7. **The sidebar heading cannot be "Overview".** Control Plane already has a group headed "Overview"
   (`Sidebar.tsx:119`), and collapsed-group state is one `localStorage` key across all three apps, so headings must
   be distinct (`Sidebar.tsx:79-80`, `:157-158`).
8. **Existing role and catalog-tag events are not directly usable as written.** Role events carry the role's id, not
   its name (`access/mod.rs:466-477`). `tag:assign`/`tag:unassign` are emitted both for catalog columns
   (`resource_type = 'catalog'`, `catalog_tags.rs:683-692`) and for workspace BI objects
   (`resource_type = 'object'`, `crates/egret-bi/src/shares.rs:615-622`); only the first is governance.
9. **Compile failures mostly leave no trace to persist.** Almost every compile runs on the caller's transaction and
   a failure rolls the caller's write back (`recompile_services_in_tx`, `crates/egret-perm-compile/src/trigger.rs:1902-1945`).
   The profile then still holds its previous compile; nothing failed *on the engine*. §3.3 records failures only
   where no write is refused with them.

## Decisions

| id | decision | by |
|---|---|---|
| O1 | Scope: existing signals + engine drift + governance audit. Excluded: the view-exposure DEFINER audit (it dials engines; `crates/egret-controlplane/src/view_exposure.rs`), linked only; classification coverage | user, 2026-10-18 |
| O2 | `GET /api/control-plane/governance/changes` returns governance events only, gated `policy:read` OR `tag:read`; each event family is returned only to a reader holding its family's permission (§3.4). `/audit` keeps `audit:read` | user, 2026-10-18 (gate); default decision (per-family filter) |
| O3 | Governance events are written **in the write's own transaction** by a new `egret_audit::governance::record_change`, into `audit.audit_log` as `kind = 'mutation'`. A refused, rolled-back, previewed or dry-run write leaves no event. They are written whether or not `audit.enabled` is set; the flag keeps governing decision rows and the existing sink events, and its doc comment says so | user, 2026-10-18 |
| O4 | The existing role and catalog-tag events stay on the sink (best effort, after commit); the feed reads them and says they are best effort | default decision (2026-10-18) |
| O5 | No backfill. Recording begins when this version is deployed; the feed's empty state and footnote say so | user, 2026-10-18 |
| O6 | `perm.compile_runs` (migration 0196): the last compile per profile. Success is written by the compile funnel in the compile's transaction; failure only by entry points no write waits on (§3.3) | default decision (2026-10-18) |
| O7 | `GET /api/control-plane/governance/attention`, gated `policy:read` OR `tag:read`; each finding computed only for a reader holding its source's permission, otherwise listed in `notChecked` | user, 2026-10-18 |
| O8 | Each source runs in its own savepoint of one `REPEATABLE READ, READ ONLY` transaction with a 5 s `statement_timeout`; a failing source becomes a `source_unavailable` finding | default decision (2026-10-18) |
| O9 | Drift: `plugin_silent` (critical, no poll in 24 h), `plugin_behind` (critical, version not confirmed 90 s after it was set), and an added `plugin_quiet` (warning, no poll in 5 min) so a stopped engine is not invisible for a day | default decision (2026-10-18); thresholds confirmed by Task 1 (`2026-10-18-dg-overview-live.md` §1.1), which also refined the rules: newest poll per profile, `-1` is "not yet reported", a kind with no recent download is not judged (§4.3) |
| O10 | Migration numbers 0196 (`perm.compile_runs`) and 0197 (partial index on audit mutations). F11 holds 0195 | user, 2026-10-18 |
| O11 | Frontend: lazy area `routes/governance/overview/`, the Governance index; the catch-all `*` redirects to `/governance` (the Overview); sidebar group headed **"Governance overview"**, item "Overview", first | default decision (2026-10-18) |
| O12 | No charts. Counts strip, Needs attention (by severity), Recent changes (paged, 25 per page) | user, 2026-10-18 |
| O13 | Out of #3: an audit trail for tag CRUD, profile create/delete and seed, workspace membership; a system actor for sweeps and the workspace-assignment sync; per-engine plugin attribution; caching | default decision (2026-10-18) |

## 1. What exists

- **Audit.** `audit.audit_log(id, ts, kind, actor_user_id, impersonator, workspace_id, action, resource_type,
  resource_id, decision, reasons, detail)` (`migrations/0076_audit_log.sql:5-17`, `0095_impersonations.sql:19`), with
  indexes on `(ts, id)`, `(actor, ts)`, `(action, ts)` (`0076:19-21`). `AuditStore::query` filters one `action` at a
  time (`lib.rs:166-205`). Rows older than `audit.retention_days` (default 90) are pruned hourly, and a retention of 0
  or less means no prune (`crates/egret-config/src/lib.rs:192-216`, `main.rs:1232-1257`). `mutation_for` stamps the
  impersonator (`lib.rs:33-51`).
- **Governance writes and their transactions.**
  - Grants: `create_grant`, `patch_grant` → `write_grant` on one transaction (`crates/egret-perm-compile/src/routes.rs:1531-1550`,
    `:1583-1595`, `:1047`); `delete_grant` (`:1612-1640`). `preview` runs the same `write_grant`/`write_rule` and always
    rolls back (`:1938-1962`).
  - Rules: `create_rule`, `patch_rule`, `delete_rule` (`:1776`, `:1821`, `:1850`) → `write_rule` (`:1314`).
  - Manual compile: `compile` calls the pool-based `recompile_service`, which opens and commits its own transaction
    (`routes.rs:3731-3748`, `trigger.rs:1356-1365`).
  - Migrate and undo: one transaction each; migrate commits only when proved and not a dry run
    (`routes.rs:3838-3871`, `:3888-3898`; `migrate.rs:840`, `:1458`).
  - Hand-written Ranger policies: create and update through `write_policy_checked`'s transaction
    (`crates/egret-policy/src/admin/mod.rs:1348-1427`, callers `:1757`, `:1774`, `:1848`, `:1866`), delete
    (`:1885-1903`), enable/disable (`:1926-1944`).
  - Clearances: `set_policy`/`clear_policy`, own transaction (`crates/egret-bi/src/tags.rs:514-586`), behind
    `tags.rs:285`/`:334` of the control plane.
- **Compile.** Every compile, in every crate, reaches `recompile_locked_service_in_tx` (`trigger.rs:1332-1350`), through
  `recompile_service_in_tx` (`:1321`) or `recompile_services_in_tx` (`:1913`). `compile_desired` (`:373`) returns
  `None` for a profile type the compiler does not target; the funnel then returns an empty summary. `CompileSummary`
  holds `created/updated/deleted/unchanged/skipped` (`:255-262`). The only persisted trace today is
  `perm.compiled.compiled_at` per compiled policy (`migrations/0154_perm_model.sql:111-127`).
- **Signals.**
  - Unresolved refs with `failsOpen` (`principal_refs.rs:395-406`, `:447`), route `routes.rs:1687` (`policy:read`).
  - Stored overlaps (`overlap.rs:478`), route `routes.rs:1656` (`policy:read`). Reads every profile's units in
    process; no engine.
  - Per-profile policy warnings, `classify::profile_warnings` (`classify.rs:912`), route `routes.rs:3799`
    (`policy:read`). No all-profiles total exists.
  - Failed view lineage: `view_lineage_state.state = 'failed'` (`migrations/0187_*.sql`), read by
    `unknown_lineage` (`crates/egret-controlplane/src/governance_lineage.rs:436-450`), whose route is
    `policy:read` OR `tag:read` (`governance_lineage.rs:169`).
  - Catalog attachment apply errors: `registry.engine_catalog_attachments.apply_status = 'failed'`, `apply_error`
    (`migrations/0147_catalog_registry.sql:54-55`), served by `GET /catalogs` under `catalog_registry:read`
    (`crates/egret-engine-mgmt/src/catalogs.rs:305`).
  - Engines: `engine_mgmt.engines.status ∈ provisioning|running|failed|deleting` (`migrations/0145_engine_status_check.sql:12-13`),
    `origin ∈ provisioned|external` with `status` deliberately meaningless for external rows
    (`migrations/0184_engine_external.sql:13-24`); `authz_service_id` binds a profile (`0058`). Engine routes need
    `engine:read`.
  - Plugin ledger: `policy.ranger_plugin_downloads(service_id, plugin_id, kind, last_known_version, served_version,
    at)` (`migrations/0186_plugin_downloads.sql`); only `last_known_version` is evidence a plugin applied a version
    (`plugin_downloads.rs:7-10`). Counters: `authz_services.policy_version`, `tag_version`, and `updated_at`, which every
    version bump sets (`trigger.rs:1238-1240`, `crates/egret-policy/src/store.rs:2051`,
    `crates/egret-catalog-registry/src/tag_version.rs:117`) but other updates may also set.
  - Clearances: `classification_policies(tag_id, required_roles, required_groups)`; both empty means no gate
    (`egret-bi/src/tags.rs:507-509`); listed by `list_tags` under `tag:read` (`controlplane/src/tags.rs:110-139`).
  - Catalogs and their engines: `GET /governance/catalogs` (`governance_browse.rs:124-150`), `policy:read` OR `tag:read`
    via `require_governance_read` (`governance_browse.rs:337-348`).
- **Frontend.** `GovernanceIndex` redirects to `/governance/data` and also serves the catch-all
  (`frontend/src/App.tsx:529-531`, `:538`, `:606`). Data and People are lazy areas with bundle guards (`App.tsx:52`,
  `:61`). `OverlapReport` renders at the bottom of `/governance/policies` (`frontend/src/routes/governance/Policies.tsx:287`);
  per-profile warnings on `/governance/policies/:service` (`ProfileDetailPage.tsx:224`). People's index takes
  `?kind=unmatched` (`people/ByNameRedirect.tsx:59`). Engine and catalog pages are `/control-plane/engines/:id` and
  `/control-plane/catalogs/:id` (`App.tsx:483`, `:486`); `engine:read` and `catalog_registry:read` each admit the
  Control Plane (`frontend/src/auth/appAccess.ts:14-22`).

## 2. The page

`/governance` (index). A `ListPageShell`-framed page (FE-PAGE-1): `h1` "Overview", then three sections.

**Counts strip** (`SummaryStrip`): catalogs · egret grants · egret rules · classifications. A count the reader may
not read is omitted, never shown as 0. "egret grants/rules" because hand-written engine policies are not counted (the
honesty rule of #1/#2: a label never claims more than it counts).

**Needs attention.** Three groups in order Critical, Warning, Info; a group with no finding is not rendered. Each
row: severity word and icon (never colour alone, FE-A11Y-4), a sentence with the count, up to five samples each with
its own link, and the row's "Open" link to where it is fixed. Below the groups:
- `notChecked`: "Not checked for you: engine liveness and plugin drift (needs `engine:read`) …", one line per skipped
  source;
- a fixed "Not checked here" line: views that may expose classified data through DEFINER (link to Classifications),
  classification coverage.

Empty state (content-derived, FE-STATE-4): only when `findings` is empty **and** no finding is
`source_unavailable`, the section shows "Nothing needs attention." with, when `notChecked` is not empty, "among the
checks you can run". A response with only `notChecked` entries therefore never claims a clean bill.

**Recent changes.** A list (FE-PAGE-4 `ui/table`, narrow: one card per row under 640 px): when (relative, with the
absolute time as `title` and in a `<time dateTime>`), who ("Ana" or "Admin as Ana" when impersonated; "a deleted
user" when the actor row is gone), what (the server's sentence), and a link to the object when it still exists
(a deleted grant's event names it and links nowhere). 25 per page, "Show older" with the keyset cursor. Footnote:
"Recorded since <first event's date or 'this version'>. Changes older than N days are pruned." (N from
`retentionDays`; "never pruned" when null). "Role assignments and catalog tags are recorded after they commit and
can be missing if the audit writer was overloaded." Empty: "No governance change has been recorded yet. Recording
began when this version was installed; earlier changes are not listed."

Each section has its own loading (skeleton), error (server message, a retry) and empty state; one failing request
never blanks the page (FE-STATE-1/2/3).

**Links per finding kind** (all built client-side from typed refs, §4.1):

| kind | Open | sample link |
|---|---|---|
| `unresolved_principals_fail_open` | `/governance/people?kind=unmatched` | `/governance/grants/:id` or `/governance/rules/:id` |
| `plugin_silent`, `plugin_behind`, `plugin_quiet` | `/governance/policies/:profile` | `/control-plane/engines/:id` |
| `compile_failed` | `/governance/policies/:profile` | same |
| `engine_failed` | `/control-plane/engines` | `/control-plane/engines/:id` |
| `rule_overlaps` | `/governance/policies` (report at the bottom) | the rule page when the member is a rule |
| `lineage_failed` | first sample | Data object page, `?tab=lineage` |
| `catalog_apply_failed` | `/control-plane/catalogs` | `/control-plane/catalogs/:id` |
| `policy_warnings` | `/governance/policies` | `/governance/policies/:profile` |
| `plugin_unmodelled` | `/control-plane/engines` | `/control-plane/engines/:id` |
| `tags_without_clearance` | `/governance/classifications` | `/governance/classifications/:id` |
| `catalogs_without_engine` | `/governance/data` | Data catalog page |
| `source_unavailable` | none | none |

## 3. Backend: recording

### 3.1 The helper (reusable by F11)

`crates/egret-audit/src/governance.rs`, exported as `egret_audit::governance`:

```rust
/// The action strings. The feed returns exactly these (plus the two legacy families, §3.4).
pub mod action {
    pub const GRANT_CREATE: &str = "perm_grant:create";
    pub const GRANT_UPDATE: &str = "perm_grant:update";
    pub const GRANT_DELETE: &str = "perm_grant:delete";
    pub const RULE_CREATE: &str = "perm_rule:create";
    pub const RULE_UPDATE: &str = "perm_rule:update";
    pub const RULE_DELETE: &str = "perm_rule:delete";
    pub const CLEARANCE_SET: &str = "tag_clearance:set";
    pub const CLEARANCE_CLEAR: &str = "tag_clearance:clear";
    pub const RANGER_POLICY_CREATE: &str = "ranger_policy:create";
    pub const RANGER_POLICY_UPDATE: &str = "ranger_policy:update";
    pub const RANGER_POLICY_DELETE: &str = "ranger_policy:delete";
    pub const RANGER_POLICY_ENABLE: &str = "ranger_policy:enable";
    pub const RANGER_POLICY_DISABLE: &str = "ranger_policy:disable";
    pub const MIGRATION_APPLY: &str = "policy_migration:apply";
    pub const MIGRATION_UNDO: &str = "policy_migration:undo";
    pub const PROFILE_COMPILE: &str = "profile:compile";
    // F11 (engine-scoped grants) adds, in its own package:
    // ENGINE_GRANT_CREATE/UPDATE/DELETE = "engine_grant:create|update|delete", family Policy.
}
/// Which permission reads an event in the feed.
pub enum Family { Policy, Tag }
pub fn family_of(action: &str, resource_type: &str) -> Option<Family>;   // None = not governance
pub const GOVERNANCE_ACTIONS: &[&str];   // every constant above

pub struct Change<'a> {
    pub action: &'static str,          // one of `action::*`
    pub resource_type: &'static str,   // "perm_grant" | "perm_rule" | "tag" | "ranger_policy" | "policy_profile"
    pub resource_id: String,           // grant/rule uuid, tag uuid, policy id, profile id
    pub summary: serde_json::Value,    // { "before": …|null, "after": …|null, …extra }, ≤ 16 KiB serialised
    pub principal: &'a egret_core::Principal,
}
/// INSERT one `kind = 'mutation'` row on the caller's connection (its transaction).
/// Stamps `actor_user_id` and `impersonator` exactly as `mutation_for` does (lib.rs:33-51).
/// A summary over 16 KiB is replaced by `{"truncated": true, "bytes": n}`.
pub async fn record_change(conn: &mut sqlx::PgConnection, c: Change<'_>) -> Result<(), sqlx::Error>;
```

Why in-transaction, not the sink: the feed is the evidence of who changed governance; the sink drops on overflow,
runs after commit, and is off with `audit.enabled = false` (premise 1). In the transaction, an event exists exactly
when its write committed: a 409 overlap, a `preview` (`routes.rs:1946-1959`), a dry-run or unproved migration
(`routes.rs:3855-3862`) leave nothing. Governance writes are low-rate, so one extra INSERT per write costs nothing
measurable. `audit.enabled`'s doc comment (`egret-config/src/lib.rs:193`) is updated: "When false, the PDP is not
wrapped and the best-effort sink drops its events. Governance changes (`egret_audit::governance`) are still recorded
in their write's transaction."

Summaries are small and stable, never whole bodies:
- grant: `{catalog, schema, table, column, effect, actions, include[], exclude[]}` (kind:ref strings), before and after;
- rule: `{name, kind, selector (path or tag key), function, include/exempt[]}`;
- clearance: `{tag_key, required_roles[], required_groups[] (names, resolved at write)}`;
- Ranger policy: `{service, name, policy_type, enabled, resources (keys and values, each list capped at 20)}`;
- migrate: `{service, policies[], created_grants[], created_rules[]}`; undo: `{service, policies[]}`;
- compile: `{service, created, updated, deleted, unchanged, skipped}`.

Each before/after is read in the same transaction before the write. Actor-less writes (the workspace-assignment
sync, sweeps) record nothing (O13).

### 3.2 Where each event is written

| write | where the INSERT goes | event |
|---|---|---|
| create/patch grant | `write_grant`, after the row write, before the recompile; `preview` reaches it too and rolls back | `perm_grant:create`/`update` |
| delete grant | `delete_grant`, after `delete_grant_in_tx` (`crates/egret-perm/src/store.rs:504`) | `perm_grant:delete` |
| create/patch/delete rule | `write_rule`; `delete_rule` | `perm_rule:*` |
| migrate | `migrate_service_policies`, only when `commit` is true, before `tx.commit()` | `policy_migration:apply`, plus one `perm_grant:create` / `perm_rule:create` per object it creates (they go through `write_grant`/`write_rule`): 1 + N events |
| undo | `undo_service_policy_migration`, before commit | `policy_migration:undo`, plus one `perm_grant:delete` / `perm_rule:delete` per object undo removes, symmetric with apply (review I2) |
| manual compile | `compile`, rewritten to open its transaction, call `recompile_service_in_tx`, record, commit | `profile:compile` |
| clearance set/clear | `ClassificationPolicyRepo::set_policy`/`clear_policy`, before commit; both gain an `actor: &Principal` parameter (replacing `by: Option<Uuid>` on `set_policy`, whose value is `actor.user_id`) | `tag_clearance:set`/`clear` |
| Ranger policy create/update | `write_policy_checked`, before commit (it gains the principal) | `ranger_policy:create`/`update` |
| Ranger policy delete/enabled | `delete_policy_handler`, `set_policy_enabled`, before commit | `ranger_policy:delete`/`enable`/`disable` |

### 3.3 `perm.compile_runs` (migration 0196)

```sql
CREATE TABLE perm.compile_runs (
    service_id      bigint PRIMARY KEY REFERENCES policy.authz_services(id) ON DELETE CASCADE,
    outcome         text NOT NULL CHECK (outcome IN ('ok', 'error')),
    attempted_at    timestamptz NOT NULL,
    error           text,
    last_ok_at      timestamptz,
    created         integer NOT NULL DEFAULT 0,
    updated         integer NOT NULL DEFAULT 0,
    deleted         integer NOT NULL DEFAULT 0,
    unchanged       integer NOT NULL DEFAULT 0,
    skipped         integer NOT NULL DEFAULT 0,
    CONSTRAINT compile_runs_error_says_why CHECK ((outcome = 'error') = (error IS NOT NULL))
);
```

- **Success**: `recompile_locked_service_in_tx` upserts `outcome = 'ok'`, `attempted_at = last_ok_at = now()`, the
  summary's counts, `error = NULL`, on the compile's transaction, when `compile_desired` returned a compile. Every
  compile path in every crate reaches this one function (§1), so none is missed, and a compile whose caller rolls
  back leaves no row (nothing changed on the engine). The profile is already locked (`lock_services_in_tx`), so
  upserts for one profile are serialised.
- **Failure**: written only where no write is refused with it, because then the profile stays behind its model with
  nothing else reporting it: `recompile_service` (the pool-based entry point; callers: the manual compile route
  before its rewrite, and `ProfileCompilerImpl::recompile_profile`, `crates/egret-bin/src/main.rs:275-280`) and the
  rewritten manual compile route. After the failed transaction rolls back, `record_compile_failure(pool, service_id,
  &error)` upserts `outcome = 'error'`, `attempted_at = clock_timestamp()`, `error` (first 2 000 chars), keeping
  `last_ok_at` and the counts, `WHERE compile_runs.attempted_at <= EXCLUDED.attempted_at`. The manual compile route
  also records `profile:compile` with `{outcome: "error", error}` in that same transaction.
- **Not recorded as failures**: a compile that fails inside a grant, rule, tag, clearance, membership, policy or
  catalog write. That write is refused with the error (its caller sees it), and the engine keeps the previous compile.
- Migration 0197: `CREATE INDEX audit_log_mutation_ts_idx ON audit.audit_log (ts DESC, id DESC) WHERE kind = 'mutation'`,
  so the feed does not scan decision rows.

### 3.4 `GET /api/control-plane/governance/changes`

`?limit=` (default 25, max 100) `&cursor=<ts_rfc3339>,<id>` → `{ entries, nextCursor, retentionDays }`.

- Gate: `require_governance_read` (`policy:read` OR `tag:read`). Then the reader's families: `Policy` needs
  `policy:read`, `Tag` needs `tag:read`. Families:
  - Policy: `perm_grant:*`, `perm_rule:*`, `ranger_policy:*`, `policy_migration:*`, `profile:compile`, and the legacy
    `role:assign`/`role:unassign` with `resource_type = 'grant'` (role holders are already readable under
    `policy:read`, #2 Q2);
  - Tag: `tag_clearance:*`, and the legacy `tag:assign`/`tag:unassign` **with `resource_type = 'catalog'` only** (the
    `object` rows are workspace BI tags, premise 8).
- SQL: `kind = 'mutation' AND (action, resource_type) IN (allowed pairs) AND (ts, id) < cursor ORDER BY ts DESC,
  id DESC LIMIT n`, served by the 0197 index. The pair list comes from `egret_audit::governance`, the one place F11
  adds its strings.
- Each entry: `{ id, at, actor: {userId, label} | null, impersonator: {userId, label} | null, action, sentence,
  target: Ref | null, targetExists: bool, detail }`. Labels are resolved at read time: user name or email, role
  name by id (the id when deleted), tag name, catalog name from `registry.catalogs.warehouse_id` for a legacy
  catalog-tag event, profile name. `sentence` is built server-side, one function per action, unit-tested
  ("Ana created a grant: allow read on ice.s.t to group g1").
- `retentionDays`: the configured `audit.retention_days` when positive, else `null`. `CpState` gains
  `audit_retention_days: Option<i64>`, set in `egret-bin` from the same `scheduled_retention` decision the prune uses
  (`main.rs:1232-1235`).

## 4. Backend: attention

### 4.1 Route and shape

`GET /api/control-plane/governance/attention` in a new `crates/egret-controlplane/src/governance_overview.rs`
(beside the other `/governance/*` routes, tag `control-plane-governance`), gated `require_governance_read`.

```text
{ generatedAt,
  counts: { catalogs: i64|null, grants: i64|null, rules: i64|null, tags: i64|null },
  findings: [ { kind, severity: "critical"|"warning"|"info", count, sentence,
                sample: [ { label, detail|null, ref: Ref|null } ],   // ≤ 5
                ref: Ref|null, source: string|null } ],                 // source set for source_unavailable
  notChecked: [ { kind, requires: [permission] } ] }
Ref = { type: "grant", id } | { type: "rule", id } | { type: "unmatched" } | { type: "profile", name }
    | { type: "profiles" } | { type: "engine", id, name } | { type: "engines" } | { type: "catalog", id, name }
    | { type: "catalogs" } | { type: "object", catalog, schema, object } | { type: "tag", id, name }
    | { type: "tags" } | { type: "data" } | { type: "data_catalog", name }
```

Findings are ordered critical, warning, info, then by kind. `count` is the true total; `sample` is at most five,
ordered deterministically (named in each source). Permission checks call `egret_core::authz::require` and map
`Forbidden` to "not held"; any other error is a 500 (a PDP failure is not a missing permission).

### 4.2 Execution

One `REPEATABLE READ, READ ONLY` transaction with `SET LOCAL statement_timeout = '5s'` (the pattern of
`list_overlaps`, `routes.rs:1662-1667`). Each source runs in its own savepoint (`tx.begin()`), because in Postgres one
failed statement aborts the transaction: a failing or timed-out source rolls back its savepoint and yields
`{kind: "source_unavailable", severity: "warning", source: <kind>, sentence: "Could not check <what>."}` (the error is logged, not shown);
the other sources continue. Sources call store functions on the transaction, never routes. The plugin ledger, the
engines and the profiles are read in the same snapshot, so drift is judged on one state.

### 4.3 Sources

| kind | severity | permission | rule |
|---|---|---|---|
| `unresolved_principals_fail_open` | critical | `policy:read` | `unresolved_principals_via` rows with `fails_open`; sample ordered as the report |
| `plugin_silent` | critical | `policy:read` + `engine:read` | profile bound to ≥ 1 engine with `origin = 'provisioned' AND status = 'running'`, and no `policies` row with `at >= now() - 24 h` |
| `plugin_behind` | critical | `policy:read` + `engine:read` | same profiles; for each kind with rows, the counter (policy_version or tag_version) V and `updated_at` U; if `U < now() - 90 s`: not confirmed when no row polled at or after U, or any row polled at or after U reports `last_known_version < V`. Rows that polled before U (an old pod's plugin id) are ignored |
| `plugin_quiet` | warning | `policy:read` + `engine:read` | same profiles, last poll older than 5 min but within 24 h |
| `compile_failed` | critical | `policy:read` | `perm.compile_runs.outcome = 'error'`; sample: profile, first line of the error, when, last success |
| `engine_failed` | critical | `engine:read` | `origin = 'provisioned' AND status = 'failed'` |
| `rule_overlaps` | warning | `policy:read` | `stored_overlaps_via` |
| `lineage_failed` | warning | `policy:read` OR `tag:read` | `view_lineage_state.state = 'failed'`; sample ordered by catalog, schema, name, with `detail` |
| `catalog_apply_failed` | warning | `catalog_registry:read` | attachments with `apply_status = 'failed'`; sample: catalog, engine, `apply_error` |
| `policy_warnings` | warning | `policy:read` | sum of `profile_warnings(..).warning_count` over every profile; sample: profiles with the most |
| `plugin_unmodelled` | warning | `engine:read` | engines whose `ranger_plugin_report` (§4.4) says not modelled, with its note |
| `tags_without_clearance` | info | `tag:read` | tags with no `classification_policies` row or both lists empty |
| `catalogs_without_engine` | info | `policy:read` OR `tag:read` | registry catalogs with no `engine_catalog_attachments` row |
| counts | — | catalogs: governance read; grants, rules: `policy:read`; tags: `tag:read` | `count(*)` |

Sentences carry the honesty this programme learned in #1 and #2: a label never claims more than the data shows.
- `plugin_silent`: "No plugin has polled profile P in 24 hours, while engine E is recorded as running. egret cannot
  tell a stopped engine from one that cannot reach egret or does not poll."
- `plugin_behind`: "No plugin of profile P has reported holding policy version V, set at T." It never says a plugin
  "enforces" anything: the ledger records what plugins report, and a forged poll can confirm early
  (`plugin_downloads.rs:12-14`).
- `plugin_quiet`: "No plugin has polled profile P for N minutes."
- External engines are never judged for liveness or drift, and the drift findings say so in their detail when the
  profile also binds an external engine: "egret does not run external engines and cannot tell whether they should
  be polling" (`0184:13`).
- **Refined by the live measurement (Task 1, `docs/en/plans/2026-10-18-dg-overview-live.md` §1.1):**
  - Liveness (`plugin_quiet`, `plugin_silent`) reads the **newest** `policies` poll per profile. A rescheduled
    pod's plugin id keeps its last `at` for 24 h; a rule reading "any row older than N" would raise a false
    `plugin_quiet` on every reschedule. Only `policies` is read for liveness: every plugin polls it.
  - `plugin_behind` reads only rows polled at or after U **and** within 5 min (a dead pod's id is neither
    evidence nor blame), and only for a profile that is not already quiet or silent (liveness covers it). A
    report of `lastKnownVersion = -1` is "not yet reported": it neither confirms nor counts as behind. Trino's
    tag retriever reports `-1` for one tag poll after every policy download, and every plugin reports it on its
    first poll after a restart. The cost, accepted (decision (b) of the Task 7 brief): a plugin that keeps
    polling but never reports a version is flagged by no drift rule; `plugin_quiet` and `plugin_silent` catch a
    plugin only once it stops polling. The threshold stays 90 s for both kinds (confirmation takes two polls, about 60 s at the rendered
    30 s poll).
  - A kind is judged only when the profile has a row of that kind polled within 24 h. StarRocks never
    downloads tags, so a missing tags download is never reported for it. A kind whose newest poll is older than
    5 min is not judged for `plugin_behind` either: when the profile's policies are still polled, its tag poll is
    reported as quiet ("No plugin has polled the tags of profile P for N minutes, while its policies are polled.",
    review M6).
- `updated_at` is set by other updates too, which can only delay `plugin_behind`, never raise it falsely; the
  spec says so and Task 1 measures it.
- `tags_without_clearance`: "N classifications have no clearance: they gate nothing." Not "unprotected".

### 4.4 `ranger_plugin_report` moves to `egret-perm`

`plugin_report` (`engines.rs:335-371`) depends only on `egret_perm` functions (`ranger_plugin_is_modelled`,
`plugin_version_for_image`, `stored_engine_image`, `engine_images::known_image_refs`,
`modelled_ranger_plugins_sentence`) and `is_external`. It moves to `egret_perm::ranger_plugin_report(engine_type,
origin, declared_version, spec) -> PluginReport { version, modelled, note }`; `EngineDto::from` calls it and its
tests (`engines.rs:7194-7305`) stay green unchanged. `egret-controlplane` already depends on `egret-perm`, so no new
crate edge (adding `egret-engine-mgmt` would be one).

## 5. Frontend

- `App.tsx`: `const OverviewRoutes = lazy(() => import("@/routes/governance/overview/OverviewPage"))`; the index
  route renders it inside `LazyRouteArea` under `RequirePermission perm={["policy:read","tag:read"]}`;
  `GovernanceIndex` is removed; `*` becomes `<Navigate to="/governance" replace />` (absolute, the react-router 7
  loop note at `App.tsx:526-528`). Unknown governance URLs used to land on Data; they now land on the Overview.
- Sidebar: a first group `{ heading: "Governance overview", items: [{ label: "Overview", icon: Gauge, perm:
  ["policy:read","tag:read"], to: "", end: true }] }`. `NavItem` gains an additive `end?: boolean` that
  `renderItem` passes to both `NavLink`s (`Sidebar.tsx:230-254`); without it the item would be active on every
  governance page, because its path is the area's base. The reservation comment is replaced.
- Files: `routes/governance/overview/{OverviewPage,CountsStrip,AttentionSection,AttentionRow,RecentChanges}.tsx`,
  `overview/refUrl.ts` (Ref → URL, using `peopleUrl.ts` and `data/objectPath.ts`, both statically importable),
  `overview/queryKeys.ts` only if a hand-written key is needed (FE-DATA-2), `__tests__/`. Bundle guard
  `frontend/scripts/assert-governance-overview-lazy.mjs`, script `test:bundle:governance-overview`, listed in
  FE-ROUTE-2 and FE-TEST-4 of `docs/en/frontend-rules.md`.
- Client regenerated: `cargo build -p egret-bin && target/debug/egret openapi > openapi.json`, then
  `cd frontend && npm run gen:api` (never bare `orval`).
- `docs/en/frontend-rules.md` in full; Metabase design language and OKLCH tokens; `ui-ux-pro-max` for the visual
  detail; 390 px; jsx-a11y. FE-REF: no source product; the Overview follows egret's list-page frame, a deliberate
  choice recorded here.

## 6. Testing

- **Recording** (DB tests on a test DB from `egret_test_db::url()`, fixtures scoped to the test): each row of §3.2
  writes exactly one event with the right action, resource, actor, impersonator and before/after; a refused write
  (409 overlap), a `preview`, a dry-run and an unproved migration write none; an event survives `audit.enabled =
  false` (no sink drained); a summary over 16 KiB is truncated.
- **compile_runs**: a success through each funnel entry writes `ok` with counts; a write whose compile fails writes
  nothing; `recompile_service` failure writes `error` and keeps `last_ok_at`; a later success clears `error`; an
  untargeted profile type writes nothing.
- **attention**: each kind fires on a minimal fixture and stays silent on its negative twin (e.g. a plugin row
  older than the version's `updated_at` is ignored; an external engine is never judged); a source that errors
  yields `source_unavailable` while the others answer. The sources are a list of `(kind, permission, fn)` entries
  that `attention_via` runs; the test passes the real list plus one whose function runs `SELECT 1/0` on the
  transaction, so a missing savepoint would abort every source after it (the test places it first); permission filtering with real `CedarPdp`: a steward holding
  `policy:read` without `engine:read` gets no plugin or engine finding and sees them in `notChecked`; `tag:read` only
  sees tag, lineage and catalog findings; 403 without either.
- **changes**: per-family filtering (a `tag:read` reader never sees a grant event; a BI `object` tag event is never
  returned), cursor paging without duplicates, labels for a deleted actor and role, `retentionDays`.
- **Frontend vitest**: route and redirect (`/governance` and `/governance/nope`), sidebar order and gating; counts
  omitted per permission; each finding kind's sentence and links; severity grouping; empty state only when content
  allows it, including the `notChecked`-only and `source_unavailable` cases; Recent changes paging, impersonation,
  deleted target, empty and error states.
- **Live** (required): the plan's Task 1 measures the plugin ledger before the drift rules are coded; the browser
  check seeds each finding and screenshots 390 px and desktop.
