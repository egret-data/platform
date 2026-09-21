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
//! is [`FeError`], capped to [`FE_ERROR_EXCERPT_MAX`] with the FE's own error
//! code pulled out where present. Capping and code-extraction happen here,
//! because this is the first code to see the FE's raw text; *secret-aware*
//! scrubbing (`egret_catalog_registry::scrub_secrets`) still needs the
//! statement's own secrets map, which this trait is never given, so a caller
//! that resolved one (Task 4/5's apply path) must still run it over
//! `FeError::excerpt` before the result reaches `apply_error` or a response.
//!
//! SP3b Task 3 introduces this seam without yet wiring it in:
//! `spawn_starrocks_catalog_wire`'s bootstrap and `run_starrocks_ddl` (both in
//! `engines.rs`) keep dialing `mysql_async` directly for now — Tasks 4 and 5
//! are the ones that move the apply path and the bootstrap onto
//! [`StarRocksDdl`] and retire the old call sites. Nothing in production
//! constructs a [`FeTarget`] yet either (fix round 1 review, F-5): only
//! [`fe_target`]'s own test exercises it, so the pinned-cert fix (D6) is
//! *implemented*, not yet *applied* — that lands with Task 4/5's callers.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_catalog_registry::{escape_dq, escape_ident, StarRocksCatalogDdl};

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
    /// already use, and [`fe_target`] will apply to every statement once
    /// Task 4/5 wire a production caller (D6).
    pub tls_cert_pem: Option<String>,
}

/// How much of the FE's raw error text [`FeError::excerpt`] keeps. Long
/// enough to be diagnostic — the FE's own `ERROR <code> (<sqlstate>): …`
/// preamble is well under this — short enough that a truncated statement
/// echo cannot smuggle a long credential through in one piece. **This cap
/// alone does not prove the excerpt is secret-free**: a short credential can
/// still fall entirely inside it (measured live: `Unexpected input
/// 'S3CRET'` from a real FE, well inside 200 chars). Capping and FE
/// error-code extraction are this seam's job; a caller holding the
/// statement's own secrets map must still run `scrub_secrets` over
/// `excerpt` before it is stored or returned.
pub const FE_ERROR_EXCERPT_MAX: usize = 200;

/// The FE's answer to a failed statement, reduced to what may safely leave
/// this module: never the statement (D6 addendum), and never unboundedly
/// long (see [`FE_ERROR_EXCERPT_MAX`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeError {
    /// The FE's own numeric error code (StarRocks' `ERROR <code>
    /// (<sqlstate>): …` convention, e.g. `1064`, `5078`), when the message
    /// contains one. `None` for a failure that never reached the FE at all
    /// (a connect or statement timeout has no FE error code).
    pub code: Option<u32>,
    /// A bounded excerpt of the FE's message, **not** scrubbed against any
    /// particular catalog's secrets — this trait is never given that map.
    pub excerpt: String,
}

impl std::fmt::Display for FeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(c) => write!(f, "FE error {c}: {}", self.excerpt),
            None => write!(f, "{}", self.excerpt),
        }
    }
}

/// Build a [`FeError`] from a raw failure (a `mysql_async::Error`, a timeout
/// message, or — in [`FakeStarRocksDdl`] — a test-authored string): pull the
/// FE's own error code out of the `ERROR <code> (…)` preamble when present,
/// and cap the text at [`FE_ERROR_EXCERPT_MAX`].
fn fe_error(raw: impl std::fmt::Display) -> FeError {
    let raw = raw.to_string();
    static CODE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = CODE.get_or_init(|| {
        regex::Regex::new(r"ERROR (\d+)").expect("static FE error code pattern compiles")
    });
    let code = re.captures(&raw).and_then(|c| c[1].parse::<u32>().ok());
    let excerpt: String = raw.chars().take(FE_ERROR_EXCERPT_MAX).collect();
    FeError { code, excerpt }
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
/// unreachable for five to ten minutes, and Task 4 will call `execute` from
/// an HTTP handler, not only from a background bootstrap with its own poll
/// loop.
const FE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FE_STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

/// The real runner: one `mysql_async` connection per statement, exactly like
/// today's `run_starrocks_ddl` (`engines.rs`) — a one-shot admin statement has
/// no caller-facing row cap/cancellation/result shape to thread through, so
/// this stays direct `mysql_async` rather than `egret_query`'s interactive
/// `StarRocksConnector`.
pub struct MysqlDdlRunner;

impl MysqlDdlRunner {
    async fn connect(fe: &FeTarget) -> Result<mysql_async::Conn, FeError> {
        let opts = crate::engines::starrocks_root_conn_opts(
            &fe.host,
            fe.port,
            fe.root_password.as_deref(),
            fe.tls_cert_pem.as_deref(),
        );
        match tokio::time::timeout(FE_CONNECT_TIMEOUT, mysql_async::Conn::new(opts)).await {
            Ok(Ok(conn)) => Ok(conn),
            Ok(Err(e)) => Err(fe_error(e)),
            Err(_) => Err(fe_error(format!(
                "connect to {}:{} timed out after {FE_CONNECT_TIMEOUT:?}",
                fe.host, fe.port
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
/// (`engines.coordinator_cert`, minted by `ensure_tls_cert`), used whenever the
/// stored spec has `starrocksConfig.delegation.tlsEnabled` — the same condition
/// under which the FE actually serves TLS. Before SP3b the catalog DDL and the
/// root-login probe both passed `None` while the delegation DDL passed the
/// cert, so an OAuth2 credential AND the FE root password crossed the wire in
/// cleartext on a TLS-capable engine (D6). This function is the fix; nothing
/// in production calls it yet (module doc, F-5) — that is Task 4/5's job.
// `pub(crate)`, not the public API: only this crate's apply/bootstrap code
// (Tasks 4/5) calls it. Until they land it has no production caller — only
// `engines::tests::fe_target_resolves_root_password_and_gates_the_cert_on_delegation_tls`
// exercises it — so the plain (non-test) compilation of this crate would
// otherwise warn `dead_code` under `-D warnings`.
#[allow(dead_code)]
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
    let tls = crate::engines::sr_delegation_block(&engine.spec)
        .get("tlsEnabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
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
        Err(e) if e.excerpt.contains("Unknown catalog") => Ok(None),
        Err(e) => Err(e),
    }
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
    catalogs: Arc<Mutex<BTreeMap<String, BTreeMap<String, String>>>>,
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
    #[allow(clippy::type_complexity)]
    on_create: Arc<
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
    pub fn clear_executed(&self) {
        self.executed.lock().unwrap().clear()
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
    /// of the bootstrap's loop (deleting the engine, say).
    pub fn on_create(
        &self,
        hook: impl Fn(String) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync + 'static,
    ) {
        *self.on_create.lock().unwrap() = Some(Arc::new(hook));
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
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            return Err(fe_error(msg));
        }
        if let Some(msg) = self.selective_failure(sql) {
            return Err(fe_error(msg));
        }
        if let Some(name) = statement_target("CREATE EXTERNAL CATALOG", sql) {
            let hook = self.on_create.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(name.clone()).await;
            }
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
