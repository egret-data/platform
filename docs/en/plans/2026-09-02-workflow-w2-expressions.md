# Workflow W2 — Expressions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `Property<T>` its `render`, by porting Kestra's expression language onto minijinja — and by building the differential oracle that proves the port rather than asserting it.

**Architecture:** Eight tasks. Task 1 builds the oracle and ships no user-visible feature; every task after it is written against golden values rather than against a guess. Task 2 adds the operator rewriter, Task 3 the render entry point, Tasks 4–6 the 35 filters, Tasks 7–8 the 37 functions.

**Tech Stack:** Rust 1.85, `minijinja` 2.24 (already in-tree), `jaq` 3.1, `ion-rs` 1.0, `chrono`; a small Java harness against `io.kestra:core:2.0.0-rc13` for the oracle only.

Spec: `docs/en/specs/2026-09-02-workflow-w2-expressions-design.md`.
Programme spec: `docs/en/specs/2026-09-01-workflow-programme-design.md`.

## Global Constraints

- **Kestra source reference:** `data-portal/kestra` at commit `96893b589`. **Artifact reference:** `io.kestra:core:2.0.0-rc13` on Maven Central, which is compiled for **Java 25** (Corretto 25 is installed at `/Users/1pro-l4r8/Library/Java/JavaVirtualMachines/amazon-corretto-25.jdk/Contents/Home`).
- **Every ported file names its Kestra source on line one**, e.g. `// kestra: core/src/main/java/io/kestra/core/runners/pebble/filters/JqFilter.java`.
- **Wire names come from three places, and getting one wrong makes a filter or function uncallable.** `Extension.java` is where you start, but it does not always hold the name:
  - **32 of the 35 filters** are registered as string literals in `getFilters()` — `filters.put("slugify", …)`. The literal is the name.
  - **3 filters** are registered by constant — `filters.put(RegexMatchFilter.NAME, …)`. Measured: `regexMatch`, `regexReplace`, `regexExtract`. Follow the constant.
  - **All 37 functions** are registered by constant — `functions.put(NowFunction.NAME, …)`. Measured: `ReadFileFunction.NAME == "read"`, **not** `readFile`; `FileURIFunction.NAME == "fileURI"`.

  **Do not derive a name from a filename, and do not assume the registration style is uniform.** I wrote this constraint's first draft claiming filters were all string literals, then found the three that are not by counting: `getFilters()` has 35 `put` calls but only 32 string literals. Count the registrations, extract the names, and reconcile the two — the same discipline that caught `BOOLEAN` versus `BOOL` in W1.
- **`RenderFunction` and `RenderOnceFunction` are registered conditionally** (`if (renderFunction != null)`) so a standalone `new Extension()` may not carry them. The oracle must record which functions its engine actually has, rather than assuming all 37.
- **The oracle's golden file is the contract.** Whoever adds a filter or function adds corpus entries in the same change. A filter with no corpus entry is unproven — this is the rule W1 arrived at after `Quota`/`Check` fell out of a tagged sweep because nothing checked the tagging.
- **`models/` and `spec/` must not touch the database or the network.** The renderer lives in `runners/`, matching Kestra's package layout.
- **The SQL seam is not W2's.** `egret.sql` renders through `egret-dataapi`'s `bind_formatter`, where `{{ }}` cannot reach raw SQL text by construction. Do not introduce a second path that renders SQL.
- **Verification:** `cargo test -p egret-workflow --no-fail-fast` fully green, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`. **Do not run `cargo test --workspace`** — the controller runs it; two cargo runs fight over the `target/` lock.
- **Database, where a task needs one:** `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`. Never the dev database. `just migrate` does **not** read `DATABASE_URL` — it reads `EGRET_DATABASE__URL`.
- Conventional Commits. Do not merge; open nothing.

## Operational rules, every one of which has cost this programme real time

- **Never use the `Monitor` tool. Never pass `run_in_background: true`.** Foreground only, `timeout: 600000`. Five agents have stalled on this.
- **Never run `git checkout -- <path>` without running `git status` first.** One agent destroyed a whole fix round that way.
- **Never restore a file with `cp`** — it is aliased to `cp -i`, silently refuses to overwrite, and returns success.
- **Never generate Rust or Java source through a Python heredoc** — the double escape-processing corrupts string literals. Use the Edit/Write tools.
- **A reviewer that needs to mutate or stash must work in its own `git worktree`**, never the shared checkout.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/egret-workflow/oracle/pom.xml`, `oracle/src/main/java/host/Oracle.java` | Java harness: renders the corpus through real Pebble, writes the golden file |
| `crates/egret-workflow/oracle/corpus.json` | the expressions + contexts, hand-authored, the artifact under review |
| `crates/egret-workflow/oracle/golden.json` | committed output of the harness; CI reads it with no JVM |
| `crates/egret-workflow/src/runners/mod.rs` | the render seam |
| `crates/egret-workflow/src/runners/rewrite.rs` | tokenizer-based operator rewrite (`??`, `???`, `isIn`, ternary) |
| `crates/egret-workflow/src/runners/env.rs` | the minijinja `Environment`, filter/function registration, fuel limit |
| `crates/egret-workflow/src/runners/filters/*.rs` | the 35 filters, grouped as Tasks 4–6 split them |
| `crates/egret-workflow/src/runners/functions/*.rs` | the 37 functions |
| `crates/egret-workflow/src/runners/javafmt.rs` | Java `DateTimeFormatter` pattern → chrono translator |
| `crates/egret-workflow/tests/oracle.rs` | the differential test: render the corpus, diff against golden |

---

### Task 1: The oracle

**This task ships no filter and no user-visible behaviour, and that is the point.** Every later task is written against a golden value instead of against its author's expectation. The W5 spike already proved the engine can be built standalone, so this is assembly, not research.

**Files:**
- Create: `crates/egret-workflow/oracle/pom.xml`, `oracle/src/main/java/host/Oracle.java`, `oracle/corpus.json`, `oracle/golden.json`, `oracle/README.md`
- Create: `crates/egret-workflow/tests/oracle.rs`
- Modify: `crates/egret-workflow/Cargo.toml` (dev-dependency on `serde_json` only)

**Interfaces:**
- Produces: `oracle/golden.json`, an array of `{ "id": String, "expr": String, "context": Value, "result": String | { "error": String } }`; and `tests/oracle.rs`'s `render_corpus_matches_golden()`, which will fail until Task 3 provides a renderer — see Step 6.

- [ ] **Step 1: Stand up the Java harness**

`crates/egret-workflow/oracle/pom.xml` depends on `io.kestra:core:2.0.0-rc13`. The engine is four lines, because `io.kestra.core.runners.pebble.Extension` has a public no-arg constructor and needs no Micronaut:

```java
PebbleEngine engine = new PebbleEngine.Builder()
    .extension(new io.kestra.core.runners.pebble.Extension())
    .strictVariables(false)
    .cacheActive(false)
    .build();
```

Build and run with Corretto 25 (`export JAVA_HOME=/Users/1pro-l4r8/Library/Java/JavaVirtualMachines/amazon-corretto-25.jdk/Contents/Home`) — Kestra 2.x is class-file version 69 and will not load on JDK 21.

- [ ] **Step 2: Record what the engine actually has**

Before rendering anything, have the harness print the registered filter, function and test names it finds on the built engine, and write them into `oracle/README.md`. `RenderFunction` and `RenderOnceFunction` are registered conditionally, so a standalone engine may carry fewer than 37 functions.

**Report the actual counts.** The spec says 35/37/1 from reading `Extension.java`; if the built engine disagrees, the engine is the authority and later tasks must be scoped to it.

- [ ] **Step 3: Write the seed corpus**

`oracle/corpus.json`, hand-authored. It must cover, at minimum, one entry per registered filter and function, plus these operator cases, which are the ones the port is most likely to get wrong:

```json
[
  {"id": "coalesce-null",        "expr": "{{ missing ?? 'fallback' }}",        "context": {"missing": null}},
  {"id": "coalesce-present",     "expr": "{{ present ?? 'fallback' }}",        "context": {"present": "x"}},
  {"id": "coalesce-absent",      "expr": "{{ nothere ?? 'fallback' }}",        "context": {}},
  {"id": "undef-coalesce-null",  "expr": "{{ missing ??? 'fallback' }}",       "context": {"missing": null}},
  {"id": "undef-coalesce-absent","expr": "{{ nothere ??? 'fallback' }}",       "context": {}},
  {"id": "ternary",              "expr": "{{ flag ? 'yes' : 'no' }}",          "context": {"flag": true}},
  {"id": "isin",                 "expr": "{{ 'b' isIn ['a','b'] }}",           "context": {}},
  {"id": "gt-strings",           "expr": "{{ 'b' > 'a' }}",                    "context": {}},
  {"id": "gt-numbers",           "expr": "{{ 2 > 10 }}",                       "context": {}},
  {"id": "gt-mixed",             "expr": "{{ '2' > 10 }}",                     "context": {}},
  {"id": "op-inside-string",     "expr": "{{ 'a ?? b' }}",                     "context": {}},
  {"id": "nested-ternary",       "expr": "{{ a ? (b ? 'x' : 'y') : 'z' }}",    "context": {"a": true, "b": false}}
]
```

`undef-coalesce-null` versus `coalesce-null` is the pair that pins `??` against `???`; `op-inside-string` and `nested-ternary` are what will break a regex-based rewriter.

- [ ] **Step 4: Generate the golden file**

The harness renders each entry and writes `{id, expr, context, result}` — capturing the exception's class and message as `{"error": "..."}` for entries that throw, since a faithful port must reproduce failures too, not only successes.

Run it, commit `golden.json`, and **read the output before trusting it**. Record in your report any result that surprises you; the spike already found that `{{ json | jq('.a.b') }}` returns `[42]`, a list rather than a scalar.

- [ ] **Step 5: Write the Rust differential test**

`crates/egret-workflow/tests/oracle.rs` reads `golden.json`, renders each `expr` against each `context` through egret's renderer, and asserts equality. It must report **every** mismatch with id, expected and actual — not fail on the first — because later tasks will use its output as a worklist.

- [ ] **Step 6: Run it and confirm it fails for the stated reason**

Run: `cargo test -p egret-workflow --test oracle`
Expected: FAIL to compile — egret has no renderer yet (Task 3 adds it). Gate the test behind `#[ignore]` with a comment naming Task 3, so the suite stays green until there is something to compare, and remove the `#[ignore]` in Task 3.

**Do not stub a renderer to make this pass.** A green oracle with no renderer behind it is worse than a red one.

- [ ] **Step 7: Document how to regenerate**

`oracle/README.md`: the exact commands, the `JAVA_HOME` requirement, the pinned artifact version, and the rule that a regenerated `golden.json` must be reviewed as a diff — a changed golden value means either Kestra changed or the corpus changed, and both need a human to say which.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-workflow/oracle crates/egret-workflow/tests/oracle.rs crates/egret-workflow/Cargo.toml
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "test(workflow): differential oracle against real Pebble

Renders a corpus through Kestra's own Pebble extension and commits the result as
a golden file, so every filter and function ported after this is written against
a measured value rather than its author's expectation.

The engine needs no Micronaut: Extension has a public no-arg constructor. CI
reads the committed golden file and needs no JVM; regenerating it is an opt-in
job that surfaces drift when the pinned Kestra version moves."
```

---

### Task 2: The operator rewriter

**Files:**
- Create: `crates/egret-workflow/src/runners/mod.rs`, `crates/egret-workflow/src/runners/rewrite.rs`
- Modify: `crates/egret-workflow/src/lib.rs`

**Interfaces:**
- Produces: `pub fn rewrite(expr: &str) -> Result<String, RewriteError>` — takes a Kestra expression, returns one minijinja accepts, leaving everything it does not recognise untouched.

- [ ] **Step 1: Read the four operators' exact semantics from the Java**

`core/src/main/java/io/kestra/core/runners/pebble/expression/` — `NullCoalescingExpression`, `UndefinedCoalescingExpression`, `InExpression`, `GreaterThanExpression` and its three siblings. **Record each one's behaviour in your report before writing any Rust.**

The two coalescing operators are not the same, and the difference is the whole reason this task is not a find-and-replace:

- `??` — evaluate left; non-null → left; **null → right**; attribute missing → right.
- `???` — evaluate left and return it **even when null**; only a missing attribute falls to right.

- [ ] **Step 2: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::rewrite;

    /// The rewriter must not touch an operator inside a string literal. A regex
    /// would; that is why this is a tokenizer.
    #[test]
    fn an_operator_inside_a_string_literal_is_left_alone() {
        assert_eq!(rewrite("{{ 'a ?? b' }}").unwrap(), "{{ 'a ?? b' }}");
        assert_eq!(rewrite(r#"{{ "x ? y : z" }}"#).unwrap(), r#"{{ "x ? y : z" }}"#);
    }

    /// `???` must not be mistaken for `??` followed by `?`.
    #[test]
    fn the_longer_operator_wins() {
        let out = rewrite("{{ a ??? b }}").unwrap();
        assert!(out.contains("undefined"), "expected the undefined-coalescing form, got {out}");
        assert!(!out.contains("null_coalesce"), "{out}");
    }

    #[test]
    fn nested_ternaries_rewrite_innermost_first() {
        let out = rewrite("{{ a ? (b ? 'x' : 'y') : 'z' }}").unwrap();
        // Both conditionals survive, and no `?` or `:` is left as an operator.
        assert_eq!(out.matches(" if ").count(), 2, "{out}");
        assert!(!out.contains(" ? "), "{out}");
    }

    #[test]
    fn text_outside_an_expression_is_untouched() {
        assert_eq!(rewrite("literal ?? text").unwrap(), "literal ?? text");
    }
}
```

- [ ] **Step 3: Run and confirm the tests fail to compile**

Run: `cargo test -p egret-workflow --lib runners::rewrite`
Expected: FAIL to compile — `rewrite` does not exist.

- [ ] **Step 4: Implement the rewriter**

It scans `{{ … }}` and `{% … %}` regions only, and inside them tokenizes: string literals (both quote styles, with escapes), parentheses and brackets for nesting, and operators matched **longest-first** so `???` is never read as `??` followed by `?`.

Map to minijinja: `a ?? b` and `a ??? b` become calls to two custom functions Task 3 registers (they need different semantics, so they cannot share one); `c ? a : b` becomes `a if c else b`; `x isIn y` becomes `x in y` **provisionally** — Task 3's oracle run decides whether minijinja's `in` actually agrees with Kestra's `InExpression`, and if it does not, `isIn` becomes a custom function too.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p egret-workflow --lib runners::rewrite`
Expected: PASS, 4 tests.

- [ ] **Step 6: Prove the tokenizer is load-bearing (mutation)**

Replace the tokenizer's string-literal handling with a plain `str::replace` for `??`. Confirm it **compiles** (`cargo build -p egret-workflow --tests`, 0 errors) and that `an_operator_inside_a_string_literal_is_left_alone` **fails** while the others still pass. Restore with `git checkout --` after a `git status` check, and confirm green.

- [ ] **Step 7: Commit**

```bash
cargo test -p egret-workflow --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
git add crates/egret-workflow
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): rewrite Kestra's operators for minijinja

minijinja has no ??, ???, or ternary. The rewrite tokenizes rather than
substitutes: ?? inside a string literal, nested ternaries, and ??? overlapping ??
all defeat a regex, and the first of those is a live corpus entry.

?? and ??? are deliberately mapped to two different functions: ?? falls back on
null OR a missing attribute, ??? only on a missing attribute, and collapsing them
is the mistake this task exists to avoid."
```

---

### Task 3: The environment and `Property<T>::render`

**Files:**
- Create: `crates/egret-workflow/src/runners/env.rs`
- Modify: `crates/egret-workflow/src/models/property.rs`, `crates/egret-workflow/tests/oracle.rs`

**Interfaces:**
- Consumes: `rewrite` from Task 2.
- Produces: `RenderContext` (a `BTreeMap<String, serde_json::Value>` wrapper); `fn environment() -> minijinja::Environment<'static>`; `impl<T: DeserializeOwned> Property<T> { pub fn render(&self, cx: &RenderContext) -> Result<T, RenderError> }`; and the two coalescing functions the rewriter targets.

- [ ] **Step 1: Build the environment**

`environment()` constructs a `minijinja::Environment`, registers the two coalescing functions, and sets a **fuel limit** — egret's minijinja already enables the `fuel` feature. Author-supplied templates are untrusted input; pick a limit, and say in a comment what it protects against and what a legitimate flow's expression costs.

- [ ] **Step 2: Implement `render`**

Rendering and conversion are one step, matching Kestra's `runContext.render(prop).as(Class)`. A `Property<T>` whose source contains no `{{ }}` must render to itself — that identity is what makes the W5 plugin host's pre-rendering coherent, so **test it explicitly**.

- [ ] **Step 3: Un-ignore the oracle test and run it**

Remove Task 1's `#[ignore]`.

Run: `cargo test -p egret-workflow --test oracle -- --nocapture`
Expected: **a long list of mismatches.** That is correct — no filter or function is ported yet. Capture the output; it is Tasks 4–8's worklist.

**Record the mismatch count in your report.** It is the number Tasks 4–8 drive to zero, and the only honest baseline for their progress.

- [ ] **Step 4: Settle the operator questions with measurements**

The oracle now answers what reading could not. Report, with the golden values beside egret's:

- do minijinja's `>`, `>=`, `<`, `<=` agree with Kestra's string-comparing overrides, for `gt-strings`, `gt-numbers` and `gt-mixed`?
- does minijinja's `in` agree with Kestra's `InExpression` for `isin`?

Where they disagree, say so plainly and leave the entry failing — Task 4 or a follow-up converts that operator to a custom function. **Do not adjust the corpus to match egret.** The corpus is the contract.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-workflow
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): render Property<T> through minijinja

Adds the environment, the two coalescing functions the rewriter targets, and
render(), which converts in the same step as Kestra's render(prop).as(Class).

The oracle test is now live and failing loudly: it lists every expression egret
does not yet reproduce, and that list is the worklist for the filter and function
tasks."
```

---

### Tasks 4–6: The 35 filters

Each task follows the same shape, and the split is by dependency weight rather than by alphabet.

**Task 4 — string and collection filters, no new dependencies** (~20 filters): `chunk`, `className`, `distinct`, `endsWith`, `escapeChar`, `flatten`, `indent`, `keys`, `nindent`, `number`, `slugify`, `startsWith`, `string`, `substringAfter`, `substringAfterLast`, `substringBefore`, `substringBeforeLast`, `urldecode`, `values`, and the hashes (`md5`, `sha1`, `sha512`, and whatever else `getFilters()` lists that needs only `sha2`/`md-5`, which are already in the workspace).

**Task 5 — date and time filters** (~6 filters): `date`, `dateAdd`, `timestamp`, `timestampMicro`, `timestampMilli`, `timestampNano`. **This task owns `javafmt.rs`.**

**Task 6 — structured-data filters** (~5 filters): `jq` (`jaq` 3.1), `toIon` (`ion-rs` 1.0), `toJson`, `yaml`, and any remaining.

For each of the three:

- [ ] **Step 1: Take the exact list from the authority**

`getFilters()` has **35** `filters.put` calls but only **32** string literals — the last three go through `RegexMatchFilter.NAME`, `RegexReplaceFilter.NAME` and `RegexExtractFilter.NAME` (`regexMatch`, `regexReplace`, `regexExtract`). **Count the `put` calls and reconcile them against the names you extracted**; if the two numbers differ, you have missed a constant-registered entry. Then confirm the whole list against the oracle's registration dump from Task 1 Step 2, which reports what the built engine actually carries.

- [ ] **Step 2: Add corpus entries first, and regenerate the golden file**

For each filter in your batch, add corpus entries covering its documented behaviour **and its edge cases** — an empty input, a null input, a wrong-typed input. Regenerate `golden.json` with the Java harness and **read the diff**. You are now looking at what Kestra actually does, before you have written a line of Rust.

Report anything that contradicts what the filter's name suggests. The `jq`-returns-a-list finding came from exactly this step.

- [ ] **Step 3: Implement, driving the oracle's mismatch count down**

Run `cargo test -p egret-workflow --test oracle -- --nocapture` after each filter. The mismatch list is the worklist; the count only goes down.

- [ ] **Step 4: The `date` filter is the hard one (Task 5 only)**

`{{ d | date("yyyy-MM-dd'T'HH:mm:ss") }}` is a Java `DateTimeFormatter` pattern: not strftime, not chrono's syntax, with single quotes escaping literal text. **No Rust crate provides this** — `javafmt.rs` translates the pattern to chrono's.

Cover at minimum: `y`/`u` year forms, `M` versus `L`, `d`/`D`, `H`/`h`/`k`/`K`, `m`, `s`, `S` fractional seconds, `z`/`Z`/`X` zones, quoted literals (`'T'`), doubled quotes (`''` for a literal quote), and pattern-letter counts (`M`, `MM`, `MMM`, `MMMM`). Every one of these is a corpus entry with a golden value; none of them is a guess.

- [ ] **Step 5: Mutation, per task**

Pick the filter in your batch whose behaviour is least obvious — for Task 4 that is `slugify` or `number`, for Task 5 it is `date`'s pattern translation, for Task 6 it is `jq`. Break it in a way that **compiles**, confirm the oracle test fails on exactly that filter's entries, and restore.

- [ ] **Step 6: Commit**, with the remaining mismatch count in the message so the next task starts from a stated number.

---

### Tasks 7–8: The 37 functions

**Task 7 — pure functions**: `now`, `uuid`, `ksuid`, `nanoId`, `randomInt`, `randomPort`, `id`, `fromJson`, `fromIon`, `yaml`, `dayOfMonth`, `dayOfWeek`, `hourOfDay`, `monthOfYear`, `isWeekend`, `isPublicHoliday`, `isLastWorkingDay`, `isDayWeekInMonth`, and the rest that need no service.

**Task 8 — service-backed functions, ported as explicit refusals**: `secret`, `kv`, `read`, `fileURI`, `fileExists`, `fileSize`, `isFileEmpty`, `render`, `renderOnce`, `errorLogs`, `fetchContext`, `loopOutputs`, `tasksWithState`, `subflow`, `env`, `encrypt`, `decrypt`, `http`.

- [ ] **Step 1: Read every wire name from its class's `NAME` constant**

Functions are registered as `XxxFunction.NAME`, **not** by filename. Measured examples: `ReadFileFunction.NAME == "read"`, `FileURIFunction.NAME == "fileURI"`. Produce the full name-to-class table in your report before implementing. A name taken from a filename produces a function nothing can call, and no test would notice because nothing would reference it.

- [ ] **Step 2: Confirm against the oracle's registration dump**

Task 1 Step 2 recorded the names the built engine actually carries. `RenderFunction` and `RenderOnceFunction` are conditionally registered. If your list and the dump disagree, **the dump wins** and you say so.

- [ ] **Step 3 (Task 8): A refusal is a feature, and it must say what it is waiting for**

`secret()`, `kv()`, `read()` and the rest reach services W2 does not have. Each must fail with a message naming **the function, and the package that will implement it** — never an empty string, never a null. A silent empty is a wrong value that renders successfully; that is the failure mode this whole programme keeps meeting.

Add a corpus entry per refusing function asserting the refusal, so nobody later "fixes" it by returning an empty.

- [ ] **Step 4: Drive the oracle to zero, mutate one function, commit** — the same shape as Tasks 4–6.

---

## Done when

- `cargo test -p egret-workflow --test oracle` passes with **zero mismatches**, or every remaining mismatch is listed in the report with a reason and a decision.
- Every filter and function has at least one corpus entry, and every corpus entry has a golden value produced by real Pebble.
- `cargo test -p egret-workflow --no-fail-fast` green; `cargo fmt --check` and `cargo clippy -D warnings` clean.
- `oracle/README.md` documents regeneration, and the golden file has been reviewed as a diff at least once.
- The four comparison operators and `isIn` have a recorded measurement, not an assumption, about whether minijinja agrees with Kestra.

Open the PR and **stop**. Do not merge.

## Deliberately not in W2

No executor, no task handlers, no execution-context construction. `{{ outputs.x.y }}` renders against a context W2 cannot yet build — W3 builds it. No expression evaluation inside the SQL seam; that path stays `egret-dataapi`'s.
