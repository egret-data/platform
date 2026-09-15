# Workflow W1 — Flow Model & Language

Status: **spec, awaiting approval**
Date: 2026-09-01
Parent: `docs/en/specs/2026-09-01-workflow-programme-design.md` (§9, row W1)
Depends on: W0 (`egret-workflow`, `egret-workflow-api`, `models::State`, the `workflows` entitlement)

## 1. Scope

W1 makes a flow a **first-class, versioned, publishable artifact**. It ships:

- the flow model, ported from Kestra;
- YAML parse + validate;
- `workflow.flows` / `workflow.flow_versions`, mirroring the Data API's draft/version split;
- publish, including **reference resolution** — the security seam;
- the management routes those need.

It ships **no executor, no scheduler, no task handler, and no expression evaluation**. A flow can be
written, validated, versioned, and published in W1; nothing runs it until W3.

## 2. Decisions taken with the user

1. **Hybrid task representation.** Control-flow tasks are a typed enum because the state machine must
   understand their structure; leaf tasks are opaque and claimed by a handler.
2. **Kestra's `inputs:` type vocabulary is ported**, not egret's 6-variant `ParamType`. The user chose
   fidelity over vocabulary reuse, accepting that egret then carries two parameter vocabularies.
3. **Port the whole Kestra flow model**, not a subset — `labels`, `variables`, `outputs`, task `retry`
   and `timeout`, `concurrency`, and the rest.

On (3): porting the *data shape* of `sla`, `checks`, `quotas`, and `concurrency` is cheap — they are
18–198 line value types with no Enterprise gating in core — and declaring them now means the snapshot
format does not change when W3 and W8 give them behaviour. Their *behaviour* is not in W1. A flow that
declares `concurrency` parses, validates, versions, and publishes; nothing enforces it yet, and the
validator says so explicitly rather than accepting it silently.

## 3. Port surface

| Kestra package | LOC | Files |
|---|---|---|
| `models/flows` (incl. `input/`, `sla/`, `check/`, `quota/`) | 3,454 | 53 |
| `models/tasks` (incl. `retrys/`, `common/`) | 3,078 | 51 |
| `models/executions` | 2,826 | 28 |
| `models/triggers` | 1,486 | 23 |
| `models/property` | 660 | 5 |
| **Total** | **11,504** | **160** |

Estimated **3.5–4.5k lines of Rust**. Java model code compresses hard: Lombok `@Builder`/`@Getter`,
`@Schema` OpenAPI annotations, and Jackson polymorphism wiring all collapse into `derive`s.

`models/executions` is ported in W1 even though nothing produces an `Execution` until W3. The reason is
the one already applied to `State` in W0: the types are what W3's pure state machine is written
against, and defining them late would mean writing that machine against a moving target.

## 4. The task model

### 4.1 Hybrid, and why

```rust
pub struct Task {
    pub id: String,
    pub r#type: String,          // Kestra's FQCN is canonical: io.kestra.plugin.core.log.Log
    pub description: Option<String>,
    pub retry: Option<Retry>,
    pub timeout: Option<Property<Duration>>,
    pub disabled: bool,
    pub kind: TaskKind,
}

/// Control flow the state machine must understand structurally; everything else is opaque.
pub enum TaskKind {
    Sequential(Sequential),
    Parallel(Parallel),
    If(If),
    Switch(Switch),
    Dag(Dag),
    ForEach(ForEach),
    Subflow(Subflow),
    AllowFailure(AllowFailure),
    WorkingDirectory(WorkingDirectory),
    /// Anything else: `egret.sql`, `core.http`, a future `io.kestra.plugin.jdbc.trino.Query`.
    Leaf { spec: serde_json::Value },
}
```

The split is not a convenience. W3's `advance()` walks the task tree to decide what runs next, so it
**must** see into `Sequential`/`Parallel`/`ForEach` — a `serde_json::Value` there would force the pure
state machine to re-parse the tree on every call, losing the property §6.3 of the programme spec was
designed around. It equally **must not** see into `egret.sql`: a state machine that knows what SQL is
cannot be a state machine.

`Leaf { spec }` is also the only representation that can hold `io.kestra.plugin.*` when the W5 plugin
host lands, without editing an enum.

### 4.2 `type` resolution

`type` is matched against a registry of known control-flow FQCNs
(`io.kestra.plugin.core.flow.Sequential` and friends, plus the short aliases `core.sequential`, …).
Anything unmatched becomes `Leaf`. **An unknown `type` is not a parse error** — parsing must not depend
on knowing what a task does, or a flow could not be edited before its handler exists.

Publish is where an unknown type is refused, against a **task-type registry that W1 introduces**. That
registry is declarative and carries no behaviour: for each known `type`, the fields it accepts and which
of them are references to egret objects (§8). W3's handlers are then written against it.

The registry has to live in W1, not W3, or the package would defeat itself: with no registry, every
egret task type is unknown, so nothing could be published and W1 would ship a publish route that always
refuses. It is also the seam §13.3 warns about — two halves that can drift — which is exactly why the
declaration side ships first, with the implementation side later required to match it.

## 5. `Property<T>`

Kestra's `PropertyDeserializer` stores **everything as a string**: arrays and objects are re-serialized
to JSON text, scalars become their string form. `Property<T>` is therefore "raw source, rendered and
converted at use time", and the Rust port is a newtype over that source:

```rust
pub struct Property<T> {
    source: String,
    _target: PhantomData<T>,
}
```

W1 defines it and its serde behaviour. **W2 gives it `render`.** Splitting it this way is what lets the
model land before the expression engine, and it is why the W5 plugin host's identity-render is coherent:
a `Property` whose source contains no `{{ }}` renders to itself.

## 6. Namespace, workspace, and folders

Kestra's `namespace` is `@NotNull`, `^[a-z0-9][a-z0-9._-]*`, 1–150 chars. Rejecting it would break the
fidelity rule at its most visible point — every Kestra flow on the internet has one.

- egret **requires** `namespace` with Kestra's exact pattern and length.
- Uniqueness is `(workspace_id, namespace, id)` — Kestra's `(tenant, namespace, id)` with the workspace
  in the tenant's place.
- `folder_id` stays the **UI** organising axis, exactly as `dataapi.endpoints` uses it, and is
  deliberately not derived from `namespace`: folders get renamed and re-parented, and a namespace is
  part of a flow's identity.

Two organising axes is a real cost, and it is the price of paste-compatibility.

## 7. Schema

```sql
CREATE TABLE workflow.flows (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id    uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  folder_id       uuid REFERENCES analytics.folders(id) ON DELETE SET NULL,
  namespace       text NOT NULL,
  flow_id         text NOT NULL,          -- the YAML's `id`; `id` is taken by the surrogate key
  source          text NOT NULL,          -- the YAML the author wrote, byte for byte
  plan            jsonb NOT NULL,         -- the parsed, normalised model
  state           text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft','live','disabled')),
  live_version_id uuid,
  created_by      uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at      timestamptz NOT NULL DEFAULT now(),
  updated_at      timestamptz NOT NULL DEFAULT now(),
  deleted_at      timestamptz,
  UNIQUE (workspace_id, namespace, flow_id)
);

CREATE TABLE workflow.flow_versions (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  flow_id     uuid NOT NULL REFERENCES workflow.flows(id) ON DELETE CASCADE,
  version     integer NOT NULL,
  snapshot    jsonb NOT NULL,   -- source + normalised plan + RESOLVED references, self-contained
  note        text NOT NULL DEFAULT '',
  created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (flow_id, version)
);

ALTER TABLE workflow.flows
  ADD CONSTRAINT flows_live_version_fk
  FOREIGN KEY (live_version_id) REFERENCES workflow.flow_versions(id) ON DELETE SET NULL;
```

Both `source` and `plan` are stored on the draft. `source` is what the editor shows and what round-trips
byte-for-byte; `plan` is what validation and (later) execution read. Keeping only `source` would mean
re-parsing on every read; keeping only `plan` would lose the author's comments and formatting.

Deliberately **not** in W1: `workflow.executions`, `workflow.task_runs`, `workflow.triggers`,
`workflow.task_logs`. Nothing runs yet, and a table with no writer is untested surface.

**Measured 2026-09-14, because "deferred" reads like "arrived later" and for two of
these it did not:** `workflow.executions` and `workflow.task_runs` landed in
migration 0134, and **`workflow.task_logs` and `workflow.triggers` still do not
exist in any migration.** They are **W11** and **W12** in the programme spec's §9,
each recorded there with its measurement.

## 8. Publish, and the security seam

**Every reference a flow makes to an egret object is resolved and workspace-checked at publish, then
frozen into the snapshot. The published artifact contains no unresolved id.**

This is the rule §6.2 of the programme spec sets, and W1 is where it is implemented. The references a
v1 flow can make:

| In the YAML | Resolved against | Port |
|---|---|---|
| `database:` on a leaf task's spec | `query.databases` | `QueryExecPort`-adjacent; needs a `workspace_of` |
| `notebook:` | `analytics` notebooks | `NotebookDocsPort` |
| `dataset:`, `chart:` | `analytics` | existing analytics ports |
| Data API `endpoint:` | `dataapi.endpoints` | `DataApiEndpointPort::workspace_of` |
| `subflow:` | `workflow.flows` | this crate |

Each answers with the object's **own** workspace, and publish compares that against the caller's,
refusing a mismatch as `NotFound` — indistinguishable from "does not exist", so publish cannot be used
to probe which objects exist in other workspaces. That is the convention
`DataApiEndpointPort::workspace_of` already documents, and the reason it exists.

**A flow referencing another workspace is unpublishable**, rather than something each handler must
remember to refuse later. Migration 0128 is the recorded case of the alternative.

Because leaf specs are opaque in W1, reference extraction is driven by a **declared reference schema per
known task type**, not by scanning arbitrary JSON for uuid-shaped strings. An unknown task type
contributes no references and cannot be published — see §4.2.

## 9. Validation

Three layers, in order, and the order is the design:

1. **Parse** — YAML → model. Failures are syntax errors with line/column.
2. **Structural** — unique task ids within the flow; no cycles in a `Dag`; `errors:`/`finally:` handlers
   are not referenced as normal successors; `subflow` does not name itself; every `dependsOn` names a
   task that exists.
3. **Semantic** — `namespace` pattern; input ids unique; `Switch` cases exhaustive-or-defaulted;
   declared-but-unimplemented features (`concurrency`, `sla`, `checks`, `quotas`, `pause`) reported as
   **warnings that name the package that will implement them**, never as silent acceptance.

> **`sla` narrowed on 2026-09-14 (W8 Task 6) and `pause` went earlier (W8 Task 3).**
> `SLA.Type`'s `EXECUTION_ASSERTION` is evaluated now (`egret-workflow/src/sla.rs`);
> `MAX_DURATION` is not. The warning is emitted for a flow that declares a
> `MAX_DURATION` and not for one whose `sla:` is assertions only — warning about
> an enforced feature costs a user the same way silent acceptance does, in the
> opposite direction.

Layer 3's warning channel matters more than it looks: without it, a user writes `concurrency: {limit: 1}`,
sees a green publish, and reasonably concludes it is enforced.

## 10. Routes

Workspace-scoped under `/api/ws/{ws}/workflows`, claimed by the `workflows` entitlement W0 registered:

```
GET    /flows                     list (folder, namespace, state filters)
POST   /flows                     create draft from YAML
GET    /flows/{id}                draft: source + plan + validation report
PUT    /flows/{id}                update draft
DELETE /flows/{id}                soft delete
POST   /flows/{id}/validate       parse + validate without saving
POST   /flows/{id}/publish        resolve references, freeze a version, set live
GET    /flows/{id}/versions       version list
GET    /flows/{id}/versions/{v}   one snapshot
```

`POST /validate` exists as its own route because the editor calls it on every keystroke-pause, and it
must not require a saved draft.

## 11. Crate layout

```
egret-workflow/
  models/
    flow.rs          Flow, Label, Concurrency, Output, SLA, Check, Quota
    task.rs          Task, TaskKind, the control-flow types
    input.rs         Input + the ported type vocabulary
    property.rs      Property<T>
    execution.rs     Execution, TaskRun          (defined here, produced in W3)
    trigger.rs       AbstractTrigger + the trigger types
    state.rs         (W0)
  spec/
    parse.rs         YAML -> Flow
    validate.rs      the three layers of §9
  repo.rs            flows + flow_versions
  publish.rs         reference resolution + snapshot freeze
  routes.rs
```

`models/` and `spec/` touch neither the database nor the network, per §6.5 of the programme spec.

## 12. Out of scope

No executor, scheduler, handler, or expression evaluation. No `workflow.executions`/`task_runs` tables.
No frontend — W7 owns it; W1's routes are exercised by tests and by `curl`.

## 13. Open questions

1. `namespace` gives egret a second organising axis beside folders. Confirmed as the price of
   paste-compatibility, but the UI in W7 has to make it legible rather than confusing.
2. Kestra's `inputs:` vocabulary overlaps egret's `ParamType` without matching it. W1 ports Kestra's.
   Whether the two ever converge is a question for whoever next touches Data API parameters, and it
   should be answered deliberately rather than by drift.
3. Reference extraction needs a declared schema per task type (§8). That registry is introduced in W1
   with entries for the v1 task types, and W3's handlers must keep it in step — the seam is a place two
   halves can drift, which is the failure mode the programme has already hit twice.
