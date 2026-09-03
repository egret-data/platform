// kestra: core/src/main/java/io/kestra/core/runners/pebble/Extension.java
// kestra: core/src/main/java/io/kestra/core/runners/pebble/functions/*.java

//! W2 Task 7 — the **pure** functions of Kestra's Pebble extension: the ones
//! that reach no service and can be answered from their arguments alone.
//!
//! # The wire name is a `NAME` constant, not a file name
//!
//! `Extension.getFunctions()` registers each class under its own
//! `XxxFunction.NAME`, and eight of them do not match the class: `range` is
//! `BoundedRangeFunction`, `read` is `ReadFileFunction`, `id`/`ksuid`/`uuid`
//! are `IDFunction`/`KSUIDFunction`/`UUIDFunction`, `nanoId` is
//! `NanoIDFunction` (lower-case `d`), `fileURI` is `FileURIFunction`, and
//! `FetchContextFunction` is registered **twice**, as `fetchContext` and as
//! `printContext`. Read out of the artifact's bytecode
//! (`javap -p -c -constants`) and cross-checked against `host.Oracle dump`,
//! which reports the 34 names the built engine actually carries.
//!
//! This module owns 19 of those 34:
//!
//! ```text
//! now uuid id ksuid nanoId randomInt randomPort range
//! fromJson fromIon yaml
//! dayOfMonth dayOfWeek hourOfDay monthOfYear
//! isWeekend isPublicHoliday isLastWorkingDay isDayWeekInMonth
//! ```
//!
//! The other 15 are Task 8's: the eight null-backed ones (`secret`, `kv`,
//! `read`, `fileURI`, `fileSize`, `isFileEmpty`, `fileExists`, `http`),
//! `encrypt`/`decrypt`, `env`, `fetchContext`/`printContext`,
//! `tasksWithState` and `loopOutputs`.
//!
//! # `range` was already bound, and it did not error
//!
//! **This is the trap that has now caught three tasks.** `indent` and
//! `string` are minijinja filters Task 4 overrode; `range` is a minijinja
//! **global**, and it resolves, runs and answers minijinja's way. Derived
//! from the lock file rather than from a list: minijinja 2.24.0's
//! `defaults::build_globals` inserts exactly four names under the `builtins`
//! feature — `range`, `dict`, `debug`, `namespace` — and `Environment` keeps
//! one map, so `add_function` **replaces** silently. Of this module's 19
//! names, `range` is the only collision; `debug` is already removed in
//! [`super::env::environment`].
//!
//! And the two disagree about the answer, not merely about the spelling:
//!
//! | | minijinja's `range` | Kestra's `BoundedRangeFunction` |
//! |---|---|---|
//! | `range(1, 3)` | `[1, 2]` — **exclusive** | `[1,2,3]` — **inclusive** |
//! | arguments | positional only | `start`/`end`/`increment`, either way |
//! | `range('a', 'e')` | an error | `["a","b","c","d","e"]` |
//! | size | unbounded | `RenderLimits.ensureAtMost(…, 1 000 000, …)` |
//!
//! # Seven of these are non-deterministic, and a shape check cannot see what
//! they guarantee
//!
//! `now`, `uuid`, `id`, `ksuid`, `nanoId`, `randomInt` and `randomPort` are
//! `tests/oracle.rs`'s `NON_DETERMINISTIC` set: compared for shape, never for
//! value. A shape check sees a length. It does not see a version nibble, an
//! alphabet, or which end of a range is inclusive — so each of those is
//! pinned here instead, and four of them are pinned **by exact value through
//! the oracle**, because the arithmetic allows it:
//!
//! - `randomInt(lower=5, upper=6)` is `5`, always —
//!   `floor(random() * (upper - lower)) + lower` over a one-wide range. That
//!   is lower-inclusive and upper-EXCLUSIVE, by value, in the differential;
//! - `nanoId(length=6, alphabet='x')` is `xxxxxx`, always, and
//!   `alphabet='xxx'` exercises the other selection path (a non-power-of-two
//!   alphabet is rejection-sampled) to the same deterministic answer;
//! - `uuid() | regexMatch(regex='…-7[0-9a-f]{3}-[89ab]…')` is `true`, always,
//!   which pins the version and variant nibbles and the lower-case hex that
//!   no length check can;
//! - `ksuid()`, `id()` and `nanoId()`'s alphabets are pinned the same way.
//!
//! # Argument defaults are documentation, not behaviour
//!
//! `KestraFunction.getArgumentDefaults()` is Kestra's own interface method,
//! not Pebble's, and nothing on the render path reads it — `RandomIntFunction`
//! declares `lower=0, upper=10` and `{{ randomInt() }}` still throws
//! `The 'randomIn()' function expects an argument lower`. Measured through the
//! harness. So an absent argument is absent here too, exactly as
//! [`super::filters::Args::get`] reports it.

use std::net::TcpListener;

use chrono::{Datelike, Days, Months, NaiveDate, NaiveDateTime, Timelike, Utc};
use minijinja::value::{Rest, Value as MjValue, ValueKind};
use minijinja::{Environment, Error as MjError};
use rand::RngCore;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

use super::env::OrderedMap;
use super::filters::{is_java_null, pebble_error, Args, DateArgs};
use super::ionfmt::read_ion;
use super::javafmt::{parse_iso, Zdt, Zone};
use super::jqsandbox::MAX_VALUE_DEPTH;

/// Registers the 19 pure functions onto `env`.
///
/// **`range` is an override, not an addition** — see this module's doc.
pub(super) fn register(env: &mut Environment<'static>) {
    env.add_function("dayOfMonth", day_of_month);
    env.add_function("dayOfWeek", day_of_week);
    env.add_function("fromIon", from_ion);
    env.add_function("fromJson", from_json);
    env.add_function("hourOfDay", hour_of_day);
    env.add_function("id", id);
    env.add_function("isDayWeekInMonth", is_day_week_in_month);
    env.add_function("isLastWorkingDay", is_last_working_day);
    env.add_function("isPublicHoliday", is_public_holiday);
    env.add_function("isWeekend", is_weekend);
    env.add_function("ksuid", ksuid);
    env.add_function("monthOfYear", month_of_year);
    env.add_function("nanoId", nano_id);
    env.add_function("now", now);
    env.add_function("randomInt", random_int);
    env.add_function("randomPort", random_port);
    // Overrides minijinja's own `range` global, which is exclusive and
    // positional-only.
    env.add_function("range", range);
    env.add_function("uuid", uuid);
    env.add_function("yaml", yaml);
}

// ---------------------------------------------------------------------------
// The structured-data readers
// ---------------------------------------------------------------------------

/// `FromJsonFunction` — `JacksonMapper.ofJson().readValue(json, Object.class)`.
///
/// Three branches, and they are not the same as `yaml`'s two lines below:
/// an **absent** `json` throws, a `json` that is Java `null` returns `null`
/// (which renders as nothing), and a non-`String` throws.
///
/// **Trailing content is not an error.** `ObjectMapper.readValue(String, …)`
/// does not enable `FAIL_ON_TRAILING_TOKENS`, so `fromJson('1 2')` reads the
/// first value and stops — measured, `1`. That is the same fact
/// `super::filters::jq`'s `readTree` branch records, and `serde_json`'s
/// `from_str` is the opposite, so this goes through `StreamDeserializer`.
fn from_json(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let Some(value) = args.get("json", 0) else {
        return Err(pebble_error(
            "The 'fromJson' function expects an argument 'json'.",
        ));
    };
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(text) = value.as_str() else {
        return Err(pebble_error(
            "The 'fromJson' function expects an argument 'json' with type string.",
        ));
    };
    // No `de.end()`: reading one value and leaving the rest is exactly what
    // Jackson does, and calling it would turn `fromJson('1 2')` into an error.
    let mut de = serde_json::Deserializer::from_str(text);
    Depth(1)
        .deserialize(&mut de)
        .map_err(|e| pebble_error(format!("Invalid json: {e}")))
}

/// `FromIonFunction` — `FileSerde.readAll(...)`, a Jackson `MappingIterator`
/// over a stream of top-level Ion values.
///
/// `allRows` picks between `toList()` and `findFirst().orElse(null)`, so the
/// default reads the **first value and drops the rest** rather than failing.
/// A `null` input returns `null`; anything that is not a `String` (or, in
/// Kestra, a `byte[]`, which no Pebble context can carry) throws.
///
/// **`fromIon('null')` throws**, which is upstream defect 6's last row and is
/// reachable without writing it by hand: `{{ nothing | toIon }}` is the
/// four-character string `null` and this function then refuses it. Both
/// halves are corpus entries.
fn from_ion(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let Some(value) = args.get("ion", 0) else {
        return Err(pebble_error(
            "The 'fromIon' function expects an argument 'ion'.",
        ));
    };
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(text) = value.as_str() else {
        return Err(pebble_error(
            "The 'fromIon' function expects an argument 'ion' with type string or byte[].",
        ));
    };
    let all_rows = args
        .get("allRows", 1)
        .is_some_and(|flag| flag.kind() == ValueKind::Bool && flag.is_true());

    // **`all_rows` decides how much of the document is even PARSED.**
    // `findFirst()` is `take(1)`, so Kestra never looks past the first value:
    // `fromIon('1 /*c')` and `fromIon('1 +5')` are both `1` there, and were
    // errors here the moment this reader grew refusals. `toList()` does read
    // the tail, and every one of those documents throws under `allRows=true`.
    // Measured, both ways, over fourteen documents.
    let rows = read_ion(text, all_rows).map_err(|e| pebble_error(format!("Invalid ion: {e}")))?;
    // `sink.next(t)` refuses a null element. Over a lazily-limited `rows` this
    // is exactly "the first value is null", which is what `fromIon('null 1')`
    // throws on and `fromIon('1 null')` does not.
    if rows.iter().any(is_java_null) {
        return Err(pebble_error("Invalid ion: t is null in sink.next(t)"));
    }
    if all_rows {
        return Ok(MjValue::from(rows));
    }
    Ok(rows.into_iter().next().unwrap_or_else(|| MjValue::from(())))
}

/// `YamlFunction` — `new ObjectMapper(new YAMLFactory()).readValue(yaml,
/// Object.class)`.
///
/// **It has no null branch, and `fromJson` does.** `yaml(yaml=nothing)` is
/// `null instanceof String`, which is false, so it reaches
/// `The 'yaml' function expects an argument 'yaml' with type string.` while
/// `fromJson(json=nothing)` returns null and renders as nothing. Two
/// functions in the same batch, two answers to the same input; both are
/// corpus entries.
///
/// # The plain-scalar resolver is YAML 1.2 here and YAML 1.1 in Kestra
///
/// **A recorded divergence, not a defect of this port, and it is the reason
/// the crate underneath is called `serde_norway`.** SnakeYAML resolves the
/// YAML 1.1 schema and `serde_norway` resolves the 1.2 core schema, and the
/// two disagree about a closed set of *plain* (unquoted) spellings — measured
/// through the harness:
///
/// | plain scalar | Kestra | egret |
/// |---|---|---|
/// | `yes`, `on` | `true` | the string |
/// | `y`, `n` | the string | the string (they agree) |
/// | `010` | `8` — a leading zero is octal | the string |
/// | `0o10` | the string | `8` |
/// | `.inf`, `.nan` | **throws** | **refused**, see [`BoundedVisitor::visit_f64`] |
///
/// It is recorded rather than reproduced because the fix is not available:
/// reproducing it needs to know whether a scalar was **quoted**, and
/// `serde_norway` hands a visitor `"yes"` and `yes` identically —
/// `visit_untagged_scalar` funnels both into `visit_str`. Re-resolving on the
/// string alone would turn `a: "010"` into `8`, which is a *new* silent wrong
/// answer traded for an old one. `function-yaml-yes-is-a-boolean-or-not` and
/// `function-yaml-octal-looking` are baselined with this reason.
///
/// # This is the READ half of the filter `super::yamlfmt` writes
///
/// Three of upstream defect 6's rows only exist end to end, and all three are
/// pinned as round trips through this function: `'42' | yaml` writes the
/// unquoted scalar `42` and this reads it back as an **integer**, `'1.5'`
/// comes back a double, `'1e5'` comes back `100000.0`, and `'.inf'` is
/// written as `.inf` and then **refused** here.
fn yaml(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let Some(value) = args.get("yaml", 0) else {
        return Err(pebble_error(
            "The 'yaml' function expects an argument 'yaml'.",
        ));
    };
    let Some(text) = value.as_str() else {
        return Err(pebble_error(
            "The 'yaml' function expects an argument 'yaml' with type string.",
        ));
    };
    // **The FIRST document, and an empty stream is an error.** Measured:
    // `yaml('a: 1\n---\nb: 2')` is `{"a":1}` in Kestra and was
    // `deserializing from YAML containing more than one document is not
    // supported` here; `yaml('')`, `yaml('   ')` and `yaml('# comment')` all
    // throw `No content to map due to end-of-input` there and answered
    // **null** here, which renders as nothing — a wrong value that renders
    // successfully.
    if is_empty_yaml_document(text) {
        return Err(pebble_error(
            "Invalid yaml: No content to map due to end-of-input",
        ));
    }
    let Some(document) = serde_norway::Deserializer::from_str(text).next() else {
        return Err(pebble_error(
            "Invalid yaml: No content to map due to end-of-input",
        ));
    };
    Depth(1)
        .deserialize(document)
        .map_err(|e| pebble_error(format!("Invalid yaml: {e}")))
}

/// Is there no content at all? — every line blank or a comment.
///
/// `serde_norway` deserialises such a source to a **unit**, which renders as
/// nothing; Jackson throws `No content to map due to end-of-input`. So the
/// question has to be asked of the source, before the parser, and it is asked
/// the narrowest way that answers it: a document with any content has at least
/// one line whose first non-space character is not `#`.
///
/// It cannot mis-fire on a real document, and it must not be confused with a
/// document whose content IS null: `yaml('~')` and `yaml('a: null')` are
/// `null` and `{}` on both engines, and neither is empty by this rule.
fn is_empty_yaml_document(text: &str) -> bool {
    text.lines()
        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
}

/// A serde seed that carries the nesting depth, so **both** readers above are
/// bounded by one number.
///
/// # Why a seed rather than a plain `Deserialize`
///
/// `super::env::OrderedJson` is the same visitor without a depth, and it is
/// enough for a context value because `serde_json`'s parser has a recursion
/// limit of its own (128, the number [`MAX_VALUE_DEPTH`] is taken from). YAML
/// has no such limit anywhere: `serde_norway`'s parser is an event stream, so
/// the only recursion is this visitor's, and `a: {a: {a: …` is a filter
/// argument a flow author writes. A `DeserializeSeed` is the one serde shape
/// that can carry state into `next_element`/`next_value`, which is what makes
/// the depth a parameter rather than a thread-local.
///
/// The bound is the crate's existing value-depth invariant rather than a new
/// number: *no value anywhere in this crate is more than `MAX_VALUE_DEPTH`
/// deep*, which is what lets `env::write_json`, `ionfmt::write_ion`,
/// `yamlfmt::to_yaml` and `env::kestra_to_string` walk anything they are
/// handed on a 2 MiB worker.
struct Depth(usize);

impl<'de> DeserializeSeed<'de> for Depth {
    type Value = MjValue;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<MjValue, D::Error> {
        if self.0 > MAX_VALUE_DEPTH {
            return Err(de::Error::custom(format!(
                "the value nests more than {MAX_VALUE_DEPTH} levels deep, which is what every \
                 writer in this crate can walk"
            )));
        }
        d.deserialize_any(BoundedVisitor(self.0))
    }
}

struct BoundedVisitor(usize);

impl<'de> Visitor<'de> for BoundedVisitor {
    type Value = MjValue;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("any value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<MjValue, E> {
        Ok(MjValue::from(()))
    }

    fn visit_none<E: de::Error>(self) -> Result<MjValue, E> {
        self.visit_unit()
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<MjValue, E> {
        Ok(MjValue::from(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<MjValue, E> {
        Ok(MjValue::from(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<MjValue, E> {
        Ok(MjValue::from(v))
    }

    /// **A non-finite float is a refusal, not a value**, and that is fidelity
    /// rather than caution.
    ///
    /// YAML 1.1 resolves `.inf`, `-.inf` and `.nan`; Jackson's own numeric
    /// coercion, one layer above SnakeYAML, then rejects them —
    /// `Malformed numeric value '.inf'`. That is upstream defect 6's sharpest
    /// row: the `yaml` FILTER writes `.inf` unquoted and this function, its
    /// own reader, refuses to read it back.
    ///
    /// **This arm is exact in effect, and the first version of this sentence
    /// gave the wrong reason for it.** It said "nothing else in either format
    /// produces a non-finite `f64`". What is actually true, measured: the only
    /// other candidate is a huge exponent, and `serde_norway` leaves `1e400`
    /// a **string** rather than resolving it to infinity — so this arm is
    /// reached only by `.inf`/`.nan`, but by luck of the other resolver rather
    /// than by construction. `1e400` is a separate, recorded divergence
    /// (Kestra answers `Infinity`; egret answers the string `1e400`, and
    /// `fromJson('1e400')` errors where Kestra answers `Infinity` because
    /// `serde_json` rejects it in the parser).
    ///
    /// The message names the canonical spelling, so `.Inf` and `.NaN` are
    /// reported as `.inf` and `.nan` where Kestra echoes the source — the raw
    /// text does not reach a `visit_f64`.
    /// `function-yaml-refuses-what-its-own-writer-wrote` is the entry.
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<MjValue, E> {
        if !v.is_finite() {
            return Err(de::Error::custom(format!(
                "Malformed numeric value '{}'",
                if v.is_nan() {
                    ".nan"
                } else if v > 0.0 {
                    ".inf"
                } else {
                    "-.inf"
                }
            )));
        }
        Ok(MjValue::from(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<MjValue, E> {
        Ok(MjValue::from(v.to_string()))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<MjValue, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(Depth(self.0 + 1))? {
            items.push(item);
        }
        Ok(MjValue::from(items))
    }

    /// Insertion order, not sorted: `{"b":1,"a":2}` reads back `{"b":1,"a":2}`
    /// through both readers, which `function-fromjson-key-order` and
    /// `function-yaml-key-order` pin.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<MjValue, A::Error> {
        let mut entries: Vec<(String, MjValue)> = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(Depth(self.0 + 1))?;
            entries.push((key, value));
        }
        Ok(MjValue::from_object(OrderedMap::new(entries)))
    }
}

// ---------------------------------------------------------------------------
// range
// ---------------------------------------------------------------------------

/// `RenderLimits.ensureAtMost`'s ceiling for `range`, read off
/// `BoundedRangeFunction.MAX_RANGE_SIZE` in the artifact: `ConstantValue:
/// 1000000l`.
///
/// This is Kestra's own bound, ported rather than invented — one of the only
/// two `RenderLimits` call sites in the whole jar (the other is
/// `AbstractIndent`).
const MAX_RANGE_SIZE: i128 = 1_000_000;

/// `BoundedRangeFunction` — Pebble's `RangeFunction` with a size guard in
/// front of it.
///
/// The guard is `|end - start| / |increment| + 1`, **not** `|end - start|`,
/// and it runs only when `start` and `end` are both numbers and `increment`
/// is not zero. `function-range-counts-by-increment` is the entry that
/// separates the two: it spans two million and produces two elements.
///
/// # Two ways Kestra's own loop does not terminate, and egret refuses both
///
/// Neither has a golden, for the reason C7 states: at the sizes that matter
/// Kestra hangs rather than answering.
///
/// - **The guard's arithmetic is `Math.abs(end - start)` on Java longs, which
///   wraps.** `range(start=-9223372036854775808, end=9223372036854775807)` is
///   `abs(-1)` = 1, so the guard sees two elements and the loop then runs
///   1.8 × 10^19 times. The count here is computed in `i128`, so it cannot
///   wrap, and the call is refused.
/// - **A character range's increment is truncated to an `int` by the loop's
///   own `(int)((long) i + increment)`.** An increment of 2^32 therefore
///   leaves `i` unchanged and the loop never ends. Refused, by name.
fn range(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let start = args.get("start", 0).unwrap_or_default();
    let end = args.get("end", 1).unwrap_or_default();
    let increment = match args.get("increment", 2) {
        None => 1i64,
        Some(value) if is_java_null(&value) => 1i64,
        Some(value) if value.kind() == ValueKind::Number => number_as_long(&value)?,
        Some(value) => {
            // The trailing space is Pebble's own constant, proven by the
            // golden: `must be a number x`, not `numberx`. One character, and
            // the test that was meant to hold it pinned the wrong sentence
            // instead -- exactly what error-class comparison hides.
            return Err(pebble_error(format!(
                "The increment of the range function must be a number {}",
                super::env::kestra_to_string(&value)
            )));
        }
    };

    if start.kind() == ValueKind::Number && end.kind() == ValueKind::Number {
        let from = number_as_long(&start)?;
        let to = number_as_long(&end)?;
        if increment != 0 {
            let count = (i128::from(to) - i128::from(from)).abs() / i128::from(increment).abs() + 1;
            if count > MAX_RANGE_SIZE {
                return Err(pebble_error(format!(
                    "The range function cannot produce more than {MAX_RANGE_SIZE} elements, \
                     but this call would produce {count}."
                )));
            }
        }
        return numeric_range(from, to, increment);
    }

    let (Some(from), Some(to)) = (start.as_str(), end.as_str()) else {
        return Err(pebble_error(
            "Arguments of range function must be of type Number or String with a length of 1",
        ));
    };
    let (from, to) =
        match (one_char(from), one_char(to)) {
            (Some(from), Some(to)) => (from, to),
            _ => return Err(pebble_error(
                "Arguments of range function must be of type Number or String with a length of 1",
            )),
        };
    char_range(from, to, increment)
}

/// `Number.longValue()` on a range bound.
fn number_as_long(value: &MjValue) -> Result<i64, MjError> {
    if value.is_integer() {
        return i64::try_from(value.clone()).map_err(|_| pebble_error("long overflow"));
    }
    f64::try_from(value.clone())
        .map(|f| f as i64)
        .map_err(|_| pebble_error("long overflow"))
}

/// One UTF-16 code unit, which is what `String.charAt(0)` reads and what the
/// `length() != 1` guard measures.
fn one_char(s: &str) -> Option<u32> {
    let mut units = s.encode_utf16();
    let first = units.next()?;
    units.next().is_none().then_some(u32::from(first))
}

fn numeric_range(from: i64, to: i64, increment: i64) -> Result<MjValue, MjError> {
    if increment == 0 {
        return Err(pebble_error(
            "The increment of the range function must be different than 0",
        ));
    }
    let mut items = Vec::new();
    let mut at = from;
    loop {
        if increment > 0 && at > to {
            break;
        }
        if increment < 0 && at < to {
            break;
        }
        items.push(MjValue::from(at));
        // Java's `long` addition wraps and its loop then never ends; a
        // checked add turns that into a finite range instead.
        match at.checked_add(increment) {
            Some(next) => at = next,
            None => break,
        }
    }
    Ok(MjValue::from(items))
}

fn char_range(from: u32, to: u32, increment: i64) -> Result<MjValue, MjError> {
    if increment == 0 {
        return Err(pebble_error(
            "The increment of the range function must be different than 0",
        ));
    }
    // `(int)((long) i + increment)`: an increment whose low 32 bits are zero
    // does not move the loop variable at all.
    let step = increment as i32;
    if step == 0 {
        return Err(pebble_error(
            "the range function's increment truncates to 0 as a 32-bit character step, \
             which never advances the range",
        ));
    }
    let mut items = Vec::new();
    let mut at = from as i32;
    loop {
        if step > 0 && at > to as i32 {
            break;
        }
        if step < 0 && at < to as i32 {
            break;
        }
        let unit = u16::try_from(at & 0xffff).expect("masked to 16 bits");
        items.push(MjValue::from(String::from_utf16_lossy(&[unit])));
        match at.checked_add(step) {
            Some(next) => at = next,
            None => break,
        }
    }
    Ok(MjValue::from(items))
}

// ---------------------------------------------------------------------------
// The calendar functions
// ---------------------------------------------------------------------------

/// `(String) args.get("date")`, then `DateUtils.parseLocalDate`.
///
/// The cascade is `LocalDate.parse` -> `ZonedDateTime.parse().toLocalDate()`
/// -> `LocalDateTime.parse().toLocalDate()`, and every one of them reads the
/// **local** date, so `2024-01-06T14:30:00Z` is the 6th whatever zone anyone
/// asks about it. [`parse_iso`] is that grammar.
fn local_date(args: &Args, function: &str) -> Result<NaiveDate, MjError> {
    Ok(date_time(args, function)?.date())
}

fn date_time(args: &Args, function: &str) -> Result<NaiveDateTime, MjError> {
    let value = args.get("date", 0).filter(|v| !is_java_null(v));
    let Some(value) = value else {
        return Err(pebble_error(format!(
            "The '{function}()' function expects a 'date' argument."
        )));
    };
    let text = super::env::kestra_to_string(&value);
    let zone = Zone::resolve(None).map_err(pebble_error)?;
    let zdt: Zdt = parse_iso(&text, &zone).map_err(|e| {
        pebble_error(format!(
            "The '{function}()' function could not parse 'date': {e}"
        ))
    })?;
    Ok(zdt.local())
}

/// `DayOfMonthFunction` — `parseLocalDate(date).getDayOfMonth()`.
fn day_of_month(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    Ok(MjValue::from(local_date(&args, "dayOfMonth")?.day()))
}

/// `MonthOfYearFunction` — `parseLocalDate(date).getMonthValue()`.
fn month_of_year(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    Ok(MjValue::from(local_date(&args, "monthOfYear")?.month()))
}

/// `DayOfWeekFunction` — `parseLocalDate(date).getDayOfWeek().name()`, so the
/// answer is the enum's own English name and no locale reaches it.
fn day_of_week(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date = local_date(&args, "dayOfWeek")?;
    Ok(MjValue::from(day_name(date)))
}

/// `HourOfDayFunction` — `ZonedDateTime.parse(...).getHour()`, falling back to
/// `LocalDateTime.parse(...).getHour()`.
///
/// **This is the one calendar function that does not use
/// `DateUtils.parseLocalDate`**, and the difference shows: a date with no time
/// at all is an error here (`Text '2024-01-06' could not be parsed at index
/// 10`) where every sibling accepts it. And `getHour()` is the hour of the
/// **parsed offset**, not of UTC — `2024-01-06T14:30:00+07:00` is 14.
fn hour_of_day(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let value = args.get("date", 0).filter(|v| !is_java_null(v));
    let Some(value) = value else {
        return Err(pebble_error(
            "The 'hourOfDay()' function expects a 'date' argument.",
        ));
    };
    let text = super::env::kestra_to_string(&value);
    // Neither `ZonedDateTime.parse` nor `LocalDateTime.parse` accepts a
    // date-only string; only `LocalDate.parse`, which this function does not
    // call, does.
    if !text.contains(['T', 't']) {
        return Err(pebble_error(format!(
            "The 'hourOfDay()' function could not parse 'date': Text '{text}' could not be \
             parsed: it carries no time"
        )));
    }
    Ok(MjValue::from(date_time(&args, "hourOfDay")?.hour()))
}

/// `IsWeekendFunction` — `SATURDAY` or `SUNDAY`, spelled out in the bytecode.
///
/// **No `WeekFields`, so no locale.** The four-letter enum constants are
/// compared directly, which is why this one is answerable where `w`, `W`,
/// `Y`, `e` and `c` are refused in [`super::javafmt`]: those resolve through
/// `WeekFields.of(locale)` and this does not.
fn is_weekend(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date = local_date(&args, "isWeekend")?;
    Ok(MjValue::from(is_weekend_day(date)))
}

fn is_weekend_day(date: NaiveDate) -> bool {
    matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun)
}

fn day_name(date: NaiveDate) -> &'static str {
    match date.weekday() {
        chrono::Weekday::Mon => "MONDAY",
        chrono::Weekday::Tue => "TUESDAY",
        chrono::Weekday::Wed => "WEDNESDAY",
        chrono::Weekday::Thu => "THURSDAY",
        chrono::Weekday::Fri => "FRIDAY",
        chrono::Weekday::Sat => "SATURDAY",
        chrono::Weekday::Sun => "SUNDAY",
    }
}

/// `IsLastWorkingDayFunction`.
///
/// `workingDays` is split on `[,\s]+`, upper-cased and read through
/// `DayOfWeek.valueOf`; a blank or absent argument is Monday-to-Friday. The
/// answer is then: the date is a working day, **and** walking back from the
/// last day of its month over non-working days lands on it.
///
/// A blank `workingDays` is `isBlank()`, not `isEmpty()`, so `'   '` is the
/// default set rather than an error — measured.
fn is_last_working_day(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date = local_date(&args, "isLastWorkingDay")?;
    let working = working_days(&args)?;

    // **This guard is an EQUIVALENT MUTANT and it is kept on purpose.**
    // Deleting it changes no answer, because the walk below always ends on a
    // working day, so `last == date` already implies that `date` is one. A
    // mutation run replaced the condition with `false` and nothing failed;
    // that is the implementation being redundant, not the tests being weak,
    // and Kestra's own code has the same shape. Recorded here so the next
    // reader does not re-derive it or "fix" the tests that cannot kill it.
    if !working.contains(&day_name(date)) {
        return Ok(MjValue::from(false));
    }
    let mut last = last_day_of_month(date);
    while !working.contains(&day_name(last)) {
        let Some(previous) = last.checked_sub_days(Days::new(1)) else {
            return Ok(MjValue::from(false));
        };
        last = previous;
    }
    Ok(MjValue::from(last == date))
}

const DEFAULT_WORKING_DAYS: &[&str] = &["MONDAY", "TUESDAY", "WEDNESDAY", "THURSDAY", "FRIDAY"];

fn working_days(args: &Args) -> Result<Vec<&'static str>, MjError> {
    let raw = match args.get("workingDays", 1) {
        None => None,
        Some(value) if is_java_null(&value) => None,
        Some(value) => Some(super::env::kestra_to_string(&value)),
    };
    let Some(raw) = raw.filter(|text| !text.trim().is_empty()) else {
        return Ok(DEFAULT_WORKING_DAYS.to_vec());
    };
    let mut days: Vec<&'static str> = Vec::new();
    for token in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let Some(day) = DAY_NAMES
            .iter()
            .find(|name| name.eq_ignore_ascii_case(token))
        else {
            return Err(pebble_error(format!(
                "The 'isLastWorkingDay()' function received an invalid 'workingDays' value: \
                 '{token}'. Expected day names like MONDAY, TUESDAY, etc."
            )));
        };
        if !days.contains(day) {
            days.push(day);
        }
    }
    if days.is_empty() {
        return Err(pebble_error(
            "The 'isLastWorkingDay()' function requires at least one valid day in 'workingDays'.",
        ));
    }
    Ok(days)
}

const DAY_NAMES: &[&str] = &[
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
    "SUNDAY",
];

fn last_day_of_month(date: NaiveDate) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date);
    first
        .checked_add_months(Months::new(1))
        .and_then(|next| next.pred_opt())
        .unwrap_or(date)
}

/// `IsDayWeekInMonthFunction` — `TemporalAdjusters.firstInMonth(day)` plus 0,
/// 1, 2 or 3 weeks, or `lastInMonth(day)`, compared with `isEqual`.
///
/// **`FOURTH` is not `LAST`.** `firstInMonth(...).plusWeeks(3)` can leave the
/// month entirely when the first occurrence falls late, so a five-occurrence
/// month has a `FOURTH` and a `LAST` that are different days.
/// `function-isdayweekinmonth-fourth-is-not-last` is the entry.
fn is_day_week_in_month(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date = local_date(&args, "isDayWeekInMonth")?;

    let day = args.get("dayOfWeek", 1).filter(|v| !is_java_null(v));
    let Some(day) = day else {
        return Err(pebble_error(
            "The 'isDayWeekInMonth()' function expects a 'dayOfWeek' argument.",
        ));
    };
    let position = args.get("position", 2).filter(|v| !is_java_null(v));
    let Some(position) = position else {
        return Err(pebble_error(
            "The 'isDayWeekInMonth()' function expects a 'position' argument.",
        ));
    };

    let day_text = super::env::kestra_to_string(&day).to_uppercase();
    let Some(day) = DAY_NAMES.iter().find(|name| **name == day_text) else {
        return Err(pebble_error(format!(
            "The 'isDayWeekInMonth()' function received an invalid 'dayOfWeek' value: \
             '{day_text}'. Expected one of: MONDAY, TUESDAY, WEDNESDAY, THURSDAY, FRIDAY, \
             SATURDAY, SUNDAY."
        )));
    };
    let position_text = super::env::kestra_to_string(&position).to_uppercase();
    let weeks = match position_text.as_str() {
        "FIRST" => 0u32,
        "SECOND" => 1,
        "THIRD" => 2,
        "FOURTH" => 3,
        "LAST" => {
            let target = last_in_month(date, day);
            return Ok(MjValue::from(target == Some(date)));
        }
        other => {
            return Err(pebble_error(format!(
                "The 'isDayWeekInMonth()' function received an invalid 'position' value: \
                 '{other}'. Expected one of: FIRST, SECOND, THIRD, FOURTH, LAST."
            )))
        }
    };
    let target = first_in_month(date, day)
        .and_then(|first| first.checked_add_days(Days::new(u64::from(weeks) * 7)));
    Ok(MjValue::from(target == Some(date)))
}

fn first_in_month(date: NaiveDate, day: &str) -> Option<NaiveDate> {
    let mut at = NaiveDate::from_ymd_opt(date.year(), date.month(), 1)?;
    for _ in 0..7 {
        if day_name(at) == day {
            return Some(at);
        }
        at = at.checked_add_days(Days::new(1))?;
    }
    None
}

fn last_in_month(date: NaiveDate, day: &str) -> Option<NaiveDate> {
    let mut at = last_day_of_month(date);
    for _ in 0..7 {
        if day_name(at) == day {
            return Some(at);
        }
        at = at.checked_sub_days(Days::new(1))?;
    }
    None
}

/// `IsPublicHolidayFunction` — **refused**, and refused by name.
///
/// Kestra answers this out of `de.focus_shift.jollyday` 2.19.0, an XML
/// holiday calendar for ~70 countries with per-subdivision rules. There is no
/// such database in this tree and no Rust crate carrying jollyday's data, so
/// egret has exactly two options: answer a different question, or say so.
///
/// It says so. This is the same call [`super::javafmt`] makes for `z`, `v`
/// and `B` — the three pattern letters whose text lives in a locale database
/// — and for the same reason: a plausible answer that is wrong on a date a
/// flow branches on is worse than a refusal an author can read. Supporting
/// one country and refusing the rest would be worse still, because an author
/// cannot predict which.
///
/// `function-ispublicholiday` stays in `oracle/baseline.txt` with this
/// reason.
fn is_public_holiday(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    // The two argument checks first, so an author with a broken call reads
    // Kestra's own message rather than the refusal.
    if args.get("date", 0).filter(|v| !is_java_null(v)).is_none() {
        return Err(pebble_error(
            "The 'isPublicHoliday()' function expects a 'date' argument.",
        ));
    }
    // **`countryCode` is only NULL-checked.** The `isBlank()` in the bytecode
    // is on `subDivision`, not here: measured,
    // `isPublicHoliday(date='2024-01-01', countryCode='  ')` answers `true` in
    // Kestra, and an earlier cut of this refused it.
    if args
        .get("countryCode", 1)
        .filter(|v| !is_java_null(v))
        .is_none()
    {
        return Err(pebble_error(
            "The 'isPublicHoliday()' function expects a 'countryCode' argument.",
        ));
    }
    Err(pebble_error(
        "egret cannot answer isPublicHoliday(): Kestra reads it out of the jollyday holiday \
         calendar (de.focus_shift.jollyday 2.19.0), a per-country, per-subdivision database \
         this port does not carry. Refusing rather than answering a date a flow may branch on.",
    ))
}

// ---------------------------------------------------------------------------
// now
// ---------------------------------------------------------------------------

/// `NowFunction` — `AbstractDate.format(ZonedDateTime.now(), args, ctx)`, so
/// it takes the same four arguments the `date` filter does and goes through
/// the same formatter.
///
/// **The default zone is UTC here and `ZoneId.systemDefault()` in Kestra**,
/// the divergence [`super::javafmt::Zone::resolve`] already records for
/// `date`. No corpus entry can hold it: an entry that reached it would not be
/// reproducible on another machine.
fn now(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date_args = DateArgs::read(&args, 0)?;
    let instant = Utc::now();
    let zone = Zone::resolve(None).map_err(pebble_error)?;
    let zdt = Zdt::from_epoch(instant.timestamp(), instant.timestamp_subsec_nanos(), zone)
        .ok_or_else(|| pebble_error("egret's dates are chrono's"))?;
    Ok(MjValue::from(date_args.render(&zdt)?))
}

// ---------------------------------------------------------------------------
// The identifier generators
// ---------------------------------------------------------------------------

/// `UUIDFunction` — `Generators.timeBasedEpochRandomGenerator().generate()`,
/// i.e. **UUIDv7**, rendered by `java.util.UUID.toString()` (lower-case hex).
///
/// A shape check sees 36 characters and four hyphens. What it cannot see, and
/// what this pins, is the layout: 48 bits of Unix milliseconds, the version
/// nibble `7`, 12 random bits, the variant bits `10`, and 62 more random
/// bits. `function-uuid-is-a-version-7-uuid` asserts exactly that through the
/// oracle, as a `regexMatch` whose answer is the deterministic `true`.
///
/// `TimeBasedEpochRandomGenerator` is the fully-random variant of v7, not the
/// counter-based one, so two UUIDs minted inside the same millisecond are not
/// ordered relative to each other on either engine.
fn uuid(_rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let millis = Utc::now().timestamp_millis().max(0) as u64 & 0x0000_ffff_ffff_ffff;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[0..6].copy_from_slice(&millis.to_be_bytes()[2..8]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(MjValue::from(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )))
}

/// The base-62 digits `friendly-id` and `ksuid` both use. Same alphabet, same
/// order, read out of both jars' `static {}` blocks.
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// `IDFunction` — `IdUtils.create()`, which is
/// `FriendlyId.createFriendlyId()`: a random **UUIDv4**, read as an unsigned
/// 128-bit integer and written in base 62 with **no padding**.
///
/// # The length is 21 or 22, and `tests/oracle.rs` used to insist on 22
///
/// `Base62.encode` loops while the value is positive and pads nothing, so the
/// digit count is `ceil(log62(value))`. `62^21` is 4.37 × 10^37 and a UUIDv4
/// spans 2^128 = 3.40 × 10^38, so **12.8 % of ids are 21 characters**, 0.2 %
/// are 20, and none is ever more than 22.
///
/// This is not a deduction: the golden file's own previous value for
/// `function-now`'s neighbour `function-id` was `iUBruItK7qj9tMOMNVPAr`, 21
/// characters, generated by Kestra itself. The shape check read
/// `value.len() == 22` and would have failed roughly one run in eight against
/// a faithful port.
fn id(_rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(MjValue::from(base62(u128::from_be_bytes(bytes), 0)))
}

/// `KSUIDFunction` — `new KsuidGenerator(new SecureRandom()).newKsuid()`.
///
/// 20 bytes: a big-endian `int` of `epochSecond - 1_400_000_000` followed by
/// 16 random ones, base-62 encoded and **left-padded to 27**
/// (`Ksuid.PAD_TO_LENGTH`). The padding is what makes the length constant,
/// unlike `id` above; and the timestamp prefix is what makes two ksuids
/// minted a second apart sort in the order they were minted, which no length
/// check can see.
fn ksuid(_rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let mut bytes = [0u8; 20];
    let stamp = (Utc::now().timestamp() - 1_400_000_000) as i32;
    bytes[0..4].copy_from_slice(&stamp.to_be_bytes());
    rand::thread_rng().fill_bytes(&mut bytes[4..20]);
    // 20 bytes do not fit in a `u128`, so the value is divided as a
    // big-endian digit string rather than as one integer.
    Ok(MjValue::from(base62_bytes(&bytes, 27)))
}

/// `NanoIDFunction`.
///
/// The default alphabet is 64 characters — read out of the class's `static {}`
/// block, `0-9A-Za-z-_` in that order — and the default length is 21. Both
/// arguments are read with `instanceof`, so a `length` that is not a `Long`
/// and an `alphabet` that is not a `String` are **silently ignored** rather
/// than refused; `function-nanoid-length-is-ignored-unless-it-is-a-long` is
/// the entry, and it answers 21 `x`s.
///
/// # Two selection paths, and the alphabet's length picks between them
///
/// A power-of-two alphabet is masked (`alphabet[byte & (len - 1)]`); anything
/// else is rejection-sampled against `256 - 256 % len`, refilling the buffer
/// as it goes. Both are ported, and both are pinned deterministically by a
/// one-character alphabet: `'x'` takes the masked path (length 1 is a power
/// of two) and `'xxx'` takes the rejection path.
fn nano_id(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);

    let mut length = 21usize;
    if let Some(value) = args.get("length", 0) {
        if value.kind() == ValueKind::Number && value.is_integer() {
            let raw = i64::try_from(value.clone()).map_err(|_| pebble_error("long overflow"))?;
            if raw < 1 {
                return Err(pebble_error(
                    "The 'nanoId()' function field 'length' must be greater than: 0",
                ));
            }
            if raw > 1000 {
                return Err(pebble_error(
                    "The 'nanoId()' function field 'length' must be lower than: 1000",
                ));
            }
            length = raw as usize;
        }
    }

    let default: Vec<char> = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_"
        .chars()
        .collect();
    let alphabet: Vec<char> = match args.get("alphabet", 1) {
        Some(value) if value.as_str().is_some() => {
            value.as_str().expect("checked").chars().collect()
        }
        _ => default,
    };
    if alphabet.is_empty() {
        return Err(pebble_error(
            "The 'nanoId()' function field 'alphabet' must not be empty",
        ));
    }
    // **`String.toCharArray().length` counts UTF-16 code units**, not
    // characters, so a 200-astral-character alphabet is 400 units and Kestra
    // refuses it. Measured; an earlier cut counted `char`s and accepted it.
    // The selection paths below index by `char`, which is the same set in a
    // different unit -- the count is the only place the two differ.
    let utf16_units: usize = alphabet.iter().map(|c| c.len_utf16()).sum();
    if utf16_units > 256 {
        return Err(pebble_error(
            "The 'nanoId()' function field 'alphabet' must not contain more than: 256 characters",
        ));
    }

    let mut rng = rand::thread_rng();
    let mut out = String::with_capacity(length);
    if alphabet.len() & (alphabet.len() - 1) == 0 {
        let mask = alphabet.len() - 1;
        let mut bytes = vec![0u8; length];
        rng.fill_bytes(&mut bytes);
        for byte in bytes {
            out.push(alphabet[usize::from(byte) & mask]);
        }
        return Ok(MjValue::from(out));
    }

    let limit = 256 - 256 % alphabet.len();
    let mut bytes = vec![0u8; length];
    let mut at = bytes.len();
    // Counted rather than re-measured: `out.chars().count()` in the condition
    // makes the loop quadratic in `length`, which at the 1000-character
    // maximum is half a million wasted steps for no reason.
    let mut written = 0usize;
    while written < length {
        if at == bytes.len() {
            rng.fill_bytes(&mut bytes);
            at = 0;
        }
        let byte = usize::from(bytes[at]);
        at += 1;
        if byte < limit {
            out.push(alphabet[byte % alphabet.len()]);
            written += 1;
        }
    }
    Ok(MjValue::from(out))
}

/// `RandomIntFunction` —
/// `(int) (Math.floor(Math.random() * (upper - lower)) + lower)`.
///
/// **`upper` is exclusive and `lower` is inclusive**, which is the thing the
/// oracle's shape check (`(1..10).contains(&n)`) states and cannot verify.
/// Three corpus entries verify it by exact value instead, because a one-wide
/// range has only one answer.
///
/// Both arguments are read with `instanceof Long`, so a numeric **string** is
/// refused rather than parsed — and all three of this function's messages
/// spell the function `randomIn()`, missing the `t`. That typo is upstream
/// and it is reproduced, because a flow author reading an egret error must
/// read the sentence Kestra would have shown them.
fn random_int(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let lower = random_int_argument(&args, "lower", 0)?;
    let upper = random_int_argument(&args, "upper", 1)?;
    if upper < lower {
        return Err(pebble_error("In 'randomIn()' upper is less than lower"));
    }
    // **`upper - lower` is Java `long` arithmetic and it WRAPS**, so
    // `randomInt(lower=Long.MIN_VALUE, upper=Long.MAX_VALUE)` has a span of
    // -1 there rather than 2^64. A plain `-` here is an arithmetic overflow:
    // a panic in a debug build and a silent wrap in a release one, from a
    // template. `wrapping_sub` is Kestra's arithmetic and `as i32` is Java's
    // `d2i`, which saturates at the `int` bounds exactly as Rust's does.
    //
    // Found by attacking the function rather than by reading it, and it
    // panicked the render thread -- which `tests/oracle.rs` treats as a
    // mismatch in every mode, but which no corpus entry reached.
    let span = upper.wrapping_sub(lower) as f64;
    let draw = rand::Rng::gen_range(&mut rand::thread_rng(), 0.0f64..1.0);
    let value = (draw * span).floor() + lower as f64;
    Ok(MjValue::from(value as i32))
}

fn random_int_argument(args: &Args, name: &str, index: usize) -> Result<i64, MjError> {
    let Some(value) = args.get(name, index) else {
        return Err(pebble_error(format!(
            "The 'randomIn()' function expects an argument {name}"
        )));
    };
    if value.kind() != ValueKind::Number || !value.is_integer() {
        return Err(pebble_error(format!(
            "The 'randomIn()' function expects an argument {name} of type Long."
        )));
    }
    i64::try_from(value.clone()).map_err(|_| pebble_error("long overflow"))
}

/// `RandomPortFunction` — `new ServerSocket(0).getLocalPort()`.
///
/// It is a real bind, and it has to be: the point of the function is a port
/// nothing else is listening on, which only the kernel can answer. The
/// listener is closed immediately (Java's try-with-resources; here, the drop
/// at the end of this function), so what comes back is a port that was free a
/// moment ago and not one that is reserved.
///
/// The range is therefore the host's ephemeral range, not `1..=65535`, and
/// neither engine avoids a well-known port because neither chooses the number
/// — `ServerSocket(0)` and `TcpListener::bind(":0")` both ask the kernel,
/// which never hands out a port in use.
fn random_port(_rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let listener =
        TcpListener::bind(("0.0.0.0", 0)).map_err(|_| pebble_error("Unable to get random port"))?;
    let port = listener
        .local_addr()
        .map_err(|_| pebble_error("Unable to get random port"))?
        .port();
    Ok(MjValue::from(i32::from(port)))
}

/// `Base62.encode(BigInteger)`: divide-and-remainder down to zero, most
/// significant digit first, left-padded to `pad` with `0`.
///
/// `friendly-id`'s encoder passes `pad = 0` (no padding at all) and `ksuid`'s
/// passes 27. That single argument is the whole difference between `id`'s
/// variable length and `ksuid`'s constant one.
fn base62(mut value: u128, pad: usize) -> String {
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(BASE62[(value % 62) as usize]);
        value /= 62;
    }
    if digits.is_empty() {
        digits.push(BASE62[0]);
    }
    while digits.len() < pad {
        digits.push(BASE62[0]);
    }
    digits.reverse();
    String::from_utf8(digits).expect("base62 digits are ASCII")
}

/// The same encoding over a byte string too wide for a `u128` — `ksuid`'s 20
/// bytes. Long division over base-256 digits, which is what
/// `BigInteger.divideAndRemainder` does.
fn base62_bytes(bytes: &[u8], pad: usize) -> String {
    let mut value: Vec<u8> = bytes.to_vec();
    let mut digits = Vec::new();
    while value.iter().any(|b| *b != 0) {
        let mut remainder = 0u32;
        for byte in value.iter_mut() {
            let current = (remainder << 8) | u32::from(*byte);
            *byte = (current / 62) as u8;
            remainder = current % 62;
        }
        digits.push(BASE62[remainder as usize]);
    }
    if digits.is_empty() {
        digits.push(BASE62[0]);
    }
    while digits.len() < pad {
        digits.push(BASE62[0]);
    }
    digits.reverse();
    String::from_utf8(digits).expect("base62 digits are ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::env::{render_str, RenderContext};

    fn render(template: &str, context: &str) -> Result<String, String> {
        let cx = RenderContext::from_json_str(context).expect("context must parse");
        render_str(template, &cx).map_err(|e| e.to_string())
    }

    fn ok(template: &str) -> String {
        render(template, "{}").expect("must render")
    }

    fn err(template: &str) -> String {
        render(template, "{}").expect_err("must fail")
    }

    // -----------------------------------------------------------------
    // The `compare: "error-class"` entries.
    //
    // The oracle compares these by error CLASS, which any failure
    // satisfies -- including `unknown function`, i.e. the function not
    // being registered at all. These assert egret's own message instead,
    // so the corpus entry is not what carries them. See
    // `oracle/error-class.txt` and `oracle/error-class-pairs.txt`.
    // -----------------------------------------------------------------

    #[test]
    fn range_refuses_an_increment_that_cannot_advance_it() {
        assert!(
            err("{{ range(start=1, end=3, increment=0) }}")
                .contains("The increment of the range function must be different than 0"),
            "{}",
            err("{{ range(start=1, end=3, increment=0) }}")
        );
        let message = err("{{ range(start=1, end=3, increment='x') }}");
        // **The space is Pebble's own constant**, proven by the golden
        // (`must be a number x`, not `numberx`). This assertion used to pin
        // egret's own missing space, which is exactly what an error-class
        // corpus entry cannot see.
        assert!(
            message.contains("The increment of the range function must be a number x"),
            "{message}"
        );
        // The 32-bit truncation Pebble's own character loop performs: an
        // increment whose low 32 bits are zero never moves the loop variable,
        // and Kestra hangs there rather than answering.
        let message = err("{{ range(start='a', end='z', increment=4294967296) }}");
        assert!(message.contains("never advances the range"), "{message}");
    }

    #[test]
    fn range_refuses_an_argument_that_is_neither_a_number_nor_one_character() {
        for template in [
            "{{ range(start='ab', end='e') }}",
            "{{ range(start=1, end='e') }}",
        ] {
            let message = err(template);
            assert!(
                message.contains(
                    "Arguments of range function must be of type Number or String with a \
                     length of 1"
                ),
                "{template}: {message}"
            );
        }
    }

    #[test]
    fn range_carries_kestras_own_million_element_cap() {
        let message = err("{{ range(start=0, end=1000000) }}");
        assert!(
            message.contains(
                "The range function cannot produce more than 1000000 elements, but this call \
                 would produce 1000001."
            ),
            "{message}"
        );
        // The count is `|end - start| / |increment| + 1`, so a two-million-wide
        // span with a two-million step is TWO elements and is allowed. A port
        // that bounded `|end - start|` would refuse it.
        assert_eq!(
            ok("{{ range(start=0, end=2000000, increment=2000000) }}"),
            "[0,2000000]"
        );
    }

    #[test]
    fn from_json_names_the_argument_it_wanted() {
        assert!(
            err("{{ fromJson() }}").contains("The 'fromJson' function expects an argument 'json'."),
            "{}",
            err("{{ fromJson() }}")
        );
        assert!(
            err("{{ fromJson(json=42) }}")
                .contains("The 'fromJson' function expects an argument 'json' with type string."),
            "{}",
            err("{{ fromJson(json=42) }}")
        );
        assert!(
            err("{{ fromJson(json='{a}') }}").contains("Invalid json"),
            "{}",
            err("{{ fromJson(json='{a}') }}")
        );
    }

    #[test]
    fn from_ion_names_the_argument_it_wanted() {
        assert!(
            err("{{ fromIon() }}").contains("The 'fromIon' function expects an argument 'ion'."),
            "{}",
            err("{{ fromIon() }}")
        );
        assert!(
            err("{{ fromIon(ion=42) }}").contains(
                "The 'fromIon' function expects an argument 'ion' with type string or byte[]."
            ),
            "{}",
            err("{{ fromIon(ion=42) }}")
        );
        assert!(
            err("{{ fromIon(ion='{a:') }}").contains("Invalid ion"),
            "{}",
            err("{{ fromIon(ion='{a:') }}")
        );
    }

    /// Upstream defect 6's last row, both halves.
    #[test]
    fn from_ion_refuses_the_text_its_own_writer_produces_for_nothing() {
        for template in [
            "{{ fromIon(ion='null') }}",
            "{{ fromIon(ion=(nothing | toIon)) }}",
        ] {
            let message = render(template, r#"{"nothing": null}"#).expect_err("must fail");
            assert!(
                message.contains("Invalid ion: t is null in sink.next(t)"),
                "{template}: {message}"
            );
        }
        // And the writer really does produce it, which is what makes the two
        // halves one defect rather than two facts.
        assert_eq!(
            render("{{ nothing | toIon }}", r#"{"nothing": null}"#).unwrap(),
            "null"
        );
    }

    /// `fromJson(json=null)` returns null; `yaml(yaml=null)` throws. Two
    /// functions in the same batch, two answers to the same input.
    #[test]
    fn yaml_has_no_null_branch_where_from_json_does() {
        assert_eq!(
            render("<{{ fromJson(json=nothing) }}>", r#"{"nothing": null}"#).unwrap(),
            "<>"
        );
        let message =
            render("{{ yaml(yaml=nothing) }}", r#"{"nothing": null}"#).expect_err("must fail");
        assert!(
            message.contains("The 'yaml' function expects an argument 'yaml' with type string."),
            "{message}"
        );
        assert!(
            err("{{ yaml() }}").contains("The 'yaml' function expects an argument 'yaml'."),
            "{}",
            err("{{ yaml() }}")
        );
        assert!(
            err("{{ yaml(yaml=42) }}")
                .contains("The 'yaml' function expects an argument 'yaml' with type string."),
            "{}",
            err("{{ yaml(yaml=42) }}")
        );
        assert!(
            err("{{ yaml(yaml='a: [') }}").contains("Invalid yaml"),
            "{}",
            err("{{ yaml(yaml='a: [') }}")
        );
    }

    #[test]
    fn yaml_refuses_the_infinity_its_own_filter_writes() {
        // The filter writes it unquoted -- that is upstream's
        // `MINIMIZE_QUOTES` with `ALWAYS_QUOTE_NUMBERS_AS_STRINGS` off.
        assert_eq!(ok("{{ '.inf' | yaml }}"), ".inf\n");
        let message = err("{{ yaml(yaml=('.inf' | yaml)) }}");
        assert!(
            message.contains("Malformed numeric value '.inf'"),
            "{message}"
        );
        assert!(
            err("{{ yaml(yaml='a: .nan') }}").contains("Malformed numeric value '.nan'"),
            "{}",
            err("{{ yaml(yaml='a: .nan') }}")
        );
    }

    #[test]
    fn a_calendar_function_names_itself_when_the_date_is_missing() {
        for (template, name) in [
            ("{{ dayOfWeek() }}", "dayOfWeek"),
            ("{{ dayOfMonth() }}", "dayOfMonth"),
            ("{{ monthOfYear() }}", "monthOfYear"),
            ("{{ isWeekend() }}", "isWeekend"),
            ("{{ hourOfDay() }}", "hourOfDay"),
            ("{{ isLastWorkingDay() }}", "isLastWorkingDay"),
            ("{{ isDayWeekInMonth() }}", "isDayWeekInMonth"),
            ("{{ isPublicHoliday() }}", "isPublicHoliday"),
        ] {
            let message = err(template);
            assert!(
                message.contains(&format!(
                    "The '{name}()' function expects a 'date' argument."
                )),
                "{template}: {message}"
            );
        }
        // A `date` that is present but null takes the same branch: Kestra's
        // guard is `date == null`, not `containsKey`.
        let message =
            render("{{ dayOfWeek(date=nothing) }}", r#"{"nothing": null}"#).expect_err("must fail");
        assert!(
            message.contains("The 'dayOfWeek()' function expects a 'date' argument."),
            "{message}"
        );
    }

    #[test]
    fn a_calendar_function_names_itself_when_the_date_does_not_parse() {
        let message = err("{{ dayOfWeek(date='not-a-date') }}");
        assert!(
            message.contains("The 'dayOfWeek()' function could not parse 'date':"),
            "{message}"
        );
    }

    /// `hourOfDay` is `ZonedDateTime.parse` / `LocalDateTime.parse`, never
    /// `LocalDate.parse`, so it is the only one of the seven that refuses a
    /// bare date.
    #[test]
    fn hour_of_day_refuses_a_date_with_no_time_where_its_siblings_take_one() {
        let message = err("{{ hourOfDay(date='2024-01-06') }}");
        assert!(
            message.contains("The 'hourOfDay()' function could not parse 'date':"),
            "{message}"
        );
        assert_eq!(ok("{{ dayOfWeek(date='2024-01-06') }}"), "SATURDAY");
        assert_eq!(ok("{{ isWeekend(date='2024-01-06') }}"), "true");
    }

    #[test]
    fn is_last_working_day_names_the_day_it_could_not_read() {
        let message = err("{{ isLastWorkingDay(date='2024-01-31', workingDays='FUNDAY') }}");
        assert!(
            message.contains(
                "The 'isLastWorkingDay()' function received an invalid 'workingDays' value: \
                 'FUNDAY'. Expected day names like MONDAY, TUESDAY, etc."
            ),
            "{message}"
        );
    }

    #[test]
    fn is_day_week_in_month_lists_what_it_would_have_accepted() {
        let message =
            err("{{ isDayWeekInMonth(date='2024-01-01', dayOfWeek='MONDAY', position='FIFTH') }}");
        assert!(
            message.contains(
                "received an invalid 'position' value: 'FIFTH'. Expected one of: FIRST, SECOND, \
                 THIRD, FOURTH, LAST."
            ),
            "{message}"
        );
        let message =
            err("{{ isDayWeekInMonth(date='2024-01-01', dayOfWeek='FUNDAY', position='FIRST') }}");
        assert!(
            message.contains(
                "received an invalid 'dayOfWeek' value: 'FUNDAY'. Expected one of: \
                 MONDAY, TUESDAY, WEDNESDAY, THURSDAY, FRIDAY, SATURDAY, SUNDAY."
            ),
            "{message}"
        );
        let message = err("{{ isDayWeekInMonth(date='2024-01-01', dayOfWeek='MONDAY') }}");
        assert!(
            message.contains("The 'isDayWeekInMonth()' function expects a 'position' argument."),
            "{message}"
        );
    }

    /// All three of `randomInt`'s messages spell the function `randomIn()`.
    /// The typo is upstream and it is reproduced, because a flow author
    /// reading an egret error must read the sentence Kestra would have shown.
    #[test]
    fn random_int_reproduces_upstreams_missing_letter() {
        let message = err("{{ randomInt(lower=5, upper=4) }}");
        assert!(
            message.contains("In 'randomIn()' upper is less than lower"),
            "{message}"
        );
        let message = err("{{ randomInt() }}");
        assert!(
            message.contains("The 'randomIn()' function expects an argument lower"),
            "{message}"
        );
        let message = err("{{ randomInt(lower='1', upper=10) }}");
        assert!(
            message.contains("The 'randomIn()' function expects an argument lower of type Long."),
            "{message}"
        );
        // And the typo is the only spelling: nothing here says `randomInt()`.
        assert!(!err("{{ randomInt() }}").contains("randomInt()"));
    }

    #[test]
    fn nano_id_refuses_a_length_or_an_alphabet_it_cannot_use() {
        assert!(
            err("{{ nanoId(alphabet='') }}")
                .contains("The 'nanoId()' function field 'alphabet' must not be empty"),
            "{}",
            err("{{ nanoId(alphabet='') }}")
        );
        assert!(
            err("{{ nanoId(length=0) }}")
                .contains("The 'nanoId()' function field 'length' must be greater than: 0"),
            "{}",
            err("{{ nanoId(length=0) }}")
        );
        assert!(
            err("{{ nanoId(length=1001) }}")
                .contains("The 'nanoId()' function field 'length' must be lower than: 1000"),
            "{}",
            err("{{ nanoId(length=1001) }}")
        );

        // **Both bounds are `>`, not `>=`.** A mutation run found `>= 1000`
        // and `>= 256` surviving, because every case sat one side of the
        // boundary. These sit ON it, and `function-nanoid-length-at-the-max`
        // / `-alphabet-at-the-max` do the same through the oracle.
        assert_eq!(
            ok("{{ nanoId(length=1000, alphabet='x') }}"),
            "x".repeat(1000)
        );
        let widest: String = (0..256u32)
            .map(|c| char::from_u32(c + 0x100).expect("a valid char"))
            .collect();
        let at_max = render(
            "{{ nanoId(length=4, alphabet=a) }}",
            &format!(r#"{{"a": {}}}"#, serde_json::to_string(&widest).unwrap()),
        )
        .expect("a 256-character alphabet is allowed");
        assert_eq!(at_max.chars().count(), 4, "{at_max}");
        let message = render(
            "{{ nanoId(length=4, alphabet=a) }}",
            &format!(
                r#"{{"a": {}}}"#,
                serde_json::to_string(&format!("{widest}x")).unwrap()
            ),
        )
        .expect_err("257 is past the bound");
        // And the count is UTF-16 code units, not characters: 200 astral
        // characters is 400 units and Kestra refuses it. Measured; an earlier
        // cut counted `char`s and accepted it. 128 astral characters is
        // exactly 256 units and is accepted.
        let astral: String = (0..200u32)
            .map(|i| char::from_u32(0x10000 + i).expect("a valid char"))
            .collect();
        let astral_message = render(
            "{{ nanoId(length=4, alphabet=a) }}",
            &format!(r#"{{"a": {}}}"#, serde_json::to_string(&astral).unwrap()),
        )
        .expect_err("400 UTF-16 units is past the bound");
        assert!(
            astral_message.contains("must not contain more than: 256 characters"),
            "{astral_message}"
        );
        let at_max: String = astral.chars().take(128).collect();
        assert_eq!(
            render(
                "{{ nanoId(length=4, alphabet=a) }}",
                &format!(r#"{{"a": {}}}"#, serde_json::to_string(&at_max).unwrap()),
            )
            .expect("256 units is allowed")
            .chars()
            .count(),
            4
        );
        assert!(
            message.contains(
                "The 'nanoId()' function field 'alphabet' must not contain more than: 256 \
                 characters"
            ),
            "{message}"
        );
    }

    /// Ion's own grammar refuses these; the first cut of this reader did not,
    /// and answered `7`, `5`, `$0` and `[1,2]`. Over-acceptance is the one
    /// kind of divergence this crate's rules do not allow — a refusal that
    /// says so is fine, a plausible wrong answer is not.
    ///
    /// **Every source here is the bad token ON ITS OWN, and that is the whole
    /// point of the test above it.** The first cut of these entries and this
    /// test used only whole-document sources, which passed the correct
    /// implementation *and* the one that applied every refusal to the whole
    /// document — see
    /// [`from_ion_reads_one_value_and_never_looks_at_the_tail`], which is the
    /// variant they were all one step away from.
    #[test]
    fn the_ion_reader_refuses_what_ion_itself_forbids() {
        for (source, why) in [
            ("007", "leading zero"),
            ("00", "leading zero"),
            ("-007", "leading zero"),
            ("+5", "leading '+'"),
            ("+1.5", "leading '+'"),
            ("$0", "symbol ID"),
            ("$12", "symbol ID"),
            ("[1,,2]", "unexpected character"),
            ("{,}", "an ion field name"),
            ("$ion_1_01", "unsupported ion version marker"),
            ("$ion_1_1 1", "unsupported ion version marker"),
        ] {
            let message = render(
                "{{ fromIon(ion=i) }}",
                &format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap()),
            )
            .expect_err("must be refused");
            assert!(message.contains(why), "{source}: {message}");
        }
        // And the shapes Ion DOES allow, which the refusals must not catch.
        assert_eq!(
            ok("{{ fromIon(ion='[0x10, 1_0, 0b11, -0]') }}"),
            "[16,10,3,0]"
        );
        assert_eq!(ok("{{ fromIon(ion='0.5') }}"), "0.5");
        assert_eq!(ok("{{ fromIon(ion='+inf') }}"), "Infinity");
        assert_eq!(ok("{{ fromIon(ion='$ion_1_0 1') }}"), "1");
        // Both i64 ends, which an `i64`-only magnitude parse refused.
        assert_eq!(
            ok("{{ fromIon(ion='-9223372036854775808') }}"),
            "-9223372036854775808"
        );
        assert_eq!(
            ok("{{ fromIon(ion='12345678901234567890') }}"),
            "12345678901234567890"
        );
    }

    /// **`FileSerde.readAll(…).findFirst()` is `take(1)`: Kestra never parses
    /// past the first value, and `allRows=true` (`toList()`) parses all of
    /// them.** Measured both ways over fourteen documents.
    ///
    /// This reader parsed the whole document eagerly and then took `rows[0]`,
    /// which was harmless while it accepted nearly everything and became
    /// thirteen regressions the moment fix round 1 gave it six refusals: a
    /// valid value followed by a bad token answered the bad token's error.
    ///
    /// Every entry written for those refusals was a document consisting only
    /// of the bad token, so each passed the correct implementation and the
    /// broken one alike. **The variant they were all one step away from is
    /// simply a valid value in front**, and that is what this asserts.
    #[test]
    fn from_ion_reads_one_value_and_never_looks_at_the_tail() {
        for (source, expected) in [
            ("1 /*c", "1"),
            ("1 +5", "1"),
            ("1 a::1", "1"),
            ("1 null", "1"),
            ("1 007", "1"),
            ("1 $0", "1"),
            ("1 [1,,2]", "1"),
            ("1 2 007", "1"),
            ("1 (", "1"),
            ("1 abc::", "1"),
            ("[1,2] /*c", "[1,2]"),
            ("{a:1} /*c", r#"{"a":1}"#),
        ] {
            assert_eq!(
                render(
                    "{{ fromIon(ion=i) }}",
                    &format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap()),
                )
                .unwrap_or_else(|e| panic!("{source}: {e}")),
                expected,
                "{source}"
            );
        }

        // A bad FIRST value is still an error -- laziness stops at one value,
        // it does not skip one.
        for source in ["null 1", "007 1", "+5 1"] {
            render(
                "{{ fromIon(ion=i) }}",
                &format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap()),
            )
            .expect_err("a bad FIRST value is still an error");
        }

        // `allRows=true` is `toList()`, which DOES read the tail.
        for source in ["1 /*c", "1 +5", "1 007", "1 [1,,2]"] {
            render(
                "{{ fromIon(ion=i, allRows=true) }}",
                &format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap()),
            )
            .expect_err("allRows reads the tail");
        }
        assert_eq!(
            render("{{ fromIon(ion=i, allRows=true) }}", r#"{"i": "1 2 3"}"#).unwrap(),
            "[1,2,3]"
        );
    }

    /// A `/*` with no `*/` mapped to end-of-input, so a **truncated** document
    /// rendered as nothing. Kestra says `Corrupt content to decode`.
    ///
    /// **The distinction the first version of this test named and did not
    /// draw**: it probed only `/* nope`, so it passed the implementation that
    /// applied the refusal to a document whose first value was already
    /// complete. `1 /*c` is `1`, because nothing ever reads that far.
    #[test]
    fn an_unterminated_ion_comment_is_corrupt_not_an_empty_document() {
        let message = render("{{ fromIon(ion=i) }}", r#"{"i": "/* nope"}"#)
            .expect_err("a truncated document must not render as nothing");
        assert!(
            message.contains("unterminated ion block comment"),
            "{message}"
        );
        // The same document with a complete value in front of it is that
        // value, on both engines.
        assert_eq!(
            render("{{ fromIon(ion=i) }}", r#"{"i": "1 /*c"}"#).unwrap(),
            "1"
        );
        assert_eq!(
            render("{{ fromIon(ion=i) }}", r#"{"i": "/* c */ 1 /*c"}"#).unwrap(),
            "1"
        );

        assert_eq!(ok("{{ fromIon(ion='1//c') }}"), "1");
        assert_eq!(ok("{{ fromIon(ion='1 // c') }}"), "1");
        assert_eq!(ok("{{ fromIon(ion='/* c */ 1') }}"), "1");
        assert_eq!(
            render("<{{ fromIon(ion=i) }}>", r#"{"i": "// c"}"#).unwrap(),
            "<>"
        );
        assert_eq!(ok("<{{ fromIon(ion='') }}>"), "<>");
    }

    /// **The second unmeasured claim about annotations in two rounds.** Round
    /// 1 corrected "Jackson drops them" to "Kestra throws for one inside a
    /// container" — and that is false too: `[a::1, 2] | length` is `2`,
    /// `[a::1, 2][1]` is `2`, and `{f:a::1} | className` is
    /// `java.util.LinkedHashMap`. Only *stringifying* a container that holds
    /// one fails, as a Jackson serialiser artifact whose message is not even
    /// stable between JVM runs.
    ///
    /// The lesson is not about annotations: **the correction inherited the
    /// method that produced the error.** Rewriting a claim that was wrong for
    /// lack of measurement is the moment to measure. Fifteen forms were probed
    /// for this one.
    #[test]
    fn an_annotated_value_is_its_own_text_and_its_container_is_a_container() {
        // The value carries its own source text, which is what Kestra renders.
        assert_eq!(ok("{{ fromIon(ion='a::1') }}"), "a::1");
        assert_eq!(ok("{{ fromIon(ion='a::b::1') }}"), "a::b::1");
        assert_eq!(ok("{{ fromIon(ion='a::{b:1}') }}"), "a::{b:1}");
        assert_eq!(ok("{{ fromIon(ion='a::1') | string }}"), "a::1");

        // And the CONTAINER is an ordinary container. These three were the
        // wrong answers the refusal produced.
        assert_eq!(ok("{{ fromIon(ion='[a::1, 2]') | length }}"), "2");
        assert_eq!(ok("{{ fromIon(ion='[a::1, 2]')[1] }}"), "2");
        assert_eq!(
            ok("{{ fromIon(ion='{f:a::1}') | className }}"),
            "java.util.LinkedHashMap"
        );
        assert_eq!(ok("{{ fromIon(ion='{f:a::1}').f }}"), "a::1");
        assert_eq!(ok("{{ fromIon(ion='[[a::1]]')[0][0] }}"), "a::1");
        assert_eq!(
            ok("{{ fromIon(ion='{f:a::1,g:2}') | keys }}"),
            r#"["f","g"]"#
        );

        // **The one thing that cannot be a corpus entry.** Stringifying a
        // container that HOLDS an annotated value throws in Kestra, and the
        // message is not stable between JVM runs -- on the same jar and the
        // same corpus, three fresh runs gave `Document nesting depth (1001)
        // exceeds the maximum` once and `Direct self-reference leading to
        // cycle` twice. A golden that changes on regeneration is worse than
        // none, so this is the pin instead, and `oracle/README.md` says why.
        assert_eq!(ok("{{ fromIon(ion='[a::1, 2]') }}"), r#"["a::1",2]"#);
        assert_eq!(ok("{{ fromIon(ion='{f:a::1}') }}"), r#"{"f":"a::1"}"#);
        assert_eq!(ok("{{ fromIon(ion='a::[1]') }}"), "a::[1]");

        // A malformed annotation is still an error: the decorated value is
        // parsed, which is what says where the text ends.
        assert!(render("{{ fromIon(ion=i) }}", r#"{"i": "abc::"}"#).is_err());
        // A bare symbol is not an annotation.
        assert_eq!(ok("{{ fromIon(ion='abc') }}"), "abc");
        assert_eq!(ok("{{ fromIon(ion='{a:b}') }}"), r#"{"a":"b"}"#);
    }

    /// **Ion's `d` exponent and its bare `.` make a `java.math.BigDecimal`;
    /// its `e` exponent makes a `java.lang.Double`.** Measured across fourteen
    /// literals, four ways each.
    ///
    /// An `f64` for the first family loses the scale, and the sentence that
    /// excused it said "no round trip through this crate reaches it — only
    /// hand-written Ion does". `{{ fromIon('1d2') | toIon }}` is a round trip
    /// through this crate that reaches it: `1d2` in Kestra, `100e0` here.
    #[test]
    fn an_ion_decimal_is_a_big_decimal_and_an_ion_float_is_a_double() {
        for (source, plain, class, ion) in [
            ("1d2", "100", "java.math.BigDecimal", "1d2"),
            (
                "1d20",
                "100000000000000000000",
                "java.math.BigDecimal",
                "1d20",
            ),
            ("1d-2", "0.01", "java.math.BigDecimal", "0.01"),
            ("1.50", "1.50", "java.math.BigDecimal", "1.50"),
            ("-1.50", "-1.50", "java.math.BigDecimal", "-1.50"),
            ("100d-2", "1.00", "java.math.BigDecimal", "1.00"),
            ("0.001", "0.001", "java.math.BigDecimal", "0.001"),
            ("1.", "1", "java.math.BigDecimal", "1."),
            ("0.", "0", "java.math.BigDecimal", "0."),
            ("1d0", "1", "java.math.BigDecimal", "1."),
            // `BigDecimal` has no negative zero.
            ("-0.0", "0.0", "java.math.BigDecimal", "0.0"),
            (
                "12345678901234567890.5",
                "12345678901234567890.5",
                "java.math.BigDecimal",
                "12345678901234567890.5",
            ),
            // An `e` exponent is a FLOAT, and stays one.
            ("1E2", "100.0", "java.lang.Double", "100e0"),
            ("1e0", "1.0", "java.lang.Double", "1e0"),
        ] {
            let cx = format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap());
            assert_eq!(
                render("{{ fromIon(ion=i) }}", &cx).unwrap(),
                plain,
                "{source}"
            );
            assert_eq!(
                render("{{ fromIon(ion=i) | className }}", &cx).unwrap(),
                class,
                "{source}"
            );
            assert_eq!(
                render("{{ fromIon(ion=i) | toIon }}", &cx).unwrap(),
                ion,
                "{source} through toIon"
            );
        }
        assert_eq!(ok("{{ fromIon(ion='1d2') | toJson }}"), "1E+2");
        // The pre-existing writer bug the decimal work exposed: a scale-0
        // `BigDecimal` needs Ion's `.`, or it reads back as an integer.
        assert_eq!(ok("{{ '1' | number(type='BIGDECIMAL') | toIon }}"), "1.");
        assert_eq!(
            ok("{{ '1.50' | number(type='BIGDECIMAL') | toIon }}"),
            "1.50"
        );
    }

    /// `serde_norway` deserialises a source with no content to a **unit**,
    /// which renders as nothing; Jackson throws. A wrong value that renders
    /// successfully is the failure mode this whole package exists to prevent,
    /// so the question is asked of the source, before the parser.
    ///
    /// The shapes that are NOT empty are asserted beside it, because a check
    /// that answered "empty" for `~` would be a new bug of the same kind.
    #[test]
    fn an_empty_yaml_document_is_an_error_not_a_null() {
        for source in ["", "   ", "# c", "\n\n", "# a\n# b\n"] {
            let message = render(
                "{{ yaml(yaml=y) }}",
                &format!(r#"{{"y": {}}}"#, serde_json::to_string(source).unwrap()),
            )
            .expect_err("must be refused");
            assert!(
                message.contains("No content to map due to end-of-input"),
                "{source:?}: {message}"
            );
        }
        assert_eq!(ok("<{{ yaml(yaml='~') }}>"), "<>");
        assert_eq!(ok("{{ yaml(yaml='a: null') }}"), "{}");
        assert_eq!(ok("{{ yaml(yaml='0') }}"), "0");
        // The FIRST of several documents, rather than a refusal.
        assert_eq!(
            render("{{ yaml(yaml=y) }}", r#"{"y": "a: 1\n---\nb: 2"}"#).unwrap(),
            r#"{"a":1}"#
        );
    }

    /// `AbstractDate.formatter` falls through both name maps to
    /// `DateTimeFormatter.ofPattern(name)`, so an unknown format NAME is a
    /// pattern-compile error rather than an unknown-format one.
    #[test]
    fn now_compiles_an_unknown_format_name_as_a_pattern() {
        let message = err("{{ now(format='nope_format') }}");
        assert!(message.contains("Unknown pattern letter: o"), "{message}");
    }

    // -----------------------------------------------------------------
    // What the oracle's SHAPE check cannot see.
    // -----------------------------------------------------------------

    /// `Base62.encode` pads nothing, so `id()` is 21 or 22 characters and
    /// occasionally 20 -- which is why `tests/oracle.rs`'s shape check is not
    /// `== 22`. Ten thousand draws is enough to see the 12.8% case
    /// (P(all 22) is about 10^-596).
    #[test]
    fn an_id_is_base62_and_is_not_always_22_characters() {
        let mut lengths = std::collections::BTreeSet::new();
        for _ in 0..10_000 {
            let value = ok("{{ id() }}");
            assert!(
                value.bytes().all(|b| b.is_ascii_alphanumeric()),
                "{value} is not base62"
            );
            assert!(value.len() <= 22, "{value} is longer than 22");
            lengths.insert(value.len());
        }
        assert!(
            lengths.contains(&21),
            "10 000 draws produced no 21-character id, so the encoder is padding: {lengths:?}"
        );
        assert!(lengths.contains(&22), "{lengths:?}");
    }

    /// `Ksuid.PAD_TO_LENGTH` is 27, and the first four bytes are
    /// `epochSecond - 1_400_000_000`, big-endian.
    ///
    /// **The prefix was pinned by nothing.** The first version of this test
    /// checked the length, the alphabet, and then `base62_bytes` on twenty
    /// ZERO bytes — which exercises the padding helper and never looks at a
    /// real ksuid at all. Replacing the four timestamp bytes with random ones
    /// left every test in the crate and the whole oracle green; verified by
    /// running that mutation. So this decodes the value back and reads the
    /// field: a random `u32` lands inside the window below with probability
    /// about 2 in 10^9.
    #[test]
    fn a_ksuid_is_27_padded_base62_characters_with_a_time_prefix() {
        for _ in 0..200 {
            assert_eq!(ok("{{ ksuid() }}").len(), 27);
        }
        let value = ok("{{ ksuid() }}");
        assert!(value.bytes().all(|b| b.is_ascii_alphanumeric()), "{value}");

        let bytes = base62_decode(&value);
        let stamp = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let expected = (Utc::now().timestamp() - 1_400_000_000) as u32;
        assert!(
            stamp.abs_diff(expected) < 5,
            "the four-byte prefix must be the ksuid epoch, not random: {stamp} vs {expected}"
        );

        // And it is monotonic across a second, which is what "sorts in mint
        // order" means. Compared as strings: base 62 over a big-endian value
        // at a fixed width is order-preserving.
        let earlier = ok("{{ ksuid() }}");
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let later = ok("{{ ksuid() }}");
        assert!(earlier < later, "{earlier} then {later}");

        // The encoder's own round trip, which is what makes the decode above
        // evidence rather than a second guess.
        assert_eq!(base62_decode(&base62_bytes(&[0u8; 20], 27)), [0u8; 20]);
        let mut probe = [0u8; 20];
        probe[0] = 0x12;
        probe[3] = 0x34;
        probe[19] = 0xff;
        assert_eq!(base62_decode(&base62_bytes(&probe, 27)), probe);
    }

    /// `Base62.encode`'s inverse, for the test above: 27 digits back into the
    /// twenty big-endian bytes they came from, by `buf = buf * 62 + digit`.
    fn base62_decode(text: &str) -> [u8; 20] {
        let mut bytes = [0u8; 20];
        for c in text.chars() {
            let digit = BASE62
                .iter()
                .position(|d| *d as char == c)
                .expect("a base62 digit") as u32;
            let mut carry = digit;
            for byte in bytes.iter_mut().rev() {
                let value = u32::from(*byte) * 62 + carry;
                *byte = (value & 0xff) as u8;
                carry = value >> 8;
            }
            assert_eq!(carry, 0, "a 27-digit ksuid must fit in twenty bytes");
        }
        bytes
    }

    /// A `TimeBasedEpochRandomGenerator` UUID: version nibble `7`, variant
    /// bits `10`, lower-case hex, and 48 bits of Unix milliseconds in front.
    #[test]
    fn a_uuid_is_a_version_7_uuid_with_a_millisecond_prefix() {
        let value = ok("{{ uuid() }}");
        assert_eq!(value.len(), 36, "{value}");
        let parts: Vec<&str> = value.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(
            value.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
            "{value}"
        );
        assert!(
            value.bytes().all(|b| !b.is_ascii_uppercase()),
            "java.util.UUID.toString is lower case: {value}"
        );
        assert!(parts[2].starts_with('7'), "version nibble: {value}");
        assert!(
            matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
            "variant bits: {value}"
        );
        // The 48-bit prefix is Unix milliseconds, so it tracks the clock.
        let millis = u64::from_str_radix(&format!("{}{}", parts[0], parts[1]), 16).expect("hex");
        let now = Utc::now().timestamp_millis() as u64;
        assert!(millis.abs_diff(now) < 60_000, "{millis} vs {now}");
    }

    /// The default alphabet is 64 characters read out of `NanoIDFunction`'s
    /// `static {}` block, and it includes `-` and `_` -- which a
    /// `[0-9A-Za-z]` shape check would pass on 21 characters and still be
    /// wrong about.
    #[test]
    fn nano_id_uses_kestras_own_64_character_alphabet_and_both_selection_paths() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            let value = ok("{{ nanoId() }}");
            assert_eq!(value.chars().count(), 21, "{value}");
            seen.extend(value.chars());
        }
        let alphabet: std::collections::BTreeSet<char> =
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_"
                .chars()
                .collect();
        assert_eq!(seen, alphabet, "the alphabet is not Kestra's");

        // A one-character alphabet is deterministic and picks each path:
        // length 1 is a power of two (masked), length 3 is not (rejection
        // sampled). Both must terminate and both must answer the same.
        assert_eq!(ok("{{ nanoId(length=6, alphabet='x') }}"), "xxxxxx");
        assert_eq!(ok("{{ nanoId(length=6, alphabet='xxx') }}"), "xxxxxx");
        // A non-Long length and a non-String alphabet are IGNORED, not
        // refused -- Kestra reads both with `instanceof`.
        assert_eq!(ok("{{ nanoId(length='6', alphabet='x') }}"), "x".repeat(21));

        // **The rejection path has to be the rejection path.** A one-character
        // alphabet cannot tell the two apart -- every answer is `x` either way
        // -- and a mutation run found "always mask" surviving because of it.
        // With a three-character alphabet the mask is `byte & 2`, which yields
        // 0 and 2 and NEVER 1, so the middle letter disappears.
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..200 {
            seen.extend(ok("{{ nanoId(length=21, alphabet='abc') }}").chars());
        }
        assert_eq!(
            seen,
            "abc".chars().collect::<std::collections::BTreeSet<char>>(),
            "a non-power-of-two alphabet must be rejection-sampled, not masked"
        );
    }

    /// The two arguments whose subtraction overflows an `i64`. Kestra's
    /// `upper - lower` is Java `long` arithmetic, which wraps; a plain Rust
    /// `-` panics in debug and wraps silently in release. A panic in the
    /// renderer is a mismatch in every oracle mode, and no corpus entry
    /// reaches this -- it was found by attacking the function.
    #[test]
    fn random_int_does_not_panic_on_a_span_wider_than_an_i64() {
        for template in [
            "{{ randomInt(lower=-9223372036854775808, upper=9223372036854775807) }}",
            "{{ randomInt(lower=-9223372036854775808, upper=0) }}",
            "{{ randomInt(lower=0, upper=9223372036854775807) }}",
        ] {
            let value = ok(template);
            assert!(
                value.parse::<i32>().is_ok(),
                "{template} must answer an int, not panic: {value}"
            );
        }
    }

    /// `floor(random() * (upper - lower)) + lower`: lower-inclusive,
    /// upper-EXCLUSIVE. `tests/oracle.rs`'s shape check states the range and
    /// cannot verify either end; this does.
    #[test]
    fn random_int_is_lower_inclusive_and_upper_exclusive() {
        for _ in 0..1_000 {
            assert_eq!(ok("{{ randomInt(lower=5, upper=6) }}"), "5");
            assert_eq!(ok("{{ randomInt(lower=5, upper=5) }}"), "5");
            assert_eq!(ok("{{ randomInt(lower=-3, upper=-2) }}"), "-3");
        }
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            seen.insert(ok("{{ randomInt(lower=1, upper=10) }}"));
        }
        assert!(seen.contains("1"), "lower is inclusive: {seen:?}");
        assert!(!seen.contains("10"), "upper is exclusive: {seen:?}");
    }

    /// `new ServerSocket(0)` asks the KERNEL, and the kernel never hands out
    /// a port that is currently bound. That is the whole property, and it is
    /// the one a fake cannot have.
    ///
    /// **The first version of this test could not see it.** It asserted
    /// `1023 < port < 65536` and that two calls differed, which any RNG
    /// satisfies: replacing the whole `TcpListener` block with
    /// `gen_range(49152..=65535)` left every test in the crate green — the
    /// build only warned about an unused import. Verified by running that
    /// mutation.
    ///
    /// So this holds a set of ports open and asserts no draw ever names one.
    /// The arithmetic, rather than a hope: the ephemeral range is 16 384 ports
    /// wide, so a uniform fake names a held port on a `held / 16384` fraction
    /// of draws and the expected number of collisions over `DRAWS` is
    /// `DRAWS * held / 16384`. With 120 held and 4 000 draws that is 29, so a
    /// fake survives with probability about `e^-29` — 2 in 10^13. The real
    /// implementation collides with probability **zero**, not merely small.
    #[test]
    fn random_port_is_a_free_port_the_kernel_chose() {
        // As many as this process's descriptor limit allows, up to 120 —
        // enough for the arithmetic above and well inside a 256-fd default.
        let held: Vec<TcpListener> = (0..120)
            .map_while(|_| TcpListener::bind(("127.0.0.1", 0)).ok())
            .collect();
        let taken: std::collections::BTreeSet<u16> = held
            .iter()
            .filter_map(|l| l.local_addr().ok().map(|a| a.port()))
            .collect();
        assert!(
            taken.len() >= 60,
            "not enough listeners to make this test decisive: {}",
            taken.len()
        );

        // Called directly rather than through `render_str`, which builds a
        // whole environment and a thread per call; the wiring is checked once
        // at the end.
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..4_000 {
            let value = random_port(Rest(Vec::new())).expect("a port");
            let port = u16::try_from(i64::try_from(value).expect("an integer")).expect("a port");
            assert!(port > 1023, "{port}");
            assert!(
                !taken.contains(&port),
                "randomPort() returned {port}, which this test is holding open — the kernel \
                 would never hand that out, so the number did not come from the kernel"
            );
            seen.insert(port);
        }
        assert!(
            seen.len() > 1,
            "every call returned the same port: {seen:?}"
        );
        drop(held);

        let port: u32 = ok("{{ randomPort() }}").parse().expect("a port");
        assert!(port > 1023 && port < 65536, "{port}");
    }

    // -----------------------------------------------------------------
    // range, the third minijinja name this crate overrides.
    // -----------------------------------------------------------------

    /// Deleting the `add_function("range", ...)` line does not produce
    /// `unknown function`: minijinja's own `range` global answers instead,
    /// exclusively and positionally. These are the values that separate them.
    #[test]
    fn range_is_kestras_and_not_minijinjas() {
        assert_eq!(ok("{{ range(1, 3) }}"), "[1,2,3]");
        assert_eq!(ok("{{ range(start=1, end=3) }}"), "[1,2,3]");
        assert_eq!(ok("{{ range(start=0, end=10, increment=3) }}"), "[0,3,6,9]");
        assert_eq!(ok("{{ range(start=3, end=1, increment=-1) }}"), "[3,2,1]");
        assert_eq!(ok("{{ range(start=3, end=1) }}"), "[]");
        assert_eq!(
            ok("{{ range(start='a', end='e') }}"),
            r#"["a","b","c","d","e"]"#
        );
        assert_eq!(
            ok("{% for i in range(start=1, end=3) %}{{ i }}{% endfor %}"),
            "123"
        );
    }

    /// Java's `long` arithmetic wraps and its loop then never ends;
    /// `checked_add` and an `i128` count turn both into a refusal or a finite
    /// range. Neither can have a golden: at these sizes Kestra hangs.
    #[test]
    fn range_refuses_the_two_shapes_kestras_own_loop_never_leaves() {
        let message = err("{{ range(start=-9223372036854775808, end=9223372036854775807) }}");
        assert!(
            message.contains("The range function cannot produce more than 1000000 elements"),
            "{message}"
        );
        let message = err("{{ range(start='a', end='z', increment=4294967296) }}");
        assert!(message.contains("never advances the range"), "{message}");
    }

    // -----------------------------------------------------------------
    // The refusals that are egret's own, not Kestra's.
    // -----------------------------------------------------------------

    /// Kestra answers `isPublicHoliday` out of `de.focus_shift.jollyday`
    /// 2.19.0. egret carries no such database, so it refuses **by name**
    /// rather than answering a date a flow branches on -- the same call
    /// `javafmt` makes for `z`, `v` and `B`. `function-ispublicholiday` is
    /// baselined with this reason.
    #[test]
    fn is_public_holiday_refuses_rather_than_guessing() {
        let message = err("{{ isPublicHoliday(date='2024-01-01', countryCode='FR') }}");
        assert!(
            message.contains("egret cannot answer isPublicHoliday()"),
            "{message}"
        );
        assert!(message.contains("jollyday"), "{message}");
        // The argument checks come first, so a broken call reads Kestra's own
        // message rather than the refusal.
        assert!(
            err("{{ isPublicHoliday(date='2024-01-01') }}")
                .contains("The 'isPublicHoliday()' function expects a 'countryCode' argument."),
            "{}",
            err("{{ isPublicHoliday(date='2024-01-01') }}")
        );
    }

    /// Both readers descend recursively over author-controlled text on the
    /// render thread, so both are bounded by the crate's one value-depth
    /// invariant. Verified by REMOVING the bound's effect -- a value one level
    /// inside it must still read.
    #[test]
    fn both_readers_are_bounded_by_the_value_depth_limit() {
        let inside = MAX_VALUE_DEPTH - 1;
        let json = format!("{}{}", "[".repeat(inside), "]".repeat(inside));
        assert!(render(
            "{{ fromJson(json=j) }}",
            &format!(r#"{{"j": {}}}"#, serde_json::to_string(&json).unwrap())
        )
        .is_ok());

        let past = MAX_VALUE_DEPTH + 50;
        let deep = format!("{}{}", "[".repeat(past), "]".repeat(past));
        let message = render(
            "{{ fromJson(json=j) }}",
            &format!(r#"{{"j": {}}}"#, serde_json::to_string(&deep).unwrap()),
        )
        .expect_err("must be refused");
        assert!(message.contains("Invalid json"), "{message}");

        // YAML has no parser-side limit at all -- `serde_norway`'s parser is
        // an event stream -- so `Depth` is the ONLY thing standing here.
        let yaml_deep = format!("{}{}", "[".repeat(past), "]".repeat(past));
        let message = render(
            "{{ yaml(yaml=y) }}",
            &format!(r#"{{"y": {}}}"#, serde_json::to_string(&yaml_deep).unwrap()),
        )
        .expect_err("must be refused");
        assert!(
            message.contains("nests more than 128 levels deep"),
            "{message}"
        );

        // And Ion, whose reader is this crate's own.
        let ion_deep = format!("{}{}", "[".repeat(past), "]".repeat(past));
        let message = render(
            "{{ fromIon(ion=i) }}",
            &format!(r#"{{"i": {}}}"#, serde_json::to_string(&ion_deep).unwrap()),
        )
        .expect_err("must be refused");
        assert!(
            message.contains("nests more than 128 levels deep"),
            "{message}"
        );
    }

    /// The round trips F3 asked for, end to end: serialise with the filter
    /// Task 6 ported and read back with the function this task ports.
    #[test]
    fn the_three_round_trips_come_back_unchanged_except_where_upstream_loses_them() {
        let cx = r#"{"v": {"b": 1, "a": [1, 2.5, "x"], "c": {"d": true}}}"#;
        let direct = render("{{ v }}", cx).unwrap();
        assert_eq!(
            render("{{ fromJson(json=(v | toJson)) }}", cx).unwrap(),
            direct
        );
        assert_eq!(
            render("{{ fromIon(ion=(v | toIon)) }}", cx).unwrap(),
            direct
        );
        assert_eq!(render("{{ yaml(yaml=(v | yaml)) }}", cx).unwrap(), direct);

        // The three that change TYPE, all `yaml`, all upstream.
        assert_eq!(
            ok("{{ yaml(yaml=('42' | yaml)) | className }}"),
            "java.lang.Integer"
        );
        assert_eq!(
            ok("{{ yaml(yaml=('1.5' | yaml)) | className }}"),
            "java.lang.Double"
        );
        assert_eq!(ok("{{ yaml(yaml=('1e5' | yaml)) }}"), "100000.0");
    }
}
