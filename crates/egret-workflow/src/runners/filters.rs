// kestra: core/src/main/java/io/kestra/core/runners/pebble/Extension.java
// kestra: core/src/main/java/io/kestra/core/runners/pebble/filters/*.java

//! W2 Task 4 — the string and collection filters of Kestra's Pebble
//! extension, ported onto minijinja.
//!
//! `Extension.getFilters()` has **35** `filters.put` calls but only **32**
//! string literals; the other three go through `RegexMatchFilter.NAME`,
//! `RegexReplaceFilter.NAME` and `RegexExtractFilter.NAME`. This module owns
//! 22 of the 35:
//!
//! `chunk`, `className`, `distinct`, `endsWith`, `escapeChar`, `flatten`,
//! `indent`, `keys`, `md5`, `nindent`, `number`, `sha1`, `sha512`, `slugify`,
//! `startsWith`, `string`, `substringAfter`, `substringAfterLast`,
//! `substringBefore`, `substringBeforeLast`, `urldecode`, `values`.
//!
//! The remaining 13 belong to other tasks: the six date/time filters (`date`,
//! `dateAdd`, `timestamp`, `timestampMicro`, `timestampMilli`,
//! `timestampNano`) to Task 5, the four structured-data ones (`jq`, `toIon`,
//! `toJson`, `yaml`) to Task 6, and the three constant-registered regex
//! filters (`regexMatch`, `regexReplace`, `regexExtract`) are **not named by
//! any of Tasks 4-6's briefs** — see `task-4-report.md`.
//!
//! # Two of these names were already taken by minijinja
//!
//! `indent` and `string` are minijinja built-ins. They do **not** fail as
//! "unknown filter": they resolve, run, and answer minijinja's way, so a
//! smoke test passes and only the oracle disagrees.
//!
//! | expression | minijinja's own | Kestra |
//! |---|---|---|
//! | `{{ "line1\nline2" \| indent(2) }}` | indents from the *second* line but takes its amount positionally and has no `prefix` | `line1\n  line2`, with a `prefix` argument |
//! | `{{ true \| string }}` | `True` | `true` |
//! | `{{ [1,2] \| string }}` | `[1, 2]` (agrees by luck) | `[1, 2]` |
//! | `{{ {"a":1} \| string }}` | `{"a": 1}` | `{a=1}` |
//!
//! [`register`] overrides both by name. `add_filter` replaces silently, which
//! is exactly why this has to be deliberate.
//!
//! # Null, and the one filter that disagrees with all the others
//!
//! Every filter here begins with Kestra's `if (input == null) return null`,
//! and a `null` render result is the empty string (`JsonWriter.write(Object)`
//! returns early on null). **`distinct` is the exception**: it returns the
//! four-character *string* `"null"` for a null input, so
//! `{{ nothing | distinct }}` renders `null` where every sibling renders
//! nothing. That is `DistinctFilter.java:24`, measured through the oracle
//! (`filter-distinct-null`), not a guess — and it is the kind of thing this
//! package exists to pin.
//!
//! # Stringification goes through `kestra_to_string`, never `to_string`
//!
//! Kestra reaches `Object::toString()` in six of these filters
//! (`startsWith`, `endsWith`, `indent`, `nindent`, `slugify`, `string`, and
//! every `substring*`). That is **not** minijinja's `Display`: a bool is
//! `true` not `True`, `1e100` is `1.0E100` not 101 digits, a map is `{a=1}`
//! not `{"a": 1}`. [`super::env::kestra_to_string`] is the single place a
//! value becomes Java text, and Task 3 already had to fix exactly this bug
//! once in `kestra_is_in`.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use minijinja::value::{Enumerator, Object, ObjectRepr, Rest, Value as MjValue, ValueKind};
use minijinja::{Environment, Error as MjError, ErrorKind};
use unicode_normalization::UnicodeNormalization;

use super::env::{kestra_to_string, write_java_double, OrderedMap};

/// Registers all 22 filters this task owns onto `env`.
///
/// **`indent` and `string` are overrides, not additions** — see this module's
/// doc. `Environment::add_filter` replaces a built-in silently, so the only
/// thing that makes the override visible is this comment and the oracle.
pub(super) fn register(env: &mut Environment<'static>) {
    env.add_filter("chunk", chunk);
    env.add_filter("className", class_name);
    env.add_filter("distinct", distinct);
    env.add_filter("endsWith", ends_with);
    env.add_filter("escapeChar", escape_char);
    env.add_filter("flatten", flatten);
    // Overrides minijinja's own `indent`.
    env.add_filter("indent", indent);
    env.add_filter("keys", keys);
    env.add_filter("md5", md5);
    env.add_filter("nindent", nindent);
    env.add_filter("number", number);
    env.add_filter("sha1", sha1);
    env.add_filter("sha512", sha512);
    env.add_filter("slugify", slugify);
    env.add_filter("startsWith", starts_with);
    // Overrides minijinja's own `string`.
    env.add_filter("string", string);
    env.add_filter("substringAfter", substring_after);
    env.add_filter("substringAfterLast", substring_after_last);
    env.add_filter("substringBefore", substring_before);
    env.add_filter("substringBeforeLast", substring_before_last);
    env.add_filter("urldecode", urldecode);
    env.add_filter("values", values);
}

// ---------------------------------------------------------------------------
// Argument plumbing
// ---------------------------------------------------------------------------

/// A Pebble filter's arguments: positional first, then the trailing kwargs.
///
/// Pebble builds one `Map<String, Object>` from `Filter.getArgumentNames()`
/// and lets a caller supply arguments either way — `date('yyyy-MM-dd')` and
/// `date(format='yyyy-MM-dd')` are the same call. minijinja keeps the two
/// apart, so this puts them back together: a positional argument at index *i*
/// is the *i*-th declared name.
struct Args {
    positional: Vec<MjValue>,
    named: Vec<(String, MjValue)>,
}

impl Args {
    /// Splits minijinja's trailing kwargs map off the positional arguments.
    ///
    /// minijinja marks the kwargs map with [`MjValue::is_kwargs`] and passes
    /// it as the last value, so this never has to guess whether a final map
    /// argument was a map or a set of keywords.
    fn new(rest: Rest<MjValue>) -> Self {
        let mut positional: Vec<MjValue> = rest.0;
        let mut named = Vec::new();

        if positional.last().is_some_and(MjValue::is_kwargs) {
            let kwargs = positional.pop().expect("checked non-empty above");
            if let Ok(entries) = kwargs.try_iter() {
                for key in entries {
                    if let Ok(value) = kwargs.get_item(&key) {
                        named.push((key.to_string(), value));
                    }
                }
            }
        }

        Self { positional, named }
    }

    /// The argument declared at `index` under `name`, whichever way it was
    /// passed.
    ///
    /// Mirrors Pebble's `args.containsKey(name)`: a name that was not supplied
    /// is `None`, and every Kestra filter in this module tests exactly that
    /// before reading it.
    fn get(&self, name: &str, index: usize) -> Option<MjValue> {
        self.named
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| self.positional.get(index).cloned())
    }
}

/// A `PebbleException` from a filter, as a minijinja error.
///
/// The text is Kestra's own message where Kestra has one. It can never match
/// the golden file byte for byte (a Java exception string carries the class
/// name and the template coordinates), which is why the corpus entries for
/// these paths are `compare: "error-class"` — but keeping the wording means
/// a flow author reading an egret error reads the same sentence they would
/// have read from Kestra.
fn pebble_error(message: impl Into<String>) -> MjError {
    MjError::new(ErrorKind::InvalidOperation, message.into())
}

/// Is this value Kestra's `null`?
///
/// An **undefined** is deliberately not folded in here. `UndefinedBehavior::
/// Strict` raises on a bare missing variable before a filter is reached, but
/// minijinja does not apply that to a value that arrives through a function
/// call, so returning the undefined unchanged keeps it raising at the point
/// of use rather than silently becoming Kestra's null-input answer. Same
/// reasoning as `super::env::kestra_is_in`.
fn is_java_null(value: &MjValue) -> bool {
    value.is_none()
}

/// The `java.lang`/`java.util` class name Kestra would report for a value.
///
/// Used both by the `className` filter and by the error messages that quote a
/// wrong input's type (`'chunk' filter can only be applied to List. Actual
/// type was: java.lang.String`).
///
/// **The integer row cannot be exactly right for both callers.** Kestra's
/// context arrives through `ObjectMapper.convertValue(..., Map.class)`, which
/// boxes a small whole number as an `Integer`; a whole number written as a
/// *literal in the template* is a `Long`. minijinja has one integer type, so
/// this reports `Integer` when the value fits in an `i32` — right for every
/// value that came from a context, wrong for `{{ 42 | className }}`. Pinned
/// both ways in the corpus: `filter-classname-int` passes and
/// `filter-classname-literal-int` is a permanent baselined divergence.
fn java_class_name(value: &MjValue) -> String {
    match value.kind() {
        ValueKind::Bool => "java.lang.Boolean".to_string(),
        ValueKind::String => "java.lang.String".to_string(),
        ValueKind::Number => {
            if value.is_integer() {
                match i64::try_from(value.clone()) {
                    Ok(n) if i32::try_from(n).is_ok() => "java.lang.Integer".to_string(),
                    Ok(_) => "java.lang.Long".to_string(),
                    Err(_) => "java.math.BigInteger".to_string(),
                }
            } else {
                "java.lang.Double".to_string()
            }
        }
        ValueKind::Seq | ValueKind::Iterable => "java.util.ArrayList".to_string(),
        ValueKind::Map => "java.util.LinkedHashMap".to_string(),
        ValueKind::None | ValueKind::Undefined => "null".to_string(),
        _ => "java.lang.Object".to_string(),
    }
}

/// Collects a sequence value into a `Vec`, or `None` if it is not a sequence.
fn as_list(value: &MjValue) -> Option<Vec<MjValue>> {
    match value.kind() {
        ValueKind::Seq | ValueKind::Iterable => value.try_iter().ok().map(Iterator::collect),
        _ => None,
    }
}

/// Collects a map value into its `(key, value)` pairs in iteration order.
fn as_map(value: &MjValue) -> Option<Vec<(String, MjValue)>> {
    if value.kind() != ValueKind::Map {
        return None;
    }
    let keys = value.try_iter().ok()?;
    Some(
        keys.map(|key| {
            let item = value.get_item(&key).unwrap_or_default();
            (key.to_string(), item)
        })
        .collect(),
    )
}

// ---------------------------------------------------------------------------
// Collection filters
// ---------------------------------------------------------------------------

/// `ChunkFilter` — `Lists.partition(list, size)`.
///
/// Guava's `partition` requires a **positive** size (`checkArgument(size >
/// 0)`), so `chunk(size=0)` throws rather than looping forever, and the last
/// chunk is short rather than padded (`filter-chunk-bigger`:
/// `[1,2,3] | chunk(size=10)` is `[[1,2,3]]`, one chunk, not three).
fn chunk(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let Some(size) = args.get("size", 0) else {
        return Err(pebble_error("'chunk' filter expects an argument 'size'."));
    };
    let Some(items) = as_list(&value) else {
        return Err(pebble_error(format!(
            "'chunk' filter can only be applied to List. Actual type was: {}",
            java_class_name(&value)
        )));
    };
    if size.kind() != ValueKind::Number {
        return Err(pebble_error(format!(
            "'chunk' filter argument 'size' must be a number. Actual type was: {}",
            java_class_name(&size)
        )));
    }
    let size = i64::try_from(size).unwrap_or(0);
    if size <= 0 {
        return Err(pebble_error(
            "'chunk' filter argument 'size' must be a positive number.",
        ));
    }

    let chunks: Vec<MjValue> = items
        .chunks(size as usize)
        .map(|chunk| MjValue::from(chunk.to_vec()))
        .collect();
    Ok(MjValue::from(chunks))
}

/// `ClassNameFilter` — `input.getClass().getName()`.
///
/// See [`java_class_name`] for the one row that cannot be right for both a
/// context value and a template literal.
fn class_name(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    Ok(MjValue::from(java_class_name(&value)))
}

/// `DistinctFilter` — `list.stream().distinct()`.
///
/// **Its null branch returns the string `"null"`, not null**, unlike every
/// other filter in this module (`DistinctFilter.java:24`). Kept because it is
/// what Kestra does and a flow may already depend on it; pinned by
/// `filter-distinct-null`, whose golden is `<null>`.
///
/// Java's `distinct()` is `equals`-based, so `1` and `"1"` are *different*
/// elements — which is why the key below carries the value's kind and not
/// only its text.
fn distinct(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from("null"));
    }
    let Some(items) = as_list(&value) else {
        return Err(pebble_error(format!(
            "Input must be a list, but received : {}",
            java_class_name(&value)
        )));
    };

    let mut seen: HashSet<(u8, String)> = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let key = (equality_tag(&item), kestra_to_string(&item));
        if seen.insert(key) {
            out.push(item);
        }
    }
    Ok(MjValue::from(out))
}

/// A coarse stand-in for "these two values could be `equals` in Java".
///
/// Java's `Integer(1).equals("1")` is `false`, so a text-only dedup key would
/// collapse two elements a real `distinct()` keeps. Sequences and iterables
/// share a tag because they are the same Java type (`ArrayList`) once they
/// have been through Jackson.
fn equality_tag(value: &MjValue) -> u8 {
    match value.kind() {
        ValueKind::Bool => 0,
        ValueKind::Number => 1,
        ValueKind::String => 2,
        ValueKind::Seq | ValueKind::Iterable => 3,
        ValueKind::Map => 4,
        _ => 5,
    }
}

/// `FlattenFilter` — **one** level, not recursive.
///
/// `[[1,[2]],3]` flattens to `[1,[2],3]`: the inner `[2]` survives, because
/// the `flatMap` only unwraps an element that is itself a `List`.
/// `filter-flatten-one-level` is that entry.
fn flatten(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(items) = as_list(&value) else {
        return Err(pebble_error(
            "The 'flatten' filter can only be applied to lists.",
        ));
    };

    let mut out = Vec::new();
    for item in items {
        match as_list(&item) {
            Some(inner) => out.extend(inner),
            None => out.push(item),
        }
    }
    Ok(MjValue::from(out))
}

/// `KeysFilter` — a map's key set, or a list's **indices**.
///
/// `[10,20,30] | keys` is `[0,1,2]`, not the elements: `IntStream.rangeClosed
/// (0, size - 1)`. Anything that is neither a map nor a list throws.
fn keys(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    if let Some(entries) = as_map(&value) {
        let out: Vec<MjValue> = entries
            .into_iter()
            .map(|(key, _)| MjValue::from(key))
            .collect();
        return Ok(MjValue::from(out));
    }
    if let Some(items) = as_list(&value) {
        let out: Vec<MjValue> = (0..items.len()).map(|i| MjValue::from(i as i64)).collect();
        return Ok(MjValue::from(out));
    }
    Err(pebble_error(format!(
        "'keys' filter can only be applied to List, Map, Array. Actual type was: {}",
        java_class_name(&value)
    )))
}

/// `ValuesFilter` — a map's values, in insertion order. Lists throw.
fn values(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(entries) = as_map(&value) else {
        return Err(pebble_error(format!(
            "'values' filter can only be applied to Map. Actual type was: {}",
            java_class_name(&value)
        )));
    };
    let out: Vec<MjValue> = entries.into_iter().map(|(_, item)| item).collect();
    Ok(MjValue::from(out))
}

// ---------------------------------------------------------------------------
// String filters
// ---------------------------------------------------------------------------

/// `StartsWithFilter` — `input.toString().startsWith(value.toString())`.
///
/// A null *input* is `false`, but a missing or null `value` **argument**
/// throws. The two nulls are not the same question.
fn starts_with(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let (data, needle) = prefix_args(&value, rest, "startsWith")?;
    match data {
        None => Ok(MjValue::from(false)),
        Some(data) => Ok(MjValue::from(data.starts_with(&needle))),
    }
}

/// `EndsWithFilter` — the mirror of [`starts_with`].
fn ends_with(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let (data, needle) = prefix_args(&value, rest, "endsWith")?;
    match data {
        None => Ok(MjValue::from(false)),
        Some(data) => Ok(MjValue::from(data.ends_with(&needle))),
    }
}

/// The shared argument handling of `startsWith`/`endsWith`.
///
/// Returns `(None, _)` for a null input — the caller answers `false` — and
/// errors when the `value` argument is absent or null, which is Kestra's
/// `MessageFormat.format("The argument ''{0}'' is required.", ...)`.
///
/// **The order matters**: Kestra checks the input's nullity *first*, so
/// `{{ nothing | endsWith }}` (no argument at all) is `false`, not an error.
fn prefix_args(
    value: &MjValue,
    rest: Rest<MjValue>,
    filter: &str,
) -> Result<(Option<String>, String), MjError> {
    let args = Args::new(rest);
    if is_java_null(value) {
        return Ok((None, String::new()));
    }
    match args.get("value", 0) {
        Some(needle) if !is_java_null(&needle) => {
            Ok((Some(kestra_to_string(value)), kestra_to_string(&needle)))
        }
        _ => Err(pebble_error(format!(
            "The argument 'value' is required. ({filter})"
        ))),
    }
}

/// `StringFilter` — `input.toString()`, which is **not** minijinja's
/// `Display`.
///
/// Overrides minijinja's own `string` filter. The two agree on a plain string
/// and on a list of numbers, and disagree on everything else: a bool is
/// `true`, a big float is `1.0E100`, a map is `{a=1, b=2}`. Four corpus
/// entries separate them (`filter-string-bool`, `filter-string-map`,
/// `filter-string-nested`, `filter-string-float`).
fn string(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    Ok(MjValue::from(kestra_to_string(&value)))
}

/// `SlugifyFilter` — `io.kestra.core.utils.Slugify.of`.
///
/// Six steps, in this order, and the order is load-bearing:
///
/// 1. every `[\s]` becomes `-` (Java's `\s` without `UNICODE_CHARACTER_CLASS`
///    is exactly `[ \t\n\x0B\f\r]`, **not** every Unicode space);
/// 2. NFD-normalise, which splits `é` into `e` + a combining acute;
/// 3. drop every character outside `[^\w-]` — Java's `\w` here is ASCII
///    `[A-Za-z0-9_]`, so the combining acute goes and the `e` stays. Without
///    step 2 the whole `é` would go and `Café` would slug to `caf`;
/// 4. `_` becomes `-`;
/// 5. collapse `--` repeatedly;
/// 6. strip one leading and one trailing `-`, then lowercase.
///
/// `中文 test` slugs to `test`, not `中文-test`: step 3 is ASCII-only.
fn slugify(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let input = kestra_to_string(&value);

    let no_whitespace: String = input
        .chars()
        .map(|c| {
            if matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r') {
                '-'
            } else {
                c
            }
        })
        .collect();

    let mut slug: String = no_whitespace
        .nfd()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();

    slug = slug.replace('_', "-");
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    if let Some(rest) = slug.strip_prefix('-') {
        slug = rest.to_string();
    }
    if let Some(rest) = slug.strip_suffix('-') {
        slug = rest.to_string();
    }

    Ok(MjValue::from(slug.to_ascii_lowercase()))
}

/// `UrlDecoderFilter` — `URLDecoder.decode(input, UTF_8)`.
///
/// Java's decoder is the `application/x-www-form-urlencoded` one, so `+` is a
/// **space**, not a literal plus — that is the row a `percent-encoding`-style
/// decoder gets wrong. Bytes that are not valid UTF-8 become U+FFFD, matching
/// `new String(bytes, UTF_8)`; a truncated or non-hex `%` escape throws, as
/// `IllegalArgumentException` does in Java.
///
/// Kestra casts the input to `String` before decoding
/// (`String arg = (String) input;`), so a non-string input is a
/// `ClassCastException` there and an error here.
fn urldecode(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(input) = value.as_str() else {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(&value)
        )));
    };

    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).ok_or_else(|| {
                    pebble_error("URLDecoder: Incomplete trailing escape (%) pattern".to_string())
                })?;
                let hex = std::str::from_utf8(hex).map_err(|_| {
                    pebble_error("URLDecoder: Illegal hex characters in escape (%) pattern")
                })?;
                let byte = u8::from_str_radix(hex, 16).map_err(|_| {
                    pebble_error(format!(
                        "URLDecoder: Illegal hex characters in escape (%) pattern - For input string: \"{hex}\""
                    ))
                })?;
                out.push(byte);
                i += 3;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }

    Ok(MjValue::from(String::from_utf8_lossy(&out).into_owned()))
}

/// `SubstringBeforeFilter` — `StringUtils.substringBefore`.
///
/// Not found means the **whole** input; an empty separator means the empty
/// string. Its `After` counterpart inverts both answers, which is the pair
/// `filter-substringbefore-missing` / `filter-substringafter-missing` exists
/// to keep straight.
fn substring_before(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some((input, separator)) = substring_args(&value, rest, "substringBefore")? else {
        return Ok(MjValue::from(()));
    };
    if input.is_empty() {
        return Ok(MjValue::from(input));
    }
    Ok(MjValue::from(match input.find(&separator) {
        Some(pos) => input[..pos].to_string(),
        None => input,
    }))
}

/// `SubstringBeforeLastFilter` — `StringUtils.substringBeforeLast`.
///
/// An **empty separator returns the whole input** here, where
/// `substringBefore` returns the empty string for the same argument. Measured
/// (`filter-substringbeforelast-empty-sep` → `abc`), not inferred from the
/// name.
fn substring_before_last(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some((input, separator)) = substring_args(&value, rest, "substringBeforeLast")? else {
        return Ok(MjValue::from(()));
    };
    if input.is_empty() || separator.is_empty() {
        return Ok(MjValue::from(input));
    }
    Ok(MjValue::from(match input.rfind(&separator) {
        Some(pos) => input[..pos].to_string(),
        None => input,
    }))
}

/// `SubstringAfterFilter` — `StringUtils.substringAfter`.
///
/// Not found is the **empty string**, not the input.
fn substring_after(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some((input, separator)) = substring_args(&value, rest, "substringAfter")? else {
        return Ok(MjValue::from(()));
    };
    if input.is_empty() {
        return Ok(MjValue::from(input));
    }
    Ok(MjValue::from(match input.find(&separator) {
        Some(pos) => input[pos + separator.len()..].to_string(),
        None => String::new(),
    }))
}

/// `SubstringAfterLastFilter` — `StringUtils.substringAfterLast`.
///
/// Empty for a missing separator, for an empty separator, **and** for a
/// separator that ends the input (`a-b- | substringAfterLast('-')` is empty,
/// not `b-`).
fn substring_after_last(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some((input, separator)) = substring_args(&value, rest, "substringAfterLast")? else {
        return Ok(MjValue::from(()));
    };
    if input.is_empty() {
        return Ok(MjValue::from(input));
    }
    if separator.is_empty() {
        return Ok(MjValue::from(String::new()));
    }
    Ok(MjValue::from(match input.rfind(&separator) {
        Some(pos) => input[pos + separator.len()..].to_string(),
        None => String::new(),
    }))
}

/// The shared argument handling of the four `substring*` filters.
///
/// `Ok(None)` means a null input, which every one of them answers with null.
/// The separator is cast to `String` in Kestra (`(String) args.get(...)`), so
/// a non-string separator is an error rather than a silent `toString`.
fn substring_args(
    value: &MjValue,
    rest: Rest<MjValue>,
    filter: &str,
) -> Result<Option<(String, String)>, MjError> {
    if is_java_null(value) {
        return Ok(None);
    }
    let args = Args::new(rest);
    let Some(separator) = args.get("separator", 0) else {
        return Err(pebble_error(format!(
            "The '{filter}' filter expects an argument 'separator'."
        )));
    };
    let Some(separator) = separator.as_str() else {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(&separator)
        )));
    };
    Ok(Some((kestra_to_string(value), separator.to_string())))
}

// ---------------------------------------------------------------------------
// escapeChar
// ---------------------------------------------------------------------------

/// `EscapeCharFilter` — quote-escaping, recursing into maps and lists.
///
/// Three styles, matched case-insensitively (`Enums.getForNameIgnoreCase`):
///
/// | `type` | replaces | with |
/// |---|---|---|
/// | `single` | `'` | `\'` |
/// | `double` | `"` | `\"` |
/// | `shell` | `'` | `'\''` |
///
/// **Kestra loses a map's key order here** and egret deliberately does not.
/// `EscapeCharFilter.processMap` allocates a `new HashMap<>()`, so
/// `{"b": …, "a": …}` comes back `{"a": …, "b": …}` — Java hash-bucket order,
/// which is neither the input's order nor sorted in general. Reproducing that
/// would mean porting `String.hashCode` bucket layout into egret to imitate a
/// bug; `filter-escapechar-map-two-keys` is baselined instead, with this note
/// as the reason.
fn escape_char(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let Some(kind) = args.get("type", 0) else {
        return Err(pebble_error(
            "The 'escapeChar' filter expects an argument 'type'.",
        ));
    };
    let style = match kind.as_str().map(str::to_ascii_uppercase).as_deref() {
        Some("SINGLE") => ("'", "\\'"),
        Some("DOUBLE") => ("\"", "\\\""),
        Some("SHELL") => ("'", "'\\''"),
        _ => {
            return Err(pebble_error(
                "The 'escapeChar' filter expects the value of 'type' to be either 'single', \
                 'double', or 'shell'.",
            ))
        }
    };

    Ok(escape_value(&value, style))
}

/// The recursive half of [`escape_char`]: strings are escaped, containers are
/// walked, everything else is `toString`-ed and then escaped.
fn escape_value(value: &MjValue, style: (&str, &str)) -> MjValue {
    if let Some(entries) = as_map(value) {
        return MjValue::from_object(OrderedMap(
            entries
                .into_iter()
                .map(|(key, item)| (key, escape_value(&item, style)))
                .collect(),
        ));
    }
    if let Some(items) = as_list(value) {
        return MjValue::from(
            items
                .iter()
                .map(|item| escape_value(item, style))
                .collect::<Vec<_>>(),
        );
    }
    MjValue::from(kestra_to_string(value).replace(style.0, style.1))
}

// ---------------------------------------------------------------------------
// indent / nindent
// ---------------------------------------------------------------------------

/// `IndentFilter` — `AbstractIndent.abstractApply(..., "indent")`.
///
/// **Overrides minijinja's own `indent`**, which has a different signature
/// (positional amount, no `prefix`) and would answer without ever erroring.
///
/// Adds `amount` copies of `prefix` (default a single space) after every line
/// separator, so the *first* line is left alone — it is assumed to be
/// indented already by whatever it is being interpolated into. The separator
/// is detected from the input: CRLF, LFCR, LF, CR, and the platform's own if
/// the input is single-line (so a single-line input comes back unchanged).
fn indent(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    apply_indent(value, rest, IndentKind::Indent)
}

/// `NindentFilter` — the same, plus a leading newline.
///
/// `(newLine + input).replace(newLine, newLine + prefix)` — note the
/// prepended separator goes through the same `replace`, which is why the
/// first line is indented too and why `"" | nindent(amount=2)` is `""` (the
/// empty-input branch returns before any of it).
fn nindent(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    apply_indent(value, rest, IndentKind::Nindent)
}

#[derive(Clone, Copy)]
enum IndentKind {
    Indent,
    Nindent,
}

impl IndentKind {
    fn name(self) -> &'static str {
        match self {
            IndentKind::Indent => "indent",
            IndentKind::Nindent => "nindent",
        }
    }
}

fn apply_indent(value: MjValue, rest: Rest<MjValue>, kind: IndentKind) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let input = kestra_to_string(&value);
    if input.is_empty() {
        return Ok(MjValue::from(input));
    }

    let args = Args::new(rest);
    let Some(amount) = args.get("amount", 0) else {
        return Err(pebble_error(format!(
            "The '{}' filter expects an integer as argument 'amount'.",
            kind.name()
        )));
    };
    let amount = i64::try_from(amount).map_err(|_| {
        pebble_error(format!(
            "The '{}' filter expects an integer as argument 'amount'.",
            kind.name()
        ))
    })?;
    if amount < 0 {
        return Err(pebble_error(format!(
            "The '{}' filter expects a positive integer >=0 as argument 'amount'.",
            kind.name()
        )));
    }

    let prefix = match args.get("prefix", 1) {
        Some(prefix) => match prefix.as_str() {
            Some(prefix) => prefix.to_string(),
            None => {
                return Err(pebble_error(format!(
                    "class {} cannot be cast to class java.lang.String",
                    java_class_name(&prefix)
                )))
            }
        },
        None => " ".to_string(),
    };

    let newline = line_separator(&input);
    let replacement = format!("{newline}{}", prefix.repeat(amount as usize));
    let out = match kind {
        IndentKind::Indent => input.replace(newline, &replacement),
        IndentKind::Nindent => format!("{newline}{input}").replace(newline, &replacement),
    };
    Ok(MjValue::from(out))
}

/// `AbstractIndent.getLineSeparator` — first match wins, in this order.
///
/// The CRLF test comes before the LF test, so a CRLF document is indented
/// after `\r\n` and not between the `\r` and the `\n`
/// (`filter-indent-crlf`). A single-line input falls through to the
/// platform separator, which on every OS this harness supports is `\n` — and
/// since the input then contains no `\n` at all, `replace` is a no-op and the
/// input comes back unchanged.
fn line_separator(input: &str) -> &'static str {
    if input.contains("\r\n") {
        "\r\n"
    } else if input.contains("\n\r") {
        "\n\r"
    } else if input.contains('\n') {
        "\n"
    } else if input.contains('\r') {
        "\r"
    } else {
        "\n"
    }
}

// ---------------------------------------------------------------------------
// number
// ---------------------------------------------------------------------------

/// A Java `BigInteger`/`BigDecimal`, kept as its own `toString`.
///
/// `number(type='BIGDECIMAL')` on `"1.50"` renders `1.50`, **with the
/// trailing zero**: `BigDecimal` carries a scale, and neither `f64` nor
/// `i128` can hold that. A `BigInteger` has the matching problem in the other
/// direction — `123456789012345678901234567890` overflows every Rust integer
/// primitive.
///
/// Kestra's own use of the result is `JsonWriter.write(Object)`, i.e.
/// `toString()`, so keeping the canonical text and rendering it is faithful
/// for every path the filter actually reaches. It is deliberately **not**
/// arithmetic: `{{ x | number(type='BIGDECIMAL') + 1 }}` is not supported,
/// and neither is it meaningfully supported in Pebble.
#[derive(Debug)]
struct BigNumber(String);

impl Object for BigNumber {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::NonEnumerable
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `NumberFilter` — `NumberUtils.create*`, selected by the `type` argument.
///
/// Only a `String` input is accepted (a number input **throws**, which reads
/// backwards from the filter's name and is `filter-number-not-string`), and
/// an unrecognised `type` falls through Java's `switch` to the general
/// `createNumber` heuristic rather than erroring.
///
/// Two measured surprises, both from `Integer.decode`/`Long.decode` rather
/// than `parseInt`:
///
/// - `"0x1F" | number(type='INT')` is **31**, not an error;
/// - `"010" | number(type='INT')` is **8**, not 10 — a leading zero is octal.
fn number(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(input) = value.as_str() else {
        return Err(pebble_error(format!(
            "'number' filter can only be applied to String. Actual type was: {}",
            java_class_name(&value)
        )));
    };

    let args = Args::new(rest);
    let kind = args
        .get("type", 0)
        .and_then(|t| t.as_str().map(str::to_string))
        .unwrap_or_default();

    match kind.as_str() {
        "INT" => java_decode(input).map(MjValue::from),
        "LONG" => java_decode(input).map(MjValue::from),
        "FLOAT" => create_float(input),
        "DOUBLE" => create_double(input),
        "BIGDECIMAL" => create_big_decimal(input),
        "BIGINTEGER" => create_big_integer(input),
        _ => create_number(input),
    }
}

/// `Integer.decode`/`Long.decode`: an optional sign, then `0x`/`0X`/`#` for
/// hex, a leading `0` for octal, or plain decimal.
///
/// `NumberUtils.createInteger` and `createLong` are both `decode`, not
/// `parseInt`/`parseLong` — confirmed against the oracle with
/// `filter-number-long-hex` (`"0x1F"` → `31`) rather than assumed, because
/// only one of the two commons-lang3 spellings survives that entry.
fn java_decode(input: &str) -> Result<i64, MjError> {
    let (negative, body) = match input.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, input.strip_prefix('+').unwrap_or(input)),
    };

    let (radix, digits) =
        if let Some(rest) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            (16, rest)
        } else if let Some(rest) = body.strip_prefix('#') {
            (16, rest)
        } else if body.len() > 1 && body.starts_with('0') {
            (8, &body[1..])
        } else {
            (10, body)
        };

    let magnitude = i64::from_str_radix(digits, radix)
        .map_err(|_| pebble_error(format!("For input string: \"{input}\"")))?;
    Ok(if negative { -magnitude } else { magnitude })
}

/// `NumberUtils.createFloat` — a Java `float`, whose `toString` is **not** a
/// `double`'s.
///
/// `"3.1415926535" | number(type='FLOAT')` renders `3.1415927`: the value is
/// narrowed to 32 bits first. minijinja has no `f32`, so the narrowed value
/// is re-read from its own shortest decimal form — which is exactly the text
/// Java would print, and which round-trips through `f64` unchanged because it
/// is a short decimal. `filter-number-float-precision` is the entry that
/// fails if this narrowing is skipped.
fn create_float(input: &str) -> Result<MjValue, MjError> {
    let narrowed: f32 = input
        .trim()
        .parse()
        .map_err(|_| pebble_error(format!("For input string: \"{input}\"")))?;
    if !narrowed.is_finite() {
        return Ok(MjValue::from(f64::from(narrowed)));
    }
    let text = java_float_string(narrowed);
    Ok(MjValue::from(
        text.parse::<f64>().unwrap_or(f64::from(narrowed)),
    ))
}

/// `NumberUtils.createDouble` — a plain `f64`.
fn create_double(input: &str) -> Result<MjValue, MjError> {
    let parsed: f64 = input
        .trim()
        .parse()
        .map_err(|_| pebble_error(format!("For input string: \"{input}\"")))?;
    Ok(MjValue::from(parsed))
}

/// `NumberUtils.createBigDecimal`. See [`BigNumber`] for why the text is kept
/// rather than the value.
fn create_big_decimal(input: &str) -> Result<MjValue, MjError> {
    if input.trim().is_empty() {
        return Err(pebble_error("A blank string is not a valid number"));
    }
    // Validated by parsing, then discarded: the *text* is the answer, because
    // `BigDecimal` keeps its scale and `f64` does not.
    input.parse::<f64>().map_err(|_| {
        pebble_error(format!(
            "Character array is missing \"exponent\" symbol: {input}"
        ))
    })?;
    let canonical = input.strip_prefix('+').unwrap_or(input);
    Ok(MjValue::from_object(BigNumber(canonical.to_string())))
}

/// `NumberUtils.createBigInteger`. See [`BigNumber`].
fn create_big_integer(input: &str) -> Result<MjValue, MjError> {
    if input.trim().is_empty() {
        return Err(pebble_error("A blank string is not a valid number"));
    }
    let (sign, digits) = match input.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", input.strip_prefix('+').unwrap_or(input)),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        // Hex and octal forms exist in `createBigInteger` too; they are not
        // reached by any corpus entry and are not guessed at here.
        return Err(pebble_error(format!("For input string: \"{input}\"")));
    }
    let trimmed = digits.trim_start_matches('0');
    let digits = if trimmed.is_empty() { "0" } else { trimmed };
    Ok(MjValue::from_object(BigNumber(format!("{sign}{digits}"))))
}

/// `NumberUtils.createNumber` — the no-`type` heuristic.
///
/// Integral text becomes the narrowest of `Integer`/`Long`/`BigInteger` that
/// holds it; decimal text becomes a `Float` **if a `float` and a `double`
/// print the same**, else a `Double`. That equality test is Java's own
/// (`f.toString().equals(d.toString())`), and it is why
/// `"3.14" | number` renders `3.14` while `"3.141592653589793" | number`
/// keeps all its digits.
///
/// The float/double choice is invisible in egret's output *because* of that
/// test: whenever Java picks a `Float`, the `Float` and the `Double` have the
/// same text by construction, so returning the `f64` is byte-identical.
fn create_number(input: &str) -> Result<MjValue, MjError> {
    if input.trim().is_empty() {
        return Err(pebble_error("A blank string is not a valid number"));
    }

    let looks_decimal = input.contains('.')
        || input.contains('e')
        || input.contains('E')
        || input.ends_with(['f', 'F', 'd', 'D']);

    if !looks_decimal {
        if let Ok(n) = java_decode(input) {
            return Ok(MjValue::from(n));
        }
        return create_big_integer(input);
    }

    let body = input.trim_end_matches(['f', 'F', 'd', 'D']);
    let Ok(as_double) = body.parse::<f64>() else {
        return create_big_decimal(input);
    };
    if !as_double.is_finite() {
        return create_big_decimal(input);
    }
    Ok(MjValue::from(as_double))
}

/// Java's `Float.toString`.
///
/// Same two-format rule as `Double.toString` (see
/// [`super::env::write_java_double`]) — plain decimal for `1e-3 <= |f| <
/// 1e7`, scientific otherwise — but over a `f32`'s shortest round-tripping
/// digits, which are fewer.
fn java_float_string(x: f32) -> String {
    let mut out = String::new();
    // `write_java_double` lays out whatever shortest-decimal digits it is
    // given; feeding it the f32's own digits (via the f64 that parses back
    // from them) is what makes this a *float*'s rendering rather than a
    // double's.
    let shortest = format!("{x:e}");
    let as_f64: f64 = shortest.parse().unwrap_or(f64::from(x));
    write_java_double(&mut out, as_f64);
    out
}

// ---------------------------------------------------------------------------
// Hashes
// ---------------------------------------------------------------------------

/// `Md5Filter` — `ShaBaseFilter("MD5")`.
fn md5(value: MjValue) -> Result<MjValue, MjError> {
    hash_with(&value, |bytes| {
        use md5::Digest;
        hex::encode(md5::Md5::digest(bytes))
    })
}

/// `Sha1Filter` — `ShaBaseFilter("SHA-1")`.
fn sha1(value: MjValue) -> Result<MjValue, MjError> {
    hash_with(&value, |bytes| {
        use sha1::Digest;
        hex::encode(sha1::Sha1::digest(bytes))
    })
}

/// `Sha512Filter` — `ShaBaseFilter("SHA-512")`.
///
/// There is no `sha256` filter, despite all three hash classes carrying a
/// copy-pasted "implements the 'sha256' filter" javadoc:
/// `Extension.getFilters()` registers `md5`, `sha1` and `sha512` and nothing
/// else.
fn sha512(value: MjValue) -> Result<MjValue, MjError> {
    hash_with(&value, |bytes| {
        use sha2::Digest;
        hex::encode(sha2::Sha512::digest(bytes))
    })
}

/// `ShaBaseFilter.apply` — null passes through, a **non-string throws**
/// (`"Need a string to hash\n"`, newline and all), and the digest is
/// lowercase hex of the input's UTF-8 bytes.
fn hash_with(value: &MjValue, digest: impl Fn(&[u8]) -> String) -> Result<MjValue, MjError> {
    if is_java_null(value) {
        return Ok(MjValue::from(()));
    }
    let Some(input) = value.as_str() else {
        return Err(pebble_error("Need a string to hash\n"));
    };
    Ok(MjValue::from(digest(input.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::super::env::{render_str, RenderContext};

    /// Renders `template` against a JSON object of bindings.
    fn render(template: &str, context: &str) -> Result<String, String> {
        let cx = RenderContext::from_json_str(context).expect("context must parse");
        render_str(template, &cx).map_err(|e| e.to_string())
    }

    fn ok(template: &str, context: &str) -> String {
        render(template, context).expect("must render")
    }

    fn err(template: &str, context: &str) -> String {
        render(template, context).expect_err("must fail")
    }

    // -- the four `compare: "error-class"` entries -----------------------
    //
    // The oracle compares these by error CLASS, which is satisfied by any
    // failure at all -- including "unknown filter", i.e. by the filter not
    // being ported. These four tests assert egret's own message instead, so
    // the corpus entries are not what is carrying them. See
    // `oracle/error-class.txt`.

    #[test]
    fn chunk_rejects_a_non_list() {
        let message = err("{{ s | chunk(size=2) }}", r#"{"s": "abc"}"#);
        assert!(
            message.contains("'chunk' filter can only be applied to List")
                && message.contains("java.lang.String"),
            "{message}"
        );
    }

    #[test]
    fn distinct_rejects_a_non_list() {
        let message = err("{{ s | distinct }}", r#"{"s": "abc"}"#);
        assert!(
            message.contains("Input must be a list, but received : java.lang.String"),
            "{message}"
        );
    }

    #[test]
    fn values_rejects_a_non_map() {
        let message = err("{{ list | values }}", r#"{"list": [1, 2]}"#);
        assert!(
            message.contains("'values' filter can only be applied to Map")
                && message.contains("java.util.ArrayList"),
            "{message}"
        );
    }

    #[test]
    fn number_rejects_a_non_string() {
        let message = err("{{ n | number(type='INT') }}", r#"{"n": 42}"#);
        assert!(
            message.contains("'number' filter can only be applied to String")
                && message.contains("java.lang.Integer"),
            "{message}"
        );
    }

    // -- the two names minijinja already bound ---------------------------

    /// `indent` resolves to minijinja's own filter unless it is overridden,
    /// and minijinja's takes its amount **positionally** and has no `prefix`.
    /// Deleting the `add_filter("indent", ...)` line leaves a template that
    /// still renders -- with the wrong answer -- which is why this asserts a
    /// value rather than the absence of an error.
    #[test]
    fn indent_is_kestras_and_not_minijinjas() {
        assert_eq!(
            ok("{{ s | indent(amount=2, prefix='-') }}", r#"{"s": "a\nb"}"#),
            "a\n--b"
        );
    }

    /// `string` is the sharper of the two: minijinja's own renders a bool as
    /// `True` and a map as `{"a": 1}`, and both spellings are wrong.
    #[test]
    fn string_is_javas_to_string_and_not_minijinjas() {
        assert_eq!(ok("{{ b | string }}", r#"{"b": true}"#), "true");
        assert_eq!(ok("{{ m | string }}", r#"{"m": {"a": 1}}"#), "{a=1}");
        assert_eq!(ok("{{ f | string }}", r#"{"f": 1e100}"#), "1.0E100");
    }

    /// The same list, two renderings: `JsonWriter` writes JSON with no space
    /// and `Object::toString` writes one. Both are Kestra's, from the same
    /// value, and confusing them is the bug this pins.
    #[test]
    fn a_list_renders_differently_bare_than_through_string() {
        assert_eq!(ok("{{ list }}", r#"{"list": [1, 2]}"#), "[1,2]");
        assert_eq!(ok("{{ list | string }}", r#"{"list": [1, 2]}"#), "[1, 2]");
    }

    // -- behaviour the corpus cannot reach --------------------------------

    /// Pebble maps a positional argument onto the filter's first declared
    /// name, so `substringBefore('-')` and `substringBefore(separator='-')`
    /// are the same call. Every corpus entry uses the named form, so nothing
    /// there would notice if positional support were dropped.
    #[test]
    fn arguments_can_be_positional_as_well_as_named() {
        assert_eq!(ok("{{ s | substringBefore('-') }}", r#"{"s": "a-b"}"#), "a");
        assert_eq!(ok("{{ s | startsWith('a') }}", r#"{"s": "abc"}"#), "true");
        assert_eq!(ok("{{ s | indent(2) }}", r#"{"s": "a\nb"}"#), "a\n  b");
    }

    /// `chunk(size=0)` is Guava's `checkArgument(size > 0)`. Without the
    /// guard the `chunks(0)` below panics, and a panic is a mismatch in every
    /// oracle mode -- but no corpus entry reaches it.
    #[test]
    fn chunk_refuses_a_non_positive_size() {
        assert!(err("{{ list | chunk(size=0) }}", r#"{"list": [1]}"#)
            .contains("must be a positive number"));
    }

    /// `AbstractIndent` rejects a negative amount explicitly rather than
    /// letting `repeat` decide.
    #[test]
    fn indent_refuses_a_negative_amount() {
        assert!(
            err("{{ s | indent(amount=-1) }}", r#"{"s": "a\nb"}"#).contains("positive integer >=0")
        );
    }

    /// A missing required argument is an error, but a null **input** is not:
    /// `EndsWithFilter` checks the input first, so `{{ null | endsWith }}` is
    /// `false` and never reaches the argument check.
    #[test]
    fn a_null_input_is_answered_before_a_missing_argument() {
        assert_eq!(ok("{{ n | endsWith }}", r#"{"n": null}"#), "false");
        assert!(err("{{ s | endsWith }}", r#"{"s": "abc"}"#).contains("required"));
    }

    /// `escapeChar` accepts `single`/`double`/`shell` in any case and refuses
    /// anything else, rather than silently passing the value through.
    #[test]
    fn escape_char_refuses_an_unknown_type() {
        assert_eq!(
            ok("{{ s | escapeChar(type='DoUbLe') }}", r#"{"s": "\"x\""}"#),
            "\\\"x\\\""
        );
        assert!(err("{{ s | escapeChar(type='xml') }}", r#"{"s": "x"}"#)
            .contains("'single', 'double', or 'shell'"));
    }

    /// `md5`/`sha1`/`sha512` need a `String`; a number is not one, and the
    /// message keeps Kestra's trailing newline.
    #[test]
    fn a_hash_refuses_a_non_string() {
        assert!(err("{{ n | md5 }}", r#"{"n": 42}"#).contains("Need a string to hash"));
        assert!(err("{{ n | sha1 }}", r#"{"n": 42}"#).contains("Need a string to hash"));
        assert!(err("{{ n | sha512 }}", r#"{"n": 42}"#).contains("Need a string to hash"));
    }

    /// `URLDecoder` throws on a truncated or non-hex escape rather than
    /// leaving the `%` in place.
    #[test]
    fn urldecode_refuses_a_broken_escape() {
        assert!(err("{{ s | urldecode }}", r#"{"s": "a%"}"#).contains("URLDecoder"));
        assert!(err("{{ s | urldecode }}", r#"{"s": "a%zz"}"#).contains("URLDecoder"));
    }

    /// `Integer.decode`, not `parseInt`. Both hex forms and the octal one.
    #[test]
    fn number_decodes_rather_than_parsing() {
        assert_eq!(
            ok("{{ s | number(type='INT') }}", r##"{"s": "#1F"}"##),
            "31"
        );
        assert_eq!(
            ok("{{ s | number(type='INT') }}", r#"{"s": "-0x10"}"#),
            "-16"
        );
        assert_eq!(ok("{{ s | number(type='INT') }}", r#"{"s": "0"}"#), "0");
    }

    /// Java's `distinct()` is `equals`-based, so `1` and `"1"` are different
    /// elements. A text-only dedup key collapses them and this is what dies.
    #[test]
    fn distinct_does_not_conflate_a_number_with_its_text() {
        assert_eq!(
            ok("{{ list | distinct }}", r#"{"list": [1, "1", 1]}"#),
            "[1,\"1\"]"
        );
    }

    /// `getLineSeparator` tests CRLF before LF, so a CRLF document is
    /// indented after the pair and not between the two characters.
    #[test]
    fn indent_detects_the_line_separator_in_order() {
        assert_eq!(super::line_separator("a\r\nb"), "\r\n");
        assert_eq!(super::line_separator("a\n\rb"), "\n\r");
        assert_eq!(super::line_separator("a\nb"), "\n");
        assert_eq!(super::line_separator("a\rb"), "\r");
        assert_eq!(super::line_separator("ab"), "\n");
    }

    /// NFD is what makes `Café` slug to `cafe` rather than `caf`: the
    /// combining accent has to be separated from the `e` before the
    /// ASCII-only `[^\w-]` pass can drop it without taking the letter too.
    #[test]
    fn slugify_decomposes_before_it_strips() {
        assert_eq!(ok("{{ s | slugify }}", r#"{"s": "Café"}"#), "cafe");
        assert_eq!(
            ok("{{ s | slugify }}", r#"{"s": "Über Straße"}"#),
            "uber-strae"
        );
    }
}
