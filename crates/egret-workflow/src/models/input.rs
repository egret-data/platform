// kestra: core/src/main/java/io/kestra/core/models/flows/Input.java
// kestra: core/src/main/java/io/kestra/core/models/flows/input/

use serde::{Deserialize, Serialize};

use crate::models::Property;

/// Kestra's `inputs:` type vocabulary, spelled exactly as Kestra spells it.
///
/// Ported from `Input.java`'s `@JsonSubTypes` list (the wire-spelling
/// authority), not from the `input/` directory's class names alone: the
/// boolean type is registered there as `BOOL`, not `BOOLEAN`, so the variant
/// keeps a readable Rust name (`Boolean`) with an explicit rename to match.
/// `Datetime` is a single Rust word (not `DateTime`) so the default
/// `SCREAMING_SNAKE_CASE` conversion already produces `DATETIME`, matching
/// Kestra, rather than `DATE_TIME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InputType {
    String,
    Int,
    Float,
    #[serde(rename = "BOOL")]
    Boolean,
    Datetime,
    Date,
    Time,
    Duration,
    File,
    Json,
    Yaml,
    Uri,
    Secret,
    Select,
    Multiselect,
    Array,
    Email,
    Ion,
    Form,
    ReusableInputs,
}

/// `dependsOn` — an input shown only when other inputs are filled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependsOn {
    pub inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub id: String,
    pub r#type: InputType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<Property<serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<DependsOn>,
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{Input, InputType};

    /// Kestra spells input types in caps. A flow pasted from Kestra must parse,
    /// which is the whole reason this vocabulary was ported instead of reusing
    /// egret's own six-variant ParamType.
    #[test]
    fn parses_a_kestra_input_block_verbatim() {
        let yaml = r#"
id: run_date
type: DATETIME
description: The day to roll up
required: true
"#;
        let i: Input = serde_norway::from_str(yaml).unwrap();
        assert_eq!(i.id, "run_date");
        assert_eq!(i.r#type, InputType::Datetime);
        assert!(i.required);
    }

    /// The type list and wire spellings, reconciled against
    /// `io.kestra.core.models.flows.Input`'s `@JsonSubTypes`: that annotation,
    /// not this crate's guess, is authoritative. Two corrections from a first
    /// pass: the boolean type is spelled `BOOL` there, not `BOOLEAN`; and
    /// `ION`, `FORM`, `REUSABLE_INPUTS` are registered subtypes that a first
    /// pass omitted (`REUSABLE_INPUTS` is `@EeOnly` -- hidden from the
    /// open-source flow schema -- but still a discriminator the deserializer
    /// accepts, so it is ported like the rest).
    #[test]
    fn every_type_round_trips_in_kestra_spelling() {
        for (spelling, variant) in [
            ("STRING", InputType::String),
            ("INT", InputType::Int),
            ("FLOAT", InputType::Float),
            ("BOOL", InputType::Boolean),
            ("DATETIME", InputType::Datetime),
            ("DATE", InputType::Date),
            ("TIME", InputType::Time),
            ("DURATION", InputType::Duration),
            ("FILE", InputType::File),
            ("JSON", InputType::Json),
            ("YAML", InputType::Yaml),
            ("URI", InputType::Uri),
            ("SECRET", InputType::Secret),
            ("SELECT", InputType::Select),
            ("MULTISELECT", InputType::Multiselect),
            ("ARRAY", InputType::Array),
            ("EMAIL", InputType::Email),
            ("ION", InputType::Ion),
            ("FORM", InputType::Form),
            ("REUSABLE_INPUTS", InputType::ReusableInputs),
        ] {
            let parsed: InputType =
                serde_norway::from_str(spelling).unwrap_or_else(|e| panic!("{spelling}: {e}"));
            assert_eq!(parsed, variant, "{spelling}");
            assert_eq!(
                serde_json::to_string(&variant).unwrap(),
                format!("\"{spelling}\"")
            );
        }
    }

    /// `required` defaults to TRUE in Kestra. Getting this backwards would make
    /// every optional input silently mandatory, and no other test would notice.
    #[test]
    fn required_defaults_to_true() {
        let i: Input = serde_norway::from_str("id: x\ntype: STRING\n").unwrap();
        assert!(i.required);
    }
}
