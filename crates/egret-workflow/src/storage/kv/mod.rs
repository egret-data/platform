//! W9c: the KV store (`docs/en/specs/2026-10-07-workflow-w9c-kv-store-design.md`).
//!
//! - [`key`]: THE key rule ([`key::check_key`] behind [`key::KvKey`], spec §3).
//! - [`value`]: the seven types, the bytes Kestra writes ([`value::encode`]),
//!   reading them back ([`value::decode`]) and `ttl` ([`value::parse_ttl`])
//!   (spec §4.2).
//! - [`store`]: the metadata table `workflow.kv_entries` (migration 0175) and
//!   the objects behind it. Every function takes the
//!   [`crate::storage::nsfiles::access::Grant`] that `authorize(…,
//!   Resource::Kv, …)` returned and checks it with `require`; the workspace
//!   is the grant's.
//!
//! The access rule is W9b's, in `nsfiles::access` (spec §4.7): one
//! `authorize` for both resources. The objects live in W9a's
//! [`super::FileStore`], under `{prefix}{workspace}/{ns-as-path}/_kv/{key}.ion[.v{N}]`
//! ([`super::FileStore::namespace_kv_uri`]).

pub mod key;
pub mod store;
pub mod value;

/// The advisory-lock text of one key, bound as a value:
/// `w9c:{ws-hyphenated}:{ns}:{key}` (spec §4.3). Every writer of the key
/// takes `pg_advisory_xact_lock(hashtextextended(<this>, 0))`.
// The writers arrive in Task 3; the text is fixed here beside the key rule.
#[allow(dead_code)]
pub(crate) fn kv_lock_text(
    grant: &crate::storage::nsfiles::access::Grant,
    key: &key::KvKey,
) -> String {
    format!(
        "w9c:{}:{}:{}",
        grant.workspace().hyphenated(),
        grant.namespace(),
        key.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::nsfiles::access::{authorize, Access, Actor, Resource};
    use uuid::Uuid;

    #[test]
    fn the_lock_text_names_the_workspace_namespace_and_key() {
        let ws = Uuid::new_v4();
        let g = authorize(ws, Actor::Api, "a.b", Resource::Kv, Access::Write).unwrap();
        let k = key::KvKey::parse("k-1.x").unwrap();
        assert_eq!(
            kv_lock_text(&g, &k),
            format!("w9c:{}:a.b:k-1.x", ws.hyphenated())
        );
    }
}
