//! `namespaceFiles` on script tasks, and `inputFiles` values that are
//! namespace-file URIs (W9b spec §7; Task 5).
//!
//! * [`NamespaceFilesSpec::parse`] is THE parse of the block, at publish (a
//!   script task's `scripts::parse`, and `WorkingDirectory`'s
//!   `spec::validate`) and at run.
//! * [`NamespaceFilesSpec::render`] renders its strings with the task's
//!   render context; [`stage`] lists, matches and lays out what the rendered
//!   block names, as [`StoredInput`]s the runner streams from the store.
//! * [`resolve_input_uri`] is one `inputFiles` value that names a namespace
//!   file (`nsfile:///…`, `nsfile://other.ns/…`, `kestra:///…/_files/…`).
//! * [`merge`] puts the two together: namespace files first, every
//!   `inputFiles` name over them, whatever `ifExists` says (the user's ruling).
//!
//! Both read paths reach [`authorize`] (any namespace of the workspace, the
//! running flow's own namespace checked too) and then `store::list_live` or
//! `store::resolve`. Both run on a connection the caller takes from the pool,
//! outside any render: nothing here holds a render's connection.
//!
//! # Bounds (spec §7, "structural")
//!
//! The listing is capped across ALL namespaces of one block by
//! `workflow.storage.max_namespace_files`, counted on the rows egret actually
//! listed (`list_live` fetches at most the remaining count + 1; a directory's
//! row counts, as it was listed). The staged bytes, as their rows record them
//! (the transfer then refuses an object of any other size), are capped by
//! `workflow.storage.max_task_bytes` over the MERGED stored inputs
//! ([`check_task_bytes`], fix round 1 M1). The patterns are bounded by
//! `glob`'s count and one compile budget per list (fix round 1 I1). Each
//! crossing is the author's failure naming the cap.

use std::collections::BTreeMap;

use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::access::{authorize, Access, Actor};
use super::glob::PathMatcher;
use super::path::NsPath;
use super::store::{list_live, resolve, Want};
use super::NsError;
use crate::storage::FileStore;
use crate::worker::commands::StoredInput;

/// `ifExists`: Kestra's `FileExistComportment`. It governs collisions
/// between namespaces only; an `inputFiles` entry always wins (spec §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfExists {
    Overwrite,
    Fail,
    Warn,
    Ignore,
}

impl IfExists {
    const ALL: [(&'static str, Self); 4] = [
        ("OVERWRITE", Self::Overwrite),
        ("FAIL", Self::Fail),
        ("WARN", Self::Warn),
        ("IGNORE", Self::Ignore),
    ];

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().find(|(n, _)| *n == s).map(|(_, v)| *v)
    }

    pub fn as_str(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, v)| *v == self)
            .map(|(n, _)| *n)
            .expect("every variant is in ALL")
    }
}

/// `ifExists` as written (fix round 1, M4, the user's ruling of
/// 2026-10-06): Kestra's `Property<FileExistComportment>`, so a literal value
/// checked at publish, or an expression checked once rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IfExistsProp {
    Value(IfExists),
    /// A string holding `{{ }}`/`{% %}`: rendered at run, then it must be
    /// one of the four values.
    Expression(String),
}

/// `include`, `exclude` or `namespaces` as written (fix round 1, M4):
/// Kestra's `Property<List<String>>`, read as `Property.asList` reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListProp {
    /// A list (or a string holding a JSON list, as `asList` accepts); each
    /// item may hold a template, rendered on its own.
    Items(Vec<String>),
    /// A string that starts with `{{`/`{%` and ends with `}}`/`%}`
    /// (`PebbleUtil`'s test in `asList`): rendered at run, and what it
    /// renders to must be a JSON list of strings.
    Expression(String),
}

impl ListProp {
    fn empty() -> Self {
        Self::Items(Vec::new())
    }
}

/// The block as written: strings may hold templates; parse checks shape,
/// and every LITERAL value (fix round 1, M4): an expression is checked once
/// it renders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceFilesSpec {
    /// A template or `"true"`/`"false"`; absent = true when the block is present.
    pub enabled: Option<String>,
    pub include: ListProp,
    pub exclude: ListProp,
    /// `None` = the flow's namespace (Kestra's `["{{flow.namespace}}"]`).
    pub namespaces: Option<ListProp>,
    pub if_exists: IfExistsProp,
    /// A template or `"true"`/`"false"`; absent = false.
    pub folder_per_namespace: Option<String>,
}

/// The block's properties, as Kestra's `NamespaceFiles` declares them.
pub const PROPERTIES: [&str; 6] = [
    "enabled",
    "exclude",
    "folderPerNamespace",
    "ifExists",
    "include",
    "namespaces",
];

impl NamespaceFilesSpec {
    /// The ONE parse, at publish (script tasks AND `WorkingDirectory`) and at
    /// run. `Err` = (property path relative to the block, message), e.g.
    /// `("namespaces", "must be a list of strings …")`; an unknown key is
    /// refused by name. An explicit `null` is absent, as everywhere in Kestra.
    pub fn parse(v: &Value) -> Result<Self, (String, String)> {
        let Value::Object(map) = v else {
            return Err((
                String::new(),
                "namespaceFiles must be a map of enabled, include, exclude, namespaces, \
                 ifExists and folderPerNamespace"
                    .to_string(),
            ));
        };
        let mut out = Self {
            enabled: None,
            include: ListProp::empty(),
            exclude: ListProp::empty(),
            namespaces: None,
            if_exists: IfExistsProp::Value(IfExists::Overwrite),
            folder_per_namespace: None,
        };
        for (key, value) in map {
            if value.is_null() {
                continue;
            }
            match key.as_str() {
                "enabled" => out.enabled = Some(boolean(key, value)?),
                "folderPerNamespace" => out.folder_per_namespace = Some(boolean(key, value)?),
                "include" => out.include = list(key, value)?,
                "exclude" => out.exclude = list(key, value)?,
                "namespaces" => out.namespaces = Some(list(key, value)?),
                "ifExists" => out.if_exists = if_exists(key, value)?,
                other => {
                    return Err((
                        other.to_string(),
                        format!(
                            "unknown property '{other}' of namespaceFiles: it takes {}",
                            PROPERTIES.join(", ")
                        ),
                    ))
                }
            }
        }
        Ok(out)
    }

    /// Render the block with `render` (the task's render context; `what` is
    /// the property, for the caller's message). `enabled` first: a disabled
    /// block renders nothing else, as Kestra's `CommandsWrapper` reads
    /// `enabled` before it loads anything. Each expression is then checked
    /// for its type (fix round 1, M4). `namespaces` absent stays `None`:
    /// [`stage`] reads it as the running flow's own namespace, so a run with
    /// no storage is refused naming `[storage]`, not a missing namespace
    /// (fix round 1, M5).
    pub fn render(
        &self,
        mut render: impl FnMut(&str, &str) -> Result<String, String>,
    ) -> Result<RenderedNamespaceFiles, String> {
        if !render_flag(&mut render, "enabled", &self.enabled, true)? {
            return Ok(RenderedNamespaceFiles::disabled());
        }
        let folder_per_namespace = render_flag(
            &mut render,
            "folderPerNamespace",
            &self.folder_per_namespace,
            false,
        )?;
        let include = render_list(&mut render, "include", &self.include)?;
        let exclude = render_list(&mut render, "exclude", &self.exclude)?;
        let namespaces = match &self.namespaces {
            Some(list) => Some(render_list(&mut render, "namespaces", list)?),
            None => None,
        };
        let if_exists = match &self.if_exists {
            IfExistsProp::Value(v) => *v,
            IfExistsProp::Expression(e) => {
                let shown = "namespaceFiles.ifExists";
                let out = render(shown, e)?;
                IfExists::parse(out.trim()).ok_or_else(|| {
                    format!(
                        "`{shown}` must render to one of OVERWRITE, FAIL, WARN or IGNORE, and \
                         it rendered to {out:?}"
                    )
                })?
            }
        };
        Ok(RenderedNamespaceFiles {
            enabled: true,
            include,
            exclude,
            namespaces,
            if_exists,
            folder_per_namespace,
        })
    }
}

fn render_flag(
    render: &mut impl FnMut(&str, &str) -> Result<String, String>,
    what: &str,
    v: &Option<String>,
    default: bool,
) -> Result<bool, String> {
    let Some(s) = v else { return Ok(default) };
    let shown = format!("namespaceFiles.{what}");
    match render(&shown, s)?.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!(
            "`{shown}` must render to true or false, and it rendered to {other:?}"
        )),
    }
}

fn render_list(
    render: &mut impl FnMut(&str, &str) -> Result<String, String>,
    what: &str,
    v: &ListProp,
) -> Result<Vec<String>, String> {
    v.render(&format!("namespaceFiles.{what}"), render)
}

impl ListProp {
    /// Kestra's `Property.asList` at parse (see [`list`]): `key` names the
    /// property in a refusal. W9b Task 6's `UploadFiles.files` is one too.
    pub(crate) fn parse(key: &str, v: &Value) -> Result<Self, (String, String)> {
        list(key, v)
    }

    /// Each item rendered on its own, or the expression rendered and read
    /// as a JSON list of strings. `shown` is the property as a message names
    /// it (`namespaceFiles.include`, `files`); an item is `shown[i]`.
    pub(crate) fn render(
        &self,
        shown: &str,
        render: &mut impl FnMut(&str, &str) -> Result<String, String>,
    ) -> Result<Vec<String>, String> {
        render_list_prop(render, shown, self)
    }
}

fn render_list_prop(
    render: &mut impl FnMut(&str, &str) -> Result<String, String>,
    shown: &str,
    v: &ListProp,
) -> Result<Vec<String>, String> {
    match v {
        ListProp::Items(items) => items
            .iter()
            .enumerate()
            .map(|(i, s)| render(&format!("{shown}[{i}]"), s))
            .collect(),
        ListProp::Expression(e) => {
            let out = render(shown, e)?;
            serde_json::from_str::<Vec<String>>(&out).map_err(|_| {
                format!(
                    "`{shown}` must render to a list of strings (a JSON list such as \
                     [\"a\", \"b\"]), and it rendered to {out:?}"
                )
            })
        }
    }
}

/// `enabled`/`folderPerNamespace`: a boolean, or a string (a template, or
/// `true`/`false` written as one -- a Kestra `Property<Boolean>`).
fn boolean(key: &str, v: &Value) -> Result<String, (String, String)> {
    match v {
        Value::Bool(b) => Ok(b.to_string()),
        Value::String(s) => Ok(s.clone()),
        _ => Err((
            key.to_string(),
            format!("must be true, false or an expression, not {v}"),
        )),
    }
}

/// Kestra's `Property.asList`, at publish: a string starting with `{{`/`{%`
/// and ending with `}}`/`%}` is an expression, checked once rendered; any
/// other string must hold a JSON list of strings; a list's items must be
/// strings (each may hold a template).
fn list(key: &str, v: &Value) -> Result<ListProp, (String, String)> {
    let must = || {
        (
            key.to_string(),
            "must be a list of strings, or an expression that renders to one".to_string(),
        )
    };
    let items = match v {
        Value::String(s) => {
            let t = s.trim();
            if (t.starts_with("{{") || t.starts_with("{%"))
                && (t.ends_with("}}") || t.ends_with("%}"))
            {
                return Ok(ListProp::Expression(s.clone()));
            }
            match serde_json::from_str::<Value>(t) {
                Ok(Value::Array(items)) => items,
                _ => return Err(must()),
            }
        }
        Value::Array(items) => items.clone(),
        _ => return Err(must()),
    };
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                (
                    format!("{key}[{i}]"),
                    format!("must be a string, not {item}"),
                )
            })
        })
        .collect::<Result<_, _>>()
        .map(ListProp::Items)
}

/// Kestra's `Property<FileExistComportment>`, at publish: one of the four
/// values, upper-case as Kestra's enum; or an expression, checked once
/// rendered.
fn if_exists(key: &str, v: &Value) -> Result<IfExistsProp, (String, String)> {
    if let Some(s) = v.as_str() {
        if let Some(value) = IfExists::parse(s) {
            return Ok(IfExistsProp::Value(value));
        }
        if crate::scripts::gate::is_unrendered(s) {
            return Ok(IfExistsProp::Expression(s.to_string()));
        }
    }
    Err((
        key.to_string(),
        format!("must be one of OVERWRITE, FAIL, WARN or IGNORE, or an expression, not {v}"),
    ))
}

/// The block after rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedNamespaceFiles {
    pub enabled: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    /// `None`: the running flow's own namespace, which [`stage`] is given.
    pub namespaces: Option<Vec<String>>,
    pub if_exists: IfExists,
    pub folder_per_namespace: bool,
}

impl RenderedNamespaceFiles {
    fn disabled() -> Self {
        Self {
            enabled: false,
            include: Vec::new(),
            exclude: Vec::new(),
            namespaces: Some(Vec::new()),
            if_exists: IfExists::Overwrite,
            folder_per_namespace: false,
        }
    }
}

/// What [`stage`] found: each file under its path in the working directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Staged {
    pub files: BTreeMap<String, StoredInput>,
    /// The namespace each of `files` came from, by the same path.
    pub sources: BTreeMap<String, String>,
    /// One line per `WARN` collision, naming the path; the handler logs them.
    pub warnings: Vec<String>,
}

/// An infrastructure failure passes through; anything else is the author's.
fn split(e: NsError) -> Result<String, NsError> {
    match e {
        NsError::Store(_) | NsError::Db(_) => Err(e),
        other => Ok(other.to_string()),
    }
}

/// Spec §7: for each namespace IN LIST ORDER: [`authorize`] (Read) ->
/// `list_live` (recursive, capped by what remains of
/// `max_namespace_files` across all namespaces) -> [`PathMatcher`] -> the
/// path without its leading '/' (or `{namespace}/{path}` with
/// `folderPerNamespace`, w9b `staging.folder_per_namespace`). A path already
/// staged from an earlier namespace: `Overwrite` replaces it; `Fail` is an
/// error naming both namespaces and the path; `Warn` keeps the first and
/// pushes one line naming the path; `Ignore` keeps the first.
///
/// `Ok(Err(_))` is the author's failure; `Err` is `NsError::{Store, Db}`,
/// infrastructure.
pub async fn stage(
    pool: &sqlx::PgPool,
    fs: &FileStore,
    workspace: Uuid,
    flow_namespace: &str,
    spec: &RenderedNamespaceFiles,
) -> Result<Result<Staged, String>, NsError> {
    let mut staged = Staged::default();
    if !spec.enabled {
        return Ok(Ok(staged));
    }
    let own = [flow_namespace.to_string()];
    let namespaces = spec.namespaces.as_deref().unwrap_or(&own);
    // Fix round 2, M-R3: one `authorize` + one listing per namespace, and an
    // expression can now produce the list, so its length is bounded too --
    // after render, before any query.
    if namespaces.len() > MAX_NAMESPACES {
        return Ok(Err(format!(
            "namespaceFiles.namespaces: {} namespaces, more than the {MAX_NAMESPACES} one \
             task may stage from",
            namespaces.len()
        )));
    }
    // Compiling the patterns is CPU work an author sizes (bounded by
    // `glob`'s count and budget): off the async runtime (fix round 1, I1).
    let (include, exclude) = (spec.include.clone(), spec.exclude.clone());
    let compiled = tokio::task::spawn_blocking(move || PathMatcher::new(&include, &exclude))
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
    let matcher = match compiled {
        Ok(m) => std::sync::Arc::new(m),
        Err(why) => return Ok(Err(format!("namespaceFiles: {why}"))),
    };
    let cap = fs.caps.max_namespace_files;
    let mut listed: u64 = 0;
    // Which namespace each staged path came from, for FAIL's message.
    let mut source: BTreeMap<String, &str> = BTreeMap::new();
    // Fix round 2 addendum (the user's ruling of 2026-10-06): list every
    // namespace first, on one connection, and RETURN it before any matching:
    // matching runs on the blocking pool for as long as the patterns cost
    // (spec §15) and holds no database connection meanwhile.
    let mut listings = Vec::with_capacity(namespaces.len());
    {
        let mut conn = pool.acquire().await.map_err(|e| {
            NsError::Db(egret_core::Error::Internal(anyhow::anyhow!(
                "the database is unavailable: {e}"
            )))
        })?;
        for (i, ns) in namespaces.iter().enumerate() {
            let grant = match authorize(
                workspace,
                Actor::Flow {
                    namespace: flow_namespace,
                },
                ns,
                Access::Read,
            ) {
                Ok(g) => g,
                Err(why) => return Ok(Err(format!("namespaceFiles.namespaces[{i}]: {why}"))),
            };
            let remaining = cap.saturating_sub(listed);
            let entries = match list_live(&mut conn, &grant, None, true, remaining).await {
                Ok(entries) => entries,
                Err(NsError::TooLarge(_)) => {
                    return Ok(Err(format!(
                        "namespaceFiles: namespaces {} hold more than {cap} namespace files \
                         together, the most one task stages (workflow.storage.max_namespace_files)",
                        namespaces[..=i]
                            .iter()
                            .map(|n| format!("'{n}'"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )))
                }
                Err(e) => return split(e).map(Err),
            };
            listed += entries.len() as u64;
            listings.push((ns, grant, entries));
        }
        drop(conn);
    }
    for (ns, grant, entries) in listings {
        for e in matching(&matcher, entries).await {
            let rel_path = e.path.as_str().trim_start_matches('/');
            let rel = if spec.folder_per_namespace {
                format!("{ns}/{rel_path}")
            } else {
                rel_path.to_string()
            };
            let uri = match crate::storage::uri::namespace_file_uri(ns, &e.path, e.revision) {
                Ok(u) => u,
                Err(why) => return Ok(Err(format!("namespaceFiles: '{}': {why}", e.path))),
            };
            let input = StoredInput {
                uri,
                object_uri: fs.namespace_object_uri(&grant, &e.path, e.revision),
                size: e.size,
            };
            match (staged.files.contains_key(&rel), spec.if_exists) {
                (false, _) | (true, IfExists::Overwrite) => {
                    staged.files.insert(rel.clone(), input);
                    source.insert(rel, ns);
                }
                (true, IfExists::Fail) => {
                    return Ok(Err(format!(
                        "namespaceFiles: '{rel}' is in namespace '{}' and in namespace '{ns}', \
                         and ifExists is FAIL",
                        source.get(&rel).copied().unwrap_or_default()
                    )))
                }
                (true, IfExists::Warn) => staged.warnings.push(format!(
                    "namespaceFiles: '{rel}' from namespace '{ns}' was not staged: the file \
                     of that name from namespace '{}' was kept (ifExists: WARN)",
                    source.get(&rel).copied().unwrap_or_default()
                )),
                (true, IfExists::Ignore) => {}
            }
        }
    }
    staged.sources = source
        .into_iter()
        .map(|(rel, ns)| (rel, ns.to_string()))
        .collect();
    Ok(Ok(staged))
}

/// The most namespaces one rendered `namespaces` may name (fix round 2,
/// M-R3; egret's bound, Kestra has none). A refusal names it and the count.
pub const MAX_NAMESPACES: usize = 64;

/// The listed files the predicate accepts, in listing order. Directories are
/// never offered to it (w9b `globs.directories`) and have no object to stage.
///
/// Fix round 2, I1-R: matching is linear but its constant is the compiled
/// size an author chose (spec §15 records the measured worst case), so it
/// runs on the blocking pool, never on an async worker that also drives the
/// lease heartbeats and other runs.
async fn matching(
    matcher: &std::sync::Arc<PathMatcher>,
    entries: Vec<super::store::NsEntry>,
) -> Vec<super::store::NsEntry> {
    let matcher = matcher.clone();
    tokio::task::spawn_blocking(move || {
        entries
            .into_iter()
            .filter(|e| !e.is_directory && matcher.matches(&e.path))
            .collect()
    })
    .await
    .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
}

/// Spec §7, "staged bytes count against `max_task_bytes`" (fix round 1, M1):
/// summed over what the runner will stream -- the stored inputs AFTER
/// [`merge`], so a namespace file an `inputFiles` name replaced does not
/// count, and an `inputFiles` namespace-file URI does. Each size is its
/// row's; the transfer refuses an object of any other size. The plugin
/// bridge calls it too (W9b Task 10 fix round 1, M1), over its execution
/// files and namespace files: one cap, one set, for both handlers.
pub fn check_task_bytes(stored: &BTreeMap<String, StoredInput>, max: u64) -> Result<(), String> {
    let bytes: u64 = stored.values().map(|f| f.size).sum();
    if bytes > max {
        return Err(format!(
            "the input files staged from storage (namespace files and execution files) hold \
             {bytes} bytes, more than the {max} one task may stage \
             (workflow.storage.max_task_bytes)"
        ));
    }
    Ok(())
}

/// One `inputFiles` value that names a namespace file (`nsfile:///…`,
/// `nsfile://other.ns/…`, `kestra:///…/_files/…[.vN]`): the shared parse
/// (`storage::scope::ns_ref`, the functions' own) -> [`authorize`] (Read)
/// -> `store::resolve` (the latest live revision, or the one revision a
/// `kestra://` URI names, R1) -> a [`StoredInput`]. `Ok(Err(_))` is the
/// author's, naming the path; `Err` is `NsError::{Store, Db}`.
pub async fn resolve_input_uri(
    conn: &mut PgConnection,
    fs: &FileStore,
    workspace: Uuid,
    flow_namespace: &str,
    uri: &str,
) -> Result<Result<StoredInput, String>, NsError> {
    Ok(resolve_ns_file(conn, fs, workspace, flow_namespace, uri)
        .await?
        .map(|r| r.input))
}

/// What [`resolve_ns_file`] found: the [`StoredInput`] the runner streams,
/// and which file and revision it is (the plugin bridge's `namespaceFiles`
/// entry, W9b Task 10).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedNsFile {
    pub input: StoredInput,
    /// The namespace the grant was given for.
    pub namespace: String,
    /// The path in its stored form, with its leading '/'.
    pub path: String,
    pub revision: u32,
}

/// [`resolve_input_uri`], keeping the namespace, path and revision it
/// resolved: the one resolver of a namespace-file URI, shared by a script
/// task's `inputFiles` (Task 5) and the plugin bridge (Task 10).
pub async fn resolve_ns_file(
    conn: &mut PgConnection,
    fs: &FileStore,
    workspace: Uuid,
    flow_namespace: &str,
    uri: &str,
) -> Result<Result<ResolvedNsFile, String>, NsError> {
    let named = match crate::storage::scope::ns_ref(uri, flow_namespace, None) {
        Ok(n) => n,
        Err(why) => return Ok(Err(why)),
    };
    let grant = match authorize(
        workspace,
        Actor::Flow {
            namespace: flow_namespace,
        },
        &named.namespace,
        Access::Read,
    ) {
        Ok(g) => g,
        Err(why) => return Ok(Err(why)),
    };
    let path = match NsPath::parse_file(&named.path) {
        Ok(p) => p,
        Err(why) => return Ok(Err(why)),
    };
    let want = named.uri_revision.map_or(Want::Latest, Want::Revision);
    let entry = match resolve(conn, &grant, &path, want).await {
        Ok(e) => e,
        Err(e) => return split(e).map(Err),
    };
    if entry.is_directory {
        return Ok(Err(format!(
            "'{}' is a directory in namespace '{}': it has no contents to stage",
            entry.path.lock_path(),
            grant.namespace()
        )));
    }
    Ok(Ok(ResolvedNsFile {
        input: StoredInput {
            uri: uri.to_string(),
            object_uri: fs.namespace_object_uri(&grant, &entry.path, entry.revision),
            size: entry.size,
        },
        namespace: grant.namespace().to_string(),
        path: entry.path.as_str().to_string(),
        revision: entry.revision,
    }))
}

/// Spec §7, the user's ruling: namespace files first, then every name
/// `inputFiles` declares (inline `contents` or `stored`) over them, whatever
/// `ifExists` said -- `ifExists` decided collisions between namespaces in
/// [`stage`], and only those. Returns the stored inputs the runner stages
/// beside `contents`.
pub fn merge(
    namespace_files: BTreeMap<String, StoredInput>,
    contents: &BTreeMap<String, String>,
    stored: BTreeMap<String, StoredInput>,
) -> BTreeMap<String, StoredInput> {
    let mut out = namespace_files;
    out.retain(|name, _| !contents.contains_key(name));
    out.extend(stored);
    out
}

/// Shared by this module's Postgres tests and `handlers::script`'s: a
/// migrated test database (`egret-test-db`'s per-process one, never the dev
/// database), a fresh workspace per test, and a `MemStore`-backed
/// [`FileStore`] -- every object here lives in memory.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Arc;

    use sqlx::PgPool;
    use uuid::Uuid;

    use crate::storage::nsfiles::access::{authorize, Access, Actor};
    use crate::storage::nsfiles::path::NsPath;
    use crate::storage::nsfiles::store::{put, soft_delete, Conflict, Written};
    use crate::storage::nsfiles::CreatedBy;
    use crate::storage::{Caps, FileStore};

    /// `None` only when `egret-test-db` allows the skip (no `DATABASE_URL`
    /// and `EGRET_ALLOW_DB_SKIP`); any other absence panics in that crate.
    pub async fn pool() -> Option<PgPool> {
        let url = egret_test_db::url().await?;
        let db = egret_metastore::Db::connect(&url, 5)
            .await
            .expect("connect");
        db.migrate().await.expect("migrate");
        Some(db.pool)
    }

    pub async fn workspace(pool: &PgPool) -> Uuid {
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (email, name, provider, status, password_hash) \
             VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
        )
        .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(pool)
        .await
        .expect("seed user");
        let ws = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 't', $3)")
            .bind(ws)
            .bind(ws.to_string())
            .bind(user)
            .execute(pool)
            .await
            .expect("seed workspace");
        ws
    }

    pub fn mem_files(caps: Caps) -> Arc<FileStore> {
        Arc::new(FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps,
            retention_days: 30,
        })
    }

    /// Write `body` at `path` of `ns` (the next revision); its revision.
    pub async fn write(
        pool: &PgPool,
        fs: &FileStore,
        ws: Uuid,
        ns: &str,
        path: &str,
        body: &str,
    ) -> u32 {
        let grant = authorize(ws, Actor::Api, ns, Access::Write).expect("a writable namespace");
        let path = NsPath::parse_file(path).expect("a file path");
        let mut reader = body.as_bytes();
        match put(
            pool,
            fs,
            &grant,
            &path,
            &mut reader,
            Conflict::Overwrite,
            CreatedBy::User(Uuid::nil()),
        )
        .await
        .expect("written")
        {
            Written::Written(e) => e.revision,
            Written::Skipped(p) => panic!("{p} skipped"),
            Written::Already(e) => panic!("{} already written", e.path),
        }
    }

    pub async fn delete(pool: &PgPool, ws: Uuid, ns: &str, path: &str) {
        let grant = authorize(ws, Actor::Api, ns, Access::Write).expect("a writable namespace");
        soft_delete(pool, &grant, &NsPath::parse_file(path).expect("a path"))
            .await
            .expect("soft-deleted");
    }

    /// The bytes behind a staged input, as UTF-8.
    pub async fn contents(fs: &FileStore, object_uri: &str) -> String {
        String::from_utf8(fs.store.get(object_uri).await.expect("the object")).expect("utf-8")
    }

    /// w9b `staging.setup`: `w9b.s1`, `w9b.s2` and the flow's own `w9b.stage`.
    pub async fn setup(pool: &PgPool, fs: &FileStore, ws: Uuid) {
        let doc = super::tests::fixture();
        let setup = doc["egret_expected"]["staging"]["setup"]
            .as_object()
            .expect("staging.setup");
        for (ns, files) in setup {
            let ns = ns.split(' ').next().expect("a namespace");
            for (path, body) in files.as_object().expect("files") {
                write(pool, fs, ws, ns, path, body.as_str().expect("a body")).await;
            }
        }
    }

    /// The flow's own namespace in w9b `staging.setup`.
    pub const OWN: &str = "w9b.stage";
}

#[cfg(test)]
pub(crate) mod tests {
    use super::testing::{self, contents, mem_files, write, OWN};
    use super::*;
    use crate::storage::Caps;
    use serde_json::json;

    pub(crate) fn fixture() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("oracle-exec/w9b-namespace-files.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("read the w9b fixture"))
            .expect("parse the w9b fixture")
    }

    fn rendered(namespaces: &[&str], if_exists: IfExists) -> RenderedNamespaceFiles {
        RenderedNamespaceFiles {
            enabled: true,
            include: Vec::new(),
            exclude: Vec::new(),
            namespaces: Some(namespaces.iter().map(|s| s.to_string()).collect()),
            if_exists,
            folder_per_namespace: false,
        }
    }

    /// Each staged file's contents, by its path in the working directory.
    async fn laid_out(fs: &FileStore, staged: &Staged) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (rel, input) in &staged.files {
            out.insert(rel.clone(), contents(fs, &input.object_uri).await);
        }
        out
    }

    /// A measured run's `files` (`./x.txt` -> `x.txt`).
    fn measured(v: &Value) -> BTreeMap<String, String> {
        v.as_object()
            .expect("files")
            .iter()
            .map(|(k, v)| {
                (
                    k.trim_start_matches("./").to_string(),
                    v.as_str().expect("contents").to_string(),
                )
            })
            .collect()
    }

    async fn run_stage(
        pool: &sqlx::PgPool,
        fs: &FileStore,
        ws: Uuid,
        spec: &RenderedNamespaceFiles,
    ) -> Result<Staged, String> {
        stage(pool, fs, ws, OWN, spec)
            .await
            .expect("no infrastructure failure")
    }

    /// w9b `staging.two_namespaces_same_path` and `staging.egret` (chosen,
    /// agreeing with 5/5 measured runs per mode): namespaces in list order, a
    /// later one wins under OVERWRITE, WARN and IGNORE keep the first, FAIL
    /// fails naming both namespaces and the path.
    #[tokio::test]
    async fn a_later_namespace_wins_under_overwrite_and_each_if_exists_does_what_the_artifact_does()
    {
        let Some(pool) = testing::pool().await else {
            return;
        };
        let fs = mem_files(Caps::default());
        let ws = testing::workspace(&pool).await;
        testing::setup(&pool, &fs, ws).await;
        let doc = fixture();
        let staging = &doc["egret_expected"]["staging"];
        assert_eq!(staging["egret"]["basis"], "chosen");
        for (mode, if_exists) in [
            ("OVERWRITE", IfExists::Overwrite),
            ("WARN", IfExists::Warn),
            ("IGNORE", IfExists::Ignore),
        ] {
            let staged = run_stage(&pool, &fs, ws, &rendered(&["w9b.s1", "w9b.s2"], if_exists))
                .await
                .unwrap_or_else(|e| panic!("{mode}: {e}"));
            let files = laid_out(&fs, &staged).await;
            // egret's answer (top level) for the colliding path ...
            assert_eq!(
                files["x.txt"],
                staging["egret"][mode]["x.txt"].as_str().unwrap(),
                "{mode}"
            );
            // ... and every measured run's whole working directory.
            for run in staging["two_namespaces_same_path"][mode]["results"]
                .as_array()
                .unwrap()
            {
                assert_eq!(files, measured(&run["files"]), "{mode}");
            }
            let warned = staged.warnings.len();
            if if_exists == IfExists::Warn {
                assert_eq!(warned, 1, "{:?}", staged.warnings);
                assert!(
                    staged.warnings[0].contains("'x.txt'")
                        && staged.warnings[0].contains("w9b.s2")
                        && staged.warnings[0].contains("w9b.s1"),
                    "{:?}",
                    staged.warnings
                );
            } else {
                assert_eq!(warned, 0, "{mode}: {:?}", staged.warnings);
            }
        }
        let err = run_stage(
            &pool,
            &fs,
            ws,
            &rendered(&["w9b.s1", "w9b.s2"], IfExists::Fail),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("'x.txt'") && err.contains("'w9b.s1'") && err.contains("'w9b.s2'"),
            "{err}"
        );
        assert_eq!(
            staging["two_namespaces_same_path"]["FAIL"]["results"][0]["state"], "WARNING",
            "Kestra's FAIL failed the task (inside its AllowFailure)"
        );
        // The order is the list's: reversed, the other namespace wins.
        let staged = run_stage(
            &pool,
            &fs,
            ws,
            &rendered(&["w9b.s2", "w9b.s1"], IfExists::Overwrite),
        )
        .await
        .unwrap();
        assert_eq!(laid_out(&fs, &staged).await["x.txt"], "from-s1");
    }

    /// w9b `staging.folder_per_namespace`, `include_exclude`,
    /// `default_namespaces` and `enabled_false`, as measured.
    #[tokio::test]
    async fn folder_per_namespace_lays_out_as_the_artifact() {
        let Some(pool) = testing::pool().await else {
            return;
        };
        let fs = mem_files(Caps::default());
        let ws = testing::workspace(&pool).await;
        testing::setup(&pool, &fs, ws).await;
        let doc = fixture();
        let staging = &doc["egret_expected"]["staging"];

        let mut spec = rendered(&["w9b.s1", "w9b.s2"], IfExists::Overwrite);
        spec.folder_per_namespace = true;
        let staged = run_stage(&pool, &fs, ws, &spec).await.unwrap();
        assert_eq!(
            laid_out(&fs, &staged).await,
            measured(&staging["folder_per_namespace"]["files"])
        );

        let ie = &staging["include_exclude"];
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        };
        let mut spec = rendered(&[], IfExists::Overwrite);
        spec.namespaces = Some(strings(&ie["namespaces"]));
        spec.include = strings(&ie["include"]);
        spec.exclude = strings(&ie["exclude"]);
        let staged = run_stage(&pool, &fs, ws, &spec).await.unwrap();
        assert_eq!(laid_out(&fs, &staged).await, measured(&ie["files"]));

        // No `namespaces`: the flow's own (`render`'s default).
        let spec = NamespaceFilesSpec::parse(&json!({}))
            .unwrap()
            .render(|_, s| Ok(s.to_string()))
            .unwrap();
        let staged = run_stage(&pool, &fs, ws, &spec).await.unwrap();
        assert_eq!(
            laid_out(&fs, &staged).await,
            measured(&staging["default_namespaces"]["files"])
        );

        let spec = NamespaceFilesSpec::parse(&json!({"enabled": false}))
            .unwrap()
            .render(|_, s| Ok(s.to_string()))
            .unwrap();
        let staged = run_stage(&pool, &fs, ws, &spec).await.unwrap();
        assert_eq!(
            laid_out(&fs, &staged).await,
            measured(&staging["enabled_false"]["files"])
        );
    }

    /// The cap is ACROSS the block's namespaces: 2 + 2 files under a cap of
    /// 3 is refused naming it, though each namespace alone is under it; at a
    /// cap of 4 the same block stages. (The staged bytes are capped after the
    /// merge: `check_task_bytes`, tested below and in `handlers::script`.)
    #[tokio::test]
    async fn one_file_past_max_namespace_files_across_namespaces_is_refused_naming_the_cap() {
        let Some(pool) = testing::pool().await else {
            return;
        };
        let ws = testing::workspace(&pool).await;
        let caps = |files: u64, bytes: u64| Caps {
            max_namespace_files: files,
            max_task_bytes: bytes,
            ..Caps::default()
        };
        let fs = mem_files(caps(3, 1 << 30));
        for (ns, path) in [
            ("w9b.c1", "/a"),
            ("w9b.c1", "/b"),
            ("w9b.c2", "/c"),
            ("w9b.c2", "/d"),
        ] {
            write(&pool, &fs, ws, ns, path, "12345").await;
        }
        let spec = rendered(&["w9b.c1", "w9b.c2"], IfExists::Overwrite);
        let err = run_stage(&pool, &fs, ws, &spec).await.unwrap_err();
        assert!(
            err.contains("more than 3")
                && err.contains("workflow.storage.max_namespace_files")
                && err.contains("'w9b.c1', 'w9b.c2'"),
            "{err}"
        );
        // Each namespace alone is under the cap.
        for ns in ["w9b.c1", "w9b.c2"] {
            let one = run_stage(&pool, &fs, ws, &rendered(&[ns], IfExists::Overwrite))
                .await
                .unwrap();
            assert_eq!(one.files.len(), 2);
        }
        // The control: at 4, the same block stages all four.
        let at = FileStore {
            caps: caps(4, 1 << 30),
            ..(*fs).clone()
        };
        let staged = run_stage(&pool, &at, ws, &spec).await.unwrap();
        assert_eq!(staged.files.len(), 4);
    }

    /// A `_files` segment, or an empty one, in `namespaces` is refused by
    /// `authorize` (through `check_namespace`), naming the namespace -- and
    /// nothing of the namespaces before it is returned.
    #[tokio::test]
    async fn a_files_segment_namespace_in_namespaces_is_refused_by_name() {
        let Some(pool) = testing::pool().await else {
            return;
        };
        let fs = mem_files(Caps::default());
        let ws = testing::workspace(&pool).await;
        testing::setup(&pool, &fs, ws).await;
        for (bad, rule) in [("a._files", "'_files'"), ("w9b..s1", "empty segment")] {
            let err = run_stage(
                &pool,
                &fs,
                ws,
                &rendered(&["w9b.s1", bad], IfExists::Overwrite),
            )
            .await
            .unwrap_err();
            assert!(
                err.contains(&format!("'{bad}'"))
                    && err.contains(rule)
                    && err.contains("namespaceFiles.namespaces[1]"),
                "{err}"
            );
        }
        // The control: the good namespace alone stages.
        let ok = run_stage(&pool, &fs, ws, &rendered(&["w9b.s1"], IfExists::Overwrite))
            .await
            .unwrap();
        assert_eq!(ok.files.len(), 2);
    }

    /// Fix round 1, M2: the RUNNING flow's own namespace is checked too, on
    /// both read paths. A flow in `a._files` or `w9b.bad.` (published before
    /// the rule; publish still accepts an empty segment) stages nothing from
    /// a good namespace, and the refusal names the running flow's namespace.
    #[tokio::test]
    async fn a_running_flow_whose_own_namespace_is_refused_stages_nothing() {
        let Some(pool) = testing::pool().await else {
            return;
        };
        let fs = mem_files(Caps::default());
        let ws = testing::workspace(&pool).await;
        testing::setup(&pool, &fs, ws).await;
        let mut conn = pool.acquire().await.expect("a connection");
        for (own, rule) in [("a._files", "'_files'"), ("w9b.bad.", "empty segment")] {
            let spec = rendered(&["w9b.s1"], IfExists::Overwrite);
            let err = stage(&pool, &fs, ws, own, &spec)
                .await
                .expect("no infrastructure failure")
                .unwrap_err();
            assert!(
                err.contains("the running flow's")
                    && err.contains(&format!("'{own}'"))
                    && err.contains(rule),
                "stage, {own}: {err}"
            );
            let err = resolve_input_uri(&mut conn, &fs, ws, own, "nsfile://w9b.s1/x.txt")
                .await
                .expect("no infrastructure failure")
                .unwrap_err();
            assert!(
                err.contains("the running flow's") && err.contains(&format!("'{own}'")),
                "resolve_input_uri, {own}: {err}"
            );
        }
        // The control: a good running namespace reads the same file.
        let spec = rendered(&["w9b.s1"], IfExists::Overwrite);
        let ok = stage(&pool, &fs, ws, OWN, &spec).await.unwrap().unwrap();
        assert_eq!(ok.files.len(), 2);
        let input = resolve_input_uri(&mut conn, &fs, ws, OWN, "nsfile://w9b.s1/x.txt")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(contents(&fs, &input.object_uri).await, "from-s1");
    }

    /// Fix round 2, I1-R: the predicate runs on the blocking pool, not on
    /// the async worker. On a CURRENT-THREAD runtime (one worker for every
    /// task), a ticker spawned beside `stage` keeps ticking while a costly
    /// match runs; were the match inline, the ticker could only run during
    /// the listing's own awaits.
    #[tokio::test(flavor = "current_thread")]
    async fn matching_runs_off_the_async_runtime_and_a_ticker_keeps_ticking() {
        let Some(pool) = testing::pool().await else {
            return;
        };
        let fs = mem_files(Caps::default());
        let ws = testing::workspace(&pool).await;
        // Long paths over {a, b}, none of which matches.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut segment = || {
            (0..200)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    if x.is_multiple_of(2) {
                        'a'
                    } else {
                        'b'
                    }
                })
                .collect::<String>()
        };
        for _ in 0..FILES {
            let path = format!("/{}/{}", segment(), segment());
            write(&pool, &fs, ws, "w9b.heavy", &path, "x").await;
        }
        // The review's family: each pattern makes the lazy DFA give up.
        let mut spec = rendered(&["w9b.heavy"], IfExists::Overwrite);
        spec.include = (0..PATTERNS)
            .map(|i| format!("regex:.*[ab]a[ab]{{{}}}c{i}", i % 40))
            .collect();

        // A pool of ONE connection: while `stage` matches, the ticker can
        // take it only if `stage` returned it after the listing (fix round 2
        // addendum).
        let one = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with((*pool.connect_options()).clone())
            .await
            .expect("a one-connection pool");
        let ticks = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let free = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let ticker = {
            let (ticks, free, one) = (ticks.clone(), free.clone(), one.clone());
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    ticks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if one.try_acquire().is_some() {
                        free.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            })
        };
        tokio::task::yield_now().await;
        let started = std::time::Instant::now();
        let before = ticks.load(std::sync::atomic::Ordering::Relaxed);
        let free_before = free.load(std::sync::atomic::Ordering::Relaxed);
        let staged = run_stage(&one, &fs, ws, &spec).await.unwrap();
        let took = started.elapsed();
        let ticked = ticks.load(std::sync::atomic::Ordering::Relaxed) - before;
        let freed = free.load(std::sync::atomic::Ordering::Relaxed) - free_before;
        ticker.abort();
        assert!(staged.files.is_empty());
        // The match must dominate, or the test proves nothing.
        assert!(
            took >= std::time::Duration::from_millis(400),
            "the match took only {took:?}: raise FILES or PATTERNS"
        );
        // A 2 ms ticker on a free runtime ticks at least once per 10 ms.
        let floor = took.as_millis() as u64 / 10;
        assert!(
            ticked >= floor,
            "{ticked} ticks in {took:?} (at least {floor} expected): the match ran on \
             the async worker"
        );
        // The connection was back in the pool for most of the match.
        assert!(
            freed >= floor / 2,
            "the one connection was free on {freed} of {ticked} ticks in {took:?} (at \
             least {} expected): stage held it while matching",
            floor / 2
        );
    }
    const FILES: usize = 40;
    const PATTERNS: usize = 64;

    /// Fix round 2, M-R3: at most [`MAX_NAMESPACES`] namespaces, checked
    /// after render and before any query (a refused namespace past the cap
    /// is not reached: the cap answers first).
    #[tokio::test]
    async fn sixty_four_namespaces_stage_and_sixty_five_are_refused_naming_the_cap() {
        let Some(pool) = testing::pool().await else {
            return;
        };
        let fs = mem_files(Caps::default());
        let ws = testing::workspace(&pool).await;
        let names: Vec<String> = (0..MAX_NAMESPACES).map(|i| format!("w9b.n{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let ok = run_stage(&pool, &fs, ws, &rendered(&refs, IfExists::Overwrite)).await;
        assert_eq!(ok, Ok(Staged::default()));
        let mut over = refs.clone();
        over.push("a._files");
        let err = run_stage(&pool, &fs, ws, &rendered(&over, IfExists::Overwrite))
            .await
            .unwrap_err();
        assert!(
            err.contains("65 namespaces") && err.contains("more than the 64"),
            "{err}"
        );
        assert!(!err.contains("_files"), "the cap answers first: {err}");
    }

    /// The workspace is the run's: another workspace's identical namespace
    /// stages nothing (and says nothing about what it holds).
    #[tokio::test]
    async fn another_workspaces_namespace_stages_nothing() {
        let Some(pool) = testing::pool().await else {
            return;
        };
        let fs = mem_files(Caps::default());
        let (mine, theirs) = (
            testing::workspace(&pool).await,
            testing::workspace(&pool).await,
        );
        testing::setup(&pool, &fs, theirs).await;
        let staged = run_stage(
            &pool,
            &fs,
            mine,
            &rendered(&["w9b.s1", "w9b.s2"], IfExists::Fail),
        )
        .await
        .unwrap();
        assert_eq!(staged, Staged::default());
        // The control: theirs stages from the same rows.
        let staged = run_stage(
            &pool,
            &fs,
            theirs,
            &rendered(&["w9b.s1"], IfExists::Overwrite),
        )
        .await
        .unwrap();
        assert_eq!(staged.files.len(), 2);
    }

    /// A soft-deleted path is not staged (staging reads the latest live
    /// revision), a rewritten one stages its latest revision, and directories
    /// are never staged.
    #[tokio::test]
    async fn only_latest_live_files_are_staged() {
        let Some(pool) = testing::pool().await else {
            return;
        };
        let fs = mem_files(Caps::default());
        let ws = testing::workspace(&pool).await;
        write(&pool, &fs, ws, "w9b.l", "/d/kept.txt", "v1").await;
        assert_eq!(write(&pool, &fs, ws, "w9b.l", "/d/kept.txt", "v2").await, 2);
        write(&pool, &fs, ws, "w9b.l", "/gone.txt", "x").await;
        testing::delete(&pool, ws, "w9b.l", "/gone.txt").await;
        let staged = run_stage(&pool, &fs, ws, &rendered(&["w9b.l"], IfExists::Fail))
            .await
            .unwrap();
        assert_eq!(
            laid_out(&fs, &staged).await,
            BTreeMap::from([("d/kept.txt".to_string(), "v2".to_string())])
        );
        assert_eq!(
            staged.files["d/kept.txt"].uri,
            "kestra:///w9b/l/_files/d/kept.txt.v2"
        );
    }

    fn spec(v: Value) -> NamespaceFilesSpec {
        NamespaceFilesSpec::parse(&v).expect("a valid block")
    }

    /// The two corpus flows' blocks (`WorkingDirectory`), and templates.
    #[test]
    fn parse_accepts_the_two_corpus_blocks_and_templates() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle-exec/corpus/valids");
        let mut seen = 0;
        for name in [
            "working-directory-namespace-files.yaml",
            "working-directory-namespace-files-with-namespaces.yaml",
        ] {
            let text = std::fs::read_to_string(dir.join(name)).expect("the corpus flow");
            let flow: Value = serde_norway::from_str(&text).expect("yaml");
            let block = &flow["tasks"][0]["tasks"][0]["namespaceFiles"];
            assert!(block.is_object(), "{name}: {block}");
            let parsed = NamespaceFilesSpec::parse(block).expect(name);
            assert_eq!(parsed.enabled.as_deref(), Some("true"));
            assert_eq!(
                parsed.exclude,
                ListProp::Items(vec!["/ignore/**".to_string()])
            );
            assert_eq!(parsed.if_exists, IfExistsProp::Value(IfExists::Overwrite));
            seen += 1;
        }
        assert_eq!(seen, 2);
        let with = spec(json!({
            "namespaces": ["io.test.first", "io.test.second", "io.test.third"],
            "enabled": true, "folderPerNamespace": true, "exclude": ["/ignore/**"],
        }));
        assert_eq!(
            with.namespaces,
            Some(ListProp::Items(vec![
                "io.test.first".to_string(),
                "io.test.second".into(),
                "io.test.third".into()
            ]))
        );
        assert_eq!(with.folder_per_namespace.as_deref(), Some("true"));

        let t = spec(json!({
            "enabled": "{{ inputs.on }}",
            "include": ["{{ inputs.glob }}"],
            "namespaces": ["{{ flow.namespace }}", "shared"],
            "ifExists": "WARN",
            "folderPerNamespace": "{{ inputs.folders }}",
            "exclude": null,
        }));
        assert_eq!(t.enabled.as_deref(), Some("{{ inputs.on }}"));
        assert_eq!(t.if_exists, IfExistsProp::Value(IfExists::Warn));
        assert_eq!(t.exclude, ListProp::empty());
        // An empty block: everything of the flow's own namespace.
        let empty = spec(json!({}));
        assert_eq!((empty.enabled, empty.namespaces), (None, None));
    }

    #[test]
    fn parse_refuses_each_malformed_property_by_name() {
        let refused = |v: Value| NamespaceFilesSpec::parse(&v).unwrap_err();
        let (p, m) = refused(json!({"namespaces": "x"}));
        assert_eq!(
            (p.as_str(), m.as_str()),
            (
                "namespaces",
                "must be a list of strings, or an expression that renders to one"
            )
        );
        // Not an expression by `asList`'s test (it must start AND end with a
        // delimiter), and not a JSON list: refused at publish, as a literal.
        for (key, v) in [
            ("include", "prefix {{ x }}"),
            ("exclude", "{{ x }} suffix"),
            ("namespaces", "[\"a\", 1]"),
            ("namespaces", "{\"a\": 1}"),
        ] {
            let (p, _) = refused(json!({ key: v }));
            assert!(p.starts_with(key), "{key}: {v} -> {p}");
        }
        let (p, m) = refused(json!({"ifExists": "NOPE"}));
        assert_eq!(p, "ifExists");
        assert!(
            m.contains("OVERWRITE, FAIL, WARN or IGNORE") && m.contains("NOPE"),
            "{m}"
        );
        let (p, m) = refused(json!({"include": [1]}));
        assert_eq!(p, "include[0]");
        assert!(m.contains("must be a string"), "{m}");
        let (p, m) = refused(json!({"exclude": ["a", {"x": 1}]}));
        assert_eq!(p, "exclude[1]", "{m}");
        let (p, m) = refused(json!({"unknown": 1}));
        assert_eq!(p, "unknown");
        assert!(m.contains("unknown property 'unknown'"), "{m}");
        let (p, _) = refused(json!({"enabled": 3}));
        assert_eq!(p, "enabled");
        let (p, _) = refused(json!({"folderPerNamespace": ["true"]}));
        assert_eq!(p, "folderPerNamespace");
        let (p, m) = refused(json!(["not", "a", "map"]));
        assert_eq!(p, "");
        assert!(m.contains("must be a map"), "{m}");
        // Lower-case is not Kestra's enum spelling.
        let (p, _) = refused(json!({"ifExists": "overwrite"}));
        assert_eq!(p, "ifExists");
        let (p, _) = refused(json!({"ifExists": ["WARN"]}));
        assert_eq!(p, "ifExists");
    }

    /// Fix round 1, M4 (the user's ruling of 2026-10-06): `ifExists`,
    /// `include`, `exclude` and `namespaces` take expressions, as Kestra's
    /// `Property` does; a literal is still checked at publish.
    #[test]
    fn parse_accepts_an_expression_in_each_property_kestra_types_as_a_property() {
        let t = spec(json!({
            "ifExists": "{{ inputs.mode }}",
            "include": "{{ inputs.globs }}",
            "exclude": " {% if true %}[]{% endif %} ",
            "namespaces": "{{ inputs.nss }}",
        }));
        assert_eq!(
            t.if_exists,
            IfExistsProp::Expression("{{ inputs.mode }}".into())
        );
        assert_eq!(t.include, ListProp::Expression("{{ inputs.globs }}".into()));
        assert_eq!(
            t.exclude,
            ListProp::Expression(" {% if true %}[]{% endif %} ".into())
        );
        assert_eq!(
            t.namespaces,
            Some(ListProp::Expression("{{ inputs.nss }}".into()))
        );
        // `ifExists` is rendered whole (`Property.as`): a template anywhere.
        assert_eq!(
            spec(json!({"ifExists": "{{ x }}RITE"})).if_exists,
            IfExistsProp::Expression("{{ x }}RITE".into())
        );
        // A string holding a JSON list is a literal list (`asList`).
        assert_eq!(
            spec(json!({"namespaces": "[\"a.b\", \"{{ y }}\"]"})).namespaces,
            Some(ListProp::Items(vec!["a.b".into(), "{{ y }}".into()]))
        );
    }

    /// Fix round 1, M4: each expression is rendered with the task's render
    /// (here a fake one), then type-checked; a bad `ifExists` names the field
    /// and the value, and so does a list expression that is not a list.
    #[test]
    fn rendered_expressions_are_type_checked_naming_the_field_and_the_value() {
        let render = |_: &str, s: &str| -> Result<String, String> {
            Ok(match s {
                "{{ mode }}" => "WARN".into(),
                "{{ bad }}" => "SOMETIMES".into(),
                "{{ globs }}" => r#"["/a/**", "regex:/b"]"#.into(),
                "{{ ex }}" => r#"["/a/x"]"#.into(),
                "{{ nss }}" => r#"["w9b.s1","w9b.s2"]"#.into(),
                "{{ one }}" => "w9b.s1".into(),
                other => other.to_string(),
            })
        };
        let r = spec(json!({
            "ifExists": "{{ mode }}", "include": "{{ globs }}",
            "exclude": "{{ ex }}", "namespaces": "{{ nss }}",
        }))
        .render(render)
        .unwrap();
        assert_eq!(r.if_exists, IfExists::Warn);
        assert_eq!(r.include, vec!["/a/**".to_string(), "regex:/b".into()]);
        assert_eq!(r.exclude, vec!["/a/x".to_string()]);
        assert_eq!(
            r.namespaces,
            Some(vec!["w9b.s1".to_string(), "w9b.s2".into()])
        );

        let err = spec(json!({"ifExists": "{{ bad }}"}))
            .render(render)
            .unwrap_err();
        assert!(
            err.contains("namespaceFiles.ifExists") && err.contains("\"SOMETIMES\""),
            "{err}"
        );
        for key in ["include", "exclude", "namespaces"] {
            let err = spec(json!({ key: "{{ one }}" }))
                .render(render)
                .unwrap_err();
            assert!(
                err.contains(&format!("`namespaceFiles.{key}`"))
                    && err.contains("\"w9b.s1\"")
                    && err.contains("list of strings"),
                "{key}: {err}"
            );
        }
        // A disabled block renders none of them.
        let r = spec(json!({"enabled": false, "ifExists": "{{ bad }}"}))
            .render(render)
            .unwrap();
        assert!(!r.enabled);
    }

    /// Fix round 1, M1: the byte cap is over the merged stored inputs.
    #[test]
    fn check_task_bytes_sums_what_the_runner_streams() {
        let stored = BTreeMap::from([
            ("a".to_string(), input("a", 15)),
            ("b".to_string(), input("b", 5)),
        ]);
        assert_eq!(check_task_bytes(&stored, 20), Ok(()));
        let err = check_task_bytes(&stored, 19).unwrap_err();
        assert!(
            err.contains("20 bytes")
                && err.contains("more than the 19")
                && err.contains("workflow.storage.max_task_bytes"),
            "{err}"
        );
        assert_eq!(check_task_bytes(&BTreeMap::new(), 0), Ok(()));
    }

    #[test]
    fn render_reads_enabled_first_and_defaults_the_namespace() {
        let echo = |_: &str, s: &str| Ok::<_, String>(s.replace("{{ x }}", "true"));
        let r = spec(json!({"include": ["*.txt"]})).render(echo).unwrap();
        // Absent: `stage` reads the running flow's own namespace (M5).
        assert_eq!(r.namespaces, None);
        assert!(r.enabled && !r.folder_per_namespace);
        // Disabled: nothing else is rendered (a failing render is not reached).
        let r = spec(json!({"enabled": false, "include": ["{{ boom }}"]}))
            .render(|_, s| {
                if s.contains("boom") {
                    Err("rendered".to_string())
                } else {
                    Ok(s.to_string())
                }
            })
            .unwrap();
        assert!(!r.enabled);
        let r = spec(json!({"folderPerNamespace": "{{ x }}", "namespaces": ["a"]}))
            .render(echo)
            .unwrap();
        assert!(r.folder_per_namespace);
        let err = spec(json!({"enabled": "maybe"})).render(echo).unwrap_err();
        assert!(
            err.contains("namespaceFiles.enabled") && err.contains("maybe"),
            "{err}"
        );
    }

    fn input(uri: &str, size: u64) -> StoredInput {
        StoredInput {
            uri: uri.to_string(),
            object_uri: format!("s3://b/{uri}"),
            size,
        }
    }

    /// The merge rule: `stage` has already decided collisions between
    /// namespaces under `ifExists`, so the merge takes no `ifExists` -- an
    /// `inputFiles` name (inline or stored) always wins. The four values are
    /// run end to end by `handlers::script`'s
    /// `namespace_files_are_staged_as_stored_inputs_and_input_files_overwrite_them_under_every_if_exists`.
    #[test]
    fn merge_lets_every_input_files_name_win() {
        let ns = BTreeMap::from([
            ("x.txt".to_string(), input("ns-x", 1)),
            ("y.txt".to_string(), input("ns-y", 2)),
            ("only.txt".to_string(), input("ns-only", 3)),
        ]);
        let contents = BTreeMap::from([("x.txt".to_string(), "from-input".to_string())]);
        let stored = BTreeMap::from([("y.txt".to_string(), input("exec-y", 9))]);
        assert_eq!(
            merge(ns, &contents, stored),
            BTreeMap::from([
                ("only.txt".to_string(), input("ns-only", 3)),
                ("y.txt".to_string(), input("exec-y", 9)),
            ])
        );
    }
}
