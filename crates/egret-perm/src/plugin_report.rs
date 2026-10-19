//! Which Ranger plugin an engine runs, and whether egret's evaluator models
//! that (engine type, plugin version) pair. Moved here from
//! `egret-engine-mgmt`'s `EngineDto` (DG #3, spec §4.4) so the Governance
//! Overview can judge it without depending on the engine-management crate.
//! The sentences are unchanged.
use crate::engine_images::known_image_refs;
use crate::{
    modelled_ranger_plugins_sentence, plugin_version_for_image, ranger_plugin_is_modelled,
    stored_engine_image,
};

/// The `origin` of an engine egret did not provision.
pub const ORIGIN_EXTERNAL: &str = "external";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginReport {
    /// The plugin version: declared (external) or derived from the pinned
    /// image (provisioned). `None` when it cannot be told.
    pub version: Option<String>,
    /// Whether `(engine_type, version)` is a pair the evaluator models.
    pub modelled: bool,
    /// Why it is not modelled, and how to fix it. `None` when modelled.
    pub note: Option<String>,
}

/// The report for an engine row. An external engine is judged by its declared
/// version; a provisioned one by the image its `spec` pins (a declared
/// version is ignored).
pub fn ranger_plugin_report(
    engine_type: &str,
    origin: &str,
    declared_version: Option<&str>,
    spec: &serde_json::Value,
) -> PluginReport {
    let unmodelled = |version: &str| {
        format!(
            "Ranger plugin {version} on a {engine_type} engine is not a pair egret's evaluator has \
             been verified against (it models {}); explain and migrations for its profile refuse",
            modelled_ranger_plugins_sentence()
        )
    };
    let report = |version: Option<String>, modelled: bool, note: Option<String>| PluginReport {
        version,
        modelled,
        note,
    };
    if origin == ORIGIN_EXTERNAL {
        return match declared_version {
            Some(v) if ranger_plugin_is_modelled(engine_type, v) => {
                report(Some(v.to_string()), true, None)
            }
            Some(v) => report(Some(v.to_string()), false, Some(unmodelled(v))),
            None => report(
                None,
                false,
                Some("no Ranger plugin version is declared".into()),
            ),
        };
    }
    let Some(image) = stored_engine_image(engine_type, spec) else {
        return report(
            None,
            false,
            Some(format!(
                "the spec pins no image, so egret cannot tell which Ranger plugin it runs; pin \
                 its image to one of: {}",
                known_image_refs(engine_type).join(", ")
            )),
        );
    };
    match plugin_version_for_image(engine_type, &image) {
        Ok(v) if ranger_plugin_is_modelled(engine_type, v) => {
            report(Some(v.to_string()), true, None)
        }
        Ok(v) => report(Some(v.to_string()), false, Some(unmodelled(v))),
        Err(sentence) => report(None, false, Some(sentence)),
    }
}
