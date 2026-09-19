//! Saved SQL that names a catalog (spec §4.8.2). Charts hold no SQL — they are
//! reached through their dataset. Matching is textual: `<name>.` or `"<name>".`
//! not preceded by an identifier character, case-insensitive.

use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
struct Hit {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedSqlHit {
    pub object_type: &'static str,
    pub object_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
}

pub async fn scan_saved_sql(
    pool: &sqlx::PgPool,
    catalog_name: &str,
) -> Result<Vec<SavedSqlHit>, sqlx::Error> {
    let pattern = format!(r#"(^|[^a-z0-9_"])"?{}"?\s*\."#, regex::escape(catalog_name));
    let mut out = Vec::new();
    let mut push = |object_type: &'static str, hits: Vec<Hit>| {
        out.extend(hits.into_iter().map(|h| SavedSqlHit {
            object_type,
            object_id: h.id,
            workspace_id: h.workspace_id,
            name: h.name,
        }));
    };
    push(
        "dataset",
        sqlx::query_as(
            "SELECT id, workspace_id, name FROM analytics.datasets \
              WHERE deleted_at IS NULL AND (sql ~* $1 OR lower(catalog) = lower($2)) ORDER BY workspace_id, name",
        )
        .bind(&pattern)
        .bind(catalog_name)
        .fetch_all(pool)
        .await?,
    );
    push(
        "saved_query",
        sqlx::query_as(
            "SELECT id, workspace_id, name FROM analytics.saved_queries WHERE sql ~* $1 ORDER BY workspace_id, name",
        )
        .bind(&pattern)
        .fetch_all(pool)
        .await?,
    );
    push(
        "sql_snippet",
        sqlx::query_as(
            "SELECT id, workspace_id, name FROM analytics.sql_snippets WHERE body ~* $1 ORDER BY workspace_id, name",
        )
        .bind(&pattern)
        .fetch_all(pool)
        .await?,
    );
    push(
        "alert_or_report",
        sqlx::query_as(
            "SELECT id, workspace_id, name FROM automation.schedules \
              WHERE deleted_at IS NULL AND sql ~* $1 ORDER BY workspace_id, name",
        )
        .bind(&pattern)
        .fetch_all(pool)
        .await?,
    );
    push(
        "data_api_endpoint_draft",
        sqlx::query_as(
            "SELECT e.id, e.workspace_id, e.name FROM dataapi.endpoints e \
              WHERE e.deleted_at IS NULL AND EXISTS ( \
                    SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(e.statements) = 'array' \
                                                            THEN e.statements ELSE '[]'::jsonb END) s \
                     WHERE s->>'sql' ~* $1) \
              ORDER BY e.workspace_id, e.name",
        )
        .bind(&pattern)
        .fetch_all(pool)
        .await?,
    );
    push(
        "data_api_endpoint_live",
        sqlx::query_as(
            "SELECT e.id, e.workspace_id, e.name FROM dataapi.endpoints e \
               JOIN dataapi.endpoint_versions v ON v.id = e.live_version_id \
              WHERE e.deleted_at IS NULL AND ((v.snapshot->>'sql') ~* $1 OR EXISTS ( \
                    SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(v.snapshot->'statements') = 'array' \
                                                            THEN v.snapshot->'statements' ELSE '[]'::jsonb END) s \
                     WHERE s->>'sql' ~* $1)) \
              ORDER BY e.workspace_id, e.name",
        )
        .bind(&pattern)
        .fetch_all(pool)
        .await?,
    );
    Ok(out)
}
