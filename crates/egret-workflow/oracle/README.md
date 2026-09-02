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
java -cp "target/classes:$CP" host.Oracle dump

# Step 4: render corpus.json -> golden.json
java -cp "target/classes:$CP" host.Oracle generate corpus.json golden.json
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
an entry matches on any failure at all, so it is born matching, never enters
`baseline.txt`, and can never fail. A task that could not reproduce a filter
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
| `test:<name>` | `<name>` is a `#[test] fn` that **exists** in `src/runners/`, found by scanning the committed source with `include_str!` |
| `baselined` | the id really is in `baseline.txt`, so it is not born matching at all and the ratchet already holds it |
| `unported:<task>` | nothing is ported behind it; the **set** of these must equal `UNPORTED_ERROR_CLASS` in `tests/oracle.rs`, so a ninth cannot be added without editing a test |

The two files must name exactly the same ids, in both directions. An entry
added without a pair fails **by name**, the way a stale baseline line does.
Verified by deliberate breakage before committing: an unpaired new entry, a
renamed paired test, a false `baselined` claim and a quietly-added
`unported:` all fail, each naming the id.

**78** entries carry it today, counted out of `golden.json` rather than added
up: the four `*-mixed` comparisons (Pebble throws on a String/Number mix), the
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
ones). **Every one of them is separately backed by a unit test in
`runners::filters` asserting egret's own message**, named alongside it in
`error-class.txt` and mechanically checked against `error-class-pairs.txt`, so
what carries them is the test and not the mode.

That is a lot, and it is a deliberate trade: each records an exception Kestra
actually throws, which is worth having in `golden.json`, at the cost of an
entry the ratchet cannot fail. **The ratio deserves a look before Task 8**,
whose whole deliverable is refusals. Note that marking an entry error-class
does **not** make it pass: 5 of the 78 are *also* in `baseline.txt`, because
egret *succeeds* where Kestra throws — a real divergence, and it stays
visible.

## `baseline.txt` — the ratchet

`tests/oracle.rs` is not a pass/fail test while Tasks 4-8 are outstanding; it
is a ratchet. `oracle/baseline.txt` lists the ids that do not match yet: 76 of
138 at W2 Task 3, 57 of 241 after Task 4, 59 of 268 after its fix round,
60 of 291 after fix round 3, 59 of 426 after W2 Task 5, 62 of 457 after that
task's fix round, 62 of 486 after its second, 66 of 513 after its third,
**76 of 632** after W2 Task 6 -- which BANKED seven (its whole batch of
filters) and RECORDED seventeen, every one of them a place the two engines
answer differently rather than a filter that is not ported.
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

**Run the whole target, not one test.** `--test oracle` carries **six** tests,
and four of them guard the differential test rather than take part in it:
`corpus_and_golden_describe_the_same_expressions` (the corpus and the golden
file describe the same expressions),
`error_class_entries_are_exactly_the_committed_allowlist`,
`every_error_class_entry_is_paired_with_something_that_can_fail`, and
`a_panic_fails_even_when_the_ratchet_is_satisfied`. Filtering to
`-- render_corpus_matches_golden` — the natural thing to type while iterating —
**skips all four**, so the ratchet will happily pass against a corpus that has
drifted out of sync with the golden file it is being checked against, or
against a free pass nothing can fail.

`--nocapture` is what actually shows the count and the exclusion lists. Without `--nocapture`, a
green run tells you nothing about how much of the corpus is still baselined.

Both directions were verified by deliberate breakage before this was
committed: breaking `kestra_null_coalesce` produced `REGRESSION: 6`, and
registering a working `json` test without editing the baseline produced
`PROGRESS NOT BANKED: 2`.

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

## Corpus (`corpus.json`, 632 entries)

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
  `UndefinedBehavior::Strict` diverges from Kestra's `??`).
- **`op-range` records that `..` is inclusive**: `{{ a .. b }}` with `a=1,
  b=3` renders `[1,2,3]`, and `{% for i in 1 .. 3 %}` iterates three times.
  minijinja's `range(1, 3)` is `[1, 2]` — exclusive, and printed with spaces.
  Whoever ports `range` (Task 7's `BoundedRangeFunction`) owns both
  differences.

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

## What W2 Task 6's 116 new entries measured (read before touching `jq`, `toJson`, `toIon`, `yaml` or the three `regex*` filters)

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
