// kestra: core/src/main/java/io/kestra/core/runners/pebble/Extension.java
// kestra: core/src/main/java/io/kestra/core/runners/pebble/filters/*.java

//! W2 Task 4 — the string and collection filters of Kestra's Pebble
//! extension, ported onto minijinja.
//!
//! `Extension.getFilters()` has **35** `filters.put` calls but only **32**
//! string literals; the other three go through `RegexMatchFilter.NAME`,
//! `RegexReplaceFilter.NAME` and `RegexExtractFilter.NAME`. Confirmed against
//! `host.Oracle dump`, which lists what the built engine actually carries:
//! 35 filters, 34 functions, 1 test. **This module owns all 35 of them.**
//!
//! `ReplaceFilter` is in the jar and is **not** registered — no `put` names
//! it — so `{{ x | replace(…) }}` is Pebble's own built-in in Kestra and
//! minijinja's own here, and neither is Kestra's class. Reconciling the
//! built-in surface is Task 8's.
//!
//! W2 Task 4 ported 22: `chunk`, `className`, `distinct`, `endsWith`,
//! `escapeChar`, `flatten`, `indent`, `keys`, `md5`, `nindent`, `number`,
//! `sha1`, `sha512`, `slugify`, `startsWith`, `string`, `substringAfter`,
//! `substringAfterLast`, `substringBefore`, `substringBeforeLast`,
//! `urldecode`, `values`.
//!
//! W2 Task 5 ported the six date/time ones: `date`, `dateAdd`, `timestamp`,
//! `timestampMicro`, `timestampMilli`, `timestampNano`. All six are
//! `AbstractDate` subclasses and all six go through [`super::javafmt`], which
//! is where Java's `DateTimeFormatter` pattern language lives.
//!
//! W2 Task 6 ported the last 7: the four structured-data filters (`jq`,
//! `toIon`, `toJson`, `yaml`) and the three constant-registered regex ones
//! (`regexMatch`, `regexReplace`, `regexExtract`). 22 + 6 + 4 + 3 = 35 is
//! what closes the count.
//!
//! Three of Task 6's seven do not fit in this file and have their own
//! modules, for the same reason [`super::javafmt`] has one: they are ports of
//! a foreign format, not of a Pebble filter. [`super::ionfmt`] is Ion text,
//! [`super::yamlfmt`] is Jackson-YAML over SnakeYAML's emitter, and
//! [`super::javare`] is `java.util.regex` translated onto the `regex` crate.
//! Read `javare`'s module doc before touching any pattern: nine of Java's
//! commonest constructs answer a DIFFERENT question in the crate, silently.
//!
//! # The null-input branch is not the same in all seven
//!
//! `toJson`, `toIon` and `yaml` return the four-character *string* `"null"`
//! (the same branch `distinct` has); `regexMatch` returns `false`; `jq`,
//! `regexReplace` and `regexExtract` return Java `null`, which renders as
//! nothing. `the_null_input_branches_are_not_all_the_same` is the test.
//!
//! # And a null MAP VALUE splits them three ways
//!
//! `JacksonMapper.configure()` sets `NON_NULL`; `ofIon()` overrides it back
//! to `ALWAYS`; and `YamlFilter` builds its own mapper that never sees
//! `configure()` at all. So `{"a":null,"b":1}` is `{"b":1}` through `toJson`,
//! `{a:null,b:1}` through `toIon` and `a: null\nb: 1` through `yaml` — and
//! `jq` gets the `NON_NULL` treatment on the way **in**, so it never sees the
//! key at all. Four filters, one batch, four answers.
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

use super::env::{kestra_to_string, write_java_double, write_json, OrderedMap, MAX_OUTPUT_BYTES};
use super::ionfmt::write_ion;
use super::javafmt::{self, ChronoUnit, Format, Locale as JavaLocale, Zdt, Zone};
use super::javare;
use super::jqsandbox;
use super::yamlfmt::to_yaml;

/// Registers all 35 of Kestra's filters onto `env`.
///
/// **`indent` and `string` are overrides, not additions** — see this module's
/// doc. `Environment::add_filter` replaces a built-in silently, so the only
/// thing that makes the override visible is this comment and the oracle.
///
/// **None of Task 5's six collides with a minijinja built-in.** Checked
/// against `defaults::build_builtin_filters` in the minijinja 2.24.0 the lock
/// file resolves, at this workspace's feature set (`json` and `urlencode`
/// off): 47 names, of which none is `date`, `dateAdd`, `timestamp`,
/// `timestampMicro`, `timestampMilli` or `timestampNano`. Task 4 found
/// `string` colliding invisibly, so this is verified rather than assumed.
pub(super) fn register(env: &mut Environment<'static>) {
    env.add_filter("chunk", chunk);
    env.add_filter("className", class_name);
    env.add_filter("date", date);
    env.add_filter("dateAdd", date_add);
    env.add_filter("distinct", distinct);
    env.add_filter("endsWith", ends_with);
    env.add_filter("escapeChar", escape_char);
    env.add_filter("flatten", flatten);
    // Overrides minijinja's own `indent`.
    env.add_filter("indent", indent);
    env.add_filter("jq", jq);
    env.add_filter("keys", keys);
    env.add_filter("md5", md5);
    env.add_filter("nindent", nindent);
    env.add_filter("regexExtract", regex_extract);
    env.add_filter("regexMatch", regex_match);
    env.add_filter("regexReplace", regex_replace);
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
    env.add_filter("timestamp", timestamp);
    env.add_filter("timestampMicro", timestamp_micro);
    env.add_filter("timestampMilli", timestamp_milli);
    env.add_filter("timestampNano", timestamp_nano);
    env.add_filter("toIon", to_ion);
    env.add_filter("toJson", to_json);
    env.add_filter("urldecode", urldecode);
    env.add_filter("values", values);
    env.add_filter("yaml", yaml);
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

/// Is this value Kestra's `null` at a filter's `input == null` branch?
///
/// **An undefined counts, and that is measured rather than convenient.**
/// Pebble's `null` is a real literal; minijinja has no such name, so
/// `{{ null | distinct }}` reaches this crate as `ValueKind::Undefined`. Six
/// corpus entries pin what Kestra answers for the literal
/// (`filter-string-literal-null` and its siblings), and one of them —
/// `filter-distinct-literal-null`, whose golden is `<null>` because `distinct`
/// alone returns the *string* `"null"` — cannot be produced by any route that
/// treats an undefined as "not null".
///
/// # What this costs, stated rather than hidden
///
/// egret cannot tell Pebble's `null` literal from a **typo'd variable** at a
/// filter's input, and Kestra can: `{{ typo | string }}` is a
/// `RootAttributeNotFoundException` there and renders empty here.
/// `filter-string-undefined-input` is that entry, baselined.
///
/// **This is not a cost of the choice above.** Measured on minijinja 2.24:
/// `UndefinedBehavior::Strict` raises for `{{ typo }}` but **not** for
/// `{{ typo | string }}` — an undefined piped into a filter is simply passed
/// in. So the typo renders empty either way, and the only thing the two routes
/// differ on is whether the *literal* is answered correctly. The real fix is
/// one layer up, in [`super::rewrite`]: translating Pebble's `null` literal to
/// minijinja's `none` would make the literal a `None` and leave the typo an
/// `Undefined`, and the filters could then keep them apart. That is a change
/// to the rewriter's vocabulary, which is not this module's to make.
///
/// `super::env::kestra_is_in` is deliberately different: it is a *function*,
/// its operands are not filter inputs, and returning the undefined unchanged
/// is what keeps a typo inside an `isIn` from silently deciding a branch.
fn is_java_null(value: &MjValue) -> bool {
    value.is_none() || value.is_undefined()
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
/// Three properties, each a real `equals` rule rather than a convenience:
///
/// - **kind-tagged**, because `Integer(1).equals("1")` is `false` and a
///   text-only key would collapse two elements a real `distinct()` keeps;
/// - **maps are order-independent**, because `AbstractMap.equals` compares
///   entry sets: `[{"a":1,"b":2},{"b":2,"a":1}]` is ONE element in Kestra
///   (`filter-distinct-map-key-order`);
/// - **lists stay order-dependent**, because `AbstractList.equals` is.
///
/// A null element keys as `null` and dedups against another null, which is
/// `filter-distinct-nulls` (`[null,1,null]` -> `[null,1]`, the null surviving
/// in the output because `NON_NULL` does not apply to array elements).
///
/// # Every component is length-prefixed, and that is not tidiness
///
/// A first cut joined the pieces with bare `,`, `=` and `:` separators, so a
/// *value containing those characters* could forge another element's key and
/// `distinct` would **silently drop a row** — data loss, not a rendering
/// difference. Measured: `[["a,s:b"], ["a","b"]]` collapsed to one element and
/// `[{"a":"b,c=s:d"}, {"a":"b","c":"d"}]` did too, where Kestra keeps both.
/// `filter-distinct-key-ambiguity-list` and `-map` are those two entries.
///
/// Length-prefixing (`<byte length>:<bytes>`) makes the encoding injective, so
/// no content can be mistaken for structure. Sorting the map's parts happens
/// *after* each part is already unambiguous, which is what keeps the
/// order-independence from re-introducing the same hazard.
fn equality_key(value: &MjValue) -> String {
    let mut out = String::new();
    write_equality_key(&mut out, value);
    out
}

/// Appends `value`'s injective key to `out`.
///
/// `tag` distinguishes the Java types; every variable-length piece is written
/// as `<len>:<bytes>` so concatenation cannot be ambiguous.
fn write_equality_key(out: &mut String, value: &MjValue) {
    fn piece(out: &mut String, text: &str) {
        out.push_str(&text.len().to_string());
        out.push(':');
        out.push_str(text);
    }

    match value.kind() {
        ValueKind::None | ValueKind::Undefined => out.push_str("N;"),
        ValueKind::Bool => {
            out.push_str("B;");
            piece(out, &kestra_to_string(value));
        }
        ValueKind::Number => {
            out.push_str("D;");
            piece(out, &kestra_to_string(value));
        }
        ValueKind::String => {
            out.push_str("S;");
            piece(out, value.as_str().unwrap_or_default());
        }
        ValueKind::Seq | ValueKind::Iterable => {
            let items = as_list(value).unwrap_or_default();
            out.push_str("L;");
            out.push_str(&items.len().to_string());
            out.push(';');
            for item in &items {
                write_equality_key(out, item);
            }
        }
        ValueKind::Map => {
            let entries = as_map(value).unwrap_or_default();
            // Each entry is keyed WHOLE and then sorted, so ordering happens
            // between already-unambiguous strings.
            let mut parts: Vec<String> = entries
                .iter()
                .map(|(key, item)| {
                    let mut part = String::new();
                    piece(&mut part, key);
                    write_equality_key(&mut part, item);
                    part
                })
                .collect();
            // `AbstractMap.equals` does not care about order.
            parts.sort();
            out.push_str("M;");
            out.push_str(&parts.len().to_string());
            out.push(';');
            for part in &parts {
                piece(out, part);
            }
        }
        _ => {
            out.push_str("O;");
            piece(out, &kestra_to_string(value));
        }
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
    // Reached only from inside a container: `escape_char` answers a null
    // *input* with null before calling this. Kestra's `else` branch is
    // `escapeString(value.toString(), …)`, an NPE on a null map value or list
    // element.
    if is_java_null(value) {
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

/// `AbstractIndent.MAX_AMOUNT` — **Kestra's own bound**, 10000.
///
/// Read off the artifact, because it is not in the checkout:
/// `javap -p -c` on `AbstractIndent.class` in `io.kestra:core:2.0.0-rc13`
/// shows `private static final int MAX_AMOUNT` with `ConstantValue: 10000`
/// and an `ldc2_w // long 10000l` feeding
/// `RenderLimits.ensureAtMost(JJLjava/lang/String;)V`, between the
/// `amount >= 0` check and `prefix(args)`. Neither `RenderLimits` nor
/// `MAX_AMOUNT` exists anywhere in the pinned `kestra` checkout at
/// `96893b589`.
///
/// This is the same artifact-versus-checkout gap `oracle/README.md` records
/// for `BoundedRangeFunction`, and the same rule applies: **the artifact is
/// authoritative for behaviour, the checkout only for reading an
/// implementation.** `RenderLimits` has exactly two call sites in the whole
/// jar — this one and `BoundedRangeFunction` — confirmed by
/// `grep -rla RenderLimits` over every extracted class. Nothing else in
/// Kestra's expression language is bounded, which is why
/// [`big_decimal_plain_string`]'s own limit is a divergence and this one is
/// not.
const MAX_INDENT_AMOUNT: i32 = 10000;

/// The shared body of `indent` and `nindent`.
///
/// # Two limits, and only one of them is egret's
///
/// **[`MAX_INDENT_AMOUNT`] is Kestra's**: `amount` above 10000 throws
/// `RenderLimitExceededException`, with the message reproduced here word for
/// word. `filter-indent-amount-at-max` and `filter-indent-amount-over-max`
/// pin both sides of it.
///
/// **The [`MAX_OUTPUT_BYTES`] projection is egret's**, and it is a deliberate
/// divergence — but a much narrower one than an earlier version of this
/// comment claimed. Kestra bounds `amount`, **not** the product, so
/// `indent(amount=10000, prefix=<100 chars>)` really does build a megabyte
/// there. egret refuses when
/// `separators × amount × prefix.len()` (plus the input, plus `nindent`'s
/// prepended separator) would pass the render cap:
///
/// - fidelity does not extend to reproducing a **resource-exhaustion
///   vector**. A workflow's source is author-supplied and W3 renders it in a
///   request path;
/// - [`super::rewrite`]'s `MAX_NESTING_DEPTH` is the precedent — also a bound
///   Kestra lacks, added because unbounded recursion aborts the process;
/// - an allocation past [`MAX_OUTPUT_BYTES`] defeats a cap that only measures
///   what *survives* the allocation.
///
/// **The bound is derived, not invented**: no second constant, the size is
/// computed exactly, and nothing is allocated on the refusing path.
///
/// The `prefix`-width case has no corpus entry, because the golden would be a
/// multi-megabyte string in `golden.json`;
/// `indent_refuses_to_allocate_past_the_output_cap` is its pin, and
/// `oracle/README.md` says so. `MAX_INDENT_AMOUNT` needs no such excuse — its
/// goldens are 10 KB and are committed.
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

    // `RenderLimits.ensureAtMost(amount, 10000, …)` — Kestra's OWN bound,
    // after the `>= 0` check and before `prefix(args)` and `String.repeat`.
    // See this function's doc for why it is read off the artifact and not the
    // checkout.
    if amount > MAX_INDENT_AMOUNT {
        return Err(pebble_error(format!(
            "The '{}' filter's 'amount' argument cannot exceed {MAX_INDENT_AMOUNT}, but {amount} \
             was given.",
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

    // **A DELIBERATE DIVERGENCE: bounded before it allocates.** See this
    // function's doc. The size of the result is exactly computable here, so
    // the refusal happens instead of the allocation, never after it.
    let separators = match kind {
        IndentKind::Indent => input.matches(newline).count(),
        // `(newLine + input).replace(...)` — the prepended separator is
        // rewritten too, so there is one more.
        IndentKind::Nindent => input.matches(newline).count() + 1,
    };
    let padding = separators
        .saturating_mul(amount as usize)
        .saturating_mul(prefix.len());
    // `nindent` prepends one separator to the input; `indent` does not. The
    // projection is the exact output length, not an upper bound.
    let prepended = match kind {
        IndentKind::Indent => 0,
        IndentKind::Nindent => newline.len(),
    };
    let projected = input
        .len()
        .saturating_add(prepended)
        .saturating_add(padding);
    if projected > MAX_OUTPUT_BYTES {
        return Err(pebble_error(format!(
            "the '{}' filter would produce {projected} bytes, over the {MAX_OUTPUT_BYTES}-byte \
             render limit",
            kind.name()
        )));
    }

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
    /// `toPlainString()`: what a **bare** `{{ x }}` renders, measured.
    text: String,
    /// `toString()`: what a **serialiser** writes. The two differ whenever the
    /// scale is negative or the adjusted exponent is below -6, so
    /// `'1E3' | number(type='BIGDECIMAL')` is `1000` bare and `1E+3` through
    /// `toJson`. One value, two spellings, and an earlier cut carried only the
    /// first — which is why every serialiser quoted it as a STRING.
    canonical: String,
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

/// `BigDecimal.toString()` / `BigInteger.toString()` for a [`BigNumber`], or
/// `None` for anything else.
///
/// **This is what a serialiser must write, and it is a NUMBER, not a string.**
/// `super::env::write_json`, `super::ionfmt` and `super::yamlfmt` all end in a
/// catch-all that treats an unrecognised value as text, and a `BigNumber` is
/// `ValueKind::Plain`, so all three quoted it:
/// `'1.50' | number(type='BIGDECIMAL') | toJson` was `"1.50"` here and is
/// `1.50` in Kestra. `{{ [x] }}` was wrong for the same reason and is not a
/// filter's doing at all.
pub(super) fn big_number_text(value: &MjValue) -> Option<String> {
    value
        .downcast_object_ref::<BigNumber>()
        .map(|big| big.canonical.clone())
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
    // `(String) args.getOrDefault("type", "")` then `switch (type)`. The cast
    // rejects a non-string, and the `switch` dereferences the result — so a
    // null `type` is an NPE (`String.hashCode()` on null) and a numeric one a
    // `ClassCastException`, neither of which quietly means "no type".
    // `filter-number-type-not-string` and `filter-number-type-null`.
    let kind = match args.get("type", 0) {
        None => String::new(),
        Some(kind) if is_java_null(&kind) => {
            return Err(pebble_error(
                "Cannot invoke \"String.hashCode()\" because the 'type' argument is null",
            ))
        }
        Some(kind) => match kind.as_str() {
            Some(text) => text.to_string(),
            None => {
                return Err(pebble_error(format!(
                    "class {} cannot be cast to class java.lang.String",
                    java_class_name(&kind)
                )))
            }
        },
    };

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
    let (text, canonical) = big_decimal_strings(input).ok_or_else(|| {
        pebble_error(format!(
            "Character array is missing \"exponent\" symbol: {input}"
        ))
    })?;
    Ok(MjValue::from_object(BigNumber {
        text,
        canonical,
        class_name: "java.math.BigDecimal",
    }))
}

/// `new BigDecimal(str)`, as the two strings Java can print it with:
/// `toPlainString()` and `toString()`.
///
/// A `BigDecimal` is an unscaled integer plus a scale. `toPlainString` always
/// lays that out without an exponent; `toString` uses scientific notation when
/// the scale is **negative** or the adjusted exponent is below **-6**, which
/// is the javadoc's own rule. So `1E3` is `1000` one way and `1E+3` the
/// other, and both are reachable from a template: the first from `{{ x }}`,
/// the second from `{{ x | toJson }}`.
///
/// Returns `None` when `str` is not a valid `BigDecimal` literal, which is a
/// `NumberFormatException` in Java.
fn big_decimal_strings(input: &str) -> Option<(String, String)> {
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

    // **A DELIBERATE DIVERGENCE, the same one `apply_indent` makes and for the
    // same reason.** `toPlainString` writes `|scale|` zeros, and `exponent` is
    // an `i32`, so the 12-byte source `"1E-10000000"` expands to ten million
    // characters -- and `"1E-2000000000"` to two billion. Kestra really does
    // build it: `RenderLimits` has exactly two call sites in the whole
    // artifact (`AbstractIndent` and `BoundedRangeFunction`, confirmed by
    // `grep -rla RenderLimits` over every extracted class), and `NumberFilter`
    // is not one of them. So this is a bound Kestra lacks, refusing before the
    // allocation rather than after it -- a cap that only measures what
    // survives the allocation is not a cap.
    //
    // The projection is exact and derived from the same [`MAX_OUTPUT_BYTES`]
    // as `apply_indent`'s: no second constant, nothing allocated on the
    // refusing path. `filter-number-bigdecimal-wide-exp` (a 402-character
    // result) is the largest case with a committed golden; the multi-megabyte
    // ones cannot have one, for the reason `oracle/README.md` records.
    let projected = digits
        .len()
        .saturating_add(scale.unsigned_abs() as usize)
        .saturating_add(3);
    if projected > MAX_OUTPUT_BYTES {
        return None;
    }

    let trimmed = digits.trim_start_matches('0');
    let coefficient = if trimmed.is_empty() { "0" } else { trimmed };
    let sign = if negative && coefficient != "0" {
        "-"
    } else {
        ""
    };

    let plain = if scale == 0 {
        format!("{sign}{coefficient}")
    } else if scale < 0 {
        if coefficient == "0" {
            "0".to_string()
        } else {
            let zeros = "0".repeat((-scale) as usize);
            format!("{sign}{coefficient}{zeros}")
        }
    } else {
        let adjusted = coefficient.len() as i64 - scale;
        if adjusted > 0 {
            let at = adjusted as usize;
            format!("{sign}{}.{}", &coefficient[..at], &coefficient[at..])
        } else {
            let zeros = "0".repeat((-adjusted) as usize);
            format!("{sign}0.{zeros}{coefficient}")
        }
    };

    // `toString`'s rule, from `BigDecimal`'s javadoc: plain when the scale is
    // non-negative AND the adjusted exponent is at least -6; scientific
    // otherwise, with the point after the first digit and a SIGNED exponent.
    let adjusted_exponent = coefficient.len() as i64 - 1 - scale;
    let canonical = if scale >= 0 && adjusted_exponent >= -6 {
        plain.clone()
    } else if coefficient.len() == 1 {
        format!("{sign}{coefficient}E{adjusted_exponent:+}")
    } else {
        format!(
            "{sign}{}.{}E{adjusted_exponent:+}",
            &coefficient[..1],
            &coefficient[1..]
        )
    };
    Some((plain, canonical))
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
        canonical: text.clone(),
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
    // Java's own guards, both of which fall through to `createBigDecimal`:
    // an infinite double, and a double that came out as **zero from a
    // non-zero mantissa** — i.e. underflow. `"1E-400" | number` is not `0.0`,
    // it is a 402-character `BigDecimal`, because the value did not survive
    // the double. `filter-number-default-underflow` pins it, and
    // `filter-number-default-zero` ("0.0" -> "0.0") pins the other side, where
    // the mantissa really is zero and the double is kept.
    let all_zeros = input.chars().all(|c| !c.is_ascii_digit() || c == '0');
    if !as_double.is_finite() || (as_double == 0.0 && !all_zeros) {
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
/// **commons-lang3 3.20.0**, which is what the harness's classpath carries —
/// not the pre-3.12 form. 3.12 replaced the literal prefix list
/// `{"0x","0X","-0x","-0X","#","-#"}` with an `offset` computed from a leading
/// `+` **or** `-`, then `startsWith` over `{"0x","0X","#"}` at that offset. So
/// a leading `+` takes the hex branch too.
///
/// Not cosmetic: `java_radix` already strips `+`, so with the old list
/// `"+0xFF" | number` errored while `"+0xFF" | number(type='INT')` answered
/// `255` — the same string, two answers depending on `type`, which is exactly
/// the class of bug the shared [`java_radix`] was introduced to kill.
/// `filter-number-plus-hex-default` and `-int` pin both halves.
fn hex_prefix_len(input: &str) -> Option<usize> {
    let offset = usize::from(input.starts_with(['+', '-']));
    let body = input.get(offset..)?;
    for prefix in ["0x", "0X", "#"] {
        if body.starts_with(prefix) {
            return Some(offset + prefix.len());
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

// ---------------------------------------------------------------------------
// Date and time (W2 Task 5)
// ---------------------------------------------------------------------------

/// The four arguments every date filter shares — `AbstractDate.getArgumentNames()`
/// is `[format, timeZone, existingFormat, locale]`, and `DateAddFilter`
/// prepends `[amount, unit]` to it.
///
/// `base` is where that shared block starts in the filter's own argument list:
/// 0 for `date`, 2 for `dateAdd`. The `timestamp*` filters declare only
/// `[timeZone, existingFormat]` and build this by hand.
struct DateArgs {
    format: String,
    zone: Zone,
    existing_format: Option<String>,
    locale: JavaLocale,
}

impl DateArgs {
    fn read(args: &Args, base: usize) -> Result<Self, MjError> {
        // `args.containsKey("format") ? (String) args.get("format") : "iso"`.
        // A *present but null* `format` is not the default: it reaches
        // `DateTimeFormatter.ofPattern(null)`, which is an NPE whose message
        // is the single word `pattern`.
        let format = match args.get("format", base) {
            None => "iso".to_string(),
            Some(value) if is_java_null(&value) => return Err(pebble_error("pattern")),
            Some(value) => cast_to_string(&value)?,
        };
        let zone = Zone::resolve(optional_string(args, "timeZone", base + 1)?.as_deref())
            .map_err(pebble_error)?;
        let existing_format = optional_string(args, "existingFormat", base + 2)?;
        let locale = match optional_string(args, "locale", base + 3)? {
            Some(spec) => JavaLocale::resolve(&spec).map_err(pebble_error)?,
            None => JavaLocale,
        };
        Ok(DateArgs {
            format,
            zone,
            existing_format,
            locale,
        })
    }

    /// `formatter(format).withLocale(locale).withZone(zone).format(zdt)`.
    fn render(&self, zdt: &Zdt) -> Result<String, MjError> {
        Format::resolve(&self.format)
            .and_then(|format| format.format(zdt, &self.zone, self.locale))
            .map_err(pebble_error)
    }
}

/// `(String) args.get(name)` — absent and explicitly-null are the same thing
/// here (Kestra uses a plain `get`, not `containsKey`), and a non-string is a
/// `ClassCastException`.
fn optional_string(args: &Args, name: &str, index: usize) -> Result<Option<String>, MjError> {
    match args.get(name, index) {
        None => Ok(None),
        Some(value) if is_java_null(&value) => Ok(None),
        Some(value) => Ok(Some(cast_to_string(&value)?)),
    }
}

/// Java's `(String) x` on an argument that is not one.
fn cast_to_string(value: &MjValue) -> Result<String, MjError> {
    value.as_str().map(str::to_string).ok_or_else(|| {
        pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(value)
        ))
    })
}

/// `AbstractDate.convert(input, zone, existingFormat)`.
///
/// Kestra's branches are `Date`, `Instant`, `LocalDateTime`, `LocalDate`,
/// `ZonedDateTime`, `Long`, and then a cast to `String`. Only the last two can
/// arrive from a Pebble context, since Jackson deserialises JSON into boxed
/// primitives, strings, lists and maps.
///
/// # A whole number is Kestra's `Long` here, and that is a chosen divergence
///
/// `convert` has **no `Integer` branch**: a context integer small enough for
/// `java.lang.Integer` falls through to `(String) input` and throws
/// `ClassCastException`, while the same digits written as a template literal
/// are a `Long` and are read as an epoch. So `{{ 1705314600 | date(…) }}`
/// works in Kestra and `{{ epoch | date(…) }}` with `epoch: 1705314600` in the
/// context does not.
///
/// egret cannot tell the two apart — the same limit that baselines
/// `filter-classname-literal-int`, and for the same reason (minijinja carries
/// the `U64`/`I64` discriminant but exposes no accessor, and the split is
/// positive-versus-signed rather than context-versus-literal). Given the
/// choice, this reads **every** whole number as a `Long`, because the
/// alternative is refusing a ten-digit epoch — the commonest number anyone
/// pipes into `date` — to reproduce what is plainly a Kestra bug.
/// `filter-date-epoch-seconds-context` is baselined with that reason.
fn convert(value: &MjValue, zone: &Zone, existing_format: Option<&str>) -> Result<Zdt, MjError> {
    if value.kind() == ValueKind::Number && value.is_integer() {
        let epoch = i64::try_from(value.clone()).map_err(|_| pebble_error("long overflow"))?;
        return epoch_to_zdt(epoch, zone.clone());
    }
    let Some(text) = value.as_str() else {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(value)
        )));
    };
    match existing_format {
        Some(existing) => Format::resolve(existing)
            .and_then(|format| format.parse_text(text, zone))
            .map_err(pebble_error),
        None => javafmt::parse_iso(text, zone).map_err(pebble_error),
    }
}

/// The `Long` branch of `AbstractDate.convert`, which reads a number's
/// **digit count** rather than its magnitude.
///
/// - 13 digits is milliseconds;
/// - 19 digits ending in `000` is nanoseconds *(`l / 1e9` seconds, `l % 1e9`
///   nanos)*;
/// - 19 digits otherwise is `ofEpochMilli(l / 1e6)` and then
///   `withNano((l % 1e6) * 1000)` — note `withNano` **replaces** the whole
///   nanosecond field, so the millisecond part of the first step is discarded:
///   `1705314600123456789` is `.456789`, not `.123456`. Measured.
/// - anything else is seconds.
///
/// `digitCount` subtracts one for a leading `-`, so the rule is about the
/// magnitude's digits, not the string's length.
fn epoch_to_zdt(epoch: i64, zone: Zone) -> Result<Zdt, MjError> {
    let digits = epoch.unsigned_abs().to_string().len();
    let (seconds, nanos) = match digits {
        13 => (
            epoch.div_euclid(1_000),
            (epoch.rem_euclid(1_000) * 1_000_000) as u32,
        ),
        19 if epoch.rem_euclid(1_000) == 0 => (
            epoch.div_euclid(1_000_000_000),
            epoch.rem_euclid(1_000_000_000) as u32,
        ),
        // `Instant.ofEpochMilli(l / 1e6)` -- MILLIseconds, so the division by
        // a thousand below is `ofEpochMilli`'s own, not a second truncation.
        // `withNano` then REPLACES the whole nanosecond field, discarding the
        // millisecond part the first step just set.
        19 => (
            epoch.div_euclid(1_000_000).div_euclid(1_000),
            (epoch.rem_euclid(1_000_000) * 1_000) as u32,
        ),
        _ => (epoch, 0),
    };
    Zdt::from_epoch(seconds, nanos, zone).ok_or_else(|| {
        pebble_error(format!(
            "Instant exceeds minimum or maximum instant: {epoch}"
        ))
    })
}

/// `DateFilter` — `AbstractDate.format(input, args, context)`.
///
/// `{{ d | date("yyyy-MM-dd'T'HH:mm:ss", timeZone='UTC') }}`. See
/// [`super::javafmt`] for the pattern language and for the two pattern letters
/// egret refuses.
fn date(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let date_args = DateArgs::read(&args, 0)?;
    let zdt = convert(
        &value,
        &date_args.zone,
        date_args.existing_format.as_deref(),
    )?;
    Ok(MjValue::from(date_args.render(&zdt)?))
}

/// `DateAddFilter` — `convert(...).plus(amount, ChronoUnit.valueOf(unit))`,
/// then the same `format` as `date`.
///
/// **The zone the arithmetic happens in is the input's, not `timeZone`'s.**
/// `timeZone` is only the formatter's zone and the fallback for an input with
/// no offset of its own, so `'2024-03-09T12:00:00-05:00' | dateAdd(1, 'DAYS',
/// 'iso', 'America/New_York')` adds a day in the **fixed -05:00 offset** the
/// string carried and only then re-zones for display. Measured; it is why the
/// oracle entry for DST arithmetic spells its input
/// `2024-03-09T12:00:00-05:00[America/New_York]`.
fn date_add(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let amount = get_as_long(args.get("amount", 0).as_ref())?;
    // `ChronoUnit.valueOf((String) args.get("unit"))` -- an absent `unit` is
    // `valueOf(null)`, which is `NullPointerException: Name is null`, not a
    // missing-argument error.
    let unit = match optional_string(&args, "unit", 1)? {
        Some(unit) => ChronoUnit::resolve(&unit).map_err(pebble_error)?,
        None => return Err(pebble_error("Name is null")),
    };
    let date_args = DateArgs::read(&args, 2)?;
    let zdt = convert(
        &value,
        &date_args.zone,
        date_args.existing_format.as_deref(),
    )?;
    let moved = zdt.plus(amount, unit).map_err(pebble_error)?;
    Ok(MjValue::from(date_args.render(&moved)?))
}

/// `DateAddFilter.getAsLong` — `Long`, `Integer`, any `Number` via
/// `longValue()`, or a `String` via `Long.parseLong`.
///
/// Both refusals are Kestra's own wording, and they differ: a string that is
/// not a number says `%s can't be converted to long`, while a non-number that
/// is not a string at all says `Incorrect %s format, must be a number`. A
/// `null` amount reaches the second, with the literal text `null` in it.
fn get_as_long(value: Option<&MjValue>) -> Result<i64, MjError> {
    let incorrect =
        |text: String| pebble_error(format!("Incorrect {text} format, must be a number"));
    let Some(value) = value else {
        return Err(incorrect("null".to_string()));
    };
    if is_java_null(value) {
        return Err(incorrect("null".to_string()));
    }
    if value.kind() == ValueKind::Number {
        // `Number.longValue()` truncates toward zero for a floating-point
        // value, which is why `dateAdd(2.9, 'DAYS')` adds two days.
        return if value.is_integer() {
            i64::try_from(value.clone()).map_err(|_| pebble_error("long overflow"))
        } else {
            f64::try_from(value.clone())
                .map(|f| f as i64)
                .map_err(|_| incorrect(kestra_to_string(value)))
        };
    }
    if let Some(text) = value.as_str() {
        return text
            .parse::<i64>()
            .map_err(|_| pebble_error(format!("{text} can't be converted to long")));
    }
    Err(incorrect(kestra_to_string(value)))
}

/// `TimestampFilter` — `convert(...).toEpochSecond()`, returning a **`Long`**.
fn timestamp(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some(zdt) = timestamp_input(&value, rest)? else {
        return Ok(MjValue::from(()));
    };
    Ok(MjValue::from(zdt.epoch_second()))
}

/// `TimestampMicroFilter` — `SECONDS.toMicros(epochSecond) +
/// NANOSECONDS.toMicros(nano)`, returning a **`String`**, not a number.
///
/// The four `timestamp*` filters do not agree about their return type:
/// `timestamp` and `timestampMilli` return `Long`, `timestampMicro` and
/// `timestampNano` return `String.valueOf(...)`. It renders the same digits
/// either way, so the only thing that shows it is a second filter —
/// `{{ d | timestampMicro | className }}` is `java.lang.String` where
/// `{{ d | timestampMilli | className }}` is `java.lang.Long`. Both are corpus
/// entries.
fn timestamp_micro(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some(zdt) = timestamp_input(&value, rest)? else {
        return Ok(MjValue::from(()));
    };
    let micros = zdt
        .epoch_second()
        .saturating_mul(1_000_000)
        .saturating_add(i64::from(zdt.nano() / 1_000));
    Ok(MjValue::from(micros.to_string()))
}

/// `TimestampMilliFilter` — `toInstant().toEpochMilli()`, a **`Long`**.
fn timestamp_milli(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some(zdt) = timestamp_input(&value, rest)? else {
        return Ok(MjValue::from(()));
    };
    let millis = zdt
        .epoch_second()
        .saturating_mul(1_000)
        .saturating_add(i64::from(zdt.nano() / 1_000_000));
    Ok(MjValue::from(millis))
}

/// `TimestampNanoFilter` — `SECONDS.toNanos(epochSecond) + nano`, a
/// **`String`**. See [`timestamp_micro`].
fn timestamp_nano(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let Some(zdt) = timestamp_input(&value, rest)? else {
        return Ok(MjValue::from(()));
    };
    let nanos = zdt
        .epoch_second()
        .saturating_mul(1_000_000_000)
        .saturating_add(i64::from(zdt.nano()));
    Ok(MjValue::from(nanos.to_string()))
}

/// The shared head of the four `timestamp*` filters: null passes through, and
/// the only arguments they declare are `[timeZone, existingFormat]` — no
/// `format` and no `locale`, because none of them formats anything.
fn timestamp_input(value: &MjValue, rest: Rest<MjValue>) -> Result<Option<Zdt>, MjError> {
    if is_java_null(value) {
        return Ok(None);
    }
    let args = Args::new(rest);
    let zone =
        Zone::resolve(optional_string(&args, "timeZone", 0)?.as_deref()).map_err(pebble_error)?;
    let existing_format = optional_string(&args, "existingFormat", 1)?;
    convert(value, &zone, existing_format.as_deref()).map(Some)
}

// ---------------------------------------------------------------------------
// Structured data and regular expressions (W2 Task 6)
// ---------------------------------------------------------------------------

/// `ToJsonFilter` — `JacksonMapper.ofJson().writeValueAsString(input)`.
///
/// **A null input is the four-character string `"null"`, not Java `null`**,
/// so `{{ nothing | toJson }}` renders `null` where `{{ nothing | chunk(…) }}`
/// renders nothing. `toIon` and `yaml` share that branch; `distinct` is the
/// only filter outside this batch that has it.
///
/// The serialisation is `super::env::write_json` with `omit_nulls` on, which
/// is the same writer the environment's own formatter uses, because
/// `JacksonMapper.ofJson()` is the same mapper `JsonWriter` holds. **`toIon`
/// and `yaml` are NOT the same mapper** — see the null-inclusion table in
/// [`super::ionfmt`].
fn to_json(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from("null"));
    }
    let mut out = String::new();
    write_json(&mut out, &value, true);
    Ok(MjValue::from(out))
}

/// `ToIonFilter` — `JacksonMapper.ofIon().writeValueAsString(input)`.
///
/// `createIonObjectMapper` calls the shared `configure()` and then
/// **overrides its `NON_NULL` back to `ALWAYS`**, so this keeps a null map
/// value where [`to_json`] drops it. Confirmed by `javap -p -c` on
/// `JacksonMapper.class`, and pinned by `filter-toion-map-null-value` against
/// `filter-tojson-map-null-value`.
fn to_ion(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from("null"));
    }
    let mut out = String::new();
    write_ion(&mut out, &value);
    Ok(MjValue::from(out))
}

/// `YamlFilter` — its own `ObjectMapper` over a `YAMLFactory` with seven
/// features set. See [`super::yamlfmt`] for what each of them does to the
/// output.
///
/// That mapper is built in `YamlFilter`'s static initialiser and never sees
/// `JacksonMapper.configure`, so it keeps null map values too.
fn yaml(value: MjValue) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from("null"));
    }
    Ok(MjValue::from(to_yaml(&value)))
}

/// `JqFilter` — a jq query, run over the input, **returning a LIST**.
///
/// Three things about it are not what the name suggests, and all three are
/// measured:
///
/// - **the result is always a list.** `JsonQuery.apply` emits into an
///   `ArrayList` and that list is the filter's return value, so
///   `{{ json | jq('.a.b') }}` renders `[42]` and not `42`. A single match is
///   still wrapped.
/// - **a String input is PARSED as JSON, not treated as a string.**
///   `ObjectMapper.readTree(input)` is the String branch, so
///   `{{ '42' | jq('.') }}` is `[42]` and `{{ 'abc' | jq('.') }}` is an error.
/// - **null map values are gone before jq sees them.** The non-String branch
///   is `JacksonMapper.ofJson().valueToTree(input)` and that mapper is
///   `NON_NULL`, so `{{ obj | jq('keys') }}` omits every key whose value is
///   null. `filter-jq-keys-drops-null-values` is the entry.
///
/// Every failure — a bad query, a runtime error, an unparseable String input
/// — comes back as the *same* sentence, because `JqFilter` catches
/// `Exception` around the whole body and rebuilds the message from the input:
/// `Unable to parse jq value '<input>' with type '<class>'`.
fn jq(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let Some(expression) = args.get("expression", 0) else {
        return Err(pebble_error(
            "The 'jq' filter expects an argument 'expression'.",
        ));
    };
    // `(String) args.get("expression")` -- a non-String is a
    // ClassCastException, which Pebble does not wrap, so it is its own
    // message rather than the catch-all one below.
    if expression.kind() != ValueKind::String {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(&expression)
        )));
    }
    let expression = expression.as_str().unwrap_or_default().to_string();

    let failed = || {
        pebble_error(format!(
            "Unable to parse jq value '{}' with type '{}'",
            kestra_to_string(&value),
            java_class_name(&value)
        ))
    };

    let input = match value.as_str() {
        Some(text) => read_tree(text).ok_or_else(failed)?,
        None => {
            let mut json = String::new();
            write_json(&mut json, &value, true);
            jaq_json::read::parse_single(json.as_bytes()).map_err(|_| failed())?
        }
    };

    let outputs = run_jq(&expression, input).map_err(|e| match e {
        JqFailure::Failed => failed(),
        JqFailure::Refused(message) => pebble_error(message),
    })?;
    Ok(MjValue::from(outputs))
}

/// Compiles and runs one jq program, collecting every output.
///
/// Split out so the filter above reads as Kestra's `apply` does, and so the
/// three things this has to do that Kestra's four lines do not are in one
/// place: **withhold `env`, refuse to let `halt` kill the process, and bound
/// the output**.
///
/// # `env` is withheld
///
/// `jaq_std` registers `env`, which is `std::env::vars()` — the whole process
/// environment of whatever is doing the rendering, `DATABASE_URL` and every
/// secret in it included. `{{ x | jq('env') }}` would hand a flow author all
/// of it.
///
/// Kestra's own jq (`jackson-jq` at `Versions.JQ_1_6`) **has no `env`**:
/// measured through the harness, `{{ obj | jq('env | type') }}` is an error
/// there. So withholding it is fidelity as well as the obvious thing to do.
/// `$ENV` is unreachable for a different reason — it is a variable, and this
/// binds none.
///
/// # `halt` must not reach `process::exit`
///
/// `jaq_core::unwrap_valr` — the function every example in the crate's own
/// docs calls — ends in `std::process::exit(code)` when the program was
/// `halt`. A rendered `Property` is author-supplied source on a request path,
/// so `{{ x | jq('halt') }}` would **terminate the process**. This matches on
/// [`jaq_core::Exn`] directly instead and turns a halt into an ordinary
/// failure, which is what Kestra does with it anyway (jackson-jq 1.6 has no
/// `halt` and errors).
///
/// # The output is bounded
///
/// jq generators are infinite: `1 | repeat(. + 1)` never stops, and Kestra
/// collects into an `ArrayList` until the JVM dies. The accumulated size is
/// checked against [`MAX_OUTPUT_BYTES`], the same 1 MiB ceiling
/// `runners::env` already caps a render at — nothing that survives this loop
/// could fit in the output anyway, so refusing here rather than after the
/// allocation costs no reachable behaviour. Same call as `indent`'s prefix
/// bound and `rewrite`'s nesting depth.
fn run_jq(expression: &str, input: jaq_json::Val) -> Result<Vec<MjValue>, JqFailure> {
    // Before the loader, because the loader is what overflows the stack.
    jqsandbox::check_source(expression).map_err(|e| JqFailure::Refused(e.0))?;

    let program = jaq_core::load::File {
        code: expression,
        path: (),
    };

    // `jqsandbox` owns both lists: jaq's own unbounded `repeat`/`recurse`/
    // `while`/`until`/`range` definitions are spliced out for bounded ones,
    // and the native list is an allow-list with six host-reading names
    // replaced by refusals. Read that module before changing either.
    let defs = jqsandbox::definitions();
    let funs = jqsandbox::natives();

    let loader = jaq_core::load::Loader::new(defs);
    let arena = jaq_core::load::Arena::default();
    let modules = loader
        .load(&arena, program)
        .map_err(|_| JqFailure::Failed)?;
    let filter = jaq_core::Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|_| JqFailure::Failed)?;

    let ctx = jaq_core::Ctx::<jqsandbox::Data>::new(&filter.lut, jaq_core::Vars::new([]));
    let mut outputs = Vec::new();
    let mut bytes = 0usize;
    for result in filter.id.run((ctx, input)) {
        // Never `unwrap_valr`: see this function's doc. A halt and an error
        // are both just "this jq program failed", which is the only thing
        // `JqFilter`'s catch-all reports either.
        // A jq runtime error is Kestra's catch-all sentence -- except when it
        // is EGRET's own bound talking. `runners::jqsandbox` writes every one
        // of its refusals with a `jq: ` prefix precisely so it can be told
        // apart here, the same line `runners::javare` draws between
        // `Invalid regex` and `Unsupported regex`. Without this the bound
        // fires and the author is told their data could not be parsed.
        let value = result.map_err(|exn| match exn.get_err() {
            Ok(error) => {
                // jq's `error(msg)` carries the message as a VALUE, and a
                // string value's `Display` is its JSON form -- quotes
                // included. Measured, not assumed: the text arriving here is
                // `"\"jq: repeat exceeded ...\""`.
                let message = error.to_string();
                let unquoted = message.trim_matches('"');
                if unquoted.starts_with(EGRET_JQ_PREFIX) {
                    JqFailure::Refused(unquoted.to_string())
                } else {
                    JqFailure::Failed
                }
            }
            // A halt. Never `unwrap_valr`, which would `process::exit`.
            Err(_) => JqFailure::Failed,
        })?;
        bytes = bytes
            .saturating_add(value.to_string().len())
            .saturating_add(1);
        // This bounds a generator that EMITS. It is not the bound that stops
        // `[repeat(1)]` -- that one never reaches here, and
        // `runners::jqsandbox` is what stops it.
        if bytes > MAX_OUTPUT_BYTES {
            return Err(JqFailure::Failed);
        }
        outputs.push(jaq_value(value));
    }
    Ok(outputs)
}

/// The marker every refusal `runners::jqsandbox` writes begins with, so a
/// bound firing is not reported as the author's data being unparseable.
const EGRET_JQ_PREFIX: &str = "jq: ";

/// Why a jq call did not produce a value.
///
/// The two are reported differently on purpose. Kestra collapses every runtime
/// failure into one sentence about the *input*, and egret reproduces that --
/// but a program egret **refuses** is not a Kestra failure at all, and saying
/// "Unable to parse jq value …" about it would blame the author's data for
/// egret's own limit. `runners::javare` draws the same line between
/// `Invalid regex` and `Unsupported regex`.
enum JqFailure {
    /// Kestra fails here too, with the message `jq` reports.
    Failed,
    /// egret refused the query; Kestra would have run it.
    Refused(String),
}

/// One `jaq_json::Val` as a minijinja value.
///
/// Objects become [`OrderedMap`], because `JqFilter`'s own output lambda
/// converts an `ObjectNode` to a `Map` and Kestra's writer keeps insertion
/// order — `keys_unsorted` is the entry that separates the two.
fn jaq_value(value: jaq_json::Val) -> MjValue {
    match value {
        jaq_json::Val::Null => MjValue::from(()),
        jaq_json::Val::Bool(b) => MjValue::from(b),
        jaq_json::Val::Num(n) => match n {
            jaq_json::Num::Int(i) => MjValue::from(i as i64),
            jaq_json::Num::Float(f) => MjValue::from(f),
            // A big integer or a decimal keeps its text; both are outside
            // what an `i64` holds and neither has a corpus entry.
            other => match other.to_string().parse::<i128>() {
                Ok(i) => MjValue::from(i),
                Err(_) => match other.to_string().parse::<f64>() {
                    Ok(f) => MjValue::from(f),
                    Err(_) => MjValue::from(other.to_string()),
                },
            },
        },
        jaq_json::Val::BStr(s) | jaq_json::Val::TStr(s) => {
            MjValue::from(String::from_utf8_lossy(&s).into_owned())
        }
        jaq_json::Val::Arr(items) => {
            MjValue::from(items.iter().cloned().map(jaq_value).collect::<Vec<_>>())
        }
        jaq_json::Val::Obj(entries) => MjValue::from_object(OrderedMap(
            entries
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        jaq_json::Val::BStr(s) | jaq_json::Val::TStr(s) => {
                            String::from_utf8_lossy(s).into_owned()
                        }
                        other => other.to_string(),
                    };
                    (key, jaq_value(v.clone()))
                })
                .collect(),
        )),
    }
}

/// `ObjectMapper.readTree(String)`, which is **not** "parse this whole string
/// as JSON".
///
/// Two of its behaviours had to be reproduced deliberately, both measured:
///
/// - **an empty (or blank) input is not an error.** `readTree("")` returns a
///   `MissingNode`, and `{{ '' | jq('.') }}` is `[null]`.
/// - **it reads the FIRST value and stops.** `readTree("1 2")` is `1`, so
///   `{{ '1 2' | jq('.') }}` is `[1]` and not an error. `jaq`'s
///   `parse_single` refuses trailing input, which is the stricter and here
///   the wrong answer.
///
/// A String input that is not JSON at all (`'abc'`) is still a failure on
/// both sides.
fn read_tree(text: &str) -> Option<jaq_json::Val> {
    if text.trim().is_empty() {
        return Some(jaq_json::Val::Null);
    }
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>();
    let first = stream.next()?.ok()?;
    jaq_json::read::parse_single(first.to_string().as_bytes()).ok()
}

/// The `regex` argument, which all three regex filters read the same way.
///
/// Kestra's guard is `args.get("regex") == null`, so an *explicitly null*
/// argument fails exactly as an absent one does — and the message is
/// `MessageFormat`'s, with the name already substituted into the `''{0}''`
/// placeholder.
fn regex_argument(args: &Args, index: usize) -> Result<javare::JavaRegex, MjError> {
    let Some(regex) = args.get("regex", index).filter(|v| !is_java_null(v)) else {
        return Err(pebble_error("The argument 'regex' is required."));
    };
    // `args.get("regex").toString()` -- a number or a boolean is stringified
    // rather than refused, which is why this goes through `kestra_to_string`
    // and not through an `as_str`.
    let pattern = kestra_to_string(&regex);
    javare::compile(&pattern).map_err(|e| match e {
        // Kestra's own wording, from `MessageFormat.format("Invalid regex
        // ''{0}'': {1}", pattern, e.getDescription())`.
        javare::JavaRegexError::Syntax(_) => {
            pebble_error(format!("Invalid regex '{pattern}': {e}"))
        }
        // egret's own: Kestra accepts this pattern and egret will not run it.
        // Worded so it cannot be mistaken for the sentence above.
        javare::JavaRegexError::Unsupported(_) => {
            pebble_error(format!("Unsupported regex '{pattern}': {e}"))
        }
    })
}

/// Turns a haystack refusal into the same shape of message.
fn regex_haystack_error(pattern: &str, e: javare::JavaRegexError) -> MjError {
    pebble_error(format!("Unsupported regex '{pattern}': {e}"))
}

/// `RegexMatchFilter` — `Pattern.compile(regex).matcher(input).find()`.
///
/// **`find()`, not `matches()`**: the pattern is searched for anywhere in the
/// input, so `'abc123' | regexMatch(regex='\\d+')` is `true` even though the
/// pattern does not describe the whole string. A null input is `false` and
/// not null, which is this filter alone in the batch.
fn regex_match(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(false));
    }
    let args = Args::new(rest);
    let pattern = regex_pattern_text(&args, 0)?;
    let regex = regex_argument(&args, 0)?;
    let subject = kestra_to_string(&value);
    regex
        .is_match(&subject)
        .map(MjValue::from)
        .map_err(|e| regex_haystack_error(&pattern, e))
}

/// `RegexReplaceFilter` — `Matcher.replaceAll(replacement)`.
///
/// The replacement is Java's language, not the `regex` crate's: `\$` is a
/// literal dollar, `$2` against a one-group pattern throws, and `$10` backs
/// off to group 1. [`super::javare::expand_replacement`]'s doc has the table.
fn regex_replace(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let regex = regex_argument(&args, 0)?;
    let Some(replacement) = args.get("replacement", 1).filter(|v| !is_java_null(v)) else {
        return Err(pebble_error("The argument 'replacement' is required."));
    };
    let replacement = kestra_to_string(&replacement);
    let subject = kestra_to_string(&value);
    regex
        .replace_all(&subject, &replacement)
        .map(MjValue::from)
        .map_err(|e| pebble_error(e.to_string()))
}

/// `RegexExtractFilter` — the first match's group `group`, or null.
///
/// Three ways it answers null and they are not the same thing: no match at
/// all, a group that did not participate in the match, and a null input. Java
/// returns `null` for each and Pebble renders all three as nothing.
fn regex_extract(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let pattern = regex_pattern_text(&args, 0)?;
    let regex = regex_argument(&args, 0)?;

    // `args.containsKey("group") && args.get("group") != null
    //     ? ((Number) args.get("group")).intValue() : 0`.
    let group = match args.get("group", 1) {
        None => 0,
        Some(value) if is_java_null(&value) => 0,
        Some(value) => java_int_value(&value).ok_or_else(|| {
            pebble_error(format!(
                "class {} cannot be cast to class java.lang.Number",
                java_class_name(&value)
            ))
        })?,
    };
    if group < 0 {
        return Err(pebble_error(format!(
            "Group index {} is out of bounds: must be >= 0.",
            grouped(group)
        )));
    }
    let group = group as usize;
    if group > regex.group_count() {
        return Err(pebble_error(format!(
            "Group index {} is out of bounds: the pattern has only {} capture group(s).",
            grouped(group as i32),
            grouped(regex.group_count() as i32)
        )));
    }

    let subject = kestra_to_string(&value);
    match regex
        .find_group(&subject, group)
        .map_err(|e| regex_haystack_error(&pattern, e))?
    {
        Some(Some(text)) => Ok(MjValue::from(text)),
        _ => Ok(MjValue::from(())),
    }
}

/// The `regex` argument's text, read once so an error message can quote it.
fn regex_pattern_text(args: &Args, index: usize) -> Result<String, MjError> {
    match args.get("regex", index).filter(|v| !is_java_null(v)) {
        Some(value) => Ok(kestra_to_string(&value)),
        None => Err(pebble_error("The argument 'regex' is required.")),
    }
}

/// `MessageFormat`'s number placeholder, which is `NumberFormat.getInstance()`
/// and therefore **groups thousands**: `{0}` on 12345 writes `12,345`.
///
/// It only shows above 999, which no plausible `group` argument reaches — but
/// the alternative is a message that silently differs from Kestra's for a
/// value that can be reached, and the whole point of quoting Kestra's wording
/// is that it is Kestra's.
fn grouped(n: i32) -> String {
    let negative = n < 0;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if negative {
        format!("-{out}")
    } else {
        out
    }
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

    /// `getLineSeparator`'s order, **through the filter** rather than through
    /// the private helper.
    ///
    /// The helper-level test that used to stand here could not see the
    /// separator being used at all: replacing `line_separator(&input)` with a
    /// literal `"\n"` inside `apply_indent` — deleting CRLF/LFCR/CR detection
    /// from both filters — left it green, and `filter-indent-crlf` cannot see
    /// it either, because `"\r\n"` contains `"\n"` and the naive `replace`
    /// lands in the same place. **LFCR and CR-only are the discriminating
    /// inputs**, and they are corpus entries now as well
    /// (`filter-indent-lfcr`, `filter-indent-cr-only`,
    /// `filter-nindent-cr-only`).
    #[test]
    fn indent_uses_the_detected_line_separator() {
        // CR-only: a `"\n"`-assuming implementation finds nothing to replace
        // and returns the input untouched.
        assert_eq!(
            ok("{{ s | indent(amount=2) }}", r#"{"s": "a\rb"}"#),
            "a\r  b"
        );
        assert_eq!(
            ok("{{ s | nindent(amount=2) }}", r#"{"s": "a\rb"}"#),
            "\r  a\r  b"
        );
        // LFCR: a `"\n"`-assuming implementation indents BETWEEN the two
        // characters instead of after them.
        assert_eq!(
            ok("{{ s | indent(amount=2) }}", r#"{"s": "a\n\rb"}"#),
            "a\n\r  b"
        );
        // CRLF, which `filter-indent-crlf` also covers, kept so the order of
        // the first two tests is visible here as well.
        assert_eq!(
            ok("{{ s | indent(amount=2) }}", r#"{"s": "a\r\nb"}"#),
            "a\r\n  b"
        );
    }

    /// The helper's own table, kept beside the filter-level test above: it
    /// documents the fallthrough case (`"ab"`), which no filter-level
    /// assertion can distinguish because a `replace` of a separator the input
    /// does not contain is a no-op either way.
    #[test]
    fn line_separator_detects_in_kestras_order() {
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

    /// The root cause, one level below `escapeChar`: what
    /// `kestra_to_string` writes for each kind a Kestra expression can
    /// produce.
    ///
    /// **This asserts the VALUES, not the absence of `None`.** The
    /// absence-only assertion that stood here was satisfied by any other
    /// wrong spelling — returning `"nil"` left it green — and it is the only
    /// pin on a function every filter Tasks 5-8 inherits. That weakness is
    /// exactly what let the `Undefined` regression through: folding
    /// `Undefined` into the `"null"` arm removed no `None` and broke five
    /// filters.
    #[test]
    fn kestra_to_string_writes_javas_spellings_not_minijinjas() {
        // A null INSIDE a container is `String.valueOf(null)` -> `null`.
        assert_eq!(
            ok("{{ list | string }}", r#"{"list": [1, null]}"#),
            "[1, null]"
        );
        assert_eq!(ok("{{ m | string }}", r#"{"m": {"a": null}}"#), "{a=null}");
        // The other two Java spellings this function exists for.
        assert_eq!(
            ok("{{ list | string }}", r#"{"list": [true, 1e100]}"#),
            "[true, 1.0E100]"
        );
        // A null as the filter's own INPUT is Kestra's `return null`, which
        // renders as nothing -- NOT the four characters `null`.
        assert_eq!(ok("<{{ n | string }}>", r#"{"n": null}"#), "<>");
        // And Pebble's `null` LITERAL, which reaches minijinja as an
        // undefined, answers the same way. Folding that into the container
        // rule is the regression this pins, filter by filter.
        assert_eq!(ok("<{{ null | string }}>", "{}"), "<>");
        assert_eq!(ok("<{{ null | slugify }}>", "{}"), "<>");
        assert_eq!(ok("<{{ null | indent(amount=2) }}>", "{}"), "<>");
        assert_eq!(ok("<{{ null | escapeChar(type='single') }}>", "{}"), "<>");
        assert_eq!(
            ok("<{{ null | substringBefore(separator='-') }}>", "{}"),
            "<>"
        );
        // ...except `distinct`, which alone answers the STRING "null".
        assert_eq!(ok("<{{ null | distinct }}>", "{}"), "<null>");
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

    /// **I-1.** A dedup key built by joining pieces with bare separators can
    /// be forged by a value that contains those separators, and `distinct`
    /// then **drops a row** — data loss, not a rendering difference. Both
    /// shapes, because the map key and the list key were built by different
    /// code paths.
    #[test]
    fn distinct_cannot_have_its_key_forged_by_content() {
        assert_eq!(
            ok(
                "{{ list | distinct }}",
                r#"{"list": [["a,s:b"], ["a", "b"]]}"#
            ),
            "[[\"a,s:b\"],[\"a\",\"b\"]]"
        );
        assert_eq!(
            ok(
                "{{ list | distinct }}",
                r#"{"list": [{"a": "b,c=s:d"}, {"a": "b", "c": "d"}]}"#
            ),
            "[{\"a\":\"b,c=s:d\"},{\"a\":\"b\",\"c\":\"d\"}]"
        );
        // Still order-independent for maps, which is the property the
        // length-prefixing had to preserve rather than replace.
        assert_eq!(
            ok(
                "{{ list | distinct }}",
                r#"{"list": [{"a": 1, "b": 2}, {"b": 2, "a": 1}]}"#
            ),
            "[{\"a\":1,\"b\":2}]"
        );
    }

    /// **I-2.** The same unbounded allocation `indent` was given a bound for,
    /// one screen away: `toPlainString` writes `|scale|` zeros and the
    /// exponent is an `i32`, so `"1E-2000000000"` is a two-gigabyte string
    /// from a 13-byte input.
    ///
    /// Kestra really does build it — `RenderLimits` has exactly two call
    /// sites in the artifact and `NumberFilter` is neither — so this is a
    /// divergence, recorded as one. The 402-character case below is the
    /// largest with a committed golden.
    #[test]
    fn bigdecimal_refuses_to_expand_past_the_output_cap() {
        assert_eq!(
            ok("{{ s | number(type='BIGDECIMAL') }}", r#"{"s": "1E-400"}"#).len(),
            402
        );
        for input in ["1E-10000000", "1E-2000000000", "1E2000000000"] {
            let cx = format!(r#"{{"s": "{input}"}}"#);
            assert!(
                render("{{ s | number(type='BIGDECIMAL') }}", &cx).is_err(),
                "{input} was not refused"
            );
        }
    }

    /// **I-3.** commons-lang3 3.20's `createNumber` computes an offset from a
    /// leading `+` or `-` and then tests `{"0x","0X","#"}`; the pre-3.12 form
    /// was a literal prefix list with no `+`.
    ///
    /// The consequence is the bug I2 existed to kill: `java_radix` strips `+`,
    /// so with the old list the same string answered `255` with a `type` and
    /// errored without one.
    #[test]
    fn create_number_accepts_a_signed_hex_prefix() {
        assert_eq!(ok("{{ s | number }}", r#"{"s": "+0xFF"}"#), "255");
        assert_eq!(
            ok("{{ s | number(type='INT') }}", r#"{"s": "+0xFF"}"#),
            "255"
        );
        assert_eq!(ok("{{ s | number }}", r#"{"s": "-0xFF"}"#), "-255");
        assert_eq!(ok("{{ s | number }}", r#"{"s": "+#FF"}"#), "255");
    }

    /// `createNumber` falls through to `createBigDecimal` when the double
    /// **underflowed to zero from a non-zero mantissa**, so `"1E-400"` keeps
    /// all 400 places instead of becoming `0.0`. `"0.0"` is the other side:
    /// the mantissa really is zero, so the double is kept.
    #[test]
    fn create_number_does_not_lose_a_value_that_underflows() {
        assert_eq!(ok("{{ s | number }}", r#"{"s": "1E-400"}"#).len(), 402);
        assert_eq!(ok("{{ s | number }}", r#"{"s": "0.0"}"#), "0.0");
        assert_eq!(ok("{{ s | number }}", r#"{"s": "0.000"}"#), "0.0");
    }

    /// `(String) args.getOrDefault("type", "")` then `switch (type)`: a
    /// non-string `type` is a `ClassCastException` and a null one an NPE,
    /// neither of which quietly means "no type given".
    #[test]
    fn number_reads_its_type_argument_strictly() {
        assert!(err("{{ s | number(type=t) }}", r#"{"s": "12", "t": 5}"#)
            .contains("cannot be cast to class java.lang.String"));
        assert!(err("{{ s | number(type=t) }}", r#"{"s": "12", "t": null}"#)
            .contains("because the 'type' argument is null"));
        // Absent really is "no type given", and an UNRECOGNISED one falls
        // through Java's `switch` to `createNumber` rather than erroring.
        assert_eq!(ok("{{ s | number }}", r#"{"s": "12"}"#), "12");
        assert_eq!(ok("{{ s | number(type='NOPE') }}", r#"{"s": "12"}"#), "12");
    }

    /// **Kestra's own bound**, `AbstractIndent.MAX_AMOUNT` = 10000, read off
    /// the artifact (`javap -p -c` on `AbstractIndent.class`) because neither
    /// it nor `RenderLimits` exists in the pinned checkout.
    ///
    /// This is fidelity, not a divergence: the message is Kestra's own text,
    /// word for word, and `filter-indent-amount-at-max` /
    /// `filter-indent-amount-over-max` commit both sides of the boundary.
    #[test]
    fn indent_reproduces_kestras_own_max_amount() {
        for kind in ["indent", "nindent"] {
            let message = err(
                &format!("{{{{ s | {kind}(amount=10001) }}}}"),
                r#"{"s": "a\nb"}"#,
            );
            assert_eq!(
                message,
                format!(
                    "invalid operation: The '{kind}' filter's 'amount' argument cannot exceed \
                     10000, but 10001 was given. (in <string>:1)"
                )
            );
        }
        // 10000 itself is allowed, so the comparison is `>` and not `>=`.
        assert_eq!(
            ok("{{ s | indent(amount=10000) }}", r#"{"s": "a\nb"}"#).len(),
            3 + 10000
        );
    }

    /// **A bound Kestra does NOT have, and much narrower than an earlier
    /// version of this test claimed.**
    ///
    /// Kestra bounds `amount` (see above), not the product, so
    /// `indent(amount=10000, prefix=<100 chars>)` really does build a megabyte
    /// there. egret refuses when the projected size passes
    /// `MAX_OUTPUT_BYTES`, because a cap that only measures what *survives*
    /// the allocation is not a cap — the same call `rewrite`'s
    /// `MAX_NESTING_DEPTH` makes.
    ///
    /// No corpus entry: the golden would be a multi-megabyte string in
    /// `golden.json`. This test is the whole pin, and `oracle/README.md` says
    /// so.
    #[test]
    fn indent_refuses_to_allocate_past_the_output_cap() {
        // `amount` is inside Kestra's own limit in every case here, so what
        // fires is egret's projection and not `MAX_INDENT_AMOUNT`.
        for template in [
            "{{ s | indent(amount=2, prefix=wide) }}",
            "{{ s | nindent(amount=2, prefix=wide) }}",
            // 10000 x 110 = 1_100_000 > 1_048_576. At a 100-character
            // prefix the product is 1_000_003 and BOTH engines allow it --
            // the divergence band starts around 105, not at "a megabyte".
            "{{ s | indent(amount=10000, prefix=wide_enough) }}",
        ] {
            let message = err(
                template,
                &format!(
                    r#"{{"s": "a\nb", "wide": "{}", "wide_enough": "{}"}}"#,
                    "-".repeat(600_000),
                    "-".repeat(110)
                ),
            );
            // The FILTER's own message, not `RenderError::OutputTooLarge`'s
            // ("rendered output exceeded 1048576 bytes"). The distinction is
            // the whole claim: the output cap fires after the allocation, this
            // fires instead of it.
            assert!(
                message.contains("filter would produce")
                    && message.contains("over the 1048576-byte render limit"),
                "{template} gave {message}"
            );
        }

        // And an ordinary indent is untouched, so the bound is about the size
        // and not about the filter.
        assert_eq!(
            ok("{{ s | indent(amount=2) }}", r#"{"s": "a\nb"}"#),
            "a\n  b"
        );
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

    // -- W2 Task 5: the fourteen date/time `error-class` entries ----------
    //
    // Same discipline as the four above: the oracle's error-class comparison
    // is satisfied by any failure, so what actually holds these to account is
    // egret's own message being asserted here. `oracle/error-class-pairs.txt`
    // names each of these tests against its id, and
    // `every_error_class_entry_is_paired_with_something_that_can_fail` fails
    // by name if one is renamed away.

    /// `DateTimeFormatter.ofPattern` bounds every repeatable letter itself,
    /// and **that is why this task needed no `RenderLimits`-style guard of
    /// its own.** `d` stops at 2, `S` at 9 and `y` at 19, so a pattern of ten
    /// thousand letters is an `IllegalArgumentException` rather than an
    /// allocation, in Kestra and here. The three messages are the JDK's own,
    /// and they are three DIFFERENT sentences for what looks like one rule.
    #[test]
    fn date_reproduces_javas_pattern_letter_limits() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert!(
            err("{{ d | date('ddd', timeZone='UTC') }}", d).contains("Too many pattern letters: d")
        );
        assert!(err("{{ d | date('SSSSSSSSSS', timeZone='UTC') }}", d)
            .contains("Minimum width must be from 0 to 9 inclusive but was 10"));
        assert!(
            err("{{ d | date('yyyyyyyyyyyyyyyyyyyy', timeZone='UTC') }}", d)
                .contains("The minimum width must be from 1 to 19 inclusive but was 20")
        );
        // **A fourth sentence, for the two letters whose accepted counts are
        // not a range.** `v` takes 1 or 4 and `B` takes 1, 4 or 5; anything
        // else is `Wrong number of pattern letters`, which is a different rule
        // from `Too many pattern letters` and was unpinned until the fix
        // round's probe sweep walked every letter at counts 1-11.
        assert!(err("{{ d | date('vv', timeZone='UTC') }}", d)
            .contains("Wrong number of pattern letters: v"));
        assert!(err("{{ d | date('BB', timeZone='UTC') }}", d)
            .contains("Wrong number of pattern letters: B"));
        // `v` at a count Java accepts is a REFUSAL, not an invalid pattern --
        // the distinction the sweep found missing entirely.
        assert!(err("{{ d | date('vvvv', timeZone='UTC') }}", d)
            .contains("cannot render pattern letter 'v'"));
        // One under each limit still works, so the messages above are a
        // boundary and not a blanket refusal.
        assert_eq!(ok("{{ d | date('dd', timeZone='UTC') }}", d), "15");
        assert_eq!(
            ok("{{ d | date('yyyyyyyyyyyyyyyyyyy', timeZone='UTC') }}", d),
            "0000000000000002024"
        );
    }

    /// The three ways a pattern's *syntax* is rejected, as opposed to its
    /// letter counts. Note `]` is an error but a dangling `[` is not, and
    /// `#`/`{`/`}` are reserved rather than literal.
    #[test]
    fn date_reproduces_javas_pattern_syntax_errors() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert!(err(r#"{{ d | date("yyyy'MM", timeZone='UTC') }}"#, d)
            .contains("Pattern ends with an incomplete string literal: yyyy'MM"));
        assert!(err("{{ d | date('yyyy#MM', timeZone='UTC') }}", d)
            .contains("Pattern includes reserved character: '#'"));
        assert!(err("{{ d | date('yyyy]', timeZone='UTC') }}", d)
            .contains("Pattern invalid as it contains ] without previous ["));
        assert_eq!(ok("{{ d | date('yyyy[MM', timeZone='UTC') }}", d), "202401");
    }

    /// **An unknown format NAME is not an unknown-format error.**
    /// `AbstractDate.formatter` falls through the STYLES map and the
    /// FORMATTERS map to `ofPattern(name)`, so the name is compiled as a
    /// pattern and the message names the first character of it that is not a
    /// pattern letter. `nope_format` fails on its `o`.
    #[test]
    fn an_unknown_format_name_is_compiled_as_a_pattern() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert!(err("{{ d | date('nope_format', timeZone='UTC') }}", d)
            .contains("Unknown pattern letter: o"));
        // …and a name made only of pattern letters really does format.
        assert_eq!(ok("{{ d | date('dd', timeZone='UTC') }}", d), "15");
        // **The boundary**: the eighteen FORMATTERS names and the four styles
        // are looked up BEFORE the fallthrough, so a name that IS in the maps
        // never reaches the pattern compiler -- `sql_sec` would otherwise die
        // on its `l`, which is not a pattern letter at all.
        assert_eq!(
            ok("{{ d | date('sql_sec', timeZone='UTC') }}", d),
            "2024-01-15 10:30:00"
        );
        assert!(err("{{ d | date('sql_secx', timeZone='UTC') }}", d)
            .contains("Unknown pattern letter: l"));
    }

    /// `convert`'s last branch is a hard `(String) input` cast, so anything
    /// that is neither a number nor a string is a `ClassCastException` naming
    /// the Java class it would have been.
    #[test]
    fn date_rejects_an_input_that_is_not_a_string() {
        assert!(
            err("{{ d | date('iso', timeZone='UTC') }}", r#"{"d": true}"#)
                .contains("class java.lang.Boolean cannot be cast to class java.lang.String")
        );
        assert!(
            err("{{ d | date('iso', timeZone='UTC') }}", r#"{"d": [1, 2]}"#)
                .contains("class java.util.ArrayList cannot be cast to class java.lang.String")
        );
        // A non-integer number is the same refusal -- `convert` has a `Long`
        // branch and no `Double` one.
        assert!(
            err("{{ d | date('iso', timeZone='UTC') }}", r#"{"d": 1.5}"#)
                .contains("class java.lang.Double cannot be cast to class java.lang.String")
        );
        // **The boundary.** A whole number one step away is NOT refused -- it
        // is the `Long` branch -- and neither is a string. An error-class
        // entry pins only THAT egret refuses; without these two lines nothing
        // pins WHICH inputs it refuses, which is exactly where `v` and the
        // four legacy zone ids lived.
        assert_eq!(
            ok(
                "{{ d | date('iso', timeZone='UTC') }}",
                r#"{"d": 1705314600}"#
            ),
            "2024-01-15T10:30:00.000000Z"
        );
        assert_eq!(
            ok(
                "{{ d | date('iso', timeZone='UTC') }}",
                r#"{"d": "2024-01-15T10:30:00Z"}"#
            ),
            "2024-01-15T10:30:00.000000Z"
        );
    }

    #[test]
    fn date_refuses_an_unparseable_input() {
        for input in [
            r#"{"d": "not-a-date"}"#,
            r#"{"d": ""}"#,
            r#"{"d": "2024-02-30"}"#,
            // One character past what the ISO grammar takes, in three
            // different places -- the BOUNDARY, not just three bad strings.
            r#"{"d": "2024-1-5"}"#,
            r#"{"d": "2024-01-15T10:30:00+0700"}"#,
            r#"{"d": "2024-01-15 10:30:00"}"#,
            r#"{"d": "2024-01-15T10:30:00.1234567891Z"}"#,
        ] {
            let message = err("{{ d | date('iso', timeZone='UTC') }}", input);
            assert!(message.contains("could not be parsed"), "{message}");
        }
        // …and each one's accepted neighbour.
        for input in [
            r#"{"d": "2024-02-29"}"#,
            r#"{"d": "2024-01-05"}"#,
            r#"{"d": "2024-01-15T10:30:00+07:00"}"#,
            r#"{"d": "2024-01-15T10:30:00"}"#,
            r#"{"d": "2024-01-15T10:30:00.123456789Z"}"#,
        ] {
            render("{{ d | date('iso', timeZone='UTC') }}", input)
                .unwrap_or_else(|e| panic!("{input} must parse: {e}"));
        }
    }

    /// `ZoneId.of` is case-sensitive and rejects an unknown region, and so is
    /// this: `utc` is not `UTC`.
    #[test]
    fn date_refuses_an_unknown_time_zone() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert!(err("{{ d | date('iso', timeZone='Not/AZone') }}", d)
            .contains("Unknown time-zone ID: Not/AZone"));
        assert!(
            err("{{ d | date('iso', timeZone='utc') }}", d).contains("Unknown time-zone ID: utc")
        );
        // The four `chrono-tz` ids `ZoneId.of` rejects -- see
        // `the_four_legacy_zone_ids_java_rejects_are_denied_here_too` for why
        // this set is closed rather than a guess.
        assert!(
            err("{{ d | date('iso', timeZone='EST') }}", d).contains("Unknown time-zone ID: EST")
        );
    }

    /// `ChronoUnit.valueOf` takes the enum's own spelling, so `'days'` is not
    /// `'DAYS'` -- and the message is `java.lang.IllegalArgumentException`'s,
    /// which names the fully-qualified enum.
    #[test]
    fn date_add_refuses_a_unit_that_is_not_a_chrono_unit() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert!(err("{{ d | dateAdd(1, 'days', 'iso', 'UTC') }}", d)
            .contains("No enum constant java.time.temporal.ChronoUnit.days"));
        assert!(err("{{ d | dateAdd(1, 'FORTNIGHTS', 'iso', 'UTC') }}", d)
            .contains("No enum constant java.time.temporal.ChronoUnit.FORTNIGHTS"));
        // **The boundary.** An error-class entry pins only THAT egret
        // refuses; its paired test has to pin WHICH inputs, or the refusal
        // could quietly widen -- which is exactly where `v` and the four
        // legacy zone ids lived. So: the same word in the enum's own
        // spelling is accepted, and so is the one name with punctuation in
        // it.
        assert_eq!(
            ok("{{ d | dateAdd(1, 'DAYS', 'uuuu-MM-dd', 'UTC') }}", d),
            "2024-01-16"
        );
        assert_eq!(
            ok(
                "{{ d | dateAdd(1, 'HALF_DAYS', 'uuuu-MM-dd HH:mm', 'UTC') }}",
                d
            ),
            "2024-01-15 22:30"
        );
    }

    /// An absent `unit` is `ChronoUnit.valueOf(null)`, whose message is the
    /// three words `Name is null` -- not a missing-argument error, which is
    /// what a reader would expect and what an invented message would say.
    #[test]
    fn date_add_refuses_a_missing_unit() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert!(
            err("{{ d | dateAdd(1, format='iso', timeZone='UTC') }}", d).contains("Name is null")
        );
        // **The boundary**: an explicitly-null `unit` is the same refusal,
        // and supplying it by keyword rather than by position is not.
        assert!(err("{{ d | dateAdd(1, null, 'iso', 'UTC') }}", d).contains("Name is null"));
        assert_eq!(
            ok(
                "{{ d | dateAdd(1, unit='DAYS', format='uuuu-MM-dd', timeZone='UTC') }}",
                d
            ),
            "2024-01-16"
        );
    }

    /// `getAsLong` has **two** refusals with different wording, and which one
    /// you get depends on whether the value was a string.
    #[test]
    fn date_add_refuses_an_amount_that_is_not_a_number() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert!(err("{{ d | dateAdd(true, 'DAYS', 'iso', 'UTC') }}", d)
            .contains("Incorrect true format, must be a number"));
        assert!(err("{{ d | dateAdd('x', 'DAYS', 'iso', 'UTC') }}", d)
            .contains("x can't be converted to long"));
        assert!(err(
            "{{ d | dateAdd(unit='DAYS', format='iso', timeZone='UTC') }}",
            d
        )
        .contains("Incorrect null format, must be a number"));
        // A numeric STRING is fine, and a fractional number truncates through
        // `Number.longValue()` rather than erroring.
        assert_eq!(
            ok("{{ d | dateAdd('3', 'DAYS', 'yyyy-MM-dd', 'UTC') }}", d),
            "2024-01-18"
        );
        assert_eq!(
            ok("{{ d | dateAdd(2.9, 'DAYS', 'yyyy-MM-dd', 'UTC') }}", d),
            "2024-01-17"
        );
    }

    // -- W2 Task 5: behaviours with no corpus entry ----------------------

    /// **Nothing in this batch can be made to allocate disproportionately, so
    /// nothing in it is bounded by egret.**
    ///
    /// `RenderLimits` has exactly two call sites in the whole
    /// `io.kestra:core:2.0.0-rc13` jar — `AbstractIndent` and
    /// `BoundedRangeFunction` — confirmed with `grep -rla RenderLimits` over
    /// every extracted class, and `AbstractDate` is neither. That is fidelity
    /// here rather than a gap, because the two vectors a date filter could
    /// have are already closed upstream:
    ///
    /// - a **repeated pattern letter** is capped by the JDK's own compiler
    ///   for all but one letter (see
    ///   [`date_reproduces_javas_pattern_letter_limits`]). **`Y` is the
    ///   exception and the first version of this claim was wrong about it**:
    ///   `WeekBasedFieldPrinterParser` has no cap, so `Y` × 20 really does
    ///   print twenty characters and `Y` × n prints n. The bound survives, but
    ///   for a weaker reason than "every letter writes O(1)" — every letter
    ///   writes at most `count` characters, so a pattern's output stays
    ///   **linear in the pattern's own length**, which is what
    ///   `MAX_OUTPUT_BYTES` already measures where it is written. Corrected by
    ///   the third review, which also measured that egret had been safe here
    ///   only by refusing `Y` past 19, i.e. by a bug;
    /// - a **huge `dateAdd` amount** is arithmetic, not allocation, and
    ///   overflows into an error long before it costs anything.
    ///
    /// This test is the pin for that claim: both shapes refuse, and neither
    /// takes measurable time.
    #[test]
    fn no_date_filter_can_be_made_to_allocate_out_of_proportion() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        let huge = "y".repeat(100_000);
        assert!(
            err(&format!("{{{{ d | date('{huge}', timeZone='UTC') }}}}"), d)
                .contains("The minimum width must be from 1 to 19 inclusive")
        );
        // `Y` is the one letter with NO cap upstream, so it is the one that
        // has to be linear rather than refused: 4096 letters, 4096 characters.
        let wide = "Y".repeat(4096);
        assert_eq!(
            ok(&format!("{{{{ d | date('{wide}', timeZone='UTC') }}}}"), d).len(),
            4096
        );
        // The longest a pattern's output can be is a pad, and a pad's width is
        // the pattern's own length -- linear, so `MAX_OUTPUT_BYTES` already
        // measures it where it is written.
        //
        // **Asserted as the exact string, not as a length.** The first cut
        // asserted `.len() == 64`, which a pad that filled with `'0'` instead
        // of `' '`, or padded on the wrong side, satisfies just as well --
        // mutating the pad character left all 246 lib tests green. Found by
        // the fix round's review.
        let pad = format!("{}H", "p".repeat(64));
        let padded = ok(&format!("{{{{ d | date('{pad}', timeZone='UTC') }}}}"), d);
        assert_eq!(padded, format!("{}10", " ".repeat(62)));
        assert!(err(
            "{{ d | dateAdd(9223372036854775807, 'DAYS', 'iso', 'UTC') }}",
            d
        )
        .contains("long overflow"));
        assert!(
            err("{{ d | dateAdd(1000000000, 'YEARS', 'iso', 'UTC') }}", d)
                .contains("Invalid value for Year")
        );
    }

    /// `ChronoUnit.ERAS` is a real unit and `plus(1, ERAS)` is a real call --
    /// it just cannot succeed on an ISO date, because ISO has two eras. The
    /// message is `DateTimeException`'s, and reproducing it is what stops the
    /// unit looking unsupported.
    #[test]
    fn date_add_refuses_an_era_rollover_but_not_the_unit() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert!(err("{{ d | dateAdd(1, 'ERAS', 'iso', 'UTC') }}", d)
            .contains("Invalid value for Era (valid values 0 - 1): 2"));
        assert!(err("{{ d | dateAdd(1, 'FOREVER', 'iso', 'UTC') }}", d)
            .contains("Unsupported unit: Forever"));
        // A zero-era shift is the identity and does NOT throw.
        assert_eq!(
            ok("{{ d | dateAdd(0, 'ERAS', 'yyyy-MM-dd', 'UTC') }}", d),
            "2024-01-15"
        );
    }

    /// A **present but null** `format` is not the default.
    /// `AbstractDate.format` reads it with `containsKey`, so `date(null)`
    /// reaches `DateTimeFormatter.ofPattern(null)` and the message is the
    /// single word `pattern`, from the NPE. Omitting the argument entirely is
    /// what selects `iso`.
    /// **This assertion used to be satisfiable by the wrong implementation.**
    /// It read `contains("pattern")`, and the obvious wrong port -- drop the
    /// `containsKey` special case, stringify a null `format` as `"null"`,
    /// reach `Pattern::parse("null")` -- fails with
    /// `Unknown pattern letter: l`, which contains "pattern" too. Mutating the
    /// message left all 246 lib tests green. Found by the fix round's review;
    /// the message is now pinned WHOLE, and the wrong route's message is
    /// asserted absent.
    #[test]
    fn a_present_but_null_format_is_not_the_default() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        let message = err("{{ d | date(null, timeZone='UTC') }}", d);
        assert!(
            message.contains("invalid operation: pattern"),
            "the NPE's message is the single word `pattern`: {message}"
        );
        assert!(
            !message.contains("Unknown pattern letter"),
            "a null `format` must not be stringified into a pattern: {message}"
        );
        assert_eq!(
            ok("{{ d | date(timeZone='UTC') }}", d),
            "2024-01-15T10:30:00.000000Z"
        );
    }

    /// **egret's default zone is UTC where Kestra's is the host's.**
    ///
    /// `AbstractDate.zoneId(null)` is `ZoneId.systemDefault()`, which makes
    /// every unqualified `date` call answer differently on a laptop and on a
    /// cluster node — `oracle/README.md` records that as a standing rule for
    /// the corpus (every date entry passes `timeZone` explicitly) and it is
    /// the wrong default for a server rendering other people's workflows.
    /// This is the one divergence in `Zone::resolve`, and no corpus entry can
    /// hold it, because a corpus entry that reached it would not be
    /// reproducible on another machine — which is the whole point.
    #[test]
    fn an_absent_time_zone_is_utc_here_and_the_hosts_zone_in_kestra() {
        assert_eq!(
            ok("{{ d | date('iso') }}", r#"{"d": "2024-01-15T10:30:00"}"#),
            "2024-01-15T10:30:00.000000Z"
        );
    }

    /// The twelve FORMATTERS entries that are `java.time` constants have no
    /// pattern spelling, so they cannot drive egret's pattern PARSER. Four of
    /// them fail in Kestra too (they yield no date); the rest are a stated
    /// divergence rather than an approximation.
    #[test]
    fn a_java_constant_format_cannot_be_an_existing_format() {
        let message = err(
            "{{ d | date('iso', timeZone='UTC', existingFormat='iso_local_date') }}",
            r#"{"d": "2024-01-15"}"#,
        );
        assert!(
            message.contains("java.time constant with no pattern spelling"),
            "{message}"
        );
        // The six FORMATTERS entries Kestra builds with `ofPattern` DO parse.
        assert_eq!(
            ok(
                "{{ d | date('yyyy', timeZone='UTC', existingFormat='sql_sec') }}",
                r#"{"d": "2024-01-15 10:30:00"}"#
            ),
            "2024"
        );
    }

    // -- W2 Task 5 fix round 1: what the probe sweep earned --------------

    /// The three year letters take **three sign styles by count**, and both
    /// edges were silently wrong.
    ///
    /// `uu` is `Math.abs(value) % 100`, so year -44 is `44` and not `56`; and
    /// `SignStyle.EXCEEDS_PAD` writes a `+` at count >= 4 when the value needs
    /// more digits than the pad, so Kestra's DEFAULT `iso` format renders
    /// `+10000-01-15T…`. `filter-date-pattern-era-bc` could not see the first
    /// because it probes `y|u|yyyy|uuuu` and omits the one count that breaks;
    /// nothing probed a year past 9999 at all.
    #[test]
    fn the_year_letters_take_three_sign_styles_by_count() {
        let bc = r#"{"d": "-0044-03-15T10:30:00Z"}"#;
        let big = r#"{"d": "+10000-01-15T10:30:00Z"}"#;
        // Reduced: absolute value FIRST, so -44 is `44` and -1 is `01`.
        assert_eq!(
            ok("{{ d | date('u|uu|uuu|uuuu', timeZone='UTC') }}", bc),
            "-44|44|-044|-0044"
        );
        assert_eq!(
            ok(
                "{{ d | date('u|uu', timeZone='UTC') }}",
                r#"{"d": "-0001-12-31T00:00:00Z"}"#
            ),
            "-1|01"
        );
        // EXCEEDS_PAD: a `+` only at count >= 4 and only when the digits
        // exceed the width. Count 5 holds 10000 exactly, so no sign; count 6
        // pads instead.
        assert_eq!(
            ok(
                "{{ d | date('y|yy|yyy|yyyy|yyyyy|yyyyyy', timeZone='UTC') }}",
                big
            ),
            "10000|00|10000|+10000|10000|010000"
        );
        // …and it reaches the DEFAULT format, which is the reason this matters.
        assert_eq!(
            ok("{{ d | date(timeZone='UTC') }}", big),
            "+10000-01-15T10:30:00.000000Z"
        );
    }

    /// `p` needs a **pattern letter** after it, and a pad that is too narrow
    /// for its value **throws** rather than letting the value through.
    ///
    /// Both were wrong in the first cut, and `filter-date-pattern-pad` could
    /// not see either: its `ppH|'x'pppmm` is evaluated at **05:07**, the one
    /// clock time where both fields fit inside their pads. At 10:30 the same
    /// shape throws.
    #[test]
    fn date_reproduces_javas_padding_rules() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        // A pad must be followed by a letter -- not a quote, not a bracket.
        assert!(err(r#"{{ d | date("pp'x'", timeZone='UTC') }}"#, d)
            .contains("Pad letter 'p' must be followed by valid pad pattern: pp'x'"));
        assert!(err("{{ d | date('p[H]', timeZone='UTC') }}", d)
            .contains("Pad letter 'p' must be followed by valid pad pattern: p[H]"));
        assert!(err("{{ d | date('pp', timeZone='UTC') }}", d)
            .contains("Pad letter 'p' must be followed by valid pad pattern: pp"));
        // …and the pad must be wide enough for what it pads.
        assert!(err("{{ d | date('pH', timeZone='UTC') }}", d)
            .contains("Cannot print as output of 2 characters exceeds pad width of 1"));
        // The passing side, spelled out so the pad character and side are
        // pinned and not just the width.
        assert_eq!(ok("{{ d | date('ppppH', timeZone='UTC') }}", d), "  10");
        assert_eq!(
            ok(
                "{{ d | date('pH', timeZone='UTC') }}",
                r#"{"d": "2024-01-15T05:30:00Z"}"#
            ),
            "5"
        );
    }

    /// `ZoneOffset.of`'s grammar is driven by the id's **length**, and each
    /// rejection has its own sentence.
    ///
    /// A first cut counted digits and ignored where the colon sat, so
    /// `+0:730` and `+5:30` both resolved to a zone here and throw in Kestra.
    #[test]
    fn date_reproduces_javas_zone_offset_grammar() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert!(err("{{ d | date('iso', timeZone='+0:730') }}", d)
            .contains("Invalid ID for ZoneOffset, non numeric characters found: +0:730"));
        assert!(err("{{ d | date('iso', timeZone='+5:30') }}", d)
            .contains("Invalid ID for ZoneOffset, non numeric characters found: +5:30"));
        assert!(err("{{ d | date('iso', timeZone='+19:00') }}", d)
            .contains("Zone offset hours not in valid range: value 19 is not in the range"));
        assert!(err("{{ d | date('iso', timeZone='+18:00:01') }}", d)
            .contains("Zone offset not in valid range: -18:00 to +18:00"));
        // A prefixed offset re-wraps the failure under `ZoneId`'s own message.
        assert!(err("{{ d | date('iso', timeZone='GMT+') }}", d)
            .contains("Invalid ID for offset-based ZoneId: GMT+"));
        // The forms Java DOES take, including the one-digit hour that only
        // exists under a prefix.
        assert_eq!(ok("{{ d | date('VV', timeZone='+18:00') }}", d), "+18:00");
        assert_eq!(ok("{{ d | date('VV', timeZone='GMT+7') }}", d), "GMT+07:00");
        assert_eq!(ok("{{ d | date('VV', timeZone='+05') }}", d), "+05:00");
    }

    /// `rfc_1123_date_time` is the one named format with a **year range**:
    /// its year is `appendValue(YEAR, 4, 4, NOT_NEGATIVE)`, so a BC year and a
    /// five-digit year each throw, with different sentences.
    #[test]
    fn rfc_1123_refuses_a_year_it_cannot_print() {
        assert!(err(
            "{{ d | date('rfc_1123_date_time', timeZone='UTC') }}",
            r#"{"d": "+10000-01-15T10:30:00Z"}"#
        )
        .contains("exceeds the maximum print width of 4"));
        assert!(err(
            "{{ d | date('rfc_1123_date_time', timeZone='UTC') }}",
            r#"{"d": "-0044-03-15T10:30:00Z"}"#
        )
        .contains("cannot be negative according to the SignStyle"));
        // Four digits is fine, and so is a three-digit year zero-padded to it.
        assert_eq!(
            ok(
                "{{ d | date('rfc_1123_date_time', timeZone='UTC') }}",
                r#"{"d": "0999-03-15T10:30:00Z"}"#
            ),
            "Fri, 15 Mar 0999 10:30:00 GMT"
        );
    }

    /// **`HALF_DAYS` is not twelve hours per unit past 255.**
    ///
    /// `LocalDateTime.plus` is
    /// `plusDays(n / 256).plusHours((n % 256) * 12)` — the divisor should be
    /// 2 — so `dateAdd(256, 'HALF_DAYS')` adds ONE DAY, not 128. Reproduced
    /// rather than corrected: it is a wrong answer, not a resource vector, and
    /// a port that quietly fixed it would give a different date for the same
    /// flow. Found by the fix round's probe sweep; the original corpus probed
    /// `HALF_DAYS` only at 1, where the two agree.
    #[test]
    fn date_add_reproduces_javas_half_days_arithmetic() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        let day = |amount: i64| {
            ok(
                &format!("{{{{ d | dateAdd({amount}, 'HALF_DAYS', 'uuuu-MM-dd HH:mm', 'UTC') }}}}"),
                d,
            )
        };
        // Below 256 the identity holds: 255 half-days is 127 days and 12h.
        assert_eq!(day(255), "2024-05-21 22:30");
        // At 256 it does not: one day, where twelve-hours-per-unit is 128.
        assert_eq!(day(256), "2024-01-16 10:30");
        assert_eq!(day(-1000), "2023-09-18 10:30");
    }

    /// `plus(n, ERAS)` **flips the era** when the target is 0 or 1; it is not
    /// a no-op-or-throw. `with(ERA, 0)` on 2024 is `withYear(1 - 2024)`.
    #[test]
    fn date_add_flips_the_era_rather_than_refusing_every_shift() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        assert_eq!(
            ok("{{ d | dateAdd(-1, 'ERAS', 'uuuu-MM-dd', 'UTC') }}", d),
            "-2023-01-15"
        );
        assert_eq!(
            ok("{{ d | dateAdd(0, 'ERAS', 'uuuu-MM-dd', 'UTC') }}", d),
            "2024-01-15"
        );
        // Only a target outside {0, 1} is the range error.
        assert!(err("{{ d | dateAdd(1, 'ERAS', 'iso', 'UTC') }}", d)
            .contains("Invalid value for Era (valid values 0 - 1): 2"));
        assert!(err("{{ d | dateAdd(-2, 'ERAS', 'iso', 'UTC') }}", d)
            .contains("Invalid value for Era (valid values 0 - 1): -1"));
    }

    /// **egret's date range is `chrono`'s, and it says so in its own words.**
    ///
    /// A `NaiveDateTime` reaches about ±262 143 years and `LocalDateTime`
    /// reaches ±999 999 999, so a `dateAdd` in between succeeds in Kestra and
    /// cannot be represented here. The limit is `chrono`'s; the first cut
    /// reported it as `long overflow`, which is Java's sentence for a
    /// different cause, and its test asserted exactly the two extremes where
    /// the two engines agree. `filter-dateadd-past-egrets-date-range` is the
    /// baselined entry.
    #[test]
    fn date_add_past_egrets_range_says_so_rather_than_claiming_overflow() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        let message = err("{{ d | dateAdd(100000000, 'DAYS', 'iso', 'UTC') }}", d);
        assert!(message.contains("egret's dates are chrono's"), "{message}");
        assert!(
            !message.contains("long overflow"),
            "a representable-range refusal must not borrow Java's arithmetic-overflow \
             sentence: {message}"
        );
        // Genuine `long` overflow keeps Java's wording, and the two are now
        // told apart rather than sharing one message.
        assert!(err(
            "{{ d | dateAdd(9223372036854775807, 'DAYS', 'iso', 'UTC') }}",
            d
        )
        .contains("long overflow"));
    }

    /// `chrono-tz` carries four zone ids `ZoneId.of` rejects, and the set is
    /// closed: every one of its 597 names was fed to `ZoneId.of` and exactly
    /// `EST`, `HST`, `MST` and `ROC` threw. Longer aliases of the same zones
    /// are fine on both sides, which is what makes this counter-intuitive.
    #[test]
    fn the_four_legacy_zone_ids_java_rejects_are_denied_here_too() {
        let d = r#"{"d": "2024-01-15T10:30:00Z"}"#;
        for id in ["EST", "HST", "MST", "ROC"] {
            let message = err(&format!("{{{{ d | date('iso', timeZone='{id}') }}}}"), d);
            assert!(
                message.contains(&format!("Unknown time-zone ID: {id}")),
                "{message}"
            );
        }
        assert_eq!(ok("{{ d | date('VV', timeZone='EST5EDT') }}", d), "EST5EDT");
        assert_eq!(
            ok("{{ d | date('VV', timeZone='US/Eastern') }}", d),
            "US/Eastern"
        );
    }

    /// Java's **adjacent value parsing**: a variable-width field hands digits
    /// back to the fixed-width fields that follow it, which is the only reason
    /// `yyyyMMdd` can parse `20240115`. And `ofPattern`'s resolver is
    /// **SMART**, so a day past the end of its month is clamped — while the
    /// bare ISO path is STRICT and refuses the same input.
    #[test]
    fn an_existing_format_parses_adjacent_values_and_resolves_smartly() {
        let iso = "{{ d | date('iso', timeZone='UTC', existingFormat=";
        assert_eq!(
            ok(&format!("{iso}'yyyyMMdd') }}}}"), r#"{"d": "20240115"}"#),
            "2024-01-15T00:00:00.000000Z"
        );
        assert_eq!(
            ok(
                &format!("{iso}'yyyyMMddHHmmss') }}}}"),
                r#"{"d": "20240115103000"}"#
            ),
            "2024-01-15T10:30:00.000000Z"
        );
        // SMART clamps the day; STRICT (the bare ISO path) does not.
        assert_eq!(
            ok(
                &format!("{iso}'yyyy-MM-dd') }}}}"),
                r#"{"d": "2024-02-30"}"#
            ),
            "2024-02-29T00:00:00.000000Z"
        );
        assert!(err(
            "{{ d | date('iso', timeZone='UTC') }}",
            r#"{"d": "2024-02-30"}"#
        )
        .contains("could not be parsed"));
        // A field range is still checked: SMART clamps a date, not a value.
        assert!(err(
            &format!("{iso}'yyyy-MM-dd') }}}}"),
            r#"{"d": "2024-13-01"}"#
        )
        .contains("could not be parsed"));
    }

    // -- W2 Task 5 fix round 3 -----------------------------------------
    //
    // Every corpus entry behind these was written BEFORE its fix and shown to
    // FAIL against the pre-fix implementation. Two reviews in a row found new
    // entries sitting one variant away from the failure they were added for --
    // `Q E G` on values that agree, `KK` at 12 but not `hh` at 0, `DAYS` for a
    // bug in the time-based units -- and demonstrating the failure first is
    // the only thing that breaks that pattern.

    /// **Java cross-checks the fields it parses; it does not ignore them.**
    ///
    /// `Parsed.updateCheckConflict` resolves each auxiliary field into the
    /// date and compares. W2 Task 5's fix round 2 wrote down the opposite and
    /// consumed nine letters silently, which turned thirteen probes from
    /// *agreeing* (both engines refused the letter) into egret answering where
    /// Kestra throws — twice with a wrong time.
    ///
    /// The same mechanism refuses a field supplied twice with two values.
    #[test]
    fn java_cross_checks_the_fields_it_parses() {
        // 2024-03-09 is a Saturday in Q1: day-of-year 69, aligned week 2,
        // modified Julian day 60378.
        let day = r#"{"d": "2024-03-09 VALUE"}"#;
        let ef = |pattern: &str| {
            format!("{{{{ d | date('uuuu-MM-dd', timeZone='UTC', existingFormat='uuuu-MM-dd {pattern}') }}}}")
        };
        for (pattern, agrees, conflicts) in [
            ("Q", "1", "4"),
            ("EEEE", "Saturday", "Monday"),
            ("DDD", "069", "300"),
            ("F", "2", "5"),
            ("g", "60378", "60000"),
        ] {
            // The value that AGREES is accepted…
            assert_eq!(
                ok(&ef(pattern), &day.replace("VALUE", agrees)),
                "2024-03-09",
                "`{pattern}` = {agrees} agrees with the date"
            );
            // …and the one beside it is a conflict. This is the pair the old
            // `filter-date-existing-format-ignored-fields` entry could not
            // have: it used `Q E G` on `1 Mon AD`, and 2024-01-15 *is* a
            // Monday in Q1 AD.
            let message = err(&ef(pattern), &day.replace("VALUE", conflicts));
            assert!(message.contains("Conflict found"), "`{pattern}`: {message}");
        }
        // A field parsed twice must agree with itself.
        assert_eq!(
            ok(
                "{{ d | date('uuuu-MM-dd HH:mm', timeZone='UTC', existingFormat='uuuu-MM-dd HH:mm dd') }}",
                r#"{"d": "2024-03-09 14:25 09"}"#
            ),
            "2024-03-09 14:25"
        );
        assert!(err(
            "{{ d | date('uuuu-MM-dd HH:mm', timeZone='UTC', existingFormat='uuuu-MM-dd HH:mm dd') }}",
            r#"{"d": "2024-03-09 14:25 22"}"#
        )
        .contains("Conflict found"));
        // `A` is a whole time, so it conflicts with an h/m/s that disagrees.
        assert_eq!(
            ok(
                "{{ d | date('uuuu-MM-dd HH:mm:ss', timeZone='UTC', existingFormat='uuuu-MM-dd HH:mm:ss A') }}",
                r#"{"d": "2024-03-09 14:25:36 51936000"}"#
            ),
            "2024-03-09 14:25:36"
        );
        assert!(err(
            "{{ d | date('uuuu-MM-dd HH:mm:ss', timeZone='UTC', existingFormat='uuuu-MM-dd HH:mm:ss A') }}",
            r#"{"d": "2024-03-09 14:25:36 1"}"#
        )
        .contains("Conflict found"));
    }

    /// `ResolverStyle.SMART` exempts a **zero** clock-hour, for `hh` as well
    /// as for `k` — the same block twice in `Parsed`, and fix round 2
    /// implemented it once.
    ///
    /// `hh:mm a` on `00:30 AM` was byte-exact before that round and an error
    /// after it: the one probe in 5 528 that regressed from right to broken.
    #[test]
    fn smart_exempts_a_zero_clock_hour_but_not_a_thirteenth() {
        let ef = "{{ d | date('uuuu-MM-dd HH:mm', timeZone='UTC', existingFormat='uuuu-MM-dd hh:mm a') }}";
        assert_eq!(
            ok(ef, r#"{"d": "2024-03-09 00:30 AM"}"#),
            "2024-03-09 00:30"
        );
        assert_eq!(
            ok(ef, r#"{"d": "2024-03-09 12:30 AM"}"#),
            "2024-03-09 00:30"
        );
        // The exemption is 0, not "anything": 13 is still out of range.
        assert!(err(ef, r#"{"d": "2024-03-09 13:30 AM"}"#)
            .contains("Invalid value for ClockHourOfAmPm"));
        // And `k` keeps its own, which is the block this was copied from.
        assert_eq!(
            ok(
                "{{ d | date('uuuu-MM-dd HH:mm', timeZone='UTC', existingFormat='uuuu-MM-dd kk:mm') }}",
                r#"{"d": "2024-03-09 24:30"}"#
            ),
            "2024-03-09 00:30"
        );
    }

    /// `SignStyle.EXCEEDS_PAD` reads back the way it writes: the `+` is
    /// **required** when the value needs more digits than the pad and
    /// forbidden otherwise.
    ///
    /// Without it an `iso` round trip of a year past 9999 failed on its second
    /// leg — the other half of fix round 1's EXCEEDS_PAD fix, which had only
    /// done the writing.
    #[test]
    fn a_wide_year_is_read_back_only_with_its_sign() {
        let ef = |pattern: &str| {
            format!("{{{{ d | date('uuuu-MM-dd', timeZone='UTC', existingFormat='{pattern}') }}}}")
        };
        assert_eq!(
            ok(&ef("uuuu-MM-dd"), r#"{"d": "+10000-01-15"}"#),
            "+10000-01-15"
        );
        // `uuuuu` already holds five digits, so the sign is not expected.
        assert!(err(&ef("uuuuu-MM-dd"), r#"{"d": "+10000-01-15"}"#).contains("could not be parsed"));
        // The whole round trip, which is what this is for.
        assert_eq!(
            ok(
                "{{ d | date('iso', timeZone='UTC', existingFormat='iso') }}",
                r#"{"d": "+10000-01-15T10:30:00.000000Z"}"#
            ),
            "+10000-01-15T10:30:00.000000Z"
        );
    }

    /// An optional section that fails leaves its fields **unset**, so
    /// `[uuuu-]MM-dd` on `01-15` has no year and cannot make a date — while
    /// the same pattern on `2024-01-15` does.
    #[test]
    fn an_optional_section_that_fails_leaves_no_year() {
        let ef = "{{ d | date('uuuu-MM-dd', timeZone='UTC', existingFormat='[uuuu-]MM-dd') }}";
        assert_eq!(ok(ef, r#"{"d": "2024-01-15"}"#), "2024-01-15");
        assert!(err(ef, r#"{"d": "01-15"}"#).contains("could not be parsed"));
    }

    /// The five letters whose parse-side meaning is the **host's region**,
    /// refused rather than answered.
    ///
    /// `w`, `W`, `Y`, `e` and `c` resolve through `WeekFields.of(locale)`, and
    /// `AbstractDate.convert` never passes the template's `locale` to the
    /// parser — the harness's own conflict messages read
    /// `WeekFields[MONDAY,1]`, which is the JVM's region and not the entry's
    /// argument. Fix round 2 consumed them silently, which made egret answer
    /// a machine-dependent question; this refuses and says why.
    ///
    /// No corpus entry can hold this: an entry using those letters in an
    /// `existingFormat` would not be reproducible on another host, which is
    /// the same rule `timeZone` and `locale` already carry.
    #[test]
    fn the_locale_resolved_parse_fields_are_refused_rather_than_guessed() {
        for letter in ['w', 'W', 'Y', 'e', 'c'] {
            let message = err(
                &format!(
                    "{{{{ d | date('uuuu-MM-dd', timeZone='UTC', existingFormat='uuuu-MM-dd {letter}') }}}}"
                ),
                r#"{"d": "2024-03-09 2"}"#,
            );
            assert!(
                message.contains("the host's region"),
                "`{letter}`: {message}"
            );
        }
        // The locale-INDEPENDENT neighbours are not refused.
        assert_eq!(
            ok(
                "{{ d | date('uuuu-MM-dd', timeZone='UTC', existingFormat='uuuu-MM-dd F') }}",
                r#"{"d": "2024-03-09 2"}"#
            ),
            "2024-03-09"
        );
    }

    /// **The time-resolution cascade**: which fields make a time, and what
    /// happens to the offset when none of them does.
    ///
    /// `Parsed.resolveTimeLenient` only produces a `LocalTime` when the
    /// fields form a contiguous chain from the hour down, and
    /// `AbstractDate.convert` then falls through to
    /// `LocalDate.parse(...).atStartOfDay()` — **discarding the offset and
    /// the zone with it**. So an `existingFormat` of `uuuu-MM-dd mm` answers
    /// midnight and drops the minute. Fourteen of these were wrong before the
    /// fix round's `existingFormat` sweep, all of them `OK -> OK`.
    #[test]
    fn only_a_contiguous_chain_of_time_fields_makes_a_time() {
        let ef = |pattern: &str, input: &str| {
            ok(
                &format!(
                    "{{{{ d | date('uuuu-MM-dd HH:mm:ss', timeZone='UTC', existingFormat='{pattern}') }}}}"
                ),
                &format!(r#"{{"d": "{input}"}}"#),
            )
        };
        // hour, hour+minute, hour+minute+second: a time.
        assert_eq!(ef("uuuu-MM-dd HH", "2024-01-15 10"), "2024-01-15 10:00:00");
        assert_eq!(
            ef("uuuu-MM-dd HH:mm", "2024-01-15 10:30"),
            "2024-01-15 10:30:00"
        );
        // …and one link missing from the chain: NO time, midnight.
        assert_eq!(ef("uuuu-MM-dd mm", "2024-01-15 30"), "2024-01-15 00:00:00");
        assert_eq!(ef("uuuu-MM-dd ss", "2024-01-15 45"), "2024-01-15 00:00:00");
        assert_eq!(
            ef("uuuu-MM-dd HH ss", "2024-01-15 10 45"),
            "2024-01-15 00:00:00"
        );
        // An hour-of-am-pm needs its `a`; with one it resolves, without one
        // it does not. And an `a` on its own is the MIDDLE of its half-day.
        assert_eq!(ef("uuuu-MM-dd hh", "2024-01-15 10"), "2024-01-15 00:00:00");
        assert_eq!(
            ef("uuuu-MM-dd hh a", "2024-01-15 10 PM"),
            "2024-01-15 22:00:00"
        );
        assert_eq!(ef("uuuu-MM-dd a", "2024-01-15 AM"), "2024-01-15 06:00:00");
        assert_eq!(ef("uuuu-MM-dd a", "2024-01-15 PM"), "2024-01-15 18:00:00");
        // `A` and `N` carry a whole time in one field.
        assert_eq!(
            ef("uuuu-MM-dd A", "2024-01-15 37845123"),
            "2024-01-15 10:30:45"
        );
        // **With no time, the parsed offset is discarded too** and the
        // `timeZone` argument places the date -- so this is midnight UTC, not
        // an instant in +07:00.
        assert_eq!(
            ef("uuuu-MM-dd XXX", "2024-01-15 +07:00"),
            "2024-01-15 00:00:00"
        );
        // …and with a time it is not discarded. (Double-quoted, because the
        // pattern carries a `'T'` literal of its own.)
        assert_eq!(
            ok(
                r#"{{ d | date('uuuu-MM-dd HH:mm:ss', timeZone='UTC', existingFormat="uuuu-MM-dd'T'HH:mm:ssXXX") }}"#,
                r#"{"d": "2024-01-15T10:30:00+07:00"}"#
            ),
            "2024-01-15 03:30:00"
        );
    }

    /// An `existingFormat`'s fields are **range-checked on the way in**, and
    /// each check has Java's own sentence.
    ///
    /// The boundary matters more than the message here: `KK` takes 11 and
    /// refuses 12, `yyyy` takes 1 and refuses 0, and `uuuu` — one letter
    /// away — takes both 0 and a negative year.
    #[test]
    fn an_existing_formats_fields_are_range_checked() {
        let ef = |pattern: &str| {
            format!(
                "{{{{ d | date('uuuu-MM-dd HH:mm', timeZone='UTC', existingFormat='{pattern}') }}}}"
            )
        };
        let ctx = |input: &str| format!(r#"{{"d": "{input}"}}"#);
        assert!(err(&ef("uuuu-MM-dd KK:mm a"), &ctx("2024-01-15 12:30 PM"))
            .contains("Invalid value for HourOfAmPm (valid values 0 - 11): 12"));
        assert_eq!(
            ok(&ef("uuuu-MM-dd KK:mm a"), &ctx("2024-01-15 11:30 PM")),
            "2024-01-15 23:30"
        );
        assert!(err(&ef("yyyy-MM-dd"), &ctx("0000-01-15")).contains("Invalid value for YearOfEra"));
        assert!(err(&ef("yyyy-MM-dd"), &ctx("-0044-01-15")).contains("Invalid value for YearOfEra"));
        // One letter away, and both are fine: `u` is the proleptic year.
        assert_eq!(
            ok(&ef("uuuu-MM-dd"), &ctx("-0044-01-15")),
            "-0044-01-15 00:00"
        );
        assert_eq!(
            ok(&ef("uuuu-MM-dd"), &ctx("0000-01-15")),
            "0000-01-15 00:00"
        );
    }

    /// The offset letters are **count-specific and case-sensitive** on the
    /// parse side, where the ISO path is neither.
    ///
    /// `XXX` is `+HH:MM` and takes nothing else — not `+07`, not a lowercase
    /// `z` — while `X` takes `+07` and `Z` takes `+0700`. The first cut
    /// routed all of them through the ISO offset parser, which accepts every
    /// shape.
    #[test]
    fn an_existing_formats_offset_letters_are_count_specific() {
        let ef = |pattern: &str| {
            format!(
                "{{{{ d | date('uuuu-MM-dd HH:mm:ss', timeZone='UTC', existingFormat='uuuu-MM-dd HH:mm {pattern}') }}}}"
            )
        };
        let ctx = |input: &str| format!(r#"{{"d": "2024-01-15 10:30 {input}"}}"#);
        assert!(err(&ef("XXX"), &ctx("+07")).contains("could not be parsed"));
        assert!(err(&ef("XXX"), &ctx("z")).contains("could not be parsed"));
        // The boundary: the shapes each count DOES take.
        assert_eq!(ok(&ef("XXX"), &ctx("+07:00")), "2024-01-15 03:30:00");
        assert_eq!(ok(&ef("X"), &ctx("+07")), "2024-01-15 03:30:00");
        assert_eq!(ok(&ef("XXX"), &ctx("Z")), "2024-01-15 10:30:00");
        assert_eq!(ok(&ef("Z"), &ctx("+0700")), "2024-01-15 03:30:00");
        assert_eq!(ok(&ef("ZZZZ"), &ctx("GMT+07:00")), "2024-01-15 03:30:00");
    }

    /// The ISO literals `T` and `Z` are **case-insensitive** because the
    /// `java.time` constants call `parseCaseInsensitive()`; a user-written
    /// `'T'` in an `existingFormat` is **not**, because `ofPattern` does not.
    /// Measured both ways.
    #[test]
    fn the_iso_literals_are_case_insensitive_and_a_pattern_literal_is_not() {
        assert_eq!(
            ok(
                "{{ d | date('iso', timeZone='UTC') }}",
                r#"{"d": "2024-01-15t10:30:00z"}"#
            ),
            "2024-01-15T10:30:00.000000Z"
        );
        assert!(err(
            r#"{{ d | date('iso', timeZone='UTC', existingFormat="yyyy-MM-dd'T'HH:mm:ss") }}"#,
            r#"{"d": "2024-01-15t10:30:00"}"#
        )
        .contains("could not be parsed"));
    }

    // -- W2 Task 6's fourteen `compare: "error-class"` entries -------------
    //
    // Same standard as the blocks above: the oracle's error-class mode is
    // satisfied by any failure at all, so what actually carries these entries
    // is the message asserted here.

    /// **C2: check every name you register against what minijinja already
    /// binds.** Task 4 found `string` colliding invisibly — the built-in
    /// resolved, ran, and answered minijinja's way, so nothing failed as
    /// "unknown filter" and only the oracle disagreed.
    ///
    /// This asks the minijinja in the LOCK FILE rather than trusting a list
    /// in a comment: it builds a bare `Environment` and renders through each
    /// of W2 Task 6's seven names. A name minijinja does not bind fails with
    /// its own "unknown filter" wording; a name it DOES bind renders or fails
    /// some other way, and this test says which one.
    ///
    /// `indent` and `string` are included as the positive controls, because a
    /// test that only ever sees "unknown filter" cannot tell "nothing
    /// collides" from "this check is broken".
    #[test]
    fn task_sixs_seven_names_are_not_minijinja_built_ins() {
        fn is_unknown_to_minijinja(name: &str) -> bool {
            let env = minijinja::Environment::new();
            match env.render_str(&format!("{{{{ 'x' | {name} }}}}"), ()) {
                Ok(_) => false,
                Err(e) => e.to_string().contains("unknown filter"),
            }
        }

        for name in [
            "jq",
            "toIon",
            "toJson",
            "yaml",
            "regexMatch",
            "regexReplace",
            "regexExtract",
        ] {
            assert!(
                is_unknown_to_minijinja(name),
                "{name} is already bound by minijinja -- registering it is an OVERRIDE, \
                 and this module's doc has to say so"
            );
        }

        // The two that ARE overrides, so the check above is known to be able
        // to answer `false`.
        assert!(!is_unknown_to_minijinja("indent"));
        assert!(!is_unknown_to_minijinja("string"));
    }

    /// `JqFilter` catches `Exception` around its whole body and rebuilds the
    /// message from the INPUT, so a bad query, a runtime error and an
    /// unparseable String input all come back saying the same thing. The
    /// value and the class name in it are Java's spellings — a `LinkedHashMap`
    /// prints as `{a=1}`, not as `{"a": 1}`, which is why this goes through
    /// `kestra_to_string` and not through `Display`.
    #[test]
    fn jq_reports_the_input_it_could_not_use() {
        assert_eq!(
            err("{{ s | jq('.') }}", r#"{"s": "abc"}"#),
            "invalid operation: Unable to parse jq value 'abc' with type 'java.lang.String' \
             (in <string>:1)"
        );
        assert_eq!(
            err("{{ m | jq('.a[') }}", r#"{"m": {"a": 1}}"#),
            "invalid operation: Unable to parse jq value '{a=1}' with type \
             'java.util.LinkedHashMap' (in <string>:1)"
        );
        assert_eq!(
            err("{{ m | jq() }}", r#"{"m": {"a": 1}}"#),
            "invalid operation: The 'jq' filter expects an argument 'expression'. \
             (in <string>:1)"
        );
        assert_eq!(
            err("{{ m | jq(e) }}", r#"{"m": {"a": 1}, "e": 1}"#),
            "invalid operation: class java.lang.Integer cannot be cast to class \
             java.lang.String (in <string>:1)"
        );
    }

    /// Kestra's own wording for a `PatternSyntaxException`
    /// (`Invalid regex ''{0}'': {1}`), used only where **Java also rejects the
    /// pattern**. A construct Java accepts and egret refuses says
    /// `Unsupported regex` instead, and those entries are baselined rather
    /// than error-class — see [`a_refused_construct_says_so_rather_than_
    /// claiming_the_pattern_is_invalid`].
    #[test]
    fn an_invalid_java_pattern_is_reported_as_invalid() {
        assert!(err("{{ s | regexMatch(regex='(a') }}", r#"{"s": "a"}"#)
            .starts_with("invalid operation: Invalid regex '(a': "));
        // A bare script name compiles in the `regex` crate and is
        // `Unknown character property name {Greek}` in Java. egret refuses it
        // so the two engines agree the pattern is invalid.
        assert!(
            err(r"{{ s | regexMatch(regex='\p{Greek}') }}", r#"{"s": "α"}"#)
                .contains("Unknown character property name {Greek}")
        );
    }

    /// `MessageFormat.format("The argument ''{0}'' is required.", name)` --
    /// note the doubled quotes in the pattern collapse to single ones.
    /// Reached both by an ABSENT argument and by an explicitly null one,
    /// because Kestra's guard is `args.get(name) == null` and Pebble puts a
    /// null in the map for a supplied-but-null argument.
    #[test]
    fn a_missing_regex_argument_is_reported_by_name() {
        assert_eq!(
            err("{{ s | regexMatch() }}", r#"{"s": "abc"}"#),
            "invalid operation: The argument 'regex' is required. (in <string>:1)"
        );
        assert_eq!(
            err(
                "{{ s | regexMatch(regex=r) }}",
                r#"{"s": "abc", "r": null}"#
            ),
            "invalid operation: The argument 'regex' is required. (in <string>:1)"
        );
        assert_eq!(
            err("{{ s | regexReplace(regex='a') }}", r#"{"s": "a"}"#),
            "invalid operation: The argument 'replacement' is required. (in <string>:1)"
        );
    }

    /// Java raises these from `Matcher.appendExpandedReplacement`, not from
    /// the pattern, and they are two different exception types with two
    /// different sentences.
    #[test]
    fn a_bad_replacement_reference_is_reported() {
        assert_eq!(
            err(
                "{{ s | regexReplace(regex='(a)', replacement='$2') }}",
                r#"{"s": "a"}"#
            ),
            "invalid operation: No group 2 (in <string>:1)"
        );
        assert_eq!(
            err(
                "{{ s | regexReplace(regex='a', replacement='x$') }}",
                r#"{"s": "a"}"#
            ),
            "invalid operation: Illegal group reference: group index is missing (in <string>:1)"
        );
    }

    /// `RegexExtractFilter` checks the group index twice — once for
    /// negativity before compiling, once against `groupCount` after finding —
    /// with two different sentences, and casts a non-`Number` argument.
    #[test]
    fn a_bad_group_argument_is_reported() {
        assert_eq!(
            err(
                r"{{ s | regexExtract(regex='(\d)', group=2) }}",
                r#"{"s": "a1"}"#
            ),
            "invalid operation: Group index 2 is out of bounds: the pattern has only 1 capture \
             group(s). (in <string>:1)"
        );
        assert_eq!(
            err(
                r"{{ s | regexExtract(regex='(\d)', group=-1) }}",
                r#"{"s": "a1"}"#
            ),
            "invalid operation: Group index -1 is out of bounds: must be >= 0. (in <string>:1)"
        );
        assert_eq!(
            err(
                r"{{ s | regexExtract(regex='(\d)', group='1') }}",
                r#"{"s": "a1"}"#
            ),
            "invalid operation: class java.lang.String cannot be cast to class \
             java.lang.Number (in <string>:1)"
        );
    }

    /// Patterns **Java itself refuses** and the `regex` crate accepts.
    ///
    /// This is the silent class in the other direction, and it is the one a
    /// port is least likely to look for: a flow that works in egret and fails
    /// in Kestra. All six were found by the same 357-probe sweep that found
    /// the nine silent ones, and all six answered here before it.
    #[test]
    fn a_pattern_java_rejects_is_rejected_here_too() {
        for (pattern, subject) in [
            // A quantifier is not something a quantifier can apply to.
            ("a**", "aaa"),
            // A `{` is NEVER a literal in Java; the crate reads all three of
            // these as literal braces.
            ("a{", "a{"),
            ("a{,3}", "a{,3}"),
            ("{", "{"),
            // An intersection needs an operand on at least one side.
            ("[&&]", "a"),
            // Under `(?x)` the `#` starts a comment that eats the `]`.
            ("(?x)[a#b]", "a"),
            // Java's group names are Latin letters and digits.
            ("(?<\u{3bb}>a)", "a"),
        ] {
            let message = err(
                "{{ s | regexMatch(regex=p) }}",
                &format!(
                    "{{\"s\": {}, \"p\": {}}}",
                    serde_json_string(subject),
                    serde_json_string(pattern)
                ),
            );
            assert!(
                message.contains("Invalid regex"),
                "{pattern:?}: Java refuses this and egret answered {message:?}"
            );
        }
    }

    /// A construct Kestra ACCEPTS and egret refuses must say so in its own
    /// words. These are the entries in `baseline.txt`, not in
    /// `error-class.txt`: Kestra succeeds, so there is no error class to
    /// compare against, and the only thing standing between an author and a
    /// silently different answer is that egret refuses out loud.
    #[test]
    fn a_refused_construct_says_so_rather_than_claiming_the_pattern_is_invalid() {
        for (template, needle) in [
            ("{{ s | regexMatch(regex='a(?=b)') }}", "lookahead"),
            ("{{ s | regexMatch(regex='(?<=a)b') }}", "lookbehind"),
            // NOT a string literal: minijinja's lexer reads `\1` as the
            // character U+0001, where Pebble passes it through. See
            // `filter-regexmatch-numeric-escape-in-a-string-literal`.
            ("{{ s | regexMatch(regex=p) }}", "backreference"),
            (
                "{{ s | regexMatch(regex='a++a') }}",
                "possessive quantifier",
            ),
            ("{{ s | regexMatch(regex='(?>a+)b') }}", "atomic group"),
            (
                r"{{ s | regexMatch(regex='\p{InGreek}') }}",
                "Unicode BLOCK",
            ),
            (r"{{ s | regexMatch(regex='\Ga') }}", "'\\G'"),
        ] {
            let message = err(template, r#"{"s": "ab", "p": "(a)\\1"}"#);
            assert!(
                message.contains("Unsupported regex"),
                "{template}: {message:?}"
            );
            assert!(message.contains(needle), "{template}: {message:?}");
        }
        // The one that needs the subject as well as the pattern.
        let message = err("{{ s | regexMatch(regex='c$') }}", r#"{"s": "abc\n"}"#);
        assert!(message.contains("line terminator"), "{message:?}");
    }

    /// `jaq`'s standard library registers `env`, which is the WHOLE process
    /// environment. Kestra's jq has no such function — measured through the
    /// harness — so withholding it is fidelity, and it is also the difference
    /// between a flow author reading a database URL out of a template and
    /// not.
    ///
    /// This asserts the withholding, not merely that something failed: the
    /// message is jq's own "unknown function", which is what a jq program
    /// naming a filter that does not exist gets.
    #[test]
    fn jq_cannot_read_the_process_environment() {
        std::env::set_var("W2_TASK_6_CANARY", "must-not-be-readable");
        let message = err("{{ m | jq('env') }}", r#"{"m": {"a": 1}}"#);
        assert!(!message.contains("must-not-be-readable"), "{message:?}");
        // `$ENV` is a variable, and this renderer binds none.
        let message = err("{{ m | jq('$ENV') }}", r#"{"m": {"a": 1}}"#);
        assert!(!message.contains("must-not-be-readable"), "{message:?}");
        std::env::remove_var("W2_TASK_6_CANARY");
    }

    /// `jaq_core::unwrap_valr` — the function the crate's own doc example
    /// calls — ends in `std::process::exit` for a `halt`. A rendered
    /// `Property` is author-supplied source, so reaching that would let a
    /// flow kill the process.
    ///
    /// The test can only observe that this call RETURNS, which is exactly the
    /// claim: a process that exited would take the test binary with it and
    /// the whole suite would report a signal rather than a failure.
    #[test]
    fn jq_halt_does_not_terminate_the_process() {
        assert!(render("{{ m | jq('halt') }}", r#"{"m": {"a": 1}}"#).is_err());
        assert!(render("{{ m | jq('halt_error') }}", r#"{"m": {"a": 1}}"#).is_err());
    }

    /// A jq program that never terminates is refused, **and the refusal says
    /// which bound fired**.
    ///
    /// The previous version of this test asserted only `is_err()`, which
    /// passed the implementation that shipped: `[repeat(1)]` never emits an
    /// output, so the byte counter it was supposed to exercise was never
    /// touched, and an `is_err()` cannot tell "the bound fired" from "`repeat`
    /// is not registered". Every case below names the bound in its message,
    /// so a bound that stops firing fails here.
    ///
    /// The first four are the programs a review found walking past the byte
    /// counter unbounded; the fifth is the parser, which aborts rather than
    /// unwinds and so cannot be caught at all once it starts.
    #[test]
    fn a_nonterminating_jq_program_is_refused_by_a_named_bound() {
        for (program, needle) in [
            ("[repeat(1)]", "repeat exceeded"),
            ("reduce repeat(1) as $x (0; .+1)", "repeat exceeded"),
            ("until(false; .)", "until exceeded"),
            ("[while(true; .)]", "while exceeded"),
            ("[recurse(.)]", "recurse exceeded"),
            ("[range(1e12)]", "range exceeded"),
            ("[range(0;1e12;1)]", "range exceeded"),
        ] {
            let message = err(
                "{{ m | jq(e) }}",
                &format!(
                    "{{\"m\": {{\"a\": 1}}, \"e\": {}}}",
                    serde_json_string(program)
                ),
            );
            assert!(
                message.contains(needle),
                "{program}: {message:?} does not name the bound"
            );
        }
    }

    /// A jq source deep enough to overflow jaq's recursive-descent parser is
    /// refused **before** the parser runs.
    ///
    /// This one cannot be written as "assert it errors": a stack overflow is
    /// `SIGABRT`, so a run that reaches the parser takes the whole test binary
    /// with it and reports a signal rather than a failure. The assertion that
    /// this call RETURNS is the claim.
    #[test]
    fn a_deeply_nested_jq_source_is_refused_before_the_parser_sees_it() {
        for (open, close) in [('[', ']'), ('(', ')')] {
            let program: String = std::iter::repeat_n(open, 2000)
                .chain(std::iter::once('1'))
                .chain(std::iter::repeat_n(close, 2000))
                .collect();
            let message = err(
                "{{ m | jq(e) }}",
                &format!(
                    "{{\"m\": {{\"a\": 1}}, \"e\": {}}}",
                    serde_json_string(&program)
                ),
            );
            assert!(message.contains("nests more than"), "{message:?}");
        }
    }

    /// The six natives that read or write the host, each refused by name.
    ///
    /// `jq_cannot_read_the_process_environment` covers `env` with a canary;
    /// this covers the shape for all six, and asserts the two `debug`/`stderr`
    /// ones in particular, since those write into the service log and
    /// `egret-bin` installs `LogTracer`.
    #[test]
    fn the_host_reading_jq_natives_are_refused_by_name() {
        for (program, needle) in [
            ("debug", "'debug' is not available"),
            ("stderr", "'stderr' is not available"),
            ("halt", "'halt' is not available"),
            ("halt_error", "'halt_error' is not available"),
        ] {
            let message = err(
                "{{ m | jq(e) }}",
                &format!(
                    "{{\"m\": {{\"a\": 1}}, \"e\": {}}}",
                    serde_json_string(program)
                ),
            );
            assert!(message.contains(needle), "{program}: {message:?}");
        }
        // These three have no definition referencing them, so dropping the
        // native is enough and jq reports its own "undefined filter".
        for program in ["env", "localtime", "0 | strflocaltime(\"%Z\")"] {
            assert!(render(
                "{{ m | jq(e) }}",
                &format!(
                    "{{\"m\": {{\"a\": 1}}, \"e\": {}}}",
                    serde_json_string(program)
                ),
            )
            .is_err());
        }
    }

    /// A jq string, for the tests above that build a context by hand.
    fn serde_json_string(s: &str) -> String {
        serde_json::to_string(s).expect("a &str always serialises")
    }

    /// The three filters that answer a null input with something other than
    /// null, which is where a "they all return null" reading goes wrong.
    #[test]
    fn the_null_input_branches_are_not_all_the_same() {
        // `"null"`, the four-character string.
        assert_eq!(ok("<{{ n | toJson }}>", r#"{"n": null}"#), "<null>");
        assert_eq!(ok("<{{ n | toIon }}>", r#"{"n": null}"#), "<null>");
        assert_eq!(ok("<{{ n | yaml }}>", r#"{"n": null}"#), "<null>");
        // `false`, not null.
        assert_eq!(
            ok("{{ n | regexMatch(regex='a') }}", r#"{"n": null}"#),
            "false"
        );
        // Java null, which renders as nothing.
        assert_eq!(ok("<{{ n | jq('.') }}>", r#"{"n": null}"#), "<>");
        assert_eq!(
            ok("<{{ n | regexExtract(regex='a') }}>", r#"{"n": null}"#),
            "<>"
        );
        assert_eq!(
            ok(
                "<{{ n | regexReplace(regex='a', replacement='b') }}>",
                r#"{"n": null}"#
            ),
            "<>"
        );
    }

    /// The `timestamp*` filters declare `[timeZone, existingFormat]` and
    /// nothing else — **no `format` and no `locale`** — so a positional
    /// argument means `timeZone`, not a format.
    #[test]
    fn a_timestamp_filters_first_positional_argument_is_its_zone() {
        assert_eq!(
            ok(
                "{{ d | timestamp('Asia/Tokyo') }}",
                r#"{"d": "2024-01-15T10:30:00"}"#
            ),
            "1705282200"
        );
        assert_eq!(
            ok(
                "{{ d | timestamp('UTC') }}",
                r#"{"d": "2024-01-15T10:30:00"}"#
            ),
            "1705314600"
        );
    }
}
