# Data API — response transform in Starlark (package C2)

**Status:** design, not yet planned
**Programme:** DBAPI parity, `U → E → Z2 → Z1 → M → C → D → Z3`. This is the second
half of **C**; C1 (declarative parameter constraints) merged as `975a1632`.
**Depends on:** M2 (`ebf3791c`) for several statements per endpoint, C1 (`975a1632`)
for the constraint model this reuses nothing of but sits beside.

## 1. What this is, and the line it does not cross

An endpoint author can reshape the response with a small script:

```python
def transform(params, statements):
    return {
        "region": params["region"],
        "items": statements["rows"],
        "total": statements["total"][0]["n"],
    }
```

That is the whole feature. **No `db`, no `http`, no `tx`, no `cache`, no clock, no
randomness.** pydbapi's `ScriptContext` exposes all of those across ~525 lines; that
is a backend framework, not a Data API.

**The staging, decided already and not reopened here:** 1 transform → 2 `db.query`
(SELECT-only, caller's identity) → 3 writes → 4 outbound HTTP. **The line is between
2 and 3.** Stages 1–2 keep egret a *router*: every read still reaches the engine as
the caller, so Ranger and Cedar still arbitrate, and rollback, cache and retry keep
their meaning. Stages 3–4 make egret a *runtime* holding authority the engine cannot
check. This spec is **stage 1 only**. Stage 2 touches connection pooling, N+1 and the
still-open concurrency cap, and gets its own spec.

## 2. Starlark, and why the choice is not about syntax

`starlark-rust` (Meta/Buck2), over Rhai and rquickjs. The deciding argument:

**The safety property is a property of the language, not of our configuration.**
Starlark has no I/O to remove. A JS sandbox is safe only while the list of globals
you strip stays correct, and that list is a thing you can get wrong once. There is
nothing here to get wrong.

It is also **deterministic** — no clock, no randomness — which is what makes preview
reproducible and makes a cached response mean what it says.

Two costs, stated because they are real:

- **`starlark` is a new dependency.** It is *not* in `Cargo.lock` today — verified.
  This is not a free `regex`-style addition where the crate was already resolved
  transitively. Compile time and audit surface both grow, and the plan should
  measure the build-time delta rather than wave at it.
- The editor is free: `lib/monaco.ts` already registers exactly `sql` and `python`,
  and Starlark is close enough to Python to reuse that definition. Rhai would have
  needed a hand-written Monarch tokenizer.

**Measured, so it need not be re-derived:** `Evaluator` has `set_max_tick_count`
(one tick = a call or a loop backedge — this *is* the fuel), `set_max_heap_size`,
`set_max_callstack_size`, and `set_check_cancelled`, which is the hook a wall-clock
deadline is built from. There is **no built-in timeout**; the deadline is ours to
construct. An earlier note claiming Rhai had the stronger limit set was wrong.

## 3. The hard part: a script's output shape is not inferable

Everything about egret's Data API contract rests on one property: **publish freezes
the columns, serving checks them and fails closed.** Columns come from
`describe_as` — the *engine* describes the SQL statically. That works because a SQL
statement's output shape is a function of the query text and the branch a parameter
takes, never of the data.

A transform breaks that:

```python
if len(statements["rows"]) > 0:
    return {"ok": True, "first": statements["rows"][0]}
return []
```

The shape now depends on **the data**, which publish cannot see. M1 already records a
bounded gap here — `MAX_PROBES` assignments per context, and the absent-parameter
contexts do not include pairs — but that gap is about branches of the *text*. This is
a different kind: a shape that varies with a row count nobody can enumerate.

So "run the script at publish and infer the columns" does not work, and it is worth
being blunt about why: it would produce a contract that is right for the probe data
and wrong for a Tuesday.

### 3.1 The resolution: the author declares the shape, egret checks it

`output.transform` carries a **declared output schema** beside the script, in the
same spirit as `params`:

```json
{
  "transform": {
    "script": "def transform(params, statements): ...",
    "returns": [
      { "name": "region", "ty": "varchar", "required": true },
      { "name": "items",  "ty": "array",   "required": true },
      { "name": "total",  "ty": "bigint",  "required": false }
    ]
  }
}
```

This converts an unsolvable inference problem into a checkable one:

- **Publish** runs the script against the same probe assignments M1 already builds and
  refuses if any run disagrees with `returns`. That catches the ordinary mistakes
  cheaply — it does **not** and cannot prove the script always conforms.
- **Serving** validates every response against `returns` and **fails closed** on a
  mismatch, exactly as `shape_rows` does for SQL columns today. This is the real
  guarantee, and it is the one that holds on a Tuesday.
- The **contract** is generated from `returns`, so it describes what the endpoint
  promises rather than what one probe run happened to produce.

**The new failure mode, accepted deliberately:** a script whose output legitimately
varies will 503 on the shapes the author did not declare. That is the same trade
`shape_rows` already makes and the same reason it exists — an undeclared field
reaching a caller is a field nobody reviewed at publish being served.

`ColumnSpec`'s `required` flag from M1 carries over unchanged and does the same work:
a field may be absent when declared `required: false`, never present when undeclared.

## 4. Where it sits in the pipeline

One transform per **endpoint**, not per statement. Joining `rows` with `total` into a
single object is the actual use, and a per-statement hook cannot express it.

```
statements execute (M2, in order, no shared transaction or snapshot)
        │
        ├─ meta.statements  ← unchanged: what the SQL produced, per statement
        │
        └─ transform(params, statements) → data
                 │
                 └─ envelope (wrapped | bare)  ← unchanged
```

- `statements` is a dict keyed by statement name, each value a list of row objects.
  At N=1 it is still keyed — `{"main": [...]}` — because a script that stops working
  when an author adds a second statement is a trap.
- **`meta.statements` keeps describing the SQL**, not the transform's output. Two
  different truths, kept separate: what the engine returned, and what the endpoint
  promised.
- **`meta.row_count` is the one genuinely open decision here.** Checked while writing
  this: it is computed by `MetaContract::total_row_count()`, summing the statements.
  It is therefore *consistent* with `meta.statements` — both describe the SQL — so
  "it becomes meaningless" would be too strong. The real problem is that a consumer
  reads `row_count` as describing **`data`**, and after a transform `data` may be a
  single object. Two defensible answers:

  - **Keep it**, and make the generated contract say explicitly that it counts the
    rows the SQL produced, not the shape `data` took.
  - **Omit it** for transformed endpoints, so nothing invites the wrong reading.

  **Recommendation: omit it.** M2 already changed that field's meaning without changing
  its type — measured as an exposure to operator SQL and alert thresholds — and a field
  that is technically truthful while reliably misread is the same hazard a second time.
  Settle this in the plan, and whichever way it goes, say it in the contract.
- The **envelope** is untouched. The script produces `data`'s value; `wrapped` and
  `bare` behave exactly as they do now.

### 4.1 `shape` and `naming` are refused alongside a transform

`output.shape` (`objects` | `arrays`) and `output.naming` (`as_is` | `camel` |
`snake`) exist to reshape *rows*. Once a script decides the shape, they are either
dead or ambiguous — does `naming: camel` apply before the script sees the rows, or
to the keys the script wrote?

Publish **refuses** the combination rather than picking. The script receives rows in
one canonical form (objects, engine names) and owns everything after that.

## 5. Limits, and where each refusal lands

Every limit must be a **publish-time or serve-time refusal with a named cause**,
never a hang and never a truncation.

| Limit | Mechanism | Where |
|---|---|---|
| Instruction budget | `set_max_tick_count` | serve → 500, author's fault |
| Heap | `set_max_heap_size` | serve → 500 |
| Call depth | `set_max_callstack_size` | serve → 500 |
| Wall clock | `set_check_cancelled` + a deadline | serve → 500 |
| Script source length | a byte cap | **publish → 400** |
| Output size | a byte cap on the serialised `data` | serve → 500 |

**Why a failing script is a 500 and not a 400.** The caller sent a valid request; the
*endpoint* is broken. Blaming the caller for the author's script is the same defect
`check_defaults` exists to prevent — and it would put the wrong code in `access_log`.

**Open, to settle during planning:** the numbers. Every one of them must be
*measured* — a tick budget picked by intuition is either so low it refuses honest
scripts or so high it is not a limit. C1's precedent: `compile_pattern` was measured
at ~60 µs typical and ~650 µs worst-of-five at the cap before the caching question
was answered, and the measurement is what settled it.

## 6. Preview, and the seam that has failed twice

Preview must run the transform and show its output. Anything less repeats the
programme's most persistent defect: preview was shape-blind for one release and told
authors "fine" where publish said "no", and M2's Critical was publish and serving
disagreeing about a parameter's shape.

The rule that follows: **preview, publish and serving must reach the same evaluator
through the same function**, the way C1 put every constraint inside `coerce` and M2
made `validate_endpoint` *return* the union so a second answer could not be computed.
If the plan finds itself writing a second evaluation path for preview, the design is
wrong.

## 7. Caching and versions

Neither needs new machinery, and it is worth writing down why:

- The script lives **in the snapshot**, so a script change is a new version, and a
  cache key derived from the snapshot already changes with it.
- Starlark's determinism is what makes a cached transformed response honest: the same
  inputs give the same output, always.
- Rollback keeps its meaning because the script cannot write anything. This is exactly
  the property stage 3 would destroy, and the reason the line sits where it does.

## 8. Explicitly not in C2

- **Stage 2 (`db.query`)** — its own spec.
- **Transforming the error path.** A refusal answers with `ServingErrorBody`,
  untouched. A script that could rewrite errors could leak the author's parameter
  names into a 400, which C1 §2.1 exists to prevent.
- **Sharing scripts between endpoints.** A library of transforms is a real want and a
  different feature; one script per endpoint version keeps the snapshot self-contained.
- **A transform on the request side.** Parameters are declarative by design (C1), and
  a script that rewrites inputs would sit upstream of every check that makes publish
  and serving agree.

## 9. What would make this spec wrong

Written down so it can be checked rather than assumed:

- If running the script at publish turns out to need something `validated_draft` does
  not already have — real rows, a live connection beyond `describe_as` — then §3.1's
  publish-side check is more expensive than it looks and should be reconsidered before
  planning, not during.
- If `starlark-rust`'s build-time cost is large enough to matter against this
  workspace's already-slow builds, that is a reason to reopen the language choice,
  not a detail. **Measure it in the first task.**
- If `set_check_cancelled` cannot be driven from a wall clock without a thread per
  evaluation, the deadline design in §5 needs rework.
- If authors turn out to want per-statement transforms more than endpoint-level ones,
  §4's single-hook decision is wrong and the shape of `output.transform` changes.
