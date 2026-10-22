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
pub(crate) fn json_type(engine_ty: &str) -> &'static str {
    let t = engine_ty.trim().to_ascii_lowercase();
    let head = t.split(['(', '<']).next().unwrap_or("").trim();
    match head {
        "boolean" | "bool" => "boolean",
        "tinyint" | "smallint" | "int" | "integer" | "bigint" | "largeint" => "integer",
        "real" | "float" | "double" | "decimal" | "numeric" => "number",
        _ => "string",
    }
}

/// Map a transform's DECLARED return type onto a JSON Schema type.
///
/// **Not [`json_type`], and the difference is measurable in the served body.**
/// `json_type` reads a string an ENGINE produced, where "anything unrecognised
/// is a string" is safe because every scalar an engine returns has a string
/// spelling. A transform's `ty` is not that: nothing checks it at runtime
/// (`transform::check_returns` tests object-ness, required-presence and
/// undeclared fields — never the type name), so the generated document is its
/// ONLY consumer, and the author writing it is naming a JSON type, not a
/// column type.
///
/// Measured in a browser on 2026-08-27: a script returning
/// `{"by_region": {"EU": "30.50"}, "regions": 3}` against `returns` declaring
/// `by_region: object` was served that object verbatim, while the generated
/// document declared `by_region` a `string` — a generated client would parse
/// the wrong thing, which is exactly the failure `json_type`'s "merely
/// imprecise" argument assumes cannot happen.
///
/// Anything this does not recognise still falls through to [`json_type`], so an
/// author who writes `varchar` in a transform's `returns` gets what they always
/// did.
pub(crate) fn declared_json_type(declared_ty: &str) -> &'static str {
    let t = declared_ty.trim().to_ascii_lowercase();
    let head = t.split(['(', '<']).next().unwrap_or("").trim();
    match head {
        "object" | "dict" | "map" | "struct" => "object",
        "array" | "list" => "array",
        _ => json_type(declared_ty),
    }
}

/// Parse an author's `minimum`/`maximum` text into a JSON-Schema-legal
/// number, for the document only.
///
/// **Why this exists rather than `s.parse::<serde_json::Number>()`
/// directly.** JSON's own number grammar forbids a leading zero before the
/// decimal point (anything but a bare `"0"`), but that is NOT the grammar
/// `check_constraints` enforces at publish, nor the one `coerce` enforces
/// at request time — both gate through [`crate::serving::params::is_plain_decimal`]
/// (`-?digits[.digits]`), which allows `"007"`, `"00"`, `"01.5"`. Those
/// bounds publish today and ARE enforced by `coerce`. Handing them straight
/// to `serde_json::Number::from_str` would make `minimum: "007"` silently
/// vanish from the document — the exact failure this task exists to close,
/// just one step removed: the document would describe a WIDER endpoint
/// than the one that actually enforces `007`.
///
/// So leading zeros are trimmed first. This changes nothing about what is
/// enforced: `compare_decimal_text` — the ONLY runtime comparison, at both
/// publish and `coerce` — already treats `"007"` and `"7"` as the same
/// magnitude (it trims leading zeros before comparing), so the trimmed
/// spelling is not a different number, only a different way of writing the
/// same one.
///
/// **What keeps this LOSSLESS is a cap at publish, not anything here.**
/// serde_json without `arbitrary_precision` puts any magnitude past `u64`
/// through an `f64`, which rounds — a 40-digit `minimum` reaches the document
/// as a visibly different, HIGHER number — and past `f64`'s range the keyword
/// cannot be represented at all. Neither is fixable in this function: it is
/// handed text and asked for a JSON number. So
/// [`crate::lifecycle::MAX_BOUND_DIGITS`] refuses such a bound at publish
/// instead, where the author can still change it. Every bound that reaches
/// here through `check_constraints` is therefore narrow enough to state
/// exactly, which `openapi_gen`'s
/// `every_bound_check_constraints_accepts_is_stated_faithfully` pins as a
/// seam rather than leaving to either side to assume.
///
/// Still returns `None` for text `is_plain_decimal` never accepted in the
/// first place, and for a magnitude `serde_json::Number` cannot represent —
/// both unreachable for a spec `check_constraints` has passed, but this
/// function is also reachable from a snapshot written before those checks
/// existed, and omitting the keyword beats emitting a lie.
fn json_number(text: &str) -> Option<serde_json::Number> {
    if !crate::serving::params::is_plain_decimal(text) {
        return None;
    }
    let (sign, body) = text.strip_prefix('-').map_or(("", text), |b| ("-", b));
    let (int, frac) = body
        .split_once('.')
        .map_or((body, None), |(i, f)| (i, Some(f)));
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let mut normalised = format!("{sign}{int}");
    if let Some(f) = frac {
        normalised.push('.');
        normalised.push_str(f);
    }
    normalised.parse::<serde_json::Number>().ok()
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
    // and `json_number` parses it here for the document only — normalising
    // a leading zero rather than treating it as unparseable, because
    // `is_plain_decimal` accepts one at publish and `coerce` enforces it. See
    // `json_number` for why that normalisation is truthful.
    if let Some(v) = p.minimum.as_deref().and_then(json_number) {
        schema["minimum"] = Value::Number(v);
    }
    if let Some(v) = p.maximum.as_deref().and_then(json_number) {
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

/// What `data` holds, for THIS endpoint (spec §4.4) when it has NO response
/// transform.
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
fn statements_data_schema(
    snap: &EndpointSnapshot,
    endpoint: &str,
) -> Result<Value, OpenApiGenError> {
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

/// The `data` object a transformed endpoint promises, straight from
/// `output.transform.returns` — never from the SQL statements' own columns.
///
/// `check_returns` (`crate::transform`, the SAME function serving enforces
/// every response against) requires the value to be a JSON OBJECT, so this is
/// never wrapped in an array the way `statement_schema` wraps a row: a
/// transform's `data` is one object, not a list of rows.
///
/// **`output.shape` and `output.naming` do not apply here, and the document
/// says so.** Those two settings describe how a row straight from SQL becomes
/// JSON (`row_schema`); a transform's output never goes through that step
/// (`serving::router::shaping_output` — a transformed endpoint is served
/// `objects`/`as_is` regardless of what is published), so running `returns`'
/// names through `apply_naming` a second time would rename a key the author
/// already chose. The `description` below is where a reader of the generated
/// contract meets that fact, because it is exactly the field `shape`/`naming`
/// would otherwise have described.
///
/// Duplicate names in `returns` cannot reach here, and neither can blank or
/// whitespace-padded ones: `check_transform_shape` (`lifecycle.rs`) refuses all
/// three at publish, before a transform can be declared at all — unlike
/// `row_schema`'s naming collision, which is a runtime possibility this
/// function has no equivalent of. That is what lets `props` be built by
/// insertion without a collision check: every name here is distinct, non-blank,
/// and exactly as the author typed it.
fn returns_schema(returns: &[egret_dataapi_api::ColumnSpec]) -> Value {
    let mut props = Map::new();
    for c in returns {
        props.insert(
            c.name.clone(),
            json!({
                // `declared_json_type`, NOT `json_type`: see that function for
                // the object-declared-as-string this got wrong in the browser.
                "type": declared_json_type(&c.ty),
                "nullable": true,
                // `x-egret-DECLARED-type`, not `x-egret-engine-type`. The
                // extension `row_schema` writes carries the string an engine
                // gave for a column it really produced; this one carries a
                // string the AUTHOR typed into `output.transform.returns`, for
                // a field no engine ever saw — a transform's `data` is whatever
                // the script returned. Spelling them the same made a consumer's
                // "what did the database call this" and a consumer's "what did
                // the author call this" indistinguishable in the document.
                "x-egret-declared-type": c.ty,
            }),
        );
    }
    let required: Vec<&str> = returns
        .iter()
        .filter(|c| c.required)
        .map(|c| c.name.as_str())
        .collect();
    let mut schema = json!({
        "type": "object",
        "properties": props,
        // The document must not be LOOSER than the gate. `check_returns` runs
        // against every response and refuses an undeclared field — serving
        // fails closed with a 500 rather than passing it on — so an object with
        // an extra key is not a response this endpoint can produce. Without
        // this line the generated contract said the opposite by omission
        // (`additionalProperties` defaults to permitted), and a consumer
        // generating a client from it would model a shape the server refuses.
        "additionalProperties": false,
        "description": "This endpoint has a response transform: `data` is whatever the \
                        script returns, declared here from `output.transform.returns`. \
                        `output.shape` and `output.naming` are inert for this endpoint — \
                        they describe how a row straight from SQL becomes JSON, and a \
                        transform's output never goes through that step. The property \
                        list is exhaustive: `check_returns` refuses a response carrying \
                        a field this list does not declare.",
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

/// What `data` holds, for THIS endpoint (spec §4.4).
///
/// A response transform, when declared, is the WHOLE answer: `returns_schema`
/// from `output.transform.returns`, whatever the statement count. Without one,
/// `statements_data_schema` documents the SQL statements' own columns — see
/// that function for the one-vs-several shape.
///
/// **Not a union**, for two independent reasons. First, the one
/// `statements_data_schema` already gives: an endpoint's statement list is
/// frozen, so `oneOf` over one-vs-several would ask every consumer to branch
/// on a question their own endpoint has already answered. Second, and new
/// here: without a transform `data` is rows straight from SQL; with one it is
/// whatever the script returns. Documenting both for one endpoint would
/// describe an endpoint that answers two different ways, when `output.transform`
/// is a fixed fact of the published snapshot, not something a caller chooses
/// per request.
fn data_schema(snap: &EndpointSnapshot, endpoint: &str) -> Result<Value, OpenApiGenError> {
    if snap.statements.is_empty() {
        return Err(OpenApiGenError::Core(egret_core::Error::Internal(
            anyhow::anyhow!("published snapshot for endpoint {endpoint} carries no statements"),
        )));
    }
    if let Some(transform) = &snap.output.transform {
        return Ok(returns_schema(&transform.returns));
    }
    statements_data_schema(snap, endpoint)
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
    // Matches `serving::router::meta_contract`'s own condition exactly: a
    // transform takes the keyed arm whatever its statement count, so that the
    // endpoint-wide `row_count` — which would describe `data`, and a
    // transform's `data` is not the rows this counts — is omitted. See Task
    // 6's `meta_contract` for why the keyed arm, not a third variant, is the
    // right shape to fall back to.
    if snap.statements.len() < 2 && snap.output.transform.is_none() {
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
        // Two readings of the SAME keyed arm, and the sentence has to say which
        // one this document is. Without a transform, `data` really is keyed by
        // these names — that is what "the same names" is telling a consumer to
        // rely on. WITH one, `data`'s keys come from `output.transform.returns`
        // and have nothing to do with the statement names; the original
        // sentence was measured in a generated document on 2026-08-27 saying
        // `data` was keyed by `main` while `data` held `by_region`/`regions`.
        // Third time in this programme a shipped sentence described behaviour
        // that does not exist.
        let statements_description = if snap.output.transform.is_some() {
            "One entry per statement, under the statements' own names. This endpoint has a \
             response transform, so these are NOT `data`'s keys — `data` is what the script \
             returned, and these describe the rows it was given."
        } else {
            "One entry per statement, under the same names, in the same order, as `data`."
        };
        obj.insert(
            "statements".into(),
            json!({
                "type": "object",
                "description": statements_description,
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
///
/// # The status list is DERIVED, and that is the whole point of this function
///
/// It used to be the literal `"400", "401", "403", "404", "429", "500", "503"`.
/// Package 3b then made `502 engine_unavailable` and `504 timeout` live on this
/// surface, the root `utoipa` document was corrected, and **this one was not** —
/// so a consumer who downloads its own contract and generates a client from it
/// gets a client with no case for the status it receives when the coordinator
/// goes down. That is precisely the "the caller cannot act on it" defect this
/// package exists to remove, in the more consumer-facing of the two documents,
/// and nothing could catch it: the `openapi.json` byte gate compares the ROOT
/// document to the committed copy of itself and has never read this builder.
///
/// So the list is not written here. [`crate::serving::error::wire_answers`]
/// applies `to_wire` to the taxonomy census — which is checked against
/// `ServingError`'s own source — and a status this surface can answer with
/// therefore appears here by construction. A new variant cannot land without an
/// arm in `to_wire` (a compile error) and a census sample (a named test
/// failure), and once it has both, this document already describes it.
///
/// Each status also NAMES its codes, for the same reason the root document's
/// descriptions do: `error.code` is the stable thing a client branches on, and a
/// bare `"Error"` tells a generated client nothing about which of the two 504s
/// it just received. The names come from the same derivation, so they cannot
/// disagree with the statuses above them.
/// The `X-Egret-Version` request parameter, as this document declares it.
///
/// `version` is the operation's own `x-egret-version` — the version this
/// description was generated FROM — and it appears as the `example` so that a
/// consumer who copies it gets the contract they are reading rather than a
/// number from somewhere else. It is deliberately not the `default`: a default
/// would make every generated client pin, forever, to whatever version was live
/// on the day it was generated, which is the opposite of the opt-in this is.
///
/// `minimum: 1` matches [`crate::serving::pin::requested_version`]'s own floor,
/// so a generated client refuses `0` before the network does. The rest of that
/// function's strictness (no sign, no whitespace, digits only) is not expressible
/// in an `integer` schema and does not need to be — a client that sends an
/// integer produces an accepted spelling by construction.
fn version_pin_parameter(version: i32) -> Value {
    json!({
        "name": crate::serving::pin::VERSION_PIN_HEADER_DOCUMENTED,
        "in": "header",
        "required": false,
        "description": "Pin this call to a published version of this endpoint. \
                        Omit it to call whatever is live, which is what every caller \
                        did before this header existed. The number is `meta.version` \
                        in a served response and `x-egret-version` on this operation. \
                        A pin selects which frozen contract answers and never which \
                        live policy applies — grants, the governed-database check, \
                        the endpoint's state and this endpoint's rate limit are all \
                        evaluated exactly as they are for an unpinned call. \
                        A version this endpoint never published, or one a rollback \
                        has moved consumers off, is refused with `409 version_unavailable`.",
        "schema": { "type": "integer", "format": "int32", "minimum": 1 },
        "example": version,
    })
}

fn error_responses() -> Value {
    let mut out = Map::new();
    for (status, codes) in crate::serving::error::wire_answers() {
        let named = codes
            .iter()
            .map(|c| format!("`{c}`"))
            .collect::<Vec<_>>()
            .join(", ");
        // Two of these statuses carry two codes each (500 and 504), which is the
        // whole reason a consumer branches on `code` and not on the status line.
        let named = if codes.len() > 1 {
            format!("one of {named}")
        } else {
            named
        };
        out.insert(
            status.to_string(),
            json!({
                "description": format!(
                    "Error. `error.code` is {named} — branch on it, not on the message.",
                ),
                "content": { "application/json": {
                    "schema": { "$ref": "#/components/schemas/ServingErrorBody" }
                }}
            }),
        );
    }
    Value::Object(out)
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
    // The path and method are the LIVE route (`live_path` / `live_method`), the
    // URL the serving router matches — not the draft's, which may carry a new
    // path that only goes live at the next publish (KI-023).
    //
    // `LIMIT $3` fetches ONE more row than may be described, so an overflow is
    // detected from the count rather than from a document that came back short.
    // See [`MAX_DOCUMENTED_ENDPOINTS`].
    let rows: Vec<(Uuid, String, String, String, String, Value, i32)> = sqlx::query_as(
        "SELECT e.id, e.name, e.description, e.live_path, e.live_method, ev.snapshot, ev.version \
         FROM dataapi.endpoints e \
         JOIN dataapi.endpoint_versions ev ON ev.id = e.live_version_id \
         WHERE e.workspace_id = $1 AND e.state = 'live' AND e.live_path IS NOT NULL \
           AND ($2::uuid IS NULL OR e.id = $2) \
         ORDER BY e.live_path \
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

        let mut params: Vec<Value> = snap
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
        // Package 7's pin, on every operation. Without it a consumer generating
        // a client from this document has no argument for the one header that
        // lets them stay on the contract this very document describes — which
        // would leave the feature reachable only by hand-editing requests, i.e.
        // not reachable by the audience it exists for.
        //
        // `in: header`, which is why it can be appended to an author's own
        // parameter list with no possibility of collision: an author's names all
        // live in `in: query` and OpenAPI keys a parameter on the
        // `(name, in)` PAIR. A `?version=` pin would have had to share the
        // author's namespace, where `version` is a legal — and already
        // publishable — parameter name. See `serving::pin` for the whole
        // argument.
        params.push(version_pin_parameter(version));

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
                        // The `enum` is DERIVED — `wire_codes` is `to_wire`
                        // applied to the variant census. Written out by hand
                        // it would be the third hand-maintained list of this
                        // surface's answers in this programme, and the first
                        // two both went stale; see `serving::error::wire_codes`.
                        //
                        // Without it a generated client typed this field
                        // `string`, so the ONE field the description calls
                        // "the stable code to branch on" was the one field a
                        // consumer could not branch on safely. `example` stays:
                        // it shows the shape at a glance in a rendered document,
                        // where an enum of two dozen strings is a wall.
                        "code": {
                            "type": "string",
                            "description": "The stable code to branch on. `message` is for \
                                            humans and may be reworded. The list is every \
                                            code this surface can answer with, so a client \
                                            may treat an unlisted value as a protocol error.",
                            "enum": crate::serving::error::wire_codes(),
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
    use egret_dataapi_api::{ColumnSpec, ParamSpec, TransformSpec};

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
        // And the RIGHT number. `is_number()` alone holds for any parse at
        // all, including one that lost the fraction.
        assert_eq!(s["minimum"], json!(0.5), "{s}");
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
        // `param_schema` writes no `description` key for ANY input, so
        // asserting its absence is green whatever the bounds do. Assert what
        // can actually fail instead: neither bound may appear ANYWHERE in the
        // schema, under any key a future edit might reach for.
        let schema = param_schema(&p).to_string();
        assert!(
            !schema.contains("2020-01-01") && !schema.contains("2030-12-31"),
            "the bounds must not also be duplicated into the schema: {schema}"
        );
    }

    /// Both types, because a `String` cannot carry `minimum`/`maximum` at all
    /// (`check_constraints` refuses the combination), so on a string param
    /// that half of the assertion could never fail however `param_schema`
    /// changed. The number param is the one that can.
    #[test]
    fn an_unconstrained_parameter_gains_no_keys() {
        for p in [string_param("s"), number_param("n")] {
            let s = param_schema(&p);
            for k in ["minimum", "maximum", "minLength", "maxLength", "pattern"] {
                assert!(
                    s.get(k).is_none(),
                    "unconstrained `{}` parameter grew `{k}`: {s}",
                    p.name
                );
            }
        }
    }

    /// `is_plain_decimal` — the ONLY gate `check_constraints` applies at
    /// publish, and the same one `coerce` applies to a caller's value —
    /// accepts a leading zero (`"007"`, `"00"`, `"01.5"`), because it is a
    /// text-comparison grammar, not JSON's own number grammar. Such a bound
    /// publishes today and IS enforced. `json_number` must normalise it
    /// rather than let `str::parse::<serde_json::Number>` reject it and
    /// silently drop the keyword — the exact failure this task exists to
    /// close, one step removed: a document that omits `minimum` describes an
    /// endpoint wider than the one `coerce` actually enforces.
    #[test]
    fn a_leading_zero_bound_still_reaches_the_schema() {
        let mut p = number_param("n");
        p.minimum = Some("007".into());
        p.maximum = Some("00".into());
        let s = param_schema(&p);
        assert_eq!(s["minimum"], json!(7), "{s}");
        assert_eq!(s["maximum"], json!(0), "{s}");

        let mut q = number_param("q");
        q.minimum = Some("01.5".into());
        let s = param_schema(&q);
        assert_eq!(s["minimum"], json!(1.5), "{s}");
    }

    /// A `serde_json::Number` written out as plain `-?digits[.digits]`, with
    /// any exponent expanded — WITHOUT going through a float.
    ///
    /// serde_json prints a small-magnitude float in exponent form (`1e-14`),
    /// which `compare_decimal_text` has no grammar for. Reading the number
    /// back as an `f64` to compare would defeat the point of the test, which
    /// is to check that no float rounding happened; so the printed digits are
    /// shifted as text instead.
    fn plain_decimal_of(n: &serde_json::Number) -> String {
        let printed = n.to_string();
        let (mantissa, exponent) = match printed.split_once(['e', 'E']) {
            Some((m, e)) => (m, e.parse::<i32>().expect("serde_json printed an exponent")),
            None => (printed.as_str(), 0),
        };
        let (sign, body) = mantissa
            .strip_prefix('-')
            .map_or(("", mantissa), |b| ("-", b));
        let (int, frac) = body.split_once('.').map_or((body, ""), |(i, f)| (i, f));
        let mut digits: Vec<u8> = int.bytes().chain(frac.bytes()).collect();
        // Where the decimal point sits, counted from the left of `digits`.
        let mut point = int.len() as i32 + exponent;
        while point < 1 {
            digits.insert(0, b'0');
            point += 1;
        }
        while point as usize > digits.len() {
            digits.push(b'0');
        }
        let (i, f) = digits.split_at(point as usize);
        let int = String::from_utf8(i.to_vec()).expect("digits");
        let frac = String::from_utf8(f.to_vec()).expect("digits");
        if frac.is_empty() {
            format!("{sign}{int}")
        } else {
            format!("{sign}{int}.{frac}")
        }
    }

    /// **The seam.** Every other test here pins ONE hand-written bound, which
    /// is why a bound that `check_constraints` accepted and `param_schema`
    /// then ROUNDED survived a task review: no test asked the two halves to
    /// agree, only that each behaved as its own author expected.
    ///
    /// This one asks, as one total statement over both halves: for every
    /// bound, publish either refuses it, or the document states that same
    /// number — never a rounded one, and never nothing. Both outcomes are
    /// pinned per bound, so neither "accept everything and round" nor "refuse
    /// everything" passes.
    #[test]
    fn every_bound_check_constraints_accepts_is_stated_faithfully() {
        // Ordinary bounds, the leading-zero spellings `is_plain_decimal`
        // admits, both sides of zero, the widest bounds the cap allows (as an
        // integer, a fraction and a mixed spelling), one small enough that
        // serde_json prints it in exponent form — and, on the far side of the
        // cap, one digit past it in each spelling plus the measured 40-digit
        // case that made this test necessary.
        // Past `f64`'s range entirely — built rather than typed, because 401
        // characters in a table is unreadable.
        let past_f64_range = format!("1{}", "0".repeat(400));
        let bounds: [(&str, bool); 22] = [
            ("0", true),
            ("-0", true),
            ("1", true),
            ("-1", true),
            ("100", true),
            ("0.5", true),
            ("-0.5", true),
            ("1.50", true),
            ("007", true),
            ("00", true),
            ("01.5", true),
            // The cap, exactly.
            ("999999999999999", true),
            ("-999999999999999", true),
            ("0.99999999999999", true),
            ("123456789012.345", true),
            ("0.00000000000001", true),
            // One digit past it.
            ("1000000000000000", false),
            ("-1000000000000000", false),
            ("0.000000000000001", false),
            ("123456789012.3456", false),
            // Measured: through an `f64` this reached the document as the
            // double nearest to it, exactly
            // 1234567890123456846996462118072609669120 (serde_json prints its
            // shortest form, 1234567890123456700000000000000000000000) — a
            // HIGHER minimum than `coerce` enforces, so a generated client
            // would refuse values the server accepts.
            ("1234567890123456789012345678901234567890", false),
            // Past `f64`'s range entirely, where the keyword did not round —
            // it disappeared, leaving a document describing a wider endpoint
            // than exists.
            (past_f64_range.as_str(), false),
        ];

        for (text, publishes) in bounds {
            for constraint in ["minimum", "maximum"] {
                let mut p = number_param("n");
                if constraint == "minimum" {
                    p.minimum = Some(text.into());
                } else {
                    p.maximum = Some(text.into());
                }

                let accepted = crate::lifecycle::check_constraints(std::slice::from_ref(&p));
                assert_eq!(
                    accepted.is_ok(),
                    publishes,
                    "`{constraint}: {text}` — publish disagreed with the table: {accepted:?}"
                );
                if !publishes {
                    // Refused, so nothing is documented and there is nothing
                    // to be faithful about.
                    continue;
                }

                let schema = param_schema(&p);
                let stated = schema
                    .get(constraint)
                    .unwrap_or_else(|| panic!("`{constraint}: {text}` vanished from {schema}"))
                    .as_number()
                    .unwrap_or_else(|| panic!("`{constraint}: {text}` is not a number: {schema}"));

                let round_tripped = plain_decimal_of(stated);
                assert_eq!(
                    crate::serving::params::compare_decimal_text(&round_tripped, text),
                    std::cmp::Ordering::Equal,
                    "`{constraint}: {text}` reached the document as {round_tripped} — the \
                     contract states a bound the author never wrote"
                );
            }
        }
    }

    /// The refusal the seam test counts on, read as PROSE rather than as a
    /// boolean: an author who cannot publish has to be able to tell which
    /// bound to change and what to change it to.
    #[test]
    fn a_bound_too_wide_for_the_document_is_refused_by_name() {
        let too_wide = "1234567890123456789012345678901234567890";
        let mut p = number_param("n");
        p.minimum = Some(too_wide.into());
        let err = crate::lifecycle::check_constraints(std::slice::from_ref(&p))
            .expect_err("a bound the document cannot state must not publish");
        let msg = err.to_string();
        assert!(msg.contains("`minimum`"), "must name the constraint: {msg}");
        assert!(msg.contains("`n`"), "must name the parameter: {msg}");
        assert!(
            msg.contains("Round the bound"),
            "must tell the author what to do: {msg}"
        );
        assert!(
            msg.contains(&crate::lifecycle::MAX_BOUND_DIGITS.to_string()),
            "must say what the limit is: {msg}"
        );
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
            source: egret_dataapi_api::StatementSource::Sql("SELECT 1".into()),
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
            iterated: Vec::new(),
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

    // -----------------------------------------------------------------------
    // C2 Task 7 — the contract is generated from `returns`
    // -----------------------------------------------------------------------

    /// A snapshot with ONE statement, carrying a response transform whose
    /// `returns` is `returns`. Mirrors `serving::router`'s own
    /// `transformed_snapshot` test fixture — a single `main` statement is
    /// enough to exercise the transformed path, and keeping it to one
    /// statement is what lets
    /// `the_contract_says_row_count_is_absent_for_a_transformed_endpoint`
    /// tell the fix apart from the pre-existing "several statements omit
    /// `row_count` anyway" case.
    fn transformed_snapshot_for_doc(returns: Vec<ColumnSpec>) -> EndpointSnapshot {
        let mut snap = snap_with_columns(vec![col("n", "integer", true)], "as_is");
        snap.output.transform = Some(TransformSpec {
            script: "def transform(params, statements):\n    return {}".into(),
            returns,
        });
        snap
    }

    /// **The scenario the whole task turns on.** A transformed endpoint's
    /// `data` schema is built from `output.transform.returns`, not from the
    /// SQL statement's own columns — `required` names exactly the `returns`
    /// entries marked `required: true`, and the SQL column `n` must not leak
    /// in, or the document would describe two different things a caller could
    /// receive.
    #[test]
    fn a_transformed_endpoints_data_schema_comes_from_returns() {
        let snap = transformed_snapshot_for_doc(vec![
            col("region", "varchar", true),
            col("total", "bigint", false),
        ]);
        let doc = data_schema(&snap, "e").expect("a transformed endpoint has a document");
        assert_eq!(doc["required"], json!(["region"]), "{doc}");
        assert!(doc["properties"]["total"].is_object(), "{doc}");
        assert!(
            doc["properties"].get("n").is_none(),
            "the underlying SQL column must not leak into a transformed endpoint's declared \
             data: {doc}"
        );
    }

    /// The declared object is CLOSED, because `check_returns` closes it.
    ///
    /// `check_returns` (`crate::transform`) runs against every response and
    /// refuses an undeclared field — serving turns that into a 500 rather than
    /// passing the field on. A document that omits `additionalProperties` says
    /// the opposite by omission, so the generated contract was strictly LOOSER
    /// than the gate: a client generated from it would model a response the
    /// server refuses to send.
    ///
    /// Asserted as `json!(false)` rather than `.is_some()`: the failure this
    /// exists to catch is the key being present and permissive.
    #[test]
    fn a_transformed_endpoints_declared_object_is_closed_like_check_returns_is() {
        let snap = transformed_snapshot_for_doc(vec![col("region", "varchar", true)]);
        let doc = data_schema(&snap, "e").expect("a transformed endpoint has a document");
        assert_eq!(
            doc["additionalProperties"],
            json!(false),
            "`check_returns` 500s on an undeclared field, so the document may not say a \
             consumer might receive one: {doc}"
        );
    }

    /// A declared type is labelled as DECLARED, never as an engine's.
    ///
    /// `row_schema` stamps `x-egret-engine-type` on a column an engine really
    /// produced, and that is what the extension means everywhere else in this
    /// document. `returns` is the author's own list for a field no engine ever
    /// saw — a transform's `data` is whatever the script returned — so reusing
    /// the same key told a consumer that a string the author typed came from
    /// the database.
    #[test]
    fn a_declared_return_type_is_not_labelled_as_an_engine_type() {
        let snap = transformed_snapshot_for_doc(vec![col("region", "my_own_type", true)]);
        let doc = data_schema(&snap, "e").expect("a transformed endpoint has a document");
        let prop = &doc["properties"]["region"];
        assert_eq!(prop["x-egret-declared-type"], json!("my_own_type"), "{doc}");
        assert!(
            prop.get("x-egret-engine-type").is_none(),
            "no engine produced this column, so nothing here may claim an engine named it: \
             {doc}"
        );
    }

    /// Without a transform, several statements document `data` as an object
    /// keyed by statement name
    /// (`several_statements_document_data_as_an_object_keyed_by_statement_name`).
    /// With one, `data` is whatever the script returns — documenting both
    /// would describe an endpoint that answers two different ways, so the
    /// statement keys (`rows`, `total`) must not appear anywhere in the
    /// transformed document.
    #[test]
    fn a_transformed_endpoint_does_not_document_the_statement_keyed_data() {
        let mut snap = snap_with_statements(
            vec![
                stmt("rows", vec![col("id", "integer", true)]),
                stmt("total", vec![col("n", "bigint", true)]),
            ],
            json!({}),
        );
        snap.output.transform = Some(TransformSpec {
            script: "def transform(params, statements):\n    return {\"total\": 1}".into(),
            returns: vec![col("total", "bigint", true)],
        });
        let doc = data_schema(&snap, "e").expect("a transformed endpoint has a document");
        let s = doc.to_string();
        assert!(
            !s.contains("\"rows\""),
            "the statement keys must not appear in `data`: {s}"
        );
    }

    /// Task 6 omits `row_count` from a served, transformed response
    /// (`meta_contract`'s keyed arm). A generated contract that still
    /// promised it would be a lie a generated client acts on.
    ///
    /// A single-statement snapshot on purpose: with two or more statements
    /// `row_count` was already absent before this task, so that shape cannot
    /// tell the fix apart from doing nothing. Only a single statement PLUS a
    /// transform exercises the new condition.
    #[test]
    fn the_contract_says_row_count_is_absent_for_a_transformed_endpoint() {
        let snap = transformed_snapshot_for_doc(vec![col("total", "bigint", true)]);
        let meta = meta_schema(&snap);
        assert!(meta["properties"].get("row_count").is_none(), "{meta}");
        assert!(meta["properties"].get("columns").is_none(), "{meta}");
        assert!(
            !meta["required"]
                .as_array()
                .expect("required array")
                .iter()
                .any(|v| v == "row_count"),
            "{meta}"
        );
    }

    /// **Found in a browser, not by review** (Task 10, 2026-08-27). A script
    /// returning `{"by_region": {...}, "regions": 3}` was served that object
    /// verbatim while the generated document declared `by_region` a `string`,
    /// because `returns_schema` reused `json_type` — the mapper for a type
    /// name an ENGINE produced, where "unrecognised means string" is safe. A
    /// transform's `ty` is checked by nothing at runtime, so the document is
    /// its only consumer and the author is naming a JSON type.
    ///
    /// Both halves are asserted: the JSON type names now map, and anything
    /// else still falls through to `json_type` unchanged, so an author who
    /// writes an engine spelling here loses nothing.
    #[test]
    fn a_transforms_declared_object_is_documented_as_an_object() {
        let snap = transformed_snapshot_for_doc(vec![
            col("by_region", "object", true),
            col("series", "array", true),
            col("regions", "integer", true),
            col("label", "varchar", true),
        ]);
        let doc = data_schema(&snap, "e").expect("a transformed endpoint has a document");
        assert_eq!(doc["properties"]["by_region"]["type"], "object", "{doc}");
        assert_eq!(doc["properties"]["series"]["type"], "array", "{doc}");
        assert_eq!(doc["properties"]["regions"]["type"], "integer", "{doc}");
        assert_eq!(
            doc["properties"]["label"]["type"], "string",
            "an engine spelling must still fall through to `json_type`: {doc}"
        );
    }

    /// The keyed `meta.statements` arm is shared by two endpoints that mean
    /// different things by it, and its sentence used to state the untransformed
    /// reading for both: "under the same names, in the same order, as `data`"
    /// — measured in a live generated document where `data` held
    /// `by_region`/`regions` and `meta.statements` held `main`.
    ///
    /// Asserted from BOTH sides, because a rule that only fires one way is how
    /// the first sentence survived: the untransformed multi-statement document
    /// must keep the original claim, since there `data` really is keyed by
    /// these names.
    #[test]
    fn the_statements_meta_does_not_claim_data_is_keyed_by_statement_name_when_transformed() {
        let mut snap = snap_with_statements(
            vec![
                stmt("rows", vec![col("id", "integer", true)]),
                stmt("total", vec![col("n", "bigint", true)]),
            ],
            json!({}),
        );

        let untransformed = meta_schema(&snap);
        let plain = untransformed["properties"]["statements"]["description"]
            .as_str()
            .expect("a description");
        assert!(
            plain.contains("the same names, in the same order, as `data`"),
            "without a transform `data` IS keyed by these names: {plain}"
        );

        snap.output.transform = Some(TransformSpec {
            script: "def transform(params, statements):\n    return {\"total\": 1}".into(),
            returns: vec![col("total", "bigint", true)],
        });
        let transformed = meta_schema(&snap);
        let d = transformed["properties"]["statements"]["description"]
            .as_str()
            .expect("a description");
        assert!(
            !d.contains("as `data`"),
            "a transformed endpoint's `data` is not keyed by statement name: {d}"
        );
        assert!(
            d.contains("response transform"),
            "and the document has to say why: {d}"
        );
    }

    /// The refactor that makes `data_schema`/`meta_schema` transform-aware
    /// must not move a single byte of an UNTRANSFORMED endpoint's document —
    /// this is the test most likely to catch a change that "just moves" the
    /// generation code. Pinned as a full literal rather than a handful of
    /// spot assertions, because a partial pin is exactly the kind of check
    /// that lets an unrelated field move.
    #[test]
    fn an_untransformed_endpoints_document_is_unchanged() {
        let snap = snap_with_columns(vec![col("id", "integer", true)], "as_is");
        let doc = success_schema(&snap, "e").expect("document");
        assert_eq!(
            doc,
            json!({
                "type": "object",
                "required": ["status", "message", "data", "meta"],
                "properties": {
                    "status": { "type": "string", "enum": ["ok"] },
                    "message": {
                        "type": "string",
                        "nullable": true,
                        "description": "Always null on success. A string here is the human half \
                                        of a refusal — read `error.code`.",
                    },
                    "data": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "integer",
                                    "nullable": true,
                                    "x-egret-engine-type": "integer",
                                }
                            },
                            "required": ["id"],
                        }
                    },
                    "meta": {
                        "type": "object",
                        "required": ["version", "cached", "request_id", "row_count", "columns"],
                        "properties": {
                            "version": {
                                "type": "integer",
                                "description": "The published version that served this call.",
                            },
                            "cached": { "type": "boolean" },
                            "request_id": {
                                "type": "string",
                                "description": "Correlation id, also written to this call's \
                                                access-log row.",
                            },
                            "row_count": { "type": "integer" },
                            "columns": {
                                "type": "array",
                                "description": "The FROZEN contract from the published \
                                                snapshot, not the shape this execution happened \
                                                to return.",
                                "items": { "$ref": "#/components/schemas/ServingColumn" },
                            }
                        }
                    }
                }
            }),
            "an untransformed endpoint's document must not move: {doc}"
        );
    }
}
