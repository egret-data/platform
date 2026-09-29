//! W5b: Kestra plugin tasks (`type: kestra.<FQCN>`), run by the plugin host
//! image built from `plugin-host/` (spec
//! `docs/en/specs/2026-09-29-workflow-w5b-kestra-plugin-host-design.md` §3).
//!
//! [`CATALOG_JSON`] is `EgretPluginHost describe`'s output over the jars
//! `plugin-host/plugins.lock` names, committed and embedded so publish needs
//! no JVM. Regenerate it whenever `plugins.lock` changes:
//!
//! ```bash
//! cd plugin-host
//! ./fetch-plugins.sh target/plugins
//! mvn -q package -DskipTests
//! java -cp 'target/egret-plugin-host.jar:target/lib/*:target/plugins/*' \
//!   io.kestra.core.runners.EgretPluginHost describe target/plugins \
//!   > ../crates/egret-workflow/src/plugins/catalog.json
//! ```

/// `{"lockSha256", "kestra", "classes": {FQCN: {"properties": {name:
/// {"schema", "required"}}, "outputs": [..]}}}`.
// Task 3 is its first reader; until then only the test below reads it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const CATALOG_JSON: &str = include_str!("catalog.json");

#[cfg(test)]
mod tests {
    use super::CATALOG_JSON;
    use sha2::{Digest, Sha256};

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn the_committed_catalog_was_generated_from_the_committed_lockfile() {
        let lock = include_bytes!("../../../../plugin-host/plugins.lock");
        let want = sha256_hex(lock);
        let got: serde_json::Value = serde_json::from_str(CATALOG_JSON).unwrap();
        assert_eq!(
            got["lockSha256"], want,
            "plugins.lock changed; regenerate catalog.json"
        );
    }
}
