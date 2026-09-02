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
//! filters (`regexMatch`, `regexReplace`, `regexExtract`) to Task 6 as well —
//! its batch is "`jq`, `toIon`, `toJson`, `yaml`, **and any remaining**", and
//! after Tasks 4 and 5 those three are what remains. They are the only
//! `getFilters()` names no brief enumerates by hand, so 22 + 6 + 4 + 3 = 35
//! is what closes the count.
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
/// **Three rows cannot be right, and the reason is not "minijinja has one
/// integer type" — it has four.** `ValueRepr` carries `U64`, `I64`, `U128`
/// and `I128` without normalising, and this crate's own context deserialiser
/// routes a positive whole number through `visit_u64` while a template
/// literal is always `I64`. So the discriminant *does* separate
/// `{{ 42 | className }}` from a context `42` — but:
///
/// - the split is **positive-versus-signed, not literal-versus-context**: a
///   negative context integer is `I64` too, so keying on it would trade
///   `filter-classname-literal-int` for a new wrong row on the commoner case;
/// - minijinja exposes no public accessor for the discriminant anyway.
///
/// So this reports `Integer` when the value fits in an `i32` — right for
/// every value that came from a context, wrong for `{{ 42 | className }}`,
/// which is pinned both ways (`filter-classname-int` passes,
/// `filter-classname-literal-int` is baselined).
///
/// The same shape of limit costs two more rows: `number(type='LONG')` and
/// `number(type='FLOAT')` produce Java `Long` and `Float` boxes that egret
/// represents as a plain integer and a plain `f64`. Carrying the box type
/// would mean returning an opaque object — which is what `BigDecimal` and
/// `BigInteger` already are, and why *those* two answer correctly — and would
/// stop the result being usable as a number, which is the filter's purpose.
/// `filter-classname-of-long` and `filter-classname-of-float` are baselined
/// with that reason.
fn java_class_name(value: &MjValue) -> String {
    // Checked before `kind()`, because a `BigNumber` is `ValueKind::Plain`
    // and would otherwise land in the catch-all as `java.lang.Object`. It
    // already knows which of the two Java classes it is; the first cut of
    // this function threw that away.
    if let Some(class) = big_number_class(value) {
        return class.to_string();
    }
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

/// Java's `Number.intValue()` — a **narrowing to 32 bits**, which two of
/// these filters perform and which changes the answer rather than only the
/// range.
///
/// `ChunkFilter` does `((Number) sizeObj).intValue()` and `AbstractIndent`
/// does `((Long) args.get("amount")).intValue()`. The two Java conversions
/// behind that are not the same:
///
/// - **`long` -> `int` truncates** (two's complement, no clamp), so
///   `chunk(size=4294967296)` is `chunk(size=0)` and Guava then throws, and
///   `indent(amount=-4294967296)` is `indent(amount=0)` and returns the input
///   unchanged. Both measured.
/// - **`double` -> `int` truncates toward zero and *clamps*** at the extremes,
///   with `NaN` becoming `0`. So `chunk(size=2.5)` is `chunk(size=2)`.
///
/// Rust's `as` matches both exactly (integer `as` wraps, float `as` saturates
/// and maps `NaN` to zero), which is why this is a cast and not a range check.
///
/// **This is a bound, not only a fidelity fix.** Without the narrowing,
/// `indent(amount=2147483648)` reaches `prefix.repeat(2_147_483_648)` — a
/// 2 GiB allocation from one template literal on author-supplied source.
/// Kestra rejects it because the narrowing makes it negative.
fn java_int_value(value: &MjValue) -> Option<i32> {
    if value.kind() != ValueKind::Number {
        return None;
    }
    if value.is_integer() {
        i128::try_from(value.clone()).ok().map(|n| n as i32)
    } else {
        f64::try_from(value.clone()).ok().map(|f| f as i32)
    }
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
    // `intValue()`, so a fractional size truncates and a size past 2^31 wraps
    // -- see [`java_int_value`]. Kestra's own type check is on `Number`, so a
    // `Double` gets here and is narrowed rather than refused.
    let Some(size) = java_int_value(&size) else {
        return Err(pebble_error(format!(
            "'chunk' filter argument 'size' must be a number. Actual type was: {}",
            java_class_name(&size)
        )));
    };
    if size <= 0 {
        // Guava's `Lists.partition` -> `checkArgument(size > 0)`, an
        // `IllegalArgumentException` with a null message. Reached by
        // `chunk(size=0)` and by any size that narrows to a non-positive int.
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

    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(equality_key(&item)) {
            out.push(item);
        }
    }
    Ok(MjValue::from(out))
}

/// A canonical text key standing in for Java's `equals` on a list element.
///
/// Three properties, each of which is a real `equals` rule rather than a
/// convenience:
///
/// - **kind-tagged**, because `Integer(1).equals("1")` is `false` and a
///   text-only key would collapse two elements a real `distinct()` keeps;
/// - **maps are order-independent**, because `AbstractMap.equals` compares
///   entry sets: `[{"a":1,"b":2},{"b":2,"a":1}]` is ONE element in Kestra
///   (`filter-distinct-map-key-order`). The first cut keyed on
///   `kestra_to_string`, which is insertion-ordered, and kept both;
/// - **lists stay order-dependent**, because `AbstractList.equals` is.
///
/// A null element keys as `null` and dedups against another null, which is
/// `filter-distinct-nulls` (`[null,1,null]` -> `[null,1]`, the null surviving
/// in the output because `NON_NULL` does not apply to array elements).
fn equality_key(value: &MjValue) -> String {
    match value.kind() {
        ValueKind::None | ValueKind::Undefined => "null".to_string(),
        ValueKind::Bool => format!("b:{}", kestra_to_string(value)),
        ValueKind::Number => format!("n:{}", kestra_to_string(value)),
        ValueKind::String => format!("s:{}", kestra_to_string(value)),
        ValueKind::Seq | ValueKind::Iterable => {
            let items = as_list(value).unwrap_or_default();
            let parts: Vec<String> = items.iter().map(equality_key).collect();
            format!("l:[{}]", parts.join(","))
        }
        ValueKind::Map => {
            let entries = as_map(value).unwrap_or_default();
            let mut parts: Vec<String> = entries
                .iter()
                .map(|(key, item)| format!("{key}={}", equality_key(item)))
                .collect();
            // `AbstractMap.equals` does not care about order; sorting here is
            // what makes the key agree with it.
            parts.sort();
            format!("m:{{{}}}", parts.join(","))
        }
        _ => format!("o:{}", kestra_to_string(value)),
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
        // `MessageFormat.format("The argument ''{0}'' is required.", …)` --
        // no filter name in it. `filter` stays a parameter because the two
        // callers are separate Java classes and a future message may differ.
        _ => {
            let _ = filter;
            Err(pebble_error("The argument 'value' is required."))
        }
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
/// Three fallbacks, all different from its `After` twin's: a **null**
/// separator returns the whole input, an **empty** separator returns the
/// empty string, and a separator that is simply **not found** returns the
/// whole input again.
fn substring_before(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some((input, separator)) = substring_args(&value, rest, "substringBefore")? else {
        return Ok(MjValue::from(()));
    };
    // `if (isEmpty(str) || separator == null) return str;`
    let Some(separator) = separator.filter(|_| !input.is_empty()) else {
        return Ok(MjValue::from(input));
    };
    if separator.is_empty() {
        return Ok(MjValue::from(String::new()));
    }
    Ok(MjValue::from(match input.find(&separator) {
        Some(pos) => input[..pos].to_string(),
        None => input,
    }))
}

/// `SubstringBeforeLastFilter` — `StringUtils.substringBeforeLast`.
///
/// An **empty separator returns the whole input** here, where
/// `substringBefore` returns the empty string for the same argument, and a
/// null separator is treated identically to an empty one (`isEmpty`, not a
/// separate null test). Measured — `filter-substringbeforelast-empty-sep` and
/// `filter-substringbeforelast-null-sep` both render `abc`.
fn substring_before_last(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some((input, separator)) = substring_args(&value, rest, "substringBeforeLast")? else {
        return Ok(MjValue::from(()));
    };
    // `if (isEmpty(str) || isEmpty(separator)) return str;`
    let Some(separator) = separator.filter(|sep| !input.is_empty() && !sep.is_empty()) else {
        return Ok(MjValue::from(input));
    };
    Ok(MjValue::from(match input.rfind(&separator) {
        Some(pos) => input[..pos].to_string(),
        None => input,
    }))
}

/// `SubstringAfterFilter` — `StringUtils.substringAfter`.
///
/// The mirror image of [`substring_before`]'s fallbacks, in both directions:
/// a **null** separator is the empty string (not the input), an **empty**
/// separator is the whole input (not the empty string), and not-found is the
/// empty string. Every one of the three is the opposite of `substringBefore`.
fn substring_after(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some((input, separator)) = substring_args(&value, rest, "substringAfter")? else {
        return Ok(MjValue::from(()));
    };
    // `if (isEmpty(str)) return str; if (separator == null) return EMPTY;`
    if input.is_empty() {
        return Ok(MjValue::from(input));
    }
    let Some(separator) = separator else {
        return Ok(MjValue::from(String::new()));
    };
    Ok(MjValue::from(match input.find(&separator) {
        Some(pos) => input[pos + separator.len()..].to_string(),
        None => String::new(),
    }))
}

/// `SubstringAfterLastFilter` — `StringUtils.substringAfterLast`.
///
/// Empty for a missing separator, for a null or empty separator, **and** for a
/// separator that ends the input (`a-b- | substringAfterLast('-')` is empty,
/// not `b-`).
fn substring_after_last(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some((input, separator)) = substring_args(&value, rest, "substringAfterLast")? else {
        return Ok(MjValue::from(()));
    };
    // `if (isEmpty(str)) return str; if (isEmpty(separator)) return EMPTY;`
    if input.is_empty() {
        return Ok(MjValue::from(input));
    }
    let Some(separator) = separator.filter(|sep| !sep.is_empty()) else {
        return Ok(MjValue::from(String::new()));
    };
    Ok(MjValue::from(match input.rfind(&separator) {
        Some(pos) => input[pos + separator.len()..].to_string(),
        None => String::new(),
    }))
}

/// The shared argument handling of the four `substring*` filters.
///
/// Two nested `Option`s, and they mean different things:
///
/// - the **outer** `None` is a null *input*, which every one of them answers
///   with null;
/// - the **inner** `None` is a null *separator*, which each of them answers
///   differently — `(String) args.get("separator")` casts a Java null
///   happily, so the null reaches `StringUtils` and each method has its own
///   guard. Collapsing the two, or erroring on a null separator as the first
///   cut of this module did, loses four measured behaviours.
///
/// A separator that is present but not a string is still an error: that is
/// the cast failing, not a null.
fn substring_args(
    value: &MjValue,
    rest: Rest<MjValue>,
    filter: &str,
) -> Result<Option<(String, Option<String>)>, MjError> {
    if is_java_null(value) {
        return Ok(None);
    }
    let args = Args::new(rest);
    let Some(separator) = args.get("separator", 0) else {
        return Err(pebble_error(format!(
            "The '{filter}' filter expects an argument 'separator'."
        )));
    };
    if is_java_null(&separator) {
        return Ok(Some((kestra_to_string(value), None)));
    }
    let Some(separator) = separator.as_str() else {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(&separator)
        )));
    };
    Ok(Some((kestra_to_string(value), Some(separator.to_string()))))
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
    // NOT validated here. Kestra reaches `argsToType` only from inside
    // `escapeString`, i.e. at a leaf, so a container with no leaf never
    // consults `type` at all: `{{ [] | escapeChar }}` is `[]`, with no
    // argument and no error. Measured -- `filter-escapechar-empty-list-no-type`
    // and `filter-escapechar-empty-map-no-type`.
    escape_value(&value, &args)
}

/// The recursive half of [`escape_char`]: strings are escaped, containers are
/// walked, everything else is `toString`-ed and then escaped.
///
/// **A null inside a container is an error, not the text `null`.** Kestra's
/// `else` branch is `escapeString(value.toString(), …)`, which is a
/// `NullPointerException` on a null map value or list element -- measured,
/// `filter-escapechar-map-null-value` and `filter-escapechar-list-null-element`.
/// egret refuses rather than emitting a word into what `type='shell'` exists
/// to build. Before the fix round this path emitted the literal string `None`.
fn escape_value(value: &MjValue, args: &Args) -> Result<MjValue, MjError> {
    if let Some(entries) = as_map(value) {
        let mut out = Vec::with_capacity(entries.len());
        for (key, item) in entries {
            out.push((key, escape_value(&item, args)?));
        }
        return Ok(MjValue::from_object(OrderedMap(out)));
    }
    if let Some(items) = as_list(value) {
        let mut out = Vec::with_capacity(items.len());
        for item in &items {
            out.push(escape_value(item, args)?);
        }
        return Ok(MjValue::from(out));
    }
    if is_java_null(value) || value.is_undefined() {
        return Err(pebble_error(
            "Cannot invoke \"Object.toString()\" because the value is null",
        ));
    }
    let style = escape_style(args)?;
    Ok(MjValue::from(
        kestra_to_string(value).replace(style.0, style.1),
    ))
}

/// `EscapeCharFilter.argsToType` — resolved at the leaf, once per escaped
/// string, exactly where Kestra resolves it.
fn escape_style(args: &Args) -> Result<(&'static str, &'static str), MjError> {
    let Some(kind) = args.get("type", 0) else {
        return Err(pebble_error(
            "The 'escapeChar' filter expects an argument 'type'.",
        ));
    };
    // `Enums.getForNameIgnoreCase` upper-cases and compares against the enum
    // constant names, so `SHELL`, `shell` and `ShElL` are one value.
    match kind.as_str().map(str::to_ascii_uppercase).as_deref() {
        Some("SINGLE") => Ok(("'", "\\'")),
        Some("DOUBLE") => Ok(("\"", "\\\"")),
        Some("SHELL") => Ok(("'", "'\\''")),
        _ => Err(pebble_error(
            "The 'escapeChar' filter expects the value of 'type' to be either 'single', \
             'double', or 'shell'.",
        )),
    }
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
    // `((Long) args.get("amount")).intValue()` — a hard cast to `Long`, so a
    // fractional amount is a `ClassCastException` in Kestra rather than a
    // truncation, and only then does it narrow to `int`. See
    // [`java_int_value`] for why the narrowing is what keeps
    // `indent(amount=2147483648)` from allocating 2 GiB here.
    if !amount.is_integer() {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.Long",
            java_class_name(&amount)
        )));
    }
    let Some(amount) = java_int_value(&amount) else {
        return Err(pebble_error(format!(
            "The '{}' filter expects an integer as argument 'amount'.",
            kind.name()
        )));
    };
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
/// (`filter-indent-crlf`).
///
/// The last arm is Java's `System.lineSeparator()`, and this returns `\n`
/// unconditionally rather than consulting the host — **on Windows the two
/// would differ**. It cannot change an answer: that arm is only reached when
/// the input contains no line break at all, so the `replace` that follows
/// finds nothing either way and the input comes back unchanged. Kestra's
/// goldens are pinned to a `\n` platform for an unrelated reason
/// (`YamlFilter`'s `USE_PLATFORM_LINE_BREAKS`, see `oracle/README.md`), so
/// there is no `\r\n` oracle to disagree with.
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

/// A Java `BigInteger`/`BigDecimal`, kept as its own rendered text plus the
/// class it came from.
///
/// `number(type='BIGDECIMAL')` on `"1.50"` renders `1.50`, **with the
/// trailing zero**: `BigDecimal` carries a scale, and neither `f64` nor
/// `i128` can hold that. `BigInteger` has the matching problem in the other
/// direction — `123456789012345678901234567890` overflows every Rust integer
/// primitive.
///
/// Kestra's use of the result is `JsonWriter.write(Object)`, so keeping the
/// canonical text and rendering it is faithful for the path the filter
/// actually reaches. Carrying `class_name` alongside is what lets
/// `className` answer `java.math.BigDecimal` instead of falling into
/// [`java_class_name`]'s catch-all — measured as
/// `filter-classname-bigdecimal` / `filter-classname-biginteger`.
///
/// It is deliberately **not** arithmetic:
/// `{{ x | number(type='BIGDECIMAL') + 1 }}` is not supported here, and
/// Pebble does not meaningfully support it either.
#[derive(Debug)]
struct BigNumber {
    text: String,
    class_name: &'static str,
}

impl Object for BigNumber {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::NonEnumerable
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// The Java class of a [`BigNumber`], or `None` for anything else.
fn big_number_class(value: &MjValue) -> Option<&'static str> {
    value
        .downcast_object_ref::<BigNumber>()
        .map(|big| big.class_name)
}

/// `NumberFilter` — `NumberUtils.create*`, selected by the `type` argument.
///
/// Only a `String` input is accepted (a number input **throws**, which reads
/// backwards from the filter's name and is `filter-number-not-string`), and
/// an unrecognised `type` falls through Java's `switch` to the general
/// `createNumber` heuristic rather than erroring.
///
/// Four measured surprises, none of which the `type` names predict:
///
/// - `"0x1F" | number(type='INT')` is **31** and `"010" | number(type='INT')`
///   is **8** — `createInteger`/`createLong` are `Integer.decode`/
///   `Long.decode`, not `parseInt`/`parseLong`;
/// - `"010" | number(type='BIGINTEGER')` is **8** too, because
///   `createBigInteger` has its own copy of the same radix rules — the same
///   string under two `type`s must not give two answers;
/// - `"3000000000" | number(type='INT')` **throws**, because `INT` really is
///   32 bits;
/// - `"1.5f" | number(type='DOUBLE')` is **1.5**: `Double.valueOf` accepts a
///   trailing `f`/`F`/`d`/`D`.
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
        "INT" => create_integer(input),
        "LONG" => create_long(input),
        "FLOAT" => create_float(input),
        "DOUBLE" => create_double(input),
        "BIGDECIMAL" => create_big_decimal(input),
        "BIGINTEGER" => create_big_integer(input),
        _ => create_number(input),
    }
}

fn number_format_error(input: &str) -> MjError {
    pebble_error(format!("For input string: \"{input}\""))
}

/// The radix rules shared by `Integer.decode`, `Long.decode` and
/// commons-lang3's `createBigInteger`: an optional sign, then `0x`/`0X`/`#`
/// for hex, a leading `0` for octal, or plain decimal.
///
/// Returns `(negative, radix, digits)`. Split out because all three Java
/// entry points implement it separately and a port that gives them different
/// rules answers the same string two ways depending on `type` — which is
/// exactly the bug the fix round found in the first cut of this module.
fn java_radix(input: &str) -> (bool, u32, &str) {
    let (negative, body) = match input.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, input.strip_prefix('+').unwrap_or(input)),
    };

    if let Some(rest) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        (negative, 16, rest)
    } else if let Some(rest) = body.strip_prefix('#') {
        (negative, 16, rest)
    } else if body.len() > 1 && body.starts_with('0') {
        (negative, 8, &body[1..])
    } else {
        (negative, 10, body)
    }
}

/// `Integer.decode`/`Long.decode`, widened to `i128` so the caller can apply
/// its own range.
fn java_decode(input: &str) -> Result<i128, MjError> {
    let (negative, radix, digits) = java_radix(input);
    let magnitude = i128::from_str_radix(digits, radix).map_err(|_| number_format_error(input))?;
    Ok(if negative { -magnitude } else { magnitude })
}

/// `NumberUtils.createInteger` — `Integer.decode`, and it really is 32 bits.
fn create_integer(input: &str) -> Result<MjValue, MjError> {
    let decoded = java_decode(input)?;
    let narrowed = i32::try_from(decoded).map_err(|_| number_format_error(input))?;
    Ok(MjValue::from(i64::from(narrowed)))
}

/// `NumberUtils.createLong` — `Long.decode`, 64 bits.
fn create_long(input: &str) -> Result<MjValue, MjError> {
    let decoded = java_decode(input)?;
    let narrowed = i64::try_from(decoded).map_err(|_| number_format_error(input))?;
    Ok(MjValue::from(narrowed))
}

/// `Double.valueOf`/`Float.valueOf` accept one trailing `f`/`F`/`d`/`D` and
/// surrounding whitespace; `"1.5f" | number(type='DOUBLE')` is 1.5.
///
/// Java also accepts a hexadecimal float literal (`0x1p3`). That is not
/// ported, no corpus entry reaches it, and it is said here rather than left
/// as an unstated gap.
fn java_float_text(input: &str) -> &str {
    let trimmed = input.trim();
    match trimmed.strip_suffix(['f', 'F', 'd', 'D']) {
        // A bare suffix is not a number; leave it to fail in `parse`.
        Some(rest) if !rest.is_empty() => rest,
        _ => trimmed,
    }
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
    let narrowed: f32 = java_float_text(input)
        .parse()
        .map_err(|_| number_format_error(input))?;
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
    let parsed: f64 = java_float_text(input)
        .parse()
        .map_err(|_| number_format_error(input))?;
    Ok(MjValue::from(parsed))
}

/// `NumberUtils.createBigDecimal`, rendered as Java's `toPlainString()`.
///
/// **Not the input text.** `"1e5"` renders `100000` and `"1E-3"` renders
/// `0.001`, while `"1.50"` keeps its trailing zero — measured
/// (`filter-number-bigdecimal-exp`, `-negexp`, `filter-number-bigdecimal`).
/// The first cut of this filter kept the raw string, which passed `"1.50"`
/// only because that string is already in plain notation.
///
/// The conversion is done on the digits as text, so precision is arbitrary
/// exactly as `BigDecimal`'s is; nothing goes through `f64`.
fn create_big_decimal(input: &str) -> Result<MjValue, MjError> {
    if input.trim().is_empty() {
        return Err(pebble_error("A blank string is not a valid number"));
    }
    if input.trim_start().starts_with("--") {
        return Err(pebble_error(format!("{input} is not a valid number.")));
    }
    let text = big_decimal_plain_string(input).ok_or_else(|| {
        pebble_error(format!(
            "Character array is missing \"exponent\" symbol: {input}"
        ))
    })?;
    Ok(MjValue::from_object(BigNumber {
        text,
        class_name: "java.math.BigDecimal",
    }))
}

/// `new BigDecimal(str).toPlainString()`, on the decimal text itself.
///
/// A `BigDecimal` is an unscaled integer plus a scale; `toPlainString` lays
/// that out without an exponent. Returns `None` when `str` is not a valid
/// `BigDecimal` literal, which is a `NumberFormatException` in Java.
fn big_decimal_plain_string(input: &str) -> Option<String> {
    let (negative, body) = match input.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, input.strip_prefix('+').unwrap_or(input)),
    };

    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => {
            let raw = &body[at + 1..];
            let raw = raw.strip_prefix('+').unwrap_or(raw);
            (&body[..at], raw.parse::<i32>().ok()?)
        }
        None => (body, 0),
    };

    let (int_part, fraction) = match mantissa.find('.') {
        Some(at) => (&mantissa[..at], &mantissa[at + 1..]),
        None => (mantissa, ""),
    };
    if fraction.contains('.') {
        return None;
    }
    let digits: String = format!("{int_part}{fraction}");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    // `scale` is how many digits sit to the right of the point. `BigInteger`
    // normalises the unscaled value, so leading zeros go — but a value of
    // zero keeps its scale, which is why `0.00` stays `0.00`.
    let scale = fraction.len() as i64 - i64::from(exponent);
    let trimmed = digits.trim_start_matches('0');
    let coefficient = if trimmed.is_empty() { "0" } else { trimmed };
    let sign = if negative && coefficient != "0" {
        "-"
    } else {
        ""
    };

    if scale == 0 {
        return Some(format!("{sign}{coefficient}"));
    }
    if scale < 0 {
        if coefficient == "0" {
            return Some("0".to_string());
        }
        let zeros = "0".repeat((-scale) as usize);
        return Some(format!("{sign}{coefficient}{zeros}"));
    }

    let adjusted = coefficient.len() as i64 - scale;
    if adjusted > 0 {
        let at = adjusted as usize;
        Some(format!(
            "{sign}{}.{}",
            &coefficient[..at],
            &coefficient[at..]
        ))
    } else {
        let zeros = "0".repeat((-adjusted) as usize);
        Some(format!("{sign}0.{zeros}{coefficient}"))
    }
}

/// `NumberUtils.createBigInteger` — **with the same radix rules as
/// `createInteger`**.
///
/// `"010" | number(type='BIGINTEGER')` is `8` and `"0xFF"` is `255`. The
/// first cut of this filter refused both, which meant the same string gave
/// two different answers depending on `type` — a silently wrong number, and
/// the worst defect of this task's first round.
///
/// Decimal input keeps arbitrary precision (it is text throughout). A hex or
/// octal literal goes through `i128`, so roughly 31 hex digits; a wider one
/// errors rather than answering wrongly. No corpus entry reaches that, and it
/// is stated rather than left implicit.
fn create_big_integer(input: &str) -> Result<MjValue, MjError> {
    if input.is_empty() {
        return Err(pebble_error("An empty string is not a valid number"));
    }
    let (negative, radix, digits) = java_radix(input);
    let text = if radix == 10 {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(number_format_error(input));
        }
        let trimmed = digits.trim_start_matches('0');
        let magnitude = if trimmed.is_empty() { "0" } else { trimmed };
        if negative && magnitude != "0" {
            format!("-{magnitude}")
        } else {
            magnitude.to_string()
        }
    } else {
        let magnitude =
            i128::from_str_radix(digits, radix).map_err(|_| number_format_error(input))?;
        let signed = if negative { -magnitude } else { magnitude };
        signed.to_string()
    };
    Ok(MjValue::from_object(BigNumber {
        text,
        class_name: "java.math.BigInteger",
    }))
}

/// `NumberUtils.createNumber` — the no-`type` heuristic, in Java's own order.
///
/// 1. **A hex prefix is checked first** (`0x`, `0X`, `#`, and their negatives),
///    and the width of the significant digits picks Integer / Long /
///    BigInteger. `"0xFF" | number` is `255` and `"0xFFFFFFFFF" | number` is
///    `68719476735`. The first cut tested for a trailing `f`/`d` *before*
///    this, so every hex string ending in a hex `d` or `f` was routed to the
///    decimal branch and refused.
/// 2. Then a trailing type qualifier: `l`/`L` integral, `f`/`F` float,
///    `d`/`D` double.
/// 3. Otherwise integral text becomes the narrowest of Integer / Long /
///    BigInteger that holds it, and decimal text becomes a `Float` **if a
///    `float` and a `double` print the same**, else a `Double`. That equality
///    test is Java's own (`f.toString().equals(d.toString())`), and it is why
///    the float/double choice is invisible in egret's output: whenever Java
///    picks a `Float`, the two have the same text by construction, so
///    returning the `f64` is byte-identical.
fn create_number(input: &str) -> Result<MjValue, MjError> {
    if input.trim().is_empty() {
        return Err(pebble_error("A blank string is not a valid number"));
    }

    if let Some(prefix_len) = hex_prefix_len(input) {
        // Java skips insignificant leading zeros before measuring the width,
        // so `0x0000000F` is an Integer, not a Long.
        let mut significant = prefix_len;
        let bytes = input.as_bytes();
        let mut first = b'\0';
        for &byte in &bytes[prefix_len..] {
            first = byte;
            if byte == b'0' {
                significant += 1;
            } else {
                break;
            }
        }
        let hex_digits = input.len() - significant;
        if hex_digits > 16 || (hex_digits == 16 && first > b'7') {
            return create_big_integer(input);
        }
        if hex_digits > 8 || (hex_digits == 8 && first > b'7') {
            return create_long(input);
        }
        return create_integer(input);
    }

    match input.chars().last() {
        Some('l' | 'L') => return create_integral(&input[..input.len() - 1]),
        Some('f' | 'F') => return create_float(input),
        Some('d' | 'D') => return create_double(input),
        _ => {}
    }

    if !input.contains(['.', 'e', 'E']) {
        return create_integral(input);
    }

    let Ok(as_double) = java_float_text(input).parse::<f64>() else {
        return create_big_decimal(input);
    };
    if !as_double.is_finite() {
        return create_big_decimal(input);
    }
    Ok(MjValue::from(as_double))
}

/// `createNumber`'s integral fallback chain: Integer, then Long, then
/// BigInteger.
fn create_integral(input: &str) -> Result<MjValue, MjError> {
    if let Ok(value) = create_integer(input) {
        return Ok(value);
    }
    if let Ok(value) = create_long(input) {
        return Ok(value);
    }
    create_big_integer(input)
}

/// The length of `createNumber`'s hex prefix, or `None`.
///
/// Java's list is exactly `{"0x", "0X", "-0x", "-0X", "#", "-#"}` — note a
/// leading `+` is **not** in it, so `"+0xFF" | number` does not take the hex
/// branch even though `createInteger` would decode it.
fn hex_prefix_len(input: &str) -> Option<usize> {
    for prefix in ["0x", "0X", "-0x", "-0X", "#", "-#"] {
        if input.starts_with(prefix) {
            return Some(prefix.len());
        }
    }
    None
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
        // The list pair above is carried entirely by the FIRST assertion:
        // minijinja's own `string` also answers `[1, 2]`, so the second
        // survives deleting the override. A map separates the two spellings
        // in both directions, so this line is what makes the whole test about
        // the override rather than only about the formatter.
        assert_eq!(ok("{{ m }}", r#"{"m": {"a": 1}}"#), "{\"a\":1}");
        assert_eq!(ok("{{ m | string }}", r#"{"m": {"a": 1}}"#), "{a=1}");
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
        // `indent(2, '-')`, NOT `indent(2)`. minijinja's own `indent` answers
        // `"a\n  b"` for the one-argument form, so that assertion stayed green
        // with the override deleted and pinned nothing. Its second positional
        // argument is a bool (`indent_blank_lines`), so a string prefix there
        // is a shape only Kestra's filter accepts.
        assert_eq!(ok("{{ s | indent(2, '-') }}", r#"{"s": "a\nb"}"#), "a\n--b");
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
    /// Nothing else in this module distinguishes the three hashes: the
    /// rejection test below asserts only the message they SHARE, so it passes
    /// with `sha1` and `sha512` both wired to md5. These are the three
    /// digests of `"hello"`, and they differ in length as well as content.
    #[test]
    fn each_hash_uses_its_own_algorithm() {
        assert_eq!(
            ok("{{ s | md5 }}", r#"{"s": "hello"}"#),
            "5d41402abc4b2a76b9719d911017c592"
        );
        assert_eq!(
            ok("{{ s | sha1 }}", r#"{"s": "hello"}"#),
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
        );
        assert_eq!(
            ok("{{ s | sha512 }}", r#"{"s": "hello"}"#),
            "9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca7\
             2323c3d99ba5c11d7c7acc6e14b8c5da0c4663475c2e5c3adef46f73bcdec043"
        );
    }

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

    // -- W2 Task 4 fix round -------------------------------------------
    //
    // The five paired tests for the new `error-class` corpus entries, plus
    // the value-comparable halves of the same fixes.

    /// **C1.** A null inside a map or a list is a `NullPointerException` in
    /// Kestra, and before this round it was the literal text `None` here —
    /// Python's spelling, from a filter whose `shell` mode builds command
    /// lines. Both container shapes, because `processMap` and `processList`
    /// are separate methods with the same bug.
    #[test]
    fn escape_char_refuses_a_null_inside_a_container() {
        let map = err(
            "{{ m | escapeChar(type='single') }}",
            r#"{"m": {"a": null}}"#,
        );
        assert!(map.contains("because the value is null"), "{map}");
        assert!(!map.contains("None"), "{map}");

        let list = err(
            "{{ list | escapeChar(type='shell') }}",
            r#"{"list": ["a", null]}"#,
        );
        assert!(list.contains("because the value is null"), "{list}");
        assert!(!list.contains("None"), "{list}");
    }

    /// The root cause, one level below `escapeChar`: `kestra_to_string` must
    /// be total. Nothing reachable may come back as `None`, because every
    /// filter Tasks 5-8 add inherits that function.
    #[test]
    fn no_filter_can_render_pythons_none() {
        for template in [
            "{{ list | string }}",
            "{{ list }}",
            "{{ list | distinct }}",
            "{{ m | string }}",
        ] {
            let out = ok(template, r#"{"list": [1, null], "m": {"a": null}}"#);
            assert!(!out.contains("None"), "{template} rendered {out:?}");
        }
    }

    /// **I5.** `((Number) size).intValue()`. `4294967296` narrows to `0` and
    /// Guava then refuses; `2.5` truncates to `2` rather than being refused
    /// as non-integral.
    #[test]
    fn chunk_narrows_its_size_to_an_int() {
        assert!(err(
            "{{ list | chunk(size=4294967296) }}",
            r#"{"list": [1, 2, 3]}"#
        )
        .contains("must be a positive number"));
        assert_eq!(
            ok("{{ list | chunk(size=2.5) }}", r#"{"list": [1, 2, 3]}"#),
            "[[1,2],[3]]"
        );
    }

    /// **I5, and it is a bound as much as a fidelity fix.**
    /// `indent(amount=2147483648)` narrows to `-2147483648` and is refused;
    /// without the narrowing it reached `prefix.repeat(2_147_483_648)`, a
    /// 2 GiB allocation from one template literal. The negative case narrows
    /// to `0` and returns the input untouched.
    #[test]
    fn indent_narrows_its_amount_to_an_int() {
        assert!(
            err("{{ s | indent(amount=2147483648) }}", r#"{"s": "a\nb"}"#)
                .contains("positive integer >=0")
        );
        assert_eq!(
            ok("{{ s | indent(amount=-4294967296) }}", r#"{"s": "a\nb"}"#),
            "a\nb"
        );
        // `((Long) …)` is a hard cast, so a fractional amount is a
        // ClassCastException rather than a truncation — unlike `chunk`'s
        // `((Number) …)` above. The two filters differ and both are ported.
        assert!(err("{{ s | indent(amount=2.5) }}", r#"{"s": "a\nb"}"#)
            .contains("cannot be cast to class java.lang.Long"));
    }

    /// **I2.** `INT` is 32 bits, and saying so is the only thing that
    /// separates it from `LONG` for a value in between.
    #[test]
    fn number_int_really_is_thirty_two_bits() {
        assert!(
            err("{{ s | number(type='INT') }}", r#"{"s": "3000000000"}"#)
                .contains("For input string")
        );
        assert_eq!(
            ok("{{ s | number(type='LONG') }}", r#"{"s": "3000000000"}"#),
            "3000000000"
        );
    }

    /// **I2, the worst row: a silently wrong number.** `createBigInteger` has
    /// its own copy of the radix rules, so the same string must not give two
    /// answers depending on `type`.
    #[test]
    fn biginteger_uses_the_same_radix_rules_as_int() {
        for kind in ["INT", "LONG", "BIGINTEGER"] {
            assert_eq!(
                ok(
                    &format!("{{{{ s | number(type='{kind}') }}}}"),
                    r#"{"s": "010"}"#
                ),
                "8",
                "type={kind}"
            );
            assert_eq!(
                ok(
                    &format!("{{{{ s | number(type='{kind}') }}}}"),
                    r#"{"s": "0xFF"}"#
                ),
                "255",
                "type={kind}"
            );
        }
    }

    /// **I2.** A `BigDecimal` renders as `toPlainString()`, not as the text
    /// it was parsed from. The first cut kept the raw string, which passed
    /// `"1.50"` only because that string is already in plain notation.
    #[test]
    fn bigdecimal_renders_in_plain_notation() {
        let plain = |input: &str| {
            ok(
                "{{ s | number(type='BIGDECIMAL') }}",
                &format!(r#"{{"s": "{input}"}}"#),
            )
        };
        assert_eq!(plain("1.50"), "1.50");
        assert_eq!(plain("1e5"), "100000");
        assert_eq!(plain("1E-3"), "0.001");
        assert_eq!(plain("0.00"), "0.00");
        assert_eq!(plain("-2.5e2"), "-250");
    }

    /// **I2.** `createNumber` checks the hex prefix BEFORE the trailing type
    /// qualifier. Testing the suffix first sends every hex string ending in a
    /// hex `d` or `f` down the decimal branch, where it is refused.
    #[test]
    fn create_number_checks_hex_before_the_type_suffix() {
        assert_eq!(ok("{{ s | number }}", r#"{"s": "0xFF"}"#), "255");
        assert_eq!(ok("{{ s | number }}", r#"{"s": "0xFD"}"#), "253");
        assert_eq!(
            ok("{{ s | number }}", r#"{"s": "0xFFFFFFFFF"}"#),
            "68719476735"
        );
        // And the suffix branch itself still works, in both spellings.
        assert_eq!(ok("{{ s | number }}", r#"{"s": "1.5f"}"#), "1.5");
        assert_eq!(
            ok("{{ s | number(type='DOUBLE') }}", r#"{"s": "1.5f"}"#),
            "1.5"
        );
    }

    /// **I3.** `BigNumber` already knows which Java class it is; the first
    /// cut let both fall into the `java.lang.Object` catch-all.
    #[test]
    fn class_name_knows_the_two_big_number_classes() {
        assert_eq!(
            ok(
                "{{ s | number(type='BIGDECIMAL') | className }}",
                r#"{"s": "1.50"}"#
            ),
            "java.math.BigDecimal"
        );
        assert_eq!(
            ok(
                "{{ s | number(type='BIGINTEGER') | className }}",
                r#"{"s": "12"}"#
            ),
            "java.math.BigInteger"
        );
    }

    /// **I4.** `AbstractMap.equals` ignores key order, so two maps with the
    /// same entries are ONE element. The old key was `kestra_to_string`,
    /// which is insertion-ordered, and kept both.
    #[test]
    fn distinct_compares_maps_without_regard_to_key_order() {
        assert_eq!(
            ok(
                "{{ list | distinct }}",
                r#"{"list": [{"a": 1, "b": 2}, {"b": 2, "a": 1}]}"#
            ),
            "[{\"a\":1,\"b\":2}]"
        );
        // Lists, by contrast, ARE order-dependent, so these stay two.
        assert_eq!(
            ok("{{ list | distinct }}", r#"{"list": [[1, 2], [2, 1]]}"#),
            "[[1,2],[2,1]]"
        );
        assert_eq!(
            ok("{{ list | distinct }}", r#"{"list": [null, 1, null]}"#),
            "[null,1]"
        );
    }

    /// **I6.** A null separator is not an error and is not an empty
    /// separator: each of the four filters has its own fallback, and
    /// `substringBefore`/`substringAfter` take **opposite** ones.
    #[test]
    fn a_null_separator_takes_each_filters_own_fallback() {
        let cx = r#"{"s": "abc", "sep": null}"#;
        assert_eq!(ok("{{ s | substringBefore(separator=sep) }}", cx), "abc");
        assert_eq!(ok("{{ s | substringAfter(separator=sep) }}", cx), "");
        assert_eq!(
            ok("{{ s | substringBeforeLast(separator=sep) }}", cx),
            "abc"
        );
        assert_eq!(ok("{{ s | substringAfterLast(separator=sep) }}", cx), "");
    }

    /// Minor: Kestra reaches `argsToType` only from inside `escapeString`, so
    /// a container with no leaf never consults `type` at all.
    #[test]
    fn escape_char_does_not_consult_its_type_without_a_leaf() {
        assert_eq!(ok("{{ list | escapeChar }}", r#"{"list": []}"#), "[]");
        assert_eq!(ok("{{ m | escapeChar }}", r#"{"m": {}}"#), "{}");
        // …but one leaf is enough to make it required again.
        assert!(err("{{ list | escapeChar }}", r#"{"list": ["a"]}"#)
            .contains("expects an argument 'type'"));
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
