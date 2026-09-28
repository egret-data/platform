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

/// egret's side of the pod: where it runs and under what bounds. Built from
/// `workflow.kubernetes.*` (W5a Task 5); never author-settable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodConfig {
    /// The task namespace (Pod Security `restricted`).
    pub namespace: String,
    /// `runAsUser` for every container. Must be non-zero: `runAsNonRoot`
    /// makes the kubelet refuse uid 0.
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

/// The pod of one task-run attempt: `egret-run-<run id, hyphenless>-<attempt>`.
///
/// **Keyed by (run, attempt), and that is the re-attach contract.** A second
/// `run` for the same attempt — the next holder of a lease whose worker was
/// dropped — computes the same name and finds the pod; attempt N+1 (a retry)
/// computes a different one and can never adopt attempt N's pod.
///
/// The Docker runner's name carries the lease instead ([`container_name`]),
/// because its `cancel` had to be safe against a reaped worker still inside
/// its handler. W5a Task 3 removed that case: a worker that loses its fence has
/// its handler future dropped before it can reach `cancel`, so the lease no
/// longer needs to be part of the address.
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
        // `TaskCommands::argv` — W4's rule, one copy of it.
        command: Some(cmds.argv()),
        working_dir: Some(WORKING_DIR.to_string()),
        env: Some(env),
        volume_mounts: Some(vec![mount(WORKING_DIR_VOLUME, WORKING_DIR)]),
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
        init_containers: has_inputs.then(|| vec![helper(cfg, FILES_IN, READY_MARKER)]),
        containers: vec![main, helper(cfg, FILES_OUT, ENDED_MARKER)],
        volumes: Some(vec![
            empty_dir(WORKING_DIR_VOLUME),
            empty_dir(MARKERS_VOLUME),
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

fn empty_dir(name: &str) -> Volume {
    Volume {
        name: name.to_string(),
        empty_dir: Some(EmptyDirVolumeSource::default()),
        ..Default::default()
    }
}

fn helper(cfg: &PodConfig, name: &str, marker: &str) -> Container {
    Container {
        name: name.to_string(),
        image: Some(cfg.helper_image.clone()),
        image_pull_policy: Some("IfNotPresent".to_string()),
        command: Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            wait_script(marker),
        ]),
        volume_mounts: Some(vec![
            mount(WORKING_DIR_VOLUME, WORKING_DIR),
            mount(MARKERS_VOLUME, MARKERS_DIR),
        ]),
        resources: Some(requirements(
            &ResourceAmounts {
                cpu: Some(HELPER_REQUEST_CPU.into()),
                memory: Some(HELPER_REQUEST_MEMORY.into()),
            },
            &ResourceAmounts {
                cpu: Some(HELPER_LIMIT_CPU.into()),
                memory: Some(HELPER_LIMIT_MEMORY.into()),
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
/// likewise. Each is then lowered to its cap — `max.request` for a request,
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
    Ok(Resources {
        request: ResourceAmounts {
            cpu: req_cpu,
            memory: req_mem,
        },
        limit: ResourceAmounts {
            cpu: lim_cpu,
            memory: lim_mem,
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
            claimed_at: chrono::Utc::now(),
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
                },
                limit: ResourceAmounts {
                    cpu: Some("1".into()),
                    memory: Some("512Mi".into()),
                },
            },
            max_resources: Resources {
                request: ResourceAmounts {
                    cpu: Some("1".into()),
                    memory: Some("1Gi".into()),
                },
                limit: ResourceAmounts {
                    cpu: Some("2".into()),
                    memory: Some("2Gi".into()),
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

    #[test]
    fn resources_over_the_maximum_are_capped_and_those_under_it_kept() {
        let mut s = k8s();
        s.resources = Some(Resources {
            request: ResourceAmounts {
                cpu: Some("4".into()),        // over max.request.cpu = 1
                memory: Some("200Mi".into()), // under
            },
            limit: ResourceAmounts {
                cpu: Some("1500m".into()),   // under max.limit.cpu = 2
                memory: Some("64Gi".into()), // over max.limit.memory = 2Gi
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
            },
            limit: ResourceAmounts {
                cpu: Some("300m".into()),
                memory: None,
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
            vec![(WORKING_DIR_VOLUME, true), (MARKERS_VOLUME, true)]
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
            vec![(WORKING_DIR_VOLUME, WORKING_DIR)],
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
        for helper in [FILES_IN, FILES_OUT] {
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
            assert_eq!(m, vec![WORKING_DIR, MARKERS_DIR], "{helper}");
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
                "set -e\necho a\necho b".to_string()
            ],
            "W4's rule: interpreter ++ [ (beforeCommands ++ commands).join('\\n') ]"
        );
        assert!(main.args.is_none());

        let mut c = cmds();
        c.interpreter = vec!["/bin/bash".into(), "-lc".into()];
        let p = built(&c, &k8s());
        assert_eq!(
            container(&p, MAIN).command.clone().unwrap(),
            c.argv(),
            "the argv is TaskCommands::argv, not a second copy of the rule"
        );
    }
}
