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

/// Task 22: without `opa.policy.batched-uri`, `information_schema.columns` fails every time.
pub const OPA_BATCHED_URI_WARNING: &str = "Keep opa.policy.batched-uri in access-control.properties: without it, queries on information_schema.columns fail.";

/// How an engine's access decisions are made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessMechanism {
    /// The native Ranger plugin downloads policy from egret.
    #[default]
    Ranger,
    /// The Trino OPA plugin asks egret per decision.
    Opa,
}

/// Why an OPA binding has no context file to render.
pub const NO_OPA_CREDENTIAL: &str = "no engine credential for the OPA context file";

/// A value that cannot be written into a Java `.properties` line as is, or
/// (`missing`) is not there at all. Names the field, never the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnrenderableOpaValue {
    pub field: &'static str,
    /// The value is absent: an OPA binding with no engine credential, which
    /// would otherwise render an empty `egret.secret=`.
    pub missing: bool,
}

impl UnrenderableOpaValue {
    /// The binding carries no engine credential ([`NO_OPA_CREDENTIAL`]).
    pub fn missing_credential() -> Self {
        Self {
            field: "secret",
            missing: true,
        }
    }
}

impl std::fmt::Display for UnrenderableOpaValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.missing {
            return f.write_str(NO_OPA_CREDENTIAL);
        }
        write!(
            f,
            "the OPA plugin's {} contains a character that is unsafe in a .properties file",
            self.field
        )
    }
}

impl std::error::Error for UnrenderableOpaValue {}

/// Refuses (never escapes) a value unsafe in a `.properties` line: an ASCII
/// control character (a line break would inject a property) or `\`. With
/// `strict`, also `=`, `:` and leading or trailing whitespace.
fn check_properties_value(
    field: &'static str,
    value: &str,
    strict: bool,
) -> Result<(), UnrenderableOpaValue> {
    let bad = value.chars().any(|c| c.is_ascii_control() || c == '\\')
        || (strict && (value.contains(['=', ':']) || value.trim() != value));
    if bad {
        Err(UnrenderableOpaValue {
            field,
            missing: false,
        })
    } else {
        Ok(())
    }
}

/// The two files of the OPA plugin, in order: the selector, then the context
/// file. Nothing guarantees the secret's alphabet (`RenderableCredential` only
/// rejects `<`, `&` and `--`), so every interpolated value is checked here and
/// an unsafe one is refused ([`check_properties_value`]).
pub fn opa_plugin_files(
    engine_id: &str,
    egret_url: &str,
    secret: &str,
) -> Result<RangerPluginFiles, UnrenderableOpaValue> {
    check_properties_value("secret", secret, true)?;
    check_properties_value("engine id", engine_id, false)?;
    check_properties_value("egret URL", egret_url, false)?;
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
    Ok(RangerPluginFiles {
        files: vec![
            ("access-control.properties".to_string(), access_control),
            (OPA_CONTEXT_FILE.to_string(), context),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opa_files_point_every_uri_at_egret_and_pin_the_safe_settings() {
        let f = opa_plugin_files("e-1", "http://egret:8080/", "pw").unwrap();
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
        assert!(ac.contains("opa.context-file=/etc/trino/egret-opa-context.properties\n"));
        assert_eq!(OPA_CONTEXT_PATH, "/etc/trino/egret-opa-context.properties");
        let b = "http://egret:8080/opa/v1/data/egret/trino";
        assert_eq!(
            ac,
            &format!(
                "access-control.name=opa\nopa.policy.uri={b}/allow\nopa.policy.batched-uri={b}/batch\nopa.policy.row-filters-uri={b}/row_filters\nopa.policy.batch-column-masking-uri={b}/batch_column_masks\nopa.policy.column-masking-uri={b}/column_mask\nopa.allow-permission-management-operations=false\nopa.log-requests=false\nopa.log-responses=false\nopa.context-file=/etc/trino/egret-opa-context.properties\n"
            ),
            "the whole file, so an appended contradictory line is caught"
        );
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
        let f = opa_plugin_files("e-1", "http://egret:8080", "s3cr3t-value").unwrap();
        assert!(!format!("{f:?}").contains("s3cr3t-value"));
    }

    #[test]
    fn a_value_that_would_inject_a_property_is_refused() {
        let url = "http://egret:8080";
        for bad in [
            "x\nopa.log-requests=true",
            "x\ropa.log-requests=true",
            "a\\b",
            "a=b",
            "a:b",
            " lead",
            "trail ",
            "nul\0",
        ] {
            let e = opa_plugin_files("e-1", url, bad).unwrap_err();
            assert_eq!(e.field, "secret", "{bad:?}");
            assert!(
                !e.to_string().contains(bad.trim()),
                "the value is never echoed"
            );
        }
        assert_eq!(
            opa_plugin_files("e\n1", url, "pw").unwrap_err().field,
            "engine id"
        );
        assert_eq!(
            opa_plugin_files("e-1", "http://egret\nx=y", "pw")
                .unwrap_err()
                .field,
            "egret URL"
        );
        // A real URL-safe base64 secret passes.
        assert!(opa_plugin_files("e-1", url, "Ab-_09xyz").is_ok());
    }

    #[test]
    fn mechanism_defaults_to_ranger() {
        assert_eq!(AccessMechanism::default(), AccessMechanism::Ranger);
    }
}
