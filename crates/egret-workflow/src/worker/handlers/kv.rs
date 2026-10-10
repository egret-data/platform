//! `io.kestra.plugin.core.kv.{Set, Get, GetKeys, Delete, Put, PurgeKV}` --
//! the core KV tasks W9c Task 5 ports (W9c spec §8), over Tasks 2-3's store
//! (`storage::kv::store`).
//!
//! # The shapes are Kestra's, measured
//!
//! Each class's properties (name, `required`, `default`) and outputs are
//! [`DECLARED`], held by `tests/kv_tasks.rs` against
//! `oracle-exec/w9c-kv.json`'s `schema` cells (egret's answer, the top level
//! of each cell). Every `namespace` defaults to the flow's own
//! (`{{ flow.namespace }}`, as the artifact prints it).
//!
//! # Where egret differs, on purpose (spec §2, §5)
//!
//! - **A task writes only to its flow's own namespace** (`Set`, `Put`,
//!   `Delete`): any other is refused by `nsfiles::access::authorize`
//!   (Write), naming the namespace, before anything is read or written.
//!   Reads (`Get`, `GetKeys`) reach any namespace of the workspace.
//!   `PurgeKV` purges the flow's own namespace and its children, on a dot
//!   boundary.
//! - **`Set` and `Put` render `value` by ONE rule** ([`render_value`], the
//!   user's ruling R-a, 2026-10-07): a string through the typed render
//!   ([`crate::runners::render_typed`]), a YAML map or list kept a map or
//!   list (each string leaf typed, each key rendered as text), any other
//!   leaf as YAML gave it. The artifact's `Set` stores a map as its JSON
//!   text, a STRING (w9c `typed_render.tr_yaml_*`, chosen).
//! - **An expired key is a missing key** for every task (spec §4.5).
//! - **`Get` outputs `value` as the KV store's JSON** (`KvData::to_json`): a
//!   decimal keeps its digits in the stored output, but a later render of
//!   `outputs.<id>.value` reads that JSON as an `f64` (spec §15).
//! - **`PurgeKV`** refuses `expiredOnly: false` (naming `{type: version,
//!   keepAmount: 0}`) and a `version` behaviour with neither or both of
//!   `keepAmount` and `before`; `before` purges the revisions that stopped
//!   being the latest at or before it, never the latest (Kestra's rule, the
//!   user's ruling 2026-10-07); with `keepAmount` a soft-deleted key is
//!   purged entirely. Its default namespaces are the flow's own and its
//!   children (Kestra: every namespace with a flow).
//!
//! # Errors (spec §11)
//!
//! A value the KV store refuses maps through `store::unstorable` by its
//! variant ([`set_value`], [`put_patch`]); `NsError::{Store, Db}` and a pool
//! that cannot connect are infrastructure: `Err`, so the lease lapses and
//! the run is re-claimed with no attempt spent. egret's own bug (`Get`'s
//! walk answering a namespace it did not ask, a panic in the detached purge)
//! fails the run once, "internal error (an egret bug, not retried)", as the
//! store's wrong grant does. A re-claim of the same task
//! run is idempotent: `Set` and `Put` skip a key whose latest revision this
//! run wrote, `Delete` answers `deleted: true` for a key this run deleted,
//! and `PurgeKV` recomputes from the rows as they are.
//!
//! # The hold audit (spec §4.3)
//!
//! Every property, `value` included, is rendered before the first store
//! call, so no render runs while a connection or a lock is held. `Get` and
//! `GetKeys` hold one pool connection for their row reads only; `Get`
//! releases it before `read_value` downloads the object. `Set`, `Put`,
//! `Delete` and `PurgeKV` hold what Task 3's store functions hold (its
//! module doc): one server-side copy and `head` (`Set`), one bounded read
//! and write (`Put`), SQL only (`Delete`), and the object deletes of a
//! purged key under its session lock (`PurgeKV`).

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;

use super::super::{Outcome, TaskHandler, TaskInput};
use super::namespace::{flow_namespace, split, DeclaredClass};
use super::storage::{no_storage, outage, spec};
use crate::logs::LogLevel;
use crate::models::Property;
use crate::runners::{render_str, render_typed, RenderContext};
use crate::storage::kv::key::KvKey;
use crate::storage::kv::store::{
    ancestors, delete, list_live, namespaces_with_kv, purge_key, put_merge, read_value,
    resolve_first_live, rows_page, set, unstorable, Deleted, KvWritten, NewValue, PurgeRow,
};
use crate::storage::kv::value::{coerce, parse_ttl, too_deep, KvData, KvType, KvValue, Ttl};
use crate::storage::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use crate::storage::nsfiles::glob::wildcard_match;
use crate::storage::nsfiles::stage::{ListProp, MAX_NAMESPACES};
use crate::storage::nsfiles::{CreatedBy, NsError};
use crate::storage::FileStore;

/// `Set` (spec §8.1).
pub const SET: &str = "io.kestra.plugin.core.kv.Set";
/// `Get` (spec §8.2).
pub const GET: &str = "io.kestra.plugin.core.kv.Get";
/// `GetKeys` (spec §8.3).
pub const GET_KEYS: &str = "io.kestra.plugin.core.kv.GetKeys";
/// `Delete` (spec §8.4).
pub const DELETE: &str = "io.kestra.plugin.core.kv.Delete";
/// `Put` (spec §8.5).
pub const PUT: &str = "io.kestra.plugin.core.kv.Put";
/// `PurgeKV` (spec §8.6).
pub const PURGE_KV: &str = "io.kestra.plugin.core.kv.PurgeKV";

/// What the artifact prints as every task's `namespace` default.
const FLOW_NAMESPACE: Option<&str> = Some("{{ flow.namespace }}");

/// The classes this module runs: `(class, [(property, required, default)],
/// [outputs])`, each in the order the artifact's plugin API lists them,
/// held against w9c `schema` by `tests/kv_tasks.rs`.
pub const DECLARED: &[DeclaredClass] = &[
    (
        SET,
        &[
            ("key", true, None),
            ("kvDescription", false, None),
            ("kvType", false, None),
            ("namespace", false, FLOW_NAMESPACE),
            ("overwrite", false, Some("true")),
            ("ttl", false, None),
            ("value", true, None),
        ],
        &[],
    ),
    (
        GET,
        &[
            ("errorOnMissing", false, Some("false")),
            ("key", true, None),
            ("namespace", false, FLOW_NAMESPACE),
        ],
        &["value"],
    ),
    (
        GET_KEYS,
        &[
            ("namespace", false, FLOW_NAMESPACE),
            ("prefix", false, None),
        ],
        &["keys"],
    ),
    (
        DELETE,
        &[
            ("errorOnMissing", false, Some("false")),
            ("key", true, None),
            ("namespace", false, FLOW_NAMESPACE),
        ],
        &["deleted"],
    ),
    (
        PUT,
        &[
            ("errorOnMissing", false, Some("false")),
            ("key", true, None),
            ("namespace", false, FLOW_NAMESPACE),
            ("value", true, None),
        ],
        &[],
    ),
    (
        PURGE_KV,
        &[
            (
                "behavior",
                false,
                Some("{\"type\":\"key\",\"expiredOnly\":true}"),
            ),
            ("includeChildNamespaces", false, Some("true")),
            ("keyPattern", false, None),
            ("namespacePattern", false, None),
            ("namespaces", false, None),
        ],
        &["size"],
    ),
];

/// Register the six KV tasks, which need the pool. `runtime::handlers_for`
/// calls this for every deployment, beside `namespace::register`.
pub fn register(
    handlers: &mut super::super::Handlers,
    pool: PgPool,
    files: Option<Arc<FileStore>>,
) {
    handlers.add_publishable(Arc::new(SetKv::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(GetKv::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(GetKeys::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(DeleteKv::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(PutKv::new(pool.clone(), files.clone())));
    handlers.add_publishable(Arc::new(PurgeKv::new(pool, files)));
}

// ---------------------------------------------------------------------------
// shared
// ---------------------------------------------------------------------------

fn not_a_flow_run() -> String {
    "this run is not a task run of a flow's execution, so it has no namespace of its own \
     to read or write the KV store from"
        .to_string()
}

/// egret's own bug, as the store classes one (`store::wrong_grant`): the task
/// fails, loudly and once. NOT `Err`, which lets the lease lapse and
/// re-claims the run for ever (Task 5 review M1).
fn egret_bug(why: impl std::fmt::Display) -> String {
    format!("internal error (an egret bug, not retried): {why}")
}

/// `Get`'s grant for the namespace its walk answered: one it asked for,
/// else egret's bug (a failed run, never re-claimed).
fn grant_for<'g>(grants: &'g [Grant], answered: &str) -> std::result::Result<&'g Grant, String> {
    grants
        .iter()
        .find(|g| g.namespace() == answered)
        .ok_or_else(|| {
            egret_bug(format!(
                "egret's KV walk answered namespace '{answered}', which it did not ask"
            ))
        })
}

/// The detached purge's join: a panic in it is egret's bug, a failed run
/// (Task 5 review M1). A cancelled join (the runtime shutting down) is the
/// worker's: `Err`, re-claimed by another worker.
fn joined<T>(
    joined: std::result::Result<T, tokio::task::JoinError>,
) -> Result<std::result::Result<T, String>> {
    match joined {
        Ok(v) => Ok(Ok(v)),
        Err(e) if e.is_panic() => Ok(Err(egret_bug(format!("the purge panicked: {e}")))),
        Err(e) => Err(Error::Internal(anyhow::anyhow!(
            "{PURGE_KV}: the purge stopped: {e}"
        ))),
    }
}

/// Why a `spawn_blocking` job gave no answer (W9c final review F8).
#[derive(Debug, PartialEq)]
enum Blocked {
    /// It panicked: egret's bug, a failed run.
    Bug(String),
    /// It was cancelled (the runtime shutting down): the worker's, an `Err`
    /// that another worker re-claims.
    Stopped(String),
}

/// A `spawn_blocking` join told apart as [`joined`] tells the purge's:
/// `JoinError::into_panic` itself panics on a cancelled join, so neither
/// case is resumed as a panic here.
fn blocking<T>(
    joined: std::result::Result<T, tokio::task::JoinError>,
    what: &str,
) -> std::result::Result<T, Blocked> {
    match joined {
        Ok(v) => Ok(v),
        Err(e) if e.is_panic() => Err(Blocked::Bug(egret_bug(format!("{what} panicked: {e}")))),
        Err(e) => Err(Blocked::Stopped(format!("{what} stopped: {e}"))),
    }
}

fn db_err(type_id: &str, e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!(
        "{type_id}: the database is unavailable: {e}"
    ))
}

/// An optional property, rendered; `name` names it in the message.
fn opt<T: DeserializeOwned>(
    p: &Option<Property<T>>,
    cx: &RenderContext,
    name: &str,
) -> std::result::Result<Option<T>, String> {
    p.as_ref()
        .map(|p| p.render(cx))
        .transpose()
        .map_err(|e| format!("`{name}`: {e}"))
}

/// A required property, rendered.
fn req<T: DeserializeOwned>(
    p: &Property<T>,
    cx: &RenderContext,
    name: &str,
) -> std::result::Result<T, String> {
    p.render(cx).map_err(|e| format!("`{name}`: {e}"))
}

/// The task run that writes: what the store's re-claim checks key on.
fn by(task: &TaskInput) -> CreatedBy {
    CreatedBy::TaskRun {
        execution: task.execution_id,
        task_run: task.run_id,
    }
}

/// `kvType`, as Kestra's enum binding takes it: one of the seven names,
/// exactly (w9c `set_coercion.type_lower`, `type_bad`).
fn parse_kv_type(s: &str) -> std::result::Result<KvType, String> {
    KvType::parse(s).ok_or_else(|| {
        format!(
            "`kvType` must be one of STRING, NUMBER, BOOLEAN, DATETIME, DATE, DURATION or \
             JSON, and it is {s:?}"
        )
    })
}

/// Kestra's missing-key sentence (`Get.java:91`, `Delete.java`), naming the
/// namespace that was asked: the flow's own when `Get` walks.
fn no_value(key: &KvKey, namespace: &str) -> String {
    format!(
        "No value found for key '{key}' in namespace '{namespace}' and `errorOnMissing` is set \
         to true"
    )
}

/// Whether `v` nests containers deeper than `limit`, walked with an
/// explicit stack, so a value of any depth is judged without recursion.
fn json_deeper_than(v: &Value, limit: usize) -> bool {
    let mut stack: Vec<(&Value, usize)> = vec![(v, 0)];
    while let Some((v, depth)) = stack.pop() {
        let children: Box<dyn Iterator<Item = &Value>> = match v {
            Value::Array(items) => Box::new(items.iter()),
            Value::Object(map) => Box::new(map.values()),
            _ => continue,
        };
        if depth + 1 > limit {
            return true;
        }
        stack.extend(children.map(|c| (c, depth + 1)));
    }
    false
}

/// `Set`'s and `Put`'s ONE render rule (the user's ruling R-a, 2026-10-07;
/// spec §8.1): `raw` is `value` as the spec holds it.
///
/// - A string goes through [`render_typed`]: exactly one `{{ … }}` keeps
///   its type (`{{ 1 + 1 }}` is the number 2; a decimal `kv()` read stays
///   that exact decimal, W9c final review F1), anything else is a string.
/// - An array or object stays one, rendered leaf by leaf: each string leaf
///   through [`render_typed`], each key through [`render_str`].
/// - Every other leaf (number, boolean, null) is kept as YAML gave it, by
///   `KvData::from_json`'s rule (a fraction is a float).
///
/// So `{a: "{{ 1 + 1 }}", b: x}` is `{"a": 2, "b": "x"}` for both tasks
/// (w9c `typed_render.tr_yaml_map`, chosen; `put_merge.templated_leaf`).
///
/// A `raw` nesting containers deeper than `storage::ion::MAX_DEPTH` is
/// refused (`Invalid`, the author's) before anything is rendered, judged
/// with an explicit stack: the walk below then recurses at most that deep.
/// The rendered value is judged again, since a leaf's render may nest.
pub fn render_value(raw: &Value, cx: &RenderContext) -> std::result::Result<KvData, NsError> {
    if json_deeper_than(raw, crate::storage::ion::MAX_DEPTH) {
        return Err(unstorable(too_deep()));
    }
    let data = walk_value(raw, cx, "value")?;
    if data.nests_deeper_than(crate::storage::ion::MAX_DEPTH) {
        return Err(unstorable(too_deep()));
    }
    Ok(data)
}

fn walk_value(raw: &Value, cx: &RenderContext, at: &str) -> std::result::Result<KvData, NsError> {
    let failed = |at: &str, e: crate::runners::RenderError| NsError::Author(format!("`{at}`: {e}"));
    Ok(match raw {
        Value::String(s) => render_typed(s, cx)
            .map_err(|e| failed(at, e))?
            .map_err(unstorable)?,
        Value::Array(items) => KvData::List(
            items
                .iter()
                .enumerate()
                .map(|(i, item)| walk_value(item, cx, &format!("{at}[{i}]")))
                .collect::<std::result::Result<_, _>>()?,
        ),
        Value::Object(map) => {
            let mut out: Vec<(String, KvData)> = Vec::with_capacity(map.len());
            let mut seen = std::collections::HashSet::with_capacity(map.len());
            for (name, item) in map {
                let shown = format!("{at}.{name}");
                let key = render_str(name, cx).map_err(|e| failed(&shown, e))?;
                // Two keys rendering to one text: which value survived would
                // depend on the map's order (Task 5 review M2).
                if !seen.insert(key.clone()) {
                    return Err(NsError::Author(format!(
                        "`{at}`: two keys render to the same key '{key}', so one value would be \
                         lost; render them to different keys"
                    )));
                }
                let item = walk_value(item, cx, &shown)?;
                out.push((key, item));
            }
            KvData::Struct(out)
        }
        other => KvData::from_json(other).map_err(unstorable)?,
    })
}

/// `Set`'s value: [`render_value`], then `coerce` by `kv_type` (spec §8.1).
/// A refusal maps through `store::unstorable` by its VARIANT (`TooLarge`: a
/// limit; `Invalid`: the author's), never by its message.
pub fn set_value(
    raw: &Value,
    kv_type: Option<KvType>,
    cx: &RenderContext,
) -> std::result::Result<KvValue, NsError> {
    let rendered = render_value(raw, cx)?;
    coerce(rendered, kv_type).map_err(unstorable)
}

/// `Put`'s patch: [`render_value`], whose [`KvData`] is the patch.
pub fn put_patch(raw: &Value, cx: &RenderContext) -> std::result::Result<KvData, NsError> {
    render_value(raw, cx)
}

/// The log line of a write a re-claim found already done.
async fn already(task: &TaskInput, key: &KvKey, namespace: &str, revision: u32) {
    task.log
        .line(
            LogLevel::Info,
            &format!(
                "key '{key}' in namespace '{namespace}' was already written by an earlier \
                 attempt of this task run (revision {revision}): not written again"
            ),
        )
        .await;
}

// ---------------------------------------------------------------------------
// Set
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct SetSpec {
    key: Property<String>,
    value: Value,
    #[serde(default)]
    namespace: Option<Property<String>>,
    #[serde(default)]
    overwrite: Option<Property<bool>>,
    #[serde(default)]
    ttl: Option<Property<String>>,
    #[serde(rename = "kvType", default)]
    kv_type: Option<Property<String>>,
    #[serde(rename = "kvDescription", default)]
    kv_description: Option<Property<String>>,
}

/// `core.kv.Set`: one value into the flow's own namespace, through
/// `store::set` (spec §8.1).
#[derive(Debug, Clone)]
pub struct SetKv {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl SetKv {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for SetKv {
    fn type_id(&self) -> &'static str {
        SET
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = SET;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: SetSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        // Everything renders first (the hold audit); then a store outage a
        // render met is the worker's, before any result is used.
        let cx = &task.render;
        let key = req(&spec.key, cx, "key");
        let namespace = opt(&spec.namespace, cx, "namespace");
        let overwrite = opt(&spec.overwrite, cx, "overwrite");
        let ttl = opt(&spec.ttl, cx, "ttl");
        let description = opt(&spec.kv_description, cx, "kvDescription");
        let kv_type = opt(&spec.kv_type, cx, "kvType")
            .and_then(|t| t.as_deref().map(parse_kv_type).transpose());
        let value = match &kv_type {
            Ok(t) => Some(set_value(&spec.value, *t, cx)),
            Err(_) => None,
        };
        outage(T, task)?;
        let (key, namespace, overwrite, ttl, description) =
            match (key, namespace, overwrite, ttl, description) {
                (Ok(k), Ok(n), Ok(o), Ok(t), Ok(d)) => (k, n, o.unwrap_or(true), t, d),
                (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), ..) => return fail(e),
                (.., Err(e), _) | (.., Err(e)) => return fail(e),
            };
        if let Err(e) = kv_type {
            return fail(e);
        }
        let key = match KvKey::parse(&key) {
            Ok(k) => k,
            Err(why) => return fail(format!("key '{key}': {why}")),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };
        let target = namespace.unwrap_or_else(|| own.clone());
        // THE write rule, before the value is judged or anything written.
        let grant = match authorize(
            task.workspace_id,
            Actor::Flow { namespace: &own },
            &target,
            Resource::Kv,
            Access::Write,
        ) {
            Ok(g) => g,
            Err(why) => return fail(why),
        };
        let value = match value.expect("rendered when kvType parsed") {
            Ok(v) => v,
            Err(e) => return split(T, e).map(Outcome::failed_with),
        };
        let ttl = match ttl.as_deref().map(|t| parse_ttl(t, Ttl::Task)).transpose() {
            Ok(t) => t,
            Err(why) => return fail(why),
        };
        let new = NewValue {
            value,
            description,
            ttl,
            overwrite,
        };
        match set(&self.pool, fs, &grant, &key, new, by(task)).await {
            Ok(KvWritten::Written(_)) => {}
            Ok(KvWritten::Already(e)) => already(task, &key, &target, e.revision).await,
            Err(e) => return split(T, e).map(Outcome::failed_with),
        }
        Ok(Outcome::succeeded_with(serde_json::json!({})))
    }

    /// Holds nothing past run: the write commits on its own, on a detached
    /// task (`store::set`).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Get
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct GetSpec {
    key: Property<String>,
    #[serde(default)]
    namespace: Option<Property<String>>,
    #[serde(rename = "errorOnMissing", default)]
    error_on_missing: Option<Property<bool>>,
}

/// `core.kv.Get`: one key's live value (spec §8.2), walking the flow's
/// namespace and its parents when `namespace` is the flow's own (§4.6).
#[derive(Debug, Clone)]
pub struct GetKv {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl GetKv {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for GetKv {
    fn type_id(&self) -> &'static str {
        GET
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = GET;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: GetSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        let cx = &task.render;
        let key = req(&spec.key, cx, "key");
        let namespace = opt(&spec.namespace, cx, "namespace");
        let error_on_missing = opt(&spec.error_on_missing, cx, "errorOnMissing");
        outage(T, task)?;
        let (key, namespace, error_on_missing) = match (key, namespace, error_on_missing) {
            (Ok(k), Ok(n), Ok(e)) => (k, n, e.unwrap_or(false)),
            (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => return fail(e),
        };
        let key = match KvKey::parse(&key) {
            Ok(k) => k,
            Err(why) => return fail(format!("key '{key}': {why}")),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };
        let target = namespace.unwrap_or_else(|| own.clone());
        // §4.6: the walk only when the namespace IS the flow's own.
        let levels: Vec<&str> = if target == own {
            ancestors(&own)
        } else {
            vec![target.as_str()]
        };
        let mut grants: Vec<Grant> = Vec::with_capacity(levels.len());
        for ns in levels {
            match authorize(
                task.workspace_id,
                Actor::Flow { namespace: &own },
                ns,
                Resource::Kv,
                Access::Read,
            ) {
                Ok(g) => grants.push(g),
                Err(why) => return fail(why),
            }
        }
        // The row read on a pool connection, let go BEFORE the object is
        // read: `read_value` may take one more for one statement.
        let found = {
            let mut conn = self.pool.acquire().await.map_err(|e| db_err(T, e))?;
            resolve_first_live(&mut conn, &grants, &key).await
        };
        let entry = match found {
            Ok(Some(e)) => e,
            Ok(None) if error_on_missing => return fail(no_value(&key, &target)),
            // No `value` field, as the artifact's outputs omit a null one
            // (w9c `inheritance.get_top_ns_parent_N1`, measured `{}`): an
            // expression reading `outputs.<id>.value` then fails as on
            // Kestra, and `is defined` is false (Task 5 review, Concern 2).
            Ok(None) => return Ok(Outcome::succeeded_with(serde_json::json!({}))),
            Err(e) => return split(T, e).map(Outcome::failed_with),
        };
        let grant = match grant_for(&grants, &entry.namespace) {
            Ok(g) => g,
            Err(why) => return fail(why),
        };
        match read_value(&self.pool, fs, grant, &entry).await {
            Ok(data) => Ok(Outcome::succeeded_with(
                serde_json::json!({ "value": data.to_json() }),
            )),
            Err(e) => split(T, e).map(Outcome::failed_with),
        }
    }

    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// GetKeys
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct GetKeysSpec {
    #[serde(default)]
    prefix: Option<Property<String>>,
    #[serde(default)]
    namespace: Option<Property<String>>,
}

/// `core.kv.GetKeys`: the live keys of one namespace, never walking, in the
/// order their latest revisions were written (spec §8.3; w9c `get_keys`).
/// Reads rows only, never an object.
#[derive(Debug, Clone)]
pub struct GetKeys {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl GetKeys {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for GetKeys {
    fn type_id(&self) -> &'static str {
        GET_KEYS
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = GET_KEYS;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: GetKeysSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        let cx = &task.render;
        let prefix = opt(&spec.prefix, cx, "prefix");
        let namespace = opt(&spec.namespace, cx, "namespace");
        outage(T, task)?;
        let (prefix, namespace) = match (prefix, namespace) {
            (Ok(p), Ok(n)) => (p, n),
            (Err(e), _) | (_, Err(e)) => return fail(e),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };
        let target = namespace.unwrap_or_else(|| own.clone());
        let grant = match authorize(
            task.workspace_id,
            Actor::Flow { namespace: &own },
            &target,
            Resource::Kv,
            Access::Read,
        ) {
            Ok(g) => g,
            Err(why) => return fail(why),
        };
        let listed = {
            let mut conn = self.pool.acquire().await.map_err(|e| db_err(T, e))?;
            list_live(
                &mut conn,
                &grant,
                prefix.as_deref(),
                fs.caps.max_namespace_files,
            )
            .await
        };
        match listed {
            Ok(entries) => {
                let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
                Ok(Outcome::succeeded_with(serde_json::json!({ "keys": keys })))
            }
            Err(e) => split(T, e).map(Outcome::failed_with),
        }
    }

    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct DeleteSpec {
    key: Property<String>,
    #[serde(default)]
    namespace: Option<Property<String>>,
    #[serde(rename = "errorOnMissing", default)]
    error_on_missing: Option<Property<bool>>,
}

/// `core.kv.Delete`: a soft delete of one key of the flow's own namespace
/// (spec §8.4). Never walks.
#[derive(Debug, Clone)]
pub struct DeleteKv {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl DeleteKv {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for DeleteKv {
    fn type_id(&self) -> &'static str {
        DELETE
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = DELETE;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: DeleteSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        if self.files.is_none() {
            return Ok(Outcome::failed_with(no_storage(T)));
        }
        let cx = &task.render;
        let key = req(&spec.key, cx, "key");
        let namespace = opt(&spec.namespace, cx, "namespace");
        let error_on_missing = opt(&spec.error_on_missing, cx, "errorOnMissing");
        outage(T, task)?;
        let (key, namespace, error_on_missing) = match (key, namespace, error_on_missing) {
            (Ok(k), Ok(n), Ok(e)) => (k, n, e.unwrap_or(false)),
            (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => return fail(e),
        };
        let key = match KvKey::parse(&key) {
            Ok(k) => k,
            Err(why) => return fail(format!("key '{key}': {why}")),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };
        let target = namespace.unwrap_or_else(|| own.clone());
        let grant = match authorize(
            task.workspace_id,
            Actor::Flow { namespace: &own },
            &target,
            Resource::Kv,
            Access::Write,
        ) {
            Ok(g) => g,
            Err(why) => return fail(why),
        };
        let deleted = match delete(&self.pool, &grant, &key, by(task)).await {
            Ok(Deleted::Deleted) => true,
            // A re-claim: this task run deleted the latest revision (§8.4).
            Ok(Deleted::Already) => {
                task.log
                    .line(
                        LogLevel::Info,
                        &format!(
                            "key '{key}' in namespace '{target}' was already deleted by an \
                             earlier attempt of this task run"
                        ),
                    )
                    .await;
                true
            }
            Ok(Deleted::Missing) if error_on_missing => return fail(no_value(&key, &target)),
            Ok(Deleted::Missing) => false,
            Err(e) => return split(T, e).map(Outcome::failed_with),
        };
        Ok(Outcome::succeeded_with(
            serde_json::json!({ "deleted": deleted }),
        ))
    }

    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Put
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct PutSpec {
    key: Property<String>,
    value: Value,
    #[serde(default)]
    namespace: Option<Property<String>>,
    #[serde(rename = "errorOnMissing", default)]
    error_on_missing: Option<Property<bool>>,
}

/// `core.kv.Put`: `value` deep-merged into the key's live value under the
/// key's lock, through `store::put_merge` (spec §8.5).
#[derive(Debug, Clone)]
pub struct PutKv {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl PutKv {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for PutKv {
    fn type_id(&self) -> &'static str {
        PUT
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = PUT;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: PutSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        let cx = &task.render;
        let key = req(&spec.key, cx, "key");
        let namespace = opt(&spec.namespace, cx, "namespace");
        let error_on_missing = opt(&spec.error_on_missing, cx, "errorOnMissing");
        let patch = put_patch(&spec.value, cx);
        outage(T, task)?;
        let (key, namespace, error_on_missing) = match (key, namespace, error_on_missing) {
            (Ok(k), Ok(n), Ok(e)) => (k, n, e.unwrap_or(false)),
            (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => return fail(e),
        };
        let key = match KvKey::parse(&key) {
            Ok(k) => k,
            Err(why) => return fail(format!("key '{key}': {why}")),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };
        let target = namespace.unwrap_or_else(|| own.clone());
        let grant = match authorize(
            task.workspace_id,
            Actor::Flow { namespace: &own },
            &target,
            Resource::Kv,
            Access::Write,
        ) {
            Ok(g) => g,
            Err(why) => return fail(why),
        };
        let patch = match patch {
            Ok(p) => p,
            Err(e) => return split(T, e).map(Outcome::failed_with),
        };
        match put_merge(
            &self.pool,
            fs,
            &grant,
            &key,
            patch,
            error_on_missing,
            by(task),
        )
        .await
        {
            Ok(KvWritten::Written(_)) => {}
            Ok(KvWritten::Already(e)) => already(task, &key, &target, e.revision).await,
            Err(e) => return split(T, e).map(Outcome::failed_with),
        }
        Ok(Outcome::succeeded_with(serde_json::json!({})))
    }

    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PurgeKV
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct PurgeSpec {
    #[serde(rename = "keyPattern", default)]
    key_pattern: Option<Property<String>>,
    #[serde(default)]
    namespaces: Option<Value>,
    #[serde(rename = "namespacePattern", default)]
    namespace_pattern: Option<Property<String>>,
    #[serde(default)]
    behavior: Option<Property<BehaviorSpec>>,
    #[serde(rename = "includeChildNamespaces", default)]
    include_child_namespaces: Option<Property<bool>>,
}

/// Kestra's `PurgeKV` behaviours (`Key.java`, `Version.java`): `{type: key,
/// expiredOnly}` or `{type: version, keepAmount | before}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BehaviorSpec {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(rename = "expiredOnly", default)]
    expired_only: Option<Value>,
    #[serde(rename = "keepAmount", default)]
    keep_amount: Option<Value>,
    #[serde(default)]
    before: Option<Value>,
}

/// Which revisions of a key a purge removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Behavior {
    /// `{type: key, expiredOnly: true}`: every revision of a key whose
    /// latest revision has expired.
    Expired,
    /// `{type: version, keepAmount: n}`: all but the `n` highest; a
    /// soft-deleted key entirely (W9b's rule).
    Keep(u64),
    /// `{type: version, before: t}`: each revision that stopped being the
    /// latest at or before `t`; never the latest (Kestra's rule).
    Before(DateTime<Utc>),
}

fn behavior_rule(b: Option<BehaviorSpec>) -> std::result::Result<Behavior, String> {
    let Some(b) = b else {
        return Ok(Behavior::Expired);
    };
    match b.kind.as_deref() {
        Some("key") => {
            if b.keep_amount.is_some() || b.before.is_some() {
                return Err(
                    "`behavior` of type \"key\" takes only `expiredOnly`; `keepAmount` and \
                     `before` belong to type \"version\""
                        .to_string(),
                );
            }
            let expired_only = match b.expired_only {
                None | Some(Value::Null) | Some(Value::Bool(true)) => true,
                Some(Value::Bool(false)) => false,
                Some(Value::String(s)) if s.trim().eq_ignore_ascii_case("true") => true,
                Some(Value::String(s)) if s.trim().eq_ignore_ascii_case("false") => false,
                Some(other) => {
                    return Err(format!(
                        "`behavior.expiredOnly` must be true or false, and it is {other}"
                    ))
                }
            };
            if !expired_only {
                // The user's decision (2026-10-07, spec §8.6).
                return Err(
                    "`behavior: {type: key, expiredOnly: false}` is refused: egret's PurgeKV \
                     purges expired keys, or revisions by version; to purge every revision of \
                     every key, write `behavior: {type: version, keepAmount: 0}`"
                        .to_string(),
                );
            }
            Ok(Behavior::Expired)
        }
        Some("version") => {
            if b.expired_only.is_some() {
                return Err(
                    "`behavior` of type \"version\" takes `keepAmount` or `before`; \
                     `expiredOnly` belongs to type \"key\""
                        .to_string(),
                );
            }
            let keep = match b.keep_amount {
                None | Some(Value::Null) => None,
                Some(Value::Number(n)) if n.as_u64().is_some() => n.as_u64(),
                Some(Value::String(s)) if s.trim().parse::<u64>().is_ok() => s.trim().parse().ok(),
                Some(other) => {
                    return Err(format!(
                        "`behavior.keepAmount` must be a whole number, 0 or more, and it is \
                         {other}"
                    ))
                }
            };
            let before = match b.before {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(
                    DateTime::parse_from_rfc3339(s.trim())
                        .map(|t| t.with_timezone(&Utc))
                        .map_err(|e| {
                            format!(
                                "`behavior.before` must be an ISO 8601 instant such as \
                                 2025-01-01T00:00:00Z, and it is {s:?}: {e}"
                            )
                        })?,
                ),
                Some(other) => {
                    return Err(format!(
                        "`behavior.before` must be an ISO 8601 instant such as \
                         2025-01-01T00:00:00Z, and it is {other}"
                    ))
                }
            };
            match (keep, before) {
                // Kestra's validator, word for word (w9c `purge.version_before_keep1`).
                (Some(_), Some(_)) => Err(
                    "behavior: Cannot set both 'before' and 'keepAmount' properties".to_string(),
                ),
                (Some(n), None) => Ok(Behavior::Keep(n)),
                (None, Some(t)) => Ok(Behavior::Before(t)),
                // The user's decision (2026-10-07): refused, where Kestra
                // purges every revision of every key.
                (None, None) => Err(
                    "`behavior` of type \"version\" sets neither `keepAmount` nor `before`, so \
                     it names nothing to keep: set `keepAmount` (the newest revisions kept per \
                     key) or `before` (an instant); `keepAmount: 0` purges every revision"
                        .to_string(),
                ),
            }
        }
        other => Err(format!(
            "`behavior.type` must be \"key\" or \"version\", and it is {}",
            other.map_or("missing".to_string(), |k| format!("{k:?}"))
        )),
    }
}

/// The row ids of ONE key (`rows`, every revision ascending, read under
/// the key's lock by `purge_key`) that `behavior` purges.
fn judge(rows: &[PurgeRow], behavior: Behavior) -> Vec<i64> {
    let Some(latest) = rows.last().map(|r| &r.entry) else {
        return Vec::new();
    };
    let all = || rows.iter().map(|r| r.entry.id).collect();
    match behavior {
        Behavior::Expired if latest.expired => all(),
        Behavior::Expired => Vec::new(),
        Behavior::Keep(_) if latest.deleted => all(),
        Behavior::Keep(n) => {
            let n = usize::try_from(n).unwrap_or(usize::MAX);
            rows[..rows.len().saturating_sub(n)]
                .iter()
                .map(|r| r.entry.id)
                .collect()
        }
        // The latest has no successor: never purged by `before`.
        Behavior::Before(t) => rows
            .iter()
            .filter(|r| r.superseded_at.is_some_and(|s| s <= t))
            .map(|r| r.entry.id)
            .collect(),
    }
}

/// `ns` is a child of `parent` on a dot boundary.
fn child_of(ns: &str, parent: &str) -> bool {
    ns.strip_prefix(parent)
        .is_some_and(|rest| rest.starts_with('.'))
}

/// The namespaces a purge covers (spec §8.6), before `authorize`.
/// `candidates` are the namespaces holding KV that are `own` or its
/// children (already filtered):
/// - `pattern`: Kestra's wildcard over the candidates, NEVER expanded to
///   their children (`PurgeTask.findNamespaces`; w9c
///   `purge.namespaces.namespacePattern_exact`);
/// - `listed`: each, plus (with `include_children`) every candidate that is
///   its child;
/// - neither: `own`, plus (with `include_children`) every candidate.
///
/// Sorted, each once. CPU on a pattern of any length: run on the blocking
/// pool.
fn purge_namespaces(
    own: &str,
    listed: Option<&[String]>,
    pattern: Option<&str>,
    include_children: bool,
    candidates: &[String],
) -> Vec<String> {
    let mut out = BTreeSet::new();
    if let Some(pattern) = pattern {
        out.extend(
            candidates
                .iter()
                .filter(|ns| wildcard_match(pattern, ns))
                .cloned(),
        );
        return out.into_iter().collect();
    }
    match listed {
        Some(roots) => {
            for root in roots {
                if include_children {
                    out.extend(candidates.iter().filter(|ns| child_of(ns, root)).cloned());
                }
                out.insert(root.clone());
            }
        }
        // The candidates ARE the flow's own namespace and its children: the
        // dot boundary is `namespaces_with_kv`'s, decided once, in SQL.
        None => {
            if include_children {
                out.extend(candidates.iter().cloned());
            }
            out.insert(own.to_string());
        }
    }
    out.into_iter().collect()
}

/// One page's COMPLETE keys (every row of each, by key then revision) that
/// `key_pattern` takes. CPU on a pattern of any length: on the blocking pool.
fn page_keys(rows: &[crate::storage::kv::store::KvEntry], key_pattern: Option<&str>) -> Vec<KvKey> {
    let mut out: Vec<KvKey> = Vec::new();
    for e in rows {
        if out.last() == Some(&e.key) {
            continue;
        }
        if key_pattern.is_some_and(|p| !wildcard_match(p, e.key.as_str())) {
            continue;
        }
        out.push(e.key.clone());
    }
    out
}

/// The purge (spec §8.6), namespace by namespace: rows in keyset pages of
/// at most `cap + 1` by `(key, revision)`; each page's complete keys that
/// `key_pattern` takes go to `purge_key`, which re-reads the key's rows
/// under its session lock and purges what [`judge`] picks from THOSE rows;
/// the last, possibly partial key starts the next page. One key holding
/// more than `cap` revisions that the pattern takes fails the task, naming
/// it, with the keys before it purged (`Ok(Err)`). Answers the revisions
/// purged.
async fn run_purge(
    pool: PgPool,
    fs: Arc<FileStore>,
    grants: Vec<Grant>,
    key_pattern: Option<Arc<str>>,
    behavior: Behavior,
    cap: u64,
) -> std::result::Result<std::result::Result<u64, String>, NsError> {
    let mut purged = 0u64;
    for grant in &grants {
        let mut after: Option<KvKey> = None;
        loop {
            let mut page = {
                let mut conn = pool.acquire().await.map_err(|e| {
                    NsError::Db(Error::Internal(anyhow::anyhow!(
                        "the database is unavailable: {e}"
                    )))
                })?;
                rows_page(&mut conn, grant, after.as_ref().map(|k| (k, u32::MAX)), cap).await?
            };
            let full = page.len() as u64 > cap;
            if full {
                let last = page.last().map(|e| e.key.clone()).expect("full");
                if page.iter().all(|e| e.key == last) {
                    // One key alone holds more than `cap` revisions.
                    let takes = key_pattern
                        .as_deref()
                        .is_none_or(|p| wildcard_match(p, last.as_str()));
                    if takes {
                        return Ok(Err(format!(
                            "key '{last}' in namespace '{}' has more than {cap} revisions, the \
                             most one task reads for one key \
                             (workflow.storage.max_namespace_files); the keys before it were \
                             purged",
                            grant.namespace()
                        )));
                    }
                    after = Some(last);
                    continue;
                }
                // The last key may be partial: it starts the next page.
                page.retain(|e| e.key != last);
            }
            let next = page.last().map(|e| e.key.clone());
            let pattern = key_pattern.clone();
            let keys = match blocking(
                tokio::task::spawn_blocking(move || page_keys(&page, pattern.as_deref())).await,
                "the purge's key filter",
            ) {
                Ok(keys) => keys,
                Err(Blocked::Bug(why)) => return Ok(Err(why)),
                Err(Blocked::Stopped(why)) => {
                    return Err(NsError::Db(Error::Internal(anyhow::anyhow!(why))))
                }
            };
            for key in keys {
                purged += purge_key(&pool, &fs, grant, &key, cap, move |rows| {
                    judge(rows, behavior)
                })
                .await?;
            }
            match (full, next) {
                (true, Some(n)) => after = Some(n),
                _ => break,
            }
        }
    }
    Ok(Ok(purged))
}

/// `core.kv.PurgeKV`: a hard purge of expired keys, or of revisions by
/// version, in the flow's own namespace and its children (spec §8.6).
#[derive(Debug, Clone)]
pub struct PurgeKv {
    pool: PgPool,
    files: Option<Arc<FileStore>>,
}

impl PurgeKv {
    pub fn new(pool: PgPool, files: Option<Arc<FileStore>>) -> Self {
        Self { pool, files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for PurgeKv {
    fn type_id(&self) -> &'static str {
        PURGE_KV
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        const T: &str = PURGE_KV;
        let fail = |why: String| Ok(Outcome::failed_with(format!("{T}: {why}")));
        let spec: PurgeSpec = match spec(T, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let Some(fs) = &self.files else {
            return Ok(Outcome::failed_with(no_storage(T)));
        };
        let namespaces = match spec
            .namespaces
            .as_ref()
            .filter(|v| !v.is_null())
            .map(|v| ListProp::parse("namespaces", v))
        {
            None => None,
            Some(Ok(p)) => Some(p),
            Some(Err((key, why))) => return fail(format!("`{key}` {why}")),
        };
        let cx = &task.render;
        let mut render =
            |shown: &str, s: &str| render_str(s, cx).map_err(|e| format!("`{shown}`: {e}"));
        let namespaces = namespaces
            .map(|n| n.render("namespaces", &mut render))
            .transpose();
        let pattern = opt(&spec.namespace_pattern, cx, "namespacePattern");
        let key_pattern = opt(&spec.key_pattern, cx, "keyPattern");
        let children = opt(&spec.include_child_namespaces, cx, "includeChildNamespaces");
        let behavior = opt(&spec.behavior, cx, "behavior");
        outage(T, task)?;
        let (namespaces, pattern, key_pattern, children, behavior) =
            match (namespaces, pattern, key_pattern, children, behavior) {
                (Ok(n), Ok(p), Ok(k), Ok(c), Ok(b)) => (n, p, k, c.unwrap_or(true), b),
                (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), ..) => return fail(e),
                (.., Err(e), _) | (.., Err(e)) => return fail(e),
            };
        // Kestra's emptiness tests: an empty list and a blank pattern are absent.
        // Each listed namespace once, in the order written: the cap counts
        // distinct namespaces (Task 5 review M6).
        let listed = namespaces.filter(|l| !l.is_empty()).map(|l| {
            let mut seen = BTreeSet::new();
            l.into_iter()
                .filter(|ns| seen.insert(ns.clone()))
                .collect::<Vec<_>>()
        });
        let pattern = pattern.filter(|p| !p.trim().is_empty());
        let key_pattern = key_pattern.filter(|p| !p.trim().is_empty());
        if listed.is_some() && pattern.is_some() {
            // Kestra's text (`PurgeTask.findNamespaces`), naming both.
            return fail(
                "Properties `namespaces` and `namespacePattern` can't be used at the same time \
                 — use one or the other."
                    .to_string(),
            );
        }
        if let Some(l) = &listed {
            if l.len() > MAX_NAMESPACES {
                return fail(format!(
                    "`namespaces` names {} namespaces, more than the {MAX_NAMESPACES} one task \
                     may name",
                    l.len()
                ));
            }
        }
        let behavior = match behavior_rule(behavior) {
            Ok(b) => b,
            Err(why) => return fail(why),
        };
        let Some(own) = flow_namespace(task) else {
            return fail(not_a_flow_run());
        };
        // A listed namespace passes the purge rule first: one outside it is
        // refused by name before anything is listed or purged.
        for ns in listed.iter().flatten() {
            if let Err(why) = authorize(
                task.workspace_id,
                Actor::Flow { namespace: &own },
                ns,
                Resource::Kv,
                Access::Purge,
            ) {
                return fail(why);
            }
        }
        // The candidates: the namespaces holding KV that are the flow's own
        // or its children, on a dot boundary, filtered in SQL (review M7).
        let candidates: Vec<String> = if pattern.is_some() || children {
            let found = {
                let mut conn = self.pool.acquire().await.map_err(|e| db_err(T, e))?;
                namespaces_with_kv(&mut conn, task.workspace_id, &own).await
            };
            match found {
                Ok(found) => found,
                Err(e) => return split(T, e).map(Outcome::failed_with),
            }
        } else {
            Vec::new()
        };
        let resolved = {
            let own = own.clone();
            let listed = listed.clone();
            let pattern = pattern.clone();
            let joined = tokio::task::spawn_blocking(move || {
                purge_namespaces(
                    &own,
                    listed.as_deref(),
                    pattern.as_deref(),
                    children,
                    &candidates,
                )
            })
            .await;
            match blocking(joined, "the purge's namespace resolution") {
                Ok(resolved) => resolved,
                Err(Blocked::Bug(why)) => return fail(why),
                Err(Blocked::Stopped(why)) => {
                    return Err(Error::Internal(anyhow::anyhow!("{T}: {why}")))
                }
            }
        };
        if resolved.len() > MAX_NAMESPACES {
            return fail(format!(
                "{} namespaces would be purged, more than the {MAX_NAMESPACES} one task purges: \
                 list the ones to purge in `namespaces` with `includeChildNamespaces: false`, \
                 or narrow `namespacePattern`",
                resolved.len()
            ));
        }
        // THE purge rule, for EVERY namespace before anything is purged.
        let mut grants = Vec::with_capacity(resolved.len());
        for ns in &resolved {
            match authorize(
                task.workspace_id,
                Actor::Flow { namespace: &own },
                ns,
                Resource::Kv,
                Access::Purge,
            ) {
                Ok(g) => grants.push(g),
                Err(why) => return fail(why),
            }
        }
        task.log
            .line(
                LogLevel::Info,
                &format!(
                    "purging {} namespaces: [{}]",
                    resolved.len(),
                    resolved.join(", ")
                ),
            )
            .await;
        // Detached (spec §8.6): once started, the purge runs to completion
        // even if this run is dropped.
        let cap = fs.caps.max_namespace_files;
        let purged = joined(
            tokio::spawn(run_purge(
                self.pool.clone(),
                fs.clone(),
                grants,
                key_pattern.map(Arc::from),
                behavior,
                cap,
            ))
            .await,
        )?;
        let purged = match purged {
            Err(why) => return fail(why),
            Ok(Ok(Ok(n))) => n,
            Ok(Ok(Err(why))) => return fail(why),
            Ok(Err(e)) => return split(T, e).map(Outcome::failed_with),
        };
        task.log
            .line(LogLevel::Info, &format!("purged {purged} revisions"))
            .await;
        Ok(Outcome::succeeded_with(
            serde_json::json!({ "size": purged }),
        ))
    }

    /// Holds nothing past run: the purge is detached and finishes on its own.
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_are_cut_at_a_dot() {
        assert!(child_of("a.b.c", "a.b"));
        assert!(!child_of("a.b", "a.b"));
        assert!(!child_of("a.bc", "a.b"));
    }

    /// Task 5 review M1: `Get`'s walk answering a namespace it did not ask
    /// is egret's bug, a failed run (not `Err`, which re-claims for ever).
    #[test]
    fn a_walk_answering_an_unasked_namespace_is_an_egret_bug_not_retried() {
        let ws = uuid::Uuid::new_v4();
        let grants: Vec<Grant> = ["a.b", "a"]
            .iter()
            .map(|ns| authorize(ws, Actor::Api, ns, Resource::Kv, Access::Read).unwrap())
            .collect();
        assert_eq!(grant_for(&grants, "a").unwrap().namespace(), "a");
        assert_eq!(
            grant_for(&grants, "x").unwrap_err(),
            "internal error (an egret bug, not retried): egret's KV walk answered namespace \
             'x', which it did not ask"
        );
    }

    /// Task 5 review M1: a panic in the detached purge is egret's bug, a
    /// failed run; a cancelled join (the runtime going down) is the
    /// worker's `Err`, re-claimed.
    #[tokio::test]
    async fn a_panicking_purge_is_an_egret_bug_and_a_cancelled_one_the_workers() {
        let panicked = tokio::spawn(async {
            if true {
                panic!("boom");
            }
            7u64
        })
        .await;
        let why = joined(panicked).unwrap().unwrap_err();
        assert!(
            why.starts_with("internal error (an egret bug, not retried): the purge panicked:"),
            "{why}"
        );
        let pending = tokio::spawn(std::future::pending::<u64>());
        pending.abort();
        assert!(joined(pending.await).is_err());
        assert_eq!(joined(tokio::spawn(async { 7u64 }).await).unwrap(), Ok(7));
    }

    /// W9c final review F8: a blocking job's panic is egret's bug and a
    /// cancelled one the worker's; neither panics the handler (mutant: the
    /// old `resume_unwind(e.into_panic())` panics on the cancelled join).
    #[tokio::test]
    async fn a_blocking_job_that_panics_or_is_cancelled_is_told_apart() {
        let panicked = tokio::task::spawn_blocking(|| -> u64 { panic!("boom") }).await;
        match blocking(panicked, "the job") {
            Err(Blocked::Bug(why)) => assert!(
                why.starts_with("internal error (an egret bug, not retried): the job panicked"),
                "{why}"
            ),
            other => panic!("{other:?}"),
        }
        let pending = tokio::spawn(std::future::pending::<u64>());
        pending.abort();
        match blocking(pending.await, "the job") {
            Err(Blocked::Stopped(why)) => assert!(why.starts_with("the job stopped"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            blocking(Ok::<_, tokio::task::JoinError>(7), "the job"),
            Ok(7)
        );
    }

    #[test]
    fn a_pattern_is_never_expanded_to_children() {
        let candidates: Vec<String> = ["w9c.pp", "w9c.pp.a", "w9c.pp.a.b"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            purge_namespaces("w9c.pp", None, Some("w9c.pp"), true, &candidates),
            ["w9c.pp"]
        );
        assert_eq!(
            purge_namespaces("w9c.pp", None, None, true, &candidates),
            ["w9c.pp", "w9c.pp.a", "w9c.pp.a.b"]
        );
        assert_eq!(
            purge_namespaces("w9c.pp", None, None, false, &candidates),
            ["w9c.pp"]
        );
    }
}
