//! `DockerEngineProvisioner` over `BollardDocker` against a real Docker daemon.
//!
//! Gated on `EGRET_TEST_DOCKER=1`. Without it every case is reported IGNORED
//! (never passed), which libtest cannot do at run time, so this target runs
//! without libtest (`harness = false`) with a hand-rolled `main` printing
//! libtest's own line format -- the same contract as
//! `egret-workflow/tests/container_docker_live.rs`.
//!
//! ```bash
//! EGRET_TEST_DOCKER=1 cargo test -p egret-provisioner --test docker_live -- --nocapture
//! ```
//!
//! Needs a Docker daemon at `$DOCKER_HOST` or `/var/run/docker.sock`, and pulls
//! `trinodb/trino:483` and `alpine:3.20` when absent.
//!
//! Isolation: one throwaway network `egret-test-<id>` and the project label
//! `egret-test-<id>` on everything this run creates. The cleanup guard removes
//! containers carrying exactly that project label and that network, nothing else.
//! The cases share one engine and run in order; a failed case usually fails the
//! ones after it.

use egret_engine_mgmt_api::EngineProvisioningPort;
use egret_provisioner::docker::{BollardDocker, DockerApi, DockerEngineProvisioner};
use egret_provisioner::spec::EngineKind;
use egret_provisioner::{
    CatalogSpec, EngineSpec, RoleConfig, RoleResources, TrinoConfig, WorkerRoleConfig,
};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TRINO_IMAGE: &str = "trinodb/trino:483";
const HELPER_IMAGE: &str = "alpine:3.20";
const NS: &str = "egret-engines";
const ENGINE: &str = "live";

fn docker_cli(args: &[&str]) -> Result<String, String> {
    let out = Command::new("docker")
        .args(args)
        .output()
        .map_err(|e| format!("running docker {args:?}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "docker {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Removes this run's containers (by project label) and network, even on panic.
struct Cleanup {
    project: String,
    network: String,
}

impl Cleanup {
    fn run(&self) {
        // Belt and braces: never act on anything but our own throwaway names.
        assert!(self.project.starts_with("egret-test-") && self.network.starts_with("egret-test-"));
        let label = format!("label=egret.project={}", self.project);
        if let Ok(ids) = docker_cli(&["ps", "-aq", "--filter", &label]) {
            for id in ids.split_whitespace() {
                let _ = docker_cli(&["rm", "-f", id]);
            }
        }
        let _ = docker_cli(&["network", "rm", &self.network]);
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.run();
    }
}

struct Ctx {
    project: String,
    network: String,
    api: Arc<BollardDocker>,
    prov: DockerEngineProvisioner,
    coordinator: String,
    first_container_id: std::sync::Mutex<String>,
}

fn spec(query_max_memory: &str) -> EngineSpec {
    let mut coordinator = RoleConfig {
        replicas: 1,
        resources: RoleResources {
            cpu_limit: Some("1".into()),
            mem_limit: Some("1536Mi".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    coordinator.heap.max_size = Some("1G".into());
    coordinator.query_max_memory_per_node = Some("300MB".into());
    EngineSpec {
        name: ENGINE.into(),
        namespace: NS.into(),
        image_version: "483".into(),
        config: TrinoConfig {
            query_max_memory: Some(query_max_memory.into()),
            coordinator,
            worker: WorkerRoleConfig {
                base: RoleConfig {
                    replicas: 0,
                    ..Default::default()
                },
                graceful_shutdown: None,
            },
            ..Default::default()
        },
        catalogs: vec![CatalogSpec {
            name: "tpch".into(),
            properties: [("connector.name".to_string(), "tpch".to_string())].into(),
            env: Default::default(),
        }],
        ranger: None,
        kind: EngineKind::Trino,
        starrocks: None,
    }
}

macro_rules! ensure {
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            return Err(format!($($arg)+));
        }
    };
}

async fn wait_running(c: &Ctx, within: Duration) -> Result<(), String> {
    let deadline = Instant::now() + within;
    loop {
        let st = c.prov.status("trino", NS, ENGINE).await?;
        if st.phase == "running" {
            return Ok(());
        }
        ensure!(st.phase != "failed", "engine failed: {:?}", st.message);
        ensure!(
            Instant::now() < deadline,
            "engine not running after {within:?}: phase {} pods {:?}; logs:\n{}",
            st.phase,
            st.pods,
            c.api.log_tail(&c.coordinator, 40).await.unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn ensure_image(c: &Ctx) -> Result<(), String> {
    c.api.ensure_image(TRINO_IMAGE).await?;
    c.api.ensure_image(HELPER_IMAGE).await?;
    docker_cli(&["image", "inspect", TRINO_IMAGE]).map(|_| ())?;
    // A second call must be a pure inspect.
    c.api.ensure_image(TRINO_IMAGE).await
}

async fn provision_and_wait_running(c: &Ctx) -> Result<(), String> {
    c.prov.provision(&spec("1GB")).await?;
    wait_running(c, Duration::from_secs(120)).await?;
    *c.first_container_id.lock().unwrap() =
        docker_cli(&["inspect", "-f", "{{.Id}}", &c.coordinator])?;
    Ok(())
}

async fn reachable_through_the_alias(c: &Ctx) -> Result<(), String> {
    let url = format!("http://{ENGINE}-trino:8080/v1/info");
    let label = format!("egret.project={}", c.project);
    let body = docker_cli(&[
        "run",
        "--rm",
        "--network",
        &c.network,
        "--label",
        &label,
        HELPER_IMAGE,
        "wget",
        "-qO-",
        &url,
    ])?;
    ensure!(
        body.contains("\"starting\":false"),
        "unexpected /v1/info: {body}"
    );
    Ok(())
}

async fn hardened_container(c: &Ctx) -> Result<(), String> {
    let f = |tpl: &str| docker_cli(&["inspect", "-f", tpl, &c.coordinator]);
    let ports = f("{{json .HostConfig.PortBindings}}")?;
    ensure!(ports == "{}" || ports == "null", "published ports: {ports}");
    let published = f("{{json .NetworkSettings.Ports}}")?;
    ensure!(
        !published.contains("HostPort"),
        "ports published: {published}"
    );
    let mounts = f("{{json .Mounts}}")?;
    ensure!(mounts == "[]" || mounts == "null", "mounts: {mounts}");
    let binds = f("{{json .HostConfig.Binds}}")?;
    ensure!(binds == "null" || binds == "[]", "binds: {binds}");
    ensure!(f("{{json .HostConfig.CapDrop}}")? == "[\"ALL\"]", "CapDrop");
    ensure!(
        f("{{json .HostConfig.SecurityOpt}}")?.contains("no-new-privileges"),
        "SecurityOpt"
    );
    ensure!(f("{{.HostConfig.Privileged}}")? == "false", "privileged");
    ensure!(f("{{.HostConfig.Memory}}")? == "1610612736", "memory limit");
    ensure!(f("{{.HostConfig.NanoCpus}}")? == "1000000000", "cpu limit");
    ensure!(
        f("{{index .Config.Labels \"egret.managed\"}}")? == "true",
        "managed label"
    );
    Ok(())
}

async fn update_replaces_the_container_and_its_config(c: &Ctx) -> Result<(), String> {
    let before = docker_cli(&[
        "exec",
        &c.coordinator,
        "cat",
        "/etc/trino/config.properties",
    ])?;
    ensure!(before.contains("query.max-memory=1GB"), "before:\n{before}");
    c.prov.update(&spec("2GB")).await?;
    let new_id = docker_cli(&["inspect", "-f", "{{.Id}}", &c.coordinator])?;
    ensure!(
        new_id != *c.first_container_id.lock().unwrap(),
        "update kept the old container"
    );
    wait_running(c, Duration::from_secs(120)).await?;
    let after = docker_cli(&[
        "exec",
        &c.coordinator,
        "cat",
        "/etc/trino/config.properties",
    ])?;
    ensure!(after.contains("query.max-memory=2GB"), "after:\n{after}");
    ensure!(!after.contains("query.max-memory=1GB"), "after:\n{after}");
    Ok(())
}

async fn foreign_container_survives_delete(c: &Ctx) -> Result<(), String> {
    // Same engine/namespace/project labels as ours, but no egret.managed.
    let foreign = format!("{}-foreign", c.project);
    let (project, ns, engine) = (
        format!("egret.project={}", c.project),
        format!("egret.namespace={NS}"),
        format!("egret.engine={ENGINE}"),
    );
    docker_cli(&[
        "run",
        "-d",
        "--name",
        &foreign,
        "--network",
        &c.network,
        "--label",
        &project,
        "--label",
        &ns,
        "--label",
        &engine,
        HELPER_IMAGE,
        "sleep",
        "300",
    ])?;
    c.prov.delete("trino", NS, ENGINE).await?;
    let state = docker_cli(&["inspect", "-f", "{{.State.Status}}", &foreign])?;
    ensure!(state == "running", "foreign container is {state}");
    ensure!(
        docker_cli(&["inspect", &c.coordinator]).is_err(),
        "the engine's own container survived delete"
    );
    Ok(())
}

async fn delete_leaves_no_managed_container(c: &Ctx) -> Result<(), String> {
    // Idempotent: the engine is already gone.
    c.prov.delete("trino", NS, ENGINE).await?;
    let managed = docker_cli(&[
        "ps",
        "-aq",
        "--filter",
        "label=egret.managed=true",
        "--filter",
        &format!("label=egret.project={}", c.project),
    ])?;
    ensure!(managed.is_empty(), "managed containers remain: {managed}");
    // And a missing container is Ok at the seam.
    c.api.stop_and_remove("egret-test-does-not-exist").await?;
    Ok(())
}

type CaseFut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + 'a>>;
type Case = for<'a> fn(&'a Ctx) -> CaseFut<'a>;

macro_rules! case {
    ($f:ident) => {
        (stringify!($f), (|c| Box::pin($f(c))) as Case)
    };
}

fn cases() -> Vec<(&'static str, Case)> {
    vec![
        case!(ensure_image),
        case!(provision_and_wait_running),
        case!(reachable_through_the_alias),
        case!(hardened_container),
        case!(update_replaces_the_container_and_its_config),
        case!(foreign_container_survives_delete),
        case!(delete_leaves_no_managed_container),
    ]
}

fn main() {
    let started = Instant::now();
    let cases = cases();
    println!("\nrunning {} tests", cases.len());

    if std::env::var("EGRET_TEST_DOCKER").as_deref() != Ok("1") {
        eprintln!(
            "skipping: EGRET_TEST_DOCKER=1 was not set, so these tests did not run and are \
             reported IGNORED rather than passed"
        );
        for (name, _) in &cases {
            println!("test {name} ... ignored");
        }
        println!(
            "\ntest result: ok. 0 passed; 0 failed; {} ignored; 0 measured; 0 filtered out; \
             finished in {:.2}s\n",
            cases.len(),
            started.elapsed().as_secs_f64()
        );
        return;
    }

    let id = format!(
        "{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let project = format!("egret-test-{id}");
    let network = format!("egret-test-{id}");
    let guard = Cleanup {
        project: project.clone(),
        network: network.clone(),
    };
    docker_cli(&["network", "create", &network]).expect("create the throwaway network");

    let host =
        std::env::var("DOCKER_HOST").unwrap_or_else(|_| "unix:///var/run/docker.sock".into());
    let api = Arc::new(BollardDocker::connect(&host).expect("connect to the docker daemon"));
    let ctx = Ctx {
        prov: DockerEngineProvisioner::new(
            api.clone(),
            network.clone(),
            project.clone(),
            vec!["trinodb/trino".into()],
        ),
        coordinator: format!("egret-engine-{NS}-{ENGINE}-coordinator"),
        project,
        network,
        api,
        first_container_id: Default::default(),
    };

    let rt = tokio::runtime::Runtime::new().expect("a tokio runtime");
    let (mut passed, mut failures) = (0usize, Vec::<String>::new());
    for (name, case) in &cases {
        match rt.block_on(case(&ctx)) {
            Ok(()) => {
                println!("test {name} ... ok");
                passed += 1;
            }
            Err(e) => {
                println!("test {name} ... FAILED\n    {e}");
                failures.push((*name).to_string());
            }
        }
    }
    drop(guard);

    if !failures.is_empty() {
        println!("\nfailures:\n");
        for f in &failures {
            println!("    {f}");
        }
    }
    println!(
        "\ntest result: {}. {passed} passed; {} failed; 0 ignored; 0 measured; 0 filtered out; \
         finished in {:.2}s\n",
        if failures.is_empty() { "ok" } else { "FAILED" },
        failures.len(),
        started.elapsed().as_secs_f64()
    );
    if !failures.is_empty() {
        std::process::exit(101);
    }
}
