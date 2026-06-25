# Explore Parity + Folders — Implementation Plan (Project B)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring Charts and Dashboards to the Datasets capability bar (Chart Duplicate + bulk-select) and add per-type, flat folders to all three Explore lists, with move via kebab, bulk action, and drag-drop.

**Architecture:** Backend first — one `folders` table (polymorphic by `entity_type`) + a nullable `folder_id` column on `charts`/`dashboards`/`datasets`, a generic folder CRUD router keyed by `{kind}`, a per-resource `…/{id}/folder` move endpoint, and a chart-duplicate endpoint mirroring the dashboard one. Then frontend — a shared `useBulkSelect` hook (extracted from the existing Datasets implementation), a shared folder API + grouping helpers, and group-inline rendering applied to the three pages. Each task ends green.

**Tech Stack:** Rust (axum, sqlx, Postgres), React 18 + TypeScript, Vite, Vitest + Testing Library + `@testing-library/user-event`, `@tanstack/react-query`, shadcn primitives, `lucide-react`, `sonner`.

**Spec:** `docs/en/specs/2026-06-25-explore-parity-folders-design.md`

**Conventions:**
- Backend commands run from `egret/`. Frontend commands run from `egret/frontend/`.
- Git repo root is `egret/`; `git add` paths use the `frontend/` / `crates/` / `migrations/` / `docs/` prefixes.
- Branch `feat/explore-parity-folders` (already checked out). Git identity configured — never override the author.
- **After adding the migration, touch `crates/egret-metastore/src/lib.rs`** (e.g. add/remove a blank line) so the `sqlx::migrate!` embed re-runs; `serve`/tests otherwise use a stale embedded migrator (project memory `egret-sqlx-migration-staleness`).
- Backend sweep before each backend commit: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p egret-bi` (fmt is a CI gate — memory `egret-cargo-fmt-ci`).
- Frontend sweep before each frontend commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- End every commit message with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

## File Structure

**Backend — new:**
- `migrations/0021_folders.sql` — `folders` table + `folder_id` columns.
- `crates/egret-bi/src/folder.rs` — `FolderRepo` + `Folder` row.

**Backend — modified:**
- `crates/egret-bi/src/lib.rs` — declare `mod folder;` and wire `FolderRepo` into `BiState`.
- `crates/egret-bi/src/routes.rs` — `folder_router()`, generic folder handlers, per-resource `…/{id}/folder` move handlers, chart `…/{id}/duplicate` handler + registrations.
- `crates/egret-bi/src/model.rs` — add `folder_id` to `ChartDto`/`DatasetSummaryDto`/`DashboardSummaryDto` + the dto builders; add `FolderDto`, `CreateFolder`, `RenameFolder`, `SetFolder`.
- `crates/egret-bi/src/chart.rs` — `folder_id` on `Chart`, in `COLS`, `set_folder`, `duplicate`, `name_exists`.
- `crates/egret-bi/src/dashboard.rs` — `folder_id` on `Dashboard`, in `COLS`, `set_folder`.
- `crates/egret-bi/src/dataset.rs` — `folder_id` on `Dataset`, in `COLS`, `set_folder`.
- `crates/egret-metastore/src/lib.rs` — touch to re-embed migrations.

**Frontend — new:**
- `frontend/src/api/folders.ts` — `useFolders`, `useFolderMutations`, `useSetFolder` (generic over `kind`).
- `frontend/src/hooks/useBulkSelect.ts` — shared multi-select state.
- `frontend/src/components/explore/BulkBar.tsx` — selection action bar.
- `frontend/src/components/explore/folders.ts` — `EntityKind`, `useFolderGroups`, `useCollapsed`.
- `frontend/src/components/explore/FolderHeaderRow.tsx` — collapsible section header `<tr>`.
- `frontend/src/components/explore/MoveToFolderSub.tsx` — kebab "Move to folder ▸" submenu.

**Frontend — modified:**
- `frontend/src/api/charts.ts` — `folder_id` on `Chart`; `duplicate` in `useChartMutations`.
- `frontend/src/api/dashboards.ts` — `folder_id` on `DashboardSummary`.
- `frontend/src/api/datasets.ts` — `folder_id` on `DatasetSummary`.
- `frontend/src/components/chart/ChartActionsMenu.tsx` — add Duplicate + Move-to-folder.
- `frontend/src/components/chart/dashboard/DashboardActionsMenu.tsx` — add Move-to-folder.
- `frontend/src/components/dataset/DatasetActionsMenu.tsx` — add Move-to-folder.
- `frontend/src/routes/explore/Charts.tsx` / `Dashboards.tsx` / `Datasets.tsx` — bulk-select + group-inline + drag-drop.

**Tests (new):**
- `crates/egret-bi/tests/folder_repo.rs`
- `frontend/src/hooks/__tests__/useBulkSelect.test.tsx`
- `frontend/src/components/explore/__tests__/folders.test.ts` (useFolderGroups)
- `frontend/src/components/explore/__tests__/FolderHeaderRow.test.tsx`
- additions to `frontend/src/routes/explore/__tests__/Charts.test.tsx`

---

## Task 1: Migration — `folders` table + `folder_id` columns

**Files:**
- Create: `migrations/0021_folders.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (touch to re-embed)

- [ ] **Step 1: Write the migration**

Create `migrations/0021_folders.sql`:
```sql
-- Per-type, flat folders for Explore resources. Polymorphic by entity_type,
-- mirroring the bookmarks pattern (0020). A resource's folder_id is nullable;
-- NULL = Uncategorized. Deleting a folder sends its items back to Uncategorized.
CREATE TABLE folders (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    entity_type  text NOT NULL,           -- 'chart' | 'dashboard' | 'dataset'
    name         text NOT NULL,
    created_by   uuid REFERENCES users(id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT folder_name_uniq UNIQUE (workspace_id, entity_type, name)
);
CREATE INDEX folders_ws_type_idx ON folders (workspace_id, entity_type);

ALTER TABLE charts     ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
ALTER TABLE dashboards ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
ALTER TABLE datasets   ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
```

- [ ] **Step 2: Force a re-embed**

Edit `crates/egret-metastore/src/lib.rs` — add a trailing newline or a blank line near the top doc comment so the file's mtime changes and `sqlx::migrate!("../../migrations")` re-embeds the new file. (No logic change.)

- [ ] **Step 3: Verify it applies**

Ensure the dev Postgres is up (`docker compose -f deploy/docker-compose.dev.yml up -d`), then:
Run: `EGRET_DATABASE__URL="postgres://egret:egret@localhost:5432/egret" cargo run -p egret-bin -- migrate`
Expected: `migrations applied`, no error.

- [ ] **Step 4: Commit**

```bash
git add migrations/0021_folders.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(db): folders table + folder_id columns (charts/dashboards/datasets)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `FolderRepo`

**Files:**
- Create: `crates/egret-bi/src/folder.rs`
- Modify: `crates/egret-bi/src/lib.rs`
- Test: `crates/egret-bi/tests/folder_repo.rs`

Context: `BiError` is in `crate::error`; the `Db` wrapper exposes `db.pool` (a `sqlx::PgPool`). Follow `ChartRepo` (`chart.rs`) for style.

- [ ] **Step 1: Write the repo**

Create `crates/egret-bi/src/folder.rs`:
```rust
//! Folder repository: per-type, flat folders. entity_type is 'chart'|'dashboard'|'dataset'.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct FolderRepo {
    db: Db,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Folder {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub entity_type: String,
    pub name: String,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

const COLS: &str = "id, workspace_id, entity_type, name, created_by, created_at";

fn map_conflict_err(e: sqlx::Error) -> BiError {
    if let sqlx::Error::Database(ref dbe) = e {
        if dbe.is_unique_violation() {
            return BiError::Conflict("a folder with that name already exists".into());
        }
    }
    BiError::Db(e)
}

impl FolderRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn list(&self, workspace_id: Uuid, entity_type: &str) -> Result<Vec<Folder>, BiError> {
        let sql = format!(
            "SELECT {COLS} FROM folders WHERE workspace_id=$1 AND entity_type=$2 ORDER BY name"
        );
        Ok(sqlx::query_as::<_, Folder>(&sql)
            .bind(workspace_id)
            .bind(entity_type)
            .fetch_all(&self.db.pool)
            .await?)
    }

    pub async fn create(
        &self,
        workspace_id: Uuid,
        entity_type: &str,
        name: &str,
        created_by: Option<Uuid>,
    ) -> Result<Folder, BiError> {
        let sql = format!(
            "INSERT INTO folders (workspace_id, entity_type, name, created_by) \
             VALUES ($1,$2,$3,$4) RETURNING {COLS}"
        );
        sqlx::query_as::<_, Folder>(&sql)
            .bind(workspace_id)
            .bind(entity_type)
            .bind(name)
            .bind(created_by)
            .fetch_one(&self.db.pool)
            .await
            .map_err(map_conflict_err)
    }

    pub async fn rename(&self, id: Uuid, name: &str) -> Result<Option<Folder>, BiError> {
        let sql = format!("UPDATE folders SET name=$2 WHERE id=$1 RETURNING {COLS}");
        sqlx::query_as::<_, Folder>(&sql)
            .bind(id)
            .bind(name)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(map_conflict_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM folders WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// (workspace_id, entity_type) of a folder, for the ownership/type guard. None if absent.
    pub async fn meta(&self, id: Uuid) -> Result<Option<(Uuid, String)>, BiError> {
        Ok(sqlx::query_as::<_, (Uuid, String)>(
            "SELECT workspace_id, entity_type FROM folders WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?)
    }
}
```

- [ ] **Step 2: Wire into the module + state**

In `crates/egret-bi/src/lib.rs`: add `pub mod folder;` next to the other `mod` lines, and add a `pub folders: folder::FolderRepo` field to `BiState` (mirror how `charts: chart::ChartRepo` is declared and constructed — search for `ChartRepo::new`). Construct it as `folder::FolderRepo::new(db.clone())` in the same constructor.

- [ ] **Step 3: Write the repo test**

Create `crates/egret-bi/tests/folder_repo.rs`. Mirror the harness in `crates/egret-bi/tests/dashboard_repo.rs` (same `Db` setup / `MIGRATOR` / workspace+user seeding — copy its top-of-file setup helper verbatim). The test body:
```rust
// after seeding a workspace `ws` and user `uid` (see dashboard_repo.rs setup):
let repo = egret_bi::folder::FolderRepo::new(db.clone());
let f = repo.create(ws, "chart", "Revenue", Some(uid)).await.unwrap();
assert_eq!(f.name, "Revenue");

// uniqueness is per (workspace, entity_type, name)
let dup = repo.create(ws, "chart", "Revenue", Some(uid)).await;
assert!(matches!(dup, Err(egret_bi::error::BiError::Conflict(_))));
// same name under a different type is allowed
repo.create(ws, "dashboard", "Revenue", Some(uid)).await.unwrap();

let charts = repo.list(ws, "chart").await.unwrap();
assert_eq!(charts.len(), 1);

let meta = repo.meta(f.id).await.unwrap().unwrap();
assert_eq!(meta, (ws, "chart".to_string()));

repo.rename(f.id, "Revenue 2024").await.unwrap();
assert_eq!(repo.list(ws, "chart").await.unwrap()[0].name, "Revenue 2024");

repo.delete(f.id).await.unwrap();
assert!(repo.list(ws, "chart").await.unwrap().is_empty());
```
(If `dashboard_repo.rs` gates DB tests behind an env var / `#[ignore]`, copy that exact attribute.)

- [ ] **Step 4: Run it**

Run: `cargo test -p egret-bi folder_repo`
Expected: PASS (DB up). If the suite is `#[ignore]`-gated, run with the same flag the existing repo tests use.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-bi/src/folder.rs crates/egret-bi/src/lib.rs crates/egret-bi/tests/folder_repo.rs
git commit -m "feat(bi): FolderRepo (per-type flat folders)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `folder_id` on row structs, repos, and DTOs

**Files:**
- Modify: `crates/egret-bi/src/chart.rs`, `dashboard.rs`, `dataset.rs`, `model.rs`

This threads the new column through reads so the list payloads carry `folder_id`, and adds a `set_folder` writer to each resource repo.

- [ ] **Step 1: Charts repo**

In `crates/egret-bi/src/chart.rs`:
- Add `pub folder_id: Option<Uuid>,` to `struct Chart` (after `created_by`).
- Change `COLS` to end with `…, created_by, folder_id, updated_at`.
- Add a writer:
```rust
pub async fn set_folder(&self, id: Uuid, folder_id: Option<Uuid>) -> Result<(), BiError> {
    sqlx::query("UPDATE charts SET folder_id=$2 WHERE id=$1")
        .bind(id)
        .bind(folder_id)
        .execute(&self.db.pool)
        .await?;
    Ok(())
}
```
(The `create`/`update` INSERT/UPDATE column lists are unchanged — new charts start in Uncategorized; `RETURNING {COLS}` now includes `folder_id`, which is fine.)

- [ ] **Step 2: Dashboards + Datasets repos**

In `crates/egret-bi/src/dashboard.rs`: add `pub folder_id: Option<Uuid>,` to `struct Dashboard`; add `folder_id` to `COLS` (before `updated_at` or `deleted_at` — keep it consistent with the struct field order, and update any explicit column list used by the summary query to also select `folder_id`); add the same `set_folder` writer against `dashboards`.

In `crates/egret-bi/src/dataset.rs`: add `pub folder_id: Option<Uuid>,` to `struct Dataset`; add `folder_id` to that repo's `COLS`/select; add the same `set_folder` writer against `datasets`.

- [ ] **Step 3: DTOs**

In `crates/egret-bi/src/model.rs`:
- `ChartDto`: add `pub folder_id: Option<String>,`; in `chart_dto`, set `folder_id: c.folder_id.map(|u| u.to_string())`.
- `DatasetSummaryDto`: add `pub folder_id: Option<String>,`; in `summary_dto`, set `folder_id: ds.folder_id.map(|u| u.to_string())`.
- `DashboardSummaryDto`: add `pub folder_id: Option<String>,`; in `dashboard_summary_dto`, set `folder_id: d.folder_id.map(|u| u.to_string())`.

- [ ] **Step 4: Compile**

Run: `cargo build -p egret-bi`
Expected: builds. Fix any other `Chart`/`Dashboard`/`Dataset` struct literal that the compiler now flags for the missing field (there should be none — all construction is via `query_as`).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-bi/src/chart.rs crates/egret-bi/src/dashboard.rs crates/egret-bi/src/dataset.rs crates/egret-bi/src/model.rs
git commit -m "feat(bi): carry folder_id through repos + summary DTOs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Folder CRUD + move routes

**Files:**
- Modify: `crates/egret-bi/src/routes.rs`, `crates/egret-bi/src/model.rs`

A single generic folder router keyed by `{kind}` (`charts`|`dashboards`|`datasets`), plus a per-resource `…/{id}/folder` move endpoint on each existing router.

- [ ] **Step 1: Folder DTOs + bodies**

In `model.rs` add:
```rust
#[derive(Debug, serde::Serialize)]
pub struct FolderDto {
    pub id: String,
    pub name: String,
}
pub fn folder_dto(f: crate::folder::Folder) -> FolderDto {
    FolderDto { id: f.id.to_string(), name: f.name }
}
#[derive(Debug, serde::Deserialize)]
pub struct CreateFolder { pub name: String }
#[derive(Debug, serde::Deserialize)]
pub struct RenameFolder { pub name: String }
#[derive(Debug, serde::Deserialize)]
pub struct SetFolder { pub folder_id: Option<Uuid> }
```
(`Uuid` is already imported in `model.rs`; if not, add `use uuid::Uuid;`.)

- [ ] **Step 2: kind → (entity_type, read_perm, manage_perm) helper + router**

In `routes.rs` add near the other consts/helpers:
```rust
/// Map a URL {kind} segment to (entity_type, read_perm, manage_perm). None ⇒ 404.
fn kind_perms(kind: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match kind {
        "charts" => Some(("chart", CH_READ, CH_MANAGE)),
        "dashboards" => Some(("dashboard", DB_READ, DB_MANAGE)),
        "datasets" => Some(("dataset", DS_READ, DS_MANAGE)),
        _ => None,
    }
}

pub fn folder_router() -> Router<BiState> {
    Router::new()
        .route(
            "/ws/{workspace}/folders/{kind}",
            get(folders_list).post(folders_create),
        )
        .route(
            "/ws/{workspace}/folders/{kind}/{id}",
            axum::routing::patch(folders_rename).delete(folders_delete),
        )
}
```
Register `folder_router()` wherever `chart_router()`/`dashboard_router()` are merged into the app (search `chart_router()` in `routes.rs` / `lib.rs` and add `.merge(folder_router())` alongside).

- [ ] **Step 3: Generic folder handlers**

```rust
async fn folders_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind)): Path<(String, String)>,
) -> Result<Json<Vec<crate::model::FolderDto>>, BiApiError> {
    let (etype, read, _manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    st.authz.require_workspace(&ctx, read).await.map_err(BiApiError::from)?;
    let rows = st.folders.list(ctx.workspace_id.0, etype).await.map_err(BiApiError::from)?;
    Ok(Json(rows.into_iter().map(crate::model::folder_dto).collect()))
}

async fn folders_create(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind)): Path<(String, String)>,
    Json(b): Json<crate::model::CreateFolder>,
) -> Result<Json<crate::model::FolderDto>, BiApiError> {
    let (etype, _read, manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    st.authz.require_workspace(&ctx, manage).await.map_err(BiApiError::from)?;
    let f = st.folders
        .create(ctx.workspace_id.0, etype, &b.name, Some(ctx.principal.user_id.0))
        .await.map_err(BiApiError::from)?;
    Ok(Json(crate::model::folder_dto(f)))
}

async fn folders_rename(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
    Json(b): Json<crate::model::RenameFolder>,
) -> Result<Json<crate::model::FolderDto>, BiApiError> {
    let (etype, _read, manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    st.authz.require_workspace(&ctx, manage).await.map_err(BiApiError::from)?;
    require_folder(&st, &ctx, id, etype).await?;
    let f = st.folders.rename(id, &b.name).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("folder".into()))?;
    Ok(Json(crate::model::folder_dto(f)))
}

async fn folders_delete(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let (etype, _read, manage) = kind_perms(&kind).ok_or(Error::NotFound("kind".into()))?;
    st.authz.require_workspace(&ctx, manage).await.map_err(BiApiError::from)?;
    require_folder(&st, &ctx, id, etype).await?;
    st.folders.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Guard: folder exists, is in this workspace, and matches the URL kind's entity_type.
async fn require_folder(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
    entity_type: &str,
) -> Result<(), BiApiError> {
    match st.folders.meta(id).await.map_err(BiApiError::from)? {
        Some((w, t)) if w == ctx.workspace_id.0 && t == entity_type => Ok(()),
        _ => Err(Error::NotFound("folder".into()).into()),
    }
}
```

- [ ] **Step 4: Per-resource move handlers + routes**

Add to `chart_router()`: `.route("/ws/{workspace}/charts/{id}/folder", axum::routing::patch(ch_set_folder))`. Handler:
```rust
async fn ch_set_folder(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SetFolder>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz.require_workspace(&ctx, CH_MANAGE).await.map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    if let Some(fid) = b.folder_id {
        require_folder(&st, &ctx, fid, "chart").await?;   // cross-tenant / cross-type guard
    }
    st.charts.set_folder(id, b.folder_id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
```
Mirror exactly for dashboards (`db_set_folder`, perm `DB_MANAGE`, `require_dashboard_in_ws`, entity_type `"dashboard"`, `st.dashboards.set_folder`) and datasets (`ds_set_folder`, perm `DS_MANAGE`, the dataset in-ws guard used by `ds_get` — reuse it, entity_type `"dataset"`, `st.datasets.set_folder`). Register each on its router.

- [ ] **Step 5: Build + fmt + clippy**

Run: `cargo build -p egret-bi && cargo fmt --all -- --check && cargo clippy -p egret-bi --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-bi/src/routes.rs crates/egret-bi/src/model.rs
git commit -m "feat(bi): folder CRUD routes + per-resource move endpoint

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Chart duplicate endpoint

**Files:**
- Modify: `crates/egret-bi/src/chart.rs`, `crates/egret-bi/src/routes.rs`
- Test: `crates/egret-bi/tests/folder_repo.rs` (append) or a new `chart_duplicate.rs`

Charts have a per-workspace unique name (`chart_name_uniq`). Duplicate picks the first free `"<name> (copy)"`, `"<name> (copy 2)"`, … so it never 409s.

- [ ] **Step 1: Repo — name lookup + duplicate**

In `chart.rs` add:
```rust
/// All chart names already used in a workspace (for collision-free copy naming).
pub async fn names_in_workspace(&self, workspace_id: Uuid) -> Result<Vec<String>, BiError> {
    Ok(sqlx::query_scalar::<_, String>("SELECT name FROM charts WHERE workspace_id=$1")
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?)
}

/// Duplicate a chart in place (same dataset/viz/params/folder), owned by `new_owner`.
pub async fn duplicate(&self, src_id: Uuid, new_owner: Option<Uuid>) -> Result<Option<Chart>, BiError> {
    let Some(src) = self.get(src_id).await? else { return Ok(None); };
    let existing = self.names_in_workspace(src.workspace_id).await?;
    let name = next_copy_name(&existing, &src.name);
    let sql = format!(
        "INSERT INTO charts (workspace_id, dataset_id, name, description, viz_type, params, created_by, folder_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING {COLS}"
    );
    let c = sqlx::query_as::<_, Chart>(&sql)
        .bind(src.workspace_id)
        .bind(src.dataset_id)
        .bind(&name)
        .bind(&src.description)
        .bind(&src.viz_type)
        .bind(&src.params)
        .bind(new_owner)
        .bind(src.folder_id)
        .fetch_one(&self.db.pool)
        .await
        .map_err(map_conflict_err)?;
    Ok(Some(c))
}
```
Add the helper (free function in `chart.rs`):
```rust
/// First free "<base> (copy)" / "<base> (copy N)" not present in `existing`.
pub fn next_copy_name(existing: &[String], base: &str) -> String {
    let first = format!("{base} (copy)");
    if !existing.iter().any(|n| n == &first) {
        return first;
    }
    for n in 2.. {
        let cand = format!("{base} (copy {n})");
        if !existing.iter().any(|e| e == &cand) {
            return cand;
        }
    }
    unreachable!()
}
```

- [ ] **Step 2: Unit-test the helper**

Append to `crates/egret-bi/tests/folder_repo.rs` (or a new `tests/chart_duplicate.rs`) a pure unit test — no DB:
```rust
#[test]
fn copy_name_increments() {
    use egret_bi::chart::next_copy_name;
    let mut have = vec!["Sales".to_string()];
    assert_eq!(next_copy_name(&have, "Sales"), "Sales (copy)");
    have.push("Sales (copy)".to_string());
    assert_eq!(next_copy_name(&have, "Sales"), "Sales (copy 2)");
    have.push("Sales (copy 2)".to_string());
    assert_eq!(next_copy_name(&have, "Sales"), "Sales (copy 3)");
}
```
(Ensure `pub mod chart;` is reachable from the crate root — it is, via `egret_bi::chart`.)

- [ ] **Step 3: Run it**

Run: `cargo test -p egret-bi copy_name_increments`
Expected: PASS.

- [ ] **Step 4: Route + handler**

In `chart_router()` add: `.route("/ws/{workspace}/charts/{id}/duplicate", post(ch_duplicate))`. Handler (mirror `db_duplicate`):
```rust
async fn ch_duplicate(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
) -> Result<Json<crate::model::ChartDto>, BiApiError> {
    st.authz.require_workspace(&ctx, CH_MANAGE).await.map_err(BiApiError::from)?;
    require_chart_in_ws(&st, &ctx, id).await?;
    let copy = st.charts
        .duplicate(id, Some(ctx.principal.user_id.0))
        .await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("chart".into()))?;
    Ok(Json(crate::model::chart_dto(copy)))
}
```

- [ ] **Step 5: Sweep + commit**

Run: `cargo fmt --all -- --check && cargo clippy -p egret-bi --all-targets -- -D warnings && cargo test -p egret-bi`
Expected: clean, tests pass.
```bash
git add crates/egret-bi/src/chart.rs crates/egret-bi/src/routes.rs crates/egret-bi/tests/
git commit -m "feat(bi): chart duplicate endpoint (collision-free copy name)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Frontend API — folders + chart duplicate + `folder_id` types

**Files:**
- Create: `frontend/src/api/folders.ts`
- Modify: `frontend/src/api/charts.ts`, `dashboards.ts`, `datasets.ts`

- [ ] **Step 1: Add `folder_id` to the summary types**

- `charts.ts`: in `interface Chart`, add `folder_id: string | null;`.
- `dashboards.ts`: in `interface DashboardSummary`, add `folder_id?: string | null;`.
- `datasets.ts`: in `interface DatasetSummary`, add `folder_id: string | null;`.

- [ ] **Step 2: Chart `duplicate` mutation**

In `charts.ts` `useChartMutations`, add before `return`:
```ts
const duplicate = useMutation({
  mutationFn: (id: string) =>
    apiJson<Chart>(`/ws/${ws}/charts/${id}/duplicate`, { method: "POST" }),
  onSuccess: invalidate,
});
```
and return it: `return { create, update, remove, duplicate };`.

- [ ] **Step 3: Shared folders API**

Create `frontend/src/api/folders.ts`:
```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export type EntityKind = "charts" | "dashboards" | "datasets";
export interface Folder { id: string; name: string }

const listKeyOf: Record<EntityKind, (ws: string) => readonly unknown[]> = {
  charts: (ws) => ["charts", ws],
  dashboards: (ws) => ["dashboards", ws],
  datasets: (ws) => ["datasets", ws, "list"],
};
export const folderKeys = { list: (ws: string, kind: EntityKind) => ["folders", ws, kind] as const };

export function useFolders(ws: string, kind: EntityKind) {
  return useQuery<Folder[]>({
    queryKey: folderKeys.list(ws, kind),
    queryFn: () => apiJson<Folder[]>(`/ws/${ws}/folders/${kind}`),
  });
}

export function useFolderMutations(ws: string, kind: EntityKind) {
  const qc = useQueryClient();
  const inval = () => qc.invalidateQueries({ queryKey: folderKeys.list(ws, kind) });
  const create = useMutation({
    mutationFn: (name: string) =>
      apiJson<Folder>(`/ws/${ws}/folders/${kind}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name }) }),
    onSuccess: inval,
  });
  const rename = useMutation({
    mutationFn: (a: { id: string; name: string }) =>
      apiJson<Folder>(`/ws/${ws}/folders/${kind}/${a.id}`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: a.name }) }),
    onSuccess: inval,
  });
  const remove = useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/folders/${kind}/${id}`, { method: "DELETE" }),
    onSuccess: inval,
  });
  return { create, rename, remove };
}

/** Move a resource into (folderId) or out of (null) a folder; invalidates that type's list. */
export function useSetFolder(ws: string, kind: EntityKind) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (a: { id: string; folder_id: string | null }) =>
      apiJson(`/ws/${ws}/${kind}/${a.id}/folder`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ folder_id: a.folder_id }) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: listKeyOf[kind](ws) }),
  });
}
```

- [ ] **Step 4: Typecheck + commit**

Run: `npm run typecheck`
Expected: passes.
```bash
git add frontend/src/api/folders.ts frontend/src/api/charts.ts frontend/src/api/dashboards.ts frontend/src/api/datasets.ts
git commit -m "feat(api): folders client + chart duplicate + folder_id on summaries

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: `useBulkSelect` hook (extract from Datasets)

**Files:**
- Create: `frontend/src/hooks/useBulkSelect.ts`
- Test: `frontend/src/hooks/__tests__/useBulkSelect.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/hooks/__tests__/useBulkSelect.test.tsx`:
```tsx
import { act, renderHook } from "@testing-library/react";
import { expect, test } from "vitest";
import { useBulkSelect } from "@/hooks/useBulkSelect";

const rows = [{ id: "a" }, { id: "b" }, { id: "c" }];

test("toggle, select-all, clear", () => {
  const { result } = renderHook(() => useBulkSelect(rows));
  expect(result.current.bulkMode).toBe(false);
  act(() => result.current.setBulkMode(true));
  act(() => result.current.toggle("a"));
  expect(result.current.selected.has("a")).toBe(true);
  expect(result.current.someSelected).toBe(true);
  expect(result.current.allSelected).toBe(false);
  act(() => result.current.toggleAll());
  expect(result.current.allSelected).toBe(true);
  act(() => result.current.toggleAll());
  expect(result.current.selected.size).toBe(0);
  act(() => { result.current.toggle("b"); result.current.clear(); });
  expect(result.current.selected.size).toBe(0);
});
```

- [ ] **Step 2: Run it, verify FAIL**

Run: `npx vitest run src/hooks/__tests__/useBulkSelect.test.tsx`
Expected: FAIL — cannot resolve `@/hooks/useBulkSelect`.

- [ ] **Step 3: Implement**

Create `frontend/src/hooks/useBulkSelect.ts`:
```ts
import { useCallback, useMemo, useState } from "react";

/** Multi-select state for a list of `{id}` rows. `rows` should be the currently visible set. */
export function useBulkSelect<T extends { id: string }>(rows: T[]) {
  const [bulkMode, setBulkMode] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const toggle = useCallback((id: string) => {
    setSelected((s) => { const n = new Set(s); n.has(id) ? n.delete(id) : n.add(id); return n; });
  }, []);
  const clear = useCallback(() => setSelected(new Set()), []);
  const allSelected = rows.length > 0 && rows.every((r) => selected.has(r.id));
  const someSelected = rows.some((r) => selected.has(r.id));
  const toggleAll = useCallback(() => {
    setSelected((s) => {
      const everyOn = rows.length > 0 && rows.every((r) => s.has(r.id));
      return everyOn ? new Set() : new Set(rows.map((r) => r.id));
    });
  }, [rows]);
  const setMode = useCallback((v: boolean) => { setBulkMode(v); if (!v) setSelected(new Set()); }, []);

  // ids that still exist in the visible rows (selection can outlive filtering)
  const selectedIds = useMemo(() => rows.filter((r) => selected.has(r.id)).map((r) => r.id), [rows, selected]);

  return { bulkMode, setBulkMode: setMode, selected, selectedIds, toggle, toggleAll, clear, allSelected, someSelected };
}
```

- [ ] **Step 4: Run it, verify PASS**

Run: `npx vitest run src/hooks/__tests__/useBulkSelect.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/hooks/useBulkSelect.ts frontend/src/hooks/__tests__/useBulkSelect.test.tsx
git commit -m "feat(explore): shared useBulkSelect hook

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: `BulkBar`, and refactor Datasets onto the shared hook

**Files:**
- Create: `frontend/src/components/explore/BulkBar.tsx`
- Modify: `frontend/src/routes/explore/Datasets.tsx`

This is a pure refactor of Datasets (no behaviour change) that proves the shared pieces before Charts/Dashboards adopt them.

- [ ] **Step 1: BulkBar**

Create `frontend/src/components/explore/BulkBar.tsx`:
```tsx
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";

/** Header strip shown in bulk mode: a Bulk/Cancel toggle and (when items are selected) actions. */
export function BulkToggle({ active, onToggle }: { active: boolean; onToggle: () => void }) {
  return (
    <Button variant={active ? "secondary" : "outline"} onClick={onToggle}>
      {active ? "Cancel" : "Bulk select"}
    </Button>
  );
}

export function BulkBar({ count, children }: { count: number; children: ReactNode }) {
  if (count === 0) return null;
  return <div className="mb-2 flex items-center gap-2 text-sm">{children}</div>;
}
```

- [ ] **Step 2: Refactor Datasets**

In `Datasets.tsx`, replace the inline `bulkMode`/`selected`/`toggle`/`toggleAll`/`allSelected`/`someSelected` state (lines ~34–95) with:
```ts
const bulk = useBulkSelect(rows);
```
and use `bulk.bulkMode`, `bulk.setBulkMode`, `bulk.selected`, `bulk.toggle`, `bulk.toggleAll`, `bulk.allSelected`, `bulk.someSelected`, `bulk.selectedIds` throughout (the "Bulk select" button becomes `<BulkToggle active={bulk.bulkMode} onToggle={() => bulk.setBulkMode(!bulk.bulkMode)} />`; the bulk-delete uses `bulk.selectedIds`). Keep the existing bulk-delete `AlertDialog` and its wording unchanged. Import `useBulkSelect` and `BulkToggle`/`BulkBar`.

- [ ] **Step 3: Sweep (existing Datasets tests must stay green)**

Run: `npm run typecheck && npx vitest run src/routes/explore`
Expected: PASS — Datasets behaviour unchanged.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/explore/BulkBar.tsx frontend/src/routes/explore/Datasets.tsx
git commit -m "refactor(datasets): adopt shared useBulkSelect + BulkBar (no behaviour change)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Chart Duplicate + bulk Delete (Charts)

**Files:**
- Modify: `frontend/src/components/chart/ChartActionsMenu.tsx`, `frontend/src/routes/explore/Charts.tsx`
- Test: `frontend/src/routes/explore/__tests__/Charts.test.tsx`

- [ ] **Step 1: Add Duplicate to the chart kebab**

Edit `ChartActionsMenu.tsx` — add an `onDuplicate` prop and a menu item:
```tsx
export function ChartActionsMenu({ ws, chart, onDuplicate, onDelete }: {
  ws: string; chart: Chart; onDuplicate: (c: Chart) => void; onDelete: (c: Chart) => void;
}) {
  const nav = useNavigate();
  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/charts/${chart.id}`)}>Open</DropdownMenuItem>
      <DropdownMenuItem onClick={() => onDuplicate(chart)}>Duplicate</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDelete(chart)}>Delete</DropdownMenuItem>
    </RowActionsMenu>
  );
}
```

- [ ] **Step 2: Wire duplicate + bulk-select in Charts.tsx**

In `Charts.tsx`:
- Destructure `duplicate` too: `const { remove, duplicate } = useChartMutations(ws);`.
- Add `const bulk = useBulkSelect(rows);`.
- Pass `onDuplicate` to `ChartActionsMenu`:
```tsx
onDuplicate={(c) => duplicate.mutateAsync(c.id).then(() => toast.success("Duplicated")).catch((e) => toast.error(String(e)))}
```
- Add a `BulkToggle` next to the `+ Chart` button, a checkbox `<th>`/`<td>` column rendered only when `bulk.bulkMode` (copy the pattern from Datasets — `<Checkbox checked={bulk.allSelected} onCheckedChange={bulk.toggleAll} aria-label="Select all" />` in the header; `<Checkbox checked={bulk.selected.has(c.id)} onCheckedChange={() => bulk.toggle(c.id)} aria-label={\`Select ${c.name}\`} />` per row), and a bulk **Delete N** `AlertDialog` that deletes `bulk.selectedIds` sequentially then `bulk.clear()`:
```tsx
async function doBulkDelete() {
  for (const id of bulk.selectedIds) { try { await remove.mutateAsync(id); } catch (e) { toast.error(String(e)); } }
  bulk.clear(); setBulkDeleteOpen(false);
}
```
Wording mirrors the single-delete dialog: title `Delete {bulk.selectedIds.length} charts?`, body "This permanently deletes the selected charts. This cannot be undone."

- [ ] **Step 3: Test — Duplicate calls the mutation**

Append to `Charts.test.tsx` a test that renders the list (reuse the file's existing render/mock setup), opens the first row's kebab, clicks **Duplicate**, and asserts a POST to `/charts/{id}/duplicate` fired (mock `apiJson`/`apiFetch` the same way the existing tests do; follow `ChartExportMenu.test.tsx` for opening a Radix menu via `user-event`). Also assert the **Bulk select** toggle reveals checkboxes.

- [ ] **Step 4: Run + sweep**

Run: `npx vitest run src/routes/explore/__tests__/Charts.test.tsx && npm run typecheck && npm run lint`
Expected: PASS, clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/ChartActionsMenu.tsx frontend/src/routes/explore/Charts.tsx frontend/src/routes/explore/__tests__/Charts.test.tsx
git commit -m "feat(charts): Duplicate action + bulk-select Delete

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Bulk Move-to-Trash (Dashboards)

**Files:**
- Modify: `frontend/src/routes/explore/Dashboards.tsx`

- [ ] **Step 1: Add bulk-select + bulk trash**

In `Dashboards.tsx` (skip in the trash view — only when `!showTrash`):
- `const bulk = useBulkSelect(rows);`.
- A `BulkToggle` in the header button row (only when `!showTrash`).
- A checkbox column rendered when `bulk.bulkMode` (same pattern as Charts).
- A `BulkBar` with a **Move N to Trash** button (light, no confirm — matches the single Move to Trash):
```tsx
async function doBulkTrash() {
  for (const id of bulk.selectedIds) { try { await trash.mutateAsync(id); } catch (e) { toast.error(String(e)); } }
  toast.success("Moved to Trash"); bulk.clear();
}
```

- [ ] **Step 2: Sweep**

Run: `npm run typecheck && npm run lint && npx vitest run src/routes/explore`
Expected: PASS (existing Dashboards tests still green — open the kebab via the same pattern they already use).

- [ ] **Step 3: Commit**

```bash
git add frontend/src/routes/explore/Dashboards.tsx
git commit -m "feat(dashboards): bulk-select Move to Trash

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Folder grouping helpers (`useFolderGroups`, `useCollapsed`)

**Files:**
- Create: `frontend/src/components/explore/folders.ts`
- Test: `frontend/src/components/explore/__tests__/folders.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/explore/__tests__/folders.test.ts`:
```ts
import { expect, test } from "vitest";
import { groupByFolder } from "@/components/explore/folders";

const folders = [{ id: "f2", name: "Beta" }, { id: "f1", name: "Alpha" }];
const rows = [
  { id: "a", folder_id: "f1" }, { id: "b", folder_id: null }, { id: "c", folder_id: "f1" },
];

test("groups A→Z with Uncategorized last; hideEmpty drops empty folders", () => {
  const g = groupByFolder(rows, folders, false);
  expect(g.map((x) => x.folder?.name ?? "Uncategorized")).toEqual(["Alpha", "Beta", "Uncategorized"]);
  expect(g[0].rows.map((r) => r.id)).toEqual(["a", "c"]);

  const g2 = groupByFolder(rows, folders, true);          // hideEmpty: drop "Beta" (0 rows)
  expect(g2.map((x) => x.folder?.name ?? "Uncategorized")).toEqual(["Alpha", "Uncategorized"]);
});
```

- [ ] **Step 2: Run it, verify FAIL**

Run: `npx vitest run src/components/explore/__tests__/folders.test.ts`
Expected: FAIL — `groupByFolder` not exported.

- [ ] **Step 3: Implement**

Create `frontend/src/components/explore/folders.ts`:
```ts
import { useCallback, useState } from "react";
import type { EntityKind, Folder } from "@/api/folders";

export type { EntityKind, Folder };

export interface FolderGroup<T> { folder: Folder | null; rows: T[] }

/** Bucket rows by folder_id; folders A→Z, Uncategorized (null) last.
 *  hideEmpty drops empty named folders (used while a search/filter is active). */
export function groupByFolder<T extends { folder_id?: string | null }>(
  rows: T[], folders: Folder[], hideEmpty: boolean,
): FolderGroup<T>[] {
  const byId = new Map<string, T[]>();
  const uncat: T[] = [];
  for (const r of rows) {
    if (r.folder_id) { (byId.get(r.folder_id) ?? byId.set(r.folder_id, []).get(r.folder_id)!).push(r); }
    else uncat.push(r);
  }
  const sorted = [...folders].sort((a, b) => a.name.localeCompare(b.name));
  const out: FolderGroup<T>[] = [];
  for (const f of sorted) {
    const fr = byId.get(f.id) ?? [];
    if (hideEmpty && fr.length === 0) continue;
    out.push({ folder: f, rows: fr });
  }
  if (uncat.length > 0 || !hideEmpty) out.push({ folder: null, rows: uncat });
  return out;
}

/** Per-list collapse state persisted in localStorage. Default expanded. */
export function useCollapsed(storageKey: string) {
  const [set, setSet] = useState<Set<string>>(() => {
    try { return new Set(JSON.parse(localStorage.getItem(storageKey) ?? "[]")); } catch { return new Set(); }
  });
  const isCollapsed = useCallback((key: string) => set.has(key), [set]);
  const toggle = useCallback((key: string) => {
    setSet((s) => {
      const n = new Set(s); n.has(key) ? n.delete(key) : n.add(key);
      try { localStorage.setItem(storageKey, JSON.stringify([...n])); } catch { /* ignore */ }
      return n;
    });
  }, [storageKey]);
  return { isCollapsed, toggle };
}
```

- [ ] **Step 4: Run it, verify PASS**

Run: `npx vitest run src/components/explore/__tests__/folders.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/explore/folders.ts frontend/src/components/explore/__tests__/folders.test.ts
git commit -m "feat(explore): folder grouping + collapse-state helpers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: `FolderHeaderRow` + `MoveToFolderSub`

**Files:**
- Create: `frontend/src/components/explore/FolderHeaderRow.tsx`, `frontend/src/components/explore/MoveToFolderSub.tsx`
- Test: `frontend/src/components/explore/__tests__/FolderHeaderRow.test.tsx`

- [ ] **Step 1: FolderHeaderRow**

Create `frontend/src/components/explore/FolderHeaderRow.tsx`:
```tsx
import { ChevronDown, ChevronRight, MoreHorizontal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";

/** A collapsible section header rendered as a full-width table row. */
export function FolderHeaderRow({ name, count, colSpan, collapsed, onToggle, onRename, onDelete, onDropItem }: {
  name: string; count: number; colSpan: number; collapsed: boolean; onToggle: () => void;
  onRename?: () => void; onDelete?: () => void; onDropItem?: (id: string) => void;
}) {
  const Chevron = collapsed ? ChevronRight : ChevronDown;
  return (
    <tr
      className="border-t bg-muted/40"
      onDragOver={onDropItem ? (e) => e.preventDefault() : undefined}
      onDrop={onDropItem ? (e) => { const id = e.dataTransfer.getData("text/plain"); if (id) onDropItem(id); } : undefined}
    >
      <td colSpan={colSpan} className="py-1">
        <div className="flex items-center gap-1">
          <button className="inline-flex items-center gap-1 font-medium hover:underline" onClick={onToggle} aria-label={`Toggle ${name}`}>
            <Chevron className="h-4 w-4 text-muted-foreground" />
            {name}
            <span className="text-muted-foreground">({count})</span>
          </button>
          {(onRename || onDelete) && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="sm" className="ml-1 h-6 w-6 p-0" aria-label={`Folder ${name} actions`}>
                  <MoreHorizontal className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start">
                {onRename && <DropdownMenuItem onClick={onRename}>Rename</DropdownMenuItem>}
                {onDelete && <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={onDelete}>Delete</DropdownMenuItem>}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>
      </td>
    </tr>
  );
}
```

- [ ] **Step 2: MoveToFolderSub**

Create `frontend/src/components/explore/MoveToFolderSub.tsx`:
```tsx
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import type { Folder } from "@/api/folders";

/** Flat "Move to folder" choices for a row kebab: each folder, Uncategorized, then New folder…
 *  Rendered inline (not a Radix sub-menu) to keep it simple + testable. */
export function MoveToFolderItems({ folders, currentFolderId, onMove, onNewFolder }: {
  folders: Folder[]; currentFolderId: string | null;
  onMove: (folderId: string | null) => void; onNewFolder: () => void;
}) {
  return (
    <>
      <DropdownMenuSeparator />
      {folders.map((f) => (
        <DropdownMenuItem key={f.id} disabled={f.id === currentFolderId} onClick={() => onMove(f.id)}>
          Move to “{f.name}”
        </DropdownMenuItem>
      ))}
      {currentFolderId !== null && <DropdownMenuItem onClick={() => onMove(null)}>Remove from folder</DropdownMenuItem>}
      <DropdownMenuItem onClick={onNewFolder}>Move to new folder…</DropdownMenuItem>
    </>
  );
}
```

- [ ] **Step 3: Test FolderHeaderRow**

Create `frontend/src/components/explore/__tests__/FolderHeaderRow.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";

test("renders name + count and toggles", async () => {
  const onToggle = vi.fn();
  render(<table><tbody>
    <FolderHeaderRow name="Revenue" count={3} colSpan={4} collapsed={false} onToggle={onToggle} />
  </tbody></table>);
  expect(screen.getByText("Revenue")).toBeInTheDocument();
  expect(screen.getByText("(3)")).toBeInTheDocument();
  await userEvent.click(screen.getByLabelText("Toggle Revenue"));
  expect(onToggle).toHaveBeenCalledOnce();
});
```

- [ ] **Step 4: Run + commit**

Run: `npx vitest run src/components/explore/__tests__/FolderHeaderRow.test.tsx`
Expected: PASS.
```bash
git add frontend/src/components/explore/FolderHeaderRow.tsx frontend/src/components/explore/MoveToFolderSub.tsx frontend/src/components/explore/__tests__/FolderHeaderRow.test.tsx
git commit -m "feat(explore): FolderHeaderRow + Move-to-folder menu items

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: Group-inline rendering + move-to-folder on Charts

**Files:**
- Modify: `frontend/src/routes/explore/Charts.tsx`, `frontend/src/components/chart/ChartActionsMenu.tsx`

Charts is the template; Dashboards/Datasets follow in Task 14. This is presentational wiring — verified live in Task 15.

- [ ] **Step 1: Load folders + grouping in Charts.tsx**

Add:
```ts
const { data: folders = [] } = useFolders(ws, "charts");
const setFolder = useSetFolder(ws, "charts");
const { create: createFolder, rename: renameFolder, remove: removeFolder } = useFolderMutations(ws, "charts");
const collapse = useCollapsed(`folders:${ws}:charts`);
const filtering = query.trim() !== "" || typeFilter !== "all";
const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);
```

- [ ] **Step 2: Render groups instead of a flat tbody**

Replace the single `<tbody>{rows.map(...)}` with one `<tbody>` per group: a `FolderHeaderRow` (colSpan = 4, or 5 in bulk mode) keyed by the folder id / `"__uncat__"`, then — unless collapsed — the existing item `<tr>`s for `group.rows`. Wire the header:
```tsx
<FolderHeaderRow
  name={group.folder?.name ?? "Uncategorized"} count={group.rows.length}
  colSpan={bulk.bulkMode ? 5 : 4}
  collapsed={collapse.isCollapsed(group.folder?.id ?? "__uncat__")}
  onToggle={() => collapse.toggle(group.folder?.id ?? "__uncat__")}
  onRename={group.folder ? () => { const n = prompt("Rename folder", group.folder!.name); if (n) renameFolder.mutateAsync({ id: group.folder!.id, name: n }).catch((e) => toast.error(String(e))); } : undefined}
  onDelete={group.folder ? () => removeFolder.mutateAsync(group.folder!.id).then(() => toast.success("Folder deleted")).catch((e) => toast.error(String(e))) : undefined}
  onDropItem={(id) => setFolder.mutateAsync({ id, folder_id: group.folder?.id ?? null }).catch((e) => toast.error(String(e)))}
/>
```
Make each item `<tr>` draggable for Task… now: add `draggable onDragStart={(e) => e.dataTransfer.setData("text/plain", c.id)}` to the row. (Drag-drop and the header drop target ship together here since both are tiny; if it risks green, gate the `draggable`/`onDropItem` wiring behind a follow-up commit.)

- [ ] **Step 3: Move-to-folder in the chart kebab**

Extend `ChartActionsMenu` to accept `folders`, `onMove`, `onNewFolder` and render `<MoveToFolderItems folders={folders} currentFolderId={chart.folder_id ?? null} onMove={(fid) => onMove(chart, fid)} onNewFolder={() => onNewFolder(chart)} />` before the delete separator. In `Charts.tsx` pass:
```tsx
onMove={(c, fid) => setFolder.mutateAsync({ id: c.id, folder_id: fid }).catch((e) => toast.error(String(e)))}
onNewFolder={(c) => { const n = prompt("New folder name"); if (n) createFolder.mutateAsync(n).then((f) => setFolder.mutateAsync({ id: c.id, folder_id: f.id })).catch((e) => toast.error(String(e))); }}
```
Add a bulk **Move N to folder…** control to the bulk bar using the same `setFolder` over `bulk.selectedIds`.

- [ ] **Step 4: Sweep**

Run: `npm run typecheck && npm run lint && npx vitest run src/routes/explore/__tests__/Charts.test.tsx`
Expected: PASS (the existing Charts test still finds rows; grouping renders them under a header). Adjust the test's row queries if they assumed a single tbody.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/Charts.tsx frontend/src/components/chart/ChartActionsMenu.tsx
git commit -m "feat(charts): folder group-inline + move (kebab/bulk/drag-drop)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: Group-inline + move on Dashboards and Datasets

**Files:**
- Modify: `frontend/src/routes/explore/Dashboards.tsx`, `frontend/src/routes/explore/Datasets.tsx`, `DashboardActionsMenu.tsx`, `DatasetActionsMenu.tsx`

Apply the exact Task-13 wiring to the other two pages, using `kind="dashboards"` / `kind="datasets"`. Skip grouping in the dashboards **Trash** view (render the flat list there as today).

- [ ] **Step 1: Dashboards**

Replicate Task 13 steps 1–3 in `Dashboards.tsx` with `useFolders(ws, "dashboards")` etc.; gate all folder UI behind `!showTrash`. `colSpan` = 4 (or 5 with bulk checkboxes). Extend `DashboardActionsMenu` with `folders`/`onMove`/`onNewFolder` rendering `<MoveToFolderItems>` (only in the non-trashed branch).

- [ ] **Step 2: Datasets**

Replicate in `Datasets.tsx` with `useFolders(ws, "datasets")`. Datasets has more columns — set `colSpan` to the dataset table's column count (count the `<th>`s, +1 when bulk checkboxes show). Extend `DatasetActionsMenu` with `<MoveToFolderItems>` before the Delete separator.

- [ ] **Step 3: Sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all PASS. Fix any existing Dashboards/Datasets test row-queries that assumed a single tbody.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/Dashboards.tsx frontend/src/routes/explore/Datasets.tsx frontend/src/components/chart/dashboard/DashboardActionsMenu.tsx frontend/src/components/dataset/DatasetActionsMenu.tsx
git commit -m "feat(dashboards,datasets): folder group-inline + move

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 15: Full sweep, live verification, finish

**Files:** none (verification only).

- [ ] **Step 1: Backend + frontend sweep**

Backend (from `egret/`): `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p egret-bi`
Frontend (from `egret/frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all PASS, 0 lint errors.

- [ ] **Step 2: Live verification (Playwright)**

Bring up the stack (docker compose dev + `cargo run -p egret-bin -- serve` on :8080 + `npm run dev` on :5173; stop the nginx `egret-dev-frontend-1` container so Vite owns :5173). Log in `admin@egret.local` / `Admin12345!` (workspace `eng`). Verify:
1. **Charts** `/explore/eng/charts`: kebab → **Duplicate** creates "<name> (copy)"; **Bulk select** → check rows → **Delete N** confirms + removes; create a folder via a row kebab "Move to new folder…", row jumps under the new section; collapse the section, reload, stays collapsed; **drag** a row onto another folder header moves it; section header kebab Rename/Delete works (Delete returns items to Uncategorized).
2. **Dashboards** `/explore/eng/dashboards`: Duplicate (already present) still works; **Bulk select → Move N to Trash**; folder create/move/collapse as above; Trash view is flat (no folders) and Restore returns the dashboard to its folder.
3. **Datasets** `/explore/eng/datasets`: existing Delete/Refresh/Duplicate/bulk-delete unchanged; folder create/move/collapse work.
4. `browser_console_messages` (errors, all): no NEW errors beyond the known favicon-404 / Trino-warmup schema-502s.

Screenshots (if any) to `/Users/1pro-l4r8/Documents/github/data-portal/` (parent dir, not the repo); delete after.

- [ ] **Step 3: Cleanup + finish**

Confirm `git status` clean. Dispatch a final code reviewer over `git diff main...HEAD` (focus: the move endpoint's cross-tenant/cross-type guard, the duplicate name path, and that the three pages group identically). Then run `superpowers:finishing-a-development-branch`.

---

## Self-Review notes

- **Spec coverage:** Decision A (schema) → Task 1; FolderRepo → Task 2; folder_id through reads → Task 3; folder CRUD + move routes (Decision C) → Task 4; chart duplicate (Decision B) → Task 5; FE API → Task 6; bulk-select (Decision D) → Tasks 7–10; group-inline UI (Decision E) → Tasks 11–14; drag-drop (Decision F) → folded into Tasks 12–13 (native HTML5 DnD, not dnd-kit — lighter, no new dependency; a deliberate refinement of the spec); testing + live verify → each task + Task 15.
- **Type consistency:** `EntityKind` = `"charts"|"dashboards"|"datasets"` (folders.ts) is the URL `{kind}`; backend `kind_perms` maps the same three strings to entity_type `"chart"|"dashboard"|"dataset"`. `folder_id` is `Option<Uuid>` in Rust rows, `Option<String>` in DTOs, `string | null` in TS. `useBulkSelect` exposes `selectedIds` (filtered to visible rows) consumed by every bulk action. `groupByFolder(rows, folders, hideEmpty)` and `useCollapsed(storageKey)` signatures are identical across all three pages.
- **Reuses Project A:** `RowActionsMenu`, `SortableHeader`, `RelativeTime`, `ListSkeleton`, `Skeleton` unchanged; bulk-select is extracted from the existing Datasets code (Task 8 is a no-behaviour-change refactor) before Charts/Dashboards adopt it.
- **Green-per-commit:** backend Tasks 1–5 each build + test green; FE Task 6 is types/hooks only; Task 7 adds a tested hook; Task 8 refactors Datasets with its tests staying green; Tasks 9–14 each add isolated, tested units; Task 15 verifies end-to-end.
- **Migration footgun covered:** Task 1 Step 2 touches `egret-metastore/src/lib.rs` so the embedded migrator re-embeds `0021`.
