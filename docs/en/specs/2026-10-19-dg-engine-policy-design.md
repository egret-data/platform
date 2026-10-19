# Data Governance frontend redesign #4a: Engine & policy — design

Date: 2026-10-19. Status: design approved by the user on 2026-10-19 (U1–U11 below); the rows marked "default decision"
are this document's, and the rows marked **"open — needs the user"** are conflicts between the approved design and
the code that this document does not resolve. Base: main `3731f0ce` (#3 Overview merged as #695, F11 engine grants
as #694). Branch `feat/dg-engine-policy`, worktree `egret-dgep`.

This is sub-project #4a of the programme spec `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` ("programme
spec" below; decisions F1–F12). Part 1 of that spec gives the last sidebar group, **Engine & policy**, one page per
engine with Engine permissions (F11), compiled and hand-written policies, overlaps, migrate and plugin download state,
replacing Policies, ProfileDetail, PolicyEditor, PolicyMigration and OverlapReport (F12). Every claim about existing
code carries a `file:line` from `3731f0ce`, read in this worktree.

## Premises checked against the code

The brief's survey was checked. Statements that were wrong or incomplete are listed first; the design follows the code.

**Wrong or incomplete**

1. **"egret-seeded infra|operator".** `managed_by` has three values, `compiler`, `infra`, `operator`
   (`migrations/0180_policy_managed_by.sql:59-63`), and `operator` is the default for a policy a person writes in the
   editor (`0180:40-41`, `:59`). egret-seeded rows are `infra` only. The one exception the migration records: a
   Trino-shaped `all - …` set on a StarRocks profile was left `operator` so an operator can delete it (`0180:48-50`);
   it is hand-written for every purpose of this page. The Policies tab's source filter is therefore *egret-compiled*
   (`compiler`), *egret-seeded* (`infra`), *hand-written* (`operator`).
2. **"impersonate = user picker → targetUserId".** The write body carries the target as a **username** in
   `values[0]` (`NewEngineGrant`, `crates/egret-perm-compile/src/routes/engine_grants.rs:204-215`); the server
   resolves it to the id. The read DTO returns both the current username in `values` and `targetUserId`
   (`engine_grants.rs:148-154`); a target with no username is left out of `values`. The dialog's user picker yields a
   username.
3. **"compile failure said plainly ('saved, not yet on the engine')".** A compile that fails inside an engine-grant
   write fails the write: `write_engine_grant` returns the recompile's error before the handler commits
   (`engine_grants.rs:887`; the handler commits only after it, `:966`), and the 409 carries the refusal (`:945`). Nothing is saved. A grant can
   be saved but not compiled only when the compile **skips** it (`CompileResponse.skipped`, a `SkipReport` with
   `sourceKind = "engine_grant"` and the grant's id, `crates/egret-perm-compile/src/routes.rs:973-981`, `:1010-1018`),
   for example a target that no longer resolves. A compiled grant is on the engine only after the plugin's next
   download, which only the Plugin tab can speak to. §6.1 words the three outcomes accordingly.
4. **`valueKind` values.** On the wire: `username`, `identifier`, `catalog`, `function_signature`, and `fixed` for a
   key whose value the compiler supplies (`*`; `starrocks.manage_blacklist`) (`crates/egret-perm-compile/src/engine_capabilities.rs:24-44`,
   `engine_grants.rs:85-90`, `:492`). A `fixed` key takes no input.
5. **Compiled policy names name their source.** `attribute_policy_name` (`crates/egret-perm-compile/src/attribution.rs:97-116`)
   maps `perm-grant-{uuid}` → grant, `perm-rule-{uuid}[-locator]` → rule, `perm-clearance-{tag uuid}[-locator]` →
   the classification (tag), `perm-clearance-mv` → the engine-wide materialized-view deny (no single source), and
   `perm-engine-{uuid}` → engine grant. The source object can be derived from the name in every case but the MV deny.
   The frontend copy of the prefix list, `isCompilerOwnedPolicyName` (`frontend/src/api/policies.ts:270-271`), knows
   two of the four prefixes; this design reads `managed_by` for ownership and a new name parser only for the link
   (§6.2). Task 1 of the plan measures that stored compiler rows agree.
6. **Tag-based policies live in another profile.** A Trino profile's tag policies are stored in its tag service, a
   separate `authz_services` row named by `config.tagService` and returned as `ProfileDto.tag_service`
   (`crates/egret-policy/src/admin/mod.rs:445-455`; the client lists it with `listPolicies(tag_service)`,
   `frontend/src/routes/governance/ProfileDetailPage.tsx:248-264`). The Policies tab therefore lists two services, and
   the "Profiles without an engine" section must not list a tag service as an orphan.
7. **The policy editor needs the policy kind in its URL.** `PolicyEditorPage` reads `:kind`
   (`access`|`masking`|`rowfilter`) from the route before the policy loads (`frontend/src/routes/governance/PolicyEditorPage.tsx:287-299`)
   and builds its blank form and resource defs from it (`:153`, `:349-377`). The approved
   `…/advanced/policies/new` and `…/advanced/policies/:policyId` have no kind (Q2).
8. **The profile delete route has a generated client.** `policyAdminDeleteProfile` / `usePolicyAdminDeleteProfile`
   exist (`frontend/src/api/generated/policy-admin/policy-admin.ts:932`, `:982`); the `apiFetch` call and its comment
   ("the orval client isn't regenerated for this route yet", `frontend/src/api/policies.ts:97-108`) are stale. New
   code uses the hook (FE-DATA-2).
9. **An engine's profile is not on `EngineDto`.** `EngineDto` carries no binding (`crates/egret-engine-mgmt/src/engines.rs:290-332`).
   The engine page reads its profile and reach from `GET /perm/engines/{id}/grants` (`policy:read`,
   `engine_grants.rs:195-202`). `EngineDto`'s plugin version needs `engine:read`.
10. **`ProfileDto.engines` lacks origin and type.** `ProfileEngine` is `{id, name, status}` (`admin/mod.rs:471-475`),
    built from `EngineBinding` (`crates/egret-policy/src/admin/reader.rs:12-22`), which has neither. `ProfileDto` has a
    total `policy_count` only, no count per owner (`admin/mod.rs:438-444`). An index for a `policy:read`-only reader
    cannot show origin or a hand-written count without a backend change (Q1).
11. **The plugin ledger is read in two crates, and the Overview's judgement is not a function.** Besides
    `governance_overview.rs` (query at `:666-674`), `egret-policy` reads `policy.ranger_plugin_downloads` in
    `plugins_hold_tag_version` and `wait_for_tag_version` (`crates/egret-policy/src/plugin_downloads.rs:130`, `:175`).
    The Overview's rules are inline in `drift` (`crates/egret-controlplane/src/governance_overview.rs:666-778`) and
    `behind` (`:780-817`), over profiles with at least one provisioned running engine (`drift_profiles`, `:632-662`).
    They judge four states, not three: silent (no `policies` poll in 24 h), quiet (5 min–24 h, or a tags poll quiet
    while policies are polled), behind, and otherwise nothing. §4 extracts one function both callers use, and the
    Plugin tab shows `silent` too, or it would disagree with the Overview.
12. **The ledger is per profile, not per engine.** Its key is `(service_id, plugin_id, kind)`
    (`migrations/0186_plugin_downloads.sql`). On a legacy shared profile the tab cannot say which engine a row came
    from; it says so (O13 of the Overview kept per-engine attribution out).

**True as stated** (checked): governance routes at `frontend/src/App.tsx:615-623` (policies `:615-621`,
classifications `:622-623`); `GOVERNANCE_GROUPS` at `frontend/src/components/layout/Sidebar.tsx:90-129`, the Policies
group at `:115-118`; `LegacyGovernanceRedirect` and `RetiredPermPageRedirect` in
`frontend/src/routes/governance/LegacyGovernanceRedirect.tsx`; no `UNIQUE` on `engine_mgmt.engines.authz_service_id`
(`migrations/0058_engine_authz_service.sql:5-7`) and I2 refuses new shared binds (`engines.rs:5311-5326`);
`Reach.sharedProfile` (`engine_grants.rs:122-127`); engine-grant DTOs carry origin (`engine_grants.rs:100-136`);
`EngineDto` has `ranger_plugin_version`/`_modelled`/`_note` via `egret_perm::ranger_plugin_report`
(`engines.rs:321-366`, `crates/egret-perm/src/plugin_report.rs:29`); no per-profile plugin endpoint exists; the seven
engine-grant hooks are generated and have no caller (`frontend/src/api/generated/perm/perm.ts`); Data object pages say
"Hand-written and egret-seeded engine policies are not listed here" (`frontend/src/routes/governance/tabParts.tsx:88`),
unchanged in #4a; main's last migration is `0199`. The People picker is `PrincipalPicker`
(`frontend/src/components/perm/PrincipalPicker.tsx:114`), which edits a `{include, exclude}` set; the single-user
picker is `SubjectUserField` (`frontend/src/components/perm/ExplainResult.tsx:1078`).

## Decisions

| id | decision | by |
|---|---|---|
| EP1 | #4 is split: **#4a Engine & policy** (this spec); **#4b Classification** plus a backend route listing the hand-written policies that reach a Data object and "hand-written" markers on Data/People pages. #4a does not touch `Classifications.tsx`, `TagDetail.tsx` or the Classification sidebar item | user, 2026-10-19 (U1) |
| EP2 | The raw Ranger policy editor keeps today's permissions (`policy:create`/`update`/`delete`, no server change). "Admin-only advanced tool" means it is reachable only from the engine page's **Advanced** tab, with a fixed warning. No one loses a permission | user, 2026-10-19 (U2) |
| EP3 | Sidebar group "Policies" becomes **Engine & policy**, the last group, gated `policy:read`. `/governance/engines` index (engine table; "Profiles without an engine"; profile create/delete, delete confirmed by typing the name); `/governance/engines/:id` engine page with tabs Engine permissions · Policies · Overlaps · Migrate · Plugin · Advanced and a shared-profile banner from `reach`; old URLs redirect; old pages deleted after the redirects | user, 2026-10-19 (U3) |
| EP4 | Engine permissions tab and grant dialog on the F11 routes; Check through `explain/capability`; People principal page gets an "Engine permissions" section | user, 2026-10-19 (U4) |
| EP5 | Policies tab: every policy of the profile (and its tag service) with source/type/search filters; compiled rows read-only with "Open source"; hand-written rows enable/disable/delete and Edit in Advanced; seeded rows read-only; `PolicyWarningsCallout` kept; a count line by source | user, 2026-10-19 (U5) |
| EP6 | Overlaps tab: `GET /perm/overlaps` filtered to the profile | user, 2026-10-19 (U6) |
| EP7 | Migrate tab: today's migrate page restyled, behaviour unchanged, shown only when the profile has hand-written policies | user, 2026-10-19 (U7) |
| EP8 | New route `GET /api/control-plane/governance/engines/{id}/plugin` (`policy:read` AND `engine:read`), judging the ledger through the **same function** as the Overview | user, 2026-10-19 (U8) |
| EP9 | Advanced tab: the restyled editor, a fixed warning, the hand-written list, Create, Seed defaults (Trino, `policy:seed`) | user, 2026-10-19 (U9) |
| EP10 | Testing as §9, including a live check with real Trino and StarRocks, predictions first | user, 2026-10-19 (U10) |
| EP11 | No migration expected. If one is needed, its number is chosen with `ls migrations \| tail` right before merge | user, 2026-10-19 (U11) |
| EP12 | The engine page lives under Governance, not as a tab of Control Plane's engine page; both link to each other | default decision (2026-10-19) |
| EP13 | The index has a "Profiles without an engine" section; a profile page has the engine page's tabs minus Engine permissions | default decision (2026-10-19) |
| EP14 | A shared profile (legacy, several engines) shows a banner on every tab naming the other engines | default decision (2026-10-19) |
| EP15 | The profile page is `/governance/engines/profiles/:service`. When the profile is bound it redirects (replace) to the engine page; this one resolver serves `/governance/policies/:service` and the Overview's profile links | default decision (2026-10-19) |
| EP16 | The judging function moves to `crates/egret-controlplane/src/plugin_judgement.rs`; the Overview's sentences and findings do not change (its tests stay green untouched) | default decision (2026-10-19) |
| EP17 | Plugin tab states per kind: `up_to_date`, `pending`, `behind`, `quiet`, `silent`, `not_reported` (two reasons), and `not_judged` for the profile (external, not running, no profile) | default decision (2026-10-19); `silent` added so the tab never disagrees with the Overview (premise 11) |
| EP18 | "Download Trino config" (`TrinoConfigDialog`) moves to the Plugin tab | default decision (2026-10-19) |
| EP19 | The engine page is gated `policy:read`. Without `engine:read` the Plugin tab and the header's plugin version are replaced by one line naming the permission; every other tab works | default decision (2026-10-19) |
| EP20 | People: the "Engine permissions" section sits at the end of the principal page's Permissions tab, not as a new tab | default decision (2026-10-19) |
| EP21 | `PrincipalPicker` gains an additive `includeOnly` prop (no Exclude list); the grant dialog uses it | default decision (2026-10-19) |
| Q1 | **Index data for a `policy:read`-only reader.** Origin, engine type and a hand-written count per profile are not readable today without `engine:read` (premise 10). Proposed: additive `ProfileEngine.origin` and `.type`, and `ProfileDto.policyCounts {compiler, infra, operator}` (profile plus its tag service), no new route; the plugin column shows only with `engine:read`, one plugin call per engine. Alternative: show origin/type only with `engine:read` (from `GET /control-plane/engines`) and count hand-written policies with one `GET /policies` per profile | **open — needs the user** |
| Q2 | **Editor URLs keep a kind segment** (premise 7): `…/advanced/policies/:kind/new` and `…/advanced/policies/:kind/:policyId` instead of the approved `…/policies/new` and `…/policies/:policyId` | **open — needs the user** |
| Q3 | **`/governance/policies/:service` for a profile bound to several engines** (legacy shared): proposed, the first engine by name (`reach` order), whose banner names the others | **open — needs the user** |
| Q4 | **Write outcome wording** (premise 3): a refused compile is "Not saved: <server reason>" (the dialog stays open); a skipped grant is "Saved, but not compiled: <reason>. It is not on the engine."; a compiled grant is "Saved and compiled into <profile>. The engine applies it at its next policy download." There is no "saved, not yet on the engine" after a failed compile | **open — needs the user** |

## 1. What exists

- **Routes and pages.** `/governance/policies` (`Policies.tsx`: profile list, New profile, Delete profile with
  `confirmText="delete"` `:342-350`, the cross-profile `OverlapReport` `:287`); `/governance/policies/:service`
  (`ProfileDetailPage.tsx`: header, Seed defaults `:161-177`, Migrate link `:181`, Download Trino config `:188-193`,
  `PolicyWarningsCallout` `:224`, `OverlapReport serviceName` `:226`, tabs Access/Masking/Row-filter/Tags `:228-268`);
  `…/migrate` (`PolicyMigrationPage.tsx`, `service` param `:68`, writes need `policy:update` `:71-73`);
  `…/:kind/new` and `…/:kind/:id` (`PolicyEditorPage.tsx`, resource and by-tag modes, `?scope=tag`). All behind
  `RequirePermission perm="policy:read"` (`App.tsx:615-621`). Old Control Plane URLs reach them through
  `LegacyGovernanceRedirect` (`App.tsx:656`).
- **Links into them.** `ProfileDetailPage.tsx:94,181,262,376,381`, `PolicyMigrationPage.tsx:280`,
  `OverlapReport.tsx:123,183`, `Policies.tsx:120`, `PolicyEditorPage.tsx:388,1174`, `PolicyWarningsCallout.tsx:115`,
  `overview/refUrl.ts:27,29`, `control-plane/externalEngine.tsx:192-198` (`PolicyProfileLink`, used at
  `ExternalEngineDetail.tsx:265`), `control-plane/access/AccessOverview.tsx:61`.
- **F11 routes** (`engine_grants.rs`): `GET /perm/engine-capabilities` (`:463`), `GET /perm/engines/{engine_id}/grants`
  (`:513`; `engine`, `profile`, `reach`, `grants`, `managed`), `GET /perm/engine-grants` (`:579`), `POST
  /perm/engines/{engine_id}/grants` (`:935`), `PATCH`/`DELETE /perm/engine-grants/{id}` (`:979`, `:1025`), `POST
  /perm/explain/capability` (`:1192`). Reads and explain `policy:read`, writes `policy:update`. Status codes per F11
  §5.5. `ManagedKind` = `user_baseline` | `delegator` | `admin_all_access` | `default_all_access` (`:162-176`).
- **Policies list.** `GET /policies?service=&type=` returns `PolicySummary` with `managed_by` (`admin/mod.rs:677-689`,
  `frontend/src/api/policies.ts:57-72`); `PUT /policies/{id}/enabled`, `DELETE /policies/{id}` refuse `infra` and
  compiler-owned rows server-side.
- **Overlaps.** `GET /perm/overlaps` (`usePermListOverlaps`), filtered per profile by `OverlapReport`
  (`OverlapReport.tsx:45`).
- **Plugin.** The ledger and the Overview's rules (premises 11–12); `TrinoConfigDialog`
  (`frontend/src/components/policy/TrinoConfigDialog.tsx`).
- **Picker components.** `PrincipalPicker`, `SubjectUserField` (premise list); `useControlPlaneGovernanceListCatalogs`
  returns each catalog with the names of the engines it is attached to (`crates/egret-controlplane/src/governance_browse.rs:70-79`).

## 2. Information architecture

| URL | page | guard |
|---|---|---|
| `/governance/engines` | index | `policy:read` |
| `/governance/engines/:id?tab=` | engine page; `tab` ∈ `engine-permissions` (default), `policies`, `overlaps`, `migrate`, `plugin`, `advanced` | `policy:read` |
| `/governance/engines/:id/advanced/policies/:kind/new` · `…/:kind/:policyId` | the editor, for the engine's profile (Q2) | `policy:read` (writes per control, as today) |
| `/governance/engines/profiles/:service?tab=` | profile page (unbound profile); redirects to the engine page when bound (EP15) | `policy:read` |
| `/governance/engines/profiles/:service/advanced/policies/:kind/new` · `…/:kind/:policyId` | the editor for an unbound profile | `policy:read` |

The static segment `profiles` outranks `:id` in react-router's ranking; a test pins it. Everything lives in a lazy
area `frontend/src/routes/governance/engines/` (`EnginesRoutes.tsx`), mounted in `App.tsx` inside `LazyRouteArea`
(FE-ROUTE-2), with a bundle guard `npm run test:bundle:governance-engines`
(`frontend/scripts/assert-governance-engines-lazy.mjs`, listed in FE-ROUTE-2 and FE-TEST-4).

Sidebar: the group `{ heading: "Policies", … }` (`Sidebar.tsx:115-118`) is removed and a last group
`{ heading: "Engine & policy", items: [{ label: "Engine & policy", icon: ShieldCheck, perm: "policy:read", to: "engines" }] }`
is appended after Lineage. The heading is distinct from every other app's (`Sidebar.tsx:79-80`).

## 3. Redirects and links

| old URL | new URL |
|---|---|
| `/governance/policies` | `/governance/engines` |
| `/governance/policies/:service` (and `?tab=access`, `masking`, `rowfilter`, `tags`) | `/governance/engines/profiles/:service?tab=policies` → resolver (EP15, Q3); the old type tab becomes the Policies tab's type filter (`&type=`) |
| `/governance/policies/:service/migrate` | resolver with `tab=migrate` |
| `/governance/policies/:service/:kind/new` (`?scope=tag` kept) | resolver to `…/advanced/policies/:kind/new` |
| `/governance/policies/:service/:kind/:id` | resolver to `…/advanced/policies/:kind/:id` |
| `/control-plane/policies/*` | already → `/governance/policies/*` (`LegacyGovernanceRedirect`), then the rows above |

`PolicyRedirect` (new, `routes/governance/engines/PolicyRedirect.tsx`) reads the profile list (`listProfiles`), finds
the profile by name, and navigates with `replace`: bound → `/governance/engines/<engine id>/…`, unbound →
`/governance/engines/profiles/<name>/…`, unknown → the index with a "No policy profile named X" notice. It keeps
`search` and `hash`. It is statically importable (no eager page imports) so the redirect routes stay in `App.tsx`.

Links updated: `overview/refUrl.ts` (`profile` → `/governance/engines/profiles/<name>`; `profiles` →
`/governance/engines`; `refUrl` gains an optional `tab` that `AttentionRow` passes: `plugin` for `plugin_*`,
`policies` for `policy_warnings`, `overlaps` for `rule_overlaps`; engine refs keep pointing at Control Plane, where
`engine_failed` and `plugin_unmodelled` are fixed); `PolicyProfileLink` (→ the resolver); `AccessOverview.tsx:61`
(→ `/governance/engines`); Control Plane `EngineDetail` and `ExternalEngineDetail` headers gain "Open in Engine &
policy" → `/governance/engines/:id`, shown with `policy:read` (EP12).

## 4. Backend: the plugin route

### 4.1 One judging function (EP8, EP16)

`crates/egret-controlplane/src/plugin_judgement.rs`:

```rust
pub(crate) struct ProfileVersions { pub policy_version: i64, pub tag_version: i64, pub updated_at: DateTime<Utc> }
pub(crate) struct LedgerRow { pub service_id: i64, pub plugin_id: String, pub kind: String,
                              pub last_known_version: Option<i64>, pub served_version: Option<i64>, pub at: DateTime<Utc> }
pub(crate) enum Liveness { Live, Quiet { age_secs: i64 }, Silent }
pub(crate) enum NotReported { NoDownload, VersionUnreported }
pub(crate) enum KindState { UpToDate, Pending, Behind { version: i64 }, Quiet { age_secs: i64 }, Silent,
                            NotReported(NotReported) }
pub(crate) struct Judgement { pub liveness: Liveness, pub policies: KindState, pub tags: KindState }
pub(crate) fn judge_plugins(p: &ProfileVersions, rows: &[&LedgerRow], now: DateTime<Utc>) -> Judgement;
pub(crate) async fn ledger_rows_via(c: &mut PgConnection, service_ids: &[i64]) -> Result<Vec<LedgerRow>, sqlx::Error>;
```

The rules are the Overview's, moved, not rewritten (`QUIET_SECS`, `SILENT_SECS`, `BEHIND_SECS`, the newest-poll
liveness, the `-1` rule, the 5-minute window for `behind`, review M6's tags quiet). Mapping: when liveness is `Silent`
or `Quiet`, both kinds carry it; otherwise each kind is `NotReported(NoDownload)` with no row of that kind in 24 h,
`Quiet` for a tags kind quiet while policies are polled, `Pending` while `updated_at` is under 90 s old, `Behind` as
`behind` decides today, `UpToDate` when a row polled at or after U within 5 min reports ≥ V, else
`NotReported(VersionUnreported)` (polled since U, every report `-1`). `drift` calls `judge_plugins` and builds its
findings from the result: `plugin_silent` ⇔ `Silent`, `plugin_quiet` ⇔ liveness `Quiet` or tags `Quiet`,
`plugin_behind` ⇔ a kind `Behind` (the first, policies before tags, as today). `UpToDate` and `VersionUnreported` are
both "no finding", as today; only the tab tells them apart.

### 4.2 `GET /api/control-plane/governance/engines/{id}/plugin`

In a new `crates/egret-controlplane/src/governance_engines.rs`, tag `control-plane-governance`. Requires
`policy:read` **and** `engine:read` (two `egret_core::authz::require` calls; 403 without either). One `REPEATABLE
READ, READ ONLY` transaction, as `begin_snapshot` (`governance_overview.rs:359`).

```text
{ generatedAt,
  engine: { id, name, type, origin, status },
  profile: { serviceId, name, policyVersion, tagVersion, updatedAt } | null,
  sharedWith: [ { id, name, origin } ],                     // other engines bound to the profile, by name
  plugin: { version: string|null, modelled: bool, note: string|null },   // egret_perm::ranger_plugin_report
  judged: { state: "judged" } | { state: "not_judged", reason: "external" | "not_running" | "no_profile" },
  kinds: [ { kind: "policies"|"tags", currentVersion, lastDownloadAt|null, newestReportedVersion|null,
             servedVersion|null, state: "up_to_date"|"pending"|"behind"|"quiet"|"silent"|"not_reported",
             notReported: "no_download"|"version_unreported"|null, sentence } ],   // empty when not judged
  rows: [ { pluginId, kind, lastKnownVersion, servedVersion, at } ] }              // raw, newest first
```

- `not_judged`: `external` for `origin = 'external'` (sentence "egret does not judge external engines: it does not
  run them and cannot tell whether they should be polling."); `not_running` when the profile has no provisioned engine
  with `status = 'running'` (the Overview's population, `drift_profiles`); `no_profile`. `rows` are returned in every
  case except `no_profile`.
- Sentences: the Overview's sentence for the same state where it has one (`behind`, `quiet`, `silent`); for the others
  "Plugins of profile P report holding policy version V." / "A plugin downloaded version V at T; confirmation takes two
  polls." / "No plugin of profile P downloaded tags in 24 hours. StarRocks never downloads tags." (the last clause only
  for a StarRocks engine) / "Plugins polled since version V was set, but none has reported a version yet." Never
  "enforces" (the ledger records what plugins report, `plugin_downloads.rs:12-14`).
- 404 for an unknown engine. A shared profile (`sharedWith` not empty) answers per profile; the tab says the rows may
  come from any of the engines.
- No migration: the per-profile read is served by the primary key `(service_id, plugin_id, kind)` (EP11).

## 5. The index (`/governance/engines`)

`ListPageShell` (FE-PAGE-1): `h1` "Engine & policy", toolbar (New profile, `policy:create`), `SummaryStrip`
(engines · profiles without an engine · hand-written policies, the last only when counted, Q1), then two sections.

**Engines** (`FilterableTable`): name (link to the engine page), type, origin (`provisioned`/`external`), profile (name,
or "No profile" with a link to Control Plane's engine Settings when `engine:read`), plugin state (worst kind state from
the plugin route, only with `engine:read`; "Not judged: external" for external engines), engine grants (count of
`GET /perm/engine-grants` rows written on the engine), hand-written policies (Q1). Sources: with `engine:read`,
`GET /control-plane/engines` (every engine, including unbound); without it, the bound engines in `ProfileDto.engines`.
Empty: "No engine is recorded. Engines are created in Control plane."

**Profiles without an engine**: profiles whose `engines` is empty and that no other profile names as `tag_service`
(premise 6). Row: name (link to the profile page), type, origin (`manual`/`generated`), policy count, Delete
(`policy:delete`; `ConfirmDeleteDialog` with `confirmText` = the profile name; the server's 409 shown, e.g. a generated
profile). Empty: "Every profile is bound to an engine."

New profile: the existing dialog (name, type), moved from `Policies.tsx`. Delete uses `usePolicyAdminDeleteProfile`
(premise 8).

## 6. The engine page (`/governance/engines/:id`) and the profile page

**Header.** Name; type badge; origin badge; profile name (or "No profile"); plugin version and "not modelled" note
(from the plugin route, EP19); "Open in Control plane" → `/control-plane/engines/:id` when `engine:read`. Data:
`usePermListEngineGrants(id)` (engine, profile, reach). 404 → "No engine with this id" and a link to the index.

**Shared-profile banner** (EP14), on every tab, from `reach.sharedProfile`: "Profile P is shared with engines A, B.
Everything on this page applies to all of them." External engine (any tab with writes): "egret compiles this
engine's policies but cannot see whether the external engine downloads them."

**Tabs** (`ui/tabs`, `?tab=`, FE-PAGE-2): Engine permissions · Policies · Overlaps (count badge) · Migrate (only when
the profile has hand-written policies) · Plugin · Advanced. A tab named in `?tab=` that is not shown falls back to the
first. The **profile page** renders the same frame for an unbound profile: header (name, type, origin, "No engine is
bound to this profile"), tabs Policies · Overlaps · Migrate · Advanced (no Engine permissions, no Plugin).

Each tab has its own loading skeleton, error (server message, Retry) and empty state; one failing request never blanks
the page (FE-STATE-1–3).

### 6.1 Engine permissions (EP4)

- **No profile.** "This engine downloads no policy profile, so nothing can be granted on it. Bind a profile in Control
  plane." No Grant button.
- **Granted**, grouped by capability (label and description from `usePermListEngineCapabilities`): each row shows the
  values (an impersonation target as its username linked to the People user page; "a user with no username" when
  `values` lacks it), the principals (each linked through `principalUrl`), the generated policy name, and "written on
  engine X" when `engineId` ≠ this engine (shared profile). Edit and Delete only with `policy:update`; Delete through
  `ConfirmDeleteDialog` (FE-ACTION-1). Empty: "No engine permission has been granted on this engine."
- **Managed by egret** (read-only, from `managed`): kind label (`user_baseline` "Every user's baseline",
  `delegator` "egret's delegator", `admin_all_access` "Platform admins: all access", `default_all_access` "Group admin:
  all access (seeded defaults)"), the capability label when mapped, resources, accesses, users and groups, enabled.
- **Not grantable here** (from `notGrantable` of the engine's type): what and why, and "A hand-written policy in
  Advanced can grant it." with a link to the Advanced tab.
- **Grant dialog** (create and edit share it; FE-PAGE-3 dialog, FE-FORM-1–4): Capability (`Select`, the engine type's
  capabilities, label + description) → one control per `resources[]` entry in order: `username` →
  `SubjectUserField`; `catalog` → `Select` of catalogs attached to an engine in `reach`
  (`useControlPlaneGovernanceListCatalogs`, Task 1 measures that this equals what the server accepts); `identifier` →
  `Input` with the rule as help; `function_signature` → `Input` with "e.g. my_udf(INT,VARCHAR)"; `fixed` → no control,
  "Applies to all (*)". Client checks before sending: non-empty, no leading or trailing whitespace, none of `* ? { }`
  (the server's rules, F11 §2; the server stays the authority). Principals → `PrincipalPicker includeOnly` (EP21),
  user/group/role; `public` is refused beside the field with the server's sentence. Edit sends the whole body
  (`PATCH`; the engine is fixed). A 4xx shows the server's message in the dialog, which stays open with every input
  kept (FE-ACTION-3). Success closes the dialog, invalidates the engine's grants, all engine grants and the profile's
  policies, and reports per Q4: a toast "Saved and compiled into P." plus, when `reach` names other engines, "It
  reaches A, B."; when `compile.skipped` names this grant, an Alert on the tab "Saved, but not compiled: <reason>. It
  is not on the engine."; for an external engine the banner sentence above.
- **Check** (toolbar button, `policy:read`): a dialog with user (`SubjectUserField`), capability and values (the same
  controls) → `POST /perm/explain/capability`. Result: "Allowed" / "Denied" / "Cannot be decided ahead of time"
  (never a colour alone, FE-A11Y-4), the deciding policy name, and its source: *egret engine grant* (attribution
  `engine_grant`, linked to the row), *egret-seeded* (name in `managed`), *egret-compiled* (another compiler prefix),
  or *hand-written*. Undecidable lists its causes; on a delegating Trino profile the impersonation sentence of F11
  §5.4. `no_profile` → "This engine has no profile to ask."; `tagResolution = declined` → "egret cannot resolve
  classification tags for this question."

### 6.2 Policies (EP5)

Data: `GET /policies?service=<profile>` and, when `tag_service` is set, `?service=<tag_service>`. Filters in the URL
(`?source=compiler|infra|operator&type=access|masking|rowfilter|tag&q=`), FE-PAGE-2. A count line: "N egret-compiled ·
N egret-seeded · N hand-written" (FE-COPY-4 plurals).

- **egret-compiled** (`managed_by = 'compiler'`): read-only; a lock icon and "Compiled by egret; change its source".
  "Open source" from a new `compiledSource(name)` in `engines/compiledSource.ts`, the frontend twin of
  `attribute_policy_name` (premise 5): grant → `/governance/grants/:id`; rule → `/governance/rules/:id`; clearance →
  `/governance/classifications/:tagId` (shown only with `tag:read`, else the kind); engine grant → this engine's
  Engine permissions tab with `&grant=:id` (row highlighted); MV deny and an unparsable name → the kind only
  ("Materialized-view deny", "Compiled policy").
- **egret-seeded** (`infra`): read-only, "Managed by egret (engine delegation, admin access or query limits)."
- **hand-written** (`operator`): the label "Not managed by egret"; enable/disable (`policy:update`), Delete
  (`policy:delete`, confirm), Edit → the Advanced editor URL.
- `PolicyWarningsCallout` above the table, unchanged.
- Empty: "This profile has no policy." A filter miss: "No policy matches these filters." with Clear (FE-STATE-3).

### 6.3 Overlaps (EP6)

`usePermListOverlaps`, rows whose `serviceName` is the profile. Each pair: the two rules (links to their rule pages,
or the hand-written policy's editor link as today, `OverlapReport.tsx:183`), the shared object, and why. Count badge on
the tab. Empty: "No overlapping rules on this profile." The rendering moves from `OverlapReport.tsx` into
`engines/tabs/OverlapsTab.tsx`; `OverlapReport` is then deleted with `Policies.tsx`.

### 6.4 Migrate (EP7)

`PolicyMigrationPage`'s body as `engines/tabs/MigrateTab.tsx` taking `service` as a prop instead of the route param;
classify, dry run, apply and undo unchanged; restyled to the tab frame (no back link, no `h1`). Shown only when the
Policies list holds a hand-written row. Writes need `policy:update` (today's per-control check). After apply: "See it
in Recent changes" → `/governance#recent-changes` (the section gains that `id`, additively).

### 6.5 Plugin (EP8, EP17, EP18)

From §4.2. Without `engine:read`: "Plugin state needs the engine:read permission." (EP19). Otherwise: plugin version,
"modelled" or the note; per kind a row: state word and icon, the sentence, last download (relative + absolute
`<time>`), current version, newest reported, served; the rule in one line ("A version counts as held once a plugin
reports it; confirmation takes two polls, about a minute at the default 30 s poll."). `not_judged` → its sentence and
the raw rows only. Raw rows table (plugin id, kind, last known, served, when). A shared profile: "These rows may come
from any engine bound to P." "Download Trino config" (Trino profiles), as today.

### 6.6 Advanced (EP2, EP9)

Fixed warning (an `Alert`, always shown): "Hand-written policies are outside egret's model: egret does not recompile
them, does not check overlaps with grants, and Data and People pages will mark them hand-written. Prefer Engine
permissions or grants on Data." Below: the hand-written policies (name → editor, type, resource, enabled), "Create
policy" (kind menu Access/Masking/Row filter, and By tag when the profile has a tag service; `policy:create`), and
**Seed default policies** (Trino profiles only, `policy:seed`) with "Adds egret's default Trino set (group `admin`:
all access). Existing policies are not changed." The editor (`PolicyEditorPage`, moved to
`engines/PolicyEditorPage.tsx`, behaviour unchanged) takes `service` from its parent route and builds its back link and
its after-save navigation to the Advanced tab.

## 7. People: Engine permissions on a principal page (EP4, EP20)

At the end of the Permissions tab (`people/tabs/PermissionsTab.tsx`), for user, group and role pages (not public,
not workspace): `usePermListAllEngineGrants`, filtered with `setReach({include: g.principals, exclude: []},
standsFor)` (`people/reach.ts`), so a grant reaching a user through a group is marked "through group g" exactly as data
grants are. Each row: engine (link to the engine page), capability label, values, how it reaches. Empty: "No engine
permission reaches this <user|group|role>."

## 8. Deleted

After the redirects land: `Policies.tsx`, `ProfileDetailPage.tsx`, `PolicyMigrationPage.tsx` (its body moved),
`PolicyEditorPage.tsx` (moved), `OverlapReport.tsx` (moved), their tests (ported, not dropped: every assertion that
still describes behaviour moves to the new file), `isCompilerOwnedPolicyName` and `deleteProfile` in
`api/policies.ts` when no caller is left. `TrinoConfigDialog` and `PolicyWarningsCallout` stay.

## 9. Testing (EP10)

- **Backend DB tests** (`egret_test_db::url()`, fixtures scoped to their own ids): `judge_plugins` on every state of
  §4.1 including `-1`, an old plugin id, tags quiet, a StarRocks profile with no tags; the route for a judged profile,
  external (`not_judged`, rows present), not running, no profile, unknown engine (404), 403 without `engine:read`, 403
  without `policy:read`. **Agreement test**: one fixture per Overview drift kind, asserting that `attention_via` raises
  `plugin_silent`/`plugin_quiet`/`plugin_behind` exactly when the route reports `silent`/`quiet`/`behind` for that
  profile; and the Overview's existing `governance_attention.rs` passes unchanged.
- **Frontend vitest**, red first per task: routes and every old URL's redirect (bound, unbound, unknown, shared, with
  `?scope=tag` and `?tab=`); sidebar group order, label and gating; `refUrl` and `AttentionRow` tabs; index sections,
  tag services excluded, delete by typing the name; each tab's loading/empty/error; Engine permissions grouped rows,
  managed and not-grantable sections, write controls hidden without `policy:update`; the grant dialog's controls per
  `valueKind`, client checks, server refusal kept in the open dialog, the three Q4 outcomes; Check's three outcomes;
  Policies filters, `compiledSource` for every prefix and a malformed name, controls per source; Overlaps filter;
  Migrate tab shown only with hand-written rows; Plugin states and `not_judged`; Advanced warning, Seed only on Trino
  with `policy:seed`; People section with a via-group grant.
- **Bundle guard** `test:bundle:governance-engines`, one marker per module of the area.
- **Live** (required): an isolated stack with real Trino 483 and StarRocks 4.1.6, predictions committed first (plan
  Task 17): grant impersonation through the dialog and prove it on each engine with a query; impersonation of `root` and
  of a platform admin refused with the server's reason; Check answers "cannot be decided ahead of time" for Trino
  impersonation on a delegating profile; the Plugin tab and the Overview agree while StarRocks is stopped (quiet) and
  after a compile (pending → up to date); every redirect; keyboard only, Escape closes each dialog, 390 px and 1280 px,
  light and dark.

## 10. Out of #4a

Classification and TagDetail (#4b); the route listing hand-written policies per Data object and the "hand-written"
markers (#4b); per-engine plugin attribution; deny engine grants; editing a profile's `tagService`; RisingWave.
