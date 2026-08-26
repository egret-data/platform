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
    // JSON Schema's numeric keywords take a NUMBER. The author's bound is
    // stored as text (so the runtime comparison never goes through a float),
    // and it is parsed here for the document only. `check_constraints`
    // accepted it at publish time, so a parse failure here means a snapshot
    // frozen before that check existed — omit the keyword rather than emit a
    // lie about what the endpoint accepts.
    if let Some(v) = p
        .minimum
        .as_deref()
        .and_then(|s| s.parse::<serde_json::Number>().ok())
    {
        schema["minimum"] = Value::Number(v);
    }
    if let Some(v) = p
        .maximum
        .as_deref()
        .and_then(|s| s.parse::<serde_json::Number>().ok())
    {
        schema["maximum"] = Value::Number(v);
    }
    if let Some(v) = p.min_length {
        schema["minLength"] = json!(v);
    }
    if let Some(v) = p.max_length {
        schema["maxLength"] = json!(v);
    }
    if let Some(pattern) = &p.pattern {
        // The AUTHOR's pattern, not the anchored form `compile_pattern`
        // builds. The anchors are an implementation detail of how egret
        // matches; a client validating locally against `^(?:…)$` would be
        // validating a pattern its author never wrote.
        schema["pattern"] = json!(pattern);
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
    // JSON Schema has no date-range keyword, so a Date/Timestamp parameter's
    // `not_before`/`not_after` have nowhere to go in `param_schema`. Dropping
    // them would make the document describe a wider endpoint than exists,
    // which is the one thing a generated contract must never do.
    let mut bounds = Vec::new();
    if let Some(lo) = &p.not_before {
        bounds.push(format!("not before {lo}"));
    }
    if let Some(hi) = &p.not_after {
        bounds.push(format!("not after {hi}"));
    }
    if !bounds.is_empty() {
        d.push_str(&format!("Accepted range: {}.\n\n", bounds.join(", ")));
    }
    d.push_str(
        "Repeating this parameter supplies a list (`?x=a&x=b`), one bound value per \
         occurrence. `+` in a value decodes to a space — send a literal `+` as `%2B`.",
    );
    d
}

/// §4.5's promise, and the whole of it.
///
/// N statements are N independent reads (§1.1): `router.rs`'s loop calls
/// `execute_as_with_params` once per statement, on its own connection, and
/// nothing pins a snapshot across them. So a `total` statement and the `rows`
/// statement it counts can disagree, and the ONLY place a caller can learn that
/// is this sentence. It is deliberately not a caveat about egret being slow — it
/// is a statement about what the endpoint does not promise.
///
/// Written in plain words rather than as a link to a spec a consumer cannot
/// read, and appended to the AUTHOR's description rather than replacing it.
const NO_TRANSACTION_NO_SNAPSHOT: &str =
    "This endpoint runs its SQL statements as independent reads, one after another. They \
     share no transaction and no snapshot, so a total and the rows it counts may disagree \
     if the underlying table changes between them.";

/// The operation `description`: what the author wrote, plus §4.5's sentence once
/// there is more than one statement.
///
/// One statement gets nothing appended. A single read has nothing to disclaim,
/// and telling every pre-M2 consumer their one query is inconsistent with itself
/// would make the sentence noise — which is how a disclosure stops being read.
fn operation_description(authored: &str, snap: &EndpointSnapshot) -> String {
    if snap.statements.len() < 2 {
        return authored.to_string();
    }
    let mut d = authored.trim().to_string();
    if !d.is_empty() {
        d.push_str("\n\n");
    }
    d.push_str(NO_TRANSACTION_NO_SNAPSHOT);
    d
}

/// One ROW of one statement's result set.
///
/// `output.shape` decides the container: `objects` gives one property per
/// column, `arrays` gives a positional tuple. Property names go through
/// `apply_naming` — the SAME function serving calls — because the document must
/// name the keys a consumer will actually receive.
///
/// Per STATEMENT, never per endpoint: `output.shape` and `output.naming` are one
/// setting for the endpoint, but the COLUMNS they are applied to belong to one
/// statement, and two statements have no reason to share either a column list or
/// an order.
fn row_schema(
    stmt: &egret_dataapi_api::StatementSnapshot,
    snap: &EndpointSnapshot,
    endpoint: &str,
) -> Result<Value, OpenApiGenError> {
    let frozen = &stmt.columns;
    if snap.output.shape == "arrays" {
        // A positional row: OpenAPI 3.0 has no tuple type, so this is an array
        // of mixed scalars and the column order is documented in the
        // description rather than pretended into the schema.
        //
        // The order is a fact about THIS statement and is stable: publish
        // refuses `arrays` over a definition whose branches resolve to different
        // column lists (`lifecycle.rs`'s `BranchingArraysShape`), and it applies
        // that rule per statement.
        return Ok(json!({
            "type": "array",
            "description": format!(
                "Positional row. Column order: {}",
                frozen.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
            ),
            "items": {}
        }));
    }
    let mut props = Map::new();
    for c in frozen {
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
    let required: Vec<String> = frozen
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

/// One statement's whole result set: an array of its rows.
fn statement_schema(
    stmt: &egret_dataapi_api::StatementSnapshot,
    snap: &EndpointSnapshot,
    endpoint: &str,
) -> Result<Value, OpenApiGenError> {
    Ok(json!({ "type": "array", "items": row_schema(stmt, snap, endpoint)? }))
}

/// What `data` holds, for THIS endpoint (spec §4.4).
///
/// One statement: the rows array. Several: an object with one property per
/// statement NAME, each that statement's own rows array — the same keys, in the
/// same order, that `statements_data` writes.
///
/// **Not a union.** The document is generated per endpoint and an endpoint's
/// statement list is frozen in its snapshot, so a `oneOf` covering both shapes
/// would ask every consumer to branch on a question their own endpoint has
/// already answered — and openapi-generator would hand them an untagged
/// either-type to unwrap on every call.
fn data_schema(snap: &EndpointSnapshot, endpoint: &str) -> Result<Value, OpenApiGenError> {
    if snap.statements.is_empty() {
        return Err(OpenApiGenError::Core(egret_core::Error::Internal(
            anyhow::anyhow!("published snapshot for endpoint {endpoint} carries no statements"),
        )));
    }
    if snap.statements.len() == 1 {
        return statement_schema(&snap.statements[0], snap, endpoint);
    }
    let mut props = Map::new();
    for stmt in &snap.statements {
        props.insert(stmt.name.clone(), statement_schema(stmt, snap, endpoint)?);
    }
    Ok(json!({
        "type": "object",
        // Every statement answers or the call is refused: `router.rs`'s loop
        // returns on the first refusal and discards the result sets that already
        // succeeded, so there is no 200 carrying a subset of these keys.
        "required": snap.statements.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        "properties": props,
    }))
}

/// What `meta` holds, for THIS endpoint (spec §4.4).
///
/// Per-endpoint for the same reason `data` is: one statement reports the
/// endpoint-wide `row_count`/`columns`, several report `statements` keyed by
/// name and carry NEITHER of those two — `MetaContract` is an untagged enum
/// precisely so that a response cannot carry both readings at once.
///
/// `request_id` is documented although spec §4.4's field list omits it. It is
/// emitted on every response, it predates §4.4, and it is the only thing tying
/// the body a caller holds to the `tracing` line, the `dataapi.access_log` row
/// and the engine's query id — "quote your request_id" stops being actionable
/// without it. Documenting the served body is the point of this module; the
/// omission is a spec correction, not a field to hide.
fn meta_schema(snap: &EndpointSnapshot) -> Value {
    let mut required = vec!["version", "cached", "request_id"];
    let mut props = json!({
        "version": {
            "type": "integer",
            "description": "The published version that served this call.",
        },
        "cached": { "type": "boolean" },
        "request_id": {
            "type": "string",
            "description": "Correlation id, also written to this call's access-log row.",
        },
    });
    let obj = props.as_object_mut().expect("just built");
    if snap.statements.len() < 2 {
        required.push("row_count");
        required.push("columns");
        obj.insert("row_count".into(), json!({ "type": "integer" }));
        obj.insert(
            "columns".into(),
            json!({
                "type": "array",
                "description": "The FROZEN contract from the published snapshot, not the \
                                shape this execution happened to return.",
                "items": { "$ref": "#/components/schemas/ServingColumn" },
            }),
        );
    } else {
        required.push("statements");
        let mut per = Map::new();
        for stmt in &snap.statements {
            per.insert(
                stmt.name.clone(),
                json!({ "$ref": "#/components/schemas/StatementMeta" }),
            );
        }
        obj.insert(
            "statements".into(),
            json!({
                "type": "object",
                "description": "One entry per statement, under the same names, in the \
                                same order, as `data`.",
                "required": snap.statements.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                "properties": per,
            }),
        );
    }
    json!({ "type": "object", "required": required, "properties": props })
}

/// The whole 200 body for THIS endpoint — envelope included, or not.
///
/// `output.envelope: "bare"` writes `data`'s VALUE alone (`Served::into_response`),
/// so the document for such an endpoint is that value's schema and nothing
/// around it. An endpoint documented as wrapped that answers bare is the same
/// class of defect this function exists to fix, and `bare` is reachable through
/// the API today.
///
/// The envelope is read through `envelope_of` — the function SERVING calls — so
/// an unrecognised value is documented as wrapped because it is served wrapped.
/// A local `== "bare"` here would agree with serving today and drift silently
/// the moment that fallback changed.
fn success_schema(snap: &EndpointSnapshot, endpoint: &str) -> Result<Value, OpenApiGenError> {
    let data = data_schema(snap, endpoint)?;
    if matches!(
        crate::serving::router::envelope_of(&snap.output),
        crate::serving::router::Envelope::Bare
    ) {
        return Ok(data);
    }
    Ok(json!({
        "type": "object",
        // `message` is required AND nullable: the key is written on every
        // success and its value there is always `null`. Declaring it merely
        // optional would let a client that omits it believe it conformed.
        "required": ["status", "message", "data", "meta"],
        "properties": {
            "status": {
                "type": "string",
                // One value, not two: this schema describes a 200, and `status`
                // MIRRORS the HTTP status (`ServingStatus::mirroring`). `error`
                // belongs to the refusal bodies, which reference
                // `ServingErrorBody`.
                "enum": ["ok"],
            },
            "message": {
                "type": "string",
                "nullable": true,
                "description": "Always null on success. A string here is the human half \
                                of a refusal — read `error.code`.",
            },
            "data": data,
            "meta": meta_schema(snap),
        }
    }))
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

        let bare = matches!(
            crate::serving::router::envelope_of(&snap.output),
            crate::serving::router::Envelope::Bare
        );
        let mut responses = Map::new();
        responses.insert(
            "200".into(),
            json!({
                "description": if bare {
                    "Rows. This endpoint is published with `output.envelope: \"bare\"`, so the \
                     body is the data value alone — no `status`, `message` or `meta` around it. \
                     A refusal is still returned in the standard error envelope, under its real \
                     HTTP status."
                } else {
                    "Rows, in the standard serving envelope."
                },
                "content": { "application/json": {
                    "schema": success_schema(&snap, &name)?
                }}
            }),
        );
        if let Value::Object(errs) = error_responses() {
            responses.extend(errs);
        }

        let op = json!({
            "operationId": format!("dataApi_{}", id.simple()),
            "summary": name,
            // The author's words, plus §4.5's sentence once there is more than
            // one statement — see `operation_description`.
            "description": operation_description(&description, &snap),
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
                // The refusal body as `Wire::into_response` writes it —
                // `{status, message, error:{code, message, request_id}}`. It
                // was documented FLAT here (`{code, message, request_id}`,
                // i.e. the nested detail hoisted to the top level) until M2
                // Task 9, which is a body egret has never produced.
                "ServingErrorBody": {
                    "type": "object",
                    "required": ["status", "message", "error"],
                    "properties": {
                        "status": { "type": "string", "enum": ["error"] },
                        "message": {
                            "type": "string",
                            "description": "The same human-readable string as `error.message`, \
                                            lifted to where the success envelope carries it, so \
                                            `status` and `message` are readable on every serving \
                                            response without knowing which body arrived."
                        },
                        "error": { "$ref": "#/components/schemas/ServingErrorDetail" }
                    }
                },
                "ServingErrorDetail": {
                    "type": "object",
                    "required": ["code", "message", "request_id"],
                    "properties": {
                        "code": {
                            "type": "string",
                            "description": "The stable code to branch on. `message` is for \
                                            humans and may be reworded.",
                            "example": "row_limit_exceeded"
                        },
                        "message": { "type": "string" },
                        "request_id": { "type": "string" }
                    }
                },
                // One frozen output column, as `meta` reports it.
                "ServingColumn": {
                    "type": "object",
                    "required": ["name", "ty", "required"],
                    "properties": {
                        "name": { "type": "string" },
                        "ty": {
                            "type": "string",
                            "description": "The engine's own type name, unmapped."
                        },
                        "required": {
                            "type": "boolean",
                            "description": "Whether every described branch of the definition \
                                            returns this column."
                        }
                    }
                },
                // Referenced only by the endpoints that have several statements;
                // a document containing none simply does not point at it.
                "StatementMeta": {
                    "type": "object",
                    "required": ["row_count", "columns"],
                    "properties": {
                        "row_count": { "type": "integer" },
                        "columns": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/ServingColumn" }
                        }
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
    use egret_dataapi_api::{ColumnSpec, ParamSpec};

    fn spec(name: &str, ty: ParamType) -> ParamSpec {
        ParamSpec {
            name: name.into(),
            ty,
            location: ParamLocation::Query,
            required: false,
            default: None,
            allowed: None,
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            not_before: None,
            not_after: None,
            description: String::new(),
        }
    }

    fn number_param(name: &str) -> ParamSpec {
        spec(name, ParamType::Number)
    }

    fn string_param(name: &str) -> ParamSpec {
        spec(name, ParamType::String)
    }

    fn date_param(name: &str) -> ParamSpec {
        spec(name, ParamType::Date)
    }

    /// A numeric bound lands in the schema as a JSON NUMBER, both keywords.
    #[test]
    fn a_numeric_bound_reaches_the_schema() {
        let mut p = number_param("n");
        p.minimum = Some("1".into());
        p.maximum = Some("100".into());
        let s = param_schema(&p);
        assert_eq!(s["minimum"], json!(1), "{s}");
        assert_eq!(s["maximum"], json!(100), "{s}");
    }

    /// JSON Schema's `minimum` is a NUMBER, not a string. The author's text is
    /// parsed for the document even though it is compared as text at runtime.
    #[test]
    fn a_decimal_bound_stays_a_number_in_the_schema() {
        let mut p = number_param("n");
        p.minimum = Some("0.5".into());
        let s = param_schema(&p);
        assert!(
            s["minimum"].is_number(),
            "minimum must be a JSON number: {s}"
        );
    }

    #[test]
    fn string_constraints_reach_the_schema() {
        let mut p = string_param("s");
        p.min_length = Some(2);
        p.max_length = Some(10);
        p.pattern = Some("^[a-z]+$".into());
        let s = param_schema(&p);
        assert_eq!(s["minLength"], json!(2), "{s}");
        assert_eq!(s["maxLength"], json!(10), "{s}");
        assert_eq!(s["pattern"], json!("^[a-z]+$"), "{s}");
    }

    /// The anchors `compile_pattern` adds are ours, not the author's.
    /// Publishing `"^(?:^[a-z]+$)$"` would be confusing and would not
    /// round-trip if a client used it to validate locally.
    #[test]
    fn the_schema_publishes_the_authors_pattern_not_the_anchored_one() {
        let mut p = string_param("s");
        p.pattern = Some("[a-z]+".into());
        let s = param_schema(&p);
        assert_eq!(s["pattern"], json!("[a-z]+"), "{s}");
    }

    /// NOT the schema: JSON Schema has no date-range keyword, and the caller
    /// already emits `param_description(p)` beside `param_schema(p)`. A
    /// second description inside the schema would sit where no reader looks.
    #[test]
    fn temporal_bounds_reach_the_parameter_description() {
        let mut p = date_param("d");
        p.not_before = Some("2020-01-01".into());
        p.not_after = Some("2030-12-31".into());
        let d = param_description(&p);
        assert!(d.contains("2020-01-01"), "{d}");
        assert!(d.contains("2030-12-31"), "{d}");
        assert!(
            param_schema(&p).get("description").is_none(),
            "the bounds must not also be duplicated into the schema"
        );
    }

    #[test]
    fn an_unconstrained_parameter_gains_no_keys() {
        let p = string_param("s");
        let s = param_schema(&p);
        for k in ["minimum", "maximum", "minLength", "maxLength", "pattern"] {
            assert!(
                s.get(k).is_none(),
                "unconstrained parameter grew `{k}`: {s}"
            );
        }
    }

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

    fn stmt(name: &str, columns: Vec<ColumnSpec>) -> egret_dataapi_api::StatementSnapshot {
        egret_dataapi_api::StatementSnapshot {
            name: name.into(),
            sql: "SELECT 1".into(),
            columns,
        }
    }

    fn snap_with_columns(columns: Vec<ColumnSpec>, naming: &str) -> EndpointSnapshot {
        snap_with_statements(vec![stmt("main", columns)], json!({ "naming": naming }))
    }

    fn snap_with_statements(
        statements: Vec<egret_dataapi_api::StatementSnapshot>,
        output: Value,
    ) -> EndpointSnapshot {
        EndpointSnapshot {
            statements,
            method: "GET".into(),
            path: "/x".into(),
            database_id: Uuid::nil(),
            params: vec![],
            output: serde_json::from_value(output).unwrap(),
            cache: serde_json::from_value(json!({})).unwrap(),
            limits: egret_dataapi_api::LimitsSpec {
                max_rows: 1,
                timeout_seconds: 1,
            },
            rate_limit: serde_json::from_value(json!({})).unwrap(),
            delegated: false,
            impersonate: false,
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
        let row = &doc["items"];
        assert_eq!(row["required"], json!(["id"]));
        assert!(
            row["properties"]["name"].is_object(),
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
            doc["items"].get("required").is_none(),
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
        let row = &doc["items"];
        assert_eq!(row["required"], json!(["totalRevenue"]));
        assert!(row["properties"]["totalRevenue"].is_object());
        assert!(
            row["properties"].get("total_revenue").is_none(),
            "the raw name must not leak into properties either: {doc}"
        );
    }

    /// **The M2 shape.** With more than one statement `data` is an OBJECT with
    /// one property per statement NAME, each an array of that statement's own
    /// row schema — the same keys serving writes (`statements_data`).
    ///
    /// The per-statement columns are asserted separately from the keys because
    /// they fail separately: a generator that keys correctly and hands every
    /// statement the FIRST one's columns passes a key-only assertion, and that
    /// is exactly the bridge this task deletes.
    #[test]
    fn several_statements_document_data_as_an_object_keyed_by_statement_name() {
        let snap = snap_with_statements(
            vec![
                stmt("rows", vec![col("id", "integer", true)]),
                stmt("total", vec![col("total", "bigint", true)]),
            ],
            json!({}),
        );
        let doc = data_schema(&snap, "e").expect("document");
        assert_eq!(doc["type"], "object", "{doc}");
        assert_eq!(
            doc["properties"]
                .as_object()
                .expect("properties")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["rows".to_string(), "total".to_string()],
            "one property per statement name, in the published order: {doc}"
        );
        assert_eq!(doc["properties"]["rows"]["type"], "array", "{doc}");
        assert_eq!(
            doc["properties"]["rows"]["items"]["properties"]
                .as_object()
                .expect("row properties")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["id".to_string()],
            "the rows statement's OWN columns: {doc}"
        );
        assert_eq!(
            doc["properties"]["total"]["items"]["properties"]
                .as_object()
                .expect("row properties")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["total".to_string()],
            "the total statement's own column, not the rows statement's: {doc}"
        );
        // Every statement answers, or the whole call is refused — there is no
        // 200 carrying a subset of them (`router.rs`'s loop returns on the
        // first refusal), so every key is `required`.
        assert_eq!(doc["required"], json!(["rows", "total"]), "{doc}");
    }

    /// **Neither shape is a union type.** The document is generated per
    /// endpoint and an endpoint's statement count is frozen, so a `oneOf`
    /// covering both would make every consumer branch on a question its own
    /// endpoint already answered.
    #[test]
    fn neither_shape_is_a_union_type() {
        for snap in [
            snap_with_columns(vec![col("id", "integer", true)], "as_is"),
            snap_with_statements(
                vec![
                    stmt("rows", vec![col("id", "integer", true)]),
                    stmt("total", vec![col("total", "bigint", true)]),
                ],
                json!({}),
            ),
        ] {
            let doc = success_schema(&snap, "e").expect("document");
            let rendered = doc.to_string();
            for combinator in ["oneOf", "anyOf", "allOf"] {
                assert!(
                    !rendered.contains(combinator),
                    "the per-endpoint 200 schema must not need {combinator}: {rendered}"
                );
            }
        }
    }

    /// Each statement keeps its OWN positional order under `output.shape:
    /// "arrays"`. There is one order per statement, never one for the endpoint:
    /// `arrays` over a branching definition is refused per statement at publish
    /// (`lifecycle.rs`'s `BranchingArraysShape`), so each frozen list has an
    /// order — but two statements' lists have no reason to agree.
    #[test]
    fn each_statement_documents_its_own_column_order_under_the_arrays_shape() {
        let snap = snap_with_statements(
            vec![
                stmt(
                    "rows",
                    vec![col("id", "integer", true), col("name", "varchar", true)],
                ),
                stmt("total", vec![col("total", "bigint", true)]),
            ],
            json!({ "shape": "arrays" }),
        );
        let doc = data_schema(&snap, "e").expect("document");
        assert_eq!(doc["type"], "object", "{doc}");
        assert_eq!(
            doc["properties"]["rows"]["items"]["description"],
            "Positional row. Column order: id, name",
            "{doc}"
        );
        assert_eq!(
            doc["properties"]["total"]["items"]["description"],
            "Positional row. Column order: total",
            "{doc}"
        );
    }

    /// A naming collision in the SECOND statement refuses the document too.
    ///
    /// Serving answers `output_naming_collision` (503) for such a version
    /// whichever statement carries it, and the fold at publish runs per
    /// statement for the same reason. A generator that checked only the first
    /// would emit a document for an endpoint that never returns 200.
    #[test]
    fn a_naming_collision_in_the_second_statement_refuses_the_document() {
        let snap = snap_with_statements(
            vec![
                stmt("rows", vec![col("id", "integer", true)]),
                stmt(
                    "total",
                    vec![
                        col("total_revenue", "bigint", true),
                        col("totalRevenue", "bigint", true),
                    ],
                ),
            ],
            json!({ "naming": "camel" }),
        );
        assert!(
            matches!(
                data_schema(&snap, "e"),
                Err(OpenApiGenError::NamingCollision { .. })
            ),
            "a collision anywhere in the list refuses the document"
        );
    }

    /// The wrapped 200 body is the one Task 8 emits: `status`, `message`,
    /// `data`, `meta`, all four present on every success.
    ///
    /// `message` is `required` AND `nullable` — that pair is the accurate
    /// statement, because the key is always written and its value on a success
    /// is always `null`. Declaring it merely optional would let a client that
    /// omits it think it was conforming.
    #[test]
    fn the_wrapped_body_carries_status_message_data_and_meta() {
        let snap = snap_with_columns(vec![col("id", "integer", true)], "as_is");
        let doc = success_schema(&snap, "e").expect("document");
        assert_eq!(
            doc["required"],
            json!(["status", "message", "data", "meta"]),
            "{doc}"
        );
        assert_eq!(doc["properties"]["status"]["enum"], json!(["ok"]), "{doc}");
        assert_eq!(doc["properties"]["message"]["nullable"], true, "{doc}");
        assert_eq!(doc["properties"]["data"]["type"], "array", "{doc}");
        let meta = &doc["properties"]["meta"];
        assert_eq!(
            meta["required"],
            json!(["version", "cached", "request_id", "row_count", "columns"]),
            "one statement reports the endpoint-wide contract: {doc}"
        );
        assert!(
            meta["properties"].get("statements").is_none(),
            "`statements` is absent from a one-statement meta: {doc}"
        );
    }

    /// `meta` is per-endpoint too. With several statements it carries
    /// `statements`, keyed by the same names as `data`, and NOT the
    /// endpoint-wide `row_count`/`columns` — which Task 8 stopped emitting.
    #[test]
    fn several_statements_document_meta_keyed_the_same_way_as_data() {
        let snap = snap_with_statements(
            vec![
                stmt("rows", vec![col("id", "integer", true)]),
                stmt("total", vec![col("total", "bigint", true)]),
            ],
            json!({}),
        );
        let doc = success_schema(&snap, "e").expect("document");
        let meta = &doc["properties"]["meta"];
        assert_eq!(
            meta["required"],
            json!(["version", "cached", "request_id", "statements"]),
            "{doc}"
        );
        assert!(
            meta["properties"].get("row_count").is_none()
                && meta["properties"].get("columns").is_none(),
            "the endpoint-wide count and columns are absent for several: {doc}"
        );
        assert_eq!(
            meta["properties"]["statements"]["properties"]
                .as_object()
                .expect("statements properties")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            doc["properties"]["data"]["properties"]
                .as_object()
                .expect("data properties")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            "`meta.statements` and `data` are keyed the same, in the same order: {doc}"
        );
    }

    /// `output.envelope: "bare"` writes `data`'s VALUE alone, so the document
    /// for a bare endpoint is that value's schema and nothing around it. An
    /// endpoint documented as wrapped that answers bare is the defect this task
    /// exists to close, and it is reachable through the API today.
    #[test]
    fn a_bare_endpoint_is_documented_without_the_envelope() {
        let snap = snap_with_columns(vec![col("id", "integer", true)], "as_is");
        let bare = snap_with_statements(
            vec![stmt("main", vec![col("id", "integer", true)])],
            json!({ "envelope": "bare" }),
        );
        let wrapped_doc = success_schema(&snap, "e").expect("document");
        let bare_doc = success_schema(&bare, "e").expect("document");
        assert_eq!(bare_doc["type"], "array", "{bare_doc}");
        assert_eq!(
            bare_doc["items"], wrapped_doc["properties"]["data"]["items"],
            "bare is the SAME data schema, unwrapped: {bare_doc}"
        );
        assert!(
            bare_doc["properties"].is_null(),
            "no `status`, `message` or `meta` around it: {bare_doc}"
        );
    }

    /// An unrecognised `output.envelope` is documented as WRAPPED, because
    /// `envelope_of` — the function serving itself calls — falls back to
    /// wrapped. Reading `== "bare"` here instead would agree with serving today
    /// and drift the moment that fallback changed.
    #[test]
    fn an_unrecognised_envelope_is_documented_as_wrapped() {
        let snap = snap_with_statements(
            vec![stmt("main", vec![col("id", "integer", true)])],
            json!({ "envelope": "sideways" }),
        );
        let doc = success_schema(&snap, "e").expect("document");
        assert_eq!(
            doc["required"],
            json!(["status", "message", "data", "meta"]),
            "the documented default is the served default: {doc}"
        );
    }

    /// §4.5's sentence, on the operation, once there is more than one
    /// statement — and NOT before. It is the whole of the consistency promise:
    /// a caller has no other way to learn that a total and the rows it counts
    /// are two independent reads.
    #[test]
    fn the_consistency_sentence_appears_only_with_several_statements() {
        let one = snap_with_columns(vec![col("id", "integer", true)], "as_is");
        assert!(
            operation_description("An author's words.", &one).trim() == "An author's words.",
            "one statement has nothing to disclaim"
        );
        let several = snap_with_statements(
            vec![
                stmt("rows", vec![col("id", "integer", true)]),
                stmt("total", vec![col("total", "bigint", true)]),
            ],
            json!({}),
        );
        let d = operation_description("An author's words.", &several);
        assert!(
            d.starts_with("An author's words."),
            "the author's description is kept: {d}"
        );
        assert!(
            d.contains("no transaction") && d.contains("no snapshot") && d.contains("may disagree"),
            "the plain sentence §4.5 requires: {d}"
        );
    }
}
