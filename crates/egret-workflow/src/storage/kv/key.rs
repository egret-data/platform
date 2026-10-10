//! THE key rule of the KV store (W9c spec §3): one function, [`check_key`],
//! behind [`KvKey::parse`], for `kv()`, the tasks and the API.
//!
//! Kestra's regex `^[a-zA-Z0-9][a-zA-Z0-9._-]*$` (`KVStore.java:137`) and at
//! most [`MAX_KEY_BYTES`] bytes, Kestra's own column width
//! (`kv_metadata.name VARCHAR(350)`). The key is ASCII by the regex, so bytes
//! equal characters. A key never holds `/` (it is one object-key segment) and
//! never starts with `_` (no key is named like a marker). Migration 0175's
//! CHECK on `workflow.kv_entries.key` is the same rule.

/// The longest key, in bytes.
pub const MAX_KEY_BYTES: usize = 350;

/// Kestra's message for an empty key (w9c `keys.<empty>.set`).
const EMPTY_KEY: &str = "Key cannot be null or empty";

/// Kestra's message for a key that breaks the regex (w9c `keys._a.set`).
const BAD_KEY: &str = "Key must start with an alphanumeric character (uppercase or lowercase) \
     and can contain alphanumeric characters (uppercase or lowercase), dots (.), underscores \
     (_), and hyphens (-) only.";

/// A key that passed [`check_key`]. The only way to build one is
/// [`KvKey::parse`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KvKey(String);

impl KvKey {
    /// [`check_key`], then the key. Never trims.
    pub fn parse(raw: &str) -> Result<Self, String> {
        check_key(raw)?;
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for KvKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Kestra's messages (w9c `keys`) for an empty key and a key that breaks
/// `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`; over 350 bytes: "key '<first 40>…' is N
/// bytes long; a KV key is at most 350 bytes (Kestra's own column width)".
pub fn check_key(raw: &str) -> Result<(), String> {
    let Some(first) = raw.bytes().next() else {
        return Err(EMPTY_KEY.to_string());
    };
    let rest_ok = raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !first.is_ascii_alphanumeric() || !rest_ok {
        return Err(BAD_KEY.to_string());
    }
    if raw.len() > MAX_KEY_BYTES {
        // ASCII by the check above, so slicing at 40 is a char boundary.
        return Err(format!(
            "key '{}…' is {} bytes long; a KV key is at most {MAX_KEY_BYTES} bytes \
             (Kestra's own column width)",
            &raw[..40],
            raw.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture's `keys` cells: each raw key and the `set` cell's answer.
    fn artifact_keys() -> Vec<(String, serde_json::Value)> {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/oracle-exec/w9c-kv.json"
        )))
        .expect("the fixture parses");
        let keys = fixture["egret_expected"]["keys"]
            .as_object()
            .expect("keys")
            .clone();
        keys.into_iter()
            .filter(|(_, cell)| cell.get("set").is_some())
            .map(|(name, cell)| {
                let raw = match name.as_str() {
                    "<empty>" => String::new(),
                    "k*350" => "k".repeat(350),
                    "k*351" => "k".repeat(351),
                    other => other.to_string(),
                };
                (raw, cell["set"].clone())
            })
            .collect()
    }

    #[test]
    fn the_keys_match_the_artifact() {
        let cells = artifact_keys();
        assert_eq!(cells.len(), 12, "every keys row is examined");
        for (raw, set) in cells {
            let got = check_key(&raw);
            match set["outcome"].as_str() {
                Some("ok") => assert_eq!(got, Ok(()), "{raw:?}"),
                Some("error") => {
                    let err = got.expect_err(&raw);
                    if let Some(m) = set["message"].as_str() {
                        assert_eq!(err, m, "{raw:?}");
                    }
                    for frag in set["message_contains"].as_array().into_iter().flatten() {
                        assert!(err.contains(frag.as_str().unwrap()), "{raw:?}: {err}");
                    }
                }
                other => panic!("{raw:?}: outcome {other:?}"),
            }
            // KvKey::parse is check_key, and keeps the key as given.
            match KvKey::parse(&raw) {
                Ok(k) => assert_eq!(k.as_str(), raw),
                Err(e) => assert_eq!(Err(e), check_key(&raw)),
            }
        }
    }

    #[test]
    fn a_key_of_350_bytes_is_accepted_and_351_is_refused_naming_the_bound() {
        assert!(check_key(&"k".repeat(350)).is_ok());
        assert!(check_key(&format!("A{}", "b.-_9".repeat(69) + "z")).is_ok());
        let err = check_key(&"k".repeat(351)).unwrap_err();
        assert_eq!(
            err,
            format!(
                "key '{}…' is 351 bytes long; a KV key is at most 350 bytes (Kestra's own \
                 column width)",
                "k".repeat(40)
            )
        );
    }

    #[test]
    fn an_empty_key_is_refused_with_kestras_message() {
        assert_eq!(
            check_key(""),
            Err("Key cannot be null or empty".to_string())
        );
        // Never trimmed: a space is a character the regex refuses.
        assert_eq!(check_key(" "), Err(BAD_KEY.to_string()));
        assert_eq!(check_key(" a"), Err(BAD_KEY.to_string()));
        assert_eq!(check_key("a "), Err(BAD_KEY.to_string()));
    }

    #[test]
    fn a_leading_dot_dash_or_underscore_is_refused() {
        for bad in ["_a", ".a", "-a", "_", ".", "-", "_kv", "_files"] {
            assert_eq!(check_key(bad), Err(BAD_KEY.to_string()), "{bad}");
        }
        // The controls: the same characters after the first are legal.
        for ok in ["a_", "a.", "a-", "A-1._x", "0", "Z9"] {
            assert!(check_key(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn a_slash_space_or_non_ascii_letter_is_refused() {
        for bad in [
            "a/b", "a b", "é", "aé", "a\tb", "a\nb", "a:b", "a*b", "a\0b", "ａ",
        ] {
            assert_eq!(check_key(bad), Err(BAD_KEY.to_string()), "{bad:?}");
        }
        // A non-ASCII key longer than 350 bytes is refused by the regex, not
        // the bound (the slice at 40 would not be a char boundary).
        assert_eq!(check_key(&"é".repeat(200)), Err(BAD_KEY.to_string()));
    }
}
