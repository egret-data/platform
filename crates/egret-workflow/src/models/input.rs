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
    pub prefill: Option<Property<serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<DependsOn>,
    /// `ArrayInput.itemType` / `MultiselectInput.itemType` -- the element type
    /// of an `ARRAY` or `MULTISELECT` input.
    ///
    /// It lives on `Input` rather than on a per-type struct because this
    /// crate models `Input` as one flat shape keyed by `type`, not as Kestra's
    /// `@JsonSubTypes` hierarchy. That flattening is why the check that reads
    /// it (`spec::validate`'s `input_item_type_invalid`) has to gate on the
    /// input's own `type` being `ARRAY` or `MULTISELECT`: in Java the
    /// validator is attached to those two classes and can never see any other.
    ///
    /// `None` covers both "absent" and "this input type has no item type".
    /// Kestra gives `MultiselectInput` a `@Builder.Default` of `STRING` and
    /// `ArrayInput` none at all; neither default can fail the check, so the
    /// distinction changes nothing this crate reads and is not modelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_type: Option<InputType>,
    /// `FileInput.allowedFileExtensions` (`[".csv", ".txt"]`, each with its
    /// dot) -- a `FILE` input's file must end in one of them, compared
    /// lower-cased on both sides. Kept as written; the check is
    /// [`crate::storage::inputs::extension_allowed`].
    ///
    /// **egret enforces it and the artifact does not** (W9a spec §2, §7.1;
    /// `w9a-storage.json` `file_input.allowed_file_extensions`, basis
    /// `chosen`): Kestra 2.x accepted `bad.csv` for `[".txt"]`. Flat on
    /// `Input` for [`Self::item_type`]'s reason; `None` on every other type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_file_extensions: Option<Vec<String>>,
    /// `StringInput.validator` / `SecretInput.validator` -- a regular
    /// expression the value must match. Modelled only; enforced by a later
    /// package-#3 task. Flat on `Input` for [`Self::item_type`]'s reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validator: Option<String>,
    /// `IntInput` / `FloatInput` / `DurationInput` `min`: a JSON number
    /// (`INT`, `FLOAT`) or an ISO-8601 duration string (`DURATION`). Kept as
    /// written so a Kestra flow round-trips verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<serde_json::Value>,
    /// Upper bound; see [`Self::min`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<serde_json::Value>,
    /// `DateInput` / `DateTimeInput` / `TimeInput` `after`: the value must be
    /// later than this (an ISO date, datetime or time string, kept as written).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// The value must be earlier than this; see [`Self::after`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// `SelectInput` / `MultiselectInput` `values`: scalars or
    /// `{label, value}` objects, kept as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<serde_json::Value>>,
    /// `SelectInput` / `MultiselectInput` `expression`: a Pebble expression
    /// producing the option list instead of `values`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    /// `SelectInput` / `MultiselectInput` `allowCustomValue` (Kestra default
    /// `false`): a value outside the options is accepted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_custom_value: bool,
    /// `SelectInput.isRadio`: render as radio buttons. `None` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_radio: Option<bool>,
    /// `SelectInput` / `MultiselectInput` `autoSelectFirst`. `None` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_select_first: Option<bool>,
    // `JsonInput.jsonSchema` is deliberately NOT modelled (spec D12): no JSON
    // Schema validator is in the dependency tree, so egret would keep the key
    // and do nothing with it. Left out, it is `ignored` like every other key
    // egret drops: the flow schema does not offer it and a draft that writes
    // it gets the `not_honoured` warning (`spec::keys::lift_of`).
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

    /// `Input.java`'s base class declares `defaults` AND `prefill` as separate
    /// fields (`Property<T> defaults; Property<T> prefill;`), both between
    /// `required` and `displayName`. A round 1 review caught that a first pass
    /// ported only `defaults` -- with no `deny_unknown_fields` on `models/`,
    /// that meant a Kestra flow carrying `prefill:` parsed successfully and
    /// silently dropped the value on re-serialisation. This test fails the
    /// same way the omission would have, had it existed from the start.
    #[test]
    fn defaults_and_prefill_both_survive_round_trip() {
        let yaml = r#"
id: x
type: STRING
defaults: hello
prefill: world
"#;
        let i: Input = serde_norway::from_str(yaml).unwrap();
        assert_eq!(i.defaults.as_ref().unwrap().source(), "hello");
        assert_eq!(i.prefill.as_ref().unwrap().source(), "world");

        let json = serde_json::to_value(&i).unwrap();
        assert_eq!(json["defaults"], "hello");
        assert_eq!(json["prefill"], "world");
    }

    /// W9a Task 7: a FILE input's `allowedFileExtensions` parses in Kestra's
    /// spelling and survives re-serialisation as written.
    #[test]
    fn allowed_file_extensions_round_trips_in_kestra_spelling() {
        let yaml = "id: f\ntype: FILE\nallowedFileExtensions: [\".txt\", \".CSV\"]\n";
        let i: Input = serde_norway::from_str(yaml).unwrap();
        assert_eq!(
            i.allowed_file_extensions.as_deref(),
            Some(&[".txt".to_string(), ".CSV".to_string()][..])
        );
        let json = serde_json::to_value(&i).unwrap();
        assert_eq!(
            json["allowedFileExtensions"],
            serde_json::json!([".txt", ".CSV"])
        );
        let none: Input = serde_norway::from_str("id: f\ntype: FILE\n").unwrap();
        assert_eq!(none.allowed_file_extensions, None);
        assert!(serde_json::to_value(&none)
            .unwrap()
            .get("allowedFileExtensions")
            .is_none());
    }

    /// Package #3 A1: the validation keys parse in Kestra's spelling and
    /// re-serialise as written (numbers stay numbers, durations strings).
    #[test]
    fn validation_keys_round_trip_verbatim() {
        let cases = [
            "id: s\ntype: STRING\nvalidator: ^a+$\n",
            "id: n\ntype: INT\nmin: 1\nmax: 10\n",
            "id: f\ntype: FLOAT\nmin: 0.5\nmax: 2.5\n",
            "id: d\ntype: DURATION\nmin: PT1S\nmax: PT1H\n",
            "id: t\ntype: DATE\nafter: '2024-01-01'\nbefore: '2025-01-01'\n",
            "id: s\ntype: SELECT\nvalues:\n- a\n- 2\n- label: L\n  value: v\nexpression: '{{ x }}'\nallowCustomValue: true\nisRadio: true\nautoSelectFirst: false\n",
        ];
        for yaml in cases {
            let i: Input = serde_norway::from_str(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}"));
            let want: serde_json::Value = serde_norway::from_str(yaml).unwrap();
            let mut got = serde_json::to_value(&i).unwrap();
            got.as_object_mut().unwrap().remove("required");
            assert_eq!(got, want, "{yaml}");
        }
        let i: Input = serde_norway::from_str("id: n\ntype: INT\nmin: 1\n").unwrap();
        assert_eq!(i.min, Some(serde_json::json!(1)));
        assert!(i.max.is_none() && !i.allow_custom_value);
    }

    #[test]
    fn absent_validation_keys_are_not_serialised() {
        let i: Input = serde_norway::from_str("id: x\ntype: STRING\n").unwrap();
        let json = serde_json::to_value(&i).unwrap();
        for k in [
            "validator",
            "min",
            "max",
            "after",
            "before",
            "values",
            "expression",
            "allowCustomValue",
            "isRadio",
            "autoSelectFirst",
        ] {
            assert!(json.get(k).is_none(), "{k}");
        }
        assert!(!i.allow_custom_value);
    }
}
