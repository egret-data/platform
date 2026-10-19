//! `egret_perm::ranger_plugin_report` (DG #3, spec §4.4): which Ranger plugin
//! an engine runs and whether egret's evaluator models it. The same five
//! cases `egret-engine-mgmt`'s `EngineDto` tests assert, called directly, with
//! the same notes.
use egret_perm::{ranger_plugin_report, PluginReport};
use serde_json::json;

#[test]
fn a_modelled_external_engine_keeps_its_declared_version() {
    let r = ranger_plugin_report("starrocks", "external", Some("2.9.0"), &json!({}));
    assert_eq!(
        r,
        PluginReport {
            version: Some("2.9.0".into()),
            modelled: true,
            note: None,
        }
    );
}

#[test]
fn an_unmodelled_external_engine_says_which_pairs_are_modelled() {
    let r = ranger_plugin_report("trino", "external", Some("2.9.0"), &json!({}));
    assert_eq!(r.version.as_deref(), Some("2.9.0"));
    assert!(!r.modelled);
    assert_eq!(
        r.note.as_deref(),
        Some(
            format!(
                "Ranger plugin 2.9.0 on a trino engine is not a pair egret's evaluator has been \
                 verified against (it models {}); explain and migrations for its profile refuse",
                egret_perm::modelled_ranger_plugins_sentence()
            )
            .as_str()
        )
    );
}

#[test]
fn an_external_engine_with_no_declared_version_is_unmodelled() {
    let r = ranger_plugin_report("trino", "external", None, &json!({}));
    assert_eq!(
        r,
        PluginReport {
            version: None,
            modelled: false,
            note: Some("no Ranger plugin version is declared".into()),
        }
    );
}

#[test]
fn a_provisioned_engine_with_no_pinned_image_lists_the_known_images() {
    let r = ranger_plugin_report("trino", "provisioned", None, &json!({}));
    assert_eq!(r.version, None);
    assert!(!r.modelled);
    assert_eq!(
        r.note.as_deref(),
        Some(
            format!(
                "the spec pins no image, so egret cannot tell which Ranger plugin it runs; pin \
                 its image to one of: {}",
                egret_perm::engine_images::known_image_refs("trino").join(", ")
            )
            .as_str()
        )
    );
}

#[test]
fn a_provisioned_engine_on_a_known_image_reports_its_plugin() {
    let r = ranger_plugin_report(
        "trino",
        "provisioned",
        None,
        &json!({ "imageVersion": "483" }),
    );
    assert_eq!(
        r,
        PluginReport {
            version: Some("2.8.0".into()),
            modelled: true,
            note: None,
        }
    );
}

#[test]
fn a_provisioned_engine_on_an_unmodelled_image_says_pin() {
    let r = ranger_plugin_report(
        "starrocks",
        "provisioned",
        None,
        &json!({ "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:4.1-latest" } }),
    );
    assert_eq!(r.version, None);
    assert!(!r.modelled);
    let note = r.note.expect("a note");
    assert!(
        note.contains("pin") && note.contains("4.1-latest"),
        "{note}"
    );
}

/// A provisioned engine ignores a declared version: only its image counts.
#[test]
fn a_provisioned_engine_ignores_a_declared_version() {
    let r = ranger_plugin_report("trino", "provisioned", Some("2.9.0"), &json!({}));
    assert!(!r.modelled);
    assert_eq!(r.version, None);
}
