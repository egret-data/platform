//! The task pod, as a **pure** function of the task, its commands, its
//! `taskRunner:` and egret's configuration (spec §5, *Pod, one per task-run
//! attempt*). No client, no clock: everything here is unit-tested.
//!
//! # Hardening, and why each field is here
//!
//! The namespace runs Pod Security **`restricted`**, which the API server
//! enforces at admission, so a field missing here is not a weaker pod — it is
//! a pod that is refused. Every container (init and sidecar included) gets
//! `allowPrivilegeEscalation: false` and `capabilities.drop: [ALL]`; the pod
//! gets `runAsNonRoot: true`, `runAsUser` from config and `seccompProfile:
//! RuntimeDefault`. Beyond `restricted`:
//!
//! * `automountServiceAccountToken: false` — a task has no business holding
//!   an API credential (the NetworkPolicy also blocks the API server);
//! * `enableServiceLinks: false` — otherwise the kubelet injects one
//!   `<SERVICE>_HOST`/`_PORT` env var per Service in the namespace into the
//!   author's process;
//! * `restartPolicy: Never` — a task-run attempt runs once; a retry is a new
//!   attempt, and so a new pod.
//!
//! # Resources: defaulted, then capped
//!
//! `taskRunner.resources` is the author's; `PodConfig::default_resources`
//! fills what the author left out, and `PodConfig::max_resources` caps the
//! result. A value over the cap is **lowered to the cap, not refused**: the
//! platform limit is the operator's decision, and a flow that asks for more
//! runs with what it may have (Kubernetes' own `LimitRange` behaves the same
//! way for defaults). A request left above its limit after capping is lowered
//! to the limit, because the API refuses `request > limit`.
//!
//! # Node disk: `ephemeral-storage` and `emptyDir.sizeLimit` (final review I5)
//!
//! `main` gets an `ephemeral-storage` request and limit from egret's config
//! only (the author has no such key), defaulted and capped like cpu and
//! memory. The limit is also the `sizeLimit` of the two volumes `main` writes
//! to -- the working directory and the stderr file's -- and the markers
//! volume gets a fixed 1 MiB. So a task that writes without bound is evicted
//! by the kubelet (it enforces both: the pod's summed container limits over
//! its containers' disk and its `emptyDir`s, and each `sizeLimit`), and the
//! runner FAILS the task with the kubelet's message -- rather than filling
//! the node until `DiskPressure` evicts everything on it, egret included. With
//! no limit configured anywhere the volumes have no `sizeLimit` either.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{
    Capabilities, Container, EmptyDirVolumeSource, EnvVar, LocalObjectReference, Pod,
    PodSecurityContext, PodSpec, ResourceRequirements, SeccompProfile, SecurityContext, Volume,
    VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::scripts::{PullPolicy, ResourceAmounts, Resources, RunnerSpec};
use crate::worker::commands::{
    TaskCommands, ENV_WORKING_DIR, RUN_NAME_PREFIX, TASKRUN_LABEL, WORKING_DIR,
};
use crate::worker::TaskInput;

/// The container running the flow's image and the author's command.
pub const MAIN: &str = "main";
/// The init container input files are uploaded into (Kestra: `init-files`).
pub const FILES_IN: &str = "files-in";
/// The sidecar output files are downloaded from (Kestra: `out-files`).
pub const FILES_OUT: &str = "files-out";

/// The `emptyDir` mounted at [`WORKING_DIR`] in all three containers.
pub const WORKING_DIR_VOLUME: &str = "working-dir";
/// The `emptyDir` holding the markers, mounted in the two helpers only.
pub const MARKERS_VOLUME: &str = "egret-markers";
/// Where the markers live — NOT under [`WORKING_DIR`], so `main` cannot touch
/// them.
pub const MARKERS_DIR: &str = "/egret/markers";
/// Uploaded by egret once the input files are verified; `files-in` exits on it.
pub const READY_MARKER: &str = "ready";
/// Uploaded by egret once the output files are downloaded; `files-out` exits
/// on it (Kestra: "Signaled sidecar to exit gracefully before pod deletion").
pub const ENDED_MARKER: &str = "ended";

/// The `emptyDir` carrying `main`'s stderr to `files-out`, mounted in those
/// two only (fix round 1, Important 1).
pub const STREAMS_VOLUME: &str = "egret-streams";
/// Where it is mounted.
pub const STREAMS_DIR: &str = "/egret/streams";
/// The file `main`'s fd 2 is redirected to.
pub const STDERR_FILE: &str = "/egret/streams/stderr";
/// The line egret puts in front of the author's joined script when the
/// interpreter is a shell: every later write to fd 2 — the author's, the
/// shell's own error messages, `set -e`'s — lands in [`STDERR_FILE`], which
/// `files-out` streams to its own log. Append (`>>`) because a re-attached
/// pod's file is never truncated.
pub const STDERR_REDIRECT: &str = "exec 2>>/egret/streams/stderr";
/// The interpreters [`STDERR_REDIRECT`] is valid in: POSIX-family shells.
const SHELLS: &[&str] = &["sh", "bash", "dash", "ash", "ksh", "zsh", "mksh"];

/// The four labels spec §5 names.
pub const WORKSPACE_LABEL: &str = "egret.io/workspace-id";
pub const EXECUTION_LABEL: &str = "egret.io/execution-id";
pub const ATTEMPT_LABEL: &str = "egret.io/attempt";

/// `"true"`/`"false"`: the task's `taskRunner.delete`. An annotation, not a
/// label, because nothing selects on it; the orphan sweep READS it so that a
/// pod the author asked to keep is not swept once its run ends.
pub const DELETE_ANNOTATION: &str = "egret.io/delete";
/// The author's task id. An annotation because a task id is not a valid label
/// value in general (length, characters).
pub const TASK_ID_ANNOTATION: &str = "egret.io/task-id";

/// What the two helper containers may use. Fixed: they run `sh`, `sleep` and
/// `tar` over at most a few MiB, and the author has no say in them.
const HELPER_REQUEST_CPU: &str = "10m";
const HELPER_REQUEST_MEMORY: &str = "16Mi";
const HELPER_LIMIT_CPU: &str = "200m";
const HELPER_LIMIT_MEMORY: &str = "64Mi";
/// The markers volume holds two empty files.
const MARKERS_SIZE_LIMIT: &str = "1Mi";

/// egret's side of the pod: where it runs and under what bounds. Built from
/// `workflow.kubernetes.*` (W5a Task 5); never author-settable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodConfig {
    /// The task namespace (Pod Security `restricted`).
    pub namespace: String,
    /// `runAsUser` for every container. Must be positive: `runAsNonRoot`
    /// makes the kubelet refuse uid 0, and [`PodConfig::validate`] refuses it
    /// first, with a message that names the setting.
    pub run_as_user: i64,
    /// egret's helper image (`deploy/docker/task-helper.Dockerfile`): needs
    /// `sh`, `sleep`, `tar`, `touch` and `wc`.
    pub helper_image: String,
    /// Filled in where the author's `taskRunner.resources` says nothing.
    pub default_resources: Resources,
    /// The most a task may have; `None` on a side/resource means no cap there.
    pub max_resources: Resources,
    /// Names of existing `kubernetes.io/dockerconfigjson` Secrets.
    pub image_pull_secrets: Vec<String>,
}

impl PodConfig {
    /// egret's own configuration, judged before any pod is built.
    ///
    /// `run_as_user <= 0` is refused: with `runAsNonRoot: true` the kubelet
    /// refuses uid 0 as `CreateContainerConfigError` on `main`, which the
    /// runner would otherwise report as the AUTHOR's start failure; a negative
    /// uid is refused by the API server with a message about the pod.
    pub fn validate(&self) -> Result<(), String> {
        if self.run_as_user <= 0 {
            return Err(format!(
                "workflow.kubernetes.run_as_user is {}; task pods run with runAsNonRoot, so \
                 it must be a positive uid",
                self.run_as_user
            ));
        }
        Ok(())
    }
}

/// Whether `main`'s stderr can be separated from its stdout: the interpreter
/// is a shell (basename in [`SHELLS`]) whose last argument is a short-option
/// cluster containing `c` (`-c`, `-lc`, `-ec`) — i.e. the joined script is
/// shell source that [`STDERR_REDIRECT`] can lead.
///
/// # A non-shell interpreter keeps the merged stream
///
/// `interpreter: ["python", "-c"]` runs the script as Python, where no line
/// egret could prepend redirects fd 2 without changing the author's program.
/// Its stderr then reaches `pods/log` merged with stdout and is logged at
/// **INFO** and counted as stdout — the one case where the Kubernetes runner
/// differs from the Docker runner (W11: stderr = ERROR). Refusing such an
/// interpreter was rejected: the flow runs correctly and only the level of
/// its stderr lines differs, and Kestra accepts any interpreter. The runner
/// writes one WARN line saying so at the start of such a task.
pub fn stderr_is_separable(cmds: &TaskCommands) -> bool {
    let default: Vec<String>;
    let interp: &[String] = if cmds.interpreter.is_empty() {
        default = crate::worker::commands::DEFAULT_INTERPRETER
            .iter()
            .map(|s| s.to_string())
            .collect();
        &default
    } else {
        &cmds.interpreter
    };
    if interp.len() < 2 {
        return false;
    }
    let prog = interp[0].rsplit('/').next().unwrap_or("");
    let flag = interp[interp.len() - 1].as_str();
    SHELLS.contains(&prog) && flag.starts_with('-') && !flag.starts_with("--") && flag.contains('c')
}

/// `main`'s argv: W4's `interpreter ++ [joined]` ([`TaskCommands::argv`]),
/// with [`STDERR_REDIRECT`] as the joined script's first line when the
/// interpreter is a shell ([`stderr_is_separable`]).
pub fn main_argv(cmds: &TaskCommands) -> Vec<String> {
    let mut argv = cmds.argv();
    if stderr_is_separable(cmds) {
        if let Some(script) = argv.pop() {
            argv.push(format!("{STDERR_REDIRECT}\n{script}"));
        }
    }
    argv
}

/// `files-out`'s command: stream [`STDERR_FILE`] to this container's own
/// log (`tail -F`, from the first line, retried until the file exists), then
/// wait for [`ENDED_MARKER`]. Nothing else writes to this container's stdout,
/// so its log IS `main`'s stderr (plus the end sentinel egret appends).
///
/// # It exits on SIGTERM (W5b Task 5)
///
/// A cancel with a grace deletes the pod with `gracePeriodSeconds` and waits
/// for it to go ([`TaskRunner::cancel`]). Measured on kind (Kubernetes
/// 1.33.1): while a pod terminates, the kubelet reports NO container as
/// terminated until every container has stopped -- `main` exited at +5 s
/// and showed `running` until +31 s, when this helper, which ignored SIGTERM
/// as a PID 1 without a handler, was SIGKILLed at the grace. So this helper
/// must stop on SIGTERM, or every graceful cancel sits out its whole grace.
/// Nothing reads this stream after a cancel -- the run's future was dropped
/// by the budget before `cancel` is called -- and a grace-0 delete is
/// SIGKILL either way, so W5a's paths see no difference.
///
/// [`TaskRunner::cancel`]: crate::worker::commands::TaskRunner::cancel
pub fn files_out_script() -> String {
    format!(
        "trap 'exit 0' TERM\n\
         touch {STDERR_FILE}\n\
         tail -n +1 -F {STDERR_FILE} 2>/dev/null &\n\
         while [ ! -f {MARKERS_DIR}/{ENDED_MARKER} ]\n\
         do\n  sleep 0.5\n\
         done\n"
    )
}

/// The pod of one task-run attempt: `egret-run-<run id, hyphenless>-<attempt>`.
///
/// **Keyed by (run, attempt), and that is the re-attach contract.** A second
/// `run` for the same attempt — the next holder of a lease whose worker was
/// dropped — computes the same name and finds the pod; attempt N+1 (a retry)
/// computes a different one and can never adopt attempt N's pod.
///
/// The Docker runner names its container the same way ([`container_name`]
/// calls this). W4 put the lease in that name so a reaped worker still inside
/// its handler could not reach its replacement's container; W5a took it out,
/// because `claim::under_lease` now drops a handler whose fence moved, or
/// whose renewals failed for `working_timeout`, before it can reach `cancel`
/// or the post-commit release (see `TaskHandler::cancel`'s *READ THIS*).
///
/// `attempt` is `TaskInput::attempt`, 1-based. 10 + 32 + 1 + ≤10 characters,
/// inside a DNS label's 63.
///
/// [`container_name`]: crate::worker::commands::container_name
pub fn pod_name(run_id: uuid::Uuid, attempt: i32) -> String {
    format!("{RUN_NAME_PREFIX}{}-{attempt}", run_id.simple())
}

/// The attempt number a pod is named and labelled with.
pub fn attempt_of(task: &TaskInput) -> i32 {
    i32::try_from(task.attempt).unwrap_or(i32::MAX)
}

/// The four labels, in one place so the builder and the sweep agree.
pub fn labels(task: &TaskInput) -> BTreeMap<String, String> {
    BTreeMap::from([
        (WORKSPACE_LABEL.to_string(), task.workspace_id.to_string()),
        (EXECUTION_LABEL.to_string(), task.execution_id.to_string()),
        (TASKRUN_LABEL.to_string(), task.run_id.to_string()),
        (ATTEMPT_LABEL.to_string(), attempt_of(task).to_string()),
    ])
}

/// The helper's command: Kestra's `AbstractPod.filesContainer` loop, verbatim
/// in shape, over [`MARKERS_DIR`].
pub fn wait_script(marker: &str) -> String {
    format!(
        "echo 'waiting to be {marker}!'\n\
         while [ ! -f {MARKERS_DIR}/{marker} ]\n\
         do\n  sleep 0.5\n\
         done\n\
         echo '{marker} successfully'\n"
    )
}

/// Build the pod.
///
/// `Err` is a message the **task** fails with: a `taskRunner.resources` value
/// that is not a Kubernetes quantity. The parser at save accepts any scalar
/// there (W5a Task 2), and a `{{ }}` could render to anything, so this is the
/// first place the value is judged.
pub fn build(
    task: &TaskInput,
    cmds: &TaskCommands,
    spec: &RunnerSpec,
    cfg: &PodConfig,
) -> Result<Pod, String> {
    cfg.validate()?;
    let resources = effective_resources(spec.resources.as_ref(), cfg)?;
    let has_inputs = !cmds.input_files.is_empty();

    let mut env = vec![EnvVar {
        name: ENV_WORKING_DIR.to_string(),
        value: Some(WORKING_DIR.to_string()),
        value_from: None,
    }];
    env.extend(cmds.env.iter().map(|(k, v)| EnvVar {
        name: k.clone(),
        value: Some(v.clone()),
        value_from: None,
    }));

    let main = Container {
        name: MAIN.to_string(),
        image: Some(cmds.container_image.clone()),
        image_pull_policy: Some(pull_policy(spec.pull_policy).to_string()),
        // `TaskCommands::argv` — W4's rule, one copy of it — led by the
        // stderr redirect when the interpreter is a shell.
        command: Some(main_argv(cmds)),
        working_dir: Some(WORKING_DIR.to_string()),
        env: Some(env),
        volume_mounts: Some(vec![
            mount(WORKING_DIR_VOLUME, WORKING_DIR),
            mount(STREAMS_VOLUME, STREAMS_DIR),
        ]),
        resources: Some(requirements(&resources.request, &resources.limit)),
        security_context: Some(container_security()),
        ..Default::default()
    };

    let mut annotations = BTreeMap::new();
    annotations.insert(DELETE_ANNOTATION.to_string(), spec.delete.to_string());
    annotations.insert(TASK_ID_ANNOTATION.to_string(), task.task_id.clone());

    let pod_spec = PodSpec {
        restart_policy: Some("Never".to_string()),
        automount_service_account_token: Some(false),
        enable_service_links: Some(false),
        security_context: Some(PodSecurityContext {
            run_as_non_root: Some(true),
            run_as_user: Some(cfg.run_as_user),
            seccomp_profile: Some(SeccompProfile {
                type_: "RuntimeDefault".to_string(),
                localhost_profile: None,
            }),
            ..Default::default()
        }),
        image_pull_secrets: (!cfg.image_pull_secrets.is_empty()).then(|| {
            cfg.image_pull_secrets
                .iter()
                .map(|n| LocalObjectReference { name: n.clone() })
                .collect()
        }),
        init_containers: has_inputs.then(|| {
            vec![helper(
                cfg,
                FILES_IN,
                wait_script(READY_MARKER),
                &[
                    (WORKING_DIR_VOLUME, WORKING_DIR),
                    (MARKERS_VOLUME, MARKERS_DIR),
                ],
            )]
        }),
        containers: vec![
            main,
            helper(
                cfg,
                FILES_OUT,
                files_out_script(),
                &[
                    (WORKING_DIR_VOLUME, WORKING_DIR),
                    (MARKERS_VOLUME, MARKERS_DIR),
                    (STREAMS_VOLUME, STREAMS_DIR),
                ],
            ),
        ],
        volumes: Some(vec![
            empty_dir(
                WORKING_DIR_VOLUME,
                resources.limit.ephemeral_storage.as_deref(),
            ),
            empty_dir(MARKERS_VOLUME, Some(MARKERS_SIZE_LIMIT)),
            empty_dir(STREAMS_VOLUME, resources.limit.ephemeral_storage.as_deref()),
        ]),
        ..Default::default()
    };

    Ok(Pod {
        metadata: ObjectMeta {
            name: Some(pod_name(task.run_id, attempt_of(task))),
            namespace: Some(cfg.namespace.clone()),
            labels: Some(labels(task)),
            annotations: Some(annotations),
            ..Default::default()
        },
        spec: Some(pod_spec),
        status: None,
    })
}

fn pull_policy(p: PullPolicy) -> &'static str {
    match p {
        PullPolicy::Always => "Always",
        PullPolicy::IfNotPresent => "IfNotPresent",
        PullPolicy::Never => "Never",
    }
}

/// The security context every container gets: the container-level half of
/// Pod Security `restricted`.
fn container_security() -> SecurityContext {
    SecurityContext {
        allow_privilege_escalation: Some(false),
        privileged: Some(false),
        capabilities: Some(Capabilities {
            add: None,
            drop: Some(vec!["ALL".to_string()]),
        }),
        ..Default::default()
    }
}

fn mount(volume: &str, path: &str) -> VolumeMount {
    VolumeMount {
        name: volume.to_string(),
        mount_path: path.to_string(),
        ..Default::default()
    }
}

/// An `emptyDir` on the node's disk, bounded by `size_limit` when one is set.
fn empty_dir(name: &str, size_limit: Option<&str>) -> Volume {
    Volume {
        name: name.to_string(),
        empty_dir: Some(EmptyDirVolumeSource {
            medium: None,
            size_limit: size_limit.map(|l| Quantity(l.to_string())),
        }),
        ..Default::default()
    }
}

fn helper(cfg: &PodConfig, name: &str, script: String, mounts: &[(&str, &str)]) -> Container {
    Container {
        name: name.to_string(),
        image: Some(cfg.helper_image.clone()),
        image_pull_policy: Some("IfNotPresent".to_string()),
        command: Some(vec!["sh".to_string(), "-c".to_string(), script]),
        volume_mounts: Some(mounts.iter().map(|(v, p)| mount(v, p)).collect()),
        resources: Some(requirements(
            &ResourceAmounts {
                cpu: Some(HELPER_REQUEST_CPU.into()),
                memory: Some(HELPER_REQUEST_MEMORY.into()),
                ephemeral_storage: None,
            },
            &ResourceAmounts {
                cpu: Some(HELPER_LIMIT_CPU.into()),
                memory: Some(HELPER_LIMIT_MEMORY.into()),
                ephemeral_storage: None,
            },
        )),
        security_context: Some(container_security()),
        ..Default::default()
    }
}

fn requirements(request: &ResourceAmounts, limit: &ResourceAmounts) -> ResourceRequirements {
    let side = |a: &ResourceAmounts| {
        let mut m = BTreeMap::new();
        if let Some(c) = &a.cpu {
            m.insert("cpu".to_string(), Quantity(c.clone()));
        }
        if let Some(mem) = &a.memory {
            m.insert("memory".to_string(), Quantity(mem.clone()));
        }
        if let Some(disk) = &a.ephemeral_storage {
            m.insert("ephemeral-storage".to_string(), Quantity(disk.clone()));
        }
        (!m.is_empty()).then_some(m)
    };
    ResourceRequirements {
        requests: side(request),
        limits: side(limit),
        claims: None,
    }
}

/// The author's `resources`, defaulted from config and capped by it.
///
/// Per resource (cpu, memory): request = author's, else the default's; limit
/// likewise. **A value still absent where a maximum is set becomes that
/// maximum** (fix round 1, Important 2): an absent limit is *unlimited*, which
/// is above every cap, so leaving it absent would let the task escape
/// `max_resources` whenever neither the author nor the defaults name one. An
/// absent request with `max.request` set becomes `max.request` for the same
/// reason (Kubernetes would otherwise default the request to the limit, which
/// may be above `max.request`); with only `max.limit` set it stays absent and
/// Kubernetes defaults it to the (capped) limit.
///
/// Each is then lowered to its cap — `max.request` for a request,
/// `max.limit` for a limit, and a request is ALSO capped by `max.limit` (a
/// request above the most a task may use is never satisfiable usefully).
/// Finally a request above its own limit is lowered to the limit.
pub fn effective_resources(
    author: Option<&Resources>,
    cfg: &PodConfig,
) -> Result<Resources, String> {
    let empty = Resources::default();
    let author = author.unwrap_or(&empty);
    let d = &cfg.default_resources;
    let m = &cfg.max_resources;

    let pick = |a: &Option<String>, b: &Option<String>| a.clone().or_else(|| b.clone());
    let one = |what: &str,
               req: Option<String>,
               lim: Option<String>,
               max_req: &Option<String>,
               max_lim: &Option<String>|
     -> Result<(Option<String>, Option<String>), String> {
        let lim = lim.or_else(|| max_lim.clone());
        let req = req.or_else(|| max_req.clone());
        let lim = cap(what, "limit", lim, &[max_lim])?;
        let req = cap(what, "request", req, &[max_req, max_lim])?;
        let req = match (&req, &lim) {
            (Some(r), Some(l)) if quantity(r)? > quantity(l)? => Some(l.clone()),
            _ => req,
        };
        Ok((req, lim))
    };
    let (req_cpu, lim_cpu) = one(
        "cpu",
        pick(&author.request.cpu, &d.request.cpu),
        pick(&author.limit.cpu, &d.limit.cpu),
        &m.request.cpu,
        &m.limit.cpu,
    )?;
    let (req_mem, lim_mem) = one(
        "memory",
        pick(&author.request.memory, &d.request.memory),
        pick(&author.limit.memory, &d.limit.memory),
        &m.request.memory,
        &m.limit.memory,
    )?;
    // Never the author's (the parser refuses the key): egret's default,
    // capped by egret's maximum, exactly as cpu and memory.
    let (req_disk, lim_disk) = one(
        "ephemeral-storage",
        d.request.ephemeral_storage.clone(),
        d.limit.ephemeral_storage.clone(),
        &m.request.ephemeral_storage,
        &m.limit.ephemeral_storage,
    )?;
    Ok(Resources {
        request: ResourceAmounts {
            cpu: req_cpu,
            memory: req_mem,
            ephemeral_storage: req_disk,
        },
        limit: ResourceAmounts {
            cpu: lim_cpu,
            memory: lim_mem,
            ephemeral_storage: lim_disk,
        },
    })
}

/// `value`, lowered to the smallest of `caps` that is set.
fn cap(
    what: &str,
    side: &str,
    value: Option<String>,
    caps: &[&Option<String>],
) -> Result<Option<String>, String> {
    let Some(v) = value else { return Ok(None) };
    let mut best = (
        quantity(&v).map_err(|e| format!("`taskRunner.resources.{side}.{what}`: {e}"))?,
        v,
    );
    for c in caps.iter().filter_map(|c| c.as_ref()) {
        let q = quantity(c).map_err(|e| format!("egret's configured maximum {what}: {e}"))?;
        if q < best.0 {
            best = (q, c.clone());
        }
    }
    Ok(Some(best.1))
}

/// A Kubernetes quantity (`resource.Quantity`) as a number of base units:
/// cpu cores or bytes. Precision is `f64`'s, which is ample for comparing two
/// quantities; nothing here is written back from the number.
///
/// Grammar (`k8s.io/apimachinery/pkg/api/resource/quantity.go`):
/// `<signedNumber><suffix>`, suffix one of the binary SI `Ki Mi Gi Ti Pi Ei`,
/// the decimal SI `n u m "" k M G T P E`, or a decimal exponent `e<n>`/`E<n>`.
/// A negative quantity is refused: no resource is negative.
pub fn quantity(s: &str) -> Result<f64, String> {
    let bad = || format!("{s:?} is not a Kubernetes quantity (e.g. `500m`, `2`, `256Mi`, `1G`)");
    let t = s.trim();
    let split = t
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '+' || *c == '-'))))
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    let (num, suffix) = t.split_at(split);
    if num.is_empty() || num == "+" || num == "-" || num == "." {
        return Err(bad());
    }
    let n: f64 = num.parse().map_err(|_| bad())?;
    if n < 0.0 {
        return Err(format!("{s:?} is negative, and no resource is"));
    }
    let mult = match suffix {
        "" => 1.0,
        "n" => 1e-9,
        "u" => 1e-6,
        "m" => 1e-3,
        "k" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        "P" => 1e15,
        "E" => 1e18,
        "Ki" => 1024f64,
        "Mi" => 1024f64.powi(2),
        "Gi" => 1024f64.powi(3),
        "Ti" => 1024f64.powi(4),
        "Pi" => 1024f64.powi(5),
        "Ei" => 1024f64.powi(6),
        other => {
            let exp = other
                .strip_prefix('e')
                .or_else(|| other.strip_prefix('E'))
                .ok_or_else(bad)?;
            let e: i32 = exp.parse().map_err(|_| bad())?;
            10f64.powi(e)
        }
    };
    Ok(n * mult)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripts::RunnerKind;

    fn task() -> TaskInput {
        TaskInput {
            workspace_id: uuid::Uuid::from_u128(0x11),
            execution_id: uuid::Uuid::from_u128(0x22),
            run_id: uuid::Uuid::from_u128(0x33),
            task_id: "the-task".into(),
            type_id: crate::scripts::SHELL_COMMANDS.into(),
            value: None,
            attempt: 2,
            spec: serde_json::Value::Null,
            render: crate::runners::RenderContext::new(),
            timeout: None,
            principal: None,
            log: crate::logs::RunLog::discard(),
        }
    }

    fn cfg() -> PodConfig {
        PodConfig {
            namespace: "egret-tasks".into(),
            run_as_user: 1234,
            helper_image: "egret-task-helper:dev".into(),
            default_resources: Resources {
                request: ResourceAmounts {
                    cpu: Some("100m".into()),
                    memory: Some("128Mi".into()),
                    ephemeral_storage: Some("256Mi".into()),
                },
                limit: ResourceAmounts {
                    cpu: Some("1".into()),
                    memory: Some("512Mi".into()),
                    ephemeral_storage: Some("10Gi".into()),
                },
            },
            max_resources: Resources {
                request: ResourceAmounts {
                    cpu: Some("1".into()),
                    memory: Some("1Gi".into()),
                    ..Default::default()
                },
                limit: ResourceAmounts {
                    cpu: Some("2".into()),
                    memory: Some("2Gi".into()),
                    ephemeral_storage: Some("10Gi".into()),
                },
            },
            image_pull_secrets: vec!["regcred".into(), "other".into()],
        }
    }

    fn cmds() -> TaskCommands {
        TaskCommands {
            container_image: "alpine:3.20".into(),
            interpreter: vec![],
            before_commands: vec!["set -e".into()],
            commands: vec!["echo a".into(), "echo b".into()],
            env: BTreeMap::from([("B".into(), "2".into()), ("A".into(), "1".into())]),
            input_files: BTreeMap::new(),
            output_files: vec![],
            host_protocol: false,
        }
    }

    fn k8s() -> RunnerSpec {
        RunnerSpec::defaults(RunnerKind::Kubernetes)
    }

    fn built(c: &TaskCommands, s: &RunnerSpec) -> Pod {
        build(&task(), c, s, &cfg()).expect("builds")
    }

    fn pod_spec(p: &Pod) -> &PodSpec {
        p.spec.as_ref().expect("a pod spec")
    }

    fn container<'a>(p: &'a Pod, name: &str) -> &'a Container {
        pod_spec(p)
            .containers
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no container {name}"))
    }

    fn every_container(p: &Pod) -> Vec<&Container> {
        let s = pod_spec(p);
        s.init_containers
            .iter()
            .flatten()
            .chain(s.containers.iter())
            .collect()
    }

    fn q(m: &Option<BTreeMap<String, Quantity>>, k: &str) -> Option<String> {
        m.as_ref().and_then(|m| m.get(k)).map(|q| q.0.clone())
    }

    #[test]
    fn the_pod_is_named_by_run_and_attempt_and_carries_the_four_labels() {
        let run = uuid::Uuid::from_u128(0x33);
        assert_eq!(
            pod_name(run, 2),
            format!("egret-run-{}-2", run.simple()),
            "spec §5: egret-run-<run_id>-<attempt>"
        );
        assert_ne!(
            pod_name(run, 1),
            pod_name(run, 2),
            "attempt N+1 never adopts N's pod"
        );

        let p = built(&cmds(), &k8s());
        assert_eq!(p.metadata.name.as_deref(), Some(pod_name(run, 2).as_str()));
        assert_eq!(p.metadata.namespace.as_deref(), Some("egret-tasks"));
        let labels = p.metadata.labels.clone().expect("labels");
        assert_eq!(
            labels,
            BTreeMap::from([
                (
                    "egret.io/workspace-id".to_string(),
                    uuid::Uuid::from_u128(0x11).to_string()
                ),
                (
                    "egret.io/execution-id".to_string(),
                    uuid::Uuid::from_u128(0x22).to_string()
                ),
                ("egret.io/taskrun-id".to_string(), run.to_string()),
                ("egret.io/attempt".to_string(), "2".to_string()),
            ])
        );
        let ann = p.metadata.annotations.clone().expect("annotations");
        assert_eq!(ann.get(DELETE_ANNOTATION).map(String::as_str), Some("true"));
        assert_eq!(
            ann.get(TASK_ID_ANNOTATION).map(String::as_str),
            Some("the-task")
        );

        let mut keep = k8s();
        keep.delete = false;
        let p = built(&cmds(), &keep);
        assert_eq!(
            p.metadata
                .annotations
                .unwrap()
                .get(DELETE_ANNOTATION)
                .map(String::as_str),
            Some("false"),
            "the sweep reads this to leave a `delete: false` pod alone"
        );
    }

    #[test]
    fn every_restricted_field_is_set_on_the_pod_and_on_every_container() {
        let mut c = cmds();
        c.input_files.insert("in.txt".into(), "x".into());
        let p = built(&c, &k8s());
        let s = pod_spec(&p);
        let sc = s.security_context.as_ref().expect("pod security context");
        assert_eq!(sc.run_as_non_root, Some(true));
        assert_eq!(sc.run_as_user, Some(1234), "runAsUser comes from config");
        assert_eq!(
            sc.seccomp_profile.as_ref().map(|p| p.type_.as_str()),
            Some("RuntimeDefault")
        );
        assert_eq!(s.restart_policy.as_deref(), Some("Never"));
        let all = every_container(&p);
        assert_eq!(all.len(), 3, "files-in, main, files-out");
        for c in all {
            let cs = c
                .security_context
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no security context", c.name));
            assert_eq!(cs.allow_privilege_escalation, Some(false), "{}", c.name);
            assert_ne!(cs.privileged, Some(true), "{}", c.name);
            let caps = cs.capabilities.as_ref().expect("capabilities");
            assert_eq!(
                caps.drop.as_deref(),
                Some(&["ALL".to_string()][..]),
                "{}",
                c.name
            );
            assert!(caps.add.as_ref().is_none_or(|a| a.is_empty()), "{}", c.name);
        }
    }

    #[test]
    fn no_service_account_token_and_no_service_links_reach_the_pod() {
        let p = built(&cmds(), &k8s());
        let s = pod_spec(&p);
        assert_eq!(s.automount_service_account_token, Some(false));
        assert_eq!(s.enable_service_links, Some(false));
        assert!(
            s.service_account_name.is_none(),
            "not author-settable, not set"
        );
    }

    #[test]
    fn resources_are_defaulted_from_config() {
        let p = built(&cmds(), &k8s());
        let r = container(&p, MAIN).resources.as_ref().expect("resources");
        assert_eq!(q(&r.requests, "cpu").as_deref(), Some("100m"));
        assert_eq!(q(&r.requests, "memory").as_deref(), Some("128Mi"));
        assert_eq!(q(&r.limits, "cpu").as_deref(), Some("1"));
        assert_eq!(q(&r.limits, "memory").as_deref(), Some("512Mi"));
    }

    /// **Node disk is bounded** (final review I5): `main` gets egret's
    /// `ephemeral-storage` request and limit, and every `emptyDir` has a
    /// `sizeLimit` -- the working directory and the stderr volume at the
    /// limit, the markers at 1 MiB -- so a task that writes without bound is
    /// evicted (and FAILED) instead of filling the node. The helpers get none:
    /// the pod's own limit is `main`'s, which the kubelet sums over every
    /// container and `emptyDir`.
    #[test]
    fn ephemeral_storage_is_limited_and_every_empty_dir_is_sized() {
        let p = built(&cmds(), &k8s());
        let r = container(&p, MAIN).resources.as_ref().expect("resources");
        assert_eq!(
            q(&r.requests, "ephemeral-storage").as_deref(),
            Some("256Mi")
        );
        assert_eq!(q(&r.limits, "ephemeral-storage").as_deref(), Some("10Gi"));
        let sizes: BTreeMap<&str, Option<String>> = pod_spec(&p)
            .volumes
            .as_ref()
            .expect("volumes")
            .iter()
            .map(|v| {
                let e = v.empty_dir.as_ref().expect("every volume is an emptyDir");
                assert_eq!(e.medium, None, "on the node's disk, not in memory");
                (v.name.as_str(), e.size_limit.as_ref().map(|q| q.0.clone()))
            })
            .collect();
        assert_eq!(
            sizes,
            BTreeMap::from([
                (WORKING_DIR_VOLUME, Some("10Gi".to_string())),
                (STREAMS_VOLUME, Some("10Gi".to_string())),
                (MARKERS_VOLUME, Some("1Mi".to_string())),
            ])
        );
        // The cap applies to egret's own default, as to cpu and memory.
        let mut c = cfg();
        c.max_resources.limit.ephemeral_storage = Some("2Gi".into());
        c.max_resources.request.ephemeral_storage = Some("64Mi".into());
        let p = build(&task(), &cmds(), &k8s(), &c).expect("builds");
        let r = container(&p, MAIN).resources.as_ref().expect("resources");
        assert_eq!(q(&r.limits, "ephemeral-storage").as_deref(), Some("2Gi"));
        assert_eq!(q(&r.requests, "ephemeral-storage").as_deref(), Some("64Mi"));
        let working = pod_spec(&p)
            .volumes
            .as_ref()
            .unwrap()
            .iter()
            .find(|v| v.name == WORKING_DIR_VOLUME)
            .and_then(|v| v.empty_dir.as_ref())
            .and_then(|e| e.size_limit.clone())
            .map(|q| q.0);
        assert_eq!(
            working.as_deref(),
            Some("2Gi"),
            "the sizeLimit is the CAPPED limit"
        );
        let r = container(&p, FILES_OUT)
            .resources
            .as_ref()
            .expect("resources");
        assert_eq!(
            q(&r.limits, "ephemeral-storage"),
            None,
            "the helpers get none"
        );
        // With no limit configured anywhere, nothing is invented.
        let mut c = cfg();
        c.default_resources.limit.ephemeral_storage = None;
        c.default_resources.request.ephemeral_storage = None;
        c.max_resources.limit.ephemeral_storage = None;
        let p = build(&task(), &cmds(), &k8s(), &c).expect("builds");
        let r = container(&p, MAIN).resources.as_ref().expect("resources");
        assert_eq!(q(&r.limits, "ephemeral-storage"), None);
        let working = pod_spec(&p)
            .volumes
            .as_ref()
            .unwrap()
            .iter()
            .find(|v| v.name == WORKING_DIR_VOLUME)
            .and_then(|v| v.empty_dir.as_ref())
            .and_then(|e| e.size_limit.clone());
        assert_eq!(working, None);
    }

    /// The author cannot reach ephemeral storage: Kestra's `resources` has no
    /// such key, so the parser refuses it at save.
    #[test]
    fn the_author_cannot_set_ephemeral_storage() {
        let refused = crate::scripts::parse(
            crate::scripts::SHELL_COMMANDS,
            &serde_json::json!({
                "commands": ["true"],
                "taskRunner": {
                    "type": crate::scripts::KUBERNETES_RUNNER,
                    "resources": {"limit": {"ephemeral-storage": "1Ti"}},
                },
            }),
        )
        .expect_err("refused");
        assert!(
            refused
                .iter()
                .any(|r| r.message.contains("ephemeral-storage")),
            "{refused:?}"
        );
    }

    #[test]
    fn resources_over_the_maximum_are_capped_and_those_under_it_kept() {
        let mut s = k8s();
        s.resources = Some(Resources {
            request: ResourceAmounts {
                cpu: Some("4".into()),        // over max.request.cpu = 1
                memory: Some("200Mi".into()), // under
                ..Default::default()
            },
            limit: ResourceAmounts {
                cpu: Some("1500m".into()),   // under max.limit.cpu = 2
                memory: Some("64Gi".into()), // over max.limit.memory = 2Gi
                ..Default::default()
            },
        });
        let p = built(&cmds(), &s);
        let r = container(&p, MAIN).resources.as_ref().expect("resources");
        assert_eq!(
            q(&r.requests, "cpu").as_deref(),
            Some("1"),
            "capped at max.request.cpu"
        );
        assert_eq!(q(&r.requests, "memory").as_deref(), Some("200Mi"), "kept");
        assert_eq!(q(&r.limits, "cpu").as_deref(), Some("1500m"), "kept");
        assert_eq!(
            q(&r.limits, "memory").as_deref(),
            Some("2Gi"),
            "capped at max.limit.memory"
        );
    }

    #[test]
    fn a_request_left_above_its_limit_is_lowered_to_the_limit() {
        let mut s = k8s();
        s.resources = Some(Resources {
            request: ResourceAmounts {
                cpu: Some("900m".into()),
                memory: None,
                ..Default::default()
            },
            limit: ResourceAmounts {
                cpu: Some("300m".into()),
                memory: None,
                ..Default::default()
            },
        });
        let p = built(&cmds(), &s);
        let r = container(&p, MAIN).resources.as_ref().expect("resources");
        assert_eq!(q(&r.requests, "cpu").as_deref(), Some("300m"));
        assert_eq!(q(&r.limits, "cpu").as_deref(), Some("300m"));
    }

    #[test]
    fn a_resource_that_is_not_a_quantity_fails_the_task_by_name() {
        let mut s = k8s();
        s.resources = Some(Resources {
            request: ResourceAmounts::default(),
            limit: ResourceAmounts {
                cpu: None,
                memory: Some("lots".into()),
                ..Default::default()
            },
        });
        let err = build(&task(), &cmds(), &s, &cfg()).expect_err("refused");
        assert!(
            err.contains("taskRunner.resources.limit.memory") && err.contains("lots"),
            "{err}"
        );
    }

    #[test]
    fn quantities_parse_the_way_kubernetes_writes_them() {
        assert_eq!(quantity("500m").unwrap(), 0.5);
        assert_eq!(quantity("2").unwrap(), 2.0);
        assert_eq!(quantity("1.5").unwrap(), 1.5);
        assert_eq!(quantity("1Ki").unwrap(), 1024.0);
        assert_eq!(quantity("256Mi").unwrap(), 256.0 * 1024.0 * 1024.0);
        assert_eq!(quantity("1G").unwrap(), 1e9);
        assert_eq!(quantity("1e3").unwrap(), 1000.0);
        assert!(quantity("1Gi").unwrap() > quantity("1G").unwrap());
        for bad in ["", "m", "abc", "1Xi", "-1", "1e"] {
            assert!(quantity(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn pull_policy_and_pull_secrets_come_from_the_runner_and_config() {
        for (policy, want) in [
            (PullPolicy::Always, "Always"),
            (PullPolicy::IfNotPresent, "IfNotPresent"),
            (PullPolicy::Never, "Never"),
        ] {
            let mut s = k8s();
            s.pull_policy = policy;
            let p = built(&cmds(), &s);
            assert_eq!(container(&p, MAIN).image_pull_policy.as_deref(), Some(want));
        }
        let p = built(&cmds(), &k8s());
        assert_eq!(
            container(&p, MAIN).image_pull_policy.as_deref(),
            Some("IfNotPresent"),
            "the default is IF_NOT_PRESENT"
        );
        assert_eq!(container(&p, MAIN).image.as_deref(), Some("alpine:3.20"));
        let secrets: Vec<String> = pod_spec(&p)
            .image_pull_secrets
            .iter()
            .flatten()
            .map(|r| r.name.clone())
            .collect();
        assert_eq!(secrets, vec!["regcred".to_string(), "other".to_string()]);
    }

    #[test]
    fn the_init_container_exists_only_when_there_are_input_files() {
        let p = built(&cmds(), &k8s());
        assert!(
            pod_spec(&p)
                .init_containers
                .as_ref()
                .is_none_or(|v| v.is_empty()),
            "no input files, no files-in"
        );

        let mut c = cmds();
        c.input_files.insert("data/in.txt".into(), "hello".into());
        let p = built(&c, &k8s());
        let inits = pod_spec(&p)
            .init_containers
            .clone()
            .expect("init containers");
        assert_eq!(inits.len(), 1);
        let fi = &inits[0];
        assert_eq!(fi.name, FILES_IN);
        assert_eq!(fi.image.as_deref(), Some("egret-task-helper:dev"));
        let cmd = fi.command.clone().expect("command");
        assert_eq!(cmd[..2], ["sh".to_string(), "-c".to_string()]);
        assert!(cmd[2].contains("/egret/markers/ready"), "{}", cmd[2]);
    }

    #[test]
    fn the_files_out_sidecar_is_always_present() {
        for with_inputs in [false, true] {
            let mut c = cmds();
            if with_inputs {
                c.input_files.insert("f".into(), "x".into());
            }
            let p = built(&c, &k8s());
            let names: Vec<&str> = pod_spec(&p)
                .containers
                .iter()
                .map(|c| c.name.as_str())
                .collect();
            assert_eq!(names, vec![MAIN, FILES_OUT]);
            let fo = container(&p, FILES_OUT);
            assert_eq!(fo.image.as_deref(), Some("egret-task-helper:dev"));
            assert!(fo.command.clone().unwrap()[2].contains("/egret/markers/ended"));
        }
    }

    #[test]
    fn main_runs_in_the_working_dir_emptydir_with_working_dir_set() {
        let mut c = cmds();
        c.input_files.insert("f".into(), "x".into());
        let p = built(&c, &k8s());
        let s = pod_spec(&p);
        let vols: Vec<(&str, bool)> = s
            .volumes
            .iter()
            .flatten()
            .map(|v| (v.name.as_str(), v.empty_dir.is_some()))
            .collect();
        assert_eq!(
            vols,
            vec![
                (WORKING_DIR_VOLUME, true),
                (MARKERS_VOLUME, true),
                (STREAMS_VOLUME, true)
            ]
        );

        let main = container(&p, MAIN);
        assert_eq!(main.working_dir.as_deref(), Some(WORKING_DIR));
        let mounts: Vec<(&str, &str)> = main
            .volume_mounts
            .iter()
            .flatten()
            .map(|m| (m.name.as_str(), m.mount_path.as_str()))
            .collect();
        assert_eq!(
            mounts,
            vec![
                (WORKING_DIR_VOLUME, WORKING_DIR),
                (STREAMS_VOLUME, STREAMS_DIR)
            ],
            "main does NOT mount the markers"
        );
        let env: Vec<(String, Option<String>)> = main
            .env
            .iter()
            .flatten()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect();
        assert_eq!(
            env,
            vec![
                (ENV_WORKING_DIR.to_string(), Some(WORKING_DIR.to_string())),
                ("A".to_string(), Some("1".to_string())),
                ("B".to_string(), Some("2".to_string())),
            ]
        );
        for (helper, want) in [
            (FILES_IN, vec![WORKING_DIR, MARKERS_DIR]),
            (FILES_OUT, vec![WORKING_DIR, MARKERS_DIR, STREAMS_DIR]),
        ] {
            let h = every_container(&p)
                .into_iter()
                .find(|c| c.name == helper)
                .unwrap();
            let m: Vec<&str> = h
                .volume_mounts
                .iter()
                .flatten()
                .map(|m| m.mount_path.as_str())
                .collect();
            assert_eq!(m, want, "{helper}");
        }
    }

    #[test]
    fn mains_argv_is_the_interpreter_then_the_joined_script() {
        let p = built(&cmds(), &k8s());
        let main = container(&p, MAIN);
        assert_eq!(
            main.command.clone().expect("command"),
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "exec 2>>/egret/streams/stderr\nset -e\necho a\necho b".to_string()
            ],
            "W4's rule: interpreter ++ [ (beforeCommands ++ commands).join('\\n') ], \
             led by the stderr redirect"
        );
        assert!(main.args.is_none());

        let mut c = cmds();
        c.interpreter = vec!["/bin/bash".into(), "-lc".into()];
        let p = built(&c, &k8s());
        let mut want = c.argv();
        let script = want.pop().unwrap();
        want.push(format!("{STDERR_REDIRECT}\n{script}"));
        assert_eq!(
            container(&p, MAIN).command.clone().unwrap(),
            want,
            "the argv is TaskCommands::argv plus one leading line, not a second copy of the rule"
        );
    }

    #[test]
    fn stderr_is_redirected_only_when_the_interpreter_is_a_shell() {
        for (interp, separable) in [
            (vec![], true),
            (vec!["/bin/sh", "-c"], true),
            (vec!["sh", "-c"], true),
            (vec!["/usr/bin/bash", "-lc"], true),
            (vec!["bash", "-ec"], true),
            (vec!["python", "-c"], false),
            (vec!["/usr/bin/env", "sh", "-c"], false),
            (vec!["/bin/sh"], false),
            (vec!["/bin/sh", "-x"], false),
            (vec!["/bin/sh", "--c"], false),
        ] {
            let mut c = cmds();
            c.interpreter = interp.iter().map(|s| s.to_string()).collect();
            assert_eq!(stderr_is_separable(&c), separable, "{interp:?}");
            let argv = main_argv(&c);
            assert_eq!(
                argv.last().unwrap().starts_with(STDERR_REDIRECT),
                separable,
                "{interp:?}: {argv:?}"
            );
            if !separable {
                assert_eq!(argv, c.argv(), "a non-shell argv is W4's, untouched");
            }
        }
    }

    #[test]
    fn files_out_streams_the_stderr_file_and_prints_nothing_else() {
        let script = files_out_script();
        assert!(
            script.contains("tail -n +1 -F /egret/streams/stderr"),
            "{script}"
        );
        assert!(script.contains("/egret/markers/ended"), "{script}");
        assert!(
            script.starts_with("trap 'exit 0' TERM\n"),
            "files-out stops on SIGTERM, so a graceful delete ends when main does: {script}"
        );
        assert!(
            !script.contains("echo"),
            "files-out's log IS main's stderr, so the helper itself prints nothing: {script}"
        );
    }

    #[test]
    fn a_non_positive_run_as_user_is_refused_by_name() {
        for uid in [0, -1] {
            let mut cfg = cfg();
            cfg.run_as_user = uid;
            let err = cfg.validate().expect_err("refused");
            assert!(
                err.contains("run_as_user") && err.contains(&uid.to_string()),
                "{err}"
            );
            assert!(build(&task(), &cmds(), &k8s(), &cfg).is_err());
        }
        assert!(cfg().validate().is_ok());
    }

    fn limits(
        r: &Resources,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        (
            r.request.cpu.clone(),
            r.request.memory.clone(),
            r.limit.cpu.clone(),
            r.limit.memory.clone(),
        )
    }

    #[test]
    fn an_absent_value_with_a_maximum_set_becomes_the_maximum() {
        let s = |v: &str| Some(v.to_string());
        // No defaults at all; the operator set only maxima.
        let mut c = cfg();
        c.default_resources = Resources::default();

        // Both maxima set: absent request -> max.request, absent limit -> max.limit.
        let r = effective_resources(None, &c).unwrap();
        assert_eq!(limits(&r), (s("1"), s("1Gi"), s("2"), s("2Gi")));

        // Only max.limit set: the limit is the max; the request stays absent
        // (Kubernetes defaults it to the capped limit).
        c.max_resources.request = ResourceAmounts::default();
        let r = effective_resources(None, &c).unwrap();
        assert_eq!(limits(&r), (None, None, s("2"), s("2Gi")));

        // Only max.request set: the request is the max, no limit exists to cap.
        c.max_resources = Resources {
            request: ResourceAmounts {
                cpu: s("1"),
                memory: s("1Gi"),
                ..Default::default()
            },
            limit: ResourceAmounts::default(),
        };
        let r = effective_resources(None, &c).unwrap();
        assert_eq!(limits(&r), (s("1"), s("1Gi"), None, None));

        // No maxima: nothing is invented.
        c.max_resources = Resources::default();
        let r = effective_resources(None, &c).unwrap();
        assert_eq!(limits(&r), (None, None, None, None));

        // An author's request above an absent-then-maxed limit is lowered to it.
        c.max_resources = Resources {
            request: ResourceAmounts::default(),
            limit: ResourceAmounts {
                cpu: s("500m"),
                memory: s("256Mi"),
                ..Default::default()
            },
        };
        let author = Resources {
            request: ResourceAmounts {
                cpu: s("2"),
                memory: s("1Gi"),
                ..Default::default()
            },
            limit: ResourceAmounts::default(),
        };
        let r = effective_resources(Some(&author), &c).unwrap();
        assert_eq!(limits(&r), (s("500m"), s("256Mi"), s("500m"), s("256Mi")));
    }
}
