//! Namespace-file globs (W9b spec §7): Kestra's `PathMatcherPredicate`
//! (`core/…/utils/PathMatcherPredicate.java`) over the JDK's Unix glob
//! (`sun.nio.fs.Globs.toRegexPattern`), and Commons-IO's
//! `FilenameUtils.wildcardMatch` for `PurgeFiles` (R2, Task 7).
//!
//! Implemented against the w9b fixture's `globs` section, not against a
//! reading of the Java: every one of its rows (`rows` and
//! `rows_without_leading_slash_for_contrast`, each with the pattern as
//! `PathMatcherPredicate` compiles it) and every measured run is a test here.
//!
//! # What the predicate sees
//!
//! `NamespaceFile.filePath()`: the namespace-relative path WITH its leading
//! `/` (`/a/b.txt`) and never a `.vN` -- [`NsPath::as_str`] of a file.
//!
//! # The pattern, as `PathMatcherPredicate` compiles it
//!
//! * `regex:…` and `glob:…` are taken as written;
//! * any other pattern is a glob; one that starts with a word character
//!   (`[A-Za-z0-9_]`, Java's ASCII `\w`) gets `**/` prepended
//!   (`PathMatcherPredicate.java:112-114`), and every `\` becomes `/`. So
//!   `*.txt` keeps no prefix and matches NOTHING against `/…` paths, while
//!   `x` becomes `**/x` and matches `/x` and `/sub/x` (fixture runs g00, g06).
//!
//! Empty includes match everything; the predicate is includes AND NOT
//! excludes (`Builder.build`).
//!
//! # Where the regex differs from Java's, on the record
//!
//! A glob is translated here, to the regex the JDK would build, with Java's
//! `.` (no line terminator) spelled out. A `regex:` pattern is compiled by the
//! `regex` crate, whose grammar is not `java.util.regex`'s (no look-around,
//! no back-references; `.` excludes only `\n`): a pattern it refuses is an
//! author error naming it. Every pattern is compiled under
//! [`REGEX_SIZE_LIMIT`], so a pattern that would blow up is refused by name,
//! never a panic or an allocation the author chose.

use regex::{Regex, RegexBuilder};

use super::path::NsPath;

/// The most memory one compiled pattern may take (`RegexBuilder::size_limit`
/// and `dfa_size_limit`). Real globs compile to a few KiB.
pub const REGEX_SIZE_LIMIT: usize = 1 << 20;

const SYNTAX_GLOB: &str = "glob:";
const SYNTAX_REGEX: &str = "regex:";

/// Java's `.` without `DOTALL`: anything but a line terminator.
const JAVA_DOT: &str = r"[^\n\r\x{85}\x{2028}\x{2029}]";

/// `includes AND NOT excludes`, each a list of glob or regex patterns.
#[derive(Clone, Debug)]
pub struct PathMatcher {
    includes: Vec<Regex>,
    excludes: Vec<Regex>,
}

impl PathMatcher {
    /// `glob:` is the default syntax, `regex:` is supported, `**/` is
    /// prepended to a pattern starting with a word character
    /// (`PathMatcherPredicate.java:112-114`). Each pattern is compiled with
    /// [`REGEX_SIZE_LIMIT`]. A refusal names the pattern and why.
    pub fn new(includes: &[String], excludes: &[String]) -> Result<Self, String> {
        let compile = |list: &[String], which: &str| -> Result<Vec<Regex>, String> {
            list.iter()
                .map(|p| compile(p).map_err(|why| format!("{which} pattern {p:?}: {why}")))
                .collect()
        };
        Ok(Self {
            includes: compile(includes, "include")?,
            excludes: compile(excludes, "exclude")?,
        })
    }

    /// Whether `path` (a file's [`NsPath`], leading `/` included) is
    /// included and not excluded.
    pub fn matches(&self, path: &NsPath) -> bool {
        self.matches_str(path.as_str())
    }

    /// The predicate on any string -- the fixture's contrast rows test paths
    /// with no leading `/`, which no [`NsPath`] is.
    pub(crate) fn matches_str(&self, path: &str) -> bool {
        let included = self.includes.is_empty() || self.includes.iter().any(|r| r.is_match(path));
        included && !self.excludes.iter().any(|r| r.is_match(path))
    }
}

/// The pattern as `PathMatcherPredicate` hands it to `getPathMatcher`
/// (its `syntaxAndPatterns()`, the fixture's `effective_*`).
pub fn effective(pattern: &str) -> String {
    if pattern.starts_with(SYNTAX_REGEX) || pattern.starts_with(SYNTAX_GLOB) {
        return pattern.to_string();
    }
    let starts_with_word = pattern
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let prefixed = if starts_with_word {
        format!("**/{pattern}")
    } else {
        pattern.to_string()
    };
    format!("{SYNTAX_GLOB}{}", prefixed.replace('\\', "/"))
}

/// One pattern, through [`effective`], to an anchored regex.
fn compile(pattern: &str) -> Result<Regex, String> {
    let eff = effective(pattern);
    let source = if let Some(body) = eff.strip_prefix(SYNTAX_REGEX) {
        // Checked alone first, so a body with an unbalanced ')' cannot close
        // the group it is wrapped in and change what it means.
        build(body)?;
        format!("^(?:{body})$")
    } else {
        let glob = eff.strip_prefix(SYNTAX_GLOB).unwrap_or(&eff);
        glob_to_regex(glob)?
    };
    build(&source)
}

fn build(source: &str) -> Result<Regex, String> {
    RegexBuilder::new(source)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|e| match e {
            regex::Error::CompiledTooBig(limit) => {
                format!("it compiles to more than {limit} bytes, the most one pattern may take")
            }
            other => format!("it is not a valid pattern: {other}"),
        })
}

/// One character, literal in a regex (inside a class too).
fn literal(out: &mut String, c: char) {
    let mut buf = [0u8; 4];
    out.push_str(&regex::escape(c.encode_utf8(&mut buf)));
}

/// `sun.nio.fs.Globs.toRegexPattern(glob, isDos = false)`, emitting the
/// `regex` crate's syntax; each refusal is the JDK's.
fn glob_to_regex(glob: &str) -> Result<String, String> {
    let chars: Vec<char> = glob.chars().collect();
    let next = |i: usize| chars.get(i).copied();
    let mut out = String::from("^");
    let mut in_group = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '\\' => {
                let Some(escaped) = next(i) else {
                    return Err("no character to escape after its final '\\'".to_string());
                };
                literal(&mut out, escaped);
                i += 1;
            }
            '/' => out.push('/'),
            '[' => {
                // A class never matches the name separator.
                out.push_str("[[^/]&&[");
                // Whether the class holds any member: `[]` and `[!]` would
                // be an empty class, which the JDK refuses too.
                let mut has_member = false;
                if next(i) == Some('^') {
                    // A literal '^': the glob's negation is '!'.
                    out.push_str(r"\^");
                    has_member = true;
                    i += 1;
                } else {
                    if next(i) == Some('!') {
                        out.push('^');
                        i += 1;
                    }
                    if next(i) == Some('-') {
                        out.push_str(r"\-");
                        has_member = true;
                        i += 1;
                    }
                }
                let mut has_range_start = false;
                let mut last = '\0';
                let mut closed = false;
                while i < chars.len() {
                    let c = chars[i];
                    i += 1;
                    if c == ']' {
                        closed = true;
                        break;
                    }
                    if c == '/' {
                        return Err("a class [..] may not hold the name separator '/'".into());
                    }
                    has_member = true;
                    if c == '-' {
                        if !has_range_start {
                            return Err("its class [..] has an invalid range".to_string());
                        }
                        match next(i) {
                            None => {
                                i += 1;
                                break;
                            }
                            Some(']') => {
                                // `[a-]`: the '-' is literal, and the class ends.
                                out.push_str(r"\-");
                                i += 1;
                                closed = true;
                                break;
                            }
                            Some(hi) => {
                                i += 1;
                                if hi < last {
                                    return Err("its class [..] has an invalid range".to_string());
                                }
                                out.push('-');
                                literal(&mut out, hi);
                                has_range_start = false;
                            }
                        }
                    } else {
                        literal(&mut out, c);
                        has_range_start = true;
                        last = c;
                    }
                }
                if !closed {
                    return Err("its class [..] is missing its ']'".to_string());
                }
                if !has_member {
                    return Err("its class [] is empty".to_string());
                }
                out.push_str("]]");
            }
            '{' => {
                if in_group {
                    return Err("groups {..} cannot nest".to_string());
                }
                out.push_str("(?:(?:");
                in_group = true;
            }
            '}' => {
                if in_group {
                    out.push_str("))");
                    in_group = false;
                } else {
                    literal(&mut out, '}');
                }
            }
            ',' => {
                if in_group {
                    out.push_str(")|(?:");
                } else {
                    out.push(',');
                }
            }
            '*' => {
                if next(i) == Some('*') {
                    // Crosses directory boundaries.
                    out.push_str(JAVA_DOT);
                    out.push('*');
                    i += 1;
                } else {
                    out.push_str("[^/]*");
                }
            }
            '?' => out.push_str("[^/]"),
            other => literal(&mut out, other),
        }
    }
    if in_group {
        return Err("a group {..} is missing its '}'".to_string());
    }
    out.push('$');
    Ok(out)
}

/// Commons-IO `FilenameUtils.wildcardMatch(subject, pattern, IOCase.SENSITIVE)`:
/// `*` is any run of characters (`/` and `.` included), `?` exactly one, and
/// everything else is itself, case-sensitively. `PurgeFiles.namespacePattern`
/// matches namespaces with it (R2, Task 7): `w9b.pp.*` does not match
/// `w9b.ppx.c`.
pub fn wildcard_match(pattern: &str, subject: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = subject.chars().collect();
    let (mut pi, mut si) = (0usize, 0usize);
    // The last '*' seen, and where in `s` its current extent ends.
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || (p[pi] != '*' && p[pi] == s[si])) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            // Let the last '*' take one more character and retry.
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// `PurgeFiles.filePattern` (Task 7): [`wildcard_match`] on the path WITHOUT
/// its leading `/` (spec §8.4, measured).
pub fn wildcard_matches(pattern: &str, path: &NsPath) -> bool {
    let p = path.as_str();
    wildcard_match(pattern, p.strip_prefix('/').unwrap_or(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("oracle-exec/w9b-namespace-files.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("read the w9b fixture"))
            .expect("parse the w9b fixture")
    }

    fn strings(v: &serde_json::Value) -> Vec<String> {
        v.as_array()
            .expect("an array")
            .iter()
            .map(|s| s.as_str().expect("a string").to_string())
            .collect()
    }

    fn matcher(include: &[&str], exclude: &[&str]) -> PathMatcher {
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        PathMatcher::new(&own(include), &own(exclude)).expect("valid patterns")
    }

    fn file(p: &str) -> NsPath {
        NsPath::parse_file(p).expect("a file path")
    }

    /// Every row of `globs.rows` and `globs.rows_without_leading_slash_for_contrast`:
    /// the compiled form is `effective_*`, and the answer is `matches`; and
    /// every measured run stages exactly its files from the fixture's tree.
    #[test]
    fn every_glob_row_of_the_artifact_matches_as_kestra_does() {
        let doc = fixture();
        let globs = &doc["egret_expected"]["globs"];
        let mut checked = 0;
        for section in ["rows", "rows_without_leading_slash_for_contrast"] {
            let rows = globs[section].as_array().expect("rows");
            for row in rows {
                let (include, exclude) = (strings(&row["include"]), strings(&row["exclude"]));
                let effective_of =
                    |v: &[String]| v.iter().map(|p| effective(p)).collect::<Vec<_>>();
                assert_eq!(
                    effective_of(&include),
                    strings(&row["effective_include"]),
                    "{row}"
                );
                assert_eq!(
                    effective_of(&exclude),
                    strings(&row["effective_exclude"]),
                    "{row}"
                );
                let m = PathMatcher::new(&include, &exclude).expect("the fixture's patterns");
                let path = row["path"].as_str().expect("a path");
                assert_eq!(
                    m.matches_str(path),
                    row["matches"].as_bool().expect("a bool"),
                    "{section}: {row}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 315 + 70, "every row");

        // The measured runs over the fixture's tree (directories are never
        // offered to the predicate: `globs.directories`).
        let tree: Vec<NsPath> = strings(&globs["tree"])
            .iter()
            .filter(|p| !p.contains("(directory"))
            .map(|p| file(p))
            .collect();
        assert_eq!(tree.len(), 8);
        let runs = globs["runs"].as_array().expect("runs");
        assert_eq!(runs.len(), 31);
        for run in runs {
            let m = PathMatcher::new(&strings(&run["include"]), &strings(&run["exclude"]))
                .expect("the run's patterns");
            let mut staged: Vec<&str> = tree
                .iter()
                .filter(|p| m.matches(p))
                .map(NsPath::as_str)
                .collect();
            staged.sort_unstable();
            assert_eq!(staged, strings(&run["staged"]), "{run}");
        }
    }

    #[test]
    fn a_pattern_starting_with_a_word_character_gets_double_star_prefix() {
        assert_eq!(effective("x"), "glob:**/x");
        assert_eq!(effective("_x"), "glob:**/_x");
        assert_eq!(effective("1.txt"), "glob:**/1.txt");
        for unprefixed in ["*.txt", "?b.txt", "{a,b}/*.txt", "/a", "-x", ".x", ""] {
            assert_eq!(effective(unprefixed), format!("glob:{unprefixed}"));
        }
        // Java's `\w` is ASCII: a non-ASCII letter gets no prefix.
        assert_eq!(effective("é.txt"), "glob:é.txt");
        // A backslash becomes '/' in an unprefixed pattern only.
        assert_eq!(effective(r"a\b"), "glob:**/a/b");
        assert_eq!(effective(r"glob:a\*"), r"glob:a\*");
        let m = matcher(&["x"], &[]);
        assert!(m.matches(&file("/x")) && m.matches(&file("/sub/x")));
        assert!(!m.matches(&file("/xx")) && !m.matches(&file("/sub/ax")));
        let m = matcher(&["*.txt"], &[]);
        assert!(!m.matches(&file("/top.txt")), "no prefix: '*' stops at '/'");
        // `glob:a\*` escapes the star: the literal name `a*` only.
        let m = matcher(&[r"glob:/a\*"], &[]);
        assert!(m.matches(&file("/a*")) && !m.matches(&file("/ab")));
    }

    #[test]
    fn regex_patterns_are_supported_and_an_invalid_one_is_refused_naming_it() {
        let m = matcher(&["regex:^/a/.*$"], &[]);
        assert!(m.matches(&file("/a/b.txt")) && !m.matches(&file("/b/a.txt")));
        // Whole-string, as `Pattern.matches`: `a/.*` does not match `/a/b`.
        let m = matcher(&["regex:a/.*"], &[]);
        assert!(!m.matches(&file("/a/b")));
        // An alternation stays inside its anchors.
        let m = matcher(&["regex:/x|/y"], &[]);
        assert!(m.matches(&file("/x")) && !m.matches(&file("/x/z")));
        for bad in ["regex:(unclosed", "regex:a)|(b"] {
            let err = PathMatcher::new(&[bad.to_string()], &[]).unwrap_err();
            assert!(
                err.contains(&format!("{bad:?}")) && err.contains("include"),
                "{err}"
            );
        }
        let err = PathMatcher::new(&[], &["regex:[".to_string()]).unwrap_err();
        assert!(err.contains("exclude pattern \"regex:[\""), "{err}");
        // A malformed glob is refused naming it too.
        for bad in ["/[ab", "/{a,{b}}", "/{a", "/[a/b]", "/[z-a]", "glob:x\\"] {
            let err = PathMatcher::new(&[bad.to_string()], &[]).unwrap_err();
            assert!(err.contains(&format!("{bad:?}")), "{bad}: {err}");
        }
    }

    #[test]
    fn an_oversized_pattern_is_refused_naming_it_not_a_panic() {
        let huge = "regex:(?:a{1000}){1000}".to_string();
        let err = PathMatcher::new(std::slice::from_ref(&huge), &[]).unwrap_err();
        assert!(
            err.contains(&format!("{huge:?}")) && err.contains("more than"),
            "{err}"
        );
        // The brief's spelling: refused by name, whichever rule refuses it.
        let stacked = "regex:a{1000}{1000}".to_string();
        let err = PathMatcher::new(std::slice::from_ref(&stacked), &[]).unwrap_err();
        assert!(err.contains(&format!("{stacked:?}")), "{err}");
        // A glob that grows past the limit is refused the same way.
        let long_glob = format!("/{}", "{a,b}*?".repeat(20_000));
        let err = PathMatcher::new(&[], &[long_glob]).unwrap_err();
        assert!(
            err.contains("exclude pattern") && err.contains("more than"),
            "{err}"
        );
    }

    #[test]
    fn excludes_win_over_includes() {
        let m = matcher(&["**"], &["/a/**"]);
        assert!(m.matches(&file("/b/a.txt")));
        assert!(!m.matches(&file("/a/b.txt")));
        let m = matcher(&["/a/b.txt"], &["/a/b.txt"]);
        assert!(!m.matches(&file("/a/b.txt")));
        // Empty includes match everything; excludes alone still exclude.
        let m = matcher(&[], &["**/*.csv"]);
        assert!(m.matches(&file("/top.txt")) && !m.matches(&file("/a/c.csv")));
        assert!(matcher(&[], &[]).matches(&file("/anything")));
    }

    #[test]
    fn classes_ranges_and_groups_as_the_jdk() {
        let m = matcher(&["/[!a]*"], &[]);
        assert!(m.matches(&file("/b")) && !m.matches(&file("/abc")));
        let m = matcher(&["/[a-c]x"], &[]);
        assert!(m.matches(&file("/bx")) && !m.matches(&file("/dx")));
        let m = matcher(&["/[^]x"], &[]);
        assert!(m.matches(&file("/^x")) && !m.matches(&file("/ax")));
        let m = matcher(&["/[a-]x"], &[]);
        assert!(m.matches(&file("/-x")) && m.matches(&file("/ax")));
        let m = matcher(&["/[&~]x"], &[]);
        assert!(m.matches(&file("/&x")) && m.matches(&file("/~x")));
        let m = matcher(&["/{a,b*}.txt"], &[]);
        assert!(m.matches(&file("/a.txt")) && m.matches(&file("/bcd.txt")));
        assert!(!m.matches(&file("/c.txt")));
        // A '}' outside a group, '(' and '.' are literal.
        let m = matcher(&["/a}(.)"], &[]);
        assert!(m.matches(&file("/a}(.)")) && !m.matches(&file("/a}(x)")));
        // Case-sensitive, as the JDK's Unix matcher.
        assert!(!matcher(&["/A.txt"], &[]).matches(&file("/a.txt")));
    }

    #[test]
    fn wildcard_star_and_question_mark_only() {
        assert!(wildcard_match("w9b.pp.*", "w9b.pp.a"));
        assert!(wildcard_match("w9b.pp.*", "w9b.pp.a.b"));
        assert!(!wildcard_match("w9b.pp.*", "w9b.ppx.c"), "'.' is literal");
        assert!(!wildcard_match("w9b.pp.*", "w9b.pp"));
        assert!(wildcard_match("a?c", "abc") && !wildcard_match("a?c", "ac"));
        assert!(!wildcard_match("ABC", "abc"), "case-sensitive");
        assert!(wildcard_match("*", "") && wildcard_match("", "") && !wildcard_match("", "a"));
        assert!(wildcard_match("*b*b", "abab") && !wildcard_match("*b*b", "abba"));
        assert!(wildcard_match("a**?", "ab") && !wildcard_match("a**?", "a"));
        assert!(!wildcard_match("[ab]", "a"), "no classes: '[' is literal");
        assert!(wildcard_match("[ab]", "[ab]"));
        // filePattern sees the path without its leading '/'.
        assert!(wildcard_matches("sub/*.txt", &file("/sub/a.txt")));
        assert!(
            wildcard_matches("*.txt", &file("/sub/a.txt")),
            "'*' crosses '/'"
        );
        assert!(!wildcard_matches("/sub/*", &file("/sub/a.txt")));
    }
}
