# Phase D2 — Dashboard Tabs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give an egret dashboard multiple named tabs, each with its own grid of cards, with a tab bar (view + edit) and the Metabase edit affordances (add / rename / reorder / delete-with-reassign / duplicate / move-card).

**Architecture:** A new `dashboard_tabs` table + a nullable `tab_id` FK on `dashboard_cards`; a zero-tab dashboard keeps all cards at `tab_id = null` (no bar, unchanged behaviour). The whole-layout save becomes `replace_layout(tabs, cards)` — delete cards+tabs, re-insert tabs by order, resolve each card's `tab_id` by a client-supplied `tab_index`. The frontend addresses the active tab by **position** (`?tab=<position>`); edit-mode mutations run on pure helpers over a `{draftTabs, draftCards}` model, then serialize to the PUT body.

**Tech Stack:** Rust (axum, sqlx, Postgres) for `egret-bi`; React 18 + TypeScript + Vite + Vitest + react-grid-layout for the frontend.

**Spec:** `docs/en/specs/2026-06-22-phase-3d-d2-dashboard-tabs-design.md`

**Branch:** `feat/dashboards-phase-d2` (already created off `main`, which now includes D1).

## Conventions (every task)

- TDD: write the failing test, watch it fail, implement, watch it pass, commit.
- **Full sweep before each commit:**
  - Frontend tasks (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
  - Backend tasks (from repo root): `cargo test -p egret-bi` (the DB-gated repo test self-skips without `DATABASE_URL`; the local stack exports one — see the migration note below).
- Commit with the configured git identity (never `--author`); message body ends with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Stage explicit paths only (never `git add -A`).
- Metabase fidelity, clean-room (no AGPL code).

## File structure

**Backend (`crates/egret-bi/`):**
- `../../migrations/0017_dashboard_tabs.sql` (Create) — `dashboard_tabs` table + `tab_id` column.
- `src/dashboard.rs` (Modify) — `DashboardTab`, `NewTab`, `NewCard.tab_index`, `DashboardCard.tab_id`, `replace_layout`, `tabs()`.
- `src/lib.rs` (Modify) — re-export `NewTab`, `DashboardTab`.
- `src/model.rs` (Modify) — `DashboardTabDto`, `DashboardCardDto.tab_id`, `DashboardDetailDto.tabs`, `CardInput.tab_index`, `NewTabInput`, `UpdateDashboard.tabs`, `dashboard_tab_dto`, `dashboard_detail_dto` signature.
- `src/routes.rs` (Modify) — `db_update` builds tabs+cards, validates, calls `replace_layout`; `load_dashboard` fetches tabs.
- `tests/dashboard_repo.rs` (Modify) — `replace_layout` round-trip + cascade + validation test.

**Frontend (`frontend/src/`):**
- `api/dashboards.ts` (Modify) — `DashboardTab`, `tabs`, `card.tab_id`, `CardInput.tab_index`, PUT `tabs`.
- `api/useDashboardTabs.ts` (Create) — `?tab=<position>` URL sync + clamp.
- `components/chart/dashboard/tabs.ts` (Create) — `DraftTab`, `DraftCard`, pure draft helpers + serializer.
- `components/chart/dashboard/DashboardTabBar.tsx` (Create) — the tab bar (view + edit).
- `routes/explore/DashboardView.tsx` (Modify) — wire tabs state, filtering, lazy mount, affordances, save.
- Test files alongside, under `__tests__/`.

---

## Task 1: Migration + repo (`dashboard_tabs`, `replace_layout`)

**Files:**
- Create: `migrations/0017_dashboard_tabs.sql`
- Modify: `crates/egret-bi/src/dashboard.rs`
- Modify: `crates/egret-bi/src/lib.rs`
- Test: `crates/egret-bi/tests/dashboard_repo.rs`

> **sqlx migration-staleness gotcha:** egret embeds migrations at compile time. Adding a `.sql` file does **not** recompile `egret-metastore`, so `serve` (and the test harness) run a stale migrator. After creating the file, run `touch crates/egret-metastore/src/lib.rs` before building/testing so the new migration is re-embedded.

- [ ] **Step 1: Create the migration**

`migrations/0017_dashboard_tabs.sql`:

```sql
CREATE TABLE dashboard_tabs (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id uuid NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
    name         text NOT NULL,
    position     int  NOT NULL DEFAULT 0
);
CREATE INDEX dashboard_tabs_dashboard_idx ON dashboard_tabs (dashboard_id);

ALTER TABLE dashboard_cards
    ADD COLUMN tab_id uuid REFERENCES dashboard_tabs(id) ON DELETE CASCADE;
```

Then force a re-embed: `touch crates/egret-metastore/src/lib.rs`.

- [ ] **Step 2: Write the failing repo test**

Append to `crates/egret-bi/tests/dashboard_repo.rs` (the file already has `setup()` → `Fixture`, and imports `NewCard`; add `NewTab` to the `use egret_bi::{…}` line):

```rust
#[tokio::test]
async fn replace_layout_tabs_and_card_assignment() {
    let Some(f) = setup().await else { eprintln!("skip"); return; };
    let params = serde_json::json!({ "spec": { "metrics": ["revenue"], "row_limit": 100 } });
    let chart = f.charts.create(f.ws, f.dataset, "Sales", "", "bar", &params, Some(f.user)).await.unwrap();
    let d = f.repo.create(f.ws, "Tabbed", "", Some(f.user)).await.unwrap();

    // two tabs; card 0 on tab 0, card 1 on tab 1
    let tabs = vec![
        NewTab { name: "Overview".into(), position: 0 },
        NewTab { name: "Detail".into(),   position: 1 },
    ];
    let cards = vec![
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: Some(0) },
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: Some(1) },
    ];
    f.repo.replace_layout(d.id, &tabs, &cards).await.unwrap();

    let got_tabs = f.repo.tabs(d.id).await.unwrap();
    assert_eq!(got_tabs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["Overview", "Detail"]);
    let got_cards = f.repo.cards(d.id).await.unwrap();
    assert_eq!(got_cards.len(), 2);
    assert_eq!(got_cards[0].tab_id, Some(got_tabs[0].id));
    assert_eq!(got_cards[1].tab_id, Some(got_tabs[1].id));

    // tabless save: cards keep tab_id = null
    f.repo.replace_layout(d.id, &[], &[
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: None },
    ]).await.unwrap();
    assert!(f.repo.tabs(d.id).await.unwrap().is_empty());
    assert_eq!(f.repo.cards(d.id).await.unwrap()[0].tab_id, None);

    // out-of-range tab_index → BadRequest
    let bad = f.repo.replace_layout(d.id, &[NewTab { name: "Only".into(), position: 0 }], &[
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: Some(5) },
    ]).await;
    assert!(matches!(bad, Err(egret_bi::BiError::BadRequest(_))));

    // deleting a tab cascades its cards
    f.repo.replace_layout(d.id, &[NewTab { name: "T".into(), position: 0 }], &[
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: Some(0) },
    ]).await.unwrap();
    f.repo.replace_layout(d.id, &[], &[]).await.unwrap();   // drop the tab → its card goes too
    assert!(f.repo.cards(d.id).await.unwrap().is_empty());
}
```

- [ ] **Step 3: Run it to confirm it fails (or skips)**

Run: `touch crates/egret-metastore/src/lib.rs && cargo test -p egret-bi replace_layout_tabs_and_card_assignment`
Expected: **compile error** (`NewTab` / `replace_layout` / `tab_index` / `tabs` don't exist yet). That is the RED state.

- [ ] **Step 4: Add the structs and columns**

In `crates/egret-bi/src/dashboard.rs`, add `tab_id` to `DashboardCard`:

```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DashboardCard {
    pub id: Uuid,
    pub dashboard_id: Uuid,
    pub chart_id: Option<Uuid>,
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    pub options: serde_json::Value,
    pub ordinal: i32,
    pub tab_id: Option<Uuid>,
}
```

Add the tab row struct and inputs (near `NewCard`):

```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DashboardTab {
    pub id: Uuid,
    pub name: String,
    pub position: i32,
}

/// A tab as supplied by the client when saving a layout (no server id yet).
#[derive(Debug, Clone)]
pub struct NewTab {
    pub name: String,
    pub position: i32,
}
```

Add `tab_index` to `NewCard`:

```rust
pub struct NewCard {
    pub chart_id: Option<Uuid>,
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    pub options: serde_json::Value,
    /// 0-based index into the layout's `tabs`; `None` for a tabless dashboard.
    pub tab_index: Option<usize>,
}
```

Update `CARD_COLS` to include the new column:

```rust
const CARD_COLS: &str =
    "id, dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal, tab_id";
```

- [ ] **Step 5: Add `tabs()` and `replace_layout()`; remove `replace_cards`**

In `impl DashboardRepo`, add a tab query (place near `cards()`):

```rust
/// All tabs on a dashboard, ordered by position.
pub async fn tabs(&self, dashboard_id: Uuid) -> Result<Vec<DashboardTab>, BiError> {
    Ok(sqlx::query_as::<_, DashboardTab>(
        "SELECT id, name, position FROM dashboard_tabs WHERE dashboard_id = $1 ORDER BY position, id",
    )
    .bind(dashboard_id)
    .fetch_all(&self.db.pool)
    .await?)
}
```

Replace the existing `replace_cards` method with `replace_layout`:

```rust
/// Atomically replace the dashboard's full tab + card set (in supplied order).
/// Cards reference their tab by `tab_index` into `tabs`; `None` means no tab.
pub async fn replace_layout(
    &self,
    dashboard_id: Uuid,
    tabs: &[NewTab],
    cards: &[NewCard],
) -> Result<(), BiError> {
    // Validate before opening the transaction: every card's tab_index must be in range.
    for c in cards {
        match c.tab_index {
            Some(i) if i >= tabs.len() => {
                return Err(BiError::BadRequest(format!(
                    "card tab_index {i} out of range (0..{})",
                    tabs.len()
                )));
            }
            _ => {}
        }
    }

    let mut tx = self.db.pool.begin().await?;
    // Cards first (FK → dashboard_tabs), then tabs.
    sqlx::query("DELETE FROM dashboard_cards WHERE dashboard_id = $1")
        .bind(dashboard_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM dashboard_tabs WHERE dashboard_id = $1")
        .bind(dashboard_id).execute(&mut *tx).await?;

    let mut tab_ids: Vec<Uuid> = Vec::with_capacity(tabs.len());
    for t in tabs {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO dashboard_tabs (dashboard_id, name, position) VALUES ($1,$2,$3) RETURNING id",
        )
        .bind(dashboard_id).bind(&t.name).bind(t.position)
        .fetch_one(&mut *tx).await?;
        tab_ids.push(id);
    }

    for (i, c) in cards.iter().enumerate() {
        let tab_id = c.tab_index.map(|idx| tab_ids[idx]);
        sqlx::query(
            "INSERT INTO dashboard_cards \
             (dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal, tab_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(dashboard_id).bind(c.chart_id).bind(&c.card_type)
        .bind(c.grid_col).bind(c.grid_row).bind(c.size_x).bind(c.size_y)
        .bind(&c.options).bind(i as i32).bind(tab_id)
        .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
```

- [ ] **Step 6: Re-export the new types**

In `crates/egret-bi/src/lib.rs`, add `NewTab` and `DashboardTab` to the `dashboard::{…}` re-export line (it already re-exports `NewCard`, `DashboardRepo`, etc.). Find the existing `pub use` for `dashboard` and add the names.

- [ ] **Step 7: Run the test to confirm it passes**

Run: `touch crates/egret-metastore/src/lib.rs && cargo test -p egret-bi replace_layout_tabs_and_card_assignment`
Expected: PASS (or "skip" if no `DATABASE_URL` — run against the local stack DB so it actually executes).

- [ ] **Step 8: Full sweep + commit**

Run: `cargo test -p egret-bi`
```bash
git add migrations/0017_dashboard_tabs.sql crates/egret-bi/src/dashboard.rs crates/egret-bi/src/lib.rs crates/egret-bi/tests/dashboard_repo.rs crates/egret-metastore/src/lib.rs
git commit -m "feat(dashboards): dashboard_tabs table + replace_layout repo"
```

---

## Task 2: DTOs + router wiring (`tabs` in GET, validation in PUT)

**Files:**
- Modify: `crates/egret-bi/src/model.rs`
- Modify: `crates/egret-bi/src/routes.rs`
- Test: `crates/egret-bi/src/model.rs` (`#[cfg(test)]` module — unit test the validation helper)

- [ ] **Step 1: Write the failing unit test for the layout-validation helper**

In `crates/egret-bi/src/model.rs`, inside the existing `#[cfg(test)] mod tests { … }`, add:

```rust
#[test]
fn validate_layout_rejects_mixed_and_out_of_range() {
    use super::{validate_layout, CardInput, NewTabInput};
    let tab = || NewTabInput { name: "T".into(), position: 0 };
    let card = |ti: Option<usize>| CardInput {
        chart_id: None, card_type: "chart".into(),
        grid_col: 0, grid_row: 0, size_x: 6, size_y: 4,
        options: serde_json::json!({}), tab_index: ti,
    };
    // tabs present but a card has no tab_index → error
    assert!(validate_layout(&[tab()], &[card(None)]).is_err());
    // no tabs but a card has a tab_index → error
    assert!(validate_layout(&[], &[card(Some(0))]).is_err());
    // out of range → error
    assert!(validate_layout(&[tab()], &[card(Some(1))]).is_err());
    // valid: tabs + in-range index
    assert!(validate_layout(&[tab()], &[card(Some(0))]).is_ok());
    // valid: no tabs + no index
    assert!(validate_layout(&[], &[card(None)]).is_ok());
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p egret-bi validate_layout_rejects_mixed_and_out_of_range`
Expected: compile error (`validate_layout`, `NewTabInput`, `CardInput.tab_index` don't exist).

- [ ] **Step 3: Add the DTOs**

In `crates/egret-bi/src/model.rs`:

Add `tab_id` to `DashboardCardDto`:

```rust
#[derive(Debug, Serialize)]
pub struct DashboardCardDto {
    pub id: String,
    pub chart_id: Option<String>,
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    pub options: serde_json::Value,
    pub tab_id: Option<String>,
}
```

Add the tab DTO and put `tabs` on the detail DTO:

```rust
#[derive(Debug, Serialize)]
pub struct DashboardTabDto {
    pub id: String,
    pub name: String,
    pub position: i32,
}

#[derive(Debug, Serialize)]
pub struct DashboardDetailDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub created_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub tabs: Vec<DashboardTabDto>,
    pub cards: Vec<DashboardCardDto>,
}
```

Add `tab_index` to `CardInput` and a tab input + `tabs` on `UpdateDashboard`:

```rust
#[derive(Debug, Deserialize)]
pub struct CardInput {
    #[serde(default)]
    pub chart_id: Option<Uuid>,
    #[serde(default = "default_card_type")]
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    #[serde(default = "empty_obj")]
    pub options: serde_json::Value,
    #[serde(default)]
    pub tab_index: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct NewTabInput {
    pub name: String,
    pub position: i32,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDashboard {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "empty_arr")]
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub tabs: Vec<NewTabInput>,
    #[serde(default)]
    pub cards: Vec<CardInput>,
}
```

- [ ] **Step 4: Add conversions, the validation helper, and the dto builders**

Update `CardInput::into_new` to carry `tab_index`, add `NewTabInput::into_new`, the `validate_layout` helper, and the tab dto. Replace `dashboard_detail_dto` to take tabs:

```rust
impl CardInput {
    pub fn into_new(self) -> NewCard {
        NewCard {
            chart_id: self.chart_id,
            card_type: self.card_type,
            grid_col: self.grid_col,
            grid_row: self.grid_row,
            size_x: self.size_x,
            size_y: self.size_y,
            options: self.options,
            tab_index: self.tab_index,
        }
    }
}

impl NewTabInput {
    pub fn into_new(self) -> crate::dashboard::NewTab {
        crate::dashboard::NewTab { name: self.name, position: self.position }
    }
}

/// Mirror Metabase's "can't mix tabs and null-tab cards": with tabs, every card needs an
/// in-range `tab_index`; without tabs, no card may carry one.
pub fn validate_layout(tabs: &[NewTabInput], cards: &[CardInput]) -> Result<(), egret_core::Error> {
    for c in cards {
        match c.tab_index {
            Some(i) if i >= tabs.len() => {
                return Err(egret_core::Error::InvalidInput(format!(
                    "card tab_index {i} out of range (0..{})", tabs.len()
                )));
            }
            None if !tabs.is_empty() => {
                return Err(egret_core::Error::InvalidInput(
                    "every card needs a tab_index when the dashboard has tabs".into(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn dashboard_tab_dto(t: crate::dashboard::DashboardTab) -> DashboardTabDto {
    DashboardTabDto { id: t.id.to_string(), name: t.name, position: t.position }
}

pub fn dashboard_detail_dto(
    d: Dashboard,
    tabs: Vec<crate::dashboard::DashboardTab>,
    cards: Vec<DashboardCard>,
) -> DashboardDetailDto {
    DashboardDetailDto {
        id: d.id.to_string(),
        name: d.name,
        description: d.description,
        parameters: d.parameters,
        created_by: d.created_by.map(|u| u.to_string()),
        updated_at: d.updated_at,
        tabs: tabs.into_iter().map(dashboard_tab_dto).collect(),
        cards: cards.into_iter().map(dashboard_card_dto).collect(),
    }
}
```

Add `tab_id` to `dashboard_card_dto`:

```rust
pub fn dashboard_card_dto(c: DashboardCard) -> DashboardCardDto {
    DashboardCardDto {
        id: c.id.to_string(),
        chart_id: c.chart_id.map(|i| i.to_string()),
        card_type: c.card_type,
        grid_col: c.grid_col,
        grid_row: c.grid_row,
        size_x: c.size_x,
        size_y: c.size_y,
        options: c.options,
        tab_id: c.tab_id.map(|i| i.to_string()),
    }
}
```

Add the import at the top of `model.rs` (it already imports `DashboardCard`, `NewCard`): extend `use crate::dashboard::{…}` with `DashboardTab`.

- [ ] **Step 5: Run the unit test to confirm it passes**

Run: `cargo test -p egret-bi validate_layout_rejects_mixed_and_out_of_range`
Expected: PASS.

- [ ] **Step 6: Wire the router**

In `crates/egret-bi/src/routes.rs`, change `db_update` to validate + call `replace_layout`:

```rust
async fn db_update(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<UpdateDashboard>,
) -> Result<Json<DashboardDetailDto>, BiApiError> {
    st.authz.require_workspace(&ctx, DB_MANAGE).await.map_err(BiApiError::from)?;
    require_dashboard_in_ws(&st, &ctx, id).await?;
    require_dashboard_owner(&st, &ctx, id).await?;
    require_cards_in_ws(&st, &ctx, &b.cards).await?;
    crate::model::validate_layout(&b.tabs, &b.cards).map_err(BiApiError::from)?;
    st.dashboards.update(id, &b.name, &b.description, &b.parameters).await.map_err(BiApiError::from)?;
    let tabs: Vec<crate::dashboard::NewTab> = b.tabs.into_iter().map(|t| t.into_new()).collect();
    let cards: Vec<crate::dashboard::NewCard> = b.cards.into_iter().map(|c| c.into_new()).collect();
    st.dashboards.replace_layout(id, &tabs, &cards).await.map_err(BiApiError::from)?;
    Ok(Json(load_dashboard(&st, id).await?))
}
```

Update `load_dashboard` to fetch tabs:

```rust
async fn load_dashboard(st: &BiState, id: Uuid) -> Result<DashboardDetailDto, BiApiError> {
    let d = st.dashboards.get(id).await.map_err(BiApiError::from)?
        .ok_or(Error::NotFound("dashboard".into()))?;
    let tabs = st.dashboards.tabs(id).await.map_err(BiApiError::from)?;
    let cards = st.dashboards.cards(id).await.map_err(BiApiError::from)?;
    Ok(crate::model::dashboard_detail_dto(d, tabs, cards))
}
```

Update the `use crate::model::{…}` import to add `NewTabInput` if the compiler asks (it is referenced only inside `model`, so usually not needed in `routes.rs`).

- [ ] **Step 7: Full sweep + commit**

Run: `cargo test -p egret-bi`
```bash
git add crates/egret-bi/src/model.rs crates/egret-bi/src/routes.rs
git commit -m "feat(dashboards): tabs in dashboard DTOs + layout validation in PUT"
```

---

## Task 3: Frontend API types

**Files:**
- Modify: `frontend/src/api/dashboards.ts`

- [ ] **Step 1: Add the tab type and thread it through**

Edit `frontend/src/api/dashboards.ts`. Add `tab_id` to `DashboardCard`:

```ts
export interface DashboardCard {
  id: string;
  chart_id: string | null;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options: Record<string, unknown>;
  tab_id: string | null;
}
```

Add the tab interface (after `DashboardCard`):

```ts
export interface DashboardTab {
  id: string;
  name: string;
  position: number;
}
```

Add `tabs` to `DashboardDetail`:

```ts
export interface DashboardDetail {
  id: string; name: string; description: string;
  created_by: string | null; updated_at: string;
  parameters: DashboardParameter[];
  tabs: DashboardTab[];
  cards: DashboardCard[];
}
```

Add `tab_index` to `CardInput`:

```ts
export interface CardInput {
  chart_id: string | null;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options?: Record<string, unknown>;
  tab_index: number | null;
}
```

Add `tabs` to the `update` mutation's body type:

```ts
  const update = useMutation({
    mutationFn: ({ id, ...b }: { id: string; name: string; description?: string; parameters: DashboardParameter[]; tabs: { name: string; position: number }[]; cards: CardInput[] }) =>
      apiJson<DashboardDetail>(`/ws/${ws}/dashboards/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: (d) => invalidate(d.id),
  });
```

- [ ] **Step 2: Typecheck (no test — pure types)**

Run (from `frontend/`): `npm run typecheck`
Expected: this will FAIL where `DashboardView.tsx` builds the PUT payload without `tabs` and where it reads cards lacking `tab_id`. That is expected; Task 9 fixes the call sites. To keep this commit green on its own, **do Step 3 now**.

- [ ] **Step 3: Keep the build green — temporary payload shim**

In `frontend/src/routes/explore/DashboardView.tsx` `onSave`, add `tabs: []` and `tab_index: null` so the existing tabless behaviour still compiles (Task 9 replaces this with real serialization):

```ts
  async function onSave() {
    const payload: CardInput[] = cards.map((c) => ({
      chart_id: c.chart_id, card_type: c.card_type,
      grid_col: c.grid_col, grid_row: c.grid_row, size_x: c.size_x, size_y: c.size_y, options: c.options,
      tab_index: null,
    }));
    try {
      await update.mutateAsync({ id: dashboard.id, name: dashboard.name, description: dashboard.description, parameters, tabs: [], cards: payload });
      setEditing(false); setDraftCards(null);
      toast.success("Saved");
    } catch (e) { toast.error(String(e)); }
  }
```

- [ ] **Step 4: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all pass (existing 342 tests still green; tabless save now sends `tabs: []`).
```bash
git add frontend/src/api/dashboards.ts frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboards): frontend tab types + tabless save payload"
```

---

## Task 4: `useDashboardTabs` hook (`?tab=<position>`)

**Files:**
- Create: `frontend/src/api/useDashboardTabs.ts`
- Test: `frontend/src/api/__tests__/useDashboardTabs.test.tsx`

- [ ] **Step 1: Write the failing test**

`frontend/src/api/__tests__/useDashboardTabs.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { useDashboardTabs } from "../useDashboardTabs";

const wrap = (initial: string) =>
  ({ children }: { children: React.ReactNode }) => <MemoryRouter initialEntries={[initial]}>{children}</MemoryRouter>;

describe("useDashboardTabs", () => {
  it("reads ?tab= and clamps into range", () => {
    const { result } = renderHook(() => useDashboardTabs(3), { wrapper: wrap("/d?tab=2") });
    expect(result.current.activeTabPos).toBe(2);
  });
  it("clamps an out-of-range ?tab= to the last tab", () => {
    const { result } = renderHook(() => useDashboardTabs(2), { wrapper: wrap("/d?tab=9") });
    expect(result.current.activeTabPos).toBe(1);
  });
  it("defaults to 0 with no ?tab=", () => {
    const { result } = renderHook(() => useDashboardTabs(3), { wrapper: wrap("/d") });
    expect(result.current.activeTabPos).toBe(0);
  });
  it("setActiveTabPos writes the param", () => {
    const { result } = renderHook(() => useDashboardTabs(3), { wrapper: wrap("/d") });
    act(() => result.current.setActiveTabPos(2));
    expect(result.current.activeTabPos).toBe(2);
  });
  it("clamps to 0 when there are no tabs", () => {
    const { result } = renderHook(() => useDashboardTabs(0), { wrapper: wrap("/d?tab=1") });
    expect(result.current.activeTabPos).toBe(0);
  });
});
```

- [ ] **Step 2: Run it to confirm it fails**

Run (from `frontend/`): `npx vitest run src/api/__tests__/useDashboardTabs.test.tsx`
Expected: FAIL (module not found).

- [ ] **Step 3: Implement the hook**

`frontend/src/api/useDashboardTabs.ts`:

```ts
import { useCallback } from "react";
import { useSearchParams } from "react-router-dom";

/** URL-synced active dashboard tab, addressed by position (`?tab=<n>`). Clamped to
 *  [0, max(0, tabCount-1)] so a stale/out-of-range link never selects a missing tab. */
export function useDashboardTabs(tabCount: number) {
  const [params, setParams] = useSearchParams();
  const maxPos = Math.max(0, tabCount - 1);
  const raw = Number(params.get("tab"));
  const activeTabPos = Number.isFinite(raw) && raw > 0 ? Math.min(raw, maxPos) : 0;

  const setActiveTabPos = useCallback((pos: number) => {
    setParams((prev) => {
      const next = new URLSearchParams(prev);
      if (pos <= 0) next.delete("tab");
      else next.set("tab", String(pos));
      return next;
    }, { replace: true });
  }, [setParams]);

  return { activeTabPos, setActiveTabPos };
}
```

- [ ] **Step 4: Run the test to confirm it passes**

Run (from `frontend/`): `npx vitest run src/api/__tests__/useDashboardTabs.test.tsx`
Expected: PASS (5 tests).

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/api/useDashboardTabs.ts frontend/src/api/__tests__/useDashboardTabs.test.tsx
git commit -m "feat(dashboards): useDashboardTabs URL-synced active tab"
```

---

## Task 5: Tab draft model — types, `startTabsDraft`, `serializeLayout`

**Files:**
- Create: `frontend/src/components/chart/dashboard/tabs.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/tabs.test.ts`

The draft model is `{ draftTabs: DraftTab[]; draftCards: DraftCard[] }`. `DraftCard` is a `DashboardCard` plus a client `tabKey` (the draft-only link to a `DraftTab`, since server tab ids churn on save). All edit mutations are pure functions over this model; the component just holds the state and calls them.

- [ ] **Step 1: Write the failing test**

`frontend/src/components/chart/dashboard/__tests__/tabs.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { startTabsDraft, serializeLayout, type DraftCard, type DraftTab } from "../tabs";
import type { DashboardCard, DashboardTab } from "@/api/dashboards";

const card = (id: string, tab_id: string | null): DashboardCard => ({
  id, chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {}, tab_id,
});

describe("startTabsDraft", () => {
  it("maps server tab_id to a draft tab key", () => {
    const tabs: DashboardTab[] = [{ id: "t1", name: "A", position: 0 }, { id: "t2", name: "B", position: 1 }];
    const cards = [card("k1", "t1"), card("k2", "t2")];
    const { draftTabs, draftCards } = startTabsDraft(tabs, cards);
    expect(draftTabs.map((t) => t.name)).toEqual(["A", "B"]);
    const keyA = draftTabs[0].key, keyB = draftTabs[1].key;
    expect(draftCards.find((c) => c.id === "k1")!.tabKey).toBe(keyA);
    expect(draftCards.find((c) => c.id === "k2")!.tabKey).toBe(keyB);
  });
  it("a tabless dashboard yields no tabs and null tabKeys", () => {
    const { draftTabs, draftCards } = startTabsDraft([], [card("k1", null)]);
    expect(draftTabs).toEqual([]);
    expect(draftCards[0].tabKey).toBeNull();
  });
});

describe("serializeLayout", () => {
  it("emits tabs by position and resolves each card's tab_index", () => {
    const draftTabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }, { key: "kb", name: "B", position: 1 }];
    const draftCards: DraftCard[] = [
      { ...card("k1", null), tabKey: "kb" },
      { ...card("k2", null), tabKey: "ka" },
    ];
    const out = serializeLayout(draftTabs, draftCards);
    expect(out.tabs).toEqual([{ name: "A", position: 0 }, { name: "B", position: 1 }]);
    expect(out.cards.find((c) => c.tab_index === 0)).toBeTruthy(); // some card on tab A (index 0)
    expect(out.cards.find((c) => c.tab_index === 1)).toBeTruthy(); // some card on tab B (index 1)
  });
  it("tabless: tabs empty and every tab_index null", () => {
    const draftCards: DraftCard[] = [{ ...card("k1", null), tabKey: null }];
    const out = serializeLayout([], draftCards);
    expect(out.tabs).toEqual([]);
    expect(out.cards[0].tab_index).toBeNull();
  });
});
```

- [ ] **Step 2: Run it to confirm it fails**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/tabs.test.ts`
Expected: FAIL (module not found).

- [ ] **Step 3: Implement the types + these two functions**

`frontend/src/components/chart/dashboard/tabs.ts`:

```ts
import type { CardInput, DashboardCard, DashboardTab } from "@/api/dashboards";

/** A tab in the edit-mode draft. `key` is a client-only id (server ids churn on save). */
export interface DraftTab { key: string; name: string; position: number }

/** A card in the edit-mode draft: the API card plus its draft tab link (`tabKey`). */
export type DraftCard = DashboardCard & { tabKey: string | null };

let seq = 0;
/** A fresh client tab key. Deterministic prefix; unique per call. */
export function newTabKey(): string { return `tab-${seq++}`; }

/** Build the edit draft from the server payload: one DraftTab per server tab (fresh key),
 *  and each card's server `tab_id` mapped to that tab's key. */
export function startTabsDraft(
  tabs: DashboardTab[],
  cards: DashboardCard[],
): { draftTabs: DraftTab[]; draftCards: DraftCard[] } {
  const ordered = [...tabs].sort((a, b) => a.position - b.position);
  const keyByServerId = new Map<string, string>();
  const draftTabs = ordered.map((t, i) => {
    const key = newTabKey();
    keyByServerId.set(t.id, key);
    return { key, name: t.name, position: i };
  });
  const draftCards = cards.map((c) => ({
    ...c, tabKey: c.tab_id ? (keyByServerId.get(c.tab_id) ?? null) : null,
  }));
  return { draftTabs, draftCards };
}

/** Serialize the draft into the PUT body shape: tabs ordered by position, each card's
 *  `tab_index` resolved from its `tabKey` (or null when there are no tabs). */
export function serializeLayout(
  draftTabs: DraftTab[],
  draftCards: DraftCard[],
): { tabs: { name: string; position: number }[]; cards: CardInput[] } {
  const ordered = [...draftTabs].sort((a, b) => a.position - b.position);
  const indexByKey = new Map(ordered.map((t, i) => [t.key, i] as const));
  return {
    tabs: ordered.map((t, i) => ({ name: t.name, position: i })),
    cards: draftCards.map((c) => ({
      chart_id: c.chart_id, card_type: c.card_type,
      grid_col: c.grid_col, grid_row: c.grid_row, size_x: c.size_x, size_y: c.size_y, options: c.options,
      tab_index: c.tabKey != null ? (indexByKey.get(c.tabKey) ?? null) : null,
    })),
  };
}
```

- [ ] **Step 4: Run the test to confirm it passes**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/tabs.test.ts`
Expected: PASS.

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/components/chart/dashboard/tabs.ts frontend/src/components/chart/dashboard/__tests__/tabs.test.ts
git commit -m "feat(dashboards): tab draft model (startTabsDraft, serializeLayout)"
```

---

## Task 6: Tab draft mutations — `addTab` (first-tab split), `renameTab`, `reorderTab`

**Files:**
- Modify: `frontend/src/components/chart/dashboard/tabs.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/tabs.test.ts`

- [ ] **Step 1: Write the failing tests**

Append to `tabs.test.ts`:

```ts
import { addTab, renameTab, reorderTab } from "../tabs";

describe("addTab", () => {
  it("first add on a tabless dashboard splits existing cards onto Tab 1 + empty Tab 2", () => {
    const draftCards: DraftCard[] = [{ ...card("k1", null), tabKey: null }];
    const r = addTab([], draftCards);
    expect(r.draftTabs.map((t) => t.name)).toEqual(["Tab 1", "Tab 2"]);
    const k1Tab = r.draftTabs[0].key;
    expect(r.draftCards.find((c) => c.id === "k1")!.tabKey).toBe(k1Tab);  // existing card on Tab 1
    expect(r.activeTabPos).toBe(1);                                       // lands on the new empty Tab 2
  });
  it("subsequent add appends one empty tab and activates it", () => {
    const start = addTab([], [{ ...card("k1", null), tabKey: null }]);
    const r = addTab(start.draftTabs, start.draftCards);
    expect(r.draftTabs.map((t) => t.name)).toEqual(["Tab 1", "Tab 2", "Tab 3"]);
    expect(r.activeTabPos).toBe(2);
  });
});

describe("renameTab", () => {
  it("renames by key", () => {
    const start = addTab([], [{ ...card("k1", null), tabKey: null }]);
    const out = renameTab(start.draftTabs, start.draftTabs[0].key, "Sales");
    expect(out[0].name).toBe("Sales");
    expect(out[1].name).toBe("Tab 2");
  });
});

describe("reorderTab", () => {
  it("moves a tab from one position to another and renumbers", () => {
    const start = addTab(addTab([], [{ ...card("k1", null), tabKey: null }]).draftTabs, []);
    // start has 3 tabs: Tab 1, Tab 2, Tab 3
    const out = reorderTab(start.draftTabs, 0, 2); // move first to last
    expect(out.map((t) => t.name)).toEqual(["Tab 2", "Tab 3", "Tab 1"]);
    expect(out.map((t) => t.position)).toEqual([0, 1, 2]);
  });
});
```

- [ ] **Step 2: Run to confirm fail**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/tabs.test.ts`
Expected: FAIL (`addTab`/`renameTab`/`reorderTab` undefined).

- [ ] **Step 3: Implement**

Append to `tabs.ts`:

```ts
/** Add a tab. On a tabless dashboard that has cards, the first add creates Tab 1 (holding the
 *  existing cards) + an empty Tab 2 and activates Tab 2 (Metabase's first-tab behaviour);
 *  every later add appends one empty tab and activates it. Returns the new active position. */
export function addTab(
  draftTabs: DraftTab[],
  draftCards: DraftCard[],
): { draftTabs: DraftTab[]; draftCards: DraftCard[]; activeTabPos: number } {
  if (draftTabs.length === 0) {
    const tab1 = { key: newTabKey(), name: "Tab 1", position: 0 };
    const tab2 = { key: newTabKey(), name: "Tab 2", position: 1 };
    return {
      draftTabs: [tab1, tab2],
      draftCards: draftCards.map((c) => ({ ...c, tabKey: tab1.key })),  // existing cards → Tab 1
      activeTabPos: 1,
    };
  }
  const next = [...draftTabs, { key: newTabKey(), name: `Tab ${draftTabs.length + 1}`, position: draftTabs.length }];
  return { draftTabs: next, draftCards, activeTabPos: next.length - 1 };
}

/** Rename the tab with `key`. */
export function renameTab(draftTabs: DraftTab[], key: string, name: string): DraftTab[] {
  return draftTabs.map((t) => (t.key === key ? { ...t, name } : t));
}

/** Move the tab at `fromPos` to `toPos` (positions renumbered 0..n-1). */
export function reorderTab(draftTabs: DraftTab[], fromPos: number, toPos: number): DraftTab[] {
  const ordered = [...draftTabs].sort((a, b) => a.position - b.position);
  const [moved] = ordered.splice(fromPos, 1);
  ordered.splice(toPos, 0, moved);
  return ordered.map((t, i) => ({ ...t, position: i }));
}
```

- [ ] **Step 4: Run to confirm pass**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/tabs.test.ts`
Expected: PASS.

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/components/chart/dashboard/tabs.ts frontend/src/components/chart/dashboard/__tests__/tabs.test.ts
git commit -m "feat(dashboards): addTab/renameTab/reorderTab draft mutations"
```

---

## Task 7: Tab draft mutations — `deleteTab` (reassign vs delete), `duplicateTab`, `moveCardToTab`

**Files:**
- Modify: `frontend/src/components/chart/dashboard/tabs.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/tabs.test.ts`

`duplicateTab` and `moveCardToTab` place cards on the destination tab's own 0-based grid, below its current max row (the same rule `DashboardView.addCard` uses today). Helper `nextRowFor(cards)` computes that.

- [ ] **Step 1: Write the failing tests**

Append to `tabs.test.ts`:

```ts
import { deleteTab, duplicateTab, moveCardToTab } from "../tabs";

const positioned = (id: string, tabKey: string | null, row: number, h = 4): DraftCard =>
  ({ ...card(id, null), tabKey, grid_row: row, size_y: h });

describe("deleteTab", () => {
  it("reassign mode moves the tab's cards to the target tab", () => {
    const tabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }, { key: "kb", name: "B", position: 1 }];
    const cards = [positioned("k1", "ka", 0), positioned("k2", "kb", 0)];
    const out = deleteTab(tabs, cards, "ka", { mode: "reassign", toKey: "kb" });
    expect(out.draftTabs.map((t) => t.name)).toEqual(["B"]);
    expect(out.draftCards.every((c) => c.tabKey === "kb")).toBe(true);   // k1 moved to B
  });
  it("delete-cards mode drops the tab and its cards", () => {
    const tabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }, { key: "kb", name: "B", position: 1 }];
    const cards = [positioned("k1", "ka", 0), positioned("k2", "kb", 0)];
    const out = deleteTab(tabs, cards, "ka", { mode: "delete" });
    expect(out.draftCards.map((c) => c.id)).toEqual(["k2"]);
  });
});

describe("duplicateTab", () => {
  it("clones the tab's cards into a new tab with fresh ids", () => {
    const tabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }];
    const cards = [positioned("k1", "ka", 0)];
    const out = duplicateTab(tabs, cards, "ka");
    expect(out.draftTabs).toHaveLength(2);
    const newKey = out.draftTabs[1].key;
    const clones = out.draftCards.filter((c) => c.tabKey === newKey);
    expect(clones).toHaveLength(1);
    expect(clones[0].id).not.toBe("k1");   // fresh client id
    expect(out.activeTabPos).toBe(1);
  });
});

describe("moveCardToTab", () => {
  it("reassigns a card and stacks it below the destination's max row", () => {
    const cards = [positioned("k1", "ka", 0, 4), positioned("k2", "kb", 0, 4)];
    const out = moveCardToTab(cards, "k1", "kb");
    const moved = out.find((c) => c.id === "k1")!;
    expect(moved.tabKey).toBe("kb");
    expect(moved.grid_row).toBe(4);   // below k2 (row 0 + height 4)
  });
});
```

- [ ] **Step 2: Run to confirm fail**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/tabs.test.ts`
Expected: FAIL (`deleteTab`/`duplicateTab`/`moveCardToTab` undefined).

- [ ] **Step 3: Implement**

Append to `tabs.ts`:

```ts
/** Next free grid row on a tab: below the current max (row + height) of its cards. */
function nextRowFor(cards: DraftCard[]): number {
  return cards.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
}

/** Delete the tab with `key`. `reassign` moves its cards to `toKey`; `delete` drops them.
 *  Positions on the remaining tabs are renumbered. (Callers gate this to ≥2 tabs.) */
export function deleteTab(
  draftTabs: DraftTab[],
  draftCards: DraftCard[],
  key: string,
  opts: { mode: "reassign"; toKey: string } | { mode: "delete" },
): { draftTabs: DraftTab[]; draftCards: DraftCard[] } {
  const remaining = draftTabs
    .filter((t) => t.key !== key)
    .sort((a, b) => a.position - b.position)
    .map((t, i) => ({ ...t, position: i }));
  let cards: DraftCard[];
  if (opts.mode === "reassign") {
    const dest = draftCards.filter((c) => c.tabKey === opts.toKey);
    let row = nextRowFor(dest);
    cards = draftCards.map((c) => {
      if (c.tabKey !== key) return c;
      const placed = { ...c, tabKey: opts.toKey, grid_row: row };
      row += c.size_y;
      return placed;
    });
  } else {
    cards = draftCards.filter((c) => c.tabKey !== key);
  }
  return { draftTabs: remaining, draftCards: cards };
}

/** Duplicate the tab with `key`: a new tab inserted right after it, holding deep copies of its
 *  cards (fresh client ids, same grid). Activates the new tab. */
export function duplicateTab(
  draftTabs: DraftTab[],
  draftCards: DraftCard[],
  key: string,
): { draftTabs: DraftTab[]; draftCards: DraftCard[]; activeTabPos: number } {
  const ordered = [...draftTabs].sort((a, b) => a.position - b.position);
  const srcPos = ordered.findIndex((t) => t.key === key);
  const newKey = newTabKey();
  const newTab = { key: newKey, name: `${ordered[srcPos].name} copy`, position: srcPos + 1 };
  const next = [...ordered.slice(0, srcPos + 1), newTab, ...ordered.slice(srcPos + 1)]
    .map((t, i) => ({ ...t, position: i }));
  const clones = draftCards
    .filter((c) => c.tabKey === key)
    .map((c) => ({ ...c, id: `new-dup-${seq++}`, tabKey: newKey }));
  return { draftTabs: next, draftCards: [...draftCards, ...clones], activeTabPos: srcPos + 1 };
}

/** Move the card `cardId` to tab `toKey`, stacking it below that tab's current max row. */
export function moveCardToTab(draftCards: DraftCard[], cardId: string, toKey: string): DraftCard[] {
  const row = nextRowFor(draftCards.filter((c) => c.tabKey === toKey));
  return draftCards.map((c) => (c.id === cardId ? { ...c, tabKey: toKey, grid_col: 0, grid_row: row } : c));
}
```

- [ ] **Step 4: Run to confirm pass**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/tabs.test.ts`
Expected: PASS.

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/components/chart/dashboard/tabs.ts frontend/src/components/chart/dashboard/__tests__/tabs.test.ts
git commit -m "feat(dashboards): deleteTab/duplicateTab/moveCardToTab draft mutations"
```

---

## Task 8: `DashboardTabBar` component

**Files:**
- Create: `frontend/src/components/chart/dashboard/DashboardTabBar.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx`

The bar is presentational: it renders tabs, marks the active one, switches on click, and — in edit mode — exposes add / rename (double-click) / delete / duplicate controls and is drag-reorderable. It takes callbacks; all draft mutation lives in the parent (Task 9) via the Task 5–7 helpers.

- [ ] **Step 1: Write the failing test**

`frontend/src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DashboardTabBar } from "../DashboardTabBar";
import type { DraftTab } from "../tabs";

const tabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }, { key: "kb", name: "B", position: 1 }];

describe("DashboardTabBar", () => {
  it("renders tabs and switches on click", () => {
    const onSelect = vi.fn();
    render(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={false}
      onSelect={onSelect} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    fireEvent.click(screen.getByRole("tab", { name: "B" }));
    expect(onSelect).toHaveBeenCalledWith(1);
  });
  it("shows the add button only in edit mode", () => {
    const { rerender } = render(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={false}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "add tab" })).toBeNull();
    rerender(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={true}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    expect(screen.getByRole("button", { name: "add tab" })).toBeTruthy();
  });
  it("double-click commits a rename in edit mode", () => {
    const onRename = vi.fn();
    render(<DashboardTabBar tabs={tabs} activeTabPos={0} editing={true}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={onRename} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    fireEvent.doubleClick(screen.getByRole("tab", { name: "A" }));
    const input = screen.getByLabelText("rename tab A") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "Sales" } });
    fireEvent.blur(input);
    expect(onRename).toHaveBeenCalledWith("ka", "Sales");
  });
  it("delete is disabled with fewer than 2 tabs", () => {
    render(<DashboardTabBar tabs={[tabs[0]]} activeTabPos={0} editing={true}
      onSelect={vi.fn()} onAdd={vi.fn()} onRename={vi.fn()} onReorder={vi.fn()} onDelete={vi.fn()} onDuplicate={vi.fn()} />);
    expect((screen.getByRole("button", { name: "delete tab A" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
```

- [ ] **Step 2: Run to confirm fail**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx`
Expected: FAIL (module not found).

- [ ] **Step 3: Implement the component**

`frontend/src/components/chart/dashboard/DashboardTabBar.tsx`:

```tsx
import { useState } from "react";
import type { DraftTab } from "./tabs";

interface Props {
  tabs: DraftTab[];
  activeTabPos: number;
  editing: boolean;
  onSelect: (pos: number) => void;
  onAdd: () => void;
  onRename: (key: string, name: string) => void;
  onReorder: (fromPos: number, toPos: number) => void;
  onDelete: (key: string) => void;
  onDuplicate: (key: string) => void;
}

/** The dashboard tab bar. In view mode the parent only renders this when `tabs.length >= 2`;
 *  in edit mode it is always rendered (so the first tab can be added). */
export function DashboardTabBar({ tabs, activeTabPos, editing, onSelect, onAdd, onRename, onReorder, onDelete, onDuplicate }: Props) {
  const ordered = [...tabs].sort((a, b) => a.position - b.position);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [dragFrom, setDragFrom] = useState<number | null>(null);

  return (
    <div className="mb-3 flex items-center gap-1 border-b" role="tablist">
      {ordered.map((t, pos) => {
        const active = pos === activeTabPos;
        const cls = `flex items-center gap-1 px-3 py-1.5 text-sm border-b-2 ${active ? "border-foreground font-medium" : "border-transparent text-muted-foreground"}`;
        if (renaming === t.key && editing) {
          return (
            <input key={t.key} autoFocus aria-label={`rename tab ${t.name}`} defaultValue={t.name}
              className="m-0.5 rounded border px-1 text-sm"
              onBlur={(e) => { onRename(t.key, e.target.value.trim() || t.name); setRenaming(null); }}
              onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); if (e.key === "Escape") setRenaming(null); }} />
          );
        }
        return (
          <div key={t.key} className="flex items-center"
            draggable={editing}
            onDragStart={() => setDragFrom(pos)}
            onDragOver={(e) => editing && e.preventDefault()}
            onDrop={() => { if (editing && dragFrom !== null && dragFrom !== pos) onReorder(dragFrom, pos); setDragFrom(null); }}>
            <button type="button" role="tab" aria-selected={active} aria-label={t.name} className={cls}
              onClick={() => onSelect(pos)}
              onDoubleClick={() => { if (editing) setRenaming(t.key); }}>
              {t.name}
            </button>
            {editing && (
              <>
                <button type="button" aria-label={`duplicate tab ${t.name}`} className="px-1 text-xs text-muted-foreground" onClick={() => onDuplicate(t.key)}>⧉</button>
                <button type="button" aria-label={`delete tab ${t.name}`} className="px-1 text-xs text-destructive disabled:opacity-40"
                  disabled={ordered.length < 2} onClick={() => onDelete(t.key)}>✕</button>
              </>
            )}
          </div>
        );
      })}
      {editing && (
        <button type="button" aria-label="add tab" className="px-2 py-1 text-sm text-muted-foreground hover:text-foreground" onClick={onAdd}>+</button>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Run to confirm pass**

Run (from `frontend/`): `npx vitest run src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx`
Expected: PASS (4 tests).

- [ ] **Step 5: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/components/chart/dashboard/DashboardTabBar.tsx frontend/src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx
git commit -m "feat(dashboards): DashboardTabBar component"
```

---

## Task 9: Wire tabs into `DashboardView` (state, filtering, lazy mount, save)

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/routes/explore/__tests__/DashboardView.tabs.test.tsx`

This task makes a tabbed dashboard render the active tab only and round-trip through the PUT. Edit affordances (add/rename/reorder/delete dialog/duplicate/move-card) are wired in Task 10; here we add the state, the bar, filtering, lazy mount, and save serialization. The move-card affordance and delete-confirm dialog are added next.

> **Rules-of-Hooks (critical):** every new hook (`useDashboardTabs`, any `useState`) MUST be declared **above** the `if (isLoading || !dash) return …` early return at line ~65. This component has crashed twice from hooks placed below it.

- [ ] **Step 1: Write the failing test**

`frontend/src/routes/explore/__tests__/DashboardView.tabs.test.tsx`. Mirror the mocking style already used by `DashboardView.clickbehavior.test.tsx` (mock `DashboardCardView`, `@/api/dashboards`, `@/api/charts`, router). The key assertions: only the active tab's cards mount, and clicking a tab switches which cards mount.

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

// Render each card as a button exposing its id, so we can assert which cards are mounted.
vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card }: { card: { id: string } }) => <div data-testid={`card-${card.id}`} />,
}));
vi.mock("@/api/charts", () => ({ useCharts: () => ({ data: [] }) }));

const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [],
  tabs: [{ id: "t1", name: "A", position: 0 }, { id: "t2", name: "B", position: 1 }],
  cards: [
    { id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: "t1" },
    { id: "c2", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 1, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: "t2" },
  ],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod,
    useDashboard: () => ({ data: detail, isLoading: false }),
    useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
  };
});

const renderView = (path = "/explore/eng/dashboards/d1") => {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[path]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
};

describe("DashboardView tabs", () => {
  it("shows the tab bar and mounts only the active tab's cards", () => {
    renderView();
    expect(screen.getByRole("tab", { name: "A" })).toBeTruthy();
    expect(screen.getByTestId("card-c1")).toBeTruthy();      // tab A active
    expect(screen.queryByTestId("card-c2")).toBeNull();      // tab B not mounted
  });
  it("switching tabs mounts the other tab's cards", () => {
    renderView();
    fireEvent.click(screen.getByRole("tab", { name: "B" }));
    expect(screen.getByTestId("card-c2")).toBeTruthy();
    expect(screen.queryByTestId("card-c1")).toBeNull();
  });
  it("honours ?tab= deep link", () => {
    renderView("/explore/eng/dashboards/d1?tab=1");
    expect(screen.getByTestId("card-c2")).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run to confirm fail**

Run (from `frontend/`): `npx vitest run src/routes/explore/__tests__/DashboardView.tabs.test.tsx`
Expected: FAIL (no tab bar; both cards render).

- [ ] **Step 3: Add imports + tab state (above the early return)**

In `DashboardView.tsx`, add imports near the others:

```ts
import { useDashboardTabs } from "@/api/useDashboardTabs";
import { DashboardTabBar } from "@/components/chart/dashboard/DashboardTabBar";
import { startTabsDraft, serializeLayout, addTab, renameTab, reorderTab, deleteTab, duplicateTab, moveCardToTab, type DraftTab, type DraftCard } from "@/components/chart/dashboard/tabs";
```

Add state + the hook **above** the `if (isLoading || !dash) return …` line (next to the existing `drillMenu`/`records` states):

```ts
  const [draftTabs, setDraftTabs] = useState<DraftTab[] | null>(null);
  const serverTabCount = dash?.tabs.length ?? 0;
  const { activeTabPos, setActiveTabPos } = useDashboardTabs(
    editing ? (draftTabs?.length ?? 0) : serverTabCount,
  );
```

- [ ] **Step 4: Derive tabs, active tab, and visible cards (after the early return)**

After `const cards = editing && draftCards ? draftCards : dashboard.cards;` add:

```ts
  const tabs: DraftTab[] = editing
    ? (draftTabs ?? [])
    : dashboard.tabs.map((t, i) => ({ key: t.id, name: t.name, position: i }));
  const activeTab = tabs[activeTabPos] ?? null;
  // In edit mode draft cards carry `tabKey`; in view mode server cards carry `tab_id`.
  const visibleCards = tabs.length === 0
    ? cards
    : cards.filter((c) =>
        editing ? (c as DraftCard).tabKey === activeTab?.key : c.tab_id === activeTab?.key,
      );
```

Then change the grid + layout to use `visibleCards` instead of `cards`: in the `layout` memo (line ~137) map over `visibleCards`, in the empty check use `cards.length === 0` (keep), and in `<Grid>` map `visibleCards.map((c) => …)` for the children. (Leave `cards` as the source for save and counts.)

- [ ] **Step 5: Build the draft on edit, serialize on save**

Update `startEdit` to seed the tab draft:

```ts
  function startEdit() {
    const { draftTabs: dt, draftCards: dc } = startTabsDraft(dashboard.tabs, dashboard.cards);
    setDraftCards(dc);
    setDraftParameters((dashboard.parameters ?? []).map((p) => ({ ...p })));
    setDraftTabs(dt);
    setEditing(true);
  }
  function cancelEdit() { setEditing(false); setDraftCards(null); setDraftParameters(null); setDraftTabs(null); }
```

Replace `onSave` to serialize tabs + cards:

```ts
  async function onSave() {
    const { tabs: tabPayload, cards: cardPayload } = serializeLayout(draftTabs ?? [], (cards as DraftCard[]));
    try {
      await update.mutateAsync({ id: dashboard.id, name: dashboard.name, description: dashboard.description, parameters, tabs: tabPayload, cards: cardPayload });
      setEditing(false); setDraftCards(null); setDraftTabs(null);
      toast.success("Saved");
    } catch (e) { toast.error(String(e)); }
  }
```

> Note: `draftCards` must now hold `DraftCard` (with `tabKey`). `addCard`/`addVirtualCard`/`duplicateCard` create cards without `tabKey`; set their `tabKey` to the active tab's key (or `null` when tabless). Update those three helpers to add `tabKey: activeTab?.key ?? null` to the new card object. This keeps a newly added card on the tab you're viewing.

- [ ] **Step 6: Render the bar**

Just before the `<DashboardFilterBar …/>` line, render the bar when it should show (view: ≥2 tabs; edit: always):

```tsx
      {(editing || tabs.length >= 2) && (
        <DashboardTabBar
          tabs={tabs} activeTabPos={activeTabPos} editing={editing}
          onSelect={setActiveTabPos}
          onAdd={() => { const r = addTab(draftTabs ?? [], (cards as DraftCard[])); setDraftTabs(r.draftTabs); setDraftCards(r.draftCards); setActiveTabPos(r.activeTabPos); }}
          onRename={(key, name) => setDraftTabs(renameTab(draftTabs ?? [], key, name))}
          onReorder={(from, to) => setDraftTabs(reorderTab(draftTabs ?? [], from, to))}
          onDelete={(key) => { /* Task 10: confirm dialog */ const r = deleteTab(draftTabs ?? [], (cards as DraftCard[]), key, { mode: "delete" }); setDraftTabs(r.draftTabs); setDraftCards(r.draftCards); setActiveTabPos(Math.min(activeTabPos, r.draftTabs.length - 1)); }}
          onDuplicate={(key) => { const r = duplicateTab(draftTabs ?? [], (cards as DraftCard[]), key); setDraftTabs(r.draftTabs); setDraftCards(r.draftCards); setActiveTabPos(r.activeTabPos); }}
        />
      )}
```

(The `onDelete` here uses plain delete; Task 10 swaps in the reassign-vs-delete dialog and the move-card affordance.)

- [ ] **Step 7: Run the test to confirm it passes**

Run (from `frontend/`): `npx vitest run src/routes/explore/__tests__/DashboardView.tabs.test.tsx`
Expected: PASS (3 tests). Also run the existing `DashboardView.test.tsx` + `DashboardView.clickbehavior.test.tsx` to confirm tabless dashboards still render (they have `tabs: []`).

- [ ] **Step 8: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.tabs.test.tsx
git commit -m "feat(dashboards): tab bar + lazy per-tab rendering in DashboardView"
```

---

## Task 10: Delete-with-reassign dialog + move-card-to-tab affordance

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/routes/explore/__tests__/DashboardView.tabs.test.tsx`

- [ ] **Step 1: Write the failing tests**

Append to `DashboardView.tabs.test.tsx` (these drive edit mode). Add a helper to enter edit mode (`fireEvent.click(screen.getByRole("button", { name: "Edit" }))`).

```ts
describe("DashboardView tab edit affordances", () => {
  it("deleting a tab with cards opens a reassign dialog and reassigns", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "delete tab A" }));
    // dialog appears: reassign target select + confirm
    const target = screen.getByLabelText("reassign target") as HTMLSelectElement;
    fireEvent.change(target, { target: { value: target.options[0].value } });
    fireEvent.click(screen.getByRole("button", { name: "reassign and delete" }));
    // tab A gone; its card c1 now lives on tab B (switch to B and see both)
    expect(screen.queryByRole("tab", { name: "A" })).toBeNull();
  });

  it("a chart card shows a move-to-tab control in edit mode", () => {
    // (this test uses a chart card; see note — the heading-only fixture has no chart card,
    //  so assert the control's presence on a chart card fixture variant)
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("move card c1 to tab")).toBeTruthy();
  });
});
```

> Note for the implementer: the move-to-tab `<select>` is rendered for **every** editable card (chart and virtual), so the heading fixture cards (`c1`/`c2`) qualify. If you scope it to chart cards only, switch the fixture's `c1` to a chart card with a stub `chart_id` and add it to the mocked `useCharts`.

- [ ] **Step 2: Run to confirm fail**

Run (from `frontend/`): `npx vitest run src/routes/explore/__tests__/DashboardView.tabs.test.tsx`
Expected: FAIL (no dialog, no move control).

- [ ] **Step 3: Add a delete-confirm dialog state + handler**

Add state above the early return:

```ts
  const [deletingTabKey, setDeletingTabKey] = useState<string | null>(null);
```

Change the bar's `onDelete` to open the dialog only when the tab has cards (otherwise delete immediately):

```tsx
          onDelete={(key) => {
            const hasCards = (cards as DraftCard[]).some((c) => c.tabKey === key);
            if (!hasCards) {
              const r = deleteTab(draftTabs ?? [], (cards as DraftCard[]), key, { mode: "delete" });
              setDraftTabs(r.draftTabs); setDraftCards(r.draftCards);
              setActiveTabPos(Math.min(activeTabPos, r.draftTabs.length - 1));
            } else {
              setDeletingTabKey(key);
            }
          }}
```

Render the dialog near the existing Replace dialog (`replacingId`), using the same `Dialog`/`DialogContent` primitives:

```tsx
      <Dialog open={deletingTabKey !== null} onOpenChange={(o) => { if (!o) setDeletingTabKey(null); }}>
        <DialogContent>
          <DialogHeader><DialogTitle>Delete tab</DialogTitle></DialogHeader>
          <p className="text-sm text-muted-foreground">This tab has cards. Move them to another tab, or delete them with the tab.</p>
          {(() => {
            const others = (draftTabs ?? []).filter((t) => t.key !== deletingTabKey);
            return (
              <div className="flex flex-col gap-2 py-2">
                <label className="flex items-center gap-2 text-sm">
                  <span>Move cards to</span>
                  <select aria-label="reassign target" className="h-8 rounded border bg-background px-1 text-sm" defaultValue={others[0]?.key}>
                    {others.map((t) => <option key={t.key} value={t.key}>{t.name}</option>)}
                  </select>
                </label>
                <div className="flex justify-end gap-2">
                  <Button size="sm" variant="outline" aria-label="reassign and delete"
                    onClick={(e) => {
                      const sel = (e.currentTarget.closest("[data-slot=dialog-content]")?.querySelector("[aria-label='reassign target']") as HTMLSelectElement | null);
                      const toKey = sel?.value ?? others[0]?.key;
                      const r = deleteTab(draftTabs ?? [], (cards as DraftCard[]), deletingTabKey!, { mode: "reassign", toKey });
                      setDraftTabs(r.draftTabs); setDraftCards(r.draftCards);
                      setActiveTabPos(Math.min(activeTabPos, r.draftTabs.length - 1));
                      setDeletingTabKey(null);
                    }}>reassign and delete</Button>
                  <Button size="sm" variant="ghost" className="text-destructive" aria-label="delete cards too"
                    onClick={() => {
                      const r = deleteTab(draftTabs ?? [], (cards as DraftCard[]), deletingTabKey!, { mode: "delete" });
                      setDraftTabs(r.draftTabs); setDraftCards(r.draftCards);
                      setActiveTabPos(Math.min(activeTabPos, r.draftTabs.length - 1));
                      setDeletingTabKey(null);
                    }}>delete cards too</Button>
                </div>
              </div>
            );
          })()}
        </DialogContent>
      </Dialog>
```

> The `reassign target` select is uncontrolled; the confirm handler reads the current value via the dialog DOM. Simpler alternative (preferred if the implementer wants pure state): lift the selected target into a `reassignTo` state initialized when the dialog opens. Either is acceptable; keep whichever passes the test cleanly.

- [ ] **Step 4: Add the move-card-to-tab control**

Inside the card-editing block (where the title/“On click” controls render for editing cards, around line 277–357), add a move-to-tab `<select>` when there are ≥2 tabs:

```tsx
              {editing && tabs.length >= 2 && (
                <label className="m-1 flex items-center justify-between gap-1 border-t pt-1 text-[10px] text-muted-foreground">
                  <span>Move to tab</span>
                  <select aria-label={`move card ${c.id} to tab`} className="h-6 rounded border bg-background text-[10px]"
                    value={(c as DraftCard).tabKey ?? ""}
                    onChange={(e) => setDraftCards(moveCardToTab((cards as DraftCard[]), c.id, e.target.value))}>
                    {tabs.map((t) => <option key={t.key} value={t.key}>{t.name}</option>)}
                  </select>
                </label>
              )}
```

This must render for every editable card (so it sits outside the `c.card_type === "chart"` guard — place it right after the remove/duplicate button row, inside the `{editing && (...)}` region but not gated on chart type). Map over `visibleCards` already provides `c`.

- [ ] **Step 5: Run the tests to confirm they pass**

Run (from `frontend/`): `npx vitest run src/routes/explore/__tests__/DashboardView.tabs.test.tsx`
Expected: PASS.

- [ ] **Step 6: Full sweep + commit**

Run (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`
```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.tabs.test.tsx
git commit -m "feat(dashboards): tab delete-with-reassign dialog + move-card-to-tab"
```

---

## Task 11: Live verification + PR

**Files:** none (manual verification).

- [ ] **Step 1: Start the stack**

Per the run-app notes: backend `cargo run -p egret-bin -- serve` (after the migration re-embed `touch crates/egret-metastore/src/lib.rs`), frontend `npm run dev`. Log in `admin@egret.local` / `Admin12345!`, workspace `eng`.

- [ ] **Step 2: Drive the Playwright MCP browser** (manual — egret has no automated Playwright suite)

Create a throwaway dashboard ("Smoke D2"). Verify, watching the console for **0 errors** throughout:
1. Edit → add a chart card → click **+** on the tab bar → confirm Tab 1 (your card) + empty Tab 2 appear and Tab 2 is active.
2. Add a card on Tab 2; switch tabs → only the active tab's card is shown (check the network panel: only the active tab's card query fires).
3. Double-click a tab → rename; drag to reorder; **Save**; reload → tabs + names + order persisted, `?tab=` deep-links the active tab.
4. Edit → "Move to tab" on a card → it moves; delete a tab with cards → reassign dialog → reassign → cards land on the target tab; delete a tab → "delete cards too" drops them.
5. Open the existing `a1` dashboard → confirm it still renders with **no** tab bar (tabless, untouched).

- [ ] **Step 3: Clean up + full sweep**

Delete the "Smoke D2" dashboard (only the one you created — never touch `a1`). Run the final sweep: from `frontend/` `npm run typecheck && npm run lint && npm run build && npx vitest run`, and `cargo test -p egret-bi`.

- [ ] **Step 4: Push + PR**

```bash
git push -u origin feat/dashboards-phase-d2
gh pr create --base main --title "feat(dashboards): Phase D2 — dashboard tabs" --body "<summary, changes, testing, Metabase-divergences, 🤖 footer>"
```
Then await the user to merge.

---

## Notes for the implementer

- **Rules of Hooks:** all new hooks in `DashboardView` go above `if (isLoading || !dash) return …`. Regressed twice before.
- **`DraftCard` vs `DashboardCard`:** in edit mode `draftCards` holds `DraftCard` (adds `tabKey`); the render/layout code only reads fields common to both, so the cast `(cards as DraftCard[])` is safe where `tabKey` is needed. `onSave`/serialize operate on `DraftCard`.
- **Tabless dashboards stay byte-for-byte unchanged:** `tabs: []` → no bar, `visibleCards === cards`, `tab_index: null`, `tab_id: null`. The existing `DashboardView.test.tsx` and `DashboardView.clickbehavior.test.tsx` fixtures must keep passing (add `tabs: []` and `tab_id: null` to their mocked detail/cards if the types now require them).
- **Migration staleness:** `touch crates/egret-metastore/src/lib.rs` after touching `migrations/` before any `cargo run`/`cargo test`.
