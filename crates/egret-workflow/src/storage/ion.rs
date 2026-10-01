//! The Ion READER of the execution file API's preview (W9a spec §8).
//!
//! `ion-rs` 1.0.1 over bytes, **one top-level value at a time, text or
//! binary**: Kestra's STORE and item-task files are binary Ion 1.0 with a
//! version marker and a local symbol table before every row and no lines
//! (`w9a-storage.json` finding `store_binary_ion`); `Write`'s and `ionfmt`'s
//! are text. Nothing here splits on `'\n'`, and nothing reads past the value
//! that fills the page (plus one, to know whether more follow).
//!
//! # The JSON a value becomes (Kestra's preview rows, measured)
//!
//! | Ion | JSON | measured |
//! |---|---|---|
//! | struct | object; a field whose value is any `null` is **dropped** | `n:null` absent from every row |
//! | list, sexp | array; a `null` element **stays** `null` | `l:[null]` |
//! | timestamp | its Ion text as a string, a day-precision date without the `T` | `"2025-12-03"`, `"2025-12-03T12:00:00.123+07:00"` |
//! | decimal | a JSON number, as a double | `12.50` -> `12.5`, `1d2` -> `100.0` |
//! | float | a JSON number; NaN and the infinities as strings | `1.5e0` -> `1.5`, `2e0` -> `2.0` |
//! | int | a JSON number, exact at any size | `9007199254740993` |
//! | string, symbol | a string | |
//! | blob, clob | base64 of the bytes, as a `byte[]` is | (chosen; `runners::ionfmt`'s table) |
//! | annotations | dropped: `LocalDateTime::"…"` is the string | (chosen) |
//! | top-level `null` | `null` | |

use std::io::Read;

use serde_json::Value;

/// Why reading stopped.
#[derive(Debug)]
pub(crate) enum Stop {
    /// `max` values were read and another follows.
    Full,
    /// The input ended after the last value read.
    End,
    /// The input is not valid Ion, or the reader failed, after the values read.
    Failed(String),
}

/// Up to `max` top-level values of `bytes` as JSON, and whether more follow.
///
/// `Err` names why the input is not Ion; the values before the fault are lost
/// (see [`read_values`] for a caller that keeps them).
pub fn values_to_json(
    bytes: impl std::io::Read,
    max: usize,
) -> Result<(Vec<Value>, bool /*truncated*/), String> {
    match read_values(bytes, max) {
        (rows, Stop::Full) => Ok((rows, true)),
        (rows, Stop::End) => Ok((rows, false)),
        (_, Stop::Failed(e)) => Err(e),
    }
}

/// [`values_to_json`], keeping the values read before a fault.
///
/// Reads one value past `max` only to learn whether more follow. A reader
/// that fails mid-value (the preview's byte budget does, on purpose) never
/// yields that value cut short: the Ion reader asks for more bytes before it
/// ends a token, and gets the error.
pub(crate) fn read_values(bytes: impl Read, max: usize) -> (Vec<Value>, Stop) {
    let reader: Box<dyn Read + '_> = Box::new(bytes);
    // `Box<dyn Read>` is an `IonInput` only for `'static`; a `BufReader` is
    // one for any `Read`.
    let mut values = match ion_rs::Element::iter(std::io::BufReader::new(reader)) {
        Ok(it) => it,
        Err(e) => return (Vec::new(), Stop::Failed(e.to_string())),
    };
    let mut rows = Vec::new();
    loop {
        match values.next() {
            None => return (rows, Stop::End),
            Some(Err(e)) => return (rows, Stop::Failed(e.to_string())),
            Some(Ok(_)) if rows.len() == max => return (rows, Stop::Full),
            Some(Ok(element)) => rows.push(to_json(&element)),
        }
    }
}

fn to_json(element: &ion_rs::Element) -> Value {
    use base64::Engine as _;
    use ion_rs::Value as V;
    let b64 = |b: &[u8]| Value::String(base64::engine::general_purpose::STANDARD.encode(b));
    match element.value() {
        V::Null(_) => Value::Null,
        V::Bool(b) => Value::Bool(*b),
        V::Int(i) => match i.as_i64() {
            Some(n) => Value::from(n),
            // Exact at any size (this crate's serde_json is arbitrary_precision).
            None => serde_json::from_str::<serde_json::Number>(&i.to_string())
                .map(Value::Number)
                .unwrap_or_else(|_| Value::String(i.to_string())),
        },
        V::Float(f) => float(*f),
        V::Decimal(d) => {
            // As a double, as Kestra's preview shows it (12.50 -> 12.5, 1d2 -> 100.0).
            // `Decimal`'s Display is Ion text (`-0.001`, `1d2`); `d` is the exponent.
            let text = d.to_string();
            match text.replace(['d', 'D'], "e").parse::<f64>() {
                Ok(f) => float(f),
                Err(_) => Value::String(text),
            }
        }
        V::Timestamp(t) => {
            let text = t.to_string();
            // ion-java writes a day-precision timestamp without its `T`.
            if t.precision() == ion_rs::TimestampPrecision::Day {
                Value::String(text.trim_end_matches('T').to_string())
            } else if let Some(utc) = text.strip_suffix("+00:00") {
                // ... and UTC as `Z` (`-00:00`, an unknown offset, stays).
                Value::String(format!("{utc}Z"))
            } else {
                Value::String(text)
            }
        }
        V::Symbol(s) => s
            .text()
            .map(|t| Value::String(t.to_string()))
            .unwrap_or(Value::Null),
        V::String(s) => Value::String(s.text().to_string()),
        V::Clob(b) | V::Blob(b) => b64(b.as_ref()),
        V::List(seq) | V::SExp(seq) => Value::Array(seq.elements().map(to_json).collect()),
        V::Struct(st) => {
            let mut map = serde_json::Map::new();
            for (name, value) in st.fields() {
                if value.is_null() {
                    continue;
                }
                map.insert(name.text().unwrap_or("").to_string(), to_json(value));
            }
            Value::Object(map)
        }
    }
}

fn float(f: f64) -> Value {
    if f.is_nan() {
        Value::String("NaN".into())
    } else if f.is_infinite() {
        Value::String(if f > 0.0 { "Infinity" } else { "-Infinity" }.into())
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::json;

    fn oracle() -> Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w9a-storage.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn b64(s: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(s).unwrap()
    }

    /// `file_api.ion_preview_source_text` (the text Ion Kestra previewed)
    /// gives `file_api.preview_maxRows_99999`'s rows, and `maxRows=2` its
    /// `preview_maxRows_2` rows, truncated.
    #[test]
    fn kestras_ion_preview_rows_are_reproduced_value_for_value() {
        let o = oracle();
        let fa = &o["egret_expected"]["file_api"];
        let text = fa["ion_preview_source_text"].as_str().unwrap();
        let all = fa["preview_maxRows_99999"]["body"]["content"].clone();
        let (rows, truncated) = values_to_json(text.as_bytes(), 5000).unwrap();
        assert_eq!(Value::Array(rows), all);
        assert!(!truncated);

        let two = fa["files"]["ion"]["preview_maxRows_2"]["body"]["content"].clone();
        let (rows, truncated) = values_to_json(text.as_bytes(), 2).unwrap();
        assert_eq!(Value::Array(rows), two);
        assert!(truncated);

        // Exactly as many values as asked for is not truncated.
        let (rows, truncated) = values_to_json(text.as_bytes(), 3).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(!truncated);
    }

    /// The decoded values of what each `ion_rows` row should preview as.
    fn expected_row(sql_type: &str) -> Value {
        match sql_type {
            "int" => json!({"v": 42}),
            "bigint" => json!({"v": 9007199254740993_u64}),
            "decimal" => json!({"v": 12.34}),
            "float" => json!({"v": 1.5}),
            "bool" => json!({"v": true}),
            "text_with_quote" => json!({"v": "say \"hi\""}),
            "timestamp" => json!({"v": "2026-01-02T03:04:05.123456"}),
            "timestamptz" => json!({"v": "2026-01-01T20:04:05Z[Etc/UTC]"}),
            "date" => json!({"v": "2026-01-02"}),
            "null" => json!({}),
            "bytea" => json!({"v": "3q2+7w=="}),
            other => panic!("no expectation for {other}"),
        }
    }

    /// `ion_rows`: Kestra's BINARY STORE rows decode to the same values as
    /// their `ion_text`, and to the expected preview value -- decoded values
    /// compared, never bytes.
    #[test]
    fn binary_store_rows_decode_as_their_text_does() {
        let o = oracle();
        let rows = o["egret_expected"]["ion_rows"].as_array().unwrap();
        assert_eq!(rows.len(), 11);
        for r in rows {
            let ty = r["sql_type"].as_str().unwrap();
            let bytes = b64(r["bytes_b64"].as_str().unwrap());
            assert_eq!(&bytes[..4], &[0xe0, 0x01, 0x00, 0xea], "{ty}: binary Ion");
            let (bin, t1) = values_to_json(&bytes[..], 100).unwrap();
            let (txt, t2) =
                values_to_json(r["ion_text"].as_str().unwrap().as_bytes(), 100).unwrap();
            assert_eq!(bin, txt, "{ty}: binary and text decode alike");
            assert_eq!(bin, vec![expected_row(ty)], "{ty}");
            assert!(!t1 && !t2);
        }
    }

    /// A STORE file is every row's bytes one after another, each with its own
    /// version marker and symbol table: read value by value, all 11 come back,
    /// and `max` stops after the asked number.
    #[test]
    fn a_binary_file_of_many_version_markers_reads_value_by_value() {
        let o = oracle();
        let rows = o["egret_expected"]["ion_rows"].as_array().unwrap();
        let mut file = Vec::new();
        let mut want = Vec::new();
        for r in rows {
            file.extend(b64(r["bytes_b64"].as_str().unwrap()));
            want.push(expected_row(r["sql_type"].as_str().unwrap()));
        }
        let (all, truncated) = values_to_json(&file[..], 100).unwrap();
        assert_eq!(all, want);
        assert!(!truncated);
        let (some, truncated) = values_to_json(&file[..], 4).unwrap();
        assert_eq!(some, want[..4].to_vec());
        assert!(truncated);
    }

    #[test]
    fn not_ion_is_an_error_and_the_values_before_it_are_kept_by_read_values() {
        assert!(values_to_json(&b"{a:1} {b:"[..], 10).is_err());
        let (rows, stop) = read_values(&b"{a:1} {b:"[..], 10);
        assert_eq!(rows, vec![json!({"a": 1})]);
        assert!(matches!(stop, Stop::Failed(_)), "{stop:?}");
    }

    /// A reader that fails after `n` bytes, as the preview's byte budget does.
    struct FailsAfter<'a>(&'a [u8], usize);
    impl Read for FailsAfter<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1 == 0 {
                return Err(std::io::Error::other("budget"));
            }
            let n = buf.len().min(self.1).min(self.0.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            self.1 -= n;
            Ok(n)
        }
    }

    /// A top-level text scalar cut by the budget is never a (wrong) value:
    /// `123456` cut after `12` must not preview as `12`.
    #[test]
    fn a_value_cut_by_a_failing_reader_is_not_read_as_a_shorter_one() {
        let (rows, stop) = read_values(FailsAfter(b"1 2 123456 7", 6), 10);
        assert_eq!(rows, vec![json!(1), json!(2)]);
        assert!(matches!(stop, Stop::Failed(_)), "{stop:?}");
    }

    #[test]
    fn scalars_lobs_annotations_and_special_floats() {
        let (rows, _) = values_to_json(
            &br#"null.int sym "s" {{aGVsbG8=}} {{"clob"}} a::b::5 nan +inf -inf 2025T 2025-12T 2025-12-03T 2025-12-03T12:00Z 2025-12-03T12:00:00-00:00 99999999999999999999999 -7 (1 2)"#[..],
            100,
        )
        .unwrap();
        assert_eq!(
            Value::Array(rows),
            json!([
                null,
                "sym",
                "s",
                "aGVsbG8=",
                "Y2xvYg==",
                5,
                "NaN",
                "Infinity",
                "-Infinity",
                "2025T",
                "2025-12T",
                "2025-12-03",
                "2025-12-03T12:00Z",
                "2025-12-03T12:00:00-00:00",
                serde_json::from_str::<Value>("99999999999999999999999").unwrap(),
                -7,
                [1, 2]
            ])
        );
    }
}
