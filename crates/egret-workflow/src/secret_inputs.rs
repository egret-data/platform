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

/// Process-wide, set once by egret-bin from `secrets.master_key`. A second
/// call returns the cipher it was given back, and the first stays.
pub fn install(cipher: Crypto) -> std::result::Result<(), Crypto> {
    CIPHER.set(cipher)
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
/// in `inputs`. Masks them, and seals them when this process has a cipher (so a
/// replay of such an execution still runs with the value); without one the
/// value is only masked. Idempotent: a row already sealed, or whose SECRET
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
            if masked == Value::Object(clear) {
                continue;
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
    let cipher = CIPHER.get().ok_or_else(|| {
        Error::Internal(anyhow::anyhow!(
            "this execution has SECRET inputs and secrets.master_key is not set"
        ))
    })?;
    if sealed.len() < NONCE_BYTES {
        return Err(Error::Internal(anyhow::anyhow!(
            "the sealed SECRET inputs are truncated"
        )));
    }
    let (nonce, ct) = sealed.split_at(NONCE_BYTES);
    let text = cipher
        .open(nonce, ct)
        .map_err(|e| Error::Internal(anyhow::anyhow!("opening SECRET inputs: {e}")))?;
    match serde_json::from_str(&text) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(Error::Internal(anyhow::anyhow!(
            "the sealed SECRET inputs are not an object"
        ))),
    }
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
