//! Which object keys the Storage router's Browse and Download may touch
//! (KI-206, KI-129). Both routes resolve every key through [`scope_key`] and
//! filter every listing through [`StorageScope::visible`], so the rule lives in
//! one place.
//!
//! The rule:
//! - a key is relative to the connection's Key prefix: `sandbox-lake` (prefix
//!   `sandbox/`) browses `sandbox/…`, never the bucket root;
//! - a key is a plain object key: no `.` or `..` segment, no empty segment (a
//!   leading `/` or a `//`), no `\` and no control character, also after
//!   percent-decoding it, so an S3 server that cleans or decodes paths cannot be
//!   walked out of the connection's prefix or into a system prefix;
//! - in the bucket egret itself writes to (`[storage] bucket`), the prefixes
//!   egret owns (`engine.results_prefix`, `apps.storage.prefix`,
//!   `workflow.storage.prefix`) hold other users' data: stored SQL Lab results
//!   (post-mask, per user), Data App sources and workflow files. A caller who is
//!   not a platform admin gets a 403 that says so, and listings leave them
//!   out. The match ignores case. A platform admin can open them: the role
//!   already holds the platform's storage credentials and every other bypass.

use std::sync::Arc;

use egret_storage::{ObjectStore, StorageConfig, StorageError};

use crate::error::CpError;

/// Opens the object store a storage connection points at. The real one builds
/// an [`egret_storage::S3Store`]; tests pass a fake.
#[async_trait::async_trait]
pub trait ConnStoreOpener: Send + Sync {
    async fn open(&self, cfg: &StorageConfig) -> Result<Arc<dyn ObjectStore>, StorageError>;
}

/// [`ConnStoreOpener`] over S3 (RustFS, MinIO, AWS).
pub struct S3StoreOpener;

#[async_trait::async_trait]
impl ConnStoreOpener for S3StoreOpener {
    async fn open(&self, cfg: &StorageConfig) -> Result<Arc<dyn ObjectStore>, StorageError> {
        Ok(Arc::new(egret_storage::S3Store::new(cfg).await?))
    }
}

/// The platform's own bucket and the key prefixes egret writes inside it.
#[derive(Clone, Debug, Default)]
pub struct SystemPrefixes {
    bucket: String,
    /// Lower-cased, without leading `/`, ending with exactly one `/`.
    prefixes: Vec<String>,
}

impl SystemPrefixes {
    /// `bucket` is `[storage] bucket`; `prefixes` the key prefixes egret writes
    /// there (`query-results`, `apps/`, `workflow/`). Empty prefixes are
    /// dropped: they would cover the whole bucket.
    pub fn new<'a>(bucket: &str, prefixes: impl IntoIterator<Item = &'a str>) -> Self {
        let prefixes = prefixes
            .into_iter()
            .map(|p| p.trim_matches('/').to_lowercase())
            .filter(|p| !p.is_empty())
            .map(|p| format!("{p}/"))
            .collect();
        Self {
            bucket: bucket.to_string(),
            prefixes,
        }
    }

    /// The system prefix `abs_key` (a key from the bucket root) falls in, when
    /// `bucket` is the platform's. A key equal to a prefix without its `/`
    /// counts too.
    fn covering(&self, bucket: &str, abs_key: &str) -> Option<&str> {
        if self.bucket.is_empty() || !self.bucket.eq_ignore_ascii_case(bucket) {
            return None;
        }
        let key = abs_key.to_lowercase();
        self.prefixes
            .iter()
            .find(|p| key.starts_with(p.as_str()) || key == p[..p.len() - 1])
            .map(String::as_str)
    }
}

/// The Storage router's object access: how to open a connection's store and
/// which prefixes are egret's own.
#[derive(Clone)]
pub struct StorageScope {
    pub system: SystemPrefixes,
    pub opener: Arc<dyn ConnStoreOpener>,
}

impl StorageScope {
    /// Real S3 stores, `system` hidden.
    pub fn s3(system: SystemPrefixes) -> Self {
        Self {
            system,
            opener: Arc::new(S3StoreOpener),
        }
    }
}

/// What the caller asked for: a folder to list (may be empty, may end in `/`)
/// or one object (non-empty, no trailing `/`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum KeyKind {
    Folder,
    Object,
}

/// One connection seen by one caller: the connection's bucket and normalised
/// Key prefix, and whether system prefixes are open to the caller.
pub(crate) struct ConnView<'a> {
    pub system: &'a SystemPrefixes,
    pub bucket: &'a str,
    /// `""`, or the Key prefix without leading `/`, ending with one `/`.
    pub root: String,
    pub admin: bool,
}

impl<'a> ConnView<'a> {
    /// `key_prefix` is the connection's stored Key prefix (`sandbox`,
    /// `sandbox/` or none). A malformed stored prefix is refused rather than
    /// trusted.
    pub fn new(
        system: &'a SystemPrefixes,
        bucket: &'a str,
        key_prefix: Option<&str>,
        admin: bool,
    ) -> Result<Self, CpError> {
        let trimmed = key_prefix.unwrap_or("").trim_matches('/');
        let root = if trimmed.is_empty() {
            String::new()
        } else {
            check_shape(trimmed, KeyKind::Object)
                .map_err(|why| CpError::BadRequest(format!("the connection's key prefix {why}")))?;
            format!("{trimmed}/")
        };
        Ok(Self {
            system,
            bucket,
            root,
            admin,
        })
    }

    /// Resolve a key the caller gave (relative to the connection's Key
    /// prefix) to the key from the bucket root, or refuse it.
    pub fn scope_key(&self, raw: &str, kind: KeyKind) -> Result<String, CpError> {
        if kind == KeyKind::Object && raw.is_empty() {
            return Err(CpError::BadRequest("key is empty".into()));
        }
        for form in decoded_forms(raw) {
            check_shape(&form, kind).map_err(|why| CpError::BadRequest(format!("key {why}")))?;
            let abs = format!("{}{form}", self.root);
            if let Some(p) = self.system_prefix(&abs) {
                return Err(CpError::ForbiddenBecause(format!(
                    "{p} holds data egret writes for its users (stored SQL Lab results, \
                     Data App sources, workflow files), so it cannot be browsed or \
                     downloaded with storage:read; only a platform admin can open it"
                )));
            }
        }
        Ok(format!("{}{raw}", self.root))
    }

    /// The listing entry `abs_key` (from the bucket root) as the caller sees
    /// it: relative to the Key prefix, or `None` when it is outside the prefix
    /// or under a system prefix the caller may not open.
    pub fn visible(&self, abs_key: &str) -> Option<String> {
        let rel = abs_key.strip_prefix(self.root.as_str())?;
        if self.system_prefix(abs_key).is_some() {
            return None;
        }
        Some(rel.to_string())
    }

    /// Whether a listing entry was left out because it is a system prefix.
    pub fn is_hidden_system(&self, abs_key: &str) -> bool {
        abs_key.starts_with(self.root.as_str()) && self.system_prefix(abs_key).is_some()
    }

    fn system_prefix(&self, abs_key: &str) -> Option<&str> {
        if self.admin {
            return None;
        }
        self.system.covering(self.bucket, abs_key)
    }
}

/// `raw`, then each further percent-decoding of it until it stops changing
/// (bounded): a server that decodes once more must not see a different path.
fn decoded_forms(raw: &str) -> Vec<String> {
    let mut forms = vec![raw.to_string()];
    for _ in 0..4 {
        let last = forms.last().expect("non-empty");
        let next = percent_decode(last);
        if &next == last {
            break;
        }
        forms.push(next);
    }
    forms
}

/// Decode `%XX` sequences; a `%` not followed by two hex digits stays as is.
/// Bytes that are not UTF-8 are replaced, which can only make a key fail a
/// later check, never pass one.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A plain object key: segments split by `/`, none of them empty, `.` or
/// `..`; no `\` and no control character. A folder may be empty and may end
/// with one `/`. Returns why not, as the end of a sentence.
fn check_shape(key: &str, kind: KeyKind) -> Result<(), String> {
    if key.is_empty() {
        return Ok(());
    }
    if key.chars().any(|c| c.is_control()) {
        return Err("contains a control character".into());
    }
    if key.contains('\\') {
        return Err("contains a backslash".into());
    }
    let body = match kind {
        KeyKind::Folder => key.strip_suffix('/').unwrap_or(key),
        KeyKind::Object => key,
    };
    for seg in body.split('/') {
        match seg {
            "" => {
                return Err("has an empty path segment (a leading, trailing or double '/')".into())
            }
            "." | ".." => return Err(format!("has a '{seg}' path segment")),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys() -> SystemPrefixes {
        SystemPrefixes::new("warehouse", ["query-results", "apps/", "workflow/"])
    }

    fn view<'a>(s: &'a SystemPrefixes, prefix: Option<&str>, admin: bool) -> ConnView<'a> {
        ConnView::new(s, "warehouse", prefix, admin).unwrap()
    }

    fn refused_403(r: Result<String, CpError>) -> bool {
        matches!(r, Err(CpError::ForbiddenBecause(_)))
    }

    fn refused_400(r: Result<String, CpError>) -> bool {
        matches!(r, Err(CpError::BadRequest(_)))
    }

    #[test]
    fn a_system_prefix_is_refused_to_a_non_admin_in_every_spelling() {
        let s = sys();
        let v = view(&s, None, false);
        for key in [
            "query-results/a.jsonl",
            "Query-Results/a.jsonl",
            "QUERY-RESULTS/a.jsonl",
            "query%2Dresults/a.jsonl",
            "query-results%2Fa.jsonl",
            "query%252Dresults/a.jsonl",
            "apps/ws/app/draft/1.py",
            "workflow/ws/f/executions/e/x",
        ] {
            assert!(refused_403(v.scope_key(key, KeyKind::Object)), "{key}");
        }
        for folder in ["query-results/", "query-results", "QUERY-results/", "apps/"] {
            assert!(
                refused_403(v.scope_key(folder, KeyKind::Folder)),
                "{folder}"
            );
        }
    }

    #[test]
    fn path_tricks_are_refused_before_any_prefix_check() {
        let s = sys();
        let v = view(&s, Some("sandbox"), false);
        for key in [
            "../query-results/a.jsonl",
            "x/../../query-results/a.jsonl",
            "/query-results/a.jsonl",
            "a//b",
            "./a",
            "%2e%2e/query-results/a.jsonl",
            "%252e%252e/query-results/a.jsonl",
            "..%2fquery-results/a.jsonl",
            "a\\..\\b",
            "a%5c..",
            "a\nb",
            "a/",
        ] {
            assert!(refused_400(v.scope_key(key, KeyKind::Object)), "{key}");
        }
        for folder in ["../", "/", "//", "a//", "%2e%2e/"] {
            assert!(
                refused_400(v.scope_key(folder, KeyKind::Folder)),
                "{folder}"
            );
        }
    }

    #[test]
    fn a_normal_key_resolves_under_the_connection_prefix() {
        let s = sys();
        assert_eq!(
            view(&s, Some("sandbox"), false)
                .scope_key("data/x.parquet", KeyKind::Object)
                .unwrap(),
            "sandbox/data/x.parquet"
        );
        assert_eq!(
            view(&s, Some("/sandbox/"), false)
                .scope_key("", KeyKind::Folder)
                .unwrap(),
            "sandbox/"
        );
        assert_eq!(
            view(&s, None, false)
                .scope_key("demo/", KeyKind::Folder)
                .unwrap(),
            "demo/"
        );
        // A name that only starts like a system prefix is not one.
        assert_eq!(
            view(&s, None, false)
                .scope_key("query-results-old/a", KeyKind::Object)
                .unwrap(),
            "query-results-old/a"
        );
    }

    #[test]
    fn a_platform_admin_opens_system_prefixes_but_not_path_tricks() {
        let s = sys();
        let v = view(&s, None, true);
        assert_eq!(
            v.scope_key("query-results/a.jsonl", KeyKind::Object)
                .unwrap(),
            "query-results/a.jsonl"
        );
        assert!(refused_400(v.scope_key("../x", KeyKind::Object)));
        assert_eq!(
            v.visible("query-results/").as_deref(),
            Some("query-results/")
        );
    }

    #[test]
    fn a_connection_prefix_inside_a_system_prefix_is_closed_too() {
        let s = sys();
        let v = view(&s, Some("query-results"), false);
        assert!(refused_403(v.scope_key("", KeyKind::Folder)));
        assert!(refused_403(v.scope_key("a.jsonl", KeyKind::Object)));
    }

    #[test]
    fn another_bucket_has_no_system_prefixes() {
        let s = sys();
        let v = ConnView::new(&s, "lake", None, false).unwrap();
        assert!(v.scope_key("query-results/a", KeyKind::Object).is_ok());
    }

    #[test]
    fn listings_drop_system_entries_and_entries_outside_the_prefix() {
        let s = sys();
        let v = view(&s, None, false);
        assert_eq!(v.visible("query-results/"), None);
        assert_eq!(v.visible("Apps/"), None);
        assert_eq!(v.visible("demo/").as_deref(), Some("demo/"));
        assert!(v.is_hidden_system("workflow/"));
        let v = view(&s, Some("sandbox"), false);
        assert_eq!(v.visible("sandbox/a.csv").as_deref(), Some("a.csv"));
        assert_eq!(v.visible("other/a.csv"), None);
    }

    #[test]
    fn a_malformed_stored_key_prefix_is_refused() {
        let s = sys();
        assert!(ConnView::new(&s, "warehouse", Some("a/../query-results"), false).is_err());
        assert!(ConnView::new(&s, "warehouse", Some("a//b"), false).is_err());
    }
}
