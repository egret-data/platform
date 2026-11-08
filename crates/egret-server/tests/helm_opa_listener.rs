//! The chart half of the OPA listener (port-8080 hardening, B5), asserted against
//! the templates. Like `deployment_probes.rs`, these read text: the templates are
//! Go templates, not YAML. The question asked is whether the OPA port exists on the
//! Service and pod, and whether only engines are admitted to it.

fn read(rel: &str) -> String {
    let path = format!(
        "{}/../../deploy/helm/egret/{rel}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

/// The `ingress:` rules of the policy, one string per `- ` rule.
fn rules() -> Vec<String> {
    let policy = read("templates/networkpolicy-egret.yaml");
    let body = policy
        .split("  ingress:\n")
        .nth(1)
        .expect("no ingress rules");
    format!("\n{body}")
        .split("\n    - ")
        .skip(1)
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_policy_selects_only_the_backend_pods() {
    let policy = read("templates/networkpolicy-egret.yaml");
    assert!(policy.contains("podSelector: { matchLabels: { app: egret } }"));
    assert!(read("templates/backend-deployment.yaml").contains("labels: { app: egret }"));
}

#[test]
fn api_and_apps_ports_stay_open_to_every_peer() {
    let rules = rules();
    let open = rules
        .iter()
        .find(|r| r.contains("port: 8080"))
        .expect("no 8080 rule");
    assert!(
        !open.contains("from:"),
        "8080 must stay open to every peer: {open}"
    );
    assert!(open.contains("port: 8081"));
    assert!(!open.contains(".Values.opa.port"));
}

#[test]
fn the_opa_port_admits_only_engines() {
    let rules = rules();
    let opa = rules
        .iter()
        .find(|r| r.contains(".Values.opa.port"))
        .expect("no OPA rule");
    assert!(
        opa.contains("from:"),
        "the OPA port must name its peers: {opa}"
    );
    assert!(opa.contains(".Values.engineNamespace"));
    assert!(opa.contains(".Values.opa.extraFrom"));
    assert!(!opa.contains("port: 8080"));
}

#[test]
fn the_opa_port_is_on_the_pod_service_and_env_under_opa_enabled() {
    for (file, needle) in [
        (
            "templates/backend-deployment.yaml",
            "name: opa, containerPort: {{ .Values.opa.port }}",
        ),
        (
            "templates/backend-service.yaml",
            "name: opa, port: {{ .Values.opa.port }}",
        ),
        (
            "templates/configmap-env.yaml",
            "EGRET_GOVERNANCE__OPA__ENABLED: \"true\"",
        ),
        (
            "templates/configmap-env.yaml",
            "EGRET_GOVERNANCE__OPA__LISTEN: \"0.0.0.0:{{ .Values.opa.port }}\"",
        ),
    ] {
        let text = read(file);
        let at = text
            .find(needle)
            .unwrap_or_else(|| panic!("{file}: no `{needle}`"));
        let guard = text[..at]
            .rfind("{{- if")
            .expect("no guard before the line");
        assert!(
            text[guard..at].contains(".Values.opa.enabled"),
            "{file}: `{needle}` must sit under `if .Values.opa.enabled`"
        );
    }
}

#[test]
fn values_default_the_port_to_8082_disabled() {
    let v = read("values.yaml");
    let opa = v.split("\nopa:\n").nth(1).expect("no opa: block");
    assert!(opa.contains("  enabled: false"));
    assert!(opa.contains("  port: 8082"));
}
