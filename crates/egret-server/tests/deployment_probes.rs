//! The chart half of the deployment bounds, asserted against the templates.
//!
//! # Why a Rust test reads YAML
//!
//! Three of this package's numbers live on both sides of a seam — the drain
//! deadline in `egret-server` and `terminationGracePeriodSeconds` in the chart;
//! `/readyz` as a handler and `/readyz` as a probe target. Each pair is correct
//! only together, and nothing else in the tree can fail when one half moves.
//! That is the seam class this repo keeps shipping defects through, so the
//! relation is asserted rather than commented.
//!
//! The templates are not parseable YAML (`replicas: {{ … }}` is a Go template,
//! not a mapping), so these read text. That is a weaker instrument than a parse
//! and it is enough for the one question asked: did the probe stop being a
//! `tcpSocket`.

use std::time::Duration;

fn read(rel: &str) -> String {
    let path = format!("{}/../../{rel}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn deployment() -> String {
    read("deploy/helm/egret/templates/backend-deployment.yaml")
}

fn values() -> String {
    read("deploy/helm/egret/values.yaml")
}

/// The value of the first `key: <integer>` line in `yaml`.
fn scalar_int(yaml: &str, key: &str) -> u64 {
    yaml.lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim())
        })
        .unwrap_or_else(|| panic!("no `{key}:` line"))
        .parse()
        .unwrap_or_else(|e| panic!("`{key}` is not an integer: {e}"))
}

/// The finding this exists to close: `/readyz` is a handler that pings the
/// database, and the probe was a `tcpSocket`, so it had never once been called.
/// A pod with a dead metastore still had a listening socket and therefore still
/// received traffic.
#[test]
fn the_readiness_probe_actually_calls_readyz() {
    let d = deployment();
    let readiness = d
        .split("readinessProbe:")
        .nth(1)
        .expect("no readinessProbe in the backend deployment");
    assert!(
        readiness.contains("httpGet"),
        "the readiness probe must be an HTTP GET; a tcpSocket probe passes on a \
         pod whose database is gone"
    );
    assert!(
        readiness.contains("path: /readyz"),
        "the readiness probe must call /readyz — the only handler that checks a \
         dependency"
    );
}

/// The liveness probe must NOT check the database.
///
/// This is the tempting simplification and it is a trap: pointing liveness at
/// `/readyz` turns a Postgres blip into a simultaneous restart of every replica,
/// which is strictly worse than the outage it reacts to. The distinction is only
/// visible in the chart, so it is pinned here.
#[test]
fn the_liveness_probe_checks_the_process_and_not_the_database() {
    let d = deployment();
    let liveness = d
        .split("livenessProbe:")
        .nth(1)
        .expect("no livenessProbe in the backend deployment");
    assert!(
        liveness.contains("path: /healthz"),
        "liveness must call /healthz"
    );
    assert!(
        !liveness.contains("/readyz"),
        "liveness must not depend on the database: a blip would restart every \
         replica at once"
    );
}

/// A Kubernetes memory quantity (`4Gi`, `512Mi`, `1G`) in bytes.
fn memory_bytes(q: &str) -> u64 {
    let q = q.trim().trim_matches('"');
    for (suffix, mult) in [
        ("Gi", 1024u64 * 1024 * 1024),
        ("Mi", 1024 * 1024),
        ("Ki", 1024),
        ("G", 1_000_000_000),
        ("M", 1_000_000),
        ("K", 1_000),
    ] {
        if let Some(n) = q.strip_suffix(suffix) {
            return n.parse::<u64>().unwrap_or_else(|e| panic!("{q}: {e}")) * mult;
        }
    }
    q.parse().unwrap_or_else(|e| panic!("{q}: {e}"))
}

/// A Kubernetes CPU quantity (`500m`, `2000m`, `2`) in millicores.
fn cpu_millis(q: &str) -> u64 {
    let q = q.trim().trim_matches('"');
    match q.strip_suffix('m') {
        Some(n) => n.parse().unwrap_or_else(|e| panic!("{q}: {e}")),
        None => q.parse::<u64>().unwrap_or_else(|e| panic!("{q}: {e}")) * 1000,
    }
}

/// The value of `key:` inside the block that follows `section:`, stopping at the
/// next line indented no further than `section` itself.
fn scoped_scalar<'a>(yaml: &'a str, section: &str, key: &str) -> &'a str {
    let mut lines = yaml.lines().skip_while(|l| l.trim() != section);
    let head = lines
        .next()
        .unwrap_or_else(|| panic!("no `{section}` line"));
    let indent = head.len() - head.trim_start().len();
    for l in lines {
        if l.trim().is_empty() {
            continue;
        }
        if l.len() - l.trim_start().len() <= indent {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            if k.trim() == key {
                return v.trim();
            }
        }
    }
    panic!("no `{key}:` under `{section}`");
}

/// BestEffort QoS was the finding; a `resources:` block that the template never
/// reads would be the silent way to keep it — and so would a block that declares
/// every key with a number nobody could run on.
///
/// **This test used to assert key PRESENCE only, which `memory: 64Mi` passes.**
/// That is the shape this repo has shipped a green test over fourteen times, so
/// the values are read and bounded. The floor is not arbitrary: 2Gi is below the
/// 4Gi the deployment comment argues for and far above anything that could be a
/// typo or a leftover, and the comment is explicit that even 4Gi does not hold a
/// largest-publishable response — that is what streaming is for, not this number.
#[test]
fn the_backend_container_declares_requests_and_limits() {
    let d = deployment();
    assert!(
        d.contains(".Values.backend.resources"),
        "the deployment must render backend.resources, or the values below are \
         decoration and the container stays BestEffort"
    );
    let v = values();
    let backend = v
        .split("\nfrontend:")
        .next()
        .expect("values.yaml must start with the backend block");

    let req_mem = memory_bytes(scoped_scalar(backend, "requests:", "memory"));
    let lim_mem = memory_bytes(scoped_scalar(backend, "limits:", "memory"));
    let req_cpu = cpu_millis(scoped_scalar(backend, "requests:", "cpu"));
    let lim_cpu = cpu_millis(scoped_scalar(backend, "limits:", "cpu"));

    const TWO_GI: u64 = 2 * 1024 * 1024 * 1024;
    assert!(
        lim_mem >= TWO_GI,
        "the backend memory LIMIT is {lim_mem} bytes. Below 2Gi the pod cannot \
         hold egret's working set beside an ordinary response, and the kernel \
         kills it under normal load rather than under abuse — which is a worse \
         failure than the BestEffort QoS this block replaced"
    );
    assert!(
        req_mem >= 512 * 1024 * 1024,
        "the backend memory REQUEST is {req_mem} bytes; the scheduler places the \
         pod on this number, so a token value overcommits the node the limit \
         exists to protect"
    );
    assert!(
        lim_mem >= req_mem && lim_cpu >= req_cpu,
        "limits ({lim_mem} bytes / {lim_cpu}m) must not be below requests \
         ({req_mem} bytes / {req_cpu}m) — the pod would be unschedulable"
    );
    assert!(
        req_cpu > 0 && lim_cpu > 0,
        "both CPU quantities must be non-zero: {req_cpu}m / {lim_cpu}m"
    );
}

/// The seam that makes the graceful shutdown reachable at all.
///
/// Kubernetes' default grace period is 30 s — below even the per-request
/// deadline — so leaving it unset would have meant SIGKILL arriving in the
/// middle of every drain and the whole of this package's item 3 doing nothing.
#[test]
fn the_drain_deadline_fits_inside_the_pods_termination_grace_period() {
    let d = deployment();
    assert!(
        d.contains(".Values.backend.terminationGracePeriodSeconds"),
        "the deployment must set terminationGracePeriodSeconds; the default 30 s \
         is shorter than the drain"
    );
    let grace = Duration::from_secs(scalar_int(&values(), "terminationGracePeriodSeconds"));
    assert!(
        grace > egret_server::DRAIN_DEADLINE,
        "terminationGracePeriodSeconds ({grace:?}) must exceed DRAIN_DEADLINE \
         ({:?}), or SIGKILL ends the drain before the drain does",
        egret_server::DRAIN_DEADLINE
    );
}

/// The readiness block, as text, so a relation can be read out of it without
/// tripping over `startupProbe`'s identically-named keys above it.
fn readiness_probe() -> String {
    let d = deployment();
    let after = d
        .split("readinessProbe:")
        .nth(1)
        .expect("no readinessProbe in the backend deployment")
        .to_string();
    after
        .split("livenessProbe:")
        .next()
        .expect("readiness block")
        .to_string()
}

/// egret must answer the probe before the kubelet stops listening.
///
/// If `READYZ_DB_TIMEOUT` were at or above the probe's `timeoutSeconds`, every
/// slow probe would be decided by the kubelet instead of by egret: no body, no
/// `db` field to read, and — because the kubelet's give-up does not cancel
/// anything inside the process — one abandoned `acquire` left queued behind the
/// traffic that caused it, once per `periodSeconds`.
#[test]
fn readyz_answers_before_the_kubelet_gives_up_on_it() {
    let probe = readiness_probe();
    let kubelet = Duration::from_secs(scalar_int(&probe, "timeoutSeconds"));
    assert!(
        kubelet > egret_server::READYZ_DB_TIMEOUT,
        "the readiness probe's timeoutSeconds ({kubelet:?}) must exceed \
         READYZ_DB_TIMEOUT ({:?}), or egret never gets to answer its own probe",
        egret_server::READYZ_DB_TIMEOUT
    );
}

/// The relation that stops a healthy pod being withdrawn for being busy.
///
/// `SERVING_MAX_CONCURRENCY` (64) is deliberately 8× the metastore pool (10), so
/// contention for a connection is the DESIGNED steady state under load, and one
/// admitted request may hold its connection for `SERVING_REQUEST_DEADLINE`. A
/// withdrawal budget shorter than one request therefore removes pods that are
/// merely working, and hands their traffic to peers that then fail the same
/// check — the cascade the bulkhead exists to prevent, caused by the probe.
///
/// Asserted against the constant rather than against the comment, because the
/// two halves live in different repositories' worth of file.
#[test]
fn readiness_is_not_withdrawn_before_one_full_length_request_can_finish() {
    let probe = readiness_probe();
    let period = scalar_int(&probe, "periodSeconds");
    let failures = scalar_int(&probe, "failureThreshold");
    let budget = Duration::from_secs(period * failures);
    assert!(
        budget > egret_dataapi::serving::SERVING_REQUEST_DEADLINE,
        "readiness withdraws traffic after {period}s x {failures} = {budget:?}, \
         which is inside SERVING_REQUEST_DEADLINE ({:?}). A pod holding one \
         legal, full-length request would be taken out of Endpoints for being \
         busy, and its load would flap its peers in turn",
        egret_dataapi::serving::SERVING_REQUEST_DEADLINE
    );
}

/// And the drain must outlast the request it is draining, or it cuts in half the
/// very requests it exists to protect.
#[test]
fn the_drain_deadline_outlasts_a_full_length_request() {
    assert!(
        egret_server::DRAIN_DEADLINE > egret_dataapi::serving::SERVING_REQUEST_DEADLINE,
        "DRAIN_DEADLINE ({:?}) must exceed SERVING_REQUEST_DEADLINE ({:?}): a \
         request admitted just before SIGTERM has to be able to finish or time \
         out on its own terms",
        egret_server::DRAIN_DEADLINE,
        egret_dataapi::serving::SERVING_REQUEST_DEADLINE
    );
}

/// Comment and blank lines removed, so the text assertions below cannot be
/// satisfied by prose. These templates carry their argument in a comment right
/// beside the line the assertion is about — which means a comment could spell
/// the very string being looked for, and a test a comment can satisfy is the
/// shape this repo has shipped a green test over sixteen times.
fn without_comments(yaml: &str) -> String {
    yaml.lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A single-instance dependency must be REPLACED on upgrade, never surged past.
///
/// `replicas: 1` does not on its own deliver "one of these exists". A
/// Deployment's default strategy is RollingUpdate, which starts the new pod
/// BEFORE terminating the old one, so any edit to the pod spec — the image, a
/// memory number, an argument — briefly puts two pods behind the one Service
/// that selects them.
///
/// For Valkey that is Package 4's own defect one level down: two stores means
/// the Data API's rate-limit buckets and response cache split across them for
/// the length of the rollout, and a published limit transiently doubles on a
/// `helm upgrade` that touched nothing to do with limits. For postgres and
/// minio it is older and blunter: the surged pod cannot mount a ReadWriteOnce
/// PVC, so the upgrade wedges instead of rolling.
///
/// Only the stateful three. The frontend and keycloak Deployments are
/// deliberately NOT in this list — they hold nothing behind their Service that
/// two copies could disagree about, and Recreate would buy them a downtime gap
/// for nothing.
#[test]
fn every_single_instance_store_is_recreated_rather_than_rolled() {
    for (file, what) in [
        (
            "deps-valkey.yaml",
            "two Valkeys behind one `app: valkey` Service split the Data API's \
             rate-limit buckets and response cache — the very defect Package 4 \
             removed, reintroduced by the store's own upgrade",
        ),
        (
            "deps-postgres.yaml",
            "a surged postgres cannot mount the ReadWriteOnce postgres-data PVC, \
             and on a shared volume would be a second postmaster over one data \
             directory",
        ),
        (
            "deps-minio.yaml",
            "a surged minio cannot mount the ReadWriteOnce minio-data PVC",
        ),
    ] {
        let yaml = without_comments(&read(&format!("deploy/helm/egret/templates/{file}")));
        assert!(
            yaml.contains("kind: Deployment"),
            "{file} is no longer a Deployment; this test's premise has moved"
        );
        assert!(
            yaml.contains("\n  replicas: 1"),
            "{file} no longer declares a single replica; this test's premise has moved"
        );
        let after = yaml
            .split("\n  strategy:")
            .nth(1)
            .unwrap_or_else(|| panic!("{file} declares no `strategy:` — {what}"));
        assert!(
            after.starts_with("\n    type: Recreate"),
            "{file} must declare `strategy: {{ type: Recreate }}`: {what}"
        );
    }
}

// ── the apps host ───────────────────────────────────────────────────────────

/// The apps host has its own Ingress object because the `egret` one pins a 60 s read timeout (see
/// that file for why) which would cut every idle marimo WebSocket at a minute. This pins the
/// relation: the timeouts here must out-last the apps proxy's idle token window's shortest sensible
/// wait, buffering must be off for server-sent events, and the backend must be the APPS listener
/// (8081), never the API port. The apps listener's port is in the backend Deployment and the
/// Service; all three have to say 8081 or the host serves nothing.
#[test]
fn the_apps_ingress_keeps_websockets_alive_and_reaches_the_apps_listener() {
    let ingress = read("deploy/helm/egret/templates/ingress-apps.yaml");
    // an annotation value must be a quoted string, which `scalar_int` does not unquote
    let read_timeout: u64 = ingress
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("nginx.ingress.kubernetes.io/proxy-read-timeout:")
        })
        .expect("no proxy-read-timeout annotation")
        .trim()
        .trim_matches('"')
        .parse()
        .expect("proxy-read-timeout is a number of seconds");
    assert!(
        read_timeout >= 3600,
        "an idle marimo WebSocket would be cut at {read_timeout}s"
    );
    assert!(
        ingress.contains("proxy-buffering: \"off\""),
        "SSE needs buffering off"
    );
    assert!(
        ingress.contains("port: { number: 8081 }"),
        "the apps host must reach the apps listener, not the API port"
    );
    assert!(
        !ingress.contains("number: 8080"),
        "the API port must not be reachable through the apps host"
    );
    // the other two places that must agree on the port
    assert!(
        deployment().contains("containerPort: 8081"),
        "the Deployment must expose 8081"
    );
    assert!(
        read("deploy/helm/egret/templates/backend-service.yaml").contains("targetPort: 8081"),
        "the Service must route 8081"
    );
    // and the default config of the listener is the same number
    assert!(
        egret_config::AppsConfig::default()
            .listen
            .ends_with(":8081"),
        "the chart and the config default disagree on the apps port"
    );
}

/// TLS ends at the ingress, so the backend sees plain http. If it were not told its cookies are
/// `Secure`, the `__Host-` names would not be issued and the cookie-tossing defence would be off
/// exactly where the apps origin exists.
#[test]
fn the_backend_is_told_its_cookies_are_secure_when_tls_is_on() {
    let d = deployment();
    let at = d
        .find("EGRET_SESSION__SECURE")
        .expect("the chart never sets EGRET_SESSION__SECURE");
    let before = &d[..at];
    let guard = before.rfind("{{- if").expect("an `if` guards it");
    assert!(
        before[guard..].contains(".Values.ingress.tls.enabled"),
        "EGRET_SESSION__SECURE must be conditional on ingress.tls.enabled"
    );
}

/// Run pods (D24) get their own idle/cap/kernel-mode env, next to the edit settings, only when
/// `apps.pod.launcher` is kubernetes — the same guard the edit settings already live under.
#[test]
fn run_pod_settings_reach_the_backend_env_under_the_kubernetes_launcher_guard() {
    let d = deployment();
    for name in [
        "EGRET_APPS__POD__RUN_IDLE_SECS",
        "EGRET_APPS__POD__RUN_SESSION_CAP",
        "EGRET_APPS__POD__RUN_KERNEL_MODE",
    ] {
        let at = d
            .find(name)
            .unwrap_or_else(|| panic!("the chart never sets {name}"));
        let before = &d[..at];
        let guard = before.rfind("{{- if").expect("an `if` guards it");
        assert!(
            before[guard..].contains(r#".Values.apps.pod.launcher "kubernetes""#),
            "{name} must be conditional on apps.pod.launcher == kubernetes"
        );
    }
}
