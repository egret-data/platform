//! W7a Task 1 Step 2: how egret treats each property Kestra documents, for
//! every type egret accepts that has a Kestra definition, and for the flow
//! document itself. The result is `property-treatment.json`, which
//! [`super::filter_properties`] serves from; the test below re-measures it on
//! every run and fails when the committed table no longer matches, so a
//! handler that starts (or stops) reading a property cannot leave the schema
//! behind. `EGRET_WRITE_PROPERTY_TREATMENT=1` rewrites the file instead.
//!
//! # The instrument, per property
//!
//! 1. **What egret reads** -- the *read set*:
//!    * the base task properties `models::Task`'s own deserializer takes
//!      ([`crate::plugins::HANDLED_BY_TASK_MODEL`]);
//!    * for a core leaf task, the fields of the struct its handler
//!      deserializes the leaf spec into, listed by serde itself
//!      ([`fields_of`]). None of those structs denies unknown fields, so a
//!      property outside the list is dropped by serde without a word;
//!    * for a flowable or a trigger, the fields of its typed model struct
//!      (same instrument), plus, for a trigger, the base fields with a
//!      consumer outside `models` (see [`TRIGGER_BASE_READ`]);
//!    * for a script task, `ScriptKind::implemented_properties`, and for a
//!      plugin class, its catalog properties minus the refused base ones --
//!      the plugin host runs Kestra's own code over them;
//!    * for the flow document, the fields of `models::Flow`.
//! 2. **What parse + validate say** -- a minimal flow carrying the property
//!    (with a sample value) is parsed and validated, and compared with the
//!    same flow without it. A parse failure or a new error refuses it; a new
//!    `not_yet_enforced` warning means egret parses it, does not act on it,
//!    and says so.
//!
//! | parse + validate | in the read set | treatment |
//! |---|---|---|
//! | parse fails, or a new error | no | `refused` |
//! | new `not_yet_enforced` warning | either | `warned` |
//! | clean | yes | `honoured` |
//! | clean | no | `ignored` |
//!
//! A property in the read set whose sample fails to parse stays `honoured`
//! and is printed, so a bad sample is visible rather than silently turning a
//! read property into a refused one.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::{self, DeserializeOwned, Visitor};
use serde_json::{json, Value};

use super::{Treatment, TreatmentTable, FLOWABLE_TYPES, FLOW_ROW, TRIGGER_TYPES};
use crate::models;
use crate::spec::{parse, registry, validate};
use crate::worker::handlers as h;

// ---------- serde's own field list ----------

#[derive(Debug)]
struct Captured(Option<&'static [&'static str]>);

impl std::fmt::Display for Captured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
impl std::error::Error for Captured {}
impl de::Error for Captured {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Captured(None)
    }
}

/// A deserializer that answers nothing and records the field list a derived
/// `Deserialize` hands to `deserialize_struct`.
struct FieldProbe;

impl<'de> de::Deserializer<'de> for FieldProbe {
    type Error = Captured;
    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Captured> {
        Err(Captured(None))
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Captured> {
        Err(Captured(Some(fields)))
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

/// The wire names of `T`'s fields, as its derived `Deserialize` knows them.
fn fields_of<T: DeserializeOwned>() -> Vec<&'static str> {
    match T::deserialize(FieldProbe) {
        Err(Captured(Some(fields))) => fields.to_vec(),
        _ => panic!(
            "{} does not deserialize as a struct",
            std::any::type_name::<T>()
        ),
    }
}

// ---------- the read sets ----------

/// The base trigger fields `models::Trigger`'s deserializer takes AND
/// something outside `models` reads. Measured by searching `src/` outside
/// `models/` for each field (`grep -rn '\.<field>\b'`): `when`, `labels`,
/// `disabled`, `id`, `type`, `description` have readers; `stop_after`,
/// `log_to_file`, `fail_on_trigger_error` and `allow_concurrent` have none
/// (the model's own doc says so of `stopAfter`), so those four are not here.
const TRIGGER_BASE_READ: &[&str] = &["description", "disabled", "id", "labels", "type", "when"];

/// Properties egret reads and that, by Kestra's own definition, do nothing:
/// the script tasks' `warningOnStdErr` ("Not used anymore, will be removed
/// soon"; W5a Task 1 measured no effect with `true`, `false` or unset).
const NO_EFFECT: &[&str] = &["warningOnStdErr"];

fn task_model() -> BTreeSet<&'static str> {
    crate::plugins::HANDLED_BY_TASK_MODEL
        .iter()
        .copied()
        .collect()
}

/// The fields a core leaf task's handler deserializes, or `None` for a type
/// with no such handler.
fn leaf_handler_fields(t: &str) -> Option<Vec<&'static str>> {
    use h::storage::items;
    Some(match t {
        h::log::TYPE_ID => fields_of::<h::log::Spec>(),
        h::ret::TYPE_ID => fields_of::<h::ret::Spec>(),
        h::fail::TYPE_ID => fields_of::<h::fail::Spec>(),
        h::sleep::TYPE_ID => fields_of::<h::sleep::Spec>(),
        h::storage::WRITE => fields_of::<h::storage::WriteSpec>(),
        h::storage::SIZE => fields_of::<h::storage::UriSpec>(),
        h::storage::DELETE => fields_of::<h::storage::DeleteSpec>(),
        // Reads no property: it purges the running execution's own files.
        h::storage::PURGE_CURRENT_EXECUTION_FILES => Vec::new(),
        h::storage::CONCAT => fields_of::<items::ConcatSpec>(),
        h::storage::SPLIT => fields_of::<items::SplitSpec>(),
        h::storage::REVERSE => fields_of::<items::ReverseSpec>(),
        h::storage::DEDUPLICATE_ITEMS => fields_of::<items::DeduplicateSpec>(),
        h::storage::FILTER_ITEMS => fields_of::<items::FilterSpec>(),
        h::namespace::UPLOAD_FILES => fields_of::<h::namespace::UploadSpec>(),
        h::namespace::DOWNLOAD_FILES => fields_of::<h::namespace::DownloadSpec>(),
        h::namespace::DELETE_FILES => fields_of::<h::namespace::DeleteSpec>(),
        h::namespace::PURGE_FILES => fields_of::<h::namespace::PurgeSpec>(),
        h::kv::SET => fields_of::<h::kv::SetSpec>(),
        h::kv::GET => fields_of::<h::kv::GetSpec>(),
        h::kv::GET_KEYS => fields_of::<h::kv::GetKeysSpec>(),
        h::kv::DELETE => fields_of::<h::kv::DeleteSpec>(),
        h::kv::PUT => fields_of::<h::kv::PutSpec>(),
        h::kv::PURGE_KV => fields_of::<h::kv::PurgeSpec>(),
        _ => return None,
    })
}

fn flowable_fields(t: &str) -> Vec<&'static str> {
    match t {
        "io.kestra.plugin.core.flow.Sequential" => fields_of::<models::Sequential>(),
        "io.kestra.plugin.core.flow.Parallel" => fields_of::<models::Parallel>(),
        "io.kestra.plugin.core.flow.If" => fields_of::<models::If>(),
        "io.kestra.plugin.core.flow.Switch" => fields_of::<models::Switch>(),
        "io.kestra.plugin.core.flow.Dag" => fields_of::<models::Dag>(),
        "io.kestra.plugin.core.flow.Loop" => fields_of::<models::Loop>(),
        "io.kestra.plugin.core.flow.LoopUntil" => fields_of::<models::LoopUntil>(),
        "io.kestra.plugin.core.flow.Subflow" => fields_of::<models::Subflow>(),
        "io.kestra.plugin.core.flow.AllowFailure" => fields_of::<models::AllowFailure>(),
        "io.kestra.plugin.core.flow.WorkingDirectory" => fields_of::<models::WorkingDirectory>(),
        "io.kestra.plugin.core.flow.Pause" => fields_of::<models::Pause>(),
        other => panic!("no typed model for flowable {other}"),
    }
}

fn trigger_fields(t: &str) -> Vec<&'static str> {
    match t {
        "io.kestra.plugin.core.trigger.Schedule" => fields_of::<models::Schedule>(),
        "io.kestra.plugin.core.trigger.Webhook" => fields_of::<models::Webhook>(),
        "io.kestra.plugin.core.trigger.Flow" => fields_of::<models::FlowTrigger>(),
        other => panic!("no typed model for trigger {other}"),
    }
}

// ---------- parse + validate ----------

#[derive(Debug, PartialEq)]
struct Outcome {
    parsed: bool,
    errors: BTreeSet<(String, String)>,
    not_yet_enforced: BTreeSet<String>,
}

fn run(flow: &Value) -> Outcome {
    let source = serde_json::to_string(flow).expect("a Value serialises");
    match parse(&source) {
        Err(_) => Outcome {
            parsed: false,
            errors: BTreeSet::new(),
            not_yet_enforced: BTreeSet::new(),
        },
        Ok(f) => {
            let r = validate(&f);
            Outcome {
                parsed: true,
                errors: r
                    .errors
                    .iter()
                    .map(|e| (e.code.to_string(), e.path.clone()))
                    .collect(),
                not_yet_enforced: r
                    .warnings
                    .iter()
                    .filter(|w| w.code == "not_yet_enforced")
                    .map(|w| w.path.clone())
                    .collect(),
            }
        }
    }
}

fn log_task(id: &str) -> Value {
    json!({ "id": id, "type": "io.kestra.plugin.core.log.Log", "message": "m" })
}

/// A value for `prop` that egret's model can hold. Hand-picked where a
/// property's warning only fires on a meaningful value; otherwise by the
/// Kestra schema's type.
fn sample(row: &str, prop: &str, schema: &Value) -> Value {
    match (row, prop) {
        (FLOW_ROW, "concurrency") => return json!({ "limit": 1 }),
        (FLOW_ROW, "sla") => {
            return json!([{ "id": "s", "type": "MAX_DURATION", "behavior": "FAIL", "duration": "PT1H" }])
        }
        (FLOW_ROW, "checks") => return json!([{ "when": "{{ true }}", "message": "m" }]),
        (FLOW_ROW, "quotas") => {
            return json!([{ "duration": "PT1H", "limit": 1, "behavior": "FAIL" }])
        }
        (FLOW_ROW, "triggers") => {
            return json!([{ "id": "s", "type": "io.kestra.plugin.core.trigger.Schedule", "cron": "0 * * * *" }])
        }
        (FLOW_ROW, "tasks" | "errors" | "finally" | "afterExecution") => {
            return json!([log_task("x")])
        }
        ("io.kestra.plugin.core.flow.Dag", "tasks") => return json!([{ "task": log_task("x") }]),
        (_, "namespaceFiles") => return json!({ "enabled": true }),
        (_, "tasks" | "errors" | "finally" | "then" | "else" | "defaults") => {
            return json!([log_task("x")])
        }
        ("io.kestra.plugin.core.flow.Pause", "onPause") => return log_task("x"),
        ("io.kestra.plugin.core.flow.Switch", "cases") => return json!({ "a": [log_task("x")] }),
        ("io.kestra.plugin.core.flow.LoopUntil", "checkFrequency") => {
            return json!({ "maxIterations": 1 })
        }
        (FLOW_ROW, "inputs") => return json!([{ "id": "i", "type": "STRING" }]),
        (FLOW_ROW, "outputs") => return json!([{ "id": "o", "type": "STRING", "value": "x" }]),
        ("io.kestra.plugin.core.trigger.Webhook", "fetchType") => return json!("FETCH"),
        ("io.kestra.plugin.core.trigger.Flow", "states") => return json!(["SUCCESS"]),
        ("io.kestra.plugin.core.trigger.Flow", "mode") => return json!("ANY"),
        ("io.kestra.plugin.core.trigger.Flow", "dependsOn") => {
            return json!([{ "namespace": "a.b", "flowId": "f" }])
        }
        ("io.kestra.plugin.core.trigger.Flow", "window") => return json!({ "lookback": "PT1H" }),
        (_, "stopAfter") => return json!(["SUCCESS"]),
        (_, "labels") => return json!({ "k": "v" }),
        (_, "retry") => return json!({ "type": "constant", "interval": "PT1S", "maxAttempts": 1 }),
        ("io.kestra.plugin.core.flow.Pause", "behavior") => return json!("RESUME"),
        _ => {}
    }
    let ty = schema
        .get("type")
        .or_else(|| schema.pointer("/allOf/0/type"))
        .or_else(|| schema.pointer("/anyOf/0/type"))
        .and_then(Value::as_str);
    match ty {
        Some("boolean") => json!(true),
        Some("integer") | Some("number") => json!(1),
        Some("array") => json!(["x"]),
        Some("object") => json!({ "x": "x" }),
        _ => json!("x"),
    }
}

/// A minimal flow for `row`, carrying `extra` when given.
fn flow_for(row: &str, kind: Kind, extra: Option<(&str, &Value)>) -> Value {
    let mut node = json!({ "id": "t", "type": row });
    let mut flow = json!({ "id": "m", "namespace": "m.t", "tasks": [log_task("base")] });
    let target = match kind {
        Kind::Flow => &mut flow,
        Kind::Task | Kind::Trigger => &mut node,
    };
    if let Some((p, v)) = extra {
        target[p] = v.clone();
    }
    match kind {
        Kind::Flow => {}
        Kind::Task => flow["tasks"] = json!([node]),
        Kind::Trigger => flow["triggers"] = json!([node]),
    }
    flow
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Task,
    Trigger,
    Flow,
}

fn classify(
    row: &str,
    kind: Kind,
    props: &BTreeMap<String, Value>,
    read: &BTreeSet<&str>,
    notes: &mut Vec<String>,
) -> BTreeMap<String, Treatment> {
    let baseline = run(&flow_for(row, kind, None));
    let mut out = BTreeMap::new();
    for (prop, schema) in props {
        // The identity keys: replacing them changes every finding's path.
        if kind != Kind::Flow && (prop == "type" || prop == "id") {
            out.insert(prop.clone(), Treatment::Honoured);
            continue;
        }
        let value = sample(row, prop, schema);
        let o = run(&flow_for(row, kind, Some((prop, &value))));
        let new_errors: Vec<_> = o.errors.difference(&baseline.errors).collect();
        let warned = o
            .not_yet_enforced
            .difference(&baseline.not_yet_enforced)
            .next()
            .is_some();
        let is_read = read.contains(prop.as_str());
        let t = if !o.parsed || !new_errors.is_empty() {
            if is_read {
                notes.push(format!(
                    "{row}.{prop}: read, but the sample {value} did not pass ({}; {new_errors:?})",
                    if o.parsed { "parsed" } else { "parse failed" }
                ));
                Treatment::Honoured
            } else {
                Treatment::Refused
            }
        } else if warned {
            Treatment::Warned
        } else if is_read {
            Treatment::Honoured
        } else {
            Treatment::Ignored
        };
        let t = if NO_EFFECT.contains(&prop.as_str()) && t == Treatment::Honoured {
            Treatment::Ignored
        } else {
            t
        };
        out.insert(prop.clone(), t);
    }
    out
}

fn kestra_props(fqcn: &str) -> BTreeMap<String, Value> {
    super::kestra_definition(fqcn)
        .and_then(|d| d.get("properties"))
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// The whole measurement.
pub(super) fn measure(notes: &mut Vec<String>) -> TreatmentTable {
    let mut table = TreatmentTable::new();
    let base = task_model();

    for row in registry::TASK_TYPES {
        let props = kestra_props(row.r#type);
        if props.is_empty() {
            continue; // egret-only: hand-written, nothing of Kestra's to classify
        }
        let mut read = base.clone();
        if let Some(kind) = crate::scripts::kind_of(row.r#type) {
            read.extend(kind.implemented_properties());
        } else {
            let fields = leaf_handler_fields(row.r#type)
                .unwrap_or_else(|| panic!("{}: no handler field list", row.r#type));
            read.extend(fields);
        }
        table.insert(
            row.r#type.to_string(),
            classify(row.r#type, Kind::Task, &props, &read, notes),
        );
    }

    for (fqcn, class) in &crate::plugins::catalog().classes {
        let name = format!("{}{fqcn}", crate::plugins::PREFIX);
        let mut props = kestra_props(fqcn);
        for (p, spec) in &class.properties {
            props
                .entry(p.clone())
                .or_insert_with(|| spec.schema.clone());
        }
        let mut read = base.clone();
        read.extend(class.properties.keys().map(String::as_str).filter(|p| {
            !crate::plugins::REFUSED_BASE_PROPERTIES.contains(p) && *p != "taskRunner"
        }));
        table.insert(
            name.clone(),
            classify(&name, Kind::Task, &props, &read, notes),
        );
    }

    for (fqcn, _) in FLOWABLE_TYPES {
        let mut read = base.clone();
        read.extend(flowable_fields(fqcn));
        table.insert(
            fqcn.to_string(),
            classify(fqcn, Kind::Task, &kestra_props(fqcn), &read, notes),
        );
    }

    for (fqcn, _) in TRIGGER_TYPES {
        let mut read: BTreeSet<&str> = TRIGGER_BASE_READ.iter().copied().collect();
        read.extend(trigger_fields(fqcn));
        table.insert(
            fqcn.to_string(),
            classify(fqcn, Kind::Trigger, &kestra_props(fqcn), &read, notes),
        );
    }

    let flow_props: BTreeMap<String, Value> = super::INPUTS
        .kestra_flow
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let read: BTreeSet<&str> = fields_of::<models::Flow>().into_iter().collect();
    table.insert(
        FLOW_ROW.to_string(),
        classify(FLOW_ROW, Kind::Flow, &flow_props, &read, notes),
    );
    table
}

#[test]
fn the_committed_property_treatment_is_what_egret_does_today() {
    let mut notes = Vec::new();
    let measured = measure(&mut notes);
    for n in &notes {
        eprintln!("note: {n}");
    }
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/spec/schema/property-treatment.json"
    );
    if std::env::var_os("EGRET_WRITE_PROPERTY_TREATMENT").is_some() {
        let mut text = serde_json::to_string_pretty(&measured).unwrap();
        text.push('\n');
        std::fs::write(path, text).unwrap();
        return;
    }
    let committed = super::property_treatment();
    let mut diffs = Vec::new();
    for (row, props) in &measured {
        for (p, t) in props {
            let c = committed.get(row).and_then(|r| r.get(p));
            if c != Some(t) {
                diffs.push(format!("{row}.{p}: measured {t:?}, committed {c:?}"));
            }
        }
    }
    for (row, props) in committed {
        for p in props.keys() {
            if measured.get(row).and_then(|r| r.get(p)).is_none() {
                diffs.push(format!("{row}.{p}: committed, no longer measured"));
            }
        }
    }
    assert!(
        diffs.is_empty(),
        "property-treatment.json is stale; re-run with EGRET_WRITE_PROPERTY_TREATMENT=1 and \
         review the diff:\n{}",
        diffs.join("\n")
    );
}

/// The instrument itself: serde's field list is the renamed wire names, and
/// a property outside it really is dropped by the handler's struct.
#[test]
fn the_field_probe_reads_wire_names_and_unknown_keys_are_dropped() {
    let fields = fields_of::<h::fail::Spec>();
    assert_eq!(fields, vec!["condition", "errorMessage"]);
    let spec: h::fail::Spec =
        serde_json::from_value(json!({ "errorMessage": "x", "logLevel": "INFO" }))
            .expect("an unknown key is dropped, not refused");
    assert!(format!("{spec:?}").contains("error_message"));
}
