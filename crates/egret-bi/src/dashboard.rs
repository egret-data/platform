//! Dashboard repository: a dashboard row plus its placed cards (grid layout).
//!
//! Card layout is saved whole-list: the client sends the full set of tabs + cards on each
//! update and `replace_layout` swaps them in a transaction (Metabase-style layout PUT),
//! which keeps grid edits atomic and avoids per-card diffing.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct DashboardRepo {
    db: Db,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Dashboard {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub created_by: Option<Uuid>,
    pub folder_id: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// NULL for every dashboard that predates migration 0106 — see that file for why it is not
    /// backfilled. The UI renders the "Added" line only when this is present.
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Who last saved this dashboard. NULL until the first save after 0106, and NULL again if
    /// that user is hard-deleted (0093's ON DELETE SET NULL treatment).
    pub updated_by: Option<Uuid>,
    pub width: String,
    /// Certification (0108): a stewardship mark, written only through `set_certification`
    /// behind the platform-scoped `dashboard:certify` perm — never through the ordinary
    /// dashboard PUT, so an editor cannot certify their own work by saving it.
    pub certified: bool,
    pub certified_by: String,
    pub certification_details: String,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

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

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DashboardTab {
    pub id: Uuid,
    pub name: String,
    pub position: i32,
}

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

/// A tab as supplied by the client when saving a layout. `id` is present when the client is
/// re-sending a tab that already exists; see [`NewCard::id`].
#[derive(Debug, Clone)]
pub struct NewTab {
    pub id: Option<Uuid>,
    pub name: String,
    pub position: i32,
}

/// Private struct for list() — sqlx tuple FromRow is version-sensitive; use flatten instead.
#[derive(sqlx::FromRow)]
struct DashboardWithCount {
    #[sqlx(flatten)]
    dash: Dashboard,
    card_count: i64,
}

#[derive(sqlx::FromRow)]
struct DashboardWithCountBm {
    #[sqlx(flatten)]
    dash: Dashboard,
    card_count: i64,
    bookmarked: bool,
}

/// Private struct for attribution_names — sqlx tuple FromRow is version-sensitive, same reason
/// DashboardWithCount exists.
#[derive(sqlx::FromRow)]
struct AttributionNames {
    created_by_name: Option<String>,
    updated_by_name: Option<String>,
}

/// A card as supplied by the client when saving a layout.
#[derive(Debug, Clone)]
pub struct NewCard {
    /// The card's existing id, when it has one. `None` is a genuinely new card.
    pub id: Option<Uuid>,
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

const COLS: &str =
    "id, workspace_id, name, description, parameters, created_by, folder_id, updated_at, created_at, updated_by, width, certified, certified_by, certification_details, deleted_at";
const CARD_COLS: &str =
    "id, dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal, tab_id";
const REV_COLS: &str = "id, dashboard_id, created_by, created_at, kind, snapshot";

/// How many revisions one dashboard keeps.
///
/// Metabase keeps 15 (`revision/max-revisions`). egret keeps 20 for a shape reason rather than a
/// taste one: Metabase writes a revision per small action — add card, move card, rename tab are
/// each their own API call — so fifteen Metabase revisions can be fifteen minutes of a single
/// session. egret has exactly one write path, the whole-layout PUT behind the Save button, so one
/// egret revision is a whole editing session and twenty of them is months of calendar history.
const MAX_REVISIONS_PER_DASHBOARD: i64 = 20;

/// Map a unique-violation to a Conflict; pass everything else through.
fn map_conflict_err(e: sqlx::Error) -> BiError {
    if let sqlx::Error::Database(ref dbe) = e {
        if dbe.is_unique_violation() {
            return BiError::Conflict("a dashboard with that name already exists".into());
        }
    }
    BiError::Db(e)
}

impl DashboardRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn create(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        created_by: Option<Uuid>,
    ) -> Result<Dashboard, BiError> {
        let sql = format!(
            "INSERT INTO analytics.dashboards (workspace_id, name, description, created_by) \
             VALUES ($1,$2,$3,$4) RETURNING {COLS}"
        );
        sqlx::query_as::<_, Dashboard>(sqlx::AssertSqlSafe(sql))
            .bind(workspace_id)
            .bind(name)
            .bind(description)
            .bind(created_by)
            .fetch_one(&self.db.pool)
            .await
            .map_err(map_conflict_err)
    }

    pub async fn get(&self, id: Uuid, workspace_id: Uuid) -> Result<Option<Dashboard>, BiError> {
        let sql =
            format!("SELECT {COLS} FROM analytics.dashboards WHERE id = $1 AND workspace_id = $2");
        Ok(sqlx::query_as::<_, Dashboard>(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .bind(workspace_id)
            .fetch_optional(&self.db.pool)
            .await?)
    }

    /// Dashboards in a workspace, each with its card count, newest first.
    /// `trashed` selects the trash bin (soft-deleted) instead of the live list.
    pub async fn list_by_workspace(
        &self,
        workspace_id: Uuid,
        trashed: bool,
    ) -> Result<Vec<(Dashboard, i64)>, BiError> {
        let pred = if trashed {
            "deleted_at IS NOT NULL"
        } else {
            "deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM analytics.dashboard_cards c WHERE c.dashboard_id = dashboards.id) AS card_count \
             FROM analytics.dashboards WHERE workspace_id = $1 AND {pred} ORDER BY updated_at DESC"
        );
        let rows = sqlx::query_as::<_, DashboardWithCount>(sqlx::AssertSqlSafe(sql))
            .bind(workspace_id)
            .fetch_all(&self.db.pool)
            .await?;
        Ok(rows.into_iter().map(|r| (r.dash, r.card_count)).collect())
    }

    /// Soft-delete (trashed=true → deleted_at=now()) or restore (false → NULL).
    pub async fn set_trashed(&self, id: Uuid, trashed: bool) -> Result<(), BiError> {
        let sql = if trashed {
            "UPDATE analytics.dashboards SET deleted_at = now() WHERE id = $1"
        } else {
            "UPDATE analytics.dashboards SET deleted_at = NULL WHERE id = $1"
        };
        sqlx::query(sql)
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(map_conflict_err)?;
        Ok(())
    }

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

    /// Set or withdraw certification (0108).
    ///
    /// SEPARATE FROM `update` ON PURPOSE. Folding these columns into the ordinary dashboard PUT
    /// would mean anyone holding `dashboard:manage` could certify by saving — which is exactly
    /// the self-certification that makes the badge worthless. The route guards this one with the
    /// platform-scoped `dashboard:certify`, and `update` cannot reach these columns at all.
    ///
    /// Withdrawing clears the attribution rather than leaving a stale certifier behind: a
    /// dashboard that says "not certified, by Ana" is a worse answer than saying nothing.
    /// `updated_at` is deliberately NOT touched — certifying is not editing, and moving it would
    /// misreport who last changed the dashboard's content.
    pub async fn set_certification(
        &self,
        id: Uuid,
        certified: bool,
        certified_by: &str,
        details: &str,
    ) -> Result<(), BiError> {
        sqlx::query(
            "UPDATE analytics.dashboards SET certified=$2, certified_by=$3, certification_details=$4 WHERE id=$1",
        )
        .bind(id)
        .bind(certified)
        .bind(if certified { certified_by } else { "" })
        .bind(if certified { details } else { "" })
        .execute(&self.db.pool)
        .await
        .map_err(map_conflict_err)?;
        Ok(())
    }

    /// Permanently delete a dashboard. Cards/tabs cascade via FK; polymorphic bookmark rows do not, so purge them in the same transaction.
    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        let mut tx = self.db.pool.begin().await?;
        // bookmarks are polymorphic (no FK on entity_id) so they don't cascade — purge manually.
        sqlx::query(
            "DELETE FROM analytics.bookmarks WHERE entity_type='dashboard' AND entity_id=$1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM analytics.dashboards WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_folder(&self, id: Uuid, folder_id: Option<Uuid>) -> Result<(), BiError> {
        sqlx::query("UPDATE analytics.dashboards SET folder_id=$2 WHERE id=$1")
            .bind(id)
            .bind(folder_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn add_bookmark(
        &self,
        user: Uuid,
        entity_type: &str,
        entity_id: Uuid,
    ) -> Result<(), BiError> {
        sqlx::query(
            "INSERT INTO analytics.bookmarks (user_id, entity_type, entity_id) VALUES ($1,$2,$3) \
             ON CONFLICT (user_id, entity_type, entity_id) DO NOTHING",
        )
        .bind(user)
        .bind(entity_type)
        .bind(entity_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_bookmark(
        &self,
        user: Uuid,
        entity_type: &str,
        entity_id: Uuid,
    ) -> Result<(), BiError> {
        sqlx::query(
            "DELETE FROM analytics.bookmarks WHERE user_id=$1 AND entity_type=$2 AND entity_id=$3",
        )
        .bind(user)
        .bind(entity_type)
        .bind(entity_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn is_bookmarked(&self, user: Uuid, id: Uuid) -> Result<bool, BiError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM analytics.bookmarks WHERE user_id=$1 AND entity_type='dashboard' AND entity_id=$2)",
        )
        .bind(user)
        .bind(id)
        .fetch_one(&self.db.pool)
        .await?)
    }

    /// Like `list_by_workspace` but also returns a per-user `bookmarked` flag.
    pub async fn list_by_workspace_for(
        &self,
        workspace_id: Uuid,
        user: Uuid,
        trashed: bool,
    ) -> Result<Vec<(Dashboard, i64, bool)>, BiError> {
        let pred = if trashed {
            "deleted_at IS NOT NULL"
        } else {
            "deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM analytics.dashboard_cards c WHERE c.dashboard_id = dashboards.id) AS card_count, \
             EXISTS (SELECT 1 FROM analytics.bookmarks b WHERE b.user_id = $2 AND b.entity_type='dashboard' AND b.entity_id = dashboards.id) AS bookmarked \
             FROM analytics.dashboards WHERE workspace_id = $1 AND {pred} ORDER BY updated_at DESC"
        );
        let rows = sqlx::query_as::<_, DashboardWithCountBm>(sqlx::AssertSqlSafe(sql))
            .bind(workspace_id)
            .bind(user)
            .fetch_all(&self.db.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.dash, r.card_count, r.bookmarked))
            .collect())
    }

    /// All cards on a dashboard, in placement order.
    pub async fn cards(&self, dashboard_id: Uuid) -> Result<Vec<DashboardCard>, BiError> {
        let sql = format!(
            "SELECT {CARD_COLS} FROM analytics.dashboard_cards WHERE dashboard_id = $1 ORDER BY ordinal, grid_row, grid_col"
        );
        Ok(sqlx::query_as::<_, DashboardCard>(sqlx::AssertSqlSafe(sql))
            .bind(dashboard_id)
            .fetch_all(&self.db.pool)
            .await?)
    }

    /// All tabs on a dashboard, ordered by position.
    pub async fn tabs(&self, dashboard_id: Uuid) -> Result<Vec<DashboardTab>, BiError> {
        Ok(sqlx::query_as::<_, DashboardTab>(
            "SELECT id, name, position FROM analytics.dashboard_tabs WHERE dashboard_id = $1 ORDER BY position, id",
        )
        .bind(dashboard_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Atomically reconcile the dashboard's full tab + card set to the supplied one (in supplied
    /// order). Cards reference their tab by `tab_index` into `tabs`; `None` means no tab.
    ///
    /// RECONCILE, NOT DELETE-AND-REINSERT. It used to delete every tab and card and insert the
    /// payload fresh, which meant **every card and tab got a new id on every save** — Metabase
    /// keeps its dashcard ids, and so does this now. The churn was not theoretical: a save left
    /// the client holding ids that no longer existed, and the five cards of one ordinary dashboard
    /// each fetched their data into a 404 (fixed downstream in the cache by #474, but this is
    /// where it came from). Anything that stores a card id — a bookmark, a link, a future
    /// per-card setting — was invalidated by an ordinary save.
    ///
    /// A row is kept when the client sends an id THIS dashboard already owns; everything else in
    /// the payload is inserted, and everything not in the payload is deleted. The whole thing is
    /// one transaction, so a partial reconcile cannot be observed.
    pub async fn replace_layout(
        &self,
        dashboard_id: Uuid,
        tabs: &[NewTab],
        cards: &[NewCard],
    ) -> Result<(), BiError> {
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

        // WHICH IDS THIS DASHBOARD ACTUALLY OWNS. Every id the client sent is checked against
        // these sets and against nothing else, which is what makes an id a hint rather than an
        // instruction: an id belonging to ANOTHER dashboard is not in here, so it falls through to
        // the insert branch and mints a fresh row. No statement below ever addresses a row by id
        // alone — `dashboard_id` is in every WHERE clause as a second, independent guard.
        let existing_tabs: std::collections::HashSet<Uuid> = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM analytics.dashboard_tabs WHERE dashboard_id = $1",
        )
        .bind(dashboard_id)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        let existing_cards: std::collections::HashSet<Uuid> = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM analytics.dashboard_cards WHERE dashboard_id = $1",
        )
        .bind(dashboard_id)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();

        // `kept` doubles as the delete filter and, for tabs, as the index→id table the cards
        // resolve `tab_index` through. `claimed` stops a payload that names one id twice from
        // updating the same row twice and then deleting nothing — the duplicate becomes a new row.
        let mut kept_tabs: Vec<Uuid> = Vec::with_capacity(tabs.len());
        let mut claimed_tabs: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        for t in tabs {
            let reuse =
                t.id.filter(|id| existing_tabs.contains(id) && claimed_tabs.insert(*id));
            let id = match reuse {
                Some(id) => {
                    sqlx::query(
                        "UPDATE analytics.dashboard_tabs SET name = $3, position = $4 \
                         WHERE dashboard_id = $1 AND id = $2",
                    )
                    .bind(dashboard_id).bind(id).bind(&t.name).bind(t.position)
                    .execute(&mut *tx).await?;
                    id
                }
                None => sqlx::query_scalar(
                    "INSERT INTO analytics.dashboard_tabs (dashboard_id, name, position) VALUES ($1,$2,$3) RETURNING id",
                )
                .bind(dashboard_id).bind(&t.name).bind(t.position)
                .fetch_one(&mut *tx).await?,
            };
            kept_tabs.push(id);
        }

        let mut kept_cards: Vec<Uuid> = Vec::with_capacity(cards.len());
        let mut claimed_cards: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        for (i, c) in cards.iter().enumerate() {
            let tab_id = c.tab_index.map(|idx| kept_tabs[idx]);
            let reuse =
                c.id.filter(|id| existing_cards.contains(id) && claimed_cards.insert(*id));
            let id = match reuse {
                Some(id) => {
                    sqlx::query(
                        "UPDATE analytics.dashboard_cards SET chart_id = $3, card_type = $4, \
                         grid_col = $5, grid_row = $6, size_x = $7, size_y = $8, options = $9, \
                         ordinal = $10, tab_id = $11 WHERE dashboard_id = $1 AND id = $2",
                    )
                    .bind(dashboard_id).bind(id).bind(c.chart_id).bind(&c.card_type)
                    .bind(c.grid_col).bind(c.grid_row).bind(c.size_x).bind(c.size_y)
                    .bind(&c.options).bind(i as i32).bind(tab_id)
                    .execute(&mut *tx).await?;
                    id
                }
                None => sqlx::query_scalar(
                    "INSERT INTO analytics.dashboard_cards \
                     (dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal, tab_id) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id",
                )
                .bind(dashboard_id).bind(c.chart_id).bind(&c.card_type)
                .bind(c.grid_col).bind(c.grid_row).bind(c.size_x).bind(c.size_y)
                .bind(&c.options).bind(i as i32).bind(tab_id)
                .fetch_one(&mut *tx).await?,
            };
            kept_cards.push(id);
        }

        // CARDS BEFORE TABS. `dashboard_cards.tab_id` is `ON DELETE CASCADE` (migration 0017), so
        // dropping a tab takes its cards with it; deleting the stale cards first means the cascade
        // has nothing left to reach that this save wanted to keep. Nothing kept ever points at a
        // dropped tab anyway — `tab_id` is resolved out of `kept_tabs` — but the order costs
        // nothing and removes the question.
        sqlx::query(
            "DELETE FROM analytics.dashboard_cards WHERE dashboard_id = $1 AND id <> ALL($2)",
        )
        .bind(dashboard_id)
        .bind(&kept_cards)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM analytics.dashboard_tabs WHERE dashboard_id = $1 AND id <> ALL($2)",
        )
        .bind(dashboard_id)
        .bind(&kept_tabs)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// The workspace a dashboard belongs to (for the nested-ownership guard). None if absent.
    pub async fn dashboard_workspace(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(sqlx::query_scalar::<_, Uuid>(
            "SELECT workspace_id FROM analytics.dashboards WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// The owner (created_by) of a dashboard, for owner-or-admin checks.
    pub async fn owner(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT created_by FROM analytics.dashboards WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .flatten())
    }

    /// Display names for a dashboard's creator and last editor, resolved in ONE query.
    ///
    /// `COALESCE(NULLIF(u.name, ''), u.email::text)` is the label rule the principal picker
    /// already uses: the profile name when it is set, the email otherwise. It is resolved here
    /// rather than in the browser because the only endpoints that can turn a user id into a name
    /// are admin-gated — /api/control-plane/users needs `user:read` and /ws/{ws}/members needs
    /// `workspace:manage_members` — so a ws_editor opening the info sidebar on their own dashboard
    /// would get a 403 from both. Five other DTOs in this repo (MemberDto.principal_name and
    /// friends) resolve names the same way.
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

    /// Deep-copy a dashboard (row + tabs + cards) under a new owner, in one transaction.
    /// The copy's name is "<name> (copy)"; tab references on cards are remapped to the copy's tabs.
    /// Returns None if the source doesn't exist.
    pub async fn duplicate(
        &self,
        src_id: Uuid,
        workspace_id: Uuid,
        new_owner: Option<Uuid>,
    ) -> Result<Option<Dashboard>, BiError> {
        let Some(src) = self.get(src_id, workspace_id).await? else {
            return Ok(None);
        };
        let src_tabs = self.tabs(src_id).await?;
        let src_cards = self.cards(src_id).await?;

        let mut tx = self.db.pool.begin().await?;
        let copy: Dashboard = sqlx::query_as::<_, Dashboard>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO analytics.dashboards (workspace_id, name, description, parameters, created_by, width) \
             VALUES ($1,$2,$3,$4,$5,$6) RETURNING {COLS}"
        )))
        .bind(src.workspace_id)
        .bind(format!("{} (copy)", src.name))
        .bind(&src.description)
        .bind(&src.parameters)
        .bind(new_owner)
        .bind(&src.width)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_conflict_err)?;

        let mut tab_map: std::collections::HashMap<Uuid, Uuid> = std::collections::HashMap::new();
        for t in &src_tabs {
            let nid: Uuid = sqlx::query_scalar(
                "INSERT INTO analytics.dashboard_tabs (dashboard_id, name, position) VALUES ($1,$2,$3) RETURNING id",
            )
            .bind(copy.id).bind(&t.name).bind(t.position)
            .fetch_one(&mut *tx).await?;
            tab_map.insert(t.id, nid);
        }
        for c in &src_cards {
            let new_tab = c.tab_id.and_then(|old| tab_map.get(&old).copied());
            sqlx::query(
                "INSERT INTO analytics.dashboard_cards \
                 (dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal, tab_id) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(copy.id).bind(c.chart_id).bind(&c.card_type)
            .bind(c.grid_col).bind(c.grid_row).bind(c.size_x).bind(c.size_y)
            .bind(&c.options).bind(c.ordinal).bind(new_tab)
            .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(Some(copy))
    }

    /// The dashboard's CURRENT state, shaped exactly like a PUT body so it can be fed straight
    /// back through the update path. `None` if the dashboard is gone.
    ///
    /// IT CARRIES IDS, and used to not. The old contract — stated here, in `db_revision_revert`
    /// and at length in migration 0107 — was that a snapshot is id-free, on the grounds that
    /// nothing can then collide with the ids currently in the tables. That reasoning was sound
    /// only because `replace_layout` deleted and reinserted everything on every save, so ids were
    /// worthless. They are not any more: a save keeps them, and a revert that dropped them would
    /// be the one remaining operation that silently renumbered a dashboard.
    ///
    /// Sending the ids back makes a revert reconcile the same way a save does — a card that still
    /// exists is updated in place and keeps its id; a card deleted since is inserted fresh, since
    /// its id is no longer one this dashboard owns and `replace_layout` refuses it. The collision
    /// the old design worried about is exactly what that ownership check already handles.
    ///
    /// TAB IDS TOO, and `tab_index` STAYS. Cards still reference their tab by index, because the
    /// index also survives a tab being replaced; the tab's own id rides alongside so a tab that
    /// still exists is not recreated, which would cascade its cards away.
    ///
    /// Migration 0107's comment still describes the id-free design. It is left alone deliberately:
    /// editing an applied migration changes its checksum and sqlx then refuses to run. Read it as
    /// history — this doc comment is the current contract.
    pub async fn snapshot_current(
        &self,
        id: Uuid,
        workspace_id: Uuid,
    ) -> Result<Option<serde_json::Value>, BiError> {
        let Some(d) = self.get(id, workspace_id).await? else {
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
                .map(|(i, t)| serde_json::json!({ "id": t.id, "name": t.name, "position": i as i32 }))
                .collect::<Vec<_>>(),
            "cards": cards.iter().map(|c| serde_json::json!({
                "id": c.id,
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
        let rev = sqlx::query_as::<_, DashboardRevision>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO analytics.dashboard_revisions \
             (dashboard_id, created_by, kind, snapshot, created_at) \
             VALUES ($1,$2,$3,$4, COALESCE($5, now())) RETURNING {REV_COLS}"
        )))
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
        Ok(
            sqlx::query_as::<_, DashboardRevision>(sqlx::AssertSqlSafe(sql))
                .bind(dashboard_id)
                .bind(limit.clamp(1, MAX_REVISIONS_PER_DASHBOARD))
                .fetch_all(&self.db.pool)
                .await?,
        )
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
        Ok(
            sqlx::query_as::<_, DashboardRevision>(sqlx::AssertSqlSafe(sql))
                .bind(dashboard_id)
                .bind(revision_id)
                .fetch_optional(&self.db.pool)
                .await?,
        )
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
}
