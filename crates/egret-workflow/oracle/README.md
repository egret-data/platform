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

## Corpus (`corpus.json`, 99 entries)

- **28 operator cases**, taken verbatim from the task brief plus three
  follow-up fixes: the `??`/`???` pair (5 entries covering null vs. absent on
  both operators, plus a plain present case), ternary (including nesting),
  `isIn`, each of `>`/`>=`/`<`/`<=` across string/number/mixed operands
  (`Extension.getBinaryOperators()` registers all four as separate classes —
  `GreaterThanExpression`, `GreaterThanEqualsExpression`, `LessThanExpression`,
  `LessThanEqualsExpression` — so each can diverge independently), three
  equality-boundary cases (`'a' >= 'a'`, `'a' <= 'a'`, `2 >= 2`) that only the
  `=`-inclusive operators can reach, a literal `??`-looking substring inside a
  quoted string (proves a naive regex-based rewriter would corrupt it), three
  `{% %}` tag-body cases (`tag-if-coalesce`, `tag-set-coalesce`,
  `tag-if-ternary`) added after a review of the Task 2 rewriter found it was
  only ever exercised against `{{ }}` — Pebble parses a `{% if %}`/`{% set
  %}`/`{% for %}` tag's body as an expression too, and a tag-body-aware
  rewrite needs its own oracle coverage, not just `{{ }}`'s — and
  `unparenthesized-nested-ternary` (`{{ a ? b ? c : d : e }}`,
  `a=false, b=true`), which empirically pins Pebble's
  `parseTernaryExpression` grouping (`a ? (b ? c : d) : e`, giving `"e"`)
  against the wrong right-associative reading a port could plausibly emit
  instead (which would give `"c"`) — added after a review found the port's
  own then-branch-parens fix was proven only by prose next to the code and a
  test whose input (`a ? (b ? 'x' : 'y') : 'z'`) had its own source parens
  making the grouping explicit, so the fix could regress with nothing in
  this repository catching it.
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

## What this harness does **not** reproduce

- `VariableRenderer`'s `{% raw %}...{% endraw %}` tag protection (pre/post
  UUID-substitution around raw blocks) — no corpus entry uses `{% raw %}`,
  so this was not implemented. If a future task adds one, this harness needs
  the same protection `VariableRenderer.renderOnce` has, or the golden value
  will be wrong.
- Recursive re-rendering *is* reproduced (`Oracle.render()` loops exactly
  like `VariableRenderer.renderRecursively`, stopping once output stops
  changing, capped at 100 iterations) even though no current corpus entry's
  output happens to look like another template.
