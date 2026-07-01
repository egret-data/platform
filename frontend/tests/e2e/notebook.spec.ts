import { test, expect, type Page } from "@playwright/test";

/**
 * Notebook execution e2e — the real money path (Notebook epic E):
 * login → create a notebook → run a Python cell on a REAL kernel → see the
 * output render live → reload → the output restores from the persisted
 * `last_output` (E-P2 recorder). Requires the full stack (see playwright.config.ts).
 */

const ADMIN_EMAIL = process.env.EGRET_E2E_EMAIL ?? "admin@egret.local";
const ADMIN_PASSWORD = process.env.EGRET_E2E_PASSWORD ?? "Admin12345!";

async function login(page: Page) {
  await page.goto("/login");
  await page.getByRole("textbox", { name: "Email" }).fill(ADMIN_EMAIL);
  await page.getByRole("textbox", { name: "Password" }).fill(ADMIN_PASSWORD);
  await page.getByRole("button", { name: "Sign in" }).click();
  // Post-login lands inside a workspace (…/explore/{ws}/…).
  await page.waitForURL(/\/explore\/[^/]+\//, { timeout: 20_000 });
}

/** Set a Monaco editor's value directly (typing into Monaco is unreliable in automation). */
async function setFirstCellSource(page: Page, code: string) {
  await page.waitForFunction(() => (window as any).monaco?.editor?.getEditors?.().length > 0, null, {
    timeout: 20_000,
  });
  await page.evaluate((src) => {
    const eds = (window as any).monaco.editor.getEditors();
    eds[0].setValue(src);
  }, code);
}

test("run a Python cell end-to-end and restore its output on reload", async ({ page }) => {
  await login(page);

  // Go to Notebooks and create a fresh one.
  await page.getByRole("link", { name: "Notebooks" }).click();
  await page.waitForURL(/\/notebooks$/);
  await page.getByRole("button", { name: "+ Notebook" }).click();
  const nbName = `e2e ${Date.now()}`;
  await page.getByRole("textbox", { name: "Name" }).fill(nbName);
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.waitForURL(/\/notebooks\/[0-9a-f-]{36}$/, { timeout: 20_000 });

  // Type a Python cell whose output is unambiguous.
  const marker = `egret-e2e-${Date.now()}`;
  await setFirstCellSource(page, `print("${marker}", 6 * 7)`);

  // Run it — this spawns a real kernel the first time (allow generous time).
  await page.getByRole("button", { name: "Run", exact: true }).first().click();

  // The output renders live in the cell.
  await expect(page.getByText(`${marker} 42`)).toBeVisible({ timeout: 60_000 });

  // Reload → the output restores from the persisted last_output (E-P2 recorder + E-P3 restore).
  await page.reload();
  await expect(page.getByText(`${marker} 42`)).toBeVisible({ timeout: 20_000 });
});

test("the notebook toolbar exposes the full run controls", async ({ page }) => {
  await login(page);
  await page.getByRole("link", { name: "Notebooks" }).click();
  await page.getByRole("button", { name: "+ Notebook" }).click();
  await page.getByRole("textbox", { name: "Name" }).fill(`e2e controls ${Date.now()}`);
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.waitForURL(/\/notebooks\/[0-9a-f-]{36}$/);

  for (const label of ["Run all", "Interrupt", "Restart", "Clear outputs"]) {
    await expect(page.getByRole("button", { name: label })).toBeVisible();
  }
  // Per-cell controls.
  await expect(page.getByRole("button", { name: "Run", exact: true }).first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Run options" }).first()).toBeVisible();
});
