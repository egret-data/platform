//! The channel egret's control plane uses to reach a StarRocks FE: one trait so
//! the whole apply path can be driven in a test without a MySQL port (design
//! D8), the real `mysql_async` runner, the FE coordinates for an engine row,
//! and the `SHOW CATALOGS` / `SHOW CREATE CATALOG` reads that decide whether a
//! catalog is absent, matching or drifted (D3).
//!
//! D6 (addendum): no statement passed through here is ever logged. Scrubbing
//! whole secret occurrences is not enough on its own — an FE error that
//! quotes a *truncated* statement can leak a fragment no substring match
//! catches — so an error out of this module is never the raw FE message: it
//! is [`FeError`], built by [`fe_error`] through a pipeline (normalise
//! `mysql_async`'s own quote convention, redact quoted spans, redact
//! unquoted URL-like and opaque-token shapes, then — the fail-closed step —
//! withhold the excerpt entirely if any quote character survives all of
//! that) before it is capped to [`FE_ERROR_EXCERPT_MAX`], with the FE's own
//! error code pulled out separately. This is the first code to see the FE's
//! raw text, which is why the pipeline lives here.
//!
//! **The guarantee the excerpt makes, stated plainly (fix round 3 — rounds 1
//! and 2 both overstated this, corrected live each time by measured probes;
//! Task 4 corrected the opening clause once more, which still said "never
//! contains a whole secret" and was contradicted by this module's own
//! boundary paragraph below)**: it never contains a secret in any shape this
//! module can RECOGNISE — it never contains a `scheme://…` URL or a
//! 20-plus-character unquoted opaque run, and it never
//! contains a quoted fragment of the FE's echo — INCLUDING a malformed one. A
//! stray apostrophe in prose, a SQL-escaped `\'`, a `\"` (reachable from
//! egret's own `escape_dq`), or an unterminated echo all make the quoting
//! unpairable, and an unpairable span is never trusted on its own: either it
//! gets swept up whole by the opaque-run redaction (the mis-paired remainder
//! plus the `<redacted>` marker glued to it is long enough to cross
//! [`MIN_OPAQUE_RUN`] in every case measured so far), or — when it isn't long
//! enough for that — the fail-closed check finds the leftover quote character
//! and withholds the WHOLE excerpt rather than ship a partial redaction this
//! module cannot prove correct. Either way a secret of THOSE shapes does not
//! survive; which of the two happens is not something a caller should rely on.
//!
//! **What it does NOT guarantee, and cannot, stated as an honest boundary
//! rather than another absolute sentence**: this module has no secrets map,
//! so a SHORT (under [`MIN_OPAQUE_RUN`] characters), entirely UNQUOTED token
//! that merely looks like ordinary prose is invisible to every check here —
//! there is a real, standing shape where such a token reaches
//! the excerpt and, from there, `apply_error`: an FE message that
//! contains a short unquoted secret alongside OTHER, unrelated, correctly
//! paired quoted content elsewhere (so the fail-closed check finds nothing
//! wrong) and no `scheme://` prefix and no adjacent long run to be swept up
//! by (so neither redaction pass reaches it either). Nothing observed on a
//! live 4.1.4 FE takes this shape — StarRocks always quotes what it echoes —
//! but this module cannot prove no FE message ever will. Closing THAT needs
//! the caller's own resolved secret values, which is exactly why
//! [`FeError::scrubbed`] is the ONLY way out of this type: Task 3 wrote that
//! obligation down and Task 4 made it structural — the excerpt is private,
//! `scrubbed(&secrets)` is the only accessor that yields it, and `Display`
//! carries the FE's error code alone, so `apply_error = e.to_string()` can no
//! longer compile into a leak. Every caller that resolves a catalog's secrets
//! and writes `apply_error` (or an API response) runs it.
//!
//! SP3b Task 4 wired the apply path onto this seam:
//! `catalog_apply::{apply_starrocks, converge_attachment}` construct a
//! [`FeTarget`] through [`fe_target`] and reach the FE only through
//! [`StarRocksDdl`], so D6's pinned certificate is now *applied*, not merely
//! implemented. Task 5 finished the job: `engines::spawn_starrocks_catalog_wire`
//! — its root-login probe, its per-attachment convergence and its delegation
//! statements — comes through here too, and the `run_starrocks_ddl` it used to
//! dial `mysql_async` with is deleted. [`MysqlDdlRunner`] is the only place in
//! egret that opens a connection to a StarRocks FE for a statement.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_catalog_registry::{escape_dq, escape_ident, StarRocksCatalogDdl};
use egret_query_api::catalog_listing::{
    is_hidden_schema, relation_kind, CatalogLevel, ListedColumn, ListedRelation,
};

use crate::Engine;
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;

/// Where and how to reach a StarRocks FE.
///
/// Every field is `pub`: [`fe_target`] is the only SANCTIONED PRODUCTION
/// constructor (it is the one place D6's cert-gating decision is made), but
/// the type itself cannot enforce that — a hand-built value with
/// `tls_cert_pem: None` compiles fine, and the live suite legitimately does
/// exactly that for a plaintext dev FE (`tests/starrocks_ddl_live.rs`).
/// Tightening this into a real invariant (private fields + a fallible
/// constructor, or `#[non_exhaustive]`) would also have to give tests a
/// second, non-`fe_target` way to build one for a dev/plaintext target, which
/// is more surface than this fix round buys; noted rather than done
/// (fix round 1 review, F-10).
#[derive(Clone, Debug)]
pub struct FeTarget {
    pub host: String,
    pub port: u16,
    pub root_password: Option<String>,
    /// The engine's pinned FE certificate. `Some` ⇒ the connection runs over
    /// TLS with this cert as its only trust root and domain validation off
    /// (the cert's CN cannot match the in-cluster Service DNS egret dials) —
    /// the same posture `StarRocksConnector::new` and the delegation bootstrap
    /// already use, and which [`fe_target`] now applies to every statement the
    /// APPLY path sends (D6). The bootstrap in `engines.rs` still dials
    /// `mysql_async` directly and is not covered by that yet; Task 5 moves it.
    pub tls_cert_pem: Option<String>,
}

impl FeTarget {
    /// The secrets this target's own fields carry, keyed the way
    /// [`FeError::scrubbed`] wants them: synthetically, since `scrub_secrets`
    /// reads only the values. Today that is just the root password every
    /// statement on this connection logs in with.
    ///
    /// **PRIVATE, and that is the fix.** It was `pub`, with a doc telling
    /// callers to build their scrub map from here — and the root password was
    /// then left out of a scrub map TWICE: once by the delegation bootstrap
    /// (fix round 1, F3) and once by the apply path Task 4 wrote *after* that
    /// fix (final whole-branch review, F2). A rule forgotten twice will be
    /// forgotten again, so it stopped being a rule: [`FeError::scrubbed`] now
    /// takes the `&FeTarget` itself and reads this, there is no other way to
    /// obtain the map, and a caller that could omit it no longer type-checks.
    fn secrets(&self) -> BTreeMap<uuid::Uuid, String> {
        self.root_password
            .iter()
            .map(|pw| (uuid::Uuid::nil(), pw.clone()))
            .collect()
    }
}

/// How much of the FE's (already redacted) error text a [`FeError`]
/// keeps, once the whole [`fe_error`] pipeline has decided the text is not
/// withheld. Long enough to be diagnostic — the FE's own `ERROR <code>
/// (<sqlstate>): …` preamble is well under this — short enough to bound how
/// much unquoted prose an unusually verbose FE message can carry. **This cap
/// is a secondary bound, not the mechanism that keeps a secret out**: on its
/// own it does nothing against a short credential that falls entirely inside
/// it (measured live: a real FE's `'S3CRET'` fragment, a prefix of a 16-char
/// secret, lands around character 130 of a 189-char message — well inside
/// 200 either way). Redaction is what actually closes that, and it MUST run
/// on the RAW, uncapped text before this cap is applied: a quoted span that
/// straddles the cap boundary would otherwise leave a truncated, unredacted
/// prefix in the capped output with no closing quote left for a later pass
/// to find.
pub const FE_ERROR_EXCERPT_MAX: usize = 200;

/// What a redacted fragment is replaced with.
const REDACTED: &str = "<redacted>";

/// What the excerpt becomes when the fail-closed check in [`fe_error`] finds
/// a quote character survived every redaction pass — the text was not
/// something this module could reason about, so no excerpt is a smaller harm
/// than a credential in `apply_error`. The FE error code, when the raw text
/// had one, is still preserved on [`FeError::code`].
const WITHHELD: &str =
    "(diagnostic withheld: could not confirm the FE's message was fully redacted)";

/// The shortest unbroken (no-whitespace) run of characters this pipeline
/// treats as an opaque token worth redacting even with no quoting at all — a
/// credential, a base64 blob, a `user:pass` pair. **20, justified in one
/// line**: short enough that no FE message this module has observed (error
/// codes, sqlstates, short identifiers, the ordinary words around them)
/// reaches it, long enough that the accepted cost — an unquoted long
/// class/package name or path the FE might someday echo gets redacted too —
/// is a smaller loss than letting a credential-length token of the same
/// shape through unredacted.
const MIN_OPAQUE_RUN: usize = 20;

/// `mysql_async`'s own `Error` Display wraps its cause as `` `…' `` — an
/// ASCII "smart quote" convention (`thiserror`'s `#[error("… \`{}'", _0)]`
/// on `Error::Server`/`Driver`/`Url`; verified against
/// `third_party/mysql_async/src/error/mod.rs`), backtick to open, a bare
/// apostrophe to close. Left alone this is a quote character with no
/// matching partner of its own kind, which does two kinds of damage: it can
/// pair against an UNRELATED inner quote (shifting a later redaction's span
/// off whatever it was supposed to cover), and — because [`fe_error`]'s
/// fail-closed step withholds on ANY residual quote — it would make every
/// single real error withhold, since this wrapper is present on nearly
/// everything `MysqlDdlRunner` can return. Must run before anything else.
/// Targeted at the EXACT format above (`": \`"` then the cause then a
/// trailing `'`), not a generic "strip the ends" heuristic, so it does
/// nothing to text that doesn't have this specific shape (an `Io` or
/// `Other` variant, `FakeStarRocksDdl`'s own plain test strings, …).
fn normalize_mysql_async_wrapper(raw: &str) -> String {
    let Some(rest) = raw.strip_suffix('\'') else {
        return raw.to_string();
    };
    let Some(idx) = rest.find(": `") else {
        return raw.to_string();
    };
    let mut out = String::with_capacity(rest.len());
    out.push_str(&rest[..idx]);
    out.push_str(": ");
    out.push_str(&rest[idx + ": `".len()..]);
    out
}

/// Replace every single-, double- or backtick-quoted fragment in `raw` with
/// [`REDACTED`], keeping the quote characters and everything outside them.
/// In an FE error the quoted fragments ARE the echoed statement text — the
/// token the parser choked on, a property name, a value (or a truncated
/// prefix of one). **This step alone is not sound** — it assumes quoting is
/// balanced and unescaped, which a stray apostrophe in prose, a SQL-escaped
/// `\'`, or a `\"` (reachable from egret's own [`escape_dq`]) all violate,
/// each one re-pairing the regex against the WRONG partner quote and leaving
/// a secret exposed between two quote characters that individually look
/// fine. That is exactly why [`fe_error`] does not trust this function's
/// output on its own: it checks afterward for any leftover quote character
/// and withholds the whole excerpt if it finds one, rather than shipping a
/// redaction this function cannot itself prove correct.
fn redact_quoted(raw: &str) -> String {
    static QUOTED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = QUOTED.get_or_init(|| {
        regex::Regex::new(r#"'[^']*'|"[^"]*"|`[^`]*`"#)
            .expect("static quoted-fragment pattern compiles")
    });
    re.replace_all(raw, REDACTED).into_owned()
}

/// Replace every `scheme://…` token in `text` with [`REDACTED`], up to the
/// next whitespace or quote character — closes the leak fix round 1's live
/// investigation measured: a `driver_url` (which can carry embedded HTTP
/// Basic credentials) echoed by the FE with NO quoting at all
/// (`… msg: Cannot get driver from url: http://127.0.0.1:1/x.jar}`).
fn redact_urls(text: &str) -> String {
    static URL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = URL.get_or_init(|| {
        regex::Regex::new(r#"[A-Za-z][A-Za-z0-9+.-]*://[^\s'"`]+"#)
            .expect("static URL pattern compiles")
    });
    re.replace_all(text, REDACTED).into_owned()
}

/// Replace every unbroken run of [`MIN_OPAQUE_RUN`] or more non-whitespace
/// characters in `text` with [`REDACTED`] — the catch-all for a secret the
/// FE echoes with no quoting and no `scheme://` prefix: a bare credential, a
/// base64 blob, a `user:pass` pair.
fn redact_opaque_runs(text: &str) -> String {
    static OPAQUE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = OPAQUE.get_or_init(|| {
        regex::Regex::new(&format!(r"\S{{{MIN_OPAQUE_RUN},}}"))
            .expect("static opaque-run pattern compiles")
    });
    re.replace_all(text, REDACTED).into_owned()
}

/// Whether any single-, double- or backtick-quote character remains in
/// `text` — the fail-closed check [`fe_error`] runs after every redaction
/// pass. A survivor means the quoting could not be confidently paired
/// (unbalanced, escaped, or truncated), so nothing this pipeline redacted
/// can be trusted either: the whole excerpt is withheld rather than shipped
/// partially redacted.
fn has_residual_quote(text: &str) -> bool {
    text.contains(['\'', '"', '`'])
}

/// The FE's answer to a failed statement, reduced to what may safely leave
/// this module: never the statement (D6 addendum), never a quoted fragment
/// of it, never a bare URL or long opaque token, and — when [`fe_error`]'s
/// fail-closed check can't confirm all of that held — no text at all. See
/// the module doc's "guarantee" paragraph for exactly what this promises.
// NOT `Debug`: see the hand-written impl below. A derived one would print the
// private `excerpt`, which is the whole hole `Display` was narrowed to close.
#[derive(Clone, PartialEq, Eq)]
pub struct FeError {
    /// The FE's own numeric error code (StarRocks' `ERROR <code>
    /// (<sqlstate>): …` convention, e.g. `1064`, `5078`), when the message
    /// contains one. `None` for a failure that never reached the FE at all
    /// (a connect or statement timeout has no FE error code), or when the
    /// excerpt was withheld and the raw text simply had none.
    pub code: Option<u32>,
    /// A bounded, redacted excerpt of the FE's message — or [`WITHHELD`]
    /// when [`fe_error`]'s fail-closed check found a quote character it
    /// could not account for.
    ///
    /// **Private on purpose** (Task 4, carried from Task 3's review): this
    /// module has no secrets map, so the excerpt closes only the secret
    /// SHAPES it can recognise — the module doc's boundary paragraph names a
    /// real, standing shape it cannot. [`FeError::scrubbed`] is the only way
    /// to read it, and it takes the caller's resolved secrets, so the
    /// remaining obligation is enforced by the type rather than remembered
    /// from a doc comment.
    excerpt: String,
    /// Whether the FE said the object was already there — decided in
    /// [`fe_error`] from the RAW message, before a single redaction pass
    /// touches it, and carried here so that no caller has to.
    ///
    /// This is the fix for fix-round-1 F2. The delegation bootstrap used to
    /// ask `sr_already_exists(&e.scrubbed(&secrets))`, which runs a question
    /// about WHAT THE FE MEANT against a string built to be safe to PRINT:
    /// capped at [`FE_ERROR_EXCERPT_MAX`], and replaced wholesale by
    /// [`WITHHELD`] when the fail-closed check fires. An FE message carrying
    /// a SQL-escaped apostrophe — a shape this module's own doc names as
    /// reachable — therefore answered `false`, the repair `ALTER SECURITY
    /// INTEGRATION` never ran, and a changed `required_issuer` stayed stale
    /// for ever behind a "statement failed; admin can retry" warning.
    ///
    /// The rule this encodes: **redaction is for what humans and columns
    /// see, never for what code branches on.** Any future predicate about
    /// the FE's meaning belongs here, next to this one, and not at a call
    /// site holding a `scrubbed` string.
    already_exists: bool,
    /// Whether the FE said the catalog does not exist
    /// (`ERR_BAD_CATALOG_ERROR` 5078, `Unknown catalog '<name>'`), decided
    /// from the RAW message for exactly the reasons [`Self::already_exists`]
    /// gives.
    ///
    /// The second predicate to move here, and the reason the rule above is
    /// written as a rule (final whole-branch review, F1):
    /// [`show_create_catalog`] used to ask `e.excerpt.contains("Unknown
    /// catalog")` of the redacted string, one field access away from the
    /// `already_exists` fix. It failed SAFE — a withheld or over-capped
    /// excerpt answers `false`, so the error propagates and the attachment
    /// records an honest `failed` instead of a wrong `Absent` — but it was
    /// a predicate on a string built to be printed, and the branch claimed
    /// in two places that no such predicate was left.
    unknown_catalog: bool,
}

impl FeError {
    /// The only form that may be logged, stored in `apply_error`, or returned
    /// to an API caller (D6): this module's shape-based redaction, PLUS every
    /// resolved secret value on the wire, which closes the short-unquoted
    /// residual the module doc describes.
    ///
    /// **Two halves, and only one of them is the caller's.** `secrets` is
    /// what the failed STATEMENT carried — `StarRocksCatalogDdl::secret_ids`
    /// resolved, or an empty map for a `DROP` or a `SHOW`. `fe` is what the
    /// CONNECTION carried: every statement in this module logs in as `root`,
    /// and a refused login echoes the credential it was given. Taking the
    /// `&FeTarget` rather than trusting the caller to have folded
    /// `FeTarget::secrets` into its own map is the whole point of the
    /// signature — see that method for the two times it was forgotten.
    ///
    /// The old, forgettable shape no longer type-checks:
    ///
    /// ```compile_fail
    /// # use std::collections::BTreeMap;
    /// # use egret_engine_mgmt::FeError;
    /// fn diagnose(e: &FeError, statement_secrets: &BTreeMap<uuid::Uuid, String>) -> String {
    ///     // The FE root password is nowhere in sight, and that is now an error.
    ///     e.scrubbed(statement_secrets)
    /// }
    /// ```
    ///
    /// and the shape that does compile cannot leave the connection's own
    /// secrets out, because it is handed the connection:
    ///
    /// ```
    /// # use std::collections::BTreeMap;
    /// # use egret_engine_mgmt::{FeError, FeTarget};
    /// fn diagnose(
    ///     e: &FeError,
    ///     fe: &FeTarget,
    ///     statement_secrets: &BTreeMap<uuid::Uuid, String>,
    /// ) -> String {
    ///     e.scrubbed(fe, statement_secrets)
    /// }
    /// ```
    ///
    /// Nor can a caller re-derive the connection half by hand and get it
    /// wrong, because `FeTarget::secrets` is private:
    ///
    /// ```compile_fail
    /// # use egret_engine_mgmt::FeTarget;
    /// fn connection_secrets(fe: &FeTarget) -> usize {
    ///     fe.secrets().len()
    /// }
    /// ```
    ///
    /// **What those two blocks do and do not prove.** `compile_fail` asserts
    /// only that the snippet does not build; rustdoc checks an appended error
    /// code (`compile_fail,E0061`) on NIGHTLY ONLY, and this workspace pins
    /// stable, so such an annotation is decorative — measured, not assumed: it
    /// was written, deliberately falsified to `E0308`, and the doctest still
    /// passed. So the reasons are pinned by mutation instead, and both were
    /// run: making `secrets` `pub` again turns the second block green (it is
    /// about privacy and nothing else), and reverting this signature to one
    /// argument turns the first green (it is about the arity and nothing
    /// else). The middle block is the control: it is the same snippet plus the
    /// `fe` argument, so a snippet failing for some unrelated reason would
    /// show up there.
    // The positive block above is deliberately NOT `no_run`: it must be
    // type-checked, since type-checking is the entire assertion.
    pub fn scrubbed(&self, fe: &FeTarget, secrets: &BTreeMap<uuid::Uuid, String>) -> String {
        // The FE's own go in LAST, so nothing a caller passes can displace
        // them — not even a map that happens to key on the nil UUID.
        let mut all = secrets.clone();
        all.extend(fe.secrets());
        let text = egret_catalog_registry::scrub_secrets(&self.excerpt, &all);
        match self.code {
            Some(c) => format!("FE error {c}: {text}"),
            None => text,
        }
    }

    /// Did the FE mean "the object is already there", as opposed to a real
    /// failure? The delegation bootstrap branches on this to repair an
    /// `egret_oidc` integration that already exists instead of leaving its
    /// properties stale. Read off [`Self::already_exists`], which was decided
    /// from the unredacted message — see that field for why this is not a
    /// call-site `contains` on [`Self::scrubbed`].
    pub fn is_already_exists(&self) -> bool {
        self.already_exists
    }

    /// Did the FE mean "there is no catalog by that name"?
    /// [`show_create_catalog`] branches on this to turn a `SemanticException`
    /// into `Ok(None)` — the FE answers an unknown catalog with an error, not
    /// an empty result. Read off [`Self::unknown_catalog`], decided from the
    /// unredacted message.
    pub fn is_unknown_catalog(&self) -> bool {
        self.unknown_catalog
    }
}

/// Does this FE message mean "the object is already there"? StarRocks answers
/// `CREATE` on an existing integration with `Getting analyzing error. Detail
/// message: security integration 'egret_oidc' already exists.` — matched
/// loosely on the phrase so a reworded prefix does not turn a repairable state
/// into a warning nobody acts on.
///
/// Lives here, beside [`fe_error`], rather than in `engines.rs` where it used
/// to (`sr_already_exists`): the only text it may ever be shown is the raw
/// message, and the only code holding that is [`fe_error`] itself.
fn says_already_exists(raw: &str) -> bool {
    raw.to_ascii_lowercase().contains("already exists")
}

/// Does this FE message mean "there is no catalog by that name"? StarRocks
/// answers `SHOW CREATE CATALOG` on an absent catalog with
/// `ERROR 5078 (HY000): Unknown catalog '<name>'` (`ERR_BAD_CATALOG_ERROR`).
/// Matched on the phrase, case-insensitively, for the same reason
/// [`says_already_exists`] is: a reworded prefix must not turn "absent" into
/// a hard failure.
///
/// Lives here, beside [`fe_error`], and not at [`show_create_catalog`]'s
/// call site, which is where it used to be — see [`FeError::unknown_catalog`].
fn says_unknown_catalog(raw: &str) -> bool {
    raw.to_ascii_lowercase().contains("unknown catalog")
}

/// The FE's error code and nothing else. Deliberately NOT the excerpt: this
/// is what `format!("{e}")` and `e.to_string()` produce, and those are exactly
/// the expressions a caller reaches for when assembling an `apply_error`. A
/// `Display` that carried the excerpt made the leak in the module doc's
/// boundary paragraph a one-line mistake. Use [`FeError::scrubbed`] for the
/// diagnosis.
impl std::fmt::Display for FeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(c) => write!(f, "FE error {c}"),
            None => write!(f, "FE error (no code)"),
        }
    }
}

/// Hand-written, and it must stay that way (fix round 1 review, F2): a DERIVED
/// `Debug` prints private fields, so it would print `excerpt` — and `Debug` is
/// not an exotic path here. `tracing::error!(error = ?e, …)` is one character
/// away from the `%e` this crate uses everywhere and is the idiom already in
/// use a few call sites over, `FeError` is publicly re-exported from `lib.rs`,
/// and `{e:?}` inside a `panic!`/`expect`/`unwrap` would do it too. Making
/// `Display` safe while leaving `Debug` leaky would have left the documented
/// short-unquoted residual reachable by the most ordinary typo in the
/// codebase, with a comment next to it claiming it was not.
///
/// So this prints the code and says the excerpt was withheld. Both formatters
/// out of this type are now safe by construction, and [`FeError::scrubbed`] —
/// which takes the caller's secrets — remains the only way to read the
/// diagnosis.
impl std::fmt::Debug for FeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeError")
            .field("code", &self.code)
            .field(
                "excerpt",
                &"<withheld: use FeError::scrubbed(fe, &secrets)>",
            )
            .field("already_exists", &self.already_exists)
            .field("unknown_catalog", &self.unknown_catalog)
            .finish()
    }
}

/// Build a [`FeError`] from a raw failure (a `mysql_async::Error`, a timeout
/// message, or — in [`FakeStarRocksDdl`] — a test-authored string).
///
/// Pipeline, in order (each step's own doc explains why this order):
/// 1. [`normalize_mysql_async_wrapper`] — strip the dependency's own
///    asymmetric `` `…' `` quoting before anything else can be confused by it.
/// 2. [`redact_quoted`] — balanced quoted spans.
/// 3. [`redact_urls`] then [`redact_opaque_runs`] — unquoted shapes that
///    still carry secrets.
/// 4. [`has_residual_quote`] — fail closed: if the four steps above could
///    not fully account for every quote character in the text, withhold the
///    whole excerpt (keep [`WITHHELD`]) rather than ship a partial redaction
///    this module cannot prove correct.
///
/// The FE's own error code is read off the ORIGINAL raw text (never inside
/// a quoted span, so redaction never touches it) regardless of whether the
/// excerpt ends up withheld. So is [`FeError::already_exists`], and for a
/// stronger reason: the excerpt may be withheld entirely, and a caller
/// branching on the withheld string would silently take the wrong branch.
fn fe_error(raw: impl std::fmt::Display) -> FeError {
    let raw = raw.to_string();
    static CODE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let code_re = CODE.get_or_init(|| {
        regex::Regex::new(r"ERROR (\d+)").expect("static FE error code pattern compiles")
    });
    let code = code_re
        .captures(&raw)
        .and_then(|c| c[1].parse::<u32>().ok());

    // Decided on the RAW text, and deliberately BEFORE the pipeline below
    // (fix round 1, F2): this is a question about what the FE meant, and the
    // redacted form is allowed to be lossy — capped, or withheld entirely.
    let already_exists = says_already_exists(&raw);
    let unknown_catalog = says_unknown_catalog(&raw);

    let redacted = normalize_mysql_async_wrapper(&raw);
    let redacted = redact_quoted(&redacted);
    let redacted = redact_urls(&redacted);
    let redacted = redact_opaque_runs(&redacted);

    let excerpt = if has_residual_quote(&redacted) {
        WITHHELD.to_string()
    } else {
        redacted.chars().take(FE_ERROR_EXCERPT_MAX).collect()
    };
    FeError {
        code,
        excerpt,
        already_exists,
        unknown_catalog,
    }
}

/// One statement/query channel to a StarRocks FE. A trait so the apply and
/// bootstrap paths (Tasks 4/5) can be driven in a test against
/// [`FakeStarRocksDdl`] instead of a real MySQL port (D8).
#[async_trait::async_trait]
pub trait StarRocksDdl: Send + Sync {
    async fn execute(&self, fe: &FeTarget, sql: &str) -> Result<(), FeError>;
    async fn query(&self, fe: &FeTarget, sql: &str) -> Result<Vec<Vec<Option<String>>>, FeError>;
}

/// Bounds on the real runner's own connection and per-statement work,
/// gathered in this one named place (fix round 1 review, I4): before this,
/// neither had a deadline at all, and `mysql_async` (this workspace's vendored
/// fork) offers no built-in connect-timeout option to lean on, so both are
/// enforced with `tokio::time::timeout` around the call. This matters
/// precisely because D1's premise is that a freshly-provisioned FE is
/// unreachable for five to ten minutes, and since Task 4 `execute` IS called
/// from an HTTP handler (`catalog_apply::converge_attachment`, reached by
/// `POST /engines/{id}/catalogs/{cid}`), not only from a background bootstrap
/// with its own poll loop. An attach against an unreachable FE now blocks a
/// request for `FE_CONNECT_TIMEOUT` and then records `failed` (D1).
const FE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FE_STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

/// The real runner: one `mysql_async` connection per statement, exactly like
/// the `run_starrocks_ddl` it replaced (Task 5 deleted that) — a one-shot
/// admin statement has no caller-facing row cap/cancellation/result shape to
/// thread through, so this stays direct `mysql_async` rather than
/// `egret_query`'s interactive `StarRocksConnector`.
pub struct MysqlDdlRunner;

impl MysqlDdlRunner {
    async fn connect(fe: &FeTarget) -> Result<mysql_async::Conn, FeError> {
        Self::connect_within(fe, FE_CONNECT_TIMEOUT).await
    }

    async fn connect_within(fe: &FeTarget, limit: Duration) -> Result<mysql_async::Conn, FeError> {
        let opts = crate::engines::starrocks_root_conn_opts(
            &fe.host,
            fe.port,
            fe.root_password.as_deref(),
            fe.tls_cert_pem.as_deref(),
        );
        match tokio::time::timeout(limit, mysql_async::Conn::new(opts)).await {
            Ok(Ok(conn)) => Ok(conn),
            Ok(Err(e)) => Err(fe_error(e)),
            Err(_) => Err(fe_error(format!(
                // No `host:port` (Task 4 review M-7): this text reaches the
                // classification page through `list_materialized_views`, and the
                // FE address is internal service DNS. Every caller already names
                // the engine.
                "connect to the FE timed out after {limit:?}"
            ))),
        }
    }
}

#[async_trait::async_trait]
impl StarRocksDdl for MysqlDdlRunner {
    async fn execute(&self, fe: &FeTarget, sql: &str) -> Result<(), FeError> {
        use mysql_async::prelude::Queryable;
        let mut conn = Self::connect(fe).await?;
        match tokio::time::timeout(FE_STATEMENT_TIMEOUT, conn.query_drop(sql)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(fe_error(e)),
            Err(_) => {
                return Err(fe_error(format!(
                    "statement timed out after {FE_STATEMENT_TIMEOUT:?}"
                )))
            }
        }
        let _ = conn.disconnect().await;
        Ok(())
    }

    async fn query(&self, fe: &FeTarget, sql: &str) -> Result<Vec<Vec<Option<String>>>, FeError> {
        use mysql_async::prelude::Queryable;
        let mut conn = Self::connect(fe).await?;
        let rows: Vec<mysql_async::Row> =
            match tokio::time::timeout(FE_STATEMENT_TIMEOUT, conn.query(sql)).await {
                Ok(Ok(rows)) => rows,
                Ok(Err(e)) => return Err(fe_error(e)),
                Err(_) => {
                    return Err(fe_error(format!(
                        "statement timed out after {FE_STATEMENT_TIMEOUT:?}"
                    )))
                }
            };
        let out = rows
            .into_iter()
            .map(|row| {
                (0..row.columns_ref().len())
                    .map(|i| {
                        row.as_ref(i)
                            .cloned()
                            .and_then(|v| mysql_async::from_value_opt::<Option<String>>(v).ok())
                            .flatten()
                    })
                    .collect()
            })
            .collect();
        let _ = conn.disconnect().await;
        Ok(out)
    }
}

/// Where and how to reach this engine's FE. The cert is the engine's own
/// (`engines.coordinator_cert`, minted by `ensure_tls_cert`), used whenever
/// [`crate::engines::sr_tls_enabled`] says the FE actually serves TLS — the
/// same predicate `build_starrocks_provision_req` provisions by, which is the
/// point of it being one function (Task 5 found the two apart: this one read
/// `tlsEnabled` with `unwrap_or(false)`, so a delegation-enabled engine with
/// no explicit `tlsEnabled` was provisioned with TLS and dialled without the
/// cert).
///
/// Before SP3b the catalog DDL and the root-login probe both passed `None`
/// while the delegation DDL passed the cert, so an OAuth2 credential AND the FE
/// root password crossed the wire in cleartext on a TLS-capable engine (D6).
/// This function is the fix, and since Task 5 it is applied EVERYWHERE: the
/// attach/re-apply/detach path (`catalog_apply::apply_starrocks`) and the
/// bootstrap (`engines::spawn_starrocks_catalog_wire` — its root-login probe,
/// its per-attachment convergence and its delegation statements alike) all take
/// their target from here. There is no second way to reach a StarRocks FE left.
// `pub(crate)`, not the public API: only this crate's apply/bootstrap code
// calls it. (The `#[allow(dead_code)]` that used to sit here for the window
// where it had no production caller is gone with that window — fix round 1
// review, F6.)
pub(crate) async fn fe_target(st: &CpState, engine: &Engine) -> Result<FeTarget, CpError> {
    let root_password = match engine.root_secret_id {
        Some(sid) => Some(
            st.secrets
                .resolve_plaintext(None, sid)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve root password secret: {e}")))?,
        ),
        None => None,
    };
    let tls = crate::engines::sr_tls_enabled(&engine.spec);
    Ok(FeTarget {
        host: engine.coordinator_host(),
        port: engine.coordinator_port() as u16,
        root_password,
        tls_cert_pem: tls.then(|| engine.coordinator_cert.clone()).flatten(),
    })
}

/// `SHOW CATALOGS` → column 0 (`Catalog`; the three columns are
/// `Catalog, Type, Comment`, `ShowResultMetaFactory.java:1305-1311`).
pub async fn show_catalogs(
    ddl: &dyn StarRocksDdl,
    fe: &FeTarget,
) -> Result<BTreeSet<String>, FeError> {
    Ok(ddl
        .query(fe, "SHOW CATALOGS")
        .await?
        .into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
        .collect())
}

/// `SHOW CREATE CATALOG <name>` → column 1 (`Create Catalog`), parsed into its
/// properties. `None` when the FE says the catalog is unknown
/// (`ERR_BAD_CATALOG_ERROR` 5078, text `Unknown catalog '<name>'`) — that is a
/// SemanticException, not an empty result, so it arrives as an Err and is
/// translated here rather than at every call site.
///
/// The "is this an unknown catalog" verdict is [`FeError::is_unknown_catalog`],
/// taken from the raw FE text inside [`fe_error`] — never a `contains` on the
/// redacted excerpt, which this used to be (final review, F1).
pub async fn show_create_catalog(
    ddl: &dyn StarRocksDdl,
    fe: &FeTarget,
    name: &str,
) -> Result<Option<BTreeMap<String, String>>, FeError> {
    let sql = format!("SHOW CREATE CATALOG `{}`", escape_ident(name));
    match ddl.query(fe, &sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().nth(1).flatten())
            .map(|text| parse_show_create_properties(&text))),
        Err(e) if e.is_unknown_catalog() => Ok(None),
        Err(e) => Err(e),
    }
}

/// View governance A4 (revised 2026-10-14): every materialized view on the
/// engine, one statement per engine.
///
/// StarRocks creates a materialized view only in `default_catalog`, whatever
/// catalog its base tables are in, and `SHOW MATERIALIZED VIEWS FROM
/// <external catalog>.<db>` answers an empty set even when one reads that
/// database (both measured on 4.1.6,
/// `docs/en/plans/2026-10-14-starrocks-mv-placement-measurement.md`). So the
/// listing reads `information_schema.materialized_views`, which covers every
/// database of `default_catalog`, and the caller matches the base tables.
///
/// The columns are NAMED, so their positions in the reply are the ones this
/// statement chooses rather than the FE's column order (Task 4 review M-2). The
/// statement carries no identifier, so there is nothing to quote.
pub const MATERIALIZED_VIEWS_SQL: &str = "SELECT TABLE_SCHEMA, TABLE_NAME, \
     BASE_TABLE_REFRESH_VERSION_TIMES, MATERIALIZED_VIEW_DEFINITION FROM \
     information_schema.materialized_views";

/// One row of [`MATERIALIZED_VIEWS_SQL`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedViewRow {
    /// The `default_catalog` database the view lives in.
    pub database: String,
    pub name: String,
    /// The keys of `BASE_TABLE_REFRESH_VERSION_TIMES`, normalised
    /// `catalog.db.table` names, sorted. `None` when the column is missing,
    /// not a JSON object, or an empty one (for example a view never
    /// refreshed): the caller then falls back to matching the definition text.
    pub base_tables: Option<Vec<String>>,
    /// `MATERIALIZED_VIEW_DEFINITION`, as the user wrote it (not normalised).
    pub definition: String,
}

/// The base tables out of `BASE_TABLE_REFRESH_VERSION_TIMES`, read defensively:
/// only a non-empty JSON object yields any, and its keys are the names. Anything
/// else is `None`, never an error, because the text match still applies.
pub fn parse_base_tables(raw: Option<&str>) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(raw?).ok()?;
    let map = value.as_object()?;
    if map.is_empty() {
        return None;
    }
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    Some(keys)
}

/// [`MATERIALIZED_VIEWS_SQL`] on one FE. A row whose shape is not the four
/// selected columns, or that has no database or name, is an error rather than a
/// guess: a listing that silently drops rows would read as "none".
pub async fn select_materialized_views(
    ddl: &dyn StarRocksDdl,
    fe: &FeTarget,
) -> Result<Vec<MaterializedViewRow>, FeError> {
    ddl.query(fe, MATERIALIZED_VIEWS_SQL)
        .await?
        .into_iter()
        .map(|row| {
            let [Some(database), Some(name), base, definition]: [Option<String>; 4] =
                row.try_into().map_err(|r: Vec<Option<String>>| {
                    fe_error(format!(
                        "information_schema.materialized_views answered {} columns, not 4",
                        r.len()
                    ))
                })?
            else {
                return Err(fe_error(
                    "information_schema.materialized_views answered a view with no database or name",
                ));
            };
            Ok(MaterializedViewRow {
                database,
                name,
                base_tables: parse_base_tables(base.as_deref()),
                definition: definition.unwrap_or_default(),
            })
        })
        .collect()
}

/// [`select_materialized_views`] on a stored engine, for the classification
/// page's exposure report. Every failure — an unknown or non-StarRocks engine,
/// an unresolvable FE target, an unreachable FE — is a message, because the
/// caller must tell "could not list" apart from "none exist" and never render
/// the former as the latter. The FE's own text is scrubbed of the root password
/// first.
pub async fn list_materialized_views(
    st: &CpState,
    engine_id: uuid::Uuid,
) -> Result<Vec<MaterializedViewRow>, String> {
    let fe = admin_fe(st, engine_id).await?;
    select_materialized_views(&*st.starrocks_ddl, &fe)
        .await
        .map_err(|e| e.scrubbed(&fe, &BTreeMap::new()))
}

/// The `root` connection to a stored, provisioned StarRocks engine — the one
/// [`list_materialized_views`] and [`list_catalog_level`] read through. Every
/// failure (an unknown, non-StarRocks or external engine, an unresolvable FE
/// target) is a message, before any connection.
async fn admin_fe(st: &CpState, engine_id: uuid::Uuid) -> Result<FeTarget, String> {
    let engine = st
        .engine_mgmt
        .get_engine(engine_id)
        .await
        .map_err(|e| format!("read engine: {e}"))?
        .ok_or_else(|| "engine no longer exists".to_string())?;
    if !engine.is_starrocks() {
        return Err("engine is not a StarRocks engine".to_string());
    }
    // Task 4b review C-1: an external engine is stored `running` (a placeholder),
    // but egret never connects to a cluster it does not run and holds no
    // credential for one; `fe_target` would dial it as `root` with no password.
    // The route already skips external engines; this refuses before any
    // connection in case a caller does not.
    if crate::external::is_external(&engine) {
        return Err(
            "engine is external: egret does not connect to engines it does not run".to_string(),
        );
    }
    fe_target(st, &engine).await.map_err(|e| e.to_string())
}

/// DG frontend redesign #1 (Governance Data): ``SHOW DATABASES FROM `c` ``.
///
/// The catalog, schema and table names given to these builders are untrusted
/// caller input. Each is a backtick-delimited identifier
/// ([`egret_query_api::quote_ident`], the same doubling as [`escape_ident`]);
/// no string literal is built, so the StarRocks backslash escape that #504
/// guards against in a literal does not apply — inside backticks a backslash is
/// an ordinary character. Statements measured on 4.1.6 as `root`
/// (`docs/en/plans/2026-10-16-dg-frontend-data-live.md` §1.4).
pub fn catalog_schemas_sql(catalog: &str) -> String {
    format!("SHOW DATABASES FROM {}", sr_ident(catalog))
}

/// ``SHOW FULL TABLES FROM `c`.`s` ``: columns `Tables_in_<s>, Table_type`.
pub fn catalog_relations_sql(catalog: &str, schema: &str) -> String {
    format!(
        "SHOW FULL TABLES FROM {}.{}",
        sr_ident(catalog),
        sr_ident(schema)
    )
}

/// ``SHOW FULL COLUMNS FROM `c`.`s`.`t` ``: `Field, Type, …`, the types
/// lowercase (`DESCRIBE` spells them uppercase).
pub fn catalog_columns_sql(catalog: &str, schema: &str, table: &str) -> String {
    format!(
        "SHOW FULL COLUMNS FROM {}.{}.{}",
        sr_ident(catalog),
        sr_ident(schema),
        sr_ident(table)
    )
}

fn sr_ident(name: &str) -> String {
    egret_query_api::quote_ident(egret_query_api::Engine::StarRocks, name)
}

/// The text in cell `i` of `row`.
fn listed_cell(row: &mut [Option<String>], i: usize, what: &str) -> Result<String, String> {
    match row.get_mut(i) {
        Some(cell) => cell
            .take()
            .ok_or_else(|| format!("the FE answered a NULL {what}")),
        None => Err(format!("the FE answered a row with no {what}")),
    }
}

/// The schemas out of `SHOW DATABASES`, sorted, hidden ones dropped.
pub fn parse_catalog_schemas(rows: Vec<Vec<Option<String>>>) -> Result<Vec<String>, String> {
    let mut schemas = Vec::new();
    for mut row in rows {
        let name = listed_cell(&mut row, 0, "database name")?;
        if !is_hidden_schema(egret_query_api::Engine::StarRocks, &name) {
            schemas.push(name);
        }
    }
    schemas.sort();
    Ok(schemas)
}

/// The relations out of `SHOW FULL TABLES`, sorted by name.
pub fn parse_catalog_relations(
    rows: Vec<Vec<Option<String>>>,
) -> Result<Vec<ListedRelation>, String> {
    let mut relations = rows
        .into_iter()
        .map(|mut row| {
            Ok(ListedRelation {
                name: listed_cell(&mut row, 0, "table name")?,
                kind: relation_kind(&listed_cell(&mut row, 1, "table type")?)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    relations.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(relations)
}

/// The columns out of `SHOW FULL COLUMNS`, in order. No column is an error:
/// a table always has one, so an empty answer is not a listing.
pub fn parse_catalog_columns(rows: Vec<Vec<Option<String>>>) -> Result<Vec<ListedColumn>, String> {
    let columns = rows
        .into_iter()
        .map(|mut row| {
            Ok(ListedColumn {
                name: listed_cell(&mut row, 0, "column name")?,
                r#type: listed_cell(&mut row, 1, "column type")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if columns.is_empty() {
        return Err("the FE listed no column".to_string());
    }
    Ok(columns)
}

/// One level of `catalog` on one FE: its schemas when `schema` is `None`, the
/// schema's relations when `table` is `None`, the table's columns otherwise.
/// A missing schema or table is the FE's error (5501 / 1064, measured), never
/// an empty listing. The FE's text is scrubbed of the root password.
///
/// Only `root` may read this: a StarRocks account without privilege on the
/// catalog gets an EMPTY set from `SHOW DATABASES` / `SHOW TABLES`, not an error
/// (measured S8), so an empty listing is trustworthy only as `root`.
pub async fn select_catalog_level(
    ddl: &dyn StarRocksDdl,
    fe: &FeTarget,
    catalog: &str,
    schema: Option<&str>,
    table: Option<&str>,
) -> Result<CatalogLevel, String> {
    let sql = match (schema, table) {
        (None, None) => catalog_schemas_sql(catalog),
        (None, Some(_)) => return Err("a table needs a schema".to_string()),
        (Some(schema), None) => catalog_relations_sql(catalog, schema),
        (Some(schema), Some(table)) => catalog_columns_sql(catalog, schema, table),
    };
    let rows = ddl
        .query(fe, &sql)
        .await
        .map_err(|e| e.scrubbed(fe, &BTreeMap::new()))?;
    match (schema, table) {
        (None, _) => parse_catalog_schemas(rows).map(CatalogLevel::Schemas),
        (Some(_), None) => parse_catalog_relations(rows).map(CatalogLevel::Relations),
        (Some(_), Some(_)) => parse_catalog_columns(rows).map(CatalogLevel::Columns),
    }
}

/// [`select_catalog_level`] on a stored engine, through the same `root`
/// connection as [`list_materialized_views`] — the StarRocks half of the
/// Governance browse route's `CatalogLister`. `catalog`, `schema` and `table`
/// are untrusted.
pub async fn list_catalog_level(
    st: &CpState,
    engine_id: uuid::Uuid,
    catalog: &str,
    schema: Option<&str>,
    table: Option<&str>,
) -> Result<CatalogLevel, String> {
    if schema.is_none() && table.is_some() {
        return Err("a table needs a schema".to_string());
    }
    let fe = admin_fe(st, engine_id).await?;
    select_catalog_level(&*st.starrocks_ddl, &fe, catalog, schema, table).await
}

/// The `"k" = "v"` pairs inside a parenthesized property body — the core
/// [`parse_show_create_properties`] shares with [`FakeStarRocksDdl`]'s own
/// re-parse of the statements it just ran (see that module's doc). `\"`
/// escapes inside either side are honoured.
fn parse_property_pairs(body: &str) -> BTreeMap<String, String> {
    static PAIR: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = PAIR.get_or_init(|| {
        // "key" = "value", with \" escapes inside either side.
        regex::Regex::new(r#""((?:[^"\\]|\\.)*)"\s*=\s*"((?:[^"\\]|\\.)*)""#)
            .expect("static property pattern compiles")
    });
    re.captures_iter(body)
        .map(|c| (unescape_dq(&c[1]), unescape_dq(&c[2])))
        .collect()
}

/// The properties out of a `SHOW CREATE CATALOG` reply. The FE prints a
/// `HashMap`, so the ORDER is unstable and the text must never be diffed —
/// only the pairs. Requires the literal `PROPERTIES (` — that is what a real
/// FE always prints, for a `CREATE` **or** an `ALTER`-then-`SHOW CREATE`
/// round trip (measured live: `SHOW CREATE CATALOG` re-prints the object as
/// a `CREATE … PROPERTIES (…)` statement even after an `ALTER`). A statement
/// egret itself is *about to run* is a different shape for `ALTER` (`SET
/// (…)`, since `SET PROPERTIES` is not in the grammar) — [`FakeStarRocksDdl`]
/// needs to parse that shape too, which is what [`parse_property_pairs`]
/// exists to share without baking the `PROPERTIES (` requirement into it.
pub fn parse_show_create_properties(text: &str) -> BTreeMap<String, String> {
    match (text.find("PROPERTIES ("), text.rfind(')')) {
        (Some(start), Some(end)) if end > start => parse_property_pairs(&text[start..=end]),
        _ => BTreeMap::new(),
    }
}

/// The inverse of [`escape_dq`], for values read back off the FE.
fn unescape_dq(s: &str) -> String {
    s.replace("\\\"", "\"").replace("\\\\", "\\")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Drift {
    Absent,
    /// Every comparable key that could actually be verified agreed, and
    /// none was masked. See [`Drift::Unverifiable`] for the case where that
    /// second part doesn't hold.
    Matching,
    /// The keys egret would set that the FE disagrees about, sorted. A
    /// conclusive verdict always wins over [`Drift::Unverifiable`]: if even
    /// one key is genuinely different, that is reason enough to re-apply
    /// regardless of what else can or can't be confirmed.
    Different(Vec<String>),
    /// Every key that COULD be verified agreed, but at least one of egret's
    /// own comparable keys is on [`FE_MASKED_KEYS`] — a key the FE returns
    /// masked (`xx******yy`) rather than deleting outright, so its presence
    /// proves nothing either way (fix round 1 review, I2: a changed
    /// `aws.s3.access_key` used to read as `Matching` because the masked
    /// heuristic `!actual.contains("******")` treated "can't tell" as "no
    /// diff"). Sorted list of the unverifiable keys. Per D3's "present and
    /// matching → treat as applied": a caller must NOT fold this into
    /// `Matching` — these keys need a re-apply (`ALTER`, which is safe to
    /// run unconditionally) to be sure, exactly like a secret-valued
    /// (`PropValue::Secret`) key already does by being excluded from
    /// `comparable()` altogether.
    Unverifiable(Vec<String>),
}

/// Keys that can never be compared at all, because the FE returns nothing
/// usable for them (`CredentialUtil.java:38-91`): `password` and
/// `azure_path_key` are DELETED outright, and `checksum` is the FE's own MD5
/// of `driver_url`, not something egret ever sets. There is no masked
/// placeholder to even reason about — these carry zero information — which is
/// what separates them from [`FE_MASKED_KEYS`].
///
/// This is half of the D3 addendum: drift comparison is blind to secrets.
/// Egret cannot tell a rotated credential from an unchanged one by reading
/// the FE back, and must not pretend to — which is exactly why re-applying an
/// edited catalog always issues `ALTER` rather than trusting `Drift::Matching`
/// to mean "nothing to send" for a secret-valued key.
pub const FE_UNCOMPARABLE_KEYS: [&str; 3] = ["password", "checksum", "azure_path_key"];

/// Keys the FE returns MASKED (`xx******yy`, or `******` when ≤ 4 chars)
/// rather than deleted (`CredentialUtil.java:86-103`) — the other half of the
/// D3 addendum. Two of these (`aws.s3.access_key`, `aws.glue.access_key`) are
/// rendered as `PropValue::Plain`, not `PropValue::Secret`
/// (`render/starrocks.rs:332`, `:348`), so they DO reach `comparable()` — a
/// masked value here is neither confirmed matching nor confirmed different;
/// see [`Drift::Unverifiable`]. Single-sourced with [`FakeStarRocksDdl`]'s
/// own masking imitation in `show_create_text` (fix round 1 review, I2/I2's
/// "derive one from the other" suggestion) so the two cannot drift apart the
/// way [`FE_UNCOMPARABLE_KEYS`] and this list once did.
pub const FE_MASKED_KEYS: [&str; 6] = [
    "aws.s3.access_key",
    "aws.s3.secret_key",
    "aws.glue.access_key",
    "aws.glue.secret_key",
    "iceberg.catalog.oauth2.credential",
    "iceberg.catalog.jdbc.password",
];

/// Compare what egret would render against what `SHOW CREATE CATALOG` last
/// returned. Only [`StarRocksCatalogDdl::comparable`]'s plain keys are ever
/// looked at — a secret-valued property is never in that map to begin with.
/// [`FE_UNCOMPARABLE_KEYS`] excludes keys with zero information;
/// [`FE_MASKED_KEYS`] keys are checked for PRESENCE (its absence is
/// conclusive — the FE genuinely doesn't have that property set) but never
/// for VALUE equality, since a masked value could be hiding either an
/// unchanged or a changed credential. `Drift::Absent` is NOT produced here: a
/// name the FE's `SHOW CATALOGS` does not list is absent whatever `SHOW
/// CREATE CATALOG` would say, so that verdict belongs to the caller that has
/// both reads, not to this comparison alone.
pub fn compare_to_fe(rendered: &StarRocksCatalogDdl, fe: &BTreeMap<String, String>) -> Drift {
    let mut differing: Vec<String> = Vec::new();
    let mut unverifiable: Vec<String> = Vec::new();
    for (k, v) in rendered.comparable() {
        if FE_UNCOMPARABLE_KEYS.contains(&k.as_str()) {
            continue;
        }
        if FE_MASKED_KEYS.contains(&k.as_str()) {
            if fe.contains_key(&k) {
                unverifiable.push(k);
            } else {
                // Genuinely absent, not masked — that is conclusive evidence
                // the FE does not have this property, whatever it would show.
                differing.push(k);
            }
            continue;
        }
        match fe.get(&k) {
            None => differing.push(k),
            Some(actual) if actual != &v => differing.push(k),
            Some(_) => {}
        }
    }
    if !differing.is_empty() {
        differing.sort();
        return Drift::Different(differing);
    }
    if !unverifiable.is_empty() {
        unverifiable.sort();
        return Drift::Unverifiable(unverifiable);
    }
    Drift::Matching
}

/// The rows the fake FE answers [`MATERIALIZED_VIEWS_SQL`] with, as the real
/// runner returns them: one `Option<String>` per selected column.
/// Rows as [`StarRocksDdl::query`] answers them.
type MaterializedViewRows = Vec<Vec<Option<String>>>;

/// A StarRocks FE that lives in memory. Public because the route tests are an
/// external crate; it is the whole of design D8 — without it, every statement
/// egret builds could only be exercised against a real MySQL port, which is
/// why five of the bootstrap's six guard checkpoints used to be pinned by a
/// test that grepped the function's own source.
///
/// It imitates the FE behaviours the apply path actually depends on, and no
/// others: a second `CREATE` errors with `already exists`, `ALTER` MERGES and
/// cannot unset, `DROP … IF EXISTS` on an unknown catalog succeeds, and
/// `SHOW CREATE CATALOG` masks credentials and DELETES `password`, exactly as
/// `CredentialUtil` does. Imitating the masking is the point: a fake that
/// echoed back what was written would make the drift comparison look correct
/// while it silently reported permanent drift against a real FE.
#[derive(Clone, Default)]
pub struct FakeStarRocksDdl {
    executed: Arc<Mutex<Vec<String>>>,
    /// Every `query` this FE was asked, in order — the read half of
    /// `executed`. Without it the root-login probe (a `SELECT 1`, a query)
    /// is INVISIBLE to a test, which is why the checkpoint standing between
    /// the FE-ready poll and that probe could be deleted with the whole suite
    /// staying green (fix round 1, F1).
    queried: Arc<Mutex<Vec<String>>>,
    catalogs: Arc<Mutex<BTreeMap<String, BTreeMap<String, String>>>>,
    /// The rows of `information_schema.materialized_views`, for
    /// [`MATERIALIZED_VIEWS_SQL`].
    materialized_views: Arc<Mutex<MaterializedViewRows>>,
    /// Canned answers to `query`, by exact statement text.
    answers: Arc<Mutex<BTreeMap<String, MaterializedViewRows>>>,
    /// When set, every `execute`/`query` fails with this message.
    fail: Arc<Mutex<Option<String>>>,
    /// `(needle, message)`: fail only the statements containing `needle` —
    /// honoured by both `execute` AND `query` (fix round 1 review, F-7; it
    /// used to be `execute`-only, so "this one `SHOW CREATE CATALOG` fails"
    /// could only be expressed with the global `fail`, which also broke
    /// `SHOW CATALOGS`).
    selective_fail: Arc<Mutex<Vec<(String, String)>>>,
    /// The `FeTarget` of the most recent call, so a test can assert the cert.
    last_target: Arc<Mutex<Option<FeTarget>>>,
    /// Runs on EVERY `execute`, with the statement text, after it is
    /// recorded and BEFORE the fail/succeed decision — so a hook can fire for
    /// a statement the FE is about to reject, which is what the delegation
    /// checkpoints need (fix round 1, F1). [`FakeStarRocksDdl::on_create`] is
    /// a filter over this one hook, not a second one with its own timing.
    #[allow(clippy::type_complexity)]
    on_execute: Arc<
        Mutex<
            Option<
                Arc<dyn Fn(String) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync>,
            >,
        >,
    >,
}

impl FakeStarRocksDdl {
    pub fn executed(&self) -> Vec<String> {
        self.executed.lock().unwrap().clone()
    }
    /// Every `query` the FE was asked (`SELECT 1`, `SHOW CATALOGS`,
    /// `SHOW CREATE CATALOG …`), in order. `executed()` records statements
    /// only, so this is the only way to see that the root-login probe was —
    /// or, for a checkpoint test, was NOT — attempted.
    pub fn queried(&self) -> Vec<String> {
        self.queried.lock().unwrap().clone()
    }
    pub fn clear_executed(&self) {
        self.executed.lock().unwrap().clear();
        self.queried.lock().unwrap().clear();
    }
    /// Catalog names in the order `CREATE EXTERNAL CATALOG` ran for them.
    pub fn created(&self) -> Vec<String> {
        self.executed()
            .iter()
            .filter_map(|s| statement_target("CREATE EXTERNAL CATALOG", s))
            .collect()
    }
    /// Every call fails with this message until it is cleared.
    pub fn set_fail(&self, msg: Option<&str>) {
        *self.fail.lock().unwrap() = msg.map(str::to_string);
    }
    /// Fail only the statements naming this catalog — one bad catalog among
    /// several, which is what the bootstrap loop has to survive.
    pub fn fail_catalog(&self, name: &str, msg: &str) {
        self.fail_matching(&format!("`{}`", escape_ident(name)), msg)
    }
    /// Fail only the statements/queries containing this substring (the
    /// `already exists` branch of the delegation bootstrap, or a single
    /// `SHOW CREATE CATALOG` among several `SHOW CATALOGS` calls).
    pub fn fail_matching(&self, needle: &str, msg: &str) {
        self.selective_fail
            .lock()
            .unwrap()
            .push((needle.to_string(), msg.to_string()));
    }
    /// The `FeTarget` of the most recent call — how a test proves the pinned
    /// certificate really reaches every statement (D6).
    pub fn last_target(&self) -> FeTarget {
        self.last_target
            .lock()
            .unwrap()
            .clone()
            .expect("a call was made")
    }
    /// Run this when a `CREATE EXTERNAL CATALOG` is executed, before it is
    /// applied — the only way to make something happen BETWEEN two iterations
    /// of the bootstrap's loop (deleting the engine, say). The hook receives
    /// the catalog's name.
    pub fn on_create(
        &self,
        hook: impl Fn(String) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync + 'static,
    ) {
        self.on_execute(
            move |sql| match statement_target("CREATE EXTERNAL CATALOG", &sql) {
                Some(name) => hook(name),
                None => Box::pin(std::future::ready(())),
            },
        )
    }

    /// Run this on EVERY statement, receiving its text, before the fake
    /// decides whether to accept or reject it. The general form of
    /// [`Self::on_create`], and what a test uses to make the engine vanish
    /// between two statements of the delegation bootstrap — including
    /// between a statement that FAILS and the repair that follows it, which
    /// `on_create`'s catalog filter cannot express.
    pub fn on_execute(
        &self,
        hook: impl Fn(String) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync + 'static,
    ) {
        *self.on_execute.lock().unwrap() = Some(Arc::new(hook));
    }
    pub fn catalog(&self, name: &str) -> Option<BTreeMap<String, String>> {
        self.catalogs.lock().unwrap().get(name).cloned()
    }
    /// Seed a catalog egret did not create — a hand-made `egret`, or one left
    /// by a pre-SP3a engine. This is how the drift test builds its premise.
    pub fn seed(&self, name: &str, props: &[(&str, &str)]) {
        self.catalogs.lock().unwrap().insert(
            name.to_string(),
            props
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
    }

    /// Seed one row of `information_schema.materialized_views`, in the column
    /// order [`MATERIALIZED_VIEWS_SQL`] selects. An FE never seeded has none.
    pub fn seed_materialized_view(
        &self,
        database: &str,
        name: &str,
        base_table_refresh_version_times: Option<&str>,
        definition: &str,
    ) {
        self.seed_raw_materialized_view_row(vec![
            Some(database.to_string()),
            Some(name.to_string()),
            base_table_refresh_version_times.map(str::to_string),
            Some(definition.to_string()),
        ]);
    }

    /// Seed a row exactly as given, for the shape-check tests.
    /// Answer `sql` (exact text) with `rows` from now on.
    pub fn answer_query(&self, sql: &str, rows: Vec<Vec<Option<String>>>) {
        self.answers.lock().unwrap().insert(sql.to_string(), rows);
    }

    pub fn seed_raw_materialized_view_row(&self, row: Vec<Option<String>>) {
        self.materialized_views.lock().unwrap().push(row);
    }

    /// Remove a catalog WITHOUT recording a statement — an FE that lost it to
    /// someone else's `DROP`, which is what detach must tolerate (D7).
    pub fn forget(&self, name: &str) {
        self.catalogs.lock().unwrap().remove(name);
    }

    /// The needle-matched failure for this statement/query text, if any.
    fn selective_failure(&self, sql: &str) -> Option<String> {
        self.selective_fail
            .lock()
            .unwrap()
            .iter()
            .find(|(needle, _)| sql.contains(needle.as_str()))
            .map(|(_, msg)| msg.clone())
    }
}

#[async_trait::async_trait]
impl StarRocksDdl for FakeStarRocksDdl {
    async fn execute(&self, fe: &FeTarget, sql: &str) -> Result<(), FeError> {
        *self.last_target.lock().unwrap() = Some(fe.clone());
        // Record the attempt BEFORE deciding fail/succeed (fix round 1
        // review, F-7): a statement egret sent and that was rejected must
        // still show up in `executed()`, so a test can assert both "we sent
        // it" and "it failed" instead of only one.
        self.executed.lock().unwrap().push(sql.to_string());
        // BEFORE the fail/succeed decision (fix round 1, F1): a hook must be
        // able to fire for a statement the FE is about to reject — that is
        // exactly the window the delegation bootstrap's repair checkpoint
        // sits in.
        let hook = self.on_execute.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(sql.to_string()).await;
        }
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            return Err(fe_error(msg));
        }
        if let Some(msg) = self.selective_failure(sql) {
            return Err(fe_error(msg));
        }
        let mut cats = self.catalogs.lock().unwrap();
        if let Some(name) = statement_target("CREATE EXTERNAL CATALOG", sql) {
            if cats.contains_key(&name) {
                return Err(fe_error(format!("Catalog '{name}' already exists")));
            }
            cats.insert(name, parse_statement_properties(sql));
        } else if let Some(name) = statement_target("ALTER CATALOG", sql) {
            // Merge, never replace — CatalogMgr.java:212-219. An unknown
            // catalog is a SILENT no-op (CatalogMgr.java:184-187).
            //
            // `parse_statement_properties`, not `parse_show_create_properties`
            // (fix round 1 review, I1): an `ALTER … SET (…)` statement has no
            // `PROPERTIES (` literal, so the production parser — correctly
            // scoped to what a real FE's `SHOW CREATE CATALOG` reply always
            // says — would silently return an empty map here, making this
            // `extend` a no-op and the doc above's "ALTER MERGES" claim false.
            // Measured before this fix: CREATE stored `user=u1`, ALTER to
            // `u2` ran and was recorded in `executed()`, but the stored value
            // stayed `u1`.
            if let Some(existing) = cats.get_mut(&name) {
                existing.extend(parse_statement_properties(sql));
            }
        } else if let Some(name) = statement_target("DROP CATALOG IF EXISTS", sql) {
            cats.remove(&name);
        }
        Ok(())
    }

    async fn query(&self, fe: &FeTarget, sql: &str) -> Result<Vec<Vec<Option<String>>>, FeError> {
        *self.last_target.lock().unwrap() = Some(fe.clone());
        self.queried.lock().unwrap().push(sql.to_string());
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            return Err(fe_error(msg));
        }
        if let Some(msg) = self.selective_failure(sql) {
            return Err(fe_error(msg));
        }
        if sql == "SELECT 1" {
            return Ok(vec![vec![Some("1".into())]]);
        }
        if sql == "SHOW CATALOGS" {
            return Ok(self
                .catalogs
                .lock()
                .unwrap()
                .keys()
                .map(|n| vec![Some(n.clone()), Some("External".into()), None])
                .collect());
        }
        if sql == MATERIALIZED_VIEWS_SQL {
            return Ok(self.materialized_views.lock().unwrap().clone());
        }
        if let Some(rows) = self.answers.lock().unwrap().get(sql) {
            return Ok(rows.clone());
        }
        if let Some(name) = statement_target("SHOW CREATE CATALOG", sql) {
            let Some(props) = self.catalogs.lock().unwrap().get(&name).cloned() else {
                return Err(fe_error(format!("Unknown catalog '{name}'")));
            };
            return Ok(vec![vec![
                Some(name.clone()),
                Some(show_create_text(&name, &props)),
            ]]);
        }
        Ok(vec![])
    }
}

/// The backtick-quoted name out of a statement that starts with `prefix`,
/// with doubled backticks undone — the inverse of what [`escape_ident`] wrote.
fn statement_target(prefix: &str, sql: &str) -> Option<String> {
    let rest = sql.strip_prefix(prefix)?.trim_start();
    let rest = rest.strip_prefix('`')?;
    let mut name = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '`' if chars.clone().next() == Some('`') => {
                chars.next();
                name.push('`');
            }
            '`' => return Some(name),
            c => name.push(c),
        }
    }
    None
}

/// The property pairs out of a statement [`FakeStarRocksDdl`] itself just
/// ran — `CREATE … PROPERTIES (…)` OR `ALTER … SET (…)` (fix round 1 review,
/// I1). Unlike [`parse_show_create_properties`], which is scoped to a real
/// FE's `SHOW CREATE CATALOG` reply (always the `PROPERTIES (` shape), this
/// takes whatever is between the FIRST `(` and the LAST `)` in the text —
/// sound here because neither statement shape has an unrelated parenthesis
/// before the property list (the catalog name is backtick-quoted, not
/// paren-quoted).
fn parse_statement_properties(sql: &str) -> BTreeMap<String, String> {
    match (sql.find('('), sql.rfind(')')) {
        (Some(start), Some(end)) if end > start => parse_property_pairs(&sql[start..=end]),
        _ => BTreeMap::new(),
    }
}

/// What the FE would print back: the same `"k" = "v"` shape, with
/// `CredentialUtil`'s masking applied — `password` and `azure_path_key`
/// DELETED (`CredentialUtil.java:88,91`), the [`FE_MASKED_KEYS`] masked as
/// first-two + `******` + last-two, or `******` when the value is ≤ 4 chars
/// (`CredentialUtil.java:86-103`).
fn show_create_text(name: &str, props: &BTreeMap<String, String>) -> String {
    let body: Vec<String> = props
        .iter()
        .filter(|(k, _)| k.as_str() != "password" && k.as_str() != "azure_path_key")
        .map(|(k, v)| {
            let shown = if FE_MASKED_KEYS.contains(&k.as_str()) {
                if v.chars().count() <= 4 {
                    "******".to_string()
                } else {
                    let c: Vec<char> = v.chars().collect();
                    format!("{}{}{}", c[0], c[1], "******")
                        + &c[c.len() - 2..].iter().collect::<String>()
                }
            } else {
                v.clone()
            };
            format!(r#""{}" = "{}""#, escape_dq(k), escape_dq(&shown))
        })
        .collect();
    format!(
        "CREATE EXTERNAL CATALOG `{}`\nPROPERTIES ({}\n)",
        escape_ident(name),
        body.join(",\n")
    )
}

#[cfg(test)]
mod catalog_listing_tests {
    use super::*;
    use egret_query_api::catalog_listing::ListedKind;

    /// A name with every character that could break out of a quoted form.
    const NASTY: &str = r#"Mi"x`e.d\"#;

    fn row(cells: &[Option<&str>]) -> Vec<Option<String>> {
        cells.iter().map(|c| c.map(String::from)).collect()
    }

    fn fe() -> FeTarget {
        FeTarget {
            host: "fe".into(),
            port: 9030,
            root_password: Some("hunter2xyz".into()),
            tls_cert_pem: None,
        }
    }

    #[test]
    fn every_name_is_a_backtick_identifier() {
        assert_eq!(catalog_schemas_sql("ext"), "SHOW DATABASES FROM `ext`");
        assert_eq!(
            catalog_relations_sql("ext", "sales"),
            "SHOW FULL TABLES FROM `ext`.`sales`"
        );
        assert_eq!(
            catalog_columns_sql("ext", "sales", "orders"),
            "SHOW FULL COLUMNS FROM `ext`.`sales`.`orders`"
        );
    }

    /// Only the backtick is doubled. A backslash stays single: inside a
    /// delimited identifier it is not an escape (unlike a string literal,
    /// #504), so doubling it would name a different object.
    #[test]
    fn a_nasty_name_cannot_leave_its_identifier() {
        assert_eq!(
            catalog_schemas_sql(NASTY),
            r#"SHOW DATABASES FROM `Mi"x``e.d\`"#
        );
        assert_eq!(
            catalog_relations_sql(NASTY, "a.b"),
            r#"SHOW FULL TABLES FROM `Mi"x``e.d\`.`a.b`"#
        );
        assert_eq!(
            catalog_columns_sql("ext", NASTY, "`"),
            r#"SHOW FULL COLUMNS FROM `ext`.`Mi"x``e.d\`.````"#
        );
    }

    #[test]
    fn schemas_are_sorted_without_information_schema() {
        // S1's rows; `system` is a source schema on StarRocks and is kept.
        let got = parse_catalog_schemas(vec![
            row(&[Some("sales")]),
            row(&[Some("information_schema")]),
            row(&[Some("pg_catalog")]),
            row(&[Some("empty_s")]),
            row(&[Some("system")]),
        ])
        .unwrap();
        assert_eq!(got, ["empty_s", "pg_catalog", "sales", "system"]);
    }

    #[test]
    fn relations_carry_the_table_type_as_their_kind() {
        let got = parse_catalog_relations(vec![
            row(&[Some("v_orders"), Some("VIEW")]),
            row(&[Some("orders"), Some("BASE TABLE")]),
        ])
        .unwrap();
        assert_eq!(
            got,
            [
                ListedRelation {
                    name: "orders".into(),
                    kind: ListedKind::Table
                },
                ListedRelation {
                    name: "v_orders".into(),
                    kind: ListedKind::View
                },
            ]
        );
        let err = parse_catalog_relations(vec![row(&[Some("t"), Some("SYNONYM")])]).unwrap_err();
        assert!(err.contains("SYNONYM"), "{err}");
    }

    #[test]
    fn columns_are_field_and_type_in_order() {
        // S4's `SHOW FULL COLUMNS` answer: Field, Type, Collation, Null, …
        let got = parse_catalog_columns(vec![
            row(&[Some("id"), Some("int"), None, Some("YES")]),
            row(&[Some("amount"), Some("decimal(10,2)"), None, Some("YES")]),
            row(&[Some("note"), Some("varchar(1048576)"), None, Some("YES")]),
        ])
        .unwrap();
        let pairs: Vec<(&str, &str)> = got
            .iter()
            .map(|c| (c.name.as_str(), c.r#type.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("id", "int"),
                ("amount", "decimal(10,2)"),
                ("note", "varchar(1048576)")
            ]
        );
    }

    #[test]
    fn a_malformed_answer_is_an_error() {
        assert!(parse_catalog_columns(vec![]).is_err());
        assert!(parse_catalog_schemas(vec![row(&[None])]).is_err());
        assert!(parse_catalog_schemas(vec![row(&[])]).is_err());
        assert!(parse_catalog_relations(vec![row(&[Some("t")])]).is_err());
        assert!(parse_catalog_relations(vec![row(&[Some("t"), None])]).is_err());
        assert!(parse_catalog_columns(vec![row(&[Some("id"), None])]).is_err());
    }

    #[tokio::test]
    async fn each_level_is_one_statement_on_the_fe() {
        let ddl = FakeStarRocksDdl::default();
        ddl.answer_query(
            &catalog_schemas_sql("ext"),
            vec![row(&[Some("sales")]), row(&[Some("information_schema")])],
        );
        ddl.answer_query(
            &catalog_relations_sql("ext", "sales"),
            vec![row(&[Some("orders"), Some("BASE TABLE")])],
        );
        ddl.answer_query(
            &catalog_columns_sql("ext", "sales", "orders"),
            vec![row(&[Some("id"), Some("int")])],
        );
        let fe = fe();
        assert_eq!(
            select_catalog_level(&ddl, &fe, "ext", None, None).await,
            Ok(CatalogLevel::Schemas(vec!["sales".into()]))
        );
        assert_eq!(
            select_catalog_level(&ddl, &fe, "ext", Some("sales"), None).await,
            Ok(CatalogLevel::Relations(vec![ListedRelation {
                name: "orders".into(),
                kind: ListedKind::Table
            }]))
        );
        assert_eq!(
            select_catalog_level(&ddl, &fe, "ext", Some("sales"), Some("orders")).await,
            Ok(CatalogLevel::Columns(vec![ListedColumn {
                name: "id".into(),
                r#type: "int".into()
            }]))
        );
        assert_eq!(
            ddl.queried(),
            [
                catalog_schemas_sql("ext"),
                catalog_relations_sql("ext", "sales"),
                catalog_columns_sql("ext", "sales", "orders"),
            ]
        );
    }

    #[tokio::test]
    async fn a_missing_schema_is_an_error_without_the_root_password() {
        let ddl = FakeStarRocksDdl::default();
        ddl.fail_matching(
            "nosuch",
            "ERROR 5501: Unknown database nosuch for root using password hunter2xyz",
        );
        let err = select_catalog_level(&ddl, &fe(), "ext", Some("nosuch"), None)
            .await
            .unwrap_err();
        assert!(err.contains("5501"), "{err}");
        assert!(
            !err.contains("hunter2xyz"),
            "the root password leaked: {err}"
        );
    }

    #[tokio::test]
    async fn a_table_without_a_schema_sends_nothing() {
        let ddl = FakeStarRocksDdl::default();
        assert!(select_catalog_level(&ddl, &fe(), "ext", None, Some("t"))
            .await
            .is_err());
        assert!(ddl.queried().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_catalog_registry::PropValue;
    use uuid::Uuid;

    fn fe() -> FeTarget {
        FeTarget {
            host: "fe.test".into(),
            port: 9030,
            root_password: None,
            tls_cert_pem: None,
        }
    }

    // ---- select_materialized_views (view governance A4, revised 2026-10-14) ----

    #[tokio::test]
    async fn select_materialized_views_reads_the_named_columns_of_information_schema() {
        let ddl = FakeStarRocksDdl::default();
        ddl.seed_materialized_view(
            "db1",
            "mv_def",
            Some(r#"{"ice.db1.t":"2026-10-13 18:37:36","ice.db1.u":"2026-10-13 18:37:36"}"#),
            "CREATE MATERIALIZED VIEW mv_def AS SELECT id, v FROM ice.db1.t",
        );

        let got = select_materialized_views(&ddl, &fe()).await.unwrap();

        assert_eq!(
            ddl.queried(),
            [MATERIALIZED_VIEWS_SQL],
            "one statement, naming its columns"
        );
        assert_eq!(
            MATERIALIZED_VIEWS_SQL,
            "SELECT TABLE_SCHEMA, TABLE_NAME, BASE_TABLE_REFRESH_VERSION_TIMES, \
             MATERIALIZED_VIEW_DEFINITION FROM information_schema.materialized_views"
        );
        assert_eq!(
            got,
            [MaterializedViewRow {
                database: "db1".into(),
                name: "mv_def".into(),
                base_tables: Some(vec!["ice.db1.t".into(), "ice.db1.u".into()]),
                definition: "CREATE MATERIALIZED VIEW mv_def AS SELECT id, v FROM ice.db1.t".into(),
            }]
        );
    }

    #[test]
    fn base_tables_are_the_keys_of_a_non_empty_json_object_and_nothing_else() {
        assert_eq!(
            parse_base_tables(Some(r#"{"b.d.t":"x","a.d.t":null}"#)),
            Some(vec!["a.d.t".to_string(), "b.d.t".to_string()])
        );
        for raw in [
            None,
            Some(""),
            Some("{}"),
            Some("null"),
            Some(r#"["a.d.t"]"#),
            Some(r#""a.d.t""#),
            Some("not json {"),
        ] {
            assert_eq!(parse_base_tables(raw), None, "{raw:?}");
        }
    }

    #[tokio::test]
    async fn a_row_of_the_wrong_shape_is_an_error_not_a_guess() {
        let ddl = FakeStarRocksDdl::default();
        ddl.seed_raw_materialized_view_row(vec![Some("db1".into()), Some("mv".into())]);
        assert!(select_materialized_views(&ddl, &fe()).await.is_err());

        let ddl = FakeStarRocksDdl::default();
        ddl.seed_raw_materialized_view_row(vec![None, Some("mv".into()), None, None]);
        assert!(
            select_materialized_views(&ddl, &fe()).await.is_err(),
            "a view with no database cannot be reported"
        );
    }

    #[tokio::test]
    async fn select_materialized_views_surfaces_an_fe_failure_instead_of_an_empty_list() {
        let ddl = FakeStarRocksDdl::default();
        ddl.set_fail(Some("connect timed out"));

        let got = select_materialized_views(&ddl, &fe()).await;

        assert!(got.is_err(), "a failed listing is an Err, never Ok(vec![])");
    }

    /// Task 4 review M-7: the connect-timeout text reaches the classification
    /// page, so it must not carry the FE's internal `host:port`. A listener
    /// that accepts and never speaks is an FE that never sends its greeting.
    #[tokio::test]
    async fn a_connect_timeout_does_not_name_the_fe_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = FeTarget {
            host: "127.0.0.1".into(),
            port,
            root_password: None,
            tls_cert_pem: None,
        };

        let err = MysqlDdlRunner::connect_within(&target, Duration::from_millis(200))
            .await
            .expect_err("nothing answers");
        let text = err.scrubbed(&target, &BTreeMap::new());

        assert!(text.contains("timed out"), "{text}");
        assert!(
            !text.contains("127.0.0.1") && !text.contains(&port.to_string()),
            "{text}"
        );
        drop(listener);
    }

    // ---- fe_error / redact_quoted (fix round 2 review, F-14) ----

    /// F-14: `fe_error` itself had no test at all — the reviewer replaced its
    /// two substantive lines (code extraction, capping) with `code = None`
    /// and an uncapped `excerpt = raw`, and every one of the 129 lib tests
    /// still passed. No quote characters here at all, so `redact_quoted` is a
    /// no-op and this isolates the cap + code-extraction mechanism (that gets
    /// its own tests below).
    #[test]
    fn fe_error_extracts_the_code_and_caps_the_excerpt_char_safely() {
        let preamble = "ERROR 5078 (42000): ";
        // Multi-byte UTF-8 scalars straddling the cap boundary: proves the
        // cap can never split a codepoint (it can't, using `.chars()` — but
        // that is exactly the invariant a future rewrite to byte-slicing
        // would break, and this is what would catch it). Space-separated so
        // no single run reaches `MIN_OPAQUE_RUN` and gets redacted out from
        // under this test — that redaction gets its own tests below.
        let body: String = std::iter::repeat_n("é ", 200).collect();
        let raw = format!("{preamble}{body}");
        assert!(
            raw.chars().count() > FE_ERROR_EXCERPT_MAX,
            "the raw text must be longer than the cap for this test to mean anything"
        );

        let e = fe_error(raw);
        assert_eq!(e.code, Some(5078));
        assert_eq!(
            e.excerpt.chars().count(),
            FE_ERROR_EXCERPT_MAX,
            "excerpt={}",
            e.excerpt
        );
        assert!(e.excerpt.starts_with(preamble), "excerpt={}", e.excerpt);
    }

    /// The class this fix round exists to close: an FE truncates the value
    /// it echoes back to a PREFIX, but the quotes around that prefix are
    /// still well-formed (measured live against a real FE). Modeled on the
    /// exact reply the original F-3 investigation captured for a rotated
    /// `password` value `S3CRET-PLAINTEXT…`.
    #[test]
    fn redact_quoted_removes_a_secret_prefix_the_fe_only_echoed_a_truncated_form_of() {
        let raw = "Server error: `ERROR 1064 (HY000): Getting syntax error at line 1, \
                    column 78. Detail message: Unexpected input 'S3CRET', the most \
                    similar input is {'a', 'b'}.'";
        let e = fe_error(raw);
        assert_eq!(e.code, Some(1064));
        assert!(!e.excerpt.contains("S3CRET"), "excerpt={}", e.excerpt);
        // mysql_async's own wrapper is `` `...'`` (asymmetric — opens with a
        // backtick, closes with a bare single quote that pairs with
        // nothing), so a lone trailing `'` surviving is expected punctuation,
        // not a leak. What must be gone is the secret-bearing PAIR.
        assert!(e.excerpt.contains(REDACTED), "excerpt={}", e.excerpt);
        // The unquoted prose — what an operator actually reads — survives.
        assert!(
            e.excerpt.contains("Getting syntax error"),
            "excerpt={}",
            e.excerpt
        );
        assert!(
            e.excerpt.contains("Unexpected input"),
            "excerpt={}",
            e.excerpt
        );
    }

    /// The other half: an error with no quoted fragment at all must survive
    /// byte-for-byte (besides capping, which doesn't trigger here) — the
    /// redaction must not eat prose it has no reason to.
    #[test]
    fn redact_quoted_leaves_quote_free_prose_untouched() {
        let raw = "ERROR 5081 (42000): Getting analyzing error. Detail message: \
                    [type : benchmark] is not supported.";
        let e = fe_error(raw);
        assert_eq!(e.code, Some(5081));
        assert_eq!(
            e.excerpt, raw,
            "no quotes present; must pass through unchanged"
        );
    }

    /// A double- or backtick-quoted fragment is redacted too, not only
    /// single-quoted ones — StarRocks' own DDL echoes use double quotes
    /// around property values (`"password" = "s3cr3t"`), and identifiers are
    /// backtick-quoted.
    #[test]
    fn redact_quoted_covers_double_and_backtick_quotes_too() {
        let raw = r#"rejected: "password" = "s3cr3t" for catalog `sales`"#;
        let e = fe_error(raw);
        assert!(!e.excerpt.contains("s3cr3t"), "excerpt={}", e.excerpt);
        assert!(!e.excerpt.contains("password"), "excerpt={}", e.excerpt);
        assert!(!e.excerpt.contains("sales"), "excerpt={}", e.excerpt);
        assert_eq!(
            e.excerpt,
            format!("rejected: {REDACTED} = {REDACTED} for catalog {REDACTED}")
        );
    }

    // ---- Fix round 3: the four ways round 2's `redact_quoted` alone leaked
    // a secret, all measured live by the re-reviewer, plus the unquoted-URL
    // case round 1 measured. Each of the first four makes the quoting
    // unpairable — a naive single-pass regex cannot tell that apart from
    // ordinary text, which is exactly why `fe_error` never trusts it alone.
    // What saves each case is either `redact_opaque_runs` (when the
    // mis-paired remainder glued to the `<redacted>` marker crosses
    // `MIN_OPAQUE_RUN`) or the fail-closed residual-quote check (when it
    // doesn't) — these tests assert the one thing that must hold either way:
    // the secret is gone. `an_unterminated_echo_no_longer_leaks_the_secret`
    // is the one case short enough to rely on the fail-closed path alone,
    // and asserts that explicitly.

    /// Case 1, the re-reviewer's exact string: a stray apostrophe earlier in
    /// the prose (`couldn't`) re-pairs the regex against the wrong partner,
    /// which shifts the "redacted" span off the secret entirely — round 2's
    /// `redact_quoted` alone produced `FE couldn<redacted>S3CRET-PLAINTEXT'
    /// here`, leaking the secret in the clear.
    #[test]
    fn an_apostrophe_in_prose_no_longer_leaks_the_secret_it_shifts_past() {
        let raw = "FE couldn't parse input 'S3CRET-PLAINTEXT' here";
        let e = fe_error(raw);
        // Round 2's `redact_quoted` alone produced `FE couldn<redacted>S3CRET-PLAINTEXT'
        // here` — the secret in the clear. Here the mis-paired span plus the
        // `<redacted>` marker glued to the leftover `S3CRET-PLAINTEXT'` forms
        // one 20+-char run, which `redact_opaque_runs` sweeps whole — the
        // secret never has to reach the fail-closed check at all. Whichever
        // path actually saves it (redaction or withholding), the one thing
        // that must hold is the secret being gone.
        assert!(!e.excerpt.contains("S3CRET"), "excerpt={}", e.excerpt);
    }

    /// Case 2, the re-reviewer's exact string: a SQL-escaped `\'` inside the
    /// echoed value has the same effect as case 1 — an odd number of `'`
    /// characters, so the last one pairs with nothing.
    #[test]
    fn a_sql_escaped_quote_no_longer_leaks_the_secret_it_shifts_past() {
        let raw = "Unexpected input 'O\\'BRIEN-S3CRET', try again";
        let e = fe_error(raw);
        // Same shape as the apostrophe case: the leftover `BRIEN-S3CRET',`
        // glued to the `<redacted>` marker is long enough for
        // `redact_opaque_runs` to sweep whole.
        assert!(!e.excerpt.contains("S3CRET"), "excerpt={}", e.excerpt);
    }

    /// Case 3, the re-reviewer's exact string: a `\"`-escaped double quote —
    /// reachable from egret's OWN renderer (`escape_dq` emits exactly this
    /// shape for a secret containing a literal `"`), not only from the FE.
    #[test]
    fn a_backslash_escaped_double_quote_no_longer_leaks_the_secret_it_shifts_past() {
        let raw = "rejected \"pass\\\"word-S3CRET\" value";
        let e = fe_error(raw);
        // Same shape again: `word-S3CRET"` glued to the `<redacted>` marker
        // is long enough for `redact_opaque_runs` to sweep whole.
        assert!(!e.excerpt.contains("S3CRET"), "excerpt={}", e.excerpt);
    }

    /// Case 4, the re-reviewer's exact string: the FE's own echo is cut off
    /// mid-value, with no closing quote anywhere — round 2's `redact_quoted`
    /// left this one completely untouched (no pair to match at all).
    #[test]
    fn an_unterminated_echo_no_longer_leaks_the_secret() {
        let raw = "Unexpected input 'S3CRET-PLAINTEXT";
        let e = fe_error(raw);
        assert!(!e.excerpt.contains("S3CRET"), "excerpt={}", e.excerpt);
        assert_eq!(e.excerpt, WITHHELD, "excerpt={}", e.excerpt);
    }

    /// The fix round 1 case, re-closed: an unquoted URL, no quote characters
    /// anywhere in the message, so the fail-closed check must NOT withhold —
    /// `redact_urls` has to do the actual work, and the surrounding unquoted
    /// prose (what an operator reads) must survive. Exact reply the original
    /// F-3 investigation captured for a `driver_url` echoed by a real FE.
    #[test]
    fn an_unquoted_url_is_redacted_without_being_withheld() {
        let raw = "Server error: `ERROR 1064 (HY000): connector create failed {… msg: \
                    Cannot get driver from url: http://127.0.0.1:1/x.jar}'";
        let e = fe_error(raw);
        assert_eq!(e.code, Some(1064));
        assert!(!e.excerpt.contains("127.0.0.1"), "excerpt={}", e.excerpt);
        assert!(!e.excerpt.contains(".jar"), "excerpt={}", e.excerpt);
        assert_ne!(
            e.excerpt, WITHHELD,
            "the unquoted-URL case must not fail closed: excerpt={}",
            e.excerpt
        );
        assert!(e.excerpt.contains(REDACTED), "excerpt={}", e.excerpt);
        assert!(
            e.excerpt.contains("connector create failed"),
            "excerpt={}",
            e.excerpt
        );
        assert!(
            e.excerpt.contains("Cannot get driver from url"),
            "excerpt={}",
            e.excerpt
        );
    }

    /// The other unquoted shape: a bare opaque token (credential, base64
    /// blob, `user:pass`) with no `scheme://` prefix at all.
    #[test]
    fn a_long_unquoted_opaque_token_is_redacted_without_being_withheld() {
        let raw = "auth failed for token AKIAABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 rejected";
        let e = fe_error(raw);
        assert!(
            !e.excerpt
                .contains("AKIAABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"),
            "excerpt={}",
            e.excerpt
        );
        assert_ne!(e.excerpt, WITHHELD, "excerpt={}", e.excerpt);
        assert!(
            e.excerpt.contains("auth failed for token"),
            "excerpt={}",
            e.excerpt
        );
        assert!(e.excerpt.contains("rejected"), "excerpt={}", e.excerpt);
    }

    /// The predicate that decides repair-vs-warn, against the message
    /// StarRocks actually sends. Measured on a live FE 3.x:
    ///
    /// ```text
    /// Getting analyzing error. Detail message: security integration 'egret_oidc' already exists.
    /// ```
    ///
    /// The negative cases matter more than the positive one: treating an
    /// unrelated failure as "already exists" would run an `ALTER` against an
    /// FE that has no integration, which StarRocks answers with `not found` —
    /// a second confusing error instead of the real one.
    ///
    /// It asserts through `fe_error`, not on the bare `says_already_exists`
    /// helper: what the bootstrap actually holds is a [`FeError`], and the
    /// bug fix round 1's F2 names is exactly that the two can disagree.
    #[test]
    fn already_exists_is_decided_for_the_caller_from_the_raw_message() {
        assert!(fe_error(
            "Getting analyzing error. Detail message: security integration 'egret_oidc' already exists."
        )
        .is_already_exists());
        assert!(fe_error("SECURITY INTEGRATION ALREADY EXISTS").is_already_exists());
        assert!(!fe_error(
            "Getting analyzing error. Detail message: security integration 'egret_oidc' not found."
        )
        .is_already_exists());
        assert!(!fe_error("Access denied for user 'root'").is_already_exists());
        assert!(!fe_error("Connection refused (os error 111)").is_already_exists());
    }

    /// Fix round 1, F2 — the regression this decision was moved to prevent.
    /// An FE message whose quoting this module cannot pair (a SQL-escaped
    /// apostrophe, the shape the module doc names as reachable from egret's
    /// own escaping) has its excerpt WITHHELD in full. Asking
    /// `scrubbed(...).contains("already exists")` of that string answers
    /// `false`, so the repair `ALTER SECURITY INTEGRATION` would never run and
    /// a changed `required_issuer` would stay stale for ever. The verdict is
    /// taken from the raw text, so it survives the withholding.
    #[test]
    fn already_exists_survives_an_excerpt_that_is_withheld_entirely() {
        let e = fe_error(
            r#"Getting analyzing error. Detail message: security integration 'egret's oidc' already exists."#,
        );
        let printable = e.scrubbed(&fe(), &BTreeMap::new());
        assert!(
            printable.contains(WITHHELD),
            "premise: this message's quoting cannot be paired, so the excerpt is \
             withheld — if this ever stops being true, find another unpairable \
             message rather than deleting the test; scrubbed={printable}"
        );
        assert!(
            !printable.to_ascii_lowercase().contains("already exists"),
            "premise: the phrase is gone from what a caller may print; scrubbed={printable}"
        );
        assert!(
            e.is_already_exists(),
            "and the DECISION survives it: the repair ALTER must still run"
        );
    }

    /// The other half of the same fix: a capped excerpt. An FE that echoes a
    /// long statement can push the phrase past [`FE_ERROR_EXCERPT_MAX`], which
    /// a `contains` on the excerpt would also answer `false` for.
    #[test]
    fn already_exists_survives_an_excerpt_the_cap_truncates() {
        // Whitespace-separated short words: nothing here is a quoted span, a
        // URL, or a >=MIN_OPAQUE_RUN token, so the redaction passes leave the
        // length alone and the CAP is what moves the phrase out of reach.
        let long = format!(
            "Getting analyzing error. Detail message: {}already exists.",
            "detail word ".repeat(FE_ERROR_EXCERPT_MAX / 4)
        );
        let e = fe_error(&long);
        let printable = e.scrubbed(&fe(), &BTreeMap::new());
        assert!(
            !printable.to_ascii_lowercase().contains("already exists"),
            "premise: the phrase falls outside the cap; scrubbed={printable}"
        );
        assert!(e.is_already_exists());
    }

    /// F1 (final whole-branch review), the positive/negative half: the
    /// "unknown catalog" verdict is taken from the raw FE text, against the
    /// message a real FE sends for `SHOW CREATE CATALOG` on an absent
    /// catalog (`ERR_BAD_CATALOG_ERROR`, code 5078).
    ///
    /// The negatives matter more: reading an unrelated failure as "absent"
    /// would make `converge_attachment` CREATE a catalog that is already
    /// there, and a genuine connection failure would be reported as a
    /// successful read of nothing.
    #[test]
    fn unknown_catalog_is_decided_for_the_caller_from_the_raw_message() {
        assert!(fe_error("ERROR 5078 (HY000): Unknown catalog 'sales'").is_unknown_catalog());
        assert!(fe_error("UNKNOWN CATALOG 'sales'").is_unknown_catalog());
        assert!(!fe_error("ERROR 1064 (HY000): Unknown database 'sales'").is_unknown_catalog());
        assert!(!fe_error("Access denied for user 'root'").is_unknown_catalog());
        assert!(!fe_error("Connection refused (os error 111)").is_unknown_catalog());
    }

    /// F1's point, at the call site that had the defect: `show_create_catalog`
    /// must answer `Ok(None)` for an FE "unknown catalog" reply even when the
    /// excerpt a caller may PRINT no longer carries the phrase.
    ///
    /// Two independent ways for that to happen, both exercised here — the
    /// fail-closed withholding and the [`FE_ERROR_EXCERPT_MAX`] cap. Before
    /// this fix the predicate was `e.excerpt.contains("Unknown catalog")`, so
    /// both answered `false` and the drop-between-the-two-reads race
    /// (`a_catalog_that_vanishes_between_the_two_reads_is_absent_not_drifted`)
    /// came back as a hard `failed` instead of `Absent`.
    #[tokio::test]
    async fn show_create_catalog_reads_absent_from_the_raw_text_not_the_excerpt() {
        for (label, raw) in [
            (
                "withheld: the FE's quoting cannot be paired",
                r#"ERROR 5078 (HY000): Unknown catalog 'sales' in db 'ops"#.to_string(),
            ),
            (
                "capped: the phrase falls past FE_ERROR_EXCERPT_MAX",
                format!(
                    "{}Unknown catalog sales",
                    "detail word ".repeat(FE_ERROR_EXCERPT_MAX / 4)
                ),
            ),
        ] {
            // Premise first: the printable form really has lost the phrase,
            // so this test cannot pass for the wrong reason.
            let e = fe_error(&raw);
            let printable = e.scrubbed(&fe(), &BTreeMap::new());
            assert!(
                !printable.to_ascii_lowercase().contains("unknown catalog"),
                "premise ({label}): the phrase must be gone from the excerpt; \
                 scrubbed={printable}"
            );

            let ddl = FakeStarRocksDdl::default();
            let fe = fe();
            ddl.seed("sales", &[("type", "jdbc")]);
            ddl.fail_matching("SHOW CREATE CATALOG", &raw);
            assert_eq!(
                show_create_catalog(&ddl, &fe, "sales").await,
                Ok(None),
                "({label}) the FE said the catalog is unknown; that is ABSENT"
            );
        }
    }

    /// The other side of the same predicate: an FE error that is NOT about an
    /// unknown catalog must still propagate as an error. A `show_create` that
    /// swallowed a connection failure into `Ok(None)` would make
    /// `converge_attachment` CREATE over a catalog it never managed to read.
    #[tokio::test]
    async fn show_create_catalog_propagates_an_unrelated_failure() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        ddl.seed("sales", &[("type", "jdbc")]);
        ddl.fail_matching("SHOW CREATE CATALOG", "ERROR 1045 (28000): Access denied");
        let err = show_create_catalog(&ddl, &fe, "sales")
            .await
            .expect_err("an unrelated failure is not `absent`");
        assert_eq!(err.code, Some(1045));
    }

    /// The module doc's honest boundary, and the accessor that closes it
    /// (SP3b Task 4, carried from Task 3's review). A SHORT, unquoted secret
    /// in an otherwise well-formed message survives every shape-based pass
    /// here — that is the residual, reproduced rather than asserted away —
    /// and `scrubbed`, which is the only way to read the excerpt at all,
    /// removes it. `Display` carries the FE code and no text, so the
    /// `apply_error = e.to_string()` shortcut can no longer leak.
    #[test]
    fn scrubbed_closes_the_short_unquoted_residual_and_display_carries_no_text() {
        let raw = "ERROR 1064 (HY000): Access denied using password pw-abc for user \"reader\"";
        let e = fe_error(raw);
        assert_eq!(e.code, Some(1064));
        // The residual, stated as a measurement: no pass in this module sees
        // `pw-abc` (8 chars, unquoted, and the one quoted span here is
        // correctly paired, so the fail-closed check finds nothing).
        assert!(
            e.excerpt.contains("pw-abc"),
            "the documented residual changed shape; excerpt={}",
            e.excerpt
        );

        let mut secrets = BTreeMap::new();
        secrets.insert(uuid::Uuid::nil(), "pw-abc".to_string());
        let out = e.scrubbed(&fe(), &secrets);
        assert!(!out.contains("pw-abc"), "scrubbed={out}");
        assert!(out.contains("Access denied"), "diagnosis survives: {out}");
        assert!(out.starts_with("FE error 1064: "), "scrubbed={out}");

        assert_eq!(
            e.to_string(),
            "FE error 1064",
            "Display must carry the code alone — the excerpt leaves only through `scrubbed`"
        );
        assert_eq!(
            FeError {
                code: None,
                excerpt: "whatever".into(),
                already_exists: false,
                unknown_catalog: false,
            }
            .to_string(),
            "FE error (no code)"
        );

        // Fix round 1, F2: `Debug` is the other formatter, and a DERIVED one
        // would print the private excerpt. `?e` is one character from `%e`.
        let dbg = format!("{e:?}");
        assert!(
            !dbg.contains("pw-abc"),
            "the secret leaked through Debug: {dbg}"
        );
        assert!(
            !dbg.contains("Access denied"),
            "no FE text may leave through Debug either: {dbg}"
        );
        assert!(dbg.contains("1064"), "the code still reaches a log: {dbg}");
        assert!(dbg.contains("withheld"), "and says why: {dbg}");
    }

    /// `normalize_mysql_async_wrapper` in isolation: strips EXACTLY the
    /// `": \`" ... "'"` shape `thiserror` produces for `Error::Server` (and
    /// `Driver`/`Url`), and leaves everything else — no wrapper present, or
    /// text that merely happens to end in a quote — untouched.
    #[test]
    fn normalize_mysql_async_wrapper_strips_exactly_the_known_shape() {
        assert_eq!(
            normalize_mysql_async_wrapper("Server error: `ERROR 1064 (HY000): boom'"),
            "Server error: ERROR 1064 (HY000): boom"
        );
        assert_eq!(
            normalize_mysql_async_wrapper("Input/output error: connection reset"),
            "Input/output error: connection reset",
            "no wrapper shape present; must pass through unchanged"
        );
        assert_eq!(
            normalize_mysql_async_wrapper("plain text ending in a quote'"),
            "plain text ending in a quote'",
            "ends in a quote but has no `: \\`` — not this wrapper"
        );
    }

    /// The `MIN_OPAQUE_RUN` boundary, pinned directly: one character under
    /// is left alone, exactly at it is redacted.
    #[test]
    fn redact_opaque_runs_threshold_is_exactly_min_opaque_run() {
        let just_under = "x".repeat(MIN_OPAQUE_RUN - 1);
        let just_at = "x".repeat(MIN_OPAQUE_RUN);
        assert_eq!(
            redact_opaque_runs(&just_under),
            just_under,
            "one char under the threshold must survive untouched"
        );
        assert_eq!(
            redact_opaque_runs(&just_at),
            REDACTED,
            "exactly at the threshold must be redacted"
        );
    }

    /// The shape `SHOW CREATE CATALOG` really returns: the statement text with
    /// `"k" = "v"` pairs, `,\n` between them, `\"` escaped inside values, and
    /// NO stable order (the FE prints a HashMap — `PrintableMap`,
    /// `ShowExecutor.java:3079-3081`). So egret parses the pairs; it never
    /// diffs the text.
    const SHOW_CREATE: &str = "CREATE EXTERNAL CATALOG `sales`\nPROPERTIES (\"type\" = \"jdbc\",\n\"user\" = \"u\",\n\"jdbc_uri\" = \"jdbc:postgresql://db:5432/sales?sslmode=require\",\n\"driver_class\" = \"org.postgresql.Driver\",\n\"checksum\" = \"2b1c…\",\n\"aws.s3.access_key\" = \"AK******LE\"\n)";

    /// The `StarRocksCatalogDdl` the constant above describes: a jdbc catalog
    /// named `sales` with a plain `type`/`user`/`jdbc_uri`/`driver_class` and a
    /// secret-valued `password` — hand-built rather than run through
    /// `render_starrocks`, because this module tests `compare_to_fe` in
    /// isolation from rendering.
    fn jdbc_ddl() -> StarRocksCatalogDdl {
        let mut properties = BTreeMap::new();
        properties.insert("type".to_string(), PropValue::Plain("jdbc".into()));
        properties.insert("user".to_string(), PropValue::Plain("u".into()));
        properties.insert(
            "jdbc_uri".to_string(),
            PropValue::Plain("jdbc:postgresql://db:5432/sales?sslmode=require".into()),
        );
        properties.insert(
            "driver_class".to_string(),
            PropValue::Plain("org.postgresql.Driver".into()),
        );
        properties.insert("password".to_string(), PropValue::Secret(Uuid::nil()));
        StarRocksCatalogDdl {
            name: "sales".into(),
            properties,
        }
    }

    #[test]
    fn properties_are_parsed_out_of_the_statement_the_fe_prints() {
        let p = parse_show_create_properties(SHOW_CREATE);
        assert_eq!(p["type"], "jdbc");
        assert_eq!(p["user"], "u");
        assert_eq!(
            p["jdbc_uri"],
            "jdbc:postgresql://db:5432/sales?sslmode=require"
        );
        assert_eq!(p["checksum"], "2b1c…");
        assert_eq!(p.len(), 6);
        // An escaped quote inside a value does not end it.
        let p = parse_show_create_properties("PROPERTIES (\"a\" = \"x\\\"y\", \"b\" = \"z\")");
        assert_eq!(p["a"], "x\"y");
        assert_eq!(p["b"], "z");
    }

    #[test]
    fn drift_is_decided_on_the_comparable_properties_only() {
        let rendered = jdbc_ddl(); // the same catalog the constant above describes
        let fe = parse_show_create_properties(SHOW_CREATE);
        assert_eq!(compare_to_fe(&rendered, &fe), Drift::Matching);

        // `checksum` is the FE's own (`JDBCConnector.java:74-100`) and must not
        // count as drift. `rendered` never sets `aws.s3.access_key` itself, so
        // the masked noise already present in `fe` (from the constant above)
        // is simply never visited — see `a_masked_plain_key_is_unverifiable_not_matching`
        // for what happens when `rendered` DOES set one.
        let mut fe2 = fe.clone();
        fe2.insert("checksum".into(), "different".into());
        assert_eq!(compare_to_fe(&rendered, &fe2), Drift::Matching);

        // A real difference names the key, and only that key.
        let mut fe3 = fe.clone();
        fe3.insert("user".into(), "someone_else".into());
        assert_eq!(
            compare_to_fe(&rendered, &fe3),
            Drift::Different(vec!["user".into()])
        );

        // A key egret sets that the FE does not have is drift too.
        let mut fe4 = fe.clone();
        fe4.remove("jdbc_uri");
        assert_eq!(
            compare_to_fe(&rendered, &fe4),
            Drift::Different(vec!["jdbc_uri".into()])
        );
    }

    /// `password` is not masked — it is DELETED from the output
    /// (`CredentialUtil.java:88,91`), and so are our own secret-valued keys by
    /// construction. A rotated credential is therefore INVISIBLE to drift
    /// detection, which is exactly why re-apply always ALTERs (D3).
    #[test]
    fn a_secret_valued_key_is_never_compared() {
        let rendered = jdbc_ddl();
        let mut fe = parse_show_create_properties(SHOW_CREATE);
        assert!(!fe.contains_key("password"));
        assert_eq!(compare_to_fe(&rendered, &fe), Drift::Matching);
        fe.insert("password".into(), "whatever".into());
        assert_eq!(compare_to_fe(&rendered, &fe), Drift::Matching);
    }

    /// I2 (fix round 1): a `PropValue::Plain` key the FE happens to mask
    /// (`aws.s3.access_key`) must never read as `Matching` — masked means
    /// "cannot tell", not "unchanged". A genuinely-absent masked key is,
    /// conversely, conclusive: the FE simply does not have that property.
    #[test]
    fn a_masked_plain_key_is_unverifiable_not_matching() {
        let mut properties = BTreeMap::new();
        properties.insert("type".to_string(), PropValue::Plain("iceberg".into()));
        properties.insert(
            "aws.s3.access_key".to_string(),
            PropValue::Plain("AKIAROTATEDKEY".into()),
        );
        let rendered = StarRocksCatalogDdl {
            name: "wh".into(),
            properties,
        };

        // The FE shows the masked placeholder, of some OTHER key's value —
        // egret has no way to tell it apart from an unchanged one.
        let mut fe = BTreeMap::new();
        fe.insert("type".into(), "iceberg".into());
        fe.insert("aws.s3.access_key".into(), "AK******EY".into());
        assert_eq!(
            compare_to_fe(&rendered, &fe),
            Drift::Unverifiable(vec!["aws.s3.access_key".into()])
        );

        // A conclusive difference elsewhere always wins over "unverifiable".
        let mut fe2 = fe.clone();
        fe2.insert("type".into(), "hive".into());
        assert_eq!(
            compare_to_fe(&rendered, &fe2),
            Drift::Different(vec!["type".into()])
        );

        // Genuinely absent (not masked) IS conclusive: the FE never got it.
        let mut fe3 = fe.clone();
        fe3.remove("aws.s3.access_key");
        assert_eq!(
            compare_to_fe(&rendered, &fe3),
            Drift::Different(vec!["aws.s3.access_key".into()])
        );
    }

    // ---- FakeStarRocksDdl (fix round 1 review, F-6: previously zero coverage) ----

    #[tokio::test]
    async fn fake_create_then_show_round_trips() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        ddl.execute(
            &fe,
            "CREATE EXTERNAL CATALOG `c` PROPERTIES (\"type\" = \"jdbc\", \"user\" = \"u1\")",
        )
        .await
        .unwrap();
        assert!(show_catalogs(&ddl, &fe).await.unwrap().contains("c"));
        let props = show_create_catalog(&ddl, &fe, "c").await.unwrap().unwrap();
        assert_eq!(props["type"], "jdbc");
        assert_eq!(props["user"], "u1");
        assert_eq!(ddl.created(), vec!["c".to_string()]);
    }

    /// The bug this fix round exists to close (I1): before the fix, this
    /// mutation-proven scenario left `user` at `u1` after the `ALTER`.
    #[tokio::test]
    async fn fake_alter_then_show_really_changes_the_value() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        ddl.execute(
            &fe,
            "CREATE EXTERNAL CATALOG `c` PROPERTIES (\"type\" = \"jdbc\", \"user\" = \"u1\")",
        )
        .await
        .unwrap();
        ddl.execute(&fe, "ALTER CATALOG `c` SET (\"user\" = \"u2\")")
            .await
            .unwrap();
        let props = show_create_catalog(&ddl, &fe, "c").await.unwrap().unwrap();
        assert_eq!(props["user"], "u2", "ALTER must have merged, not no-op'd");
        // `type` is untouched — ALTER merges, it doesn't replace.
        assert_eq!(props["type"], "jdbc");
    }

    /// `ALTER` on a name the fake never had is a silent no-op, mirroring the
    /// real FE (`CatalogMgr.java:184-187`).
    #[tokio::test]
    async fn fake_alter_of_an_unknown_catalog_is_a_silent_noop() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        ddl.execute(&fe, "ALTER CATALOG `ghost` SET (\"user\" = \"u2\")")
            .await
            .unwrap();
        assert!(ddl.catalog("ghost").is_none());
    }

    #[tokio::test]
    async fn fake_drop_then_show_reports_absent() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        ddl.execute(
            &fe,
            "CREATE EXTERNAL CATALOG `c` PROPERTIES (\"type\" = \"jdbc\")",
        )
        .await
        .unwrap();
        ddl.execute(&fe, "DROP CATALOG IF EXISTS `c`")
            .await
            .unwrap();
        assert!(!show_catalogs(&ddl, &fe).await.unwrap().contains("c"));
        assert!(show_create_catalog(&ddl, &fe, "c").await.unwrap().is_none());
        // `IF EXISTS` on an already-gone catalog still succeeds (D7).
        ddl.execute(&fe, "DROP CATALOG IF EXISTS `c`")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn fake_unknown_catalog_is_ok_none_not_an_error_that_escapes() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        assert!(show_create_catalog(&ddl, &fe, "no_such_cat")
            .await
            .unwrap()
            .is_none());
    }

    /// F-7: a rejected statement must appear in `executed()` too — a test
    /// has to be able to assert both "we sent it" and "it failed".
    #[tokio::test]
    async fn fake_a_rejected_statement_is_still_recorded_as_executed() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        ddl.fail_catalog("c", "boom");
        let err = ddl
            .execute(
                &fe,
                "CREATE EXTERNAL CATALOG `c` PROPERTIES (\"type\" = \"jdbc\")",
            )
            .await
            .unwrap_err();
        assert_eq!(err.excerpt, "boom");
        assert_eq!(ddl.executed().len(), 1, "the rejected attempt was recorded");
        assert!(ddl.catalog("c").is_none(), "and it did not take effect");
    }

    /// F-7: `query` must honour `selective_fail` too — before this fix only
    /// `execute` did, so "this one `SHOW CREATE CATALOG` fails" could only be
    /// expressed with the global `set_fail`, which also broke `SHOW
    /// CATALOGS` — exactly the shape D3's read-before-create needs to avoid.
    #[tokio::test]
    async fn fake_query_can_be_selectively_failed_without_breaking_other_queries() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        ddl.execute(
            &fe,
            "CREATE EXTERNAL CATALOG `ok` PROPERTIES (\"type\" = \"jdbc\")",
        )
        .await
        .unwrap();
        ddl.fail_matching("SHOW CREATE CATALOG `ok`", "simulated read failure");

        let err = show_create_catalog(&ddl, &fe, "ok").await.unwrap_err();
        assert_eq!(err.excerpt, "simulated read failure");
        // SHOW CATALOGS is a different statement text — unaffected.
        assert!(show_catalogs(&ddl, &fe).await.unwrap().contains("ok"));
    }

    /// A "timeout" is, to any consumer of this trait, just an `Err` whose
    /// text says so — the fake models it the same way `MysqlDdlRunner` does
    /// once `tokio::time::timeout` fires (I4), without an actual delay.
    #[tokio::test]
    async fn fake_can_model_a_timeout_shaped_failure() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        ddl.set_fail(Some("statement timed out after 30s"));
        let err = ddl.query(&fe, "SHOW CATALOGS").await.unwrap_err();
        assert!(err.excerpt.contains("timed out"), "excerpt={}", err.excerpt);
        let err = ddl
            .execute(
                &fe,
                "CREATE EXTERNAL CATALOG `x` PROPERTIES (\"type\" = \"jdbc\")",
            )
            .await
            .unwrap_err();
        assert!(err.excerpt.contains("timed out"), "excerpt={}", err.excerpt);
    }

    /// `last_target` proves the `FeTarget` — cert included — really reaches
    /// the fake on every call, which is what a Task 4/5 test asserting D6's
    /// fix will lean on.
    #[tokio::test]
    async fn fake_last_target_is_visible_to_a_test() {
        let ddl = FakeStarRocksDdl::default();
        let fe = FeTarget {
            host: "fe.svc".into(),
            port: 9030,
            root_password: Some("pw".into()),
            tls_cert_pem: Some("CERTPEM".into()),
        };
        ddl.query(&fe, "SHOW CATALOGS").await.unwrap();
        let seen = ddl.last_target();
        assert_eq!(seen.host, "fe.svc");
        assert_eq!(seen.tls_cert_pem.as_deref(), Some("CERTPEM"));
    }

    /// The masking imitation itself: `show_create_text` must actually mask
    /// `FE_MASKED_KEYS` and delete `password`/`azure_path_key`, exactly like
    /// a real FE. Proven by mutation in the fix-round-1 review (gutting this
    /// killed no test) — this is the test that now catches it.
    #[tokio::test]
    async fn fake_show_create_masks_and_deletes_like_a_real_fe() {
        let ddl = FakeStarRocksDdl::default();
        let fe = fe();
        ddl.execute(
            &fe,
            "CREATE EXTERNAL CATALOG `c` PROPERTIES (\
                \"type\" = \"jdbc\", \
                \"password\" = \"s3cr3t\", \
                \"aws.s3.access_key\" = \"AKIAABCDEFGH\")",
        )
        .await
        .unwrap();
        let props = show_create_catalog(&ddl, &fe, "c").await.unwrap().unwrap();
        assert!(!props.contains_key("password"), "props={props:?}");
        assert_eq!(props["aws.s3.access_key"], "AK******GH", "props={props:?}");

        // A ≤4-char masked value collapses to a bare `******`.
        ddl.execute(
            &fe,
            "CREATE EXTERNAL CATALOG `short` PROPERTIES (\
                \"type\" = \"jdbc\", \"aws.s3.access_key\" = \"AB\")",
        )
        .await
        .unwrap();
        let props2 = show_create_catalog(&ddl, &fe, "short")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(props2["aws.s3.access_key"], "******");
    }
}
