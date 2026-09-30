//! W5b Task 4: Kestra's `secret()`, resolved from the execution's workspace's
//! `workspace_secrets`, for `kestra.*` plugin tasks only (spec §6).
//!
//! # Where `secret()` resolves, and where it does not
//!
//! The shared [`super::env::environment`] keeps **refusing** `secret` (the
//! `REFUSED` row in `functions`, with the pod-spec reason): script tasks,
//! inputs, `runIf`, `Log` and triggers all render against it. Only
//! [`render_with_secrets`] registers a working `secret`, per render, over the
//! refusal on a copy of the environment ([`super::env::render_once_with`]).
//!
//! # One pass each, never the fixpoint loop (fix round 1, user ruling 1A)
//!
//! Every string renders **once**, as Kestra renders a plugin's properties by
//! default (`recursiveRendering = false`). With `render_str`'s loop, text a
//! pass produced was evaluated on the next: an input whose value was
//! `{{ secret('db') }}` resolved a secret the author never wrote, and a
//! secret whose value contained `{{` was evaluated as a template. With one
//! pass, only the author's source is evaluated; render-context data and
//! secret values are inert text.
//!
//! # Collect, fetch, resolve: a render is synchronous and a fetch is not
//!
//! The render runs on its own thread (`env::RENDER_STACK_BYTES`) and cannot
//! await the database. So:
//!
//! 1. **collect**, from two places:
//!    - the **source text** of every string: each `secret(` whose key is a
//!      string literal, first positional or `key=` ([`literal_keys`]). This
//!      sees calls a render never reaches;
//!    - a render of every string with a `secret` that records its key and
//!      answers `""`. This sees computed keys (`secret('db_' ~ env)`);
//! 2. **fetch**: each distinct key is read once from the [`SecretSource`];
//! 3. **resolve**: every string renders again with a `secret` that answers
//!    from what was fetched, and records every value it hands out
//!    ([`Resolved::values`], which the caller masks the attempt's log with).
//!
//! A key first seen in pass 3 was neither written literally nor reachable
//! with `""` in place of every secret, so it was computed from a secret's
//! value, and it is refused **without naming it** -- a name computed from a
//! value can be the value, and a "not found" that named it would write it
//! into the task run's message. What is left of the false positive is on
//! [`DEPENDS_ON_A_SECRET`].
//!
//! # What never carries a value
//!
//! [`SecretError`]'s `Display`, every message this module builds, and any
//! `tracing` event (this module emits none). An error from pass 3 is a
//! minijinja error over a template that has seen real values, so it is
//! returned through [`crate::logs::mask`] with every value handed out.
//! [`Resolved`]'s `Debug` prints a count, not the values.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use minijinja::value::{Rest, Value as MjValue};
use minijinja::{Environment, Error as MjError};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::env::{render_once_with, RenderContext};
use super::filters::{pebble_error, Args};

/// Kestra's `SecretFunction.getArgumentNames()`, in order: a positional
/// argument at index *i* is the *i*-th of these.
const ARGUMENT_NAMES: [&str; 4] = ["key", "namespace", "subkey", "full"];

/// The two of [`ARGUMENT_NAMES`] egret refuses, in any form.
const REFUSED_ARGUMENTS: [&str; 2] = ["namespace", "full"];

/// The key a secret with no `subkey=` answers: Kestra's `VALUE_KEY`, and the
/// key an `Opaque` workspace secret created with a single value holds it in.
const VALUE_KEY: &str = "value";

/// Pass 3 met a key pass 1 never saw.
///
/// **One false positive remains:** a *computed* key (not a string literal) in
/// a `secret()` call the collecting render does not reach -- after an
/// expression that fails when every secret is `""`, or in a branch `""` does
/// not take. A literal key is always collected from the source text.
pub const DEPENDS_ON_A_SECRET: &str = "a secret() key was computed from another secret's value, \
     and secret names must not depend on secrets. (A computed key in a secret() call that \
     follows an expression failing when every secret is empty, or in a branch an empty secret \
     does not take, cannot be collected either: write that key as a string literal.)";

/// Why a secret could not be read. **`Display` names the secret, never a
/// value.**
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// No secret of that name in the execution's workspace. A secret of the
    /// same name in another workspace, or a global one, is this too.
    #[error("secret '{name}' was not found in this execution's workspace")]
    NotFound { name: String },
    /// The store failed. `reason` must not carry a value either: the
    /// production source passes `egret_secrets::SecretsError`'s `Display`,
    /// whose variants carry none.
    #[error("secret '{name}' could not be read: {reason}")]
    Unavailable { name: String, reason: String },
}

/// Where [`render_with_secrets`] reads a secret: its data map, by name, in
/// ONE workspace.
///
/// The production source is `egret_bin::workflow_secrets::WorkspaceSecretSource`,
/// over `egret_secrets::WorkspaceSecretStore::resolve_typed_by_name`. It lives
/// in `egret-bin` so this crate keeps `egret-secrets` a dev-dependency.
#[async_trait]
pub trait SecretSource: Send + Sync {
    /// The data map of `workspace`'s secret named `name`. Never a secret of
    /// another workspace and never a global one.
    async fn get(
        &self,
        workspace: Uuid,
        name: &str,
    ) -> Result<BTreeMap<String, String>, SecretError>;
}

/// Every value a render handed out, for masking the attempt's log
/// (`RunLog::masked`) and anything else the caller writes.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    pub values: Vec<String>,
}

impl fmt::Debug for Resolved {
    /// A count: a value must never reach a `{:?}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resolved")
            .field("values", &format_args!("<{} masked>", self.values.len()))
            .finish()
    }
}

/// Render every string in `props` (recursively; object keys stay literal),
/// resolving `secret()` in `workspace` through `source`.
///
/// See the module doc for the three passes. The error is a message for the
/// task run: it names the property path and the secret, never a value.
pub async fn render_with_secrets(
    props: &Map<String, Value>,
    cx: &RenderContext,
    workspace: Uuid,
    source: &dyn SecretSource,
) -> Result<(Map<String, Value>, Resolved), String> {
    // Pass 1: collect. An error here is not final: `""` in place of a secret
    // can fail an expression the real value satisfies, and pass 3 reproduces
    // any error that does not depend on a value.
    let collected = Arc::new(Collected::default());
    {
        let mut names = collected.names.lock().expect("collect lock");
        let _ = walk_object(props, "", &mut |_, text| {
            names.extend(literal_keys(text));
            Ok(String::new())
        });
    }
    {
        let c = collected.clone();
        let extend = move |env: &mut Environment<'static>| {
            let c = c.clone();
            env.add_function("secret", move |rest: Rest<MjValue>| c.call(rest));
        };
        let _ = walk_object(props, "", &mut |_, text| {
            let _ = render_once_with(text, cx, &extend);
            Ok(String::new())
        });
    }

    // Pass 2: fetch each distinct key once. A failed fetch is kept, not
    // returned: only a key pass 3 actually asks for may fail the render.
    let names: BTreeSet<String> = collected.names.lock().expect("collect lock").clone();
    let mut fetched = BTreeMap::new();
    for name in names {
        let answer = source.get(workspace, &name).await;
        fetched.insert(name, answer);
    }

    // Pass 3: resolve.
    let resolver = Arc::new(Resolver {
        fetched,
        handed: Mutex::default(),
    });
    let rendered = {
        let r = resolver.clone();
        let extend = move |env: &mut Environment<'static>| {
            let r = r.clone();
            env.add_function("secret", move |rest: Rest<MjValue>| r.call(rest));
        };
        walk_object(props, "", &mut |path, text| {
            render_once_with(text, cx, &extend).map_err(|e| format!("`{path}` did not render: {e}"))
        })
    };
    let values = resolver.handed.lock().expect("resolve lock").clone();
    match rendered {
        Ok(out) => Ok((out, Resolved { values })),
        Err(message) => Err(crate::logs::mask(&message, &values).into_owned()),
    }
}

/// The string-literal keys of every `secret(` call in `source`: a first
/// argument `'k'` / `"k"`, or `key='k'` anywhere in the call.
///
/// **A regex, not the parser**, because minijinja 2.24 exposes its AST only
/// under `unstable_machinery`, which this workspace does not enable. What it
/// cannot see, each of which the collecting render still sees when it reaches
/// the call:
/// - a literal with an escaped quote or backslash (`'it\'s'`);
/// - a call whose `key=` follows a nested call's `)` (`secret(subkey=f(), key='k')`);
/// - `secret` bound to another name (`{% set s = secret %}{{ s('k') }}`).
///
/// It also over-matches -- text outside `{{ }}`, a `{% raw %}` region -- and
/// that costs one extra fetch of a name the author wrote, never a value: an
/// over-matched name that nothing asks for in pass 3 is never handed out and
/// its failed fetch is never reported. It scans the AUTHOR's source only,
/// never render-context data.
fn literal_keys(source: &str) -> Vec<String> {
    static PATTERNS: std::sync::OnceLock<[regex::Regex; 2]> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        let literal = r#"(?:'([^'\\]*)'|"([^"\\]*)")"#;
        [
            regex::Regex::new(&format!(r"\bsecret\s*\(\s*(?:key\s*=\s*)?{literal}"))
                .expect("first-argument pattern"),
            regex::Regex::new(&format!(r"\bsecret\s*\([^()]*?\bkey\s*=\s*{literal}"))
                .expect("key= pattern"),
        ]
    });
    if !source.contains("secret") {
        return Vec::new();
    }
    patterns
        .iter()
        .flat_map(|p| p.captures_iter(source))
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Whether one string, as the author wrote it, can call `secret()` when it
/// is rendered (fix round 1: the publish refusals of I2 and the
/// `workflow:use_secrets` gate of I3).
///
/// **The identifier, not the call.** `{% set s = secret %}{{ s('k') }}`
/// calls it with no `secret(` in the text, so any `secret` that is not an
/// attribute (`inputs.secret`) or part of a longer name (`mysecret`,
/// `secret_task`) counts, in a string that holds a `{{` or a `{%` -- a string
/// with neither renders as itself. The whole string is scanned rather than
/// the regions between delimiters: `{{ "}}" ~ secret('k') }}` would end a
/// naive region early. Over-matching (the word inside a string literal of a
/// template) costs a refusal the author can read, never a secret.
///
/// Complete while nothing can look a function up by a computed name:
/// minijinja has no such lookup, and Kestra's `render()`/`renderOnce()`,
/// which would render a string built at run time, are refused
/// (`functions::REFUSED`). If either arrives, this must change with it.
pub fn text_uses_secret(text: &str) -> bool {
    static IDENT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if !(text.contains("{{") || text.contains("{%")) || !text.contains("secret") {
        return false;
    }
    IDENT
        .get_or_init(|| regex::Regex::new(r"(?:^|[^.\w])secret\b").expect("identifier pattern"))
        .is_match(text)
}

/// Whether any string or mapping key of the flow `source` can call
/// `secret()` ([`text_uses_secret`]), read **as YAML decodes it**: a
/// double-quoted `"\u0073ecret"` is `secret` to the renderer, and an alias
/// is its anchor's value. What `routes` gates `workflow:use_secrets` on.
///
/// Source that does not parse as YAML is scanned as raw text, so the answer
/// only errs towards "yes"; the routes never store such a source anyway.
pub fn source_uses_secret(source: &str) -> bool {
    fn walk(v: &serde_norway::Value) -> bool {
        use serde_norway::Value as Y;
        match v {
            Y::String(s) => text_uses_secret(s),
            Y::Sequence(items) => items.iter().any(walk),
            Y::Mapping(m) => m.iter().any(|(k, v)| walk(k) || walk(v)),
            Y::Tagged(t) => walk(&t.value),
            Y::Null | Y::Bool(_) | Y::Number(_) => false,
        }
    }
    match serde_norway::from_str::<serde_norway::Value>(source) {
        Ok(doc) => walk(&doc),
        Err(_) => text_uses_secret(source),
    }
}

/// One `secret(...)` call's arguments, after the refusals.
struct Call {
    key: String,
    subkey: Option<String>,
}

/// Both passes parse the same way, so a refusal is the same in both.
fn parse_call(rest: Rest<MjValue>) -> Result<Call, MjError> {
    let args = Args::new(rest);
    // `namespace` and `full`, by name, in any form. A second positional
    // argument IS `namespace` in Kestra's signature, not the subkey, so any
    // positional argument after `key` passes `namespace` first.
    let second_positional = (args.positional_only().len() > 1).then_some(ARGUMENT_NAMES[1]);
    let refused = second_positional
        .into_iter()
        .chain(args.named_keys())
        .find(|name| REFUSED_ARGUMENTS.contains(name) || !ARGUMENT_NAMES.contains(name));
    if let Some(name) = refused {
        return Err(pebble_error(if REFUSED_ARGUMENTS.contains(&name) {
            format!(
                "The 'secret' function's argument '{name}' is not supported: egret resolves a \
                 secret by name in the execution's workspace only, and a subkey only by \
                 `subkey=`."
            )
        } else {
            format!("The 'secret' function has no argument '{name}'.")
        }));
    }

    let Some(key) = args.get("key", 0) else {
        return Err(pebble_error(
            "The 'secret' function expects an argument 'key'.",
        ));
    };
    let Some(key) = key.as_str().map(str::to_string) else {
        return Err(pebble_error(
            "The 'secret' function expects an argument 'key' with type string.",
        ));
    };
    // Kestra: `subkey != null && !subkey.isEmpty()`.
    let subkey = match args.get("subkey", usize::MAX) {
        None => None,
        Some(v) if v.is_none() || v.is_undefined() => None,
        Some(v) => match v.as_str() {
            Some("") => None,
            Some(s) => Some(s.to_string()),
            None => {
                return Err(pebble_error(
                    "The 'secret' function expects an argument 'subkey' with type string.",
                ))
            }
        },
    };
    Ok(Call { key, subkey })
}

/// Pass 1's `secret`: records the key, answers `""`.
#[derive(Default)]
struct Collected {
    names: Mutex<BTreeSet<String>>,
}

impl Collected {
    fn call(&self, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
        let call = parse_call(rest)?;
        self.names.lock().expect("collect lock").insert(call.key);
        Ok(MjValue::from(""))
    }
}

/// Pass 3's `secret`: answers from what pass 2 fetched.
struct Resolver {
    fetched: BTreeMap<String, Result<BTreeMap<String, String>, SecretError>>,
    handed: Mutex<Vec<String>>,
}

impl Resolver {
    fn call(&self, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
        let call = parse_call(rest)?;
        let name = &call.key;
        let data = match self.fetched.get(name) {
            // NOT named: see the module doc.
            None => return Err(pebble_error(DEPENDS_ON_A_SECRET)),
            Some(Err(e)) => return Err(pebble_error(e.to_string())),
            Some(Ok(data)) => data,
        };
        let keys = || data.keys().cloned().collect::<Vec<_>>().join(", ");
        let value = match &call.subkey {
            Some(subkey) => data.get(subkey).ok_or_else(|| {
                pebble_error(format!(
                    "Cannot find secret sub-key '{subkey}' in secret '{name}'; its keys are: {}.",
                    keys()
                ))
            })?,
            None => data.get(VALUE_KEY).ok_or_else(|| {
                pebble_error(format!(
                    "secret '{name}' has no key '{VALUE_KEY}'; its keys are: {}. Name one with \
                     `subkey=`.",
                    keys()
                ))
            })?,
        };
        let mut handed = self.handed.lock().expect("resolve lock");
        if !handed.contains(value) {
            handed.push(value.clone());
        }
        Ok(MjValue::from(value.as_str()))
    }
}

/// `object` with every string rendered by `render`, keys literal. `path` is
/// the dotted path of `object` itself (`""` at the root).
fn walk_object(
    object: &Map<String, Value>,
    path: &str,
    render: &mut dyn FnMut(&str, &str) -> Result<String, String>,
) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for (k, v) in object {
        let here = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        out.insert(k.clone(), walk(v, &here, render)?);
    }
    Ok(out)
}

fn walk(
    value: &Value,
    path: &str,
    render: &mut dyn FnMut(&str, &str) -> Result<String, String>,
) -> Result<Value, String> {
    Ok(match value {
        Value::String(text) => Value::String(render(path, text)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, item)| walk(item, &format!("{path}[{i}]"), render))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(walk_object(map, path, render)?),
        other => other.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::render_str;
    use serde_json::json;

    /// A source over a fixed table, keyed by (workspace, name), that counts
    /// its reads.
    #[derive(Default)]
    struct Table {
        rows: BTreeMap<(Uuid, String), BTreeMap<String, String>>,
        reads: Mutex<Vec<String>>,
    }

    impl Table {
        fn with(mut self, ws: Uuid, name: &str, data: &[(&str, &str)]) -> Self {
            self.rows.insert(
                (ws, name.to_string()),
                data.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            );
            self
        }
    }

    #[async_trait]
    impl SecretSource for Table {
        async fn get(
            &self,
            workspace: Uuid,
            name: &str,
        ) -> Result<BTreeMap<String, String>, SecretError> {
            self.reads.lock().unwrap().push(name.to_string());
            self.rows
                .get(&(workspace, name.to_string()))
                .cloned()
                .ok_or_else(|| SecretError::NotFound { name: name.into() })
        }
    }

    const WS: Uuid = Uuid::from_u128(7);

    fn table() -> Table {
        Table::default().with(WS, "p", &[("value", "s3cr3t")]).with(
            WS,
            "db",
            &[("username", "admin"), ("password", "pa55word")],
        )
    }

    async fn render(
        props: Value,
        source: &Table,
    ) -> Result<(Map<String, Value>, Resolved), String> {
        render_with_secrets(
            props.as_object().unwrap(),
            &RenderContext::new(),
            WS,
            source,
        )
        .await
    }

    /// The error of rendering one property `v`.
    async fn err(template: &str, source: &Table) -> String {
        render(json!({ "v": template }), source)
            .await
            .expect_err("must fail")
    }

    #[tokio::test]
    async fn a_value_and_a_subkey_resolve_and_are_handed_out() {
        let source = table();
        let (out, resolved) = render(
            json!({
                "url": "jdbc:postgresql://h/db",
                "password": "{{ secret('p') }}",
                "nested": {"list": ["{{ secret('db', subkey='username') }}", 3, null]},
                "{{ secret('p') }}": "the key is literal",
            }),
            &source,
        )
        .await
        .unwrap();
        assert_eq!(
            Value::Object(out),
            json!({
                "url": "jdbc:postgresql://h/db",
                "password": "s3cr3t",
                "nested": {"list": ["admin", 3, null]},
                "{{ secret('p') }}": "the key is literal",
            })
        );
        let mut values = resolved.values.clone();
        values.sort();
        assert_eq!(values, vec!["admin".to_string(), "s3cr3t".into()]);
        // A value never reaches a `{:?}`.
        let debug = format!("{resolved:?}");
        assert!(
            !debug.contains("s3cr3t") && debug.contains("<2 masked>"),
            "{debug}"
        );
    }

    #[tokio::test]
    async fn each_distinct_key_is_fetched_once() {
        let source = table();
        render(
            json!({"a": "{{ secret('p') }}{{ secret('p') }}", "b": ["{{ secret(key='p') }}"]}),
            &source,
        )
        .await
        .unwrap();
        assert_eq!(*source.reads.lock().unwrap(), vec!["p".to_string()]);
    }

    #[tokio::test]
    async fn namespace_and_full_are_refused_by_name() {
        let source = table();
        for (call, name) in [
            ("{{ secret('p', namespace='other') }}", "namespace"),
            ("{{ secret('p', 'other') }}", "namespace"),
            ("{{ secret('p', full=true) }}", "full"),
            ("{{ secret('p', none, none, true) }}", "namespace"),
        ] {
            let e = err(call, &source).await;
            assert!(
                e.contains(&format!("argument '{name}' is not supported")),
                "{call}: {e}"
            );
            assert!(!e.contains("s3cr3t"), "{call}: {e}");
        }
        // The accepted neighbours: `key=` and `subkey=` by name.
        let (out, _) = render(
            json!({"v": "{{ secret(key='db', subkey='password') }}"}),
            &source,
        )
        .await
        .unwrap();
        assert_eq!(out["v"], "pa55word");
        // An unknown keyword is refused as unknown, not silently ignored.
        let e = err("{{ secret('p', subky='x') }}", &source).await;
        assert!(e.contains("has no argument 'subky'"), "{e}");
    }

    #[tokio::test]
    async fn an_opaque_secret_without_value_names_its_keys() {
        let source = table();
        let no_value = err("{{ secret('db') }}", &source).await;
        assert!(
            no_value.contains("secret 'db' has no key 'value'; its keys are: password, username"),
            "{no_value}"
        );
        let no_subkey = err("{{ secret('db', subkey='token') }}", &source).await;
        assert!(
            no_subkey.contains("sub-key 'token' in secret 'db'; its keys are: password, username"),
            "{no_subkey}"
        );
        for e in [&no_value, &no_subkey] {
            // Keys, never values; and the property path is named.
            assert!(!e.contains("pa55word") && !e.contains("admin"), "{e}");
            assert!(e.starts_with("`v` did not render:"), "{e}");
        }
    }

    #[tokio::test]
    async fn a_missing_secret_is_named_and_carries_no_value() {
        let source = table();
        let e = err("{{ secret('p') }}{{ secret('nope') }}", &source).await;
        assert!(
            e.contains("secret 'nope' was not found in this execution's workspace"),
            "{e}"
        );
        assert!(!e.contains("s3cr3t"), "{e}");
    }

    #[tokio::test]
    async fn secret_under_a_filter_sees_the_real_value() {
        let source = table();
        let (out, _) = render(
            json!({"n": "{{ secret('p') | length }}", "u": "{{ secret('p') | upper }}"}),
            &source,
        )
        .await
        .unwrap();
        assert_eq!(out["n"], "6");
        assert_eq!(out["u"], "S3CR3T");
    }

    #[tokio::test]
    async fn a_secret_name_computed_from_a_secret_is_refused() {
        // `p`'s value is the NAME of another secret that exists: resolving it
        // would work, and must not.
        let source = table().with(WS, "s3cr3t", &[("value", "second")]);
        for call in [
            "{{ secret(secret('p')) }}",
            "{{ secret(secret('p') | upper | lower) }}",
        ] {
            let e = err(call, &source).await;
            assert!(e.contains(DEPENDS_ON_A_SECRET), "{call}: {e}");
            assert!(
                !e.contains("s3cr3t") && !e.contains("second"),
                "{call}: {e}"
            );
        }
        // It was never fetched either: pass 1 saw `""`, not the value.
        assert!(!source.reads.lock().unwrap().contains(&"s3cr3t".to_string()));
    }

    /// An error raised after a value was handed out is masked with it.
    #[tokio::test]
    async fn a_render_error_after_a_value_is_masked() {
        let source = table();
        // `date` quotes the text it could not parse: `Text 's3cr3t' could
        // not be parsed ...`.
        let e = err("{{ secret('p') | date('yyyy') }}", &source).await;
        assert!(e.contains("Text '******' could not be parsed"), "{e}");
        assert!(!e.contains("s3cr3t"), "{e}");
    }

    #[tokio::test]
    async fn another_workspaces_secret_is_not_asked_for() {
        let other = Uuid::from_u128(8);
        let source = Table::default().with(other, "p", &[("value", "theirs")]);
        let e = err("{{ secret('p') }}", &source).await;
        assert!(e.contains("secret 'p' was not found"), "{e}");
        assert!(!e.contains("theirs"), "{e}");
    }

    // -----------------------------------------------------------------
    // Fix round 1 (user ruling 1A): one pass, and literal keys from source.
    // -----------------------------------------------------------------

    async fn render_in(
        props: Value,
        context: Value,
        source: &Table,
    ) -> Result<(Map<String, Value>, Resolved), String> {
        let cx = RenderContext::from_json_object(&context).expect("an object");
        render_with_secrets(props.as_object().unwrap(), &cx, WS, source).await
    }

    /// CRITICAL 1: render-context data is data. An input (or webhook body, or
    /// prior output) whose VALUE is a `secret()` call is not evaluated, so it
    /// cannot resolve a secret the author never wrote -- and the source is
    /// never even asked.
    #[tokio::test]
    async fn an_input_holding_a_secret_call_renders_literally_and_nothing_is_fetched() {
        let source = table();
        // The third keeps its body and loses its markers: Kestra strips the
        // `{% raw %}` markers from the OUTPUT of `renderOnce`
        // (`RAW_PATTERN.matcher(x).replaceAll("$2")`), data included.
        for (injected, stored) in [
            ("{{ secret('p') }}", "{{ secret('p') }}"),
            (
                "{{ secret('db', subkey='password') }}",
                "{{ secret('db', subkey='password') }}",
            ),
            (
                "{% raw %}{{ secret('p') }}{% endraw %}",
                "{{ secret('p') }}",
            ),
        ] {
            let (out, resolved) = render_in(
                json!({"sql": "SELECT '{{ q }}' AS v", "url": "{{ q }}"}),
                json!({ "q": injected }),
                &source,
            )
            .await
            .unwrap();
            assert_eq!(out["sql"], format!("SELECT '{stored}' AS v"), "{injected}");
            assert_eq!(out["url"], stored);
            assert!(resolved.values.is_empty(), "{injected}: {resolved:?}");
        }
        assert!(
            source.reads.lock().unwrap().is_empty(),
            "the source was asked for {:?}",
            source.reads.lock().unwrap()
        );
    }

    /// IMPORTANT 1: a secret's value is text, whatever it contains.
    #[tokio::test]
    async fn a_secret_value_with_template_syntax_renders_verbatim() {
        let values = [
            "a{{b",
            "x{#y",
            "x{{ '' }}y",
            "{{ printContext() }}",
            "{% if true %}t{% endif %}",
        ];
        let mut source = Table::default();
        for (i, v) in values.iter().enumerate() {
            source = source.with(WS, &format!("s{i}"), &[("value", v)]);
        }
        for (i, v) in values.iter().enumerate() {
            let (out, resolved) = render(
                json!({ "v": format!("<{{{{ secret('s{i}') }}}}>") }),
                &source,
            )
            .await
            .unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(out["v"], format!("<{v}>"));
            assert_eq!(resolved.values, vec![v.to_string()]);
        }
    }

    /// IMPORTANT 2: a literal key is collected from the source, so a call the
    /// collecting render never reaches -- after an expression that fails on
    /// `""`, or in a branch `""` does not take -- still resolves.
    #[tokio::test]
    async fn a_literal_key_past_a_failing_expression_or_an_untaken_branch_resolves() {
        let source = table()
            .with(WS, "cfg", &[("value", r#"{"x": 1}"#)])
            .with(WS, "a", &[("value", "yes")])
            .with(WS, "b", &[("value", "B")]);
        let (out, _) = render(
            json!({
                "failing": "{{ fromJson(secret('cfg')).x }}-{{ secret('b') }}",
                "branch": "{% if secret('a') %}{{ secret(key=\"b\") }}{% endif %}",
            }),
            &source,
        )
        .await
        .unwrap();
        assert_eq!(out["failing"], "1-B");
        assert_eq!(out["branch"], "B");
    }

    #[test]
    fn literal_keys_sees_both_spellings_and_both_quotes() {
        let got = literal_keys(
            "{{ secret('a') }} {{ secret(\"b\", subkey='x') }} {{ secret(key='c') }} \
             {{ secret(subkey='y', key=\"d\") }} {{ secret( 'e' ) | upper }} \
             {{ secret(k) }} {{ mysecret('no') }}",
        );
        let got: BTreeSet<String> = got.into_iter().collect();
        let want: BTreeSet<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(got, want);
        assert!(literal_keys("no call here").is_empty());
    }

    /// Fix round 1, I3: what counts as "this flow calls secret()". Every
    /// spelling a render could reach counts, including `secret` bound to
    /// another name and a YAML escape that decodes to it; `.secret`
    /// (an attribute) and text outside any `{{ }}`/`{% %}` do not.
    #[test]
    fn text_uses_secret_sees_every_spelling_a_render_reaches() {
        for yes in [
            "{{ secret('db') }}",
            "{{secret('db')}}",
            "{{ secret ('db') }}",
            "{{- secret(key='db') -}}",
            "{% set s = secret %}{{ s('db') }}",
            "{{ (secret)('db') }}",
            "x {{ 'a' ~ secret('db') }} y",
            "{% if secret('a') %}1{% endif %}",
            "{{ \"}}\" ~ secret('db') }}",
        ] {
            assert!(text_uses_secret(yes), "{yes}");
        }
        for no in [
            "secret('db')",
            "select secret from t",
            "{{ inputs.secret }}",
            "{{ outputs.secret_task.value }}",
            "{{ mysecret('x') }}",
            "{{ secrets('x') }}",
            "",
        ] {
            assert!(!text_uses_secret(no), "{no}");
        }
    }

    #[test]
    fn source_uses_secret_walks_every_string_and_key_as_yaml_decodes_it() {
        let yes = [
            "id: f\nnamespace: a\ntasks:\n  - id: t\n    type: x\n    p: \"{{ secret('db') }}\"\n",
            // A YAML escape the raw text does not show.
            "id: f\nnamespace: a\ntasks:\n  - id: t\n    type: x\n    p: \"{{ \\u0073ecret('db') }}\"\n",
            // Inputs, triggers, a key, deep nesting, an anchor's alias.
            "id: f\nnamespace: a\ninputs:\n  - id: i\n    type: STRING\n    defaults: \"{{ secret('x') }}\"\ntasks: []\n",
            "id: f\nnamespace: a\ntasks:\n  - id: t\n    type: x\n    m:\n      \"{{ secret('k') }}\": v\n",
            "id: f\nnamespace: a\nvariables:\n  a: &x \"{% set s = secret %}{{ s('k') }}\"\ntasks:\n  - id: t\n    type: x\n    p: *x\n",
        ];
        for src in yes {
            assert!(source_uses_secret(src), "{src}");
        }
        let no = [
            "id: f\nnamespace: a\ntasks:\n  - id: t\n    type: x\n    p: \"{{ inputs.secret }}\"\n",
            "id: f\nnamespace: a\ndescription: keep the secret safe\ntasks: []\n",
        ];
        for src in no {
            assert!(!source_uses_secret(src), "{src}");
        }
    }

    /// The shared environment -- the one a script task renders its commands
    /// and env against -- still refuses `secret`, and says why.
    #[test]
    fn secret_on_a_script_task_is_still_refused_with_the_pod_spec_reason() {
        for call in [
            "{{ secret('p') }}",
            "{{ secret('p', subkey='k') }}",
            "{{ secret(key='p') }}",
        ] {
            let e = render_str(call, &RenderContext::new())
                .expect_err("must refuse")
                .to_string();
            assert!(
                e.contains("resolves only on `kestra.*` plugin tasks"),
                "{call}: {e}"
            );
            assert!(e.contains("fields of the pod spec"), "{call}: {e}");
            assert!(e.contains("`get pods`"), "{call}: {e}");
        }
    }
}
