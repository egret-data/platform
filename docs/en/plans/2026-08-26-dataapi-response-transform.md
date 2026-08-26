# C2 — Response Transform in Starlark Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An endpoint author can reshape the response with a Starlark script that declares its own output schema, and egret checks every response against that declaration.

**Architecture:** One script per endpoint, `transform(params, statements) -> data`, evaluated by a single function that publish, preview and serving all reach. The author **declares** the output schema rather than egret inferring it, because a script's shape can depend on the data and publish cannot see the data. Serving validates against the declaration and fails closed, exactly as `shape_rows` does for SQL columns.

**Tech Stack:** Rust, `starlark-rust`, `serde` with `deny_unknown_fields`, `utoipa` for OpenAPI, React + orval-generated client, Monaco (reusing the already-registered `python` definition).

**Spec:** `docs/en/specs/2026-08-26-dataapi-response-transform-design.md` (VI mirror alongside). Read §3 and §3.1 before Task 1 — they explain why the author declares the shape instead of egret inferring it, and that decision is what every later task is built on.

## Global Constraints

- **One evaluator, reached by publish, preview and serving through the same function.** If you find yourself writing a second evaluation path, the design is wrong. C1 put every constraint inside `coerce` and M2 made `validate_endpoint` *return* the union for exactly this reason; the preview/publish seam has already failed twice in this programme.
- **No `db`, no `http`, no `tx`, no `cache`, no clock, no randomness.** This is stage 1 of four. Stage 2 (`db.query`) is a separate spec and must not be started here.
- **A failing script is a 500, never a 400.** The caller sent a valid request; the endpoint is broken. Blaming the caller for the author's script puts the wrong code in `access_log`.
- **No parameter name, bound, or constraint kind may appear in a 400.** `ServingError::ParamInvalid` and `ParamMissing` stay unit variants; `serving/error.rs` enforces this by shape.
- **`output.shape` and `output.naming` are refused alongside a transform.** The script receives rows in one canonical form (objects, engine names) and owns everything after.
- **`meta.statements` keeps describing the SQL**, not the transform's output.
- **Publish's probe values stay constraint-blind.** `representative_value` yields `1` under `minimum: 100`; changing it would alter which branches are probed and silently change a frozen column contract.
- **The single-statement gate is untouchable.** `#` forbidden; `;` rejected at `reject_unscannable_bytes`, at the comment/literal scan, and after render. `git diff <base>..HEAD -- crates/egret-dataapi/tests/template_render.rs` must stay **0 bytes**.
- **`openapi.json` is committed and asserted byte-for-byte by the `egret-server` crate** — a different crate from the ones you will be editing. It was silently red for six tasks in C1. Run `cargo test -p egret-server` and report it separately.
- Regenerate the frontend client with **`npm run gen:api`**, never bare `orval`.
- Tests read `DATABASE_URL`; export `postgres://egret:egret@localhost:5432/egret_test`. **Never** the dev `egret` database; its migration-127 drift must not be repaired.
- Frontend: suite green, `tsc --noEmit` clean, eslint **177 warnings, 0 errors**.
- `cargo fmt` + `cargo clippy -p egret-dataapi --all-targets` clean. Conventional Commits.

## A standing instruction about tests

**Fifteen tests written for C1 turned out to be incapable of failing as intended** — a float pair that did not collide, a regex that hit the library's own default limit rather than ours, a date pair that sorted right either way, an assertion on a word the message did not contain. Most were found by running a mutation, not by reading.

For **every** test in this plan: stub the thing it targets, **confirm the file BUILDS**, and watch it fail. A mutation that does not compile looks exactly like one no test caught. When you run a suite, check its exit code directly — do not pipe through `tail`, which launders a failing exit code.

## File Structure

| File | Responsibility in C2 |
|---|---|
| `crates/egret-dataapi/Cargo.toml` | the `starlark` dependency |
| `crates/egret-dataapi/src/transform.rs` (new) | the evaluator, its limits, and its errors. The single place a script runs. |
| `crates/egret-dataapi-api/src/lib.rs` | `TransformSpec` on `OutputSpec`; `returns` reuses `ColumnSpec`. |
| `crates/egret-dataapi/src/lifecycle.rs` | publish refuses a transform it cannot honour, and probe-runs it against `returns`. |
| `crates/egret-dataapi/src/serving/router.rs` | the transform replaces `data`; the response is validated and fails closed. |
| `crates/egret-dataapi/src/openapi_gen.rs` | the contract is generated from `returns`. |
| `crates/egret-dataapi/src/preview.rs` | preview runs the transform through the same evaluator. |
| `frontend/src/routes/explore/dataapi/detail/ResponseSection.tsx` | the script editor and the `returns` table. |

---

### Task 1: Add Starlark, measure what it costs, and prove the sandbox

**This is the go/no-go task.** The spec (§9) says that if `starlark-rust`'s build cost is large enough to matter against this workspace's already-slow builds, that is a reason to reopen the language choice, not a detail. Settle it before anything is built on top.

**Files:**
- Modify: `crates/egret-dataapi/Cargo.toml`, root `Cargo.toml`
- Create: `crates/egret-dataapi/src/transform.rs`
- Modify: `crates/egret-dataapi/src/lib.rs` (add `mod transform;`)

**Interfaces:**
- Consumes: nothing.
- Produces: the `starlark` dependency at a pinned version, and a `transform` module. Later tasks depend on the evaluator API you establish here — **record the exact method names and signatures in your report**, because this plan does not assume them.

**About the API.** The spec records that `Evaluator` has `set_max_tick_count`, `set_max_heap_size`, `set_max_callstack_size` and `set_check_cancelled`, and that there is **no built-in timeout**. Those were measured during design, against whatever version was current then. **Verify each against the version you actually add**, and if any differs, report the real name rather than working around it silently.

- [ ] **Step 1: Measure the build baseline before adding anything**

```bash
cargo build -p egret-dataapi 2>&1 | tail -1     # warm the cache
touch crates/egret-dataapi/src/lib.rs
/usr/bin/time -p cargo build -p egret-dataapi 2>&1 | grep real
```

Record the number. This is the "one crate changed" rebuild, which is what a developer pays on every edit.

- [ ] **Step 2: Add the dependency**

Root `Cargo.toml` under `[workspace.dependencies]`, and `crates/egret-dataapi/Cargo.toml` under `[dependencies]` as `starlark = { workspace = true }`. Pin a specific minor version rather than a caret range so the build is reproducible.

- [ ] **Step 3: Measure the cost of the addition**

```bash
/usr/bin/time -p cargo build -p egret-dataapi 2>&1 | grep real   # cold, includes starlark
touch crates/egret-dataapi/src/lib.rs
/usr/bin/time -p cargo build -p egret-dataapi 2>&1 | grep real   # warm, the number that matters
cargo tree -p egret-dataapi -e normal --depth 1 | grep -c .
```

**Report both numbers and the transitive dependency count.** If the *warm* rebuild grew by more than a few seconds, **stop and report** — that is the signal the spec asks you to escalate on, and it is a decision for the plan's owner, not a thing to absorb.

- [ ] **Step 4: Write the failing sandbox test**

Create `crates/egret-dataapi/src/transform.rs` with a `#[cfg(test)]` module:

```rust
#[test]
fn a_script_cannot_reach_the_filesystem_or_the_clock() {
    // The whole language choice rests on this: safety is a property of the
    // LANGUAGE, not of a list of globals we remembered to strip. If any of
    // these evaluate, that premise is false and the choice must be reopened.
    for src in [
        "open('/etc/passwd')",
        "print(__file__)",
        "import os",
        "time.time()",
        "random.random()",
    ] {
        assert!(
            eval_probe(src).is_err(),
            "a script reached something it must not: {src}"
        );
    }
}

#[test]
fn an_infinite_loop_is_refused_rather_than_hanging() {
    // No built-in timeout exists, so the tick budget IS the defence. A test
    // that hangs here does not fail — it stalls the suite, which is why this
    // one is written before anything depends on the evaluator.
    let err = eval_probe("def f():\n  for i in range(1, 1000000000):\n    pass\nf()")
        .expect_err("an unbounded loop must be refused");
    assert!(
        format!("{err:?}").to_lowercase().contains("tick")
            || format!("{err:?}").to_lowercase().contains("budget"),
        "the refusal must name the budget it hit: {err:?}"
    );
}
```

`eval_probe(src: &str) -> Result<(), TransformError>` is a private test helper you write in this task: build an `Evaluator`, apply the limits, evaluate `src`, discard the value.

- [ ] **Step 5: Run the tests to verify they fail**

```bash
cargo test -p egret-dataapi --lib transform
```

Expected: FAIL to compile — `eval_probe` does not exist.

- [ ] **Step 6: Implement the probe helper and the limits**

Wire every limit the version you added actually offers. Where the spec's recorded name differs from reality, use the real one and say so in your report. Give the tick budget a named constant with a comment saying it is provisional and Task 3 measures it.

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cargo test -p egret-dataapi --lib transform
cargo clippy -p egret-dataapi --all-targets
```

Expected: both tests pass, clippy clean.

- [ ] **Step 8: Prove the loop test bites**

Remove the tick-budget line, **confirm the file builds**, and run `an_infinite_loop_is_refused_rather_than_hanging` **with a timeout**:

```bash
timeout 30 cargo test -p egret-dataapi --lib an_infinite_loop_is_refused
```

Expected: it hangs and `timeout` kills it (exit 124). That is the mutation succeeding — the test cannot pass without the budget. Restore the line.

- [ ] **Step 9: Commit**

```bash
git add crates/egret-dataapi/Cargo.toml Cargo.toml Cargo.lock crates/egret-dataapi/src/transform.rs crates/egret-dataapi/src/lib.rs
git commit -m "feat(dataapi): add starlark and pin what a transform may not do"
```

---

### Task 2: `TransformSpec` on `OutputSpec`

**Files:**
- Modify: `crates/egret-dataapi-api/src/lib.rs` (`OutputSpec`, around line 864)
- Test: same file, `#[cfg(test)]` module

**Interfaces:**
- Consumes: `ColumnSpec` (already exists, `crates/egret-dataapi-api/src/lib.rs:413`, with `name`, `ty`, `required`).
- Produces: `OutputSpec.transform: Option<TransformSpec>`, where `TransformSpec { script: String, returns: Vec<ColumnSpec> }`, both `#[serde(deny_unknown_fields)]`.

**`returns` reuses `ColumnSpec` deliberately.** M1's `required` flag already means "present in every permutation described at publish", and that is exactly what it must mean here. A parallel type would be a second definition of the same idea and would drift.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_output_spec_without_a_transform_round_trips_and_means_no_transform() {
    // A snapshot written before C2 has no `transform` key. Reading it back must
    // give `None`, and writing it back must not GROW the key — a snapshot that
    // gains fields on re-read is how two binaries disagree about one row.
    let json = r#"{"shape":"objects","naming":"as_is","envelope":"wrapped"}"#;
    let spec: OutputSpec = serde_json::from_str(json).expect("pre-C2 spec must still read");
    assert!(spec.transform.is_none());
    let back = serde_json::to_string(&spec).expect("serialize");
    assert!(!back.contains("transform"), "spec grew a key: {back}");
}

#[test]
fn a_transform_round_trips_with_its_declared_returns() {
    let json = r#"{"shape":"objects","naming":"as_is","envelope":"wrapped",
                   "transform":{"script":"def transform(params, statements):\n  return {}",
                                "returns":[{"name":"total","ty":"bigint","required":true}]}}"#;
    let spec: OutputSpec = serde_json::from_str(json).expect("must read");
    let t = spec.transform.as_ref().expect("transform present");
    assert!(t.script.contains("def transform"));
    assert_eq!(t.returns.len(), 1);
    assert_eq!(t.returns[0].name, "total");
    assert!(t.returns[0].required);

    let round: OutputSpec =
        serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
    assert_eq!(round, spec, "round trip must be lossless");
}

#[test]
fn an_unknown_key_inside_a_transform_is_refused() {
    // `deny_unknown_fields` is what turned a documented-but-wrong rate_limit key
    // into an error instead of a silent default. The same protection has to
    // reach INSIDE the new struct, not just around it.
    let json = r#"{"shape":"objects","naming":"as_is","envelope":"wrapped",
                   "transform":{"script":"x","returns":[],"timeout_ms":5000}}"#;
    assert!(
        serde_json::from_str::<OutputSpec>(json).is_err(),
        "an unknown key inside `transform` must be refused, not defaulted"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p egret-dataapi-api transform
```

Expected: FAIL to compile — `no field 'transform' on type 'OutputSpec'`.

- [ ] **Step 3: Add the type and the field**

```rust
/// An author's response transform: a Starlark script plus the output schema it
/// promises.
///
/// # Why `returns` is declared rather than inferred
///
/// Every other column egret freezes comes from `describe_as`, which works
/// because a SQL statement's shape is a function of its text and the branch a
/// parameter takes. A script's shape can depend on the DATA — `if len(rows) > 0`
/// — which publish cannot see. Inferring from a probe run would produce a
/// contract that is right for the probe data and wrong for a Tuesday. So the
/// author declares, publish checks the probes against the declaration, and
/// serving fails closed on every response that disagrees. See spec §3.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformSpec {
    /// The script source. Must define `transform(params, statements)`.
    pub script: String,
    /// The output schema this endpoint promises. `ColumnSpec` is reused rather
    /// than duplicated: its `required` flag already means "present in every
    /// permutation described at publish", which is exactly what it means here.
    pub returns: Vec<ColumnSpec>,
}
```

and on `OutputSpec`:

```rust
    /// Absent means no transform, which is every snapshot frozen before C2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformSpec>,
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p egret-dataapi-api transform
cargo build -p egret-dataapi
```

Expected: 3 passed; `egret-dataapi` still builds. If `OutputSpec` struct-literal sites elsewhere stop compiling, add the field there — **do not derive `Default` on `OutputSpec`** to dodge it.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-dataapi-api/src/lib.rs
git commit -m "feat(dataapi): an endpoint may declare a response transform"
```

---

### Task 3: The evaluator — one function, and its measured budget

**Files:**
- Modify: `crates/egret-dataapi/src/transform.rs`
- Test: same file

**Interfaces:**
- Consumes: `TransformSpec` from Task 2; the limits API established in Task 1.
- Produces:

```rust
pub(crate) fn run_transform(
    spec: &TransformSpec,
    params: &BTreeMap<String, serde_json::Value>,
    statements: &BTreeMap<String, Vec<serde_json::Value>>,
) -> Result<serde_json::Value, TransformError>;

pub(crate) enum TransformError {
    Parse(String),
    MissingEntryPoint,
    WrongArity { found: usize },
    Runtime(String),
    BudgetExceeded(&'static str),
    OutputTooLarge { bytes: usize, cap: usize },
    NotSerialisable(String),
}
```

**This is the single evaluator.** Publish (Task 5), serving (Task 6) and preview (Task 8) all call exactly this. No other function may evaluate a script.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_transform_receives_params_and_statements_and_returns_json() {
    let spec = TransformSpec {
        script: "def transform(params, statements):\n  \
                 return {\"r\": params[\"region\"], \"n\": len(statements[\"rows\"])}"
            .into(),
        returns: vec![],
    };
    let params = BTreeMap::from([("region".to_string(), json!("EU"))]);
    let statements = BTreeMap::from([("rows".to_string(), vec![json!({"a": 1}), json!({"a": 2})])]);
    let out = run_transform(&spec, &params, &statements).expect("runs");
    assert_eq!(out, json!({"r": "EU", "n": 2}));
}

#[test]
fn statements_are_keyed_even_when_there_is_only_one() {
    // A script that stops working when the author adds a second statement is a
    // trap. At N=1 the key is still there.
    let spec = TransformSpec {
        script: "def transform(params, statements):\n  return list(statements.keys())".into(),
        returns: vec![],
    };
    let statements = BTreeMap::from([("main".to_string(), vec![json!({"a": 1})])]);
    let out = run_transform(&spec, &BTreeMap::new(), &statements).expect("runs");
    assert_eq!(out, json!(["main"]));
}

#[test]
fn a_script_without_the_entry_point_is_refused_by_name() {
    let spec = TransformSpec { script: "x = 1".into(), returns: vec![] };
    assert!(matches!(
        run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()),
        Err(TransformError::MissingEntryPoint)
    ));
}

#[test]
fn a_wrong_arity_entry_point_is_refused_by_name() {
    // Distinguished from MissingEntryPoint on purpose: "you named it wrong" and
    // "you took the wrong arguments" send an author to different lines.
    let spec = TransformSpec {
        script: "def transform(params):\n  return {}".into(),
        returns: vec![],
    };
    assert!(matches!(
        run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()),
        Err(TransformError::WrongArity { found: 1 })
    ));
}

#[test]
fn an_unserialisable_return_is_refused_rather_than_lost() {
    // Starlark can return values with no JSON counterpart. Silently dropping
    // one would serve a response missing a field the script produced.
    let spec = TransformSpec {
        script: "def transform(params, statements):\n  return transform".into(),
        returns: vec![],
    };
    assert!(matches!(
        run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()),
        Err(TransformError::NotSerialisable(_))
    ));
}

#[test]
fn an_oversized_output_is_refused_with_both_numbers() {
    let spec = TransformSpec {
        script: format!(
            "def transform(params, statements):\n  return [\"x\"] * {}",
            MAX_TRANSFORM_OUTPUT_BYTES
        ),
        returns: vec![],
    };
    match run_transform(&spec, &BTreeMap::new(), &BTreeMap::new()) {
        Err(TransformError::OutputTooLarge { bytes, cap }) => {
            assert!(bytes > cap, "the error must carry both numbers: {bytes} vs {cap}");
        }
        other => panic!("expected OutputTooLarge, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p egret-dataapi --lib transform
```

Expected: FAIL to compile — `run_transform` does not exist.

- [ ] **Step 3: Implement `run_transform`**

Convert `params` and `statements` into Starlark values, evaluate the module, look up `transform`, check its arity, call it, convert the result back to `serde_json::Value`, and enforce the output cap on the **serialised** bytes.

Apply every limit from Task 1. Give `MAX_TRANSFORM_OUTPUT_BYTES` a named constant.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p egret-dataapi --lib transform
```

Expected: all 6 pass, plus Task 1's two.

- [ ] **Step 5: Measure the tick budget instead of guessing it**

Task 1 set a provisional budget. Settle it now, and **do not pick a number by intuition** — a budget too low refuses honest scripts, one too high is not a limit.

Write a throwaway benchmark (not a committed test) that runs a realistic transform — the `{region, items, total}` example from spec §1 — over 1,000 statements' worth of rows, and reports the ticks consumed and the wall clock. Then pick a budget with headroom over the realistic case and report both numbers and your choice.

C1's precedent: `compile_pattern` was measured at ~60 µs typical and ~650 µs worst-of-five at the cap before the caching question was answered, and the measurement is what settled it.

**Delete the benchmark afterwards.** A previous task in this programme left a probe crate behind and the reviewer checked for it.

- [ ] **Step 6: Prove the arity test bites**

Make `WrongArity` unreachable by removing the arity check, **confirm the file builds**, and run `a_wrong_arity_entry_point_is_refused_by_name`. Expected: FAIL. Restore.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-dataapi/src/transform.rs
git commit -m "feat(dataapi): one evaluator for a response transform"
```

---

### Task 4: Publish refuses a transform it cannot honour

**Files:**
- Modify: `crates/egret-dataapi/src/lifecycle.rs` (new `check_transform`, wired into `check_definition_scans`)
- Test: same file

**Interfaces:**
- Consumes: `TransformSpec` (Task 2), `run_transform` (Task 3).
- Produces: `pub fn check_transform(output: &OutputSpec) -> Result<(), ValidationError>`; the constant `pub const MAX_TRANSFORM_SCRIPT_BYTES: usize` (**this task defines it — Task 3 defines only `MAX_TRANSFORM_OUTPUT_BYTES`, which is a different limit on a different thing**); and new `ValidationError` arms: `TransformScriptTooLong { bytes, cap }`, `TransformReturnsEmpty`, `TransformReturnsDuplicateName(String)`, `TransformWithShapeOrNaming(&'static str)`, `TransformScriptInvalid(String)`.

**Pick the script cap by looking at a real script, not by intuition.** The `{region, items, total}` example in spec §1 is about 200 bytes; a cap has to leave room for comments and a helper function or two while still being a cap. Say in a comment what you sized it against.

**Where it goes.** `check_definition_scans` in `lifecycle.rs` is what all three entry points reach — endpoint create, publish, and preview. C1's `check_constraints` lives there for exactly that reason, and a reviewer called that placement better than C1's own brief. Put `check_transform` beside it.

**Find it by name, not by line number.** It was at 1721 when this plan was written and it moves whenever anyone adds to that file; a stale line number in a brief is how an implementer edits the wrong place confidently.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_transform_alongside_shape_or_naming_is_refused() {
    // Once a script decides the shape, `shape` and `naming` are dead or
    // ambiguous — does `naming: camel` apply before the script sees the rows,
    // or to the keys the script wrote? Refuse rather than pick.
    let mut o = transformed_output("def transform(params, statements):\n  return {}");
    o.shape = "arrays".into();
    let err = check_transform(&o).expect_err("arrays plus a transform");
    assert!(err.to_string().contains("shape"), "{err}");

    let mut p = transformed_output("def transform(params, statements):\n  return {}");
    p.naming = "camel".into();
    assert!(check_transform(&p).is_err(), "naming plus a transform must be refused too");
}

#[test]
fn a_transform_with_objects_and_as_is_is_accepted() {
    // The defaults must not be read as "the author asked for a shape".
    let o = transformed_output("def transform(params, statements):\n  return {}");
    assert_eq!(o.shape, "objects");
    assert_eq!(o.naming, "as_is");
    assert!(check_transform(&o).is_ok());
}

#[test]
fn an_empty_returns_is_refused() {
    // A transform with no declared output has no contract at all, and serving
    // would have nothing to check against — it would fail OPEN.
    let mut o = transformed_output("def transform(params, statements):\n  return {}");
    o.transform.as_mut().unwrap().returns.clear();
    assert!(check_transform(&o).is_err());
}

#[test]
fn two_returns_with_one_name_are_refused() {
    let mut o = transformed_output("def transform(params, statements):\n  return {}");
    let r = &mut o.transform.as_mut().unwrap().returns;
    r.push(ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true });
    r.push(ColumnSpec { name: "total".into(), ty: "varchar".into(), required: true });
    let err = check_transform(&o).expect_err("duplicate name");
    assert!(err.to_string().contains("total"), "{err}");
}

#[test]
fn a_script_that_does_not_parse_is_the_authors_400_not_the_callers_500() {
    let o = transformed_output("def transform(params, statements)\n  return {}");
    assert!(matches!(
        check_transform(&o),
        Err(ValidationError::TransformScriptInvalid(_))
    ));
}

#[test]
fn a_script_longer_than_the_cap_is_refused_with_both_numbers() {
    let long = format!(
        "def transform(params, statements):\n  # {}\n  return {{}}",
        "x".repeat(MAX_TRANSFORM_SCRIPT_BYTES)
    );
    match check_transform(&transformed_output(&long)) {
        Err(ValidationError::TransformScriptTooLong { bytes, cap }) => {
            assert!(bytes > cap, "the error must carry both numbers: {bytes} vs {cap}")
        }
        other => panic!("expected TransformScriptTooLong, got {other:?}"),
    }
}
```

Add the helper beside the tests:

```rust
fn transformed_output(script: &str) -> OutputSpec {
    let mut o = /* the same way neighbouring tests build an OutputSpec —
                   read one first; OutputSpec does NOT derive Default */;
    o.transform = Some(TransformSpec {
        script: script.into(),
        returns: vec![ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true }],
    });
    o
}
```

**Read a neighbouring test before writing that helper.** `OutputSpec` does not derive `Default`, and this plan does not know which constructor the test module already uses. Copy the local one.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p egret-dataapi --lib check_transform
```

Expected: FAIL to compile — `check_transform` does not exist.

- [ ] **Step 3: Implement `check_transform` and wire it in**

Refuse, in this order: script length, `shape`/`naming` combination, empty `returns`, duplicate `returns` names, then parse the script (via Task 3's evaluator, so publish and serving agree about which scripts exist).

Wire it into `check_definition_scans` **beside** `check_constraints`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p egret-dataapi --lib check_transform
cargo test -p egret-dataapi
```

Expected: all 6 pass, crate green.

- [ ] **Step 5: Prove the wiring test bites**

Comment out the `check_transform(...)` call in `check_definition_scans`, **confirm it builds**, and run a test that publishes a transform with `shape: arrays` end to end. Expected: it publishes, so the test FAILS. Restore.

**If no such end-to-end test exists yet, write one** — a unit test on `check_transform` alone cannot tell you the function is reachable, and an unreachable check is the defect this step exists to catch.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-dataapi/src/lifecycle.rs
git commit -m "feat(dataapi): publish refuses a transform it cannot honour"
```

---

### Task 5: Publish probe-runs the script against `returns`

**Files:**
- Modify: `crates/egret-dataapi/src/lifecycle.rs`
- Test: same file

**Interfaces:**
- Consumes: `run_transform` (Task 3), `check_transform` (Task 4), `DescribedBranches` (already exists, `lifecycle.rs:2531`).
- Produces: `pub(crate) fn probe_transform(output: &OutputSpec, described: &[StatementSnapshot], params: &[ParamSpec]) -> Result<(), ValidationError>` and a `TransformOutputMismatch { context: String, detail: String }` arm.

**What rows the script sees at publish, and why they are synthetic.**

`describe_as` gives publish the *columns* of each statement, never the data — verified: `validated_draft` (`routes.rs:1371`) returns `(DataApiEndpointDto, Vec<StatementSnapshot>)`, and `StatementSnapshot` is `{ name, sql, columns }`. So the probe rows are **built from the described columns** with type-correct stand-in values, the same way `representative_value` builds parameter values for M1's branch probes. Publish reads no data and makes no extra round trip.

**Probe with both a populated and an empty row set.** `if len(rows) == 0` is the most common way a script's output shape diverges, and it costs one extra evaluation to cover.

**Say plainly what this proves.** It proves the script runs and matches `returns` for a canonical row set and for an empty one. It does **not** prove the script always conforms — that is what serving's check is for, and the spec (§3.1) says so.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_script_that_does_not_produce_its_declared_returns_is_refused_at_publish() {
    let mut o = transformed_output(
        "def transform(params, statements):\n  return {\"wrong\": 1}",
    );
    o.transform.as_mut().unwrap().returns =
        vec![ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true }];
    let err = probe_transform(&o, &[one_statement("rows")], &[])
        .expect_err("declared `total`, produced `wrong`");
    assert!(err.to_string().contains("total"), "must name the missing field: {err}");
}

#[test]
fn an_optional_field_may_be_absent_but_an_undeclared_one_may_not() {
    // M1's `required` flag, unchanged: absent-when-optional is fine, present-
    // when-undeclared never is.
    let mut o = transformed_output("def transform(params, statements):\n  return {\"a\": 1}");
    o.transform.as_mut().unwrap().returns = vec![
        ColumnSpec { name: "a".into(), ty: "bigint".into(), required: true },
        ColumnSpec { name: "b".into(), ty: "bigint".into(), required: false },
    ];
    assert!(probe_transform(&o, &[one_statement("rows")], &[]).is_ok(), "optional `b` may be absent");

    let mut p = transformed_output("def transform(params, statements):\n  return {\"a\": 1, \"z\": 2}");
    p.transform.as_mut().unwrap().returns =
        vec![ColumnSpec { name: "a".into(), ty: "bigint".into(), required: true }];
    assert!(probe_transform(&p, &[one_statement("rows")], &[]).is_err(), "undeclared `z` must be refused");
}

#[test]
fn the_empty_row_set_is_probed_too() {
    // The commonest way a script's shape diverges. This script conforms with
    // rows and returns a bare list without them — publish must catch that, and
    // it can only catch it by running the empty case.
    let mut o = transformed_output(
        "def transform(params, statements):\n  \
         if len(statements[\"rows\"]) == 0:\n    return []\n  \
         return {\"total\": 1}",
    );
    o.transform.as_mut().unwrap().returns =
        vec![ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true }];
    let err = probe_transform(&o, &[one_statement("rows")], &[])
        .expect_err("the empty-rows branch returns a list, not the declared object");
    assert!(
        err.to_string().contains("empty"),
        "the refusal must say WHICH probe failed, or an author cannot find it: {err}"
    );
}

#[test]
fn probe_rows_are_built_from_the_described_columns_not_from_data() {
    // Publish must not read data. This pins the shape of what the script sees:
    // one row per statement, keyed by the described column names.
    let o = transformed_output(
        "def transform(params, statements):\n  \
         return {\"total\": len(statements[\"rows\"][0])}",
    );
    // `one_statement("rows")` describes two columns, so the probe row has two keys.
    assert!(probe_transform(&o, &[one_statement("rows")], &[]).is_ok());
}
```

Add the helper:

```rust
fn one_statement(name: &str) -> StatementSnapshot {
    StatementSnapshot {
        name: name.into(),
        sql: "SELECT 1 AS a, 'x' AS b".into(),
        columns: vec![
            ColumnSpec { name: "a".into(), ty: "bigint".into(), required: true },
            ColumnSpec { name: "b".into(), ty: "varchar".into(), required: true },
        ],
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p egret-dataapi --lib probe_transform
```

Expected: FAIL to compile — `probe_transform` does not exist.

- [ ] **Step 3: Implement `probe_transform`**

Build a synthetic row per statement from its `columns`, run the script twice — populated and empty — and compare each result against `returns`. Name the failing probe in the error.

Wire it into `validate` where the described statements are already in hand.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p egret-dataapi --lib probe_transform
cargo test -p egret-dataapi
```

Expected: 4 pass, crate green.

- [ ] **Step 5: Prove the empty-probe test bites**

Remove the empty-row-set evaluation, **confirm the file builds**, and run `the_empty_row_set_is_probed_too`. Expected: FAIL. Restore.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-dataapi/src/lifecycle.rs
git commit -m "feat(dataapi): publish probes a transform against what it declares"
```

---

### Task 6: Serving — the transform produces `data`, and fails closed

**Files:**
- Modify: `crates/egret-dataapi/src/serving/router.rs`
- Test: same file, plus `crates/egret-dataapi/tests/` for the end-to-end path

**Interfaces:**
- Consumes: `run_transform` (Task 3).
- Produces: no new public names. `ServingError` gains **no** variant — a transform failure is `ServingError::internal`, like a non-compiling pattern in C1.

**The `meta.row_count` decision, which the spec left open with a recommendation.**

`MetaContract::total_row_count()` sums the statements, so on a transformed endpoint it stays *consistent* with `meta.statements` — both describe the SQL. But a consumer reads `row_count` as describing **`data`**, and after a transform `data` may be a single object.

**Implement the spec's recommendation: omit `row_count` for a transformed endpoint**, and say so in the generated contract (Task 7). M2 already changed that field's meaning without changing its type, and a field that is technically truthful while reliably misread is that hazard a second time.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_transformed_response_is_validated_against_returns_and_fails_closed() {
    // The real guarantee. Publish's probe cannot prove a script always conforms,
    // so this is what holds when the data is not what the probe assumed.
    let snap = transformed_snapshot(
        "def transform(params, statements):\n  return {\"surprise\": 1}",
        vec![ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true }],
    );
    let err = build_body(&snap, /* rows that make the script take that branch */)
        .expect_err("undeclared `surprise` must not reach a caller");
    assert!(matches!(err, ServingError::Internal(_)));
}

#[test]
fn a_transform_failure_is_an_internal_error_never_a_400() {
    // The caller sent a valid request; the ENDPOINT is broken. A 400 would
    // blame the caller for the author's script and put the wrong code in
    // access_log — the same defect check_defaults exists to prevent.
    let snap = transformed_snapshot(
        "def transform(params, statements):\n  fail(\"boom\")",
        vec![ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true }],
    );
    let err = build_body(&snap, /* any rows */).expect_err("the script raised");
    assert!(matches!(err, ServingError::Internal(_)), "must not be ParamInvalid");
}

#[test]
fn meta_row_count_is_omitted_for_a_transformed_endpoint() {
    let snap = transformed_snapshot(
        "def transform(params, statements):\n  return {\"total\": 1}",
        vec![ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true }],
    );
    let body = build_body(&snap, /* any rows */).expect("runs");
    assert!(
        body["meta"].get("row_count").is_none(),
        "row_count describes the SQL, and a consumer reads it as describing `data`: {body}"
    );
}

#[test]
fn meta_statements_still_describes_the_sql_after_a_transform() {
    // Two different truths, kept separate: what the engine returned, and what
    // the endpoint promised.
    let snap = transformed_snapshot(
        "def transform(params, statements):\n  return {\"total\": 1}",
        vec![ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true }],
    );
    let body = build_body(&snap, /* two rows */).expect("runs");
    assert!(body["meta"]["statements"].is_object(), "{body}");
}

#[test]
fn an_untransformed_endpoint_is_byte_for_byte_unchanged() {
    // The whole feature must be invisible to every endpoint that does not use
    // it. This is the test that catches a refactor that "just moves" the
    // shaping code and changes it.
    // …build one endpoint WITHOUT a transform, assert the body matches what the
    // pre-C2 path produced for the same rows.
}
```

`build_body` and `transformed_snapshot` are illustrative. **Read `router.rs` and drive the real function** — the body is assembled by the serving path, and this plan does not name it. If the assembly is inline in a larger handler, drive it through that handler.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test -p egret-dataapi transform
```

Expected: FAIL — the transform is not applied.

- [ ] **Step 3: Implement**

After the statements execute and `meta` is built, if `output.transform` is present: run the script, validate the result against `returns`, and use it as `data`. On any failure, `ServingError::internal` with a message naming the cause.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p egret-dataapi
cargo test -p egret-server
```

Expected: both green. **Report `egret-server` separately** — it is a different crate and was silently red for six tasks in C1.

- [ ] **Step 5: Prove the fail-closed test bites**

Make the `returns` validation a no-op, **confirm it builds**, and run `a_transformed_response_is_validated_against_returns_and_fails_closed`. Expected: FAIL. Restore.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-dataapi/src/serving/router.rs crates/egret-dataapi/tests
git commit -m "feat(dataapi): a transform produces data, checked against what it declared"
```

---

### Task 7: The contract is generated from `returns`

**Files:**
- Modify: `crates/egret-dataapi/src/openapi_gen.rs`
- Modify: `openapi.json` only by regenerating
- Test: `crates/egret-dataapi/src/openapi_gen.rs`

**Interfaces:**
- Consumes: `TransformSpec.returns` (Task 2).
- Produces: no new names.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_transformed_endpoints_data_schema_comes_from_returns() {
    let snap = transformed_snapshot_for_doc(vec![
        ColumnSpec { name: "region".into(), ty: "varchar".into(), required: true },
        ColumnSpec { name: "total".into(), ty: "bigint".into(), required: false },
    ]);
    let doc = build_doc(&snap);
    let data = &doc["paths"][/* the endpoint path */]["get"]["responses"]["200"]
        /* …down to the `data` schema — read openapi_gen.rs for the real path */;
    assert_eq!(data["required"], json!(["region"]), "only the required field: {data}");
    assert!(data["properties"]["total"].is_object(), "{data}");
}

#[test]
fn a_transformed_endpoint_does_not_document_the_statement_keyed_data() {
    // Without a transform, M2 documents `data` as an object keyed by statement
    // name. With one, `data` is whatever the script returns — documenting both
    // would describe an endpoint that answers two different ways.
    let snap = transformed_snapshot_for_doc(vec![
        ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true },
    ]);
    let doc = build_doc(&snap);
    let s = serde_json::to_string(&doc).unwrap();
    assert!(!s.contains("\"rows\""), "the statement keys must not appear in `data`: {s}");
}

#[test]
fn the_contract_says_row_count_is_absent_for_a_transformed_endpoint() {
    // Task 6 omits it. A document that still promises it would be a lie a
    // generated client acts on.
    let snap = transformed_snapshot_for_doc(vec![
        ColumnSpec { name: "total".into(), ty: "bigint".into(), required: true },
    ]);
    let doc = build_doc(&snap);
    let meta = /* …the `meta` schema */;
    assert!(meta["properties"].get("row_count").is_none(), "{meta}");
}

#[test]
fn an_untransformed_endpoints_document_is_unchanged() {
    // …build the same endpoint without a transform and assert the document is
    // byte-identical to what the pre-C2 generator produced.
}
```

**Read `openapi_gen.rs` for the real accessor paths** before writing these. This plan does not know them, and a guess that compiles is still a guess.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p egret-dataapi --lib openapi_gen
```

- [ ] **Step 3: Implement**

When `output.transform` is present, build `data`'s schema from `returns` — `required` lists exactly the columns with `required: true` — and omit `row_count` from `meta`.

- [ ] **Step 4: Run the tests, regenerate, and run the byte gate**

```bash
cargo test -p egret-dataapi --lib openapi_gen
cargo run -p egret-bin -- openapi > openapi.json
cargo test -p egret-server
```

**Report `cargo test -p egret-server` explicitly.**

- [ ] **Step 5: Read the regenerated diff, do not just trust the gate**

```bash
git diff openapi.json
```

A change in this area once silently deleted a struct's `description`, and the byte gate accepted it because the regenerated and committed files agreed — both wrong. The gate proves currency, never correctness. Read what was **removed** as well as what was added.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-dataapi/src/openapi_gen.rs openapi.json
git commit -m "feat(dataapi): a transformed endpoint documents what it declares"
```

---

### Task 8: Preview runs the transform

**Files:**
- Modify: `crates/egret-dataapi/src/preview.rs`
- Test: `crates/egret-dataapi/tests/preview.rs`

**Interfaces:**
- Consumes: `run_transform` (Task 3).
- Produces: no new names. `DataApiPreviewDto` may gain a field carrying the transformed output.

**This is the seam that has failed twice.** Preview was shape-blind for one release and told authors "fine" where publish said "no"; M2's Critical was publish and serving disagreeing about a parameter's shape. Preview must reach the **same** `run_transform`, not a second path.

Preview currently runs one `sql`, not an endpoint. Establish by reading how it composes `statements` for the script — if it cannot, **that is a finding: report it rather than papering over it**, because a preview that cannot show the transform is a preview that will disagree with publish.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn preview_shows_the_transformed_output_not_the_raw_rows() {
    // …drive a preview with a transform declaring `{total}` and assert the
    // response carries the transformed value, not the statement's rows.
}
```

Write the body against the real preview harness in that file, not against this sketch.

- [ ] **Step 2: Run it and confirm it fails for the right reason**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test -p egret-dataapi --test preview
```

A DB-gated test that cannot reach a database reports `ok` while asserting nothing, and only its duration distinguishes the two. Check the duration.

- [ ] **Step 3: Implement**

- [ ] **Step 4: Prove it bites**

Stub `run_transform`'s call in the preview path, **confirm it builds**, re-run. Expected: FAIL. Restore.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-dataapi/src/preview.rs crates/egret-dataapi/tests/preview.rs
git commit -m "feat(dataapi): preview runs the transform it will publish"
```

---

### Task 9: The editor — script and declared returns

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/detail/ResponseSection.tsx`
- Modify: `frontend/src/api/generated/**` (via `npm run gen:api`)
- Test: `frontend/src/routes/explore/dataapi/__tests__/ResponseSection.test.tsx` (check whether it exists first)

**Interfaces:**
- Consumes: the regenerated `TransformSpec` model.
- Produces: no new exports.

**Monaco is already there.** `frontend/src/lib/monaco.ts` registers exactly `sql` and `python` (lines 40–41, verified). Starlark reuses the `python` definition, so this adds no bundle weight.

**Follow `ParamEditor.tsx`'s precedent** for the `returns` table — C1 added per-type constraint inputs there, and the same reviewer verified the placement decision. Read it before designing a second pattern.

- [ ] **Step 1: Regenerate the client**

```bash
cd frontend && npm run gen:api
git diff --stat src/api/generated
```

Never bare `orval` — the npm script runs a committed codemod restoring the `encodeURIComponent` orval drops.

- [ ] **Step 2: Write the failing tests**

```tsx
it("hides shape and naming once a transform is present", async () => {
  // Publish refuses the combination (Task 4). An editor that lets an author set
  // both produces an error about a control the editor is still showing them —
  // the same defect C1's clearInapplicableConstraints exists to prevent.
});

it("keeps a declared return row when the script changes", async () => {
  // `returns` is the contract, not a derived view of the script. A keystroke in
  // the editor must not drop it.
});

it("refuses to save a transform with no declared returns", async () => {
  // Serving would have nothing to check against — it would fail OPEN.
});
```

Write the bodies following the sibling tests' setup. **`toBeUndefined()` passes when a field was never set at all** — set it first, then prove it survives or goes away.

- [ ] **Step 3: Run to verify they fail**

```bash
cd frontend && npx vitest run ResponseSection
```

- [ ] **Step 4: Implement**

- [ ] **Step 5: Run the gates**

```bash
cd frontend && npx vitest run && npx tsc --noEmit && npx eslint .
```

Expected: green, clean, **177 warnings / 0 errors**.

- [ ] **Step 6: Commit**

```bash
git add frontend/src
git commit -m "feat(dataapi): author a response transform in the editor"
```

---

### Task 10: Live check

**Files:**
- Test: whatever the browser finds needs one.

**Interfaces:**
- Consumes: everything.

**Every defect that mattered in this programme was found in a browser, not by review and not by the suites.** C1's live check found two: preview describing a refusal wrongly, and a publish refusal shown in an unpausable 4.3-second toast whose `elementFromPoint` resolved to the dialog overlay.

Start the stack against a **throwaway** database. The dev `egret` database **cannot boot the server** — it records migration `127` as "dataapi alerts" while on disk `127` is `render_tokens_workspace_id`. **Do not repair it.** Credentials `admin@egret.local` / `Admin12345!`. **Login is rate-limited 5 per 5 minutes — log in once.** Confirm the server is built from this branch before trusting any negative result.

Look at six things and report what you **saw**:

1. **Author a transform end to end** and publish it. Is the script editor usable — syntax highlighting, a readable error when the script does not parse?
2. **A script that does not match its `returns`.** The publish refusal must be readable **without hovering a disabled control**, and must say which probe failed.
3. **Call the endpoint.** Does `data` carry the transformed shape, and does `meta.statements` still describe the SQL?
4. **A script that fails at serve time** — force the undeclared-field case with real data. The caller must get a 500, not a 400, and nothing about the endpoint's internals.
5. **The generated contract** for a transformed endpoint: `data` from `returns`, no `row_count`.
6. **An endpoint with no transform.** Unchanged in every respect. This is the one most likely to have quietly regressed.

**Fix anything the browser finds.** If a fix is a layout claim, cover it with a real-browser test — `frontend/tests/e2e/disabled-control-affordances.spec.ts` is the precedent and `docs/en/ui-conventions.md` §10 states the repo's position. **Do not add a jsdom test for a layout defect.**

- [ ] **Step 1: Run everything**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test -p egret-dataapi
cargo test -p egret-server
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cd frontend && npx vitest run && npx tsc --noEmit && npx eslint .
```

- [ ] **Step 2: Confirm the untouchable gate**

```bash
git diff <base>..HEAD -- crates/egret-dataapi/tests/template_render.rs | wc -c
```

Expected: **0**.

- [ ] **Step 3: Commit whatever the browser made you fix**

---

## Self-Review

**Spec coverage.** §1 scope → Tasks 2, 3 (no `db`/`http`/`tx` is enforced by Task 1's sandbox test). §2 Starlark and its cost → Task 1. §3/§3.1 declared-not-inferred → Tasks 2, 5, 6. §4 pipeline position → Task 6. §4.1 `shape`/`naming` refused → Task 4. §5 limits → Tasks 1, 3, 4. §6 preview seam → Task 8. §7 caching and versions → no task needed, and the plan says why: the script lives in the snapshot, so existing keys already change with it. §8 exclusions → no task, by construction.

**The `meta.row_count` decision the spec left open** is settled in Task 6 (omit) and documented in Task 7. If the plan's owner prefers keeping it, both tasks change together.

**Known soft spots, named rather than hidden.** Three tasks contain sketched test bodies rather than complete ones — Task 6's `build_body`, Task 7's document accessor paths, and Task 8's preview harness — because this plan did not read those functions line by line. Each says so and tells the implementer to use the real signature. C1 shipped a plan with an invented function name (`resolve_one`) that did not exist, and naming the guess is what stops that repeating.

**One decision deliberately left to measurement, not guessed:** the tick budget (Task 3, Step 5) and the `starlark` build cost (Task 1, Step 3). The plan states the escalation rule for the second — if the warm rebuild grows by more than a few seconds, stop and report, because the spec says that reopens the language choice rather than being a detail to absorb.
