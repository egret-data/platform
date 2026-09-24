# SP4a — the egret data-permission model, and its compiler

**Status:** designed
**Date:** 2026-09-24
**Extends:** `2026-09-15-control-plane-dg-review-programme.md` §2.3 and §3 (SP4), on top of SP3 (#587–#593)
**Delivered as three PRs:** SP4a the model and compiler, SP4b the UI, SP4c the migration.

SP3 made `registry.catalogs` the one place a catalog is defined. SP4 makes one place where *access to
it* is defined. This spec covers SP4a: the model, its store, and the compiler that turns it into the
Ranger JSON engines already enforce.

## 1. What the code says that the programme spec did not

Read before designing; each of these changed the design.

- **There is no permission model to replace.** `policy.authz_policies` stores **verbatim Ranger JSON**
  in a jsonb column (`egret-policy/src/store.rs:286`), and a "profile" is one
  `policy.authz_services` row — no separate table, no rendered artefact. Today's editor
  (`PolicyEditorPage.tsx`, 954 lines) is a form over Ranger JSON. SP4a is not a migration between two
  models; it is the first model, with Ranger JSON demoted to output.
- **A tag carries only its name to the engine.** `ServiceTags::from_assignments`
  (`egret-policy-api/src/dto.rs:142-205`) emits `{"type": <tag NAME>}`; the mask that enforces it is a
  *separate* policy authored against that name. This is why renaming a tag breaks enforcement
  silently, and it is why the model must reference `tags.key`.
- **Tags attach to tables and columns only.** `catalog_tag_assignments` is keyed
  `(tag_id, warehouse_id, schema_name, table_name, column_name)` (`migrations/0081`), with
  `column_name = ''` meaning table-level. There is no catalog- or schema-level tag, so a tag selector
  cannot mean "everything in this catalog".
- **StarRocks can mask and filter; it just has no tag service.** Its service-def declares
  `dataMaskDef` (6 mask types) and `rowFilterDef`
  (`egret-policy/src/service-defs/starrocks.json`), while `service_def.rs:22-24` records that the
  plugin has no tag support (upstream `starrocks#67458`) and `tag_service` is only ever computed for
  Trino (`admin/mod.rs:543-553`). The gap is narrower than "StarRocks is unenforced" and wider than
  "StarRocks is fine" — see D5.
- **Ranger's four-way allow/deny/exception structure exists because Ranger has no set difference on
  principals.** A policy item's principal is a set of users, groups and roles with no subtract
  operator, so "the finance group except contractors" is otherwise inexpressible
  ([Ranger deny-conditions and excludes](https://cwiki.apache.org/confluence/display/RANGER/Deny-conditions+and+excludes+in+Ranger+policies)).
  Give the model a subtract and two of the four blocks stop being concepts a user must learn.
- **Ranger's masking and row-filter policies take no wildcards and no multi-values**: one column per
  policy item, items ordered, first match wins, no deny semantics
  ([row-level filtering and column masking](https://cwiki.apache.org/confluence/display/RANGER/Row-level+filtering+and+column-masking+using+Apache+Ranger+policies+in+Apache+Hive)).
  A user on dev@ranger reports exactly this against Trino: "we can only select one catalog, schema,
  table, and column at a time"
  ([thread](https://www.mail-archive.com/dev@ranger.apache.org/msg37731.html)). This is the cause of
  policy-count explosion, and it is what D1's named rules exist to avoid.
- **Ranger's audit breaks at the denial.** The Access tab links a decision to the policy that made
  it, but when nothing matched and the request fell through to default-deny, the Policy ID column
  shows `-`. The most common denial has nothing to click.
- **A custom mask expression is a string Ranger never parses**; it fails at query time, in the engine
  ([Cloudera thread](https://community.cloudera.com/t5/Support-Questions/Custom-masking-expression-using-UDF-in-Apache-Ranger/m-p/170283)).

Not used as evidence: the widely-cited "Ranger needs 75× more policy changes" benchmark is
vendor-sponsored and has a substantive published rebuttal (the benchmark wrote one policy per user
rather than adding users to a policy, and ignored `denyAllElse`). The defensible residue is only
that Ranger's naive authoring path produces policy-per-column and the UI does not steer you off it.

## 2. Decisions

**D1 — Two objects and one operator.** A **grant** says which principals may take which actions on
which scope. A **rule** is a *named, reusable* mask or row filter that selects data by tag or by
path and names who is exempt. Both take a **principal set** with an explicit subtract:

```text
PrincipalSet { include: [user|group|role|workspace-members …],
               exclude: [user|group|role …] }
```

The subtract is the whole reason egret can drop allow-exceptions and deny-exceptions from the
authoring surface. Named rules are Snowflake's shape rather than Ranger's: one `PII → hash` rule
covers every column carrying the tag, instead of one policy per column
([Snowflake POLICY_REFERENCES](https://docs.snowflake.com/en/sql-reference/account-usage/policy_references)).

A grant's **actions** are egret's own small vocabulary, not either engine's: `read`, `write`,
`create`, `drop`. The compiler maps each to the target engine's access types — `read` becomes Trino's
`select` and StarRocks' `select`, `write` becomes `insert, update, delete` on both — because the two
service-defs do not agree (Trino has `use`, `impersonate`, `read_sysinfo`; StarRocks has `node`,
`plugin`, `usage`). A steward should not have to know which engine is underneath, and an action that
one engine cannot express is a compile error naming the engine, not a silent drop.

**D2 — Deny wins, allow unions, and tags do not grant access.** One sentence, true for every
engine: *you can see it if some grant allows it, unless some grant denies it; masks and row filters
always apply, however the access was granted.* A rule's `exempt` set is how an individual escapes a
mask — not a competing grant.

Tags select data for rules and nothing else. This is not a simplification for its own sake: Ranger
evaluates tag policies **first and short-circuits** — a tag ALLOW ends evaluation and resource
policies are never consulted
([Cloudera, tags and policy evaluation](https://docs.cloudera.com/runtime/7.2.17/security-ranger-authorization/topics/security-ranger-tags-and-policy-evaluation.html)).
If egret never emits a tag *access* policy, there is no short-circuit, and D2's sentence is exactly
what the engine does. The model's semantics and the enforcer's semantics agree by construction
rather than by documentation.

**D3 — Assigning a catalog to a workspace grants that workspace read.** Today an assignment changes
only what egret shows; the engine still refuses the query
(`2026-09-19-catalog-registry-design.md:158`). SP4a makes the assignment compile to an implicit read
grant, so one action has one meaning. **This is a behaviour change and must be released as one**: after
SP4a, assigning a catalog gives real access. Explicit grants remain, both to narrow (a deny) and to
widen (write, or a different principal set). The implicit grant is visible in the UI as a grant the
user did not write, with its origin named — never invisible.

**D4 — A profile is generated or manual; generated is read-only.** Engines egret provisions bind to
a generated profile, compiled from the model. Hand-written profiles stay for external engines and
keep today's raw-JSON editor. The programme spec already took this position
(`2026-09-15-…-programme.md:83`); SP4a makes `generated | manual` an explicit column rather than a
convention, because a generated profile that someone edits by hand will be silently overwritten on
the next compile and must say so before it is.

**D5 — egret recompiles StarRocks when tags change; on Trino the plugin does it.** A tag-selected
rule compiles to one tag-masking policy on Trino, which the plugin applies to whatever currently
carries the tag. StarRocks has no tag service, so the same rule compiles to **one resource-masking
policy per matching column**, enumerated at compile time — and that enumeration is stale the moment a
new column gets the tag. egret already bumps the bound service on every tag write
(`egret-catalog-registry/src/tag_version.rs:54`), so egret recompiles the StarRocks profile on the
same trigger. What `ranger-tagsync` does for Trino, egret does for StarRocks.

The cost is real and must be bounded: one rule over a broadly-applied tag becomes N policies on
StarRocks. SP4a enforces a **compile-time ceiling** on expanded policies per rule, refuses the
compile above it with a message naming the rule and the count, and does not silently truncate. The
ceiling's value is set from a measurement on kind, not chosen in advance.

**D6 — Rules reference `tags.key`, grants reference `registry.catalogs.id`.** Both fix documented
breakage: tag policies today store the mutable, non-unique `tags.name`
(`programme spec:43`), and catalog identity now lives in the registry. The compiler resolves key →
name and id → name at compile time, so a rename recompiles instead of breaking.

**D7 — One canonical principal form, and group support is verified, not assumed.** Principals are
written today sometimes as email and sometimes as username (`programme spec:44`), and that spec
*infers* — without verifying — that no group provider or usersync exists, so group rules may never
match inside an engine. SP4a picks one form, and **proves by a live query on kind** whether a group
grant is enforced. If it is not, the UI says so where a group is chosen; a control that silently
does nothing is worse than an absent one.

This decides how D3's implicit grant compiles, and the two must be settled together. If groups do
resolve inside engines, `workspace-members` compiles to a group and membership changes need no
recompile. If they do not, it compiles to **the member list enumerated at compile time**, and adding
someone to a workspace must recompile the profile — the same staleness shape as D5's StarRocks
expansion, with the same answer: egret owns the compile, so egret recompiles on the membership
write. The spec cannot choose between these before the verification, and SP4a's plan must treat the
verification as the task that unblocks the rest, not as a checkbox at the end.

**D8 — The evaluator is built in SP4a, to explain, not yet to enforce.** SP4b's "why can this user
see this?" needs a decision *with provenance*, and the only honest source of that is egret
evaluating its own model. The same evaluator is what SP5 needs to enforce on engines with no Ranger
plugin. Building it once, here, also gives the compiler an oracle: a compiled profile can be checked
against the model's own answer instead of against a JSON snapshot.

Its output names the **reason class**, not just a policy id: `allowed_by_grant`, `explicit_deny`,
`no_grant_matched`, `masked_by_rule`, `filtered_by_rule`. This is the direct answer to Ranger's
audit gap, where a fall-through denial has nothing to link to.

**D9 — What is deliberately not modelled.** Each with its reason, because "Ranger has it" is not
one:

| Dropped | Why |
|---|---|
| Policy conditions (JavaScript expressions) | The Trino service-def declares no `policyConditions` at all, so they are not available on egret's primary engine. Re-open if a real case appears. |
| Delegated admin (the per-policy-item checkbox) | egret already has a permission system (Cedar) and a three-layer management model. A second, per-policy admin plane that nothing reconciles with the first is how Ranger ended up with three. |
| Security zones | They exist to partition one Ranger install between teams. egret partitions by workspace already. |
| Validity schedules and IP conditions | No stated case. Time-bounded access is better expressed as a grant someone removes than as a policy that expires invisibly. |
| Allow-exceptions / deny-exceptions as *authoring* concepts | Replaced by the subtract operator (D1). They remain as a **compilation target** — invisible in the UI. |

## 3. Storage

A new `perm` schema, beside `policy` (which keeps serving raw Ranger JSON for manual profiles).

| Table | Holds |
|---|---|
| `perm.grants` | `effect`, `actions[]`, scope: `catalog_id` → `schema`, `table`, `column` (nullable, wildcards allowed), `origin` (`explicit` \| `workspace_assignment`) |
| `perm.grant_principals` | `(grant_id, kind, ref, mode)` where `mode ∈ {include, exclude}` |
| `perm.rules` | `name` UNIQUE, `kind` (`mask` \| `row_filter`), `body`, `selector_kind` (`tag` \| `path`), `tag_key` or the path columns |
| `perm.rule_exemptions` | the rule's exempt principal set, same shape as `grant_principals` |
| `perm.compiled` | the reverse index: which egret object produced which policy in which profile |

`perm.compiled` is the piece Ranger has no equivalent of. It answers "where is this rule actually
applied?" and, from the other end, "what produced this policy?" — the question an operator asks when
a generated profile surprises them. It is also how a compile knows what to delete: a recompile
diffs against this table rather than dropping and re-creating a profile, so a policy id an engine has
cached does not churn.

## 4. Compilation

One direction only: model → Ranger JSON. Nothing reads Ranger JSON back into the model.

- **Grants** → resource access policies. A principal set's `exclude` compiles to Ranger's
  `allowExceptions` (or `denyExceptions` for a deny grant) — the four-way structure survives as an
  implementation detail and appears nowhere in egret's API or UI.
- **Rules, `kind = mask`** → on Trino, one tag-masking policy per rule when the selector is a tag;
  one resource-masking policy per column when it is a path. On StarRocks, always per column (D5).
- **Rules, `kind = row_filter`** → row-filter policies, same split.
- **Mask bodies** are chosen from the engine's declared `maskTypes`, not free text, wherever the
  intent is expressible that way. A custom expression is allowed but is **parsed and rejected at
  authoring time if it does not parse** — the point being that Ranger never parses it and the author
  finds out at query time.

**The compiler is the security boundary.** Its output is what an engine enforces, so SP4a tests it
against a live engine and not only against expected JSON: a mask Ranger accepts can still fail in
Trino, and a JSON snapshot test would pass while the data leaked.

## 5. What SP4b and SP4c inherit

SP4a deliberately ships without a new UI. It leaves behind:

- the model, its store and its API;
- the compiler, and generated profiles bound to egret-provisioned engines;
- the evaluator, and an `explain` endpoint returning decision + provenance + reason class;
- `perm.compiled`, the reverse index both later phases read.

**SP4b** builds the two-tier UI: intent language by default (a steward says "email is PII; analysts
see it hashed"), with a structural view for an operator who needs to see grants and rules as objects.
It adds the two verbs — *explain one decision* and *enumerate the closure* (who can see this / what
can this user see) — which are distinct questions and distinct screens
([GCP Policy Intelligence](https://cloud.google.com/bigquery/docs/troubleshoot-access-control)).

**SP4c** migrates: existing hand-written policies on egret-provisioned engines are read and offered
as model objects (never auto-converted silently); Cedar classification clearance — today a third
system that gates BI objects only and cannot mask a catalog column
(`programme spec §2.3`) — folds into rules; and the per-workspace notebook role and namespace that
SP3 deferred (`2026-09-19-catalog-registry-design.md:158`) lands here.

## 6. Testing

- A compiler golden test per rule kind and per engine — necessary, not sufficient.
- **Live on kind, both engines**: a grant, a deny, a tag mask and a row filter, each verified by
  running a query as a real principal and checking what comes back. A mask that renders correct JSON
  and does not mask is the failure this catches.
- **The evaluator and the engine must agree.** For each live case, `explain` must return the same
  decision the engine reached. A disagreement is a bug in the compiler or the evaluator, and finding
  it needs both — which is the second reason D8 puts the evaluator in SP4a.
- A test that the StarRocks expansion ceiling refuses rather than truncates, asserting the message
  names the rule and the count.
- A test that renaming a tag does not change enforcement (D6), and that renaming a catalog
  recompiles.
- The group-support verification from D7, recorded as a live result with its date — not as a claim.

## 7. Out of scope

The UI (SP4b). Migration of existing policies, the Cedar clearance fold-in, and the notebook role
(SP4c). Enforcement on engines with no Ranger plugin (SP5) — the evaluator built here is that work's
foundation, but nothing in SP4a rewrites SQL. Ranger audit ingestion; egret's own data-access log is
named for SP4/SP5 in the governance-split spec and is not attempted here.
