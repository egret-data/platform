//! The Trino OPA plugin configuration of an engine bound to an OPA profile
//! (governance SP2b Task 13), rendered in ONE place beside
//! [`crate::ranger_config`].
//!
//! egret answers the plugin itself. The plugin reads two files: the selector
//! (`access-control.properties`) and a context file that carries the engine's
//! identity and secret on every request. **The secret lives only in the context
//! file**, and `opa.log-requests` / `opa.log-responses` stay `false`, because a
//! logged request would carry `egret.secret` into the coordinator log.

use crate::ranger_config::RangerPluginFiles;

/// The context file's name.
pub const OPA_CONTEXT_FILE: &str = "egret-opa-context.properties";
/// Where the context file is mounted on the coordinator.
pub const OPA_CONTEXT_PATH: &str = "/etc/trino/egret-opa-context.properties";
/// Where egret serves the OPA surface (equal to `egret_perm_compile`'s; pinned
/// by a test there).
pub const OPA_SURFACE_PREFIX: &str = "/opa/v1/data/egret/trino";
/// Shown in the files an operator installs by hand when the engine id is not repeated.
pub const PLACEHOLDER_ENGINE_ID: &str = "REPLACE_WITH_THE_ENGINE_ID";
/// Shown beside the OPA files.
pub const OPA_LOG_REQUESTS_WARNING: &str = "Do not enable opa.log-requests: every request carries egret.secret in input.context.properties, and the coordinator log would hold it.";
/// C6: egret cannot see an external engine's image.
pub const OPA_EXTERNAL_ENGINE_NOTE: &str = "The coordinator must run Trino 483 or later with plugin/opa; egret cannot check an external engine's image.";

/// How an engine's access decisions are made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessMechanism {
    /// The native Ranger plugin downloads policy from egret.
    #[default]
    Ranger,
    /// The Trino OPA plugin asks egret per decision.
    Opa,
}

/// The two files of the OPA plugin, in order: the selector, then the context
/// file. `secret` must not contain `\n`, `\r` or `\`; the caller refuses such
/// a value (a `RenderableCredential` is URL-safe base64).
pub fn opa_plugin_files(engine_id: &str, egret_url: &str, secret: &str) -> RangerPluginFiles {
    let base = format!("{}{OPA_SURFACE_PREFIX}", egret_url.trim_end_matches('/'));
    let access_control = format!(
        "access-control.name=opa\n\
         opa.policy.uri={base}/allow\n\
         opa.policy.batched-uri={base}/batch\n\
         opa.policy.row-filters-uri={base}/row_filters\n\
         opa.policy.batch-column-masking-uri={base}/batch_column_masks\n\
         opa.policy.column-masking-uri={base}/column_mask\n\
         opa.allow-permission-management-operations=false\n\
         opa.log-requests=false\n\
         opa.log-responses=false\n\
         opa.context-file={OPA_CONTEXT_PATH}\n"
    );
    let context = format!("egret.engine={engine_id}\negret.secret={secret}\n");
    RangerPluginFiles {
        files: vec![
            ("access-control.properties".to_string(), access_control),
            (OPA_CONTEXT_FILE.to_string(), context),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opa_files_point_every_uri_at_egret_and_pin_the_safe_settings() {
        let f = opa_plugin_files("e-1", "http://egret:8080/", "pw");
        let ac = f.get("access-control.properties").unwrap();
        assert!(ac.starts_with("access-control.name=opa\n"));
        assert!(ac.contains("opa.policy.uri=http://egret:8080/opa/v1/data/egret/trino/allow\n"));
        assert!(
            ac.contains("opa.policy.batched-uri=http://egret:8080/opa/v1/data/egret/trino/batch\n")
        );
        assert!(ac.contains(
            "opa.policy.row-filters-uri=http://egret:8080/opa/v1/data/egret/trino/row_filters\n"
        ));
        assert!(ac.contains("opa.policy.batch-column-masking-uri=http://egret:8080/opa/v1/data/egret/trino/batch_column_masks\n"));
        assert!(
            ac.contains("opa.policy.column-masking-uri=http://egret:8080/opa/v1/data/egret/trino/column_mask\n"),
            "both masking URIs, so neither mask path can fail open"
        );
        assert!(ac.contains("opa.allow-permission-management-operations=false\n"));
        assert!(ac.contains("opa.log-requests=false\n"));
        assert!(ac.contains("opa.log-responses=false\n"));
        assert!(ac.contains(&format!("opa.context-file={OPA_CONTEXT_PATH}\n")));
        assert_eq!(
            f.get(OPA_CONTEXT_FILE).unwrap(),
            "egret.engine=e-1\negret.secret=pw\n"
        );
        assert!(
            !ac.contains("pw"),
            "the secret lives only in the context file"
        );
        assert_eq!(f.files[0].0, "access-control.properties");
        assert_eq!(f.files[1].0, OPA_CONTEXT_FILE);
    }

    #[test]
    fn debug_of_the_rendered_files_never_shows_the_secret() {
        let f = opa_plugin_files("e-1", "http://egret:8080", "s3cr3t-value");
        assert!(!format!("{f:?}").contains("s3cr3t-value"));
    }

    #[test]
    fn mechanism_defaults_to_ranger() {
        assert_eq!(AccessMechanism::default(), AccessMechanism::Ranger);
    }
}
