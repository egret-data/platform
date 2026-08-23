# M1 — the declared columns are the union of the branches Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A conditional Data API endpoint stops publishing green and then `503`-ing on its
own documented default call, by describing every branch at publish and enforcing a subset
contract at serving.

**Architecture:** Publish already renders many parameter permutations for the injection
gate but describes only one. M1 renders the same permutations, **deduplicates by the
rendered SQL string**, describes each distinct one, and freezes the union with a
`required` flag per column. Serving's `shape_rows` changes from "the live columns equal
the frozen columns" to "the live columns are a subset, every `required` one is present, and
the types match".

**Tech Stack:** Rust, `crates/egret-dataapi` + `crates/egret-dataapi-api`, sqlx/JSONB
snapshots, minijinja renderer.

**Spec:** `docs/en/specs/2026-08-23-dataapi-multi-statement-design.md` §3. Read §1.3 for
the defect being fixed.

## Global Constraints

- **The single-statement gate is untouched.** `#` stays forbidden, `;` stays rejected by
  `reject_unscannable_bytes`, by the comment/literal scan at `lifecycle.rs:601`, and after
  render at `render.rs:856`. **If any existing `;` or `#` test needs editing to pass, the
  change is wrong.**
- **`ServingMeta.columns` keeps its meaning and its doc comment**: the frozen contract, not
  this execution's shape. It is now the union. Do not change it to per-execution.
- **`output.shape: "arrays"` may not branch** — refused at publish (Task 5). Every other
  change in this plan is confined to the `objects` path.
- **Fail closed.** Every new mismatch is `ServingError::SchemaDrift` (`503`), never a
  warning and never a silent pass.
- **A non-conditional template must cost exactly one `describe_as_with_params`.** Asserted
  as a count, not as "at least one".
- **`ALLOWED_METHODS = ["GET", "POST"]`** unchanged; read-only unchanged.
- Backend: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, and the
  full test binaries must be green before each commit. Export
  `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` — **never** the dev
  `egret` database.

## Fixtures that already exist — use these, do not invent helpers

Read these before writing a test. Every name below was verified on disk; anything not
listed here, take from the surrounding file's own pattern rather than inventing it.

- **`crates/egret-dataapi/tests/lifecycle.rs`** — the publish suite.
  - `FakePort` (`:50`) is a `QueryExecPort` under the test's control that also **records**
    every call: `seen: Mutex<Vec<(Uuid, Uuid, Uuid, String)>>`, so `call_count()` (`:97`),
    `last_sql()` (`:88`) and `last_param_count()` (`:100`) already exist. **A counting port
    does not need to be written.**
  - `enum Describe` (`:34`) — `Columns(Vec<(&'static str, &'static str)>)`, `Forbidden`,
    `BadSql`, `Missing`. Set with `f.port.set(Describe::Columns(vec![("id","uuid")]))`.
  - Fixture: `f.create_endpoint(name, path, sql)`, `f.publish(ep)`, `f.publish_raw(ep)`,
    `f.live_snapshot(ep)`.
- **`crates/egret-dataapi/tests/serving_common/mod.rs`** — the serving fixture, with
  `col(name)` (`:181`), `Fixture`, `publish_endpoint(method, path, sql)` (`:473`),
  `call(path)` (`:406`), `call_as(sa, path)`.
- **`crates/egret-dataapi/src/serving/router.rs:1360`** — `snapshot_with_limits(...)`
  builds an `EndpointSnapshot` inside the private `mod tests`. `shape_rows` is private, so
  Task 6's tests belong in that module and should build snapshots the same way.
- **`skip_without_db!()`** — the macro each DB-backed test binary defines for itself
  (`endpoints_schema.rs:70`, `audit.rs:271`, …).

**A DB-gated test that skips reports `ok` while asserting nothing.** Only its *duration*
distinguishes a skip from a pass. Always
`export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` before running, and
when a new test passes suspiciously fast, confirm it actually ran.

---

### Task 1: `ColumnSpec` carries `required`, and old snapshots stay exact

**Files:**
- Modify: `crates/egret-dataapi-api/src/lib.rs:242-257`
- Test: same file, `#[cfg(test)]`

**Interfaces:**
- Produces: `ColumnSpec { name: String, ty: String, required: bool }`, and
  `ColumnSpec::from(&ColumnMeta)` yielding `required: true`.

Snapshots are persisted as `jsonb` (`migrations/0121_dataapi_endpoints.sql:55`), so a
snapshot written before this task has no `required` key. It must default to **`true`**, not
`false`: those columns all came from the single describe that every row of the old contract
was checked against, so `true` reproduces today's exact-match behaviour for every
already-published endpoint. Defaulting to `false` would silently loosen the gate on every
endpoint in the fleet until it was republished.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_snapshot_written_before_required_existed_deserializes_as_required() {
    // Exactly the shape migration 0121 rows carry today.
    let old = r#"{"name":"id","ty":"integer"}"#;
    let c: ColumnSpec = serde_json::from_str(old).unwrap();
    assert!(
        c.required,
        "a column frozen before M1 was checked for equality on every call; \
         defaulting it to optional would loosen the gate on the whole fleet"
    );
}

#[test]
fn a_described_column_is_required_until_publish_says_otherwise() {
    let meta = egret_core::ColumnMeta { name: "id".into(), ty: "integer".into() };
    assert!(ColumnSpec::from(&meta).required);
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p egret-dataapi-api required`
Expected: FAIL — `missing field \`required\`` and `no field \`required\``.

- [ ] **Step 3: Implement**

```rust
fn default_required() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ColumnSpec {
    pub name: String,
    pub ty: String,
    /// Present in EVERY permutation described at publish.
    ///
    /// Defaults to `true` when absent, which is what a snapshot written before
    /// this field existed carries. Those columns were checked for equality on
    /// every call, so `true` is the only default that does not loosen an
    /// already-published contract.
    #[serde(default = "default_required")]
    pub required: bool,
}

impl From<&egret_core::ColumnMeta> for ColumnSpec {
    fn from(c: &egret_core::ColumnMeta) -> Self {
        Self {
            name: c.name.clone(),
            ty: c.ty.clone(),
            // Publish downgrades this to `false` for a column that only some
            // permutation returns; a single describe means every column is
            // required, which is the pre-M1 behaviour.
            required: true,
        }
    }
}
```

- [ ] **Step 4: Run the test, then the crate**

Run: `cargo test -p egret-dataapi-api`
Expected: PASS. Fix every other crate that constructs `ColumnSpec` literally
(`cargo build --workspace --all-targets` will name them).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(dataapi): ColumnSpec carries required, defaulting true for old snapshots"
```

---

### Task 2: a Boolean parameter is probed with both of its values

**Files:**
- Modify: `crates/egret-dataapi/src/render.rs:362-378` (`ParamValues::candidates`)
- Test: same file

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `candidates()` returns `[Bool(true), Bool(false)]` for a Boolean parameter.

This is why the §1.3 defect could not have been found by the existing machinery.
`candidates()` returns an enum's `allowed` entries, and **for everything else the
definition's literals**. `SELECT id {% if detail %}, name, email {% endif %} FROM users`
contains no literals, so `detail` gets no candidates at all and only ever renders `true`.
No amount of describing permutations helps if the permutation that breaks is never
enumerated.

**This deliberately widens `check_dynamic` too**, which is the other consumer of
`probe_assignments`. That is a gain — the injection gate now also probes the `false` branch
— and it stays inside `MAX_PROBES = 64`. It is called out here so a reviewer sees it as
intended rather than as a side effect.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_boolean_parameter_is_probed_with_both_values() {
    let spec = ParamSpec {
        name: "detail".into(),
        ty: ParamType::Boolean,
        required: false,
        default: None,
        allowed: None,
        ..Default::default()
    };
    let mut ctx = ParamValues::new();
    ctx.insert_for_spec(&spec, Param::Bool(true));

    // A definition with no literals in it at all — the shape that hid the defect.
    let cands = ctx.candidates("detail", &[]);
    assert!(
        cands.contains(&Param::Bool(false)),
        "a boolean whose definition has no literals was never probed with false, \
         so the branch it guards was never described"
    );
    assert!(cands.contains(&Param::Bool(true)));
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p egret-dataapi probed_with_both_values`
Expected: FAIL — `cands` is empty.

- [ ] **Step 3: Implement**

In `candidates`, branch on the cell's parameter type before falling through to literals:

```rust
fn candidates(&self, name: &str, literals: &[Param]) -> Vec<Param> {
    let cell = self
        .entries
        .get(name)
        .and_then(|v| v.downcast_object_ref::<Cell>());
    // A boolean's value space is closed and has exactly two members, so
    // enumerate it rather than hoping the definition mentions `true`/`false`
    // as literals. Without this a `{% if flag %}` branch is never rendered
    // with the flag off, and publish describes only the branch it happened to
    // take.
    if matches!(cell.map(|c| &c.value), Some(Param::Bool(_))) {
        return vec![Param::Bool(true), Param::Bool(false)];
    }
    match cell.and_then(|c| c.allowed.clone()) {
        Some(list) => list
            .into_iter()
            .take(MAX_ENUM_CANDIDATES)
            .map(Param::Str)
            .collect(),
        None => literals.to_vec(),
    }
}
```

If `Cell` does not expose `value` under that name, read `Cell`'s definition
(`render.rs` around line 273) and use whatever it does expose — **do not add a field**.

- [ ] **Step 4: Run the test, then the crate's render tests**

Run: `cargo test -p egret-dataapi render`
Expected: PASS, including every existing `check_dynamic` test.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "fix(dataapi): probe a boolean parameter with both of its values"
```

---

### Task 3: one function returns every distinct statement a definition can render to

**Files:**
- Modify: `crates/egret-dataapi/src/render.rs` (new `pub fn`, near `check_dynamic:1171`)
- Test: same file

**Interfaces:**
- Consumes: `probe_assignments` (private, `render.rs:1213`), `render_inner`.
- Produces:
  ```rust
  pub fn render_permutations(sql: &str, ctx: &ParamValues) -> Vec<(String, Vec<Param>)>
  ```
  Distinct by rendered SQL, in first-seen order. Assignments that fail to render are
  skipped, exactly as `check_dynamic:1173-1176` skips them and for the same reason: publish
  refuses them through its own render of the representative values, with the real error.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_definition_with_no_branch_renders_to_exactly_one_statement() {
    // The literals matter. `{{ region }}` emits a bind marker, so every value of
    // `region` renders to the SAME statement — and `candidates()` harvests a
    // non-enum, non-boolean parameter's candidates from the definition's own
    // literals. Without them `probe_assignments` returns a single assignment and
    // this test passes with the deduplication DELETED, asserting nothing.
    let sql = "SELECT id FROM t WHERE r = {{ region }} AND a = 'EU' AND b = 'US' AND c = 'APAC'";
    let mut ctx = ParamValues::new();
    ctx.insert("region".into(), Param::Str("EU".into()));
    assert_eq!(
        render_permutations(sql, &ctx).len(),
        1,
        "deduplication must collapse every probe of a non-branching definition \
         to one statement, or publish silently pays for a describe per probe"
    );
}

#[test]
fn a_branching_definition_renders_to_one_statement_per_branch() {
    let sql = "SELECT id {% if detail %}, name {% endif %}FROM t";
    let mut ctx = ParamValues::new();
    ctx.insert("detail".into(), Param::Bool(true));
    let got = render_permutations(sql, &ctx);
    let rendered: Vec<&String> = got.iter().map(|p| &p.0).collect();
    assert_eq!(got.len(), 2, "got: {rendered:?}");
}
```

`ParamValues::insert` is the module's own constructor used throughout its tests
(`render.rs:267`); if the surrounding tests use `insert_for_spec` instead, follow them.
This task depends on Task 2 — without boolean candidates the second test returns 1.

**Measure before you assert.** Confirm `probe_assignments` really returns more than one
assignment for the first test's definition. If it returns one, the test is vacuous no
matter what it claims, and you must find a shape that is not.

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p egret-dataapi render_permutations`
Expected: FAIL — function does not exist.

- [ ] **Step 3: Implement**

```rust
/// Every DISTINCT statement this definition can render to, across the probe
/// assignments.
///
/// Deduplicated **by the rendered SQL string**, which is what makes publish
/// affordable: the probe budget is sixty-four assignments, but a definition
/// with no `{% if %}` renders all sixty-four to the same statement and costs
/// one describe — exactly what it cost before this existed. Cost tracks the
/// number of structurally distinct queries, not the number of parameters.
pub fn render_permutations(sql: &str, ctx: &ParamValues) -> Vec<(String, Vec<Param>)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for probe in probe_assignments(sql, ctx) {
        // Skipped for the same reason `check_dynamic` skips it: publish refuses
        // an unrenderable definition through its own render of the
        // representative values, and reports the real error from there.
        let Ok((rendered, binds)) = render_inner(sql, &probe) else {
            continue;
        };
        if seen.insert(rendered.clone()) {
            out.push((rendered, binds));
        }
    }
    out
}
```

- [ ] **Step 4: Run**

Run: `cargo test -p egret-dataapi render_permutations`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(dataapi): render_permutations returns each distinct statement once"
```

---

### Task 4: publish describes every branch and freezes their union

**Files:**
- Modify: `crates/egret-dataapi/src/lifecycle.rs:1526-1558` (`validate`)
- Modify: `crates/egret-dataapi/src/lifecycle.rs` — a new `ValidationError` variant
- Test: `crates/egret-dataapi/tests/` (the binary that already exercises `validate`)

**Interfaces:**
- Consumes: `render::render_permutations` (Task 3), `ColumnSpec::required` (Task 1),
  `representative(sql, params, absent)` (`lifecycle.rs:1144`).
- Produces: `validate` returns the union, `required` set correctly, and a new
  `ValidationError::ColumnTypeConflict { name, first, second }`.

**The permutation set is the probe assignments *plus* the absent-parameter contexts.**
`check_render_rules:1201-1229` already builds the absent contexts — one per optional
parameter with no default, and one with all of them absent — because an omitted parameter
takes a different branch than a present one. Describing only `probe_assignments(full_ctx)`
would miss exactly the `503` in spec §1.3 that comes from **omitting** the query string.

- [ ] **Step 1: Teach `FakePort` to answer per statement**

`Describe::Columns` returns one fixed list for every call, which cannot express "this
branch has three columns and that one has one". `FakePort` already records the SQL it was
given (`seen`, `:51`), so add one variant beside the existing four (`tests/lifecycle.rs:34`):

```rust
    /// Resolve by matching a SUBSTRING of the rendered statement, first match
    /// wins; the last entry is the fallback. Publish now describes several
    /// renderings of one definition, so a fake that answers the same columns for
    /// all of them cannot tell a union apart from a single describe.
    PerSql(Vec<(&'static str, Vec<(&'static str, &'static str)>)>),
```

and handle it in `describe_as` alongside `Describe::Columns`.

- [ ] **Step 2: Write the failing tests**

```rust
#[tokio::test]
async fn publishing_freezes_the_union_of_every_branch() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("name", vec![("id", "uuid"), ("name", "varchar")]),
        ("", vec![("id", "uuid")]),
    ]));
    let ep = f
        .create_endpoint(
            "cond",
            "reports/cond",
            "SELECT id {% if detail %}, name {% endif %}FROM t",
        )
        .await;
    // `detail` must be declared optional with no default — follow the parameter
    // shape `publish_refuses_each_parameter_rule_with_its_own_reason` (:1092) uses.
    f.publish(ep).await;

    let snap = f.live_snapshot(ep).await;
    let by = |n: &str| snap.columns.iter().find(|c| c.name == n).expect(n).clone();
    assert!(by("id").required, "id is in every branch");
    assert!(
        !by("name").required,
        "name appears only under `detail`, so it is declared and optional"
    );
}

#[tokio::test]
async fn a_column_whose_type_differs_between_branches_refuses_to_publish() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("name", vec![("v", "varchar")]),
        ("", vec![("v", "integer")]),
    ]));
    let ep = f
        .create_endpoint(
            "conflict",
            "reports/conflict",
            "SELECT {% if detail %}name{% else %}id{% endif %} AS v FROM t",
        )
        .await;
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    assert!(
        body["message"].as_str().unwrap_or_default().contains("two types"),
        "the message must name the fix, not just the fault: {body}"
    );
}

#[tokio::test]
async fn every_allowed_enum_value_is_described_not_only_the_first() {
    // `representative_value` picks `allowed[0]` (lifecycle.rs:1114), so before
    // this task the ORDER the author wrote `allowed` in decided the published
    // contract. Reversing it must now change nothing.
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("name", vec![("id", "uuid"), ("name", "varchar")]),
        ("", vec![("id", "uuid")]),
    ]));
    let ep = f
        .create_endpoint(
            "enum",
            "reports/enum",
            "SELECT id {% if mode | value == 'full' %}, name {% endif %}FROM t",
        )
        .await;
    // `mode` enum, allowed = ["summary", "full"].
    f.publish(ep).await;
    let snap = f.live_snapshot(ep).await;
    assert!(
        snap.columns.iter().any(|c| c.name == "name"),
        "the `full` branch is not allowed[0], and it was never described before M1"
    );
}
```

- [ ] **Step 3: Run and watch them fail**

Run: `cargo test -p egret-dataapi --test lifecycle union_of_every_branch differs_between_branches every_allowed_enum`
Expected: FAIL — the first and third freeze only `[id]`, the second publishes `200`.

- [ ] **Step 4: Implement**

Replace the single-describe body of `validate` (currently `lifecycle.rs:1543-1557`):

```rust
    // Every context that can take a different branch: the representative values,
    // and each optional-parameter-absent context. `check_render_rules` builds the
    // same absent set for its own rules — an omitted parameter is a branch, and
    // it is the branch spec §1.3's failing call takes.
    let optional: Vec<&str> = draft
        .params
        .iter()
        .filter(|p| !p.required && p.default.is_none())
        .map(|p| p.name.as_str())
        .collect();
    let mut contexts = vec![representative(&draft.sql, &draft.params, &[])];
    for name in &optional {
        contexts.push(representative(&draft.sql, &draft.params, &[name]));
    }
    if optional.len() > 1 {
        contexts.push(representative(&draft.sql, &draft.params, &optional));
    }

    let mut statements: Vec<(String, usize)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ctx in &contexts {
        for (rendered, binds) in crate::render::render_permutations(&draft.sql, ctx) {
            if seen.insert(rendered.clone()) {
                statements.push((rendered, binds.len()));
            }
        }
    }
    if statements.is_empty() {
        // The representative render is the one whose error is worth reporting.
        let ctx = representative(&draft.sql, &draft.params, &[]);
        let (rendered, binds) =
            crate::render::render(&draft.sql, &ctx).map_err(ValidationError::Render)?;
        statements.push((rendered, binds.len()));
    }

    // Union, in first-seen order, with `required` = present in EVERY described
    // statement. A name that describes with two different types is refused:
    // two types cannot be unioned into one honest declaration, and this is the
    // gap that served a 200 carrying an integer under a declared varchar.
    let mut union: Vec<ColumnSpec> = Vec::new();
    let mut per_statement: Vec<Vec<String>> = Vec::new();
    for (rendered, bind_count) in &statements {
        let cols = port
            .describe_as_with_params(workspace, draft.database_id, principal, rendered, *bind_count)
            .await?;
        if cols.is_empty() {
            return Err(ValidationError::DescribeReturnedNoColumns.into());
        }
        per_statement.push(cols.iter().map(|c| c.name.clone()).collect());
        for c in &cols {
            let spec = ColumnSpec::from(c);
            match union.iter().find(|u| u.name == spec.name) {
                Some(existing) if existing.ty != spec.ty => {
                    return Err(ValidationError::ColumnTypeConflict {
                        name: spec.name,
                        first: existing.ty.clone(),
                        second: spec.ty,
                    }
                    .into());
                }
                Some(_) => {}
                None => union.push(spec),
            }
        }
    }
    for col in &mut union {
        col.required = per_statement.iter().all(|names| names.contains(&col.name));
    }
    Ok(union)
```

Add the variant beside the other `ValidationError`s:

```rust
    #[error(
        "column `{name}` is `{first}` in one branch of this definition and `{second}` in \
         another. A published contract cannot declare two types for one name — give the \
         branches a `CAST` to the same type, or return them under different names"
    )]
    ColumnTypeConflict {
        name: String,
        first: String,
        second: String,
    },
```

- [ ] **Step 5: Run, and repair the existing freeze test**

Run: `cargo test -p egret-dataapi --test lifecycle`
Expected: PASS. `publishing_freezes_the_described_columns_and_the_canonical_path`
(`tests/lifecycle.rs:655`) asserts the frozen column list directly and will need
`required: true` on both columns — that is the correct new value for a definition with no
branch, **not** a test being bent to fit.

- [ ] **Step 6: Prove the type-conflict refusal with a mutation**

Replace the `ColumnTypeConflict` arm with `Some(_) => {}` so conflicting types are
silently unioned. **Build first and confirm zero errors** — a mutation that does not
compile looks exactly like one no test caught. Then confirm
`a_column_whose_type_differs_between_branches_refuses_to_publish` **fails**, and restore.

- [ ] **Step 7: Prove the cost claim**

Add a test asserting the describe **count**, using whatever counting fake the crate's other
`QueryExecPort` tests use:

```rust
#[tokio::test]
async fn a_non_conditional_definition_describes_exactly_once() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid")]));
    let ep = f
        .create_endpoint("plain", "reports/plain", "SELECT id FROM t WHERE r = {{ region }}")
        .await;
    let before = f.port.call_count();
    f.publish(ep).await;
    assert_eq!(
        f.port.call_count() - before, 1,
        "deduplication is the whole cost argument; `at least one` would pass with it removed"
    );
}
```

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "fix(dataapi): describe every branch at publish and freeze their union"
```

---

### Task 5: an `arrays` endpoint may not branch

**Files:**
- Modify: `crates/egret-dataapi/src/lifecycle.rs` (`validate`, after the union is built)
- Test: alongside Task 4's tests

**Interfaces:**
- Consumes: `statements` and `per_statement` from Task 4.
- Produces: `ValidationError::BranchingArraysShape`.

`shape_rows` compares the live and frozen lists **positionally** (`router.rs:1119-1124`)
and `data_schema` documents the column order in prose (`openapi_gen.rs:123-133`), both
because `arrays` is a positional tuple and OpenAPI 3.0 has no tuple type. A tuple whose
arity varies by parameter cannot be read by anyone: `[[1]]` under a declared
`[id, name, email]` is ambiguous. So it is refused, and every other change in this package
stays on the `objects` path.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn an_arrays_endpoint_that_branches_refuses_to_publish() {
    let f = skip_without_db!();
    f.port.set(Describe::PerSql(vec![
        ("name", vec![("id", "uuid"), ("name", "varchar")]),
        ("", vec![("id", "uuid")]),
    ]));
    let ep = f
        .create_endpoint(
            "arr",
            "reports/arr",
            "SELECT id {% if detail %}, name {% endif %}FROM t",
        )
        .await;
    // Set `output.shape = "arrays"` on the draft the way the suite's other
    // output-shape tests do — search `"arrays"` in this file for the pattern.
    let (status, body) = f.publish_raw(ep).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    assert!(
        body["message"].as_str().unwrap_or_default().contains("objects"),
        "the message must point at the fix: {body}"
    );
}

#[tokio::test]
async fn an_arrays_endpoint_that_does_not_branch_still_publishes() {
    let f = skip_without_db!();
    f.port.set(Describe::Columns(vec![("id", "uuid"), ("name", "varchar")]));
    let ep = f.create_endpoint("arr2", "reports/arr2", "SELECT id, name FROM t").await;
    // …same `output.shape = "arrays"`.
    f.publish(ep).await; // must not refuse: the rule costs a non-branching endpoint nothing
}
```

- [ ] **Step 2: Run and watch the first fail**

Run: `cargo test -p egret-dataapi --test lifecycle arrays_endpoint`
Expected: the first FAILs (publishes), the second PASSes.

- [ ] **Step 3: Implement**, immediately before `Ok(union)`:

```rust
    // `arrays` is positional: `shape_rows` zips the live and frozen lists by
    // index and `data_schema` documents the order in prose, because OpenAPI 3.0
    // has no tuple type. A tuple whose members vary by parameter is not a
    // contract — `[[1]]` under `[id, name, email]` cannot be read — so branching
    // is refused here rather than served ambiguously.
    if draft.output.shape == "arrays" && per_statement.iter().any(|n| n != &per_statement[0]) {
        return Err(ValidationError::BranchingArraysShape.into());
    }
```

```rust
    #[error(
        "this definition returns different columns depending on its parameters, which \
         `output.shape: arrays` cannot express — a positional row has no way to say which \
         column is missing. Use `output.shape: objects`, where a column is simply absent"
    )]
    BranchingArraysShape,
```

- [ ] **Step 4: Run**

Run: `cargo test -p egret-dataapi --test lifecycle arrays_endpoint`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(dataapi): refuse a branching projection under output.shape arrays"
```

---

### Task 6: serving accepts a subset, and compares types

**Files:**
- Modify: `crates/egret-dataapi/src/serving/router.rs:1113-1206` (`shape_rows`)
- Test: same file's `#[cfg(test)]`

**Interfaces:**
- Consumes: `ColumnSpec::required` (Task 1).
- Produces: unchanged signature.

The `arrays` path keeps its present positional check verbatim — Task 5 guarantees an
`arrays` endpoint cannot branch, so equality is still the right predicate there. Only the
`objects` path changes.

- [ ] **Step 1: Write the failing tests**

Write these in `router.rs`'s own `mod tests` (`:1268`), building snapshots the way
`snapshot_with_limits` (`:1360`) does — clone it into a small `snapshot_with_columns`
helper rather than repeating the literal four times.

```rust
#[test]
fn a_branch_that_omits_an_optional_column_is_served() {
    let snap = snapshot_with_columns(vec![
        ColumnSpec { name: "id".into(), ty: "integer".into(), required: true },
        ColumnSpec { name: "name".into(), ty: "varchar".into(), required: false },
    ]);
    let out = TabularResult {
        columns: vec![ColumnMeta { name: "id".into(), ty: "integer".into() }],
        rows: vec![vec![json!(1)]],
    };
    let v = shape_rows(&out, &snap).expect("an optional column may be absent");
    assert_eq!(v, json!([{"id": 1}]), "the key is absent, not null");
}

#[test]
fn a_missing_required_column_is_still_drift() {
    let snap = snapshot_with_columns(vec![
        ColumnSpec { name: "id".into(), ty: "integer".into(), required: true },
        ColumnSpec { name: "name".into(), ty: "varchar".into(), required: false },
    ]);
    let out = TabularResult {
        columns: vec![ColumnMeta { name: "name".into(), ty: "varchar".into() }],
        rows: vec![vec![json!("x")]],
    };
    assert!(matches!(shape_rows(&out, &snap), Err(ServingError::SchemaDrift)));
}

#[test]
fn an_undeclared_column_is_drift() {
    let snap = snapshot_with_columns(vec![
        ColumnSpec { name: "id".into(), ty: "integer".into(), required: true },
    ]);
    let out = TabularResult {
        columns: vec![
            ColumnMeta { name: "id".into(), ty: "integer".into() },
            ColumnMeta { name: "secret".into(), ty: "varchar".into() },
        ],
        rows: vec![vec![json!(1), json!("s")]],
    };
    assert!(
        matches!(shape_rows(&out, &snap), Err(ServingError::SchemaDrift)),
        "an undeclared column is a field nobody reviewed at publish reaching a caller"
    );
}

#[test]
fn a_declared_column_returning_the_wrong_type_is_drift() {
    let snap = snapshot_with_columns(vec![
        ColumnSpec { name: "v".into(), ty: "varchar".into(), required: true },
    ]);
    let out = TabularResult {
        columns: vec![ColumnMeta { name: "v".into(), ty: "integer".into() }],
        rows: vec![vec![json!(1)]],
    };
    assert!(
        matches!(shape_rows(&out, &snap), Err(ServingError::SchemaDrift)),
        "this is the gap that served 200 with an integer under a declared varchar"
    );
}
```

Check `TabularResult`'s real field names in `egret-core` before writing these; if it has
more than `columns` and `rows`, follow how the file's existing `shape_rows` tests build
one.

- [ ] **Step 2: Run and watch them fail**

Run: `cargo test -p egret-dataapi shape_rows`
Expected: the first FAILs with `SchemaDrift`, the last FAILs by returning `Ok`.

- [ ] **Step 3: Implement**

Keep everything above `if snap.output.shape == "arrays"` for the `arrays` path. For
`objects`, replace the positional drift check with:

```rust
    // Subset, not equality. A branch may legitimately omit a column declared
    // `required: false`; it may never return one that was never declared, and it
    // may never change a declared column's type.
    //
    // This check is the backstop for everything publish did not reach: the probe
    // budget is sixty-four assignments and a definition can branch on more. It
    // therefore FAILS CLOSED — an undeclared column is not a documentation
    // defect, it is a field nobody reviewed at publish reaching a caller.
    for live in &out.columns {
        match snap.columns.iter().find(|f| f.name == live.name) {
            None => return Err(drift(snap, out, "a column that was never declared")),
            Some(frozen) if frozen.ty != live.ty => {
                return Err(drift(snap, out, "a declared column changed type"))
            }
            Some(_) => {}
        }
    }
    for frozen in snap.columns.iter().filter(|c| c.required) {
        if !out.columns.iter().any(|l| l.name == frozen.name) {
            return Err(drift(snap, out, "a required column was not returned"));
        }
    }
```

with one helper beside `shape_rows`, so every refusal logs the same way the present one
does:

```rust
fn drift(
    snap: &EndpointSnapshot,
    out: &egret_core::TabularResult,
    why: &str,
) -> ServingError {
    tracing::warn!(
        frozen = ?snap.columns.iter().map(|c| &c.name).collect::<Vec<_>>(),
        live = ?out.columns.iter().map(|c| &c.name).collect::<Vec<_>>(),
        why,
        "published output contract no longer matches the engine's columns"
    );
    ServingError::SchemaDrift
}
```

Then build the object from the **live** columns rather than the frozen ones:

```rust
    // The live names, because a branch's response carries the keys that branch
    // returned. The collision check below still runs over the FROZEN union, so
    // serving and the generated document agree about naming for every branch,
    // not only for the one this call took.
    let live_names: Vec<String> = out
        .columns
        .iter()
        .map(|c| apply_naming(&c.name, &snap.output.naming))
        .collect();
```

Keep the existing duplicate-key check, but run it over the frozen union's mapped names —
`names` in the present code — because `output.naming` collapsing two *declared* columns
onto one key must refuse regardless of which branch is being served. Keep the row-arity
guard, comparing against `out.columns.len()`.

- [ ] **Step 4: Run**

Run: `cargo test -p egret-dataapi`
Expected: PASS, including every existing `shape_rows` and naming-collision test.

- [ ] **Step 5: Prove the type check with a mutation**

Delete the `frozen.ty != live.ty` arm, rebuild, and confirm
`a_declared_column_returning_the_wrong_type_is_drift` **fails**. A mutation that does not
compile proves nothing — assert the build succeeds first, then restore the arm.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "fix(dataapi): serve a subset of the declared columns, and compare types"
```

---

### Task 7: the generated document says which columns are guaranteed

**Files:**
- Modify: `crates/egret-dataapi/src/openapi_gen.rs:121-156` (`data_schema`)
- Test: the crate's existing `openapi_gen` tests

**Interfaces:**
- Consumes: `ColumnSpec::required`.
- Produces: `data_schema` emits a `required` array for the `objects` shape.

The `arrays` branch is unchanged — Task 5 refuses a branching `arrays` endpoint, so its
prose column order stays true.

- [ ] **Step 1: Write the failing test**

`tests/openapi_gen.rs` already builds snapshots for this generator — follow its existing
builder rather than adding one.

```rust
#[tokio::test]
async fn only_the_columns_present_in_every_branch_are_required() {
    let snap = snapshot_with_columns(vec![
        ColumnSpec { name: "id".into(), ty: "integer".into(), required: true },
        ColumnSpec { name: "name".into(), ty: "varchar".into(), required: false },
    ]);
    let doc = data_schema(&snap, "e").expect("a branching endpoint has a document");
    assert_eq!(doc["required"], json!(["id"]));
    assert!(
        doc["properties"]["name"].is_object(),
        "an optional column is still documented — it is reachable, just not guaranteed"
    );
}
```

`data_schema` is private to `openapi_gen.rs`; if `tests/openapi_gen.rs` only reaches the
public document builder, assert against the built document's
`components`/`paths` fragment instead of calling `data_schema` directly.

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p egret-dataapi only_the_columns_present_in_every_branch`
Expected: FAIL — `required` is `null`.

- [ ] **Step 3: Implement**, replacing the final `Ok(json!(...))` of `data_schema`:

```rust
    // Every declared column is a property; only the ones present in every
    // described branch are `required`. This is what stops the document
    // advertising a call that 503s: before M1 a conditional endpoint declared
    // one branch's columns as if they were the whole contract.
    let required: Vec<&str> = snap
        .columns
        .iter()
        .filter(|c| c.required)
        .map(|c| c.name.as_str())
        .collect();
    let mut schema = json!({ "type": "object", "properties": props });
    if !required.is_empty() {
        schema["required"] = json!(required
            .iter()
            .map(|n| crate::serving::router::apply_naming(n, &snap.output.naming))
            .collect::<Vec<_>>());
    }
    Ok(schema)
```

Note the `apply_naming` call: the `required` list must name the keys a consumer actually
receives, for the same reason `props` does.

- [ ] **Step 4: Run**

Run: `cargo test -p egret-dataapi openapi`
Expected: PASS. If the crate asserts a committed `openapi.json` byte-for-byte, regenerate
it in the same commit — that gate lives in a **different** crate than the one edited.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(dataapi): document which response columns are guaranteed"
```

---

### Task 8: the defect from spec §1.3, driven end to end

**Files:**
- Test: `crates/egret-dataapi/tests/` — the binary that already publishes and serves an
  endpoint through the real router.

**Interfaces:**
- Consumes: Tasks 1-7.

Every task above tests its own half. **None of them proves the defect is fixed**, because
the union can be computed correctly while `shape_rows` still refuses. This task is the one
that matters; the others are its scaffolding.

- [ ] **Step 1: Write the test**

Put it in a serving binary that already uses `serving_common::Fixture` — `serving_errors.rs`
owns the `schema_drift` cases, so it is the honest home for the test that retires one.

```rust
#[tokio::test]
async fn a_conditional_endpoint_serves_its_own_default_call() {
    // Spec §1.3: this published green and then 503'd on the call its own
    // generated document advertises.
    let f = skip_without_db!();
    let ep = f
        .publish_endpoint(
            "GET",
            "reports/cond",
            "SELECT id {% if detail %}, name {% endif %}FROM t",
        )
        .await;
    let _ = ep;

    // The documented default: no query string at all.
    let bare = f.call("reports/cond").await;
    assert_eq!(bare.status, StatusCode::OK, "this was the 503: {:?}", bare.body);
    assert_eq!(bare.body["data"][0].as_object().expect("an object row").len(), 1);

    let off = f.call("reports/cond?detail=false").await;
    assert_eq!(off.status, StatusCode::OK);
    assert_eq!(off.body["data"][0], json!({"id": 1}));

    let on = f.call("reports/cond?detail=true").await;
    assert_eq!(on.status, StatusCode::OK);
    assert_eq!(on.body["data"][0], json!({"id": 1, "name": "a"}));
}
```

`publish_endpoint` (`serving_common/mod.rs:473`) takes no parameter list; declare `detail`
the way the parameterised serving tests do — `serving_params.rs` has the pattern, and
`publish_with_day_param` (`serving_params.rs:344`) is the closest example. The fake engine
must return the two column sets per statement, as in Task 4. Follow `Res`'s real field
names (`:406`) rather than the `.status()` / `.body()` spelling above if they differ.

- [ ] **Step 2: Run it against `HEAD~7` to see it fail as described**

```bash
git stash && git checkout HEAD~7 -- crates/ && cargo test -p egret-dataapi serves_its_own_default_call
```

Expected: `503`. This is the step that proves the test can detect the defect at all; a test
written after the fix that has never been red proves nothing. Restore with
`git checkout HEAD -- crates/ && git stash pop`.

- [ ] **Step 3: Run it on the fix**

Run: `DATABASE_URL=… cargo test -p egret-dataapi serves_its_own_default_call`
Expected: PASS.

- [ ] **Step 4: Prove the single-statement gate is untouched**

```bash
git diff origin/main --stat -- crates/egret-dataapi/tests/template_render.rs
grep -c "DROP TABLE" crates/egret-dataapi/tests/template_render.rs
cargo test -p egret-dataapi --test template_render
```

Expected: **no diff** to `template_render.rs`, and every `;` / `#` rejection test green
unedited. If any needed changing to pass, stop — the change is wrong (Global Constraints).

- [ ] **Step 5: Full sweep**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test --workspace
```

`cargo test --workspace` **stops at the first failing binary**, so a green tail does not
mean a green suite — read the summary line for each binary.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "test(dataapi): a conditional endpoint serves its own default call"
```

---

## Before opening the PR

Run it on kind and check it in the browser. On this programme every live check has found a
defect the suites did not — CSV headers reading `column_1`, an editor sending
`database_id: ""`, a CSRF layer killing a `POST` that no router-level test could see.
Publish a conditional endpoint through the real editor, call it from the Try-it panel with
the parameter set both ways and omitted, and read the generated OpenAPI document for the
`required` list.
