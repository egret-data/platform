# SP4a — the egret data-permission model and its compiler — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give egret its first data-permission model — grants and named rules over registry catalogs — an evaluator that can explain a decision, and a compiler that turns the model into the Ranger JSON Trino and StarRocks already enforce.

**Architecture:** A new `perm` schema holds grants and rules; `policy` keeps serving raw Ranger JSON for hand-written profiles. Compilation is one-way (model → Ranger) and reuses the renderers that already exist in `egret-policy/src/admin/ranger_json.rs`, so the compiler's job is to produce `AccessPolicyInput` / `MaskingPolicyInput` / `RowFilterPolicyInput` values, not to hand-write JSON. `perm.compiled` records which egret object produced which policy, so a recompile diffs instead of dropping and recreating.

**Tech Stack:** Rust (axum, sqlx 0.9, utoipa), Postgres, Apache Ranger service-defs, Trino + StarRocks on kind.

## Global Constraints

- **Read the spec first:** `docs/en/specs/2026-09-24-egret-permission-model-sp4a-design.md`. D1–D9 bind every task.
- **Migrations are append-only.** `0154` is the first free number. Never edit `0147`–`0153`; they are applied on the live kind cluster. Editing an applied migration breaks `egret_test_template` with `VersionMismatch` — drop that database and re-run if it happens.
- **sqlx 0.9:** `sqlx::query()` takes `&'static str`. Never build SQL with `format!`; bind parameters.
- **Tags are referenced by `tags.key`, never `tags.name`** (D6). Catalogs are referenced by `registry.catalogs.id`, never by name. Names are resolved at compile time only.
- **egret never emits a tag *access* policy** (D2). Tag selectors drive masks and row filters only. Emitting one reintroduces Ranger's short-circuit and makes the model lie.
- **`allowExceptions` / `denyExceptions` are a compilation target only.** They must not appear in any egret API type, OpenAPI schema, or UI-facing name.
- **A compile that would exceed the StarRocks expansion ceiling refuses**, naming the rule and the count. It never truncates silently (D5).
- **Actions are egret's vocabulary, and SP4a ships exactly two:** `read` and `write`. `read` → `select` on both engines. `write` → `insert, delete` on Trino (it declares **no** `update` access type) and `insert, update, delete` on StarRocks. DDL is out: StarRocks has no bare `create`, only ten level-specific ones. An action an engine cannot express is a compile error naming the engine, never a silent drop.
- Regenerate the committed OpenAPI with the built binary: `<CARGO_TARGET_DIR>/debug/egret openapi > openapi.json`. Never hand-edit. `committed_openapi_json_is_current` must pass.
- Regenerate the frontend client with `npm run gen:api` from `frontend/`, never bare `orval`. `orval.config.ts` sets no `output.clean`: if a generated model orphans, delete the file AND `src/api/generated/model/index.ts`, then regenerate, then confirm `npx tsc --noEmit`.
- `rm` is aliased interactive in this shell; use `command rm -f` in scripts.
- **`cargo fmt --all -- --check` must be clean before every commit.**
- Test environment:
  ```bash
  export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target
  export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
  export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
  ```
  and run with `env -u EGRET_ALLOW_DB_SKIP` — without it, DB-gated tests report `ok` while asserting nothing. Never use the dev database `egret`.
- Never use `git stash`. Never override the git author. Never merge without the user's explicit approval.
- Do not modify the kind `demo-trino` data.
- Commit trailer, exactly:
  ```text
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  ```

---

## File Structure

**New crate: `crates/egret-perm`.** The model, its store, the evaluator and the compiler. It depends on `egret-policy` (for `ranger_json`'s renderers and the store that writes `authz_policies`) and on `egret-catalog-registry` (for catalog names). It must NOT depend on `egret-engine-mgmt`, so the dependency direction matches `egret-policy`'s.

| File | Responsibility |
|---|---|
| `crates/egret-perm/src/model.rs` | `Grant`, `Rule`, `PrincipalSet`, `Action`, `Scope`, `Selector` — types only, no I/O |
| `crates/egret-perm/src/store.rs` | All SQL for the `perm` schema |
| `crates/egret-perm/src/eval.rs` | The evaluator: `Decision`, `Reason`, `evaluate()` |
| `crates/egret-perm/src/compile/mod.rs` | Compile orchestration, `perm.compiled` diffing, the ceiling |
| `crates/egret-perm/src/compile/grants.rs` | Grants → `AccessPolicyInput` |
| `crates/egret-perm/src/compile/rules.rs` | Rules → `MaskingPolicyInput` / `RowFilterPolicyInput`, per engine |
| `crates/egret-perm/src/actions.rs` | egret action ↔ engine access-type mapping |
| `migrations/0154_perm_model.sql` | The `perm` schema |
| `crates/egret-perm/src/routes.rs` | The HTTP surface (mounted by `egret-server`) |

**Modified:**
- `crates/egret-policy/src/store.rs` — add a `generated | manual` column read/write (Task 8).
- `crates/egret-bi/src/catalog_tags.rs` — call the recompile hook on tag writes (Task 7).
- `crates/egret-server/src/lib.rs` — mount `egret-perm`'s router and register its utoipa paths.

---

### Task 1: Verify whether a group grant is enforced inside an engine

**This task produces a measurement, not a feature.** D7 records that the programme spec *inferred* — and never verified — that no group provider or usersync exists, so group rules may never match inside an engine. The answer decides how Task 6 compiles the implicit workspace grant. Do this first; everything else can be built while it is known.

**Files:**
- Create: `docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md` (the recorded result)

**Interfaces:**
- Consumes: nothing.
- Produces: a recorded, dated answer to "does a Ranger group policy match a real user inside Trino?" — Task 6 branches on it.

- [ ] **Step 1: Find out what principal an engine actually sees**

On the kind cluster, run a query as a real user through Trino and read what Ranger was asked about. The backend logs the PDP call; the engine's own session user is what matters.

```bash
command kubectl --context kind-egret get pods -n egret-engines | grep trino
```

Pick a running Trino coordinator, then run a query through egret's SQL Lab as `admin@egret.local` and capture the username Trino reports:

```bash
command kubectl --context kind-egret exec -n egret-engines <coordinator-pod> -- \
  sh -c 'grep -i "user" /var/log/trino/*.log 2>/dev/null | tail -20'
```

Record the exact string. It is either an email or a bare username, and D7 says egret writes both in different places.

- [ ] **Step 2: Create a group-only Ranger policy by hand and test it**

Through egret's existing policy editor (`/governance/policies`), create an access policy on the Trino profile whose only principal is a **group** that the test user belongs to in egret. Grant `select` on one catalog/schema/table the user cannot otherwise read.

- [ ] **Step 3: Run the query as that user and record the result**

Run a `SELECT` against that table as the group member. Record verbatim: the query, the user, whether it succeeded, and the error if not.

- [ ] **Step 4: Distinguish the two failure modes**

If the query is denied, it is not yet proven that *groups* are the problem — the policy might be wrong. Change the same policy to name the **user** directly, re-run, and record. Then:

| user-policy | group-policy | Conclusion |
|---|---|---|
| allows | allows | groups work; Task 6 compiles `workspace-members` to a group |
| allows | denies | groups do not reach the engine; Task 6 enumerates members |
| denies | denies | the test itself is wrong — fix the fixture and repeat, do not conclude |

The third row is the one to watch: a test that fails for both is measuring nothing, and recording it as "groups don't work" would be the wrong answer written down confidently.

- [ ] **Step 5: Write the result down with its date and commit**

Create `docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md` containing: the date, the exact principal string the engine reported, the two policies tried, both query results verbatim, and the conclusion drawn from the table above. If groups do not work, add one line on whether egret has any group→engine sync at all (`grep -rn "usersync\|group" crates/egret-policy/src/ranger/`), because Task 6 needs to know whether this is a missing feature or a broken one.

```bash
git add docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md
git commit -m "docs: D7 — whether a group grant reaches the engine, measured"
```

---

### Task 2: The `perm` schema and its store

**Files:**
- Create: `migrations/0154_perm_model.sql`
- Create: `crates/egret-perm/Cargo.toml`, `src/lib.rs`, `src/model.rs`, `src/store.rs`
- Create: `crates/egret-perm/tests/store.rs`
- Modify: root `Cargo.toml` workspace members

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub enum Action { Read, Write }
  pub enum Effect { Allow, Deny }
  pub enum PrincipalKind { User, Group, Role, WorkspaceMembers }
  pub struct Principal { pub kind: PrincipalKind, pub r#ref: String }
  pub struct PrincipalSet { pub include: Vec<Principal>, pub exclude: Vec<Principal> }
  pub struct Scope { pub catalog_id: Uuid, pub schema: Option<String>,
                     pub table: Option<String>, pub column: Option<String> }
  pub struct Grant { pub id: Uuid, pub effect: Effect, pub actions: Vec<Action>,
                     pub scope: Scope, pub principals: PrincipalSet, pub origin: GrantOrigin }
  pub enum GrantOrigin { Explicit, WorkspaceAssignment { workspace_id: Uuid } }
  pub enum Selector { Tag { key: String }, Path(Scope) }
  pub enum RuleKind { Mask { mask_type: String, value_expr: Option<String> },
                      RowFilter { filter_expr: String } }
  pub struct Rule { pub id: Uuid, pub name: String, pub kind: RuleKind,
                    pub selector: Selector, pub exempt: PrincipalSet }
  pub struct PermStore { /* pool */ }
  impl PermStore {
      pub async fn create_grant(&self, g: &Grant) -> Result<Uuid, PermError>;
      pub async fn list_grants(&self) -> Result<Vec<Grant>, PermError>;
      pub async fn delete_grant(&self, id: Uuid) -> Result<bool, PermError>;
      pub async fn create_rule(&self, r: &Rule) -> Result<Uuid, PermError>;
      pub async fn list_rules(&self) -> Result<Vec<Rule>, PermError>;
      pub async fn delete_rule(&self, id: Uuid) -> Result<bool, PermError>;
  }
  ```

- [ ] **Step 1: Write the migration**

`migrations/0154_perm_model.sql`:

```sql
-- 0154_perm_model.sql
-- SP4a. egret's first data-permission model. `policy.*` keeps serving raw
-- Ranger JSON for hand-written profiles; `perm.*` is the model that COMPILES
-- to it. One direction only — nothing reads Ranger JSON back into perm.
CREATE SCHEMA perm;

CREATE TYPE perm.effect AS ENUM ('allow', 'deny');
CREATE TYPE perm.principal_kind AS ENUM ('user', 'group', 'role', 'workspace_members');
CREATE TYPE perm.principal_mode AS ENUM ('include', 'exclude');
CREATE TYPE perm.rule_kind AS ENUM ('mask', 'row_filter');
CREATE TYPE perm.selector_kind AS ENUM ('tag', 'path');

CREATE TABLE perm.grants (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    effect      perm.effect NOT NULL,
    actions     text[] NOT NULL CHECK (cardinality(actions) > 0),
    catalog_id  uuid NOT NULL REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    schema_name text,
    table_name  text,
    column_name text,
    -- `workspace_assignment` rows are written by the assignment path, not by a
    -- human, and are recreated from the assignment on demand (D3).
    origin      text NOT NULL DEFAULT 'explicit'
                CHECK (origin IN ('explicit', 'workspace_assignment')),
    origin_workspace_id uuid REFERENCES workspaces(id) ON DELETE CASCADE,
    description text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now(),
    -- A path cannot skip a level: a column implies a table implies a schema.
    CHECK (schema_name IS NOT NULL OR table_name IS NULL),
    CHECK (table_name IS NOT NULL OR column_name IS NULL),
    -- An implicit grant names its workspace; an explicit one does not.
    CHECK ((origin = 'workspace_assignment') = (origin_workspace_id IS NOT NULL))
);
CREATE INDEX grants_catalog_idx ON perm.grants (catalog_id);
-- One implicit grant per (catalog, workspace): the assignment is the identity.
CREATE UNIQUE INDEX grants_one_per_assignment
    ON perm.grants (catalog_id, origin_workspace_id)
    WHERE origin = 'workspace_assignment';

CREATE TABLE perm.grant_principals (
    grant_id uuid NOT NULL REFERENCES perm.grants(id) ON DELETE CASCADE,
    kind     perm.principal_kind NOT NULL,
    ref      text NOT NULL,
    mode     perm.principal_mode NOT NULL,
    PRIMARY KEY (grant_id, kind, ref, mode)
);

CREATE TABLE perm.rules (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name          text NOT NULL UNIQUE,
    kind          perm.rule_kind NOT NULL,
    -- mask: the Ranger mask type + optional expression. row_filter: the expression.
    mask_type     text,
    value_expr    text,
    filter_expr   text,
    selector_kind perm.selector_kind NOT NULL,
    -- D6: tags are referenced by KEY. `tags.name` is mutable and non-unique,
    -- and referencing it is what makes a rename break enforcement silently.
    tag_key       text REFERENCES tags(key) ON UPDATE CASCADE,
    catalog_id    uuid REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    schema_name   text,
    table_name    text,
    column_name   text,
    description   text NOT NULL DEFAULT '',
    created_at    timestamptz NOT NULL DEFAULT now(),
    CHECK ((selector_kind = 'tag') = (tag_key IS NOT NULL)),
    CHECK ((selector_kind = 'path') = (catalog_id IS NOT NULL)),
    CHECK ((kind = 'mask') = (mask_type IS NOT NULL)),
    CHECK ((kind = 'row_filter') = (filter_expr IS NOT NULL))
);

CREATE TABLE perm.rule_exemptions (
    rule_id uuid NOT NULL REFERENCES perm.rules(id) ON DELETE CASCADE,
    kind    perm.principal_kind NOT NULL,
    ref     text NOT NULL,
    PRIMARY KEY (rule_id, kind, ref)
);

-- The reverse index Ranger has no equivalent of: which egret object produced
-- which policy, in which profile. Read to answer "where is this rule applied",
-- and used by a recompile to DIFF rather than drop-and-recreate, so a policy id
-- an engine has cached does not churn.
CREATE TABLE perm.compiled (
    service_id  bigint NOT NULL REFERENCES policy.authz_services(id) ON DELETE CASCADE,
    policy_id   bigint NOT NULL,
    source_kind text NOT NULL CHECK (source_kind IN ('grant', 'rule')),
    grant_id    uuid REFERENCES perm.grants(id) ON DELETE CASCADE,
    rule_id     uuid REFERENCES perm.rules(id) ON DELETE CASCADE,
    compiled_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (service_id, policy_id),
    CHECK ((source_kind = 'grant') = (grant_id IS NOT NULL)),
    CHECK ((source_kind = 'rule') = (rule_id IS NOT NULL))
);
CREATE INDEX compiled_by_grant ON perm.compiled (grant_id);
CREATE INDEX compiled_by_rule  ON perm.compiled (rule_id);
```

Before writing this, confirm the real names of the referenced tables: `tags(key)` must be unique for the FK to be legal (`migrations/0080_tags.sql`), and `workspaces(id)` must exist. If `tags.key` carries no UNIQUE constraint, add one in this migration rather than dropping the FK — the whole point of D6 is that the reference is stable.

- [ ] **Step 2: Write the failing store test**

`crates/egret-perm/tests/store.rs`:

```rust
//! The store round-trips the model. The assertion that matters is the
//! principal SUBTRACT: it is the reason egret can drop Ranger's
//! allow-exception/deny-exception blocks from the authoring surface, so if it
//! does not survive a round trip, nothing above it is trustworthy.

#[tokio::test]
async fn grant_round_trips_with_its_principal_subtract() {
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let catalog_id = seed_catalog(&db.pool).await;

    let g = Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope { catalog_id, schema: Some("sales".into()), table: None, column: None },
        principals: PrincipalSet {
            include: vec![Principal { kind: PrincipalKind::Group, r#ref: "finance".into() }],
            exclude: vec![Principal { kind: PrincipalKind::Group, r#ref: "contractors".into() }],
        },
        origin: GrantOrigin::Explicit,
    };
    let id = store.create_grant(&g).await.expect("create");

    let back = store.list_grants().await.expect("list");
    let got = back.iter().find(|x| x.id == id).expect("present");
    assert_eq!(got.principals.include.len(), 1);
    assert_eq!(got.principals.exclude.len(), 1, "the subtract must survive the round trip");
    assert_eq!(got.principals.exclude[0].r#ref, "contractors");
    assert_eq!(got.scope.schema.as_deref(), Some("sales"));
    assert!(got.scope.table.is_none());
}

#[tokio::test]
async fn a_column_scope_without_a_table_is_refused_by_the_database() {
    // The CHECK, not the Rust layer, is what makes this unrepresentable — a
    // predicate in one code path is a rule somebody later forgets.
    let Some(db) = test_db().await else { return };
    let catalog_id = seed_catalog(&db.pool).await;
    let err = sqlx::query(
        "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name, column_name) \
         VALUES ('allow', ARRAY['read'], $1, 'sales', 'email')",
    )
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .expect_err("a column with no table must be refused");
    assert!(err.to_string().contains("grants_check") || err.to_string().contains("violates check"),
            "expected a CHECK violation, got: {err}");
}
```

- [ ] **Step 3: Run the tests and watch them fail**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export CARGO_TARGET_DIR=$PWD/target DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm 2>&1 | tail -20
```

Expected: FAIL — the crate does not exist.

- [ ] **Step 4: Create the crate and the model types**

Add `crates/egret-perm` to the workspace members in the root `Cargo.toml`. Write `src/model.rs` with exactly the types in the Interfaces block above, deriving `Debug, Clone, Serialize, Deserialize, utoipa::ToSchema` on the public ones.

- [ ] **Step 5: Write the store**

`src/store.rs`. Each `create_*` runs in one transaction: the row, then its principals. Every SQL string is a `&'static str` literal with bind parameters — sqlx 0.9 requires it and `format!`-built SQL is forbidden by the global constraints.

- [ ] **Step 6: Run the tests and the migration**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: the perm schema and its store (SP4a)"
```

---

### Task 3: The evaluator

**Files:**
- Create: `crates/egret-perm/src/eval.rs`
- Create: `crates/egret-perm/tests/eval.rs`

**Interfaces:**
- Consumes: Task 2's `Grant`, `Rule`, `PrincipalSet`, `Scope`.
- Produces:
  ```rust
  pub struct Subject { pub user: String, pub groups: Vec<String>,
                       pub roles: Vec<String>, pub workspaces: Vec<Uuid> }
  pub enum Reason { AllowedByGrant { grant_id: Uuid },
                    ExplicitDeny   { grant_id: Uuid },
                    NoGrantMatched,
                    MaskedByRule   { rule_id: Uuid, mask_type: String },
                    FilteredByRule { rule_id: Uuid, filter_expr: String } }
  pub struct Decision { pub allowed: bool, pub reasons: Vec<Reason> }
  pub fn evaluate(subject: &Subject, action: Action, target: &Scope,
                  grants: &[Grant], rules: &[Rule], tags_on: &[String]) -> Decision;
  ```

- [ ] **Step 1: Write the failing tests — the semantics of D2, one test each**

`crates/egret-perm/tests/eval.rs`:

```rust
//! D2: you can see it if some grant allows it, unless some grant denies it;
//! masks and row filters always apply, however the access was granted.

#[test]
fn deny_beats_allow_regardless_of_order() {
    let allow = grant(Effect::Allow, &["analysts"], &[], scope_table("sales", "orders"));
    let deny  = grant(Effect::Deny,  &["analysts"], &[], scope_table("sales", "orders"));
    let subj  = subject_in_group("analysts");

    // Both orders, because an evaluator that depends on grant order is a
    // Ranger first-match-wins in disguise, and D2 promises it is not.
    for grants in [vec![allow.clone(), deny.clone()], vec![deny.clone(), allow.clone()]] {
        let d = evaluate(&subj, Action::Read, &scope_table("sales", "orders"), &grants, &[], &[]);
        assert!(!d.allowed, "deny must win in either order");
        assert!(matches!(d.reasons[0], Reason::ExplicitDeny { .. }));
    }
}

#[test]
fn the_subtract_removes_a_member_of_an_included_group() {
    let g = grant(Effect::Allow, &["finance"], &["contractors"], scope_table("sales", "orders"));
    let insider  = Subject { user: "a".into(), groups: vec!["finance".into()], roles: vec![], workspaces: vec![] };
    let outsider = Subject { user: "b".into(), groups: vec!["finance".into(), "contractors".into()], roles: vec![], workspaces: vec![] };

    assert!(evaluate(&insider,  Action::Read, &scope_table("sales", "orders"), &[g.clone()], &[], &[]).allowed);
    let d = evaluate(&outsider, Action::Read, &scope_table("sales", "orders"), &[g], &[], &[]);
    assert!(!d.allowed, "exclude must subtract from include");
    assert!(matches!(d.reasons[0], Reason::NoGrantMatched),
            "a subtracted principal matches NO grant — it is not an explicit deny, \
             and telling those apart is the whole point of the reason class");
}

#[test]
fn no_grant_at_all_is_a_distinct_reason_from_an_explicit_deny() {
    let d = evaluate(&subject_in_group("nobody"), Action::Read,
                     &scope_table("sales", "orders"), &[], &[], &[]);
    assert!(!d.allowed);
    assert!(matches!(d.reasons[0], Reason::NoGrantMatched));
}

#[test]
fn a_mask_applies_even_when_access_came_from_a_wider_grant() {
    let g = grant(Effect::Allow, &["analysts"], &[], scope_schema("sales"));
    let r = mask_rule_on_tag("pii", "MASK_HASH", &[]);
    let d = evaluate(&subject_in_group("analysts"), Action::Read,
                     &scope_column("sales", "orders", "email"), &[g], &[r], &["pii".into()]);
    assert!(d.allowed, "the grant still allows the read");
    assert!(d.reasons.iter().any(|x| matches!(x, Reason::MaskedByRule { .. })),
            "the mask must apply regardless of how access was granted (D2)");
}

#[test]
fn an_exempt_principal_escapes_the_mask() {
    let g = grant(Effect::Allow, &["analysts"], &[], scope_schema("sales"));
    let r = mask_rule_on_tag("pii", "MASK_HASH", &["stewards"]);
    let subj = Subject { user: "s".into(),
                         groups: vec!["analysts".into(), "stewards".into()],
                         roles: vec![], workspaces: vec![] };
    let d = evaluate(&subj, Action::Read, &scope_column("sales", "orders", "email"),
                     &[g], &[r], &["pii".into()]);
    assert!(d.allowed);
    assert!(!d.reasons.iter().any(|x| matches!(x, Reason::MaskedByRule { .. })));
}

#[test]
fn a_grant_on_a_schema_covers_a_table_beneath_it_but_not_a_sibling_schema() {
    let g = grant(Effect::Allow, &["analysts"], &[], scope_schema("sales"));
    let subj = subject_in_group("analysts");
    assert!(evaluate(&subj, Action::Read, &scope_table("sales", "orders"), &[g.clone()], &[], &[]).allowed);
    assert!(!evaluate(&subj, Action::Read, &scope_table("hr", "salaries"), &[g], &[], &[]).allowed);
}
```

Write the small helpers (`grant`, `scope_table`, `scope_schema`, `scope_column`, `subject_in_group`, `mask_rule_on_tag`) at the top of the file. Keep them dumb — a helper that computes anything is a second implementation of the thing under test.

- [ ] **Step 2: Run them and watch them fail**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm --test eval 2>&1 | tail -20
```

Expected: FAIL — `evaluate` does not exist.

- [ ] **Step 3: Implement `evaluate`**

```rust
pub fn evaluate(subject: &Subject, action: Action, target: &Scope,
                grants: &[Grant], rules: &[Rule], tags_on: &[String]) -> Decision {
    let applicable = |g: &Grant| {
        g.actions.contains(&action) && covers(&g.scope, target) && matches_set(subject, &g.principals)
    };

    // Deny first, and exhaustively: D2 says deny wins, so a single matching
    // deny ends it whatever else allows.
    if let Some(d) = grants.iter().find(|g| g.effect == Effect::Deny && applicable(g)) {
        return Decision { allowed: false, reasons: vec![Reason::ExplicitDeny { grant_id: d.id }] };
    }

    let allow = grants.iter().find(|g| g.effect == Effect::Allow && applicable(g));
    let Some(a) = allow else {
        return Decision { allowed: false, reasons: vec![Reason::NoGrantMatched] };
    };

    let mut reasons = vec![Reason::AllowedByGrant { grant_id: a.id }];
    for r in rules {
        if !selector_hits(&r.selector, target, tags_on) { continue; }
        if matches_set(subject, &r.exempt) { continue; }
        reasons.push(match &r.kind {
            RuleKind::Mask { mask_type, .. } =>
                Reason::MaskedByRule { rule_id: r.id, mask_type: mask_type.clone() },
            RuleKind::RowFilter { filter_expr } =>
                Reason::FilteredByRule { rule_id: r.id, filter_expr: filter_expr.clone() },
        });
    }
    Decision { allowed: true, reasons }
}

/// `include` minus `exclude` — the operator that lets egret drop Ranger's
/// allow-exception and deny-exception blocks from the authoring surface (D1).
fn matches_set(s: &Subject, set: &PrincipalSet) -> bool {
    set.include.iter().any(|p| matches_principal(s, p))
        && !set.exclude.iter().any(|p| matches_principal(s, p))
}

fn matches_principal(s: &Subject, p: &Principal) -> bool {
    match p.kind {
        PrincipalKind::User => s.user == p.r#ref,
        PrincipalKind::Group => s.groups.iter().any(|g| g == &p.r#ref),
        PrincipalKind::Role => s.roles.iter().any(|r| r == &p.r#ref),
        PrincipalKind::WorkspaceMembers =>
            p.r#ref.parse::<Uuid>().map(|w| s.workspaces.contains(&w)).unwrap_or(false),
    }
}

/// A grant's scope covers a target when every level the grant names matches.
/// A level the grant leaves `None` means "everything below".
fn covers(g: &Scope, t: &Scope) -> bool {
    if g.catalog_id != t.catalog_id { return false; }
    level_covers(&g.schema, &t.schema) && level_covers(&g.table, &t.table)
        && level_covers(&g.column, &t.column)
}

fn level_covers(grant: &Option<String>, target: &Option<String>) -> bool {
    match (grant, target) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(g), Some(t)) => g == "*" || g == t,
    }
}
```

- [ ] **Step 4: Run the tests**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm --test eval 2>&1 | tail -20
```

Expected: PASS, all six.

- [ ] **Step 5: Mutation-check the deny path**

The deny branch is the one whose failure is a data leak, so prove the test can catch its removal. Change `find(|g| g.effect == Effect::Deny && applicable(g))` to `find(|_| false)`, re-run, confirm `deny_beats_allow_regardless_of_order` FAILS, then restore and confirm `git diff` is empty for that file.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: the perm evaluator, with reason classes (SP4a)"
```

---

### Task 4: Compile grants into Ranger access policies

**Files:**
- Create: `crates/egret-perm/src/actions.rs`, `src/compile/mod.rs`, `src/compile/grants.rs`
- Create: `crates/egret-perm/tests/compile_grants.rs`

**Interfaces:**
- Consumes: Task 2's model; `egret_policy::admin::ranger_json::{AccessPolicyInput, AccessItemInput, ResourceLevel}`.
- Produces:
  ```rust
  pub enum EngineType { Trino, StarRocks }
  pub fn engine_accesses(a: Action, e: EngineType) -> Result<Vec<&'static str>, CompileError>;
  pub fn compile_grant(g: &Grant, catalog_name: &str, service: &str, e: EngineType)
      -> Result<AccessPolicyInput, CompileError>;
  pub enum CompileError { UnsupportedAction { action: Action, engine: EngineType },
                          ExpansionCeiling { rule: String, count: usize, ceiling: usize } }
  ```

- [ ] **Step 1: Write the failing tests**

`crates/egret-perm/tests/compile_grants.rs`:

```rust
//! The compiler is the security boundary: its output is what an engine
//! enforces. These tests pin the two things a reader cannot check by eye —
//! that the subtract lands in `allowExceptions`, and that an action an engine
//! cannot express is an ERROR rather than a silent drop.

#[test]
fn a_principal_subtract_compiles_to_allow_exceptions() {
    let g = grant_allow_read(&["finance"], &["contractors"]);
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();

    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].groups, vec!["finance"]);
    assert_eq!(out.allow_exceptions.len(), 1,
               "the exclude set must become an allowException — that block is a \
                compilation target and must never surface in egret's own API");
    assert_eq!(out.allow_exceptions[0].groups, vec!["contractors"]);
    assert!(out.deny_exceptions.is_empty());
}

#[test]
fn a_deny_grants_subtract_compiles_to_deny_exceptions() {
    let mut g = grant_allow_read(&["finance"], &["contractors"]);
    g.effect = Effect::Deny;
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();
    assert_eq!(out.items[0].effect, "deny");
    assert_eq!(out.deny_exceptions.len(), 1);
    assert!(out.allow_exceptions.is_empty());
}

#[test]
fn write_maps_to_two_accesses_on_trino_and_three_on_starrocks() {
    assert_eq!(engine_accesses(Action::Read, EngineType::Trino).unwrap(), vec!["select"]);
    assert_eq!(engine_accesses(Action::Read, EngineType::StarRocks).unwrap(), vec!["select"]);

    // Trino's service-def declares NO `update` access type. Asserting the same
    // three on both engines would be asserting a mapping that cannot exist —
    // this test exists because the plan's first draft did exactly that.
    let mut t = engine_accesses(Action::Write, EngineType::Trino).unwrap();
    t.sort();
    assert_eq!(t, vec!["delete", "insert"]);

    let mut s = engine_accesses(Action::Write, EngineType::StarRocks).unwrap();
    s.sort();
    assert_eq!(s, vec!["delete", "insert", "update"]);
}

#[test]
fn every_access_type_the_map_emits_is_declared_by_that_service_def() {
    // The map is only correct relative to the service-defs, so read them
    // rather than trusting the table above. An access type Ranger does not
    // know is silently ignored by the plugin: the policy compiles, deploys,
    // and grants nothing.
    for (engine, path) in [(EngineType::Trino, "crates/egret-policy/src/service-defs/trino.json"),
                           (EngineType::StarRocks, "crates/egret-policy/src/service-defs/starrocks.json")] {
        let def: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let declared: Vec<String> = def["accessTypes"].as_array().unwrap().iter()
            .map(|a| a["name"].as_str().unwrap().to_string()).collect();
        for action in [Action::Read, Action::Write] {
            for acc in engine_accesses(action, engine).unwrap() {
                assert!(declared.iter().any(|d| d == acc),
                        "{engine:?} does not declare access type {acc:?}");
            }
        }
    }
}

#[test]
fn the_scope_becomes_the_ranger_resource_levels_the_service_def_declares() {
    let g = grant_allow_read_on(Scope { catalog_id: Uuid::nil(), schema: Some("sales".into()),
                                        table: Some("orders".into()), column: None });
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();
    assert_eq!(out.resources["catalog"].values, vec!["warehouse"]);
    assert_eq!(out.resources["schema"].values, vec!["sales"]);
    assert_eq!(out.resources["table"].values, vec!["orders"]);
    assert!(!out.resources.contains_key("column"),
            "an unnamed level must be ABSENT, not a wildcard string — Ranger \
             treats a missing level and a '*' level differently");
}
```

- [ ] **Step 2: Run and watch fail**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm --test compile_grants 2>&1 | tail -20
```

Expected: FAIL.

- [ ] **Step 3: Write the action map from the service-defs, not from memory**

`src/actions.rs`. Read the two service-defs first and map only to access types they declare:

```bash
python3 -c "
import json
for n in ('trino','starrocks'):
    d=json.load(open(f'crates/egret-policy/src/service-defs/{n}.json'))
    print(n, [a['name'] for a in d['accessTypes']])
"
```

Write the mapping against that output. The two engines genuinely differ and the map must too — Trino
declares no `update`. `CompileError::UnsupportedAction` stays in the error type even though neither
of SP4a's two actions triggers it today: SP4b or SP4c may add an action, and a compiler that silently
drops one grants less than the model says with nothing anywhere to show it.

- [ ] **Step 4: Write `compile_grant`**

It builds an `AccessPolicyInput`: `resources` from the scope (catalog name resolved by the caller, so the compiler stays free of registry I/O), one `AccessItemInput` from `principals.include` with the engine access types, and `principals.exclude` into `allow_exceptions` for an allow grant or `deny_exceptions` for a deny.

- [ ] **Step 5: Emit principals in ONE canonical form**

D7's other half. Principals are written today sometimes as email and sometimes as username
(`programme spec:44`), which means two policies naming the same human can disagree about whether
they match. Task 1 recorded the exact string the engine reports; the compiler emits **that** form and
nothing else.

Write the test against Task 1's recorded value rather than against a guess:

```rust
#[test]
fn a_user_principal_is_emitted_in_the_form_the_engine_reports() {
    // The literal below is copied from the D7 measurement, not assumed. If the
    // engine reports bare usernames, a policy naming an email matches nobody —
    // and nothing in Ranger or egret would show it.
    let g = grant_allow_read_for_user("admin@egret.local");
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();
    assert_eq!(out.items[0].users, vec![CANONICAL_ADMIN_PRINCIPAL]);
}
```

Define `CANONICAL_ADMIN_PRINCIPAL` next to the canonicaliser, with a comment naming the D7 result
file and its date. If the measurement said the engine reports something other than what egret stores,
the canonicaliser converts, and this test is what pins the direction.

- [ ] **Step 6: Run the tests**

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: compile grants into Ranger access policies (SP4a)"
```

---

### Task 5: Compile rules into masks and row filters, with the StarRocks expansion

**Files:**
- Create: `crates/egret-perm/src/compile/rules.rs`
- Create: `crates/egret-perm/tests/compile_rules.rs`

**Interfaces:**
- Consumes: Task 4's `EngineType`, `CompileError`; `egret_policy::admin::ranger_json::{MaskingPolicyInput, RowFilterPolicyInput, tag_masking_to_ranger_json}`.
- Produces:
  ```rust
  pub struct TaggedColumn { pub catalog: String, pub schema: String,
                            pub table: String, pub column: String }
  pub fn compile_rule(r: &Rule, e: EngineType, service: &str, tag_service: Option<&str>,
                      resolved: &ResolvedNames, tagged: &[TaggedColumn], ceiling: usize)
      -> Result<Vec<serde_json::Value>, CompileError>;

  /// CORRECTED 2026-09-24, after the Task 5 review. The first signature took
  /// neither the tag's name nor the catalog's, so the compiler stamped
  /// `tags.key` and a catalog UUID into the policy — and spec D6 requires it to
  /// "resolve key -> name and id -> name at compile time". Both halves of that
  /// sentence are load-bearing: egret's own tag download pushes `tags.name` as
  /// the Ranger tag type (`egret-policy/src/store.rs:245`), and engines send
  /// the catalog NAME. A policy carrying the key or the id deploys cleanly,
  /// appears in Ranger, and matches nothing — so the mask returns the real
  /// value. The compiler still does no I/O; the caller resolves.
  pub struct ResolvedNames<'a> { pub catalog_name: &'a str, pub tag_name: Option<&'a str> }
  ```

- [ ] **Step 1: Write the failing tests**

`crates/egret-perm/tests/compile_rules.rs`:

```rust
//! D5: the same rule compiles compactly on Trino and expansively on StarRocks,
//! because StarRocks' Ranger plugin has no tag service (upstream
//! starrocks#67458). The asymmetry is the design; the ceiling is the guard.

#[test]
fn a_tag_rule_is_one_policy_on_trino_however_many_columns_carry_the_tag() {
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let tagged = (0..50).map(|i| col("wh", "sales", "orders", &format!("c{i}"))).collect::<Vec<_>>();
    let out = compile_rule(&r, EngineType::Trino, "trino-1", Some("trino-1_tag"), &tagged, 100).unwrap();
    assert_eq!(out.len(), 1, "Trino's plugin resolves the tag itself — one policy covers all 50");
}

#[test]
fn the_same_rule_becomes_one_policy_per_column_on_starrocks() {
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let tagged = (0..50).map(|i| col("wh", "sales", "orders", &format!("c{i}"))).collect::<Vec<_>>();
    let out = compile_rule(&r, EngineType::StarRocks, "sr-1", None, &tagged, 100).unwrap();
    assert_eq!(out.len(), 50, "no tag service — the tag must be enumerated at compile time");
}

#[test]
fn the_ceiling_refuses_and_names_the_rule_and_the_count() {
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let tagged = (0..101).map(|i| col("wh", "sales", "orders", &format!("c{i}"))).collect::<Vec<_>>();
    let err = compile_rule(&r, EngineType::StarRocks, "sr-1", None, &tagged, 100).unwrap_err();
    match err {
        CompileError::ExpansionCeiling { rule, count, ceiling } => {
            assert_eq!(rule, "pii-hash");
            assert_eq!(count, 101);
            assert_eq!(ceiling, 100);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn the_ceiling_refuses_rather_than_truncating() {
    // The failure this guards is silent partial enforcement: 100 of 101
    // columns masked looks exactly like success from every screen egret has.
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let tagged = (0..101).map(|i| col("wh", "sales", "orders", &format!("c{i}"))).collect::<Vec<_>>();
    assert!(compile_rule(&r, EngineType::StarRocks, "sr-1", None, &tagged, 100).is_err());
    // And one under the ceiling still compiles, so the test above is not
    // passing merely because compilation is broken.
    let ok = compile_rule(&r, EngineType::StarRocks, "sr-1", None, &tagged[..100], 100).unwrap();
    assert_eq!(ok.len(), 100);
}

#[test]
fn a_custom_mask_expression_that_does_not_parse_is_refused_at_compile_time() {
    // Ranger never parses this string; it fails inside the engine at query
    // time. Refusing here is the difference between an author finding out now
    // and a user finding out from a broken dashboard.
    let r = mask_rule_custom("pii", "initcap(reverse({col})");  // unbalanced paren
    let err = compile_rule(&r, EngineType::Trino, "trino-1", Some("trino-1_tag"), &[], 100);
    assert!(err.is_err(), "an unparseable custom mask must not compile");
}
```

- [ ] **Step 2: Run and watch fail**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm --test compile_rules 2>&1 | tail -20
```

- [ ] **Step 3: Implement `compile_rule`**

Branch on `(selector, engine)`:
- tag + Trino → one policy via `tag_masking_to_ranger_json` (or `tag_rowfilter_to_ranger_json`).
- tag + StarRocks → filter `tagged` to the rule's tag, check `len() <= ceiling` and return `ExpansionCeiling` if not, then one `MaskingPolicyInput` per column.
- path + either → one policy at the named path.

For the custom-expression check, balance-check parentheses and require the `{col}` token to appear; a full SQL parser is SP5's, and a parser this task cannot finish is worse than a check it can.

- [ ] **Step 4: Run the tests**

Expected: PASS, all five.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: compile rules into masks and row filters (SP4a)"
```

---

### Task 6: The implicit workspace grant

**Files:**
- Create: `crates/egret-perm/src/assignment.rs`
- Modify: the catalog-workspace assignment handler in `crates/egret-engine-mgmt/src/catalogs.rs`
- Create: `crates/egret-perm/tests/assignment.rs`

**Interfaces:**
- Consumes: Task 1's recorded answer; Task 2's store.
- Produces: `pub async fn sync_assignment_grants(store: &PermStore, catalog_id: Uuid, workspace_ids: &[Uuid]) -> Result<(), PermError>;`

- [ ] **Step 1: Read Task 1's result and pick the principal shape**

Open `docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`. If it concluded that groups reach the engine, `workspace-members` compiles to a group principal and this task is small. If it concluded they do not, the implicit grant stores `PrincipalKind::WorkspaceMembers` and the **compiler** expands it to the member list, which means Task 7 must recompile on membership writes. Write which branch you took at the top of `assignment.rs`, with the date of the measurement.

- [ ] **Step 2: Write the failing test**

```rust
#[tokio::test]
async fn assigning_a_catalog_creates_exactly_one_read_grant_for_that_workspace() {
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let (catalog_id, ws) = seed_catalog_and_workspace(&db.pool).await;

    sync_assignment_grants(&store, catalog_id, &[ws]).await.unwrap();
    let g: Vec<_> = store.list_grants().await.unwrap().into_iter()
        .filter(|g| matches!(g.origin, GrantOrigin::WorkspaceAssignment { .. })).collect();
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].actions, vec![Action::Read], "an assignment grants READ, never write");

    // Idempotent: re-running must not accumulate. The unique index is the
    // enforcement; this asserts the code path agrees with it.
    sync_assignment_grants(&store, catalog_id, &[ws]).await.unwrap();
    assert_eq!(store.list_grants().await.unwrap().len(), 1);
}

#[tokio::test]
async fn unassigning_removes_the_implicit_grant_but_never_an_explicit_one() {
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let (catalog_id, ws) = seed_catalog_and_workspace(&db.pool).await;
    sync_assignment_grants(&store, catalog_id, &[ws]).await.unwrap();
    let explicit = store.create_grant(&explicit_write_grant(catalog_id)).await.unwrap();

    sync_assignment_grants(&store, catalog_id, &[]).await.unwrap();
    let left = store.list_grants().await.unwrap();
    assert_eq!(left.len(), 1, "the explicit grant must survive");
    assert_eq!(left[0].id, explicit);
}
```

- [ ] **Step 3: Run, watch fail, implement**

`sync_assignment_grants` deletes implicit grants for this catalog whose workspace is not in the list, and upserts one per workspace in it. It touches only rows with `origin = 'workspace_assignment'` — the second test is what proves that.

- [ ] **Step 4: Call it from the assignment handler**

In `crates/egret-engine-mgmt/src/catalogs.rs`, the handler that replaces a catalog's workspace set calls `sync_assignment_grants` **inside the same transaction** as the assignment write. An assignment that succeeds while its grant fails is exactly the split-brain D3 exists to remove.

- [ ] **Step 5: Run the tests and commit**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm -p egret-engine-mgmt 2>&1 | grep -E "^test result" | tail -3
git add -A
git commit -m "feat: assigning a catalog to a workspace grants it read (SP4a)"
```

---

### Task 7: Recompile triggers

**Files:**
- Create: `crates/egret-perm/src/compile/trigger.rs`
- Modify: `crates/egret-bi/src/catalog_tags.rs` (tag writes), `crates/egret-perm/src/assignment.rs` (membership, if Task 1 said so)
- Create: `crates/egret-perm/tests/recompile.rs`

**Interfaces:**
- Consumes: Tasks 4, 5, 6.
- Produces: `pub async fn recompile_service(store: &PermStore, policy: &PolicyStore, service_id: i64) -> Result<CompileSummary, CompileError>;`

- [ ] **Step 1: Write the failing test — the staleness D5 names**

```rust
#[tokio::test]
async fn tagging_a_new_column_recompiles_the_starrocks_profile() {
    // On Trino the plugin resolves tags itself, so nothing needs to happen.
    // On StarRocks the tag was enumerated at compile time, so a new tagged
    // column is UNMASKED until egret recompiles — which is the whole of D5.
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;

    let before = count_policies(&db, f.service_id).await;
    tag_column(&db, f.warehouse_id, "sales", "orders", "email_2", "pii").await;
    let after = count_policies(&db, f.service_id).await;

    assert_eq!(after, before + 1,
               "a newly tagged column must produce a new masking policy on StarRocks");
}
```

- [ ] **Step 2: Run, watch fail, implement the trigger**

`recompile_service` loads the model, compiles it for that service's engine type, diffs against `perm.compiled`, and writes only the difference: create the new, update the changed, delete the gone. It bumps `authz_services.policy_version` once at the end, in the same transaction — the plugin polls that version, and a bump with no write, or a write with no bump, are both invisible failures this programme has already shipped once.

Call it from `catalog_tags.rs`'s three write paths, in the same transaction that already calls `bump_bound_tag_versions`.

- [ ] **Step 3: Assert the diff does not churn ids**

```rust
#[tokio::test]
async fn recompiling_an_unchanged_model_writes_nothing_and_keeps_policy_ids() {
    let Some(db) = test_db().await else { return };
    let f = seed_starrocks_engine_with_tag_rule(&db).await;
    let ids_before = policy_ids(&db, f.service_id).await;
    let v_before = policy_version(&db, f.service_id).await;

    recompile_service(&f.perm, &f.policy, f.service_id).await.unwrap();

    assert_eq!(policy_ids(&db, f.service_id).await, ids_before,
               "a no-op recompile must not churn policy ids — an engine caches them");
    assert_eq!(policy_version(&db, f.service_id).await, v_before,
               "and must not bump the version, or every plugin re-downloads for nothing");
}
```

- [ ] **Step 4: Run and commit**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm --test recompile 2>&1 | tail -10
git add -A
git commit -m "feat: recompile on tag and assignment writes (SP4a)"
```

---

### Task 8: Generated profiles are read-only

**Files:**
- Create: `migrations/0155_profile_generated.sql`
- Modify: `crates/egret-policy/src/store.rs`, `crates/egret-policy/src/admin/mod.rs`
- Create: `crates/egret-policy/tests/generated_profile_guard.rs`

**Interfaces:**
- Consumes: Task 7's compile.
- Produces: `policy.authz_services.origin` (`generated | manual`); the admin write paths refuse `generated`.

- [ ] **Step 1: The migration**

```sql
-- 0155_profile_generated.sql
-- SP4a D4. A profile is compiled from the perm model (`generated`) or
-- hand-authored (`manual`). Existing profiles are manual: they were written by
-- hand, and calling them generated would make the next compile delete policies
-- nobody asked it to.
ALTER TABLE policy.authz_services
    ADD COLUMN origin text NOT NULL DEFAULT 'manual'
    CHECK (origin IN ('generated', 'manual'));
```

- [ ] **Step 2: Write the failing test**

```rust
#[tokio::test]
async fn the_editor_refuses_to_write_a_policy_into_a_generated_profile() {
    let Some(f) = setup().await else { return };
    let svc = seed_generated_profile(&f).await;
    let (status, body) = post_policy(&f, svc, sample_policy()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.to_string().contains("compiled"),
            "the refusal must say WHY — a 409 that does not name the compiler \
             reads as a bug, and the operator's next move is to retry");
}
```

- [ ] **Step 3: Implement, run, commit**

`create_policy`, `update_policy`, `delete_policy` and `set_enabled` all check the profile's origin first and return a 409 naming the compiler. `seed-defaults` too.

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy 2>&1 | grep -E "^test result" | tail -2
git add -A
git commit -m "feat: generated profiles are read-only in the editor (SP4a)"
```

---

### Task 9: The HTTP surface

**Files:**
- Create: `crates/egret-perm/src/routes.rs`
- Modify: `crates/egret-server/src/lib.rs`
- Create: `crates/egret-perm/tests/routes.rs`

**Interfaces:**
- Consumes: Tasks 2, 3, 7.
- Produces:
  - `GET/POST /api/control-plane/perm/grants`, `DELETE /api/control-plane/perm/grants/{id}`
  - `GET/POST /api/control-plane/perm/rules`, `DELETE /api/control-plane/perm/rules/{id}`
  - `POST /api/control-plane/perm/explain`
  - `POST /api/control-plane/perm/compile/{service_id}`

- [ ] **Step 1: Write the failing test for `explain`**

```rust
#[tokio::test]
async fn explain_names_the_reason_class_not_just_a_verdict() {
    let Some(f) = setup().await else { return };
    let (status, body) = explain(&f, "nobody@egret.local", "read", &target_table()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["allowed"], false);
    assert_eq!(body["reasons"][0]["kind"], "no_grant_matched",
               "Ranger's audit shows '-' for exactly this case; naming it is the point");
}
```

- [ ] **Step 2: Implement the routes**

All gated `policy:read` for the GETs and `policy:update` for the writes — reuse the existing permission family rather than adding a key, because SP4c may fold the policy editor away entirely and a key added now would need retiring twice.

- [ ] **Step 3: Regenerate OpenAPI and the client**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
CARGO_TARGET_DIR=$PWD/target cargo build -p egret-bin --bin egret
"$PWD/target/debug/egret" openapi > openapi.json
cd frontend && npm run gen:api && npx tsc --noEmit
```

- [ ] **Step 4: Run and commit**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm -p egret-server 2>&1 | grep -E "^test result" | tail -3
git add -A
git commit -m "feat: the perm HTTP surface, including explain (SP4a)"
```

---

### Task 10: The live gate — both engines, and the evaluator against reality

**Files:**
- Create: `docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md`

**This is the task that decides whether SP4a is real.** A compiler with green unit tests and a mask that does not mask is the exact failure the spec says a JSON snapshot test would miss.

- [ ] **Step 1: Deploy the branch to kind**

Build images, `kind load`, then helm upgrade preserving **all five** live values, then roll the pods — a helm upgrade alone does not replace running pods when the image tag is unchanged, and the init-job migrates with the new image while the old one keeps serving:

```bash
command helm --kube-context kind-egret upgrade egret deploy/helm/egret -n egret-system --wait --timeout 10m \
  --set oidc.enabled=true --set ingress.enabled=true --set kernel.launcher=kubernetes \
  --set backend.replicas=2 --set sharedStore.enabled=true
command kubectl --context kind-egret rollout restart deploy/egret deploy/egret-frontend -n egret-system
command kubectl --context kind-egret rollout status deploy/egret -n egret-system --timeout=300s
```

Confirm the serving pods carry the new image by checking `imageID`, not by trusting `helm status`.

- [ ] **Step 2: Four cases, on Trino, each verified by a real query**

For each: create the model object through the API, compile, then run a query through egret's SQL Lab as a real user and record what came back.

| Case | Expect |
|---|---|
| grant read on a schema | the query succeeds |
| deny on one table inside it | that table is refused, siblings still work |
| tag mask on a column | the column comes back masked, others unchanged |
| row filter on a table | fewer rows, and the excluded rows are genuinely absent |

Record the actual values returned, not "worked". A mask that returns the real value is the failure being hunted.

- [ ] **Step 3: Repeat on StarRocks**

Same four cases. The tag mask is the one that differs: confirm the expansion produced one policy per column, and that adding a tag to a fifth column recompiles (Task 7) and masks it without a manual step.

- [ ] **Step 4: The evaluator must agree with the engine**

For each of the eight cases above, call `POST /perm/explain` with the same subject, action and target, and record whether `allowed` matches what the engine actually did.

A disagreement is a defect in the compiler or the evaluator, and finding out which needs both — that is D8's second reason. Do not resolve a disagreement by changing the test; report it.

- [ ] **Step 5: Measure the StarRocks ceiling**

D5 says the ceiling's value comes from a measurement, not a guess. Tag N columns, compile, and record the wall-clock compile time and the resulting policy count for N in {10, 50, 100, 250}. Pick the ceiling from where compile time or the Ranger download starts to hurt, and write the number and the measurement into the spec.

- [ ] **Step 6: Write the result down and commit**

`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md`: every case, the query, the user, and the verbatim result. Anything that failed is recorded as failed, with the output.

```bash
git add -A
git commit -m "docs: SP4a live gate — both engines, evaluator vs reality"
```

---

## Deliberately not in this plan

- The UI (SP4b). Task 9's endpoints are the contract it will consume.
- Migrating existing hand-written policies, folding in Cedar classification clearance, and the per-workspace notebook role (SP4c).
- SQL rewriting and enforcement on engines with no Ranger plugin (SP5). Task 3's evaluator is that work's foundation; nothing here rewrites a query.
- A full SQL parser for custom mask expressions. Task 5 does a balance-and-token check and says so; the real parser arrives with SP5, which needs one anyway.
