# OIDC (Keycloak) as default auth for dev + e2e — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make real OIDC (local Keycloak) the default sign-in path for the dev stack and Playwright e2e, with local email/password kept as a break-glass admin login.

**Architecture:** OIDC backend + Keycloak realm already exist. This plan (1) turns OIDC on in the dev `.env`, (2) makes the login page lead with SSO and hide the local form behind a break-glass disclosure, (3) adds a small idempotent CLI helper to bind an IdP group to a workspace (so an SSO member lands in a working workspace — the "admin grants access" step), and (4) rewrites the notebook e2e to authenticate through the real Keycloak UI.

**Tech Stack:** Rust (axum, sqlx, clap) backend; React 18 + Vite + Vitest frontend; Playwright e2e; Keycloak 26 (dev compose); Postgres.

> Bilingual: English canonical; VI reference at `docs/vi/plans/2026-07-01-oidc-default-dev-e2e.md`.

**Spec:** `docs/en/specs/2026-07-01-oidc-keycloak-default-dev-e2e-design.md`

---

## File Structure

- `.env`, `.env.example` — add `EGRET_OIDC__*` + `EGRET_SECURITY__SECRET_KEY` (dev values); keep local-admin vars as break-glass. *(Task 1)*
- `crates/egret-config/src/lib.rs` — add one unit test proving a dev-representative OIDC config validates. *(Task 1)*
- `crates/egret-bin/src/lib.rs` — new reusable `grant_group_to_workspace()` helper. *(Task 2)*
- `crates/egret-bin/tests/provisioning.rs` — DB-gated integration test for the helper. *(Task 2)*
- `crates/egret-bin/src/cli.rs`, `crates/egret-bin/src/main.rs` — new `workspace add-group` CLI subcommand calling the helper. *(Task 3)*
- `frontend/src/auth/LoginPage.tsx` — SSO-primary + break-glass disclosure. *(Task 4)*
- `frontend/src/auth/__tests__/LoginPage.test.tsx` — new Vitest coverage. *(Task 4)*
- `frontend/tests/e2e/helpers.ts` — new `loginViaOidc()` / `loginLocalAdmin()` helpers. *(Task 5)*
- `frontend/tests/e2e/notebook.spec.ts` — authenticate via OIDC. *(Task 5)*
- `frontend/tests/e2e/README.md` — document the SSO setup + group binding. *(Task 5)*

---

## Task 1: Enable OIDC in the dev environment

**Files:**
- Modify: `egret/.env.example`
- Modify: `egret/.env`
- Test: `egret/crates/egret-config/src/lib.rs` (add a unit test)

Context: `OidcConfig` fields are `enabled, issuer_url, client_id, client_secret, redirect_url, groups_claim` (env prefix `EGRET_OIDC__`, double underscore for nesting). `Config::validate()` requires `security.secret_key` (base64 ≥64 bytes) plus `issuer_url` + `client_id` when `oidc.enabled`. Keycloak runs on host `:8081` (`deploy/docker-compose.dev.yml`), realm `egret`, client `egret` / secret `egret-secret`. The browser lives on the Vite origin `:5173`, so the redirect must return there.

- [ ] **Step 1: Add OIDC + security vars to `.env.example`**

Append to `egret/.env.example`:

```bash
# OIDC / SSO (local Keycloak — see deploy/docker-compose.dev.yml). SSO is the
# default dev sign-in; the local admin below stays as a break-glass login.
EGRET_OIDC__ENABLED=true
EGRET_OIDC__ISSUER_URL=http://localhost:8081/realms/egret
EGRET_OIDC__CLIENT_ID=egret
EGRET_OIDC__CLIENT_SECRET=egret-secret
# Must return to the Vite origin (:5173) so the session cookie lands on the app.
EGRET_OIDC__REDIRECT_URL=http://localhost:5173/api/auth/oidc/callback
EGRET_OIDC__GROUPS_CLAIM=groups
# Dev-only key for encrypting the OIDC state cookie (base64, >=64 bytes). NOT a secret.
EGRET_SECURITY__SECRET_KEY=ZGV2LW9ubHktc2VjcmV0LWtleS1kby1ub3QtdXNlLWluLXByb2R1Y3Rpb24tMDAwMDAwMDAwMDA=

# Break-glass local admin (used to elevate SSO users to admin later)
EGRET_ADMIN_EMAIL=admin@egret.local
EGRET_ADMIN_PASSWORD=Admin12345!
```

- [ ] **Step 2: Mirror the same vars into `.env`**

Add the identical `EGRET_OIDC__*` and `EGRET_SECURITY__SECRET_KEY` lines to `egret/.env` (it already has the admin + database + MinIO vars). Keep existing lines.

- [ ] **Step 3: Write a config test proving the dev OIDC config validates**

Add to `crates/egret-config/src/lib.rs` (in the existing `#[cfg(test)] mod tests`), mirroring the existing `validate_requires_secret_key_when_oidc_enabled` test:

```rust
#[test]
fn validate_ok_for_dev_oidc_config() {
    let mut cfg = Config::default();
    cfg.security.secret_key =
        "ZGV2LW9ubHktc2VjcmV0LWtleS1kby1ub3QtdXNlLWluLXByb2R1Y3Rpb24tMDAwMDAwMDAwMDA=".into();
    cfg.oidc.enabled = true;
    cfg.oidc.issuer_url = "http://localhost:8081/realms/egret".into();
    cfg.oidc.client_id = "egret".into();
    cfg.oidc.client_secret = "egret-secret".into();
    cfg.oidc.redirect_url = "http://localhost:5173/api/auth/oidc/callback".into();
    assert!(cfg.validate().is_ok(), "dev OIDC config should validate");
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p egret-config validate_ok_for_dev_oidc_config -- --nocapture`
Expected: PASS. (If it fails because the sample key is <64 bytes once base64-decoded, regenerate with `head -c 64 /dev/urandom | base64` and update both the test and the `.env*` files to match.)

- [ ] **Step 5: Commit**

```bash
git add egret/.env.example egret/crates/egret-config/src/lib.rs
git commit -m "feat(auth): enable OIDC by default in dev env"
```
(Note: `.env` is gitignored — do not add it; only `.env.example` is committed.)

---

## Task 2: `grant_group_to_workspace` helper

**Files:**
- Modify: `crates/egret-bin/src/lib.rs`
- Test: `crates/egret-bin/tests/provisioning.rs` (create)

Context: An OIDC user JIT-provisions as a plain member with no workspace. Binding the IdP group `data-team` to a workspace as `ws_editor` (which grants `notebook:write` per migration `0037_notebooks.sql`) lets any SSO member of that group land in a working workspace. `MemberRepo::add(ws, ptype, pid, role, by)` is idempotent (`ON CONFLICT ... DO UPDATE`). Group upsert matches `GroupRepo::upsert_oidc_group` (`INSERT ... ON CONFLICT (name) DO UPDATE`). RBAC unions group memberships via `user_groups` (`Authorizer::workspace_permissions`).

- [ ] **Step 1: Write the failing integration test**

Create `crates/egret-bin/tests/provisioning.rs`:

```rust
//! DB-gated: self-skips without DATABASE_URL. Verifies that granting an IdP
//! group access to a workspace makes a user in that group inherit its perms.

use egret_auth::groups::GroupRepo;
use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn grant_group_binds_workspace_perms() {
    let Some(db) = connect().await else { return };
    let users = UserRepo::new(db.clone());

    // Admin acts as `added_by`.
    let admin_email = format!("{}@test.local", uuid::Uuid::new_v4());
    let admin_id = users
        .insert_local(&admin_email, "Admin", &hash_password("pw").unwrap())
        .await
        .unwrap();

    // A workspace to grant into.
    let slug = uniq("ws");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Test WS", admin_id)
        .await
        .unwrap();

    // An OIDC user in group `data-team`.
    let group = uniq("data-team");
    let member_id: UserId = users
        .insert_oidc(&uniq("sub"), &format!("{}@test.local", uuid::Uuid::new_v4()), "Member")
        .await
        .unwrap();
    let groups = GroupRepo::new(db.clone());
    groups.upsert_oidc_group(&group).await.unwrap();
    groups
        .sync_user_oidc_groups(member_id, &[group.clone()])
        .await
        .unwrap();

    // Before the grant: no workspace perms.
    let authz = Authorizer::new(db.clone());
    let before = authz.workspace_permissions(member_id, ws.id).await.unwrap();
    assert!(!before.contains("notebook:write"), "no perms before grant");

    // Act.
    egret_bin::grant_group_to_workspace(&db, &slug, &group, "ws_editor", &admin_email)
        .await
        .unwrap();

    // After: the member inherits ws_editor perms via the group.
    let after = authz.workspace_permissions(member_id, ws.id).await.unwrap();
    assert!(after.contains("notebook:write"), "group grant confers notebook:write");
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bin --test provisioning grant_group_binds_workspace_perms -- --nocapture`
Expected: FAIL — `egret_bin::grant_group_to_workspace` does not exist (compile error). (Requires `just up` running so Postgres is available; otherwise the test self-skips and this step is inconclusive.)

- [ ] **Step 3: Implement the helper**

Add to `crates/egret-bin/src/lib.rs` (add any missing `use` at the top of the file):

```rust
use anyhow::Context;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

/// Grant an IdP (OIDC) group access to a workspace, idempotently. Upserts the
/// group by name, resolves the workspace by slug and the `added_by` admin by
/// email, then records a group workspace-membership with `role`.
///
/// Faithful to the "local admin grants access" model: an operator runs this to
/// bind an IdP group to a workspace so its SSO members inherit that role.
pub async fn grant_group_to_workspace(
    db: &Db,
    workspace_slug: &str,
    group_name: &str,
    role: &str,
    admin_email: &str,
) -> anyhow::Result<()> {
    // Upsert the group (matches egret_auth::groups::GroupRepo::upsert_oidc_group).
    let group_id: Uuid = sqlx::query_scalar(
        "INSERT INTO groups (name, source) VALUES ($1, 'oidc') \
         ON CONFLICT (name) DO UPDATE SET name = groups.name RETURNING id",
    )
    .bind(group_name)
    .fetch_one(&db.pool)
    .await
    .context("upserting group")?;

    let ws = WorkspaceRepo::new(db.clone())
        .get_by_slug(workspace_slug)
        .await
        .context("looking up workspace")?
        .ok_or_else(|| anyhow::anyhow!("workspace not found: {workspace_slug}"))?;

    let admin = UserRepo::new(db.clone())
        .find_by_email(admin_email)
        .await
        .context("looking up admin")?
        .ok_or_else(|| anyhow::anyhow!("admin user not found: {admin_email}"))?;

    MemberRepo::new(db.clone())
        .add(ws.id, "group", group_id, role, admin.id)
        .await
        .context("adding group workspace-membership")?;
    Ok(())
}
```

- [ ] **Step 4: Run it to confirm it passes**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bin --test provisioning grant_group_binds_workspace_perms -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Format + commit**

```bash
cargo fmt -p egret-bin -p egret-config
git add egret/crates/egret-bin/src/lib.rs egret/crates/egret-bin/tests/provisioning.rs
git commit -m "feat(bin): grant_group_to_workspace helper for IdP group binding"
```

---

## Task 3: `workspace add-group` CLI subcommand

**Files:**
- Modify: `crates/egret-bin/src/cli.rs`
- Modify: `crates/egret-bin/src/main.rs`

Context: `cli.rs` has a `Command` enum (`Serve`, `Migrate`, `Init`, `Catalog { action }`) and a `CatalogAction` subcommand group with `#[arg(long)]` flags. `main.rs` dispatches on `Command` (`Command::Init { force } => init(...)`, `Command::Catalog { action } => catalog_cmd(...)`). Mirror that shape.

- [ ] **Step 1: Add the subcommand to `cli.rs`**

In `crates/egret-bin/src/cli.rs`, add a variant to `Command`:

```rust
    /// Workspace administration (membership).
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
```

And add the action enum (near `CatalogAction`):

```rust
/// Workspace membership operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Grant an IdP group a role in a workspace (idempotent).
    AddGroup {
        /// Workspace slug (e.g. "engineering").
        #[arg(long)]
        workspace: String,
        /// IdP group name as it appears in the `groups` claim (e.g. "data-team").
        #[arg(long)]
        group: String,
        /// Workspace role: ws_admin | ws_editor | ws_viewer.
        #[arg(long, default_value = "ws_editor")]
        role: String,
    },
}
```

- [ ] **Step 2: Dispatch it in `main.rs`**

In `crates/egret-bin/src/main.rs`, add to the `match command` block (alongside `Command::Catalog`):

```rust
        Command::Workspace { action } => workspace_cmd(config, action).await,
```

And add the handler (near `catalog_cmd`), reusing `connect_db` and the Task 2 helper:

```rust
async fn workspace_cmd(config: Config, action: WorkspaceAction) -> anyhow::Result<()> {
    let db = connect_db(&config).await?;
    match action {
        WorkspaceAction::AddGroup { workspace, group, role } => {
            let admin_email =
                std::env::var("EGRET_ADMIN_EMAIL").unwrap_or_else(|_| "admin@egret.local".into());
            egret_bin::grant_group_to_workspace(&db, &workspace, &group, &role, &admin_email)
                .await?;
            println!("Granted group '{group}' role '{role}' in workspace '{workspace}'");
            Ok(())
        }
    }
}
```

Add `use egret_bin::cli::WorkspaceAction;` (or the crate-local path already used for `CatalogAction`) to the imports in `main.rs`.

- [ ] **Step 3: Build**

Run: `cargo build -p egret-bin`
Expected: builds clean (no warnings-as-errors issues).

- [ ] **Step 4: Manual smoke (requires `just up` + an admin + a workspace)**

Run: `cargo run -p egret-bin -- workspace add-group --workspace engineering --group data-team --role ws_editor`
Expected: prints `Granted group 'data-team' role 'ws_editor' in workspace 'engineering'` (or a clear "workspace not found" if the slug differs — adjust to your seeded workspace slug).

- [ ] **Step 5: Commit**

```bash
cargo fmt -p egret-bin
git add egret/crates/egret-bin/src/cli.rs egret/crates/egret-bin/src/main.rs
git commit -m "feat(bin): 'workspace add-group' CLI to bind an IdP group to a workspace"
```

---

## Task 4: Login page — SSO primary, local break-glass

**Files:**
- Modify: `frontend/src/auth/LoginPage.tsx`
- Test: `frontend/src/auth/__tests__/LoginPage.test.tsx` (create)

Context: `LoginPage.tsx` uses `useAuthConfig()` (`GET /api/auth/config` → `{ oidc_enabled }`) and `useLogin()` from `@/api/hooks`. Today it always shows the email/password form and appends an SSO button when `oidc_enabled`. Follow the existing `__tests__` mocking style for `@/api/hooks`.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/auth/__tests__/LoginPage.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { describe, it, expect, vi } from "vitest";
import { LoginPage } from "../LoginPage";

vi.mock("@/api/hooks", () => ({
  useAuthConfig: () => ({ data: { oidc_enabled: true } }),
  useLogin: () => ({ mutateAsync: vi.fn(), isPending: false }),
}));

function renderPage() {
  render(
    <MemoryRouter>
      <LoginPage />
    </MemoryRouter>,
  );
}

describe("LoginPage with OIDC enabled", () => {
  it("leads with SSO and hides the local form until break-glass is expanded", async () => {
    renderPage();
    // SSO is present and primary.
    expect(screen.getByRole("link", { name: /sign in with sso/i })).toBeInTheDocument();
    // Local password field is NOT rendered initially.
    expect(screen.queryByPlaceholderText("Password")).not.toBeInTheDocument();
    // Expanding the break-glass disclosure reveals it.
    await userEvent.click(screen.getByRole("button", { name: /admin sign-in/i }));
    expect(screen.getByPlaceholderText("Password")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd frontend && npx vitest run src/auth/__tests__/LoginPage.test.tsx`
Expected: FAIL — the password field is currently rendered immediately (query finds it), and/or no "Admin sign-in" toggle exists.

- [ ] **Step 3: Implement the SSO-primary layout**

Replace `frontend/src/auth/LoginPage.tsx` with:

```tsx
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card } from "@/components/ui/card";
import { useAuthConfig, useLogin } from "@/api/hooks";
import { ApiError } from "@/api/client";

export function LoginPage() {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [showLocal, setShowLocal] = useState(false);
  const login = useLogin();
  const cfg = useAuthConfig();
  const navigate = useNavigate();

  const oidcEnabled = !!cfg.data?.oidc_enabled;
  // When OIDC is on, SSO is primary and the local form is break-glass (hidden
  // until expanded). When OIDC is off, the local form is the only option.
  const localVisible = !oidcEnabled || showLocal;

  const onSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      await login.mutateAsync({ email, password });
      navigate("/", { replace: true });
    } catch (err) {
      if (err instanceof ApiError && err.status === 429) {
        setError("Too many attempts. Please wait a few minutes and try again.");
      } else {
        setError("Invalid email or password");
      }
    }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <Card className="w-full max-w-sm p-6">
        <h1 className="mb-4 text-xl font-semibold">Sign in to Egret</h1>

        {oidcEnabled && (
          <a href="/api/auth/oidc/login" className="block">
            <Button className="w-full">Sign in with SSO</Button>
          </a>
        )}

        {localVisible && (
          <form onSubmit={onSubmit} className="mt-3 space-y-3">
            <Input type="email" placeholder="Email" value={email} onChange={(e) => setEmail(e.target.value)} required />
            <Input type="password" placeholder="Password" value={password} onChange={(e) => setPassword(e.target.value)} required />
            {error && <p className="text-sm text-destructive">{error}</p>}
            <Button type="submit" variant={oidcEnabled ? "outline" : "default"} className="w-full" disabled={login.isPending}>
              Sign in
            </Button>
          </form>
        )}

        {oidcEnabled && !showLocal && (
          <button
            type="button"
            onClick={() => setShowLocal(true)}
            className="mt-3 block w-full text-center text-xs text-muted-foreground hover:underline"
          >
            Admin sign-in (break-glass)
          </button>
        )}
      </Card>
    </div>
  );
}
```

- [ ] **Step 4: Run it to confirm it passes**

Run: `cd frontend && npx vitest run src/auth/__tests__/LoginPage.test.tsx`
Expected: PASS.

- [ ] **Step 5: Guard the OIDC-disabled case + run the full auth suite**

Run: `cd frontend && npx vitest run src/auth`
Expected: PASS, including the existing `LoginRedirect.test.tsx` / `AuthProvider.test.tsx`. If any existing test assumed the password field is always present, update its mock so `useAuthConfig` returns `{ oidc_enabled: false }` for that case (local form is unconditionally visible then).

- [ ] **Step 6: Commit**

```bash
git add frontend/src/auth/LoginPage.tsx frontend/src/auth/__tests__/LoginPage.test.tsx
git commit -m "feat(auth-ui): SSO-primary login with break-glass local admin form"
```

---

## Task 5: Rewrite the notebook e2e to authenticate via Keycloak

**Files:**
- Create: `frontend/tests/e2e/helpers.ts`
- Modify: `frontend/tests/e2e/notebook.spec.ts`
- Modify: `frontend/tests/e2e/README.md`

Context: e2e is a manual/opt-in smoke against a live stack (`baseURL` `:5173`). Today `notebook.spec.ts` has a local `login()` that fills the email/password form. The SSO button is an `<a href="/api/auth/oidc/login">`; clicking it triggers a full-page redirect → Keycloak login page → callback → the app resolves the user's single workspace and lands on `/explore/{ws}/`. Realm test user: `tester@egret.local` / `pw` in group `data-team`.

- [ ] **Step 1: Add the auth helpers**

Create `frontend/tests/e2e/helpers.ts`:

```ts
import { type Page, expect } from "@playwright/test";

const OIDC_USER = process.env.EGRET_E2E_OIDC_USER ?? "tester@egret.local";
const OIDC_PASSWORD = process.env.EGRET_E2E_OIDC_PASSWORD ?? "pw";
const ADMIN_EMAIL = process.env.EGRET_E2E_EMAIL ?? "admin@egret.local";
const ADMIN_PASSWORD = process.env.EGRET_E2E_PASSWORD ?? "Admin12345!";

/** Sign in through the real Keycloak login page (SSO). Lands inside a workspace. */
export async function loginViaOidc(page: Page): Promise<void> {
  await page.goto("/login");
  await page.getByRole("link", { name: /sign in with sso/i }).click();
  // Now on the Keycloak-hosted login form (issuer origin).
  await page.locator("#username, input[name='username']").first().fill(OIDC_USER);
  await page.locator("#password, input[name='password']").first().fill(OIDC_PASSWORD);
  await page.locator("#kc-login, button[type='submit'], input[type='submit']").first().click();
  // Redirects back through the callback into a workspace.
  await page.waitForURL(/\/explore\/[^/]+\//, { timeout: 30_000 });
}

/** Break-glass: expand the local form and sign in as the platform admin. */
export async function loginLocalAdmin(page: Page): Promise<void> {
  await page.goto("/login");
  const adminToggle = page.getByRole("button", { name: /admin sign-in/i });
  if (await adminToggle.isVisible().catch(() => false)) await adminToggle.click();
  await page.getByPlaceholder("Email").fill(ADMIN_EMAIL);
  await page.getByPlaceholder("Password").fill(ADMIN_PASSWORD);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await page.waitForURL(/\/explore\/[^/]+\//, { timeout: 20_000 });
}
```

- [ ] **Step 2: Switch `notebook.spec.ts` to OIDC login**

In `frontend/tests/e2e/notebook.spec.ts`: delete the local `ADMIN_EMAIL`/`ADMIN_PASSWORD` consts and the local `login()` function, add `import { loginViaOidc } from "./helpers";`, and replace both `await login(page);` calls with `await loginViaOidc(page);`. Leave `setFirstCellSource` and the test bodies unchanged.

- [ ] **Step 3: Document the SSO setup in the e2e README**

Add a section to `frontend/tests/e2e/README.md` after the Prerequisites list:

```markdown
### SSO login (default)

The specs authenticate through the real Keycloak in `docker-compose.dev.yml`
(`:8081`, realm `egret`, user `tester@egret.local` / `pw` in group `data-team`).
The backend must run with the OIDC env from `.env.example`
(`EGRET_OIDC__ENABLED=true`, `EGRET_OIDC__REDIRECT_URL=http://localhost:5173/api/auth/oidc/callback`, …).

For `tester` to land in a workspace that has a provisioned warehouse, bind the
`data-team` group to that workspace once (the "admin grants access" step):

```bash
cargo run -p egret-bin -- workspace add-group \
  --workspace <your-workspace-slug> --group data-team --role ws_editor
```

Override SSO creds with `EGRET_E2E_OIDC_USER` / `EGRET_E2E_OIDC_PASSWORD`.
```

- [ ] **Step 4: Type-check the e2e sources**

Run: `cd frontend && npx tsc --noEmit -p tsconfig.json`
Expected: no errors from `tests/e2e/helpers.ts` or `notebook.spec.ts`. (Playwright is not run in CI; this catches type mistakes cheaply.)

- [ ] **Step 5: Commit**

```bash
git add frontend/tests/e2e/helpers.ts frontend/tests/e2e/notebook.spec.ts frontend/tests/e2e/README.md
git commit -m "test(e2e): authenticate the notebook smoke via real Keycloak SSO"
```

---

## Task 6: Full verification

**Files:** none (verification only).

- [ ] **Step 1: Bring up the stack with OIDC**

```bash
cd egret
just up   # includes Keycloak on :8081
cargo run -p egret-bin -- init   # ensure the break-glass admin exists
# start backend with the OIDC env loaded (justfile has set dotenv-load):
PATH="/tmp/kernelenv/bin:$PATH" just serve
# in another shell:
cd frontend && npm run dev
```

- [ ] **Step 2: Bind the group + provision the warehouse**

Provision a warehouse for your workspace via the UI (Storage → Add connection → Catalog → Add warehouse, per the e2e README), then:

```bash
cargo run -p egret-bin -- workspace add-group --workspace <slug> --group data-team --role ws_editor
```

- [ ] **Step 3: Manual browser check (SSO + break-glass)**

- Open `http://localhost:5173/login` → confirm "Sign in with SSO" is primary and the password form is hidden.
- Click SSO → Keycloak → log in `tester@egret.local` / `pw` → confirm you land in `/explore/{ws}/`, can create a notebook, and Run prints output.
- Log out → `/login` → click "Admin sign-in (break-glass)" → log in `admin@egret.local` / `Admin12345!` → confirm it still works.

- [ ] **Step 4: Automated e2e**

Run: `cd frontend && npm run test:e2e`
Expected: `notebook.spec.ts` green (login now via Keycloak).

- [ ] **Step 5: Full local CI (CI is out of quota — run locally before merge)**

```bash
cd egret
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test --workspace
cd frontend && npm run lint && npx tsc --noEmit && npx vitest run && npm run build
```
Expected: all green. (Run the real-Trino/catalog manual integration test too if any authz path changed — not applicable here.)

- [ ] **Step 6: Finish the branch**

Use `superpowers:finishing-a-development-branch` to open the PR / merge.

---

## Notes / risks

- **Cookie/port:** `EGRET_OIDC__REDIRECT_URL` must point at `:5173` (the Vite origin) or the session cookie lands on the wrong origin and login silently fails. If e2e ever serves the built app from `:8080`, move `REDIRECT_URL` and Playwright `baseURL` together.
- **Keycloak markup:** `loginViaOidc` uses resilient selectors (`name=username`/`name=password`, submit) and a 30s wait for KC cold start. If KC's theme changes, update the selectors.
- **Signature assumptions to verify while implementing:** `User { id, email }`, `UserRepo::find_by_email`/`insert_oidc`, `MemberRepo::new`, `WorkspaceRepo::create(slug, name, created_by)`, and that `egret-bin` exposes `cli::WorkspaceAction` — all confirmed against the current tree, but re-check if the crate has drifted.
- **Realm `redirectUris` include `*`** — fine for dev; a production realm must pin exact URIs (out of scope, noted for the prod-hardening spec).
```
