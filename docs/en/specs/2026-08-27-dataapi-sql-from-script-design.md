# Data API — SQL from a script (package S)

**Status:** design, not yet planned
**Programme:** DBAPI parity. This **replaces stages 2–4** of the script engine
(`db.query`, writes, outbound HTTP), which are hereby dropped.
**Depends on:** C2 (`5774bc77`) for the evaluator and its limits; M1/M2
(`1140dff4`, `ebf3791c`) for the probe machinery and per-statement contracts;
P4.5 for the bind formatter this design refuses to go around.

## 1. What this is

A statement's SQL may be **written** (today) or **generated** by a Starlark script:

```python
TABLES = {"daily": "sales_daily", "hourly": "sales_hourly"}

def build_sql(params):
    table = TABLES[params["grain"]]
    where = ["region = {{ region }}"]
    if params["min_total"]:
        where.append("total >= {{ min_total }}")
    return "SELECT region, sum(total) AS total FROM " + table + \
           " WHERE " + " AND ".join(where) + " GROUP BY region"
```

**The script returns a template, not finished SQL.** `{{ region }}` is still
`{{ region }}` when the script hands it back, and egret runs it through the same
minijinja pipeline that runs hand-written SQL. The script decides the statement's
**structure**; the existing formatter decides its **values**.

### 1.1 Why this and not `db.query`

Stages 2–4 were staged as `db.query` → writes → outbound HTTP, with the line drawn
between 2 and 3 because stage 3 turns egret from a *router* into a *runtime*.

This gets the practical value of stage 2 while staying on the router side of that
line, and it does so more simply: the script produces **text**, and egret executes
that text exactly as it executes an author's own. No new connection handling, no
N+1, no concurrency cap, no second identity path, no question about who the reader
is. Ranger and Cedar arbitrate exactly as before because the query reaches the
engine the same way it always did.

**The one-line summary of the safety argument: a script can produce nothing an
author could not have typed.** Every rule that guards hand-written SQL guards
generated SQL, because it is the same pipeline.

## 2. The safety property, and the one place it could break

### 2.1 Values are already safe, and nothing about them changes

The bind formatter turns every `{{ x }}` into a bind marker. A script emitting
`{{ region }}` produces a bind, exactly as a hand-written statement does. **The
path a caller's value takes to the engine is unchanged by this feature**, which is
the whole reason the script returns a template rather than a string.

This is not a stylistic preference. P4.5 established, expensively, that **a byte
scanner guarding a template language will lose** — five independent reviews found
five different bypasses of one. And `sql_literal` shipped a real hole: it doubled
`''` and ignored `\`, which escapes on StarRocks (Trino was safe), fixed in #504
and proven live rather than argued. A design where a script hands back finished
SQL would put every parameter back on the wrong side of that lesson.

### 2.2 Identifiers, which is the actual question

`ident_filter` (`render.rs:569`) compares the caller's bytes to the author's
`allowed` list with `==` and then **emits the author's `String`** — the code's own
comment calls cloning `entry` rather than `supplied` "the whole security property".

A script may emit an identifier from exactly two sources, and **neither needs a new
mechanism**:

- **A string literal the script contains.** `TABLES["daily"]` yields `sales_daily`,
  which is written in the script, and the script is author-written and frozen in the
  snapshot. Provenance is the author's — the same provenance an `enum`'s `allowed`
  entry has. What the script adds is the ability to *decide which* literal appears,
  with arbitrary logic. That is the "choose the table from a parameter" case, and it
  is safe because the set of possible tables is written down by the author.
- **`{{ x | ident }}`**, where `x` is an `enum` parameter. Today's rule, unchanged.

What is **not** allowed is building an identifier from caller bytes:
`"sales_" + params["region"]`.

### 2.3 A sentinel probe, and what it does not prove

Detecting §2.2's forbidden case by scanning the emitted template is the mistake
P4.5 already paid for. The gate is **behavioural** instead, in the same shape as
"render twice, opaque vs string":

**Publish runs the script with a distinctive sentinel as each parameter's value and
checks the sentinel does not appear in the emitted template.** If the script
concatenated the parameter into the SQL text, the sentinel is there. If it used the
parameter only for control flow, or passed it through `{{ }}`, it is not.

**Corrected 2026-08-27, measured during Task 4.** An earlier draft said this costs
"one more assignment, not new machinery", by folding into M1's probe assignments. It
cannot: `render::probe_assignments(sql, ctx)` derives its assignments **from the SQL**,
by harvesting the literals a definition compares against — and when `build_sql` runs,
that SQL is precisely what does not exist yet. They are also different operations on
different objects: M1's assignments drive *renders of one template*, this drives
*evaluations of one script*. So the gate needs its own pass, at `2 × params` evaluations
per scripted statement.

What **does** fold is §2.4: the unreferenced rule reads `emitted != baseline` off the
runs this gate already performs, at zero extra cost. And the pass needs its own bound —
`MAX_DECLARED_PARAMS` lived downstream of generation, so nothing capped the probe count
until Task 4 moved a check ahead of it. Worst case is `MAX_STATEMENTS × MAX_DECLARED_PARAMS × 2`
= 1024 evaluations, measured at 524 ms in a debug build.

**Stated plainly, because a limit that reads like a guarantee is worse than a
documented absence:** a script that *transforms* the value first — uppercases it,
hashes it, slices it — defeats the sentinel. The probe catches an author creating
an injection hole **by accident**, which is the case that actually happens. It does
not defend against an author who sets out to build one.

That residue is acceptable, and it is worth saying why rather than hoping nobody
asks. The author is authenticated, holds `data_api_endpoint:manage`, and is already
trusted to write the SQL. An author determined to expose their own endpoint can do
so; the same is true of one who grants too much access. The threat this design must
stop is the **caller**, and against the caller the property is unchanged: values are
binds, identifiers are the author's.

**Amended 2026-08-31 by production-readiness package 5 — read this before quoting
the paragraph above.** The last sentence was the weak point: "against the caller the
property is unchanged" was true of `{{ }}`, and *not* true of a script that
concatenates. Both escape routes above hand the **caller's own bytes** into SQL text,
which is a caller-controlled hole with an author-side trigger, not an author-only
one.

The probe is **unchanged** — deliberately, and both tests that pin its residue still
assert it accepts both scripts. Making it scan the emitted bytes harder is the P4.5
mistake this gate is shaped to avoid, and a probe cannot prove a negative anyway: it
can show a value *did* reach the text, never that one never will. Package 5 closes
the caller's half **by construction** instead, in
`check_script_parameter_constraints`: on an endpoint where any statement is a
`script`, every `string` parameter must carry a `pattern` whose language is inside
ASCII letters, digits and `_`. Then it does not matter which route a script takes —
transform the value, refuse the sentinel, or hide the concatenation behind a branch
no probe reaches — the value is a bare word either way. See package 5's own section
in `2026-08-30-dataapi-production-readiness-design.md`.

### 2.4 What "unreferenced" means for a generated statement

**Decided 2026-08-27, after Task 3 found the collision.** `UnreferencedParameter`
says *"parameter `x` is declared but never referenced as `{{x}}` in the query"*. It
exists so an author cannot declare a parameter the SQL ignores — a caller supplies
it and nothing happens.

A script breaks the test without breaking the rule. `if params["grain"] == "daily"`
**uses** `grain`, and the emitted template may contain no `{{ grain }}` at all. Applying
the written-SQL test to generated text would refuse the branching case this package
exists to enable — and refusing it is not conservative, it is wrong.

So for a **scripted** statement the rule becomes behavioural, on the probes §2.3
already runs: a parameter is unreferenced when **varying it changes neither the
emitted template nor the binds**. That is the same question the written-SQL rule asks,
asked in the only way a script can answer it.

Two consequences worth stating:

- The check costs nothing extra — it reads the probe runs the sentinel gate performs.
- It is **weaker than the written-SQL rule in one direction**: a parameter used only
  inside a branch the probe budget never reaches reads as unreferenced. That is the
  same bounded-probe gap §3 records, and the same one M1 lives with.

## 3. The column contract needs nothing new

This is the part that makes the feature cheap, and it is worth being explicit.

**A generated statement's SQL is a pure function of `params`.** The script never
sees a row — that is C2's transform, a different phase. So the situation is exactly
the one M1 already solved for `{% if %}` branches: the output shape varies with the
parameters and with nothing else.

So publish runs the script under M1's existing probe assignments, `describe_as`
describes each resulting statement, and the endpoint freezes **the union of the
columns across every probe**, with `required` marking those present in all of them.
Serving checks against that union and fails closed, unchanged.

M1's recorded gap carries over unchanged too, and must not be described as smaller
than it is: the probe budget is bounded (`MAX_PROBES`), and a definition can branch
on more combinations than it covers. A script can branch on more still. The backstop
is the same one that already exists — `shape_rows` refuses a column nobody declared.

## 4. Where it sits, and what it composes with

- **Per statement, not per endpoint.** M2 gave each statement its own name, SQL and
  frozen column contract. A generated statement is one of those, so `StatementSpec`'s
  `sql` becomes *either* literal text *or* a script. One script producing all N
  statements would collide with the per-statement snapshot for no gain.
- **Composes with C2's transform**, which runs after the statements execute. They are
  different phases and do not interact. An endpoint may have both, either, or neither.
- **The single-statement gate applies unchanged.** A script's output goes through
  `reject_unscannable_bytes`, the comment/literal scan and the post-render check, so
  `#` is forbidden and `;` is refused everywhere it is refused today. **A script
  cannot produce a multi-statement definition.** If a test in
  `crates/egret-dataapi/tests/template_render.rs` appears to need editing for this
  feature, the change is wrong.
- **The evaluator is C2's**, unchanged: `Globals::standard()`, `enable_load: false`,
  the tick/heap/callstack budgets, the panic containment, and the single
  `AstModule::parse`. A second parse site is refused by an existing test.

## 5. Limits

Reuse C2's where they apply. Two are specific to this feature and both need
**measuring**, not choosing:

- **The emitted template's length.** A script that returns a megabyte of SQL is not
  a statement an engine should be asked to plan. C2's script cap is 8 KiB against a
  ~200-byte example; size this against a real generated statement.
- **Where the evaluation runs.** C2 measured that a Starlark evaluation on the async
  worker is worth avoiding, and moved preview to `spawn_blocking`. Generation happens
  on **every served request**, which is a stronger case than preview's. Decide this
  with the measurement, not by analogy — and note that `/validate` and `publish`
  still evaluate inline today, which this package is the natural moment to fix.

## 6. Preview

Preview already runs one statement. A generated statement previews by running the
script, showing **the SQL it produced**, and then executing that.

**Showing the generated SQL is the point, not a nicety.** An author debugging a
branching script needs to see the statement, and preview is the only surface that
can show it — C2's transform could not be previewed at all, and the honest answer
there was to say so. Here the honest answer is better: the artefact is available and
should be on screen.

The rule from C2 holds: preview, publish and serving must reach the same generator
through the same function. If the plan finds itself writing a second generation path
for preview, the design is wrong. That seam has failed twice in this programme.

## 7. Explicitly not in this package

- **`db.query`, writes, outbound HTTP.** Dropped, not deferred. This package is what
  replaces them.
- **A script choosing the connection.** The database is the endpoint's, frozen in the
  snapshot; a script selecting one would be a new authority decision and is exactly
  the router/runtime line.
- **Sharing generator scripts between endpoints.** A real want and a different
  feature; one script per statement version keeps the snapshot self-contained.
- **Generating the parameter list.** Parameters stay declarative (C1). A script that
  invented its own parameters would sit upstream of every check that makes publish
  and serving agree.

## 8. What would make this spec wrong

Written down so it can be checked rather than assumed:

- ~~If the emitted template turns out **not** to go through the identical render
  path as hand-written SQL, §2.1's "the path is unchanged" is false.~~ **Checked
  while writing this spec — it does.** `render(sql: &str, ctx: &ParamValues)`
  (`render.rs:771`) is the single public entry, and serving calls it as
  `crate::render::render(&stmt.sql, &resolved.values)` (`router.rs:1076`). Generating
  `stmt.sql` and passing it to that same call is literally the same path.
  `render_permutations` (`render.rs:1275`), which M1's probes use, takes `&str` too,
  so §3 holds for the same reason.

- **One consequence to plan for, not a risk:** `StatementSpec` is
  `{ name: String, sql: String }` and is a `ToSchema` type, so making `sql` *either
  text or a script* moves `openapi.json` and the generated client. That gate lives in
  the `egret-server` crate and was silently red for six consecutive tasks in C1 —
  regenerate and report it separately.
- If a script's SQL can depend on anything other than `params`, §3 collapses and the
  contract cannot be frozen from probes.
- If the sentinel probe cannot be threaded into M1's existing assignments without a
  second evaluation path, §2.3 costs more than "one more assignment" and the
  trade-off should be re-argued.
- If per-request generation on the serving path measures badly enough to need a
  cache, that is a design change (a cache keyed by params, invalidated by version)
  and belongs in the plan, not bolted on during it.
