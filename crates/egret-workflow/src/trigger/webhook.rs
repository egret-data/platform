// kestra: webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java
// kestra: core/src/main/java/io/kestra/plugin/core/trigger/Webhook.java
// kestra: core/src/main/java/io/kestra/plugin/core/trigger/AbstractWebhookTrigger.java
// kestra: webserver/src/main/java/io/kestra/webserver/services/MicronautHttpService.java
// kestra: core/src/main/java/io/kestra/core/services/WebhookService.java

//! The webhook trigger — W6 Task 7.
//!
//! # The ratchet says nothing about this module, and that is not a verdict
//!
//! Fifteen corpus flows declare a `io.kestra.plugin.core.trigger.Webhook`.
//! **Zero are reachable by the oracle**, and the reason is not an accident of
//! this task: `oracle-exec/harvest.py` **never calls**
//! `POST …/executions/webhook/…`. The string `webhook` appears in that file
//! exactly once, as the *reason* `valids/webhook-inputs` is excluded:
//!
//! ```text
//! "valids/webhook-inputs":
//!     "Executed only through POST /executions/webhook/{ns}/webhook-inputs/{key} with a "
//!     "webhook body, not from an execution POST."
//! ```
//!
//! So `oracle-exec/baseline.txt` did not move for this work and **could not
//! have**: 127 before, 127 after, measured both times with `--nocapture` (see
//! the task report). Every claim in this module is judged by its own tests,
//! which puts it where `trigger::schedule`, `trigger::flow`, `egret.sql` and
//! `egret.container` already sit. Nothing here reaches a database, a container
//! or a network, so there is no dependency to be missing and nothing to skip:
//! every test in this file runs on every `cargo test`, or the build fails.
//!
//! # Measured on the artifact, not read off a class
//!
//! Everything below was taken from `kestra/kestra@sha256:28f59bf1…` — the
//! pinned digest — by `javap`-ing the fat `/app/kestra` (class-file v69, so
//! Corretto 25) **and** by driving real requests at a running server. Where
//! the two could disagree, the running server wins and the bytecode says why.
//!
//! ## 1. The URL shape, and that it is unauthenticated
//!
//! `ExecutionController` carries
//! `@Controller("/api/v1/{tenant}/executions")`, and its five webhook methods
//! all carry the same `uri`:
//!
//! ```text
//! @AnonymousAccess
//! @Post(uri = "/webhook/{namespace}/{id}/{key}{/path}", consumes = "*/*")
//! Mono<HttpResponse<?>> triggerExecutionByPostWebhook(namespace, id, key, path, request)
//! ```
//!
//! — plus `triggerExecutionByGetWebhook`, `triggerExecutionByPutWebhook` and
//! the two multipart variants, on the identical template. **`@AnonymousAccess`
//! is the whole of the security model**: the key in the path is the only
//! credential, which is why [`keys_match`] is written the way it is.
//!
//! egret's equivalent is [`crate::webhook_routes`], and it differs in exactly
//! one structural way: Kestra's `{tenant}` is resolved by
//! `tenantService.resolveTenant()` from the *request*, and egret's
//! `{workspace}` is a slug in the path that is resolved to a workspace id and
//! then used to scope **every** subsequent lookup. See that module for the two
//! places the scoping is load-bearing.
//!
//! ## 2. The key comparison — `ExecutionController.lambda$findWebhook$2`
//!
//! ```text
//! RunContext runContext = runContextFactory.of(flow, w);
//! String webhookKey = runContext.render(w.getKey()).trim();
//! return MessageDigest.isEqual(webhookKey.getBytes(UTF_8), key.getBytes(UTF_8));
//! ```
//!
//! Four facts, each of which [`keys_match`] and [`render_key`] reproduce and
//! each of which has a test with a discriminating fixture:
//!
//! * **the DECLARED key is rendered**, so `key: "{{ flow.id }}"` matches the
//!   flow's id and does *not* match the literal text `{{ flow.id }}`
//!   (measured: `…/t7-probe-multi/t7-probe-multi` → 200,
//!   `…/t7-probe-multi/%7B%7B%20flow.id%20%7D%7D` → 404);
//! * **the declared key is `.trim()`ed and the URL key is NOT.** This is the
//!   one-sided trim, and it is the fixture that separates two rules which
//!   agree everywhere else. A flow declaring `key: "  spaced-key  "` answers
//!   **200** at `…/spaced-key` and **404** at `…/%20%20spaced-key%20%20`;
//! * **the comparison is constant-time** — `MessageDigest.isEqual`, not
//!   `String.equals`. [`ct_eq`] is a behavioural port of that method,
//!   including its length handling;
//! * **a key that fails to render makes that trigger invisible, not the
//!   request an error.** Kestra catches `IllegalVariableEvaluationException`,
//!   logs `Unable to render the webhook key {}, the webhook will be ignored`
//!   (interpolating the **URL** key, not the declared one — `aload_2`) and
//!   returns `false`. Measured: a flow whose *first* trigger declares
//!   `key: "{{ nosuchroot.nothing }}"` still answers 200 for its *third*
//!   trigger's literal key, and the server logs that warning once per
//!   unrenderable trigger examined.
//!
//! ## 3. What the request becomes — `Webhook.buildOutput`
//!
//! ```text
//! output = Output.builder()
//!     .headers(request.getHeaders() != null ? request.getHeaders().map() : null)
//!     .parameters(webhookService.parseParameters(context));
//! if (storedBodyUri != null) return output.uri(storedBodyUri.toString()).build();
//! switch (requestBody) {
//!     case MultipartFormDataRequestBody m -> output.parts(…).formFields(…);
//!     case ByteArrayRequestBody b         -> output.body(BASE64.encodeToString(b.getContent()));
//!     case StringRequestBody t            -> output.body(tryMap(s).or(() -> tryArray(s)).orElse(s));
//!     default                             -> output.body(requestBody.getContent());
//! }
//! ```
//!
//! and which `RequestBody` you get is `MicronautHttpService.byteArrayRequestBody`:
//!
//! ```text
//! if (content.length == 0) return null;                     // no body at all
//! isTextual(mediaType) ? StringRequestBody : ByteArrayRequestBody
//! ```
//!
//! with `isTextual` = `mediaType == null || mediaType.isTextBased() ||
//! mediaType == APPLICATION_FORM_URLENCODED_TYPE`.
//!
//! Reproduced by [`build_output`]; measured end to end:
//!
//! | request | `trigger.variables` |
//! |---|---|
//! | `POST` `{"hello":"world","n":3}`, `application/json` | `body` = the **object** |
//! | `POST` `[1,2]`, `application/json` | `body` = the **array** |
//! | `POST` `just plain text`, `text/plain` | `body` = the **string** |
//! | `GET` with no body | **no `body` key at all** |
//! | `?validationToken=abc&validationToken=def&x=1` | `parameters` = `{"validationToken":["abc","def"],"x":["1"]}` |
//! | `fetchType: NONE` | no `body`, no `uri` |
//! | `fetchType: STORE` | no `body`; `uri` = `kestra:///…/webhook/body` |
//!
//! `parameters` is `URIBuilder(uri).getQueryParams()` grouped by name into
//! lists (`Collectors.groupingBy` + `Collectors.mapping`), so a repeated
//! parameter keeps **both** values in URL order and a single one is still a
//! one-element list. `parameters` is `{}` — present and empty — when there is
//! no query string at all.
//!
//! ## 4. The refusals, and their order
//!
//! Measured, in the order Kestra evaluates them:
//!
//! | condition | Kestra | egret |
//! |---|---|---|
//! | no such flow | **404** `Flow not found` | **404**, body identical to the next row |
//! | flow `disabled: true` | **409** `Cannot execute flow: flow is disabled.` | **409**, but only *after* the key matches |
//! | no trigger whose key matches | **404** `Webhook not found` | **404** |
//! | matched trigger `disabled: true` | **409** `Cannot execute webhook: the trigger 'x' is disabled.` | **409** |
//! | trigger `inputs:` will not render | **422** | **422** (`InvalidInput`) |
//! | flow `draft: true` | **200** — a draft IS reachable | n/a: egret has no `draft:` on the flow model |
//!
//! **The two divergences are deliberate and are about disclosure, not
//! behaviour.** Kestra's first two rows answer differently for a caller who
//! has presented no credential at all, which makes the endpoint an oracle for
//! "does namespace N hold a flow called F, and is it switched on" — on a
//! surface that is anonymous by construction. egret collapses "no such flow"
//! into the same 404 as "no such webhook", and moves the disabled checks
//! **behind** the key comparison, so a 409 is only ever seen by a caller that
//! has already proved it holds the key. Neither direction fires where Kestra
//! would not, which is the constraint that actually binds
//! (`over-acceptance must not ship`); both are under-disclosure.
//!
//! # Where this is knowingly narrower than Kestra, stated rather than found
//!
//! * **`wait:`, `returnOutputs:`, `responseContentType:` and `responseCode:`
//!   are not implemented.** They are `Webhook.evaluate`'s streaming path —
//!   `WebhookService.followExecution` returns a `Flux<Event<Execution>>` and
//!   the response is held open until the execution terminates. egret answers
//!   the asynchronous shape only. A flow declaring them still fires; it simply
//!   gets the `CREATED` response immediately. **Not silently**: see
//!   [`Unsupported::Wait`].
//! * **`fetchType: STORE` is refused**, not approximated. It needs egret's
//!   object store, which this crate does not depend on, and answering as if it
//!   were `NONE` would hand the flow a `trigger` with the body silently
//!   missing. A refusal is the safe direction; a wrong output is not.
//! * **`multipart/form-data` is not implemented** — `parts` needs the same
//!   object store `STORE` does (each file part is written to internal storage
//!   and reaches the flow as a URI). Refused, for the same reason.
//! * **`trigger.*` is NOT published into the render context.** See the next
//!   section; this is the single most consequential omission here and it is
//!   deliberate.
//!
//! # Why `trigger.*` is not in the render context
//!
//! [`Output`] is *built* here and *stored* on the execution row
//! (`workflow.executions.execution_trigger`, migration 0140). It is **not**
//! published as a `trigger` root by `worker::render::Surface`, and this task
//! did not touch that file.
//!
//! Two reasons, one measured and one structural:
//!
//! 1. **The golden was harvested with no trigger context at all.** Every one of
//!    the 294 golden entries was produced by a plain execution POST, so
//!    Kestra's own `Execution.trigger` is null for all of them. A `trigger`
//!    root that exists unconditionally therefore changes the render context of
//!    293 entries that have nothing to do with webhooks, and the plan's rule
//!    is that an entry which moves for an unattributed reason is a coincidence
//!    rather than a bank.
//!
//!    **The specific risk this task was warned about does not exist, and the
//!    measurement is what says so.** The brief predicted that publishing the
//!    root "would make `valids/webhook-plaintext` diverge further". It does
//!    not: that entry renders
//!    `{{ trigger.parameters.validationToken[0] ?? 'hello-world' }}`, which
//!    misses at `trigger.parameters` and *then* indexes `.validationToken` on
//!    the result — two levels, where `oracle-exec/baseline.txt`'s
//!    missing-root/missing-member sentence is about one. With the root absent
//!    it errored; with the root present and empty it errored identically. The
//!    task report records the direct ratchet measurement taken by temporarily
//!    adding the root to `worker/render.rs`: **127 before, 127 after, zero
//!    entries moved.** Since package #5a Task A5 (2026-10-29, spec D6) `??`
//!    answers through a missing chain, so both contexts render the fallback,
//!    `hello-world`, as Kestra does, and the entry banked;
//!    [`tests::a_missing_trigger_chain_renders_webhook_plaintexts_fallback`]
//!    is the running program that holds all four rows.
//!
//!    So the reason not to publish it here is the 293, not the one.
//! 2. `worker/render.rs` already carries two findings assigned to W6 Task 8.
//!    Publishing the root is that task's, on top of the column this one
//!    writes.
//!
//! **The key-rendering context built here is a different object and is not
//! covered by that sentence.** [`KeyContext::render_context_for`] does bind a
//! `trigger` root, because Kestra's own `runContextFactory.of(flow, trigger)`
//! does and `key: "tid-{{ trigger.id }}"` was measured resolving to
//! `tid-trg-tid` on the artifact. It is local to the key comparison and cannot
//! reach the oracle.
//!
//! **"Never seen by a task" was false when it was first written**, and fix
//! round 1's Critical 2 is the proof: `inputs:` were rendered against that very
//! context, so `inputs: {x: "{{ trigger }}"}` handed the execution the key
//! comparison's private object. The sentence is true now because the thing it
//! describes changed, not because the wording did — `inputs:` render against
//! [`InputContext`], which binds no `trigger` at all, and
//! `tests::the_key_context_is_reachable_from_no_other_render` is the running
//! check that the key context has exactly one consumer.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::models::{FetchType, Trigger, TriggerKind, Webhook};
use crate::runners::{render_str, render_str_guarded, GuardedRender, RenderContext};

/// The request, reduced to what the trigger can see.
///
/// Deliberately owned and inert: nothing in this module touches an axum type,
/// so every arm below is reachable from a unit test without a server.
///
/// **There is no `method` field.** One was here, holding the HTTP verb, and it
/// was hard-coded `"POST"` by the only caller — so a GET recorded itself as a
/// POST and nothing could tell, because nothing read it (fix round 1's sweep).
/// Kestra's `Webhook.Output` has no method either. A field nothing reads is a
/// claim nothing can check, so it is deleted rather than fixed: the verb
/// genuinely does not affect what a trigger sees, and
/// `webhook_route::a_get_carries_parameters_and_no_body` is where the verbs are
/// distinguished by their observable consequence instead.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebhookRequest {
    /// Query parameters **in URL order, duplicates kept**. Grouping them is
    /// [`build_output`]'s job, and it cannot be done by the caller without
    /// losing the order two values of one name have to keep.
    pub query: Vec<(String, String)>,
    /// Request headers, lowercased names, in wire order.
    pub headers: Vec<(String, String)>,
    /// The `Content-Type` header value, if any, already extracted from
    /// [`Self::headers`]. `None` is *textual* — see [`is_textual`].
    pub content_type: Option<String>,
    /// The raw body. Empty means "no body", which is a distinct outcome from
    /// "an empty JSON object": Kestra's
    /// `MicronautHttpService.byteArrayRequestBody` returns `null` for a
    /// zero-length body and `Webhook.Output.body` is then absent entirely.
    pub body: Vec<u8>,
}

/// `io.kestra.plugin.core.trigger.Webhook$Output`, which is what Kestra puts
/// in `Execution.trigger.variables` and therefore what a flow sees as
/// `trigger.*`.
///
/// Every field is skipped when absent, because Kestra's own serialisation is
/// `@JsonInclude(NON_NULL)` through `ExecutionTrigger.of` and the measured
/// wire shape has no `body` key at all for a `GET`.
///
/// `parts` and `formFields` are **not fields here** — see the module doc's
/// "knowingly narrower" list. They cannot be half-supported: a `parts` entry
/// is a URI into internal storage, and there is nothing to put a file in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Output {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<BTreeMap<String, Vec<String>>>,
}

/// `io.kestra.core.models.executions.ExecutionTrigger`, which is the whole of
/// what gets stored on the execution row.
///
/// `{id, type, variables}` — the three fields `ExecutionTrigger.of(trigger,
/// output)` sets, confirmed on the wire:
///
/// ```json
/// "trigger": {"id":"hook","type":"io.kestra.plugin.core.trigger.Webhook",
///             "variables":{"body":…,"headers":…,"parameters":…}}
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionTrigger {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub variables: Output,
}

/// A declared webhook this port cannot honour, named rather than approximated.
///
/// Each of these fires **after** the key has matched, so reporting it discloses
/// nothing to a caller that has not already authenticated itself with the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    /// `fetchType: STORE` — the body goes to internal storage and the flow
    /// gets a `uri`. Needs an object store this crate does not depend on.
    StoreBody,
    /// `multipart/form-data` — `parts` are stored files.
    Multipart,
    /// `wait: true` — the response is held open until the execution terminates.
    Wait,
    /// `returnOutputs: true` — the terminal execution's `outputs` are put in
    /// the response body. Needs the same streaming path [`Self::Wait`] does.
    ReturnOutputs,
    /// `responseContentType:` — the content type of the body a waited response
    /// carries. Same path.
    ResponseContentType,
    /// `responseCode:` — the status a waited response carries. Same path.
    ResponseCode,
}

/// The declarations of `webhook` this port parses and cannot honour,
/// **refused by name** rather than accepted and ignored: the first one, or
/// `None`. The webhook route answers it with a 409 on every firing, after the
/// key has matched; `spec::schema`'s measurement calls it too, so none of
/// these properties is offered by the flow editor.
///
/// `returnOutputs`, `responseContentType` and `responseCode` were once parsed
/// into `models::Webhook` and then silently dropped (webhook fix round 1,
/// Important 3) -- the `lateMaximumDelay` shape: a field an author can write,
/// that validates, and that changes nothing.
pub fn declared_unsupported(webhook: &Webhook) -> Option<Unsupported> {
    [
        webhook.wait.then_some(Unsupported::Wait),
        webhook.return_outputs.then_some(Unsupported::ReturnOutputs),
        webhook
            .response_content_type
            .is_some()
            .then_some(Unsupported::ResponseContentType),
        webhook
            .response_code
            .is_some()
            .then_some(Unsupported::ResponseCode),
    ]
    .into_iter()
    .flatten()
    .next()
}

impl Unsupported {
    /// The message the route turns into a 409.
    pub fn message(&self, trigger_id: &str) -> String {
        match self {
            Self::StoreBody => format!(
                "webhook trigger '{trigger_id}' declares fetchType: STORE, which egret does not \
                 implement yet -- the request body would have to be written to internal storage \
                 and reach the flow as `trigger.uri`"
            ),
            Self::Multipart => format!(
                "webhook trigger '{trigger_id}' received a multipart/form-data body, which egret \
                 does not implement yet -- each file part would have to be written to internal \
                 storage and reach the flow as `trigger.parts`"
            ),
            Self::Wait => format!(
                "webhook trigger '{trigger_id}' declares wait: true, which egret does not \
                 implement yet -- the response would have to be held open until the execution \
                 terminates (roadmap package #11)"
            ),
            Self::ReturnOutputs => format!(
                "webhook trigger '{trigger_id}' declares returnOutputs: true, which egret does \
                 not implement yet -- it needs the same held-open response wait: true does \
                 (roadmap package #11)"
            ),
            Self::ResponseContentType => format!(
                "webhook trigger '{trigger_id}' declares responseContentType, which egret does \
                 not implement yet -- it describes the body of a held-open response \
                 (roadmap package #11)"
            ),
            Self::ResponseCode => format!(
                "webhook trigger '{trigger_id}' declares responseCode, which egret does not \
                 implement yet -- it is the status of a held-open response \
                 (roadmap package #11)"
            ),
        }
    }
}

/// Everything the key template may read, and nothing else.
///
/// Kestra builds `runContextFactory.of(flow, trigger)`. Measured against the
/// artifact, that context resolves `flow.id`, `flow.namespace`,
/// `flow.revision`, `vars.*` and `trigger.id` — all four probes answered 200.
///
/// `flow.revision` is **absent here and that is a pre-existing gap, not a
/// choice made in this task**: egret's flow model has no revision at all
/// (`worker::render::Surface` publishes `flow` as `{id, namespace}` for the
/// same reason). A key declaring `{{ flow.revision }}` therefore fails to
/// render, which puts it on the "trigger ignored" arm rather than making it
/// match something wrong.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyContext {
    pub flow_id: String,
    pub namespace: String,
    pub vars: BTreeMap<String, Value>,
}

impl KeyContext {
    /// The context a *specific* trigger's key renders against.
    ///
    /// Per-trigger rather than per-flow because `trigger.id` is in it. Two
    /// triggers in one flow therefore get two different contexts, and a key of
    /// `"{{ trigger.id }}"` means a different string in each — which is the
    /// artifact's behaviour (`key: "tid-{{ trigger.id }}"` matched
    /// `tid-trg-tid`).
    ///
    /// **This `trigger` root is not the one the module doc says is withheld.**
    /// That one is `worker::render::Surface`'s, seen by tasks.
    ///
    /// This one is local to the key comparison, and that is an invariant about
    /// its CALLERS rather than about its contents: it was violated the day it
    /// was written, because `render_inputs` used it too and a
    /// `{{ trigger }}` input therefore reached an execution (fix round 1,
    /// Critical 2). `inputs:` now render against [`InputContext`], which binds
    /// no `trigger`, and `tests::the_key_context_is_reachable_from_no_other_render`
    /// counts the callers so the invariant cannot quietly acquire a second one.
    pub fn render_context_for(&self, trigger: &Trigger) -> RenderContext {
        let mut cx = RenderContext::new();
        cx.insert(
            "flow",
            json!({"id": self.flow_id, "namespace": self.namespace}),
        );
        cx.insert(
            "vars",
            Value::Object(
                self.vars
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        );
        cx.insert("trigger", json!({"id": trigger.id, "type": trigger.r#type}));
        cx
    }
}

/// Kestra's `MessageDigest.isEqual`, ported behaviour-for-behaviour.
///
/// ```text
/// if (digesta == digestb) return true;
/// if (digesta == null || digestb == null) return false;
/// int lenA = digesta.length, lenB = digestb.length;
/// if (lenB == 0) return lenA == 0;
/// int result = 0;
/// result |= lenA - lenB;
/// for (int i = 0; i < lenA; i++) {
///     int indexB = ((i - lenB) >>> 31) * i;
///     result |= digesta[i] ^ digestb[indexB];
/// }
/// return result == 0;
/// ```
///
/// The `indexB` trick clamps out-of-range reads to index 0 so the loop always
/// runs `lenA` times whatever `lenB` is. This port keeps that: the running
/// time depends on the length of the **declared** key and on nothing the
/// caller supplies except its own length, and no byte comparison short-circuits.
///
/// **Why it is not `a == b`.** The key is a bearer credential on an anonymous
/// endpoint, so a comparison that returns early on the first differing byte
/// leaks the key one byte at a time to anyone who can time the response. That
/// is not a hypothetical class for this repository's threat model — it is the
/// reason Kestra reached for `MessageDigest.isEqual` rather than
/// `String.equals` in the one place it compares a webhook key.
/// **A behavioural test cannot see the difference, and a mutation table cannot
/// either** — measured, not argued: replacing this function's body with
/// `a == b` (mutation M1) leaves all 27 unit tests and all 22 route tests
/// green. The only instrument that sees it is one that reads the source, so
/// [`tests::the_key_is_never_compared_with_a_plain_equality`] is that
/// instrument, and it pins this body's SHAPE as well as forbidding an equality
/// at the call sites. The first version of it forbade only the call sites and
/// M1 walked straight through; the assertion below is the correction.
#[must_use]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if b.is_empty() {
        return a.is_empty();
    }
    // `lenA - lenB` as a 64-bit OR, so a length difference can never cancel
    // against a byte difference the way an 8-bit accumulator could.
    let mut result: u64 = (a.len() as i64 - b.len() as i64) as u64;
    for (i, byte) in a.iter().enumerate() {
        let index_b = if i < b.len() { i } else { 0 };
        result |= u64::from(byte ^ b[index_b]);
    }
    result == 0
}

/// Does a **rendered** declared key match the key taken from the URL?
///
/// The trim is one-sided on purpose, and that is the measurement this whole
/// function exists to hold: Kestra trims the declared key (`render(...).trim()`)
/// and hands the URL key to `getBytes` untouched.
///
/// * declared `"  spaced-key  "` + URL `"spaced-key"` → **true** (measured 200)
/// * declared `"  spaced-key  "` + URL `"  spaced-key  "` → **false** (measured 404)
#[must_use]
pub fn keys_match(declared_rendered: &str, url_key: &str) -> bool {
    ct_eq(declared_rendered.trim().as_bytes(), url_key.as_bytes())
}

/// Render one trigger's declared key, or `None` if it cannot be rendered.
///
/// `None` is *ignore this trigger*, never *fail the request*. Kestra:
///
/// ```text
/// } catch (IllegalVariableEvaluationException e) {
///     log.warn("Unable to render the webhook key {}, the webhook will be ignored", key, e);
///     return false;
/// }
/// ```
///
/// A trigger with no `key:` at all is also `None` — `getKey()` would be null
/// and `render(null)` throws — so it can never be matched by an empty URL
/// segment.
#[must_use]
pub fn render_key(trigger: &Trigger, webhook: &Webhook, kcx: &KeyContext) -> Option<String> {
    let source = webhook.key.as_ref()?.source();
    render_str(source, &kcx.render_context_for(trigger)).ok()
}

/// Find the webhook trigger a URL key names, in declaration order.
///
/// `findFirst()` in Kestra, over `flow.getTriggers()` filtered to
/// `instanceof AbstractWebhookTrigger`. **Declaration order matters and is
/// tested**: a trigger whose key will not render does not stop the scan, so a
/// later trigger is still reachable past it (measured — a flow whose first
/// trigger declared `{{ nosuchroot.nothing }}` still answered 200 on its third
/// trigger's key).
///
/// `disabled:` is deliberately **not** filtered here. Kestra matches a disabled
/// trigger and then refuses it with a 409 naming it
/// (`Cannot execute webhook: the trigger 'offhook' is disabled.`, measured), so
/// filtering it out here would turn that 409 into a 404 and lose the operator's
/// only clue. The refusal is the caller's job — see
/// [`crate::webhook_routes`].
#[must_use]
pub fn find<'a>(
    triggers: &'a [Trigger],
    url_key: &str,
    kcx: &KeyContext,
) -> Option<(&'a Trigger, &'a Webhook)> {
    triggers.iter().find_map(|t| {
        let TriggerKind::Webhook(w) = &t.kind else {
            return None;
        };
        let rendered = render_key(t, w, kcx)?;
        keys_match(&rendered, url_key).then_some((t, w))
    })
}

/// Is this content type "textual" in Kestra's sense?
///
/// `MicronautHttpService.isTextual`:
///
/// ```text
/// mediaType == null
///   || mediaType.isTextBased()
///   || mediaType == MediaType.APPLICATION_FORM_URLENCODED_TYPE
/// ```
///
/// Micronaut's `MediaType.isTextBased()` is true for a `text/*` type, and for
/// a type whose subtype ends in `json` or `xml` (`application/json`,
/// `application/ld+json`, `application/xml`, `image/svg+xml`). Reproduced
/// rather than approximated, because getting it wrong turns a JSON body into a
/// base64 string.
#[must_use]
pub fn is_textual(content_type: Option<&str>) -> bool {
    let Some(ct) = content_type else {
        return true;
    };
    // Strip parameters (`application/json; charset=utf-8`) and normalise.
    let ct = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if ct.is_empty() {
        return true;
    }
    if ct == "application/x-www-form-urlencoded" {
        return true;
    }
    let Some((kind, subtype)) = ct.split_once('/') else {
        return false;
    };
    kind == "text" || subtype.ends_with("json") || subtype.ends_with("xml")
}

/// `Webhook.tryMap(s).or(() -> tryArray(s)).orElse(s)`.
///
/// An object first, then an array, then the raw string. Note the order is
/// observable: it is why `[1,2]` arrives as an array and `just plain text`
/// arrives as a string rather than as a parse error.
///
/// A bare JSON scalar (`3`, `true`, `"x"`) is **neither** a map nor an array,
/// so it stays the raw string — which is what the two `tryX` helpers do and
/// not what a single `serde_json::from_str::<Value>` would do.
#[must_use]
fn parse_text_body(text: &str) -> Value {
    if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(text) {
        return Value::Object(m);
    }
    if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(text) {
        return Value::Array(a);
    }
    Value::String(text.to_string())
}

/// Group `(name, value)` pairs into `{name: [values…]}`, preserving the order
/// two values of one name arrived in.
///
/// `Collectors.groupingBy(NameValuePair::getName,
/// Collectors.mapping(NameValuePair::getValue, Collectors.toList()))`.
fn group(pairs: &[(String, String)]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (k, v) in pairs {
        out.entry(k.clone()).or_default().push(v.clone());
    }
    out
}

/// Build `trigger.variables` from the request.
///
/// Returns `Err(Unsupported)` for the shapes this port refuses rather than
/// approximates. `stored_uri` is always `None` today — it is the parameter
/// `fetchType: STORE` would fill, kept so the refusal has an obvious place to
/// be deleted from when someone implements it.
pub fn build_output(
    req: &WebhookRequest,
    fetch: FetchType,
    stored_uri: Option<&str>,
) -> Result<Output, Unsupported> {
    let mut out = Output {
        headers: Some(group(&req.headers)),
        parameters: Some(group(&req.query)),
        ..Output::default()
    };

    if is_multipart(req.content_type.as_deref()) {
        return Err(Unsupported::Multipart);
    }

    // `if (storedBodyUri != null) return output.uri(...).build();` — the URI
    // branch returns BEFORE the body switch, so a STORE webhook never carries
    // both.
    if let Some(uri) = stored_uri {
        out.uri = Some(uri.to_string());
        return Ok(out);
    }
    match fetch {
        FetchType::Store => return Err(Unsupported::StoreBody),
        // `WebhookBodyService.read` hands `null` back for NONE, so the switch
        // falls through the `case null` arm and no body is set.
        FetchType::None => return Ok(out),
        FetchType::Fetch => {}
    }

    // `if (content.length == 0) return null;` — a zero-length body is not an
    // empty string, it is the absence of a body, and `body` is then absent.
    if req.body.is_empty() {
        return Ok(out);
    }

    out.body = Some(if is_textual(req.content_type.as_deref()) {
        // `new String(bytes, charset)` — Java replaces malformed input rather
        // than failing, which `from_utf8_lossy` also does.
        parse_text_body(&String::from_utf8_lossy(&req.body))
    } else {
        use base64::Engine as _;
        Value::String(base64::engine::general_purpose::STANDARD.encode(&req.body))
    });
    Ok(out)
}

/// Is this a `multipart/form-data` request?
///
/// Separate from [`is_textual`] because Micronaut routes multipart to a
/// different controller method entirely (`triggerExecutionByPostWebhookMultipart`),
/// so it is not a body-typing question but a routing one.
#[must_use]
fn is_multipart(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|ct| {
        ct.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("multipart/form-data")
    })
}

/// Why a webhook's `inputs:` may not be refused by looking at its TEXT.
///
/// Fix round 1, Critical 2. The first version of this module decided whether a
/// trigger's `inputs:` read `trigger.*` by scanning each value for the
/// substrings `"trigger."` and `"trigger["`, and rendered everything else
/// against [`KeyContext::render_context_for`] — the context the KEY comparison
/// uses, which binds `trigger` to `{id, type}`.
///
/// **That shipped the leak the guard was written to prevent.**
/// `inputs: {x: "{{ trigger }}"}` contains neither substring, so it passed the
/// scan, rendered against the key context, and fed the execution the key
/// comparison's own private object — falsifying this module's claim that "no
/// task can reach it". `{{ trigger | json }}` did the same. A substring scan is
/// a lexical test applied to an expression language, and an expression language
/// has arbitrarily many ways to name a binding.
///
/// The replacement is not a better scan. **`inputs:` is rendered against a
/// context in which `trigger` is not bound at all** — this type — so the object
/// is unreachable rather than merely un-asked-for: there is no spelling,
/// filter, index or alias that can reach a binding the environment does not
/// contain.
///
/// # Two questions, and they are not the same question
///
/// 1. **Can an input reach the object?** No, and that is this type: the name is
///    not bound. It is a property of the environment, not of any check.
/// 2. **Should an input that mentions `trigger` be accepted at all?** No, and
///    that is [`crate::runners::render_str_guarded`] with
///    [`FORBIDDEN_INPUT_ROOTS`], asked **before** every pass of the render.
///
/// Keeping them apart is fix round 2's Important 1. Closing (1) alone left
/// `{{ [trigger] }}`, `{{ trigger ?? 'x' }}`, `{{ trigger | default('x') }}`
/// and `{{ trigger is defined }}` **rendering** — to `[null]`, `'x'`, `'x'` and
/// `false` — so they were accepted with a value the author never asked for.
/// Nothing leaked; the acceptance was still wrong.
///
/// # There is no sentinel
///
/// This doc described "a sentinel `trigger` binding" as the live discriminator
/// until fix round 2's claim 15. **That approach was tried and rejected** — it
/// misclassifies `{{ trigger.body }}`, because a sentinel object has no `body`
/// member either — and no sentinel has ever existed in this file. The
/// discriminator is [`crate::runners::render_str_guarded`], which reads
/// minijinja's AST — see [`FORBIDDEN_INPUT_ROOTS`].
#[derive(Debug, Clone, PartialEq)]
pub struct InputContext {
    pub flow_id: String,
    pub namespace: String,
    pub vars: BTreeMap<String, Value>,
}

impl InputContext {
    #[must_use]
    pub fn of(kcx: &KeyContext) -> Self {
        Self {
            flow_id: kcx.flow_id.clone(),
            namespace: kcx.namespace.clone(),
            vars: kcx.vars.clone(),
        }
    }

    /// The context an `inputs:` value renders against.
    ///
    /// `flow` and `vars`, and **deliberately no `trigger`** — see
    /// [`InputContext`]. `tests::the_input_context_binds_no_trigger` is a
    /// running check of that, and it asks the renderer rather than the map.
    #[must_use]
    pub fn render_context(&self) -> RenderContext {
        let mut cx = RenderContext::new();
        cx.insert(
            "flow",
            json!({"id": self.flow_id, "namespace": self.namespace}),
        );
        cx.insert(
            "vars",
            Value::Object(
                self.vars
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        );
        cx
    }
}

/// Why one `inputs:` entry could not be rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputRefusal {
    /// It needs the `trigger` root, which egret stores on the execution but
    /// does not publish into the render surface (W6 Task 8). A **capability
    /// gap**: the flow is well-formed and Kestra would answer 200.
    NeedsTriggerContext { key: String },
    /// It fails for some other reason. The author's template is wrong.
    Unrenderable { key: String, error: String },
}

impl InputRefusal {
    #[must_use]
    pub fn message(&self, trigger_id: &str) -> String {
        match self {
            Self::NeedsTriggerContext { key } => format!(
                "webhook trigger '{trigger_id}' declares input '{key}' whose value needs the \
                 `trigger` render root, which egret does not publish yet -- the trigger record \
                 is stored on the execution but no task can read it"
            ),
            Self::Unrenderable { key, error } => {
                format!("webhook trigger '{trigger_id}' input '{key}': {error}")
            }
        }
    }
}

/// Does `template` refer to the `trigger` root, in **any** pass of the render?
///
/// [`crate::runners::render_str_guarded`] asks
/// [`crate::runners::referenced_roots`] — minijinja's own AST — of every text
/// the fixpoint loop is about to evaluate, not only of the source. See that
/// function for why one pass is not enough.
///
/// It is **not** the security boundary. That is [`InputContext`] simply not
/// binding the name, which is what makes the object unreachable by any spelling
/// at all. This decides *acceptance*, and a false negative here is an
/// over-acceptance rather than a leak.
const FORBIDDEN_INPUT_ROOTS: &[&str] = &["trigger"];

/// Render one declared value the way the artifact renders it.
///
/// `pub(crate)` because a Flow trigger's `inputs:` are rendered by the same
/// `runContext.render(Map)` (W13a Task 4, `flow_trigger::process`), against the
/// SOURCE execution's context rather than an `InputContext`.
///
/// `VariableRenderer.renderObject(Object, Map, boolean)`, measured from the
/// bytecode of the pinned image rather than from a signature — see
/// [`render_inputs`] for the table and for the sentence this replaced.
pub(crate) fn render_value(
    v: &Value,
    cx: &RenderContext,
    path: &str,
) -> Result<Value, InputRefusal> {
    match v {
        // `instanceof String` -> render.
        Value::String(text) => match render_str_guarded(text, cx, FORBIDDEN_INPUT_ROOTS) {
            Ok(GuardedRender::Rendered(out)) => Ok(Value::String(out)),
            Ok(GuardedRender::Forbidden(_)) => Err(InputRefusal::NeedsTriggerContext {
                key: path.to_string(),
            }),
            Err(e) => Err(InputRefusal::Unrenderable {
                key: path.to_string(),
                error: e.to_string(),
            }),
        },
        // `instanceof List` -> renderList, which is renderObject per element.
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                out.push(render_value(item, cx, &format!("{path}[{i}]"))?);
            }
            Ok(Value::Array(out))
        }
        // `instanceof Map` -> render(Map, Map, Z), which renders the KEY and
        // renderObject's the value.
        Value::Object(map) => {
            let mut out = Map::new();
            // Sorted rather than source order, and that is a deliberate,
            // recorded divergence -- see [`render_inputs`].
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                let here = if path.is_empty() {
                    k.to_string()
                } else {
                    format!("{path}.{k}")
                };
                let rendered_key = match render_str_guarded(k, cx, FORBIDDEN_INPUT_ROOTS) {
                    Ok(GuardedRender::Rendered(out)) => out,
                    Ok(GuardedRender::Forbidden(_)) => {
                        return Err(InputRefusal::NeedsTriggerContext {
                            key: format!("{here} (the key itself)"),
                        })
                    }
                    Err(e) => {
                        return Err(InputRefusal::Unrenderable {
                            key: format!("{here} (the key itself)"),
                            error: e.to_string(),
                        })
                    }
                };
                let value = render_value(&map[k], cx, &here)?;
                // `putIfAbsent`: the first key to render to a given string
                // wins, and the later one is DROPPED rather than overwriting.
                out.entry(rendered_key).or_insert(value);
            }
            Ok(Value::Object(out))
        }
        // Everything else is returned unchanged -- the `else` arm of
        // `renderObject`.
        other => Ok(other.clone()),
    }
}

/// The `inputs:` a matched webhook trigger supplies to the execution it starts.
///
/// `WebhookService.newExecution`:
///
/// ```text
/// if (webhook.getInputs() != null) {
///     Map<String, Object> inputs = runContext.render(webhook.getInputs());
///     inputs = readExecutionInputs(flow, execution, inputs);
///     execution = execution.withInputs(inputs);
/// }
/// ```
///
/// with a `catch (Exception e)` that logs `Unable to render the webhook inputs`
/// and throws `WebhookInputRenderException` — measured as an HTTP **422**.
///
/// **The `runContext` Kestra renders against is the EXECUTION's**, i.e. the one
/// that already carries `trigger.*` fully populated. egret does not publish
/// that root, so a trigger declaring `inputs: {body: "{{ trigger.body }}"}` —
/// which is `valids/webhook-inputs`, the corpus flow `harvest.py` names in its
/// own exclusion reason — cannot be honoured. Kestra answers **200**; egret
/// answers **409**, which is under-acceptance and therefore the safe direction.
///
/// # What `runContext.render(Map)` actually does
///
/// This doc used to say *"Non-string values pass through untouched:
/// `runContext.render(Map)` renders strings and leaves everything else
/// alone."* **That is false, and it is stated backwards** — fix round 3's
/// claim 16. It was read off a signature; the behaviour is the opposite, and
/// the false sentence is why nobody looked at the nested case, which is fix
/// round 3's Importants 5 and 6.
///
/// Measured from `javap -c io.kestra.core.runners.VariableRenderer` on the
/// pinned image. `renderObject(Object, Map, boolean)` is a four-arm instanceof:
///
/// | arm | what happens |
/// |---|---|
/// | `Map` | `render(Map, Map, Z)` — **recurses**, and **renders the KEY** with `render(String, Map)` |
/// | `List` | `renderList` — `renderObject` per element, **recurses** |
/// | `Set` | `render(Set, Map, Z)` — `render(String, …)` per element |
/// | `String` | `render(Object, Map, Z, true)` — the **typed** render |
/// | anything else | `Optional.ofNullable(o)` — returned unchanged |
///
/// and `render(Map, Map, Z)` builds a **`LinkedHashMap`** and inserts with
/// **`putIfAbsent`**, so when two keys render to the same string the **first in
/// iteration order wins** and the second is dropped.
///
/// [`render_value`] reproduces the `Map`, `List` and `String` arms and the
/// `putIfAbsent` rule. **Three divergences, named rather than found:**
///
/// 1. **`Set` has no JSON counterpart**, so there is no arm for it. A YAML
///    sequence parses to `Value::Array` and takes the `List` arm, which is what
///    Kestra does with a `List` too.
/// 2. **The typed String render is not reproduced.** Kestra's String arm passes
///    `true` for `tryParseJson`, so a nested string that renders to `{"a":1}`
///    becomes a `Map`. egret keeps it a `Value::String`. Downstream,
///    `worker::render::Surface::load` applies the flow's declared input types
///    to whatever is stored, so the coercion happens later rather than never —
///    but it happens against the *declared* type, not against the rendered
///    text's shape, and those differ for an input declared `STRING` whose value
///    renders to JSON.
/// 3. **Key order is sorted, not source order.** Kestra iterates a
///    `LinkedHashMap` in YAML order, so its `putIfAbsent` winner is the one
///    written first. `models::Webhook::inputs` is a `HashMap`, so egret has no
///    source order to iterate — it would be *random per process*. Sorting makes
///    the winner deterministic, which is strictly better than random and still
///    not Kestra's. It is observable only when two keys render to one string.
///
/// # The check runs on every pass, not on the source
///
/// See [`crate::runners::render_str_guarded`]. Asking
/// [`crate::runners::referenced_roots`] of the declared text alone asks about
/// pass 1, and `vars.tpl = "{{ [trigger] }}"` with `a: "{{ vars.tpl }}"`
/// reaches the root on pass 2.
pub fn render_inputs(webhook: &Webhook, icx: &InputContext) -> Result<Value, InputRefusal> {
    let Some(declared) = webhook.inputs.as_ref() else {
        return Ok(Value::Object(Map::new()));
    };
    let as_value = Value::Object(
        declared
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    render_value(&as_value, &icx.render_context(), "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::parse;

    /// Spec §5: the four refusals a parity gap causes name roadmap package #11;
    /// the two that are a deployment's configuration do not.
    #[test]
    fn the_unsupported_messages_name_the_package_that_lifts_them() {
        assert_eq!(
            Unsupported::Wait.message("w"),
            "webhook trigger 'w' declares wait: true, which egret does not implement yet -- \
             the response would have to be held open until the execution terminates \
             (roadmap package #11)"
        );
        assert_eq!(
            Unsupported::ReturnOutputs.message("w"),
            "webhook trigger 'w' declares returnOutputs: true, which egret does not implement \
             yet -- it needs the same held-open response wait: true does (roadmap package #11)"
        );
        assert_eq!(
            Unsupported::ResponseContentType.message("w"),
            "webhook trigger 'w' declares responseContentType, which egret does not implement \
             yet -- it describes the body of a held-open response (roadmap package #11)"
        );
        assert_eq!(
            Unsupported::ResponseCode.message("w"),
            "webhook trigger 'w' declares responseCode, which egret does not implement yet -- \
             it is the status of a held-open response (roadmap package #11)"
        );
        for u in [Unsupported::StoreBody, Unsupported::Multipart] {
            assert!(!u.message("w").contains("roadmap package"), "{u:?}");
        }
    }

    fn flow_of(yaml: &str) -> crate::models::Flow {
        parse(yaml).unwrap_or_else(|e| panic!("fixture does not parse: {}", e.message))
    }

    fn kcx_of(f: &crate::models::Flow) -> KeyContext {
        KeyContext {
            flow_id: f.id.clone(),
            namespace: f.namespace.clone(),
            vars: f.variables.clone().into_iter().collect(),
        }
    }

    /// The fixture the whole module is calibrated on.
    ///
    /// **Every field is distinct from every other**, which is the point: the
    /// flow id, the namespace, each trigger id and each key are five different
    /// strings, so a mutation that reads the wrong one of them cannot survive
    /// by coincidence. That is the operand problem this branch has paid for
    /// twice — a fixture whose `max_depth` happened to equal the default made
    /// a mutation to the literal `100` invisible.
    const FIXTURE: &str = "\
id: fx-flow-id
namespace: fx.ns
variables:
  vkey: fx-var-value
tasks:
  - id: fx-task
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: fx-trg-broken
    type: io.kestra.plugin.core.trigger.Webhook
    key: \"{{ nosuchroot.nothing }}\"
  - id: fx-trg-literal
    type: io.kestra.plugin.core.trigger.Webhook
    key: fx-literal-key
  - id: fx-trg-dynamic
    type: io.kestra.plugin.core.trigger.Webhook
    key: \"dyn-{{ flow.id }}\"
  - id: fx-trg-vars
    type: io.kestra.plugin.core.trigger.Webhook
    key: \"{{ vars.vkey }}\"
  - id: fx-trg-tid
    type: io.kestra.plugin.core.trigger.Webhook
    key: \"tid-{{ trigger.id }}\"
  - id: fx-trg-spaced
    type: io.kestra.plugin.core.trigger.Webhook
    key: \"  fx-spaced-key  \"
  - id: fx-trg-schedule
    type: io.kestra.plugin.core.trigger.Schedule
    cron: \"* * * * *\"
";

    // ------------------------------------------------------------------
    // ct_eq: the comparison itself
    // ------------------------------------------------------------------

    /// The four `MessageDigest.isEqual` arms, each with its own row.
    #[test]
    fn ct_eq_reproduces_message_digest_is_equal() {
        assert!(ct_eq(b"", b""), "two empties are equal");
        assert!(!ct_eq(b"a", b""), "non-empty vs empty");
        assert!(!ct_eq(b"", b"a"), "empty vs non-empty");
        assert!(ct_eq(b"abc", b"abc"), "identical");
        assert!(!ct_eq(b"abc", b"abd"), "last byte differs");
        assert!(!ct_eq(b"abc", b"bbc"), "first byte differs");
        assert!(!ct_eq(b"abc", b"abcd"), "a is a prefix of b");
        assert!(!ct_eq(b"abcd", b"abc"), "b is a prefix of a");
        assert!(!ct_eq(b"ABC", b"abc"), "case is significant");
    }

    /// **The control for the accumulator.** A 64-bit OR is used rather than a
    /// `u8` so that a length difference cannot cancel against a byte
    /// difference. This is the pair that would pass under a `u8` accumulator
    /// if the length term were dropped, and it is here so that the width is
    /// asserted rather than assumed.
    #[test]
    fn a_length_difference_cannot_cancel_a_byte_difference() {
        // `lenA - lenB` = 1; byte 0 differs by 1 as well. Under a narrow
        // accumulator that ORed a truncated difference these would still be
        // caught (OR never cancels) -- so the real property is that BOTH
        // signals are present, and each alone is enough.
        assert!(!ct_eq(b"b", b"a"), "same length, byte differs");
        assert!(!ct_eq(b"ab", b"a"), "length differs, prefix matches");
        assert!(!ct_eq(b"bb", b"a"), "both differ");
    }

    /// Non-UTF-8 bytes are compared as bytes, not as strings.
    #[test]
    fn ct_eq_is_over_bytes() {
        assert!(ct_eq(&[0xff, 0x00], &[0xff, 0x00]));
        assert!(!ct_eq(&[0xff, 0x00], &[0xff, 0x01]));
    }

    // ------------------------------------------------------------------
    // keys_match: the one-sided trim
    // ------------------------------------------------------------------

    /// **The discriminating row.** Two rules agree on every ordinary key:
    ///
    /// * R1 — trim the declared key only (Kestra's, measured);
    /// * R2 — trim both sides.
    ///
    /// They disagree on exactly one input, and this is it. Measured on the
    /// artifact in both directions: `…/spaced-key` → 200,
    /// `…/%20%20spaced-key%20%20` → 404.
    #[test]
    fn only_the_declared_key_is_trimmed() {
        assert!(
            keys_match("  spaced-key  ", "spaced-key"),
            "R1 and R2 agree: the declared key trims"
        );
        assert!(
            !keys_match("  spaced-key  ", "  spaced-key  "),
            "R1 says no, R2 says yes -- the artifact says 404"
        );
        // And the third rule the first row alone does not kill: no trim at all.
        assert!(
            !keys_match("spaced-key", "  spaced-key  "),
            "R3 (no trim anywhere) would also reject this -- kept as the control \
             that the first row is what separates R1 from R3"
        );
    }

    /// A key of only whitespace trims to empty, and an empty URL segment is
    /// not routable, so it can never match.
    #[test]
    fn a_whitespace_only_key_matches_nothing_routable() {
        assert!(keys_match("   ", ""), "it does equal the empty string");
        assert!(!keys_match("   ", " "), "and nothing a URL can carry");
    }

    #[test]
    fn keys_match_is_not_a_prefix_test() {
        assert!(!keys_match("key", "keyX"));
        assert!(!keys_match("key", "ke"));
        assert!(!keys_match("keyX", "key"));
    }

    #[test]
    fn keys_match_is_case_sensitive() {
        assert!(!keys_match("Key", "key"));
        assert!(!keys_match("key", "KEY"));
    }

    // ------------------------------------------------------------------
    // render_key / find
    // ------------------------------------------------------------------

    /// **The row that kills "compare the declared source".** The literal text
    /// of the template must NOT match; the rendered value must. Measured on
    /// the artifact in both directions (200 for `t7-probe-multi`, 404 for the
    /// percent-encoded `{{ flow.id }}`).
    #[test]
    fn the_rendered_key_is_what_is_compared_not_the_source() {
        let f = flow_of(FIXTURE);
        let kcx = kcx_of(&f);

        let hit = find(&f.triggers, "dyn-fx-flow-id", &kcx).expect("the rendered key matches");
        assert_eq!(hit.0.id, "fx-trg-dynamic");

        assert!(
            find(&f.triggers, "dyn-{{ flow.id }}", &kcx).is_none(),
            "the SOURCE must not match -- this is the mutation `render_key` exists to fail"
        );
    }

    /// The three roots the artifact was measured resolving, each with a key
    /// whose rendered value is distinct from the flow id, the namespace and
    /// the trigger id.
    #[test]
    fn the_key_context_carries_flow_vars_and_trigger() {
        let f = flow_of(FIXTURE);
        let kcx = kcx_of(&f);

        assert_eq!(
            find(&f.triggers, "dyn-fx-flow-id", &kcx).map(|(t, _)| t.id.as_str()),
            Some("fx-trg-dynamic"),
            "flow.id"
        );
        assert_eq!(
            find(&f.triggers, "fx-var-value", &kcx).map(|(t, _)| t.id.as_str()),
            Some("fx-trg-vars"),
            "vars.vkey"
        );
        assert_eq!(
            find(&f.triggers, "tid-fx-trg-tid", &kcx).map(|(t, _)| t.id.as_str()),
            Some("fx-trg-tid"),
            "trigger.id -- and note it is the PER-TRIGGER id, not the first one"
        );
    }

    /// `trigger.id` is per-trigger, which is only observable with two triggers
    /// that both read it. Without this row, a context built once per flow and
    /// pinned to `triggers[0]` would pass every other test in this file.
    #[test]
    fn the_trigger_root_is_rebuilt_for_each_trigger() {
        let yaml = "\
id: two-tid
namespace: fx.ns
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: alpha
    type: io.kestra.plugin.core.trigger.Webhook
    key: \"k-{{ trigger.id }}\"
  - id: beta
    type: io.kestra.plugin.core.trigger.Webhook
    key: \"k-{{ trigger.id }}\"
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        assert_eq!(
            find(&f.triggers, "k-alpha", &kcx).map(|(t, _)| t.id.as_str()),
            Some("alpha")
        );
        assert_eq!(
            find(&f.triggers, "k-beta", &kcx).map(|(t, _)| t.id.as_str()),
            Some("beta"),
            "a context pinned to triggers[0] would answer None here"
        );
    }

    /// The artifact logs a warning and returns `false`; it does not fail the
    /// request. The proof that the scan CONTINUES is that a trigger declared
    /// *after* the broken one is still reachable — `fx-trg-broken` is
    /// deliberately first in the fixture.
    #[test]
    fn an_unrenderable_key_hides_its_trigger_and_does_not_stop_the_scan() {
        let f = flow_of(FIXTURE);
        let kcx = kcx_of(&f);

        let (broken, w) = f
            .triggers
            .iter()
            .find_map(|t| match (&t.kind, t.id.as_str()) {
                (TriggerKind::Webhook(w), "fx-trg-broken") => Some((t, w)),
                _ => None,
            })
            .expect("fixture carries the broken trigger");
        assert_eq!(
            render_key(broken, w, &kcx),
            None,
            "an unresolvable root does not render"
        );
        assert_eq!(
            f.triggers.iter().position(|t| t.id == "fx-trg-broken"),
            Some(0),
            "and it is FIRST, so the next assertion proves the scan continued"
        );
        assert_eq!(
            find(&f.triggers, "fx-literal-key", &kcx).map(|(t, _)| t.id.as_str()),
            Some("fx-trg-literal")
        );
    }

    /// A `Schedule` in the same `triggers:` list is skipped, not matched and
    /// not an error. The fixture carries one.
    #[test]
    fn a_non_webhook_trigger_is_never_matched() {
        let f = flow_of(FIXTURE);
        let kcx = kcx_of(&f);
        assert!(f
            .triggers
            .iter()
            .any(|t| matches!(t.kind, TriggerKind::Schedule(_))));
        assert!(find(&f.triggers, "* * * * *", &kcx).is_none());
        assert!(find(&f.triggers, "fx-trg-schedule", &kcx).is_none());
    }

    /// A trigger with no `key:` at all can never be matched — in particular
    /// not by an empty key.
    #[test]
    fn a_webhook_with_no_key_matches_nothing() {
        let yaml = "\
id: nokey
namespace: fx.ns
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: keyless
    type: io.kestra.plugin.core.trigger.Webhook
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        assert!(find(&f.triggers, "", &kcx).is_none());
        assert!(find(&f.triggers, "keyless", &kcx).is_none());
    }

    /// Declaration order decides, and the fixture for it has **two triggers
    /// with the same key**, which is the only way the rule is observable.
    #[test]
    fn the_first_matching_trigger_in_declaration_order_wins() {
        let yaml = "\
id: dup
namespace: fx.ns
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: first
    type: io.kestra.plugin.core.trigger.Webhook
    key: shared
  - id: second
    type: io.kestra.plugin.core.trigger.Webhook
    key: shared
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        assert_eq!(
            find(&f.triggers, "shared", &kcx).map(|(t, _)| t.id.as_str()),
            Some("first")
        );
    }

    /// A disabled trigger is still FOUND. It is refused one layer up, with a
    /// 409 that names it — filtering it here would turn that into a 404.
    #[test]
    fn a_disabled_trigger_is_still_matched_here() {
        let yaml = "\
id: off
namespace: fx.ns
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: offhook
    type: io.kestra.plugin.core.trigger.Webhook
    key: offkey
    disabled: true
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        let (t, _) = find(&f.triggers, "offkey", &kcx).expect("matched despite being disabled");
        assert!(t.disabled);
    }

    // ------------------------------------------------------------------
    // build_output
    // ------------------------------------------------------------------

    fn req(ct: Option<&str>, body: &[u8]) -> WebhookRequest {
        WebhookRequest {
            query: vec![],
            headers: vec![("content-type".into(), ct.unwrap_or("").into())],
            content_type: ct.map(str::to_string),
            body: body.to_vec(),
        }
    }

    /// The five body shapes, each measured on the artifact and each with the
    /// value a *different* rule would have produced named in the message.
    #[test]
    fn the_body_is_typed_the_way_the_artifact_types_it() {
        let o = build_output(
            &req(Some("application/json"), br#"{"hello":"world","n":3}"#),
            FetchType::Fetch,
            None,
        )
        .unwrap();
        assert_eq!(
            o.body,
            Some(json!({"hello":"world","n":3})),
            "an object, not a string"
        );

        let o = build_output(
            &req(Some("application/json"), b"[1,2]"),
            FetchType::Fetch,
            None,
        )
        .unwrap();
        assert_eq!(o.body, Some(json!([1, 2])), "an array, not a string");

        let o = build_output(
            &req(Some("text/plain"), b"just plain text"),
            FetchType::Fetch,
            None,
        )
        .unwrap();
        assert_eq!(
            o.body,
            Some(json!("just plain text")),
            "a string, not a parse error"
        );

        // A bare JSON scalar is neither a map nor an array: `tryMap` and
        // `tryArray` both miss, so it stays the RAW STRING. A single
        // `from_str::<Value>` would have made this the number 3.
        let o = build_output(&req(Some("application/json"), b"3"), FetchType::Fetch, None).unwrap();
        assert_eq!(o.body, Some(json!("3")), "a bare scalar stays a string");

        // Non-textual: base64.
        let o = build_output(
            &req(Some("application/octet-stream"), &[0x00, 0xff]),
            FetchType::Fetch,
            None,
        )
        .unwrap();
        assert_eq!(o.body, Some(json!("AP8=")), "base64, not lossy UTF-8");
    }

    /// An absent body is not an empty body, and `body` is then absent
    /// entirely. Measured: a `GET` webhook's `trigger.variables` has no `body`
    /// key at all.
    #[test]
    fn an_empty_body_produces_no_body_key() {
        let o = build_output(&req(Some("application/json"), b""), FetchType::Fetch, None).unwrap();
        assert_eq!(o.body, None);
        // The control: one byte of body, same content type, and the key
        // appears. Without this row the assertion above would also pass for an
        // implementation that never set `body` at all.
        let o = build_output(&req(Some("application/json"), b"x"), FetchType::Fetch, None).unwrap();
        assert_eq!(o.body, Some(json!("x")));
    }

    /// `fetchType` decides whether there is a body at all, and the three
    /// values are three different answers.
    #[test]
    fn fetch_type_none_carries_headers_and_parameters_but_no_body() {
        let mut r = req(Some("application/json"), br#"{"k":1}"#);
        r.query = vec![("a".into(), "1".into())];

        let none = build_output(&r, FetchType::None, None).unwrap();
        assert_eq!(none.body, None);
        assert_eq!(none.uri, None);
        assert_eq!(
            none.parameters.as_ref().and_then(|p| p.get("a")),
            Some(&vec!["1".to_string()]),
            "NONE still carries parameters -- measured"
        );
        assert!(none.headers.is_some(), "and headers");

        let fetch = build_output(&r, FetchType::Fetch, None).unwrap();
        assert_eq!(
            fetch.body,
            Some(json!({"k":1})),
            "the control: the SAME request under FETCH does carry the body"
        );

        assert_eq!(
            build_output(&r, FetchType::Store, None),
            Err(Unsupported::StoreBody)
        );
    }

    /// The `uri` branch returns before the body switch, so a stored body never
    /// carries both. `stored_uri` is unreachable from the route today; this is
    /// the unit test that keeps the branch honest for whoever wires it.
    #[test]
    fn a_stored_uri_replaces_the_body_rather_than_joining_it() {
        let o = build_output(
            &req(Some("application/json"), br#"{"k":1}"#),
            FetchType::Store,
            Some("kestra:///ns/flow/executions/e/webhook/body"),
        )
        .unwrap();
        assert_eq!(
            o.uri.as_deref(),
            Some("kestra:///ns/flow/executions/e/webhook/body")
        );
        assert_eq!(o.body, None);
    }

    #[test]
    fn multipart_is_refused_rather_than_half_read() {
        let r = req(Some("multipart/form-data; boundary=abc"), b"--abc--");
        assert_eq!(
            build_output(&r, FetchType::Fetch, None),
            Err(Unsupported::Multipart)
        );
    }

    /// Repeated query parameters keep both values, in URL order, and a single
    /// one is still a list. Measured:
    /// `{"validationToken":["abc","def"],"x":["1"]}`.
    #[test]
    fn query_parameters_group_by_name_and_keep_order() {
        let mut r = req(Some("application/json"), b"{}");
        r.query = vec![
            ("validationToken".into(), "abc".into()),
            ("validationToken".into(), "def".into()),
            ("x".into(), "1".into()),
        ];
        let o = build_output(&r, FetchType::Fetch, None).unwrap();
        let p = o.parameters.expect("parameters present");
        assert_eq!(p["validationToken"], vec!["abc", "def"]);
        assert_eq!(p["x"], vec!["1"]);

        // The control for "keep order": the reversed input must give the
        // reversed list, or the assertion above is satisfied by sorting.
        let mut r2 = r.clone();
        r2.query.swap(0, 1);
        let o2 = build_output(&r2, FetchType::Fetch, None).unwrap();
        assert_eq!(
            o2.parameters.expect("present")["validationToken"],
            vec!["def", "abc"]
        );
    }

    /// No query string at all still produces `parameters`, empty — measured.
    #[test]
    fn an_absent_query_string_produces_an_empty_map_not_an_absent_key() {
        let o = build_output(&req(Some("text/plain"), b"x"), FetchType::Fetch, None).unwrap();
        let p = o.parameters.as_ref().expect("parameters is PRESENT");
        assert!(p.is_empty());
        assert!(
            serde_json::to_string(&o)
                .unwrap()
                .contains("\"parameters\":{}"),
            "and it serialises as an empty object, not as a missing key"
        );
    }

    #[test]
    fn textual_media_types_follow_micronauts_is_text_based() {
        for ct in [
            None,
            Some("text/plain"),
            Some("text/csv"),
            Some("application/json"),
            Some("application/json; charset=utf-8"),
            Some("application/ld+json"),
            Some("application/xml"),
            Some("image/svg+xml"),
            Some("application/x-www-form-urlencoded"),
            Some("  APPLICATION/JSON  "),
        ] {
            assert!(is_textual(ct), "{ct:?} should be textual");
        }
        for ct in [
            Some("application/octet-stream"),
            Some("image/png"),
            Some("application/pdf"),
            Some("audio/mpeg"),
        ] {
            assert!(!is_textual(ct), "{ct:?} should NOT be textual");
        }
    }

    // ------------------------------------------------------------------
    // inputs
    // ------------------------------------------------------------------

    /// **Critical 2's regression test, and the four spellings that make it
    /// one.**
    ///
    /// The first version of this module decided by scanning each input value
    /// for `"trigger."` / `"trigger["`. Rows 1 and 2 below passed that scan and
    /// were rendered against the KEY context, so the execution was handed the
    /// key comparison's private `{id, type}` object.
    ///
    /// The fixture is the discriminating set, not a confirming one: two
    /// spellings the old scan missed, one it caught, and two controls that must
    /// still render.
    #[test]
    fn no_spelling_of_the_trigger_root_can_reach_an_input() {
        let yaml = "\
id: wi
namespace: fx.ns
variables:
  vkey: fx-var-value
inputs:
  - id: a
    type: STRING
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: bare
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-bare
    inputs:
      a: \"{{ trigger }}\"
  - id: filtered
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-filtered
    inputs:
      a: \"{{ trigger | json }}\"
  - id: dotted
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-dotted
    inputs:
      a: \"{{ trigger.body }}\"
  - id: indexed
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-indexed
    inputs:
      a: \"{{ trigger['body'] }}\"
  - id: aliased
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-aliased
    inputs:
      a: \"{{ trigger.id ~ '' }}\"
  - id: listed
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-listed
    inputs:
      a: \"{{ [trigger] }}\"
  - id: dicted
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-dicted
    inputs:
      a: \"{{ {'a': trigger} }}\"
  - id: coalesced
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-coalesced
    inputs:
      a: \"{{ trigger ?? 'fallback' }}\"
  - id: defaulted
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-defaulted
    inputs:
      a: \"{{ trigger | default('fallback') }}\"
  - id: probed
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-probed
    inputs:
      a: \"{{ trigger is defined }}\"
  - id: ok-flow
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-ok-flow
    inputs:
      a: \"from-{{ flow.namespace }}\"
  - id: ok-vars
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-ok-vars
    inputs:
      a: \"{{ vars.vkey }}\"
  - id: broken
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-broken
    inputs:
      a: \"{{ nosuchroot.nothing }}\"
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        let icx = InputContext::of(&kcx);

        // **Ten spellings, and the last five are fix round 2's Important 1.**
        //
        // The first five FAIL to render against a context with no `trigger`
        // binding, so closing the leak also refused them. The last five
        // **succeed**: `[trigger]` is `[null]`, `{'a': trigger}` is `{}`,
        // `?? 'fallback'` and `| default(...)` are the fallback, and
        // `is defined` is `false`. Nothing leaked — a missing binding cannot
        // produce the object — but each was ACCEPTED, and a flow that says
        // `trigger` and is handed `false` has been accepted wrongly.
        //
        // Over-acceptance is the one direction the oracle cannot catch, which
        // is why the root check now runs BEFORE the render rather than on its
        // failure path.
        const SPELLINGS: [&str; 10] = [
            "k-bare",
            "k-filtered",
            "k-dotted",
            "k-indexed",
            "k-aliased",
            "k-listed",
            "k-dicted",
            "k-coalesced",
            "k-defaulted",
            "k-probed",
        ];
        for key in SPELLINGS {
            let (_, w) = find(&f.triggers, key, &kcx).expect("the fixture matches");
            let out = render_inputs(w, &icx);
            assert!(
                out.is_err(),
                "{key} RENDERED -- this is Critical 2 again: {out:?}"
            );
        }

        // Then the classification, which is a diagnostic rather than a
        // guarantee. All ten name the root, so all ten get the message that
        // says so.
        for key in SPELLINGS {
            let (_, w) = find(&f.triggers, key, &kcx).unwrap();
            let err = render_inputs(w, &icx).unwrap_err();
            assert_eq!(
                err,
                InputRefusal::NeedsTriggerContext { key: "a".into() },
                "{key} was classified wrongly"
            );
            assert!(
                err.message("t-id").contains("input 'a'")
                    && err.message("t-id").contains("`trigger` render root"),
                "{key}: {}",
                err.message("t-id")
            );
        }

        // **A known imprecision, recorded rather than left to be rediscovered.**
        // `{{ trigger | json }}` has a SECOND, independent problem: egret has no
        // `json` filter at all -- a pre-existing W2 gap that `valids/webhook.yaml`
        // also runs into -- so supplying the trigger root would not make it
        // render, and the message this classifier gives names only the first
        // cause. Asserted so that if the filter ever lands, this comment is
        // what fails rather than quietly becoming wrong.
        assert!(
            render_str("{{ trigger | json }}", &{
                let mut cx = icx.render_context();
                cx.insert("trigger", json!({"id": "x"}));
                cx
            })
            .is_err_and(|e| e.to_string().contains("unknown filter")),
            "egret has acquired a `json` filter; the imprecision noted above is gone"
        );

        // And the OTHER failure mode, which must NOT be classified as the
        // capability gap: without this row the classifier could answer
        // `NeedsTriggerContext` for everything that fails.
        let (_, w) = find(&f.triggers, "k-broken", &kcx).unwrap();
        let err = render_inputs(w, &icx).expect_err("a broken template does not render");
        match &err {
            InputRefusal::Unrenderable { key, error } => {
                assert_eq!(key, "a");
                assert!(error.contains("undefined value"), "{error}");
            }
            other => panic!("a broken template must not be blamed on the trigger root: {other:?}"),
        }
    }

    /// The mechanism, asserted directly rather than through its consequences:
    /// the inputs context binds no `trigger`, and the way to know is to ask the
    /// renderer rather than to read the map.
    #[test]
    fn the_input_context_binds_no_trigger() {
        let f = flow_of(FIXTURE);
        let icx = InputContext::of(&kcx_of(&f));
        let cx = icx.render_context();

        assert!(
            render_str("{{ trigger }}", &cx).is_err(),
            "`trigger` must not be a binding in the inputs context"
        );
        // The controls: the two roots that ARE bound, so the assertion above
        // cannot be satisfied by a context that binds nothing at all.
        assert_eq!(render_str("{{ flow.id }}", &cx).unwrap(), "fx-flow-id");
        assert_eq!(render_str("{{ vars.vkey }}", &cx).unwrap(), "fx-var-value");
        // And the discriminator against the KEY context, which does bind it.
        let (t, _) = find(&f.triggers, "fx-literal-key", &kcx_of(&f)).unwrap();
        assert_eq!(
            render_str("{{ trigger.id }}", &kcx_of(&f).render_context_for(t)).unwrap(),
            "fx-trg-literal",
            "the KEY context still binds it -- the two contexts differ, which is the point"
        );
    }

    /// The classifier reads the PARSE, so the literal word in a text run is
    /// not a reference.
    ///
    /// This is the false-positive direction of Critical 2's fix, and it has to
    /// be held because the failed approach — a substring scan — got it wrong in
    /// both directions: it missed `{{ trigger }}` and it would have flagged a
    /// message that merely says the word.
    #[test]
    fn the_word_trigger_in_text_is_not_a_reference_to_the_root() {
        let yaml = "\
id: wi3
namespace: fx.ns
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: mentions
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-mentions
    inputs:
      a: \"this trigger.body is prose, not a lookup: {{ flow.id }}\"
  - id: reads
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-reads
    inputs:
      a: \"{{ trigger.body }}\"
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        let icx = InputContext::of(&kcx);

        let (_, w) = find(&f.triggers, "k-mentions", &kcx).unwrap();
        assert_eq!(
            render_inputs(w, &icx).unwrap(),
            json!({"a": "this trigger.body is prose, not a lookup: wi3"}),
            "the OLD substring scan would have refused this"
        );

        // The control in the other direction, on the same fixture: a real
        // reference is still caught, so the assertion above is not satisfied by
        // a classifier that has stopped looking.
        let (_, w) = find(&f.triggers, "k-reads", &kcx).unwrap();
        assert_eq!(
            render_inputs(w, &icx).unwrap_err(),
            InputRefusal::NeedsTriggerContext { key: "a".into() }
        );
    }

    /// `referenced_roots` sees through every spelling the substring scan
    /// missed, and through the two the scan would have false-positived on.
    #[test]
    fn the_parser_reports_the_roots_a_substring_scan_cannot() {
        use crate::runners::referenced_roots;
        for src in [
            "{{ trigger }}",
            "{{ trigger.body }}",
            "{{ trigger['body'] }}",
            "{{ trigger | json }}",
            "{% set t = trigger %}{{ t }}",
            "{{ (trigger).body }}",
        ] {
            assert!(
                referenced_roots(src).unwrap().contains("trigger"),
                "{src} does reference the root"
            );
        }
        for src in [
            "plain text mentioning trigger.body",
            "{{ flow.id }}",
            "{{ vars.trigger }}",
            "{{ outputs.trigger.x }}",
        ] {
            assert!(
                !referenced_roots(src).unwrap().contains("trigger"),
                "{src} does NOT reference the root"
            );
        }
    }

    /// **Claim 16's regression test.** Nested values are rendered, and the
    /// fixture is templates rather than numbers.
    ///
    /// The test this replaces was called
    /// non_string_input_values_are_not_rendered (deleted; named here as history,
    /// not as a citation) and its "list" row was
    /// `l: [1, 2]` — two numbers, never a template — so it asserted the
    /// doc's false sentence against a fixture that could not have
    /// contradicted it. That is this branch's recurring shape one more time,
    /// and it is why nobody looked at the nested case.
    ///
    /// Measured arms of `VariableRenderer.renderObject`: `Map` recurses **and
    /// renders the key**, `List` recurses per element, `String` renders,
    /// anything else is returned unchanged.
    #[test]
    fn nested_values_are_rendered_the_way_the_artifact_renders_them() {
        let yaml = "\
id: nested
namespace: fx.ns
variables:
  vkey: V
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: h
    type: io.kestra.plugin.core.trigger.Webhook
    key: k
    inputs:
      plain: \"{{ vars.vkey }}\"
      inlist: [\"{{ vars.vkey }}\", 7, true, null]
      inmap:
        deep: \"{{ vars.vkey }}\"
      deeper:
        - a: [\"{{ vars.vkey }}\"]
      untouched: 7
      alsountouched: true
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        let (_, w) = find(&f.triggers, "k", &kcx).unwrap();
        let out = render_inputs(w, &InputContext::of(&kcx)).unwrap();

        assert_eq!(out["plain"], json!("V"), "the top-level string");
        assert_eq!(
            out["inlist"],
            json!(["V", 7, true, null]),
            "a LIST element is rendered and the non-strings beside it are not"
        );
        assert_eq!(out["inmap"]["deep"], json!("V"), "a MAP value is rendered");
        assert_eq!(
            out["deeper"][0]["a"][0],
            json!("V"),
            "list -> map -> list -> string still renders"
        );
        assert_eq!(out["untouched"], json!(7));
        assert_eq!(out["alsountouched"], json!(true));
    }

    /// Map **keys** are rendered, and two keys that render to one string
    /// resolve by `putIfAbsent`.
    #[test]
    fn map_keys_are_rendered_and_collide_by_put_if_absent() {
        let yaml = "\
id: keys
namespace: fx.ns
variables:
  vkey: V
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: h
    type: io.kestra.plugin.core.trigger.Webhook
    key: k
    inputs:
      \"{{ vars.vkey }}\": rendered-key
      nested:
        \"pre-{{ vars.vkey }}\": also-rendered
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        let (_, w) = find(&f.triggers, "k", &kcx).unwrap();
        let out = render_inputs(w, &InputContext::of(&kcx)).unwrap();

        assert_eq!(out["V"], json!("rendered-key"), "the KEY rendered");
        assert!(
            out.get("{{ vars.vkey }}").is_none(),
            "and the source spelling is gone: {out}"
        );
        assert_eq!(out["nested"]["pre-V"], json!("also-rendered"));

        // **`putIfAbsent`, and the fixture is eight groups of eight rather
        // than one pair.** Fix round 4's Important.
        //
        // The previous version used TWO colliding keys, so a random iteration
        // order picked the sorted-first one half the time -- and mutation D6
        // (drop `keys.sort()`) accordingly **survived 10 of 18 runs**. A
        // mutation reported dead from one run is a coin flip reported as a
        // measurement, and this fixture is what makes the coin have 8^8 faces:
        // eight independent collision groups, each of eight keys, each group's
        // winner independently uniform if the order is random. A false pass
        // needs all eight groups to land on their sorted-first key at once --
        // 8^-8, about 6 in 100 million.
        //
        // The keys differ only by a minijinja COMMENT, which renders away
        // (`{#0#}G0` and `{#7#}G0` both render to `G0`, measured), so the eight
        // are genuinely one key after rendering while being eight distinct YAML
        // keys before it. Their sort order is the comment's digit.
        let mut decl = String::new();
        for g in 0..8 {
            for i in 0..8 {
                decl.push_str(&format!("      \"{{#{i}#}}G{g}\": won-by-{i}\n"));
            }
        }
        let collide = format!(
            "\
id: collide
namespace: fx.ns
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: h
    type: io.kestra.plugin.core.trigger.Webhook
    key: k
    inputs:
{decl}"
        );
        let f = flow_of(&collide);
        let kcx = kcx_of(&f);
        let (_, w) = find(&f.triggers, "k", &kcx).unwrap();
        assert_eq!(
            w.inputs.as_ref().map(|m| m.len()),
            Some(64),
            "the fixture must carry all 64 source keys, or the groups are not independent"
        );
        let out = render_inputs(w, &InputContext::of(&kcx)).unwrap();

        let expected: serde_json::Value = (0..8)
            .map(|g| (format!("G{g}"), json!("won-by-0")))
            .collect::<serde_json::Map<_, _>>()
            .into();
        assert_eq!(
            out, expected,
            "eight groups of eight keys all rendering to one string each; `putIfAbsent` keeps \
             the first in SORTED order, which is `won-by-0` in every group. A random iteration \
             order agrees on all eight at once with probability 8^-8."
        );
    }

    /// **Important 5.** A `trigger` reference inside a NON-STRING value is
    /// refused, wherever it is.
    ///
    /// It used to be neither rendered nor refused: the check was asked only for
    /// a top-level `Value::String`, so `a: ["{{ trigger }}"]` was stored
    /// verbatim — the template text itself reaching the execution's `inputs`.
    #[test]
    fn a_trigger_reference_anywhere_in_a_nested_value_is_refused() {
        for (what, decl) in [
            ("in a list", "      a: [\"{{ trigger }}\"]"),
            ("in a map value", "      a:\n        b: \"{{ trigger }}\""),
            ("in a map KEY", "      a:\n        \"{{ trigger }}\": 1"),
            (
                "two deep",
                "      a:\n        - b: [\"{{ trigger.body }}\"]",
            ),
            (
                "beside a legitimate sibling",
                "      a: [\"{{ flow.id }}\", \"{{ trigger }}\"]",
            ),
        ] {
            let yaml = format!(
                "\
id: nest
namespace: fx.ns
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: h
    type: io.kestra.plugin.core.trigger.Webhook
    key: k
    inputs:
{decl}
"
            );
            let f = flow_of(&yaml);
            let kcx = kcx_of(&f);
            let (_, w) = find(&f.triggers, "k", &kcx).unwrap();
            let out = render_inputs(w, &InputContext::of(&kcx));
            assert!(
                matches!(out, Err(InputRefusal::NeedsTriggerContext { .. })),
                "{what}: {out:?}"
            );
            // The refusal names WHERE, which a flat key could not.
            let Err(e) = out else { unreachable!() };
            let msg = e.message("h");
            assert!(msg.contains("input 'a"), "{what}: {msg}");
        }

        // The control: the same shapes without the root still render.
        let yaml = "\
id: nestok
namespace: fx.ns
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: h
    type: io.kestra.plugin.core.trigger.Webhook
    key: k
    inputs:
      a: [\"{{ flow.id }}\"]
      b:
        c: \"{{ flow.id }}\"
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        let (_, w) = find(&f.triggers, "k", &kcx).unwrap();
        assert_eq!(
            render_inputs(w, &InputContext::of(&kcx)).unwrap(),
            json!({"a": ["nestok"], "b": {"c": "nestok"}})
        );
    }

    /// **Important 6.** Indirection through `vars` cannot reach the root.
    ///
    /// The render is a fixpoint of up to `MAX_RENDERING_AMOUNT` passes and a
    /// pass can emit a template the next pass evaluates. Asking the source
    /// alone asks about pass 1: `vars.tpl = "{{ [trigger] }}"` with
    /// `a: "{{ vars.tpl }}"` named only `vars` on pass 1, rendered to
    /// `"[null]"` on pass 2, and answered **200**.
    ///
    /// Indirection through `vars` is ordinary authoring, not an exotic attack.
    #[test]
    fn indirection_through_vars_cannot_reach_the_trigger_root() {
        let yaml = "\
id: indirect
namespace: fx.ns
variables:
  tpl: \"{{ [trigger] }}\"
  deep: \"{{ vars.tpl }}\"
  innocent: \"{{ flow.id }}\"
tasks:
  - id: t
    type: io.kestra.plugin.core.debug.Return
    format: x
triggers:
  - id: one
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-one
    inputs:
      a: \"{{ vars.tpl }}\"
  - id: two
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-two
    inputs:
      a: \"{{ vars.deep }}\"
  - id: three
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-three
    inputs:
      a: [\"{{ vars.tpl }}\"]
  - id: ok
    type: io.kestra.plugin.core.trigger.Webhook
    key: k-ok
    inputs:
      a: \"{{ vars.innocent }}\"
";
        let f = flow_of(yaml);
        let kcx = kcx_of(&f);
        let icx = InputContext::of(&kcx);

        for key in ["k-one", "k-two", "k-three"] {
            let (_, w) = find(&f.triggers, key, &kcx).unwrap();
            let out = render_inputs(w, &icx);
            assert!(
                matches!(out, Err(InputRefusal::NeedsTriggerContext { .. })),
                "{key} reached the root by indirection: {out:?}"
            );
        }

        // The control, on the same fixture: indirection through `vars` that
        // does NOT reach the root still renders, and renders to its fixpoint
        // rather than to the intermediate template.
        let (_, w) = find(&f.triggers, "k-ok", &kcx).unwrap();
        assert_eq!(
            render_inputs(w, &icx).unwrap(),
            json!({"a": "indirect"}),
            "two passes, and the answer is the fixpoint"
        );
    }

    /// No `inputs:` at all is the empty object, not a null and not an error.
    #[test]
    fn a_trigger_with_no_inputs_renders_the_empty_object() {
        let f = flow_of(FIXTURE);
        let kcx = kcx_of(&f);
        let (_, w) = find(&f.triggers, "fx-literal-key", &kcx).unwrap();
        assert!(w.inputs.is_none(), "the fixture's trigger declares none");
        assert_eq!(
            render_inputs(w, &InputContext::of(&kcx)).unwrap(),
            json!({})
        );
    }

    /// **Step 4's measurement, and what package #5a Task A5 changed in it.**
    ///
    /// *Deleted by accident in fix round 1 and restored here (fix round 2,
    /// Important 4).* The module doc cites it as the running program that
    /// holds all four rows.
    ///
    /// `valids/webhook-plaintext`'s only task renders
    /// `{{ trigger.parameters.validationToken[0] ?? 'hello-world' }}`. Kestra
    /// renders `hello-world` (the golden was harvested through a plain
    /// execution POST, so its `Execution.trigger` is null).
    ///
    /// | context | before Task A5 | since Task A5 (2026-10-29) |
    /// |---|---|---|
    /// | no `trigger` root at all | `Err(undefined value)` | **`hello-world`** |
    /// | `trigger` root present, no `parameters` | `Err(undefined value)` | **`hello-world`** |
    /// | `trigger.parameters.validationToken` present | `Ok("tok")` | `Ok("tok")` |
    ///
    /// Step 4's finding stands as history: defining the root changed nothing,
    /// because the expression misses at `trigger.parameters` and then indexes
    /// `.validationToken` on the result -- two levels, not one. Spec D6 (Q6)
    /// compiles a path on the left of `??` to `kestra_path(..)`, which answers
    /// `Undefined` at the first missing step instead of raising, so both
    /// missing contexts reach the fallback and the executor entry banked
    /// (`oracle-exec/baseline.txt` lost `valids/webhook-plaintext`).
    #[test]
    fn a_missing_trigger_chain_renders_webhook_plaintexts_fallback() {
        const EXPR: &str = "{{ trigger.parameters.validationToken[0] ?? 'hello-world' }}";

        fn cx(trigger: Option<Value>) -> RenderContext {
            let mut cx = RenderContext::new();
            cx.insert(
                "flow",
                json!({"id": "webhook-plaintext", "namespace": "io.kestra.tests"}),
            );
            if let Some(t) = trigger {
                cx.insert("trigger", t);
            }
            cx
        }

        // Row 1 -- today's shipping context: no `trigger` root.
        assert_eq!(
            render_str(EXPR, &cx(None)).expect("a missing root reaches the fallback"),
            "hello-world"
        );
        // Row 2 -- a defined-but-empty root: the same answer.
        assert_eq!(
            render_str(EXPR, &cx(Some(json!({})))).expect("a missing member reaches the fallback"),
            "hello-world"
        );
        // Row 3 -- the control, without which rows 1 and 2 are satisfied by a
        // renderer that can never render this expression at all.
        assert_eq!(
            render_str(
                EXPR,
                &cx(Some(json!({"parameters": {"validationToken": ["tok"]}})))
            )
            .expect("a fully populated trigger root renders"),
            "tok"
        );
        // Row 4 -- the one-level miss the old baseline sentence was about.
        let mut one_level = cx(None);
        one_level.insert("outputs", json!({}));
        assert_eq!(
            render_str("{{ outputs.not_existing ?? 'fallback' }}", &one_level)
                .expect("a one-level miss does fall back"),
            "fallback"
        );
    }

    // ------------------------------------------------------------------
    // policy
    // ------------------------------------------------------------------

    /// The key context has **exactly one consumer**, and that is the whole of
    /// what makes "local to the key comparison" true.
    ///
    /// It was not true when it was first asserted in prose: `render_inputs`
    /// used the same context, so a `{{ trigger }}` input reached an execution
    /// (fix round 1, Critical 2). An invariant about callers has to be counted,
    /// because nothing about the type stops a second one appearing.
    ///
    /// Scans the crate's whole `src/`, shipping halves only, so a new caller in
    /// any module fails this.
    #[test]
    fn the_key_context_is_reachable_from_no_other_render() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("read_dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        files.sort();
        assert!(
            files.len() > 20,
            "only {} files scanned -- the walk is looking at the wrong tree",
            files.len()
        );

        let mut callers: Vec<(String, usize)> = Vec::new();
        for path in &files {
            let source = std::fs::read_to_string(path).expect("read source");
            let shipping = match source.find("\n#[cfg(test)]") {
                Some(i) => &source[..i],
                None => &source[..],
            };
            // The definition itself is `fn render_context_for`; a CALL is the
            // method-position spelling.
            let n = shipping.matches(".render_context_for(").count();
            if n > 0 {
                callers.push((path.strip_prefix(&root).unwrap().display().to_string(), n));
            }
        }
        assert_eq!(
            callers,
            vec![("trigger/webhook.rs".to_string(), 1)],
            "the key context must have exactly one consumer -- `render_key`. A second one is \
             how Critical 2 happened: `render_inputs` shared it and handed an execution the \
             key comparison's private object."
        );
    }

    /// A running check that the key is never compared with `==`.
    ///
    /// Modelled on `super::super::tests::no_wall_clock_in_the_trigger_path`,
    /// and here for the same reason: a constant-time comparison and a plain
    /// one are behaviourally identical, so **no behavioural test can tell them
    /// apart**. A mutation table cannot either — replacing [`ct_eq`]'s body
    /// with `a == b` passes every other test in this file. The only instrument
    /// that sees the difference is one that reads the source, so that is the
    /// instrument.
    ///
    /// It scans the shipping slice of this file only (everything before
    /// `#[cfg(test)]`), and asserts the marker was found so that a file with
    /// no test module cannot silently scan nothing.
    #[test]
    fn the_key_is_never_compared_with_a_plain_equality() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/trigger/webhook.rs");
        let source = std::fs::read_to_string(&path).expect("read own source");
        assert!(source.len() > 500, "did the scan read the right file?");

        let marker = source
            .find("\n#[cfg(test)]")
            .expect("this file has a test module; the scan must stop at it");
        let shipping = &source[..marker];
        assert!(!shipping.is_empty());

        // The needles are spelled as they would appear in the key path. A
        // plain `==` is legal elsewhere in the file (`ct == "text"`), so the
        // check is over the two IDENTIFIERS that carry the key, not over the
        // operator alone.
        for needle in [
            "declared_rendered ==",
            "== declared_rendered",
            "url_key ==",
            "== url_key",
            "declared_rendered.eq",
            "url_key.eq",
            "rendered == ",
            ".key ==",
        ] {
            assert!(
                !shipping.contains(needle),
                "`{needle}` appears in the shipping half of webhook.rs: the webhook key must be \
                 compared only through ct_eq()"
            );
        }

        // ------------------------------------------------------------------
        // The body of `ct_eq` itself.
        //
        // The needles above are call-site spellings, and **mutation M1 proved
        // they are not enough**: swapping this function's body for `a == b`
        // changes no call site, matches no needle, and passes every
        // behavioural test in the crate. So the body is pinned by shape.
        //
        // `pub fn ct_eq(` .. the next line that is exactly `}` at column 0.
        // ------------------------------------------------------------------
        let start = shipping
            .find("pub fn ct_eq(")
            .expect("ct_eq is defined in the shipping half");
        let rest = &shipping[start..];
        let end = rest.find("\n}\n").expect("ct_eq has a closing brace") + 1;
        let body = &rest[..end];
        assert!(
            body.len() > 100 && body.len() < 1000,
            "the ct_eq slice is {} bytes -- the extraction is looking at the wrong thing",
            body.len()
        );
        for required in [
            // the loop runs over EVERY byte of the declared key
            "for (i, byte) in a.iter().enumerate()",
            // out-of-range reads are clamped rather than skipped
            "let index_b = if i < b.len() { i } else { 0 };",
            // the accumulator ORs; it never branches out early
            "result |= u64::from(byte ^ b[index_b]);",
            "result == 0",
        ] {
            assert!(
                body.contains(required),
                "ct_eq no longer contains `{required}`. Its constant-time property is not \
                 observable from behaviour -- mutation M1 (body -> `a == b`) passed all 49 \
                 tests -- so this shape is the only thing asserting it."
            );
        }
        for forbidden in [
            "a == b",
            "b == a",
            "a.eq(b)",
            "b.eq(a)",
            "break",
            "return false",
        ] {
            assert!(
                !body.contains(forbidden),
                "ct_eq contains `{forbidden}`, which either compares or short-circuits: \
                 both leak the key one byte at a time to a caller that can time the response"
            );
        }

        // And the positive half: the one comparison that IS there.
        assert_eq!(
            shipping.matches("ct_eq(").count(),
            2,
            "expected exactly two occurrences of `ct_eq(` in the shipping half -- its \
             definition and its one call in keys_match. A third means a second comparison \
             site appeared; a first means keys_match stopped using it. (Counted, not \
             guessed: the first draft of this assertion said three and was wrong, because \
             the doc-link spelling is ``[`ct_eq`]`` and carries no parenthesis.)"
        );
    }
}
