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
//! The security XML carries the engine's plugin credential (HTTP Basic
//! `engine-<uuid>` + its password, spec 2026-10-20-ranger-surface-auth A9) only
//! when the caller passes one. egret's own engines get it through a Kubernetes
//! Secret, never a ConfigMap. egret's optional bearer token is never rendered.

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

/// How often an engine's Ranger plugin polls egret for policy, in milliseconds:
/// one value for the engines egret provisions, the files an external engine's
/// admin installs and the files the policy admin API serves.
pub const RANGER_POLL_INTERVAL_MS: u32 = 30_000;

/// The `fe.conf` line that switches StarRocks' access control to Ranger.
pub const STARROCKS_ACCESS_CONTROL_LINE: &str = "access_control = ranger\n";

/// Shown in the files an operator installs by hand when the password is not repeated.
pub const PASSWORD_PLACEHOLDER: &str = "REPLACE_WITH_THE_ENGINE_PASSWORD";

/// Why a credential cannot be rendered: it is written UNESCAPED into the
/// plugin's security XML, so it must not carry `<` or `&` (markup), nor `--`
/// (StarRocks' FE refuses a double hyphen in its config XML).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnrenderableCredential {
    /// `"username"` or `"password"` — never the value.
    pub field: &'static str,
}

impl std::fmt::Display for UnrenderableCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the plugin credential's {} contains `--`, `<` or `&`, so it cannot be rendered \
             into the Ranger plugin's XML",
            self.field
        )
    }
}

impl std::error::Error for UnrenderableCredential {}

fn check_renderable(username: &str, password: &str) -> Result<(), UnrenderableCredential> {
    let bad = |v: &str| v.contains('<') || v.contains('&') || v.contains("--");
    if bad(username) {
        return Err(UnrenderableCredential { field: "username" });
    }
    if bad(password) {
        return Err(UnrenderableCredential { field: "password" });
    }
    Ok(())
}

/// The credential an engine's plugin presents to egret's `/ranger` surface,
/// checked renderable ([`UnrenderableCredential`]) in every build: the only way
/// to make one is [`RenderableCredential::new`], so the renderer can never be
/// handed a value it would have to escape. Its `Debug` redacts the password.
#[derive(Clone, PartialEq, Eq)]
pub struct RenderableCredential {
    username: String,
    password: String,
}

impl RenderableCredential {
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self, UnrenderableCredential> {
        let (username, password) = (username.into(), password.into());
        check_renderable(&username, &password)?;
        Ok(Self { username, password })
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    /// The borrowed form [`ranger_plugin_files`] takes.
    pub fn as_ref(&self) -> PluginCredentialRef<'_> {
        PluginCredentialRef {
            username: &self.username,
            password: &self.password,
        }
    }
}

impl std::fmt::Debug for RenderableCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderableCredential")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// A borrowed, already-checked credential for [`ranger_plugin_files`]. Built by
/// [`PluginCredentialRef::new`] (checked) or [`RenderableCredential::as_ref`].
#[derive(Clone, Copy)]
pub struct PluginCredentialRef<'a> {
    username: &'a str,
    password: &'a str,
}

impl<'a> PluginCredentialRef<'a> {
    pub fn new(username: &'a str, password: &'a str) -> Result<Self, UnrenderableCredential> {
        check_renderable(username, password)?;
        Ok(Self { username, password })
    }
}

impl std::fmt::Debug for PluginCredentialRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginCredentialRef")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// The files of one engine's Ranger plugin, in a stable order.
///
/// Its `Debug` prints the file names only: the security XML may carry a
/// password.
#[derive(Clone, PartialEq, Eq)]
pub struct RangerPluginFiles {
    /// `(name, content)`.
    pub files: Vec<(String, String)>,
}

impl std::fmt::Debug for RangerPluginFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RangerPluginFiles")
            .field(
                "files",
                &self
                    .files
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
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

/// Where egret serves the Ranger plugin surface. The plugin appends
/// `/service/plugins/...` to its configured URL, so the URL it is given is
/// `<egret>` + this prefix. One spelling, used by the server and the renderer.
pub const RANGER_SURFACE_PREFIX: &str = "/ranger";

/// The URL an engine's plugin is configured with.
pub fn plugin_base_url(egret_url: &str) -> String {
    format!("{}{RANGER_SURFACE_PREFIX}", egret_url.trim_end_matches('/'))
}

/// The exact files egret mounts into its own engines, for `kind`.
///
/// Trino: `access-control.properties`, `ranger-trino-security.xml`.
/// StarRocks: `fe.conf (add this line)` (`access_control = ranger`),
/// `ranger-starrocks-security.xml`, `ranger-starrocks-audit.xml`.
///
/// `egret_url` is egret's own URL; the plugin is configured with
/// [`plugin_base_url`] of it and appends
/// `/service/plugins/policies/download/{service}` itself.
///
/// `credential`, when `Some`, adds `policy.rest.client.username` and
/// `.password` to the security XML; no other file changes.
pub fn ranger_plugin_files(
    kind: EngineKind,
    service_name: &str,
    egret_url: &str,
    poll_interval_ms: u64,
    credential: Option<PluginCredentialRef<'_>>,
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
                security_xml("trino", service_name, egret_url, poll_interval_ms, credential),
            ),
        ],
        EngineKind::StarRocks => vec![
            (
                STARROCKS_FE_CONF_LINE.to_string(),
                STARROCKS_ACCESS_CONTROL_LINE.to_string(),
            ),
            (
                STARROCKS_SECURITY_XML.to_string(),
                security_xml(
                    "starrocks",
                    service_name,
                    egret_url,
                    poll_interval_ms,
                    credential,
                ),
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

fn security_xml(
    plugin: &str,
    svc: &str,
    url: &str,
    poll: u64,
    credential: Option<PluginCredentialRef<'_>>,
) -> String {
    let base = plugin_base_url(url);
    // Nothing is escaped: a `PluginCredentialRef` can only be built checked
    // (`check_renderable`), so neither value carries XML markup or a `--`.
    let creds = credential
        .map(|c| {
            format!(
                "<property><name>ranger.plugin.{plugin}.policy.rest.client.username</name><value>{u}</value></property>\n\
<property><name>ranger.plugin.{plugin}.policy.rest.client.password</name><value>{p}</value></property>\n",
                u = c.username,
                p = c.password
            )
        })
        .unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<configuration>\n\
  <property><name>ranger.plugin.{plugin}.service.name</name><value>{svc}</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.rest.url</name><value>{base}</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.source.impl</name><value>org.apache.ranger.admin.client.RangerAdminRESTClient</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.cache.dir</name><value>/tmp/ranger/{svc}/policycache</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.pollIntervalMs</name><value>{poll}</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.rest.client.connection.timeoutMs</name><value>120000</value></property>\n\
  <property><name>ranger.plugin.{plugin}.policy.rest.client.read.timeoutMs</name><value>30000</value></property>\n\
{creds}</configuration>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://host.docker.internal:8080";

    fn trino() -> RangerPluginFiles {
        ranger_plugin_files(
            EngineKind::Trino,
            "demo2",
            URL,
            RANGER_POLL_INTERVAL_MS.into(),
            None,
        )
    }
    fn starrocks() -> RangerPluginFiles {
        ranger_plugin_files(
            EngineKind::StarRocks,
            "sr1",
            URL,
            RANGER_POLL_INTERVAL_MS.into(),
            None,
        )
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
        // emits no comment, and the inputs are egret's own names and URL. A
        // credential is URL-safe base64 (`-` and `_` occur, never `--`).
        for credential in [
            None,
            Some(
                PluginCredentialRef::new(
                    "engine-0b6f6c9e-3f5e-4c1a-9a43-2b1d1f3c4e5a",
                    "a-b_c-d_e-F0-9_zZ",
                )
                .unwrap(),
            ),
        ] {
            let s = ranger_plugin_files(EngineKind::StarRocks, "sr1", URL, 1, credential);
            for (name, content) in &s.files {
                assert!(!content.contains("--"), "{name} contains a double hyphen");
                assert!(!content.contains("<!--"), "{name} contains an XML comment");
            }
        }
    }

    #[test]
    fn the_security_xml_carries_the_credential_only_when_given() {
        let none = ranger_plugin_files(EngineKind::Trino, "svc", "http://e", 30_000, None);
        assert!(!none
            .get(TRINO_SECURITY_XML)
            .unwrap()
            .contains("rest.client.password"));
        let with = ranger_plugin_files(
            EngineKind::Trino,
            "svc",
            "http://e",
            30_000,
            Some(PluginCredentialRef::new("engine-1", "pw").unwrap()),
        );
        let xml = with.get(TRINO_SECURITY_XML).unwrap();
        assert!(xml.contains(
            "<name>ranger.plugin.trino.policy.rest.client.username</name><value>engine-1</value>"
        ));
        assert!(xml.contains(
            "<name>ranger.plugin.trino.policy.rest.client.password</name><value>pw</value>"
        ));
        assert!(xml.contains("<value>http://e/ranger</value>"));
        // Still well-formed: the two properties sit inside `<configuration>`.
        assert!(xml.ends_with("</configuration>\n"));
        assert_eq!(xml.matches("</configuration>").count(), 1);
    }

    /// The credential changes the security XML and NOTHING else: the other
    /// files are byte-identical with and without it.
    #[test]
    fn the_credential_changes_only_the_security_xml() {
        let cred = PluginCredentialRef::new("engine-1", "pw").unwrap();
        for (kind, security) in [
            (EngineKind::Trino, TRINO_SECURITY_XML),
            (EngineKind::StarRocks, STARROCKS_SECURITY_XML),
        ] {
            let a = ranger_plugin_files(kind, "svc", URL, 1, None);
            let b = ranger_plugin_files(kind, "svc", URL, 1, Some(cred));
            assert_eq!(a.files.len(), b.files.len());
            for ((na, ca), (nb, cb)) in a.files.iter().zip(&b.files) {
                assert_eq!(na, nb);
                if na == security {
                    assert_ne!(ca, cb);
                    let plugin = if kind == EngineKind::Trino {
                        "trino"
                    } else {
                        "starrocks"
                    };
                    assert!(cb.contains(&format!(
                        "<name>ranger.plugin.{plugin}.policy.rest.client.username</name>"
                    )));
                } else {
                    assert_eq!(ca, cb, "{na} changed with a credential");
                }
            }
        }
    }

    /// The check holds in every build, not only under `debug_assertions`: a
    /// credential that would need escaping is an error, never rendered.
    #[test]
    fn a_credential_that_would_need_escaping_is_refused_not_rendered() {
        for (u, p, field) in [
            ("engine-1", "abc--def", "password"),
            ("engine-1", "a<b", "password"),
            ("engine-1", "a&b", "password"),
            ("engine--1", "pw", "username"),
        ] {
            assert_eq!(
                RenderableCredential::new(u, p).unwrap_err(),
                UnrenderableCredential { field },
                "{u} / {p}"
            );
            assert_eq!(
                PluginCredentialRef::new(u, p).unwrap_err(),
                UnrenderableCredential { field }
            );
        }
        let err = RenderableCredential::new("engine-1", "s3cr3t--x").unwrap_err();
        assert!(
            !err.to_string().contains("s3cr3t"),
            "no value in the message"
        );
        let ok = RenderableCredential::new("engine-1", "a-b_c").unwrap();
        assert_eq!((ok.username(), ok.password()), ("engine-1", "a-b_c"));
        assert!(!format!("{ok:?}").contains("a-b_c"));
    }

    #[test]
    fn the_placeholder_is_not_a_plausible_password() {
        // It is shown in place of the password; it must never pass for one.
        assert!(PASSWORD_PLACEHOLDER
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_'));
        assert!(!PASSWORD_PLACEHOLDER.contains("--"));
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

    // -- the committed deploy files cannot drift from the renderer -------------

    /// The StarRocks files `deploy/docker-compose.dev.yml` mounts. They are kept
    /// (compose mounts them) but the renderer owns their content.
    const DEPLOY_SECURITY_XML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/starrocks/ranger/ranger-starrocks-security.xml"
    ));
    const DEPLOY_AUDIT_XML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/starrocks/ranger/ranger-starrocks-audit.xml"
    ));

    /// The service name and URL the deploy security file is written for.
    const DEPLOY_SERVICE: &str = "sr1";
    const DEPLOY_URL: &str = "http://host.docker.internal:8080";

    /// The deploy files differ from the renderer in two ways that carry no
    /// meaning to Hadoop's configuration parser: an explanatory XML comment (the
    /// renderer emits none by design, see the module doc) and two-space
    /// indentation (the renderer's string literals continue with `\`, which
    /// drops it; the byte-identity golden above pins that output, so it is the
    /// deploy files that are indented, not the renderer that is wrong). The
    /// comparison is on everything else: every property name and value.
    fn comparable(xml: &str) -> String {
        without_xml_comments(xml)
            .lines()
            .map(str::trim_start)
            .map(|l| format!("{l}\n"))
            .collect()
    }

    fn without_xml_comments(xml: &str) -> String {
        let mut out = String::new();
        let mut rest = xml;
        while let Some(start) = rest.find("<!--") {
            out.push_str(&rest[..start]);
            let end = rest[start..]
                .find("-->")
                .expect("an unterminated XML comment");
            rest = rest[start + end + "-->".len()..].trim_start_matches('\n');
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn the_deploy_starrocks_files_equal_the_renderer_output_but_for_comments() {
        let rendered = ranger_plugin_files(
            EngineKind::StarRocks,
            DEPLOY_SERVICE,
            DEPLOY_URL,
            RANGER_POLL_INTERVAL_MS.into(),
            None,
        );
        assert_eq!(
            comparable(DEPLOY_SECURITY_XML),
            rendered.get(STARROCKS_SECURITY_XML).unwrap(),
            "deploy/starrocks/ranger/{STARROCKS_SECURITY_XML} drifted from the renderer"
        );
        assert_eq!(
            comparable(DEPLOY_AUDIT_XML),
            rendered.get(STARROCKS_AUDIT_XML).unwrap(),
            "deploy/starrocks/ranger/{STARROCKS_AUDIT_XML} drifted from the renderer"
        );
    }

    #[test]
    fn stripping_comments_removes_only_the_comment() {
        assert_eq!(
            without_xml_comments("<?xml?>\n<!--\n a - b\n-->\n<a/>\n"),
            "<?xml?>\n<a/>\n"
        );
    }
}
