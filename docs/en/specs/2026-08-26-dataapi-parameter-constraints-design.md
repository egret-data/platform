# Data API — declarative parameter constraints (package C1)

**Status:** design, not yet planned
**Programme:** DBAPI parity, `U → E → Z2 → Z1 → M → C → D → Z3`. This is the first
half of **C**; the transform half (Starlark) gets its own spec.
**Depends on:** M2 (`ebf3791c`), which made `statements` plural and froze a column
contract per statement.

## 1. What this adds, and why it is not a script

An endpoint author can already say a parameter's `type`, whether it is `required`,
its `default`, and — for `Enum` — the exact `allowed` list. They cannot say *"a
number between 1 and 100"*, *"a string of at most 10 characters"*, or *"something
shaped like an email address"*.

DBAPI expresses those with a script, and the reason is visible in its own guide:
**DBAPI has no typed parameter model**, so a script is the only place a constraint
can live. Egret has one. Both examples the DBAPI guide gives — `name.length <= 10`
and an email regex — are declarative constraints on a typed parameter, and putting a
language runtime in the request path to express them would buy nothing and cost the
whole runtime.

So C1 adds fields, not a runtime.

### 1.1 The fields

On `ParamSpec`, all optional, all `#[serde(default)]`:

| field | applies to | meaning |
|---|---|---|
| `minimum` / `maximum` | `Number` | inclusive bounds |
| `min_length` / `max_length` | `String` | bounds on **Unicode scalar count**, not bytes |
| `pattern` | `String` | a regex the whole value must match |
| `not_after` / `not_before` | `Date`, `Timestamp` | inclusive bounds |

`Enum` gets nothing: `allowed` already is the constraint. `Boolean` gets nothing:
the type is the constraint.

**Length is counted in characters, not bytes.** A `max_length: 10` that a Vietnamese
name fails at seven letters is a bug the author cannot see from the declaration, and
egret's own UI is Vietnamese. Stated here because `String::len()` in Rust is bytes and
would be the accidental implementation.

## 2. The one place these can live

`serving::params::coerce(spec, text) -> Result<Param, CoerceError>` is **already
shared between publish and serving**: `lifecycle.rs:1179` validates an author's
`default` by running it through the same function the request path uses. Its doc
comment says why, and it is the property this design depends on.

**Every constraint goes inside `coerce`.** Not beside it, not in a second pass in
the router. M2 shipped a Critical that came from exactly this shape — publish
inferred a parameter's shape from one statement while serving computed it across
all of them, so an endpoint published green and 500'd on every call. The lesson
recorded from it is that the fix is not another check but a signature that cannot
express the disagreement.

Concretely, that yields three consequences that are requirements, not
side effects:

1. **An author's `default` that violates their own constraint is refused at
   publish**, free, because publish already runs `default` through `coerce`.
2. **A caller's value is refused with the same rule**, free, same reason.
3. **A constraint cannot be added to serving without publish seeing it**, because
   there is only one function.

### 2.1 The detail goes to the author, never into a 400

**Corrected 2026-08-26, after reading `serving/error.rs`.** An earlier draft of this
section proposed a new `ServingError` variant carrying which constraint failed, so a
caller exceeding a `maximum` by one would be told so. That is wrong here, and the
file says why.

`serving/error.rs` enforces a rule: **no parameter name appears in a 400.** The name
belongs to the endpoint's author, not to its caller, and putting it in an error turns
the error surface into a directory of the parameters an endpoint declares — the same
class of oracle that a single `Unauthorized` and a single `NotFound` exist to close.
It is enforced by shape, not by discipline: `ParamInvalid` and `ParamMissing` are unit
variants carrying nothing, so there is no field for a later edit to interpolate out of
habit, and widening the type is a decision someone has to take deliberately.

So the resolution is smaller than the draft's:

- **Serving keeps `ParamInvalid` exactly as it is** — unit variant, no name, no bound,
  no constraint kind. A constraint violation and a type violation ask the caller for
  the same thing: *send a different value*. That is already what `ParamInvalid` means,
  and the existing 400 vocabulary ("send it" vs "send something else") already covers it.
- **`CoerceError` does gain a variant** carrying which constraint failed and its bound
  — but only so **publish** can report it. Publish is the author reading an error about
  their own endpoint; there is no oracle there, and "your `default` of 5 is below the
  `minimum` of 10 you declared" is exactly the message that saves them a round trip.
- **Serving discards that detail** at the `CoerceError → ServingError` mapping
  (`params.rs:263`), deliberately and with a comment saying so.

There is therefore **no user-visible error-shape change in C1** for callers, and a
better error at publish for authors.

The bound is not secret from a legitimate client, which is why this costs nothing: §4
publishes `minimum`/`maximum`/`pattern` in the generated OpenAPI document. The rule is
about not turning the *serving* path into a discovery channel for people who cannot
read that contract.

### 2.2 Publish's probe values deliberately ignore these constraints

Verified while writing this spec: outside `coerce`, a `Param` is constructed in only
two kinds of place — publish's synthetic probe values (`representative_value`, and the
`true`/`false` pair used to probe a boolean) and read-only conversions. No caller value
reaches a `Param` by any other route, which is what §2 rests on.

`representative_value` yields `Param::I64(1)`. Under `minimum: 100` that value
**violates the author's own constraint**, and it must stay that way.

Probe values exist to decide *which branches a definition takes*, and the branches taken
are what M1 freezes as the endpoint's column union. Making the probes constraint-aware
would change which branches are probed, and therefore silently change a published
column contract — a much worse outcome than a probe value that would not survive a real
request. This is called out because "the probe violates the constraint" reads like a bug
to anyone who finds it later.

## 3. `pattern`, and the one real safety question

`pattern` is written by the **author** and evaluated against the **caller's** input.
With a backtracking engine that is a denial-of-service surface: an author writes
`(a+)+$` without malice, a caller sends thirty `a`s and a `b`, and the request path
burns CPU. The author does not have to be hostile for this to happen.

**Use the `regex` crate** (already at `1.13.1` in `Cargo.lock`). It compiles to
finite automata and has no backtracking, so match time is linear in the input and
this class of failure does not exist. As with the Starlark decision in the paused
transform spec: **the safety property is a property of the engine, not of our
configuration** — there is no list of dangerous constructs to get right.

Three things still have to be decided rather than inherited:

- **Compile at publish and refuse there.** An unparseable pattern must be an author's
  400 at publish, never a caller's 500 at serve.
- **Set an explicit `size_limit`.** `regex`'s default compiled-program cap is 10 MB;
  a pattern that legitimately compiles to megabytes is not something an endpoint
  parameter needs, and the cap should be small and stated.
- **Anchoring is implicit.** The value must match in full. `is_match` is a *search*,
  so `pattern: "abc"` would accept `xxabcxx` — which is not what an author writing a
  validation rule means. Anchor it, and say so in the field's documentation, because
  an author who writes their own `^…$` must get the same answer.

**Open, to settle during planning:** whether a compiled `Regex` is cached across
requests. The snapshot is deserialized from `jsonb` per request
(`router.rs:1289`), so a naive implementation compiles the pattern on every call.
Measure `Regex::new` for a realistic pattern against the request's existing cost
before adding a cache — a cache keyed by pattern string is easy, and premature if
compilation is noise next to a Trino round trip.

## 4. The contract has to learn them too

`openapi_gen::param_schema` emits `type`, `format` and `enum` today. A generated
contract that omits `maximum` describes an endpoint that accepts more than it does,
and the generated client will happily send a value the server refuses.

Every field in §1.1 has a direct JSON Schema counterpart — `minimum`, `maximum`,
`minLength`, `maxLength`, `pattern`, and `format: date`/`date-time` with the bounds
expressed as `minimum`/`maximum` on the string form. They must all be emitted.

**This is testable in exactly the way M2's limit was**: the generated document is
committed and asserted byte-for-byte by the **`egret-server`** crate — a different
crate from the one being edited. Any change here regenerates it.

## 5. The editor

The Parameters section of the endpoint editor grows the fields, shown only for the
type they apply to — a `pattern` box on a `Boolean` parameter is noise that
teaches an author the wrong model.

Two things learned from this programme's own defects apply directly:

- **The bound must be visible where the author is stopped.** M2 capped statements at
  16 and put the reason in a `title` on a disabled button — and `disabled:pointer-events-none`
  meant that title never appeared in any browser. Whatever explains a rejected
  constraint has to be readable without hovering a disabled control.
- **Preview must judge the value the same way serving will.** Preview was shape-blind
  for exactly one release and told authors "fine" where publish said "no". Preview
  runs through `coerce`, so it inherits the constraints for free — but that is a
  property to *test*, not to assume, because it is the same seam that has now failed
  twice.

## 6. Compatibility

`ParamSpec` is `#[serde(deny_unknown_fields)]`, and it is also how a stored `jsonb`
snapshot is read back. Adding fields is therefore **forward-compatible and not
backward-compatible**: a binary rolled back to before C1 will refuse to deserialize
any snapshot published after it, as a 500 on read.

That is the standard price of strict deserialization here and it is already
documented on the type. It is called out because it makes C1 a **one-way deploy** in
the same sense M2's envelope was, and the rollback plan is "roll forward", not "roll
back". No existing snapshot changes: every new field is optional and absent means
today's behaviour exactly.

## 7. Explicitly not in C1

- **The Starlark transform half of C.** Its own spec, after C1 has run in
  production. The decisions already made for it — transform-only, `script(params,
  rows) → JSON`, no `db`/`http`/`tx`, publish must run the script to learn the real
  columns, serve-time must re-check and fail closed — are recorded and not reopened
  here.
- **Cross-parameter rules** (*"`to` must be after `from`"*). A real need and a
  genuinely different shape: it is a constraint on the *request*, not on a
  parameter, and it has nowhere to live in `ParamSpec`. Deferred deliberately rather
  than bolted on.
- **Custom error messages per constraint.** The generated message names the
  constraint and the bound; letting an author write prose is a separate question
  about i18n and about what a public endpoint should reveal.

## 8. What would make this spec wrong

Written down so it can be checked rather than assumed:

- ~~If `coerce` turns out **not** to be the only path a caller's value takes to a
  `Param`, §2 collapses.~~ **Checked while writing this spec — it is the only path.**
  See §2.2 for what the other `Param` constructions are and why they stay as they are.
- If `Regex::new` for a realistic pattern costs enough to matter against a request
  that already crosses the network to an engine, §3's "measure before caching"
  becomes "cache", and the snapshot cache is where it goes.
- If the editor's Parameters section cannot show per-type fields without a
  restructure, §5 is a bigger task than it looks and should be split.
