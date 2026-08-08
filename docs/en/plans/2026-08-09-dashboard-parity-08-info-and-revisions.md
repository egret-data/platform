# W8 — Info Sidebar and Revision History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** The dashboard info sidebar says who made this dashboard and who last touched it, in
names rather than UUIDs; it says which folder the dashboard lives in and links there; and it grows
a History tab that can put an earlier version back.

**Architecture:** Three items, cheapest first, in that order, each landing in its own commit
*before* the next begins. Items 1 and 2 are attribution and a link — one migration, four DTO
fields, one pure formatter, one sidebar. Item 3 is the only place in the entire dashboard-parity
programme that adds a table and endpoints, and it turns on one observation about code that already
exists: **the dashboard PUT body is already a complete, id-free snapshot of everything a dashboard
edit can change.** So a revision is one `UpdateDashboard` value stored as JSONB, and a revert is
that value fed back through the same handler. Nothing has to be diffed, grafted or remapped.

**Tech Stack:** Rust (axum + sqlx + utoipa), Postgres, React 18 + TypeScript, TanStack Query,
orval-generated API client, vitest + Testing Library.

**Design reference:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` §4 W8.

**Branch:** `feat/dashboard-parity-08-info-and-revisions`, from `main`.

---

## Three places the design's assumptions do not match the code

Read these before starting. Each one changes the shape of a task.

**1. W8 is not "one backend item". All three items need backend work.** The design says revision
history is "the only item in this programme needing backend work" (spec §4 W8). It is not:

- `analytics.dashboards` has **no `created_at`** and **no `updated_by`**. It has carried
  `created_by` and `updated_at` since `migrations/0015_dashboard.sql` and nothing has been added
  since (`0016` parameters, `0017` tabs, `0018` width, `0019` soft-delete, `0021` folder_id). Two
  of the four values Metabase's line needs — created-when, edited-by-whom — do not exist anywhere.
- `DashboardDetailDto` (`crates/egret-analytics-api/src/bi.rs:360-373`) has **no `folder_id`**.
  `DashboardSummaryDto` does (`:333`), the detail DTO does not, so the sidebar has no folder to
  name.

**2. There is no frontend user-id→name lookup, and the two endpoints that could be one are
admin-gated.** The only uuid-bearing user endpoints are `GET /api/control-plane/users` (needs
`user:read`) and `GET /api/ws/{workspace}/members` (needs `workspace:manage_members`). A `ws_editor`
opening the info sidebar on their own dashboard would get a 403 from either. `GET
/api/control-plane/principals` returns `{kind, name, label}` with **no id at all**, so it cannot
resolve one.

What the repo does instead, in five places already, is resolve the name **server-side with a LEFT
JOIN** and ship it on the DTO: `MemberDto.principal_name`
(`crates/egret-workspace/src/members.rs:107` — `COALESCE(u.email::text, g.name) AS principal_name`),
`ObjectGrantDto.principal_name` (`crates/egret-bi/src/shares.rs:432`),
`ObjectGrantRollupDto` (`crates/egret-workspace/src/access_views.rs:161`), `AccessEntry`
(`crates/egret-auth/src/effective.rs:421`), and the shares rollup. The frontend consumes it with
`grant.principal_name ?? grant.principal_id` (`components/sharing/ShareDialog.tsx:316`). That is the
pattern this plan follows. Do not add a client-side map.

**3. Folders have no route.** `grep -n folder frontend/src/App.tsx` returns nothing; the only
dashboard routes are `dashboards` and `dashboards/:id` (`App.tsx:95-96`). A folder on the list page
is a collapsible `<tbody>` section header (`routes/explore/Dashboards.tsx:219-238`,
`components/explore/FolderHeaderRow.tsx:19-23`) whose name is a `<button>` that toggles collapse,
not an anchor, with collapse state in `localStorage` under `folders:${ws}:dashboards`
(`components/explore/folders.ts:36-50`). So "Saved in *X*" cannot link to a folder page — there
isn't one. Task 4 links to the list with `?folder=<id>` and teaches the list to expand that section
on arrival, which is the smallest thing that makes the link mean something.

---

## Background the implementer needs

### What is stored per revision, exactly

`UpdateDashboard` (`crates/egret-analytics-api/src/bi.rs:418-431`):

```rust
pub struct UpdateDashboard {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub width: String,
    pub tabs: Vec<NewTabInput>,   // { name, position }
    pub cards: Vec<CardInput>,    // { chart_id, card_type, grid_col, grid_row, size_x, size_y, options, tab_index }
}
```

**A revision is one of these, verbatim, as a JSONB column.** Three properties of that DTO make it
the right unit and are worth stating because the whole design rests on them:

1. **It is complete.** `db_update` (`crates/egret-bi/src/routes.rs:2693-2723`) writes exactly two
   things from it: `dashboards.update(name, description, parameters, width)` and
   `replace_layout(tabs, cards)`. There is no third write and no partial-update path — the PUT is a
   whole-layout replace, documented as such at `crates/egret-bi/src/dashboard.rs:3-5`. So the DTO
   covers 100% of what an edit can change. What it does *not* cover — `folder_id`, `deleted_at`,
   `created_by` — is changed by other endpoints (`PATCH …/folder`, `POST …/trash`) and is
   deliberately **not** revertible: reverting a layout must not silently move the dashboard back
   into a folder someone has since deleted, or un-trash it.
2. **It is id-free.** `NewTabInput` has no id and `CardInput` references its tab by `tab_index`,
   because `replace_layout` (`dashboard.rs:326-378`) `DELETE`s both tables and re-`INSERT`s, minting
   fresh ids on every single save. Tab and card ids are therefore not stable identifiers at all;
   they churn on each Save. This is why the frontend's own draft model uses client-side `tabKey`s
   and remaps at serialize time (`components/chart/dashboard/tabs.ts:3-4,34-48`).
3. **Because of (2), revert needs no remapping.** The instruction to read `DashboardRepo::duplicate`
   (`dashboard.rs:404-453`) was right to point at it as the closest analogue, and reading it settles
   the question in the negative: `duplicate` needs its `tab_map: HashMap<Uuid, Uuid>` (`:430-438`)
   *because it copies rows*, and rows carry ids that would collide. A revision does not copy rows —
   it stores the request — so there are no ids in it to collide with anything. **Do not reuse
   `duplicate`.** Revert is `serde_json::from_value::<UpdateDashboard>(snapshot)` followed by the
   ordinary update path.

### Where the capture point is

Inside `db_update`, **after both writes have returned `Ok`** and immediately before
`load_dashboard`. Not earlier, because every earlier point has a way to fail after it:

| point | what still fails after it |
|---|---|
| on entry | `require_object` → 403 |
| after authz | `require_cards_in_ws` → 404 on a foreign chart |
| after that | `validate_layout` → 400 on a card/tab mismatch |
| after that | the `width` check → 400 |
| after `update()` | `replace_layout()` → 400/500 |

A revision written before any of those describes an edit that never happened, and a history that
contains edits that did not happen is worse than no history. Placing it last means the failure mode
is **under-recording** — a process crash between `replace_layout` and the insert loses one entry —
which is the safe direction.

The insert itself is **best-effort**: if it fails, log and return 200. The update already landed, so
a 500 would tell the client the save was lost when it was not. This is the same stance the audit
sink takes, documented at `crates/egret-bi/src/routes.rs:51-53` ("best-effort, never fails the
request").

### Is there an existing revisions/audit pattern to follow?

Looked. **There is no revision/version/history table for any user content in this repo** — not for
charts, dashboards, datasets, notebooks or snippets. `charts.params` carries a *schema* version
constant (`chartparams.rs:17`), not per-save history. `catalog_table_snapshots` /
`catalog_view_versions` (`migrations/0023`) are Iceberg REST spec structures, not an app pattern.
A dead `policy_audit` table from `0007` was dropped by `0063` with the note "was never read or
written by any code".

**The one audit-like thing is `audit.audit_log`** (`migrations/0076_audit_log.sql`,
`crates/egret-audit/src/lib.rs`). Follow three of its conventions and none of its storage:

- **Follow:** best-effort write that never fails the request; newest-first ordering with the index
  built for it (`audit_log_ts_idx ON (ts DESC, id DESC)`); an explicit retention policy rather than
  unbounded growth (`prune`, `lib.rs:209-216`).
- **Do not use it as the store,** for two disqualifying reasons. Its writes go through a bounded
  channel that **drops on backpressure** (`AuditSink`, `lib.rs:55-71`) — a history with silent
  holes is not a history. And its `detail jsonb` carries no entity content, so nothing in the table
  can reconstruct a dashboard, which is the entire point of a revision.

So: a new table, and — because `DashboardRepo` already holds the `Db` handle — new methods on
`DashboardRepo` rather than a new repo. No `BiState` field, no constructor plumbing.

### Retention policy, and why

**Keep the newest 20 revisions per dashboard, trimmed inline in the same transaction as the insert.**

- **Count-based, not age-based.** `audit_log` prunes by age because it is one global firehose where
  "how old" is the only sensible axis. A per-entity history has the opposite shape: under a 90-day
  rule, a dashboard saved 400 times in one afternoon keeps all 400, and a dashboard saved twice a
  year keeps **none** — the second case being exactly the dashboard whose history matters most.
  Count-based bounds the worst case *and* guarantees the rarely-edited dashboard a usable history.
- **20, where Metabase keeps 15** (`revision/max-revisions`). Not arbitrary: Metabase writes a
  revision per small action — add card, move card, rename tab are each their own API call — so 15
  Metabase revisions can be fifteen minutes of one session. egret has exactly one write path, the
  whole-layout PUT behind the Save button (`DashboardView.tsx:385-394`), so one egret revision is a
  whole editing session and 20 of them is months of calendar history.
- **Trimmed inline, so no janitor.** The audit prune needs a background `tokio::spawn` loop
  (`crates/egret-bin/src/main.rs:1044-1069`) because it is not scoped to an entity. Ours is: the
  insert already knows which dashboard it is bounding, so a `DELETE … WHERE id NOT IN (SELECT …
  ORDER BY … LIMIT 20)` in the same transaction keeps the table self-bounding with no new scheduled
  task and no new config key.
- **Size.** The harness dashboard's PUT body is the realistic worst case in this repo — 22 cards
  with `vizOverride` blobs. 20 snapshots of it is single-digit megabytes for that one dashboard.
  Task 10 measures the real number with `pg_column_size` rather than asserting this.

### Two repo traps that apply here

**sqlx migration staleness — yes, it applies.** `crates/egret-metastore/src/lib.rs:25` is
`sqlx::migrate!("../../migrations")`, which embeds at **compile time**; adding a `.sql` file does
not by itself make the crate rebuild. The house convention is a one-line `// re-embed: NNNN <slug>
(<why>)` comment near the top of that file (see `:2-7` and `:23`), which both forces the rebuild and
records what changed. Both migrations in this plan add one.

**The frontend API client is generated.** `frontend/src/api/dashboards.ts` is a thin wrapper over
`src/api/generated/dashboards/dashboards.ts` (orval, `frontend/orval.config.ts`). New endpoints are
not callable until the spec and client are regenerated:

```bash
cargo run -p egret-bin -- openapi > openapi.json
cd frontend && npm run gen:api
```

Task 8 does this. Do not hand-write a fetch into `src/api/generated/`.

---

## File structure

| Path | Responsibility after this workstream |
|---|---|
| `migrations/0106_dashboard_attribution.sql` | **New.** `analytics.dashboards.created_at` + `.updated_by`. (Items 1) |
| `migrations/0107_dashboard_revisions.sql` | **New.** `analytics.dashboard_revisions`. (Item 3) |
| `crates/egret-bi/src/dashboard.rs` | Gains `created_at`/`updated_by` on the row, `attribution_names`, and the five revision methods (`snapshot_current`, `insert_revision`, `list_revisions`, `revision`, `revision_count`). |
| `crates/egret-analytics-api/src/bi.rs` | `DashboardDetailDto` gains attribution + `folder_id`; `UpdateDashboard`/`CardInput`/`NewTabInput` gain `Serialize`; new `DashboardRevisionDto`. |
| `crates/egret-bi/src/model.rs` | `dashboard_detail_dto` carries the resolved names; new `dashboard_revision_dto`. |
| `crates/egret-bi/src/routes.rs` | `load_dashboard` resolves names; one shared `apply_dashboard_update`; two new handlers; two new routes; two new OpenAPI entries. |
| `frontend/src/components/chart/dashboard/attribution.ts` | **New.** Pure: `attributionLine`, `revisionSummary`. No React, no DOM. |
| `frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx` | Overview/History tabs; attribution rows; "Saved in" link. |
| `frontend/src/components/chart/dashboard/DashboardHistoryTab.tsx` | **New.** The timeline + revert action, so the sidebar stays a shell. |
| `frontend/src/components/explore/folders.ts` | `useCollapsed` gains `expand`. |
| `frontend/src/routes/explore/Dashboards.tsx` | Honours `?folder=<id>` by expanding that section. |
| `frontend/src/routes/explore/DashboardView.tsx` | **Wiring only** — new props passed through, no new logic. |

**On the two migration numbers.** 0104 is the highest on `main` and **W3 claims 0105** (the
dashboard width column default), so W8 takes 0106 and 0107 — attribution first, because items 1–2
ship before the revision work. Both numbers are written out below for concreteness, but run
`ls migrations/ | tail -3` before creating either file and take the next free pair if another
workstream has landed in the meantime; a duplicate prefix makes `sqlx::migrate!` fail at compile
time with a message that names the directory, not the culprit.

---

## Task 1: Migration — dashboards learn when they were created and who last edited them

**Files:**
- Create: `migrations/0106_dashboard_attribution.sql`
- Modify: `crates/egret-metastore/src/lib.rs`
- Modify: `docs/en/plans/2026-08-09-dashboard-parity-00-index.md`

- [ ] **Step 1: Write the migration**

Create `migrations/0106_dashboard_attribution.sql`:

```sql
-- Dashboard attribution: WHEN a dashboard was created, and WHO last edited it.
--
-- THE GAP. analytics.dashboards has carried `created_by` and `updated_at` since 0015 and has not
-- gained an attribution column since. Metabase's info sidebar states four things — created when,
-- created by whom, last edited when, last edited by whom — and two of the four do not exist in
-- this schema at all. DashboardInfoSidebar.tsx:29-30 renders what there is: the raw `created_by`
-- UUID under "Owner", and a bare `updated_at` under "Last edited".
--
-- WHY created_at IS NULLABLE AND IS NOT BACKFILLED. The obvious shape is
-- `timestamptz NOT NULL DEFAULT now()`, which is what 0021_folders.sql used — on a NEW table.
-- On an EXISTING table it is a different statement: since Postgres 11, ADD COLUMN with a DEFAULT
-- materialises that default for every pre-existing row, so every dashboard in the deployment
-- would report that it was created the moment this migration ran. That is a wrong date which is
-- indistinguishable from a right one, and no later repair can recover the true value because the
-- true value was never recorded. Backfilling from `updated_at` is the same lie with extra steps:
-- `updated_at` is rewritten by every save, so it is the creation date of nothing. NULL is the
-- honest encoding of "never recorded", and the sidebar renders the "Added" line only when the
-- value is present. New rows pick up the DEFAULT attached below, so the gap closes going forward.
--
-- updated_by is ON DELETE SET NULL, matching the treatment 0093 gave every other attribution FK:
-- hard-deleting a user preserves the content and drops only the name.
ALTER TABLE analytics.dashboards
  ADD COLUMN created_at timestamptz,
  ADD COLUMN updated_by uuid REFERENCES users(id) ON DELETE SET NULL;

-- Attached AFTER the column exists, deliberately. Folding it into the ADD COLUMN above is what
-- would backfill every existing row (see the header); this form applies to future INSERTs only.
ALTER TABLE analytics.dashboards ALTER COLUMN created_at SET DEFAULT now();
```

- [ ] **Step 2: Force the migrator to re-embed**

In `crates/egret-metastore/src/lib.rs`, add one line immediately below the `// re-embed: 0104 …`
comment at `:23`, keeping the existing style:

```rust
// re-embed: 0106 dashboard_attribution (dashboards.created_at nullable + updated_by)
```

Without this the migration is not compiled into `MIGRATOR` and `just migrate` is a no-op that
reports success. `sqlx::migrate!` reads the directory at compile time, not at run time.

- [ ] **Step 3: Correct the index's file table**

The index's new-files table originally gave W8 a single migration, `0105_dashboard_revisions.sql`.
Two numbers have moved since. **0104 is the highest on `main`, and W3 (the dashboard width column
default) claims 0105** — check `ls migrations/ | tail -3` before writing either file and take the
next free pair, rather than trusting these numbers. W8 also needs *two* migrations, not one, because
attribution must land before the revision work (see "Three places…" above). Replace W8's row with:

```markdown
| `migrations/0106_dashboard_attribution.sql` | `dashboards.created_at` + `updated_by`. (W8) |
| `migrations/0107_dashboard_revisions.sql` | Revisions table. (W8) |
```

- [ ] **Step 4: Apply and verify against the live schema**

```bash
just migrate
```

Expected: exits 0. Then confirm the column is genuinely NULL for existing rows — this is the whole
point of Step 1's shape, so verify it rather than trusting it:

```bash
psql "$DATABASE_URL" -c "SELECT count(*) AS total, count(created_at) AS with_created_at FROM analytics.dashboards;"
```

Expected: `with_created_at` is **0** and `total` is whatever the deployment holds. If
`with_created_at == total`, the DEFAULT was folded into the ADD COLUMN — fix the migration, drop
the columns, and re-run.

- [ ] **Step 5: Commit**

```bash
git add migrations/0106_dashboard_attribution.sql \
        crates/egret-metastore/src/lib.rs \
        docs/en/plans/2026-08-09-dashboard-parity-00-index.md
git commit -m "feat(dashboard): record when a dashboard was created and who last edited it"
```

---

## Task 2: The detail DTO carries names, dates and its folder

**Files:**
- Modify: `crates/egret-bi/src/dashboard.rs`
- Modify: `crates/egret-analytics-api/src/bi.rs`
- Modify: `crates/egret-bi/src/model.rs`
- Modify: `crates/egret-bi/src/routes.rs`
- Test: `crates/egret-bi/tests/dashboard_attribution.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/egret-bi/tests/dashboard_attribution.rs`:

```rust
//! DB-gated attribution tests. Self-skip without DATABASE_URL.
use egret_bi::DashboardRepo;
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(DashboardRepo, Db, Uuid, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // One user WITH a profile name and one with only an email: the label rule has to prefer the
    // name when there is one and fall back to the email when there is not, and a test that used
    // only one of them would pass against either half of that rule.
    let named: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider) VALUES ($1,'Ref User','local') RETURNING id",
    )
    .bind(format!("named_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let email = format!("bare_{}@t.local", Uuid::new_v4().simple());
    let nameless: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider) VALUES ($1,'','local') RETURNING id",
    )
    .bind(&email)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(named)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    Some((DashboardRepo::new(db.clone()), db, ws, named, nameless))
}

#[tokio::test]
async fn new_dashboards_stamp_created_at_and_updates_stamp_updated_by() {
    let Some((repo, _db, ws, named, nameless)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Ops", "", Some(named)).await.unwrap();
    assert!(
        d.created_at.is_some(),
        "0106's DEFAULT must stamp newly created rows"
    );
    assert!(d.updated_by.is_none(), "a fresh dashboard has no editor yet");

    repo.update(
        d.id,
        "Ops",
        "edited",
        &serde_json::json!([]),
        "full",
        Some(nameless),
    )
    .await
    .unwrap();
    let after = repo.get(d.id).await.unwrap().unwrap();
    assert_eq!(after.updated_by, Some(nameless));
    assert_eq!(
        after.created_at, d.created_at,
        "an edit must not move the creation date"
    );
}

#[tokio::test]
async fn attribution_names_prefer_the_profile_name_and_fall_back_to_the_email() {
    let Some((repo, db, ws, named, nameless)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let email: String = sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1")
        .bind(nameless)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    let d = repo.create(ws, "Ops2", "", Some(named)).await.unwrap();
    repo.update(d.id, "Ops2", "", &serde_json::json!([]), "full", Some(nameless))
        .await
        .unwrap();

    let (created, updated) = repo.attribution_names(d.id).await.unwrap();
    assert_eq!(created.as_deref(), Some("Ref User"));
    assert_eq!(updated.as_deref(), Some(email.as_str()));
}

#[tokio::test]
async fn attribution_names_are_none_when_the_id_is_none() {
    let Some((repo, _db, ws, _named, _nameless)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Ops3", "", None).await.unwrap();
    let (created, updated) = repo.attribution_names(d.id).await.unwrap();
    assert_eq!(created, None);
    assert_eq!(updated, None, "never edited ⇒ no editor name");
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p egret-bi --test dashboard_attribution
```

Expected: FAIL to compile — `no field created_at on type Dashboard`, `no method attribution_names`,
and `update` takes 5 arguments not 6.

- [ ] **Step 3: Extend the repository**

In `crates/egret-bi/src/dashboard.rs`, add the two fields to `Dashboard` (after `updated_at`, at
`:26`):

```rust
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// NULL for every dashboard that predates migration 0106 — see that file for why it is not
    /// backfilled. The UI renders the "Added" line only when this is present.
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Who last saved this dashboard. NULL until the first save after 0105, and NULL again if
    /// that user is hard-deleted (0093's ON DELETE SET NULL treatment).
    pub updated_by: Option<Uuid>,
```

Extend `COLS` (`:90-91`) — the two new names must appear or every `SELECT {COLS}` fails to decode:

```rust
const COLS: &str =
    "id, workspace_id, name, description, parameters, created_by, folder_id, updated_at, created_at, updated_by, width, deleted_at";
```

Change `update` (`:180-200`) to take and record the actor:

```rust
    /// Update dashboard metadata only (name/description/parameters/width) and stamp the editor.
    /// Card layout goes through `replace_layout`; callers that change both run them together.
    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        parameters: &serde_json::Value,
        width: &str,
        updated_by: Option<Uuid>,
    ) -> Result<(), BiError> {
        sqlx::query(
            "UPDATE analytics.dashboards SET name=$2, description=$3, parameters=$4, width=$5, updated_by=$6, updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(parameters)
        .bind(width)
        .bind(updated_by)
        .execute(&self.db.pool)
        .await
        .map_err(map_conflict_err)?;
        Ok(())
    }
```

Add the row struct beside the other private `FromRow` helpers (after `DashboardWithCountBm`, `:74`):

```rust
/// Private struct for attribution_names — sqlx tuple FromRow is version-sensitive, same reason
/// DashboardWithCount exists.
#[derive(sqlx::FromRow)]
struct AttributionNames {
    created_by_name: Option<String>,
    updated_by_name: Option<String>,
}
```

And the lookup itself, after `owner` (`:399`):

```rust
    /// Display names for a dashboard's creator and last editor, resolved in ONE query.
    ///
    /// `COALESCE(NULLIF(u.name, ''), u.email::text)` is the label rule the principal picker
    /// already uses (egret-policy/src/admin/mod.rs:868-873): the profile name when it is set, the
    /// email otherwise. It is resolved here rather than in the browser because the only endpoints
    /// that can turn a user id into a name are admin-gated — /api/control-plane/users needs
    /// `user:read` and /ws/{ws}/members needs `workspace:manage_members` — so a ws_editor opening
    /// the info sidebar on their own dashboard would get a 403 from both. Five other DTOs in this
    /// repo (MemberDto.principal_name and friends) resolve names the same way.
    ///
    /// `(None, None)` for a dashboard that does not exist, which callers already treat as 404 via
    /// `get`.
    pub async fn attribution_names(
        &self,
        id: Uuid,
    ) -> Result<(Option<String>, Option<String>), BiError> {
        let row = sqlx::query_as::<_, AttributionNames>(
            "SELECT COALESCE(NULLIF(cu.name, ''), cu.email::text) AS created_by_name, \
                    COALESCE(NULLIF(eu.name, ''), eu.email::text) AS updated_by_name \
             FROM analytics.dashboards d \
             LEFT JOIN users cu ON cu.id = d.created_by \
             LEFT JOIN users eu ON eu.id = d.updated_by \
             WHERE d.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(match row {
            Some(r) => (r.created_by_name, r.updated_by_name),
            None => (None, None),
        })
    }
```

- [ ] **Step 4: Extend the DTO**

In `crates/egret-analytics-api/src/bi.rs`, replace `DashboardDetailDto` (`:360-373`):

```rust
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DashboardDetailDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub created_by: Option<String>,
    /// Display name for `created_by` — the profile name, else the email. Resolved server-side:
    /// the endpoints that can turn a user id into a name are admin-gated, so the browser cannot
    /// do it for an ordinary workspace editor. Same shape and same reason as
    /// `MemberDto.principal_name`.
    pub created_by_name: Option<String>,
    /// NULL for dashboards predating migration 0106, which does not invent a creation date.
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_by: Option<String>,
    /// Display name for `updated_by`, resolved the same way as `created_by_name`.
    pub updated_by_name: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// The folder this dashboard is filed under; NULL = Uncategorized. Present on
    /// `DashboardSummaryDto` since folders shipped, and absent here until now, which is why the
    /// info sidebar had no folder to name.
    pub folder_id: Option<String>,
    pub width: String,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub bookmarked: bool,
    pub tabs: Vec<DashboardTabDto>,
    pub cards: Vec<DashboardCardDto>,
}
```

- [ ] **Step 5: Extend the conversion and the loader**

In `crates/egret-bi/src/model.rs`, replace `dashboard_detail_dto` (`:273-292`):

```rust
/// `names` is `(created_by_name, updated_by_name)` from `DashboardRepo::attribution_names` —
/// passed in rather than fetched here so this stays a pure domain→wire conversion.
pub fn dashboard_detail_dto(
    d: Dashboard,
    tabs: Vec<crate::dashboard::DashboardTab>,
    cards: Vec<DashboardCard>,
    bookmarked: bool,
    names: (Option<String>, Option<String>),
) -> DashboardDetailDto {
    DashboardDetailDto {
        id: d.id.to_string(),
        name: d.name,
        description: d.description,
        parameters: d.parameters,
        created_by: d.created_by.map(|u| u.to_string()),
        created_by_name: names.0,
        created_at: d.created_at,
        updated_by: d.updated_by.map(|u| u.to_string()),
        updated_by_name: names.1,
        updated_at: d.updated_at,
        folder_id: d.folder_id.map(|u| u.to_string()),
        width: d.width,
        deleted_at: d.deleted_at,
        bookmarked,
        tabs: tabs.into_iter().map(dashboard_tab_dto).collect(),
        cards: cards.into_iter().map(dashboard_card_dto).collect(),
    }
}
```

In `crates/egret-bi/src/routes.rs`, replace the tail of `load_dashboard` (`:2934-2942`):

```rust
    let bookmarked = st
        .dashboards
        .is_bookmarked(user, id)
        .await
        .map_err(BiApiError::from)?;
    let names = st
        .dashboards
        .attribution_names(id)
        .await
        .map_err(BiApiError::from)?;
    Ok(crate::model::dashboard_detail_dto(
        d, tabs, cards, bookmarked, names,
    ))
```

And update the one `update` call site, at `:2710-2711`:

```rust
    st.dashboards
        .update(
            id,
            &b.name,
            &b.description,
            &b.parameters,
            &b.width,
            Some(ctx.principal.user_id.0),
        )
        .await
        .map_err(BiApiError::from)?;
```

- [ ] **Step 6: Run the test to verify it passes**

```bash
cargo test -p egret-bi --test dashboard_attribution
```

Expected: PASS, 3 tests (or three `skip` lines if `DATABASE_URL` is unset — in which case export it
and re-run; a skipped test has verified nothing).

- [ ] **Step 7: Prove the tests survive mutation**

Three mutations, each aimed at one assertion:

1. In `attribution_names`, drop the `NULLIF(cu.name, '')` and use plain `COALESCE(cu.name, …)`.
   Re-run. Expected: `attribution_names_prefer_the_profile_name_and_fall_back_to_the_email` FAILS
   (the nameless user resolves to `""`, not the email).
2. In `update`, delete `updated_by=$6,` from the SQL and the matching `.bind`. Re-run. Expected:
   `new_dashboards_stamp_created_at_and_updates_stamp_updated_by` FAILS.
3. In migration 0106, move `DEFAULT now()` up into the `ADD COLUMN`. Drop the columns, re-run
   `just migrate`, and re-run Task 1 Step 4's query. Expected: `with_created_at == total` — the
   backfill this migration exists to avoid.

Restore all three. This step is not optional; six assertions in the chart programme passed against
a stubbed implementation and only this caught them.

- [ ] **Step 8: Run the backend gates**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

These are the three lines of `just check` (`justfile:14-17`) run individually so a failure names
itself. `cargo fmt` is a real CI gate here — do not skip it.

- [ ] **Step 9: Commit**

```bash
git add crates/egret-bi/src/dashboard.rs \
        crates/egret-analytics-api/src/bi.rs \
        crates/egret-bi/src/model.rs \
        crates/egret-bi/src/routes.rs \
        crates/egret-bi/tests/dashboard_attribution.rs
git commit -m "feat(dashboard): resolve dashboard owner and last editor to display names on the detail DTO"
```

---

## Task 3: The sidebar says "August 1, 2026 by Ref User"

**Files:**
- Create: `frontend/src/components/chart/dashboard/attribution.ts`
- Create: `frontend/src/components/chart/dashboard/__tests__/attribution.test.ts`
- Modify: `frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx`
- Modify: `frontend/src/components/chart/dashboard/__tests__/DashboardInfoSidebar.test.tsx`
- Modify: `frontend/src/api/dashboards.ts`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/attribution.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { attributionLine } from "../attribution";

describe("attributionLine", () => {
  // 12:00 UTC on purpose: the local calendar day for that instant is Aug 1 or Aug 2 in every
  // timezone on earth (UTC-12 … UTC+14), so the month and year can be pinned without pinning the
  // test to the machine's TZ. Asserting a hard-coded "August 1, 2026" would be green in CI and
  // red for anyone west of UTC-12... and asserting toLocaleDateString() against itself would
  // assert nothing at all.
  const iso = "2026-08-01T12:00:00Z";

  it("renders the date and the name, Metabase's wording", () => {
    expect(attributionLine(iso, "Ref User")).toMatch(/^August [12], 2026 by Ref User$/);
  });

  it("drops the 'by' clause when there is no name rather than printing 'by null'", () => {
    const out = attributionLine(iso, null);
    expect(out).toMatch(/^August [12], 2026$/);
    expect(out).not.toContain("by");
  });

  it("renders nothing at all when there is no date", () => {
    // A dashboard predating migration 0106 has created_at NULL. That is a gap in what was
    // recorded, not a fact about the dashboard, so the row is omitted — not shown as "—" beside
    // a name that would then look like it belonged to a date.
    expect(attributionLine(null, "Ref User")).toBeNull();
  });

  it("renders nothing for an unparseable date", () => {
    expect(attributionLine("not-a-date", "Ref User")).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/attribution.test.ts
```

Expected: FAIL — `Failed to resolve import "../attribution"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/attribution.ts`:

```ts
/** One line of Metabase's info-sidebar attribution: "August 1, 2026 by Ref User".
 *
 *  Returns null when there is no date. `created_at` is NULL for every dashboard predating
 *  migration 0106, which deliberately does not invent one; a row rendered as "— by Ref User"
 *  would read as a fact about the dashboard rather than a gap in what was recorded.
 *
 *  The name is dropped rather than defaulted when absent: `created_by` can be NULL because the
 *  user was hard-deleted (0093 sets the FK NULL to preserve the content), and "by Unknown" is a
 *  worse answer than the date on its own. */
export function attributionLine(iso: string | null, name: string | null): string | null {
  if (!iso) return null;
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  const date = d.toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
  return name ? `${date} by ${name}` : date;
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/attribution.test.ts
```

Expected: PASS, 4 tests.

- [ ] **Step 5: Prove the test survives mutation**

Change the last line to `return `${date} by ${name ?? "Unknown"}`;` and re-run. Expected: the
"drops the 'by' clause" test FAILS. Then change the guard to `if (!iso) return "";` and re-run.
Expected: the "renders nothing at all" test FAILS. Restore both.

- [ ] **Step 6: Widen the sidebar's props and render the two lines**

Replace `frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx` in full:

```tsx
import { useEffect, useState } from "react";
import { attributionLine } from "./attribution";

interface Props {
  name: string;
  description: string;
  /** Display name for the creator, already resolved by the API (`created_by_name`); falls back
   *  to the raw id, exactly as ShareDialog.tsx:316 does for a grant's principal. */
  ownerLabel: string | null;
  createdAt: string | null;
  editorLabel: string | null;
  updatedAt: string;
  onSaveDescription: (next: string) => void;
  onClose: () => void;
}

export function DashboardInfoSidebar({
  name, description, ownerLabel, createdAt, editorLabel, updatedAt, onSaveDescription, onClose,
}: Props) {
  const [draft, setDraft] = useState(description);
  // Re-sync if the description changes underneath us (e.g. a concurrent edit refetch);
  // [description] compares by value, so this only fires on a genuine external change.
  useEffect(() => { setDraft(description); }, [description]);
  const added = attributionLine(createdAt, ownerLabel);
  const edited = attributionLine(updatedAt, editorLabel ?? ownerLabel);
  return (
    <aside className="sticky top-4 max-h-[calc(100vh-7rem)] w-[300px] shrink-0 self-start overflow-auto rounded-lg border bg-card p-3 text-sm">
      <div className="mb-2 flex items-center justify-between">
        <div className="font-medium">{name}</div>
        <button type="button" aria-label="close info" className="text-muted-foreground hover:text-foreground" onClick={onClose}>✕</button>
      </div>
      <label className="mb-3 flex flex-col gap-1 text-xs text-muted-foreground">Description
        <textarea aria-label="description" className="h-24 w-full rounded border bg-background px-2 py-1 text-sm"
          value={draft} onChange={(e) => setDraft(e.target.value)}
          onBlur={() => { if (draft !== description) onSaveDescription(draft); }} />
      </label>
      <dl className="flex flex-col gap-1 text-xs">
        {added && (
          <div className="flex justify-between gap-2"><dt className="shrink-0 text-muted-foreground">Added</dt><dd className="text-right">{added}</dd></div>
        )}
        {edited && (
          <div className="flex justify-between gap-2"><dt className="shrink-0 text-muted-foreground">Last edited</dt><dd className="text-right">{edited}</dd></div>
        )}
      </dl>
    </aside>
  );
}
```

`editorLabel ?? ownerLabel` is deliberate: `updated_by` is NULL for every dashboard that has not
been saved since migration 0106, and in that state the last person known to have touched it is its
creator. That is what Metabase shows too — its `last-edit-info` falls back to the creator.

- [ ] **Step 7: Update the sidebar's existing tests**

In `frontend/src/components/chart/dashboard/__tests__/DashboardInfoSidebar.test.tsx`, replace the
`base` fixture and the first test; the other two are untouched:

```tsx
  const base = {
    name: "Sales",
    description: "old",
    ownerLabel: "Ref User",
    createdAt: "2026-08-01T12:00:00Z",
    editorLabel: "Other User",
    updatedAt: "2026-08-05T12:00:00Z",
  };
  it("shows both attribution lines by name and renders the description", () => {
    render(<DashboardInfoSidebar {...base} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText(/^August [12], 2026 by Ref User$/)).toBeTruthy();
    expect(screen.getByText(/^August [56], 2026 by Other User$/)).toBeTruthy();
    expect((screen.getByLabelText("description") as HTMLTextAreaElement).value).toBe("old");
  });
  it("falls back to the creator when nothing has edited the dashboard since 0105", () => {
    render(<DashboardInfoSidebar {...base} editorLabel={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText(/^August [56], 2026 by Ref User$/)).toBeTruthy();
  });
  it("omits the Added row for a dashboard with no recorded creation date", () => {
    render(<DashboardInfoSidebar {...base} createdAt={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.queryByText("Added")).toBeNull();
  });
```

- [ ] **Step 8: Widen the frontend type and wire the route**

In `frontend/src/api/dashboards.ts`, extend `DashboardDetail` (`:67-76`):

```ts
export interface DashboardDetail {
  id: string; name: string; description: string;
  created_by: string | null; created_by_name: string | null;
  created_at: string | null;
  updated_by: string | null; updated_by_name: string | null;
  updated_at: string;
  folder_id?: string | null;
  parameters: DashboardParameter[];
  tabs: DashboardTab[];
  width: "fixed" | "full";
  cards: DashboardCard[];
  deleted_at?: string | null;
  bookmarked?: boolean;
}
```

In `frontend/src/routes/explore/DashboardView.tsx`, replace the `<DashboardInfoSidebar>` props at
`:688-695`:

```tsx
          <DashboardInfoSidebar
            name={dash.name}
            description={dash.description}
            ownerLabel={dash.created_by_name ?? dash.created_by}
            createdAt={dash.created_at ?? null}
            editorLabel={dash.updated_by_name ?? dash.updated_by}
            updatedAt={dash.updated_at}
            onClose={() => setShowInfo(false)}
            onSaveDescription={saveDescription}
          />
```

`?? dash.created_by` is the raw-id fallback and is the same expression `ShareDialog.tsx:316` uses.
It fires only when the JOIN found no user row, which after 0093's `ON DELETE SET NULL` means the
id itself is NULL too — so in practice this renders the "no name" branch, not a UUID.

- [ ] **Step 9: Run the frontend gates**

Each as its own command. Never chain them through a pipe and never pipe into `rtk` — a swallowed
pipe breaks the `&&` behind it and the later gates silently never run.

```bash
cd frontend && npm run lint
cd frontend && npx tsc --noEmit
cd frontend && npm run test
```

Expected: all green. Record the test count.

- [ ] **Step 10: Commit**

```bash
git add frontend/src/components/chart/dashboard/attribution.ts \
        frontend/src/components/chart/dashboard/__tests__/attribution.test.ts \
        frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardInfoSidebar.test.tsx \
        frontend/src/api/dashboards.ts \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): show the info sidebar's owner and last editor as names with dates"
```

---

## Task 4: "Saved in" links to the folder's section on the list

**Files:**
- Modify: `frontend/src/components/explore/folders.ts`
- Create: `frontend/src/components/explore/__tests__/folders.test.tsx`
- Modify: `frontend/src/routes/explore/Dashboards.tsx`
- Modify: `frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

There is no folder route (see "Three places…" above), so the link goes to the dashboards list with
`?folder=<id>` and the list expands that section on arrival. Without the expand half, a link into a
collapsed folder lands on a page where the dashboard you came from is not visible, which is worse
than no link.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/explore/__tests__/folders.test.tsx`:

```tsx
import { describe, it, expect, beforeEach } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { useCollapsed } from "../folders";

beforeEach(() => localStorage.clear());

describe("useCollapsed.expand", () => {
  it("un-collapses a collapsed key and persists that", () => {
    localStorage.setItem("k", JSON.stringify(["f1", "f2"]));
    const { result } = renderHook(() => useCollapsed("k"));
    expect(result.current.isCollapsed("f1")).toBe(true);
    act(() => result.current.expand("f1"));
    expect(result.current.isCollapsed("f1")).toBe(false);
    expect(result.current.isCollapsed("f2")).toBe(true);
    // Persisted, not just in state: the deep link is followed once, and the section must still be
    // open after the reload that a "Saved in" click is often followed by.
    expect(JSON.parse(localStorage.getItem("k")!)).toEqual(["f2"]);
  });

  it("is a no-op on a key that is already expanded, and does not rewrite storage", () => {
    localStorage.setItem("k", JSON.stringify(["f2"]));
    const { result } = renderHook(() => useCollapsed("k"));
    act(() => result.current.expand("f1"));
    expect(result.current.isCollapsed("f1")).toBe(false);
    expect(JSON.parse(localStorage.getItem("k")!)).toEqual(["f2"]);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd frontend && npx vitest run src/components/explore/__tests__/folders.test.tsx
```

Expected: FAIL — `result.current.expand is not a function`.

- [ ] **Step 3: Add `expand` to the hook**

In `frontend/src/components/explore/folders.ts`, insert after `toggle` (`:48`) and widen the
return:

```ts
  /** Force one key open, idempotently. Used by the "Saved in" deep link: arriving at
   *  ?folder=<id> must reveal that section even when the visitor last left it collapsed.
   *  Returns the same Set reference when nothing changes, so it cannot loop a caller's effect. */
  const expand = useCallback((key: string) => {
    setSet((s) => {
      if (!s.has(key)) return s;
      const n = new Set(s);
      n.delete(key);
      try { localStorage.setItem(storageKey, JSON.stringify([...n])); } catch { /* ignore */ }
      return n;
    });
  }, [storageKey]);
  return { isCollapsed, toggle, expand };
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd frontend && npx vitest run src/components/explore/__tests__/folders.test.tsx
```

Expected: PASS, 2 tests.

- [ ] **Step 5: Prove the test survives mutation**

Delete the `if (!s.has(key)) return s;` early return and re-run. Expected: the second test FAILS
(storage is rewritten on a no-op). Then change `n.delete(key)` to `n.add(key)` and re-run. Expected:
the first test FAILS. Restore both.

- [ ] **Step 6: Honour `?folder=` on the list page**

In `frontend/src/routes/explore/Dashboards.tsx`, add `useSearchParams` to the existing
`react-router-dom` import, and after the `collapse` hook (`:36`):

```tsx
  const [searchParams] = useSearchParams();
  const focusFolder = searchParams.get("folder");
  // The info sidebar's "Saved in" link lands here. Reveal that section once on arrival; the
  // visitor's own collapse choices afterwards are theirs and are not re-applied.
  useEffect(() => { if (focusFolder) collapse.expand(focusFolder); }, [focusFolder, collapse]);
```

- [ ] **Step 7: Render the row in the sidebar**

In `frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx`, add the import and two props:

```tsx
import { Link } from "react-router-dom";
```

```tsx
  /** Folder name, or null for Uncategorized. */
  folderName: string | null;
  /** Where "Saved in" points. There is no folder route in this app — this is the dashboards list
   *  scoped with ?folder=<id>, which the list uses to expand that section. */
  folderHref: string | null;
```

and the row, as the first entry of the `<dl>`:

```tsx
        <div className="flex justify-between gap-2">
          <dt className="shrink-0 text-muted-foreground">Saved in</dt>
          <dd className="truncate text-right">
            {folderName && folderHref
              ? <Link className="hover:underline" to={folderHref}>{folderName}</Link>
              : "Uncategorized"}
          </dd>
        </div>
```

Add `folderName`/`folderHref` to the destructured parameter list.

- [ ] **Step 8: Update the sidebar tests for the router**

The sidebar now renders a `<Link>`, which throws outside a router. In
`__tests__/DashboardInfoSidebar.test.tsx`, add the import and a wrapper, and use it in every
`render` call in the file:

```tsx
import { MemoryRouter } from "react-router-dom";

const wrap = (ui: React.ReactElement) => render(<MemoryRouter>{ui}</MemoryRouter>);
```

Extend `base` with `folderName: "Ops", folderHref: "/explore/demo/dashboards?folder=f1"`, and add:

```tsx
  it("links Saved in at the folder's section on the list", () => {
    wrap(<DashboardInfoSidebar {...base} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    const link = screen.getByRole("link", { name: "Ops" });
    expect(link.getAttribute("href")).toBe("/explore/demo/dashboards?folder=f1");
  });

  it("says Uncategorized, with no link, for a dashboard in no folder", () => {
    wrap(<DashboardInfoSidebar {...base} folderName={null} folderHref={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText("Uncategorized")).toBeTruthy();
    expect(screen.queryByRole("link")).toBeNull();
  });
```

- [ ] **Step 9: Wire the route**

In `frontend/src/routes/explore/DashboardView.tsx`, add the folders hook beside the other data hooks
(next to `useCharts` at `:60`):

```tsx
  const { data: dashboardFolders = [] } = useFolders(ws, "dashboards");
```

with `import { useFolders } from "@/api/folders";`, and add the two props at the
`<DashboardInfoSidebar>` call site:

```tsx
            folderName={dashboardFolders.find((f) => f.id === dash.folder_id)?.name ?? null}
            folderHref={dash.folder_id ? `/explore/${ws}/dashboards?folder=${dash.folder_id}` : null}
```

- [ ] **Step 10: Run the frontend gates and commit**

```bash
cd frontend && npm run lint
cd frontend && npx tsc --noEmit
cd frontend && npm run test
```

```bash
git add frontend/src/components/explore/folders.ts \
        frontend/src/components/explore/__tests__/folders.test.tsx \
        frontend/src/routes/explore/Dashboards.tsx \
        frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardInfoSidebar.test.tsx \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): show the dashboard's folder in the info sidebar and link to its section"
```

**Items 1 and 2 are now complete and independently shippable.** Everything below is the expensive
third; if the programme is trimmed here, nothing above is left half-built.

---

## Task 5: The revisions table and the repository that bounds it

**Files:**
- Create: `migrations/0107_dashboard_revisions.sql`
- Modify: `crates/egret-metastore/src/lib.rs`
- Modify: `crates/egret-bi/src/dashboard.rs`
- Modify: `crates/egret-bi/src/lib.rs`
- Test: `crates/egret-bi/tests/dashboard_revisions.rs`

- [ ] **Step 1: Write the migration**

Create `migrations/0107_dashboard_revisions.sql`:

```sql
-- Dashboard revision history: one immutable snapshot per successful dashboard PUT.
--
-- WHAT A REVISION IS. `snapshot` holds exactly the body of PUT /ws/{ws}/dashboards/{id} — the
-- `UpdateDashboard` DTO (egret-analytics-api/src/bi.rs): name, description, parameters, width,
-- tabs [{name, position}] and cards [{chart_id, card_type, grid_col, grid_row, size_x, size_y,
-- options, tab_index}]. That DTO is the complete, ID-FREE description of everything a dashboard
-- edit can change. Complete, because db_update writes exactly two things from it and there is no
-- partial-update path — the PUT is a whole-layout replace (dashboard.rs:3-5). ID-free, because
-- replace_layout DELETEs and re-INSERTs both the tabs and the cards table on every save and mints
-- fresh ids each time, so tabs carry no id in the DTO and cards reference their tab by INDEX.
--
-- ID-FREE IS WHAT MAKES REVERT CHEAP. There are no ids inside a snapshot that could collide with
-- the ids currently in the tables, so nothing has to be remapped: revert deserialises the snapshot
-- back into an UpdateDashboard and runs the ordinary update path. DashboardRepo::duplicate needs
-- its tab_map for the opposite reason — it copies ROWS, and rows carry ids.
--
-- WHAT IS DELIBERATELY NOT IN A SNAPSHOT: folder_id, deleted_at, created_by. Those are changed by
-- other endpoints (PATCH .../folder, POST .../trash) and are not part of a layout edit. Reverting
-- a layout must not silently move the dashboard back into a folder someone has since deleted, or
-- pull it out of the trash.
--
-- WHY NOT audit.audit_log (0076). It is the only audit-like table in the repo and it is the wrong
-- vehicle twice over. Its writes go through a bounded channel that DROPS on backpressure
-- (AuditSink, egret-audit/src/lib.rs:55-71), so the history would have silent holes; and its
-- `detail` column carries no entity content, so nothing in that table can reconstruct a dashboard.
-- Revisions are written synchronously on the request path and must be complete enough to restore.
--
-- RETENTION IS COUNT-BASED, NOT AGE-BASED, and is applied inline at insert. audit_log prunes by
-- age (ts < now() - interval) because it is one global firehose where age is the only axis. A
-- per-entity history has the opposite shape: under a 90-day rule a dashboard saved 400 times in an
-- afternoon keeps all 400, and a dashboard saved twice a year keeps NONE — the second being
-- exactly the dashboard whose history matters most. Keeping the newest N per dashboard bounds the
-- worst case AND guarantees the rarely-edited dashboard a usable history. N = 20; see
-- DashboardRepo::MAX_REVISIONS_PER_DASHBOARD for why 20 and not Metabase's 15.
CREATE TABLE analytics.dashboard_revisions (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id uuid NOT NULL REFERENCES analytics.dashboards(id) ON DELETE CASCADE,
    created_by   uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    -- 'edit'     a PUT
    -- 'revert'   a PUT produced by reverting to an earlier revision
    -- 'baseline' the pre-feature state, captured once on a dashboard's first PUT so that the very
    --            first edit after this ships is still undoable. Mirrors Metabase's is_creation /
    --            is_reversion flags, which exist for the same two reasons.
    kind         text NOT NULL DEFAULT 'edit' CHECK (kind IN ('baseline','edit','revert')),
    snapshot     jsonb NOT NULL
);
CREATE INDEX dashboard_revisions_dash_idx ON analytics.dashboard_revisions (dashboard_id, created_at DESC, id DESC);
```

- [ ] **Step 2: Force the migrator to re-embed**

In `crates/egret-metastore/src/lib.rs`, below the 0106 line added in Task 1:

```rust
// re-embed: 0107 dashboard_revisions (per-dashboard snapshot history, count-bounded at 20)
```

- [ ] **Step 3: Write the failing test**

Create `crates/egret-bi/tests/dashboard_revisions.rs`:

```rust
//! DB-gated dashboard revision tests. Self-skip without DATABASE_URL.
use egret_bi::{DashboardRepo, NewCard, NewTab};
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(DashboardRepo, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    Some((DashboardRepo::new(db.clone()), ws, uid))
}

fn card(tab_index: Option<usize>, col: i32) -> NewCard {
    NewCard {
        chart_id: None,
        card_type: "text".into(),
        grid_col: col,
        grid_row: 0,
        size_x: 6,
        size_y: 4,
        options: serde_json::json!({ "text": "hi" }),
        tab_index,
    }
}

#[tokio::test]
async fn snapshot_current_is_a_put_body_with_tab_indices_not_tab_ids() {
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Snap", "desc", Some(uid)).await.unwrap();
    repo.replace_layout(
        d.id,
        &[
            NewTab { name: "A".into(), position: 0 },
            NewTab { name: "B".into(), position: 1 },
        ],
        &[card(Some(1), 3), card(Some(0), 0)],
    )
    .await
    .unwrap();

    let snap = repo.snapshot_current(d.id).await.unwrap().unwrap();
    assert_eq!(snap["name"], "Snap");
    assert_eq!(snap["description"], "desc");
    assert_eq!(snap["width"], "full");
    assert_eq!(snap["tabs"], serde_json::json!([
        { "name": "A", "position": 0 },
        { "name": "B", "position": 1 },
    ]));
    // The point of the whole design: no uuid anywhere in the snapshot, so nothing has to be
    // remapped on the way back in. Serialising the value and searching it is a blunt check and
    // that is exactly why it is used — it catches an id leaking in from ANY field, not just the
    // fields this test thought to name.
    let text = snap.to_string();
    assert!(!text.contains(&d.id.to_string()), "dashboard id leaked into the snapshot");
    for c in snap["cards"].as_array().unwrap() {
        assert!(c.get("id").is_none(), "card id leaked into the snapshot: {c}");
        assert!(c["tab_index"].is_number(), "cards must reference a tab by index: {c}");
    }
}

#[tokio::test]
async fn insert_revision_keeps_only_the_newest_twenty_per_dashboard() {
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Trim", "", Some(uid)).await.unwrap();
    for i in 0..25 {
        repo.insert_revision(
            d.id,
            Some(uid),
            "edit",
            &serde_json::json!({ "name": format!("v{i}") }),
            None,
        )
        .await
        .unwrap();
    }
    let revs = repo.list_revisions(d.id, 100).await.unwrap();
    assert_eq!(revs.len(), 20, "retention is a cap, not a suggestion");
    // Newest first, and it is the OLDEST five that were dropped.
    assert_eq!(revs[0].snapshot["name"], "v24");
    assert_eq!(revs[19].snapshot["name"], "v5");
}

#[tokio::test]
async fn a_revision_is_only_reachable_through_its_own_dashboard() {
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let a = repo.create(ws, "A", "", Some(uid)).await.unwrap();
    let b = repo.create(ws, "B", "", Some(uid)).await.unwrap();
    let rev = repo
        .insert_revision(a.id, Some(uid), "edit", &serde_json::json!({ "name": "A" }), None)
        .await
        .unwrap();
    assert!(repo.revision(a.id, rev.id).await.unwrap().is_some());
    // Scoped lookup, not a bare `WHERE id = $1`. An unscoped read is the IDOR shape: a caller
    // authorised on dashboard B could revert B to a snapshot of A.
    assert!(repo.revision(b.id, rev.id).await.unwrap().is_none());
}

#[tokio::test]
async fn revisions_cascade_when_the_dashboard_is_deleted() {
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Gone", "", Some(uid)).await.unwrap();
    repo.insert_revision(d.id, Some(uid), "edit", &serde_json::json!({}), None)
        .await
        .unwrap();
    assert_eq!(repo.revision_count(d.id).await.unwrap(), 1);
    repo.delete(d.id).await.unwrap();
    assert_eq!(repo.revision_count(d.id).await.unwrap(), 0);
}
```

- [ ] **Step 4: Run the test to verify it fails**

```bash
cargo test -p egret-bi --test dashboard_revisions
```

Expected: FAIL to compile — `no method snapshot_current`, `insert_revision`, `list_revisions`,
`revision`, `revision_count` on `DashboardRepo`.

- [ ] **Step 5: Write the repository methods**

In `crates/egret-bi/src/dashboard.rs`, add the row type next to `DashboardTab` (`:51`):

```rust
/// One stored dashboard revision. `snapshot` is an `UpdateDashboard`-shaped JSON value — see
/// migration 0107 for why that shape and not a row copy.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DashboardRevision {
    pub id: Uuid,
    pub dashboard_id: Uuid,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub kind: String,
    pub snapshot: serde_json::Value,
}
```

Add the column list and the cap next to `CARD_COLS` (`:92-93`):

```rust
const REV_COLS: &str = "id, dashboard_id, created_by, created_at, kind, snapshot";

/// How many revisions one dashboard keeps.
///
/// Metabase keeps 15 (`revision/max-revisions`). egret keeps 20 for a shape reason rather than a
/// taste one: Metabase writes a revision per small action — add card, move card, rename tab are
/// each their own API call — so fifteen Metabase revisions can be fifteen minutes of a single
/// session. egret has exactly one write path, the whole-layout PUT behind the Save button, so one
/// egret revision is a whole editing session and twenty of them is months of calendar history.
const MAX_REVISIONS_PER_DASHBOARD: i64 = 20;
```

Then, at the end of `impl DashboardRepo` (after `duplicate`), the five methods:

```rust
    /// The dashboard's CURRENT state, shaped exactly like a PUT body so it can be fed straight
    /// back through the update path. Tab ids become indices — the inverse of `replace_layout`'s
    /// index→id mapping — and card ids are dropped. That is what makes the value id-free, which
    /// is what makes revert a re-PUT rather than a graft. `None` if the dashboard is gone.
    pub async fn snapshot_current(&self, id: Uuid) -> Result<Option<serde_json::Value>, BiError> {
        let Some(d) = self.get(id).await? else {
            return Ok(None);
        };
        let tabs = self.tabs(id).await?;
        let cards = self.cards(id).await?;
        let index_of: std::collections::HashMap<Uuid, usize> =
            tabs.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        Ok(Some(serde_json::json!({
            "name": d.name,
            "description": d.description,
            "parameters": d.parameters,
            "width": d.width,
            // `tabs()` orders by (position, id), so enumerate both indexes the cards and
            // re-normalises position to 0..n-1 — the same thing the frontend's serializeLayout
            // does, so a snapshot and a hand-made save are the same shape.
            "tabs": tabs.iter().enumerate()
                .map(|(i, t)| serde_json::json!({ "name": t.name, "position": i as i32 }))
                .collect::<Vec<_>>(),
            "cards": cards.iter().map(|c| serde_json::json!({
                "chart_id": c.chart_id,
                "card_type": c.card_type,
                "grid_col": c.grid_col,
                "grid_row": c.grid_row,
                "size_x": c.size_x,
                "size_y": c.size_y,
                "options": c.options,
                "tab_index": c.tab_id.and_then(|t| index_of.get(&t).copied()),
            })).collect::<Vec<_>>(),
        })))
    }

    /// Append a revision and trim the dashboard back to `MAX_REVISIONS_PER_DASHBOARD`, in one
    /// transaction so a reader can never observe the table over its cap.
    ///
    /// `at` overrides the timestamp, and exists for exactly one caller: the 'baseline' revision,
    /// whose content is the state as of the dashboard's `updated_at`, not as of now. Everything
    /// else passes `None` and gets the column default.
    pub async fn insert_revision(
        &self,
        dashboard_id: Uuid,
        created_by: Option<Uuid>,
        kind: &str,
        snapshot: &serde_json::Value,
        at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<DashboardRevision, BiError> {
        let mut tx = self.db.pool.begin().await?;
        let rev = sqlx::query_as::<_, DashboardRevision>(&format!(
            "INSERT INTO analytics.dashboard_revisions \
             (dashboard_id, created_by, kind, snapshot, created_at) \
             VALUES ($1,$2,$3,$4, COALESCE($5, now())) RETURNING {REV_COLS}"
        ))
        .bind(dashboard_id)
        .bind(created_by)
        .bind(kind)
        .bind(snapshot)
        .bind(at)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM analytics.dashboard_revisions \
             WHERE dashboard_id = $1 AND id NOT IN ( \
               SELECT id FROM analytics.dashboard_revisions \
                WHERE dashboard_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2)",
        )
        .bind(dashboard_id)
        .bind(MAX_REVISIONS_PER_DASHBOARD)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rev)
    }

    /// A dashboard's revisions, newest first. `limit` is clamped to the retention cap, since
    /// nothing beyond it can exist.
    pub async fn list_revisions(
        &self,
        dashboard_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DashboardRevision>, BiError> {
        let sql = format!(
            "SELECT {REV_COLS} FROM analytics.dashboard_revisions \
             WHERE dashboard_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2"
        );
        Ok(sqlx::query_as::<_, DashboardRevision>(&sql)
            .bind(dashboard_id)
            .bind(limit.clamp(1, MAX_REVISIONS_PER_DASHBOARD))
            .fetch_all(&self.db.pool)
            .await?)
    }

    /// One revision, SCOPED to its dashboard. The scope is the point: an unscoped `WHERE id = $1`
    /// would let a caller authorised on dashboard B revert B to a snapshot of A, which is the
    /// nested-resource ownership hole this repo checks for everywhere else.
    pub async fn revision(
        &self,
        dashboard_id: Uuid,
        revision_id: Uuid,
    ) -> Result<Option<DashboardRevision>, BiError> {
        let sql = format!(
            "SELECT {REV_COLS} FROM analytics.dashboard_revisions WHERE dashboard_id = $1 AND id = $2"
        );
        Ok(sqlx::query_as::<_, DashboardRevision>(&sql)
            .bind(dashboard_id)
            .bind(revision_id)
            .fetch_optional(&self.db.pool)
            .await?)
    }

    /// How many revisions a dashboard has. Used to decide whether a baseline is still owed.
    pub async fn revision_count(&self, dashboard_id: Uuid) -> Result<i64, BiError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM analytics.dashboard_revisions WHERE dashboard_id = $1",
        )
        .bind(dashboard_id)
        .fetch_one(&self.db.pool)
        .await?)
    }
```

Export the new type: in `crates/egret-bi/src/lib.rs`, add `DashboardRevision` to the existing
`pub use crate::dashboard::{…}` list (beside `DashboardRepo`, `NewCard`, `NewTab`).

- [ ] **Step 6: Run the test to verify it passes**

```bash
just migrate
cargo test -p egret-bi --test dashboard_revisions
```

Expected: PASS, 4 tests.

- [ ] **Step 7: Prove the tests survive mutation**

1. Change `MAX_REVISIONS_PER_DASHBOARD` to `25`. Re-run. Expected:
   `insert_revision_keeps_only_the_newest_twenty_per_dashboard` FAILS with `25 != 20`.
2. Flip the trim's `ORDER BY created_at DESC, id DESC` to `ASC, id ASC`. Re-run. Expected: the same
   test FAILS on `revs[0].snapshot["name"]` — the *newest* five were dropped.
3. Change `revision`'s SQL to `WHERE id = $2` (dropping the dashboard scope). Re-run. Expected:
   `a_revision_is_only_reachable_through_its_own_dashboard` FAILS. This is the IDOR test; if it
   does not go red here, it is verifying nothing.
4. In `snapshot_current`, add `"id": c.id` to the card object. Re-run. Expected:
   `snapshot_current_is_a_put_body_with_tab_indices_not_tab_ids` FAILS.

Restore all four.

- [ ] **Step 8: Run the backend gates and commit**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

```bash
git add migrations/0107_dashboard_revisions.sql \
        crates/egret-metastore/src/lib.rs \
        crates/egret-bi/src/dashboard.rs \
        crates/egret-bi/src/lib.rs \
        crates/egret-bi/tests/dashboard_revisions.rs
git commit -m "feat(dashboard): a count-bounded per-dashboard revision store"
```

---

## Task 6: Every successful PUT records a revision, and no failed one does

**Files:**
- Modify: `crates/egret-analytics-api/src/bi.rs`
- Modify: `crates/egret-bi/src/routes.rs`
- Test: `crates/egret-analytics-api/src/bi.rs` (inline), `crates/egret-bi/tests/dashboard_revisions.rs`

- [ ] **Step 1: Write the failing serde round-trip test**

The whole design depends on a snapshot being a faithful `UpdateDashboard`. Prove the round-trip
rather than assuming it. Append to the existing `#[cfg(test)] mod tests` in
`crates/egret-analytics-api/src/bi.rs` (the block that begins at `:559`):

```rust
    #[test]
    fn update_dashboard_round_trips_through_json_unchanged() {
        // A revision is stored by serialising this DTO and restored by deserialising it. If the
        // two directions disagree about ANY field, a revert silently restores a different
        // dashboard than the one the user picked — with no error at any layer.
        let body = serde_json::json!({
            "name": "Ops",
            "description": "d",
            "parameters": [{ "id": "p1", "name": "Cat", "slug": "cat", "type": "category" }],
            "width": "fixed",
            "tabs": [{ "name": "A", "position": 0 }],
            "cards": [{
                "chart_id": "6f9619ff-8b86-d011-b42d-00c04fc964ff",
                "card_type": "chart",
                "grid_col": 3, "grid_row": 1, "size_x": 6, "size_y": 4,
                "options": { "vizOverride": { "cartesian.stack": true } },
                "tab_index": 0
            }]
        });
        let dto: UpdateDashboard = serde_json::from_value(body.clone()).expect("deserialises");
        assert_eq!(serde_json::to_value(&dto).unwrap(), body);
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p egret-analytics-api update_dashboard_round_trips
```

Expected: FAIL to compile — `UpdateDashboard: Serialize is not satisfied`.

- [ ] **Step 3: Make the PUT DTOs serialisable**

In `crates/egret-analytics-api/src/bi.rs`, add `Serialize` to three derives — `UpdateDashboard`
(`:418`), `CardInput` (`:383`) and `NewTabInput` (`:399`):

```rust
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
```

with, on `UpdateDashboard`, the reason:

```rust
/// The dashboard PUT body — and, byte for byte, one stored revision (migration 0107). `Serialize`
/// is derived alongside `Deserialize` so the snapshot is produced from this exact type rather than
/// hand-built JSON that could drift from it; a compile error is how a new field announces that
/// revisions must carry it too.
```

- [ ] **Step 4: Run it to verify it passes**

```bash
cargo test -p egret-analytics-api update_dashboard_round_trips
```

Expected: PASS, 1 test.

- [ ] **Step 5: Prove it survives mutation**

Add `#[serde(skip_serializing)]` to `UpdateDashboard::width` and re-run. Expected: FAIL. Restore.

- [ ] **Step 6: Extract the one write path**

In `crates/egret-bi/src/routes.rs`, add these three functions immediately above `db_update`
(`:2693`):

```rust
/// Best-effort revision write. A failure here must NOT fail the request: the dashboard has already
/// been updated, so a 500 would tell the client the save was lost when it was not. Same stance as
/// the audit sink (see the `audit` field on `BiState`) — log and continue.
async fn capture_revision(
    st: &BiState,
    id: Uuid,
    actor: Option<Uuid>,
    kind: &str,
    snapshot: &serde_json::Value,
    at: Option<chrono::DateTime<chrono::Utc>>,
) {
    if let Err(e) = st
        .dashboards
        .insert_revision(id, actor, kind, snapshot, at)
        .await
    {
        tracing::warn!(dashboard = %id, kind, error = %e, "failed to record dashboard revision");
    }
}

/// The state a dashboard is about to leave, for dashboards that have no history yet.
///
/// READS ONLY. The value is held in memory and inserted by `apply_dashboard_update` after the
/// write lands: snapshotting afterwards would capture the NEW state, and inserting beforehand
/// would leave a restore point for an edit that then got rejected. Without this, the first edit
/// after this feature ships is the one edit that cannot be undone — which is the edit a user is
/// most likely to want back.
///
/// Attributed to whoever last touched the dashboard and stamped with that moment, not with now(),
/// because that is when the state being captured was actually written.
async fn pending_baseline(
    st: &BiState,
    id: Uuid,
) -> Option<(
    serde_json::Value,
    Option<Uuid>,
    chrono::DateTime<chrono::Utc>,
)> {
    if st.dashboards.revision_count(id).await.ok()? != 0 {
        return None;
    }
    let d = st.dashboards.get(id).await.ok()??;
    let snap = st.dashboards.snapshot_current(id).await.ok()??;
    Some((snap, d.updated_by.or(d.created_by), d.updated_at))
}

/// The one write path a dashboard layout takes. `db_update` and `db_revision_revert` both go
/// through it, so a revert can never take a different validation path than the save it undoes.
///
/// Authorization is the CALLER's job — the two handlers require different permissions on different
/// objects and both resolve the share before calling here.
async fn apply_dashboard_update(
    st: &BiState,
    ctx: &egret_workspace::context::WorkspaceContext,
    id: Uuid,
    b: UpdateDashboard,
    kind: &str,
) -> Result<DashboardDetailDto, BiApiError> {
    require_cards_in_ws(st, ctx, &b.cards).await?;
    crate::model::validate_layout(&b.tabs, &b.cards).map_err(BiApiError::from)?;
    if b.width != "fixed" && b.width != "full" {
        return Err(egret_core::Error::InvalidInput("width must be 'fixed' or 'full'".into()).into());
    }
    // Held, not written — see pending_baseline.
    let baseline = pending_baseline(st, id).await;
    // `UpdateDashboard` is a plain struct of owned values, so this cannot fail; on the impossible
    // branch we keep the save working and lose one history entry rather than 500-ing.
    let snapshot = serde_json::to_value(&b).ok();

    st.dashboards
        .update(
            id,
            &b.name,
            &b.description,
            &b.parameters,
            &b.width,
            Some(ctx.principal.user_id.0),
        )
        .await
        .map_err(BiApiError::from)?;
    let tabs: Vec<crate::dashboard::NewTab> = b.tabs.into_iter().map(Into::into).collect();
    let cards: Vec<crate::dashboard::NewCard> = b.cards.into_iter().map(Into::into).collect();
    st.dashboards
        .replace_layout(id, &tabs, &cards)
        .await
        .map_err(BiApiError::from)?;

    // EVERYTHING THAT CAN REJECT THIS REQUEST HAS NOW RUN. Capturing any earlier would record an
    // edit that never happened — a 403 from the caller's authz, a 404 from require_cards_in_ws, a
    // 400 from validate_layout or the width check, or a failed replace_layout. From here the only
    // failure is a crash, which loses one entry: under-recording, the safe direction.
    if let Some((snap, actor, at)) = baseline {
        capture_revision(st, id, actor, "baseline", &snap, Some(at)).await;
    }
    if let Some(snap) = snapshot {
        capture_revision(st, id, Some(ctx.principal.user_id.0), kind, &snap, None).await;
    }
    load_dashboard(st, id, ctx.principal.user_id.0).await
}
```

- [ ] **Step 7: Reduce `db_update` to authz plus a call**

Replace the body of `db_update` (`:2693-2723`), leaving its `#[utoipa::path]` block untouched:

```rust
async fn db_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateDashboard>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(
        apply_dashboard_update(&st, &ctx, id, b, "edit").await?,
    ))
}
```

- [ ] **Step 8: Write the capture-point test**

Append to `crates/egret-bi/tests/dashboard_revisions.rs`:

```rust
#[tokio::test]
async fn a_baseline_captures_the_state_the_first_edit_overwrites() {
    // The repo-level half of the capture rule: `pending_baseline` reads the CURRENT state, and
    // that state must survive being overwritten. (The routing half — that nothing is written when
    // the PUT is rejected — is asserted live in Task 10 Step 4, because it needs a real 400.)
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Base", "before", Some(uid)).await.unwrap();
    repo.replace_layout(d.id, &[], &[card(None, 0), card(None, 6)])
        .await
        .unwrap();

    let before = repo.snapshot_current(d.id).await.unwrap().unwrap();
    repo.update(d.id, "Base", "after", &serde_json::json!([]), "fixed", Some(uid))
        .await
        .unwrap();
    repo.replace_layout(d.id, &[], &[card(None, 0)]).await.unwrap();
    repo.insert_revision(d.id, Some(uid), "baseline", &before, None)
        .await
        .unwrap();

    let revs = repo.list_revisions(d.id, 10).await.unwrap();
    assert_eq!(revs.len(), 1);
    assert_eq!(revs[0].kind, "baseline");
    assert_eq!(revs[0].snapshot["description"], "before");
    assert_eq!(revs[0].snapshot["width"], "full");
    assert_eq!(revs[0].snapshot["cards"].as_array().unwrap().len(), 2);
    // And the live dashboard has genuinely moved on, so the assertions above are about the
    // snapshot rather than about a dashboard that never changed.
    let now = repo.snapshot_current(d.id).await.unwrap().unwrap();
    assert_eq!(now["description"], "after");
    assert_eq!(now["cards"].as_array().unwrap().len(), 1);
}
```

- [ ] **Step 9: Run it, then prove it survives mutation**

```bash
cargo test -p egret-bi --test dashboard_revisions
```

Expected: PASS, 5 tests. Then change `pending_baseline` to snapshot *after* the write by moving the
`let baseline = …` line below `replace_layout` in `apply_dashboard_update`, rebuild, and confirm
`cargo test --workspace` still compiles but Task 10 Step 3 would show the baseline equal to the new
state. Restore, and note in the PR body that this particular ordering is only provable live — the
repo test above covers the snapshot's content, not the handler's statement order.

- [ ] **Step 10: Run the backend gates and commit**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

```bash
git add crates/egret-analytics-api/src/bi.rs \
        crates/egret-bi/src/routes.rs \
        crates/egret-bi/tests/dashboard_revisions.rs
git commit -m "feat(dashboard): record a revision on every successful dashboard save"
```

---

## Task 7: List and revert endpoints

**Files:**
- Modify: `crates/egret-analytics-api/src/bi.rs`
- Modify: `crates/egret-bi/src/model.rs`
- Modify: `crates/egret-bi/src/routes.rs`
- Test: `crates/egret-bi/tests/dashboard_revision_routes.rs`

- [ ] **Step 1: Add the wire DTO**

In `crates/egret-analytics-api/src/bi.rs`, after `DashboardDetailDto`:

```rust
/// One entry in a dashboard's History tab.
///
/// The `snapshot` is deliberately NOT on this DTO. A 22-card dashboard's PUT body is hundreds of
/// kilobytes and the cap is twenty of them, so shipping the content would make opening the info
/// sidebar a multi-megabyte download for a list the user reads as six lines of text. Revert takes
/// a revision id and does the work server-side, where the snapshot already is.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DashboardRevisionDto {
    pub id: String,
    /// `baseline` | `edit` | `revert` — see migration 0107.
    pub kind: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Option<String>,
    /// Display name for `created_by`, resolved server-side for the same reason
    /// `DashboardDetailDto.created_by_name` is.
    pub created_by_name: Option<String>,
    /// True for the revision that matches the dashboard's live state — the newest one. The client
    /// suppresses the revert action on it rather than deriving "newest" from the array order,
    /// which would silently become wrong the day the endpoint gains a sort parameter.
    pub is_current: bool,
}
```

- [ ] **Step 2: Add the conversion**

In `crates/egret-bi/src/model.rs`, add `DashboardRevisionDto` to the `pub use
egret_analytics_api::bi::{…}` list, and after `dashboard_detail_dto`:

```rust
pub fn dashboard_revision_dto(
    r: crate::dashboard::DashboardRevision,
    name: Option<String>,
    is_current: bool,
) -> DashboardRevisionDto {
    DashboardRevisionDto {
        id: r.id.to_string(),
        kind: r.kind,
        created_at: r.created_at,
        created_by: r.created_by.map(|u| u.to_string()),
        created_by_name: name,
        is_current,
    }
}
```

- [ ] **Step 3: Add the routes**

In `dashboard_router` (`crates/egret-bi/src/routes.rs:2337-2362`), after the `/folder` route:

```rust
        .route(
            "/ws/{workspace}/dashboards/{id}/revisions",
            get(db_revisions),
        )
        .route(
            "/ws/{workspace}/dashboards/{id}/revisions/{revision_id}/revert",
            post(db_revision_revert),
        )
```

- [ ] **Step 4: Add the handlers**

After `db_update`:

```rust
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/dashboards/{id}/revisions",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
    ),
    responses(
        (status = 200, description = "Revision history, newest first (at most 20 — see migration 0107)", body = Vec<DashboardRevisionDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:read permission", body = ErrorBody),
        (status = 404, description = "No such dashboard in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
/// Reading history needs only `dashboard:read` — it discloses who edited the dashboard and when,
/// which anyone who can read the dashboard can already see in the info sidebar. Reverting needs
/// `dashboard:manage`, because it writes.
async fn db_revisions(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<crate::model::DashboardRevisionDto>>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    let revs = st
        .dashboards
        .list_revisions(id, 20)
        .await
        .map_err(BiApiError::from)?;
    // One users lookup for the whole page rather than one per row: at most twenty rows and
    // usually one or two distinct authors.
    let mut names: std::collections::HashMap<Uuid, String> = std::collections::HashMap::new();
    for r in &revs {
        if let Some(u) = r.created_by {
            if let std::collections::hash_map::Entry::Vacant(e) = names.entry(u) {
                if let Ok(Some(user)) = st.users.find_by_id(egret_core::UserId(u)).await {
                    let label = if user.name.trim().is_empty() {
                        user.email.clone()
                    } else {
                        user.name.clone()
                    };
                    e.insert(label);
                }
            }
        }
    }
    Ok(Json(
        revs.into_iter()
            .enumerate()
            .map(|(i, r)| {
                let name = r.created_by.and_then(|u| names.get(&u).cloned());
                crate::model::dashboard_revision_dto(r, name, i == 0)
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/dashboards/{id}/revisions/{revision_id}/revert",
    tag = "dashboards",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Dashboard id"),
        ("revision_id" = Uuid, Path, description = "Revision to restore (must belong to this dashboard)"),
    ),
    responses(
        (status = 200, description = "Dashboard restored to the revision; the restored state is itself recorded as a new revision", body = DashboardDetailDto),
        (status = 400, description = "The stored snapshot is not a valid dashboard layout", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing dashboard:manage permission", body = ErrorBody),
        (status = 404, description = "No such dashboard or revision, or a chart the revision references no longer exists in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
/// Restore an earlier revision by replaying it through the ordinary update path.
///
/// The snapshot is id-free by construction (migration 0107), so nothing is remapped — this is NOT
/// `DashboardRepo::duplicate`, which needs a tab_map only because it copies rows. Revalidation is
/// not optional: a chart a revision references may have been deleted since, and
/// `require_cards_in_ws` inside `apply_dashboard_update` turns that into a 404 rather than a
/// dangling card.
///
/// The revert is itself recorded, as kind 'revert', so undoing a revert is an ordinary revert.
async fn db_revision_revert(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id, revision_id)): Path<(String, Uuid, Uuid)>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "dashboard", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, DB_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    // Scoped on the dashboard, so a revision of ANOTHER dashboard is a 404 and not a cross-object
    // restore, even for a caller who may manage both.
    let rev = st
        .dashboards
        .revision(id, revision_id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("revision".into()))?;
    let body: UpdateDashboard = serde_json::from_value(rev.snapshot).map_err(|e| {
        BiApiError::from(egret_core::Error::InvalidInput(format!(
            "revision snapshot is not a valid dashboard layout: {e}"
        )))
    })?;
    Ok(Json(
        apply_dashboard_update(&st, &ctx, id, body, "revert").await?,
    ))
}
```

- [ ] **Step 5: Register them with OpenAPI**

In the `BiDashboardApi` doc struct (`:3268-3303`), add `db_revisions` and `db_revision_revert` to
`paths(…)` and `DashboardRevisionDto` to `components(schemas(…))`. An endpoint missing from here
compiles, serves, and is invisible to `npm run gen:api` — so the client cannot call it and nothing
reports why.

- [ ] **Step 6: Write the failing route test**

Create `crates/egret-bi/tests/dashboard_revision_routes.rs`, modelled on the existing
`crates/egret-bi/tests/dashboard_object_scope.rs` harness (reuse its app/session helpers verbatim;
they are the crate's only in-process HTTP fixture):

```rust
//! DB-gated revision route tests. Self-skip without DATABASE_URL.
//! Uses the same in-process axum fixture as dashboard_object_scope.rs.
mod common {
    include!("dashboard_object_scope.rs");
}

// Three behaviours, each of which is a way the feature can be wrong rather than absent:
//
//  1. a rejected PUT leaves no revision            — the capture point
//  2. revert restores the earlier card set          — the round trip
//  3. revert is itself recorded, as kind 'revert'   — undo of an undo
//
// Written against the router rather than the repo because all three are properties of the HANDLER
// ordering, and a repo-level test cannot observe a request being rejected.
```

Then, in the same file, three `#[tokio::test]` functions driving the fixture:
`put_with_a_bad_tab_index_leaves_no_revision` (PUT a body whose card has `tab_index: 7` with one
tab → expect 400, then `GET …/revisions` → `[]`);
`revert_restores_the_earlier_card_set` (PUT two cards, PUT one card, revert to the older revision,
`GET` the dashboard → 2 cards);
`revert_is_recorded_as_a_revision_of_kind_revert` (after the revert, the newest revision has
`kind == "revert"` and `is_current == true`).

- [ ] **Step 7: Run the tests**

```bash
cargo test -p egret-bi --test dashboard_revision_routes
```

Expected: PASS, 3 tests.

- [ ] **Step 8: Prove they survive mutation**

1. Move the `let baseline = …` and the two `capture_revision` calls in `apply_dashboard_update` to
   the top of the function, above `require_cards_in_ws`. Re-run. Expected:
   `put_with_a_bad_tab_index_leaves_no_revision` FAILS. **This is the test that guards the capture
   point; if it stays green, it is asserting nothing.**
2. Change `db_revision_revert` to pass `"edit"` instead of `"revert"`. Re-run. Expected:
   `revert_is_recorded_as_a_revision_of_kind_revert` FAILS.
3. Change `st.dashboards.revision(id, revision_id)` to a call that ignores `id`. Re-run against a
   second dashboard's revision id. Expected: the scope assertion in Task 5 already covers this at
   the repo level; confirm it still goes red.

Restore all three.

- [ ] **Step 9: Run the backend gates and commit**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

```bash
git add crates/egret-analytics-api/src/bi.rs \
        crates/egret-bi/src/model.rs \
        crates/egret-bi/src/routes.rs \
        crates/egret-bi/tests/dashboard_revision_routes.rs
git commit -m "feat(dashboard): list and revert dashboard revisions"
```

---

## Task 8: Regenerate the spec and the client

**Files:** `openapi.json`, `frontend/src/api/generated/**`

- [ ] **Step 1: Emit the spec**

```bash
cargo run -p egret-bin -- openapi > openapi.json
```

Expected: `openapi.json` grows. Verify the two new operations are actually in it — a handler missing
from `BiDashboardApi::paths` (Task 7 Step 5) fails silently here:

```bash
python3 -c "import json;d=json.load(open('openapi.json'));print([p for p in d['paths'] if 'revisions' in p])"
```

Expected: exactly two paths, `…/revisions` and `…/revisions/{revision_id}/revert`. An empty list
means Step 5 of Task 7 was skipped.

- [ ] **Step 2: Regenerate the client**

```bash
cd frontend && npm run gen:api
```

Expected: `src/api/generated/dashboards/dashboards.ts` gains `dashboardsDbRevisions`,
`useDashboardsDbRevisions`, `getDashboardsDbRevisionsQueryKey` and `dashboardsDbRevisionRevert`.

- [ ] **Step 3: Typecheck and commit**

```bash
cd frontend && npx tsc --noEmit
```

```bash
git add openapi.json frontend/src/api/generated
git commit -m "chore(api): regenerate the OpenAPI spec and client for dashboard revisions"
```

---

## Task 9: The History tab

**Files:**
- Modify: `frontend/src/api/dashboards.ts`
- Modify: `frontend/src/components/chart/dashboard/attribution.ts`
- Create: `frontend/src/components/chart/dashboard/DashboardHistoryTab.tsx`
- Modify: `frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/attribution.test.ts`,
  `frontend/src/components/chart/dashboard/__tests__/DashboardHistoryTab.test.tsx`

- [ ] **Step 1: Write the failing wording test**

Append to `frontend/src/components/chart/dashboard/__tests__/attribution.test.ts`:

```ts
import { revisionSummary } from "../attribution";

describe("revisionSummary", () => {
  it("uses Metabase's three phrasings, keyed on the RAW kind", () => {
    // Keyed on the raw enum from the API, never on a display string. Three of the six
    // stubbed-assertion defects in the chart programme were tests that read a display string
    // where the code keyed on a raw one.
    expect(revisionSummary("baseline", "Ref User")).toBe("Ref User created this");
    expect(revisionSummary("edit", "Ref User")).toBe("Ref User edited this");
    expect(revisionSummary("revert", "Ref User")).toBe("Ref User reverted to an earlier version");
  });

  it("says 'Someone' rather than dropping the sentence when the author is gone", () => {
    // created_by is NULL after a hard delete (0093's ON DELETE SET NULL), and a timeline row with
    // no subject reads as a bug.
    expect(revisionSummary("edit", null)).toBe("Someone edited this");
  });

  it("degrades to a neutral phrasing for a kind it does not know", () => {
    expect(revisionSummary("something-new", "Ref User")).toBe("Ref User changed this");
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/attribution.test.ts
```

Expected: FAIL — `revisionSummary is not exported`.

- [ ] **Step 3: Implement it**

Append to `frontend/src/components/chart/dashboard/attribution.ts`:

```ts
/** One timeline sentence, in Metabase's wording. `kind` is the RAW value from the API
 *  (`baseline` | `edit` | `revert`, migration 0107), never a rendered label. */
export function revisionSummary(kind: string, name: string | null): string {
  const who = name ?? "Someone";
  switch (kind) {
    case "baseline":
      return `${who} created this`;
    case "edit":
      return `${who} edited this`;
    case "revert":
      return `${who} reverted to an earlier version`;
    default:
      return `${who} changed this`;
  }
}
```

- [ ] **Step 4: Run it to verify it passes, then mutate**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/attribution.test.ts
```

Expected: PASS, 7 tests. Then change `case "revert"` to return the `edit` string and re-run.
Expected: FAIL. Change `const who = name ?? "Someone"` to `name ?? ""` and re-run. Expected: FAIL.
Restore both.

- [ ] **Step 5: Add the API hooks**

Append to `frontend/src/api/dashboards.ts`:

```ts
export interface DashboardRevision {
  id: string;
  kind: "baseline" | "edit" | "revert";
  created_at: string;
  created_by: string | null;
  created_by_name: string | null;
  is_current: boolean;
}

/** A dashboard's history, newest first. `enabled` is false until the History tab is open, so an
 *  info sidebar opened on Overview costs nothing. */
export function useDashboardRevisions(ws: string, id: string | null, enabled: boolean) {
  return useDashboardsDbRevisions<DashboardRevision[], ApiError>(ws, id ?? "", {
    query: { enabled: enabled && !!id, select: (r) => r.data as DashboardRevision[] },
  });
}
```

and, inside `useDashboardMutations`, alongside `restore`:

```ts
  const revert = useMutation<DashboardDetail, ApiError, { id: string; revisionId: string }>({
    mutationFn: async ({ id, revisionId }) =>
      (await dashboardsDbRevisionRevert(ws, id, revisionId)).data as DashboardDetail,
    // A revert changes the dashboard AND appends to its history, so both caches are stale.
    onSuccess: (d) => {
      invalidate(d.id);
      qc.invalidateQueries({ queryKey: getDashboardsDbRevisionsQueryKey(ws, d.id) });
    },
  });
```

Add `revert` to the returned object, and the four generated symbols to the import block at `:4-17`.

- [ ] **Step 6: Write the failing component test**

Create `frontend/src/components/chart/dashboard/__tests__/DashboardHistoryTab.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DashboardHistoryTab } from "../DashboardHistoryTab";
import type { DashboardRevision } from "@/api/dashboards";

const revisions: DashboardRevision[] = [
  { id: "r3", kind: "revert", created_at: "2026-08-05T12:00:00Z", created_by: "u1", created_by_name: "Ref User", is_current: true },
  { id: "r2", kind: "edit", created_at: "2026-08-03T12:00:00Z", created_by: "u2", created_by_name: null, is_current: false },
  { id: "r1", kind: "baseline", created_at: "2026-08-01T12:00:00Z", created_by: "u1", created_by_name: "Ref User", is_current: false },
];

describe("DashboardHistoryTab", () => {
  it("lists every revision newest first with its sentence and date", () => {
    render(<DashboardHistoryTab revisions={revisions} isLoading={false} onRevert={vi.fn()} reverting={false} />);
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(items[0].textContent).toContain("Ref User reverted to an earlier version");
    expect(items[1].textContent).toContain("Someone edited this");
    expect(items[2].textContent).toContain("Ref User created this");
    expect(items[2].textContent).toMatch(/August [12], 2026/);
  });

  it("offers revert on every entry except the current one", () => {
    // is_current comes from the API. Deriving "newest" from array position here would go silently
    // wrong the day the endpoint gains a sort parameter.
    render(<DashboardHistoryTab revisions={revisions} isLoading={false} onRevert={vi.fn()} reverting={false} />);
    expect(screen.getAllByRole("button", { name: "Revert" })).toHaveLength(2);
  });

  it("passes the revision id, not the index, to onRevert", () => {
    const onRevert = vi.fn();
    render(<DashboardHistoryTab revisions={revisions} isLoading={false} onRevert={onRevert} reverting={false} />);
    fireEvent.click(screen.getAllByRole("button", { name: "Revert" })[1]);
    expect(onRevert).toHaveBeenCalledWith("r1");
  });

  it("says so when there is no history yet", () => {
    render(<DashboardHistoryTab revisions={[]} isLoading={false} onRevert={vi.fn()} reverting={false} />);
    expect(screen.getByText("No history yet — this dashboard hasn't been saved since revisions were turned on.")).toBeTruthy();
  });
});
```

- [ ] **Step 7: Run it to verify it fails**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardHistoryTab.test.tsx
```

Expected: FAIL — `Failed to resolve import "../DashboardHistoryTab"`.

- [ ] **Step 8: Write the component**

Create `frontend/src/components/chart/dashboard/DashboardHistoryTab.tsx`:

```tsx
import { Button } from "@/components/ui/button";
import type { DashboardRevision } from "@/api/dashboards";
import { attributionLine, revisionSummary } from "./attribution";

interface Props {
  revisions: DashboardRevision[];
  isLoading: boolean;
  /** Called with the revision's id. */
  onRevert: (revisionId: string) => void;
  reverting: boolean;
}

/** Metabase's History tab: a newest-first timeline with a revert action on every entry that is
 *  not the live one. Lives in its own file so the info sidebar stays a shell — DashboardView.tsx
 *  is touched by seven workstreams and must not grow. */
export function DashboardHistoryTab({ revisions, isLoading, onRevert, reverting }: Props) {
  if (isLoading) return <p className="py-2 text-xs text-muted-foreground">Loading history…</p>;
  if (revisions.length === 0) {
    return (
      <p className="py-2 text-xs text-muted-foreground">
        No history yet — this dashboard hasn't been saved since revisions were turned on.
      </p>
    );
  }
  return (
    <ol className="flex flex-col gap-2 py-1">
      {revisions.map((r) => (
        <li key={r.id} className="flex items-start justify-between gap-2 border-b pb-2 text-xs last:border-b-0">
          <span className="min-w-0">
            <span className="block">{revisionSummary(r.kind, r.created_by_name)}</span>
            <span className="block text-muted-foreground">{attributionLine(r.created_at, null)}</span>
          </span>
          {/* is_current comes from the API, not from this array's order. */}
          {!r.is_current && (
            <Button size="sm" variant="ghost" className="h-6 shrink-0 px-2 text-xs"
              disabled={reverting} onClick={() => onRevert(r.id)}>Revert</Button>
          )}
        </li>
      ))}
    </ol>
  );
}
```

- [ ] **Step 9: Run it to verify it passes, then mutate**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardHistoryTab.test.tsx
```

Expected: PASS, 4 tests. Then change `{!r.is_current && (` to `{i > 0 && (` with an index from
`map`, and re-run — it will still pass, which is the point: re-run with a fixture whose `is_current`
is on `r1` rather than `r3` and confirm the "offers revert on every entry except the current one"
test then FAILS with the index version and passes with the `is_current` version. Restore.

- [ ] **Step 10: Put the tab in the sidebar**

In `frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx`, wrap the existing body in
tabs and add the four new props:

```tsx
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DashboardHistoryTab } from "./DashboardHistoryTab";
```

```tsx
  revisions: DashboardRevision[];
  revisionsLoading: boolean;
  onRevert: (revisionId: string) => void;
  reverting: boolean;
  /** Fires when the History tab is first selected, so the route can enable its query. */
  onOpenHistory: () => void;
```

```tsx
      <Tabs defaultValue="overview" onValueChange={(v) => { if (v === "history") onOpenHistory(); }}>
        <TabsList className="mb-2">
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="history">History</TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          {/* the description label and the <dl> move in here unchanged */}
        </TabsContent>
        <TabsContent value="history">
          <DashboardHistoryTab revisions={revisions} isLoading={revisionsLoading}
            onRevert={onRevert} reverting={reverting} />
        </TabsContent>
      </Tabs>
```

The name row and the close button stay above the `<Tabs>`.

- [ ] **Step 11: Wire the route**

In `frontend/src/routes/explore/DashboardView.tsx` — wiring only, no logic:

```tsx
  const [historyOpen, setHistoryOpen] = useState(false);
  const revisionsQ = useDashboardRevisions(ws, id, showInfo && historyOpen);
```

and on `<DashboardInfoSidebar>`:

```tsx
            revisions={revisionsQ.data ?? []}
            revisionsLoading={revisionsQ.isLoading}
            reverting={revert.isPending}
            onOpenHistory={() => setHistoryOpen(true)}
            onRevert={async (revisionId) => {
              try { await revert.mutateAsync({ id: dash.id, revisionId }); toast.success("Reverted"); }
              catch (e) { toast.error(String(e)); }
            }}
```

Add `revert` to the `useDashboardMutations` destructure at `:61` and `useDashboardRevisions` to the
import at `:6`.

- [ ] **Step 12: Run the frontend gates and commit**

```bash
cd frontend && npm run lint
cd frontend && npx tsc --noEmit
cd frontend && npm run test
```

Expected: all green. Record the count.

```bash
git add frontend/src/api/dashboards.ts \
        frontend/src/components/chart/dashboard/attribution.ts \
        frontend/src/components/chart/dashboard/DashboardHistoryTab.tsx \
        frontend/src/components/chart/dashboard/DashboardInfoSidebar.tsx \
        frontend/src/components/chart/dashboard/__tests__/attribution.test.ts \
        frontend/src/components/chart/dashboard/__tests__/DashboardHistoryTab.test.tsx \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): a History tab in the info sidebar with a timeline and revert"
```

---

## Task 10: Live check — measure, do not eyeball

**Files:** none. This task produces the numbers for the PR body.

Every defect that mattered in the chart programme was found by measuring the running app. Three of
that programme's six live findings were invisible at a glance. A screenshot is not evidence.

- [ ] **Step 1: Build and deploy**

```bash
just deploy-kind
```

Expected: `deployment "egret" successfully rolled out`, SPA at `http://localhost:30080`
(admin@egret.local / Admin12345!).

- [ ] **Step 2: Attribution reads as names, not UUIDs**

Fetch the harness dashboard and record the four attribution fields verbatim:

```bash
curl -s -b cookies.txt \
  http://localhost:30080/api/ws/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb \
  | python3 -c "import json,sys;d=json.load(sys.stdin);print({k:d[k] for k in ['created_by','created_by_name','created_at','updated_by','updated_by_name','updated_at','folder_id']})"
```

Required: `created_by_name` is a name or an email, **not** a 36-character UUID. `created_at` is
`None` for this dashboard (it predates 0105 and the migration refuses to invent one) — record that
it is null, because a non-null value here means the migration backfilled and Task 1 Step 4 was
mis-read. Then open the info sidebar and record the two rendered lines.

- [ ] **Step 3: History accumulates, baseline included**

Open the harness dashboard, press Edit then Save three times (Save with no change is a legitimate
save — the PUT is a whole-layout replace, so the layout is written back byte-identical; **do not
hand-write the harness JSON**). Then:

```bash
curl -s -b cookies.txt \
  http://localhost:30080/api/ws/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb/revisions \
  | python3 -c "import json,sys;r=json.load(sys.stdin);print(len(r));[print(x['kind'],x['created_at'],x['created_by_name'],x['is_current']) for x in r]"
```

Required: **4** rows — one `baseline` and three `edit` — the `baseline` **oldest** and carrying the
dashboard's pre-edit `updated_at` rather than today's timestamp, and `is_current` true on exactly
one row (the first). If the count is 3, `pending_baseline` is not firing. If the baseline's
timestamp is now(), the `at` parameter is not being passed.

- [ ] **Step 4: A rejected PUT records nothing**

This is the capture-point assertion and it cannot be made from the UI. Note the revision count from
Step 3, then send a body that fails validation *after* authz:

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X PUT -b cookies.txt -H 'content-type: application/json' \
  -d '{"name":"ZZ Chart Parity","description":"","parameters":[],"width":"full","tabs":[{"name":"A","position":0}],"cards":[{"chart_id":null,"card_type":"text","grid_col":0,"grid_row":0,"size_x":6,"size_y":4,"options":{},"tab_index":7}]}' \
  http://localhost:30080/api/ws/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb
```

Required: `400`, and the revision count from Step 3's command is **unchanged at 4**. A count of 5
means capture is happening before validation.

Then send an invalid `width` (`"wide"`) and repeat. Required: `400`, count still 4.

- [ ] **Step 5: Revert restores, on a scratch dashboard**

Do **not** do this on the harness. Create a new dashboard "ZZ Revision Revert", add two cards, Save;
delete one card, Save. Open Info → History, press Revert on the two-card entry.

Required, recorded as three numbers: cards before revert (**1**), cards after revert (**2**), and
the revision count after the revert (**4** — baseline, two edits, one revert). Confirm the newest
entry reads "… reverted to an earlier version".

- [ ] **Step 6: Retention holds at 20**

On a second scratch dashboard "ZZ Revision Retention", save 25 times (a `for` loop over the PUT is
fine here — this dashboard is not the harness), then:

```bash
curl -s -b cookies.txt http://localhost:30080/api/ws/demo/dashboards/<scratch-id>/revisions \
  | python3 -c "import json,sys;print(len(json.load(sys.stdin)))"
```

Required: exactly **20**.

- [ ] **Step 7: Record the storage cost**

```bash
kubectl exec -n egret-system deploy/postgres -- psql -U egret -d egret -c \
  "SELECT count(*) AS revisions, pg_size_pretty(sum(pg_column_size(snapshot))) AS snapshot_bytes, pg_size_pretty(pg_total_relation_size('analytics.dashboard_revisions')) AS table_total FROM analytics.dashboard_revisions;"
```

Record all three numbers. This is the retention decision's evidence: if the table is already tens of
megabytes across a handful of dashboards, the cap of 20 is the wrong number and the PR should say so
rather than the plan pretending it cannot be.

- [ ] **Step 8: Clean up and open the PR**

Delete the two scratch dashboards. Open the PR quoting every number from Steps 2–7 verbatim, plus
the frontend test count from Task 9 Step 12 and the backend `cargo test --workspace` summary. Then
stop — the merge is the user's call.

---

## Self-review checklist for the implementer

- [ ] Items 1 and 2 are in their own commits, **before** the first revision commit, and the branch
      is still coherent if every commit from Task 5 onward is dropped.
- [ ] `DashboardView.tsx` gained wiring only — the formatter is in `attribution.ts`, the timeline is
      in `DashboardHistoryTab.tsx`, and the file is not net longer than the props it passes.
- [ ] `attribution_names` resolves server-side; no client-side user map was introduced, and neither
      admin-gated user endpoint is called from the dashboard route.
- [ ] Migration 0106 leaves `created_at` NULL on pre-existing rows — verified by the count query in
      Task 1 Step 4, not by reading the SQL.
- [ ] Both migrations have a `// re-embed:` line in `crates/egret-metastore/src/lib.rs`.
- [ ] The revision capture sits after **every** rejecting statement in `apply_dashboard_update`, and
      the mutation in Task 7 Step 8 turned `put_with_a_bad_tab_index_leaves_no_revision` red.
- [ ] `DashboardRepo::revision` is scoped by `dashboard_id`, and the test that proves it went red
      when the scope was removed.
- [ ] Revert reuses `apply_dashboard_update`, so it revalidates; `duplicate` was **not** reused, and
      no id remapping exists anywhere in the revision code.
- [ ] The revision DTO does not carry `snapshot`.
- [ ] `db_revisions` and `db_revision_revert` are in `BiDashboardApi::paths`, and Task 8 Step 1's
      path check printed two entries.
- [ ] No test asserts a display string where the code keys on a raw one — `revisionSummary` is keyed
      on `kind`, and `is_current` comes from the API rather than from array position.
- [ ] The harness dashboard was saved through the UI, never hand-edited, and the two scratch
      dashboards were deleted.
</content>
</invoke>
