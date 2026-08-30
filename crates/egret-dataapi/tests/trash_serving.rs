//! The trash, measured from the CONSUMER side: what a caller with a valid token
//! gets back across the whole disable → trash → restore → enable cycle.
//!
//! **What this file proves, stated precisely, because the obvious reading of it is
//! wrong.** `trash` never writes `state`, and it refuses a `live` endpoint outright
//! (`endpoints.rs`, `AND state <> 'live'` in the UPDATE's own `WHERE`), so an
//! endpoint can only reach the trash from `draft` or `disabled` — by which point it
//! was *already* not being served. "Trashing stops serving" is therefore a property
//! of `disable`, and a test asserting it would pass even if `trash` did nothing at
//! all.
//!
//! So this is a **regression guard on the end state**, not a proof of causation. It
//! would catch a `trash` or a `restore` that resurrected serving — a restore
//! landing on `live` instead of `disabled` is the plausible version of that, and it
//! is one line away in `EndpointStore::restore` — and it pins the answer a consumer
//! gets while an endpoint sits in the trash to be *byte-identical* to the answer for
//! a URL that never existed. The guarantee that a trashed endpoint cannot serve
//! lives elsewhere: in migration 0126's `CHECK (deleted_at IS NULL OR state <>
//! 'live')`, and in the `deleted_at IS NULL` predicates that turn every write toward
//! `live` into a 409 (measured in `tests/trash.rs`).
//!
//! Driven through the REAL serving router and the REAL management routes from
//! `serving_common`, so nothing here is a store call in disguise.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no DATABASE_URL FAILS — check the DURATION,
//! not just the `ok`.

mod serving_common;

use axum::http::StatusCode;
use serving_common::{fixture, marker};

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

fn no_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": false })
}

fn no_limits() -> serde_json::Value {
    serde_json::json!({ "per_key_qps": 0, "burst": 0, "per_endpoint_qps": 0 })
}

/// An error body with `request_id` removed, so two refusals can be compared for
/// equality.
///
/// `request_id` is per-request by design — it is what correlates a caller's
/// complaint with a row in `dataapi.access_log` — so it is the ONE field that must
/// differ between two calls, and comparing whole bodies without dropping it would
/// fail on a correct implementation. Everything else is compared, including the
/// error `code` and the `message`: those are what would leak that a URL exists.
fn without_request_id(body: &serde_json::Value) -> serde_json::Value {
    let mut b = body.clone();
    if let Some(err) = b.get_mut("error").and_then(|e| e.as_object_mut()) {
        assert!(
            err.remove("request_id").is_some(),
            "every serving error body carries a request_id; if that stopped being \
             true this helper is silently comparing nothing: {body}"
        );
    }
    b
}

/// The full cycle, in one test, because the assertions are about the TRANSITIONS
/// between these states and splitting them would leave each half asserting a state
/// it did not itself reach.
///
/// The `never_existed` probe is taken at the end rather than assumed: it is the
/// reference answer the trashed and restored probes are compared against, and
/// comparing against a hard-coded `404` would not notice a body that leaked
/// `"state": "disabled"` or the endpoint's name.
#[tokio::test]
async fn a_trashed_endpoint_answers_exactly_as_one_that_never_existed_and_a_restore_does_not_resume_serving(
) {
    let f = skip_without_db!();
    let ep = f
        .publish("/trash-cycle", f.database_id, no_cache(), no_limits())
        .await;
    let path = "trash-cycle";
    let mgmt = format!(
        "/api/ws/{}/data-api/endpoints/{ep}",
        f.ws_slug.clone().as_str()
    );

    // 1. Published and live: the consumer is served.
    let served = f.call_as(f.principal_a, path).await;
    assert_eq!(
        served.status,
        StatusCode::OK,
        "the fixture must be serving before anything is asserted about it not \
         serving — otherwise every assertion below passes vacuously: {}",
        served.body
    );
    // The fake engine returns a row derived from the CALLING principal, so this
    // also says WHOSE answer it was — not merely that a row came back.
    assert_eq!(
        served.ns(),
        vec![marker(f.principal_a)],
        "body: {}",
        served.body
    );

    // 2. Disabled. This is the step that actually stops traffic, and it is measured
    //    here so the file cannot be misread as attributing it to the trash.
    let (status, body) = f.mgmt_post(&format!("{mgmt}/disable"), None).await;
    assert_eq!(status, StatusCode::OK, "disable: {body}");
    let disabled = f.call_as(f.principal_a, path).await;
    assert_eq!(
        disabled.status,
        StatusCode::NOT_FOUND,
        "disable is what takes the URL off the internet: {}",
        disabled.body
    );

    // 3. Trashed. The endpoint was already not serving, so the assertion that
    //    matters is that it STILL does not, and that the answer gives a prober
    //    nothing an unknown URL would not have given them.
    let (status, body) = f.mgmt_post(&format!("{mgmt}/trash"), None).await;
    assert_eq!(status, StatusCode::OK, "trash: {body}");
    assert!(
        body["deleted_at"].is_string(),
        "trash must have written the timestamp: {body}"
    );
    let trashed = f.call_as(f.principal_a, path).await;

    // 4. Restored — and NOT serving. `restore` lands on `disabled`, never on
    //    whatever the endpoint was before, so a Trash screen's undo button cannot
    //    put a URL back on the internet as a side effect. This is the assertion the
    //    file exists for.
    let (status, body) = f.mgmt_post(&format!("{mgmt}/restore"), None).await;
    assert_eq!(status, StatusCode::OK, "restore: {body}");
    assert_eq!(
        body["state"],
        serde_json::json!("disabled"),
        "restore must come back disabled: {body}"
    );
    let restored = f.call_as(f.principal_a, path).await;
    assert_eq!(
        restored.status,
        StatusCode::NOT_FOUND,
        "a restore must not resume serving — enabling is a separate, audited act: {}",
        restored.body
    );

    // The reference answer, taken from the same router with the same token.
    let never_existed = f.call_as(f.principal_a, "no-such-endpoint-here").await;
    assert_eq!(
        never_existed.status,
        StatusCode::NOT_FOUND,
        "body: {}",
        never_existed.body
    );
    let reference = without_request_id(&never_existed.body);
    assert_eq!(
        (trashed.status, without_request_id(&trashed.body)),
        (never_existed.status, reference.clone()),
        "a trashed endpoint must answer EXACTLY as an unknown URL does, body \
         included — a body naming the endpoint or its state tells a prober that \
         something is there. Full trashed body: {}",
        trashed.body
    );
    assert_eq!(
        (restored.status, without_request_id(&restored.body)),
        (never_existed.status, reference.clone()),
        "and so must a restored-but-not-enabled one: {}",
        restored.body
    );
    assert_eq!(
        (disabled.status, without_request_id(&disabled.body)),
        (never_existed.status, reference),
        "as must the disabled one it passed through on the way in: {}",
        disabled.body
    );

    // 5. Explicitly enabled: serving again, from the SAME version — which is what
    //    makes the four 404s above statements about state rather than about a
    //    fixture that broke somewhere in the middle.
    let (status, body) = f.mgmt_post(&format!("{mgmt}/enable"), None).await;
    assert_eq!(status, StatusCode::OK, "enable: {body}");
    assert_eq!(body["state"], serde_json::json!("live"), "body: {body}");
    let again = f.call_as(f.principal_a, path).await;
    assert_eq!(
        again.status,
        StatusCode::OK,
        "the endpoint must serve again once explicitly enabled: {}",
        again.body
    );
    assert_eq!(
        again.ns(),
        vec![marker(f.principal_a)],
        "body: {}",
        again.body
    );
    assert_eq!(
        again.body["meta"]["version"], 1,
        "and from the version that was live before it went to the trash, with no \
         re-publish: {}",
        again.body
    );
}
