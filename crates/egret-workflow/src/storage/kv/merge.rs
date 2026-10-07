//! `Put`'s merge (W9c spec §8.5): Kestra's `Put.mergeValues`
//! (`Put.java:120-200`) and `MapUtils.deepMerge` / `flattenToNestedMap`
//! (`MapUtils.java:62-225`), re-stated on [`KvData`] and pinned by every
//! `put_merge` cell of `oracle-exec/w9c-kv.json` (the user's ruling,
//! 2026-10-07: `Put` stays a real deep merge, with the artifact's semantics).
//!
//! - A patch that is a non-empty list of `{key, value}` / `{value}` maps (no
//!   other field) is an **entry list**: one `{value}` replaces the whole
//!   value; keyed entries are dotted paths (`a.b` builds `{a: {b: …}}`),
//!   nested and deep-merged into the current map.
//! - Otherwise a map patch deep-merges into a map: the current fields keep
//!   their order, a field the patch names is replaced IN PLACE (two maps
//!   merge recursively, two lists append the patch's items the current list
//!   does not already hold, a null in the patch keeps the current field),
//!   and a new field is appended.
//! - Anything else replaces the value.
//!
//! Every [`KvNumber`] passes through unchanged: a stored decimal stays a
//! decimal (the user's ruling, 2026-10-07). The merge never goes through
//! JSON.
//!
//! What a merge costs, while the key's lock is held (Task 3 re-review 1,
//! C1):
//! - A keyed entry's dotted path is at most [`MAX_DEPTH`] segments, refused
//!   before anything is built: the merged value could not nest deeper and
//!   still be read. A patch's segments IN ALL are bounded by the caller
//!   (`store::put_merge`, by the dense-value limit), before the lock.
//! - The conflict check and the nesting walk each path once, over a trie of
//!   the paths (a segment is one hash lookup keyed by its parent's id), so
//!   they are linear in the total length of the keys.
//! - The deep merge moves the current value rather than copying a level per
//!   step: linear in the sizes of the current value and the patch, each map
//!   level indexed. A list's membership test hashes Java's `equals` key of
//!   each item, which sorts a map's field names: `n log n` in a map's width.
//! - Recursion (the deep merge, the build of the nesting, a drop) is as deep
//!   as the values: the current one is at most [`MAX_DEPTH`] (reads enforce
//!   it), the nesting at most [`MAX_DEPTH`] plus a leaf's own depth.

use std::collections::{HashMap, HashSet};

use super::value::{KvData, KvNumber};
use crate::storage::ion::MAX_DEPTH;

/// `Put.java`: an unkeyed entry beside any other.
const UNKEYED: &str =
    "`value` entries without `key` can only be used as a single replacement item.";
/// `Put.java`: a keyed entry after an unkeyed one (unreachable: an unkeyed
/// entry in a list of two or more is refused first, as on Kestra).
const MIXED: &str = "Cannot mix keyed and non-keyed entries in `value`.";
/// `Put.java`: a `key` that is not a string, or is blank.
const BAD_KEY: &str = "Each keyed `value` entry must have a non-empty string `key`.";
/// `Put.java`: keyed entries over a value that is not a map.
const NOT_A_MAP: &str = "Cannot apply keyed `value` updates to a non-JSON existing KV value.";

type Fields = Vec<(String, KvData)>;

/// Kestra's Put (Put.java:72-121), against w9c `put_merge`, re-stated on
/// KvData: a map deep-merges into a map (KvData::Struct: field order kept,
/// an existing name replaced IN PLACE, a new name appended); a list of
/// {key, value} entries patches dotted paths; a single [{value}] replaces. A
/// shape the fixture refuses is Err with its message. `current` None: the
/// key is missing. Every KvNumber passes through unchanged (a stored
/// decimal stays a decimal).
pub fn merge(current: Option<&KvData>, patch: &KvData) -> Result<KvData, String> {
    // Kestra cannot tell a stored null from a missing key: both are `null`.
    let current = current.filter(|c| !matches!(c, KvData::Null));
    if let KvData::List(entries) = patch {
        if is_entry_list(entries) {
            return merge_entries(current, entries);
        }
    }
    match (current, patch) {
        (Some(KvData::Struct(a)), KvData::Struct(b)) => {
            Ok(KvData::Struct(deep_merge(a.clone(), b)))
        }
        _ => Ok(patch.clone()),
    }
}

/// The dotted segments of every keyed entry of an entry-list patch, in all,
/// as Java's `split` counts them (a key repeated counts each time); 0 for
/// any other patch. `store::put_merge` bounds it before it takes the key's
/// lock (C1). Linear in the keys' length; builds nothing.
pub(crate) fn patch_path_segments(patch: &KvData) -> usize {
    let KvData::List(entries) = patch else {
        return 0;
    };
    if !is_entry_list(entries) {
        return 0;
    }
    entries
        .iter()
        .filter_map(|entry| match entry {
            KvData::Struct(f) => match field(f, "key") {
                Some(KvData::String(key)) => Some(segment_count(key)),
                _ => None,
            },
            _ => None,
        })
        .sum()
}

/// `java_split_dots(key).len()`, without building the split.
fn segment_count(key: &str) -> usize {
    let kept = key.trim_end_matches('.');
    if kept.is_empty() {
        0
    } else {
        kept.matches('.').count() + 1
    }
}

/// C1: a keyed entry whose path is deeper than [`MAX_DEPTH`]. The key is
/// quoted shortened: it may be 100 KB.
fn too_deep(key: &str, segments: usize) -> String {
    const SHOWN: usize = 40;
    let shown = match key.char_indices().nth(SHOWN) {
        Some((end, _)) => format!("{}...", &key[..end]),
        None => key.to_string(),
    };
    format!(
        "the keyed `value` entry '{shown}' has {segments} dotted segments, more than the \
         {MAX_DEPTH} levels a KV value may nest"
    )
}

fn field<'a>(fields: &'a [(String, KvData)], name: &str) -> Option<&'a KvData> {
    fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
}

/// `Put.isValueEntryList`: non-empty, and every item a map holding `value`
/// and no field but `key` and `value`.
fn is_entry_list(entries: &[KvData]) -> bool {
    !entries.is_empty()
        && entries.iter().all(|item| match item {
            KvData::Struct(f) => {
                field(f, "value").is_some() && f.iter().all(|(n, _)| n == "key" || n == "value")
            }
            _ => false,
        })
}

/// `Put.mergeEntryListValues`.
fn merge_entries(current: Option<&KvData>, entries: &[KvData]) -> Result<KvData, String> {
    let mut replace: Option<KvData> = None;
    // Kestra keeps the updates in a HashMap (a later entry for the same key
    // replaces the earlier one); egret keeps the entries' order (w9c
    // `put_merge.missing_entries`' note).
    let mut updates: Fields = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        let KvData::Struct(f) = entry else {
            unreachable!("is_entry_list checked every entry is a map")
        };
        let value = field(f, "value").cloned().unwrap_or(KvData::Null);
        match field(f, "key") {
            None | Some(KvData::Null) => {
                if replace.is_some() || !updates.is_empty() || entries.len() > 1 {
                    return Err(UNKEYED.into());
                }
                replace = Some(value);
            }
            Some(key) => {
                if replace.is_some() {
                    return Err(MIXED.into());
                }
                let KvData::String(key) = key else {
                    return Err(BAD_KEY.into());
                };
                if key.chars().all(java_whitespace) {
                    return Err(BAD_KEY.into());
                }
                // C1: before the conflict check and the nesting build
                // anything.
                let segments = segment_count(key);
                if segments > MAX_DEPTH {
                    return Err(too_deep(key, segments));
                }
                match seen.get(key) {
                    Some(&i) => updates[i].1 = value,
                    None => {
                        seen.insert(key.clone(), updates.len());
                        updates.push((key.clone(), value));
                    }
                }
            }
        }
    }
    if let Some(value) = replace {
        return Ok(value);
    }
    refuse_conflicts(&updates)?;
    let nested = flatten_to_nested(updates)?;
    match current {
        None => Ok(KvData::Struct(nested)),
        Some(KvData::Struct(a)) => Ok(KvData::Struct(deep_merge(a.clone(), &nested))),
        Some(_) => Err(NOT_A_MAP.into()),
    }
}

/// The message of two keyed entries whose dotted paths conflict.
pub(crate) fn conflict_message(first: &str, second: &str) -> String {
    format!(
        "the keyed `value` entries '{first}' and '{second}' conflict: one sets a field that the \
         other also sets or writes inside; Kestra's result then depends on the order of its \
         HashMap, so egret refuses them"
    )
}

/// The user's ruling (2026-10-07, Task 3 review I2): keyed entries whose
/// dotted paths conflict -- one key's path is a leaf that is also the path,
/// or a path through which, another key writes (`q` and `q.r`; `a` and
/// `a.`) -- are refused, naming both keys in entry order. On Kestra the
/// outcome of such a patch depends on its `HashMap`'s iteration order, not
/// on the entries. Non-conflicting entries keep Kestra's semantics.
///
/// The paths go into a trie: a node is a path, its child one segment
/// further, looked up by `(parent id, segment)`. Each segment of each key is
/// one lookup, so the check is linear in the total length of the keys (C1;
/// the previous version hashed every prefix as a vector, quadratic in a
/// path's depth).
fn refuse_conflicts(updates: &[(String, KvData)]) -> Result<(), String> {
    /// The first key that set the node's path as a leaf, and the first that
    /// wrote through it.
    #[derive(Default)]
    struct Node<'a> {
        leaf: Option<&'a str>,
        through: Option<&'a str>,
    }
    let mut nodes: Vec<Node> = vec![Node::default()];
    let mut children: HashMap<(usize, &str), usize> = HashMap::new();
    let mut child = |nodes: &mut Vec<Node>, parent: usize, segment| {
        *children.entry((parent, segment)).or_insert_with(|| {
            nodes.push(Node::default());
            nodes.len() - 1
        })
    };
    for (key, _) in updates {
        let path = java_split_dots(key);
        let Some((last, steps)) = path.split_last() else {
            continue; // refused by flatten_to_nested, naming the key
        };
        let mut at = 0;
        for segment in steps {
            at = child(&mut nodes, at, *segment);
            if let Some(other) = nodes[at].leaf {
                return Err(conflict_message(other, key));
            }
            nodes[at].through.get_or_insert(key);
        }
        let end = child(&mut nodes, at, *last);
        if let Some(other) = nodes[end].leaf.or(nodes[end].through) {
            return Err(conflict_message(other, key));
        }
        nodes[end].leaf = Some(key);
    }
    Ok(())
}

/// Java's `Character.isWhitespace`, which `String.isBlank` asks of every
/// character: the Unicode space, line and paragraph separators but the
/// no-break ones (U+00A0, U+2007, U+202F), and `\t \n \u000B \f \r` and
/// U+001C..U+001F. (Rust's `char::is_whitespace` differs on U+0085, the
/// no-break spaces and U+001C..U+001F.)
fn java_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0B}'
            | '\u{0C}'
            | '\r'
            | '\u{1C}'..='\u{1F}'
            | ' '
            | '\u{1680}'
            | '\u{2000}'..='\u{2006}'
            | '\u{2008}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

/// Java's `key.split("\\.")`: split at every dot, then drop the TRAILING
/// empty segments (`"a."` is `["a"]`, `".a"` is `["", "a"]`, `"a..b"` is
/// `["a", "", "b"]`, `"..."` is `[]`).
fn java_split_dots(key: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = key.split('.').collect();
    while parts.last() == Some(&"") {
        parts.pop();
    }
    parts
}

/// One field of a map [`flatten_to_nested`] is building: a map it built
/// (by its index in the arena), or a leaf's value.
enum Slot {
    Map(usize),
    Leaf(KvData),
}

/// A map [`flatten_to_nested`] is building, its names indexed.
#[derive(Default)]
struct Level {
    fields: Vec<(String, Slot)>,
    names: HashMap<String, usize>,
}

impl Level {
    /// A leaf's map, opened so a later key's path can go inside it.
    fn of(fields: Fields) -> Self {
        let names = fields
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.clone(), i))
            .collect();
        let fields = fields
            .into_iter()
            .map(|(n, v)| (n, Slot::Leaf(v)))
            .collect();
        Self { fields, names }
    }

    fn push(&mut self, name: &str, slot: Slot) {
        self.names.insert(name.to_string(), self.fields.len());
        self.fields.push((name.to_string(), slot));
    }
}

/// `MapUtils.flattenToNestedMap`: a missing level is a new map. Its two
/// conflict quirks (a level over a non-map stays in the current map; a last
/// segment already present is ignored) depend on Kestra's `HashMap` order
/// and are unreachable here: [`refuse_conflicts`] refuses every patch that
/// would reach them (the user's ruling, 2026-10-07). They are kept as Kestra
/// wrote them.
///
/// A key of dots only names no field: Kestra fails it with an
/// `ArrayIndexOutOfBoundsException`; egret refuses it with a message.
///
/// The maps are built in an arena, each step one lookup in its parent's
/// index, then turned into [`KvData`] once: linear in the total length of
/// the keys (C1; the previous version re-walked the path from the root and
/// hashed it at every segment, quadratic in a path's depth).
fn flatten_to_nested(updates: Fields) -> Result<Fields, String> {
    let mut levels: Vec<Level> = vec![Level::default()];
    for (key, value) in updates {
        let segments = java_split_dots(&key);
        let Some((last, steps)) = segments.split_last() else {
            return Err(format!(
                "the keyed `value` entry '{key}' names no field: its key holds only dots"
            ));
        };
        let mut at = 0;
        for segment in steps {
            let next = levels.len();
            match levels[at].names.get(*segment).copied() {
                None => {
                    levels[at].push(segment, Slot::Map(next));
                    levels.push(Level::default());
                    at = next;
                }
                Some(i) => {
                    let slot = &mut levels[at].fields[i].1;
                    match slot {
                        Slot::Map(id) => at = *id,
                        Slot::Leaf(KvData::Struct(_)) => {
                            // A map set as a leaf: the path goes inside it
                            // (Kestra's `instanceof Map`).
                            let Slot::Leaf(KvData::Struct(fields)) =
                                std::mem::replace(slot, Slot::Map(next))
                            else {
                                unreachable!("matched above")
                            };
                            levels.push(Level::of(fields));
                            at = next;
                        }
                        // Kestra's "Conflict at key": the path stays here.
                        Slot::Leaf(_) => {}
                    }
                }
            }
        }
        if !levels[at].names.contains_key(*last) {
            levels[at].push(last, Slot::Leaf(value));
        }
    }
    Ok(build(&mut levels, 0))
}

/// The arena's map `id` as fields, its child maps built recursively (as deep
/// as the longest path, at most [`MAX_DEPTH`], plus a leaf map's depth).
fn build(levels: &mut [Level], id: usize) -> Fields {
    std::mem::take(&mut levels[id].fields)
        .into_iter()
        .map(|(name, slot)| match slot {
            Slot::Leaf(value) => (name, value),
            Slot::Map(child) => (name, KvData::Struct(build(levels, child))),
        })
        .collect()
}

/// `MapUtils.deepMerge(a, b)`, `a` moved in: each level is moved, never
/// copied again.
fn deep_merge(a: Fields, b: &[(String, KvData)]) -> Fields {
    if a.is_empty() {
        return b.to_vec();
    }
    if b.is_empty() {
        return a;
    }
    let mut out = a;
    let mut names: HashMap<String, usize> = out
        .iter()
        .enumerate()
        .map(|(i, (n, _))| (n.clone(), i))
        .collect();
    for (name, value_b) in b {
        match names.get(name) {
            Some(&i) => {
                let current = std::mem::replace(&mut out[i].1, KvData::Null);
                out[i].1 = merge_values(Some(current), value_b);
            }
            None => {
                // `result.put(key, mergeValues(null, valueB))`: a null in the
                // patch for a field the map lacks adds the field, null.
                out.push((name.clone(), merge_values(None, value_b)));
                names.insert(name.clone(), out.len() - 1);
            }
        }
    }
    out
}

/// `MapUtils.mergeValues(valueA, valueB)`.
fn merge_values(a: Option<KvData>, b: &KvData) -> KvData {
    match (a, b) {
        (a, KvData::Null) => a.unwrap_or(KvData::Null),
        (None | Some(KvData::Null), b) => b.clone(),
        (Some(KvData::Struct(x)), KvData::Struct(y)) => KvData::Struct(deep_merge(x, y)),
        (Some(KvData::List(x)), KvData::List(y)) => KvData::List(merge_lists(x, y)),
        (_, b) => b.clone(),
    }
}

/// `MapUtils.mergeCollections`: `a`, then each item of `b` that `a` does not
/// contain (by Java's `equals`, [`java_key`]); `b`'s own duplicates stay.
fn merge_lists(a: Vec<KvData>, b: &[KvData]) -> Vec<KvData> {
    if b.is_empty() {
        return a;
    }
    let held: HashSet<String> = a.iter().map(java_key).collect();
    let mut out = a;
    out.extend(
        b.iter()
            .filter(|item| !held.contains(&java_key(item)))
            .cloned(),
    );
    out
}

/// A text equal for two values exactly when Java's `equals` holds between
/// what Kestra's reader makes of them: a map equals a map with the same
/// entries in any order; a list is ordered; numbers are equal only of the
/// same kind and text (an `Integer` never equals a `Double`, a
/// `BigDecimal`'s scale counts); a `Double` by its bits, every NaN one.
fn java_key(d: &KvData) -> String {
    let mut out = String::new();
    write_key(d, &mut out);
    out
}

fn write_key(d: &KvData, out: &mut String) {
    use std::fmt::Write as _;
    match d {
        KvData::Null => out.push('n'),
        KvData::Bool(b) => out.push(if *b { 't' } else { 'f' }),
        KvData::Number(KvNumber::Int(t)) => {
            let _ = write!(out, "i{t};");
        }
        KvData::Number(KvNumber::Decimal(t)) => {
            let _ = write!(out, "d{t};");
        }
        KvData::Number(KvNumber::Float(f)) => {
            let bits = if f.is_nan() {
                f64::NAN.to_bits()
            } else {
                f.to_bits()
            };
            let _ = write!(out, "x{bits};");
        }
        KvData::String(s) => {
            let _ = write!(out, "s{}:{s}", s.len());
        }
        KvData::List(items) => {
            out.push('[');
            for item in items {
                write_key(item, out);
            }
            out.push(']');
        }
        KvData::Struct(fields) => {
            let mut sorted: Vec<&(String, KvData)> = fields.iter().collect();
            sorted.sort_by(|x, y| x.0.cmp(&y.0));
            out.push('{');
            for (name, value) in sorted {
                let _ = write!(out, "{}:{name}", name.len());
                write_key(value, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::ion::MAX_DEPTH;
    use crate::storage::kv::value::{check_readable, decode, encode, KvType, KvValue};
    use serde_json::{json, Value};

    fn data(v: &Value) -> KvData {
        KvData::from_json(v).unwrap()
    }

    fn fixture() -> Value {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/oracle-exec/w9c-kv.json"
        )))
        .unwrap()
    }

    /// Every `put_merge` cell, driven from its top level only. A cell whose
    /// patch is a template is the render's (Task 5); a missing key with
    /// `errorOnMissing: true` is the store's refusal, before any merge
    /// (`tests/kv_store.rs`). The rest: `merge(existing, patch)` is the
    /// cell's `result` with key ORDER exact and its `ion_bytes` byte-exact,
    /// or its `message`.
    #[test]
    fn every_put_merge_cell_of_the_artifact() {
        let f = fixture();
        let cells = f["egret_expected"]["put_merge"].as_object().unwrap();
        let (mut merged, mut refused, mut skipped) = (0, 0, Vec::new());
        for (name, cell) in cells {
            if name.starts_with('_') {
                continue;
            }
            let patch = &cell["patch"];
            if patch.to_string().contains("{{") {
                skipped.push(name.as_str());
                continue;
            }
            let live = cell["existing_state"] == "live";
            if !live && cell["errorOnMissing"] == json!(true) {
                skipped.push(name.as_str());
                continue;
            }
            let existing = live.then(|| data(&cell["existing"]));
            let got = merge(existing.as_ref(), &data(patch));
            match cell["outcome"].as_str().unwrap() {
                "ok" => {
                    let got = got.unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert_eq!(
                        serde_json::to_string(&got.to_json()).unwrap(),
                        serde_json::to_string(&cell["result"]).unwrap(),
                        "{name}: the result, key order exact"
                    );
                    if let Some(bytes) = cell["ion_bytes"].as_str() {
                        let v = KvValue {
                            kind: KvType::of_data(&got),
                            data: got,
                        };
                        assert_eq!(
                            String::from_utf8(encode(&v).unwrap()).unwrap(),
                            bytes,
                            "{name}: the Ion bytes"
                        );
                    }
                    merged += 1;
                }
                "error" => {
                    assert_eq!(
                        got,
                        Err(cell["message"].as_str().unwrap().to_string()),
                        "{name}"
                    );
                    refused += 1;
                }
                other => panic!("{name}: outcome {other}"),
            }
        }
        // 47 cells: 32 merged, 10 refused by the merge (4 of them the user's
        // ruling on conflicting dotted keys, 2026-10-07), 2 templates (the
        // render's, Task 5) and 3 missing keys with errorOnMissing (the
        // store's refusal, tests/kv_store.rs).
        skipped.sort_unstable();
        assert_eq!(
            skipped,
            [
                "missing_true",
                "missing_true_entries",
                "over_expired_eom_true",
                "templated",
                "templated_leaf"
            ]
        );
        assert_eq!((merged, refused), (32, 10));
    }

    #[test]
    fn a_merge_keeps_a_stored_decimal_and_a_patch_float_stays_a_float() {
        let current = decode(KvType::Json, b"{a:1.5}", 4096).unwrap();
        let got = merge(Some(&current), &data(&json!({"b": 0.5}))).unwrap();
        let v = KvValue {
            kind: KvType::of_data(&got),
            data: got,
        };
        assert_eq!(encode(&v).unwrap(), b"{a:1.5,b:0.5e0}");
        // A long decimal keeps every digit through a nested merge.
        let digits = format!("{}.{}", "1".repeat(40), "2".repeat(40));
        let current = decode(
            KvType::Json,
            format!("{{x:{{d:{digits}}},l:[1.50]}}").as_bytes(),
            4096,
        )
        .unwrap();
        let got = merge(Some(&current), &data(&json!({"x": {"e": 1}, "l": [2]}))).unwrap();
        let v = KvValue {
            kind: KvType::of_data(&got),
            data: got,
        };
        assert_eq!(
            String::from_utf8(encode(&v).unwrap()).unwrap(),
            // (`encode` writes a decimal without its trailing zeros, as
            // Kestra does: w9c `api.put_bodies.dec_150`.)
            format!("{{x:{{d:{digits},e:1}},l:[1.5,2]}}")
        );
    }

    #[test]
    fn lists_append_only_what_the_current_list_lacks_by_java_equality() {
        let cur = data(&json!({"l": [1, {"a": 1, "b": 2}, 1.0]}));
        let got = merge(
            Some(&cur),
            &data(&json!({"l": [1, {"b": 2, "a": 1}, 1, 2, 2, 1.0, 1.5]})),
        )
        .unwrap();
        // 1 and the reordered map are held; an Integer 1 never equals the
        // Double 1.0; the patch's own duplicates (2, 2) stay, as in Java.
        assert_eq!(
            got.to_json(),
            json!({"l": [1, {"a": 1, "b": 2}, 1.0, 2, 2, 1.5]})
        );
        // A decimal's scale counts (BigDecimal.equals): 1.50 is not 1.5.
        let cur = decode(KvType::Json, b"{l:[1.5]}", 4096).unwrap();
        let patch = decode(KvType::Json, b"{l:[1.50, 1.5]}", 4096).unwrap();
        let dec = |t: &str| KvData::Number(KvNumber::Decimal(t.into()));
        assert_eq!(
            merge(Some(&cur), &patch).unwrap(),
            KvData::Struct(vec![(
                "l".into(),
                KvData::List(vec![dec("1.5"), dec("1.50")])
            )])
        );
    }

    #[test]
    fn a_null_patch_field_keeps_the_current_one_and_adds_a_missing_one_as_null() {
        let got = merge(
            Some(&data(&json!({"a": 1}))),
            &data(&json!({"a": null, "z": null})),
        )
        .unwrap();
        assert_eq!(
            got,
            KvData::Struct(vec![
                ("a".into(), data(&json!(1))),
                ("z".into(), KvData::Null)
            ])
        );
        // A whole null patch replaces (Put.mergeValues: not a map, not a list).
        assert_eq!(
            merge(Some(&data(&json!({"a": 1}))), &KvData::Null),
            Ok(KvData::Null)
        );
        // A stored null is a missing key to the merge, as in Java.
        assert_eq!(
            merge(
                Some(&KvData::Null),
                &data(&json!([{"key": "a.b", "value": 1}]))
            ),
            Ok(data(&json!({"a": {"b": 1}})))
        );
    }

    #[test]
    fn dotted_keys_that_conflict_are_refused_naming_both() {
        let cur = data(&json!({"a": 1}));
        // The user's ruling (2026-10-07, review I2): a leaf that another key
        // writes inside, in either order, and two keys naming one field.
        for (patch, first, second) in [
            (
                json!([{"key": "q", "value": 1}, {"key": "q.r", "value": 2}]),
                "q",
                "q.r",
            ),
            (
                json!([{"key": "q.r", "value": 1}, {"key": "q", "value": 5}]),
                "q.r",
                "q",
            ),
            (
                json!([{"key": "a.b", "value": 2}, {"key": "a", "value": {"z": 1}}]),
                "a.b",
                "a",
            ),
            (
                json!([{"key": "a", "value": {"z": 1}}, {"key": "a.b", "value": 2}]),
                "a",
                "a.b",
            ),
            (
                json!([{"key": "x", "value": 1}, {"key": "a.b.c", "value": 1}, {"key": "a", "value": 2}]),
                "a.b.c",
                "a",
            ),
            (
                json!([{"key": "a", "value": 1}, {"key": "a.", "value": 2}]),
                "a",
                "a.",
            ),
            (
                json!([{"key": "m.n", "value": 1}, {"key": "m.n", "value": 2}, {"key": "m", "value": 3}]),
                "m.n",
                "m",
            ),
        ] {
            assert_eq!(
                merge(Some(&cur), &data(&patch)),
                Err(conflict_message(first, second)),
                "{patch}"
            );
            assert_eq!(
                merge(None, &data(&patch)),
                Err(conflict_message(first, second))
            );
        }
        // Non-conflicting siblings and a shared parent are not conflicts.
        assert_eq!(
            merge(
                Some(&cur),
                &data(&json!([{"key": "p.x", "value": 1}, {"key": "p.y", "value": 2}, {"key": "p.z.w", "value": 3}]))
            )
            .unwrap()
            .to_json(),
            json!({"a": 1, "p": {"x": 1, "y": 2, "z": {"w": 3}}})
        );
        // The same key twice: the later value, in the first place.
        assert_eq!(
            merge(
                Some(&cur),
                &data(&json!([{"key": "b", "value": 1}, {"key": "c", "value": 2}, {"key": "b", "value": 3}]))
            )
            .unwrap()
            .to_json(),
            json!({"a": 1, "b": 3, "c": 2})
        );
        // Dots only: no field named.
        for k in [".", "..."] {
            let err = merge(Some(&cur), &data(&json!([{"key": k, "value": 1}]))).unwrap_err();
            assert!(err.contains("names no field"), "{k}: {err}");
        }
        assert_eq!(java_split_dots("a."), ["a"]);
        assert_eq!(java_split_dots(".a"), ["", "a"]);
        assert_eq!(java_split_dots("a..b"), ["a", "", "b"]);
        assert!(java_split_dots("...").is_empty());
    }

    #[test]
    fn a_blank_key_is_javas_blank() {
        let cur = data(&json!({"a": 1}));
        for blank in [" ", "\t\n", "\u{2003}", "\u{1F}", "\u{3000}"] {
            assert_eq!(
                merge(Some(&cur), &data(&json!([{"key": blank, "value": 1}]))),
                Err(BAD_KEY.to_string()),
                "{blank:?}"
            );
        }
        // Java's isWhitespace is false for the no-break spaces and U+0085.
        for not_blank in ["\u{A0}", "\u{2007}", "\u{202F}", "\u{85}"] {
            assert!(
                merge(Some(&cur), &data(&json!([{"key": not_blank, "value": 1}]))).is_ok(),
                "{not_blank:?}"
            );
        }
    }

    /// `f` on a thread with a 2 MiB stack, the size of tokio's blocking-pool
    /// threads, where `put_merge` runs the merge (Task 3 re-review 1, C1).
    /// `f` drops what it builds before it returns, so the drop runs there
    /// too.
    fn on_a_blocking_pool_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    fn entry_list(keys: impl IntoIterator<Item = String>) -> KvData {
        KvData::List(
            keys.into_iter()
                .map(|k| {
                    KvData::Struct(vec![
                        ("key".into(), KvData::String(k)),
                        ("value".into(), KvData::Number(KvNumber::Int("1".into()))),
                    ])
                })
                .collect(),
        )
    }

    fn nesting(d: &KvData) -> usize {
        let mut depth = 0;
        let mut at = d;
        while let KvData::Struct(f) = at {
            depth += 1;
            at = &f.last().unwrap().1;
        }
        depth
    }

    /// C1: a dotted key of 50 000 segments took 88 s and then overflowed a
    /// 2 MiB stack while it was dropped, aborting the process. It is refused
    /// before anything is built, naming the limit; so is one of
    /// `MAX_DEPTH + 1` segments.
    #[test]
    fn a_dotted_key_deeper_than_max_depth_is_refused_at_once() {
        for n in [MAX_DEPTH + 1, 50_000] {
            let (got, took) = on_a_blocking_pool_stack(move || {
                let patch = entry_list([vec!["a"; n].join(".")]);
                let started = std::time::Instant::now();
                let got = merge(Some(&data(&json!({"x": 1}))), &patch);
                (got, started.elapsed())
            });
            let err = got.unwrap_err();
            assert!(
                err.contains(&format!("{n} dotted segments"))
                    && err.contains(&format!("more than the {MAX_DEPTH}")),
                "{err}"
            );
            assert!(err.len() < 400, "the message quotes the whole key: {err}");
            assert!(
                took < std::time::Duration::from_millis(250),
                "{n} segments took {took:?}"
            );
        }
    }

    /// C1's control: a key of exactly `MAX_DEPTH` segments merges, into a
    /// missing key and into a map, and the result is storable.
    #[test]
    fn a_dotted_key_of_exactly_max_depth_merges_and_is_storable() {
        on_a_blocking_pool_stack(|| {
            let patch = entry_list([vec!["a"; MAX_DEPTH].join(".")]);
            for current in [None, Some(data(&json!({"x": 1})))] {
                let got = merge(current.as_ref(), &patch).unwrap();
                assert_eq!(nesting(&got), MAX_DEPTH);
                let v = KvValue {
                    kind: KvType::of_data(&got),
                    data: got,
                };
                check_readable(&v, 10 * 1024 * 1024).unwrap();
            }
        });
    }

    /// C1: many keys near the depth limit, as many segments in all as the
    /// patch budget allows at the default cap (163 840), stay fast: the
    /// conflict check and the nesting are linear in the segments. Two
    /// shapes: one long shared prefix, and no prefix shared at all.
    #[test]
    fn many_keys_near_the_depth_limit_merge_fast() {
        let keys = (10 * 1024 * 1024 / 64) / MAX_DEPTH;
        let took = on_a_blocking_pool_stack(move || {
            let shared = vec!["p"; MAX_DEPTH - 1].join(".");
            let deep = vec!["a"; MAX_DEPTH - 1].join(".");
            let started = std::time::Instant::now();
            let one = merge(
                None,
                &entry_list((0..keys).map(|i| format!("{shared}.k{i}"))),
            )
            .unwrap();
            assert_eq!(nesting(&one), MAX_DEPTH);
            let other = merge(
                Some(&data(&json!({"x": 1}))),
                &entry_list((0..keys).map(|i| format!("k{i}.{deep}"))),
            )
            .unwrap();
            let KvData::Struct(f) = &other else { panic!() };
            assert_eq!(f.len(), keys + 1);
            drop((one, other));
            started.elapsed()
        });
        assert!(
            took < std::time::Duration::from_secs(2),
            "{keys} keys of {MAX_DEPTH} segments took {took:?}"
        );
    }

    /// The budget's count: every keyed entry's dotted segments, as Java's
    /// split counts them; 0 for a patch that is not an entry list.
    #[test]
    fn patch_path_segments_counts_as_java_splits() {
        let patch = data(&json!([
            {"key": "a.b", "value": 1},
            {"key": "c.", "value": 1},
            {"key": ".d", "value": 1},
            {"key": "e..f", "value": 1},
            {"key": "...", "value": 1}
        ]));
        assert_eq!(patch_path_segments(&patch), 2 + 1 + 2 + 3);
        assert_eq!(patch_path_segments(&data(&json!({"a.b.c": 1}))), 0);
        assert_eq!(patch_path_segments(&data(&json!([{"value": 1}]))), 0);
        for k in ["a", "a.", ".a", "a..b", "...", "a.b.c", ""] {
            assert_eq!(segment_count(k), java_split_dots(k).len(), "{k:?}");
        }
    }

    /// A merge of two maps of 100 000 fields each, and of two lists of
    /// 100 000 items, is linear: well under a second (a quadratic merge
    /// takes minutes).
    #[test]
    fn a_large_merge_is_linear() {
        let n = 100_000;
        let a = KvData::Struct(
            (0..n)
                .map(|i| {
                    (
                        format!("f{i}"),
                        KvData::Number(KvNumber::Int(i.to_string())),
                    )
                })
                .collect(),
        );
        let b = KvData::Struct(
            (n / 2..n + n / 2)
                .map(|i| (format!("f{i}"), KvData::Number(KvNumber::Int("1".into()))))
                .collect(),
        );
        let la = KvData::Struct(vec![(
            "l".into(),
            KvData::List((0..n).map(|i| KvData::String(i.to_string())).collect()),
        )]);
        let lb = KvData::Struct(vec![(
            "l".into(),
            KvData::List(
                (n / 2..n + n / 2)
                    .map(|i| KvData::String(i.to_string()))
                    .collect(),
            ),
        )]);
        let started = std::time::Instant::now();
        let KvData::Struct(m) = merge(Some(&a), &b).unwrap() else {
            panic!()
        };
        assert_eq!(m.len(), n + n / 2);
        let KvData::Struct(l) = merge(Some(&la), &lb).unwrap() else {
            panic!()
        };
        assert!(matches!(&l[0].1, KvData::List(items) if items.len() == n + n / 2));
        let entries = KvData::List(
            (0..n)
                .map(|i| {
                    KvData::Struct(vec![
                        ("key".into(), KvData::String(format!("x.k{i}"))),
                        ("value".into(), KvData::Bool(true)),
                    ])
                })
                .collect(),
        );
        let KvData::Struct(e) = merge(Some(&a), &entries).unwrap() else {
            panic!()
        };
        assert!(matches!(&e[n].1, KvData::Struct(f) if f.len() == n));
        let took = started.elapsed();
        assert!(
            took < std::time::Duration::from_secs(5),
            "three merges took {took:?}"
        );
    }
}
