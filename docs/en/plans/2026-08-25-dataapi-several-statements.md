# M2 — several statements, and one envelope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One Data API endpoint runs several named, read-only SQL statements and returns
them under one fixed response envelope.

**Architecture:** `sql: String` becomes `statements: Vec<StatementSpec>` in the draft row
and `Vec<StatementSnapshot>` in the published snapshot, each statement carrying its own
frozen columns from M1. Serving executes them sequentially and wraps the result in
`{status, message, data, meta}`. Every statement passes the **existing, unchanged**
single-statement gate; multi-statement is a property of the configuration and never of a
string handed to an engine.

**Tech Stack:** Rust, `crates/egret-dataapi{,-api}`, sqlx migrations over `jsonb`, minijinja.

**Spec:** `docs/en/specs/2026-08-23-dataapi-multi-statement-design.md` §4 — **including
§4.1.1**, which was written after M1 landed and re-decides two things §4 assumed.

## Global Constraints

- **The single-statement gate is untouchable.** `#` forbidden, `;` rejected by
  `reject_unscannable_bytes`, by the comment/literal scan, and after render. **If any
  existing `;` or `#` test needs editing to pass, the change is wrong** — report BLOCKED.
  `git diff <base>..HEAD -- crates/egret-dataapi/tests/template_render.rs` must stay
  **0 bytes**.
- **"Statement" means one of the endpoint's N SQL definitions.** M1 used the same word for
  one rendered *branch*; Task 1 renames M1's usage before any behaviour changes. After
  Task 1, a "statement" is never a branch.
- **The describe budget applies across the endpoint, not per statement** (§4.1.1). Ten
  statements must not permit ten times the engine round trips.
- **Everything M1 enforces stays enforced, per statement**: the union of branch columns
  with `required`, the type comparison, the `arrays`-may-not-branch refusal, the
  duplicate-name refusal, the naming-collision fold.
- **`ServingMeta.columns` keeps its meaning**: the frozen contract, not this execution's
  shape.
- Backend: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` clean.
  `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` — **never** the
  dev `egret` database. **Do not run `cargo test --workspace`** unless a task says to.

## Method rules — each of these caught a real defect in M1

1. **Reproduce before fixing; write the test first and watch it fail.** Six tests in M1
   were green for the wrong reason.
2. **Mutate in both directions.** Deleting a refusal must redden a test; making it refuse
   unconditionally must redden a **different** one.
3. **Build the mutant to zero errors first**, assert the **match count** of any text
   substitution, and `touch` after restoring — a mutation that fails to compile, one that
   never applied, and one reverted without a rebuild all produce a green run that means
   nothing. All three happened in M1.
4. **A refusal test proves nothing until you read *which* refusal fired.** `503` covers
   both `schema_drift` and `output_naming_collision` here; `400` covers several.
5. **The plan's illustrative code is a guess; the file is the authority.** Say where they
   disagreed.
6. **A fixture keyed on rendered SQL cannot key on a parameter name** — rendering consumes
   it. That silently collapsed a two-branch fixture to one in M1.

---

### Task 1: give the word "statement" back

**Files:** `crates/egret-dataapi/src/lifecycle.rs`

Mechanical rename, no behaviour change, its own commit so every later diff is readable.
`MAX_PUBLISHED_STATEMENTS` → `MAX_DESCRIBED_BRANCHES`; `DescribedBranches::per_statement`
→ `per_branch`; `branch_statements` → `branch_renderings`; and the prose in each doc
comment that says "statement" where it means "branch". `ValidationError::TooManyBranches`
already reads correctly — check its message says branches, not statements.

- [ ] **Step 1: rename, and prove it changed nothing**

The whole `egret-dataapi` suite must pass with **zero test edits**. If a test name or
assertion string has to change, that string was describing behaviour, not a symbol — say
so in your report rather than editing silently.

Run: `cargo test -p egret-dataapi` — expect the same totals as before your change.

- [ ] **Step 2: Commit**

```bash
git add -A && git commit -m "refactor(dataapi): a statement is what an author writes, a branch is what publish renders"
```

---

### Task 2: `StatementSpec`, named and validated

**Files:** `crates/egret-dataapi-api/src/lib.rs`

**Interfaces produced:**
```rust
pub struct StatementSpec { pub name: String, pub sql: String }
pub struct StatementSnapshot { pub name: String, pub sql: String, pub columns: Vec<ColumnSpec> }
```

- [ ] **Step 1: Write the failing tests**

A name must match `^[a-z][a-z0-9_]*$` and be unique within the endpoint. Names are how a
caller reads the response, so the rules are a contract, not a style preference.

```rust
#[test]
fn a_statement_name_must_be_an_identifier() {
    for bad in ["Rows", "1st", "row count", "rows-total", "", "_rows"] {
        assert!(validate_statement_name(bad).is_err(), "accepted {bad:?}");
    }
    for good in ["rows", "row_count", "r2"] {
        assert!(validate_statement_name(good).is_ok(), "refused {good:?}");
    }
}

#[test]
fn two_statements_may_not_share_a_name() { /* the list validator refuses */ }

#[test]
fn an_endpoint_needs_at_least_one_statement() { /* an empty list refuses */ }
```

- [ ] **Step 2: Run, watch them fail. Step 3: implement. Step 4: run.**
- [ ] **Step 5: Commit**

---

### Task 3: the draft row stores a statement list

**Files:** a new migration; `crates/egret-dataapi/src/endpoints.rs`

`dataapi.endpoints.sql` is `text`; every other structured column (`params`, `output`,
`cache`, `limits`, `rate_limit`) is `jsonb`. Replace `sql text` with `statements jsonb`,
converting every existing row to a one-element list:

```sql
ALTER TABLE dataapi.endpoints ADD COLUMN statements jsonb;
UPDATE dataapi.endpoints SET statements = jsonb_build_array(jsonb_build_object('name','main','sql',sql));
ALTER TABLE dataapi.endpoints ALTER COLUMN statements SET NOT NULL;
ALTER TABLE dataapi.endpoints DROP COLUMN sql;
```

**Check the migration number against `migrations/` before writing it** — this repo has had
a collision, and the dev database still cannot boot because of one (127 renumbered by
#537). Take the next free number, and after adding it **touch `metastore/src/lib.rs`** to
force the embed to refresh.

- [ ] **Step 1: a test that an existing draft survives the migration** — insert a row with
  the old shape against the pre-migration schema if the harness allows it, or assert the
  `UPDATE`'s shape directly. Then the CRUD tests in `endpoints_routes.rs` must pass.
- [ ] **Steps 2-5: implement, run `--test endpoints_routes` and `--test endpoints_schema`, commit.**

---

### Task 4: the snapshot carries statements, and old snapshots still deserialize

**Files:** `crates/egret-dataapi-api/src/lib.rs`

`EndpointSnapshot.sql: String` and `.columns: Vec<ColumnSpec>` become
`.statements: Vec<StatementSnapshot>`.

**Published snapshots are immutable and must not be rewritten.** M1 set the precedent with
`#[serde(default = "default_true")]` on `ColumnSpec::required`: an old row keeps working
because the *reader* accommodates it, not because the data was migrated. Do the same —
a snapshot carrying `sql` + `columns` and no `statements` deserializes into a single
statement named `main`.

That name is invisible for a one-statement endpoint, because `data` is an array rather
than a keyed object (§4.4). Confirm that before choosing it.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_snapshot_published_before_m2_reads_as_one_statement() {
    // Exactly the shape rows carry today.
    let old = r#"{"sql":"SELECT 1","columns":[{"name":"a","ty":"integer","required":true}],
                  "method":"GET","path":"x", …}"#;
    let s: EndpointSnapshot = serde_json::from_str(old).unwrap();
    assert_eq!(s.statements.len(), 1);
    assert_eq!(s.statements[0].sql, "SELECT 1");
    assert_eq!(s.statements[0].columns[0].name, "a");
}
```

- [ ] **Step 2: run and watch it fail. Step 3: implement. Step 4: run.**
- [ ] **Step 5: prove it by mutation** — make the compat path produce an empty list; the
  test must redden. Then fix every call site the type change breaks
  (`cargo build --workspace --all-targets` names them).
- [ ] **Step 6: Commit**

---

### Task 5: publish validates and describes every statement

**Files:** `crates/egret-dataapi/src/lifecycle.rs`

Each statement goes through `validate_definition` **unchanged** — the same `;`/`#` scan,
the same `SELECT`-only head-word rule, the same parameter rules. Then each is described
through M1's branch machinery and gets its own frozen column union.

**The budget is endpoint-wide** (§4.1.1): sum the distinct branch renderings across all
statements and refuse above `MAX_DESCRIBED_BRANCHES`, before the first describe.

- [ ] **Step 1: failing tests**
  - a two-statement endpoint publishes and freezes two column sets
  - a `;` inside any one statement still refuses, with the same error as today
  - two statements each just under the branch cap refuse **together**, and the message
    names the total
  - M1's per-statement rules still fire: the `arrays` branching refusal, the duplicate-name
    refusal, the naming-collision fold, the type conflict
- [ ] **Step 2: watch them fail. Step 3: implement. Step 4: run.**
- [ ] **Step 5: mutations, both directions** — the endpoint-wide sum reverted to per
  statement must redden the cap test; the cap set enormous must redden a *different* test
  proving a legitimate two-statement endpoint still publishes.
- [ ] **Step 6: Commit**

---

### Task 6: parameters are the union across statements

**Files:** `crates/egret-dataapi/src/serving/params.rs`

`resolve_supplied(supplied, sql, specs)` takes the union across statements. **The change
belongs in `resolve_supplied` and nowhere else** — serving reaches it through `resolve`,
**preview calls it directly**, and its own comment records that this is the single place a
declared parameter becomes a `Param`. Computing the union upstream would let preview and
serving disagree; this programme has already shipped a defect of exactly that shape.

- [ ] **Step 1: a test that a parameter used by only the second statement is still
  resolved**, and one that preview and serving agree — assert on both entry points, in
  adjacent tests, following the convention at `lifecycle.rs:3208/3209`.
- [ ] **Steps 2-5: implement, mutate (union → first statement only must redden), commit.**

---

### Task 7: serving executes each statement and shapes it against its own columns

**Files:** `crates/egret-dataapi/src/serving/router.rs`

Sequential execution, one `Connector::execute` each. `max_rows` applies **per statement** —
a count returning one row must not consume the row statement's budget. `shape_rows` runs
per statement against **that statement's** frozen columns, keeping M1's subset/required/
type predicate intact.

- [ ] **Step 1: failing tests** — two statements shape independently; a drift in the second
  refuses the whole call with the right code; `max_rows` is charged per statement.
- [ ] **Steps 2-5: implement, mutate both directions, commit.**

---

### Task 8: the envelope

**Files:** `crates/egret-dataapi/src/serving/router.rs`, `crates/egret-dataapi-api/src/lib.rs`

```json
{ "status": "ok", "message": null, "data": …, "meta": { … } }
```

`status` **mirrors** the HTTP status and never replaces it — egret keeps returning real
`403`/`422`/`503` with `ServingErrorBody`. `data` is the rows for one statement, an object
keyed by statement name for several. `meta` is
`{version, cached, row_count, columns}` for one and
`{version, cached, statements:{<name>:{row_count, columns}}}` for several.

`output.envelope: "wrapped" | "bare"`, default `wrapped`; `bare` writes `data`'s value
alone.

**This breaks every existing consumer, once, at deploy.** That is the accepted decision
(§4.4) and it must appear in the deploy note. Do not soften it into a version negotiation.

- [ ] **Step 1: failing tests** — one statement wraps; several statements key by name;
  `bare` emits `data` alone; an error response still carries the real HTTP status.
- [ ] **Step 2-4: implement and run.**
- [ ] **Step 5: mutation** — `status` hard-coded `"ok"` must redden an error-path test.
- [ ] **Step 6: Commit**

---

### Task 9: the generated document describes each endpoint unambiguously

**Files:** `crates/egret-dataapi/src/openapi_gen.rs`

One statement documents `data` as an array of the M1 object schema with its `required`
list; several document `data` as an object with one property per statement name. **OpenAPI
is generated per endpoint, so neither is a union type.**

Any endpoint with more than one statement carries, in its operation description, the plain
sentence that the statements share **no transaction and no snapshot**, so a total and the
rows it counts may disagree if the table changes between them (§4.5). That sentence is the
whole of the consistency promise.

- [ ] **Step 1: failing tests** for both shapes and for the presence of that sentence.
- [ ] **Step 2-4: implement, run, and regenerate `openapi.json`** — it is asserted
  byte-for-byte by a **different crate** (`egret-server`), so run that crate's test
  yourself rather than reporting a green you did not measure.
- [ ] **Step 5: Commit**

---

### Task 10: the cache key does not change, and that is a claim to test

**Files:** `crates/egret-dataapi/src/serving/cache.rs` (read only), tests

The key stream is `endpoint(16) ‖ version(4) ‖ params ‖ principal(16)`. The cached value is
the whole response and republishing bumps `version`, so the statement list is already
covered. **"The cache key must change" is the obvious guess and it is wrong** — pin it:
two endpoints differing only in their statement list must not collide, because their
`endpoint` ids differ; and a republished endpoint must miss, because `version` moved.

- [ ] **Steps 1-5: write, fail, implement nothing if it already holds, mutate the version
  byte out of the key and watch a test redden, commit the tests.**

---

### Task 11: drive it end to end, and prove the upgrade path

**Files:** `crates/egret-dataapi/tests/serving_errors.rs` or a sibling that owns the real router

Tasks 1-10 each test their own half. **None proves the feature works.**

- [ ] **Step 1: the canonical pair.** Publish a two-statement endpoint — `rows` and
  `total` — through the real publish route, call it, and assert the envelope, both result
  sets keyed by name, and both `meta.statements` entries.
- [ ] **Step 2: the upgrade path.** An endpoint published **before** this branch, whose
  snapshot carries `sql` + `columns` and no `statements`, must serve `200` afterwards
  **without being re-published**. Use the `jsonb_set` pattern at
  `tests/serving_routing.rs:898` to forge the old shape, asserting `rows_affected == 1` so
  a forge that missed cannot pass green.
- [ ] **Step 3: prove the gate is untouched.**
  `git diff <base>..HEAD -- crates/egret-dataapi/tests/template_render.rs | wc -c` = 0, and
  that binary green unedited.
- [ ] **Step 4: full sweep.** `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace --no-fail-fast`. **`cargo test --workspace` stops at the first
  failing binary**, so use `--no-fail-fast` and sum the per-binary lines yourself — this
  plan's predecessor had its total wrong twice with every per-binary figure correct.
- [ ] **Step 5: Commit**

---

## Before opening the PR

- **Measure the render cost again.** M1 measured a ~122 ms worst case for
  `MAX_DECLARED_PARAMS = 32`; with N statements that multiplies. Re-measure rather than
  carrying the number over (§4.1.1).
- **Run it on both engines.** M1's merge blocker was a Trino type-string divergence that
  438 passing tests could not see, because every fixture types every column `integer`.
  Publish a two-statement endpoint returning `varchar`, `decimal(p,s)`, `date`, `timestamp`
  and `bigint` on **Trino and StarRocks**, both `output.shape` values.
- **Check it in a browser.** Every live check on this programme has found a defect the
  suites did not. Publish a two-statement endpoint through the real editor, call it from
  Try-it, and read the generated document. The editor must be able to *author* several
  statements at all — if it cannot, that is the finding.
