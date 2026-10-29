//! `type: SECRET` flow inputs (KI-112): the value a run needs, kept out of
//! everything a person or a client can read.
//!
//! # Where the value lives
//!
//! `workflow.executions.inputs` is what every read of an execution returns
//! (the execution API, the executions list, the UI's Overview tab), so a
//! SECRET input's value is never written there: [`split`] replaces it with
//! [`MASK`] and seals the real values, as one JSON object, into
//! `workflow.executions.secret_inputs` (migration 0203). The seal is
//! `egret_secrets::Crypto`, AES-256-GCM under `secrets.master_key`, the key
//! the workspace secret store already uses; the column is `nonce || ciphertext`.
//! No projection of `ExecutionRepo` selects it.
//!
//! Only the render surface ([`reveal`], from `worker::render::Surface::load`)
//! opens it, so a task's `{{ inputs.x }}` renders the real value. The same
//! values are then masked in every log line the run writes and in a failed
//! run's message (`RenderContext::redactions`).
//!
//! # What copies the sealed column
//!
//! Every writer that copies an execution's `inputs` copies `secret_inputs`
//! beside it: a `Loop` iteration (`worker::spawn`), a `CREATE_NEW_EXECUTION`
//! retry (`worker::replay`) and the replay route (`NewExecution::secret_inputs`).
//! A `Subflow` child is a different flow: its rendered inputs go through
//! [`split`] against the CHILD's declaration.
//!
//! # A deployment without a master key
//!
//! Has no cipher installed. A start that would store a SECRET value is
//! refused, naming the input and the setting, rather than storing it in clear.
//! Kestra refuses the same start ("encryption is not configured").
//!
//! # What is not masked
//!
//! A task's `outputs` (a `Return` whose `format:` is the input returns it), as
//! the `secret()` function's values are not (W5b spec §12, as Kestra): the
//! author chose to publish it.

use std::sync::OnceLock;

use egret_core::{Error, Result};
use egret_secrets::Crypto;
use serde_json::{Map, Value};

use crate::models::{Flow, InputType};

/// What a SECRET input reads as wherever it is shown: the run log's mask.
pub use crate::logs::MASK;

const NONCE_BYTES: usize = 12;

static CIPHER: OnceLock<Crypto> = OnceLock::new();

/// Process-wide, set once by egret-bin from `secrets.master_key`. `false`
/// when one was already installed; the first stays.
pub fn install(cipher: Crypto) -> bool {
    CIPHER.set(cipher).is_ok()
}

/// Idempotent: a fixed test key. The first caller in a test binary installs it.
#[cfg(any(test, feature = "test-util"))]
pub fn install_for_tests() {
    CIPHER.get_or_init(|| Crypto::new(&[7u8; 32]).expect("a 32-byte key"));
}

/// The ids of `flow`'s inputs declared `type: SECRET`.
fn secret_ids(flow: &Flow) -> impl Iterator<Item = &str> {
    flow.inputs
        .iter()
        .filter(|i| i.r#type == InputType::Secret)
        .map(|i| i.id.as_str())
}

/// `inputs` with every SECRET input's value replaced by [`MASK`], and the
/// values themselves sealed for `workflow.executions.secret_inputs`.
///
/// `None` when no SECRET input has a value (an absent or `null` one stays as
/// it is: there is nothing to hide). A non-object `inputs` is returned as is.
pub fn split(flow: &Flow, inputs: Value) -> Result<(Value, Option<Vec<u8>>)> {
    split_with(flow, inputs, CIPHER.get())
}

fn split_with(
    flow: &Flow,
    inputs: Value,
    cipher: Option<&Crypto>,
) -> Result<(Value, Option<Vec<u8>>)> {
    let Value::Object(mut posted) = inputs else {
        return Ok((inputs, None));
    };
    let mut secret = Map::new();
    for id in secret_ids(flow) {
        if let Some(v) = posted.get_mut(id) {
            if !v.is_null() {
                secret.insert(id.to_string(), std::mem::replace(v, Value::from(MASK)));
            }
        }
    }
    if secret.is_empty() {
        return Ok((Value::Object(posted), None));
    }
    let Some(cipher) = cipher else {
        let names: Vec<&str> = secret.keys().map(String::as_str).collect();
        return Err(Error::InvalidInput(format!(
            "the flow declares SECRET input(s) {}, and this deployment cannot store a secret \
             value: set secrets.master_key",
            names.join(", ")
        )));
    };
    let plaintext = Value::Object(secret).to_string();
    let (nonce, ct) = cipher
        .seal(&plaintext)
        .map_err(|e| Error::Internal(anyhow::anyhow!("sealing SECRET inputs: {e}")))?;
    let mut sealed = nonce;
    sealed.extend(ct);
    Ok((Value::Object(posted), Some(sealed)))
}

/// Executions stored before migration 0203 hold their SECRET values in clear
/// in `inputs`, and their runs' log lines and messages may too. Masks them, and
/// seals the inputs when this process has a cipher (so a replay of such an
/// execution still runs with the value); without one the value is only masked. Idempotent: a row already sealed, or whose SECRET
/// values already read [`MASK`], is left alone. Returns the rows changed.
pub async fn backfill(pool: &sqlx::PgPool) -> Result<u64> {
    let db_err = |e: sqlx::Error| Error::Internal(anyhow::anyhow!("secret inputs backfill: {e}"));
    let mut changed = 0u64;
    let mut after = uuid::Uuid::nil();
    loop {
        let rows: Vec<(uuid::Uuid, String, Value)> = sqlx::query_as(
            "SELECT id, source, inputs FROM workflow.executions \
              WHERE secret_inputs IS NULL AND id > $1 AND source LIKE '%SECRET%' \
              ORDER BY id LIMIT 500",
        )
        .bind(after)
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        let Some(last) = rows.last() else {
            return Ok(changed);
        };
        after = last.0;
        for (id, source, inputs) in rows {
            let Ok(flow) = crate::spec::parse(&source) else {
                continue;
            };
            let Value::Object(mut clear) = inputs else {
                continue;
            };
            // A value that already reads as the mask has nothing to seal.
            let masked_already: Vec<String> = secret_ids(&flow)
                .filter(|sid| clear.get(*sid).and_then(Value::as_str) == Some(MASK))
                .map(str::to_string)
                .collect();
            for sid in &masked_already {
                clear.remove(sid);
            }
            let (masked, sealed) = match CIPHER.get() {
                Some(c) => split_with(&flow, Value::Object(clear.clone()), Some(c))?,
                None => (mask_only(&flow, Value::Object(clear.clone())), None),
            };
            if masked == Value::Object(clear.clone()) {
                continue;
            }
            // The lines and messages its runs already stored carry the value
            // too (`RunLog::masked` came with this change): mask them, longest
            // value first, as `logs::mask` does.
            let mut values: Vec<String> = secret_ids(&flow)
                .filter_map(|sid| clear.get(sid))
                .filter(|v| !v.is_null())
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .filter(|v| !v.is_empty())
                .collect();
            values.sort_by_key(|v| std::cmp::Reverse(v.len()));
            for value in &values {
                for statement in [
                    "UPDATE workflow.task_logs SET message = replace(message, $2, $3) \
                      WHERE execution_id = $1 AND strpos(message, $2) > 0",
                    "UPDATE workflow.task_runs SET message = replace(message, $2, $3) \
                      WHERE execution_id = $1 AND strpos(message, $2) > 0",
                ] {
                    sqlx::query(statement)
                        .bind(id)
                        .bind(value)
                        .bind(MASK)
                        .execute(pool)
                        .await
                        .map_err(db_err)?;
                }
            }
            let Value::Object(mut masked) = masked else {
                continue;
            };
            for sid in masked_already {
                masked.insert(sid, Value::from(MASK));
            }
            let done = sqlx::query(
                "UPDATE workflow.executions SET inputs = $2, secret_inputs = $3 \
                  WHERE id = $1 AND secret_inputs IS NULL",
            )
            .bind(id)
            .bind(Value::Object(masked))
            .bind(sealed)
            .execute(pool)
            .await
            .map_err(db_err)?;
            changed += done.rows_affected();
        }
    }
}

/// `inputs` with each SECRET value replaced by [`MASK`], sealing nothing.
fn mask_only(flow: &Flow, inputs: Value) -> Value {
    let Value::Object(mut posted) = inputs else {
        return inputs;
    };
    for id in secret_ids(flow) {
        if let Some(v) = posted.get_mut(id) {
            if !v.is_null() {
                *v = Value::from(MASK);
            }
        }
    }
    Value::Object(posted)
}

/// `inputs` with the sealed values put back, and those values as strings, for
/// masking what the run writes.
///
/// An `Err` is an outage of this deployment (no key, or another key than the
/// one that sealed it), never the run's: the caller does not run the task.
pub fn reveal(inputs: Value, sealed: Option<&[u8]>) -> Result<(Value, Vec<String>)> {
    let Some(sealed) = sealed else {
        return Ok((inputs, Vec::new()));
    };
    let opened = open(sealed)?;
    let mut values = Vec::with_capacity(opened.len());
    let mut merged = match inputs {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    for (id, v) in opened {
        values.push(match &v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
        merged.insert(id, v);
    }
    Ok((Value::Object(merged), values))
}

fn open(sealed: &[u8]) -> Result<Map<String, Value>> {
    open_named(sealed, "inputs")
}

/// Open one sealed JSON object; `what` (`inputs`, `outputs`) names it in the
/// errors.
fn open_named(sealed: &[u8], what: &str) -> Result<Map<String, Value>> {
    let cipher = CIPHER.get().ok_or_else(|| {
        Error::Internal(anyhow::anyhow!(
            "this execution has SECRET {what} and secrets.master_key is not set"
        ))
    })?;
    if sealed.len() < NONCE_BYTES {
        return Err(Error::Internal(anyhow::anyhow!(
            "the sealed SECRET {what} are truncated"
        )));
    }
    let (nonce, ct) = sealed.split_at(NONCE_BYTES);
    let text = cipher
        .open(nonce, ct)
        .map_err(|e| Error::Internal(anyhow::anyhow!("opening SECRET {what}: {e}")))?;
    match serde_json::from_str(&text) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(Error::Internal(anyhow::anyhow!(
            "the sealed SECRET {what} are not an object"
        ))),
    }
}

// ---------------------------------------------------------------------------
// SECRET outputs (package #6, spec D9): the same cipher, the same column
// shape (`nonce || ciphertext` of one JSON object), for a `Loop` run's list
// (`workflow.task_runs.secret_outputs`, migration 0211). Brought forward from
// Task B4 to Task A4, which is the first writer of a `Loop`'s outputs: a
// SECRET value is never stored in clear, not even for one task.
// ---------------------------------------------------------------------------

/// Whether this process can seal a SECRET value (`secrets.master_key` set).
/// An evaluation that would store one refuses without it (spec D9), as a
/// SECRET input start does.
pub fn can_seal() -> bool {
    CIPHER.get().is_some()
}

/// The refusal of a SECRET output on a deployment with no master key, naming
/// the outputs and the setting (spec D9; [`split`]'s text for inputs).
pub fn no_key_for_secret_outputs(ids: &[String]) -> String {
    format!(
        "egret refuses output {}: a SECRET output's value cannot be stored on this \
         deployment: set secrets.master_key",
        ids.join(", ")
    )
}

/// Seal `object` (one JSON object) under the installed cipher, as
/// [`split`] seals inputs. `Err` with no cipher installed: the caller checks
/// [`can_seal`] at evaluation, so reaching this without one is an outage.
pub(crate) fn seal_object(object: &Map<String, Value>) -> Result<Vec<u8>> {
    seal_object_with(object, CIPHER.get())
}

fn seal_object_with(object: &Map<String, Value>, cipher: Option<&Crypto>) -> Result<Vec<u8>> {
    let cipher = cipher.ok_or_else(|| {
        Error::Internal(anyhow::anyhow!(
            "sealing SECRET outputs: secrets.master_key is not set"
        ))
    })?;
    let (nonce, ct) = cipher
        .seal(&Value::Object(object.clone()).to_string())
        .map_err(|e| Error::Internal(anyhow::anyhow!("sealing SECRET outputs: {e}")))?;
    let mut sealed = nonce;
    sealed.extend(ct);
    Ok(sealed)
}

/// [`split_outputs`]'s answer: the masked map and the sealed SECRET values.
pub(crate) type SplitOutputs = (Map<String, Value>, Option<Vec<u8>>);

/// A flow's typed `outputs:` (package #6 Task B1, spec D9) split for
/// `workflow.executions`: `values` with each of `secret_ids` replaced by
/// [`MASK`], and those values sealed as one `{output id: value}` object for
/// `secret_outputs` (`None` when no SECRET output has a value). `Err` with no
/// cipher installed: the caller refuses first with [`can_seal`].
pub(crate) fn split_outputs(
    values: &Map<String, Value>,
    secret_ids: &[String],
) -> Result<SplitOutputs> {
    split_outputs_with(values, secret_ids, CIPHER.get())
}

fn split_outputs_with(
    values: &Map<String, Value>,
    secret_ids: &[String],
    cipher: Option<&Crypto>,
) -> Result<SplitOutputs> {
    let mut masked = values.clone();
    let mut secret = Map::new();
    for id in secret_ids {
        if let Some(v) = masked.get_mut(id) {
            if !v.is_null() {
                secret.insert(id.clone(), std::mem::replace(v, Value::from(MASK)));
            }
        }
    }
    if secret.is_empty() {
        return Ok((masked, None));
    }
    Ok((masked, Some(seal_object_with(&secret, cipher)?)))
}

/// Open a `Loop` run's sealed SECRET outputs: `{list position: {output id:
/// value}}`.
pub fn open_outputs(sealed: &[u8]) -> Result<Map<String, Value>> {
    open_named(sealed, "outputs")
}

/// An execution's stored flow `outputs` (`workflow.executions.outputs`, the
/// SECRET values masked; package #6 Task B1) with its sealed `{output id:
/// value}` object (`secret_outputs`) put back, and those values as strings,
/// for masking what the execution writes (as [`reveal`] for inputs). Only the
/// worker's render surface calls it (Task B2, spec D9). An id the stored map
/// does not hold is skipped: only a value the evaluation stored is revealed.
pub(crate) fn reveal_flow_outputs(
    outputs: Value,
    sealed: Option<&[u8]>,
) -> Result<(Value, Vec<String>)> {
    let (Some(sealed), Value::Object(mut map)) = (sealed, outputs.clone()) else {
        return Ok((outputs, Vec::new()));
    };
    let mut values = Vec::new();
    for (id, v) in open_outputs(sealed)? {
        if let Some(slot) = map.get_mut(&id) {
            values.push(match &v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
            *slot = v;
        }
    }
    Ok((Value::Object(map), values))
}

/// A `Subflow` run's stored `outputs` (`{executionId, state, outputs: {…}}`,
/// package #6 Task B3) with the child's sealed `{output id: value}` object,
/// copied onto the run, put back into `outputs.outputs`, as
/// [`reveal_flow_outputs`] does for the child itself. Only the worker's
/// render surface calls it.
pub(crate) fn reveal_subflow_outputs(
    outputs: Value,
    sealed: Option<&[u8]>,
) -> Result<(Value, Vec<String>)> {
    let (Some(sealed), Value::Object(mut run)) = (sealed, outputs.clone()) else {
        return Ok((outputs, Vec::new()));
    };
    let Some(map) = run.remove("outputs") else {
        return Ok((outputs, Vec::new()));
    };
    let (map, values) = reveal_flow_outputs(map, Some(sealed))?;
    run.insert("outputs".to_string(), map);
    Ok((Value::Object(run), values))
}

/// A `Loop` run's stored `outputs` (`{…, outputs: [{item, outputs}, …]}`)
/// with its sealed SECRET values put back at their list positions, and those
/// values as strings, for masking what the run writes (as [`reveal`] for
/// inputs). Only the worker's render surface calls it. A position the list
/// does not hold (a list cleared since) is skipped.
pub(crate) fn reveal_loop_outputs(
    outputs: Value,
    sealed: Option<&[u8]>,
) -> Result<(Value, Vec<String>)> {
    let Some(sealed) = sealed else {
        return Ok((outputs, Vec::new()));
    };
    let opened = open_outputs(sealed)?;
    let mut outputs = outputs;
    let mut values = Vec::new();
    for (pos, secret) in opened {
        let (Ok(pos), Value::Object(secret)) = (pos.parse::<usize>(), secret) else {
            continue;
        };
        let Some(Value::Object(entry)) = outputs
            .get_mut("outputs")
            .and_then(|l| l.get_mut(pos))
            .and_then(|e| e.get_mut("outputs"))
        else {
            continue;
        };
        for (id, v) in secret {
            values.push(match &v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
            entry.insert(id, v);
        }
    }
    Ok((outputs, values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flow() -> Flow {
        crate::spec::parse(
            "id: s\nnamespace: t\ninputs:\n  - id: pw\n    type: SECRET\n  \
             - id: who\n    type: STRING\ntasks: []\n",
        )
        .expect("parses")
    }

    #[test]
    fn a_secret_value_is_masked_sealed_and_revealed_and_the_rest_is_untouched() {
        install_for_tests();
        let (masked, sealed) = split(&flow(), json!({"pw": "hunter2", "who": "world"})).unwrap();
        assert_eq!(masked, json!({"pw": MASK, "who": "world"}));
        let sealed = sealed.expect("a value to seal");
        assert!(
            !sealed.windows(7).any(|w| w == b"hunter2"),
            "the column never holds the plaintext"
        );
        let (revealed, values) = reveal(masked, Some(&sealed)).unwrap();
        assert_eq!(revealed, json!({"pw": "hunter2", "who": "world"}));
        assert_eq!(values, vec!["hunter2".to_string()]);
    }

    #[test]
    fn nothing_is_sealed_without_a_secret_value() {
        install_for_tests();
        let (masked, sealed) = split(&flow(), json!({"who": "world", "pw": null})).unwrap();
        assert_eq!(masked, json!({"who": "world", "pw": null}));
        assert_eq!(sealed, None);
        assert_eq!(
            reveal(json!({"who": "world"}), None).unwrap(),
            (json!({"who": "world"}), Vec::new())
        );
    }

    #[test]
    fn a_loop_runs_secret_outputs_are_sealed_by_position_and_revealed_there() {
        install_for_tests();
        let mut sealed = Map::new();
        sealed.insert("1".into(), json!({"s": "hunter2-out"}));
        let sealed = seal_object(&sealed).unwrap();
        assert!(
            !sealed.windows(11).any(|w| w == b"hunter2-out"),
            "the column never holds the plaintext"
        );
        let stored = json!({"iterationCount": 2, "outputs": [
            {"item": {"value": "a", "iteration": 0}, "outputs": {"s": MASK, "n": 1}},
            {"item": {"value": "b", "iteration": 1}, "outputs": {"s": MASK, "n": 2}},
        ]});
        let (revealed, values) = reveal_loop_outputs(stored.clone(), Some(&sealed)).unwrap();
        assert_eq!(revealed["outputs"][1]["outputs"]["s"], json!("hunter2-out"));
        assert_eq!(revealed["outputs"][0]["outputs"]["s"], json!(MASK));
        assert_eq!(values, vec!["hunter2-out".to_string()]);
        assert_eq!(reveal_loop_outputs(stored.clone(), None).unwrap().0, stored);
    }

    #[test]
    fn a_secret_output_cannot_be_sealed_without_a_key() {
        let mut object = Map::new();
        object.insert("0".into(), json!({"s": "v"}));
        let err = seal_object_with(&object, None).unwrap_err().to_string();
        assert!(err.contains("secrets.master_key"), "{err}");
        let refusal = no_key_for_secret_outputs(&["s".to_string()]);
        assert!(
            refusal.starts_with("egret refuses output s:")
                && refusal.contains("secrets.master_key"),
            "{refusal}"
        );
    }

    /// Package #6 Task B1: a flow's outputs keep their order and non-SECRET
    /// values, a SECRET one reads [`MASK`] and opens from the seal, a `null`
    /// SECRET one stays `null` and seals nothing, and no cipher is an error.
    #[test]
    fn flow_outputs_mask_and_seal_their_secret_values() {
        install_for_tests();
        let mut values = Map::new();
        values.insert("p".into(), json!("plain"));
        values.insert("s".into(), json!("hush-b1-unit"));
        values.insert("n".into(), Value::Null);
        let ids = vec!["s".to_string(), "n".to_string()];
        let (masked, sealed) = split_outputs(&values, &ids).unwrap();
        assert_eq!(
            Value::Object(masked),
            json!({"p": "plain", "s": MASK, "n": null})
        );
        let sealed = sealed.expect("one value sealed");
        assert!(!sealed.windows(12).any(|w| w == b"hush-b1-unit"));
        assert_eq!(
            Value::Object(open_outputs(&sealed).unwrap()),
            json!({"s": "hush-b1-unit"})
        );
        let (_, none) = split_outputs(&values, &["n".to_string()]).unwrap();
        assert_eq!(none, None);
        assert!(split_outputs_with(&values, &ids, None).is_err());
    }

    /// Package #6 Task B2: a flow's stored outputs get their sealed values
    /// back (and the values are returned for masking); an id the stored map
    /// does not hold is not added; no seal leaves the map as stored; a
    /// tampered seal is an error, never a silently masked render.
    #[test]
    fn flow_outputs_are_revealed_from_their_seal() {
        install_for_tests();
        let mut values = Map::new();
        values.insert("p".into(), json!("plain"));
        values.insert("s".into(), json!("hush-b2-unit"));
        let (masked, sealed) = split_outputs(&values, &["s".to_string()]).unwrap();
        let sealed = sealed.expect("one value sealed");
        let (revealed, secrets) =
            reveal_flow_outputs(Value::Object(masked.clone()), Some(&sealed)).unwrap();
        assert_eq!(revealed, json!({"p": "plain", "s": "hush-b2-unit"}));
        assert_eq!(secrets, vec!["hush-b2-unit".to_string()]);

        let (only_p, secrets) = reveal_flow_outputs(json!({"p": "plain"}), Some(&sealed)).unwrap();
        assert_eq!(only_p, json!({"p": "plain"}), "nothing added");
        assert!(secrets.is_empty());

        let stored = Value::Object(masked);
        assert_eq!(
            reveal_flow_outputs(stored.clone(), None).unwrap(),
            (stored.clone(), Vec::new())
        );
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        assert!(reveal_flow_outputs(stored, Some(&tampered)).is_err());
    }

    #[test]
    fn a_tampered_seal_is_an_error_not_a_masked_run() {
        install_for_tests();
        let (_, sealed) = split(&flow(), json!({"pw": "hunter2"})).unwrap();
        let mut sealed = sealed.unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        assert!(reveal(json!({}), Some(&sealed)).is_err());
        assert!(reveal(json!({}), Some(&[1, 2, 3])).is_err());
    }
}
