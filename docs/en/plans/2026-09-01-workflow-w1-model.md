# Workflow W1 — Flow Model & Language Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a flow a first-class, versioned, publishable artifact — the Kestra flow model ported to Rust, YAML parse + validate, draft/version tables, and publish with reference resolution.

**Architecture:** Seven tasks, bottom-up. Tasks 1–3 port the model (no I/O). Task 4 adds parse + validate + the task-type registry (no I/O). Task 5 adds the tables and repo. Task 6 adds publish, where every reference to an egret object is resolved and workspace-checked. Task 7 adds routes and wires them into `egret-server`.

**Tech Stack:** Rust 1.85 / edition 2021, `serde` + `serde_norway` (YAML), `sqlx` + Postgres, `axum`, `utoipa`.

Spec: `docs/en/specs/2026-09-01-workflow-w1-model-design.md` (VI: `docs/vi/...`).
Programme spec: `docs/en/specs/2026-09-01-workflow-programme-design.md`.

## Global Constraints

- **Kestra source reference:** `data-portal/kestra` at commit `96893b589` (2.0.0-SNAPSHOT). Port against this checkout only. **Artifact reference:** `2.0.0-rc13`.
- **Every ported file names its Kestra source on line one**, e.g. `// kestra: core/src/main/java/io/kestra/core/models/flows/Flow.java`.
- **Type and field names are kept verbatim from Kestra**, converted to Rust case (`isTerminatedNoFail` → `is_terminated_no_fail`, `taskRunner` → `task_runner`). Where a Rust keyword collides, use `r#type`, never `type_` or `kind_`.
- **Serde spelling matches Kestra's YAML**: `#[serde(rename_all = "camelCase")]` on structs whose Kestra fields are camelCase; `SCREAMING_SNAKE_CASE` on enums Kestra writes in caps (input types, `State`).
- **`models/` and `spec/` must not touch the database or the network.** No `sqlx`, no `reqwest`, no `axum` in those modules.
- **YAML crate: `serde_norway` 0.9.42.** Chosen because it is *already* in this workspace's dependency tree via `utoipa v5.5.0`, so it adds zero new transitive crates (`cargo tree -i serde_norway` proves it), and its `Error::location()` returns line/column, which §9 layer 1 requires. `serde_yaml` is present too but only via `kube-client` and is archived upstream — do not add a direct dependency on it.
- **SQL:** use `sqlx::query_as(concat!(...))` with a `&'static str`, matching `dataapi/src/endpoints.rs`. Do not reach for `AssertSqlSafe`.
- **Verification (Rust):** `just test-no-db` is NOT a gate on this repo — `crates/egret-audit/tests/audit_store.rs:6` `.expect()`s on a `None` that `EGRET_ALLOW_DB_SKIP=1` produces by design, and `cargo test` stops at the first failing binary, so it reaches 3 of ~249. Use:
  ```bash
  cargo test -p egret-workflow --no-fail-fast            # must be fully green
  env -u DATABASE_URL EGRET_ALLOW_DB_SKIP=1 cargo test --workspace --no-fail-fast \
    > /tmp/w1-<task>-after.log 2>&1; echo "EXIT=$?"
  grep -c '^test result:' /tmp/w1-<task>-after.log        # must be ~249, never 0 and never 3
  grep -E '^test .* FAILED$' /tmp/w1-<task>-after.log | sort -u > /tmp/w1-<task>-after.failed
  diff .superpowers/sdd/2026-09-01-workflow-w0-foundation/baseline.failed /tmp/w1-<task>-after.failed
  ```
  `sort -u` on **both** sides is load-bearing: one guard test repeats 13× across binaries, so an un-deduplicated list compares 41 lines against a 26-line baseline and reports a spurious mismatch.
- **DB-backed tests must export `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`.** Never the dev database (`.../egret`). A DB-gated test with no `DATABASE_URL` reports `ok` having asserted nothing; only its *duration* distinguishes a real run from a silent skip.
- **Never restore a file with `cp`** — it is aliased to `cp -i` here and silently refuses to overwrite while returning success. Use `git checkout -- <path>` for tracked files, `cat backup > file` for untracked ones, then `grep` to prove the restore happened.
- Conventional Commits. Do not merge; open nothing.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/egret-workflow/src/models/property.rs` | `Property<T>` — raw source + target type |
| `crates/egret-workflow/src/models/input.rs` | `Input`, `InputType` — Kestra's `inputs:` vocabulary |
| `crates/egret-workflow/src/models/task.rs` | `Task`, `TaskKind`, control-flow types, `Retry` |
| `crates/egret-workflow/src/models/flow.rs` | `Flow`, `Label`, `Concurrency`, `Output`, `Sla`, `Check`, `Quota` |
| `crates/egret-workflow/src/models/trigger.rs` | `Trigger` and its variants |
| `crates/egret-workflow/src/models/execution.rs` | `Execution`, `TaskRun` — defined here, produced in W3 |
| `crates/egret-workflow/src/spec/parse.rs` | YAML → `Flow`, with line/column on failure |
| `crates/egret-workflow/src/spec/registry.rs` | task-type registry: known types + their reference fields |
| `crates/egret-workflow/src/spec/validate.rs` | the three validation layers |
| `migrations/0132_workflow_flows.sql` | `workflow.flows` + `workflow.flow_versions` |
| `crates/egret-workflow/src/repo.rs` | CRUD over both tables |
| `crates/egret-workflow/src/publish.rs` | reference resolution + snapshot freeze |
| `crates/egret-workflow-api/src/port.rs` | `WorkflowRefPort` — what publish asks other seams |
| `crates/egret-workflow/src/routes.rs` | the nine routes |
| `crates/egret-server/src/lib.rs` | mount + openapi merge |

---

### Task 1: `Property<T>` and the input vocabulary

**Files:**
- Create: `crates/egret-workflow/src/models/property.rs`, `crates/egret-workflow/src/models/input.rs`
- Modify: `crates/egret-workflow/src/models/mod.rs`, `crates/egret-workflow/Cargo.toml`

**Interfaces:**
- Consumes: nothing.
- Produces: `Property<T>` with `Property::from_source(String) -> Self`, `fn source(&self) -> &str`, `Serialize`/`Deserialize` for any `T`; `InputType` (a `SCREAMING_SNAKE_CASE` enum), `Input { id: String, r#type: InputType, description: Option<String>, required: bool, defaults: Option<Property<serde_json::Value>>, display_name: Option<String>, depends_on: Option<DependsOn> }`, `DependsOn { inputs: Vec<String>, condition: Option<String> }`.

- [ ] **Step 1: Add the YAML dependency**

In `crates/egret-workflow/Cargo.toml`, under `[dependencies]`:

```toml
# Already in this workspace's tree via utoipa 5.5.0, so this adds zero new
# transitive crates -- `cargo tree -i serde_norway` shows utoipa as its only
# parent today. Chosen over the more popular serde_yaml because that crate is
# archived upstream (0.9.34+deprecated) and reaches us only through kube-client.
# Error::location() gives the line/column that spec section 9 layer 1 requires.
serde_norway = "0.9"
serde_json.workspace = true
```

Add to the root `Cargo.toml` `[workspace.dependencies]` if not present:

```toml
serde_norway = "0.9"
```

Then confirm nothing new entered the tree:

Run: `cargo tree -p egret-workflow -e normal --depth 1`
Expected: `serde`, `serde_json`, `serde_norway` and nothing surprising.

- [ ] **Step 2: Write the failing test for `Property<T>`**

Create `crates/egret-workflow/src/models/property.rs` with ONLY this test module:

```rust
// kestra: core/src/main/java/io/kestra/core/models/property/Property.java

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
```

- [ ] **Step 3: Run it and confirm it fails to compile for the stated reason**

Run: `cargo test -p egret-workflow --lib models::property`
Expected: FAIL to compile — `cannot find type Property in this scope`. If the message is anything else, stop and fix that first: a test that does not build proves nothing.

- [ ] **Step 4: Implement `Property<T>`**

Insert above the test module:

```rust
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
        Self { source, _target: PhantomData }
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
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p egret-workflow --lib models::property`
Expected: PASS, 2 tests.

If `every_yaml_shape_becomes_source_text` fails on the map case with key ordering, that is real: `serde_json::Value`'s object is a `BTreeMap` by default, so `{k: v}` is deterministic here. A multi-key map would need `preserve_order`; do not add that feature — instead assert on a single-key map, as the test does.

- [ ] **Step 6: Write the failing test for the input vocabulary**

Create `crates/egret-workflow/src/models/input.rs` with ONLY this test module:

```rust
// kestra: core/src/main/java/io/kestra/core/models/flows/Input.java
// kestra: core/src/main/java/io/kestra/core/models/flows/input/

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

    #[test]
    fn every_type_round_trips_in_kestra_spelling() {
        for (spelling, variant) in [
            ("STRING", InputType::String),
            ("INT", InputType::Int),
            ("FLOAT", InputType::Float),
            ("BOOLEAN", InputType::Boolean),
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
        ] {
            let parsed: InputType =
                serde_norway::from_str(spelling).unwrap_or_else(|e| panic!("{spelling}: {e}"));
            assert_eq!(parsed, variant, "{spelling}");
            assert_eq!(serde_json::to_string(&variant).unwrap(), format!("\"{spelling}\""));
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
```

- [ ] **Step 7: Confirm `required`'s default against the Kestra source before implementing**

Read `kestra/core/src/main/java/io/kestra/core/models/flows/Input.java` and find the `required` field. Record in your report what its declared default is. **If it is not `true`, change the test in Step 6 to match the source and say so** — the source is the authority, not this plan.

- [ ] **Step 8: Implement `InputType` and `Input`**

Port the type list from `kestra/core/src/main/java/io/kestra/core/models/flows/input/` (24 files; ignore `EeOnly.java`, `InputAndValue.java`, `ItemTypeInterface.java`, `ValueOption.java` — they are not types). Insert above the test module:

```rust
use serde::{Deserialize, Serialize};

use crate::models::Property;

/// Kestra's `inputs:` type vocabulary, spelled exactly as Kestra spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InputType {
    String,
    Int,
    Float,
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
```

Note the `SCREAMING_SNAKE_CASE` rename produces `DATETIME` for `Datetime` (one word, not `DATE_TIME`) — the round-trip test in Step 6 is what proves it. If serde produces `DATE_TIME`, add `#[serde(rename = "DATETIME")]` on that variant rather than renaming the variant.

- [ ] **Step 9: Wire both modules**

In `crates/egret-workflow/src/models/mod.rs`, after the existing `mod state;`:

```rust
mod input;
mod property;
pub use input::{DependsOn, Input, InputType};
pub use property::Property;
```

- [ ] **Step 10: Run the crate's tests**

Run: `cargo test -p egret-workflow --no-fail-fast`
Expected: PASS — the 3 `State` tests from W0 plus the 5 added here.

- [ ] **Step 11: Prove the input-type test can fail (mutation)**

Change `InputType::Datetime`'s spelling by adding `#[serde(rename = "DATE_TIME")]`, rebuild, and confirm `every_type_round_trips_in_kestra_spelling` FAILS. **Confirm it compiled first** (`cargo build -p egret-workflow --tests` → 0 errors) — a mutation that does not build looks exactly like a test too weak to notice. Restore with `git checkout -- crates/egret-workflow/src/models/input.rs` and re-run to confirm green.

- [ ] **Step 12: Full check and commit**

```bash
cargo test -p egret-workflow --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
git add Cargo.toml Cargo.lock crates/egret-workflow
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): port Property<T> and the inputs type vocabulary

Property is raw source rendered at use time, reproducing Kestra's
PropertyDeserializer, which flattens every YAML shape to a string. W2 gives it
render; W1 only defines it.

The inputs vocabulary is Kestra's, spelled as Kestra spells it, so a Kestra flow
pasted into egret parses. egret's own six-variant ParamType stays with the Data
API -- carrying two vocabularies is the cost of that fidelity (spec section 13.2)."
```

---

### Task 2: `Task`, `TaskKind`, and the control-flow types

**Files:**
- Create: `crates/egret-workflow/src/models/task.rs`
- Modify: `crates/egret-workflow/src/models/mod.rs`

**Interfaces:**
- Consumes: `Property<T>` from Task 1.
- Produces: `Task { id: String, r#type: String, description: Option<String>, retry: Option<Retry>, timeout: Option<Property<Duration>>, disabled: bool, kind: TaskKind }`; `TaskKind` with variants `Sequential(Sequential)`, `Parallel(Parallel)`, `If(If)`, `Switch(Switch)`, `Dag(Dag)`, `ForEach(ForEach)`, `Subflow(Subflow)`, `AllowFailure(AllowFailure)`, `WorkingDirectory(WorkingDirectory)`, `Leaf { spec: serde_json::Value }`; `Retry` with `RetryBehavior`; `fn Task::children(&self) -> Vec<&Task>` returning the tasks a control-flow node owns, in declaration order, empty for `Leaf`.

- [ ] **Step 1: Read the Kestra sources you are porting**

All under `kestra/core/src/main/java/io/kestra/plugin/core/flow/`:
`Sequential.java`, `Parallel.java`, `If.java`, `Switch.java`, `Dag.java`, `Loop.java` (Kestra's `ForEach` lives in the plugin `io.kestra.plugin.core.flow.ForEach`; if it is not in this checkout, port `Loop.java` and name the Rust type `ForEach` only if the Kestra type is genuinely named that — otherwise keep Kestra's name), `Subflow.java`, `AllowFailure.java`, `WorkingDirectory.java`.
Plus `kestra/core/src/main/java/io/kestra/core/models/tasks/Task.java` and `retrys/`.

**Record in your report the exact list of control-flow types this checkout actually has, and their real Kestra names.** If a name in this plan does not exist in the checkout, the checkout wins — say so rather than inventing the type.

- [ ] **Step 2: Write the failing test**

Create `crates/egret-workflow/src/models/task.rs` with ONLY this test module:

```rust
// kestra: core/src/main/java/io/kestra/core/models/tasks/Task.java
// kestra: core/src/main/java/io/kestra/plugin/core/flow/

#[cfg(test)]
mod tests {
    use super::{Task, TaskKind};

    /// A leaf task egret owns. The model must NOT understand what egret.sql is --
    /// a state machine that knows what SQL is has stopped being a state machine.
    #[test]
    fn an_unknown_type_becomes_an_opaque_leaf() {
        let yaml = r#"
id: stage
type: egret.sql
database: trino_prod
sql: SELECT 1
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        assert_eq!(t.id, "stage");
        assert_eq!(t.r#type, "egret.sql");
        match &t.kind {
            TaskKind::Leaf { spec } => {
                assert_eq!(spec["database"], "trino_prod");
                assert_eq!(spec["sql"], "SELECT 1");
                // `id` and `type` are Task's own fields and must not be
                // duplicated into the leaf spec, or a handler would see two
                // sources of truth for them.
                assert!(spec.get("id").is_none(), "id leaked into the leaf spec");
                assert!(spec.get("type").is_none(), "type leaked into the leaf spec");
            }
            other => panic!("expected Leaf, got {other:?}"),
        }
    }

    /// Control flow the state machine DOES have to see into. The FQCN is the
    /// canonical spelling; the short alias is the secondary path.
    #[test]
    fn control_flow_is_typed_under_both_spellings() {
        for spelling in ["io.kestra.plugin.core.flow.Sequential", "core.sequential"] {
            let yaml = format!(
                "id: fan\ntype: {spelling}\ntasks:\n  - id: a\n    type: egret.sql\n    sql: SELECT 1\n"
            );
            let t: Task = serde_norway::from_str(&yaml).unwrap();
            match &t.kind {
                TaskKind::Sequential(s) => assert_eq!(s.tasks.len(), 1, "{spelling}"),
                other => panic!("{spelling}: expected Sequential, got {other:?}"),
            }
        }
    }

    /// `children()` is what W3's advance() walks. A Leaf owning children would
    /// make the state machine descend into a task it must not understand.
    #[test]
    fn children_are_the_control_flow_nodes_only() {
        let yaml = r#"
id: outer
type: core.sequential
tasks:
  - id: a
    type: egret.sql
    sql: SELECT 1
  - id: b
    type: egret.sql
    sql: SELECT 2
"#;
        let t: Task = serde_norway::from_str(yaml).unwrap();
        let ids: Vec<&str> = t.children().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "children must be in declaration order");
        assert!(t.children()[0].children().is_empty(), "a leaf owns no children");
    }

    #[test]
    fn disabled_defaults_to_false() {
        let t: Task =
            serde_norway::from_str("id: a\ntype: egret.sql\nsql: SELECT 1\n").unwrap();
        assert!(!t.disabled);
    }
}
```

- [ ] **Step 3: Run it and confirm it fails to compile**

Run: `cargo test -p egret-workflow --lib models::task`
Expected: FAIL to compile — `cannot find type Task in this scope`.

- [ ] **Step 4: Implement the model**

`Task` cannot use a plain `#[serde(tag = "type")]` enum, because the tag values are FQCNs *and* short aliases *and* anything unknown must fall through to `Leaf`. Deserialize into an intermediate map, pull `id`/`type`/the shared fields out, then dispatch on `type`:

```rust
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

use crate::models::Property;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Property<Duration>>,
    pub disabled: bool,
    #[serde(flatten)]
    pub kind: TaskKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TaskKind {
    Sequential(Sequential),
    Parallel(Parallel),
    If(If),
    Switch(Switch),
    Dag(Dag),
    ForEach(ForEach),
    Subflow(Subflow),
    AllowFailure(AllowFailure),
    WorkingDirectory(WorkingDirectory),
    Leaf { spec: serde_json::Value },
}

impl Task {
    /// The tasks this node owns, in declaration order. Empty for a leaf.
    ///
    /// This is what W3's `advance()` walks, and it is the reason control flow is
    /// typed while leaves are opaque.
    pub fn children(&self) -> Vec<&Task> {
        match &self.kind {
            TaskKind::Sequential(s) => s.tasks.iter().collect(),
            TaskKind::Parallel(p) => p.tasks.iter().collect(),
            TaskKind::If(i) => i.then.iter().chain(i.r#else.iter()).collect(),
            TaskKind::Switch(s) => s
                .cases
                .values()
                .flatten()
                .chain(s.defaults.iter())
                .collect(),
            TaskKind::Dag(d) => d.tasks.iter().map(|n| &n.task).collect(),
            TaskKind::ForEach(f) => f.tasks.iter().collect(),
            TaskKind::AllowFailure(a) => a.tasks.iter().collect(),
            TaskKind::WorkingDirectory(w) => w.tasks.iter().collect(),
            TaskKind::Subflow(_) | TaskKind::Leaf { .. } => Vec::new(),
        }
    }
}
```

Then the control-flow structs, each ported field-for-field from its Java file, each with `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]` and `#[serde(rename_all = "camelCase")]`. `Sequential`/`Parallel`/`AllowFailure`/`WorkingDirectory`/`ForEach` carry `tasks: Vec<Task>`; `If` carries `condition: Property<String>`, `then: Vec<Task>`, `r#else: Vec<Task>` (serde-renamed to `else`); `Switch` carries `value: Property<String>`, `cases: std::collections::BTreeMap<String, Vec<Task>>`, `defaults: Vec<Task>`; `Dag` carries `tasks: Vec<DagNode>` where `DagNode { task: Task, depends_on: Vec<String> }`; `Subflow` carries `namespace: String`, `flow_id: String`, `inputs`, `wait`, `transmit_failed`.

`Parallel` also has `concurrent: Property<i32>` in Kestra — port it.

The custom `Deserialize` for `Task`:

```rust
impl<'de> Deserialize<'de> for Task {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        let mut map = serde_json::Map::deserialize(d)?;

        let id = map
            .remove("id")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::missing_field("id"))?;
        let r#type = map
            .remove("type")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::missing_field("type"))?;
        let description = map.remove("description").and_then(|v| v.as_str().map(str::to_owned));
        let retry = match map.remove("retry") {
            Some(v) => Some(serde_json::from_value(v).map_err(D::Error::custom)?),
            None => None,
        };
        let timeout = match map.remove("timeout") {
            Some(v) => Some(serde_json::from_value(v).map_err(D::Error::custom)?),
            None => None,
        };
        let disabled = map
            .remove("disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let rest = serde_json::Value::Object(map);
        let kind = kind_for(&r#type, rest).map_err(D::Error::custom)?;

        Ok(Task { id, r#type, description, retry, timeout, disabled, kind })
    }
}

/// Dispatch a task's remaining fields on its `type`.
///
/// Both spellings are accepted and the FQCN is canonical, so a Kestra flow
/// pasted in parses and looking a task up in Kestra's docs lands on the right
/// page (programme spec section 7 rule 4).
fn kind_for(r#type: &str, rest: serde_json::Value) -> Result<TaskKind, serde_json::Error> {
    let canonical = r#type
        .strip_prefix("io.kestra.plugin.core.flow.")
        .map(str::to_ascii_lowercase)
        .or_else(|| r#type.strip_prefix("core.").map(str::to_ascii_lowercase));

    Ok(match canonical.as_deref() {
        Some("sequential") => TaskKind::Sequential(serde_json::from_value(rest)?),
        Some("parallel") => TaskKind::Parallel(serde_json::from_value(rest)?),
        Some("if") => TaskKind::If(serde_json::from_value(rest)?),
        Some("switch") => TaskKind::Switch(serde_json::from_value(rest)?),
        Some("dag") => TaskKind::Dag(serde_json::from_value(rest)?),
        Some("foreach") => TaskKind::ForEach(serde_json::from_value(rest)?),
        Some("subflow") => TaskKind::Subflow(serde_json::from_value(rest)?),
        Some("allowfailure") => TaskKind::AllowFailure(serde_json::from_value(rest)?),
        Some("workingdirectory") => TaskKind::WorkingDirectory(serde_json::from_value(rest)?),
        _ => TaskKind::Leaf { spec: rest },
    })
}
```

Port `Retry` from `kestra/core/src/main/java/io/kestra/core/models/tasks/retrys/` — `AbstractRetry.java` plus `Constant`, `Exponential`, `Random`. Keep Kestra's `type` discriminator spelling.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p egret-workflow --lib models::task`
Expected: PASS, 4 tests.

`an_unknown_type_becomes_an_opaque_leaf` asserting `spec.get("id").is_none()` is the one most likely to fail first — if it does, the `map.remove` calls are not running before `rest` is built. That is the bug, not the test.

- [ ] **Step 6: Prove `children()` is load-bearing (mutation)**

Change `TaskKind::Sequential(s) => s.tasks.iter().collect()` to `=> Vec::new()`, rebuild (`cargo build -p egret-workflow --tests`, expect 0 errors), and confirm `children_are_the_control_flow_nodes_only` FAILS. Restore with `git checkout --` and re-run green. Record both the compile status and the failure output.

- [ ] **Step 7: Full check and commit**

```bash
cargo test -p egret-workflow --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
git add crates/egret-workflow
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): port Task, TaskKind and the control-flow types

Hybrid model: control flow is typed because W3's advance() must walk the task
tree structurally, leaves are opaque because a state machine that knows what SQL
is has stopped being a state machine. Leaf is also the only representation that
can hold io.kestra.plugin.* when the W5 plugin host lands, without editing an enum.

An unknown type is NOT a parse error -- parsing must not depend on knowing what a
task does, or a flow could not be edited before its handler exists. Publish is
where unknown types are refused (Task 4's registry)."
```

---

### Task 3: `Flow`, triggers, and the execution model

**Files:**
- Create: `crates/egret-workflow/src/models/flow.rs`, `crates/egret-workflow/src/models/trigger.rs`, `crates/egret-workflow/src/models/execution.rs`
- Modify: `crates/egret-workflow/src/models/mod.rs`

**Interfaces:**
- Consumes: `Task`, `Input`, `Property<T>`, `State`.
- Produces: `Flow { id: String, namespace: String, description, labels: Vec<Label>, inputs: Vec<Input>, outputs: Vec<FlowOutput>, variables: BTreeMap<String, serde_json::Value>, tasks: Vec<Task>, errors: Vec<Task>, finally_: Vec<Task>, triggers: Vec<Trigger>, concurrency: Option<Concurrency>, retry: Option<Retry>, sla: Vec<Sla>, disabled: bool }`; `Label { key: String, value: String }`; `Concurrency { limit: i32, behavior: ConcurrencyBehavior }`; `Trigger { id, r#type, disabled, kind: TriggerKind }` with `TriggerKind::{Schedule, Webhook, Flow, Leaf}`; `Execution { id, namespace, flow_id, flow_revision, task_run_list: Vec<TaskRun>, inputs, labels, state: StateHistory, ... }`; `TaskRun { id, execution_id, task_id, parent_task_run_id, value, attempts, outputs, state: StateHistory }`; `StateHistory { current: State, histories: Vec<StateTransition> }`.

- [ ] **Step 1: Read the sources and record what actually exists**

`kestra/core/src/main/java/io/kestra/core/models/flows/` — `Flow.java`, `AbstractFlow.java`, `Concurrency.java`, `Output.java`, `Label.java` (in `models/`), `sla/`, `check/`, `quota/`.
`.../models/triggers/` — `AbstractTrigger.java` and the trigger types in `io/kestra/plugin/core/trigger/`.
`.../models/executions/` — `Execution.java`, `TaskRun.java`, and `State.java`'s history types.

**Record the actual field list of `Flow` in your report** before writing Rust. The spec's list is from a reading; the checkout is the authority.

- [ ] **Step 2: Write the failing test**

Create `crates/egret-workflow/src/models/flow.rs` with ONLY this test module:

```rust
// kestra: core/src/main/java/io/kestra/core/models/flows/Flow.java

#[cfg(test)]
mod tests {
    use super::Flow;

    /// The example from the programme spec, which is what a user will paste.
    #[test]
    fn parses_the_reference_flow() {
        let yaml = r#"
id: daily_sales_rollup
namespace: company.analytics
description: Roll up orders by day
labels:
  team: analytics
inputs:
  - id: run_date
    type: DATE
tasks:
  - id: stage
    type: egret.sql
    database: trino_prod
    sql: SELECT 1
  - id: fanout
    type: core.parallel
    tasks:
      - id: enrich
        type: egret.python
        notebook: nb_enrich
errors:
  - id: alert
    type: core.slack
    message: FAILED
triggers:
  - id: nightly
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "0 2 * * *"
"#;
        let f: Flow = serde_norway::from_str(yaml).unwrap();
        assert_eq!(f.id, "daily_sales_rollup");
        assert_eq!(f.namespace, "company.analytics");
        assert_eq!(f.inputs.len(), 1);
        assert_eq!(f.tasks.len(), 2);
        assert_eq!(f.errors.len(), 1);
        assert_eq!(f.triggers.len(), 1);
        assert_eq!(f.labels.len(), 1);
        // Nesting must survive: `fanout` owns `enrich`.
        assert_eq!(f.tasks[1].children().len(), 1);
    }

    /// A flow with only the required fields must parse. Every optional field
    /// defaulting correctly is what lets a minimal Kestra flow work.
    #[test]
    fn a_minimal_flow_parses_with_empty_collections() {
        let f: Flow = serde_norway::from_str(
            "id: min\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sql\n    sql: SELECT 1\n",
        )
        .unwrap();
        assert!(f.inputs.is_empty());
        assert!(f.errors.is_empty());
        assert!(f.triggers.is_empty());
        assert!(f.labels.is_empty());
        assert!(f.concurrency.is_none());
        assert!(!f.disabled);
    }

    /// `finally` is a Rust keyword-adjacent name and a Kestra field. Getting the
    /// serde rename wrong would silently drop the handler.
    #[test]
    fn finally_is_read_under_its_kestra_spelling() {
        let f: Flow = serde_norway::from_str(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\nfinally:\n  - id: c\n    type: y\n",
        )
        .unwrap();
        assert_eq!(f.finally_.len(), 1, "`finally:` must map onto finally_");
    }
}
```

- [ ] **Step 3: Run it and confirm it fails to compile**

Run: `cargo test -p egret-workflow --lib models::flow`
Expected: FAIL to compile — `cannot find type Flow in this scope`.

- [ ] **Step 4: Implement `Flow` and its value types**

Port field-for-field from the sources you read in Step 1. Every collection field gets `#[serde(default)]` so a minimal flow parses; `finally` gets `#[serde(rename = "finally", default)]` on a field named `finally_`. `labels` accepts both Kestra spellings — a map (`team: analytics`) and a list of `{key, value}` — via a custom deserializer, because Kestra accepts both and a flow using the list form must not fail.

Also port, into the same file: `Label`, `Concurrency` + `ConcurrencyBehavior`, `FlowOutput` (Kestra's `Output`, renamed to avoid colliding with task output), `Sla`, `Check`, `Quota`.

- [ ] **Step 5: Implement triggers and the execution model**

`crates/egret-workflow/src/models/trigger.rs` mirrors Task 2's dispatch shape: a `Trigger` struct with shared fields plus a `TriggerKind` that is typed for `Schedule`, `Webhook`, and `Flow`, and `Leaf { spec }` for anything else.

`crates/egret-workflow/src/models/execution.rs` ports `Execution` and `TaskRun`. Nothing produces these until W3; they are defined now so W3's pure state machine is written against a fixed target rather than a moving one — the same reason `State` landed in W0.

Add to each file's line 1 its `// kestra:` source header.

- [ ] **Step 6: Wire the modules and run**

In `models/mod.rs`:

```rust
mod execution;
mod flow;
mod trigger;
pub use execution::{Execution, StateHistory, StateTransition, TaskRun};
pub use flow::{Check, Concurrency, ConcurrencyBehavior, Flow, FlowOutput, Label, Quota, Sla};
pub use trigger::{Trigger, TriggerKind};
```

Run: `cargo test -p egret-workflow --no-fail-fast`
Expected: PASS.

- [ ] **Step 7: Prove the `finally` rename is load-bearing (mutation)**

Remove `#[serde(rename = "finally")]` from the `finally_` field, rebuild (expect 0 errors — the field still exists, so it compiles), and confirm `finally_is_read_under_its_kestra_spelling` FAILS. This mutation is worth running precisely because it compiles: a wrong serde rename is invisible to the compiler and silently drops the field. Restore and re-run green.

- [ ] **Step 8: Full check and commit**

```bash
cargo test -p egret-workflow --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
git add crates/egret-workflow
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): port Flow, triggers, and the execution model

Execution and TaskRun are defined here although nothing produces them until W3 --
they are what W3's pure state machine is written against, and defining them late
would mean writing that machine against a moving target. Same reason State landed
in W0.

sla, checks and quotas are ported as data shapes only. Their behaviour is W3/W8;
Task 4's validator reports them as declared-but-unimplemented rather than
accepting them silently."
```

> **Half of that quotation went out of date on 2026-09-14 (W8 Task 6), and it is
> annotated rather than edited because it is a record of what W1 decided.**
> `SLA.Type` has two subtypes and they are two different mechanisms in the
> artifact. `EXECUTION_ASSERTION` (`ExecutionChangedSLA`) is **evaluated** from
> W8 Task 6 — `crates/egret-workflow/src/sla.rs`, called by
> `worker::claim::settle` — and it banks two golden entries.
> `MAX_DURATION` (`ExecutionMonitoringSLA`) is still a data shape, and so are
> `checks` and `quotas`. The `not_yet_enforced` warning below narrowed with it:
> a flow whose `sla:` is assertions only no longer gets one, because for that
> flow the warning would be false.

---

### Task 4: Parse, the task-type registry, and validation

**Files:**
- Create: `crates/egret-workflow/src/spec/mod.rs`, `spec/parse.rs`, `spec/registry.rs`, `spec/validate.rs`
- Modify: `crates/egret-workflow/src/lib.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–3.
- Produces: `parse(source: &str) -> Result<Flow, ParseError>` where `ParseError { message: String, line: Option<usize>, column: Option<usize> }`; `validate(flow: &Flow) -> ValidationReport`; `ValidationReport { errors: Vec<Finding>, warnings: Vec<Finding> }`; `Finding { code: &'static str, message: String, path: String }`; `registry::known_type(t: &str) -> Option<&'static TaskTypeSpec>`; `TaskTypeSpec { r#type: &'static str, references: &'static [RefField] }`; `RefField { field: &'static str, kind: RefKind }`; `RefKind::{Database, Notebook, Dataset, Chart, DataApiEndpoint, Subflow}`.

- [ ] **Step 1: Write the failing tests**

Create `crates/egret-workflow/src/spec/validate.rs` with ONLY this test module (the parse tests live here too so one file holds the language's behaviour):

```rust
#[cfg(test)]
mod tests {
    use crate::spec::{parse, validate};

    #[test]
    fn a_syntax_error_carries_line_and_column() {
        let err = parse("id: a\n  namespace: bad indent\n").unwrap_err();
        assert!(err.line.is_some(), "a syntax error must locate itself: {err:?}");
    }

    #[test]
    fn duplicate_task_ids_are_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n  - id: t\n    type: y\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.iter().any(|e| e.code == "duplicate_task_id"), "{r:?}");
    }

    /// Nested duplicates are the case a shallow check misses.
    #[test]
    fn duplicate_task_ids_are_found_inside_control_flow() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: x\n  - id: outer\n    type: core.parallel\n    tasks:\n      - id: t\n        type: y\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.iter().any(|e| e.code == "duplicate_task_id"), "{r:?}");
    }

    #[test]
    fn a_bad_namespace_is_an_error() {
        let f = parse("id: f\nnamespace: Not-Valid\ntasks:\n  - id: t\n    type: x\n").unwrap();
        let r = validate(&f);
        assert!(r.errors.iter().any(|e| e.code == "invalid_namespace"), "{r:?}");
    }

    /// The single most important behaviour in this task. A feature that parses
    /// but is not enforced must SAY SO: without this, a user writes
    /// `concurrency: {limit: 1}`, sees a green publish, and reasonably concludes
    /// it is enforced.
    #[test]
    fn declared_but_unimplemented_features_warn_and_name_their_package() {
        let f = parse(
            "id: f\nnamespace: a.b\nconcurrency:\n  limit: 1\ntasks:\n  - id: t\n    type: x\n",
        )
        .unwrap();
        let r = validate(&f);
        let w = r
            .warnings
            .iter()
            .find(|w| w.code == "not_yet_enforced")
            .unwrap_or_else(|| panic!("no not_yet_enforced warning: {r:?}"));
        assert!(w.message.contains("concurrency"), "{}", w.message);
        assert!(w.message.contains("W8"), "the warning must name the package: {}", w.message);
        assert!(r.errors.is_empty(), "a declared-but-unimplemented feature is not an error");
    }

    #[test]
    fn a_dag_cycle_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: d\n    type: core.dag\n    tasks:\n      - task:\n          id: a\n          type: x\n        dependsOn: [b]\n      - task:\n          id: b\n          type: x\n        dependsOn: [a]\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.iter().any(|e| e.code == "dag_cycle"), "{r:?}");
    }

    #[test]
    fn a_subflow_naming_itself_is_an_error() {
        let f = parse(
            "id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: core.subflow\n    namespace: a.b\n    flowId: f\n",
        )
        .unwrap();
        let r = validate(&f);
        assert!(r.errors.iter().any(|e| e.code == "self_subflow"), "{r:?}");
    }
}
```

- [ ] **Step 2: Run and confirm they fail to compile**

Run: `cargo test -p egret-workflow --lib spec::validate`
Expected: FAIL to compile — `unresolved import crate::spec`.

- [ ] **Step 3: Implement `parse`**

`spec/parse.rs`:

```rust
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
pub fn parse(source: &str) -> Result<Flow, ParseError> {
    serde_norway::from_str::<Flow>(source).map_err(|e| {
        let loc = e.location();
        ParseError {
            message: e.to_string(),
            line: loc.as_ref().map(|l| l.line()),
            column: loc.as_ref().map(|l| l.column()),
        }
    })
}
```

- [ ] **Step 4: Implement the registry**

`spec/registry.rs` — declarative, no behaviour:

```rust
/// What kind of egret object a task field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Database,
    Notebook,
    Dataset,
    Chart,
    DataApiEndpoint,
    Subflow,
}

#[derive(Debug, Clone, Copy)]
pub struct RefField {
    pub field: &'static str,
    pub kind: RefKind,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskTypeSpec {
    pub r#type: &'static str,
    pub references: &'static [RefField],
}

/// Every task type egret knows, and which of its fields name egret objects.
///
/// This table is the declaration half of a seam: W3's handlers are the
/// implementation half and must stay in step with it. Publish refuses a type
/// absent from here, so the table is also what makes an unknown type
/// unpublishable rather than silently unresolvable (spec section 4.2, 8).
pub const TASK_TYPES: &[TaskTypeSpec] = &[
    TaskTypeSpec {
        r#type: "egret.sql",
        references: &[RefField { field: "database", kind: RefKind::Database }],
    },
    TaskTypeSpec {
        r#type: "egret.python",
        references: &[RefField { field: "notebook", kind: RefKind::Notebook }],
    },
    TaskTypeSpec { r#type: "egret.container", references: &[] },
    TaskTypeSpec {
        r#type: "egret.dataapi.call",
        references: &[RefField { field: "endpoint", kind: RefKind::DataApiEndpoint }],
    },
    TaskTypeSpec {
        r#type: "egret.dataset.refresh",
        references: &[RefField { field: "dataset", kind: RefKind::Dataset }],
    },
    TaskTypeSpec {
        r#type: "egret.report.send",
        references: &[RefField { field: "chart", kind: RefKind::Chart }],
    },
    TaskTypeSpec { r#type: "core.http", references: &[] },
    TaskTypeSpec { r#type: "core.slack", references: &[] },
    TaskTypeSpec { r#type: "core.log", references: &[] },
];

pub fn known_type(t: &str) -> Option<&'static TaskTypeSpec> {
    TASK_TYPES.iter().find(|s| s.r#type == t)
}
```

- [ ] **Step 5: Implement `validate`**

Above the test module in `spec/validate.rs`. Three layers in order, per spec §9. Walk the whole task tree via `Task::children()` for the id-uniqueness and cycle checks — the nested-duplicate test in Step 1 is what proves the walk is recursive rather than shallow.

The declared-but-unimplemented warning table:

```rust
/// Features that parse today but nothing enforces yet, and the package that will.
///
/// Naming the package is not decoration. A user who writes `concurrency: {limit: 1}`
/// and sees a green publish will reasonably conclude it is enforced; the warning is
/// the only thing standing between them and that conclusion.
const NOT_YET_ENFORCED: &[(&str, &str)] = &[
    ("concurrency", "W8"),
    ("sla", "W8"),
    ("checks", "W8"),
    ("quotas", "W8"),
    ("retry", "W3"),
    ("timeout", "W3"),
    ("triggers", "W6"),
];
```

`namespace` is validated against Kestra's exact pattern `^[a-z0-9][a-z0-9._-]*` and length 1–150. Use a hand-written character check rather than adding a `regex` dependency for one pattern.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p egret-workflow --lib spec`
Expected: PASS, 7 tests.

- [ ] **Step 7: Prove the recursive walk is load-bearing (mutation)**

Change the id-collection to look at `flow.tasks` only, without recursing through `children()`. Rebuild (expect 0 errors), and confirm `duplicate_task_ids_are_found_inside_control_flow` FAILS while `duplicate_task_ids_are_an_error` still passes. That asymmetry is the point: the shallow check looks correct until a flow nests. Restore and re-run green.

- [ ] **Step 8: Full check and commit**

```bash
cargo test -p egret-workflow --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
git add crates/egret-workflow
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): parse, validate, and the task-type registry

Three validation layers in order: parse (syntax, located), structural (ids,
cycles, self-subflow), semantic (namespace, and declared-but-unimplemented
features reported as warnings that NAME the package that will implement them).

The registry lives in W1, not W3: without it every egret task type is unknown, so
nothing could be published and W1 would ship a publish route that always refuses.
It is the declaration half of a seam W3's handlers must stay in step with."
```

---

### Task 5: Tables and repository

**Files:**
- Create: `migrations/0132_workflow_flows.sql`, `crates/egret-workflow/src/repo.rs`
- Create: `crates/egret-workflow/tests/repo.rs`
- Modify: `crates/egret-workflow/Cargo.toml`, `crates/egret-workflow/src/lib.rs`

**Interfaces:**
- Consumes: `Flow` from Task 3.
- Produces: `FlowRepo::new(db: Db) -> Self`; `create(ws: Uuid, by: UserId, input: &CreateFlowInput) -> Result<FlowRow>`; `get(ws: Uuid, id: Uuid) -> Result<FlowRow>`; `list(ws: Uuid, filter: &ListFilter) -> Result<Vec<FlowRow>>`; `update(ws, id, by, &UpdateFlowInput) -> Result<FlowRow>`; `soft_delete(ws, id) -> Result<()>`; `insert_version(flow: Uuid, by: UserId, snapshot: &serde_json::Value, note: &str) -> Result<FlowVersionRow>`; `set_live(flow: Uuid, version: Uuid) -> Result<()>`; `list_versions(flow: Uuid) -> Result<Vec<FlowVersionRow>>`; `get_version(flow: Uuid, version: i32) -> Result<FlowVersionRow>`; `workspace_of(id: Uuid) -> Result<Option<Uuid>>`.

- [ ] **Step 1: Write the migration**

Create `migrations/0132_workflow_flows.sql` with the two tables exactly as spec §7 gives them, plus this header comment:

```sql
-- Workflow W1: flow drafts and their immutable published versions.
--
-- TWO tables, and the split is the whole feature, for the same reason recorded
-- in 0121 for dataapi.endpoints: if execution read the draft, editing a flow
-- would change the behaviour of tonight's scheduled run, and "versioning" would
-- be decoration.
--
-- `namespace` is Kestra's, kept because every Kestra flow has one and rejecting
-- it would break paste-compatibility at its most visible point. Uniqueness is
-- (workspace_id, namespace, flow_id) -- Kestra's (tenant, namespace, id) with
-- the workspace in the tenant's place. `folder_id` remains the UI's organising
-- axis, as it is for dataapi.endpoints, and is deliberately NOT derived from
-- namespace: folders get renamed and re-parented, a namespace is identity.
CREATE SCHEMA IF NOT EXISTS workflow;
```

Then the two `CREATE TABLE`s and the deferred FK from spec §7, plus:

```sql
CREATE INDEX workflow_flows_ws_idx ON workflow.flows (workspace_id);
CREATE INDEX workflow_flow_versions_flow_idx
  ON workflow.flow_versions (flow_id, version DESC);

-- Managing flows is an editor act, following alert:manage (0110) and
-- chart:manage (0014).
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'workflow:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

- [ ] **Step 2: Apply it and confirm it is reversible in the only way that matters**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
just migrate
psql "$DATABASE_URL" -c '\d workflow.flows'
psql "$DATABASE_URL" -c '\d workflow.flow_versions'
```

Expected: both tables exist with the columns from spec §7.

**If you edit this migration after applying it, its checksum changes and sqlx will refuse.** Drop and recreate `egret_test` in that case; do not hand-edit the migrations table.

- [ ] **Step 3: Write the failing test**

Create `crates/egret-workflow/tests/repo.rs`:

```rust
//! FlowRepo insert + read + version. Requires DATABASE_URL.

use uuid::Uuid;

mod common;

#[tokio::test]
async fn a_draft_round_trips_and_versions_are_ordered() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());

    let created = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: "a.b".into(),
                flow_id: "f".into(),
                source: "id: f\nnamespace: a.b\ntasks: []\n".into(),
                plan: serde_json::json!({"id": "f"}),
                folder_id: None,
            },
        )
        .await
        .expect("create");

    assert_eq!(created.namespace, "a.b");
    assert_eq!(created.state, "draft");
    assert!(created.live_version_id.is_none());

    let v1 = repo
        .insert_version(created.id, user, &serde_json::json!({"v": 1}), "first")
        .await
        .expect("v1");
    let v2 = repo
        .insert_version(created.id, user, &serde_json::json!({"v": 2}), "second")
        .await
        .expect("v2");
    assert_eq!(v1.version, 1);
    assert_eq!(v2.version, 2, "versions must increment per flow, not globally");

    let versions = repo.list_versions(created.id).await.expect("list");
    assert_eq!(versions[0].version, 2, "newest first");
}

/// The scoping obligation every port in this repo carries: answer with the
/// object's OWN workspace so the caller can compare, and let the caller refuse a
/// mismatch as NotFound. A never-existed id and a foreign id must be
/// indistinguishable to the caller.
#[tokio::test]
async fn workspace_of_answers_with_the_flows_own_workspace() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = repo
        .create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: "a.b".into(),
                flow_id: "g".into(),
                source: "id: g\n".into(),
                plan: serde_json::json!({}),
                folder_id: None,
            },
        )
        .await
        .expect("create");

    assert_eq!(repo.workspace_of(f.id).await.unwrap(), Some(ws));
    assert_eq!(repo.workspace_of(Uuid::new_v4()).await.unwrap(), None);
}

/// (workspace_id, namespace, flow_id) is the Kestra identity. Two flows with the
/// same id in different namespaces must both exist.
#[tokio::test]
async fn the_same_flow_id_is_allowed_in_two_namespaces() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    for ns in ["a.b", "a.c"] {
        repo.create(
            ws,
            user,
            &egret_workflow::CreateFlowInput {
                namespace: ns.into(),
                flow_id: "same".into(),
                source: "id: same\n".into(),
                plan: serde_json::json!({}),
                folder_id: None,
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{ns}: {e}"));
    }
}
```

Create `crates/egret-workflow/tests/common/mod.rs` following the pattern in `crates/egret-dataapi/tests/` — read one of those files and copy its `db()` and workspace-seeding helpers rather than inventing new ones. `db()` must return `None` (skipping) only when `egret_test_db::url()` says so, so the suite is honest about not running.

- [ ] **Step 4: Run and confirm the tests fail for the right reason**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test -p egret-workflow --test repo
```
Expected: FAIL to compile — `FlowRepo` does not exist.

**Check the duration when they later pass.** A DB-gated test with no database reports `ok` having asserted nothing; only its duration distinguishes a real run from a silent skip. Record the duration in your report.

- [ ] **Step 5: Implement `repo.rs`**

Add `sqlx`, `uuid`, `chrono`, `egret-core`, `egret-metastore` to `crates/egret-workflow/Cargo.toml`. Follow `crates/egret-dataapi/src/endpoints.rs` for shape: `sqlx::query_as(concat!(...))` with a `&'static str`, `Result<T> = std::result::Result<T, egret_core::Error>`, and a `db_err` mapping helper.

`insert_version` computes the next version with
`SELECT COALESCE(MAX(version), 0) + 1 FROM workflow.flow_versions WHERE flow_id = $1` **inside the same statement** as the insert, not as a separate query — two publishes a millisecond apart must not both get version 2.

- [ ] **Step 6: Run the tests**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test -p egret-workflow --test repo
```
Expected: PASS, 3 tests, in a duration that is clearly not zero.

- [ ] **Step 7: Prove the version race is closed**

Write and run this additional test, then keep it:

```rust
#[tokio::test]
async fn concurrent_publishes_do_not_collide_on_a_version_number() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = repo
        .create(ws, user, &egret_workflow::CreateFlowInput {
            namespace: "a.b".into(), flow_id: "race".into(),
            source: "id: race\n".into(), plan: serde_json::json!({}), folder_id: None,
        })
        .await
        .expect("create");

    let mut set = tokio::task::JoinSet::new();
    for i in 0..8 {
        let r = repo.clone();
        set.spawn(async move {
            r.insert_version(f.id, user, &serde_json::json!({ "i": i }), "").await
        });
    }
    let mut versions = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(Ok(v)) = res {
            versions.push(v.version);
        }
    }
    versions.sort_unstable();
    let before = versions.len();
    versions.dedup();
    assert_eq!(versions.len(), before, "two versions got the same number: {versions:?}");
}
```

`FlowRepo` must therefore be `Clone`. Expected: PASS. If it fails, the version number is being computed in a separate round trip — fix that, do not retry the test.

- [ ] **Step 8: Full check and commit**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test -p egret-workflow --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
git add migrations crates/egret-workflow
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): flows and flow_versions tables with their repository

Draft/version split for the reason 0121 records for dataapi.endpoints: if
execution read the draft, editing a flow would change tonight's scheduled run and
versioning would be decoration.

Version numbers are allocated inside the INSERT, not in a prior SELECT --
concurrent_publishes_do_not_collide_on_a_version_number is the test that would
catch the two-round-trip version."
```

---

### Task 6: Publish and reference resolution

**Files:**
- Create: `crates/egret-workflow-api/src/port.rs`, `crates/egret-workflow/src/publish.rs`
- Create: `crates/egret-workflow/tests/publish.rs`
- Modify: `crates/egret-workflow-api/src/lib.rs`, `crates/egret-workflow-api/Cargo.toml`, `crates/egret-workflow/src/lib.rs`

**Interfaces:**
- Consumes: `Flow`, `validate`, `registry::{known_type, RefKind}`, `FlowRepo`.
- Produces: `trait WorkflowRefPort { async fn workspace_of(&self, kind: RefKind, name_or_id: &str) -> Result<Option<(Uuid, Uuid)>> }` returning `(object_id, owning_workspace)`; `publish(repo: &FlowRepo, refs: &dyn WorkflowRefPort, ws: Uuid, by: UserId, flow: Uuid, note: &str) -> Result<FlowVersionRow>`.

- [ ] **Step 1: Write the failing test — the security one first**

Create `crates/egret-workflow/tests/publish.rs`:

```rust
//! Publish, and the reference-resolution seam. Requires DATABASE_URL.

mod common;

use egret_workflow::{RefKind, WorkflowRefPort};
use uuid::Uuid;

/// A fake that owns objects in a NAMED workspace, so a test can put an object in
/// a different workspace than the caller's and watch publish refuse.
#[derive(Default)]
struct FakeRefs {
    objects: std::collections::HashMap<(RefKind, String), (Uuid, Uuid)>,
}

#[async_trait::async_trait]
impl WorkflowRefPort for FakeRefs {
    async fn workspace_of(
        &self,
        kind: RefKind,
        name: &str,
    ) -> Result<Option<(Uuid, Uuid)>, egret_core::Error> {
        Ok(self.objects.get(&(kind, name.to_owned())).copied())
    }
}

/// THE test of this task. Migration 0128 shipped a plain FK with no workspace
/// predicate and a Cedar check that was told the CALLER's workspace, never the
/// object's; five columns had that shape. A flow is a machine for creating more
/// of them, so a foreign reference must make the flow UNPUBLISHABLE rather than
/// something a handler must remember to refuse later.
#[tokio::test]
async fn a_flow_referencing_another_workspace_cannot_be_published() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let other_ws = Uuid::new_v4();

    let mut refs = FakeRefs::default();
    refs.objects.insert(
        (RefKind::Database, "trino_prod".into()),
        (Uuid::new_v4(), other_ws), // owned by SOMEONE ELSE
    );

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sql\n    database: trino_prod\n    sql: SELECT 1\n",
    )
    .await;

    let err = egret_workflow::publish(&repo, &refs, ws, user, f.id, "")
        .await
        .expect_err("publishing a foreign reference must fail");

    // NotFound, not Forbidden: absence and denial must be indistinguishable, or
    // publish becomes a probe for which objects exist in other workspaces.
    assert!(
        matches!(err, egret_core::Error::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
}

/// The same refusal for an id that never existed anywhere. If these two produced
/// different errors, the difference would be the probe.
#[tokio::test]
async fn an_unknown_reference_fails_the_same_way_as_a_foreign_one() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sql\n    database: nope\n    sql: SELECT 1\n",
    )
    .await;

    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, f.id, "")
        .await
        .expect_err("must fail");
    assert!(matches!(err, egret_core::Error::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn a_resolvable_flow_publishes_and_freezes_its_references() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let db_id = Uuid::new_v4();

    let mut refs = FakeRefs::default();
    refs.objects.insert((RefKind::Database, "trino_prod".into()), (db_id, ws));

    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sql\n    database: trino_prod\n    sql: SELECT 1\n",
    )
    .await;

    let v = egret_workflow::publish(&repo, &refs, ws, user, f.id, "first").await.expect("publish");
    assert_eq!(v.version, 1);

    // The snapshot must be self-contained: the RESOLVED id, not the name.
    let s = v.snapshot.to_string();
    assert!(s.contains(&db_id.to_string()), "snapshot must carry the resolved id: {s}");

    let live = repo.get(ws, f.id).await.expect("get");
    assert_eq!(live.live_version_id, Some(v.id));
    assert_eq!(live.state, "live");
}

/// An unknown task type contributes no references, so it would resolve
/// vacuously. Publish must refuse it instead -- otherwise a typo'd type
/// publishes cleanly and fails only at run time, in W3, with no author present.
#[tokio::test]
async fn an_unknown_task_type_cannot_be_published() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    let f = common::draft(
        &repo, ws, user,
        "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: egret.sqll\n    sql: SELECT 1\n",
    )
    .await;

    let err = egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, f.id, "")
        .await
        .expect_err("an unknown type must not publish");
    assert!(matches!(err, egret_core::Error::InvalidInput(_)), "{err:?}");
}

/// Validation errors block publish; warnings do not. A flow declaring
/// `concurrency` must publish -- it just is not enforced yet.
#[tokio::test]
async fn warnings_do_not_block_publish_but_errors_do() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());

    let warns = common::draft(
        &repo, ws, user,
        "id: w\nnamespace: a.b\nconcurrency:\n  limit: 1\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n",
    )
    .await;
    egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, warns.id, "")
        .await
        .expect("a warning must not block publish");

    let bad = common::draft(
        &repo, ws, user,
        "id: e\nnamespace: BAD\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n",
    )
    .await;
    assert!(egret_workflow::publish(&repo, &FakeRefs::default(), ws, user, bad.id, "")
        .await
        .is_err());
}
```

Add a `draft()` helper to `crates/egret-workflow/tests/common/mod.rs` that parses the source, creates a draft through `FlowRepo`, and returns the row.

- [ ] **Step 2: Run and confirm the tests fail to compile**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test && cargo test -p egret-workflow --test publish`
Expected: FAIL to compile — `publish` and `WorkflowRefPort` do not exist.

- [ ] **Step 3: Define the port**

`crates/egret-workflow-api/src/port.rs`:

```rust
use uuid::Uuid;

use crate::RefKind;

/// What publish asks the rest of egret about the objects a flow names.
///
/// # Scoping — an implementation MUST honour this
///
/// `name_or_id` comes from a flow's YAML, so it is author-controlled. An
/// implementation returns the object's id and **its own** owning workspace, and
/// never filters by a caller-supplied workspace. The caller compares and refuses
/// a mismatch exactly as it refuses `None` — see `publish`. Splitting it this way
/// is deliberate: it mirrors `DataApiEndpointPort::workspace_of`, and it means a
/// caller that forgets its own check still cannot be told about another
/// workspace's objects, because it never learns anything but a uuid it already
/// has to compare.
#[async_trait::async_trait]
pub trait WorkflowRefPort: Send + Sync {
    async fn workspace_of(
        &self,
        kind: RefKind,
        name_or_id: &str,
    ) -> Result<Option<(Uuid, Uuid)>, egret_core::Error>;
}
```

Move `RefKind` from `egret-workflow`'s registry into `egret-workflow-api` so the port crate stays a leaf that does not depend on the engine, and re-export it from `egret-workflow`.

- [ ] **Step 4: Implement `publish`**

`crates/egret-workflow/src/publish.rs`. The order is the design:

1. load the draft, scoped to `ws`;
2. `parse(source)` — a parse failure is `InvalidInput`;
3. `validate(&flow)` — any **error** is `InvalidInput`; warnings are carried into the snapshot but do not block;
4. walk every task; for each, `registry::known_type(&task.r#type)` — `None` is `InvalidInput`, naming the type;
5. for each declared `RefField`, read the leaf spec's field, call `refs.workspace_of(kind, value)`, and refuse both `None` **and** `Some((_, other_ws))` as `NotFound` with the same message;
6. rewrite the resolved fields into the plan;
7. `insert_version` with a snapshot of `{ source, plan (resolved), warnings }`;
8. `set_live`.

Steps 5 and 6 are the whole point: after 6, the snapshot contains no name that still has to be looked up.

- [ ] **Step 5: Run the tests**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test && cargo test -p egret-workflow --test publish`
Expected: PASS, 6 tests, in a non-trivial duration.

- [ ] **Step 6: Prove the workspace check is load-bearing (mutation)**

Change step 5's refusal so it accepts any `Some((id, _))` regardless of the owning workspace. Rebuild (expect 0 errors — it compiles fine, which is exactly why this bug class keeps happening), and confirm `a_flow_referencing_another_workspace_cannot_be_published` FAILS while every other test still passes. Restore with `git checkout --`, re-run green, and record both outputs.

This is the single most important mutation in W1. A cross-workspace leak compiles, passes every test that does not specifically look for it, and returns 200.

- [ ] **Step 7: Full check and commit**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test -p egret-workflow --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
git add crates/egret-workflow crates/egret-workflow-api
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): publish with reference resolution

Every reference a flow makes to an egret object is resolved and workspace-checked
at publish, then frozen into the snapshot; the published artifact contains no
unresolved name. A flow referencing another workspace is UNPUBLISHABLE rather than
something each handler must remember to refuse later -- migration 0128 is the
recorded case of the alternative, where five columns carried a plain FK and the
Cedar check that followed was told the caller's workspace, never the object's.

A foreign reference and a non-existent one fail identically, as NotFound: a
distinguishable response would make publish a probe for other workspaces."
```

---

### Task 7: Routes and server wiring

**Files:**
- Create: `crates/egret-workflow/src/routes.rs`, `crates/egret-workflow/tests/routes.rs`
- Modify: `crates/egret-workflow/src/lib.rs`, `crates/egret-workflow/Cargo.toml`, `crates/egret-server/src/lib.rs`, `openapi.json`

**Interfaces:**
- Consumes: `FlowRepo`, `publish`, `parse`, `validate`.
- Produces: `WorkflowState { repo: FlowRepo, refs: Arc<dyn WorkflowRefPort> }`; `pub fn management_router() -> Router<WorkflowState>`; `pub fn openapi() -> utoipa::openapi::OpenApi`.

- [ ] **Step 1: Write the failing test**

Create `crates/egret-workflow/tests/routes.rs` covering: create returns 201 with the validation report; `POST /validate` on unsaved YAML returns findings without persisting; publish returns the version; a flow in another workspace returns 404 from `GET /flows/{id}`; and the entitlement gate. Model it on `crates/egret-dataapi/tests/` — read one of those files first and reuse its harness rather than building a new one.

Include this test specifically:

```rust
/// The gate W0 registered must actually bite now that routes exist. Before this
/// task the rule was inert because nothing matched it; a route landing before its
/// rule is what fails OPEN.
#[tokio::test]
async fn the_workflows_entitlement_gates_these_routes() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::disable_feature(&db, ws, "workflows").await;

    let app = common::app(&db).await;
    let res = common::get(&app, &format!("/api/ws/{ws}/workflows/flows"), user).await;
    assert_eq!(res.status(), 403, "a disabled module must not serve its routes");
}
```

- [ ] **Step 2: Run and confirm it fails**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test && cargo test -p egret-workflow --test routes`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the routes**

`management_router()` returns `Router<WorkflowState>` with paths under `/ws/{workspace}/workflows/...` — **not** `/api/ws/...`, because `egret-server` nests the router under `/api`. Copy the shape from `crates/egret-dataapi/src/routes.rs:2307`, including its comment about `WsContext`'s feature gate only recognising a path beginning `/api/ws/` or `/ws/`.

The nine routes from spec §10. Each handler takes `WsContext` and checks `workflow:manage` for the mutating ones, `workflow:read` for the rest, following how `egret-dataapi`'s handlers do it.

- [ ] **Step 4: Wire into `egret-server`**

In `crates/egret-server/src/lib.rs`, beside the existing `.nest("/api", egret_dataapi::management_router())` at line ~365, add the workflow router, and merge its OpenAPI beside `doc.merge(egret_dataapi::openapi())` at line ~158.

- [ ] **Step 5: Regenerate `openapi.json`**

Adding routes changes the committed spec, and a byte-identity gate in `egret-server` asserts it. Regenerate rather than hand-editing:

The generator is the `egret` binary's `openapi` subcommand — the drift test's own
failure message names it (`crates/egret-server/src/lib.rs:631`), and the
subcommand is pure: it emits the spec and exits before touching config or the
database (`crates/egret-bin/src/main.rs:37-40`). Serialization must match it
exactly: pretty-printed with a trailing newline.

```bash
cargo run -q -p egret-bin --bin egret -- openapi > openapi.json
env -u DATABASE_URL EGRET_ALLOW_DB_SKIP=1 \
  cargo test -p egret-server committed_openapi_json_is_current
git diff --stat openapi.json
```

Expected: the test passes and the diff shows only the new workflow paths. **Never
hand-edit `openapi.json`** — the gate compares it byte-for-byte against what
`openapi_doc()` emits, so a hand edit either fails immediately or, worse, matches
by luck while the generator disagrees.

One trap worth knowing: this gate lives in `egret-server`, a *different* crate
from the one you edited. Doc comments that feed `utoipa` change the committed
spec from anywhere in the workspace, which is how the W0 rename moved four lines
of it by rewording prose.

- [ ] **Step 6: Run everything**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test -p egret-workflow --no-fail-fast
cargo test -p egret-server --no-fail-fast
```
Expected: PASS.

- [ ] **Step 7: The workspace-wide regression gate**

```bash
env -u DATABASE_URL EGRET_ALLOW_DB_SKIP=1 cargo test --workspace --no-fail-fast \
  > /tmp/w1-t7-after.log 2>&1; echo "EXIT=$?"
grep -c '^test result:' /tmp/w1-t7-after.log
grep -E '^test result:' /tmp/w1-t7-after.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'
grep -E '^test .* FAILED$' /tmp/w1-t7-after.log | sort -u > /tmp/w1-t7-after.failed
diff .superpowers/sdd/2026-09-01-workflow-w0-foundation/baseline.failed /tmp/w1-t7-after.failed \
  && echo "IDENTICAL FAILURE SET"
```

Expected: binary count well above 249 (the new test binaries), and `IDENTICAL FAILURE SET`. A count of 3 means `just test-no-db` was used by mistake; a count of 0 means the run died.

- [ ] **Step 8: Full check and commit**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
git add crates/egret-workflow crates/egret-server openapi.json
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): flow management routes

Nine routes under /api/ws/{ws}/workflows, claimed by the workflows entitlement W0
registered before any route existed. POST /validate is its own route because the
editor calls it on every keystroke-pause and must not require a saved draft.

openapi.json moves because a byte-identity gate in egret-server asserts it --
regenerated, not hand-edited."
```

---

## Done when

- `cargo test -p egret-workflow --no-fail-fast` fully green against `egret_test`, in a duration that proves the DB tests ran.
- `cargo test --workspace --no-fail-fast` reaches more binaries than the 249 baseline and its FAILED-name set is identical to `.superpowers/sdd/2026-09-01-workflow-w0-foundation/baseline.failed`.
- `cargo fmt --check` and `cargo clippy -D warnings` clean.
- `cargo test -p egret-server committed_openapi_json_is_current` passes.
- A flow referencing another workspace cannot be published, and fails identically to one referencing nothing.
- Every mutation named in Tasks 1, 2, 3, 4 and 6 was run, **compiled**, and observed to fail.

Open the PR and **stop**. Do not merge.

## Deliberately not in W1

No executor, scheduler, task handler, or expression evaluation. No `workflow.executions`, `task_runs`,
`triggers`, or `task_logs` tables. No frontend — W7 owns it.
