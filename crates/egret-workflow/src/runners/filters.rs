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
//! Package #5b Task B4 added Pebble core's ten that Kestra does not register
//! itself but its engine binds (`abbreviate`, `base64decode`, `base64encode`,
//! `merge`, `nl2br`, `numberformat`, `raw`, `rsort`, `sha256`, `urlencode`);
//! `numberformat`'s `DecimalFormat` is [`super::decimalformat`].
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

use minijinja::value::{
    DynObject, Enumerator, Object, ObjectRepr, Rest, Value as MjValue, ValueKind,
};
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
    // W2 Task 8: OVERRIDES minijinja's own `replace`, which takes `(old, new)`
    // where Pebble's takes one map. The fourth name in this crate that
    // resolved to minijinja's answer instead of erroring as unknown -- see
    // [`replace`].
    env.add_filter("replace", replace);
    // W2 Task 8 fix round 1: ten more names minijinja and Pebble BOTH bind,
    // where egret was answering minijinja's question. See the section header
    // above [`pebble_string_cast`].
    env.add_filter("abs", abs);
    env.add_filter("capitalize", capitalize);
    env.add_filter("default", default);
    env.add_filter("escape", escape);
    env.add_filter("first", first);
    env.add_filter("format", format);
    env.add_filter("join", join);
    env.add_filter("last", last);
    env.add_filter("length", length);
    env.add_filter("lower", lower);
    env.add_filter("reverse", reverse);
    env.add_filter("slice", slice);
    env.add_filter("sort", sort);
    env.add_filter("split", split);
    env.add_filter("title", title);
    env.add_filter("trim", trim);
    env.add_filter("upper", upper);
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
    // Package #5b Task B4 (spec D13): Pebble core's ten, which Kestra does
    // not register itself and which minijinja does not bind under this
    // workspace's features (its `urlencode` is off).
    env.add_filter("abbreviate", abbreviate);
    env.add_filter("base64decode", base64decode);
    env.add_filter("base64encode", base64encode);
    env.add_filter("merge", merge);
    env.add_filter("nl2br", nl2br);
    env.add_filter("numberformat", numberformat);
    env.add_filter("raw", raw);
    env.add_filter("rsort", rsort);
    env.add_filter("sha256", sha256);
    env.add_filter("urlencode", urlencode);
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
pub(super) struct Args {
    positional: Vec<MjValue>,
    named: Vec<(String, MjValue)>,
}

impl Args {
    /// Splits minijinja's trailing kwargs map off the positional arguments.
    ///
    /// minijinja marks the kwargs map with [`MjValue::is_kwargs`] and passes
    /// it as the last value, so this never has to guess whether a final map
    /// argument was a map or a set of keywords.
    pub(super) fn new(rest: Rest<MjValue>) -> Self {
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
    /// The positional arguments alone, in order.
    ///
    /// `FormatFilter` declares **no** argument names, so Pebble stores every
    /// argument under `"0"`, `"1"`, … and `String.format`'s varargs are those
    /// in order. Nothing else in this module needs them that way.
    pub(super) fn positional_only(&self) -> Vec<MjValue> {
        self.positional.clone()
    }

    /// The names passed as keywords, in call order. For a function that
    /// refuses some of its declared names **by name** (W5b's `secret`).
    pub(super) fn named_keys(&self) -> impl Iterator<Item = &str> {
        self.named.iter().map(|(key, _)| key.as_str())
    }

    pub(super) fn get(&self, name: &str, index: usize) -> Option<MjValue> {
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
pub(super) fn pebble_error(message: impl Into<String>) -> MjError {
    MjError::new(ErrorKind::InvalidOperation, message.into())
}

/// Is this value Kestra's `null` at a filter's `input == null` branch?
///
/// **`is_none()` alone, and an undefined is deliberately NOT one.** This used
/// to be `value.is_none() || value.is_undefined()`, and the `||` was not
/// laziness: Pebble's `null` is a real literal, minijinja had no such name, so
/// `{{ null | distinct }}` arrived here as `ValueKind::Undefined` and folding
/// it in was the only way to answer the literal at all. The cost was written
/// down at the time — egret could not tell the literal from a **typo'd
/// variable** — and W2 Task 8 measured what that cost: applied at the entry
/// point of a filter that used to refuse an undefined, it turned
/// `{{ nothere | upper }}` into `""`, which is the one divergence this crate's
/// rules do not allow. Task 8 put [`strict_undefined`] in front of sixteen
/// filters, and the sixteen renderable `{{ null | F }}` cases broke instead.
///
/// W2b removed the reason for the choice rather than picking a side of it:
/// [`super::rewrite::tokenize`] now translates a bare `null`/`NULL`/`none`/
/// `NONE` to minijinja's `none`, so **the literal is a `None` and a typo is
/// still an `Undefined`**, and this predicate can ask the narrow question. The
/// pair that proves it are in the same test run: sixteen `filter-*-literal-null`
/// corpus entries render, and
/// [`tests::the_ported_filters_refuse_an_undefined_input`] holds seventeen
/// filters x {missing root, missing attribute} refusing.
///
/// `filter-distinct-literal-null` is the sharp one either way: its golden is
/// `<null>` because `distinct` alone returns the *string* `"null"`, which no
/// route that treats the input as "not null" can produce.
///
/// `super::env::kestra_is_in` is deliberately different: it is a *function*,
/// its operands are not filter inputs, and returning the undefined unchanged
/// is what keeps a typo inside an `isIn` from silently deciding a branch.
pub(super) fn is_java_null(value: &MjValue) -> bool {
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
pub(super) fn java_class_name(value: &MjValue) -> String {
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

/// **What a filter's result IS to the NEXT filter**, which is not the same
/// question as what it contains.
///
/// Kestra's collection filters return whatever Java type their implementation
/// happens to produce, and the next filter in the chain *casts*. So
/// `{{ m | keys | sort }}` throws on Kestra and answered here, and no
/// measurement of `keys` or of `sort` alone can see it — the defect is in the
/// type flowing through the pipe. W2 Task 8 recorded fourteen such pairs by
/// hand; W2a enumerated all 63 engine filters through Kestra's own
/// `className` filter (`oracle/return-classes.txt`) and crossed the result
/// (`oracle/collection-pairs.txt`), which found **38** divergent pairs and two
/// consumers the hand-written list had missed (`keys` and, through the
/// re-render loop, `regexReplace`).
///
/// This is deliberately **four capabilities, not a class name**. The engine
/// has at least nine distinct classes here (`ArrayList`, `ArrayList$SubList`,
/// `Arrays$ArrayList`, `AbstractList$RandomAccessSubList`,
/// `ImmutableCollections$ListN`, `ImmutableCollections$SubList`,
/// `Lists$RandomAccessPartition`, `LinkedHashMap$LinkedKeySet`,
/// `LinkedHashMap$LinkedValues`) and every consumer's behaviour is decided by
/// three questions about them: is it a `List`, does `set` work, is it an
/// array. Carrying the names instead would be `oracle/README.md`'s
/// copy-structure line, and it would still be wrong on the map side
/// (`HashMap` vs `LinkedHashMap`), which is a different recorded class.
/// `className` therefore still answers `java.util.ArrayList` for all of them
/// and stays a recorded divergence, like `filter-classname-of-long`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JavaSeqKind {
    /// `java.util.ArrayList` and its mutable views (`ArrayList$SubList`,
    /// `Arrays$ArrayList`). Every consumer accepts it; this is what a context
    /// list, `sort`, `distinct`, `reverse`, `jq` and `escapeChar` produce, and
    /// it is the default for any sequence carrying no tag.
    MutableList,
    /// `java.util.ImmutableCollections` — what `Stream.toList()` returns, so
    /// `flatten` and `keys` of a list or array, and `slice` of either
    /// (`ImmutableCollections$SubList`).
    ///
    /// `AbstractImmutableList` **overrides `sort` to throw unconditionally**,
    /// so `{{ [] | flatten | sort }}` throws on an EMPTY list — measured, and
    /// the reason this is not one kind with [`Self::Partition`].
    ImmutableList,
    /// Guava's `Lists.partition`, which is `chunk`'s result (and `slice` of
    /// one, `AbstractList$RandomAccessSubList`).
    ///
    /// It does **not** override `sort`, so `List.sort`'s default runs: it
    /// copies to an array, sorts, then writes back with `ListIterator.set`,
    /// one call per element. Zero elements means zero calls, so
    /// `{{ [] | chunk(size=2) | sort }}` is `[]` where the
    /// `ImmutableCollections` line above throws. Both measured.
    Partition,
    /// `Map.keySet()` / `Map.values()` — a `Collection` that is **not a
    /// `List`**, so every consumer that casts to `List` refuses it.
    Collection,
    /// A Java array (`SplitFilter` returns `String[]`). Not a `Collection` at
    /// all: `sort`, `slice` and `keys` have explicit array branches and the
    /// other four do not.
    Array,
}

impl JavaSeqKind {
    /// Whether Java's `instanceof List` is true, which is the test
    /// `distinct`, `chunk`, `flatten` and `reverse` make.
    fn is_list(self) -> bool {
        matches!(
            self,
            Self::MutableList | Self::ImmutableList | Self::Partition
        )
    }

    /// Whether `Collections.sort` — i.e. `List.sort(null)` — throws.
    ///
    /// **The length is a parameter and that is measured, not tidiness.** A
    /// first cut refused every immutable list at every length and turned two
    /// empty-list expressions Kestra answers into errors, which the cross
    /// caught by name.
    fn sort_in_place_throws(self, len: usize) -> bool {
        match self {
            Self::ImmutableList => true,
            Self::Partition => len >= 1,
            Self::MutableList | Self::Collection | Self::Array => false,
        }
    }

    /// Whether `Collections.reverse` throws.
    ///
    /// It swaps `size >> 1` pairs, so a list of 0 or 1 elements calls `set`
    /// **no times** and an immutable one comes back unchanged rather than
    /// throwing: `{{ [x] | keys | reverse }}` is `[0]`, measured.
    fn reverse_in_place_throws(self, len: usize) -> bool {
        matches!(self, Self::ImmutableList | Self::Partition) && len >= 2
    }

    /// What egret's own refusal calls it. **Not a Java class name**: see the
    /// type's doc for why the names are deliberately not carried.
    fn description(self) -> &'static str {
        match self {
            Self::MutableList => "a list",
            Self::ImmutableList | Self::Partition => "an immutable list",
            Self::Collection => "a map's key or value view, which is not a List",
            Self::Array => "an array, which is not a List",
        }
    }
}

/// A sequence that remembers which [`JavaSeqKind`] it is.
///
/// `ObjectRepr::Seq`, so `ValueKind::Seq`, so every existing reader —
/// [`as_list`], `try_iter`, `env::write_json`, `env::kestra_to_string`, the
/// YAML and Ion writers, `jq`'s input conversion — sees exactly what it saw
/// before. Only the seven consumers that cast look at the tag.
#[derive(Debug)]
struct JavaSeq {
    items: Vec<MjValue>,
    kind: JavaSeqKind,
}

impl Object for JavaSeq {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Seq
    }

    /// The SUBSCRIPT operator, which is a fifth thing the kind decides and the
    /// one thing here that is not a filter.
    ///
    /// Pebble resolves `x[0]` through `GetAttributeExpression`, which handles a
    /// `List` and an array and has no `Collection` branch, so
    /// `{{ m | keys | first }}` is `b` and `{{ (m | keys)[0] }}` is
    /// `AttributeNotFoundException: Attribute [0] of
    /// [java.util.LinkedHashMap$LinkedKeySet] does not exist` — measured over
    /// all 46 producers x five ways of reading each, and among non-empty
    /// SEQUENCE producers the only ones whose `[0]` refuses are the
    /// `Collection` kind -- `keys` and `values` of a map, at both lengths.
    /// (Every *empty* sequence refuses too, on both engines, and that is an
    /// index out of range rather than a capability — which is why the
    /// predicate is the kind and not the length.)
    fn get_value(self: &Arc<Self>, key: &MjValue) -> Option<MjValue> {
        if self.kind == JavaSeqKind::Collection {
            return None;
        }
        let index = usize::try_from(i64::try_from(key.clone()).ok()?).ok()?;
        self.items.get(index).cloned()
    }

    /// `Enumerator::Seq` drives minijinja's own iteration *through*
    /// [`Self::get_value`], so a `Collection` — which must iterate but must not
    /// subscript — has to yield its values directly instead. `size_hint` on a
    /// `vec::IntoIter` is exact, so `length`, `default`'s emptiness test and
    /// `{% for %}` all keep working; measured against Kestra, where a key view
    /// answers `length`, `first` and a for-loop and refuses only `[0]`.
    fn enumerate(self: &Arc<Self>) -> Enumerator {
        if self.kind == JavaSeqKind::Collection {
            return Enumerator::Iter(Box::new(self.items.clone().into_iter()));
        }
        Enumerator::Seq(self.items.len())
    }
}

/// Builds a sequence carrying `kind`.
///
/// [`JavaSeqKind::MutableList`] is returned as a **plain** minijinja sequence
/// rather than a tagged one: it is the default that [`java_seq_kind`] reads
/// for an untagged value, so tagging it would change nothing and would put an
/// object in the way of every value the crate already produces.
fn java_seq(kind: JavaSeqKind, items: Vec<MjValue>) -> MjValue {
    if kind == JavaSeqKind::MutableList {
        return MjValue::from(items);
    }
    MjValue::from_object(JavaSeq { items, kind })
}

/// The [`JavaSeqKind`] of a value — [`JavaSeqKind::MutableList`] for anything
/// untagged, which is every list that came from the context, from `fromJson`,
/// from `range` or from a filter that builds a fresh `ArrayList`.
fn java_seq_kind(value: &MjValue) -> JavaSeqKind {
    value
        .downcast_object_ref::<JavaSeq>()
        .map(|seq| seq.kind)
        .unwrap_or(JavaSeqKind::MutableList)
}

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
    // `instanceof List` — see [`JavaSeqKind`]. A map's key or value view and a
    // `String[]` are sequences here and are not `List`s there, so
    // `{{ m | keys | chunk(size=2) }}` throws on Kestra.
    let kind = java_seq_kind(&value);
    if !kind.is_list() {
        return Err(pebble_error(format!(
            "'chunk' filter can only be applied to List. Actual type was: {}",
            kind.description()
        )));
    }
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

    // `Lists.partition` returns an immutable view whose ELEMENTS are
    // `list.subList(..)`, so each chunk inherits the input's mutability while
    // the partition itself never has any. Measured:
    // `((l | chunk(size=2)) | first) | className` is `ArrayList$SubList` and
    // `(l | chunk(size=2)) | className` is `Lists$RandomAccessPartition`.
    let chunks: Vec<MjValue> = items
        .chunks(size as usize)
        .map(|chunk| java_seq(kind, chunk.to_vec()))
        .collect();
    Ok(java_seq(JavaSeqKind::Partition, chunks))
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

// ---------------------------------------------------------------------------
// The per-RENDER `replace` budget
// ---------------------------------------------------------------------------

/// The total bytes every `replace` call in ONE render may scan, together.
///
/// 1 024 x [`super::env::MAX_OUTPUT_BYTES`] = 1 GiB, **measured at 1.48 s** on
/// the machine this was written on for a single call that exhausts it. (An
/// earlier note said ~290 ms. That was five times optimistic and the arithmetic
/// said why: each pair does *two* full passes — `matches().count()` and then
/// `replace` — plus a full-length allocation, while the budget charges
/// `out.len()` once. The number is now measured end to end rather than derived
/// from a per-scan rate.)
///
/// # Why per-render and not per-call
///
/// A per-call cap is multiplied by any loop. Measured before this change, with
/// a `{% for %}` whose per-iteration output is empty — so
/// [`super::env::MAX_OUTPUT_BYTES`] is satisfied for ever — 20 / 80 / 320
/// iterations cost 267 ms / 1.05 s / **4.42 s** and produced zero bytes.
/// `range()` caps at 1 000 000 elements, so one template reached hours.
///
/// The class (an expensive filter inside a loop, with no per-render work
/// budget) is older than this filter and still applies to `jq` and the regex
/// filters. What made it this filter's problem is that a per-call gigabyte
/// raised the legal per-iteration cost by about three orders of magnitude over
/// anything else in the module. One budget for the whole render is the shape
/// that cannot be multiplied, and `replace_is_bounded_across_a_whole_render`
/// is the test — it has a loop in it, which the first cut's sweep did not.
pub(super) const MAX_REPLACE_SCAN_BYTES: usize = 1024 * super::env::MAX_OUTPUT_BYTES;

thread_local! {
    /// What is left of [`MAX_REPLACE_SCAN_BYTES`] on this render thread.
    ///
    /// A thread-local rather than an argument because `minijinja` gives a
    /// filter no place to hang per-render state that survives across calls,
    /// and because [`super::env::render_str`] already gives every render its
    /// own thread — see `RENDER_STACK_BYTES`. [`ReplaceBudget::install`] still
    /// resets it, so a caller that renders on a thread of its own reusing it
    /// is correct too.
    static REPLACE_BUDGET: std::cell::Cell<usize> =
        const { std::cell::Cell::new(MAX_REPLACE_SCAN_BYTES) };
}

/// Resets the render-wide `replace` budget, and restores it on drop.
pub(super) struct ReplaceBudget(());

impl ReplaceBudget {
    /// Installed once per render by [`super::env::render_str`].
    pub(super) fn install() -> Self {
        REPLACE_BUDGET.with(|left| left.set(MAX_REPLACE_SCAN_BYTES));
        ReplaceBudget(())
    }

    /// Charges `bytes` against the render's budget, or refuses.
    fn charge(bytes: usize) -> Result<(), MjError> {
        REPLACE_BUDGET.with(|left| {
            let remaining = left.get();
            if bytes > remaining {
                return Err(pebble_error(format!(
                    "the 'replace' filter would scan more than {MAX_REPLACE_SCAN_BYTES} bytes \
                     in one render"
                )));
            }
            left.set(remaining - bytes);
            Ok(())
        })
    }
}

impl Drop for ReplaceBudget {
    fn drop(&mut self) {
        REPLACE_BUDGET.with(|left| left.set(MAX_REPLACE_SCAN_BYTES));
    }
}

/// `replace` — **Pebble's own built-in**, not Kestra's, and this override is
/// W2 Task 8's G4 ruling.
///
/// # Why a filter nobody assigned to a batch ends up here
///
/// Three facts, each measured rather than reasoned:
///
/// 1. `io.kestra.core.runners.pebble.filters.ReplaceFilter` **exists** in
///    `core-2.0.0-rc13.jar` — it has a `regexp` argument and a
///    `replace_pairs` one — and `Extension.getFilters()` does **not** register
///    it. `host.Oracle dump`'s 35 Kestra filter names do not include
///    `replace`, and `{{ s | replace({'a':'b'}, regexp=true) }}` answers
///    `The following named argument does not exist: regexp`, which is the
///    proof: the class the engine is running is Pebble's, whose only argument
///    is `replace_pairs`.
/// 2. so on Kestra's side `replace` is
///    `io.pebbletemplates.pebble.extension.core.ReplaceFilter`: **one**
///    argument, a `Map`, applied as a chain of literal `String.replace` calls;
/// 3. and on egret's side, before this, it was **minijinja's** `replace`,
///    which takes `(old, new)`.
///
/// The two signatures are not compatible in either direction — Kestra refuses
/// `replace('a','b')` (`The argument at position 2 is not allowed`) and
/// minijinja refuses `replace({'a':'b'})` — so leaving it was not cosmetic. It
/// is the same trap as `indent`, `string` and `range`, and it is the fourth.
///
/// # What it does
///
/// `input == null` → null. `args.get("replace_pairs") == null` → a
/// `PebbleException` (a **null check**, so passing `replace_pairs=null`
/// explicitly says the same thing as omitting it). A non-`Map` is a
/// `ClassCastException`. Then `input.toString()` — Java's, so a list is
/// `[1, 2]` and a map is `{a=1}`, which is why this goes through
/// [`super::env::kestra_to_string`] and not `to_string` — and one literal
/// `String.replace(key.toString(), value.toString())` per entry, **in the
/// map's iteration order**, each seeing the previous one's output:
/// `'ab' | replace({'a':'b','b':'c'})` is `cc`, not `bc` — but see the note
/// below on **which** order that is.
///
/// A null *value* is an NPE on its `toString()`; a null *key* cannot occur,
/// since neither JSON nor a Pebble map literal can carry one.
///
/// # The order is the map's, and for a map LITERAL the two maps disagree
///
/// An earlier draft of this doc said pairs are applied "in the map's iteration
/// order" and cited `'ab' | replace({'a':'b','b':'c'})` → `cc`. That example
/// agrees **by luck**: for the key set `{a, b}`, Java's `HashMap` bucket order
/// and `minijinja`'s key-sorted literal order coincide. In general they do
/// not, and because the pairs chain the whole value differs:
///
/// | expression | Kestra | egret |
/// |---|---|---|
/// | `'ap' \| replace({'p':'a','a':'x'})` | `xx` | `xa` |
/// | `'a' \| replace({'p':'z','a':'p'})` | `p` | `z` |
///
/// **A map that came from the CONTEXT agrees**, in both directions: Jackson
/// builds a `LinkedHashMap` and [`super::env::OrderedMap`] keeps insertion
/// order, so `replace(m)` over the same two pairs answers `xx` on both sides.
/// It is map literals only — which is, unhelpfully, the documented idiom.
///
/// This is recorded rather than fixed, under the standing ruling that Java's
/// `HashMap` bucket order is implementation *structure* (see
/// `oracle/README.md`, "What egret copies from upstream", and
/// `filter-escapechar-map-two-keys`). `filter-replace-literal-order` and
/// `-literal-order-chained` are the baselined entries, and
/// `filter-replace-context-map-order` is the one that pins the agreeing half —
/// without it, "context maps agree" would be a claim rather than a test.
///
/// An empty key is Java's `String.replace("", x)` — `'ab'` becomes `-a-b-` —
/// and Rust's `str::replace` agrees, which is measured here rather than
/// assumed (`filter-replace-empty-key`).
///
/// # One bound Kestra does not have, and the arithmetic for it
///
/// Pebble's `ReplaceFilter` is an unguarded loop of `String.replace`
/// (`javap -p -c io.pebbletemplates.pebble.extension.core.ReplaceFilter`: no
/// `RenderLimits`, no length check, nothing) and it grows **multiplicatively**
/// on author-controlled input in two independent ways:
///
/// - one pair, `'x' x 1e6 | replace({'x': <a 1 MiB context string>})`, is
///   1e6 x 1e6 = 1 TB allocated inside a single `String.replace`;
/// - and pairs **chain**, each seeing the last one's output, so
///   `{'a':'bb','b':'cc', …}` over the alphabet is `2^25` times the input from
///   a 25-entry map literal that fits in a template a hundred bytes wide.
///
/// So it is bounded here, per the C7 ruling ("a resource-exhaustion vector is
/// not a behaviour to port"), against the ceiling that already exists rather
/// than a second invented one: [`super::env::MAX_OUTPUT_BYTES`], 1 MiB, the
/// same limit `indent`'s padding is measured against. The check is the
/// **predicted** length — `len - matches x needle + matches x replacement` —
/// computed before each `replace` call, because a check afterwards is a check
/// on a string that has already been allocated.
///
/// **There is no corpus entry**: Kestra answers this, slowly and then not at
/// all, so a golden would be a hang or an OOM — the C7 case, where the rule is
/// a unit test instead. `replace_is_bounded_where_pebbles_own_loop_is_not` and
/// `replace_is_bounded_across_a_whole_render` are the pins. An earlier draft
/// of this sentence named `filter-replace-past-the-cap`, which does not exist
/// in any control file and never did.
fn replace(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    strict_undefined("replace", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let pairs = args
        .get("replace_pairs", 0)
        .unwrap_or_else(|| MjValue::from(()));
    if is_java_null(&pairs) {
        return Err(pebble_error("The argument 'replace_pairs' is required."));
    }
    if pairs.kind() != ValueKind::Map {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.util.Map",
            java_class_name(&pairs)
        )));
    }

    let mut out = super::env::kestra_to_string(&value);
    let keys: Vec<MjValue> = pairs
        .try_iter()
        .map(|keys| keys.collect())
        .unwrap_or_default();
    for key in keys {
        let Ok(replacement) = pairs.get_item(&key) else {
            continue;
        };
        if is_java_null(&replacement) {
            return Err(pebble_error(
                "Cannot invoke \"Object.toString()\" because the return value of \
                 \"java.util.Map$Entry.getValue()\" is null",
            ));
        }
        let needle = super::env::kestra_to_string(&key);
        let replacement = super::env::kestra_to_string(&replacement);
        // **Checked BEFORE the allocation, not after it.** See the note on
        // the multiplicative blow-up below: a single `String.replace` can
        // allocate `matches x replacement` bytes, so measuring `out.len()`
        // afterwards is measuring a string that has already been built.
        // The SECOND axis, and the one the growth check below cannot see:
        // the loop is `pairs x |out|`, and a shrinking pair keeps `|out|`
        // under the growth cap while the pair count stays the author's.
        // Found by attacking the growth check rather than by reasoning about
        // it -- 20 000 pairs over a 500 KB subject is 2 825 ms, and a context
        // map is not bounded at 20 000. Charged against the RENDER's budget,
        // not this call's, because a `{% for %}` multiplies a per-call one.
        ReplaceBudget::charge(out.len())?;
        let matches = out.matches(&needle).count();
        let grown = out
            .len()
            .saturating_sub(matches.saturating_mul(needle.len()))
            .saturating_add(matches.saturating_mul(replacement.len()));
        if grown > super::env::MAX_OUTPUT_BYTES {
            return Err(pebble_error(format!(
                "the 'replace' filter would produce {grown} bytes, over egret's \
                 {} byte limit",
                super::env::MAX_OUTPUT_BYTES
            )));
        }
        out = out.replace(&needle, &replacement);
    }
    Ok(MjValue::from(out))
}

// ---------------------------------------------------------------------------
// W2 Task 8, fix round 1 — Pebble's OWN core filters
// ---------------------------------------------------------------------------
//
// **Sixteen names both engines bind, where egret was running minijinja's
// semantics and nobody had compared them.** W2's filter batches were Kestra's
// 35 *registered* filters; Pebble's own 28 were never in a batch, and the
// seventeen names minijinja happens to share with them were therefore
// answering minijinja's question. 104 differential probes: 43 agreed, 9 failed
// on both sides, **52 diverged**. `abs` is the only one of the seventeen that
// was clean.
//
// That is the `replace` class, sixteen more times, and three of them are as
// bad as `replace` was:
//
// | filter | Kestra | egret, before |
// |---|---|---|
// | `'a.b.c' \| split('.')` | `[]` — the delimiter is a **regex** | `["a","b","c"]` |
// | `[3,1,2] \| slice(0,2)` | `[3,1]` — `(fromIndex, toIndex)` | an error; `slice(1)` was `[[3,1,2]]` |
// | `null \| default('D')` | `D` — null, blank and empty count | empty |
//
// and the Python-spelling defect this crate has now met four times was back
// through the built-ins, because `upper`/`lower`/`capitalize`/`title`/`trim`/
// `escape` stringify with `Display` rather than [`super::env::kestra_to_string`]:
// `{{ nothing | upper }}` rendered `NONE`, `lower` rendered `none`, and the
// other four rendered `None`.
//
// What is ported here is what the fix round was scoped to: `split`, `slice`,
// `default`, `capitalize`, `title`, `trim`, `length`, `upper`, `lower` and
// `escape`'s null/stringify branch. `first`, `last`, `join`, `sort`,
// `reverse`, `format` and `escape`'s three non-HTML strategies are **measured
// and recorded** instead — in `oracle/name-surface.txt`'s `semantics` column,
// which names a baselined corpus id per row, so the record is in the ratchet
// rather than in prose.

/// The guard every filter W2 Task 8's fix round ported runs **first**, before
/// its `input == null` branch.
///
/// # Why this is not [`is_java_null`]
///
/// `is_java_null` folds `Undefined` into "Kestra's null", and that is right
/// for the filters Task 4 wrote: Pebble's `null` is a real literal, minijinja
/// has no such name, and six corpus entries pin what Kestra answers for it.
/// The cost is written down in that function's doc — egret cannot tell the
/// literal from a **typo'd variable**.
///
/// Applying it at the *entry point* of a filter that previously refused an
/// undefined turns that cost into a **regression**, and the fix round's first
/// cut did exactly that to sixteen names at once:
///
/// | expression | Kestra | pre-port | the first cut |
/// |---|---|---|---|
/// | `{{ nothere \| upper }}` | `RootAttributeNotFoundException` | an error | `""` |
/// | `{{ obj.missing \| length }}` | `AttributeNotFoundException` | an error | `"0"` |
///
/// 28 measured regressions over 14 names x {missing root, missing attribute}.
/// In a flow that turns a typo'd variable from a hard failure into an empty
/// string, and **converting a refusal into an answer is the one divergence
/// this crate's rules do not allow** — the rule the same round cited to
/// justify sixteen ports.
///
/// So these filters refuse an undefined, exactly as the minijinja built-ins
/// they replaced did. What that costs, stated rather than hidden:
/// `{{ null \| upper }}` — the **literal** — renders empty on Kestra and is an
/// error here, because the literal and the typo arrive identically. That is
/// the same trade `filter-string-undefined-input` already records, and it is
/// the direction that fails loudly. `filter-upper-literal-null` is the entry.
///
/// The two exceptions are deliberate: `default` and `abs` are reached through
/// this too, and Kestra raises for an undefined input to *both*.
fn strict_undefined(filter: &str, value: &MjValue) -> Result<(), MjError> {
    if value.kind() == ValueKind::Undefined {
        return Err(pebble_error(format!(
            "an undefined value reached the '{filter}' filter: Kestra renders under \
             strictVariables(true), where a missing root attribute or a missing \
             attribute raises rather than becoming null"
        )));
    }
    Ok(())
}

/// Pebble's `(String) input` cast, which tolerates a Java `null`.
///
/// `checkcast` lets a `null` through, so `null | capitalize` is `null` and
/// `42 | capitalize` is a `ClassCastException`. Five of these filters share
/// exactly that shape.
fn pebble_string_cast(value: &MjValue) -> Result<Option<String>, MjError> {
    if is_java_null(value) {
        return Ok(None);
    }
    match value.as_str() {
        Some(text) => Ok(Some(text.to_string())),
        None => Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(value)
        ))),
    }
}

/// `UpperFilter` — `String.toUpperCase(locale)`, and **`toString()` first for
/// anything that is not a String**.
///
/// That second branch is the whole reason this overrides minijinja's: a null
/// rendered `NONE` and a map rendered `{"A": 1}` where Kestra writes `{A=1}`,
/// because minijinja's stringifies with `Display` and Java's with
/// `Object::toString`. [`super::env::kestra_to_string`] is the only correct
/// route and this crate has now learned that four times.
fn upper(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("upper", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    Ok(MjValue::from(kestra_to_string(&value).to_uppercase()))
}

/// `LowerFilter` — the mirror of [`upper`].
fn lower(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("lower", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    Ok(MjValue::from(kestra_to_string(&value).to_lowercase()))
}

/// `CapitalizeFilter` — title-case the first non-whitespace character and
/// **leave the rest of the string exactly as it is**.
///
/// minijinja's lowercases the tail, which is Python's rule and not Pebble's:
/// `'hello WORLD' | capitalize` is `Hello WORLD` there and was `Hello world`
/// here. Leading whitespace is preserved and skipped over, so
/// `'  hi there'` is `'  Hi there'`.
///
/// A non-String input is a `ClassCastException`, unlike [`upper`], which
/// stringifies — the two neighbouring filters really do differ on that.
fn capitalize(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("capitalize", &value)?;
    let Some(text) = pebble_string_cast(&value)? else {
        return Ok(MjValue::from(()));
    };
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices();
    for (at, c) in chars.by_ref() {
        if java_is_whitespace(c) {
            out.push(c);
            continue;
        }
        // `Character.toTitleCase`, then the remainder untouched.
        out.push(java_title_case(c));
        out.push_str(&text[at + c.len_utf8()..]);
        break;
    }
    Ok(MjValue::from(out))
}

/// `TitleFilter` — title-case the first character of every whitespace-delimited
/// word, and leave every other character alone.
///
/// Again not minijinja's, which lowercases the rest of each word and treats
/// punctuation as a boundary: `"a-b c'd" | title` is `A-b C'd` on Kestra and
/// was `A-B C'D` here. Only `Character.isWhitespace` delimits, so `'1a b'` is
/// `'1a B'` — the `1` is a word start and title-casing it changes nothing.
fn title(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("title", &value)?;
    let Some(text) = pebble_string_cast(&value)? else {
        return Ok(MjValue::from(()));
    };
    let mut out = String::with_capacity(text.len());
    let mut at_word_start = true;
    for c in text.chars() {
        if java_is_whitespace(c) {
            out.push(c);
            at_word_start = true;
        } else if at_word_start {
            out.push(java_title_case(c));
            at_word_start = false;
        } else {
            out.push(c);
        }
    }
    Ok(MjValue::from(out))
}

/// `Character.isWhitespace`, which is **not** Rust's `char::is_whitespace`.
///
/// The two are opposite on both of the sets that matter, and both matter:
/// Java says **false** for the three non-breaking spaces (U+00A0, U+2007,
/// U+202F) that Rust calls whitespace, and **true** for the four information
/// separators (U+001C..U+001F) that Rust does not. 16 measured cases, all in
/// `capitalize` and `title` — `'\u{a0}ab' | capitalize` is `\u{a0}ab` on
/// Kestra and was `\u{a0}Ab` here, because the first non-whitespace character
/// there is the space itself.
///
/// The 25 code points are read out of the JVM, not out of a specification:
/// `Character.isWhitespace(c)` over the whole of Unicode on Corretto 25.
///
/// `trim` does not use this — `String.trim()` is `<= U+0020`, a third and
/// different rule, and it is right where these two were wrong.
fn java_is_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{001c}'..='\u{0020}' | '\u{1680}' | '\u{2000}'..='\u{2006}' | '\u{2008}'..='\u{200a}' | '\u{2028}'..='\u{2029}' | '\u{205f}' | '\u{3000}')
}

/// The code points where `Character.toTitleCase(char)` is neither the
/// character's single-character uppercase nor the character itself.
///
/// **85, not 35, and the 35 was wrong for a structural reason worth keeping.**
/// The first cut derived this from a fixture dumped as "every code point whose
/// title case is not itself" — which is blind, by construction, to a code
/// point whose title case IS itself while its uppercase is not. Those take the
/// uppercase fallback and come out wrong, and there are 50 of them: the four
/// **already**-title-case digraphs `\u{1c5} \u{1c8} \u{1cb} \u{1f2}` — so the
/// half of the family the first cut fixed was `\u{1c6}` to `\u{1c5}` and the
/// half it broke was `\u{1c5}` to `\u{1c5}` — plus the whole Georgian
/// Mkhedruli block, 46 code points that upper-case to Mtavruli and title-case
/// to themselves.
///
/// The fixture is now the **union** of both maps (`toTitleCase(c) != c` OR
/// `toUpperCase(c) != c`), 1 199 rows, and
/// `title_case_matches_the_jvm_over_every_code_point_it_maps` walks all of
/// them — including the rows where the title case is the character, which is
/// exactly where the bug lived and exactly what the old fixture could not
/// contain.
const JAVA_TITLE_CASE: &[(char, char)] = &[
    ('\u{01c4}', '\u{01c5}'),
    ('\u{01c5}', '\u{01c5}'),
    ('\u{01c6}', '\u{01c5}'),
    ('\u{01c7}', '\u{01c8}'),
    ('\u{01c8}', '\u{01c8}'),
    ('\u{01c9}', '\u{01c8}'),
    ('\u{01ca}', '\u{01cb}'),
    ('\u{01cb}', '\u{01cb}'),
    ('\u{01cc}', '\u{01cb}'),
    ('\u{01f1}', '\u{01f2}'),
    ('\u{01f2}', '\u{01f2}'),
    ('\u{01f3}', '\u{01f2}'),
    ('\u{10d0}', '\u{10d0}'),
    ('\u{10d1}', '\u{10d1}'),
    ('\u{10d2}', '\u{10d2}'),
    ('\u{10d3}', '\u{10d3}'),
    ('\u{10d4}', '\u{10d4}'),
    ('\u{10d5}', '\u{10d5}'),
    ('\u{10d6}', '\u{10d6}'),
    ('\u{10d7}', '\u{10d7}'),
    ('\u{10d8}', '\u{10d8}'),
    ('\u{10d9}', '\u{10d9}'),
    ('\u{10da}', '\u{10da}'),
    ('\u{10db}', '\u{10db}'),
    ('\u{10dc}', '\u{10dc}'),
    ('\u{10dd}', '\u{10dd}'),
    ('\u{10de}', '\u{10de}'),
    ('\u{10df}', '\u{10df}'),
    ('\u{10e0}', '\u{10e0}'),
    ('\u{10e1}', '\u{10e1}'),
    ('\u{10e2}', '\u{10e2}'),
    ('\u{10e3}', '\u{10e3}'),
    ('\u{10e4}', '\u{10e4}'),
    ('\u{10e5}', '\u{10e5}'),
    ('\u{10e6}', '\u{10e6}'),
    ('\u{10e7}', '\u{10e7}'),
    ('\u{10e8}', '\u{10e8}'),
    ('\u{10e9}', '\u{10e9}'),
    ('\u{10ea}', '\u{10ea}'),
    ('\u{10eb}', '\u{10eb}'),
    ('\u{10ec}', '\u{10ec}'),
    ('\u{10ed}', '\u{10ed}'),
    ('\u{10ee}', '\u{10ee}'),
    ('\u{10ef}', '\u{10ef}'),
    ('\u{10f0}', '\u{10f0}'),
    ('\u{10f1}', '\u{10f1}'),
    ('\u{10f2}', '\u{10f2}'),
    ('\u{10f3}', '\u{10f3}'),
    ('\u{10f4}', '\u{10f4}'),
    ('\u{10f5}', '\u{10f5}'),
    ('\u{10f6}', '\u{10f6}'),
    ('\u{10f7}', '\u{10f7}'),
    ('\u{10f8}', '\u{10f8}'),
    ('\u{10f9}', '\u{10f9}'),
    ('\u{10fa}', '\u{10fa}'),
    ('\u{10fd}', '\u{10fd}'),
    ('\u{10fe}', '\u{10fe}'),
    ('\u{10ff}', '\u{10ff}'),
    ('\u{1f80}', '\u{1f88}'),
    ('\u{1f81}', '\u{1f89}'),
    ('\u{1f82}', '\u{1f8a}'),
    ('\u{1f83}', '\u{1f8b}'),
    ('\u{1f84}', '\u{1f8c}'),
    ('\u{1f85}', '\u{1f8d}'),
    ('\u{1f86}', '\u{1f8e}'),
    ('\u{1f87}', '\u{1f8f}'),
    ('\u{1f90}', '\u{1f98}'),
    ('\u{1f91}', '\u{1f99}'),
    ('\u{1f92}', '\u{1f9a}'),
    ('\u{1f93}', '\u{1f9b}'),
    ('\u{1f94}', '\u{1f9c}'),
    ('\u{1f95}', '\u{1f9d}'),
    ('\u{1f96}', '\u{1f9e}'),
    ('\u{1f97}', '\u{1f9f}'),
    ('\u{1fa0}', '\u{1fa8}'),
    ('\u{1fa1}', '\u{1fa9}'),
    ('\u{1fa2}', '\u{1faa}'),
    ('\u{1fa3}', '\u{1fab}'),
    ('\u{1fa4}', '\u{1fac}'),
    ('\u{1fa5}', '\u{1fad}'),
    ('\u{1fa6}', '\u{1fae}'),
    ('\u{1fa7}', '\u{1faf}'),
    ('\u{1fb3}', '\u{1fbc}'),
    ('\u{1fc3}', '\u{1fcc}'),
    ('\u{1ff3}', '\u{1ffc}'),
];

/// `Character.toTitleCase(char)`, from the JVM's own table.
///
/// Three arms, and the first cut had only the middle one:
///
/// 1. a code point with a **distinct title case** — [`JAVA_TITLE_CASE`], 58 of
///    them — takes it;
/// 2. otherwise, if the uppercase is a single character, that;
/// 3. otherwise **the character unchanged**, because
///    `Character.toTitleCase(char)` returns a `char` and cannot express an
///    expansion.
fn java_title_case(c: char) -> char {
    if let Some((_, title)) = JAVA_TITLE_CASE.iter().find(|(from, _)| *from == c) {
        return *title;
    }
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

/// `TrimFilter` — `String.trim()`, which is **not** Rust's `str::trim`.
///
/// Java strips every character `<= U+0020` from both ends and nothing else, so
/// U+00A0 and U+2028 survive there and Rust's Unicode-whitespace `trim` eats
/// them. And Pebble's takes **no argument**: `'xxhixx' | trim('x')` is
/// `xxhixx` on Kestra, where minijinja's honoured the `x`.
fn trim(value: MjValue, _rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    strict_undefined("trim", &value)?;
    let Some(text) = pebble_string_cast(&value)? else {
        return Ok(MjValue::from(()));
    };
    let trimmed = text.trim_matches(|c: char| c <= '\u{20}').to_string();
    Ok(MjValue::from(trimmed))
}

/// `LengthFilter` — and its last branch is `return 0`, not an error.
///
/// A null is `0`, a String is its **UTF-16** length, a Collection or Map is
/// its size, and **anything else is `0`** — a number, a boolean, a date. egret
/// raised for all four of those. The UTF-16 part is observable without
/// contrivance: `'a😀b' | length` is `4` on Kestra and was `3` here.
fn length(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("length", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(0));
    }
    let count = match value.kind() {
        ValueKind::String => value
            .as_str()
            .map(|s| s.chars().map(char::len_utf16).sum::<usize>())
            .unwrap_or(0),
        ValueKind::Seq | ValueKind::Iterable | ValueKind::Map => {
            value.try_iter().map(Iterator::count).unwrap_or(0)
        }
        _ => 0,
    };
    Ok(MjValue::from(count))
}

/// `DefaultFilter` — `EmptyTest ? default : input`, and `EmptyTest` is much
/// wider than minijinja's `is undefined`.
///
/// Pebble falls back for a Java `null`, for a String that is **blank after
/// `trim()`**, and for an empty Collection or Map. minijinja's `default` fires
/// only for an undefined value, so `{{ nothing | default('D') }}` rendered
/// empty where Kestra writes `D` — probably the most-written filter in a real
/// flow, silently wrong on the case it exists for.
///
/// `0` and `false` are **not** empty and are kept, which is the boundary worth
/// pinning in both directions.
fn default(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    // **No `strict_undefined` here, and that is measured rather than an
    // oversight.** Pebble's filter-invocation expression special-cases
    // `default`: it catches the attribute-not-found exception so the filter
    // can be the fallback it exists to be. `{{ nothere | default('D') }}` and
    // `{{ obj.missing | default('D') }}` are both `D` on Kestra, where every
    // other filter in this module raises. `filter-default-undefined-root` and
    // `-undefined-attribute` are the entries, and they are the reason this
    // function's guard is absent instead of forgotten.
    let args = Args::new(rest);
    let fallback = args.get("default", 0).unwrap_or_else(|| MjValue::from(()));
    // **`is_undefined()` is spelled out here and NOT in [`is_java_null`], and
    // that split is the whole point of W2b.** Until the rewriter learned
    // Pebble's `null` literal, `is_java_null` folded `Undefined` in, and this
    // branch got Pebble's catch-the-attribute-exception behaviour for free.
    // Narrowing that predicate to `is_none()` took it away and turned both
    // `filter-default-undefined-*` entries red -- caught by the ratchet, not
    // by reasoning. `default` is the one filter that must treat a missing name
    // as empty, so it says so itself rather than making every other filter
    // share a predicate that is only right here.
    let empty = if value.is_undefined() || is_java_null(&value) {
        true
    } else {
        match value.kind() {
            ValueKind::String => value
                .as_str()
                .is_some_and(|s| s.trim_matches(|c: char| c <= '\u{20}').is_empty()),
            // **An ARRAY is never empty to this filter, however few elements
            // it has.** `EmptyTest` is `null || (String && trim().isEmpty())
            // || (Collection && isEmpty()) || (Map && isEmpty())` --
            // `javap -p -c EmptyTest`, four branches and **no array branch** --
            // and a Java array is not a `Collection`. So
            // `{{ ',' | split(',') | default('D') }}` is the empty array there
            // and was `D` here, and every filter after it then saw a String
            // where Kestra sees an array: `| capitalize` is a
            // `ClassCastException` there and answered `D` here.
            //
            // Reachable only at length 0 of the `Array` kind -- a cell that
            // existed in no instrument until `oracle/return-classes.txt`'s
            // length bucket was derived from the MEASURED length instead of
            // from the input binding's name. Eight rows, all over-acceptance.
            ValueKind::Seq | ValueKind::Iterable if java_seq_kind(&value) == JavaSeqKind::Array => {
                false
            }
            ValueKind::Seq | ValueKind::Iterable | ValueKind::Map => value
                .try_iter()
                .map(|mut i| i.next().is_none())
                .unwrap_or(false),
            _ => false,
        }
    };
    Ok(if empty { fallback } else { value })
}

/// `SplitFilter` — `String.split`, and the delimiter is a **Java regex**.
///
/// This is the worst of the sixteen, because every common delimiter is a
/// metacharacter and every one of them was silently wrong:
///
/// | expression | Kestra | egret, before |
/// |---|---|---|
/// | `'a.b.c' \| split('.')` | `[]` | `["a","b","c"]` |
/// | `'a\|b' \| split('\|')` | `["a","\|","b"]` | `["a","b"]` |
/// | `'a+b' \| split('+')` | `PatternSyntaxException` | `["a","b"]` |
/// | `'a,b,,c' \| split(',', 2)` | `["a","b,,c"]` | the limit was ignored |
///
/// The delimiter and the input are both `(String)` casts (a number is a
/// `ClassCastException`, not a stringification), an absent delimiter is
/// `missing delimiter parameter in split filter`, and the `limit` is a
/// `(Number)`. [`super::javare::JavaRegex::split`] carries Java's own loop.
fn split(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    strict_undefined("split", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let Some(delimiter) = args.get("delimiter", 0) else {
        return Err(pebble_error("missing delimiter parameter in split filter"));
    };
    // `(String) args.get("delimiter")`, so an explicit null reaches the null
    // check below rather than the cast.
    let Some(delimiter) = pebble_string_cast(&delimiter)? else {
        return Err(pebble_error("missing delimiter parameter in split filter"));
    };
    let limit = match args.get("limit", 1).filter(|v| !is_java_null(v)) {
        None => 0i64,
        Some(limit) => {
            // `(Number) args.get("limit")` then `intValue()`, which truncates.
            if limit.kind() != ValueKind::Number {
                return Err(pebble_error(format!(
                    "class {} cannot be cast to class java.lang.Number",
                    java_class_name(&limit)
                )));
            }
            i64::try_from(limit.clone())
                .unwrap_or_else(|_| f64::try_from(limit).map(|f| f as i64).unwrap_or(0))
        }
    };
    let Some(input) = pebble_string_cast(&value)? else {
        return Ok(MjValue::from(()));
    };

    let regex = super::javare::compile(&delimiter).map_err(|e| match e {
        super::javare::JavaRegexError::Syntax(_) => {
            pebble_error(format!("Invalid regex '{delimiter}': {e}"))
        }
        super::javare::JavaRegexError::Unsupported(_) => {
            pebble_error(format!("Unsupported regex '{delimiter}': {e}"))
        }
    })?;
    let parts = regex
        .split(&input, limit)
        .map_err(|e| pebble_error(format!("Unsupported regex '{delimiter}': {e}")))?;
    // `String.split` returns `String[]`, **an array and not a `List` at all**
    // — the single fact behind four of W2a's 38 divergent pairs. See
    // [`JavaSeqKind::Array`].
    Ok(java_seq(
        JavaSeqKind::Array,
        parts.into_iter().map(MjValue::from).collect::<Vec<_>>(),
    ))
}

/// `SliceFilter` — `(fromIndex, toIndex)`, which is **a different filter from
/// minijinja's `slice(slices, index)` wearing the same name**.
///
/// `[3,1,2] | slice(0,2)` is `[3,1]` on Kestra and was an error here;
/// `[3,1,2] | slice(1)` is `[1,2]` there and was `[[3,1,2]]` here — a chunked
/// list, silently, because minijinja's first argument is a *number of slices*.
/// This is exactly the `replace` shape.
///
/// Both indices are `Number`s and `intValue()` truncates (`slice(1.9)` is
/// `slice(1)`); a negative `fromIndex` is
/// `IllegalArgumentException: fromIndex must be greater than 0`; a `toIndex`
/// past the length or not greater than `fromIndex` are two different
/// `PebbleException`s; and `fromIndex` past the length is `subList`'s own
/// `fromIndex(4) > toIndex(3)`. Strings, lists and arrays only.
fn slice(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    strict_undefined("slice", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let index = |name: &str, at: usize| -> Result<Option<i64>, MjError> {
        match args.get(name, at).filter(|v| !is_java_null(v)) {
            None => Ok(None),
            Some(v) if v.kind() == ValueKind::Number => {
                Ok(Some(i64::try_from(v.clone()).unwrap_or_else(|_| {
                    f64::try_from(v).map(|f| f as i64).unwrap_or(0)
                })))
            }
            Some(v) => Err(pebble_error(format!(
                "Argument {name} must be a number. Actual type: {}",
                java_class_name(&v)
            ))),
        }
    };
    let from = index("fromIndex", 0)?.unwrap_or(0);
    if from < 0 {
        return Err(pebble_error("fromIndex must be greater than 0"));
    }
    let to = index("toIndex", 1)?;

    // The three input shapes, and their lengths. A String's length is UTF-16
    // here too, and so is `substring`, so the two agree by construction.
    let units: Vec<MjValue> = match value.kind() {
        ValueKind::String => value
            .as_str()
            .unwrap_or_default()
            .encode_utf16()
            .map(|u| MjValue::from(u64::from(u)))
            .collect(),
        // `SliceFilter` has three branches — `List`, `isArray()` and
        // `String` — and **no `Collection` branch**, so a map's key or value
        // view is refused here where every other sequence is accepted.
        ValueKind::Seq | ValueKind::Iterable
            if java_seq_kind(&value) != JavaSeqKind::Collection =>
        {
            value.try_iter().map(Iterator::collect).unwrap_or_default()
        }
        ValueKind::Seq | ValueKind::Iterable => {
            return Err(pebble_error(format!(
                "Slice filter can only be applied to String, List and array inputs. \
                 Actual type was: {}",
                java_seq_kind(&value).description()
            )))
        }
        _ => {
            return Err(pebble_error(format!(
                "Slice filter can only be applied to String, List and array inputs. \
                 Actual type was: {}",
                java_class_name(&value)
            )))
        }
    };
    let length = units.len() as i64;

    let to = match to {
        Some(to) => {
            if to > length {
                return Err(pebble_error(format!(
                    "toIndex must be smaller than input size: {length}"
                )));
            }
            if from >= to {
                return Err(pebble_error("toIndex must be greater than fromIndex"));
            }
            to
        }
        None => length,
    };
    // `List.subList` / `String.substring`'s own guard, which fires only once
    // `toIndex` has defaulted to the length.
    if from > to {
        return Err(pebble_error(format!("fromIndex({from}) > toIndex({to})")));
    }

    let picked = &units[from as usize..to as usize];
    if value.kind() == ValueKind::String {
        let code_units: Vec<u16> = picked
            .iter()
            .filter_map(|u| u64::try_from(u.clone()).ok().map(|n| n as u16))
            .collect();
        return Ok(MjValue::from(String::from_utf16_lossy(&code_units)));
    }
    // `subList` of a mutable list is mutable and of an immutable one is not;
    // the array branch copies the range and stays an array. Measured through
    // `className`: `ArrayList$SubList`, `ImmutableCollections$SubList` and
    // `[Ljava.lang.String;` respectively. So the kind is INHERITED, which is
    // why `((l | chunk(size=2)) | slice(0,2)) | reverse` still throws.
    Ok(java_seq(java_seq_kind(&value), picked.to_vec()))
}

/// `FormatFilter` — `String.format(input.toString(), args…)`.
///
/// The grammar lives in [`super::javaformat`], which is a port rather than a
/// wrapper — fix round 1 wrapped `minijinja::filters::format` on the premise
/// that the two agreed on the six specifiers minijinja has, and **two of the
/// six were counter-examples**: `'%x' | format(-255)` is `ffffff01` there and
/// was `-ff` here, and `'%.0f' | format(0.5)` is `1` (HALF_UP) against `0`
/// (half-to-even). Three more silently differed, two of them the
/// Python-spelling defect the same round existed to kill:
/// `'%s' | format(true)` was `True` and `'%s' | format({'a':1})` was
/// `{"a": 1}`.
///
/// A null **input** is null; the input is `toString`'d; and the arguments go
/// through [`super::env::kestra_to_string`] inside the conversion rather than
/// being pre-mapped, which is what fixes `%s`.
fn format(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    strict_undefined("format", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    super::javaformat::format(&kestra_to_string(&value), &args.positional_only()).map(MjValue::from)
}

/// `AbsFilter` — and it reproduces Java's `Math.abs` **overflow**.
///
/// `Math.abs(Long.MIN_VALUE)` is `Long.MIN_VALUE`: there is no positive
/// counterpart in the range, and Java returns the input rather than throwing.
/// So `{{ -9223372036854775808 | abs }}` is `-9223372036854775808` on Kestra
/// and was `9223372036854775808` here — a silently wrong sign at exactly one
/// input, which is the only thing the seventeen-name sweep found wrong with
/// `abs`. Reproduced, because a flow author on both engines must see one
/// answer; it is upstream defect 7 in `oracle/README.md`.
///
/// The Integer/Long split follows [`java_class_name`]'s, so
/// `Integer.MIN_VALUE` overflows the same way inside `Math.abs(int)`.
///
/// A null is `Can not pass null value to "abs" filter.` and anything that is
/// not a number is `The 'abs' filter does require a number` — a String is
/// **not** coerced, unlike `upper`'s input.
fn abs(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("abs", &value)?;
    if is_java_null(&value) {
        return Err(pebble_error("Can not pass null value to \"abs\" filter."));
    }
    // `BigDecimal.abs()` / `BigInteger.abs()`: the sign goes, the scale stays.
    if let Some(big) = value.downcast_object_ref::<BigNumber>() {
        return Ok(MjValue::from_object(BigNumber {
            text: big.text.trim_start_matches('-').to_string(),
            canonical: big.canonical.trim_start_matches('-').to_string(),
            class_name: big.class_name,
        }));
    }
    if value.kind() != ValueKind::Number {
        return Err(pebble_error(
            "The 'abs' filter does require a number as input.",
        ));
    }
    if value.is_integer() {
        let Ok(n) = i64::try_from(value.clone()) else {
            return Ok(value);
        };
        return Ok(MjValue::from(match i32::try_from(n) {
            // `Math.abs(int)`, which wraps at `Integer.MIN_VALUE`.
            Ok(small) => i64::from(small.wrapping_abs()),
            // `Math.abs(long)`, which wraps at `Long.MIN_VALUE`.
            Err(_) => n.wrapping_abs(),
        }));
    }
    match f64::try_from(value.clone()) {
        Ok(f) => Ok(MjValue::from(f.abs())),
        Err(_) => Ok(value),
    }
}

/// `FirstFilter` — the first **character** of a String, the first element of a
/// Collection, and a `ClassCastException` for everything else.
///
/// An empty Collection is `null`, and egret raised; an empty String is
/// `charAt(0)`'s `StringIndexOutOfBoundsException`; and a Map is **not** a
/// Collection, so `{"a":1} | first` throws there and answered `a` here.
///
/// # The String branch walks CHARACTERS where Java walks UTF-16 code units
///
/// `charAt(0)` of an astral character is a lone high surrogate; this returns
/// the whole character. Rust cannot represent a lone surrogate in a `String`
/// at all, so this is not a fix that was skipped — and it cannot have a corpus
/// entry either, because the golden file would need a JSON string holding an
/// unpaired surrogate escape, which no strict JSON parser reads. Recorded here
/// and in `oracle/README.md` rather than in a fixture that could not hold it.
///
/// **Nothing else discriminates the String branch from minijinja's**, which is
/// worth saying plainly: measured, `'' | first` errors on both engines and
/// `'abc' | first` is `a` on both, so `filter-first-string` passes the
/// untouched built-in too. The branch is ported for the Map and empty-list
/// cases beside it, which do discriminate.
fn first(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("first", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    match value.kind() {
        ValueKind::String => match value.as_str().unwrap_or_default().chars().next() {
            Some(c) => Ok(MjValue::from(c.to_string())),
            None => Err(pebble_error(
                "Index 0 out of bounds for length 0".to_string(),
            )),
        },
        ValueKind::Seq | ValueKind::Iterable => Ok(value
            .try_iter()
            .ok()
            .and_then(|mut items| items.next())
            .unwrap_or_else(|| MjValue::from(()))),
        _ => Err(pebble_error(format!(
            "class {} cannot be cast to class java.util.Collection",
            java_class_name(&value)
        ))),
    }
}

/// `LastFilter` — [`first`]'s mirror, `charAt(length - 1)` and the last
/// element.
fn last(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("last", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    match value.kind() {
        ValueKind::String => match value.as_str().unwrap_or_default().chars().next_back() {
            Some(c) => Ok(MjValue::from(c.to_string())),
            None => Err(pebble_error(
                "Index -1 out of bounds for length 0".to_string(),
            )),
        },
        ValueKind::Seq | ValueKind::Iterable => Ok(value
            .try_iter()
            .ok()
            .and_then(|items| items.last())
            .unwrap_or_else(|| MjValue::from(()))),
        _ => Err(pebble_error(format!(
            "class {} cannot be cast to class java.util.Collection",
            java_class_name(&value)
        ))),
    }
}

/// `JoinFilter` — `StringBuilder.append(Object)` per element, which is
/// `String.valueOf` and therefore **`null` for a null**.
///
/// The Python-spelling defect again, in a filter nobody had looked at:
/// `[1,null,2] | join('-')` is `1-null-2` on Kestra and was `1-None-2` here,
/// and a map element is `{a=1}` there against `{"a": 1}` here. Both silent.
///
/// A String or a Map **input** is a `PebbleException`, where minijinja's
/// joined a string's characters and a map's keys. And the separator is a
/// `(String)` cast, so `join(3)` is a `ClassCastException`; an explicitly null
/// separator joins with nothing at all, measured.
fn join(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    strict_undefined("join", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    let separator = match args.get("separator", 0) {
        None => String::new(),
        Some(sep) => pebble_string_cast(&sep)?.unwrap_or_default(),
    };
    let items = match value.kind() {
        ValueKind::Seq | ValueKind::Iterable => value
            .try_iter()
            .map(Iterator::collect::<Vec<_>>)
            .unwrap_or_default(),
        _ => {
            return Err(pebble_error(
                "The 'join' filter expects that the input is either a collection or an array.",
            ))
        }
    };
    let rendered: Vec<String> = items.iter().map(kestra_to_string).collect();
    Ok(MjValue::from(rendered.join(&separator)))
}

/// `SortFilter` — `Collections.sort`, which is `Comparable` and therefore
/// **refuses a mixed list**.
///
/// Four over-acceptances, all of them egret answering where Kestra throws:
/// `[1, 2147483648] | sort` is a `ClassCastException` there (an `Integer` and
/// a `Long` are not mutually comparable, and which one a JSON number becomes
/// is its magnitude — see [`java_class_name`]), and so are `[true, 1]`,
/// `[[1],[2]]` and a String input.
///
/// **The comparison is pairwise, so a one-element list never compares
/// anything**: `[null] | sort` is `[null]` and `[{"a":1}] | sort` is itself,
/// both measured. That is why the class check below is skipped below two
/// elements rather than run over every input.
/// An integer as an exactly ordered key, over every `i128` and every `u128`
/// (W9c final review C-M1): the variants order negatives first.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum WideInt {
    Negative(i128),
    NonNegative(u128),
}

/// `v` as a [`WideInt`], or `None` when it is not an integer either type
/// holds.
fn wide_integer(v: &MjValue) -> Option<WideInt> {
    match i128::try_from(v.clone()) {
        Ok(i) if i < 0 => Some(WideInt::Negative(i)),
        Ok(i) => Some(WideInt::NonNegative(i.unsigned_abs())),
        Err(_) => u128::try_from(v.clone()).ok().map(WideInt::NonNegative),
    }
}

fn sort(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("sort", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let mut items: Vec<MjValue> = match value.kind() {
        ValueKind::Seq | ValueKind::Iterable => value
            .try_iter()
            .map(Iterator::collect::<Vec<_>>)
            .unwrap_or_default(),
        _ => return Err(pebble_error("Unsupported input type for sort filter")),
    };
    // `SortFilter` is `instanceof List` -> cast, else `instanceof
    // Comparable[]` -> `Arrays.asList`, else this message; then
    // `Collections.sort`, which needs `set` and throws on an immutable list.
    // So an ARRAY sorts (and comes back as the mutable `Arrays$ArrayList`)
    // while a map's key view does not, and `flatten`'s list does not either.
    // See [`JavaSeqKind`].
    let input_kind = java_seq_kind(&value);
    if input_kind == JavaSeqKind::Collection {
        return Err(pebble_error("Unsupported input type for sort filter"));
    }
    java_list_sort(&mut items, input_kind, false)?;
    // **`SortFilter` `areturn`s the list it sorted**, so the kind SURVIVES it
    // — `{{ [] | chunk(size=2) | sort | className }}` is still
    // `Lists$RandomAccessPartition`, measured. The one input that does not
    // come back as itself is an array: that branch is `Arrays.asList(array)`,
    // a mutable `Arrays$ArrayList`, which is why
    // `{{ 'a,b,c' | split(',') | sort | reverse }}` answers where
    // `{{ 'a,b,c' | split(',') | reverse }}` throws.
    let out_kind = match input_kind {
        JavaSeqKind::Array => JavaSeqKind::MutableList,
        kind => kind,
    };
    Ok(java_seq(out_kind, items))
}

/// `List.sort` with `Comparable`'s natural order (`sort`) or
/// `Collections.reverseOrder()` (`rsort`, package #5b Task B4): the
/// in-place refusal of an immutable list, the class check, then one
/// comparator for the one class. `reverse` flips the comparator, not the
/// result, so equal elements keep their input order as Java's stable sort
/// keeps them under `reverseOrder()`.
fn java_list_sort(
    items: &mut [MjValue],
    input_kind: JavaSeqKind,
    reverse: bool,
) -> Result<(), MjError> {
    if input_kind.sort_in_place_throws(items.len()) {
        return Err(pebble_error(
            "'sort' filter cannot sort an immutable list in place: 'flatten', 'chunk' \
             and 'keys' return one",
        ));
    }
    let first_class = items.first().map(java_class_name).unwrap_or_default();
    if items.len() >= 2 {
        for item in &items[1..] {
            let class = java_class_name(item);
            if class != first_class {
                return Err(pebble_error(format!(
                    "class {class} cannot be cast to class {first_class}"
                )));
            }
        }
        if !matches!(
            first_class.as_str(),
            "java.lang.String"
                | "java.lang.Boolean"
                | "java.lang.Integer"
                | "java.lang.Long"
                | "java.lang.Double"
                | "java.math.BigInteger"
                | "java.math.BigDecimal"
        ) {
            return Err(pebble_error(format!(
                "class {first_class} cannot be cast to class java.lang.Comparable"
            )));
        }
    }
    // **One class, so one comparator -- and an integer class must NEVER reach
    // `f64`.** The first cut compared every number through
    // `f64::try_from(..).total_cmp(..)`, which collapses every integer past
    // 2^53, and `sort_by` is stable, so a pair it could not tell apart kept
    // its INPUT order:
    //
    // | list | Kestra | the f64 comparator |
    // |---|---|---|
    // | `[9007199254740993, 9007199254740992]` | `..992, ..993` | `..993, ..992` |
    // | `[9223372036854775807, 9223372036854775806]` | `..806, ..807` | `..807, ..806` |
    //
    // Java sorts a `Long` with `Long.compareTo`, which is exact. **The
    // pre-port `sort` was minijinja's own, which also compares integers
    // exactly, so the port introduced a divergence the default it replaced did
    // not have** -- on epoch nanoseconds, snowflake ids and counters.
    // `filter-sort-past-two-to-the-53` is the entry, and it is the first in
    // this family that exercises the COMPARATOR rather than the type check.
    let integers = matches!(
        first_class.as_str(),
        "java.lang.Integer" | "java.lang.Long" | "java.math.BigInteger"
    );
    items.sort_by(|a, b| {
        let order = natural_order(a, b, integers, &first_class);
        if reverse {
            order.reverse()
        } else {
            order
        }
    });
    Ok(())
}

/// `compareTo` between two elements of one `Comparable` class (`first_class`).
fn natural_order(
    a: &MjValue,
    b: &MjValue,
    integers: bool,
    first_class: &str,
) -> std::cmp::Ordering {
    {
        // W9c Task 4 fix round 1: `BigDecimal.compareTo` and
        // `BigInteger.compareTo` are exact. A `BigDecimal` list used to be
        // refused as not `Comparable` (it is), and a `BigInteger` past `i128`
        // fell to the TEXT order below, where `9` sorts after `10`. The class
        // check above has already made the two elements one class.
        if super::compare::exact_cmp(a, b).is_some() {
            return super::compare::values_cmp(a, b);
        }
        if integers {
            // `i128` holds every `i64` and every `u64`; a native `u128` past
            // `i128::MAX` (a `kv()` integer, `integer_from_literal`) is
            // compared as one too (W9c final review C-M1: it fell to the
            // TEXT order, so `2^127` sorted before `2^64`).
            return match (wide_integer(a), wide_integer(b)) {
                (Some(x), Some(y)) => x.cmp(&y),
                // A `BigInteger` past `u128` is `exact_cmp`'s, above, and
                // `java_class_name` has already made the two elements one
                // class.
                _ => kestra_to_string(a).cmp(&kestra_to_string(b)),
            };
        }
        if first_class == "java.lang.Double" {
            // `Double.compare`'s total order, which is `total_cmp`: -0.0 below
            // 0.0 and NaN above everything.
            if let (Ok(x), Ok(y)) = (f64::try_from(a.clone()), f64::try_from(b.clone())) {
                return x.total_cmp(&y);
            }
        }
        // `String.compareTo` is UTF-16 code-unit order and `Boolean.compareTo`
        // is false before true; the text order gives both. Strings disagree
        // with Java only for a supplementary character against U+E000..U+FFFF,
        // which `oracle/name-surface.txt` records.
        kestra_to_string(a).cmp(&kestra_to_string(b))
    }
}

/// `ReverseFilter` — `(List) input`, so a String or a Map is a
/// `ClassCastException` rather than a reversed string.
fn reverse(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("reverse", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let mut items: Vec<MjValue> = match value.kind() {
        ValueKind::Seq | ValueKind::Iterable => value
            .try_iter()
            .map(Iterator::collect::<Vec<_>>)
            .unwrap_or_default(),
        _ => {
            return Err(pebble_error(format!(
                "class {} cannot be cast to class java.util.List",
                java_class_name(&value)
            )))
        }
    };
    // `ReverseFilter` is a bare `(List) input` and then `Collections.reverse`,
    // so it is the STRICTEST of the seven: a key view and an array are
    // `ClassCastException`s and an immutable list is an
    // `UnsupportedOperationException`. See [`JavaSeqKind`].
    let kind = java_seq_kind(&value);
    if !kind.is_list() {
        return Err(pebble_error(format!(
            "'reverse' filter can only be applied to a List; this is {}",
            kind.description()
        )));
    }
    if kind.reverse_in_place_throws(items.len()) {
        return Err(pebble_error(
            "'reverse' filter cannot reverse an immutable list in place: 'flatten', \
             'chunk' and 'keys' return one",
        ));
    }
    items.reverse();
    // **`Collections.reverse` is `void` and the filter `areturn`s the SAME
    // local**, so the kind survives — the fact the doc comment above already
    // quoted and the first cut did not act on. Reachable only at length 0 or
    // 1, because at length 2 the reverse itself throws:
    // `{{ [] | flatten | reverse | sort }}` is an
    // `UnsupportedOperationException` there and was `[]` here.
    Ok(java_seq(kind, items))
}

/// The `escape` filter's null and stringify branches.
///
/// `EscaperExtension`'s filter takes a `strategy` argument and Pebble's own
/// escaper library implements `html`, `js`, `css` and `url_param`. Only the
/// default — HTML — is ported; the other three are **recorded**, because they
/// are three character tables this fix round was not scoped to and because
/// refusing them is the safe direction (an author gets an error, never a
/// wrong escape).
///
/// What is fixed is the part that was silently wrong: a null rendered `None`
/// and a boolean rendered `True`, both `Display` rather than
/// `Object::toString`. And `&#x27;` was `&#39;` on Kestra — the same
/// character, a different spelling, and a byte difference in rendered output.
fn escape(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    strict_undefined("escape", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let args = Args::new(rest);
    if let Some(strategy) = args.get("strategy", 0) {
        let strategy = kestra_to_string(&strategy);
        if strategy != "html" {
            return Err(pebble_error(format!(
                "the 'escape' filter's '{strategy}' strategy is not implemented in egret; \
                 only 'html' is"
            )));
        }
    }
    let mut out = String::new();
    for c in kestra_to_string(&value).chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    Ok(MjValue::from(out))
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
    // `instanceof List` — see [`JavaSeqKind`]. `{{ m | keys | distinct }}` and
    // `{{ s | split(',') | distinct }}` both throw on Kestra.
    let kind = java_seq_kind(&value);
    if !kind.is_list() {
        return Err(pebble_error(format!(
            "Input must be a list, but received : {}",
            kind.description()
        )));
    }

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
    // `instanceof List` — see [`JavaSeqKind`]. Same message as the non-list
    // arm above, because Kestra's is a constant that names no type.
    if !java_seq_kind(&value).is_list() {
        return Err(pebble_error(
            "The 'flatten' filter can only be applied to lists.",
        ));
    }

    let mut out = Vec::new();
    for item in items {
        // `lambda$apply$0` is `instanceof List ? list.stream() :
        // Stream.of(o)`, so the ELEMENT test is the same cast as the input's:
        // an inner array or key view is kept whole rather than unwrapped.
        match as_list(&item).filter(|_| java_seq_kind(&item).is_list()) {
            Some(inner) => out.extend(inner),
            None => out.push(item),
        }
    }
    // `Stream.toList()`, which is `ImmutableCollections$ListN`.
    Ok(java_seq(JavaSeqKind::ImmutableList, out))
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
        // `Map.keySet()` is a VIEW, not a `List` — the producer half of the
        // class. `{{ m | keys | sort }}` throws on Kestra for this reason.
        return Ok(java_seq(JavaSeqKind::Collection, out));
    }
    if let Some(items) = as_list(&value) {
        // `KeysFilter` has a third branch, `getClass().isArray()`, which is
        // why an array is accepted here where `distinct`, `chunk`, `flatten`
        // and `reverse` refuse it. A key view reaches none of the three and
        // falls through to the refusal below.
        let kind = java_seq_kind(&value);
        if kind == JavaSeqKind::Collection {
            return Err(pebble_error(format!(
                "'keys' filter can only be applied to List, Map, Array. Actual type was: {}",
                kind.description()
            )));
        }
        // Both the list and the array branch end in `Stream.toList()`.
        let out: Vec<MjValue> = (0..items.len()).map(|i| MjValue::from(i as i64)).collect();
        return Ok(java_seq(JavaSeqKind::ImmutableList, out));
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
    // `Map.values()` is a `Collection` view, not a `List` — see
    // [`JavaSeqKind::Collection`] and `keys` above.
    Ok(java_seq(JavaSeqKind::Collection, out))
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
///
/// # The undefined guard, and why this one filter got it in W2b
///
/// `filter-string-undefined-input` (`<{{ typo | string }}>`) has been baselined
/// since W2 Task 4's third fix round, and its comment in `oracle/baseline.txt`
/// is a hand-off in as many words: the real fix is for `super::rewrite` to
/// translate Pebble's `null` literal, "flagged for whoever owns it". W2b is
/// that owner, so the entry is banked here rather than left as a note.
///
/// A W2b sweep over all 35 registered filters x {missing root, missing
/// attribute} found **nineteen** that answer where Kestra raises; narrowing
/// [`is_java_null`] closed sixteen of the thirty-five outright, `string` is the
/// seventeenth and closes here, and the remaining eighteen are recorded with
/// their names in `oracle/README.md` rather than ported in this task.
fn string(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("string", &value)?;
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
    Ok(MjValue::from(slugify_str(&kestra_to_string(&value))))
}

/// [`slugify`]'s body over a string: `Slugify.of` without the filter's Java-null
/// arm. Also the slug Kestra puts into a storage URI for a flow, task or
/// trigger id (`StorageContext`), which is why `storage::uri` calls it (W9a).
pub(crate) fn slugify_str(input: &str) -> String {
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

    slug.to_ascii_lowercase()
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
        return Ok(MjValue::from_object(OrderedMap::new(out)));
    }
    // `instanceof List`, and it is the SAME cast the other consumers make —
    // a map's key or value view and a `String[]` are not `List`s, so they fall
    // past this branch to `toString()` below. `{{ m | keys | escapeChar(…) }}`
    // is the six characters `[b, a]` on Kestra, not a walked list, and
    // **both engines' orders are identical**, which is what makes this the
    // collection class rather than the `HashMap` one it was first filed under.
    // The gate is here rather than only at the top because Kestra's recursive
    // helper repeats the same three branches at every depth.
    if java_seq_kind(value).is_list() {
        if let Some(items) = as_list(value) {
            let mut out = Vec::with_capacity(items.len());
            for item in &items {
                out.push(escape_value(item, args)?);
            }
            return Ok(MjValue::from(out));
        }
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
/// Arithmetic on it is Pebble's (package #5a Task A4, spec D3): the
/// operators are compiled to `super::arith`, which computes a `BigDecimal`
/// operand as `bigDecimalOperation` does and returns a new `BigNumber`.
/// minijinja's own operators still refuse it, which only the arguments of a
/// `{% do f(..) %}` (not Pebble syntax; its value is discarded) can reach.
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

    /// Package #5a Task A4: Pebble's `{% if %}` casts a `Number` to a
    /// boolean as `TypeUtils.compatibleCast` does, `doubleValue()` neither
    /// NaN nor zero, so a zero decimal is false (an object is otherwise
    /// always true in minijinja). `1E-400` is false too: its double is `0.0`.
    fn is_true(self: &Arc<Self>) -> bool {
        !matches!(self.text.parse::<f64>(), Ok(d) if d == 0.0)
    }

    /// W9c Task 4 fix round 1: two `BigNumber`s compare by value, wherever
    /// minijinja compares two of them itself (`1.5 == 1.50`, `-1.5 > -2.5`).
    /// minijinja asks this ONLY for two objects of the same type; a
    /// `BigNumber` against a number never reaches it, which is why the
    /// comparison operators are compiled to `super::compare`'s functions.
    fn custom_cmp(self: &Arc<Self>, other: &DynObject) -> Option<std::cmp::Ordering> {
        let other = other.downcast_ref::<Self>()?;
        super::compare::plain_text_cmp(&self.text, &other.text)
    }
}

/// W9c Task 4 fix round 1: is this value a [`BigNumber`]?
pub(super) fn is_big_number(value: &MjValue) -> bool {
    value.downcast_object_ref::<BigNumber>().is_some()
}

/// W9c Task 4 fix round 1: a [`BigNumber`]'s `toPlainString()` text, the
/// exact value `super::compare` compares, or `None` for anything else.
pub(super) fn big_number_plain(value: &MjValue) -> Option<&str> {
    value
        .downcast_object_ref::<BigNumber>()
        .map(|big| big.text.as_str())
}

/// The Java class of a [`BigNumber`], or `None` for anything else.
pub(super) fn big_number_class(value: &MjValue) -> Option<&'static str> {
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

/// W9c Task 4: a `java.math.BigDecimal` [`BigNumber`] of `plain`, a KV
/// value's decimal (`storage::kv::value::KvNumber::Decimal`, plain digits
/// with their scale): a bare `{{ x }}` prints `plain`, a serialiser writes
/// `BigDecimal.toString()`. `None` when `plain` is not a decimal literal.
///
/// Also the result of `super::arith`'s `BigDecimal` arithmetic (from its
/// `unscaledE-scale` text) and an Ion decimal from `fromIon`. `{{ kv('d') +
/// 1 }}` computes as Kestra does (package #5a Task A4, spec D3, which
/// reverses W9c spec §15's refusal).
pub(crate) fn big_decimal(plain: &str) -> Option<MjValue> {
    let (text, canonical) = big_decimal_strings(plain)?;
    Some(MjValue::from_object(BigNumber {
        text,
        canonical,
        class_name: "java.math.BigDecimal",
    }))
}

/// W9c final review F1: `new BigDecimal(plain).toString()`, or `None` when
/// `plain` is not a decimal literal.
pub(crate) fn big_decimal_to_string(plain: &str) -> Option<String> {
    big_decimal_strings(plain).map(|(_, canonical)| canonical)
}

/// W9c Task 4: a `java.math.BigInteger` [`BigNumber`] of `digits`
/// (`-?[0-9]+`), a KV value's integer beyond every Rust integer primitive.
pub(crate) fn big_integer(digits: &str) -> MjValue {
    MjValue::from_object(BigNumber {
        text: digits.to_string(),
        canonical: digits.to_string(),
        class_name: "java.math.BigInteger",
    })
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

/// Is this value a `BigDecimal` rather than a `BigInteger`?
///
/// [`super::ionfmt::write_ion`] needs the difference: Ion spells a decimal
/// with a `.` or a `d` and an integer with neither, so a scale-0
/// `BigDecimal` is `1.` and a `BigInteger` is `1`. Measured —
/// `'1' | number(type='BIGDECIMAL') | toIon` is `1.`, which the first cut of
/// that writer wrote as `1`.
pub(super) fn is_big_decimal(value: &MjValue) -> bool {
    big_number_class(value) == Some("java.math.BigDecimal")
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
/// Kestra has no `sha256` filter of its own, despite all three hash classes
/// carrying a copy-pasted "implements the 'sha256' filter" javadoc:
/// `Extension.getFilters()` registers `md5`, `sha1` and `sha512` and nothing
/// else. The `sha256` name is Pebble core's `Sha256Filter` ([`sha256`],
/// package #5b Task B4).
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
// Pebble core's ten (package #5b Task B4, spec D13)
// ---------------------------------------------------------------------------
//
// `abbreviate`, `base64decode`, `base64encode`, `merge`, `nl2br`,
// `numberformat`, `rsort`, `sha256` and `urlencode` are Pebble 4.1.2's
// `CoreExtension` filters and `raw` its `EscaperExtension`'s; Kestra registers
// none of them itself, so they were outside every W2 batch. Each is ported
// from the pinned jar (`javap -c -p`), and each refuses an undefined input as
// strict variables do (`strict_undefined`).

/// `Sha256Filter` — Pebble's, not Kestra's (see [`sha512`]): the same shape
/// as `ShaBaseFilter`, `Need a string to hash\n` for a non-string, lowercase
/// hex of the UTF-8 bytes.
fn sha256(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("sha256", &value)?;
    hash_with(&value, |bytes| {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(bytes))
    })
}

/// `AbbreviateFilter` — `(String) input`, `(Long) length` narrowed by
/// `intValue()`, and a cut counted in UTF-16 code units:
///
/// - a negative width fails (`Invalid argument to abbreviate filter; must be
///   greater than zero`); **zero does not** (spec §2.2 item 12, measured);
/// - shorter than the width, or three units or fewer, is the input;
/// - a width of three or less is a plain cut (so `abbreviate(0)` of a string
///   longer than three is empty: the measured `'abc' | abbreviate(0)` is
///   `abc` only because `abc` is three long);
/// - otherwise the first `width - 3` units and `...`, so a width EQUAL to the
///   length still cuts (`'abcdef' | abbreviate(6)` is `abc...`, measured).
fn abbreviate(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    strict_undefined("abbreviate", &value)?;
    let Some(text) = pebble_string_cast(&value)? else {
        return Ok(MjValue::from(()));
    };
    let args = Args::new(rest);
    let width = match args.get("length", 0).filter(|v| !is_java_null(v)) {
        None => {
            return Err(pebble_error(
                "Cannot invoke \"java.lang.Long.intValue()\" because the return value of \
                 \"java.util.Map.get(Object)\" is null",
            ))
        }
        Some(v) if v.kind() == ValueKind::Number && v.is_integer() => {
            java_int_value(&v).unwrap_or_default()
        }
        Some(v) => {
            return Err(pebble_error(format!(
                "class {} cannot be cast to class java.lang.Long",
                java_class_name(&v)
            )))
        }
    };
    if width < 0 {
        return Err(pebble_error(
            "Invalid argument to abbreviate filter; must be greater than zero",
        ));
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let width = width as usize;
    if units.len() < width || units.len() <= 3 {
        return Ok(MjValue::from(text));
    }
    if width <= 3 {
        return Ok(MjValue::from(String::from_utf16_lossy(&units[..width])));
    }
    let mut out = String::from_utf16_lossy(&units[..width - 3]);
    out.push_str("...");
    Ok(MjValue::from(out))
}

/// `java.util.Base64.getDecoder()`: the standard alphabet, padding optional
/// but correct when present, no whitespace, unused trailing bits ignored.
const JAVA_BASE64_DECODER: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    base64::engine::GeneralPurposeConfig::new()
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// `Base64EncoderFilter` — `Object.toString()` (so a number or a map is
/// encoded as its Java text), UTF-8, standard alphabet with padding.
fn base64encode(value: MjValue) -> Result<MjValue, MjError> {
    use base64::Engine as _;
    strict_undefined("base64encode", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let text = kestra_to_string(&value);
    Ok(MjValue::from(
        base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
    ))
}

/// `Base64DecoderFilter` — a String only (`This filter applies to String\n`
/// otherwise); a string the decoder refuses is `Please provide a correctly
/// Base64 encoded string containing an UTF-8 string\n`; the bytes become a
/// String as UTF-8 with malformed sequences replaced, as `new String(bytes,
/// UTF_8)` replaces them. An unpadded string decodes (measured).
fn base64decode(value: MjValue) -> Result<MjValue, MjError> {
    use base64::Engine as _;
    strict_undefined("base64decode", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(text) = value.as_str() else {
        return Err(pebble_error("This filter applies to String\n"));
    };
    // Padding is optional, but present padding must complete the last
    // four-character unit: Java refuses `YQ=` (measured: `Input byte array
    // has wrong 4-byte ending unit`) where the crate's indifferent mode
    // accepts a partial pad.
    let bad_padding = text.contains('=') && !text.len().is_multiple_of(4);
    match JAVA_BASE64_DECODER
        .decode(text.as_bytes())
        .ok()
        .filter(|_| !bad_padding)
    {
        Some(bytes) => Ok(MjValue::from(String::from_utf8_lossy(&bytes).into_owned())),
        None => Err(pebble_error(
            "Please provide a correctly Base64 encoded string containing an UTF-8 string\n",
        )),
    }
}

/// `Nl2brFilter` — every `\n`, `\r` and `\r\n` becomes `<br />`, with **no
/// escaping** of the rest (`'<a>\nb'` is `<a><br />b`, measured). A
/// non-string is an `IllegalArgumentException`.
fn nl2br(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("nl2br", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(text) = value.as_str() else {
        return Err(pebble_error("nl2br filters only supports String input."));
    };
    let mut out = String::with_capacity(text.len() + 16);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                out.push_str("<br />");
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\n' => out.push_str("<br />"),
            c => out.push(c),
        }
    }
    Ok(MjValue::from(out))
}

/// `RawFilter` — `new SafeString(input.toString())`. Kestra renders with
/// `autoEscaping(false)`, so the safe mark changes nothing there; what is
/// left is the `toString()`, which is Java's text (`[1, 2]`, `{a=1}`), not
/// the JSON a bare `{{ x }}` writes.
fn raw(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("raw", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    Ok(MjValue::from(kestra_to_string(&value)))
}

/// `UrlEncoderFilter` — `URLEncoder.encode(s, "UTF-8")`: form encoding, so a
/// space is `+`, `a-zA-Z0-9.-*_` stay, and every other UTF-8 byte is `%XX`
/// in upper-case hex (`~` is `%7E`, measured). `(String) input`.
fn urlencode(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("urlencode", &value)?;
    let Some(text) = pebble_string_cast(&value)? else {
        return Ok(MjValue::from(()));
    };
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'*' | b'_' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    Ok(MjValue::from(out))
}

/// `RsortFilter` — `(List) input`, then `List.sort(Collections.reverseOrder())`
/// in place, which is [`sort`]'s comparison reversed: a mixed list fails the
/// same way (`[1, 'a']` is a `ClassCastException`, and so is a context
/// `[1.5, 3, 2]`, a `Double` beside two `Integer`s), and an immutable list
/// refuses to be sorted. Unlike `sort` there is no array branch: a `String[]`
/// (`split`) and a map's key view are not `List`s, so both are cast failures.
fn rsort(value: MjValue) -> Result<MjValue, MjError> {
    strict_undefined("rsort", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let kind = java_seq_kind(&value);
    let mut items = match value.kind() {
        ValueKind::Seq | ValueKind::Iterable if kind.is_list() => value
            .try_iter()
            .map(Iterator::collect::<Vec<_>>)
            .unwrap_or_default(),
        ValueKind::Seq | ValueKind::Iterable => {
            return Err(pebble_error(format!(
                "{} cannot be cast to class java.util.List",
                kind.description()
            )))
        }
        _ => {
            return Err(pebble_error(format!(
                "class {} cannot be cast to class java.util.List",
                java_class_name(&value)
            )))
        }
    };
    java_list_sort(&mut items, kind, true)?;
    Ok(java_seq(kind, items))
}

/// `MergeFilter` — by the INPUT's type:
///
/// | input | argument | result |
/// |---|---|---|
/// | null | null | fails: `The two arguments to be merged are null` |
/// | null | any | the argument |
/// | any | null | the input (measured) |
/// | Map | Map | a new map, the argument's values winning |
/// | Map | List | the map plus each element as a key mapped to itself (`{"a":1,"1":1}`, measured) |
/// | List | List | concatenated |
/// | List | Map | the list plus one single-entry map per entry (`addAll(map.entrySet())`) |
/// | array | array | concatenated |
/// | other | | `The object being filtered is not a Map/List/Array` |
///
/// **The merged map keeps insertion order here** where Kestra's is a
/// `java.util.HashMap`, in bucket order; the crate's standing ruling (see
/// [`replace`]) is not to port `String.hashCode`'s layout, and every corpus
/// entry agrees. A key here is a String, so a list element merged into a map
/// is keyed by its Java text (`1` and `'1'` are one key here, two there).
fn merge(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    strict_undefined("merge", &value)?;
    let args = Args::new(rest);
    let items = args.get("items", 0).unwrap_or_else(|| MjValue::from(()));
    strict_undefined("merge", &items)?;
    if is_java_null(&value) {
        if is_java_null(&items) {
            return Err(pebble_error("The two arguments to be merged are null"));
        }
        return Ok(items);
    }
    if is_java_null(&items) {
        return Ok(value);
    }
    let arg_class = |v: &MjValue| match v.kind() {
        ValueKind::Seq | ValueKind::Iterable => match java_seq_kind(v) {
            JavaSeqKind::Array => "[Ljava.lang.String;".to_string(),
            JavaSeqKind::Collection => java_seq_kind(v).description().to_string(),
            _ => java_class_name(v),
        },
        _ => java_class_name(v),
    };
    let arg_is_list = matches!(items.kind(), ValueKind::Seq | ValueKind::Iterable)
        && java_seq_kind(&items).is_list();
    if let Some(mut entries) = as_map(&value) {
        if let Some(more) = as_map(&items) {
            entries.extend(more);
        } else if arg_is_list {
            for item in as_list(&items).unwrap_or_default() {
                entries.push((kestra_to_string(&item), item));
            }
        } else {
            return Err(pebble_error(format!(
                "Currently, only Maps and Lists can be merged with a Map. Arg2: {}",
                arg_class(&items)
            )));
        }
        return Ok(MjValue::from_object(OrderedMap::new(entries)));
    }
    let kind = java_seq_kind(&value);
    let Some(mut list) = as_list(&value) else {
        return Err(pebble_error(
            "The object being filtered is not a Map/List/Array",
        ));
    };
    match kind {
        JavaSeqKind::Collection => Err(pebble_error(
            "The object being filtered is not a Map/List/Array",
        )),
        JavaSeqKind::Array => {
            if java_seq_kind(&items) == JavaSeqKind::Array {
                list.extend(as_list(&items).unwrap_or_default());
                Ok(java_seq(JavaSeqKind::Array, list))
            } else {
                Err(pebble_error(format!(
                    "Currently, only Arrays of the same component class can be merged. \
                     Arg1: java.lang.String, Arg2: {}",
                    arg_class(&items)
                )))
            }
        }
        _ => {
            if let Some(entries) = as_map(&items) {
                list.extend(
                    entries.into_iter().map(|(key, item)| {
                        MjValue::from_object(OrderedMap::new(vec![(key, item)]))
                    }),
                );
            } else if arg_is_list {
                list.extend(as_list(&items).unwrap_or_default());
            } else {
                return Err(pebble_error(format!(
                    "Currently, only Maps and Lists can be merged with a List. Arg2: {}",
                    arg_class(&items)
                )));
            }
            Ok(MjValue::from(list))
        }
    }
}

/// `NumberFormatFilter` — `java.text.DecimalFormat` (see
/// [`super::decimalformat`]): with a `format` pattern, `new
/// DecimalFormat(format, symbols)`; without one, `NumberFormat.getInstance`,
/// whose pattern is `#,##0.###` (`1234.5678` is `1,234.568`, measured).
///
/// **The locale is pinned to `en`** where Kestra uses the JVM default (spec
/// D13's recorded divergence); the corpus ran on an `en_VN` JVM, whose number
/// symbols are `en`'s, so no golden entry shows it.
///
/// A non-`Number` input fails with Pebble's text; a `format` that is not a
/// String is a cast failure; a pattern Java refuses fails with Java's
/// `IllegalArgumentException` message.
fn numberformat(value: MjValue, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    use super::decimalformat::{DecimalFormat, JavaNumber};
    strict_undefined("numberformat", &value)?;
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let number = if let Some(class) = big_number_class(&value) {
        let canonical = big_number_text(&value).unwrap_or_default();
        if class == "java.math.BigInteger" {
            let (negative, magnitude) = match canonical.strip_prefix('-') {
                Some(rest) => (true, rest.to_string()),
                None => (false, canonical.clone()),
            };
            let long_path = canonical.parse::<i64>().is_ok();
            JavaNumber::Integer {
                negative,
                magnitude,
                long_path,
            }
        } else {
            JavaNumber::Decimal(canonical)
        }
    } else if value.kind() == ValueKind::Number {
        if value.is_integer() {
            let (negative, magnitude) = match i128::try_from(value.clone()) {
                Ok(n) => (n < 0, n.unsigned_abs().to_string()),
                Err(_) => (
                    false,
                    u128::try_from(value.clone())
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                ),
            };
            let long_path = i64::try_from(value.clone()).is_ok();
            JavaNumber::Integer {
                negative,
                magnitude,
                long_path,
            }
        } else {
            JavaNumber::Double(f64::try_from(value.clone()).unwrap_or(f64::NAN))
        }
    } else {
        return Err(pebble_error(
            "The input for the 'NumberFormat' filter has to be a number.",
        ));
    };
    let args = Args::new(rest);
    let format = match args.get("format", 0) {
        Some(pattern) => pebble_string_cast(&pattern)?,
        None => None,
    };
    let formatter = match format {
        Some(pattern) => DecimalFormat::parse(&pattern).map_err(pebble_error)?,
        None => DecimalFormat::english_default(),
    };
    Ok(MjValue::from(formatter.format(&number)))
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
pub(super) struct DateArgs {
    format: String,
    zone: Zone,
    existing_format: Option<String>,
    locale: JavaLocale,
}

impl DateArgs {
    pub(super) fn read(args: &Args, base: usize) -> Result<Self, MjError> {
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
    pub(super) fn render(&self, zdt: &Zdt) -> Result<String, MjError> {
        Format::resolve(&self.format)
            .and_then(|format| format.format(zdt, &self.zone, self.locale))
            .map_err(pebble_error)
    }
}

/// `(String) args.get(name)` — absent and explicitly-null are the same thing
/// here (Kestra uses a plain `get`, not `containsKey`), and a non-string is a
/// `ClassCastException`.
pub(super) fn optional_string(
    args: &Args,
    name: &str,
    index: usize,
) -> Result<Option<String>, MjError> {
    match args.get(name, index) {
        None => Ok(None),
        Some(value) if is_java_null(&value) => Ok(None),
        Some(value) => Ok(Some(cast_to_string(&value)?)),
    }
}

/// Java's `(String) x` on an argument that is not one.
pub(super) fn cast_to_string(value: &MjValue) -> Result<String, MjError> {
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
pub(super) fn to_json(value: MjValue) -> Result<MjValue, MjError> {
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

    // The input is carried as TEXT, not as a `jaq_json::Val`: `Val` is
    // `Rc`-based and therefore not `Send`, and the whole jq call happens on
    // its own thread (see [`run_jq`]). Which of the two readings applies is
    // decided here, where the value's type is still visible.
    let input = match value.as_str() {
        Some(text) => JqInput::ReadTree(text.to_string()),
        None => {
            let mut json = String::new();
            write_json(&mut json, &value, true);
            JqInput::Json(json)
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
fn run_jq(expression: &str, input: JqInput) -> Result<Vec<MjValue>, JqFailure> {
    // Before anything else, because everything else is what overflows.
    jqsandbox::check_source(expression).map_err(|e| JqFailure::Refused(e.0))?;

    // **On its own thread, with an explicit stack.** The parser's cost is a
    // property of the source, and `jqsandbox::MAX_SOURCE_BYTES` is derived
    // from THIS stack size -- so the bound must not depend on whether the
    // caller was a 2 MiB tokio worker or an 8 MiB main thread. See
    // `runners::jqsandbox`'s module doc for the measured cost per byte.
    let expression = expression.to_string();
    let worker = std::thread::Builder::new()
        .name("egret-jq".to_string())
        .stack_size(jqsandbox::JQ_STACK_BYTES)
        .spawn(move || run_jq_on_this_thread(&expression, input))
        .map_err(|_| JqFailure::Failed)?;
    // A panic on the worker is a failure like any other; it cannot be a
    // silent success.
    let outputs = worker.join().map_err(|_| JqFailure::Failed)??;
    Ok(outputs.into_iter().map(jaq_value).collect())
}

/// The body of [`run_jq`], on the jq thread.
///
/// Returns [`JqOut`] rather than `minijinja::Value` because the latter is not
/// `Send`; the conversion happens back on the caller's thread.
fn run_jq_on_this_thread(expression: &str, input: JqInput) -> Result<Vec<JqOut>, JqFailure> {
    jqsandbox::start_run();

    let input = match input {
        JqInput::ReadTree(text) => read_tree(&text).ok_or(JqFailure::Failed)?,
        JqInput::Json(json) => {
            jaq_json::read::parse_single(json.as_bytes()).map_err(|_| JqFailure::Failed)?
        }
    };

    let program = jaq_core::load::File {
        code: expression,
        path: (),
    };

    // `jqsandbox` owns both lists: jaq's own unbounded `repeat`/`recurse`/
    // `while`/`until`/`range` definitions are spliced out for bounded ones,
    // and the native list is an allow-list with six host-reading names
    // withheld and a per-step deadline added. Read that module before
    // changing either.
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
        // **Before anything touches it.** `to_string()` below, `jq_out`
        // after it, `jaq_value` back on the caller's thread and the DROP of
        // all three are recursive over this value; the caller's thread is a
        // 2 MiB tokio worker and none of `runners::jqsandbox`'s other bounds
        // reaches this axis. `too_deep` is itself iterative, because a
        // recursive depth check is the same overflow one function earlier.
        if jqsandbox::too_deep(&value) {
            return Err(JqFailure::Refused(jqsandbox::too_deep_message()));
        }
        bytes = bytes
            .saturating_add(value.to_string().len())
            .saturating_add(1);
        // This bounds a generator that EMITS. It is not the bound that stops
        // `[repeat(1)]` or `[range(0;1;0)]` -- neither reaches here, and
        // `runners::jqsandbox` is what stops them.
        if bytes > MAX_OUTPUT_BYTES {
            return Err(JqFailure::Failed);
        }
        outputs.push(jq_out(value));
    }
    Ok(outputs)
}

/// A jq input, as text, because `jaq_json::Val` is `Rc`-based and not `Send`.
///
/// The two readings are not interchangeable: a String input goes through
/// `ObjectMapper.readTree`, which takes the FIRST JSON value and tolerates an
/// empty document, and anything else is already JSON.
enum JqInput {
    ReadTree(String),
    Json(String),
}

/// One jq output, in a form that can cross a thread boundary.
///
/// `minijinja::Value` is not `Send`, and a JSON round trip would lose a big
/// integer, so this is the same shape [`jaq_value`] used to build directly.
enum JqOut {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Text(String),
    Arr(Vec<JqOut>),
    Obj(Vec<(String, JqOut)>),
}

fn jq_out(value: jaq_json::Val) -> JqOut {
    match value {
        jaq_json::Val::Null => JqOut::Null,
        jaq_json::Val::Bool(b) => JqOut::Bool(b),
        jaq_json::Val::Num(n) => match n {
            jaq_json::Num::Int(i) => JqOut::Int(i as i128),
            jaq_json::Num::Float(f) => JqOut::Float(f),
            // A big integer or a decimal keeps its text; both are outside
            // what an `i64` holds and neither has a corpus entry.
            other => match other.to_string().parse::<i128>() {
                Ok(i) => JqOut::Int(i),
                Err(_) => match other.to_string().parse::<f64>() {
                    Ok(f) => JqOut::Float(f),
                    Err(_) => JqOut::Text(other.to_string()),
                },
            },
        },
        jaq_json::Val::BStr(s) | jaq_json::Val::TStr(s) => {
            JqOut::Text(String::from_utf8_lossy(&s).into_owned())
        }
        jaq_json::Val::Arr(items) => JqOut::Arr(items.iter().cloned().map(jq_out).collect()),
        jaq_json::Val::Obj(entries) => JqOut::Obj(
            entries
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        jaq_json::Val::BStr(s) | jaq_json::Val::TStr(s) => {
                            String::from_utf8_lossy(s).into_owned()
                        }
                        other => other.to_string(),
                    };
                    (key, jq_out(v.clone()))
                })
                .collect(),
        ),
    }
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
fn jaq_value(value: JqOut) -> MjValue {
    match value {
        JqOut::Null => MjValue::from(()),
        JqOut::Bool(b) => MjValue::from(b),
        JqOut::Int(i) => MjValue::from(i),
        JqOut::Float(f) => MjValue::from(f),
        JqOut::Text(s) => MjValue::from(s),
        JqOut::Arr(items) => MjValue::from(items.into_iter().map(jaq_value).collect::<Vec<_>>()),
        JqOut::Obj(entries) => MjValue::from_object(OrderedMap::new(
            entries
                .into_iter()
                .map(|(k, v)| (k, jaq_value(v)))
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

/// One output of a `concatJqExpression` (`http.SseRequest`), as
/// `SseRequest.java:168-179` reads it: a text, number or boolean node becomes
/// its text (a number as `numberValue().toString()`, so a double is
/// `Double.toString`), and anything else -- null, an array, an object -- is
/// the "Expected text value" failure, named by Jackson's node type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JqScalar {
    Text(String),
    NotText(&'static str),
}

/// What one input gave a [`jq_each`] program.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct JqEach {
    /// The outputs, in order, up to and including the first
    /// [`JqScalar::NotText`] (Kestra's lambda throws there, so nothing after
    /// it is seen).
    pub outputs: Vec<JqScalar>,
    /// jq raised an error on this input (Kestra: the catch around `apply`).
    pub failed: bool,
    /// egret's own bound stopped the program (a `jq: ` refusal): not the
    /// author's data, so the task fails whatever `failedOnMissingJq` says.
    pub refused: Option<String>,
}

/// The total time [`jq_each`] may spend over all of a stream's inputs.
/// The per-input deadline ([`jqsandbox`]'s 1 s) restarts for each input, so
/// without this a 10 000-event stream could keep the jq thread busy for
/// hours (final review I3). It is checked before each input, so a run ends
/// at most one per-input deadline past it.
pub(crate) const JQ_EACH_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

/// `http.SseRequest`'s `concatJqExpression`: one program, compiled ONCE and
/// run over every input (each a JSON text), on one thread with the jq stack
/// and every [`jqsandbox`] bound `run_jq` applies, the deadline restarted per
/// input, and [`JQ_EACH_BUDGET`] over all of them. `Err` when the program
/// does not compile or is refused, or the budget is spent, with the reason
/// (Kestra fails the task at `JsonQuery.compile`).
///
/// **Async, and it never blocks the runtime** (final review I3): the jq
/// thread hands its result back through a oneshot channel that this future
/// awaits, so a tokio worker is never held in a `join`. Dropping the future
/// (the task's `timeout:` or a kill) closes the channel, and the jq thread
/// stops before its next input.
pub(crate) async fn jq_each(expression: &str, inputs: Vec<String>) -> Result<Vec<JqEach>, String> {
    jq_each_within(expression, inputs, JQ_EACH_BUDGET).await
}

/// [`jq_each`] with the total budget given.
pub(crate) async fn jq_each_within(
    expression: &str,
    inputs: Vec<String>,
    budget: std::time::Duration,
) -> Result<Vec<JqEach>, String> {
    jqsandbox::check_source(expression).map_err(|e| e.0)?;
    let expression = expression.to_string();
    let deadline = std::time::Instant::now().checked_add(budget);
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("egret-jq".to_string())
        .stack_size(jqsandbox::JQ_STACK_BYTES)
        .spawn(move || {
            let result =
                jq_each_on_this_thread(&expression, inputs, deadline, budget, &|| tx.is_closed());
            let _ = tx.send(result);
        })
        .map_err(|e| format!("jq: could not start the jq thread: {e}"))?;
    rx.await
        .map_err(|_| "jq: the jq program panicked".to_string())?
}

fn jq_each_on_this_thread(
    expression: &str,
    inputs: Vec<String>,
    deadline: Option<std::time::Instant>,
    budget: std::time::Duration,
    abandoned: &dyn Fn() -> bool,
) -> Result<Vec<JqEach>, String> {
    let program = jaq_core::load::File {
        code: expression,
        path: (),
    };
    let loader = jaq_core::load::Loader::new(jqsandbox::definitions());
    let arena = jaq_core::load::Arena::default();
    let not_compiled = || format!("{expression:?} is not a valid jq expression");
    let modules = loader.load(&arena, program).map_err(|_| not_compiled())?;
    let filter = jaq_core::Compiler::default()
        .with_funs(jqsandbox::natives())
        .compile(modules)
        .map_err(|_| not_compiled())?;

    let mut out = Vec::with_capacity(inputs.len());
    for json in inputs {
        if abandoned() {
            return Err("jq: the task stopped waiting for the jq program".to_string());
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            return Err(format!(
                "jq: the program ran for more than its total budget of {} seconds over the \
                 stream's events",
                budget.as_secs()
            ));
        }
        jqsandbox::start_run();
        let mut each = JqEach::default();
        let Ok(input) = jaq_json::read::parse_single(json.as_bytes()) else {
            each.failed = true;
            out.push(each);
            continue;
        };
        let ctx = jaq_core::Ctx::<jqsandbox::Data>::new(&filter.lut, jaq_core::Vars::new([]));
        let mut bytes = 0usize;
        for result in filter.id.run((ctx, input)) {
            let value = match result {
                Ok(v) => v,
                Err(exn) => {
                    if let Ok(error) = exn.get_err() {
                        let message = error.to_string();
                        let unquoted = message.trim_matches('"');
                        if unquoted.starts_with(EGRET_JQ_PREFIX) {
                            each.refused = Some(unquoted.to_string());
                        }
                    }
                    each.failed = true;
                    break;
                }
            };
            let scalar = match value {
                jaq_json::Val::BStr(s) | jaq_json::Val::TStr(s) => {
                    JqScalar::Text(String::from_utf8_lossy(&s).into_owned())
                }
                jaq_json::Val::Bool(b) => JqScalar::Text(b.to_string()),
                jaq_json::Val::Num(n) => JqScalar::Text(java_number_text(&n)),
                jaq_json::Val::Null => JqScalar::NotText("NULL"),
                jaq_json::Val::Arr(_) => JqScalar::NotText("ARRAY"),
                jaq_json::Val::Obj(_) => JqScalar::NotText("OBJECT"),
            };
            if let JqScalar::Text(t) = &scalar {
                bytes = bytes.saturating_add(t.len());
                if bytes > MAX_OUTPUT_BYTES {
                    each.refused = Some(format!(
                        "{EGRET_JQ_PREFIX}one event's results are over {MAX_OUTPUT_BYTES} bytes"
                    ));
                    each.failed = true;
                    break;
                }
            }
            let stop = matches!(scalar, JqScalar::NotText(_));
            each.outputs.push(scalar);
            if stop {
                break;
            }
        }
        out.push(each);
    }
    Ok(out)
}

/// `NumericNode.numberValue().toString()`: an integer as written, a double
/// as `Double.toString` (a decimal literal is a `DoubleNode` in Jackson).
fn java_number_text(n: &jaq_json::Num) -> String {
    match n {
        jaq_json::Num::Int(i) => i.to_string(),
        jaq_json::Num::BigInt(i) => i.to_string(),
        jaq_json::Num::Float(f) => {
            let mut s = String::new();
            write_java_double(&mut s, *f);
            s
        }
        jaq_json::Num::Dec(d) => match d.parse::<f64>() {
            Ok(f) => {
                let mut s = String::new();
                write_java_double(&mut s, f);
                s
            }
            Err(_) => d.to_string(),
        },
    }
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
/// `filter-regexextract-group-one` and `-group-two` are both kept
/// deliberately. `-group-two` alone does catch an off-by-one in the group
/// index, so the pair is redundant against that one defect; it is not
/// redundant against a change that reads group 1 correctly and group 2 by a
/// different path, and two goldens is what that costs.
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
    use super::{is_java_null, MjValue};

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

    /// The budget is spent by the whole RENDER, so a `{% for %}` cannot
    /// multiply it.
    ///
    /// **A per-call cap is not a bound.** Measured before this changed, with
    /// a loop whose per-iteration output is empty — so
    /// [`super::env::MAX_OUTPUT_BYTES`] stays satisfied for ever — 20 / 80 /
    /// 320 iterations cost 267 ms / 1.05 s / 4.42 s and produced zero bytes,
    /// and `range()` caps at a million, so one template reached hours. The
    /// first cut's adversarial sweep had 21 cases in it and **not one of them
    /// contained a loop**, which is why a per-call cap looked sufficient.
    ///
    /// Verified by removing the reset in [`ReplaceBudget::install`], not by
    /// asserting it: the loop below then runs to completion.
    #[test]
    fn replace_is_bounded_across_a_whole_render() {
        // Each iteration is well inside the per-call cap on its own -- 2 000
        // pairs over 64 KiB is 128 MiB, an eighth of the budget -- so only a
        // render-wide budget refuses this.
        let pairs: Vec<String> = (0..2_000).map(|i| format!("\"k{i}\": \"\"")).collect();
        let context = format!(
            r#"{{"s": {}, "m": {{{}}}}}"#,
            serde_json::to_string(&"z".repeat(64 * 1024)).unwrap(),
            pairs.join(",")
        );
        let message = render(
            "{% for i in range(start=1, end=64) %}{{ (s | replace(m)) | length > 0 }}{% endfor %}",
            &context,
        )
        .expect_err("a loop must not multiply the budget");
        assert!(message.contains("in one render"), "{message}");

        // The accepted neighbour: the same filter, the same map, few enough
        // iterations to stay inside one render's budget.
        assert!(render(
            "{% for i in range(start=1, end=4) %}{{ (s | replace(m)) | length > 0 }}{% endfor %}",
            &context,
        )
        .is_ok());

        // And a fresh render starts with a full budget, which is what
        // `ReplaceBudget`'s reset is for -- without it the second call here
        // would inherit the first's exhaustion.
        assert!(render(
            "{% for i in range(start=1, end=4) %}{{ (s | replace(m)) | length > 0 }}{% endfor %}",
            &context,
        )
        .is_ok());
    }

    /// The sixteen ported filters refuse an **undefined** input, which fix
    /// round 1 broke in all sixteen at once.
    ///
    /// Pairs `filter-upper-undefined-root`/`-attribute` and the same pair for
    /// `length`, `sort`, `split` and `join`, plus
    /// `filter-length-of-an-undefined`.
    ///
    /// # What this is guarding against, and why the entries alone cannot
    ///
    /// Kestra runs with `strictVariables(true)`: a missing root attribute and
    /// a missing attribute both raise. Before the ports, egret's built-ins
    /// raised too, so the two agreed. Fix round 1 applied
    /// [`is_java_null`] — which folded `Undefined` into Kestra's null until
    /// W2b — at the
    /// **entry point** of every ported filter, and 28 measured cases turned
    /// from an error into an answer: `{{ nothere | upper }}` became `""` and
    /// `{{ obj.missing | length }}` became `"0"`. In a flow that turns a
    /// typo'd variable name from a hard failure into an empty string.
    ///
    /// The corpus entries are `error-class`, which is satisfied by *any*
    /// failure — so what they pin is only that egret still refuses. This pins
    /// what it says, and pins the two exceptions:
    ///
    /// - **`default` must NOT refuse.** Pebble's filter-invocation expression
    ///   catches the attribute exception for `default` specifically, so
    ///   `{{ nothere | default('D') }}` is `D` on Kestra. Measured, both for a
    ///   root name and for an attribute.
    /// - a **context** null still reaches the null branch, because it is a
    ///   `ValueKind::None` and not an undefined.
    #[test]
    fn the_ported_filters_refuse_an_undefined_input() {
        let cx = r#"{"obj": {"a": 1}, "d": ",", "nul": null,
            "str": "a b", "low": "a b", "pad": "  a b  ", "tag": "<i>",
            "list": [1, 2, 3], "unsorted": [2, 3, 1], "csv": "a,b", "fmt": "%s-x"}"#;
        for filter in [
            "upper",
            "lower",
            "trim",
            "capitalize",
            "title",
            "escape",
            "length",
            "first",
            "last",
            "join('-')",
            "sort",
            "reverse",
            "abs",
            "split(d)",
            "slice(0,1)",
            "format('x')",
            "replace({'a':'b'})",
        ] {
            for subject in ["nothere", "obj.missing"] {
                let template = format!("<{{{{ {subject} | {filter} }}}}>");
                let message = render(&template, cx).unwrap_or_else(|message| message);
                assert!(
                    render(&template, cx).is_err(),
                    "{template} must be refused; it answered {message:?}"
                );
                // **No `|| contains("Can not pass null value")` here, and its
                // removal is the point of W2b.** That alternative was reachable
                // while `is_java_null` folded an `Undefined` in, so this test --
                // whose whole subject is that a typo is NOT a null -- accepted a
                // typo being reported as a null. It is unreachable at head, and
                // leaving it would let the fold come back with this test green.
                assert!(
                    message.contains("an undefined value reached"),
                    "{template} must name the UNDEFINED, not a null: {message}"
                );
            }
        }

        // **The exception, and it is Pebble's own.** The filter-invocation
        // expression catches the attribute exception for `default`, so the
        // fallback works on a name that does not exist -- measured for a root
        // name and for an attribute.
        assert_eq!(render("<{{ nothere | default('D') }}>", cx).unwrap(), "<D>");
        assert_eq!(
            render("<{{ obj.missing | default('D') }}>", cx).unwrap(),
            "<D>"
        );

        // A CONTEXT null is a `ValueKind::None`, not an undefined, and still
        // reaches the filter's own null branch.
        assert_eq!(render("<{{ nul | upper }}>", cx).unwrap(), "<>");
        assert_eq!(render("{{ nul | length }}", cx).unwrap(), "0");

        // **And the other half of the pair, in the same test.** Until W2b the
        // `null` LITERAL arrived as an undefined too, so every line above cost
        // a literal that Kestra renders: this assertion read
        // `assert!(render("{{ null | upper }}", cx).is_err())`. Now
        // `super::super::rewrite` translates the literal to minijinja's `none`
        // and the two are different values, so the seventeen refusals above
        // and the sixteen renders below hold **together** — which is the thing
        // neither W2 Task 8's fix round 1 nor its fix round 2 could have.
        //
        // `abs` is the seventeenth and is deliberately absent from this list:
        // Kestra REFUSES a null there (`Can not pass null value to "abs"
        // filter.`), measured against the artifact, which is why sixteen bank
        // and not seventeen. `the_null_literal_and_a_typo_get_different_refusals`
        // is its test.
        // **Each row carries a live input as well as the null**, because
        // `<>` on its own is satisfied by a filter that ignores its input and
        // returns the empty string -- and three of these (`escape`,
        // `format('x')`, `replace({'a':'b'})`) really could. The live column is
        // what separates "answered the null correctly" from "answers `<>` to
        // everything". **All sixteen also have a `filter-*-literal-null` corpus
        // entry** -- this said fourteen, which is the number W2b ADDED;
        // `filter-upper-literal-null` and `filter-length-literal-null` were
        // banked out of `baseline.txt`, not out of `corpus.json`. The live
        // column is what makes the test carry its own weight rather than
        // borrowing all of it from the corpus.
        //
        // **Every live value below was measured on the artifact, not guessed.**
        // Five of the first six written from memory were wrong -- a list
        // renders through `JsonWriter` as `[1,2,3]` and not
        // `AbstractCollection.toString`'s `[1, 2]`, `split` writes
        // `["a","b"]`, `'%s-x' | format('x')` is `x-x` because the `%s`
        // consumes the argument, and `'a b' | replace({'a':'b'})` is `b b`.
        // All sixteen agree between the two engines.
        for (filter, on_null, live_input, on_live) in [
            ("upper", "<>", "str", "<A B>"),
            ("lower", "<>", "str", "<a b>"),
            ("trim", "<>", "pad", "<a b>"),
            ("capitalize", "<>", "low", "<A b>"),
            ("title", "<>", "low", "<A B>"),
            ("escape", "<>", "tag", "<&lt;i&gt;>"),
            ("length", "<0>", "str", "<3>"),
            ("first", "<>", "list", "<1>"),
            ("last", "<>", "list", "<3>"),
            ("join('-')", "<>", "list", "<1-2-3>"),
            ("sort", "<>", "unsorted", "<[1,2,3]>"),
            ("reverse", "<>", "list", "<[3,2,1]>"),
            ("split(d)", "<>", "csv", "<[\"a\",\"b\"]>"),
            ("slice(0,1)", "<>", "list", "<[1]>"),
            ("format('x')", "<>", "fmt", "<x-x>"),
            ("replace({'a':'b'})", "<>", "low", "<b b>"),
        ] {
            let template = format!("<{{{{ null | {filter} }}}}>");
            assert_eq!(
                render(&template, cx).unwrap_or_else(|e| panic!("{template}: {e}")),
                on_null,
                "{template}"
            );

            let live = format!("<{{{{ {live_input} | {filter} }}}}>");
            assert_eq!(
                render(&live, cx).unwrap_or_else(|e| panic!("{live}: {e}")),
                on_live,
                "{live} -- if this is `{on_null}` the filter is ignoring its input"
            );
            assert_ne!(on_null, on_live, "{filter}: the live row must discriminate");
        }
    }

    /// `abs` is the one of the seventeen where Kestra refuses a null, and the
    /// two refusals egret can now give there are **different sentences**.
    ///
    /// Pairs `filter-abs-literal-null`, whose golden is a Java exception and
    /// which is therefore `error-class` — a mode satisfied by any failure at
    /// all. Before W2b both inputs below produced [`strict_undefined`]'s
    /// message, because the literal and the typo were the same value; that
    /// entry would have been "born matching" for the wrong reason. This is
    /// what makes it answerable, and it fails if the rewriter's translation is
    /// removed.
    #[test]
    fn the_null_literal_and_a_typo_get_different_refusals() {
        let cx = r#"{"obj": {"a": 1}}"#;

        let literal = render("{{ null | abs }}", cx).expect_err("a null literal must be refused");
        assert!(
            literal.contains(r#"Can not pass null value to "abs" filter."#),
            "the LITERAL must name the null: {literal}"
        );

        for subject in ["nothere", "obj.missing"] {
            let template = format!("{{{{ {subject} | abs }}}}");
            let typo = render(&template, cx).expect_err("a typo must be refused");
            assert!(
                typo.contains("an undefined value reached"),
                "{template} must name the UNDEFINED: {typo}"
            );
        }

        // A CONTEXT null takes the literal's route, because after the rewrite
        // they are the same value -- which is the point.
        let from_context = render("{{ n | abs }}", r#"{"n": null}"#).expect_err("also refused");
        assert!(from_context.contains(r#"Can not pass null value to "abs" filter."#));
    }

    /// Pebble's null literal is **exactly four spellings**, and the rule is not
    /// case-insensitivity.
    ///
    /// Pairs `null-literal-mixed-case-is-not-the-literal`. Measured through the
    /// oracle harness against the pinned artifact, one probe per spelling:
    /// `null`, `NULL`, `none` and `NONE` render empty and compare `== none` as
    /// `true`; `None`, `Null`, `nUll`, `NoNe` and `nulL` are every one of them
    /// a `RootAttributeNotFoundException`, i.e. ordinary variable names.
    ///
    /// A `matches!` widened to `eq_ignore_ascii_case` would turn all five of
    /// those refusals into answers, and only this test would notice — the
    /// corpus entry is `error-class`, so it is satisfied by any failure.
    ///
    /// **`None` and `True` are the exception, and they are recorded rather
    /// than fixed**: minijinja's own lexer binds Python's spellings as
    /// constants, so egret answers where Kestra raises. No rewrite of a *name*
    /// can reach a keyword of the lexer it feeds.
    /// `null-literal-python-spelling-is-not-the-literal` and
    /// `boolean-literal-python-spelling-is-not-the-literal` are baselined.
    #[test]
    fn the_null_literal_is_exactly_four_spellings() {
        for spelling in ["null", "NULL", "none", "NONE"] {
            let template = format!("<{{{{ {spelling} }}}}>");
            assert_eq!(
                render(&template, "{}").unwrap_or_else(|e| panic!("{template}: {e}")),
                "<>",
                "{template}"
            );
            let compared = format!("{{{{ {spelling} == none }}}}");
            assert_eq!(render(&compared, "{}").unwrap(), "true", "{compared}");
        }

        // **The message, not just `is_err()`.** The paired corpus entry is
        // `error-class`, which any failure satisfies, so `is_err()` alone left
        // NOTHING pinning what egret says: a rewriter that renamed a mixed-case
        // spelling into something unparseable would pass all four assertions
        // and the entry. What Kestra raises is a
        // `RootAttributeNotFoundException` -- an absent VARIABLE -- and
        // minijinja's strict mode says `undefined value` for exactly that.
        for near_miss in ["Null", "nUll", "NoNe", "nulL"] {
            let template = format!("<{{{{ {near_miss} }}}}>");
            let message = render(&template, "{}").expect_err(&format!(
                "{template} is a VARIABLE on Kestra and must be refused"
            ));
            assert!(
                message.contains("undefined value"),
                "{template} must be refused as an ABSENT VARIABLE, not by some \
                 other accident: {message}"
            );
        }

        // Recorded, not fixed. Both are ordinary absent variables on Kestra.
        assert_eq!(render("<{{ None }}>", "{}").unwrap(), "<>");
        assert_eq!(render("<{{ True }}>", "{}").unwrap(), "<true>");
    }

    /// Narrowing [`is_java_null`] to `is_none()` is what makes sixteen MORE
    /// filters refuse a typo'd variable, and without this test that narrowing
    /// is an equivalent mutant.
    ///
    /// **Measured, and it is the reason this test exists.** Restoring the old
    /// body (`value.is_none() || value.is_undefined()`) left the whole crate
    /// GREEN — 412 lib tests and all ten oracle tests — because the seventeen
    /// filters that pin the undefined half run [`strict_undefined`] *before*
    /// this predicate, so an undefined never reaches it there, and the filters
    /// that do reach it had no entry. The change was right and nothing could
    /// see it.
    ///
    /// These sixteen are what can. Kestra raises for every one of them under
    /// `strictVariables(true)`; before the narrowing egret ANSWERED — `chunk`
    /// gave `<>`, `distinct` gave `<null>`, `md5` gave the hash of nothing —
    /// because the undefined reached each filter's null branch. Measured as a
    /// 105-case sweep over all 35 registered filters x {null literal, missing
    /// root, missing attribute}: 70 undefined cases, **34** of which changed
    /// class from "answers" to "refuses" and none of which changed the other
    /// way, leaving 36. (This doc said 32 and 38; the numbers came from a build
    /// that did not yet carry `string`'s guard, and they contradicted the
    /// eighteen names listed below -- 18 x 2 is 36, not 38. Re-measured.)
    ///
    /// The message is each filter's own type refusal rather than
    /// `strict_undefined`'s sentence, and that is deliberate: these sixteen
    /// were not part of W2 Task 8's port and W2b did not add a guard to them.
    /// What is asserted is the CLASS — a refusal where Kestra refuses — which
    /// is exactly what the narrowing bought.
    ///
    /// **The other eighteen are recorded, not fixed.** `className`, `endsWith`,
    /// `escapeChar`, `indent`, `jq`, `nindent`, `regexExtract`, `regexMatch`,
    /// `regexReplace`, `slugify`, `startsWith`, `substringAfter`,
    /// `substringAfterLast`, `substringBefore`, `substringBeforeLast`, `toIon`,
    /// `toJson` and `yaml` still answer for an undefined input, by a route that
    /// does not go through this predicate. `oracle/README.md` carries the list
    /// and the sweep. `string` was the nineteenth and closes here, because
    /// `oracle/baseline.txt` named the rewriter's owner as the one to fix it.
    #[test]
    fn narrowing_java_null_is_what_makes_sixteen_more_filters_refuse_a_typo() {
        let cx = r#"{"obj": {"a": 1}, "nul": null}"#;
        for filter in [
            "chunk(size=2)",
            "date('iso', timeZone='UTC')",
            "dateAdd(1,'DAYS','iso','UTC')",
            "distinct",
            "flatten",
            "keys",
            "md5",
            "number(type='INT')",
            "sha1",
            "sha512",
            "timestamp",
            "timestampMicro",
            "timestampMilli",
            "timestampNano",
            "urldecode",
            "values",
        ] {
            for subject in ["nothere", "obj.missing"] {
                let template = format!("<{{{{ {subject} | {filter} }}}}>");
                let answered = render(&template, cx).unwrap_or_default();
                assert!(
                    render(&template, cx).is_err(),
                    "{template} must be refused, as Kestra does; it answered {answered:?}"
                );
            }

            // And the null LITERAL still renders through the same filter, which
            // is the half that folding an undefined back in would keep working
            // -- so the pair above and this line together are what pin the
            // narrowing rather than either alone.
            let literal = format!("{{{{ null | {filter} }}}}");
            render(&literal, cx).unwrap_or_else(|e| panic!("{literal} must render: {e}"));
        }

        // The predicate's own contract, stated directly, because the sixteen
        // above reach it through sixteen different type checks.
        assert!(is_java_null(&MjValue::from(())), "a null IS Kestra's null");
        assert!(
            !is_java_null(&MjValue::UNDEFINED),
            "an undefined is NOT -- that is the whole of W2b at this line"
        );
    }

    /// `string` tells the literal from a typo, which is what banks
    /// `filter-string-undefined-input`.
    ///
    /// That entry has been baselined since W2 Task 4's third fix round, and
    /// the comment beside it in `oracle/baseline.txt` names the rewriter's
    /// owner as the person to fix it. It is `error-class` from W2b on, because
    /// its golden is a Java exception string; this is what makes it
    /// answerable.
    #[test]
    fn the_string_filter_now_tells_the_literal_from_a_typo() {
        let cx = r#"{"obj": {"a": 1}, "nul": null}"#;

        // The literal, and a context null, both render.
        assert_eq!(render("<{{ null | string }}>", cx).unwrap(), "<>");
        assert_eq!(render("<{{ nul | string }}>", cx).unwrap(), "<>");

        // A typo does not.
        for subject in ["typo", "obj.missing"] {
            let template = format!("<{{{{ {subject} | string }}}}>");
            let message = render(&template, cx).expect_err("a typo must be refused");
            assert!(
                message.contains("an undefined value reached the 'string' filter"),
                "{template}: {message}"
            );
        }
    }

    /// `format` refuses what Java refuses **and** what `super::javaformat` does
    /// not implement, and the two say different things.
    ///
    /// Pairs `filter-format-integer-through-a-float`,
    /// `filter-format-unknown-conversion` and
    /// `filter-format-trailing-percent`.
    #[test]
    fn the_format_filter_refuses_what_java_refuses_and_what_it_does_not_implement() {
        // Java's own refusals.
        assert!(render("{{ s | format(n) }}", r#"{"s": "%f", "n": 42}"#)
            .expect_err("an Integer is not a Double")
            .contains("java.lang.Integer != java.lang.Double"));
        assert!(render("{{ s | format('a') }}", r#"{"s": "%q"}"#)
            .expect_err("must be refused")
            .contains("Conversion"));
        assert!(render("{{ s | format('a') }}", r#"{"s": "abc%"}"#)
            .expect_err("a trailing % is not a specifier")
            .contains("Conversion = '%'"));
        assert!(render("{{ s | format('a') }}", r#"{"s": "%s-%s"}"#)
            .expect_err("a missing argument")
            .contains("Format specifier"));

        // egret's own, and they name the conversion so an author can act.
        for conversion in ["%g", "%a", "%c", "%h"] {
            let message = render(
                "{{ s | format(n) }}",
                &format!(r#"{{"s": "{conversion}", "n": 1.5}}"#),
            )
            .expect_err("must be refused");
            assert!(
                message.contains(&format!("'{conversion}' conversion is not implemented")),
                "{conversion}: {message}"
            );
        }

        // The accepted neighbours: every conversion that IS implemented.
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%.2f", "n": 1.5}"#),
            "1.50"
        );
        assert_eq!(ok("{{ s | format(n) }}", r#"{"s": "%d", "n": 42}"#), "42");
        assert_eq!(ok("{{ s | format(n) }}", r#"{"s": "%x", "n": 42}"#), "2a");
    }

    /// `java.util.Formatter`'s rules that a wrapper around minijinja's
    /// `format` got silently wrong.
    ///
    /// Five values, and fix round 1 shipped all five: two of them were the
    /// Python-spelling defect the round existed to kill, still live inside
    /// `format` because the wrapper mapped only a *null* argument and passed
    /// every other one through untouched.
    #[test]
    fn the_format_filter_is_javas_formatter_and_not_minijinjas() {
        // Two's complement at the argument's own width, which is the
        // Integer/Long split.
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%x", "n": -255}"#),
            "ffffff01"
        );
        assert_eq!(
            ok(
                "{{ s | format(n) }}",
                r#"{"s": "%x", "n": -9223372036854775808}"#
            ),
            "8000000000000000"
        );
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%o", "n": -8}"#),
            "37777777770"
        );
        // HALF_UP over the SHORTEST decimal representation, not half-to-even
        // over the exact binary value.
        assert_eq!(ok("{{ s | format(n) }}", r#"{"s": "%.0f", "n": 0.5}"#), "1");
        assert_eq!(ok("{{ s | format(n) }}", r#"{"s": "%.0f", "n": 1.5}"#), "2");
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%.2f", "n": 1.005}"#),
            "1.01"
        );
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%.2f", "n": 1e300}"#),
            format!("1{}.00", "0".repeat(300))
        );
        // `%s` goes through `kestra_to_string`, which is the whole reason the
        // wrapper was not enough.
        assert_eq!(
            ok("{{ s | format(b) }}", r#"{"s": "%s", "b": true}"#),
            "true"
        );
        assert_eq!(
            ok("{{ s | format(m) }}", r#"{"s": "%s", "m": {"a": 1}}"#),
            "{a=1}"
        );
        assert_eq!(
            ok("{{ s | format(l) }}", r#"{"s": "%s", "l": [1, 2]}"#),
            "[1, 2]"
        );
        assert_eq!(ok("{{ s | format('ab') }}", r#"{"s": "%S"}"#), "AB");
        // A null argument is the four characters `null`, through the STRING
        // path -- so precision truncates it and `%b` is the exception.
        assert_eq!(
            render("<{{ s | format(n) }}>", r#"{"s": "%.0f", "n": null}"#).unwrap(),
            "<>"
        );
        assert_eq!(
            render("{{ s | format(n) }}", r#"{"s": "%.2f", "n": null}"#).unwrap(),
            "nu"
        );
        assert_eq!(
            render("{{ s | format(n) }}", r#"{"s": "%b", "n": null}"#).unwrap(),
            "false"
        );
        // Flags, width and the argument index.
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%05d", "n": -255}"#),
            "-0255"
        );
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%(d", "n": -255}"#),
            "(255)"
        );
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%,d", "n": 1234567}"#),
            "1,234,567"
        );
        assert_eq!(
            ok("{{ s | format('a','b') }}", r#"{"s": "%2$s-%1$s"}"#),
            "b-a"
        );
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%e", "n": 1.23456}"#),
            "1.234560e+00"
        );
        assert_eq!(
            ok("{{ s | format('a') }}", r#"{"s": "100%% done"}"#),
            "100% done"
        );
    }

    /// `capitalize` and `title` use **Java's** `isWhitespace` and title case,
    /// and both of the first cut's predicates were wrong in both directions.
    #[test]
    fn the_word_boundary_filters_use_javas_own_tables() {
        // Java says NOT whitespace for the three non-breaking spaces, so the
        // space itself is the first "word" character and nothing is capitalised.
        for space in ['\u{a0}', '\u{2007}', '\u{202f}'] {
            let cx = format!(r#"{{"s": "\u{:04x}ab"}}"#, space as u32);
            assert_eq!(ok("{{ s | capitalize }}", &cx), format!("{space}ab"));
        }
        // and Java says IS whitespace for the four information separators,
        // which Rust does not.
        for sep in ['\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}'] {
            let cx = format!(r#"{{"s": "\u{:04x}ab"}}"#, sep as u32);
            assert_eq!(ok("{{ s | capitalize }}", &cx), format!("{sep}Ab"));
        }
        // The digraphs have a title case that is NOT their upper case.
        assert_eq!(
            ok("{{ s | capitalize }}", "{\"s\": \"\\u01c6x\"}"),
            "\u{1c5}x"
        );
        assert_eq!(ok("{{ s | title }}", "{\"s\": \"\\u01c9x\"}"), "\u{1c8}x");
        // `toTitleCase(char)` cannot expand, so a character whose uppercase is
        // two characters comes back unchanged.
        assert_eq!(
            ok("{{ s | capitalize }}", "{\"s\": \"\\u00dfx\"}"),
            "\u{df}x"
        );
        assert_eq!(
            ok("{{ s | capitalize }}", "{\"s\": \"\\ufb01x\"}"),
            "\u{fb01}x"
        );
        // ...except where a precomposed title-case character exists, which is
        // the Greek iota-subscript family.
        assert_eq!(
            ok("{{ s | capitalize }}", "{\"s\": \"\\u1f80x\"}"),
            "\u{1f88}x"
        );
    }

    /// [`JAVA_TITLE_CASE`] is DERIVED from the JVM's own dump, and this checks
    /// the derivation against all 1 149 code points it covers.
    ///
    /// The 35-entry table is "where `toTitleCase` is neither the
    /// single-character uppercase nor the character itself". That is a claim
    /// about Rust's `char::to_uppercase` as much as about Java, and neither
    /// half is checkable by reading. `oracle/fixtures/java-title-case.txt` is
    /// the JVM's answer for every mapped code point; this walks it.
    #[test]
    fn title_case_matches_the_jvm_over_every_code_point_it_maps() {
        const DUMP: &str = include_str!("../../oracle/fixtures/java-title-case.txt");
        let mut checked = 0usize;
        for pair in DUMP
            .lines()
            .filter(|l| !l.starts_with('#'))
            .flat_map(|l| l.split(','))
            .filter(|p| !p.is_empty())
        {
            let (from, to) = pair.split_once(':').expect("<from>:<to>");
            let from = char::from_u32(u32::from_str_radix(from, 16).expect("hex"))
                .expect("a BMP code point");
            let to = char::from_u32(u32::from_str_radix(to, 16).expect("hex"))
                .expect("a BMP code point");
            assert_eq!(super::java_title_case(from), to, "U+{:04X}", from as u32);
            checked += 1;
        }
        assert_eq!(checked, 1199, "the dump lost entries");

        // **The identity half asserted the implementation's own fallback arm
        // back at itself** -- `java_title_case(c) == c.to_uppercase()...` is
        // literally what the function does when the table misses, so it was
        // unfailable for exactly the inputs where the bug lived. It asserts
        // the FIXTURE now, on the rows where the title case IS the character
        // and the uppercase is not: the already-title-case digraphs and the
        // Georgian block, 50 code points the old dump could not contain.
        let mut identity = 0usize;
        for pair in DUMP
            .lines()
            .filter(|l| !l.starts_with('#'))
            .flat_map(|l| l.split(','))
            .filter(|p| !p.is_empty())
        {
            let (from, to) = pair.split_once(':').expect("<from>:<to>");
            let from = u32::from_str_radix(from, 16).expect("hex");
            let to = u32::from_str_radix(to, 16).expect("hex");
            if from == to {
                let c = char::from_u32(from).expect("a BMP code point");
                assert_eq!(super::java_title_case(c), c, "U+{from:04X}");
                identity += 1;
            }
        }
        assert!(
            identity >= 50,
            "only {identity} identity rows: the dump narrowed"
        );
        // And a code point NEITHER map touches comes back unchanged.
        for c in ['1', ' ', '\u{4e00}'] {
            assert_eq!(super::java_title_case(c), c);
        }
    }

    /// `sort` compares integers **exactly**, which the first cut's `f64`
    /// comparator did not.
    ///
    /// Pairs nothing — it is an exact-compared entry
    /// (`filter-sort-past-two-to-the-53`) — and it exists because the three
    /// `sort` entries the port shipped with all test the **type check** and
    /// none of them reaches the comparator. Round-tripping through `f64`
    /// collapses every integer past 2^53, and `sort_by` is stable, so a pair
    /// it cannot tell apart keeps its input order and the bug is invisible
    /// unless the input is already out of order.
    #[test]
    fn sort_compares_integers_exactly() {
        assert_eq!(
            ok(
                "{{ l | sort }}",
                r#"{"l": [9007199254740993, 9007199254740992]}"#
            ),
            "[9007199254740992,9007199254740993]"
        );
        assert_eq!(
            ok(
                "{{ l | sort }}",
                r#"{"l": [9223372036854775807, 9223372036854775806]}"#
            ),
            "[9223372036854775806,9223372036854775807]"
        );
        assert_eq!(
            ok(
                "{{ l | sort }}",
                r#"{"l": [-9223372036854775807, -9223372036854775808]}"#
            ),
            "[-9223372036854775808,-9223372036854775807]"
        );
        // Doubles still take `Double.compare`'s total order.
        assert_eq!(
            ok("{{ l | sort }}", r#"{"l": [2.5, 1.5, 10.25]}"#),
            "[1.5,2.5,10.25]"
        );
        // And the pre-port default was RIGHT about this, which is the part
        // worth remembering: minijinja compares integers exactly too.
        assert_eq!(ok("{{ l | sort }}", r#"{"l": [3, 1, 2]}"#), "[1,2,3]");
        // W9c final review C-M1: a native `u128` past `i128::MAX` (2^127,
        // as `kv()` hands one over) sorts by value against 2^64, an `i128`,
        // against -2^64 and against another `u128` (all one class,
        // `BigInteger`). Mutant: the text order puts 2^127 (`17…`) before
        // 2^64 (`18…`).
        assert_eq!(
            ok(
                "{{ l | sort }}",
                r#"{"l": [170141183460469231731687303715884105729, 170141183460469231731687303715884105728, 18446744073709551616, -18446744073709551616]}"#
            ),
            "[-18446744073709551616,18446744073709551616,170141183460469231731687303715884105728,170141183460469231731687303715884105729]"
        );
    }

    // -- W2 Task 8 fix round 1: Pebble's own core filters ----------------

    /// The nine `error-class` entries where Pebble **refuses an input type**
    /// that minijinja happily answered.
    ///
    /// Pairs `filter-abs-not-a-number`, `filter-capitalize-not-a-string`,
    /// `filter-first-map`, `filter-join-string-input`,
    /// `filter-reverse-string`, `filter-sort-mixed-classes`,
    /// `filter-sort-string-input`, `filter-split-input-not-a-string` and
    /// `filter-trim-not-a-string`.
    ///
    /// Every one of these was **over-acceptance** before this round — egret
    /// answered where Kestra throws — which is the direction this crate's
    /// rules do not allow, because the flow renders here and fails there.
    #[test]
    fn pebble_core_filters_refuse_a_wrong_input_type() {
        for (template, context, needle) in [
            (
                "{{ s | abs }}",
                r#"{"s": "-5"}"#,
                "The 'abs' filter does require",
            ),
            (
                "{{ n | capitalize }}",
                r#"{"n": 42}"#,
                "cannot be cast to class java.lang.String",
            ),
            (
                "{{ m | first }}",
                r#"{"m": {"a": 1}}"#,
                "cannot be cast to class java.util.Collection",
            ),
            (
                "{{ s | join('-') }}",
                r#"{"s": "abc"}"#,
                "either a collection or an array",
            ),
            (
                "{{ s | reverse }}",
                r#"{"s": "abc"}"#,
                "cannot be cast to class java.util.List",
            ),
            (
                "{{ l | sort }}",
                r#"{"l": [1, "a"]}"#,
                "cannot be cast to class java.lang.Integer",
            ),
            (
                "{{ s | sort }}",
                r#"{"s": "cba"}"#,
                "Unsupported input type for sort filter",
            ),
            (
                "{{ n | split(p) }}",
                r#"{"n": 42, "p": "."}"#,
                "cannot be cast to class java.lang.String",
            ),
            (
                "{{ n | trim }}",
                r#"{"n": 42}"#,
                "cannot be cast to class java.lang.String",
            ),
        ] {
            let message = render(template, context).expect_err("must be refused");
            assert!(message.contains(needle), "{template}\n  got: {message}");
        }

        // The accepted neighbour of each refusal, so the boundary is pinned in
        // both directions and not only where it says no.
        assert_eq!(ok("{{ n | abs }}", r#"{"n": -5}"#), "5");
        assert_eq!(ok("{{ s | capitalize }}", r#"{"s": "ab"}"#), "Ab");
        assert_eq!(ok("{{ l | first }}", r#"{"l": [3, 1]}"#), "3");
        assert_eq!(ok("{{ l | join('-') }}", r#"{"l": [1, 2]}"#), "1-2");
        assert_eq!(ok("{{ l | reverse }}", r#"{"l": [1, 2]}"#), "[2,1]");
        assert_eq!(ok("{{ l | sort }}", r#"{"l": [3, 1, 2]}"#), "[1,2,3]");
        assert_eq!(
            ok("{{ s | split(p) }}", r#"{"s": "a,b", "p": ","}"#),
            r#"["a","b"]"#
        );
        assert_eq!(ok("{{ s | trim }}", r#"{"s": " hi "}"#), "hi");
        // And `sort`'s real boundary, which is the pair COUNT: one element is
        // never compared, so a list Java could not sort still answers.
        assert_eq!(ok("{{ l | sort }}", r#"{"l": [null]}"#), "[null]");
        // Two Integers sort; an Integer and a Long do not, and which one a
        // JSON number becomes is its magnitude.
        assert_eq!(ok("{{ l | sort }}", r#"{"l": [2, 1]}"#), "[1,2]");
        assert!(render("{{ l | sort }}", r#"{"l": [1, 2147483648]}"#).is_err());
    }

    /// `split` and `slice` refuse what Java refuses, and the messages are
    /// three different sentences.
    ///
    /// Pairs `filter-split-dangling-quantifier`,
    /// `filter-split-missing-delimiter` and
    /// `filter-slice-to-index-past-the-end`.
    ///
    /// The first is the point of the whole `split` port: the delimiter is a
    /// **regex**, so `+` is a `PatternSyntaxException` where minijinja's split
    /// treated it as a literal and answered.
    #[test]
    fn split_and_slice_refuse_what_java_refuses() {
        let message = render("{{ s | split(p) }}", r#"{"s": "a+b", "p": "+"}"#)
            .expect_err("a dangling quantifier is not a delimiter");
        assert!(message.contains("Invalid regex"), "{message}");
        let message = render("{{ s | split }}", r#"{"s": "a.b"}"#).expect_err("must be refused");
        assert!(
            message.contains("missing delimiter parameter in split filter"),
            "{message}"
        );
        let message =
            render("{{ l | slice(0,9) }}", r#"{"l": [3, 1, 2]}"#).expect_err("must be refused");
        assert!(
            message.contains("toIndex must be smaller than input size: 3"),
            "{message}"
        );

        // The neighbours: a metacharacter that IS a valid pattern splits, and
        // a `toIndex` exactly at the length is accepted.
        assert_eq!(
            ok("{{ s | split(p) }}", r#"{"s": "a.b.c", "p": "."}"#),
            "[]"
        );
        assert_eq!(ok("{{ l | slice(0,3) }}", r#"{"l": [3, 1, 2]}"#), "[3,1,2]");
        // And the three `slice` errors are three different sentences.
        assert!(render("{{ l | slice(2,1) }}", r#"{"l": [1, 2, 3]}"#)
            .expect_err("must fail")
            .contains("toIndex must be greater than fromIndex"));
        assert!(render("{{ l | slice(4) }}", r#"{"l": [1, 2, 3]}"#)
            .expect_err("must fail")
            .contains("fromIndex(4) > toIndex(3)"));
        assert!(render("{{ l | slice(-1) }}", r#"{"l": [1, 2, 3]}"#)
            .expect_err("must fail")
            .contains("fromIndex must be greater than 0"));
    }

    /// The Python-spelling defect, found for the **fifth** time and this time
    /// through minijinja's own built-ins.
    ///
    /// `kestra_to_string` was made total in W2 Task 3 and every filter this
    /// crate wrote goes through it. These six never did, because egret never
    /// wrote them — they were minijinja's, and minijinja stringifies with
    /// `Display`, which is Python's spelling:
    ///
    /// | expression | Kestra | egret, before |
    /// |---|---|---|
    /// | `{{ nothing \| upper }}` | *(empty)* | `NONE` |
    /// | `{{ nothing \| lower }}` | *(empty)* | `none` |
    /// | `{{ nothing \| capitalize }}` | *(empty)* | `None` |
    /// | `{{ [1,null,2] \| join('-') }}` | `1-null-2` | `1-None-2` |
    ///
    /// The last one is the one a shape check would never find: it is not a
    /// null rendering as a word, it is a null *inside a container* rendering
    /// as the wrong word.
    #[test]
    fn the_built_in_filters_no_longer_write_pythons_spelling_of_null() {
        for template in [
            "<{{ n | upper }}>",
            "<{{ n | lower }}>",
            "<{{ n | capitalize }}>",
            "<{{ n | title }}>",
            "<{{ n | trim }}>",
            "<{{ n | escape }}>",
            "<{{ n | format('x') }}>",
        ] {
            assert_eq!(
                render(template, r#"{"n": null}"#).unwrap(),
                "<>",
                "{template}"
            );
        }
        assert_eq!(
            ok("{{ l | join('-') }}", r#"{"l": [1, null, 2]}"#),
            "1-null-2"
        );
        assert_eq!(
            ok("{{ s | format(n) }}", r#"{"s": "%s", "n": null}"#),
            "null"
        );
        // And a container, which is Java's `toString` and not minijinja's
        // `Display`: `{A=1}`, no quotes and an `=`.
        assert_eq!(ok("{{ m | upper }}", r#"{"m": {"a": 1}}"#), "{A=1}");
        assert_eq!(ok("{{ b | escape }}", r#"{"b": true}"#), "true");
    }

    /// The three that were silently answering a **different question**, each
    /// pinned by the value that separates the two filters.
    ///
    /// These are the `replace` shape: minijinja resolved the name and answered,
    /// so nothing but a differential could see it.
    #[test]
    fn split_slice_and_default_are_pebbles_and_not_minijinjas() {
        // `split`'s delimiter is a Java regex. minijinja's is a literal, so
        // every dotted, piped or bracketed delimiter was wrong.
        assert_eq!(
            ok("{{ s | split(p) }}", r#"{"s": "a.b.c", "p": "."}"#),
            "[]"
        );
        assert_eq!(
            ok("{{ s | split(p) }}", r#"{"s": "a|b", "p": "|"}"#),
            r#"["a","|","b"]"#
        );
        // The limit argument, which minijinja's ignored.
        assert_eq!(
            ok("{{ s | split(p, 2) }}", r#"{"s": "a,b,,c", "p": ","}"#),
            r#"["a","b,,c"]"#
        );
        // Java's own two edge rules: trailing empties dropped at limit 0,
        // inner ones kept, and no leading empty for a zero-width match at 0.
        assert_eq!(
            ok("{{ s | split(p) }}", r#"{"s": "a,b,,", "p": ","}"#),
            r#"["a","b"]"#
        );
        assert_eq!(
            ok("{{ s | split(p) }}", r#"{"s": "a,b,,c", "p": ","}"#),
            r#"["a","b","","c"]"#
        );
        assert_eq!(
            ok("{{ s | split(p) }}", r#"{"s": "abc", "p": ""}"#),
            r#"["a","b","c"]"#
        );

        // `slice` is `(fromIndex, toIndex)`, not `(slices, index)`.
        assert_eq!(ok("{{ l | slice(0,2) }}", r#"{"l": [3, 1, 2]}"#), "[3,1]");
        assert_eq!(ok("{{ l | slice(1) }}", r#"{"l": [3, 1, 2]}"#), "[1,2]");
        assert_eq!(ok("{{ s | slice(1,3) }}", r#"{"s": "abcdef"}"#), "bc");
        // `intValue()` truncates.
        assert_eq!(
            ok("{{ l | slice(f) }}", r#"{"l": [1,2,3], "f": 1.9}"#),
            "[2,3]"
        );

        // `default` fires on null, blank and empty -- not only on undefined.
        assert_eq!(
            render("<{{ n | default('D') }}>", r#"{"n": null}"#).unwrap(),
            "<D>"
        );
        assert_eq!(ok("<{{ s | default('D') }}>", r#"{"s": "  "}"#), "<D>");
        assert_eq!(ok("<{{ l | default('D') }}>", r#"{"l": []}"#), "<D>");
        assert_eq!(ok("<{{ m | default('D') }}>", r#"{"m": {}}"#), "<D>");
        // And the boundary in the other direction: `0` and `false` are NOT
        // empty and are kept.
        assert_eq!(ok("{{ z | default('D') }}", r#"{"z": 0}"#), "0");
        assert_eq!(ok("{{ f | default('D') }}", r#"{"f": false}"#), "false");
    }

    /// The four that were wrong about *how much* of the input they touch.
    #[test]
    fn capitalize_title_trim_and_length_follow_javas_rules() {
        // `capitalize` and `title` leave the tail of every word alone; Python
        // lowercases it, and minijinja is Python's.
        assert_eq!(
            ok("{{ s | capitalize }}", r#"{"s": "hello WORLD"}"#),
            "Hello WORLD"
        );
        assert_eq!(
            ok("{{ s | capitalize }}", r#"{"s": "  hi there"}"#),
            "  Hi there"
        );
        assert_eq!(
            ok("{{ s | title }}", r#"{"s": "hello WORLD"}"#),
            "Hello WORLD"
        );
        // Only whitespace delimits a word: punctuation does not.
        assert_eq!(ok("{{ s | title }}", r#"{"s": "a-b c'd"}"#), "A-b C'd");
        assert_eq!(ok("{{ s | title }}", r#"{"s": "1a b"}"#), "1a B");

        // `trim` takes NO argument -- minijinja's honoured one.
        assert_eq!(
            ok("<{{ s | trim('x') }}>", r#"{"s": "xxhixx"}"#),
            "<xxhixx>"
        );
        // and it is Java's `String.trim()`: everything <= U+0020, and nothing
        // else. U+00A0 survives here and Rust's `str::trim` would eat it.
        assert_eq!(
            ok("<{{ s | trim }}>", "{\"s\": \"\\u00a0hi\"}"),
            "<\u{a0}hi>"
        );

        // `length` counts UTF-16 code units, and its last branch is `0`.
        assert_eq!(ok("{{ s | length }}", "{\"s\": \"a\\ud83d\\ude00b\"}"), "4");
        assert_eq!(ok("{{ n | length }}", r#"{"n": 42}"#), "0");
        assert_eq!(ok("{{ b | length }}", r#"{"b": true}"#), "0");
        assert_eq!(render("{{ n | length }}", r#"{"n": null}"#).unwrap(), "0");
        assert_eq!(ok("{{ s | length }}", r#"{"s": "abc"}"#), "3");
    }

    /// `abs` reproduces `Math.abs`'s overflow, which is upstream defect 7.
    ///
    /// There is no positive `Long.MIN_VALUE`, and Java returns the input
    /// rather than throwing. Fixing it would give a flow author a different
    /// number on the two engines, which is the single thing this package
    /// exists to prevent.
    #[test]
    fn abs_reproduces_javas_overflow_at_the_minimum() {
        assert_eq!(
            ok("{{ n | abs }}", r#"{"n": -9223372036854775808}"#),
            "-9223372036854775808"
        );
        assert_eq!(ok("{{ n | abs }}", r#"{"n": -2147483648}"#), "-2147483648");
        // One away from each boundary, so the test says where the wrap starts
        // rather than only that it exists.
        assert_eq!(ok("{{ n | abs }}", r#"{"n": -2147483647}"#), "2147483647");
        assert_eq!(ok("{{ n | abs }}", r#"{"n": -5}"#), "5");
        assert_eq!(ok("{{ f | abs }}", r#"{"f": -1.5}"#), "1.5");
    }

    // -- W2 Task 8: `replace` ------------------------------------------

    /// `replace` is **Pebble's** map-taking filter, not minijinja's
    /// `(old, new)` one, and the two are incompatible in both directions.
    ///
    /// Pairs `filter-replace-two-positional-arguments`,
    /// `filter-replace-missing-argument`, `filter-replace-pairs-not-a-map` and
    /// `filter-replace-null-replacement`.
    ///
    /// # What fails when the override is deleted
    ///
    /// Not `unknown filter` — that is the whole point, and it is why this is
    /// the fourth name in this crate to need a test of this shape. With the
    /// `add_filter("replace", replace)` line removed, minijinja's own filter
    /// answers: `{{ 'ab' | replace('a','b') }}` renders `bb` where Kestra
    /// throws, and `{{ 'ab' | replace({'a':'b'}) }}` complains about a missing
    /// second argument rather than doing the substitution. Both assertions
    /// below fail on that build, which is what makes them the test and not the
    /// corpus entry.
    #[test]
    fn replace_is_pebbles_map_taking_filter_not_minijinjas_pair() {
        // The accepted shape: one map, positional or named.
        assert_eq!(
            ok("{{ s | replace({'a': 'b'}) }}", r#"{"s": "banana"}"#),
            "bbnbnb"
        );
        assert_eq!(
            ok(
                "{{ s | replace(replace_pairs={'a': 'b'}) }}",
                r#"{"s": "banana"}"#
            ),
            "bbnbnb"
        );

        // minijinja's shape, which Kestra refuses. egret refuses it too --
        // for a different reason (a String is not a Map, where Pebble's own
        // arity check fires first), but it refuses.
        let message = err("{{ s | replace('a', 'b') }}", r#"{"s": "banana"}"#);
        assert!(
            message.contains("cannot be cast to class java.util.Map"),
            "minijinja's own replace would have answered `bbnbnb` here: {message}"
        );
        assert!(
            err("{{ s | replace }}", r#"{"s": "banana"}"#)
                .contains("The argument 'replace_pairs' is required."),
            "{}",
            err("{{ s | replace }}", r#"{"s": "banana"}"#)
        );
        assert!(
            err("{{ s | replace('a') }}", r#"{"s": "banana"}"#)
                .contains("cannot be cast to class java.util.Map"),
            "{}",
            err("{{ s | replace('a') }}", r#"{"s": "banana"}"#)
        );
        // An explicit null `replace_pairs` says the same thing as an absent
        // one: Pebble's guard is `get(...) == null`, not `containsKey`.
        assert!(
            err("{{ s | replace(n) }}", r#"{"s": "ab", "n": null}"#)
                .contains("The argument 'replace_pairs' is required."),
            "{}",
            err("{{ s | replace(n) }}", r#"{"s": "ab", "n": null}"#)
        );
        assert!(
            err("{{ s | replace({'a': n}) }}", r#"{"s": "abc", "n": null}"#)
                .contains("because the return value of \"java.util.Map$Entry.getValue()\" is null"),
            "{}",
            err("{{ s | replace({'a': n}) }}", r#"{"s": "abc", "n": null}"#)
        );

        // The input goes through Java's `toString`, not minijinja's Display:
        // `None`/`True` would be the giveaway, and a list is `[1, 2]`.
        assert_eq!(
            ok("{{ l | replace({'1': 'x'}) }}", r#"{"l": [1, 2]}"#),
            "[x, 2]"
        );
        assert_eq!(
            ok("{{ m | replace({'1': 'x'}) }}", r#"{"m": {"a": 1}}"#),
            "{a=x}"
        );
        assert_eq!(
            ok("{{ b | replace({'true': 'y'}) }}", r#"{"b": true}"#),
            "y"
        );
        // A null INPUT is null, which is every Pebble filter's first line.
        assert_eq!(
            ok("<{{ n | replace({'a': 'b'}) }}>", r#"{"n": null}"#),
            "<>"
        );

        // Pairs chain: each sees the previous one's output, so this is `cc`
        // and not `bc`. **That example agrees with Kestra by luck** -- for
        // the key set `{a, b}`, Java's HashMap bucket order and minijinja's
        // key-sorted literal order coincide. The next row is where they part,
        // and it is a recorded divergence rather than a fidelity claim.
        assert_eq!(
            ok("{{ s | replace({'a': 'b', 'b': 'c'}) }}", r#"{"s": "ab"}"#),
            "cc"
        );
        assert_eq!(
            ok("{{ s | replace({'p': 'a', 'a': 'x'}) }}", r#"{"s": "ap"}"#),
            "xa"
        );
        // And the half that DOES agree, which without a test would be a
        // claim: a map out of the CONTEXT is a `LinkedHashMap` there and an
        // `OrderedMap` here, both insertion order, so the same two pairs
        // answer `xx` on both engines.
        assert_eq!(
            ok(
                "{{ s | replace(m) }}",
                r#"{"s": "ap", "m": {"p": "a", "a": "x"}}"#
            ),
            "xx"
        );
        // Java's `String.replace("", x)`, which Rust's `str::replace` agrees
        // with -- measured rather than assumed.
        assert_eq!(
            ok("{{ s | replace({'': '-'}) }}", r#"{"s": "ab"}"#),
            "-a-b-"
        );
    }

    /// The bound `replace` has and Kestra does not — and the two shapes that
    /// need it.
    ///
    /// No corpus entry: Kestra answers both of these, slowly and then not at
    /// all, so a golden would be a hang or an OOM. That is the C7 case
    /// (`oracle/README.md`, "A resource-exhaustion vector is not a behaviour
    /// to port"), and it is a unit test instead.
    ///
    /// Verified by REMOVING the bound, not by asserting it: with the
    /// `grown > MAX_OUTPUT_BYTES` check deleted, the first case allocates
    /// ~4 GB inside one `String::replace` and the second runs for minutes.
    ///
    /// See [`replace_is_bounded_across_a_whole_render`] for the third axis, a
    /// loop, which a per-call budget cannot see at all.
    #[test]
    fn replace_is_bounded_where_pebbles_own_loop_is_not() {
        // One pair, multiplying: 65 536 needles x a 65 536-byte replacement.
        let wide = "x".repeat(1 << 16);
        let cx = format!(
            r#"{{"s": {}, "r": {}}}"#,
            serde_json::to_string(&wide).unwrap(),
            serde_json::to_string(&wide).unwrap()
        );
        let message = render("{{ s | replace({'x': r}) }}", &cx).expect_err("must be refused");
        assert!(message.contains("over egret's"), "{message}");

        // Pairs CHAINING, which no single-step check would see: each entry
        // doubles the string, so twenty-five entries are 2^25 times the input
        // from a template a hundred bytes wide.
        let alphabet = "abcdefghijklmnopqrstuvwxyz";
        let pairs: Vec<String> = alphabet
            .chars()
            .zip(alphabet.chars().skip(1))
            .map(|(from, to)| format!("'{from}': '{to}{to}'"))
            .collect();
        let template = format!("{{{{ s | replace({{{}}}) }}}}", pairs.join(", "));
        let message =
            render(&template, r#"{"s": "aaaaaaaaaaaaaaaa"}"#).expect_err("must be refused");
        assert!(message.contains("over egret's"), "{message}");

        // The SECOND axis, which the growth check above does not cover and
        // which attacking that check is what found: a pair that SHRINKS the
        // subject keeps `|out|` small forever while the pair count stays the
        // author's, so the cost is `pairs x |out|` and only `|out|` was
        // bounded. Thirty thousand pairs over a 64 KiB subject.
        let pairs: Vec<String> = (0..30_000).map(|i| format!("\"k{i}\": \"\"")).collect();
        let message = render(
            "{{ s | replace(m) }}",
            &format!(
                r#"{{"s": {}, "m": {{{}}}}}"#,
                serde_json::to_string(&"z".repeat(64 * 1024)).unwrap(),
                pairs.join(",")
            ),
        )
        .expect_err("must be refused");
        assert!(message.contains("would scan more than"), "{message}");

        // And the accepted neighbour: a substitution that stays under the cap
        // still answers, so the bound is a ceiling and not a refusal.
        assert_eq!(
            ok("{{ s | replace({'x': 'yy'}) }}", r#"{"s": "xx"}"#),
            "yyyy"
        );
        // A thousand pairs over a 64 KiB subject is 64 MiB of scanning, well
        // inside the budget, and still answers.
        let pairs: Vec<String> = (0..1_000).map(|i| format!("\"k{i}\": \"\"")).collect();
        assert_eq!(
            render(
                "{{ s | replace(m) }}",
                &format!(
                    r#"{{"s": {}, "m": {{{}}}}}"#,
                    serde_json::to_string(&"z".repeat(64 * 1024)).unwrap(),
                    pairs.join(",")
                )
            )
            .expect("must render")
            .len(),
            64 * 1024
        );
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
            // Two runs of `&` where the first is odd and at least 3.
            ("[a&&&b&&&c]", "a"),
            ("[a&&&b&&&c&&&d]", "a"),
            ("[&&&a&&&b]", "a"),
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
            // **Either bound is a pass, and that is not a weakening.**
            // `MAX_STEPS` and `MAX_RUN` race by construction: 65 536 steps
            // take well under a second idle, and more than one when the
            // machine is saturated. Measured -- these two tests passed thirty
            // runs of the lib binary alone and failed when `--test oracle`
            // ran beside them, which this round made heavier. What the
            // assertion is FOR is unchanged: it fails on `unknown function`,
            // on a generic failure, and on a bound that stopped firing, which
            // is the hole (`is_err()` hiding an unregistered `repeat`) it was
            // written to close.
            assert!(
                message.contains(needle) || message.contains("ran for longer"),
                "{program}: {message:?} does not name a bound egret owns"
            );
        }
    }

    /// **A value at the depth limit renders on a 2 MiB stack — a tokio
    /// worker's — through every writer that recurses over it.**
    ///
    /// This is the other half of `MAX_VALUE_DEPTH`'s derivation, and it is
    /// the half that matters in production: the depth check runs on the jq
    /// thread, but `jaq_value`, the DROP of the value, and every serialiser
    /// run on the CALLER's thread, whose stack egret does not choose.
    ///
    /// The claim is that these calls RETURN. A stack overflow is `SIGABRT`,
    /// so a failure here takes the test binary with it and reports a signal.
    ///
    /// Measured cost per level, by bisecting the depth that overflows a
    /// thread of a given size and confirming it is linear in that size:
    /// `kestra_to_string` 1.346 KiB, `write_ion` 1.049, `write_json` 1.018,
    /// `to_yaml` 0.391. At the worst of those, 128 levels is 172 KiB — a
    /// twelvefold margin on 2 MiB.
    #[test]
    fn a_value_at_the_depth_limit_renders_on_a_tokio_workers_stack() {
        let deep = format!(
            "reduce range({}) as $i (.;[.])",
            crate::runners::jqsandbox::MAX_VALUE_DEPTH - 1
        );
        let worker = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                for suffix in ["", " | toJson", " | toIon", " | yaml", " | string"] {
                    let template = format!("{{{{ v | jq(e){suffix} }}}}");
                    let context = format!("{{\"v\": 1, \"e\": {}}}", serde_json_string(&deep));
                    let rendered = ok(&template, &context);
                    assert!(
                        rendered.len() > crate::runners::jqsandbox::MAX_VALUE_DEPTH,
                        "{suffix}: {} is too short to be the deep value",
                        rendered.len()
                    );
                }
                // And one level deeper is refused rather than rendered.
                let deeper = format!(
                    "reduce range({}) as $i (.;[.])",
                    crate::runners::jqsandbox::MAX_VALUE_DEPTH
                );
                let message = err(
                    "{{ v | jq(e) }}",
                    &format!("{{\"v\": 1, \"e\": {}}}", serde_json_string(&deeper)),
                );
                assert!(message.contains("nests more than"), "{message:?}");

                // **And the depths that discriminate WHERE the guard sits.**
                // The boundary pair above passes whether the check runs
                // before the conversions or after them, because on the jq
                // thread's 256 MiB every conversion survives 128 levels. What
                // does not survive is THIS thread: at 1 400 levels -- the
                // exploit a review actually landed -- `jaq_value` and the
                // value's own `Drop` run here, on 2 MiB, and a guard placed
                // after them takes the process rather than failing. So a
                // message, not an abort, is what this asserts.
                //
                // 60 000 is the deepest `MAX_STEPS` can build at all, and it
                // must reach the same sentence: the guard is the FIRST thing
                // that touches the value, not the cheapest.
                for depth in [1_400, 60_000] {
                    let program = format!("reduce range({depth}) as $i (.;[.])");
                    let message = err(
                        "{{ v | jq(e) }}",
                        &format!("{{\"v\": 1, \"e\": {}}}", serde_json_string(&program)),
                    );
                    assert!(message.contains("nests more than"), "{depth}: {message:?}");
                }
            })
            .expect("spawn");
        worker.join().expect("the 2 MiB worker must not overflow");
    }

    /// A jq source big enough to overflow jaq's parser is refused **before**
    /// the parser runs, for every recursive construct — not for the two the
    /// review happened to name.
    ///
    /// **Six of these contain no bracket at all.** An earlier cut bounded
    /// bracket nesting and tested `[` and `(`; `-`x241 aborted in 241 bytes.
    ///
    /// This one cannot be written as "assert it errors": a stack overflow is
    /// `SIGABRT`, so a run that reaches the parser takes the whole test
    /// binary with it and reports a signal rather than a failure. The
    /// assertion that these calls RETURN is the claim.
    #[test]
    fn every_deep_jq_source_is_refused_before_the_parser_sees_it() {
        // Derived from the cap, so the test stays right if the cap moves.
        let n = crate::runners::jqsandbox::MAX_SOURCE_BYTES + 1;
        for program in [
            "-".repeat(n) + "1",
            "[".repeat(n) + "1" + &"]".repeat(n),
            "(".repeat(n) + "1" + &")".repeat(n),
            "{a:".repeat(n) + "1" + &"}".repeat(n),
            "try ".repeat(n) + ".",
            ".".to_string() + &"|.".repeat(n),
            ".".to_string() + &",.".repeat(n),
            ".".to_string() + &" // .".repeat(n),
            "if true then ".repeat(n) + "1" + &" else 1 end".repeat(n),
            // The shape that walked past the string-aware scanner: after
            // `"\((1),` it believed it was inside a string and stopped
            // counting brackets.
            "\"\\((1),".to_string() + &"[".repeat(n) + "1" + &"]".repeat(n) + ")\"",
        ] {
            let message = err(
                "{{ m | jq(e) }}",
                &format!(
                    "{{\"m\": {{\"a\": 1}}, \"e\": {}}}",
                    serde_json_string(&program)
                ),
            );
            assert!(message.contains("egret's limit is"), "{message:?}");
        }

        // And the 352-byte version of that last shape -- the one that walked
        // past the scanner -- now ANSWERS, because 352 bytes is inside both
        // the cap and the stack. The bound is the source's size, not its
        // shape, so a shape the scanner could not see is no longer special.
        let inside = "\"\\((1),".to_string() + &"[".repeat(171) + "1" + &"]".repeat(171) + ")\"";
        assert!(inside.len() < 400);
        assert!(render(
            "{{ m | jq(e) }}",
            &format!(
                "{{\"m\": {{\"a\": 1}}, \"e\": {}}}",
                serde_json_string(&inside)
            ),
        )
        .is_ok());
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

    /// Egret's OWN message for every cell of the [`JavaSeqKind`] gate, which
    /// is what backs the twenty-seven `error-class` ids in
    /// `oracle/error-class-pairs.txt` that name this test.
    ///
    /// error-class comparison is a free pass — it matches on any failure at
    /// all — so what has to be asserted is that the refusal is *this* refusal.
    /// The table walks every (kind, consumer) cell rather than sampling, and
    /// the length column is here because it is the axis a first cut got wrong:
    /// an EMPTY `chunk` sorts on Kestra and an empty `flatten` does not.
    #[test]
    fn the_collection_kind_gate_refuses_what_java_refuses() {
        // (producer, context) -> a value of each kind.
        let map = r#"{"m": {"b": 1, "a": 2}}"#;
        let list = r#"{"l": ["b", "a", "c"]}"#;
        let text = r#"{"s": "a,b,c", "d": ","}"#;

        const IMMUTABLE_SORT: &str =
            "'sort' filter cannot sort an immutable list in place: 'flatten', 'chunk' and \
             'keys' return one";
        const IMMUTABLE_REVERSE: &str =
            "'reverse' filter cannot reverse an immutable list in place: 'flatten', 'chunk' \
             and 'keys' return one";

        // --- Collection: a map's key or value view is not a `List` at all.
        for producer in ["m | keys", "m | values"] {
            assert!(err(&format!("{{{{ ({producer}) | sort }}}}"), map)
                .contains("Unsupported input type for sort filter"));
            assert!(
                err(&format!("{{{{ ({producer}) | reverse }}}}"), map).contains(
                    "'reverse' filter can only be applied to a List; this is a map's key or value \
                 view, which is not a List"
                )
            );
            assert!(
                err(&format!("{{{{ ({producer}) | slice(0,2) }}}}"), map).contains(
                    "Slice filter can only be applied to String, List and array inputs. Actual \
                 type was: a map's key or value view"
                )
            );
            assert!(err(&format!("{{{{ ({producer}) | distinct }}}}"), map)
                .contains("Input must be a list, but received : a map's key or value view"));
            assert!(
                err(&format!("{{{{ ({producer}) | chunk(2) }}}}"), map).contains(
                    "'chunk' filter can only be applied to List. Actual type was: a map's key or \
                 value view"
                )
            );
            assert!(err(&format!("{{{{ ({producer}) | flatten }}}}"), map)
                .contains("The 'flatten' filter can only be applied to lists."));
            assert!(
                err(&format!("{{{{ ({producer}) | keys }}}}"), map).contains(
                    "'keys' filter can only be applied to List, Map, Array. Actual type was: a \
                 map's key or value view"
                )
            );
        }

        // --- Array: `SplitFilter` returns `String[]`. `sort`, `slice` and
        // `keys` have array branches in the engine and the other four do not.
        assert_eq!(ok("{{ (s | split(d)) | sort }}", text), r#"["a","b","c"]"#);
        assert_eq!(
            ok("{{ (s | split(d)) | slice(0,2) }}", text),
            r#"["a","b"]"#
        );
        assert_eq!(ok("{{ (s | split(d)) | keys }}", text), "[0,1,2]");
        assert!(err("{{ (s | split(d)) | reverse }}", text)
            .contains("'reverse' filter can only be applied to a List; this is an array"));
        assert!(err("{{ (s | split(d)) | distinct }}", text)
            .contains("Input must be a list, but received : an array"));
        assert!(err("{{ (s | split(d)) | chunk(2) }}", text)
            .contains("'chunk' filter can only be applied to List. Actual type was: an array"));
        assert!(err("{{ (s | split(d)) | flatten }}", text)
            .contains("The 'flatten' filter can only be applied to lists."));
        // `Arrays.asList` is what `sort` casts an array through, and it IS
        // mutable, so the result leaves the class entirely.
        assert_eq!(
            ok("{{ ((s | split(d)) | sort) | reverse }}", text),
            r#"["c","b","a"]"#
        );
        // …while `slice` copies the range and stays an array.
        assert!(err("{{ ((s | split(d)) | slice(0,2)) | distinct }}", text)
            .contains("Input must be a list, but received : an array"));

        // --- ImmutableList (`Stream.toList()`): `sort` is OVERRIDDEN to throw,
        // so it throws at every length, including zero.
        for producer in ["l | flatten", "l | keys"] {
            assert!(err(&format!("{{{{ ({producer}) | sort }}}}"), list).contains(IMMUTABLE_SORT));
            assert!(
                err(&format!("{{{{ ({producer}) | reverse }}}}"), list).contains(IMMUTABLE_REVERSE)
            );
            // Everything that COPIES is fine — the gate is `set`, not `List`
            // — and each is pinned to its VALUE, not to being non-empty.
            let elements = if producer == "l | keys" {
                ["[0,1,2]", "[[0,1],[2]]", "[0,1]"]
            } else {
                [r#"["b","a","c"]"#, r#"[["b","a"],["c"]]"#, r#"["b","a"]"#]
            };
            assert_eq!(
                ok(&format!("{{{{ ({producer}) | distinct }}}}"), list),
                elements[0]
            );
            assert_eq!(
                ok(&format!("{{{{ ({producer}) | chunk(2) }}}}"), list),
                elements[1]
            );
            assert_eq!(
                ok(&format!("{{{{ ({producer}) | slice(0,2) }}}}"), list),
                elements[2]
            );
        }
        assert!(err(r#"{{ (el | flatten) | sort }}"#, r#"{"el": []}"#).contains(IMMUTABLE_SORT));
        assert_eq!(
            ok(r#"{{ (el | flatten) | reverse }}"#, r#"{"el": []}"#),
            "[]"
        );
        assert_eq!(
            ok(r#"{{ (l1 | keys) | reverse }}"#, r#"{"l1": ["b"]}"#),
            "[0]"
        );
        // …and `slice` INHERITS it, which is why the kind is not thrown away
        // at the first copy.
        assert!(
            err("{{ ((l | flatten) | slice(0,2)) | reverse }}", list).contains(IMMUTABLE_REVERSE)
        );

        // --- Partition (Guava): `sort` is NOT overridden, so `List.sort`'s
        // default runs and calls `set` once per element. Zero elements is zero
        // calls, and this row is the reason `Partition` is not folded into
        // `ImmutableList`.
        assert_eq!(ok(r#"{{ (el | chunk(2)) | sort }}"#, r#"{"el": []}"#), "[]");
        assert!(
            err(r#"{{ (l2 | chunk(2)) | sort }}"#, r#"{"l2": ["b", "a"]}"#)
                .contains(IMMUTABLE_SORT)
        );
        assert_eq!(
            ok(r#"{{ (el | chunk(2)) | reverse }}"#, r#"{"el": []}"#),
            "[]"
        );
        assert!(err("{{ (l | chunk(2)) | reverse }}", list).contains(IMMUTABLE_REVERSE));

        // --- MutableList: the default, and untagged. Nothing refuses it, and
        // the VALUE is pinned rather than its shape -- the first cut asserted
        // only a leading `[` and a non-empty string, so a gate that returned
        // the wrong elements, or a truncated list, passed it.
        for (consumer, expected) in [
            ("sort", r#"["a","b","c"]"#),
            ("reverse", r#"["c","a","b"]"#),
            ("distinct", r#"["b","a","c"]"#),
            ("chunk(2)", r#"[["b","a"],["c"]]"#),
            ("flatten", r#"["b","a","c"]"#),
            ("keys", "[0,1,2]"),
            ("slice(0,2)", r#"["b","a"]"#),
        ] {
            assert_eq!(
                ok(&format!("{{{{ (l | distinct) | {consumer} }}}}"), list),
                expected,
                "a plain ArrayList must reach {consumer} AND come back intact"
            );
        }

        // --- The ELEMENT side of `flatten` makes the same cast as its input
        // side, so an inner key view or array is kept whole.
        assert_eq!(
            ok("{{ [(m | keys), 'z'] | flatten }}", map),
            r#"[["b","a"],"z"]"#
        );
        assert_eq!(
            ok("{{ [(s | split(d)), 'z'] | flatten }}", text),
            r#"[["a","b","c"],"z"]"#
        );

        // --- W2a fix round 1: `reverse` and `sort` RETURN THEIR INPUT, so the
        // kind survives them. Reachable only at length 0 or 1, because at 2
        // the reverse itself throws — the boundary this whole type exists to
        // model, and the one the depth-2 cross could not reach.
        let one = r#"{"l1": ["b"], "el": []}"#;
        assert!(err("{{ ((el | flatten) | reverse) | sort }}", one).contains(IMMUTABLE_SORT));
        assert!(err("{{ ((l1 | keys) | reverse) | sort }}", one).contains(IMMUTABLE_SORT));
        assert!(err("{{ ((l1 | chunk(2)) | reverse) | sort }}", one).contains(IMMUTABLE_SORT));
        // …and the other side: a reversal that is allowed keeps answering.
        assert_eq!(
            ok("{{ ((l1 | flatten) | reverse) | reverse }}", one),
            r#"["b"]"#
        );
        assert_eq!(ok("{{ ((el | chunk(2)) | sort) | reverse }}", one), "[]");
        // An array through `sort` becomes `Arrays$ArrayList`, a mutable List,
        // so the kind is DROPPED there rather than propagated.
        assert_eq!(
            ok("{{ ((s | split(d)) | sort) | reverse }}", text),
            r#"["c","b","a"]"#
        );

        // --- W2a fix round 1: `escapeChar` is the ninth consumer.
        // `instanceof Map` -> `instanceof List` -> `toString()`, so a key or
        // value view falls to the LAST branch and is stringified whole. Both
        // engines' orders are identical here, which is why filing this under
        // the `HashMap` order hid it.
        assert_eq!(
            ok("{{ (m | keys) | escapeChar(type='shell') }}", map),
            "[b, a]"
        );
        assert_eq!(
            ok("{{ (m | values) | escapeChar(type='shell') }}", map),
            "[1, 2]"
        );
        // A real List is still WALKED, which is the other side of that gate.
        assert_eq!(
            ok("{{ (l | flatten) | escapeChar(type='shell') }}", list),
            r#"["b","a","c"]"#
        );

        // --- W2a fix round 1: the SUBSCRIPT operator, which is not a filter
        // and which the filter x filter cross therefore could not reach.
        // Pebble's `GetAttributeExpression` has a `List` branch and an array
        // branch and no `Collection` branch.
        assert!(!err("{{ (m | keys)[0] }}", map).is_empty());
        assert!(!err("{{ (m | values)[0] }}", map).is_empty());
        assert_eq!(ok("{{ (s | split(d))[0] }}", text), "a");
        assert_eq!(ok("{{ (l | flatten)[0] }}", list), "b");
        // …and a key view still ITERATES, still has a length and still
        // answers `first`. That is the whole reason `Collection` needs its own
        // enumerator rather than simply refusing `get_value`.
        assert_eq!(ok("{{ (m | keys) | length }}", map), "2");
        assert_eq!(ok("{{ (m | keys) | first }}", map), "b");
        assert_eq!(
            ok("{% for x in (m | keys) %}{{ x }};{% endfor %}", map),
            "b;a;"
        );
        assert_eq!(ok("{{ (m | keys) | default('D') }}", map), r#"["b","a"]"#);
        assert_eq!(ok("{{ (em | keys) | default('D') }}", r#"{"em": {}}"#), "D");

        // --- W2a fix round 2: an ARRAY is never empty to `default`.
        // `EmptyTest` is `null || (String && trim().isEmpty()) ||
        // (Collection && isEmpty()) || (Map && isEmpty())` -- four branches,
        // no array branch -- and a Java array is not a `Collection`. Reachable
        // only at length 0 of the `Array` kind, a cell no instrument held
        // until the length bucket stopped being read off the input's name.
        let comma = r#"{"sc": ",", "d": ","}"#;
        assert_eq!(ok("{{ (sc | split(d)) | default('D') }}", comma), "[]");
        assert_eq!(
            ok("{{ ((sc | split(d)) | default('D')) | length }}", comma),
            "0"
        );
        assert!(
            err("{{ ((sc | split(d)) | default('D')) | capitalize }}", comma)
                .contains("cannot be cast to class java.lang.String")
        );
        // …and a non-empty array is unchanged either way, which is why the
        // divergence hid: the two engines agree at every length but zero.
        assert_eq!(
            ok("{{ (s | split(d)) | default('D') }}", text),
            r#"["a","b","c"]"#
        );
    }

    // ---- W2a: the three answer fixtures ----------------------------------

    /// A probe corpus row: `oracle/fixtures/*.probe.json` is in
    /// `corpus.json`'s own schema, so the same `host.Oracle generate` writes
    /// both and a Kestra bump moves the answers visibly.
    #[derive(serde::Deserialize)]
    struct FixtureProbe {
        expr: String,
        context: serde_json::Value,
    }

    /// The harness's reply to one probe. `result` is a string when the render
    /// succeeded and an object carrying `error` when it threw.
    #[derive(serde::Deserialize)]
    struct FixtureAnswer {
        expr: String,
        result: serde_json::Value,
    }

    /// JSON's escapes, so a tab-separated file can carry a value containing a
    /// tab or a newline. The same helper `javare`'s fixture uses.
    fn fixture_unescape(field: &str) -> String {
        serde_json::from_str::<String>(&format!("\"{field}\"")).unwrap_or_else(|e| {
            panic!("{field:?} is not a JSON string body: {e}");
        })
    }

    /// The probe corpus, **keyed by the expression itself**.
    ///
    /// The first cut keyed this by an id built from the row's own name columns
    /// and deserialised only `id` and `result`, so both compared columns moved
    /// together: a probe rewritten to `{{ 1 }}`, with the row still reading
    /// `keys / m / sort` and the answers regenerated, was fully self-consistent
    /// and green. `javare.rs`'s fixture -- the precedent this followed -- keys
    /// its harness map on the probe's own `context["p"]`/`context["s"]` and
    /// does not have that hole. Keying on the expression closes it the same
    /// way, and the `expr` column added to all three fixtures is what makes
    /// the row able to name its own probe.
    fn fixture_probes(json: &str) -> std::collections::BTreeMap<String, serde_json::Value> {
        let probes: Vec<FixtureProbe> =
            serde_json::from_str(json).expect("the probe corpus must parse");
        let mut by_expr = std::collections::BTreeMap::new();
        for probe in probes {
            assert!(
                by_expr.insert(probe.expr.clone(), probe.context).is_none(),
                "two probes share an expression, so the key is not a key: {}",
                probe.expr
            );
        }
        by_expr
    }

    /// The harness answers, keyed by expression for the same reason.
    fn fixture_answers(json: &str) -> std::collections::BTreeMap<String, String> {
        let answers: Vec<FixtureAnswer> =
            serde_json::from_str(json).expect("the harness answers must parse");
        answers
            .into_iter()
            .map(|answer| {
                let value = match answer.result {
                    serde_json::Value::String(text) => text,
                    _ => "error".to_string(),
                };
                (answer.expr, value)
            })
            .collect()
    }

    /// Renders one expression through egret and reduces it to the fixture's
    /// alphabet: the rendered text, or the single word `error`.
    fn fixture_render(expr: &str, context: &serde_json::Value) -> String {
        let cx = RenderContext::from_json_str(&context.to_string())
            .expect("the probe context must parse");
        match render_str(expr, &cx) {
            Ok(text) => text,
            Err(_) => "error".to_string(),
        }
    }

    /// One fixture's shape: how many tab-separated columns, and where the
    /// `expr`, `kestra`, `egret` and `reason` columns sit.
    struct FixtureShape {
        columns: usize,
        rows: usize,
        unstable: usize,
    }

    /// Walks one fixture: every row's egret column is asserted by RENDERING
    /// the row's own `expr`, every divergent row must carry a reason, every
    /// agreeing row must carry `-`, and the row count and `unstable` count are
    /// pinned so the file cannot quietly lose rows or widen its one exemption.
    fn walk_fixture(fixture: &str, probes_json: &str, shape: FixtureShape) {
        let probes = fixture_probes(probes_json);
        let (mut rows, mut unstable) = (0, 0);
        for line in fixture.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(
                cols.len(),
                shape.columns,
                "{} tab-separated columns: {line}",
                shape.columns
            );
            let names = cols[..shape.columns - 4].join(" | ");
            let expr = fixture_unescape(cols[shape.columns - 4]);
            let kestra = fixture_unescape(cols[shape.columns - 3]);
            let declared = fixture_unescape(cols[shape.columns - 2]);
            let reason = cols[shape.columns - 1];
            rows += 1;
            if kestra == "unstable" {
                unstable += 1;
            }

            if kestra == declared {
                assert_eq!(
                    reason, "-",
                    "{names}: the engine and egret agree, so there is nothing to explain"
                );
            } else {
                assert!(
                    reason.len() > 20,
                    "{names}: the engine says {kestra} and egret says {declared}; a divergence \
                     must say why, in the file"
                );
            }

            let context = probes.get(&expr).unwrap_or_else(|| {
                panic!(
                    "{names}: no probe renders `{expr}` -- the row and the probe corpus have \
                        drifted apart"
                )
            });
            let actual = fixture_render(&expr, context);
            assert_eq!(
                actual, declared,
                "{names}: the file says egret answers {declared} for `{expr}` and it answers \
                 {actual}"
            );
        }
        assert_eq!(rows, shape.rows, "the fixture lost rows");
        assert_eq!(unstable, shape.unstable, "the unstable set changed size");
    }

    /// The `kestra` column against the **committed harness answers**, keyed by
    /// the row's own expression, so a hand-edited fixture fails.
    ///
    /// `unstable` rows are skipped **here and nowhere else**: their value is a
    /// Java identity hash, which is exactly why they carry no value. The row
    /// count of that exemption is pinned by [`walk_fixture`].
    fn check_kestra_column(fixture: &str, answers_json: &str, columns: usize) {
        let answers = fixture_answers(answers_json);
        for line in fixture.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            let names = cols[..columns - 4].join(" | ");
            let expr = fixture_unescape(cols[columns - 4]);
            let kestra = fixture_unescape(cols[columns - 3]);
            if kestra == "unstable" {
                continue;
            }
            let measured = answers
                .get(&expr)
                .unwrap_or_else(|| panic!("{names}: the harness has no answer for `{expr}`"));
            assert_eq!(
                measured, &kestra,
                "{names}: the fixture says Kestra answers {kestra} for `{expr}` and the \
                 committed harness answer is {measured}"
            );
        }
    }

    /// All 63 engine filters over a fifteen-input matrix, asked through
    /// Kestra's own `className` filter — the enumeration W2 Task 8's fix round
    /// said it did not have.
    #[test]
    fn the_return_classes_match_the_engine() {
        walk_fixture(
            include_str!("../../oracle/return-classes.txt"),
            include_str!("../../oracle/fixtures/return-classes.probe.json"),
            FixtureShape {
                columns: 6,
                rows: 945,
                unstable: 0,
            },
        );
    }

    #[test]
    fn the_return_class_fixtures_kestra_column_is_the_harnesss_own_answer() {
        check_kestra_column(
            include_str!("../../oracle/return-classes.txt"),
            include_str!("../../oracle/fixtures/return-classes.answers.json"),
            6,
        );
    }

    /// The differential no measurement of a filter in isolation could make:
    /// 2 898 producer x consumer pairs, of which 38 were egret answering where
    /// Kestra throws. See [`JavaSeqKind`].
    #[test]
    fn the_collection_pairs_match_the_engine() {
        walk_fixture(
            include_str!("../../oracle/collection-pairs.txt"),
            include_str!("../../oracle/fixtures/collection-pairs.probe.json"),
            FixtureShape {
                columns: 7,
                rows: 2898,
                unstable: 46,
            },
        );
    }

    #[test]
    fn the_collection_pair_fixtures_kestra_column_is_the_harnesss_own_answer() {
        check_kestra_column(
            include_str!("../../oracle/collection-pairs.txt"),
            include_str!("../../oracle/fixtures/collection-pairs.answers.json"),
            7,
        );
    }

    /// **The depth-3 family, which the depth-2 cross could not see by
    /// construction.** `reverse` and `sort` return their input, so the kind
    /// survives them; nothing downstream of either was measured until this
    /// fixture existed, and five expressions answered here where Kestra throws.
    #[test]
    fn the_propagation_pairs_match_the_engine() {
        walk_fixture(
            include_str!("../../oracle/propagation-pairs.txt"),
            include_str!("../../oracle/fixtures/propagation-pairs.probe.json"),
            FixtureShape {
                columns: 8,
                rows: 4347,
                unstable: 60,
            },
        );
    }

    #[test]
    fn the_propagation_pair_fixtures_kestra_column_is_the_harnesss_own_answer() {
        check_kestra_column(
            include_str!("../../oracle/propagation-pairs.txt"),
            include_str!("../../oracle/fixtures/propagation-pairs.answers.json"),
            8,
        );
    }

    /// Package #5b, Task B1 (spec D13) wrote this as a placeholder in
    /// `runners::env` while the ten filters were unknown; Task B4 ported them
    /// and moved it here under the same name, so the seven
    /// `oracle/error-class-pairs.txt` rows pair with it. Each row fails for
    /// Kestra's reason, with Kestra's text, and not as an unknown filter.
    #[test]
    fn a_filter_input_kestra_refuses_is_refused_here_too() {
        for (template, context, want) in [
            (
                "{{ 'abc' | abbreviate(-1) }}",
                "{}",
                "Invalid argument to abbreviate filter; must be greater than zero",
            ),
            (
                "{{ n | abbreviate(3) }}",
                r#"{"n": 123456}"#,
                "class java.lang.Integer cannot be cast to class java.lang.String",
            ),
            (
                "{{ '!!' | base64decode }}",
                "{}",
                "Please provide a correctly Base64 encoded string containing an UTF-8 string",
            ),
            (
                "{{ 'a' | merge(['b']) }}",
                "{}",
                "The object being filtered is not a Map/List/Array",
            ),
            (
                "{{ '12' | numberformat }}",
                "{}",
                "The input for the 'NumberFormat' filter has to be a number.",
            ),
            (
                "{{ l | rsort }}",
                r#"{"l": [1.5, 3, 2]}"#,
                "class java.lang.Integer cannot be cast to class java.lang.Double",
            ),
            // Kestra's text ends `java.lang.Long` (a template literal is a
            // Long); egret names a small integer `Integer`, spec D2's
            // recorded Integer/Long divergence, so the class is not asserted.
            (
                "{{ [1, 'a'] | rsort }}",
                "{}",
                "class java.lang.String cannot be cast to class java.lang.",
            ),
        ] {
            let message = err(template, context);
            assert!(message.contains(want), "{template}: {message}");
            assert!(!message.contains("unknown filter"), "{template}: {message}");
        }
    }

    /// The ten answer as Pebble's classes do beyond the corpus: the branches
    /// `javap -c` shows that no golden entry reaches.
    #[test]
    fn pebble_core_filters_follow_the_jar() {
        for (template, context, want) in [
            // A width of three or less cuts without dots; zero empties a
            // string longer than three.
            ("{{ 'abcdef' | abbreviate(0) }}", "{}", ""),
            ("{{ 'abcdef' | abbreviate(2) }}", "{}", "ab"),
            ("{{ 'abcdef' | abbreviate(7) }}", "{}", "abcdef"),
            (
                "{{ s | abbreviate(5) }}",
                r#"{"s": "éééééé"}"#,
                "\u{e9}\u{e9}...",
            ),
            ("{{ 12 | base64encode }}", "{}", "MTI="),
            ("{{ [1, 2] | base64encode }}", "{}", "WzEsIDJd"),
            ("{{ 'YQ' | base64decode }}", "{}", "a"),
            (
                "{{ s | nl2br }}",
                r#"{"s": "a\rb\n\nc"}"#,
                "a<br />b<br /><br />c",
            ),
            ("{{ 'abc' | nl2br }}", "{}", "abc"),
            ("{{ [1, 2] | raw }}", "{}", "[1, 2]"),
            ("{{ {'a': 1} | raw }}", "{}", "{a=1}"),
            ("{{ 'a b+c' | urlencode }}", "{}", "a+b%2Bc"),
            ("{{ n | merge(['x']) }}", r#"{"n": null}"#, "[\"x\"]"),
            ("{{ ['a'] | merge(['b']) | length }}", "{}", "2"),
            ("{{ {'a': 1} | merge({'a': 2}) }}", "{}", "{\"a\":2}"),
            (
                "{{ [1] | merge({'a': 1, 'b': 2}) }}",
                "{}",
                "[1,{\"a\":1},{\"b\":2}]",
            ),
            ("{{ ['b', 'a', 'c'] | rsort | first }}", "{}", "c"),
            ("{{ [] | rsort }}", "{}", "[]"),
            ("{{ [2.5, 10.0, -1.0] | rsort }}", "{}", "[10.0,2.5,-1.0]"),
            (
                "{{ 1234567 | numberformat(format='#,##0') }}",
                "{}",
                "1,234,567",
            ),
            ("{{ n | numberformat(n) }}", r#"{"n": null}"#, ""),
            (
                "{{ 1234.5 | numberformat(f) }}",
                r#"{"f": null}"#,
                "1,234.5",
            ),
            ("{{ 0.5 | numberformat('0') }}", "{}", "0"),
            ("{{ 2.5 | numberformat('0') }}", "{}", "2"),
            ("{{ -0.001 | numberformat('0.00') }}", "{}", "-0.00"),
            ("{{ 0.0125 | numberformat('0.0%') }}", "{}", "1.2%"),
            (
                "{{ fromIon(ion='0.0025') | numberformat('0.000') }}",
                "{}",
                "0.002",
            ),
            (
                "{{ 'ab' | sha256 }}",
                "{}",
                "fb8e20fc2e4c3f248c60c39bd652f3c1347298bb977b8b4d5903b85055620603",
            ),
        ] {
            assert_eq!(ok(template, context), want, "{template}");
        }
        for (template, context, want) in [
            (
                "{{ 'abc' | abbreviate }}",
                "{}",
                "because the return value of",
            ),
            (
                "{{ 'abc' | abbreviate(1.5) }}",
                "{}",
                "cannot be cast to class java.lang.Long",
            ),
            (
                "{{ 12 | base64decode }}",
                "{}",
                "This filter applies to String",
            ),
            (
                "{{ 'YQ=' | base64decode }}",
                "{}",
                "correctly Base64 encoded",
            ),
            (
                "{{ 'a b' | base64decode }}",
                "{}",
                "correctly Base64 encoded",
            ),
            (
                "{{ 12 | nl2br }}",
                "{}",
                "nl2br filters only supports String input.",
            ),
            (
                "{{ 12 | urlencode }}",
                "{}",
                "cannot be cast to class java.lang.String",
            ),
            ("{{ 12 | sha256 }}", "{}", "Need a string to hash"),
            (
                "{{ n | merge(n) }}",
                r#"{"n": null}"#,
                "The two arguments to be merged are null",
            ),
            (
                "{{ [1] | merge('x') }}",
                "{}",
                "only Maps and Lists can be merged with a List. Arg2: java.lang.String",
            ),
            (
                "{{ {'a': 1} | merge(2) }}",
                "{}",
                "only Maps and Lists can be merged with a Map. Arg2: java.lang.Integer",
            ),
            (
                "{{ {'a': 1} | keys | merge([1]) }}",
                "{}",
                "not a Map/List/Array",
            ),
            (
                "{{ 'a' | rsort }}",
                "{}",
                "class java.lang.String cannot be cast to class java.util.List",
            ),
            (
                "{{ 'a,b' | split(',') | rsort }}",
                "{}",
                "cannot be cast to class java.util.List",
            ),
            ("{{ [[2], [1]] | flatten | rsort }}", "{}", "immutable list"),
            ("{{ true | numberformat }}", "{}", "has to be a number"),
            (
                "{{ 1 | numberformat(2) }}",
                "{}",
                "cannot be cast to class java.lang.String",
            ),
            (
                "{{ 1 | numberformat('#,') }}",
                "{}",
                "Malformed pattern \"#,\"",
            ),
            (
                "{{ 1 | numberformat('0.0E0') }}",
                "{}",
                "scientific notation",
            ),
        ] {
            let message = err(template, context);
            assert!(message.contains(want), "{template}: {message}");
        }
        // An undefined input is refused as strict variables refuse it, never
        // treated as a null.
        for filter in [
            "abbreviate(3)",
            "base64decode",
            "base64encode",
            "merge([1])",
            "nl2br",
            "numberformat",
            "raw",
            "rsort",
            "sha256",
            "urlencode",
        ] {
            for subject in ["nothere", "obj.missing"] {
                let template = format!("<{{{{ {subject} | {filter} }}}}>");
                let message = err(&template, r#"{"obj": {}}"#);
                assert!(
                    message.contains("an undefined value reached"),
                    "{template}: {message}"
                );
            }
        }
        let message = err("{{ [1] | merge(nothere) }}", "{}");
        assert!(message.contains("an undefined value reached"), "{message}");
    }
}
