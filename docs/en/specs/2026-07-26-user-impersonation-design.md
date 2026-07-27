# User impersonation — Design

## 1. Goal

Let an admin **act as another user** to reproduce a problem that user reported —
seeing exactly what they see, hitting the same authorization decisions, and
running the same data queries under the same Ranger masking/row-filters.

- **platform_admin** may impersonate any non-admin user.
- **ws_admin** may impersonate a regular member of a workspace they administer,
  and only within that workspace.

Purpose: support/debugging ("I can't see this table", "this page 403s for me"),
not doing the user's work for them.

## 2. The core idea

Every engine token, catalog token, and authorization decision in egret is built
from the request's `Principal` (`egret-core/src/context.rs`) — the machine-IdP
mints per-user engine JWTs from the principal's identity, Cedar takes the
principal as its subject, and the workspace/RBAC layer reads its roles. So
impersonation needs exactly one thing:

> **Make the session resolve to the target user's `Principal`, carrying an
> overlay that remembers the real admin.**

Data-plane access, Cedar, RBAC, and the UI then follow the target identity
**for free** — no change to `egret-machine-idp`, `egret-query`, or the engine
token minters. The impersonator is carried alongside for audit, the banner,
and the ws_admin containment check.

## 3. Mechanism: an overlay, not a session swap

Two shapes were considered:

- **Overlay (chosen).** A row keyed to the admin's existing session marks it as
  "currently impersonating target T on behalf of admin A". The `SessionService`
  applies it when validating. The admin's cookie never changes; the admin's real
  session is never destroyed.
- Session swap (rejected). Mint a distinct impersonation session token, swap the
  cookie, restore the admin token on exit. More moving parts (cookie swap, token
  recovery — sessions store only a token *hash*, so the raw admin token can't be
  recovered to restore it), no benefit.

The overlay is simpler and safer: the admin's identity and session are never
swapped or torn down; impersonation is an explicit, revocable, time-boxed layer.

### 3.1 Data model — migration `0095_impersonations.sql`

```sql
-- An active impersonation overlay on an admin's session. Absence = the session
-- is the admin acting as themselves. At most one active row per admin session.
CREATE TABLE impersonations (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- The admin session this overlay attaches to. Deleting the session (logout,
    -- expiry cascade) ends the impersonation.
    admin_session_id UUID      NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    impersonator_id  UUID      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_user_id   UUID      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- NULL = unrestricted (platform_admin). Non-NULL = ws_admin, confined to
    -- this one workspace.
    workspace_scope_id UUID    REFERENCES workspaces(id) ON DELETE CASCADE,
    reason         TEXT        NOT NULL,
    started_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at     TIMESTAMPTZ NOT NULL
);

-- One active impersonation per admin session.
CREATE UNIQUE INDEX impersonations_one_per_session ON impersonations (admin_session_id);
```

Note `sessions` has an `id` column (from the initial migration) which the
overlay references; `SessionService` currently resolves rows by `token_hash`, so
it will also read the row's `id` to look up the overlay.

## 4. Identity: `Principal` gains an impersonator

`egret-core/src/context.rs` — add an optional overlay to `Principal`:

```rust
#[derive(Debug, Clone)]
pub struct Impersonator {
    pub user_id: UserId,
    pub email: String,
    /// NULL for platform_admin (unrestricted); Some(ws) confines a ws_admin.
    pub workspace_scope: Option<WorkspaceId>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub struct Principal {
    // … existing fields (user_id, email, platform_roles, permissions, group_ids) …
    /// Present iff this request is an impersonation. `user_id`/`email`/roles above
    /// are the TARGET's; this is the real admin behind them.
    pub impersonator: Option<Impersonator>,
}
```

The PDP subject (`AuthTarget::subject`) ignores `impersonator` — authorization
decisions are made as the target, which is the whole point. `impersonator` is
read only by: the audit layer, the `/me` endpoint (banner), and the ws_admin
containment check (§7).

## 5. Session validation applies the overlay

`SessionService::validate(token)`:

1. Resolve the session row (as today) → this is the **admin** identity, with the
   session's `id`.
2. Look up an impersonation row by `admin_session_id = <session id>` that is not
   past `expires_at`.
3. **No active overlay** → return the admin `Principal` with `impersonator: None`
   (unchanged behaviour).
4. **Active overlay** → build the **target's** `Principal` (target's roles,
   permissions, groups — the existing helpers, keyed on `target_user_id`) and set
   `impersonator = Some(Impersonator { impersonator_id, …, workspace_scope,
   expires_at })`.
5. **Expired overlay** → ignore it (return the admin principal) and best-effort
   delete the row. This is the auto-return: once the 30-minute window closes, the
   next request is the admin again. The admin's session was never touched, so no
   re-login is needed.

Nested impersonation is impossible by construction: `validate` only consults the
overlay for the admin session; an impersonated request has no session of its own
to attach a second overlay to, and the start endpoint (§6) refuses when the
caller is already impersonating.

## 6. Starting an impersonation

Two endpoints, differing only in scope resolution:

- `POST /api/admin/impersonate` — body `{target_user_id, reason, password?}`.
  Requires the caller to be `platform_admin`. `workspace_scope_id = NULL`.
- `POST /api/ws/{workspace}/impersonate` — body `{target_user_id, reason,
  password?}`. Requires the caller to hold `ws_admin` on `{workspace}`.
  `workspace_scope_id = <that workspace>`.

Both perform, in order:

1. **Reject if the caller is already impersonating** (`principal.impersonator`
   is set) — no nesting.
2. **Step-up re-authentication** (§8). Fail → `401`, nothing written.
3. **Target validation:**
   - target ≠ caller (no self-impersonation);
   - target is not a `platform_admin` (checked against the target's platform
     roles) → `403`;
   - for the ws_admin endpoint: target is a member of `{workspace}` and is **not**
     a `ws_admin` of it → `403`.
4. Insert the impersonation row (`expires_at = now() + 30 min`), attached to the
   caller's current admin session.
5. Emit an audit event `impersonation.start` (actor = impersonator, detail =
   target + workspace_scope + reason).

Response `200` with the new impersonation state; the client refetches `/me` and
shows the banner.

## 7. ws_admin workspace containment

When `workspace_scope` is `Some(ws)`, the impersonated principal is confined to
that one workspace even though the target may belong to others:

- **`GET /api/me/workspaces`** returns only `ws` (not the target's full
  membership).
- **`WsContext`** (`egret-workspace/src/context.rs`) denies any workspace other
  than `ws` with `403`, before membership resolution — so Alice-as-Bob cannot
  reach W2 even by typing its URL, and cannot mint an engine token scoped to W2
  (engine tokens are only minted inside a resolved `WsContext`).

`platform_admin` impersonation has `workspace_scope = None` → the target's full
membership is visible, unrestricted.

The containment check lives next to the feature-entitlement gate added in the
per-workspace-features work — both are "may this request touch this workspace at
all", resolved once in `WsContext`.

## 8. Step-up re-authentication

Starting an impersonation always requires a fresh proof of identity plus a
reason:

- **Local-auth admin** — re-enter password; verified with the existing
  `verify_password` against the admin's `password_hash`.
- **OIDC admin** — re-authenticate through the IdP with `prompt=login` (reusing
  the existing OIDC login flow). The start endpoint returns a `409` carrying an
  OIDC authorize URL when step-up is required; the client redirects, and on
  return the impersonation is created with the reason carried through the OIDC
  `state`.

The reason is mandatory (non-empty) for both and is stored on the row and in the
start audit event.

## 9. Read-write with audited attribution

Impersonation is **read-write** — the admin can do anything the target can,
including mutations, so a failing write path can be reproduced. The control is
**attribution, not restriction**:

- `AuditEvent::Mutation` gains `impersonator: Option<Uuid>`. Every audited
  mutation made during an impersonation records both `actor` (the target, under
  whose identity the action ran) and `impersonator` (the real admin). The audit
  helper reads `principal.impersonator` — no per-handler change.
- No per-action confirmation dialog (it would make multi-step flows unusable).
- The start step-up (§8) is the gate; the audit trail is the accountability.

## 10. Exiting

- `POST /api/auth/impersonation/exit` deletes the overlay row for the caller's
  session and emits `impersonation.end`. The next request is the admin again.
- The 30-minute `expires_at` is the passive backstop (§5, step 5): a forgotten
  impersonation ends on its own.

## 11. `/me` and the UI

`GET /api/auth/me` gains:

```jsonc
"impersonating": {
  "target":       { "user_id": "…", "email": "…" },
  "impersonator": { "user_id": "…", "email": "…" },
  "workspace_scope": "…" | null,
  "expires_at":   "…"
} | null
```

- **Entry points.** Control Plane → Users → row action **Impersonate**
  (platform_admin). Workspace → Members → row action **Impersonate** (ws_admin;
  the workspace scope is implied by where the action is). A dialog shows the
  target, a required **reason** field, and either a password field (local) or a
  **Re-authenticate with SSO** button (OIDC).
- **Banner.** A persistent, high-contrast top banner whenever `impersonating` is
  set: *"Impersonating **{target}** · {countdown} left · [Exit]"*. It is
  unmissable so the admin never forgets they are acting as someone else. Backed
  by `/me`; a toast fires on start and exit (the global Toaster is now mounted).
- **Exit** is available in the banner and the user menu.

## 12. Guard summary

| Guard | Where |
| --- | --- |
| Cannot impersonate a platform_admin | start endpoint, target check |
| Cannot impersonate self | start endpoint |
| No nested impersonation | start endpoint (caller already impersonating → reject) + `validate` only overlays the admin session |
| ws_admin: target must be a non-admin member of the managed workspace | ws_admin start endpoint |
| ws_admin confined to the one workspace | `/me/workspaces` + `WsContext` |
| Step-up + reason at start | start endpoint |
| 30-minute cap + manual exit | `expires_at`, exit endpoint |
| Read-write, but every mutation records both parties | audit layer |
| Exit / expiry returns to the admin | `validate` + exit endpoint |

## 13. Testing

**Integration (sqlx, real DB):**
- platform_admin impersonates a user → `/me` shows `impersonating`; the resolved
  `Principal` is the target's (roles/permissions match the target, not the
  admin).
- ws_admin impersonation is confined: entering the managed workspace works;
  entering another workspace the target belongs to → `403`; `/me/workspaces`
  lists only the managed one.
- Cannot impersonate a platform_admin → `403`, nothing written.
- Cannot impersonate self; cannot nest (an impersonating session calling start →
  `403`).
- Step-up: wrong password → `401`, nothing written.
- Expiry: an overlay past `expires_at` is ignored — the next request is the admin
  (assert the resolved principal is the admin, and the row is gone).
- A mutation performed while impersonating writes an audit row carrying both
  `actor` (target) and `impersonator` (admin).
- Exit deletes the overlay and returns to the admin.

**Frontend (vitest):** banner renders from `/me` when impersonating and hides
otherwise; the countdown reads `expires_at`; Exit calls the endpoint; the
Impersonate dialog requires a reason.

**End-to-end (Playwright):** platform_admin impersonates a workspace member →
the banner appears → the nav/permissions reflect the target → Exit returns to the
admin. Gated behind the same opt-in env var convention as the other e2e specs.

## 14. Out of scope (deliberate)

- Notifying the impersonated user (decided: audit-only, no notification).
- An audit view filtered to impersonation activity (later).
- Scheduled or delegated impersonation.
- Cross-tenant impersonation beyond a ws_admin's managed workspaces.
- Read-only impersonation mode (this design is read-write with attribution).
