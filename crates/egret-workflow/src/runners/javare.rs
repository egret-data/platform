// kestra: core/src/main/java/io/kestra/core/runners/pebble/filters/RegexMatchFilter.java
// kestra: core/src/main/java/io/kestra/core/runners/pebble/filters/RegexReplaceFilter.java
// kestra: core/src/main/java/io/kestra/core/runners/pebble/filters/RegexExtractFilter.java
// kestra: core/src/main/java/io/kestra/core/utils/RegexUtils.java
// jdk: java.util.regex.Pattern (Corretto 25)

//! `java.util.regex` translated onto the `regex` crate — the pattern language
//! the three `regex*` filters take, and the replacement language
//! `regexReplace` takes.
//!
//! # This module exists because the two engines quietly disagree
//!
//! A 265-probe sweep of Java's pattern constructs against the `regex` crate
//! (see `oracle/README.md`) found **101 divergences**, and the dangerous half
//! is not the half that errors:
//!
//! | pattern | haystack | Java | `regex` crate, untranslated |
//! |---|---|---|---|
//! | `\d` | `٣` | no match | **match** — `\d` is Unicode there |
//! | `\w` | `é` | no match | **match** |
//! | `\s` | `\u{a0}` | no match | **match** |
//! | `\v` | `\n` | match | **no match** — `\v` is a vertical TAB there |
//! | `a.b` | `a\rb` | no match | **match** — only `\n` is excluded there |
//! | `\bcafé\b` | `le café noir` | no match | **match** |
//! | `(?i)k` | `K` (Kelvin) | no match | **match** |
//! | `c$` | `abc\n` | match | **no match** |
//! | `a++a` | `aaa` | no match | **match** — `+` after `+` is parsed as a quantifier there |
//!
//! Not one of those raises anything on either side. A filter that handed the
//! author's pattern straight to `regex::Regex::new` would answer a different
//! question from Kestra's, silently, on eight of the commonest constructs in
//! the language — which is the exact failure class this package exists to
//! prevent. So every one of them is **translated**, and what cannot be
//! translated is **refused by name**.
//!
//! # What is translated, and what is refused
//!
//! Translated exactly: `\d \D \w \W \s \S \h \H \v \V \R`, `.` (with `s` and
//! `d`), `\b \B` as Java's ASCII boundaries, `^` and `$` (see below), `\A \z`,
//! `\Q…\E`, the numeric escapes `\0ooo \xhh \x{…} \uXXXX \cX \e \a`, Java's
//! POSIX `\p{Alpha}`-family names, Unicode categories and scripts, character
//! classes including `&&` intersection and nesting, and case-insensitivity —
//! which is expanded **here**, per literal, because Java's `(?i)` is ASCII-only
//! and the crate's is Unicode-aware.
//!
//! Refused, each naming itself: lookahead, lookbehind, atomic groups,
//! possessive quantifiers, backreferences, `\G`, `\b{g}`, Unicode *blocks*
//! (`\p{InGreek}`, `\p{block=…}`) and `\p{java…}` methods. The `regex` crate
//! is a linear-time automaton; the first five of those are backtracking
//! constructs it cannot have, and the last three are tables it does not carry.
//!
//! # `$` is the one that needs the haystack
//!
//! Java's `$` outside `MULTILINE` is `\Z`: end of input, **or just before a
//! final line terminator**. The crate's `$` is end of haystack, full stop. The
//! two agree on every haystack that does not end in a line terminator and
//! differ on every haystack that does.
//!
//! Translating `$` to `\z` is therefore exact except on those haystacks, and
//! [`JavaRegex::check_haystack`] refuses exactly those, naming the reason.
//! The alternative — consuming an optional trailing terminator — answers
//! `regexMatch` correctly and then silently deletes the newline in
//! `regexReplace` and lengthens group 0 in `regexExtract`. A refusal on the
//! divergent input beats a wrong answer on it.
//!
//! # `RegexUtils`' 10-second timeout is not ported, and that is deliberate
//!
//! All three filters run their matcher through `RegexUtils.matcher`, which
//! wraps the haystack in a `TimeoutCharSequence` whose `charAt` throws
//! `RegexTimeoutException` after `Duration.ofSeconds(10)`
//! (`javap -p -c RegexUtils.class`). That bound exists because
//! `java.util.regex` backtracks and a pattern like `(a+)+b` is exponential on
//! a non-matching input.
//!
//! The `regex` crate cannot backtrack: its search is linear in the haystack
//! and the compiled program, so the vector the timeout guards does not exist
//! here. The crate's own compile-time size limit refuses a pattern whose
//! program would be too large. egret therefore answers, quickly, where Kestra
//! would spend ten seconds and throw — a divergence recorded rather than
//! reproduced, per the "a resource-exhaustion vector is not a behaviour to
//! port" rule, and pinned by
//! [`tests::a_catastrophic_java_pattern_is_answered_instead_of_timed_out`].

use std::fmt;

use regex::{Regex, RegexBuilder};

/// Java's line terminators, outside `UNIX_LINES`: `\n`, `\r`, `\u0085`,
/// `\u2028`, `\u2029` — plus the two-character `\r\n`, which only `\R` and
/// the `$` check treat as one unit.
///
/// `Pattern`'s javadoc calls these "line terminators" and they are what `.`
/// excludes, what `$` may sit in front of, and what `\R` matches.
/// The one refusal `(?U)` earns, in one place because it is raised from
/// three.
///
/// `(?U)` (UNICODE_CHARACTER_CLASS) redefines `\d`, `\w`, `\s` **and `\b`**.
/// An earlier cut guarded the first three and not `\b` — while the message
/// itself named `\b` — so `(?U)\bcafé\b` silently answered `false` where
/// Java answers `true`. The message and the guard now come from the same
/// constant.
const UNICODE_CLASSES_REFUSAL: &str = "'(?U)' (UNICODE_CHARACTER_CLASS), which redefines \\d, \\w, \\s, \\b \
     AND the whole POSIX family (\\p{Alpha}, \\p{Alnum}, \\p{Graph}, \\p{Print}, \\p{Upper}, \\p{Lower}, \\p{Punct}, \\p{Digit}, \\p{Space}, \\p{Blank}, \\p{XDigit})";

/// A subexpression that can never match: a word boundary and a non-boundary
/// at the same position. Used where Java compiles something that cannot match
/// anything — a lone surrogate.
const NEVER_MATCHES: &str = "(?:(?-u:\\b)(?-u:\\B))";

const LINE_TERMINATORS: &[char] = ['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}'].as_slice();

/// A compiled Java pattern, plus what the translation had to remember about
/// it.
#[derive(Debug)]
pub(crate) struct JavaRegex {
    regex: Regex,
    /// The pattern used `$` outside `MULTILINE`, or `\Z`. See the module doc.
    end_of_input_anchor: bool,
    /// The pattern used `^`/`$` under `MULTILINE`. The crate splits lines on
    /// `\n` and (with `crlf`) `\r\n`/`\r`; Java also splits on `\u0085`,
    /// `\u2028` and `\u2029`.
    line_anchor: bool,
    /// Every line anchor in the pattern was under `(?d)` (UNIX_LINES), where
    /// Java's only line terminator is `\n` -- so the crate's `crlf` is OFF
    /// and no subject needs refusing.
    unix_lines: bool,
}

/// Why a Java pattern could not be used.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JavaRegexError {
    /// Java rejects the pattern too. The text is the closest egret gets to
    /// `PatternSyntaxException.getDescription()`.
    Syntax(String),
    /// Java accepts the pattern and egret will not run it. The text names the
    /// construct, so an author reading the message knows what to rewrite.
    Unsupported(String),
}

impl fmt::Display for JavaRegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JavaRegexError::Syntax(description) => f.write_str(description),
            JavaRegexError::Unsupported(what) => write!(
                f,
                "{what} is a Java regex construct egret's regex engine does not support"
            ),
        }
    }
}

impl JavaRegex {
    /// Refuses a haystack on which this pattern's anchors would answer
    /// differently from Java's.
    ///
    /// Both clauses are narrow on purpose: a pattern with no line anchor at
    /// all never reaches either, and a haystack with ordinary content never
    /// trips either.
    pub(super) fn check_haystack(&self, haystack: &str) -> Result<(), JavaRegexError> {
        let terminators: &[char] = if self.unix_lines {
            &['\n']
        } else {
            LINE_TERMINATORS
        };
        if self.end_of_input_anchor && haystack.ends_with(terminators) {
            return Err(JavaRegexError::Unsupported(
                "an end-of-input '$' or '\\Z' against a subject that ends with a line terminator \
                 (Java matches before that terminator and egret's engine cannot)"
                    .to_string(),
            ));
        }
        // Java has no line at all in an empty subject, so `(?m)^` does not
        // match there; the crate's multi-line `^` matches at position 0.
        if self.line_anchor && haystack.is_empty() {
            return Err(JavaRegexError::Unsupported(
                "a multi-line '^' or '$' against an EMPTY subject (Java has no line there and \
                 egret's engine matches at position 0)"
                    .to_string(),
            ));
        }
        if self.line_anchor
            && !self.unix_lines
            && haystack.contains(['\u{85}', '\u{2028}', '\u{2029}'])
        {
            return Err(JavaRegexError::Unsupported(
                "a multi-line '^' or '$' against a subject containing U+0085, U+2028 or U+2029 \
                 (Java breaks lines on those and egret's engine does not)"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub(super) fn is_match(&self, haystack: &str) -> Result<bool, JavaRegexError> {
        self.check_haystack(haystack)?;
        Ok(self.regex.is_match(haystack))
    }

    pub(crate) fn group_count(&self) -> usize {
        // `captures_len` counts group 0.
        self.regex.captures_len() - 1
    }

    /// `Matcher.find()` then `Matcher.group(n)`: `None` for no match, and
    /// `Some(None)` for a group that did not participate in the match.
    pub(crate) fn find_group(
        &self,
        haystack: &str,
        group: usize,
    ) -> Result<Option<Option<String>>, JavaRegexError> {
        self.check_haystack(haystack)?;
        Ok(self
            .regex
            .captures(haystack)
            .map(|caps| caps.get(group).map(|m| m.as_str().to_string())))
    }

    /// `Pattern.split(CharSequence, int)`, ported.
    ///
    /// Java's own loop, not the `regex` crate's `split`, because three of its
    /// rules are not the crate's and each is observable:
    ///
    /// - a **zero-width match at index 0** produces no leading empty string,
    ///   so `'a.b.c'.split("")` is `["a",".","b",".","c"]` and not
    ///   `["","a",".","b",".","c",""]`;
    /// - with `limit == 0` the **trailing** empty strings are dropped, and
    ///   with any other limit they are kept: `'a,b,,c'.split(",")` is
    ///   `["a","b","","c"]` but `'a,b,,'.split(",")` is `["a","b"]`;
    /// - a **positive limit** caps the number of parts and the last part is
    ///   the whole remaining input, so `'a,b,,c'.split(",", 2)` is
    ///   `["a","b,,c"]`.
    ///
    /// It also matters that this is `find()` in a loop and not
    /// `Regex::find_iter`: the same distinction [`JavaRegex::replace_all`]
    /// documents, where the crate's iterator suppresses an empty match that
    /// abuts the previous match's end and Java's matcher takes it.
    ///
    /// **`^` and `$` split nothing rather than erroring**, because they match
    /// zero-width at index 0 / the end: `'a^b' | split('^')` is `["a^b"]`,
    /// measured. `+` and `*` are `PatternSyntaxException`s, which
    /// [`compile`] already produces.
    pub(super) fn split(&self, haystack: &str, limit: i64) -> Result<Vec<String>, JavaRegexError> {
        self.check_haystack(haystack)?;

        let limited = limit > 0;
        let cap = if limited { limit as usize } else { usize::MAX };
        let mut parts: Vec<String> = Vec::new();
        let mut index = 0usize;
        let mut from = 0usize;

        while from <= haystack.len() {
            let Some(found) = self.regex.find_at(haystack, from) else {
                break;
            };
            let (start, end) = (found.start(), found.end());
            let step = if start == end {
                // One CHARACTER on, exactly as `replace_all` does, because
                // `from` is a byte index into a `&str`.
                match haystack[end..].chars().next() {
                    Some(c) => end + c.len_utf8(),
                    None => haystack.len() + 1,
                }
            } else {
                end
            };

            if !limited || parts.len() < cap - 1 {
                // Java: `if (index == 0 && index == m.start() && m.start() ==
                // m.end()) continue;` -- no empty leading substring for a
                // zero-width match at the beginning.
                if index == 0 && start == 0 && start == end {
                    from = step;
                    continue;
                }
                parts.push(haystack[index..start].to_string());
                index = end;
            } else if parts.len() == cap - 1 {
                parts.push(haystack[index..].to_string());
                index = end;
            }
            from = step;
        }

        // "If no match was found, return this."
        if index == 0 {
            return Ok(vec![haystack.to_string()]);
        }
        if !limited || parts.len() < cap {
            parts.push(haystack[index..].to_string());
        }
        if limit == 0 {
            while parts.last().is_some_and(String::is_empty) {
                parts.pop();
            }
        }
        Ok(parts)
    }

    /// `Matcher.replaceAll(replacement)`, with Java's replacement grammar —
    /// see [`expand_replacement`] for why the crate's own cannot be used.
    pub(super) fn replace_all(
        &self,
        haystack: &str,
        replacement: &str,
    ) -> Result<String, ReplacementError> {
        self.check_haystack(haystack)
            .map_err(|e| ReplacementError(e.to_string()))?;

        // **`captures_iter` is not `Matcher.find()` in a loop.** The crate's
        // iterator suppresses an empty match that abuts the previous match's
        // end; Java's matcher takes it and then advances by one character.
        // The difference is a whole replacement:
        //
        // | expression | Java | `captures_iter` |
        // |---|---|---|
        // | `'aab' \| regexReplace('a*', '-')` | `--b-` | `-b-` |
        // | `'abc' \| regexReplace('b*', '-')` | `-a--c-` | `-a-c-` |
        //
        // Neither side errors, so this is the silent class. The loop below is
        // `Matcher.appendReplacement`'s: search from `from`, append the gap,
        // expand, and step one character on when the match was empty.
        let mut out = String::new();
        let mut appended = 0;
        let mut from = 0;
        while from <= haystack.len() {
            let Some(caps) = self.regex.captures_at(haystack, from) else {
                break;
            };
            let whole = caps.get(0).expect("group 0 always participates");
            out.push_str(&haystack[appended..whole.start()]);
            expand_replacement(&mut out, replacement, &caps, self.group_count())?;
            appended = whole.end();
            from = if whole.is_empty() {
                // One CHARACTER, not one byte: `from` is a byte index into a
                // `&str` and slicing it mid-codepoint panics.
                match haystack[whole.end()..].chars().next() {
                    Some(c) => whole.end() + c.len_utf8(),
                    None => break,
                }
            } else {
                whole.end()
            };
        }
        out.push_str(&haystack[appended..]);
        Ok(out)
    }
}

/// A failure in the *replacement* string, which Java raises as an
/// `IllegalArgumentException` or an `IndexOutOfBoundsException` rather than a
/// `PatternSyntaxException`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ReplacementError(pub(super) String);

impl fmt::Display for ReplacementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `Matcher.appendExpandedReplacement`, ported.
///
/// **The crate's own `$1` syntax is a different language and would answer
/// differently on four of thirteen measured replacements**, silently on two:
///
/// | replacement | Java | `regex` crate |
/// |---|---|---|
/// | `\$` | `$` | `\$` — `\` is not an escape there |
/// | `\\` | `\` | `\\` |
/// | `$2` with one group | `IndexOutOfBoundsException: No group 2` | `""` |
/// | `x$` | `IllegalArgumentException` | `x$` |
///
/// The two-digit rule is the subtle one: Java reads digits greedily but backs
/// off as soon as the number would exceed the group count, so `$10` against a
/// one-group pattern is group 1 followed by a literal `0` — measured, `a0`.
fn expand_replacement(
    out: &mut String,
    replacement: &str,
    caps: &regex::Captures<'_>,
    group_count: usize,
) -> Result<(), ReplacementError> {
    let chars: Vec<char> = replacement.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                i += 1;
                if i >= chars.len() {
                    return Err(ReplacementError(
                        "character to be escaped is missing".to_string(),
                    ));
                }
                out.push(chars[i]);
                i += 1;
            }
            '$' => {
                i += 1;
                if i >= chars.len() {
                    return Err(ReplacementError(
                        "Illegal group reference: group index is missing".to_string(),
                    ));
                }
                if chars[i] == '{' {
                    i += 1;
                    let start = i;
                    while i < chars.len() && chars[i].is_alphanumeric() {
                        i += 1;
                    }
                    if i == start {
                        return Err(ReplacementError(
                            "named capturing group has 0 length name".to_string(),
                        ));
                    }
                    if i >= chars.len() || chars[i] != '}' {
                        return Err(ReplacementError(
                            "named capturing group is missing trailing '}'".to_string(),
                        ));
                    }
                    let name: String = chars[start..i].iter().collect();
                    i += 1;
                    match caps.name(&name) {
                        Some(m) => out.push_str(m.as_str()),
                        None => {
                            // Java distinguishes "no such group" (an error)
                            // from "the group did not participate" (empty).
                            // The crate's `name()` folds them together, so
                            // this asks the pattern.
                            return Err(ReplacementError(format!("No group with name {{{name}}}")));
                        }
                    }
                } else if chars[i].is_ascii_digit() {
                    let mut reference = chars[i].to_digit(10).expect("checked digit") as usize;
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        let wider =
                            reference * 10 + chars[i].to_digit(10).expect("checked digit") as usize;
                        if wider > group_count {
                            break;
                        }
                        reference = wider;
                        i += 1;
                    }
                    if reference > group_count {
                        return Err(ReplacementError(format!("No group {reference}")));
                    }
                    if let Some(m) = caps.get(reference) {
                        out.push_str(m.as_str());
                    }
                } else {
                    return Err(ReplacementError("Illegal group reference".to_string()));
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(())
}

/// How deeply a character class may nest.
///
/// **Java bounds this and egret must too.** `parse_class` -> `class_member`
/// -> `parse_class` is a mutual recursion driven by the pattern text, and the
/// pattern text is a flow author's string: `regexMatch(regex='[' * 2000)`
/// SIGABRTed the worker on its 2 MiB stack, and no `catch_unwind` sees a
/// stack overflow. `rewrite::MAX_NESTING_DEPTH` does not help — it never
/// looks inside a template's string literal.
///
/// The number is Java's own, measured against the fidelity target rather than
/// chosen: Java compiles a class nested 10 000 deep and answers
/// `Stack overflow during pattern compilation` at 15 000, so 10 000 is the
/// deepest class that must still WORK.
/// **What it costs was measured, not estimated**, by bisecting the depth that
/// survives a thread of a given size — and the first version of this sentence
/// was wrong by a factor of ten, which is why the number is now derived from
/// a measurement instead of a guess:
///
/// | stack | deepest class that compiles | bytes per level |
/// |---|---:|---:|
/// | 2 MiB (a tokio worker) | 1 046 | 2 004 |
/// | 8 MiB (a libtest thread) | 4 192 | 2 001 |
///
/// so one level costs **about 2 KiB** — the parse frame, `regex-syntax`'s own
/// frame over the translated text, and the `Drop` that unwinds the same tree.
/// 10 000 levels therefore need about 20 MB, which is why
/// `env::RENDER_STACK_BYTES` is 128 MiB: a 6.4x margin. Neither a tokio
/// worker's 2 MiB nor a libtest thread's 8 MiB can hold Java's own depth,
/// which is the whole reason the render moved onto a thread of its own.
///
/// The same number caps the crate's own class nesting (`nest_limit`), which
/// recurses over the SAME text in `regex-syntax`'s parser.
pub(super) const MAX_CLASS_DEPTH: usize = 10_000;

/// Compiles a Java pattern.
pub(crate) fn compile(pattern: &str) -> Result<JavaRegex, JavaRegexError> {
    let mut translator = Translator::new(pattern);
    let translated = translator.translate()?;
    // `(?d)` (UNIX_LINES) makes `\n` Java's ONLY line terminator, so the
    // crate's `crlf` mode -- which additionally breaks on `\r\n` and `\r` --
    // is exactly wrong under it. A pattern that mixes the two spellings has
    // no single setting and is refused rather than answered under one of them.
    let unix_lines = translator.anchors_are_unix_lines.unwrap_or_default();
    let mut builder = RegexBuilder::new(&translated);
    // The crate parses the TRANSLATED text with its own recursive descent, so
    // bounding only egret's parser would move the abort rather than remove
    // it. Its default is 250, which would refuse patterns Java compiles.
    builder.nest_limit(MAX_CLASS_DEPTH as u32);
    builder.multi_line(translator.saw_multiline_anchor);
    builder.crlf(translator.saw_multiline_anchor && !unix_lines);
    match builder.build() {
        Ok(regex) => Ok(JavaRegex {
            regex,
            end_of_input_anchor: translator.saw_end_of_input_anchor,
            line_anchor: translator.saw_multiline_anchor,
            unix_lines,
        }),
        // Reached for a pattern that translated cleanly and the crate still
        // refuses -- in practice its compiled-size limit, which is a bound
        // Java does not have and which this port keeps.
        Err(e) => Err(JavaRegexError::Syntax(e.to_string())),
    }
}

/// Java's `Pattern` flags, as the scanner tracks them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Flags {
    /// `i` — CASE_INSENSITIVE. **ASCII-only in Java** unless `u` is also set,
    /// and expanded here rather than handed to the engine for exactly that
    /// reason.
    case_insensitive: bool,
    /// `u` — UNICODE_CASE. Widens `i` to full Unicode folding.
    unicode_case: bool,
    /// `s` — DOTALL.
    dot_all: bool,
    /// `m` — MULTILINE.
    multiline: bool,
    /// `x` — COMMENTS.
    comments: bool,
    /// `d` — UNIX_LINES: the only line terminator becomes `\n`.
    unix_lines: bool,
    /// `U` — UNICODE_CHARACTER_CLASS: `\w`/`\d`/`\s`/`\b` become Unicode-aware.
    unicode_classes: bool,
}

struct Translator {
    chars: Vec<char>,
    pos: usize,
    flags: Flags,
    stack: Vec<Flags>,
    saw_end_of_input_anchor: bool,
    saw_multiline_anchor: bool,
    /// Whether the pattern's line anchors were written under `(?d)`. `None`
    /// until the first one; a second one that disagrees is refused.
    anchors_are_unix_lines: Option<bool>,
    /// Whether the token just emitted can take a quantifier. Java rejects
    /// `a**` and a bare `{`; the crate accepts the first and reads the second
    /// as a literal.
    quantifiable: bool,
    /// How many `parse_class` frames are live. See [`MAX_CLASS_DEPTH`].
    class_depth: usize,
}

impl Translator {
    fn new(pattern: &str) -> Self {
        Self {
            chars: pattern.chars().collect(),
            pos: 0,
            flags: Flags::default(),
            stack: Vec::new(),
            saw_end_of_input_anchor: false,
            saw_multiline_anchor: false,
            anchors_are_unix_lines: None,
            quantifiable: false,
            class_depth: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn unsupported(&self, what: &str) -> JavaRegexError {
        JavaRegexError::Unsupported(what.to_string())
    }

    fn syntax(&self, what: &str) -> JavaRegexError {
        JavaRegexError::Syntax(format!("{what} near index {}", self.pos))
    }

    fn translate(&mut self) -> Result<String, JavaRegexError> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            match c {
                '(' => {
                    self.pos += 1;
                    self.quantifiable = false;
                    self.group(&mut out)?;
                }
                ')' => {
                    self.pos += 1;
                    self.flags = self
                        .stack
                        .pop()
                        .ok_or_else(|| self.syntax("Unmatched closing ')'"))?;
                    self.quantifiable = true;
                    out.push(')');
                }
                '[' => {
                    self.pos += 1;
                    let class = self.parse_class()?;
                    write_class(&mut out, &class, self.flags)?;
                    self.quantifiable = true;
                }
                '.' => {
                    self.pos += 1;
                    self.quantifiable = true;
                    out.push_str(self.dot());
                }
                '^' => {
                    self.pos += 1;
                    self.quantifiable = false;
                    if self.flags.multiline {
                        self.saw_multiline_anchor = true;
                        self.record_line_mode()?;
                        out.push('^');
                    } else {
                        out.push_str("\\A");
                    }
                }
                '$' => {
                    self.pos += 1;
                    self.quantifiable = false;
                    if self.flags.multiline {
                        self.saw_multiline_anchor = true;
                        self.record_line_mode()?;
                        out.push('$');
                    } else {
                        self.saw_end_of_input_anchor = true;
                        self.record_line_mode()?;
                        out.push_str("\\z");
                    }
                }
                '\\' => {
                    self.pos += 1;
                    // An escape is quantifiable unless it is an anchor, which
                    // `escape` says by leaving the flag alone.
                    self.quantifiable = true;
                    self.escape(&mut out)?;
                }
                '*' | '+' | '?' => {
                    // Java: a quantifier needs something to quantify, and a
                    // quantifier is not something to quantify. `a**` is
                    // `Dangling meta character` there and the crate accepts
                    // it, so this is a silent divergence without the check.
                    if !self.quantifiable {
                        return Err(self.syntax(&format!("Dangling meta character '{c}'")));
                    }
                    self.pos += 1;
                    self.quantifiable = false;
                    out.push(c);
                    self.quantifier_suffix(&mut out)?;
                }
                '{' => {
                    // **A `{` is never a literal in Java.** `a{`, `a{,3}` and
                    // a bare `{` are all `PatternSyntaxException`; the crate
                    // reads every one of them as a literal brace. Measured.
                    let Some(text) = self.repetition() else {
                        return Err(self.syntax("Illegal repetition"));
                    };
                    if !self.quantifiable {
                        return Err(self.syntax("Dangling meta character '{'"));
                    }
                    self.quantifiable = false;
                    out.push_str(&text);
                    self.quantifier_suffix(&mut out)?;
                }
                '|' => {
                    self.pos += 1;
                    self.quantifiable = false;
                    out.push('|');
                }
                ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}' if self.flags.comments => {
                    self.pos += 1;
                }
                '#' if self.flags.comments => {
                    while let Some(c) = self.next() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                c => {
                    self.pos += 1;
                    self.quantifiable = true;
                    write_literal(&mut out, c, self.flags);
                }
            }
        }
        if !self.stack.is_empty() {
            return Err(JavaRegexError::Syntax("Unclosed group".to_string()));
        }
        Ok(out)
    }

    /// Records whether this pattern's line anchors are `(?d)` ones.
    ///
    /// `(?d)` changes what `^`, `$` and `\Z` mean, and the crate expresses
    /// that as a whole-pattern `crlf` setting. A pattern that anchors under
    /// both spellings has no single setting, so it is refused rather than
    /// answered under one of them.
    fn record_line_mode(&mut self) -> Result<(), JavaRegexError> {
        match self.anchors_are_unix_lines {
            None => {
                self.anchors_are_unix_lines = Some(self.flags.unix_lines);
                Ok(())
            }
            Some(previous) if previous == self.flags.unix_lines => Ok(()),
            Some(_) => Err(self.unsupported(
                "a pattern whose line anchors are under both '(?d)' and the default line \
                 terminators (the two need different whole-pattern settings)",
            )),
        }
    }

    /// `.`, expanded so the line-terminator set is Java's and not the crate's.
    fn dot(&self) -> &'static str {
        if self.flags.dot_all {
            "(?s:.)"
        } else if self.flags.unix_lines {
            "[^\\n]"
        } else {
            "[^\\n\\r\\x{85}\\x{2028}\\x{2029}]"
        }
    }

    /// A `?` or `+` immediately after a quantifier. `?` is reluctant (the
    /// crate has it); `+` is possessive (it does not).
    fn quantifier_suffix(&mut self, out: &mut String) -> Result<(), JavaRegexError> {
        match self.peek() {
            Some('?') => {
                self.pos += 1;
                if self.flags.unicode_classes {
                    // `(?U)` swaps greediness in Java's UNIX-ish sense only
                    // for the `U` flag's own semantics; greediness is not
                    // affected. Kept explicit so the arm is not read as a
                    // no-op by accident.
                }
                out.push('?');
            }
            Some('+') => {
                self.pos += 1;
                return Err(
                    self.unsupported("a possessive quantifier ('*+', '++', '?+', '{n,m}+')")
                );
            }
            _ => {}
        }
        Ok(())
    }

    /// `{n}`, `{n,}` or `{n,m}` if that is what is at the cursor, consuming
    /// it; `None` (cursor unmoved) otherwise.
    fn repetition(&mut self) -> Option<String> {
        let start = self.pos;
        let mut i = self.pos + 1;
        let mut body = String::new();
        while let Some(&c) = self.chars.get(i) {
            if c == '}' {
                break;
            }
            if !c.is_ascii_digit() && c != ',' {
                self.pos = start;
                return None;
            }
            body.push(c);
            i += 1;
        }
        if self.chars.get(i) != Some(&'}') || body.is_empty() || body.starts_with(',') {
            self.pos = start;
            return None;
        }
        self.pos = i + 1;
        Some(format!("{{{body}}}"))
    }

    /// Everything after a `(`.
    fn group(&mut self, out: &mut String) -> Result<(), JavaRegexError> {
        if self.peek() != Some('?') {
            self.stack.push(self.flags);
            out.push('(');
            return Ok(());
        }
        self.pos += 1;
        match self.peek() {
            Some(':') => {
                self.pos += 1;
                self.stack.push(self.flags);
                out.push_str("(?:");
                Ok(())
            }
            Some('=') => Err(self.unsupported("a lookahead '(?=…)'")),
            Some('!') => Err(self.unsupported("a negative lookahead '(?!…)'")),
            Some('>') => Err(self.unsupported("an atomic group '(?>…)'")),
            Some('<') => {
                match self.chars.get(self.pos + 1) {
                    Some('=') => Err(self.unsupported("a lookbehind '(?<=…)'")),
                    Some('!') => Err(self.unsupported("a negative lookbehind '(?<!…)'")),
                    _ => {
                        // `(?<name>` — Java requires a letter first and then
                        // letters and digits.
                        self.pos += 1;
                        let start = self.pos;
                        while self.peek().is_some_and(|c| c != '>') {
                            self.pos += 1;
                        }
                        if self.peek() != Some('>') {
                            return Err(self.syntax("Unclosed group name"));
                        }
                        let name: String = self.chars[start..self.pos].iter().collect();
                        self.pos += 1;
                        // Java's own rule, and its own error text: a Latin
                        // letter then Latin letters and digits. An earlier cut
                        // used `char::is_alphabetic`, which accepts `(?<λ>a)`
                        // where Java raises.
                        let mut name_chars = name.chars();
                        let valid = name_chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                            && name_chars.all(|c| c.is_ascii_alphanumeric());
                        if !valid {
                            return Err(self.syntax(
                                "capturing group name does not start with a Latin letter",
                            ));
                        }
                        self.stack.push(self.flags);
                        out.push_str("(?<");
                        out.push_str(&name);
                        out.push('>');
                        Ok(())
                    }
                }
            }
            // `(?P<name>…)` is Python's spelling. Java rejects it with
            // "Unknown inline modifier"; so does this, so the two engines
            // agree that the pattern is invalid.
            Some('P') => Err(self.syntax("Unknown inline modifier")),
            _ => self.inline_flags(out),
        }
    }

    /// `(?idmsuxU)` / `(?idmsuxU-idmsuxU)` / the same with a trailing `:`.
    fn inline_flags(&mut self, out: &mut String) -> Result<(), JavaRegexError> {
        let mut flags = self.flags;
        let mut negating = false;
        loop {
            match self.next() {
                Some('-') => negating = true,
                Some(')') => {
                    // Applies to the rest of the enclosing group.
                    self.flags = flags;
                    return Ok(());
                }
                Some(':') => {
                    self.stack.push(self.flags);
                    self.flags = flags;
                    out.push_str("(?:");
                    return Ok(());
                }
                // **`U` moves TWO bits.** Java's `(?U)` sets `UNICODE_CASE`
                // as well as `UNICODE_CHARACTER_CLASS` and `(?-U)` clears
                // both. Measured three ways, because "U implies u" would be a
                // different rule and the third row separates them:
                //
                // | pattern | on `ſ` |
                // |---|---|
                // | `(?iU)[a-z]` | `true` |
                // | `(?iU)(?-U)[a-z]` | `false` |
                // | `(?iuU)(?-u)[a-z]` | `false` — a BIT, not an implication |
                // |                    | read at use time |
                //
                // Setting only the one bit made `(?iU)` fold ASCII-only, and
                // it took the flag × intersection cross-product to see it.
                Some('U') => {
                    flags.unicode_classes = !negating;
                    flags.unicode_case = !negating;
                }
                Some(c) => {
                    let slot = match c {
                        'i' => &mut flags.case_insensitive,
                        'd' => &mut flags.unix_lines,
                        'm' => &mut flags.multiline,
                        's' => &mut flags.dot_all,
                        'u' => &mut flags.unicode_case,
                        'x' => &mut flags.comments,
                        _ => return Err(self.syntax("Unknown inline modifier")),
                    };
                    *slot = !negating;
                }
                None => return Err(self.syntax("Unclosed inline modifier group")),
            }
        }
    }

    /// A `\` escape outside a character class.
    fn escape(&mut self, out: &mut String) -> Result<(), JavaRegexError> {
        let Some(c) = self.next() else {
            return Err(JavaRegexError::Syntax(
                "Trailing backslash near index 0".to_string(),
            ));
        };
        match c {
            'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'h' | 'H' | 'v' | 'V' => {
                if self.flags.unicode_classes && matches!(c, 'd' | 'D' | 'w' | 'W' | 's' | 'S') {
                    return Err(self.unsupported(UNICODE_CLASSES_REFUSAL));
                }
                out.push_str(perl_class(c));
                Ok(())
            }
            'R' => {
                out.push_str("(?:\\r\\n|[\\n\\x{b}\\x{c}\\r\\x{85}\\x{2028}\\x{2029}])");
                Ok(())
            }
            // Java's `\b` is the ASCII word boundary; the crate's is Unicode.
            // `(?-u:\b)` is the crate's spelling for Java's.
            'b' => {
                if self.peek() == Some('{') {
                    return Err(self.unsupported("'\\b{g}' (a grapheme-cluster boundary)"));
                }
                // `(?U)` makes this a UNICODE boundary. The guard was missing
                // here and present three lines above, which is exactly the
                // shape of defect a doc that names the guard hides.
                if self.flags.unicode_classes {
                    return Err(self.unsupported(UNICODE_CLASSES_REFUSAL));
                }
                self.quantifiable = false;
                out.push_str("(?-u:\\b)");
                Ok(())
            }
            'B' => {
                if self.flags.unicode_classes {
                    return Err(self.unsupported(UNICODE_CLASSES_REFUSAL));
                }
                self.quantifiable = false;
                out.push_str("(?-u:\\B)");
                Ok(())
            }
            // A grapheme cluster. The crate has no such construct and no way
            // to spell one; Java accepts it, so this is a refusal and not a
            // syntax error.
            'X' => Err(self.unsupported("'\\X' (a grapheme cluster)")),
            // `\N{NAME}` is a codepoint by its Unicode NAME. The crate's
            // `\p{name=…}` does not exist and no name table is in this
            // dependency tree.
            'N' => Err(self.unsupported("'\\N{…}' (a codepoint named by its Unicode name)")),
            'A' => {
                self.quantifiable = false;
                out.push_str("\\A");
                Ok(())
            }
            'z' => {
                self.quantifiable = false;
                out.push_str("\\z");
                Ok(())
            }
            'Z' => {
                self.quantifiable = false;
                self.saw_end_of_input_anchor = true;
                self.record_line_mode()?;
                out.push_str("\\z");
                Ok(())
            }
            'G' => Err(self.unsupported("'\\G' (the end of the previous match)")),
            'Q' => {
                // Everything up to `\E` is literal -- and still subject to
                // `(?i)`, which is why it goes through `write_literal`.
                while let Some(c) = self.next() {
                    if c == '\\' && self.peek() == Some('E') {
                        self.pos += 1;
                        return Ok(());
                    }
                    write_literal(out, c, self.flags);
                }
                Ok(())
            }
            'E' => Ok(()),
            'p' | 'P' => self.unicode_property(out, c == 'P'),
            'k' => Err(self.unsupported("'\\k<name>' (a named backreference)")),
            '1'..='9' => Err(self.unsupported("'\\1'..'\\9' (a backreference)")),
            _ => {
                match self.char_escape(c)? {
                    Some(ch) => write_literal(out, ch, self.flags),
                    // A lone surrogate: legal in Java, impossible in a Rust
                    // `&str`, so a subexpression that cannot match.
                    None => out.push_str(NEVER_MATCHES),
                }
                Ok(())
            }
        }
    }

    /// The escapes that denote one character. Shared with the class parser,
    /// which is why it returns a `char` rather than writing.
    fn char_escape(&mut self, c: char) -> Result<Option<char>, JavaRegexError> {
        match c {
            'n' => Ok(Some('\n')),
            'r' => Ok(Some('\r')),
            't' => Ok(Some('\t')),
            'f' => Ok(Some('\u{c}')),
            'a' => Ok(Some('\u{7}')),
            'e' => Ok(Some('\u{1b}')),
            '0' => {
                // `\0n`, `\0nn`, `\0mnn` -- octal, one to three digits after
                // the `0`. `\0101` is 'A'.
                let mut digits = String::new();
                while digits.len() < 3 && self.peek().is_some_and(|c| ('0'..='7').contains(&c)) {
                    digits.push(self.next().expect("peeked"));
                }
                if digits.is_empty() {
                    return Err(self.syntax("Illegal octal escape sequence"));
                }
                let n = u32::from_str_radix(&digits, 8).map_err(|_| {
                    JavaRegexError::Syntax("Illegal octal escape sequence".to_string())
                })?;
                char::from_u32(n)
                    .map(Some)
                    .ok_or_else(|| JavaRegexError::Syntax("Illegal octal escape".to_string()))
            }
            'x' => {
                if self.peek() == Some('{') {
                    self.pos += 1;
                    let start = self.pos;
                    while self.peek().is_some_and(|c| c != '}') {
                        self.pos += 1;
                    }
                    if self.peek() != Some('}') {
                        return Err(self.syntax("Unclosed hexadecimal escape sequence"));
                    }
                    let digits: String = self.chars[start..self.pos].iter().collect();
                    self.pos += 1;
                    let n = u32::from_str_radix(&digits, 16)
                        .map_err(|_| self.syntax("Illegal hexadecimal escape sequence"))?;
                    return char::from_u32(n)
                        .map(Some)
                        .ok_or_else(|| self.syntax("Hexadecimal codepoint is too big"));
                }
                self.fixed_hex(2).map(Some)
            }
            'u' => {
                let high = self.fixed_hex_value(4)?;
                // A Java pattern is UTF-16 source, so an astral codepoint is
                // written as a SURROGATE PAIR: `\ud83d\ude00` is one emoji.
                // Neither half is a `char` on its own.
                if (0xd800..0xdc00).contains(&high)
                    && self.peek() == Some('\\')
                    && self.chars.get(self.pos + 1) == Some(&'u')
                {
                    let mark = self.pos;
                    self.pos += 2;
                    match self.fixed_hex_value(4) {
                        Ok(low) if (0xdc00..0xe000).contains(&low) => {
                            let combined = 0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00);
                            return char::from_u32(combined)
                                .map(Some)
                                .ok_or_else(|| self.syntax("Illegal surrogate pair"));
                        }
                        _ => self.pos = mark,
                    }
                }
                // A LONE surrogate is a legal Java pattern that can never
                // match -- a `char` there is a UTF-16 unit and a `String`
                // never holds an unpaired one. It is not a `char` in Rust at
                // all, so `None` means "matches nothing". Measured: Java
                // compiles `\\uD83D` and answers false; an earlier cut called
                // the pattern invalid.
                Ok(char::from_u32(high))
            }
            'c' => {
                let Some(c) = self.next() else {
                    return Err(self.syntax("Illegal control escape sequence"));
                };
                char::from_u32((c as u32) ^ 64)
                    .map(Some)
                    .ok_or_else(|| self.syntax("Illegal control escape sequence"))
            }
            // Java rejects an alphanumeric escape it does not know and treats
            // every other escaped character as itself.
            c if c.is_ascii_alphanumeric() => {
                Err(self.syntax(&format!("Illegal/unsupported escape sequence '\\{c}'")))
            }
            c => Ok(Some(c)),
        }
    }

    fn fixed_hex(&mut self, width: usize) -> Result<char, JavaRegexError> {
        let n = self.fixed_hex_value(width)?;
        char::from_u32(n).ok_or_else(|| self.syntax("Illegal hexadecimal escape sequence"))
    }

    /// The scalar value, before it is checked for being a `char` — a lone
    /// surrogate is not one and has to survive long enough to be paired.
    fn fixed_hex_value(&mut self, width: usize) -> Result<u32, JavaRegexError> {
        let start = self.pos;
        for _ in 0..width {
            if !self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                return Err(self.syntax("Illegal hexadecimal escape sequence"));
            }
            self.pos += 1;
        }
        let digits: String = self.chars[start..self.pos].iter().collect();
        u32::from_str_radix(&digits, 16)
            .map_err(|_| self.syntax("Illegal hexadecimal escape sequence"))
    }

    /// `\p{…}` / `\P{…}`, with Java's name vocabulary rather than the crate's.
    fn unicode_property(&mut self, out: &mut String, negated: bool) -> Result<(), JavaRegexError> {
        let name = self.property_name()?;
        // `(?U)` redefines the POSIX names too -- 29 measured rows, none of
        // which the message used to mention and none of which the guard
        // covered. The categories (`\p{L}`, `\p{Lu}`, …) are unaffected and
        // stay allowed.
        if self.flags.unicode_classes && is_redefined_by_unicode_classes(&name) {
            return Err(self.unsupported(UNICODE_CLASSES_REFUSAL));
        }
        let translated =
            translate_property(&name).ok_or_else(|| match property_refusal(&name) {
                Some(reason) => JavaRegexError::Unsupported(reason),
                None => {
                    JavaRegexError::Syntax(format!("Unknown character property name {{{name}}}"))
                }
            })?;
        // Through the same writer a `[...]` goes through. A bare property's
        // fold is decided by the property alone: an ASCII-only one never
        // takes the Unicode fold, `(?u)` or not.
        // Through the same writer a `[...]` goes through, as a class of one
        // operand.
        let class = ClassAst {
            negated,
            operands: vec![vec![ClassItem::Property(
                false,
                translated,
                !is_ascii_only_property(&name),
            )]],
        };
        out.push_str(&class_text(&class, self.flags));
        Ok(())
    }

    fn property_name(&mut self) -> Result<String, JavaRegexError> {
        match self.peek() {
            Some('{') => {
                self.pos += 1;
                let start = self.pos;
                while self.peek().is_some_and(|c| c != '}') {
                    self.pos += 1;
                }
                if self.peek() != Some('}') {
                    return Err(self.syntax("Unclosed character family"));
                }
                let name: String = self.chars[start..self.pos].iter().collect();
                self.pos += 1;
                Ok(name)
            }
            // `\pL` -- the one-letter form.
            Some(c) => {
                self.pos += 1;
                Ok(c.to_string())
            }
            None => Err(self.syntax("Unclosed character family")),
        }
    }

    /// A `[...]` class.
    ///
    /// Java's grammar is `[ ^? union ( && union )* ]`, and the `&&` chain is
    /// **left-associative intersection of every operand**. An earlier cut read
    /// the whole tail after the first `&&` as one operand, so `[a-z&&b&&c]`
    /// became `a-z` intersected with the *set* `{b, &, c}` and matched `b` —
    /// which Java does not. `&&` is what separates operands here.
    fn parse_class(&mut self) -> Result<ClassAst, JavaRegexError> {
        // The bound is HERE rather than at each call site because this is the
        // only way into the recursion, and an unbounded one aborts the
        // process rather than failing the render.
        self.class_depth += 1;
        if self.class_depth > MAX_CLASS_DEPTH {
            // Java's own sentence for it, which is a `PatternSyntaxException`
            // and not a crash: `Pattern` catches its own `StackOverflowError`.
            return Err(JavaRegexError::Syntax(
                "Stack overflow during pattern compilation".to_string(),
            ));
        }
        let result = self.parse_class_body();
        self.class_depth -= 1;
        result
    }

    fn parse_class_body(&mut self) -> Result<ClassAst, JavaRegexError> {
        let mut class = ClassAst::default();
        if self.peek() == Some('^') {
            self.pos += 1;
            class.negated = true;
        }
        let mut operand: Vec<ClassItem> = Vec::new();
        // Set by a run of an ODD number of `&` at least 3 long; a later run
        // that contains a no-op is then a syntax error. See the `&` arm.
        let mut after_an_odd_run = false;
        // A `]` in first position is a literal `]`, in Java as in POSIX.
        if self.peek() == Some(']') {
            self.pos += 1;
            operand.push(ClassItem::Char(']'));
        }
        loop {
            let Some(c) = self.peek() else {
                return Err(JavaRegexError::Syntax(
                    "Unclosed character class".to_string(),
                ));
            };
            // `(?x)` ignores whitespace INSIDE a class too, and a `#` there
            // starts a comment. Both measured: `(?x)[a b]` does not match a
            // space, and `(?x)[a#b]` is an unclosed class in Java because the
            // comment eats the `]`.
            if self.flags.comments && (c.is_whitespace() || c == '#') {
                self.skip_insignificant();
                continue;
            }
            match c {
                ']' => {
                    self.pos += 1;
                    class.operands.push(operand);
                    // Java IGNORES an empty operand -- `[&&a]` is `[a]` --
                    // and rejects a class whose operands are all empty.
                    class.operands.retain(|o| !o.is_empty());
                    if class.operands.is_empty() {
                        return Err(self.syntax("Bad class syntax"));
                    }
                    return Ok(class);
                }
                '&' => {
                    // **A run of `&` is read as a whole**, because what Java
                    // does with it depends on the run's LENGTH and on whether
                    // an earlier run was odd. Measured over 43 patterns
                    // (`[a&^k b]` for k in 1..7, and `[a&^k1 b&^k2 c]` for
                    // k1,k2 in 1..6):
                    //
                    // | k | what the run is |
                    // |---|---|
                    // | 1 | a literal `&` |
                    // | 2 | an intersection |
                    // | 3 | a no-op `&&` and a literal `&` |
                    // | 4 | a no-op `&&` and an intersection |
                    // | odd | (k-1)/2 no-ops and a literal `&` |
                    // | even | k/2-1 no-ops and an intersection |
                    //
                    // and **a run containing a no-op is `Bad intersection
                    // syntax` if an earlier run was ODD and at least 3** —
                    // `[a&&&b&&&c]` throws while `[a&&&&b&&&&c]` and
                    // `[a&&&&&b]` do not. Counting no-ops alone was wrong in
                    // both directions.
                    // **Under `(?x)` the run is separated by whitespace, and
                    // the whitespace AFTER it changes what the run MEANS.**
                    // Java's `next()` skips insignificant whitespace while its
                    // `unread()` backs up ONE position, so a lone `&` followed
                    // by whitespace is swallowed together with it and the next
                    // character is read as a plain member. All four corners
                    // measured:
                    //
                    // | pattern | Java |
                    // |---|---|
                    // | `(?x)[a &b]` | `&` is a member |
                    // | `(?x)[a & b]` | the `&` is GONE — `{a,b}` |
                    // | `(?x)[a & ]` | `Unclosed character class`: the `]` |
                    // |              | is eaten as a member |
                    // | `(?x)[a & ]x]` | `{a,],x}` |
                    //
                    // and `(?x)[a & [bc]]` matches nothing, because the `[` is
                    // eaten as a member too, the class closes at the first
                    // `]`, and the second `]` stays in the pattern as a
                    // literal — so the regex needs TWO characters.
                    //
                    // The run itself was read with a raw `while` that never
                    // consulted the `(?x)` skip, which left 21 rows silent:
                    // `(?x)[a & & b]` is `[a&&b]`, an INTERSECTION.
                    let mut run = 0usize;
                    let mut whitespace_after_run = false;
                    loop {
                        if self.chars.get(self.pos) == Some(&'&') {
                            self.pos += 1;
                            run += 1;
                        } else {
                            break;
                        }
                        let before = self.pos;
                        self.skip_insignificant();
                        whitespace_after_run = self.pos != before;
                    }
                    let odd = run % 2 == 1;
                    let no_ops = if odd { (run - 1) / 2 } else { run / 2 - 1 };
                    if no_ops > 0 {
                        // With an empty LEFT as well, Java has nothing to
                        // union and throws. `[&&&]`, `[&&&&]`, `[&&&a&&&b]`.
                        if operand.is_empty() && class.operands.is_empty() {
                            return Err(self.syntax("Bad class syntax"));
                        }
                        if after_an_odd_run {
                            return Err(self.syntax("Bad intersection syntax"));
                        }
                    }
                    if odd && run >= 3 {
                        after_an_odd_run = true;
                    }
                    if odd {
                        if whitespace_after_run {
                            // Swallowed with the whitespace: the next member
                            // is read where the `&` was, `]` and `[` losing
                            // their meaning because Java reads it through
                            // `single()`.
                            operand.push(self.class_member()?);
                        } else {
                            // Back onto the `&` so that a RANGE still parses:
                            // `[&-a]` is a range in Java, and pushing a bare
                            // `Char('&')` here would silently split it.
                            self.pos -= 1;
                            operand.push(self.class_member()?);
                        }
                    } else {
                        class.operands.push(std::mem::take(&mut operand));
                    }
                }
                '[' => {
                    self.pos += 1;
                    let nested = self.parse_class()?;
                    operand.push(ClassItem::Nested(Box::new(nested)));
                }
                _ => operand.push(self.class_member()?),
            }
        }
    }

    /// Consumes whitespace and `#` comments where `(?x)` makes them
    /// insignificant. A no-op when the flag is off.
    fn skip_insignificant(&mut self) {
        if !self.flags.comments {
            return;
        }
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => self.pos += 1,
                Some('#') => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    /// One member of a class: a character, a range, or a shorthand.
    fn class_member(&mut self) -> Result<ClassItem, JavaRegexError> {
        let Some(c) = self.next() else {
            return Err(JavaRegexError::Syntax(
                "Unclosed character class".to_string(),
            ));
        };
        let low = if c == '\\' {
            let Some(e) = self.next() else {
                return Err(JavaRegexError::Syntax("Trailing backslash".to_string()));
            };
            match e {
                'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'h' | 'H' | 'v' | 'V' => {
                    if self.flags.unicode_classes && matches!(e, 'd' | 'D' | 'w' | 'W' | 's' | 'S')
                    {
                        return Err(self.unsupported(UNICODE_CLASSES_REFUSAL));
                    }
                    return Ok(ClassItem::Shorthand(perl_class(e)));
                }
                'p' | 'P' => {
                    let name = self.property_name()?;
                    if self.flags.unicode_classes && is_redefined_by_unicode_classes(&name) {
                        return Err(self.unsupported(UNICODE_CLASSES_REFUSAL));
                    }
                    let translated =
                        translate_property(&name).ok_or_else(|| match property_refusal(&name) {
                            Some(reason) => JavaRegexError::Unsupported(reason),
                            None => JavaRegexError::Syntax(format!(
                                "Unknown character property name {{{name}}}"
                            )),
                        })?;
                    return Ok(ClassItem::Property(
                        e == 'P',
                        translated,
                        !is_ascii_only_property(&name),
                    ));
                }
                // `[\Qa+b\E]` is three literal members. Java allows it and an
                // earlier cut rejected it as an unknown escape.
                'Q' => {
                    let mut members = Vec::new();
                    while let Some(c) = self.next() {
                        if c == '\\' && self.peek() == Some('E') {
                            self.pos += 1;
                            break;
                        }
                        members.push(ClassItem::Char(c));
                    }
                    return Ok(ClassItem::Quoted(members));
                }
                'E' => return Ok(ClassItem::Quoted(Vec::new())),
                '1'..='9' => return Err(self.unsupported("'\\1'..'\\9' (a backreference)")),
                // A lone surrogate inside a class contributes nothing, which
                // is what it matches.
                _ => match self.char_escape(e)? {
                    Some(c) => c,
                    None => return Ok(ClassItem::Quoted(Vec::new())),
                },
            }
        } else {
            c
        };
        // A range, but only when the `-` is not the last character.
        if self.peek() == Some('-')
            && self.chars.get(self.pos + 1).is_some_and(|c| *c != ']')
            && self.chars.get(self.pos + 1) != Some(&'[')
        {
            self.pos += 1;
            let Some(hi) = self.next() else {
                return Err(JavaRegexError::Syntax(
                    "Unclosed character class".to_string(),
                ));
            };
            let high = if hi == '\\' {
                let Some(e) = self.next() else {
                    return Err(JavaRegexError::Syntax("Trailing backslash".to_string()));
                };
                match self.char_escape(e)? {
                    Some(c) => c,
                    None => return Err(self.syntax("Illegal character range")),
                }
            } else {
                hi
            };
            return Ok(ClassItem::Range(low, high));
        }
        Ok(ClassItem::Char(low))
    }
}

/// A parsed `[...]`: a negation and one or more `&&`-separated operands.
#[derive(Debug, Default)]
struct ClassAst {
    negated: bool,
    operands: Vec<Vec<ClassItem>>,
}

#[derive(Debug)]
enum ClassItem {
    Char(char),
    Range(char, char),
    /// `\d` and friends, already in the crate's spelling **including** their
    /// brackets, so they are spliced in as a nested class.
    Shorthand(&'static str),
    /// `\p{…}` / `\P{…}`: (negated, body, folds-with-Unicode).
    Property(bool, String, bool),
    /// `\Q…\E` inside a class: its characters, as members.
    Quoted(Vec<ClassItem>),
    Nested(Box<ClassAst>),
}

/// Writes a class, folding ASCII case into it when `(?i)` is on.
fn write_class(out: &mut String, class: &ClassAst, flags: Flags) -> Result<(), JavaRegexError> {
    // A class that MIXES an ASCII-only property with anything else needs a
    // PER-MEMBER fold under `(?iu)`: Java folds the literal with Unicode and
    // the property with ASCII. One class-level choice cannot express that,
    // and splitting the class into an alternation would be wrong in the
    // presence of `&&`. Refused rather than answered under either fold --
    // 14 rows of a 7 182-probe sweep, and `(?iu)` over a mixed class is not
    // something a flow writes.
    if flags.case_insensitive
        && flags.unicode_case
        && class_has_ascii_only_property(class)
        && !class_is_only_ascii_properties(class)
    {
        return Err(JavaRegexError::Unsupported(
            "'(?iu)' over a class that mixes an ASCII-only property (\\p{Alpha} and its family) \
             with anything else -- Java folds the two halves differently and a single class \
             cannot"
                .to_string(),
        ));
    }
    out.push_str(&class_text(class, flags));
    Ok(())
}

/// Does this class contain a **Unicode** `\p{…}` anywhere, nesting included?
///
/// It decides which of Java's TWO case folds applies — see [`class_text`]. An
/// ASCII-only property such as `\p{Alpha}` does NOT count: it takes the ASCII
/// fold, like a literal.
fn class_has_property(class: &ClassAst) -> bool {
    class.operands.iter().flatten().any(|item| match item {
        ClassItem::Property(_, _, unicode) => *unicode,
        ClassItem::Nested(nested) => class_has_property(nested),
        ClassItem::Quoted(members) => members
            .iter()
            .any(|m| matches!(m, ClassItem::Property(_, _, true))),
        _ => false,
    })
}

/// Is this class **nothing but** ASCII-only properties?
///
/// Such a class keeps the ASCII fold even under `(?u)`: Java's `(?u)` is
/// `UNICODE_CASE`, which widens the folding of literals and ranges, and does
/// **not** widen a POSIX property — only `(?U)` does that, and `(?U)` is
/// refused. Measured: `(?iu)\p{Alpha}` does not match `ſ` or `\u212a`, while
/// `(?iu)k` matches `\u212a`.
///
/// This is the same over-fold as the bare-`(?i)` one, re-entering through the
/// `(?u)` dimension — the fixture had `(?iu)` on a LITERAL and never on a
/// property.
fn class_has_ascii_only_property(class: &ClassAst) -> bool {
    class.operands.iter().flatten().any(|item| match item {
        ClassItem::Property(_, _, unicode) => !unicode,
        ClassItem::Nested(nested) => class_has_ascii_only_property(nested),
        ClassItem::Quoted(members) => members
            .iter()
            .any(|m| matches!(m, ClassItem::Property(_, _, false))),
        _ => false,
    })
}

fn class_is_only_ascii_properties(class: &ClassAst) -> bool {
    let mut any = false;
    for item in class.operands.iter().flatten() {
        match item {
            ClassItem::Property(_, _, false) => any = true,
            ClassItem::Nested(nested) => {
                if !class_is_only_ascii_properties(nested) {
                    return false;
                }
                any = true;
            }
            _ => return false,
        }
    }
    any
}

/// A class as the crate spells it, with **whichever of Java's two case folds**
/// applies, applied WHERE Java applies it.
///
/// **There are two folds, and an earlier cut knew about one.** Its doc said
/// Java's rule is `c ∈ C or swapAsciiCase(c) ∈ C` "whatever `C` is". That is
/// true for a character, for a range, and for an ASCII-only property, and
/// **false for a Unicode `CharProperty`**: Java case-folds `\p{Lu}` with full
/// Unicode mapping even without `UNICODE_CASE`. Measured, 20 rows:
/// `(?i)\p{Lu}` matches `é`, `ς`, `ſ`, `à` and `к` in Java and matched none
/// of them here. A first cut of that fix then folded EVERY property with
/// Unicode and got the other half wrong: `(?i)\p{Alpha}` matched `ſ` and
/// `K`, which Java does not. So:
///
/// - **a Unicode property, or `(?u)`/`(?U)`** — the crate's own `(?i:…)`,
///   which is Unicode folding, which is what Java does in both cases;
/// - **everything else under `(?i)`** — the ASCII fold, computed by asking
///   the engine: write the operand, compile it, test the 52 ASCII letters.
///   That is what keeps `(?i)k` from matching the Kelvin sign, and it is
///   exact where a member-rewriting fold is not (`[Z-a]` spans `[`, `\`, `]`,
///   `^`, `_` and `` ` `` and has no mirrored range).
///
/// **And the fold is applied PER OPERAND and PER NESTING LEVEL, not to the
/// flattened class.** It happens before a negation — `(?i)[^a-c]` does not
/// match `B`, because the fold closes `a-c` into `a-cA-C` first — and it
/// distributes over the operands of an `&&`. Appending the extras to the
/// flattened text put them in the LAST operand of an intersection:
/// `(?i)[\p{Alpha}&&[^b]]` on `b` is `false` in Java and was `true` here,
/// because `[^b]` became `[^b] ∪ {b,B}`, which is everything. Union is the
/// only operation the fold distributes over for free, which is why an operand
/// (always a union) may be folded whole and an intersection may not.
fn class_text(class: &ClassAst, flags: Flags) -> String {
    let mut out = String::new();
    write_class_text(&mut out, class, flags);
    out
}

/// [`class_text`], writing in place.
///
/// **In place because it was quadratic.** Returning a `String` per level and
/// embedding it in the parent's copies the whole subtree once per level: a
/// well-formed class nested 9 999 deep — inside `MAX_CLASS_DEPTH`, so it must
/// work — took **10 s**, and 14 s at only 4 000 under `(?i)`. Doubling the
/// depth quadrupled the time, which is the signature. It is linear now.
fn write_class_text(out: &mut String, class: &ClassAst, flags: Flags) {
    // `(?u)` widens the fold of literals and ranges; it does NOT widen an
    // ASCII-only property. So a class that is nothing but those keeps the
    // ASCII fold even under `(?iu)`.
    let unicode_fold =
        class_has_property(class) || (flags.unicode_case && !class_is_only_ascii_properties(class));
    if flags.case_insensitive && unicode_fold {
        // ONE group: the crate's `(?i)` folds each member, across `&&` and
        // `^` as well, which is what Java does under this fold.
        //
        // **Nested classes go in UNFOLDED.** A nested `class_text` here would
        // emit its own `(?i:…)`, and inside a character class that group's
        // punctuation is not syntax — `(`, `?`, `i`, `:` and `)` would join
        // the class as members. 13 rows of a 63-probe sweep, every one of
        // them a boolean egret answered wrongly; the group is written once,
        // at the top, and everything below it is plain text.
        out.push_str("(?i:");
        write_plain_class(out, class, flags);
        out.push(')');
        return;
    }

    // The ASCII fold, computed by ASKING THE ENGINE: write the operand's own
    // members, compile them, and test the 52 ASCII letters. That is exact
    // where a member-rewriting fold is not -- `[Z-a]` spans `[`, `\`, `]`,
    // `^`, `_` and `` ` `` and has no mirrored range.
    let bracket = class.negated || class.operands.len() > 1;
    if class.negated {
        out.push_str("[^");
    } else if bracket {
        out.push('[');
    }
    for (index, operand) in class.operands.iter().enumerate() {
        if index > 0 {
            out.push_str("&&");
        }
        out.push('[');
        // **The probe sees the operand's OWN members only**, and that is not
        // an approximation: a nested class was folded at ITS level, so it is
        // already closed under the ASCII fold and can contribute no
        // counterpart the parent does not already have. Union is the only
        // operation the fold distributes over, and an operand is a union.
        let mut own = String::new();
        for item in operand {
            write_own_member(&mut own, item, flags);
        }
        if flags.case_insensitive {
            let extras = ascii_case_extras(&own);
            own.push_str(&extras);
        }
        out.push_str(&own);
        for item in operand {
            if let ClassItem::Nested(nested) = item {
                if !is_plain_union(nested) {
                    // Folded at ITS level, which is what makes a nested
                    // `[^b]` become `[^bB]` under `(?i)` rather than being
                    // unioned with the extras of the class that contains it.
                    write_class_text(out, nested, flags);
                }
            }
        }
        out.push(']');
    }
    if class.negated || bracket {
        out.push(']');
    }
}

/// One member inside the Unicode fold's group, flattening a nested union all
/// the way down — the same quadratic that [`is_plain_union`] describes.
fn write_plain_member(out: &mut String, item: &ClassItem, flags: Flags) {
    match item {
        ClassItem::Nested(nested) if is_plain_union(nested) => {
            for inner in &nested.operands[0] {
                write_plain_member(out, inner, flags);
            }
        }
        ClassItem::Nested(nested) => write_plain_class(out, nested, flags),
        item => write_class_item(out, item, flags),
    }
}

/// Is this nested class **just a union**, with no negation and no `&&`?
///
/// `[[[[a]]]]` is `[a]`, and writing the brackets out costs more than the
/// characters: `regex-syntax` unions a nested class into its parent at every
/// level, which is quadratic in the nesting depth. A well-formed class nested
/// 9 999 deep — inside `MAX_CLASS_DEPTH`, so it must WORK, and Java compiles
/// it — took **10 s** of CPU for one render, which is a denial of service
/// with a 20 KB pattern. Flattened, the same pattern is linear.
///
/// A negation or an `&&` is NOT flattenable: `[a[^b]]` is not `[a^b]` and
/// `[a[b&&c]]` is not `[ab&&c]`. Only the union case is inlined.
fn is_plain_union(class: &ClassAst) -> bool {
    !class.negated && class.operands.len() == 1
}

/// One member's own text, inlining a nested class that is a plain union.
///
/// A union's members join the parent's directly, which is also why the ASCII
/// fold stays correct: the probe sees them, and the fold distributes over a
/// union.
fn write_own_member(own: &mut String, item: &ClassItem, flags: Flags) {
    match item {
        ClassItem::Nested(nested) if is_plain_union(nested) => {
            for inner in &nested.operands[0] {
                write_own_member(own, inner, flags);
            }
        }
        ClassItem::Nested(_) => {}
        item => write_class_item(own, item, flags),
    }
}

/// A class exactly as written — brackets, negation, operands and nesting —
/// with **no fold applied at any level**. Used inside the one `(?i:…)` the
/// Unicode fold emits, where the crate does the folding instead.
fn write_plain_class(out: &mut String, class: &ClassAst, flags: Flags) {
    out.push('[');
    if class.negated {
        out.push('^');
    }
    for (index, operand) in class.operands.iter().enumerate() {
        if index > 0 {
            out.push_str("&&[");
        }
        for item in operand {
            write_plain_member(out, item, flags);
        }
        if index > 0 {
            out.push(']');
        }
    }
    out.push(']');
}

/// The ASCII letters a `(?i)` operand must also accept.
///
/// Compiles the operand as written and asks it about the other case of each
/// of the 52 letters. A class the crate cannot compile yields nothing — the
/// caller is about to fail on it anyway, and guessing here would be worse.
fn ascii_case_extras(inner: &str) -> String {
    let Ok(probe) = Regex::new(&format!("^(?:[{inner}])$")) else {
        return String::new();
    };
    let mut extras = String::new();
    for c in ('a'..='z').chain('A'..='Z') {
        let other = ascii_case_counterpart(c).expect("every letter has one");
        if probe.is_match(&other.to_string()) && !probe.is_match(&c.to_string()) {
            write_class_char(&mut extras, c);
        }
    }
    extras
}

fn write_class_item(out: &mut String, item: &ClassItem, flags: Flags) {
    match item {
        ClassItem::Char(c) => write_class_char(out, *c),
        ClassItem::Range(a, b) => {
            write_class_char(out, *a);
            out.push('-');
            write_class_char(out, *b);
        }
        ClassItem::Shorthand(text) => out.push_str(text),
        ClassItem::Property(negated, body, _) => {
            if *negated {
                out.push_str("[^");
                out.push_str(body);
                out.push(']');
            } else {
                out.push('[');
                out.push_str(body);
                out.push(']');
            }
        }
        ClassItem::Quoted(members) => {
            for member in members {
                write_class_item(out, member, flags);
            }
        }
        // Reached only from `write_plain_class`'s sibling arm and from a
        // caller that already handles nesting itself; folded at its level.
        ClassItem::Nested(nested) => write_class_text(out, nested, flags),
    }
}

fn write_class_char(out: &mut String, c: char) {
    match c {
        '\\' | ']' | '[' | '^' | '-' | '&' | '~' => {
            out.push('\\');
            out.push(c);
        }
        c if (c as u32) < 0x20 || c as u32 == 0x7f => {
            out.push_str(&format!("\\x{{{:x}}}", c as u32));
        }
        c => out.push(c),
    }
}

/// One literal character, folded here when `(?i)` is on because Java's
/// case-insensitivity is **ASCII-only** and the crate's `(?i)` is not.
///
/// `(?i)k` matches `K` (U+212A, KELVIN SIGN) under the crate and does not
/// under Java; expanding the literal to `[kK]` is what keeps the two the same.
fn write_literal(out: &mut String, c: char, flags: Flags) {
    if flags.case_insensitive {
        if flags.unicode_case {
            // `(?iu)` really is the crate's own `(?i)`.
            out.push_str("(?i:");
            push_escaped(out, c);
            out.push(')');
            return;
        }
        if let Some(other) = ascii_case_counterpart(c) {
            out.push('[');
            write_class_char(out, c);
            write_class_char(out, other);
            out.push(']');
            return;
        }
    }
    push_escaped(out, c);
}

fn push_escaped(out: &mut String, c: char) {
    if (c as u32) < 0x20 || c as u32 == 0x7f {
        out.push_str(&format!("\\x{{{:x}}}", c as u32));
        return;
    }
    if "\\.+*?()|[]{}^$#& ".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

fn ascii_case_counterpart(c: char) -> Option<char> {
    match c {
        'a'..='z' => Some(c.to_ascii_uppercase()),
        'A'..='Z' => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// Java's predefined classes, spelled out. Each is a bracket expression so it
/// can be spliced into a class as well as used alone.
///
/// These are the JDK's own definitions (`Pattern`'s javadoc): ASCII for
/// `\d \w \s`, and the horizontal/vertical whitespace sets for `\h \v`. The
/// crate's `\d \w \s` are Unicode and its `\v` is a vertical **tab**, which is
/// why none of them is passed through.
fn perl_class(c: char) -> &'static str {
    match c {
        'd' => "[0-9]",
        'D' => "[^0-9]",
        'w' => "[0-9A-Za-z_]",
        'W' => "[^0-9A-Za-z_]",
        's' => "[ \\t\\n\\x{b}\\x{c}\\r]",
        'S' => "[^ \\t\\n\\x{b}\\x{c}\\r]",
        'h' => "[ \\t\\x{a0}\\x{1680}\\x{180e}\\x{2000}-\\x{200a}\\x{202f}\\x{205f}\\x{3000}]",
        'H' => "[^ \\t\\x{a0}\\x{1680}\\x{180e}\\x{2000}-\\x{200a}\\x{202f}\\x{205f}\\x{3000}]",
        'v' => "[\\n\\x{b}\\x{c}\\r\\x{85}\\x{2028}\\x{2029}]",
        'V' => "[^\\n\\x{b}\\x{c}\\r\\x{85}\\x{2028}\\x{2029}]",
        other => unreachable!("{other} is not a predefined class"),
    }
}

/// The thirteen Java property names that are defined over ASCII only.
///
/// **They take Java's ASCII case fold, and every other property takes the
/// Unicode one.** Measured both ways: `(?i)\p{Lu}` matches `é`, `ς`, `ſ`,
/// `à` and `к`, and `(?i)\p{Alpha}` matches none of them — `ſ` folds to `s`
/// and `\u212a` folds to `k`, and neither is in an ASCII class.
fn is_ascii_only_property(name: &str) -> bool {
    is_redefined_by_unicode_classes(name) || matches!(name, "Cntrl" | "ASCII")
}

/// The eleven Java property names `(?U)` redefines.
///
/// `Cntrl` and `ASCII` are ASCII-only names too and are NOT here: measured,
/// `(?U)` leaves both alone.
fn is_redefined_by_unicode_classes(name: &str) -> bool {
    matches!(
        name,
        "Alpha"
            | "Alnum"
            | "Graph"
            | "Print"
            | "Upper"
            | "Lower"
            | "Punct"
            | "Digit"
            | "Space"
            | "Blank"
            | "XDigit"
    )
}

/// Java's `\p{…}` vocabulary -> the crate's, as a **class body** (no
/// brackets), so the caller decides the negation.
///
/// Java accepts four families of name here and the crate accepts a different
/// four, and the overlap is not the whole of either. Measured against the
/// harness: `\p{Greek}` is `Unknown character property name` in Java and
/// compiles in the crate, so a bare script name is refused here too; and
/// `\p{Alnum}`, `\p{XDigit}`, `\p{Blank}`, `\p{Graph}` and `\p{Print}` are
/// Java POSIX names the crate does not know at all.
fn translate_property(name: &str) -> Option<String> {
    // POSIX, which Java spells `\p{Alpha}` and the crate spells `[:alpha:]`.
    let posix = match name {
        "Lower" => Some("[:lower:]"),
        "Upper" => Some("[:upper:]"),
        "ASCII" => Some("[:ascii:]"),
        "Alpha" => Some("[:alpha:]"),
        "Digit" => Some("[:digit:]"),
        "Alnum" => Some("[:alnum:]"),
        "Punct" => Some("[:punct:]"),
        "Graph" => Some("[:graph:]"),
        "Print" => Some("[:print:]"),
        "Blank" => Some("[:blank:]"),
        "Cntrl" => Some("[:cntrl:]"),
        "XDigit" => Some("[:xdigit:]"),
        "Space" => Some("[:space:]"),
        _ => None,
    };
    if let Some(posix) = posix {
        return Some(posix.to_string());
    }

    // Java's own name for "every codepoint".
    if name == "all" {
        return Some("\\x00-\\x{10FFFF}".to_string());
    }

    // A one- or two-letter general category, which both engines spell the
    // same way.
    if (1..=2).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphabetic())
        && name.chars().next().is_some_and(char::is_uppercase)
    {
        return Some(format!("\\p{{{name}}}"));
    }

    // `gc=`, `general_category=`, `sc=`, `script=` -- the crate takes all
    // four spellings.
    for prefix in ["gc=", "general_category=", "sc=", "script="] {
        if let Some(value) = name.strip_prefix(prefix) {
            return Some(format!("\\p{{{prefix}{value}}}"));
        }
    }

    // `\p{IsX}`: a script or a binary property in Java. `Is` is not a prefix
    // the crate knows, so it is stripped and the remainder handed over.
    if let Some(value) = name.strip_prefix("Is") {
        if !value.is_empty() {
            return Some(format!("\\p{{{value}}}"));
        }
    }

    None
}

/// The Java property names egret refuses rather than mis-answers, each with
/// its reason. Distinguished from an unknown name, which is a syntax error on
/// both sides.
fn property_refusal(name: &str) -> Option<String> {
    if name.starts_with("In") || name.starts_with("block=") || name.starts_with("blk=") {
        return Some(format!(
            "'\\p{{{name}}}' (a Unicode BLOCK -- egret's regex engine carries scripts and \
             categories but no block tables)"
        ));
    }
    if name.starts_with("java") {
        return Some(format!("'\\p{{{name}}}' (a java.lang.Character predicate)"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, haystack: &str) -> bool {
        compile(pattern)
            .unwrap_or_else(|e| panic!("{pattern}: {e}"))
            .is_match(haystack)
            .unwrap_or_else(|e| panic!("{pattern}: {e}"))
    }

    /// The eight silent divergences from the module doc, each asserted as
    /// Java answers it. Every expectation here is a value the Java harness
    /// produced; every one of them is the OPPOSITE of what the `regex` crate
    /// answers for the untranslated pattern, which is what makes this test
    /// able to fail if the translation is removed.
    /// **The sweep, as a fixture.** `oracle/regex-classes.txt` records what
    /// Java answers for 1 868 (pattern, subject) pairs and what egret must
    /// answer, and this asserts both directions.
    ///
    /// It was 171 rows and sampled where the answers agree — `(?iu)` on a
    /// LITERAL and never on a property, one `&&&` run and never two. Both
    /// gaps leaked a silent class. It is now a cross-product along those two
    /// dimensions.
    ///
    /// It exists because the claim it checks — "after the translation nothing
    /// is silent" — was made once as a sentence and was **false**: a review
    /// found thirteen classes the sentence had not been checked against,
    /// nine of them silent. A sentence cannot fail; this can.
    ///
    /// Three rules, and the third is the point:
    ///
    /// - a row whose two columns DIFFER must carry a reason;
    /// - a row whose columns AGREE must not, so a refusal cannot claim a
    ///   divergence it does not have;
    /// - **a row whose `egret` column is a boolean and differs from `java`
    ///   fails outright.** That is the silent class, and no reason excuses
    ///   one — the only acceptable divergence is a refusal that says so.
    #[test]
    fn the_regex_classes_match_the_engine() {
        let fixture = include_str!("../../oracle/regex-classes.txt");
        let mut rows = 0;
        for line in fixture.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(cols.len(), 5, "five tab-separated columns: {line}");
            let (pattern, subject, java, declared, reason) = (
                unescape(cols[0]),
                unescape(cols[1]),
                cols[2],
                cols[3],
                cols[4],
            );
            rows += 1;

            if java == declared {
                assert_eq!(
                    reason, "-",
                    "`{pattern}` on {subject:?}: the engine and egret agree, so there is nothing \
                     to explain"
                );
            } else {
                assert!(
                    declared == "refused" || declared == "error",
                    "`{pattern}` on {subject:?}: Java answers {java} and egret answers \
                     {declared}. A DIFFERENT ANSWER is the silent class this module exists to \
                     eliminate; the only acceptable divergence is a refusal."
                );
                assert!(
                    reason.len() > 20,
                    "`{pattern}` on {subject:?}: the engine says {java} and egret says \
                     {declared}; a divergence must say why, in the file"
                );
            }

            let actual = match compile(&pattern) {
                Err(JavaRegexError::Unsupported(_)) => "refused".to_string(),
                Err(JavaRegexError::Syntax(_)) => "error".to_string(),
                Ok(re) => match re.is_match(&subject) {
                    Ok(true) => "true".to_string(),
                    Ok(false) => "false".to_string(),
                    Err(JavaRegexError::Unsupported(_)) => "refused".to_string(),
                    Err(_) => "error".to_string(),
                },
            };
            assert_eq!(
                actual, declared,
                "`{pattern}` on {subject:?}: the file says egret answers {declared} and it \
                 answers {actual}"
            );
        }
        assert_eq!(rows, 2916, "the fixture lost rows");
    }

    /// The fixture's `java` column must be what the **committed harness
    /// answers** say, so a hand-edited fixture fails and a Kestra bump moves
    /// the answers file visibly.
    ///
    /// Without this, that column was a snapshot nothing could re-derive —
    /// unlike `golden.json`, which the harness regenerates. `oracle/fixtures/`
    /// carries the probe corpus and the harness's reply beside it; see
    /// `oracle/README.md`, "Regenerating the two answer fixtures".
    #[test]
    fn the_regex_fixtures_java_column_is_the_harnesss_own_answer() {
        #[derive(serde::Deserialize)]
        struct Probe {
            id: String,
            context: std::collections::BTreeMap<String, serde_json::Value>,
        }
        #[derive(serde::Deserialize)]
        struct Answer {
            id: String,
            #[serde(default)]
            result: Option<serde_json::Value>,
        }
        let probes: Vec<Probe> = serde_json::from_str(include_str!(
            "../../oracle/fixtures/regex-classes.probe.json"
        ))
        .expect("the probe corpus must parse");
        let answers: Vec<Answer> = serde_json::from_str(include_str!(
            "../../oracle/fixtures/regex-classes.answers.json"
        ))
        .expect("the harness answers must parse");
        let by_id: std::collections::BTreeMap<&str, &Answer> =
            answers.iter().map(|a| (a.id.as_str(), a)).collect();

        let mut harness: std::collections::BTreeMap<(String, String), String> =
            std::collections::BTreeMap::new();
        for probe in probes.iter().filter(|p| p.id.starts_with("re-")) {
            let answer = by_id
                .get(probe.id.as_str())
                .expect("every probe is answered");
            let value = match &answer.result {
                Some(serde_json::Value::String(text)) => text.clone(),
                _ => "error".to_string(),
            };
            harness.insert(
                (
                    probe.context["p"].as_str().expect("a pattern").to_string(),
                    probe.context["s"].as_str().expect("a subject").to_string(),
                ),
                value,
            );
        }

        for line in include_str!("../../oracle/regex-classes.txt").lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            let (pattern, subject, java) = (unescape(cols[0]), unescape(cols[1]), cols[2]);
            let measured = harness
                .get(&(pattern.clone(), subject.clone()))
                .unwrap_or_else(|| panic!("`{pattern}` on {subject:?} is not in the probe corpus"));
            assert_eq!(
                measured, java,
                "`{pattern}` on {subject:?}: the fixture says Kestra answers {java} and the \
                 committed harness answer is {measured}"
            );
        }
    }

    /// JSON's escapes, which is what the fixture uses so a tab-separated file
    /// can carry a subject containing a tab.
    fn unescape(field: &str) -> String {
        serde_json::from_str::<String>(&format!("\"{field}\"")).unwrap_or_else(|e| {
            panic!("{field:?} is not a JSON string body: {e}");
        })
    }

    #[test]
    fn the_predefined_classes_are_ascii_the_way_javas_are() {
        assert!(!matches(r"\d", "٣"));
        assert!(!matches(r"\d", "３"));
        assert!(matches(r"\D", "٣"));
        assert!(!matches(r"\w", "é"));
        assert!(!matches(r"\w", "中"));
        assert!(matches(r"\W", "é"));
        assert!(!matches(r"\s", "\u{a0}"));
        assert!(matches(r"\S", "\u{a0}"));
        assert!(matches(r"\d+", "abc123"));
    }

    #[test]
    fn vertical_and_horizontal_whitespace_are_javas_sets() {
        // The crate's own `\v` is a vertical tab and would not match this.
        assert!(matches(r"\v", "\n"));
        assert!(matches(r"\h", "\t"));
        assert!(matches(r"\h", "\u{a0}"));
        assert!(matches(r"\V", "a"));
        assert!(matches(r"\R", "\r\n"));
    }

    #[test]
    fn the_dot_excludes_every_java_line_terminator() {
        assert!(!matches("a.b", "a\nb"));
        assert!(!matches("a.b", "a\rb"));
        assert!(!matches("a.b", "a\u{2028}b"));
        assert!(!matches("a.b", "a\u{85}b"));
        assert!(matches("(?s)a.b", "a\nb"));
        assert!(matches("(?d)a.b", "a\u{85}b"));
    }

    #[test]
    fn word_boundaries_are_ascii() {
        assert!(matches(r"\bcat\b", "the cat sat"));
        assert!(!matches(r"\bcafé\b", "le café noir"));
        assert!(matches(r"\Bat", "cat"));
    }

    /// Java's `(?i)` does not fold outside ASCII; the crate's does.
    #[test]
    fn case_insensitivity_is_ascii_only() {
        assert!(matches("(?i)ABC", "abc"));
        assert!(matches("(?i)abc", "ABC"));
        assert!(!matches("(?i)k", "\u{212a}"));
        assert!(!matches("(?i)ss", "ß"));
        assert!(!matches("(?i)I", "ı"));
        assert!(matches("(?i:ab)c", "ABc"));
        assert!(matches("(?i)a(?-i:B)", "aB"));
        assert!(!matches("(?i)a(?-i:B)", "ab"));
        assert!(matches("(?i)[a-c]+", "ABC"));
        assert!(!matches("(?i)[a-c]", "D"));
    }

    /// `\Q…\E` is still subject to `(?i)`, and its content is never regex.
    #[test]
    fn quoted_literals_are_literal() {
        assert!(matches(r"\Qa+b\E", "a+b"));
        assert!(!matches(r"\Qa+b\E", "aab"));
        assert!(matches(r"x\Q.\Ey", "x.y"));
        assert!(!matches(r"x\Q.\Ey", "xzy"));
        assert!(matches(r"(?i)\Qab\E", "AB"));
    }

    #[test]
    fn the_numeric_escapes_denote_the_characters_java_says() {
        assert!(matches(r"\x41", "A"));
        assert!(matches(r"\x{1F642}", "\u{1f642}"));
        assert!(matches(r"\u0041", "A"));
        assert!(matches(r"\0101", "A"));
        assert!(matches(r"\cA", "\u{1}"));
        assert!(matches(r"\e", "\u{1b}"));
        assert!(matches(r"\a", "\u{7}"));
    }

    #[test]
    fn java_posix_names_and_categories_translate() {
        assert!(matches(r"\p{Alpha}+", "abc"));
        assert!(matches(r"\p{Alnum}", "a"));
        assert!(matches(r"\p{XDigit}", "f"));
        assert!(matches(r"\p{Blank}", "\t"));
        assert!(matches(r"\p{Graph}", "a"));
        assert!(matches(r"\p{Print}", "a"));
        assert!(matches(r"\p{Punct}", "!"));
        assert!(matches(r"\p{L}+", "café"));
        assert!(matches(r"\p{Lu}", "A"));
        assert!(matches(r"\P{L}", "1"));
        assert!(matches(r"\p{IsLatin}", "a"));
        assert!(matches(r"\p{sc=Greek}", "α"));
        assert!(matches(r"\p{gc=Lu}", "A"));
    }

    /// A bare script name is a Java syntax error, so it must be one here too;
    /// the crate accepts it, which is the direction that would have made
    /// egret answer a question Kestra refuses.
    #[test]
    fn a_bare_script_name_is_refused_the_way_java_refuses_it() {
        assert!(matches!(
            compile(r"\p{Greek}"),
            Err(JavaRegexError::Syntax(_))
        ));
    }

    #[test]
    fn class_operators_survive_translation() {
        assert!(matches(r"[a-z&&[^c]]+", "abc"));
        assert!(!matches(r"[a-z&&[^c]]", "c"));
        assert!(matches(r"[[a][b]]+", "ab"));
        assert!(matches(r"[]]", "]"));
        assert!(matches(r"[a-]", "-"));
        assert!(matches(r"[&]", "&"));
        assert!(matches(r"[^\W]", "a"));
    }

    /// Each refusal names its own construct, so the message tells an author
    /// what to rewrite. Java accepts every one of these.
    #[test]
    fn the_backtracking_constructs_are_refused_by_name() {
        for (pattern, needle) in [
            ("a(?=b)", "lookahead"),
            ("a(?!b)", "negative lookahead"),
            ("(?<=a)b", "lookbehind"),
            ("(?<!a)b", "negative lookbehind"),
            ("(?>a+)b", "atomic group"),
            (r"(a)\1", "backreference"),
            (r"(?<x>a)\k<x>", "named backreference"),
            ("a++", "possessive quantifier"),
            ("a{2,3}+", "possessive quantifier"),
            (r"\Ga", "\\G"),
            (r"a\b{g}b", "grapheme-cluster"),
            (r"\p{InGreek}", "Unicode BLOCK"),
            (r"\p{block=Greek}", "Unicode BLOCK"),
            (r"\p{javaLowerCase}", "java.lang.Character"),
            // `(?U)` reaches this from FIVE places -- `\d`, `\w`, `\s`,
            // `\b`, and every POSIX property, inside a class and out. The
            // message named `\b` while the `\b` arm had no guard, and it
            // never named the POSIX family at all; 29 measured rows.
            (r"(?U)\d", "(?U)"),
            (r"(?U)\w", "(?U)"),
            (r"(?U)\s", "(?U)"),
            (r"(?U)\bx\b", "(?U)"),
            (r"(?U)\p{Alpha}", "(?U)"),
            (r"(?U)\p{Upper}", "(?U)"),
            (r"(?U)[\p{Digit}]", "(?U)"),
            (r"(?U)[\d]", "(?U)"),
            (r"\X", "grapheme cluster"),
            (r"\N{LATIN SMALL LETTER A}", "Unicode name"),
        ] {
            match compile(pattern) {
                Err(JavaRegexError::Unsupported(message)) => assert!(
                    message.contains(needle),
                    "{pattern}: {message:?} does not name {needle:?}"
                ),
                other => panic!("{pattern} was not refused: {other:?}"),
            }
        }
    }

    /// `$` is exact where the two engines agree and refuses where they do
    /// not, rather than answering `false` where Java answers `true`.
    #[test]
    fn an_end_anchor_refuses_only_the_haystack_the_engines_disagree_about() {
        assert!(matches("c$", "abc"));
        assert!(!matches("c$", "abcd"));
        let re = compile("c$").expect("compiles");
        match re.is_match("abc\n") {
            Err(JavaRegexError::Unsupported(message)) => {
                assert!(message.contains("line terminator"), "{message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        // No end anchor, so the same haystack is fine.
        assert!(matches("c", "abc\n"));
    }

    #[test]
    fn multiline_anchors_use_the_crates_line_mode() {
        assert!(matches("(?m)^b", "a\nb"));
        assert!(!matches("^b", "a\nb"));
        assert!(matches("(?m)^b$", "a\nb"));
        assert!(matches("(?m)^b", "a\r\nb"));
    }

    /// The three refusals that need the SUBJECT as well as the pattern.
    ///
    /// None of them can be carried by a corpus entry: Kestra answers and
    /// egret refuses, so the entry is baselined either way and removing the
    /// refusal leaves it baselined. Only an assertion on the message can
    /// fail, which is why they are here.
    #[test]
    fn the_subject_dependent_refusals_name_their_reason() {
        let end_anchor = compile("c$").expect("compiles");
        assert!(end_anchor.is_match("abc").expect("no terminator"));
        let message = end_anchor
            .is_match("abc\n")
            .expect_err("a trailing terminator must be refused")
            .to_string();
        assert!(message.contains("line terminator"), "{message}");

        // Java has no line in an empty subject; the crate's `(?m)^` matches
        // at position 0.
        let multiline = compile("(?m)^").expect("compiles");
        let message = multiline
            .is_match("")
            .expect_err("an empty subject must be refused")
            .to_string();
        assert!(message.contains("EMPTY subject"), "{message}");
        assert!(multiline
            .is_match("a")
            .expect("a non-empty subject is fine"));

        // U+0085 is a line terminator to Java and not to the crate.
        let message = multiline
            .is_match("a\u{85}b")
            .expect_err("an exotic terminator must be refused")
            .to_string();
        assert!(message.contains("U+0085"), "{message}");
    }

    /// Java's replacement grammar, on the four measured cases the crate's own
    /// would have answered differently.
    #[test]
    fn the_replacement_grammar_is_javas() {
        let re = compile("a").expect("compiles");
        assert_eq!(re.replace_all("a", "\\$").expect("ok"), "$");
        assert_eq!(re.replace_all("a", "\\\\").expect("ok"), "\\");
        assert_eq!(
            re.replace_all("a", "x$"),
            Err(ReplacementError(
                "Illegal group reference: group index is missing".to_string()
            ))
        );
        assert_eq!(
            re.replace_all("a", "${"),
            Err(ReplacementError(
                "named capturing group has 0 length name".to_string()
            ))
        );

        let one_group = compile("(a)").expect("compiles");
        assert_eq!(
            one_group.replace_all("a", "$2"),
            Err(ReplacementError("No group 2".to_string()))
        );
        // `$10` backs off to group 1 and a literal `0`.
        assert_eq!(one_group.replace_all("a", "$10").expect("ok"), "a0");

        let two = compile(r"(\d)(\d)").expect("compiles");
        assert_eq!(two.replace_all("12", "$2$1").expect("ok"), "21");

        let named = compile(r"(?<a>\d)").expect("compiles");
        assert_eq!(named.replace_all("1", "${a}").expect("ok"), "1");
    }

    /// A pattern whose Java run is exponential is answered here in the time
    /// one linear scan takes.
    ///
    /// Kestra bounds this with `RegexUtils`' ten-second `TimeoutCharSequence`
    /// and then throws. There is nothing to bound here: the crate cannot
    /// backtrack. This is the divergence stated in the module doc, as a test
    /// rather than a sentence.
    #[test]
    fn a_catastrophic_java_pattern_is_answered_instead_of_timed_out() {
        let started = std::time::Instant::now();
        let subject = "a".repeat(40) + "!";
        assert!(!matches("(a+)+b", &subject));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "a linear-time engine should not spend {:?} here",
            started.elapsed()
        );
    }

    /// `(?x)` ignores whitespace and comments **inside a character class as
    /// well**, which is the half this test used to omit while being named for
    /// the whole feature: `(?x)[a b]` does not match a space in Java, and
    /// `(?x)[a#b]` is an unclosed class there because the comment eats the
    /// `]`. Both were wrong here and the test passed.
    #[test]
    fn comments_mode_drops_whitespace_and_comments() {
        assert!(matches("(?x) a b  # comment\n", "ab"));
        assert!(!matches("(?x) a b  # comment\n", "a b"));
        assert!(matches("(?x)[a b]", "a"));
        assert!(!matches("(?x)[a b]", " "));
        assert!(matches!(
            compile("(?x)[a#b]"),
            Err(JavaRegexError::Syntax(_))
        ));
    }

    #[test]
    fn a_group_count_is_javas_group_count() {
        assert_eq!(compile(r"(\d+)-(\d+)").expect("ok").group_count(), 2);
        assert_eq!(compile(r"(?:a)(b)").expect("ok").group_count(), 1);
        assert_eq!(compile("abc").expect("ok").group_count(), 0);
    }

    /// **The class-nesting abort, and the two sentences Java answers with.**
    ///
    /// `parse_class` -> `class_member` -> `parse_class` is a mutual recursion
    /// driven by the pattern text. Unbounded, `regexMatch(regex='[' * 2000)`
    /// SIGABRTed the worker: a stack overflow is not a panic, so neither
    /// `catch_unwind` nor the oracle's guard sees it — the process dies.
    ///
    /// Java does not die, because `Pattern` catches its own
    /// `StackOverflowError` and reports a `PatternSyntaxException`. Both of
    /// its sentences are reproduced here.
    ///
    /// **The depth at which Java switches between them is not assertable.**
    /// Measured across repetitions inside ONE JVM, the same depth flips:
    /// 7 000, 8 000 and 9 000 each answered `Stack overflow during pattern
    /// compilation` on the first repetition and `Unclosed character class` on
    /// the next two, because it depends on the live stack at compile time.
    /// So this asserts the two ends, where Java is consistent with itself,
    /// and `oracle/error-class.txt` records why the corpus entries compare by
    /// class.
    #[test]
    fn a_class_nested_past_the_cap_is_refused_the_way_java_refuses_it() {
        // **On the stack the shipped path gives it.** A libtest thread has
        // 8 MiB, which holds 4 192 of these levels -- so running this test
        // inline would abort the whole binary, and `MAX_CLASS_DEPTH` is only
        // affordable BECAUSE `render_str` moved the render onto a thread of
        // `env::RENDER_STACK_BYTES`. That makes this a pin on that constant
        // too: shrink it and this test's process dies.
        std::thread::Builder::new()
            .stack_size(crate::runners::env::RENDER_STACK_BYTES)
            .spawn(the_class_nesting_assertions)
            .expect("a render-sized thread")
            .join()
            .expect("no abort");
    }

    fn the_class_nesting_assertions() {
        // The number is Java's own and it is load-bearing: at 10 000 Java
        // still COMPILES a well-formed class, so a smaller cap would refuse
        // patterns Java accepts.
        assert_eq!(MAX_CLASS_DEPTH, 10_000);

        let past = "[".repeat(MAX_CLASS_DEPTH + 1);
        assert_eq!(
            compile(&past).err(),
            Some(JavaRegexError::Syntax(
                "Stack overflow during pattern compilation".to_string()
            )),
            "past the cap egret must answer Java's own sentence, not abort"
        );

        // Inside the cap the class is merely unterminated, which is Java's
        // other sentence -- and reaching it at all proves the recursion ran
        // 2 000 deep without dying.
        assert_eq!(
            compile(&"[".repeat(2_000)).err(),
            Some(JavaRegexError::Syntax(
                "Unclosed character class".to_string()
            ))
        );

        // A WELL-FORMED deep class must still work. This is what pins
        // `nest_limit` as well: `regex-syntax`'s own default is 250, and
        // without raising it this line fails on a pattern Java compiles.
        let deep = format!("{}a{}", "[".repeat(1_000), "]".repeat(1_000));
        assert!(compile(&deep)
            .expect("Java compiles this")
            .is_match("a")
            .unwrap());

        // And the cap counts DEPTH, not length: a pattern far longer than the
        // cap but shallow compiles.
        assert!(compile(&"a".repeat(MAX_CLASS_DEPTH * 2)).is_ok());
    }

    /// **`(?x)` inside a class, where Java's whitespace skip and its
    /// one-position `unread()` disagree.**
    ///
    /// Every expectation is a measured Java answer. The pair that matters is
    /// the first two: the same class, one space apart, and the `&` survives
    /// in one and vanishes in the other.
    #[test]
    fn the_comments_flag_changes_what_an_ampersand_run_means() {
        let m = |p: &str, s: &str| compile(p).expect("compiles").is_match(s).unwrap();

        // No whitespace after the `&`: it is a member.
        assert!(m("(?x)[a &b]", "&"));
        // Whitespace after it: Java swallows both and the `&` is GONE.
        assert!(!m("(?x)[a & b]", "&"));
        assert!(m("(?x)[a & b]", "a") && m("(?x)[a & b]", "b"));

        // Whitespace INSIDE the run does not break the run: `& &` is `&&`,
        // an intersection, and `[a&&b]` matches nothing.
        assert!(!m("(?x)[a & & b]", "a"));
        assert!(!m("(?x)[a & & b]", "b"));
        assert!(!m("(?x)[a && b]", "a"));

        // An odd run of three is a no-op plus a lone `&` -- which the
        // whitespace then swallows.
        assert!(m("(?x)[a&&&b]", "&"));
        assert!(!m("(?x)[a &&& b]", "&"));

        // The swallowed `&` takes the NEXT character with it as a member,
        // `]` included, which is how Java loses the class terminator.
        assert_eq!(
            compile("(?x)[a & ]").err(),
            Some(JavaRegexError::Syntax(
                "Unclosed character class".to_string()
            ))
        );
        assert!(m("(?x)[a & ]x]", "]") && m("(?x)[a & ]x]", "x"));

        // Without `(?x)` none of this happens: a lone `&` is a member and a
        // space is a space.
        assert!(m("[a & b]", "&") && m("[a & b]", " "));
        // And a lone `&` still starts a RANGE, which pushing a bare `&`
        // member instead of re-reading it would silently split.
        assert!(m("[&-a]", "_") && !m("[&-a]", "b"));
    }

    /// **The case fold is per operand and per nesting level.**
    ///
    /// Each of these was a silent divergence: egret answered a boolean and it
    /// was the wrong one. The mechanism is the same in both — the ASCII fold
    /// used to be appended to the FLATTENED class text, which lands the
    /// counterparts inside the last operand of an intersection and inside a
    /// negation that should already have closed over them.
    #[test]
    fn the_ascii_fold_distributes_over_operands_and_nesting() {
        let m = |p: &str, s: &str| compile(p).expect("compiles").is_match(s).unwrap();

        // `[^b]` folds to `[^bB]` BEFORE the intersection, so `b` is out.
        assert!(!m(r"(?i)[\p{Alpha}&&[^b]]", "b"));
        assert!(!m(r"(?i)[\p{Alpha}&&[^b]]", "B"));
        assert!(m(r"(?i)[\p{Alpha}&&[^b]]", "a"));
        assert!(!m(r"(?i)[a-z&&[^b]]", "B"));
        assert!(m(r"(?i)[a-z&&[^b]]", "C"));
        // The fold before a negation, which was already true and must stay.
        assert!(!m("(?i)[^a-c]", "B"));
        // And the Unicode fold still reaches across an intersection.
        assert!(!m(r"(?i)[\p{L}&&[^b]]", "b"));
    }

    /// **`(?U)` carries `UNICODE_CASE` with it, and `(?-U)` takes it back.**
    ///
    /// Three rows, because the third separates a BIT from an implication read
    /// at use time: with `(?iuU)(?-u)` the `U` is still on and Java still
    /// answers `false`.
    #[test]
    fn unicode_classes_moves_the_unicode_case_bit_too() {
        let m = |p: &str, s: &str| compile(p).expect("compiles").is_match(s).unwrap();
        assert!(m("(?iU)[a-z]", "\u{17f}"));
        assert!(!m("(?iU)(?-U)[a-z]", "\u{17f}"));
        assert!(!m("(?iuU)(?-u)[a-z]", "\u{17f}"));
        // Without it, the ASCII fold: `(?i)[a-z]` does not reach a long s.
        assert!(!m("(?i)[a-z]", "\u{17f}"));
    }
}
