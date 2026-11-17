# Governance SP5 — one grant model — design

> Sub-project 5 (the capstone) of the governance-layer programme. The authoritative
> target is the concepts spec `2026-11-02-governance-layer-concepts.md` (merged
> #824, D1–D10). SP1 (neutral IR) #833, SP2 (Trino OPA) #838/#839/#844, SP3
> (enforcement points) #854…#865, SP4 (service accounts as principals) #867/#869/
> #870/#871 are all done. SP5 is what SP3 explicitly deferred.

## What SP5 is (concepts spec, sub-project 5)

**Acceptance:** "one grant store; external Iceberg clients obey common grants and
`withhold`; engines no longer hold `manage` through the shared role."

Three pieces, deferred from SP3 (`gov-sp3` spec §"scope boundary"):
1. **Full privilege vocabulary.** SP3 kept only `read`/`write` in the grant model
   (`perm.grants.actions text[] CHECK (actions <@ ARRAY['read','write'])`, migration
   `0154`). The concepts spec D4 is `BROWSE, READ, WRITE, CREATE, MANAGE` + one
   `OWNER` per securable. `READ/WRITE/CREATE/MANAGE` each imply `BROWSE` on the
   object and its ancestors; grants inherit down the tree; deny wins; no priority,
   no exclude. `OWNER` = exactly one per securable, default the creator, holds every
   privilege on it and below, transferable.
2. **Catalog merge (one grant store).** `catalog.catalog_grants`/`catalog_roles`/
   `catalog_clients` become the SAME common grants on securables; `catalog_roles`
   removed; engines get their own credential instead of the shared `engine` role
   that carries `manage`. A data migration with provenance + undo.
3. **iceberg-rest enforcement point.** A new EP `kind = iceberg-rest`: the catalog
   (Lakekeeper) itself enforces common grants + `withhold` — it denies `READ` on any
   table that has a mask, row filter or withhold, to everyone but the exemptions
   (masks/row-filters can't run at the storage layer, so withhold is the control) —
   plus STS credential scope. This closes the SA2-noted gap that a catalog credential
   reads Iceberg directly, outside DG.

## Decomposition (decided 2026-11-16): three packages, built SP5a → SP5b → SP5c

Dependency: catalog grants use create/manage/owner-level privileges, so the catalog
merge needs the vocabulary first; the iceberg-rest EP enforces the merged common
grants, so it needs the grant store unified. Hence the order.

| Package | Scope | Acceptance |
|---|---|---|
| **SP5a — Full privilege vocabulary** | add `browse/create/manage` to the grant actions + an `OWNER` per securable; thread them through the neutral IR, the capability matrix, and the compilers (Ranger/OPA map what they can, lower the rest with a reason). No behaviour change for existing read/write grants. | the new privileges exist end-to-end (grant → IR → Ranger/OPA), byte-identical for the existing read/write corpus; OWNER modelled; NotEnforced reasons generated for unmappable privileges. |
| **SP5b — Catalog merge (one grant store)** | fold `catalog_grants`/`catalog_roles` into `perm.grants` on catalog securables; remove `catalog_roles`; engines get their own credential (no shared `engine` role with `manage`); data migration (extend `perm.migrated_from`) + undo. | one grant store; the embedded-catalog RBAC is gone; engines keep working on their own credential; migration reversible while the switch is open. |
| **SP5c — iceberg-rest EP** | new EP kind `iceberg-rest`; Lakekeeper enforces common grants + `withhold` (deny READ on masked/filtered/withheld tables bar exemptions) + STS scope; capability matrix entry; live-check on the demo stack. | an external Iceberg client obeys common grants and withhold; a masked/filtered table denies READ at the catalog; live-verified. |

## SP5a — Full privilege vocabulary (authoritative design for the first package)

**Goal:** make `BROWSE/CREATE/MANAGE` first-class grant actions and model `OWNER`
per securable, carried through the neutral IR, the capability matrix and the
compilers — with zero behaviour change for the existing `read`/`write` corpus.

**In scope:**
- **Grant actions:** extend `perm.grants.actions` from `{read,write}` to
  `{browse,read,write,create,manage}` (migration: relax the `0154` CHECK). An
  empty-to-these superset; existing rows unchanged.
- **Implication + inheritance in the IR:** `read/write/create/manage` imply
  `browse` on the object and its ancestors; grants inherit down the securable tree
  (the IR already inherits — extend it to the new actions). Deny wins, no priority.
- **OWNER:** model "exactly one owner per securable, default the creator,
  transferable, holds every privilege on it and below." Decide the representation
  in the plan — most likely a dedicated owner record per securable (NOT an actions
  entry, since it is a single-holder, transferable role), seeded from the catalog's
  recorded creator where one exists, else the platform-admins group (concepts spec
  "Migration principles → Owners of existing securables"). SP5a introduces the owner
  concept + its grant-expansion in the IR; the data backfill of existing securables'
  owners may land here or in SP5b (the plan decides, noting the dependency).
- **Capability matrix + compilers:** each target (Ranger, OPA) declares which of the
  new privileges it can enforce; the IR lowers an unenforceable privilege to a
  reason (`NotEnforced`, generated from the matrix — the SP1 mechanism). Ranger/OPA
  map `browse`→(the engine's "show"/metadata right if any), `create`/`manage`→the
  engine's DDL/ownership rights where they exist, else lowered. The byte-identical
  guarantee holds for the existing read/write corpus (new privileges only appear
  where a grant uses them).

**Out of scope (later packages):** the catalog grant merge + removing `catalog_roles`
(SP5b); the iceberg-rest EP (SP5c); any UI for granting the new privileges (a thin FE
follow-up, scoped separately unless trivial).

**Verification (SP1-style):**
- IR unit tests: `browse` implication, inheritance of each new action, deny-wins with
  the new actions, capability lowering for an unmappable privilege, OWNER expands to
  all privileges on the subtree — each with a shape test.
- **Byte-identical**: the Ranger target through the IR is byte-identical to today on
  the whole corpus + the demo stack, because no existing grant uses the new actions
  (re-bless only if a golden legitimately gains a new-privilege rule).
- Differential (`ranger-differential`) stays green; `explain` matches the enforcer.

## Global constraints

- Branch `feat/gov-sp5a-vocab` off main `a251f238`. `CARGO_TARGET_DIR=target-sp5`
  on every cargo command. Throwaway Postgres; DB-gated tests MUST run, not skip.
- Chat VI; code/commits/docs EN. Co-author `Claude Opus 4.8`. Conventional Commits;
  `Closes #N` only in PR body. Plain `git commit`; stage by explicit path. No secrets.
- Migration numbers are a shared namespace — renumber above main AND open PRs at merge.
- Model-first (the governance-layer direction): new privileges live in the neutral
  IR + capability matrix, not as Ranger-only concepts. Each package merges on green
  CI after the user's word; docs land in egret-data/docs.
- SDD per code package; opus whole-branch review (it has repeatedly caught
  cross-cutting bypasses the task reviews missed — e.g. SA4 `replace_platform_roles`).

## Programme close

SP5 completes the governance-layer vision: one grant model, every enforcement point
(engine plugins, OPA, catalog) compiled from one neutral IR, catalog access governed
like engine access. After SP5c, sub-projects 0–5 are all done.
