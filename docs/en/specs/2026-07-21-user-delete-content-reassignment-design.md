# User-Delete Content Reassignment — Design

> **Status:** Design approved (brainstorm). Next: implementation plan.
> **Builds on:** #305 (`UserRepo::hard_delete` + `DELETE /users/{id}` + the
> `23503`→409 safety net already exist).

**Goal:** Make hard-delete succeed for a user who owns content. Today
`DELETE /users/{id}` returns 409 whenever the target owns a dashboard, notebook,
dataset, etc. (a `REFERENCES users(id)` FK blocks the row delete). Handle each FK
so the delete succeeds while preserving the content.

## 1. The FK landscape (25 columns referencing `users(id)`, three groups)

**Group A — already `ON DELETE CASCADE` (zero work):** `sessions.user_id`,
`oidc_identities.user_id`, `bookmarks.user_id`, `kernels.user_id`,
`notebook_kernels.user_id`, `access_requests.requester_id`, `user_groups.user_id`.
(`oidc_refresh_tokens` was dropped in #305.) These auto-clean on user delete.

**Group B — nullable attribution columns → `ON DELETE SET NULL` (~14):**
`dashboards.created_by`, `charts.created_by`, `datasets.created_by`,
`folders.created_by`, `sqllab.created_by`, `databases.created_by`,
`databases.granted_by`, `catalog.granted_by`, `tags.created_by`,
`tags.assigned_by`, `catalog_tag_targets.assigned_by`,
`workspace_rbac.created_by`, `workspace_rbac.added_by`,
`classification_policies.created_by`, `engine_workspaces.assigned_by`,
`engine.user_id`, `access_requests.decided_by`. These record *who did it*;
nulling loses attribution but keeps the content. (`policy_audit.actor` is NOT in
scope — that table was dropped in migration 0063.)

**Group C — the one true-ownership column:** `notebooks.owner_user_id` is
`NOT NULL` — a notebook must have an owner, so it cannot be SET NULL.

## 2. Decisions (settled)

| Decision | Choice |
| --- | --- |
| Group B mechanism | **Declarative `ON DELETE SET NULL` migration** — the DB nulls attribution columns automatically on user delete (single source of truth); `hard_delete` stays tiny. |
| Group C (notebooks) | **Reassign to the deleting admin** — the target's notebooks get `owner_user_id = <caller admin>`; content survives, delete succeeds. |

## 3. Components

### 3.1 Migration `0093_user_delete_set_null.sql`
For each Group B column, drop and re-add the FK constraint with `ON DELETE SET NULL`:
```sql
ALTER TABLE <schema.table>
  DROP CONSTRAINT <table>_<col>_fkey,
  ADD CONSTRAINT <table>_<col>_fkey
    FOREIGN KEY (<col>) REFERENCES users(id) ON DELETE SET NULL;
```
The implementation enumerates every column with its **real** constraint name
(Postgres default `<table>_<col>_fkey`) and **real schema qualifier** (several
tables moved into schemas in migration 0067 — e.g. `analytics.dashboards`), each
verified against the live schema before writing the `ALTER`. `touch
crates/egret-metastore/src/lib.rs` so the embedded migrator re-embeds.

### 3.2 `UserRepo::hard_delete` — reassign notebooks
New signature `hard_delete(target: UserId, reassign_notebooks_to: UserId)`. In the
existing transaction, before deleting the user, reassign the target's notebooks:
```rust
sqlx::query("UPDATE notebooks SET owner_user_id = $2 WHERE owner_user_id = $1")
    .bind(target.0).bind(reassign_to.0).execute(&mut *tx).await?;
```
The rest is unchanged: the explicit `access_grants` delete stays (polymorphic
`principal_id`, no FK, no cascade); `DELETE FROM users` now triggers Group A
cascades + Group B set-nulls automatically. The explicit `sessions`/`user_groups`
deletes are kept as harmless belt-and-suspenders (they're `ON DELETE CASCADE`).

### 3.3 `delete_user` handler
Passes the caller as the reassignee: `st.users.hard_delete(user_id, p.user_id)`.
The existing self-guard guarantees caller ≠ target, so an admin never reassigns to
the account being deleted.

## 4. The `23503`→409 safety net

The existing FK-violation catch stays, but is now a rare **guard**, not the normal
path: it only fires if a *future* table adds a NOT-NULL FK to `users(id)` without
a cascade/set-null/reassign rule. Reword its message to:
> "cannot delete: this user owns content in a table that isn't auto-handled on
> delete — a new FK to users(id) needs an ON DELETE rule."

## 5. Error handling

All statements run in the one transaction; any failure (e.g. the reassignee admin
was concurrently deleted) rolls the whole delete back — no partial state. The
migration's per-table drop-and-re-add is atomic; a wrong constraint name fails the
migration loudly, so names are verified first.

## 6. Testing

- **DB integration (`hard_delete`):**
  - Delete a user who created a **dashboard** → 204; the dashboard survives with
    `created_by IS NULL`.
  - Delete a user who owns a **notebook** → 204; the notebook survives with
    `owner_user_id = <reassignee admin>`.
  - Existing cleanup assertions (sessions/user_groups/access_grants gone, user
    gone) still hold.
  - **Flip the existing `deleting_a_content_owner_conflicts_not_500` test** — a
    dashboard-owner delete now returns 204, not 409. Repurpose it to
    `deleting_a_content_owner_reassigns_and_succeeds` (dashboard nulled + notebook
    reassigned + user removed). This is the key behavior change to lock in.
- **Live e2e (optional, extend `local-user-mgmt-kind.spec.ts`):** create a local
  user → they create a notebook via the API → admin deletes them → 204 + the
  notebook still exists. Optional because the DB tests are the authoritative
  reassignment coverage; the e2e already proves the delete round-trip.

## 7. Task decomposition (for the plan)

1. Migration `0093` — `ON DELETE SET NULL` on the ~14 Group B FKs (real constraint
   names + schema qualifiers verified).
2. `hard_delete` notebook-reassign (signature + `UPDATE` + handler passes caller) +
   flip the tests + reword the 409 safety-net message.
3. (Optional) Live e2e: content-owner delete → 204 + notebook survives.

## 8. Out of scope

- A per-user "transfer all my content to X" self-service flow (this is admin-driven
  delete cleanup only).
- Changing what `disabled` does (unchanged).
- The other STS follow-up (IdP-driven revocation / back-channel logout).
