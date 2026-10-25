//! Everything the print page needs, in ONE response.
//!
//! The page makes exactly one authenticated call, so single-use is literal
//! rather than aspirational. That forces this module to assemble server-side
//! what the interactive UI fetches piecemeal: the object's spec, and each
//! card's already-executed rows.
//!
//! Every query runs as the token's OWNER, through the SAME execution seam the
//! tick engine uses (§5.2) — `crate::evaluator::QueryRunner`, injected on
//! `AutomationState` as `query` — a report must never render data its owner
//! could not open themselves, and this module must be testable without a live
//! engine exactly the way `AutomationRunner`/`tick_end_to_end.rs` already are.
//! And every chart is compiled through `egret_bi`'s own `compile_chart_sql` —
//! the exact function `charts/{id}/data` calls — so there is exactly one place
//! a chart's saved spec turns into SQL. Two code paths that compile a chart
//! differently is how a report silently stops matching the dashboard it
//! claims to show.

use serde::Serialize;
use uuid::Uuid;

use crate::error::AutomationError;
use crate::render::token::RenderGrant;
use crate::routes::AutomationState;
use egret_core::tabular::ColumnMeta;

/// One card's IDENTITY and query shape — no rows.
///
/// Step one of the render. The page needs the saved spec to resolve the
/// dashboard's parameters against it, which it cannot do if the server has
/// already run the query.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RenderCard {
    pub chart_id: String,
    pub title: String,
    pub viz_type: String,
    pub params: serde_json::Value,
    /// `dashboard_cards.options.paramMappings`: parameter id -> the column on
    /// THIS card it filters. A parameter with no mapping here does not apply to
    /// this card, which is why the mapping travels per card rather than once
    /// for the dashboard.
    pub param_mappings: serde_json::Value,
    /// The chart's dataset's semantic layer: each column's and metric's
    /// display name and format. The print page layers it under the chart's own
    /// settings with the SAME functions the dashboard and the chart page use
    /// (`datasetColumnDefaults` / `withDatasetDefaults`), so a report says
    /// **Revenue** where they do rather than `revenue` (KI-301). Raw rows, not
    /// a server-built format map: building that map here would be a second
    /// implementation of those rules. Empty when the dataset is not in the
    /// grant's workspace.
    pub dataset: RenderDataset,
}

/// What the print page needs of a card's dataset to name and format its
/// columns as the dashboard does — nothing about its source or SQL.
#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
pub struct RenderDataset {
    pub columns: Vec<RenderDatasetColumn>,
    pub metrics: Vec<RenderDatasetMetric>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RenderDatasetColumn {
    pub column_name: String,
    pub verbose_name: String,
    pub format: String,
    pub semantic_type: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RenderDatasetMetric {
    pub metric_name: String,
    pub verbose_name: String,
    pub format: String,
}

/// One card's DATA. Step two, once the page has resolved the filters.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RenderCardData {
    pub chart_id: String,
    // The engine's own name + type per column, not names alone — the print
    // page (`PrintObject.tsx`) needs the SAME type a live chart gets from
    // `ChartRenderer`'s callers to format a date axis, a numeric column that
    // happens to hold a numeric STRING, or a column whose first sampled row
    // is null. Guessing from cell values (which an earlier revision of this
    // page did, client-side) is exactly the silent-drift risk this module's
    // own header comment warns about for SQL compilation; the same argument
    // applies to formatting.
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

/// What the page asks for in step two: the resolved dashboard-parameter
/// filters for one card.
///
/// The same shape as `egret_bi`'s `CardRunBody`, and merged the same way —
/// onto the card's SAVED spec, never replacing it. A client supplies FILTERS,
/// never a query: each `col` is checked against the dataset's real columns by
/// `compile_chart_sql` -> `chartquery::build` -> `find_column`, and each
/// temporal unit against the `TemporalUnit` enum. Accepting a whole spec here
/// would let whoever holds a render token run arbitrary SQL as the schedule's
/// owner.
#[derive(Debug, Clone, Default, serde::Deserialize, utoipa::ToSchema)]
pub struct RenderCardRequest {
    pub chart_id: Uuid,
    #[serde(default)]
    pub filters: Vec<egret_bi::chartquery::Filter>,
    #[serde(default)]
    pub temporal_units: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RenderPayload {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub cards: Vec<RenderCard>,
    /// The dashboard's parameter definitions, verbatim. The print page resolves
    /// them to filters with `applyParamsToChart` — the SAME module the
    /// interactive dashboard uses — and sends the result back to
    /// [`run_cards`].
    ///
    /// Ported to Rust instead, this would be a second implementation of six
    /// text operators, relative date windows and month/quarter/year arithmetic
    /// that must agree with the first one exactly, forever. When they drifted,
    /// the emailed image would quietly stop matching the dashboard — which is
    /// the bug this whole path exists to fix.
    ///
    /// `null` for a chart report: a chart has no parameters.
    pub parameters: serde_json::Value,
}

/// Assemble the full payload for whatever the grant names.
pub async fn build(
    st: &AutomationState,
    grant: &RenderGrant,
) -> Result<RenderPayload, AutomationError> {
    match grant.kind.as_str() {
        "chart" => build_chart(st, grant).await,
        "dashboard" => build_dashboard(st, grant).await,
        // Unreachable while migration 0112's CHECK constraint holds; if it
        // ever fires the row is corrupt rather than the request being wrong.
        other => Err(AutomationError::Internal(format!(
            "render token names an unknown object kind: {other}"
        ))),
    }
}

async fn build_chart(
    st: &AutomationState,
    grant: &RenderGrant,
) -> Result<RenderPayload, AutomationError> {
    let chart = st
        .charts
        .get(grant.object_id, grant.workspace_id)
        .await
        .map_err(bi_err)?
        .ok_or_else(|| AutomationError::NotFound("chart".into()))?;
    Ok(RenderPayload {
        kind: "chart".into(),
        id: chart.id.to_string(),
        title: chart.name.clone(),
        cards: vec![card_spec(st, grant, &chart, serde_json::Value::Null).await?],
        // A chart is not on a dashboard, so nothing parameterises it.
        parameters: serde_json::Value::Null,
    })
}

async fn build_dashboard(
    st: &AutomationState,
    grant: &RenderGrant,
) -> Result<RenderPayload, AutomationError> {
    let dash = st
        .dashboards
        .get(grant.object_id, grant.workspace_id)
        .await
        .map_err(bi_err)?
        .ok_or_else(|| AutomationError::NotFound("dashboard".into()))?;
    let dash_cards = st.dashboards.cards(dash.id).await.map_err(bi_err)?;

    // Only cards backed by a chart produce rows — a text/markdown card has no
    // `chart_id` and nothing to execute.
    let mut cards = Vec::with_capacity(dash_cards.len());
    for dc in &dash_cards {
        let Some(chart_id) = dc.chart_id else {
            continue;
        };
        let chart = st
            .charts
            .get(chart_id, grant.workspace_id)
            .await
            .map_err(bi_err)?
            .ok_or_else(|| AutomationError::NotFound("chart".into()))?;
        let mappings = dc
            .options
            .get("paramMappings")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        cards.push(card_spec(st, grant, &chart, mappings).await?);
    }

    Ok(RenderPayload {
        kind: "dashboard".into(),
        id: dash.id.to_string(),
        title: dash.name.clone(),
        cards,
        parameters: dash.parameters.clone(),
    })
}

async fn card_spec(
    st: &AutomationState,
    grant: &RenderGrant,
    chart: &egret_bi::Chart,
    param_mappings: serde_json::Value,
) -> Result<RenderCard, AutomationError> {
    Ok(RenderCard {
        chart_id: chart.id.to_string(),
        title: chart.name.clone(),
        viz_type: chart.viz_type.clone(),
        params: chart.params.clone(),
        param_mappings,
        dataset: card_dataset(st, grant, chart.dataset_id).await?,
    })
}

/// The card's dataset's display names and formats. Read as the render's
/// owner would see them on the dashboard: the report already runs this
/// dataset's queries as that owner, so its column names are no more than the
/// report shows. Scoped to the grant's workspace, the same guard
/// `compile_chart_sql` applies before running anything.
async fn card_dataset(
    st: &AutomationState,
    grant: &RenderGrant,
    dataset_id: Uuid,
) -> Result<RenderDataset, AutomationError> {
    let Some(ds) = st.datasets.get(dataset_id).await.map_err(bi_err)? else {
        return Ok(RenderDataset::default());
    };
    if ds.workspace_id != grant.workspace_id {
        return Ok(RenderDataset::default());
    }
    let columns = st.datasets.columns(ds.id).await.map_err(bi_err)?;
    let metrics = st.datasets.metrics(ds.id).await.map_err(bi_err)?;
    Ok(RenderDataset {
        columns: columns
            .into_iter()
            .map(|c| RenderDatasetColumn {
                column_name: c.column_name,
                verbose_name: c.verbose_name,
                format: c.format,
                semantic_type: c.semantic_type,
            })
            .collect(),
        metrics: metrics
            .into_iter()
            .map(|m| RenderDatasetMetric {
                metric_name: m.metric_name,
                verbose_name: m.verbose_name,
                format: m.format,
            })
            .collect(),
    })
}

/// Step two: run every card the page asks for, with the filters it resolved.
///
/// One call for the whole page rather than one per card, so the token is spent
/// exactly once and the render still cannot become a data tap.
pub async fn run_cards(
    st: &AutomationState,
    grant: &RenderGrant,
    reqs: &[RenderCardRequest],
) -> Result<Vec<RenderCardData>, AutomationError> {
    // Which charts this grant actually covers. Without it, a token for
    // dashboard A would run any chart id the caller cared to name — the token
    // authorises ONE object, and that has to be enforced here rather than
    // assumed from the payload the page was handed.
    let allowed = allowed_charts(st, grant).await?;

    let mut out = Vec::with_capacity(reqs.len());
    for req in reqs {
        if !allowed.contains(&req.chart_id) {
            return Err(AutomationError::BadRequest(format!(
                "chart {} is not part of this render",
                req.chart_id
            )));
        }
        let chart = st
            .charts
            .get(req.chart_id, grant.workspace_id)
            .await
            .map_err(bi_err)?
            .ok_or_else(|| AutomationError::NotFound("chart".into()))?;
        out.push(run_card(st, grant, &chart, req).await?);
    }
    Ok(out)
}

async fn allowed_charts(
    st: &AutomationState,
    grant: &RenderGrant,
) -> Result<std::collections::HashSet<Uuid>, AutomationError> {
    match grant.kind.as_str() {
        "chart" => Ok(std::iter::once(grant.object_id).collect()),
        "dashboard" => {
            // Scoped explicitly, even though every id this returns is about
            // to be re-checked by `charts.get(id, grant.workspace_id)` in
            // `run_cards` — so a foreign dashboard already 404s here today
            // via that second check. This first check is what keeps that
            // true if `run_cards` is ever refactored to trust `allowed`
            // without re-fetching: without it, `dashboards.cards(object_id)`
            // reads by id alone and would enumerate a foreign dashboard's
            // cards with no workspace predicate at all.
            //
            // The redundancy cuts both ways — do not "clean up" EITHER
            // fetch as the duplicate of the other. This one is what defends
            // `run_cards`' per-chart `get` from ever being simplified away;
            // that one is what defends THIS fetch from the same fate. Removing
            // either leaves the hole this comment describes with no compiler
            // error and no test failure, because today each backstops the
            // other on every path this crate can currently drive.
            let dash = st
                .dashboards
                .get(grant.object_id, grant.workspace_id)
                .await
                .map_err(bi_err)?
                .ok_or_else(|| AutomationError::NotFound("dashboard".into()))?;
            Ok(st
                .dashboards
                .cards(dash.id)
                .await
                .map_err(bi_err)?
                .iter()
                .filter_map(|c| c.chart_id)
                .collect())
        }
        other => Err(AutomationError::Internal(format!(
            "render token names an unknown object kind: {other}"
        ))),
    }
}

/// Compile the chart's SAVED spec exactly the way `charts/{id}/data` does
/// (`egret_bi::compile_chart_sql`), then execute it as the token's OWNER
/// (`owner_identity`) — never as a service identity.
async fn run_card(
    st: &AutomationState,
    grant: &RenderGrant,
    chart: &egret_bi::Chart,
    req: &RenderCardRequest,
) -> Result<RenderCardData, AutomationError> {
    let mut spec: egret_bi::chartquery::QuerySpec =
        serde_json::from_value(chart.params.get("spec").cloned().ok_or_else(|| {
            AutomationError::BadRequest(format!("chart {} has no runnable query", chart.id))
        })?)
        .map_err(|_| AutomationError::BadRequest(format!("chart {} spec invalid", chart.id)))?;

    // EXTEND, never replace — the same merge `db_card_run` performs for the
    // interactive dashboard, so a report and the page it mirrors compile the
    // same query. An unknown column or temporal unit is rejected downstream by
    // `chartquery::build`.
    spec.filters.extend(req.filters.iter().cloned());
    if !req.temporal_units.is_empty() {
        for (col, unit) in &req.temporal_units {
            spec.temporal_units.insert(col.clone(), unit.clone());
        }
    }

    // The GRANT's own workspace, not the chart's — see `runner.rs`'s
    // `build_chart_report` for why the two must never be conflated: they are
    // the same value by construction once `build`/`allowed_charts` above have
    // already re-fetched the chart scoped to `grant.workspace_id`, but a
    // chart-sourced value here would leave `compile_chart_sql`'s own
    // `ds.workspace_id != workspace_id` guard comparing a value against
    // itself, permanently dead.
    let (sql, database_id) = egret_bi::compile_chart_sql(
        &st.datasets,
        &st.databases,
        grant.workspace_id,
        chart.dataset_id,
        &spec,
    )
    .await
    .map_err(bi_api_err)?;

    let identity = crate::evaluator::owner_identity(
        grant.owner_id,
        grant.workspace_id,
        database_id,
        &st.databases,
        &st.users,
        &*st.engine_minter,
    )
    .await
    .map_err(|e| AutomationError::Internal(e.to_string()))?;

    // Through `QueryRunner`, not a direct `run_query`/`DefaultConnectorFactory`
    // call: the same seam `AutomationRunner` injects a fake engine behind in
    // `tick_end_to_end.rs`, so this path is fakeable in tests too. The chart's
    // own saved `row_limit` goes through untouched — an alert's runner asks
    // for `Some(1)` because it only reads one cell; a report needs every row
    // the chart was built to show.
    let result = st
        .query
        .run(
            database_id,
            &sql,
            &identity,
            Some(spec.row_limit.max(0) as usize),
        )
        .await
        .map_err(|e| AutomationError::Internal(format!("query failed: {e}")))?;

    Ok(RenderCardData {
        chart_id: chart.id.to_string(),
        columns: result.columns,
        rows: result.rows,
    })
}

fn bi_err(e: egret_bi::BiError) -> AutomationError {
    match e {
        egret_bi::BiError::NotFound(s) => AutomationError::NotFound(s),
        egret_bi::BiError::BadRequest(s) => AutomationError::BadRequest(s),
        egret_bi::BiError::Conflict(s) => AutomationError::Conflict(s),
        // SP4d-4 Task 3: only a tag write answers this; a render never does.
        egret_bi::BiError::Overlap { body } => AutomationError::Conflict(
            body["error"]
                .as_str()
                .unwrap_or("two mask or row-filter rules overlap")
                .to_string(),
        ),
        egret_bi::BiError::Coded { status, body } => {
            let msg = body["error"].as_str().unwrap_or("refused").to_string();
            if status == axum::http::StatusCode::BAD_REQUEST {
                AutomationError::BadRequest(msg)
            } else {
                AutomationError::Conflict(msg)
            }
        }
        // Only a delete or a metric rename answers this; a render never does.
        egret_bi::BiError::InUse { message, .. } => AutomationError::Conflict(message),
        egret_bi::BiError::Forbidden => {
            AutomationError::Internal("bi repo unexpectedly refused a render read".into())
        }
        egret_bi::BiError::Engine(s) => AutomationError::Internal(s),
        egret_bi::BiError::Db(e) => AutomationError::Db(e),
    }
}

fn bi_api_err(e: egret_bi::BiApiError) -> AutomationError {
    match e {
        egret_bi::BiApiError::Auth(err) => match err {
            egret_core::Error::NotFound(s) => AutomationError::NotFound(s),
            egret_core::Error::InvalidInput(s) => AutomationError::BadRequest(s),
            egret_core::Error::Conflict(s) => AutomationError::Conflict(s),
            other => AutomationError::Internal(other.to_string()),
        },
        egret_bi::BiApiError::Domain(d) => bi_err(d),
    }
}
