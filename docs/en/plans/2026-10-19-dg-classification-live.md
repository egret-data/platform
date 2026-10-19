# Classification: live measurements

> Canonical (English). Plan: `docs/en/plans/2026-10-19-dg-classification.md`. Spec:
> `docs/en/specs/2026-10-19-dg-classification-design.md` (premises 1–18, C1, C2, CL1–CL17, Q1–Q5). Branch
> `feat/dg-classification`, worktree `egret-dgcl`.

## Measurements (Task 1)

The question: do the premises that reading alone could not settle hold on a real stack? Eight cells: the catalog name
an engine's access request carries (M1), how each policy shape matches each object and what the engine does with it
(M2), whether a hand-written item naming a group or a role reaches anyone (M3), what a BI read check costs (M4),
whether explain names a hand-written decider (M5), the app URL of each BI kind (M6), whether a tag policy matches a
tag name of another case (M7), and what a tag delete removes or refuses (M8).

### 1.0 Predictions (committed before anything was observed)

Written from the code at `07a84415` (main `309405b1` plus the spec and plan commits, which change no code), before any
container of this run was started. A wrong prediction is a finding. Nothing in this subsection is edited after
observing.

**Setup the predictions are about.** egret from this branch with its own DB `egret_dgcl_live` in the private `dgcl-pg`;
Trino 483 (`trinodb/trino@sha256:db58cc93…`) with the Ranger plugin on profile `cltr` (tag service `cltr-tag`) and
StarRocks 4.1.6 allin1 (`5cbf09c4…`, `access_control = ranger`) on profile `clsr`, both polling egret every 5000 ms;
every container and network named `dgcl-`. A registry catalog `ice` attached to both engines, whose engine-side catalog
is named `ice` and holds `s.t (id, email, country)` and `s.u (id)`. Users `ua`, `ub`; egret group `g1` (`ua` a member);
egret role `r1` (held by `ua`). Hand-written policies written while a profile is briefly `manual` (#4a DEV-4). Each M2
policy grants `ua` `select`, `use` and `show` and is the only non-baseline policy present while it is measured.

| id | check | prediction |
|---|---|---|
| M1a | catalog name in each engine's access request for `ice` | the registry name `ice` on both (Trino names a catalog file `<registry name>.properties`, StarRocks runs `CREATE EXTERNAL CATALOG \`<registry name>\``) |
| M1b | a hand-written allow on `catalog = ICE` | works on both: the `catalog` level is `ignoreCase: true` in both service-defs; `catalog = ic3` (another spelling) refused on both |
| M2a | `{catalog: ice}` only | matcher: `ice` Self, `ice.s` Ancestor, `ice.s.t` Ancestor, `ice.s.t.email` Ancestor; engine refuses `SELECT email FROM ice.s.t` on both |
| M2b | `{ice, schema: s}` only | `ice` Descendant, `ice.s` Self, table and column Ancestor; refused on both |
| M2c | `{ice, s, table: *, column: *}` | `ice` Descendant, `ice.s` SelfAndAllDescendants, `ice.s.t` SelfAndAllDescendants, column Self; allowed on both |
| M2d | `{ice, s, t}` no column | table Self, column Ancestor; refused on both |
| M2e | `{ice, s, t, column: email}` | `ice.s.t` Descendant, column Self; the query naming only `email` allowed on both |
| M2f | `{ice, s, table: u (isExcludes), column: *}` | `ice.s.t` SelfAndAllDescendants (t is not u), any `ice.s.u…` None; `email` allowed on both |
| M2g | `{ice, schema: s (isRecursive), table: *, column: *}` | the default matcher ignores `isRecursive`: same as M2c on the matcher and the engines |
| M3a | allow naming group `g1` only (ua a member in egret) | refused on both engines (no group provider, D7) |
| M3b | allow naming role `r1` only (held by ua in egret) | refused on both engines (no roles download, D0) |
| M4 | 500 sequential `object_ref` + `Pdp::authorize` over 500 dashboards in 5 workspaces | admin: p50 ≈ 1.5 ms, p95 ≈ 4 ms, total ≈ 0.8 s; one-workspace user: p50 ≈ 2 ms, p95 ≈ 5 ms, total ≈ 1.2 s; both under 2 s |
| M5 | `POST /perm/explain` for `ua` on each M2 policy the engine applied | `allowed` with `policyName` = that hand-written policy's name (and its id in the attribution as a hand-written row); for M2a/b/d `denied`, `policyName ""` |
| M6 | app URL per BI kind | dataset `/explore/<slug>/datasets/<id>`, chart `/explore/<slug>/charts/<id>`, dashboard `/explore/<slug>/dashboards/<id>`, app `/explore/<slug>/apps/<id>`; saved query and folder: no URL opens one by id |
| M7 | hand-written Trino tag deny on `PII` against a tag named `pii` on `ice.s.t.email` | not applied: `ua` reads `email`; the same policy on `pii` refuses it |
| M8a | delete a gated tag with a table and a column assignment, no rule | 204; no `tag_assignments`/`catalog_tag_assignments` row for it; the clearance deny (`perm-clearance-<tag id>`) absent from the next `cltr-tag` and `clsr` downloads |
| M8b | delete a tag a `perm.rules` row selects | 409 with `this tag is still selected by a permission rule (perm.rules) — delete or retarget that rule first`; nothing removed |

**Stop rules (plan Task 1).** M1 shows a spelling other than `registry.catalogs.name`; M2 contradicts Q2's mapping
(Self/SelfAndAllDescendants applied, Descendant partial, Ancestor not applied); M3 shows a group or role item applied;
M4 exceeds 5 s. Any of these is recorded and reported before Tasks 3–6.
