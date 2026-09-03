# Workflow W2 — Expressions

Status: **spec, awaiting approval**
Date: 2026-09-02
Parent: `docs/en/specs/2026-09-01-workflow-programme-design.md` (§4, §9 row W2)
Depends on: W1 (`Property<T>`, the flow model, `spec::validate`)

## 1. Scope

W2 gives `Property<T>` its `render`. It ports Kestra's expression language onto minijinja, and — the
part that makes the port checkable rather than hopeful — builds a **differential oracle** that renders
the same expressions through real Pebble in CI and diffs the results.

It ships no executor and no task handler. After W2 a flow's expressions can be rendered against a
context; nothing yet builds that context from a running execution.

## 2. The surface, measured

Read from `core/src/main/java/io/kestra/core/runners/pebble/Extension.java` at the pinned commit
`96893b589`:

| Kind | Count |
|---|---|
| Filters | **35** |
| Functions | **37** |
| Tests | **1** |
| Custom binary operators | **7** |

Kestra does not own its template engine: `platform/build.gradle:105` pins
`io.pebbletemplates:pebble:4.1.2`. What Kestra adds is this extension — 4,652 lines across
`runners/pebble/{filters,functions}` plus 1,251 lines of glue.

### 2.1 The operators are the hard part

`Extension.getBinaryOperators()` registers seven, and four of them **override operators Pebble already
has**:

```java
"??"   NullCoalescingExpression        "isIn"  InExpression
"???"  UndefinedCoalescingExpression   ">" ">=" "<" "<="   (overridden)
```

minijinja has none of `??`, `???`, and no ternary `? :`. It does have `~`, `is` tests, inline
`a if c else b`, and — verified against its own documentation — **`in` / `not in`**, which is a
candidate target for `isIn`. Whether Kestra's `InExpression` and minijinja's `in` agree on every operand
pair is a question for §4's oracle, not for reading: `isIn` is registered as a *binary operator* in
Kestra rather than a test, and operators are precisely where this port's divergences live.

**`??` and `???` are not the same operator**, and a port that collapses them is wrong in a way no
casual reading catches:

- `??` — evaluate left; **non-null** → left; **null** → right; **attribute missing** → right.
- `???` — evaluate left and return it **even when null**; only an **attribute missing** falls to right.

**The overridden comparisons are a live divergence.** `GreaterThanExpression` adds string comparison on
top of Pebble's numeric-only default:

```java
if (left instanceof String sLeft && right instanceof String sRight) {
    return sLeft.compareTo(sRight) > 0;
}
```

minijinja's `>` on two strings may or may not agree. That is not a question to settle by reading — it
is exactly what §4's oracle exists to answer, for all four operators and for mixed-type operands.

## 3. Strategy

**minijinja, plus a tokenizer-based expression rewrite, proven by a differential oracle.**

Rejected, with reasons:

- **A hand-written Pebble-compatible engine in Rust.** Full fidelity by construction, but it puts a
  from-scratch template engine on egret's SQL-injection surface. `egret-dataapi/src/render.rs`'s safety
  argument rests on minijinja internals — `set_formatter` plus the `is_default_formatter()` fast-path
  trap it documents at length. Rebuilding that from zero to gain expression fidelity is the wrong trade.
- **Calling real Pebble at runtime through the W5 plugin host.** Perfect fidelity, but expressions are
  rendered at every task property and every loop iteration. An RPC per render is the wrong shape.
- **Patching minijinja's parser to add the operators.** Vendoring a fork of the crate egret's SQL safety
  depends on is a maintenance liability aimed at the wrong risk.

### 3.1 The rewrite must tokenize, not regex

`a ?? b` → `kestra_null_coalesce(a, b)` and the ternary → `a if c else b` are simple as *transforms*
and dangerous as *string substitutions*: `??` inside a string literal, nested ternaries, and `???`
overlapping `??` all break a regex. The rewriter reads the expression as tokens — string literals,
parens, operators — and rewrites at the AST-ish level.

Its correctness is not asserted; it is **measured** by §4, including for every expression in the corpus
that contains an operator inside a string literal.

`kestra_null_coalesce`/`kestra_undefined_coalesce` are plain function calls, not filters (`a |
kestra_null_coalesce(b)`), because Kestra registers `??`/`???` as *operators* with lazy right-side
evaluation, and neither a minijinja filter nor a minijinja function receives an unevaluated argument —
there is no "don't evaluate the right side unless needed" hook a text-only rewrite can reach into
either way. The call form was chosen for Task 2 because the brief that shaped it described the target
as "calls to two custom functions"; giving them the actual short-circuit/missing-attribute semantics is
whichever later task registers them, not this rewrite.

### 3.2 Dependencies

| Need | Crate | Note |
|---|---|---|
| `jq` filter | `jaq` 3.1 | pure Rust, maintained (updated 2026-08) |
| `toIon` / `fromIon` | `ion-rs` 1.0 | Amazon's official Rust implementation |
| **Java date patterns** | **none exists** | must be hand-written |

The third is the expensive one. `{{ d | date("yyyy-MM-dd'T'HH:mm:ss") }}` is a Java `DateTimeFormatter`
pattern — not strftime, not chrono's syntax, with its own quoting rules for literals. No crate provides
it. A translator from Java patterns to chrono is W2's largest single piece and its most likely source of
silent divergence, which is the argument for building §4 first.

## 4. The oracle, and why it is built first

The W5 spike proved a standalone Pebble engine can be constructed with **no Micronaut**:
`io.kestra.core.runners.pebble.Extension` has a public no-arg constructor, so a raw `PebbleEngine` with
Kestra's extension is a few lines of Java. That is the whole oracle.

**Shape.** A corpus of expressions plus a context, held as data in the repository. A small Java harness
renders every entry through real Pebble and writes a golden file. A Rust test renders the same corpus
and diffs. The corpus is the artifact; the harness only regenerates it.

**Order.** The oracle lands in the package's **first** task, before any filter is ported. A filter
written against a guess and then checked is a filter whose author has already decided what the answer
is; a filter written against a golden value is not.

**What it already caught, before any of it was written.** During the W5 spike the standalone engine
rendered:

```
{{ json | jq('.a.b') }}   =>  [42]
```

A **list**, not `42`. A hand-written Rust `jq` filter would plausibly return the scalar, render something
that looks right, and never fail a test written from the same assumption.

**CI.** The Java harness needs a JDK, which CI may not have. The golden file is therefore **committed**,
and the Rust test runs against it with no JVM. A separate, opt-in job regenerates and diffs the golden
file to catch drift when the pinned Kestra version moves — the same shape as
`committed_openapi_json_is_current`, and for the same reason.

## 5. `Property<T>::render`

W1 defined `Property<T>` as raw source with no rendering. W2 adds:

```rust
impl<T: DeserializeOwned> Property<T> {
    pub fn render(&self, cx: &RenderContext) -> Result<T, RenderError>;
}
```

Rendering and conversion are one step, as they are in Kestra: `runContext.render(prop).as(Integer.class)`.
A `Property<T>` whose source contains no `{{ }}` renders to itself — which is what makes the W5 plugin
host's identity-render coherent.

**The SQL seam is not W2's.** `egret.sql`'s rendering goes through `egret-dataapi`'s `bind_formatter`,
where `{{ }}` cannot reach raw SQL text by construction. W2 must not introduce a second path that
renders SQL, and `spec/` and `models/` keep their no-I/O boundary.

## 6. Out of scope

No executor, no task handlers, no execution context construction. `{{ outputs.x.y }}` parses and renders
against a context W2 does not yet know how to build — W3 builds it. Kestra's `secret()`, `kv()`,
`namespaceFile()` and `render()` functions reach services that do not exist here yet; they are ported as
**explicit refusals** naming the package that will implement them, never as silent empties.

## 7. Open questions

1. **How faithful must the four overridden comparison operators be?** The oracle will say whether
   minijinja already agrees. Where it does not, the fix is a custom operator via the rewriter, which
   costs more than a filter. Worth deciding once the measurement exists rather than before.
2. **`fuel`** is already enabled on egret's minijinja. Expression evaluation on author-supplied
   templates should carry a fuel limit, and W2 is where that limit gets a number. Kestra's own guard is
   `VariableRenderer`'s `MAX_RENDERING_AMOUNT`; the two are not the same mechanism and the mapping needs
   a decision.
3. **Corpus ownership.** The golden file is the contract. Whoever adds a filter adds corpus entries in
   the same change, or the filter is unproven — the same rule W1 arrived at for validator checks after
   `Quota`/`Check` fell out of a tagged sweep.
