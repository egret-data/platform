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
//! author error naming it.
//!
//! *(W9b Task 5 fix round 1, M3, 2026-10-06.)* Java's Perl classes are ASCII
//! (`\w` is `[a-zA-Z_0-9]`, `\d` `[0-9]`, `\s` `[ \t\n\x0B\f\r]`, and `\b`
//! the boundary of that `\w`, without `UNICODE_CHARACTER_CLASS`); the `regex`
//! crate's are Unicode. So a `regex:` body has each of `\w \W \d \D \s \S
//! \b \B` spelled as the ASCII class Java means ([`java_ascii_classes`]):
//! `regex:/\w` does not match `/é`, as in Kestra. `unicode(false)` cannot do
//! it, because it also makes `.` and a negated class match single bytes, which
//! a `&str` regex refuses and which would match half of `é`. Still Unicode, as
//! the record: `(?i)` folds case beyond ASCII and `\p{Alpha}` is Unicode's
//! `Alphabetic`, where Java's are ASCII.
//! Java's `\Q…\E` quoting is not in the crate's grammar: refused, naming the
//! pattern (fix round 2, M-R2).
//!
//! # Bounds (W9b Task 5 fix round 1, I1, 2026-10-06)
//!
//! One list (the includes, or the excludes) holds at most [`MAX_PATTERNS`]
//! patterns, and the whole list is compiled as ONE [`RegexSet`] under ONE
//! [`REGEX_SIZE_LIMIT`] (`size_limit` and `dfa_size_limit`): the budget is
//! what the compiler actually builds for every pattern together, so a
//! thousand patterns each under the limit cannot add up to gigabytes. Nothing
//! here inspects a pattern's grammar to bound it. [`PathMatcher::new`] is CPU
//! work; `stage` runs it off the async runtime.

use regex::{RegexSet, RegexSetBuilder};

use super::path::NsPath;

/// The most memory ONE LIST of patterns may take once compiled
/// (`RegexSetBuilder::size_limit` and `dfa_size_limit`), shared by every
/// pattern of the list. Real globs compile to 1-10 KiB each; 50 realistic
/// ones to about 221 KiB. *(The user's ruling, 2026-10-06, Task 5 fix round 2
/// addendum: 256 KiB, down from 1 MiB.)* Matching costs about linear in this
/// size × path bytes × paths: the measured worst case at 256 KiB is about
/// 70-77 s of CPU per list per 10 000 paths of 512 bytes (spec §15), on the
/// blocking pool.
pub const REGEX_SIZE_LIMIT: usize = 256 << 10;

/// The most patterns one list (`include`, or `exclude`) may hold. egret's
/// bound (Kestra has none); a refusal names it.
pub const MAX_PATTERNS: usize = 256;

const SYNTAX_GLOB: &str = "glob:";
const SYNTAX_REGEX: &str = "regex:";

/// Java's `.` without `DOTALL`: anything but a line terminator.
const JAVA_DOT: &str = r"[^\n\r\x{85}\x{2028}\x{2029}]";

/// `includes AND NOT excludes`, each a list of glob or regex patterns.
#[derive(Clone, Debug)]
pub struct PathMatcher {
    /// `None`: no includes, so everything is included.
    includes: Option<RegexSet>,
    excludes: RegexSet,
}

impl PathMatcher {
    /// `glob:` is the default syntax, `regex:` is supported, `**/` is
    /// prepended to a pattern starting with a word character
    /// (`PathMatcherPredicate.java:112-114`). Each list holds at most
    /// [`MAX_PATTERNS`] and is compiled as one set under [`REGEX_SIZE_LIMIT`].
    /// A refusal names the pattern and why, or the list and the bound it
    /// crossed.
    pub fn new(includes: &[String], excludes: &[String]) -> Result<Self, String> {
        Ok(Self {
            includes: if includes.is_empty() {
                None
            } else {
                Some(compile_list(includes, "include")?)
            },
            excludes: compile_list(excludes, "exclude")?,
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
        let included = self.includes.as_ref().is_none_or(|set| set.is_match(path));
        included && !self.excludes.is_match(path)
    }
}

/// One list, to one set: the count first, then each pattern checked alone
/// (so a refusal names it), then the whole list compiled under ONE budget.
fn compile_list(list: &[String], which: &str) -> Result<RegexSet, String> {
    if list.len() > MAX_PATTERNS {
        return Err(format!(
            "{} {which} patterns, more than the {MAX_PATTERNS} one list may hold",
            list.len()
        ));
    }
    let sources = list
        .iter()
        .map(|p| source(p).map_err(|why| format!("{which} pattern {p:?}: {why}")))
        .collect::<Result<Vec<_>, _>>()?;
    RegexSetBuilder::new(&sources)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|e| match (e, list) {
            (regex::Error::CompiledTooBig(limit), [one]) => format!(
                "{which} pattern {one:?}: it compiles to more than {limit} bytes, the most \
                 one list of patterns may take"
            ),
            (regex::Error::CompiledTooBig(limit), _) => format!(
                "the {} {which} patterns together compile to more than {limit} bytes, the \
                 most one list of patterns may take",
                list.len()
            ),
            (other, _) => format!("the {which} patterns are not valid: {other}"),
        })
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

/// One pattern, through [`effective`], to the anchored regex source the set
/// compiles; checked by the `regex` crate's own parser (no compilation, so
/// no cost the author chose), so a refusal names this pattern.
fn source(pattern: &str) -> Result<String, String> {
    let eff = effective(pattern);
    let source = if let Some(body) = eff.strip_prefix(SYNTAX_REGEX) {
        let body = java_ascii_classes(body);
        // Checked alone first, so a body with an unbalanced ')' cannot close
        // the group it is wrapped in and change what it means.
        parse(&body)?;
        format!("^(?:{body})$")
    } else {
        let glob = eff.strip_prefix(SYNTAX_GLOB).unwrap_or(&eff);
        glob_to_regex(glob)?
    };
    parse(&source)?;
    Ok(source)
}

/// The parse `regex::RegexSetBuilder` runs, with its defaults (Unicode and
/// UTF-8 on, nest limit 250): a syntax error is the author's, by name.
fn parse(source: &str) -> Result<(), String> {
    regex_syntax::Parser::new()
        .parse(source)
        .map(drop)
        .map_err(|e| format!("it is not a valid pattern: {e}"))
}

/// A `regex:` body with Java's ASCII Perl classes spelled out (module doc,
/// M3). Only an escape's next character is looked at, so `\\w` stays a
/// backslash and a `w`; inside a class the replacement is a nested class,
/// which the `regex` crate accepts (`[\w-]` -> `[[0-9A-Za-z_]-]`).
fn java_ascii_classes(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('w') => out.push_str("[0-9A-Za-z_]"),
            Some('W') => out.push_str("[^0-9A-Za-z_]"),
            Some('d') => out.push_str("[0-9]"),
            Some('D') => out.push_str("[^0-9]"),
            Some('s') => out.push_str(r"[\t\n\x0B\x0C\r ]"),
            Some('S') => out.push_str(r"[^\t\n\x0B\x0C\r ]"),
            Some('b') => out.push_str(r"(?-u:\b)"),
            Some('B') => out.push_str(r"(?-u:\B)"),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
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

    /// Fix round 1, I1: the review's list -- 1,000 patterns `regex:\w{20}N`,
    /// each under the limit alone -- is refused at once by the count, naming
    /// it, before anything is compiled; a list AT the count compiles.
    #[test]
    fn a_thousand_patterns_are_refused_quickly_naming_the_cap() {
        let list: Vec<String> = (0..1000).map(|i| format!(r"regex:\w{{20}}{i}")).collect();
        let started = std::time::Instant::now();
        let err = PathMatcher::new(&list, &[]).unwrap_err();
        let took = started.elapsed();
        assert!(
            err.contains("1000 include patterns")
                && err.contains(&format!("the {MAX_PATTERNS} one list may hold")),
            "{err}"
        );
        assert!(took < std::time::Duration::from_secs(1), "took {took:?}");
        let err = PathMatcher::new(&[], &list).unwrap_err();
        assert!(err.contains("1000 exclude patterns"), "{err}");
        // The control: MAX_PATTERNS small patterns compile (about 94 KiB
        // together), and match.
        let at_cap: Vec<String> = (0..MAX_PATTERNS).map(|i| format!("/f{i}.txt")).collect();
        let m = PathMatcher::new(&at_cap, &[]).expect("a list at the cap");
        assert!(m.matches_str("/f255.txt") && !m.matches_str("/f256.txt"));
    }

    /// Fix round 1, I1: ONE budget per list, not one per pattern. Two
    /// patterns that each compile under [`REGEX_SIZE_LIMIT`] alone are
    /// refused together, naming the budget, quickly.
    #[test]
    fn patterns_each_under_the_limit_are_refused_together_by_one_budget() {
        let one = |s: &str| vec![s.to_string()];
        // About 168 KiB each alone, about 336 KiB together.
        let (a, b) = (r"regex:/\pL{4}a", r"regex:/\pL{4}b");
        PathMatcher::new(&one(a), &[]).expect("a alone is under the limit");
        PathMatcher::new(&one(b), &[]).expect("b alone is under the limit");
        let both = vec![a.to_string(), b.to_string()];
        let started = std::time::Instant::now();
        let err = PathMatcher::new(&both, &[]).unwrap_err();
        assert!(
            err.contains("the 2 include patterns together compile to more than")
                && err.contains(&REGEX_SIZE_LIMIT.to_string()),
            "{err}"
        );
        let err = PathMatcher::new(&[], &both).unwrap_err();
        assert!(err.contains("2 exclude patterns together"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        // An include list and an exclude list each have their own budget.
        PathMatcher::new(&one(a), &one(b)).expect("one per list");
    }

    /// Fix round 2 addendum (the user's ruling of 2026-10-06): the 256 KiB
    /// budget still takes 50 realistic globs (about 221 KiB together, measured)
    /// as one list, and every pattern of the fixture as one list.
    #[test]
    fn fifty_realistic_globs_compile_under_the_budget() {
        let mut list = Vec::new();
        for (i, e) in [
            "csv", "parquet", "json", "txt", "sql", "py", "yaml", "md", "xlsx", "log",
        ]
        .iter()
        .enumerate()
        {
            list.push(format!("**/*.{e}"));
            list.push(format!("/data/{e}/**"));
            list.push(format!("reports/2024-??/*.{e}"));
            list.push(format!("/sub{i}/**/x-?.{e}"));
            list.push(format!(r"regex:/data/[\w-]{{1,64}}\.{e}"));
        }
        assert_eq!(list.len(), 50);
        let m =
            PathMatcher::new(&list, &list).expect("50 realistic globs, as includes and excludes");
        assert!(m.includes.as_ref().unwrap().is_match("/a/b.parquet"));
        assert!(m
            .includes
            .as_ref()
            .unwrap()
            .is_match("/reports/2024-07/x.md"));
        // Every fixture pattern together, as one list.
        let doc = fixture();
        let globs = &doc["egret_expected"]["globs"];
        let mut all = std::collections::BTreeSet::new();
        for section in ["rows", "rows_without_leading_slash_for_contrast", "runs"] {
            for row in globs[section].as_array().unwrap() {
                all.extend(strings(&row["include"]));
                all.extend(strings(&row["exclude"]));
            }
        }
        let all: Vec<String> = all.into_iter().collect();
        assert!(all.len() >= 30, "{}", all.len());
        PathMatcher::new(&all, &[]).expect("every fixture pattern as one list");
    }

    /// Fix round 1, M3: `regex:` Perl classes are Java's, ASCII; `.` and
    /// literals still work on characters, so a non-ASCII path is matched
    /// per character, not per byte.
    #[test]
    fn regex_perl_classes_are_ascii_as_in_java() {
        let m = |p: &str, path: &str| matcher(&[p], &[]).matches(&file(path));
        assert!(!m(r"regex:/\w", "/é") && m(r"regex:/\w", "/a"));
        assert!(m(r"regex:/\W", "/é") && !m(r"regex:/\W", "/a"));
        assert!(!m(r"regex:/\d", "/\u{663}") && m(r"regex:/\d", "/3"));
        assert!(m(r"regex:/\D", "/\u{663}"));
        assert!(!m(r"regex:/a\sb", "/a\u{a0}b") && m(r"regex:/a\sb", "/a b"));
        assert!(m(r"regex:/a\Sb", "/a\u{a0}b"));
        // `\b` is the ASCII `\w`'s boundary: between `a` and `é` is one.
        assert!(m(r"regex:/a\b.*", "/aé") && !m(r"regex:/a\b.*", "/ab"));
        assert!(m(r"regex:/a\B.*", "/ab") && !m(r"regex:/a\B.*", "/aé"));
        // `.` is one character, `é` included; a literal non-ASCII matches.
        assert!(m(r"regex:/.\.txt", "/é.txt") && m("regex:/é+", "/éé"));
        assert!(m(r"regex:/[^a]x", "/éx"));
        // An escaped backslash stays one: `\\w` is a backslash, then `w`.
        assert!(!matcher(&[r"regex:/\\w"], &[]).matches_str("/a"));
        assert!(matcher(&[r"regex:/\\w"], &[]).matches_str(r"/\w"));
        // Inside a class too, and the review's pattern now compiles small.
        let parquet = r"regex:/data/[\w-]{1,64}\.parquet";
        assert!(m(parquet, "/data/a-b_1.parquet"));
        assert!(!m(parquet, "/data/é.parquet"));
        assert!(m(r"regex:/[^\W]", "/a") && !m(r"regex:/[^\W]", "/é"));
        // A glob's `[a-z]` and `?` were already the JDK's.
        assert!(!m("/[a-z]", "/é") && m("/?", "/é"));
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
