//! THE path rule of a namespace file (W9b spec §3): one function for every
//! entry point -- the expression functions, the tasks, the API and the plugin
//! bridge. A path that breaks a rule is the author's error, naming the path
//! and the rule.

/// A namespace file or directory path in its stored form: a leading `/`, a
/// directory ending with `/` (migration 0174's `path`). Only
/// [`NsPath::parse_file`] and [`NsPath::parse_dir`] build one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NsPath(String);

/// The longest one segment may be, in characters (Kestra's
/// "A file or folder name exceeds the maximum length of 255 characters.").
const MAX_SEGMENT_CHARS: usize = 255;

/// The longest a path may be, in UTF-8 bytes of its STORED form (the leading
/// '/' and, for a directory, the trailing '/' included): the user's ruling,
/// 2026-10-05 (W9b Task 2 fix round 1, I2). It keeps the object key
/// `{prefix}{ws}/{ns-as-path}/_files{path}.v{N}` under the 1024-byte S3 key
/// limit, and the index row under Postgres's btree limit, so a long path is
/// the author's error naming it -- never an infrastructure error at write.
pub const MAX_PATH_BYTES: usize = 512;

/// The folder Kestra keeps its flow sources in; no namespace file lives there.
const FLOWS_FOLDER: &str = "_flows";

impl NsPath {
    /// A file: leading '/' added when missing; refuses an empty, '.' or '..'
    /// segment, a segment over 255 chars, `/_flows` or under it, '\\' '?' '#'
    /// or a control char, a last segment ending in `.v{digits}`, a trailing
    /// '/'. The error names the path and the rule.
    pub fn parse_file(raw: &str) -> Result<Self, String> {
        let full = with_leading_slash(raw);
        let (body, trailing) = match full[1..].strip_suffix('/') {
            Some(b) => (b, true),
            None => (&full[1..], false),
        };
        check_segments(raw, body)?;
        check_length(raw, &full)?;
        let last = body.rsplit('/').next().unwrap_or(body);
        if ends_in_revision_suffix(last) {
            return Err(refusal(
                raw,
                "a file name may not end in '.v' and digits, which names a revision of \
                 another file",
            ));
        }
        if trailing {
            return Err(refusal(
                raw,
                "a file path may not end with '/', which names a directory",
            ));
        }
        Ok(Self(full))
    }

    /// A directory: the same rules, the stored form ends with '/'; `/` itself
    /// is refused (the root has no row). The `.v{digits}` rule is a FILE's
    /// (spec §3: it would read as a revision of another file; a directory has
    /// no URI and no revisions).
    pub fn parse_dir(raw: &str) -> Result<Self, String> {
        let full = with_leading_slash(raw);
        let body = full[1..].strip_suffix('/').unwrap_or(&full[1..]);
        if body.is_empty() {
            return Err(refusal(
                raw,
                "it is the namespace's root, which is not a directory one can name",
            ));
        }
        check_segments(raw, body)?;
        let stored = format!("/{body}/");
        check_length(raw, &stored)?;
        Ok(Self(stored))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_dir(&self) -> bool {
        self.0.ends_with('/')
    }

    /// The lock key's path part: the path without its trailing '/', so a file
    /// `/a` and a directory `/a/` share it.
    pub fn lock_path(&self) -> &str {
        self.0.strip_suffix('/').unwrap_or(&self.0)
    }

    /// Every parent directory, outermost first: `/a/b/c.txt` -> [`/a/`, `/a/b/`].
    /// The root is never one (it has no row).
    pub fn parents(&self) -> Vec<NsPath> {
        let own = self.lock_path();
        own.char_indices()
            .skip(1)
            .filter(|(_, c)| *c == '/')
            .map(|(i, _)| NsPath(own[..=i].to_string()))
            .collect()
    }
}

impl std::fmt::Display for NsPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn with_leading_slash(raw: &str) -> String {
    if raw.starts_with('/') {
        raw.to_string()
    } else {
        format!("/{raw}")
    }
}

fn refusal(raw: &str, rule: &str) -> String {
    format!("namespace file path {raw:?}: {rule}")
}

/// The rules every path shares, in the interface's order. `body` is the path
/// without its leading '/' (and, for a directory, its trailing one).
fn check_segments(raw: &str, body: &str) -> Result<(), String> {
    let segments: Vec<&str> = body.split('/').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(refusal(
            raw,
            "it has an empty segment ('//', or nothing at all)",
        ));
    }
    if segments.iter().any(|s| *s == "." || *s == "..") {
        return Err(refusal(
            raw,
            "it has a '.' or '..' segment; a namespace file is named by its full path",
        ));
    }
    if segments
        .iter()
        .any(|s| s.chars().count() > MAX_SEGMENT_CHARS)
    {
        return Err(refusal(
            raw,
            "a file or folder name is longer than 255 characters",
        ));
    }
    if segments.first() == Some(&FLOWS_FOLDER) {
        return Err(refusal(
            raw,
            "'/_flows' and everything under it is reserved for flow sources",
        ));
    }
    if let Some(c) = body
        .chars()
        .find(|c| matches!(c, '\\' | '?' | '#') || c.is_control())
    {
        return Err(refusal(
            raw,
            &format!("it holds the character {c:?}, which no namespace file path may"),
        ));
    }
    Ok(())
}

/// [`MAX_PATH_BYTES`], over the stored form, in bytes (not characters).
fn check_length(raw: &str, stored: &str) -> Result<(), String> {
    if stored.len() > MAX_PATH_BYTES {
        return Err(refusal(
            raw,
            &format!(
                "it is {} bytes long; a namespace file path may be at most {MAX_PATH_BYTES} \
                 bytes of UTF-8, its leading '/' (and a directory's trailing '/') included",
                stored.len()
            ),
        ));
    }
    Ok(())
}

/// `name` ends in `.v` followed by one or more ASCII digits.
pub(crate) fn ends_in_revision_suffix(name: &str) -> bool {
    match name.rfind(".v") {
        Some(i) => {
            let digits = &name[i + 2..];
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn file_err(raw: &str) -> String {
        NsPath::parse_file(raw).expect_err(raw)
    }

    /// `w9b-namespace-files.json` `egret_expected.path_rules`: egret's accepted
    /// set and refused set (with the rule); a `chosen` cell is egret's answer
    /// where Kestra's differs, and is asserted the same way (its top level).
    #[test]
    fn the_paths_match_the_artifact() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("oracle-exec/w9b-namespace-files.json");
        let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let rules = fixture["egret_expected"]["path_rules"].as_object().unwrap();
        let rule_words = |rule: &str| -> &'static str {
            match rule {
                "dot_or_dotdot_segment" => "'..' segment",
                "empty_segment" => "empty segment",
                "segment_over_255" => "longer than 255",
                "flows_folder" => "_flows",
                "revision_suffix" => "'.v' and digits",
                "refused_character" => "character",
                other => panic!("unknown rule {other}"),
            }
        };
        let (mut accepted, mut refused, mut chosen) = (0, 0, 0);
        // `raw_path` is a display form (`control_0x01`'s is the six characters
        // `a\u0001b`); `query_sent`, percent-decoded, is the path Kestra was sent.
        let decode = |s: &str| -> String {
            let (b, mut out, mut i) = (s.as_bytes(), Vec::new(), 0);
            while i < b.len() {
                if b[i] == b'%' && i + 2 < b.len() {
                    let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
                    out.push(u8::from_str_radix(hex, 16).unwrap());
                    i += 3;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).unwrap()
        };
        for (key, row) in rules {
            let Some(sent) = row["query_sent"].as_str() else {
                continue; // `_note`
            };
            let raw = &*decode(sent);
            if row["basis"] == "chosen" {
                chosen += 1;
            }
            if row["accepted"].as_bool().unwrap() {
                accepted += 1;
                let got = NsPath::parse_file(raw).unwrap_or_else(|e| panic!("{key}: {e}"));
                assert_eq!(got.as_str(), row["normalised"].as_str().unwrap(), "{key}");
            } else {
                refused += 1;
                let err = file_err(raw);
                let words = rule_words(row["rule"].as_str().unwrap());
                assert!(err.contains(words), "{key}: {err:?} names {words:?}");
                assert!(
                    err.contains(&format!("{raw:?}")),
                    "{key}: {err} names the path"
                );
            }
        }
        // A zero beside a number that can disagree: every row was compared.
        assert_eq!((accepted, refused, chosen), (7, 11, 9));
    }

    #[test]
    fn dotdot_and_dot_and_empty_segments_are_refused() {
        for raw in ["a/../b", "..", "/a/..", "./a", "a/./b", "/."] {
            assert!(file_err(raw).contains("'.' or '..' segment"), "{raw}");
        }
        for raw in ["a//b", "", "/", "//a"] {
            assert!(file_err(raw).contains("empty segment"), "{raw}");
        }
        assert!(NsPath::parse_dir("a//b/")
            .unwrap_err()
            .contains("empty segment"));
        assert!(NsPath::parse_dir("a/../").unwrap_err().contains("'..'"));
        // The controls: a name that merely holds dots.
        assert!(NsPath::parse_file("a/..b/.c").is_ok());
        assert!(NsPath::parse_dir("a/...").is_ok());
    }

    #[test]
    fn a_segment_of_256_characters_is_refused_and_255_is_not() {
        let (ok, long) = ("é".repeat(255), "é".repeat(256));
        // 255 two-byte characters (510 bytes) fit only at the top level: under
        // `/d/` the same name is 513 bytes and the 512-byte rule (I2) refuses it.
        assert!(NsPath::parse_file(&format!("/{ok}")).is_ok());
        assert!(file_err(&format!("/d/{ok}")).contains("513 bytes"));
        assert!(NsPath::parse_file(&format!("/d/{}", "a".repeat(255))).is_ok());
        // The segment rule is checked first, so a 256-character name names it.
        assert!(file_err(&format!("/d/{long}")).contains("longer than 255"));
        assert!(NsPath::parse_dir(&format!("/{long}/"))
            .unwrap_err()
            .contains("longer than 255"));
        assert!(NsPath::parse_dir(&format!("/{ok}/")).is_ok());
    }

    /// I2: 512 bytes of the stored form, counted in bytes -- `é` is 2 bytes,
    /// so each path below is about half as many characters, and a rule that
    /// counted characters would accept the 513-byte ones.
    #[test]
    fn a_path_of_512_bytes_is_accepted_and_513_is_refused() {
        let e200 = "é".repeat(200); // 400 bytes, one segment under 255 chars
                                    // A file: '/' + 400 + '/' + 110 = 512 bytes, 257 characters.
        let file512 = format!("/{e200}/{}", "é".repeat(55));
        assert_eq!((file512.len(), file512.chars().count()), (512, 257));
        assert_eq!(NsPath::parse_file(&file512).unwrap().as_str(), file512);
        // Without its leading '/', the stored form is still 512.
        assert!(NsPath::parse_file(&file512[1..]).is_ok());
        let file513 = format!("{file512}a");
        let err = file_err(&file513);
        assert!(
            err.contains("513 bytes") && err.contains("at most 512"),
            "{err}"
        );
        assert!(
            err.contains(&format!("{file513:?}")),
            "names the path: {err}"
        );
        // A directory: its stored form's trailing '/' counts.
        let body511 = format!("{e200}/{}a", "é".repeat(54)); // 400+1+108+1 = 510
        let dir_raw = format!("/{body511}"); // 511 bytes; stored "/…/" is 512
        assert_eq!(dir_raw.len(), 511);
        assert_eq!(NsPath::parse_dir(&dir_raw).unwrap().as_str().len(), 512);
        let err = NsPath::parse_dir(&format!("{dir_raw}b")).unwrap_err();
        assert!(err.contains("513 bytes"), "{err}");
        // Typed with its trailing '/', the same 512 bytes pass.
        assert!(NsPath::parse_dir(&format!("{dir_raw}/")).is_ok());
    }

    #[test]
    fn flows_and_under_it_are_refused_and_flowsx_is_not() {
        for raw in ["/_flows", "_flows/x", "/_flows/a/b.yml"] {
            assert!(file_err(raw).contains("_flows"), "{raw}");
        }
        assert!(NsPath::parse_dir("/_flows/")
            .unwrap_err()
            .contains("_flows"));
        assert!(NsPath::parse_dir("/_flows/sub")
            .unwrap_err()
            .contains("_flows"));
        for ok in ["/_flowsx", "/a/_flows", "/_flows.txt", "/x_flows/y"] {
            assert!(NsPath::parse_file(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn a_file_ending_in_v_digits_is_refused() {
        for raw in ["x.v2", "/a/x.v1", "x.v0", "x.v02", "/x.txt.v12345678901234"] {
            assert!(file_err(raw).contains("'.v' and digits"), "{raw}");
        }
        for ok in ["x.v", "x.vx", "x.v2x", "x.v2/y", "x.V2", "xv2", "x.v 2"] {
            assert!(NsPath::parse_file(ok).is_ok(), "{ok}");
        }
        // A directory has no revisions: the rule is a file's.
        assert_eq!(NsPath::parse_dir("x.v2").unwrap().as_str(), "/x.v2/");
    }

    #[test]
    fn backslash_query_fragment_and_control_characters_are_refused() {
        for raw in [
            "a\\b", "a?b", "a#b", "a\nb", "a\u{1}b", "a\u{7f}b", "a\tb", "d\0/x",
        ] {
            assert!(file_err(raw).contains("character"), "{raw:?}");
            assert!(
                NsPath::parse_dir(raw).unwrap_err().contains("character"),
                "{raw:?}"
            );
        }
        for ok in ["a b.txt", "é.txt", "a%20b", "a+b", "a_b-c.d"] {
            assert!(NsPath::parse_file(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn a_trailing_slash_names_a_directory_not_a_file() {
        let err = file_err("a/b/");
        assert!(err.contains("may not end with '/'"), "{err}");
        assert_eq!(NsPath::parse_dir("a/b/").unwrap().as_str(), "/a/b/");
        assert_eq!(NsPath::parse_dir("a/b").unwrap().as_str(), "/a/b/");
        assert!(NsPath::parse_dir("/").unwrap_err().contains("root"));
        assert!(NsPath::parse_dir("").unwrap_err().contains("root"));
    }

    #[test]
    fn a_missing_leading_slash_is_added() {
        assert_eq!(NsPath::parse_file("a/b.txt").unwrap().as_str(), "/a/b.txt");
        assert_eq!(NsPath::parse_file("/a/b.txt").unwrap().as_str(), "/a/b.txt");
        let d = NsPath::parse_dir("a").unwrap();
        assert_eq!((d.as_str(), d.is_dir()), ("/a/", true));
        let f = NsPath::parse_file("a").unwrap();
        assert_eq!((f.as_str(), f.is_dir()), ("/a", false));
        assert_eq!(f.lock_path(), d.lock_path());
    }

    #[test]
    fn parents_are_outermost_first_and_exclude_the_root() {
        let p = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
        let names = |x: NsPath| -> Vec<String> {
            x.parents().iter().map(|n| n.as_str().to_string()).collect()
        };
        assert_eq!(
            names(NsPath::parse_file("/a/b/c.txt").unwrap()),
            p(&["/a/", "/a/b/"])
        );
        assert_eq!(names(NsPath::parse_dir("/a/b/").unwrap()), p(&["/a/"]));
        assert!(NsPath::parse_file("/top.txt").unwrap().parents().is_empty());
        assert!(NsPath::parse_dir("/a/").unwrap().parents().is_empty());
        for parent in NsPath::parse_file("/é/ü/x").unwrap().parents() {
            assert!(parent.is_dir());
        }
    }
}
