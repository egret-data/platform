# Data Governance frontend redesign: design

Date: 2026-10-16. Status: approved in brainstorming (user, 2026-10-16). Base: main `58831374`.

The Governance app today mirrors Ranger's object model: one page per concept (Data permissions, Mask & filter rules,
Explain access, Policies, Classifications), so a user has to assemble the answer to a real question ("how is this
table protected?") across pages. This redesign organises Governance around the data, Unity Catalog style, and moves
the raw Ranger tools into one admin area.

Part 1 is the information architecture shared by the whole programme. Part 2 specifies sub-project #1 in full. The
other sub-projects get their own short spec on this frame.

## Decisions

| id | decision | by |
|---|---|---|
| F1 | Primary users: data stewards and platform admins, equally | user, 2026-10-16 |
| F2 | Primary axis: the data (catalog → schema → table/view → column) | user, 2026-10-16 |
| F3 | Raw Ranger tools live in an "Engine & policy" area inside Governance, per engine | user, 2026-10-16 |
| F4 | Scope: overview, people & groups, classification catalogue and lineage graph, as five sub-projects in the order of Part 1 | user, 2026-10-16 |
| F5 | Object page layout: Unity Catalog Catalog Explorer — tree left, object header with actions, tabs | user, 2026-10-16 |
| F6 | External catalogs are listed through an engine via a new backend API | user, 2026-10-16 |
| F7 | Old pages stay until the sub-project that replaces them lands; then their URLs redirect | default decision (2026-10-16) |
| F8 | The browse API uses egret's existing internal metadata identity per engine (see §2.3); widening any of its privileges is asked first | default decision (2026-10-16) |
| F9 | Tag assignment from the object page works on egret-managed catalogs only in #1 (the tag API is warehouse-keyed); external catalogs show it disabled with the reason | default decision (2026-10-16) |
| F10 | The object page lists the rules that select a column, never a client-side "who sees what"; only explain evaluates | default decision (2026-10-16) |
| F11 | Engine-level permissions (Trino `trinouser` impersonate, `queryid` execute/kill, `systemproperty`/`sessionproperty` alter, function/procedure execute; StarRocks `system`, resource group, global function) become real grants in egret's model with an `engine` scope, compiled by perm-compile, granted from an "Engine permissions" tab on each engine's page in Engine & policy. Data `read`/`write` stay on the Data object page. Needs a backend package before #4 | user, 2026-10-16 |
| F12 | Every Ranger-shaped page is replaced or redirected by the end of #4. The raw Ranger policy editor stays, restyled, as an admin-only advanced tool inside Engine & policy (hand-written policies remain the escape hatch for what the model cannot express, existing hand-written policies, and profiles egret does not manage). Data object pages also list the hand-written policies that reach the object, marked "hand-written", so they are never hidden | user, 2026-10-16 |

## Part 1. Information architecture (all sub-projects)

Governance sidebar, top to bottom:

| item | contents | sub-project | replaces |
|---|---|---|---|
| Overview | what needs attention | #3 | (new) |
| Data | catalog tree and Unity-Catalog-style object pages | #1 | the create/edit flows of Data permissions and Mask & filter rules |
| People & groups | a user, group or role and their effective access | #2 | Explain access; the flat Data permissions and Mask & filter rules lists |
| Classification | the tag and clearance catalogue | #4 | Classifications, TagDetail |
| Lineage | the dependency graph | #5 | the Data Lineage placeholder |
| Engine & policy (last group, admin) | one page per engine: **Engine permissions** (engine-scoped grants, F11; egret's seeded baselines shown as managed, read-only), profile, compiled and hand-written policies, overlaps, migrate, plugin download state | #4 (after the engine-grants backend package) | Policies, ProfileDetail, PolicyEditor, PolicyMigration, OverlapReport |

- Until #3 lands, the Governance index opens Data.
- Data permissions and Mask & filter rules stay in the sidebar until #2 replaces them, as audit lists; after #1
  creating and editing happen on object pages, and each list row links "Open in Data".
- When a sub-project replaces a page, the old URL redirects (the pattern of `LegacyGovernanceRedirect`).
- Permissions are unchanged (`policy:read`, `tag:read`, the per-control write permissions), so the sidebar filters
  as it does today.

Sub-projects in order: **#1** shell + Data; **#2** People & groups; **#3** Overview; **#4** Engine & policy +
Classification; **#5** Lineage.

## Part 2. Sub-project #1: the shell and Data

### 2.1 Routes

- `/governance/data` — the tree with no object selected (a short "pick a catalog" state).
- `/governance/data/:catalog` · `/:catalog/:schema` · `/:catalog/:schema/:object` — object pages.
  `?column=<name>` opens the column panel. `?tab=<id>` selects a tab. Back/forward and bookmarks work.
- `/governance` index → `/governance/data` (until #3).

### 2.2 The page

**Tree (left).**
- Top level: the registry's catalogs, each labelled with its source (`Iceberg · egret`, or the connector type plus
  `via engine`).
- Expanding loads the next level lazily: schemas, then tables / views / materialized views.
- A search box filters what is loaded. A "go to path" box opens any `catalog.schema.object` directly.
- A node that cannot be listed shows the API's reason (§2.3 `notListed`) and the "go to path" hint, never an empty
  branch.
- Keyboard: the ARIA treegrid pattern of the workflows namespaces list (`frontend/src/routes/workflows/NamespacesList.tsx`, #671).

**Object header.** Name, tags (direct and inherited, distinguished), kind, and the engines the catalog is attached
to. Actions, each shown only with its permission: **Grant access**, **Add tag** (F9), **Who can read?**.

**Tabs per level.**

| level | tabs |
|---|---|
| catalog | Overview (schemas, attached engines, workspaces) · Permissions · Masks & filters · Details |
| schema | Overview (tables and views) · Permissions · Masks & filters |
| table | Overview (columns: type, tags, masking rules) · Permissions · Masks & filters (masks, row filters) · Lineage (views and MVs that read it) · Details |
| view / MV | as a table, plus on Overview: security mode (INVOKER/DEFINER), whether the owner is cleared, lineage state; Lineage shows its base tables |

- **Overview (table).** Columns with type, tags (direct or inherited) and the masking rules that select the column
  by path or by tag (F10).
- **Permissions.** Every grant whose scope covers this object, marked "inherited from <level>" or "here"; grants on
  child columns; deny grants marked. Edit and delete reuse `GrantDialog` and the existing delete flow.
- **Masks & filters.** Mask and row-filter rules that select the object. "Add rule" opens `RuleDialog` with the
  path filled in.
- **Lineage.** From §2.3's lineage API.
- **Details.** Full path, warehouse, workspaces, engines.
- **Column panel** (right sheet, from a column row or `?column=`): its tags, masking rules, grants on that column, and
  "Who can read this column?".
- **Who can read?** A dialog to pick a user, group or role; the answer is `/perm/explain` for exactly this object
  (and column).

### 2.3 Backend

**Browse.** `GET /api/control-plane/governance/browse/{catalog_id}` with optional `schema` and `table` query
parameters.
- egret-managed Iceberg catalogs are read from the warehouse directly (as `warehouse_admin`'s
  `namespaces` / `tables` routes do); no engine.
- Other catalogs are listed through an engine that is **provisioned by egret, running, and attached to the catalog**
  (Trino: `SHOW SCHEMAS FROM` / `SHOW TABLES FROM` / `DESCRIBE`, or `information_schema`; StarRocks: the equivalent
  `SHOW … FROM <catalog>`). An engine with `origin = external` is never dialled (the rule of commit `19d7ac2db`).
- Identity (F8): on a Trino JWT engine, the existing `egret-lineage` principal (its download-time transform already
  grants `use`/`select`); on StarRocks, the service account the materialized-view listing in `view_exposure` uses.
  The plan's first task measures on live engines whether these can `SHOW`/`DESCRIBE` everything; if a privilege must
  be widened, stop and ask. The lineage transform is served only to provisioned **JWT** Trino engines; for an
  impersonate-mode engine the same task decides between the delegator identity and `notListed`
  (`engine_error` with that reason), and asks if it would need a new grant.
- Deadline 15 s per call (the view audit's).
- Response: `{ source: "egret" | { engine: name }, items?: [{ name, kind: schema|table|view|materialized_view }],
  columns?: [{ name, type }], notListed?: { reason, detail } }`, where `reason` ∈ `no_engine_attached`,
  `only_external_engines`, `engine_not_running`, `timeout`, `engine_error`. A listing that fails says so; it is
  never returned as empty.

**Lineage.** `GET /api/control-plane/governance/lineage/{catalog_id}?schema=&table=`.
- For a table: the views whose recorded lineage (`view_lineage`, migration 0187) reads it, and the StarRocks MVs
  that read it (the `view_exposure` listing).
- For a view: its recorded base tables, security mode, owner, whether the owner is cleared, and `lineage_state`
  (`view_lineage_state`).
- Reuse `view_exposure`'s DTO pieces where they fit.

**Not new.** Grants and rules come from the existing list APIs filtered by scope on the client; tags from the
existing `resource-tags` routes; "who can read" from `/perm/explain`.

**Permissions.** Both new routes need `policy:read` or `tag:read` (they reveal catalog structure).

**Out of #1.** Tagging objects in external catalogs; server-side grant/rule filters; caching browse results.

### 2.4 Transition

- Data joins the sidebar and becomes the Governance index. Every other item stays (Part 1).
- Old list pages gain "Open in Data" links per row. No old page is removed in #1.

### 2.5 Errors and states

- Each tab has its own loading, empty and error state; one failing API never blanks the page.
- Empty states state the fact ("No grant reaches this table").
- `notListed` shows its reason and the "go to path" box.
- Controls without permission are hidden or disabled with the reason, as on the current pages.
- After a write (grant, tag, rule), refetch the open tab and confirm with a toast.

### 2.6 Frontend standards

- `docs/en/frontend-rules.md` in full (MUST rules, egret and jsx-a11y lint, no new legacy file).
- Metabase design language and OKLCH tokens (`var(--token)`); `ui-ux-pro-max` for the detailed visual design.
- Tree: ARIA treegrid; tabs: Radix Tabs; the page works at 390 px and at desktop widths.
- Regenerate the client with `npm run gen:api` only.

### 2.7 Testing

- Backend DB tests on `egret_test`: browse for an egret catalog, for an external catalog through a fake engine, and
  each `notListed` reason; an external-origin engine is never dialled; the lineage API for a table and for a view.
- Frontend vitest: the tree (lazy load, search, go-to-path, `notListed`), each tab, the column panel, the "Who can
  read?" dialog, empty and error states, permission gating.
- Full frontend gate (`npm run lint`, `npx tsc -b`, `npx vitest run`, `npm run build`, bundle tests).
- **Live browser check (required):** a throwaway stack with Trino and StarRocks; browse an egret catalog and an
  external catalog through an engine; grant and tag from the page; compare "Who can read?" with what the engine
  returns.
