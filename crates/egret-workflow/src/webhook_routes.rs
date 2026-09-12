// kestra: webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java

//! The **unauthenticated** webhook surface — W6 Task 7.
//!
//! ```text
//! POST|PUT|GET  /workflows/v1/{ws}/executions/webhook/{namespace}/{flow}/{key}
//! ```
//!
//! Kestra's is
//! `@AnonymousAccess @Post("/api/v1/{tenant}/executions/webhook/{namespace}/{id}/{key}{/path}")`
//! — measured off `ExecutionController`'s bytecode and confirmed against a
//! running server. This is that shape with Kestra's `{tenant}`, which it
//! resolves from the request through `tenantService.resolveTenant()`, replaced
//! by egret's `{ws}` **slug in the path**.
//!
//! **The parameter is spelled `{ws}` and not `{workspace}`, and that is a
//! convention rather than a shortening.** `egret-server`'s
//! `feature_route_coverage` suite reads `{workspace}` as the marker of a route
//! the `WsContext` feature gate is expected to cover, and this surface is
//! deliberately outside that gate — it has no session for the gate to resolve.
//! `egret-dataapi`'s serving router carries the identical spelling for the
//! identical reason, and `egret-workspace`'s `features.rs` records both
//! exemptions beside the module each belongs to. **Turning the `workflows`
//! module off does not stop webhook traffic**; `state = 'disabled'` on the flow
//! does, which is the same answer the Data API gives for its own serving path.
//!
//! # Authorization is the whole of this module
//!
//! A webhook is an unauthenticated public surface by construction: **the key in
//! the URL is the only credential there is.** Everything below follows from
//! that.
//!
//! ## The scoping, and the two places it is load-bearing
//!
//! 1. **The flow lookup is `workspace_id = $1 AND namespace = $2 AND flow_id =
//!    $3`, and `workspace_id` is the ONLY thing standing between a caller and
//!    another tenant's flow.** `workflow.flows` carries
//!    `UNIQUE (workspace_id, namespace, flow_id)`, so two workspaces may both
//!    hold `io.kestra.tests/webhook` — that is not an edge case, it is what the
//!    corpus looks like when two people import it.
//!
//!    **`FlowRepo::find_by_name` must not be used here**, and its own doc says
//!    why: it is `WHERE namespace = $1 AND flow_id = $2 ORDER BY id LIMIT 1`,
//!    with no workspace predicate at all, because `WorkflowRefPort` has no
//!    caller-workspace parameter. `worker::subflow::resolve` already refused it
//!    for the same reason and recorded that **nothing in the schema is a second
//!    line of defence** — that module's own mutation showed a cross-workspace
//!    resolve producing a child execution with no constraint anywhere objecting.
//!    This module has the workspace, so it queries with it.
//!
//! 2. **The key is compared only against the triggers of the flow that lookup
//!    returned.** A key is not a workspace-wide capability and not a
//!    namespace-wide one: it authorizes exactly one trigger of exactly one
//!    flow of exactly one workspace. `tests/webhook_route.rs` stands two
//!    workspaces up holding byte-identical flows and asserts that each key
//!    fires its own and that **no execution row appears in the other
//!    workspace** — a count, not a status code, because a 404 with a
//!    side-effect is the failure that matters.
//!
//! An archived workspace answers 404: `get_by_slug(...).filter(|w| w.status ==
//! "active")`, the same predicate `egret-dataapi`'s serving path applies for
//! the same reason.
//!
//! ## What every refusal looks like, and why two of them diverge from Kestra
//!
//! | condition | Kestra | here |
//! |---|---|---|
//! | workspace absent or archived | n/a | **404** `not found: webhook` |
//! | flow absent, deleted, or unparseable | **404** `Flow not found` | **404** `not found: webhook` |
//! | no trigger whose key matches | **404** `Webhook not found` | **404** `not found: webhook` |
//! | flow `disabled` | **409**, *before* the key check | **409**, *after* it |
//! | matched trigger `disabled: true` | **409** `…the trigger 'x' is disabled.` | **409**, same sentence |
//! | trigger declares something egret cannot do | n/a | **409**, naming it |
//! | trigger `inputs:` will not render | **422** | **400** (`InvalidInput`) |
//!
//! **The first three rows are one byte-identical response**, and that is the
//! divergence. Kestra distinguishes "no such flow" from "no such webhook" on a
//! surface where the caller has presented no credential at all, which makes the
//! endpoint an oracle for *does workspace W, namespace N hold a flow called F*.
//! Anyone can enumerate it. Collapsing them costs an operator one diagnostic
//! and removes the enumeration; `the_three_absences_are_indistinguishable` is
//! the running check that they stay collapsed, and it compares **bodies**, not
//! just statuses.
//!
//! **The 409s were moved behind the key comparison** for the same reason, and
//! that reordering is safe in the direction that matters: a caller who has not
//! produced the key learns nothing, and a caller who has produced it already
//! knows the flow exists, so telling it *why* the request will not run discloses
//! nothing new. Neither divergence ever fires where Kestra would not — the
//! programme's actual constraint — because both only ever turn a 200 or a 409
//! into a 404.
//!
//! The `inputs:` row is a status difference and nothing more: egret has no
//! `Error` variant that maps to 422 and inventing one for this seam would be a
//! cross-crate change for one endpoint.
//!
//! # What is deliberately not here
//!
//! * **No rate limit.** `egret-dataapi`'s serving path carries a bulkhead and a
//!   token bucket; this does not, and a body cap is the only bound it has. An
//!   anonymous caller that knows a valid key can start executions as fast as it
//!   can post. That is a real exposure, it is stated rather than discovered, and
//!   it is the first thing to add if this surface is ever exposed publicly.
//!   Kestra has no limiter here either.
//! * **No `wait:`.** See [`crate::trigger::webhook`]'s "knowingly narrower"
//!   list. A flow declaring it is refused by name rather than answered with a
//!   response shape it did not ask for.
//! * **No OpenAPI document.** The management router's `openapi()` covers the
//!   session-authenticated seam; this one is machine-facing and anonymous, like
//!   `catalog_engine`'s mount, which also carries none.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use egret_core::{Error, Result as EgretResult};
use serde::Serialize;
use uuid::Uuid;

use crate::routes::WorkflowState;
use crate::trigger::webhook::{
    build_output, find, inputs_reference_the_trigger_root, render_inputs, ExecutionTrigger,
    KeyContext, Unsupported, WebhookRequest,
};

/// The largest webhook body this surface accepts, in bytes.
///
/// The only bound on an anonymous endpoint that writes to the database, so it
/// is deliberately small: the body is held whole in memory, parsed as JSON and
/// then written into a `jsonb` column, and every one of those steps is paid for
/// by egret rather than by the caller. 1 MiB is `egret-dataapi`'s serving cap
/// (`guard::SERVING_MAX_BODY_BYTES`) and there is no reason for this seam to be
/// more generous than the one that requires a token.
pub const MAX_WEBHOOK_BODY_BYTES: usize = 1024 * 1024;

/// The single "no" this surface gives to anyone who has not produced the key.
///
/// A constant rather than three call sites, because the property the tests
/// assert is that the three absences are **indistinguishable**, and three
/// hand-written strings drift.
fn not_found() -> Error {
    Error::NotFound("webhook".into())
}

/// `io.kestra.plugin.core.trigger.WebhookResponse`, reduced to the fields
/// egret can fill.
///
/// Kestra's carries `tenantId`, `id`, `namespace`, `flowId`, `flowRevision`,
/// `trigger`, `outputs`, `labels`, `state` and `url` (measured on the wire).
/// Dropped here, each with a reason: `flowRevision` (egret has no revision),
/// `outputs` (the `returnOutputs` streaming path, not implemented), `labels`
/// (`system.from`/`system.correlationId` are Kestra's execution-label scheme,
/// which egret does not have) and `url` (needs the configured public base URL,
/// which this crate does not read).
#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    /// egret's workspace id — Kestra's `tenantId`.
    pub workspace_id: Uuid,
    /// The execution that was created.
    pub id: Uuid,
    pub namespace: String,
    pub flow_id: String,
    /// `{id, type, variables}` — exactly what was stored on the execution row.
    pub trigger: ExecutionTrigger,
    /// Always `CREATED`: this surface answers asynchronously.
    pub state: &'static str,
}

/// The anonymous webhook router. Mount it **outside** `/api` and outside the
/// CSRF layer.
///
/// Outside `/api` because `WsContext`'s feature-entitlement gate recognises a
/// path beginning `/api/ws/` or `/ws/` and would demand a session this caller
/// does not have. Outside CSRF for the reason `egret-dataapi`'s serving mount
/// records: double-submit CSRF defends requests a browser authenticates
/// *ambiently*, and nothing here is ambient — the credential is a path segment
/// the caller types.
///
/// All three methods share one handler, as Kestra's five do.
pub fn webhook_router() -> Router<WorkflowState> {
    Router::new()
        .route(
            "/{ws}/executions/webhook/{namespace}/{flow}/{key}",
            post(handle).put(handle).get(handle),
        )
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
}

/// Every method lands here. `GET` carries no body, which is not a special case:
/// an empty body is already "no body" (see
/// [`crate::trigger::webhook::build_output`]).
async fn handle(
    State(st): State<WorkflowState>,
    Path((workspace, namespace, flow, key)): Path<(String, String, String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> EgretResult<Json<WebhookResponse>> {
    // --- 1. the workspace, from the path and from nowhere else ---------------
    //
    // `status == "active"` is checked here rather than left to a later query:
    // an archived workspace must not be startable by a caller holding an old
    // key, and this is the one place that fact is in hand.
    let ws = st
        .workspaces
        .get_by_slug(&workspace)
        .await?
        .filter(|w| w.status == "active")
        .ok_or_else(not_found)?;
    let ws_id = ws.id.0;

    // --- 2. the flow, scoped to THAT workspace -------------------------------
    //
    // Not `FlowRepo::find_by_name` -- see the module doc. `deleted_at IS NULL`
    // for the same reason `subflow::resolve` has it; `state` is read rather
    // than filtered, because a disabled flow still has to be told apart from an
    // absent one AFTER the key matches.
    let row: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, source, state FROM workflow.flows \
          WHERE workspace_id = $1 AND namespace = $2 AND flow_id = $3 \
            AND deleted_at IS NULL",
    )
    .bind(ws_id)
    .bind(&namespace)
    .bind(&flow)
    .fetch_optional(&st.db.pool)
    .await
    .map_err(|e| Error::Internal(anyhow::anyhow!("webhook flow lookup: {e}")))?;
    let Some((flow_uuid, source, state)) = row else {
        return Err(not_found());
    };

    // An unparseable draft has no triggers, so no key can match it. Answering
    // the same 404 rather than a 409 keeps it in the indistinguishable set.
    let Ok(parsed) = crate::spec::parse(&source) else {
        return Err(not_found());
    };

    // --- 3. the key ----------------------------------------------------------
    let kcx = KeyContext {
        flow_id: parsed.id.clone(),
        namespace: parsed.namespace.clone(),
        vars: parsed.variables.clone().into_iter().collect(),
    };
    let Some((trigger, webhook)) = find(&parsed.triggers, &key, &kcx) else {
        return Err(not_found());
    };

    // --- 4. from here the caller HAS the key, so refusals may explain ---------
    if trigger.disabled {
        // Kestra's own sentence, measured on the wire.
        return Err(Error::Conflict(format!(
            "Cannot execute webhook: the trigger '{}' is disabled.",
            trigger.id
        )));
    }
    if state == "disabled" {
        return Err(Error::Conflict(
            "Cannot execute flow: flow is disabled.".into(),
        ));
    }
    if webhook.wait {
        return Err(Error::Conflict(Unsupported::Wait.message(&trigger.id)));
    }

    // --- 5. what the flow will see -------------------------------------------
    let req = WebhookRequest {
        method: "POST".into(),
        query: parse_query(query.as_deref()),
        headers: collect_headers(&headers),
        content_type: headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        body: body.to_vec(),
    };
    let output = build_output(&req, webhook.fetch_type, None)
        .map_err(|u| Error::Conflict(u.message(&trigger.id)))?;

    // `inputs:` that read `trigger.*` cannot be honoured, and the refusal names
    // that rather than letting the renderer blame the author's template -- see
    // `trigger::webhook::render_inputs`.
    if inputs_reference_the_trigger_root(webhook) {
        return Err(Error::Conflict(format!(
            "webhook trigger '{}' declares inputs that read `trigger.*`, which egret cannot \
             render yet -- the trigger context is stored on the execution but is not published \
             into the render surface",
            trigger.id
        )));
    }
    let inputs = render_inputs(trigger, webhook, &kcx).map_err(Error::InvalidInput)?;

    // --- 6. start it ---------------------------------------------------------
    let record = ExecutionTrigger {
        id: trigger.id.clone(),
        r#type: trigger.r#type.clone(),
        variables: output,
    };
    let trigger_json = serde_json::to_value(&record)
        .map_err(|e| Error::Internal(anyhow::anyhow!("serialising the trigger record: {e}")))?;

    let id = crate::worker::start_execution_with_trigger(
        &st.db.pool,
        ws_id,
        Some(flow_uuid),
        &source,
        inputs,
        Some(trigger_json),
    )
    .await?;

    Ok(Json(WebhookResponse {
        workspace_id: ws_id,
        id,
        namespace: parsed.namespace,
        flow_id: parsed.id,
        trigger: record,
        state: "CREATED",
    }))
}

/// `URIBuilder(uri).getQueryParams()` — split on `&`, then on the **first**
/// `=`, percent-decoding both halves and treating `+` as a space.
///
/// Written out rather than taken from `serde_urlencoded`, because that crate
/// deserialises into a map and a map cannot hold `?a=1&a=2`, which is the one
/// shape this function exists to preserve.
fn parse_query(raw: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            // `?flag` with no `=` is a name with an empty value, which is what
            // `URIBuilder` produces for it too.
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

/// Percent-decoding with `+` as a space, lossy on invalid UTF-8.
///
/// Lossy rather than failing: a webhook must not 400 on a query string it can
/// still route, and the key itself never comes through here -- it is a path
/// segment axum has already decoded.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    // A stray `%` is a literal `%`, not an error.
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Request headers as `(lowercase-name, value)` in wire order.
///
/// Kestra's are `java.net.http.HttpHeaders.map()`, which is already a
/// multimap; the grouping into lists happens in
/// [`crate::trigger::webhook::build_output`] so that headers and query
/// parameters go through one code path.
///
/// A header whose value is not valid UTF-8 is **dropped**, not lossily decoded:
/// it would otherwise reach a flow as a string the caller never sent.
fn collect_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|s| (k.as_str().to_string(), s.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `?a=1&a=2` is the shape a map cannot hold, which is why this function
    /// exists at all.
    #[test]
    fn repeated_query_parameters_survive_parsing() {
        assert_eq!(
            parse_query(Some("validationToken=abc&validationToken=def&x=1")),
            vec![
                ("validationToken".to_string(), "abc".to_string()),
                ("validationToken".to_string(), "def".to_string()),
                ("x".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn query_parsing_handles_the_awkward_shapes() {
        assert_eq!(parse_query(None), vec![]);
        assert_eq!(parse_query(Some("")), vec![]);
        assert_eq!(
            parse_query(Some("flag")),
            vec![("flag".to_string(), String::new())],
            "a bare name is a name with an empty value"
        );
        assert_eq!(
            parse_query(Some("a=")),
            vec![("a".to_string(), String::new())]
        );
        assert_eq!(
            parse_query(Some("a=b=c")),
            vec![("a".to_string(), "b=c".to_string())],
            "split on the FIRST `=` only"
        );
        assert_eq!(
            parse_query(Some("a=1&&b=2")),
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string())
            ],
            "an empty pair is skipped, not turned into an empty name"
        );
    }

    #[test]
    fn percent_and_plus_decoding() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("100%25"), "100%");
        assert_eq!(percent_decode("%E2%9C%93"), "\u{2713}");
        assert_eq!(percent_decode("a%"), "a%", "a trailing stray % is literal");
        assert_eq!(percent_decode("a%zz"), "a%zz", "a bad escape is literal");
        // The control: decoding must actually be happening, or every assertion
        // above is satisfied by returning the input.
        assert_ne!(percent_decode("a%20b"), "a%20b");
    }

    /// The three absences share one message, and this is the assertion that
    /// they cannot drift apart -- there is only one constructor.
    #[test]
    fn there_is_exactly_one_not_found_message() {
        assert_eq!(not_found().to_string(), "not found: webhook");
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webhook_routes.rs"),
        )
        .expect("read own source");
        let shipping = &source[..source
            .find("\n#[cfg(test)]")
            .expect("this file has a test module")];
        assert_eq!(
            shipping.matches("Error::NotFound(").count(),
            1,
            "every 404 on this surface must go through not_found(), so that the three \
             absences stay byte-identical"
        );
        // Counted rather than guessed. The needle is the bare identifier,
        // because one of the four call sites passes the function rather than
        // calling it (`.ok_or_else(not_found)`) and a `not_found()` needle
        // silently misses it -- the same spelling trap that made this file's
        // sibling assertion in `trigger/webhook.rs` say "three" for two.
        assert_eq!(
            shipping.matches("not_found").count(),
            5,
            "the definition plus its FOUR call sites: no workspace, no flow, unparseable \
             flow, no matching key"
        );
    }
}
