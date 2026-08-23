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
use egret_core::{Error, WorkspaceId};
use egret_dataapi_api::{EndpointSnapshot, ParamLocation, ParamType};
use serde_json::{json, Map, Value};
use uuid::Uuid;

/// The most live endpoints one generated document may describe.
///
/// **A bound, not a page.** Building the document asks the PDP whether the
/// service account may invoke each candidate ([`crate::invoke::can_invoke`]),
/// and a `CedarPdp::authorize` over an `Object` costs two DB round-trips before
/// it links templates and runs the authorizer (`egret-auth`'s
/// `pdp/mod.rs` — `grants_for`, plus the `tag_assignments` scan). That loop is
/// the cost of this route, and left unbounded a workspace can make one `:read`
/// GET arbitrarily expensive.
///
/// The question cannot be batched without changing what it answers.
/// `can_invoke` is the SAME expression the serving path evaluates before it runs
/// anything, and its answer must stay identical here or the document describes a
/// different access set than the one a consumer meets. So the count is capped
/// rather than the check weakened.
///
/// Exceeding it is an ERROR, never a truncated document: a document silently
/// missing paths is exactly the failure §5.2 spends a Cedar call to avoid, and a
/// consumer cannot tell a short document from a complete one. The caller's way
/// through is `?endpoint_id=`, which narrows the query to a single row.
pub const MAX_DOCUMENTED_ENDPOINTS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum OpenApiGenError {
    /// The endpoint's `output.naming` maps two columns onto one key. Serving
    /// answers `output_naming_collision` (503) for such a version, so a document
    /// describing it would describe something that never returns 200 — and a
    /// document quietly missing a column is worse than no document.
    #[error("endpoint {endpoint} has an output naming collision; re-publish it")]
    NamingCollision { endpoint: String },
    /// More live endpoints than [`MAX_DOCUMENTED_ENDPOINTS`]. See that constant.
    #[error(
        "this workspace has more than {max} live endpoints, more than one generated document \
         may describe; download them one at a time with ?endpoint_id="
    )]
    TooManyEndpoints { max: usize },
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
    // Every declared column is a property; only the ones present in every
    // described branch are `required`. This is what stops the document
    // advertising a call that 503s: before M1 a conditional endpoint declared
    // one branch's columns as if they were the whole contract.
    //
    // Names go through `apply_naming` — the same as `props` above — because
    // this `required` list must name the keys a consumer actually receives,
    // not the raw column name. Note this is OpenAPI's `required` (which
    // properties must be present), a different thing from `ColumnSpec::required`
    // (which branches guarantee a column) even though the latter feeds the
    // former here.
    let required: Vec<String> = snap
        .columns
        .iter()
        .filter(|c| c.required)
        .map(|c| crate::serving::router::apply_naming(&c.name, &snap.output.naming))
        .collect();
    let mut schema = json!({ "type": "object", "properties": props });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    Ok(schema)
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
/// from the aggregate. It is also the way past
/// [`MAX_DOCUMENTED_ENDPOINTS`], which bounds the aggregate form.
pub async fn build(
    st: &DataApiState,
    ws: WorkspaceId,
    service_account: UserId,
    only: Option<Uuid>,
) -> Result<Value, OpenApiGenError> {
    // `state = 'live'` AND a live version: a draft has no published contract,
    // and a disabled endpoint answers 404, so documenting either describes
    // something that does not happen.
    //
    // `LIMIT $3` fetches ONE more row than may be described, so an overflow is
    // detected from the count rather than from a document that came back short.
    // See [`MAX_DOCUMENTED_ENDPOINTS`].
    let rows: Vec<(Uuid, String, String, String, String, Value, i32)> = sqlx::query_as(
        "SELECT e.id, e.name, e.description, e.path, e.method, ev.snapshot, ev.version \
         FROM dataapi.endpoints e \
         JOIN dataapi.endpoint_versions ev ON ev.id = e.live_version_id \
         WHERE e.workspace_id = $1 AND e.state = 'live' \
           AND ($2::uuid IS NULL OR e.id = $2) \
         ORDER BY e.path \
         LIMIT $3",
    )
    .bind(ws.0)
    .bind(only)
    .bind(MAX_DOCUMENTED_ENDPOINTS as i64 + 1)
    .fetch_all(&st.db.pool)
    .await?;

    if rows.len() > MAX_DOCUMENTED_ENDPOINTS {
        // Logged as well as returned: the caller sees a 400 they can act on, and
        // the operator — who is the one who can decide whether the bound is
        // still the right number — sees which workspace hit it.
        tracing::warn!(
            workspace_id = %ws.0,
            max = MAX_DOCUMENTED_ENDPOINTS,
            "generated OpenAPI refused: more live endpoints than one document may describe"
        );
        return Err(OpenApiGenError::TooManyEndpoints {
            max: MAX_DOCUMENTED_ENDPOINTS,
        });
    }

    // The CONSUMER-facing origin (`data_api.public_url`), NOT `st.issuer`: this
    // document is read by a client outside the cluster, and the issuer is an
    // in-cluster Service DNS name it cannot resolve. Falls back to the issuer
    // when unset, so a deployment that sets nothing keeps today's document.
    let base = st.consumer_base_url.trim_end_matches('/');
    // The workspace SLUG, resolved exactly as `try_it` resolves it
    // (`try_it.rs`'s `try_endpoint`) — from `ws`, which the handler already
    // authorized against, never from a second read of the `{workspace}` path
    // segment.
    //
    // **Without it every documented operation 404s.** The serving ROUTE is
    // `/data-api/v1/{ws}/{*path}` (`serving::serving_router`), not the mount
    // alone: joining a slug-less `servers[0].url` to a `paths` key of `/reports/daily`
    // yields `…/data-api/v1/reports/daily`, which binds `ws = "reports"`, fails
    // to match the token's `ws` claim, and refuses the call. `tests/openapi_gen.rs`'s
    // `the_documented_url_is_the_url_try_it_calls` composes the two halves and
    // pins them to the URL `try_it` echoes for the same endpoint.
    let slug = st
        .workspaces
        .get_by_id(ws)
        .await?
        .ok_or_else(|| Error::NotFound("workspace".into()))?
        .slug;
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
        // Both halves of the origin come from `base` — the ONE consumer-facing
        // value. See spec §5.3: if these could disagree, one file would send a
        // consumer to fetch a token from one host and call another.
        //
        // The WORKSPACE SLUG is part of the base URL because it is part of the
        // serving route, not of the mount — see the `slug` binding above.
        "servers": [ { "url": format!("{base}/data-api/v1/{slug}") } ],
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

#[cfg(test)]
mod tests {
    use super::*;
    use egret_dataapi_api::ColumnSpec;

    /// `data_schema` is private to this module and `tests/openapi_gen.rs` only
    /// reaches the public, DB-gated `build` — so this pins the fragment
    /// directly, the same way `serving/router.rs`'s own test module drives
    /// `shape_rows` directly rather than through the router.
    fn col(name: &str, ty: &str, required: bool) -> ColumnSpec {
        ColumnSpec {
            name: name.into(),
            ty: ty.into(),
            required,
        }
    }

    fn snap_with_columns(columns: Vec<ColumnSpec>, naming: &str) -> EndpointSnapshot {
        EndpointSnapshot {
            sql: "SELECT 1".into(),
            method: "GET".into(),
            path: "/x".into(),
            database_id: Uuid::nil(),
            params: vec![],
            output: serde_json::from_value(json!({ "naming": naming })).unwrap(),
            cache: serde_json::from_value(json!({})).unwrap(),
            limits: egret_dataapi_api::LimitsSpec {
                max_rows: 1,
                timeout_seconds: 1,
            },
            rate_limit: serde_json::from_value(json!({})).unwrap(),
            delegated: false,
            impersonate: false,
            columns,
            param_gate: egret_dataapi_api::PARAM_GATE,
        }
    }

    /// The scenario the whole plan turns on: `id` is guaranteed by every
    /// branch, `name` only by some. Both are documented as properties — an
    /// optional column is still reachable — but only `id` may appear in
    /// `required`, or the document promises a key some branch never returns.
    #[test]
    fn only_the_columns_present_in_every_branch_are_required() {
        let snap = snap_with_columns(
            vec![col("id", "integer", true), col("name", "varchar", false)],
            "as_is",
        );
        let doc = data_schema(&snap, "e").expect("a branching endpoint has a document");
        assert_eq!(doc["required"], json!(["id"]));
        assert!(
            doc["properties"]["name"].is_object(),
            "an optional column is still documented — it is reachable, just not guaranteed"
        );
    }

    /// No column is guaranteed by every branch: the schema must not carry an
    /// empty `required: []`, which some generators read as "no properties are
    /// ever required" rather than "this key happens to be absent from the
    /// object" — the field is simply omitted.
    #[test]
    fn no_required_columns_omits_the_required_key_entirely() {
        let snap = snap_with_columns(vec![col("name", "varchar", false)], "as_is");
        let doc = data_schema(&snap, "e").expect("document");
        assert!(
            doc.get("required").is_none(),
            "an empty required array is a different claim than no required key: {doc}"
        );
    }

    /// The trap named in this task: `required` must name the key a consumer
    /// actually receives, run through the SAME `apply_naming` as `properties`
    /// — not the raw column name.
    #[test]
    fn required_names_are_camel_cased_the_same_as_properties() {
        let snap = snap_with_columns(vec![col("total_revenue", "integer", true)], "camel");
        let doc = data_schema(&snap, "e").expect("document");
        assert_eq!(doc["required"], json!(["totalRevenue"]));
        assert!(doc["properties"]["totalRevenue"].is_object());
        assert!(
            doc["properties"].get("total_revenue").is_none(),
            "the raw name must not leak into properties either: {doc}"
        );
    }
}
