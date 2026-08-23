# An endpoint may run several statements, and its schema may vary by parameter

Package **M** of the Data API programme. It does two things that share one cause: both
are constrained by what `EndpointSnapshot` freezes at publish.

Everything asserted about engine behaviour below was measured before it was written.
The probes are `.superpowers/sdd/multi-statement-transaction-probe.md` and
`.superpowers/sdd/frozen-columns-vs-conditional-projection.md`; this document cites them
rather than restating them, and marks what is **not established** as such.

## 1. What was measured

### 1.1 No transaction can span two statements, on any engine

The decisive layer is egret's own, not the engines'. `Connector::execute` stands entirely
alone: StarRocks does `Conn::new(...)` then `disconnect()` *inside* `execute` — not even a
pool checkout — and Trino builds a fresh `reqwest::Client` per call. Two `execute` calls on
one `Connector` returned `CONNECTION_ID()` **33564235** and **33564236**; `SET @probe=42`
read back `null`; `SET autocommit=0` read back `@@autocommit=1` — while every one of those
statements, **including `START TRANSACTION`, returned `OK`**.

There is no session in which to hold a transaction, so the engines' own capabilities are
moot until the `Connector` trait changes. Below that layer both engines fail anyway: Trino
returns a hard `EngineError("Client does not support transactions")` on egret's path,
because the transaction id comes back in an HTTP *response header* that body-only
`TrinoResp` can never read. StarRocks accepts `START TRANSACTION` and does not isolate — a
concurrent commit became visible *inside* the open transaction — and `ROLLBACK` **silently
kept the row**.

**Consequence for this spec:** M runs its statements as N independent reads and **does not
promise consistency between them**. §4.5 says where that is written down for the caller.

### 1.2 Iceberg snapshot pinning does work, on both engines, unmodified

`FOR VERSION AS OF <snapshot_id>` pinned reads through egret's own connector on Trino and
on StarRocks. The M-shaped pair — two *separate* `execute` calls, both pinned — returned
count 2 and exactly 2 rows while the live table held 5.

It is nonetheless **not in M**; §7 gives the reason.

### 1.3 A conditional endpoint publishes green and then 503s on its own default call

This is a live defect on `main`, and M1 exists to fix it.

Output columns are frozen once, at publish: `lifecycle.rs:1546` calls
`describe_as_with_params` with a single set of representative values. Conditional templates
are a deliberate, supported feature — `lifecycle.rs:1505` records that publish renders the
real template through the real renderer precisely so `{% if %}` endpoints can describe at
all. Nothing compares one branch's columns against another's, because nothing ever
describes a second branch.

So this publishes, on both engines:

```sql
SELECT id {% if detail %}, name, email {% endif %} FROM users
```

and then:

| call | result |
|---|---|
| `?detail=true` | `200`, three columns |
| `?detail=false` | `503 {"code":"schema_drift"}`, log `frozen=["id","name","email"] live=["id"]` |
| no query string at all — the documented default | `503` |

The generated OpenAPI does worse than describe the wrong set: it **advertises calls that
cannot succeed**. `detail` is declared `"required": false` while omitting it 503s, and a
`mode` parameter is declared `"enum":["summary","full"]` while `mode=full` 503s.

Which branch freezes is decided by the representative values (`lifecycle.rs:1108`):
Boolean → `true`; Enum → **`allowed[0]`**. **The order the author wrote `allowed` in
decides the published contract.** Reversing `["summary","full"]` to `["full","summary"]`
changes a public schema, silently.

**What saves it from being worse:** `shape_rows` (`serving/router.rs:1113`) refuses on
name/arity mismatch *before* shaping, and takes every response key from `snap.columns`, so
no dropped or mislabelled column is possible. It is a gate, not a formatter. That property
is the reason §3 keeps a declared column contract rather than adopting the industry answer
in §6.

**But types are deliberately not compared**, and that gap is real:
`SELECT id, {% if detail %}name{% else %}id{% endif %} AS v` serves **200** with `v` as an
integer while `meta.columns` and the OpenAPI both declare `varchar` / `"type":"string"`.
Identical on Trino and StarRocks.

### Not established

- Behaviour of a pinned read after Iceberg snapshot expiry.
- Time travel over egret's real REST catalog. It was measured against a JDBC-backed
  Iceberg catalog because the dev `iceberg` catalog is dead — it still points at
  `polaris:8181`, removed from compose, and returns `Cannot obtain metadata`.
- Whether a Trino transaction would pin reads if the headers were carried. Moot under
  §1.1, and not driven.

### An unrelated deployment landmine, recorded because it was seen

egret always sends `X-Forwarded-Proto: https` to Trino. A coordinator without
`http-server.process-forwarded=true` answers **406 to every request**. The dev
coordinator sets the flag, so it does not bite here; a stock Trino image does. One
observation, not investigated further.

## 2. Scope

**In:** several read-only statements per endpoint; a column contract that survives
branching; a fixed response envelope; the type-comparison gap from §1.3.

**Unchanged, and load-bearing:** `ALLOWED_METHODS = ["GET", "POST"]`
(`endpoints.rs:319`); only `SELECT` or a leading `WITH … SELECT` may be published; `#` is
forbidden outright and `;` is rejected at two layers before publish
(`reject_unscannable_bytes`, then the comment/literal-stripping scan at
`lifecycle.rs:601`) and once more after render (`render.rs:856`); OAuth2
`client_credentials` on the existing machine-IdP is the only credential.

**Out:** writes and transactions — package **D is cancelled**, see §7; snapshot pinning;
a script authoring mode; Z1 (IP allow/deny and public endpoints), still deferred.

M ships as two PRs. **M1** is the column contract and fixes §1.3; it touches neither the
envelope nor the statement list. **M2** is several statements and the envelope. M1 first,
because M2's per-statement columns are M1's contract repeated N times.

## 3. M1 — the declared columns are the union of the branches, and serving enforces them

### 3.1 Publish describes every distinct rendered permutation

`probe_assignments` (`render.rs:1213`) already enumerates up to `MAX_PROBES = 64` parameter
assignments for `check_dynamic`, round-robin so the budget degrades evenly across
parameters rather than being exhausted by whichever name sorts first. M1 reuses it, and
adds one step that makes the cost honest:

1. render each assignment — machinery exists;
2. **deduplicate by the rendered SQL string** — new;
3. `describe_as_with_params` each *distinct* SQL — was exactly one call.

A template with no conditionals renders to one distinct string, so it costs **exactly one
describe, as today**. One `{% if %}` costs two. The cost tracks the number of structurally
distinct queries, not the number of parameters.

### 3.2 The contract

```rust
pub struct ColumnSpec {
    pub name: String,
    pub ty: String,
    pub required: bool, // NEW: present in EVERY described permutation
}
```

`columns` becomes the union across permutations, ordered by first appearance. A column
present in every permutation is `required: true`; one that appears only under some branch
is `required: false`. That is exactly the semantics of a sparse response, and OpenAPI
expresses it natively through its own `required` list.

**A name that appears with two different types across permutations refuses to publish.**
Two types cannot be unioned into one honest declaration, and this is the same gap that
serves the silent `200` in §1.3. Refusing is the only answer that is true.

### 3.3 Serving enforces it, and fails closed

`shape_rows` keeps its position as a gate; its predicate changes from equality to
containment, and gains the type check:

- every returned name must be **in** `snap.columns` — otherwise `503 schema_drift`;
- every `required` column must be **present** — otherwise `503 schema_drift`;
- a returned column's type must **equal** the declared type — otherwise `503 schema_drift`;
- response keys come from the columns this execution **returned**, not from all declared.

The 64-probe budget is a cap, not a proof of exhaustiveness: a template can branch on more
combinations than 64. The serving check is therefore the backstop for everything publish
did not reach, and it **must fail closed**. An undeclared column is not a documentation
defect — it is a field nobody reviewed at publish reaching a caller.

### 3.4 What this fixes

The §1.3 table becomes `200` in every row, with `?detail=false` returning `{id}` and
`?detail=true` returning `{id, name, email}`; `name` and `email` are declared
`required: false` and `id` `required: true`; the OpenAPI stops advertising calls that
cannot succeed; and the enum-ordering hazard disappears, because every allowed value is
described rather than only `allowed[0]`.

## 4. M2 — several statements, and one envelope

### 4.1 The statement list

```rust
pub struct StatementSnapshot {
    pub name: String,               // ^[a-z][a-z0-9_]*$, unique within the endpoint
    pub sql: String,
    pub columns: Vec<ColumnSpec>,   // §3.2, frozen per statement
}

pub struct EndpointSnapshot {
    pub statements: Vec<StatementSnapshot>, // replaces `sql: String` and `columns`
    // …everything else unchanged
}
```

**Each `sql` passes the existing single-statement gate, unchanged.** The wire never carries
a `;`. Relaxing that rule to "support multi-statement" would delete a defence built from a
live-proven StarRocks bypass, in which `#` was read as a comment by the scanner and as
nothing at all by the engine. Multi-statement is a property of the *configuration*, never
of a string handed to an engine.

Statements are **named**, not positional. A name is self-describing in the response, and it
does not break when an author inserts a statement in the middle. §6 records that SQLREST
chose position and why we diverge.

### 4.2 Parameters are shared

One request carries one parameter set. `resolve_supplied`
(`serving/params.rs:119`, reached from `resolve` at `:108`) currently takes `&snap.sql` to
decide which declared parameters the template actually uses; it takes the **union across
statements**. A parameter used by only one statement is still declared once, and its
`required` semantics are unchanged.

The change belongs in `resolve_supplied` and nowhere else. Its own comment records why:
serving reaches it through `resolve`, **preview calls it directly**, and that is
deliberate — "there is exactly one place where a declared parameter becomes a `Param`".
Computing the union anywhere upstream would let preview and serving disagree about the
same endpoint. This programme has already shipped one defect of exactly that shape, where
preview reported `who=admin` while the real call reported the service account.

### 4.3 Execution and limits

Statements run **sequentially**, each through its own `Connector::execute`. `max_rows`
applies **per statement**: a count statement returning one row must not consume the row
statement's budget.

### 4.4 The envelope

```json
{ "status": "ok", "message": null, "data": …, "meta": { … } }
```

- **`status`** — `"ok"` or `"error"`. It **mirrors** the HTTP status; it never replaces it.
  egret keeps returning real `403` / `422` / `503` with `ServingErrorBody`. §6 records that
  SQLREST returns `200` for everything and why we do not.
- **`message`** — a human-readable string, `null` on success.
- **`data`** — the rows, when the endpoint has one statement; an object keyed by statement
  name, when it has several. This is not a union type at the contract level: OpenAPI is
  generated per endpoint, so each endpoint's schema is unambiguous.
- **`meta`** — `{ version, cached, row_count, columns }` for one statement;
  `{ version, cached, statements: { <name>: { row_count, columns } } }` for several.
  `meta` is kept rather than folded away because `version` is the only way a client can
  detect that the contract was republished.

**A per-endpoint toggle**, `output.envelope: "wrapped" | "bare"`, default `wrapped`. `bare`
writes `data`'s value alone. This is taken from SQLREST's `USE_SYSTEM_RESPONSE_FORMAT` and
is the one part of its response design worth copying.

**This is a breaking change to a live API.** Today's shape is `{data, meta}`, and P4
endpoints have served real traffic since 2026-08-16. The decision to break rather than
version was taken deliberately: consumers are few and internal, the API is pre-1.0, and
carrying two envelope shapes forever costs more than one migration now. The migration sets
`envelope = "wrapped"` on every existing endpoint, and the change must appear in the deploy
note. It is not silent and must not be made silent.

### 4.5 What the caller is told about consistency

N statements are N independent reads (§1.1). The generated OpenAPI description for any
endpoint with more than one statement states this in the operation description, in plain
words: the statements do not share a transaction or a snapshot, so a total and the rows it
counts may disagree if the table changes between them.

Writing it down is the whole of the promise. §7 records what it would take to make a
stronger one.

### 4.6 The cache key does not change

The key stream is `endpoint(16) ‖ version(4) ‖ params ‖ principal(16)`. The cached value is
the whole response, and republishing bumps `version`, so the statement list is already
covered. Recorded because "the cache key must change" is the obvious guess and it is wrong.

## 5. Authoring

M ships one authoring mode: **SQL + Jinja**, as today, per statement.

This is worth stating because it was a genuine open question. The proposal was a script
that generates SQL, on the grounds that different parameters need different SQL. That
premise is true, and it is already satisfied: `render.rs` re-renders on **every request**,
with `{% if %}`, `{% for %}`, and `{{ sort | ident }}`, and `set_formatter` makes `{{ x }}`
emit `?` and push a bind. Different parameters already produce different SQL, safely.

A script that returns SQL *text* would hand that back — every value it interpolates becomes
raw SQL — and egret has been cut by exactly this, at three call sites, live-proven on
StarRocks. §6 records what SQLREST's script engine is and what it costs.

The door is open on evidence, not on hypothesis: a real endpoint that Jinja cannot express
is the thing that reopens this.

## 6. Where we diverge from SQLREST and DBAPI, and why

Read from `/Users/1pro-l4r8/Documents/github/data-portal/sqlrest` at `e580f60`.

| Question | SQLREST | egret M | Why |
|---|---|---|---|
| Envelope | `ResultEntity{code, message, data}`, `code=0` = success | `{status, message, data, meta}` | Same idea. `meta` added because `version` has no home in three keys |
| Envelope toggle | `USE_SYSTEM_RESPONSE_FORMAT=false` → bare `data` | `output.envelope: "bare"` | Copied |
| HTTP status | Always `200`; `HttpApiServlet.process` never calls `setStatus` | Real `403`/`422`/`503` | Always-200 breaks HTTP caches, proxies, monitoring, and forces every client to parse a body to learn it failed |
| N results | `execute(...) → List<Object>`; `results.size() > 1 ? results : …` — **positional** | keyed by statement name | A name survives an author inserting a statement in the middle |
| Statement storage | One row per statement, `SQLREST_API_CONTEXT(api_id, sql_text)` | `Vec<StatementSnapshot>` in the snapshot | Same shape; egret freezes columns alongside |
| Script | Groovy, `GroovyShell.evaluate` **per request**, no `SecureASTCustomizer`; modules include `DbVarModule`, `TxVarModule`, `HttpModule` | none in M | That is arbitrary JVM execution by design. It is coherent — SQLREST's authors are trusted operators — and it is not egret's trust model |
| Output schema | `OutParam` hand-declared in the UI; `getOutputs()` appears only in Swagger generation, export/import, and save-time `checkValid` — **never in the execution path** | derived from the engine and **enforced** by `shape_rows` | SQLREST cannot detect an endpoint silently changing its columns. `shape_rows` is the property we are not willing to trade for flexibility |

The honest summary: SQLREST and DBAPI sit at the dynamic, unenforced, author-trusted end.
egret sits at the derived, enforced, injection-proof-by-construction end. M moves egret
toward them on the **envelope** — the part that is ergonomics — and deliberately not on the
**contract** — the part that is a gate.

## 7. Deferred, with the reason

- **Package D — writes and transactions. CANCELLED.** §1.1 removed its foundation: there is
  no transaction on any engine under egret's connection model, so "multi-statement with
  rollback" is not a promise that can be made. Reviving it needs a partial-failure design
  first, and a `Connector` trait that has a session at all — not an implementation.
- **Iceberg snapshot pinning.** It works (§1.2) and it is the only route to consistent
  multi-statement reads. It is out of M for three reasons: it is **Iceberg-only**
  (`Unsupported table type for temporal clauses` on anything else), so it cannot be
  promised for a Trino catalog over postgres or mysql; the value must reach the SQL as
  **text**, which means a second raw-emission path in `render.rs`, a file whose entire
  security claim rests on there being exactly **one** deliberate hole (`| ident`); and M
  already carries a breaking envelope change. A real pagination user, with a real Iceberg
  table, is the evidence that decides whether the second hole is worth it.
- **A script authoring mode.** §5.
- **Z1 — IP allow/deny lists and public endpoints.** Still deferred; the reason is the edge
  posture recorded in the programme spec §7.
- **Positional statements.** §6.

## 8. How each claim is proven

- **§1.3 is fixed, not merely covered.** The failing case is driven end to end before the
  fix and after it: publish the `{% if detail %}` endpoint, call it with the parameter
  omitted, and require `503` before and `200` with `{id}` after. A test that only asserts
  the union is computed would pass with `shape_rows` still refusing.
- **The type-conflict refusal is proven by mutation.** Stub the type comparison and require
  the publish test to fail. Six assertions in the chart programme survived mutation; this
  one must not.
- **The describe count is measured, not reasoned.** A non-conditional template must issue
  **exactly one** `describe_as_with_params` — asserted as a count, not as "at least one" —
  or the dedupe step is not doing what §3.1 claims and every publish silently costs more.
- **The single-statement gate is proven still closed.** The existing `;` and `#` rejection
  tests run unchanged against the new statement list, per statement. If any of them needs
  editing to pass, the change is wrong.
- **The envelope break is proven at the boundary.** An endpoint published before the
  migration and called after it returns `{status, message, data, meta}` — asserted against
  the serving route, not against a formatter unit test.
- **Run it on kind and check it in the browser before calling it done.** On this programme,
  every live check has found a defect that the suites did not: CSV headers reading
  `column_1`, an editor sending `database_id: ""`, a CSRF layer killing a `POST` in
  production that no router-level test could see.

## 9. Programme order after M

`M → C → Z3`, with Z1 held and D cancelled. C — parameter validation rules and response
transformation — is unchanged by this spec except that it now inherits a per-statement
column contract rather than a single one. Z3 stays last because the export format *is* the
snapshot, and M changes the snapshot.
