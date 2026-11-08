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
fn the_opa_port_is_on_the_pod_service_and_listen_env_under_opa_enabled() {
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
            "EGRET_GOVERNANCE__OPA__LISTEN: \"0.0.0.0:{{ .Values.opa.port }}\"",
        ),
        (
            "templates/networkpolicy-egret.yaml",
            "port: {{ .Values.opa.port }}",
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
        assert!(
            text[guard..at].contains(".Values.ranger.enabled"),
            "{file}: `{needle}` must also need `.Values.ranger.enabled`: OPA does not run without it"
        );
    }
}

/// The binary now defaults OPA on, so the chart's value must always be rendered:
/// `opa.enabled=false` has to reach the pod as an explicit "false".
#[test]
fn the_enabled_env_is_rendered_from_the_value_whatever_it_is() {
    let text = read("templates/configmap-env.yaml");
    let needle = "EGRET_GOVERNANCE__OPA__ENABLED: \"{{ .Values.opa.enabled }}\"";
    let at = text.find(needle).unwrap_or_else(|| panic!("no `{needle}`"));
    // No open `if` between the start of the file and the line.
    let before = &text[..at];
    let opens = before.matches("{{- if").count() + before.matches("{{ if").count();
    let closes = before.matches("{{- end").count() + before.matches("{{ end").count();
    assert_eq!(opens, closes, "the enabled env must not sit under a guard");
    assert!(!text.contains("OPA__ENABLED: \"true\""));
}

#[test]
fn values_default_the_port_to_8082_enabled() {
    let v = read("values.yaml");
    let opa = v.split("\nopa:\n").nth(1).expect("no opa: block");
    assert!(opa.contains("  enabled: true"));
    assert!(opa.contains("  port: 8082"));
}

/// Renders the chart with `helm template` when helm is installed (CI images
/// without it print a note and skip, the text tests above still bind).
fn render(args: &[&str]) -> Option<String> {
    let chart = format!("{}/../../deploy/helm/egret", env!("CARGO_MANIFEST_DIR"));
    let out = match std::process::Command::new("helm")
        .args(["template", "egret", &chart])
        .args(args)
        .output()
    {
        Ok(o) => o,
        Err(_) => {
            eprintln!("helm not installed: render test skipped");
            return None;
        }
    };
    assert!(
        out.status.success(),
        "helm template failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8(out.stdout).expect("utf8"))
}

/// No OPA port, listen env or NetworkPolicy rule anywhere in the render.
fn assert_no_opa_listener(y: &str) {
    assert!(!y.contains("OPA__LISTEN"));
    assert!(!y.contains("8082"), "no OPA port anywhere in the render");
    assert!(!y.contains("name: opa"));
}

/// Default install: OPA is on in the binary's config but Ranger is off, so
/// nothing opens a port or a NetworkPolicy rule.
#[test]
fn rendered_default_has_opa_on_but_no_listener_while_ranger_is_off() {
    let Some(y) = render(&[]) else { return };
    assert!(y.contains("EGRET_GOVERNANCE__OPA__ENABLED: \"true\""));
    assert!(y.contains("EGRET_RANGER__ENABLED: \"false\""));
    assert_no_opa_listener(&y);
}

#[test]
fn rendered_with_ranger_on_has_the_opa_port_and_rule() {
    let Some(y) = render(&["--set", "ranger.enabled=true"]) else {
        return;
    };
    assert!(y.contains("EGRET_GOVERNANCE__OPA__ENABLED: \"true\""));
    assert!(y.contains("EGRET_GOVERNANCE__OPA__LISTEN: \"0.0.0.0:8082\""));
    assert!(y.contains("name: opa, containerPort: 8082"));
    assert!(y.contains("name: opa, port: 8082"));
    assert!(
        y.contains("port: 8082 }"),
        "the engines' NetworkPolicy rule"
    );
}

#[test]
fn rendered_with_opa_disabled_says_false_and_has_no_opa_port() {
    for extra in [vec![], vec!["--set", "ranger.enabled=true"]] {
        let mut args = vec!["--set", "opa.enabled=false"];
        args.extend(extra);
        let Some(y) = render(&args) else { return };
        assert!(y.contains("EGRET_GOVERNANCE__OPA__ENABLED: \"false\""));
        assert_no_opa_listener(&y);
    }
}
