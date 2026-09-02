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
const UNICODE_CLASSES_REFUSAL: &str =
    "'(?U)' (UNICODE_CHARACTER_CLASS), which redefines \\d, \\w, \\s and \\b";

const LINE_TERMINATORS: &[char] = ['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}'].as_slice();

/// A compiled Java pattern, plus what the translation had to remember about
/// it.
#[derive(Debug)]
pub(super) struct JavaRegex {
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
pub(super) enum JavaRegexError {
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

    pub(super) fn group_count(&self) -> usize {
        // `captures_len` counts group 0.
        self.regex.captures_len() - 1
    }

    /// `Matcher.find()` then `Matcher.group(n)`: `None` for no match, and
    /// `Some(None)` for a group that did not participate in the match.
    pub(super) fn find_group(
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

/// Compiles a Java pattern.
pub(super) fn compile(pattern: &str) -> Result<JavaRegex, JavaRegexError> {
    let mut translator = Translator::new(pattern);
    let translated = translator.translate()?;
    // `(?d)` (UNIX_LINES) makes `\n` Java's ONLY line terminator, so the
    // crate's `crlf` mode -- which additionally breaks on `\r\n` and `\r` --
    // is exactly wrong under it. A pattern that mixes the two spellings has
    // no single setting and is refused rather than answered under one of them.
    let unix_lines = translator.anchors_are_unix_lines.unwrap_or_default();
    let mut builder = RegexBuilder::new(&translated);
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
                Some(c) => {
                    let slot = match c {
                        'i' => &mut flags.case_insensitive,
                        'd' => &mut flags.unix_lines,
                        'm' => &mut flags.multiline,
                        's' => &mut flags.dot_all,
                        'u' => &mut flags.unicode_case,
                        'x' => &mut flags.comments,
                        'U' => &mut flags.unicode_classes,
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
                let ch = self.char_escape(c)?;
                write_literal(out, ch, self.flags);
                Ok(())
            }
        }
    }

    /// The escapes that denote one character. Shared with the class parser,
    /// which is why it returns a `char` rather than writing.
    fn char_escape(&mut self, c: char) -> Result<char, JavaRegexError> {
        match c {
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'f' => Ok('\u{c}'),
            'a' => Ok('\u{7}'),
            'e' => Ok('\u{1b}'),
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
                        .ok_or_else(|| self.syntax("Hexadecimal codepoint is too big"));
                }
                self.fixed_hex(2)
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
                                .ok_or_else(|| self.syntax("Illegal surrogate pair"));
                        }
                        _ => self.pos = mark,
                    }
                }
                char::from_u32(high).ok_or_else(|| self.syntax("Illegal Unicode escape sequence"))
            }
            'c' => {
                let Some(c) = self.next() else {
                    return Err(self.syntax("Illegal control escape sequence"));
                };
                char::from_u32((c as u32) ^ 64)
                    .ok_or_else(|| self.syntax("Illegal control escape sequence"))
            }
            // Java rejects an alphanumeric escape it does not know and treats
            // every other escaped character as itself.
            c if c.is_ascii_alphanumeric() => {
                Err(self.syntax(&format!("Illegal/unsupported escape sequence '\\{c}'")))
            }
            c => Ok(c),
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
        let translated =
            translate_property(&name).ok_or_else(|| match property_refusal(&name) {
                Some(reason) => JavaRegexError::Unsupported(reason),
                None => {
                    JavaRegexError::Syntax(format!("Unknown character property name {{{name}}}"))
                }
            })?;
        // Through the same writer a `[...]` goes through, so `(?i)\p{Lower}`
        // gets the ASCII fold that makes it match `A`. Writing the brackets
        // here directly is what made it not.
        emit_class(out, &translated, negated, self.flags);
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
        let mut class = ClassAst::default();
        if self.peek() == Some('^') {
            self.pos += 1;
            class.negated = true;
        }
        let mut operand: Vec<ClassItem> = Vec::new();
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
            if self.flags.comments {
                if c.is_whitespace() {
                    self.pos += 1;
                    continue;
                }
                if c == '#' {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                    continue;
                }
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
                '&' if self.chars.get(self.pos + 1) == Some(&'&') => {
                    self.pos += 2;
                    class.operands.push(std::mem::take(&mut operand));
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
                    let translated =
                        translate_property(&name).ok_or_else(|| match property_refusal(&name) {
                            Some(reason) => JavaRegexError::Unsupported(reason),
                            None => JavaRegexError::Syntax(format!(
                                "Unknown character property name {{{name}}}"
                            )),
                        })?;
                    return Ok(ClassItem::Property(e == 'P', translated));
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
                _ => self.char_escape(e)?,
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
                self.char_escape(e)?
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
    /// `\p{…}` / `\P{…}`: (negated, body).
    Property(bool, String),
    /// `\Q…\E` inside a class: its characters, as members.
    Quoted(Vec<ClassItem>),
    Nested(Box<ClassAst>),
}

/// Writes a class, folding ASCII case into it when `(?i)` is on.
fn write_class(out: &mut String, class: &ClassAst, flags: Flags) -> Result<(), JavaRegexError> {
    emit_class(out, &class_inner(class, flags), class.negated, flags);
    Ok(())
}

/// Writes one class from its inner text, applying Java's ASCII case fold.
///
/// **The fold is computed by asking the engine, not by rewriting the
/// members.** Java's `CASE_INSENSITIVE` without `UNICODE_CASE` matches a
/// character `c` against a class `C` iff `c ∈ C` **or**
/// `swapAsciiCase(c) ∈ C`, whatever `C` is — a range, a property, an
/// intersection. So the class is written once, compiled, and the 52 ASCII
/// letters are tested against it; the ones whose other case is in the class
/// are added.
///
/// That is exact where a member-rewriting fold was not: `(?i)\p{Lower}`
/// matches `A`, `(?i)\p{Upper}` matches `a`, and `(?i)[Z-a]` matches `z` —
/// the last because a range spanning `[`, `\`, `]`, `^`, `_` and `` ` `` has
/// no mirrored range at all.
///
/// **The fold happens BEFORE the negation, and that is measured.**
/// `(?i)[^a-c]` does **not** match `B` in Java: the fold closes `a-c` into
/// `a-cA-C` and the negation then excludes both cases. Closing the negated
/// class instead — which an earlier cut did — matched `B` and was silently
/// wrong.
fn emit_class(out: &mut String, inner: &str, negated: bool, flags: Flags) {
    let extras = if flags.case_insensitive && !flags.unicode_case {
        ascii_case_extras(inner)
    } else {
        String::new()
    };
    match (negated, extras.is_empty()) {
        (false, true) => {
            out.push('[');
            out.push_str(inner);
            out.push(']');
        }
        // An alternation rather than `[inner extras]`, because `inner` can be
        // an intersection (`a-z&&[^c]`) and appending to one of those would
        // land inside the right-hand operand.
        (false, false) => {
            out.push_str("(?:[");
            out.push_str(inner);
            out.push_str("]|[");
            out.push_str(&extras);
            out.push_str("])");
        }
        (true, true) => {
            out.push_str("[^");
            out.push_str(inner);
            out.push(']');
        }
        (true, false) => {
            out.push_str("[^[");
            out.push_str(inner);
            out.push_str("][");
            out.push_str(&extras);
            out.push_str("]]");
        }
    }
}

/// The class's members as the crate spells them, with no brackets, no
/// negation and no case folding.
fn class_inner(class: &ClassAst, flags: Flags) -> String {
    let mut out = String::new();
    let mut first = true;
    for operand in &class.operands {
        if !first {
            out.push_str("&&");
            out.push('[');
        }
        for item in operand {
            write_class_item(&mut out, item, flags);
        }
        if !first {
            out.push(']');
        }
        first = false;
    }
    out
}

/// The class as the crate spells it, brackets and negation included. Used
/// where a class is nested inside another one.
fn class_body(class: &ClassAst, flags: Flags) -> String {
    let mut out = String::new();
    out.push('[');
    if class.negated {
        out.push('^');
    }
    out.push_str(&class_inner(class, flags));
    out.push(']');
    out
}

/// The ASCII letters a `(?i)` class must also accept.
///
/// Compiles the class as written and asks it about the other case of each of
/// the 52 letters. A class the crate cannot compile yields nothing — the
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
        ClassItem::Property(negated, body) => {
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
        ClassItem::Nested(nested) => out.push_str(&class_body(nested, flags)),
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
    /// Java answers for 171 (pattern, subject) pairs and what egret must
    /// answer, and this asserts both directions.
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
        assert_eq!(rows, 171, "the fixture lost rows");
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
}
