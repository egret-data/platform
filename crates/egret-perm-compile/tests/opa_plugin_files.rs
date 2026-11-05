//! Governance SP2b Task 13: the OPA plugin configuration egret renders points
//! at exactly the routes the surface mounts, and keeps the secret out of the
//! selector file.

use egret_engine_mgmt_api::opa_config::{
    opa_plugin_files, OPA_CONTEXT_FILE, OPA_SURFACE_PREFIX, PLACEHOLDER_ENGINE_ID,
};
use egret_perm_compile::opa::surface::{opa_route_paths, OPA_SURFACE_PREFIX as SURFACE_PREFIX};

const ORIGIN: &str = "http://egret:8080";
const URI_KEYS: [&str; 5] = [
    "opa.policy.uri",
    "opa.policy.batched-uri",
    "opa.policy.row-filters-uri",
    "opa.policy.batch-column-masking-uri",
    "opa.policy.column-masking-uri",
];

fn uris(access_control: &str) -> Vec<String> {
    URI_KEYS
        .iter()
        .map(|k| {
            access_control
                .lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")))
                .unwrap_or_else(|| panic!("{k} missing"))
                .to_string()
        })
        .collect()
}

#[test]
fn the_prefix_is_the_surfaces() {
    assert_eq!(OPA_SURFACE_PREFIX, SURFACE_PREFIX);
}

#[test]
fn every_rendered_uri_is_a_mounted_route_and_every_route_is_rendered() {
    let f = opa_plugin_files(PLACEHOLDER_ENGINE_ID, ORIGIN, "pw").unwrap();
    let mut rendered = uris(f.get("access-control.properties").unwrap());
    let mut mounted: Vec<String> = opa_route_paths()
        .into_iter()
        .map(|p| format!("{ORIGIN}{p}"))
        .collect();
    rendered.sort();
    mounted.sort();
    assert_eq!(rendered, mounted);
}

#[test]
fn logging_is_off_and_the_secret_is_only_in_the_context_file() {
    let secret = "S3CRET-value_abc";
    let f = opa_plugin_files("e-1", ORIGIN, secret).unwrap();
    let ac = f.get("access-control.properties").unwrap();
    assert!(ac.contains("opa.log-requests=false\n"));
    assert!(ac.contains("opa.log-responses=false\n"));
    assert!(!ac.contains(secret));
    let ctx = f.get(OPA_CONTEXT_FILE).unwrap();
    assert!(ctx.contains(&format!("egret.secret={secret}\n")));
    assert!(!format!("{f:?}").contains(secret));
}
