// kestra: core/src/main/java/io/kestra/core/models/property/Property.java

use std::marker::PhantomData;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A plugin property: raw source that is rendered and converted to `T` at use
/// time (W2), never at parse time.
///
/// `T` is a marker only. It records what the field will become so callers read
/// as documentation and so W2's `render` can pick a conversion, but it holds no
/// value of that type -- Kestra's `Property<T>` works the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property<T> {
    source: String,
    _target: PhantomData<T>,
}

impl<T> Property<T> {
    pub fn from_source(source: String) -> Self {
        Self {
            source,
            _target: PhantomData,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

impl<T> Serialize for Property<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.source)
    }
}

impl<'de, T> Deserialize<'de> for Property<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Deserialize to an untyped value first, then flatten to source text
        // the way Kestra's PropertyDeserializer does: a scalar keeps its string
        // form, a sequence or mapping is re-serialised to JSON.
        let v = serde_json::Value::deserialize(d)?;
        let source = match v {
            serde_json::Value::String(s) => s,
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        };
        Ok(Self::from_source(source))
    }
}

#[cfg(test)]
mod tests {
    use super::Property;

    /// Kestra's PropertyDeserializer stores EVERYTHING as a string: scalars
    /// become their string form, and arrays/objects are re-serialised to JSON
    /// text. Reproducing that is the whole point of this type -- a Property is
    /// raw source, rendered and converted at use time (W2), not a parsed value.
    #[test]
    fn every_yaml_shape_becomes_source_text() {
        let scalar: Property<String> = serde_norway::from_str("hello").unwrap();
        assert_eq!(scalar.source(), "hello");

        let number: Property<i64> = serde_norway::from_str("42").unwrap();
        assert_eq!(number.source(), "42");

        let expression: Property<String> = serde_norway::from_str("\"{{ inputs.x }}\"").unwrap();
        assert_eq!(expression.source(), "{{ inputs.x }}");

        // A sequence is re-serialised to JSON, exactly as Java does.
        let list: Property<Vec<String>> = serde_norway::from_str("[a, b]").unwrap();
        assert_eq!(list.source(), r#"["a","b"]"#);

        let map: Property<serde_json::Value> = serde_norway::from_str("{k: v}").unwrap();
        assert_eq!(map.source(), r#"{"k":"v"}"#);
    }

    /// Round-tripping must not quote a source that was not quoted, or a
    /// published snapshot would stop matching the author's YAML.
    #[test]
    fn serialises_back_to_its_source() {
        let p: Property<String> = serde_norway::from_str("hello").unwrap();
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"hello\"");
    }
}
