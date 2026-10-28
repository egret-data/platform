# Survey for roadmap package #5, "Renderer parity"

Worktree: `/Users/1pro-l4r8/Documents/github/data-portal/egret-render` (branch `feat/workflow-renderer`, from main `5d5289f0`).
Kestra: `/Users/1pro-l4r8/Documents/github/data-portal/kestra` (pinned `96893b589`, 2.0.0-SNAPSHOT). The oracles' pinned engine is `io.kestra:core:2.0.0-rc13` + Pebble 4.1.2.
Abbreviations: `EW` = `crates/egret-workflow/`, `KR` = `kestra/core/src/main/java/io/kestra/core/runners/`.
This is a read-only survey. Items marked **(unverified)** were inferred and not measured.

Roadmap anchors: row 5 at `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md:78`. The cause list ("Renderer divergence (8)", "Missing Pebble functions (2)") is at `:275-280`, the cause table at `:326`, `:337`, the Appendix "Pebble functions, filters and tests" at `:548-570`, and note 10 (`valids/task-runif-taskswithstate`) at `:155`.

## 0. How the premises were measured

Each item was measured on both engines with the same probes, without editing code:

- **Kestra side:** the W2 expression oracle harness (`EW oracle/src/main/java/host/Oracle.java`) was copied to the scratchpad and run offline (`mvn -o`, Corretto 25) with `generate` over 99 probe expressions. This is the same engine and strict-variables setting that produced `EW oracle/golden.json`.
- **egret side:** a scratch binary renders the same 99 probes through `egret_workflow::render_str` (the function `tests/oracle.rs` calls), built against this worktree.
- **Corpus counts:** `EW oracle/golden.json` (1 336 expression entries, 129 baselined) and `EW oracle-exec/corpus` + `golden.json` (301 entries, 50 baselined) were scanned with scripts.
- **Not run:** `cargo test -p egret-workflow --test executor`. Bank counts below are therefore **upper bounds** from cause attribution, not measurements.

Probe results are in the scratchpad (`r5-oracle/probe-golden.json`, `r5-probe/results.txt`). Each one that matters is quoted in the tables below as `egret → Kestra`.

---

## 1. Item by item

### 1.1 Division by zero (the `restart-*` Infinity case)

| Probe | egret | Kestra |
|---|---|---|
| `{{ 1 / 0 }}`, `{{ 1 / n }}` (n=0) | `Infinity` | error `Could not perform division` |
| `{{ fromIon(ion='1.5') / 0 }}` | `Infinity` | error |
| `{{ 1.0 / 0 }}` | `Infinity` | `Infinity` (double) |
| `{{ 7 % 0 }}` | error | error |
| **`{{ 7 / 2 }}`, `{{ a / b }}` (7, 2)** | **`3.5`** | **`3`** |
| `{{ -7 / 2 }}` | `-3.5` | `-3` |
| **`{{ 9223372036854775807 + 1 }}`** | `9223372036854775808` | `-9223372036854775808` (long wraps) |
| `{{ 4611686018427387904 * 4 }}` | `18446744073709551616` | `0` |

- **egret today:** there is no arithmetic seam. `+ - * / %` are minijinja's own operators. minijinja's `/` always divides as floats, and integers widen to i128 instead of wrapping. The comparison rewrite (`EW src/runners/cmp_rewrite.rs`) is the model for a fix: it compiles `<` into `kestra_lt(..)` and so on.
- **Kestra:** Pebble's `OperatorUtils` widens. Long with Long is Java `long` arithmetic: it divides as integers, wraps on overflow, and throws on `/ 0`. Any double makes the result a double. BigDecimal is covered in §1.13.
- **Premise:** correct, but **too narrow.** Integer division (`7/2` is `3`) and long overflow are the same root cause, and the roadmap does not name either. No golden entry exercises them: the expression oracle has only 4 arithmetic entries, all BigDecimal (`function-fromion-decimal-*`, `filter-number-bigdecimal-*`).
- **Corpus:** `valids/restart-child` (`{{ 1 / taskrun.attemptsCount }}`, `restart-child.yaml:10`) and `restart-loop` (`:14`). `restart-parent` and `restart-parent-loop` go through `restart-child`. **4 entries** (`oracle-exec/baseline.txt:186-189`, `tests/executor.rs:5255`). Note `restart-child` is in `STILL_OVER_ACCEPTED_AT_RUNTIME` (`tests/executor.rs:5238-5258`), and that register must shrink.
- **Size:** M for one arithmetic seam. It is a third pass in the `cmp_rewrite` style (or an extension of it) that turns `+ - * / %` into `kestra_add` and the others, plus one numeric-tower function shared with §1.13. Unary minus and `~` stay as they are.
- **Risk:** **this is a behaviour change for stored flows** (`{{ x / 2 }}` on ints changes its answer). Precedence must stay minijinja's, as `cmp_rewrite` guarantees for comparisons. `range`, `loop.index` and the date functions do their own arithmetic and are unaffected.

### 1.2 Undefined `taskrun.value`

| Probe | egret | Kestra |
|---|---|---|
| `<{{ taskrun.value }}>` with `{"value": null}` | `<>` | `<>` |
| `<{{ taskrun.value }}>` with no `value` key | error | error (`AttributeNotFoundException`) |

- **The renderer already agrees with Pebble.** The divergence is in the **render context**: `EW src/worker/render.rs:866` always writes `"value": r.value`, which is `null` when there is no value. Kestra writes the key only when the value is non-null (`KR RunVariables.java:162-164`; `parentId` and `iteration` follow the same rule at `:158`, `:166`).
- **Premise:** right about the effect, but **mislabelled as a renderer item**. The fix is the context builder.
- **Corpus:** `valids/all-flowable` (Loop body) and `valids/disable-flowable` (Switch case), **2 entries**. Both are in `STILL_OVER_ACCEPTED_AT_RUNTIME` (`tests/executor.rs:5247-5254`).
- **Size:** S. Omit `value` (and check `parentId`/`iteration`) when it is `None`.
- **Risk:** any egret flow that relied on `{{ taskrun.value }}` rendering empty now fails, which is Kestra's behaviour. Re-check `Loop`/`ForEach` iteration runs, where egret may set `value` and Kestra may not **(unverified)**.

### 1.3 Number-vs-string comparison

| Probe | egret | Kestra |
|---|---|---|
| `{{ n < '5' }}` (n=3), `{{ a > b }}` (3, "2") | `true` / `false` | error `Could not perform … comparison` |
| `{{ true < 3 }}`, `{{ false < true }}`, `{{ [1] < [2] }}` | `true` | error |
| **`{{ n < 1 }}` (n=null)** | **`true`** | **`false`** |
| `{{ 'a' < 'b' }}`, `{{ 1.5 > 1 }}` | `true` | `true` |
| `{{ 3 == '3' }}` | `false` | `false` |

- **egret today:** every relational operator already goes through one chokepoint, `CmpOp::apply` (`EW src/runners/compare.rs:357-379`), which falls back to minijinja's total order (`values_cmp`, `:270`).
- **Kestra:** numbers compare numerically and strings compare with `compareTo`. Any other pair throws. A null operand answers `false`.
- **Premise:** correct. New finding: `null < 1` also diverges.
- **Corpus:** expression `gt-mixed`, `lt-mixed`, `lte-mixed`, `gte-mixed` (`oracle/baseline.txt:26-31`, **4**). Executor `valids/loopuntil-failed-flowable` (`{{ outputs.loop.iterationCount < "5" }}`, **1**).
- **Size:** S. One `match` in `apply`, plus the pinned tripwire `the_one_corpus_comparison_pebble_refuses_and_minijinja_answers` (`EW src/executor/mod.rs:9708`), which asserts the divergent answer on purpose and must flip.
- **Risk:** low. A stored flow comparing a number with a string starts failing, which is Kestra's behaviour.

### 1.4 `??` on a missing root

| Probe | egret | Kestra |
|---|---|---|
| `{{ nothere ?? 'h' }}` | `h` | `h` |
| `{{ trigger.body ?? 'h' }}`, `{{ trigger.parameters.x ?? 'h' }}` | error `undefined value` | `h` |
| `{{ trigger.parameters.validationToken[0] ?? 'hello-world' }}` | error | `hello-world` |
| `{{ obj.a.b ?? 'h' }}` (obj = {}) | error | `h` |
| `{{ x ?? missing.y }}` (x=1) | **error** | `1` (the right side is lazy) |

- **egret today:** `??` is rewritten to `kestra_null_coalesce(left, right)` (`EW src/runners/env.rs:938-957`). A bare missing root reaches the function as `Undefined` and works (`coalesce-absent` passes). But `UndefinedBehavior::Strict` raises on the first `.attr`/`[i]` **after** a missing value, before the call is reached. Both operands are evaluated eagerly (`env.rs:951-955`).
- **Premise: partly stale.** "Missing root" already works. The gap is **a member or index chain through a missing value** (root or member), and the **eager right side**.
- **Corpus:** expression `coalesce-nested-absent` (**1**, baselined as "Deliberate: the alternatives are worse", `oracle/baseline.txt:79-80`, a recorded ruling to revisit). Executor `valids/webhook-plaintext` (**1**).
- **Size:** M. Options: (a) the rewriter compiles the left operand of `??` into a safe-navigation call (`kestra_path(root_or_undefined, 'a', 'b', 0)`), and the right side into a lazy form such as minijinja's inline `if` (`(L) if kestra_present(L) else (R)`, with L computed once through `{% set %}` or a helper); (b) relax strictness only inside `??` operands. Option (a) fits the existing rewriter (`EW src/runners/rewrite.rs`).
- **Risk:** the rewriter is the most delicate code in the crate (precedence tiers, `rewrite.rs:1096-1130`). `???` (`kestra_undefined_coalesce`) needs the same treatment.

### 1.5 `render` / `renderOnce`

- **egret today:** both are refused with "needs an execution context to render against", owner W3 (`EW src/runners/functions.rs:299-311`). Measured: `{{ render('x') }}` gives that refusal.
- **Kestra:** `KR pebble/functions/RenderFunction.java` takes `toRender` and `recursive` (default `true`). It re-renders with the caller's whole global scope. Its depth is capped by `VariableConfiguration.DEFAULT_MAX_RENDER_DEPTH = 16`, which throws "Maximum render() nesting depth (16) exceeded". A non-string is returned as it is (`renderObject`). It is registered only when `kestra.variables.recursive-rendering=false`, which is the default. `renderOnce` is `recursive=false`. The **standalone** oracle engine does not carry either (`oracle/name-surface.txt` rows `render`, `renderOnce`; probe `render-fn` errors on both sides), so the expression oracle **cannot** test them: tests must be unit tests plus the executor corpus.
- **Corpus:** `valids/variables` (`{{render(vars.third)}}`, **1**; its other two tasks need `envs`, which egret lacks, so `env` FAILS in both engines). `valids/replay-revision` (`render(vars.greeting)` over an undefined name) matches today because the refusal fails the task. It is a **guard**: it must still FAIL after the change.
- **Size:** M. It needs the current context inside a minijinja function. `fetchContext` already reads the render context's names through the `egret context scope` global (`oracle/name-surface.txt` note at `:56-58`). Add a depth counter and a size bound.
- **Risk (security):** `render(inputs.x)` evaluates expressions that a user supplied at run time. egret already confines `secret()` and refuses `http()`, so what this exposes is `kv()`, `read()` and the file functions within the execution's own workspace. See Q4.

### 1.6 `errorLogs`

- **egret today:** refused, owner W11 (`functions.rs:312`).
- **Kestra:** `KR pebble/functions/ErrorLogsFunction.java`. It works only on the worker ("can only be used in the Worker"). It returns the execution's ERROR `LogEntry` list and **retries for up to 5 s** while the list is empty (exponential, 100 ms to 1 s), then returns `[]`.
- **Premise stale:** the roadmap and baseline say it "needs a task-log store". egret has one since W11 (`EW src/logs/{repo,writer,run_log}.rs`, `tests/task_logs.rs`). What is missing is a query (by execution, level ERROR) and a **prefetch**: minijinja functions are synchronous, so the worker surface must load the logs before the render. That follows the kv/namespace-file prefetch pattern (`tests/prefetch_hold.rs`).
- **Corpus:** `valids/errors` (**1**). Its golden checks only states, so any list that does not throw banks it.
- **Size:** S–M. Decide the 5 s wait. egret's log writer batches, so an empty first read is likely.
- **Risk:** a log message is user data, so render it as data and never re-render it.

### 1.7 `subflow()`

- **egret today:** refused, owner "W6" (`functions.rs:313`).
- **Kestra:** `KR pebble/functions/SubflowFunction.java`. It **starts a whole execution synchronously inside a render**, waits for it (`runAndWait`, default timeout `PT1M`, maximum `PT5M`, per-thread depth 3; `SubflowFunctionConfiguration.java`) and returns `{id, state, outputs, labels}`, where `outputs` is the child's **flow-level** `outputs:`. It is registered only on server types that render execute forms. It is meant for input `defaults` in the run form.
- **Corpus:** `subflow-function-recursive`, `-mutual-a`, `-mutual-b` and `-child`, **0 golden entries**. They are not standalone (`oracle-exec/README.md:741-764`).
- **Premise wrong in scope.** This is not renderer work. It needs flow `outputs:` evaluation (row #6, note 6), a synchronous start-and-wait from a render (blocking a request or claim thread), and Subflow authz. It banks nothing. **Recommend moving it out of #5** (Q1).

### 1.8 `max` / `min`

| Probe | Kestra |
|---|---|
| `max(1, 5, 3)` / `min(2, 1)` | `5` / `1` |
| `max(1, 2.5)` | `2.5` |
| `max(4)`, `max([1,2])` | `4`, `[1,2]` (one argument is returned as it is) |
| `min(null, 1)` | `1` |
| `max('a', 'b')` | error `invalid operands for mathematical comparison` |

egret: `unknown function` (`name-surface.txt:194` and the `min` row). Corpus: **0**. Size: S, built on §1.3's comparison.

### 1.9 The ten missing filters (the count is verified)

`awk '$3=="yes" && $4=="no"' oracle/name-surface.txt` lists exactly ten filters: **`abbreviate`, `base64decode`, `base64encode`, `merge`, `nl2br`, `numberformat`, `raw`, `rsort`, `sha256`, `urlencode`**. All are Pebble core, and Kestra registers none of them itself. Corpus: **0** in both oracles. egret answers `unknown filter` for all ten.

| Filter | Kestra (measured) | Note |
|---|---|---|
| `abbreviate(n)` | `'this is a long sentence'\|abbreviate(10)` gives `this is...`; `'abc'\|abbreviate(2)` gives `abc` | `n <= 0` throws |
| `base64encode` / `base64decode` | UTF-8; a bad input throws "Please provide a correctly Base64 encoded string…" | |
| `merge` | list+list concatenates; map+map has the right side win; list+map appends the map | |
| `numberformat(fmt?)` | `#,##0.00` gives `1,234.57`; with no argument it gives `1,234.568` | `java.text.DecimalFormat` with the **context locale** (JVM default). egret needs a pinned locale, see B2 |
| `rsort` | `[3,2,1]`, `["c","b","a"]` | |
| `sha256` | hex | `sha1`/`sha512`/`md5` already exist (`filters.rs`) |
| `urlencode` | `a+b%26c%3Dd%2F%C3%A9` (`URLEncoder`, form encoding) | `urldecode` exists |
| `raw` | identity (Kestra runs with `autoEscaping(false)`, `KR pebble/PebbleEngineFactory.java:163`) | egret handles raw **tags** already (`env::replace_raw_tags`) |
| `nl2br` | `a<br />b` with no escaping | |

Size: S each, about M together. A side finding: a **string literal that spans a newline** is legal in Pebble but egret's rewriter refuses it ("unterminated string literal", by design: `rewrite.rs` "bounded by physical line"). Recorded only.

### 1.10 The four missing tests (the count is verified), and three wrong ones

The same scan lists **`null`, `empty`, `map`, `json`**. egret answers `unknown test` for each. `null` arrives as `test none is unknown` because the `null` literal is translated to minijinja's `none` (`name-surface.txt:240`).

| Probe | Kestra |
|---|---|
| `x is null` (null) / `x is not null` (1) | `true` / `true`; a missing root throws (strict) |
| `x is empty` for `""`, `"  "`, `[]`, `{}`, null | `true` (**whitespace counts as empty**) |
| `0 is empty` | `false` |
| `x is map` | `{}` gives `true`, `[]` gives `false` |
| `x is json` | `"1"` and `'{"key":1}'` give `true`, `"plain"` gives `false`; **a map throws** `ClassCastException` |

Also divergent and named nowhere in the row: `even`, `odd` and `iterable` (`oracle/baseline.txt` `test-even`, `test-odd`, `test-iterable`). Pebble throws on a String for even/odd, and `iterable` is false for a String. The pinned fence test is `env.rs:2962` `the_defined_test_is_pebbles_and_it_is_the_only_one_overridden`, which asserts the four are still unknown (`:3005`). It must be rewritten.

**Corpus:**
- Expression: `test-json-true`, `test-json-false`, `null-literal-is-null-test`, likely `coalesce-is-not-null` (**3–4**), plus `test-even`/`odd`/`iterable` (**3**).
- Executor: `valids/task-runif-taskswithstate` (`is [not] empty` in `runIf`, note 10, `baseline.txt:630`) (**1**), and `valids/loop-object` and `valids/loop-object-in-list` (`{{ item.value is json }}` in an `If`, **2, uncertain**). These two are among roadmap #6's "4 unattributed `Loop`" entries, and #2b's B7 also saw them differ on `order` (`oracle-exec/README.md:13736`). So `is json` is a strong candidate cause, not a proven one.

Size: S.

### 1.11 `escape` strategies

Pebble 4.1.2's `EscapeFilter` (decompiled) knows **`html`, `js`, `css`, `url_param` and `json`**. The roadmap misses `json`. Anything else throws "Unknown escaping strategy [x]". It uses `unbescape` 1.1.6.

| Probe | Kestra |
|---|---|
| `js` on `a'b"</script>\n é` | `a\'b\"<\/script>\n \xE9` |
| `js` on `…g&h` | `…g\x26h` |
| `css` on `a b;é` / `a-b_c#d1` | `a\ b\;\E9` / `a-b_c\#d1` |
| `url_param` on `a b&c=é/` / `a+b~c*d!e'f(g)h-_.i` | `a%20b%26c%3D%C3%A9/` / `a%2Bb~c*d!e'f(g)h-_.i` |
| `json` on `a"b\c\n/é\u0001` | `a\"b\\c\n/é\u0001` |
| `html_attr`, `nope` | error "Unknown escaping strategy" |

egret refuses every strategy except `html` (`EW src/runners/filters.rs:1915-1929`). Its error text also differs for an unknown strategy. Corpus: `filter-escape-js-strategy` (**1**). Size: S–M. The tables must be ported from unbescape's level-2 rules exactly. Each strategy gets a corpus batch.

### 1.12 "Lazy chained comparisons" (the premise is wrong)

| Probe | egret | Kestra |
|---|---|---|
| `{{ 1 < 2 < 3 }}` | `true` | **error** (`(1<2) < 3` compares a Boolean with a Number) |
| `{{ 3 < 2 < 1 }}` | `false` | **error** |
| `{{ 1 == 1 == true }}` | `true` | `true` (left-associative) |
| `{{ false and missing.y }}` | `false` | `false` (already lazy) |

- **Pebble has no chained comparison.** It parses `a < b < c` left-associatively, so the outer comparison sees a Boolean and throws (§1.3). egret compiles minijinja's Python-style chain into `kestra_compare_chain` (`compare.rs:432-444`), which evaluates every operand eagerly (`tests::a_chain_evaluates_every_operand`, `:825`).
- The fix is **not** "make the chain lazy". It is "compile a chain left-associatively", so §1.3's strict rule makes `<`-chains throw and `==`-chains compare a Boolean.
- The laziness that really diverges is **`??`'s right operand** (§1.4).
- Corpus: **0**. Size: S, after §1.3, in the same `cmp_rewrite` pass.

### 1.13 BigDecimal arithmetic

| Probe | egret | Kestra |
|---|---|---|
| `fromIon(ion='1.5') + 1` | `2.5` | `2.5` |
| `fromIon(ion='1.50') * 2` | `3.0` | `3.00` (scale kept) |
| `fromIon(ion='1') / 3` | `0.3333333333333333` | `0` (`BigDecimal.divide` keeps the dividend's scale) |
| `fromIon(ion='1.5') / 0` | `Infinity` | error |
| `'1234…890.5' \| number(type='BIGDECIMAL') + 1` | error "tried to use + operator on unsupported types plain object and number" | `123456789012345680000000000001.0` (Kestra loses the precision too) |

- **egret today:** a `fromIon` decimal reaches arithmetic as a float. A `BigNumber` (`kv` decimal, `number(type='BIGDECIMAL')`, an integer past i128) is deliberately **not** arithmetic (`filters.rs:2936`, W9c spec §15, and the W9c plan's amendment that made it a named refusal).
- **Corpus:** `function-fromion-decimal-multiplication-loses-the-scale` and `filter-number-bigdecimal-is-not-a-number-here` (**2**).
- **Size:** M, and part of §1.1's arithmetic seam. Kestra's exact rules (rounding mode and scale per operation, and when a Double operand turns the result into a double) must be **read from `OperatorUtils`** before porting. The table above is the only measured evidence.
- **Risk:** this reverses a recorded W9c decision (refuse rather than compute), so it should be confirmed. The row lists it, so it is treated as in scope.

### 1.14 `{{ task.type }}` on a flowable

- **egret:** `null` for every kind `build_plan` descends into, and for `Executable` (Subflow), `Pause` and `LoopUntil` (`EW src/worker/render.rs:808-858`, `_ => None`). `TaskKind::Executable` already holds a `type_id` (`EW src/executor/plan.rs:394-396`), and the others do not.
- **Kestra:** `RunVariables.of(Task)` writes `{"id", "type": task.getType()}` for every task (`KR RunVariables.java:124-129`). Whether `getType()` gives an alias as written or the canonical FQCN is **(unverified)**.
- **Corpus:** **0**. Only `valids/logs` and `valids/disable-simple` use it, both on a `Log` leaf (`oracle-exec/README.md:3225`).
- **Size:** S. Carry the declared type string on every `PlanTask`.

---

## 2. Adjacent divergences the row does not name

They have the same cause class, sit in the same seams, and some of them hold executor entries.

| Item | Evidence | Entries |
|---|---|---|
| **`equals`, `contains`, `..` operators** have no minijinja syntax, so egret gives a syntax error (probe `a equals 'value 2'`: Kestra `true`) | `rewrite.rs:1673-1680`; `oracle/baseline.txt:72-77` | expression `op-equals`, `op-contains`, `op-contains-boxed-int`, `op-contains-literal`, `op-range` (**5**). Executor `valids/if-in-flowable` (`item.value equals 'value 2'`) and `valids/task-runif-workingdirectory` (`runIf: … contains 'orders'`) (**2, likely**: both are attributed elsewhere today, to "Loop unattributed" and "WorkingDirectory + runIf") |
| **`and`/`or` return a strict boolean** in Pebble, while minijinja returns an operand | `oracle/baseline.txt:33-34` | `coalesce-and`, `coalesce-or` (**2**) |
| Integer division and long overflow wrap | §1.1 | 0 |
| `escape` `json` strategy | §1.11 | 0 |
| `null < 1` is `false` | §1.3 | 0 |
| `even`/`odd`/`iterable` | §1.10 | 3 |

Not renderer work (left out): `working-directory-taskrun-nested` (`{{ workerTaskrun }}` context), `working-directory-invalid-runif` (where a failed `runIf` lands), and `envs`/`globals` (no deployment config).

---

## 3. Totals

**Executor oracle (upper bound, not measured):**
- In the row's own items: `restart-child`, `restart-parent`, `restart-loop`, `restart-parent-loop` (4); `all-flowable`, `disable-flowable` (2); `loopuntil-failed-flowable` (1); `webhook-plaintext` (1); `variables` (1); `errors` (1); `task-runif-taskswithstate` (1); `loop-object`, `loop-object-in-list` (2, uncertain). That is **13**.
- With §2's operators: `if-in-flowable` and `task-runif-workingdirectory` (+2), for **15**.
- The roadmap says "~8–10". A strict estimate is **about 11** (the restart family, the two `taskrun.value` entries, `loopuntil`, `webhook-plaintext`, `variables`, `errors`, `taskswithstate`).
- After the change, `STILL_OVER_ACCEPTED_AT_RUNTIME` loses `all-flowable`, `disable-flowable` and `restart-child`.

**Expression oracle (129 baselined):**
- In the row's items: mixed comparisons 4, `coalesce-nested-absent` 1, BigDecimal 2, `json`/`null` tests 3–4, `escape` js 1. That is **11–12**.
- With §2: operators 5, `and`/`or` 2, even/odd/iterable 3, for up to **22**.

**New coverage needed:** max/min, the ten filters, the escape tables, integer arithmetic, and `null`/`empty`/`map` have **zero** golden entries today. The 99 probes in the scratchpad are a ready seed for new `corpus.json` batches, regenerated with the README's recipe.

**Fixtures that must change with the code:**
- `oracle/name-surface.txt`: 10 filter rows, 2 function rows, 4 test rows, the `escape` row's reason, and the `render`/`renderOnce`/`errorLogs`/`subflow` reasons.
- `functions.rs` `REFUSED` (its test asserts `len() == 6`, `:3586`).
- The tripwires `executor/mod.rs:9708`, `env.rs:2962`/`:3005` and `compare.rs:825`.
- Both `baseline.txt` files and the executor ratchet tuple.

## 4. Size and slicing

The row says M. Measured, it is **two M packages**:

- **#5a "Operators and the render context"** (the semantic core; changes answers of existing flows):
  - strict relational comparison and left-associative chains (§1.3, §1.12);
  - the arithmetic seam: Long semantics, `/0`, BigDecimal (§1.1, §1.13);
  - `??` through a missing chain, with a lazy right side (§1.4);
  - `taskrun.value` omitted when null (§1.2);
  - `task.type` on every kind (§1.14);
  - with Q3: `equals`/`contains`/`..` and strict `and`/`or`.
  - Banks: restart ×4, `taskrun.value` ×2, `loopuntil`, `webhook-plaintext` (8), and +2 with the operators.
- **#5b "Names"** (additive; the only answers that change are refusals becoming answers):
  - the tests `null`/`empty`/`map`/`json`, plus fixing `even`/`odd`/`iterable` (§1.10);
  - `max`/`min` (§1.8);
  - the ten filters (§1.9);
  - the `escape` strategies js/css/url_param/json (§1.11);
  - `render`/`renderOnce` (§1.5);
  - `errorLogs` (§1.6).
  - Banks: `taskswithstate`, `variables`, `errors`, and probably `loop-object` ×2.
- **Out:** `subflow()` (§1.7), see Q1.

#5b has no dependency on #5a except `max`/`min`, which reuse §1.3's comparison. It can come first if quick banks are wanted. #5a first is safer, because #5b's `max`/`min` should be built on the strict comparison.

## 5. Open questions for the user

Only security, scope changes and contradictions with the spec are listed. Everything else has a proposed default in §6.

- **Q1 (scope).** Move `subflow()` out of #5, to #12 "Subflow in full" after #6 (flow `outputs:`). It runs a child execution synchronously inside a render and banks 0. Proposed: yes.
- **Q2 (scope, behaviour change).** Adopt Pebble's integer arithmetic: `7/2` is `3`, `long` overflow wraps, and `/0` fails. This silently changes the result of any stored flow that divides integers. Proposed: yes (parity), and record it in the release notes.
- **Q3 (scope).** Add the §2 items the row does not name: `equals`/`contains`/`..`, strict boolean `and`/`or`, `even`/`odd`/`iterable`, `escape` `json`, and `null < 1`. Proposed: yes. The operators probably bank 2 more executor entries.
- **Q4 (security).** `render()` on run-time data (`render(inputs.x)`, `render(trigger.body)`) lets whoever starts an execution evaluate expressions. Kestra allows it. Proposed: allow, with Kestra's depth cap of 16, an output size cap, `secret()` still confined and `http()` still refused.
- **Q5 (contradicts a recorded ruling).** BigDecimal arithmetic reverses W9c's "arithmetic on an exact value is a named refusal" (W9c plan, amendment of 2026-10-07; `filters.rs:2936`). The row lists it. Confirm the reversal.
- **Q6 (contradicts a recorded ruling).** `coalesce-nested-absent` is baselined as "Deliberate: the alternatives are worse" (`oracle/baseline.txt:79`). Fixing `webhook-plaintext` means undoing that ruling. Proposed: undo it, with the rewriter's safe-path compilation (§1.4 option a).

## 6. Proposed defaults (no decision needed)

- **B1.** `errorLogs()`: worker renders only, as in Kestra. Return the current execution's ERROR lines, each as `{taskId, taskRunId, message, level, timestamp, …}` in Kestra's `LogEntry` shape. Use a short bounded wait (for example 1 s) instead of 5 s, recorded as a divergence.
- **B2.** `numberformat` uses a pinned `en` locale, as egret's `date` filter does when no locale is given. Record the difference from Kestra's JVM-default locale.
- **B3.** `render()` of a non-string returns it as it is. `recursive=false` is `renderOnce`. The depth error text copies Kestra's.
- **B4.** An unknown `escape` strategy gives Kestra's "Unknown escaping strategy [x]".
- **B5.** `task.type` holds the type string as written in the flow. If Kestra canonicalises aliases, follow that, but verify it first.
- **B6.** The multi-line string literal (§1.9) stays a recorded divergence.

## 7. Roadmap premises that are wrong or stale

1. "Lazy chained comparisons": Pebble has no chained comparison, and `a < b < c` **errors** (§1.12). The real laziness gap is `??`'s right side.
2. "`??` on a missing root": a bare missing root works already. The gap is a member or index chain through a missing value (§1.4).
3. "`errorLogs()` needs a task-log store" (`:280`, `:337`, `oracle-exec/baseline.txt:177-179`): the store exists since W11. What is missing is a query and a prefetch (§1.6).
4. "Undefined `taskrun.value`" is a render-**context** fix (`render.rs:866`), not a renderer one (§1.2).
5. "`escape` strategies js/css/url_param": Pebble also has `json` (§1.11).
6. "`subflow`" is not a renderer function in any useful sense (§1.7).
7. "~8–10 banks": the upper bound is 13 (15 with the operators), about 11 strict (§3).
8. The cause table's "Loop unattributed (4)" (`:332`) is probably partly renderer: `loop-object` ×2 use `is json`, and `if-in-flowable` uses `equals`.

## 8. Recommended scope and ordered task list

**Recommended scope:** everything in row #5 except `subflow()`, plus the §2 items (pending Q3), in two PRs, #5a and then #5b.

Decisions: **D1** = Q1 (subflow out), **D2** = Q2 (integer arithmetic), **D3** = Q3 (adjacent items), **D4** = Q4 (render on run-time data), **D5** = Q5 (BigDecimal reversal), **D6** = Q6 (undo the `coalesce-nested-absent` ruling).

**#5a, Operators and the render context**
1. T1. Add the probe batches to `oracle/corpus.json` and regenerate `golden.json`: integer arithmetic, `/0`, BigDecimal, mixed and null comparisons, chains, `??` chains and laziness, `equals`/`contains`/`..`, `and`/`or`. Baseline the new failures (red first).
2. T2. Strict relational comparison in `CmpOp::apply`, with `null` answering `false`. Chains compile left-associatively. Flip the tripwires at `executor/mod.rs:9708` and `compare.rs:825`.
3. T3. The arithmetic seam: a rewriter pass to `kestra_add`/`sub`/`mul`/`div`/`mod` and one numeric tower (Long wrap, integer `/`, `/0` error, Double contagion, BigDecimal per `OperatorUtils`, read first). Remove the `BigNumber` refusal (D5).
4. T4. `??`/`???` through a missing chain, with a lazy right operand (rewriter), and un-baseline `coalesce-nested-absent` (D6).
5. T5. `equals`/`contains`/`..` and strict `and`/`or` (D3).
6. T6. The render context: omit `taskrun.value`, `parentId` and `iteration` when null, and `task.type` for every kind. Shrink `STILL_OVER_ACCEPTED_AT_RUNTIME`.
7. T7. The executor ratchet, both baselines, and the roadmap note. Docs for the changed arithmetic (user-visible).

**#5b, Names**
1. T8. Corpus batches for the tests, `max`/`min`, the ten filters and the five escape strategies.
2. T9. The tests `null` (through the `none` spelling), `empty`, `map`, `json`, and fix `even`/`odd`/`iterable`. Rewrite the fence at `env.rs:2962`.
3. T10. `max`/`min` on T2's comparison.
4. T11. The ten filters.
5. T12. `escape` js/css/url_param/json, with unbescape's tables.
6. T13. `render`/`renderOnce`: context capture, depth 16, size cap (D4). Unit tests, because the standalone oracle cannot carry them. Keep `replay-revision` failing.
7. T14. `errorLogs()`: a logs-repo query, a worker-surface prefetch and a bounded wait (B1).
8. T15. Update `name-surface.txt` and the `REFUSED` table (subflow's owner becomes #12), the ratchet, the roadmap note and the docs (the expression reference in `egret-data/docs`).
