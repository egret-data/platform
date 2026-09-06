use std::collections::BTreeSet;
use std::fmt;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Serialize;

use crate::models::Flow;

/// A YAML syntax or shape error, located.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParseError {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// Parse a flow's YAML source into the model.
///
/// `serde_norway::Error::location()` is the reason that crate was chosen over a
/// bare YAML parser: an editor needs to put the squiggle somewhere.
///
/// Two passes, and the order is deliberate. [`DuplicateKeyProbe`] runs FIRST,
/// because a mapping that writes the same key twice deserialises perfectly —
/// the last value silently wins — so by the time the `Flow` exists the evidence
/// is gone. Kestra refuses the same source with
/// `Illegal genericflow source: YAML parsing error: Duplicate field 'message'`
/// (`invalids/duplicate-key`, measured at `v2.0.0-rc13`), and the probe reports
/// the same key with the same line and column serde_norway would have given for
/// any other shape error.
pub fn parse(source: &str) -> Result<Flow, ParseError> {
    serde_norway::from_str::<DuplicateKeyProbe>(source).map_err(located)?;
    serde_norway::from_str::<Flow>(source).map_err(located)
}

fn located(e: serde_norway::Error) -> ParseError {
    let loc = e.location();
    ParseError {
        message: e.to_string(),
        line: loc.as_ref().map(|l| l.line()),
        column: loc.as_ref().map(|l| l.column()),
    }
}

/// A whole-document walk that carries no data and answers exactly one
/// question: does any mapping in this YAML declare the same key twice?
///
/// **It has to be a second pass over the source, not a check inside `Flow`'s
/// own deserialisation.** A flow's plugin properties land in
/// `TaskKind::Leaf { spec: serde_json::Value }`, and a `Value`'s map takes the
/// last of a repeated key without complaint; serde's derived "duplicate field"
/// error only ever fires for a *named* field of a struct, which those are not.
/// So the only place both entries are still visible is the deserializer's own
/// `MapAccess`, which yields one `(key, value)` pair per YAML mapping entry
/// including the repeats.
///
/// The cost is one extra parse of the source. That is paid on the publish path
/// (`routes.rs`'s `create_flow`/`update_flow`/`validate_flow`) and in
/// `build_plan`, neither of which is on `advance()`'s path — `advance()` takes
/// an already-built [`Plan`].
///
/// **Recursion:** `visit_map`/`visit_seq` descend through the document, so the
/// depth is the YAML's own nesting depth rather than anything derived from a
/// task tree. serde_norway's own deserializer recurses to the same depth to
/// produce the events this visitor consumes, so this pass adds a constant
/// factor to a descent the parser already performs and cannot be made to
/// overflow by an input the parser itself survives.
struct DuplicateKeyProbe;

impl<'de> de::Deserialize<'de> for DuplicateKeyProbe {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(DuplicateKeyProbe)
    }
}

impl<'de> Visitor<'de> for DuplicateKeyProbe {
    type Value = DuplicateKeyProbe;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        // Keys are read as `String`. A YAML mapping may be keyed by anything,
        // and a non-string key errors here rather than being skipped -- which
        // is safe for this crate's purpose because every key `Flow` reads is a
        // string, so a document with a non-string key was never going to parse
        // as a `Flow` anyway. The second pass reports it with serde_norway's
        // own message.
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate field '{key}'")));
            }
            map.next_value::<DuplicateKeyProbe>()?;
        }
        Ok(DuplicateKeyProbe)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<DuplicateKeyProbe>()?.is_some() {}
        Ok(DuplicateKeyProbe)
    }

    fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
        Ok(DuplicateKeyProbe)
    }
    fn visit_string<E>(self, _: String) -> Result<Self::Value, E> {
        Ok(DuplicateKeyProbe)
    }
    fn visit_bytes<E>(self, _: &[u8]) -> Result<Self::Value, E> {
        Ok(DuplicateKeyProbe)
    }
    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
        Ok(DuplicateKeyProbe)
    }
    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
        Ok(DuplicateKeyProbe)
    }
    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
        Ok(DuplicateKeyProbe)
    }
    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
        Ok(DuplicateKeyProbe)
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(DuplicateKeyProbe)
    }
    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(DuplicateKeyProbe)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(DuplicateKeyProbe)
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    /// The same key twice inside a task's plugin properties.
    ///
    /// `message` is not a named field of any Rust struct here — it lands in
    /// `TaskKind::Leaf`'s opaque `spec` — so nothing in `Flow`'s own
    /// deserialisation could ever have reported it. Without the probe this
    /// flow parses, plans, and runs with `b`.
    #[test]
    fn a_duplicate_key_in_a_leafs_plugin_properties_is_refused_with_its_line() {
        let yaml = "id: duplicate-key\nnamespace: io.kestra.tests\n\ntasks:\n  \
                    - id: bad-task\n    type: io.kestra.plugin.core.log.Log\n    \
                    message: Hello World\n    message: Hello World\n";
        let err = parse(yaml).expect_err("a duplicate key must be refused");
        assert!(
            err.message.contains("duplicate field 'message'"),
            "the message must name the key: {}",
            err.message
        );
        assert_eq!(
            (err.line, err.column),
            (Some(5), Some(5)),
            "a duplicate key is located like any other shape error"
        );
    }

    /// The same key twice at the top level of the document, where the field
    /// IS a named field of `Flow` — so that the probe is shown to fire on
    /// both shapes and not only on the opaque one.
    #[test]
    fn a_duplicate_key_at_the_flow_level_is_refused_too() {
        let yaml = "id: a\nnamespace: io.kestra.tests\nid: b\ntasks:\n  - id: t\n    \
                    type: io.kestra.plugin.core.log.Log\n";
        let err = parse(yaml).expect_err("a duplicate flow-level key must be refused");
        assert!(
            err.message.contains("duplicate field 'id'"),
            "got {}",
            err.message
        );
    }

    /// The same key in two DIFFERENT mappings is not a duplicate.
    ///
    /// The `seen` set is per-mapping, and this is what says so: every task in
    /// a flow writes `id` and `type`, so a probe that pooled keys across
    /// mappings would refuse every flow ever written.
    #[test]
    fn the_same_key_in_two_sibling_mappings_is_not_a_duplicate() {
        let yaml = "id: a\nnamespace: io.kestra.tests\ntasks:\n  - id: t1\n    \
                    type: io.kestra.plugin.core.log.Log\n    message: x\n  - id: t2\n    \
                    type: io.kestra.plugin.core.log.Log\n    message: y\n";
        let flow = parse(yaml).expect("two tasks may both write `id`, `type` and `message`");
        assert_eq!(flow.tasks.len(), 2);
    }
}
