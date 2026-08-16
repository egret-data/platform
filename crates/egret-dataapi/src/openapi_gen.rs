//! Data API P5 §5 — the OpenAPI document a consumer is handed.
//!
//! Assembled as `serde_json::Value` rather than through utoipa, because utoipa
//! describes THIS server's compiled routes and these paths exist only in a
//! user's data.
//!
//! OpenAPI **3.0.3**, not 3.1: this artifact's entire purpose is being consumed
//! by someone else's tooling, and Postman and openapi-generator support 3.0 far
//! more widely.

use crate::routes::DataApiState;
use egret_core::ids::UserId;
use egret_core::WorkspaceId;
use egret_dataapi_api::{EndpointSnapshot, ParamLocation, ParamType};
use serde_json::{json, Map, Value};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum OpenApiGenError {
    /// The endpoint's `output.naming` maps two columns onto one key. Serving
    /// answers `output_naming_collision` (503) for such a version, so a document
    /// describing it would describe something that never returns 200 — and a
    /// document quietly missing a column is worse than no document.
    #[error("endpoint {endpoint} has an output naming collision; re-publish it")]
    NamingCollision { endpoint: String },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Core(#[from] egret_core::Error),
}

/// Map a raw engine type string onto a JSON Schema type, conservatively.
///
/// `ColumnSpec.ty` is whatever the engine called it, and Trino and StarRocks do
/// not agree. Anything unrecognised becomes `string` and keeps its original
/// spelling in `x-egret-engine-type`: a confident wrong mapping sends a client
/// to parse a number that is not one, while `string` is merely imprecise.
fn json_type(engine_ty: &str) -> &'static str {
    let t = engine_ty.trim().to_ascii_lowercase();
    let head = t.split(['(', '<']).next().unwrap_or("").trim();
    match head {
        "boolean" | "bool" => "boolean",
        "tinyint" | "smallint" | "int" | "integer" | "bigint" | "largeint" => "integer",
        "real" | "float" | "double" | "decimal" | "numeric" => "number",
        _ => "string",
    }
}

fn param_schema(p: &egret_dataapi_api::ParamSpec) -> Value {
    let mut schema = match p.ty {
        ParamType::Number => json!({ "type": "number" }),
        ParamType::Boolean => json!({ "type": "boolean" }),
        ParamType::Date => json!({ "type": "string", "format": "date" }),
        ParamType::Timestamp => json!({ "type": "string", "format": "date-time" }),
        ParamType::String | ParamType::Enum => json!({ "type": "string" }),
    };
    if matches!(p.ty, ParamType::Enum) {
        if let Some(allowed) = &p.allowed {
            schema["enum"] = json!(allowed);
        }
    }
    schema
}

/// The two behaviours a caller cannot see in the schema and will otherwise meet
/// as wrong data rather than as an error.
///
/// A repeated query parameter is a LIST, one bind per value — but `ParamSpec`
/// carries no multiplicity, so the generator genuinely cannot tell which
/// parameters accept repetition. Declaring `oneOf [scalar, array]` to cover it
/// would break most generators; saying it in prose does not.
///
/// And `+` decodes to a SPACE (measured 2026-08-16), so `?token=a+b` binds
/// `"a b"` and returns 200 with rows for a value nobody typed.
fn param_description(p: &egret_dataapi_api::ParamSpec) -> String {
    let mut d = p.description.clone();
    if !d.is_empty() {
        d.push_str("\n\n");
    }
    d.push_str(
        "Repeating this parameter supplies a list (`?x=a&x=b`), one bound value per \
         occurrence. `+` in a value decodes to a space — send a literal `+` as `%2B`.",
    );
    d
}

/// The response schema for one published version.
///
/// `output.shape` decides the container: `objects` gives one property per
/// column, `arrays` gives a positional tuple. Property names go through
/// `apply_naming` — the SAME function serving calls — because the document must
/// name the keys a consumer will actually receive.
fn data_schema(snap: &EndpointSnapshot, endpoint: &str) -> Result<Value, OpenApiGenError> {
    if snap.output.shape == "arrays" {
        // A positional row: OpenAPI 3.0 has no tuple type, so this is an array
        // of mixed scalars and the column order is documented in the
        // description rather than pretended into the schema.
        return Ok(json!({
            "type": "array",
            "description": format!(
                "Positional row. Column order: {}",
                snap.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
            ),
            "items": {}
        }));
    }
    let mut props = Map::new();
    for c in &snap.columns {
        let key = crate::serving::router::apply_naming(&c.name, &snap.output.naming);
        // Serving answers `output_naming_collision` (503) for this version, so
        // there is no document to write: emitting the surviving column would
        // describe a response that never comes, and quietly dropping one is
        // worse than refusing.
        if props.contains_key(&key) {
            return Err(OpenApiGenError::NamingCollision {
                endpoint: endpoint.to_string(),
            });
        }
        props.insert(
            key,
            json!({
                "type": json_type(&c.ty),
                "nullable": true,
                "x-egret-engine-type": c.ty,
            }),
        );
    }
    Ok(json!({ "type": "object", "properties": props }))
}

/// The error responses, identical for every path, referenced not inlined.
fn error_responses() -> Value {
    let err = json!({
        "description": "Error",
        "content": { "application/json": {
            "schema": { "$ref": "#/components/schemas/ServingErrorBody" }
        }}
    });
    json!({
        "400": err, "401": err, "403": err,
        "404": err, "429": err, "500": err, "503": err,
    })
}

/// Build the document for one service account.
///
/// `only` narrows to a single endpoint — the per-endpoint download is this same
/// builder, not a second mechanism that could describe an endpoint differently
/// from the aggregate.
pub async fn build(
    st: &DataApiState,
    ws: WorkspaceId,
    service_account: UserId,
    only: Option<Uuid>,
) -> Result<Value, OpenApiGenError> {
    // `state = 'live'` AND a live version: a draft has no published contract,
    // and a disabled endpoint answers 404, so documenting either describes
    // something that does not happen.
    let rows: Vec<(Uuid, String, String, String, String, Value, i32)> = sqlx::query_as(
        "SELECT e.id, e.name, e.description, e.path, e.method, ev.snapshot, ev.version \
         FROM dataapi.endpoints e \
         JOIN dataapi.endpoint_versions ev ON ev.id = e.live_version_id \
         WHERE e.workspace_id = $1 AND e.state = 'live' \
           AND ($2::uuid IS NULL OR e.id = $2) \
         ORDER BY e.path",
    )
    .bind(ws.0)
    .bind(only)
    .fetch_all(&st.db.pool)
    .await?;

    let base = st.issuer.trim_end_matches('/');
    let mut paths = Map::new();

    for (id, name, description, path, method, snapshot, version) in rows {
        // THE line this whole module turns on. A scan of `access_grants` would
        // answer "no" for any principal reaching invoke through the ws_admin
        // cascade and silently omit those paths.
        if !crate::invoke::can_invoke(st, service_account, ws, id).await? {
            continue;
        }
        let snap: EndpointSnapshot = serde_json::from_value(snapshot)
            .map_err(|e| OpenApiGenError::Db(sqlx::Error::Decode(Box::new(e))))?;

        let params: Vec<Value> = snap
            .params
            .iter()
            // `location: body` is refused at publish and `serve_post` has no
            // body extractor, so nothing can supply one. Skipped rather than
            // trusted, for snapshots frozen before that refusal existed.
            .filter(|p| matches!(p.location, ParamLocation::Query))
            .map(|p| {
                json!({
                    "name": p.name,
                    "in": "query",
                    "required": p.required,
                    "description": param_description(p),
                    "schema": param_schema(p),
                })
            })
            .collect();

        let mut responses = Map::new();
        responses.insert(
            "200".into(),
            json!({
                "description": "Rows",
                "content": { "application/json": { "schema": {
                    "type": "object",
                    "properties": {
                        "data": { "type": "array", "items": data_schema(&snap, &name)? },
                        "meta": { "$ref": "#/components/schemas/ServingMeta" }
                    }
                }}}
            }),
        );
        if let Value::Object(errs) = error_responses() {
            responses.extend(errs);
        }

        let op = json!({
            "operationId": format!("dataApi_{}", id.simple()),
            "summary": name,
            "description": description,
            "parameters": params,
            "responses": responses,
            // Which frozen contract this description belongs to. Without it a
            // consumer holding a stale file cannot tell that it is stale.
            "x-egret-version": version,
        });

        let verb = method.to_ascii_lowercase();
        paths
            .entry(path)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("path item is an object")
            .insert(verb, op);
    }

    Ok(json!({
        // 3.0.3, not 3.1 — see the module comment.
        "openapi": "3.0.3",
        "info": { "title": "Data API", "version": "1.0.0" },
        // Both halves of the origin come from `issuer`. See spec §5.3: if these
        // could disagree, one file would send a consumer to fetch a token from
        // one host and call another.
        "servers": [ { "url": format!("{base}/data-api/v1") } ],
        "security": [ { "egret_oauth2": [] } ],
        "components": {
            "securitySchemes": {
                "egret_oauth2": {
                    "type": "oauth2",
                    "description": "Machine-IdP client_credentials. The client id and \
                                    secret are the ones issued with the service account.",
                    // Same derivation `issue_client` used to build the `token_endpoint`
                    // it handed the consumer when their client was issued
                    // (`routes.rs`'s `issue_client`) — `/oauth/token`, not the
                    // `/oauth2/token` a naive read of the machine-IdP name would guess.
                    // If this drifted from that route, one document would send a
                    // consumer to fetch a token from a URL that does not exist.
                    "flows": { "clientCredentials": {
                        "tokenUrl": format!("{base}/oauth/token"),
                        "scopes": {}
                    }}
                }
            },
            "schemas": {
                "ServingErrorBody": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "string" },
                        "message": { "type": "string" },
                        "request_id": { "type": "string" }
                    }
                },
                "ServingMeta": {
                    "type": "object",
                    "properties": {
                        "columns": { "type": "array", "items": { "type": "object" } },
                        "row_count": { "type": "integer" },
                        "version": { "type": "integer" },
                        "cached": { "type": "boolean" },
                        "request_id": { "type": "string" }
                    }
                }
            }
        },
        "paths": paths,
    }))
}
