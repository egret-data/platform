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
reviewable diff instead of a field edit buried in a 138-entry JSON file — the
same discipline `baseline.txt` applies to mismatches.

25 entries carry it today: the four `*-mixed` comparisons (Pebble throws on a
String/Number mix), the eight registered-but-null-backed functions,
`raw-multiline`, the three `isin-*` entries where an absent variable raises,
and **nine from W2 Task 4** — four wrong-typed inputs
(`filter-chunk-not-list`, `filter-distinct-not-list`, `filter-values-not-map`,
`filter-number-not-string`) and five exceptions its fix round found
(`filter-escapechar-map-null-value`, `filter-escapechar-list-null-element`,
`filter-chunk-size-truncated`, `filter-indent-amount-overflow`,
`filter-number-int-overflow`). **Every one of the nine is separately backed by
a unit test in `runners::filters` asserting egret's own message**, named
alongside it in `error-class.txt`, so what carries them is the test and not
the mode. Note that marking an entry error-class does **not** make it pass:
five of the 25 still mismatch, because egret *succeeds* where Kestra throws —
which is a real divergence and stays visible.

## `baseline.txt` — the ratchet

`tests/oracle.rs` is not a pass/fail test while Tasks 4-8 are outstanding; it
is a ratchet. `oracle/baseline.txt` lists the ids that do not match yet: 76 of
138 at W2 Task 3, 57 of 241 after Task 4, **59 of 268** after Task 4's fix
round (which added 27 entries and two more baselined rows of one already-known
`className` fact). The test fails in **both** directions:

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

**Run the whole target, not one test.** `--test oracle` carries five tests, and
three of them guard the differential test rather than take part in it:
`corpus_and_golden_describe_the_same_expressions` (the corpus and the golden
file describe the same expressions), `error_class_entries_are_exactly_the_committed_allowlist`,
and `a_panic_fails_even_when_the_ratchet_is_satisfied`. Filtering to
`-- render_corpus_matches_golden` — the natural thing to type while iterating —
**skips all three**, so the ratchet will happily pass against a corpus that has
drifted out of sync with the golden file it is being checked against.

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

## Corpus (`corpus.json`, 268 entries)

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
- **130 W2 Task 4 additions** (138 -> 241 -> 268), the null / empty /
  wrong-typed / boundary inputs of the 22 string and collection filters that
  task ported, plus 27 more from its fix round. Their findings are the two
  sections "What W2 Task 4's 103 new entries measured" and "What W2 Task 4's
  fix round measured" below. **117 of the 130 are compared by value and pass**;
  nine are compared by error class (see above) and four are baselined
  divergences with their reasons in `baseline.txt`.
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
  refused. That last one is a **bound, not only a fidelity point**: without
  the narrowing the port reached `prefix.repeat(2_147_483_648)`, a 2 GiB
  allocation from one template literal. Note `chunk` casts to `Number` (so
  `chunk(size=2.5)` truncates to 2) while `indent` casts to `Long` (so
  `indent(amount=2.5)` is a `ClassCastException`) — the two filters differ.
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

### One behaviour here has NO golden, on purpose

`indent`/`nindent` refuse when `separators x amount x prefix.len()` would push
the result past `runners::env::MAX_OUTPUT_BYTES` (1 MiB). **Kestra does not
refuse** — it will attempt `indent(amount=2000000000)`, a 2 GiB allocation from
one template literal, and fall over. egret bounds it deliberately, on the same
call as `rewrite`'s `MAX_NESTING_DEPTH` (also a limit Kestra lacks): fidelity
does not extend to reproducing a resource-exhaustion vector in a renderer that
W3 runs on author-supplied source in a request path, and an intermediate
allocation three orders of magnitude past the output cap defeats that cap,
which only measures what survives the allocation.

The bound is **derived from the existing cap, not a second number**, and it is
checked before anything is allocated.

There is no corpus entry, and that is not an omission: the harness cannot
produce a golden for it. At `amount=2000000000` Kestra OOMs or hangs; at the
largest amount Kestra answers quickly, the golden would be a multi-megabyte
string committed into `golden.json`. `runners::filters`'
`indent_refuses_to_allocate_past_the_output_cap` is the pin instead. Its
falsification is arranged to be safe — the 1.2 MB case runs first, so removing
the bound fails the test there rather than allocating 2 GiB — and it asserts
the *filter's* message, since `RenderError::OutputTooLarge` ("rendered output
exceeded 1048576 bytes") is exactly the after-the-fact error the bound exists
to pre-empt.

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
