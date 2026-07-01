import { type Page } from "@playwright/test";

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
