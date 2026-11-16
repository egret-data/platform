# Governance SP5a — full privilege vocabulary — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make `BROWSE / CREATE / MANAGE` first-class grant actions and model one `OWNER` per securable, carried through the neutral IR, the capability matrix, and the Ranger/OPA compilers — with zero behaviour change for the existing `read`/`write` corpus (byte-identical).

**Architecture:** Extend the `Action` enum (a compiler-enforced chokepoint) and the grant `actions` migration CHECK; add the new actions' engine-capability mappings (or lower them via the matrix); add `browse`-implication + inheritance in the IR; model `OWNER` as a per-securable owner record that expands to every privilege on the subtree. Byte-identical Ranger output for existing grants; `NotEnforced` reasons for privileges a target can't enforce.

**Tech Stack:** Rust, sqlx (Postgres), the SP1 neutral IR + capability matrix.

## Global Constraints
- Branch `feat/gov-sp5a-vocab` (worktree `../egret-sp5`, off main `a251f238`). `CARGO_TARGET_DIR=target-sp5` on EVERY cargo command. Throwaway Postgres `DATABASE_URL` — DB-gated tests MUST run, not skip.
- Chat VI; code/commits/docs EN. Conventional Commits; `Closes #N` only in PR body. Plain `git commit`; stage by explicit path (`target-sp5/` not gitignored — never `git add -A`). Co-author `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. No secrets.
- Migration numbers are a shared namespace (number provisional; renumber above main AND open PRs at merge).
- **Byte-identical guarantee:** the Ranger target through the IR must stay byte-identical to today on the whole corpus + the demo stack, because no existing grant uses the new actions. Re-bless a golden ONLY if it legitimately gains a new-privilege rule (it should not in SP5a). Run `cargo fmt` + `cargo clippy -p <crate> --all-targets -- -D warnings` before each commit.
- Model-first: the new privileges live in the neutral IR + capability matrix, not as Ranger-only concepts.
- **Out of scope** (later packages): the catalog grant merge / removing `catalog_roles` (SP5b); the iceberg-rest EP (SP5c); UI for granting the new privileges (separate thin follow-up).

**Key code anchors (verified on main):**
- `crates/egret-perm/src/model.rs:20` — `pub enum Action { Read, Write }` (the chokepoint).
- `crates/egret-perm/src/actions.rs:741` — `(Action, EngineType) -> Vec<&str>` engine-capability mapping.
- `crates/egret-perm-compile/src/grammar.rs:919` — the `expansions: [(Action::Read,…),(Action::Write,…)]` list.
- `crates/egret-perm-compile/src/explain_effect.rs` — per-action explain (exhaustive match on Action).
- `crates/egret-perm-compile/src/engine_capabilities.rs` — the capability matrix.
- `migrations/0154_perm_model.sql:23` — `CHECK (actions <@ ARRAY['read','write'])`.

---

### Task 1: Extend the `Action` enum + the actions CHECK; map or lower the new actions

**Files:**
- Create: `migrations/NNNN_grant_actions_vocab.sql` (relax the 0154 CHECK to `<@ ARRAY['browse','read','write','create','manage']`)
- Modify: `crates/egret-perm/src/model.rs` (add `Browse, Create, Manage` to `Action`), `crates/egret-perm/src/actions.rs` (the engine-capability mapping), `crates/egret-perm-compile/src/grammar.rs` (the expansion list), and EVERY exhaustive `match Action` the compiler flags (explain_effect.rs, etc.).
- Test: extend `crates/egret-perm/tests/*` / the grammar tests.

**Interfaces:**
- Produces: `Action::{Browse,Create,Manage}` (serde lower-case, matching the DB strings). The grant loader accepts them. Each maps to engine capabilities via `actions.rs` OR is lowered (no engine capability) and surfaced through the matrix.

- [ ] **Step 1: Write the migration** relaxing the CHECK (keep it a superset — existing rows valid). Add a comment citing SP5a.
- [ ] **Step 2: Write failing tests** — a grant with `actions = ['browse']` / `['create']` / `['manage']` round-trips through the loader and the IR; the `Action` enum serializes to the DB strings; the engine-capability mapping returns the expected caps for a target that supports them (define per Step 4) and is lowered where not.
- [ ] **Step 3: Run, verify fail.**
- [ ] **Step 4: Implement.** Add the variants to `Action`; let the compiler enumerate every non-exhaustive match and handle each (explain, grammar expansion, engine mapping). Engine-capability mapping (`actions.rs`): `Browse` → the engine's metadata/"show" right where one exists (Trino has no distinct catalog-level "browse" grant → lower it, or map to the minimal visibility right — CONFIRM what Trino/StarRocks Ranger expose; if nothing, lower with a reason); `Create`/`Manage` → the engine's DDL/ownership rights (Trino Ranger: `create`, `drop`, `alter`, `ownership`; StarRocks: its equivalents — read the Ranger service-def enums already used in `actions.rs`/`grammar.rs`), else lower. Keep `Read`/`Write` output BYTE-IDENTICAL.
- [ ] **Step 5: Run, verify pass. Confirm byte-identical:** run the existing Ranger golden/corpus test — no diff (no existing grant uses the new actions). `cargo test -p egret-perm -p egret-perm-compile`.
- [ ] **Step 6: Commit.** `feat(gov): BROWSE/CREATE/MANAGE grant actions in the IR (SP5a)`

---

### Task 2: BROWSE implication + inheritance of the new actions

**Files:**
- Modify: the IR grant-expansion / inheritance code (find where `read`/`write` inherit down the securable tree and where ancestors are considered — grep `inherit`/`ancestor`/`parent` in `crates/egret-perm-compile/src/` and `egret-perm`).
- Test: IR unit tests with shape tests.

**Interfaces:**
- `Read/Write/Create/Manage` on a securable imply `Browse` on that securable AND each ancestor (you can see what you can use). The new actions inherit DOWN the tree like read/write. Deny wins, no priority (unchanged).

- [ ] **Step 1: Write failing tests** — a `read` (or create/manage) grant on `cat.sch.tbl` yields an effective `browse` on `cat`, `cat.sch`, `cat.sch.tbl`; a `create` grant on a schema inherits to its tables; a deny of `browse` on an ancestor beats an allow below (deny-wins). Shape tests per the SP1 style.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the browse-implication (when expanding a grant, add `browse` on the object + ancestors) and extend inheritance to the new actions. Do NOT change read/write behaviour (regression-guard with the existing tests).
- [ ] **Step 4: Run, verify pass** incl. the existing IR + byte-identical tests.
- [ ] **Step 5: Commit.** `feat(gov): BROWSE implication + inheritance for the new actions (SP5a)`

---

### Task 3: `OWNER` per securable

**Files:**
- Create: `migrations/NNNN_securable_owner.sql` (an owner record per securable — one holder, transferable).
- Modify: the IR load (`ir_load.rs`) to read owners + expand an owner to every privilege on the securable and below; a store/repo for owner read/transfer.
- Test: DB-gated + IR shape tests.

**Interfaces:**
- One `OWNER` per securable: `securable_owners(securable_id/ref PK, principal_type, principal_id, …)` — exactly one row per securable (UNIQUE on the securable). Default the creator; transferable (a later API; SP5a models + reads it). In the IR, the owner expands to `{browse,read,write,create,manage}` (every privilege) on the securable and its subtree, as an allow (deny still wins over it per D6 — CONFIRM the concepts spec: "the owner holds every privilege on it and below"; owner is an allow, a deny elsewhere on the path still beats it unless the concepts spec exempts the owner — read D6 + the owner line and encode whichever it states).
- **Backfill decision (resolve in this task):** seed owners for existing securables from the catalog's recorded creator where one exists, else the platform-admins group (concepts spec "Owners of existing securables"). If the creator data isn't readily joinable in SP5a (catalog securables are SP5b's merge), scope SP5a's backfill to the securables the perm model already knows and DEFER catalog-securable owners to SP5b — state which in the report, and make the migration safe either way (nullable/partial is fine; the owner is additive).

- [ ] **Step 1: Write failing tests** — a securable with an owner grants the owner every privilege on it + its subtree in the IR; exactly one owner per securable (UNIQUE enforced); a deny on the path interacts per the encoded rule; transfer changes the holder.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the owner table + read + IR expansion + the backfill (per the decision above). Byte-identical must hold where no owner is seeded (owner expansion only appears for seeded owners — so on the existing corpus, if no owners are backfilled for the demo stack's securables, no change; if they ARE backfilled, the goldens legitimately gain owner rules → re-bless with the diff shown and justified).
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.** `feat(gov): one OWNER per securable, expands to all privileges on the subtree (SP5a)`

---

### Task 4: Capability matrix + compiler lowering; byte-identical + differential

**Files:**
- Modify: `crates/egret-perm-compile/src/engine_capabilities.rs` (declare, per target × version, which of `browse/create/manage/owner` it can enforce), the Ranger + OPA compilers (map the supported ones, lower the rest to a `NotEnforced` reason via the matrix — the SP1 mechanism).
- Test: matrix lowering shape tests + the differential.

- [ ] **Step 1: Write failing tests** — a target that lacks `manage` enforcement lowers a `manage` grant to `NotEnforced("not supported by <target>@<ver>")`; a target that supports `create` emits the right Ranger/OPA rule; the matrix drives it (not hard-coded). The existing read/write corpus is unchanged.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the matrix entries + the compiler mapping/lowering for the new privileges, generated from the matrix (mirror how NotEnforced is already generated). Keep read/write byte-identical.
- [ ] **Step 4: Run, verify pass.** Run `ranger-differential` (jars 2.5.0/2.8.0) + the byte-identical corpus — green, zero diff on the existing corpus. `explain` must match the enforcer for the new privileges where enforced.
- [ ] **Step 5: Commit.** `feat(gov): capability matrix + lowering for the new privileges (SP5a)`

---

### Task 5: OpenAPI/client regen (if the grant API surface changed) + full gate

- [ ] **Step 1:** if the grant create/read API now accepts/returns the new actions in its schema, regenerate the spec + client (`egret openapi` + `npm run gen:api`); confirm the drift tests pass. If the API is unchanged (actions are a free `string[]` in the spec), note no regen needed.
- [ ] **Step 2:** full gate — `cargo fmt --check`; `cargo clippy -p egret-perm -p egret-perm-compile --all-targets -- -D warnings`; `cargo test -p egret-perm -p egret-perm-compile` (DB-gated). The `routes.rs` TAG_FANOUT_LOCK flake is shared-DB contention — if it's the only failure, re-run it serially on a quiet DB to confirm.
- [ ] **Step 3: Commit** any regenerated artifacts. `chore(gov): regenerate OpenAPI for the privilege vocabulary (SP5a)`

---

## Self-Review
- **Spec coverage:** Action enum + migration + mapping → T1; browse-implication + inheritance → T2; OWNER → T3; matrix + lowering + byte-identical/differential → T4; regen + gate → T5. Out-of-scope (catalog merge, iceberg-rest, FE) named in Global Constraints.
- **Byte-identical:** every task re-runs the Ranger corpus/golden; the guarantee holds because the existing corpus uses no new action/owner (re-bless only a legitimately-changed golden, with the diff justified).
- **Chokepoint:** adding `Action` variants is compiler-enforced — T1 relies on the compiler to enumerate every match that must handle the new actions (no silent fall-through).
- **Open items resolved in-plan (not placeholders):** the exact Trino/StarRocks Ranger privilege names for create/manage/browse (read from `actions.rs`/`grammar.rs`/the service-def enums during T1); the owner-vs-deny interaction (read D6 + the owner line of the concepts spec in T3); the owner backfill scope (T3, with the catalog-securable owners deferred to SP5b if the join isn't available yet).
