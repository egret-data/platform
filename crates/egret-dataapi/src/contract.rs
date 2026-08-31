//! What publishing a new version does to the consumers of the old one.
//!
//! Package 6 of the production-readiness programme. Until it,
//! [`crate::lifecycle::EndpointStore::publish`] validated the draft against
//! **itself**: its only read of `dataapi.endpoint_versions` was the
//! `COALESCE(MAX(version), 0) + 1` inside the INSERT. A removed column, a
//! narrowed type, a renamed parameter or a new required parameter therefore
//! published in silence, and every deployed consumer discovered it at its next
//! call.
//!
//! This module is the comparison that was missing: given the snapshot a
//! consumer is receiving right now and the snapshot a publish is about to
//! freeze, it says what changed and which of those changes a caller cannot
//! survive.
//!
//! # The rule, in one sentence
//!
//! > A change is **breaking** when a request that succeeded against the live
//! > version could — with the caller changing nothing — now be **refused**,
//! > **silently ignored**, or answered with a **body their existing code cannot
//! > read**.
//!
//! Everything in [`ChangeKind::is_breaking`] is derived from that sentence, and
//! nothing is flagged that does not follow from it. That restraint is the point
//! rather than a nicety: a rule that fires on every publish teaches an author to
//! click through the warning, and an acknowledgement nobody reads is worse than
//! no acknowledgement at all, because it also carries a record saying they
//! agreed.
//!
//! ## Why "silently ignored" is in the rule
//!
//! Because of one case, and it is the case the package's own brief names:
//! **renaming a parameter**. `serving::params::resolve_supplied_with` walks the
//! DECLARED specs and looks each one up in the supplied map; a supplied name
//! that is no longer declared is never looked at. So renaming `region` to
//! `regions` does not refuse anybody's call — it answers `?region=EU` with
//! unfiltered data, a 200, and nothing anywhere saying the filter stopped
//! applying. That is worse than a refusal, so removing a declared parameter is
//! breaking even though no request fails.
//!
//! ## What the wire shape actually depends on
//!
//! The worst case in this package is the one that looks additive, and it is not
//! a guess about the envelope — it is
//! [`crate::serving::router`]'s own predicate, read from the two functions that
//! decide `data` and `meta`:
//!
//! * `statements_data` unwraps a single statement's rows onto `data` and keys
//!   them by statement name once there is more than one;
//! * `meta_contract` takes its `Single` arm only when
//!   `snap.statements.len() == 1 && snap.output.transform.is_none()`.
//!
//! So the shape of a response is decided by that pair, and **adding a second
//! statement flips `data` from an array to an object** for every consumer, as
//! does **adding a transform to a single-statement endpoint**. Both are one-line
//! edits an author reads as "I added something". [`keyed_envelope`] is that
//! predicate, in one place, and a change to it is [`ChangeKind::EnvelopeFlipped`].
//!
//! # What is compared, and what is deliberately not
//!
//! Compared: `method`, `path`, `output` (envelope/shape/naming/transform),
//! the statement NAMES and each surviving statement's frozen `columns`, the
//! declared `params` with their types, locations, requiredness and constraints,
//! `limits`, and `rate_limit`.
//!
//! Not compared, each for a stated reason:
//!
//! * **`sql` / `script` text** — reported, never breaking. If editing a `WHERE`
//!   clause were breaking then every publish is breaking and this whole surface
//!   is noise. The contract is the SHAPE of the answer; the data is the
//!   endpoint's job.
//! * **`iterated`** — derived from that same statement text by
//!   `render::iterated_params_union`, so it cannot move unless the text does,
//!   and every request that succeeded still succeeds.
//! * **`delegated` / `impersonate`** — witnesses of the CONNECTION, not of the
//!   definition. They move without anyone editing anything, and serving already
//!   has `cache_scope_stale` for them.
//! * **`param_gate`** — a stamp of which revision of the publish gate ran.
//! * **`database_id`** — reported, never breaking on its own. Moving an endpoint
//!   to another connection changes nothing a caller receives that the column
//!   diff does not already say; if the new connection resolves different
//!   columns, THAT is what is breaking, and it is reported as such.
//! * **`cache`** — reported, never breaking. `meta.cached` is an observation of
//!   this call, not a promise about the next one, and `cache.scope` already has
//!   its own publish-time refusal.
//! * **`name`, `description`, `folder_id`** — not in a snapshot at all. Renaming
//!   an endpoint changes nothing a consumer receives.
//!
//! # A pre-M2 live version compares correctly, and it was checked
//!
//! The published corpus contains two snapshot shapes at once and always will: a
//! pre-M2 row carries a single `sql` with one `columns` list beside it, and
//! `EndpointSnapshotRepr` folds that into one statement named
//! [`egret_dataapi_api::LEGACY_STATEMENT_NAME`]. Migration `0129` gave every
//! pre-M2 DRAFT's single statement the same name, so re-publishing such an
//! endpoint compares `main` against `main` rather than reading as "the only
//! statement was removed and a differently-named one added" — which, with the
//! envelope rule above, would have been two spurious breaking changes on the
//! first re-publish of every endpoint predating M2.
//!
//! # Its relationship to the editor's state pill
//!
//! `frontend/src/routes/explore/dataapi/endpointState.tsx` already compares a
//! draft against the live snapshot, field by field, for the header pill. It is
//! **not** duplicated here and it is not replaced, because the two answer
//! different questions:
//!
//! | | question | answer |
//! |---|---|---|
//! | `draftVsServed` | is there anything to publish? | one boolean over nine fields |
//! | `compare_contracts` | does publishing break a consumer? | a classified list |
//!
//! A `sql` edit makes the pill say "unpublished changes" — correctly — and
//! produces no breaking change here, also correctly. Collapsing them would make
//! one of the two wrong. What the frontend must NOT grow is a second
//! *classification*: the breaking/safe judgement is this module's, it reaches
//! the console as data on `POST /validate`, and the acknowledgement token is
//! computed from it here. A UI that decided for itself which changes were
//! breaking would be the second answer this programme keeps closing.
//!
//! # `rollback` is deliberately NOT gated by this
//!
//! Putting yesterday's version back can break a consumer who coded against
//! today's — the diff would run just as well in that direction, and it is not
//! run. `EndpointStore::rollback`'s own comment already argues the case for the
//! two rules it *does* re-ask: re-checking everything would make a rollback fail
//! for a rule that did not exist when the version was published, "turning 'put
//! yesterday's version back' into an upgrade gate during an incident, which is
//! the one moment rollback has to work". An acknowledgement prompt is that same
//! gate with a checkbox on it. The asymmetry is deliberate: publish is an
//! author choosing to move forward and can afford to be stopped; rollback is
//! usually someone stopping an outage. It is recorded here so it is read as a
//! decision rather than rediscovered as a gap.

use std::cmp::Ordering;

use egret_dataapi_api::{
    compare_column_types, ColumnSpec, EndpointSnapshot, ParamSpec, StatementSnapshot, TypeAgreement,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::serving::params::{compare_decimal_text, normalize_temporal_bound};

/// The stable machine code for one kind of difference between two published
/// contracts.
///
/// A closed enum rather than a free string, and it is what
/// [`Self::is_breaking`] matches on — so the classification is a single table
/// that a reader can check against the rule at the top of this module, rather
/// than a `breaking: true` written by hand at each construction site where one
/// of them would eventually be wrong.
///
/// Serialised in `snake_case` and **recorded in `dataapi.endpoint_versions`**,
/// so these names are persisted data: renaming one silently reinterprets every
/// stored row. Add variants; do not rename them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    // ---- routing -------------------------------------------------------
    /// The HTTP method changed. Every deployed caller is now using the wrong
    /// verb, and the router matches on `(method, path)`.
    MethodChanged,
    /// The URL changed. The old one stops resolving.
    PathChanged,

    // ---- envelope ------------------------------------------------------
    /// `data` changed between "this statement's rows" and "an object keyed by
    /// statement name", and `meta` with it. See [`keyed_envelope`].
    EnvelopeFlipped,
    /// `output.envelope` moved between `wrapped` and `bare`, so the body is
    /// either gaining or losing the `{status, message, data, meta}` wrapper.
    EnvelopeWrapperChanged,
    /// `output.shape` moved between `objects` and `arrays`: every row changes
    /// between a JSON object and a positional array.
    RowShapeChanged,
    /// `output.naming` changed, so every key in every row is spelled
    /// differently.
    NamingChanged,
    /// A response transform was added or removed, so `data` changes between the
    /// statements' rows and whatever the script returns.
    TransformPresenceChanged,
    /// The transform's script was edited while its declared `returns` stayed the
    /// same. The contract is `returns`; the script is the implementation.
    TransformScriptChanged,

    // ---- statements ----------------------------------------------------
    /// A statement is gone, so its key is gone from `data` and from `meta`.
    StatementRemoved,
    /// A statement was added. Additive on its own — a consumer reading
    /// `data.rows` still finds `data.rows`. When it takes the endpoint from one
    /// statement to several it also flips the envelope, which is reported
    /// separately as [`Self::EnvelopeFlipped`].
    StatementAdded,
    /// The SQL or the `build_sql` script of a statement changed. The data is not
    /// the contract — see the module comment.
    StatementDefinitionChanged,

    // ---- columns -------------------------------------------------------
    /// A declared column is gone. A consumer reading it by name gets nothing,
    /// and under `shape: arrays` every later column shifts one position left.
    ColumnRemoved,
    /// A declared column is new. A consumer reading by name ignores it; under
    /// `shape: arrays` an appended column changes no existing index, and a
    /// column inserted anywhere else is reported as
    /// [`Self::ColumnOrderChanged`].
    ColumnAdded,
    /// A column's type no longer agrees with the one it was published under,
    /// judged by `egret_dataapi_api::compare_column_types` — the SAME comparison
    /// serving uses to answer `503 schema_drift`, so this surface and that
    /// refusal cannot disagree about what "the same column" means.
    ColumnTypeChanged,
    /// A column that was present in every described permutation may now be
    /// absent from some of them. A consumer that always found the key may now
    /// not.
    ColumnBecameOptional,
    /// A column that could be absent is now guaranteed. Strictly more promise.
    ColumnBecameRequired,
    /// Under `shape: arrays` the rows are positional and `shape_rows` zips them
    /// by INDEX, so a surviving column that moved is a different value at the
    /// index a consumer already reads.
    ColumnOrderChanged,

    // ---- parameters ----------------------------------------------------
    /// A declared parameter is gone. Callers still sending it are NOT refused —
    /// `resolve_supplied_with` never looks at an undeclared name — so their
    /// filter silently stops applying. This is the rename case; see the module
    /// comment.
    ParameterRemoved,
    /// A new optional parameter, or a new required one carrying a `default`.
    /// Existing calls are unaffected: the default is bound when the caller says
    /// nothing.
    ParameterAdded,
    /// A new **required** parameter with no `default`. Every existing call now
    /// answers `param_missing`.
    RequiredParameterAdded,
    /// An optional parameter became required with no `default`, which refuses
    /// every call that omitted it.
    ParameterBecameRequired,
    /// A required parameter became optional, or gained a `default`. Strictly
    /// fewer ways to be refused.
    ParameterBecameOptional,
    /// A parameter's declared type changed, so a value that coerced before may
    /// not now.
    ParameterTypeChanged,
    /// A parameter moved between `query` and `body`. A query parameter that is
    /// now `body` is not read from the query string at all.
    ParameterLocationChanged,
    /// A constraint got stricter, or one appeared where there was none: values
    /// that were accepted are now `param_invalid`.
    ParameterConstraintTightened,
    /// A constraint got looser or disappeared. Strictly more values accepted.
    ParameterConstraintRelaxed,
    /// A parameter's `default` changed. What an omitting caller receives changes
    /// — but so does what any `WHERE` clause edit changes, and for the same
    /// reason that is data rather than contract.
    ParameterDefaultChanged,

    // ---- limits --------------------------------------------------------
    /// `limits.max_rows` was lowered. A result over the cap is
    /// `row_limit_exceeded` — an ERROR, never a truncated 200 (see
    /// `ServingError::RowLimitExceeded`) — so a call that returned rows can now
    /// fail outright.
    RowLimitLowered,
    /// `limits.max_rows` was raised.
    RowLimitRaised,
    /// `limits.timeout_seconds` was lowered, so a query that finished in time
    /// may now be cut off.
    TimeoutLowered,
    /// `limits.timeout_seconds` was raised.
    TimeoutRaised,
    /// A rate limit appeared where there was none, or an existing one was
    /// lowered: a caller sending exactly what it sent yesterday now gets 429s.
    RateLimitTightened,
    /// A rate limit was raised or removed.
    RateLimitRelaxed,

    // ---- neither the shape nor the promise -----------------------------
    /// The response cache's enablement, TTL or scope changed. Invisible to a
    /// consumer beyond `meta.cached`, which describes THIS call and promises
    /// nothing about the next.
    CacheChanged,
    /// The endpoint was pointed at a different connection. What a consumer
    /// receives is decided by the columns, which are diffed on their own.
    DatabaseChanged,
}

impl ChangeKind {
    /// The single classification table.
    ///
    /// Every `true` here follows from the module comment's one sentence; if a
    /// future variant cannot be argued from it, the sentence is what has to
    /// change first.
    pub fn is_breaking(self) -> bool {
        match self {
            // Refused: the request no longer resolves, or no longer coerces, or
            // trips a limit it did not trip before.
            Self::MethodChanged
            | Self::PathChanged
            | Self::RequiredParameterAdded
            | Self::ParameterBecameRequired
            | Self::ParameterTypeChanged
            | Self::ParameterLocationChanged
            | Self::ParameterConstraintTightened
            | Self::RowLimitLowered
            | Self::TimeoutLowered
            | Self::RateLimitTightened => true,
            // Silently ignored.
            Self::ParameterRemoved => true,
            // A body their existing code cannot read.
            Self::EnvelopeFlipped
            | Self::EnvelopeWrapperChanged
            | Self::RowShapeChanged
            | Self::NamingChanged
            | Self::TransformPresenceChanged
            | Self::StatementRemoved
            | Self::ColumnRemoved
            | Self::ColumnTypeChanged
            | Self::ColumnBecameOptional
            | Self::ColumnOrderChanged => true,
            // None of the three.
            Self::TransformScriptChanged
            | Self::StatementAdded
            | Self::StatementDefinitionChanged
            | Self::ColumnAdded
            | Self::ColumnBecameRequired
            | Self::ParameterAdded
            | Self::ParameterBecameOptional
            | Self::ParameterConstraintRelaxed
            | Self::ParameterDefaultChanged
            | Self::RowLimitRaised
            | Self::TimeoutRaised
            | Self::RateLimitRelaxed
            | Self::CacheChanged
            | Self::DatabaseChanged => false,
        }
    }
}

/// One difference, in the words an author has to act on.
///
/// There is deliberately **no `breaking` field**. Which list a change is in IS
/// its classification ([`ContractDiff`]), so a flag and a kind cannot drift into
/// disagreeing — the failure mode of every "carry the answer beside the
/// question" shape this codebase has removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ContractChange {
    pub kind: ChangeKind,
    /// What the change is ABOUT: `column `total` of statement `rows``, or
    /// `parameter `region``. Free text for a reader; it is also part of the
    /// acknowledgement digest, so two changes of one kind on two different
    /// subjects cannot be acknowledged by one token.
    pub subject: String,
    /// One sentence naming the old value and the new one. The author is entitled
    /// to know exactly what to fix — a publish-time refusal is an AUTHORING
    /// error, unlike a serving error, where naming the definition would be
    /// context disclosure to a consumer.
    pub detail: String,
}

impl ContractChange {
    fn new(kind: ChangeKind, subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            subject: subject.into(),
            detail: detail.into(),
        }
    }
}

/// The comparison of a prospective version against the one being served.
///
/// Two lists, not one list with a flag: see [`ContractChange`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ContractDiff {
    /// Changes a deployed consumer cannot survive. Non-empty means publish
    /// refuses without an acknowledgement.
    pub breaking: Vec<ContractChange>,
    /// Changes that were looked at and judged survivable. Reported rather than
    /// dropped, so "we found nothing" is distinguishable from "we found six
    /// things and none of them matter" — an author who sees the safe list is an
    /// author who can tell that the breaking list being empty means something.
    pub safe: Vec<ContractChange>,
}

impl ContractDiff {
    fn breaks(&mut self, kind: ChangeKind, subject: impl Into<String>, detail: impl Into<String>) {
        let change = ContractChange::new(kind, subject, detail);
        // The kind decides the list. A caller cannot file a change into the
        // wrong one, because a caller does not choose.
        if change.kind.is_breaking() {
            self.breaking.push(change);
        } else {
            self.safe.push(change);
        }
    }

    /// The token that acknowledges EXACTLY these breaking changes, or `None`
    /// when there are none to acknowledge.
    ///
    /// # Why a digest and not a `force: true`
    ///
    /// Because a boolean can be set once, in a script, by someone who has never
    /// read a diff — and then every future breaking publish from that script is
    /// pre-approved, forever, silently. That is the "relies on a caller
    /// remembering a flag" shape the package brief rules out.
    ///
    /// This token is a function of the breaking changes themselves, so it is
    /// **unforgeable without having been handed them**: there is no constant a
    /// client can hardcode, and a token obtained for yesterday's break does not
    /// match today's. The endpoint id and the live version id are mixed in so a
    /// token cannot be replayed against a different endpoint, or against the
    /// same endpoint after a rollback moved which version is live.
    ///
    /// Length-prefixed, for the same reason `serving::cache`'s key is: without
    /// it, two different change lists can concatenate to the same bytes.
    pub fn acknowledgement(&self, endpoint: Uuid, live_version: Uuid) -> Option<String> {
        if self.breaking.is_empty() {
            return None;
        }
        let mut h = Sha256::new();
        h.update(b"egret-dataapi/publish-break-ack/v1");
        h.update(endpoint.as_bytes());
        h.update(live_version.as_bytes());
        h.update((self.breaking.len() as u64).to_be_bytes());
        for c in &self.breaking {
            // The `kind` through its serialised name rather than through its
            // discriminant: the name is what is persisted and what the console
            // shows, and a discriminant would move the token every time a
            // variant is inserted in the middle of the enum.
            let kind = serde_json::to_string(&c.kind).unwrap_or_default();
            for field in [kind.as_str(), c.subject.as_str(), c.detail.as_str()] {
                h.update((field.len() as u64).to_be_bytes());
                h.update(field.as_bytes());
            }
        }
        let mut out = String::with_capacity(64);
        for byte in h.finalize() {
            out.push(char::from_digit((byte >> 4) as u32, 16).expect("nibble"));
            out.push(char::from_digit((byte & 0x0f) as u32, 16).expect("nibble"));
        }
        Some(out)
    }

    /// Is this publish allowed to proceed with what the author sent?
    ///
    /// The whole point of the package is the REFUSAL, so this is written as the
    /// gate rather than as a helper the caller may or may not call: `publish`
    /// has no other path to the insert.
    pub fn check_acknowledgement(
        &self,
        endpoint: Uuid,
        live_version: Uuid,
        supplied: Option<&str>,
    ) -> Result<(), AckRefusal> {
        let expected = self.acknowledgement(endpoint, live_version);
        match (expected, supplied) {
            (None, None) => Ok(()),
            (None, Some(_)) => Err(AckRefusal::NothingToAcknowledge),
            (Some(_), None) => Err(AckRefusal::NotAcknowledged {
                changes: self.breaking.clone(),
            }),
            (Some(want), Some(got)) if want == got => Ok(()),
            (Some(_), Some(_)) => Err(AckRefusal::StaleAcknowledgement {
                changes: self.breaking.clone(),
            }),
        }
    }
}

/// Why a publish carrying breaking changes was refused.
///
/// Its `Display` is what the author reads, and it names every break — a
/// publish-time refusal is an authoring error, and an author who is told "this
/// breaks consumers" without being told WHICH change did it cannot act on it.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AckRefusal {
    #[error(
        "publishing this version would break consumers of the one currently live, and the \
         publish did not acknowledge it:\n{}\n\nRun `POST …/validate` to obtain the \
         acknowledgement for exactly these changes and send it as `acknowledge_breaking`, or \
         change the definition so none of them is necessary.",
        render(changes)
    )]
    NotAcknowledged { changes: Vec<ContractChange> },
    #[error(
        "the acknowledgement sent does not match the breaking changes this publish would \
         actually make — the definition or the live version moved since it was issued:\n{}\n\n\
         Run `POST …/validate` again for a current one.",
        render(changes)
    )]
    StaleAcknowledgement { changes: Vec<ContractChange> },
    #[error(
        "an acknowledgement was sent but this publish breaks nothing; it must have been issued \
         for a different definition or before a rollback. Publish again without it."
    )]
    NothingToAcknowledge,
}

fn render(changes: &[ContractChange]) -> String {
    changes
        .iter()
        .map(|c| format!("  - {}: {}", c.subject, c.detail))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Does this snapshot key `data` by statement name — the array-to-object flip?
///
/// **The predicate is `serving::router`'s, not a restatement of it.**
/// `meta_contract` takes its `Single` arm on exactly
/// `statements.len() == 1 && output.transform.is_none()`, and `statements_data`
/// unwraps a lone statement's rows onto `data`. Both are quoted in the module
/// comment. Anything that moves this boolean changes the top-level shape of
/// every response, which is why it is one function with one caller rather than
/// two comparisons that could disagree.
fn keyed_envelope(s: &EndpointSnapshot) -> bool {
    !(s.statements.len() == 1 && s.output.transform.is_none())
}

/// Compare the version a consumer is receiving against the one about to be
/// frozen.
///
/// `live` is the snapshot reached through `endpoints.live_version_id` — **not**
/// `MAX(version)`. A rollback makes an OLDER version live, and a diff against
/// the newest row would then describe a contract nobody is calling.
pub fn compare_contracts(live: &EndpointSnapshot, next: &EndpointSnapshot) -> ContractDiff {
    let mut d = ContractDiff::default();

    if live.method != next.method {
        d.breaks(
            ChangeKind::MethodChanged,
            "the endpoint's method",
            format!("`{}` becomes `{}`", live.method, next.method),
        );
    }
    if live.path != next.path {
        d.breaks(
            ChangeKind::PathChanged,
            "the endpoint's path",
            format!("`{}` becomes `{}`", live.path, next.path),
        );
    }
    if live.database_id != next.database_id {
        d.breaks(
            ChangeKind::DatabaseChanged,
            "the endpoint's connection",
            "the statements run against a different database; what a consumer receives is \
             decided by the columns, which are compared on their own",
        );
    }
    if live.cache != next.cache {
        d.breaks(
            ChangeKind::CacheChanged,
            "the response cache",
            "enablement, TTL or scope changed; a consumer sees this only as `meta.cached`",
        );
    }

    compare_output(live, next, &mut d);
    compare_statements(live, next, &mut d);
    compare_params(&live.params, &next.params, &mut d);
    compare_limits(live, next, &mut d);

    d
}

fn compare_output(live: &EndpointSnapshot, next: &EndpointSnapshot, d: &mut ContractDiff) {
    if keyed_envelope(live) != keyed_envelope(next) {
        let (from, to) = if keyed_envelope(next) {
            ("an array of rows", "an object keyed by statement name")
        } else {
            ("an object keyed by statement name", "an array of rows")
        };
        d.breaks(
            ChangeKind::EnvelopeFlipped,
            "the top-level `data`",
            format!(
                "`data` becomes {to} instead of {from}, and `meta` changes with it. This is the \
                 whole response body for every consumer, not an addition to it"
            ),
        );
    }
    if live.output.envelope != next.output.envelope {
        d.breaks(
            ChangeKind::EnvelopeWrapperChanged,
            "the response envelope",
            format!(
                "`{}` becomes `{}`, so the `{{status, message, data, meta}}` wrapper is added or \
                 removed",
                live.output.envelope, next.output.envelope
            ),
        );
    }

    match (&live.output.transform, &next.output.transform) {
        (None, None) => {
            // `shape` and `naming` decide how `data` is WRITTEN, and they are
            // inert under a transform (`serving::router::output_for` shapes a
            // transformed endpoint `objects`/`as_is` whatever they say). So they
            // are only compared when neither side has one — reporting them for a
            // transformed endpoint would be a false break, which is the failure
            // mode the module comment refuses.
            if live.output.shape != next.output.shape {
                d.breaks(
                    ChangeKind::RowShapeChanged,
                    "the row shape",
                    format!(
                        "`{}` becomes `{}`: every row changes between a JSON object and a \
                         positional array",
                        live.output.shape, next.output.shape
                    ),
                );
            }
            if live.output.naming != next.output.naming {
                d.breaks(
                    ChangeKind::NamingChanged,
                    "the column naming",
                    format!(
                        "`{}` becomes `{}`: every key in every row is spelled differently",
                        live.output.naming, next.output.naming
                    ),
                );
            }
        }
        (Some(_), None) | (None, Some(_)) => {
            let to = if next.output.transform.is_some() {
                "whatever the transform returns"
            } else {
                "the statements' rows"
            };
            d.breaks(
                ChangeKind::TransformPresenceChanged,
                "the response transform",
                format!("`data` becomes {to}"),
            );
        }
        (Some(a), Some(b)) => {
            // `returns` IS the contract of a transformed endpoint — it is what
            // `transform::check_returns` validates every response against — so it
            // is compared exactly as a statement's frozen columns are.
            compare_columns("the transform output", &a.returns, &b.returns, false, d);
            if a.script != b.script {
                d.breaks(
                    ChangeKind::TransformScriptChanged,
                    "the response transform",
                    "the script changed but its declared `returns` did not",
                );
            }
        }
    }
}

fn compare_statements(live: &EndpointSnapshot, next: &EndpointSnapshot, d: &mut ContractDiff) {
    let find = |list: &[StatementSnapshot], name: &str| -> Option<StatementSnapshot> {
        list.iter().find(|s| s.name == name).cloned()
    };
    for old in &live.statements {
        match find(&next.statements, &old.name) {
            None => d.breaks(
                ChangeKind::StatementRemoved,
                format!("statement `{}`", old.name),
                "its key disappears from `data` and from `meta.statements`",
            ),
            Some(new) => {
                // Under a transform the statements' rows never reach the wire —
                // but their frozen columns are still published in
                // `meta.statements.<name>.columns` (`meta_contract`'s `Several`
                // arm), so they are still an advertised contract and are still
                // compared. What does NOT apply is the positional rule: a
                // transform is handed named objects, so there is no index for a
                // reorder to move.
                let positional = next.output.shape == "arrays" && next.output.transform.is_none();
                compare_columns(
                    &format!("statement `{}`", old.name),
                    &old.columns,
                    &new.columns,
                    positional,
                    d,
                );
                if old.source != new.source {
                    d.breaks(
                        ChangeKind::StatementDefinitionChanged,
                        format!("statement `{}`", old.name),
                        "its SQL or `build_sql` script changed; the columns it resolves to are \
                         compared separately",
                    );
                }
            }
        }
    }
    for new in &next.statements {
        if find(&live.statements, &new.name).is_none() {
            d.breaks(
                ChangeKind::StatementAdded,
                format!("statement `{}`", new.name),
                "a new key under `data`; on its own a consumer reading the keys it already knows \
                 is unaffected",
            );
        }
    }
}

/// Compare one frozen column list against another.
///
/// `subject` names what the list belongs to — a statement, or the transform's
/// `returns` — because an endpoint has one list per statement and a change has
/// to say which.
///
/// `positional` is `shape: arrays` with no transform, where `shape_rows` zips
/// the live and frozen lists by INDEX: there, a surviving column that moved is a
/// different value at an index a consumer already reads.
fn compare_columns(
    subject: &str,
    live: &[ColumnSpec],
    next: &[ColumnSpec],
    positional: bool,
    d: &mut ContractDiff,
) {
    for old in live {
        let Some(new) = next.iter().find(|c| c.name == old.name) else {
            d.breaks(
                ChangeKind::ColumnRemoved,
                format!("column `{}` of {subject}", old.name),
                format!("declared `{}` and is gone", old.ty),
            );
            continue;
        };
        // The SAME comparison serving answers `503 schema_drift` with, so a
        // change this surface calls safe cannot be one that refuses at runtime.
        if compare_column_types(&old.ty, &new.ty) == TypeAgreement::No {
            d.breaks(
                ChangeKind::ColumnTypeChanged,
                format!("column `{}` of {subject}", old.name),
                format!("`{}` becomes `{}`", old.ty, new.ty),
            );
        }
        if old.required && !new.required {
            d.breaks(
                ChangeKind::ColumnBecameOptional,
                format!("column `{}` of {subject}", old.name),
                "was present in every described permutation and may now be absent from some",
            );
        }
        if !old.required && new.required {
            d.breaks(
                ChangeKind::ColumnBecameRequired,
                format!("column `{}` of {subject}", old.name),
                "could be absent and is now present in every described permutation",
            );
        }
    }
    for new in next {
        if !live.iter().any(|c| c.name == new.name) {
            d.breaks(
                ChangeKind::ColumnAdded,
                format!("column `{}` of {subject}", new.name),
                format!("new, declared `{}`", new.ty),
            );
        }
    }
    if positional {
        // Only the SURVIVING columns, and only their order relative to each
        // other. A column removed has already been reported, and an APPENDED
        // column moves no existing index — which is the common, safe way a
        // column list grows, and flagging it would be exactly the over-broad
        // rule that trains an author to click through.
        let old_order: Vec<&str> = live.iter().map(|c| c.name.as_str()).collect();
        let new_order: Vec<&str> = next
            .iter()
            .map(|c| c.name.as_str())
            .filter(|n| old_order.contains(n))
            .collect();
        let surviving: Vec<&str> = old_order
            .iter()
            .copied()
            .filter(|n| next.iter().any(|c| c.name == *n))
            .collect();
        if surviving != new_order {
            d.breaks(
                ChangeKind::ColumnOrderChanged,
                subject.to_string(),
                format!(
                    "`shape: arrays` rows are positional, and the surviving columns move from \
                     [{}] to [{}]",
                    surviving.join(", "),
                    new_order.join(", ")
                ),
            );
        }
    }
}

fn compare_params(live: &[ParamSpec], next: &[ParamSpec], d: &mut ContractDiff) {
    for old in live {
        let Some(new) = next.iter().find(|p| p.name == old.name) else {
            d.breaks(
                ChangeKind::ParameterRemoved,
                format!("parameter `{}`", old.name),
                "callers still sending it are NOT refused — an undeclared name is never read — so \
                 whatever it selected or filtered silently stops applying",
            );
            continue;
        };
        if old.ty != new.ty {
            d.breaks(
                ChangeKind::ParameterTypeChanged,
                format!("parameter `{}`", old.name),
                format!("`{:?}` becomes `{:?}`", old.ty, new.ty).to_lowercase(),
            );
        }
        if old.location != new.location {
            d.breaks(
                ChangeKind::ParameterLocationChanged,
                format!("parameter `{}`", old.name),
                format!("`{:?}` becomes `{:?}`", old.location, new.location).to_lowercase(),
            );
        }
        // Requiredness is only refusable when there is no `default` to fall back
        // on: `resolve_supplied_with` answers `param_missing` on
        // `(no value, no default, required)` and on nothing else.
        let was_refusable = old.required && old.default.is_none();
        let is_refusable = new.required && new.default.is_none();
        if !was_refusable && is_refusable {
            d.breaks(
                ChangeKind::ParameterBecameRequired,
                format!("parameter `{}`", old.name),
                "every call that omitted it now answers `param_missing`",
            );
        }
        if was_refusable && !is_refusable {
            d.breaks(
                ChangeKind::ParameterBecameOptional,
                format!("parameter `{}`", old.name),
                "a call that omits it is no longer refused",
            );
        }
        if old.default != new.default && was_refusable == is_refusable {
            d.breaks(
                ChangeKind::ParameterDefaultChanged,
                format!("parameter `{}`", old.name),
                format!(
                    "the value bound when a caller says nothing changes from {} to {}",
                    quoted(old.default.as_deref()),
                    quoted(new.default.as_deref())
                ),
            );
        }
        compare_constraints(old, new, d);
    }
    for new in next {
        if live.iter().any(|p| p.name == new.name) {
            continue;
        }
        if new.required && new.default.is_none() {
            d.breaks(
                ChangeKind::RequiredParameterAdded,
                format!("parameter `{}`", new.name),
                "new, required, and with no `default` — every existing call answers \
                 `param_missing`",
            );
        } else {
            d.breaks(
                ChangeKind::ParameterAdded,
                format!("parameter `{}`", new.name),
                "new, and a call that omits it is not refused",
            );
        }
    }
}

fn quoted(v: Option<&str>) -> String {
    match v {
        Some(s) => format!("`{s}`"),
        None => "none".to_string(),
    }
}

/// One parameter's declared constraints, each judged tighter, looser or
/// unchanged.
///
/// **Comparison, not inequality.** The bounds are decimal STRINGS on purpose
/// (`ParamSpec::minimum`'s own comment: a decimal through an `f64` stops
/// comparing equal to itself), so they are ordered with
/// `serving::params::compare_decimal_text` — the same function `coerce`
/// enforces them with. Temporal bounds go through
/// `serving::params::normalize_temporal_bound` first, for the same reason
/// `check_constraints` does: `2026-1-1` and `2026-01-01` are the same instant
/// and must not read as a change.
///
/// Judging direction rather than flagging every edit is what keeps this from
/// being the over-broad rule the module comment warns about: raising a
/// `maximum` from 100 to 1000 accepts strictly more values and refuses nobody,
/// and an author who is warned about it learns to ignore the warning.
fn compare_constraints(old: &ParamSpec, new: &ParamSpec, d: &mut ContractDiff) {
    let subject = format!("parameter `{}`", old.name);
    let mut judge = |name: &str, tighter: bool, detail: String| {
        d.breaks(
            if tighter {
                ChangeKind::ParameterConstraintTightened
            } else {
                ChangeKind::ParameterConstraintRelaxed
            },
            subject.clone(),
            format!("`{name}` {detail}"),
        );
    };

    // A LOWER bound: appearing or rising refuses values that were accepted.
    for (name, a, b) in [
        ("minimum", &old.minimum, &new.minimum),
        (
            "min_length",
            &fmt_u32(old.min_length),
            &fmt_u32(new.min_length),
        ),
    ] {
        if let Some(dir) = bound_direction(a.as_deref(), b.as_deref(), decimal_cmp) {
            judge(name, dir.tighter_as_lower_bound(), dir.detail(a, b));
        }
    }
    // An UPPER bound: appearing or falling refuses values that were accepted.
    for (name, a, b) in [
        ("maximum", &old.maximum, &new.maximum),
        (
            "max_length",
            &fmt_u32(old.max_length),
            &fmt_u32(new.max_length),
        ),
    ] {
        if let Some(dir) = bound_direction(a.as_deref(), b.as_deref(), decimal_cmp) {
            judge(name, dir.tighter_as_upper_bound(), dir.detail(a, b));
        }
    }
    // The temporal pair, normalised through the parser `coerce` uses. An
    // unparseable bound cannot reach a published snapshot — `check_constraints`
    // refuses it — so the fallback below (compare the raw text) exists only so
    // this function is total, and it errs toward reporting.
    let temporal = |v: Option<&String>| {
        v.map(|s| normalize_temporal_bound(&old.ty, s).unwrap_or_else(|| s.clone()))
    };
    if let Some(dir) = bound_direction(
        temporal(old.not_before.as_ref()).as_deref(),
        temporal(new.not_before.as_ref()).as_deref(),
        |a, b| a.cmp(b),
    ) {
        judge(
            "not_before",
            dir.tighter_as_lower_bound(),
            dir.detail(&old.not_before, &new.not_before),
        );
    }
    if let Some(dir) = bound_direction(
        temporal(old.not_after.as_ref()).as_deref(),
        temporal(new.not_after.as_ref()).as_deref(),
        |a, b| a.cmp(b),
    ) {
        judge(
            "not_after",
            dir.tighter_as_upper_bound(),
            dir.detail(&old.not_after, &new.not_after),
        );
    }

    // A pattern is a LANGUAGE, and two regexes have no order: there is no way to
    // decide that `^[a-z]+$` accepts a superset of `^[a-z]{2,}$` short of
    // deciding regular-language inclusion. So only the two unambiguous
    // directions are judged — appearing where there was none is strictly fewer
    // values, disappearing is strictly more — and an EDIT is reported as
    // tightened, which is the direction that shows the author something rather
    // than hiding it.
    match (&old.pattern, &new.pattern) {
        (None, Some(p)) => judge("pattern", true, format!("appears as `{p}`")),
        (Some(p), None) => judge("pattern", false, format!("`{p}` is removed")),
        (Some(a), Some(b)) if a != b => judge(
            "pattern",
            true,
            format!(
                "changes from `{a}` to `{b}`; two regexes have no order, so this is reported as \
                 a tightening"
            ),
        ),
        _ => {}
    }

    // An `allowed` list is a set, and a set DOES have an order.
    match (&old.allowed, &new.allowed) {
        (None, Some(v)) => judge(
            "allowed",
            true,
            format!(
                "appears, restricting the value to one of [{}]",
                v.join(", ")
            ),
        ),
        (Some(v), None) => judge("allowed", false, format!("[{}] is removed", v.join(", "))),
        (Some(a), Some(b)) => {
            let gone: Vec<&str> = a
                .iter()
                .filter(|v| !b.contains(v))
                .map(String::as_str)
                .collect();
            let added: Vec<&str> = b
                .iter()
                .filter(|v| !a.contains(v))
                .map(String::as_str)
                .collect();
            if !gone.is_empty() {
                judge(
                    "allowed",
                    true,
                    format!("no longer accepts [{}]", gone.join(", ")),
                );
            } else if !added.is_empty() {
                judge(
                    "allowed",
                    false,
                    format!("also accepts [{}]", added.join(", ")),
                );
            }
        }
        (None, None) => {}
    }
}

fn fmt_u32(v: Option<u32>) -> Option<String> {
    v.map(|n| n.to_string())
}

fn decimal_cmp(a: &str, b: &str) -> Ordering {
    compare_decimal_text(a, b)
}

/// Which way a single bound moved, kept abstract so the "appeared" and
/// "disappeared" cases are answered once instead of once per bound.
enum BoundMove {
    Appeared,
    Disappeared,
    Rose,
    Fell,
}

impl BoundMove {
    fn tighter_as_lower_bound(&self) -> bool {
        matches!(self, Self::Appeared | Self::Rose)
    }
    fn tighter_as_upper_bound(&self) -> bool {
        matches!(self, Self::Appeared | Self::Fell)
    }
    fn detail(&self, a: &Option<String>, b: &Option<String>) -> String {
        match self {
            Self::Appeared => format!("appears as {}", quoted(b.as_deref())),
            Self::Disappeared => format!("{} is removed", quoted(a.as_deref())),
            Self::Rose | Self::Fell => format!(
                "moves from {} to {}",
                quoted(a.as_deref()),
                quoted(b.as_deref())
            ),
        }
    }
}

fn bound_direction(
    a: Option<&str>,
    b: Option<&str>,
    cmp: impl Fn(&str, &str) -> Ordering,
) -> Option<BoundMove> {
    match (a, b) {
        (None, None) => None,
        (None, Some(_)) => Some(BoundMove::Appeared),
        (Some(_), None) => Some(BoundMove::Disappeared),
        (Some(x), Some(y)) => match cmp(x, y) {
            Ordering::Equal => None,
            Ordering::Less => Some(BoundMove::Rose),
            Ordering::Greater => Some(BoundMove::Fell),
        },
    }
}

fn compare_limits(live: &EndpointSnapshot, next: &EndpointSnapshot, d: &mut ContractDiff) {
    if next.limits.max_rows < live.limits.max_rows {
        d.breaks(
            ChangeKind::RowLimitLowered,
            "`limits.max_rows`",
            format!(
                "{} becomes {}. A result over the cap is refused outright — \
                 `row_limit_exceeded`, never a truncated 200 — so a call that returned rows can \
                 now fail",
                live.limits.max_rows, next.limits.max_rows
            ),
        );
    } else if next.limits.max_rows > live.limits.max_rows {
        d.breaks(
            ChangeKind::RowLimitRaised,
            "`limits.max_rows`",
            format!("{} becomes {}", live.limits.max_rows, next.limits.max_rows),
        );
    }
    if next.limits.timeout_seconds < live.limits.timeout_seconds {
        d.breaks(
            ChangeKind::TimeoutLowered,
            "`limits.timeout_seconds`",
            format!(
                "{}s becomes {}s, so a query that finished in time may now be cut off",
                live.limits.timeout_seconds, next.limits.timeout_seconds
            ),
        );
    } else if next.limits.timeout_seconds > live.limits.timeout_seconds {
        d.breaks(
            ChangeKind::TimeoutRaised,
            "`limits.timeout_seconds`",
            format!(
                "{}s becomes {}s",
                live.limits.timeout_seconds, next.limits.timeout_seconds
            ),
        );
    }

    // `0` is "no limit" on all three fields, so the comparison is not a plain
    // `<`: going from 0 to 5 is the tightest change any of them can make, and a
    // numeric comparison reads it as a relaxation.
    //
    // The per-key pair is judged TOGETHER because they are one limiter: a bucket
    // with `qps: 0` is off whatever its burst says, so a burst change on an
    // inactive limiter is not a change to anything.
    let (lq, lb) = (live.rate_limit.per_key_qps, live.rate_limit.burst);
    let (nq, nb) = (next.rate_limit.per_key_qps, next.rate_limit.burst);
    if (lq, lb) != (nq, nb) {
        let tightened = nq > 0 && (lq == 0 || nq < lq || nb < lb);
        d.breaks(
            if tightened {
                ChangeKind::RateLimitTightened
            } else {
                ChangeKind::RateLimitRelaxed
            },
            "the per-key rate limit",
            format!("{lq} qps / burst {lb} becomes {nq} qps / burst {nb}"),
        );
    }
    let (le, ne) = (
        live.rate_limit.per_endpoint_qps,
        next.rate_limit.per_endpoint_qps,
    );
    if le != ne {
        let tightened = ne > 0 && (le == 0 || ne < le);
        d.breaks(
            if tightened {
                ChangeKind::RateLimitTightened
            } else {
                ChangeKind::RateLimitRelaxed
            },
            "the per-endpoint rate limit",
            format!("{le} qps becomes {ne} qps"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_dataapi_api::{
        CacheSpec, LimitsSpec, OutputSpec, ParamLocation, ParamType, RateLimitSpec,
        StatementSource, TransformSpec,
    };

    fn col(name: &str, ty: &str) -> ColumnSpec {
        ColumnSpec {
            name: name.into(),
            ty: ty.into(),
            required: true,
        }
    }

    fn stmt(name: &str, sql: &str, columns: Vec<ColumnSpec>) -> StatementSnapshot {
        StatementSnapshot {
            name: name.into(),
            source: StatementSource::Sql(sql.into()),
            columns,
        }
    }

    fn param(name: &str) -> ParamSpec {
        ParamSpec {
            name: name.into(),
            ty: ParamType::String,
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

    fn snap() -> EndpointSnapshot {
        EndpointSnapshot {
            statements: vec![stmt(
                "main",
                "SELECT id, n FROM t",
                vec![col("id", "uuid"), col("n", "bigint")],
            )],
            iterated: vec![],
            method: "GET".into(),
            path: "/rev".into(),
            database_id: Uuid::nil(),
            params: vec![],
            output: OutputSpec::default(),
            cache: CacheSpec {
                enabled: false,
                ttl_seconds: 60,
                scope: egret_dataapi_api::CacheScope::PerPrincipal,
            },
            limits: LimitsSpec {
                max_rows: 10_000,
                timeout_seconds: 30,
            },
            rate_limit: RateLimitSpec {
                per_key_qps: 0,
                burst: 0,
                per_endpoint_qps: 0,
            },
            delegated: false,
            impersonate: false,
            param_gate: egret_dataapi_api::PARAM_GATE,
        }
    }

    fn kinds(changes: &[ContractChange]) -> Vec<ChangeKind> {
        changes.iter().map(|c| c.kind).collect()
    }

    /// The baseline the whole surface depends on. An over-broad rule fires
    /// HERE first, and an author who is warned on a publish that changes
    /// nothing learns to click through — which the module comment argues is
    /// worse than no warning at all.
    #[test]
    fn an_unchanged_definition_produces_no_change_of_either_kind() {
        let d = compare_contracts(&snap(), &snap());
        assert_eq!(d.breaking, vec![]);
        assert_eq!(d.safe, vec![]);
    }

    /// Editing a `WHERE` clause is not a contract change. If it were, every
    /// publish would be breaking and the acknowledgement would be a formality.
    #[test]
    fn changing_only_the_sql_is_reported_and_is_not_breaking() {
        let mut next = snap();
        next.statements[0].source = StatementSource::Sql("SELECT id, n FROM t WHERE x".into());
        let d = compare_contracts(&snap(), &next);
        assert_eq!(d.breaking, vec![], "a SQL edit must not be breaking");
        assert_eq!(kinds(&d.safe), vec![ChangeKind::StatementDefinitionChanged]);
    }

    /// THE case the package exists for: the edit that looks additive.
    ///
    /// `serving::router::statements_data` unwraps a lone statement's rows onto
    /// `data` and keys them by name once there are two, so a one-line addition
    /// changes the whole body for every deployed consumer.
    #[test]
    fn adding_a_second_statement_flips_the_envelope_and_is_breaking() {
        let mut next = snap();
        next.statements.push(stmt(
            "total",
            "SELECT count(*) AS n",
            vec![col("n", "bigint")],
        ));
        let d = compare_contracts(&snap(), &next);
        assert!(
            kinds(&d.breaking).contains(&ChangeKind::EnvelopeFlipped),
            "adding the second statement must be reported as the envelope flip \
             it is: {:?}",
            d.breaking
        );
        // The addition itself is separately reported and separately safe: at
        // three statements it changes no shape at all.
        assert!(kinds(&d.safe).contains(&ChangeKind::StatementAdded));
    }

    /// The SAME predicate, reached the other way, and the reason
    /// `keyed_envelope` is one function: `meta_contract` takes its keyed arm on
    /// `statements.len() == 1 && transform.is_none()`, so attaching a transform
    /// to a single-statement endpoint flips `data` and `meta` exactly as adding
    /// a statement does.
    #[test]
    fn adding_a_transform_to_a_single_statement_endpoint_flips_the_envelope() {
        let mut next = snap();
        next.output.transform = Some(TransformSpec {
            script: "def transform(params, statements): return {}".into(),
            returns: vec![col("total", "bigint")],
        });
        let d = compare_contracts(&snap(), &next);
        assert!(
            kinds(&d.breaking).contains(&ChangeKind::EnvelopeFlipped),
            "{:?}",
            d.breaking
        );
        assert!(kinds(&d.breaking).contains(&ChangeKind::TransformPresenceChanged));
    }

    /// A three-statement endpoint gaining a fourth flips nothing, and must not
    /// be reported as if it did — the other half of the rule above, and the one
    /// that keeps it from being "any statement change is breaking".
    #[test]
    fn adding_a_statement_to_an_already_keyed_endpoint_flips_nothing() {
        let mut live = snap();
        live.statements.push(stmt(
            "total",
            "SELECT count(*) AS n",
            vec![col("n", "bigint")],
        ));
        let mut next = live.clone();
        next.statements
            .push(stmt("meta", "SELECT 1 AS one", vec![col("one", "integer")]));
        let d = compare_contracts(&live, &next);
        assert_eq!(d.breaking, vec![], "{:?}", d.breaking);
        assert_eq!(kinds(&d.safe), vec![ChangeKind::StatementAdded]);
    }

    #[test]
    fn a_removed_column_is_breaking_and_an_added_one_is_not() {
        let mut next = snap();
        next.statements[0].columns = vec![col("id", "uuid"), col("extra", "varchar")];
        let d = compare_contracts(&snap(), &next);
        assert_eq!(kinds(&d.breaking), vec![ChangeKind::ColumnRemoved]);
        assert!(d.breaking[0].subject.contains("`n`"), "{:?}", d.breaking[0]);
        assert_eq!(kinds(&d.safe), vec![ChangeKind::ColumnAdded]);
    }

    /// The type comparison is `compare_column_types`, not `!=`, and this is what
    /// says so. Trino suppresses a datetime precision unless the caller declares
    /// `PARAMETRIC_DATETIME`, so `timestamp` and `timestamp(0)` are the same
    /// column described two ways — and BOTH spellings are already in the
    /// published corpus. A `!=` here would call every re-publish of such an
    /// endpoint a breaking change.
    #[test]
    fn a_suppressed_datetime_precision_is_not_a_type_change() {
        let mut live = snap();
        live.statements[0].columns = vec![col("t", "timestamp")];
        let mut next = snap();
        next.statements[0].columns = vec![col("t", "timestamp(0)")];
        let d = compare_contracts(&live, &next);
        assert_eq!(d.breaking, vec![], "{:?}", d.breaking);
        assert_eq!(d.safe, vec![]);
    }

    #[test]
    fn a_type_that_no_longer_agrees_is_breaking() {
        let mut next = snap();
        next.statements[0].columns = vec![col("id", "uuid"), col("n", "varchar")];
        let d = compare_contracts(&snap(), &next);
        assert_eq!(kinds(&d.breaking), vec![ChangeKind::ColumnTypeChanged]);
    }

    /// `required` means "present in every described permutation". Losing it
    /// means a consumer that always found the key may now not; gaining it is
    /// strictly more promise.
    #[test]
    fn a_column_that_stops_being_required_is_breaking_and_the_reverse_is_not() {
        let mut next = snap();
        next.statements[0].columns[1].required = false;
        let d = compare_contracts(&snap(), &next);
        assert_eq!(kinds(&d.breaking), vec![ChangeKind::ColumnBecameOptional]);

        let d = compare_contracts(&next, &snap());
        assert_eq!(d.breaking, vec![]);
        assert_eq!(kinds(&d.safe), vec![ChangeKind::ColumnBecameRequired]);
    }

    /// Under `shape: arrays` a row is positional and `shape_rows` zips live
    /// against frozen by INDEX. Appending moves no existing index; reordering
    /// moves every one after the first.
    #[test]
    fn under_arrays_a_reorder_is_breaking_and_an_append_is_not() {
        let mut live = snap();
        live.output.shape = "arrays".into();
        let mut appended = live.clone();
        appended.statements[0].columns.push(col("z", "varchar"));
        let d = compare_contracts(&live, &appended);
        assert!(
            !kinds(&d.breaking).contains(&ChangeKind::ColumnOrderChanged),
            "appending must not be reported as a reorder: {:?}",
            d.breaking
        );

        let mut swapped = live.clone();
        swapped.statements[0].columns = vec![col("n", "bigint"), col("id", "uuid")];
        let d = compare_contracts(&live, &swapped);
        assert_eq!(kinds(&d.breaking), vec![ChangeKind::ColumnOrderChanged]);
    }

    /// The same reorder under `objects` moves nothing a consumer reads, and
    /// reporting it would be a break that is not one.
    #[test]
    fn under_objects_a_reorder_is_not_reported_at_all() {
        let mut swapped = snap();
        swapped.statements[0].columns = vec![col("n", "bigint"), col("id", "uuid")];
        let d = compare_contracts(&snap(), &swapped);
        assert_eq!(d.breaking, vec![], "{:?}", d.breaking);
        assert_eq!(d.safe, vec![]);
    }

    /// The rename case, and the one that is breaking without any request
    /// failing: `resolve_supplied_with` walks the DECLARED specs, so a supplied
    /// name that is no longer declared is never read. `?region=EU` keeps
    /// answering 200 with the filter silently no longer applied.
    #[test]
    fn a_removed_parameter_is_breaking_even_though_no_call_is_refused() {
        let mut live = snap();
        live.params = vec![param("region")];
        let mut next = snap();
        next.params = vec![param("regions")];
        let d = compare_contracts(&live, &next);
        assert!(kinds(&d.breaking).contains(&ChangeKind::ParameterRemoved));
        // And the new one is not itself a break — it is optional, so nothing is
        // refused. The pair together is what a rename is.
        assert!(kinds(&d.safe).contains(&ChangeKind::ParameterAdded));
    }

    /// `resolve_supplied_with` answers `param_missing` only on
    /// `(no value, no default, required)`. A required parameter carrying a
    /// default refuses nobody, so flagging it would be a warning about nothing.
    #[test]
    fn a_new_required_parameter_breaks_only_when_it_has_no_default() {
        let mut bare = snap();
        let mut p = param("region");
        p.required = true;
        bare.params = vec![p.clone()];
        let d = compare_contracts(&snap(), &bare);
        assert_eq!(kinds(&d.breaking), vec![ChangeKind::RequiredParameterAdded]);

        let mut defaulted = snap();
        p.default = Some("EU".into());
        defaulted.params = vec![p];
        let d = compare_contracts(&snap(), &defaulted);
        assert_eq!(d.breaking, vec![], "{:?}", d.breaking);
        assert_eq!(kinds(&d.safe), vec![ChangeKind::ParameterAdded]);
    }

    /// Direction, not difference. Raising a `maximum` accepts strictly more
    /// values and refuses nobody; lowering it turns accepted values into
    /// `param_invalid`. Judged with `compare_decimal_text`, the same comparison
    /// `coerce` enforces the bound with — so a bound this calls safe cannot be
    /// one that refuses at serve time.
    #[test]
    fn a_bound_is_judged_by_direction_not_by_inequality() {
        let mut live = snap();
        let mut p = param("n");
        p.ty = ParamType::Number;
        p.maximum = Some("100".into());
        live.params = vec![p.clone()];

        let mut raised = live.clone();
        raised.params[0].maximum = Some("1000".into());
        let d = compare_contracts(&live, &raised);
        assert_eq!(d.breaking, vec![], "raising a maximum refuses nobody");
        assert_eq!(kinds(&d.safe), vec![ChangeKind::ParameterConstraintRelaxed]);

        let mut lowered = live.clone();
        lowered.params[0].maximum = Some("99.9".into());
        let d = compare_contracts(&live, &lowered);
        assert_eq!(
            kinds(&d.breaking),
            vec![ChangeKind::ParameterConstraintTightened]
        );
    }

    /// A trailing zero is the same bound. Compared as text it is a change; the
    /// decimal comparison is what makes it not one.
    #[test]
    fn a_bound_respelled_with_the_same_value_is_not_a_change() {
        let mut live = snap();
        let mut p = param("n");
        p.ty = ParamType::Number;
        p.minimum = Some("10".into());
        live.params = vec![p];
        let mut next = live.clone();
        next.params[0].minimum = Some("10.00".into());
        let d = compare_contracts(&live, &next);
        assert_eq!(d.breaking, vec![], "{:?}", d.breaking);
        assert_eq!(d.safe, vec![], "{:?}", d.safe);
    }

    /// `0` is "no limit" on every rate-limit field, so the tightest change any
    /// of them can make reads as an increase under a plain `<`.
    #[test]
    fn a_rate_limit_introduced_where_there_was_none_is_breaking() {
        let mut next = snap();
        next.rate_limit.per_key_qps = 5;
        next.rate_limit.burst = 5;
        let d = compare_contracts(&snap(), &next);
        assert_eq!(kinds(&d.breaking), vec![ChangeKind::RateLimitTightened]);

        // And removing it is the relaxation it is.
        let d = compare_contracts(&next, &snap());
        assert_eq!(d.breaking, vec![], "{:?}", d.breaking);
        assert_eq!(kinds(&d.safe), vec![ChangeKind::RateLimitRelaxed]);
    }

    /// `max_rows` is a REFUSAL, not a truncation — `ServingError::RowLimitExceeded`
    /// — so lowering it turns calls that returned rows into failures.
    #[test]
    fn lowering_the_row_limit_is_breaking_and_raising_it_is_not() {
        let mut lower = snap();
        lower.limits.max_rows = 100;
        assert_eq!(
            kinds(&compare_contracts(&snap(), &lower).breaking),
            vec![ChangeKind::RowLimitLowered]
        );
        assert_eq!(compare_contracts(&lower, &snap()).breaking, vec![]);
    }

    /// `shape` and `naming` describe how `data` is WRITTEN, and
    /// `serving::router` shapes a transformed endpoint `objects`/`as_is`
    /// whatever they say. Reporting them there would be a break that cannot
    /// happen.
    #[test]
    fn shape_and_naming_are_inert_under_a_transform_and_are_not_reported() {
        let transform = Some(TransformSpec {
            script: "def transform(params, statements): return {}".into(),
            returns: vec![col("total", "bigint")],
        });
        let mut live = snap();
        live.output.transform = transform.clone();
        let mut next = live.clone();
        next.output.shape = "arrays".into();
        next.output.naming = "camel".into();
        let d = compare_contracts(&live, &next);
        assert_eq!(d.breaking, vec![], "{:?}", d.breaking);

        // Without a transform the same edit is the total rewrite of every row it
        // actually is.
        let mut bare = snap();
        bare.output.shape = "arrays".into();
        bare.output.naming = "camel".into();
        assert_eq!(
            kinds(&compare_contracts(&snap(), &bare).breaking),
            vec![ChangeKind::RowShapeChanged, ChangeKind::NamingChanged]
        );
    }

    /// A transform's `returns` IS the contract of a transformed endpoint, so it
    /// is compared exactly as a statement's frozen columns are — while the
    /// script that produces it is implementation.
    #[test]
    fn a_transforms_returns_is_compared_and_its_script_is_not() {
        let mut live = snap();
        live.output.transform = Some(TransformSpec {
            script: "one".into(),
            returns: vec![col("total", "bigint"), col("label", "varchar")],
        });
        let mut edited = live.clone();
        edited.output.transform.as_mut().unwrap().script = "two".into();
        let d = compare_contracts(&live, &edited);
        assert_eq!(d.breaking, vec![], "{:?}", d.breaking);
        assert_eq!(kinds(&d.safe), vec![ChangeKind::TransformScriptChanged]);

        let mut narrowed = live.clone();
        narrowed.output.transform.as_mut().unwrap().returns = vec![col("total", "bigint")];
        assert_eq!(
            kinds(&compare_contracts(&live, &narrowed).breaking),
            vec![ChangeKind::ColumnRemoved]
        );
    }

    /// The corpus has two snapshot shapes in it and always will. A pre-M2 row
    /// carries `sql` + `columns` and reads back as one statement named `main`;
    /// migration 0129 named every pre-M2 DRAFT's single statement `main` too.
    /// If those two names had not agreed, the first re-publish of every
    /// endpoint predating M2 would have reported a removed statement, an added
    /// one, and — through `keyed_envelope` — nothing else that was true.
    ///
    /// Deserialized from JSON rather than constructed, because the fold is in
    /// `EndpointSnapshotRepr` and constructing the value directly would test
    /// this file against itself.
    #[test]
    fn a_pre_m2_live_snapshot_compares_against_a_republished_one_with_no_changes() {
        let pre_m2 = r#"{
            "sql": "SELECT id, n FROM t",
            "columns": [
                { "name": "id", "ty": "uuid" },
                { "name": "n", "ty": "bigint" }
            ],
            "method": "GET",
            "path": "/rev",
            "database_id": "00000000-0000-0000-0000-000000000000",
            "params": [],
            "output": {},
            "cache": {},
            "limits": {},
            "rate_limit": {},
            "delegated": false
        }"#;
        let live: EndpointSnapshot =
            serde_json::from_str(pre_m2).expect("a pre-M2 snapshot must still read");
        assert_eq!(
            live.statements[0].name,
            egret_dataapi_api::LEGACY_STATEMENT_NAME
        );

        // `snap()` is the M2-shaped re-publish of exactly that definition: one
        // statement, named `main`, the same SQL and the same columns.
        let d = compare_contracts(&live, &snap());
        assert_eq!(d.breaking, vec![], "{:?}", d.breaking);
        assert_eq!(d.safe, vec![], "{:?}", d.safe);
    }

    // ---------- the acknowledgement ----------

    fn removed_column() -> ContractDiff {
        let mut next = snap();
        next.statements[0].columns = vec![col("id", "uuid")];
        compare_contracts(&snap(), &next)
    }

    /// **The property is the refusal.** A test that only asserted a correct
    /// token is ACCEPTED would pass with the whole gate deleted.
    #[test]
    fn a_breaking_publish_with_no_acknowledgement_is_refused() {
        let ep = Uuid::from_u128(1);
        let live = Uuid::from_u128(2);
        let d = removed_column();
        assert!(
            !d.breaking.is_empty(),
            "fixture must actually break something"
        );
        let err = d
            .check_acknowledgement(ep, live, None)
            .expect_err("a breaking publish with no acknowledgement must be refused");
        assert!(matches!(err, AckRefusal::NotAcknowledged { .. }));
        // And the author is told which change it was — a publish-time refusal is
        // an authoring error, so naming the definition is the point rather than
        // a leak.
        assert!(err.to_string().contains("column `n`"), "{err}");
    }

    #[test]
    fn the_acknowledgement_it_issues_is_the_one_it_accepts() {
        let ep = Uuid::from_u128(1);
        let live = Uuid::from_u128(2);
        let d = removed_column();
        let token = d.acknowledgement(ep, live).expect("a break to acknowledge");
        d.check_acknowledgement(ep, live, Some(&token))
            .expect("the token it just issued must be accepted");
    }

    /// The whole reason this is a digest and not a `force: true`: a token is
    /// bound to the CHANGES, so one obtained for a removed column does not
    /// authorise a removed parameter.
    #[test]
    fn an_acknowledgement_for_one_break_does_not_cover_a_different_one() {
        let ep = Uuid::from_u128(1);
        let live = Uuid::from_u128(2);
        let token = removed_column()
            .acknowledgement(ep, live)
            .expect("a break to acknowledge");

        let mut other_next = snap();
        other_next.params = vec![];
        let mut other_live = snap();
        other_live.params = vec![param("region")];
        let other = compare_contracts(&other_live, &other_next);
        assert!(!other.breaking.is_empty());
        assert!(matches!(
            other.check_acknowledgement(ep, live, Some(&token)),
            Err(AckRefusal::StaleAcknowledgement { .. })
        ));
    }

    /// Bound to the endpoint and to the version currently live, so a token
    /// cannot be replayed against another endpoint or across a rollback that
    /// moved which contract is being broken.
    #[test]
    fn an_acknowledgement_is_bound_to_the_endpoint_and_the_live_version() {
        let d = removed_column();
        let a = d.acknowledgement(Uuid::from_u128(1), Uuid::from_u128(2));
        assert_ne!(a, d.acknowledgement(Uuid::from_u128(9), Uuid::from_u128(2)));
        assert_ne!(a, d.acknowledgement(Uuid::from_u128(1), Uuid::from_u128(9)));
    }

    /// There is nothing to acknowledge when nothing breaks, so there is no token
    /// to obtain and none to hardcode.
    #[test]
    fn a_publish_that_breaks_nothing_issues_no_acknowledgement_and_needs_none() {
        let d = compare_contracts(&snap(), &snap());
        assert_eq!(
            d.acknowledgement(Uuid::from_u128(1), Uuid::from_u128(2)),
            None
        );
        d.check_acknowledgement(Uuid::from_u128(1), Uuid::from_u128(2), None)
            .expect("nothing to acknowledge");
        // A token sent anyway is refused rather than ignored: silently accepting
        // one is how a client learns to send it always, which is the `force:
        // true` this shape exists to avoid.
        assert!(matches!(
            d.check_acknowledgement(Uuid::from_u128(1), Uuid::from_u128(2), Some("anything")),
            Err(AckRefusal::NothingToAcknowledge)
        ));
    }
}
