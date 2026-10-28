# W2 expression-language oracle

A standalone Java harness that renders `corpus.json` through Kestra's own
Pebble extension (`io.kestra.core.runners.pebble.Extension`) and commits the
result as `golden.json`. `crates/egret-workflow/tests/oracle.rs` reads
`golden.json` at compile time (`include_str!`) and, starting in Task 3, diffs
it against egret's own renderer. CI reads the committed `golden.json`; it
needs no JVM. Regenerating `golden.json` is a manual, opt-in step (below) —
never run automatically.

## Requirements

- Corretto 25, not the machine default (JDK 21): Kestra 2.x class files are
  version 69 and will not load on JDK 21.
  ```bash
  export JAVA_HOME=/Users/1pro-l4r8/Library/Java/JavaVirtualMachines/amazon-corretto-25.jdk/Contents/Home
  ```
- Maven (`mvn`), any recent version. `pom.xml` pins the target artifact to
  `io.kestra:core:2.0.0-rc13` and `io.pebbletemplates:pebble:4.1.2` (see
  "Kestra checkout vs. Maven artifact" below for why the Pebble version is
  pinned explicitly rather than inherited).
- **A JVM whose default locale has English as its language.** Measured on the
  machine that generates this file: `Locale.getDefault()` is **`en_VN`**
  (language `en`, country `VN`), `ZoneId.systemDefault()` is
  `Asia/Ho_Chi_Minh`, `java.version` is `25.0.3`. Pebble's `EvaluationContext`
  locale is `Locale.getDefault()` unless a template passes one, and
  `AbstractDate.format` hands it straight to
  `DateTimeFormatter.withLocale`, so **every** month name, day name, era name
  and quarter name a `date` filter writes comes from it.

  W2 Task 5 pins this rather than relying on it: every corpus entry whose
  output depends on the locale passes `locale='en'` (or `locale='en_US'`)
  explicitly, the same standing rule the `timeZone` note below states for
  zones. Two things make that not merely tidy:

  - `WeekFields.of(locale)` is resolved from the locale's **country**, and it
    is what `w`, `W`, `Y`, `e` and `c` are printed through. Measured directly:
    `en` and `en_US` are Sunday-first with one minimal day, `fr_FR` is
    Monday-first with four, and **`en_VN` — this machine's own default — is
    Monday-first with one**. So an entry using those letters without an
    explicit locale would answer differently on a machine whose region is not
    VN, even though the month names would look identical.
  - a language-only locale falls back to Sunday/1 regardless of language, so
    `fr` and `en` agree about `w` and `e` while `fr_FR` and `en` do not. An
    earlier reading of two goldens suggested the week fields were not
    locale-sensitive at all; they are, through the country.

- A Unix-family OS (macOS or Linux), not Windows.
  `io.kestra.core.runners.pebble.filters.YamlFilter` configures its Jackson
  `YAMLFactory` with `YAMLGenerator.Feature.USE_PLATFORM_LINE_BREAKS` set to
  `true` (confirmed by decompiling the class's static initializer —
  `javap -p -c` shows an `iconst_1` pushed immediately before that
  `configure` call), so the `filter-yaml` entry's golden value is only
  reproducible on a platform whose line separator is `\n`. Regenerating on
  Windows would silently flip that one value to `\r\n` with no real semantic
  drift behind the byte change — the same class of environment-dependence as
  the JDK and timezone pins in this file.

## Regenerating

```bash
export JAVA_HOME=/Users/1pro-l4r8/Library/Java/JavaVirtualMachines/amazon-corretto-25.jdk/Contents/Home
cd crates/egret-workflow/oracle
mvn -q compile
CP=$(mvn -q dependency:build-classpath -Dmdep.outputFile=/dev/stdout | tail -1)

# Step 2: what the built engine actually registers (filters/functions/tests + counts)
# NOTE: `"$JAVA_HOME/bin/java"`, not bare `java` -- exporting JAVA_HOME does
# not change what `java` resolves to, and the machine default is JDK 21, which
# fails with `UnsupportedClassVersionError` on Kestra 2.x's version-69 classes.
"$JAVA_HOME/bin/java" -cp "target/classes:$CP" host.Oracle dump

# Step 4: render corpus.json -> golden.json
"$JAVA_HOME/bin/java" -cp "target/classes:$CP" host.Oracle generate corpus.json golden.json
```

## The corpus schema, and `compare`

A corpus entry is `{"id", "expr", "context"}` plus one optional field:

```json
{"id": "function-secret", "expr": "{{ secret(key='x') }}", "context": {},
 "compare": "error-class"}
```

`"compare": "error-class"` says: this entry's golden value is a **Java
exception string**, which no Rust port can ever reproduce byte for byte, so
`tests/oracle.rs` compares only that egret **also fails** — not what it says.
The Java message stays in `golden.json` as documentation of what Kestra
actually says. `Oracle.java` copies the field through verbatim.

Two properties keep this from becoming a silent weakening of the oracle, and
both are deliberate:

- It is **marked in the corpus**, never inferred at comparison time from "the
  golden happens to look like an error". A comparison that decides its own
  strictness from the data it is comparing is not an instrument.
- `tests/oracle.rs` **prints the marked ids with a count on every run**, pass
  or fail, alongside the eight shape-compared non-deterministic ones.

**The set is committed as `error-class.txt`** and asserted to equal the
corpus's markers exactly, because error-class comparison is a *free pass*: such
an entry matches on any failure at all, so it is born matching and cannot fail
**for the reason it was written** — a refusal that becomes a different refusal
is invisible. It is not literally unfailable, and an earlier version of this
sentence ("never enters `baseline.txt`, and can never fail") was wrong about
that: **nineteen ids are in this file AND in `baseline.txt`** (twenty-five
until package #5a's Task A3 banked six, thirty-eight
until package #5a's Task A2 banked thirteen, eighteen until
package #5a's Task A1 added twenty, thirty-one until
W2a banked fourteen of them and added one back), because such an
entry still fails when egret *succeeds* where Kestra threw. (This said "five"
until W2 Task 8, and it was already stale at six -- Task 7's fix round 3 added
`function-fromion-annotation-of-a-list` without moving the number. Counted out
of the two files, not carried forward.) A task that could not reproduce a filter
could mark its entry error-class and turn the oracle green on work it did not
do. Keeping the set in its own short file makes adding one a one-line,
reviewable diff instead of a field edit buried in a 291-entry JSON file — the
same discipline `baseline.txt` applies to mismatches.

**And `error-class-pairs.txt` says, per id, what CAN fail.** `error-class.txt`
makes adding a free pass visible; it does not make the entry answerable —
nothing notices if the refusal behind one turns into a success or into a
different refusal for a different reason. Until W2 Task 4's last round that was
closed by a convention ("each is paired with a unit test asserting egret's own
message") written in prose. Tasks 6-8 add more entries — **Task 8's entire
deliverable is refusals** — and a convention nobody checks is not a constraint.
So the pairing is committed and asserted by
`every_error_class_entry_is_paired_with_something_that_can_fail`:

| justification | what is checked |
|---|---|
| `test:<name>` | `<name>` is a `#[test] fn` that **exists** in `src/runners/`, found by scanning the committed source with `include_str!`. The scanned list is a `const` and the directory is not, so `every_runners_module_is_scanned_for_declared_tests` asserts they are the same set — it was three files while this sentence said the directory, which would have rejected a real `javafmt` test as nonexistent |
| `baselined` | the id really is in `baseline.txt`, so it is not born matching at all and the ratchet already holds it |
| `unported:<task>` | nothing is ported behind it; the **set** of these must equal `UNPORTED_ERROR_CLASS` in `tests/oracle.rs`, so a ninth cannot be added without editing a test |

The two files must name exactly the same ids, in both directions. An entry
added without a pair fails **by name**, the way a stale baseline line does.
Verified by deliberate breakage before committing: an unpaired new entry, a
renamed paired test, a false `baselined` claim and a quietly-added
`unported:` all fail, each naming the id.

**328** entries carry it today (302 before package #5b's Task B1 added
twenty-six, 271 before package #5a's Task A1 added
thirty-one, 251 after W2b's fix round, 245 before it, 240
before W2b, 153 before W2 Task 8, 182 before its fix round), counted out of
`golden.json` rather than added up (39 of them W2 Task 7's, listed at the end of `error-class.txt`): the four `*-mixed` comparisons (Pebble throws on a String/Number mix), the
eight registered-but-null-backed functions, `raw-multiline`, the three `isin-*`
entries where an absent variable raises, **twelve from W2 Task 4** — four
wrong-typed inputs, five exceptions its first fix round found, and three more
from its third (`indent`'s own `MAX_AMOUNT`, and `number`'s two `type`-argument
casts) — **fourteen from W2 Task 5** (eight of them the Java pattern compiler
refusing a pattern), **nine more from Task 5's fix round**, which its probe
sweep earned, and **five from its second round**, from the `existingFormat`
parse sweep, and **eight from its third**, every one of which was shown to
fail against the pre-fix implementation before it was written down, and
**fourteen from W2 Task 6** (four `jq` argument and query failures, four
regex-argument ones, three replacement-string ones and three `group`-argument
ones), and **nine from its fix round** (six patterns Java itself rejects and
the `regex` crate accepts, and the three withheld jq natives). **Every one of them is separately backed by a unit test in
`runners::filters` asserting egret's own message**, named alongside it in
`error-class.txt` and mechanically checked against `error-class-pairs.txt`, so
what carries them is the test and not the mode.

That is a lot, and it is a deliberate trade: each records an exception Kestra
actually throws, which is worth having in `golden.json`, at the cost of an
entry the ratchet cannot fail. **The ratio was to be looked at before Task 8**,
whose deliverable was refusals; Task 8 added 29, which is 17% of the file for
7% of the corpus, and every one of them is `test:`-paired — the pairing file
now has **no `unported:` line left in it**, and `UNPORTED_ERROR_CLASS` in
`tests/oracle.rs` is empty. Note that marking an entry error-class does **not**
make it pass: 16 of the 328 are *also* in `baseline.txt` (12 before package
#5b's Task B1 added four, 17 before package
#5a's Task A6, 19 before its Task A4, 25 before its Task A3, 38 before its
Task A2; re-counted from the two files 2026-10-29, after Task A5, which moved
none of them, and again after Task A6, which banked five), because egret
*succeeds* where Kestra throws — a real divergence, and it stays visible.

## `name-surface.txt` — which names each side has at all

W2 Task 8. Four times this package found a name minijinja binds that Kestra
also binds, resolving to minijinja's answer instead of erroring as unknown
(`string`, `indent`, `range`, `replace`). `name-surface.txt` is the fixture for
the other half of that question: every filter, function and test name any of
the three sources binds — the engine, egret, and minijinja's own defaults —
with what each side does about it and a reason on every row where they differ.
`runners::env::the_built_in_surface_matches_the_engines` asserts it by
**probing** `render_str`, in both directions. See "The name surface, and what
W2 did not compare" at the end of this file for what it closed and what it
deliberately left open.

## `baseline.txt` — the ratchet

`tests/oracle.rs` is not a pass/fail test while Tasks 4-8 are outstanding; it
is a ratchet. `oracle/baseline.txt` lists the ids that do not match yet: 76 of
138 at W2 Task 3, 57 of 241 after Task 4, 59 of 268 after its fix round,
60 of 291 after fix round 3, 59 of 426 after W2 Task 5, 62 of 457 after that
task's fix round, 62 of 486 after its second, 66 of 513 after its third,
76 of 632 after W2 Task 6 -- which BANKED seven (its whole batch of
filters) and RECORDED seventeen, every one of them a place the two engines
answer differently rather than a filter that is not ported -- and 78 of
673 after that task's fix round, 82 of 693 after its second, 86 of 706 after its
third, 70 of 855 after W2 Task 7 -- which BANKED eighteen (its whole batch of
pure functions) and RECORDED two, both places the two YAML scalar resolvers
disagree -- 80 of 892 after that task's fix round, which recorded ten
divergences that had no entry at all, 87 of 931 after its second, which fixed
thirteen regressions and recorded nine more of the same kind, and **109 of 966**
after its third, which REVERTED one of the second's fixes and recorded what
that costs, and **105 of 1047** after W2 Task 8 -- which BANKED seven (every
function the brief called service-backed that is not) and RECORDED three -- and
**117 of 1141** after that task's fix round, which fixed 110 measured
divergences that had NO corpus entry at all and recorded twelve that it
deliberately did not fix, and **130 of 1203** after its second round, and **128 of 1234** after its third, which
BANKED two and recorded fifteen while closing two regressions the first round
had introduced -- **that number said 131 in two places until W2b counted it out
of the file, which is what C8 asks for and what nobody had done here** -- and
**131 of 1278** after W2b -- which added 44 entries for
Pebble's `null` literal (a class the corpus had NO entry for at all), BANKED
three and RECORDED six -- and **139 of 1288** after its fix rounds, which
RECORDED ten and BANKED two: `null` in a BOOLEAN context, where translating the
literal removed the accidental reason egret used to fail for and left
minijinja's own semantics showing, and where `is defined` -- the one of the five
a crate can reach -- is closed by overriding a single test -- and **130 of 1336**
after W2a, which added 48, BANKED **fourteen** (every pair W2 Task 8 had recorded
as unfixable) and RECORDED four -- and **129 of 1336** after W9c Task 4's fix
round 1 (2026-10-08), which BANKED one, `filter-number-bigdecimal-comparison`
(see "The same hole" below), and changed no other answer -- and **227 of
1493** after package #5a's Task A1 (2026-10-28), which added 157 probe
entries and RECORDED the 98 that fail under the `#5a red` heading, each group
named with the task that banks it (see "What package #5a's Task A1 batches
measured" below) -- and **208 of 1493** after Task A2 (2026-10-28), which
BANKED nineteen: W2 Task 3's four `*-mixed` comparisons and A1's fifteen
comparison and chain entries (spec D1: `<`, `<=`, `>`, `>=` are Kestra's,
and a chain compiles left-associatively) -- and **173 of 1493** after Task A3
(2026-10-28), which BANKED A1's thirty-six arithmetic entries (spec D2: `+ -
* / %` are Pebble's `OperatorUtils`, Long and Double, string and list
operands) and RECORDED one, `coalesce-lazy-right-division`, which had passed
only because minijinja's `1 / 0` answered `inf`; it now fails for Task A5's
reason (an eager `??` right side) and is baselined in A5's group -- and
**152 of 1493** after Task A4 (2026-10-29), which BANKED twenty-one (spec D3,
Q5 reversing the W9c refusal: a `BigDecimal` operand is
`BigDecimal.valueOf(doubleValue())` on both sides, computed at
`DECIMAL128` with Java's preferred scale): A1's eight BigDecimal entries,
`filter-number-bigdecimal-is-not-a-number-here`, and the twelve
`function-fromion-decimal-*` entries W2 Task 7 recorded, because an Ion
decimal is a `java.math.BigDecimal` `BigNumber` again -- and **140 of 1493**
after Task A5 (2026-10-29), which BANKED twelve (spec D6, Q6 reversing the
"deliberate" `coalesce-nested-absent` ruling: a path on the left of
`??`/`???` compiles to `kestra_path(..)` under a lazy inline `if`): A1's
eleven coalesce entries (`coalesce-lazy-right-division` among them) and
`coalesce-nested-absent`. `coalesce-non-path-left-missing-right` stays
baselined, the recorded D6 divergence (a call on the left keeps the eager
form) -- and **107 of 1493** after Task A6 (2026-10-29), which BANKED
thirty-three (spec D5, Q3: `equals` is `==`, `contains` is Pebble's
`ContainsExpression`, `..` is Kestra's `range`, and each `and`/`or` operand
is coerced to a strict boolean, `runners::operators`): twenty-five of A1's
twenty-six operator entries, `op-equals`, `op-contains`,
`op-contains-literal`, `op-range`, `coalesce-and`, `coalesce-or`, and W2b's
`null-literal-in-an-and-operand` and `null-literal-in-an-or-operand` (a null
`and`/`or` operand now fails with Pebble's own text). Two stay, each
re-attributed with its measured cause: `op-contains-boxed-int` (spec D2's
Integer/Long divergence) and `logic-and-returns-boolean` (minijinja's `~`
writes a boolean as `True`) -- and **329 of 1753** after package #5b's Task B1
(2026-10-29), which added 260 probe entries (tests, `max`/`min`, the ten
filters, `escape` strategies) and RECORDED the 222 that fail under the `#5b red`
heading, each group named with the task that banks it (see "What package #5b's
Task B1 batches measured" below).
The test fails in **both** directions:

- an id **not** in the baseline mismatches — a regression;
- an id **in** the baseline now matches — progress that must be banked by
  deleting that line, in the same commit that earned it.

The second direction is what makes it a ratchet rather than a suppression
list: a task cannot make progress without shrinking the file, and cannot claim
progress it did not make. A baseline id that is not in `golden.json` at all
fails too, so a renamed corpus entry cannot leave a dead line behind. The
current mismatch count prints on every run — but **libtest captures stdout on
a passing test**, so run it as

```bash
cargo test -p egret-workflow --test oracle -- --nocapture
```

**Run the whole target, not one test.** `--test oracle` carries **ten** tests,
and **nine** of them guard the differential test rather than take part in it.
Filtering to `-- render_corpus_matches_golden` — the natural thing to type while
iterating — skips every one, so the ratchet will happily pass against a corpus
that has drifted out of sync with the golden file it is being checked against,
against a free pass nothing can fail, or with three of its own four failure
arms deleted.

| guard | what it holds |
|---|---|
| `corpus_and_golden_describe_the_same_expressions` | the corpus and the golden file describe the same expressions |
| `error_class_entries_are_exactly_the_committed_allowlist` | `error-class.txt` is exactly the corpus's `compare: "error-class"` set |
| `every_error_class_entry_is_paired_with_something_that_can_fail` | every free pass in `error-class.txt` has a justification in `error-class-pairs.txt` that is checkable |
| `every_runners_module_is_scanned_for_declared_tests` | the `const` list of scanned files equals the `src/runners/` directory — **the table at the top of this section depends on it**, and a `test:<name>` justification is otherwise checked against a partial scan |
| `a_panicking_renderer_is_a_mismatch_in_every_mode` | a panicking render is a mismatch in `entry_matches`, in every comparison mode |
| `a_panic_fails_even_when_the_ratchet_is_satisfied` | … and it fails the RUN even when every panicking id is baselined |
| `a_stale_baseline_line_fails_and_names_it` | `assemble_failures`' `stale` arm fires, and names the ids |
| `a_regression_outside_the_baseline_fails_and_carries_its_detail` | its `regressions` arm fires, names the ids, and carries their expected/actual |
| `progress_that_is_not_banked_fails_and_names_it` | its `banked` arm fires, and names the lines to delete |

**This list said "six", and named four.** The two it omitted were
`every_runners_module_is_scanned_for_declared_tests` — which the table at :129
of this file explicitly relies on — and the panic guard. A maintainer trusting
the count believed two guards did not exist.

The last three arrived at the merge gate, and the reason they were missing is
worth keeping: a green run sits exactly **at** baseline, so `stale`,
`regressions` and `banked` are empty on every passing run, and nothing else in
the file constructed a `Findings`. Measured — neutering all three arms
(`if false && !f.stale.is_empty()`, and likewise) left the target at **7
passed, 0 failed**. Three quarters of the instrument this whole package's
authority rests on was dead code to its own test suite. Each arm was then
neutered one at a time, and exactly its own test died.

`--nocapture` is what actually shows the count and the exclusion lists. Without `--nocapture`, a
green run tells you nothing about how much of the corpus is still baselined.

Both directions were verified by deliberate breakage before this was
committed: breaking `kestra_null_coalesce` produced `REGRESSION: 6`, and
registering a working `json` test without editing the baseline produced
`PROGRESS NOT BANKED: 2`.

**EIGHT ids always differ on every regeneration, and two more differ
sometimes** — this said nine, then ten, and both were wrong the same way:
three regenerations gave 8, 10 and 9. The eight always move; the two
`fromion` ones are Jackson serialiser artifacts that alternate
*independently*, so any of 8, 9 or 10 is a normal diff. The eight
non-deterministic ones in `tests/oracle.rs`'s `NON_DETERMINISTIC`, plus
`function-fromion-annotation-in-a-list` **and
`function-fromion-annotation-of-a-list`**, whose goldens are a **Jackson
serialiser artifact that is not stable between JVM runs** — on the same jar and the same
corpus, five runs gave `Document nesting depth (1001) exceeds the maximum`
three times and `Direct self-reference leading to cycle` twice. It is
baselined rather than shape-checked: egret answers where Kestra throws, so the
ratchet holds it as an ordinary recorded divergence and only the golden text
moves. The tenth is `error-class`, so nothing can fail for it at all, which is
why it took a regeneration diff rather than a red run to find. W2 Task 7's fix round 2 deleted the entry for that instability and its
third round put it back — `error-class` exists precisely for an unstable Java
message, and what stopped it being used here is that egret no longer fails,
which makes `baseline.txt` the honest home rather than deletion.

**A regenerated `golden.json` MUST be reviewed as a `git diff` before
committing.** A changed value means either upstream Kestra changed (the
pinned `2.0.0-rc13` artifact was bumped) or `corpus.json` changed — a human
has to say which; never auto-commit a regenerated golden file.

## What the built engine actually carries (Task 1 Step 2)

The spec (read off `Extension.java` in the sibling `kestra` checkout, pinned
at commit `96893b589`) says **35 filters / 37 functions / 1 test**. The
*built* engine — the actual authority, per this task's brief — carries:

| | spec (source reading) | built engine (`host.Oracle dump`) |
|---|---:|---:|
| filters | 35 | **35** — matches |
| tests | 1 | **1** — matches |
| functions | 37 | **34** — differs, see below |

### Functions: 34, not 37 — and not just "4 fewer for the obvious reason"

`RenderFunction` and `RenderOnceFunction` are `@Inject @Nullable` fields on
`Extension`; `ErrorLogsFunction` and `SubflowFunction` are the same. A
standalone `new Extension()` never runs through Micronaut, so all four fields
stay `null`, and `Extension.getFunctions()` guards exactly these four with
`if (x != null)` before `put`-ing them — so none of the four make it into the
map at all. 37 total `functions.put(...)` call sites in source, 4 guarded,
gives 33 unconditional registrations from what's in the checkout.

But the built engine reports **34**, not 33 — one more than source-reading
predicts. The extra name is **`range`**, backed by a class
(`io.kestra.core.runners.pebble.functions.BoundedRangeFunction`, wrapping
Pebble's own built-in `RangeFunction` with a 1,000,000-element cap via a
`RenderLimits.ensureAtMost` guard) that **does not exist anywhere in the
pinned `kestra` checkout** at `96893b589` — confirmed by `find` turning up
nothing, and by decompiling `Extension.class` straight out of the resolved
`core-2.0.0-rc13.jar` (`javap -p -c`), which shows an unconditional
`functions.put("range", new BoundedRangeFunction())` that has no counterpart
in `Extension.java` as checked out locally.

**This means the Maven Central artifact `io.kestra:core:2.0.0-rc13` was not
built from the commit this repo has pinned as its Kestra source reference.**
33 (checkout-visible, unconditional) + 1 (`range`, artifact-only) = 34, which
is exactly what the built engine reports — so the arithmetic reconciles, but
only once `range` is accepted as real. **The built artifact is what actually
executes in this harness and in every later task's port, so it is the
authority: `range` is in scope, at 34 total functions, and Task 7/8's
function-count checkpoints should expect 34, not 37 and not 33.** Any task
reading `Extension.java` in the checkout for a function's *semantics* should
know that source can lag the artifact — `BoundedRangeFunction`'s behavior
here was read from decompiled bytecode, not source, for exactly that reason.

The 34 registered names, sorted:

```
dayOfMonth, dayOfWeek, decrypt, encrypt, env, fetchContext, fileExists,
fileSize, fileURI, fromIon, fromJson, hourOfDay, http, id, isDayWeekInMonth,
isFileEmpty, isLastWorkingDay, isPublicHoliday, isWeekend, ksuid, kv,
loopOutputs, monthOfYear, nanoId, now, printContext, randomInt, randomPort,
range, read, secret, tasksWithState, uuid, yaml
```

(`printContext` is `FetchContextFunction.ALIAS` — same class, same behavior,
as `fetchContext` — registered under two keys, which is why 33 *classes*
still produce 34 *names* even before `range` is added.)

### 8 of those 34 are registered but structurally unusable standalone

`secret`, `kv`, `read`, `fileURI`, `fileSize`, `isFileEmpty`, `fileExists`,
`http` are backed by `@Inject`-only fields on `Extension` (no `@Nullable`,
unlike the four above) — `SecretFunction secretFunction`, `KvFunction
kvFunction`, etc. A standalone `new Extension()` leaves these fields `null`
too, but `getFunctions()` `put`s their `NAME` keys **unconditionally** — so
the map ends up with 8 keys whose *value* is a `null` `Function` reference.

**Surprising, and worth knowing before Task 8 relies on it**: this does
*not* surface as a `NullPointerException` at call time. Empirically (see
`golden.json`), calling any of these eight —
e.g. `{{ secret(key='my-key') }}` — fails at template-compile time with

```
io.pebbletemplates.pebble.error.PebbleException: Function or Macro [secret] does not exist.
```

i.e. Pebble's parser treats a `null`-valued map entry exactly as if the key
were never `put` at all. This is a clean, useful fact for Task 8 ("ported as
explicit refusals"): the port doesn't need to reproduce an NPE, just a "not
registered" style error — which is what the real engine already does.

## Deliberate deviations from the task brief's literal harness code

### `strictVariables(true)`, not `strictVariables(false)`

The brief's four-line snippet (proven buildable in an earlier spike) uses
`.strictVariables(false)`. **This harness uses `.strictVariables(true)`
instead** — the same setting `PebbleEngineFactory.newPebbleEngineBuilder()`
uses in real, Micronaut-driven Kestra (read from
`core/src/main/java/io/kestra/core/runners/VariableRenderer.java` and
`PebbleEngineFactory.java` in the checkout). This was checked empirically,
not assumed — see `/tmp` throwaway harness output captured while building
this task (`StrictTest`, not committed):

| expression | context | `strictVariables(false)` | `strictVariables(true)` (real Kestra) |
|---|---|---|---|
| `{{ nothere ??? 'fallback' }}` | `{}` (key absent) | `null` (→ `""`) | **`'fallback'`** |
| `{{ missing ??? 'fallback' }}` | `{"missing": null}` | `null` | `null` |
| `{{ nothere }}` (no operator) | `{}` | `null` (→ `""`) | **throws** `RootAttributeNotFoundException` |

`UndefinedCoalescingExpression` (`???`) only catches
`AttributeNotFoundException` — unlike `NullCoalescingExpression` (`??`),
which *also* checks `result != null` directly. Under lenient mode
(`strictVariables(false)`), Pebble never throws for an absent root variable
in the first place — it silently resolves to `null` — so `???`'s catch block
never fires and it can **never** fall back to its right-hand side, for
*either* the null case or the absent case. That erases exactly the
distinction `corpus.json`'s `undef-coalesce-absent` entry exists to pin (the
brief calls the `??`/`???` pair "the ones the port is most likely to get
wrong" for good reason). Only `strictVariables(true)` reproduces `???`'s real
behavior: fall back on *absent*, but not on *explicit null*. Using the
brief's literal `false` would have silently baked a wrong golden value for
the single case the corpus was specifically designed to catch.

`newLineTrimming(false)`, `autoEscaping(false)`, and
`allowOverrideCoreOperators(true)` are also added (not in the brief's
snippet), copied from `PebbleEngineFactory.newPebbleEngineBuilder()`.
`allowOverrideCoreOperators(true)` is not cosmetic: without it, Kestra's
custom `>`, `>=`, `<`, `<=`, `??`, `???`, `isIn` operators (registered by
`Extension.getBinaryOperators()`) would collide with Pebble's built-in
operators of the same symbols, defeating exactly the operator corpus this
task was told to prioritize.

### `io.pebbletemplates:pebble` needs an explicit compile-scope dependency

`io.kestra:core`'s own POM declares Pebble as `scope=runtime` (Kestra's
*compiled* API never references Pebble types directly — only its internal
implementation does). This harness calls `PebbleEngine.Builder` directly, so
it needs Pebble at compile scope, added explicitly in `pom.xml` with a pinned
version (`4.1.2`, what `2.0.0-rc13` itself resolves to) since this harness
has no parent BOM to inherit a version from.

## Surprising golden values (read before trusting `golden.json`)

- **`filter-jq` returns a list, not a scalar.** `{{ json | jq('.a.b') }}`
  with `json = {"a": {"b": 42}}` renders `'[42]'` — `jq`'s API always returns
  a `List` of matches even for a single-value query, and that list, not its
  sole element, is what gets rendered. This was already known from an
  earlier spike; re-confirmed here.
- **`coalesce-or` and `coalesce-and`'s golden values will not be reproduced
  by a correctly-shaped Task 2 rewrite once Task 3 wires up rendering, for a
  reason that has nothing to do with the rewrite's boundary logic being
  wrong.** Pebble's `or`/`and` coerce their result to a strict boolean —
  confirmed directly: `{{ a or b ?? c }}` (`a=false, b=null, c="c-value"`)
  renders `true`, not the operand `"c-value"` that Python/Jinja-style
  short-circuit semantics would return. minijinja's own `or`/`and` are
  exactly that Python/Jinja style: `or` returns whichever *operand* is
  truthy, `and` returns the FIRST falsy operand or the last one, never a
  coerced boolean. This rewrite deliberately does not touch `or`/`and`
  syntax (same "leave it alone, Task 3's problem" treatment as the
  comparison operators — see `gt-mixed` below) — it only recognises them as
  precedence *boundaries* so a coalesce search does not cross them (see
  `rewrite.rs`'s `resolve_pieces` doc). Neither entry can be used, as-is, to
  verify Task 2's rewrite against Pebble's golden value end-to-end — that
  additionally needs whoever ports `or`/`and` semantics (likely via a
  `kestra_or`/`kestra_and` function wrapper, mirroring how `??`/`???` are
  already handled).

  **The two entries are NOT equally useful for the one thing Task 2 does
  own — did the rewrite put the boundary in the right place — and an
  earlier version of this note wrongly claimed they were.**
  `coalesce-and` genuinely discriminates a correctly-shaped rewrite from a
  wrongly-shaped one when both are rendered through minijinja's actual
  `and` — confirmed, not reasoned about, with a real minijinja
  `Environment` and a `kestra_null_coalesce` matching
  `NullCoalescingExpression`'s actual semantics: with `a=1, b=2, c=null,
  d="D"`, the correctly-shaped rewrite renders `"D"` and the wrongly-shaped
  one renders `"1"`, genuinely different (Pebble's own golden for that
  context is `"true"`, matching neither — the boolean-coercion gap above —
  but the two REWRITES still separate from each other, which is what this
  entry is for).

  `coalesce-or` **cannot** discriminate its two rewrites this way, for ANY
  context — not an unlucky choice, algebraically unachievable. With
  minijinja's operand-returning `or`: if `a` is truthy, `(a or b) ?? c`
  reduces to `a` and `a or (b ?? c)` also reduces to `a` (both short-circuit
  on `a` before `b`/`c` matter); if `a` is falsy, both reduce to `b ?? c`
  (the `or` in the wrong shape's `(a or b) ?? c` returns `b`, non-null,
  since Pebble/Kestra's `??` only cares about null-ness and a boolean
  `false` is not null — so the wrong shape reaches `b` too). Confirmed by
  rendering both shapes through the same real minijinja `Environment` above
  across four contexts (`a` truthy, `a` falsy-but-non-null, `a` null, and
  a mix with `b` defined) — identical output in all four. `coalesce-or`
  keeps its value as a record of Pebble's actual grouping/golden value; it
  just cannot be used to catch a wrong-grouping REWRITE bug by rendering,
  the same limitation `coalesce-is-not-null` (below) has for an unrelated
  reason.

  **(Superseded 2026-10-29, package #5a Task A6, spec D5 and Q3.)** The
  `kestra_or`/`kestra_and` wrapper this note asked for was built as
  `kestra_bool` (`runners::operators`): every `and`/`or` operand is coerced
  as Pebble's `TypeUtils.compatibleCast(x, Boolean)` does, so the result is a
  strict boolean, and **both entries are banked** -- their golden values are
  reproduced, not just their grouping.
- **`coalesce-is-not-null` can never be rendered once Task 3 wires up
  minijinja, for a naming reason unrelated to this rewrite.** minijinja has
  no test named `null` — confirmed directly: `{{ a is null }}` PARSES (the
  test-invocation grammar accepts any identifier in test-name position) but
  fails at render with `unknown test: test null is unknown`; minijinja's
  equivalent test is spelled `none` (`{{ a is none }}` renders correctly).
  This entry keeps real Pebble's actual test name (`null`) because the
  corpus records real Kestra source, not minijinja's own vocabulary —
  translating test/filter *names* between the two engines is Task 3's job,
  not this rewrite's (which only ever reshapes operator *syntax*, never
  touches names). So, like `coalesce-and`/`coalesce-or` above, this entry
  cannot currently be used for an end-to-end render comparison — a
  correctly-shaped AND a wrongly-shaped rewrite of it both fail with the
  identical `unknown test: null` error, discriminating nothing at that
  layer — though it remains a valid record of what real Pebble does with
  this Kestra source. `coalesce-is-defined` has no such problem: `defined`
  is a real, working minijinja test name, and it genuinely discriminates
  (correct rewrite → `true`; the grouping this entry was added to catch a
  regression of (a coalesce search that does not recognise `is` as a
  boundary at all) → `A`, the left operand's own value).
- **minijinja renders booleans as `True`/`False`; Pebble renders `true`/
  `false`.** **FIXED in W2 Task 3's fix round** — this is
  `JsonWriter.write(Object)`'s behaviour, so it belongs to the environment's
  value formatter (`runners::env::json_writer_formatter`), and no filter or
  function task could have reached it: `{{ list }}` names neither. Measured
  before and after: it moved the oracle from 102 mismatches to 87. The note
  below is kept because the *inventory* is still useful.

  Affects exactly **27** corpus entries whose golden value is a
  bare `"true"`/`"false"` (counted directly from `golden.json`, not
  estimated): `isin`, the full `gt-*`/`lt-*`/`lte-*`/`gte-*` family (12
  entries, including the three equality-boundary ones), `coalesce-and`,
  `coalesce-or`, `coalesce-eq`, `coalesce-is-defined`,
  `coalesce-is-not-null`, `is-defined-then-coalesce`, `filter-startswith`,
  `filter-endswith`, `filter-regexmatch`, `function-ispublicholiday`,
  `function-isdayweekinmonth`, `function-isweekend`,
  `function-islastworkingday`, and both `test-json-*` entries.
  Of those 27, exactly **15** were failing on casing *alone* (the other 12
  also fail on a missing filter or on the mixed-comparison throw), so the
  formatter fix recovered 15. `JsonWriter` also compact-encodes `Map`s,
  `Collection`s and arrays where minijinja writes `[1, 2]` / `{"a": 1}` with
  spaces; that is in the same formatter, and it is why every container-valued
  golden here has no space after `,` or `:`.
- **`gt-mixed` throws, it doesn't coerce.** `{{ '2' > 10 }}` (`GreaterThanExpression`)
  only special-cases the case where *both* sides are `String`
  (`"b" > "a"` does a lexicographic compare); a `String`/`Number` mix falls
  through to Pebble's default `OperatorUtils.gt`, which throws
  `PebbleException: Could not perform greater than comparison`. Captured in
  `golden.json` as `{"error": "..."}`, per this task's requirement that a
  faithful port reproduce failures too. A regex/string-based rewriter that
  tries to be clever about numeric coercion here would diverge from Kestra.
- **`date`/`dateAdd` are silently machine-timezone-dependent unless
  `timeZone` is passed explicitly.** `AbstractDate.format()` defaults to
  `ZoneId.systemDefault()` when no `timeZone` argument is given. On this
  machine (`Asia/Ho_Chi_Minh`, `+07:00`) the first golden-file generation
  produced `2024-01-16T17:30:00.000000+07:00` for `dateAdd`; the *same*
  expression on a UTC CI machine would produce
  `2024-01-16T10:30:00.000000Z` — a different string for the same logical
  instant, which would make `golden.json` non-reproducible across
  environments. **Fixed** by adding an explicit `timeZone='UTC'` argument to
  both `filter-date` and `filter-dateadd` in `corpus.json`; every other
  date/timestamp filter and function either works from an explicit-offset
  input (`timestamp*` filters, whose output is an epoch value: instant-based,
  zone-invariant by construction) or never re-zones at all
  (`dayOfWeek`/`hourOfDay`/`isWeekend`/etc. parse the date/datetime string
  literally, with no default-zone step). **Any future corpus entry that
  formats a date/datetime without an explicit `timeZone` argument will be
  non-reproducible across machines — this is not just a one-off fix, it's a
  standing rule for this corpus.**
- **`undef-coalesce-absent` = `'fallback'`, `undef-coalesce-null` = `''`.**
  This is the intended, correct distinction (see the `strictVariables`
  section above) — `???` treats "key absent from context" and "key present
  with value `null`" differently; `??` treats them the same. Get this pair
  right in the port or the whole reason for including it is lost.
- **The 8 null-valued functions fail with "Function or Macro [x] does not
  exist", not a `NullPointerException`.** See above.
- **`null` renders as the empty string `""`, never the literal text
  `"null"`.** `render()` mirrors `VariableRenderer.render(String, Map)`:
  a `null` `JsonWriter.output()` becomes `""`. (Some filters that receive an
  *input* of `null`, e.g. `toJson`/`yaml`/`toIon`, choose to emit the
  four-character string `"null"` themselves — that's the filter's own
  choice on a null *input*, unrelated to this top-level render-result rule.)

## Entries that can never be reproduced by exact-string equality

**`now`, `uuid`, `id`, `ksuid`, `nanoId`, `randomInt`, `randomPort`, and
`encrypt` are all non-deterministic** — wall-clock time, `SecureRandom`,
`Math.random()`, or (for `encrypt`) a fresh random IV on every call. Their
corpus entries are included because the brief requires "at minimum one entry
per registered filter and function," and their *shape* is worth pinning
(`uuid` is a UUIDv7, `ksuid`/`nanoId` are fixed-length base62-ish strings,
`randomInt(lower=1, upper=10)` is an integer in `[1, 10)`, `encrypt` is valid
base64), but **no implementation — Kestra's or a port's — can ever reproduce
these exact committed strings on a second run.** `tests/oracle.rs`'s strict
string-equality comparison will report these 8 ids as permanent, expected
mismatches once Task 3 wires a real comparison. **A future task must special-
case these ids** (e.g. assert a shape/regex instead of exact equality, or
exclude them from the strict diff with a comment referencing this section)
rather than trying to make the oracle "pass" them by exact match — flagged
here per this task's brief: capture every surprise, don't paper over it.

`function-decrypt`'s corpus entry avoids this problem by hardcoding a
ciphertext generated once (via a throwaway harness against the same fixed
key used in `function-encrypt`'s entry) rather than depending on that other
entry's non-deterministic output within the same run.

**Task 3 resolved this**: `tests/oracle.rs` now carries a
`const NON_DETERMINISTIC: &[&str]` naming exactly those eight ids, with a
per-id comment. They are compared for *shape* (length, numeric range, error
vs. success), never for value, and the test prints the eight names on every
run — pass or fail — so the exclusion can never be silent. **Every other entry
is compared for exact byte equality**: no trimming, no normalisation.

## Raw tags (W2 Task 3)

`{% raw %}` is **not** a Pebble tag — Pebble 4.1.2 throws
`ParserException: Unexpected tag name "raw"` — and Kestra registers no custom
tags (`Extension.getTokenParsers()` returns `null`). Kestra supports it one
layer *above* Pebble, in `VariableRenderer`, which substitutes raw regions out
of the string before compiling and puts them back after evaluating. Both this
harness and `crates/egret-workflow/src/runners/env.rs` copy that structure and
that exact regex.

Five entries pin it. The first three are the spellings Kestra's own
`PebbleVariableRendererTest.raw` asserts; every body is `{{ var }}` with `var`
BOUND in the context, so an implementation that renders instead of preserving
emits `V` and the entry fails visibly:

| id | expr | golden |
|---|---|---|
| `raw-spaced` | `See some code {% raw %}{{ var }}{% endraw %}` | `See some code {{ var }}` |
| `raw-no-spaces` | `See some code {%raw%}{{ var }}{%endraw%}` | `See some code {{ var }}` |
| `raw-uneven` | `See some code {%-  raw%}{{ var }}{%endraw -%}` | `See some code {{ var }}` |
| `raw-two-regions` | two regions with a live `{{ var }}` between them | `{{ var }} V {{ var }}` |
| `raw-multiline` | `{% raw %}\n{{ var }}\n{% endraw %}` | **a `ParserException`** |

- **`raw-multiline` is the surprising one.** `RAW_PATTERN` sets no DOTALL
  flag, so `.` does not match a newline and **a raw region spanning lines is
  not protected at all**; Pebble then chokes on the unknown `raw` tag. That is
  real Kestra behaviour, not a harness artifact. egret does **not** reproduce
  it — minijinja has a real, multiline `{% raw %}` tag of its own, so egret
  renders `"\nV"` where Kestra errors. Recorded as a permanent divergence, not
  a defect to fix: making egret error here would mean special-casing a
  construct minijinja handles correctly.
- **`raw-two-regions` proves the pattern is non-greedy**: each region ends at
  its own `endraw`, and the `{{ var }}` between them still renders.

## `contains` does not work on a number that came from the context (W2 Task 3)

`{{ list contains 2 }}` with `list = [1, 2]` renders **`false`**. The same
expression with a template-literal list — `{{ [1,2] contains 2 }}` — renders
`true`, and `{{ list contains 'a' }}` with `list = ["a","b"]` renders `true`.
Measured, all three, through this harness.

The explanation the evidence supports: a context value arrives through
`ObjectMapper.convertValue(..., Map.class)` as a boxed `Integer`, a numeric
literal in a template is a `Long`, and `Integer(2).equals(Long(2))` is
`false`. Strings are unaffected, and so is `isIn` — `InExpression` calls
`toString()` on both sides before comparing, which is exactly the difference
`isin-number-in-strings` records. Kestra's own variables come from
Jackson-deserialized YAML/JSON too, so this is production behaviour, not a
property of the harness.

Three entries pin it: `op-contains` (strings, `true`),
`op-contains-boxed-int` (context list, `false`), `op-contains-literal`
(literal list, `true`). Whoever ports `contains` must reproduce all three or
they have ported a *different*, more sensible operator.

(Package #5a's Task A6, 2026-10-29, ported `contains` as
`ContainsExpression` does it and reproduces two of the three: egret's values
carry no Integer/Long distinction, so `op-contains-boxed-int` answers `true`
and stays baselined as spec D2's recorded divergence -- a "fix" that answered
`false` there would also answer `false` for
`operator-contains-boxed-int-context`, where Kestra says `true`.)

## Value formatting is the ENVIRONMENT's job, not a filter's (W2 Task 3)

`JsonWriter.write(Object)`
(`core/src/main/java/io/kestra/core/runners/pebble/JsonWriter.java:63-87`) is
what turns a rendered value into output text, and **four** of its behaviours
differ from minijinja's defaults. None of them is reachable from a filter or a
function -- `{{ list }}` and `{{ flag }}` name neither -- so all four live in
`runners::env::json_writer_formatter` / `write_json`:

| value | Kestra | minijinja default | corpus entry |
|---|---|---|---|
| `null` | writes nothing | `None` | `undef-coalesce-null` |
| boolean | `true` / `false` | `True` / `False` | 27 entries |
| `Map`/`Collection`/array | compact JSON, **insertion order**, **null entries omitted** | `{"a": 1}` / `[1, 2]`, sorted, nulls kept | `map-key-order`, `map-nested-null`, `map-top-level-null` |
| non-integer number | `Double.toString` | Rust's own float rendering | `float-list`, `float-scalar-huge` |

Three of these were invisible until entries existed for them, because every
*other* container- or float-valued golden also fails on a filter that is not
ported yet. They were added deliberately rather than discovered thirty entries
into Task 4.

- **Key order** (`map-key-order`): `{"b":1,"a":2,"c":3}`, not sorted. Kestra's
  variables are `LinkedHashMap`s. The layer that sorts on the egret side is
  **minijinja's own `ValueMap`**, which is a `BTreeMap` unless minijinja's
  `preserve_order` feature is on (it is not, and turning it on would be a
  workspace-wide change affecting `egret-dataapi`). Measured directly:
  `Value::from_serialize` on `{"b":1,"a":2,"c":3}` gives
  `{"a":2,"b":1,"c":3}`. `serde_json` is **not** the culprit -- its own
  `preserve_order` is already on in this workspace, transitively via `utoipa`
  (workspace `Cargo.toml:61`) and `cedar-policy-core`, so a
  `serde_json::Value` round-trip keeps the order. The fix is a local
  order-preserving map object (`runners::env::OrderedMap`); no workspace
  feature list was touched.
- **Null omission is recursive, and asymmetric** (`map-nested-null`): the
  mapper is `JsonInclude.Include.NON_NULL`, so
  `{"a":null,"b":{"c":null,"d":1},"e":[null,1]}` renders
  `{"b":{"d":1},"e":[null,1]}`. The **array element survives** -- `NON_NULL`
  applies to object properties, not to sequence elements. An implementation
  that strips nulls everywhere passes the map half of that entry and fails the
  array half, which is why one entry carries both.
- **Floats are `Double.toString`** (`float-list`): plain decimal with at least
  one fractional digit for `1e-3 <= |d| < 1e7`, scientific otherwise --
  `[1.0,0.1,2.5,1.0E100,1.0E-7,1.2345678E7,0.001,1.0E-4,9999999.0,-1.5]`.
  Neither Rust's `{}` nor its `{:?}` produces that. Integers stay plain
  (`int-list`).

## Stringification is a SECOND formatting path (W2 Task 3, fix round 3)

`env.set_formatter` governs what a value looks like in **output**. It does not
govern `Display for Value`, which is what `.to_string()` calls — so the
boolean-casing and float-format fixes above are undone by any code that
stringifies a value some other way. Measured on minijinja 2.24:

| value | `Value::to_string()` | Java `toString()` |
|---|---|---|
| `true` | `True` | `true` |
| `1e100` | `1000000…0.0` (101 digits) | `1.0E100` |
| `none` | `None` | (never reached — see below) |

`InExpression` compares **stringified** operands, so `kestra_is_in` was
silently answering with minijinja's spellings:

| id | expr | context | Kestra | egret before |
|---|---|---|---|---|
| `isin-bool-lowercase` | `{{ b isIn ['true'] }}` | `b=true` | `true` | **`false`** |
| `isin-bool-uppercase` | `{{ b isIn ['True'] }}` | `b=true` | `false` | **`true`** |
| `isin-float-scientific` | `{{ f isIn ['1.0E100'] }}` | `f=1e100` | `true` | **`false`** |

No entry caught it, because none had a boolean or float `isIn` operand.
`runners::env::kestra_to_string` is now the single place a value becomes text
for comparison, and anything added later that compares against Kestra's own
stringification must go through it.

`isin-null-vs-none` is the entry that makes `InExpression`'s null guard
falsifiable, and its needle is `'None'` **with a capital N** — measured, not
assumed. `Value::from(()).to_string()` is `"None"`, so `['none']` would leave
the test exactly as unfalsifiable as `['a']`: neither matches, guard or no
guard.

## `isIn` and an ABSENT variable are two different questions (W2 Task 3, fix round 2)

`InExpression`'s first guard is `leftValue == null || rightValue == null ->
false`. That is a Java **null**, which Pebble keeps strictly apart from an
absent variable: under `strictVariables(true)`,
`{{ nothere isIn ['a'] }}` throws `RootAttributeNotFoundException` and
`{{ obj.missing isIn ['a'] }}` throws `AttributeNotFoundException`. Three
entries pin it (`isin-undefined-left`, `isin-undefined-right`,
`isin-missing-attr-left`), and they exist because porting `isIn` from an
operator to a **function call** created the hazard: minijinja does not apply
`UndefinedBehavior::Strict` to function arguments, so an undefined operand
reaches the function instead of raising. Answering it `false` would let a
typo'd variable inside an `isIn` silently decide a branch.

## `isIn` is not minijinja's `in` (W2 Task 3, addendum A3)

`InExpression.evaluate` stringifies **both** sides before comparing, walks a
`Map`'s keys **and** its values, and returns `false` for any other right-hand
type. minijinja's `in` does none of the three. Measured:

| id | expr | Kestra | minijinja `in` |
|---|---|---|---|
| `isin` | `{{ 'b' isIn ['a','b'] }}` | `true` | `True` (agrees) |
| `isin-number-in-strings` | `{{ 1 isIn ['1','2'] }}` | `true` | **`False`** |
| `isin-map-value` | `{{ 'v' isIn m }}`, `m = {"k":"v"}` | `true` | **`False`** (keys only) |
| `isin-string-rhs` | `{{ 'b' isIn 'abc' }}` | `false` | **`True`** (substring) |

The existing `isin` entry agrees by luck — same types on both sides, list
right-hand — and cannot separate the two. **Verdict: `isIn` needs its own
function.** Implemented in Task 3's fix round: `rewrite` now emits
`kestra_is_in((x), (y))` instead of `(x in y)`, and
`runners::env::kestra_is_in` reproduces all four rows. All four entries
match.

## Corpus (`corpus.json`, 1 234 entries)

- **37 operator cases**, taken verbatim from the task brief plus several
  follow-up fixes: the `??`/`???` pair (5 entries covering null vs. absent on
  both operators, plus a plain present case), ternary (including nesting and
  the unparenthesized `a ? b ? c : d : e` form, which pins Pebble's
  `parseTernaryExpression` grouping empirically -- `a=false, b=true` gives
  `"e"`, not the `"c"` a wrong right-associative reading would give), `isIn`,
  each of `>`/`>=`/`<`/`<=` across string/number/mixed operands
  (`Extension.getBinaryOperators()` registers all four as separate classes —
  `GreaterThanExpression`, `GreaterThanEqualsExpression`, `LessThanExpression`,
  `LessThanEqualsExpression` — so each can diverge independently), three
  equality-boundary cases (`'a' >= 'a'`, `'a' <= 'a'`, `2 >= 2`) that only the
  `=`-inclusive operators can reach, a literal `??`-looking substring inside a
  quoted string (proves a naive regex-based rewriter would corrupt it), three
  `{% %}` tag-body cases (`tag-if-coalesce`, `tag-set-coalesce`,
  `tag-if-ternary`, plus `tag-if-ternary-inverted` and the whitespace-control
  pair `tag-if-coalesce-trimmed`/`coalesce-trimmed-expr` for `{%- -%}`/
  `{{- -}}`), and six entries pinning Pebble's full binary-operator
  precedence table (read out of the pinned jar's bytecode: `or`=10 (NORMAL),
  `and`=15 (NORMAL), `is`/`is not`=20 (TEST), `contains`=20 (NORMAL),
  `equals`/`==`/`!=`/comparisons=30 (NORMAL), the same tier Kestra registers
  `??`/`???`/`isIn` into at `Extension.java:81-87`) against `??`:
  `coalesce-and` (`(a ?? b) and (c ?? d)`, context chosen so a correctly- vs.
  wrongly-shaped REWRITE render to different values through minijinja's
  actual `and` — see "Surprising golden values" below for why this and its
  own golden value are two different, both-worth-knowing things),
  `coalesce-or` (`a or (b ?? c)` — same class of caveat, see below),
  `coalesce-eq` (`(a ?? b) == c`), the tier-20 pair
  `coalesce-is-defined`/`coalesce-is-not-null` (`(a ?? b) is defined` /
  `(a ?? b) is not null` — `coalesce-is-not-null` carries the same
  render-time caveat as `coalesce-and`/`coalesce-or`, see below, for an
  unrelated reason: minijinja's `null` test is spelled `none`), and
  `is-defined-then-coalesce` (`{{ a is defined ?? b }}`, `(a is defined) ??
  b` — the asymmetric side of `is`/`is not`: this operator can only ever
  extend LEFT, confirmed both against the harness for Pebble's own grouping
  and against a real minijinja render of this rewrite's actual output,
  which matches the golden value exactly, modulo the boolean-casing note
  below) — none of which a port that treats `??` as swallowing everything
  up to the next `??`/`isIn` (ignoring lower- and same-tier operators as
  boundaries, or treating `is`/`is not` as symmetric like every other tier)
  would reproduce.
- **35 filter entries**, one per registered filter name, each exercising the
  filter's primary documented behavior with a small, readable context.
- **34 function entries**, one per registered function name (including
  `range` and both of `fetchContext`/`printContext`), split roughly:
  - 26 that actually execute (date/calendar helpers, `env`, `fromJson`,
    `fromIon`, `yaml`, `fetchContext`/`printContext`, `tasksWithState`,
    `loopOutputs`, `range`, the 8 non-deterministic ones above);
  - 8 that are registered but structurally null-backed standalone (`secret`,
    `kv`, `read`, `fileURI`, `fileSize`, `isFileEmpty`, `fileExists`,
    `http`) — included specifically to capture the "Function or Macro does
    not exist" behavior documented above, which is itself the useful signal.
- **2 test entries** for the one registered Pebble test (`json`): a
  true case and a false case, since a single-direction test proves nothing
  about the false branch.
- **153 W2 Task 4 additions** (138 -> 241 -> 268 -> 291), the null / empty /
  wrong-typed / boundary inputs of the 22 string and collection filters that
  task ported, plus its three fix rounds. Their findings are the sections
  "What W2 Task 4's 103 new entries measured", "What W2 Task 4's fix round
  measured" and the three `###` sections after it. **136 of the 153 are
  compared by value and pass**; twelve are compared by error class (see above)
  and five are baselined divergences with their reasons in `baseline.txt`.
- **30 W2 Task 3 additions** (the corpus went 108 -> 138 across three fix
  rounds), covering the three
  controller addenda that could not be written before the oracle existed:
  5 `{% raw %}` entries (addendum A5, see "Raw tags" above), 5 operator
  entries for the three Pebble operators with no minijinja spelling at all
  (addendum A6: `op-equals`, `op-contains`, `op-contains-boxed-int`,
  `op-contains-literal`, `op-range`), 3 `isIn` entries that actually separate
  `InExpression` from minijinja's `in` (addendum A3, see above), and
  `coalesce-nested-absent` (`{{ obj.missing.deeper ?? 'fallback' }}` — the
  shape a real flow writes most often, and the one place
  `UndefinedBehavior::Strict` diverges from Kestra's `??`). Baselined then
  as "deliberate: the alternatives are worse" (task-3-report.md S9).
  **Reversed 2026-10-29**: spec
  `docs/en/specs/2026-10-28-workflow-renderer-design.md` Q6 rules that `??`
  answers through a missing member or index chain, and package #5a's Task
  A5 banked the entry (D6: a path on the left compiles to
  `kestra_path(root, 'a', 'b')` and the operator to
  `(P) if kestra_present(P) else (R)`, so the right side is lazy too).
- **`op-range` records that `..` is inclusive**: `{{ a .. b }}` with `a=1,
  b=3` renders `[1,2,3]`, and `{% for i in 1 .. 3 %}` iterates three times.
  minijinja's `range(1, 3)` is `[1, 2]` — exclusive, and printed with spaces.
  Whoever ports `range` (Task 7's `BoundedRangeFunction`) owns both
  differences.
  *(2026-10-29, package #5a Task A6: `a .. b` compiles to
  `kestra_range(a, b)`, Kestra's inclusive `range`, and `op-range` is
  banked. Descending (`3 .. 1`) and character (`'c' .. 'a'`) ranges are
  `[]`, as A1 measured.)*

## What W2 Task 4's 103 new entries measured (read before porting a filter)

Task 4 added 103 corpus entries — the null, empty and wrong-typed input of
each of its 22 filters — and regenerated. Ten of the answers are not what the
filter's name suggests, and every one of them was read off the regenerated
golden file rather than reasoned about:

- **`distinct` on a null input returns the four-character STRING `"null"`.**
  Every other filter in the batch returns Java `null` (which renders as
  nothing); `DistinctFilter.java:24` returns `"null"`. `filter-distinct-null`
  renders `<null>` where `filter-chunk-null` renders `<>`, from the same
  wrapper. A port that "tidies" this up diverges.
- **`escapeChar` silently loses a map's key order.** `processMap` allocates a
  `new HashMap<>()`, so `{"b": …, "a": …}` comes back `{"a": …, "b": …}` —
  Java hash-bucket order, which is neither the input's order nor sorted in
  general. egret deliberately keeps insertion order and
  `filter-escapechar-map-two-keys` is baselined as a permanent divergence:
  matching would mean porting `String.hashCode`'s bucket layout to imitate a
  bug. Note this is the **opposite** decision from `map-key-order`, where
  Kestra preserves order and minijinja was the one sorting.
- **`className` cannot be right for both a context value and a literal.**
  A whole number from the context is `java.lang.Integer` (Jackson's
  `convertValue` boxes it); the same number written in the template is
  `java.lang.Long`. minijinja has one integer type. `filter-classname-int`
  passes and `filter-classname-literal-int` is baselined — the same shape of
  fact as `op-contains-boxed-int`, and from the same cause.
- **`number` decodes, it does not parse.** `NumberUtils.createInteger` is
  `Integer.decode`, so `"0x1F" | number(type='INT')` is **31** and
  `"010" | number(type='INT')` is **8** — a leading zero is octal. Measured;
  `filter-number-long-hex` additionally settles that `createLong` is
  `Long.decode` and not `Long.valueOf`, which the two commons-lang3 spellings
  do not otherwise distinguish.
- **`number(type='FLOAT')` narrows to 32 bits and it shows.**
  `"3.1415926535"` renders `3.1415927`, not `3.1415926535`. The no-`type`
  heuristic hides this, because it only picks a `Float` when the `float` and
  the `double` print the same string.
- **`number(type='BIGDECIMAL')` keeps its scale**: `"1.50"` renders `1.50`,
  which neither `f64` nor `i128` can hold. egret keeps the canonical text
  (`runners::filters::BigNumber`).
- **`keys` on a list returns its INDICES**, `[0,1,2]`, not its elements.
- **`flatten` is one level only**: `[[1,[2]],3]` is `[1,[2],3]`.
- **`substringBefore` and `substringBeforeLast` disagree about an empty
  separator.** `'abc' | substringBefore(separator='')` is the empty string;
  `'abc' | substringBeforeLast(separator='')` is `abc`. Both are
  commons-lang3's, and no reading of the names predicts the split.
- **`slugify` is ASCII-only after an NFD pass.** `Café Münster` slugs to
  `cafe-munster` (the accents decompose and the combining marks are dropped),
  but `中文 test` slugs to `test` — the CJK is not transliterated, it is
  deleted. `Straße` becomes `strae`.

## What W2 Task 4's fix round measured (27 more entries)

A review built its own 69-entry probe corpus and ran it through real Pebble and
egret side by side. Everything below was then re-measured through *this*
harness before any code moved.

- **`escapeChar` on a null inside a container is a `NullPointerException`**,
  in both container shapes: `Cannot invoke "Object.toString()" because "value"
  is null` for a map, `… because "item" is null` for a list. egret was
  emitting the literal text **`None`** there — Python's spelling, out of a
  filter whose `shell` mode exists to build command lines. The root cause was
  one level down, in `runners::env::kestra_to_string`, which still ended in a
  `Display` fallthrough; that function is now total. **This was the third
  appearance of the `Display` trap in this package**, after Task 3's
  `kestra_is_in` (which was fixed by routing it *through* the function that
  still had the bug).
- **`intValue()` is a 32-bit narrowing and it changes answers.**
  `chunk(size=4294967296)` is `chunk(size=0)` and Guava then throws;
  `indent(amount=-4294967296)` is `indent(amount=0)` and returns the input
  unchanged; `indent(amount=2147483648)` narrows to `-2147483648` and is
  refused. Note `chunk` casts to `Number` (so `chunk(size=2.5)` truncates to
  2) while `indent` casts to `Long` (so `indent(amount=2.5)` is a
  `ClassCastException`) — the two filters differ. (`indent`'s large-`amount`
  case is additionally covered by Kestra's own `MAX_AMOUNT` = 10000, which
  this note originally missed; see "`indent` has TWO limits" below.)
- **`createBigInteger` has its own copy of the radix rules.** `"010" |
  number(type='BIGINTEGER')` is `8` and `"0xFF"` is `255`, exactly as under
  `type='INT'`. A port that implements the rules for one and not the other
  answers the same string two ways depending on `type` — a silently wrong
  number, which is the worst defect this fix round closed.
- **`BigDecimal` renders as `toPlainString()`, not as its input text.**
  `"1e5"` is `100000` and `"1E-3"` is `0.001`, while `"1.50"` keeps its
  trailing zero. Keeping the raw string passes `"1.50"` only because that
  string is already in plain notation.
- **`number(type='INT')` really is 32 bits**: `"3000000000"` throws where
  `type='LONG'` succeeds.
- **`createNumber` checks the hex prefix BEFORE the trailing type qualifier.**
  Testing for a trailing `f`/`d` first sends every hex string ending in a hex
  `d` or `f` down the decimal branch. `"0xFF" | number` is `255`,
  `"0xFFFFFFFFF" | number` is `68719476735` (the width rule promotes it to a
  `Long`), and `"1.5f" | number(type='DOUBLE')` is `1.5` because
  `Double.valueOf` accepts the suffix.
- **`className` on a `BigDecimal`/`BigInteger`** is `java.math.BigDecimal` /
  `java.math.BigInteger`. Those two are the *only* Java box types egret can
  report correctly, because they are the only ones it represents as an opaque
  object; `number(type='LONG')` and `number(type='FLOAT')` produce
  `java.lang.Long` and `java.lang.Float` in Kestra and are baselined here with
  that reason.
- **`Map.equals` ignores key order**, so `distinct` folds
  `[{"a":1,"b":2},{"b":2,"a":1}]` to one element — while `List.equals` is
  order-dependent and `[[1,2],[2,1]]` stays two. A dedup key built from an
  insertion-ordered stringification gets the map half wrong.
- **A null separator is not an empty separator, and the four `substring*`
  filters take four different fallbacks for it**: `substringBefore` returns the
  whole input, `substringAfter` returns the empty string,
  `substringBeforeLast` returns the whole input (its guard is `isEmpty`, which
  folds null and empty together), `substringAfterLast` returns the empty
  string. Erroring on a null separator loses all four.
- **`escapeChar` validates `type` at the LEAF**, inside `escapeString`, so a
  container with no leaf never consults it: `{{ [] | escapeChar }}` is `[]`
  with no argument at all, and `{{ ['a'] | escapeChar }}` is an error.

### `indent` has TWO limits, and only one of them is egret's

**Kestra bounds `amount` at 10000.** `AbstractIndent.MAX_AMOUNT` is
`ConstantValue: 10000`, fed to `RenderLimits.ensureAtMost` between the
`amount >= 0` check and `prefix(args)`; `indent(amount=10001)` throws
`RenderLimitExceededException: The 'indent' filter's 'amount' argument cannot
exceed 10000, but 10001 was given.` **Neither `RenderLimits` nor `MAX_AMOUNT`
exists anywhere in the pinned `kestra` checkout at `96893b589`** — this is the
same artifact-versus-checkout gap recorded above for `BoundedRangeFunction`,
and the same rule settles it: the artifact is authoritative for behaviour, the
checkout only for reading an implementation. `filter-indent-amount-at-max` and
`filter-indent-amount-over-max` commit both sides; the passing golden is 10 KB.

An earlier version of this section said Kestra had no bound here. It was
written from the checkout without checking the artifact, which is exactly the
mistake the `BoundedRangeFunction` note exists to prevent. `RenderLimits` has
**exactly two call sites in the whole jar** — `AbstractIndent` and
`BoundedRangeFunction` — confirmed by `grep -rla RenderLimits` over every
extracted class. Nothing else in the expression language is bounded, which is
what makes the two entries below divergences rather than fidelity.

### Two behaviours have NO golden, on purpose

Both are egret refusing an allocation Kestra attempts. Both bound the
projection against `runners::env::MAX_OUTPUT_BYTES` (1 MiB) — derived from the
existing cap, not a second number — and refuse *before* allocating:

- **`indent`/`nindent` on the `prefix` WIDTH.** Kestra bounds `amount`, not
  the product, so `indent(amount=10000, prefix=<110 chars>)` really does build
  1.1 MB there. (At a 100-character prefix the product is 1 000 003 bytes and
  both engines allow it; the divergence band starts around 105, not "at a
  megabyte".)
- **`number(type='BIGDECIMAL')` on the EXPONENT.** `toPlainString` writes
  `|scale|` zeros and the exponent is an `int`, so `"1E-2000000000"` is a
  two-gigabyte string from a 13-byte input. `NumberFilter` is not one of
  `RenderLimits`' two call sites, so Kestra genuinely builds it.

The reasoning is the same in both cases and is the same call
`rewrite`'s `MAX_NESTING_DEPTH` makes: fidelity does not extend to reproducing
a resource-exhaustion vector in a renderer that W3 runs on author-supplied
source in a request path, and an allocation orders of magnitude past the output
cap defeats a cap that only measures what *survives* the allocation.

Neither can have a golden. At the sizes that matter Kestra OOMs or hangs; at
the largest size it survives quickly the golden would be a multi-megabyte
string committed to `golden.json`. `runners::filters`'
`indent_refuses_to_allocate_past_the_output_cap` and
`bigdecimal_refuses_to_expand_past_the_output_cap` are the pins.
`filter-number-bigdecimal-wide-exp` is the largest case that *does* have a
golden, at 402 characters.

### Pebble's `null` literal is a name minijinja does not have

`{{ null | string }}` is a real literal in Pebble and renders as nothing.
minijinja has no `null` name, so it arrives as an **undefined**, and so does a
**typo'd variable** — which Kestra distinguishes sharply
(`RootAttributeNotFoundException`).

Measured on minijinja 2.24: `UndefinedBehavior::Strict` raises for
`{{ typo }}` but **not** for `{{ typo | string }}`. An undefined piped into a
filter is simply passed in. So the typo renders empty whatever the filters do,
and the only thing left to decide is whether the *literal* is answered
correctly. `runners::filters::is_java_null` therefore treats an undefined as
Kestra's null, which is what makes the seven `*-literal-null` entries pass —
including `filter-distinct-literal-null`, whose golden is `<null>` because
`distinct` alone returns the string `"null"`, and which no other route can
produce.

`filter-string-undefined-input` is the divergence that remains, baselined. The
real fix is one layer up: if `runners::rewrite` translated Pebble's `null`
literal to minijinja's `none`, the literal would be a `None` and the typo would
stay an `Undefined`, and the two could be told apart. That is a change to the
rewriter's vocabulary.

### `string` was the second name minijinja had already bound

Task 3's addendum B2 named `indent` and `range`. **`string` is a third**, and
it is the sharper one: `{{ b | string }}` on a bool renders `True` through
minijinja's built-in and `true` through Kestra's `StringFilter`, and
`{{ m | string }}` renders `{"a": 1}` versus `{a=1}`. The pre-existing
`filter-string` entry (`{{ 42 | string }}`) agreed **by luck** — same answer
either way — so it was never baselined and nothing pointed at the name.
`filter-string-bool`, `filter-string-map`, `filter-string-nested` and
`filter-string-float` are the entries that separate them.

The same finding has a second half. `{{ list }}` and `{{ list | string }}`
render the **same list two different ways**: `[1,2]` through
`JsonWriter.write(Object)` and `[1, 2]` through `AbstractCollection.toString`
— with the space. `runners::env::kestra_to_string` grew `Seq`/`Map` arms for
this; the JSON writer beside it is unchanged.

## What this harness does **not** reproduce

Nothing, as of W2 Task 3. The two `VariableRenderer` behaviours this section
used to list as unimplemented are now both in `Oracle.render`:

- **`{% raw %}...{% endraw %}` tag protection is now reproduced.** Task 1
  left it out and said so ("no corpus entry uses `{% raw %}`"); Task 3 added
  five raw entries, so `Oracle.renderOnce` now does the same UUID
  substitution `VariableRenderer.renderOnce` does, and `Oracle.render` does
  the final `RAW_PATTERN.replaceAll("$2")` that strips the markers. See "Raw
  tags" below.
- **The `indexOf('{') == -1` short-circuit is now reproduced**
  (`VariableRenderer.java:76-79`): a source with no brace at all is not a
  template and is returned unchanged. No current corpus entry reaches it, but
  it is the identity `Property<T>::render` is built on, so the harness and the
  port must agree about it rather than differ silently.
- Recursive re-rendering was already reproduced (`Oracle.render()` loops
  exactly like `VariableRenderer.renderRecursively`, stopping once output
  stops changing, capped at 100 iterations) even though no current corpus
  entry's output happens to look like another template.

## What W2 Task 5's 166 new entries measured (read before touching `date`)

Task 5 ported the six `AbstractDate` filters — `date`, `dateAdd`, `timestamp`,
`timestampMicro`, `timestampMilli`, `timestampNano` — and with them Java's
whole `DateTimeFormatter` pattern language, which lives in
`runners::javafmt`. Everything below was read off a regenerated golden file,
not reasoned about.

### `AbstractDate` reads a number's DIGIT COUNT, not its magnitude

`convert`'s `Long` branch switches on `digitCount(l)`: 13 digits is
milliseconds, 19 digits ending in `000` is nanoseconds, any *other* 19 digits
is `Instant.ofEpochMilli(l / 1e6)` followed by `withNano((l % 1e6) * 1000)`,
and everything else is seconds. That third branch is the surprising one:
`withNano` **replaces** the whole nanosecond field, so the millisecond part the
first step just set is thrown away — `1705314600123456789` renders `.456789`,
not `.123456`. `filter-date-epoch-nanos-truncating` pins it.

### The same digits are a `Long` or an `Integer` depending on where they came from

`convert` has branches for `Long` and for `String` and **none for `Integer`**.
A context integer small enough to box as `java.lang.Integer` therefore falls
through to `(String) input` and throws `ClassCastException` — while the same
digits written as a template literal are a `Long` and are read as an epoch. So
`{{ 1705314600 | date(…) }}` works and `{{ epoch | date(…) }}` with
`epoch: 1705314600` does not, for the commonest number anyone would pipe in.
Same shape of fact as `filter-classname-literal-int`, same cause, and
`filter-date-epoch-seconds-context` is baselined with the reason.

### `dateAdd` does its arithmetic in the INPUT's zone, not `timeZone`'s

`timeZone` is only two things: the formatter's zone (`withZone`) and the
fallback for an input that carries no offset. `ZonedDateTime.parse` keeps
whatever the *string* carried, so
`'2024-03-09T12:00:00-05:00' | dateAdd(1, 'DAYS', 'iso', 'America/New_York')`
adds a day in a fixed `-05:00` offset that has no DST at all, and only then
re-zones for display. The corpus entries spell their inputs
`2024-03-09T12:00:00-05:00[America/New_York]` for exactly this reason — without
the bracketed region there is no zone to be DST-aware about.

Given a real region, the split between `plus`'s two arms is visible and it is
Java's `unit.isDateBased()`: **`+ 1 DAYS` keeps the wall clock** (12:00 → 12:00)
and **`+ 24 HOURS` does not** (12:00 → 13:00). `filter-dateadd-dst-days` and
`filter-dateadd-dst-hours` are the pair. A local time that lands in the spring
gap moves forward by the gap's own duration, so 02:30 becomes 03:30 and not
03:00 (`filter-dateadd-dst-into-the-gap`).

### An unknown format NAME is compiled as a PATTERN

`AbstractDate.formatter` falls through the STYLES map and the FORMATTERS map to
`DateTimeFormatter.ofPattern(name)`. So `date('nope_format')` is not an
unknown-format error at all: it is `Unknown pattern letter: o`.
`filter-date-unknown-named-format` records it. Note also that a **present but
null** `format` is not the default — it is read with `containsKey`, so
`date(null)` reaches `ofPattern(null)` and the message is the single word
`pattern`.

### The ISO_* constants and the `ofPattern` six write fractions differently

Six of the eighteen FORMATTERS entries are `ofPattern` strings (`iso`,
`iso_milli`, `iso_sec`, `sql`, `sql_milli`, `sql_sec`) and twelve are
`java.time` constants. The two families disagree about the same value:
`.120` seconds is `.12` through `ISO_LOCAL_TIME` (a variable-width fraction
with trailing zeros trimmed) and `.120` through `ISO_INSTANT` (groups of
three). `filter-date-format-iso-second-zero` and
`filter-date-format-instant-groups-of-three` are the pair.

`ISO_INSTANT` additionally **ignores `withZone`** — it formats
`INSTANT_SECONDS`, so `iso_instant` answers the same string for
`timeZone='UTC'` and `timeZone='Asia/Tokyo'`
(`filter-date-format-iso-instant-ignores-zone`). And the `['['VV']']` suffix
`iso_date_time`/`iso_zoned_date_time` write appears only when the zone is a
*region*: `timeZone='UTC'` gets `[UTC]`, `timeZone='+05:30'` gets nothing.

### The ISO parser is stricter than it looks

`ZonedDateTime.parse` accepts `+07` and **rejects `+0700`**; it accepts
`2024-01-15T10:30Z` (seconds optional) and rejects `2024-01-15 10:30:00` (a
space for the `T`); a `[Zone]` suffix with no offset in front of it does not
parse at all; and a fraction longer than nine digits is refused. All measured.

### Two pattern letters are text out of a locale database, and egret refuses them

`z`/`zz`/`zzz`/`zzzz` (the zone display name) and `B` (the day period as text)
are CLDR data. Measured across zones: `Asia/Tokyo` is `JST`,
`Asia/Ho_Chi_Minh` is `GMT+07:00`, `Australia/Eucla` is `GMT+08:45`,
`Europe/Moscow` is `GMT+03:00`. tzdb's own abbreviations — the only zone text
available to a Rust port, via `chrono-tz` — agree for the first and third and
say `+07` and `MSK` for the other two, so **no rule is right for both halves**.
`runners::javafmt` refuses with the letter named rather than answering
plausibly, and `filter-date-pattern-zone-name` is baselined with that reason.
`full` and `long` inherit the refusal because their CLDR patterns end in
`zzzz` and `z`; `medium` and `short` do not and both pass.

Everything else in the alphabet is arithmetic or a fixed English word and is
ported, including the ones a reader might expect to be locale text: `O`/`OOOO`
and `ZZZZ` (the localized GMT offset, `GMT+9` / `GMT+09:00`), `VV` (the zone
id), `G` (era), `Q`/`q` (quarter) and the `w`/`W`/`Y`/`e`/`c` week fields.

### Nothing here needed a `RenderLimits`-style bound, and that is fidelity

`RenderLimits` has **exactly two call sites in the whole jar** — `AbstractIndent`
and `BoundedRangeFunction` — re-confirmed for this task with
`grep -rla RenderLimits` over every extracted class. `AbstractDate` is neither,
and it does not need to be: `DateTimeFormatter.ofPattern` caps every repeatable
letter itself (`d` at 2, `S` at 9, `y` at 19, and three *different* JDK messages
for the three), so a pattern of a hundred thousand letters is an
`IllegalArgumentException` rather than an allocation, in Kestra and in the port.
Every letter that survives writes O(1) characters and the one padding construct
(`p`) pads to a width that is itself bounded by the pattern's length, so a
pattern's output is linear in its own input. A huge `dateAdd` amount is
arithmetic, not allocation, and overflows into an error.

So, unlike `indent` and `number(type='BIGDECIMAL')`, **this batch adds no
divergent bound of its own**. `runners::filters::no_date_filter_can_be_made_to_allocate_out_of_proportion`
is the pin for the claim.

### egret's `date` is English-only, and its default zone is UTC

Two stated divergences, both refusals rather than approximations:

- **locale**: only `en` and `en_US` are accepted. Not every English tag,
  because `WeekFields.of(locale)` reads the *country* — see the locale pin in
  Requirements above. `filter-date-locale-french` is baselined.
- **`timeZone`**: absent means UTC here and `ZoneId.systemDefault()` in Kestra.
  No corpus entry can hold this one, because an entry that reached it would not
  be reproducible on another machine — which is the whole reason for the
  standing rule. `runners::filters::an_absent_time_zone_is_utc_here_and_the_hosts_zone_in_kestra`
  is the pin.

A third, smaller one: the twelve `java.time`-constant formats cannot be used as
an `existingFormat`, since they have no pattern spelling for egret's parser to
walk. Four of the twelve fail in Kestra too (they yield no date); the other
eight are the divergence.

## W2 Task 5, fix round 1 — and the PROBE SWEEP that produced it

The first cut of Task 5 was reviewed with a 2 580-expression probe corpus and
then re-checked with a 3 441-expression one of its own. Between them they found
**nine divergence families the 135 hand-written corpus entries could not see**,
and the pattern behind them is worth more than any single fix:

> **Six of the nine sat exactly one variant away from an entry that passed.**
> `uu` beside `u` and `uuuu`; 10:30 beside 05:07; year 10000 beside 2024;
> `timestamp` beside `timestampMilli`; `HALF_DAYS` at 1000 beside `HALF_DAYS`
> at 1. The entries had been chosen, unconsciously, to be the variants that
> pass — and a ratchet structurally cannot notice that, because every entry in
> it is green.

### Run the sweep before committing a filter batch

This is now the standing process for anything with a **vocabulary** — a
pattern language, an enum argument, a set of format names, a zone id. Generate
the cross-product, push it through the harness and through egret, difference
the two sets, and add corpus entries for what comes back. It is a script, not
a design: the corpus format and the harness already exist.

Task 5's sweep was every ASCII letter at counts 1-11 against five dates (a BC
one, a year past 9999, one with single-digit fields, one with sub-second
nanos), all 16 `ChronoUnit` names at ten magnitudes, 50 zone ids, 30
`existingFormat` patterns, 39 quoting/pad patterns, 25 ISO parse inputs and 27
format names. Two machine checks came with it, and both are worth repeating:

- **every pattern letter's accepted COUNTS**, tabulated from the harness and
  differenced against `runners::javafmt::letter_limits`. That is how `v`
  (missing from the alphabet) and `B` (accepted at counts 2 and 3, which Java
  rejects) were found;
- **every one of `chrono-tz` 0.10's 597 zone names fed to `ZoneId.of`**, which
  is how the deny-list below is a closed set rather than a guess.

### What it found

- **`ReducedPrinterParser.getValue` is `Math.abs(value) % 100`.** `uu` on year
  -44 is `44`, not `56`; on year -1 it is `01`, not `99`. `yy` was right only
  because year-of-era is always positive — which is exactly why
  `filter-date-pattern-era-bc` could not see it: it probes
  `G|GG|GGGG|GGGGG|y|u|yyyy|uuuu` and omits the one count that breaks.
- **`SignStyle.EXCEEDS_PAD` puts a `+` on a year past the pad width**, at
  count >= 4 only, and only when the digits *exceed* the width. So `yyyy` on
  year 10000 is `+10000`, `yyyyy` is `10000` and `yyyyyy` is `010000` — and
  Kestra's DEFAULT `iso` format therefore writes `+10000-01-15T…`.
- **`LocalDateTime.plus(n, HALF_DAYS)` is
  `plusDays(n / 256).plusHours((n % 256) * 12)`.** Read out of `src.zip` on the
  same Corretto 25 the goldens come from, where it carries the comment
  `// no overflow (256 is multiple of 2)`. The divisor should be 2, so the
  identity `n half-days == n × 12h` holds only for `|n| < 256`: at 256 the JDK
  adds **one day**, not 128, and `dateAdd(-1000, 'HALF_DAYS')` moves 119 days
  rather than 500. Reproduced rather than corrected — it is a wrong *answer*,
  not a resource vector, and a port that quietly fixed it would give a
  different date for the same flow.
- **`plus(n, ERAS)` FLIPS the era**; it is not a no-op-or-throw.
  `with(ERA, 0)` on 2024 is `withYear(1 - 2024)`, so `dateAdd(-1, 'ERAS')` is
  `-2023-01-15`. Only a target outside `{0, 1}` is the range error.
- **`v` is a pattern letter and `B` has a non-contiguous count set.** `v` takes
  1 or 4 (`JT` / `Japan Time`, the *generic* zone name — a third CLDR table
  beside `z`'s specific names); `B` takes 1, 4 or 5. Everything else is
  `Wrong number of pattern letters`, a **fourth** JDK sentence that nothing had
  pinned. egret had no `v` at all and said `Unknown pattern letter: v`, which
  tells an author their pattern is invalid where Java says it is valid.
- **`p` must be followed by a pattern LETTER, and a too-narrow pad throws.**
  `pp'x'`, `p[H]` and `p-` are all rejected by the JDK, and `date('pH')` at
  10:30 is `Cannot print as output of 2 characters exceeds pad width of 1`.
  `filter-date-pattern-pad` is `ppH|'x'pppmm` at **05:07** — the one clock time
  where both fields fit inside their pads.
- **`ZoneOffset.of` is a LENGTH-driven grammar.** It switches on the id's
  length (2 `+h`, 3 `+hh`, 5 `+hhmm`, 6 `+hh:mm`, 7 `+hhmmss`, 9 `+hh:mm:ss`)
  and reads each number from a fixed position, so `+0:730` and `+5:30` are
  `non numeric characters found` rather than plausible offsets. `+19:00` and
  `+18:00:01` have two further, different sentences, and a failure under a
  `GMT`/`UTC`/`UT` prefix is re-wrapped as `Invalid ID for offset-based
  ZoneId`.
- **Four `chrono-tz` zone ids `ZoneId.of` rejects**: `EST`, `HST`, `MST`,
  `ROC` — the JDK's tzdb build drops the three-letter `backward` links that
  collide with `ZoneId.SHORT_IDS`. `EST5EDT` and `US/Eastern` are fine on both
  sides, which is what makes it counter-intuitive. The set is closed (all 597
  names checked), so `runners::javafmt` denies exactly those four by name. The
  other direction is 13 `SystemV/*` ids Java has and `chrono-tz` does not
  carry at all; those are not fixable here and have no corpus entry.
- **`rfc_1123_date_time` has a year range.** Its year is
  `appendValue(YEAR, 4, 4, NOT_NEGATIVE)`, so a BC year and a five-digit year
  each throw, with different sentences. Every other named format takes both.
- **The ISO literals `T` and `Z` are case-insensitive** (the `java.time`
  constants call `parseCaseInsensitive()`), and a user-written `'T'` in an
  `existingFormat` is **not** (`ofPattern` does not). Measured both ways.
- **`existingFormat` needs adjacent value parsing and a SMART resolver.**
  `yyyyMMdd` parses `20240115` only because a variable-width field hands
  digits back to the fixed-width fields that follow it; and `ofPattern`'s
  default resolver clamps `2024-02-30` to `2024-02-29`, where the bare ISO
  path is STRICT and refuses the same input. `G` is parseable too, and Java
  cross-checks it against a `u` year (`Conflict found: Field Era 1 differs`).

### The two divergences this round had to RECORD rather than fix

- **egret's date range is `chrono`'s** (about ±262 143 years) where
  `java.time`'s is ±999 999 999, so `dateAdd(100000000, 'DAYS')` succeeds in
  Kestra and cannot be represented here. The limit is the crate's; the
  *message* was a choice and was wrong — it reported `long overflow`, Java's
  sentence for a different cause. `runners::javafmt` now checks
  `Math.addExact` first, keeps `long overflow` for genuine arithmetic
  overflow, and says `egret's dates are chrono's` for its own limit.
  `filter-dateadd-past-egrets-date-range` is the baselined entry.
- **`{{ d | timestamp | className }}` is `java.lang.Long` in Kestra and
  `java.lang.Integer` here.** `java_class_name` decides Integer-versus-Long by
  magnitude, and every epoch *second* between 1901 and 2038 fits in an `i32`;
  the milli/micro/nano ones are all outside it, which is exactly why the three
  `className` entries that were added pass and the fourth was the one not
  added. Same underlying limit as `filter-classname-literal-int`.

## What egret copies from upstream, and what it does not

**egret reproduces observable BEHAVIOUR, including defects. It does not
reproduce implementation STRUCTURE.**

The test is a flow author's: would someone running the same flow on both
engines see a different result? A typo in an error message and a wrong date
out of `HALF_DAYS` are both observable, so both get copied. Java's `HashMap`
bucket order, an internal class name, a representation nothing can read back —
not observable from a flow, so not copied. That is already why
`filter-escapechar-map-two-keys` is a recorded divergence rather than a port of
`String.hashCode`'s bucket layout, and why `filter-classname-of-long` is one
rather than a boxed number that has stopped being usable as a number.

The rule cuts both ways and is meant to: it is not licence to tidy anything up
either. A port that "fixed" `HALF_DAYS` would give a **different date for the
same flow**, which is the single thing this package exists to prevent.

### The upstream defects egret deliberately reproduces

Kept as a list, not as scattered comments, for one concrete reason: **when
Kestra or the JDK fixes one of these, our faithful port becomes a divergence
overnight**, and a regenerated `golden.json` will show it as an unexplained
change. Anyone bumping `kestra.version` in `pom.xml` or the JDK should walk
this list first, or they will read a moved golden as a regression in egret.

| # | defect | evidence | pinned by |
|---|---|---|---|
| 1 | **`LocalDateTime.plus(n, HALF_DAYS)` divides by 256, not 2.** `plusDays(n / 256).plusHours((n % 256) * 12)`, so `n × 12h` holds only for `\|n\| < 256`: at 256 the JDK adds one day, not 128. | `src.zip`, `java.base/java/time/LocalDateTime.java:1189`, Corretto 25 — carrying the comment `// no overflow (256 is multiple of 2)`, which is true and irrelevant | `filter-dateadd-half-days-under-256`, `-at-256`, `-past-255`; `runners::filters::date_add_reproduces_javas_half_days_arithmetic` |
| 2 | **`DistinctFilter` returns the four-character STRING `"null"` for a null input**, where every sibling filter returns Java `null` (which renders as nothing). | `javap -p -c DistinctFilter.class`: `ifnonnull 7 / ldc #7 // String null / areturn` | `filter-distinct-null` (renders `<null>` where `filter-chunk-null` renders `<>`) |
| 3 | **Kestra's six `ofPattern` formats use `yyyy` (year-of-era) where they mean `uuuu`, so a BC date silently loses its era.** `iso` on `-0044-03-15` writes `0045-03-15T…` — a positive year with no era marker, which parses back as AD 45, a 2 069-year round-trip error. The twelve `java.time`-constant formats use `uuuu` and are correct. | `javap -p -c AbstractDate.class` static initialiser: `ldc #209 // String yyyy-MM-dd'T'HH:mm:ss.SSSSSSXXX` and its five siblings | `filter-date-format-iso-loses-the-era` against `filter-date-format-iso-instant-keeps-the-era` and `-iso-local-date-keeps-the-era` |

| 4 | **`AbstractDate.convert`'s 19-digit branch throws away the milliseconds it just read.** It is `Instant.ofEpochMilli(l / 1_000_000).atZone(zone).withNano((int)(l % 1_000_000) * 1000)`, and `withNano` **replaces** the whole nanosecond field — so `ofEpochMilli`'s `.123` is discarded and `1705314600123456789` renders `.456789`, not `.123456789`. | `javap -p -c AbstractDate.class`, the `digitCount == 19` branch: `ldiv` by `1000000`, `Instant.ofEpochMilli`, `atZone`, then `ZonedDateTime.withNano` | `filter-date-epoch-nanos-truncating` |

| 5 | **`jq` never sees a null-valued key.** `JqFilter`'s non-String branch is `JacksonMapper.ofJson().valueToTree(input)`, and that mapper is `NON_NULL`, so a map's null-valued entries are dropped on the way IN: `{{ obj \| jq('keys') }}` on `{a:1,b:null,c:3}` answers `["a","c"]`. A flow author reading `keys` gets fewer keys than the object has, and nothing says so. | `javap -p -c JacksonMapper.class`: `configure()` calls `setDefaultPropertyInclusion(NON_NULL)`, and `JqFilter.apply` calls `ofJson().valueToTree` | `filter-jq-keys-drops-null-values` |
| 6 | **`yaml` writes values its own reader cannot read back, and values it reads back as a DIFFERENT TYPE.** With `MINIMIZE_QUOTES` on and `ALWAYS_QUOTE_NUMBERS_AS_STRINGS` off, a numeric-looking STRING is written unquoted: `{{ '1e5' \| yaml }}` is `1e5`, and `yaml(yaml='1e5')` reads that back as the double `100000.0`. `'42'` comes back as an integer. Worse, `{{ '.inf' \| yaml }}` is `.inf` and Kestra's own `yaml()` function then **throws** `Malformed numeric value '.inf'`. | round-tripped 86 values through `x \| yaml` and back through `yaml(yaml=…)` in the harness: 3 of 86 changed type and 2 more errored, all of them `yaml` | `filter-yaml-number-shaped-string`, `filter-yaml-exponent-shaped-string`, `filter-yaml-writes-what-its-own-reader-refuses` |

| 7 | **`Math.abs(Long.MIN_VALUE)` is `Long.MIN_VALUE`.** There is no positive counterpart in the range, so `Math.abs(long)` overflows and Java returns the input rather than throwing; `Math.abs(int)` does the same at `Integer.MIN_VALUE`. `{{ -9223372036854775808 \| abs }}` is negative on both engines. | `javap -p -c AbsFilter.class` reaching `Math.abs:(J)J`, plus the harness | `filter-abs-long-min-value`; `runners::filters::abs_reproduces_javas_overflow_at_the_minimum` |

### What is NOT on that list, and why the distinction matters

**`LessThanExpression`'s `comparisonPebb` typo is recorded, not reproduced.**
`javap -p -c LessThanExpression.class` really does show
`ldc #44 // String Could not perform less than comparisonPebb`, while its three
siblings spell `comparison` correctly — so the typo is real, and `golden.json`
keeps it as documentation. But **egret never emits it**: `lt-mixed` is *both*
`error-class` and baselined, because egret coerces a String/Number comparison
where Pebble throws, so the message is never reached. A version bump that fixed
the typo would move that golden and change nothing in egret.

That is the difference the list is for. A defect on the list is one whose fix
would break us; a defect merely captured in `golden.json` is one whose fix
costs a golden diff and nothing else.

## W2 Task 5, fix round 2 — the `existingFormat` parse sweep

Fix round 1 named `existingFormat` as the thinnest surface and left it at 30
patterns against one input each. This is that surface's own sweep, and it is
the second half of the process the previous section states: **round-trip every
pattern letter through Java's own output, and feed each field the inputs it
must REFUSE as well as the ones it takes.** 426 probes: every letter at every
count that formats, formatted by the harness and then parsed back by it, plus
24 patterns against their near-misses.

**65 of 146 round-trips mismatched on the first run**, fourteen of them
`OK → OK` — a silently wrong *time*, no error anywhere.

### `AbstractDate.convert`'s cascade decides whether there is a time at all

This is the finding, and it is not a detail. `Parsed.resolveTimeLenient` in
SMART mode produces a `LocalTime` only when the fields form a **contiguous
chain from the hour down**:

```text
hod + moh + som (+ nos)  -> a time
hod + moh                -> a time, seconds 0   (nos must be absent)
hod                      -> a time, m/s/n 0     (som and nos absent)
anything else            -> NO time
```

When there is no time, `ZonedDateTime.parse` and `LocalDateTime.parse` both
fail and `convert` falls through to `LocalDate.parse(…).atStartOfDay()`. So:

- `existingFormat='uuuu-MM-dd mm'` **silently discards the minute** and answers
  midnight, and so do `ss`, `S`, `n` and `hh` on their own;
- `uuuu-MM-dd HH ss` discards **both** — a link is missing from the chain;
- `uuuu-MM-dd hh` is midnight but `uuuu-MM-dd hh a` is not, because an
  hour-of-am-pm cannot become an hour-of-day without its `a`;
- **an `a` on its own is the MIDDLE of its half-day**: `AM` is 06:00 and `PM`
  is 18:00. No reading of the pattern predicts that;
- `A` (milli-of-day) and `N` (nano-of-day) each carry a whole time in one
  field, so they resolve where a bare `m` does not;
- and **with no time the parsed offset and zone are discarded too** — the
  `timeZone` argument places the date. `uuuu-MM-dd XXX` on `2024-01-15 +07:00`
  is midnight in `timeZone`, not an instant in +07:00.

### The rest of what it found

- **Fields are range-checked on the way in**, with Java's own sentences.
  `KK:mm a` refuses `12:30 PM` (`HourOfAmPm` is 0-11) and `yyyy-MM-dd` refuses
  `0000-01-15` and `-0044-01-15` (`YearOfEra` starts at 1) — while `uuuu`, one
  letter away, takes both. `ClockHourOfDay` is the exception: its range is
  1-24 but SMART accepts 0 and reads it as hour 0.
- **The offset letters are count-specific and case-sensitive on the parse
  side**, where the ISO path is neither. `XXX` is `+HH:MM` and takes neither
  `+07` nor a lowercase `z`; `X` takes `+07`; `Z` takes `+0700`; `ZZZZ` and `O`
  take the localized `GMT+07:00` form, with a one-digit hour under `O` and two
  under `OOOO`.
- **`A`, `N`, `n` and `g` are variable-width when parsed** (`appendValue(field,
  count, 19, …)`), so reading them at exactly `count` digits leaves the rest of
  the number as unparsed text.
- **`E` is text at every count** — `E`, `EE` and `EEE` all print `Mon` — and
  `EEEEE`, `GGGGG`, `QQQQQ` are the narrow forms.
- ~~**Java parses and then IGNORES `Q`, `q`, `w`, `W`, `F`, `Y`, `g`, `E` and
  `G`**~~ — **this sentence was wrong, and acting on it cost thirteen
  probes.** Java parses those letters and **CROSS-CHECKS** them:
  `Parsed.updateCheckConflict` resolves each into a field and compares it
  against the resolved date, so `uuuu-MM-dd Q` on `2024-03-09 4` is
  `Conflict found: Field QuarterOfYear 1 differs from QuarterOfYear 4`. The
  same mechanism refuses a field supplied twice with two values.
  Consuming-and-discarding them (fix round 2) turned thirteen probes from
  agreeing — both engines had refused the letter — into egret answering where
  Kestra throws, twice with a wrong *time*. Corrected in fix round 3; see that
  section below.

### The one thing `existingFormat` does that `format` does not

**`AbstractDate.convert` calls `formatter(existingFormat)` with no
`.withLocale(…)`**, while `AbstractDate.format` calls
`formatter(format).withLocale(locale).withZone(zone)`. Read off the bytecode.
So the `locale` argument does **not** reach the parse side: an `existingFormat`
of `MMM d uuuu` with `locale='fr'` still wants the *English* `Jan`, and the
localized day-of-week `e`/`c` resolve under `Locale.getDefault(FORMAT)` —
`en_VN` on this machine, Monday-first — which is why parsing `e` = `2` for a
Monday is a `Conflict found: Field DayOfWeek 1 differs from DayOfWeek 2`.

Two consequences, both standing rules:

- **no corpus entry may use `e`, `c`, `w`, `W` or `Y` inside an
  `existingFormat`**, because their meaning there is the harness machine's
  region, not the entry's `locale` argument. The same class of
  non-reproducibility as the `timeZone` rule above;
- egret's parser is locale-independent (English names, no week fields with
  meaning), which matches the pinned harness and is a *narrower* behaviour than
  Kestra's rather than a different one.

### What is still refused on the parse side

**`B`, `v` and `z`** — the three the FORMAT side refuses too, for the same
reason: their text lives in a locale database.

**And, since fix round 3, `w`, `W`, `Y`, `e` and `c`.** All five resolve
through `WeekFields.of(locale)`, and `AbstractDate.convert` passes the parser
**no locale at all** (see the section just above), so their meaning is the
JVM's own region — the harness's conflict messages read
`WeekFields[MONDAY,1]` verbatim. Fix round 2 consumed them silently, which made
egret answer a machine-dependent question; giving them meaning would make egret
answer it *differently* on another host. Refusing says so, and it is what egret
did before round 2.

**And the set is now a committed fixture, checked against the engine.**
`refusal-sets.txt` records what the real engine does and what egret must do,
per letter and per side; `runners::javafmt::the_refusal_sets_match_the_engines_own_per_side`
asserts it in both directions and refuses a row that claims a divergence it
does not have. See "The refusal sets" below.

Kestra parses all eight, so all eight are divergences. **Four now have
baselined corpus entries** (`filter-date-pattern-day-period` for the format
side, and `-day-period-parse` / `-zone-name-parse` / `-generic-zone-parse` for
the parse side); the locale-resolved five cannot have one, because an entry
using them would not be reproducible on another host — the same rule
`timeZone` and `locale` already carry. `runners::filters::
the_locale_resolved_parse_fields_are_refused_rather_than_guessed` is their pin.

**An earlier version of this section said these refusals were "named as
such".** They were named in prose and nowhere in the ratchet: `B`'s only entry
was error-class at a count *both* engines reject, so it was born matching and
pinned nothing at all.

~~One further divergence has no entry…~~ **That paragraph was wrong and is
withdrawn.** It claimed `[uuuu-]MM-dd` was refused by Java on `2024-01-15`; it
is not — Java accepts it, on every output format tried. The probe it came from
was `01-15`, the input with **no** year, and I read the wrong index out of my
own sweep. Both engines refuse that one, so there was never a divergence at
all. `filter-date-existing-format-optional-year-absent` and
`-optional-year-present` now pin both halves, and
`an_optional_section_that_fails_leaves_no_year` is the paired test. A reviewer
could not reproduce the claim because the claim named the wrong input.

## W2 Task 5, fix round 3 — and the rule that finally breaks the pattern

### An entry added for a defect must be shown to FAIL before the fix

**This is now the rule for this corpus, and it is mutation discipline applied
to entry selection.** Write the entry, run it against the *pre-fix*
implementation, watch it fail, then fix. If it cannot be made to fail, it sits
on a value where the two engines agree and it is decoration.

It exists because two reviews in a row found the same shape — new entries
sitting **one variant away** from the failure they were added for — after a
sweep that was supposed to end it. A sweep finds divergences; nothing was
forcing the *entry* written afterwards to sit on one. The instances, all
measured:

- `filter-date-existing-format-ignored-fields` used `Q E G` on `1 Mon AD`, and
  2024-01-15 **is** a Monday in Q1 AD — so it passed no matter what egret did
  with those letters. Its name encoded a belief that was false. Deleted, and
  replaced by `-cross-checked-fields-agree` plus four conflicting siblings.
- `filter-date-existing-format-hour-of-ampm-out-of-range` covered `KK` at 12
  and not `hh` at 0 — the value that had actually regressed.
- `filter-dateadd-past-egrets-date-range` used `DAYS` for a bug that lived in
  the six **time-based** units.
- The sharpest: `date_add_past_egrets_range_says_so_rather_than_claiming_overflow`
  criticises its own predecessor, in its docstring, for "asserting exactly the
  two extremes where the two engines happen to agree" — and then asserted only
  `DAYS`, repeating the shape one unit over.

Fix round 3's 28 entries were written first and run first: **17 failed against
the pre-fix implementation** and 11 are their deliberate agreeing-side partners.

### What round 3 fixed

- **Java CROSS-CHECKS the fields it parses.** See the corrected bullet in the
  round-2 section above. `Q`, `q`, `E`, `D`, `F` and `g` now resolve into the
  date and are compared, and a field parsed twice with two values conflicts the
  same way.
- **An offset is no longer discarded when a zone id follows it.**
  `ZonedDateTime.from` computes `INSTANT_SECONDS` from the parsed *offset* and
  uses the zone for display; clearing the offset moved the instant by the
  difference between them — seven hours for `+02:00 America/New_York` — and
  only when the offset came **first**, which is why no single-letter round trip
  saw it. `OK → OK`, and order-dependent.
- **The AM/PM midpoint fires only when no time field was parsed at all.** The
  round-2 note stated the rule correctly ("an `a` on its own"); the code
  dropped the "on its own", so `uuuu MM dd mm a` on `2024 03 09 25 PM` was
  18:25 here and is midnight in Kestra.
- **SMART's zero-clock-hour exemption applies to `hh`, not only to `k`.**
  `Parsed` has the same block twice and round 2 implemented it once.
  `hh:mm a` on `00:30 AM` was byte-exact before round 2 and an error after —
  the only probe in 5 528 that regressed from *right* to broken.
- **The six time-based `dateAdd` units reach as far as the ten date-based
  ones.** `LocalDateTime.plus` divides by the unit's count-per-day before
  multiplying; a first cut multiplied the whole amount into nanoseconds and so
  overflowed an `i64` at about ±292 years. `dateAdd(10000000000, 'SECONDS')`
  said `long overflow` where Kestra answers `2340-12-05` — a date **inside**
  what egret can represent, 900× short of the limit its own message claimed,
  and reported with Java's arithmetic-overflow sentence for a case Java's
  arithmetic does not overflow on.
- **`Y` has no upper letter count.** `WeekBasedFieldPrinterParser` has no cap,
  so `Y` × 20 is `00000000000000002024` in Kestra and was a refusal here. Note
  the consequence for `no_date_filter_can_be_made_to_allocate_out_of_proportion`:
  its claim that every surviving letter writes O(1) was **false upstream**, and
  egret had been safe only by way of this bug. The bound survives on the
  weaker, true statement — every letter writes at most `count` characters, so
  output stays linear in the pattern's own length.
- **A wide year is read back only with its sign.** `SignStyle.EXCEEDS_PAD`
  parses the way it prints, so `uuuu` takes `+10000` and `uuuuu` does not —
  the other half of fix round 1's fix, which had done only the writing, and
  without which an `iso` round trip of a year past 9999 failed on its second
  leg.

## The refusal sets, and why they needed their own fixture

`letter-counts.txt` covers whether a pattern **compiles**. It does not cover
what happens next, and that is a different layer: `z` compiles and refuses at
render, `w` compiles and refuses at parse. **That layer is where `v` and the
four legacy zone ids lived** — both found by a reviewer running probes, neither
by the suite, because what held them was prose. Prose does not fail.

`refusal-sets.txt` is that layer's fixture: one row per letter per side,
recording what the engine does (measured through the harness) and what egret
must do, with a reason on every row where the two differ.
`runners::javafmt::the_refusal_sets_match_the_engines_own_per_side` asserts it
**bidirectionally**:

- a letter egret refuses that the engine accepts fails, naming the letter and
  the side;
- a letter egret accepts that the engine refuses fails the same way;
- a row whose outcomes **differ** must carry a reason, and a row whose outcomes
  **agree** must not — so a divergence cannot be introduced silently, and a
  refusal cannot claim to be one when the engine refuses too;
- every letter that compiles per `letter-counts.txt` must appear on **both**
  sides, so a new letter cannot enter the alphabet undeclared.

**The two sides differ deliberately, and the file encodes the difference rather
than flattening it.** egret refuses three letters when writing (`z`, `v`, `B` —
CLDR text) and eight when reading (those three plus `w`, `W`, `Y`, `e`, `c` —
resolved through the JVM's own region). Fix round 2 made the sets identical and
wrote down "a letter egret can write, it can read"; fix round 3 broke that on
purpose, and this file is where that decision now lives instead of in a
paragraph that can drift.

The parse-side samples are the engine's **own output** for the same instant, so
a row can never fail for want of a well-formed input — the round-trip technique
from fix round 2, used as the fixture's input generator. The five
locale-resolved samples are formatted without an explicit locale, so the round
trip is self-consistent on the harness machine; that is precisely the fact that
makes them divergent, since under `locale='en'` the engine would write a value
its own parser then rejects.

**Writing the fixture found an undeclared divergence before its test existed.**
`A` and `N` were cross-checked against the *assembled* time rather than field by
field, so a round trip through the engine's own output — where `A` carries a
millisecond that `ss` does not — raised a conflict Kestra does not raise. Java
resolves `A`/`N` into hour/minute/second/nano and compares only the fields that
were actually parsed. Fixed; `A` = 1 against `14:25:36` still conflicts, because
there the hours differ.

Verified by six deliberate breakages, each failing by name: widening the format
refusal (`z` answering with tzdb's id), widening the parse refusal (`w`
consumed rather than refused, as in fix round 2), narrowing a non-refusal (`O`
refusing), deleting a divergent row's reason, deleting a row, and adding a
reason to a row that has no divergence.

## What W2 Task 6's 119 new entries measured (read before touching `jq`, `toJson`, `toIon`, `yaml` or the three `regex*` filters)

Task 6 ported the last seven filters — the four structured-data ones (`jq`,
`toIon`, `toJson`, `yaml`) and the three registered through a constant
(`regexMatch`, `regexReplace`, `regexExtract`) — which closes the count at
**35 of 35**. It ran four sweeps first, before writing a line of Rust, and
everything below was read off a regenerated golden file or off `javap -p -c`.

The sweeps, and what each one is:

| sweep | probes | what it compared |
|---|---:|---|
| structured values | 258 | 86 values × `toJson`/`toIon`/`yaml`, Kestra vs egret |
| the Java pattern language | 265 | Java's constructs against the **raw `regex` crate** |
| jq's surface | 132 | Kestra's `jackson-jq` against **raw `jaq`** |
| YAML scalar styling | 857 | strings and map keys, as values and as names |

### The four filters split THREE ways on a null map value

`JacksonMapper`'s shared `configure()` sets
`setDefaultPropertyInclusion(NON_NULL)`. Three things then happen to it, and
all three are in this one batch:

| filter | mapper | `{"a":null,"b":1}` |
|---|---|---|
| `toJson` | `JacksonMapper.ofJson()` — inherits `NON_NULL` | `{"b":1}` |
| `toIon` | `JacksonMapper.ofIon()` — `createIonObjectMapper` **overrides it back to `ALWAYS`** | `{a:null,b:1}` |
| `yaml` | `YamlFilter`'s OWN mapper, built in its static initialiser, never sees `configure()` | `a: null\nb: 1\n` |
| `jq` | `ofJson().valueToTree(input)` — `NON_NULL` on the way **IN** | `{{ obj \| jq('keys') }}` never sees `b` |

The `jq` row is the surprising one: it is not about output at all.
`{{ obj | jq('keys') }}` omits every key whose value is null, because the
value was already dropped when the `Map` was turned into a `JsonNode`.
`filter-jq-keys-drops-null-values` is the entry.

Confirmed by `javap -p -c` on `JacksonMapper.class` — `createIonObjectMapper`
calls `configure(...)` and then `setDefaultPropertyInclusion(ALWAYS)` on the
next four instructions.

### `jq` returns a LIST, a String input is PARSED, and every failure says the same sentence

- **The result is always a list.** `JsonQuery.apply` emits into an
  `ArrayList` and that list is the return value, so `{{ json | jq('.a.b') }}`
  is `[42]`. `filter-jq-single-match-is-still-a-list`.
- **A String input goes through `ObjectMapper.readTree`, which is not "parse
  this whole string".** `readTree("")` is a `MissingNode` (`[null]`, not an
  error) and `readTree("1 2")` reads the **first value and stops** (`[1]`).
  `jaq`'s `parse_single` refuses both. Two entries, and both were shown to
  fail against the first cut.
- **Every failure is one sentence.** `JqFilter` catches `Exception` around its
  whole body and rebuilds the message from the *input*:
  `Unable to parse jq value '<String.valueOf(input)>' with type '<class>'`.
  A syntax error, a runtime error and an unparseable String input are
  indistinguishable. The value in it is Java's `toString`, so a map prints as
  `{a=1}` — `runners::env::kestra_to_string`, not `Display`.

### `jaq` is not `jackson-jq`, and the differences run both ways

116 of 132 probes agreed. The 16 that did not:

- **three functions Kestra has and `jaq` does not**: `@csv`, `@tsv`,
  `leaf_paths`;
- **five `jaq` has and Kestra's 1.6 does not**: `todate`, `strftime`,
  `repeat`, `debug`, and `1 / 0` (which is `Infinity` in `jaq` and a throw in
  `jackson-jq`);
- **`del` re-sorts an object's keys** in `jaq` and preserves them in
  `jackson-jq`. Every other operation measured preserves insertion order on
  both sides, `keys_unsorted` included;
- the rest were the `NON_NULL`-on-the-way-in and `readTree` facts above, now
  reproduced.

Five entries record what is left, in `baseline.txt`.

### Three things `jaq` does that a renderer on a request path must not

None of these is a fidelity question and none of them has a golden.

1. **`jq('env')` returns the whole process environment.** `jaq_std::funs()`
   registers `env` as `std::env::vars()` — `DATABASE_URL` and every secret in
   it. Kestra's jq has no `env` at all (measured: it errors), so withholding
   it is fidelity as well as the obvious call. `runners::filters::run_jq`
   filters it out of the function list by name;
   `jq_cannot_read_the_process_environment` is the test, and it sets a canary
   variable rather than asserting on a message.
2. **`jq('halt')` calls `std::process::exit`.** Not through anything exotic:
   `jaq_core::unwrap_valr`, the function the crate's own doc example calls,
   ends in `exit(code)` for a halt. A rendered `Property` is author-supplied
   source, so that is a flow terminating the process. `run_jq` matches on
   `Exn` directly instead and never calls `unwrap_valr`.
3. **jq generators are infinite.** `1 | repeat(. + 1)` never stops and Kestra
   collects into an `ArrayList` until the JVM dies. `run_jq` bounds the
   accumulated output against `MAX_OUTPUT_BYTES`, the same 1 MiB ceiling
   `runners::env` caps a whole render at — the same call `indent`'s prefix
   bound and `rewrite`'s `MAX_NESTING_DEPTH` make.

### Java's regex is not Rust's, and 101 of 265 probes proved it

The sweep ran Java's pattern constructs against the **raw `regex` crate**, and
the dangerous half is not the half that errors:

| pattern | subject | Java | crate, untranslated |
|---|---|---|---|
| `\d` | `٣` | no match | **match** |
| `\w` | `é` | no match | **match** |
| `\s` | `\u00a0` | no match | **match** |
| `\v` | `\n` | match | **no match** (it is a vertical TAB there) |
| `a.b` | `a\rb` | no match | **match** |
| `\bcafé\b` | `le café noir` | no match | **match** |
| `(?i)k` | `K` (U+212A) | no match | **match** |
| `c$` | `abc\n` | match | **no match** |
| `a++a` | `aaa` | no match | **match** |

Not one of those raises anything on either side. `runners::javare` translates
every one of them — `\d \D \w \W \s \S \h \H \v \V \R`, `.` under `s` and `d`,
`\b`/`\B` as Java's ASCII boundaries, `\Q…\E`, the numeric escapes, Java's
POSIX `\p{Alpha}` family, categories and scripts, class intersection and
nesting, and case-insensitivity **expanded per literal** because Java's `(?i)`
is ASCII-only and the crate's is not.

**Nine constructs are refused instead, each naming itself**: lookahead,
lookbehind, atomic groups, possessive quantifiers, backreferences, `\G`,
`\b{g}`, Unicode *blocks* and `\p{java…}` predicates. After the translation
the whole 265-probe sweep leaves exactly those and nothing silent.

Two further facts came out of it:

- **`$` needs the subject, not just the pattern.** Java's `$` outside
  `MULTILINE` is `\Z` — it matches before a *final* line terminator. The
  crate's is end-of-haystack. `$` is translated to `\z`, which is exact on
  every subject that does not end in a terminator, and the subject that does
  is refused. Consuming an optional terminator instead answers `regexMatch`
  correctly and then **deletes the newline** in `regexReplace`.
- **the replacement string is a third language.** Java's `\$` is a literal
  dollar and the crate's `\` is not an escape at all; `$2` against a one-group
  pattern throws in Java and yields `""` in the crate; and `$10` against a
  one-group pattern is group 1 followed by a literal `0`, because Java reads
  digits greedily and backs off at the group count. `expand_replacement` is
  Java's, not the crate's.

### `RegexUtils`' ten-second timeout is not ported, and that is the C7 call

All three filters go through `RegexUtils.matcher`, which wraps the subject in
a `TimeoutCharSequence` whose `charAt` throws after `Duration.ofSeconds(10)`
(`javap -p -c RegexUtils.class`; `MAX_USER_REGEX_LENGTH = 250` is on the same
class and is **not** on this path). That bound exists because
`java.util.regex` backtracks.

The `regex` crate cannot. `(a+)+b` against 40 `a`s and a `!` is answered here
in the time one linear scan takes, and Kestra spends ten seconds and throws.
egret is strictly better and cheaper, so there is nothing to bound and nothing
to reproduce; `runners::javare::a_catastrophic_java_pattern_is_answered_instead_of_timed_out`
is the pin. Note this is the opposite direction from `indent`'s `MAX_AMOUNT`:
there, Kestra had a bound and egret had to grow one.

### `ion-rs` cannot write Kestra's Ion, measured before it was refused

Task 6's brief names `ion-rs` 1.0 for `toIon`. It was tried first. On
`ion-rs` 1.0.1, against the same value:

| | Kestra | `ion-rs` 1.0.1 |
|---|---|---|
| `{a:1}` | `{a:1}` | `{a: 1}` — a space after every `:` and `,` |
| a list | `[1,2]` | `[1, 2, ]` — a trailing comma |
| `1e100` | `1.0e100` | `1e100` |
| `"a\x01b"` | `"a\x01b"` | the raw control byte, unescaped |

and none of it is configurable: `ion_rs::Writer`, `ValueWriter` and the
`v1_0::Text` layout options are `pub(crate)` in 1.0.1, so `Element::to_string`
and `encode_as` are the only public routes and both produce the right column.
Post-processing that into Kestra's form is a second serialiser wrapped around
the first, so `runners::ionfmt` is the first one instead. **No `ion-rs` in the
lock file.**

Its two non-obvious rules, both measured over 27 floats and 130 strings:

- **the float text starts from `Double.toString` and then diverges by whether
  there is an exponent.** With one, lowercase the `E` and keep the mantissa
  (`1.0E100` -> `1.0e100`); without one, drop a trailing `.0` and append `e0`
  (`1.0` -> `1e0`, `9999999.0` -> `9999999e0`, `-0.0` -> `-0e0`). So `1.0` and
  `1.0e100` disagree about whether the `.0` survives.
- **Ion's keyword set is case-sensitive and tiny.** `true`, `false`, `null`
  and `nan` are quoted as field names; `True`, `TRUE`, `Null`, `yes`, `on`,
  `y` are **bare** — the exact opposite of what the `yaml` filter in the same
  batch does with the same keys.

### `yaml` is TWO layers, and they pick different quote characters

Jackson's `YAMLGenerator.writeString` requests a style and SnakeYAML's
`Emitter` downgrades it. Everything Jackson forces comes out **double**-quoted;
everything SnakeYAML downgrades comes out **single**-quoted. So
`{{ 'yes' | yaml }}` is `"yes"` and `{{ '&a' | yaml }}` is `'&a'`, from the
same filter on the same shape of input.

Jackson's half, off the bytecode:

```text
if (text.isEmpty())                  -> DOUBLE
if (text.indexOf('\n') >= 0)         -> LITERAL
if (needToQuoteValue(text))          -> DOUBLE
else                                 -> PLAIN
```

with `needToQuoteValue = isReservedKeyword || valueHasQuotableChar`, and
`looksLikeYAMLNumber` **not** in it. That is why:

- **`{{ '42' | yaml }}` is `42`** — an unquoted scalar that reads back as a
  number. `ALWAYS_QUOTE_NUMBERS_AS_STRINGS` is off and nothing else looks at
  numbers on the value side. This is upstream's configuration, not a defect of
  the port.
- **a NAME is quoted by a different rule.** `needToQuoteName` is
  `isReservedKeyword || looksLikeYAMLNumber` and **drops**
  `valueHasQuotableChar`, so `{'a,b': 1}` is `a,b: 1` (a comma is quotable in
  a value and not in a name) and `{'-a': 1}` is `"-a": 1` (a leading `-` makes
  a name look like a number and does nothing to a value). And
  `_looksLikeYAMLNumber` is a switch on the **first character only**, which is
  why `2024-01-15` and `.inf` are quoted as names.

Three rules came only out of the 857-probe styling sweep:

- **`^0[0-9_]+$` is not plain.** `007`, `01`, `00`, `0789` and `010` are
  single-quoted; `0`, `0x0`, `0b1`, `0.7`, `0e7`, `08.5`, `+007` and `-007`
  are plain. `0789` is not octal and is quoted anyway, so no reading of "it
  looks octal" predicts the set.
- **the simple-key limit is 127, not 128.** `'k' * 127` is `k…: 1` and
  `'k' * 128` is `? k…\n: 1`. SnakeYAML's constant is 128 and its comparison
  is `>=`; an earlier cut of `runners::yamlfmt` read the constant and got the
  boundary wrong, and one probe out of 857 sat on it.
- **the root empty string brings the document marker back.**
  `{{ '' | yaml }}` is `--- ""\n` even though `WRITE_DOC_START_MARKER` is off.

`INDENT_ARRAYS` is what makes the layout unlike every other YAML writer's: a
sequence's `-` sits at *parent + 1* and a sequence under a map key keeps the
**map's** indent, so `{"a":[1]}` is `a:\n - 1\n` and `[[[1]]]` is
` -\n   -\n     - 1\n`.

### The re-render loop was eating a trailing newline, and only `yaml` could see it

`render_str` feeds each pass's OUTPUT back in as the next pass's SOURCE, and
minijinja's default `keep_trailing_newline(false)` drops one trailing newline
from a template source. So any rendered value ending in a newline lost it on
the pass after the one that produced it — silently, in a loop that is not
about newlines at all.

Pebble is configured `newLineTrimming(false)` and the environment now matches
it (`env.set_keep_trailing_newline(true)`). **Every `yaml` output ends in a
newline**, which is why this filter is the first thing in the crate that could
see it: reverting the one line makes 18 entries fail, `filter-yaml` included.
`a_multiline_raw_region_is_not_protected_by_the_pattern`'s expectation moved
from `"\nV"` to `"\nV\n"` with it, and `"\nV\n"` is the faithful value.

### minijinja's string-literal lexer is not Pebble's

Pebble does **not** process backslash escapes inside a single-quoted string:
`'(a)\1'` reaches a filter as six characters and `'\\'` as two. minijinja
reads `\1` as U+0001 and `\\` as one backslash. `\d`, `\G`, `\p` and `\$` pass
through both.

So a Kestra pattern or replacement written with either spelling means
something different in egret **before any filter sees it**, and no filter can
fix it. Two entries record it and both are baselined;
`filter-regexreplace-escaped-backslash-from-a-variable` is the same
replacement supplied through the context and it passes, which is what
separates "the filter is wrong" from "the literal arrived different". The fix
belongs in `runners::rewrite` / the environment's lexing.

### Two formatter rows moved, and both moved zero entries

`runners::env::write_json` is the `JsonWriter` port and W2 Task 6's brief says
not to touch it, because a change there moves ~20 entries at once. Two changes
were still necessary, and the mismatch count was measured either side of each:

- **`\uXXXX` escapes are UPPER case in Jackson and lower case in
  `serde_json`.** Only reachable for the control characters whose code point
  contains a hex letter — `0x0B`, `0x0E`, `0x0F`, `0x1A`-`0x1F`, since
  `\b \t \n \f \r` have named escapes. `filter-tojson-control-character` is
  the entry and it is the only one that moves.
- **a non-finite double is QUOTED.** JSON has no infinity, so Jackson writes
  `["Infinity"]`. Only reachable through `jq` (a JSON context cannot carry
  one); `filter-jq-infinite-is-a-quoted-string` is the entry.

Both were verified by reverting them one at a time: each makes exactly its own
entry fail and nothing else.

### The round trip: 6 of 258 do not come back, and all six are upstream

Serialising with the engine and feeding the output back through the engine's
own reader is the technique W2 Task 5's fix round 2 used on `existingFormat`,
and it is the one instrument that finds a value that survived being written
and then means something else. 86 values × three round trips
(`fromJson(x | toJson)`, `fromIon(x | toIon)`, `yaml(x | yaml)`), through the
harness, comparing each against `x` serialised directly:

- **252 stable**;
- **3 came back a different TYPE**: `'42'` -> `42`, `'1.5'` -> `1.5`, and
  `'1e5'` -> `100000.0` — a string in, a double out, no error anywhere. All
  three are `yaml`, and they are `MINIMIZE_QUOTES` with
  `ALWAYS_QUOTE_NUMBERS_AS_STRINGS` off;
- **2 errored on the way back in**: `{{ '.inf' | yaml }}` is `.inf` and
  Kestra's own `yaml()` function refuses it with `Malformed numeric value
  '.inf'`. Same for `.nan`. The filter writes what its own reader rejects;
- **1 errored for a different reason**: `{{ nothing | toIon }}` is the string
  `"null"` and `fromIon('null')` throws `Invalid ion: t is null in
  sink.next(t)`. That is the null-input branch this whole batch shares meeting
  the `fromIon` function, which is Task 7's.

`toJson`/`fromJson` and `toIon`/`fromIon` are otherwise lossless across all 86.
egret reproduces the three type changes and the two refusals exactly — its
`yaml` writes `1e5` and `.inf` unquoted, because that is what Kestra writes.

### The registration count closes at 35

`Extension.getFilters()`' 35 `Map.put` call sites, read out of the artifact's
bytecode and sorted:

```
chunk className date dateAdd distinct endsWith escapeChar flatten indent jq
keys md5 nindent number regexExtract regexMatch regexReplace sha1 sha512
slugify startsWith string substringAfter substringAfterLast substringBefore
substringBeforeLast timestamp timestampMicro timestampMilli timestampNano
toIon toJson urldecode values yaml
```

22 (Task 4) + 6 (Task 5) + 7 (Task 6) = 35, and `runners::filters::register`
now has 35 `add_filter` calls against them.

**`ReplaceFilter` is in the jar and is not registered.** `javap` finds
`io/kestra/core/runners/pebble/filters/ReplaceFilter.class` next to the other
38 class files, and no `put` names it — so `{{ x | replace(…) }}` in a Kestra
flow reaches **minijinja's own `replace`** in egret and Pebble's built-in in
Kestra, and neither is Kestra's class. Worth knowing before Task 8 reconciles
the built-in surface.

## W2 Task 6, fix round 1 — three vectors that end the process, and thirteen silent regex classes

A review of the port ran its own sweeps and found three things the task's own
sweeps had not, plus a set of counting errors. Everything below was
re-measured here before it was believed.

### Two ways a jq expression takes the process with it

Neither is a wrong answer; both are "the service is gone", from a template.

- **The parser overflows the stack.** `jaq`'s loader is recursive descent with
  no depth limit: **692** nested `[` on the 8 MiB main stack and **171 on a
  2 MiB one — a tokio worker's — i.e. a template about 360 bytes long.** `[`,
  `(` and `{a:` all reach it. A stack overflow is `SIGABRT`, not a panic, so
  `catch_unwind` does not see it and `tests/oracle.rs`'s panic guard cannot
  report it — reproduced here by watching the probe binary die with signal 6.
  Kestra parses 2000 levels without complaint.
- **The evaluator has no budget of any kind.** `grep fuel|budget|steps` over
  `jaq-core` 3.1 finds nothing, and minijinja's own `FUEL` does not apply
  because the whole jq program runs inside one filter call. The first cut
  counted bytes **as outputs were emitted**, which bounds a generator that
  emits and nothing else: `[repeat(1)]`, `reduce repeat(1) as $x (0; .+1)`,
  `until(false; .)`, `[while(true; .)]` and `def f: f; f` produce their first
  output never and walked straight past it. The doc said "the output is
  bounded" and the test asserted `is_err()`, which **passed the implementation
  that shipped** — an `is_err()` cannot tell "the bound fired" from "`repeat`
  is not registered".

`runners::jqsandbox` is the answer to both, and it is built on the only two
places `jaq` lets a caller stand — the definition list and the native list:

| bound | what it is | what it stops |
|---|---|---|
| `MAX_SOURCE_DEPTH` = 64 | a scanner over the source, **before** the loader | the parser overflow, with a 2.6× margin on the measured 171 |
| `MAX_SOURCE_BYTES` = 1 MiB | source length | the parse cost itself |
| `BOUNDED_DEFS` | `repeat`, `recurse` ×3, `while`, `until` and `range` **replaced in place** with depth-counting versions | the whole recursion family, `..`, `paths`, `walk` and `leaf_paths` included, since every one of them is defined on `recurse` |
| refusing a user `def` | by name, in the scanner | `def f: f; f`, which is two words and which no bound above touches; mutual recursion cannot be read out of the text |
| the emitted-byte cap | unchanged | a generator that does emit |

**`MAX_STEPS` is 65 536, and two ceilings meet there.** The output cap bounds
*useful* steps at `MAX_OUTPUT_BYTES / 2` = 524 288, because an emitted value
costs at least two characters. Time bounds *tolerable* steps lower: measured,
`[repeat(1)]` spends **3.3 s** at 1 048 576 steps and **0.2 s** at 65 536. The
lower one wins, and the deepest recursion in this package's own jq sweep is in
single digits.

**Replacing a definition has to happen IN PLACE**, and `splice` asserts it
rather than assuming it: everything above the group is what it needs (`error`,
`select`, `empty`) and everything below it needs the group. Appending would
leave `paths` resolving `jaq`'s unbounded `recurse`; prepending would leave
`error` undefined. A `jaq` bump that renames, re-arities or separates one of
them fails the assertion.

### Withholding a native is not enough on its own

`jaq-std/src/defs.jq` lines 3-9 say `def stderr: (stderr_empty as $x | .), .;`
and `def halt: halt(0);`. Dropping `stderr_empty/0` or `halt/1` therefore makes
the **whole standard library** fail to compile, and every jq call fails
whatever the author wrote — measured, because the first cut of this module did
exactly that and `{{ x | jq('.a') }}` broke along with `env`. So the six
user-visible names stay and their **bodies** become refusals that say why.

Six natives are withheld, and **Kestra's `jackson-jq` at `Versions.JQ_1_6` has
none of them**, so withholding is fidelity as well as the obvious call:

| withheld | why |
|---|---|
| `env` | `std::env::vars()` — the whole process environment |
| `debug`, `stderr` | write arbitrary author text into the service log; `egret-bin` installs `LogTracer` |
| `localtime`, `strflocaltime` | `jiff::tz::TimeZone::system()` — the host's timezone |
| `halt` | reaches `std::process::exit` through `jaq_core::unwrap_valr` |

**And the guard is an ALLOW-list, because a deny-list is fail-open.**
`jaq-std = "3.0"` is a caret range, so a `cargo update` to 3.1 can register a
native no deny-list mentions. `ALLOWED_NATIVES` is the 108 egret runs,
committed, and `the_allow_list_covers_every_native_jaq_registers` fails if the
three crates ever register a name that is on neither list.

### Java regex: thirteen more classes, nine of them silent

A 357-probe sweep with **every pattern and subject passed through the
context** — so the known string-literal lexer divergence cannot contaminate it
— found thirteen classes the first cut answered differently. Nine raised
nothing on either side:

| pattern | subject | Java | egret, before |
|---|---|---|---|
| `[a-z&&b&&c]` | `c` | false | **true** |
| `[&&a]` | `a` | true | **false** |
| `(?i)\p{Lower}` | `A` | true | **false** |
| `(?i)[^a-c]` | `B` | false | **true** |
| `(?i)[Z-a]` | `z` | true | **false** |
| `(?U)\bcafé\b` | `le café noir` | true | **false** |
| `(?dm)^b` | `a\rb` | false | **true** |
| `(?x)[a b]` | `" "` | false | **true** |
| `'aab' \| regexReplace('a*','-')` | | `--b-` | **`-b-`** |

Four more were the other direction: egret **answering** where Java raises
(`a**`, `a{`, `a{,3}`, `[&&]`, `(?x)[a#b]`, `(?<λ>a)`) or **refusing** what
Kestra accepts (`\X`, `\N{…}`, `\p{all}`, a surrogate pair, `[\Qa+b\E]`).

Five of them are worth reading for the shape of the mistake:

- **`&&` is what separates operands, and the parser did not know it.** The
  tail after the first `&&` was read as ONE operand, so `[a-z&&b&&c]` became
  `a-z` intersected with the *set* `{b, &, c}`. Java's `&&` chain is
  left-associative intersection of every operand, and an empty operand is
  IGNORED (`[&&a]` is `[a]`) while all-empty is a syntax error.
- **`(?i)` is now folded by asking the engine.** Java's `CASE_INSENSITIVE`
  without `UNICODE_CASE` matches `c` against class `C` iff `c ∈ C` **or**
  `swapAsciiCase(c) ∈ C`, whatever `C` is. So the class is written once,
  compiled, and the 52 ASCII letters are tested against it. A member-rewriting
  fold cannot do this: `[Z-a]` spans `[`, `\`, `]`, `^`, `_` and `` ` `` and
  has no mirrored range, and `\p{Lower}` has no members to rewrite at all.
- **The fold happens BEFORE the negation.** `(?i)[^a-c]` does not match `B`:
  the fold closes `a-c` into `a-cA-C` and the negation then excludes both
  cases. Closing the negated class instead — which the first cut did — matched
  `B`.
- **`(?U)`'s refusal named `\b` while the guard did not cover it.** The
  message at the `\d`/`\w`/`\s` arm read "which redefines \d, \w, \s and \b"
  and the `'b'`/`'B'` arms had no `unicode_classes` check at all. The
  refusal message and the guard now come from one constant.
- **`Matcher.replaceAll` is not `captures_iter`.** The crate's iterator
  suppresses an empty match abutting the previous match's end; Java's matcher
  takes it and then advances one character. Reverting the fix to
  `if false && whole.is_empty()` does **not** reproduce the old behaviour — it
  is an infinite loop, which is how that was found.

**`oracle/regex-classes.txt` is that sweep, committed.** 171 rows of
`pattern / subject / java / egret / reason`, asserted bidirectionally by
`runners::javare::the_regex_classes_match_the_engine`, with one rule the
`refusal-sets.txt` fixture does not need: **a row whose `egret` column is a
boolean and differs from `java` fails outright.** That is the silent class,
and no reason excuses one — the only acceptable divergence is a refusal that
says so. It exists because the claim it checks ("after the translation nothing
is silent") was made once as a sentence and was false. A sentence cannot fail.

Twelve constructs are refused by name: lookahead, lookbehind, atomic groups,
possessive quantifiers, backreferences, `\G`, `\b{g}`, Unicode blocks,
`\p{java…}`, `(?U)` where it redefines a class or a boundary, `\X`, and
`\N{…}`.

### `oracle/jq-surface.txt`, and why "five is what is left" was wrong

The first round recorded FIVE jq divergences and wrote "five entries record
what is left". A 191-expression differential finds **twenty-one**. Two are now
fixed and nineteen are recorded, each with a reason, in a fixture
`runners::jqsandbox::the_jq_surface_matches_the_engine` re-runs.

The two fixed are `scan` at both arities, and they are the ones that would
have reached a flow: `jaq-std/defs.jq` says `def scan(re): scan(re; "");` — no
`g` flag — so `"a1b2" | [scan("[0-9]")]` was `["1"]` here and is `["1","2"]`
in Kestra and in jq 1.6. With a capture group jq yields the captures and jaq
yielded the whole match. Both engines answered without error.

### `BigDecimal` was a STRING in three writers, one of them the environment's

`'1.50' | number(type='BIGDECIMAL') | toJson` was `"1.50"` here and is `1.50`
in Kestra. So was `toIon`, and so was **`{{ [x] }}`** — `runners::env`'s own
`write_json`, with no filter involved. All three end in a catch-all that
treats an unrecognised value as text, and a `BigNumber` is `ValueKind::Plain`.

Fixing it turned up a second fact: **a `BigDecimal` has two spellings and both
are reachable from a template.** `toPlainString()` is what a bare `{{ x }}`
renders and `toString()` is what a serialiser writes, and they differ whenever
the scale is negative or the adjusted exponent is below -6. `'1E3'` is `1000`
bare, `1E+3` through `toJson` and `yaml`, and **`1d3` through `toIon`**, which
spells a decimal exponent with `d`. `BigNumber` now carries both.

### Three `yamlfmt` bugs, and two entries that MASKED them

Worse than the one-variant-away entries earlier rounds found: those masked
nothing.

- **The literal-block indentation indicator is RELATIVE, and SnakeYAML always
  writes `2`.** egret wrote the absolute column, so past depth 4 it emitted
  `|10-` — and the indicator is a single digit 1-9, so **the output was not
  valid YAML for any reader**. `filter-yaml-literal-block-indent-indicator`
  used a root scalar, where the relative and absolute values coincide.
- **The simple-key limit counts UTF-16 code units**, because SnakeYAML compares
  against `String.length()`. 64 astral characters is a 128-unit key and is
  NOT simple. Both existing entries were pure ASCII — exactly where the two
  candidate rules agree.
- **YAML's four named escapes were missing**: `\N` (U+0085), `\_` (U+00A0),
  `\L` (U+2028), `\P` (U+2029). The last three matter for more than looks: a
  raw one inside a double-quoted scalar is a line break to a YAML 1.1 reader,
  so the output read back as a different string.

### The counting, corrected

Three figures in the first round's report and in this file were wrong, and the
right ones come out of the files:

- **119 entries were added in round 1**, not 118, and this file said 116 —
  written before the last two went in. The corpus is **673** after this round.
- **The `keep_trailing_newline` reversion fails 27 entries** on the current
  corpus, not the 18 first reported. The first measurement was taken against a
  smaller corpus and was not re-taken when it grew.
- **`filter-yaml-scalar-keeps-its-newline` did not discriminate its own fix.**
  It was written `<{{ s | yaml }}>`, and the `>` after the value is what stops
  minijinja trimming the newline — so the entry passed either way. It is now
  a bare `{{ s | yaml }}` and it fails under the reversion.

## Regenerating the two answer fixtures

`regex-classes.txt` and `jq-surface.txt` each carry a **Kestra column** that
`golden.json` does not: they are answers from the Java harness, committed as
text. Unlike `golden.json` they were, for one round, snapshots nothing could
re-derive — so a change upstream could not make them rot loudly.

Both are regenerated the same way `golden.json` is, from a probe corpus in the
same schema:

```bash
export JAVA_HOME=/Users/1pro-l4r8/Library/Java/JavaVirtualMachines/amazon-corretto-25.jdk/Contents/Home
cd crates/egret-workflow/oracle
mvn -q -o compile
CP=$(mvn -q -o dependency:build-classpath -Dmdep.outputFile=/dev/stdout | tail -1)

# `fixtures/regex-classes.probe.json` and `fixtures/jq-surface.probe.json` are
# committed beside this file, in `corpus.json`'s schema.
"$JAVA_HOME/bin/java" -cp "target/classes:$CP" host.Oracle generate \
    fixtures/regex-classes.probe.json fixtures/regex-classes.answers.json
"$JAVA_HOME/bin/java" -cp "target/classes:$CP" host.Oracle generate \
    fixtures/jq-surface.probe.json fixtures/jq-surface.answers.json
```

The probe corpora and the harness answers are **committed next to the
fixtures**, so the `kestra` and `java` columns can be diffed against a
regenerated pair the same way `golden.json` is, and a Kestra bump moves them
visibly. `runners::javare::the_regex_classes_match_the_engine` and
`runners::jqsandbox::the_jq_surface_matches_the_engine` assert that the
committed column and the committed answers agree, so a hand-edited fixture
fails.

## W2 Task 6, fix round 2 — bounding by CLASS, not by the examples in the review

A review found that both bounds from fix round 1 had been fitted to the
examples in its own message rather than to the class behind them, and walked
past both in minutes. That is the "the entries were the variants that pass"
shape, moved from the corpus to the fix. **Measured examples are evidence of a
class, not a specification of it.**

### The parse: no scanner can bound it, so the STACK is the bound

`check_source` counted bracket nesting. **Six of the seven abort vectors
contain no bracket at all**, so it saw depth 0. Re-measured here by bisecting
the shortest program of each shape that aborts a 2 MiB stack:

| construct | shortest abort | stack per source byte |
|---|---:|---:|
| `-`×N + `1` | **205 B** | **9.99 KiB** |
| `[`×N / `(`×N | 345 B | 5.94 KiB |
| `{a:`×N | 513 B | 3.99 KiB |
| `try `×N | 817 B | 2.51 KiB |
| `.` + `\|.`×N | 5.1 kB | 0.40 KiB |
| `.` + `,.`×N | 43 kB | 0.05 KiB |
| `.` + ` // .`×N | 109 kB | 0.02 KiB |

And the scanner had a scanner's defect on top: it popped an interpolation
frame on **any** `)`, so after `"\((1),` it believed it was inside a string
and stopped counting — `"\((1),` + `[`×171 + `]`×171 + `)"`, 352 bytes,
aborted.

The cost per byte is **constant in the stack size** (9.99 at 2 MiB, 9.93 at
8 MiB, 9.91 at 64 MiB), so it is a property of the construct and can be used
as a bound. Two mechanisms, no scanner:

1. the parse and the run happen on a **dedicated thread with an explicit
   stack** (`JQ_STACK_BYTES` = 64 MiB), so the budget does not depend on
   whether the caller was a 2 MiB tokio worker;
2. the source is capped at `MAX_SOURCE_BYTES` = 64 MiB / 10 KiB / 6 =
   **1092 bytes**, and the arithmetic is in the constants.

`a_source_at_the_cap_parses_on_the_jq_thread` is the other half of the
derivation: a bound that refuses everything is not evidence that what it
admits is safe.

The `def` refusal is now **not string-aware on purpose** — a `def` inside a
string literal is refused too. That over-refuses `jq('"def"')` and removes the
class of defect a scanner has, which this module has now paid for twice.

### The run: a step counter cannot bound a step that never happens, or one whose cost grows

Two shapes got past `MAX_STEPS`:

- **`range` with a step of `0`** spins inside the native, one call, no steps.
  `jqsandbox.rs`'s guard was `$by != 0 and …`, and the short-circuit removed
  the guard for precisely the step that never terminates. Measured: 1.67 GiB
  RSS at 20 s, still climbing; `reduce range(0;1;0) as $x (0;.+1)` a flat
  8.9 MiB pure CPU spin.
- **`[recurse(.+[1])]`** stops at 65 536 iterations as designed and reaches
  **9.9 GiB**, because each value is one element longer than the last.

So the bound is no longer only a counter:

- **`__egret_tick` is a wall-clock DEADLINE** (`MAX_RUN` = 1 s), called once
  per step by all four bounded recursion definitions. A deadline does not need
  anyone to predict the shape of the next abuse, which is the property a
  counter lacks.
- **`__egret_guard` adds a shallow-length cap** in jq, because a `RunPtr` is a
  higher-ranked `fn` over `<D as DataT>::V<'a>` and naming that projection as
  `Val` — which inspecting the value needs — does not typecheck. It tests
  `. >= ""`, since jq's total order puts numbers before strings, arrays and
  objects, and **`BOUNDED_DEFS` may use only what `jaq-core`'s defs.jq has
  defined above the group it replaces**: `type` comes from `jaq-std`, and
  referencing it made the whole standard library fail to compile.
- **`range` with a step of `0` now emits nothing**, which is what Kestra's
  `jackson-jq` does with it — measured, `[range(0;1;0)] | length` is `0`
  there. `jaq` spins. Matching Kestra is both the fidelity answer and the
  terminating one.

**What is left, stated rather than implied:** a program can still compose
bounded pieces into an expensive whole and reach the deadline only at the next
tick. The complete answer is abandoning the thread on a timeout, which leaks a
runaway thread into a long-lived process.

### The regex sweep, widened: 151 silent rows, all closed

Round 1's fixture sampled where the answers agree. A 1 465-probe
cross-product of every Java POSIX name and six Unicode categories against
`(?i)`, `(?u)`, `(?U)` and 19 subjects found **151 silent rows in 30 pattern
classes**. All 2 161 regex probes now leave **zero**.

- **`(?i)` over a property has a DIFFERENT fold, and the round-1 doc's premise
  was wrong.** It said Java's rule is `c ∈ C or swapAsciiCase(c) ∈ C`
  "whatever `C` is". True for a character, a range and an **ASCII-only**
  property; false for a Unicode one — Java folds `\p{Lu}` with full Unicode
  mapping even without `UNICODE_CASE`. `(?i)\p{Lu}` matches `é`, `ς`, `ſ`,
  `à` and `к`. A first cut of the fix then folded *every* property with
  Unicode and got the other half wrong: `(?i)\p{Alpha}` matched `ſ` and
  `\u212a`, which Java does not.
- **`(?U)` redefines the whole POSIX family**, not the four escapes its own
  refusal message named. 29 rows.
- **`(?u)` was dropped inside a class**, so `(?iu)[k]` did not match U+212A
  while `(?iu)k` did — the fixture had the literal forms.
- **`[a&&&b]` is `[ab&]` in Java.** `clazz()` reads the right operand
  `while (ch != ']' && ch != '&')`, so a third `&` leaves it null, the `&&`
  does nothing, and the `&` is a literal. Derived from eleven measured
  `&`-runs, not guessed: `[a&&&&b]` IS an intersection and `[&&&]` throws.
- **`(?m)^` does not match an empty subject** in Java — there is no line —
  and the crate's matches at position 0. Refused.
- **A lone surrogate (`\uD83D`) compiles in Java** and matches nothing; egret
  called the pattern invalid. It is now a subexpression that cannot match.

### Two more

- **A context integer wider than 64 bits loses its digits, and not in a
  filter.** `{"v": 18446744073709551616}` renders `1.8446744073709552E19`
  where Kestra renders the exact digits, through `{{ v }}`, `toJson`, `yaml`,
  `toIon` and `jq`. `serde_json` without `arbitrary_precision` converts such a
  literal to an `f64` **inside its own parser**, before any `Visitor` method
  is called — so `visit_i128`/`visit_u128` do not help and are deliberately
  not implemented. The fix is a Cargo feature, and features are additive
  across the graph: it would change `serde_json::Value`'s number
  representation for every crate in this workspace. Baselined with that
  reason.
- **The definition list is now guarded the way the native list is.**
  `ALLOWED_NATIVES` made a new *native* a test failure; nothing did the same
  for a new *definition*, which is the thing this module bounds by replacing.
  `the_definition_list_is_the_one_this_module_was_written_against` pins the
  count and the set of definitions that name themselves.

### Two tests that passed the implementation that shipped

Both are recorded because the shape recurs:

- `a_deeply_nested_jq_source_is_refused_before_the_parser_sees_it` tested `[`
  and `(` — the two the review named — and passed while `-`×241 aborted in
  241 bytes.
- `the_bounded_definitions_parse` asserted the definition **names**, which
  `def repeat(f): .;` passes with no bound anywhere; and
  `the_literal_in_the_jq_source_is_the_bound` counted the literal `65536`,
  which `rec($n)` instead of `rec($n-1)` passes while the bound never fires.
  They are replaced by `every_bounded_definition_actually_stops`, which RUNS
  each program and asserts the message names the bound that stopped it.

### `\L` and `\P` are unreachable, and the hazard is still there

`yamlfmt`'s double-quoted writer emits `\N`, `\_`, `\L` and `\P`. The last
two are unreachable today, and that matches Java: the style selector never
picks a double-quoted scalar for a string whose only unusual character is
U+2028 or U+2029 — SnakeYAML folds a **plain** scalar across them instead,
which is what `filter-yaml-next-line-character-folds` records.

**The read-back hazard is still there, on both engines.** A raw U+0085,
U+2028 or U+2029 in a scalar is a line break to a YAML 1.1 reader, so Kestra's
own plain-scalar output does not round trip. Escaping is what avoids it, and
Kestra does not escape.

## W2 Task 6, fix round 3 — the THIRD axis, and where egret itself recurses

A reviewer aborted the process with a 40-byte template:

```
{{ v | jq('reduce range(1400) as $i (.;[.])') }}   ->  SIGABRT
```

Three orders of magnitude inside `MAX_SOURCE_BYTES`, 0.04 s of evaluation so
the deadline never fires, and `reduce` is not one of the definitions
`BOUNDED_DEFS` replaces. **Both existing bounds were on the wrong axis.**
`MAX_SOURCE_BYTES` bounds the source; `MAX_STEPS` and `MAX_RUN` bound the
evaluation; neither bounds the **depth of the value jq builds**, and every one
of egret's own value writers recurses over that.

**This was the fourth time a bound had been placed where the last exploit was
rather than where the recursion is.** So, before fixing it: every place egret
recurses on a value, and what bounds each.

| recursion | thread | bounded by |
|---|---|---|
| `jaq_json::Val::to_string()` (the byte cap) | jq | the depth check, which runs first |
| `runners::filters::jq_out` | jq | the depth check |
| `jaq_json::Val`'s `Drop` | jq | `JQ_STACK_BYTES`, 256 MiB |
| `jaq_json::read::parse_single` (the input) | jq | `serde_json`'s own 128 limit upstream of it |
| `runners::filters::jaq_value` | **caller** | the depth check |
| `JqOut`'s and `minijinja::Value`'s `Drop` | **caller** | the depth check |
| `runners::env::write_json` | **caller** | the depth check, or `serde_json`'s 128 for a context value |
| `runners::ionfmt::write_ion` | **caller** | as above |
| `runners::yamlfmt::write_map`/`write_seq` | **caller** | as above |
| `runners::env::kestra_to_string` | **caller** | as above |
| `runners::env::json_to_minijinja` | **caller** | `serde_json`'s 128 |
| `runners::filters::write_equality_key` (`distinct`) | **caller** | as above |
| `runners::filters::escape_value` (`escapeChar`) | **caller** | as above |

**Nine of the thirteen are on the CALLER's thread**, which in production is a
2 MiB tokio worker and which `JQ_STACK_BYTES` does not touch — the module's
own comment said so ("the conversion happens back on the caller's thread") and
the bound was placed as though it did not.

### `MAX_VALUE_DEPTH` = 128, and why that number

The check runs **on the jq thread, before anything touches the value**, so
nothing deep ever crosses back; and it is **iterative**, because a recursive
depth check is the same overflow one function earlier.

128 is **`serde_json`'s own recursion limit**, which is what already bounds the
other way a value enters this crate — a context deeper than that fails to
parse. One invariant instead of two: *no value anywhere in this crate is more
than `MAX_VALUE_DEPTH` deep.*

It is also far inside what the recursion can afford. Measured by bisecting the
depth that overflows a thread of a given size, and linear in that size:

| writer | KiB of stack per level |
|---|---:|
| `kestra_to_string` | **1.346** |
| `write_ion` | 1.049 |
| `write_json` | 1.018 |
| `to_yaml` | 0.391 |

128 levels at the worst of those is 172 KiB — a **twelvefold** margin on 2 MiB.
`a_value_at_the_depth_limit_renders_on_a_tokio_workers_stack` renders a
max-depth value through every writer on a real 2 MiB thread, because that is
the configuration the claim is about.

**`__egret_guard` was measuring the wrong dimension.** Its test is
`. >= "" and length > 1048576`; every node of a `[.]`-nest has length 1, so it
is a **breadth** guard and the attack is **depth**. It stays — a single
enormous step is a real shape — but it was never going to catch this.

### The source cap was cutting real programs, and the lever is the stack

A 1 352-byte pipeline Kestra runs was refused. A thread stack is reserved
address space committed lazily, so `JQ_STACK_BYTES` is now **256 MiB** and
`MAX_SOURCE_BYTES` follows it to **4 369 bytes**, with `PARSE_STACK_MARGIN`
untouched at 6. What can actually be touched is cap × 10 KiB ≈ 43 MiB, and
only by a deliberately pathological source.

### The over-fold came back through `(?u)`

`(?iu)\p{Alpha}` on `ſ` and `\u212a`: Kestra false, egret true, both booleans,
no error. `emit_class`'s condition was
`has_property || flags.unicode_case`, and for an ASCII-only property
`has_property` is false while `|| flags.unicode_case` still selected the
Unicode fold. **Java's `(?u)` is `UNICODE_CASE` and does not widen a POSIX
property — only `(?U)` does.** The round-2 fixture had `(?iu)` on a LITERAL
and never on a fold-preimage.

A class that **mixes** an ASCII-only property with anything else needs a
per-member fold under `(?iu)` — Java folds the two halves differently — and one
character class cannot carry two. Refused, 14 rows of a 7 182-probe sweep.

### `&&` runs: the rule is the run LENGTH, and an earlier ODD run

Round 2 derived the single-run rule correctly and then generalised it wrongly
by counting no-ops. Measured over 43 patterns (`[a&^k b]` for k in 1..7 and
`[a&^k1 b&^k2 c]` for k1,k2 in 1..6):

| k | what the run is |
|---|---|
| 1 | a literal `&` |
| 2 | an intersection |
| odd | (k-1)/2 no-ops and a literal `&` |
| even | k/2-1 no-ops and an intersection |

and a run containing a no-op is **`Bad intersection syntax` if an earlier run
was odd and at least 3**. `[a&&&b&&&c]` throws; `[a&&&&b&&&&c]` and
`[a&&&&&b]` do not. Counting no-ops was wrong in both directions.

### The probe corpus is wider along the two dimensions that leaked

`regex-classes.txt` went from 171 rows to **1 868**: every Java POSIX name and
four categories × `(?i)`/`(?u)`/`(?iu)`/`(?U)`/`(?iU)` × bare, in a class, and
in a MIXED class; plus every `&`-run pair from 1 to 6. The regeneration path
was already there — the gap was the corpus's width, not the mechanism.

### After the fix: 30 adversarial programs, no abort and no hang

Deep values through every filter, deep objects, deep-and-wide, many deep
results, `paths`/`walk`/`tostream`/`setpath`/`getpath` over deep values, a
100 000-bracket JSON string input, a 65 536-element output, a catastrophic
regex, a 100 kB `regexReplace`, and a source at and past the cap. **Nothing
took more than 0.8 s and nothing aborted.**

## W2 Task 6, fix round 4 — the hazard is per-THREAD, and every descent over author-controlled input

A reviewer aborted the process again, this time without going near jq:

```
{{ s | regexMatch(regex=p) }}   with p = '[' x 2000    ->  SIGABRT
{{ 'a' | regexMatch(regex='[' x 3000) }}               ->  SIGABRT
```

`javare::parse_class` -> `class_member` -> `parse_class` is a mutual recursion
driven by the pattern text, and a flow author's pattern is text. The second
form is the one that matters: **`rewrite::MAX_NESTING_DEPTH` never looks
inside a template's string literal**, so it is not a second guard.

That was the fifth bound placed where the last exploit was. The review's
instruction was to stop doing that: *"The hazard is not per-value-type. It is
per-thread."* So the render moved onto a thread whose stack egret chooses
(`env::RENDER_STACK_BYTES`), and the per-construct caps are derived from that
one number — and this is the list the next person should read before adding a
recursive descent.

### Every recursive descent over author-controlled input

Author-controlled means: the template text, a filter argument, or a context
value. "Render thread" is `render_str`'s own thread since fix round 4; before
it, every one of these ran on a 2 MiB tokio worker.

**What this table is, and what it is not.** It is a living artifact validated
by *search*, not by construction: nothing derives it from the grammar or the
call graph, and no tool checks it. **Five successive review sweeps each found
an axis the previous round did not cover** — the jq parser, the jq value
depth, the regex class parser, `minijinja`'s expression parser, and the date
formatter's optional sections — and the fifth found a row here that was
**affirmatively wrong**: `javafmt` was certified as having "no nesting
construct" while `[...]` was one, recursing unbounded and quadratically. That
is not the table failing. It is what adversarial search against a large
surface looks like, and it is the reason the table exists rather than a reason
to distrust it.

So: **a reader who adds a recursive descent must add a row AND attack it.**
A row asserting a bound that nobody tried to break is the exact shape of the
error this table already made once. The bounds below each have a test that
fails when the bound is removed — verified by removing it, not by asserting
it — and that is the standard a new row has to meet.

| descent | input | thread | bounded by |
|---|---|---|---|
| `javare::Translator::parse_class` <-> `class_member` | pattern text | render | `MAX_CLASS_DEPTH` = 10 000, Java's own |
| `regex_syntax`'s parser over the translated pattern | pattern text | render | `nest_limit(MAX_CLASS_DEPTH)` — its default 250 would refuse patterns Java compiles |
| `javare::class_text` / `write_class_item` (AST walk) | pattern text | render | the same parse bound: the AST cannot be deeper than the parse |
| `javare::class_has_property`, `class_has_ascii_only_property`, `class_is_only_ascii_properties` | pattern text | render | as above |
| `ClassAst`'s `Drop` | pattern text | render | as above — and it unwinds the same tree, so it is on the same budget |
| `minijinja`'s expression parser and evaluator | **template text** | render | `rewrite::MAX_EXPRESSION_BYTES` = 40 329 significant bytes per region |
| `rewrite::expand`, `rewrite::resolve_flat` and `rewrite::resolve_pieces` — mutually recursive, and `resolve_pieces` recurses on OPERATOR SPLITTING as well as on region nesting | template text | render | `MAX_NESTING_DEPTH` = 500, **one budget shared by all three** |
| `serde_json`'s parser (a context value) | context | caller | its own 128, which admits 126 levels under a variable |
| `jaq`'s parser (a jq program) | filter argument | jq | `MAX_SOURCE_BYTES`, derived from `JQ_STACK_BYTES` |
| `jaq_json::Val::to_string`, `filters::jq_out` | jq result | jq | `MAX_VALUE_DEPTH`, checked first |
| `jaq_json::Val`'s `Drop` | jq result | jq | `JQ_STACK_BYTES`, 256 MiB |
| `filters::jaq_value`, `JqOut`'s and `MjValue`'s `Drop` | jq result | render | `MAX_VALUE_DEPTH` |
| `env::write_json`, `ionfmt::write_ion`, `yamlfmt::write_map`/`write_seq`, `env::kestra_to_string`, `env::json_to_minijinja` | any value | render | `MAX_VALUE_DEPTH`, or `serde_json`'s 128 for a context value |
| `filters::write_equality_key` (`distinct`), `filters::escape_value` (`escapeChar`) | any value | render | as above |
| `javafmt::Pattern::parse_pieces` over `[...]` optional sections | **format string**, from `existingFormat` | render | `javafmt::MAX_OPTIONAL_DEPTH` = 39 568 |
| `javafmt`'s other formatter walking | format string | render | the format string's own length; no nesting construct |
| `ionfmt::IonReader::value` <-> `list`/`struct_` (W2 Task 7's `fromIon`) | **a filter argument** | render | `MAX_VALUE_DEPTH`, checked as it descends |
| `functions::BoundedVisitor` over `serde_norway`'s events (`yaml()`) | **a filter argument** | render | `functions::Depth`, a `DeserializeSeed` carrying `MAX_VALUE_DEPTH`. `serde_norway`'s parser is an event stream with no limit of its own, so this is the ONLY thing standing here |
| `functions::BoundedVisitor` over `serde_json`'s parser (`fromJson()`) | a filter argument | render | the same `Depth`, and `serde_json`'s own 128 upstream of it |

### The template parser: what round 4 deferred, and why the deferral was wrong

Round 4 disclosed this descent, deferred it, and **named the wrong trigger**.
The example cited was `{{ v | lower | lower | … }}`, which is the case
`minijinja` already defends: its parse fuel stops the chain cleanly. Two
constructs cost almost no fuel per level and slip past it. Re-measured on the
shipped path:

| trigger | source bytes per level | stack per level | result |
|---|---:|---:|---|
| `{{ ---…---0 }}` unary minus | 1 | 832 | **`SIGABRT` at 170 000** |
| `{{ not not …true }}` | 1 | 832 | **`SIGABRT` at 170 000** |
| `{{ v.a.a…a }}` attribute chain | 2 | 1 472 | **`SIGABRT` at 100 000** |
| `{{ v \| lower \| … }}` | 8 | — | parse fuel catches it |
| `{{ ((((…)))) }}` | 1 | — | `MAX_NESTING_DEPTH` catches it |

The panic is `thread 'egret-render' … has overflowed its stack`. **It
overflows the thread egret sized**, which is the part that matters: "bound the
thread, not the site" does not contain this one. 128 MiB is itself
overflowable by ~170 KB of single-byte characters, and `render_str` is `pub`,
reached from `Property::render` with no length cap in `property.rs`,
`spec/validate.rs` or `publish.rs`.

**The bound is `rewrite::MAX_EXPRESSION_BYTES`**, derived the way
`jqsandbox::MAX_SOURCE_BYTES` is: 134 217 728 / 832 / 4 = **40 329**. Every
recursion level in the template grammar consumes at least one token, and every
token outside a string literal is at least one byte, so a region's length is
an upper bound on the parser's depth.

Round 4 also said a length cap would be the wrong instrument because it would
refuse a 40 KB SQL body with three variables in it. That was right about a cap
on the TEMPLATE and does not apply to a cap on **one region's insides**: the
SQL body is the text between the regions, and it is never counted. Two shapes
make the difference concrete, and both are pinned:

- `{{ '<200 000 characters>' | upper }}` renders. A string literal is one
  token however long it is, so `significant_bytes` counts it as one — and a
  date test really does pass a 100 000-character pattern that way.
- a 50 KB body with fifty small `{{ v }}` regions renders, unchanged.

### The date formatter's optional sections, which the table had certified safe

`oracle/README.md` used to read: *"`javafmt`'s formatter tree | format string
| render | the format string's own length; **no nesting construct**"*. `[...]`
is a nesting construct. Two defects lived behind that sentence.

**Time.** `parse_pieces` found the matching `]` by scanning forward, once per
level per attempt — O(remaining) inside O(depth), so O(depth²). Measured on
the shipped render path:

| depth | before | after |
|---:|---:|---:|
| 10 000 | 833 ms | 9 ms |
| 20 000 | 3 223 ms | 15 ms |
| 40 000 | 12 774 ms | 29 ms |
| 200 000 | killed at >2 min | refused in 17 ms |

Four times the work for twice the input, before; twice, after. The matching
end is now resolved **once, while the pattern is parsed**, and carried in
`Piece::OptionalStart(distance)` as a distance **relative** to the start —
which is what survives the `&pieces[i + 1..end]` slicing the parser recurses
through, where an absolute index would not. The per-level `parsed.clone()` was
suspected of being part of it and is not: it is a fixed-size struct, and
removing only the scan made the whole thing linear.

**Depth.** The recursion itself was unbounded and overflowed the render thread
at 160 000. `javafmt::MAX_OPTIONAL_DEPTH` is derived the same way as the
others: 134 217 728 / 848 / 4 = **39 568**.

**Kestra's own limit is far lower, and egret does not copy it.** Measured
through the harness: Kestra parses 1 000 nested sections and throws
`java.lang.StackOverflowError` at 5 000. A `StackOverflowError` is a catchable
`Error` in Java and an abort in Rust, so matching that boundary would refuse
patterns for no reason but that a JVM stack ran out. The cap sits above
anything Kestra can parse and below what would abort.

### A third quadratic, found by attacking the round-5 fix

Neither Critical, and on the same axis as both: **`rewrite` itself was
quadratic in the number of regions.** The loop asked for the next `{{` and the
next `{%` separately, and `find("{%")` on a template containing no `{%` at all
reads to the end — once per region.

| regions (7 bytes each) | before | after |
|---:|---:|---:|
| 10 000 | 869 ms | 23 ms |
| 40 000 | 13 480 ms | 94 ms |
| 200 000 (1.4 MB) | **331 572 ms** | 449 ms |

Five and a half minutes, entirely inside `rewrite` — the engine had not been
reached. It terminates, so it is not an abort, but a template a flow author
can write should not burn a worker for five minutes. One scan for `{`, deciding
from the next byte, is linear and picks the same delimiter: the two openers
cannot begin at the same index, so "earliest, `{{` winning a tie" and "first
match while scanning" agree.

Worth noting for what it says about the method: this was **not** on the list.
It is not a recursion at all, so no row of the enumeration covered it, and no
review round had raised it. It turned up because the round's last step was to
attack the fix rather than to reason about it — which is the argument for
making that step mandatory rather than a courtesy.

### A note for W3, so this is not rediscovered

Five rounds have each found one more unbounded recursive descent over author
input, and each was closed with a per-construct cap. The caps are correct and
each is pinned, but the *pattern* has a limit: every one of them is a promise
that a human enumerated every descent correctly, and the enumeration was
already wrong once about `javafmt`.

At the point where this goes on a request path, the right answer stops being
another cap and becomes **a supervisor that survives a crash** — a render
that runs where a stack overflow costs a request rather than the process. That
is an architecture decision for the package that puts rendering behind an
endpoint, not for a filter batch, and it is written here so W3 inherits the
reasoning rather than rediscovering it. What W3 should know:

- a Rust stack overflow is `SIGABRT`: `catch_unwind` does not see it, and no
  amount of `Result` plumbing changes that;
- the render already runs on its own thread, so moving it behind a process or
  a supervised worker is a smaller step than it looks;
- the five caps are cheap and should stay either way — they turn an abort into
  a message, which is what a flow author needs to see regardless of who
  supervises the process.

### What the eighth sweep found, and what the report may no longer say

477 patterns x 18 subjects, crossing the flags with the intersection forms,
found 37 rows where egret answered a boolean and it was the wrong one. Two
families, both pre-existing, both now closed:

**`(?x)` and a run of `&`.** Java's `next()` skips insignificant whitespace
and its `unread()` backs up one position, and those two disagree. All four
corners, measured:

| pattern | Java |
|---|---|
| `(?x)[a &b]` | `&` is a member |
| `(?x)[a & b]` | the `&` is GONE — `{a,b}` |
| `(?x)[a & ]` | `Unclosed character class`: the `]` is eaten as a member |
| `(?x)[a & ]x]` | `{a,],x}` |

and `(?x)[a & [bc]]` matches nothing at all, because the `[` is eaten as a
member too, the class closes at the first `]`, and the second `]` is left in
the pattern as a literal — so the regex needs TWO characters. `& &` with
whitespace between is still an INTERSECTION, which is what made
`(?x)[a & & b]` silent: the run scanner was a raw `while` that never consulted
the `(?x)` skip.

**The ASCII fold across an intersection.** The fold was appended to the
FLATTENED class text, which lands the case counterparts in the last operand of
an `&&`: `(?i)[\p{Alpha}&&[^b]]` on `b` is `false` in Java and was `true`
here, because `[^b]` became `[^b] u {b,B}` — everything. The fold is now
applied per operand and per nesting level. Union is the only operation it
distributes over for free, which is exactly why an operand may be folded whole
and an intersection may not.

**And a third, which the widened corpus found on its own:** `(?U)` sets
`UNICODE_CASE` as well as `UNICODE_CHARACTER_CLASS`, and `(?-U)` clears both.
Three rows separate a bit from an implication: `(?iU)[a-z]` matches `ſ`,
`(?iU)(?-U)[a-z]` does not, and `(?iuU)(?-u)[a-z]` does not either — so `U`
moves the bit rather than implying the behaviour at use time. Setting only the
one bit made `(?iU)` fold ASCII-only.

The report's earlier sentence — "zero silent rows across seven sweeps" — was
true of the sweeps that had been run and false as a claim about the filter.
`oracle/regex-classes.txt` now carries 2 853 rows including the flag x
intersection cross-product, so the claim is a fixture rather than a sentence.

### Java's own threshold is not a number

`MAX_CLASS_DEPTH` reproduces both of Java's sentences — `Unclosed character
class` inside it, `Stack overflow during pattern compilation` past it — but
**the depth at which Java switches between them is not stable.** Within one
JVM run, the same depth answered differently on consecutive repetitions:

| depth | rep 1 | rep 2 | rep 3 |
|---:|---|---|---|
| 7 000 | stack overflow | unclosed | unclosed |
| 8 000 | stack overflow | unclosed | unclosed |
| 9 000 | stack overflow | unclosed | unclosed |
| 12 000 | stack overflow | stack overflow | stack overflow |

It depends on the live stack when the pattern is compiled. So the corpus
entries compare by error class, the fixture asserts only the two ends, and no
test anywhere may assert Java's crossover.

### What it costs, measured rather than estimated

The first version of `MAX_CLASS_DEPTH`'s doc said one level costs "under 200
bytes". Bisecting the depth that survives a thread of a given size:

| stack | deepest class that compiles | bytes per level |
|---|---:|---:|
| 2 MiB (a tokio worker) | 1 046 | 2 004 |
| 8 MiB (a libtest thread) | 4 192 | 2 001 |

so it is **about 2 KiB**, ten times the guess. 10 000 levels need about 20 MB,
which is where `RENDER_STACK_BYTES` = 128 MiB comes from. Note the second row:
a libtest thread cannot hold Java's own depth either, so the test for this
spawns its own thread of `RENDER_STACK_BYTES` — which makes it a pin on that
constant as well.

### Two more, found by attacking the fix rather than reasoning about it

The review asked whether it could still be aborted or hung after the fix.
Twenty-nine attacks were run — class and group nesting at 50 000, alternation,
nested quantifiers, `\Q` at a megabyte, a backtracking bait, a deep context
value through every writer, a jq value at `MAX_STEPS`, template nesting, and
the filter chain. Nothing aborted and nothing hung. Two real defects fell out
of it anyway, and neither would have been found by reading the diff.

**A nested class inside the Unicode fold emitted its own group.** The fold
writes one `(?i:…)` around the class; a nested class written through the same
entry point wrote a second one INSIDE it, and inside a character class `(`,
`?`, `i`, `:` and `)` are members rather than syntax. `(?i)[a[\p{L}]]` matched
`(`, `?`, `i`, `:` and `)` in egret and matches none of them in Java — **13
silent rows in a 63-probe sweep, introduced by this round's own diff.** The
group is now written once, at the top, and everything below it is plain text.
The 63 rows are in the fixture.

**A well-formed class 9 999 deep cost 10 s of CPU.** That depth is inside
`MAX_CLASS_DEPTH`, so it must work, and Java compiles it in milliseconds. Two
separate quadratics: `class_text` returned a `String` per level and the parent
copied it (fixed by writing in place, which also removed a 10x penalty under
`(?i)`, where every level compiled its own probe over the whole subtree), and
`regex-syntax` unions a nested class into its parent at every level. The
second is not egret's to fix, so nested classes that are **plain unions** are
now flattened — `[[[[a]]]]` is `[a]` — which removes the nesting the crate
would recurse over. A negation or an `&&` is not flattenable and is left
alone.

| depth | before | after |
|---:|---:|---:|
| 1 000 | 90 ms | 2 ms |
| 2 000 | 344 ms | 3 ms |
| 4 000 | 1 397 ms | 7 ms |
| 4 000 under `(?i)` | 14 064 ms | 8 ms |
| 9 999 | 10 140 ms | 19 ms |

**And one about the suite itself.** `MAX_STEPS` and `MAX_RUN` race by
construction, and two tests asserted which one wins. They passed thirty runs
of the lib binary alone and failed when `--test oracle` ran beside them —
which this round's corpus entries made heavier. Both now accept either bound
by name, which still fails on `unknown function`, on a generic failure, and on
a bound that stopped firing.


## What W2 Task 7's 156 new entries measured (read before touching a function)

Task 7 ported the **19** registered functions that reach no service —
`now`, `uuid`, `id`, `ksuid`, `nanoId`, `randomInt`, `randomPort`, `range`,
`fromJson`, `fromIon`, `yaml`, `dayOfMonth`, `dayOfWeek`, `hourOfDay`,
`monthOfYear`, `isWeekend`, `isPublicHoliday`, `isLastWorkingDay`,
`isDayWeekInMonth` — into `runners::functions`. The remaining 15 of the 34 are
Task 8's.

### The wire name is a `NAME` constant, and eight of them are not the file name

Read out of the artifact's bytecode (`javap -p -c -constants` on every class in
`io/kestra/core/runners/pebble/functions/`) and cross-checked against
`host.Oracle dump`. The two agree, at 34.

| wire name | class | note |
|---|---|---|
| `range` | `BoundedRangeFunction` | artifact-only; not in the checkout at all |
| `read` | `ReadFileFunction` | |
| `fileURI` | `FileURIFunction` | camel-cased `URI` |
| `id` | `IDFunction` | |
| `ksuid` | `KSUIDFunction` | |
| `uuid` | `UUIDFunction` | |
| `nanoId` | `NanoIDFunction` | lower-case `d` on the wire, upper-case in the class |
| `fetchContext` **and** `printContext` | `FetchContextFunction` | one class, two `put` calls — `NAME` and `ALIAS` |

The other 26 names match their class stem. **A name taken from a file name
gives `boundedRange`, `readFile`, `fileUri`, `nanoID` and one missing
`printContext`** — five functions nothing can call and one that silently is not
there, none of which any test would notice.

### `KestraFunction.getArgumentDefaults()` is documentation, not behaviour

It is Kestra's own interface method, not Pebble's, and nothing on the render
path reads it. `RandomIntFunction` declares `lower=0, upper=10` and
`{{ randomInt() }}` still throws `The 'randomIn()' function expects an argument
lower`. Measured. An absent argument is absent.

### `range` is the THIRD minijinja name this crate overrides, and the first global

`indent` and `string` are minijinja *filters* Task 4 overrode. `range` is a
*global*, and `Environment` keeps one map for globals, so `add_function`
replaces it silently. Derived from the lock file rather than from a list:
minijinja 2.24.0's `defaults::build_globals` inserts exactly four names under
the `builtins` feature — `range`, `dict`, `debug`, `namespace`. Of Task 7's
nineteen, `range` is the only collision.

The two disagree about the answer: `range(1, 3)` is `[1, 2]` in minijinja and
`[1,2,3]` in Kestra, minijinja's takes no named arguments, and it has no
character form.

**And an existing test was quietly relying on minijinja's.**
`runners::env::a_runaway_loop_runs_out_of_fuel` read `range(1000)`, which under
Kestra's signature is `start=1000` with no `end` — the test failed with an
argument error rather than exercising fuel. The same trap as `indent` and
`string`, seen from the caller's side.

### `BoundedRangeFunction`'s guard counts by the INCREMENT

`RenderLimits.ensureAtMost(|end - start| / |increment| + 1, 1000000, ...)`,
and it runs only when both bounds are numbers and the increment is not zero. So
`range(start=0, end=2000000, increment=2000000)` spans two million and produces
two elements; a port that bounded `|end - start|` refuses it.
`function-range-counts-by-increment` is that entry, and a mutation run confirms
it is what kills the wrong bound.

**Two shapes of Kestra's own loop do not terminate, and egret refuses both.**
Neither can have a golden.

- `Math.abs(end - start)` is Java `long` arithmetic and wraps:
  `range(start=Long.MIN_VALUE, end=Long.MAX_VALUE)` is `abs(-1)` = 1, the guard
  sees two elements, and the loop then runs 1.8 x 10^19 times. egret computes
  the count in `i128`.
- a character range's step is `(int)((long) i + increment)`, so an increment of
  2^32 leaves the loop variable unchanged. egret refuses it by name.

### The eight non-deterministic ones: what a shape check cannot see

`tests/oracle.rs` compares these for shape. A shape check sees a length. Four
of them can be pinned **by exact value through the differential** anyway,
because the arithmetic collapses:

- **`randomInt(lower=5, upper=6)` is `5`, always.** The expression is
  `(int)(Math.floor(Math.random() * (upper - lower)) + lower)`, so over a
  one-wide range there is one answer — which pins lower-inclusive and
  upper-**exclusive** by value where the shape check's `(1..10)` only states
  them. `lower=5, upper=5` is `5` too: an empty range is not an error.
- **`nanoId(length=6, alphabet='x')` is `xxxxxx`, always**, and `alphabet='xxx'`
  is too — a one-character alphabet is deterministic on *both* selection paths
  (a power-of-two alphabet is masked, anything else is rejection-sampled).
- **`uuid() | regexMatch(regex='…-7[0-9a-f]{3}-[89ab]…')` is `true`, always**,
  which pins the version nibble, the variant bits and `java.util.UUID`'s
  lower-case hex. `ksuid()`, `id()` and `nanoId()`'s alphabets are pinned the
  same way, and `randomPort() > 0` likewise.

**A regex over a random value is a deterministic boolean the EXACT comparator
can check.** That is a stronger instrument than the shape list, and it is worth
reaching for before adding a ninth id to `NON_DETERMINISTIC`.

What each one actually guarantees, read off the jars:

| | class | guarantee |
|---|---|---|
| `uuid` | `Generators.timeBasedEpochRandomGenerator()` | **UUIDv7**: 48 bits of Unix ms, version `7`, variant `10`, 74 random bits. The fully-random variant, so two in the same millisecond are unordered |
| `id` | `FriendlyId` -> `Base62.encode(BigInteger)` | a random **UUIDv4** as an unsigned 128-bit integer, base 62, alphabet `0-9A-Za-z`, **no padding** |
| `ksuid` | `KsuidGenerator(new SecureRandom())` | 4 big-endian bytes of `epochSecond - 1400000000` + 16 random, base 62, **padded to 27** (`PAD_TO_LENGTH`) |
| `nanoId` | `NanoIDFunction` | 21 characters from a 64-character alphabet, `0-9A-Za-z-_` in that order |
| `randomPort` | `new ServerSocket(0).getLocalPort()` | a real bind; the kernel picks, so it is in the host's ephemeral range and never a port in use |
| `randomInt` | `Math.random()` | `[lower, upper)`, as an `int` |
| `now` | `AbstractDate.format(ZonedDateTime.now(), ...)` | the same four arguments `date` takes, through the same formatter |

### `id()` is 21 or 22 characters, and the shape check said 22

`Base62.encode` pads nothing, so the digit count is `ceil(log62(value))`.
`62^21` is 4.37 x 10^37 against a 2^128 = 3.40 x 10^38 space, so **12.8% of ids
are 21 characters** and 0.2% are 20.

This is not a deduction. The golden file's own **previous** value for
`function-id` was `iUBruItK7qj9tMOMNVPAr` — 21 characters, minted by Kestra,
committed, and sitting under a shape check that read `value.len() == 22`. It
would have failed about one run in eight against a faithful port and passed
against one that padded. `tests/oracle.rs` now asserts the alphabet and the
ceiling, and `runners::functions::an_id_is_base62_and_is_not_always_22_characters`
draws ten thousand and asserts both lengths occur.

### The three round trips, end to end

W2 Task 6 round-tripped the **write** halves and found six values that do not
come back. Task 7 is the read half, and all six are now pinned with both halves
inside one expression:

| | |
|---|---|
| `{{ yaml(yaml=('42' \| yaml)) \| className }}` | `java.lang.Integer` — a string in, an integer out |
| `{{ yaml(yaml=('1.5' \| yaml)) \| className }}` | `java.lang.Double` |
| `{{ yaml(yaml=('1e5' \| yaml)) }}` | `100000.0` |
| `{{ yaml(yaml=('.inf' \| yaml)) }}` | **throws** `Malformed numeric value '.inf'` |
| `{{ fromIon(ion=(nothing \| toIon)) }}` | **throws** `Invalid ion: t is null in sink.next(t)` |

`toJson`/`fromJson` and `toIon`/`fromIon` are otherwise lossless, and the
value-shaped round trips (`fromJson(v | toJson)` and friends) all answer exactly
what `{{ v }}` does.

### The three readers do not agree about a null argument, or about trailing text

- **`fromJson(json=null)` returns null; `yaml(yaml=null)` throws.**
  `FromJsonFunction` has an explicit null branch and `YamlFunction` does not —
  `null instanceof String` is false, so it reaches
  `The 'yaml' function expects an argument 'yaml' with type string.` Two
  functions in one batch, two answers to the same input.
- **`fromJson('1 2')` is `1`, not an error.**
  `ObjectMapper.readValue(String, ...)` does not enable
  `FAIL_ON_TRAILING_TOKENS`, so it reads the first value and stops — the same
  fact `JqFilter`'s `readTree` branch records. `serde_json::from_str` is the
  opposite, so egret goes through a `StreamDeserializer` and does not call
  `end()`.
- **`fromIon` without `allRows` takes the FIRST value and drops the rest**
  (`findFirst().orElse(null)`), rather than failing on a multi-value stream.

### The `yaml()` reader's plain-scalar resolver is the one recorded divergence

SnakeYAML resolves the YAML 1.1 schema; `serde_norway` resolves the 1.2 core
schema. Measured through the harness:

| plain scalar | Kestra | egret |
|---|---|---|
| `yes`, `on`, `off` | `true` / `false` | the string |
| `y`, `n` | the string | the string (they agree) |
| `010` | `8` — a leading zero is octal | the string |
| `0o10` | the string | `8` |
| `0x10` | `16` | `16` (they agree) |
| `1_000` | `1000` | the string |
| `1e400` | `Infinity` | the string |
| `.inf`, `.nan` | **throws**, in Jackson's own numeric coercion | **refused**, with Jackson's sentence |

Only the last row is reproduced. **The first version of this section gave the
wrong reason for that** — it said "nothing else in either format produces a
non-finite `f64`". What is true, measured: the other candidate is a huge
exponent, and `serde_norway` leaves `1e400` a **string** rather than resolving
it to infinity, so the refusal is reached only by `.inf`/`.nan` — by luck of
the other resolver rather than by construction. `1e400` is its own recorded
row above, and `fromJson('1e400')` is a third: `serde_json` rejects it in the
parser where Jackson answers `Infinity`.

The message names the canonical spelling, so `.Inf` and `.NaN` are reported as
`.inf` and `.nan` where Kestra echoes the source; the raw text does not reach a
`visit_f64`.

The rest is recorded rather than fixed because **the fix is not available**:
reproducing it needs to know whether a scalar was *quoted*, and `serde_norway`
hands a visitor `"yes"` and `yes` identically — `visit_untagged_scalar` funnels
both into the same call. Re-resolving on the string alone would make
`a: "010"` answer `8`, which is a new silent wrong answer traded for an old one.
`function-yaml-yes-is-a-boolean-or-not` and `function-yaml-octal-looking` are
baselined with this reason. (The crate is called `serde_norway` because of
exactly this footgun.)

### `isPublicHoliday` is refused, not approximated

Kestra answers it out of `de.focus_shift.jollyday` **2.19.0**, a per-country,
per-subdivision XML holiday calendar for about seventy countries — pin it
alongside the JDK and the artifact if that version ever moves, because a
regenerated `function-ispublicholiday` would move with it. There is no such
database in this tree and no Rust crate carrying jollyday's data.

egret refuses **by name**, the same call `javafmt` makes for `z`, `v` and `B`,
whose text lives in a locale database. Supporting one country and refusing the
rest would be worse: an author cannot predict which. `function-ispublicholiday`
is baselined with that reason and
`runners::functions::is_public_holiday_refuses_rather_than_guessing` is the pin.

### `hourOfDay` is the only calendar function that does not use `parseLocalDate`

The other six go through `DateUtils.parseLocalDate`, whose cascade is
`LocalDate.parse` -> `ZonedDateTime.parse().toLocalDate()` ->
`LocalDateTime.parse().toLocalDate()`. `hourOfDay` is `ZonedDateTime.parse`
then `LocalDateTime.parse` and **never** `LocalDate.parse`, so `2024-01-06` is
`Text '2024-01-06' could not be parsed at index 10` there and fine in all six
siblings.

`getHour()` is also the hour of the **parsed offset**, not of UTC:
`2024-01-06T14:30:00+07:00` is `14`.

None of the seven reads `WeekFields`, so W2 Task 5's locale hazard does not
reach them: `isWeekend` compares the `SATURDAY`/`SUNDAY` enum constants
directly, `dayOfWeek` returns `DayOfWeek.name()`, and `isLastWorkingDay` reads
`DayOfWeek.valueOf` over an upper-cased argument. All English, no locale, on
both engines.

Two smaller facts, both measured:

- **`isLastWorkingDay`'s blank check is `isBlank()`, not `isEmpty()`**, so
  `workingDays='   '` is the default Monday-to-Friday set rather than an error,
  and the split is `[,\s]+` so `'saturday sunday'` works.
- **`FOURTH` is not `LAST`.** `firstInMonth(day).plusWeeks(3)` can leave the
  month entirely, so a five-occurrence month has a `FOURTH` and a `LAST` that
  are different days.

### The upstream typo this batch reproduces

All three of `RandomIntFunction`'s messages spell the function **`randomIn()`**,
missing the `t`: `In 'randomIn()' upper is less than lower`,
`The 'randomIn()' function expects an argument %s` and
`... of type Long.` Reproduced, for the same reason `LessThanExpression`'s
`comparisonPebb` is recorded: a flow author reading an egret error must read the
sentence Kestra would have shown them. Unlike `comparisonPebb`, this one **is**
reached by egret, so a version bump that fixed it would move three goldens and
break three tests.

### Ion is read by hand too, and three constructs are refused

`runners::ionfmt::read_ion` is the reader beside the writer. The writer is
hand-written because `ion-rs` cannot produce Kestra's text; the reader is
hand-written for a different reason worth stating — this workspace builds
offline against a vendored registry and `ion-rs` is not in it.

S-expressions, blobs, clobs and **timestamps** are refused by name rather than
approximated: none has a `minijinja::Value` that means the same thing, and
`write_ion` never emits one, so nothing egret writes can produce them. A
timestamp in particular would otherwise become a string, which reads back as a
different type with no error anywhere.

### W2 Task 7's attack sweep, and what it found

Per F5, the last step was to attack the batch with shapes it was not designed
for: 40 probes over the three readers (a million open brackets, a million
annotations, a million top-level values, unterminated strings, open comments, a
million-digit number, the billion-laughs alias bomb), `range` at and past both
its bounds, `nanoId` at its widest alphabet, `randomInt` at the `i64` extremes,
and a 100 000-letter `now` pattern. **Nothing aborted and nothing hung**; the
slowest is 4.2 s for a 6 MB Ion document with a million top-level values, which
is a linear parse Kestra also spends seconds on.

Three real defects fell out of it, and none would have been found by reading the
diff:

- **`randomInt` PANICKED.** `upper - lower` is Java `long` arithmetic that
  wraps; a plain Rust `-` is an overflow — a panic in a debug build, a silent
  wrap in a release one — reachable from
  `{{ randomInt(lower=-9223372036854775808, upper=9223372036854775807) }}`. A
  renderer panic is a mismatch in **every** oracle mode, and no corpus entry
  reached it. `wrapping_sub` is Kestra's arithmetic, and `as i32` is Java's
  `d2i`, which saturates the same way.
- **A wide map made every render QUADRATIC.** Every value writer in this crate
  walks a map as "enumerate the keys, then `get_item` each one" — minijinja's
  `Object` API — and `OrderedMap::get_value` was a linear `find`. Measured on
  the render path:

  | keys | `{{ v }}` before | after |
  |---:|---:|---:|
  | 1 000 | 8 ms | 5 ms |
  | 10 000 | 745 ms | 22 ms |
  | 40 000 | 7 370 ms | 94 ms |
  | 100 000 (through `yaml()`) | **43 723 ms** | 633 ms |

  **It was already there for a context object** — no Task 7 code involved — and
  the readers widened its reach to a *filter argument*. `OrderedMap` now builds
  a lookup index on first use, above a 32-entry threshold below which a scan is
  cheaper than hashing. The risk the index adds is a duplicate key answering
  the last occurrence where a scan answers the first; `or_insert` is what keeps
  them the same and
  `runners::env::a_wide_map_answers_exactly_what_the_narrow_one_does` exercises
  both sides of the threshold.
- **`nanoId`'s rejection loop was quadratic in `length`**, because the
  condition re-counted `out.chars()`. Bounded at 1000, so half a million wasted
  steps rather than a vector — fixed on sight.

### The mutation run, and the one equivalent mutant

Sixteen mutations of `runners::functions`, each built before it was believed
(a mutation that does not compile is not evidence) and each run against the lib
tests and the whole oracle target. **Twelve killed on the first pass, three
survived and are now killed, and one is equivalent:**

| survivor | why nothing killed it | what does now |
|---|---|---|
| always take the masked selection path | a one-character alphabet answers the same on both paths, and the default 64-character alphabet **is** a power of two, so no case could separate them | a three-character alphabet: masking is `byte & 2`, which yields 0 and 2 and never 1, so the middle letter disappears |
| `length > 1000` -> `>= 1000` | nothing sat on the boundary | `function-nanoid-length-at-the-max` |
| `alphabet.len() > 256` -> `>= 256` | as above | `function-nanoid-alphabet-at-the-max`, and `-past-the-max` for the other side |

The equivalent one is `isLastWorkingDay`'s `working.contains(date)` guard:
deleting it changes no answer, because the walk back from the last day of the
month always ends on a working day, so `last == date` already implies that
`date` is one. That is the implementation being redundant, not the tests being
weak — and Kestra's own code has the same shape. Recorded in the code so the
next reader does not re-derive it or weaken a test trying to kill it.

## W2 Task 7, fix round 1 — a review's 330 probes, and the property a report can name without pinning

A review wrote 330 probe expressions of its own and ran them through both
engines. Most of the task came back verified — the name table, the
`FetchContextFunction` double registration, `range` as the only minijinja
collision, the four exact-value pins (2 000 Java draws for the uuid nibbles,
5 000 for ksuid's length, no flake), and the `function-id` length finding, which
it re-measured at **200 000 Java draws: `{19: 9, 20: 409, 21: 25 274,
22: 174 308}`** — 12.8% at 21, exactly as the arithmetic predicts.

What it found is worth more than that, and it is one shape three times over.

### The knowledge was in the diff, in the other reader

**Both new readers lost a duplicate key's value, and the third reader in the
same commit got it right.** `LinkedHashMap.put` keeps a key's **first
position** and its **last value**; `functions::BoundedVisitor::visit_map`
pushed a second entry instead, and `OrderedMap::get_value` then answered the
first:

| | Kestra | egret, before |
|---|---|---|
| `fromJson('{"a":1,"b":2,"a":3}')` | `{"a":3,"b":2}` | `{"a":1,"b":2,"a":1}` |
| `fromJson('{"a":1,"a":2}').a` | `2` | `1` |
| the same, through `keys` | `["a"]` | `["a","a"]` |
| `yaml('a: 1\na: 2')` | `{"a":2}` | `{"a":1,"a":1}` |

The `2` was not mis-selected — it was **discarded and replaced by a second copy
of the `1`**, which then propagated through every writer.

`ionfmt`'s struct reader, in the same commit, did it correctly and said so in a
comment; `RenderContext::insert` has said so in its doc since W2 Task 3. Two
places in the crate already knew, and two new visitors did not. The rule now
lives in **one** place, `env::last_value_wins`, which `OrderedMap::new` runs —
so it is a crate-wide invariant rather than three bugs, and `RenderContext`'s
own deserialiser (the one map that does not go through `OrderedMap`) uses it
too.

### The third quadratic of the same shape, and the axis no row covers

`ionfmt`'s struct reader paid for its correctness with a linear scan per field:

| fields | `fromIon` before | after | `fromJson`, same width |
|---:|---:|---:|---:|
| 10 000 | 425 ms | **48 ms** | 29 ms |
| 40 000 | 5 858 ms | **227 ms** | 142 ms |
| 80 000 | **26 644 ms** | **620 ms** | (past the output cap) |

Kestra parses the 40 000-field document inside a 30-entry sweep that finished
in 2.3 s total, and an ~1.1 MB Ion string is exactly what `read('…ion')` output
looks like. Moving the de-duplication into `last_value_wins` removed the scan
and the quadratic together.

**This is the third quadratic of the same shape on this branch** — Task 6's
`rewrite` regions, Task 7's `OrderedMap` width, now this. Every one is
quadratic in the number of **siblings**, and the recursive-descent table is
organised by *nesting*, so no row of it covers the axis. Task 7's own attack
sweep probed a million top-level values, a million annotations and the alias
bomb, and never a wide struct.

> **For W3: when adding a construct, probe it wide as well as deep.** A
> thousand siblings is a different question from a thousand levels, and this
> branch has now paid for it three times.

### A "Kestra does" sentence written without measuring, and what it was holding up

`ionfmt`'s doc said *"Jackson's `IonParser` drops [annotations], so the value
they decorate is what comes back."* **It does not.** Measured:
`fromIon('a::1')` is the string `a::1` and `fromIon('[a::1, 2]')` throws. That
sentence was load-bearing for an annotation-skipping loop that silently
answered `1` and `[1,2]` — a C9 violation aimed at a Java class rather than a
Rust crate, which is the same mistake the `BoundedRangeFunction` note exists to
prevent.

Annotations are now refused by name. `function-fromion-annotation` is
baselined (Kestra answers a string) and
`function-fromion-annotation-in-a-list` is error-class (Kestra throws).

### Over-acceptance is the one divergence this crate's rules do not allow

A refusal is allowed and is recorded; a plausible wrong answer is not. Six more
of them, all measured, all now refusals:

> **The rule has exactly two ruled exceptions, and they are here rather than
> only in the section that earned them.** Nine rows on this branch are egret
> answering where Kestra throws, and both classes are ruled RECORDED:
>
> | class | rows | why it is not fixed |
> |---|---:|---|
> | the `java.util.HashMap` order reaching Kestra's **re-render loop** as `{#` | 3 | fixing it means porting `String.hashCode`'s bucket layout, which is squarely the copy-structure line |
> | a context `Integer` reaching a date-family filter | 6 | the split is **literal-vs-context provenance** for the same digits inside int32; no predicate over the *value* can recover it, and closing it would break `filter-date-epoch-seconds-literal` |
>
> They are counted out of `return-classes.txt`, `collection-pairs.txt` and
> `propagation-pairs.txt`, not carried forward. Anyone reading the heading
> above as an absolute is reading it without its exceptions; anyone adding a
> third exception is changing this rule and should say so here.

| | Kestra | egret, before |
|---|---|---|
| `fromIon('007')`, `'00'`, `'-007'` | throws — Ion forbids a leading zero | `7` |
| `fromIon('+5')`, `'+1.5'` | throws — Ion forbids a leading `+` | `5` |
| `fromIon('$0')` | throws — a reserved symbol ID | the string `$0` |
| `fromIon('[1,,2]')`, `'{,}'` | throws | `[1,2]`, `{}` |
| `fromIon('/* nope')` | throws — corrupt | **the empty string** |
| `nanoId(alphabet=<200 astral chars>)` | throws — 400 UTF-16 units | a four-character id |

The unterminated-comment row is the sharpest: `skip_ignorable` mapped a `/*`
with no `*/` to end-of-input, so a **truncated document rendered as nothing**.
A line comment is a different case and still works — `1//c` and `1 // c` are
both `1`, and a document that is only a comment is an empty stream, which is
`null` on both engines.

`nanoId`'s bound is `String.toCharArray().length`, i.e. **UTF-16 code units**,
so 128 astral characters is exactly 256 and is accepted while 200 is 400 and is
not. The 256 boundary was right for the BMP and wrong above it.

### Four shapes the first cut refused or mis-read, now correct

| | |
|---|---|
| `fromIon('1//c')` | a comment may abut a token; `/` ends one |
| `fromIon('-9223372036854775808')`, `'12345678901234567890'` | parsed as `i128`, narrowed to the `i64`/`u64`/`i128` that holds it — an `i64`-only parse refused both, while `fromJson` and `yaml` carried them |
| `fromIon('(1 2)')` | Kestra reads an s-expression as a **list**: `[1,2]`, `(a b)` is `["a","b"]`, `(+ 1 2)` is `["+",1,2]`. Ported, bounded by `MAX_VALUE_DEPTH` like the other two containers |
| `yaml('a: 1\n---\nb: 2')` | Jackson reads the **first** document; `serde_norway` refused a multi-document source |

And one that was answering where it should throw: `yaml('')`, `yaml('   ')` and
`yaml('# c')` deserialised to a **unit**, which renders as nothing, where
Jackson throws `No content to map due to end-of-input`. The question is now
asked of the source before the parser — every line blank or a comment — which
cannot mis-fire on `~` or on `a: null`, both of which stay `null` and `{}`.

### `isPublicHoliday`'s blank check is on the wrong argument

The `isBlank()` in the bytecode is on **`subDivision`**; `countryCode` is only
null-checked. Measured:
`isPublicHoliday(date='2024-01-01', countryCode='  ')` is `true` in Kestra, and
egret refused it with the wrong sentence. (It still refuses the function
itself, for the jollyday reason above; the argument guard is what was wrong.)

### A one-character sentence that error-class comparison could not see

`range`'s increment message is `The increment of the range function must be a
number ` — Pebble's constant carries a **trailing space**, proven by the
golden (`must be a number x`, not `numberx`). egret dropped it, and
`range_refuses_an_increment_that_cannot_advance_it` asserted egret's own wrong
sentence, so the corpus entry (error-class) and the paired test agreed with
each other and both with nothing. That is the exact failure mode the pairing
convention exists to prevent, and it survived because the pair was written
from the implementation rather than from the golden.

### Two properties the report NAMED as invisible to a shape check, and pinned with nothing

Both mutations built cleanly and both left the whole suite and the whole oracle
green:

- **`ksuid` without its timestamp prefix.** The module doc said the prefix "is
  what makes two ksuids sort in the order they were minted"; the test checked
  the length, the alphabet, and then `base62_bytes` on twenty **zero** bytes —
  the padding helper, never a real ksuid. It now decodes the value back and
  reads the field (a random `u32` lands in the window with probability 2 in
  10^9) and asserts monotonicity across a second.
- **`randomPort` faking the number.** Replacing the whole `TcpListener` block
  with `gen_range(49152..=65535)` left everything green; the build only warned
  about an unused import. The test asserted `1023 < port < 65536` and that two
  draws differed, which any RNG satisfies. It now holds 120 ports open and
  draws 4 000, asserting none is named: the ephemeral range is 16 384 wide, so
  a fake collides 29 times in expectation and survives with probability about
  `e^-29`. The real implementation collides with probability **zero**.

> **Naming a property is not pinning it.** Both of these were written down, in
> the report and in the module doc, as the thing a shape check cannot see —
> and then nothing was written that could fail. A sentence in a doc is exactly
> as strong as a sentence in `oracle/README.md`, which this file has already
> learned twice.

### What the fix round changed, counted

**18 of the 37 measured disagreements fixed, 17 recorded** (plus two rows that
are `isPublicHoliday`'s standing refusal). Ten of the recorded ones had no
corpus entry at all before this round and now do, which is why the ratchet went
**70 → 80 while the port got closer**: `function-fromion-annotation`, the three
extra members of the YAML plain-scalar class (`1_000`, `off`, `1e400`),
`function-fromjson-huge-exponent`, `function-yaml-alias`,
`function-yaml-tab-after-a-key`, `function-ispublicholiday-blank-country`, and
the two `Args` rows below.

### Recorded, not fixed: unknown and extra arguments are ignored

`dayOfWeek(date=…, timeZone='X')` throws `The following named argument does not
exist` in Pebble and answers here; a third positional argument throws
`The argument at position 2 is not allowed` and is ignored here. That is
`filters::Args`' behaviour and it predates Task 7 — the `date` filter has
always done it — but Task 7 widened the surface by nineteen functions, so it is
two corpus entries now rather than a sentence.

The other divergence that is not a refusal **was** an Ion decimal read as a
`Double`, and the sentence excusing it — "no round trip through this crate
reaches it, only hand-written Ion does" — was falsified by
`{{ fromIon('1d2') | toIon }}`, which is a round trip through this crate and
answered `100e0` against Kestra's `1d2`. Fixed in fix round 2; see below.

### The attack, re-run against the fix

The wide-struct axis, the all-duplicate worst case for `last_value_wins`, the
new s-expression descent at 200 000 open parens and 200 000 operators, 200 000
annotations, 200 000 blank and comment-only YAML lines, and the previous
round's whole sweep. **Nothing panicked, nothing hung, and the slowest row is
620 ms** for an 80 000-field Ion document that then hits the output cap. Six
mutations — the ksuid prefix, the randomPort fake, the duplicate-key rule, the
Ion leading zero, the unterminated comment, the empty YAML document — are each
killed by a named test.

## W2 Task 7, fix round 2 — one bug in three costumes, and a correction that inherited its error's method

A second review of 330 probes verified fix round 1 broadly — C1's de-duplication
agrees **98/98** across all three readers times nine ways of touching the
result, the two pre-dating `env.rs` visitors agree 11/11, the `OrderedMap`
index agrees **146/146** across its threshold, and there is no new quadratic
anywhere (80 000-field Ion 161 ms, a 100 000-key map through every writer
≤ 405 ms, 500 000 top-level values 407 ms).

It also found that fix round 1 had introduced thirteen regressions, and the
shape they share is the one this file keeps writing down.

### `fromIon` is LAZY, and six new refusals turned that into thirteen wrong answers

`FileSerde.readAll(…).findFirst()` is `take(1)`: **Kestra never parses past the
first value.** This reader parsed the whole document eagerly and then took
`rows[0]`, which was harmless while it accepted nearly everything. Measured
over fourteen documents:

| | Kestra | before round 1 | after round 1 |
|---|---|---|---|
| `fromIon('1 /*c')` | `1` | `1` | unterminated ion block comment |
| `fromIon('1 +5')` | `1` | `1` | ion forbids a leading `+` |
| `fromIon('1 a::1')` | `1` | `1` | does not read an ion annotation |
| `fromIon('1 null')` | `1` | `1` | `t is null in sink.next(t)` |
| `fromIon('[1,2] /*c')` | `[1,2]` | `[1,2]` | as above |

**The unterminated-comment fix was right about the defect and wrong about its
scope**: a truncated document really does render as nothing, but only when no
complete value precedes the comment. `/* nope` is an error; `1 /*c` is `1`.

`allRows=true` is `toList()` and **does** read the tail — every one of those
documents throws there. Both halves are now corpus entries.

> **Every entry written for round 1's six refusals was a document consisting
> only of the bad token**, so each passed the correct implementation and the
> broken one alike. The variant they were all one step away from is simply *a
> valid value in front*. That is the "one variant away" pattern from W2 Task 5's
> fix round 3, and it has now cost this package four rounds.

And the correct shape was **already in the same commit**: round 1's new
multi-document `yaml()` read is lazy — `yaml('a: 1\n---\nb:\tc')` is `{"a":1}`
on both engines even though the second document is a scanner error. One reader
in one commit was lazy and the other was eager. That is the second time this
task the knowledge was in the diff.

### The second unmeasured claim about annotations, in two rounds, in one file

Round 1 was raised *because* an `ionfmt` "Kestra does" sentence had been written
without running one ("Jackson's `IonParser` drops them"). The replacement
sentence — *"Kestra throws for one inside a container"* — is wrong in the same
way, and it went into three committed places. Measured:

| | Kestra | after round 1 |
|---|---|---|
| `fromIon('[a::1, 2]') \| length` | `2` | error |
| `fromIon('[a::1, 2]')[1]` | `2` | error |
| `fromIon('{f:a::1}') \| className` | `java.util.LinkedHashMap` | error |

> **The lesson is not about annotations. The correction inherited the method
> that produced the error.** Rewriting a claim that was wrong for lack of
> measurement is the moment to measure.

What Jackson actually does, across fifteen probed forms: an **annotated value**
is an opaque `IonValue` whose `toString()` is its Ion text, and its
**container** is an ordinary `ArrayList`/`LinkedHashMap`. Only *stringifying* a
container that holds one fails.

egret now carries an annotated value as its own **source text**. That gets the
container right, gets the bare render right (`a::1`, `a::b::1`, `a::{b:1}`,
`a::"s"`, `a::null.int`, `a::-1` — every probed form but two), and leaves four
narrow divergences in `baseline.txt` instead of poisoning a whole document for
one exotic element:

- `className` is `java.lang.String` against
  `com.amazon.ion.impl.lite.IonIntLite` — and this file's own rule excludes
  "an internal class name" from what a port copies, the same call
  `filter-classname-of-long` makes;
- `fromIon('a::1') == 'a::1'` is `true` here and `false` there;
- Ion's `toString()` **normalises** what it decorates: `a::'x'` is `a::x` and
  `a::0x10` is `a::16`. egret keeps the source text;
- a serialiser refuses an `IonValue` in Kestra and writes the text here.

### The ninth non-deterministic entry, which the instrument had predicted

Round 1's `function-fromion-annotation-in-a-list` had a **Jackson serialiser
artifact** for a golden, and it is not stable: on the same jar and the same
corpus, three fresh JVM runs gave `Document nesting depth (1001) exceeds the
maximum` once and `Direct self-reference leading to cycle` twice. It was
error-class, so the ratchet could not see it — but the regeneration gate would
have produced a permanent spurious diff line, and `tests/oracle.rs`'s own doc
says a ninth such id "showing up is visible as new information about the
corpus". It showed up and nobody looked.

**It is deleted rather than added to `NON_DETERMINISTIC`**, and replaced with
the three stable expressions above, which is what it should have been. The
whole-container stringification it stood for cannot be a corpus entry at all —
a golden that changes on regeneration is worse than none — so it is pinned by
`runners::functions::an_annotated_value_is_its_own_text_and_its_container_is_a_container`
instead.

### An Ion `d` is a decimal and an `e` is a float, and the difference is a Java class

Measured over fourteen literals, four ways each:

| | Kestra | before |
|---|---|---|
| `fromIon('1d2')` | `100`, `java.math.BigDecimal` | `100.0`, Double |
| `fromIon('1.50')` | `1.50` | `1.5` |
| `fromIon('-0.0')` | `0.0` (`BigDecimal` has no negative zero) | `-0.0` |
| `fromIon('1d2') \| toIon` | `1d2` | `100e0` |
| `fromIon('1E2')` | `100.0`, `java.lang.Double` | (agreed) |

An `f64` loses the scale. Ion decimals now go through
`filters::big_decimal`, and the sentence that excused the old behaviour —
*"no round trip through this crate reaches it, only hand-written Ion does"* —
is falsified by the last row, which **is** a round trip through this crate.

It exposed a pre-existing bug in the writer beside it: **a scale-0
`BigDecimal` needs Ion's `.`**, or it reads back as an integer.
`'1' | number(type='BIGDECIMAL') | toIon` is `1.` and this wrote `1`, on a path
that predates Task 7 entirely. `filter-number-bigdecimal-scale-zero-to-ion` is
the entry.

### Three more, from the same review

- **`$ion_1_01` is `UnsupportedIonVersionException`**, not the symbol
  `$ion_1_01`. One more over-acceptance, refused.
- **Blobs, clobs and timestamps were divergences recorded only in prose.** This
  file's own rule is that a divergence gets an entry because a sentence cannot
  fail, and round 1 applied that rule to `yaml` and not to these. All three are
  entries now, with what Kestra actually answers: a blob and a clob are
  `byte[]` (`[B`) and base64 on output, and a timestamp is a
  `java.time.LocalDate`.
- **`function-yaml-huge-exponent` was filed under the wrong cause.**
  `baseline.txt` had it with `1_000` and `off` as the YAML 1.1-vs-1.2
  plain-scalar class; it is not that class. `1e300` is a `Double` on **both**
  engines, and `1e400` stays a string here only because `serde_norway` declines
  to produce a non-finite `f64`. Two committed control files were giving one id
  two different causes.

### Three tests that passed the implementation that shipped

All three probed only whole-document sources, which is exactly the shape the
eager reader could not fail:

- `an_unterminated_ion_comment_is_corrupt_not_an_empty_document` probed only
  `/* nope`, while its own doc comment named the distinction ("a *truncated*
  document") its assertions did not draw;
- `the_ion_reader_refuses_an_annotation_rather_than_dropping_it` used four
  sources that were entirely the annotated shape, so it passed an
  implementation that refuses any document containing `::` anywhere;
- `the_ion_reader_refuses_what_ion_itself_forbids` had nine, all
  whole-document.

Each now carries the valid-value-in-front variant. **Fourteen corpus entries
fail against the build that shipped round 1**, verified by rebuilding it
(eager reader plus the annotation refusal) and running the ratchet against it.

### And the mode's own description was overstated

`error-class` comparison "is born matching, never enters `baseline.txt`, and can
never fail" — the last two clauses are false, and this file has said them since
W2 Task 4. **Five ids are in `error-class.txt` and in `baseline.txt`**, because
such an entry still fails when egret *succeeds* where Kestra threw. What is
true is narrower and is what the pairing fixture exists for: it cannot fail *for
the reason it was written*.

Also corrected: the `Args` divergence (an unknown named argument and an extra
positional one are ignored here and throw in Pebble) is **crate-wide**, not
`dayOfWeek`'s. Every one of the 35 filters and 19 functions has it; the two
entries are a sample standing for all of them, and `baseline.txt` now says so
rather than implying one function is the case.

## W2 Task 7, fix round 3 — proving a fix discriminates is not probing its blast radius

A third review verified round 2's laziness fix across thirteen refusal classes
with a valid value in front, plus adjacency, nulls with and without `allRows`,
and decimals inside containers: **every one matches**. Integrity is clean and
the annotation facts are all true. It also corrected this file's own count in
the safe direction — **25** entries fail against a rebuilt round 1, not
fourteen.

Both of its Criticals are in the decimal path, which was round 2's third fix
and the one its verification never probed off its own axis.

### The method point, which is the reason both got through

Round 2's evidence was: rebuild the previous build, check the new entries fail
against it. That is a good technique and it stays. But it proves the entries
**pin the fix**. It says nothing about whether the fix broke something **off
the axis of the fix** — and both Criticals are off-axis: classification is not
rendering, and arithmetic is not `className`.

> **When you change what a value IS, the blast radius is every operation the
> type takes part in, not the operations the entry exercises.** Round 2's
> decimal test checked render, `className`, `toIon` and `toJson` — exactly the
> four axes on which `BigNumber` is right — and nothing in the corpus put an
> operator on a decimal.

### C-A: routing Ion decimals through `BigNumber` broke every operator

`filters::BigNumber` is a `minijinja` **`Object`**, and minijinja has no numeric
protocol for one. Read out of the lock file's own source, not inferred:

- `value::ops::coerce` matches on `ValueRepr` alone; its fallback is
  `i128::try_from`, whose arms are `Bool`/`I64`/`U64`/`F64`/`I128`/`U128`.
  There is no object arm, so `+ - * / %` cannot reach one.
- `impl Ord for Value` begins `self.kind().cmp(&other.kind())` and returns
  early on a difference. `ValueKind::Plain` is declared **after**
  `ValueKind::Number`, so **an object is Greater than every number,
  unconditionally**.
- `Object::custom_cmp` does not help: `PartialEq` consults it only when both
  sides are objects of the same type, and `Ord`'s kind check fires first
  anyway.

So the shipped round 2 answered:

| | Kestra | round 1 | round 2 |
|---|---|---|---|
| `fromIon('1.5') + 1` | `2.5` | `2.5` | error |
| `fromIon('1.5') \| abs` | `1.5` | `1.5` | error |
| `fromIon('1.') > 1` | `false` | `false` | **`true`** |
| `fromIon('1.5') == 1.5` | `true` | `true` | `false` |

Kestra's `BigDecimal` is a first-class number — `1.5 * 2` is `3.00` with the
scale kept, and `(1.5 + 1) | className` is `java.math.BigDecimal`. egret cannot
have both that and working operators, so **the scale is what gives way**: Ion
decimals are `f64` again, and twelve entries record what that costs (`1d2` is
`100.0` not `100`, `1.50` is `1.5`, `className` is `java.lang.Double`,
`1d2 | toIon` is `100e0` not `1d2`, `1.5 * 2` is `3.0` not `3.00`).

A wrong spelling is recorded. A **silently wrong comparison** is not
acceptable, and `{{ fromIon(outputs.x.uri) }}` followed by arithmetic is the
ordinary case — Ion is the format Kestra task outputs are written in.

**The same hole is still on the `number(type='BIGDECIMAL')` path**, which
returns the same object. It predates Task 7 and it is unchanged — but it now
has two entries (`filter-number-bigdecimal-is-not-a-number-here`,
`filter-number-bigdecimal-comparison`) where before this round nothing in the
corpus could see it.

**2026-10-08, W9c Task 4 fix round 1 (the user's ruling: comparisons on exact
numbers compare exactly; option A).** The comparison half of that hole is
closed, on this path and on `kv()`'s, which returns the same object. minijinja
still cannot compare the object, so the operators no longer reach minijinja:
`runners::cmp_rewrite` reads the rewriter's output with minijinja's own grammar
and turns every `==`, `!=`, `<`, `<=`, `>`, `>=`, `in`, `not in` and chain into
a call to `runners::compare`, which compares a `BigDecimal`/`BigInteger`
against a number exactly and answers every other pair with minijinja's own
`PartialEq`/`Ord`. `filter-number-bigdecimal-comparison` is BANKED; the corpus
moved by exactly that one entry, which is the evidence that no other answer
changed. Arithmetic stays a named refusal, so
`filter-number-bigdecimal-is-not-a-number-here` stays baselined. A float is
compared by its exact binary value, where Pebble 4.1.2 compares any two
numbers by `doubleValue()` -- W9c spec §15 records the difference.
`fromIon` decimals are still `f64` (their scale is still what gave way); this
round does not revisit that.

**2026-10-29, package #5a Task A4 (spec D3; Q5 reverses the W9c ruling).** The
arithmetic half is closed too. Task A3 compiled `+ - * / %` to
`runners::arith`, so the operators no longer reach minijinja either, and A4
ported `bigDecimalOperation` there (`runners::arith::decimal`, on
`num-bigint`): either operand a `BigDecimal` makes both
`BigDecimal.valueOf(x.doubleValue())`, `add`/`subtract` exact,
`multiply`/`divide`/`remainder` at `DECIMAL128` with Java's preferred scale.
With nothing left that minijinja computes on the object, an Ion decimal is a
`BigNumber` again (`ionfmt::ion_number`) and `abs` keeps its scale. All
thirteen entries above BANKED. Still minijinja's: a region the compile passes
do not mirror (`{% do %}`, macro arguments) and unary minus (`-x`, which
Pebble computes as `-1 * x`), both of which refuse the object.

**2026-10-29, #5a fix wave (review M3, M8).** Two corrections to the paragraph
above. Unary minus is Pebble's since Task A8 (`kestra_mul(-1, x)`), and macro
and `call` regions cannot be reached at all (the `macros` feature is off, so
minijinja refuses them as unknown statements); only the arguments of a
`{% do f(..) %}`, which is not Pebble syntax and whose value is discarded,
keep minijinja's operators. And the id
`filter-number-bigdecimal-is-not-a-number-here` is now only a label: it names
the W9c ruling it was written under, but the entry is BANKED and its golden is
Kestra's computed `2.5`. The id is kept, not renamed, because it is cited in
the spec, the plan and the survey of #5 and in this file's history.

### C-B: an `e` exponent with a dot was classified as a decimal

Ion's rule is that `e`/`E` makes a **float regardless of the dot**. Round 2
tested `contains(['d','D']) || contains('.')` in that order, so `1.5E2` became a
decimal: `150` instead of `150.0`, `BigDecimal` instead of `Double`, `1.5d2`
instead of `150e0`, and `1.0E308` became a 309-digit integer.

> **The one-variant-away pattern, recurring inside the commit that fixed the
> previous instance.** Round 2's float side had three entries and a unit test,
> and every one of them used `1E2` or `1e0` — both **dotless**. "Fourteen
> literals, four ways each" was true and contained exactly two floats, both on
> the same side of the boundary that was wrong.

Both sides of that boundary are entries now, and so is the one comparison that
discriminates it: `fromIon('1.5') > 1` and `< 1` agree with the **broken**
build by luck (an object is Greater, and Kestra also says `true`), while
`fromIon('1.') > 1` is `false` in Kestra and was `true`. An entry on the wrong
side of a boundary agrees with the bug.

### Three more from the same review

- **The version-marker test was on a PREFIX**, so `$ion_1_`, `$ion_1_x`,
  `$ion_1_0abc` and `$ion_1_0_x` were refused — four ordinary symbols Kestra
  answers, traded for one refusal that is reachable without them. It is a whole
  **token** test now, and the four are entries.
- **The annotation-normalisation class is at least sixteen forms, and
  `baseline.txt` recorded two.** `IonValue.toString()` re-serialises
  canonically: radix (`a::0b101` is `a::5`), underscores (`a::1_000` is
  `a::1000`), `e`-exponents (`a::1e2` is `a::100e0`), uppercase `D`, `-0`,
  quoted annotation names (`'a'::1` is `a::1`), triple-quoted strings, and every
  run of whitespace and comments. **This is the third round running in which a
  sentence about annotations generalised past what was measured**, and this
  time it was in `baseline.txt`, justifying a divergence's recorded extent.
  Round 2's list of eleven "right" forms was right and had been chosen from
  spellings that were already canonical.
- **A unit test pinned an answer Kestra does not give.** `fromIon('a::[1]')`
  throws there; the test asserted it renders. The annotated-*container* case had
  never been probed — `a::{b:1}`, which does render, stood in for it in a
  fifteen-form table. Measured now: `a::[1]`, `a::[1,2]` and `a::(1 2)` throw,
  `a::[]` and `a::()` are `[]` with the annotation **dropped**, and `a::{}` and
  `a::{b:1}` keep it.

### Two writer facts, both pre-existing and both newly visible

- **Ion writes a decimal's UNSCALED digits and its scale**, where
  `BigDecimal.toString()` normalises the mantissa: `1.5E+2` is `15d1`, not
  `1.5d2`. The two agree only when the mantissa has no fraction — which is
  where both existing round-trip entries sat.
- **`String.valueOf(BigDecimal)` is `toString()`, not `toPlainString()`.**
  `'1E2' | number(type='BIGDECIMAL') | string` is `1E+2`; `kestra_to_string`
  fell through to `Display`, which is the plain string, and answered `100`.

And one recorded rather than fixed: `(Boolean) args.get("allRows")` is a
`ClassCastException` in Kestra for a non-boolean and a `NullPointerException`
for an explicit null, where `filters::Args` reads it as false.

### The evidence

Rebuilding round 2's decimal path — `filters::big_decimal` restored and the
`ionfmt` branch with it — fails **eleven corpus entries** and the unit test:
five on the operator axis (`-decimal-arithmetic`, `-decimal-division`,
`-decimal-abs`, `-decimal-equality`, `-scale-zero-decimal-greater-than`) and
six on the `e`-with-a-dot boundary (`-float-with-a-dot` and its `-class` and
`-round-trip`, `-float-negative-exponent`, `-float-at-the-top-of-a-double`,
`-float-unit-exponent-round-trip`). Four further mutations — the dot-first
classification, the prefix version marker, the normalised Ion mantissa and the
plain-string `String.valueOf` — are each killed by name.

## What W2 Task 8's 81 new entries measured (read before touching a function, a refusal, or the built-in surface)

Task 8 is the last task of W2. It took the **fifteen** remaining registered
functions plus the **four** the built engine does not register, and one filter
nobody had owned. The brief called all nineteen "service-backed, ported as
explicit refusals". Seven of them are not.

### Seven of the "service-backed" nineteen reach no service

`env`, `encrypt`, `decrypt`, `fetchContext`, `printContext`, `tasksWithState`
and `loopOutputs` **answer in the standalone harness**, with ordinary values in
`golden.json`, and had been sitting in `baseline.txt` as unported work — not in
`error-class.txt`. Reading the bytecode says why:

| function | what it actually reads |
|---|---|
| `env` | the `envs` **context variable**, never `System.getenv`. `EnvFunction.execute` is `context.getVariable("envs")` and a `Map.get` |
| `encrypt` / `decrypt` | `io.kestra.core.encryption.EncryptionService`, which is pure: `AES/GCM/NoPadding`, a base64 key **argument**, a 12-byte random IV prefixed to the ciphertext, a 128-bit tag, base64 out |
| `fetchContext` / `printContext` | the render context itself, collected off Pebble's global scopes |
| `tasksWithState` | the `tasks` context variable |
| `loopOutputs` | its own `outputs` argument |

So they are ported, and all seven baseline lines are banked. What they wait on
is W3 putting `envs` and `tasks` into the context — not a service.

### The other twelve, and the two different reasons they are absent

| group | why absent | what Pebble says |
|---|---|---|
| `secret`, `kv`, `read`, `fileURI`, `fileSize`, `fileExists`, `isFileEmpty`, `http` | **registered** with a `null` `Function` value: `Extension.getFunctions()` `put`s their `NAME` keys unguarded and Micronaut never fills the `@Inject` fields | `Function or Macro [secret] does not exist.` |
| `render`, `renderOnce`, `errorLogs`, `subflow` | **not registered at all**: `@Inject @Nullable`, guarded by `if (x != null)` | the same sentence |

Pebble's parser treats a null-valued map entry exactly as a missing key, so the
two groups are **one behaviour** from a flow author's seat, and egret gives them
one message. `function-render`, `-renderonce`, `-errorlogs` and `-subflow` are
new corpus entries recording that the built engine really does not carry them.

**A green oracle is not evidence a refusal works, and it never will be.** All
twelve are `compare: "error-class"`, which any failure satisfies — including
`unknown function: secret is unknown`, which is what egret said before this task
and which names neither the function nor anything a reader can act on. The eight
that were `unported:task-8` in `error-class-pairs.txt` are now
`test:the_twelve_refusals_name_themselves_and_their_owner`, and
`UNPORTED_ERROR_CLASS` in `tests/oracle.rs` is empty.

**W9a Task 4 (2026-10-01):** `read`, `fileSize`, `fileExists` and `isFileEmpty`
are implemented over a render's file scope, and a corpus render has none (no
running execution), so their four entries still fail -- by a sentence asserted in
`without_a_scope_the_four_are_refused_by_name_saying_they_need_an_execution`,
which is now their pairing. The test above is
`the_eight_refusals_name_themselves_and_their_owner`; `fileURI` names W9b and
`kv` W9c.

**W9b Task 4 (2026-10-05):** `fileURI` is implemented over namespace files and
left the refusals; a corpus render has no file scope, so `function-fileuri`
still fails, by the sentence the four's test asserts for it too, and is paired
with `without_a_scope_the_four_are_refused_by_name_saying_they_need_an_execution`.
The refusals' test is now `kv_still_names_w9c_and_the_refused_list_shrinks_to_seven`
(seven names; `kv` names W9c).

**W9c Task 4 (2026-10-07):** `kv` is implemented over the KV store and left the
refusals; a corpus render has no file scope, so `function-kv` still fails, by
the sentence `kv_without_a_scope_refuses_by_name` asserts, which is now its
pairing. The refusals' test is now `the_refused_list_is_six_and_kv_is_not_in_it`
(six names).

### `fetchContext`'s key order is `HashMap`'s, and that is a recorded divergence

Measured: `{"b":1,"a":2}` comes back `{"a":2,"b":1}` and
`{"zebra":1,"apple":2,"mango":3}` comes back in that same order. Neither sorted
nor insertion — `String.hashCode`'s bucket layout, because the function collects
into a `java.util.HashMap`. This file's standing ruling is that `HashMap` bucket
order is implementation *structure* and is recorded rather than ported (see
"What egret copies from upstream" and `filter-escapechar-map-two-keys`), and
minijinja's `known_variables` returns a `HashSet` anyway, so egret must impose
*some* order. It sorts. `function-fetchcontext-two-keys` matches because sorted
and hashed agree for `{a, b}`; `-three-keys` is where they part, and it is
baselined.

**`tasksWithState` is the same question with a different answer.** Its result
rows are `HashMap`s too — `state, value, taskId` for a three-key row and
`state, taskId` for a two-key one, both measured — but the key set is *fixed* at
three names, so **one emission order reproduces both shapes exactly**. That is
written down as a constant, not as a port of `String.hashCode`.

### Two things `fetchContext` sees that a reading of the name would not predict

- **inside a `{% for %}`, Pebble's loop scope IS global**, so `fetchContext()`
  returns the loop variable and the `loop` object as well. minijinja's
  `known_variables` agrees; the two `loop` objects do not
  (`first/last/length/index/revindex` against nine different fields), so
  `function-fetchcontext-in-a-loop` is baselined for the loop object's shape and
  not for this function's;
- a **null** context value is kept, not dropped. `{"x": null}` renders `{}` on
  both sides, and for the same reason: `JsonWriter` omits null map entries one
  layer later.

### `replace` was the fourth minijinja name resolving to the wrong answer

`io.kestra.core.runners.pebble.filters.ReplaceFilter` is in the jar with a
`regexp` argument and `Extension.getFilters()` does not register it, so the live
`replace` is **Pebble's own**: one argument, `replace_pairs`, a `Map`, applied as
a chain of literal `String.replace` calls. The proof is a probe rather than a
reading — `{{ s | replace({'a':'b'}, regexp=true) }}` answers
`The following named argument does not exist: regexp`, which only Pebble's class
can say.

egret's was **minijinja's** `(old, new)`, and the two are incompatible in both
directions: Kestra refuses `replace('a','b')`
(`The argument at position 2 is not allowed`) and minijinja refuses
`replace({'a':'b'})`. It is now overridden, the fourth after `indent`, `string`
and `range`.

Measured details worth keeping: the input goes through Java `toString`
(`[1, 2]`, `{a=1}`), pairs **chain** so `'ab' | replace({'a':'b','b':'c'})` is
`cc`, an empty key is Java's `String.replace("", x)` (`ab` → `-a-b-`, which
Rust's `str::replace` agrees with), a null *value* is an NPE, and a null *input*
is null.

### The name surface, and what W2 did not compare

`oracle/name-surface.txt` is new, and it is the fixture for the question the
four collisions above are instances of: **which names does each side have at
all?**

The whole engine — Pebble's `CoreExtension`, `EscaperExtension` and
`I18nExtension` plus Kestra's `Extension` — carries **63 filters, 37 functions
and 8 tests**. minijinja at this workspace's feature set carries 47 filters, 4
globals and 42 tests. Sixty-two of minijinja's names have **no counterpart in
the engine at all**, and egret answered every one of them where Kestra says
`Filter [count] does not exist.` Over-acceptance is the one divergence this
crate's rules do not allow — a flow that renders here and fails there fails in
production rather than at authoring time — so they are removed:

- 27 filters (`safe e count dictsort items lines d round int float attr min max
  list bool batch sum select reject selectattr rejectattr map groupby unique
  chain zip pprint`);
- 3 globals (`dict debug namespace`);
- 32 tests.

Six more minijinja test names (`==`, `!=`, `<`, `<=`, `>`, `>=`) need no
removal: minijinja's `is` operator parses an identifier, so they are unreachable
from template syntax either way. Pinned by a test, because "unreachable" written
in prose is not distinguishable from an oversight.

**What is NOT closed, and is stated rather than hidden.** The fixture is about
name *presence*, not semantics, and three sets are left open with an owner:

- **17 Pebble-core filter names both sides bind, running minijinja's semantics
  here and nobody's differential** — `abs capitalize default escape first format
  join last length lower reverse slice sort split title trim upper`. W2's
  batches were Kestra's 35 registered filters; Pebble's own 28 were never in
  one.
- **10 filters and 3 functions the engine has and egret does not** —
  `abbreviate base64decode base64encode merge nl2br numberformat raw rsort
  sha256 urlencode`, and `i18n max min`. Note `sha256`: Kestra's own
  `Md5Filter`/`Sha1Filter`/`Sha512Filter` each carry a copy-pasted "implements
  the 'sha256' filter" javadoc and register three names, and the fourth name
  comes from Pebble instead — which is why W2's filter batches correctly did not
  own it, and why egret does not have it.
- **the `is` surface.** The engine's eight tests are `defined empty even
  iterable json map null odd`; `json` is Kestra's own and the only one any W2
  task could have owned. egret keeps the four names that overlap and they are
  **not** semantically identical (Pebble's `even`/`odd` throw a
  `ClassCastException` on a non-number where minijinja answers `false`; Pebble's
  `iterable` is `false` for a String where minijinja's is `true`). Recorded as
  divergent rows.

`runners::env::the_built_in_surface_matches_the_engines` asserts every row by
**probing** `render_str`, in both directions. **Its first cut had a hole a
mutant found**: it listed only names one side or the other binds, so
re-registering a removed minijinja filter produced no row and no failure — the
exact over-acceptance the fixture existed to catch, invisible to the fixture.
Every removed default now has a `no`/`no` row with a reason.

### Two bounds Kestra does not have, both derived from a measured cost

Per the C7 ruling. Both were found by **attacking the new code**, and the second
was found by attacking the first — which is now the third time on this branch
that a fix's own blast radius carried the defect.

| construct | why it is unbounded upstream | bound |
|---|---|---|
| `encrypt` | base64 of `12 + n + 16` bytes is `4/3 (n + 28)`, so **nesting multiplies**. Measured on the shipped path: 30 levels is 672 KB in 6 ms, 48 levels is 1 686 ms, and the trend puts 60 levels at ~16 GB. `rewrite`'s nesting cap does not see a call chain, and the output cap sees only the last value, after every intermediate is allocated | the plaintext may not exceed `MAX_OUTPUT_BYTES`; a value over 1 MiB could not be rendered anyway |
| `replace` | Pebble's is an unguarded loop of `String.replace` with **two** growth axes: one pair can allocate `matches x replacement` inside a single call, and pairs *chain*, so a 25-entry map literal is `2^25` times its input | the predicted length is checked **before** each `replace`, against `MAX_OUTPUT_BYTES` |
| `replace`, again | **the growth check does not bound the pair count**, and a pair that *shrinks* the subject keeps `|out|` under the growth cap forever. The loop is `pairs x |out|`, and the pairs come out of a context map: 20 000 pairs over a 500 KB subject is 2 825 ms | a cumulative scan budget, `MAX_REPLACE_SCAN_BYTES` = 1 024 x `MAX_OUTPUT_BYTES` = 1 GiB, measured at 0.28 ms per MiB scanned, so about 290 ms |

Neither adds a row to "Every recursive descent over author-controlled input":
neither recurses. `render` and `renderOnce` — the only two functions in W2 that
*would* have — are **refused**, so no recursion was added at all.

### The tenth unstable golden

This file has said "**nine** ids are expected to differ on every regeneration".
It is **ten**: `function-fromion-annotation-of-a-list` alternates between
`Document nesting depth (1001) exceeds the maximum` and
`Direct self-reference leading to cycle` across runs, exactly as
`function-fromion-annotation-in-a-list` does. It is `error-class`, so only the
documentation text moves and nothing can fail for it — which is why it took a
regeneration diff to notice.

## W2 Task 8, fix round 1 — sixteen more names answering minijinja's question, and a function that hid the author's data

A review measured what the task's own report had asked about and left open.
Two findings and the rest of the seventeen, in the order they matter.

### `fetchContext` was deleting forty names from its own answer

`known_variables(true)` is the union of the context's names and the
**environment's**, and every `add_function` name is in the second set. The
first cut subtracted the globals **by name**, so a context key that shared a
name with a registered function vanished from the answer:

| context | Kestra | egret, before |
|---|---|---|
| `{"range":1,"a":2}` | `{"a":2,"range":1}` | `{"a":2}` |
| `{"secret":1,"a":2}` | `{"a":2,"secret":1}` | `{"a":2}` |

`{{ range }}` still rendered `1`, so the value was there and only this function
hid it — omission of data an author asked for by name, in a function shipped as
ported and verified. Eleven corpus entries now cover it.

**The test that replaces the name filter is an identity test.** `State::lookup`
returns the context's binding when there is one and the environment's global
otherwise, and `Value`'s `PartialEq` falls through to
`Object::is_same_object` — `Arc` identity — for two objects. So a name is the
environment's *only* when what `lookup` hands back **is the very global
object**. A JSON-derived context value can never be the same `Arc` as a
registered function, so this cannot confuse the two.

**And Kestra's own predicate is a different thing entirely**, which the first
cut did not port: `FetchContextFunction.lambda$execute$0` drops a whole
**scope** whose key set contains `_context`, and never filters by name — which
is how Pebble's own internal scope stays out of the answer. Measured:

| template | context | Kestra |
|---|---|---|
| `{{ fetchContext() }}` | `{"_context":1,"a":2}` | `{}` |
| `{% set y = 9 %}{{ fetchContext() }}` | the same | `{"y":9}` |

so "drop everything" would have been wrong in the one case the guard exists
for. `minijinja` carries no provenance on `known_variables`, so
`env::CONTEXT_SCOPE_NAMES` — a global whose name contains a space and is
therefore unreachable from template syntax — publishes the render context's own
top-level names for the function to read.

### 16 of the 17 shared Pebble-core filter names diverged

The task's report named this as the largest open item and asked what it cost.
It cost 52 divergences out of 104 differential probes: 43 agreed, 9 failed on
both sides, and **`abs` was the only one of the seventeen that was clean** —
and even `abs` turned out to have one.

Three were as bad as `replace` was, and two of those silently:

| filter | Kestra | egret, before |
|---|---|---|
| `'a.b.c' \| split('.')` | `[]` — the delimiter is a **Java regex** | `["a","b","c"]` |
| `'a\|b' \| split('\|')` | `["a","\|","b"]` | `["a","b"]` |
| `[3,1,2] \| slice(0,2)` | `[3,1]` — `(fromIndex, toIndex)` | an error |
| `[3,1,2] \| slice(1)` | `[1,2]` | `[[3,1,2]]` — minijinja's first argument is a *slice count* |
| `null \| default('D')` | `D` — null, blank and empty all count | *(empty)* |

and **the Python-spelling defect was back for a fifth time**, through
minijinja's own built-ins rather than through anything this crate wrote:
`{{ nothing | upper }}` rendered `NONE`, `lower` rendered `none`,
`capitalize`/`title`/`escape`/`trim` rendered `None`, and
`[1,null,2] | join('-')` rendered `1-None-2` where Kestra writes `1-null-2`.
`kestra_to_string` was made total in Task 3 and every filter *this crate wrote*
goes through it; these six never did, because egret never wrote them.

**Sixteen are now ported**, and the seventeenth is `format`, which is wrapped
rather than reimplemented. What each was wrong about:

| filter | what Pebble does that minijinja does not |
|---|---|
| `split` | `String.split` — a **regex**, with Java's limit rules and its zero-width-at-index-0 rule |
| `slice` | `(fromIndex, toIndex)`, `intValue()` truncation, and four distinct refusals |
| `default` | falls back on null, on a **blank** String and on an empty Collection or Map — not only on undefined |
| `capitalize` | title-cases the first non-whitespace character and **leaves the tail alone** |
| `title` | the same per word, and **only whitespace** delimits: `"a-b c'd"` is `A-b C'd` |
| `trim` | `String.trim()` — everything `<= U+0020` and nothing else — and it takes **no argument** |
| `length` | a String's **UTF-16** length, and its last branch is `0`, not an error: a number, a boolean and a null are all `0` |
| `upper` / `lower` | `toString()` first, so a map is `{A=1}` and a null is nothing |
| `join` | `String.valueOf` per element (`null`, `{a=1}`), a `(String)` separator, and a **String or Map input throws** |
| `sort` | `Collections.sort`, so a mixed list is a `ClassCastException` — including an `Integer` beside a `Long` |
| `reverse` | `(List)`, so a String throws |
| `first` / `last` | a Collection, so a **Map throws**; an empty Collection is `null` |
| `escape` | `&#39;` where minijinja writes `&#x27;`, and `toString()` first |
| `abs` | **`Math.abs`'s overflow**: `abs(Long.MIN_VALUE)` is `Long.MIN_VALUE` |

That last one joins the list of upstream defects egret deliberately
reproduces — it is defect **7**.

### What is deliberately NOT ported, and where it is recorded

- **Java's `String.format` conversion language.** `%,d`, `%2$s` and `%b` raise
  here and answer there. `minijinja::filters::format` was measured to agree
  with `java.util.Formatter` on `%s`, `%5s`, `%05d`, `%.2f`, `%x` and `%%`, so
  reimplementing the grammar would risk turning four honest errors into four
  silently wrong values. What *is* fixed is the null input, the non-String
  input and the null **argument**, which was the silent one.
- **`escape`'s `js`, `css` and `url_param` strategies.** Three character
  tables; egret refuses them by name.

Both are under-acceptance, which is the safe direction, and both now have a
baselined corpus entry — `filter-format-argument-index` and
`filter-escape-js-strategy`.

### `replace`'s map-literal order: the doc claimed a fidelity that does not hold

The first cut's doc said pairs are applied "in the map's iteration order" and
cited `'ab' | replace({'a':'b','b':'c'})` → `cc`. **That agrees by luck**: for
the key set `{a, b}`, Java's `HashMap` bucket order and minijinja's key-sorted
literal order coincide. They do not in general, and because the pairs chain the
value differs — `'ap' | replace({'p':'a','a':'x'})` is `xx` there and `xa`
here.

A map out of the **context** agrees, in both directions: Jackson's
`LinkedHashMap` against `OrderedMap`, both insertion order. That half is now a
test and a corpus entry (`filter-replace-context-map-order`) rather than a
claim. The literal half is recorded under the standing `HashMap` ruling.

### The `replace` budget was per call, and a loop multiplied it

Measured in release: one call exhausting the 1 GiB budget takes **1.48 s** —
not the ~290 ms the first cut's note claimed, and the arithmetic says why (each
pair does *two* full passes plus a full-length allocation, while the budget
charged `out.len()` once). A `{% for %}` whose per-iteration output is empty
keeps `MAX_OUTPUT_BYTES` satisfied for ever, and 20 / 80 / 320 iterations cost
267 ms / 1.05 s / **4.42 s** with zero bytes of output; `range()` caps at a
million, so one template reached hours.

The class — an expensive filter in a loop with no per-render work budget — is
older than this filter and still applies to `jq` and the regex filters. What
made it this filter's problem is that a per-call gigabyte raised the legal
per-iteration cost by about three orders of magnitude over anything else. The
budget is now **per render**, installed by `render_str` and reset on drop.

**And the first cut's adversarial sweep had 21 cases in it with no loop in any
of them**, which is why a per-call cap looked sufficient. The sweep has one now.

### The fixture was blind to a name with no row, and blind to semantics

Two holes, both closed:

- **Addition.** `the_built_in_surface_matches_the_engines` iterates the file's
  rows and probes egret for each; a filter registered under a brand-new name
  has no row, so nothing probes it and `rows == 177` is still satisfied.
  Measured: registering one left the suite green.
  `no_name_egret_binds_is_missing_from_the_surface_fixture` now **computes**
  what egret binds — functions exactly from `Environment::globals()`, filters
  and tests from minijinja's pinned defaults minus the removal lists, plus
  every `add_*` call scanned out of the committed source — and fails on a name
  with no row. (The scan is textual, so a doc comment containing a registration
  call adds a phantom name. That is not hypothetical: this comment used to
  spell the example out and the test failed on its own prose.)
- **Semantics.** The file's rule *forbade* a reason on a both-bind row, which
  is exactly where the `replace` class lives — so the fixture could not record
  a semantic divergence on a shared name. Rows now carry a `semantics` column
  naming a `corpus.json` id, with a **baselined id preferred** where the name
  has one, and the reason column is `-` exactly when that entry passes. A
  divergence on a shared name is therefore held by the ratchet.

And **minijinja is pinned `=2.24.0`, not `^2.24`**: a minor release that adds a
default filter would silently re-open the over-acceptance this fixture exists
to close, and minijinja exposes no filter or test registry for anything else to
notice with.

### What attacking the ports off their own axis found

Two classes, neither of them on the axis any of the sixteen fixes was aimed at.

- **Which Java collection a filter RETURNS decides whether the next filter
  accepts it, and egret has one sequence type.** `keys` and `values` hand back
  a `Set`/`Collection` and `flatten` an **immutable** list, so `sort` (which
  needs a mutable `List`), `reverse` (a `List`) and `slice` (a List, String or
  array) all refuse them there and answer here — `{{ m | keys | sort }}` is an
  error on Kestra. `first`, `last`, `join`, `length` and `default` take a
  `Collection` and agree, and `distinct`, `chunk`, `split`, `range` and
  `fromJson` return real mutable Lists, so the class is exactly three
  producers wide. **That count was wrong and was committed**: fix round 2
  falsified it with two probes — `split` returns a Java `String[]` and `chunk`
  an immutable list, and `distinct`, `chunk` and `flatten` cast to `List` on
  the consumer side. It is five producers by six consumers, fourteen baselined
  entries, with `filter-sort-of-a-real-list` pinning the other side.
  **The consumer count is wrong too, and this is the third time it was
  committed** — W2a enumerated the class instead of counting the pairs someone
  thought to cross, and it is five creators plus two propagators by NINE
  consumers, all fourteen now banked. W2a's own first cut said EIGHT and was
  corrected by its review, which makes this the FOURTH commit of a wrong count
  for the same class. See "What W2a's entries measured" at the end of this
  file.
- **The sixteen new names join the recorded "an undefined root variable"
  class.** `{{ nothere | upper }}` is a `RootAttributeNotFoundException` there
  and renders empty here, exactly as `filter-string-undefined-input` already
  records for the other 35. `length` is the one that answers a *value* (`0`)
  rather than rendering nothing, so it gets the entry.

### Counted

**117 of 1141**, from 105 of 1047. The count went **up**, and that is the
honest shape of this round: 110 measured divergences were fixed and **none of
them had a corpus entry before**, while the twelve entries added for what is
deliberately not fixed are new recorded divergences the ratchet now holds.

## W2 Task 8, fix round 2 — two regressions the round before introduced, and a premise that was false

A 350-case differential, run through the artifact **and both builds**, found
two Criticals. Both were **introduced by the round that was fixing
divergences**, which is the shape worth remembering: a port undertaken to close
a gap opened two that the default it replaced did not have.

### `sort` became worse than the minijinja default it replaced

The comparator went through `f64::try_from(..).total_cmp(..)`. That collapses
every integer past 2^53, and `sort_by` is stable, so a pair it could not tell
apart kept its **input** order:

| list | Kestra | the f64 comparator |
|---|---|---|
| `[9007199254740993, 9007199254740992]` | `…992, …993` | `…993, …992` |
| `[1234567890123456789, …788]` | `…788, …789` | `…789, …788` |
| `[9223372036854775807, …806]` | `…806, …807` | `…807, …806` |

**minijinja's own `sort` compares integers exactly, so egret agreed with Kestra
on all of these before the port.** Epoch nanoseconds, snowflake ids and
counters are ordinary flow data.

The comparator now branches on the element class — which `sort` has already
established is *one* class, since a mixed list throws — and integers go through
`i128`. What the entries could not see, and the reason this was invisible: the
three `sort` entries the port shipped with all exercise the **type check**, and
not one of them reaches the comparator. `filter-sort-past-two-to-the-53` is the
first that does.

### Sixteen filters swallowed an undefined variable

`is_java_null` folds `Undefined` into "Kestra's null". That is right for the
filters W2 Task 4 wrote — Pebble's `null` is a real literal, minijinja has no
such name, and six entries pin what Kestra answers for it — and its cost is
written down: egret cannot tell the literal from a **typo'd variable**.

Applying it at the *entry point* of a filter that previously refused an
undefined turns that cost into a regression, and fix round 1 did it to sixteen
names at once:

| expression | Kestra | pre-port | fix round 1 |
|---|---|---|---|
| `{{ nothere \| upper }}` | `RootAttributeNotFoundException` | an error | `""` |
| `{{ obj.missing \| length }}` | `AttributeNotFoundException` | an error | `"0"` |

28 measured cases over 14 names x {missing root, missing attribute}. In a flow
that turns a typo'd variable from a hard failure into an empty string — and
**converting a refusal into an answer is the divergence this crate's rules do
not allow**, which is the rule the same round cited to justify sixteen ports.

`runners::filters::strict_undefined` now runs first in each of them. What that
costs is recorded rather than hidden: `{{ null | upper }}` — the **literal** —
renders empty on Kestra and is an error here, because the literal and the typo
arrive identically. Same trade as `filter-string-undefined-input`, seen from
the other side, and it is the direction that fails loudly.

**One exception, and it is Pebble's own**: the filter-invocation expression
catches the attribute exception for `default` specifically, so
`{{ nothere | default('D') }}` is `D` on Kestra. Measured for a root name and
for an attribute, and `default` is the one ported filter with no guard.

### The `format` wrapper rested on a false premise

Fix round 1 wrapped `minijinja::filters::format` on the stated premise that the
two engines agreed on the six specifiers minijinja has. **Two of the six were
counter-examples, and all five differences were silently wrong values:**

| expression | Java | the wrapper |
|---|---|---|
| `'%x' \| format(-255)` | `ffffff01` | `-ff` |
| `'%.0f' \| format(0.5)` | `1` (HALF_UP) | `0` (half-to-even) |
| `'%.2f' \| format(1e300)` | the shortest repr, padded | 300 digits of the exact value |
| `'%s' \| format(true)` | `true` | `True` |
| `'%s' \| format({'a':1})` | `{a=1}` | `{"a": 1}` |

The last two are **the Python-spelling defect that round existed to kill**,
still live inside `format`, because the wrapper mapped a null argument and
passed every other one through untouched.

So the grammar is ported: `runners::javaformat` implements
`%[N$][flags][width][.precision]conversion` for `s S b B d o x X f e E % n`
with the flags `- + space 0 , ( #`, and **refuses every other conversion by
name** — `c`, `g`, `a`, `h` and the `t` family. Refusing is the safe direction;
a partial implementation would have traded four honest errors for four chances
at a different number. Measured against a 519-case grid: 491 agree and the 28
that do not are exactly the four refused conversions.

Three rules in it are not obvious and all three are measured:

- a **null argument** is the four characters `null` through the *string* path
  for every conversion but `b`/`B` — so `'%.0f' | format(null)` is the empty
  string, because precision truncates it;
- `%x`/`%X`/`%o` of a negative print two's complement **at the argument's own
  width**, and which width comes from `java_class_name`'s Integer/Long
  magnitude rule;
- `%f` rounds the **shortest decimal representation**, HALF_UP — not the exact
  binary value. That is why `1.005` is `1.01` and `1e300` is a 1 and 300 zeros.
  Rust's `{}` is also shortest-round-trip and positional, so it is the right
  digit source.

### The two word-boundary predicates were both wrong, in both directions

`capitalize` and `title` used Rust's `char::is_whitespace`.
`Character.isWhitespace` is **false** for the three non-breaking spaces
(U+00A0, U+2007, U+202F) and **true** for the four information separators
(U+001C..U+001F) — opposite on both sets, 16 measured cases. `trim` was right,
because `String.trim()` is a third rule (`<= U+0020`) and it had that one.

`java_title_case` was `to_uppercase()`, and the doc's dismissal of the
difference as "one character wide" was wrong twice: the Latin digraphs have a
**distinct** title case (`\u{1c6}` titles to `\u{1c5}`, not `\u{1c4}`), and the
Greek iota-subscript family upper-cases to *two* characters where
`toTitleCase(char)` returns a single precomposed one. Both are now a 35-entry
table **derived** from `oracle/fixtures/java-title-case.txt`, the JVM's own
`toTitleCase` map over the BMP, and
`title_case_matches_the_jvm_over_every_code_point_it_maps` walks all 1 149
entries — so the derivation cannot rot and a JDK bump fails a test rather than
moving a golden nobody reads.

### The collection class is five producers and six consumers

> **Superseded by W2a**, which is the third time this heading's number moved
> and not the last: the CONSUMER side is NINE, not six and not the eight W2a's
> own first cut committed, and all fourteen entries below are banked. See
> "What W2a's entries measured" at the end of this file. What is still right
> here is the count of producers that CREATE a divergent class, and the two
> probes.

Fix round 1 committed "three producers wide". Two probes falsify it:

```
'a,b,c' | split(',') | reverse    ClassCastException ([Ljava.lang.String;)
l | chunk(2) | reverse            UnsupportedOperationException
```

`SplitFilter` returns a Java **array** and `ChunkFilter` an **immutable** list.
And `distinct`, `chunk` and `flatten` — W2 Task 4's, not this batch's — cast to
`List` too, so the consumer side is six. Producers whose result is not a
mutable `List`: `keys`, `values`, `flatten`, `chunk`, `split`. Ten more
baselined entries; `filter-sort-of-a-real-list` still pins the other side. Two
of the pairs were **regressions** rather than pre-existing —
`keys | slice` and `values | slice` agreed before fix round 1, because
minijinja's `slice` errored on them for its own reasons.

### Two holes in the C1 fix, and one of them was reachable from data

- The `_context` guard read only the render context's own names.
  `FetchContextFunction` drops **any** scope whose keys contain `_context`:
  `{% set _context = 1 %}{{ fetchContext() }}` is `{"a":2}` there and kept the
  key here, and the `{% for %}` case additionally kept `loop`. There are two
  scope groups in egret — the context's and everything a `{% set %}` or
  `{% for %}` bound — and both are now dropped independently.
- **The sentinel was reachable from data.** `CONTEXT_SCOPE_NAMES` was the one
  global whose value is a `Seq`, and `Seq`'s `PartialEq` is *structural* rather
  than `is_same_object` — so a context key whose value equalled the sentinel
  was silently deleted from the answer. Measured:
  `{"egret context scope": ["egret context scope"]}` renders on Kestra and
  yielded `{}` here. It is an `ObjectRepr::Plain` object now, which no JSON
  context can produce, so the identity test is exact rather than nearly exact.

### The fixture's own rule was not enforced

`name-surface.txt`'s header has said since fix round 1 that a **baselined id is
preferred** "so the row surfaces a known divergence rather than hiding it
behind a passing sibling". Nothing checked it: swapping the `sort` row from
`filter-sort-of-keys` to the passing `filter-sort`, reason `-`, survived — which
is precisely the hiding the rule forbids. `the_built_in_surface_matches_the_engines`
now computes each row's id family and fails when a baselined member exists and
the row names something else.

### Counted, and the honest shape of it

**130 of 1203**, from 117 of 1141. Two entries were **banked**
(`filter-format-argument-index`, which the ported formatter now answers, and
`filter-length-of-an-undefined`, which the strict guard now refuses), and
fifteen recorded. The two Criticals were **regressions**, so closing them moved
110-odd cases back to agreeing without moving the count at all — they had no
entries when they broke, which is the finding underneath both of them: the
round that introduced them added 94 entries and not one covered the axis it
broke.

**And this round nearly repeated it.** Two of its own mutations —
reverting the `_context` local-scope guard, and making the sentinel a `Seq`
again — **survived on the first attempt**, because the behaviour had been fixed
without an entry. Three entries later they fail by name. A fix with no entry is
a fix the ratchet cannot keep.

## W2 Task 8, fix round 3 — the third time a fix reintroduced its own criticism

Scoped to `javaformat` and the title-case fallback. Both round-2 Criticals held
across 291 probes; these are the defects the *fix* for them shipped.

### The pattern, because it is the third instance

`parse_spec` accepted **any flag before any conversion** — no per-conversion
set, no duplicate check, no mutual exclusion, no width requirement. Over a
3 994-case grid of flag x conversion x width x argument, **1 748 cases Kestra
refuses were answered**, and **765 of those were a refusal the same round
deleted**, because the wrapper it replaced had refused them for its own
reasons.

`runners::filters::strict_undefined`'s doc, written in that round, says:
*"converting a refusal into an answer is the one divergence this crate's rules
do not allow."*

That is the third time on this branch: after two mutations surviving because
two behaviours were fixed and neither entered, which the report itself named as
"the criticism you levelled at round 1, reproduced inside the round fixing it".
**When a sentence like that is written, the next action is to grep the same
diff for the shape it forbids** — not to trust that writing it down was the
work.

### The flag grammar, measured

`LEGAL_FLAGS` and `validate` carry Java's rules, and every one is read off the
grid rather than out of memory:

| rule | what Java raises |
|---|---|
| per-conversion legal flag set | `FormatFlagsConversionMismatch` (761 cases) |
| a repeated flag, checked **first** | `DuplicateFormatFlags` (432) |
| `+` with ` `, `-` with `0`, any flag on `%`/`n` | `IllegalFormatFlags` (285) |
| `-` or `0` without a width | `MissingFormatWidth` (160) |
| a width on `%n` | `IllegalFormatWidth` (90) |
| a precision on `d o x X %` or `n` | `IllegalFormatPrecision` (12) |

The order matters and is measured: `%--s` is a **duplicate** and not a missing
width, and `%+ s` is a **mismatch** (because `+` is not legal for `s`) where
`%+ d` is an exclusion.

**Three flags are checked at PRINT time, not parse time**, and that distinction
is only visible through a null: `+`, ` ` and `(` on `%o`/`%x`/`%X` live inside
Java's `printInteger`, which a null argument never reaches because a null takes
the string path first. So `'%+x' | format(null)` is `null` and
`'%+x' | format(42)` raises. 27 cases turned on it.

### `%b` was two arms where Java has three

`if null { "false" } else { "true" }` — the middle arm, **a `Boolean` gives its
own value**, was missing, so `'%b' | format(false)` rendered `true` and `%B`
rendered `TRUE`. The module's only `%b` corpus entry passed a **null**: the one
argument for which two arms are right.

### `format`'s arguments escaped the round's own undefined guard

`is_java_null` folds `Undefined`, and the argument path called it — so
`{{ '%s' | format(nothere) }}` rendered the four characters `null` where Kestra
raises. `strict_undefined` had been applied to the seventeen filter *inputs*
and to nothing else, in the round whose premise was that this is not allowed.

### Four more, all measured

- **`%d`/`%x` past `i64::MAX` was refused** where Kestra answers.
  `java_class_name` already says `BigInteger` there and `sort`'s comparator
  already routes those through `i128`; the formatter was the only consumer that
  narrowed. A `BigInteger` prints sign-and-magnitude rather than a two's
  complement, because it has no fixed width.
- **`%#012x` put the zero fill in front of the `0x`** (`000xffffff01` against
  `0x00ffffff01`), and `%(012d` put it outside the parentheses. `pad` skipped
  only a leading sign; it now takes the position to fill after.
- **Width and precision counted `char`s where Java counts UTF-16 code units.**
  The same branch `filters::first` documents. A precision that would split a
  supplementary character produces a lone surrogate in Java, which Rust cannot
  hold — egret stops one short, recorded in the module doc, and it can have no
  corpus entry for the same reason `first`'s cannot.
- **`'%tY' | format(null)` was `nullY`**: the null arm sat in front of the
  refusal and `parse_spec` consumed only one of the family's two characters.
  The refusal comes first now and the family is parsed before it is refused.

### `capitalize`/`title` were wrong on 50 code points, and the guard could not fail

`JAVA_TITLE_CASE` was derived from a fixture dumped as *"every code point whose
title case is not itself"* — which is blind **by construction** to a code point
whose title case IS itself while its uppercase is not. Those took the uppercase
fallback: the four already-title-case digraphs (so the half of the family round
2 fixed was `\u{1c6}` to `\u{1c5}`, and the half it broke was `\u{1c5}` to
itself) plus the whole Georgian Mkhedruli block, 46 code points.

**And the test guarding the derivation asserted the implementation's own
fallback arm back at itself** — `java_title_case(c) == c.to_uppercase()…` is
literally what the function does when the table misses, so it was unfailable
for exactly the inputs where the bug lived. The fixture is the **union** of both
maps now (1 199 rows), the table is 85 entries, and the identity half asserts
the fixture's `from == to` rows — the 50 the old dump could not contain.

### And the module had no unit tests at all

476 lines of a foreign grammar, every assertion in 21 corpus entries, none of
which carried an illegal flag, a `#` with a width, a precision on a string, a
supplementary character or an argument above `i64::MAX`. That is why five
defects shipped green. It has **eight** now, one of them the grid itself — and
the grid pins the accept-set as an **exact count** (455 refusals, 229
acceptances) rather than a threshold, because a threshold lets it widen.

### Counted

**128 of 1234** (this said 131; counted out of `baseline.txt` by W2b, which
found 128 ids and a test printing 128), from 130 of 1203. Thirty-one entries,
one new recorded divergence (`%t` with a null, which Kestra answers through its string path).
The 1 748 over-acceptances the grid found had **no entries at all**, which is
the same shape as both of round 2's Criticals and the reason the count barely
moves when a large class is closed.

## What W2b's 44 new entries measured (read before touching `rewrite`'s vocabulary)

W2b is the `null` **literal**. It is a follow-on rather than a fix round
because nothing failed first: **the corpus had no entry for a bare `{{ null }}`
anywhere**, so the ratchet was blind to the whole class. The entries went in
first and 23 of the 44 failed against the pre-fix build; the count is the
deliverable, not the fix.

### Pebble's literal is FOUR spellings, and the rule is not case-insensitivity

Measured through the harness against `io.kestra:core:2.0.0-rc13`, one probe per
spelling, `{{ x }}` and `{{ x == none }}`:

| spelling | Kestra |
|---|---|
| `null`, `NULL`, `none`, `NONE` | renders empty; `== none` is `true` |
| `None`, `Null`, `nUll`, `NoNe`, `nulL` | `RootAttributeNotFoundException` — a **variable** |

The booleans split the same way (`true`/`TRUE`/`false`/`FALSE` are literals,
`True` is a variable), which is what makes "all-lower or all-upper, nothing in
between" the rule rather than an artifact of the five negative probes chosen.
`runners::rewrite::is_null_literal_spelling` is the `matches!`, and
`runners::filters::tests::the_null_literal_is_exactly_four_spellings` is what
fails if it is ever widened — measured by widening it to
`eq_ignore_ascii_case`, which killed that test **and**
`null-literal-mixed-case-is-not-the-literal` by name.

**`none` is both an input and the target**, so the rename is a no-op for it.
That is deliberate: `{{ none }}` already agreed and is one of the boundary
entries.

### minijinja's vocabulary is WIDER, and no rewrite of a NAME can reach it

The other direction, and W2b **recorded** it rather than fixing it: minijinja's
own lexer binds `None`, `True` and `False` — Python's spellings — as constants.
Pebble does not, so `{{ None }}` renders empty here and raises there. This is an
**over-acceptance**, the divergence this crate's rules do not allow, and it is
not reachable from `runners::rewrite`: the rewriter emits minijinja *source*,
and by the time those words are read they are keywords of the lexer it feeds,
not names. Closing it means a different lever (an environment-level rejection,
or a rewrite to a name that cannot resolve — which risks colliding with a real
variable). `null-literal-python-spelling-is-not-the-literal` and
`boolean-literal-python-spelling-is-not-the-literal` are the entries, both in
`baseline.txt` **and** `error-class.txt` paired `baselined`, so neither is a
free pass.

### The attribute guard has to skip whitespace, and that is measured

`{{ o . null }}` renders `1` on the artifact — Pebble's lexer allows whitespace
around the attribute dot. A guard that looked only at the immediately preceding
token would translate it, and `o . none` is a **parse error** in minijinja
(`none` is a literal there, and a literal is not an attribute name), so the
failure would be loud but wrong. `null-is-an-attribute-name-across-whitespace`
is the entry. Both halves of the guard were mutated: removing it entirely
failed all three attribute entries, and removing only the whitespace skip
failed exactly that one.

### The `~` OPERATOR is unported, and W2b is the first thing that ever looked

Translating the literal is **not enough** for `{{ 'a' ~ null ~ 'b' }}`. After
the rewrite the operand is a real `None`, and minijinja's `~` then writes it
through `Display` — Python's spelling — so egret answers `aNoneb` where Kestra
answers `ab`. This is the same stringification trap this file records three
times for filters (`kestra_to_string` is the only correct route, and the crate
"has now learned that four times"), found for the first time in an **operator**,
where a filter's fix cannot reach it.

The corpus had **zero** entries containing `~` before W2b, so the two engines'
concatenation had never been compared at all. Four value classes probed, three
diverge:

| expression | Kestra | egret |
|---|---|---|
| `'a' ~ null` | `a` | `aNone` |
| `'a' ~ true` | `atrue` | `aTrue` |
| `'a' ~ m` (a map) | `a{a=1}` | `a{"a": 1}` |
| `'a' ~ [1,2]` | `a[1, 2]` | `a[1, 2]` — agrees, and it is the boundary |

**And W2b reversed the direction of this failure**, which "unported operator"
undersells: `{{ 'a' ~ null ~ 'b' }}` was an *error* at `4498b48b` (the literal
was an undefined and minijinja's strict mode raised) and now answers `aNoneb`.
Loud failure to silently wrong value is the worse mode. The entry is compared
**exact**, not error-class, so the ratchet holds the value rather than merely
the fact that something happened.

Closing this means teaching `runners::rewrite` to emit a function call for `~`,
the way it already does for `??`, `???` and `isIn`, and that needs Pebble's `~`
**precedence** measured against the rest of the table first. That is a task, not
a line, so W2b recorded four entries (`null-literal-concatenated`,
`concat-null-from-the-context`, `concat-boolean`, `concat-map`) and left them
baselined. **A reader planning that work should start from the precedence
table, not from these four values.**

### `is_java_null` narrowed to `is_none()`, and the mutation SURVIVED

The narrowing is the point of the task: once the literal is a `None` and a typo
is an `Undefined`, the predicate can ask the narrow question, and the sixteen
`{{ null | F }}` cases and the seventeen `{{ nothere | F }}` cases hold in the
same run — which W2 Task 8 could only ever have one of.

**And restoring the old body left the whole crate green.** 412 lib tests, all
ten oracle tests, 131 of 1278 exactly at baseline. The reason is structural: the
seventeen filters that pin the undefined half run `strict_undefined` *before*
this predicate, so an undefined never reaches it there, and the filters that do
reach it had no entry. A change that is right and that nothing can see is an
equivalent mutant. The sweep below is what *found* the observers; what was
written from it is a **stronger assertion**, and calling it "an observer, not a
stronger assertion" undersold it. `assert!(!is_java_null(&MjValue::UNDEFINED))`
is the predicate's contract stated directly, which no alternative
implementation of the sixteen filters can satisfy by accident, and each of the
sixteen is paired with a `render(...)` of `{{ null | F }}` that must SUCCEED --
so a lazy "make everything refuse" fails it in the other direction.

The observer is a 105-case sweep over all **35** registered filters x {null
literal, missing root, missing attribute}, run against the pre-change build and
the post-change build with the same Java goldens:

| | undefined cases answering where Kestra raises |
|---|---:|
| before the narrowing | **70 of 70** |
| after | **36 of 70** |

**Thirty-four** moved from "answers" to "refuses" and none moved the other way.
(This said 38 and thirty-two in its first version, and the paragraph below it
listing eighteen names contradicted its own table — 18 x 2 = 36, not 38. The 38
was measured on a build that did not yet carry `string`'s guard, which the same
section says closes here. Re-measured on the committed tree.)
`runners::filters::tests::narrowing_java_null_is_what_makes_sixteen_more_filters_refuse_a_typo`
pins sixteen of them plus the predicate's own contract, and it fails on the
restored body naming `chunk`.

### Nineteen filters answered a typo'd variable, and eighteen still do

That sweep is the finding W2b did not go looking for. `string` was the
nineteenth and closes here — `oracle/baseline.txt` named the rewriter's owner as
the one to fix it, so `filter-string-undefined-input` is **banked**, moved to
`error-class.txt` (its golden is a Java exception string) and paired with a test
that asserts both halves.

The other eighteen still answer where Kestra raises: `className`, `endsWith`,
`escapeChar`, `indent`, `jq`, `nindent`, `regexExtract`, `regexMatch`,
`regexReplace`, `slugify`, `startsWith`, `substringAfter`,
`substringAfterLast`, `substringBefore`, `substringBeforeLast`, `toIon`,
`toJson`, `yaml`. Each reaches its answer by a route that does not go through
`is_java_null`, so each needs its own `strict_undefined` call the way W2 Task
8's sixteen did. **Three of them changed their wrong answer rather than keeping
it** — `className` `<>` to `<null>`, `jq` `<>` to `<[null]>`, `yaml` `<null>` to
`<null\n>` — because an `Undefined` now reaches code that used to be short-cut
by the null branch. All three are still wrong in the same direction (an answer
where Kestra raises), none is newly wrong, and none has an entry; saying only
"34 changed class" hid that anything else moved at all. **They are a declared debt with no corpus entry**, because 36
`error-class` entries would be 36 free passes for work not done — the ratio this
file already flags. The sweep above is how to re-measure them.

### `default` was the one filter that needed the fold

Narrowing the predicate turned `filter-default-undefined-root` and
`-undefined-attribute` red, and the ratchet is what said so. Pebble's
filter-invocation expression catches the attribute exception for `default`
specifically, so `{{ nothere | default('D') }}` is `D` on Kestra where every
other filter raises. `default` now spells `value.is_undefined()` out at its own
call site rather than making thirty-four other filters share a predicate that
is only right there.

### `abs` is why it is sixteen and not seventeen

Seventeen filters run `strict_undefined`. Only **sixteen** `{{ null | F }}`
cases render on Kestra: `abs` refuses with `Can not pass null value to "abs"
filter.`, measured. egret already refused correctly, so nothing was ported —
but before W2b it refused with `strict_undefined`'s *undefined* message, so the
entry would have been born matching for the wrong reason.
`filter-abs-literal-null` is `error-class` and
`the_null_literal_and_a_typo_get_different_refusals` is what makes it
answerable: it asserts the literal names the **null** and a typo names the
**undefined**, which is only possible after the rewriter's translation.

### Counted

**131 of 1278**, from **128** of 1234 -- and the 128 is itself a correction:
the two places this file said 131 were both wrong, `baseline.txt` held 128 ids
and the pre-W2b run printed 128. Forty-four entries. BANKED three
(`filter-upper-literal-null`, `filter-length-literal-null`,
`filter-string-undefined-input`) and RECORDED six (four for the unported `~`
operator, two for minijinja's wider literal vocabulary). Thirty-eight of the
forty-four new entries pass, and **28 of the 44 failed against the pre-fix
build** -- which is the number Step 1 exists to produce, so it is worth saying
exactly how it was measured and what it was before.

It said **23**, which was the count of a sub-measurement: 37 of the 44 entries
existed when the pre-fix ratchet was run, 23 of those failed, and the remaining
7 (added later, after the artifact probes found the spelling boundary and the
`~` divergence) were never folded in. Five of those seven fail as well.

The honest measurement is one command on a `4498b48b` worktree carrying the new
`corpus.json`, `golden.json` and the two error-class files but **its own**
`baseline.txt`, so that "regression" means exactly "a new entry that fails on
the pre-fix build" with no arithmetic:

```
oracle: 161 of 1288 entries mismatch (baseline: 128)
REGRESSION: 33
```

161 = 128 + 33 with no `PROGRESS`/`STALE` arm firing, and **zero** of the 33 is
a pre-existing id. 28 of them are W2b's 44; the other 5 are the fix round's 10.
The 16 that pass at the parent are the declared boundary (`{{ o.null }}` and
its two siblings, the two string literals, the two longer names, the quoted map
key, `{{ none }}`), plus `{{ [null,1] }}`, `{{ {'a':null} }}`,
`{{ null ?? 'f' }}`, `{{ fromJson(json=null) }}`, `concat-list`,
`null-literal-mixed-case-is-not-the-literal` and `filter-abs-literal-null` --
every one of which is an entry that is supposed to agree.

Note what the flat count hides, which is the same shape this file records for
W2 Task 8's fix round: the sixteen `{{ null | F }}` cases that now bank were
**fourteen entries that did not exist** plus two that did, and the thirty-two
undefined cases the narrowing fixed have **no entries at all**.

## W2b, fix round — the class one construct over, and three numbers that were wrong

### `null` in a BOOLEAN context, and the direction of the change

W2b routed the literal onto minijinja's `none`. That is right in every **value**
position and wrong in every **boolean** one, because Pebble refuses a null there
and minijinja treats it as falsy. Measured against the artifact — three distinct
messages, and `or` reports itself as `and`:

| expression | Kestra | `4498b48b` | W2b head |
|---|---|---|---|
| `{% if null %}y{% else %}n{% endif %}` | `Wrong operand(s) type in conditional expression` | error | **`n`** |
| `{{ null and true }}` | `null value used in and operator` | error | **`false`** |
| `{{ null or 'z' }}` | `null value used in and operator` | error | **`z`** |
| `{{ not null }}` | `null value given to not() and strict variables is set to true` | error | **`true`** |
| `{{ null is defined }}` | `false` | `false` | **`true`** |

**The four boolean ones are not regressions, and calling them that misreads
what happened.** They *agreed* at `4498b48b` — both engines errored — for two
different reasons. Pebble errors because a null in a boolean position is an
error. egret errored because `null` was an **undefined variable** and
minijinja's strict mode raises for one; it had no opinion about nulls in
boolean positions then and has none now. Translating the literal removed
egret's accidental reason and left minijinja's own semantics showing. That is a
**masked divergence exposed by a root-cause fix**, and the difference matters to
whoever reads this next: reverting the translation would not fix these four, it
would only put the mask back — and the mask is what kept them out of
`baseline.txt` for the whole of W2.

The direction is still the worse one — a loud failure now reads as a wrong
value — which is the same shape as the `~` note above and the reason they are
recorded rather than left to a report. And none of the five had a corpus entry,
so the ratchet was blind to all of them: the exact blindness Step 1 existed to
remove for `{{ null | F }}`, left in place one construct over. The one boundary entry
that existed here, `null-literal-ternary-condition`, **cannot see any of it**:
Pebble's ternary answers `n` for a null while its `{% if %}` raises, so the two
constructs disagree with each other and only the agreeing one had been written
down.

**The mask is measurable, not argued.** A *context* null answered `n` at
`4498b48b` too — no undefined involved there, so nothing was masked — which is
what shows the wrong answer predates W2b and that what W2b changed is which
inputs reach an answer egret was already giving. Kestra treats a literal and a context null identically in every
one of these positions, so egret is now at least self-consistent.
`null-from-the-context-in-an-if-condition` and `null-from-the-context-is-defined`
are the controls, and they fail on **both** builds — which is what makes that
claim checkable rather than a defence.

Not fixable from this crate: minijinja lets a crate override a filter, a
function and a test, but not `{% if %}`, `and`, `or` or `not`.

### `is defined` agreed BY LUCK, and it IS the one a crate can reach

Pebble's `defined` is "present **and not null**". Measured four ways: a null
literal, a context null, an absent root and a missing attribute are all `false`.
minijinja's is "not undefined", so a `none` is defined. Before W2b the literal
was an undefined and answered `false` — the right answer for the wrong reason,
the same shape this file already records for `filter-string` and `isin`.

`is defined` is also the **only one of the five a crate can reach**, and the
only one where egret answers a *value* that differs from Kestra's value rather
than answering where Kestra errors. `runners::env` now overrides that one test,
and both entries are **banked**.

The fence in `runners::env::MINIJINJA_ONLY_TESTS` says *porting* the `is`
surface "is not W2's — no task was given the eight". Putting back one name W2b's
own change broke is not porting the surface, and the difference is a
**measurement**, not a reading: overriding it moved the ratchet from
`141 of 1288` to `139 of 1288` — the two entries it fixes, no regression,
nothing else touched — and
`the_defined_test_is_pebbles_and_it_is_the_only_one_overridden` asserts the
seven siblings unchanged (`even`, `iterable`, `odd` still answer minijinja's
way; `empty`, `json`, `map`, `null` are still unknown). "An `add_test` binds one
name" is a property of minijinja rather than of this crate, so it is asserted
rather than assumed.

`test-defined` (a present String) agrees either way and could never have
separated them, which is why the row now names `null-literal-is-defined`.

The same surface from the other side: Pebble's null **test** is spelled `null`
and minijinja's is spelled `none`, so `{{ null is null }}` is `true` there and
`unknown test: test none is unknown` here. `null-literal-is-null-test` records
it. The rename W2b added is what would make registering a `none`-spelled `null`
test a one-liner, which is why it is written down now.

### A bare `null` map key leaks Python's spelling into a KEY

`{{ {null: 1} }}` is a Java null key and Jackson refuses it. egret builds a key
from it either way — `{"":1}` at `4498b48b`, `{"None":1}` at W2b's head, because
`none` stringifies through `Display` in key position where
`runners::env::kestra_to_string` never gets a chance to run. Already wrong, now
wrong differently, and it had no entry until this round.

### An entry that agreed for a reason that had nothing to do with `null`

A sixth boolean-context entry was written for `{% elseif null %}` and **passed
on both builds**. Not because egret agrees about null: minijinja spells the tag
`elif` and refuses `elseif` outright — `syntax error: unknown statement elseif`,
whatever the condition, measured with `{% elseif true %}` as the control. A null
entry there would have been a lucky pass in a block whose whole purpose is
discrimination, so it was dropped and `tag-elseif-is-pebbles-spelling` written
instead: a real divergence, compared exact, baselined, with no free pass.

**And it runs both ways, which the first write-up did not say.** Kestra refuses
`elif` exactly as hard as minijinja refuses `elseif` —
`ParserException: Unexpected tag name "elif"`, measured on the artifact with
`{% if false %}a{% elif true %}b{% else %}c{% endif %}`. Neither engine accepts
the other's spelling, so this is not "egret is missing a tag": it is one
construct with two incompatible names, and a port has to translate it in
`runners::rewrite` the way it translates `null`. The entry is written from
Pebble's side because the corpus is Pebble source.

### Three numbers in the committed record were wrong

**"23 of the 44 failed against the pre-fix build" was 28.** It was the count of
a sub-measurement — 37 of the 44 entries existed when that ratchet was run and
23 of those failed; the 7 added later were never folded in, and 5 of them fail
too. Measured as one command on a `4498b48b` worktree carrying the new corpus,
golden and error-class files but **its own** `baseline.txt`, so "regression"
means exactly "a new entry that fails pre-fix": `161 of 1288`, `REGRESSION: 33`,
`161 = 128 + 33` with no other arm firing and **zero** pre-existing ids. 28 of
the 33 are W2b's, 5 are this round's. The wrong number sat in the same paragraph
that corrects somebody else's miscount, which is the reason to state the method
and not just the digit.

**"38 of 70" was 36, and "thirty-two moved" was thirty-four.** The observer the
whole mutation-6 answer rests on. The paragraph listing eighteen names
contradicted its own table — 18 x 2 is 36 — and the 38 came from a build that
did not yet carry `string`'s guard, which the same section says closes here.
Re-measured on the committed tree: of 105 sweep cases, 35 null-literal cases
agree **exactly**, 34 undefined cases refuse, 36 still answer.

### And three more prose numbers, written by the round that fixed three numbers

Named by the re-review, all three in text this round had just added:

- `error-class-pairs.txt` said "All **seven** are ALSO in baseline.txt" over a
  block of **six** ids. The seventh was the `{% elseif null %}` entry, dropped
  for the reason above, and the count was not carried back.
- `error-class.txt` quoted Pebble's `not` message as `null value given to not()
  and or operator`. The artifact says `null value given to not() and strict
  variables is set to true`. `baseline.txt` and this file both had it right —
  the one wrong copy was in the file whose own comment says all three messages
  were measured against the artifact, which is exactly where a paraphrase does
  the most damage.
- `filters.rs` said "fourteen of the sixteen also have a corpus entry". **All
  sixteen do.** Fourteen is the number W2b *added*;
  `filter-upper-literal-null` and `filter-length-literal-null` were banked out
  of `baseline.txt`, not out of `corpus.json`. The error understated this
  crate's own coverage, which is why it survived a round whose subject was
  wrong numbers.

The pattern across all six: every one was a number about *this package's own
work*, and each was written from what the author remembered doing rather than
counted out of the file it describes. C8 says to count them out of the files.
That rule is now three rounds old and has caught something every time.

### What this round strengthened

- the near-miss loop in `the_null_literal_is_exactly_four_spellings` asserted
  only `is_err()`, and its corpus entry is `error-class`, so a rewriter that
  renamed a mixed-case spelling into something unparseable passed all four
  assertions **and that entry**. It now asserts `undefined value`, i.e. refused
  as an absent variable.

  **The ratchet is not silent on that mutant, and the first write-up implied it
  was.** Measured: widening the predicate to `eq_ignore_ascii_case` over *both*
  words renames `None` too, so `null-literal-python-spelling-is-not-the-literal`
  starts matching and the oracle goes red at `138 of 1288 (baseline: 139)` on
  the **PROGRESS** arm. What it does not do is name the cause — it reads as
  progress on a Python-spelling divergence — and
  `null-literal-mixed-case-is-not-the-literal`, the entry actually about mixed
  case, still passes, because `error-class` accepts any failure. So the unit
  test is still the only thing that names the right cause, and the ratchet is a
  second, blunter alarm rather than nothing. (A narrower mutant widening only
  the `null` spelling leaves `None` alone and the ratchet **stays green** —
  measured, which is why the write-up has to say which mutant was run.)
- the sixteen `{{ null | F }}` rows appended to
  `the_ported_filters_refuse_an_undefined_input` asserted only `<>`, which a
  filter that ignored its input would satisfy — live for `escape`,
  `format('x')` and `replace({'a':'b'})`. Each row now carries a **live input
  and its answer**, all sixteen measured on the artifact and all sixteen
  agreeing. Five of the first six written from memory were wrong, which is the
  usual reason to measure.
- `the_ported_filters_refuse_an_undefined_input` accepted
  `contains("an undefined value reached") || contains("Can not pass null
  value")`. The second alternative was reachable only while `is_java_null`
  folded an `Undefined` in — so a test whose subject is "a typo is not a null"
  accepted a typo being reported as a null. Dropped.

### Counted

**139 of 1288**, from 131 of 1278 — ten entries recorded, then two of them
banked by the `defined` override in the round after. All ten failed against
W2b's own head; five also failed at `4498b48b`.

The honest shape of it: the four boolean-context entries are a **masked
divergence made visible**, not a fix's damage; `is defined` was the one of the
five a crate could reach and it is closed; and the remaining eight are held by
the ratchet rather than by prose.
## What W2a's entries measured (read before touching a collection filter)

> **The four round figures below are historical, and deliberately not
> renumbered.** They were measured against `10a484ee` — W2b's first commit —
> which is the base W2a was written on and ran four rounds against. W2a was
> then rebased onto W2b's tip, which had added ten entries and eight baseline
> lines of its own, so the branch's ratchet is **130 of 1336**, not the
> `122 of 1326` those sections close on. The DELTAS in them are still exact;
> only the totals belong to a base that is no longer in this history.
> Renumbering them would state a measurement that was never run, which is the
> failure this section spends four rounds documenting.

W2a is one question that no measurement of a filter *in isolation* can answer:
**a filter's result in Kestra is a specific Java collection type, and the NEXT
filter casts.** `{{ m | keys | sort }}` threw there and answered here, and
neither `keys` nor `sort` is wrong on its own.

W2 Task 8's fix round recorded fourteen such pairs and said in its own report
why fourteen could not be trusted -- *"those are the pairs I thought to cross
-- nothing enumerates the return collection of the other 40 filters."* The
count had already been "three", then "five". So W2a replaced the list with an
enumeration and a generated cross, both committed as answer fixtures beside
`regex-classes.txt` and `jq-surface.txt`:

| fixture | what it holds | rows |
|---|---|---:|
| `return-classes.txt` | all 63 engine filters x a twelve-input matrix, asked through Kestra's own `className` filter | 756 |
| `collection-pairs.txt` | every distinct (producer, returned class, empty?) the enumeration found, crossed with all 63 filters | 2 142 |

The instrument is the engine itself. `className` is a Kestra filter, so
`{{ (<input> | <filter>) | className }}` asks Pebble what its own types are --
no reflection and no bytecode, one `host.Oracle generate`.

### The producer count is eight, and the number that mattered was the CONSUMER count

Eight filters return something that is not a plain `java.util.ArrayList`:

| filter | Java returns |
|---|---|
| `split` | `[Ljava.lang.String;` -- an ARRAY, not a `List` at all |
| `chunk` | `com.google.common.collect.Lists$RandomAccessPartition` |
| `flatten` | `java.util.ImmutableCollections$ListN` |
| `keys` | `ImmutableCollections$ListN` (list/array) / `LinkedHashMap$LinkedKeySet` (map) |
| `values` | `java.util.LinkedHashMap$LinkedValues` |
| `slice` | `java.util.ArrayList$SubList` |
| `escapeChar` | `java.util.HashMap` (map input) |
| `merge` | `java.util.HashMap` (map) / `java.util.ArrayList` (list) |

**But eight is the answer to the class question, not to the behaviour one.**
`slice`'s `SubList` is a mutable `List` that every consumer accepts, and
`escapeChar`/`merge` return a `Map` whose only observable difference is its
ORDER -- already the recorded `filter-escapechar-map-two-keys` class. The
producers that change what the *next* filter does are five, which is what Task
8's fix round said. The number it had wrong was the other one: the cross found
**nine consumers, not six** — and the first cut of this section said eight.
The three it missed are

- **`keys`**, which accepts `Map`, `List` and `isArray()` and therefore refuses
  a map's key view: `{{ m | keys | keys }}` and `{{ m | values | keys }}` throw;
- **`escapeChar`**, which is `instanceof Map` → `instanceof List` →
  `toString()`, so a key or value view falls to the **last** branch:
  `{{ m | keys | escapeChar(type='shell') }}` is the six characters `[b, a]`
  there and was a walked list here. See the fix-round section below for why
  this one took a second review to find;
- **`regexReplace`**, which is not really a cast at all -- `escapeChar` on a map
  renders `{#=2, b=1}` in `HashMap` order, and Kestra's **re-render loop** reads
  the leading `{#` as an unclosed Pebble comment. That one is recorded, not
  fixed: the order is the divergence and the parse error is downstream of it.

A **tenth** thing the kind decides is not a filter at all — the subscript
operator — and no filter × filter cross can reach it. See the fix round.

38 of the pairs were egret answering where Kestra throws. All but the
`regexReplace` one are refusals now.

### `JavaSeqKind` is four capabilities, and `className` is still a recorded divergence

The fix is a tag on the sequence value, not a class name. There are at least
nine distinct Java classes in play (`ArrayList`, `ArrayList$SubList`,
`Arrays$ArrayList`, `AbstractList$RandomAccessSubList`,
`ImmutableCollections$ListN`, `ImmutableCollections$SubList`,
`Lists$RandomAccessPartition`, `LinkedHashMap$LinkedKeySet`,
`LinkedHashMap$LinkedValues`) and every consumer's behaviour follows from four
questions about them: is it a `List`, does `set` work, is it an array, and at
what length. `runners::filters::JavaSeqKind` carries those and nothing else —
and the subscript operator, added in the fix round, needs no fifth question:
it refuses exactly `Collection`, which the model already had.

That keeps the copy-behaviour-not-structure line where this file has always put
it. **`className` still answers `java.util.ArrayList` for every sequence** and
`filter-classname-of-split`, `-of-keys` and `-of-chunk` are baselined, for the
same reason `filter-classname-of-long` is: the name is not observable from a
flow, carrying it would rot the day Guava or the JDK renames an inner class,
and it would still be wrong on the map side. The behaviour behind the names IS
ported, and `collection-pairs.txt` is where it is measured.

The previous round's ruling -- *"not fixable without giving egret a second
sequence type whose only purpose is to be refused, which is implementation
structure by this file's own rule"* -- was the thing to disagree with, and the
disagreement is one sentence: **the class name is structure, but "does this
flow error or answer" is behaviour**, and over-acceptance is the one divergence
this file's rules do not allow — subject to the two ruled exceptions recorded
beside that rule, neither of which is a collection question.

### The LENGTH is part of the rule, and a first cut got it wrong

`ImmutableList` and `Partition` are two kinds because Java's two immutable
lists refuse `sort` at *different* lengths, measured:

| | `sort` throws | `reverse` throws |
|---|---|---|
| `ImmutableCollections` (`flatten`, `keys`) | **always, including on an empty list** -- `AbstractImmutableList` overrides `sort` | at length >= 2 |
| `Lists.partition` (`chunk`) | at length >= 1 -- `List.sort`'s default calls `ListIterator.set` once per element | at length >= 2 |
| `Arrays$ArrayList` (an array through `sort`) | never | never |

`Collections.reverse` swaps `size >> 1` pairs, so a 0- or 1-element list calls
`set` no times and comes back unchanged: `{{ [x] | keys | reverse }}` is `[0]`
and `{{ [] | chunk(size=2) | sort }}` is `[]`. A first cut refused every
immutable list at every length and turned four expressions Kestra answers into
errors; the cross named all four. `filter-sort-of-an-empty-chunk` and
`filter-reverse-of-a-one-element-keys` are the entries that hold the boundary.

The kind is **inherited through `slice`** (`subList` of a mutable list is
mutable, of an immutable one is not, and of an array is still an array) and
**lost through `sort` of an array** (`Arrays.asList` is mutable), both measured
at depth 3 through `className`. And `flatten`'s ELEMENT test is the same
`instanceof List` as its input test, so an inner key view or array is kept
whole rather than unwrapped -- `filter-flatten-keeps-an-inner-key-view`.

### `unstable` is decided by REORDERING, because re-running proves nothing

Fifteen cells of `collection-pairs.txt` carry `unstable` in the Kestra column
instead of a value. They are `split` piped into a filter that stringifies its
input, and what they render is a Java array's `toString` -- its class name and
its **identity hash**, e.g. `[Ljava.lang.String;@7e38a7fe`.

**Two runs of the harness in file order agree perfectly**, because the JVM's
identity hash is a deterministic PRNG over a deterministic allocation sequence.
Re-running is therefore not an instability test. Rendering the probe corpus
**reversed** moves exactly those cells, and that is how the column is decided --
mechanically, and by a rule the fixture states rather than by eye. (Two further
rows of the same class do not move, only because their answer discards the
hash; they carry their value and the same reason.)

There is deliberately **no corpus entry** for any of them: a golden that moves
when the corpus is reordered is a golden nobody can review.

### One divergence off the collection axis, and one CORRECTION that is not one

- `first`/`last` on a String return a `java.lang.Character` there and a
  one-character `String` here. egret has no character type distinct from a
  string, and eight rows of `return-classes.txt` record it.
- `timestampMicro` and `timestampNano` really do return a `java.lang.String`
  where `timestamp` and `timestampMilli` return a `Long` — which corrects the
  brief's table, and **is not a divergence**: egret answers `java.lang.String`
  for both too, so those rows carry `-`. The first cut of this section reported
  it as a third divergence and was wrong; the fixture said so at the time and
  nobody read the column.

And one hole the enumeration exposed rather than closed: **ten of the 63 names
the engine binds have no corpus entry at all** -- `abbreviate`,
`base64decode`, `base64encode`, `merge`, `nl2br`, `numberformat`, `raw`,
`rsort`, `sha256`, `urlencode`. They are Pebble-core filters egret does not
bind, and they account for 465 of the cross's remaining divergent rows.

**That is TEN rows of `name-surface.txt`, not one**, and the first cut of this
section said "the same one row repeated". It is also not always the *consumer*
that fails: in many of those rows the unbound name is the **producer**, so the
expression never reaches a consumer at all. The substance holds — every one of
those rows has its producer, its propagator or its consumer among the ten, with
no exceptions — but "one row" was a compression that misdescribes both halves.

### Regenerating the three W2a fixtures

The same shape as `regex-classes.txt` and `jq-surface.txt`, plus the reversal:

```bash
export JAVA_HOME=/Users/1pro-l4r8/Library/Java/JavaVirtualMachines/amazon-corretto-25.jdk/Contents/Home
cd crates/egret-workflow/oracle
mvn -q -o compile
CP=$(mvn -q -o dependency:build-classpath -Dmdep.outputFile=/dev/stdout | tail -1)

"$JAVA_HOME/bin/java" -cp "target/classes:$CP" host.Oracle generate \
    fixtures/return-classes.probe.json fixtures/return-classes.answers.json
"$JAVA_HOME/bin/java" -cp "target/classes:$CP" host.Oracle generate \
    fixtures/collection-pairs.probe.json fixtures/collection-pairs.answers.json
"$JAVA_HOME/bin/java" -cp "target/classes:$CP" host.Oracle generate \
    fixtures/propagation-pairs.probe.json fixtures/propagation-pairs.answers.json
```

To re-derive the `unstable` set, render a REVERSED copy of
`fixtures/collection-pairs.probe.json` and diff the two answer files by id.

### The mutation run

Two mutations, each built and run in its own `CARGO_TARGET_DIR` and each
restored with the restore **proven by `diff`** and re-run green before the
next:

| mutation | what died |
|---|---|
| `chunk`'s gate neutered to `if !kind.is_list() && false` | `the_collection_kind_gate_refuses_what_java_refuses`; `the_collection_pairs_match_the_engine`, naming `keys(m) \| chunk`; and the oracle with **REGRESSION: 3** -- `filter-chunk-of-keys`, `filter-chunk-of-split`, `filter-chunk-of-values`, which is exactly `chunk`'s pairs and nothing else |
| the length threshold folded: `Partition => len >= 1` becomes `len >= 1 \|\| len == 0` | the same two unit tests, naming `chunk(el) \| sort`, and the oracle with **REGRESSION: 1** -- `filter-sort-of-an-empty-chunk`, the one entry that exists because an empty Guava partition sorts and an empty `Stream.toList()` does not |

The second is the one worth having: it is the mistake the first cut of this fix
actually made, it compiles, it is one token wide, and before
`filter-sort-of-an-empty-chunk` existed nothing in the corpus could have seen
it -- the cross found it by name.

### Counted

**121 of 1305**, from 131 of 1278. Twenty-seven entries. **BANKED fourteen** --
every pair W2 Task 8 had recorded as unfixable -- and **RECORDED four**
(`filter-classname-of-split`, `-of-keys`, `-of-chunk` and
`filter-regexreplace-of-an-escaped-map`). The thirteen new refusals and the
fourteen banked ones are all `error-class`, and all twenty-seven are paired to
one test, `runners::filters::the_collection_kind_gate_refuses_what_java_refuses`,
which walks every (kind, consumer) cell rather than sampling.

What the flat count hides, the same way it did in Task 8's rounds: counted row
by row, exactly **fourteen** of the 38 divergent pairs had a corpus entry of
the same (producer, input, consumer) shape, so 24 of the 38 were fixed with
nothing in the corpus that could have failed. Eleven of those 24 have one now;
the other thirteen -- all of them the empty-map and empty-list variants -- are
held by `collection-pairs.txt`, which asserts every one of its 2 142 rows by
rendering it.

## W2a, fix round 1 — a fact quoted three lines above the code that contradicts it

A review ran **4 344 fresh probes of its own construction** against real Pebble
and against `10a484ee`, verified all eight of W2a's `javap` claims against the
artifact, reproduced `return-classes.answers.json` 756/756 and
`collection-pairs.answers.json` 2 127/2 142 with the fifteen misses being
exactly the fifteen `unstable` rows, and **independently reproduced the
reordering mechanism** — reversing the probe corpus moves exactly those cells
while two file-order runs agree perfectly. It then found three things.

### `reverse` returned its input and the port did not

`ReverseFilter` is `checkcast List; Collections.reverse(list); areturn` **on
the same local**. `Collections.reverse` is `void`. So the filter returns its
INPUT, and the kind survives it. `runners::filters::reverse` rebuilt a plain
sequence and threw the kind away:

| | Kestra | egret, before |
|---|---|---|
| `{{ [] \| flatten \| reverse \| sort }}` | `UnsupportedOperationException` | `[]` |
| `{{ ['b'] \| keys \| reverse \| sort }}` | `UnsupportedOperationException` | `[0]` |
| `{{ ['b'] \| chunk(size=2) \| reverse \| sort }}` | `UnsupportedOperationException` | `[["b"]]` |

Reachable only at length 0 or 1 — at 2 the reverse itself throws — which is
**precisely the boundary this task went to trouble to model**. `sort` has the
same shape and `areturn`s the list it sorted; its propagation is observable
only through `className` on an empty partition, because after `sort` the only
surviving immutable case is length 0 and nothing downstream throws there.

Two things about it matter more than the bug.

**The doc comment three lines above the defect stated the fact that falsifies
it** — *"`ReverseFilter` is a bare `(List) input` and then
`Collections.reverse`"*. The evidence was read, written down, and the
consequence was not drawn. This file now records that shape three times.

**And the cross could not see it by construction.** Every producer in
`collection-pairs.txt` is one filter applied to a context value, so all its
rows are depth 2 and no consumer is ever measured downstream of `reverse`.
A depth-3 run existed during W2a and was not committed, so nothing held it.
`propagation-pairs.txt` is the answer, and its own first cut caught **one** of
the five — see the length note below.

### The ninth consumer, hidden by a wrong reason

`EscapeCharFilter.apply` is `instanceof Map` → `instanceof List` →
`toString()` (`javap -p -c`, and the recursive helper repeats all three at
every depth), so a key or value view falls to the **last** branch:

```
{{ (m|keys)   | escapeChar('shell') }}   Kestra [b, a]    egret ["b","a"]
{{ (m|values) | escapeChar('shell') }}   Kestra [1, 2]    egret ["1","2"]
```

**Both engines' orders are identical** in those two rows — and W2a had filed
them under *"`escapeChar` and `merge` return a `java.util.HashMap`, so the
iteration order is `String.hashCode`'s buckets"*. That reason is false for
them, and it is what closed the question: a row with a reason reads as
explained. **A wrong reason does not merely mislead a reader, it stops anyone
looking.** Correcting the two rows drops the genuine `HashMap` class from 24 to
22 — every one of the 22 audited and confirmed to be a real order difference —
and takes the consumer count from eight to **nine**.

### The subscript operator, and why it is NOT a fifth capability

`{{ (m | keys)[0] }}` is `AttributeNotFoundException: Attribute [0] of
[LinkedHashMap$LinkedKeySet] does not exist` on Kestra and was `b` here.
Pre-existing, and the review proposed recording it as *"a fifth capability the
model does not carry: indexability"*.

**Measured over all 34 producers × five ways of reading them, that framing is
wrong and the fix is cheap.** `[0]` refuses on **every empty sequence** on both
engines — that is an index out of range, not a capability — and among the
non-empty ones the only two that refuse are the two map views. So the predicate
is exactly `kind == Collection`, which `JavaSeqKind` **already carries**. What
was missing was not a capability but a *reader*: the subscript is an operator,
not a filter, and a filter × filter cross cannot reach it.

It is fixed rather than recorded. `JavaSeq::get_value` refuses a `Collection`,
and because `Enumerator::Seq` drives minijinja's own iteration *through*
`get_value`, a `Collection` yields `Enumerator::Iter` instead — `size_hint` on
a `vec::IntoIter` is exact, so `length`, `first`, `default`'s emptiness test and
`{% for %}` all keep working, exactly as they do on Kestra, which answers all
four for a key view and refuses only `[0]`.

### The fixture tests could not see a probe that was not the row

Both looked the probe up by an **id derived from the row's own columns** and
deserialised only `id` and `result`, so both compared columns moved together: a
probe rewritten to `{{ 1 }}`, with the row still reading `keys / m / sort` and
the answers regenerated, was fully self-consistent and green. `javare.rs`, the
precedent W2a followed, keys its harness map on the probe's own
`context["p"]`/`context["s"]` and does not have that hole.

All three fixtures now carry an **`expr` column** and both maps are keyed by
it. Verified by deliberate breakage: rewriting one probe to `{{ 1 }}` fails
with *"keys | m | sort: no probe renders `{{ (m | keys) | sort }}` — the row
and the probe corpus have drifted apart"*, by name.

### The length bucket, and a generated family being only as wide as its key

`propagation-pairs.txt`'s first cut deduplicated producers by
`(propagator, class, empty?)` — the same rule `collection-pairs.txt` used — and
caught **one** of the five `reverse` expressions. The other four need a
producer of length **1**, and the matrix had no length-1 member, so no dedup key
could keep a representative of one.

The matrix gained `l1` and `m1`, the key became `(propagator, class, length
bucket)` with the bucket 0, 1 or many, and the same fixture then catches five.
`Collections.reverse` calls `set` no times below two elements and `List.sort`'s
default calls it once per element: three lengths, three behaviours, and a
two-valued key cannot express that. **A generated family is only as wide as the
axes its dedup key names**, and the axis this one dropped was the one the whole
type exists to model.

### What the counts became

|  | first cut | fix round |
|---|---:|---:|
| input matrix | 12 | **14** |
| `return-classes.txt` | 756 | **882** |
| producers | 34 | **50** |
| `collection-pairs.txt` | 2 142 | **3 150** |
| `propagation-pairs.txt` | — | **4 914** |
| consumers the kind decides | 8 | **9**, and a tenth that is an operator |
| the `HashMap` order class | 24 | **22** |

Over-acceptance across both crosses is **3** — `{{ m | escapeChar(…) |
regexReplace(…) }}` and its **two** propagated twins, all three the recorded
`{#` re-render class — down from 38 + 5. (This said 2 when written, and the
count was 3 at the time; the fix round below corrects it to nine rows across
two classes once `return-classes.txt` is counted too, and the two statements
sat 120 lines apart contradicting each other.)

### The mutation run

Four mutations, all sharing one scratch `CARGO_TARGET_DIR` (never the real
one), each restored with the restore **proven by `diff`** and re-run green
before the next:

| mutation | what died |
|---|---|
| `reverse` returns `MjValue::from(items)` again | `the_collection_kind_gate_refuses_what_java_refuses`; `the_propagation_pairs_match_the_engine`, naming `chunk \| l1 \| reverse \| sort`; oracle **REGRESSION: 3** — the three `filter-sort-of-a-reversed-*` entries |
| `escapeChar`'s list gate neutered to `\|\| true` | the gate test, naming `{{ (m \| keys) \| escapeChar(type='single') }}`; `the_collection_pairs_match_the_engine`; oracle **REGRESSION: 2** |
| `JavaSeq::get_value`'s `Collection` arm neutered | the gate test; oracle **REGRESSION: 2** — `filter-subscript-of-{keys,values}` |
| `JavaSeq::enumerate`'s `Collection` arm neutered | the gate test *and* oracle **REGRESSION: 11** — `filter-keys`, `filter-keys-order`, `filter-first-of-keys`, `filter-for-over-keys`, `filter-flatten-keeps-an-inner-key-view` and more, all rendering `[null,null]` |

The fourth is the one worth keeping: it shows the two halves of the
`Collection` handling are coupled, and that the "must not refuse" assertions
added this round — `length`, `first`, `{% for %}` and `default` over a key view
— are load-bearing rather than decorative. The first cut's versions of those
arms asserted only a leading `[` and a non-empty string; they are pinned to
their values now.

### Counted

**121 of 1320**, from 121 of 1305. Fifteen entries, **no change to the
baseline**: seven of the fifteen fail against `037ed87e` (the three `reverse`
propagations, the two `escapeChar` views, the two subscripts) and eight are
over-refusal guards for the new gates — the side that must keep answering.

Measured the same way for the first commit's twenty-seven: **eight passed
pre-fix too** — `filter-sort-of-an-empty-chunk`,
`filter-reverse-of-an-empty-flatten`, `filter-reverse-of-a-one-element-keys`,
`filter-sort-of-split`, `filter-keys-of-split`, `filter-slice-of-split`,
`filter-reverse-of-a-sorted-split` and `filter-slice-of-chunk`. All eight are
legitimate over-refusal guards and one of them killed W2a's second mutation,
but the first report framed all twenty-seven as earned by the enumeration and
disclosed only two as boundary entries. Both numbers come from checking the
historical `filters.rs` out, building it and rendering the entries, not from
reasoning about them.

## W2a, fix round 2 — the axis a header claimed and the key could not express

A re-review generated 6 946 expressions (34 producers × 17 propagator chains of
length 0-3 × 12 consumers, subscript included, lengths 0-5) and found **zero
regressions**: agreed-at-`10a484ee`-and-diverges-at-head was 0, fixed 400,
over-acceptance 397 → 3. It established two things worth more than any fix.

**The propagation family earns its rows.** Six one-token mutations — `reverse`
drops the tag, `slice` drops the tag, `sort` keeps `Array` — are **invisible to
the depth-2 cross** and die only in the depth-3 family. It is also the only
thing holding the `escapeChar` gate against widening on the array side; the
hand-written gate test does not pin `split | escapeChar`.

**Depth 4 is bounded by the shape of the model, not by hope** — and the first
cut of this paragraph gave the wrong reason for it. It said "every propagator's
kind transition is the identity except `sort: Array → MutableList`", which is
false for **five of the eleven**. The measured table, re-derivable from the
`className` rows of `collection-pairs.txt` and `propagation-pairs.txt`:

| | propagators | transition |
|---|---|---|
| identity | `default` `reverse` `rsort` `slice` | the kind comes back unchanged |
| constant | `escapeChar` `first` `last` `merge` | always `MutableList` |
| constant | `keys` | always `ImmutableList` |
| varies | `sort` | identity, except `Array → MutableList` |
| vacuous | `values` | requires a `Map`, so it never applies to a sequence at all |

**The conclusion survives for a different reason than the one first written:**
every constant's target, and `sort`'s one non-identity target, is `MutableList`
or `ImmutableList`, and both are already reachable at depth 1 — so composition
adds no kind and the reachable set closes after one propagator. The honest
sentence, now in `propagation-pairs.txt`: *depth 3 is measured and committed;
depth 4 is not measured; the kind lattice closes after one propagator, so a
depth-4-only kind regression cannot exist unless `JavaSeqKind` gains
history-dependence.* "Unmeasured" alone reads as a hole and this one is closed
by a five-element lattice.

That it was the right conclusion for the wrong reason is worth keeping. **On
this branch a wrong reason is the recorded way a question gets closed** — it is
what hid the ninth consumer — and the depth-4 argument is the single place a
future reader will stop looking.

### The length axis was still not filled, and filling it found a live defect

The review's last minor was that `propagation-pairs.txt` **claims a
three-valued length axis it does not carry** — `Array` appeared at one length
only — and judged there was no live risk, because every `Collection`/`Array`
gate is length-independent and the length-sensitive readers are held at depth 2.

The first half is right and the diagnosis is deeper than "two producers are
missing": **the bucket was read off the input BINDING**, which is exact for a
list or a map and meaningless for a string, so every `split` result landed in
the "many" bucket whatever its actual length. Round 2 derives the bucket from
the **measured length of the produced value** (a `| length` probe beside every
`| className` one) and adds `sc` — `","`, whose split is the empty array, the
only way this matrix reaches length 0 of `Array`. Tabulated rather than
assumed, all six kinds now carry all three buckets.

**The second half is wrong, and filling the axis is what showed it.** Eight
rows appeared where egret answers and Kestra throws:

```
{{ ',' | split(',') | default('D') }}               Kestra []   egret D
{{ ',' | split(',') | default('D') | capitalize }}  Kestra CCE  egret D
```

`EmptyTest` is `null || (String && trim().isEmpty()) || (Collection &&
isEmpty()) || (Map && isEmpty())` — `javap -p -c`, four branches and **no array
branch** — and a Java array is not a `Collection`, so Pebble's `default` never
substitutes for one however few elements it has. egret's `default` matched on
`ValueKind::Seq` and substituted. Every filter after it then saw a `String`
where Kestra sees an array.

The gate that escaped is **not a `Collection`/`Array` cast at all** — it is
`default`'s emptiness test, which is both kind- and length-sensitive and which
nothing had classified as part of this class. It is reachable at exactly one
cell: length 0 of the `Array` kind, which existed in no instrument until the
key could express it. The two engines agree at every other length, which is why
it hid.

That is the third time on this branch the same shape has appeared, and it is
worth stating plainly: **"I could not construct a mutant that escapes" is not
the same as "the axis is safe to leave unfilled."** The first is a statement
about the mutants someone thought of; the second is a statement about the
space. Filling the axis cost one probe family and found a real over-acceptance
in a filter nobody was looking at.

### The over-acceptance count, corrected

`propagation-pairs.txt` said its two remaining rows were "the only
over-acceptance left in either cross". Counted out of all three fixtures rather
than asserted, this branch commits **nine over-accepting rows across two
classes**:

| class | rows | where |
|---|---:|---|
| the HashMap order reaching the re-render loop as `{#` | 3 | 1 in `collection-pairs.txt`, 2 in `propagation-pairs.txt` |
| a context `Integer` reaching a date-family filter | 6 | `return-classes.txt`, through `date`, `dateAdd` and the four `timestamp*` |

The second is the pre-existing `filter-date-epoch-seconds-context` class —
`AbstractDate.convert` has a `Long` branch and a `String` branch and none for
`Integer`. Both classes are ruled recorded; the ruling was written when only
the first was named, and it now names both.

### `loop.index` is 0-based there and 1-based here

Recorded, not fixed, and **not a collection question**: it reproduces on a bare
context list with no filter in the expression. `loop.index`/`loop.revindex` are
`0/3/2|1/3/1|2/3/0|` on Kestra and `1/3/3|2/3/2|3/3/1|` here; `loop.length`
agrees; and Pebble has no `loop.index0` at all
(`AttributeNotFoundException`), so there is no spelling that means the same
thing on both sides. `loop-index-and-revindex` is baselined with that reason.

It is recorded **because this branch commits `filter-for-over-keys`**, whose
whole argument is that a key view still iterates. A reader who finds that entry
green would reasonably conclude the loop is modelled. The loop *variable*
silently is not.

### Two wordings and an ordinal

- A source comment said the value "is the four characters `[b, a]`". It is six.
  The README said six and the test asserts six — a comment arithmetically wrong
  about the value it exists to explain.
- `baseline.txt` calls `regexReplace` the eighth consumer and `README.md` lists
  the three missed ones as keys / escapeChar / regexReplace, which reads as
  ninth. Both orderings are now labelled: chronological in `baseline.txt`
  (`regexReplace` was #8, `escapeChar` became #9 a round later), code order in
  the README.

### The mutation run

One mutation, in its own scratch `CARGO_TARGET_DIR`, restore proven by `diff`
and re-run green: neutering the `Array`-is-never-empty arm killed the gate
test, `the_collection_pairs_match_the_engine` (naming
`split | sc | default`), and the oracle with **REGRESSION: 3** —
`filter-default-of-an-empty-array`,
`filter-length-of-a-defaulted-empty-array` and
`filter-capitalize-of-a-defaulted-empty-array`.

> **That list, and this round's commit message, were an UNDER-claim.** Re-run
> across the whole crate rather than the three targets that were expected to
> care, deleting the arm fails **four** tests, not three:
> `the_collection_kind_gate_refuses_what_java_refuses`,
> `the_collection_pairs_match_the_engine`,
> **`the_propagation_pairs_match_the_engine`** and the oracle's
> `render_corpus_matches_golden` (420 lib tests, 3 failed; oracle 10, 1
> failed). The propagation fixture was not named because the mutation was
> checked with `-- runners::filters::tests::the_collection`, a filter that
> excludes it by name. A mutation run that filters the tests it expects to kill
> can only confirm the expectation — the honest form is the whole target, and
> the number here is now measured that way.

### Counted

**122 of 1326**, from 121 of 1320. Six entries, one new baseline line
(`loop-index-and-revindex`, recorded). The producer set went 50 → **46** and
`collection-pairs.txt` 3 150 → **2 898**.

For `collection-pairs.txt` that reduction really is a pure duplicate-collapse:
every dropped producer shares its `(filter, class, bucket)` key with one that
was kept. **For `propagation-pairs.txt` it is not, and the first cut of this
paragraph said it was.** Thirteen triples were dropped, nine collapse onto a
representative and **four do not** — `jq|el|first`, `jq|el|last`, `jq|l1|first`
and `jq|l1|last`, i.e. `first`/`last` over a plain `ArrayList` at lengths 0 and
1, which the previous key carried and this one does not.

The cause is this branch's own lesson turned on the new key: the depth-3
producers derive from the depth-2 deduped set, and the depth-2 key is the class
and length of the *produced* value, which cannot express the length of the
**element** `first`/`last` extracts — `jq|el`, `jq|l1` and `jq|l` are all
`(ArrayList, 1)` while `first` of them is length 0, 1 and 3.

The four cells were **measured** before the trade was accepted: 4 × 63 = 252
expressions through both engines, of which 140 agree, 96 agree by both
refusing, 16 are the recorded unbound-name class, and **none** is an
over-acceptance or a differing answer. Four cells lost, four gained (`default`
and `sort` over `Array` at lengths 0 and 1). Carrying them would take a fourth
axis — the length of the *element* — on the depth-2 set, which 252 agreeing
rows do not justify. See `propagation-pairs.txt`, which now names the loss.

Fewer rows, and the axis the header claims is the axis the key carries — for
the three axes the key names.

## W2a, fix round 3 — what a reduction lost, and a right answer with a wrong reason

No behaviour changed in this round. A check of 1 296 expressions (36 producers
× {plain, `| default('D')`} × 18 consumers) through real Pebble and through
egret at **both** commits came back clean: zero regressions, 12 fixed,
over-acceptance 2 → 0, no panics. The `default` arm matches `EmptyTest` branch
for branch over all 36 producers — including that NBSP and U+200B are **not**
trimmed on either side, and that `split | sort | default` is `D` on both, so
the arm does not over-reach. The producer set was re-derived from scratch
through real Pebble and came back as **exactly the committed 46 triples, zero
missing and zero extra**, with `sc` confirmed as the only route to `Array`
length 0.

Four things were wrong in the record, and three of them are this task's own
subject matter.

### The reduction was not free, and three places said it was

`collection-pairs` 50 → 46 is a clean dedup. **`propagation-pairs` is not.**
Thirteen `(producer, input, propagator)` triples were dropped; nine share their
`(propagator, class, bucket)` key with a triple that was kept, and four do not:

```
jq|el|first -> (first, ArrayList, 0)     no representative
jq|el|last  -> (last,  ArrayList, 0)     no representative
jq|l1|first -> (first, ArrayList, 1)     no representative
jq|l1|last  -> (last,  ArrayList, 1)     no representative
```

Verified by computing every dropped triple's key through the artifact rather
than by reading the diff: nine collapse, four do not. `first`/`last` over a
plain `ArrayList` is now carried at "many" only, where the previous key carried
0, 1 and many.

**The cause is this branch's own lesson turned on the new key.** The depth-3
producers derive from the depth-2 deduped set, and the depth-2 key is the class
and length of the *produced* value — which cannot express the length of the
**element** that `first`/`last` extracts. `jq|el`, `jq|l1` and `jq|l` are all
`(ArrayList, 1)`, each a one-element list holding its input, so only one
survives depth 2, while `first` of them has length 0, 1 and 3.

The trade is acceptable and it was **measured** before it was accepted: the
four cells are 4 × 63 = 252 expressions, of which 140 agree, 96 agree by both
refusing, 16 are the recorded unbound-name class, and **none** is an
over-acceptance or a differing answer. Four cells lost, four gained (`default`
and `sort` over `Array` at lengths 0 and 1). Carrying them needs a fourth axis
— the element's length — on the depth-2 set, which 252 agreeing rows do not
buy. What was not acceptable is three places describing it as pure
duplicate-collapse; all three now name the loss, the measurement and the price
of reversing it.

### The exception clause never reached the rule

"Over-acceptance is the one divergence this crate's rules do not allow" stood
unqualified at its own heading and was repeated absolutely 1 400 lines later,
while nine documented violations sat under it and the exception lived only in a
fix-round section. **A rule and its exceptions have to be in the same place** or
the rule is read as absolute by exactly the reader it is written for. The
clause is now beside the heading, as a table of the two ruled classes with the
reason each is not fixed, and the second restatement carries the qualifier.

The second class was checked rather than taken on trust: the split is
**literal-versus-context provenance for the same digits inside int32**, which
no predicate over the *value* can recover, and closing it would break
`filter-date-epoch-seconds-literal`.

### The depth-4 argument was right for the wrong reason

See the round-2 section above, now corrected: five of the eleven propagators
are not identity transitions. The conclusion holds because those five are
**constant** and their targets are already reachable at depth 1 — not because
they are the identity. `values` is neither: it requires a `Map`, so it is
**vacuous** on the sequence lattice.

### Three stale numbers, in the commit whose thesis was that a header's claim is not the axis

`propagation-pairs.txt` said "all 34 producers" beside a line saying 46, and
"all 2 142 rows are depth 2" about a file with 2 898; `collection-pairs.txt`
restored a `2 142` denominator that round 1 had deliberately removed, and still
said "Thirteen cells move" where the file carries 46 `unstable` rows and the
test asserts 46. The propagation twin had been updated to "Sixty move" and this
one had not. All corrected, and the historical figure is now labelled as
historical instead of read as current.

### Counted

**122 of 1326** against `10a484ee`, unchanged — no corpus entry, no baseline
line, no behaviour. This round is entirely the record. The one number that
moved is a claim about the previous round's mutation, which is now four tests
rather than three, measured across the whole crate.

**On the rebased branch the ratchet is `130 of 1336` (baseline: 130)**, from
`139 of 1288` at W2b's tip. The merge of the two corpora was by `id`: the base
is a prefix of both sides, no added id collides, and the only entries both
sides had edited are ten whose golden is NONDETERMINISTIC — `now`, `uuid`,
`id`, `ksuid`, `nanoId`, `randomInt`, `randomPort`, `encrypt`'s IV and two
`fromIon` exception texts — where neither side is more correct and W2b's
values were kept so this branch's diff carries only its own entries.

Four figures in the sections above this one were **recomputed** on the merged
tree rather than picked from a side, and the recomputation caught one of W2a's
own: the `error-class` total read **259** where the file carried **265**, a
stale number that survived the round whose subject was stale numbers. The
`error-class` ∩ `baseline.txt` count is the one that proves picking a side
would have been wrong — 25 at the base, 31 at W2b's tip, 12 at W2a's, and
**18** merged. It is smaller than either side because W2a DELETES baseline
lines when it banks, so the intersection is not monotone and no choice between
two claims could have produced it.

## What package #5a's Task A1 batches measured (2026-10-28)

Plan `docs/en/plans/2026-10-28-workflow-renderer.md` A1; spec
`docs/en/specs/2026-10-28-workflow-renderer-design.md` D1-D6. 157 entries,
appended at the end of `corpus.json` in eleven batches, each a blank-line
group whose id prefix names it: `arith-long-` (and `arith-double-`),
`arith-zero-`, `arith-bigdecimal-` (and `arith-ion-`), `arith-operand-`,
`cmp-`, `chain-`, `coalesce-chain-`/`coalesce-lazy-`/`coalesce-...`,
`operator-` and `logic-`. Four survey probes were left out because an
existing entry already holds the same expression and context
(`function-fromion-decimal-arithmetic`, `-division`, `-greater-than`,
`op-contains-boxed-int`). 31 golden values are Kestra exceptions and are
`error-class`; 20 of them are baselined (egret answers), 11 are born matching
and paired with three `runners::env` tests that assert the failure.

The golden file was regenerated offline with the recipe above (`mvn -o`).
The regeneration changed exactly the eight `NON_DETERMINISTIC` ids and
`function-fromion-annotation-of-a-list`, as this file predicts; those nine
were kept at their committed values so the diff is only the new entries.

What the batches measured that a later task depends on:

- **Long arithmetic is Java's.** `7 / 2` is `3`, `-7 / 2` and `7 / -2` are
  `-3`, `-7 % 2` is `-1`, `7 % -2` is `1`; `+ - *` wrap at both ends;
  `Long.MIN_VALUE / -1` is `Long.MIN_VALUE` and `% -1` is `0` (no exception).
  `/ 0` fails `Could not perform division`; `% 0` fails `Could not perform
  greater modulus` (Pebble's text, verbatim). Double: `1.0 / 0` is
  `Infinity`, `-1.0 / 0` is `-Infinity`, `0.0 / 0` and `1.5 % 0` are `NaN`.
- **Two context integers wrap at 2^31** (`65536 * 65536` is `0`,
  `2147483647 + 1` is `-2147483648`), and a context integer times a literal
  does not (`4294967296`): spec D2's Integer-vs-Long divergence is real. Both
  entries are baselined as that recorded divergence.
- **A context `BigInteger` goes down the Long branch through `longValue()`**,
  not `intValue()`: `99999999999999999999 + 1` is `7766279631452241920`
  (the value mod 2^64, plus one). `+ 0.5` makes it a Double (`1.0E20`).
- **Operand kinds.** A string on either side of `+` concatenates
  (`1 + 'a'` is `1a`, `'a' + null` is `anull`, `'a' + [1, 2]` is
  `a[1, 2]` -- Java's `List.toString`, with a space). Otherwise a string
  goes through `toNumber`: `'5' * 2` is `10`, `'7' / 2` is `3`, `'2.5' * 2`
  is `5.0`, `'1e2' * 1` is `100.0`, `'abc' * 2` fails. A list on the left of
  `+` appends (`[1] + 2` and `[1] + [2]` are both `[1,2]`); on the left of
  `-` it removes the first occurrence (`[1, 2, 1] - 1` is `[2,1]`) or every
  element of a right-hand list (`[1, 2, 3] - [1, 3]` is `[2]`). `2 + [1]`,
  `[1] * 2`, a map operand, a null operand and a boolean operand fail.
- **BigDecimal** follows D3: `fromIon('1.50') * 2` is `3.00` (both operands
  through `BigDecimal.valueOf(double)`, so `1.5 * 2.0`), `fromIon('1.50') + 1`
  is `2.5`, `fromIon('1.0') / 3` rounds at the 34th digit (`...3333` and
  `...6667` for `2.0 / 3`), `fromIon('1') / 3` is Long division (`0`), and a
  30-digit value loses its precision through the double. egret today computes
  a `fromIon` decimal as a Double (`(fromIon('1.5') * 2) | className` is
  `java.lang.Double` here, `java.math.BigDecimal` there): that is A4's first
  check, answered.
- **Comparison.** Every non-number, non-string pair fails, and so does a
  number against a string; a null operand answers `false` for all four
  relational operators, null against null included. `==` and `<` share a
  precedence tier and associate left: `1 < 2 == true` is `true`,
  `1 == 1 < 2` fails, `1 == 2 == false` is `true`.
- **`??`/`???`** answer through every missing member, literal subscript and
  out-of-range index, on a missing root or a missing member, and inside `{% if
  %}` and a filter's operand; the right side is lazy (`x ?? missing.y`,
  `x ?? (1 / 0)`). `obj.a ??? 'h'` with `a: null` is the empty string. A
  call on the left (`fromJson('1') ?? missing.y`) is `1` in Kestra and is
  the D6 non-path divergence, baselined as such.
- **`equals`** is `==` (`1 equals 1.0` is `true`, `1 equals '1'` is `false`).
  **`contains`** checks a map's keys, not its values, a list's elements
  (and every element of a list on the right), and a substring; a number on
  the left fails. **`..`** is ascending only: `3 .. 1` and `'c' .. 'a'` are
  `[]`.
- **`and`/`or`** return a boolean and coerce each operand: a number is true
  when non-zero, a string when non-empty (`'false' or false` is `true`); a
  list or map fails with a `ClassCastException`; a null fails, with the text
  `null value used in and operator` even under `or`.

**What #5a did with them (Task A8, 2026-10-29).** Of the 98 lines A1 recorded
under `#5a red` (and the one A3 added, `coalesce-lazy-right-division`), **four
remain**, each a recorded divergence named in spec
`docs/en/specs/2026-10-28-workflow-renderer-design.md` §6.1: the two
context-integer probes (D2, Integer vs Long), `coalesce-non-path-left-missing-right`
(D6: a call on the left keeps the eager form) and `logic-and-returns-boolean`
(minijinja's `~` writes a boolean as `True`). The sentence above that says egret
"today" computes a `fromIon` decimal as a Double is A1's measurement: since Task
A4 it is a `java.math.BigDecimal` (Q5 reverses the W9c refusal), and since A8 a
unary minus on one is Pebble's `multiply(-1, x)` (`-fromIon(ion='1.50')` is
`-1.50`). The oracle ends #5a at **107 of 1493** baselined: 103 lines from
before A1 and those four.

## What package #5b's Task B1 batches measured (2026-10-29)

Plan `docs/en/plans/2026-10-28-workflow-renderer.md` B1; spec
`docs/en/specs/2026-10-28-workflow-renderer-design.md` D8, D10, D12, D13.
260 entries, appended at the end of `corpus.json` in nine blank-line batches:
`test-` (59, spec D8), `function-max-`/`function-min-` (34, D10), the ten
filters (60, D13), `filter-escape-` with the survey's inputs, the position
rules and the unknown strategies (23, D12), and one boundary batch per
`escape` strategy (`filter-escape-<strategy>-boundary-`, 84 together). The
regeneration changed only the eight `NON_DETERMINISTIC` ids and
`function-fromion-annotation-of-a-list`/`-in-a-list`; those ten were kept at
their committed values, so the `golden.json` diff is only the new entries.

26 are Kestra exceptions (`error-class.txt`): 4 baselined (`even`/`odd` on a
String or a Double answer here), 22 born matching for the WRONG reason (the
name is unknown here) and paired with four `runners::env` placeholder tests
(`a_test_kestra_refuses_is_refused_here_too`,
`max_and_min_refuse_what_pebble_cannot_compare`,
`a_filter_input_kestra_refuses_is_refused_here_too`,
`an_escape_strategy_pebble_does_not_know_is_refused`). 38 pass today: those 22,
`even`/`odd` on a Long, `iterable` on a list, the twelve `html` strategy entries (the
default escaper is already Pebble's) and `escape(strategy='js')` of a null. The other **222** are baselined under
`#5b red`: B2 49, B3 29, B4 53, B5 91.

**How the boundary batches were chosen.** Pebble 4.1.2's `EscapeFilter` maps
`html`, `js`, `css`, `url_param` and `json` to unbescape's
`HtmlEscape.escapeHtml4Xml`, `JavaScriptEscape.escapeJavaScript`,
`CssEscape.escapeCssIdentifier`, `UriEscape.escapeUriQueryParam` and
`JsonEscape.escapeJson` (`javap -v`, bootstrap method handles). Each was called
on every code point from U+0000 to U+10001 (after an `a`, so in the middle of
a string) and the output classified by shape (unchanged, single-character
escape, `\xHH`, `\uHHHH`, CSS `\H..`, `%HH` by byte count, named or numeric
entity). Each run of one shape is one entry holding both ends of the run
between `z`s (`z` is not a hex digit, so a CSS escape before it takes no
space); the last run's upper end is U+10FFFF. Plus the surrogate gap, U+D7FF
and U+E000, which is a boundary of the UTF-8 encoding on the Rust side.

**Measured answers B2-B5 follow.**

- **Tests.** `is null`: a null is `true`, `0` and `''` are `false`; a missing
  root or member fails (strict). `is empty`: `''`, whitespace (`trim()`, so a
  tab and a newline count, a no-break space U+00A0 does not), `[]`, `{}` and
  null are empty; `0`, `false` are not; a missing root fails. `is map`: a map
  only (null, a list, a string, a number are `false`). `is json`: a string
  that Jackson reads as a value -- `'1'`, `'[1, 2]'`, `'"s"'`, `'true'`,
  `'null'`, `'  1  '`, **`''`** and **`'1 x'`** (Jackson stops after the
  first value) are `true`; `'plain'` and `'{'` are `false`; a map, list or
  number fails (`ClassCastException` to String), a null fails (`argument
  "content" is null`). `even`/`odd` cast to Long: a String or a Double fails.
  `iterable` is `instanceof Iterable`: a list is `true`; a String, a map and
  null are `false`.
- **`max`/`min`.** One argument is returned as it is (`max([1, 2])` is
  `[1,2]`); `max()` and `max(null)` and `max(null, null)` render empty; nulls
  are skipped; the larger operand keeps its own type (`max(1, 2.5) |
  className` is `java.lang.Double`, `min(fromIon(ion='2.50'), 1) | className`
  is `java.lang.Long`); **a tie keeps the first operand** (`max(2.0, 2)` is
  `2.0`, `max(2, 2.0)` is `2`, `max(fromIon(ion='2.50'), 2.5)` is `2.50`); a
  string, boolean or list operand fails with `invalid operands for
  mathematical comparison [GREATER_THAN]` (`[LESS_THAN]` under `min`).
- **Filters.** `abbreviate`: `'abcdef' | abbreviate(6)` is `abc...` (a
  string of exactly the width is cut too), `abbreviate(3)` of `abcdefg` is
  `abc`, `abbreviate(4)` is `a...`, **`abbreviate(0)` is the input
  unchanged** (only a negative width fails: `Invalid argument to abbreviate
  filter; must be greater than zero`), a number input fails, a null renders
  empty. `base64decode` accepts an unpadded input (`'YWI'` is `ab`); a bad one
  fails with `Please provide a correctly Base64 encoded string ...`. `merge`:
  list+list concatenates, map+map right wins, list+map appends the map,
  **map+list adds each element as a key mapped to itself**
  (`{'a': 1} | merge([1])` is `{"a":1,"1":1}`), a null argument answers the
  input, a string input fails (`The object being filtered is not a
  Map/List/Array`). `nl2br` turns `\r\n` into one `<br />`. `numberformat`:
  with no argument, three fraction digits at most (`1,234.568`, `0.123`);
  `'0.00'` rounds `0.125` to `0.12` (HALF_EVEN on the double); `'#%'`; a
  BigDecimal formats; a string fails (`The input for the 'NumberFormat'
  filter has to be a number.`); null renders empty. The JVM locale was
  `en_VN`, so D13's locale divergence is not exercised. `raw` is the
  identity (`'<b>' | escape | raw` stays escaped). `rsort` of a context list
  mixing a Double with Integers fails (`ClassCastException`), as a String with
  a Long does. `urlencode` is `URLEncoder` (`~` is `%7E`, `*` stays).
- **`escape`.** `json` escapes **every** non-ASCII character as `\uHHHH`
  (`é` is `\u00E9`; the survey's table showed it unescaped), `&` as
  `\u0026` and `/` after `<`; `js` writes U+0000 as `\0`, U+007F-U+00FF as
  `\xHH`, `&` as `\x26`, `/` after `<`. `css` escapes a leading digit as
  `\31 `, a leading `-` before a digit, `--` and a leading `_`; a hex escape
  takes a trailing space only before a hex digit (`\E9 a`, `\E9z`), and a
  space after it is itself escaped (`\E9\ z`). An unknown strategy, an
  upper-case name (`JS`) and `html_attr` fail with `Unknown escaping strategy
  [x]`.
