# P5b: deny and expired grants never grant admin or roles

DG debt wave 3, package P5b. It follows #810 (KI-322, KI-323).

## The rule, in one place

#810 added the clause `effect = 'allow' AND (expires_at IS NULL OR expires_at > now())` in three places. P5b moves it into one macro, `egret_auth::live_allow_grant!("<alias>")`. The macro expands to a string literal, so callers `concat!` it into a `&'static str` query, which sqlx 0.9 requires. `effective.rs`'s `principal_select!` already uses this pattern. Every read below that decides who holds a role now uses the macro, and so do #810's three sites.

Kept from #810: role *lists* show live allow grants only, and a deny grant does not take away an allow grant of the same role. The PDP enforces deny.

## Rows

1. **PDP `is_admin`** (`pdp/mod.rs`) counted a deny `platform_admin` grant.
   - `grants_for` already drops lapsed rows.
   - The decision itself was masked: a deny `platform_admin` links an unrestricted `forbid`. The flag was still wrong, and it is the exemption from the admin-graph and classification `forbid`s.
   - Fix: the flag moves into `entities::holds_platform_admin`, which counts only `allow` grants. A unit test covers it.
2. **`SessionService::ws_admin_of` / `workspace_roles_of`** ignored `effect`. Both already checked expiry.
   - Fix: both use the macro.
   - These feed the OIDC step-up re-check and the ws-impersonation target rule.
3. **`RoleRepo::platform_admin_user_ids`** ignored both `effect` and expiry.
   - Fix: it uses the macro.
   - It has no production caller (tests only); `engine_admin_user_ids_via` replaced it for the engine seed and the guards.
4. **Sweep** over every read of `access_grants`.

### Fixed in the sweep

| Read | Why it decides access |
|---|---|
| `egret_workspace::rbac::Authorizer::workspace_roles_for` | `WsContext` admits a member when this list is non-empty. A deny-only grant admitted its holder. The automation owner-membership check also uses it. |
| `WorkspaceRepo::list_for_principal` | The workspace switcher. Kept in line with `WsContext`. |
| `MemberRepo::count_ws_admins` + new `is_live_ws_admin` | The last-`ws_admin` guard is the workspace form of KI-322. It counted lapsed and deny rows, so it let the last live admin go. |
| `MemberRepo::list` platform-admin badge | Answers "is this member a platform admin". |
| `RoleRepo::effective_platform_roles_of_users` | Its doc promises the session's roles (KI-260). After #810 the session counts allow grants only. |
| `groups::keep_last_admin_memberships` | Keeps an OIDC group membership only when the group restores a live admin. Before, a deny or lapsed group grant also kept a membership, along with every other role that group carries. |
| `egret-bin init` | Bootstrap of the first admin. A lapsed or deny grant no longer counts as an existing admin. |

### Left as they are

- **Assignment inventories** list rows to manage. A deny or lapsed row is still a row an admin may remove.
  - `RoleRepo::roles_of`, `assignees_of`, `assignees_named`.
  - `MemberRepo::list` rows and `role_of`.
  - `dataapi list_endpoint_grants`.
- **Affected-set and lock reads** over-include, which is the safe side for a recompile.
  - `replace_platform_roles` (`losing`), `MemberRepo::current_roles`, `group_membership::lock_affected_by_user_delete_in_tx`.
  - `trigger::affected_services_for_group_membership`, which is also P6's file.
  - `expiry::recompile_lapsed_grants`.
- **Reads that already apply the rule or need deny rows**: `grants_for`, `AccessResolver`, `who_can_access`, `has_object_grant_in_workspace`, bi shares, perm-compile `members`/`people`/`identity_of`.
- **`AccessRequestRepo::joinable`**: any live grant still hides the workspace, a deny one too. A principal denied in a workspace is not offered a join request. This is the conservative side.
- **`engine_grants::is_platform_admin`** (perm-compile): it refuses an impersonation chain that reaches a platform admin. Over-counting refuses more, which is the safe side.
- **`username_reuse` `admins` CTE**: report only. It should mirror the engine seed set; that is listed as a follow-up.
- **`dataapi grant_endpoint_invoke`** idempotency check ignores expiry. Nothing writes expiring endpoint grants; listed as a follow-up.

## Tests

Red tests insert `access_grants` rows directly, because no API writes deny or expiring platform/workspace grants. Each test asks as the affected principal: the member through `WsContext`, and the admin through the member routes.
