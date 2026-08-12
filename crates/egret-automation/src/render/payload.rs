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

use crate::error::AutomationError;
use crate::render::token::RenderGrant;
use crate::routes::AutomationState;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RenderCard {
    pub chart_id: String,
    pub title: String,
    pub viz_type: String,
    pub params: serde_json::Value,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RenderPayload {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub cards: Vec<RenderCard>,
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
        .get(grant.object_id)
        .await
        .map_err(bi_err)?
        .ok_or_else(|| AutomationError::NotFound("chart".into()))?;
    let card = render_chart_card(st, grant, &chart).await?;
    Ok(RenderPayload {
        kind: "chart".into(),
        id: chart.id.to_string(),
        title: chart.name.clone(),
        cards: vec![card],
    })
}

async fn build_dashboard(
    st: &AutomationState,
    grant: &RenderGrant,
) -> Result<RenderPayload, AutomationError> {
    let dash = st
        .dashboards
        .get(grant.object_id)
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
            .get(chart_id)
            .await
            .map_err(bi_err)?
            .ok_or_else(|| AutomationError::NotFound("chart".into()))?;
        cards.push(render_chart_card(st, grant, &chart).await?);
    }

    Ok(RenderPayload {
        kind: "dashboard".into(),
        id: dash.id.to_string(),
        title: dash.name.clone(),
        cards,
    })
}

/// Compile the chart's SAVED spec exactly the way `charts/{id}/data` does
/// (`egret_bi::compile_chart_sql`), then execute it as the token's OWNER
/// (`owner_identity`) — never as a service identity.
async fn render_chart_card(
    st: &AutomationState,
    grant: &RenderGrant,
    chart: &egret_bi::Chart,
) -> Result<RenderCard, AutomationError> {
    let spec: egret_bi::chartquery::QuerySpec =
        serde_json::from_value(chart.params.get("spec").cloned().ok_or_else(|| {
            AutomationError::BadRequest(format!("chart {} has no runnable query", chart.id))
        })?)
        .map_err(|_| AutomationError::BadRequest(format!("chart {} spec invalid", chart.id)))?;

    let (sql, database_id) = egret_bi::compile_chart_sql(
        &st.datasets,
        &st.databases,
        chart.workspace_id,
        chart.dataset_id,
        &spec,
    )
    .await
    .map_err(bi_api_err)?;

    let identity = crate::evaluator::owner_identity(
        grant.owner_id,
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

    Ok(RenderCard {
        chart_id: chart.id.to_string(),
        title: chart.name.clone(),
        viz_type: chart.viz_type.clone(),
        params: chart.params.clone(),
        columns: result.columns.into_iter().map(|c| c.name).collect(),
        rows: result.rows,
    })
}

fn bi_err(e: egret_bi::BiError) -> AutomationError {
    match e {
        egret_bi::BiError::NotFound(s) => AutomationError::NotFound(s),
        egret_bi::BiError::BadRequest(s) => AutomationError::BadRequest(s),
        egret_bi::BiError::Conflict(s) => AutomationError::Conflict(s),
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
