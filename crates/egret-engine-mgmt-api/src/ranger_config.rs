//! The Ranger plugin configuration of an engine, rendered in ONE place.
//!
//! egret is the Ranger Admin: an engine's Ranger plugin downloads policy from it.
//! For an engine egret provisions, the provisioners mount these files; for an
//! external engine (SP4d-4 Part C) egret never touches the cluster, so its admin
//! installs the same files by hand and egret serves them read-only. Both paths
//! call [`ranger_plugin_files`], so there is one spelling of every key.
//!
//! **No XML comment may be emitted.** StarRocks' FE refuses to boot on a comment
//! containing `--`; the renderer emits no comments at all, so it is safe by
//! construction (`no_starrocks_file_contains_a_double_hyphen`).
//!
//! The files carry no credential: the plugin is configured with a URL and a
//! service name only. egret's optional bearer token is never rendered here.

use crate::EngineKind;

/// Trino: the access-control selector file.
pub const TRINO_ACCESS_CONTROL: &str = "access-control.properties";
/// Trino: the plugin's security configuration.
pub const TRINO_SECURITY_XML: &str = "ranger-trino-security.xml";
/// StarRocks: the line an admin adds to `fe.conf`. The provisioner appends the
/// same text to the `fe.conf` it generates; the name is the admin-facing label.
pub const STARROCKS_FE_CONF_LINE: &str = "fe.conf (add this line)";
/// StarRocks: the plugin's security configuration.
pub const STARROCKS_SECURITY_XML: &str = "ranger-starrocks-security.xml";
/// StarRocks: the plugin's audit configuration.
pub const STARROCKS_AUDIT_XML: &str = "ranger-starrocks-audit.xml";

/// The `fe.conf` line that switches StarRocks' access control to Ranger.
pub const STARROCKS_ACCESS_CONTROL_LINE: &str = "access_control = ranger\n";

/// The files of one engine's Ranger plugin, in a stable order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangerPluginFiles {
    /// `(name, content)`.
    pub files: Vec<(String, String)>,
}

impl RangerPluginFiles {
    /// The content of the file called `name`, if the kind renders one.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| c.as_str())
    }
}

/// The exact files egret mounts into its own engines, for `kind`.
///
/// Trino: `access-control.properties`, `ranger-trino-security.xml`.
/// StarRocks: `fe.conf (add this line)` (`access_control = ranger`),
/// `ranger-starrocks-security.xml`, `ranger-starrocks-audit.xml`.
///
/// `egret_url` is the base the plugin polls (the plugin appends
/// `/service/plugins/policies/download/{service}` itself).
pub fn ranger_plugin_files(
    kind: EngineKind,
    service_name: &str,
    egret_url: &str,
    poll_interval_ms: u64,
) -> RangerPluginFiles {
    let files = match kind {
        EngineKind::Trino => vec![
            (
                TRINO_ACCESS_CONTROL.to_string(),
                format!(
                    "access-control.name=ranger\nranger.service.name={service_name}\nranger.plugin.config.resource=/etc/trino/ranger-trino-security.xml\n"
                ),
            ),
            (
                TRINO_SECURITY_XML.to_string(),
                security_xml("trino", service_name, egret_url, poll_interval_ms),
            ),
        ],
        EngineKind::StarRocks => vec![
            (
                STARROCKS_FE_CONF_LINE.to_string(),
                STARROCKS_ACCESS_CONTROL_LINE.to_string(),
            ),
            (
                STARROCKS_SECURITY_XML.to_string(),
                security_xml("starrocks", service_name, egret_url, poll_interval_ms),
            ),
            (
                STARROCKS_AUDIT_XML.to_string(),
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<configuration>\n\
  <property><name>xasecure.audit.is.enabled</name><value>false</value></property>\n\
</configuration>\n"
                    .to_string(),
            ),
        ],
    };
    RangerPluginFiles { files }
}

fn security_xml(plugin: &str, svc: &str, url: &str, poll: u64) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<configuration>\n\
  <property><name>ranger.plugin.{plugin}.service.name</name><value>{svc}</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.rest.url</name><value>{url}</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.source.impl</name><value>org.apache.ranger.admin.client.RangerAdminRESTClient</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.cache.dir</name><value>/tmp/ranger/{svc}/policycache</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.pollIntervalMs</name><value>{poll}</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.rest.client.connection.timeoutMs</name><value>120000</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.rest.client.read.timeoutMs</name><value>30000</value></property>\n\
</configuration>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://host.docker.internal:8080";

    fn trino() -> RangerPluginFiles {
        ranger_plugin_files(EngineKind::Trino, "demo2", URL, 30_000)
    }
    fn starrocks() -> RangerPluginFiles {
        ranger_plugin_files(EngineKind::StarRocks, "sr1", URL, 30_000)
    }

    /// The files captured from the provisioners BEFORE they were refactored onto
    /// this renderer (`tests/golden/`), byte for byte.
    #[test]
    fn the_renderer_is_byte_identical_to_the_pre_refactor_output() {
        let t = trino();
        assert_eq!(
            t.get(TRINO_ACCESS_CONTROL).unwrap(),
            include_str!("../tests/golden/trino.access-control.properties")
        );
        assert_eq!(
            t.get(TRINO_SECURITY_XML).unwrap(),
            include_str!("../tests/golden/trino.ranger-trino-security.xml")
        );
        assert_eq!(t.files.len(), 2);
        let s = starrocks();
        assert_eq!(
            s.get(STARROCKS_SECURITY_XML).unwrap(),
            include_str!("../tests/golden/starrocks.ranger-starrocks-security.xml")
        );
        assert_eq!(
            s.get(STARROCKS_AUDIT_XML).unwrap(),
            include_str!("../tests/golden/starrocks.ranger-starrocks-audit.xml")
        );
        assert_eq!(
            s.get(STARROCKS_FE_CONF_LINE).unwrap(),
            "access_control = ranger\n"
        );
        assert_eq!(s.files.len(), 3);
    }

    #[test]
    fn no_starrocks_file_contains_a_double_hyphen() {
        // Hostile-looking inputs must not introduce one either: the renderer
        // emits no comment, and the inputs are egret's own names and URL.
        let s = ranger_plugin_files(EngineKind::StarRocks, "sr1", URL, 1);
        for (name, content) in &s.files {
            assert!(!content.contains("--"), "{name} contains a double hyphen");
            assert!(!content.contains("<!--"), "{name} contains an XML comment");
        }
    }

    #[test]
    fn the_file_names_are_exactly_the_kinds_files() {
        let names =
            |f: &RangerPluginFiles| f.files.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
        assert_eq!(names(&trino()), [TRINO_ACCESS_CONTROL, TRINO_SECURITY_XML]);
        assert_eq!(
            names(&starrocks()),
            [
                STARROCKS_FE_CONF_LINE,
                STARROCKS_SECURITY_XML,
                STARROCKS_AUDIT_XML
            ]
        );
    }

    #[test]
    fn the_rendered_files_carry_no_credential() {
        for f in [trino(), starrocks()] {
            for (name, content) in &f.files {
                let lower = content.to_lowercase();
                for w in ["token", "bearer", "password", "secret"] {
                    assert!(!lower.contains(w), "{name} mentions {w}");
                }
            }
        }
    }
}
