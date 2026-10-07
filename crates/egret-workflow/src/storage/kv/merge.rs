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
//! Every step is linear in the sizes of its inputs (each map level is
//! indexed, a list's membership test is hashed), so a merge of values at
//! `max_read_bytes` stays bounded while the key's lock is held.

use std::collections::{HashMap, HashSet};

use super::value::{KvData, KvNumber};

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
        (Some(KvData::Struct(a)), KvData::Struct(b)) => Ok(KvData::Struct(deep_merge(a, b))),
        _ => Ok(patch.clone()),
    }
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
    let nested = flatten_to_nested(updates)?;
    match current {
        None => Ok(KvData::Struct(nested)),
        Some(KvData::Struct(a)) => Ok(KvData::Struct(deep_merge(a, &nested))),
        Some(_) => Err(NOT_A_MAP.into()),
    }
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

/// The fields of the map at `path` (indices from the root, each naming a
/// map field).
fn level_mut<'a>(root: &'a mut Fields, path: &[usize]) -> &'a mut Fields {
    let mut level = root;
    for &i in path {
        level = match &mut level[i].1 {
            KvData::Struct(f) => f,
            _ => unreachable!("a path only ever steps into a map"),
        };
    }
    level
}

/// `MapUtils.flattenToNestedMap`, Kestra's quirks kept:
/// - a missing level is a new map;
/// - a level already holding a non-map stays where it is, and the rest of
///   the path is built in the CURRENT map (Kestra logs "Conflict at key"
///   and `continue`s);
/// - a last segment already present is ignored (the first entry wins).
///
/// A key of dots only names no field: Kestra fails it with an
/// `ArrayIndexOutOfBoundsException`; egret refuses it with a message.
fn flatten_to_nested(updates: Fields) -> Result<Fields, String> {
    let mut root: Fields = Vec::new();
    // Each visited map's name -> index, by its path from the root.
    let mut index: HashMap<Vec<usize>, HashMap<String, usize>> = HashMap::new();
    fn find(
        root: &mut Fields,
        index: &mut HashMap<Vec<usize>, HashMap<String, usize>>,
        path: &[usize],
        name: &str,
    ) -> Option<usize> {
        let names = index.entry(path.to_vec()).or_insert_with(|| {
            level_mut(root, path)
                .iter()
                .enumerate()
                .map(|(i, (n, _))| (n.clone(), i))
                .collect()
        });
        names.get(name).copied()
    }
    fn push(
        root: &mut Fields,
        index: &mut HashMap<Vec<usize>, HashMap<String, usize>>,
        path: &[usize],
        name: &str,
        value: KvData,
    ) -> usize {
        let level = level_mut(root, path);
        level.push((name.to_string(), value));
        let i = level.len() - 1;
        if let Some(names) = index.get_mut(path) {
            names.insert(name.to_string(), i);
        }
        i
    }
    for (key, value) in updates {
        let segments = java_split_dots(&key);
        let Some((last, levels)) = segments.split_last() else {
            return Err(format!(
                "the keyed `value` entry '{key}' names no field: its key holds only dots"
            ));
        };
        let mut path: Vec<usize> = Vec::new();
        for segment in levels {
            match find(&mut root, &mut index, &path, segment) {
                None => {
                    let i = push(
                        &mut root,
                        &mut index,
                        &path,
                        segment,
                        KvData::Struct(Vec::new()),
                    );
                    path.push(i);
                }
                Some(i) => {
                    if matches!(level_mut(&mut root, &path)[i].1, KvData::Struct(_)) {
                        path.push(i);
                    }
                    // else: Kestra's "Conflict at key", the path stays here.
                }
            }
        }
        if find(&mut root, &mut index, &path, last).is_none() {
            push(&mut root, &mut index, &path, last, value);
        }
    }
    Ok(root)
}

/// `MapUtils.deepMerge(a, b)`.
fn deep_merge(a: &[(String, KvData)], b: &[(String, KvData)]) -> Fields {
    if a.is_empty() {
        return b.to_vec();
    }
    if b.is_empty() {
        return a.to_vec();
    }
    let mut out: Fields = a.to_vec();
    let mut names: HashMap<String, usize> = out
        .iter()
        .enumerate()
        .map(|(i, (n, _))| (n.clone(), i))
        .collect();
    for (name, value_b) in b {
        match names.get(name) {
            Some(&i) => {
                let merged = merge_values(Some(&out[i].1), value_b);
                out[i].1 = merged;
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
fn merge_values(a: Option<&KvData>, b: &KvData) -> KvData {
    match (a, b) {
        (a, KvData::Null) => a.cloned().unwrap_or(KvData::Null),
        (None | Some(KvData::Null), b) => b.clone(),
        (Some(KvData::Struct(x)), KvData::Struct(y)) => KvData::Struct(deep_merge(x, y)),
        (Some(KvData::List(x)), KvData::List(y)) => KvData::List(merge_lists(x, y)),
        (_, b) => b.clone(),
    }
}

/// `MapUtils.mergeCollections`: `a`, then each item of `b` that `a` does not
/// contain (by Java's `equals`, [`java_key`]); `b`'s own duplicates stay.
fn merge_lists(a: &[KvData], b: &[KvData]) -> Vec<KvData> {
    let mut out = a.to_vec();
    if b.is_empty() {
        return out;
    }
    let held: HashSet<String> = a.iter().map(java_key).collect();
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
    use crate::storage::kv::value::{decode, encode, KvType, KvValue};
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
        // 44 cells: 33 merged, 6 refused by the merge, 2 templates (the
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
        assert_eq!((merged, refused), (33, 6));
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
    fn dotted_keys_follow_kestras_split_and_conflict_rules() {
        let cur = data(&json!({"a": 1}));
        // A later level over a scalar entry stays in the current map
        // (Kestra's "Conflict at key", logged and skipped).
        assert_eq!(
            merge(
                Some(&cur),
                &data(&json!([{"key": "q", "value": 1}, {"key": "q.r", "value": 2}]))
            )
            .unwrap()
            .to_json(),
            json!({"a": 1, "q": 1, "r": 2})
        );
        // A last segment already set: the first entry wins.
        assert_eq!(
            merge(
                Some(&cur),
                &data(&json!([{"key": "q.r", "value": 1}, {"key": "q", "value": 5}]))
            )
            .unwrap()
            .to_json(),
            json!({"a": 1, "q": {"r": 1}})
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
