//! **The Kestra plugin handler** (W5b spec §7): every `type: kestra.<FQCN>`
//! in [`crate::plugins::catalog`], run by the plugin host image
//! (`plugin-host/`, `deploy/docker/plugin-host.Dockerfile`) on W5a's
//! configured [`TaskRunner`].
//!
//! # What one run does, in order
//!
//! 1. **parse** the leaf spec with [`plugins::parse`] -- the function publish
//!    uses, so a draft that never passed publish is refused here by the same
//!    named errors;
//! 2. **gate** with [`gate::check`] -- the one gate publish also calls -- with
//!    `workflow_kestra_plugins` and `workflow_containers` **read now**
//!    ([`FeatureState`]). A refusal with nothing to adopt
//!    ([`TaskRunner::adoptable`]) fails here, before any secret is read
//!    (fix round 1, M2);
//! 3. **render** the parsed properties ONCE with
//!    [`render_with_secrets`], resolving `secret()` in the execution's
//!    workspace. It is handed the author's UNRENDERED properties: it renders
//!    once (Kestra's `recursiveRendering=false`) and collects literal secret
//!    names from that source text. A render error is already masked by it;
//! 4. **bridge storage** ([`Kestra::bridge`], W9a spec §5, §9.1): every
//!    rendered property value that is a WHOLE `kestra:///` string is resolved
//!    through the run's render snapshot -- the one readability decision
//!    (`storage::resolve_readable_many`, taken when the run was claimed) -- and
//!    staged at `.egret/storage/<n>`; an unreadable one fails the task here,
//!    naming it, before any container exists. With internal storage the run
//!    collects `.egret/out/` ([`Naming::UnderPrefixVerbatim`]) under its own
//!    prefix, and the host's `EgretStorage` writes there. **W9b Task 10
//!    (spec §10, 2026-10-06):** a WHOLE `nsfile://…` or `kestra:///…/_files/…`
//!    value names a namespace file: it is read-checked (`authorize`, Read, in
//!    the execution's workspace) and resolved by `nsfiles::stage`'s one
//!    resolver on [`KestraRuntime::pool`], staged at the same numbering, and
//!    listed in `task.json`'s `namespaceFiles` -- which the host's
//!    `EgretNamespace` reads (`storage().namespace(ns)`);
//! 5. **build** the [`TaskCommands`] ([`KestraRuntime::commands`]): the
//!    configured host image, `/bin/sh -c "exec java …"`, the one input file
//!    `task.json` (`{"type": FQCN, "properties": {…rendered…}, "mask":
//!    [values], "storage": {uri: path}, "baseUri", "maxTaskBytes",
//!    "context", "namespaceFiles": [{uri, namespace, path, revision, size,
//!    staged}]}`), the staged files, `host_protocol: true`;
//! 6. **mask**, twice (fix round 1, I1): the host masks every line of its
//!    own stdout and stderr (but the outputs marker) with `task.json`'s
//!    `mask` before it leaves the JVM -- which is what `kubectl logs`, a
//!    node's log shipper and Docker's json-file driver keep -- and the
//!    runner is handed a [`TaskInput`]
//!    whose log is `task.log.masked(values)`, so egret masks again before
//!    `RunLog`;
//! 7. **run** -- or, when the gate refused, **adopt** a container or pod that
//!    already exists for this (run, attempt), exactly as the script handler
//!    does; `adopt` never creates;
//! 8. **map** the exit code ([`outcome_of`]), with the files collected and
//!    the §5 check that no output names a file under this run's prefix that
//!    was not collected.
//!
//! # Files a plugin writes are not masked
//!
//! As outputs are not (W5b §12, W9a §9.1): a `fetchType: STORE` of a query
//! that selects a secret stores it in the file, exactly as Kestra does.
//!
//! # Where a secret's value goes, and where it does not
//!
//! Into `task.json`'s `properties` and `mask` only -- an input file, streamed
//! as a tar into `docker cp -`'s stdin or the `pods/exec` stdin on
//! Kubernetes (`storage::transfer::write_tar`, W9a Task 5), never written to
//! the worker's disk. **Never** into the argv, the env,
//! the image, or the pod spec: the command reads the file by
//! `$WORKING_DIR`. Every FAILED reason this handler writes is passed
//! through [`crate::logs::mask`] with the resolved values: the host's
//! `egretFailure` message can echo a connection string. **Outputs are not
//! masked** (spec §12, as Kestra): a query that selects a secret returns it,
//! and the host's `::{"outputs": …}::` marker is the one line of its stdout
//! it leaves as it is (fix round 1, I1).
//!
//! # The exit codes (`EgretPluginHost`)
//!
//! `0` success, the `::{"outputs": …}::` marker's map as the outputs, as it
//! is; `2` the host refused the task (unknown type, invalid properties);
//! `3` the plugin threw; `4` the plugin asked for something egret does not
//! provide -- `localPath`/`decrypt`/`sdk`/`services`, and a plugin's
//! `namespaceKv()` (2026-10-07, W9c: the KV store is the `core.kv` tasks'
//! and `kv()`'s; a plugin's `namespaceKv()` still throws, pointing at
//! them). Internal storage is provided (W9a): a refused storage call (a write outside the prefix, a file not
//! staged) is the plugin's own failure, exit 3. So are namespace files
//! (W9b, 2026-10-06): a namespace file not staged, or any namespace-file
//! write, is the plugin's own failure, exit 3. Any other code (137: the JVM was OOM-killed) is
//! `the plugin host exited with <n>`.
//!
//! # A timeout is the budget's outcome, whatever the host printed
//!
//! `worker::timeout::guard` drops this handler's future when the task's
//! budget (its `timeout:`, or `workflow.container_default_timeout_s` when it
//! has none) runs out, then `cancel::guarded` calls [`TaskHandler::cancel`],
//! which asks the runner to stop the container with grace
//! `workflow.plugin_host.kill_grace_s`: SIGTERM reaches the JVM (PID 1,
//! through `exec`), whose shutdown hook calls the plugin's `kill()`. The run
//! is FAILED with W11's `Timeout after …`.
//!
//! **Known and accepted:** after SIGTERM the host prints `WARN killed:
//! timeout` and, when `kill()` made `run()` throw, an exit-3 `egretFailure`
//! marker. Nobody reads those lines: the runner's future was dropped before
//! `cancel` was called, so no pump is attached during the grace. They are
//! not stored in W11's log and do not change the outcome.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use egret_config::{PluginConfigurations, PluginHostConfig};
use egret_core::{Error, Result};
use serde_json::{Map, Value};

use super::super::commands::{
    CollectTo, CommandsResult, Naming, StoredInput, TaskCommands, TaskRunner, DEFAULT_INTERPRETER,
};
use super::super::{Handlers, Outcome, TaskHandler, TaskInput};
use super::container::mapped;
use super::script::FeatureState;
use crate::logs::mask;
use crate::plugins::{self, gate};
use crate::runners::files::{is_namespace_file_uri, NSFILE_SCHEME};
use crate::runners::render_str;
use crate::runners::secrets::{render_with_secrets, SecretSource};
use crate::scripts::{RunnerKind, RunnerSpec};
use crate::storage::nsfiles::stage::{check_task_bytes, resolve_ns_file};
use crate::storage::uri::SCHEME_PREFIX;
use crate::worker::commands::NoObserver;

/// The one input file: what the host reads (`EgretPluginHost run <file>`).
pub const TASK_JSON: &str = "task.json";

/// The host's classpath inside the image
/// (`deploy/docker/plugin-host.Dockerfile`: Kestra core and the host jar in
/// `lib/`, the three locked plugin jars in `plugins/`).
pub const HOST_CLASSPATH: &str = "/opt/egret-plugin-host/lib/*:/opt/egret-plugin-host/plugins/*";

/// The host's main class.
pub const HOST_MAIN: &str = "io.kestra.core.runners.EgretPluginHost";

/// The env var the host's shutdown hook reads: how long it waits for `run()`
/// to return after calling `kill()`.
pub const ENV_KILL_WAIT_MS: &str = "EGRET_KILL_WAIT_MS";

/// Where staged files go, relative to the working directory: the `n`th
/// distinct URI at `.egret/storage/<n>` (W9a spec §5).
pub const STAGED_DIR: &str = ".egret/storage";

/// What one run's storage looks like to the host (W9a spec §5, §9.1): the
/// `task.json` keys and the files the runner moves.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bridge {
    /// `task.json`'s `storage`: `kestra://` URI -> `.egret/storage/<n>`.
    pub storage: BTreeMap<String, String>,
    /// `.egret/storage/<n>` -> the file the runner streams in.
    pub stored_inputs: BTreeMap<String, StoredInput>,
    /// Where `.egret/out/` is collected to; its `task_prefix` is `task.json`'s
    /// `baseUri`. `None`: egret has no internal storage, and the host refuses
    /// every write naming `[storage]`.
    pub collect_to: Option<CollectTo>,
    /// `task.json`'s `maxTaskBytes`: `workflow.storage.max_task_bytes`.
    pub max_task_bytes: u64,
    /// `task.json`'s `context`, sent for every task run of a flow WITH or
    /// without internal storage (fix round 1, I1): `namespace`, `flowId`, `executionId` (the
    /// run's own execution, an iteration's id inside a `Loop`), `taskId`,
    /// `taskRunId` -- what Kestra's `StorageContext.forTask` is built from,
    /// whose context URI equals `baseUri` (`plugin-host`'s
    /// `EgretStorageTest`, over `base-uri-cases.json`).
    pub context: Option<Value>,
    /// `task.json`'s `namespaceFiles` (W9b spec §10): every namespace file a
    /// whole property value named, staged beside [`storage`](Self::storage)'s
    /// files in one numbering. Not in `storage` (the controller's ruling).
    pub namespace_files: Vec<StagedNamespaceFile>,
}

/// One entry of `task.json`'s `namespaceFiles` (W9b spec §10, Task 10).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct StagedNamespaceFile {
    /// The property value, as it was rendered: `nsfile://…` or
    /// `kestra:///…/_files/…[.vN]`.
    pub uri: String,
    pub namespace: String,
    /// The stored path, with its leading '/'.
    pub path: String,
    pub revision: u32,
    pub size: u64,
    /// `.egret/storage/<n>`, relative to the working directory.
    pub staged: String,
}

/// Everything a [`Kestra`] handler needs besides the task: built once per
/// process (`runtime::handlers_for`) and shared by every catalog type.
pub struct KestraRuntime {
    /// The one runner this deployment has -- the script handler's `Arc`.
    pub runner: Arc<dyn TaskRunner>,
    /// Which runner [`runner`](Self::runner) is.
    pub runner_kind: RunnerKind,
    /// The workspace half of the gate.
    pub features: Arc<dyn FeatureState>,
    /// `workflow.plugin_configurations`: the `kestra` entry's
    /// `allowed-types`.
    pub plugin_configurations: PluginConfigurations,
    /// `workflow.plugin_host`.
    pub host: PluginHostConfig,
    /// Where `secret()` reads, in the execution's workspace.
    pub secrets: Arc<dyn SecretSource>,
    /// Internal storage (`[storage]`): where staged `kestra://` values are
    /// read from and `.egret/out/` is collected to (W9a spec §9.1). `None`: a
    /// `kestra://` value fails the task, and the host refuses every write.
    pub files: Option<Arc<crate::storage::FileStore>>,
    /// W9b Task 10: where a namespace-file value is resolved. One connection
    /// is taken for a run's namespace files and returned before its runner is
    /// called. `None`: a namespace-file value fails the task naming
    /// `[storage]`.
    pub pool: Option<sqlx::PgPool>,
}

impl std::fmt::Debug for KestraRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KestraRuntime")
            .field("runner_kind", &self.runner_kind)
            .field("runner", &self.runner)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

impl KestraRuntime {
    /// The shell command: `exec java …`.
    ///
    /// **`exec` is load-bearing**: without it `/bin/sh` is PID 1, and a
    /// SIGTERM on a timeout never reaches the JVM, so the plugin's `kill()`
    /// never runs and the query outlives its task. The interpreter must be a
    /// shell `-c` for the `$WORKING_DIR` expansion, and on Kubernetes because
    /// a non-shell interpreter merges stderr into stdout (W5b Task 5).
    pub fn command(&self) -> String {
        format!(
            "exec java -Xmx{heap} -cp '{HOST_CLASSPATH}' {HOST_MAIN} run \"${env}/{TASK_JSON}\"",
            heap = self.host.jvm_max_heap,
            env = super::super::commands::ENV_WORKING_DIR,
        )
    }

    /// The host's kill wait: the grace less 2 s, so `run()` returning after
    /// `kill()` still leaves the JVM time to exit before SIGKILL (spec §7.1).
    fn kill_wait_ms(&self) -> u64 {
        self.host.kill_grace_s.saturating_sub(2) * 1000
    }

    /// Step 5 of the module doc. `properties` is the RENDERED map; it goes
    /// into `task.json` and nowhere else. `mask` is every secret value the
    /// render handed out: the host replaces each by `******` on its own
    /// stdout and stderr (fix round 1, I1). The file already holds them in
    /// `properties`, so listing them again adds no place a value is.
    /// `bridge` is step 4's: `task.json`'s storage keys, the staged files,
    /// and -- with internal storage -- `.egret/out/` collected verbatim.
    pub fn commands(
        &self,
        fqcn: &str,
        properties: Map<String, Value>,
        mask: &[String],
        bridge: Bridge,
    ) -> TaskCommands {
        let mut task_json = serde_json::json!({
            "type": fqcn,
            "properties": Value::Object(properties),
            "mask": mask,
            "storage": bridge.storage,
            "namespaceFiles": bridge.namespace_files,
        });
        if let Some(to) = &bridge.collect_to {
            task_json["baseUri"] = Value::String(to.task_prefix.clone());
            task_json["maxTaskBytes"] = Value::from(bridge.max_task_bytes);
        }
        if let Some(context) = bridge.context {
            task_json["context"] = context;
        }
        TaskCommands {
            container_image: self.host.image.clone().unwrap_or_default(),
            interpreter: DEFAULT_INTERPRETER.iter().map(|s| s.to_string()).collect(),
            before_commands: Vec::new(),
            commands: vec![self.command()],
            env: BTreeMap::from([(
                ENV_KILL_WAIT_MS.to_string(),
                self.kill_wait_ms().to_string(),
            )]),
            input_files: BTreeMap::from([(TASK_JSON.to_string(), task_json.to_string())]),
            stored_inputs: bridge.stored_inputs,
            output_files: Vec::new(),
            // The host's `EgretStorage` writes `.egret/out/<path under baseUri>`;
            // the runner collects it as `{baseUri}{path}`.
            output_directory: bridge.collect_to.is_some(),
            collect_to: bridge.collect_to,
            host_protocol: true,
        }
    }
}

/// Register one handler per catalog class, under `kestra.<FQCN>`
/// ([`plugins::prefixed_types`]). With `Handlers::register`, not
/// `add_publishable`: these types have no `TASK_TYPES` row, publish accepts
/// them through the catalog. Called only when a container runner exists.
pub fn register(handlers: &mut Handlers, rt: Arc<KestraRuntime>) {
    for type_id in plugins::prefixed_types() {
        handlers.register(
            type_id,
            Arc::new(Kestra {
                type_id,
                rt: rt.clone(),
            }),
        );
    }
}

/// One catalog type over the shared [`KestraRuntime`].
#[derive(Debug)]
pub struct Kestra {
    type_id: &'static str,
    rt: Arc<KestraRuntime>,
}

/// Step 8: what a finished host means. Every message is masked with
/// `secrets`; the outputs are not (spec §12), nor are the files the plugin
/// wrote (W9a §9.1).
///
/// **The collected files ride along whatever the state** (as the script
/// handler's, `container::outcome_for`): they are indexed with the outcome.
/// `task_prefix` is this run's own prefix: on exit 0, an output naming a URI
/// under it that was never collected fails the task (W9a spec §5) -- a host
/// that returned a URI it did not write points at nothing.
pub fn outcome_of(
    type_id: &str,
    result: &CommandsResult,
    secrets: &[String],
    task_prefix: Option<&str>,
) -> Outcome {
    let (files, shared) = super::container::distinct_files(result);
    let failed = |why: String| Outcome {
        files: files.clone(),
        ..Outcome::failed_with(format!("{type_id}: {}", mask(&why, secrets)))
    };
    // A problem egret found after the host ran (the outputs over W5a's
    // 1 MiB cap, a file over a storage cap, two outputs at one URI) fails
    // the task whatever the exit code.
    if let Some(why) = result.failure.clone().or(shared) {
        return failed(why);
    }
    let said = || {
        result
            .host_failure
            .as_ref()
            .map(|f| f.message.clone())
            .unwrap_or_else(|| "(the host printed no reason)".to_string())
    };
    match result.exit_code {
        0 => {
            let outputs = Value::Object(result.vars.clone());
            let unbacked = task_prefix.and_then(|prefix| {
                crate::storage::collect::check_outputs_collected(&outputs, prefix, &files).err()
            });
            match unbacked {
                Some(why) => {
                    let why = match encoded_twin(&outputs, &files) {
                        Some(hint) => format!("{why}. {hint}"),
                        None => why,
                    };
                    Outcome {
                        outputs: Some(outputs),
                        ..failed(why)
                    }
                }
                None => Outcome {
                    files,
                    ..Outcome::succeeded_with(outputs)
                },
            }
        }
        2 => failed(format!("the plugin host refused the task: {}", said())),
        3 => failed(format!("the plugin failed: {}", said())),
        // W9b (2026-10-06): namespace files are provided, so no longer named here.
        4 => failed(format!(
            "the plugin needs a Kestra service egret does not provide to plugins: {}",
            said()
        )),
        n => failed(format!("the plugin host exited with {n}")),
    }
}

impl Kestra {
    /// Step 4 of the module doc (W9a spec §5, §9.1; W9b spec §10). `Ok(Err)`
    /// is the task's failure, decided before the gate's runner call and
    /// before any container exists; `Err` is egret's outage (the database or
    /// the store while resolving a namespace file), so the run stays leased
    /// and is re-claimed with no attempt spent (W9b spec §11).
    ///
    /// **Whole string leaves only.** A property value that IS a `kestra:///`
    /// or `nsfile://` URI is a file the plugin reads (`from:`, `inputFile:`);
    /// a URI quoted inside SQL text is not a file read and is not staged -- a
    /// spec ambiguity resolved this way, and reported (W9a Task 9). Each
    /// distinct URI is staged once, at `.egret/storage/<n>` in first-seen
    /// order, execution files and namespace files in ONE numbering.
    ///
    /// * An execution file (`classify` says so): readability is the run's
    ///   render snapshot's ([`RenderFiles::lookup`]), i.e.
    ///   `storage::resolve_readable_many` -- never a second predicate.
    /// * A namespace file (`nsfile://…`, `kestra:///…/_files/…`): the one
    ///   resolver, `nsfiles::stage::resolve_ns_file` (`authorize`, Read, any
    ///   namespace of the execution's workspace; the latest live revision,
    ///   or the revision a `kestra://` URI names, R1) on ONE connection from
    ///   [`KestraRuntime::pool`], dropped before this returns. Bounded by
    ///   `max_namespace_files` (their count) and, through the script
    ///   handler's `check_task_bytes`, `max_task_bytes` (the total of EVERY
    ///   staged file, execution files included; fix round 1, M1).
    ///
    /// [`RenderFiles::lookup`]: crate::runners::RenderFiles::lookup
    pub(crate) async fn bridge(
        &self,
        task: &TaskInput,
        rendered: &Map<String, Value>,
    ) -> Result<std::result::Result<Bridge, String>> {
        let mut leaves = Vec::new();
        for (key, value) in rendered {
            whole_uri_leaves(key.clone(), value, &mut leaves);
        }
        let mut bridge = Bridge::default();
        // The namespace files' one connection: taken at the first such value,
        // dropped at the end of this function, before any runner.
        let mut conn = None;
        let mut namespace_uris = std::collections::BTreeSet::new();
        for (path, uri) in leaves {
            let why = |reason: &str| format!("property `{path}`: {reason}");
            if is_namespace_file_uri(&uri) {
                if !namespace_uris.insert(uri.clone()) {
                    continue;
                }
                match self
                    .stage_namespace_file(task, &uri, &mut conn, &mut bridge)
                    .await?
                {
                    Ok(()) => continue,
                    Err(reason) => return Ok(Err(why(&reason))),
                }
            }
            if bridge.storage.contains_key(&uri) {
                continue;
            }
            let Some(scope) = task.render.files() else {
                return Ok(Err(why(&format!(
                    "{uri} cannot be read: this render has no file scope"
                ))));
            };
            let meta = match scope.0.lookup(&uri) {
                Ok(m) => m,
                Err(u) => return Ok(Err(why(u.message()))),
            };
            let Some(files) = &self.rt.files else {
                return Ok(Err(why(&format!(
                    "{uri} cannot be read: egret has no internal storage configured (the \
                     `[storage]` section of its configuration)"
                ))));
            };
            let object_uri = match files.object_uri(task.workspace_id, &uri) {
                Ok(o) => o,
                Err(e) => return Ok(Err(why(&e))),
            };
            // W9a's numbering, shared with the namespace files (Task 10).
            let rel = format!("{STAGED_DIR}/{}", bridge.stored_inputs.len());
            bridge.storage.insert(uri.clone(), rel.clone());
            bridge.stored_inputs.insert(
                rel,
                StoredInput {
                    uri,
                    object_uri,
                    size: meta.size,
                },
            );
        }
        drop(conn);
        // Fix round 1, M1: the script handler's own check over every staged
        // file, execution files and namespace files alike -- one cap, one
        // meaning, one implementation.
        if let Some(files) = &self.rt.files {
            if let Err(why) = check_task_bytes(&bridge.stored_inputs, files.caps.max_task_bytes) {
                return Ok(Err(why));
            }
        }
        let flow = (
            render_str("{{ flow.namespace }}", &task.render),
            render_str("{{ flow.id }}", &task.render),
        );
        // Fix round 1, I1: `context` does not depend on internal storage. It
        // goes with every task run of a flow, so a host with no `baseUri`
        // reaches EgretStorage's own refusal, naming `[storage]`, on a write.
        if let (Ok(namespace), Ok(flow_id)) = &flow {
            bridge.context = Some(serde_json::json!({
                "namespace": namespace,
                "flowId": flow_id,
                "executionId": task.execution_id.hyphenated().to_string(),
                "taskId": task.task_id,
                "taskRunId": task.run_id.hyphenated().to_string(),
            }));
        }
        let Some(files) = &self.rt.files else {
            return Ok(Ok(bridge));
        };
        let (Ok(namespace), Ok(flow_id)) = flow else {
            return Ok(Err(
                "this run has no storage prefix for the files a plugin writes: it is not a task \
                 run of a flow's execution"
                    .to_string(),
            ));
        };
        let prefix = match task.render.files().and_then(|f| f.0.task_prefix()) {
            Some(p) => p.to_string(),
            None => match crate::storage::uri::task_prefix(
                &namespace,
                &flow_id,
                task.execution_id,
                &task.task_id,
                task.run_id,
            ) {
                Ok(p) => p,
                Err(why) => return Ok(Err(why)),
            },
        };
        bridge.collect_to = Some(CollectTo {
            workspace: task.workspace_id,
            task_prefix: prefix,
            naming: Naming::UnderPrefixVerbatim,
        });
        bridge.max_task_bytes = files.caps.max_task_bytes;
        Ok(Ok(bridge))
    }

    /// One namespace-file value of [`Self::bridge`] (W9b spec §10, Task 10):
    /// resolved by the one resolver on `conn` (taken from
    /// [`KestraRuntime::pool`] at the first value), staged at the next
    /// `.egret/storage/<n>`, and listed in `namespaceFiles`. The running
    /// flow's namespace is the execution's (Task 4's
    /// `RenderFiles::flow_namespace`), never a rendered `{{ flow.namespace }}`.
    async fn stage_namespace_file(
        &self,
        task: &TaskInput,
        uri: &str,
        conn: &mut Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
        bridge: &mut Bridge,
    ) -> Result<std::result::Result<(), String>> {
        let type_id = self.type_id;
        let Some(own) = task.render.files().and_then(|f| f.0.flow_namespace()) else {
            return Ok(Err(format!(
                "{uri} cannot be read: a namespace file is read as the running flow's, and this \
                 run is not a task run of a flow's execution"
            )));
        };
        let (Some(pool), Some(files)) = (&self.rt.pool, &self.rt.files) else {
            return Ok(Err(format!(
                "{uri} cannot be read: egret has no internal storage configured (the \
                 `[storage]` section of its configuration)"
            )));
        };
        let cap = files.caps.max_namespace_files;
        if bridge.namespace_files.len() as u64 >= cap {
            return Ok(Err(format!(
                "{uri} is one namespace file more than the {cap} one task may stage \
                 (workflow.storage.max_namespace_files)"
            )));
        }
        let conn = match conn {
            Some(c) => c,
            None => conn.insert(pool.acquire().await.map_err(|e| {
                Error::Internal(anyhow::anyhow!(
                    "{type_id}: staging namespace files: the database is unavailable: {e}"
                ))
            })?),
        };
        let resolved = resolve_ns_file(conn, files, task.workspace_id, own, uri)
            .await
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!("{type_id}: staging namespace files: {e}"))
            })?;
        let resolved = match resolved {
            Ok(r) => r,
            Err(why) => return Ok(Err(why)),
        };
        let rel = format!("{STAGED_DIR}/{}", bridge.stored_inputs.len());
        bridge.namespace_files.push(StagedNamespaceFile {
            uri: uri.to_string(),
            namespace: resolved.namespace,
            path: resolved.path,
            revision: resolved.revision,
            size: resolved.input.size,
            staged: rel.clone(),
        });
        bridge.stored_inputs.insert(rel, resolved.input);
        Ok(Ok(()))
    }
}

/// Fix round 1, M4: when an output names a URI that was never collected but
/// a collected file's URI is the same after percent-decoding, say so: the
/// host returned an encoded spelling of a file egret stored under its raw
/// name (or the reverse), which no author can see from the URIs alone.
fn encoded_twin(outputs: &Value, files: &[crate::storage::StoredFile]) -> Option<String> {
    let mut named = std::collections::BTreeSet::new();
    crate::storage::scope::walk(outputs, &mut named);
    named.iter().find_map(|uri| {
        if files.iter().any(|f| &f.uri == uri) {
            return None;
        }
        let want = percent_decoded(uri);
        files
            .iter()
            .find(|f| percent_decoded(&f.uri) == want)
            .map(|f| {
                format!(
                    "The host returned {uri}, which is percent-encoded differently from the file \
                     egret stored, {}: egret keeps a URI exactly as written and never decodes it",
                    f.uri
                )
            })
    })
}

/// `%XX` decoded byte-wise (invalid escapes kept), for [`encoded_twin`] only.
fn percent_decoded(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Every string leaf of `value` that is WHOLE a `kestra:///` or (W9b Task 10)
/// `nsfile://` URI, with its property path (`from`, `parameters.a`,
/// `list[1]`), in document order.
fn whole_uri_leaves(path: String, value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::String(s) if s.starts_with(SCHEME_PREFIX) || s.starts_with(NSFILE_SCHEME) => {
            out.push((path, s.clone()))
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                whole_uri_leaves(format!("{path}[{i}]"), item, out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                whole_uri_leaves(format!("{path}.{k}"), v, out);
            }
        }
        _ => {}
    }
}

#[async_trait::async_trait]
impl TaskHandler for Kestra {
    fn type_id(&self) -> &'static str {
        self.type_id
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let type_id = task.type_id.as_str();

        // 1. parse -- publish's own function.
        let parsed = match plugins::parse(type_id, &task.spec) {
            Ok(p) => p,
            Err(refusals) => {
                let why: Vec<&str> = refusals.iter().map(|r| r.message.as_str()).collect();
                return Ok(Outcome::failed_with(format!(
                    "{type_id}: {}",
                    why.join("; ")
                )));
            }
        };

        // 2. the gate, with both features read now -- BEFORE any secret is
        //    resolved (fix round 1, M2): a refused workspace reads none.
        let feature_enabled = self
            .rt
            .features
            .kestra_plugins_enabled(task.workspace_id)
            .await?;
        let containers_enabled = self
            .rt
            .features
            .containers_enabled(task.workspace_id)
            .await?;
        let refused = gate::check(&gate::GateInput {
            feature_enabled,
            containers_enabled,
            plugin_configurations: &self.rt.plugin_configurations,
            runner: Some(self.rt.runner_kind),
            fqcn: parsed.fqcn,
        })
        .err();
        // A refused run is failed unless a container already exists for this
        // (run, attempt) -- W5a's adoption. Only then are secrets resolved:
        // the adopted container's lines must be masked with them.
        if let Some(refusal) = &refused {
            if !self.rt.runner.adoptable(task).await? {
                return Ok(Outcome::failed_with(format!("{type_id}: {refusal}")));
            }
        }

        // 3. render once, resolving secret(), from the UNRENDERED properties.
        //    An error fails the task, also on the adopting path: without the
        //    values nothing the container prints could be masked, and
        //    `release` removes the container once the failure commits.
        let (rendered, resolved) = match render_with_secrets(
            &parsed.properties,
            &task.render,
            task.workspace_id,
            self.rt.secrets.as_ref(),
        )
        .await
        {
            Ok(r) => r,
            // Already masked by `render_with_secrets`.
            Err(message) => return Ok(Outcome::failed_with(format!("{type_id}: {message}"))),
        };
        let secrets = resolved.values;

        // 4. the storage bridge: stage the whole `kestra://` and `nsfile://`
        //    values, and collect `.egret/out/`. A refusal fails here, before
        //    any runner; an outage is a worker `Err` (W9b spec §11).
        let bridge = match self.bridge(task, &rendered).await? {
            Ok(b) => b,
            Err(why) => {
                return Ok(Outcome::failed_with(format!(
                    "{type_id}: {}",
                    mask(&why, &secrets)
                )))
            }
        };
        // The run's own prefix, for the §5 check whether or not it collects.
        let task_prefix = bridge
            .collect_to
            .as_ref()
            .map(|c| c.task_prefix.clone())
            .or_else(|| {
                task.render
                    .files()
                    .and_then(|f| f.0.task_prefix())
                    .map(str::to_string)
            });

        // 5. the commands; 6. the masked log.
        let cmds = self.rt.commands(parsed.fqcn, rendered, &secrets, bridge);
        let masked = TaskInput {
            log: task.log.masked(&secrets),
            ..task.clone()
        };
        let spec = RunnerSpec::defaults(self.rt.runner_kind);

        // 7. run, or adopt what already exists.
        let ran = match refused {
            Some(refusal) => match self
                .rt
                .runner
                .adopt(&masked, &cmds, &spec, &NoObserver)
                .await
            {
                Ok(Some(result)) => Ok(result),
                Ok(None) => return Ok(Outcome::failed_with(format!("{type_id}: {refusal}"))),
                Err(e) => Err(e),
            },
            None => self.rt.runner.run(&masked, &cmds, &spec, &NoObserver).await,
        };

        // 8. map.
        match ran {
            Ok(result) => Ok(outcome_of(
                type_id,
                &result,
                &secrets,
                task_prefix.as_deref(),
            )),
            Err(e) => match mapped(type_id, e) {
                Ok(o) => Ok(Outcome {
                    message: o.message.map(|m| mask(&m, &secrets).into_owned()),
                    ..o
                }),
                Err(e) => Err(e),
            },
        }
    }

    /// **A graceful cancel**: the runner stops the attempt's container or pod
    /// with grace `workflow.plugin_host.kill_grace_s`, and returns only once
    /// it stopped (or grace + 5 s passed) -- so the plugin's `kill()` runs
    /// before the outcome commits and before `release` removes the pod. See
    /// the module doc for the host's lines during the grace.
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        self.rt
            .runner
            .cancel(task, Duration::from_secs(self.rt.host.kill_grace_s))
            .await
    }

    /// **After the outcome committed**: the runner deletes the attempt's
    /// container or pod. A plugin task has no `taskRunner:` (publish refuses
    /// it), so the spec is always the configured runner's defaults
    /// (`delete: true`). A task that does not parse never reached a runner.
    async fn release(&self, task: &TaskInput) -> Result<()> {
        if plugins::parse(task.type_id.as_str(), &task.spec).is_err() {
            return Ok(());
        }
        self.rt
            .runner
            .release(task, &RunnerSpec::defaults(self.rt.runner_kind))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::container::testing::Spy;
    use super::*;
    use crate::executor::RunState;
    use crate::logs::LogLevel;
    use crate::runners::secrets::SecretError;
    use crate::worker::commands::HostFailure;
    use crate::worker::handlers::script::FixedFeatureState;
    use crate::worker::handlers::testing::{input, recorded};
    use egret_config::{PluginConfiguration, PluginConfigurationValues};

    const QUERY: &str = "kestra.io.kestra.plugin.jdbc.postgresql.Query";
    const FQCN: &str = "io.kestra.plugin.jdbc.postgresql.Query";
    const IMAGE: &str = "registry.example/egret-plugin-host:test";
    const PASSWORD: &str = "pa55-w0rd!xyz";

    /// `secret('db', subkey='password')` answers [`PASSWORD`], in any
    /// workspace; every read is recorded.
    #[derive(Default)]
    struct OneSecret {
        reads: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl SecretSource for OneSecret {
        async fn get(
            &self,
            _workspace: uuid::Uuid,
            name: &str,
        ) -> std::result::Result<BTreeMap<String, String>, SecretError> {
            self.reads.lock().unwrap().push(name.to_string());
            if name == "db" {
                Ok(BTreeMap::from([
                    ("username".to_string(), "admin".to_string()),
                    ("password".to_string(), PASSWORD.to_string()),
                ]))
            } else {
                Err(SecretError::NotFound { name: name.into() })
            }
        }
    }

    fn runtime(spy: Arc<Spy>, feature: bool, configs: PluginConfigurations) -> Arc<KestraRuntime> {
        Arc::new(KestraRuntime {
            runner: spy,
            runner_kind: RunnerKind::Docker,
            features: Arc::new(FixedFeatureState(feature)),
            plugin_configurations: configs,
            host: PluginHostConfig {
                image: Some(IMAGE.to_string()),
                ..PluginHostConfig::default()
            },
            secrets: Arc::new(OneSecret::default()),
            files: None,
            pool: None,
        })
    }

    /// The handler for [`QUERY`], everything allowed.
    fn handler(spy: Arc<Spy>) -> Kestra {
        Kestra {
            type_id: QUERY,
            rt: runtime(spy, true, PluginConfigurations::default()),
        }
    }

    fn spec() -> Value {
        serde_json::json!({
            "url": "jdbc:postgresql://pg:5432/w5b",
            "username": "w5b",
            "password": "{{ secret('db', subkey='password') }}",
            "sql": "SELECT {{ inputs.n }} AS one",
            "fetchType": "FETCH_ONE",
        })
    }

    fn task() -> TaskInput {
        TaskInput {
            render: crate::runners::RenderContext::from_json_object(
                &serde_json::json!({"inputs": {"n": 1}}),
            )
            .expect("an object"),
            ..input(QUERY, spec())
        }
    }

    fn task_json(cmds: &TaskCommands) -> Value {
        serde_json::from_str(cmds.input_files.get(TASK_JSON).expect("task.json")).expect("json")
    }

    fn ok(vars: Value) -> CommandsResult {
        CommandsResult {
            exit_code: 0,
            vars: vars.as_object().cloned().expect("an object"),
            ..CommandsResult::default()
        }
    }

    fn message(out: &Outcome) -> &str {
        out.message.as_deref().unwrap_or_default()
    }

    // ---- the run-time gate --------------------------------------------------

    /// **A disabled feature fails the run and no runner creates anything**:
    /// the one runner call is `adopt`, which never creates.
    #[tokio::test]
    async fn a_disabled_feature_fails_the_run_and_calls_no_runner() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let h = Kestra {
            type_id: QUERY,
            rt: runtime(spy.clone(), false, PluginConfigurations::default()),
        };
        let out = h
            .run(&task())
            .await
            .expect("a refusal is the task's failure");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            message(&out).contains("'workflow_kestra_plugins' feature is disabled"),
            "{:?}",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty(), "`run` never called");
        assert_eq!(
            *spy.adopt_calls.lock().unwrap(),
            0,
            "nothing to adopt, so `adopt` is not called either"
        );
    }

    /// **The gate comes before `secret()` is resolved** (fix round 1, M2): a
    /// refused run with nothing to adopt reads no secret at all; one with a
    /// container to adopt resolves them -- it must, to mask what that
    /// container prints -- and adopts it.
    #[tokio::test]
    async fn a_refused_run_reads_no_secret_unless_it_adopts() {
        for (adoptable, reads) in [(false, 0), (true, 1)] {
            let spy = Spy::ok(ok(serde_json::json!({})));
            if adoptable {
                *spy.adoptable.lock().unwrap() = Some(ok(serde_json::json!({})));
            }
            let secrets = Arc::new(OneSecret::default());
            let mut rt = runtime(spy.clone(), false, PluginConfigurations::default());
            Arc::get_mut(&mut rt).expect("unshared").secrets = secrets.clone();
            let out = Kestra { type_id: QUERY, rt }
                .run(&task())
                .await
                .expect("no worker error");
            assert_eq!(
                secrets.reads.lock().unwrap().len(),
                reads,
                "adoptable = {adoptable}"
            );
            assert_eq!(
                out.state,
                if adoptable {
                    RunState::Success
                } else {
                    RunState::Failed
                },
                "{:?}",
                out.message
            );
            assert!(spy.seen.lock().unwrap().is_empty(), "never created");
        }
    }

    /// **A type outside `allowed-types` is refused at run** by the same gate.
    #[tokio::test]
    async fn a_type_outside_allowed_types_fails_the_run_and_calls_no_runner() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let configs = PluginConfigurations(vec![PluginConfiguration {
            r#type: plugins::CONFIG_TYPE.to_string(),
            values: PluginConfigurationValues {
                allowed_images: None,
                allowed_types: Some(vec!["io.kestra.plugin.jdbc.trino.*".into()]),
            },
        }]);
        let h = Kestra {
            type_id: QUERY,
            rt: runtime(spy.clone(), true, configs),
        };
        let out = h.run(&task()).await.expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(message(&out).contains("allowed-types"), "{:?}", out.message);
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// **A refused run whose container already exists is adopted and
    /// finishes**; with nothing to adopt the same refusal fails, and `run`
    /// (create) is never called either way.
    #[tokio::test]
    async fn a_refused_run_adopts_its_existing_container_and_never_creates_one() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        *spy.adoptable.lock().unwrap() = Some(ok(serde_json::json!({"size": 1})));
        let h = Kestra {
            type_id: QUERY,
            rt: runtime(spy.clone(), false, PluginConfigurations::default()),
        };
        let out = h.run(&task()).await.expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(out.outputs, Some(serde_json::json!({"size": 1})));
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 1);
        assert!(spy.seen.lock().unwrap().is_empty(), "`run` never called");

        // An admitted run is run, never adopted.
        let spy = Spy::ok(ok(serde_json::json!({})));
        handler(spy.clone()).run(&task()).await.expect("ran");
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 0);
        assert_eq!(spy.seen.lock().unwrap().len(), 1);
    }

    /// **An adopted run is masked too**: the lines a re-attached holder reads
    /// from the existing container go through the same masked log.
    #[tokio::test]
    async fn an_adopted_runs_lines_are_masked() {
        let spy = Spy::ok(ok(serde_json::json!({}))).streaming(vec![(
            false,
            format!("INFO still connected with {PASSWORD}"),
        )]);
        *spy.adoptable.lock().unwrap() = Some(ok(serde_json::json!({})));
        let h = Kestra {
            type_id: QUERY,
            rt: runtime(spy.clone(), false, PluginConfigurations::default()),
        };
        let (mut t, rec) = recorded(QUERY, spec());
        t.render = task().render;
        let out = h.run(&t).await.expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 1);
        assert_eq!(
            rec.texts(),
            vec![(LogLevel::Info, "still connected with ******".to_string())]
        );
    }

    /// An unreadable feature state is egret's outage: `Err`, no runner call.
    #[tokio::test]
    async fn an_unreadable_feature_state_is_a_worker_failure_and_calls_no_runner() {
        #[derive(Debug)]
        struct Down;
        #[async_trait::async_trait]
        impl FeatureState for Down {
            async fn containers_enabled(&self, _: uuid::Uuid) -> Result<bool> {
                Ok(true)
            }
            async fn kestra_plugins_enabled(&self, _: uuid::Uuid) -> Result<bool> {
                Err(egret_core::Error::Internal(anyhow::anyhow!(
                    "database down"
                )))
            }
        }
        let spy = Spy::ok(ok(serde_json::json!({})));
        let mut rt = runtime(spy.clone(), true, PluginConfigurations::default());
        Arc::get_mut(&mut rt).expect("unshared").features = Arc::new(Down);
        let h = Kestra { type_id: QUERY, rt };
        assert!(h.run(&task()).await.is_err());
        assert!(spy.seen.lock().unwrap().is_empty());
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 0);
    }

    /// A property publish refuses is refused at run, by name, before any
    /// secret is read or any runner called (a draft that skipped publish).
    ///
    /// W9a Task 9: this used `fetchType: STORE`, which publish no longer
    /// refuses; the replacement proving STORE runs is
    /// [`fetch_type_store_runs_and_its_file_is_indexed`]. An unknown property
    /// keeps the run-time refusal tested.
    #[tokio::test]
    async fn a_property_publish_refuses_is_refused_at_run() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let h = handler(spy.clone());
        let mut s = spec();
        s["sqll"] = serde_json::json!("SELECT 2");
        let out = h.run(&input(QUERY, s)).await.expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(message(&out).contains("'sqll'"), "{:?}", out.message);
        assert!(spy.seen.lock().unwrap().is_empty());
        assert_eq!(*spy.adopt_calls.lock().unwrap(), 0);
    }

    /// A secret that does not exist fails the task naming it, with no
    /// runner call.
    #[tokio::test]
    async fn a_missing_secret_fails_the_task_and_calls_no_runner() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let mut s = spec();
        s["password"] = serde_json::json!("{{ secret('nope') }}");
        let out = handler(spy.clone())
            .run(&input(QUERY, s))
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(message(&out).contains("'nope'"), "{:?}", out.message);
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    // ---- the commands --------------------------------------------------------

    /// **The host image, `exec java`, and `task.json` as the only input.**
    #[tokio::test]
    async fn the_commands_run_the_host_image_with_exec_java_and_task_json_as_the_only_input() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let mut s = spec();
        // Keys egret's Task model reads never reach the plugin.
        s["timeout"] = serde_json::json!("PT5S");
        s["retry"] = serde_json::json!({"type": "constant", "maxAttempts": 2});
        let out = handler(spy.clone())
            .run(&TaskInput { spec: s, ..task() })
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        assert_eq!(cmds.container_image, IMAGE);
        assert_eq!(
            cmds.argv(),
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "exec java -Xmx384m -cp '/opt/egret-plugin-host/lib/*:/opt/egret-plugin-host/\
                 plugins/*' io.kestra.core.runners.EgretPluginHost run \"$WORKING_DIR/task.json\""
                    .to_string(),
            ]
        );
        assert!(cmds.argv().join(" ").contains("exec java -Xmx384m"));
        assert_eq!(
            cmds.input_files.keys().collect::<Vec<_>>(),
            vec![TASK_JSON],
            "task.json is the only input"
        );
        assert!(cmds.output_files.is_empty());
        assert!(cmds.host_protocol);
        assert_eq!(
            cmds.env,
            BTreeMap::from([(ENV_KILL_WAIT_MS.to_string(), "13000".to_string())]),
            "the one env var is the host's kill wait: grace 15 s less 2 s"
        );
        // task.json: the FQCN without `kestra.`, the RENDERED properties,
        // and nothing egret's Task model consumes.
        let doc = task_json(&cmds);
        assert_eq!(doc["type"], FQCN);
        let mut keys: Vec<&String> = doc.as_object().expect("a map").keys().collect();
        keys.sort();
        // No internal storage in this runtime: nothing staged, no `baseUri`,
        // nothing collected (W9a Task 9); no namespace file (W9b Task 10).
        assert_eq!(
            keys,
            vec!["mask", "namespaceFiles", "properties", "storage", "type"]
        );
        assert_eq!(doc["storage"], serde_json::json!({}));
        assert_eq!(doc["namespaceFiles"], serde_json::json!([]));
        assert!(!cmds.output_directory);
        assert_eq!(cmds.collect_to, None);
        assert!(cmds.stored_inputs.is_empty());
        assert_eq!(
            doc["properties"],
            serde_json::json!({
                "url": "jdbc:postgresql://pg:5432/w5b",
                "username": "w5b",
                "password": PASSWORD,
                "sql": "SELECT 1 AS one",
                "fetchType": "FETCH_ONE",
            })
        );
        // No rendered property value in the env.
        let props = doc["properties"].as_object().expect("a map");
        for v in props.values().filter_map(Value::as_str) {
            assert!(
                cmds.env.values().all(|e| !e.contains(v)),
                "{v} is in the env"
            );
        }
    }

    /// The configured heap and grace reach the command and the env.
    #[tokio::test]
    async fn the_configured_heap_and_grace_reach_the_command() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        let mut rt = runtime(spy.clone(), true, PluginConfigurations::default());
        {
            let rt = Arc::get_mut(&mut rt).expect("unshared");
            rt.host.jvm_max_heap = "1g".into();
            rt.host.kill_grace_s = 30;
        }
        Kestra { type_id: QUERY, rt }
            .run(&task())
            .await
            .expect("ran");
        let cmds = spy.only();
        assert!(
            cmds.argv()[2].starts_with("exec java -Xmx1g -cp "),
            "{:?}",
            cmds.argv()
        );
        assert_eq!(cmds.env[ENV_KILL_WAIT_MS], "28000");
    }

    /// **A secret is in `task.json` and nowhere in the argv, the env or the
    /// image**; and the log line and the FAILED reason that echo it are
    /// masked.
    #[tokio::test]
    async fn a_secret_is_in_task_json_and_nowhere_in_the_commands_or_env() {
        let spy = Spy::ok(CommandsResult {
            exit_code: 3,
            host_failure: Some(HostFailure {
                code: 3,
                message: format!("PSQLException: password {PASSWORD} rejected"),
            }),
            ..CommandsResult::default()
        })
        .streaming(vec![
            (false, format!("INFO connecting with {PASSWORD}")),
            (true, format!("stderr {PASSWORD}")),
        ]);
        let (mut t, rec) = recorded(QUERY, spec());
        t.render = task().render;
        let out = handler(spy.clone()).run(&t).await.expect("no worker error");

        let cmds = spy.only();
        assert!(
            cmds.input_files[TASK_JSON].contains(PASSWORD),
            "the one place the value goes"
        );
        assert!(!cmds.argv().join("\n").contains(PASSWORD));
        assert!(cmds
            .env
            .iter()
            .all(|(k, v)| !k.contains(PASSWORD) && !v.contains(PASSWORD)));
        assert!(!cmds.container_image.contains(PASSWORD));
        // Fix round 1, I1: the host is told which values to mask on its own
        // stdout -- the same file, so no new place holds them.
        assert_eq!(task_json(&cmds)["mask"], serde_json::json!([PASSWORD]));

        assert_eq!(
            rec.texts(),
            vec![
                (LogLevel::Info, "connecting with ******".to_string()),
                (LogLevel::Error, "stderr ******".to_string()),
            ]
        );
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(
            message(&out),
            format!("{QUERY}: the plugin failed: PSQLException: password ****** rejected")
        );
    }

    /// A runner failure's text is masked too.
    #[tokio::test]
    async fn a_runner_failure_is_masked() {
        let spy = Spy::ok(CommandsResult {
            exit_code: 0,
            failure: Some(format!("outputs over the cap near {PASSWORD}")),
            ..CommandsResult::default()
        });
        let out = handler(spy).run(&task()).await.expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(!message(&out).contains(PASSWORD), "{:?}", out.message);
        assert!(message(&out).contains("******"), "{:?}", out.message);

        let spy = Spy::new(Err(egret_core::Error::InvalidInput(format!(
            "no such image near {PASSWORD}"
        ))));
        let out = handler(spy).run(&task()).await.expect("a task failure");
        assert_eq!(out.state, RunState::Failed);
        assert!(!message(&out).contains(PASSWORD), "{:?}", out.message);
    }

    // ---- the outcome ---------------------------------------------------------

    /// The host's `namespaceKv()` reason, as `ShimRunContext` throws it.
    const NAMESPACE_KV_REASON: &str = "namespaceKv(): egret gives flows the KV store through the \
        `io.kestra.plugin.core.kv` tasks and the `kv()` function, not through a plugin's \
        RunContext; use a `core.kv` task before or after this one.";

    /// **Each host exit code is a named reason.**
    #[tokio::test]
    async fn exit_codes_map_to_named_reasons() {
        let cases: [(i32, Option<&str>, String); 6] = [
            (
                2,
                Some("invalid io.kestra.plugin.jdbc.postgresql.Query: sql: must not be null"),
                format!(
                    "{QUERY}: the plugin host refused the task: invalid \
                     io.kestra.plugin.jdbc.postgresql.Query: sql: must not be null"
                ),
            ),
            (
                3,
                Some("org.postgresql.util.PSQLException: boom"),
                format!("{QUERY}: the plugin failed: org.postgresql.util.PSQLException: boom"),
            ),
            (
                4,
                Some(NAMESPACE_KV_REASON),
                format!(
                    "{QUERY}: the plugin needs a Kestra service egret does not provide to \
                     plugins: {NAMESPACE_KV_REASON}"
                ),
            ),
            (
                4,
                Some("decrypt(): egret has no secret key"),
                format!(
                    "{QUERY}: the plugin needs a Kestra service egret does not provide to \
                     plugins: decrypt(): egret has no secret key"
                ),
            ),
            (
                3,
                None,
                format!("{QUERY}: the plugin failed: (the host printed no reason)"),
            ),
            (
                137,
                None,
                format!("{QUERY}: the plugin host exited with 137"),
            ),
        ];
        for (code, said, want) in cases {
            let spy = Spy::ok(CommandsResult {
                exit_code: code,
                host_failure: said.map(|m| HostFailure {
                    code,
                    message: m.to_string(),
                }),
                ..CommandsResult::default()
            });
            let out = handler(spy).run(&task()).await.expect("no worker error");
            assert_eq!(out.state, RunState::Failed, "exit {code}");
            assert_eq!(message(&out), want, "exit {code}");
            assert_eq!(
                out.outputs, None,
                "exit {code}: a plugin that threw has no Output"
            );
        }
    }

    /// **Exit 0: the `outputs` marker's map is the outputs, as it is** --
    /// no `exitCode`/`vars` wrapping, no masking.
    #[tokio::test]
    async fn outputs_are_the_markers_outputs_as_they_are() {
        let vars = serde_json::json!({
            "row": {"one": 1, "pw": PASSWORD},
            "size": 1,
        });
        let spy = Spy::ok(ok(vars.clone()));
        let out = handler(spy).run(&task()).await.expect("no worker error");
        assert_eq!(out.state, RunState::Success);
        assert_eq!(out.outputs, Some(vars), "not masked, spec §12");
        assert_eq!(out.message, None);
    }

    // ---- cancel, release, the image, registration ---------------------------

    /// **`cancel` asks the runner with the configured grace**, not W5a's 0.
    #[tokio::test]
    async fn cancel_passes_the_configured_grace() {
        let spy = Spy::ok(CommandsResult::default());
        let h = handler(spy.clone());
        h.cancel(&task()).await.expect("cancelled");
        let mut rt = runtime(spy.clone(), true, PluginConfigurations::default());
        Arc::get_mut(&mut rt).expect("unshared").host.kill_grace_s = 7;
        Kestra { type_id: QUERY, rt }
            .cancel(&task())
            .await
            .expect("cancelled");
        assert_eq!(
            spy.cancel_graces.lock().unwrap().as_slice(),
            [Duration::from_secs(15), Duration::from_secs(7)]
        );
        assert_eq!(
            spy.cancelled.lock().unwrap().as_slice(),
            [
                crate::worker::commands::container_name(&task()),
                crate::worker::commands::container_name(&task()),
            ]
        );
    }

    /// `release` delegates with the configured runner's defaults; a task that
    /// does not parse never reached a runner.
    #[tokio::test]
    async fn release_delegates_to_the_configured_runner() {
        let spy = Spy::ok(CommandsResult::default());
        let h = handler(spy.clone());
        h.release(&task()).await.expect("released");
        h.release(&input(QUERY, serde_json::json!({"sqll": "x"})))
            .await
            .expect("nothing to release");
        assert_eq!(
            spy.released.lock().unwrap().as_slice(),
            [(crate::worker::commands::container_name(&task()), true)]
        );
    }

    /// **The host image is not matched against `allowed-images`**: no entry
    /// anywhere (which allows NO image to a script task) and an entry that
    /// allows only `python:*` both run the host image.
    #[tokio::test]
    async fn the_host_image_is_not_matched_against_allowed_images() {
        for configs in [
            PluginConfigurations::default(),
            PluginConfigurations(vec![PluginConfiguration {
                r#type: crate::scripts::DOCKER_RUNNER.to_string(),
                values: PluginConfigurationValues {
                    allowed_images: Some(vec!["python:*".into()]),
                    allowed_types: None,
                },
            }]),
        ] {
            let spy = Spy::ok(ok(serde_json::json!({})));
            let h = Kestra {
                type_id: QUERY,
                rt: runtime(spy.clone(), true, configs),
            };
            let out = h.run(&task()).await.expect("no worker error");
            assert_eq!(out.state, RunState::Success, "{:?}", out.message);
            assert_eq!(spy.only().container_image, IMAGE);
        }
    }

    /// **Registration**: exactly one key per catalog class, `kestra.<FQCN>`,
    /// each serving a plugin handler named by that key.
    #[test]
    fn register_serves_every_catalog_type_with_the_prefix_and_nothing_else() {
        let mut reg = Handlers::new();
        register(
            &mut reg,
            runtime(
                Spy::ok(CommandsResult::default()),
                false,
                PluginConfigurations::default(),
            ),
        );
        let want: Vec<String> = plugins::catalog()
            .classes
            .keys()
            .map(|fqcn| format!("kestra.{fqcn}"))
            .collect();
        assert_eq!(
            want.len(),
            9,
            "the three jars' nine runnable classes (W9a Task 9 re-admitted Batch ×2, CopyIn, CopyOut)"
        );
        assert_eq!(
            reg.type_ids(),
            want.iter().map(String::as_str).collect::<Vec<_>>()
        );
        for id in reg.type_ids() {
            assert_eq!(reg.get(id).expect("served").type_id(), id);
            assert!(crate::spec::registry::known_type(id).is_none());
        }
    }

    // ---- the storage bridge (W9a Task 9) -------------------------------------

    const COPY_IN: &str = "kestra.io.kestra.plugin.jdbc.postgresql.CopyIn";
    const WS: uuid::Uuid = uuid::Uuid::from_u128(0x5eed);
    const EXEC: uuid::Uuid = uuid::Uuid::from_u128(0xe8ec);
    const RUN: uuid::Uuid = uuid::Uuid::from_u128(0x7a5c);

    fn mem_files() -> Arc<crate::storage::FileStore> {
        Arc::new(crate::storage::FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps: crate::storage::Caps::default(),
            retention_days: 30,
        })
    }

    /// The handler for `type_id` over `spy`, with internal storage `files`.
    fn stored(
        type_id: &'static str,
        spy: Arc<Spy>,
        files: Option<Arc<crate::storage::FileStore>>,
    ) -> Kestra {
        let mut rt = runtime(spy, true, PluginConfigurations::default());
        Arc::get_mut(&mut rt).expect("unshared").files = files;
        Kestra { type_id, rt }
    }

    /// This run's own prefix: execution [`EXEC`], task `load`, run [`RUN`].
    fn prefix() -> String {
        crate::storage::uri::task_prefix("w9a.plugins", "pg-storage", EXEC, "load", RUN)
            .expect("a prefix")
    }

    /// A file another task of this execution wrote.
    fn upstream(name: &str) -> String {
        format!(
            "kestra:///w9a/plugins/pg-storage/executions/{EXEC}/tasks/q/{}/{name}",
            uuid::Uuid::from_u128(0xabc)
        )
    }

    fn indexed(uri: &str, size: u64) -> crate::storage::IndexedFile {
        crate::storage::IndexedFile {
            uri: uri.to_string(),
            execution_id: EXEC,
            task_run_id: Some(uuid::Uuid::from_u128(0xabc)),
            size,
        }
    }

    /// A run of `type_id` with `spec`, whose render has `outputs.q.{a,b}` =
    /// the upstream files `a.ion` and `b.ion`, the flow, and a snapshot
    /// resolving `a.ion` to `a` and `b.ion` to `b`.
    fn stored_task(
        files: &Arc<crate::storage::FileStore>,
        type_id: &str,
        spec: Value,
        a: std::result::Result<crate::storage::IndexedFile, crate::storage::Unreadable>,
        b: std::result::Result<crate::storage::IndexedFile, crate::storage::Unreadable>,
    ) -> TaskInput {
        let snapshot = crate::storage::scope::Snapshot::from_parts(
            BTreeMap::from([(upstream("a.ion"), a), (upstream("b.ion"), b)]),
            files.clone(),
            WS,
            Some(prefix()),
        );
        let mut cx = crate::runners::RenderContext::from_json_object(&serde_json::json!({
            "inputs": {"n": 1},
            "outputs": {"q": {"a": upstream("a.ion"), "b": upstream("b.ion")}},
        }))
        .expect("an object")
        .with_files(snapshot.into_scope());
        cx.insert(
            "flow",
            serde_json::json!({"namespace": "w9a.plugins", "id": "pg-storage"}),
        );
        TaskInput {
            workspace_id: WS,
            execution_id: EXEC,
            run_id: RUN,
            task_id: "load".into(),
            render: cx,
            ..input(type_id, spec)
        }
    }

    /// **Only WHOLE `kestra:///` string leaves are staged**, each distinct
    /// URI once, at `.egret/storage/<n>`: `parameters.a` and
    /// `parameters.b[1]` name `a.ion` (staged once); `sql` QUOTES `b.ion`
    /// inside its text, which is not a file read and is not staged (W9a spec
    /// §5's ambiguity, resolved and reported) -- though `b.ion` is readable.
    /// Mutation (b) stages substrings: `sql` is then looked up whole and the
    /// run fails, killing this test.
    #[tokio::test]
    async fn the_staged_set_is_exactly_the_whole_kestra_leaves() {
        let files = mem_files();
        let spy = Spy::ok(ok(serde_json::json!({"size": 1})));
        let t = stored_task(
            &files,
            QUERY,
            serde_json::json!({
                "url": "jdbc:postgresql://pg:5432/w9a",
                "sql": "SELECT '{{ outputs.q.b }}' AS u",
                "parameters": {"a": "{{ outputs.q.a }}", "b": ["x", "{{ outputs.q.a }}"]},
                "fetchType": "FETCH_ONE",
            }),
            Ok(indexed(&upstream("a.ion"), 7)),
            Ok(indexed(&upstream("b.ion"), 9)),
        );
        let out = stored(QUERY, spy.clone(), Some(files.clone()))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        assert_eq!(
            cmds.stored_inputs,
            BTreeMap::from([(
                ".egret/storage/0".to_string(),
                StoredInput {
                    uri: upstream("a.ion"),
                    object_uri: files.object_uri(WS, &upstream("a.ion")).unwrap(),
                    size: 7,
                }
            )])
        );
        let doc = task_json(&cmds);
        assert_eq!(
            doc["storage"],
            serde_json::json!({ upstream("a.ion"): ".egret/storage/0" })
        );
        assert_eq!(
            doc["properties"]["sql"],
            format!("SELECT '{}' AS u", upstream("b.ion")),
            "the quoted URI reaches the plugin as text"
        );
    }

    /// **Nothing is staged for KV (W9c Task 7):** no `task.json` key names
    /// KV, in the runtime without internal storage.
    #[tokio::test]
    async fn task_json_has_no_kv_key() {
        let spy = Spy::ok(ok(serde_json::json!({})));
        handler(spy.clone())
            .run(&task())
            .await
            .expect("no worker error");
        let doc = task_json(&spy.only());
        let keys: Vec<String> = doc.as_object().expect("a map").keys().cloned().collect();
        assert!(
            keys.iter().all(|k| !k.to_lowercase().contains("kv")),
            "{keys:?}"
        );
        assert_eq!(keys.len(), 5, "{keys:?}");
    }

    /// **Nothing is staged for KV, with internal storage either (W9c Task 7):**
    /// `baseUri` and `context` are present, and the key set is exactly the
    /// known one.
    #[tokio::test]
    async fn task_json_with_storage_has_no_kv_key() {
        let files = mem_files();
        let spy = Spy::ok(ok(serde_json::json!({"rowCount": 2})));
        let t = stored_task(
            &files,
            COPY_IN,
            serde_json::json!({
                "url": "jdbc:postgresql://pg:5432/w9a",
                "table": "dst",
                "from": "{{ outputs.q.a }}",
            }),
            Ok(indexed(&upstream("a.ion"), 7)),
            Ok(indexed(&upstream("b.ion"), 9)),
        );
        let out = stored(COPY_IN, spy.clone(), Some(files.clone()))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let doc = task_json(&spy.only());
        let mut keys: Vec<&String> = doc.as_object().expect("a map").keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "baseUri",
                "context",
                "mask",
                "maxTaskBytes",
                "namespaceFiles",
                "properties",
                "storage",
                "type"
            ]
        );
        assert!(keys.iter().all(|k| !k.to_lowercase().contains("kv")));
    }

    /// **`task.json` holds `storage`, `baseUri`, `maxTaskBytes` and
    /// `context`**, the context's `executionId` the run's OWN execution;
    /// and the runner collects `.egret/out` verbatim under `baseUri`.
    #[tokio::test]
    async fn task_json_holds_the_storage_keys_and_the_runner_collects_egret_out() {
        let files = mem_files();
        let spy = Spy::ok(ok(serde_json::json!({"rowCount": 2})));
        let t = stored_task(
            &files,
            COPY_IN,
            serde_json::json!({
                "url": "jdbc:postgresql://pg:5432/w9a",
                "table": "dst",
                "from": "{{ outputs.q.a }}",
            }),
            Ok(indexed(&upstream("a.ion"), 7)),
            Ok(indexed(&upstream("b.ion"), 9)),
        );
        let out = stored(COPY_IN, spy.clone(), Some(files.clone()))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        let doc = task_json(&cmds);
        let mut keys: Vec<&String> = doc.as_object().expect("a map").keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "baseUri",
                "context",
                "mask",
                "maxTaskBytes",
                "namespaceFiles",
                "properties",
                "storage",
                "type"
            ]
        );
        assert_eq!(doc["baseUri"], prefix());
        assert_eq!(
            doc["maxTaskBytes"],
            crate::storage::Caps::default().max_task_bytes
        );
        assert_eq!(
            doc["context"],
            serde_json::json!({
                "namespace": "w9a.plugins",
                "flowId": "pg-storage",
                "executionId": EXEC.hyphenated().to_string(),
                "taskId": "load",
                "taskRunId": RUN.hyphenated().to_string(),
            })
        );
        assert_eq!(doc["properties"]["from"], upstream("a.ion"));
        assert_eq!(
            cmds.collect_to,
            Some(CollectTo {
                workspace: WS,
                task_prefix: prefix(),
                naming: Naming::UnderPrefixVerbatim,
            })
        );
        assert!(cmds.output_directory, "the runner collects `.egret/out`");
        assert_eq!(crate::worker::commands::OUTPUT_DIR_REL, ".egret/out");
        assert!(cmds.output_files.is_empty());
        // The staged file and the output directory do not collide.
        let tar = cmds.tar_sources().expect("no clash");
        assert!(tar.contains_key(".egret/storage/0"));
        assert!(tar.contains_key(".egret/out"));
    }

    /// **An unreadable URI fails the task at render, naming it and the
    /// property, and calls no runner** -- for each way the snapshot refuses.
    #[tokio::test]
    async fn an_unreadable_uri_fails_at_render_and_calls_no_runner() {
        let files = mem_files();
        let a = upstream("a.ion");
        for (resolution, needle) in [
            (
                Err(crate::storage::Unreadable::NotReadable { uri: a.clone() }),
                "may not read",
            ),
            (
                Err(crate::storage::Unreadable::Missing { uri: a.clone() }),
                "does not exist",
            ),
            // W9a Task 13: a pruned file fails the task with its message.
            (
                Err(crate::storage::Unreadable::Pruned {
                    uri: a.clone(),
                    retention_days: 30,
                }),
                "was deleted: files are kept 30 days",
            ),
        ] {
            let spy = Spy::ok(ok(serde_json::json!({})));
            let t = stored_task(
                &files,
                COPY_IN,
                serde_json::json!({
                    "url": "jdbc:postgresql://pg:5432/w9a",
                    "table": "dst",
                    "from": "{{ outputs.q.a }}",
                }),
                resolution,
                Ok(indexed(&upstream("b.ion"), 9)),
            );
            let out = stored(COPY_IN, spy.clone(), Some(files.clone()))
                .run(&t)
                .await
                .expect("the author's error, not a worker error");
            assert_eq!(out.state, RunState::Failed);
            let m = message(&out);
            assert!(m.starts_with(COPY_IN), "{m}");
            assert!(m.contains("property `from`"), "{m}");
            assert!(m.contains(&a), "{m}");
            assert!(m.contains(needle), "{m}");
            assert!(spy.seen.lock().unwrap().is_empty(), "no runner was called");
            assert_eq!(*spy.adopt_calls.lock().unwrap(), 0);
        }
        // A URI the snapshot never looked up (assembled by the render).
        let spy = Spy::ok(ok(serde_json::json!({})));
        let t = stored_task(
            &files,
            COPY_IN,
            serde_json::json!({
                "url": "jdbc:postgresql://pg:5432/w9a",
                "table": "dst",
                "from": "{{ outputs.q.a ~ '.x' }}",
            }),
            Ok(indexed(&upstream("a.ion"), 7)),
            Ok(indexed(&upstream("b.ion"), 9)),
        );
        let out = stored(COPY_IN, spy.clone(), Some(files.clone()))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            message(&out).contains("was not looked up"),
            "{:?}",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty());
        // And with no internal storage at all, naming `[storage]`.
        let spy = Spy::ok(ok(serde_json::json!({})));
        let t = stored_task(
            &files,
            COPY_IN,
            serde_json::json!({
                "url": "jdbc:postgresql://pg:5432/w9a",
                "table": "dst",
                "from": "{{ outputs.q.a }}",
            }),
            Ok(indexed(&upstream("a.ion"), 7)),
            Ok(indexed(&upstream("b.ion"), 9)),
        );
        let out = stored(COPY_IN, spy.clone(), None)
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(message(&out).contains("[storage]"), "{:?}", out.message);
        assert!(spy.seen.lock().unwrap().is_empty());
    }

    /// **`fetchType: STORE` runs** (the replacement for W5b's refusal): the
    /// host's outputs name the file it wrote under `baseUri`, the runner
    /// collected it, and it rides on the outcome to be indexed.
    #[tokio::test]
    async fn fetch_type_store_runs_and_its_file_is_indexed() {
        let files = mem_files();
        let uri = format!("{}3654590194353406031.ion", prefix());
        let spy = Spy::ok(CommandsResult {
            exit_code: 0,
            vars: serde_json::json!({"uri": uri, "size": 3})
                .as_object()
                .cloned()
                .unwrap(),
            output_files: BTreeMap::from([(
                "3654590194353406031.ion".to_string(),
                crate::storage::StoredFile {
                    uri: uri.clone(),
                    size: 96,
                },
            )]),
            ..CommandsResult::default()
        });
        let mut s = spec();
        s["fetchType"] = serde_json::json!("STORE");
        let t = stored_task(
            &files,
            QUERY,
            s,
            Ok(indexed(&upstream("a.ion"), 7)),
            Ok(indexed(&upstream("b.ion"), 9)),
        );
        let out = stored(QUERY, spy.clone(), Some(files))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(
            out.outputs,
            Some(serde_json::json!({"uri": uri, "size": 3}))
        );
        assert_eq!(
            out.files,
            vec![crate::storage::StoredFile { uri, size: 96 }]
        );
        assert_eq!(spy.seen.lock().unwrap().len(), 1, "the runner ran it");
    }

    /// **A host that returns a URI under this run's prefix that it never
    /// wrote fails the task** (W9a spec §5); a URI outside the prefix -- the
    /// staged input echoed back -- is not this run's to check. Mutation (c)
    /// bypasses the check for plugins.
    #[tokio::test]
    async fn an_output_naming_an_uncollected_file_fails_the_task() {
        let files = mem_files();
        let forged = format!("{}never-written.ion", prefix());
        for (vars, fails) in [
            (serde_json::json!({"uri": forged.clone(), "size": 1}), true),
            (serde_json::json!({"rows": [{"u": forged.clone()}]}), true),
            (serde_json::json!({"uri": upstream("a.ion")}), false),
        ] {
            let spy = Spy::ok(ok(vars.clone()));
            let t = stored_task(
                &files,
                QUERY,
                spec(),
                Ok(indexed(&upstream("a.ion"), 7)),
                Ok(indexed(&upstream("b.ion"), 9)),
            );
            let out = stored(QUERY, spy, Some(files.clone()))
                .run(&t)
                .await
                .expect("no worker error");
            if fails {
                assert_eq!(out.state, RunState::Failed, "{vars}");
                assert!(message(&out).contains(&forged), "{:?}", out.message);
                assert!(
                    message(&out).contains("never collected"),
                    "{:?}",
                    out.message
                );
            } else {
                assert_eq!(out.state, RunState::Success, "{vars}: {:?}", out.message);
            }
        }
        // Without internal storage the run's prefix still comes from the
        // render's scope, and the check still holds.
        let spy = Spy::ok(ok(serde_json::json!({"uri": forged.clone()})));
        let t = stored_task(
            &files,
            QUERY,
            spec(),
            Ok(indexed(&upstream("a.ion"), 7)),
            Ok(indexed(&upstream("b.ion"), 9)),
        );
        let out = stored(QUERY, spy, None)
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed, "{:?}", out.message);
    }

    /// **Fix round 1, I1: `context` goes with every task run of a flow,
    /// with or without internal storage**, so the host reaches
    /// `EgretStorage`'s own refusal naming `[storage]` on a write (Java:
    /// `without_internal_storage_a_plugin_write_fails_naming_storage`) rather
    /// than a false "egret sends it with every task".
    #[tokio::test]
    async fn without_internal_storage_task_json_still_carries_the_context() {
        let files = mem_files();
        let spy = Spy::ok(ok(serde_json::json!({"size": 1})));
        let t = stored_task(
            &files,
            QUERY,
            spec(),
            Ok(indexed(&upstream("a.ion"), 7)),
            Ok(indexed(&upstream("b.ion"), 9)),
        );
        let out = stored(QUERY, spy.clone(), None)
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        let doc = task_json(&cmds);
        assert_eq!(doc["context"]["executionId"], EXEC.hyphenated().to_string());
        assert_eq!(doc["context"]["flowId"], "pg-storage");
        assert!(
            doc.get("baseUri").is_none(),
            "no storage, no baseUri: {doc}"
        );
        assert!(doc.get("maxTaskBytes").is_none(), "{doc}");
        assert!(!cmds.output_directory);
        assert_eq!(cmds.collect_to, None);
    }

    /// **Fix round 1: every plugin run gets `.egret/tmp`**, a world-writable
    /// directory under the working directory for the host's scratch and its
    /// `.part` files -- with or without storage -- and a script run does not.
    #[tokio::test]
    async fn a_plugin_run_gets_its_scratch_directory_and_a_script_run_does_not() {
        use crate::storage::transfer::TarSource;
        let spy = Spy::ok(ok(serde_json::json!({})));
        handler(spy.clone()).run(&task()).await.expect("ran");
        let cmds = spy.only();
        let tar = cmds.tar_sources().expect("sources");
        assert!(
            matches!(
                tar.get(crate::worker::commands::SCRATCH_DIR_REL),
                Some(TarSource::Directory)
            ),
            "{:?}",
            tar.keys().collect::<Vec<_>>()
        );
        assert!(!tar.contains_key(crate::worker::commands::OUTPUT_DIR_REL));
        let script = TaskCommands {
            input_files: BTreeMap::from([("a".to_string(), "x".to_string())]),
            host_protocol: false,
            ..cmds.clone()
        };
        assert!(!script
            .tar_sources()
            .expect("sources")
            .contains_key(crate::worker::commands::SCRATCH_DIR_REL));
        // An input file inside it is refused, by name.
        let clash = TaskCommands {
            input_files: BTreeMap::from([(".egret/tmp/x".to_string(), "x".to_string())]),
            ..cmds
        };
        let e = clash.tar_sources().expect_err("a clash");
        assert!(e.to_string().contains(".egret/tmp/x"), "{e}");
    }

    /// **Fix round 1, M4: a URI the host returned percent-encoded differently
    /// from the file egret stored fails naming both**, so the author can see
    /// why "never collected" when the names look alike.
    #[tokio::test]
    async fn an_encoded_twin_of_a_collected_file_is_named_in_the_failure() {
        let files = mem_files();
        let stored_uri = format!("{}a b.ion", prefix());
        let returned = format!("{}a%20b.ion", prefix());
        let spy = Spy::ok(CommandsResult {
            exit_code: 0,
            vars: serde_json::json!({"uri": returned})
                .as_object()
                .cloned()
                .unwrap(),
            output_files: BTreeMap::from([(
                "a b.ion".to_string(),
                crate::storage::StoredFile {
                    uri: stored_uri.clone(),
                    size: 3,
                },
            )]),
            ..CommandsResult::default()
        });
        let t = stored_task(
            &files,
            QUERY,
            spec(),
            Ok(indexed(&upstream("a.ion"), 7)),
            Ok(indexed(&upstream("b.ion"), 9)),
        );
        let out = stored(QUERY, spy, Some(files))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        let m = message(&out);
        assert!(m.contains("never collected"), "{m}");
        assert!(m.contains("percent-encoded"), "{m}");
        assert!(m.contains(&stored_uri), "{m}");
        // The control: a URI with no collected twin gets no hint.
        assert!(encoded_twin(
            &serde_json::json!({"uri": format!("{}other.ion", prefix())}),
            &[crate::storage::StoredFile {
                uri: stored_uri,
                size: 3
            }]
        )
        .is_none());
    }

    /// **`base-uri-cases.json`, read by `plugin-host`'s
    /// `EgretStorageTest.storage_context_for_task_is_the_base_uri_egret_computes`**:
    /// egret's `uri::task_prefix` for five values per case, which Kestra's
    /// `StorageContext.forTask(...).getContextStorageURI()` must equal. This
    /// test WRITES the file when it differs and then fails, so a change to
    /// `task_prefix` cannot leave the Java side checking a stale copy.
    #[test]
    fn the_base_uri_fixture_is_what_egret_computes() {
        let cases = [
            ("w9a.plugins", "pg-storage", 0x1u128, "copy_out", 0x2u128),
            (
                "company.team.sub",
                "My_Flow.v2",
                0x3,
                "Task.With-Dots_1",
                0x4,
            ),
            ("a", "UPPER", 0x5, "UPPER_task", 0x6),
            ("w5b", "f", 0x7, "q", 0x8),
            ("x", "a__b--c..d", 0x9, "t-_-.x", 0xa),
            ("io.kestra.tests", "loop-iteration", 0xb, "store", 0xc),
        ];
        let rows: Vec<Value> = cases
            .iter()
            .map(|(ns, flow, exec, task, run)| {
                let (exec, run) = (uuid::Uuid::from_u128(*exec), uuid::Uuid::from_u128(*run));
                serde_json::json!({
                    "namespace": ns,
                    "flowId": flow,
                    "executionId": exec.hyphenated().to_string(),
                    "taskId": task,
                    "taskRunId": run.hyphenated().to_string(),
                    "baseUri": crate::storage::uri::task_prefix(ns, flow, exec, task, run)
                        .expect("a prefix"),
                })
            })
            .collect();
        let want = serde_json::to_string_pretty(&rows).expect("json") + "\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugin-host/src/test/resources/base-uri-cases.json");
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        if have != want {
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(&path, &want).expect("write the fixture");
            panic!(
                "{} was stale and has been rewritten; run `mvn -f plugin-host/pom.xml test` and \
                 commit it",
                path.display()
            );
        }
    }

    // ---- W9b Task 10: the plugin bridge reads namespace files ---------------

    use crate::storage::nsfiles::stage::testing::{self as ns, contents, write, OWN};

    /// The handler for `type_id` over `spy`, with internal storage `files`
    /// and namespace files resolved on `pool`.
    fn with_ns(
        type_id: &'static str,
        spy: Arc<Spy>,
        files: Arc<crate::storage::FileStore>,
        pool: Option<sqlx::PgPool>,
    ) -> Kestra {
        let mut rt = runtime(spy, true, PluginConfigurations::default());
        let r = Arc::get_mut(&mut rt).expect("unshared");
        r.files = Some(files);
        r.pool = pool;
        Kestra { type_id, rt }
    }

    /// A run of `type_id` of flow namespace `own` in workspace `ws`, whose
    /// render reaches namespace files on `pool` (`fileURI`) and whose
    /// snapshot resolves the execution file `upstream("a.ion")` (7 bytes).
    fn ns_plugin_task(
        files: &Arc<crate::storage::FileStore>,
        pool: &sqlx::PgPool,
        ws: uuid::Uuid,
        own: &str,
        type_id: &str,
        spec: Value,
    ) -> TaskInput {
        ns_plugin_task_rendering(files, pool, ws, own, own, type_id, spec)
    }

    /// [`ns_plugin_task`] whose render context says `flow.namespace` is
    /// `rendered`, while the execution's flow (the file scope's) is `own`
    /// (fix round 1, M2).
    fn ns_plugin_task_rendering(
        files: &Arc<crate::storage::FileStore>,
        pool: &sqlx::PgPool,
        ws: uuid::Uuid,
        own: &str,
        rendered: &str,
        type_id: &str,
        spec: Value,
    ) -> TaskInput {
        let snapshot = crate::storage::scope::Snapshot::from_parts(
            BTreeMap::from([(upstream("a.ion"), Ok(indexed(&upstream("a.ion"), 7)))]),
            files.clone(),
            ws,
            None,
        )
        .with_namespace_files(
            Some(own.to_string()),
            Some(Arc::new((*pool.connect_options()).clone())),
        );
        let mut cx = crate::runners::RenderContext::from_json_object(&serde_json::json!({
            "outputs": {"q": {"a": upstream("a.ion")}},
        }))
        .expect("an object")
        .with_files(snapshot.into_scope());
        cx.insert(
            "flow",
            serde_json::json!({"namespace": rendered, "id": "t10"}),
        );
        TaskInput {
            workspace_id: ws,
            execution_id: EXEC,
            run_id: RUN,
            task_id: "load".into(),
            render: cx,
            ..input(type_id, spec)
        }
    }

    fn query_with(parameters: Value) -> Value {
        serde_json::json!({
            "url": "jdbc:postgresql://pg:5432/w9b",
            "sql": "SELECT 'nsfile:///own.txt' AS a, 'kestra:///w9b/s1/_files/x.txt' AS b",
            "parameters": parameters,
            "fetchType": "FETCH_ONE",
        })
    }

    /// **Every WHOLE `nsfile://` or `kestra:///…/_files/…` leaf is resolved
    /// (authorize Read, any namespace of the workspace), staged at
    /// `.egret/storage/<n>` in W9a's numbering, and listed under
    /// `namespaceFiles` -- and NOT in `storage`** (the controller's ruling,
    /// item 10). A duplicate value is staged once; the URIs quoted in `sql`
    /// are text, not staged; the execution file keeps W9a's path.
    #[tokio::test(flavor = "multi_thread")]
    async fn namespace_file_values_are_staged_and_listed_in_namespace_files_not_storage() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        assert_eq!(write(&pool, &files, ws, OWN, "/own.txt", "own-v2").await, 2);
        files
            .store
            .put(
                &files.object_uri(ws, &upstream("a.ion")).unwrap(),
                b"{i:1}\n\n".to_vec(),
            )
            .await
            .expect("the execution file's object");
        let spy = Spy::ok(ok(serde_json::json!({"size": 1}))).staging(files.clone());
        let t = ns_plugin_task(
            &files,
            &pool,
            ws,
            OWN,
            QUERY,
            // `exec` first (properties keep their order): W9a's file takes
            // `.egret/storage/0`, so a namespace file numbered on its own
            // count would collide with it.
            query_with(serde_json::json!({
                "exec": "{{ outputs.q.a }}",
                "own": "nsfile:///own.txt",
                "again": "nsfile:///own.txt",
                "other": "nsfile://w9b.s2/sub/y.txt",
                "uri": "{{ fileURI('x.txt') }}",
                "first": "kestra:///w9b/stage/_files/own.txt",
                "kfile": "kestra:///w9b/s1/_files/only1.txt",
                "text": "x nsfile:///own.txt",
            })),
        );
        let out = with_ns(QUERY, spy.clone(), files.clone(), Some(pool.clone()))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        let doc = task_json(&cmds);
        // W9a's map: the execution file only.
        let exec_rel = doc["storage"][upstream("a.ion")]
            .as_str()
            .expect("the execution file is in `storage`")
            .to_string();
        assert_eq!(exec_rel, ".egret/storage/0", "the first leaf, first staged");
        assert_eq!(doc["storage"].as_object().unwrap().len(), 1, "{doc}");
        let listed = doc["namespaceFiles"].as_array().expect("a list");
        let by_uri: BTreeMap<&str, &Value> = listed
            .iter()
            .map(|e| (e["uri"].as_str().expect("a uri"), e))
            .collect();
        assert_eq!(listed.len(), 5, "each distinct value once: {listed:?}");
        let want = [
            ("nsfile:///own.txt", OWN, "/own.txt", 2, "own-v2"),
            ("nsfile://w9b.s2/sub/y.txt", "w9b.s2", "/sub/y.txt", 1, "y2"),
            (
                "kestra:///w9b/stage/_files/x.txt",
                OWN,
                "/x.txt",
                1,
                "from-stage",
            ),
            (
                "kestra:///w9b/stage/_files/own.txt",
                OWN,
                "/own.txt",
                1,
                "own",
            ),
            (
                "kestra:///w9b/s1/_files/only1.txt",
                "w9b.s1",
                "/only1.txt",
                1,
                "only1",
            ),
        ];
        let mut staged_paths = std::collections::BTreeSet::from([exec_rel.clone()]);
        for (uri, namespace, path, revision, body) in want {
            let e = by_uri
                .get(uri)
                .unwrap_or_else(|| panic!("{uri} listed: {listed:?}"));
            assert_eq!(e["namespace"], namespace, "{uri}");
            assert_eq!(e["path"], path, "{uri}");
            assert_eq!(e["revision"], revision, "{uri}");
            assert_eq!(e["size"], body.len(), "{uri}");
            let rel = e["staged"].as_str().expect("staged").to_string();
            assert!(rel.starts_with(".egret/storage/"), "{rel}");
            assert!(staged_paths.insert(rel.clone()), "{rel} collides");
            let input = &cmds.stored_inputs[&rel];
            assert_eq!(input.uri, uri);
            assert_eq!(contents(&files, &input.object_uri).await, body, "{uri}");
            assert!(
                doc["storage"].get(uri).is_none(),
                "{uri} is not in `storage`"
            );
        }
        // One numbering, nothing else staged: the five namespace files and the
        // execution file at .egret/storage/0..=5.
        assert_eq!(
            cmds.stored_inputs
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            staged_paths
        );
        assert_eq!(
            staged_paths,
            (0..6).map(|n| format!(".egret/storage/{n}")).collect()
        );
        assert_eq!(
            doc["properties"]["sql"],
            "SELECT 'nsfile:///own.txt' AS a, 'kestra:///w9b/s1/_files/x.txt' AS b",
            "the quoted URIs reach the plugin as text"
        );
        assert_eq!(
            doc["properties"]["parameters"]["text"],
            "x nsfile:///own.txt"
        );
        cmds.tar_sources().expect("no clash between staged files");
    }

    /// **A namespace URI inside SQL text is not staged** (W9a's whole-leaf
    /// rule): nothing is resolved, no connection is needed, the run succeeds.
    /// Mutation (b) stages substrings: the SQL leaf is then looked up whole
    /// and the run fails.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_namespace_uri_inside_sql_text_is_not_staged() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        let spy = Spy::ok(ok(serde_json::json!({"size": 1})));
        let t = ns_plugin_task(
            &files,
            &pool,
            ws,
            OWN,
            QUERY,
            serde_json::json!({
                "url": "jdbc:postgresql://pg:5432/w9b",
                "sql": "SELECT 'nsfile:///own.txt' AS u",
                "fetchType": "FETCH_ONE",
            }),
        );
        // No pool: a value that needed one would fail naming `[storage]`.
        let out = with_ns(QUERY, spy.clone(), files.clone(), None)
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let doc = task_json(&spy.only());
        assert_eq!(doc["namespaceFiles"], serde_json::json!([]));
        assert_eq!(doc["storage"], serde_json::json!({}));
    }

    /// **A refused namespace fails the task at render, before any runner,
    /// naming the property and the rule** -- a `_files` segment, an empty
    /// segment, and a running flow whose own namespace is refused. Mutation
    /// (c) skips `authorize`'s check of the target: the refusal becomes a
    /// not-found and the rule is no longer named.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_namespace_fails_the_task_before_any_runner() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        for (own, value, needle) in [
            (OWN, "nsfile://w9b._files/x.txt", "has a segment '_files'"),
            (OWN, "nsfile://w9b..s1/x.txt", "has an empty segment"),
            (
                "w9b..t10",
                "nsfile://w9b.s1/x.txt",
                "the running flow's namespace",
            ),
        ] {
            let spy = Spy::ok(ok(serde_json::json!({})));
            let t = ns_plugin_task(
                &files,
                &pool,
                ws,
                own,
                COPY_IN,
                serde_json::json!({
                    "url": "jdbc:postgresql://pg:5432/w9b",
                    "table": "dst",
                    "from": value,
                }),
            );
            let out = with_ns(COPY_IN, spy.clone(), files.clone(), Some(pool.clone()))
                .run(&t)
                .await
                .expect("the author's error, not a worker error");
            assert_eq!(out.state, RunState::Failed, "{value}");
            let m = message(&out);
            assert!(m.starts_with(COPY_IN), "{m}");
            assert!(m.contains("property `from`"), "{m}");
            assert!(m.contains(needle), "{value}: {m}");
            assert!(spy.seen.lock().unwrap().is_empty(), "no runner was called");
            assert_eq!(*spy.adopt_calls.lock().unwrap(), 0);
        }
    }

    /// **A missing path fails the task naming it, before any runner** -- also
    /// a latest read of a soft-deleted path (R1), while the `kestra://` URI of
    /// that path's revision IS staged (R1), and another workspace's identical
    /// file is not seen.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_missing_namespace_file_fails_naming_it_before_any_runner() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        let elsewhere = ns::workspace(&pool).await;
        write(&pool, &files, elsewhere, OWN, "/theirs.txt", "theirs").await;
        ns::delete(&pool, ws, OWN, "/own.txt").await;
        for (value, needle) in [
            ("nsfile:///nope.txt", "/nope.txt"),
            ("kestra:///w9b/stage/_files/nope.txt", "/nope.txt"),
            ("nsfile:///own.txt", "/own.txt"),
            ("nsfile:///theirs.txt", "/theirs.txt"),
        ] {
            let spy = Spy::ok(ok(serde_json::json!({})));
            let t = ns_plugin_task(
                &files,
                &pool,
                ws,
                OWN,
                COPY_IN,
                serde_json::json!({"url": "jdbc:postgresql://pg:5432/w9b", "table": "dst", "from": value}),
            );
            let out = with_ns(COPY_IN, spy.clone(), files.clone(), Some(pool.clone()))
                .run(&t)
                .await
                .expect("the author's error");
            assert_eq!(out.state, RunState::Failed, "{value}");
            let m = message(&out);
            assert!(
                m.contains("property `from`") && m.contains(needle),
                "{value}: {m}"
            );
            assert!(spy.seen.lock().unwrap().is_empty(), "no runner was called");
        }
        // R1: the revision a kestra:// URI names is readable though deleted.
        let spy = Spy::ok(ok(serde_json::json!({}))).staging(files.clone());
        let t = ns_plugin_task(
            &files,
            &pool,
            ws,
            OWN,
            COPY_IN,
            serde_json::json!({
                "url": "jdbc:postgresql://pg:5432/w9b",
                "table": "dst",
                "from": "kestra:///w9b/stage/_files/own.txt",
            }),
        );
        let out = with_ns(COPY_IN, spy.clone(), files.clone(), Some(pool.clone()))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let doc = task_json(&spy.only());
        assert_eq!(doc["namespaceFiles"][0]["path"], "/own.txt");
        assert_eq!(doc["namespaceFiles"][0]["staged"], ".egret/storage/0");
    }

    /// **An outage while resolving or streaming a namespace file is egret's**:
    /// a worker `Err`, no task failure, no attempt spent -- the database (a
    /// closed pool, and a lookup that fails past the acquire; no runner
    /// called) and the store (the runner's staging).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_namespace_file_outage_is_a_worker_error() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        let spec = serde_json::json!({
            "url": "jdbc:postgresql://pg:5432/w9b",
            "table": "dst",
            "from": "nsfile:///own.txt",
        });
        let t = ns_plugin_task(&files, &pool, ws, OWN, COPY_IN, spec.clone());

        let closed = ns::pool().await.expect("a pool");
        closed.close().await;
        let spy = Spy::ok(ok(serde_json::json!({})));
        let err = with_ns(COPY_IN, spy.clone(), files.clone(), Some(closed))
            .run(&t)
            .await
            .expect_err("an unreachable database is egret's outage");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        assert!(err.to_string().contains("staging namespace files"), "{err}");
        assert!(spy.seen.lock().unwrap().is_empty(), "no runner call");

        // A connection is taken but the lookup itself fails (the server's
        // `postgres` database has no `workflow` schema): `resolve`'s own
        // database error, past the acquire.
        let broken = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with((*pool.connect_options()).clone().database("postgres"))
            .await
            .expect("the same server's `postgres` database");
        let spy = Spy::ok(ok(serde_json::json!({})));
        let err = with_ns(COPY_IN, spy.clone(), files.clone(), Some(broken.clone()))
            .run(&t)
            .await
            .expect_err("a failed lookup is egret's outage");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        assert!(err.to_string().contains("staging namespace files"), "{err}");
        assert!(spy.seen.lock().unwrap().is_empty(), "no runner call");
        broken.close().await;

        let down = Arc::new(crate::storage::FileStore {
            store: Arc::new(crate::worker::handlers::script::tests::DownStore),
            ..(*files).clone()
        });
        let spy = Spy::ok(ok(serde_json::json!({}))).staging(down.clone());
        let t = ns_plugin_task(&down, &pool, ws, OWN, COPY_IN, spec.clone());
        let err = with_ns(COPY_IN, spy, down, Some(pool.clone()))
            .run(&t)
            .await
            .expect_err("an unreachable store is egret's outage");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");

        // The control: both up.
        let spy = Spy::ok(ok(serde_json::json!({}))).staging(files.clone());
        let t = ns_plugin_task(&files, &pool, ws, OWN, COPY_IN, spec);
        let out = with_ns(COPY_IN, spy, files, Some(pool))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
    }

    /// **The running namespace is the execution's flow's** (the file scope's
    /// `flow_namespace`, Task 4), never the rendered `{{ flow.namespace }}`
    /// (fix round 1, M2): here the context says `w9b.s2`, the execution's
    /// flow is `w9b.stage`, and `nsfile:///x.txt` and `nsfile:///own.txt`
    /// are `w9b.stage`'s. Reading the namespace from the render would stage
    /// `w9b.s2`'s `x.txt` and miss `own.txt`.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_running_namespace_is_the_executions_flow_not_the_rendered_one() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        ns::setup(&pool, &files, ws).await;
        let spy = Spy::ok(ok(serde_json::json!({"size": 1})));
        let t = ns_plugin_task_rendering(
            &files,
            &pool,
            ws,
            OWN,
            "w9b.s2",
            QUERY,
            query_with(serde_json::json!({
                "x": "nsfile:///x.txt",
                "own": "nsfile:///own.txt",
            })),
        );
        let out = with_ns(QUERY, spy.clone(), files.clone(), Some(pool))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        let cmds = spy.only();
        let doc = task_json(&cmds);
        let listed = doc["namespaceFiles"].as_array().expect("a list");
        assert_eq!(listed.len(), 2, "{listed:?}");
        for (e, body) in listed.iter().zip(["from-stage", "own"]) {
            assert_eq!(e["namespace"], OWN, "{e}");
            let rel = e["staged"].as_str().unwrap();
            assert_eq!(
                contents(&files, &cmds.stored_inputs[rel].object_uri).await,
                body
            );
        }
        assert_eq!(
            doc["context"]["namespace"], "w9b.s2",
            "context is the render's"
        );
    }

    /// **The staged namespace files are bounded by count
    /// (`max_namespace_files`), and every staged file -- execution files
    /// included (fix round 1, M1) -- by total bytes (`max_task_bytes`)**,
    /// each an author error naming its cap, before any runner; at the cap
    /// each passes.
    #[tokio::test(flavor = "multi_thread")]
    async fn namespace_files_are_bounded_by_count_and_bytes() {
        let Some(pool) = ns::pool().await else { return };
        let spec = query_with(serde_json::json!({
            "e": "{{ outputs.q.a }}",
            "a": "nsfile:///own.txt",
            "b": "nsfile:///x.txt",
        }));
        // The execution file (7) + own.txt "own" (3) + x.txt "from-stage"
        // (10) = 20 bytes; 2 namespace files. Fix round 1, M1: the byte cap
        // is the script handler's, over EVERY staged file -- 19 is refused
        // though the namespace files alone hold 13.
        for (count, bytes, refused) in [
            (1, 20, Some("workflow.storage.max_namespace_files")),
            (2, 19, Some("workflow.storage.max_task_bytes")),
            (2, 20, None),
        ] {
            let caps = crate::storage::Caps {
                max_namespace_files: count,
                max_task_bytes: bytes,
                ..crate::storage::Caps::default()
            };
            let files = ns::mem_files(caps);
            let ws = ns::workspace(&pool).await;
            ns::setup(&pool, &files, ws).await;
            let spy = Spy::ok(ok(serde_json::json!({})));
            let t = ns_plugin_task(&files, &pool, ws, OWN, QUERY, spec.clone());
            let out = with_ns(QUERY, spy.clone(), files, Some(pool.clone()))
                .run(&t)
                .await
                .expect("no worker error");
            match refused {
                Some(cap) => {
                    assert_eq!(out.state, RunState::Failed, "{count}/{bytes}");
                    assert!(message(&out).contains(cap), "{:?}", out.message);
                    assert!(spy.seen.lock().unwrap().is_empty(), "no runner call");
                }
                None => {
                    assert_eq!(out.state, RunState::Success, "{:?}", out.message);
                    assert_eq!(
                        task_json(&spy.only())["namespaceFiles"]
                            .as_array()
                            .unwrap()
                            .len(),
                        2
                    );
                }
            }
        }
    }

    /// Without a database, or outside a flow's execution, a namespace-file
    /// value fails the task naming why, before any runner.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_namespace_file_value_without_storage_or_a_flow_fails_naming_why() {
        let Some(pool) = ns::pool().await else { return };
        let files = ns::mem_files(crate::storage::Caps::default());
        let ws = ns::workspace(&pool).await;
        let spec = serde_json::json!({
            "url": "jdbc:postgresql://pg:5432/w9b",
            "table": "dst",
            "from": "nsfile:///own.txt",
        });
        let spy = Spy::ok(ok(serde_json::json!({})));
        let t = ns_plugin_task(&files, &pool, ws, OWN, COPY_IN, spec.clone());
        let out = with_ns(COPY_IN, spy.clone(), files.clone(), None)
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(message(&out).contains("[storage]"), "{:?}", out.message);
        assert!(spy.seen.lock().unwrap().is_empty());

        let spy = Spy::ok(ok(serde_json::json!({})));
        let t = TaskInput {
            workspace_id: ws,
            ..input(COPY_IN, spec)
        };
        let out = with_ns(COPY_IN, spy.clone(), files, Some(pool))
            .run(&t)
            .await
            .expect("no worker error");
        assert_eq!(out.state, RunState::Failed);
        assert!(
            message(&out).contains("not a task run of a flow's execution"),
            "{:?}",
            out.message
        );
        assert!(spy.seen.lock().unwrap().is_empty());
    }
}
